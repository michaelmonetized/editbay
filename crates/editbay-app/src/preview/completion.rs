use crossbeam_queue::ArrayQueue;
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};
use uuid::Uuid;

const CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Scope {
    pub tab: Uuid,
    pub version: editbay_core::DocumentVersion,
    pub sequence: Uuid,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct Completed {
    pub session: Uuid,
    pub scope: Scope,
    pub index: u64,
    pub frame: u64,
    pub request_serial: u64,
    pub elapsed_us: u64,
    pub request_to_draw_us: u64,
}

struct Shared {
    queue: ArrayQueue<Completed>,
    open: AtomicBool,
    overflow: AtomicU64,
}

#[derive(Clone)]
pub(super) struct Publisher {
    session: Uuid,
    scope: Scope,
    began: Instant,
    shared: Arc<Shared>,
}

impl Publisher {
    /// Report one real GPU display completion without waiting for the UI.
    /// `frame`, `serial` and `request_to_draw_us` identify the completed picture.
    /// Returns immediately; bounded-channel loss is retained as invalid evidence.
    pub(super) fn completed(&self, frame: u64, serial: u64, request_to_draw_us: u64) {
        if !self.shared.open.load(Ordering::Acquire) {
            return;
        }
        let receipt = Completed {
            session: self.session,
            scope: self.scope,
            index: 0,
            frame,
            request_serial: serial,
            elapsed_us: self.began.elapsed().as_micros() as u64,
            request_to_draw_us,
        };
        if self.shared.queue.push(receipt).is_err() {
            self.shared.overflow.fetch_add(1, Ordering::AcqRel);
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Summary {
    pub session: Uuid,
    pub scope: Scope,
    pub first_frame: u64,
    pub end_frame_exclusive: u64,
    pub received: u64,
    pub unique_completed: u64,
    pub repeated: u64,
    pub rejected: u64,
    pub outside_history: u64,
    pub overflow: u64,
    pub cancelled: bool,
    pub observed_sound_end_us: Option<u64>,
    pub completed_by_observed_end: Option<u64>,
    pub missing_by_observed_end: Option<u64>,
}

pub(super) struct CompletionLog {
    publisher: Publisher,
    first: u64,
    end: u64,
    highest: Option<u64>,
    seen: u64,
    timely: u64,
    received: u64,
    unique: u64,
    timely_count: u64,
    repeated: u64,
    rejected: u64,
    outside_history: u64,
    cancelled: bool,
    ended_us: Option<u64>,
    events: VecDeque<Completed>,
}

impl CompletionLog {
    /// Capture a bounded display-completion session over an exact frame range.
    /// `scope` owns the sequence; `first` and `end` declare its half-open interval. Returns a new
    /// private session, or an error for an empty/reversed range.
    pub(super) fn new(scope: Scope, first: u64, end: u64) -> Result<Self, String> {
        if first >= end {
            return Err("Display playback needs a nonempty frame range".into());
        }
        Ok(Self {
            publisher: Publisher {
                session: Uuid::new_v4(),
                scope,
                began: Instant::now(),
                shared: Arc::new(Shared {
                    queue: ArrayQueue::new(CAPACITY),
                    open: AtomicBool::new(true),
                    overflow: AtomicU64::new(0),
                }),
            },
            first,
            end,
            highest: None,
            seen: 0,
            timely: 0,
            received: 0,
            unique: 0,
            timely_count: 0,
            repeated: 0,
            rejected: 0,
            outside_history: 0,
            cancelled: false,
            ended_us: None,
            events: VecDeque::with_capacity(CAPACITY),
        })
    }

    /// Retain the bounded sender for this specific play session.
    /// Takes this log. Returns a clone that cannot publish into a later session.
    pub(super) fn publisher(&self) -> Publisher {
        self.publisher.clone()
    }

    /// Consume at most one channel capacity of actual completion records.
    /// Takes this log. Returns without waiting; late, repeated and foreign records
    /// remain explicit, and a bounded history never silently asserts completeness.
    pub(super) fn poll(&mut self) {
        for _ in 0..CAPACITY {
            let Some(mut receipt) = self.publisher.shared.queue.pop() else {
                break;
            };
            self.received = self.received.saturating_add(1);
            receipt.index = self.received;
            if self.events.len() == CAPACITY {
                self.events.pop_front();
            }
            self.events.push_back(receipt);
            self.accept(receipt);
        }
    }

    fn accept(&mut self, receipt: Completed) {
        if receipt.session != self.publisher.session
            || receipt.scope != self.publisher.scope
            || receipt.frame < self.first
            || receipt.frame >= self.end
        {
            self.rejected = self.rejected.saturating_add(1);
            return;
        }
        if let Some(highest) = self.highest {
            if receipt.frame > highest {
                let shift = receipt.frame - highest;
                self.seen = self.seen.checked_shl(shift.min(64) as u32).unwrap_or(0);
                self.timely = self.timely.checked_shl(shift.min(64) as u32).unwrap_or(0);
                self.highest = Some(receipt.frame);
            } else if highest - receipt.frame >= CAPACITY as u64 {
                self.outside_history = self.outside_history.saturating_add(1);
                return;
            }
        } else {
            self.highest = Some(receipt.frame);
        }
        let bit = 1 << (self.highest.unwrap() - receipt.frame);
        if self.seen & bit == 0 {
            self.seen |= bit;
            self.unique += 1;
        } else {
            self.repeated = self.repeated.saturating_add(1);
        }
        if self.timely & bit == 0 && self.ended_us.is_none_or(|end| receipt.elapsed_us <= end) {
            self.timely |= bit;
            self.timely_count += 1;
        }
    }

    /// Check whether a worker picture belongs to this playback's frame range.
    /// `frame` is the captured sequence position. Returns false before the start
    /// or at/after the exclusive end, including late results from a prior seek.
    pub(super) fn contains_frame(&self, frame: u64) -> bool {
        self.first <= frame && frame < self.end
    }

    /// Mark the observed sound-end time while retaining late GPU receipts.
    /// Takes this log. Returns after a bounded drain; later completions remain
    /// counted separately from pictures completed by this observed boundary.
    pub(super) fn observe_end(&mut self) {
        if self.ended_us.is_none() {
            self.ended_us = Some(self.publisher.began.elapsed().as_micros() as u64);
        }
        self.poll();
    }

    /// Stop accepting receipts when playback is cancelled or its owner changes.
    /// Takes this log. Returns after closing its publisher and draining available
    /// receipts; a partial/cancelled session cannot become complete evidence.
    pub(super) fn cancel(&mut self) {
        self.publisher.shared.open.store(false, Ordering::Release);
        self.cancelled = true;
        self.poll();
    }

    /// Inspect bounded per-play counts, including initial and trailing omissions.
    /// Takes this log. Returns no missing-frame claim when evidence is incomplete.
    pub(super) fn summary(&self) -> Summary {
        let overflow = self.publisher.shared.overflow.load(Ordering::Acquire);
        let trusted =
            !self.cancelled && self.rejected == 0 && self.outside_history == 0 && overflow == 0;
        Summary {
            session: self.publisher.session,
            scope: self.publisher.scope,
            first_frame: self.first,
            end_frame_exclusive: self.end,
            received: self.received,
            unique_completed: self.unique,
            repeated: self.repeated,
            rejected: self.rejected,
            outside_history: self.outside_history,
            overflow,
            cancelled: self.cancelled,
            observed_sound_end_us: self.ended_us,
            completed_by_observed_end: self.ended_us.filter(|_| trusted).map(|_| self.timely_count),
            missing_by_observed_end: self
                .ended_us
                .filter(|_| trusted)
                .map(|_| self.end - self.first - self.timely_count),
        }
    }

    /// Inspect the most recent bounded records for the diagnostics writer.
    /// Takes this log. Returns its private session and at most 64 numbered records.
    pub(super) fn events(&self) -> (Uuid, &VecDeque<Completed>) {
        (self.publisher.session, &self.events)
    }
}

impl Drop for CompletionLog {
    fn drop(&mut self) {
        self.publisher.shared.open.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(first: u64, end: u64) -> CompletionLog {
        CompletionLog::new(
            Scope {
                tab: Uuid::new_v4(),
                version: editbay_core::DocumentVersion::of(
                    &editbay_core::Project::new("Completion test").unwrap(),
                ),
                sequence: Uuid::new_v4(),
            },
            first,
            end,
        )
        .unwrap()
    }

    fn send(log: &CompletionLog, frame: u64, elapsed_us: u64) {
        log.publisher
            .shared
            .queue
            .push(Completed {
                session: log.publisher.session,
                scope: log.publisher.scope,
                index: 0,
                frame,
                request_serial: frame + 1,
                elapsed_us,
                request_to_draw_us: 10,
            })
            .unwrap();
    }

    #[test]
    fn repeated_reordered_and_late_draws_keep_initial_and_trailing_gaps() {
        let mut log = log(10, 20);
        for frame in [12, 14, 13, 14] {
            send(&log, frame, 1);
        }
        log.poll();
        log.ended_us = Some(5);
        send(&log, 19, 10);
        send(&log, 19, 4);
        send(&log, 18, 10);
        log.poll();
        let result = log.summary();
        assert_eq!(result.unique_completed, 5);
        assert_eq!(result.completed_by_observed_end, Some(4));
        assert_eq!(result.missing_by_observed_end, Some(6));
        assert_eq!(result.repeated, 2);
        assert_eq!(log.events().1.len(), 7);
    }

    #[test]
    fn bounded_history_counts_long_runs_but_refuses_unverifiable_late_records() {
        let mut log = log(0, 1000);
        for frame in 0..1000 {
            send(&log, frame, 0);
            log.poll();
        }
        log.observe_end();
        assert_eq!(log.summary().missing_by_observed_end, Some(0));
        assert_eq!(log.events().1.len(), CAPACITY);
        send(&log, 0, 0);
        log.poll();
        assert_eq!(log.summary().outside_history, 1);
        assert!(log.summary().missing_by_observed_end.is_none());
    }

    #[test]
    fn overflow_foreign_ranges_cancellation_and_retired_publishers_never_pass() {
        let mut overflowing = log(0, 100);
        for frame in 0..=CAPACITY as u64 {
            overflowing.publisher().completed(frame, frame, 10);
        }
        overflowing.observe_end();
        assert_eq!(overflowing.summary().overflow, 1);
        assert!(overflowing.summary().missing_by_observed_end.is_none());
        let mut wrong = log(10, 20);
        send(&wrong, 9, 0);
        send(&wrong, 20, 0);
        wrong
            .publisher
            .shared
            .queue
            .push(Completed {
                session: Uuid::new_v4(),
                scope: wrong.publisher.scope,
                index: 0,
                frame: 11,
                request_serial: 1,
                elapsed_us: 0,
                request_to_draw_us: 0,
            })
            .unwrap();
        wrong.observe_end();
        assert_eq!(wrong.summary().rejected, 3);
        assert!(wrong.summary().missing_by_observed_end.is_none());
        let retired = wrong.publisher();
        wrong.cancel();
        retired.completed(12, 2, 10);
        wrong.poll();
        assert_eq!(wrong.summary().received, 3);
        assert!(wrong.summary().cancelled);
        drop(wrong);
        retired.completed(13, 3, 10);
        assert_eq!(retired.shared.overflow.load(Ordering::Acquire), 0);
    }

    #[test]
    fn concurrent_callbacks_retain_every_frame_without_waiting_for_ui() {
        let mut log = log(0, 32);
        let barrier = Arc::new(std::sync::Barrier::new(8));
        std::thread::scope(|threads| {
            for producer in 0..8 {
                let publisher = log.publisher();
                let barrier = barrier.clone();
                threads.spawn(move || {
                    barrier.wait();
                    for frame in producer * 4..producer * 4 + 4 {
                        publisher.completed(frame, frame + 1, 10);
                    }
                });
            }
        });
        log.observe_end();
        assert_eq!(log.summary().received, 32);
        assert_eq!(log.summary().missing_by_observed_end, Some(0));
        let mut foreign = log.events[0];
        foreign.scope.tab = Uuid::new_v4();
        log.publisher.shared.queue.push(foreign).unwrap();
        log.poll();
        assert_eq!(log.summary().rejected, 1);
        assert!(log.summary().missing_by_observed_end.is_none());
    }
}
