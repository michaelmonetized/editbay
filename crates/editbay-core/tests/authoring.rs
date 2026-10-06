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

fn natural_fixture(
    rate: FrameRate,
    audio_rate: u32,
    video_start: i64,
    audio_start: i64,
    samples: u64,
) -> Project {
    let mut project = source(vec![0, 40, 80, 120], 160);
    let video = &mut project.sources[0].streams[0];
    video.time_base = TimeBase {
        numerator: 1,
        denominator: rate.numerator,
    };
    video.start_tick = video_start;
    video.duration_ticks = Some(u64::from(rate.denominator) * 4);
    if let StreamFormat::Video { timing, .. } = &mut video.format {
        *timing = PictureTiming::Constant { rate };
    }
    project.sources[0].streams.push(SourceStream {
        index: 3,
        codec: "pcm_f32le".into(),
        time_base: TimeBase {
            numerator: 1,
            denominator: audio_rate,
        },
        start_tick: audio_start,
        duration_ticks: Some(samples),
        metadata: BTreeMap::new(),
        format: StreamFormat::Audio {
            sample_rate: audio_rate,
            channels: vec!["FL".into(), "FR".into()],
        },
    });
    project
}

#[test]
fn natural_sound_keeps_every_exact_sample_center_at_fractional_picture_rates_and_origins() {
    for rate in [
        FrameRate::new(24, 1).unwrap(),
        FrameRate::new(25, 1).unwrap(),
        FrameRate::new(30, 1).unwrap(),
        FrameRate::new(30000, 1001).unwrap(),
        FrameRate::new(24000, 1001).unwrap(),
    ] {
        for audio_rate in [44100, 48000] {
            for (video_start, audio_start, samples) in [
                (0, 0, 7007),
                (0, 701, 7007),
                (i64::from(rate.numerator), audio_rate as i64 - 701, 9007),
                (i64::from(rate.denominator) * 3, 0, 32003),
            ] {
                let project = natural_fixture(rate, audio_rate, video_start, audio_start, samples);
                let mut editor = DocumentEditor::new(project.clone()).unwrap();
                let commands =
                    sequence_from_video_with_audio(&project, project.sources[0].id, 2, 3, rate)
                        .unwrap();
                editor
                    .apply(
                        DocumentVersion::of(&project),
                        "Natural sound".into(),
                        &commands,
                    )
                    .unwrap();
                assert_eq!(editor.project().sources, project.sources);
                let root = editor.project().sequences[0].composition.unwrap();
                let composition = editor
                    .project()
                    .compositions
                    .iter()
                    .find(|c| c.id == root)
                    .unwrap();
                assert_eq!(
                    composition.tracks[0].clips[0].linked,
                    Some(composition.tracks[1].clips[0].id)
                );
                assert_eq!(
                    composition.tracks[1].clips[0].linked,
                    Some(composition.tracks[0].clips[0].id)
                );
                let sound = SoundSnapshot::new(
                    Arc::new(EvaluationSnapshot::new(editor.snapshot()).unwrap()),
                    root,
                    SoundProfile {
                        sample_rate: audio_rate,
                        channels: vec!["FL".into(), "FR".into()],
                    },
                    SoundBudget::default(),
                )
                .unwrap();
                for first in (0..sound.duration_samples()).step_by(4096) {
                    let count = (sound.duration_samples() - first).min(4096) as u32;
                    let plan = sound.prepare(first, count).unwrap();
                    assert_eq!(plan.sources().len(), 1);
                    for (offset, actual) in plan.sources()[0].samples.iter().enumerate() {
                        let n = first + offset as u64;
                        let expected = SourcePosition::new(
                            video_start * i64::from(audio_rate) * 2
                                + (2 * n as i64 + 1) * i64::from(rate.numerator),
                            u64::from(rate.numerator) * 2,
                        )
                        .unwrap();
                        let active = expected.compare_tick(audio_start).unwrap().is_ge()
                            && expected
                                .compare_tick(audio_start + samples as i64)
                                .unwrap()
                                .is_lt();
                        assert_eq!(actual.is_some(), active, "{rate:?}/{audio_rate} at {n}");
                        if let Some(actual) = actual {
                            assert_eq!(actual.center, expected, "{rate:?}/{audio_rate} at {n}");
                            assert_eq!(actual.gain, 1.);
                            assert!(!actual.reverse);
                            assert!((actual.step - 1.).abs() < 1e-12);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn linked_natural_sound_survives_one_undo_group_save_checkpoint_and_independent_recovery() {
    let rate = FrameRate::new(30000, 1001).unwrap();
    let project = natural_fixture(rate, 48000, 0, 801, 7007);
    let commands =
        sequence_from_video_with_audio(&project, project.sources[0].id, 2, 3, rate).unwrap();
    let mut editor = DocumentEditor::new(project.clone()).unwrap();
    editor
        .apply(
            DocumentVersion::of(&project),
            "Picture and original sound".into(),
            &commands,
        )
        .unwrap();
    let compositions = editor.project().compositions.clone();
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert!(editor.project().compositions.is_empty());
    assert_eq!(editor.project().sources, project.sources);
    editor.redo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().compositions, compositions);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Sound sequence.editbay");
    save_new(editor.project(), &path).unwrap();
    let saved = std::fs::read(&path).unwrap();
    let cp = checkpoint(editor.project(), Some(&path), dir.path().join("Recovery")).unwrap();
    let recovered = recover_copy(&cp, dir.path().join("Recovered sound.editbay")).unwrap();
    assert_ne!(recovered.id, editor.project().id);
    assert_eq!(recovered.compositions, compositions);
    assert_eq!(recovered.sources, project.sources);
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    assert_eq!(load(&path).unwrap().compositions, compositions);
}

#[test]
fn natural_sound_refuses_wrong_streams_absent_overlap_and_clock_overflow_before_mutation() {
    let rate = FrameRate::new(30000, 1001).unwrap();
    let project = natural_fixture(rate, 48000, 0, 0, 7007);
    let bytes = serde_json::to_vec(&project).unwrap();
    assert!(sequence_from_video_with_audio(&project, project.sources[0].id, 2, 2, rate).is_err());
    assert!(sequence_from_video_with_audio(&project, project.sources[0].id, 2, 99, rate).is_err());
    assert_eq!(serde_json::to_vec(&project).unwrap(), bytes);
    let before = natural_fixture(rate, 48000, 60000, 0, 7007);
    assert!(sequence_from_video_with_audio(&before, before.sources[0].id, 2, 3, rate).is_err());
    let unusual = natural_fixture(FrameRate::new(100003, 1).unwrap(), 48000, 0, 0, 7007);
    assert!(sequence_from_video_with_audio(&unusual, unusual.sources[0].id, 2, 3, rate).is_err());
}
