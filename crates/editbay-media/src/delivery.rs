use crate::{Error, Result};
use editbay_core::FrameRate;
use serde::{Deserialize, Serialize};

/// Declared RGB output tags; matrix and range follow the selected encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryColor {
    pub primaries: i32,
    pub transfer: i32,
    #[serde(default)]
    pub hdr_metadata: editbay_core::HdrMetadata,
}

impl Default for DeliveryColor {
    fn default() -> Self {
        Self {
            primaries: 1,
            transfer: 13,
            hdr_metadata: Default::default(),
        }
    }
}

impl DeliveryColor {
    /// Reject unimplemented color encodings before opening output storage.
    /// `format` selects the codec; returns success only for explicit RGB tags or legacy SDR.
    pub fn validate(self, format: DeliveryFormat) -> Result<()> {
        self.hdr_metadata.validate()?;
        if ![1, 9, 12].contains(&self.primaries)
            || ![1, 8, 13, 16, 18].contains(&self.transfer)
            || (format.dnxhr()
                && (self.primaries != 1 || self.transfer != 1 || !self.hdr_metadata.is_empty()))
            || (!format.dnxhr()
                && format != DeliveryFormat::Lossless16Mov
                && self != Self::default())
            || (!self.hdr_metadata.is_empty() && ![16, 18].contains(&self.transfer))
        {
            return Err(Error::Invalid(
                "delivery format does not support this color profile".into(),
            ));
        }
        Ok(())
    }
}

/// A declared client encoding, with an explicit container and alpha policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryFormat {
    #[default]
    LosslessMov,
    Lossless16Mov,
    H264Mp4,
    ProresMov,
    DnxhrHqxMov,
    DnxhrHqxMxf,
}

impl DeliveryFormat {
    /// Return native picture storage at the encoder boundary.
    /// Takes this format and returns its declared interleaved component encoding.
    pub fn pixel_encoding(self) -> crate::PixelEncoding {
        if self == Self::Lossless16Mov || self.dnxhr() {
            crate::PixelEncoding::Rgba16Le
        } else {
            crate::PixelEncoding::Rgba8
        }
    }

    /// Identify the declared progressive 10-bit 4:2:2 native master profiles.
    /// Takes this format; returns true only for the explicitly supported DNxHR presets.
    pub fn dnxhr(self) -> bool {
        matches!(self, Self::DnxhrHqxMov | Self::DnxhrHqxMxf)
    }

    /// Declare full-scale integer PCM rather than implying float-PCM preservation.
    /// Takes this format; returns whether samples are quantized to signed 24-bit PCM.
    pub fn pcm24(self) -> bool {
        self.dnxhr()
    }

    /// Reject transparency for a codec without an alpha channel.
    /// Takes this format; returns true when an opaque graph or explicit background is required.
    pub fn requires_opaque(self) -> bool {
        self.dnxhr()
    }

    /// Identify formats whose decoded picture bytes must match exactly.
    /// Takes this format and returns whether delivery uses lossless picture QC.
    pub fn lossless(self) -> bool {
        matches!(self, Self::LosslessMov | Self::Lossless16Mov)
    }
    /// Return the required filename suffix for this encoding.
    /// Takes this format; returns a lowercase extension without a dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::H264Mp4 => "mp4",
            Self::DnxhrHqxMxf => "mxf",
            _ => "mov",
        }
    }

    /// Validate codec-specific dimensions, rates and channel layouts.
    /// `profile` declares exact output clocks; returns failure before encoding.
    pub fn validate(self, profile: &LosslessMovProfile) -> Result<()> {
        profile.validate()?;
        if self.dnxhr() {
            let supported_rate = [
                (24000, 1001),
                (24, 1),
                (25, 1),
                (30000, 1001),
                (30, 1),
                (50, 1),
                (60000, 1001),
                (60, 1),
            ]
            .into_iter()
            .any(|(num, den)| {
                u64::from(profile.frame_rate.numerator) * den
                    == u64::from(profile.frame_rate.denominator) * num
            });
            if profile.width < 256
                || profile.height < 120
                || !profile.width.is_multiple_of(2)
                || !profile.height.is_multiple_of(2)
                || !supported_rate
                || profile.sample_rate != 48000
                || profile.channels.len() > 2
            {
                return Err(Error::Invalid("DNxHR HQX needs even progressive geometry >=256x120, a declared 23.976..60 master rate, and mono/stereo 48k PCM24".into()));
            }
            if self == Self::DnxhrHqxMxf
                && !(48000u64 * u64::from(profile.frame_rate.denominator))
                    .is_multiple_of(u64::from(profile.frame_rate.numerator))
            {
                return Err(Error::Invalid("MXF fractional sample-per-edit clocks are unavailable: preserving the selected audio phase requires a different rechunk policy; use DNxHR MOV".into()));
            }
        }
        if self == Self::H264Mp4
            && (!profile.width.is_multiple_of(2)
                || !profile.height.is_multiple_of(2)
                || profile.channels.len() > 2
                || ![44100, 48000].contains(&profile.sample_rate))
        {
            return Err(Error::Invalid(
                "H.264/AAC needs even dimensions and mono/stereo sound at 44.1 or 48 kHz".into(),
            ));
        }
        Ok(())
    }
}

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
