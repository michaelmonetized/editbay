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

/// Exact linked edits on a single picture track and its paired sound track.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineAction {
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
}

/// A document-derived selection; this is not another persisted timeline model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TimelineClip {
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
                TimelineAction::Create { .. } => unreachable!(),
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
                TimelineAction::Create { .. } => unreachable!(),
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
        commands.push(DocumentCommand::SetSequence { sequence });
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
    if composition.tracks.len() != 2
        || composition.tracks[0].kind != TrackKind::Video
        || composition.tracks[1].kind != TrackKind::Audio
    {
        return Err(invalid(
            "Choose an assembly with one video track and its linked audio track",
        ));
    }
    let mut groups = Vec::new();
    let mut sound = HashSet::new();
    let mut previous = 0;
    for clip in &composition.tracks[0].clips {
        if clip.range.start < previous {
            return Err(invalid("Linked edits cannot overlap on this track"));
        }
        clip.range.validate(composition.duration)?;
        previous = clip.range.end;
        let ClipSource::Composition {
            composition: source,
        } = clip.source
        else {
            return Err(invalid(
                "Create an assembly from this source before editing it",
            ));
        };
        let child = composition_in(project, source)?;
        if (child.width, child.height, child.frame_rate)
            != (
                composition.width,
                composition.height,
                composition.frame_rate,
            )
        {
            return Err(invalid("Assembly source profiles differ"));
        }
        let points = &clip.time_map.points;
        let length = clip.range.end - clip.range.start;
        if points.len() != 2
            || points[0].frame != 0
            || points[0].source_tick < 0
            || points[1].frame != length
            || i128::from(points[1].source_tick) - i128::from(points[0].source_tick)
                != i128::from(length)
        {
            return Err(invalid(
                "This clip needs exact unity-rate composition time for assembly editing",
            ));
        }
        let range = FrameRange {
            start: points[0].source_tick as u64,
            end: points[1].source_tick as u64,
        };
        selected_source(
            project,
            SourceSelection {
                composition: source,
                range,
            },
        )?;
        match (child.audio, clip.linked) {
            (Some(_), Some(linked)) => {
                let paired = composition.tracks[1]
                    .clips
                    .iter()
                    .find(|paired| paired.id == linked)
                    .ok_or_else(|| invalid("Linked sound clip is absent"))?;
                if paired.linked != Some(clip.id)
                    || paired.range != clip.range
                    || paired.source != clip.source
                    || paired.time_map != clip.time_map
                    || !sound.insert(paired.id)
                {
                    return Err(invalid(
                        "Linked picture and sound differ in source or timing",
                    ));
                }
            }
            (None, None) => {}
            _ => return Err(invalid("Source sound and the linked track disagree")),
        }
        groups.push(TimelineClip {
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
    if sound.len() != composition.tracks[1].clips.len() {
        return Err(invalid("Assembly contains unpaired sound"));
    }
    Ok(groups)
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
    composition.tracks[0].clips.push(clip.clone());
    if let Some(id) = audio {
        composition.tracks[1].clips.push(Clip {
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
        right.id = if track.kind == TrackKind::Video {
            picture
        } else {
            audio.ok_or_else(|| invalid("Split sound identity is absent"))?
        };
        right.linked = if track.kind == TrackKind::Video {
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
    for clip in &composition.tracks[0].clips {
        let source = retain(NodeOperation::Source { clip: clip.id }, clip.range);
        picture = retain(
            NodeOperation::Over {
                foreground: source,
                background: picture,
                mask: None,
            },
            range,
        );
    }
    let sound: Vec<_> = composition.tracks[1]
        .clips
        .iter()
        .map(|clip| retain(NodeOperation::Source { clip: clip.id }, clip.range))
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
