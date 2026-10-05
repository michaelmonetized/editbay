use editbay_core::*;
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

fn source(ticks: Vec<i64>, end: i64) -> Project {
    let mut project = Project::new("Sequence authoring").unwrap();
    let asset = Uuid::new_v4();
    project.assets.push(AssetReference {
        id: asset,
        kind: AssetKind::Media,
        path: "Read only original.mkv".into(),
        sha256: "a".repeat(64),
        bytes: 1,
        provenance: "Typed fixture, not a decoded source".into(),
    });
    project.sources.push(MediaSource {
        id: Uuid::new_v4(),
        name: "Camera".into(),
        asset,
        metadata: BTreeMap::new(),
        streams: vec![SourceStream {
            index: 2,
            codec: "ffv1".into(),
            time_base: TimeBase {
                numerator: 1,
                denominator: 1000,
            },
            start_tick: ticks[0],
            duration_ticks: Some((end - ticks[0]) as u64),
            metadata: BTreeMap::new(),
            format: StreamFormat::Video {
                width: 720,
                height: 1080,
                sample_aspect: FrameRate::new(3, 2).unwrap(),
                timing: PictureTiming::Variable {
                    presentation_ticks: ticks,
                    end_tick: end,
                },
                color: SourceColor {
                    primaries: 1,
                    transfer: 1,
                    matrix: 1,
                    range: 1,
                },
                alpha: AlphaMode::Opaque,
            },
        }],
    });
    project
}

#[test]
fn exact_uniform_sequence_is_one_reversible_saved_and_recoverable_group() {
    let project = source(vec![100, 140, 180, 220], 260);
    let commands = sequence_from_video(
        &project,
        project.sources[0].id,
        2,
        FrameRate::new(24, 1).unwrap(),
    )
    .unwrap();
    let mut editor = DocumentEditor::new(project.clone()).unwrap();
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Create sequence".into(),
            &commands,
        )
        .unwrap();
    let authored = editor.snapshot();
    let composition = &authored.compositions[0];
    assert_eq!(
        (composition.width, composition.height, composition.duration),
        (1080, 1080, 4)
    );
    assert_eq!(composition.frame_rate, FrameRate::new(25, 1).unwrap());
    let evaluation = EvaluationSnapshot::new(authored.clone()).unwrap();
    for frame in 0..4 {
        let prepared = evaluation
            .prepare(
                composition.id,
                SourcePosition::new(frame, 1).unwrap(),
                false,
            )
            .unwrap();
        assert!(
            matches!(prepared.nodes[0].source, Some(SourceRequest::Media { picture: Some(ordinal), position: SourcePosition { numerator, denominator: 1 }, .. }) if ordinal == frame as u64 && numerator == 100 + frame * 40)
        );
    }
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert!(editor.project().compositions.is_empty());
    assert_eq!(editor.project().sources, project.sources);
    editor.redo(DocumentVersion::of(editor.project())).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("Sequence.editbay");
    save_new(editor.project(), &saved).unwrap();
    assert_eq!(load(&saved).unwrap(), *editor.project());
    let checkpoint =
        checkpoint(editor.project(), Some(&saved), dir.path().join("recovery")).unwrap();
    let recovered = recover_copy(&checkpoint, dir.path().join("Recovered.editbay")).unwrap();
    assert_eq!(recovered.compositions, editor.project().compositions);
    assert_eq!(recovered.sources, project.sources);
}

#[test]
fn vfr_sequence_keeps_natural_integer_boundaries_and_explicit_partial_tail() {
    let project = source(vec![100, 133, 190, 240], 255);
    let mut editor = DocumentEditor::new(project.clone()).unwrap();
    let commands = sequence_from_video(
        &project,
        project.sources[0].id,
        2,
        FrameRate::new(25, 1).unwrap(),
    )
    .unwrap();
    editor
        .apply(
            DocumentVersion::of(&project),
            "Create indexed sequence".into(),
            &commands,
        )
        .unwrap();
    let composition = &editor.project().compositions[0];
    assert_eq!(composition.duration, 4);
    let map = &composition.tracks[0].clips[0].time_map;
    assert_eq!(
        map.position(0).unwrap(),
        SourcePosition::new(100, 1).unwrap()
    );
    assert_eq!(
        map.position(1).unwrap(),
        SourcePosition::new(140, 1).unwrap()
    );
    assert_eq!(
        map.position(2).unwrap(),
        SourcePosition::new(180, 1).unwrap()
    );
    assert_eq!(
        map.position(3).unwrap(),
        SourcePosition::new(220, 1).unwrap()
    );
    assert_eq!(
        map.position(4).unwrap(),
        SourcePosition::new(255, 1).unwrap()
    );
    let evaluation = EvaluationSnapshot::new(Arc::new(editor.project().clone())).unwrap();
    for (frame, expected) in [0, 1, 1, 2].into_iter().enumerate() {
        let prepared = evaluation
            .prepare(
                composition.id,
                SourcePosition::new(frame as i64, 1).unwrap(),
                false,
            )
            .unwrap();
        assert!(
            matches!(prepared.nodes[0].source, Some(SourceRequest::Media { picture: Some(ordinal), .. }) if ordinal == expected)
        );
    }
    assert_eq!(editor.project().sources, project.sources);
    assert!(
        sequence_from_video(
            &project,
            project.sources[0].id,
            2,
            FrameRate::new(24, 1).unwrap()
        )
        .is_err()
    );
}

#[test]
fn invalid_or_absent_source_timing_is_rejected_before_authoring() {
    let mut project = source(vec![0, 40], 80);
    assert!(
        sequence_from_video(&project, Uuid::new_v4(), 2, FrameRate::new(25, 1).unwrap()).is_err()
    );
    project.sources[0].streams[0].time_base.denominator = 0;
    assert!(
        sequence_from_video(
            &project,
            project.sources[0].id,
            2,
            FrameRate::new(25, 1).unwrap()
        )
        .is_err()
    );
}
