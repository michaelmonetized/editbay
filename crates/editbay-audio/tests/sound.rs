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
