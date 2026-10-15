use editbay_core::{multicam::*, *};
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn position(numerator: i64, denominator: u64) -> SourcePosition {
    SourcePosition::new(numerator, denominator).unwrap()
}

fn camera(project: &mut Project, base: u128, rate: FrameRate, duration: u64) {
    let samples = duration * 48_000 * u64::from(rate.denominator) / u64::from(rate.numerator);
    project.assets.push(AssetReference {
        id: id(base + 1),
        kind: AssetKind::Media,
        path: format!("camera-{base}-math-only.wav").into(),
        sha256: "a".repeat(64),
        bytes: 100,
        provenance: "Pure document fixture; no decoded or physical-media claim".into(),
    });
    project.sources.push(MediaSource {
        id: id(base + 2),
        name: format!("Camera {base} sound"),
        asset: id(base + 1),
        streams: vec![SourceStream {
            index: 0,
            codec: "pcm_f32le".into(),
            time_base: TimeBase {
                numerator: 1,
                denominator: 48_000,
            },
            start_tick: 0,
            duration_ticks: Some(samples),
            format: StreamFormat::Audio {
                sample_rate: 48_000,
                channels: vec!["FL".into(), "FR".into()],
            },
            metadata: BTreeMap::new(),
        }],
        metadata: BTreeMap::new(),
    });
    let range = FrameRange {
        start: 0,
        end: duration,
    };
    project.compositions.push(Composition {
        id: id(base),
        name: format!("Camera {base}"),
        width: 16,
        height: 16,
        frame_rate: rate,
        duration,
        tracks: vec![Track {
            id: id(base + 4),
            name: "Original sound".into(),
            kind: TrackKind::Audio,
            enabled: true,
            clips: vec![Clip {
                id: id(base + 3),
                name: "Unmodified camera recording".into(),
                range,
                source: ClipSource::Media {
                    source: id(base + 2),
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
                            frame: duration,
                            source_tick: samples as i64,
                        },
                    ],
                },
                linked: None,
            }],
        }],
        nodes: vec![
            TimedNode {
                id: id(base + 5),
                range,
                operation: NodeOperation::Source { clip: id(base + 3) },
                animation: vec![],
            },
            TimedNode {
                id: id(base + 6),
                range,
                operation: NodeOperation::Solid {
                    rgba: if base == 100 {
                        [1.0, 0.0, 0.0, 1.0]
                    } else {
                        [0.0, 1.0, 0.0, 1.0]
                    },
                },
                animation: vec![],
            },
        ],
        picture: Some(id(base + 6)),
        audio: Some(id(base + 5)),
    });
}

fn project() -> Project {
    let mut project = Project::new("Multicam math").unwrap();
    camera(&mut project, 100, FrameRate::new(24, 1).unwrap(), 240);
    camera(&mut project, 200, FrameRate::new(24, 1).unwrap(), 240);
    project.sequences[0].width = 16;
    project.sequences[0].height = 16;
    project.sequences[0].composition = Some(id(100));
    project.validate().unwrap();
    project
}

fn session(project: &Project) -> MulticamSession {
    MulticamSession::new(project, "Two-camera interview", &[id(100), id(200)]).unwrap()
}

#[test]
fn incompatible_sound_policy_is_rejected_atomically_before_a_cut_is_published() {
    let mut p = project();
    let StreamFormat::Audio { channels, .. } = &mut p.sources[1].streams[0].format else {
        unreachable!()
    };
    *channels = vec!["FC".into()];
    let mut session = session(&p);
    assert_eq!(
        session.audio_policy,
        MulticamAudioPolicy::Fixed {
            angle: session.angles[0].id
        }
    );
    session.audio_policy = MulticamAudioPolicy::FollowVideo;
    session.cut_at(&p, 24, session.angles[1].id).unwrap();
    let mut editor = DocumentEditor::new(p.clone()).unwrap();
    let version = DocumentVersion::of(editor.project());
    assert!(
        editor
            .apply(
                version,
                "Invalid follow audio".into(),
                &[DocumentCommand::SetMulticamSession {
                    session: session.clone()
                }]
            )
            .unwrap_err()
            .to_string()
            .contains("fixed audio camera")
    );
    assert_eq!(editor.project(), &p);
    assert_eq!(editor.history_usage(), (0, 0));
    session.audio_policy = MulticamAudioPolicy::Fixed {
        angle: session.angles[0].id,
    };
    editor
        .apply(
            version,
            "Fixed compatible audio".into(),
            &[DocumentCommand::SetMulticamSession { session }],
        )
        .unwrap();
}

fn compile_project(mut project: Project, session: &MulticamSession) -> Project {
    let output = compile(&project, session).unwrap();
    project.sequences[0].composition = Some(output.id);
    project.compositions.push(output);
    project.validate().unwrap();
    project
}

fn active_picture(project: &Project, output: Uuid, frame: u64) -> (Uuid, SourcePosition) {
    let plan = project.frame_plan(output, frame).unwrap();
    let active: Vec<_> = plan
        .nodes
        .iter()
        .filter(|node| node.active && node.socket == SocketType::Image)
        .filter_map(|node| match &node.source {
            Some(SourceRequest::Composition {
                composition,
                position,
                ..
            }) => Some((*composition, *position)),
            _ => None,
        })
        .collect();
    assert_eq!(active.len(), 1);
    active[0]
}

fn sound(project: Project, output: Uuid) -> SoundSnapshot {
    SoundSnapshot::new(
        Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap()),
        output,
        SoundProfile {
            sample_rate: 48_000,
            channels: vec!["FL".into(), "FR".into()],
        },
        SoundBudget::default(),
    )
    .unwrap()
}

fn active_sound(plan: &SoundBlockPlan, sample: usize) -> Vec<Uuid> {
    let mut sources: Vec<_> = plan
        .sources()
        .iter()
        .filter(|source| source.samples[sample].is_some())
        .map(|source| source.source)
        .collect();
    sources.sort();
    sources
}

fn noise(length: usize) -> Vec<f32> {
    let mut state = 7_u32;
    (0..length)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((state >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0
        })
        .collect()
}

fn options(rate: u32, max_offset_samples: u32) -> AudioSyncOptions {
    AudioSyncOptions {
        sample_rate: rate,
        max_offset_samples,
        min_overlap_samples: 64,
        min_correlation: 0.9,
        min_separation: 0.1,
    }
}

#[test]
fn cuts_and_switches_have_distinct_boundaries_and_stable_clip_identity() {
    let project = project();
    let original = project.clone();
    let mut session = session(&project);
    let first = session.angles[0].id;
    let second = session.angles[1].id;
    session.cut_at(&project, 48, second).unwrap();
    session.cut_at(&project, 96, first).unwrap();
    let before = compile(&project, &session).unwrap();
    assert_eq!(before.id, session.composition);
    assert_eq!(before.tracks[0].clips.len(), 3);
    session.switch_at(&project, 60, first).unwrap();
    assert_eq!(
        session.cuts.iter().map(|cut| cut.frame).collect::<Vec<_>>(),
        [0, 48, 96]
    );
    assert_eq!(session.cuts[1].angle, first);
    let switched = compile(&project, &session).unwrap();
    assert_eq!(
        before
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .map(|clip| clip.id)
            .collect::<Vec<_>>(),
        switched
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .map(|clip| clip.id)
            .collect::<Vec<_>>()
    );
    session.cut_at(&project, 60, second).unwrap();
    assert_eq!(
        session.cuts.iter().map(|cut| cut.frame).collect::<Vec<_>>(),
        [0, 48, 60, 96]
    );
    let compiled = compile_project(project.clone(), &session);
    assert_eq!(
        active_picture(&compiled, session.composition, 59).0,
        id(100)
    );
    assert_eq!(
        active_picture(&compiled, session.composition, 60).0,
        id(200)
    );
    assert_eq!(
        active_picture(&compiled, session.composition, 96).0,
        id(100)
    );
    assert_eq!(project, original);
}

#[test]
fn marker_sync_between_different_rates_keeps_exact_source_event_and_overlap() {
    let mut project = Project::new("Different camera clocks").unwrap();
    camera(&mut project, 100, FrameRate::new(24, 1).unwrap(), 240);
    camera(&mut project, 200, FrameRate::new(30, 1).unwrap(), 300);
    project.sequences[0].width = 16;
    project.sequences[0].height = 16;
    let mut session = session(&project);
    let first = session.angles[0].id;
    let second = session.angles[1].id;
    session
        .sync_markers(
            &project,
            &[(first, position(48, 1)), (second, position(90, 1))],
        )
        .unwrap();
    assert_eq!(session.angles[0].offset, position(0, 1));
    assert_eq!(session.angles[1].offset, position(-24, 1));
    assert_eq!(
        session.overlap(&project).unwrap(),
        MulticamOverlap {
            origin: 0,
            duration: 216
        }
    );
    session.cut_at(&project, 48, second).unwrap();
    let output = compile_project(project, &session);
    assert_eq!(
        active_picture(&output, session.composition, 47),
        (id(100), position(47, 1))
    );
    assert_eq!(
        active_picture(&output, session.composition, 48),
        (id(200), position(90, 1))
    );
    assert_eq!(
        active_picture(&output, session.composition, 49),
        (id(200), position(365, 4))
    );
}

#[test]
fn timecodes_align_camera_starts_and_validate_ntsc_drop_frame_labels() {
    let project = project();
    let mut session = session(&project);
    let rate = FrameRate::new(24, 1).unwrap();
    let first = session.angles[0].id;
    let second = session.angles[1].id;
    session
        .sync_timecodes(
            &project,
            &[
                (first, MulticamTimecode::parse("01:00:00:00", rate).unwrap()),
                (
                    second,
                    MulticamTimecode::parse("01:00:02:00", rate).unwrap(),
                ),
            ],
        )
        .unwrap();
    assert_eq!(session.angles[1].offset, position(48, 1));
    assert_eq!(
        session.overlap(&project).unwrap(),
        MulticamOverlap {
            origin: 48,
            duration: 192
        }
    );
    session.cut_at(&project, 24, second).unwrap();
    let output = compile_project(project, &session);
    assert_eq!(
        active_picture(&output, session.composition, 0),
        (id(100), position(48, 1))
    );
    assert_eq!(
        active_picture(&output, session.composition, 24),
        (id(200), position(24, 1))
    );
    let ntsc = FrameRate::new(30_000, 1001).unwrap();
    assert_eq!(
        MulticamTimecode::parse("00:01:00;02", ntsc).unwrap().frames,
        position(1800, 1)
    );
    assert_eq!(
        MulticamTimecode::parse("01:00:00;00", ntsc).unwrap().frames,
        position(107_892, 1)
    );
    assert!(MulticamTimecode::parse("00:01:00;00", ntsc).is_err());
    assert!(MulticamTimecode::parse("00:01:00;01", ntsc).is_err());
    assert_eq!(
        MulticamTimecode::parse("00:10:00;00", ntsc).unwrap().frames,
        position(17_982, 1)
    );
    let ntsc60 = FrameRate::new(60_000, 1001).unwrap();
    assert_eq!(
        MulticamTimecode::parse("00:01:00;04", ntsc60)
            .unwrap()
            .frames,
        position(3600, 1)
    );
    assert!(MulticamTimecode::parse("00:01:00;03", ntsc60).is_err());
    for input in [
        "24:00:00:00",
        "00:60:00:00",
        "00:00:00:24",
        "00;00:00:00",
        "00:00:00",
        "bad",
    ] {
        assert!(
            MulticamTimecode::parse(input, rate).is_err(),
            "accepted {input}"
        );
    }
    assert!(MulticamTimecode::parse("00:01:00;02", rate).is_err());
}

#[test]
fn audio_correlation_finds_independent_known_lags_despite_gain_and_dc_offset() {
    let reference = noise(512);
    let mut candidate = vec![0.25; 19];
    candidate.extend(reference.iter().map(|sample| sample * 0.4 + 0.25));
    let positive = correlate_audio(&reference, &candidate, options(200, 32)).unwrap();
    assert_eq!(positive.lag_samples(), 19);
    assert_eq!(positive.sample_rate(), 200);
    assert_eq!(positive.overlap_samples(), 512);
    assert!(positive.correlation() > 0.999_999);
    assert!(positive.separation() > 0.5);
    let negative = correlate_audio(&reference, &reference[13..], options(200, 32)).unwrap();
    assert_eq!(negative.lag_samples(), -13);
    assert!(negative.correlation() > 0.999_999);
}

#[test]
fn audio_sync_rejects_silence_ambiguity_nonfinite_and_excessive_search() {
    let signal = noise(512);
    assert!(correlate_audio(&[0.0; 512], &[0.0; 512], options(200, 32)).is_err());
    let periodic: Vec<_> = (0..512)
        .map(|index| if index % 2 == 0 { -1.0 } else { 1.0 })
        .collect();
    assert!(correlate_audio(&periodic, &periodic, options(200, 4)).is_err());
    let mut nonfinite = signal.clone();
    nonfinite[10] = f32::NAN;
    assert!(correlate_audio(&signal, &nonfinite, options(200, 32)).is_err());
    assert!(correlate_audio(&signal, &signal, options(0, 32)).is_err());
    assert!(correlate_audio(&signal[..10], &signal, options(200, 2)).is_err());
    assert!(correlate_audio(&signal, &signal, options(200, 512)).is_err());
    let large = noise(20_000);
    assert!(correlate_audio(&large, &large, options(200, 2000)).is_err());
    let mut strict = options(200, 32);
    strict.min_correlation = f64::NAN;
    assert!(correlate_audio(&signal, &signal, strict).is_err());
}

#[test]
fn fractional_audio_sync_survives_time_maps_picture_and_sound_evaluation() {
    let project = project();
    let mut session = session(&project);
    let first = session.angles[0].id;
    let second = session.angles[1].id;
    assert_eq!(
        session.audio_policy,
        MulticamAudioPolicy::Fixed { angle: first }
    );
    session.audio_policy = MulticamAudioPolicy::FollowVideo;
    let reference = noise(512);
    let mut candidate = vec![0.0; 3];
    candidate.extend(&reference);
    let receipt = correlate_audio(&reference, &candidate, options(200, 8)).unwrap();
    session
        .sync_audio_offset(&project, first, second, receipt)
        .unwrap();
    assert_eq!(session.angles[1].offset, position(-9, 25));
    assert_eq!(
        session.overlap(&project).unwrap(),
        MulticamOverlap {
            origin: 0,
            duration: 239
        }
    );
    session.switch_at(&project, 0, second).unwrap();
    let output = compile_project(project, &session);
    assert_eq!(
        active_picture(&output, session.composition, 0),
        (id(200), position(9, 25))
    );
    let scene = output
        .compositions
        .iter()
        .find(|scene| scene.id == session.composition)
        .unwrap();
    assert_eq!(scene.tracks[0].clips[0].time_map.source_denominator, 25);
    assert_eq!(scene.tracks[0].clips[0].time_map.points[0].source_tick, 9);
    let sound = sound(output, session.composition);
    let block = sound.prepare(0, 1).unwrap();
    assert_eq!(active_sound(&block, 0), [id(202)]);
    assert_eq!(
        block.sources()[0].samples[0].as_ref().unwrap().center,
        position(1441, 2)
    );
}

#[test]
fn linked_audio_follows_fixed_or_all_policy_at_the_exact_cut_sample() {
    for policy in [0, 1, 2] {
        let project = project();
        let mut session = session(&project);
        let first = session.angles[0].id;
        let second = session.angles[1].id;
        session.audio_policy = match policy {
            0 => MulticamAudioPolicy::FollowVideo,
            1 => MulticamAudioPolicy::Fixed { angle: first },
            _ => MulticamAudioPolicy::All,
        };
        session.cut_at(&project, 48, second).unwrap();
        let output = compile_project(project, &session);
        let scene = output
            .compositions
            .iter()
            .find(|scene| scene.id == session.composition)
            .unwrap();
        for picture in &scene.tracks[0].clips {
            let linked = picture.linked.unwrap();
            let linked = scene
                .tracks
                .iter()
                .flat_map(|track| &track.clips)
                .find(|clip| clip.id == linked)
                .unwrap();
            assert_eq!(linked.linked, Some(picture.id));
            assert_eq!(linked.range, picture.range);
        }
        let compiled_sound = sound(output, session.composition);
        let block = compiled_sound.prepare(95_999, 2).unwrap();
        let expected_before = if policy == 2 {
            vec![id(102), id(202)]
        } else {
            vec![id(102)]
        };
        let expected_after = match policy {
            0 => vec![id(202)],
            1 => vec![id(102)],
            _ => vec![id(102), id(202)],
        };
        assert_eq!(active_sound(&block, 0), expected_before);
        assert_eq!(active_sound(&block, 1), expected_after);
    }
}

#[test]
fn invalid_sync_and_edit_requests_are_atomic_and_do_not_modify_original_cameras() {
    let project = project();
    let mut session = session(&project);
    let original = session.clone();
    let first = session.angles[0].id;
    let second = session.angles[1].id;
    assert!(session.cut_at(&project, 240, second).is_err());
    assert!(session.cut_at(&project, 10, Uuid::nil()).is_err());
    assert!(session.switch_at(&project, 240, first).is_err());
    assert!(
        session
            .sync_markers(&project, &[(first, position(1, 1))])
            .is_err()
    );
    assert!(
        session
            .sync_markers(
                &project,
                &[(first, position(1, 1)), (first, position(2, 1))]
            )
            .is_err()
    );
    assert!(
        session
            .sync_markers(
                &project,
                &[(first, position(-1, 1)), (second, position(2, 1))]
            )
            .is_err()
    );
    assert!(
        session
            .sync_markers(
                &project,
                &[(first, position(1, 1)), (second, position(240, 1))]
            )
            .is_err()
    );
    assert_eq!(session, original);
    let mut invalid = session.clone();
    invalid.angles[1].offset = position(1000, 1);
    assert!(invalid.validate(&project).is_err());
    invalid = session.clone();
    invalid.angles[1].composition = id(100);
    assert!(invalid.validate(&project).is_err());
    invalid = session.clone();
    invalid.angles[1].offset.denominator = 0;
    assert!(invalid.validate(&project).is_err());
    invalid = session.clone();
    invalid.angles[1].composition = invalid.composition;
    assert!(invalid.validate(&project).is_err());
}

#[test]
fn silent_cameras_compile_with_all_available_audio_and_explicit_policy_errors() {
    let mut project = project();
    for camera in &mut project.compositions {
        camera.audio = None;
    }
    let mut session = session(&project);
    assert_eq!(session.audio_policy, MulticamAudioPolicy::All);
    let output = compile(&project, &session).unwrap();
    assert!(output.picture.is_some());
    assert!(output.audio.is_none());
    assert_eq!(output.tracks.len(), 1);
    session.audio_policy = MulticamAudioPolicy::FollowVideo;
    assert!(session.validate(&project).is_err());
    session.audio_policy = MulticamAudioPolicy::Fixed {
        angle: session.angles[0].id,
    };
    assert!(session.validate(&project).is_err());
}

#[test]
fn native_project_save_reopen_preserves_session_sources_cuts_and_evaluated_edit() {
    let mut project = project();
    let original_cameras = project.compositions.clone();
    let mut session = session(&project);
    session.cut_at(&project, 48, session.angles[1].id).unwrap();
    session.cut_at(&project, 120, session.angles[0].id).unwrap();
    project = compile_project(project, &session);
    project.multicam.push(session.clone());
    project.validate().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("multicam-core-test-")
        .tempdir_in(root)
        .unwrap();
    let path = directory.path().join("exact-edit.editbay");
    save_new(&project, &path).unwrap();
    let reopened = load(&path).unwrap();
    assert_eq!(reopened.multicam, [session.clone()]);
    assert_eq!(reopened.compositions[..2], original_cameras);
    for (frame, camera) in [
        (0, id(100)),
        (47, id(100)),
        (48, id(200)),
        (119, id(200)),
        (120, id(100)),
        (239, id(100)),
    ] {
        assert_eq!(
            active_picture(&reopened, session.composition, frame).0,
            camera
        );
    }
    let original_tracks = project.compositions.last().unwrap().tracks.clone();
    assert_eq!(
        compile(&reopened, &reopened.multicam[0]).unwrap().tracks,
        original_tracks
    );
}
