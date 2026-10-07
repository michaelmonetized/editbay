use editbay_core::*;
use editbay_media::{Cancellation, PictureBudget};
use editbay_render::{GraphBudget, GraphRenderer};
use std::sync::Arc;
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
fn project(points: &[[f64; 2]], feather: f64, inverted: bool, background: [f64; 4]) -> Project {
    let mut p = Project::new("Procedural mask fixture").unwrap();
    p.compositions = vec![Composition {
        id: id(100),
        name: "Pixel reference".into(),
        width: 8,
        height: 6,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 48,
        tracks: vec![],
        nodes: vec![
            node(
                1,
                NodeOperation::Solid {
                    rgba: [0.8, 0.25, 0.1, 0.5],
                },
            ),
            node(2, NodeOperation::Solid { rgba: background }),
            node(
                3,
                NodeOperation::Polygon {
                    points: points.to_vec(),
                },
            ),
            node(
                4,
                NodeOperation::Mask {
                    geometry: id(3),
                    feather,
                    inverted,
                },
            ),
            node(
                5,
                NodeOperation::Over {
                    foreground: id(1),
                    background: id(2),
                    mask: Some(id(4)),
                },
            ),
        ],
        picture: Some(id(5)),
        audio: None,
    }];
    p
}
fn snapshot(p: Project) -> Arc<EvaluationSnapshot> {
    Arc::new(EvaluationSnapshot::new(Arc::new(p)).unwrap())
}
fn renderer(p: Project, budget: GraphBudget, cancel: Cancellation) -> GraphRenderer {
    GraphRenderer::new(snapshot(p), PictureBudget::default(), budget, cancel).unwrap()
}
fn alpha(points: &[[f64; 2]], at: [f64; 2], feather: f64, inverted: bool) -> f64 {
    let mut crossings = 0;
    let mut distance = f64::INFINITY;
    for (a, b) in points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
    {
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let length = dx.hypot(dy);
        let along = if length == 0. {
            0.
        } else {
            ((at[0] - a[0]) * dx / length + (at[1] - a[1]) * dy / length).clamp(0., length)
        };
        let t = if length == 0. { 0. } else { along / length };
        distance = distance.min((at[0] - a[0] - t * dx).hypot(at[1] - a[1] - t * dy));
        if (a[1] > at[1]) != (b[1] > at[1]) && at[0] < a[0] + (at[1] - a[1]) * dx / dy {
            crossings += 1;
        }
    }
    let signed = if crossings % 2 == 1 {
        distance
    } else {
        -distance
    };
    let t = (0.5 + signed / feather.max(1.)).clamp(0., 1.);
    let alpha = t * t * (3. - 2. * t);
    if inverted { 1. - alpha } else { alpha }
}
fn expected(points: &[[f64; 2]], feather: f64, inverted: bool, background: [f64; 4]) -> Vec<f32> {
    (0..6)
        .flat_map(|y| {
            (0..8).flat_map(move |x| {
                let a = alpha(points, [x as f64 + 0.5, y as f64 + 0.5], feather, inverted);
                let foreground = [0.4 * a, 0.125 * a, 0.05 * a, 0.5 * a];
                std::array::from_fn::<_, 4, _>(|c| {
                    (foreground[c]
                        + if c == 3 {
                            background[c]
                        } else {
                            background[c] * background[3]
                        } * (1. - foreground[3])) as f32
                })
            })
        })
        .collect()
}
fn close(actual: &[f32], expected: &[f32], tolerance: f32) {
    assert_eq!(actual.len(), expected.len());
    for (n, (a, b)) in actual.iter().zip(expected).enumerate() {
        assert!(
            a.is_finite() && (a - b).abs() <= tolerance,
            "channel {n}: {a} != {b}"
        );
    }
}
fn read(worker: &mut GraphRenderer, frame: i64, before: bool) -> Vec<f32> {
    let image = worker
        .render(id(100), SourcePosition::new(frame, 1).unwrap(), before)
        .unwrap();
    worker.readback(&image).unwrap()
}

#[test]
fn actual_gpu_even_odd_feather_inversion_and_premultiplied_over_match_f64_reference() {
    let fixtures = [
        vec![[1., 1.], [7., 1.], [7., 5.], [1., 5.]],
        vec![[1.25, 0.75], [7.25, 4.5], [0.25, 4.75], [6.5, 0.25]],
        vec![[-2., 1.], [4.5, 0.5], [4.5, 0.5], [7., 7.], [2., 4.]],
        vec![[0.5, 0.5], [4.5, 0.5], [7.5, 0.5]],
        vec![[2.5, 2.5]; 3],
        vec![[-10000., -10000.], [10000., -10000.], [0., 10000.]],
    ];
    for (precision, tolerance) in [
        (FloatPrecision::Half, 0.002),
        (FloatPrecision::Full, 0.00002),
    ] {
        let mut worker = renderer(
            project(&fixtures[0], 0., false, [0.; 4]),
            GraphBudget::default(),
            Cancellation::new().unwrap(),
        );
        for points in &fixtures {
            for feather in [0., 1., 3.5] {
                for inverted in [false, true] {
                    for background in [[0.; 4], [0.125, 0.5, 0.75, 0.75]] {
                        let mut p = project(points, feather, inverted, background);
                        p.color.precision = precision;
                        worker
                            .rebind(snapshot(p), Cancellation::new().unwrap())
                            .unwrap();
                        close(
                            &read(&mut worker, 0, false),
                            &expected(points, feather, inverted, background),
                            tolerance,
                        );
                    }
                }
            }
        }
        worker.clear().unwrap();
        assert_eq!(worker.stats().live_texture_bytes, 0);
        assert_eq!(worker.stats().pending_submissions, 0);
    }
}

#[test]
fn mask_time_animation_revision_cache_and_pins_retain_exact_ownership() {
    let points = [[1., 1.], [7., 1.], [7., 5.], [1., 5.]];
    let mut p = project(&points, 0., true, [0.; 4]);
    p.color.precision = FloatPrecision::Full;
    p.compositions[0].nodes[2].range.end = 24;
    p.compositions[0].nodes[3].range.end = 36;
    p.compositions[0].nodes[3].animation = vec![AnimationChannel {
        id: id(60),
        property: AnimatedProperty::Feather,
        interpolation: Interpolation::Step,
        keys: vec![
            Keyframe {
                frame: 0,
                value: 0.,
                in_tangent: 0.,
                out_tangent: 0.,
            },
            Keyframe {
                frame: 12,
                value: 4.,
                in_tangent: 0.,
                out_tangent: 0.,
            },
        ],
    }];
    let cancel = Cancellation::new().unwrap();
    let mut worker = renderer(p.clone(), GraphBudget::default(), cancel.clone());
    close(
        &read(&mut worker, 12, true),
        &expected(&points, 0., true, [0.; 4]),
        0.00002,
    );
    close(
        &read(&mut worker, 12, false),
        &expected(&points, 4., true, [0.; 4]),
        0.00002,
    );
    close(
        &read(&mut worker, 24, false),
        &expected(&[], 4., true, [0.; 4]),
        0.00002,
    );
    close(&read(&mut worker, 36, false), &vec![0.; 8 * 6 * 4], 0.);
    let first = worker
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    let dispatches = worker.stats().dispatches;
    let second = worker
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    assert!(Arc::ptr_eq(first.image(), second.image()));
    assert_eq!(worker.stats().dispatches, dispatches);
    let mut foreign = renderer(
        p.clone(),
        GraphBudget::default(),
        Cancellation::new().unwrap(),
    );
    assert!(foreign.readback(&first).is_err());
    p.rename("Corrected geometry").unwrap();
    p.compositions[0].nodes[2].operation = NodeOperation::Polygon {
        points: vec![[0., 0.], [2., 0.], [2., 2.]],
    };
    let cancel = Cancellation::new().unwrap();
    worker.rebind(snapshot(p), cancel.clone()).unwrap();
    assert!(worker.validate_result(&first).is_err());
    let changed = worker
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    assert!(!Arc::ptr_eq(first.image(), changed.image()));
    drop(second);
    worker.clear().unwrap();
    assert_eq!(worker.stats().live_texture_bytes, 2 * 8 * 6 * 16);
    drop(first);
    drop(changed);
    assert_eq!(worker.stats().live_texture_bytes, 0);
    cancel.cancel();
    assert!(
        worker
            .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
            .is_err()
    );
    worker.clear().unwrap();
    assert_eq!(worker.stats().pending_submissions, 0);
}

#[test]
fn vertex_coordinate_and_pixel_edge_budgets_fail_and_reap_gpu_work() {
    let points = [[1., 1.], [7., 1.], [7., 5.], [1., 5.]];
    for (points, budget, message) in [
        (
            points.to_vec(),
            GraphBudget {
                polygon_points: 3,
                ..GraphBudget::default()
            },
            "vertex budget",
        ),
        (
            points.to_vec(),
            GraphBudget {
                mask_edge_tests: 191,
                ..GraphBudget::default()
            },
            "pixel-edge budget",
        ),
        (
            vec![[1_000_001., 0.], [0., 0.], [0., 1.]],
            GraphBudget::default(),
            "coordinate bounds",
        ),
    ] {
        let mut worker = renderer(
            project(&points, 0., false, [0.; 4]),
            budget,
            Cancellation::new().unwrap(),
        );
        let error = worker
            .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
            .err()
            .unwrap();
        assert!(error.to_string().contains(message), "{error}");
        worker.clear().unwrap();
        assert_eq!(worker.stats().live_texture_bytes, 0);
        assert_eq!(worker.stats().pending_submissions, 0);
    }
    for budget in [
        GraphBudget {
            polygon_points: 4097,
            ..GraphBudget::default()
        },
        GraphBudget {
            mask_edge_tests: 0,
            ..GraphBudget::default()
        },
    ] {
        assert!(budget.validate().is_err());
    }
}

#[test]
fn nested_reverse_masks_share_the_total_request_budget() {
    let points = [[1., 1.], [7., 1.], [7., 5.], [1., 5.]];
    let mut p = project(&points, 0., false, [0.; 4]);
    p.color.precision = FloatPrecision::Full;
    p.compositions[0].nodes[3].animation = vec![AnimationChannel {
        id: id(60),
        property: AnimatedProperty::Feather,
        interpolation: Interpolation::Step,
        keys: vec![
            Keyframe {
                frame: 0,
                value: 0.,
                in_tangent: 0.,
                out_tangent: 0.,
            },
            Keyframe {
                frame: 12,
                value: 4.,
                in_tangent: 0.,
                out_tangent: 0.,
            },
        ],
    }];
    let tracks = (0..2)
        .map(|n| Track {
            id: id(20 + n * 2),
            name: "Nested mask".into(),
            kind: TrackKind::Video,
            enabled: true,
            clips: vec![Clip {
                id: id(21 + n * 2),
                name: "Retimed mask".into(),
                range: FrameRange { start: 0, end: 24 },
                source: ClipSource::Composition {
                    composition: id(100),
                },
                linked: None,
                time_map: TimeMap {
                    source_denominator: 1,
                    points: vec![
                        TimePoint {
                            frame: 0,
                            source_tick: if n == 0 { 24 } else { 0 },
                        },
                        TimePoint {
                            frame: 24,
                            source_tick: if n == 0 { 0 } else { 24 },
                        },
                    ],
                },
            }],
        })
        .collect();
    p.compositions.push(Composition {
        id: id(101),
        name: "Two nested masks".into(),
        width: 8,
        height: 6,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 48,
        tracks,
        nodes: vec![
            node(10, NodeOperation::Source { clip: id(21) }),
            node(11, NodeOperation::Source { clip: id(23) }),
            node(
                12,
                NodeOperation::Over {
                    foreground: id(10),
                    background: id(11),
                    mask: None,
                },
            ),
        ],
        picture: Some(id(12)),
        audio: None,
    });
    let mut bounded = renderer(
        p.clone(),
        GraphBudget {
            mask_edge_tests: 383,
            ..GraphBudget::default()
        },
        Cancellation::new().unwrap(),
    );
    let error = bounded
        .render(id(101), SourcePosition::new(12, 1).unwrap(), false)
        .err()
        .unwrap();
    assert!(error.to_string().contains("pixel-edge budget"), "{error}");
    bounded.clear().unwrap();
    assert_eq!(bounded.stats().live_texture_bytes, 0);
    let mut worker = renderer(
        p,
        GraphBudget {
            mask_edge_tests: 384,
            ..GraphBudget::default()
        },
        Cancellation::new().unwrap(),
    );
    let image = worker
        .render(id(101), SourcePosition::new(12, 1).unwrap(), false)
        .unwrap();
    let foreground = expected(&points, 0., false, [0.; 4]);
    let background = expected(&points, 4., false, [0.; 4]);
    let expected: Vec<_> = foreground
        .as_chunks::<4>()
        .0
        .iter()
        .zip(background.as_chunks::<4>().0.iter())
        .flat_map(|(f, b)| (0..4).map(move |c| f[c] + b[c] * (1. - f[3])))
        .collect();
    close(&worker.readback(&image).unwrap(), &expected, 0.00002);
    let dispatches = worker.stats().dispatches;
    worker
        .render(id(101), SourcePosition::new(12, 1).unwrap(), false)
        .unwrap();
    assert_eq!(worker.stats().dispatches, dispatches);
}

#[test]
fn asset_masks_fail_before_any_gpu_work_until_their_format_is_supported() {
    let mut p = project(&[[0., 0.], [2., 0.], [2., 2.]], 0., false, [0.; 4]);
    p.assets.push(AssetReference {
        id: id(90),
        kind: AssetKind::Mask,
        path: "Unimplemented format.exr".into(),
        sha256: "b".repeat(64),
        bytes: 1024,
        provenance: "Unsupported format fixture".into(),
    });
    p.compositions[0].nodes[3].operation = NodeOperation::MaskAsset { asset: id(90) };
    let mut worker = renderer(p, GraphBudget::default(), Cancellation::new().unwrap());
    let error = worker
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .err()
        .unwrap();
    assert!(error.to_string().contains("asset-mask"), "{error}");
    assert_eq!(worker.stats().dispatches, 0);
    assert_eq!(worker.stats().live_texture_bytes, 0);
}

#[test]
fn cancelling_submitted_mask_work_retires_gpu_pins_and_preserves_consumer_charge() {
    let points: Vec<_> = (0..60)
        .map(|n| {
            let angle = n as f64 * std::f64::consts::TAU / 60.;
            [512. + 480. * angle.cos(), 512. + 480. * angle.sin()]
        })
        .collect();
    let mut p = project(&points, 8., false, [0.; 4]);
    p.color.precision = FloatPrecision::Full;
    p.compositions[0].width = 1024;
    p.compositions[0].height = 1024;
    let cancel = Cancellation::new().unwrap();
    let mut worker = renderer(
        p,
        GraphBudget {
            cache_entries: 0,
            ..GraphBudget::default()
        },
        cancel.clone(),
    );
    let image = worker
        .render(id(100), SourcePosition::new(0, 1).unwrap(), false)
        .unwrap();
    let began = std::time::Instant::now();
    cancel.cancel();
    assert!(worker.validate_result(&image).is_err());
    worker.clear().unwrap();
    assert!(began.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(worker.stats().pending_submissions, 0);
    assert_eq!(worker.stats().live_texture_bytes, 1024 * 1024 * 16);
    drop(image);
    assert_eq!(worker.stats().live_texture_bytes, 0);
}
