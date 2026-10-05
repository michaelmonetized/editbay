use crate::{
    Cancellation, DecodedPicture, Error, PictureBudget, PictureCache, PictureCacheStats,
    PictureProvider, PictureResult, Result,
    codec_process::{Ownership, Process},
    pictures::{Allocation, HandleAllocation, Key, Pixels, selection},
    planes,
};
use editbay_core::{
    AlphaMode, DocumentVersion, EvaluationSnapshot, Project, SourceColor, SourceRequest,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};
use uuid::Uuid;

/// Per-process decoded and mapped output limits, including consumer-held handles.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerBudget {
    pub pictures: PictureBudget,
    pub live_handles: usize,
}
impl Default for WorkerBudget {
    fn default() -> Self {
        Self {
            pictures: PictureBudget::default(),
            live_handles: 256,
        }
    }
}
impl WorkerBudget {
    /// Validate the isolated route against native process and descriptor limits.
    /// Takes this configuration. Returns an error before spawning excessive work.
    pub fn validate(self) -> Result<()> {
        self.pictures.validate()?;
        if self.pictures.cache_entries > 256
            || !(1..=256).contains(&self.live_handles)
            || self.pictures.live_bytes > 1024 * 1024 * 1024
        {
            return Err(Error::Invalid(
                "picture IPC budgets exceed process limits".into(),
            ));
        }
        Ok(())
    }
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
        budget: WorkerBudget,
    },
    Picture {
        request: SourceRequest,
    },
    Verify,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    width: u32,
    height: u32,
    tick: i64,
    color: SourceColor,
    alpha: AlphaMode,
    alpha_interpretation_required: bool,
    rotation_degrees: f64,
}
impl Header {
    fn of(picture: &DecodedPicture) -> Self {
        Self {
            width: picture.width,
            height: picture.height,
            tick: picture.source_tick,
            color: picture.color,
            alpha: picture.alpha,
            alpha_interpretation_required: picture.alpha_interpretation_required,
            rotation_degrees: picture.rotation_degrees,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    owner: Ownership,
    serial: u64,
    stats: PictureCacheStats,
    result: Reply,
}
fn validate_response(response: &Response, owner: Ownership, serial: u64) -> Result<()> {
    if response.owner != owner || response.serial != serial {
        return Err(Error::Invalid(
            "foreign or obsolete picture response".into(),
        ));
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Ready,
    Picture {
        request: SourceRequest,
        header: Option<Header>,
        cache_hit: bool,
    },
    Verified,
    Failed {
        message: String,
        cancelled: bool,
        source_changed: bool,
    },
}

type Packet = (Response, Option<OwnedFd>);

struct Entry {
    picture: Arc<DecodedPicture>,
    used: u64,
}

/// Actual cross-process ownership and transfers, separate from native decode counters.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct TransferStats {
    pub mapped_bytes: usize,
    pub mapped_handles: usize,
    pub cache_bytes: usize,
    pub cache_entries: usize,
    pub received_planes: u64,
    pub mapping_hits: u64,
    pub spawns: u64,
    pub child: PictureCacheStats,
}

/// Supervised retained codec process and bounded immutable shared-picture provider.
pub struct PictureWorker {
    executable: PathBuf,
    snapshot: Arc<EvaluationSnapshot>,
    budget: WorkerBudget,
    cancel: Cancellation,
    owner: Ownership,
    serial: u64,
    process: Option<Process<Response>>,
    entries: HashMap<Key, Entry>,
    bytes: usize,
    live: Arc<AtomicUsize>,
    handles: Arc<AtomicUsize>,
    child_stats: PictureCacheStats,
    received: u64,
    hits: u64,
    evictions: u64,
    spawns: u64,
    cleared: bool,
}
impl PictureWorker {
    /// Detect an idle codec process exit without requesting or copying a picture.
    /// Takes this supervisor. Returns a visible failure and reaps a dead child;
    /// subsequent work requires an explicit fresh rebind.
    pub fn check_health(&mut self) -> Result<()> {
        let exited = self
            .process
            .as_mut()
            .map(|process| process.child.try_wait())
            .transpose()?;
        if exited.flatten().is_some() {
            self.process.take();
            return Err(Error::Invalid(
                "picture worker exited; retry the viewer".into(),
            ));
        }
        Ok(())
    }
    /// Start the packaged Rust codec route over a captured immutable document.
    /// `executable` supports --picture-worker; `snapshot`, `budget` and `cancel`
    /// declare granted sources and limits. Returns after the child's validated bind.
    pub fn new(
        executable: &Path,
        snapshot: Arc<EvaluationSnapshot>,
        budget: WorkerBudget,
        cancel: Cancellation,
    ) -> Result<Self> {
        budget.validate()?;
        let mut worker = Self {
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
            serial: 0,
            process: None,
            entries: HashMap::new(),
            bytes: 0,
            live: Arc::new(AtomicUsize::new(0)),
            handles: Arc::new(AtomicUsize::new(0)),
            child_stats: PictureCacheStats::default(),
            received: 0,
            hits: 0,
            evictions: 0,
            spawns: 0,
            cleared: false,
        };
        worker.start()?;
        Ok(worker)
    }
    /// Inspect the owned child for process supervision and qualification.
    /// Takes no arguments. Returns no PID after worker termination.
    pub fn process_id(&self) -> Option<u32> {
        self.process.as_ref().map(|p| p.child.id())
    }
    /// Inspect mapped consumer pins and actual child decode/cache activity.
    /// Takes no arguments. Returns counters; child counts are the last response.
    pub fn transfer_stats(&self) -> TransferStats {
        TransferStats {
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
    fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
    fn start(&mut self) -> Result<()> {
        self.check()?;
        self.cleared = false;
        self.owner.worker = Uuid::new_v4();
        self.process = Some(Process::start(&self.executable, "--picture-worker")?);
        self.spawns += 1;
        self.bind()
    }
    fn bind(&mut self) -> Result<()> {
        let (reply, descriptor) = self.rpc(Operation::Bind {
            project: Box::new((**self.snapshot.project()).clone()),
            budget: self.budget,
        })?;
        if descriptor.is_some() || !matches!(reply.result, Reply::Ready) {
            return self.protocol_error("invalid picture bind acknowledgement");
        }
        Ok(())
    }
    fn protocol_error<T>(&mut self, message: &str) -> Result<T> {
        self.process.take();
        self.child_stats = PictureCacheStats::default();
        Err(Error::Invalid(message.into()))
    }
    fn rpc(&mut self, operation: Operation) -> Result<Packet> {
        self.check()?;
        if self.process.is_none() && self.cleared {
            self.start()?;
        }
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("picture message serial exhausted".into()))?;
        let request = Request {
            owner: self.owner,
            serial: self.serial,
            operation,
        };
        let process = self.process.as_mut().ok_or_else(|| {
            Error::Invalid("picture worker stopped; rebind with a fresh job to retry".into())
        })?;
        let packet = match process.exchange(&request, &self.cancel) {
            Ok(packet) => packet,
            Err(error) => {
                self.process.take();
                self.child_stats = PictureCacheStats::default();
                return Err(error);
            }
        };
        if validate_response(&packet.0, self.owner, self.serial).is_err() {
            return self.protocol_error("foreign or obsolete picture response");
        }
        self.check()?;
        self.child_stats = packet.0.stats;
        if let Reply::Failed {
            message,
            cancelled,
            source_changed,
        } = &packet.0.result
        {
            if packet.1.is_some() {
                return self.protocol_error("failed picture response includes a descriptor");
            }
            return Err(if *cancelled {
                Error::Cancelled
            } else if *source_changed {
                Error::SourceChanged(message.clone())
            } else {
                Error::Codec(message.clone())
            });
        }
        Ok(packet)
    }
    fn evict(&mut self) -> bool {
        let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.used)
            .map(|(key, _)| key.clone())
        else {
            return false;
        };
        if let Some(entry) = self.entries.remove(&key) {
            self.bytes -= entry.picture.rgba().len();
            self.evictions += 1;
        }
        true
    }
    fn reserve(&mut self, bytes: usize) -> Result<(Allocation, HandleAllocation)> {
        while (self.live.load(Ordering::Acquire).saturating_add(bytes)
            > self.budget.pictures.live_bytes
            || self.handles.load(Ordering::Acquire) >= self.budget.live_handles)
            && self.evict()
        {}
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes)
                    .filter(|v| *v <= self.budget.pictures.live_bytes)
            })
            .map_err(|_| Error::Invalid("shared picture bytes are pinned by consumers".into()))?;
        let allocation = Allocation {
            bytes,
            live: self.live.clone(),
        };
        self.handles
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(1).filter(|n| *n <= self.budget.live_handles)
            })
            .map_err(|_| Error::Invalid("shared picture handles are pinned by consumers".into()))?;
        Ok((allocation, HandleAllocation(self.handles.clone())))
    }
}
impl PictureProvider for PictureWorker {
    fn check_health(&mut self) -> Result<()> {
        PictureWorker::check_health(self)
    }
    fn version(&self) -> DocumentVersion {
        self.owner.version
    }
    fn picture(&mut self, request: &SourceRequest) -> Result<Option<PictureResult>> {
        self.check()?;
        let snapshot = self.snapshot.clone();
        let selected = selection(&snapshot, request, self.budget.pictures)?;
        let (reply, descriptor) = self.rpc(Operation::Picture {
            request: request.clone(),
        })?;
        let Reply::Picture {
            request: returned,
            header,
            cache_hit,
        } = reply.result
        else {
            return self.protocol_error("unexpected picture response type");
        };
        if returned != *request {
            return self.protocol_error("picture response does not own its requested source/time");
        }
        let (selected, header, descriptor) = match (selected, header, descriptor) {
            (None, None, None) => return Ok(None),
            (Some(selected), Some(header), Some(descriptor)) => (selected, header, descriptor),
            _ => {
                return self.protocol_error(
                    "picture response lacks its expected plane or supplies unsolicited pixels",
                );
            }
        };
        if header.width != selected.width
            || header.height != selected.height
            || header.tick != selected.tick
            || !header.rotation_degrees.is_finite()
        {
            return self
                .protocol_error("picture header differs from captured source geometry/time");
        }
        if let Err(error) = planes::Plane::validate(&descriptor, selected.size) {
            return self.protocol_error(&error.to_string());
        }
        self.received += 1;
        let picture = if let Some(entry) = self.entries.get_mut(&selected.key) {
            if Header::of(&entry.picture) != header {
                return self.protocol_error("cached plane interpretation changed");
            }
            self.hits += 1;
            entry.used = self.serial;
            entry.picture.clone()
        } else {
            let (allocation, handle) = self.reserve(selected.size)?;
            let plane = planes::Plane::map(descriptor, selected.size)?;
            let picture = Arc::new(DecodedPicture {
                rgba: Pixels::Shared(plane),
                width: header.width,
                height: header.height,
                source_tick: header.tick,
                color: header.color,
                alpha: header.alpha,
                alpha_interpretation_required: header.alpha_interpretation_required,
                rotation_degrees: header.rotation_degrees,
                _allocation: allocation,
                _handle: Some(handle),
            });
            if selected.size <= self.budget.pictures.cache_bytes
                && self.budget.pictures.cache_entries > 0
            {
                while (self.bytes.saturating_add(selected.size) > self.budget.pictures.cache_bytes
                    || self.entries.len() >= self.budget.pictures.cache_entries)
                    && self.evict()
                {}
                self.bytes += selected.size;
                self.entries.insert(
                    selected.key,
                    Entry {
                        picture: picture.clone(),
                        used: self.serial,
                    },
                );
            }
            picture
        };
        self.check()?;
        Ok(Some(PictureResult {
            version: self.owner.version,
            generation: self.owner.generation,
            picture,
            cache_hit,
            owner: self.owner.job,
        }))
    }
    fn validate_result(&self, result: &PictureResult) -> Result<()> {
        self.check()?;
        if self.process.is_none()
            || result.owner != self.owner.job
            || result.version != self.owner.version
            || result.generation != self.owner.generation
        {
            return Err(Error::Invalid(
                "stale or foreign picture-worker receipt".into(),
            ));
        }
        Ok(())
    }
    fn verify_sources(&mut self) -> Result<()> {
        let (reply, descriptor) = self.rpc(Operation::Verify)?;
        if descriptor.is_some() || !matches!(reply.result, Reply::Verified) {
            return self.protocol_error("invalid complete-source verification response");
        }
        Ok(())
    }
    fn stats(&self) -> PictureCacheStats {
        PictureCacheStats {
            cache_bytes: self.bytes,
            live_bytes: self.live.load(Ordering::Acquire),
            entries: self.entries.len(),
            sources: self.child_stats.sources,
            decoders: self.child_stats.decoders,
            hits: self.child_stats.hits,
            misses: self.child_stats.misses,
            evictions: self.evictions,
            seeks: self.child_stats.seeks,
            sequential_decodes: self.child_stats.sequential_decodes,
        }
    }
    fn clear(&mut self) -> Result<()> {
        self.owner.generation = self
            .owner
            .generation
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("picture generation exhausted".into()))?;
        self.owner.job = Uuid::new_v4();
        self.process.take();
        self.entries.clear();
        self.bytes = 0;
        self.child_stats = PictureCacheStats::default();
        self.cleared = true;
        Ok(())
    }
    fn rebind(&mut self, snapshot: Arc<EvaluationSnapshot>, cancel: Cancellation) -> Result<()> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let generation = self
            .owner
            .generation
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("picture generation exhausted".into()))?;
        self.cancel.cancel();
        if cancel.is_cancelled() {
            return Err(Error::Invalid(
                "picture rebind needs a fresh cancellation token".into(),
            ));
        }
        self.cancel = cancel;
        self.snapshot = snapshot;
        self.owner.version = DocumentVersion::of(self.snapshot.project());
        self.owner.generation = generation;
        self.owner.job = Uuid::new_v4();
        if self.process.is_some() {
            self.bind()
        } else {
            self.start()
        }
    }
}

/// Serve the retained codec endpoint before packaged GUI/CLI initialization.
/// Takes only its inherited private socket. Returns on cancellation/EOF/failure;
/// scoped documents, immutable planes and native handles remain process-owned.
pub fn serve() -> Result<()> {
    crate::ffi::worker_limits()?;
    let mut output = planes::inherited()?;
    let mut input = output.try_clone()?;
    let control = Arc::new(Mutex::new(Cancellation::new()?));
    let input_control = control.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("editbay-picture-control".into())
        .spawn(move || {
            loop {
                let request = planes::receive(&mut input).and_then(|packet| {
                    let Some((bytes, descriptor)) = packet else {
                        return Err(Error::Cancelled);
                    };
                    if descriptor.is_some() {
                        return Err(Error::Invalid(
                            "picture requests cannot supply descriptors".into(),
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
            if let Ok(cancel) = input_control.lock() {
                cancel.cancel();
            }
        })?;
    let mut current: Option<Ownership> = None;
    let mut serial = None;
    let mut cache: Option<PictureCache> = None;
    for request in receiver {
        if serial.is_some_and(|serial: u64| serial.checked_add(1) != Some(request.serial))
            || request.serial == 0
        {
            return Err(Error::Invalid(
                "picture request serial is not ordered".into(),
            ));
        }
        serial = Some(request.serial);
        let mut picture = None;
        let result = (|| match request.operation {
            Operation::Bind { project, budget } => {
                budget.validate()?;
                if request.owner.version != DocumentVersion::of(&project)
                    || [
                        request.owner.session,
                        request.owner.job,
                        request.owner.worker,
                    ]
                    .iter()
                    .any(Uuid::is_nil)
                    || current.is_some_and(|old| {
                        old.session != request.owner.session
                            || old.worker != request.owner.worker
                            || old.generation.checked_add(1) != Some(request.owner.generation)
                    })
                {
                    return Err(Error::Invalid("picture bind has foreign ownership".into()));
                }
                let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(*project))?);
                let cancel = Cancellation::new()?;
                let mut control = control
                    .lock()
                    .map_err(|_| Error::Invalid("picture control lock failed".into()))?;
                if let Some(cache) = cache.as_mut() {
                    cache.rebind(snapshot, cancel.clone())?;
                } else {
                    cache = Some(PictureCache::new_shared(
                        snapshot,
                        budget.pictures,
                        cancel.clone(),
                    )?);
                }
                *control = cancel;
                current = Some(request.owner);
                Ok(Reply::Ready)
            }
            operation => {
                if current != Some(request.owner) {
                    return Err(Error::Invalid(
                        "picture request has obsolete ownership".into(),
                    ));
                }
                let cache = cache
                    .as_mut()
                    .ok_or_else(|| Error::Invalid("bind the picture worker first".into()))?;
                match operation {
                    Operation::Picture { request } => {
                        let decoded = cache.picture(&request)?;
                        let header = decoded.as_ref().map(|result| Header::of(&result.picture));
                        let cache_hit = decoded.as_ref().is_some_and(|result| result.cache_hit);
                        picture = decoded.map(|result| result.picture);
                        Ok(Reply::Picture {
                            request,
                            header,
                            cache_hit,
                        })
                    }
                    Operation::Verify => {
                        cache.verify_sources()?;
                        Ok(Reply::Verified)
                    }
                    Operation::Bind { .. } => unreachable!(),
                }
            }
        })();
        let response = Response {
            owner: request.owner,
            serial: request.serial,
            stats: cache.as_ref().map(PictureCache::stats).unwrap_or_default(),
            result: result.unwrap_or_else(|error| Reply::Failed {
                cancelled: matches!(error, Error::Cancelled),
                source_changed: matches!(error, Error::SourceChanged(_)),
                message: error.to_string(),
            }),
        };
        let descriptor = picture.as_ref().and_then(|picture| match &picture.rgba {
            Pixels::Shared(plane) => Some(plane.descriptor()),
            _ => None,
        });
        planes::send(
            &mut output,
            &serde_json::to_vec(&response).map_err(|e| Error::Invalid(e.to_string()))?,
            descriptor,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_ownership_rejects_foreign_sessions_jobs_workers_versions_generations_and_serials() {
        let owner = Ownership {
            session: Uuid::new_v4(),
            job: Uuid::new_v4(),
            worker: Uuid::new_v4(),
            version: DocumentVersion {
                project_id: Uuid::new_v4(),
                revision: 7,
            },
            generation: 3,
        };
        let response = |owner, serial| Response {
            owner,
            serial,
            stats: PictureCacheStats::default(),
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
                version: DocumentVersion {
                    revision: 6,
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
            Ownership {
                generation: 2,
                ..owner
            },
        ] {
            assert!(validate_response(&response(changed, 42), owner, 42).is_err());
        }
        assert!(validate_response(&response(owner, 41), owner, 42).is_err());
        assert!(validate_response(&response(owner, 43), owner, 42).is_err());
        let mut value = serde_json::to_value(response(owner, 42)).unwrap();
        value["unsolicited"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Response>(value).is_err());
    }
}
