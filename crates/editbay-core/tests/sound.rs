use editbay_core::*;
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn map(duration: u64, start: i64, end: i64) -> TimeMap {
    TimeMap {
        points: vec![
            TimePoint {
                frame: 0,
                source_tick: start,
            },
            TimePoint {
                frame: duration,
                source_tick: end,
            },
        ],
    }
}
fn node(n: u128, operation: NodeOperation) -> TimedNode {
    TimedNode {
        id: id(n),
        range: FrameRange { start: 0, end: 48 },
        operation,
        animation: vec![],
    }
}
fn project() -> Project {
    let mut p = Project::new("Sound math").unwrap();
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
        name: "Original channels".into(),
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
        name: "Sound".into(),
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
                name: "Natural sound".into(),
                range: FrameRange { start: 0, end: 48 },
                source: ClipSource::Media {
                    source: id(20),
                    stream: 0,
                },
                time_map: map(48, 0, 96000),
                linked: None,
            }],
        }],
        nodes: vec![node(50, NodeOperation::Source { clip: id(41) })],
        picture: None,
        audio: Some(id(50)),
    });
    p
}
fn sound(p: Project, rate: u32, budget: SoundBudget) -> SoundSnapshot {
    SoundSnapshot::new(
        Arc::new(EvaluationSnapshot::new(Arc::new(p)).unwrap()),
        id(30),
        SoundProfile {
            sample_rate: rate,
            channels: vec!["FL".into(), "FR".into()],
        },
        budget,
    )
    .unwrap()
}

#[test]
fn exact_sample_centers_rate_conversion_reverse_and_final_sample() {
    let normal = sound(project(), 48000, SoundBudget::default());
    assert_eq!(normal.duration_samples(), 96000);
    for start in [0, 1999, 47999, 95999] {
        let block = normal.prepare(start, 1).unwrap();
        let sample = block.sources()[0].samples[0].unwrap();
        assert_eq!(
            sample.center,
            SourcePosition::new((2 * start + 1) as i64, 2).unwrap()
        );
        assert_eq!(sample.step, 1.);
    }
    let converted = sound(project(), 44100, SoundBudget::default())
        .prepare(0, 4096)
        .unwrap();
    for (n, sample) in converted.sources()[0].samples.iter().enumerate() {
        assert_eq!(
            sample.unwrap().center,
            SourcePosition::new((2 * n as i64 + 1) * 80, 147).unwrap()
        );
    }
    let mut p = project();
    p.compositions[0].tracks[0].clips[0].time_map = map(48, 96000, 0);
    let reversed = sound(p, 48000, SoundBudget::default())
        .prepare(0, 4096)
        .unwrap();
    for (n, sample) in reversed.sources()[0].samples.iter().enumerate() {
        let sample = sample.unwrap();
        assert_eq!(
            sample.center,
            SourcePosition::new(191999 - 2 * n as i64, 2).unwrap()
        );
        assert!(sample.reverse);
        assert_eq!(sample.step, -1.);
    }
}

#[test]
fn device_rate_uses_only_reachable_channels_and_rejects_implicit_mixing() {
    let mut p = project();
    let mut unrelated = p.sources[0].clone();
    unrelated.id = id(21);
    if let StreamFormat::Audio { channels, .. } = &mut unrelated.streams[0].format {
        *channels = vec!["FC".into()];
    }
    p.sources.insert(0, unrelated);
    let compile = |p: Project| {
        SoundSnapshot::at_output_rate(
            Arc::new(EvaluationSnapshot::new(Arc::new(p)).unwrap()),
            id(30),
            44100,
            SoundBudget::default(),
        )
    };
    let snapshot = compile(p.clone()).unwrap();
    assert_eq!(snapshot.profile().channels, ["FL", "FR"]);
    assert_eq!(snapshot.profile().sample_rate, 44100);
    assert_eq!(snapshot.duration_samples(), 88200);
    assert_eq!(
        snapshot.prepare(0, 1).unwrap().sources()[0].samples[0]
            .unwrap()
            .center,
        SourcePosition::new(80, 147).unwrap()
    );
    p.compositions[0].tracks[0].clips[0].source = ClipSource::Media {
        source: id(21),
        stream: 0,
    };
    assert_eq!(compile(p.clone()).unwrap().profile().channels, ["FC"]);
    let mut other = p.compositions[0].tracks[0].clips[0].clone();
    other.id = id(43);
    other.source = ClipSource::Media {
        source: id(20),
        stream: 0,
    };
    p.compositions[0].tracks[0].clips.push(other);
    p.compositions[0]
        .nodes
        .push(node(52, NodeOperation::Source { clip: id(43) }));
    p.compositions[0].nodes.push(node(
        53,
        NodeOperation::Mix {
            inputs: vec![id(50), id(52)],
        },
    ));
    p.compositions[0].audio = Some(id(53));
    assert!(
        compile(p.clone())
            .err()
            .unwrap()
            .to_string()
            .contains("routing")
    );
    p.compositions[0].audio = None;
    let silent = compile(p).unwrap();
    assert_eq!(silent.profile().channels, ["FL", "FR"]);
    assert!(silent.prepare(0, 4096).unwrap().sources().is_empty());
}

#[test]
fn cuts_inactive_tracks_and_freeze_are_explicit_silence() {
    let mut p = project();
    let clip = &mut p.compositions[0].tracks[0].clips[0];
    clip.range.start = 24;
    clip.time_map = map(24, 48000, 96000);
    let s = sound(p.clone(), 48000, SoundBudget::default());
    let block = s.prepare(47999, 2).unwrap();
    assert!(block.sources()[0].samples[0].is_none());
    assert_eq!(
        block.sources()[0].samples[1].unwrap().center,
        SourcePosition::new(96001, 2).unwrap()
    );
    p.compositions[0].tracks[0].enabled = false;
    assert!(
        sound(p.clone(), 48000, SoundBudget::default())
            .prepare(48000, 32)
            .unwrap()
            .sources()[0]
            .samples
            .iter()
            .all(Option::is_none)
    );
    p.compositions[0].tracks[0].enabled = true;
    p.compositions[0].tracks[0].clips[0].time_map = map(24, 48000, 48000);
    assert!(
        sound(p, 48000, SoundBudget::default())
            .prepare(48000, 32)
            .unwrap()
            .sources()[0]
            .samples
            .iter()
            .all(Option::is_none)
    );
}

#[test]
fn gain_interpolation_mix_and_nested_rates_match_closed_form() {
    for interpolation in [
        Interpolation::Step,
        Interpolation::Linear,
        Interpolation::Hermite,
    ] {
        let mut p = project();
        let mut gain = node(
            51,
            NodeOperation::Gain {
                audio: id(50),
                gain: 0.25,
            },
        );
        gain.animation.push(AnimationChannel {
            id: id(60),
            property: AnimatedProperty::Gain,
            interpolation,
            keys: vec![
                Keyframe {
                    frame: 0,
                    value: 0.,
                    in_tangent: 0.,
                    out_tangent: 0.,
                },
                Keyframe {
                    frame: 48,
                    value: 1.,
                    in_tangent: 0.,
                    out_tangent: 0.,
                },
            ],
        });
        p.compositions[0].nodes.extend([
            gain,
            node(
                52,
                NodeOperation::Mix {
                    inputs: vec![id(50), id(51)],
                },
            ),
        ]);
        p.compositions[0].audio = Some(id(52));
        let block = sound(p, 48000, SoundBudget::default())
            .prepare(24000, 64)
            .unwrap();
        for n in 0..64 {
            let t = (24000.5 + n as f64) / 96000.;
            let expected = match interpolation {
                Interpolation::Step => 0.,
                Interpolation::Linear => t,
                Interpolation::Hermite => 3. * t * t - 2. * t * t * t,
            };
            assert_eq!(block.sources()[0].samples[n].unwrap().gain, 1.);
            assert!((block.sources()[1].samples[n].unwrap().gain - expected).abs() < 1e-12);
        }
    }
    let mut p = project();
    let mut child = p.compositions[0].clone();
    child.id = id(31);
    child.frame_rate = FrameRate::new(60, 1).unwrap();
    child.duration = 120;
    child.tracks[0].id = id(42);
    child.tracks[0].clips[0].id = id(43);
    child.tracks[0].clips[0].range.end = 120;
    child.tracks[0].clips[0].time_map = map(120, 96000, 0);
    child.nodes[0] = TimedNode {
        id: id(53),
        range: FrameRange { start: 0, end: 120 },
        operation: NodeOperation::Source { clip: id(43) },
        animation: vec![],
    };
    child.audio = Some(id(53));
    p.compositions[0].tracks[0].clips[0].source = ClipSource::Composition {
        composition: id(31),
    };
    p.compositions[0].tracks[0].clips[0].time_map = map(48, 0, 120);
    p.compositions.push(child);
    let block = sound(p, 48000, SoundBudget::default())
        .prepare(47999, 2)
        .unwrap();
    assert_eq!(
        block.sources()[0].samples[0].unwrap().center,
        SourcePosition::new(96001, 2).unwrap()
    );
    assert_eq!(
        block.sources()[0].samples[1].unwrap().center,
        SourcePosition::new(95999, 2).unwrap()
    );
    assert_eq!(block.sources()[0].samples[0].unwrap().step, -1.);
}

#[test]
fn identical_public_versions_cannot_cross_private_owners_and_labels_do_not_change_content() {
    let p = project();
    let first = sound(p.clone(), 48000, SoundBudget::default());
    let foreign = sound(p.clone(), 48000, SoundBudget::default());
    let block = first.prepare(0, 32).unwrap();
    assert!(first.validate_plan(&block).is_ok());
    assert!(foreign.validate_plan(&block).is_err());
    let mut renamed = p;
    renamed.name = "Changed label".into();
    renamed.revision += 1;
    renamed.compositions[0].name = "Changed scene".into();
    renamed.compositions[0].tracks[0].clips[0].name = "Changed clip".into();
    assert_eq!(
        block.sha256(),
        sound(renamed, 48000, SoundBudget::default())
            .prepare(0, 32)
            .unwrap()
            .sha256()
    );
}

#[test]
fn limits_reject_unbounded_work_and_undeclared_channel_conversion() {
    let p = project();
    for budget in [
        SoundBudget {
            positions: 3,
            ..SoundBudget::default()
        },
        SoundBudget {
            operations: 7,
            ..SoundBudget::default()
        },
        SoundBudget {
            block_frames: 3,
            ..SoundBudget::default()
        },
    ] {
        assert!(sound(p.clone(), 48000, budget).prepare(0, 4).is_err());
    }
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(p)).unwrap());
    assert!(
        SoundSnapshot::new(
            snapshot.clone(),
            id(30),
            SoundProfile {
                sample_rate: 48000,
                channels: vec!["FC".into()]
            },
            SoundBudget::default()
        )
        .is_err()
    );
    assert!(
        SoundSnapshot::new(
            snapshot,
            id(30),
            SoundProfile {
                sample_rate: 48000,
                channels: vec!["FL".into(), "FR".into()]
            },
            SoundBudget {
                path_steps: 1,
                ..SoundBudget::default()
            }
        )
        .is_err()
    );
    let s = sound(project(), 48000, SoundBudget::default());
    assert!(s.prepare(u64::MAX, 1).is_err());
    assert!(s.prepare(96000, 1).is_err());
    assert!(s.prepare(0, 0).is_err());
}

#[test]
fn rational_conversion_reduces_before_narrowing_and_normalizes_extreme_zero() {
    let base = TimeBase {
        numerator: u32::MAX,
        denominator: u32::MAX,
    };
    let rate = FrameRate {
        numerator: u32::MAX,
        denominator: u32::MAX,
    };
    assert_eq!(
        base.at_rate(
            SourcePosition {
                numerator: 0,
                denominator: u64::MAX
            },
            rate
        )
        .unwrap(),
        SourcePosition::new(0, 1).unwrap()
    );
    for n in [i64::MIN, -1, 1, i64::MAX] {
        assert_eq!(
            base.at_rate(SourcePosition::new(n, 1).unwrap(), rate)
                .unwrap(),
            SourcePosition::new(n, 1).unwrap()
        );
    }
    assert!(
        base.at_rate(
            SourcePosition {
                numerator: 0,
                denominator: 0
            },
            rate
        )
        .is_err()
    );
    assert!(
        TimeBase {
            numerator: u32::MAX,
            denominator: 1
        }
        .at_rate(
            SourcePosition::new(i64::MAX, 1).unwrap(),
            FrameRate::new(u32::MAX, 1).unwrap()
        )
        .is_err()
    );
}

#[test]
fn finite_gain_overflow_and_compiled_leaf_explosion_are_rejected() {
    let mut p = project();
    for n in 51..=76 {
        p.compositions[0].nodes.push(node(
            n,
            NodeOperation::Gain {
                audio: id(n - 1),
                gain: 1e12,
            },
        ));
    }
    p.compositions[0].audio = Some(id(76));
    assert!(
        sound(p, 48000, SoundBudget::default())
            .prepare(0, 1)
            .is_err()
    );
    let mut p = project();
    p.compositions[0].nodes.extend([
        node(
            51,
            NodeOperation::Gain {
                audio: id(50),
                gain: 0.25,
            },
        ),
        node(
            52,
            NodeOperation::Mix {
                inputs: vec![id(50), id(51)],
            },
        ),
    ]);
    p.compositions[0].audio = Some(id(52));
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(p)).unwrap());
    assert!(
        SoundSnapshot::new(
            snapshot,
            id(30),
            SoundProfile {
                sample_rate: 48000,
                channels: vec!["FL".into(), "FR".into()]
            },
            SoundBudget {
                leaves: 1,
                ..SoundBudget::default()
            }
        )
        .is_err()
    );
}

#[test]
fn fractional_composition_tail_is_silence_beyond_the_exact_duration() {
    let mut p = project();
    let scene = &mut p.compositions[0];
    scene.duration = 1;
    scene.frame_rate = FrameRate::new(30000, 1001).unwrap();
    scene.nodes[0].range.end = 1;
    scene.tracks[0].clips[0].range.end = 1;
    scene.tracks[0].clips[0].time_map = map(1, 0, 1600);
    let compiled = sound(p, 44100, SoundBudget::default());
    assert_eq!(compiled.duration_samples(), 1472);
    let tail = compiled.prepare(1470, 2).unwrap();
    assert!(tail.sources()[0].samples[0].is_some());
    assert!(tail.sources()[0].samples[1].is_none());
}
