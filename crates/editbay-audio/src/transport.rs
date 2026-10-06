use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Running,
    Ended,
    Cancelled,
    SourceFailed,
    DeviceFailed,
    Underrun,
    InvalidOutput,
    BackendClockReset,
    BackendDiscontinuity,
}

struct Shared {
    samples: Box<[AtomicU32]>,
    channels: usize,
    capacity: u64,
    start: u64,
    length: u64,
    read: AtomicU64,
    written: AtomicU64,
    state: AtomicU8,
}

#[derive(Clone)]
pub(crate) struct Control(Arc<Shared>);
pub(crate) struct Writer(Control);
pub(crate) struct Reader(Control);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Consumed {
    pub first: u64,
    pub frames: u64,
}

/// Allocate one bounded sound transport with unique producer and consumer.
/// `start`, `end`, `channels` and `capacity` specify absolute samples and storage.
/// Returns separate handles; no buffer allocation occurs after construction.
pub(crate) fn transport(
    start: u64,
    end: u64,
    channels: usize,
    capacity: u32,
) -> Result<(Control, Writer, Reader), &'static str> {
    if start >= end || !(1..=64).contains(&channels) || !(2..=65536).contains(&capacity) {
        return Err("Invalid prepared sound transport bounds");
    }
    let control = Control(Arc::new(Shared {
        samples: (0..capacity as usize * channels)
            .map(|_| AtomicU32::new(0))
            .collect(),
        channels,
        capacity: u64::from(capacity),
        start,
        length: end - start,
        read: AtomicU64::new(0),
        written: AtomicU64::new(0),
        state: AtomicU8::new(0),
    }));
    Ok((control.clone(), Writer(control.clone()), Reader(control)))
}

impl Control {
    /// Inspect the latched delivery state.
    /// Takes no arguments; returns the first end, cancellation or failure.
    pub(crate) fn state(&self) -> State {
        match self.0.state.load(Ordering::Acquire) {
            0 => State::Running,
            1 => State::Ended,
            2 => State::Cancelled,
            3 => State::SourceFailed,
            4 => State::DeviceFailed,
            5 => State::Underrun,
            7 => State::BackendClockReset,
            8 => State::BackendDiscontinuity,
            _ => State::InvalidOutput,
        }
    }
    /// Stop delivery without waiting for any producer or callback.
    /// `state` names cancellation or failure. Returns no value; later failures
    /// cannot replace the original failure or revive queued sound. Device failure
    /// can still interrupt a drained queue while backend latency is elapsing.
    pub(crate) fn stop(&self, state: State) {
        if state != State::Running {
            let previous =
                self.0
                    .state
                    .compare_exchange(0, state as u8, Ordering::AcqRel, Ordering::Acquire);
            if matches!(
                state,
                State::DeviceFailed | State::BackendClockReset | State::BackendDiscontinuity
            ) && previous == Err(State::Ended as u8)
            {
                let _ = self.0.state.compare_exchange(
                    State::Ended as u8,
                    state as u8,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
        }
    }
    /// Inspect currently prepared frames.
    /// Takes no arguments; returns bounded queue occupancy without blocking.
    pub(crate) fn prepared(&self) -> u64 {
        let read = self.0.read.load(Ordering::Acquire);
        (self.0.written.load(Ordering::Acquire) - read).min(self.0.capacity)
    }
}

impl Writer {
    /// Copy one validated consecutive block outside the device callback.
    /// `first` is its absolute first frame; `samples` retain the chosen layout.
    /// Returns false for a full buffer, or rejects invalid/stale/stopped work.
    pub(crate) fn push(&mut self, first: u64, samples: &[f32]) -> Result<bool, &'static str> {
        let shared = &self.0.0;
        if self.0.state() != State::Running {
            return Err("Sound transport stopped");
        }
        let written = shared.written.load(Ordering::Relaxed);
        let count = (samples.len() / shared.channels) as u64;
        if samples.is_empty()
            || !samples.len().is_multiple_of(shared.channels)
            || first != shared.start + written
            || count > shared.capacity
            || count > shared.length - written
            || samples.iter().any(|v| !v.is_finite())
        {
            return Err("Invalid or nonconsecutive prepared sound");
        }
        let read = shared.read.load(Ordering::Acquire);
        if count > shared.capacity - (written - read) {
            return Ok(false);
        }
        for (frame, values) in samples.chunks_exact(shared.channels).enumerate() {
            let base = ((written + frame as u64) % shared.capacity) as usize * shared.channels;
            for (channel, value) in values.iter().enumerate() {
                shared.samples[base + channel].store(value.to_bits(), Ordering::Relaxed);
            }
        }
        shared.written.store(written + count, Ordering::Release);
        Ok(true)
    }
}

impl Reader {
    /// Consume prepared samples into a device-owned output buffer.
    /// `output` has the declared layout and `convert` performs sample conversion.
    /// Returns consumed source frames. Missing sound latches underrun; end and
    /// cancellation fill silence. This routine only reads/writes atomics and slices.
    pub(crate) fn consume<T: Copy>(
        &mut self,
        output: &mut [T],
        mut convert: impl FnMut(f32) -> T,
    ) -> Consumed {
        let shared = &self.0.0;
        output.fill(convert(0.));
        let read = shared.read.load(Ordering::Relaxed);
        let mut receipt = Consumed {
            first: shared.start + read,
            frames: 0,
        };
        if self.0.state() != State::Running {
            return receipt;
        }
        if output.is_empty() || !output.len().is_multiple_of(shared.channels) {
            self.0.stop(State::InvalidOutput);
            return receipt;
        }
        let count = (output.len() / shared.channels) as u64;
        let available = shared.written.load(Ordering::Acquire) - read;
        for (frame, values) in output
            .chunks_exact_mut(shared.channels)
            .take(count.min(available) as usize)
            .enumerate()
        {
            if self.0.state() != State::Running {
                break;
            }
            let base = ((read + frame as u64) % shared.capacity) as usize * shared.channels;
            for (channel, value) in values.iter_mut().enumerate() {
                *value = convert(f32::from_bits(
                    shared.samples[base + channel].load(Ordering::Relaxed),
                ));
            }
            receipt.frames += 1;
        }
        shared.read.store(read + receipt.frames, Ordering::Release);
        if read + receipt.frames == shared.length {
            self.0.stop(State::Ended);
        } else if receipt.frames < count {
            self.0.stop(State::Underrun);
        }
        receipt
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if self.0.0.written.load(Ordering::Relaxed) != self.0.0.length {
            self.0.stop(State::SourceFailed);
        }
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.0.stop(State::Cancelled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraparound_partial_end_and_absolute_clock_preserve_every_channel() {
        let start = u64::MAX - 8;
        let (control, mut writer, mut reader) = transport(start, start + 6, 2, 5).unwrap();
        assert!(writer.push(start, &[0., 1., 2., 3., 4., 5.]).unwrap());
        assert!(!writer.push(start + 3, &[6., 7., 8., 9., 10., 11.]).unwrap());
        let mut first = [0.; 4];
        assert_eq!(
            reader.consume(&mut first, |v| v),
            Consumed {
                first: start,
                frames: 2
            }
        );
        assert_eq!(first, [0., 1., 2., 3.]);
        assert!(writer.push(start + 3, &[6., 7., 8., 9., 10., 11.]).unwrap());
        drop(writer);
        let mut tail = [9.; 12];
        assert_eq!(
            reader.consume(&mut tail, |v| v),
            Consumed {
                first: start + 2,
                frames: 4
            }
        );
        assert_eq!(tail, [4., 5., 6., 7., 8., 9., 10., 11., 0., 0., 0., 0.]);
        assert_eq!(control.state(), State::Ended);
        assert_eq!(control.prepared(), 0);
        assert_eq!(reader.consume(&mut tail, |v| v).frames, 0);
        assert_eq!(tail, [0.; 12]);
        control.stop(State::DeviceFailed);
        assert_eq!(control.state(), State::DeviceFailed);
    }

    #[test]
    fn invalid_stale_underrun_and_cancellation_never_resume_queued_sound() {
        let (control, mut writer, mut reader) = transport(100, 108, 1, 4).unwrap();
        assert!(writer.push(99, &[1.]).is_err());
        assert!(writer.push(100, &[f32::NAN]).is_err());
        writer.push(100, &[1., 2.]).unwrap();
        let mut output = [9.; 4];
        reader.consume(&mut output, |v| v);
        assert_eq!(output, [1., 2., 0., 0.]);
        assert_eq!(control.state(), State::Underrun);
        assert!(writer.push(102, &[3.]).is_err());
        reader.consume(&mut output, |v| v);
        assert_eq!(output, [0.; 4]);
        let (control, mut writer, mut reader) = transport(0, 4, 1, 4).unwrap();
        writer.push(0, &[1., 2., 3., 4.]).unwrap();
        control.stop(State::Cancelled);
        reader.consume(&mut output, |v| v);
        assert_eq!(output, [0.; 4]);
        assert_eq!(control.state(), State::Cancelled);
        for length in [0, 3] {
            let (control, _writer, mut reader) = transport(0, 4, 2, 4).unwrap();
            reader.consume(&mut output[..length], |v| v);
            assert_eq!(control.state(), State::InvalidOutput);
        }
    }

    #[test]
    fn backend_reset_during_tail_drain_overrides_end_but_not_an_earlier_failure() {
        let (control, mut writer, mut reader) = transport(0, 2, 1, 4).unwrap();
        writer.push(0, &[1., 2.]).unwrap();
        reader.consume(&mut [0.; 4], |v| v);
        assert_eq!(control.state(), State::Ended);
        control.stop(State::BackendClockReset);
        assert_eq!(control.state(), State::BackendClockReset);
        control.stop(State::DeviceFailed);
        assert_eq!(control.state(), State::BackendClockReset);
    }

    #[test]
    fn concurrent_bounded_delivery_survives_many_wraps_without_loss() {
        let (control, mut writer, mut reader) = transport(0, 100_000, 1, 31).unwrap();
        let producer = std::thread::spawn(move || {
            for first in (0..100_000).step_by(7) {
                let count = (100_000 - first).min(7);
                let block: Vec<_> = (first..first + count).map(|v| v as f32).collect();
                while !writer.push(first, &block).unwrap() {
                    std::thread::yield_now();
                }
            }
        });
        for first in (0..100_000).step_by(5) {
            while control.prepared() < 5 {
                std::thread::yield_now();
            }
            let mut block = [0.; 5];
            let result = reader.consume(&mut block, |v| v);
            assert_eq!(result, Consumed { first, frames: 5 });
            for (index, value) in block.iter().enumerate() {
                assert_eq!(*value, (first + index as u64) as f32);
            }
        }
        producer.join().unwrap();
        assert_eq!(control.state(), State::Ended);
    }
}
