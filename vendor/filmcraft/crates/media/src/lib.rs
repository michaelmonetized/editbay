//! FilmCraft metadata types extracted for editorial interchange; no media runtime.
//! Modified from the pinned MIT source; decoding remains EditBay native code.
use filmcraft_color::ColorInfo;
use filmcraft_time::{FrameRate, Tick};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    /// Video, possibly with audio.
    Movie,
    AudioOnly,
    Still,
    ImageSequence,
    Synthetic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoStreamInfo {
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub par: (u32, u32),
    pub codec: String,
    pub pixel_format: String,
    pub color: ColorInfo,
    pub has_alpha: bool,
    /// Bitrate in bits/s when known.
    pub bitrate: Option<u64>,
    /// HDR static metadata (mastering display, content light level) when the file carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr: Option<filmcraft_color::HdrMetadata>,
}

/// Most audio streams a source reports (the count comes from the file: never trusted).
pub const MAX_AUDIO_STREAMS: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    pub sample_rate: u32,
    pub channels: u32,
    pub codec: String,
    pub bits_per_sample: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub name: String,
    pub kind: MediaKind,
    /// Media duration. Stills report a default duration (the still-image default preference).
    pub duration: Tick,
    pub video: Option<VideoStreamInfo>,
    /// Every audio stream the container carries, in file order. Stream 0 is the one
    /// [`MediaSource::audio`] reads; the others are read with [`MediaSource::audio_stream`].
    /// Project files written before multi-stream support hold a single `audio` object (or
    /// `null`): both still load.
    #[serde(default, alias = "audio", deserialize_with = "deserialize_audio_streams")]
    pub audio_streams: Vec<AudioStreamInfo>,
    pub container: String,
    /// Timecode of the first frame (in frames at `video.frame_rate`), if the file carries one.
    pub start_timecode: Option<i64>,
    pub file_size: Option<u64>,
}

/// Reads `audio_streams` from a list, or from the single optional stream older project files wrote.
fn deserialize_audio_streams<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<AudioStreamInfo>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Streams {
        Many(Vec<AudioStreamInfo>),
        One(Option<AudioStreamInfo>),
    }
    Ok(match Streams::deserialize(d)? {
        Streams::Many(v) => v,
        Streams::One(o) => o.into_iter().collect(),
    })
}

impl MediaInfo {
    pub fn frame_rate(&self) -> FrameRate {
        self.video.as_ref().map(|v| v.frame_rate).unwrap_or_default()
    }
    pub fn has_video(&self) -> bool {
        self.video.is_some()
    }
    pub fn has_audio(&self) -> bool {
        !self.audio_streams.is_empty()
    }
    /// The primary audio stream (stream 0), the one [`MediaSource::audio`] reads.
    pub fn audio(&self) -> Option<&AudioStreamInfo> {
        self.audio_streams.first()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DemoScene {
    Aurora,
    OceanSunset,
    CityNight,
    Dunes,
    Plasma,
    Forest,
}

impl DemoScene {
    pub const ALL: [DemoScene; 6] = [DemoScene::OceanSunset, DemoScene::Aurora, DemoScene::CityNight, DemoScene::Dunes, DemoScene::Forest, DemoScene::Plasma];
    pub fn file_name(self) -> &'static str {
        match self {
            DemoScene::Aurora => "Aurora_Timelapse.mp4",
            DemoScene::OceanSunset => "Ocean_Sunset.mp4",
            DemoScene::CityNight => "City_Night_Drive.mp4",
            DemoScene::Dunes => "Desert_Dunes.mp4",
            DemoScene::Plasma => "Neon_Loop.mov",
            DemoScene::Forest => "Misty_Forest.mp4",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Generator {
    BarsAndTone,
    ColorMatte {
        color: [f32; 4],
    },
    BlackVideo,
    TransparentVideo,
    CountingLeader,
    Demo(DemoScene),
    /// Pure audio tone (Hz, dBFS).
    Tone {
        hz: f32,
        db: f32,
    },
}

impl Generator {
    pub fn label(&self) -> String {
        match self {
            Generator::BarsAndTone => "Bars and Tone".into(),
            Generator::ColorMatte { .. } => "Color Matte".into(),
            Generator::BlackVideo => "Black Video".into(),
            Generator::TransparentVideo => "Transparent Video".into(),
            Generator::CountingLeader => "Universal Counting Leader".into(),
            Generator::Demo(d) => d.file_name().into(),
            Generator::Tone { hz, .. } => format!("Tone {hz} Hz"),
        }
    }
}
