use serde::{Deserialize, Serialize};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

/// The producer's current work, observed independently of completed blocks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationStage {
    #[default]
    Idle,
    Plan,
    Render,
    Finish,
}

/// Measured producer blocks and the stages of the most expensive completed attempt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationStats {
    pub blocks: u64,
    pub published_blocks: u64,
    pub last_plan_ns: u64,
    pub last_render_ns: u64,
    pub last_finish_ns: u64,
    pub last_block_ns: u64,
    pub max_block_ns: u64,
    pub max_interval_ns: u64,
    pub slowest_plan_ns: u64,
    pub slowest_render_ns: u64,
    pub slowest_finish_ns: u64,
    pub active_stage: PreparationStage,
    pub active_block_ns: u64,
    pub active_stage_ns: u64,
}

impl PreparationStats {
    pub(crate) fn valid_after(self, previous: Self) -> bool {
        self.published_blocks <= self.blocks
            && self.blocks >= previous.blocks
            && self.published_blocks >= previous.published_blocks
            && self.max_block_ns >= previous.max_block_ns
            && self.max_interval_ns >= previous.max_interval_ns
            && self.last_block_ns <= self.max_block_ns
            && self.active_stage_ns <= self.active_block_ns
            && (self.active_stage != PreparationStage::Idle || self.active_block_ns == 0)
            && self
                .last_plan_ns
                .checked_add(self.last_render_ns)
                .and_then(|n| n.checked_add(self.last_finish_ns))
                .is_some_and(|n| n <= self.last_block_ns)
            && self
                .slowest_plan_ns
                .checked_add(self.slowest_render_ns)
                .and_then(|n| n.checked_add(self.slowest_finish_ns))
                .is_some_and(|n| n <= self.max_block_ns)
    }
}

pub(crate) struct PreparationLog {
    origin: Instant,
    generation: AtomicU64,
    fields: [AtomicU64; 14],
}

impl Default for PreparationLog {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            generation: AtomicU64::new(0),
            fields: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl PreparationLog {
    /// Publish one completed producer attempt without involving a device callback.
    /// `stats` contains measured wall-clock stages; returns after a fixed atomic copy.
    pub fn publish(&self, stats: PreparationStats) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        for (field, value) in self.fields.iter().zip([
            stats.blocks,
            stats.published_blocks,
            stats.last_plan_ns,
            stats.last_render_ns,
            stats.last_finish_ns,
            stats.last_block_ns,
            stats.max_block_ns,
            stats.max_interval_ns,
            stats.slowest_plan_ns,
            stats.slowest_render_ns,
            stats.slowest_finish_ns,
            0,
            0,
            0,
        ]) {
            field.store(value, Ordering::SeqCst);
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Start observing a producer stage even if that work never completes.
    /// `stage` identifies planning, rendering or finishing; returns after a
    /// fixed atomic update. Planning also starts the next block's elapsed clock.
    pub fn begin(&self, stage: PreparationStage) {
        let now = self.origin.elapsed().as_nanos() as u64;
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.fields[11].store(stage as u64, Ordering::SeqCst);
        if stage == PreparationStage::Plan {
            self.fields[12].store(now, Ordering::SeqCst);
        }
        self.fields[13].store(now, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Read a coherent preparation observation without waiting for its producer.
    /// Takes no arguments; returns none after three concurrent-write retries.
    pub fn observation(&self) -> Option<PreparationStats> {
        for _ in 0..3 {
            let generation = self.generation.load(Ordering::SeqCst);
            if !generation.is_multiple_of(2) {
                continue;
            }
            let fields = self.fields.each_ref().map(|v| v.load(Ordering::SeqCst));
            if self.generation.load(Ordering::SeqCst) == generation {
                let [
                    blocks,
                    published_blocks,
                    last_plan_ns,
                    last_render_ns,
                    last_finish_ns,
                    last_block_ns,
                    max_block_ns,
                    max_interval_ns,
                    slowest_plan_ns,
                    slowest_render_ns,
                    slowest_finish_ns,
                    stage,
                    block_start,
                    stage_start,
                ] = fields;
                let active_stage = match stage {
                    0 => PreparationStage::Idle,
                    1 => PreparationStage::Plan,
                    2 => PreparationStage::Render,
                    3 => PreparationStage::Finish,
                    _ => return None,
                };
                let now = self.origin.elapsed().as_nanos() as u64;
                return Some(PreparationStats {
                    blocks,
                    published_blocks,
                    last_plan_ns,
                    last_render_ns,
                    last_finish_ns,
                    last_block_ns,
                    max_block_ns,
                    max_interval_ns,
                    slowest_plan_ns,
                    slowest_render_ns,
                    slowest_finish_ns,
                    active_stage,
                    active_block_ns: if stage == 0 { 0 } else { now - block_start },
                    active_stage_ns: if stage == 0 { 0 } else { now - stage_start },
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_snapshots_never_mix_concurrent_block_fields() {
        let log = std::sync::Arc::new(PreparationLog::default());
        let producer = log.clone();
        let thread = std::thread::spawn(move || {
            for blocks in 1..20000 {
                producer.publish(PreparationStats {
                    blocks,
                    published_blocks: blocks,
                    last_plan_ns: blocks,
                    last_block_ns: blocks,
                    max_block_ns: blocks,
                    slowest_plan_ns: blocks,
                    ..PreparationStats::default()
                });
                producer.begin(PreparationStage::Plan);
                producer.begin(PreparationStage::Render);
                producer.begin(PreparationStage::Finish);
            }
        });
        while !thread.is_finished() {
            if let Some(observed) = log.observation() {
                assert_eq!(observed.blocks, observed.published_blocks);
                assert_eq!(observed.blocks, observed.last_plan_ns);
                assert_eq!(observed.blocks, observed.slowest_plan_ns);
                assert!(observed.valid_after(PreparationStats::default()));
            }
        }
        thread.join().unwrap();
        assert_eq!(log.observation().unwrap().blocks, 19999);
    }

    #[test]
    fn malformed_or_regressing_preparation_metrics_are_rejected() {
        let valid = PreparationStats {
            blocks: 2,
            published_blocks: 1,
            last_plan_ns: 10,
            last_render_ns: 20,
            last_finish_ns: 5,
            last_block_ns: 40,
            max_block_ns: 40,
            max_interval_ns: 80,
            slowest_plan_ns: 10,
            slowest_render_ns: 20,
            slowest_finish_ns: 5,
            ..PreparationStats::default()
        };
        assert!(valid.valid_after(PreparationStats::default()));
        for invalid in [
            PreparationStats { blocks: 0, ..valid },
            PreparationStats {
                published_blocks: 3,
                ..valid
            },
            PreparationStats {
                published_blocks: 0,
                ..valid
            },
            PreparationStats {
                max_block_ns: 39,
                ..valid
            },
            PreparationStats {
                max_interval_ns: 79,
                ..valid
            },
            PreparationStats {
                last_plan_ns: 41,
                ..valid
            },
            PreparationStats {
                slowest_plan_ns: u64::MAX,
                ..valid
            },
            PreparationStats {
                active_block_ns: 1,
                ..valid
            },
            PreparationStats {
                active_stage: PreparationStage::Render,
                active_stage_ns: 2,
                active_block_ns: 1,
                ..valid
            },
        ] {
            assert!(!invalid.valid_after(valid));
        }
    }

    #[test]
    fn unfinished_stages_remain_observable_until_completion() {
        let log = PreparationLog::default();
        log.begin(PreparationStage::Plan);
        log.begin(PreparationStage::Render);
        let rendering = log.observation().unwrap();
        assert_eq!(rendering.blocks, 0);
        assert_eq!(rendering.active_stage, PreparationStage::Render);
        assert!(rendering.active_block_ns >= rendering.active_stage_ns);
        assert!(rendering.valid_after(PreparationStats::default()));
        log.begin(PreparationStage::Finish);
        assert_eq!(
            log.observation().unwrap().active_stage,
            PreparationStage::Finish
        );
        log.publish(PreparationStats {
            blocks: 1,
            ..PreparationStats::default()
        });
        let completed = log.observation().unwrap();
        assert_eq!(completed.blocks, 1);
        assert_eq!(completed.active_stage, PreparationStage::Idle);
        assert_eq!(completed.active_block_ns, 0);
        assert_eq!(completed.active_stage_ns, 0);
    }
}
