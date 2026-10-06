use super::{PcmBudget, PcmPreparation, presentation};
use editbay_core::{DocumentVersion, Project, StreamFormat};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// The captured interval and actual progress of one supervised PCM request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PcmProgress {
    pub version: DocumentVersion,
    pub request: u64,
    pub preparation: PcmPreparation,
}

impl PcmProgress {
    /// Validate source ownership, fixed work limits and monotonic request progress.
    /// `previous` supplies the last record; `project` and `budget` bind permitted
    /// sources and resources. Returns false for malformed, foreign or stale work.
    pub fn valid_after(self, previous: Option<Self>, project: &Project, budget: PcmBudget) -> bool {
        if self.version != DocumentVersion::of(project)
            || previous.is_some_and(|prior| prior.version != self.version)
        {
            return false;
        }
        let progress = self.preparation;
        let Some(profile) = project
            .sources
            .iter()
            .find(|source| source.id == progress.source)
            .and_then(|source| {
                source
                    .streams
                    .iter()
                    .find(|stream| stream.index == progress.stream)
            })
        else {
            return false;
        };
        let StreamFormat::Audio {
            sample_rate,
            channels,
        } = &profile.format
        else {
            return false;
        };
        let Ok((start, limit)) = presentation(profile, *sample_rate) else {
            return false;
        };
        let Some(end) = progress.first.checked_add(i64::from(progress.frames)) else {
            return false;
        };
        let wanted_start = progress.first.max(start);
        let wanted_end = end.min(limit);
        let required = if wanted_start < wanted_end {
            (i128::from(wanted_end) - i128::from(start)) as u64
        } else {
            0
        };
        if self.request == 0
            || progress.frames == 0
            || progress.frames > budget.interval_frames
            || progress.sample_rate != *sample_rate
            || u64::from(progress.frames) * channels.len() as u64 * 4 > budget.live_bytes as u64
            || progress.required_frames != required
            || required
                .checked_mul(channels.len() as u64 * 4)
                .is_none_or(|bytes| bytes > budget.store_bytes)
            || progress.decoded_frames > required
            || progress.ready != (progress.decoded_frames == required)
        {
            return false;
        }
        previous.is_none_or(|prior| {
            self.request > prior.request
                || (self.request == prior.request
                    && progress.source == prior.preparation.source
                    && progress.stream == prior.preparation.stream
                    && progress.first == prior.preparation.first
                    && progress.frames == prior.preparation.frames
                    && progress.sample_rate == prior.preparation.sample_rate
                    && required == prior.preparation.required_frames
                    && progress.decoded_frames >= prior.preparation.decoded_frames
                    && (!prior.preparation.ready || progress.ready))
        })
    }
}

/// One bounded progress record shared by control and preparation threads.
#[derive(Clone, Default)]
pub struct PcmPreparationLog(Arc<Mutex<Option<PcmProgress>>>);

impl PcmPreparationLog {
    /// Observe preparation without waiting for the producer.
    /// Takes no arguments. Returns the latest coherent record, or none while busy.
    /// This control-plane observer is never read or written by a device callback.
    pub fn observation(&self) -> Option<PcmProgress> {
        self.0.try_lock().ok().and_then(|value| *value)
    }

    pub(crate) fn publish(&self, progress: PcmProgress) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(progress);
    }
}
