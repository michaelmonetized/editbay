use crate::{Error, Result};
use serde::{Deserialize, Serialize};

/// Mastering display values in the exact integer units used by delivery tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MasteringDisplay {
    pub primaries_xy_50000: [[u32; 2]; 3],
    pub white_xy_50000: [u32; 2],
    pub minimum_luminance_10000: u32,
    pub maximum_luminance_10000: u32,
}

/// Declared content-light bounds in cd/m²; zero retains the standard unknown value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ContentLight {
    pub maximum_content_light_nits: u32,
    pub maximum_frame_average_light_nits: u32,
}

/// Optional static HDR metadata without inventing measurements from rendered pixels.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HdrMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mastering_display: Option<MasteringDisplay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_light: Option<ContentLight>,
}

impl HdrMetadata {
    /// Identify an absent declaration without replacing unknown values with guesses.
    /// Takes this metadata; returns true only when neither optional tag is present.
    pub fn is_empty(&self) -> bool {
        self.mastering_display.is_none() && self.content_light.is_none()
    }

    /// Validate bounded static metadata before accepting source or output commands.
    /// Takes this declaration; returns supported complete xy/luminance and light bounds.
    pub fn validate(self) -> Result<()> {
        if let Some(display) = self.mastering_display {
            let valid_xy = |xy: [u32; 2]| {
                xy[0] <= 50000 && xy[1] > 0 && xy[1] <= 50000 && xy[0] + xy[1] <= 50000
            };
            let [r, g, b] = display.primaries_xy_50000.map(|xy| xy.map(i64::from));
            let area = (g[0] - r[0]) * (b[1] - r[1]) - (g[1] - r[1]) * (b[0] - r[0]);
            if !display.primaries_xy_50000.into_iter().all(valid_xy)
                || !valid_xy(display.white_xy_50000)
                || area == 0
                || display.maximum_luminance_10000 == 0
                || display.maximum_luminance_10000 > 100_000_000
                || display.minimum_luminance_10000 >= display.maximum_luminance_10000
            {
                return Err(Error::Invalid(
                    "HDR mastering display needs complete bounded primaries, white and luminance"
                        .into(),
                ));
            }
        }
        if let Some(light) = self.content_light {
            if light.maximum_content_light_nits > 10000
                || light.maximum_frame_average_light_nits > 10000
                || (light.maximum_content_light_nits != 0
                    && light.maximum_frame_average_light_nits > light.maximum_content_light_nits)
            {
                return Err(Error::Invalid(
                    "HDR content-light levels exceed their supported nit bounds".into(),
                ));
            }
        }
        Ok(())
    }
}
