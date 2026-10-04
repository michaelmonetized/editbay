use crate::{Error, FrameRate, Project, Result, SourcePosition, TimeBase, TimeMap};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::PathBuf,
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Media,
    Font,
    Artwork,
    Title,
    Lut,
    Mask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AssetReference {
    pub id: Uuid,
    pub kind: AssetKind,
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub provenance: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AlphaMode {
    Opaque,
    Straight,
    Premultiplied,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceColor {
    pub primaries: i32,
    pub transfer: i32,
    pub matrix: i32,
    pub range: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PictureTiming {
    Constant {
        rate: FrameRate,
    },
    Variable {
        presentation_ticks: Vec<i64>,
        end_tick: i64,
    },
}

impl PictureTiming {
    /// Select the picture immediately before a reverse source boundary.
    /// `position`, `base` and `start_tick` use the same stream units as `picture_at`.
    /// Returns no picture before the first frame; an exclusive source end selects
    /// the last picture rather than introducing an empty first reverse frame.
    pub fn picture_before(
        &self,
        position: SourcePosition,
        base: TimeBase,
        start_tick: i64,
    ) -> Result<Option<u64>> {
        if position.compare_tick(start_tick)?.is_le() {
            return Ok(None);
        }
        match self {
            Self::Constant { rate } => {
                let relative = position.relative_to(start_tick)?;
                let negative = SourcePosition::from_fraction(
                    -i128::from(relative.numerator),
                    relative.denominator,
                )?;
                let frame = base
                    .boundary(negative, *rate)?
                    .checked_neg()
                    .and_then(|value| value.checked_sub(1))
                    .ok_or_else(|| Error::Invalid("reverse picture boundary overflow".into()))?;
                Ok(u64::try_from(frame).ok())
            }
            Self::Variable {
                presentation_ticks,
                end_tick,
            } => {
                if position.compare_tick(*end_tick)?.is_gt() {
                    return Ok(None);
                }
                let mut low = 0;
                let mut high = presentation_ticks.len();
                while low < high {
                    let mid = low + (high - low) / 2;
                    if position.compare_tick(presentation_ticks[mid])?.is_gt() {
                        low = mid + 1;
                    } else {
                        high = mid;
                    }
                }
                Ok(low.checked_sub(1).map(|index| index as u64))
            }
        }
    }
    /// Select the picture whose presentation interval owns a source position.
    /// `position` uses the stream's `base`; `start_tick` is its first presentation.
    /// Returns an integer picture ordinal. VFR uses recorded timestamps, never an
    /// average frame rate. Positions outside a VFR extent return no picture.
    pub fn picture_at(
        &self,
        position: SourcePosition,
        base: TimeBase,
        start_tick: i64,
    ) -> Result<Option<u64>> {
        if position.compare_tick(start_tick)?.is_lt() {
            return Ok(None);
        }
        match self {
            Self::Constant { rate } => {
                let numerator = i128::from(position.numerator)
                    - i128::from(start_tick) * i128::from(position.denominator);
                let relative = SourcePosition::from_fraction(numerator, position.denominator)?;
                Ok(u64::try_from(base.boundary(relative, *rate)?).ok())
            }
            Self::Variable {
                presentation_ticks,
                end_tick,
            } => {
                if presentation_ticks.is_empty() || position.compare_tick(*end_tick)?.is_ge() {
                    return Ok(None);
                }
                let mut low = 0;
                let mut high = presentation_ticks.len();
                while low < high {
                    let mid = low + (high - low) / 2;
                    if position.compare_tick(presentation_ticks[mid])?.is_ge() {
                        low = mid + 1;
                    } else {
                        high = mid;
                    }
                }
                Ok(low.checked_sub(1).map(|index| index as u64))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StreamFormat {
    Video {
        width: u32,
        height: u32,
        sample_aspect: FrameRate,
        timing: PictureTiming,
        color: SourceColor,
        alpha: AlphaMode,
    },
    Audio {
        sample_rate: u32,
        channels: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceStream {
    pub index: u32,
    pub codec: String,
    pub time_base: TimeBase,
    pub start_tick: i64,
    pub duration_ticks: Option<u64>,
    pub format: StreamFormat,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MediaSource {
    pub id: Uuid,
    pub name: String,
    pub asset: Uuid,
    pub streams: Vec<SourceStream>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClipSource {
    Media { source: Uuid, stream: u32 },
    Composition { composition: Uuid },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FrameRange {
    pub start: u64,
    pub end: u64,
}

impl FrameRange {
    pub(crate) fn validate(self, duration: u64) -> Result<()> {
        if self.start >= self.end || self.end > duration {
            return Err(Error::Invalid(
                "frame range must be nonempty and within its composition".into(),
            ));
        }
        Ok(())
    }

    /// Check a frame against an exclusive-end range.
    /// `frame` is composition time. Returns whether this range owns that frame.
    pub fn contains(self, frame: u64) -> bool {
        self.start <= frame && frame < self.end
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Clip {
    pub id: Uuid,
    pub name: String,
    pub range: FrameRange,
    pub source: ClipSource,
    pub time_map: TimeMap,
    pub linked: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub id: Uuid,
    pub name: String,
    pub kind: TrackKind,
    pub enabled: bool,
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SocketType {
    Image,
    Mask,
    Geometry,
    Audio,
    Data,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AnimatedProperty {
    Opacity,
    TranslateX,
    TranslateY,
    ScaleX,
    ScaleY,
    Rotation,
    Gain,
    Scalar,
    Feather,
    Red,
    Green,
    Blue,
    Alpha,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Step,
    Linear,
    Hermite,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Keyframe {
    pub frame: u64,
    pub value: f64,
    pub in_tangent: f64,
    pub out_tangent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AnimationChannel {
    pub id: Uuid,
    pub property: AnimatedProperty,
    pub interpolation: Interpolation,
    pub keys: Vec<Keyframe>,
}

impl AnimationChannel {
    /// Evaluate a structured animation channel with held endpoints.
    /// `frame` is composition time. Returns a finite value or an explicit error;
    /// Hermite tangents are value change per composition frame.
    pub fn value_at(&self, frame: u64) -> Result<f64> {
        self.validate_keys()?;
        self.value_fraction(i128::from(frame), 1, false)
    }

    /// Evaluate animation at an exact fractional composition position.
    /// `position` is measured in composition frames. Returns the interpolated
    /// finite parameter; key selection occurs before any floating-point conversion.
    pub fn value_at_position(&self, position: SourcePosition) -> Result<f64> {
        self.validate_keys()?;
        self.value_validated(position, false)
    }

    fn validate_keys(&self) -> Result<()> {
        if self.keys.len() > 1_000_000
            || self
                .keys
                .windows(2)
                .any(|keys| keys[0].frame >= keys[1].frame)
        {
            return Err(Error::Invalid(
                "animation keys must be ordered within their budget".into(),
            ));
        }
        for key in &self.keys {
            finite(key.value)?;
            finite(key.in_tangent)?;
            finite(key.out_tangent)?;
        }
        Ok(())
    }

    /// Evaluate retained keys after document validation.
    /// `position` selects exact frame time; `before` selects the preceding side.
    /// Returns a finite parameter without scanning every key again.
    pub(crate) fn value_validated(&self, position: SourcePosition, before: bool) -> Result<f64> {
        self.value_fraction(i128::from(position.numerator), position.denominator, before)
    }

    /// Resolve a rational time against ordered animation keys.
    /// `numerator`, `denominator` and `before` specify time and boundary ownership.
    /// Returns held, linear or Hermite interpolation with parameter bounds applied.
    fn value_fraction(&self, numerator: i128, denominator: u64, before: bool) -> Result<f64> {
        if denominator == 0 {
            return Err(Error::Invalid("animation time denominator is zero".into()));
        }
        let first = self
            .keys
            .first()
            .ok_or_else(|| Error::Invalid("empty animation channel".into()))?;
        if numerator < 0 || numerator as u128 <= u128::from(first.frame) * u128::from(denominator) {
            return finite(first.value);
        }
        let index = self.keys.partition_point(|key| {
            let tick = u128::from(key.frame) * u128::from(denominator);
            if before {
                tick < numerator as u128
            } else {
                tick <= numerator as u128
            }
        }) - 1;
        let a = &self.keys[index];
        let Some(b) = self.keys.get(index + 1) else {
            return finite(a.value);
        };
        if b.frame <= a.frame {
            return Err(Error::Invalid("animation keys must increase".into()));
        }
        let span = (b.frame - a.frame) as f64;
        let offset = numerator as u128 - u128::from(a.frame) * u128::from(denominator);
        let t = (offset as f64 / denominator as f64) / span;
        let value = finite(match self.interpolation {
            Interpolation::Step => a.value,
            Interpolation::Linear => a.value + (b.value - a.value) * t,
            Interpolation::Hermite => {
                (2.0 * t * t * t - 3.0 * t * t + 1.0) * a.value
                    + (t * t * t - 2.0 * t * t + t) * span * a.out_tangent
                    + (-2.0 * t * t * t + 3.0 * t * t) * b.value
                    + (t * t * t - t * t) * span * b.in_tangent
            }
        })?;
        Ok(match self.property {
            AnimatedProperty::Opacity | AnimatedProperty::Alpha => value.clamp(0.0, 1.0),
            AnimatedProperty::Gain | AnimatedProperty::Feather => value.max(0.0),
            _ => value,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NodeOperation {
    Source {
        clip: Uuid,
    },
    Solid {
        rgba: [f64; 4],
    },
    Transform {
        image: Uuid,
        translation: [f64; 2],
        scale: [f64; 2],
        rotation: f64,
        opacity: f64,
    },
    Over {
        foreground: Uuid,
        background: Uuid,
        mask: Option<Uuid>,
    },
    Polygon {
        points: Vec<[f64; 2]>,
    },
    Mask {
        geometry: Uuid,
        feather: f64,
        inverted: bool,
    },
    MaskAsset {
        asset: Uuid,
    },
    Gain {
        audio: Uuid,
        gain: f64,
    },
    Mix {
        inputs: Vec<Uuid>,
    },
    Scalar {
        value: f64,
    },
    Opacity {
        image: Uuid,
        value: Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TimedNode {
    pub id: Uuid,
    pub range: FrameRange,
    pub operation: NodeOperation,
    pub animation: Vec<AnimationChannel>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FloatPrecision {
    Half,
    #[default]
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WorkingGamut {
    Bt709,
    Bt2020,
    DisplayP3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum OutputTransfer {
    Linear,
    Srgb,
    Bt709,
    Pq,
    Hlg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ColorConfiguration {
    pub working_gamut: WorkingGamut,
    pub precision: FloatPrecision,
    pub display_gamut: WorkingGamut,
    pub display_transfer: OutputTransfer,
    pub output_gamut: WorkingGamut,
    pub output_transfer: OutputTransfer,
}

impl Default for ColorConfiguration {
    fn default() -> Self {
        Self {
            working_gamut: WorkingGamut::Bt709,
            precision: FloatPrecision::Full,
            display_gamut: WorkingGamut::Bt709,
            display_transfer: OutputTransfer::Srgb,
            output_gamut: WorkingGamut::Bt709,
            output_transfer: OutputTransfer::Srgb,
        }
    }
}

impl ColorConfiguration {
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Composition {
    pub id: Uuid,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub duration: u64,
    pub tracks: Vec<Track>,
    pub nodes: Vec<TimedNode>,
    pub picture: Option<Uuid>,
    pub audio: Option<Uuid>,
}

pub(crate) fn named(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::Invalid("entity needs a nonempty name".into()));
    }
    Ok(())
}

fn finite(value: f64) -> Result<f64> {
    if !value.is_finite() || value.abs() > 1.0e12 {
        return Err(Error::Invalid(
            "numeric value is nonfinite or outside its budget".into(),
        ));
    }
    Ok(value)
}

fn metadata(values: &BTreeMap<String, String>) -> Result<()> {
    if values.len() > 128
        || values
            .iter()
            .any(|(key, value)| key.is_empty() || key.len() > 256 || value.len() > 4096)
    {
        return Err(Error::Invalid(
            "metadata exceeds its entry or text budget".into(),
        ));
    }
    Ok(())
}

fn identity(id: Uuid, identities: &mut HashSet<Uuid>) -> Result<()> {
    if id.is_nil() || !identities.insert(id) || identities.len() > 100_000 {
        return Err(Error::Invalid(
            "document identities must be unique, nonempty and within the 100,000 entity budget"
                .into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_model(project: &Project, identities: &mut HashSet<Uuid>) -> Result<()> {
    if identities.len() > 100_000 {
        return Err(Error::Invalid(
            "document exceeds its 100,000 entity budget".into(),
        ));
    }
    let assets: HashMap<_, _> = project
        .assets
        .iter()
        .map(|asset| (asset.id, asset))
        .collect();
    let sources: HashMap<_, _> = project
        .sources
        .iter()
        .map(|source| (source.id, source))
        .collect();
    let compositions: HashMap<_, _> = project
        .compositions
        .iter()
        .map(|composition| (composition.id, composition))
        .collect();
    for asset in &project.assets {
        identity(asset.id, identities)?;
        if asset.path.as_os_str().is_empty()
            || asset.path.as_os_str().as_encoded_bytes().contains(&0)
            || asset.path.as_os_str().len() > 32_768
            || asset.sha256.len() != 64
            || !asset
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || asset.provenance.len() > 4096
        {
            return Err(Error::Invalid(
                "asset needs a local path, lowercase SHA-256 and bounded provenance".into(),
            ));
        }
    }
    for source in &project.sources {
        identity(source.id, identities)?;
        named(&source.name)?;
        metadata(&source.metadata)?;
        if assets
            .get(&source.asset)
            .is_none_or(|asset| asset.kind != AssetKind::Media)
            || source.streams.is_empty()
            || source.streams.len() > 256
        {
            return Err(Error::Invalid(
                "media source needs a media asset and 1..256 streams".into(),
            ));
        }
        let mut indices = HashSet::new();
        for stream in &source.streams {
            if !indices.insert(stream.index) {
                return Err(Error::Invalid(
                    "source stream indices must be unique".into(),
                ));
            }
            named(&stream.codec)?;
            metadata(&stream.metadata)?;
            stream.time_base.validate()?;
            if stream.duration_ticks.is_some_and(|duration| {
                duration == 0
                    || i128::from(stream.start_tick) + i128::from(duration) > i128::from(i64::MAX)
            }) {
                return Err(Error::Invalid("source stream duration is invalid".into()));
            }
            match &stream.format {
                StreamFormat::Video {
                    width,
                    height,
                    sample_aspect,
                    timing,
                    ..
                } => {
                    dimensions(*width, *height)?;
                    sample_aspect.validate()?;
                    match timing {
                        PictureTiming::Constant { rate } => rate.validate()?,
                        PictureTiming::Variable {
                            presentation_ticks,
                            end_tick,
                        } => {
                            if presentation_ticks.is_empty()
                                || presentation_ticks.len() > 10_000_000
                                || presentation_ticks[0] != stream.start_tick
                                || presentation_ticks.windows(2).any(|p| p[0] >= p[1])
                                || presentation_ticks
                                    .last()
                                    .is_none_or(|last| last >= end_tick)
                                || stream.duration_ticks.is_some_and(|duration| {
                                    i128::from(stream.start_tick) + i128::from(duration)
                                        != i128::from(*end_tick)
                                })
                            {
                                return Err(Error::Invalid("VFR needs ordered source timestamps and an exact exclusive end".into()));
                            }
                        }
                    }
                }
                StreamFormat::Audio {
                    sample_rate,
                    channels,
                } => {
                    if *sample_rate == 0
                        || *sample_rate > 768_000
                        || channels.is_empty()
                        || channels.len() > 64
                        || channels
                            .iter()
                            .any(|channel| channel.trim().is_empty() || channel.len() > 64)
                    {
                        return Err(Error::Invalid(
                            "audio needs a sample rate and 1..64 named channels".into(),
                        ));
                    }
                }
            }
        }
    }
    let mut nested = HashMap::new();
    for composition in &project.compositions {
        identity(composition.id, identities)?;
        named(&composition.name)?;
        dimensions(composition.width, composition.height)?;
        composition.frame_rate.validate()?;
        if composition.duration == 0
            || composition.duration > i64::MAX as u64
            || composition.tracks.len() > 256
            || composition.nodes.len() > 32_768
        {
            return Err(Error::Invalid(
                "composition duration, tracks or nodes exceed their budget".into(),
            ));
        }
        let mut clips = HashMap::new();
        let mut clip_links = HashMap::new();
        let mut dependencies = Vec::new();
        for track in &composition.tracks {
            identity(track.id, identities)?;
            named(&track.name)?;
            for clip in &track.clips {
                identity(clip.id, identities)?;
                named(&clip.name)?;
                clip.range.validate(composition.duration)?;
                clip.time_map.validate(clip.range.end - clip.range.start)?;
                let (minimum, maximum) = match clip.source {
                    ClipSource::Media { source, stream } => {
                        let stream = sources
                            .get(&source)
                            .and_then(|source| {
                                source
                                    .streams
                                    .iter()
                                    .find(|candidate| candidate.index == stream)
                            })
                            .ok_or_else(|| {
                                Error::Invalid("clip references an absent source stream".into())
                            })?;
                        if !matches!(
                            (&stream.format, track.kind),
                            (StreamFormat::Video { .. }, TrackKind::Video)
                                | (StreamFormat::Audio { .. }, TrackKind::Audio)
                        ) {
                            return Err(Error::Invalid(
                                "clip source does not match its track type".into(),
                            ));
                        }
                        (
                            stream.start_tick,
                            stream.duration_ticks.map(|duration| {
                                (i128::from(stream.start_tick) + i128::from(duration)) as i64
                            }),
                        )
                    }
                    ClipSource::Composition { composition: child } => {
                        let child = compositions
                            .get(&child)
                            .ok_or_else(|| Error::Invalid("nested composition is absent".into()))?;
                        if (track.kind == TrackKind::Video && child.picture.is_none())
                            || (track.kind == TrackKind::Audio && child.audio.is_none())
                        {
                            return Err(Error::Invalid(
                                "nested composition has no matching output".into(),
                            ));
                        }
                        dependencies.push(child.id);
                        (
                            0,
                            Some(i64::try_from(child.duration).map_err(|_| {
                                Error::Invalid("nested duration exceeds integer time".into())
                            })?),
                        )
                    }
                };
                if clip.time_map.points.iter().any(|point| {
                    point.source_tick < minimum
                        || maximum.is_some_and(|maximum| point.source_tick > maximum)
                }) {
                    return Err(Error::Invalid(
                        "clip time map extends beyond the source".into(),
                    ));
                }
                if maximum.is_some_and(|maximum| {
                    clip.time_map.points.windows(2).any(|points| {
                        points[0].source_tick == maximum && points[1].source_tick == maximum
                    })
                }) {
                    return Err(Error::Invalid(
                        "freeze mapping cannot hold an exclusive source end".into(),
                    ));
                }
                clips.insert(clip.id, track.kind);
                clip_links.insert(clip.id, clip.linked);
            }
        }
        for track in &composition.tracks {
            for clip in &track.clips {
                if let Some(linked) = clip.linked
                    && (linked == clip.id || clip_links.get(&linked) != Some(&Some(clip.id)))
                {
                    return Err(Error::Invalid(
                        "linked clips must be a reciprocal pair in one composition".into(),
                    ));
                }
            }
        }
        validate_graph(composition, &clips, &assets, identities)?;
        nested.insert(composition.id, dependencies);
    }
    acyclic(&nested)?;
    for sequence in &project.sequences {
        if let Some(id) = sequence.composition {
            let composition = compositions
                .get(&id)
                .ok_or_else(|| Error::Invalid("sequence composition is absent".into()))?;
            if (sequence.width, sequence.height, sequence.frame_rate)
                != (
                    composition.width,
                    composition.height,
                    composition.frame_rate,
                )
            {
                return Err(Error::Invalid(
                    "sequence and composition profiles differ".into(),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return Err(Error::Invalid(
            "picture dimensions need 1..16384 pixels".into(),
        ));
    }
    Ok(())
}

impl NodeOperation {
    pub(crate) fn inputs(&self) -> Vec<(Uuid, SocketType)> {
        match self {
            Self::Transform { image, .. } => vec![(*image, SocketType::Image)],
            Self::Over {
                foreground,
                background,
                mask,
            } => {
                let mut inputs = vec![
                    (*foreground, SocketType::Image),
                    (*background, SocketType::Image),
                ];
                inputs.extend(mask.map(|mask| (mask, SocketType::Mask)));
                inputs
            }
            Self::Mask { geometry, .. } => vec![(*geometry, SocketType::Geometry)],
            Self::Gain { audio, .. } => vec![(*audio, SocketType::Audio)],
            Self::Mix { inputs } => inputs.iter().map(|id| (*id, SocketType::Audio)).collect(),
            Self::Opacity { image, value } => {
                vec![(*image, SocketType::Image), (*value, SocketType::Data)]
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn socket(&self, clips: &HashMap<Uuid, TrackKind>) -> Result<SocketType> {
        Ok(match self {
            Self::Source { clip } => match clips
                .get(clip)
                .ok_or_else(|| Error::Invalid("source node clip is absent".into()))?
            {
                TrackKind::Video => SocketType::Image,
                TrackKind::Audio => SocketType::Audio,
            },
            Self::Solid { .. }
            | Self::Transform { .. }
            | Self::Over { .. }
            | Self::Opacity { .. } => SocketType::Image,
            Self::Polygon { .. } => SocketType::Geometry,
            Self::Mask { .. } | Self::MaskAsset { .. } => SocketType::Mask,
            Self::Gain { .. } | Self::Mix { .. } => SocketType::Audio,
            Self::Scalar { .. } => SocketType::Data,
        })
    }

    pub(crate) fn supports(&self, property: AnimatedProperty) -> bool {
        use AnimatedProperty::*;
        matches!(
            (self, property),
            (
                Self::Transform { .. },
                Opacity | TranslateX | TranslateY | ScaleX | ScaleY | Rotation
            ) | (Self::Solid { .. }, Red | Green | Blue | Alpha)
                | (Self::Gain { .. }, Gain)
                | (Self::Scalar { .. }, Scalar)
                | (Self::Mask { .. }, Feather)
        )
    }
}

fn validate_graph(
    composition: &Composition,
    clips: &HashMap<Uuid, TrackKind>,
    assets: &HashMap<Uuid, &AssetReference>,
    identities: &mut HashSet<Uuid>,
) -> Result<()> {
    let nodes: HashMap<_, _> = composition
        .nodes
        .iter()
        .map(|node| (node.id, node))
        .collect();
    let mut edges = HashMap::new();
    let mut key_count = 0usize;
    for node in &composition.nodes {
        identity(node.id, identities)?;
        node.range.validate(composition.duration)?;
        node.operation.socket(clips)?;
        match &node.operation {
            NodeOperation::Solid { rgba } => {
                for value in rgba {
                    finite(*value)?;
                }
                unit(rgba[3])?;
            }
            NodeOperation::Transform {
                translation,
                scale,
                rotation,
                opacity,
                ..
            } => {
                for value in translation.iter().chain(scale).chain([rotation, opacity]) {
                    finite(*value)?;
                }
                unit(*opacity)?;
            }
            NodeOperation::Polygon { points } => {
                if points.len() < 3 || points.len() > 65_536 {
                    return Err(Error::Invalid("polygon needs 3..65536 points".into()));
                }
                for point in points {
                    for value in point {
                        finite(*value)?;
                    }
                }
            }
            NodeOperation::Mask { feather, .. } | NodeOperation::Gain { gain: feather, .. } => {
                finite(*feather)?;
                if *feather < 0.0 {
                    return Err(Error::Invalid("feather/gain cannot be negative".into()));
                }
            }
            NodeOperation::MaskAsset { asset } => {
                if assets
                    .get(asset)
                    .is_none_or(|asset| asset.kind != AssetKind::Mask)
                {
                    return Err(Error::Invalid("mask node needs a mask asset".into()));
                }
            }
            NodeOperation::Mix { inputs } => {
                if inputs.is_empty() || inputs.len() > 256 {
                    return Err(Error::Invalid("audio mix needs 1..256 inputs".into()));
                }
            }
            NodeOperation::Scalar { value } => {
                finite(*value)?;
            }
            _ => {}
        }
        let mut properties = HashSet::new();
        for channel in &node.animation {
            identity(channel.id, identities)?;
            if !properties.insert(channel.property)
                || !node.operation.supports(channel.property)
                || channel.keys.is_empty()
            {
                return Err(Error::Invalid(
                    "animation needs unique supported properties and keys".into(),
                ));
            }
            key_count = key_count
                .checked_add(channel.keys.len())
                .ok_or_else(|| Error::Invalid("animation budget overflow".into()))?;
            if key_count > 1_000_000
                || channel
                    .keys
                    .windows(2)
                    .any(|keys| keys[0].frame >= keys[1].frame)
            {
                return Err(Error::Invalid(
                    "animation keys must increase within the million-key budget".into(),
                ));
            }
            for key in &channel.keys {
                if key.frame < node.range.start || key.frame > node.range.end {
                    return Err(Error::Invalid(
                        "animation key is outside its timed node".into(),
                    ));
                }
                finite(key.value)?;
                finite(key.in_tangent)?;
                finite(key.out_tangent)?;
                match channel.property {
                    AnimatedProperty::Opacity | AnimatedProperty::Alpha => unit(key.value)?,
                    AnimatedProperty::Gain | AnimatedProperty::Feather if key.value < 0.0 => {
                        return Err(Error::Invalid(
                            "animated gain/feather cannot be negative".into(),
                        ));
                    }
                    _ => {}
                }
            }
        }
        let inputs = node.operation.inputs();
        for (id, socket) in &inputs {
            if nodes
                .get(id)
                .map(|input| input.operation.socket(clips))
                .transpose()?
                .is_none_or(|actual| actual != *socket)
            {
                return Err(Error::Invalid(
                    "node input is absent or has a different socket type".into(),
                ));
            }
        }
        edges.insert(node.id, inputs.into_iter().map(|(id, _)| id).collect());
    }
    for (output, socket) in [
        (composition.picture, SocketType::Image),
        (composition.audio, SocketType::Audio),
    ] {
        if let Some(output) = output {
            let node = nodes
                .get(&output)
                .ok_or_else(|| Error::Invalid("composition output node is absent".into()))?;
            if node.operation.socket(clips)? != socket {
                return Err(Error::Invalid(
                    "composition output has the wrong socket type".into(),
                ));
            }
        }
    }
    acyclic(&edges)
}

fn unit(value: f64) -> Result<()> {
    if !(0.0..=1.0).contains(&value) {
        return Err(Error::Invalid("alpha/opacity must be within 0..1".into()));
    }
    Ok(())
}

pub(crate) fn acyclic(edges: &HashMap<Uuid, Vec<Uuid>>) -> Result<()> {
    let mut remaining: HashMap<_, _> = edges
        .iter()
        .map(|(id, inputs)| (*id, inputs.len()))
        .collect();
    let mut dependants: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (id, inputs) in edges {
        for input in inputs {
            dependants.entry(*input).or_default().push(*id);
        }
    }
    let mut ready: VecDeque<_> = remaining
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut visited = 0;
    while let Some(id) = ready.pop_front() {
        visited += 1;
        for dependant in dependants.get(&id).into_iter().flatten() {
            let count = remaining
                .get_mut(dependant)
                .ok_or_else(|| Error::Invalid("dependency is absent".into()))?;
            *count -= 1;
            if *count == 0 {
                ready.push_back(*dependant);
            }
        }
    }
    if visited != edges.len() {
        return Err(Error::Invalid(
            "composition or node graph contains a cycle or absent dependency".into(),
        ));
    }
    Ok(())
}
