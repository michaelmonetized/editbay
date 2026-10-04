use crate::{Cancellation, Error, Result, SourceFile, VideoReader};
use editbay_core::{
    AssetReference, DocumentVersion, EvaluationSnapshot, SourceRequest, StreamFormat,
};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use uuid::Uuid;

/// Explicit budgets for retained RGBA8 outputs and native decoder handles.
#[derive(Debug, Clone, Copy, Serialize)]
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
#[derive(Debug, Clone, Copy, Serialize)]
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
}

struct Allocation {
    bytes: usize,
    live: Arc<AtomicUsize>,
}

impl Drop for Allocation {
    fn drop(&mut self) {
        self.live.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Immutable native RGBA8 pixels with lifetime-charged output memory.
pub struct DecodedPicture {
    rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub source_tick: i64,
    _allocation: Allocation,
}

impl DecodedPicture {
    /// Inspect shared decoded pixels without copying or permitting mutation.
    /// Takes no arguments. Returns full-range native RGBA8 bytes.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

/// One captured version's request receipt over reusable decoded content.
pub struct PictureResult {
    pub version: DocumentVersion,
    pub generation: u64,
    pub picture: Arc<DecodedPicture>,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    bytes: u64,
    asset: String,
    interpretation: String,
    ordinal: u64,
}

struct Entry {
    picture: Arc<DecodedPicture>,
    used: u64,
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
    generation: u64,
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
            generation: 0,
        })
    }

    /// Serve an exact typed picture request on the owning worker.
    /// `request` must match captured asset/stream interpretation and source-time
    /// selection. Returns no picture outside its interval, or shared immutable
    /// pixels; cached hits still check cancellation and source pathname ownership.
    pub fn picture(&mut self, request: &SourceRequest) -> Result<Option<PictureResult>> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
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
        let snapshot = self.snapshot.clone();
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
        let tick = match timing {
            editbay_core::PictureTiming::Variable {
                presentation_ticks, ..
            } => *presentation_ticks
                .get(ordinal as usize)
                .ok_or_else(|| Error::Invalid("picture ordinal is outside its index".into()))?,
            _ => {
                return Err(Error::Invalid(
                    "exact decoded cache requires an actual presentation index".into(),
                ));
            }
        };
        let size = (*width as usize)
            .checked_mul(*height as usize)
            .and_then(|size| size.checked_mul(4))
            .ok_or_else(|| Error::Invalid("picture geometry overflow".into()))?;
        if size > self.budget.maximum_picture_bytes || *width > 8192 || *height > 8192 {
            return Err(Error::Invalid(
                "picture exceeds this worker's decode budget".into(),
            ));
        }
        let owned = self.source(reference)?;
        owned.check_current(&self.cancel)?;
        self.serial = self.serial.saturating_add(1);
        let key = Key {
            bytes: reference.bytes,
            asset: reference.sha256.clone(),
            interpretation: interpretation.into(),
            ordinal,
        };
        if let Some(entry) = self.entries.get_mut(&key) {
            self.hits = self.hits.saturating_add(1);
            entry.used = self.serial;
            return Ok(Some(PictureResult {
                version: DocumentVersion::of(snapshot.project()),
                generation: self.generation,
                picture: entry.picture.clone(),
                cache_hit: true,
            }));
        }
        self.misses = self.misses.saturating_add(1);
        while self.live.load(Ordering::Acquire).saturating_add(size) > self.budget.live_bytes
            && self.evict()
        {}
        let allocation = self.reserve(size)?;
        let decoder_key = (*asset, *stream);
        if !self.decoders.contains_key(&decoder_key) {
            if self.decoders.len() >= self.budget.decoder_handles {
                let oldest = self
                    .decoders
                    .iter()
                    .min_by_key(|(_, decoder)| decoder.used)
                    .map(|(key, _)| *key)
                    .ok_or_else(|| Error::Invalid("decoder budget cannot evict a handle".into()))?;
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
            decoder.reader.next_frame().and_then(|frame| {
                frame.ok_or_else(|| Error::Invalid("indexed picture is unavailable at EOF".into()))
            })
        } else {
            self.seeks = self.seeks.saturating_add(1);
            decoder.reader.frame_at(tick)
        };
        let decoded = match decoded {
            Ok(decoded) => decoded,
            Err(error) => {
                self.decoders.remove(&decoder_key);
                return Err(error);
            }
        };
        if decoded.source_tick != Some(tick) || decoded.rgba.len() != size {
            self.decoders.remove(&decoder_key);
            return Err(Error::Invalid(
                "native picture differs from the exact captured index".into(),
            ));
        }
        decoder.next = ordinal.checked_add(1);
        owned.check_current(&self.cancel)?;
        let picture = Arc::new(DecodedPicture {
            rgba: decoded.rgba,
            width: *width,
            height: *height,
            source_tick: tick,
            _allocation: allocation,
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
        self.used_sources.clear();
        self.generation = generation;
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
            self.bytes -= entry.picture.rgba.len();
            self.evictions = self.evictions.saturating_add(1);
        }
        true
    }
}
