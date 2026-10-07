use editbay_core::{
    DocumentCommand, DocumentEditor, DocumentVersion, FrameRange, FrameRate, Project,
    SourceSelection, TimelineAction, TimelineControls,
};
use editbay_media::{Cancellation, SourceFile};
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use uuid::Uuid;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: String,
    width: u32,
    height: u32,
    frame_rate: FrameRate,
    clips: Vec<Shot>,
    #[serde(default)]
    titles: Vec<Title>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Shot {
    path: PathBuf,
    video_stream: Option<u32>,
    audio_stream: Option<u32>,
    range: FrameRange,
    at: u64,
    track: usize,
    controls: Option<TimelineControls>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Title {
    name: String,
    text: String,
    font: PathBuf,
    size: f64,
    position: [f64; 2],
    rgba: [f64; 4],
    range: FrameRange,
    track: usize,
    #[serde(default)]
    fade_in: u64,
    #[serde(default)]
    fade_out: u64,
}

/// Author a real source job through the same commands used by the native editor.
/// `manifest` declares exact record-frame source selections; `destination` must
/// be new. Returns the saved record identity and provenance. Retained media is
/// fingerprinted, indexed and verified; no source or existing project is written.
pub(super) fn build(manifest: &Path, destination: &Path) -> Result<serde_json::Value> {
    let input = super::read_bounded(manifest, 1024 * 1024)?;
    let manifest: Manifest = serde_json::from_slice(&input)?;
    if manifest.clips.is_empty()
        || manifest.clips.len() > 256
        || manifest.titles.len() > 64
        || manifest.clips.iter().any(|s| s.track >= 16)
        || manifest.titles.iter().any(|t| t.track >= 16)
    {
        return Err("Job requires 1–256 source clips, at most 64 titles and 16 track pairs".into());
    }
    let mut project = Project::new(&manifest.name)?;
    project.sequences[0].width = manifest.width;
    project.sequences[0].height = manifest.height;
    project.sequences[0].frame_rate = manifest.frame_rate;
    let mut editor = DocumentEditor::new(project)?;
    let cancel = Cancellation::new()?;
    let mut originals = Vec::new();
    let mut retained = HashMap::new();
    let mut sources = Vec::new();
    for shot in &manifest.clips {
        let path = std::fs::canonicalize(&shot.path)?;
        let key = (path.clone(), shot.video_stream, shot.audio_stream);
        let composition = if let Some(&id) = retained.get(&key) {
            id
        } else {
            let file = SourceFile::open(&path, &cancel)?;
            let mut indices: Vec<_> = [shot.video_stream, shot.audio_stream]
                .into_iter()
                .flatten()
                .collect();
            indices.sort_unstable();
            indices.dedup();
            let name = path
                .file_name()
                .ok_or("Source filename is absent")?
                .to_string_lossy()
                .into_owned();
            let imported = file.ingest(name.clone(), &indices, cancel.clone(), |_, _| {})?;
            apply(&mut editor, &imported.commands())?;
            let commands = match (shot.video_stream, shot.audio_stream) {
                (Some(video), Some(audio)) => editbay_core::sequence_from_video_with_audio(
                    editor.project(),
                    imported.source.id,
                    video,
                    audio,
                    manifest.frame_rate,
                )?,
                (Some(video), None) => editbay_core::sequence_from_video(
                    editor.project(),
                    imported.source.id,
                    video,
                    manifest.frame_rate,
                )?,
                (None, Some(audio)) => editbay_core::sequence_from_audio(
                    editor.project(),
                    imported.source.id,
                    audio,
                    manifest.width,
                    manifest.height,
                    manifest.frame_rate,
                )?,
                _ => return Err("Choose at least one explicit video or audio stream".into()),
            };
            let original = output(&commands)?;
            apply(&mut editor, &commands)?;
            let commands = editbay_core::conform_sequence(
                editor.project(),
                original,
                manifest.width,
                manifest.height,
                manifest.frame_rate,
                format!("{name} · record source"),
            )?;
            let id = output(&commands)?;
            apply(&mut editor, &commands)?;
            file.verify(&cancel)?;
            originals.push(file);
            retained.insert(key, id);
            id
        };
        sources.push(SourceSelection {
            composition,
            range: shot.range,
        });
    }
    let first = manifest
        .clips
        .iter()
        .position(|shot| shot.video_stream.is_some() && shot.track == 0 && shot.at == 0)
        .ok_or("Job needs a picture clip at frame zero on track zero")?;
    let change = editbay_core::timeline_edit(
        editor.project(),
        &TimelineAction::Create {
            name: manifest.name.clone(),
            source: sources[first],
        },
    )?;
    let record = change.composition;
    let first_clip = change.clip.ok_or("Record clip is absent")?;
    apply(&mut editor, &change.commands)?;
    let pairs = manifest
        .clips
        .iter()
        .map(|s| s.track)
        .chain(manifest.titles.iter().map(|t| t.track))
        .max()
        .unwrap_or(0)
        + 1;
    for index in 1..pairs {
        edit(
            &mut editor,
            TimelineAction::AddTrack {
                composition: record,
                name: format!("Track {}", index + 1),
            },
        )?;
    }
    for (index, shot) in manifest.clips.iter().enumerate() {
        let clip = if index == first {
            first_clip
        } else {
            let scene = editor
                .project()
                .compositions
                .iter()
                .find(|c| c.id == record)
                .unwrap();
            let audio_only = shot.video_stream.is_none();
            let track = scene.tracks[shot.track * 2 + usize::from(audio_only)].id;
            edit(
                &mut editor,
                TimelineAction::Place {
                    composition: record,
                    track,
                    at: shot.at,
                    source: sources[index],
                    audio_only,
                },
            )?
            .ok_or("Placed clip is absent")?
        };
        if let Some(controls) = shot.controls {
            edit(
                &mut editor,
                TimelineAction::SetControls {
                    composition: record,
                    clip,
                    controls,
                },
            )?;
        }
    }
    for title in manifest.titles {
        let duration = title
            .range
            .end
            .checked_sub(title.range.start)
            .filter(|length| *length > 0)
            .ok_or("Title range is empty")?;
        let change = editbay_core::timeline_edit(
            editor.project(),
            &TimelineAction::Title {
                profile: record,
                name: title.name.clone(),
                text: title.text,
                font: title.font,
                size: title.size,
                position: title.position,
                rgba: title.rgba,
                duration,
            },
        )?;
        let mut source = change.composition;
        apply(&mut editor, &change.commands)?;
        if title.fade_in > 0 || title.fade_out > 0 {
            let change = editbay_core::timeline_edit(
                editor.project(),
                &TimelineAction::Fade {
                    source,
                    name: format!("{} · fades", title.name),
                    fade_in: title.fade_in,
                    fade_out: title.fade_out,
                },
            )?;
            source = change.composition;
            apply(&mut editor, &change.commands)?;
        }
        let scene = editor
            .project()
            .compositions
            .iter()
            .find(|c| c.id == record)
            .unwrap();
        let track = scene.tracks[title.track * 2].id;
        edit(
            &mut editor,
            TimelineAction::Place {
                composition: record,
                track,
                at: title.range.start,
                source: SourceSelection {
                    composition: source,
                    range: FrameRange {
                        start: 0,
                        end: duration,
                    },
                },
                audio_only: false,
            },
        )?;
    }
    for source in &originals {
        source.verify(&cancel)?;
    }
    let record_sequence = editor
        .project()
        .sequences
        .iter()
        .find(|sequence| sequence.composition == Some(record))
        .ok_or("Record sequence is absent")?
        .id;
    apply(
        &mut editor,
        &[DocumentCommand::SetPrimarySequence {
            id: record_sequence,
        }],
    )?;
    editbay_core::save_new(editor.project(), destination)?;
    let scene = editor
        .project()
        .compositions
        .iter()
        .find(|c| c.id == record)
        .unwrap();
    let sound = editbay_core::SoundSnapshot::at_output_rate(
        std::sync::Arc::new(editbay_core::EvaluationSnapshot::new(editor.snapshot())?),
        record,
        48000,
        editbay_core::SoundBudget::default(),
    )?;
    Ok(
        serde_json::json!({"project":destination,"composition":record,"version":DocumentVersion::of(editor.project()),
        "frames":scene.duration,"width":scene.width,"height":scene.height,"frame_rate":scene.frame_rate,
        "sound_samples":sound.duration_samples(),"clips":editbay_core::timeline_clips(editor.project(),record)?,
        "sources":editor.project().assets.iter().filter(|asset| asset.kind == editbay_core::AssetKind::Media).collect::<Vec<_>>(),
        "client_acceptance":"pending independent review"}),
    )
}

/// Apply one atomic job-authoring group to the current revision.
/// `editor` owns the document; `commands` define the change. Returns validation.
fn apply(editor: &mut DocumentEditor, commands: &[DocumentCommand]) -> Result<()> {
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Author production job".into(),
        commands,
    )?;
    Ok(())
}
/// Prepare and apply a normal native timeline action.
/// `editor` and `action` define current ownership; returns the selected clip.
fn edit(editor: &mut DocumentEditor, action: TimelineAction) -> Result<Option<Uuid>> {
    let change = editbay_core::timeline_edit(editor.project(), &action)?;
    apply(editor, &change.commands)?;
    Ok(change.clip)
}
/// Find the authored source output from its typed sequence command.
/// `commands` contain one source sequence; returns its composition identity.
fn output(commands: &[DocumentCommand]) -> Result<Uuid> {
    commands
        .iter()
        .find_map(|command| match command {
            DocumentCommand::SetSequence { sequence } => sequence.composition,
            _ => None,
        })
        .ok_or_else(|| "Source sequence command is absent".into())
}
