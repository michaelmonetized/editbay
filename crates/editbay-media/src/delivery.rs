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
        if format == DeliveryFormat::HevcMainMp4 {
            return if self.primaries == 1
                && [1, 13].contains(&self.transfer)
                && self.hdr_metadata.is_empty()
            {
                Ok(())
            } else {
                Err(Error::Invalid("HEVC Main8 is an explicit 8-bit SDR profile; use a supported high-depth profile for HDR".into()))
            };
        }
        if format.yuv10() {
            if [1, 9].contains(&self.primaries)
                && [1, 13, 16, 18].contains(&self.transfer)
                && (![16, 18].contains(&self.transfer) || self.primaries == 9)
                && self.hdr_metadata.is_empty()
            {
                return Ok(());
            }
            return Err(Error::Invalid("10-bit 4:2:0 delivery supports BT.709/sRGB or BT.2020 PQ/HLG; static HDR metadata is unavailable in this preset".into()));
        }
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
    HevcMain10Mp4,
    Vp9Profile2Webm,
    HevcMainMp4,
}

/// Packaged encoder/muxer negotiation, separate from an executed qualification receipt.
#[derive(Clone, Debug, Serialize)]
pub struct DeliveryAvailability {
    pub format: DeliveryFormat,
    pub video_codec: &'static str,
    pub profile: &'static str,
    pub pixel_format: &'static str,
    pub audio_codec: &'static str,
    pub container: &'static str,
    pub status: &'static str,
    pub reason: &'static str,
    pub qualification: &'static str,
}

impl DeliveryFormat {
    pub const ALL: [Self; 9] = [
        Self::LosslessMov,
        Self::Lossless16Mov,
        Self::H264Mp4,
        Self::ProresMov,
        Self::DnxhrHqxMov,
        Self::DnxhrHqxMxf,
        Self::HevcMain10Mp4,
        Self::Vp9Profile2Webm,
        Self::HevcMainMp4,
    ];

    /// Query installed native codec, pixel and container registration without encoding.
    /// Takes this format; returns an explicit availability reason and no qualification claim.
    pub fn availability(self) -> DeliveryAvailability {
        let (video_codec, profile, pixel_format, audio_codec) = match self {
            Self::LosslessMov => ("png", "RGBA8", "rgba", "pcm_f32le"),
            Self::Lossless16Mov => ("png", "RGBA16", "rgba64be", "pcm_f32le"),
            Self::H264Mp4 => ("libx264", "High", "yuv420p", "aac"),
            Self::ProresMov => ("prores_ks", "4444", "yuva444p10le", "pcm_f32le"),
            Self::DnxhrHqxMov | Self::DnxhrHqxMxf => {
                ("dnxhd", "DNXHR HQX", "yuv422p10le", "pcm_s24le")
            }
            Self::HevcMain10Mp4 => ("libx265", "Main 10", "yuv420p10le", "aac"),
            Self::Vp9Profile2Webm => ("libvpx-vp9", "Profile 2", "yuv420p10le", "libopus"),
            Self::HevcMainMp4 => ("libx265", "Main", "yuv420p", "aac"),
        };
        let code = crate::ffi::delivery_available(self);
        let (status, reason) = match code {
            0 => (
                "available",
                "Registered codec, pixel format, decoder and container combination; actual export/QC is still required",
            ),
            1 => ("unavailable", "Required picture encoder is absent"),
            2 => ("unavailable", "Required sound encoder is absent"),
            3 => ("unavailable", "Required container muxer is absent"),
            4 => ("unavailable", "Required QC decoder is absent"),
            5 => (
                "unavailable",
                "Encoder does not advertise the required pixel format",
            ),
            6 => ("unavailable", "Container refuses the required codec"),
            _ => (
                "undetermined",
                "Native registration cannot determine the complete combination; actual export may fail",
            ),
        };
        DeliveryAvailability {
            format: self,
            video_codec,
            profile,
            pixel_format,
            audio_codec,
            container: self.extension(),
            status,
            reason,
            qualification: "not_run_by_capability_query",
        }
    }
    /// Return native picture storage at the encoder boundary.
    /// Takes this format and returns its declared interleaved component encoding.
    pub fn pixel_encoding(self) -> crate::PixelEncoding {
        if self == Self::Lossless16Mov || self.dnxhr() || (self.hevc() || self.webm()) {
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

    /// Identify profiles using the shared bounded AAC encoding and QC route.
    /// Takes this format; returns true for full-scale mono/stereo AAC delivery.
    pub fn aac(self) -> bool {
        matches!(
            self,
            Self::H264Mp4 | Self::HevcMain10Mp4 | Self::HevcMainMp4
        )
    }

    /// Identify the explicit 8-bit Main and 10-bit Main10 HEVC presets.
    /// Takes this format; returns whether the existing x265 route is requested.
    pub fn hevc(self) -> bool {
        matches!(self, Self::HevcMainMp4 | Self::HevcMain10Mp4)
    }

    /// Reject a known unavailable packaged codec combination before starting output work.
    /// Takes this format; returns the native reason without treating registration as qualification.
    pub fn validate_availability(self) -> Result<()> {
        let available = self.availability();
        if available.status == "unavailable" {
            return Err(Error::Invalid(format!(
                "{:?} unavailable: {}",
                self, available.reason
            )));
        }
        Ok(())
    }

    /// Identify high-depth 4:2:0 delivery through the shared native conversion.
    /// Takes this format; returns whether the encoder uses YUV420P10 input.
    pub fn yuv10(self) -> bool {
        matches!(self, Self::HevcMain10Mp4 | Self::Vp9Profile2Webm)
    }

    /// Identify the explicitly bounded WebM clock and Opus preset.
    /// Takes this format; returns whether WebM timing constraints apply.
    pub fn webm(self) -> bool {
        self == Self::Vp9Profile2Webm
    }

    /// Identify sound requiring signal-error QC rather than exact float bytes.
    /// Takes this format; returns true for AAC or Opus delivery.
    pub fn lossy_sound(self) -> bool {
        self.aac() || self.webm()
    }

    pub(crate) fn native_id(self) -> i32 {
        match self {
            Self::LosslessMov => 0,
            Self::H264Mp4 => 1,
            Self::ProresMov => 2,
            Self::Lossless16Mov => 3,
            Self::DnxhrHqxMov => 4,
            Self::DnxhrHqxMxf => 5,
            Self::HevcMain10Mp4 => 6,
            Self::Vp9Profile2Webm => 7,
            Self::HevcMainMp4 => 8,
        }
    }

    /// Reject transparency for a codec without an alpha channel.
    /// Takes this format; returns true when an opaque graph or explicit background is required.
    pub fn requires_opaque(self) -> bool {
        self.dnxhr() || self.hevc() || self.webm()
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
            Self::H264Mp4 | Self::HevcMain10Mp4 | Self::HevcMainMp4 => "mp4",
            Self::DnxhrHqxMxf => "mxf",
            Self::Vp9Profile2Webm => "webm",
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
        if self.lossy_sound()
            && (!profile.width.is_multiple_of(2)
                || !profile.height.is_multiple_of(2)
                || profile.channels.len() > 2
                || ![44100, 48000].contains(&profile.sample_rate))
        {
            return Err(Error::Invalid(
                "Compressed sound video delivery needs even dimensions and mono/stereo sound at 44.1 or 48 kHz"
                    .into(),
            ));
        }
        if (self.hevc() || self.webm()) && (profile.width < 16 || profile.height < 16) {
            return Err(Error::Invalid(
                "HEVC/VP9 delivery needs geometry of at least 16x16".into(),
            ));
        }
        if self.webm()
            && (profile.sample_rate != 48000
                || !(1000u64 * u64::from(profile.frame_rate.denominator))
                    .is_multiple_of(u64::from(profile.frame_rate.numerator)))
        {
            return Err(Error::Invalid("WebM VP9/Opus needs 48 kHz sound and an integral-millisecond picture clock; fractional millisecond clocks are unavailable in this exact-timing preset".into()));
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
