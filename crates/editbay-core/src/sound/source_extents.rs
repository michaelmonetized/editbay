use super::{
    SoundSnapshot, Step,
    index::{Builder, Span},
    invalid, sample_center,
};
use crate::{Result, SourcePosition};
use serde::Serialize;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Conservative original source ticks reached by a captured output interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SoundSourceExtent {
    pub source: Uuid,
    pub stream: u32,
    pub first_tick: i64,
    pub last_tick: i64,
}

impl SoundSnapshot {
    /// Bound every source contribution before device playback or file delivery.
    /// `first` and `frames` select output samples; `cancelled` interrupts planning.
    /// Returns deduplicated inclusive source-tick bounds, enclosing fractional,
    /// nested and reverse paths without enumerating the output's sample duration.
    pub fn source_extents(
        &self,
        first: u64,
        frames: u64,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Vec<SoundSourceExtent>> {
        let end = first
            .checked_add(frames)
            .filter(|&end| frames > 0 && end <= self.duration_samples)
            .ok_or_else(|| invalid("source preparation interval is outside sequence sound"))?;
        let root = &self.snapshot.project().compositions[self.root];
        let lower = sample_center(first, self.profile.sample_rate, root.frame_rate)?;
        let upper = sample_center(end - 1, self.profile.sample_rate, root.frame_rate)?;
        let floor = |value: SourcePosition| {
            i128::from(value.numerator).div_euclid(i128::from(value.denominator))
        };
        let initial = Span {
            start: i64::try_from(floor(lower))
                .map_err(|_| invalid("source preparation start overflow"))?,
            end: i64::try_from(
                floor(upper)
                    + i128::from(
                        i128::from(upper.numerator).rem_euclid(i128::from(upper.denominator)) != 0,
                    ),
            )
            .map_err(|_| invalid("source preparation end overflow"))?,
        };
        let project = self.snapshot.project();
        let mut builder = Builder::new(self.budget.intervals);
        let mut bounds: BTreeMap<(Uuid, u32), SoundSourceExtent> = BTreeMap::new();
        for leaf in &self.leaves {
            let mut spans = vec![initial];
            for step in &leaf.steps {
                if cancelled() {
                    return Err(invalid("sound source planning cancelled"));
                }
                spans = match *step {
                    Step::Node(scene, node) => builder
                        .intersect(Some(spans), project.compositions[scene].nodes[node].range)?,
                    Step::Clip(scene, track, clip) => {
                        let track = &project.compositions[scene].tracks[track];
                        if track.enabled {
                            builder.image(spans, &track.clips[clip], &mut cancelled)?
                        } else {
                            Vec::new()
                        }
                    }
                };
                if spans.is_empty() {
                    break;
                }
            }
            for span in spans {
                let entry = bounds
                    .entry((leaf.source, leaf.stream))
                    .or_insert(SoundSourceExtent {
                        source: leaf.source,
                        stream: leaf.stream,
                        first_tick: span.start,
                        last_tick: span.end,
                    });
                entry.first_tick = entry.first_tick.min(span.start);
                entry.last_tick = entry.last_tick.max(span.end);
            }
        }
        if cancelled() {
            return Err(invalid("sound source planning cancelled"));
        }
        Ok(bounds.into_values().collect())
    }
}
