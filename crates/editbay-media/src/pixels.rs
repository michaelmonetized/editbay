use crate::{Error, Result};
use serde::{Deserialize, Serialize};

/// Native decoded component storage, separate from color interpretation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelEncoding {
    #[default]
    Rgba8,
    Rgba16Le,
    RgbaF32Le,
}

impl PixelEncoding {
    /// Return the allocation size of one pixel.
    /// Takes this encoding and returns bytes for four interleaved components.
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 => 4,
            Self::Rgba16Le => 8,
            Self::RgbaF32Le => 16,
        }
    }

    pub(crate) fn native(self) -> i32 {
        match self {
            Self::Rgba8 => 0,
            Self::Rgba16Le => 1,
            Self::RgbaF32Le => 2,
        }
    }

    pub(crate) fn from_native(value: i32) -> Result<Self> {
        match value {
            0 => Ok(Self::Rgba8),
            1 => Ok(Self::Rgba16Le),
            2 => Ok(Self::RgbaF32Le),
            _ => Err(Error::Invalid("unsupported native pixel encoding".into())),
        }
    }

    pub(crate) fn captured(metadata: &std::collections::BTreeMap<String, String>) -> Result<Self> {
        metadata
            .get("editbay.pixel_encoding")
            .map_or(Ok(Self::Rgba8), |value| {
                serde_json::from_value(serde_json::Value::String(value.clone()))
                    .map_err(|_| Error::Invalid("invalid captured pixel encoding".into()))
            })
    }

    /// Normalize native pixels without reducing their component precision.
    /// `bytes` contains complete pixels. Returns finite, straight RGBA floats;
    /// floating source headroom and negative values are preserved.
    pub fn to_f32(self, bytes: &[u8]) -> Result<Vec<f32>> {
        if !bytes.len().is_multiple_of(self.bytes_per_pixel()) {
            return Err(Error::Invalid("unaligned native pixel payload".into()));
        }
        let values: Vec<f32> = match self {
            Self::Rgba8 => bytes.iter().map(|v| f32::from(*v) / 255.).collect(),
            Self::Rgba16Le => bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|v| f32::from(u16::from_le_bytes(*v)) / 65535.)
                .collect(),
            Self::RgbaF32Le => bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| f32::from_le_bytes(*v))
                .collect(),
        };
        if values.iter().any(|v| !v.is_finite()) {
            return Err(Error::Invalid("nonfinite native pixel component".into()));
        }
        Ok(values)
    }
}
