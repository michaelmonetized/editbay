use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(super) struct NativeContinuity {
    previous: Option<(u64, u64)>,
}
impl NativeContinuity {
    /// Check the hardware graph without losing an underrun between callbacks.
    /// `clock`, `xruns` and `flags` are native driver observations. Returns false
    /// for recovery, a changed clock or any change in accumulated underrun time.
    pub(super) fn observe(&mut self, clock: u64, xruns: u64, flags: u64) -> bool {
        let current = (clock, xruns);
        flags & 2 == 0
            && self
                .previous
                .replace(current)
                .is_none_or(|last| last == current)
    }
}

/// One native graph observation, retained separately from accepted source samples.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeOutputStats {
    pub process_calls: u64,
    pub dequeue_misses: u64,
    pub graph_ticks: u64,
    pub maximum_ticks_step: u64,
    pub graph_flags: u64,
    pub graph_duration: u64,
    pub graph_xrun_ticks: u64,
    pub callback_ns: u64,
    pub graph_ns: u64,
    pub graph_delay_ticks: u64,
    pub queued_frames: u64,
    pub buffered_frames: u64,
    pub rate_numerator: u64,
    pub rate_denominator: u64,
    pub graph_clock_id: u64,
    pub queued_buffers: u64,
    pub available_buffers: u64,
}

pub(super) struct NativeTrace {
    generation: AtomicU64,
    values: [AtomicU64; 17],
}
impl Default for NativeTrace {
    fn default() -> Self {
        Self {
            generation: AtomicU64::new(0),
            values: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}
impl NativeTrace {
    /// Publish fixed native graph counters on the exclusive realtime callback.
    /// `values` are the validated C timing array. Returns after atomic stores only.
    pub(super) fn record(&self, values: &[u64; 17]) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        for (slot, value) in self.values.iter().zip(values) {
            slot.store(*value, Ordering::SeqCst);
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Read a coherent graph observation without waiting for the callback.
    /// Takes no arguments; returns none before publication or during bounded races.
    pub(super) fn observation(&self) -> Option<NativeOutputStats> {
        for _ in 0..4 {
            let before = self.generation.load(Ordering::SeqCst);
            if before == 0 || !before.is_multiple_of(2) {
                continue;
            }
            let v: [u64; 17] = std::array::from_fn(|i| self.values[i].load(Ordering::SeqCst));
            if self.generation.load(Ordering::SeqCst) == before {
                return Some(NativeOutputStats {
                    process_calls: v[0],
                    dequeue_misses: v[1],
                    graph_ticks: v[2],
                    maximum_ticks_step: v[3],
                    graph_flags: v[4],
                    graph_duration: v[5],
                    graph_xrun_ticks: v[6],
                    callback_ns: v[7],
                    graph_ns: v[8],
                    graph_delay_ticks: v[9],
                    queued_frames: v[10],
                    buffered_frames: v[11],
                    rate_numerator: v[12],
                    rate_denominator: v[13],
                    graph_clock_id: v[14],
                    queued_buffers: v[15],
                    available_buffers: v[16],
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
    fn accumulated_driver_underruns_survive_a_cleared_recovery_flag() {
        let mut graph = NativeContinuity::default();
        assert!(graph.observe(37, 14283, 0));
        assert!(graph.observe(37, 14283, 0));
        assert!(!graph.observe(37, 18394, 0));
        assert!(!NativeContinuity::default().observe(37, 0, 2));
        let mut reset = NativeContinuity::default();
        assert!(reset.observe(37, 14283, 0));
        assert!(!reset.observe(37, 0, 0));
        let mut changed = NativeContinuity::default();
        assert!(changed.observe(37, 0, 0));
        assert!(!changed.observe(38, 0, 0));
    }
}
