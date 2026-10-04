use crate::{Error, MediaInfo, Result, ffi};
use editbay_core::{
    AlphaMode, AssetKind, AssetReference, DocumentCommand, FrameRate, MediaSource, PictureTiming,
    SourceColor, SourceStream, StreamFormat, TimeBase,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{File, OpenOptions},
    os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Cancellation(pub(crate) ffi::Cancel);

impl Cancellation {
    /// Create one shared native cancellation token.
    /// Takes no arguments. Returns a token whose clones interrupt the same work.
    pub fn new() -> Result<Self> {
        Ok(Self(ffi::Cancel::new()?))
    }

    /// Interrupt the native operation that owns this token.
    /// Takes no arguments. Returns immediately; cancellation is permanent.
    pub fn cancel(&self) {
        self.0.request();
    }

    /// Inspect cancellation without blocking.
    /// Takes no arguments. Returns whether any owner requested cancellation.
    pub fn is_cancelled(&self) -> bool {
        self.0.requested()
    }

    fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFingerprint {
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    device: u64,
    inode: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl FileStamp {
    fn of(file: &File) -> Result<Self> {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(Error::Invalid("source must be a local regular file".into()));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

pub struct SourceFile {
    file: File,
    path: PathBuf,
    fingerprint: SourceFingerprint,
    stamp: FileStamp,
}

impl SourceFile {
    /// Own and checksum a user-selected local media file.
    /// `path` may resolve through a user-selected symlink; `cancel` interrupts
    /// hashing. Returns a canonical path and a descriptor retained through decode.
    pub fn open(path: &Path, cancel: &Cancellation) -> Result<Self> {
        cancel.check()?;
        let path = std::fs::canonicalize(path)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        let stamp = FileStamp::of(&file)?;
        let sha256 = hash(&file, cancel)?;
        let result = Self {
            file,
            path,
            fingerprint: SourceFingerprint {
                sha256,
                bytes: stamp.bytes,
            },
            stamp,
        };
        result.check_identity()?;
        Ok(result)
    }

    /// Identify the exact source content assigned to this handle.
    /// Takes no arguments. Returns its SHA-256 and byte count.
    pub fn fingerprint(&self) -> &SourceFingerprint {
        &self.fingerprint
    }

    /// Locate the canonical user-selected source.
    /// Takes no arguments. Returns a borrowed path; native decode uses the descriptor.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn file(&self) -> &File {
        &self.file
    }

    fn check_identity(&self) -> Result<()> {
        let current = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
            .map_err(|_| Error::SourceChanged(self.path.display().to_string()))?;
        if FileStamp::of(&current)? != self.stamp || FileStamp::of(&self.file)? != self.stamp {
            return Err(Error::SourceChanged(self.path.display().to_string()));
        }
        Ok(())
    }

    /// Check retained file ownership between decoded-frame requests.
    /// `cancel` interrupts the job. Returns success only while pathname, inode,
    /// byte count and modification/change stamps match the full-hash preflight.
    /// Final job publication still requires `verify` to check all bytes again.
    pub fn check_current(&self, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        self.check_identity()
    }

    /// Recheck both pathname ownership and all source bytes.
    /// `cancel` interrupts hashing. Returns success only if the source still
    /// matches the initial descriptor, metadata and complete checksum.
    pub fn verify(&self, cancel: &Cancellation) -> Result<()> {
        self.check_identity()?;
        if hash(&self.file, cancel)? != self.fingerprint.sha256 {
            return Err(Error::SourceChanged(self.path.display().to_string()));
        }
        self.check_identity()
    }

    /// Inspect every container stream without choosing one implicitly.
    /// `cancel` interrupts native probing. Returns bounded metadata and exact
    /// stream timing/geometry/channel order; unsupported stream kinds stay visible.
    pub fn probe(&self, cancel: Cancellation) -> Result<MediaProbe> {
        self.check_identity()?;
        let probe = ffi::Probe::open(&self.file, &self.path, cancel.clone())?;
        let mut streams = Vec::new();
        for index in 0..probe.streams()? {
            cancel.check()?;
            let (raw, codec) = probe.info(index)?;
            let mut metadata = probe.metadata(index as i32)?;
            if !raw.rotation.is_finite() {
                return Err(Error::Invalid("nonfinite source display rotation".into()));
            }
            metadata.insert(
                "editbay.display_rotation_degrees".into(),
                raw.rotation.to_string(),
            );
            let kind = match raw.kind {
                0 => StreamType::Video,
                1 => StreamType::Audio,
                2 => StreamType::Data,
                3 => StreamType::Subtitle,
                4 => StreamType::Attachment,
                _ => StreamType::Other,
            };
            let channels = if kind == StreamType::Audio {
                probe.channels(index, raw.channels)?
            } else {
                Vec::new()
            };
            streams.push(StreamProfile {
                index: index as u32,
                kind,
                codec,
                time_base: time_base(raw.base_num, raw.base_den).ok(),
                start_tick: known(raw.start_tick),
                duration_ticks: positive(raw.duration_ticks),
                width: u32::try_from(raw.width).ok().filter(|v| *v > 0),
                height: u32::try_from(raw.height).ok().filter(|v| *v > 0),
                nominal_rate: frame_rate(raw.rate_num, raw.rate_den),
                sample_aspect: frame_rate(raw.aspect_num, raw.aspect_den),
                sample_rate: u32::try_from(raw.sample_rate).ok().filter(|v| *v > 0),
                channels,
                channel_order_declared: raw.channel_order != 0,
                pixel_format: raw.pixel_format,
                color: color(
                    raw.color_primaries,
                    raw.color_transfer,
                    raw.color_matrix,
                    raw.color_range,
                ),
                alpha: alpha(raw.has_alpha, raw.alpha_mode),
                alpha_interpretation_required: raw.has_alpha != 0 && raw.alpha_mode == 0,
                disposition: raw.disposition,
                decoder_available: raw.decoder_available != 0,
                rotation_degrees: raw.rotation,
                metadata,
            });
        }
        self.check_identity()?;
        Ok(MediaProbe {
            path: self.path.clone(),
            fingerprint: self.fingerprint.clone(),
            codec_runtime: crate::version(),
            metadata: probe.metadata(-1)?,
            streams,
        })
    }

    /// Decode the presentation index of one explicit video stream.
    /// `stream` selects the picture stream, `cancel` interrupts native decoding,
    /// and `progress` receives completed picture counts. Returns ordered source
    /// ticks and an exclusive end; average rate never replaces VFR timestamps.
    pub fn index_video(
        &self,
        stream: &StreamProfile,
        cancel: Cancellation,
        mut progress: impl FnMut(u64),
    ) -> Result<VideoIndex> {
        if stream.kind != StreamType::Video {
            return Err(Error::Invalid("selected stream is not video".into()));
        }
        let base = stream
            .time_base
            .ok_or_else(|| Error::Invalid("video stream has no time base".into()))?;
        let (mut reader, _) = ffi::Reader::from_file(
            &self.file,
            &self.path,
            Some(stream.index),
            0,
            cancel.clone(),
        )?;
        let mut ticks = Vec::new();
        let mut last_duration = None;
        let mut interpretation = None;
        while let Some(frame) = reader.next_timing()? {
            cancel.check()?;
            let tick = known(frame.pts).ok_or_else(|| Error::Invalid("video picture has no presentation timestamp; choose an explicit interpretation".into()))?;
            if ticks.last().is_some_and(|previous| *previous >= tick) {
                return Err(Error::Invalid(
                    "video presentation timestamps repeat or move backwards".into(),
                ));
            }
            if ticks.len() >= 500_000 {
                return Err(Error::Invalid(
                    "video exceeds the 500,000-picture index budget".into(),
                ));
            }
            let resolved_color = resolve_color(frame, stream.color);
            let resolved_alpha = alpha(frame.has_alpha, frame.alpha_mode);
            let current = (
                resolved_color,
                resolved_alpha,
                frame.has_alpha != 0 && frame.alpha_mode == 0,
            );
            if interpretation.is_some_and(|old| old != current) {
                return Err(Error::Invalid(
                    "video color or alpha interpretation changes within the stream".into(),
                ));
            }
            interpretation = Some(current);
            ticks.push(tick);
            last_duration = positive(frame.duration);
            if ticks.len().is_multiple_of(32) {
                progress(ticks.len() as u64);
            }
        }
        let first = *ticks
            .first()
            .ok_or_else(|| Error::Invalid("video stream contains no decoded pictures".into()))?;
        let last = *ticks.last().unwrap();
        let (end_tick, end_method) = if let Some(duration) = last_duration {
            (add_tick(last, duration)?, "decoded_frame_duration")
        } else if let Some(end) = stream
            .start_tick
            .zip(stream.duration_ticks)
            .map(|(start, duration)| add_tick(start, duration))
            .transpose()?
            .filter(|end| *end > last)
        {
            (end, "stream_duration")
        } else if let Some(rate) = stream.nominal_rate {
            let numerator = u128::from(rate.denominator) * u128::from(base.denominator);
            let denominator = u128::from(rate.numerator) * u128::from(base.numerator);
            let duration = u64::try_from(numerator.div_ceil(denominator))
                .map_err(|_| Error::Invalid("video duration overflow".into()))?;
            (
                add_tick(last, duration.max(1))?,
                "declared_nominal_last_frame_duration",
            )
        } else {
            return Err(Error::Invalid(
                "last video picture has no known duration; choose an explicit interpretation"
                    .into(),
            ));
        };
        let (color, alpha, alpha_interpretation_required) = interpretation.unwrap();
        progress(ticks.len() as u64);
        cancel.check()?;
        self.check_identity()?;
        Ok(VideoIndex {
            start_tick: first,
            presentation_ticks: ticks,
            end_tick,
            end_method: end_method.into(),
            color,
            alpha,
            alpha_interpretation_required,
        })
    }

    /// Build real source document data from explicitly selected streams.
    /// `name` labels the source, `selected` contains unique container indices,
    /// `cancel` reaches hashing/native decode and `progress` reports actual work.
    /// Returns editable asset/source commands after a final complete-byte recheck.
    pub fn ingest(
        &self,
        name: String,
        selected: &[u32],
        cancel: Cancellation,
        mut progress: impl FnMut(u32, u64),
    ) -> Result<IngestedSource> {
        if selected.is_empty()
            || selected.len() > 256
            || selected.iter().copied().collect::<HashSet<_>>().len() != selected.len()
        {
            return Err(Error::Invalid(
                "choose 1..256 distinct media streams".into(),
            ));
        }
        let probe = self.probe(cancel.clone())?;
        let mut streams = Vec::new();
        for index in selected {
            let stream = probe
                .streams
                .iter()
                .find(|stream| stream.index == *index)
                .ok_or_else(|| Error::Invalid(format!("stream {index} is absent")))?;
            if !stream.decoder_available {
                return Err(Error::Invalid(format!(
                    "stream {index} has no installed native decoder"
                )));
            }
            let mut metadata = stream.metadata.clone();
            let (time_base, start_tick, duration_ticks, format) = match stream.kind {
                StreamType::Video => {
                    let video =
                        self.index_video(stream, cancel.clone(), |count| progress(*index, count))?;
                    metadata.insert(
                        "editbay.last_frame_duration".into(),
                        video.end_method.clone(),
                    );
                    if video.alpha_interpretation_required {
                        metadata.insert(
                            "editbay.alpha_interpretation".into(),
                            "unspecified source alpha interpreted as straight".into(),
                        );
                    }
                    let base = stream.time_base.unwrap();
                    (
                        base,
                        video.start_tick,
                        Some(
                            u64::try_from(
                                i128::from(video.end_tick) - i128::from(video.start_tick),
                            )
                            .map_err(|_| Error::Invalid("video extent overflow".into()))?,
                        ),
                        StreamFormat::Video {
                            width: stream
                                .width
                                .ok_or_else(|| Error::Invalid("video has no width".into()))?,
                            height: stream
                                .height
                                .ok_or_else(|| Error::Invalid("video has no height".into()))?,
                            sample_aspect: stream.sample_aspect.unwrap_or(FrameRate {
                                numerator: 1,
                                denominator: 1,
                            }),
                            timing: PictureTiming::Variable {
                                presentation_ticks: video.presentation_ticks,
                                end_tick: video.end_tick,
                            },
                            color: video.color,
                            alpha: video.alpha,
                        },
                    )
                }
                StreamType::Audio => {
                    let mut reader = NativeAudioReader::open_stream(self, *index, cancel.clone())?;
                    let mut start = None;
                    let mut end = None;
                    let mut count = 0u64;
                    while let Some(block) = reader.next_block()? {
                        let first = block.first_sample.ok_or_else(|| {
                            Error::Invalid("audio has no presentation timestamp".into())
                        })?;
                        start.get_or_insert(first);
                        if end.is_some_and(|previous| previous != first) {
                            return Err(Error::Invalid("audio presentation has a gap or overlap; repair its interpretation before ingest".into()));
                        }
                        let frames = block.samples.len() / reader.info.channels as usize;
                        end = Some(add_tick(first, frames as u64)?);
                        count = count
                            .checked_add(frames as u64)
                            .ok_or_else(|| Error::Invalid("audio sample count overflow".into()))?;
                        progress(*index, count);
                    }
                    let first = start.ok_or_else(|| {
                        Error::Invalid("audio stream has no decoded samples".into())
                    })?;
                    let last = end.unwrap();
                    metadata.insert(
                        "editbay.original_time_base".into(),
                        format!(
                            "{}/{}",
                            reader.time_base.numerator, reader.time_base.denominator
                        ),
                    );
                    metadata.insert(
                        "editbay.timing".into(),
                        "decoded native sample boundaries".into(),
                    );
                    (
                        TimeBase {
                            numerator: 1,
                            denominator: reader.info.sample_rate as u32,
                        },
                        first,
                        Some(
                            u64::try_from(i128::from(last) - i128::from(first))
                                .map_err(|_| Error::Invalid("audio extent overflow".into()))?,
                        ),
                        StreamFormat::Audio {
                            sample_rate: reader.info.sample_rate as u32,
                            channels: stream.channels.clone(),
                        },
                    )
                }
                _ => {
                    return Err(Error::Invalid(format!(
                        "stream {index} is {:?}; only picture and sound streams can be ingested",
                        stream.kind
                    )));
                }
            };
            streams.push(SourceStream {
                index: *index,
                codec: stream.codec.clone(),
                time_base,
                start_tick,
                duration_ticks,
                format,
                metadata,
            });
        }
        let asset = AssetReference {
            id: Uuid::new_v4(),
            kind: AssetKind::Media,
            path: self.path.clone(),
            sha256: self.fingerprint.sha256.clone(),
            bytes: self.fingerprint.bytes,
            provenance: format!(
                "native FFmpeg {} ingest; source bytes preserved",
                probe.codec_runtime
            ),
        };
        let source = MediaSource {
            id: Uuid::new_v4(),
            name,
            asset: asset.id,
            streams,
            metadata: probe.metadata,
        };
        let mut validation = editbay_core::Project::new("Ingest validation")?;
        validation.assets.push(asset.clone());
        validation.sources.push(source.clone());
        validation.validate()?;
        self.verify(&cancel)?;
        Ok(IngestedSource { asset, source })
    }
}

fn hash(file: &File, cancel: &Cancellation) -> Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    let mut offset = 0;
    let length = file.metadata()?.len();
    while offset < length {
        cancel.check()?;
        let capacity = usize::try_from((length - offset).min(buffer.len() as u64)).unwrap();
        let count = file.read_at(&mut buffer[..capacity], offset)?;
        if count == 0 {
            return Err(Error::SourceChanged(
                "opened source was truncated while hashing".into(),
            ));
        }
        digest.update(&buffer[..count]);
        offset = offset
            .checked_add(count as u64)
            .ok_or_else(|| Error::Invalid("source byte offset overflow".into()))?;
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn time_base(num: i32, den: i32) -> Result<TimeBase> {
    if num <= 0 || den <= 0 {
        return Err(Error::Invalid("source time base must be positive".into()));
    }
    Ok(TimeBase {
        numerator: num as u32,
        denominator: den as u32,
    })
}

fn frame_rate(num: i32, den: i32) -> Option<FrameRate> {
    (num > 0 && den > 0).then_some(FrameRate {
        numerator: num as u32,
        denominator: den as u32,
    })
}

fn known(value: i64) -> Option<i64> {
    (value != i64::MIN).then_some(value)
}
fn positive(value: i64) -> Option<u64> {
    u64::try_from(value).ok().filter(|v| *v > 0)
}
fn add_tick(tick: i64, duration: u64) -> Result<i64> {
    i64::try_from(i128::from(tick) + i128::from(duration))
        .map_err(|_| Error::Invalid("source timing overflow".into()))
}
fn color(primaries: i32, transfer: i32, matrix: i32, range: i32) -> SourceColor {
    SourceColor {
        primaries,
        transfer,
        matrix,
        range,
    }
}
fn resolve_color(frame: ffi::RawFrame, fallback: SourceColor) -> SourceColor {
    color(
        if frame.color_primaries == 2 {
            fallback.primaries
        } else {
            frame.color_primaries
        },
        if frame.color_transfer == 2 {
            fallback.transfer
        } else {
            frame.color_transfer
        },
        if frame.color_matrix == 2 {
            fallback.matrix
        } else {
            frame.color_matrix
        },
        if frame.color_range == 0 {
            fallback.range
        } else {
            frame.color_range
        },
    )
}
fn alpha(present: i32, mode: i32) -> AlphaMode {
    if present == 0 {
        AlphaMode::Opaque
    } else if mode == 1 {
        AlphaMode::Premultiplied
    } else {
        AlphaMode::Straight
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamType {
    Video,
    Audio,
    Subtitle,
    Data,
    Attachment,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamProfile {
    pub index: u32,
    pub kind: StreamType,
    pub codec: String,
    pub time_base: Option<TimeBase>,
    pub start_tick: Option<i64>,
    pub duration_ticks: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub nominal_rate: Option<FrameRate>,
    pub sample_aspect: Option<FrameRate>,
    pub sample_rate: Option<u32>,
    pub channels: Vec<String>,
    pub channel_order_declared: bool,
    pub pixel_format: i32,
    pub color: SourceColor,
    pub alpha: AlphaMode,
    pub alpha_interpretation_required: bool,
    pub disposition: i32,
    pub decoder_available: bool,
    pub rotation_degrees: f64,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaProbe {
    pub path: PathBuf,
    pub fingerprint: SourceFingerprint,
    pub codec_runtime: String,
    pub metadata: BTreeMap<String, String>,
    pub streams: Vec<StreamProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoIndex {
    pub start_tick: i64,
    pub presentation_ticks: Vec<i64>,
    pub end_tick: i64,
    pub end_method: String,
    pub color: SourceColor,
    pub alpha: AlphaMode,
    pub alpha_interpretation_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestedSource {
    pub asset: AssetReference,
    pub source: MediaSource,
}

impl IngestedSource {
    /// Insert this checked media through ordinary reversible document commands.
    /// Takes no arguments. Returns the asset before its dependent source.
    pub fn commands(&self) -> [DocumentCommand; 2] {
        [
            DocumentCommand::SetAsset {
                asset: self.asset.clone(),
            },
            DocumentCommand::SetSource {
                source: self.source.clone(),
            },
        ]
    }
}

#[derive(Debug)]
pub struct AudioBlock {
    pub source_tick: Option<i64>,
    pub first_sample: Option<i64>,
    pub samples: Vec<f32>,
}

pub struct NativeAudioReader {
    inner: ffi::Reader,
    pub info: MediaInfo,
    pub time_base: TimeBase,
    next_sample: Option<i64>,
    presentation_start: Option<i64>,
    presentation_end: Option<i64>,
}

impl NativeAudioReader {
    /// Decode the original rate and channels of an explicit sound stream.
    /// `source` owns the file, `stream` selects the container index and `cancel`
    /// reaches native IO. Returns interleaved float samples in source channel order.
    pub fn open_stream(source: &SourceFile, stream: u32, cancel: Cancellation) -> Result<Self> {
        let (inner, info) =
            ffi::Reader::from_file(source.file(), source.path(), Some(stream), 2, cancel)?;
        let time_base = time_base(inner.stream.base_num, inner.stream.base_den)?;
        let sample_rate = FrameRate {
            numerator: info.sample_rate as u32,
            denominator: 1,
        };
        let presentation_start = known(inner.stream.start_tick)
            .map(|tick| {
                time_base.boundary(
                    editbay_core::SourcePosition {
                        numerator: tick,
                        denominator: 1,
                    },
                    sample_rate,
                )
            })
            .transpose()?;
        let presentation_end = known(inner.stream.start_tick)
            .zip(positive(inner.stream.duration_ticks))
            .map(|(start, duration)| {
                let tick = add_tick(start, duration)?;
                let numerator = i128::from(tick)
                    * i128::from(time_base.numerator)
                    * i128::from(info.sample_rate);
                let denominator = i128::from(time_base.denominator);
                i64::try_from(-(-numerator).div_euclid(denominator))
                    .map_err(|_| Error::Invalid("audio presentation end overflow".into()))
            })
            .transpose()?;
        Ok(Self {
            inner,
            info,
            time_base,
            next_sample: None,
            presentation_start,
            presentation_end,
        })
    }

    /// Decode one bounded source sound block.
    /// Takes no arguments. Returns finite float samples with sample-clock timing;
    /// packet timestamp quantization is normalized only within half a source tick.
    /// Declared container presentation bounds remove codec preroll and padding.
    pub fn next_block(&mut self) -> Result<Option<AudioBlock>> {
        loop {
            let mut samples = vec![0.; self.info.channels as usize * 65_536];
            let (bytes, frame) = self.inner.next_audio(&mut samples)?;
            if bytes == 0 {
                return Ok(None);
            }
            let stride = 4 * self.info.channels as usize;
            if !bytes.is_multiple_of(stride) {
                return Err(Error::Invalid("unaligned native sound block".into()));
            }
            samples.truncate(bytes / 4);
            if samples.iter().any(|v| !v.is_finite()) {
                return Err(Error::Invalid("nonfinite source sound samples".into()));
            }
            let raw = known(frame.sample_start);
            let tolerance = (u64::from(self.time_base.numerator) * self.info.sample_rate as u64)
                .div_ceil(u64::from(self.time_base.denominator) * 2)
                .max(1);
            let first_sample = match (raw, self.next_sample) {
                (Some(actual), Some(expected)) if actual.abs_diff(expected) <= tolerance => {
                    Some(expected)
                }
                (Some(actual), _) => Some(actual),
                (None, expected) => expected,
            };
            self.next_sample = first_sample
                .map(|first| add_tick(first, (bytes / stride) as u64))
                .transpose()?;
            let mut first_sample = first_sample;
            if let Some(first) = first_sample {
                let end = self.next_sample.unwrap();
                let keep_start = self
                    .presentation_start
                    .map_or(first, |start| first.max(start));
                let keep_end = self.presentation_end.map_or(end, |limit| end.min(limit));
                if keep_start >= keep_end {
                    continue;
                }
                let lead = usize::try_from(i128::from(keep_start) - i128::from(first))
                    .map_err(|_| Error::Invalid("audio preroll overflow".into()))?
                    * self.info.channels as usize;
                let length = usize::try_from(i128::from(keep_end) - i128::from(keep_start))
                    .map_err(|_| Error::Invalid("audio presentation length overflow".into()))?
                    * self.info.channels as usize;
                samples.copy_within(lead..lead + length, 0);
                samples.truncate(length);
                first_sample = Some(keep_start);
            }
            return Ok(Some(AudioBlock {
                source_tick: known(frame.pts),
                first_sample,
                samples,
            }));
        }
    }

    /// Reset sound decoding at a source timestamp.
    /// `tick` uses the original container time base. Returns after decoder and
    /// resampler flush; the next block retains its actual source sample boundary.
    pub fn seek(&mut self, tick: i64) -> Result<()> {
        self.inner.seek(tick)?;
        self.next_sample = None;
        Ok(())
    }
}
