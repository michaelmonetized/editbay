use editbay_core::*;
use std::{collections::BTreeMap, fs, sync::Arc};
use uuid::Uuid;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}
fn range() -> FrameRange {
    FrameRange { start: 0, end: 48 }
}
fn map(start: i64, end: i64) -> TimeMap {
    TimeMap {
        source_denominator: 1,
        points: vec![
            TimePoint {
                frame: 0,
                source_tick: start,
            },
            TimePoint {
                frame: 48,
                source_tick: end,
            },
        ],
    }
}
fn node(value: u128, operation: NodeOperation) -> TimedNode {
    TimedNode {
        id: id(value),
        range: range(),
        operation,
        animation: Vec::new(),
    }
}

fn model() -> Project {
    let mut project = Project::new("Structured client job").unwrap();
    project.assets = vec![AssetReference {
        id: id(10),
        kind: AssetKind::Media,
        path: "Original camera.mkv".into(),
        sha256: "a".repeat(64),
        bytes: 12345,
        provenance: "Synthetic typed-document fixture; no claim of decoded media".into(),
    }];
    project.sources = vec![MediaSource {
        id: id(20),
        name: "Camera and production sound".into(),
        asset: id(10),
        metadata: BTreeMap::from([("reel".into(), "A001".into())]),
        streams: vec![
            SourceStream {
                index: 0,
                codec: "ffv1".into(),
                time_base: TimeBase {
                    numerator: 1,
                    denominator: 1000,
                },
                start_tick: 0,
                duration_ticks: Some(2000),
                format: StreamFormat::Video {
                    width: 1920,
                    height: 1080,
                    sample_aspect: FrameRate::new(1, 1).unwrap(),
                    timing: PictureTiming::Variable {
                        presentation_ticks: vec![0, 33, 80, 130, 1900],
                        end_tick: 2000,
                    },
                    color: SourceColor {
                        primaries: 1,
                        transfer: 13,
                        matrix: 0,
                        range: 2,
                    },
                    alpha: AlphaMode::Straight,
                },
                metadata: BTreeMap::new(),
            },
            SourceStream {
                index: 1,
                codec: "pcm_f32le".into(),
                time_base: TimeBase {
                    numerator: 1,
                    denominator: 48000,
                },
                start_tick: 0,
                duration_ticks: Some(96000),
                format: StreamFormat::Audio {
                    sample_rate: 48000,
                    channels: vec![
                        "FL".into(),
                        "FR".into(),
                        "FC".into(),
                        "LFE".into(),
                        "BL".into(),
                        "BR".into(),
                    ],
                },
                metadata: BTreeMap::new(),
            },
        ],
    }];
    let picture_clip = Clip {
        id: id(41),
        name: "Retained original".into(),
        range: range(),
        source: ClipSource::Media {
            source: id(20),
            stream: 0,
        },
        time_map: map(0, 2000),
        linked: Some(id(43)),
    };
    let sound_clip = Clip {
        id: id(43),
        name: "Linked six-channel sound".into(),
        range: range(),
        source: ClipSource::Media {
            source: id(20),
            stream: 1,
        },
        time_map: map(0, 96000),
        linked: Some(id(41)),
    };
    let mut animated = node(
        53,
        NodeOperation::Transform {
            image: id(50),
            translation: [0.0, 0.0],
            scale: [1.0, 1.0],
            rotation: 0.0,
            opacity: 1.0,
        },
    );
    animated.animation.push(AnimationChannel {
        id: id(60),
        property: AnimatedProperty::TranslateX,
        interpolation: Interpolation::Linear,
        keys: vec![
            Keyframe {
                frame: 0,
                value: 0.0,
                in_tangent: 0.0,
                out_tangent: 0.0,
            },
            Keyframe {
                frame: 48,
                value: 192.0,
                in_tangent: 0.0,
                out_tangent: 0.0,
            },
        ],
    });
    let child = Composition {
        id: id(30),
        name: "Editable source composite".into(),
        width: 1920,
        height: 1080,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 48,
        tracks: vec![
            Track {
                id: id(40),
                name: "Picture".into(),
                kind: TrackKind::Video,
                enabled: true,
                clips: vec![picture_clip],
            },
            Track {
                id: id(42),
                name: "Six-channel sound".into(),
                kind: TrackKind::Audio,
                enabled: true,
                clips: vec![sound_clip],
            },
        ],
        nodes: vec![
            node(50, NodeOperation::Source { clip: id(41) }),
            node(
                51,
                NodeOperation::Polygon {
                    points: vec![[0.0, 0.0], [1920.0, 0.0], [960.0, 1080.0]],
                },
            ),
            node(
                52,
                NodeOperation::Mask {
                    geometry: id(51),
                    feather: 4.0,
                    inverted: false,
                },
            ),
            animated,
            node(
                54,
                NodeOperation::Solid {
                    rgba: [0.1, 0.2, 1.5, 1.0],
                },
            ),
            node(
                55,
                NodeOperation::Over {
                    foreground: id(53),
                    background: id(54),
                    mask: Some(id(52)),
                },
            ),
            node(56, NodeOperation::Source { clip: id(43) }),
            node(
                57,
                NodeOperation::Gain {
                    audio: id(56),
                    gain: 0.5,
                },
            ),
            node(58, NodeOperation::Scalar { value: 0.8 }),
            node(
                59,
                NodeOperation::Opacity {
                    image: id(55),
                    value: id(58),
                },
            ),
        ],
        picture: Some(id(59)),
        audio: Some(id(57)),
    };
    let parent = Composition {
        id: id(31),
        name: "Nested alternate cut".into(),
        width: 1920,
        height: 1080,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 48,
        tracks: vec![Track {
            id: id(70),
            name: "Reusable composite".into(),
            kind: TrackKind::Video,
            enabled: true,
            clips: vec![Clip {
                id: id(71),
                name: "Reverse nested scene".into(),
                range: range(),
                source: ClipSource::Composition {
                    composition: id(30),
                },
                time_map: map(48, 0),
                linked: None,
            }],
        }],
        nodes: vec![node(72, NodeOperation::Source { clip: id(71) })],
        picture: Some(id(72)),
        audio: None,
    };
    project.compositions = vec![child, parent];
    project.sequences[0].composition = Some(id(31));
    project.validate().unwrap();
    project
}

fn prepared_node(frame: &PreparedFrame, value: u128) -> &PreparedNode {
    frame
        .nodes
        .iter()
        .find(|node| node.id == id(value))
        .unwrap()
}

#[test]
fn compiled_integer_inspection_preserves_the_published_receipt() {
    let project: Project = serde_json::from_slice(include_bytes!(
        "../../../docs/evidence/r2-document/fixture.editbay"
    ))
    .unwrap();
    let expected: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../docs/evidence/r2-document/frame-24.json"
    ))
    .unwrap();
    let snapshot = EvaluationSnapshot::new(Arc::new(project)).unwrap();
    let actual = snapshot.frame_plan(id(30), 24).unwrap();
    assert_eq!(serde_json::to_value(actual).unwrap(), expected);
}

#[test]
fn rational_retimes_cancel_large_factors_and_preserve_unsigned_endpoints() {
    let base = TimeBase {
        numerator: 1,
        denominator: u32::MAX,
    };
    let rate = FrameRate::new(1, u32::MAX).unwrap();
    assert_eq!(
        base.boundary(SourcePosition::new(1, u64::MAX).unwrap(), rate)
            .unwrap(),
        0
    );
    assert_eq!(
        base.boundary(SourcePosition::new(-1, u64::MAX).unwrap(), rate)
            .unwrap(),
        -1
    );
    let forward = TimeMap {
        source_denominator: 1,
        points: vec![
            TimePoint {
                frame: 0,
                source_tick: i64::MIN,
            },
            TimePoint {
                frame: 1,
                source_tick: i64::MAX,
            },
        ],
    };
    let fraction = SourcePosition::new(i64::MAX, u64::MAX).unwrap();
    assert_eq!(
        forward.position_at(fraction).unwrap(),
        SourcePosition::new(-1, 1).unwrap()
    );
    let reverse = TimeMap {
        source_denominator: 1,
        points: vec![
            TimePoint {
                frame: 0,
                source_tick: i64::MAX,
            },
            TimePoint {
                frame: 1,
                source_tick: i64::MIN,
            },
        ],
    };
    assert_eq!(
        reverse.position_at(fraction).unwrap(),
        SourcePosition::new(0, 1).unwrap()
    );
    assert_eq!(
        forward
            .position_at(SourcePosition::new(1, 2).unwrap())
            .unwrap(),
        SourcePosition::new(-1, 2).unwrap()
    );
    assert!(
        forward
            .position_at(SourcePosition::new(1, 4).unwrap())
            .is_err()
    );
    assert!(
        forward
            .position_at(SourcePosition {
                numerator: 0,
                denominator: 0
            })
            .is_err()
    );
    assert!(
        forward
            .position_at(SourcePosition::new(-1, 2).unwrap())
            .is_err()
    );
    let extremes = TimeMap {
        source_denominator: 1,
        points: vec![
            TimePoint {
                frame: 0,
                source_tick: i64::MIN,
            },
            TimePoint {
                frame: u64::MAX,
                source_tick: i64::MAX,
            },
        ],
    };
    assert_eq!(extremes.position(u64::MAX).unwrap().numerator, i64::MAX);
    assert_eq!(extremes.position(u64::MAX / 2).unwrap().numerator, -1);
    assert_eq!(extremes.direction_at(u64::MAX).unwrap(), 1);
}

#[test]
fn nested_different_rates_keep_fractional_picture_sample_and_animation_time() {
    let mut project = model();
    let stream = &mut project.sources[0].streams[0];
    stream.time_base = TimeBase {
        numerator: 1,
        denominator: 60,
    };
    stream.duration_ticks = Some(120);
    if let StreamFormat::Video { timing, .. } = &mut stream.format {
        *timing = PictureTiming::Constant {
            rate: FrameRate::new(60, 1).unwrap(),
        };
    }
    project.compositions[0].frame_rate = FrameRate::new(25, 1).unwrap();
    project.compositions[0].tracks[0].clips[0].time_map = map(0, 120);
    project.compositions[1].frame_rate = FrameRate::new(60, 1).unwrap();
    project.sequences[0].frame_rate = FrameRate::new(60, 1).unwrap();
    project.compositions[1].tracks[0].clips[0].time_map = map(0, 20);
    let snapshot = EvaluationSnapshot::new(Arc::new(project)).unwrap();
    let parent = snapshot
        .prepare(id(31), SourcePosition::new(1, 1).unwrap(), false)
        .unwrap();
    let Some(SourceRequest::Composition {
        composition,
        position,
        reverse,
    }) = parent.nodes[0].source
    else {
        panic!("nested source missing")
    };
    assert_eq!(position, SourcePosition::new(5, 12).unwrap());
    assert!(!reverse);
    let child = snapshot.prepare(composition, position, reverse).unwrap();
    assert!(matches!(
        prepared_node(&child, 50).source,
        Some(SourceRequest::Media {
            picture: Some(1),
            position: SourcePosition {
                numerator: 25,
                denominator: 24
            },
            ..
        })
    ));
    assert!(matches!(
        prepared_node(&child, 56).source,
        Some(SourceRequest::Media {
            sample: Some(833),
            position: SourcePosition {
                numerator: 2500,
                denominator: 3
            },
            ..
        })
    ));
    let NodeOperation::Transform { translation, .. } = *prepared_node(&child, 53).operation else {
        panic!("transform missing")
    };
    assert!((translation[0] - 5.0 / 3.0).abs() < 1e-12);
    let first = snapshot
        .prepare(id(30), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    assert!(matches!(
        prepared_node(&first, 50).source,
        Some(SourceRequest::Media {
            picture: Some(0),
            ..
        })
    ));
}

#[test]
fn reverse_nested_end_owns_the_last_picture_sample_and_preceding_step() {
    let mut project = model();
    project.compositions[0].nodes[3].animation[0].interpolation = Interpolation::Step;
    let snapshot = EvaluationSnapshot::new(Arc::new(project)).unwrap();
    let parent = snapshot
        .prepare(id(31), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    let Some(SourceRequest::Composition {
        composition,
        position,
        reverse,
    }) = parent.nodes[0].source
    else {
        panic!("nested source missing")
    };
    assert_eq!(position.numerator, 48);
    assert!(reverse);
    let child = snapshot.prepare(composition, position, reverse).unwrap();
    assert!(matches!(
        prepared_node(&child, 50).source,
        Some(SourceRequest::Media {
            picture: Some(4),
            reverse: true,
            ..
        })
    ));
    assert!(matches!(
        prepared_node(&child, 56).source,
        Some(SourceRequest::Media {
            sample: Some(95999),
            reverse: true,
            ..
        })
    ));
    assert!(matches!(
        *prepared_node(&child, 53).operation,
        NodeOperation::Transform {
            translation: [0.0, 0.0],
            ..
        }
    ));
    assert!(snapshot.prepare(id(30), position, false).is_err());
    assert!(
        snapshot
            .prepare(id(30), SourcePosition::new(0, 1).unwrap(), true)
            .is_err()
    );
    assert!(
        snapshot
            .prepare(
                id(30),
                SourcePosition {
                    numerator: 0,
                    denominator: 0
                },
                false
            )
            .is_err()
    );
}

#[test]
fn freeze_and_range_boundaries_choose_the_correct_source_side() {
    let mut project = model();
    project.compositions[0].tracks[0].clips[0].time_map = TimeMap {
        source_denominator: 1,
        points: vec![
            TimePoint {
                frame: 0,
                source_tick: 0,
            },
            TimePoint {
                frame: 24,
                source_tick: 130,
            },
            TimePoint {
                frame: 48,
                source_tick: 130,
            },
        ],
    };
    project.compositions[0].nodes[4].range = FrameRange { start: 12, end: 24 };
    let snapshot = EvaluationSnapshot::new(Arc::new(project)).unwrap();
    let after = snapshot
        .prepare(id(30), SourcePosition::new(24, 1).unwrap(), false)
        .unwrap();
    let before = snapshot
        .prepare(id(30), SourcePosition::new(24, 1).unwrap(), true)
        .unwrap();
    assert!(matches!(
        prepared_node(&after, 50).source,
        Some(SourceRequest::Media {
            picture: Some(3),
            reverse: false,
            ..
        })
    ));
    assert!(matches!(
        prepared_node(&before, 50).source,
        Some(SourceRequest::Media {
            picture: Some(2),
            reverse: true,
            ..
        })
    ));
    assert!(!prepared_node(&after, 54).active);
    assert!(prepared_node(&before, 54).active);
    let sample_time = SourcePosition::new(1, 2000).unwrap();
    let after = snapshot.prepare(id(30), sample_time, false).unwrap();
    let before = snapshot.prepare(id(30), sample_time, true).unwrap();
    assert!(matches!(
        prepared_node(&after, 56).source,
        Some(SourceRequest::Media {
            sample: Some(1),
            ..
        })
    ));
    assert!(matches!(
        prepared_node(&before, 56).source,
        Some(SourceRequest::Media {
            sample: Some(0),
            ..
        })
    ));
}

#[test]
fn retained_operations_and_held_picture_keys_reuse_only_matching_dependencies() {
    let project = model();
    let version = DocumentVersion::of(&project);
    let snapshot = EvaluationSnapshot::new(Arc::new(project.clone())).unwrap();
    let first = snapshot
        .prepare(id(30), SourcePosition::new(12, 1).unwrap(), false)
        .unwrap();
    let second = snapshot
        .prepare(id(30), SourcePosition::new(13, 1).unwrap(), false)
        .unwrap();
    assert!(Arc::ptr_eq(
        &prepared_node(&first, 51).operation,
        &prepared_node(&second, 51).operation
    ));
    assert_eq!(
        prepared_node(&first, 50).sha256,
        prepared_node(&second, 50).sha256
    );
    assert_ne!(
        prepared_node(&first, 53).sha256,
        prepared_node(&second, 53).sha256
    );
    assert_ne!(
        prepared_node(&first, 56).sha256,
        prepared_node(&second, 56).sha256
    );
    let mut editor = DocumentEditor::new(project).unwrap();
    editor
        .apply(
            version,
            "Rename retained source".into(),
            &[DocumentCommand::RenameProject {
                name: "Later edit".into(),
            }],
        )
        .unwrap();
    let changed = EvaluationSnapshot::new(editor.snapshot()).unwrap();
    let after = changed
        .prepare(id(30), SourcePosition::new(12, 1).unwrap(), false)
        .unwrap();
    assert_eq!(first.sha256, after.sha256);
    assert_ne!(first.version, after.version);
    let worker = std::thread::spawn(move || {
        snapshot
            .prepare(id(30), SourcePosition::new(12, 1).unwrap(), false)
            .unwrap()
    });
    assert_eq!(worker.join().unwrap().version, version);
}

#[test]
fn mask_bytes_working_color_precision_and_dimensions_invalidate_working_keys() {
    let mut project = model();
    project.assets.push(AssetReference {
        id: id(90),
        kind: AssetKind::Mask,
        path: "Editable mask.exr".into(),
        sha256: "b".repeat(64),
        bytes: 1024,
        provenance: "Synthetic dependency test".into(),
    });
    project.compositions[0].nodes[2].operation = NodeOperation::MaskAsset { asset: id(90) };
    let prepare = |project: &Project| {
        EvaluationSnapshot::new(Arc::new(project.clone()))
            .unwrap()
            .prepare(id(30), SourcePosition::new(24, 1).unwrap(), false)
            .unwrap()
    };
    let original = prepare(&project);
    let mut changed = project.clone();
    changed.assets[1].sha256 = "c".repeat(64);
    let mask = prepare(&changed);
    assert_ne!(
        prepared_node(&original, 52).sha256,
        prepared_node(&mask, 52).sha256
    );
    assert_ne!(
        prepared_node(&original, 59).sha256,
        prepared_node(&mask, 59).sha256
    );
    assert_eq!(
        prepared_node(&original, 50).sha256,
        prepared_node(&mask, 50).sha256
    );
    changed = project.clone();
    changed.assets[1].bytes += 1;
    assert_ne!(
        prepared_node(&original, 52).sha256,
        prepared_node(&prepare(&changed), 52).sha256
    );
    for (gamut, precision, width) in [
        (WorkingGamut::Bt2020, FloatPrecision::Full, 1920),
        (WorkingGamut::Bt709, FloatPrecision::Half, 1920),
        (WorkingGamut::Bt709, FloatPrecision::Full, 1280),
    ] {
        changed = project.clone();
        changed.color.working_gamut = gamut;
        changed.color.precision = precision;
        changed.compositions[0].width = width;
        let after = prepare(&changed);
        for node in [50, 52, 54, 59] {
            assert_ne!(
                prepared_node(&original, node).sha256,
                prepared_node(&after, node).sha256
            );
        }
        for node in [56, 57, 58] {
            assert_eq!(
                prepared_node(&original, node).sha256,
                prepared_node(&after, node).sha256
            );
        }
    }
    changed = project.clone();
    changed.color.display_transfer = OutputTransfer::Bt709;
    changed.color.output_transfer = OutputTransfer::Linear;
    let after = prepare(&changed);
    assert_ne!(original.sha256, after.sha256);
    for node in [50, 52, 54, 59] {
        assert_eq!(
            prepared_node(&original, node).sha256,
            prepared_node(&after, node).sha256
        );
    }
    changed = project;
    changed.assets[0].bytes += 1;
    let after = prepare(&changed);
    assert_ne!(
        prepared_node(&original, 50).sha256,
        prepared_node(&after, 50).sha256
    );
    assert_eq!(
        prepared_node(&original, 54).sha256,
        prepared_node(&after, 54).sha256
    );
    changed.assets[0].bytes -= 1;
    if let StreamFormat::Video { alpha, .. } = &mut changed.sources[0].streams[0].format {
        *alpha = AlphaMode::Premultiplied;
    }
    let after = prepare(&changed);
    assert_ne!(
        prepared_node(&original, 50).sha256,
        prepared_node(&after, 50).sha256
    );
    assert_eq!(
        prepared_node(&original, 54).sha256,
        prepared_node(&after, 54).sha256
    );
}

#[test]
fn structured_job_survives_atomic_commands_undo_save_and_independent_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let original = Project::new("Before").unwrap();
    let fixture = model();
    let mut sequence = original.sequences[0].clone();
    sequence.composition = fixture.sequences[0].composition;
    let commands = vec![
        DocumentCommand::SetAsset {
            asset: fixture.assets[0].clone(),
        },
        DocumentCommand::SetSource {
            source: fixture.sources[0].clone(),
        },
        DocumentCommand::SetComposition {
            composition: fixture.compositions[0].clone(),
        },
        DocumentCommand::SetComposition {
            composition: fixture.compositions[1].clone(),
        },
        DocumentCommand::SetSequence { sequence },
    ];
    let mut editor = DocumentEditor::new(original.clone()).unwrap();
    let receipt = editor
        .apply(
            DocumentVersion::of(&original),
            "Build editable client scene".into(),
            &commands,
        )
        .unwrap();
    assert_eq!(receipt.after.revision, 1);
    assert_eq!(receipt.impact.compositions.len(), 2);
    let authored = editor.project().clone();
    editor.undo(receipt.after).unwrap();
    let mut expected = original.clone();
    expected.revision = 2;
    assert_eq!(editor.project(), &expected);
    editor.redo(DocumentVersion::of(editor.project())).unwrap();
    let mut expected = authored;
    expected.revision = 3;
    assert_eq!(editor.project(), &expected);
    let path = directory.path().join("Client cut.editbay");
    save_new(editor.project(), &path).unwrap();
    assert_eq!(load(&path).unwrap(), expected);
    let source = fs::read(&path).unwrap();
    let checkpoint = checkpoint(
        editor.project(),
        Some(&path),
        directory.path().join("recovery"),
    )
    .unwrap();
    let recovery_bytes = fs::read(&checkpoint).unwrap();
    let recovered =
        recover_copy(&checkpoint, directory.path().join("Separate copy.editbay")).unwrap();
    assert_ne!(recovered.id, expected.id);
    assert_eq!(recovered.recovered_from, Some(expected.id));
    assert_eq!(recovered.compositions, expected.compositions);
    assert_eq!(recovered.sources, expected.sources);
    assert_eq!(fs::read(&path).unwrap(), source);
    assert_eq!(fs::read(checkpoint).unwrap(), recovery_bytes);
}

#[test]
fn old_checkpoint_is_verified_before_migration_and_original_bytes_are_retained() {
    let directory = tempfile::tempdir().unwrap();
    let old = include_bytes!("fixtures/schema1.checkpoint");
    let checkpoint_path = directory.path().join("legacy.checkpoint");
    fs::write(&checkpoint_path, old).unwrap();
    let value: serde_json::Value = serde_json::from_slice(old).unwrap();
    let project_path = directory.path().join("Original.editbay");
    let original = serde_json::to_vec_pretty(&value["snapshot"]["project"]).unwrap();
    fs::write(&project_path, &original).unwrap();
    let loaded = load(&project_path).unwrap();
    assert_eq!(loaded.schema, PROJECT_SCHEMA);
    assert_eq!(loaded.revision, 7);
    assert!(loaded.compositions.is_empty());
    assert_eq!(fs::read(&project_path).unwrap(), original);
    let recovered =
        recover_copy(&checkpoint_path, directory.path().join("Recovered.editbay")).unwrap();
    assert_eq!(recovered.schema, PROJECT_SCHEMA);
    assert_eq!(recovered.revision, 7);
    assert_eq!(recovered.sequences, loaded.sequences);
    assert_eq!(fs::read(&checkpoint_path).unwrap(), old);
    let mut altered = value;
    altered["snapshot"]["project"]["name"] = "Forged".into();
    fs::write(&checkpoint_path, serde_json::to_vec(&altered).unwrap()).unwrap();
    assert!(matches!(
        recover_copy(
            &checkpoint_path,
            directory.path().join("Must not exist.editbay")
        ),
        Err(Error::Integrity)
    ));
    assert!(!directory.path().join("Must not exist.editbay").exists());
    let mut editor = DocumentEditor::new(loaded.clone()).unwrap();
    editor
        .apply(
            DocumentVersion::of(&loaded),
            "First schema 2 edit".into(),
            &[DocumentCommand::RenameProject {
                name: "Migrated edit".into(),
            }],
        )
        .unwrap();
    save_if_unchanged(editor.project(), &project_path, &loaded).unwrap();
    assert_eq!(load(&project_path).unwrap().revision, 8);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(project_path).unwrap()).unwrap()["schema"],
        2
    );
}

#[test]
fn graph_cycles_socket_errors_global_duplicate_ids_and_invalid_links_fail_atomically() {
    let fixture = model();
    let invalid_operations = [
        NodeOperation::Gain {
            audio: id(50),
            gain: 1.0,
        },
        NodeOperation::Transform {
            image: id(53),
            translation: [0.0; 2],
            scale: [1.0; 2],
            rotation: 0.0,
            opacity: 1.0,
        },
        NodeOperation::Transform {
            image: id(999),
            translation: [0.0; 2],
            scale: [1.0; 2],
            rotation: 0.0,
            opacity: 1.0,
        },
    ];
    for operation in invalid_operations {
        let mut invalid = fixture.compositions[0].clone();
        invalid.nodes[3].operation = operation;
        let mut editor = DocumentEditor::new(fixture.clone()).unwrap();
        assert!(
            editor
                .apply(
                    DocumentVersion::of(&fixture),
                    "Invalid graph".into(),
                    &[
                        DocumentCommand::RenameProject {
                            name: "Partial".into()
                        },
                        DocumentCommand::SetComposition {
                            composition: invalid
                        }
                    ]
                )
                .is_err()
        );
        assert_eq!(editor.project(), &fixture);
        assert_eq!(editor.history(), (None, None));
    }
    let mut duplicate = fixture.clone();
    duplicate.assets[0].id = duplicate.sequences[0].id;
    assert!(duplicate.validate().is_err());
    let mut linked = fixture.clone();
    linked.compositions[0].tracks[1].clips[0].linked = None;
    assert!(linked.validate().is_err());
    let mut cycle = fixture.clone();
    cycle.compositions[0].tracks[0].clips[0].source = ClipSource::Composition {
        composition: id(31),
    };
    cycle.compositions[0].tracks[0].clips[0].time_map = map(0, 48);
    assert!(cycle.validate().is_err());
    let mut invalid = fixture.clone();
    invalid.compositions[0].nodes[3].animation[0].keys[1].frame = 0;
    assert!(invalid.validate().is_err());
    invalid = fixture.clone();
    invalid.compositions[0].nodes[4].operation = NodeOperation::Solid {
        rgba: [f64::NAN, 0.0, 0.0, 1.0],
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn exact_retimes_vfr_and_samples_keep_their_distinct_boundaries() {
    let forward = TimeMap {
        source_denominator: 1,
        points: vec![
            TimePoint {
                frame: 0,
                source_tick: 0,
            },
            TimePoint {
                frame: 3,
                source_tick: 10,
            },
            TimePoint {
                frame: 6,
                source_tick: 10,
            },
            TimePoint {
                frame: 9,
                source_tick: 0,
            },
        ],
    };
    assert_eq!(
        forward.position(1).unwrap(),
        SourcePosition {
            numerator: 10,
            denominator: 3
        }
    );
    assert_eq!(
        forward.position(4).unwrap(),
        SourcePosition {
            numerator: 10,
            denominator: 1
        }
    );
    assert_eq!(
        forward.position(7).unwrap(),
        SourcePosition {
            numerator: 20,
            denominator: 3
        }
    );
    assert_eq!(forward.position(9).unwrap().numerator, 0);
    assert!(forward.position(10).is_err());
    let base = TimeBase {
        numerator: 1,
        denominator: 1000,
    };
    let timing = PictureTiming::Variable {
        presentation_ticks: vec![0, 33, 80, 130],
        end_tick: 200,
    };
    for (tick, expected) in [
        (0, Some(0)),
        (32, Some(0)),
        (33, Some(1)),
        (79, Some(1)),
        (80, Some(2)),
        (199, Some(3)),
        (200, None),
    ] {
        assert_eq!(
            timing
                .picture_at(
                    SourcePosition {
                        numerator: tick,
                        denominator: 1
                    },
                    base,
                    0
                )
                .unwrap(),
            expected
        );
    }
    assert_eq!(
        base.boundary(
            SourcePosition {
                numerator: 1001,
                denominator: 1
            },
            FrameRate::new(48000, 1).unwrap()
        )
        .unwrap(),
        48048
    );
    assert_eq!(
        base.boundary(
            SourcePosition {
                numerator: -1,
                denominator: 3
            },
            FrameRate::new(24, 1).unwrap()
        )
        .unwrap(),
        -1
    );
    let rate = FrameRate::new(30000, 1001).unwrap();
    assert_eq!(
        TimeBase {
            numerator: 1001,
            denominator: 30000
        }
        .boundary(
            SourcePosition {
                numerator: 215784,
                denominator: 1
            },
            rate
        )
        .unwrap(),
        215784
    );
    let extremes = TimeMap {
        source_denominator: 1,
        points: vec![
            TimePoint {
                frame: 0,
                source_tick: i64::MIN,
            },
            TimePoint {
                frame: u64::MAX,
                source_tick: i64::MAX,
            },
        ],
    };
    assert_eq!(extremes.position(0).unwrap().numerator, i64::MIN);
    assert_eq!(extremes.position(u64::MAX).unwrap().numerator, i64::MAX);
    assert!(
        TimeBase {
            numerator: u32::MAX,
            denominator: 1
        }
        .boundary(
            SourcePosition {
                numerator: i64::MAX,
                denominator: 1
            },
            FrameRate::new(u32::MAX, 1).unwrap()
        )
        .is_err()
    );
}

#[test]
fn channels_evaluate_and_history_restores_collection_order_and_dependency_impact() {
    let fixture = model();
    let channel = &fixture.compositions[0].nodes[3].animation[0];
    assert_eq!(channel.value_at(24).unwrap(), 96.0);
    let mut hermite = channel.clone();
    hermite.interpolation = Interpolation::Hermite;
    assert_eq!(hermite.value_at(24).unwrap(), 96.0);
    let mut step = channel.clone();
    step.interpolation = Interpolation::Step;
    assert_eq!(step.value_at(47).unwrap(), 0.0);
    let mut editor = DocumentEditor::new(fixture.clone()).unwrap();
    let renamed = editor
        .apply(
            DocumentVersion::of(&fixture),
            "Name".into(),
            &[DocumentCommand::RenameProject {
                name: "Renamed only".into(),
            }],
        )
        .unwrap();
    assert!(renamed.impact.compositions.is_empty());
    assert!(editor.history_usage().1 < 1024);
    let mut source = fixture.sources[0].clone();
    source
        .metadata
        .insert("timecode".into(), "01:00:00:00".into());
    let receipt = editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Source metadata".into(),
            &[DocumentCommand::SetSource { source }],
        )
        .unwrap();
    assert_eq!(receipt.impact.compositions, vec![id(30), id(31)]);
    let mut plain = Project::new("Order").unwrap();
    plain.assets = (0..3)
        .map(|index| AssetReference {
            id: id(100 + index),
            kind: AssetKind::Artwork,
            path: format!("{index}.svg").into(),
            sha256: "b".repeat(64),
            bytes: 1,
            provenance: String::new(),
        })
        .collect();
    let original = plain.clone();
    let mut editor = DocumentEditor::new(plain).unwrap();
    editor
        .apply(
            DocumentVersion::of(&original),
            "Reorder".into(),
            &[
                DocumentCommand::RemoveAsset { id: id(100) },
                DocumentCommand::SetAsset {
                    asset: original.assets[0].clone(),
                },
            ],
        )
        .unwrap();
    assert_eq!(
        editor
            .project()
            .assets
            .iter()
            .map(|asset| asset.id)
            .collect::<Vec<_>>(),
        vec![id(101), id(102), id(100)]
    );
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().assets, original.assets);
    editor.redo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().assets[2].id, id(100));
}

#[test]
fn frame_plans_resolve_animation_sources_and_transitive_content_without_global_revision_churn() {
    let mut fixture = model();
    let plan = fixture.frame_plan(id(30), 24).unwrap();
    assert_eq!(plan.nodes.len(), 10);
    let transform = plan.nodes.iter().find(|node| node.id == id(53)).unwrap();
    assert!(matches!(
        transform.operation,
        NodeOperation::Transform {
            translation: [96.0, 0.0],
            ..
        }
    ));
    let picture = plan.nodes.iter().find(|node| node.id == id(50)).unwrap();
    assert!(matches!(
        picture.source,
        Some(SourceRequest::Media {
            picture: Some(3),
            sample: None,
            position: SourcePosition {
                numerator: 1000,
                denominator: 1
            },
            ..
        })
    ));
    let sound = plan.nodes.iter().find(|node| node.id == id(56)).unwrap();
    assert!(matches!(
        sound.source,
        Some(SourceRequest::Media {
            picture: None,
            sample: Some(48000),
            ..
        })
    ));
    assert!(fixture.frame_plan(id(30), 48).is_err());
    let parent = fixture.frame_plan(id(31), 0).unwrap();
    assert!(matches!(
        parent.nodes[0].source,
        Some(SourceRequest::Composition {
            reverse: true,
            position: SourcePosition {
                numerator: 48,
                denominator: 1
            },
            ..
        })
    ));
    fixture.rename("Metadata-only rename").unwrap();
    fixture.compositions[0].name = "Renamed scene".into();
    fixture.sources[0]
        .metadata
        .insert("reel".into(), "A002".into());
    let after = fixture.frame_plan(id(30), 24).unwrap();
    assert_eq!(after.sha256, plan.sha256);
    assert_ne!(after.version, plan.version);
    assert_eq!(fixture.frame_plan(id(31), 0).unwrap().sha256, parent.sha256);
    let leaf_hash = fixture.composition_fingerprint(id(30)).unwrap();
    fixture.compositions[1].name = "Another parent label".into();
    assert_eq!(fixture.composition_fingerprint(id(30)).unwrap(), leaf_hash);
    fixture.compositions[0].nodes[4].operation = NodeOperation::Solid {
        rgba: [0.7, 0.2, 1.5, 1.0],
    };
    assert_ne!(fixture.frame_plan(id(31), 0).unwrap().sha256, parent.sha256);
    fixture.compositions[0].tracks[0].enabled = false;
    let muted = fixture.frame_plan(id(30), 24).unwrap();
    assert!(
        !muted
            .nodes
            .iter()
            .find(|node| node.id == id(50))
            .unwrap()
            .active
    );
    assert!(
        muted
            .nodes
            .iter()
            .find(|node| node.id == id(50))
            .unwrap()
            .source
            .is_none()
    );
    let timing = PictureTiming::Variable {
        presentation_ticks: vec![0, 33, 80],
        end_tick: 100,
    };
    let base = TimeBase {
        numerator: 1,
        denominator: 1000,
    };
    assert_eq!(
        timing
            .picture_before(
                SourcePosition {
                    numerator: 100,
                    denominator: 1
                },
                base,
                0
            )
            .unwrap(),
        Some(2)
    );
    assert_eq!(
        timing
            .picture_before(
                SourcePosition {
                    numerator: 80,
                    denominator: 1
                },
                base,
                0
            )
            .unwrap(),
        Some(1)
    );
    let cfr = PictureTiming::Constant {
        rate: FrameRate::new(24, 1).unwrap(),
    };
    assert_eq!(
        cfr.picture_before(
            SourcePosition {
                numerator: 2000,
                denominator: 1
            },
            base,
            0
        )
        .unwrap(),
        Some(47)
    );
}

#[test]
fn captured_document_and_cloned_sessions_keep_independent_revision_ownership() {
    let mut editor = DocumentEditor::new(model()).unwrap();
    let captured = editor.snapshot();
    let version = DocumentVersion::of(&captured);
    let mut other = editor.clone();
    editor
        .apply(
            version,
            "Main session".into(),
            &[DocumentCommand::RenameProject {
                name: "Main edit".into(),
            }],
        )
        .unwrap();
    other
        .apply(
            version,
            "Independent session".into(),
            &[DocumentCommand::RenameProject {
                name: "Other edit".into(),
            }],
        )
        .unwrap();
    assert_eq!(captured.name, "Structured client job");
    assert_eq!(captured.revision, 0);
    assert_eq!(editor.project().name, "Main edit");
    assert_eq!(other.project().name, "Other edit");
    let worker = std::thread::spawn(move || captured.frame_plan(id(30), 24).unwrap());
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().revision, 2);
    assert_eq!(worker.join().unwrap().version, version);
    assert_eq!(other.project().revision, 1);
}

#[test]
fn document_and_recovery_read_and_write_budgets_preserve_existing_files() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("Original.editbay");
    let valid = Project::new("Retained original").unwrap();
    save_new(&valid, &original).unwrap();
    let bytes = fs::read(&original).unwrap();
    let mut oversized = valid.clone();
    oversized.name = "x".repeat(MAX_DOCUMENT_BYTES as usize);
    oversized.revision = 1;
    assert!(save_if_unchanged(&oversized, &original, &valid).is_err());
    assert_eq!(fs::read(&original).unwrap(), bytes);
    assert!(
        checkpoint(
            &oversized,
            Some(&original),
            directory.path().join("Unpublished")
        )
        .is_err()
    );
    assert!(!directory.path().join("Unpublished").exists());
    let oversized_path = directory.path().join("Oversized.editbay");
    fs::File::create(&oversized_path)
        .unwrap()
        .set_len(MAX_DOCUMENT_BYTES + 1)
        .unwrap();
    assert!(matches!(load(&oversized_path), Err(Error::Invalid(_))));
    assert!(
        recover_copy(
            &oversized_path,
            directory.path().join("Never published.editbay")
        )
        .is_err()
    );
    assert!(!directory.path().join("Never published.editbay").exists());
}
