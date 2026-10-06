use crate::{Cancellation, Error, Result, SourceFile, VideoReader};
use editbay_core::{
    AssetReference, DocumentVersion, EvaluationSnapshot, SourceRequest, StreamFormat,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use uuid::Uuid;

const MAXIMUM_FORWARD_SKIP: u64 = 8;

/// Explicit budgets for retained RGBA8 outputs and native decoder handles.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PictureBudget {
    pub cache_bytes: usize,
    pub live_bytes: usize,
    pub maximum_picture_bytes: usize,
    pub cache_entries: usize,
    pub source_handles: usize,
    pub decoder_handles: usize,
}

impl Default for PictureBudget {
    fn default() -> Self {
        Self {
            cache_bytes: 256 * 1024 * 1024,
            live_bytes: 384 * 1024 * 1024,
            maximum_picture_bytes: 64 * 1024 * 1024,
            cache_entries: 128,
            source_handles: 16,
            decoder_handles: 4,
        }
    }
}

impl PictureBudget {
    /// Validate caller-selected output and handle budgets.
    /// Takes this configuration. Returns an error for excessive or incompatible
    /// limits; zero cache bytes/entries permits bounded uncached output.
    pub fn validate(self) -> Result<()> {
        if self.cache_bytes > 1024 * 1024 * 1024
            || self.live_bytes > 2 * 1024 * 1024 * 1024
            || self.live_bytes < self.maximum_picture_bytes
            || !(4..=8192 * 8192 * 4).contains(&self.maximum_picture_bytes)
            || self.cache_entries > 4096
            || !(1..=64).contains(&self.source_handles)
            || !(1..=16).contains(&self.decoder_handles)
        {
            return Err(Error::Invalid(
                "picture budgets exceed supported limits".into(),
            ));
        }
        Ok(())
    }
}

/// Actual cache residency, consumer pins and decode activity.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PictureCacheStats {
    pub cache_bytes: usize,
    pub live_bytes: usize,
    pub entries: usize,
    pub sources: usize,
    pub decoders: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub seeks: u64,
    pub sequential_decodes: u64,
    pub forward_decodes: u64,
    pub skipped_pictures: u64,
    pub stored_reads: u64,
}

pub(crate) struct Allocation {
    pub(crate) bytes: usize,
    pub(crate) live: Arc<AtomicUsize>,
}

impl Drop for Allocation {
    fn drop(&mut self) {
        self.live.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Immutable native RGBA8 pixels with lifetime-charged output memory.
pub struct DecodedPicture {
    pub(crate) rgba: Pixels,
    pub width: u32,
    pub height: u32,
    pub source_tick: i64,
    pub color: editbay_core::SourceColor,
    pub alpha: editbay_core::AlphaMode,
    pub alpha_interpretation_required: bool,
    pub rotation_degrees: f64,
    pub(crate) _allocation: Allocation,
    pub(crate) _handle: Option<HandleAllocation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PictureHeader {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) tick: i64,
    pub(crate) color: editbay_core::SourceColor,
    pub(crate) alpha: editbay_core::AlphaMode,
    pub(crate) alpha_interpretation_required: bool,
    pub(crate) rotation_degrees: f64,
}
impl PictureHeader {
    pub(crate) fn of(picture: &DecodedPicture) -> Self {
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

pub(crate) struct HandleAllocation(pub(crate) Arc<AtomicUsize>);
impl Drop for HandleAllocation {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
pub(crate) enum Pixels {
    Owned(Vec<u8>),
    Shared(crate::planes::Plane),
}

enum Output {
    Owned(Vec<u8>),
    Shared(crate::planes::MutablePlane),
}
impl Output {
    fn bytes_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::Shared(plane) => plane.bytes_mut(),
        }
    }
    fn freeze(self) -> Result<Pixels> {
        match self {
            Self::Owned(bytes) => Ok(Pixels::Owned(bytes)),
            Self::Shared(plane) => Ok(Pixels::Shared(plane.seal()?)),
        }
    }
}

impl DecodedPicture {
    /// Inspect shared decoded pixels without copying or permitting mutation.
    /// Takes no arguments. Returns full-range native RGBA8 bytes.
    pub fn rgba(&self) -> &[u8] {
        match &self.rgba {
            Pixels::Owned(bytes) => bytes,
            Pixels::Shared(plane) => plane.bytes(),
        }
    }
}

/// One captured version's request receipt over reusable decoded content.
pub struct PictureResult {
    pub version: DocumentVersion,
    pub generation: u64,
    pub picture: Arc<DecodedPicture>,
    pub cache_hit: bool,
    pub(crate) owner: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Key {
    bytes: u64,
    asset: String,
    interpretation: String,
    ordinal: u64,
}

struct Entry {
    picture: Arc<DecodedPicture>,
    used: u64,
}

pub(crate) struct Selection<'a> {
    pub(crate) reference: &'a AssetReference,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) stream: u32,
    pub(crate) tick: i64,
    pub(crate) ordinal: u64,
    pub(crate) presentation_ticks: &'a [i64],
    pub(crate) size: usize,
    pub(crate) key: Key,
}

pub(crate) fn selection<'a>(
    snapshot: &'a EvaluationSnapshot,
    request: &SourceRequest,
    budget: PictureBudget,
) -> Result<Option<Selection<'a>>> {
    let SourceRequest::Media {
        source,
        stream,
        asset,
        asset_sha256,
        stream_sha256,
        position,
        reverse,
        picture,
        sample,
    } = request
    else {
        return Err(Error::Invalid(
            "picture cache requires a media request".into(),
        ));
    };
    let (reference, profile, interpretation) = snapshot.source_stream(*source, *stream)?;
    if *asset != reference.id
        || asset_sha256 != &reference.sha256
        || stream_sha256 != interpretation
        || sample.is_some()
    {
        return Err(Error::Invalid(
            "picture request does not own the captured interpretation".into(),
        ));
    }
    let StreamFormat::Video {
        width,
        height,
        timing,
        ..
    } = &profile.format
    else {
        return Err(Error::Invalid(
            "picture request selected a sound stream".into(),
        ));
    };
    let selected = if *reverse {
        timing.picture_before(*position, profile.time_base, profile.start_tick)?
    } else {
        timing.picture_at(*position, profile.time_base, profile.start_tick)?
    };
    if selected != *picture {
        return Err(Error::Invalid(
            "picture ordinal does not match source time".into(),
        ));
    }
    let Some(ordinal) = selected else {
        return Ok(None);
    };
    let presentation_ticks = match timing {
        editbay_core::PictureTiming::Variable {
            presentation_ticks, ..
        } => presentation_ticks,
        _ => {
            return Err(Error::Invalid(
                "exact decoded cache requires an actual presentation index".into(),
            ));
        }
    };
    let tick = *presentation_ticks
        .get(ordinal as usize)
        .ok_or_else(|| Error::Invalid("picture ordinal is outside its index".into()))?;
    let size = (*width as usize)
        .checked_mul(*height as usize)
        .and_then(|size| size.checked_mul(4))
        .ok_or_else(|| Error::Invalid("picture geometry overflow".into()))?;
    if size > budget.maximum_picture_bytes || *width > 8192 || *height > 8192 {
        return Err(Error::Invalid(
            "picture exceeds this worker's decode budget".into(),
        ));
    }
    Ok(Some(Selection {
        reference,
        width: *width,
        height: *height,
        stream: *stream,
        tick,
        ordinal,
        presentation_ticks,
        size,
        key: Key {
            bytes: reference.bytes,
            asset: reference.sha256.clone(),
            interpretation: interpretation.into(),
            ordinal,
        },
    }))
}

struct Decoder {
    reader: VideoReader,
    next: Option<u64>,
    used: u64,
}

/// Single-worker native decoder and bounded immutable picture cache.
pub struct PictureCache {
    snapshot: Arc<EvaluationSnapshot>,
    budget: PictureBudget,
    cancel: Cancellation,
    sources: HashMap<Uuid, Arc<SourceFile>>,
    used_sources: HashSet<Uuid>,
    decoders: HashMap<(Uuid, u32), Decoder>,
    entries: HashMap<Key, Entry>,
    bytes: usize,
    live: Arc<AtomicUsize>,
    serial: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
    seeks: u64,
    sequential_decodes: u64,
    forward_decodes: u64,
    skipped_pictures: u64,
    generation: u64,
    worker: Uuid,
    shared: bool,
    store: Option<crate::picture_store::StoreReader>,
    stored_reads: u64,
}

impl PictureCache {
    /// Bind a decoded-picture worker to one immutable document version.
    /// `snapshot` supplies trusted interpretations, `budget` bounds memory/handles,
    /// and `cancel` interrupts all native work. Returns an empty cache; no media IO.
    pub fn new(
        snapshot: Arc<EvaluationSnapshot>,
        budget: PictureBudget,
        cancel: Cancellation,
    ) -> Result<Self> {
        budget.validate()?;
        Ok(Self {
            snapshot,
            budget,
            cancel,
            sources: HashMap::new(),
            used_sources: HashSet::new(),
            decoders: HashMap::new(),
            entries: HashMap::new(),
            bytes: 0,
            live: Arc::new(AtomicUsize::new(0)),
            serial: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
            seeks: 0,
            sequential_decodes: 0,
            forward_decodes: 0,
            skipped_pictures: 0,
            generation: 0,
            worker: Uuid::new_v4(),
            shared: false,
            store: None,
            stored_reads: 0,
        })
    }

    pub(crate) fn new_shared(
        snapshot: Arc<EvaluationSnapshot>,
        budget: PictureBudget,
        cancel: Cancellation,
    ) -> Result<Self> {
        let mut cache = Self::new(snapshot, budget, cancel)?;
        cache.shared = true;
        Ok(cache)
    }

    pub(crate) fn attach_store(
        &mut self,
        file: std::fs::File,
        manifest: crate::picture_store::Manifest,
    ) -> Result<()> {
        self.store = Some(crate::picture_store::StoreReader::new(
            file,
            manifest,
            &self.snapshot,
            self.budget,
            &self.cancel,
        )?);
        Ok(())
    }

    /// Serve an exact typed picture request on the owning worker.
    /// `request` must match captured asset/stream interpretation and source-time
    /// selection. Returns no picture outside its interval, or shared immutable
    /// pixels; cached hits still check cancellation and source pathname ownership.
    pub fn picture(&mut self, request: &SourceRequest) -> Result<Option<PictureResult>> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let snapshot = self.snapshot.clone();
        let Some(selected) = selection(&snapshot, request, self.budget)? else {
            return Ok(None);
        };
        let Selection {
            reference,
            width,
            height,
            stream,
            tick,
            ordinal,
            presentation_ticks,
            size,
            key,
        } = selected;
        let (width, height, stream) = (&width, &height, &stream);
        let owned = self.source(reference)?;
        owned.check_current(&self.cancel)?;
        self.serial = self.serial.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(&key) {
            self.hits = self.hits.saturating_add(1);
            entry.used = self.serial;
            return Ok(Some(PictureResult {
                version: DocumentVersion::of(snapshot.project()),
                generation: self.generation,
                picture: entry.picture.clone(),
                cache_hit: true,
                owner: self.worker,
            }));
        }
        self.misses = self.misses.saturating_add(1);
        while self.live.load(Ordering::Acquire).saturating_add(size) > self.budget.live_bytes
            && self.evict()
        {}
        let allocation = self.reserve(size)?;
        let mut output = if self.shared {
            Output::Shared(crate::planes::MutablePlane::new(size)?)
        } else {
            Output::Owned(vec![0; size])
        };
        let stored = self
            .store
            .as_ref()
            .map(|store| store.read(&key, output.bytes_mut(), &self.cancel))
            .transpose()?
            .flatten();
        let header = if let Some(header) = stored {
            self.stored_reads = self.stored_reads.saturating_add(1);
            header
        } else {
            let decoder_key = (reference.id, *stream);
            if !self.decoders.contains_key(&decoder_key) {
                if self.decoders.len() >= self.budget.decoder_handles {
                    let oldest = self
                        .decoders
                        .iter()
                        .min_by_key(|(_, decoder)| decoder.used)
                        .map(|(key, _)| *key)
                        .ok_or_else(|| {
                            Error::Invalid("decoder budget cannot evict a handle".into())
                        })?;
                    self.decoders.remove(&oldest);
                }
                let reader = VideoReader::open_stream(&owned, *stream, self.cancel.clone())?;
                if reader.info.width != *width as i32 || reader.info.height != *height as i32 {
                    return Err(Error::Invalid(
                        "native picture geometry differs from the captured profile".into(),
                    ));
                }
                self.decoders.insert(
                    decoder_key,
                    Decoder {
                        reader,
                        next: Some(0),
                        used: self.serial,
                    },
                );
            }
            let decoder = self
                .decoders
                .get_mut(&decoder_key)
                .ok_or_else(|| Error::Invalid("owned decoder is absent".into()))?;
            decoder.used = self.serial;
            if decoder.reader.info.width != *width as i32
                || decoder.reader.info.height != *height as i32
            {
                return Err(Error::Invalid(
                    "native picture geometry differs from the captured profile".into(),
                ));
            }
            let decoded = if decoder.next == Some(ordinal) {
                self.sequential_decodes = self.sequential_decodes.saturating_add(1);
                decoder.reader.read_picture(output.bytes_mut(), None)
            } else if let Some(next) = decoder
                .next
                .filter(|next| *next < ordinal && ordinal - *next <= MAXIMUM_FORWARD_SKIP)
            {
                self.forward_decodes = self.forward_decodes.saturating_add(1);
                (|| {
                    for tick in &presentation_ticks[next as usize..ordinal as usize] {
                        decoder.reader.skip_picture(*tick)?;
                        self.skipped_pictures = self.skipped_pictures.saturating_add(1);
                    }
                    decoder.reader.read_picture(output.bytes_mut(), None)
                })()
            } else {
                self.seeks = self.seeks.saturating_add(1);
                decoder.reader.read_picture(output.bytes_mut(), Some(tick))
            }
            .and_then(|frame| {
                frame.ok_or_else(|| Error::Invalid("indexed picture is unavailable at EOF".into()))
            });
            let decoded = match decoded {
                Ok(decoded) => decoded,
                Err(error) => {
                    self.decoders.remove(&decoder_key);
                    return Err(error);
                }
            };
            if decoded.source_tick != Some(tick) {
                self.decoders.remove(&decoder_key);
                return Err(Error::Invalid(
                    "native picture differs from the exact captured index".into(),
                ));
            }
            decoder.next = ordinal.checked_add(1);
            PictureHeader {
                width: *width,
                height: *height,
                tick,
                color: decoded.color,
                alpha: decoded.alpha,
                alpha_interpretation_required: decoded.alpha_interpretation_required,
                rotation_degrees: decoded.rotation_degrees,
            }
        };
        owned.check_current(&self.cancel)?;
        let picture = Arc::new(DecodedPicture {
            rgba: output.freeze()?,
            width: *width,
            height: *height,
            source_tick: tick,
            color: header.color,
            alpha: header.alpha,
            alpha_interpretation_required: header.alpha_interpretation_required,
            rotation_degrees: header.rotation_degrees,
            _allocation: allocation,
            _handle: None,
        });
        if size <= self.budget.cache_bytes && self.budget.cache_entries > 0 {
            while (self.bytes.saturating_add(size) > self.budget.cache_bytes
                || self.entries.len() >= self.budget.cache_entries)
                && self.evict()
            {}
            self.bytes += size;
            self.entries.insert(
                key,
                Entry {
                    picture: picture.clone(),
                    used: self.serial,
                },
            );
        }
        Ok(Some(PictureResult {
            version: DocumentVersion::of(snapshot.project()),
            generation: self.generation,
            picture,
            cache_hit: false,
            owner: self.worker,
        }))
    }

    /// Inspect retained outputs, externally pinned bytes and actual decode work.
    /// Takes no arguments. Returns current counters without native IO.
    pub fn stats(&self) -> PictureCacheStats {
        PictureCacheStats {
            cache_bytes: self.bytes,
            live_bytes: self.live.load(Ordering::Acquire),
            entries: self.entries.len(),
            sources: self.sources.len(),
            decoders: self.decoders.len(),
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
            seeks: self.seeks,
            sequential_decodes: self.sequential_decodes,
            forward_decodes: self.forward_decodes,
            skipped_pictures: self.skipped_pictures,
            stored_reads: self.stored_reads,
        }
    }

    /// Recheck every source used in this generation before job publication.
    /// Takes no arguments. Returns success after complete-byte verification;
    /// document/session ownership must still be checked by the receiving caller.
    pub fn verify_sources(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        for id in &self.used_sources {
            self.sources[id].verify(&self.cancel)?;
        }
        Ok(())
    }

    /// Reject a picture receipt from a cleared or different worker generation.
    /// `result` supplies captured document and cache ownership. Returns success
    /// only for this generation; source verification and receiver ownership remain required.
    pub fn validate_result(&self, result: &PictureResult) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if result.version != DocumentVersion::of(self.snapshot.project())
            || result.generation != self.generation
            || result.owner != self.worker
        {
            return Err(Error::Invalid(
                "picture receipt belongs to an obsolete worker generation".into(),
            ));
        }
        Ok(())
    }

    /// Rebind retained raw pixels to a newly captured immutable document.
    /// `snapshot` supplies the next version and `cancel` is a fresh job token.
    /// Returns after invalidating previous receipts/decoders and pruning changed
    /// file bindings; matching pixels still require the next source preflight.
    pub fn rebind(
        &mut self,
        snapshot: Arc<EvaluationSnapshot>,
        cancel: Cancellation,
    ) -> Result<()> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("picture generation counter exhausted".into()))?;
        self.cancel.cancel();
        if cancel.is_cancelled() {
            return Err(Error::Invalid(
                "rebind requires a fresh cancellation token".into(),
            ));
        }
        self.decoders.clear();
        self.store = None;
        let assets: HashMap<_, _> = snapshot
            .project()
            .assets
            .iter()
            .map(|asset| (asset.id, asset))
            .collect();
        self.sources.retain(|id, source| {
            assets.get(id).is_some_and(|asset| {
                asset.path == source.path()
                    && asset.sha256 == source.fingerprint().sha256
                    && asset.bytes == source.fingerprint().bytes
                    && source.check_current(&cancel).is_ok()
            })
        });
        self.used_sources.clear();
        self.cancel = cancel;
        self.snapshot = snapshot;
        self.generation = generation;
        self.worker = Uuid::new_v4();
        Ok(())
    }

    /// Release cache-owned pixels, native decoders and source descriptors.
    /// Takes no arguments. Returns after cleanup and generation invalidation;
    /// external pixels remain valid and charged but their receipts are obsolete.
    pub fn clear(&mut self) -> Result<()> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("picture generation counter exhausted".into()))?;
        self.entries.clear();
        self.bytes = 0;
        self.decoders.clear();
        self.sources.clear();
        self.store = None;
        self.used_sources.clear();
        self.generation = generation;
        self.worker = Uuid::new_v4();
        Ok(())
    }

    /// Open one captured asset or reuse its already verified descriptor.
    /// `reference` supplies expected path/hash/size. Returns a source handle within
    /// the declared per-job cap; final verification retains every used source.
    fn source(&mut self, reference: &AssetReference) -> Result<Arc<SourceFile>> {
        if !reference.path.is_absolute() {
            return Err(Error::Invalid("resolve project-relative assets within the granted source scope before cache binding".into()));
        }
        if let Some(source) = self.sources.get(&reference.id) {
            self.used_sources.insert(reference.id);
            return Ok(source.clone());
        }
        if self.sources.len() >= self.budget.source_handles {
            return Err(Error::Invalid("source descriptor budget exhausted".into()));
        }
        let source = Arc::new(SourceFile::open(&reference.path, &self.cancel)?);
        if source.fingerprint().sha256 != reference.sha256
            || source.fingerprint().bytes != reference.bytes
        {
            return Err(Error::SourceChanged(reference.path.display().to_string()));
        }
        self.sources.insert(reference.id, source.clone());
        self.used_sources.insert(reference.id);
        Ok(source)
    }

    /// Charge one decoded output before allocating its pixels.
    /// `bytes` is exact RGBA geometry. Returns a lifetime permit or explicit pin
    /// exhaustion; consumers cannot hide memory by retaining evicted frames.
    fn reserve(&self, bytes: usize) -> Result<Allocation> {
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes)
                    .filter(|total| *total <= self.budget.live_bytes)
            })
            .map_err(|_| {
                Error::Invalid(
                    "decoded picture budget is pinned by consumers; release frames".into(),
                )
            })?;
        Ok(Allocation {
            bytes,
            live: self.live.clone(),
        })
    }

    /// Evict the least recently requested cache-owned picture.
    /// Takes no arguments. Returns whether an entry was removed; external owners
    /// keep their allocation charge and immutable bytes.
    fn evict(&mut self) -> bool {
        let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.used)
            .map(|(key, _)| key.clone())
        else {
            return false;
        };
        if let Some(entry) = self.entries.remove(&key) {
            self.bytes -= entry.picture.rgba().len();
            self.evictions = self.evictions.saturating_add(1);
        }
        true
    }
}

/// Interchangeable source-owned picture routes used by the same GPU graph.
pub trait PictureProvider {
    /// Check supervised process liveness without decoding a picture.
    /// Takes this provider. Returns success for an in-process provider or a live
    /// codec child, and a visible failure after an isolated process exits.
    fn check_health(&mut self) -> Result<()> {
        Ok(())
    }
    /// Inspect captured document ownership without media IO.
    /// Takes no arguments. Returns the version served by this provider.
    fn version(&self) -> DocumentVersion;
    /// Decode or share one exact, validated source request.
    /// `request` supplies temporal selection. Returns immutable pixels or no picture.
    fn picture(&mut self, request: &SourceRequest) -> Result<Option<PictureResult>>;
    /// Validate a receipt before upload or publication.
    /// `result` supplies private provider ownership. Returns an error when stale.
    fn validate_result(&self, result: &PictureResult) -> Result<()>;
    /// Verify complete bytes of all used sources before final publication.
    /// Takes no arguments. Returns success only for unchanged owned sources.
    fn verify_sources(&mut self) -> Result<()>;
    /// Inspect cache, live-payload and decoder activity without IO.
    /// Takes no arguments. Returns current counters.
    fn stats(&self) -> PictureCacheStats;
    /// Release owned resources and invalidate old receipts.
    /// Takes no arguments. Returns after cleanup; consumer pins remain charged.
    fn clear(&mut self) -> Result<()>;
    /// Adopt a captured document under a fresh cancellation token.
    /// `snapshot` and `cancel` supply new ownership. Returns after stale work stops.
    fn rebind(&mut self, snapshot: Arc<EvaluationSnapshot>, cancel: Cancellation) -> Result<()>;
}

impl PictureProvider for PictureCache {
    fn version(&self) -> DocumentVersion {
        DocumentVersion::of(self.snapshot.project())
    }
    fn picture(&mut self, request: &SourceRequest) -> Result<Option<PictureResult>> {
        PictureCache::picture(self, request)
    }
    fn validate_result(&self, result: &PictureResult) -> Result<()> {
        PictureCache::validate_result(self, result)
    }
    fn verify_sources(&mut self) -> Result<()> {
        PictureCache::verify_sources(self)
    }
    fn stats(&self) -> PictureCacheStats {
        PictureCache::stats(self)
    }
    fn clear(&mut self) -> Result<()> {
        PictureCache::clear(self)
    }
    fn rebind(&mut self, snapshot: Arc<EvaluationSnapshot>, cancel: Cancellation) -> Result<()> {
        PictureCache::rebind(self, snapshot, cancel)
    }
}

impl<P: PictureProvider + ?Sized> PictureProvider for Box<P> {
    fn check_health(&mut self) -> Result<()> {
        (**self).check_health()
    }
    fn version(&self) -> DocumentVersion {
        (**self).version()
    }
    fn picture(&mut self, request: &SourceRequest) -> Result<Option<PictureResult>> {
        (**self).picture(request)
    }
    fn validate_result(&self, result: &PictureResult) -> Result<()> {
        (**self).validate_result(result)
    }
    fn verify_sources(&mut self) -> Result<()> {
        (**self).verify_sources()
    }
    fn stats(&self) -> PictureCacheStats {
        (**self).stats()
    }
    fn clear(&mut self) -> Result<()> {
        (**self).clear()
    }
    fn rebind(&mut self, snapshot: Arc<EvaluationSnapshot>, cancel: Cancellation) -> Result<()> {
        (**self).rebind(snapshot, cancel)
    }
}
