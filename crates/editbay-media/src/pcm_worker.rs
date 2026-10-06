use crate::{
    Cancellation, Error, NativePcmCache, PcmBlock, PcmBudget, PcmPreparation, PcmPreparationLog,
    PcmProgress, PcmProvider, PcmResult, PcmStats, Result,
    codec_process::{Ownership, Process},
    pcm::{Charge, Samples},
    pictures::HandleAllocation,
    planes,
};
use editbay_core::{DocumentVersion, EvaluationSnapshot, Project, StreamFormat};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};
use uuid::Uuid;

/// Limits for child PCM and parent mappings, including consumer-held handles.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PcmWorkerBudget {
    pub pcm: PcmBudget,
    pub live_handles: usize,
}
impl Default for PcmWorkerBudget {
    fn default() -> Self {
        Self {
            pcm: PcmBudget::default(),
            live_handles: 128,
        }
    }
}
impl PcmWorkerBudget {
    /// Check bounded output memory and process descriptor use before starting work.
    /// Takes these limits; returns an error for excessive cache or handle counts.
    pub fn validate(self) -> Result<()> {
        self.pcm.validate()?;
        if self.pcm.cache_entries > 128 || !(1..=256).contains(&self.live_handles) {
            return Err(Error::Invalid(
                "PCM transport exceeds process handle limits".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Interval {
    source: Uuid,
    stream: u32,
    first: i64,
    frames: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    owner: Ownership,
    serial: u64,
    operation: Operation,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Bind {
        project: Box<Project>,
        budget: PcmWorkerBudget,
    },
    Interval {
        interval: Interval,
    },
    Prepare {
        interval: Interval,
    },
    Check,
    Verify,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    owner: Ownership,
    serial: u64,
    stats: PcmStats,
    result: Reply,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Ready,
    Preparing {
        progress: PcmPreparation,
    },
    Prepared {
        progress: PcmPreparation,
    },
    Pcm {
        interval: Interval,
        rate: u32,
        channels: Vec<String>,
    },
    Checked,
    Verified,
    Failed {
        message: String,
        cancelled: bool,
        source_changed: bool,
    },
}
fn validate_response(response: &Response, owner: Ownership, serial: u64) -> Result<()> {
    if response.owner != owner || response.serial != serial {
        return Err(Error::Invalid("foreign or obsolete PCM response".into()));
    }
    Ok(())
}
fn valid_stats(stats: PcmStats, previous: PcmStats, budget: PcmBudget) -> bool {
    stats.cache_bytes <= budget.cache_bytes
        && stats.live_bytes <= budget.live_bytes
        && stats.cache_bytes <= stats.live_bytes
        && stats.entries <= budget.cache_entries
        && stats.sources <= budget.source_handles
        && stats.decoders <= budget.decoder_handles
        && stats.stores <= budget.store_handles
        && stats.store_bytes <= budget.store_bytes
        && stats.decoder_scratch_bytes <= stats.decoder_scratch_limit_bytes
        && stats.decoder_scratch_limit_bytes <= budget.decoder_handles * 65536 * 64 * 4
        && stats.hits >= previous.hits
        && stats.misses >= previous.misses
        && stats.evictions >= previous.evictions
        && stats.decoded_frames >= previous.decoded_frames
        && stats.preparation_steps >= previous.preparation_steps
}
struct Selection {
    asset: Uuid,
    bytes: usize,
    rate: u32,
    channels: Vec<String>,
    required_frames: u64,
}

fn valid_preparation(
    progress: &PcmPreparation,
    interval: Interval,
    selected: &Selection,
    previous: u64,
) -> bool {
    progress.source == interval.source
        && progress.stream == interval.stream
        && progress.first == interval.first
        && progress.frames == interval.frames
        && progress.sample_rate == selected.rate
        && progress.required_frames == selected.required_frames
        && !progress.ready
        && progress.decoded_frames > previous
        && progress.decoded_frames < progress.required_frames
}
fn select(
    snapshot: &EvaluationSnapshot,
    interval: Interval,
    budget: PcmBudget,
) -> Result<Selection> {
    if interval.frames == 0
        || interval.frames > budget.interval_frames
        || interval
            .first
            .checked_add(i64::from(interval.frames))
            .is_none()
    {
        return Err(Error::Invalid(
            "PCM interval exceeds its frame or time limits".into(),
        ));
    }
    let (asset, profile, _) = snapshot.source_stream(interval.source, interval.stream)?;
    let StreamFormat::Audio {
        sample_rate,
        channels,
    } = &profile.format
    else {
        return Err(Error::Invalid(
            "PCM request selected a picture stream".into(),
        ));
    };
    let bytes = interval.frames as usize * channels.len() * size_of::<f32>();
    if bytes > budget.live_bytes {
        return Err(Error::Invalid(
            "PCM interval exceeds its byte budget".into(),
        ));
    }
    let (start, limit) = crate::pcm::presentation(profile, *sample_rate)?;
    let wanted_start = interval.first.max(start);
    let wanted_end = (interval.first + i64::from(interval.frames)).min(limit);
    let required_frames = if wanted_start < wanted_end {
        u64::try_from(i128::from(wanted_end) - i128::from(start))
            .map_err(|e| Error::Invalid(e.to_string()))?
    } else {
        0
    };
    Ok(Selection {
        asset: asset.id,
        bytes,
        rate: *sample_rate,
        channels: channels.clone(),
        required_frames,
    })
}
struct Entry {
    pcm: Arc<PcmBlock>,
    used: u64,
}

/// Observed mapped ownership and child decoding, counted separately by process.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct PcmTransferStats {
    pub mapped_bytes: usize,
    pub mapped_handles: usize,
    pub cache_bytes: usize,
    pub cache_entries: usize,
    pub received_planes: u64,
    pub mapping_hits: u64,
    pub spawns: u64,
    pub child: PcmStats,
}

/// Supervised original-channel native PCM over immutable bounded shared mappings.
pub struct PcmWorker {
    preparation: PcmPreparationLog,
    executable: PathBuf,
    snapshot: Arc<EvaluationSnapshot>,
    budget: PcmWorkerBudget,
    cancel: Cancellation,
    owner: Ownership,
    receipt_owner: Arc<()>,
    serial: u64,
    process: Option<Process<Response>>,
    entries: HashMap<Interval, Entry>,
    bytes: usize,
    live: Arc<AtomicUsize>,
    handles: Arc<AtomicUsize>,
    child_stats: PcmStats,
    received: u64,
    hits: u64,
    evictions: u64,
    spawns: u64,
    cleared: bool,
}
impl PcmWorker {
    /// Start one bounded packaged codec child for a captured sound document.
    /// `executable` supports --pcm-worker; `snapshot`, `budget` and `cancel`
    /// declare sources, resources and lifetime. Returns after a validated bind.
    pub fn new(
        executable: &Path,
        snapshot: Arc<EvaluationSnapshot>,
        budget: PcmWorkerBudget,
        cancel: Cancellation,
    ) -> Result<Self> {
        budget.validate()?;
        let mut worker = Self {
            preparation: PcmPreparationLog::default(),
            executable: executable.to_owned(),
            owner: Ownership {
                session: Uuid::new_v4(),
                job: Uuid::new_v4(),
                worker: Uuid::new_v4(),
                version: DocumentVersion::of(snapshot.project()),
                generation: 0,
            },
            snapshot,
            budget,
            cancel,
            receipt_owner: Arc::new(()),
            serial: 0,
            process: None,
            entries: HashMap::new(),
            bytes: 0,
            live: Arc::new(AtomicUsize::new(0)),
            handles: Arc::new(AtomicUsize::new(0)),
            child_stats: PcmStats::default(),
            received: 0,
            hits: 0,
            evictions: 0,
            spawns: 0,
            cleared: false,
        };
        worker.start()?;
        Ok(worker)
    }
    /// Inspect the actual supervised process identity.
    /// Takes no arguments; returns no PID after termination or explicit cleanup.
    pub fn process_id(&self) -> Option<u32> {
        self.process.as_ref().map(|p| p.child.id())
    }
    /// Check idle process health without requesting samples.
    /// Takes this provider; returns a visible failure and invalidates stale receipts.
    pub fn check_health(&mut self) -> Result<()> {
        if self.cancel.is_cancelled() {
            self.invalidate(false);
            return Err(Error::Cancelled);
        }
        if self
            .process
            .as_mut()
            .map(|p| p.child.try_wait())
            .transpose()?
            .flatten()
            .is_some()
        {
            self.invalidate(false);
            return Err(Error::Invalid("sound worker exited; retry playback".into()));
        }
        if self.process.is_none() && !self.cleared {
            return Err(Error::Invalid(
                "sound worker stopped; rebind to retry".into(),
            ));
        }
        Ok(())
    }
    /// Inspect bounded parent mappings and the latest child counters.
    /// Takes no arguments; returns counts including consumer pins after cleanup.
    pub fn transfer_stats(&self) -> PcmTransferStats {
        PcmTransferStats {
            mapped_bytes: self.live.load(Ordering::Acquire),
            mapped_handles: self.handles.load(Ordering::Acquire),
            cache_bytes: self.bytes,
            cache_entries: self.entries.len(),
            received_planes: self.received,
            mapping_hits: self.hits,
            spawns: self.spawns,
            child: self.child_stats,
        }
    }
    /// Replace the document under fresh cancellation and publication ownership.
    /// `snapshot` and `cancel` supply the new job. Returns after the previous
    /// child is reaped and a replacement acknowledges the validated document.
    pub fn rebind(
        &mut self,
        snapshot: Arc<EvaluationSnapshot>,
        cancel: Cancellation,
    ) -> Result<()> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.cancel.cancel();
        self.invalidate(false);
        if cancel.is_cancelled() {
            return Err(Error::Invalid("PCM rebind requires a fresh token".into()));
        }
        self.cancel = cancel;
        self.snapshot = snapshot;
        self.owner.version = DocumentVersion::of(self.snapshot.project());
        self.start()
    }
    /// Observe source preparation while an interval request runs elsewhere.
    /// Takes no arguments; returns a bounded control-plane record shared by clone.
    pub fn preparation_log(&self) -> PcmPreparationLog {
        self.preparation.clone()
    }

    fn invalidate(&mut self, cleared: bool) {
        self.process.take();
        self.entries.clear();
        self.bytes = 0;
        self.child_stats = PcmStats::default();
        self.receipt_owner = Arc::new(());
        self.owner.job = Uuid::new_v4();
        self.owner.generation = self.owner.generation.saturating_add(1);
        self.cleared = cleared;
    }
    fn start(&mut self) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.cleared = false;
        self.owner.worker = Uuid::new_v4();
        self.process = Some(Process::start(&self.executable, "--pcm-worker")?);
        self.spawns += 1;
        let (reply, descriptor) = self.rpc(Operation::Bind {
            project: Box::new((**self.snapshot.project()).clone()),
            budget: self.budget,
        })?;
        if descriptor.is_some() || !matches!(reply.result, Reply::Ready) {
            return self.protocol_error("invalid PCM bind acknowledgement");
        }
        Ok(())
    }
    fn protocol_error<T>(&mut self, message: &str) -> Result<T> {
        self.invalidate(false);
        Err(Error::Invalid(message.into()))
    }
    fn rpc(&mut self, operation: Operation) -> Result<(Response, Option<OwnedFd>)> {
        if self.cancel.is_cancelled() {
            self.invalidate(false);
            return Err(Error::Cancelled);
        }
        if self.process.is_none() && self.cleared {
            self.start()?;
        }
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("PCM serial exhausted".into()))?;
        let request = Request {
            owner: self.owner,
            serial: self.serial,
            operation,
        };
        let process = self
            .process
            .as_mut()
            .ok_or_else(|| Error::Invalid("sound worker stopped; rebind to retry".into()))?;
        let packet = match process.exchange(&request, &self.cancel) {
            Ok(packet) => packet,
            Err(error) => {
                self.invalidate(false);
                return Err(error);
            }
        };
        validate_response(&packet.0, self.owner, self.serial)
            .or_else(|e| self.protocol_error(&e.to_string()))?;
        let stats = packet.0.stats;
        let budget = self.budget.pcm;
        if !valid_stats(stats, self.child_stats, budget) {
            return self.protocol_error("invalid or regressing PCM resource accounting");
        }
        if self.cancel.is_cancelled() {
            self.invalidate(false);
            return Err(Error::Cancelled);
        }
        self.child_stats = packet.0.stats;
        if let Reply::Failed {
            message,
            cancelled,
            source_changed,
        } = &packet.0.result
        {
            if packet.1.is_some() {
                return self.protocol_error("failed PCM response supplies a descriptor");
            }
            let error = if *cancelled {
                Error::Cancelled
            } else if *source_changed {
                Error::SourceChanged(message.clone())
            } else {
                Error::Codec(message.clone())
            };
            self.invalidate(false);
            return Err(error);
        }
        Ok(packet)
    }
    fn evict(&mut self) -> bool {
        let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.used)
            .map(|(key, _)| *key)
        else {
            return false;
        };
        if let Some(entry) = self.entries.remove(&key) {
            self.bytes -= std::mem::size_of_val(entry.pcm.samples());
            self.evictions += 1;
        }
        true
    }
    fn reserve(&mut self, bytes: usize) -> Result<(Charge, HandleAllocation)> {
        while (self.live.load(Ordering::Acquire).saturating_add(bytes) > self.budget.pcm.live_bytes
            || self.handles.load(Ordering::Acquire) >= self.budget.live_handles)
            && self.evict()
        {}
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(bytes)
                    .filter(|n| *n <= self.budget.pcm.live_bytes)
            })
            .map_err(|_| Error::Invalid("shared PCM bytes are pinned by consumers".into()))?;
        let charge = Charge {
            bytes,
            live: self.live.clone(),
        };
        self.handles
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(1).filter(|n| *n <= self.budget.live_handles)
            })
            .map_err(|_| Error::Invalid("shared PCM handles are pinned by consumers".into()))?;
        Ok((charge, HandleAllocation(self.handles.clone())))
    }
}
impl PcmProvider for PcmWorker {
    fn budget(&self) -> PcmBudget {
        self.budget.pcm
    }
    fn cancellation(&self) -> &Cancellation {
        &self.cancel
    }
    fn process_id(&self) -> Option<u32> {
        PcmWorker::process_id(self)
    }
    fn snapshot(&self) -> &Arc<EvaluationSnapshot> {
        &self.snapshot
    }
    fn prepare_interval(
        &mut self,
        source: Uuid,
        stream: u32,
        first: i64,
        frames: u32,
    ) -> Result<PcmPreparation> {
        let interval = Interval {
            source,
            stream,
            first,
            frames,
        };
        let selected = select(&self.snapshot, interval, self.budget.pcm)?;
        let (response, descriptor) = self.rpc(Operation::Prepare { interval })?;
        let Reply::Prepared { progress } = response.result else {
            return self.protocol_error("unexpected bounded PCM preparation response");
        };
        let valid = progress.source == source
            && progress.stream == stream
            && progress.first == first
            && progress.frames == frames
            && progress.sample_rate == selected.rate
            && progress.required_frames == selected.required_frames
            && progress.decoded_frames <= selected.required_frames
            && progress.ready == (progress.decoded_frames == selected.required_frames);
        if descriptor.is_some() || !valid {
            return self.protocol_error("invalid bounded PCM preparation response");
        }
        self.preparation.publish(PcmProgress {
            version: self.owner.version,
            request: response.serial,
            preparation: progress,
        });
        Ok(progress)
    }
    fn interval(
        &mut self,
        source: Uuid,
        stream: u32,
        first: i64,
        frames: u32,
    ) -> Result<PcmResult> {
        let interval = Interval {
            source,
            stream,
            first,
            frames,
        };
        let selected = select(&self.snapshot, interval, self.budget.pcm)?;
        let mut prepared = 0;
        let request = self
            .serial
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("PCM serial exhausted".into()))?;
        let (reply, descriptor) = loop {
            let (reply, descriptor) = self.rpc(Operation::Interval { interval })?;
            if let Reply::Preparing { progress } = &reply.result {
                if descriptor.is_some()
                    || !valid_preparation(progress, interval, &selected, prepared)
                {
                    return self.protocol_error("invalid or regressing canonical PCM preparation");
                }
                prepared = progress.decoded_frames;
                self.preparation.publish(PcmProgress {
                    version: self.owner.version,
                    request,
                    preparation: *progress,
                });
                continue;
            }
            break (reply, descriptor);
        };
        validate_pcm_reply(&reply.result, interval, &selected, descriptor.as_ref())
            .or_else(|e| self.protocol_error(&e.to_string()))?;
        let descriptor = descriptor.expect("validated PCM response owns its mapping");
        self.preparation.publish(PcmProgress {
            version: self.owner.version,
            request,
            preparation: PcmPreparation {
                source,
                stream,
                first,
                frames,
                sample_rate: selected.rate,
                decoded_frames: selected.required_frames,
                required_frames: selected.required_frames,
                ready: true,
            },
        });
        self.received += 1;
        let pcm = if let Some(entry) = self.entries.get_mut(&interval) {
            self.hits += 1;
            entry.used = self.serial;
            entry.pcm.clone()
        } else {
            let (charge, handle) = self.reserve(selected.bytes)?;
            let plane = planes::Plane::map(descriptor, selected.bytes)?;
            if plane.floats().iter().any(|v| !v.is_finite()) {
                return self.protocol_error("PCM response contains nonfinite samples");
            }
            let pcm = Arc::new(PcmBlock {
                first_sample: first,
                channels: selected.channels.len(),
                samples: Samples::Shared(plane),
                _charge: charge,
                _handle: Some(handle),
            });
            if selected.bytes <= self.budget.pcm.cache_bytes && self.budget.pcm.cache_entries > 0 {
                while (self.bytes.saturating_add(selected.bytes) > self.budget.pcm.cache_bytes
                    || self.entries.len() >= self.budget.pcm.cache_entries)
                    && self.evict()
                {}
                self.bytes += selected.bytes;
                self.entries.insert(
                    interval,
                    Entry {
                        pcm: pcm.clone(),
                        used: self.serial,
                    },
                );
            }
            pcm
        };
        if self.cancel.is_cancelled() {
            self.invalidate(false);
            return Err(Error::Cancelled);
        }
        Ok(PcmResult {
            pcm,
            version: self.owner.version,
            owner: self.receipt_owner.clone(),
            asset: selected.asset,
        })
    }
    fn validate_result(&mut self, result: &PcmResult) -> Result<()> {
        if self.process.is_none()
            || result.version != self.owner.version
            || !Arc::ptr_eq(&result.owner, &self.receipt_owner)
        {
            return Err(Error::Invalid("stale or foreign PCM worker receipt".into()));
        }
        self.check_sources()
    }
    fn check_sources(&mut self) -> Result<()> {
        let (reply, descriptor) = self.rpc(Operation::Check)?;
        if descriptor.is_some() || !matches!(reply.result, Reply::Checked) {
            return self.protocol_error("invalid PCM source check response");
        }
        Ok(())
    }
    fn verify_sources(&mut self) -> Result<()> {
        let (reply, descriptor) = self.rpc(Operation::Verify)?;
        if descriptor.is_some() || !matches!(reply.result, Reply::Verified) {
            return self.protocol_error("invalid PCM source verification response");
        }
        Ok(())
    }
    fn stats(&self) -> PcmStats {
        PcmStats {
            cache_bytes: self.bytes,
            live_bytes: self.live.load(Ordering::Acquire),
            entries: self.entries.len(),
            evictions: self.evictions,
            ..self.child_stats
        }
    }
    fn clear(&mut self) {
        self.invalidate(true);
    }
}

/// Serve bounded PCM requests before initializing a packaged GUI or CLI.
/// Takes only the inherited private socket; returns on cancellation, EOF or failure.
/// Decode, validation and immutable sample publication remain child-owned.
pub fn serve() -> Result<()> {
    crate::ffi::worker_limits()?;
    serve_socket(planes::inherited()?)
}
fn serve_socket(mut output: std::os::unix::net::UnixStream) -> Result<()> {
    let mut input = output.try_clone()?;
    let cancel = Cancellation::new()?;
    let input_cancel = cancel.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("editbay-pcm-control".into())
        .spawn(move || {
            loop {
                let request = planes::receive(&mut input).and_then(|packet| {
                    let Some((bytes, descriptor)) = packet else {
                        return Err(Error::Cancelled);
                    };
                    if descriptor.is_some() {
                        return Err(Error::Invalid(
                            "PCM requests cannot supply descriptors".into(),
                        ));
                    }
                    serde_json::from_slice::<Request>(&bytes)
                        .map_err(|e| Error::Invalid(e.to_string()))
                });
                let Ok(request) = request else {
                    break;
                };
                if sender.try_send(request).is_err() {
                    break;
                }
            }
            input_cancel.cancel();
        })?;
    let mut current = None;
    let mut serial: Option<u64> = None;
    let mut cache: Option<NativePcmCache> = None;
    let mut budget = PcmBudget::default();
    for request in receiver {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if request.serial == 0 || serial.is_some_and(|n| n.checked_add(1) != Some(request.serial)) {
            return Err(Error::Invalid("PCM request serial is not ordered".into()));
        }
        serial = Some(request.serial);
        let mut samples = None;
        let result = (|| match request.operation {
            Operation::Bind {
                project,
                budget: limits,
            } => {
                limits.validate()?;
                if current.is_some()
                    || request.owner.version != DocumentVersion::of(&project)
                    || [
                        request.owner.session,
                        request.owner.job,
                        request.owner.worker,
                    ]
                    .iter()
                    .any(Uuid::is_nil)
                {
                    return Err(Error::Invalid(
                        "PCM bind has foreign or reused ownership".into(),
                    ));
                }
                budget = limits.pcm;
                cache = Some(NativePcmCache::new_shared(
                    Arc::new(EvaluationSnapshot::new(Arc::new(*project))?),
                    budget,
                    cancel.clone(),
                )?);
                current = Some(request.owner);
                Ok(Reply::Ready)
            }
            operation => {
                if current != Some(request.owner) {
                    return Err(Error::Invalid("PCM request has obsolete ownership".into()));
                }
                let cache = cache
                    .as_mut()
                    .ok_or_else(|| Error::Invalid("bind the PCM worker first".into()))?;
                match operation {
                    Operation::Prepare { interval } => {
                        select(cache.snapshot(), interval, budget)?;
                        let progress = cache.prepare_interval(
                            interval.source,
                            interval.stream,
                            interval.first,
                            interval.frames,
                        )?;
                        Ok(Reply::Prepared { progress })
                    }
                    Operation::Interval { interval } => {
                        let selected = select(cache.snapshot(), interval, budget)?;
                        let progress = cache.prepare_interval(
                            interval.source,
                            interval.stream,
                            interval.first,
                            interval.frames,
                        )?;
                        if !progress.ready {
                            return Ok(Reply::Preparing { progress });
                        }
                        let decoded = cache.interval(
                            interval.source,
                            interval.stream,
                            interval.first,
                            interval.frames,
                        )?;
                        cache.validate_result(&decoded)?;
                        samples = Some(decoded.pcm().clone());
                        Ok(Reply::Pcm {
                            interval,
                            rate: selected.rate,
                            channels: selected.channels,
                        })
                    }
                    Operation::Check => {
                        cache.check_sources()?;
                        Ok(Reply::Checked)
                    }
                    Operation::Verify => {
                        cache.verify_sources()?;
                        Ok(Reply::Verified)
                    }
                    Operation::Bind { .. } => unreachable!(),
                }
            }
        })();
        let failed = result.is_err();
        let response = Response {
            owner: request.owner,
            serial: request.serial,
            stats: cache
                .as_ref()
                .map(NativePcmCache::stats)
                .unwrap_or_default(),
            result: result.unwrap_or_else(|error| Reply::Failed {
                cancelled: matches!(error, Error::Cancelled),
                source_changed: matches!(error, Error::SourceChanged(_)),
                message: error.to_string(),
            }),
        };
        let descriptor = samples.as_ref().and_then(|pcm| match &pcm.samples {
            Samples::Shared(plane) => Some(plane.descriptor()),
            _ => None,
        });
        planes::send(
            &mut output,
            &serde_json::to_vec(&response).map_err(|e| Error::Invalid(e.to_string()))?,
            descriptor,
        )?;
        if failed {
            break;
        }
    }
    Ok(())
}

fn validate_pcm_reply(
    reply: &Reply,
    interval: Interval,
    selected: &Selection,
    descriptor: Option<&OwnedFd>,
) -> Result<()> {
    let Reply::Pcm {
        interval: returned,
        rate,
        channels,
    } = reply
    else {
        return Err(Error::Invalid("unexpected PCM response type".into()));
    };
    if *returned != interval || *rate != selected.rate || *channels != selected.channels {
        return Err(Error::Invalid(
            "PCM response differs from captured samples or channel layout".into(),
        ));
    }
    let descriptor =
        descriptor.ok_or_else(|| Error::Invalid("PCM response has no sample mapping".into()))?;
    planes::Plane::validate(descriptor, selected.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, os::unix::net::UnixStream, time::Duration};

    fn owner(project: &Project) -> Ownership {
        Ownership {
            session: Uuid::new_v4(),
            job: Uuid::new_v4(),
            worker: Uuid::new_v4(),
            version: DocumentVersion::of(project),
            generation: 0,
        }
    }

    #[test]
    fn child_accounting_rejects_over_budget_and_regressing_resources() {
        let budget = PcmBudget::default();
        let stats = PcmStats {
            hits: 1,
            misses: 2,
            evictions: 1,
            decoded_frames: 65536,
            preparation_steps: 1,
            stores: 1,
            store_bytes: 65536 * 4,
            ..PcmStats::default()
        };
        assert!(valid_stats(stats, stats, budget));
        for invalid in [
            PcmStats {
                cache_bytes: 1,
                ..stats
            },
            PcmStats {
                live_bytes: budget.live_bytes + 1,
                ..stats
            },
            PcmStats {
                entries: budget.cache_entries + 1,
                ..stats
            },
            PcmStats {
                sources: budget.source_handles + 1,
                ..stats
            },
            PcmStats {
                decoders: budget.decoder_handles + 1,
                ..stats
            },
            PcmStats {
                stores: budget.store_handles + 1,
                ..stats
            },
            PcmStats {
                store_bytes: budget.store_bytes + 1,
                ..stats
            },
            PcmStats {
                decoder_scratch_bytes: 1,
                ..stats
            },
            PcmStats {
                decoder_scratch_limit_bytes: usize::MAX,
                ..stats
            },
            PcmStats { hits: 0, ..stats },
            PcmStats { misses: 0, ..stats },
            PcmStats {
                evictions: 0,
                ..stats
            },
            PcmStats {
                decoded_frames: 0,
                ..stats
            },
            PcmStats {
                preparation_steps: 0,
                ..stats
            },
        ] {
            assert!(!valid_stats(invalid, stats, budget));
        }
    }

    #[test]
    fn canonical_progress_rejects_foreign_intervals_impossible_totals_and_regression() {
        let interval = Interval {
            source: Uuid::new_v4(),
            stream: 2,
            first: 480000,
            frames: 4096,
        };
        let selected = Selection {
            asset: Uuid::new_v4(),
            bytes: 4096 * 6 * 4,
            rate: 48000,
            channels: vec!["FL".into(); 6],
            required_frames: 484096,
        };
        let valid = PcmPreparation {
            source: interval.source,
            stream: interval.stream,
            first: interval.first,
            frames: interval.frames,
            sample_rate: selected.rate,
            decoded_frames: 65536,
            required_frames: selected.required_frames,
            ready: false,
        };
        assert!(valid_preparation(&valid, interval, &selected, 0));
        assert!(!valid_preparation(
            &valid,
            interval,
            &selected,
            valid.decoded_frames
        ));
        for invalid in [
            PcmPreparation {
                source: Uuid::new_v4(),
                ..valid
            },
            PcmPreparation { stream: 3, ..valid },
            PcmPreparation { first: 0, ..valid },
            PcmPreparation { frames: 1, ..valid },
            PcmPreparation {
                sample_rate: 44100,
                ..valid
            },
            PcmPreparation {
                required_frames: 484095,
                ..valid
            },
            PcmPreparation {
                decoded_frames: 0,
                ..valid
            },
            PcmPreparation {
                decoded_frames: 484096,
                ..valid
            },
            PcmPreparation {
                ready: true,
                ..valid
            },
        ] {
            assert!(!valid_preparation(&invalid, interval, &selected, 0));
        }
    }
    #[test]
    fn pcm_replies_reject_foreign_receipts_intervals_layouts_and_unsealed_payloads() {
        let project = Project::new("Protocol ownership").unwrap();
        let owner = owner(&project);
        let response = |owner, serial| Response {
            owner,
            serial,
            stats: PcmStats::default(),
            result: Reply::Ready,
        };
        validate_response(&response(owner, 42), owner, 42).unwrap();
        for changed in [
            Ownership {
                session: Uuid::new_v4(),
                ..owner
            },
            Ownership {
                job: Uuid::new_v4(),
                ..owner
            },
            Ownership {
                worker: Uuid::new_v4(),
                ..owner
            },
            Ownership {
                generation: 1,
                ..owner
            },
            Ownership {
                version: DocumentVersion {
                    revision: owner.version.revision + 1,
                    ..owner.version
                },
                ..owner
            },
            Ownership {
                version: DocumentVersion {
                    project_id: Uuid::new_v4(),
                    ..owner.version
                },
                ..owner
            },
        ] {
            assert!(validate_response(&response(changed, 42), owner, 42).is_err());
        }
        assert!(validate_response(&response(owner, 41), owner, 42).is_err());
        let interval = Interval {
            source: Uuid::new_v4(),
            stream: 2,
            first: 45,
            frames: 4,
        };
        let selected = Selection {
            asset: Uuid::new_v4(),
            bytes: 32,
            rate: 48000,
            channels: vec!["FL".into(), "FR".into()],
            required_frames: 4,
        };
        let reply = |interval, rate, channels| Reply::Pcm {
            interval,
            rate,
            channels,
        };
        let good = || reply(interval, 48000, selected.channels.clone());
        let plane = planes::Plane::create(&[0u8; 32]).unwrap();
        validate_pcm_reply(&good(), interval, &selected, Some(plane.descriptor())).unwrap();
        assert!(validate_pcm_reply(&good(), interval, &selected, None).is_err());
        for bad in [
            reply(
                Interval {
                    first: 44,
                    ..interval
                },
                48000,
                selected.channels.clone(),
            ),
            reply(
                Interval {
                    source: Uuid::new_v4(),
                    ..interval
                },
                48000,
                selected.channels.clone(),
            ),
            reply(
                Interval {
                    frames: 5,
                    ..interval
                },
                48000,
                selected.channels.clone(),
            ),
            reply(interval, 44100, selected.channels.clone()),
            reply(interval, 48000, vec!["FR".into(), "FL".into()]),
            Reply::Ready,
        ] {
            assert!(
                validate_pcm_reply(&bad, interval, &selected, Some(plane.descriptor())).is_err()
            );
        }
        let short = planes::Plane::create(&[0u8; 16]).unwrap();
        assert!(
            validate_pcm_reply(&good(), interval, &selected, Some(short.descriptor())).is_err()
        );
        let file = tempfile::tempfile().unwrap();
        file.set_len(32).unwrap();
        let descriptor: OwnedFd = File::try_clone(&file).unwrap().into();
        assert!(validate_pcm_reply(&good(), interval, &selected, Some(&descriptor)).is_err());
        let mut json = serde_json::to_value(response(owner, 42)).unwrap();
        json["unsolicited"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Response>(json).is_err());
    }
    #[test]
    fn codec_service_rejects_replayed_serials_and_unsolicited_descriptors() {
        for forged_descriptor in [false, true] {
            let (mut parent, child) = UnixStream::pair().unwrap();
            parent
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let thread = std::thread::spawn(move || serve_socket(child));
            let project = Project::new("Protocol service").unwrap();
            let owner = owner(&project);
            let request = Request {
                owner,
                serial: 1,
                operation: Operation::Bind {
                    project: Box::new(project),
                    budget: PcmWorkerBudget::default(),
                },
            };
            planes::send(&mut parent, &serde_json::to_vec(&request).unwrap(), None).unwrap();
            let (bytes, descriptor) = planes::receive(&mut parent).unwrap().unwrap();
            assert!(descriptor.is_none());
            let response: Response = serde_json::from_slice(&bytes).unwrap();
            assert!(matches!(response.result, Reply::Ready));
            let request = Request {
                owner,
                serial: 1,
                operation: Operation::Check,
            };
            let plane = planes::Plane::create(&[0u8; 4]).unwrap();
            planes::send(
                &mut parent,
                &serde_json::to_vec(&request).unwrap(),
                forged_descriptor.then(|| plane.descriptor()),
            )
            .unwrap();
            if !forged_descriptor {
                assert!(thread.join().unwrap().is_err());
            } else {
                thread.join().unwrap().unwrap();
            }
            parent.shutdown(std::net::Shutdown::Both).unwrap();
        }
    }
}
