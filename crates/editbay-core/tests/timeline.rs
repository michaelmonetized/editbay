use editbay_core::*;
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}
fn range(start: u64, end: u64) -> FrameRange {
    FrameRange { start, end }
}
fn selection(start: u64, end: u64) -> SourceSelection {
    SourceSelection {
        composition: id(30),
        range: range(start, end),
    }
}
fn project() -> Project {
    let mut p = Project::new("Timeline math").unwrap();
    p.assets.push(AssetReference {
        id: id(10),
        kind: AssetKind::Media,
        path: "math-only.wav".into(),
        sha256: "a".repeat(64),
        bytes: 100,
        provenance: "Mathematical fixture, no decoded-media claim".into(),
    });
    p.sources.push(MediaSource {
        id: id(20),
        name: "Original sound".into(),
        asset: id(10),
        metadata: BTreeMap::new(),
        streams: vec![SourceStream {
            index: 0,
            codec: "pcm_f32le".into(),
            time_base: TimeBase {
                numerator: 1,
                denominator: 48000,
            },
            start_tick: 0,
            duration_ticks: Some(96000),
            format: StreamFormat::Audio {
                sample_rate: 48000,
                channels: vec!["FL".into(), "FR".into()],
            },
            metadata: BTreeMap::new(),
        }],
    });
    p.compositions.push(Composition {
        id: id(30),
        name: "Source".into(),
        width: 16,
        height: 16,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 48,
        tracks: vec![Track {
            id: id(40),
            name: "Audio".into(),
            kind: TrackKind::Audio,
            enabled: true,
            clips: vec![Clip {
                id: id(41),
                name: "Sound".into(),
                range: range(0, 48),
                source: ClipSource::Media {
                    source: id(20),
                    stream: 0,
                },
                time_map: TimeMap {
                    source_denominator: 1,
                    points: vec![
                        TimePoint {
                            frame: 0,
                            source_tick: 0,
                        },
                        TimePoint {
                            frame: 48,
                            source_tick: 96000,
                        },
                    ],
                },
                linked: None,
            }],
        }],
        nodes: vec![
            TimedNode {
                id: id(50),
                range: range(0, 48),
                operation: NodeOperation::Source { clip: id(41) },
                animation: vec![],
            },
            TimedNode {
                id: id(51),
                range: range(0, 48),
                operation: NodeOperation::Solid {
                    rgba: [0.2, 0.3, 0.4, 1.],
                },
                animation: vec![],
            },
        ],
        picture: Some(id(51)),
        audio: Some(id(50)),
    });
    p.validate().unwrap();
    p
}
fn apply(editor: &mut DocumentEditor, action: TimelineAction) -> TimelineChange {
    let change = timeline_edit(editor.project(), &action).unwrap();
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Linked edit".into(),
            &change.commands,
        )
        .unwrap();
    change
}
fn assembly() -> (DocumentEditor, Uuid, Uuid) {
    let mut editor = DocumentEditor::new(project()).unwrap();
    let change = apply(
        &mut editor,
        TimelineAction::Create {
            name: "Cut".into(),
            source: selection(4, 16),
        },
    );
    (editor, change.composition, change.clip.unwrap())
}
fn centers(
    project: Arc<Project>,
    composition: Uuid,
    first: u64,
    count: u32,
) -> Vec<SourcePosition> {
    let snapshot = SoundSnapshot::at_output_rate(
        Arc::new(EvaluationSnapshot::new(project).unwrap()),
        composition,
        48000,
        SoundBudget::default(),
    )
    .unwrap();
    let block = snapshot.prepare(first, count).unwrap();
    (0..count as usize)
        .map(|index| {
            let active: Vec<_> = block
                .sources()
                .iter()
                .filter_map(|source| source.samples[index].map(|s| s.center))
                .collect();
            assert_eq!(active.len(), 1);
            active[0]
        })
        .collect()
}

#[test]
fn insert_inside_linked_clip_preserves_exact_sample_centers_across_both_cuts() {
    let (mut editor, composition, left) = assembly();
    let added = apply(
        &mut editor,
        TimelineAction::Insert {
            composition,
            at: 5,
            source: selection(24, 27),
        },
    );
    let clips = timeline_clips(editor.project(), composition).unwrap();
    assert_eq!(clips.len(), 3);
    assert_eq!(clips[0].id, left);
    assert_eq!(clips[1].id, added.clip.unwrap());
    assert_eq!(
        clips
            .iter()
            .map(|c| (c.range, c.source.range))
            .collect::<Vec<_>>(),
        vec![
            (range(0, 5), range(4, 9)),
            (range(5, 8), range(24, 27)),
            (range(8, 15), range(9, 16))
        ]
    );
    for (first, expected) in [(9999, vec![35999, 96001]), (15999, vec![107999, 36001])] {
        assert_eq!(
            centers(editor.snapshot(), composition, first, 2),
            expected
                .into_iter()
                .map(|n| SourcePosition::new(n, 2).unwrap())
                .collect::<Vec<_>>()
        );
    }
    for clip in clips {
        assert!(clip.linked.is_some());
    }
}

#[test]
fn ripple_trim_remove_and_move_preserve_linked_identity_and_source_ranges() {
    let (mut editor, composition, left) = assembly();
    let right = apply(
        &mut editor,
        TimelineAction::Split {
            composition,
            clip: left,
            at: 6,
        },
    )
    .clip
    .unwrap();
    apply(
        &mut editor,
        TimelineAction::Trim {
            composition,
            clip: left,
            source_range: range(5, 8),
            ripple: true,
        },
    );
    let clips = timeline_clips(editor.project(), composition).unwrap();
    assert_eq!(clips[0].range, range(0, 3));
    assert_eq!(clips[1].range, range(3, 9));
    assert_eq!(clips[1].source.range, range(10, 16));
    apply(
        &mut editor,
        TimelineAction::Remove {
            composition,
            clip: left,
            ripple: true,
        },
    );
    apply(
        &mut editor,
        TimelineAction::Move {
            composition,
            clip: right,
            at: 7,
        },
    );
    let clips = timeline_clips(editor.project(), composition).unwrap();
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].range, range(7, 13));
    assert_eq!(clips[0].id, right);
    assert_eq!(
        centers(editor.snapshot(), composition, 14000, 1),
        vec![SourcePosition::new(40001, 2).unwrap()]
    );
}

#[test]
fn undo_redo_save_recovery_preserve_groups_and_stale_commands_fail() {
    let (mut editor, composition, clip) = assembly();
    let original = editor.project().clone();
    let change = timeline_edit(
        editor.project(),
        &TimelineAction::Split {
            composition,
            clip,
            at: 5,
        },
    )
    .unwrap();
    let before = DocumentVersion::of(editor.project());
    editor
        .apply(before, "Split".into(), &change.commands)
        .unwrap();
    let cut = editor.snapshot();
    assert!(
        editor
            .apply(before, "Stale".into(), &change.commands)
            .is_err()
    );
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().compositions, original.compositions);
    editor.redo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().compositions, cut.compositions);
    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("Cut.editbay");
    save_new(editor.project(), &saved).unwrap();
    assert_eq!(load(&saved).unwrap(), *editor.project());
    let checkpoint =
        checkpoint(editor.project(), Some(&saved), dir.path().join("Recovery")).unwrap();
    let recovered = recover_copy(checkpoint, dir.path().join("Recovered.editbay")).unwrap();
    assert_eq!(recovered.compositions, cut.compositions);
    assert_eq!(recovered.assets, original.assets);
    assert_eq!(
        centers(Arc::new(recovered), composition, 9999, 2),
        centers(cut, composition, 9999, 2)
    );
}

#[test]
fn overlap_overflow_invalid_ranges_and_recursive_insert_leave_document_untouched() {
    let (mut editor, composition, clip) = assembly();
    let right = apply(
        &mut editor,
        TimelineAction::Split {
            composition,
            clip,
            at: 6,
        },
    )
    .clip
    .unwrap();
    let original = editor.project().clone();
    let actions = [
        TimelineAction::Move {
            composition,
            clip: right,
            at: 5,
        },
        TimelineAction::Move {
            composition,
            clip: right,
            at: i64::MAX as u64,
        },
        TimelineAction::Trim {
            composition,
            clip,
            source_range: range(0, 49),
            ripple: true,
        },
        TimelineAction::Trim {
            composition,
            clip,
            source_range: range(7, 7),
            ripple: true,
        },
        TimelineAction::Trim {
            composition,
            clip,
            source_range: range(0, 20),
            ripple: false,
        },
        TimelineAction::Split {
            composition,
            clip,
            at: 0,
        },
        TimelineAction::Split {
            composition,
            clip,
            at: 6,
        },
        TimelineAction::Insert {
            composition,
            at: 13,
            source: selection(0, 2),
        },
        TimelineAction::Insert {
            composition,
            at: 0,
            source: SourceSelection {
                composition,
                range: range(0, 1),
            },
        },
    ];
    for action in actions {
        assert!(
            timeline_edit(editor.project(), &action).is_err(),
            "{action:?}"
        );
        assert_eq!(editor.project(), &original);
    }
}

#[test]
fn unrecognized_graph_and_implicit_profile_conversion_are_rejected() {
    let (mut editor, composition, clip) = assembly();
    let mut altered = editor.project().clone();
    let assembly = altered
        .compositions
        .iter_mut()
        .find(|c| c.id == composition)
        .unwrap();
    let root = assembly
        .nodes
        .iter_mut()
        .find(|n| matches!(n.operation, NodeOperation::Solid { .. }))
        .unwrap();
    root.operation = NodeOperation::Solid {
        rgba: [1., 0., 0., 1.],
    };
    assert!(timeline_clips(&altered, composition).is_err());
    let mut other = editor.project().compositions[0].clone();
    other.id = Uuid::new_v4();
    other.width = 32;
    other.tracks.clear();
    other.audio = None;
    other.nodes = vec![TimedNode {
        id: Uuid::new_v4(),
        range: range(0, 48),
        operation: NodeOperation::Solid { rgba: [0.; 4] },
        animation: vec![],
    }];
    other.picture = Some(other.nodes[0].id);
    let foreign = other.id;
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Other profile".into(),
            &[DocumentCommand::SetComposition { composition: other }],
        )
        .unwrap();
    assert!(
        timeline_edit(
            editor.project(),
            &TimelineAction::Insert {
                composition,
                at: 0,
                source: SourceSelection {
                    composition: foreign,
                    range: range(0, 2)
                }
            }
        )
        .is_err()
    );
    apply(
        &mut editor,
        TimelineAction::Remove {
            composition,
            clip,
            ripple: true,
        },
    );
    assert!(
        timeline_clips(editor.project(), composition)
            .unwrap()
            .is_empty()
    );
    apply(
        &mut editor,
        TimelineAction::Insert {
            composition,
            at: 0,
            source: selection(1, 3),
        },
    );
    assert_eq!(
        timeline_clips(editor.project(), composition).unwrap()[0].range,
        range(0, 2)
    );
}

#[test]
fn fractional_output_sample_centers_and_silent_gaps_keep_exact_time() {
    let mut source = project();
    source.compositions[0].frame_rate = FrameRate::new(24000, 1001).unwrap();
    let mut editor = DocumentEditor::new(source).unwrap();
    let created = apply(
        &mut editor,
        TimelineAction::Create {
            name: "Fractional cut".into(),
            source: selection(3, 15),
        },
    );
    let composition = created.composition;
    let clip = created.clip.unwrap();
    apply(
        &mut editor,
        TimelineAction::Move {
            composition,
            clip,
            at: 1,
        },
    );
    let sound = SoundSnapshot::at_output_rate(
        Arc::new(EvaluationSnapshot::new(editor.snapshot()).unwrap()),
        composition,
        44100,
        SoundBudget::default(),
    )
    .unwrap();
    let gap = sound.prepare(0, 1839).unwrap();
    assert!(
        gap.sources()
            .iter()
            .all(|source| source.samples.iter().all(Option::is_none))
    );
    let cut = sound.prepare(1839, 2).unwrap();
    for (offset, sample) in cut.sources()[0].samples.iter().enumerate() {
        let index = 1839 + offset as i64;
        let expected = SourcePosition::new(
            4000 * 44100 * 1001 + (2 * index + 1) * 24000 * 1000,
            44100 * 1001,
        )
        .unwrap();
        assert_eq!(sample.unwrap().center, expected);
    }
}

#[test]
fn conformance_preserves_elapsed_sound_time_and_pads_the_tail() {
    let p = project();
    let commands = conform_sequence(
        &p,
        id(30),
        20,
        30,
        FrameRate::new(30000, 1001).unwrap(),
        "Portrait".into(),
    )
    .unwrap();
    let mut editor = DocumentEditor::new(p.clone()).unwrap();
    editor
        .apply(DocumentVersion::of(&p), "Conform".into(), &commands)
        .unwrap();
    let conformed = editor
        .project()
        .sequences
        .last()
        .unwrap()
        .composition
        .unwrap();
    let scene = editor
        .project()
        .compositions
        .iter()
        .find(|c| c.id == conformed)
        .unwrap();
    assert_eq!(scene.duration, 60);
    assert_eq!((scene.width, scene.height), (20, 30));
    let map = &scene.tracks[0].clips[0].time_map;
    assert_eq!(
        map.position_at(SourcePosition::new(1, 1).unwrap()).unwrap(),
        SourcePosition::new(1001, 1250).unwrap()
    );
    let snapshot = SoundSnapshot::at_output_rate(
        Arc::new(EvaluationSnapshot::new(editor.snapshot()).unwrap()),
        conformed,
        48000,
        SoundBudget::default(),
    )
    .unwrap();
    let block = snapshot.prepare(95990, 96).unwrap();
    let source = block.sources().first().unwrap();
    for (offset, sample) in source.samples.iter().enumerate() {
        if let Some(sample) = sample {
            assert_eq!(
                sample.center,
                SourcePosition::new(191981 + 2 * offset as i64, 2).unwrap()
            );
            assert!(offset < 10);
        } else {
            assert!(offset >= 10);
        }
    }
    assert_eq!(snapshot.duration_samples(), 96096);
    assert_eq!(block.sources()[0].samples.iter().flatten().count(), 10);
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().compositions, p.compositions);
}

#[test]
fn precision_and_layer_edits_keep_outer_boundaries_and_controls() {
    let (mut editor, record, first) = assembly();
    apply(
        &mut editor,
        TimelineAction::Insert {
            composition: record,
            at: 12,
            source: selection(16, 28),
        },
    );
    apply(
        &mut editor,
        TimelineAction::Insert {
            composition: record,
            at: 24,
            source: selection(28, 40),
        },
    );
    apply(
        &mut editor,
        TimelineAction::Roll {
            composition: record,
            clip: first,
            at: 14,
        },
    );
    let clips = timeline_clips(editor.project(), record).unwrap();
    let middle = clips[1].id;
    apply(
        &mut editor,
        TimelineAction::Slide {
            composition: record,
            clip: middle,
            at: 15,
        },
    );
    apply(
        &mut editor,
        TimelineAction::Slip {
            composition: record,
            clip: middle,
            frames: -2,
        },
    );
    let controls = TimelineControls {
        translation: [3., -2.],
        scale: [1.2, 1.2],
        gain: 0.4,
        opacity: 0.7,
        ..Default::default()
    };
    apply(
        &mut editor,
        TimelineAction::SetControls {
            composition: record,
            clip: middle,
            controls,
        },
    );
    let before = timeline_clips(editor.project(), record).unwrap();
    assert_eq!(before[1].controls, controls);
    assert_eq!(before[0].range.start, 0);
    assert_eq!(before[2].range.end, 36);
    apply(
        &mut editor,
        TimelineAction::Split {
            composition: record,
            clip: middle,
            at: 20,
        },
    );
    let split = timeline_clips(editor.project(), record).unwrap();
    assert_eq!(split[1].controls, controls);
    assert_eq!(split[2].controls, controls);
    apply(
        &mut editor,
        TimelineAction::AddTrack {
            composition: record,
            name: "Overlay".into(),
        },
    );
    let scene = editor
        .project()
        .compositions
        .iter()
        .find(|c| c.id == record)
        .unwrap();
    let overlay = scene.tracks[2].id;
    let sound = scene.tracks[3].id;
    apply(
        &mut editor,
        TimelineAction::Place {
            composition: record,
            track: overlay,
            at: 5,
            source: selection(0, 8),
            audio_only: false,
        },
    );
    apply(
        &mut editor,
        TimelineAction::Place {
            composition: record,
            track: sound,
            at: 15,
            source: selection(0, 8),
            audio_only: true,
        },
    );
    let final_clips = timeline_clips(editor.project(), record).unwrap();
    assert!(final_clips.iter().any(|c| c.track == sound && c.audio_only));
    assert_eq!(final_clips.len(), 6);
    let version = DocumentVersion::of(editor.project());
    assert!(
        timeline_edit(
            editor.project(),
            &TimelineAction::Place {
                composition: record,
                track: overlay,
                at: 6,
                source: selection(0, 8),
                audio_only: false
            }
        )
        .is_err()
    );
    assert_eq!(DocumentVersion::of(editor.project()), version);
}
