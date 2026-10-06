use crate::{Error, Result};
use editbay_core::FrameRate;
use serde::{Deserialize, Serialize};

/// Exact bounds for a lossless SDR PNG/RGBA8 and float-PCM MOV file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LosslessMovProfile {
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    #[serde(default)]
    pub first_frame: u64,
    pub frames: u64,
    pub sample_rate: u32,
    pub channels: Vec<String>,
}

impl LosslessMovProfile {
    /// Validate geometry, exact clocks and a supported original channel order.
    /// Takes this profile; returns an error before opening any destination.
    pub fn validate(&self) -> Result<()> {
        let rate = self.frame_rate;
        let allowed: &[&[&str]] = &[
            &["FC"],
            &["FL", "FR"],
            &["FL", "FR", "BL", "BR"],
            &["FL", "FR", "FC", "BL", "BR"],
            &["FL", "FR", "FC", "LFE", "BL", "BR"],
            &["FL", "FR", "FC", "SL", "SR"],
            &["FL", "FR", "FC", "LFE", "SL", "SR"],
            &["FL", "FR", "FC", "LFE", "BL", "BR", "SL", "SR"],
        ];
        if !(1..=8192).contains(&self.width)
            || !(1..=8192).contains(&self.height)
            || self.frames == 0
            || self.frames > i64::MAX as u64
            || self
                .first_frame
                .checked_add(self.frames)
                .is_none_or(|end| end > i64::MAX as u64)
            || rate.denominator == 0
            || rate.numerator < rate.denominator
            || u64::from(rate.numerator) > 240 * u64::from(rate.denominator)
            || rate.numerator > i32::MAX as u32
            || !(8000..=384000).contains(&self.sample_rate)
            || !allowed.iter().any(|layout| *layout == self.channels)
        {
            return Err(Error::Invalid(
                "unsupported lossless MOV geometry, clock or channel order".into(),
            ));
        }
        let mut a = rate.numerator;
        let mut b = self.sample_rate;
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let timescale = u64::from(rate.numerator / a) * u64::from(self.sample_rate);
        if timescale > i32::MAX as u64
            || self
                .sample_origin()?
                .checked_add(self.samples_through(self.frames)?)
                .is_none_or(|end| end > i64::MAX as u64)
        {
            return Err(Error::Invalid(
                "lossless MOV time range exceeds native limits".into(),
            ));
        }
        Ok(())
    }

    /// Resolve the exact exclusive sound boundary at a picture boundary.
    /// `frame` is relative to this delivery; returns the selected composition-grid
    /// sample count. Output timestamps begin at zero without re-rounding its phase.
    pub fn samples_through(&self, frame: u64) -> Result<u64> {
        if frame > self.frames {
            return Err(Error::Invalid("delivery frame is outside its range".into()));
        }
        let end = self
            .first_frame
            .checked_add(frame)
            .ok_or_else(|| Error::Invalid("delivery frame range overflow".into()))?;
        Ok(self.sample_boundary(end)? - self.sample_origin()?)
    }

    /// Resolve the first selected sample on the unchanged composition grid.
    /// Takes this profile; returns an absolute sample ordinal, not an output PTS.
    pub fn sample_origin(&self) -> Result<u64> {
        self.sample_boundary(self.first_frame)
    }

    fn sample_boundary(&self, frame: u64) -> Result<u64> {
        if self.frame_rate.numerator == 0 {
            return Err(Error::Invalid("delivery has no frame rate".into()));
        }
        u64::try_from(
            (u128::from(frame)
                * u128::from(self.sample_rate)
                * u128::from(self.frame_rate.denominator))
            .div_ceil(u128::from(self.frame_rate.numerator)),
        )
        .map_err(|_| Error::Invalid("delivery sound range overflow".into()))
    }
}
