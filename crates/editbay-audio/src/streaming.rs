use crate::{
    DeviceProfile, MonitorRoute, SoundRenderBudget, SoundRenderer,
    monitor::MonitorMatrix,
    sample_clock::SampleClock,
    transport::{self, Control, Reader, State, Writer},
};
use cpal::{
    FromSample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use editbay_core::{DocumentVersion, EvaluationSnapshot, Project, SoundBudget, SoundSnapshot};
use editbay_media::{
    Cancellation,
    pcm_worker::{PcmWorker, PcmWorkerBudget},
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use uuid::Uuid;

const BLOCK: u32 = 4096;
pub(crate) const CAPACITY: u32 = 16384;
type Result<T> = std::result::Result<T, String>;

/// An exact seek in composition frames or a previously observed device sample.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum PlaybackStart {
    Frame(u64),
    Sample { position: u64, sample_rate: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackPhase {
    Preparing,
    Playing,
    Finished,
    Failed,
    Stopping,
    Stopped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamingStatus {
    pub version: DocumentVersion,
    pub phase: PlaybackPhase,
    pub position_samples: Option<u64>,
    pub start_sample: Option<u64>,
    pub end_sample: Option<u64>,
    pub device: Option<DeviceProfile>,
    pub route: MonitorRoute,
    pub worker_pid: Option<u32>,
    pub device_worker_pid: Option<u32>,
    pub callbacks: u64,
    pub reported_latency_ns: u64,
    pub prepared_frames: u64,
    pub prepared_capacity_frames: u32,
    pub clipped_monitor_samples: u64,
    pub error: Option<String>,
}

struct Ready {
    clock: Arc<SampleClock>,
    control: Control,
    origin: Instant,
    end: u64,
    first: u64,
    device: DeviceProfile,
    worker_pid: Option<u32>,
    clipped: Arc<AtomicU64>,
}

enum Event {
    Ready(Ready),
    Finished,
    Stopped,
    Failed(String),
}

struct Request {
    project: Arc<Project>,
    composition: Uuid,
    start: PlaybackStart,
    route: MonitorRoute,
    cancel: Cancellation,
    stop: Arc<AtomicBool>,
}

/// One asynchronous playback lifetime with privately owned prepared sound.
pub(crate) struct LocalPlayback {
    version: DocumentVersion,
    route: MonitorRoute,
    cancel: Cancellation,
    stop: Arc<AtomicBool>,
    events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
    ready: Option<Ready>,
    phase: PlaybackPhase,
    error: Option<String>,
}

impl LocalPlayback {
    /// Prepare and play a captured composition without blocking the caller.
    /// `project`, `composition`, `start` and `route` select saved content,
    /// the seek boundary and an explicit listening route. Returns an owned job;
    /// device discovery, graph work, codec IO and stream teardown run off the UI.
    pub fn start(
        project: Arc<Project>,
        composition: Uuid,
        start: PlaybackStart,
        route: MonitorRoute,
    ) -> Result<Self> {
        let version = DocumentVersion::of(&project);
        let cancel = Cancellation::new().map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, events) = mpsc::sync_channel(4);
        let request = Request {
            project,
            composition,
            start,
            route,
            cancel: cancel.clone(),
            stop: stop.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("editbay-streaming-sound".into())
            .spawn(move || {
                let result = run(&request, &sender);
                let event = if request.stop.load(Ordering::Acquire) {
                    Event::Stopped
                } else {
                    match result {
                        Ok(()) => Event::Finished,
                        Err(error) => Event::Failed(error),
                    }
                };
                let _ = sender.try_send(event);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            version,
            route,
            cancel,
            stop,
            events,
            thread: Some(thread),
            ready: None,
            phase: PlaybackPhase::Preparing,
            error: None,
        })
    }

    /// Read live playback state without waiting for the device or preparation.
    /// Takes no arguments; returns a bounded event drain and an atomic clock
    /// observation. Device time is an estimate, not physical speaker timing.
    pub fn status(&mut self) -> StreamingStatus {
        for _ in 0..4 {
            match self.events.try_recv() {
                Ok(Event::Ready(ready)) => {
                    self.ready = Some(ready);
                    self.phase = PlaybackPhase::Playing;
                }
                Ok(Event::Finished) => self.phase = PlaybackPhase::Finished,
                Ok(Event::Stopped) => self.phase = PlaybackPhase::Stopped,
                Ok(Event::Failed(error)) => {
                    self.error = Some(error);
                    self.phase = PlaybackPhase::Failed;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if matches!(
                        self.phase,
                        PlaybackPhase::Preparing | PlaybackPhase::Playing
                    ) {
                        self.error =
                            Some("Sound preparation stopped without a final result".into());
                        self.phase = PlaybackPhase::Failed;
                    }
                    break;
                }
            }
        }
        if self.stop.load(Ordering::Acquire) && !self.is_finished() {
            self.phase = PlaybackPhase::Stopping;
        }
        let (position, end, device, worker_pid, callbacks, latency, prepared, clipped) =
            if let Some(ready) = &self.ready {
                let (callbacks, latency) = ready.clock.counters();
                if self.phase == PlaybackPhase::Playing
                    && let Some(error) = fault(ready.control.state())
                {
                    self.phase = PlaybackPhase::Failed;
                    self.error = Some(error.into());
                    self.cancel.cancel();
                }
                (
                    Some(ready.clock.position(elapsed_ns(ready.origin))),
                    Some(ready.end),
                    Some(ready.device.clone()),
                    ready.worker_pid,
                    callbacks,
                    latency,
                    ready.control.prepared(),
                    ready.clipped.load(Ordering::Relaxed),
                )
            } else {
                (None, None, None, None, 0, 0, 0, 0)
            };
        StreamingStatus {
            version: self.version,
            phase: self.phase,
            position_samples: position,
            start_sample: self.ready.as_ref().map(|ready| ready.first),
            end_sample: end,
            device,
            route: self.route,
            worker_pid,
            device_worker_pid: None,
            callbacks,
            reported_latency_ns: latency,
            prepared_frames: prepared,
            prepared_capacity_frames: CAPACITY,
            clipped_monitor_samples: clipped,
            error: self.error.clone(),
        }
    }

    /// Silence and cancel this exact playback lifetime without joining it.
    /// Takes no arguments; returns immediately after setting shared atomic flags.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(ready) = &self.ready {
            ready.control.stop(State::Cancelled);
        }
        self.cancel.cancel();
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }

    /// Inspect worker retirement without blocking input.
    /// Takes no arguments and returns whether device/codec teardown has ended.
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Reap an already-finished playback worker.
    /// Takes no arguments; returns false while device/codec teardown is active.
    pub fn reap(&mut self) -> bool {
        if !self.is_finished() {
            return false;
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            self.phase = PlaybackPhase::Failed;
            self.error = Some("Sound worker panicked during playback".into());
        }
        true
    }
}

impl Drop for LocalPlayback {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Producer {
    sound: Arc<SoundSnapshot>,
    renderer: SoundRenderer<PcmWorker>,
    matrix: MonitorMatrix,
    writer: Writer,
    control: Control,
    cursor: u64,
    end: u64,
    channels: usize,
    scratch: Vec<f32>,
    clipped: Arc<AtomicU64>,
}

impl Producer {
    fn fill(&mut self) -> Result<()> {
        while self.cursor < self.end {
            let frames = (self.end - self.cursor).min(u64::from(BLOCK)) as u32;
            if self.control.prepared() + u64::from(frames) > u64::from(CAPACITY) {
                break;
            }
            let plan = self
                .sound
                .prepare(self.cursor, frames)
                .map_err(|e| e.to_string())?;
            let result = self.renderer.render(&plan).map_err(|e| e.to_string())?;
            self.renderer
                .validate_result(&result)
                .map_err(|e| e.to_string())?;
            self.scratch.resize(frames as usize * self.channels, 0.);
            let clipped = self
                .matrix
                .apply(result.sound().samples(), &mut self.scratch)?;
            if !self.writer.push(self.cursor, &self.scratch)? {
                return Err("Prepared sound capacity changed unexpectedly".into());
            }
            self.clipped.fetch_add(clipped, Ordering::Relaxed);
            self.cursor += u64::from(frames);
        }
        Ok(())
    }
}

fn run(request: &Request, sender: &SyncSender<Event>) -> Result<()> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("No sound output device is available")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let profile = DeviceProfile {
        name: device.name().map_err(|e| e.to_string())?,
        sample_rate: supported.sample_rate().0,
        channels: supported.channels(),
        sample_format: format!("{:?}", supported.sample_format()),
    };
    let evaluation =
        Arc::new(EvaluationSnapshot::new(request.project.clone()).map_err(|e| e.to_string())?);
    let composition = request
        .project
        .compositions
        .iter()
        .find(|c| c.id == request.composition)
        .ok_or("Playback composition is absent")?;
    let first = match request.start {
        PlaybackStart::Frame(frame) => {
            if frame >= composition.duration {
                return Err("Playback start is outside the sequence".into());
            }
            u64::try_from(
                (u128::from(frame)
                    * u128::from(profile.sample_rate)
                    * u128::from(composition.frame_rate.denominator))
                .div_ceil(u128::from(composition.frame_rate.numerator)),
            )
            .map_err(|e| e.to_string())?
        }
        PlaybackStart::Sample {
            position,
            sample_rate,
        } => {
            if !(8000..=384000).contains(&sample_rate) {
                return Err("Invalid paused sample rate".into());
            }
            u64::try_from(
                (u128::from(position) * u128::from(profile.sample_rate))
                    .div_ceil(u128::from(sample_rate)),
            )
            .map_err(|e| e.to_string())?
        }
    };
    let sound = Arc::new(
        SoundSnapshot::at_output_rate(
            evaluation.clone(),
            request.composition,
            profile.sample_rate,
            SoundBudget::default(),
        )
        .map_err(|e| e.to_string())?,
    );
    let end = sound.duration_samples();
    if first >= end {
        return Err("Playback start is outside the sequence sound".into());
    }
    let matrix = MonitorMatrix::new(sound.profile(), profile.channels, request.route)?;
    let provider = PcmWorker::new(
        &std::env::current_exe().map_err(|e| e.to_string())?,
        evaluation,
        PcmWorkerBudget::default(),
        request.cancel.clone(),
    )
    .map_err(|e| e.to_string())?;
    let worker_pid = provider.process_id();
    let renderer =
        SoundRenderer::with_provider(sound.clone(), provider, SoundRenderBudget::default())
            .map_err(|e| e.to_string())?;
    let (control, writer, reader) =
        transport::transport(first, end, usize::from(profile.channels), CAPACITY)?;
    let clock = Arc::new(SampleClock::new(profile.sample_rate, first, end)?);
    let clipped = Arc::new(AtomicU64::new(0));
    let mut producer = Producer {
        sound,
        renderer,
        matrix,
        writer,
        control: control.clone(),
        cursor: first,
        end,
        channels: usize::from(profile.channels),
        scratch: Vec::with_capacity(BLOCK as usize * usize::from(profile.channels)),
        clipped: clipped.clone(),
    };
    producer.fill()?;
    if request.cancel.is_cancelled() {
        return Err("Sound preparation cancelled".into());
    }
    let origin = Instant::now();
    let callback = Callback {
        reader,
        control: control.clone(),
        clock: clock.clone(),
        cancel: request.cancel.clone(),
        origin,
        cursor: 0,
        end,
    };
    let mut config: cpal::StreamConfig = supported.clone().into();
    if let cpal::SupportedBufferSize::Range { min, max } = supported.buffer_size() {
        config.buffer_size = cpal::BufferSize::Fixed(512u32.clamp(*min, *max));
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, callback),
        cpal::SampleFormat::F64 => build::<f64>(&device, &config, callback),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, callback),
        cpal::SampleFormat::I32 => build::<i32>(&device, &config, callback),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, callback),
        other => Err(format!("Unsupported device sample format {other:?}")),
    }?;
    sender
        .try_send(Event::Ready(Ready {
            clock: clock.clone(),
            control: control.clone(),
            origin,
            end,
            first,
            device: profile,
            worker_pid,
            clipped,
        }))
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    let mut checked = Instant::now();
    let result = (|| {
        loop {
            if let Some(error) = fault(control.state()) {
                return Err(error.into());
            }
            if request.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            if request.cancel.is_cancelled() {
                return Err("Sound worker cancelled before playback finished".into());
            }
            if clock.position(elapsed_ns(origin)) >= end {
                return Ok(());
            }
            producer
                .fill()
                .map_err(|error| fault(control.state()).map_or(error, str::to_owned))?;
            if checked.elapsed() >= Duration::from_millis(100) {
                producer.renderer.poll().map_err(|e| e.to_string())?;
                checked = Instant::now();
            }
            std::thread::park_timeout(Duration::from_millis(2));
        }
    })();
    control.stop(State::Cancelled);
    request.cancel.cancel();
    let paused = stream.pause().map_err(|e| e.to_string());
    drop(stream);
    producer.renderer.clear();
    result.and(paused)
}

struct Callback {
    reader: Reader,
    control: Control,
    clock: Arc<SampleClock>,
    cancel: Cancellation,
    origin: Instant,
    cursor: u64,
    end: u64,
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut callback: Callback,
) -> Result<cpal::Stream> {
    let channels = usize::from(config.channels);
    let error_control = callback.control.clone();
    let error_cancel = callback.cancel.clone();
    device
        .build_output_stream(
            config,
            move |output: &mut [T], info| {
                if callback.cancel.is_cancelled() {
                    callback.control.stop(State::Cancelled);
                }
                let delivered = callback.reader.consume(output, T::from_sample);
                let state = callback.control.state();
                let limit = if matches!(state, State::Running | State::Ended) {
                    callback.end
                } else {
                    delivered.first + delivered.frames
                };
                let submitted = callback
                    .cursor
                    .saturating_add((output.len() / channels) as u64);
                let timestamp = info.timestamp();
                let latency = timestamp
                    .playback
                    .duration_since(&timestamp.callback)
                    .map_or(0, |v| v.as_nanos().min(u128::from(u64::MAX)) as u64);
                if !callback.clock.record(
                    callback.cursor,
                    submitted,
                    elapsed_ns(callback.origin),
                    latency,
                    limit,
                ) {
                    callback.control.stop(State::InvalidOutput);
                }
                callback.cursor = submitted;
                if fault(callback.control.state()).is_some() {
                    callback.cancel.cancel();
                }
            },
            move |_| {
                error_control.stop(State::DeviceFailed);
                error_cancel.cancel();
            },
            None,
        )
        .map_err(|e| e.to_string())
}

fn elapsed_ns(origin: Instant) -> u64 {
    origin.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
}
fn fault(state: State) -> Option<&'static str> {
    match state {
        State::SourceFailed => Some("Sound preparation stopped before the sequence ended"),
        State::DeviceFailed => Some("Sound output device failed; retry playback"),
        State::Underrun => Some(
            "Sound preparation fell behind the device; playback stopped without skipping source samples",
        ),
        State::InvalidOutput => Some("Sound output device returned an invalid buffer or clock"),
        _ => None,
    }
}
