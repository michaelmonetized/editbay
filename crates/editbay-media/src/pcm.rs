use crate::{AudioBlock, Cancellation, Error, NativeAudioReader, Result, SourceFile};
use editbay_core::{
    DocumentVersion, EvaluationSnapshot, FrameRate, SourcePosition, StreamFormat, TimeBase,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use uuid::Uuid;

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
        {
            return Err(Error::Invalid("PCM budgets exceed supported limits".into()));
        }
        Ok(())
    }
}

/// Actual retained PCM, decoder scratch and source activity.
#[derive(Debug, Clone, Copy, Serialize, Default)]
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
}
struct Charge {
    bytes: usize,
    live: Arc<AtomicUsize>,
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.live.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Immutable PCM whose memory remains charged while any consumer retains it.
pub struct PcmBlock {
    first_sample: i64,
    channels: usize,
    samples: Vec<f32>,
    _charge: Charge,
}
impl PcmBlock {
    /// Inspect absolute original-rate sample boundaries and channel count.
    /// Takes no arguments; returns first sample, frame count and channel count.
    pub fn interval(&self) -> (i64, usize, usize) {
        (
            self.first_sample,
            self.samples.len() / self.channels,
            self.channels,
        )
    }
    /// Inspect unchanged-channel interleaved float samples.
    /// Takes no arguments; returns immutable PCM, retaining finite headroom.
    pub fn samples(&self) -> &[f32] {
        &self.samples
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
    pcm: Arc<PcmBlock>,
    version: DocumentVersion,
    owner: Arc<()>,
    asset: Uuid,
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
    pending: Option<AudioBlock>,
    eof: bool,
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
        })
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
        let mut output = vec![0.; frames as usize * channels.len()];
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
        let limit = i64::try_from(
            -(-i128::from(exact_end.numerator)).div_euclid(i128::from(exact_end.denominator)),
        )
        .map_err(|_| Error::Invalid("PCM presentation end overflow".into()))?;
        let wanted_start = first.max(start);
        let wanted_end = end.min(limit);
        if wanted_start < wanted_end {
            let decoder_key = (source, stream);
            if !self.decoders.contains_key(&decoder_key) {
                if self.decoders.len() >= self.budget.decoder_handles {
                    let oldest = self
                        .decoders
                        .iter()
                        .min_by_key(|(_, d)| d.used)
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
                    decoder_key,
                    Decoder {
                        reader,
                        pending: None,
                        eof: false,
                        used: self.serial,
                    },
                );
            }
            let decoder = self.decoders.get_mut(&decoder_key).unwrap();
            decoder.used = self.serial;
            let cursor = decoder
                .pending
                .as_ref()
                .and_then(|b| b.first_sample)
                .unwrap_or(start);
            let pending_end = decoder
                .pending
                .as_ref()
                .map(|b| cursor + (b.samples.len() / channels.len()) as i64)
                .unwrap_or(cursor);
            if wanted_start < cursor
                || wanted_start.saturating_sub(pending_end)
                    > i64::from(self.budget.decode_frames / 2)
                || decoder.eof
            {
                let lead = i64::from(sample_rate).saturating_mul(2);
                let seek_sample = wanted_start.saturating_sub(lead).max(start);
                let tick = TimeBase {
                    numerator: 1,
                    denominator: sample_rate,
                }
                .boundary(
                    SourcePosition::new(seek_sample, 1)?,
                    FrameRate::new(
                        decoder.reader.time_base.denominator,
                        decoder.reader.time_base.numerator,
                    )?,
                )?;
                if seek_sample == start {
                    decoder.reader =
                        NativeAudioReader::open_stream(&owned, stream, self.cancel.clone())?;
                } else {
                    decoder.reader.seek(tick)?;
                }
                decoder.pending = None;
                decoder.eof = false;
                self.stats.seeks += 1;
            }
            let mut filled = wanted_start;
            let mut decoded = 0u64;
            let mut packets = 0u32;
            while filled < wanted_end {
                if self.cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                if let Some(block) = &decoder.pending {
                    let block_start = block.first_sample.ok_or_else(|| {
                        Error::Invalid("native PCM has no absolute sample clock".into())
                    })?;
                    let block_end = block_start
                        .checked_add((block.samples.len() / channels.len()) as i64)
                        .ok_or_else(|| Error::Invalid("native PCM clock overflow".into()))?;
                    if block_start > filled {
                        return Err(Error::Invalid(
                            "native PCM has an in-range presentation gap".into(),
                        ));
                    }
                    if block_end > filled {
                        let copy_end = block_end.min(wanted_end);
                        let from = (filled - block_start) as usize * channels.len();
                        let to = (filled - first) as usize * channels.len();
                        let count = (copy_end - filled) as usize * channels.len();
                        output[to..to + count].copy_from_slice(&block.samples[from..from + count]);
                        filled = copy_end;
                        if filled == wanted_end {
                            break;
                        }
                    }
                }
                decoder.pending = None;
                if decoder.eof {
                    return Err(Error::Invalid(
                        "native PCM ended before its captured presentation end".into(),
                    ));
                }
                if decoded + 65536 > u64::from(self.budget.decode_frames) || packets >= 4096 {
                    return Err(Error::Invalid(
                        "native PCM decode work budget exhausted".into(),
                    ));
                }
                decoder.pending = decoder.reader.next_block()?;
                packets += 1;
                match &decoder.pending {
                    Some(block) => {
                        let count = (block.samples.len() / channels.len()) as u64;
                        decoded += count;
                        self.stats.decoded_frames += count;
                    }
                    None => decoder.eof = true,
                }
            }
        }
        owned.check_current(&self.cancel)?;
        let pcm = Arc::new(PcmBlock {
            first_sample: first,
            channels: channels.len(),
            samples: output,
            _charge: charge,
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
            decoder_scratch_bytes: self
                .decoders
                .values()
                .filter_map(|d| d.pending.as_ref())
                .map(|b| b.samples.capacity() * 4)
                .sum(),
            decoder_scratch_limit_bytes: self.budget.decoder_handles * 65536 * 64 * 4,
            ..self.stats
        }
    }

    /// Release cache, native cursors and descriptors, invalidating receipts.
    /// Takes no arguments; consumer-held PCM remains immutable and charged.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.sources.clear();
        self.decoders.clear();
        self.stats.cache_bytes = 0;
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
        self.stats.cache_bytes -= entry.pcm.samples.len() * 4;
        self.stats.evictions += 1;
    }
}
