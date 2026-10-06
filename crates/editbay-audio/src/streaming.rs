use crate::{
    ClockObservation, ClockRejection, DeviceProfile, MonitorRoute, SoundRenderBudget,
    SoundRenderer,
    monitor::MonitorMatrix,
    preparation::{PreparationLog, PreparationStats},
    sample_clock::{ClockContinuity, SampleClock},
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
    pub clock_observation: Option<ClockObservation>,
    pub clock_rejection: Option<ClockRejection>,
    pub prepared_frames: u64,
    pub prepared_capacity_frames: u32,
    pub clipped_monitor_samples: u64,
    pub preparation: PreparationStats,
    pub error: Option<String>,
}

struct Ready {
    preparation: Arc<PreparationLog>,
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
    preparation: PreparationStats,
    version: DocumentVersion,
    route: MonitorRoute,
    cancel: Cancellation,
    stop: Arc<AtomicBool>,
    events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
    ready: Option<Ready>,
    phase: PlaybackPhase,
    error: Option<String>,
    clock_observation: Option<ClockObservation>,
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
            preparation: PreparationStats::default(),
            version,
            route,
            cancel,
            stop,
            events,
            thread: Some(thread),
            ready: None,
            phase: PlaybackPhase::Preparing,
            error: None,
            clock_observation: None,
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
                if let Some(observed) = ready.preparation.observation() {
                    self.preparation = observed;
                }
                if let Some(observation) = ready.clock.observation() {
                    self.clock_observation = Some(observation);
                }
                let (callbacks, latency) = self.clock_observation.map_or((0, 0), |observed| {
                    (observed.callbacks, observed.reported_latency_ns)
                });
                if self.phase == PlaybackPhase::Playing
                    && let Some(error) = fault(ready.control.state())
                {
                    self.phase = PlaybackPhase::Failed;
                    self.error = Some(error.into());
                    self.cancel.cancel();
                }
                if self.phase == PlaybackPhase::Failed {
                    self.clock_observation = ready.clock.observation();
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
            clock_observation: self.clock_observation,
            clock_rejection: self.ready.as_ref().and_then(|ready| {
                (self.phase == PlaybackPhase::Failed)
                    .then(|| ready.clock.rejection())
                    .flatten()
            }),
            prepared_frames: prepared,
            prepared_capacity_frames: CAPACITY,
            clipped_monitor_samples: clipped,
            preparation: self.preparation,
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
    preparation: Arc<PreparationLog>,
    measured: PreparationStats,
    previous_block: Option<Instant>,
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
            let began = Instant::now();
            if let Some(previous) = self.previous_block.replace(began) {
                self.measured.max_interval_ns = self
                    .measured
                    .max_interval_ns
                    .max(began.duration_since(previous).as_nanos() as u64);
            }
            self.measured.last_plan_ns = 0;
            self.measured.last_render_ns = 0;
            self.measured.last_finish_ns = 0;
            let result = self.prepare(frames);
            self.measured.blocks = self.measured.blocks.saturating_add(1);
            self.measured.last_block_ns = began.elapsed().as_nanos() as u64;
            if self.measured.last_block_ns >= self.measured.max_block_ns {
                self.measured.max_block_ns = self.measured.last_block_ns;
                self.measured.slowest_plan_ns = self.measured.last_plan_ns;
                self.measured.slowest_render_ns = self.measured.last_render_ns;
                self.measured.slowest_finish_ns = self.measured.last_finish_ns;
            }
            if result.is_ok() {
                self.measured.published_blocks = self.measured.published_blocks.saturating_add(1);
            }
            self.preparation.publish(self.measured);
            result?;
        }
        Ok(())
    }

    fn prepare(&mut self, frames: u32) -> Result<()> {
        let began = Instant::now();
        let plan = self
            .sound
            .prepare(self.cursor, frames)
            .map_err(|e| e.to_string());
        self.measured.last_plan_ns = began.elapsed().as_nanos() as u64;
        let plan = plan?;
        let began = Instant::now();
        let result = self.renderer.render(&plan).map_err(|e| e.to_string());
        self.measured.last_render_ns = began.elapsed().as_nanos() as u64;
        let result = result?;
        let began = Instant::now();
        let finished = (|| {
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
            Ok(())
        })();
        self.measured.last_finish_ns = began.elapsed().as_nanos() as u64;
        finished
    }
}

fn run(request: &Request, sender: &SyncSender<Event>) -> Result<()> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("No sound output device is available")?;
    let default = device.default_output_config().map_err(|e| e.to_string())?;
    let supported = match device.supported_output_configs() {
        Ok(configurations) => video_output_config(default, configurations),
        Err(_) => default,
    };
    let mut profile = DeviceProfile {
        name: device
            .description()
            .map_err(|e| e.to_string())?
            .name()
            .into(),
        sample_rate: supported.sample_rate(),
        channels: supported.channels(),
        sample_format: format!("{:?}", supported.sample_format()),
        requested_callback_frames: None,
        reported_callback_frames: None,
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
    let preparation = Arc::new(PreparationLog::default());
    let mut producer = Producer {
        preparation: preparation.clone(),
        measured: PreparationStats::default(),
        previous_block: None,
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
        backend_origin: None,
        backend_previous: None,
        backend_epoch: 0,
        continuity: ClockContinuity::default(),
        end,
    };
    let mut config: cpal::StreamConfig = supported.into();
    if let cpal::SupportedBufferSize::Range { min, max } = supported.buffer_size() {
        if min > max {
            return Err("Sound device reports an invalid callback range".into());
        }
        let frames = 2048u32.clamp(*min, *max);
        if frames == 0 || frames > CAPACITY {
            return Err("Sound device callback exceeds the prepared-frame budget".into());
        }
        config.buffer_size = cpal::BufferSize::Fixed(frames);
        profile.requested_callback_frames = Some(frames);
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, callback),
        cpal::SampleFormat::F64 => build::<f64>(&device, &config, callback),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, callback),
        cpal::SampleFormat::I32 => build::<i32>(&device, &config, callback),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, callback),
        other => Err(format!("Unsupported device sample format {other:?}")),
    }?;
    profile.reported_callback_frames = match stream.buffer_size() {
        Ok(frames) if (1..=CAPACITY).contains(&frames) => Some(frames),
        Ok(_) => return Err("Sound device reports an over-budget callback buffer".into()),
        Err(error) if error.kind() == cpal::ErrorKind::UnsupportedOperation => None,
        Err(error) => return Err(error.to_string()),
    };
    sender
        .try_send(Event::Ready(Ready {
            preparation,
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
    match fault(control.state()) {
        Some(error) => Err(error.into()),
        None => result.and(paused),
    }
}

struct Callback {
    reader: Reader,
    control: Control,
    clock: Arc<SampleClock>,
    cancel: Cancellation,
    origin: Instant,
    cursor: u64,
    backend_origin: Option<cpal::StreamInstant>,
    backend_previous: Option<cpal::StreamInstant>,
    backend_epoch: u64,
    continuity: ClockContinuity,
    end: u64,
}

impl Callback {
    fn fill<T: SizedSample + FromSample<f32>>(
        &mut self,
        output: &mut [T],
        channels: usize,
        rate: u32,
        timestamp: cpal::OutputStreamTimestamp,
    ) {
        if channels == 0
            || output.is_empty()
            || !output.len().is_multiple_of(channels)
            || output.len() / channels > CAPACITY as usize
        {
            self.control.stop(State::InvalidOutput);
            self.cancel.cancel();
            output.fill(T::from_sample(0.));
            return;
        }
        if self.cancel.is_cancelled() {
            self.control.stop(State::Cancelled);
            output.fill(T::from_sample(0.));
            return;
        }
        let callback_ns = elapsed_ns(self.origin);
        if self
            .backend_previous
            .is_some_and(|previous| timestamp.callback < previous)
        {
            if self.backend_epoch != 0 || callback_ns >= 1_000_000_000 {
                self.control.stop(State::BackendClockReset);
                self.cancel.cancel();
                output.fill(T::from_sample(0.));
                return;
            }
            self.backend_epoch = 1;
            self.backend_origin = Some(timestamp.callback);
        }
        self.backend_previous = Some(timestamp.callback);
        let backend_origin = self.backend_origin.get_or_insert(timestamp.callback);
        let Some(backend_elapsed) = timestamp.callback.checked_duration_since(*backend_origin)
        else {
            self.control.stop(State::InvalidOutput);
            self.cancel.cancel();
            output.fill(T::from_sample(0.));
            return;
        };
        let Some(latency) = timestamp
            .playback
            .checked_duration_since(timestamp.callback)
        else {
            self.control.stop(State::InvalidOutput);
            self.cancel.cancel();
            output.fill(T::from_sample(0.));
            return;
        };
        let latency = latency.as_nanos().min(u128::from(u64::MAX)) as u64;
        let backend_ns = backend_elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
        if !self
            .continuity
            .observe(self.cursor, backend_ns, latency, rate)
        {
            self.clock.reject(ClockRejection {
                buffer_start_frames: self.cursor,
                callback_elapsed_ns: callback_ns,
                backend_elapsed_ns: backend_ns,
                reported_latency_ns: latency,
                signed_drift_ns: self.continuity.error_ns,
            });
            self.control.stop(State::BackendDiscontinuity);
            self.cancel.cancel();
            output.fill(T::from_sample(0.));
            return;
        }
        let delivered = self.reader.consume(output, T::from_sample);
        let state = self.control.state();
        let limit = if matches!(state, State::Running | State::Ended) {
            self.end
        } else {
            delivered.first + delivered.frames
        };
        let submitted = self.cursor.saturating_add((output.len() / channels) as u64);
        if !self.clock.record(
            self.cursor,
            submitted,
            callback_ns,
            (self.backend_epoch, backend_ns),
            latency,
            limit,
        ) {
            self.control.stop(State::InvalidOutput);
        }
        self.cursor = submitted;
        if fault(self.control.state()).is_some() {
            self.cancel.cancel();
        }
    }
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut callback: Callback,
) -> Result<cpal::Stream> {
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    let error_control = callback.control.clone();
    let error_cancel = callback.cancel.clone();
    device
        .build_output_stream(
            *config,
            move |output: &mut [T], info| {
                callback.fill(output, channels, rate, info.timestamp());
            },
            move |error| {
                error_control.stop(if error.kind() == cpal::ErrorKind::Xrun {
                    State::BackendUnderrun
                } else {
                    State::DeviceFailed
                });
                error_cancel.cancel();
            },
            None,
        )
        .map_err(|e| e.to_string())
}

fn video_output_config(
    default: cpal::SupportedStreamConfig,
    configurations: impl Iterator<Item = cpal::SupportedStreamConfigRange>,
) -> cpal::SupportedStreamConfig {
    configurations
        .take(256)
        .find(|config| {
            config.channels() == default.channels()
                && config.sample_format() == default.sample_format()
                && config.min_sample_rate() <= 48000
                && config.max_sample_rate() >= 48000
        })
        .map_or(default, |config| config.with_sample_rate(48000))
}

fn elapsed_ns(origin: Instant) -> u64 {
    origin.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
}

fn fault(state: State) -> Option<&'static str> {
    match state {
        State::SourceFailed => Some("Sound preparation stopped before the sequence ended"),
        State::DeviceFailed => Some("Sound output device failed; retry playback"),
        State::BackendUnderrun => Some(
            "Sound backend reported an underrun; playback stopped without skipping source samples",
        ),
        State::BackendClockReset => {
            Some("Sound backend timestamp reset after playback started; retry playback")
        }
        State::BackendDiscontinuity => Some(
            "Sound backend timing lost continuity; playback stopped without skipping source samples",
        ),
        State::Underrun => Some(
            "Sound preparation fell behind the device; playback stopped without skipping source samples",
        ),
        State::InvalidOutput => Some("Sound output device returned an invalid buffer or clock"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn callback_fixture() -> (Callback, Writer) {
        let (control, mut writer, reader) = transport::transport(0, 4, 2, CAPACITY).unwrap();
        writer
            .push(0, &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8])
            .unwrap();
        (
            Callback {
                reader,
                control,
                clock: Arc::new(SampleClock::new(48000, 0, 4).unwrap()),
                cancel: Cancellation::new().unwrap(),
                origin: Instant::now(),
                cursor: 0,
                backend_origin: None,
                backend_previous: None,
                backend_epoch: 0,
                continuity: ClockContinuity::default(),
                end: 4,
            },
            writer,
        )
    }

    fn timestamp(callback: u64, playback: u64) -> cpal::OutputStreamTimestamp {
        cpal::OutputStreamTimestamp {
            callback: cpal::StreamInstant::from_nanos(callback),
            playback: cpal::StreamInstant::from_nanos(playback),
        }
    }

    #[test]
    fn malformed_backend_buffers_and_negative_latency_preserve_queued_source() {
        for (channels, length, time) in [
            (0, 4, timestamp(0, 0)),
            (2, 0, timestamp(0, 0)),
            (2, 3, timestamp(0, 0)),
            (2, 2 * (CAPACITY as usize + 1), timestamp(0, 0)),
            (2, 4, timestamp(100, 99)),
        ] {
            let (mut callback, _writer) = callback_fixture();
            let mut output = vec![9f32; length];
            callback.fill(&mut output, channels, 48000, time);
            assert!(output.iter().all(|value| *value == 0.));
            assert_eq!(callback.control.state(), State::InvalidOutput);
            assert_eq!(callback.control.prepared(), 4);
            assert_eq!(callback.cursor, 0);
            assert!(callback.cancel.is_cancelled());
            assert!(callback.clock.observation().is_none());
        }
    }

    #[test]
    fn backend_callback_delivers_exact_source_then_rejects_a_reset() {
        let (mut callback, _writer) = callback_fixture();
        callback.origin = Instant::now() - Duration::from_secs(2);
        let mut output = [9f32; 4];
        callback.fill(&mut output, 2, 48000, timestamp(100, 200));
        assert_eq!(output, [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(callback.cursor, 2);
        let observed = callback.clock.observation().unwrap();
        assert_eq!(observed.submitted_frames, 2);
        assert_eq!(observed.reported_latency_ns, 100);
        callback.fill(&mut output, 2, 48000, timestamp(99, 200));
        assert_eq!(output, [0.; 4]);
        assert_eq!(callback.control.state(), State::BackendClockReset);
        assert_eq!(callback.control.prepared(), 2);
        assert_eq!(callback.clock.observation().unwrap(), observed);
        assert!(callback.cancel.is_cancelled());
    }

    #[test]
    fn rejected_clock_retains_the_failed_callback_without_advancing_sound() {
        let (mut callback, _writer) = callback_fixture();
        let mut output = [0f32; 4];
        callback.fill(&mut output, 2, 48000, timestamp(0, 30_000_000));
        callback.fill(
            &mut output,
            2,
            48000,
            timestamp(1_000_000_000, 1_030_000_000),
        );
        assert_eq!(callback.control.state(), State::Ended);
        let accepted = callback.clock.observation().unwrap();
        callback.fill(
            &mut output,
            2,
            48000,
            timestamp(1_100_000_000, 1_130_000_000),
        );
        assert_eq!(output, [0.; 4]);
        assert_eq!(callback.control.state(), State::BackendDiscontinuity);
        assert_eq!(callback.clock.observation(), Some(accepted));
        let rejected = callback.clock.rejection().unwrap();
        assert_eq!(rejected.buffer_start_frames, accepted.submitted_frames);
        assert_eq!(rejected.backend_elapsed_ns, 1_100_000_000);
        assert_eq!(rejected.reported_latency_ns, 30_000_000);
        assert_eq!(rejected.signed_drift_ns, -99_958_334);
        callback.fill(
            &mut output,
            2,
            48000,
            timestamp(1_200_000_000, 1_230_000_000),
        );
        assert_eq!(callback.clock.rejection(), Some(rejected));
        assert_eq!(callback.clock.observation(), Some(accepted));
    }

    #[test]
    fn video_output_preserves_device_channels_and_format_with_honest_rate_fallback() {
        use cpal::{
            SampleFormat, SupportedBufferSize, SupportedStreamConfig, SupportedStreamConfigRange,
        };
        let default =
            SupportedStreamConfig::new(2, 44100, SupportedBufferSize::Unknown, SampleFormat::F32);
        let range = |channels, maximum, format| {
            SupportedStreamConfigRange::new(
                channels,
                44100,
                maximum,
                SupportedBufferSize::Unknown,
                format,
            )
        };
        let chosen = video_output_config(
            default,
            [
                range(6, 48000, SampleFormat::F32),
                range(2, 48000, SampleFormat::I16),
                range(2, 48000, SampleFormat::F32),
            ]
            .into_iter(),
        );
        assert_eq!(chosen.sample_rate(), 48000);
        assert_eq!(chosen.channels(), 2);
        assert_eq!(chosen.sample_format(), SampleFormat::F32);
        let fallback =
            video_output_config(default, [range(2, 44100, SampleFormat::F32)].into_iter());
        assert_eq!(fallback.sample_rate(), 44100);
        assert_eq!(
            video_output_config(default, std::iter::empty()).sample_rate(),
            44100
        );
    }
}
