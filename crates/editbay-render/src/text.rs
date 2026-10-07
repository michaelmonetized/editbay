use crate::Result;
use editbay_core::AssetReference;
use fontdue::{Font, FontSettings};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, io::Read};

#[derive(Default)]
pub(crate) struct FontCache {
    faces: HashMap<String, (Font, u64)>,
    bytes: u64,
}
impl FontCache {
    /// Retain bounded verified font faces for repeated title rendering.
    /// `asset` pins its original bytes; returns the same immutable face for its hash.
    fn face(&mut self, asset: &AssetReference) -> Result<&Font> {
        if !self.faces.contains_key(&asset.sha256) {
            if asset.bytes > 16 * 1024 * 1024 {
                return Err("Title font exceeds its limit".into());
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&asset.path)?
                .take(asset.bytes + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 != asset.bytes
                || format!("{:x}", Sha256::digest(&bytes)) != asset.sha256
            {
                return Err("Title font changed; retain or relink the original font".into());
            }
            let font = Font::from_bytes(bytes, FontSettings::default())?;
            while self.faces.len() >= 8 || self.bytes + asset.bytes > 16 * 1024 * 1024 {
                let key = self
                    .faces
                    .keys()
                    .next()
                    .ok_or("Font cache budget is invalid")?
                    .clone();
                let (_, bytes) = self.faces.remove(&key).unwrap();
                self.bytes -= bytes;
            }
            self.bytes += asset.bytes;
            self.faces.insert(asset.sha256.clone(), (font, asset.bytes));
        }
        Ok(&self.faces[&asset.sha256].0)
    }
    pub(crate) fn clear(&mut self) {
        self.faces.clear();
        self.bytes = 0;
    }
}

/// Rasterize a retained title font into bounded straight coverage pixels.
/// `asset` supplies a verified font; `text`, `size`, `position` and geometry
/// select ASCII glyphs, pixel size, baseline origin and canvas. Returns RGBA8
/// white coverage. Missing glyphs and changed fonts fail before GPU publication.
pub(crate) fn rasterize(
    cache: &mut FontCache,
    asset: &AssetReference,
    text: &str,
    size: f64,
    position: [f64; 2],
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    if asset.bytes > 16 * 1024 * 1024 || width > 8192 || height > 8192 {
        return Err("Title exceeds font or canvas limits".into());
    }
    let font = cache.face(asset)?;
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let mut pen = position;
    let mut previous = None;
    let mut work = 0usize;
    let line = font
        .horizontal_line_metrics(size as f32)
        .ok_or("Title font has no horizontal metrics")?
        .new_line_size as f64;
    for character in text.chars() {
        if character == '\n' {
            pen[0] = position[0];
            pen[1] += line;
            previous = None;
            continue;
        }
        if !font.has_glyph(character) {
            return Err(format!("Title font lacks {character:?}").into());
        }
        if let Some(previous) = previous {
            pen[0] += f64::from(
                font.horizontal_kern(previous, character, size as f32)
                    .unwrap_or(0.),
            );
        }
        let metrics = font.metrics(character, size as f32);
        work = work
            .checked_add(
                metrics
                    .width
                    .checked_mul(metrics.height)
                    .ok_or("Title glyph overflow")?,
            )
            .filter(|work| *work <= 64 * 1024 * 1024)
            .ok_or("Title glyph work exceeds its budget")?;
        if metrics.width > 8192 || metrics.height > 8192 || !metrics.advance_width.is_finite() {
            return Err("Title glyph exceeds canvas limits".into());
        }
        let (metrics, coverage) = font.rasterize(character, size as f32);
        let left = pen[0].round() as i64 + i64::from(metrics.xmin);
        let top = pen[1].round() as i64 - metrics.height as i64 - i64::from(metrics.ymin);
        for y in 0..metrics.height {
            for x in 0..metrics.width {
                let (xx, yy) = (left + x as i64, top + y as i64);
                if xx < 0 || yy < 0 || xx >= i64::from(width) || yy >= i64::from(height) {
                    continue;
                }
                let at = (yy as usize * width as usize + xx as usize) * 4;
                let alpha = coverage[y * metrics.width + x];
                let old = pixels[at + 3];
                pixels[at..at + 3].fill(255);
                pixels[at + 3] =
                    255 - (((255 - u16::from(old)) * (255 - u16::from(alpha)) + 127) / 255) as u8;
            }
        }
        pen[0] += f64::from(metrics.advance_width);
        previous = Some(character);
    }
    Ok(pixels)
}
