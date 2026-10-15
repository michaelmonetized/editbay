use crate::Result;
use editbay_core::{
    AssetKind, AssetReference,
    captions::{CaptionAlignment, CaptionCue, CaptionTrack},
};
use fontdue::{Font, FontSettings, Metrics};
use rustybuzz::{BufferFlags, Direction, Face, UnicodeBuffer};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{File, Metadata},
    io::Read,
    ops::Range,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::Arc,
};
use unicode_bidi::{BidiInfo, ParagraphInfo};
use unicode_script::{Script, UnicodeScript};
use unicode_segmentation::UnicodeSegmentation;

const FONT_LIMIT: usize = 16 * 1024 * 1024;
const WORK_LIMIT: usize = 64 * 1024 * 1024;
const SHAPE_LIMIT: usize = 2 * 1024 * 1024;
const TEXT_LIMIT: usize = 65_536;
const FONT_ENTRIES: usize = 8;
const GLYPH_ENTRIES: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Signature {
    device: u64,
    inode: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl Signature {
    /// Capture the opened font file's ownership and modification stamp.
    /// `metadata` belongs to an open descriptor; returns a comparable native signature.
    fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

struct FontEntry {
    hash: String,
    path: PathBuf,
    signature: Signature,
    bytes: Arc<[u8]>,
    charge: usize,
}

#[derive(Clone)]
struct GlyphImage {
    metrics: Metrics,
    coverage: Arc<[u8]>,
}

struct GlyphEntry {
    hash: String,
    size: u32,
    glyph: u16,
    image: GlyphImage,
    charge: usize,
}

/// Bounded verified font bytes and indexed glyph images for caption rendering.
/// Retained parsed fontdue geometry is avoided because its private allocations cannot be charged.
pub(crate) struct CaptionFontCache {
    fonts: Vec<FontEntry>,
    glyphs: Vec<GlyphEntry>,
    bytes: usize,
}

impl Default for CaptionFontCache {
    fn default() -> Self {
        let fonts = Vec::with_capacity(FONT_ENTRIES);
        let glyphs = Vec::with_capacity(GLYPH_ENTRIES);
        let bytes = fonts.capacity() * std::mem::size_of::<FontEntry>()
            + glyphs.capacity() * std::mem::size_of::<GlyphEntry>();
        Self {
            fonts,
            glyphs,
            bytes,
        }
    }
}

impl CaptionFontCache {
    /// Release cached source bytes, glyphs and their allocation charges.
    /// Takes this cache and returns no value; no original font file is modified.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Retain only verified immutable font bytes.
    /// `asset` pins complete source length/hash; returns owned bytes after a descriptor check.
    fn source(&mut self, asset: &AssetReference) -> Result<Arc<[u8]>> {
        if asset.kind != AssetKind::Font
            || asset.bytes == 0
            || asset.bytes > FONT_LIMIT as u64
            || asset.sha256.len() != 64
            || !asset
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(
                "Caption font needs a complete SHA-256, positive length and at most 16 MiB".into(),
            );
        }
        let mut file = File::open(&asset.path)?;
        let metadata = file.metadata()?;
        let signature = Signature::of(&metadata);
        if !metadata.is_file() || metadata.len() != asset.bytes {
            return Err("Caption font changed; retain or relink the original font".into());
        }
        if let Some(entry) = self.fonts.iter().find(|entry| {
            entry.hash == asset.sha256 && entry.path == asset.path && entry.signature == signature
        }) {
            return Ok(entry.bytes.clone());
        }
        let mut bytes = Vec::with_capacity(asset.bytes as usize);
        file.by_ref()
            .take(asset.bytes + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != asset.bytes
            || format!("{:x}", Sha256::digest(&bytes)) != asset.sha256
            || Signature::of(&file.metadata()?) != signature
        {
            return Err("Caption font changed; retain or relink the original font".into());
        }
        if Face::from_slice(&bytes, 0).is_none() {
            return Err("Caption font has no supported OpenType face".into());
        }
        let bytes: Arc<[u8]> = bytes.into();
        if let Some(entry) = self
            .fonts
            .iter_mut()
            .find(|entry| entry.hash == asset.sha256)
        {
            let charge = bytes.len() + asset.sha256.len() + asset.path.as_os_str().len() + 64;
            self.bytes -= entry.charge;
            entry.path = asset.path.clone();
            entry.signature = signature;
            entry.charge = charge;
            self.bytes += charge;
            if self.bytes > FONT_LIMIT {
                self.clear();
            }
            return Ok(bytes);
        }
        let charge = bytes.len() + asset.sha256.len() + asset.path.as_os_str().len() + 64;
        if self.fonts.len() == FONT_ENTRIES {
            self.remove_font(0);
        }
        if self.room(charge, None) {
            self.bytes += charge;
            self.fonts.push(FontEntry {
                hash: asset.sha256.clone(),
                path: asset.path.clone(),
                signature,
                bytes: bytes.clone(),
                charge,
            });
        }
        Ok(bytes)
    }

    /// Resolve one already rasterized indexed glyph.
    /// `hash`, `size` and `glyph` pin font content and pixel size; returns immutable coverage.
    fn glyph(&self, hash: &str, size: f32, glyph: u16) -> Option<GlyphImage> {
        self.glyphs
            .iter()
            .find(|entry| {
                entry.hash == hash && entry.size == size.to_bits() && entry.glyph == glyph
            })
            .map(|entry| entry.image.clone())
    }

    /// Retain a bounded glyph image when the font and bitmap fit together.
    /// The supplied font hash, pixel size, glyph and image are immutable; returns no value.
    fn retain_glyph(&mut self, hash: &str, size: f32, glyph: u16, image: &GlyphImage) {
        let charge = image.coverage.len() + hash.len() + 32;
        if self.glyphs.len() == GLYPH_ENTRIES {
            self.remove_glyph();
        }
        if self.room(charge, Some(hash)) {
            self.bytes += charge;
            self.glyphs.push(GlyphEntry {
                hash: hash.to_owned(),
                size: size.to_bits(),
                glyph,
                image: image.clone(),
                charge,
            });
        }
    }

    /// Make room without evicting the font currently being shaped.
    /// `charge` and `protected` select retention; returns false for an uncachable large glyph.
    fn room(&mut self, charge: usize, protected: Option<&str>) -> bool {
        while self
            .bytes
            .checked_add(charge)
            .is_none_or(|bytes| bytes > FONT_LIMIT)
        {
            if !self.glyphs.is_empty() {
                self.remove_glyph();
            } else if let Some(index) = self
                .fonts
                .iter()
                .position(|entry| Some(entry.hash.as_str()) != protected)
            {
                self.remove_font(index);
            } else {
                return false;
            }
        }
        true
    }

    /// Release the oldest retained glyph payload.
    /// Takes this nonempty cache and returns no value after reducing its charge.
    fn remove_glyph(&mut self) {
        self.bytes -= self.glyphs.remove(0).charge;
    }

    /// Release one retained source payload.
    /// `index` selects an existing font; returns no value after reducing its charge.
    fn remove_font(&mut self, index: usize) {
        self.bytes -= self.fonts.remove(index).charge;
    }
}

struct PositionedGlyph {
    id: u16,
    x: f32,
    y: f32,
    metrics: Metrics,
}

struct Line {
    glyphs: Vec<PositionedGlyph>,
    minimum_x: f32,
    width: f32,
    ascent: f32,
    descent: f32,
}

struct Engine<'a> {
    cache: &'a mut CaptionFontCache,
    bytes: &'a [u8],
    hash: &'a str,
    face: Face<'a>,
    outline: Option<Font>,
    size: f32,
    shape_work: usize,
    pixel_work: usize,
}

impl Engine<'_> {
    /// Lazily parse transient outline geometry on a glyph-cache miss.
    /// Takes this engine and returns the font used for exact indexed metrics/rasterization.
    fn outline(&mut self) -> Result<&Font> {
        if self.outline.is_none() {
            self.outline = Some(Font::from_bytes(self.bytes, FontSettings::default())?);
        }
        self.outline
            .as_ref()
            .ok_or_else(|| "Caption font outline is absent".into())
    }

    /// Read metrics from the same glyph ID used by OpenType shaping.
    /// `glyph` selects an already validated ID; returns indexed geometry at the caption size.
    fn metrics(&mut self, glyph: u16) -> Result<Metrics> {
        if let Some(image) = self.cache.glyph(self.hash, self.size, glyph) {
            return Ok(image.metrics);
        }
        let size = self.size;
        let metrics = self.outline()?.metrics_indexed(glyph, size);
        check_metrics(metrics)?;
        Ok(metrics)
    }

    /// Rasterize a shaped glyph with an explicit per-call work charge.
    /// `glyph` and `expected` pin indexed geometry; returns bounded immutable coverage.
    fn image(&mut self, glyph: u16, expected: Metrics) -> Result<GlyphImage> {
        let pixels = expected
            .width
            .checked_mul(expected.height)
            .ok_or("Caption glyph dimensions overflow")?;
        self.pixel_work = self
            .pixel_work
            .checked_add(pixels)
            .filter(|work| *work <= WORK_LIMIT)
            .ok_or("Caption glyph work exceeds 64 MiB")?;
        if let Some(image) = self.cache.glyph(self.hash, self.size, glyph) {
            return Ok(image);
        }
        let size = self.size;
        let (metrics, coverage) = self.outline()?.rasterize_indexed(glyph, size);
        check_metrics(metrics)?;
        if metrics != expected || coverage.len() != pixels {
            return Err("Caption glyph changed between layout and rasterization".into());
        }
        let image = GlyphImage {
            metrics,
            coverage: coverage.into(),
        };
        self.cache.retain_glyph(self.hash, self.size, glyph, &image);
        Ok(image)
    }

    /// Shape a logical line using its paragraph's bidi levels and real Unicode script runs.
    /// `text`, `bidi`, `paragraph` and `range` retain logical byte order and line context.
    /// Returns visual glyph IDs, OpenType advances/offsets, and exact bitmap extents.
    fn line(
        &mut self,
        text: &str,
        bidi: &BidiInfo<'_>,
        paragraph: &ParagraphInfo,
        range: Range<usize>,
    ) -> Result<Line> {
        self.shape_work = self
            .shape_work
            .checked_add(range.len())
            .filter(|work| *work <= SHAPE_LIMIT)
            .ok_or("Caption shaping exceeds its bounded text work; split the cue")?;
        let scale = self.size / self.face.units_per_em() as f32;
        let ascent = (f32::from(self.face.ascender()) * scale).ceil().max(0.0);
        let descent = (-f32::from(self.face.descender()) * scale).ceil().max(0.0);
        let mut line = Line {
            glyphs: Vec::new(),
            minimum_x: 0.0,
            width: 0.0,
            ascent,
            descent,
        };
        let (levels, runs) = bidi.visual_runs(paragraph, range.clone());
        let mut pen = 0.0_f32;
        let mut right = 0.0_f32;
        for run in runs {
            let rtl = levels[run.start].is_rtl();
            let mut scripts = script_runs(text, run);
            if rtl {
                scripts.reverse();
            }
            for (span, script) in scripts {
                let mut buffer = UnicodeBuffer::new();
                buffer.push_str(&text[span.clone()]);
                buffer.set_pre_context(&text[range.start..span.start]);
                buffer.set_post_context(&text[span.end..range.end]);
                buffer.set_direction(if rtl {
                    Direction::RightToLeft
                } else {
                    Direction::LeftToRight
                });
                let tag =
                    rustybuzz::ttf_parser::Tag::from_bytes(&script.as_iso15924_tag().to_be_bytes());
                buffer.set_script(
                    rustybuzz::Script::from_iso15924_tag(tag)
                        .ok_or("Caption script tag is invalid")?,
                );
                let mut flags = BufferFlags::REMOVE_DEFAULT_IGNORABLES;
                if span.start == range.start {
                    flags |= BufferFlags::BEGINNING_OF_TEXT;
                }
                if span.end == range.end {
                    flags |= BufferFlags::END_OF_TEXT;
                }
                buffer.set_flags(flags);
                buffer.guess_segment_properties();
                let shaped = rustybuzz::shape(&self.face, &[], buffer);
                if shaped.glyph_infos().len() != shaped.glyph_positions().len()
                    || line.glyphs.len() + shaped.glyph_infos().len() > TEXT_LIMIT
                {
                    return Err("Caption shaped glyph count exceeds its budget".into());
                }
                for (info, position) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                    let glyph = u16::try_from(info.glyph_id)
                        .map_err(|_| "Caption shaped glyph exceeds its font range")?;
                    if glyph == 0 || glyph >= self.face.number_of_glyphs() {
                        let source = text[span.clone()]
                            .get(info.cluster as usize..)
                            .and_then(|text| text.chars().next());
                        return Err(
                            format!("Caption font lacks a shaped glyph for {source:?}").into()
                        );
                    }
                    if position.y_advance != 0 {
                        return Err("Caption shaping requires horizontal glyph advances".into());
                    }
                    let metrics = self.metrics(glyph)?;
                    let source = text[span.clone()]
                        .get(info.cluster as usize..)
                        .and_then(|text| text.chars().next());
                    if (metrics.width == 0 || metrics.height == 0)
                        && !source.is_some_and(char::is_whitespace)
                    {
                        return Err(format!(
                            "Caption glyph for {source:?} has no supported outline"
                        )
                        .into());
                    }
                    let x = (pen + position.x_offset as f32 * scale).round();
                    let y = (-position.y_offset as f32 * scale).round();
                    if !x.is_finite()
                        || !y.is_finite()
                        || x.abs() > 16_777_216.0
                        || y.abs() > 16_777_216.0
                    {
                        return Err(
                            "Caption glyph offset exceeds its integer coordinate budget".into()
                        );
                    }
                    let left = x + metrics.xmin as f32;
                    let top = y - metrics.height as f32 - metrics.ymin as f32;
                    line.minimum_x = line.minimum_x.min(left);
                    right = right.max(left + metrics.width as f32);
                    line.ascent = line.ascent.max(-top);
                    line.descent = line.descent.max(top + metrics.height as f32);
                    line.glyphs.push(PositionedGlyph {
                        id: glyph,
                        x,
                        y,
                        metrics,
                    });
                    pen += position.x_advance as f32 * scale;
                    if !pen.is_finite() || pen.abs() > 1.0e9 {
                        return Err("Caption shaping exceeds its coordinate budget".into());
                    }
                }
            }
        }
        line.width = (right.max(pen.ceil()) - line.minimum_x).ceil();
        Ok(line)
    }

    /// Wrap one paragraph at real grapheme boundaries and prefer complete words.
    /// `text` retains logical order and `maximum` is safe-area width in pixels.
    /// Returns reshaped visual lines without splitting combining or joined graphemes.
    fn wrap(&mut self, text: &str, maximum: f32) -> Result<Vec<Line>> {
        let bidi = BidiInfo::new(text, None);
        let paragraph = bidi
            .paragraphs
            .first()
            .ok_or("Caption paragraph is empty")?;
        if bidi.paragraphs.len() != 1 {
            return Err(
                "Caption paragraph contains an unsupported paragraph separator; use a newline"
                    .into(),
            );
        }
        let mut boundaries: Vec<_> = text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .collect();
        boundaries.push(text.len());
        let mut start = 0;
        let mut lines = Vec::new();
        while start + 1 < boundaries.len() {
            let mut low = start + 1;
            let mut high = boundaries.len() - 1;
            let mut fit = None;
            while low <= high {
                let middle = low + (high - low) / 2;
                let line = self.line(
                    text,
                    &bidi,
                    paragraph,
                    boundaries[start]..boundaries[middle],
                )?;
                if line.width <= maximum {
                    fit = Some((middle, line));
                    low = middle + 1;
                } else {
                    if middle == 0 {
                        break;
                    }
                    high = middle - 1;
                }
            }
            let (mut end, mut line) =
                fit.ok_or("Caption grapheme cannot fit inside its safe area; reduce size")?;
            if end + 1 < boundaries.len()
                && let Some(word_end) = (start + 1..=end).rev().find(|index| {
                    text[boundaries[index - 1]..boundaries[*index]]
                        .chars()
                        .all(char::is_whitespace)
                })
                && text[boundaries[start]..boundaries[word_end]]
                    .chars()
                    .any(|character| !character.is_whitespace())
            {
                end = word_end;
                line = self.line(text, &bidi, paragraph, boundaries[start]..boundaries[end])?;
            }
            lines.push(line);
            if lines.len() > 8192 {
                return Err("Caption line count exceeds its canvas budget".into());
            }
            start = end;
        }
        Ok(lines)
    }
}

/// Segment a bidi run using real Unicode script properties on whole grapheme clusters.
/// `text` and `range` retain logical byte boundaries; returns resolved ISO-script spans.
fn script_runs(text: &str, range: Range<usize>) -> Vec<(Range<usize>, Script)> {
    let mut clusters: Vec<_> = text[range.clone()]
        .grapheme_indices(true)
        .map(|(index, cluster)| {
            let script = cluster
                .chars()
                .map(|character| character.script())
                .find(|script| {
                    !matches!(script, Script::Common | Script::Inherited | Script::Unknown)
                });
            (range.start + index, script)
        })
        .collect();
    let first = clusters
        .iter()
        .find_map(|(_, script)| *script)
        .unwrap_or(Script::Common);
    let mut previous = first;
    for (_, script) in &mut clusters {
        if let Some(value) = script {
            previous = *value;
        } else {
            *script = Some(previous);
        }
    }
    let mut runs: Vec<(Range<usize>, Script)> = Vec::new();
    for (index, (start, script)) in clusters.iter().enumerate() {
        let end = clusters
            .get(index + 1)
            .map_or(range.end, |(start, _)| *start);
        let script = script.unwrap_or(Script::Common);
        if let Some((last, previous)) = runs.last_mut()
            && *previous == script
        {
            last.end = end;
        } else {
            runs.push((*start..end, script));
        }
    }
    runs
}

/// Validate indexed glyph allocation before fontdue creates its coverage buffer.
/// `metrics` belongs to a shaped glyph; returns an error for oversized or nonfinite geometry.
fn check_metrics(metrics: Metrics) -> Result<()> {
    if metrics.width > 8192
        || metrics.height > 8192
        || metrics.xmin.unsigned_abs() > 16_777_216
        || metrics.ymin.unsigned_abs() > 16_777_216
        || metrics
            .width
            .checked_mul(metrics.height)
            .is_none_or(|pixels| pixels > WORK_LIMIT)
        || !metrics.advance_width.is_finite()
        || !metrics.advance_height.is_finite()
    {
        return Err("Caption glyph geometry exceeds its work budget".into());
    }
    Ok(())
}

/// Shape, wrap and rasterize verified captions into straight linear-color RGBA8.
/// `cache`, `asset`, `track`, active `cues`, `width` and `height` bind source and geometry.
/// Returns real bidi/OpenType glyph pixels, rejecting missing glyphs and unsafe resource use.
pub(crate) fn rasterize(
    cache: &mut CaptionFontCache,
    asset: &AssetReference,
    track: &CaptionTrack,
    cues: &[&CaptionCue],
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    track.validate_burn_in()?;
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err("Caption canvas needs dimensions from 1 through 8192".into());
    }
    let count = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("Caption canvas allocation overflows")?;
    if count > 256 * 1024 * 1024 {
        return Err("Caption canvas exceeds its allocation budget".into());
    }
    if track.style.font.is_some_and(|id| id != asset.id) {
        return Err("Caption font belongs to a different retained asset".into());
    }
    let mut ids = HashSet::new();
    let mut text_bytes = 0usize;
    for cue in cues {
        if !ids.insert(cue.id) || !track.cues.contains(cue) {
            return Err("Caption cue belongs to a different retained track".into());
        }
        text_bytes = text_bytes
            .checked_add(cue.text.len())
            .filter(|bytes| *bytes <= TEXT_LIMIT)
            .ok_or("Active caption text exceeds 64 KiB")?;
    }
    let mut pixels = vec![0; count];
    if cues.is_empty() {
        return Ok(pixels);
    }
    let area = track.style.safe_area.bounds(width, height)?;
    let safe = [
        area[0].ceil() as i32,
        area[1].ceil() as i32,
        area[2].floor() as i32,
        area[3].floor() as i32,
    ];
    if safe[0] >= safe[2] || safe[1] >= safe[3] {
        return Err("Caption safe area contains no complete pixel rectangle".into());
    }
    let bytes = cache.source(asset)?;
    let face = Face::from_slice(&bytes, 0).ok_or("Caption font has no supported OpenType face")?;
    let mut engine = Engine {
        cache,
        bytes: &bytes,
        hash: &asset.sha256,
        face,
        outline: None,
        size: track.style.font_size,
        shape_work: 0,
        pixel_work: 0,
    };
    let mut lines = Vec::new();
    for cue in cues {
        for paragraph in cue.text.split('\n') {
            lines.extend(engine.wrap(paragraph, (safe[2] - safe[0]) as f32)?);
        }
    }
    let heights: Vec<_> = lines
        .iter()
        .map(|line| {
            (track.style.font_size * track.style.line_height)
                .max(line.ascent + line.descent)
                .ceil() as i32
        })
        .collect();
    let total_height = heights
        .iter()
        .try_fold(0_i32, |sum, height| sum.checked_add(*height))
        .ok_or("Caption block height exceeds integer range")?;
    if total_height > safe[3] - safe[1] {
        return Err("Caption text exceeds its safe area; reduce size or split the cue".into());
    }
    let mut rows = Vec::with_capacity(lines.len());
    let mut top = safe[3] - total_height;
    for (line, height) in lines.iter().zip(&heights) {
        let left = match track.style.alignment {
            CaptionAlignment::Left => safe[0],
            CaptionAlignment::Center => {
                safe[0] + (((safe[2] - safe[0]) as f32 - line.width) * 0.5).floor() as i32
            }
            CaptionAlignment::Right => safe[2] - line.width as i32,
        };
        let baseline = top
            + (((*height as f32 - line.ascent - line.descent) * 0.5) + line.ascent).round() as i32;
        rows.push((left, top, baseline));
        let padding = (track.style.font_size * 0.12).ceil().max(1.0) as i32;
        for y in (top - padding).max(safe[1])..(top + height + padding).min(safe[3]) {
            for x in
                (left - padding).max(safe[0])..(left + line.width as i32 + padding).min(safe[2])
            {
                blend(
                    &mut pixels[(y as usize * width as usize + x as usize) * 4..][..4],
                    track.style.background,
                    1.0,
                );
            }
        }
        top += height;
    }
    for (line, (left, _, baseline)) in lines.iter().zip(rows) {
        for glyph in &line.glyphs {
            let image = engine.image(glyph.id, glyph.metrics)?;
            let x = left - line.minimum_x as i32 + glyph.x as i32 + glyph.metrics.xmin;
            let y = baseline + glyph.y as i32 - glyph.metrics.height as i32 - glyph.metrics.ymin;
            if x < safe[0]
                || y < safe[1]
                || x + glyph.metrics.width as i32 > safe[2]
                || y + glyph.metrics.height as i32 > safe[3]
            {
                return Err("Caption shaped glyph exceeds its safe-area rectangle".into());
            }
            for row in 0..glyph.metrics.height {
                for column in 0..glyph.metrics.width {
                    let coverage = image.coverage[row * glyph.metrics.width + column];
                    if coverage == 0 {
                        continue;
                    }
                    let at = (((y as usize + row) * width as usize) + x as usize + column) * 4;
                    blend(
                        &mut pixels[at..at + 4],
                        track.style.foreground,
                        f32::from(coverage) / 255.0,
                    );
                }
            }
        }
    }
    Ok(pixels)
}

/// Composite one straight-color caption sample with correct combined alpha.
/// `pixel`, linear `color` and coverage select source-over blending; returns no value.
fn blend(pixel: &mut [u8], color: [f32; 4], coverage: f32) {
    let alpha = color[3] * coverage;
    if alpha <= 0.0 {
        return;
    }
    let old = f32::from(pixel[3]) / 255.0;
    let combined = alpha + old * (1.0 - alpha);
    for (output, value) in pixel[..3].iter_mut().zip(&color[..3]) {
        *output = ((*value * alpha + f32::from(*output) / 255.0 * old * (1.0 - alpha)) / combined
            * 255.0)
            .round() as u8;
    }
    pixel[3] = (combined * 255.0).round() as u8;
}

#[cfg(test)]
mod tests {
    use super::*;
    use editbay_core::captions::CaptionTime;
    use uuid::Uuid;

    /// Locate the licensed font already retained by this repository.
    /// Takes no arguments; returns the existing Liberation fixture path without copying it.
    fn latin_font() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/liberation/LiberationSans-Regular.ttf")
    }

    /// Pin a real existing font for CPU qualification.
    /// `path` identifies source bytes; returns the exact asset length and complete SHA-256.
    fn asset(path: PathBuf) -> AssetReference {
        let bytes = std::fs::read(&path).expect("The caption CPU font fixture must be installed");
        AssetReference { id: Uuid::from_u128(99), kind: AssetKind::Font, path, bytes: bytes.len() as u64, sha256: format!("{:x}", Sha256::digest(&bytes)), provenance: "Read-only CPU font qualification; repository Liberation is OFL, installed Noto package has Apache-2.0 license in /usr/share/licenses/noto-fonts/LICENSE".into() }
    }

    /// Create one retained plain cue with explicit owned font and appearance.
    /// `font` and `text` select the fixture; returns an editable track for CPU raster tests.
    fn track(font: &AssetReference, text: &str) -> CaptionTrack {
        let mut track = CaptionTrack::new(Uuid::from_u128(1), "Shaped captions").unwrap();
        track.style.font = Some(font.id);
        track.style.font_size = 36.0;
        track.style.background = [0.0; 4];
        track
            .insert_cue(
                CaptionCue::new(
                    CaptionTime::from_milliseconds(0),
                    CaptionTime::from_milliseconds(2000),
                    text,
                )
                .unwrap(),
            )
            .unwrap();
        track
    }

    /// Inspect real OpenType glyph output without GPU or device work.
    /// `bytes`, `text` and `size` supply a font and logical paragraph; returns the shaped line.
    fn shaped(bytes: &[u8], text: &str, size: f32) -> Line {
        let mut cache = CaptionFontCache::default();
        let mut engine = Engine {
            cache: &mut cache,
            bytes,
            hash: "cpu-shape",
            face: Face::from_slice(bytes, 0).unwrap(),
            outline: None,
            size,
            shape_work: 0,
            pixel_work: 0,
        };
        let bidi = BidiInfo::new(text, None);
        engine
            .line(text, &bidi, &bidi.paragraphs[0], 0..text.len())
            .unwrap()
    }

    #[test]
    fn arabic_matches_pinned_pango_joining_and_visual_digit_order() {
        let font = asset(PathBuf::from(
            "/usr/share/fonts/noto/NotoSansArabic-Regular.ttf",
        ));
        let reference: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/caption-arabic-pango.json"))
                .unwrap();
        assert_eq!(font.sha256, reference["font"]["sha256"].as_str().unwrap());
        assert_eq!(font.bytes, reference["font"]["bytes"].as_u64().unwrap());
        let bytes = std::fs::read(&font.path).unwrap();
        let face = Face::from_slice(&bytes, 0).unwrap();
        for case in reference["cases"].as_array().unwrap() {
            let text = case["text"].as_str().unwrap();
            let line = shaped(&bytes, text, 48.0);
            let expected: Vec<_> = case["glyphs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| u16::try_from(id.as_u64().unwrap()).unwrap())
                .collect();
            assert_eq!(
                line.glyphs.iter().map(|glyph| glyph.id).collect::<Vec<_>>(),
                expected
            );
        }
        let unjoined: Vec<_> = "سلام"
            .chars()
            .rev()
            .map(|character| face.glyph_index(character).unwrap().0)
            .collect();
        assert_ne!(
            shaped(&bytes, "سلام", 48.0)
                .glyphs
                .iter()
                .map(|glyph| glyph.id)
                .collect::<Vec<_>>(),
            unjoined
        );
        let track = track(&font, "سلام 123");
        let pixels = rasterize(
            &mut CaptionFontCache::default(),
            &font,
            &track,
            &[&track.cues[0]],
            640,
            240,
        )
        .unwrap();
        assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }

    #[test]
    fn mixed_hebrew_latin_reorders_runs_and_retains_opentype_kerning() {
        let bytes = std::fs::read(latin_font()).unwrap();
        let face = Face::from_slice(&bytes, 0).unwrap();
        let mixed = shaped(&bytes, "ABC אבג 12", 48.0);
        let actual: Vec<_> = mixed
            .glyphs
            .iter()
            .map(|glyph| glyph.id)
            .filter(|glyph| *glyph != face.glyph_index(' ').unwrap().0)
            .collect();
        let expected: Vec<_> = "ABC12גבא"
            .chars()
            .map(|character| face.glyph_index(character).unwrap().0)
            .collect();
        assert_eq!(actual, expected);
        let kerned = shaped(&bytes, "AV", 64.0);
        let unkerned = f32::from(
            face.glyph_hor_advance(face.glyph_index('A').unwrap())
                .unwrap(),
        ) * 64.0
            / face.units_per_em() as f32;
        assert!(kerned.glyphs[1].x < unkerned.round());
    }

    #[test]
    fn combining_graphemes_stay_together_when_wrapped() {
        let bytes = std::fs::read(latin_font()).unwrap();
        let face = Face::from_slice(&bytes, 0).unwrap();
        let composed = shaped(&bytes, "e\u{301}", 36.0);
        assert_eq!(composed.glyphs.len(), 1);
        assert_eq!(composed.glyphs[0].id, face.glyph_index('é').unwrap().0);
        let mut cache = CaptionFontCache::default();
        let mut engine = Engine {
            cache: &mut cache,
            bytes: &bytes,
            hash: "cpu-wrap",
            face,
            outline: None,
            size: 36.0,
            shape_work: 0,
            pixel_work: 0,
        };
        let lines = engine.wrap("e\u{301}e\u{301}", composed.width).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(
            lines
                .iter()
                .all(|line| line.glyphs.len() == 1 && line.glyphs[0].id == composed.glyphs[0].id)
        );
    }

    #[test]
    fn wrapped_pixels_and_background_stay_inside_every_safe_area_edge() {
        let font = asset(latin_font());
        let mut track = track(
            &font,
            "Café AV e\u{301} caption words wrap safely across several lines",
        );
        track.style.font_size = 24.0;
        track.style.foreground = [1.0, 0.0, 0.0, 0.5];
        track.style.background = [0.0, 0.0, 1.0, 0.25];
        let mut cache = CaptionFontCache::default();
        for alignment in [
            CaptionAlignment::Left,
            CaptionAlignment::Center,
            CaptionAlignment::Right,
        ] {
            track.style.alignment = alignment;
            let pixels = rasterize(&mut cache, &font, &track, &[&track.cues[0]], 320, 240).unwrap();
            assert!(
                pixels
                    .chunks_exact(4)
                    .any(|pixel| pixel[0] > 0 && pixel[2] > 0)
            );
            for (index, pixel) in pixels.chunks_exact(4).enumerate() {
                if pixel[3] != 0 {
                    assert!((32..288).contains(&(index % 320)));
                    assert!((24..216).contains(&(index / 320)));
                }
            }
            assert!(cache.bytes <= FONT_LIMIT);
        }
    }

    #[test]
    fn complete_font_hash_and_cached_file_ownership_detect_changed_bytes() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
        std::fs::create_dir_all(&root).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("caption-font-cpu-")
            .tempdir_in(root)
            .unwrap();
        let path = directory.path().join("owned.ttf");
        let mut bytes = std::fs::read(latin_font()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let font = asset(path.clone());
        let track = track(&font, "Pinned font");
        let mut cache = CaptionFontCache::default();
        rasterize(&mut cache, &font, &track, &[&track.cues[0]], 640, 240).unwrap();
        bytes[100] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert!(rasterize(&mut cache, &font, &track, &[&track.cues[0]], 640, 240).is_err());
        let mut wrong_length = font.clone();
        wrong_length.bytes += 1;
        assert!(cache.source(&wrong_length).is_err());
        let mut wrong_hash = font;
        wrong_hash.sha256 = "0".repeat(64);
        assert!(cache.source(&wrong_hash).is_err());
    }

    #[test]
    fn missing_glyphs_oversized_geometry_and_foreign_cues_fail_explicitly() {
        let font = asset(latin_font());
        let unsupported = track(&font, "世界");
        assert!(
            rasterize(
                &mut CaptionFontCache::default(),
                &font,
                &unsupported,
                &[&unsupported.cues[0]],
                640,
                240
            )
            .is_err()
        );
        let mut oversized = track(&font, "W");
        oversized.style.font_size = 4096.0;
        assert!(
            rasterize(
                &mut CaptionFontCache::default(),
                &font,
                &oversized,
                &[&oversized.cues[0]],
                16,
                16
            )
            .is_err()
        );
        let normal = track(&font, "Valid");
        assert!(
            rasterize(
                &mut CaptionFontCache::default(),
                &font,
                &normal,
                &[&normal.cues[0]],
                8193,
                1
            )
            .is_err()
        );
        let foreign = CaptionCue::new(
            CaptionTime::from_milliseconds(0),
            CaptionTime::from_milliseconds(1000),
            "Foreign",
        )
        .unwrap();
        assert!(
            rasterize(
                &mut CaptionFontCache::default(),
                &font,
                &normal,
                &[&foreign],
                640,
                240
            )
            .is_err()
        );
    }

    #[test]
    fn retained_payload_and_glyph_work_limits_are_enforced_independently() {
        let font = asset(latin_font());
        let mut cache = CaptionFontCache::default();
        let bytes = cache.source(&font).unwrap();
        let image = GlyphImage {
            metrics: Metrics::default(),
            coverage: vec![0; 2 * 1024 * 1024].into(),
        };
        for glyph in 1..20 {
            cache.retain_glyph(&font.sha256, 48.0, glyph, &image);
            assert!(cache.bytes <= FONT_LIMIT);
        }
        let face = Face::from_slice(&bytes, 0).unwrap();
        let glyph = face.glyph_index('A').unwrap().0;
        let mut engine = Engine {
            cache: &mut cache,
            bytes: &bytes,
            hash: &font.sha256,
            face,
            outline: None,
            size: 48.0,
            shape_work: 0,
            pixel_work: WORK_LIMIT,
        };
        let metrics = engine.metrics(glyph).unwrap();
        assert!(engine.image(glyph, metrics).is_err());
        cache.clear();
        assert!(cache.fonts.is_empty() && cache.glyphs.is_empty());
        assert!(cache.bytes < FONT_LIMIT);
    }

    #[test]
    fn caption_color_compositing_preserves_straight_alpha() {
        let mut pixel = [0; 4];
        blend(&mut pixel, [0.0, 0.0, 1.0, 0.5], 1.0);
        assert_eq!(pixel, [0, 0, 255, 128]);
        blend(&mut pixel, [1.0, 0.0, 0.0, 0.5], 1.0);
        assert_eq!(pixel, [170, 0, 85, 192]);
    }
}
