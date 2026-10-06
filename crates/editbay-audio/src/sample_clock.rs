use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) struct SampleClock {
    rate: u32,
    first: u64,
    generation: AtomicU64,
    buffer_start: AtomicU64,
    submitted: AtomicU64,
    callback_ns: AtomicU64,
    latency_ns: AtomicU64,
    limit: AtomicU64,
    last_read: AtomicU64,
    callbacks: AtomicU64,
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
            latency_ns: AtomicU64::new(0),
            limit: AtomicU64::new(end),
            last_read: AtomicU64::new(first),
            callbacks: AtomicU64::new(0),
        })
    }

    /// Publish a callback's device interval and reported playback latency.
    /// `start` and `submitted` count every device frame including end padding;
    /// `callback_ns` shares one monotonic origin, `latency_ns` is backend delay,
    /// and `limit` caps valid source sound. Returns false for inconsistent input.
    /// One callback owns publication; only fixed atomic operations occur here.
    pub(crate) fn record(
        &self,
        start: u64,
        submitted: u64,
        callback_ns: u64,
        latency_ns: u64,
        limit: u64,
    ) -> bool {
        let previous_limit = self.limit.load(Ordering::Relaxed);
        let accepted =
            (u128::from(self.first) + u128::from(start)).min(u128::from(previous_limit)) as u64;
        if start != self.submitted.load(Ordering::Relaxed)
            || submitted < start
            || !(accepted..=previous_limit).contains(&limit)
            || callback_ns < self.callback_ns.load(Ordering::Relaxed)
        {
            return false;
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.buffer_start.store(start, Ordering::SeqCst);
        self.submitted.store(submitted, Ordering::SeqCst);
        self.callback_ns.store(callback_ns, Ordering::SeqCst);
        self.latency_ns.store(latency_ns, Ordering::SeqCst);
        self.limit.store(limit, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.callbacks.fetch_add(1, Ordering::Relaxed);
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

    /// Inspect the number of published callbacks and latest backend latency.
    /// Takes no arguments; returns independently observed callback count and ns.
    pub(crate) fn counters(&self) -> (u64, u64) {
        (
            self.callbacks.load(Ordering::Relaxed),
            self.latency_ns.load(Ordering::SeqCst),
        )
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
        assert_eq!(clock.counters(), (5, 30_000_000));
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
                assert!(clock.record(index * block, (index + 1) * block, now, 17_000_000, end));
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
        assert!(clock.record(0, 480, 0, 0, 100000));
        assert_eq!(clock.position(9_000_000), 432);
        assert!(clock.record(480, 960, 10_000_000, 20_000_000, 100000));
        assert_eq!(clock.position(11_000_000), 432);
        assert!(clock.record(960, 1440, 20_000_000, 0, 1000));
        assert_eq!(clock.position(1_000_000_000), 1000);
        assert!(!clock.record(1441, 1900, 30_000_000, 0, 1000));
        assert!(!clock.record(1440, 1900, 30_000_000, 0, 100001));
        assert!(!clock.record(1440, 1900, 30_000_000, 0, 100000));
        assert!(!clock.record(1440, 1900, 30_000_000, 0, 999));
        assert_eq!(clock.counters().0, 3);
    }
}
