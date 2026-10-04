use editbay_core::*;
use std::{collections::BTreeMap, fs};
use uuid::Uuid;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}
fn range() -> FrameRange {
    FrameRange { start: 0, end: 48 }
}
fn map(start: i64, end: i64) -> TimeMap {
    TimeMap {
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
