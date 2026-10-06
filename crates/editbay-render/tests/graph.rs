use editbay_core::*;
use editbay_media::{Cancellation, PictureBudget, SourceFile, VideoReader};
use editbay_render::{GraphBudget, GraphRenderer, ImageBoundary};
use std::{fs, path::Path, process::Command, sync::Arc};
use uuid::Uuid;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn node(n: u128, operation: NodeOperation) -> TimedNode {
    TimedNode {
        id: id(n),
        range: FrameRange { start: 0, end: 48 },
        operation,
        animation: vec![],
    }
}
fn scene(nodes: Vec<TimedNode>, root: u128) -> Composition {
    Composition {
        id: id(100),
        name: "Actual GPU fixture".into(),
        width: 8,
        height: 6,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 48,
        tracks: vec![],
        nodes,
        picture: Some(id(root)),
        audio: None,
    }
}
fn snapshot(project: Project) -> Arc<EvaluationSnapshot> {
    Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap())
}
fn solid_project() -> Project {
    let mut project = Project::new("Shared picture graph").unwrap();
    project.compositions = vec![scene(
        vec![node(
            1,
            NodeOperation::Solid {
                rgba: [0.2, 0.4, 0.8, 0.5],
            },
        )],
        1,
    )];
    project
}
fn renderer(project: Project) -> GraphRenderer {
    GraphRenderer::new(
        snapshot(project),
        PictureBudget::default(),
        GraphBudget::default(),
        Cancellation::new().unwrap(),
    )
    .unwrap()
}
fn close(actual: &[f32], expected: &[f32], tolerance: f32) {
    assert_eq!(actual.len(), expected.len());
    for (index, (a, b)) in actual.iter().zip(expected).enumerate() {
        assert!(
            a.is_finite() && (a - b).abs() <= tolerance,
            "channel {index}: {a} != {b}"
        );
    }
}
fn repeated(value: [f32; 4]) -> Vec<f32> {
    (0..48).flat_map(|_| value).collect()
}
fn decode(v: u8) -> f32 {
    let v = f32::from(v) / 255.;
    if v < 0.081 {
        v / 4.5
    } else {
        ((v + 0.099) / 1.099).powf(1. / 0.45)
    }
}

#[test]
fn actual_typed_transform_over_and_animated_opacity_match_independent_pixels() {
    for (precision, tolerance) in [
        (FloatPrecision::Half, 0.001),
        (FloatPrecision::Full, 0.00002),
    ] {
        let mut project = solid_project();
        project.color.precision = precision;
        let mut scalar = node(5, NodeOperation::Scalar { value: 0. });
        scalar.animation = vec![AnimationChannel {
            id: id(60),
            property: AnimatedProperty::Scalar,
            interpolation: Interpolation::Linear,
            keys: vec![
                Keyframe {
                    frame: 0,
                    value: 0.25,
                    in_tangent: 0.,
                    out_tangent: 0.,
                },
                Keyframe {
                    frame: 48,
                    value: 0.75,
                    in_tangent: 0.,
                    out_tangent: 0.,
                },
            ],
        }];
        project.compositions[0] = scene(
            vec![
                node(
                    1,
                    NodeOperation::Solid {
                        rgba: [0.8, 0.2, 0.1, 0.5],
                    },
                ),
                node(
                    2,
                    NodeOperation::Solid {
                        rgba: [0.1, 0.3, 0.7, 0.75],
                    },
                ),
                node(
                    3,
                    NodeOperation::Transform {
                        image: id(1),
                        translation: [2., 1.],
                        scale: [1., 1.],
                        rotation: 0.,
                        opacity: 1.,
                    },
                ),
                scalar,
                node(
                    6,
                    NodeOperation::Opacity {
                        image: id(3),
                        value: id(5),
                    },
                ),
                node(
                    7,
                    NodeOperation::Over {
                        foreground: id(6),
                        background: id(2),
                        mask: None,
                    },
                ),
            ],
            7,
        );
        let mut worker = renderer(project);
        let frame = worker
            .render(id(100), SourcePosition::new(24, 1).unwrap(), false)
            .unwrap();
        assert_eq!(worker.stats().readbacks, 0);
        let expected: Vec<_> = (0..6)
            .flat_map(|y| {
                (0..8).flat_map(move |x| {
                    if x >= 2 && y >= 1 {
                        [0.25625, 0.21875, 0.41875, 0.8125]
                    } else {
                        [0.075, 0.225, 0.525, 0.75]
                    }
                })
            })
            .collect();
        close(&worker.readback(&frame).unwrap(), &expected, tolerance);
        let dispatches = worker.stats().dispatches;
        let hit = worker
            .render(id(100), SourcePosition::new(24, 1).unwrap(), false)
            .unwrap();
        assert!(Arc::ptr_eq(frame.image(), hit.image()));
        assert_eq!(worker.stats().dispatches, dispatches);
        let changed = worker
            .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
            .unwrap();
        assert!(!Arc::ptr_eq(frame.image(), changed.image()));
        close(
            &worker.readback(&changed).unwrap()[..4],
            &[0.075, 0.225, 0.525, 0.75],
            tolerance,
        );
        worker.verify_sources().unwrap();
    }
}

#[test]
fn signed_gamut_display_output_and_rebind_keep_working_content_separate() {
    let mut project = solid_project();
    project.color.display_gamut = WorkingGamut::Bt2020;
    project.color.display_transfer = OutputTransfer::Linear;
    project.color.output_transfer = OutputTransfer::Srgb;
    project.compositions[0].nodes[0].operation = NodeOperation::Solid {
        rgba: [1., 0., 0., 0.5],
    };
    let mut worker = renderer(project.clone());
    let frame = worker
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    close(
        &worker.readback(&frame).unwrap(),
        &repeated([0.5, 0., 0., 0.5]),
        0.00001,
    );
    let display = worker.convert(&frame, ImageBoundary::Display).unwrap();
    close(
        &worker.readback(&display).unwrap(),
        &repeated([0.313_701_96, 0.034_548_644, 0.008_195_72, 0.5]),
        0.00002,
    );
    let output = worker.convert(&frame, ImageBoundary::Output).unwrap();
    close(
        &worker.readback(&output).unwrap(),
        &repeated([0.5, 0., 0., 0.5]),
        0.00002,
    );
    project.rename("Display correction").unwrap();
    project.color.display_transfer = OutputTransfer::Srgb;
    worker
        .rebind(snapshot(project.clone()), Cancellation::new().unwrap())
        .unwrap();
    assert!(worker.validate_result(&frame).is_err());
    let same = worker
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    assert!(Arc::ptr_eq(frame.image(), same.image()));
    assert_ne!(frame.version(), same.version());
    let changed = worker.convert(&same, ImageBoundary::Display).unwrap();
    assert!(!Arc::ptr_eq(display.image(), changed.image()));
    project.rename("Working correction").unwrap();
    project.color.working_gamut = WorkingGamut::DisplayP3;
    worker
        .rebind(snapshot(project.clone()), Cancellation::new().unwrap())
        .unwrap();
    let p3 = worker
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    assert!(!Arc::ptr_eq(frame.image(), p3.image()));
    let output = worker.convert(&p3, ImageBoundary::Output).unwrap();
    let values = worker.readback(&output).unwrap();
    assert!(values[0] > 0.5 && values[1] < 0. && values[2] < 0.);
    project.rename("HDR output").unwrap();
    project.color.output_transfer = OutputTransfer::Pq;
    worker
        .rebind(snapshot(project.clone()), Cancellation::new().unwrap())
        .unwrap();
    let linear = worker
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    assert!(worker.convert(&linear, ImageBoundary::Output).is_err());
    let foreign = renderer(project)
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    assert!(worker.validate_result(&foreign).is_err());
}

#[test]
fn nested_fraction_reverse_step_and_empty_ranges_match_exact_shared_evaluation() {
    let mut project = solid_project();
    let mut child = project.compositions[0].clone();
    child.frame_rate = FrameRate::new(25, 1).unwrap();
    child.nodes[0].animation = vec![AnimationChannel {
        id: id(70),
        property: AnimatedProperty::Red,
        interpolation: Interpolation::Step,
        keys: vec![
            Keyframe {
                frame: 0,
                value: 0.2,
                in_tangent: 0.,
                out_tangent: 0.,
            },
            Keyframe {
                frame: 1,
                value: 0.8,
                in_tangent: 0.,
                out_tangent: 0.,
            },
        ],
    }];
    let mut parent = scene(vec![node(2, NodeOperation::Source { clip: id(21) })], 2);
    parent.id = id(101);
    parent.frame_rate = FrameRate::new(60, 1).unwrap();
    parent.tracks = vec![Track {
        id: id(20),
        name: "Exact nested child".into(),
        kind: TrackKind::Video,
        enabled: true,
        clips: vec![Clip {
            id: id(21),
            name: "Nested forward".into(),
            range: FrameRange { start: 0, end: 48 },
            source: ClipSource::Composition {
                composition: id(100),
            },
            time_map: TimeMap {
                points: vec![
                    TimePoint {
                        frame: 0,
                        source_tick: 0,
                    },
                    TimePoint {
                        frame: 48,
                        source_tick: 20,
                    },
                ],
            },
            linked: None,
        }],
    }];
    project.compositions = vec![child, parent];
    for budget in [
        GraphBudget {
            nesting_depth: 1,
            ..GraphBudget::default()
        },
        GraphBudget {
            nodes_per_request: 1,
            ..GraphBudget::default()
        },
    ] {
        let mut bounded = GraphRenderer::new(
            snapshot(project.clone()),
            PictureBudget::default(),
            budget,
            Cancellation::new().unwrap(),
        )
        .unwrap();
        assert!(
            bounded
                .render(id(101), SourcePosition::new(1, 1).unwrap(), false)
                .is_err()
        );
        assert_eq!(bounded.stats().dispatches, 0);
    }
    let mut worker = renderer(project.clone());
    let before = worker
        .render(id(101), SourcePosition::new(1, 1).unwrap(), false)
        .unwrap();
    close(
        &worker.readback(&before).unwrap(),
        &repeated([0.1, 0.2, 0.4, 0.5]),
        0.00002,
    );
    let after = worker
        .render(id(101), SourcePosition::new(3, 1).unwrap(), false)
        .unwrap();
    close(
        &worker.readback(&after).unwrap(),
        &repeated([0.4, 0.2, 0.4, 0.5]),
        0.00002,
    );
    let reverse = worker
        .render(id(100), SourcePosition::new(1, 1).unwrap(), true)
        .unwrap();
    close(
        &worker.readback(&reverse).unwrap(),
        &repeated([0.1, 0.2, 0.4, 0.5]),
        0.00002,
    );
    let outside = worker.render(id(100), SourcePosition::new(48, 1).unwrap(), false);
    assert!(outside.is_err());
    project.compositions[0].nodes[0].range.end = 24;
    project.compositions[0].nodes[0].animation.clear();
    project.rename("Inactive node").unwrap();
    worker
        .rebind(snapshot(project), Cancellation::new().unwrap())
        .unwrap();
    let inactive = worker
        .render(id(100), SourcePosition::new(25, 1).unwrap(), false)
        .unwrap();
    close(&worker.readback(&inactive).unwrap(), &repeated([0.; 4]), 0.);
}

#[test]
fn consumer_pins_eviction_pending_work_cleanup_and_cancel_obey_limits() {
    let mut project = solid_project();
    project.compositions[0].nodes[0].animation = vec![AnimationChannel {
        id: id(71),
        property: AnimatedProperty::Red,
        interpolation: Interpolation::Linear,
        keys: vec![
            Keyframe {
                frame: 0,
                value: 0.2,
                in_tangent: 0.,
                out_tangent: 0.,
            },
            Keyframe {
                frame: 48,
                value: 0.8,
                in_tangent: 0.,
                out_tangent: 0.,
            },
        ],
    }];
    let cancel = Cancellation::new().unwrap();
    let budget = GraphBudget {
        cache_bytes: 1536,
        live_bytes: 3072,
        maximum_texture_bytes: 768,
        cache_entries: 2,
        pending_submissions: 1,
        ..GraphBudget::default()
    };
    let mut worker = GraphRenderer::new(
        snapshot(project),
        PictureBudget::default(),
        budget,
        cancel.clone(),
    )
    .unwrap();
    let mut pins = Vec::new();
    for n in 0..3 {
        pins.push(
            worker
                .render(id(100), SourcePosition::new(n, 1).unwrap(), false)
                .unwrap(),
        );
    }
    worker.finish().unwrap();
    assert_eq!(worker.stats().live_texture_bytes, 2304);
    assert!(
        worker.stats().entries <= 2
            && worker.stats().cache_bytes <= 1536
            && worker.stats().pending_submissions == 0
    );
    assert!(
        worker
            .render(id(100), SourcePosition::new(4, 1).unwrap(), false)
            .is_err()
    );
    pins.remove(0);
    pins.push(
        worker
            .render(id(100), SourcePosition::new(4, 1).unwrap(), false)
            .unwrap(),
    );
    worker.clear().unwrap();
    assert_eq!(worker.stats().entries, 0);
    assert_eq!(worker.stats().cache_bytes, 0);
    assert_eq!(worker.stats().live_texture_bytes, 2304);
    assert!(worker.validate_result(&pins[0]).is_err());
    drop(pins);
    assert_eq!(worker.stats().live_texture_bytes, 0);
    cancel.cancel();
    assert!(
        worker
            .render(id(100), SourcePosition::new(5, 1).unwrap(), false)
            .is_err()
    );
    worker.clear().unwrap();
    assert_eq!(worker.stats().live_texture_bytes, 0);
    assert!(
        GraphBudget {
            live_bytes: 4,
            ..budget
        }
        .validate()
        .is_err()
    );
}

fn media_project(path: &Path) -> Project {
    let result = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=16x16:rate=4:duration=1",
            "-c:v",
            "libx264",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let cancel = Cancellation::new().unwrap();
    let source = SourceFile::open(path, &cancel).unwrap();
    let imported = source
        .ingest("Real fixture".into(), &[0], cancel, |_, _| {})
        .unwrap();
    let mut project = solid_project();
    project.assets = vec![imported.asset];
    project.sources = vec![imported.source];
    let profile = &project.sources[0].streams[0];
    let end = profile.start_tick + profile.duration_ticks.unwrap() as i64;
    let mut composition = scene(vec![node(1, NodeOperation::Source { clip: id(41) })], 1);
    composition.width = 16;
    composition.height = 16;
    composition.tracks = vec![Track {
        id: id(40),
        name: "Original pictures".into(),
        kind: TrackKind::Video,
        enabled: true,
        clips: vec![Clip {
            id: id(41),
            name: "Original source".into(),
            range: FrameRange { start: 0, end: 48 },
            source: ClipSource::Media {
                source: project.sources[0].id,
                stream: 0,
            },
            time_map: TimeMap {
                points: vec![
                    TimePoint {
                        frame: 0,
                        source_tick: profile.start_tick,
                    },
                    TimePoint {
                        frame: 48,
                        source_tick: end,
                    },
                ],
            },
            linked: None,
        }],
    }];
    project.compositions = vec![composition];
    project
}
#[test]
fn real_source_pixels_cached_hits_interpretation_orientation_and_replacement_are_checked() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Original tagged source.mp4");
    let project = media_project(&path);
    let cancel = Cancellation::new().unwrap();
    let source = SourceFile::open(&path, &cancel).unwrap();
    let mut reference = VideoReader::open_stream(&source, 0, cancel).unwrap();
    let mut expected = Vec::new();
    while let Some(frame) = reference.next_frame().unwrap() {
        expected.push(frame);
    }
    let mut worker = renderer(project.clone());
    for (n, expected) in expected.iter().enumerate() {
        let actual = worker
            .render(
                id(100),
                SourcePosition::new(n as i64 * 12, 1).unwrap(),
                false,
            )
            .unwrap();
        let linear: Vec<_> = expected
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [decode(p[0]), decode(p[1]), decode(p[2]), 1.])
            .collect();
        close(&worker.readback(&actual).unwrap(), &linear, 0.00002);
        let upload_count = worker.stats().uploads;
        let hit = worker
            .render(
                id(100),
                SourcePosition::new(n as i64 * 12, 1).unwrap(),
                false,
            )
            .unwrap();
        assert!(Arc::ptr_eq(actual.image(), hit.image()));
        assert_eq!(worker.stats().uploads, upload_count);
    }
    worker.verify_sources().unwrap();
    let mut changed = project.clone();
    if let StreamFormat::Video { color, .. } = &mut changed.sources[0].streams[0].format {
        color.matrix = 5;
    }
    changed
        .rename("Unsupported source matrix override")
        .unwrap();
    worker
        .rebind(snapshot(changed), Cancellation::new().unwrap())
        .unwrap();
    assert!(
        worker
            .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
            .is_err()
    );
    let mut changed = project.clone();
    changed.sources[0].streams[0]
        .metadata
        .insert("editbay.display_rotation_degrees".into(), "90".into());
    changed.rename("Orientation").unwrap();
    worker
        .rebind(snapshot(changed), Cancellation::new().unwrap())
        .unwrap();
    assert!(
        worker
            .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
            .is_err()
    );
    worker
        .rebind(snapshot(project), Cancellation::new().unwrap())
        .unwrap();
    let preserved = fs::read(&path).unwrap();
    let old = directory.path().join("Preserved original.mp4");
    fs::rename(&path, &old).unwrap();
    fs::write(&path, b"replacement untouched").unwrap();
    assert!(
        worker
            .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
            .is_err()
    );
    assert!(worker.verify_sources().is_err());
    assert_eq!(fs::read(&old).unwrap(), preserved);
    assert_eq!(fs::read(&path).unwrap(), b"replacement untouched");
    worker.clear().unwrap();
    assert_eq!(worker.stats().live_texture_bytes, 0);
    assert_eq!(worker.stats().pictures.live_bytes, 0);
}

#[test]
fn masked_transparency_and_unsupported_solid_bounds_do_not_fake_output() {
    let mut project = solid_project();
    project.compositions[0].nodes[0].operation = NodeOperation::Solid { rgba: [0.; 4] };
    project.compositions[0].nodes.extend([
        node(
            3,
            NodeOperation::Polygon {
                points: vec![[0., 0.], [4., 0.], [4., 4.]],
            },
        ),
        node(
            4,
            NodeOperation::Mask {
                geometry: id(3),
                feather: 0.,
                inverted: false,
            },
        ),
        node(
            5,
            NodeOperation::Over {
                foreground: id(1),
                background: id(1),
                mask: Some(id(4)),
            },
        ),
    ]);
    project.compositions[0].picture = Some(id(5));
    let mut worker = renderer(project);
    let frame = worker
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    close(&worker.readback(&frame).unwrap(), &repeated([0.; 4]), 0.);
    drop(frame);
    let mut project = solid_project();
    project.compositions[0].nodes[0].operation = NodeOperation::Solid {
        rgba: [5000., 0., 0., 0.],
    };
    worker
        .rebind(snapshot(project), Cancellation::new().unwrap())
        .unwrap();
    assert!(
        worker
            .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
            .is_err()
    );
}

#[test]
fn affine_scale_rotation_mirror_zero_and_fractional_edges_follow_pixel_center_semantics() {
    for (translation, scale, rotation, pattern) in [
        ([0., 0.], [0.5, 1.], 0., 0),
        ([0., 0.], [1., 1.], 90., 1),
        ([0., 0.], [-1., 1.], 0., 2),
        ([0., 0.], [0., 1.], 0., 3),
        ([0.5, 0.], [1., 1.], 0., 4),
    ] {
        let mut project = solid_project();
        project.compositions[0].nodes.push(node(
            2,
            NodeOperation::Transform {
                image: id(1),
                translation,
                scale,
                rotation,
                opacity: 1.,
            },
        ));
        project.compositions[0].picture = Some(id(2));
        let mut worker = renderer(project);
        let frame = worker
            .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
            .unwrap();
        let expected: Vec<_> = (0..6)
            .flat_map(|_| {
                (0..8).flat_map(move |x| {
                    let weight = match pattern {
                        0 if (2..6).contains(&x) => 1.,
                        1 if (1..7).contains(&x) => 1.,
                        2 => 1.,
                        4 if x == 0 => 0.5,
                        4 => 1.,
                        _ => 0.,
                    };
                    [0.1 * weight, 0.2 * weight, 0.4 * weight, 0.5 * weight]
                })
            })
            .collect();
        close(&worker.readback(&frame).unwrap(), &expected, 0.00002);
    }
    let mut project = solid_project();
    project.compositions[0].nodes.push(node(
        2,
        NodeOperation::Transform {
            image: id(1),
            translation: [-4., -3.],
            scale: [1e-38; 2],
            rotation: 0.,
            opacity: 1.,
        },
    ));
    project.compositions[0].picture = Some(id(2));
    let mut worker = renderer(project);
    assert!(
        worker
            .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
            .is_err()
    );
}

#[test]
fn source_pixel_aspect_is_applied_and_native_rotation_cannot_be_hidden_by_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Source.mp4");
    let mut project = media_project(&path);
    let cancel = Cancellation::new().unwrap();
    let source = SourceFile::open(&path, &cancel).unwrap();
    let raw = VideoReader::open_stream(&source, 0, cancel.clone())
        .unwrap()
        .next_frame()
        .unwrap()
        .unwrap();
    if let StreamFormat::Video { sample_aspect, .. } = &mut project.sources[0].streams[0].format {
        *sample_aspect = FrameRate::new(2, 1).unwrap();
    }
    let mut worker = renderer(project.clone());
    let frame = worker
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    let expected: Vec<_> = (0..16)
        .flat_map(|y| {
            (0..16).flat_map({
                let raw = &raw;
                move |x| {
                    if !(4..12).contains(&y) {
                        return [0.; 4];
                    }
                    let row = (y - 4) * 2;
                    let mut value = [0.; 4];
                    for (channel, value) in value.iter_mut().enumerate().take(3) {
                        *value = (decode(raw.rgba[(row * 16 + x) * 4 + channel])
                            + decode(raw.rgba[((row + 1) * 16 + x) * 4 + channel]))
                            * 0.5;
                    }
                    value[3] = 1.;
                    value
                }
            })
        })
        .collect();
    close(&worker.readback(&frame).unwrap(), &expected, 0.00002);
    let rotated = directory.path().join("Rotated original.mp4");
    let result = Command::new("ffmpeg")
        .args(["-v", "error", "-display_rotation", "90", "-i"])
        .arg(&path)
        .args(["-c", "copy"])
        .arg(&rotated)
        .output()
        .unwrap();
    assert!(result.status.success());
    let owned = SourceFile::open(&rotated, &cancel).unwrap();
    let mut imported = owned
        .ingest("Native rotated source".into(), &[0], cancel, |_, _| {})
        .unwrap();
    assert_eq!(
        imported.source.streams[0].metadata["editbay.display_rotation_degrees"],
        "-90"
    );
    imported.source.streams[0]
        .metadata
        .insert("editbay.display_rotation_degrees".into(), "0".into());
    project.compositions[0].tracks[0].clips[0].source = ClipSource::Media {
        source: imported.source.id,
        stream: 0,
    };
    project.assets = vec![imported.asset];
    project.sources = vec![imported.source];
    project.rename("Cannot hide native orientation").unwrap();
    worker
        .rebind(snapshot(project), Cancellation::new().unwrap())
        .unwrap();
    assert!(
        worker
            .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
            .is_err()
    );
}

#[test]
fn inactive_over_history_reuses_active_pixels_and_keeps_each_pin_charged() {
    for (precision, tolerance, bytes_per_pixel) in [
        (FloatPrecision::Half, 0.001, 8),
        (FloatPrecision::Full, 0.00002, 16),
    ] {
        let mut project = solid_project();
        project.color.precision = precision;
        let composition = &mut project.compositions[0];
        composition.duration = 128;
        composition.nodes[0].operation = NodeOperation::Solid { rgba: [0.; 4] };
        composition.nodes[0].range.end = 128;
        let mut background = id(1);
        for index in 0..128 {
            let mut solid = node(
                1000 + index,
                NodeOperation::Solid {
                    rgba: [0.2 + index as f64 / 512., 0.4, 0.8, 0.5],
                },
            );
            solid.range = FrameRange {
                start: index as u64,
                end: index as u64 + 1,
            };
            let mut over = node(
                2000 + index,
                NodeOperation::Over {
                    foreground: solid.id,
                    background,
                    mask: None,
                },
            );
            over.range.end = 128;
            background = over.id;
            composition.nodes.extend([solid, over]);
        }
        composition.picture = Some(background);
        let mut worker = renderer(project);
        let mut pins = Vec::new();
        for (position, before, active) in [
            (0, false, 0),
            (64, false, 64),
            (64, true, 63),
            (127, false, 127),
        ] {
            let frame = worker
                .render(id(100), SourcePosition::new(position, 1).unwrap(), before)
                .unwrap();
            close(
                &worker.readback(&frame).unwrap(),
                &repeated([0.1 + active as f32 / 1024., 0.2, 0.4, 0.5]),
                tolerance,
            );
            pins.push(frame);
        }
        worker.finish().unwrap();
        let payload = 8 * 6 * bytes_per_pixel;
        assert_eq!(worker.stats().dispatches, 5);
        assert_eq!(worker.stats().entries, 5);
        assert_eq!(worker.stats().cache_bytes, payload * 5);
        assert_eq!(worker.stats().live_texture_bytes, payload * 5);
        assert_eq!(worker.stats().simplified_nodes, 4 * 129);
        worker.clear().unwrap();
        assert_eq!(worker.stats().entries, 0);
        assert_eq!(worker.stats().live_texture_bytes, payload * 4);
        assert!(worker.validate_result(&pins[0]).is_err());
        pins.pop();
        assert_eq!(worker.stats().live_texture_bytes, payload * 3);
        drop(pins);
        assert_eq!(worker.stats().live_texture_bytes, 0);
    }
}

#[test]
fn nested_transparent_images_propagate_through_validated_operations_and_profiles() {
    for precision in [FloatPrecision::Half, FloatPrecision::Full] {
        let mut project = solid_project();
        project.color.precision = precision;
        project.compositions[0].width = 4;
        project.compositions[0].height = 4;
        project.compositions[0].nodes[0].operation = NodeOperation::Solid {
            rgba: [3., -2., 1., 0.],
        };
        let mut parent = scene(
            vec![
                node(3, NodeOperation::Source { clip: id(40) }),
                node(
                    4,
                    NodeOperation::Transform {
                        image: id(3),
                        translation: [0.5, -2.],
                        scale: [-0.5, 2.],
                        rotation: 45.,
                        opacity: 0.7,
                    },
                ),
                node(5, NodeOperation::Scalar { value: 0.5 }),
                node(
                    6,
                    NodeOperation::Opacity {
                        image: id(4),
                        value: id(5),
                    },
                ),
                node(
                    7,
                    NodeOperation::Solid {
                        rgba: [0.2, 0.4, 0.8, 0.5],
                    },
                ),
                node(
                    8,
                    NodeOperation::Over {
                        foreground: id(6),
                        background: id(7),
                        mask: None,
                    },
                ),
                node(9, NodeOperation::Scalar { value: 1. }),
                node(
                    10,
                    NodeOperation::Opacity {
                        image: id(8),
                        value: id(9),
                    },
                ),
            ],
            10,
        );
        parent.id = id(101);
        parent.tracks = vec![Track {
            id: id(41),
            name: "Nested transparent".into(),
            kind: TrackKind::Video,
            enabled: true,
            clips: vec![Clip {
                id: id(40),
                name: "Different geometry".into(),
                range: FrameRange { start: 0, end: 48 },
                source: ClipSource::Composition {
                    composition: id(100),
                },
                time_map: TimeMap {
                    points: vec![
                        TimePoint {
                            frame: 0,
                            source_tick: 0,
                        },
                        TimePoint {
                            frame: 48,
                            source_tick: 48,
                        },
                    ],
                },
                linked: None,
            }],
        }];
        project.compositions.push(parent);
        let mut worker = renderer(project.clone());
        let frame = worker
            .render(id(101), SourcePosition::new(1, 1).unwrap(), false)
            .unwrap();
        close(
            &worker.readback(&frame).unwrap(),
            &repeated([0.1, 0.2, 0.4, 0.5]),
            0.001,
        );
        worker.finish().unwrap();
        assert_eq!(worker.stats().dispatches, 3);
        assert_eq!(worker.stats().simplified_nodes, 6);
        project.compositions[1].picture = Some(id(6));
        project.color.output_gamut = WorkingGamut::Bt2020;
        project.color.output_transfer = OutputTransfer::Srgb;
        worker
            .rebind(snapshot(project.clone()), Cancellation::new().unwrap())
            .unwrap();
        let blank = worker
            .render(id(101), SourcePosition::new(1, 1).unwrap(), false)
            .unwrap();
        let output = worker.convert(&blank, ImageBoundary::Output).unwrap();
        assert_eq!(
            output.image().interpretation(),
            (precision, WorkingGamut::Bt2020, OutputTransfer::Srgb)
        );
        close(&worker.readback(&output).unwrap(), &repeated([0.; 4]), 0.);
        project.color.output_transfer = OutputTransfer::Pq;
        worker
            .rebind(snapshot(project.clone()), Cancellation::new().unwrap())
            .unwrap();
        let blank = worker
            .render(id(101), SourcePosition::new(1, 1).unwrap(), false)
            .unwrap();
        assert!(worker.convert(&blank, ImageBoundary::Output).is_err());
        project.compositions[1].nodes[1].operation = NodeOperation::Transform {
            image: id(3),
            translation: [0., 0.],
            scale: [1e-310, 1.],
            rotation: 0.,
            opacity: 1.,
        };
        worker
            .rebind(snapshot(project), Cancellation::new().unwrap())
            .unwrap();
        assert!(
            worker
                .render(id(101), SourcePosition::new(1, 1).unwrap(), false)
                .is_err()
        );
    }
}
