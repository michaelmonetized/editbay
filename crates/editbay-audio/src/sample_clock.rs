use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// One coherent callback receipt, with device frames relative to playback start.
/// Host time uses the playback's monotonic origin; backend time uses its first
/// callback timestamp. Neither clock measures physical speaker output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockObservation {
    pub callbacks: u64,
    pub buffer_start_frames: u64,
    pub submitted_frames: u64,
    pub callback_elapsed_ns: u64,
    pub backend_elapsed_ns: u64,
    pub backend_epoch: u64,
    pub reported_latency_ns: u64,
    pub valid_end_sample: u64,
}

pub(crate) struct SampleClock {
    rate: u32,
    first: u64,
    generation: AtomicU64,
    buffer_start: AtomicU64,
    submitted: AtomicU64,
    callback_ns: AtomicU64,
    backend_ns: AtomicU64,
    backend_epoch: AtomicU64,
    latency_ns: AtomicU64,
    limit: AtomicU64,
    last_read: AtomicU64,
}

impl SampleClock {
    /// Create an exact sample clock for one playback lifetime.
    /// `rate`, `first` and `end` specify output samples and their exclusive end.
    /// Returns a stopped-at-start observation until the first callback publishes.
    pub(crate) fn new(rate: u32, first: u64, end: u64) -> Result<Self, &'static str> {
        if !(8000..=384000).contains(&rate) || first >= end {
            return Err("Invalid sound clock interval");
        }
        Ok(Self {
            rate,
            first,
            generation: AtomicU64::new(0),
            buffer_start: AtomicU64::new(0),
            submitted: AtomicU64::new(0),
            callback_ns: AtomicU64::new(0),
            backend_ns: AtomicU64::new(0),
            backend_epoch: AtomicU64::new(0),
            latency_ns: AtomicU64::new(0),
            limit: AtomicU64::new(end),
            last_read: AtomicU64::new(first),
        })
    }

    /// Publish a callback's device interval and reported playback latency.
    /// `start` and `submitted` count every device frame including end padding;
    /// `callback_ns` shares one host origin, `backend` gives the timestamp epoch
    /// and elapsed ns, `latency_ns` is backend delay,
    /// and `limit` caps valid source sound. Returns false for inconsistent input.
    /// One callback owns publication; only fixed atomic operations occur here.
    pub(crate) fn record(
        &self,
        start: u64,
        submitted: u64,
        callback_ns: u64,
        backend: (u64, u64),
        latency_ns: u64,
        limit: u64,
    ) -> bool {
        let previous_limit = self.limit.load(Ordering::Relaxed);
        let (epoch, backend_ns) = backend;
        let previous_epoch = self.backend_epoch.load(Ordering::Relaxed);
        let valid_epoch = epoch == previous_epoch
            && backend_ns >= self.backend_ns.load(Ordering::Relaxed)
            || (previous_epoch == 0
                && epoch == 1
                && callback_ns < 1_000_000_000
                && backend_ns == 0);
        let accepted =
            (u128::from(self.first) + u128::from(start)).min(u128::from(previous_limit)) as u64;
        if start != self.submitted.load(Ordering::Relaxed)
            || submitted < start
            || !(accepted..=previous_limit).contains(&limit)
            || callback_ns < self.callback_ns.load(Ordering::Relaxed)
            || !valid_epoch
        {
            return false;
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.buffer_start.store(start, Ordering::SeqCst);
        self.submitted.store(submitted, Ordering::SeqCst);
        self.callback_ns.store(callback_ns, Ordering::SeqCst);
        self.backend_ns.store(backend_ns, Ordering::SeqCst);
        self.backend_epoch.store(epoch, Ordering::SeqCst);
        self.latency_ns.store(latency_ns, Ordering::SeqCst);
        self.limit.store(limit, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        true
    }

    /// Read the monotonic device-estimated source sample position.
    /// `now_ns` uses the callback's monotonic origin. Returns an exact integer
    /// boundary capped by submitted valid sound; three attempts bound contention.
    pub(crate) fn position(&self, now_ns: u64) -> u64 {
        for _ in 0..3 {
            let generation = self.generation.load(Ordering::SeqCst);
            if !generation.is_multiple_of(2) {
                continue;
            }
            let start = self.buffer_start.load(Ordering::SeqCst);
            let submitted = self.submitted.load(Ordering::SeqCst);
            let callback = self.callback_ns.load(Ordering::SeqCst);
            let latency = self.latency_ns.load(Ordering::SeqCst);
            let limit = self.limit.load(Ordering::SeqCst);
            if generation != self.generation.load(Ordering::SeqCst) {
                continue;
            }
            let elapsed = u128::from(now_ns.saturating_sub(callback)) * u128::from(self.rate);
            let delayed = (u128::from(start) * 1_000_000_000 + elapsed)
                .saturating_sub(u128::from(latency) * u128::from(self.rate))
                .min(u128::from(submitted) * 1_000_000_000);
            let position =
                (u128::from(self.first) + delayed / 1_000_000_000).min(u128::from(limit)) as u64;
            let previous = self.last_read.fetch_max(position, Ordering::AcqRel);
            return position.max(previous).min(limit);
        }
        self.last_read.load(Ordering::Acquire)
    }

    /// Read one complete callback publication without waiting on its writer.
    /// Takes no arguments; returns none before playback or after three contended
    /// attempts. Callers may retain their previous coherent observation.
    pub(crate) fn observation(&self) -> Option<ClockObservation> {
        for _ in 0..3 {
            let generation = self.generation.load(Ordering::SeqCst);
            if generation == 0 || !generation.is_multiple_of(2) {
                continue;
            }
            let observation = ClockObservation {
                callbacks: generation / 2,
                buffer_start_frames: self.buffer_start.load(Ordering::SeqCst),
                submitted_frames: self.submitted.load(Ordering::SeqCst),
                callback_elapsed_ns: self.callback_ns.load(Ordering::SeqCst),
                backend_elapsed_ns: self.backend_ns.load(Ordering::SeqCst),
                backend_epoch: self.backend_epoch.load(Ordering::SeqCst),
                reported_latency_ns: self.latency_ns.load(Ordering::SeqCst),
                valid_end_sample: self.limit.load(Ordering::SeqCst),
            };
            if generation == self.generation.load(Ordering::SeqCst) {
                return Some(observation);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_tail_finishes_when_latency_spans_multiple_buffers() {
        let clock = SampleClock::new(48000, 100, 750).unwrap();
        assert_eq!(clock.position(999999), 100);
        for index in 0..5 {
            assert!(clock.record(
                index * 480,
                (index + 1) * 480,
                index * 10_000_000,
                (0, index * 10_000_000),
                30_000_000,
                750
            ));
            assert_eq!(
                clock.position(index * 10_000_000),
                100 + (index.saturating_sub(3) * 480).min(650)
            );
        }
        assert_eq!(clock.position(45_000_000), 750);
        assert_eq!(clock.position(100_000_000), 750);
        let observed = clock.observation().unwrap();
        assert_eq!(observed.callbacks, 5);
        assert_eq!(observed.reported_latency_ns, 30_000_000);
    }

    #[test]
    fn two_hour_integer_clock_has_no_accumulated_rounding_or_submission_overrun() {
        for rate in [44100, 48000, 96000] {
            let first = u64::MAX - u64::from(rate) * 7201;
            let end = first + u64::from(rate) * 7200;
            let clock = SampleClock::new(rate, first, end).unwrap();
            let block = u64::from(rate) / 2;
            for index in 0..14400 {
                let now = index * 500_000_000;
                assert!(clock.record(
                    index * block,
                    (index + 1) * block,
                    now,
                    (0, now),
                    17_000_000,
                    end
                ));
                let elapsed = (now + 3_400_000).saturating_sub(17_000_000);
                let expected =
                    first + (u128::from(elapsed) * u128::from(rate) / 1_000_000_000) as u64;
                assert_eq!(clock.position(now + 3_400_000), expected);
            }
            assert_eq!(clock.position(7_300_000_000_000), end);
        }
    }

    #[test]
    fn jitter_failure_caps_and_invalid_publications_never_advance_missing_sound() {
        let clock = SampleClock::new(48000, 0, 100000).unwrap();
        assert!(clock.record(0, 480, 0, (0, 0), 0, 100000));
        assert_eq!(clock.position(9_000_000), 432);
        assert!(clock.record(480, 960, 10_000_000, (0, 10_000_000), 20_000_000, 100000));
        assert_eq!(clock.position(11_000_000), 432);
        assert!(clock.record(960, 1440, 20_000_000, (0, 20_000_000), 0, 1000));
        assert_eq!(clock.position(1_000_000_000), 1000);
        assert!(!clock.record(1441, 1900, 30_000_000, (0, 30_000_000), 0, 1000));
        assert!(!clock.record(1440, 1900, 30_000_000, (0, 30_000_000), 0, 100001));
        assert!(!clock.record(1440, 1900, 30_000_000, (0, 30_000_000), 0, 100000));
        assert!(!clock.record(1440, 1900, 30_000_000, (0, 30_000_000), 0, 999));
        assert!(!clock.record(1440, 1900, 30_000_000, (0, 19_000_000), 0, 1000));
        assert_eq!(clock.observation().unwrap().callbacks, 3);
    }

    #[test]
    fn contended_observations_never_mix_callback_publications() {
        let clock = std::sync::Arc::new(SampleClock::new(48000, 0, 100000000).unwrap());
        assert!(clock.observation().is_none());
        let writer = clock.clone();
        let thread = std::thread::spawn(move || {
            for i in 0..10000 {
                assert!(writer.record(
                    i * 480,
                    (i + 1) * 480,
                    i * 10000000 + 7,
                    (0, i * 10000000),
                    i,
                    100000000
                ));
            }
        });
        while !thread.is_finished() {
            if let Some(observed) = clock.observation() {
                let i = observed.callbacks - 1;
                assert_eq!(observed.buffer_start_frames, i * 480);
                assert_eq!(observed.submitted_frames, (i + 1) * 480);
                assert_eq!(observed.callback_elapsed_ns, i * 10000000 + 7);
                assert_eq!(observed.backend_elapsed_ns, i * 10000000);
                assert_eq!(observed.reported_latency_ns, i);
            }
        }
        thread.join().unwrap();
        assert_eq!(clock.observation().unwrap().callbacks, 10000);
    }

    #[test]
    fn backend_startup_epoch_is_explicit_and_later_resets_fail() {
        let clock = SampleClock::new(48000, 0, 480000).unwrap();
        assert!(clock.record(0, 480, 10_000_000, (0, 0), 30_000_000, 480000));
        assert!(clock.record(480, 960, 20_000_000, (1, 0), 30_000_000, 480000));
        assert_eq!(clock.observation().unwrap().backend_epoch, 1);
        assert!(!clock.record(960, 1440, 30_000_000, (2, 0), 30_000_000, 480000));
        let late = SampleClock::new(48000, 0, 480000).unwrap();
        assert!(late.record(0, 480, 10_000_000, (0, 0), 30_000_000, 480000));
        assert!(!late.record(480, 960, 1_000_000_000, (1, 0), 30_000_000, 480000));
    }
}
