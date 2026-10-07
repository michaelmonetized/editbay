use crate::{
    Clip, ClipSource, Composition, DocumentCommand, DocumentEditor, DocumentVersion, Error,
    EvaluationSnapshot, FrameRange, NodeOperation, Project, Result, Sequence, SoundBudget,
    SoundSnapshot, TimeMap, TimePoint, TimedNode, Track, TrackKind,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::Arc};
use uuid::Uuid;

/// An exclusive frame range in an existing reusable picture/sound composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSelection {
    pub composition: Uuid,
    pub range: FrameRange,
}

/// Exact edits on paired picture and sound tracks with reusable source sequences.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineAction {
    Title {
        profile: Uuid,
        name: String,
        text: String,
        font: std::path::PathBuf,
        size: f64,
        position: [f64; 2],
        rgba: [f64; 4],
        duration: u64,
    },
    EditTitle {
        composition: Uuid,
        text: String,
        size: f64,
        position: [f64; 2],
        rgba: [f64; 4],
    },
    Fade {
        source: Uuid,
        name: String,
        fade_in: u64,
        fade_out: u64,
    },
    Conform {
        source: Uuid,
        width: u32,
        height: u32,
        frame_rate: crate::FrameRate,
        name: String,
    },
    Create {
        name: String,
        source: SourceSelection,
    },
    Insert {
        composition: Uuid,
        at: u64,
        source: SourceSelection,
    },
    Split {
        composition: Uuid,
        clip: Uuid,
        at: u64,
    },
    Trim {
        composition: Uuid,
        clip: Uuid,
        source_range: FrameRange,
        ripple: bool,
    },
    Move {
        composition: Uuid,
        clip: Uuid,
        at: u64,
    },
    Remove {
        composition: Uuid,
        clip: Uuid,
        ripple: bool,
    },
    AddTrack {
        composition: Uuid,
        name: String,
    },
    Place {
        composition: Uuid,
        track: Uuid,
        at: u64,
        source: SourceSelection,
        audio_only: bool,
    },
    Overwrite {
        composition: Uuid,
        at: u64,
        source: SourceSelection,
    },
    Slip {
        composition: Uuid,
        clip: Uuid,
        frames: i64,
    },
    Roll {
        composition: Uuid,
        clip: Uuid,
        at: u64,
    },
    Slide {
        composition: Uuid,
        clip: Uuid,
        at: u64,
    },
    SetControls {
        composition: Uuid,
        clip: Uuid,
        controls: TimelineControls,
    },
}

/// Editable picture placement and linear sound level for one linked clip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineControls {
    pub exposure: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub translation: [f64; 2],
    pub scale: [f64; 2],
    pub rotation: f64,
    pub opacity: f64,
    pub gain: f64,
}

impl Default for TimelineControls {
    fn default() -> Self {
        Self {
            exposure: 0.,
            contrast: 1.,
            saturation: 1.,
            translation: [0.; 2],
            scale: [1.; 2],
            rotation: 0.,
            opacity: 1.,
            gain: 1.,
        }
    }
}

/// A document-derived selection; this is not another persisted timeline model.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TimelineClip {
    pub track: Uuid,
    pub audio_only: bool,
    pub controls: TimelineControls,
    pub id: Uuid,
    pub linked: Option<Uuid>,
    pub name: String,
    pub range: FrameRange,
    pub source: SourceSelection,
}

/// One complete atomic change and its resulting native selection.
pub struct TimelineChange {
    pub commands: Vec<DocumentCommand>,
    pub composition: Uuid,
    pub clip: Option<Uuid>,
}

/// Inspect a supported assembly without modifying any document or source.
/// `project` owns `composition`; returns ordered linked clip projections, or a
/// visible error for a graph whose effects, rates or tracks need another editor.
pub fn timeline_clips(project: &Project, composition: Uuid) -> Result<Vec<TimelineClip>> {
    let composition = composition_in(project, composition)?;
    let clips = inspect(project, composition)?;
    let mut rebuilt = composition.clone();
    rebuild(&mut rebuilt)?;
    if rebuilt != *composition {
        return Err(invalid(
            "This composition has graph edits outside the assembly editor",
        ));
    }
    Ok(clips)
}

/// Prepare one exact source-to-record edit through the normal command path.
/// `project` is immutable and `action` selects a linked operation. Trim retains
/// record start; ripple shifts following groups by the duration change. Returns
/// validated commands and selection. Run this planning work off the UI thread.
pub fn timeline_edit(project: &Project, action: &TimelineAction) -> Result<TimelineChange> {
    project.validate()?;
    let authoring = match action {
        TimelineAction::Title {
            profile,
            name,
            text,
            font,
            size,
            position,
            rgba,
            duration,
        } => Some(crate::authoring::title_sequence(
            project,
            *profile,
            name.clone(),
            text.clone(),
            font,
            *size,
            *position,
            *rgba,
            *duration,
        )?),
        TimelineAction::Fade {
            source,
            name,
            fade_in,
            fade_out,
        } => Some(crate::authoring::faded_sequence(
            project,
            *source,
            name.clone(),
            *fade_in,
            *fade_out,
        )?),
        TimelineAction::EditTitle {
            composition,
            text,
            size,
            position,
            rgba,
        } => {
            let mut scene = composition_in(project, *composition)?.clone();
            let node = scene
                .nodes
                .iter_mut()
                .find(|node| matches!(node.operation, NodeOperation::Text { .. }))
                .ok_or_else(|| invalid("Choose a title source"))?;
            let NodeOperation::Text { font, .. } = node.operation else {
                unreachable!()
            };
            node.operation = NodeOperation::Text {
                font,
                text: text.clone(),
                size: *size,
                position: *position,
                rgba: *rgba,
            };
            let commands = vec![DocumentCommand::SetComposition { composition: scene }];
            let mut editor = DocumentEditor::new(project.clone())?;
            editor.apply(DocumentVersion::of(project), "Edit title".into(), &commands)?;
            return Ok(TimelineChange {
                commands,
                composition: *composition,
                clip: None,
            });
        }
        _ => None,
    };
    if let Some(commands) = authoring {
        let composition = commands
            .iter()
            .find_map(|command| match command {
                DocumentCommand::SetSequence { sequence } => sequence.composition,
                _ => None,
            })
            .ok_or_else(|| invalid("Authored sequence is absent"))?;
        return Ok(TimelineChange {
            commands,
            composition,
            clip: None,
        });
    }
    if let TimelineAction::Conform {
        source,
        width,
        height,
        frame_rate,
        name,
    } = action
    {
        let commands =
            crate::conform_sequence(project, *source, *width, *height, *frame_rate, name.clone())?;
        let composition = commands
            .iter()
            .find_map(|command| match command {
                DocumentCommand::SetSequence { sequence } => sequence.composition,
                _ => None,
            })
            .ok_or_else(|| invalid("Conformed sequence is absent"))?;
        return Ok(TimelineChange {
            commands,
            composition,
            clip: None,
        });
    }
    let (mut composition, selected, sequence) = match action {
        TimelineAction::Create { name, source } => {
            let original = selected_source(project, *source)?;
            let id = Uuid::new_v4();
            let mut composition = Composition {
                id,
                name: name.clone(),
                width: original.width,
                height: original.height,
                frame_rate: original.frame_rate,
                duration: 1,
                picture: None,
                audio: None,
                tracks: vec![
                    track(TrackKind::Video, "Video 1"),
                    track(TrackKind::Audio, "Audio 1"),
                ],
                nodes: Vec::new(),
            };
            let selected = add_group(&mut composition, original, *source, 0)?;
            let sequence = Sequence {
                id: Uuid::new_v4(),
                name: name.clone(),
                width: original.width,
                height: original.height,
                frame_rate: original.frame_rate,
                composition: Some(id),
            };
            (composition, Some(selected), Some(sequence))
        }
        _ => {
            let id = match action {
                TimelineAction::Insert { composition, .. }
                | TimelineAction::Split { composition, .. }
                | TimelineAction::Trim { composition, .. }
                | TimelineAction::Move { composition, .. }
                | TimelineAction::Remove { composition, .. } => *composition,
                TimelineAction::AddTrack { composition, .. }
                | TimelineAction::Place { composition, .. }
                | TimelineAction::Overwrite { composition, .. }
                | TimelineAction::Slip { composition, .. }
                | TimelineAction::Roll { composition, .. }
                | TimelineAction::Slide { composition, .. }
                | TimelineAction::SetControls { composition, .. } => *composition,
                TimelineAction::Create { .. }
                | TimelineAction::Conform { .. }
                | TimelineAction::Title { .. }
                | TimelineAction::Fade { .. }
                | TimelineAction::EditTitle { .. } => unreachable!(),
            };
            let groups = timeline_clips(project, id)?;
            let mut composition = composition_in(project, id)?.clone();
            let selected = match action {
                TimelineAction::Insert { at, source, .. } => {
                    let original = selected_source(project, *source)?;
                    if (original.width, original.height, original.frame_rate)
                        != (
                            composition.width,
                            composition.height,
                            composition.frame_rate,
                        )
                    {
                        return Err(invalid(
                            "Source and record profiles must match; conform mixed dimensions or rates explicitly",
                        ));
                    }
                    if *at > composition.duration {
                        return Err(invalid("Insert is beyond the record end"));
                    }
                    if let Some(group) = groups
                        .iter()
                        .find(|group| group.range.start < *at && *at < group.range.end)
                    {
                        split_group(&mut composition, group, *at)?;
                    }
                    let length = source.range.end - source.range.start;
                    shift(&mut composition, *at, i128::from(length))?;
                    Some(add_group(&mut composition, original, *source, *at)?)
                }
                TimelineAction::AddTrack { name, .. } => {
                    composition
                        .tracks
                        .push(track(TrackKind::Video, &format!("{name} picture")));
                    composition
                        .tracks
                        .push(track(TrackKind::Audio, &format!("{name} sound")));
                    None
                }
                TimelineAction::Place {
                    track,
                    at,
                    source,
                    audio_only,
                    ..
                } => {
                    let original = composition_in(project, source.composition)?;
                    source.range.validate(original.duration)?;
                    matching(&composition, original)?;
                    let index = composition
                        .tracks
                        .iter()
                        .position(|item| item.id == *track)
                        .ok_or_else(|| invalid("Target track is absent"))?;
                    if *audio_only {
                        if composition.tracks[index].kind != TrackKind::Audio
                            || original.audio.is_none()
                        {
                            return Err(invalid("Choose an audio track and a source with sound"));
                        }
                        let id = Uuid::new_v4();
                        composition.tracks[index].clips.push(Clip {
                            id,
                            name: original.name.clone(),
                            range: FrameRange {
                                start: *at,
                                end: end(*at, source.range.end - source.range.start)?,
                            },
                            source: ClipSource::Composition {
                                composition: original.id,
                            },
                            time_map: unity_map(source.range),
                            linked: None,
                        });
                        Some(id)
                    } else {
                        selected_source(project, *source)?;
                        if !index.is_multiple_of(2) {
                            return Err(invalid("Choose a picture track for linked placement"));
                        }
                        Some(add_group_at(
                            &mut composition,
                            original,
                            *source,
                            *at,
                            index,
                        )?)
                    }
                }
                TimelineAction::Overwrite { at, source, .. } => {
                    let original = selected_source(project, *source)?;
                    matching(&composition, original)?;
                    let last = end(*at, source.range.end - source.range.start)?;
                    for boundary in [*at, last] {
                        let current = inspect(project, &composition)?;
                        if let Some(group) = current.iter().find(|group| {
                            group.track == composition.tracks[0].id
                                && group.range.start < boundary
                                && boundary < group.range.end
                        }) {
                            split_group(&mut composition, group, boundary)?;
                        }
                    }
                    let removed: HashSet<_> = inspect(project, &composition)?
                        .into_iter()
                        .filter(|group| {
                            group.track == composition.tracks[0].id
                                && group.range.start >= *at
                                && group.range.end <= last
                        })
                        .flat_map(|group| [Some(group.id), group.linked].into_iter().flatten())
                        .collect();
                    for track in &mut composition.tracks {
                        track.clips.retain(|clip| !removed.contains(&clip.id));
                    }
                    Some(add_group(&mut composition, original, *source, *at)?)
                }
                TimelineAction::Slip { clip, frames, .. } => {
                    let group = group(&groups, *clip)?;
                    let selection = FrameRange {
                        start: shifted(group.source.range.start, i128::from(*frames))?,
                        end: shifted(group.source.range.end, i128::from(*frames))?,
                    };
                    selection
                        .validate(composition_in(project, group.source.composition)?.duration)?;
                    for clip in members_mut(&mut composition, group) {
                        clip.time_map = unity_map(selection);
                    }
                    Some(group.id)
                }
                TimelineAction::Roll { clip, at, .. } => {
                    let left = group(&groups, *clip)?;
                    let right = groups
                        .iter()
                        .find(|item| item.track == left.track && item.range.start == left.range.end)
                        .ok_or_else(|| {
                            invalid("Roll requires a touching following clip on the same track")
                        })?;
                    roll(project, &mut composition, left, right, *at)?;
                    Some(left.id)
                }
                TimelineAction::Slide { clip, at, .. } => {
                    let middle = group(&groups, *clip)?;
                    let left = groups
                        .iter()
                        .find(|item| {
                            item.track == middle.track && item.range.end == middle.range.start
                        })
                        .ok_or_else(|| invalid("Slide requires a touching preceding clip"))?;
                    let right = groups
                        .iter()
                        .find(|item| {
                            item.track == middle.track && item.range.start == middle.range.end
                        })
                        .ok_or_else(|| invalid("Slide requires a touching following clip"))?;
                    let delta = i128::from(*at) - i128::from(middle.range.start);
                    let mut moved_middle = middle.clone();
                    moved_middle.range.start = *at;
                    moved_middle.range.end = shifted(middle.range.end, delta)?;
                    let left_range = FrameRange {
                        start: left.source.range.start,
                        end: shifted(left.source.range.end, delta)?,
                    };
                    let right_range = FrameRange {
                        start: shifted(right.source.range.start, delta)?,
                        end: right.source.range.end,
                    };
                    left_range
                        .validate(composition_in(project, left.source.composition)?.duration)?;
                    right_range
                        .validate(composition_in(project, right.source.composition)?.duration)?;
                    if *at <= left.range.start || moved_middle.range.end >= right.range.end {
                        return Err(invalid("Slide must retain both neighbouring clips"));
                    }
                    for clip in members_mut(&mut composition, left) {
                        clip.range.end = *at;
                        clip.time_map = unity_map(left_range);
                    }
                    for clip in members_mut(&mut composition, middle) {
                        clip.range = moved_middle.range;
                    }
                    for clip in members_mut(&mut composition, right) {
                        clip.range.start = moved_middle.range.end;
                        clip.time_map = unity_map(right_range);
                    }
                    Some(middle.id)
                }
                TimelineAction::SetControls { clip, controls, .. } => {
                    let group = group(&groups, *clip)?;
                    set_controls(&mut composition, group, *controls)?;
                    Some(group.id)
                }
                TimelineAction::Split { clip, at, .. } => {
                    Some(split_group(&mut composition, group(&groups, *clip)?, *at)?)
                }
                TimelineAction::Trim {
                    clip,
                    source_range,
                    ripple,
                    ..
                } => {
                    let group = group(&groups, *clip)?;
                    let selection = SourceSelection {
                        composition: group.source.composition,
                        range: *source_range,
                    };
                    selected_source(project, selection)?;
                    let length = source_range.end - source_range.start;
                    if *ripple {
                        shift(
                            &mut composition,
                            group.range.end,
                            i128::from(length) - i128::from(group.range.end - group.range.start),
                        )?;
                    }
                    for clip in members_mut(&mut composition, group) {
                        clip.range.end = end(clip.range.start, length)?;
                        clip.time_map = unity_map(*source_range);
                    }
                    Some(group.id)
                }
                TimelineAction::Move { clip, at, .. } => {
                    let group = group(&groups, *clip)?;
                    for clip in members_mut(&mut composition, group) {
                        clip.range = FrameRange {
                            start: *at,
                            end: end(*at, group.range.end - group.range.start)?,
                        };
                    }
                    Some(group.id)
                }
                TimelineAction::Remove { clip, ripple, .. } => {
                    let group = group(&groups, *clip)?;
                    for track in &mut composition.tracks {
                        track
                            .clips
                            .retain(|clip| clip.id != group.id && Some(clip.id) != group.linked);
                    }
                    if *ripple {
                        shift(
                            &mut composition,
                            group.range.end,
                            -i128::from(group.range.end - group.range.start),
                        )?;
                    }
                    None
                }
                TimelineAction::Create { .. }
                | TimelineAction::Conform { .. }
                | TimelineAction::Title { .. }
                | TimelineAction::Fade { .. }
                | TimelineAction::EditTitle { .. } => unreachable!(),
            };
            (composition, selected, None)
        }
    };
    for track in &mut composition.tracks {
        track.clips.sort_by_key(|clip| (clip.range.start, clip.id));
    }
    composition.duration = composition
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .map(|clip| clip.range.end)
        .max()
        .unwrap_or(1);
    inspect(project, &composition)?;
    rebuild(&mut composition)?;
    let id = composition.id;
    let mut commands = vec![DocumentCommand::SetComposition { composition }];
    if let Some(sequence) = sequence {
        let sequence_id = sequence.id;
        commands.push(DocumentCommand::SetSequence { sequence });
        commands.push(DocumentCommand::SetPrimarySequence { id: sequence_id });
    }
    let mut editor = DocumentEditor::new(project.clone())?;
    editor.apply(
        DocumentVersion::of(project),
        "Timeline edit".into(),
        &commands,
    )?;
    let sound = SoundSnapshot::at_output_rate(
        Arc::new(EvaluationSnapshot::new(editor.snapshot())?),
        id,
        48000,
        SoundBudget::default(),
    )?;
    sound.prepare(0, sound.duration_samples().min(4096) as u32)?;
    Ok(TimelineChange {
        commands,
        composition: id,
        clip: selected,
    })
}

fn composition_in(project: &Project, id: Uuid) -> Result<&Composition> {
    project
        .compositions
        .iter()
        .find(|composition| composition.id == id)
        .ok_or_else(|| invalid("Composition is absent"))
}

fn selected_source(project: &Project, source: SourceSelection) -> Result<&Composition> {
    let composition = composition_in(project, source.composition)?;
    if composition.picture.is_none() {
        return Err(invalid("Choose a source with picture output"));
    }
    source.range.validate(composition.duration)?;
    Ok(composition)
}

fn inspect(project: &Project, composition: &Composition) -> Result<Vec<TimelineClip>> {
    if composition.tracks.is_empty() || !composition.tracks.len().is_multiple_of(2) {
        return Err(invalid("Choose a cut with paired picture and sound tracks"));
    }
    let mut groups = Vec::new();
    for pair in composition.tracks.as_chunks::<2>().0 {
        if pair[0].kind != TrackKind::Video || pair[1].kind != TrackKind::Audio {
            return Err(invalid("Cut tracks must alternate picture and sound"));
        }
        let mut linked_sound = HashSet::new();
        for track in pair {
            let mut previous = 0;
            for clip in &track.clips {
                if clip.range.start < previous {
                    return Err(invalid("Clips cannot overlap on the same track"));
                }
                previous = clip.range.end;
                clip.range.validate(composition.duration)?;
                let ClipSource::Composition {
                    composition: source,
                } = clip.source
                else {
                    return Err(invalid(
                        "Create a source sequence before editing this media",
                    ));
                };
                let child = composition_in(project, source)?;
                matching(composition, child)?;
                let points = &clip.time_map.points;
                let length = clip.range.end - clip.range.start;
                if clip.time_map.source_denominator != 1
                    || points.len() != 2
                    || points[0].frame != 0
                    || points[0].source_tick < 0
                    || points[1].frame != length
                    || i128::from(points[1].source_tick) - i128::from(points[0].source_tick)
                        != i128::from(length)
                {
                    return Err(invalid("Conform the source before exact frame editing"));
                }
                let range = FrameRange {
                    start: points[0].source_tick as u64,
                    end: points[1].source_tick as u64,
                };
                range.validate(child.duration)?;
                if track.kind == TrackKind::Audio {
                    if child.audio.is_none() {
                        return Err(invalid("Audio track source has no sound"));
                    }
                    if clip.linked.is_some() {
                        if !linked_sound.contains(&clip.id) {
                            return Err(invalid("Sound link has no matching picture"));
                        }
                        continue;
                    }
                } else {
                    if child.picture.is_none() {
                        return Err(invalid("Picture track source has no picture"));
                    }
                    match (child.audio, clip.linked) {
                        (Some(_), Some(linked)) => {
                            let paired = pair[1]
                                .clips
                                .iter()
                                .find(|paired| paired.id == linked)
                                .ok_or_else(|| invalid("Linked sound clip is absent"))?;
                            if paired.linked != Some(clip.id)
                                || paired.range != clip.range
                                || paired.source != clip.source
                                || paired.time_map != clip.time_map
                                || !linked_sound.insert(paired.id)
                            {
                                return Err(invalid(
                                    "Linked picture and sound differ in source or timing",
                                ));
                            }
                        }
                        (None, None) => {}
                        _ => return Err(invalid("Source sound and its linked track disagree")),
                    }
                }
                groups.push(TimelineClip {
                    track: track.id,
                    audio_only: track.kind == TrackKind::Audio,
                    controls: clip_controls(composition, clip.id, clip.linked),
                    id: clip.id,
                    linked: clip.linked,
                    name: clip.name.clone(),
                    range: clip.range,
                    source: SourceSelection {
                        composition: source,
                        range,
                    },
                });
            }
        }
    }
    groups.sort_by_key(|group| (group.range.start, group.track, group.id));
    Ok(groups)
}

fn matching(record: &Composition, source: &Composition) -> Result<()> {
    if (record.width, record.height, record.frame_rate)
        != (source.width, source.height, source.frame_rate)
    {
        return Err(invalid(
            "Source and record profiles must match; conform this source first",
        ));
    }
    Ok(())
}

fn shifted(value: u64, delta: i128) -> Result<u64> {
    u64::try_from(i128::from(value) + delta)
        .map_err(|_| invalid("Edit exceeds source handles or integer time"))
}

fn roll(
    project: &Project,
    composition: &mut Composition,
    left: &TimelineClip,
    right: &TimelineClip,
    at: u64,
) -> Result<()> {
    if at <= left.range.start || at >= right.range.end {
        return Err(invalid("Roll must retain both clips"));
    }
    let delta = i128::from(at) - i128::from(left.range.end);
    let left_range = FrameRange {
        start: left.source.range.start,
        end: shifted(left.source.range.end, delta)?,
    };
    let right_range = FrameRange {
        start: shifted(right.source.range.start, delta)?,
        end: right.source.range.end,
    };
    left_range.validate(composition_in(project, left.source.composition)?.duration)?;
    right_range.validate(composition_in(project, right.source.composition)?.duration)?;
    for clip in members_mut(composition, left) {
        clip.range.end = at;
        clip.time_map = unity_map(left_range);
    }
    for clip in members_mut(composition, right) {
        clip.range.start = at;
        clip.time_map = unity_map(right_range);
    }
    Ok(())
}

fn clip_controls(composition: &Composition, clip: Uuid, linked: Option<Uuid>) -> TimelineControls {
    let mut result = TimelineControls::default();
    let source = |id| {
        composition
            .nodes
            .iter()
            .find(|node| node.operation == NodeOperation::Source { clip: id })
            .map(|node| node.id)
    };
    let original_picture = source(clip);
    let picture = composition
        .nodes
        .iter()
        .find_map(|node| match node.operation {
            NodeOperation::Color {
                image,
                exposure,
                contrast,
                saturation,
            } if Some(image) == original_picture => {
                result.exposure = exposure;
                result.contrast = contrast;
                result.saturation = saturation;
                Some(node.id)
            }
            _ => None,
        })
        .or(original_picture);
    let audio = if composition.tracks.iter().any(|track| {
        track.kind == TrackKind::Audio && track.clips.iter().any(|item| item.id == clip)
    }) {
        picture
    } else {
        linked.and_then(source)
    };
    for node in &composition.nodes {
        match node.operation {
            NodeOperation::Transform {
                image,
                translation,
                scale,
                rotation,
                opacity,
            } if Some(image) == picture => {
                result.translation = translation;
                result.scale = scale;
                result.rotation = rotation;
                result.opacity = opacity;
            }
            NodeOperation::Gain { audio: input, gain } if Some(input) == audio => {
                result.gain = gain
            }
            _ => {}
        }
    }
    result
}

fn set_controls(
    composition: &mut Composition,
    group: &TimelineClip,
    controls: TimelineControls,
) -> Result<()> {
    if controls
        .translation
        .iter()
        .chain(&controls.scale)
        .chain([&controls.rotation, &controls.opacity, &controls.gain])
        .any(|value| !value.is_finite())
        || !(0. ..=1.).contains(&controls.opacity)
        || !(0. ..=16.).contains(&controls.gain)
        || !controls.exposure.is_finite()
        || !(-10. ..=10.).contains(&controls.exposure)
        || !controls.contrast.is_finite()
        || !(0. ..=4.).contains(&controls.contrast)
        || !controls.saturation.is_finite()
        || !(0. ..=4.).contains(&controls.saturation)
        || controls
            .scale
            .iter()
            .any(|value| value.abs() < 0.0001 || value.abs() > 100.)
    {
        return Err(invalid(
            "Clip controls require finite placement, visible scale, opacity 0–1 and gain 0–16",
        ));
    }
    let source = |id| {
        composition
            .nodes
            .iter()
            .find(|node| node.operation == NodeOperation::Source { clip: id })
            .map(|node| node.id)
    };
    let picture = source(group.id).ok_or_else(|| invalid("Selected clip has no graph source"))?;
    let audio = if group.audio_only {
        Some(picture)
    } else {
        group.linked.and_then(source)
    };
    let update = |operation: NodeOperation, nodes: &mut Vec<TimedNode>| {
        let existing = nodes
            .iter_mut()
            .find(|node| match (&node.operation, &operation) {
                (
                    NodeOperation::Transform { image: a, .. },
                    NodeOperation::Transform { image: b, .. },
                ) => a == b,
                (NodeOperation::Gain { audio: a, .. }, NodeOperation::Gain { audio: b, .. }) => {
                    a == b
                }
                (NodeOperation::Color { image: a, .. }, NodeOperation::Color { image: b, .. }) => {
                    a == b
                }
                _ => false,
            });
        if let Some(node) = existing {
            node.operation = operation;
        } else {
            nodes.push(TimedNode {
                id: Uuid::new_v4(),
                range: group.range,
                operation,
                animation: Vec::new(),
            });
        }
    };
    if !group.audio_only {
        update(
            NodeOperation::Color {
                image: picture,
                exposure: controls.exposure,
                contrast: controls.contrast,
                saturation: controls.saturation,
            },
            &mut composition.nodes,
        );
        let colored = composition.nodes.iter().find(|node| matches!(node.operation, NodeOperation::Color { image, .. } if image == picture)).unwrap().id;
        update(
            NodeOperation::Transform {
                image: colored,
                translation: controls.translation,
                scale: controls.scale,
                rotation: controls.rotation,
                opacity: controls.opacity,
            },
            &mut composition.nodes,
        );
    }
    if let Some(audio) = audio {
        update(
            NodeOperation::Gain {
                audio,
                gain: controls.gain,
            },
            &mut composition.nodes,
        );
    }
    Ok(())
}

fn track(kind: TrackKind, name: &str) -> Track {
    Track {
        id: Uuid::new_v4(),
        name: name.into(),
        kind,
        enabled: true,
        clips: Vec::new(),
    }
}

fn unity_map(range: FrameRange) -> TimeMap {
    TimeMap {
        source_denominator: 1,
        points: vec![
            TimePoint {
                frame: 0,
                source_tick: range.start as i64,
            },
            TimePoint {
                frame: range.end - range.start,
                source_tick: range.end as i64,
            },
        ],
    }
}

fn add_group(
    composition: &mut Composition,
    original: &Composition,
    source: SourceSelection,
    at: u64,
) -> Result<Uuid> {
    add_group_at(composition, original, source, at, 0)
}

fn add_group_at(
    composition: &mut Composition,
    original: &Composition,
    source: SourceSelection,
    at: u64,
    track: usize,
) -> Result<Uuid> {
    let picture = Uuid::new_v4();
    let audio = original.audio.map(|_| Uuid::new_v4());
    let clip = Clip {
        id: picture,
        name: original.name.clone(),
        range: FrameRange {
            start: at,
            end: end(at, source.range.end - source.range.start)?,
        },
        source: ClipSource::Composition {
            composition: source.composition,
        },
        time_map: unity_map(source.range),
        linked: audio,
    };
    composition.tracks[track].clips.push(clip.clone());
    if let Some(id) = audio {
        composition.tracks[track + 1].clips.push(Clip {
            id,
            linked: Some(picture),
            ..clip
        });
    }
    Ok(picture)
}

fn group(groups: &[TimelineClip], clip: Uuid) -> Result<&TimelineClip> {
    groups
        .iter()
        .find(|group| group.id == clip || group.linked == Some(clip))
        .ok_or_else(|| invalid("Selected linked clip is absent"))
}

fn members_mut<'a>(
    composition: &'a mut Composition,
    group: &TimelineClip,
) -> impl Iterator<Item = &'a mut Clip> {
    let id = group.id;
    let linked = group.linked;
    composition
        .tracks
        .iter_mut()
        .flat_map(|track| &mut track.clips)
        .filter(move |clip| clip.id == id || Some(clip.id) == linked)
}

fn split_group(composition: &mut Composition, group: &TimelineClip, at: u64) -> Result<Uuid> {
    if at <= group.range.start || at >= group.range.end {
        return Err(invalid("Split must be inside the selected clip"));
    }
    let tick = group.source.range.start + (at - group.range.start);
    let picture = Uuid::new_v4();
    let audio = group.linked.map(|_| Uuid::new_v4());
    for track in &mut composition.tracks {
        let Some(index) = track
            .clips
            .iter()
            .position(|clip| clip.id == group.id || Some(clip.id) == group.linked)
        else {
            continue;
        };
        let left = &mut track.clips[index];
        let mut right = left.clone();
        left.range.end = at;
        left.time_map = unity_map(FrameRange {
            start: group.source.range.start,
            end: tick,
        });
        right.id = if left.id == group.id {
            picture
        } else {
            audio.ok_or_else(|| invalid("Split sound identity is absent"))?
        };
        right.linked = if left.id == group.id {
            audio
        } else {
            Some(picture)
        };
        right.range.start = at;
        right.time_map = unity_map(FrameRange {
            start: tick,
            end: group.source.range.end,
        });
        track.clips.insert(index + 1, right);
    }
    let range = FrameRange {
        start: at,
        end: group.range.end,
    };
    for clip in [Some(picture), audio].into_iter().flatten() {
        composition.nodes.push(TimedNode {
            id: Uuid::new_v4(),
            range,
            operation: NodeOperation::Source { clip },
            animation: Vec::new(),
        });
    }
    let mut right = group.clone();
    right.id = picture;
    right.linked = audio;
    right.range = range;
    set_controls(composition, &right, group.controls)?;
    Ok(picture)
}

fn shift(composition: &mut Composition, from: u64, delta: i128) -> Result<()> {
    for clip in composition
        .tracks
        .iter_mut()
        .flat_map(|track| &mut track.clips)
        .filter(|clip| clip.range.start >= from)
    {
        let shifted = |value| {
            u64::try_from(i128::from(value) + delta)
                .map_err(|_| invalid("Timeline shift exceeds integer time"))
        };
        clip.range = FrameRange {
            start: shifted(clip.range.start)?,
            end: shifted(clip.range.end)?,
        };
    }
    Ok(())
}

fn rebuild(composition: &mut Composition) -> Result<()> {
    let range = FrameRange {
        start: 0,
        end: composition.duration,
    };
    let mut nodes = Vec::new();
    let controls: std::collections::HashMap<_, _> = composition
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .map(|clip| (clip.id, clip_controls(composition, clip.id, clip.linked)))
        .collect();
    let mut retain = |operation: NodeOperation, range: FrameRange| {
        let id = composition
            .nodes
            .iter()
            .find(|node| node.operation == operation)
            .map_or_else(Uuid::new_v4, |node| node.id);
        nodes.push(TimedNode {
            id,
            range,
            operation,
            animation: Vec::new(),
        });
        id
    };
    let mut picture = retain(NodeOperation::Solid { rgba: [0.; 4] }, range);
    for clip in composition
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
        .flat_map(|track| &track.clips)
    {
        let mut source = retain(NodeOperation::Source { clip: clip.id }, clip.range);
        let setting = controls[&clip.id];
        if setting.exposure != 0. || setting.contrast != 1. || setting.saturation != 1. {
            source = retain(
                NodeOperation::Color {
                    image: source,
                    exposure: setting.exposure,
                    contrast: setting.contrast,
                    saturation: setting.saturation,
                },
                clip.range,
            );
        }
        if setting.translation != [0.; 2]
            || setting.scale != [1.; 2]
            || setting.rotation != 0.
            || setting.opacity != 1.
        {
            source = retain(
                NodeOperation::Transform {
                    image: source,
                    translation: setting.translation,
                    scale: setting.scale,
                    rotation: setting.rotation,
                    opacity: setting.opacity,
                },
                clip.range,
            );
        }
        picture = retain(
            NodeOperation::Over {
                foreground: source,
                background: picture,
                mask: None,
            },
            range,
        );
    }
    let sound: Vec<_> = composition
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio)
        .flat_map(|track| &track.clips)
        .map(|clip| {
            let source = retain(NodeOperation::Source { clip: clip.id }, clip.range);
            let gain = controls[&clip.id].gain;
            if gain == 1. {
                source
            } else {
                retain(
                    NodeOperation::Gain {
                        audio: source,
                        gain,
                    },
                    clip.range,
                )
            }
        })
        .collect();
    composition.audio = match sound.as_slice() {
        [] => None,
        [source] => Some(*source),
        _ => Some(retain(NodeOperation::Mix { inputs: sound }, range)),
    };
    composition.picture = Some(picture);
    composition.nodes = nodes;
    Ok(())
}

fn end(start: u64, length: u64) -> Result<u64> {
    start
        .checked_add(length)
        .filter(|end| *end <= i64::MAX as u64)
        .ok_or_else(|| invalid("Timeline exceeds supported integer time"))
}

fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
