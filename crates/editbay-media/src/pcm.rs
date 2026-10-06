use crate::{Cancellation, Error, NativeAudioReader, Result, SourceFile};
use editbay_core::{DocumentVersion, EvaluationSnapshot, FrameRate, SourcePosition, StreamFormat};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use uuid::Uuid;
mod progress;
mod store;
pub use progress::{PcmPreparationLog, PcmProgress};
use store::Store;

/// Limits for retained original-channel PCM and native decoder cursors.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PcmBudget {
    pub cache_bytes: usize,
    pub live_bytes: usize,
    pub interval_frames: u32,
    pub cache_entries: usize,
    pub source_handles: usize,
    pub decoder_handles: usize,
    pub decode_frames: u32,
    pub store_bytes: u64,
    pub store_handles: usize,
}
impl Default for PcmBudget {
    fn default() -> Self {
        Self {
            cache_bytes: 32 * 1024 * 1024,
            live_bytes: 64 * 1024 * 1024,
            interval_frames: 131072,
            cache_entries: 64,
            source_handles: 8,
            decoder_handles: 2,
            decode_frames: 262144,
            store_bytes: 8 * 1024 * 1024 * 1024,
            store_handles: 16,
        }
    }
}
impl PcmBudget {
    /// Validate memory, request and native work bounds.
    /// Takes this configuration; returns an error for excessive limits.
    pub fn validate(self) -> Result<()> {
        if self.cache_bytes > 512 * 1024 * 1024
            || self.live_bytes == 0
            || self.live_bytes > 1024 * 1024 * 1024
            || !(1..=262144).contains(&self.interval_frames)
            || self.cache_entries > 1024
            || !(1..=32).contains(&self.source_handles)
            || !(1..=8).contains(&self.decoder_handles)
            || !(65536..=1048576).contains(&self.decode_frames)
            || self.store_bytes == 0
            || self.store_bytes > 8 * 1024 * 1024 * 1024
            || !(1..=32).contains(&self.store_handles)
        {
            return Err(Error::Invalid("PCM budgets exceed supported limits".into()));
        }
        Ok(())
    }
}

/// Actual retained PCM, decoder scratch and source activity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PcmStats {
    pub cache_bytes: usize,
    pub live_bytes: usize,
    pub decoder_scratch_bytes: usize,
    pub decoder_scratch_limit_bytes: usize,
    pub entries: usize,
    pub sources: usize,
    pub decoders: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub seeks: u64,
    pub decoded_frames: u64,
    pub store_bytes: u64,
    pub stores: usize,
    pub preparation_steps: u64,
}

/// Progress through canonical source history for one requested PCM interval.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PcmPreparation {
    pub source: Uuid,
    pub stream: u32,
    pub first: i64,
    pub frames: u32,
    pub sample_rate: u32,
    pub decoded_frames: u64,
    pub required_frames: u64,
    pub ready: bool,
}
pub(crate) struct Charge {
    pub(crate) bytes: usize,
    pub(crate) live: Arc<AtomicUsize>,
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.live.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Immutable PCM whose memory remains charged while any consumer retains it.
pub struct PcmBlock {
    pub(crate) first_sample: i64,
    pub(crate) channels: usize,
    pub(crate) samples: Samples,
    pub(crate) _charge: Charge,
    pub(crate) _handle: Option<crate::pictures::HandleAllocation>,
}
impl PcmBlock {
    /// Inspect absolute original-rate sample boundaries and channel count.
    /// Takes no arguments; returns first sample, frame count and channel count.
    pub fn interval(&self) -> (i64, usize, usize) {
        (
            self.first_sample,
            self.samples().len() / self.channels,
            self.channels,
        )
    }
    /// Inspect unchanged-channel interleaved float samples.
    /// Takes no arguments; returns immutable PCM, retaining finite headroom.
    pub fn samples(&self) -> &[f32] {
        self.samples.as_slice()
    }
}

/// A native PCM result with private cache-generation ownership.
/// Payload replacement cannot preserve a valid publication receipt.
/// ```compile_fail,E0616
/// fn replace(a: &mut editbay_media::PcmResult, b: editbay_media::PcmResult) {
///     a.pcm = b.pcm;
/// }
/// ```
pub struct PcmResult {
    pub(crate) pcm: Arc<PcmBlock>,
    pub(crate) version: DocumentVersion,
    pub(crate) owner: Arc<()>,
    pub(crate) asset: Uuid,
}
impl PcmResult {
    /// Retain this receipt's immutable original-channel PCM.
    /// Takes no arguments; returns the exact charged buffer without payload mutation.
    pub fn pcm(&self) -> &Arc<PcmBlock> {
        &self.pcm
    }
    /// Inspect the captured document publication owner.
    /// Takes no arguments; returns this receipt's immutable document version.
    pub fn version(&self) -> DocumentVersion {
        self.version
    }
}
struct Entry {
    pcm: Arc<PcmBlock>,
    used: u64,
}
struct Decoder {
    reader: NativeAudioReader,
    cursor: i64,
    used: u64,
}

/// Worker-side retained original-rate PCM; never called by an audio callback.
pub struct NativePcmCache {
    snapshot: Arc<EvaluationSnapshot>,
    budget: PcmBudget,
    cancel: Cancellation,
    owner: Arc<()>,
    sources: HashMap<Uuid, Arc<SourceFile>>,
    decoders: HashMap<(Uuid, u32), Decoder>,
    entries: HashMap<String, Entry>,
    live: Arc<AtomicUsize>,
    stats: PcmStats,
    serial: u64,
    shared: bool,
    directory: PathBuf,
    stores: HashMap<(Uuid, u32), Store>,
}
impl NativePcmCache {
    /// Bind native decoding to one immutable document.
    /// `snapshot` owns source interpretations, `budget` bounds resources and
    /// `cancel` interrupts native IO. Returns an empty retained worker cache.
    pub fn new(
        snapshot: Arc<EvaluationSnapshot>,
        budget: PcmBudget,
        cancel: Cancellation,
    ) -> Result<Self> {
        Self::new_in(snapshot, budget, cancel, &std::env::temp_dir())
    }

    /// Bind canonical PCM to a selected temporary-storage directory.
    /// `snapshot`, `budget` and `cancel` own work; `directory` hosts anonymous
    /// files. Returns an empty cache without opening a source or allocating disk.
    pub fn new_in(
        snapshot: Arc<EvaluationSnapshot>,
        budget: PcmBudget,
        cancel: Cancellation,
        directory: &Path,
    ) -> Result<Self> {
        budget.validate()?;
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(Self {
            snapshot,
            budget,
            cancel,
            owner: Arc::new(()),
            sources: HashMap::new(),
            decoders: HashMap::new(),
            entries: HashMap::new(),
            live: Arc::new(AtomicUsize::new(0)),
            stats: PcmStats::default(),
            serial: 0,
            shared: false,
            directory: directory.to_owned(),
            stores: HashMap::new(),
        })
    }

    /// Bind native decoding to charged sealed sample mappings.
    /// `snapshot`, `budget` and `cancel` declare ownership and limits; returns
    /// an empty cache that writes directly into shared PCM for codec transport.
    pub(crate) fn new_shared(
        snapshot: Arc<EvaluationSnapshot>,
        budget: PcmBudget,
        cancel: Cancellation,
    ) -> Result<Self> {
        let mut cache = Self::new(snapshot, budget, cancel)?;
        cache.shared = true;
        Ok(cache)
    }

    /// Prepare one bounded step of canonical sequential source history.
    /// `source`, `stream`, `first` and `frames` select the eventual interval.
    /// Returns exact progress; repeat until ready before reading a late interval.
    pub fn prepare_interval(
        &mut self,
        source: Uuid,
        stream: u32,
        first: i64,
        frames: u32,
    ) -> Result<PcmPreparation> {
        self.check()?;
        if frames == 0 || frames > self.budget.interval_frames {
            return Err(Error::Invalid(
                "PCM interval exceeds its frame budget".into(),
            ));
        }
        let end = first
            .checked_add(i64::from(frames))
            .ok_or_else(|| Error::Invalid("PCM interval overflow".into()))?;
        let (asset, profile, _) = self.snapshot.source_stream(source, stream)?;
        let asset = asset.clone();
        let profile = profile.clone();
        let StreamFormat::Audio {
            sample_rate,
            ref channels,
        } = profile.format
        else {
            return Err(Error::Invalid("PCM source is not sound".into()));
        };
        if frames as usize * channels.len() * 4 > self.budget.live_bytes {
            return Err(Error::Invalid(
                "PCM interval exceeds its byte budget".into(),
            ));
        }
        let (start, limit) = presentation(&profile, sample_rate)?;
        let wanted_start = first.max(start);
        let wanted_end = end.min(limit);
        let required = if wanted_start < wanted_end {
            u64::try_from(i128::from(wanted_end) - i128::from(start))
                .map_err(|e| Error::Invalid(e.to_string()))?
        } else {
            0
        };
        let mut progress = PcmPreparation {
            source,
            stream,
            first,
            frames,
            sample_rate,
            decoded_frames: required,
            required_frames: required,
            ready: true,
        };
        let owned = self.source(&asset)?;
        owned.check_current(&self.cancel)?;
        if required == 0 {
            return Ok(progress);
        }
        let key = (source, stream);
        if self
            .stores
            .get(&key)
            .is_some_and(|store| store.end() >= wanted_end)
        {
            return Ok(progress);
        }
        let required_bytes = required
            .checked_mul(channels.len() as u64 * 4)
            .ok_or_else(|| Error::Invalid("canonical PCM storage size overflow".into()))?;
        let existing = self.stores.get(&key).map_or(0, Store::bytes);
        if self
            .stats
            .store_bytes
            .checked_add(required_bytes.saturating_sub(existing))
            .is_none_or(|bytes| bytes > self.budget.store_bytes)
        {
            return Err(Error::Invalid(
                "canonical PCM exceeds its disk byte budget".into(),
            ));
        }
        if !self.stores.contains_key(&key) {
            if self.stores.len() >= self.budget.store_handles {
                return Err(Error::Invalid(
                    "canonical PCM exceeds its storage handle budget".into(),
                ));
            }
            self.stores
                .insert(key, Store::new(&self.directory, start, channels.len())?);
        }
        self.serial = self.serial.saturating_add(1);
        if !self.decoders.contains_key(&key) {
            if self.decoders.len() >= self.budget.decoder_handles {
                let oldest = self
                    .decoders
                    .iter()
                    .min_by_key(|(_, decoder)| decoder.used)
                    .map(|(key, _)| *key)
                    .unwrap();
                self.decoders.remove(&oldest);
            }
            let probe = owned.probe(self.cancel.clone())?;
            let native = probe
                .streams
                .iter()
                .find(|s| s.index == stream)
                .ok_or_else(|| Error::Invalid("native PCM stream absent".into()))?;
            let original_base = profile.metadata.get("editbay.original_time_base");
            let native_base = native
                .time_base
                .ok_or_else(|| Error::Invalid("native sound time base is absent".into()))?;
            if native.sample_rate != Some(sample_rate)
                || native.channels != *channels
                || original_base.map_or(native_base != profile.time_base, |base| {
                    *base != format!("{}/{}", native_base.numerator, native_base.denominator)
                })
            {
                return Err(Error::Invalid(
                    "native PCM interpretation differs from captured stream".into(),
                ));
            }
            let reader = NativeAudioReader::open_stream(&owned, stream, self.cancel.clone())?;
            if reader.info.sample_rate != sample_rate as i32
                || reader.info.channels != channels.len() as i32
            {
                return Err(Error::Invalid(
                    "native PCM decoder changed rate or channels".into(),
                ));
            }
            self.decoders.insert(
                key,
                Decoder {
                    reader,
                    cursor: start,
                    used: self.serial,
                },
            );
        }
        let decoder = self.decoders.get_mut(&key).unwrap();
        decoder.used = self.serial;
        let store = self.stores.get_mut(&key).unwrap();
        let mut decoded = 0u64;
        let mut blocks = 0u32;
        while store.end() < wanted_end
            && decoded + 65536 <= u64::from(self.budget.decode_frames)
            && blocks < 4096
        {
            if self.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let block = decoder.reader.next_block()?.ok_or_else(|| {
                Error::Invalid("native PCM ended before its captured presentation end".into())
            })?;
            let first_sample = block
                .first_sample
                .ok_or_else(|| Error::Invalid("native PCM has no absolute sample clock".into()))?;
            if first_sample != decoder.cursor {
                return Err(Error::Invalid(
                    "canonical PCM decode lost sequential sample continuity".into(),
                ));
            }
            let count = (block.samples.len() / channels.len()) as u64;
            decoder.cursor = first_sample
                .checked_add(count as i64)
                .ok_or_else(|| Error::Invalid("canonical PCM clock overflow".into()))?;
            self.stats.store_bytes += store.append(
                &block,
                self.budget.store_bytes - self.stats.store_bytes,
                &self.cancel,
            )?;
            decoded += count;
            self.stats.decoded_frames += count;
            blocks += 1;
        }
        self.stats.preparation_steps += 1;
        progress.ready = store.end() >= wanted_end;
        progress.decoded_frames = if progress.ready {
            required
        } else {
            u64::try_from(i128::from(decoder.cursor) - i128::from(start))
                .map_err(|e| Error::Invalid(e.to_string()))?
                .min(required)
        };
        owned.check_current(&self.cancel)?;
        Ok(progress)
    }

    /// Read a bounded interval in absolute original-rate sample units.
    /// `source` and `stream` select captured media; `first` and `frames` select
    /// PCM. Returns float samples in the exact original channel order. Declared
    /// presentation edges are zero padded; missing in-range decoded PCM is an error.
    pub fn interval(
        &mut self,
        source: Uuid,
        stream: u32,
        first: i64,
        frames: u32,
    ) -> Result<PcmResult> {
        self.check()?;
        if frames == 0 || frames > self.budget.interval_frames {
            return Err(Error::Invalid(
                "PCM interval exceeds its frame budget".into(),
            ));
        }
        let end = first
            .checked_add(i64::from(frames))
            .ok_or_else(|| Error::Invalid("PCM interval overflow".into()))?;
        let (asset, profile, fingerprint) = self.snapshot.source_stream(source, stream)?;
        let asset = asset.clone();
        let profile = profile.clone();
        let StreamFormat::Audio {
            sample_rate,
            ref channels,
        } = profile.format
        else {
            return Err(Error::Invalid("PCM source is not sound".into()));
        };
        let key = format!(
            "{}:{}:{}:{fingerprint}:{first}:{frames}",
            asset.id, asset.sha256, asset.bytes
        );
        let owned = self.source(&asset)?;
        owned.check_current(&self.cancel)?;
        self.serial = self.serial.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = self.serial;
            self.stats.hits += 1;
            return Ok(PcmResult {
                pcm: entry.pcm.clone(),
                version: DocumentVersion::of(self.snapshot.project()),
                owner: self.owner.clone(),
                asset: asset.id,
            });
        }
        self.stats.misses += 1;
        let bytes = frames as usize * channels.len() * 4;
        while !self.entries.is_empty()
            && (self.stats.cache_bytes.saturating_add(bytes) > self.budget.cache_bytes
                || self.entries.len() >= self.budget.cache_entries
                || self.live.load(Ordering::Acquire).saturating_add(bytes) > self.budget.live_bytes)
        {
            self.evict();
        }
        let charge = self.reserve(bytes)?;
        let mut output = if self.shared {
            Output::Shared(crate::planes::MutablePlane::new(bytes)?)
        } else {
            Output::Owned(vec![0.; frames as usize * channels.len()])
        };
        let (start, limit) = presentation(&profile, sample_rate)?;
        let wanted_start = first.max(start);
        let wanted_end = end.min(limit);
        if wanted_start < wanted_end {
            while !self.prepare_interval(source, stream, first, frames)?.ready {}
            let offset = (wanted_start - first) as usize * channels.len();
            let count = (wanted_end - wanted_start) as usize * channels.len();
            self.stores.get(&(source, stream)).unwrap().read(
                wanted_start,
                &mut output.samples_mut()[offset..offset + count],
                &self.cancel,
            )?;
        }
        owned.check_current(&self.cancel)?;
        let pcm = Arc::new(PcmBlock {
            first_sample: first,
            channels: channels.len(),
            samples: output.freeze()?,
            _charge: charge,
            _handle: None,
        });
        if bytes <= self.budget.cache_bytes && self.budget.cache_entries > 0 {
            self.stats.cache_bytes += bytes;
            self.entries.insert(
                key,
                Entry {
                    pcm: pcm.clone(),
                    used: self.serial,
                },
            );
        }
        Ok(PcmResult {
            pcm,
            version: DocumentVersion::of(self.snapshot.project()),
            owner: self.owner.clone(),
            asset: asset.id,
        })
    }

    /// Check private publication ownership and current source identity.
    /// `result` is a retained receipt; returns an error for foreign, rebound,
    /// cancelled or changed-source publications even when public versions match.
    pub fn validate_result(&self, result: &PcmResult) -> Result<()> {
        self.check()?;
        if !Arc::ptr_eq(&self.owner, &result.owner)
            || result.version != DocumentVersion::of(self.snapshot.project())
        {
            return Err(Error::Invalid(
                "PCM result belongs to an obsolete or foreign worker".into(),
            ));
        }
        self.sources
            .get(&result.asset)
            .ok_or_else(|| Error::Invalid("PCM source ownership was released".into()))?
            .check_current(&self.cancel)
    }

    /// Verify every retained source before durable job publication.
    /// Takes no arguments; returns after complete source checksums match again.
    pub fn verify_sources(&self) -> Result<()> {
        self.check()?;
        for source in self.sources.values() {
            source.verify(&self.cancel)?;
        }
        Ok(())
    }

    /// Check retained path, inode and mutation stamps before publication.
    /// Takes no arguments; returns an error for cancelled or replaced sources.
    pub fn check_sources(&self) -> Result<()> {
        self.check()?;
        for source in self.sources.values() {
            source.check_current(&self.cancel)?;
        }
        Ok(())
    }

    /// Inspect cache residency, consumer pins and bounded decoder scratch.
    /// Takes no arguments; returns current accounting plus cumulative activity.
    pub fn stats(&self) -> PcmStats {
        PcmStats {
            live_bytes: self.live.load(Ordering::Acquire),
            entries: self.entries.len(),
            sources: self.sources.len(),
            decoders: self.decoders.len(),
            decoder_scratch_bytes: 0,
            decoder_scratch_limit_bytes: self.budget.decoder_handles * 65536 * 64 * 4,
            stores: self.stores.len(),
            ..self.stats
        }
    }

    /// Release cache, native cursors and descriptors, invalidating receipts.
    /// Takes no arguments; consumer-held PCM remains immutable and charged.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.sources.clear();
        self.decoders.clear();
        self.stores.clear();
        self.stats.cache_bytes = 0;
        self.stats.store_bytes = 0;
        self.owner = Arc::new(());
    }

    /// Capture a new immutable document under fresh cancellation ownership.
    /// `snapshot` replaces the document and `cancel` must be a fresh token.
    /// Returns after invalidating old receipts and releasing native source handles.
    pub fn rebind(
        &mut self,
        snapshot: Arc<EvaluationSnapshot>,
        cancel: Cancellation,
    ) -> Result<()> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.cancel.cancel();
        self.clear();
        if cancel.is_cancelled() {
            return Err(Error::Invalid("PCM rebind requires a fresh token".into()));
        }
        self.snapshot = snapshot;
        self.cancel = cancel;
        Ok(())
    }
    fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
    fn source(&mut self, asset: &editbay_core::AssetReference) -> Result<Arc<SourceFile>> {
        if let Some(source) = self.sources.get(&asset.id) {
            return Ok(source.clone());
        }
        if !asset.path.is_absolute() || self.sources.len() >= self.budget.source_handles {
            return Err(Error::Invalid(
                "PCM source scope or descriptor budget is invalid".into(),
            ));
        }
        let source = Arc::new(SourceFile::open(&asset.path, &self.cancel)?);
        if source.fingerprint().sha256 != asset.sha256 || source.fingerprint().bytes != asset.bytes
        {
            return Err(Error::SourceChanged(asset.path.display().to_string()));
        }
        self.sources.insert(asset.id, source.clone());
        Ok(source)
    }
    fn reserve(&self, bytes: usize) -> Result<Charge> {
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes)
                    .filter(|total| *total <= self.budget.live_bytes)
            })
            .map_err(|_| {
                Error::Invalid("PCM output budget is pinned by consumers; release samples".into())
            })?;
        Ok(Charge {
            bytes,
            live: self.live.clone(),
        })
    }
    fn evict(&mut self) {
        let key = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.used)
            .map(|(key, _)| key.clone())
            .unwrap();
        let entry = self.entries.remove(&key).unwrap();
        self.stats.cache_bytes -= entry.pcm.samples().len() * 4;
        self.stats.evictions += 1;
    }
}

/// Resolve captured sound presentation on its original sample grid.
/// `profile` and `sample_rate` declare exact time. Returns start and exclusive end.
pub(crate) fn presentation(
    profile: &editbay_core::SourceStream,
    sample_rate: u32,
) -> Result<(i64, i64)> {
    let rate = FrameRate::new(sample_rate, 1)?;
    let start = profile
        .time_base
        .boundary(SourcePosition::new(profile.start_tick, 1)?, rate)?;
    let end_tick = profile
        .duration_ticks
        .and_then(|duration| {
            i64::try_from(i128::from(profile.start_tick) + i128::from(duration)).ok()
        })
        .ok_or_else(|| Error::Invalid("PCM requires a finite presentation interval".into()))?;
    let exact_end = profile
        .time_base
        .at_rate(SourcePosition::new(end_tick, 1)?, rate)?;
    let end = i64::try_from(
        -(-i128::from(exact_end.numerator)).div_euclid(i128::from(exact_end.denominator)),
    )
    .map_err(|_| Error::Invalid("PCM presentation end overflow".into()))?;
    Ok((start, end))
}

pub(crate) enum Samples {
    Owned(Vec<f32>),
    Shared(crate::planes::Plane),
}
impl Samples {
    fn as_slice(&self) -> &[f32] {
        match self {
            Self::Owned(samples) => samples,
            Self::Shared(plane) => plane.floats(),
        }
    }
}
enum Output {
    Owned(Vec<f32>),
    Shared(crate::planes::MutablePlane),
}
impl Output {
    fn samples_mut(&mut self) -> &mut [f32] {
        match self {
            Self::Owned(samples) => samples,
            Self::Shared(plane) => plane.floats_mut(),
        }
    }
    fn freeze(self) -> Result<Samples> {
        match self {
            Self::Owned(samples) => Ok(Samples::Owned(samples)),
            Self::Shared(plane) => Ok(Samples::Shared(plane.seal()?)),
        }
    }
}

/// Original-channel PCM routes shared by sound preview and delivery workers.
pub trait PcmProvider {
    /// Inspect the cancellation owner of underlying decode and transport work.
    /// Takes no arguments; returns the token shared by a bound sound renderer.
    fn cancellation(&self) -> &Cancellation;
    /// Inspect a supervised codec child when this route uses one.
    /// Takes no arguments; returns no PID for in-process native decoding.
    fn process_id(&self) -> Option<u32> {
        None
    }
    /// Inspect the immutable source interpretation owner.
    /// Takes no arguments; returns the exact retained evaluation snapshot.
    fn snapshot(&self) -> &Arc<EvaluationSnapshot>;
    /// Read an exact absolute original-rate sample interval.
    /// `source`/`stream` select captured media; `first`/`frames` select samples.
    /// Returns immutable, lifetime-charged PCM with declared edge padding.
    fn interval(&mut self, source: Uuid, stream: u32, first: i64, frames: u32)
    -> Result<PcmResult>;
    /// Validate private receipt ownership and retained source identity.
    /// `result` supplies a previous result; returns an error when stale or foreign.
    fn validate_result(&mut self, result: &PcmResult) -> Result<()>;
    /// Recheck used source identities before transient publication.
    /// Takes no arguments; returns an error after mutation, cancellation or worker death.
    fn check_sources(&mut self) -> Result<()>;
    /// Recheck complete used source checksums before durable publication.
    /// Takes no arguments; returns success only for unchanged source bytes.
    fn verify_sources(&mut self) -> Result<()>;
    /// Inspect native and consumer-held resource accounting.
    /// Takes no arguments; returns current counters without decoding.
    fn stats(&self) -> PcmStats;
    /// Release decoding resources and invalidate all previous receipts.
    /// Takes no arguments; retained consumer buffers remain charged and immutable.
    fn clear(&mut self);
}
impl PcmProvider for NativePcmCache {
    fn cancellation(&self) -> &Cancellation {
        &self.cancel
    }
    fn snapshot(&self) -> &Arc<EvaluationSnapshot> {
        &self.snapshot
    }
    fn interval(
        &mut self,
        source: Uuid,
        stream: u32,
        first: i64,
        frames: u32,
    ) -> Result<PcmResult> {
        NativePcmCache::interval(self, source, stream, first, frames)
    }
    fn validate_result(&mut self, result: &PcmResult) -> Result<()> {
        NativePcmCache::validate_result(self, result)
    }
    fn check_sources(&mut self) -> Result<()> {
        NativePcmCache::check_sources(self)
    }
    fn verify_sources(&mut self) -> Result<()> {
        NativePcmCache::verify_sources(self)
    }
    fn stats(&self) -> PcmStats {
        NativePcmCache::stats(self)
    }
    fn clear(&mut self) {
        NativePcmCache::clear(self)
    }
}
impl<P: PcmProvider + ?Sized> PcmProvider for Box<P> {
    fn cancellation(&self) -> &Cancellation {
        (**self).cancellation()
    }
    fn process_id(&self) -> Option<u32> {
        (**self).process_id()
    }
    fn snapshot(&self) -> &Arc<EvaluationSnapshot> {
        (**self).snapshot()
    }
    fn interval(
        &mut self,
        source: Uuid,
        stream: u32,
        first: i64,
        frames: u32,
    ) -> Result<PcmResult> {
        (**self).interval(source, stream, first, frames)
    }
    fn validate_result(&mut self, result: &PcmResult) -> Result<()> {
        (**self).validate_result(result)
    }
    fn check_sources(&mut self) -> Result<()> {
        (**self).check_sources()
    }
    fn verify_sources(&mut self) -> Result<()> {
        (**self).verify_sources()
    }
    fn stats(&self) -> PcmStats {
        (**self).stats()
    }
    fn clear(&mut self) {
        (**self).clear()
    }
}
