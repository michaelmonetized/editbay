//! Editable caption tracks with exact rational timing and honest interchange.

use crate::{Error, FrameRate, Result};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, collections::HashSet};
use uuid::Uuid;

const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CUES: usize = 100_000;
const MAX_TEXT_BYTES: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CaptionTime {
    pub numerator: u64,
    pub denominator: u64,
}

impl CaptionTime {
    /// Construct exact nonnegative seconds.
    /// `numerator` and `denominator` describe seconds; returns a reduced fraction.
    pub fn new(numerator: u64, denominator: u64) -> Result<Self> {
        Self::from_fraction(u128::from(numerator), u128::from(denominator))
    }

    /// Keep a subtitle timestamp exact.
    /// `milliseconds` is elapsed time; returns reduced seconds without rounding.
    pub fn from_milliseconds(milliseconds: u64) -> Self {
        let factor = gcd(u128::from(milliseconds), 1000) as u64;
        Self {
            numerator: milliseconds / factor,
            denominator: 1000 / factor,
        }
    }

    /// Keep a sequence frame boundary exact.
    /// `frame` and `rate` select the boundary; returns reduced elapsed seconds.
    pub fn from_frame(frame: u64, rate: FrameRate) -> Result<Self> {
        rate.validate()?;
        Self::from_fraction(
            u128::from(frame) * u128::from(rate.denominator),
            u128::from(rate.numerator),
        )
    }

    /// Validate a stored exact timestamp.
    /// Takes this value and returns an error for a zero or noncanonical scale.
    pub fn validate(self) -> Result<()> {
        if self.denominator == 0
            || gcd(u128::from(self.numerator), u128::from(self.denominator)) != 1
        {
            return invalid("caption time needs a reduced fraction with a positive denominator");
        }
        Ok(())
    }

    /// Export only timestamps a subtitle file can represent exactly.
    /// Takes this value and returns milliseconds, rejecting submillisecond loss.
    pub fn to_milliseconds(self) -> Result<u64> {
        self.validate()?;
        let numerator = u128::from(self.numerator) * 1000;
        if !numerator.is_multiple_of(u128::from(self.denominator)) {
            return invalid(
                "caption boundary is not an exact millisecond; retime it before SRT/VTT export",
            );
        }
        u64::try_from(numerator / u128::from(self.denominator))
            .map_err(|_| Error::Invalid("caption timestamp exceeds subtitle integer range".into()))
    }

    /// Compare two timestamps without float conversion.
    /// `other` is elapsed seconds; returns exact ordering after validation.
    pub fn compare(self, other: Self) -> Result<Ordering> {
        self.validate()?;
        other.validate()?;
        Ok(time_order(self, other))
    }

    /// Move a boundary by an integer number of milliseconds.
    /// `delta` may be negative; returns exact seconds or rejects negative time.
    pub fn shifted_milliseconds(self, delta: i64) -> Result<Self> {
        self.validate()?;
        let factor = gcd(u128::from(self.denominator), 1000);
        let scale = 1000 / factor;
        let origin = u128::from(self.numerator) * scale;
        let change = u128::from(delta.unsigned_abs()) * (u128::from(self.denominator) / factor);
        let numerator = if delta < 0 {
            origin.checked_sub(change).ok_or_else(|| {
                Error::Invalid("caption retiming would move a cue before zero".into())
            })?
        } else {
            origin.checked_add(change).ok_or_else(|| {
                Error::Invalid("caption retiming exceeds its integer range".into())
            })?
        };
        Self::from_fraction(numerator, u128::from(self.denominator) * scale)
    }

    /// Reduce a wider intermediate timestamp.
    /// `numerator` and `denominator` are seconds; returns a stored exact fraction.
    fn from_fraction(numerator: u128, denominator: u128) -> Result<Self> {
        if denominator == 0 {
            return invalid("caption timestamp denominator is zero");
        }
        let factor = gcd(numerator, denominator);
        Ok(Self {
            numerator: u64::try_from(numerator / factor)
                .map_err(|_| Error::Invalid("caption time exceeds its integer range".into()))?,
            denominator: u64::try_from(denominator / factor)
                .map_err(|_| Error::Invalid("caption time exceeds its integer range".into()))?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CaptionAlignment {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct CaptionSafeArea {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Default for CaptionSafeArea {
    fn default() -> Self {
        Self {
            left: 0.1,
            top: 0.1,
            right: 0.1,
            bottom: 0.1,
        }
    }
}

impl CaptionSafeArea {
    /// Check normalized frame insets.
    /// Takes this area and returns an error for empty, negative or nonfinite bounds.
    pub fn validate(self) -> Result<()> {
        if [self.left, self.top, self.right, self.bottom]
            .into_iter()
            .any(|value| !value.is_finite() || !(0.0..1.0).contains(&value))
            || self.left + self.right >= 1.0
            || self.top + self.bottom >= 1.0
        {
            return invalid("caption safe area must leave a positive rectangle inside the frame");
        }
        Ok(())
    }

    /// Locate the caption-safe rectangle in pixels.
    /// `width` and `height` are frame dimensions; returns left, top, right, bottom.
    pub fn bounds(self, width: u32, height: u32) -> Result<[f32; 4]> {
        self.validate()?;
        if width == 0 || height == 0 {
            return invalid("caption safe-area preview needs positive frame dimensions");
        }
        Ok([
            self.left * width as f32,
            self.top * height as f32,
            (1.0 - self.right) * width as f32,
            (1.0 - self.bottom) * height as f32,
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct CaptionStyle {
    pub font: Option<Uuid>,
    pub font_size: f32,
    pub line_height: f32,
    pub foreground: [f32; 4],
    pub background: [f32; 4],
    pub alignment: CaptionAlignment,
    pub safe_area: CaptionSafeArea,
}

impl Default for CaptionStyle {
    fn default() -> Self {
        Self {
            font: None,
            font_size: 48.0,
            line_height: 1.2,
            foreground: [1.0; 4],
            background: [0.0, 0.0, 0.0, 0.75],
            alignment: CaptionAlignment::Center,
            safe_area: CaptionSafeArea::default(),
        }
    }
}

impl CaptionStyle {
    /// Check native caption appearance.
    /// Takes this style and returns an error for unusable geometry, colors or font ID.
    pub fn validate(&self) -> Result<()> {
        self.safe_area.validate()?;
        if self.font.is_some_and(|id| id.is_nil())
            || !self.font_size.is_finite()
            || !(1.0..=4096.0).contains(&self.font_size)
            || !self.line_height.is_finite()
            || !(0.5..=8.0).contains(&self.line_height)
            || self
                .foreground
                .iter()
                .chain(&self.background)
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return invalid("caption style has an invalid font, size, line height or RGBA color");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CaptionDelivery {
    None,
    #[default]
    SidecarSrt,
    SidecarVtt,
    BurnIn,
    BurnInAndSrt,
    BurnInAndVtt,
}

impl CaptionDelivery {
    /// Select tracks that become picture pixels.
    /// Takes this delivery choice and returns whether burn-in is requested.
    pub fn burns_in(self) -> bool {
        matches!(self, Self::BurnIn | Self::BurnInAndSrt | Self::BurnInAndVtt)
    }

    /// Select the requested sidecar extension.
    /// Takes this delivery choice and returns SRT, VTT or no sidecar.
    pub fn sidecar_extension(self) -> Option<&'static str> {
        match self {
            Self::SidecarSrt | Self::BurnInAndSrt => Some("srt"),
            Self::SidecarVtt | Self::BurnInAndVtt => Some("vtt"),
            Self::None | Self::BurnIn => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CaptionCue {
    pub id: Uuid,
    pub start: CaptionTime,
    pub end: CaptionTime,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vtt_settings: String,
}

impl CaptionCue {
    /// Create an editable cue with a fresh identity.
    /// `start`, `end` and `text` define the cue; returns a validated exclusive range.
    pub fn new(start: CaptionTime, end: CaptionTime, text: impl Into<String>) -> Result<Self> {
        let cue = Self {
            id: Uuid::new_v4(),
            start,
            end,
            text: text.into(),
            identifier: None,
            vtt_settings: String::new(),
        };
        cue.validate()?;
        Ok(cue)
    }

    /// Validate one caption independently of a document.
    /// Takes this cue and returns an error for empty identity, text or time range.
    pub fn validate(&self) -> Result<()> {
        if self.id.is_nil() || self.start.compare(self.end)? != Ordering::Less {
            return invalid("caption cue needs an identity and a positive time range");
        }
        validate_text(&self.text)?;
        if let Some(identifier) = &self.identifier
            && (identifier.trim().is_empty()
                || identifier.contains(['\n', '\r', '\0'])
                || identifier.contains("-->"))
        {
            return invalid(
                "caption cue identifier must be a nonempty single line without an arrow",
            );
        }
        validate_settings(&self.vtt_settings)
    }

    /// Test an exact instant against the cue's half-open range.
    /// `time` is elapsed seconds; returns true from start through just before end.
    pub fn contains(&self, time: CaptionTime) -> Result<bool> {
        self.validate()?;
        time.validate()?;
        Ok(time_order(self.start, time) != Ordering::Greater
            && time_order(time, self.end) == Ordering::Less)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CaptionTrack {
    pub id: Uuid,
    pub composition: Uuid,
    pub name: String,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    #[serde(default)]
    pub cues: Vec<CaptionCue>,
    #[serde(default)]
    pub style: CaptionStyle,
    #[serde(default)]
    pub delivery: CaptionDelivery,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vtt_header: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vtt_metadata: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vtt_blocks: Vec<String>,
}

impl CaptionTrack {
    /// Create an empty caption track owned by a composition.
    /// `composition` and `name` identify its destination; returns a validated track.
    pub fn new(composition: Uuid, name: impl Into<String>) -> Result<Self> {
        let track = Self {
            id: Uuid::new_v4(),
            composition,
            name: name.into(),
            enabled: true,
            cues: Vec::new(),
            style: CaptionStyle::default(),
            delivery: CaptionDelivery::default(),
            vtt_header: String::new(),
            vtt_metadata: Vec::new(),
            vtt_blocks: Vec::new(),
        };
        track.validate()?;
        Ok(track)
    }

    /// Validate durable caption state.
    /// Takes this track and returns an error for invalid content or duplicate cue IDs.
    pub fn validate(&self) -> Result<()> {
        if self.id.is_nil()
            || self.composition.is_nil()
            || self.name.trim().is_empty()
            || self.name.len() > 1024
            || self.name.contains(['\n', '\r', '\0'])
            || self.cues.len() > MAX_CUES
        {
            return invalid("caption track needs valid identities, a name and a bounded cue count");
        }
        self.style.validate()?;
        let bytes = self
            .cues
            .iter()
            .map(|cue| {
                cue.text.len()
                    + cue.vtt_settings.len()
                    + cue.identifier.as_ref().map_or(0, String::len)
            })
            .chain(self.vtt_metadata.iter().map(String::len))
            .chain(self.vtt_blocks.iter().map(String::len))
            .try_fold(self.vtt_header.len(), |total, bytes| {
                total.checked_add(bytes)
            });
        if bytes.is_none_or(|bytes| bytes > MAX_FILE_BYTES) {
            return invalid("caption track text and metadata exceed 16 MiB");
        }
        let mut ids = HashSet::from([self.id]);
        for cue in &self.cues {
            cue.validate()?;
            if !ids.insert(cue.id) {
                return invalid("caption cue identities must be unique within a track");
            }
        }
        if self
            .cues
            .windows(2)
            .any(|cues| time_order(cues[0].start, cues[1].start) == Ordering::Greater)
        {
            return invalid("caption cues must be ordered by start time");
        }
        if self.vtt_header.contains(['\n', '\r', '\0']) || self.vtt_header.contains("-->") {
            return invalid("WebVTT header must be one line without a timing arrow");
        }
        for line in &self.vtt_metadata {
            if line.contains(['\n', '\r', '\0']) || line.trim().is_empty() || line.contains("-->") {
                return invalid(
                    "WebVTT header metadata must be nonempty lines without timing arrows",
                );
            }
        }
        for block in &self.vtt_blocks {
            validate_block(block)?;
        }
        Ok(())
    }

    /// Add a cue without disturbing chronological order.
    /// `cue` is independently validated; returns an error without mutating on failure.
    pub fn insert_cue(&mut self, cue: CaptionCue) -> Result<()> {
        cue.validate()?;
        if self.cues.len() >= MAX_CUES
            || cue.id == self.id
            || self.cues.iter().any(|other| other.id == cue.id)
        {
            return invalid("caption insert exceeds the cue limit or duplicates an identity");
        }
        self.cues.push(cue);
        self.sort_cues();
        Ok(())
    }

    /// Replace cue text through its stable identity.
    /// `id` selects a cue and `text` is Unicode content; returns an atomic edit result.
    pub fn edit_cue(&mut self, id: Uuid, text: impl Into<String>) -> Result<()> {
        let text = text.into();
        validate_text(&text)?;
        self.cue_mut(id)?.text = text;
        Ok(())
    }

    /// Change cue timing without rounding to sequence frames.
    /// `id`, `start` and `end` select an exact range; returns an atomic edit result.
    pub fn retime_cue(&mut self, id: Uuid, start: CaptionTime, end: CaptionTime) -> Result<()> {
        if start.compare(end)? != Ordering::Less {
            return invalid("caption retiming requires end after start");
        }
        let cue = self.cue_mut(id)?;
        cue.start = start;
        cue.end = end;
        self.sort_cues();
        Ok(())
    }

    /// Split both text and time while preserving the first cue identity.
    /// `id`, interior `at` and Unicode-scalar `text_offset` select the split.
    /// Returns the second cue's new identity, rejecting empty halves atomically.
    pub fn split_cue(&mut self, id: Uuid, at: CaptionTime, text_offset: usize) -> Result<Uuid> {
        at.validate()?;
        if self.cues.len() >= MAX_CUES {
            return invalid("caption split exceeds the cue count limit");
        }
        let original = self
            .cues
            .iter()
            .find(|cue| cue.id == id)
            .ok_or_else(|| Error::Invalid("caption cue does not exist".into()))?;
        if time_order(original.start, at) != Ordering::Less
            || time_order(at, original.end) != Ordering::Less
        {
            return invalid("caption split time must be strictly inside the cue");
        }
        let byte = original
            .text
            .char_indices()
            .nth(text_offset)
            .map(|(offset, _)| offset)
            .ok_or_else(|| Error::Invalid("caption split needs an interior text offset".into()))?;
        let left = original.text[..byte].trim_end().to_owned();
        let right = original.text[byte..].trim_start().to_owned();
        validate_text(&left)?;
        validate_text(&right)?;
        let mut second = original.clone();
        second.id = Uuid::new_v4();
        second.identifier = None;
        second.start = at;
        second.text = right;
        let new_id = second.id;
        let first = self.cue_mut(id)?;
        first.end = at;
        first.text = left;
        self.cues.push(second);
        self.sort_cues();
        Ok(new_id)
    }

    /// Move every cue with one exact offset.
    /// `delta` is signed milliseconds; returns an error with no partial retiming.
    pub fn shift_milliseconds(&mut self, delta: i64) -> Result<()> {
        let mut cues = self.cues.clone();
        for cue in &mut cues {
            cue.start = cue.start.shifted_milliseconds(delta)?;
            cue.end = cue.end.shifted_milliseconds(delta)?;
        }
        self.cues = cues;
        Ok(())
    }

    /// Select active captions for preview at exact elapsed time.
    /// `time` is rational seconds; returns enabled cues with exclusive end boundaries.
    pub fn active_at(&self, time: CaptionTime) -> Result<Vec<&CaptionCue>> {
        time.validate()?;
        if !self.enabled {
            return Ok(Vec::new());
        }
        self.cues
            .iter()
            .filter_map(|cue| match cue.contains(time) {
                Ok(true) => Some(Ok(cue)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    /// Select active captions at a sequence frame boundary.
    /// `frame` and `rate` select exact time; returns active cues without millisecond rounding.
    pub fn active_at_frame(&self, frame: u64, rate: FrameRate) -> Result<Vec<&CaptionCue>> {
        self.active_at(CaptionTime::from_frame(frame, rate)?)
    }

    /// Reject styling the plain native caption renderer cannot reproduce.
    /// Takes this track and returns an explicit error instead of dropping WebVTT styling.
    pub fn validate_burn_in(&self) -> Result<()> {
        self.validate()?;
        if self.vtt_blocks.iter().any(|block| {
            block
                .lines()
                .next()
                .is_some_and(|line| line == "STYLE" || line == "REGION")
        }) || self
            .cues
            .iter()
            .any(|cue| !cue.vtt_settings.is_empty() || contains_markup(&cue.text))
        {
            return invalid(
                "caption burn-in supports plain Unicode text and native track style; WebVTT positioning, markup and entities need explicit conversion first",
            );
        }
        Ok(())
    }

    /// Import a complete SRT file into a fresh editable track.
    /// `composition`, `name` and UTF-8 `input` select ownership and contents.
    /// Returns exact cue times, preserving Unicode and rejecting malformed blocks.
    pub fn import_srt(composition: Uuid, name: impl Into<String>, input: &str) -> Result<Self> {
        let input = normalize_input(input)?;
        let mut track = Self::new(composition, name)?;
        for block in blocks(&input) {
            if track.cues.len() >= MAX_CUES {
                return invalid("caption import exceeds the cue count limit");
            }
            let lines: Vec<_> = block.lines().collect();
            let (timing_index, identifier) = if lines[0].contains("-->") {
                (0, None)
            } else {
                if lines.len() < 3
                    || lines[0].is_empty()
                    || !lines[0].bytes().all(|byte| byte.is_ascii_digit())
                {
                    return invalid("SRT block needs a numeric cue index and a timing line");
                }
                (1, Some(lines[0].to_owned()))
            };
            let (start, end, settings) = parse_timing(lines[timing_index], false)?;
            if !settings.is_empty() {
                return invalid(
                    "SRT cue positioning is not supported; use WebVTT for positioned cues",
                );
            }
            let text = lines[timing_index + 1..].join("\n");
            let mut cue = CaptionCue::new(start, end, text)?;
            cue.identifier = identifier;
            track.cues.push(cue);
        }
        track.validate()?;
        Ok(track)
    }

    /// Import a WebVTT file without discarding metadata or styling.
    /// `composition`, `name` and UTF-8 `input` select ownership and contents.
    /// Returns cues with exact timing and retained header, NOTE, STYLE and REGION blocks.
    pub fn import_vtt(composition: Uuid, name: impl Into<String>, input: &str) -> Result<Self> {
        let input = normalize_input(input)?;
        let mut input_blocks = blocks(&input);
        let header = input_blocks
            .next()
            .ok_or_else(|| Error::Invalid("WebVTT file needs a WEBVTT header".into()))?;
        let mut header_lines = header.lines();
        let signature = header_lines.next().unwrap_or_default();
        if signature != "WEBVTT"
            && !signature.starts_with("WEBVTT ")
            && !signature.starts_with("WEBVTT\t")
        {
            return invalid("WebVTT file needs a WEBVTT header and a blank line before cues");
        }
        let mut track = Self::new(composition, name)?;
        track.delivery = CaptionDelivery::SidecarVtt;
        track.vtt_header = signature[6..].trim_start_matches([' ', '\t']).to_owned();
        track.vtt_metadata = header_lines.map(str::to_owned).collect();
        for block in input_blocks {
            let first = block.lines().next().unwrap_or_default();
            if is_note(first) || first == "STYLE" || first == "REGION" {
                if !is_note(first) && !track.cues.is_empty() {
                    return invalid("WebVTT STYLE and REGION blocks must precede cues");
                }
                validate_block(block)?;
                track.vtt_blocks.push(block.to_owned());
                continue;
            }
            if track.cues.len() >= MAX_CUES {
                return invalid("caption import exceeds the cue count limit");
            }
            let lines: Vec<_> = block.lines().collect();
            let (timing_index, identifier) = if first.contains("-->") {
                (0, None)
            } else {
                if lines.len() < 3 {
                    return invalid("WebVTT cue needs timing and nonempty text");
                }
                (1, Some(first.to_owned()))
            };
            let (start, end, settings) = parse_timing(lines[timing_index], true)?;
            let text = lines[timing_index + 1..].join("\n");
            if text.contains("-->") {
                return invalid("WebVTT cue text cannot contain a timing arrow");
            }
            let mut cue = CaptionCue::new(start, end, text)?;
            cue.identifier = identifier;
            cue.vtt_settings = settings.to_owned();
            cue.validate()?;
            track.cues.push(cue);
        }
        track.validate()?;
        Ok(track)
    }

    /// Write a complete SRT sidecar with exact timestamps.
    /// Takes this track and returns UTF-8 SRT or an explicit timing/style-loss error.
    pub fn export_srt(&self) -> Result<String> {
        self.validate()?;
        if !self.vtt_metadata.is_empty()
            || self
                .vtt_blocks
                .iter()
                .any(|block| block.lines().next().is_some_and(|line| !is_note(line)))
            || self.cues.iter().any(|cue| !cue.vtt_settings.is_empty())
        {
            return invalid(
                "SRT cannot preserve this track's WebVTT metadata or positioning; export VTT instead",
            );
        }
        let mut output = String::new();
        for (index, cue) in self.cues.iter().enumerate() {
            validate_markup(&cue.text, false)?;
            output.push_str(&(index + 1).to_string());
            output.push('\n');
            output.push_str(&format_timestamp(cue.start, ',')?);
            output.push_str(" --> ");
            output.push_str(&format_timestamp(cue.end, ',')?);
            output.push('\n');
            output.push_str(&cue.text);
            output.push_str("\n\n");
        }
        Ok(output)
    }

    /// Write a WebVTT sidecar with retained identifiers, metadata and cue settings.
    /// Takes this track and returns UTF-8 WebVTT, rejecting unrepresentable timing/text.
    pub fn export_vtt(&self) -> Result<String> {
        self.validate()?;
        let mut output = "WEBVTT".to_owned();
        if !self.vtt_header.is_empty() {
            output.push(' ');
            output.push_str(&self.vtt_header);
        }
        output.push('\n');
        for line in &self.vtt_metadata {
            output.push_str(line);
            output.push('\n');
        }
        output.push('\n');
        for block in &self.vtt_blocks {
            output.push_str(block);
            output.push_str("\n\n");
        }
        for cue in &self.cues {
            validate_markup(&cue.text, true)?;
            if cue.text.contains("-->") {
                return invalid("WebVTT cue text cannot contain a timing arrow");
            }
            if let Some(identifier) = &cue.identifier {
                if identifier == "STYLE" || identifier == "REGION" || is_note(identifier) {
                    return invalid("WebVTT cue identifier collides with a reserved block header");
                }
                output.push_str(identifier);
                output.push('\n');
            }
            output.push_str(&format_timestamp(cue.start, '.')?);
            output.push_str(" --> ");
            output.push_str(&format_timestamp(cue.end, '.')?);
            if !cue.vtt_settings.is_empty() {
                output.push(' ');
                output.push_str(&cue.vtt_settings);
            }
            output.push('\n');
            output.push_str(&cue.text);
            output.push_str("\n\n");
        }
        Ok(output)
    }

    /// Find a cue for one validated edit.
    /// `id` is its stable document identity; returns a mutable cue or a missing-ID error.
    fn cue_mut(&mut self, id: Uuid) -> Result<&mut CaptionCue> {
        self.cues
            .iter_mut()
            .find(|cue| cue.id == id)
            .ok_or_else(|| Error::Invalid("caption cue does not exist".into()))
    }

    /// Keep cue serialization and export chronological.
    /// Takes this track and orders equal starts by their exact end times.
    fn sort_cues(&mut self) {
        self.cues
            .sort_by(|a, b| time_order(a.start, b.start).then_with(|| time_order(a.end, b.end)));
    }
}

/// Preserve enabled output for older caption documents.
/// Takes no arguments and returns the legacy default, true.
fn enabled_default() -> bool {
    true
}

/// Compare already validated rational seconds.
/// `a` and `b` are exact times; returns ordering without narrowing multiplication.
fn time_order(a: CaptionTime, b: CaptionTime) -> Ordering {
    (u128::from(a.numerator) * u128::from(b.denominator))
        .cmp(&(u128::from(b.numerator) * u128::from(a.denominator)))
}

/// Find a common integer factor.
/// `a` and `b` are magnitudes; returns their greatest common divisor.
fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Produce a caption validation failure.
/// `message` explains the rejected input; returns the shared document error.
fn invalid<T>(message: &str) -> Result<T> {
    Err(Error::Invalid(message.to_owned()))
}

/// Reject cue text that cannot round-trip through subtitle blocks.
/// `text` is Unicode cue content; returns an error for empty or ambiguous blocks.
fn validate_text(text: &str) -> Result<()> {
    if text.trim().is_empty()
        || text.len() > MAX_TEXT_BYTES
        || text.contains('\r')
        || text
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        || text.split('\n').any(|line| line.trim().is_empty())
    {
        return invalid(
            "caption text must be nonempty bounded Unicode lines without blank lines or control characters",
        );
    }
    Ok(())
}

/// Normalize common subtitle encodings of line endings.
/// `input` is decoded UTF-8; returns bounded LF text with an optional BOM removed.
fn normalize_input(input: &str) -> Result<String> {
    if input.len() > MAX_FILE_BYTES || input.contains('\0') {
        return invalid("caption file exceeds 16 MiB or contains a NUL character");
    }
    let normalized = input
        .strip_prefix('\u{feff}')
        .unwrap_or(input)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    if normalized
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return invalid("caption file contains unsupported control characters");
    }
    Ok(normalized
        .split('\n')
        .map(|line| if line.trim().is_empty() { "" } else { line })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Iterate nonempty subtitle blocks.
/// `input` has normalized line endings; returns blocks separated by blank lines.
fn blocks(input: &str) -> impl Iterator<Item = &str> {
    input
        .split("\n\n")
        .map(|block| block.trim_matches('\n'))
        .filter(|block| !block.trim().is_empty())
}

/// Read one subtitle timing line.
/// `line` and `vtt` select syntax; returns exact boundaries and retained cue settings.
fn parse_timing(line: &str, vtt: bool) -> Result<(CaptionTime, CaptionTime, &str)> {
    let (start, remainder) = line
        .split_once("-->")
        .ok_or_else(|| Error::Invalid("caption cue needs a timing arrow".into()))?;
    if remainder.contains("-->") {
        return invalid("caption timing line contains more than one arrow");
    }
    let remainder = remainder.trim();
    let split = remainder
        .find(char::is_whitespace)
        .unwrap_or(remainder.len());
    let (end, settings) = remainder.split_at(split);
    let settings = settings.trim();
    let start = parse_timestamp(start.trim(), vtt)?;
    let end = parse_timestamp(end, vtt)?;
    if start.compare(end)? != Ordering::Less {
        return invalid("caption cue end must be after its start");
    }
    validate_settings(settings)?;
    Ok((start, end, settings))
}

/// Parse an SRT or WebVTT timestamp strictly.
/// `input` is one token and `vtt` allows MM:SS.mmm; returns exact milliseconds.
fn parse_timestamp(input: &str, vtt: bool) -> Result<CaptionTime> {
    let separator = if vtt { '.' } else { ',' };
    let (clock, millis) = input
        .rsplit_once(separator)
        .ok_or_else(|| Error::Invalid("caption timestamp needs three millisecond digits".into()))?;
    if millis.len() != 3 || !millis.bytes().all(|byte| byte.is_ascii_digit()) {
        return invalid("caption timestamp needs exactly three millisecond digits");
    }
    let parts: Vec<_> = clock.split(':').collect();
    if parts.len() != 3 && !(vtt && parts.len() == 2) {
        return invalid("caption timestamp needs HH:MM:SS or WebVTT MM:SS");
    }
    let seconds = parts[parts.len() - 1];
    let minutes = parts[parts.len() - 2];
    if [seconds, minutes]
        .into_iter()
        .any(|value| value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return invalid("caption timestamp minute and second fields need two digits");
    }
    let seconds = parse_u64(seconds)?;
    let minutes = parse_u64(minutes)?;
    if seconds >= 60 || minutes >= 60 {
        return invalid("caption timestamp minute and second fields must be below 60");
    }
    let hours = if parts.len() == 3 {
        if parts[0].len() < 2 || !parts[0].bytes().all(|byte| byte.is_ascii_digit()) {
            return invalid("caption timestamp hours need at least two digits");
        }
        parse_u64(parts[0])?
    } else {
        0
    };
    let milliseconds = hours
        .checked_mul(3_600_000)
        .and_then(|value| value.checked_add(minutes * 60_000 + seconds * 1000))
        .and_then(|value| value.checked_add(parse_u64(millis).ok()?))
        .ok_or_else(|| Error::Invalid("caption timestamp exceeds its integer range".into()))?;
    Ok(CaptionTime::from_milliseconds(milliseconds))
}

/// Parse a bounded unsigned timestamp component.
/// `input` contains decimal digits; returns its value or a subtitle error.
fn parse_u64(input: &str) -> Result<u64> {
    input
        .parse()
        .map_err(|_| Error::Invalid("caption timestamp exceeds its integer range".into()))
}

/// Format a timestamp without losing fractional time.
/// `time` and `separator` select exact milliseconds and format; returns HH:MM:SS.mmm.
fn format_timestamp(time: CaptionTime, separator: char) -> Result<String> {
    let milliseconds = time.to_milliseconds()?;
    Ok(format!(
        "{:02}:{:02}:{:02}{separator}{:03}",
        milliseconds / 3_600_000,
        milliseconds / 60_000 % 60,
        milliseconds / 1000 % 60,
        milliseconds % 1000
    ))
}

/// Validate retained WebVTT cue positioning tokens.
/// `settings` is the timing-line suffix; returns an error for malformed or unknown settings.
fn validate_settings(settings: &str) -> Result<()> {
    if settings.len() > MAX_TEXT_BYTES || settings.contains(['\n', '\r', '\0']) {
        return invalid("WebVTT cue settings must occupy one line");
    }
    let mut names = HashSet::new();
    for setting in settings.split_whitespace() {
        let (name, value) = setting
            .split_once(':')
            .ok_or_else(|| Error::Invalid("WebVTT cue setting needs name:value".into()))?;
        if !names.insert(name) || value.is_empty() {
            return invalid("WebVTT cue settings must have unique names and nonempty values");
        }
        let valid = match name {
            "align" => matches!(value, "start" | "center" | "end" | "left" | "right"),
            "vertical" => matches!(value, "rl" | "lr"),
            "size" => valid_percentage(value),
            "position" => {
                let (position, alignment) = value.split_once(',').unwrap_or((value, "auto"));
                valid_percentage(position)
                    && matches!(alignment, "auto" | "line-left" | "center" | "line-right")
            }
            "line" => {
                let (line, alignment) = value.split_once(',').unwrap_or((value, "start"));
                (line == "auto" || valid_percentage(line) || line.parse::<i64>().is_ok())
                    && matches!(alignment, "start" | "center" | "end")
            }
            "region" => !value.contains([':', ',']),
            _ => false,
        };
        if !valid {
            return invalid("WebVTT cue setting has an unsupported name or invalid value");
        }
    }
    Ok(())
}

/// Check one WebVTT percentage.
/// `value` is a percent token; returns true for decimal percentages from zero to 100.
fn valid_percentage(value: &str) -> bool {
    let Some(number) = value.strip_suffix('%') else {
        return false;
    };
    let mut dots = 0;
    if number.is_empty()
        || !number.bytes().all(|byte| {
            if byte == b'.' {
                dots += 1;
                dots == 1
            } else {
                byte.is_ascii_digit()
            }
        })
    {
        return false;
    }
    number
        .parse::<f64>()
        .is_ok_and(|number| (0.0..=100.0).contains(&number))
}

/// Recognize a WebVTT comment block.
/// `line` is its first line; returns true for NOTE followed by space, tab or line end.
fn is_note(line: &str) -> bool {
    line == "NOTE" || line.starts_with("NOTE ") || line.starts_with("NOTE\t")
}

/// Keep noncue WebVTT blocks safe to write back.
/// `block` is retained source text; returns an error for ambiguous or unknown blocks.
fn validate_block(block: &str) -> Result<()> {
    let first = block.lines().next().unwrap_or_default();
    if block.is_empty()
        || block.len() > MAX_FILE_BYTES
        || block.contains(['\r', '\0'])
        || block.contains("\n\n")
        || block.contains("-->")
        || (!is_note(first) && first != "STYLE" && first != "REGION")
        || (!is_note(first) && block.lines().count() < 2)
    {
        return invalid(
            "WebVTT block must be a bounded NOTE, STYLE or REGION block without timing arrows",
        );
    }
    Ok(())
}

/// Detect styled text requiring richer rendering than plain glyphs.
/// `text` is cue content; returns true for tag pairs or character entities.
fn contains_markup(text: &str) -> bool {
    text.split_once('<')
        .is_some_and(|(_, rest)| rest.contains('>'))
        || text.match_indices('&').any(|(index, _)| {
            text[index + 1..]
                .split_once(';')
                .is_some_and(|(entity, _)| {
                    !entity.is_empty()
                        && entity.len() <= 32
                        && entity
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'#')
                })
        })
}

/// Reject markup another subtitle format cannot preserve.
/// `text` and `vtt` select destination syntax; returns an explicit unsupported-tag error.
fn validate_markup(text: &str, vtt: bool) -> Result<()> {
    let mut remaining = text;
    while let Some((_, after_open)) = remaining.split_once('<') {
        let Some((tag, rest)) = after_open.split_once('>') else {
            break;
        };
        remaining = rest;
        let simple = matches!(tag, "b" | "/b" | "i" | "/i" | "u" | "/u");
        let supported = if vtt {
            simple
                || matches!(
                    tag,
                    "c" | "/c" | "v" | "/v" | "lang" | "/lang" | "ruby" | "/ruby" | "rt" | "/rt"
                )
                || tag.starts_with("c.")
                || tag.starts_with("v ")
                || tag.starts_with("lang ")
                || parse_timestamp(tag, true).is_ok()
        } else {
            simple || tag == "/font" || tag.starts_with("font ")
        };
        if !supported {
            return invalid("caption markup cannot be preserved in the requested subtitle format");
        }
    }
    Ok(())
}
