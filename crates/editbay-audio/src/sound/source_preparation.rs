use super::{DOWNSAMPLE_CUTOFF, MAXIMUM_STEP, SINC_LOBES, SoundRenderer};
use editbay_core::{FrameRate, SourcePosition, StreamFormat};
use editbay_media::{Error, PcmPreparation, PcmProvider, Result, pcm_presentation};
use serde::Serialize;
use std::{collections::BTreeSet, sync::Arc};
use uuid::Uuid;

struct Request {
    source: Uuid,
    stream: u32,
    last_sample: i64,
    frames: u64,
    rate: u32,
}

/// One private, renderer-owned source preparation with bounded retained requests.
pub struct SoundPreparation {
    owner: Arc<()>,
    requests: Vec<Request>,
    next: usize,
    completed: u64,
    total: u64,
    current: Option<PcmPreparation>,
    required_bytes: u64,
}

/// Actual decoded source frames across all sources in a captured preparation.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct SoundSourceProgress {
    pub prepared_samples: u64,
    pub total_samples: u64,
    pub completed_sources: usize,
    pub total_sources: usize,
    pub required_bytes: u64,
    pub current: Option<PcmPreparation>,
    pub ready: bool,
}

impl SoundPreparation {
    /// Inspect preparation without reading a file or advancing a decoder.
    /// Takes no arguments; returns source-frame counts and the current interval.
    pub fn progress(&self) -> SoundSourceProgress {
        SoundSourceProgress {
            prepared_samples: self.completed
                + self
                    .current
                    .filter(|current| !current.ready)
                    .map_or(0, |current| current.decoded_frames),
            total_samples: self.total,
            completed_sources: self.next,
            total_sources: self.requests.len(),
            required_bytes: self.required_bytes,
            current: self.current,
            ready: self.next == self.requests.len(),
        }
    }
}

impl<P: PcmProvider> SoundRenderer<P> {
    /// Plan canonical source history for an output interval before playback.
    /// `first` and `frames` select output samples. Returns privately owned source
    /// requests with conservative sinc margins, or fails before exceeding source,
    /// storage-handle or disk budgets. No media decoding occurs in this method.
    pub fn source_preparation(&self, first: u64, frames: u64) -> Result<SoundPreparation> {
        self.check()?;
        let extents = self
            .snapshot
            .source_extents(first, frames, || self.cancel.is_cancelled());
        self.check()?;
        let extents = extents?;
        let budget = self.pcm.budget();
        let mut requests = Vec::new();
        let mut assets = BTreeSet::new();
        let mut total = 0u64;
        let mut required_bytes = 0u64;
        let margin = (SINC_LOBES * MAXIMUM_STEP / DOWNSAMPLE_CUTOFF).ceil() as i64 + 1;
        for extent in extents {
            self.check()?;
            let (asset, profile, _) = self
                .snapshot
                .evaluation()
                .source_stream(extent.source, extent.stream)?;
            let StreamFormat::Audio {
                sample_rate,
                ref channels,
            } = profile.format
            else {
                return Err(Error::Invalid(
                    "source sound preparation selected picture data".into(),
                ));
            };
            let rate = FrameRate::new(sample_rate, 1)?;
            let (start, limit) = pcm_presentation(profile, sample_rate)?;
            let first = profile
                .time_base
                .boundary(SourcePosition::new(extent.first_tick, 1)?, rate)?
                .saturating_sub(margin)
                .max(start);
            let last = profile
                .time_base
                .boundary(SourcePosition::new(extent.last_tick, 1)?, rate)?
                .saturating_add(margin)
                .min(limit);
            if first >= last {
                continue;
            }
            let frames = u64::try_from(i128::from(last) - i128::from(start))
                .map_err(|e| Error::Invalid(e.to_string()))?;
            let bytes = frames
                .checked_mul(channels.len() as u64 * 4)
                .ok_or_else(|| Error::Invalid("source preparation bytes overflow".into()))?;
            required_bytes = required_bytes
                .checked_add(bytes)
                .filter(|&bytes| bytes <= budget.store_bytes)
                .ok_or_else(|| {
                    Error::Invalid("source sound preparation exceeds its disk budget".into())
                })?;
            total = total
                .checked_add(frames)
                .ok_or_else(|| Error::Invalid("source preparation frames overflow".into()))?;
            assets.insert(asset.id);
            if assets.len() > budget.source_handles || requests.len() >= budget.store_handles {
                return Err(Error::Invalid(
                    "source sound preparation exceeds its source or storage handle budget".into(),
                ));
            }
            requests.push(Request {
                source: extent.source,
                stream: extent.stream,
                last_sample: last - 1,
                frames,
                rate: sample_rate,
            });
        }
        Ok(SoundPreparation {
            owner: self.owner.clone(),
            requests,
            next: 0,
            completed: 0,
            total,
            current: None,
            required_bytes,
        })
    }

    /// Advance one source by at most the PCM provider's bounded decode allowance.
    /// `preparation` must belong to this live renderer. Returns actual progress;
    /// call again until ready before starting the device or rendering delivery.
    pub fn prepare_sources_step(
        &mut self,
        preparation: &mut SoundPreparation,
    ) -> Result<SoundSourceProgress> {
        self.check()?;
        if !Arc::ptr_eq(&preparation.owner, &self.owner) {
            return Err(Error::Invalid(
                "foreign or obsolete source sound preparation".into(),
            ));
        }
        let Some(request) = preparation.requests.get(preparation.next) else {
            return Ok(preparation.progress());
        };
        let previous = preparation
            .current
            .filter(|current| !current.ready)
            .map_or(0, |current| current.decoded_frames);
        let current =
            self.pcm
                .prepare_interval(request.source, request.stream, request.last_sample, 1)?;
        if current.source != request.source
            || current.stream != request.stream
            || current.first != request.last_sample
            || current.frames != 1
            || current.sample_rate != request.rate
            || current.required_frames != request.frames
            || current.decoded_frames > request.frames
            || current.decoded_frames < previous
            || (!current.ready && current.decoded_frames == previous)
            || current.ready != (current.decoded_frames == request.frames)
        {
            return Err(Error::Invalid(
                "source sound preparation returned invalid or stalled progress".into(),
            ));
        }
        preparation.current = Some(current);
        if current.ready {
            preparation.completed += request.frames;
            preparation.next += 1;
        }
        Ok(preparation.progress())
    }
}
