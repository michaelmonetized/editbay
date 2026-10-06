use crate::{
    DeviceProfile, MonitorRoute, PlaybackPhase, PlaybackStart, StreamingStatus,
    streaming::{CAPACITY, LocalPlayback},
};
use editbay_core::{DocumentVersion, FrameRate, Project};
use editbay_media::{
    Cancellation,
    native_job::{JobChannel, NativeJob},
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, String>;
const RESPONSE: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_millis(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    session: Uuid,
    version: DocumentVersion,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Begin {
        project: Box<Project>,
        composition: Uuid,
        start: PlaybackStart,
        route: MonitorRoute,
    },
    Poll {},
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    owner: Owner,
    serial: u64,
    operation: Operation,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    owner: Owner,
    serial: u64,
    status: StreamingStatus,
}

type Latest = Arc<Mutex<Arc<StreamingStatus>>>;

fn publish(latest: &Latest, status: StreamingStatus) {
    let status = Arc::new(status);
    *latest.lock().unwrap_or_else(|error| error.into_inner()) = status;
}

fn observe(latest: &Latest) -> StreamingStatus {
    let status = latest
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    (*status).clone()
}

fn preparing(version: DocumentVersion, route: MonitorRoute) -> StreamingStatus {
    StreamingStatus {
        version,
        phase: PlaybackPhase::Preparing,
        position_samples: None,
        start_sample: None,
        end_sample: None,
        device: None,
        route,
        worker_pid: None,
        device_worker_pid: None,
        callbacks: 0,
        reported_latency_ns: 0,
        prepared_frames: 0,
        prepared_capacity_frames: CAPACITY,
        clipped_monitor_samples: 0,
        error: None,
    }
}

fn terminal(phase: PlaybackPhase) -> bool {
    matches!(
        phase,
        PlaybackPhase::Failed | PlaybackPhase::Finished | PlaybackPhase::Stopped
    )
}

/// Playback supervised outside the native device process.
pub struct StreamingPlayback {
    latest: Latest,
    cancel: Cancellation,
    thread: Option<JoinHandle<()>>,
}

impl StreamingPlayback {
    /// Start one private device lifetime without device work on the caller.
    /// `project`, `composition`, `start` and `route` capture exact saved content
    /// and monitoring choices. Returns an owner with a bounded latest status.
    pub fn start(
        project: Arc<Project>,
        composition: Uuid,
        start: PlaybackStart,
        route: MonitorRoute,
    ) -> Result<Self> {
        let owner = Owner {
            session: Uuid::new_v4(),
            version: DocumentVersion::of(&project),
        };
        let latest = Arc::new(Mutex::new(Arc::new(preparing(owner.version, route))));
        let cancel = Cancellation::new().map_err(|e| e.to_string())?;
        let output = latest.clone();
        let cancelled = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("editbay-device-supervisor".into())
            .spawn(move || {
                let result = supervise(
                    project,
                    composition,
                    start,
                    owner,
                    route,
                    &output,
                    &cancelled,
                );
                let mut status = observe(&output);
                if cancelled.is_cancelled() && !terminal(status.phase) {
                    status.phase = PlaybackPhase::Stopped;
                    status.error = None;
                } else if let Err(error) = result {
                    status.phase = PlaybackPhase::Failed;
                    status.error = Some(error.chars().take(8192).collect());
                }
                publish(&output, status);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            latest,
            cancel,
            thread: Some(thread),
        })
    }

    /// Observe the latest validated device clock without IPC or device access.
    /// Takes no arguments; returns one bounded copy, sampled by the supervisor
    /// every eight milliseconds. It does not extrapolate unobserved device time.
    pub fn status(&mut self) -> StreamingStatus {
        self.reap();
        let mut status = observe(&self.latest);
        if self.cancel.is_cancelled() && !terminal(status.phase) {
            status.phase = PlaybackPhase::Stopping;
        }
        status
    }

    /// Cancel this lifetime and wake its supervisor without joining on the caller.
    /// Takes no arguments; transport closure stops the child, with forced process
    /// retirement if its backend fails to leave within the supervision deadline.
    pub fn stop(&self) {
        self.cancel.cancel();
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }

    /// Inspect retirement without waiting for native backend teardown.
    /// Takes no arguments; returns true after the device child has been reaped.
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Join a finished supervisor without blocking active input.
    /// Takes no arguments; returns false until process retirement is complete.
    pub fn reap(&mut self) -> bool {
        if !self.is_finished() {
            return false;
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            let mut status = observe(&self.latest);
            status.phase = PlaybackPhase::Failed;
            status.error = Some("Sound device supervisor panicked".into());
            publish(&self.latest, status);
        }
        true
    }
}

impl Drop for StreamingPlayback {
    fn drop(&mut self) {
        self.stop();
    }
}

fn supervise(
    project: Arc<Project>,
    composition: Uuid,
    start: PlaybackStart,
    owner: Owner,
    route: MonitorRoute,
    latest: &Latest,
    cancel: &Cancellation,
) -> Result<()> {
    project.validate().map_err(|e| e.to_string())?;
    let scene = project
        .compositions
        .iter()
        .find(|scene| scene.id == composition)
        .ok_or("Playback composition is absent")?;
    let bounds = Bounds {
        duration: scene.duration,
        rate: scene.frame_rate,
        start,
    };
    let mut job = NativeJob::<Response>::start(
        &std::env::current_exe().map_err(|e| e.to_string())?,
        "--sound-device-worker",
    )
    .map_err(|e| e.to_string())?;
    let pid = job.process_id();
    let mut previous = observe(latest);
    previous.device_worker_pid = Some(pid);
    publish(latest, previous.clone());
    let mut request = Request {
        owner,
        serial: 0,
        operation: Operation::Begin {
            project: Box::new(Arc::unwrap_or_clone(project)),
            composition,
            start,
            route,
        },
    };
    let began = Instant::now();
    let mut progress = began;
    while !cancel.is_cancelled() {
        let reply = job
            .request_with_timeout(&request, None, cancel, RESPONSE)
            .map_err(|e| e.to_string())?;
        validate(&reply, owner, request.serial, &previous, &bounds)?;
        let mut status = reply.status;
        status.device_worker_pid = Some(pid);
        if status.callbacks > previous.callbacks || previous.phase == PlaybackPhase::Preparing {
            progress = Instant::now();
        }
        if status.phase == PlaybackPhase::Preparing && began.elapsed() > Duration::from_secs(30) {
            return Err("Sound device preparation exceeded 30 seconds".into());
        }
        if status.phase == PlaybackPhase::Playing && progress.elapsed() > Duration::from_secs(1) {
            return Err("Sound device callbacks stopped for more than one second".into());
        }
        publish(latest, status.clone());
        if terminal(status.phase) {
            break;
        }
        previous = status;
        request = Request {
            owner,
            serial: request
                .serial
                .checked_add(1)
                .ok_or("Sound device serial exhausted")?,
            operation: Operation::Poll {},
        };
        std::thread::park_timeout(POLL);
    }
    drop(job);
    Ok(())
}

struct Bounds {
    duration: u64,
    rate: FrameRate,
    start: PlaybackStart,
}
impl Bounds {
    fn interval(&self, sample_rate: u32) -> Result<(u64, u64)> {
        let frame = |position: u64| {
            (u128::from(position) * u128::from(sample_rate) * u128::from(self.rate.denominator))
                .div_ceil(u128::from(self.rate.numerator))
        };
        let start = match self.start {
            PlaybackStart::Frame(position) => frame(position),
            PlaybackStart::Sample {
                position,
                sample_rate: rate,
            } => {
                if !(8000..=384000).contains(&rate) {
                    return Err("Invalid paused sample rate".into());
                }
                (u128::from(position) * u128::from(sample_rate)).div_ceil(u128::from(rate))
            }
        };
        let start = u64::try_from(start).map_err(|e| e.to_string())?;
        let end = u64::try_from(frame(self.duration)).map_err(|e| e.to_string())?;
        if start >= end {
            return Err("Playback start is outside the sequence sound".into());
        }
        Ok((start, end))
    }
}

fn valid_device(device: &DeviceProfile) -> bool {
    !device.name.is_empty()
        && device.name.len() <= 1024
        && (8000..=384000).contains(&device.sample_rate)
        && (1..=2).contains(&device.channels)
        && ["F32", "F64", "I16", "I32", "U16"].contains(&device.sample_format.as_str())
}

fn validate(
    reply: &Response,
    owner: Owner,
    serial: u64,
    previous: &StreamingStatus,
    bounds: &Bounds,
) -> Result<()> {
    let status = &reply.status;
    if reply.owner != owner
        || reply.serial != serial
        || status.version != owner.version
        || status.route != previous.route
        || status.device_worker_pid.is_some()
    {
        return Err("Foreign or stale sound device response".into());
    }
    if status.prepared_capacity_frames != CAPACITY
        || status.prepared_frames > u64::from(CAPACITY)
        || status.callbacks < previous.callbacks
        || status.clipped_monitor_samples < previous.clipped_monitor_samples
        || status.reported_latency_ns > 5_000_000_000
        || status
            .error
            .as_ref()
            .is_some_and(|error| error.is_empty() || error.len() > 32768)
        || (status.phase == PlaybackPhase::Failed) != status.error.is_some()
        || matches!(
            status.phase,
            PlaybackPhase::Stopping | PlaybackPhase::Stopped
        )
        || (previous.phase == PlaybackPhase::Playing && status.phase == PlaybackPhase::Preparing)
    {
        return Err("Malformed sound device status".into());
    }
    if let Some(device) = &status.device {
        if status.phase == PlaybackPhase::Preparing
            || !valid_device(device)
            || status.worker_pid.is_none_or(|pid| pid == 0)
            || previous
                .worker_pid
                .is_some_and(|pid| Some(pid) != status.worker_pid)
            || previous
                .device
                .as_ref()
                .is_some_and(|prior| prior != device)
        {
            return Err("Sound device identity changed or is invalid".into());
        }
        let (first, end) = bounds.interval(device.sample_rate)?;
        if status.start_sample != Some(first)
            || status.end_sample != Some(end)
            || status.position_samples.is_none_or(|position| {
                position < first
                    || position > end
                    || previous
                        .position_samples
                        .is_some_and(|prior| position < prior)
            })
            || (status.phase == PlaybackPhase::Finished && status.position_samples != Some(end))
        {
            return Err("Sound device clock escaped its captured interval".into());
        }
    } else if status.start_sample.is_some()
        || status.end_sample.is_some()
        || status.position_samples.is_some()
        || status.worker_pid.is_some()
        || status.callbacks != 0
        || status.prepared_frames != 0
        || previous.device.is_some()
        || matches!(
            status.phase,
            PlaybackPhase::Playing | PlaybackPhase::Finished
        )
    {
        return Err("Sound device clock is missing its device".into());
    }
    Ok(())
}

/// Serve the packaged native device endpoint on its single inherited socket.
/// Takes no arguments; accepts one owned playback and monotonically numbered
/// observations. Returns on parent closure or rejects malformed/foreign traffic.
pub fn serve_device_worker() -> Result<()> {
    let mut channel = JobChannel::inherited().map_err(|e| e.to_string())?;
    let mut playback: Option<LocalPlayback> = None;
    let mut owner = None;
    let mut serial = 0;
    while let Some((request, file)) = channel.receive::<Request>().map_err(|e| e.to_string())? {
        if file.is_some()
            || request.serial != serial
            || owner.is_some_and(|bound| bound != request.owner)
        {
            return Err("Foreign, stale or descriptor-bearing sound device request".into());
        }
        match request.operation {
            Operation::Begin {
                project,
                composition,
                start,
                route,
            } if playback.is_none() => {
                if serial != 0
                    || request.owner.session.is_nil()
                    || request.owner.version != DocumentVersion::of(&project)
                {
                    return Err("Invalid sound device ownership".into());
                }
                project.validate().map_err(|e| e.to_string())?;
                owner = Some(request.owner);
                playback = Some(LocalPlayback::start(
                    Arc::from(project),
                    composition,
                    start,
                    route,
                )?);
            }
            Operation::Poll {} if playback.is_some() => {}
            _ => return Err("Invalid sound device operation order".into()),
        }
        let status = playback
            .as_mut()
            .ok_or("Sound device lacks playback")?
            .status();
        channel
            .send(&Response {
                owner: request.owner,
                serial,
                status,
            })
            .map_err(|e| e.to_string())?;
        serial = serial
            .checked_add(1)
            .ok_or("Sound device serial exhausted")?;
    }
    if let Some(mut playback) = playback {
        playback.stop();
        let began = Instant::now();
        while !playback.reap() && began.elapsed() < Duration::from_millis(75) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn playing() -> (Owner, StreamingStatus, Bounds) {
        let project = Project::new("Device protocol").unwrap();
        let owner = Owner {
            session: Uuid::new_v4(),
            version: DocumentVersion::of(&project),
        };
        let mut status = preparing(owner.version, MonitorRoute::Stereo);
        status.phase = PlaybackPhase::Playing;
        status.device = Some(DeviceProfile {
            name: "Declared device".into(),
            sample_rate: 48000,
            channels: 2,
            sample_format: "F32".into(),
        });
        status.worker_pid = Some(123);
        status.start_sample = Some(2002);
        status.position_samples = Some(3000);
        status.end_sample = Some(48048);
        status.callbacks = 3;
        status.prepared_frames = 8192;
        let bounds = Bounds {
            duration: 24,
            rate: FrameRate::new(24000, 1001).unwrap(),
            start: PlaybackStart::Frame(1),
        };
        (owner, status, bounds)
    }

    #[test]
    fn exact_boundaries_survive_device_rate_changes_and_large_positions() {
        let (_, _, mut bounds) = playing();
        assert_eq!(bounds.interval(44100).unwrap(), (1840, 44145));
        bounds.start = PlaybackStart::Sample {
            position: 2002,
            sample_rate: 48000,
        };
        assert_eq!(bounds.interval(44100).unwrap(), (1840, 44145));
        bounds.start = PlaybackStart::Sample {
            position: u64::MAX,
            sample_rate: 0,
        };
        assert!(bounds.interval(48000).is_err());
        bounds.start = PlaybackStart::Frame(u64::MAX);
        assert!(bounds.interval(384000).is_err());
    }

    #[test]
    fn rejects_foreign_stale_unbounded_and_regressing_status() {
        let (owner, previous, bounds) = playing();
        let baseline = Response {
            owner,
            serial: 7,
            status: previous.clone(),
        };
        assert!(validate(&baseline, owner, 7, &previous, &bounds).is_ok());
        let original = serde_json::to_value(&baseline).unwrap();
        for (pointer, value) in [
            ("/owner/session", json!(Uuid::new_v4())),
            ("/owner/version/revision", json!(owner.version.revision + 1)),
            ("/serial", json!(6)),
            (
                "/status/version/revision",
                json!(owner.version.revision + 1),
            ),
            ("/status/route", json!("Original")),
            ("/status/device_worker_pid", json!(123)),
            ("/status/callbacks", json!(2)),
            ("/status/prepared_frames", json!(16385)),
            ("/status/prepared_capacity_frames", json!(16385)),
            ("/status/reported_latency_ns", json!(5_000_000_001u64)),
            ("/status/position_samples", json!(2999)),
            ("/status/start_sample", json!(2001)),
            ("/status/end_sample", json!(48049)),
            ("/status/device/channels", json!(6)),
            ("/status/device/sample_rate", json!(0)),
            ("/status/device/sample_format", json!("invalid")),
            ("/status/device/name", json!("Changed device")),
            ("/status/worker_pid", json!(124)),
            ("/status/error", json!("unexpected error")),
            ("/status/phase", json!("preparing")),
            ("/status/phase", json!("stopped")),
            ("/status/phase", json!("finished")),
            ("/status/device", json!(null)),
        ] {
            let mut altered = original.clone();
            *altered.pointer_mut(pointer).unwrap() = value;
            let reply = serde_json::from_value(altered).unwrap();
            assert!(
                validate(&reply, owner, 7, &previous, &bounds).is_err(),
                "accepted {pointer}"
            );
        }
    }

    #[test]
    fn permits_exact_completion_and_bounded_device_failures() {
        let (owner, previous, bounds) = playing();
        let mut reply = Response {
            owner,
            serial: 8,
            status: previous.clone(),
        };
        reply.status.position_samples = reply.status.end_sample;
        reply.status.phase = PlaybackPhase::Finished;
        assert!(validate(&reply, owner, 8, &previous, &bounds).is_ok());
        reply.status.phase = PlaybackPhase::Failed;
        reply.status.error = Some("Device disconnected".into());
        assert!(validate(&reply, owner, 8, &previous, &bounds).is_ok());
        let before = preparing(owner.version, MonitorRoute::Stereo);
        reply.status = before.clone();
        reply.status.phase = PlaybackPhase::Failed;
        reply.status.error = Some("No device available".into());
        assert!(validate(&reply, owner, 8, &before, &bounds).is_ok());
        reply.status.error = Some("x".repeat(32769));
        assert!(validate(&reply, owner, 8, &before, &bounds).is_err());
    }

    #[test]
    fn protocol_rejects_unknown_fields_and_untyped_operations() {
        let (owner, status, _) = playing();
        let mut reply = serde_json::to_value(Response {
            owner,
            serial: 0,
            status,
        })
        .unwrap();
        reply["status"]["extra"] = json!(true);
        assert!(serde_json::from_value::<Response>(reply).is_err());
        for operation in [
            json!({"kind":"poll","extra":true}),
            json!({"kind":"stop"}),
            json!(null),
        ] {
            assert!(
                serde_json::from_value::<Request>(
                    json!({"owner":owner,"serial":0,"operation":operation})
                )
                .is_err()
            );
        }
    }

    #[test]
    fn latest_status_retains_terminal_failure_without_a_reader() {
        let (owner, mut status, _) = playing();
        let latest = Arc::new(Mutex::new(Arc::new(preparing(owner.version, status.route))));
        for callback in 3..1000 {
            status.callbacks = callback;
            publish(&latest, status.clone());
        }
        status.phase = PlaybackPhase::Failed;
        status.error = Some("Worker died".into());
        publish(&latest, status);
        let observed = observe(&latest);
        assert_eq!(observed.callbacks, 999);
        assert_eq!(observed.phase, PlaybackPhase::Failed);
        assert_eq!(observed.error.as_deref(), Some("Worker died"));
        assert_eq!(Arc::strong_count(&latest.lock().unwrap()), 1);
    }

    #[test]
    fn supervisor_panic_becomes_visible_without_waiting_for_ui_retirement() {
        let (owner, _, _) = playing();
        let mut playback = StreamingPlayback {
            latest: Arc::new(Mutex::new(Arc::new(preparing(
                owner.version,
                MonitorRoute::Stereo,
            )))),
            cancel: Cancellation::new().unwrap(),
            thread: Some(std::thread::spawn(|| panic!("failed supervisor"))),
        };
        while !playback.is_finished() {
            std::thread::yield_now();
        }
        assert_eq!(playback.status().phase, PlaybackPhase::Failed);
        assert!(
            playback
                .status()
                .error
                .unwrap()
                .contains("supervisor panicked")
        );
        assert!(playback.reap());
    }
}
