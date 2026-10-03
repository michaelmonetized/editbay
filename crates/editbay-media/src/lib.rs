//! Local-file decode and lossless prototype encoding through a narrow FFmpeg adapter.

#[allow(unsafe_code)]
mod ffi;

use serde::Serialize;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("FFmpeg: {0}")]
    Codec(String),
    #[error("invalid media request: {0}")]
    Invalid(String),
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[repr(C)]
pub struct MediaInfo {
    pub width: i32,
    pub height: i32,
    pub rate_num: i32,
    pub rate_den: i32,
    pub sample_rate: i32,
    pub channels: i32,
    pub duration_ns: i64,
    pub color_primaries: i32,
    pub color_transfer: i32,
    pub color_matrix: i32,
    pub color_range: i32,
}

#[derive(Debug)]
pub struct DecodedFrame {
    pub timestamp_ns: Option<i64>,
    pub rgba: Vec<u8>,
}

pub struct VideoReader {
    inner: ffi::Reader,
    pub info: MediaInfo,
}

impl VideoReader {
    /// Open local picture media.
    /// `path` is a regular local file. Returns a bounded CPU decoder and stream profile.
    pub fn open(path: &Path) -> Result<Self> {
        let (inner, info) = ffi::Reader::open(path, false)?;
        Ok(Self { inner, info })
    }

    /// Decode one source picture.
    /// Returns timestamped full-range RGBA8 pixels, or `None` at end of stream.
    pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>> {
        let size = self.info.width as usize * self.info.height as usize * 4;
        let mut rgba = vec![0; size];
        let (length, timestamp_ns) = self.inner.next(&mut rgba)?;
        if length == 0 {
            return Ok(None);
        }
        if length != size {
            return Err(Error::Invalid("decoded picture size changed".into()));
        }
        Ok(Some(DecodedFrame { timestamp_ns, rgba }))
    }
}

pub struct AudioReader {
    inner: ffi::Reader,
    pub info: MediaInfo,
}

impl AudioReader {
    /// Open local sound media.
    /// `path` is a regular local file. Returns stereo 48 kHz float decoding.
    pub fn open(path: &Path) -> Result<Self> {
        let (inner, info) = ffi::Reader::open(path, true)?;
        Ok(Self { inner, info })
    }

    /// Decode one sound block.
    /// Returns interleaved stereo float samples, or `None` after draining the resampler.
    pub fn next_samples(&mut self) -> Result<Option<Vec<f32>>> {
        let mut samples = vec![0.0; 2 * 65_536];
        let length = self.inner.next_audio(&mut samples)?;
        if length == 0 {
            return Ok(None);
        }
        if !length.is_multiple_of(8) {
            return Err(Error::Invalid("unaligned audio block".into()));
        }
        samples.truncate(length / 4);
        Ok(Some(samples))
    }
}

pub use ffi::VideoWriter;

/// Identify the linked codec runtime.
/// Returns the FFmpeg runtime version used by this binary.
pub fn version() -> String {
    ffi::version()
}
