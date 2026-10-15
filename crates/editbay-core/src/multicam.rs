//! Retained multicamera sync, exact angle edits and native composition compilation.

use crate::{
    Clip, ClipSource, Composition, Error, FrameRange, FrameRate, Project, Result, SourcePosition,
    TimeMap, TimePoint, Track, TrackKind,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{cmp::Ordering, collections::HashSet};
use uuid::Uuid;

const MAX_ANGLES: usize = 64;
const MAX_CUTS: usize = 4096;
const MAX_AUDIO_POINTS: usize = 131_072;
const MAX_AUDIO_COMPARISONS: usize = 32_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MulticamAngle {
    pub id: Uuid,
    pub composition: Uuid,
    pub name: String,
    pub offset: SourcePosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MulticamCut {
    pub frame: u64,
    pub angle: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MulticamAudioPolicy {
    Fixed {
        angle: Uuid,
    },
    #[default]
    FollowVideo,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MulticamSession {
    pub id: Uuid,
    pub composition: Uuid,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub angles: Vec<MulticamAngle>,
    pub cuts: Vec<MulticamCut>,
    #[serde(default)]
    pub audio_policy: MulticamAudioPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MulticamOverlap {
    pub origin: i64,
    pub duration: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MulticamTimecode {
    pub frames: SourcePosition,
    pub rate: FrameRate,
}

impl MulticamTimecode {
    /// Parse an exact SMPTE camera start timecode.
    /// `input` uses HH:MM:SS:FF or supported NTSC HH:MM:SS;FF; `rate` is its rate.
    /// Returns a counted frame clock, rejecting nonexistent drop-frame labels.
    pub fn parse(input: &str, rate: FrameRate) -> Result<Self> {
        rate.validate()?;
        let fields: Vec<_> = input.split([':', ';']).collect();
        if fields.len() != 4
            || fields
                .iter()
                .any(|field| field.len() != 2 || !field.bytes().all(|byte| byte.is_ascii_digit()))
            || input.bytes().filter(|byte| *byte == b':').count()
                + input.bytes().filter(|byte| *byte == b';').count()
                != 3
        {
            return invalid("multicam timecode needs HH:MM:SS:FF or HH:MM:SS;FF");
        }
        let numbers: Vec<u64> = fields
            .iter()
            .map(|field| field.parse().map_err(|_| error("invalid timecode field")))
            .collect::<Result<_>>()?;
        let [hours, minutes, seconds, frame] = numbers.as_slice() else {
            return invalid("invalid timecode fields");
        };
        let nominal = (u64::from(rate.numerator) + u64::from(rate.denominator) / 2)
            / u64::from(rate.denominator);
        if *hours >= 24 || *minutes >= 60 || *seconds >= 60 || nominal == 0 || *frame >= nominal {
            return invalid("multicam timecode field exceeds its clock or frame-rate range");
        }
        let drop = input.contains(';');
        if input.as_bytes()[2] != b':'
            || input.as_bytes()[5] != b':'
            || input.as_bytes()[8] != if drop { b';' } else { b':' }
        {
            return invalid("multicam drop-frame separator belongs before the frame field");
        }
        let skipped = if drop {
            let drop_count = if equivalent_rate(rate, FrameRate::new(30_000, 1001)?) {
                2
            } else if equivalent_rate(rate, FrameRate::new(60_000, 1001)?) {
                4
            } else {
                return invalid("drop-frame timecode requires 30000/1001 or 60000/1001 fps");
            };
            if *seconds == 0 && minutes % 10 != 0 && *frame < drop_count {
                return invalid("multicam timecode names a skipped drop-frame label");
            }
            let total_minutes = hours * 60 + minutes;
            drop_count * (total_minutes - total_minutes / 10)
        } else {
            0
        };
        let counted = ((hours * 3600 + minutes * 60 + seconds) * nominal + frame) - skipped;
        Ok(Self {
            frames: SourcePosition::new(counted as i64, 1)?,
            rate,
        })
    }
}

impl MulticamSession {
    /// Create a multicam session while preserving every source composition.
    /// `project`, `name` and ordered `sources` select cameras; returns a fresh output identity.
    pub fn new(project: &Project, name: impl Into<String>, sources: &[Uuid]) -> Result<Self> {
        let first = source(
            project,
            sources
                .first()
                .copied()
                .ok_or_else(|| error("multicam needs at least two cameras"))?,
        )?;
        let session = Self {
            id: Uuid::new_v4(),
            composition: Uuid::new_v4(),
            name: name.into(),
            width: first.width,
            height: first.height,
            frame_rate: first.frame_rate,
            angles: sources
                .iter()
                .map(|id| {
                    let scene = source(project, *id)?;
                    Ok(MulticamAngle {
                        id: Uuid::new_v4(),
                        composition: *id,
                        name: scene.name.clone(),
                        offset: SourcePosition::new(0, 1)?,
                    })
                })
                .collect::<Result<_>>()?,
            cuts: Vec::new(),
            audio_policy: if sources
                .iter()
                .all(|id| source(project, *id).is_ok_and(|camera| camera.audio.is_some()))
            {
                MulticamAudioPolicy::FollowVideo
            } else {
                MulticamAudioPolicy::All
            },
        };
        let mut session = session;
        session.cuts.push(MulticamCut {
            frame: 0,
            angle: session.angles[0].id,
        });
        session.validate(project)?;
        Ok(session)
    }

    /// Check camera ownership, sync availability, cuts and requested audio.
    /// `project` owns the independent camera compositions; returns a complete validation result.
    pub fn validate(&self, project: &Project) -> Result<()> {
        self.frame_rate.validate()?;
        if self.id.is_nil()
            || self.composition.is_nil()
            || self.id == self.composition
            || self.name.trim().is_empty()
            || self.name.len() > 1024
            || self.name.contains(['\n', '\r', '\0'])
            || self.width == 0
            || self.height == 0
            || self.width > 32_768
            || self.height > 32_768
            || !(2..=MAX_ANGLES).contains(&self.angles.len())
            || self.cuts.is_empty()
            || self.cuts.len() > MAX_CUTS
            || self.cuts[0].frame != 0
        {
            return invalid(
                "multicam needs valid identities, dimensions, two to 64 angles and a first cut at zero",
            );
        }
        let mut ids = HashSet::from([self.id, self.composition]);
        let mut sources = HashSet::new();
        for angle in &self.angles {
            validate_position(angle.offset)?;
            let camera = source(project, angle.composition)?;
            camera.frame_rate.validate()?;
            if angle.id.is_nil()
                || !ids.insert(angle.id)
                || !sources.insert(angle.composition)
                || angle.name.trim().is_empty()
                || angle.name.len() > 1024
                || angle.name.contains(['\n', '\r', '\0'])
                || camera.picture.is_none()
                || camera.duration == 0
                || camera.duration > i64::MAX as u64
                || camera.width != self.width
                || camera.height != self.height
                || depends_on(project, camera.id, self.composition)?
            {
                return invalid(
                    "multicam angle needs unique ownership and independent, matching-size picture output; conform camera dimensions first",
                );
            }
        }
        let overlap = self.overlap(project)?;
        for (index, cut) in self.cuts.iter().enumerate() {
            self.angle(cut.angle)?;
            if cut.frame >= overlap.duration || index > 0 && self.cuts[index - 1].frame >= cut.frame
            {
                return invalid("multicam cuts must be increasing and inside synchronized overlap");
            }
        }
        let audio_count = match self.audio_policy {
            MulticamAudioPolicy::Fixed { angle } => {
                if source(project, self.angle(angle)?.composition)?
                    .audio
                    .is_none()
                {
                    return invalid("fixed multicam audio angle has no sound output");
                }
                1
            }
            MulticamAudioPolicy::FollowVideo => {
                if self.angles.iter().any(|angle| {
                    source(project, angle.composition).is_ok_and(|camera| camera.audio.is_none())
                }) {
                    return invalid(
                        "follow-video multicam audio needs sound on every angle; choose a fixed sound camera",
                    );
                }
                1
            }
            MulticamAudioPolicy::All => self
                .angles
                .iter()
                .filter(|angle| {
                    source(project, angle.composition).is_ok_and(|camera| camera.audio.is_some())
                })
                .count(),
        };
        if self
            .cuts
            .len()
            .checked_mul(2 + audio_count)
            .is_none_or(|nodes| nodes > 32_767)
        {
            return invalid("multicam cut and audio count exceed the native graph budget");
        }
        Ok(())
    }

    /// Find full output frames available on every synchronized camera.
    /// `project` supplies source rates and durations; returns the shared origin and duration.
    pub fn overlap(&self, project: &Project) -> Result<MulticamOverlap> {
        self.frame_rate.validate()?;
        let mut start = None;
        let mut end = None;
        for angle in &self.angles {
            validate_position(angle.offset)?;
            let camera = source(project, angle.composition)?;
            camera.frame_rate.validate()?;
            let duration = convert_frames(
                SourcePosition::new(
                    i64::try_from(camera.duration).map_err(|_| {
                        error("multicam camera duration exceeds exact integer time")
                    })?,
                    1,
                )?,
                camera.frame_rate,
                self.frame_rate,
            )?;
            let angle_end = add(angle.offset, duration)?;
            start = Some(start.map_or(angle.offset, |current| {
                if order(current, angle.offset).is_lt() {
                    angle.offset
                } else {
                    current
                }
            }));
            end = Some(end.map_or(angle_end, |current| {
                if order(current, angle_end).is_gt() {
                    angle_end
                } else {
                    current
                }
            }));
        }
        let origin = ceil(start.ok_or_else(|| error("multicam needs cameras"))?)?;
        let end = floor(end.ok_or_else(|| error("multicam needs cameras"))?)?;
        let duration = end
            .checked_sub(origin)
            .filter(|duration| *duration > 0)
            .ok_or_else(|| error("multicam cameras have no full common output frame"))?
            as u64;
        Ok(MulticamOverlap { origin, duration })
    }

    /// Synchronize a shared event marked independently in each camera.
    /// `markers` maps every angle ID to its exact local source-frame marker.
    /// Returns an atomic offset update anchored to the first camera at zero.
    pub fn sync_markers(
        &mut self,
        project: &Project,
        markers: &[(Uuid, SourcePosition)],
    ) -> Result<()> {
        validate_sync_ids(self, markers.iter().map(|(id, _)| *id))?;
        let reference = self
            .angles
            .first()
            .ok_or_else(|| error("multicam needs cameras"))?;
        let reference_marker = point_for(markers, reference.id)?;
        let reference_clock = convert_frames(
            reference_marker,
            source(project, reference.composition)?.frame_rate,
            self.frame_rate,
        )?;
        let mut next = self.clone();
        for angle in &mut next.angles {
            let camera = source(project, angle.composition)?;
            let marker = point_for(markers, angle.id)?;
            validate_position(marker)?;
            if marker.numerator < 0
                || marker.compare_tick(
                    i64::try_from(camera.duration)
                        .map_err(|_| error("multicam camera duration exceeds integer time"))?,
                )? != Ordering::Less
            {
                return invalid("multicam marker must be inside its camera composition");
            }
            angle.offset = subtract(
                reference_clock,
                convert_frames(marker, camera.frame_rate, self.frame_rate)?,
            )?;
        }
        next.validate(project)?;
        *self = next;
        Ok(())
    }

    /// Synchronize camera start timecodes without float conversion or clock wrap guesses.
    /// `timecodes` supplies every angle's counted source clock and exact rate.
    /// Returns an atomic offset update relative to the first camera's start.
    pub fn sync_timecodes(
        &mut self,
        project: &Project,
        timecodes: &[(Uuid, MulticamTimecode)],
    ) -> Result<()> {
        validate_sync_ids(self, timecodes.iter().map(|(id, _)| *id))?;
        let reference = self
            .angles
            .first()
            .ok_or_else(|| error("multicam needs cameras"))?;
        let reference_clock = timecodes
            .iter()
            .find(|(id, _)| *id == reference.id)
            .map(|(_, clock)| *clock)
            .ok_or_else(|| error("multicam reference timecode is absent"))?;
        validate_position(reference_clock.frames)?;
        let reference_start = convert_frames(
            reference_clock.frames,
            reference_clock.rate,
            self.frame_rate,
        )?;
        let mut next = self.clone();
        for angle in &mut next.angles {
            let clock = timecodes
                .iter()
                .find(|(id, _)| *id == angle.id)
                .map(|(_, clock)| *clock)
                .ok_or_else(|| error("multicam timecode is absent"))?;
            validate_position(clock.frames)?;
            if clock.frames.numerator < 0 {
                return invalid("multicam timecode must be nonnegative");
            }
            angle.offset = subtract(
                convert_frames(clock.frames, clock.rate, self.frame_rate)?,
                reference_start,
            )?;
        }
        next.validate(project)?;
        *self = next;
        Ok(())
    }

    /// Apply a measured audio lag as an exact rational frame offset.
    /// `reference`, `angle` and accepted `sync` select the two cameras and sample clock.
    /// Returns an atomic update; a positive candidate lag moves that camera earlier.
    pub fn sync_audio_offset(
        &mut self,
        project: &Project,
        reference: Uuid,
        angle: Uuid,
        sync: AudioSyncResult,
    ) -> Result<()> {
        if reference == angle
            || sync.sample_rate == 0
            || !sync.correlation.is_finite()
            || !sync.separation.is_finite()
        {
            return invalid("multicam audio sync needs distinct cameras and a valid measured lag");
        }
        let reference_offset = self.angle(reference)?.offset;
        self.angle(angle)?;
        let frames = multiply(
            SourcePosition::new(sync.lag_samples, u64::from(sync.sample_rate))?,
            u64::from(self.frame_rate.numerator),
            u64::from(self.frame_rate.denominator),
        )?;
        let mut next = self.clone();
        next.angles
            .iter_mut()
            .find(|candidate| candidate.id == angle)
            .ok_or_else(|| error("multicam audio angle is absent"))?
            .offset = subtract(reference_offset, frames)?;
        next.validate(project)?;
        *self = next;
        Ok(())
    }

    /// Insert a live camera cut or replace the camera at an existing cut.
    /// `frame` is output-local and `angle` is a stable camera ID; returns one validated edit.
    pub fn cut_at(&mut self, project: &Project, frame: u64, angle: Uuid) -> Result<()> {
        self.angle(angle)?;
        let mut next = self.clone();
        match next.cuts.binary_search_by_key(&frame, |cut| cut.frame) {
            Ok(index) => next.cuts[index].angle = angle,
            Err(index) => next.cuts.insert(index, MulticamCut { frame, angle }),
        }
        next.validate(project)?;
        *self = next;
        Ok(())
    }

    /// Change the owning camera segment without introducing a new cut boundary.
    /// `frame` selects an existing segment and `angle` selects its camera.
    /// Returns one validated edit while retaining every prior segment boundary.
    pub fn switch_at(&mut self, project: &Project, frame: u64, angle: Uuid) -> Result<()> {
        self.angle(angle)?;
        if frame >= self.overlap(project)?.duration {
            return invalid("multicam switch is outside synchronized output");
        }
        let index = self
            .cuts
            .partition_point(|cut| cut.frame <= frame)
            .checked_sub(1)
            .ok_or_else(|| error("multicam needs a first cut at zero"))?;
        let mut next = self.clone();
        next.cuts[index].angle = angle;
        next.validate(project)?;
        *self = next;
        Ok(())
    }

    /// Resolve one retained camera by identity.
    /// `id` selects an angle; returns its source reference or a missing-angle error.
    pub fn angle(&self, id: Uuid) -> Result<&MulticamAngle> {
        self.angles
            .iter()
            .find(|angle| angle.id == id)
            .ok_or_else(|| error("multicam angle is absent"))
    }
}

/// Compile a retained multicam edit into existing native picture and sound graphs.
/// `project` owns original cameras and `session` owns sync/cuts/audio policy.
/// Returns an output composition with stable clip identities and exact source maps.
pub fn compile(project: &Project, session: &MulticamSession) -> Result<Composition> {
    session.validate(project)?;
    let overlap = session.overlap(project)?;
    let mut scene = Composition {
        id: session.composition,
        name: session.name.clone(),
        width: session.width,
        height: session.height,
        frame_rate: session.frame_rate,
        duration: overlap.duration,
        tracks: Vec::new(),
        nodes: Vec::new(),
        picture: None,
        audio: None,
    };
    let mut video = Track {
        id: stable_id(session.id, "video-track", 0, None),
        name: "Multicam picture".into(),
        kind: TrackKind::Video,
        enabled: true,
        clips: Vec::new(),
    };
    let mut audio: Vec<Track> = match session.audio_policy {
        MulticamAudioPolicy::All => session
            .angles
            .iter()
            .filter(|angle| {
                source(project, angle.composition).is_ok_and(|camera| camera.audio.is_some())
            })
            .map(|angle| Track {
                id: stable_id(session.id, "all-audio-track", 0, Some(angle.id)),
                name: format!("{} sound", angle.name),
                kind: TrackKind::Audio,
                enabled: true,
                clips: Vec::new(),
            })
            .collect(),
        _ => vec![Track {
            id: stable_id(session.id, "audio-track", 0, None),
            name: "Multicam linked sound".into(),
            kind: TrackKind::Audio,
            enabled: true,
            clips: Vec::new(),
        }],
    };
    for (index, cut) in session.cuts.iter().enumerate() {
        let range = FrameRange {
            start: cut.frame,
            end: session
                .cuts
                .get(index + 1)
                .map_or(overlap.duration, |next| next.frame),
        };
        let video_angle = session.angle(cut.angle)?;
        let mut picture = camera_clip(
            project,
            session,
            overlap,
            video_angle,
            range,
            "video-clip",
            None,
        )?;
        match session.audio_policy {
            MulticamAudioPolicy::FollowVideo | MulticamAudioPolicy::Fixed { .. } => {
                let sound_angle = match session.audio_policy {
                    MulticamAudioPolicy::Fixed { angle } => session.angle(angle)?,
                    _ => video_angle,
                };
                let mut sound = camera_clip(
                    project,
                    session,
                    overlap,
                    sound_angle,
                    range,
                    "audio-clip",
                    None,
                )?;
                picture.linked = Some(sound.id);
                sound.linked = Some(picture.id);
                audio[0].clips.push(sound);
            }
            MulticamAudioPolicy::All => {
                for track in &mut audio {
                    let sound_angle = session
                        .angles
                        .iter()
                        .find(|angle| {
                            stable_id(session.id, "all-audio-track", 0, Some(angle.id)) == track.id
                        })
                        .ok_or_else(|| error("multicam sound track lost its camera"))?;
                    let mut sound = camera_clip(
                        project,
                        session,
                        overlap,
                        sound_angle,
                        range,
                        "all-audio-clip",
                        Some(sound_angle.id),
                    )?;
                    if sound_angle.id == video_angle.id {
                        picture.linked = Some(sound.id);
                        sound.linked = Some(picture.id);
                    }
                    track.clips.push(sound);
                }
            }
        }
        video.clips.push(picture);
    }
    scene.tracks.push(video);
    scene.tracks.extend(audio);
    crate::editorial::rebuild_editorial(&mut scene, &[])?;
    let nodes: std::collections::HashMap<_, _> = scene
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            (
                node.id,
                stable_id(session.id, "compiled-node", index as u64, None),
            )
        })
        .collect();
    for node in &mut scene.nodes {
        node.id = nodes[&node.id];
        match &mut node.operation {
            crate::NodeOperation::Source { .. } => {}
            crate::NodeOperation::Over {
                foreground,
                background,
                mask,
            } => {
                *foreground = nodes[foreground];
                *background = nodes[background];
                *mask = mask.map(|id| nodes[&id]);
            }
            crate::NodeOperation::Mix { inputs } => {
                for id in inputs {
                    *id = nodes[id];
                }
            }
            _ => return invalid("multicam compiler produced an unexpected graph operation"),
        }
    }
    scene.picture = scene.picture.map(|id| nodes[&id]);
    scene.audio = scene.audio.map(|id| nodes[&id]);
    Ok(scene)
}

/// Retained bounds and confidence policy for normalized mono audio correlation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AudioSyncOptions {
    pub sample_rate: u32,
    pub max_offset_samples: u32,
    pub min_overlap_samples: u32,
    pub min_correlation: f64,
    pub min_separation: f64,
}

impl Default for AudioSyncOptions {
    fn default() -> Self {
        Self {
            sample_rate: 200,
            max_offset_samples: 2000,
            min_overlap_samples: 64,
            min_correlation: 0.85,
            min_separation: 0.05,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AudioSyncResult {
    lag_samples: i64,
    sample_rate: u32,
    correlation: f64,
    separation: f64,
    overlap_samples: u32,
}

impl AudioSyncResult {
    /// Inspect the exact measured candidate lag.
    /// Takes this receipt and returns samples; positive means later within the candidate file.
    pub fn lag_samples(self) -> i64 {
        self.lag_samples
    }
    /// Inspect the clock of the measured arrays.
    /// Takes this receipt and returns envelope samples per second.
    pub fn sample_rate(self) -> u32 {
        self.sample_rate
    }
    /// Inspect normalized peak correlation.
    /// Takes this receipt and returns a value between negative one and one.
    pub fn correlation(self) -> f64 {
        self.correlation
    }
    /// Inspect separation from every competing sample lag.
    /// Takes this receipt and returns the best correlation minus the next best.
    pub fn separation(self) -> f64 {
        self.separation
    }
    /// Inspect the shared samples supporting this match.
    /// Takes this receipt and returns the matched overlap length.
    pub fn overlap_samples(self) -> u32 {
        self.overlap_samples
    }
}

/// Find a confident exact sample lag in bounded mono audio or envelope arrays.
/// `reference`, `candidate` and `options` declare their common clock and search budget.
/// Returns a measured lag, rejecting silence, ambiguity, nonfinite input and excessive work.
pub fn correlate_audio(
    reference: &[f32],
    candidate: &[f32],
    options: AudioSyncOptions,
) -> Result<AudioSyncResult> {
    let minimum = options.min_overlap_samples as usize;
    let lag_limit = options.max_offset_samples as usize;
    if options.sample_rate == 0
        || options.sample_rate > 768_000
        || minimum < 16
        || reference.len() < minimum
        || candidate.len() < minimum
        || reference.len() > MAX_AUDIO_POINTS
        || candidate.len() > MAX_AUDIO_POINTS
        || lag_limit >= reference.len().max(candidate.len())
        || !(0.0..=1.0).contains(&options.min_correlation)
        || !(0.0..=1.0).contains(&options.min_separation)
        || !options.min_correlation.is_finite()
        || !options.min_separation.is_finite()
        || reference
            .iter()
            .chain(candidate)
            .any(|value| !value.is_finite() || value.abs() > 1.0e6)
        || lag_limit
            .checked_mul(2)
            .and_then(|lags| lags.checked_add(1))
            .and_then(|lags| lags.checked_mul(reference.len().min(candidate.len())))
            .is_none_or(|work| work > MAX_AUDIO_COMPARISONS)
    {
        return invalid(
            "multicam audio sync exceeds its sample/work budget or has invalid clock, confidence or input",
        );
    }
    let minimum = minimum.max(reference.len().min(candidate.len()) / 2);
    let mut best = None;
    let mut second = -1.0;
    for lag in -(options.max_offset_samples as i64)..=i64::from(options.max_offset_samples) {
        let reference_start = if lag < 0 { (-lag) as usize } else { 0 };
        let candidate_start = if lag > 0 { lag as usize } else { 0 };
        let overlap = reference
            .len()
            .saturating_sub(reference_start)
            .min(candidate.len().saturating_sub(candidate_start));
        if overlap < minimum {
            continue;
        }
        let Some(score) = correlation(
            &reference[reference_start..reference_start + overlap],
            &candidate[candidate_start..candidate_start + overlap],
        ) else {
            continue;
        };
        if best.is_none_or(|(_, peak, _)| score > peak) {
            if let Some((_, peak, _)) = best {
                second = peak;
            }
            best = Some((lag, score, overlap));
        } else if score > second {
            second = score;
        }
    }
    let (lag_samples, score, overlap) =
        best.ok_or_else(|| error("multicam audio sync found no usable variation and overlap"))?;
    let separation = score - second;
    if score < options.min_correlation || separation < options.min_separation {
        return invalid(
            "multicam audio sync is uncertain or ambiguous; choose a marker or a clearer audio interval",
        );
    }
    Ok(AudioSyncResult {
        lag_samples,
        sample_rate: options.sample_rate,
        correlation: score,
        separation,
        overlap_samples: overlap as u32,
    })
}

/// Compute a stable normalized correlation without gain or DC sensitivity.
/// `a` and `b` have equal overlap; returns no score for silent or constant arrays.
fn correlation(a: &[f32], b: &[f32]) -> Option<f64> {
    let (mut mean_a, mut mean_b, mut variance_a, mut variance_b, mut covariance) =
        (0.0, 0.0, 0.0, 0.0, 0.0);
    for (index, (&a, &b)) in a.iter().zip(b).enumerate() {
        let count = (index + 1) as f64;
        let delta_a = f64::from(a) - mean_a;
        let delta_b = f64::from(b) - mean_b;
        mean_a += delta_a / count;
        mean_b += delta_b / count;
        variance_a += delta_a * (f64::from(a) - mean_a);
        variance_b += delta_b * (f64::from(b) - mean_b);
        covariance += delta_a * (f64::from(b) - mean_b);
    }
    let energy = (variance_a * variance_b).sqrt();
    if !energy.is_finite() || variance_a <= 1.0e-16 || variance_b <= 1.0e-16 {
        return None;
    }
    Some((covariance / energy).clamp(-1.0, 1.0))
}

/// Build one nested camera clip at exact common-clock boundaries.
/// The supplied ownership, camera, range and role produce stable identity and source time.
/// Returns the clip without changing any original camera composition.
fn camera_clip(
    project: &Project,
    session: &MulticamSession,
    overlap: MulticamOverlap,
    angle: &MulticamAngle,
    range: FrameRange,
    role: &str,
    salt: Option<Uuid>,
) -> Result<Clip> {
    let camera = source(project, angle.composition)?;
    let start = overlap
        .origin
        .checked_add(
            i64::try_from(range.start).map_err(|_| error("multicam cut exceeds integer time"))?,
        )
        .ok_or_else(|| error("multicam cut exceeds integer time"))?;
    let end = overlap
        .origin
        .checked_add(
            i64::try_from(range.end).map_err(|_| error("multicam cut exceeds integer time"))?,
        )
        .ok_or_else(|| error("multicam cut exceeds integer time"))?;
    let start = convert_frames(
        subtract(SourcePosition::new(start, 1)?, angle.offset)?,
        session.frame_rate,
        camera.frame_rate,
    )?;
    let end = convert_frames(
        subtract(SourcePosition::new(end, 1)?, angle.offset)?,
        session.frame_rate,
        camera.frame_rate,
    )?;
    let common = lcm(start.denominator, end.denominator)?;
    let source_denominator = u32::try_from(common).map_err(|_| {
        error("multicam source subframe precision exceeds the native time-map scale")
    })?;
    let start_tick = i128::from(start.numerator)
        .checked_mul(i128::from(common / start.denominator))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| error("multicam source map exceeds integer time"))?;
    let end_tick = i128::from(end.numerator)
        .checked_mul(i128::from(common / end.denominator))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| error("multicam source map exceeds integer time"))?;
    Ok(Clip {
        id: stable_id(session.id, role, range.start, salt),
        name: angle.name.clone(),
        range,
        source: ClipSource::Composition {
            composition: angle.composition,
        },
        time_map: TimeMap {
            source_denominator,
            points: vec![
                TimePoint {
                    frame: 0,
                    source_tick: start_tick,
                },
                TimePoint {
                    frame: range.end - range.start,
                    source_tick: end_tick,
                },
            ],
        },
        linked: None,
    })
}

/// Resolve an existing independent composition.
/// `project` and `id` select ownership; returns a source or an explicit missing-reference error.
fn source(project: &Project, id: Uuid) -> Result<&Composition> {
    project
        .compositions
        .iter()
        .find(|scene| scene.id == id)
        .ok_or_else(|| error("multicam source composition is absent"))
}

/// Reject feedback from an output into any camera dependency.
/// `root` and `target` select retained scenes; returns whether the target is reachable.
fn depends_on(project: &Project, root: Uuid, target: Uuid) -> Result<bool> {
    let mut visited = HashSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if id == target {
            return Ok(true);
        }
        if visited.insert(id) {
            if visited.len() > 100_000 {
                return invalid("multicam dependency walk exceeds its budget");
            }
            pending.extend(
                source(project, id)?
                    .tracks
                    .iter()
                    .flat_map(|track| &track.clips)
                    .filter_map(|clip| match clip.source {
                        ClipSource::Composition { composition } => Some(composition),
                        _ => None,
                    }),
            );
        }
    }
    Ok(false)
}

/// Derive a stable generated entity identity.
/// `owner`, `role`, `frame` and optional camera `salt` select one persistent edit entity.
/// Returns a deterministic UUID, independent of compilation count or graph allocation.
fn stable_id(owner: Uuid, role: &str, frame: u64, salt: Option<Uuid>) -> Uuid {
    let mut hash = Sha256::new();
    hash.update(b"editbay-multicam-1");
    hash.update(owner.as_bytes());
    hash.update((role.len() as u64).to_le_bytes());
    hash.update(role.as_bytes());
    hash.update(frame.to_le_bytes());
    if let Some(salt) = salt {
        hash.update(salt.as_bytes());
    }
    let digest = hash.finalize();
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// Check that every camera supplies exactly one synchronization point.
/// `session` owns IDs and `ids` are submitted points; returns an exact-coverage result.
fn validate_sync_ids(session: &MulticamSession, ids: impl Iterator<Item = Uuid>) -> Result<()> {
    let ids: Vec<_> = ids.collect();
    let unique: HashSet<_> = ids.iter().copied().collect();
    if ids.len() != session.angles.len()
        || ids.len() != unique.len()
        || session
            .angles
            .iter()
            .any(|angle| !unique.contains(&angle.id))
    {
        return invalid("multicam sync needs exactly one point per camera");
    }
    Ok(())
}

/// Resolve one exact camera marker.
/// `points` and `id` identify the measurement; returns its rational local frame.
fn point_for(points: &[(Uuid, SourcePosition)], id: Uuid) -> Result<SourcePosition> {
    points
        .iter()
        .find(|(angle, _)| *angle == id)
        .map(|(_, point)| *point)
        .ok_or_else(|| error("multicam marker is absent"))
}

/// Validate an exact signed frame coordinate.
/// `position` is retained rational time; returns an error for zero or noncanonical scale.
fn validate_position(position: SourcePosition) -> Result<()> {
    if SourcePosition::new(position.numerator, position.denominator)? != position {
        return invalid("multicam offset must be a reduced exact frame fraction");
    }
    Ok(())
}

/// Convert exact frame coordinates between source and output clocks.
/// `position`, `from` and `to` select frame rates; returns an unrounded rational frame.
fn convert_frames(
    position: SourcePosition,
    from: FrameRate,
    to: FrameRate,
) -> Result<SourcePosition> {
    from.validate()?;
    to.validate()?;
    multiply(
        position,
        u64::from(from.denominator) * u64::from(to.numerator),
        u64::from(from.numerator) * u64::from(to.denominator),
    )
}

/// Multiply exact signed time by an unsigned ratio.
/// The supplied fraction and scale are reduced before multiplication; returns exact time.
fn multiply(
    position: SourcePosition,
    mut numerator: u64,
    mut denominator: u64,
) -> Result<SourcePosition> {
    validate_position(position)?;
    if denominator == 0 {
        return invalid("multicam time scale is zero");
    }
    let factor = gcd(numerator, denominator);
    numerator /= factor;
    denominator /= factor;
    let factor = gcd(position.numerator.unsigned_abs(), denominator);
    let position_numerator = i128::from(position.numerator) / i128::from(factor);
    denominator /= factor;
    let factor = gcd(position.denominator, numerator);
    let position_denominator = position.denominator / factor;
    numerator /= factor;
    let numerator = position_numerator
        .checked_mul(i128::from(numerator))
        .ok_or_else(|| error("multicam exact time exceeds integer range"))?;
    let denominator = position_denominator
        .checked_mul(denominator)
        .ok_or_else(|| error("multicam exact time exceeds integer range"))?;
    SourcePosition::from_fraction(numerator, denominator)
}

/// Add exact signed frame coordinates.
/// `a` and `b` share frame units; returns a reduced representable sum.
fn add(a: SourcePosition, b: SourcePosition) -> Result<SourcePosition> {
    validate_position(a)?;
    validate_position(b)?;
    let factor = gcd(a.denominator, b.denominator);
    let a_scale = b.denominator / factor;
    let b_scale = a.denominator / factor;
    let numerator = i128::from(a.numerator)
        .checked_mul(i128::from(a_scale))
        .and_then(|left| {
            i128::from(b.numerator)
                .checked_mul(i128::from(b_scale))
                .and_then(|right| left.checked_add(right))
        })
        .ok_or_else(|| error("multicam exact time exceeds integer range"))?;
    let denominator = u128::from(a.denominator) * u128::from(a_scale);
    let factor = gcd128(numerator.unsigned_abs(), denominator);
    SourcePosition::from_fraction(
        numerator
            / i128::try_from(factor)
                .map_err(|_| error("multicam exact time exceeds integer range"))?,
        u64::try_from(denominator / factor)
            .map_err(|_| error("multicam exact time exceeds integer range"))?,
    )
}

/// Subtract exact signed frame coordinates.
/// `a` and `b` share units; returns a reduced difference without float conversion.
fn subtract(a: SourcePosition, b: SourcePosition) -> Result<SourcePosition> {
    let negative = SourcePosition::from_fraction(-i128::from(b.numerator), b.denominator)?;
    add(a, negative)
}

/// Compare validated rational frame coordinates.
/// `a` and `b` share frame units; returns exact ordering.
fn order(a: SourcePosition, b: SourcePosition) -> Ordering {
    (i128::from(a.numerator) * i128::from(b.denominator))
        .cmp(&(i128::from(b.numerator) * i128::from(a.denominator)))
}

/// Find the first complete integer frame at or after a rational boundary.
/// `position` is exact output time; returns its mathematical ceiling.
fn ceil(position: SourcePosition) -> Result<i64> {
    validate_position(position)?;
    let value = -(-i128::from(position.numerator)).div_euclid(i128::from(position.denominator));
    i64::try_from(value).map_err(|_| error("multicam overlap exceeds integer time"))
}

/// Find the last complete integer boundary at or before rational time.
/// `position` is exact output time; returns its mathematical floor.
fn floor(position: SourcePosition) -> Result<i64> {
    validate_position(position)?;
    i64::try_from(i128::from(position.numerator).div_euclid(i128::from(position.denominator)))
        .map_err(|_| error("multicam overlap exceeds integer time"))
}

/// Find a shared source-map scale.
/// `a` and `b` are positive denominators; returns their checked least common multiple.
fn lcm(a: u64, b: u64) -> Result<u64> {
    (a / gcd(a, b))
        .checked_mul(b)
        .ok_or_else(|| error("multicam source-map scale exceeds integer range"))
}

/// Compare frame-rate fractions exactly.
/// `a` and `b` are positive rates; returns whether they describe the same clock.
fn equivalent_rate(a: FrameRate, b: FrameRate) -> bool {
    u64::from(a.numerator) * u64::from(b.denominator)
        == u64::from(b.numerator) * u64::from(a.denominator)
}

/// Find a common integer factor.
/// `a` and `b` are magnitudes; returns their greatest common divisor.
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Reduce a wider exact-time intermediate.
/// `a` and `b` are magnitudes; returns their greatest common divisor.
fn gcd128(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Construct a multicam document error.
/// `message` explains rejection; returns the shared native error.
fn error(message: &str) -> Error {
    Error::Invalid(message.into())
}

/// Return a multicam validation failure.
/// `message` explains rejection; returns an unsuccessful shared result.
fn invalid<T>(message: &str) -> Result<T> {
    Err(error(message))
}
