//! Local-file decode and lossless prototype encoding through a narrow FFmpeg adapter.

mod codec_process;
mod delivery;
#[allow(unsafe_code)]
mod ffi;
pub mod native_job;
mod pcm;
pub mod pcm_worker;
pub mod picture_worker;
mod pictures;
#[allow(unsafe_code)]
mod planes;
mod source;
pub mod worker;

pub use delivery::LosslessMovProfile;
pub use ffi::LosslessMovWriter;

pub use pcm::{NativePcmCache, PcmBlock, PcmBudget, PcmProvider, PcmResult, PcmStats};
pub use pictures::{
    DecodedPicture, PictureBudget, PictureCache, PictureCacheStats, PictureProvider, PictureResult,
};
pub use source::{
    AudioBlock, Cancellation, IngestedSource, MediaProbe, NativeAudioReader, SourceFile,
    SourceFingerprint, StreamProfile, StreamType, VideoIndex,
};

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
    #[error("media operation cancelled")]
    Cancelled,
    #[error("source changed during media work: {0}")]
    SourceChanged(String),
    #[error("document: {0}")]
    Document(#[from] editbay_core::Error),
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
    pub source_tick: Option<i64>,
    pub duration_ticks: Option<u64>,
    pub rgba: Vec<u8>,
    pub color: editbay_core::SourceColor,
    pub alpha: editbay_core::AlphaMode,
    pub alpha_interpretation_required: bool,
    pub rotation_degrees: f64,
}

pub(crate) struct PictureMetadata {
    pub(crate) timestamp_ns: Option<i64>,
    pub(crate) source_tick: Option<i64>,
    pub(crate) duration_ticks: Option<u64>,
    pub(crate) color: editbay_core::SourceColor,
    pub(crate) alpha: editbay_core::AlphaMode,
    pub(crate) alpha_interpretation_required: bool,
    pub(crate) rotation_degrees: f64,
}
impl PictureMetadata {
    fn with_pixels(self, rgba: Vec<u8>) -> DecodedFrame {
        DecodedFrame {
            rgba,
            timestamp_ns: self.timestamp_ns,
            source_tick: self.source_tick,
            duration_ticks: self.duration_ticks,
            color: self.color,
            alpha: self.alpha,
            alpha_interpretation_required: self.alpha_interpretation_required,
            rotation_degrees: self.rotation_degrees,
        }
    }
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
        Ok(self
            .read_picture(&mut rgba, None)?
            .map(|details| details.with_pixels(rgba)))
    }

    /// Decode one picture into an exclusively owned, geometry-checked output.
    /// `output` supplies allocated RGBA bytes; `tick` optionally requests an exact
    /// seek. Returns actual metadata or EOF, without converting skipped pictures.
    pub(crate) fn read_picture(
        &mut self,
        output: &mut [u8],
        tick: Option<i64>,
    ) -> Result<Option<PictureMetadata>> {
        let size = self.info.width as usize * self.info.height as usize * 4;
        if output.len() != size {
            return Err(Error::Invalid(
                "decode output does not match source geometry".into(),
            ));
        }
        let (length, details) = if let Some(tick) = tick {
            self.inner.picture_at(tick, output)?
        } else {
            self.inner.next(output)?
        };
        if length == 0 {
            return Ok(None);
        }
        if length != size {
            return Err(Error::Invalid("decoded picture size changed".into()));
        }
        let source_tick = (details.pts != i64::MIN).then_some(details.pts);
        let timestamp_ns = source_tick
            .map(|tick| {
                i64::try_from(
                    i128::from(tick) * i128::from(self.inner.stream.base_num) * 1_000_000_000
                        / i128::from(self.inner.stream.base_den),
                )
                .map_err(|_| Error::Invalid("picture timestamp overflow".into()))
            })
            .transpose()?;
        Ok(Some(PictureMetadata {
            timestamp_ns,
            source_tick,
            duration_ticks: u64::try_from(details.duration)
                .ok()
                .filter(|value| *value > 0),
            color: source::resolve_color(
                details,
                editbay_core::SourceColor {
                    primaries: self.info.color_primaries,
                    transfer: self.info.color_transfer,
                    matrix: self.info.color_matrix,
                    range: self.info.color_range,
                },
            ),
            alpha: source::alpha(details.has_alpha, details.alpha_mode),
            alpha_interpretation_required: details.has_alpha != 0 && details.alpha_mode == 0,
            rotation_degrees: self.inner.stream.rotation,
        }))
    }

    /// Advance one exact indexed picture without allocating or converting RGBA.
    /// `tick` is the expected next presentation timestamp. Returns after native
    /// decode, or an error when EOF, cancellation or a changed index intervenes.
    pub(crate) fn skip_picture(&mut self, tick: i64) -> Result<()> {
        match self.inner.next_timing()? {
            Some(frame) if frame.pts != i64::MIN && frame.pts == tick => Ok(()),
            _ => Err(Error::Invalid(
                "skipped native picture differs from the exact captured index".into(),
            )),
        }
    }

    /// Select a picture stream from an owned local source.
    /// `source` retains the validated descriptor, `stream` is its explicit index,
    /// and `cancel` interrupts native IO/decode. Returns an RGBA8 CPU reader.
    pub fn open_stream(source: &SourceFile, stream: u32, cancel: Cancellation) -> Result<Self> {
        let (inner, info) =
            ffi::Reader::from_file(source.file(), source.path(), Some(stream), 0, cancel)?;
        if inner.stream.base_num <= 0 || inner.stream.base_den <= 0 {
            return Err(Error::Invalid(
                "picture stream has no valid time base".into(),
            ));
        }
        Ok(Self { inner, info })
    }

    /// Seek to a source keyframe at or before a presentation tick.
    /// `tick` uses the selected stream's time base. Returns after flushing all
    /// delayed decoder state; callers decode forward to the requested picture.
    pub fn seek(&mut self, tick: i64) -> Result<()> {
        self.inner.seek(tick)
    }

    /// Decode the exact indexed picture after a source seek.
    /// `tick` is a previously indexed presentation timestamp. Returns its
    /// picture or an error rather than substituting a neighbouring frame.
    pub fn frame_at(&mut self, tick: i64) -> Result<DecodedFrame> {
        let size = self.info.width as usize * self.info.height as usize * 4;
        let mut rgba = vec![0; size];
        let details = self.read_picture(&mut rgba, Some(tick))?.ok_or_else(|| {
            Error::Invalid(format!("indexed picture at tick {tick} is unavailable"))
        })?;
        if details.source_tick != Some(tick) {
            return Err(Error::Invalid("indexed picture timestamp differs".into()));
        }
        Ok(details.with_pixels(rgba))
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
        let (length, _) = self.inner.next_audio(&mut samples)?;
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
