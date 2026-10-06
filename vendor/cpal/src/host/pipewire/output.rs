use crate::{Error, ErrorKind};
use pipewire::{self as pw, spa::sys::spa_chunk, stream::Stream};
use std::{ptr::NonNull, slice};

pub(super) struct OutputBuffer<'a> {
    raw: NonNull<pw::sys::pw_buffer>,
    stream: &'a Stream,
    chunk: Option<NonNull<spa_chunk>>,
    stride: usize,
    frames: usize,
    queued: bool,
}

impl<'a> OutputBuffer<'a> {
    /// Own one dequeued native buffer until it is returned to its stream.
    /// `stream` owns the allocation; returns no value when no buffer is available.
    pub(super) fn take(stream: &'a Stream) -> Option<Self> {
        let mut raw = NonNull::new(unsafe { stream.dequeue_raw_buffer() })?;
        unsafe {
            raw.as_mut().size = 0;
        }
        Some(Self {
            raw,
            stream,
            chunk: None,
            stride: 0,
            frames: 0,
            queued: false,
        })
    }

    /// Borrow the checked interleaved output region for this process callback.
    /// `channels` and `sample_bytes` are negotiated format sizes. Returns the
    /// requested frame count and its mapped bytes; no allocation or copy occurs.
    pub(super) fn active(
        &mut self,
        channels: usize,
        sample_bytes: usize,
    ) -> Result<(usize, &mut [u8]), Error> {
        let raw = unsafe { self.raw.as_ref() };
        let spa = NonNull::new(raw.buffer).ok_or(ErrorKind::InvalidInput)?;
        let spa = unsafe { spa.as_ref() };
        if spa.n_datas != 1 {
            return Err(ErrorKind::InvalidInput.into());
        }
        let data = NonNull::new(spa.datas).ok_or(ErrorKind::InvalidInput)?;
        let data = unsafe { data.as_ref() };
        let mut chunk = NonNull::new(data.chunk).ok_or(ErrorKind::InvalidInput)?;
        unsafe {
            chunk.as_mut().size = 0;
        }
        self.chunk = Some(chunk);
        let samples = NonNull::new(data.data.cast::<u8>()).ok_or(ErrorKind::InvalidInput)?;
        let (stride, frames, bytes) = layout(raw.requested, data.maxsize, channels, sample_bytes)?;
        if samples.as_ptr() as usize % sample_bytes != 0 {
            return Err(ErrorKind::InvalidInput.into());
        }
        self.stride = stride;
        self.frames = frames;
        Ok((frames, unsafe {
            slice::from_raw_parts_mut(samples.as_ptr(), bytes)
        }))
    }

    /// Queue exactly the frames written by the completed application callback.
    /// Consumes the native buffer and returns any submission failure. Queue time
    /// uses frames, matching the stream sample rate used by timestamp conversion.
    pub(super) fn submit(mut self) -> Result<(), Error> {
        let mut chunk = self.chunk.ok_or(ErrorKind::InvalidInput)?;
        unsafe {
            let chunk = chunk.as_mut();
            chunk.offset = 0;
            chunk.stride = self.stride as i32;
            chunk.size = (self.frames * self.stride) as u32;
            self.raw.as_mut().size = self.frames as u64;
        }
        self.queued = true;
        if unsafe { pw::sys::pw_stream_queue_buffer(self.stream.as_raw_ptr(), self.raw.as_ptr()) }
            < 0
        {
            return Err(ErrorKind::BackendError.into());
        }
        Ok(())
    }
}

/// Bound requested interleaved frames by the mapped native capacity.
/// Takes requested frames, capacity bytes and negotiated channel/sample sizes.
/// Returns stride, frame count and byte count, rejecting invalid geometry.
fn layout(
    requested: u64,
    capacity: u32,
    channels: usize,
    sample_bytes: usize,
) -> Result<(usize, usize, usize), Error> {
    let stride = channels
        .checked_mul(sample_bytes)
        .filter(|n| *n > 0 && *n <= i32::MAX as usize)
        .ok_or(ErrorKind::InvalidInput)?;
    let capacity = capacity as usize / stride;
    let frames = if requested == 0 {
        capacity
    } else {
        usize::try_from(requested.min(capacity as u64)).map_err(|_| ErrorKind::InvalidInput)?
    };
    let bytes = frames
        .checked_mul(stride)
        .filter(|n| *n > 0 && *n <= isize::MAX as usize)
        .ok_or(ErrorKind::InvalidInput)?;
    Ok((stride, frames, bytes))
}

impl Drop for OutputBuffer<'_> {
    fn drop(&mut self) {
        if !self.queued {
            unsafe {
                self.stream.queue_raw_buffer(self.raw.as_ptr());
            }
        }
    }
}

/// Convert one captured graph clock and stream queue to output presentation time.
/// `now` is the report timestamp; graph delay uses `num/denom` seconds per tick,
/// while queued/buffered frames use `sample_rate`. Returns checked nanoseconds.
pub(super) fn presentation(
    now: i64,
    delay: i64,
    num: u32,
    denom: u32,
    queued: u64,
    buffered: u64,
    sample_rate: u32,
) -> Option<(u64, u64)> {
    if now <= 0 || num == 0 || denom == 0 || sample_rate == 0 {
        return None;
    }
    let graph = u128::try_from(delay.max(0))
        .ok()?
        .checked_mul(u128::from(num))?
        .checked_mul(1_000_000_000)?
        / u128::from(denom);
    let stream = (u128::from(queued) + u128::from(buffered)).checked_mul(1_000_000_000)?
        / u128::from(sample_rate);
    let start = u64::try_from(now).ok()?;
    let playback =
        u64::try_from(u128::from(start).checked_add(graph)?.checked_add(stream)?).ok()?;
    Some((start, playback))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_geometry_preserves_whole_frames_and_bounds_untrusted_requests() {
        assert_eq!(layout(0, 33, 2, 4).unwrap(), (8, 4, 32));
        assert_eq!(layout(2, 33, 2, 4).unwrap(), (8, 2, 16));
        assert_eq!(layout(u64::MAX, 33, 2, 4).unwrap(), (8, 4, 32));
        for (capacity, channels, bytes) in [
            (0, 2, 4),
            (7, 2, 4),
            (32, 0, 4),
            (32, 2, 0),
            (32, usize::MAX, 4),
        ] {
            assert_eq!(
                layout(1, capacity, channels, bytes).unwrap_err().kind(),
                ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn graph_and_stream_queues_use_their_own_rates_without_float_rounding() {
        assert_eq!(
            presentation(1_000_000_000, 480, 1, 48000, 441, 441, 44100),
            Some((1_000_000_000, 1_030_000_000))
        );
        assert_eq!(
            presentation(1_000_000_000, 240, 2, 48000, 0, 0, 44100),
            Some((1_000_000_000, 1_010_000_000))
        );
        assert_eq!(
            presentation(1, -480, 1, 48000, 1, 0, 48000),
            Some((1, 20834))
        );
        assert_eq!(presentation(1, 1, 1, 44100, 1, 0, 48000), Some((1, 43509)));
    }

    #[test]
    fn invalid_clocks_and_overflow_cannot_become_plausible_timestamps() {
        for (now, num, denom, rate) in [
            (0, 1, 48000, 48000),
            (-1, 1, 48000, 48000),
            (1, 0, 48000, 48000),
            (1, 1, 0, 48000),
            (1, 1, 48000, 0),
        ] {
            assert!(presentation(now, 0, num, denom, 0, 0, rate).is_none());
        }
        assert!(presentation(i64::MAX, i64::MAX, u32::MAX, 1, u64::MAX, u64::MAX, 1).is_none());
    }
}
