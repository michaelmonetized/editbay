use editbay_audio::{SoundRenderBudget, SoundRenderer};
use editbay_core::*;
use editbay_media::{Cancellation, PcmBudget, SourceFile};
use std::{path::Path, process::Command, sync::Arc};
use uuid::Uuid;
fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn fixture(path: &Path, frequency: u32) -> Project {
    let expression = format!(
        "aevalsrc=1.25*sin(2*PI*{frequency}*t)|0.75*cos(2*PI*{frequency}*t):s=48000:d=4:c=stereo"
    );
    let result = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            &expression,
            "-c:a",
            "pcm_f32le",
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
    let ingested = SourceFile::open(path, &cancel)
        .unwrap()
        .ingest("Float stereo".into(), &[0], cancel, |_, _| {})
        .unwrap();
    let mut p = Project::new("Shared sound graph").unwrap();
    let source = ingested.source.id;
    p.assets.push(ingested.asset);
    p.sources.push(ingested.source);
    p.compositions.push(Composition {
        id: id(30),
        name: "Sound".into(),
        width: 16,
        height: 16,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 96,
        tracks: vec![Track {
            id: id(40),
            name: "Audio".into(),
            kind: TrackKind::Audio,
            enabled: true,
            clips: vec![Clip {
                id: id(41),
                name: "Natural sound".into(),
                range: FrameRange { start: 0, end: 96 },
                source: ClipSource::Media { source, stream: 0 },
                time_map: TimeMap {
                    points: vec![
                        TimePoint {
                            frame: 0,
                            source_tick: 0,
                        },
                        TimePoint {
                            frame: 96,
                            source_tick: 192000,
                        },
                    ],
                },
                linked: None,
            }],
        }],
        nodes: vec![TimedNode {
            id: id(50),
            range: FrameRange { start: 0, end: 96 },
            operation: NodeOperation::Source { clip: id(41) },
            animation: vec![],
        }],
        picture: None,
        audio: Some(id(50)),
    });
    p
}
fn renderer(
    p: Project,
    rate: u32,
    budget: SoundRenderBudget,
) -> (Arc<SoundSnapshot>, SoundRenderer) {
    let StreamFormat::Audio { channels, .. } = &p.sources[0].streams[0].format else {
        panic!("fixture is not sound");
    };
    let channels = channels.clone();
    let snapshot = Arc::new(
        SoundSnapshot::new(
            Arc::new(EvaluationSnapshot::new(Arc::new(p)).unwrap()),
            id(30),
            SoundProfile {
                sample_rate: rate,
                channels,
            },
            SoundBudget::default(),
        )
        .unwrap(),
    );
    let render = SoundRenderer::new(
        snapshot.clone(),
        PcmBudget::default(),
        budget,
        Cancellation::new().unwrap(),
    )
    .unwrap();
    (snapshot, render)
}
fn reference(path: &Path) -> Vec<f32> {
    let result = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "f32le", "pipe:1"])
        .output()
        .unwrap();
    assert!(result.status.success());
    result
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect()
}

#[test]
fn late_resampled_aac_matches_full_independent_decode_across_overlapping_reads() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("original.wav");
    let captured = fixture(&original, 997);
    let aac = directory.path().join("encoded.m4a");
    let decoded = directory.path().join("decoded.wav");
    for (input, output, codec) in [(&original, &aac, "aac"), (&aac, &decoded, "pcm_f32le")] {
        let result = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(input)
            .args(["-c:a", codec])
            .arg(output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let with_source = |path: &Path| {
        let cancel = Cancellation::new().unwrap();
        let mut imported = SourceFile::open(path, &cancel)
            .unwrap()
            .ingest("Decoded history".into(), &[0], cancel, |_, _| {})
            .unwrap();
        let mut project = captured.clone();
        imported.asset.id = project.assets[0].id;
        imported.source.id = project.sources[0].id;
        imported.source.asset = imported.asset.id;
        imported.source.streams[0].duration_ticks = Some(192000);
        project.assets[0] = imported.asset;
        project.sources[0] = imported.source;
        project
    };
    let (aac_plan, mut aac_renderer) =
        renderer(with_source(&aac), 44100, SoundRenderBudget::default());
    let (decoded_plan, mut decoded_renderer) =
        renderer(with_source(&decoded), 44100, SoundRenderBudget::default());
    for first in [147000, 145000, 126000, 128000, 127500, 170000, 0] {
        let actual = aac_renderer
            .render(&aac_plan.prepare(first, 4096).unwrap())
            .unwrap();
        let expected = decoded_renderer
            .render(&decoded_plan.prepare(first, 4096).unwrap())
            .unwrap();
        assert!(
            actual
                .sound()
                .samples()
                .iter()
                .zip(expected.sound().samples())
                .all(|(actual, expected)| actual.to_bits() == expected.to_bits()),
            "late block {first}"
        );
    }
}
#[test]
fn unchanged_samples_reverse_and_mix_preserve_original_channel_order_and_headroom() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sound.wav");
    let p = fixture(&path, 997);
    let expected = reference(&path);
    let (s, mut r) = renderer(p.clone(), 48000, SoundRenderBudget::default());
    for start in [0, 47999, 160000] {
        let result = r.render(&s.prepare(start, 4096).unwrap()).unwrap();
        assert_eq!(
            result.sound().samples(),
            &expected[start as usize * 2..(start as usize + 4096) * 2]
        );
        r.validate_result(&result).unwrap();
    }
    let mut reversed = p.clone();
    reversed.compositions[0].tracks[0].clips[0]
        .time_map
        .points
        .reverse();
    reversed.compositions[0].tracks[0].clips[0].time_map.points[0].frame = 0;
    reversed.compositions[0].tracks[0].clips[0].time_map.points[1].frame = 96;
    let (s, mut r) = renderer(reversed, 48000, SoundRenderBudget::default());
    let result = r.render(&s.prepare(0, 4096).unwrap()).unwrap();
    for n in 0..4096 {
        assert_eq!(
            &result.sound().samples()[n * 2..n * 2 + 2],
            &expected[(191999 - n) * 2..(192000 - n) * 2]
        );
    }
    let mut mixed = p;
    mixed.compositions[0].nodes.extend([
        TimedNode {
            id: id(51),
            range: FrameRange { start: 0, end: 96 },
            operation: NodeOperation::Gain {
                audio: id(50),
                gain: 0.25,
            },
            animation: vec![],
        },
        TimedNode {
            id: id(52),
            range: FrameRange { start: 0, end: 96 },
            operation: NodeOperation::Mix {
                inputs: vec![id(50), id(51)],
            },
            animation: vec![],
        },
    ]);
    mixed.compositions[0].audio = Some(id(52));
    let (s, mut r) = renderer(mixed, 48000, SoundRenderBudget::default());
    let result = r.render(&s.prepare(1024, 4096).unwrap()).unwrap();
    for (n, actual) in result.sound().samples().iter().enumerate() {
        assert_eq!(*actual, (f64::from(expected[2048 + n]) * 1.25) as f32);
    }
    assert!(result.sound().samples().iter().any(|v| v.abs() > 1.));
    r.verify_sources().unwrap();
}
#[test]
fn bandlimited_resampling_matches_analytic_passband_and_rejects_aliases() {
    let dir = tempfile::tempdir().unwrap();
    for frequency in [1000, 18000] {
        let p = fixture(&dir.path().join(format!("{frequency}.wav")), frequency);
        for rate in [44100, 24000] {
            let (s, mut r) = renderer(p.clone(), rate, SoundRenderBudget::default());
            let result = r.render(&s.prepare(2048, 4096).unwrap()).unwrap();
            let mut maximum = 0f64;
            for (n, sample) in result
                .sound()
                .samples()
                .as_chunks::<2>()
                .0
                .iter()
                .enumerate()
            {
                let coordinate = (2048.5 + n as f64) * 48000. / rate as f64 - 0.5;
                let phase = 2. * std::f64::consts::PI * frequency as f64 * coordinate / 48000.;
                let expected = if frequency == 18000 && rate == 24000 {
                    [0., 0.]
                } else {
                    [1.25 * phase.sin(), 0.75 * phase.cos()]
                };
                for c in 0..2 {
                    maximum = maximum.max((f64::from(sample[c]) - expected[c]).abs());
                }
            }
            assert!(maximum < 1e-4, "{frequency}Hz -> {rate}: {maximum}");
        }
    }
}
#[test]
fn sound_pins_foreign_owners_and_work_budgets_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let p = fixture(&dir.path().join("sound.wav"), 997);
    let budget = SoundRenderBudget {
        live_bytes: 256,
        ..SoundRenderBudget::default()
    };
    let (s, mut r) = renderer(p.clone(), 48000, budget);
    let plan = s.prepare(0, 32).unwrap();
    let a = r.render(&plan).unwrap();
    assert_eq!(r.live_bytes(), 256);
    assert!(r.render(&plan).is_err());
    let (foreign, mut other) = renderer(p.clone(), 48000, SoundRenderBudget::default());
    assert!(other.render(&plan).is_err());
    assert!(other.validate_result(&a).is_err());
    assert!(r.render(&foreign.prepare(0, 32).unwrap()).is_err());
    r.clear();
    assert!(r.validate_result(&a).is_err());
    assert_eq!(r.live_bytes(), 256);
    drop(a);
    assert_eq!(r.live_bytes(), 0);
    let (s, mut r) = renderer(
        p,
        44100,
        SoundRenderBudget {
            operations: 16,
            ..SoundRenderBudget::default()
        },
    );
    assert!(r.render(&s.prepare(1024, 32).unwrap()).is_err());
    assert_eq!(r.live_bytes(), 0);
    assert_eq!(r.pcm_stats().decoded_frames, 0);
}

#[test]
fn cached_and_evicted_kernels_preserve_uncached_pcm_bits_and_release_storage() {
    let dir = tempfile::tempdir().unwrap();
    let original = fixture(&dir.path().join("kernels.wav"), 997);
    for mode in ["forward", "reverse", "gain", "freeze"] {
        let mut project = original.clone();
        match mode {
            "reverse" => {
                project.compositions[0].tracks[0].clips[0].time_map.points[0].source_tick = 192000;
                project.compositions[0].tracks[0].clips[0].time_map.points[1].source_tick = 0;
            }
            "gain" => {
                project.compositions[0].nodes.push(TimedNode {
                    id: id(51),
                    range: FrameRange { start: 0, end: 96 },
                    operation: NodeOperation::Gain {
                        audio: id(50),
                        gain: 1.25,
                    },
                    animation: vec![],
                });
                project.compositions[0].audio = Some(id(51));
            }
            "freeze" => {
                project.compositions[0].tracks[0].clips[0].time_map.points[1].source_tick = 0
            }
            _ => {}
        }
        for rate in [44100, 24000, 96000] {
            let (a, mut cached) = renderer(project.clone(), rate, SoundRenderBudget::default());
            let (b, mut uncached) = renderer(
                project.clone(),
                rate,
                SoundRenderBudget {
                    kernel_bytes: 0,
                    ..SoundRenderBudget::default()
                },
            );
            let tiny = SoundRenderBudget {
                kernel_entries: 1,
                kernel_bytes: 4096,
                ..SoundRenderBudget::default()
            };
            let (c, mut evicted) = renderer(project.clone(), rate, tiny);
            for first in [2048, 4096, 2048, 12000] {
                let expected = uncached.render(&b.prepare(first, 512).unwrap()).unwrap();
                let actual = cached.render(&a.prepare(first, 512).unwrap()).unwrap();
                let limited = evicted.render(&c.prepare(first, 512).unwrap()).unwrap();
                for (i, sample) in expected.sound().samples().iter().enumerate() {
                    assert_eq!(
                        sample.to_bits(),
                        actual.sound().samples()[i].to_bits(),
                        "{mode} {rate} {first} {i}"
                    );
                    assert_eq!(
                        sample.to_bits(),
                        limited.sound().samples()[i].to_bits(),
                        "evicted {mode} {rate} {first} {i}"
                    );
                }
                let stats = cached.kernel_stats();
                assert!(stats.entries <= 256);
                assert!(stats.metadata_bytes + stats.coefficient_bytes <= 512 * 1024);
                let stats = evicted.kernel_stats();
                assert!(stats.entries <= 1);
                assert!(stats.metadata_bytes + stats.coefficient_bytes <= tiny.kernel_bytes);
            }
            assert_eq!(uncached.kernel_stats().entries, 0);
            if mode != "freeze" {
                assert!(cached.kernel_stats().hits > 0);
            }
            if mode != "freeze" && rate != 24000 {
                assert!(evicted.kernel_stats().evictions > 0);
            }
            let held = cached.render(&a.prepare(2048, 512).unwrap()).unwrap();
            cached.clear();
            assert_eq!(cached.kernel_stats().entries, 0);
            assert_eq!(cached.kernel_stats().metadata_bytes, 0);
            assert_eq!(cached.kernel_stats().coefficient_bytes, 0);
            assert_eq!(held.sound().samples().len(), 1024);
            assert!(cached.validate_result(&held).is_err());
        }
    }
}

#[test]
fn source_preparation_covers_future_reverse_cuts_without_decoding_during_render() {
    let directory = tempfile::tempdir().unwrap();
    let mut project = fixture(&directory.path().join("future.wav"), 997);
    let mut second = project.sources[0].clone();
    second.id = id(21);
    project.sources.push(second);
    let scene = &mut project.compositions[0];
    let mut clip = scene.tracks[0].clips[0].clone();
    scene.tracks[0].clips[0].range.end = 48;
    scene.tracks[0].clips[0].time_map.points[1] = TimePoint {
        frame: 48,
        source_tick: 48000,
    };
    scene.nodes[0].range.end = 48;
    clip.id = id(42);
    clip.range = FrameRange { start: 48, end: 96 };
    clip.source = ClipSource::Media {
        source: id(21),
        stream: 0,
    };
    clip.time_map.points = vec![
        TimePoint {
            frame: 0,
            source_tick: 144000,
        },
        TimePoint {
            frame: 48,
            source_tick: 96000,
        },
    ];
    scene.tracks[0].clips.push(clip);
    scene.nodes.extend([
        TimedNode {
            id: id(51),
            range: FrameRange { start: 48, end: 96 },
            operation: NodeOperation::Source { clip: id(42) },
            animation: vec![],
        },
        TimedNode {
            id: id(52),
            range: FrameRange { start: 0, end: 96 },
            operation: NodeOperation::Mix {
                inputs: vec![id(50), id(51)],
            },
            animation: vec![],
        },
    ]);
    scene.audio = Some(id(52));
    let (snapshot, mut comparison) = renderer(project, 44100, SoundRenderBudget::default());
    let cancel = Cancellation::new().unwrap();
    let mut renderer = SoundRenderer::new(
        snapshot.clone(),
        PcmBudget {
            decode_frames: 65536,
            decoder_handles: 1,
            cache_entries: 0,
            ..PcmBudget::default()
        },
        SoundRenderBudget::default(),
        cancel.clone(),
    )
    .unwrap();
    let mut preparation = renderer
        .source_preparation(0, snapshot.duration_samples())
        .unwrap();
    let initial = preparation.progress();
    assert_eq!(initial.total_sources, 2);
    assert_eq!(renderer.pcm_stats().decoded_frames, 0);
    assert!(comparison.prepare_sources_step(&mut preparation).is_err());
    let mut steps = 0;
    let mut previous = 0;
    while !preparation.progress().ready {
        let decoded = renderer.pcm_stats().decoded_frames;
        let progress = renderer.prepare_sources_step(&mut preparation).unwrap();
        assert!(renderer.pcm_stats().decoded_frames - decoded <= 65536);
        assert!(progress.prepared_samples > previous);
        previous = progress.prepared_samples;
        steps += 1;
    }
    assert!(steps > 2);
    assert_eq!(previous, initial.total_samples);
    let prepared = renderer.pcm_stats();
    assert_eq!(prepared.decoders, 1);
    assert_eq!(prepared.stores, 2);
    for index in (0..snapshot.duration_samples().div_ceil(4096)).rev() {
        let start = index * 4096;
        let plan = snapshot
            .prepare(
                start,
                (snapshot.duration_samples() - start).min(4096) as u32,
            )
            .unwrap();
        let actual = renderer.render(&plan).unwrap();
        let expected = comparison.render(&plan).unwrap();
        assert!(
            actual
                .sound()
                .samples()
                .iter()
                .zip(expected.sound().samples())
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        assert_eq!(renderer.pcm_stats().decoded_frames, prepared.decoded_frames);
        assert_eq!(renderer.pcm_stats().store_bytes, prepared.store_bytes);
    }
    renderer.clear();
    assert!(renderer.prepare_sources_step(&mut preparation).is_err());
    let mut fresh = renderer
        .source_preparation(0, snapshot.duration_samples())
        .unwrap();
    cancel.cancel();
    assert!(matches!(
        renderer.prepare_sources_step(&mut fresh),
        Err(editbay_media::Error::Cancelled)
    ));
    let limited = SoundRenderer::new(
        snapshot.clone(),
        PcmBudget {
            store_bytes: 1024,
            ..PcmBudget::default()
        },
        SoundRenderBudget::default(),
        Cancellation::new().unwrap(),
    )
    .unwrap();
    assert!(
        limited
            .source_preparation(0, snapshot.duration_samples())
            .is_err()
    );
    assert_eq!(limited.pcm_stats().sources, 0);
    assert_eq!(limited.pcm_stats().stores, 0);
}
