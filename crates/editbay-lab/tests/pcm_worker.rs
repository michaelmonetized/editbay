use editbay_audio::{SoundRenderBudget, SoundRenderer};
use editbay_core::*;
use editbay_media::{
    Cancellation, Error, NativePcmCache, PcmBudget, PcmProvider, SourceFile,
    pcm_worker::{PcmWorker, PcmWorkerBudget},
};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

fn fixture(path: &Path, codec: &str) -> Arc<EvaluationSnapshot> {
    fixture_seconds(path, codec, 4)
}
fn fixture_seconds(path: &Path, codec: &str, seconds: u32) -> Arc<EvaluationSnapshot> {
    let input = format!(
        "aevalsrc=1.25*sin(2*PI*137*t)|0.7*cos(2*PI*263*t)|0.25*sin(2*PI*701*t)|0.1*cos(2*PI*67*t)|0.3*sin(2*PI*997*t)|0.4*cos(2*PI*1103*t):s=48000:d={seconds}:c=5.1"
    );
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", &input, "-c:a", codec])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cancel = Cancellation::new().unwrap();
    let source = SourceFile::open(path, &cancel)
        .unwrap()
        .ingest("Original channels".into(), &[0], cancel, |_, _| {})
        .unwrap();
    let mut project = Project::new("Sound worker verification").unwrap();
    project.assets.push(source.asset);
    project.sources.push(source.source);
    Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap())
}

#[test]
fn bounded_preparation_allocates_no_output_handles_and_reaps_cancelled_or_dead_workers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bounded.m4a");
    let snapshot = fixture(&path, "aac");
    let source = snapshot.project().sources[0].id;
    let original = fs::read(&path).unwrap();
    let expected = reference(&path);
    let budget = PcmWorkerBudget {
        pcm: PcmBudget {
            decode_frames: 65536,
            ..PcmBudget::default()
        },
        ..PcmWorkerBudget::default()
    };
    for mode in ["complete", "cancel", "kill"] {
        let cancel = Cancellation::new().unwrap();
        let mut worker = spawn(snapshot.clone(), budget, cancel.clone());
        let pid = worker.process_id().unwrap();
        let first = worker.prepare_interval(source, 0, 191999, 1).unwrap();
        assert!(!first.ready);
        assert!(first.decoded_frames > 0 && first.decoded_frames <= 65536);
        assert_eq!(worker.transfer_stats().mapped_handles, 0);
        assert_eq!(worker.transfer_stats().child.live_bytes, 0);
        assert_eq!(worker.transfer_stats().child.entries, 0);
        if mode != "complete" {
            if mode == "cancel" {
                cancel.cancel();
            } else {
                signal(pid, "-KILL");
            }
            let began = Instant::now();
            assert!(worker.prepare_interval(source, 0, 191999, 1).is_err());
            assert!(began.elapsed() < Duration::from_secs(2));
            assert!(!Path::new(&format!("/proc/{pid}")).exists());
            worker
                .rebind(snapshot.clone(), Cancellation::new().unwrap())
                .unwrap();
        }
        let mut previous = None;
        for step in 0..1000 {
            let decoded = worker.transfer_stats().child.decoded_frames;
            let progress = worker.prepare_interval(source, 0, 191999, 1).unwrap();
            let record = worker.preparation_log().observation().unwrap();
            assert!(record.valid_after(previous, snapshot.project(), budget.pcm));
            previous = Some(record);
            assert!(worker.transfer_stats().child.decoded_frames - decoded <= 65536);
            assert_eq!(worker.transfer_stats().mapped_handles, 0);
            assert_eq!(worker.transfer_stats().child.live_bytes, 0);
            if progress.ready {
                break;
            }
            assert!(step < 999);
        }
        let decoded = worker.transfer_stats().child.decoded_frames;
        let tail = worker.interval(source, 0, 180000, 4096).unwrap();
        assert!(
            tail.pcm()
                .samples()
                .iter()
                .zip(&expected[180000 * 6..184096 * 6])
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        assert_eq!(worker.transfer_stats().child.decoded_frames, decoded);
        worker.clear();
        assert_eq!(worker.transfer_stats().child.store_bytes, 0);
        assert!(worker.validate_result(&tail).is_err());
        drop(tail);
        assert_eq!(worker.transfer_stats().mapped_handles, 0);
    }
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn cold_canonical_worker_progress_matches_sequential_pcm_and_reaps_interrupted_preparation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("long.m4a");
    let snapshot = fixture_seconds(&path, "aac", 12);
    let source = snapshot.project().sources[0].id;
    let expected = reference(&path);
    let original = fs::read(&path).unwrap();
    let budget = PcmWorkerBudget {
        pcm: PcmBudget {
            decode_frames: 65536,
            ..PcmBudget::default()
        },
        ..PcmWorkerBudget::default()
    };
    for mode in ["complete", "cancel", "kill"] {
        let cancel = Cancellation::new().unwrap();
        let mut worker = spawn(snapshot.clone(), budget, cancel.clone());
        let log = worker.preparation_log();
        let pid = worker.process_id().unwrap();
        let thread = std::thread::spawn(move || {
            let result = worker.interval(source, 0, 560000, 4096);
            (worker, result)
        });
        let began = Instant::now();
        let mut previous = None;
        let mut partial = false;
        let mut interrupted = None;
        while !thread.is_finished() {
            assert!(began.elapsed() < Duration::from_secs(15));
            if let Some(progress) = log.observation() {
                assert!(progress.valid_after(previous, snapshot.project(), budget.pcm));
                previous = Some(progress);
                partial |= !progress.preparation.ready;
                if partial && mode != "complete" && interrupted.is_none() {
                    interrupted = Some(Instant::now());
                    if mode == "cancel" {
                        cancel.cancel();
                    } else {
                        signal(pid, "-KILL");
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let (mut worker, result) = thread.join().unwrap();
        assert!(partial, "no intermediate preparation record for {mode}");
        if mode == "complete" {
            let held = result.unwrap();
            assert_eq!(held.pcm().samples(), &expected[560000 * 6..564096 * 6]);
            assert!(log.observation().unwrap().preparation.ready);
            let decoded = worker.transfer_stats().child.decoded_frames;
            let earlier = worker.interval(source, 0, 140001, 8192).unwrap();
            assert_eq!(earlier.pcm().samples(), &expected[140001 * 6..148193 * 6]);
            assert_eq!(worker.transfer_stats().child.decoded_frames, decoded);
            assert!(worker.transfer_stats().child.store_bytes <= budget.pcm.store_bytes);
            worker.clear();
            assert_eq!(held.pcm().samples(), &expected[560000 * 6..564096 * 6]);
            assert!(worker.validate_result(&held).is_err());
            drop((held, earlier));
        } else {
            assert!(result.is_err());
            assert!(interrupted.unwrap().elapsed() < Duration::from_secs(2));
            assert!(!Path::new(&format!("/proc/{pid}")).exists());
            worker
                .rebind(snapshot.clone(), Cancellation::new().unwrap())
                .unwrap();
            let retry = worker.interval(source, 0, 560000, 4096).unwrap();
            assert_eq!(retry.pcm().samples(), &expected[560000 * 6..564096 * 6]);
            drop(retry);
            worker.clear();
        }
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        assert_eq!(worker.transfer_stats().child.store_bytes, 0);
        assert_eq!(worker.transfer_stats().mapped_bytes, 0);
        assert_eq!(worker.transfer_stats().mapped_handles, 0);
    }
    assert_eq!(fs::read(path).unwrap(), original);
}
fn spawn(
    snapshot: Arc<EvaluationSnapshot>,
    budget: PcmWorkerBudget,
    cancel: Cancellation,
) -> PcmWorker {
    PcmWorker::new(
        Path::new(env!("CARGO_BIN_EXE_editbay-lab")),
        snapshot,
        budget,
        cancel,
    )
    .unwrap()
}
fn signal(pid: u32, signal: &str) {
    assert!(
        Command::new("kill")
            .args([signal, &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
}
fn reference(path: &Path) -> Vec<f32> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-f",
            "f32le",
            "-c:a",
            "pcm_f32le",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stdout.len().is_multiple_of(4));
    out.stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|s| f32::from_le_bytes(*s))
        .collect()
}
#[test]
fn actual_pcm_process_preserves_original_channels_headroom_seeks_and_padding() {
    let dir = tempfile::tempdir().unwrap();
    for (name, codec) in [
        ("Original sound.wav", "pcm_f32le"),
        ("Delayed sound.m4a", "aac"),
        ("Container ticks.mkv", "pcm_f32le"),
    ] {
        let path = dir.path().join(name);
        let snapshot = fixture(&path, codec);
        let original = fs::read(&path).unwrap();
        let source = snapshot.project().sources[0].id;
        let expected = reference(&path);
        let mut worker = spawn(
            snapshot.clone(),
            PcmWorkerBudget::default(),
            Cancellation::new().unwrap(),
        );
        let pid = worker.process_id().unwrap();
        let limits = fs::read_to_string(format!("/proc/{pid}/limits")).unwrap();
        assert!(limits.contains("3221225472") && limits.contains("900"));
        let mut native =
            NativePcmCache::new(snapshot, PcmBudget::default(), Cancellation::new().unwrap())
                .unwrap();
        for (first, frames) in [
            (0, 4096),
            (4096, 4096),
            (47999, 8192),
            (180000, 12000),
            (24000, 4096),
            (100000, 4096),
            (0, 4096),
        ] {
            let result = worker.interval(source, 0, first, frames).unwrap();
            let local = native.interval(source, 0, first, frames).unwrap();
            assert_eq!(result.pcm().interval(), (first, frames as usize, 6));
            assert_eq!(result.pcm().samples(), local.pcm().samples());
            let error = result
                .pcm()
                .samples()
                .iter()
                .zip(&expected[first as usize * 6..(first as usize + frames as usize) * 6])
                .map(|(a, b)| (a - b).abs())
                .fold(0f32, f32::max);
            assert!(error <= 1e-6, "{codec} at {first}: {error}");
            worker.validate_result(&result).unwrap();
        }
        let a = worker.interval(source, 0, -16, 32).unwrap();
        assert_eq!(&a.pcm().samples()[..96], &[0.; 96]);
        let b = worker.interval(source, 0, -16, 32).unwrap();
        assert!(Arc::ptr_eq(a.pcm(), b.pcm()));
        let tail = worker.interval(source, 0, 191984, 32).unwrap();
        assert_eq!(&tail.pcm().samples()[96..], &[0.; 96]);
        if codec == "pcm_f32le" {
            assert!(expected.iter().any(|v| v.abs() > 1.));
        }
        worker.verify_sources().unwrap();
        drop((a, b, tail));
        worker.clear();
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        assert_eq!(worker.transfer_stats().mapped_bytes, 0);
        assert_eq!(worker.transfer_stats().mapped_handles, 0);
        assert_eq!(fs::read(path).unwrap(), original);
    }
}
#[test]
fn parent_byte_and_handle_pins_remain_charged_through_eviction_clear_and_rebind() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = fixture(&dir.path().join("Pins.wav"), "pcm_f32le");
    let source = snapshot.project().sources[0].id;
    for (live_bytes, live_handles, reason) in [(192, 8, "bytes"), (4096, 2, "handles")] {
        let budget = PcmWorkerBudget {
            pcm: PcmBudget {
                cache_bytes: 96,
                live_bytes,
                cache_entries: 1,
                ..PcmBudget::default()
            },
            live_handles,
        };
        let mut worker = spawn(snapshot.clone(), budget, Cancellation::new().unwrap());
        let a = worker.interval(source, 0, 0, 4).unwrap();
        let b = worker.interval(source, 0, 4, 4).unwrap();
        let error = worker.interval(source, 0, 8, 4).err().unwrap();
        assert!(error.to_string().contains(reason), "{error}");
        assert_eq!(worker.transfer_stats().mapped_bytes, 192);
        assert_eq!(worker.transfer_stats().mapped_handles, 2);
        drop(b);
        let c = worker.interval(source, 0, 8, 4).unwrap();
        worker.validate_result(&c).unwrap();
        worker.clear();
        assert!(worker.validate_result(&a).is_err());
        assert_eq!(worker.transfer_stats().mapped_bytes, 192);
        assert_eq!(worker.transfer_stats().child.sources, 0);
        drop(c);
        worker
            .rebind(snapshot.clone(), Cancellation::new().unwrap())
            .unwrap();
        let fresh = worker.interval(source, 0, 0, 4).unwrap();
        assert_eq!(a.pcm().samples(), fresh.pcm().samples());
        assert!(worker.validate_result(&a).is_err());
        worker.validate_result(&fresh).unwrap();
        let mut foreign = spawn(snapshot.clone(), budget, Cancellation::new().unwrap());
        assert!(foreign.validate_result(&fresh).is_err());
        drop((a, fresh));
        worker.clear();
        assert_eq!(worker.transfer_stats().mapped_bytes, 0);
        assert_eq!(worker.transfer_stats().mapped_handles, 0);
    }
}
#[test]
fn death_and_stopped_request_cancellation_reap_children_and_require_explicit_retry() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = fixture(&dir.path().join("Crash.wav"), "pcm_f32le");
    let source = snapshot.project().sources[0].id;
    let cancel = Cancellation::new().unwrap();
    let mut worker = spawn(snapshot.clone(), PcmWorkerBudget::default(), cancel.clone());
    let old = worker.interval(source, 0, 0, 4096).unwrap();
    let pid = worker.process_id().unwrap();
    signal(pid, "-STOP");
    let trigger = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        cancel.cancel();
        Instant::now()
    });
    assert!(matches!(
        worker.interval(source, 0, 120000, 8192),
        Err(Error::Cancelled)
    ));
    assert!(trigger.join().unwrap().elapsed() < Duration::from_secs(2));
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert!(worker.process_id().is_none());
    assert!(worker.validate_result(&old).is_err());
    worker
        .rebind(snapshot.clone(), Cancellation::new().unwrap())
        .unwrap();
    let new = worker.interval(source, 0, 0, 4096).unwrap();
    assert_eq!(old.pcm().samples(), new.pcm().samples());
    assert!(worker.validate_result(&old).is_err());
    worker.validate_result(&new).unwrap();
    let pid = worker.process_id().unwrap();
    signal(pid, "-KILL");
    assert!(worker.interval(source, 0, 0, 4096).is_err());
    assert!(worker.process_id().is_none());
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert!(worker.interval(source, 0, 0, 4096).is_err());
    worker
        .rebind(snapshot, Cancellation::new().unwrap())
        .unwrap();
    worker.interval(source, 0, 0, 4096).unwrap();
    worker.clear();
    drop((old, new));
    assert_eq!(worker.transfer_stats().mapped_bytes, 0);
    assert_eq!(worker.transfer_stats().mapped_handles, 0);
}
#[test]
fn changed_source_rejects_cached_samples_and_publication_without_touching_either_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Source.wav");
    let snapshot = fixture(&path, "pcm_f32le");
    let source = snapshot.project().sources[0].id;
    let bytes = fs::read(&path).unwrap();
    let mut worker = spawn(
        snapshot,
        PcmWorkerBudget::default(),
        Cancellation::new().unwrap(),
    );
    let held = worker.interval(source, 0, 0, 16).unwrap();
    let retained = dir.path().join("Retained.wav");
    fs::rename(&path, &retained).unwrap();
    fs::write(&path, b"Replacement stays intact").unwrap();
    assert!(matches!(
        worker.validate_result(&held),
        Err(Error::SourceChanged(_))
    ));
    assert!(worker.interval(source, 0, 0, 16).is_err());
    assert!(worker.verify_sources().is_err());
    assert_eq!(fs::read(retained).unwrap(), bytes);
    assert_eq!(fs::read(path).unwrap(), b"Replacement stays intact");
    assert_eq!(held.pcm().samples().len(), 96);
}
#[test]
fn shared_renderer_uses_identical_fractional_reverse_gain_and_mix_over_codec_ipc() {
    let dir = tempfile::tempdir().unwrap();
    let initial = fixture(&dir.path().join("Graph.wav"), "pcm_f32le");
    let mut project = (**initial.project()).clone();
    let source = project.sources[0].id;
    let duration = 96;
    let composition = Uuid::new_v4();
    let clip = Uuid::new_v4();
    let source_node = Uuid::new_v4();
    let gain = Uuid::new_v4();
    let mix = Uuid::new_v4();
    project.compositions.push(Composition {
        id: composition,
        name: "Reverse retimed sound".into(),
        width: 32,
        height: 24,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration,
        tracks: vec![Track {
            id: Uuid::new_v4(),
            name: "Six channels".into(),
            kind: TrackKind::Audio,
            enabled: true,
            clips: vec![Clip {
                id: clip,
                name: "Original".into(),
                range: FrameRange {
                    start: 0,
                    end: duration,
                },
                source: ClipSource::Media { source, stream: 0 },
                linked: None,
                time_map: TimeMap {
                    source_denominator: 1,
                    points: vec![
                        TimePoint {
                            frame: 0,
                            source_tick: 192000,
                        },
                        TimePoint {
                            frame: duration,
                            source_tick: 0,
                        },
                    ],
                },
            }],
        }],
        nodes: vec![
            TimedNode {
                id: source_node,
                range: FrameRange {
                    start: 0,
                    end: duration,
                },
                operation: NodeOperation::Source { clip },
                animation: vec![],
            },
            TimedNode {
                id: gain,
                range: FrameRange {
                    start: 0,
                    end: duration,
                },
                operation: NodeOperation::Gain {
                    audio: source_node,
                    gain: 0.25,
                },
                animation: vec![],
            },
            TimedNode {
                id: mix,
                range: FrameRange {
                    start: 0,
                    end: duration,
                },
                operation: NodeOperation::Mix {
                    inputs: vec![source_node, gain],
                },
                animation: vec![],
            },
        ],
        picture: None,
        audio: Some(mix),
    });
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap());
    let StreamFormat::Audio { channels, .. } = &snapshot.project().sources[0].streams[0].format
    else {
        panic!()
    };
    for rate in [48000, 44100] {
        let sound = Arc::new(
            SoundSnapshot::new(
                snapshot.clone(),
                composition,
                SoundProfile {
                    sample_rate: rate,
                    channels: channels.clone(),
                },
                SoundBudget::default(),
            )
            .unwrap(),
        );
        let cancel = Cancellation::new().unwrap();
        let worker = spawn(snapshot.clone(), PcmWorkerBudget::default(), cancel.clone());
        let mut isolated =
            SoundRenderer::with_provider(sound.clone(), worker, SoundRenderBudget::default())
                .unwrap();
        let mut native = SoundRenderer::new(
            sound.clone(),
            PcmBudget::default(),
            SoundRenderBudget::default(),
            Cancellation::new().unwrap(),
        )
        .unwrap();
        for first in [0, 4096, 70000, 2048] {
            let plan = sound.prepare(first, 4096).unwrap();
            let a = isolated.render(&plan).unwrap();
            let b = native.render(&plan).unwrap();
            assert_eq!(a.sound().samples(), b.sound().samples());
            isolated.validate_result(&a).unwrap();
        }
        isolated.verify_sources().unwrap();
        let pid = isolated.pcm_provider().process_id().unwrap();
        signal(pid, "-STOP");
        let plan = sound.prepare(0, 4096).unwrap();
        let thread = std::thread::spawn(move || {
            let rejected = matches!(isolated.render(&plan), Err(Error::Cancelled));
            isolated.clear();
            (rejected, isolated.pcm_stats().live_bytes)
        });
        std::thread::sleep(Duration::from_millis(20));
        let began = Instant::now();
        cancel.cancel();
        let (rejected, live) = thread.join().unwrap();
        assert!(rejected);
        assert_eq!(live, 0);
        assert!(began.elapsed() < Duration::from_secs(2));
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        let foreign =
            Arc::new(EvaluationSnapshot::new(Arc::new((**snapshot.project()).clone())).unwrap());
        let provider =
            NativePcmCache::new(foreign, PcmBudget::default(), Cancellation::new().unwrap())
                .unwrap();
        assert!(
            SoundRenderer::with_provider(sound, provider, SoundRenderBudget::default()).is_err()
        );
    }
}
