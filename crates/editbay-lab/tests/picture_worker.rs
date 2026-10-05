use editbay_core::*;
use editbay_media::{
    Cancellation, Error, PictureBudget, PictureCache, PictureProvider, SourceFile, VideoReader,
    picture_worker::{PictureWorker, WorkerBudget},
};
use editbay_render::{GraphBudget, GraphRenderer, ImageBoundary};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

fn snapshot(path: &Path) -> Arc<EvaluationSnapshot> {
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=32x24:rate=12:duration=2",
            "-f",
            "lavfi",
            "-i",
            "color=blue:size=16x16:rate=6:duration=2",
            "-map",
            "0:v",
            "-map",
            "1:v",
            "-filter:v:0",
            "select='not(eq(n,4))'",
            "-fps_mode:v:0",
            "vfr",
            "-c:v",
            "libx264",
            "-threads",
            "2",
            "-g",
            "24",
            "-bf",
            "3",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cancel = Cancellation::new().unwrap();
    let owned = SourceFile::open(path, &cancel).unwrap();
    let imported = owned
        .ingest("Selected VFR streams".into(), &[0, 1], cancel, |_, _| {})
        .unwrap();
    let mut project = Project::new("Codec process qualification").unwrap();
    project.assets = vec![imported.asset];
    project.sources = vec![imported.source];
    Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap())
}
fn request(snapshot: &EvaluationSnapshot, stream: u32, ordinal: usize) -> SourceRequest {
    let source = snapshot.project().sources[0].id;
    let (asset, profile, interpretation) = snapshot.source_stream(source, stream).unwrap();
    let StreamFormat::Video {
        timing: PictureTiming::Variable {
            presentation_ticks, ..
        },
        ..
    } = &profile.format
    else {
        panic!("index absent")
    };
    SourceRequest::Media {
        source,
        stream,
        asset: asset.id,
        asset_sha256: asset.sha256.clone(),
        stream_sha256: interpretation.into(),
        position: SourcePosition::new(presentation_ticks[ordinal], 1).unwrap(),
        reverse: false,
        picture: Some(ordinal as u64),
        sample: None,
    }
}
fn spawn_worker(
    snapshot: Arc<EvaluationSnapshot>,
    budget: WorkerBudget,
    cancel: Cancellation,
) -> PictureWorker {
    PictureWorker::new(
        Path::new(env!("CARGO_BIN_EXE_editbay-lab")),
        snapshot,
        budget,
        cancel,
    )
    .unwrap()
}
fn kill(pid: u32, signal: &str) {
    assert!(
        Command::new("kill")
            .args([signal, &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
}
#[test]
fn actual_process_planes_match_selected_vfr_delayed_native_decode_and_backward_after_eof() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Original source.mp4");
    let snapshot = snapshot(&path);
    let original = fs::read(&path).unwrap();
    let cancel = Cancellation::new().unwrap();
    let source = SourceFile::open(&path, &cancel).unwrap();
    let mut worker = spawn_worker(snapshot.clone(), WorkerBudget::default(), cancel.clone());
    let pid = worker.process_id().unwrap();
    let limits = fs::read_to_string(format!("/proc/{pid}/limits")).unwrap();
    assert!(limits.contains("3221225472"));
    assert!(limits.contains("900"));
    for stream in [0, 1] {
        let mut reference = VideoReader::open_stream(&source, stream, cancel.clone()).unwrap();
        let mut expected = Vec::new();
        while let Some(frame) = reference.next_frame().unwrap() {
            expected.push(frame);
        }
        assert!(reference.next_frame().unwrap().is_none());
        for ordinal in (0..expected.len()).chain((0..expected.len()).rev()) {
            let result = worker
                .picture(&request(&snapshot, stream, ordinal))
                .unwrap()
                .unwrap();
            assert_eq!(result.picture.rgba(), expected[ordinal].rgba);
            assert_eq!(result.picture.color, expected[ordinal].color);
            assert_eq!(result.picture.alpha, expected[ordinal].alpha);
            assert_eq!(
                Some(result.picture.source_tick),
                expected[ordinal].source_tick
            );
            worker.validate_result(&result).unwrap();
        }
        let (_, profile, _) = snapshot
            .source_stream(snapshot.project().sources[0].id, stream)
            .unwrap();
        let StreamFormat::Video {
            timing: PictureTiming::Variable { end_tick, .. },
            ..
        } = profile.format
        else {
            panic!()
        };
        let mut reverse = request(&snapshot, stream, expected.len() - 1);
        if let SourceRequest::Media {
            position, reverse, ..
        } = &mut reverse
        {
            *position = SourcePosition::new(end_tick, 1).unwrap();
            *reverse = true;
        }
        let result = worker.picture(&reverse).unwrap().unwrap();
        assert_eq!(result.picture.rgba(), expected.last().unwrap().rgba);
        let hit = worker.picture(&reverse).unwrap().unwrap();
        assert!(Arc::ptr_eq(&result.picture, &hit.picture));
    }
    let before = worker.transfer_stats().received_planes;
    let mut invalid = request(&snapshot, 0, 0);
    if let SourceRequest::Media { asset_sha256, .. } = &mut invalid {
        *asset_sha256 = "0".repeat(64);
    }
    assert!(worker.picture(&invalid).is_err());
    assert_eq!(worker.transfer_stats().received_planes, before);
    worker.verify_sources().unwrap();
    worker.clear().unwrap();
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert_eq!(worker.transfer_stats().mapped_bytes, 0);
    assert_eq!(worker.transfer_stats().mapped_handles, 0);
    assert_eq!(fs::read(path).unwrap(), original);
}
#[test]
fn pinned_shared_outputs_remain_charged_through_eviction_worker_death_and_retry() {
    let directory = tempfile::tempdir().unwrap();
    let snapshot = snapshot(&directory.path().join("Pinned source.mp4"));
    let bytes = 32 * 24 * 4;
    let budget = WorkerBudget {
        pictures: PictureBudget {
            cache_bytes: bytes * 2,
            live_bytes: bytes * 3,
            maximum_picture_bytes: bytes,
            cache_entries: 2,
            source_handles: 1,
            decoder_handles: 1,
        },
        live_handles: 3,
    };
    let mut worker = spawn_worker(snapshot.clone(), budget, Cancellation::new().unwrap());
    let mut pins = Vec::new();
    for i in 0..3 {
        pins.push(worker.picture(&request(&snapshot, 0, i)).unwrap().unwrap());
    }
    assert_eq!(worker.transfer_stats().mapped_bytes, bytes * 3);
    assert_eq!(worker.transfer_stats().mapped_handles, 3);
    assert!(worker.picture(&request(&snapshot, 0, 3)).is_err());
    pins.remove(0);
    let fourth = worker.picture(&request(&snapshot, 0, 3)).unwrap().unwrap();
    worker.validate_result(&fourth).unwrap();
    let before = worker.picture(&request(&snapshot, 0, 3)).unwrap().unwrap();
    let pid = worker.process_id().unwrap();
    kill(pid, "-KILL");
    assert!(worker.picture(&request(&snapshot, 0, 3)).is_err());
    assert!(worker.process_id().is_none());
    assert!(worker.validate_result(&before).is_err());
    assert_eq!(fourth.picture.rgba(), before.picture.rgba());
    let mut project = (**snapshot.project()).clone();
    project.rename("Next version").unwrap();
    let next = Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap());
    worker
        .rebind(next.clone(), Cancellation::new().unwrap())
        .unwrap();
    let after = worker.picture(&request(&next, 0, 3)).unwrap().unwrap();
    assert!(Arc::ptr_eq(&before.picture, &after.picture));
    assert!(worker.validate_result(&before).is_err());
    worker.validate_result(&after).unwrap();
    worker.clear().unwrap();
    assert_eq!(worker.transfer_stats().cache_entries, 0);
    assert_eq!(worker.transfer_stats().mapped_bytes, bytes * 3);
    drop(pins);
    drop(fourth);
    drop(before);
    drop(after);
    assert_eq!(worker.transfer_stats().mapped_bytes, 0);
    assert_eq!(worker.transfer_stats().mapped_handles, 0);
}
#[test]
fn stopped_codec_cancellation_reaps_underlying_process_and_fresh_retry_rejects_old_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let snapshot = snapshot(&directory.path().join("Stopped source.mp4"));
    let cancel = Cancellation::new().unwrap();
    let mut worker = spawn_worker(snapshot.clone(), WorkerBudget::default(), cancel.clone());
    let mut old = worker.picture(&request(&snapshot, 0, 0)).unwrap().unwrap();
    let pid = worker.process_id().unwrap();
    kill(pid, "-STOP");
    let trigger = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        cancel.cancel();
        Instant::now()
    });
    let start = Instant::now();
    assert!(matches!(
        worker.picture(&request(&snapshot, 0, 22)),
        Err(Error::Cancelled)
    ));
    let cancelled = trigger.join().unwrap();
    assert!(cancelled.elapsed() < Duration::from_secs(2));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    worker
        .rebind(snapshot.clone(), Cancellation::new().unwrap())
        .unwrap();
    let fresh = worker.picture(&request(&snapshot, 0, 0)).unwrap().unwrap();
    assert!(worker.validate_result(&old).is_err());
    worker.validate_result(&fresh).unwrap();
    old.version = fresh.version;
    old.generation = fresh.generation;
    assert!(worker.validate_result(&old).is_err());
    assert!(Arc::ptr_eq(&old.picture, &fresh.picture));
    let mut foreign = spawn_worker(
        snapshot,
        WorkerBudget::default(),
        Cancellation::new().unwrap(),
    );
    assert!(foreign.validate_result(&fresh).is_err());
    foreign.clear().unwrap();
}

#[test]
fn shared_handle_budget_is_independent_of_byte_budget_and_remains_charged_after_clear() {
    let directory = tempfile::tempdir().unwrap();
    let snapshot = snapshot(&directory.path().join("Handle source.mp4"));
    let bytes = 32 * 24 * 4;
    let budget = WorkerBudget {
        pictures: PictureBudget {
            cache_bytes: bytes * 2,
            live_bytes: bytes * 4,
            maximum_picture_bytes: bytes,
            cache_entries: 2,
            source_handles: 1,
            decoder_handles: 1,
        },
        live_handles: 2,
    };
    let mut worker = spawn_worker(snapshot.clone(), budget, Cancellation::new().unwrap());
    let first = worker.picture(&request(&snapshot, 0, 0)).unwrap().unwrap();
    let second = worker.picture(&request(&snapshot, 0, 1)).unwrap().unwrap();
    let error = worker
        .picture(&request(&snapshot, 0, 2))
        .err()
        .expect("handle budget rejects a third pin");
    assert!(error.to_string().contains("handles are pinned"));
    assert_eq!(worker.transfer_stats().mapped_bytes, bytes * 2);
    assert_eq!(worker.transfer_stats().mapped_handles, 2);
    drop(first);
    let third = worker.picture(&request(&snapshot, 0, 2)).unwrap().unwrap();
    worker.clear().unwrap();
    assert_eq!(worker.transfer_stats().mapped_handles, 2);
    drop(second);
    drop(third);
    assert_eq!(worker.transfer_stats().mapped_handles, 0);
    assert_eq!(worker.transfer_stats().mapped_bytes, 0);
}
#[test]
fn replaced_source_is_rejected_without_altering_original_or_replacement_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Source ownership.mp4");
    let snapshot = snapshot(&path);
    let bytes = fs::read(&path).unwrap();
    let mut worker = spawn_worker(
        snapshot.clone(),
        WorkerBudget::default(),
        Cancellation::new().unwrap(),
    );
    let pinned = worker.picture(&request(&snapshot, 0, 0)).unwrap().unwrap();
    let retained = directory.path().join("Retained original.mp4");
    fs::rename(&path, &retained).unwrap();
    fs::write(&path, b"Outside replacement stays intact").unwrap();
    assert!(matches!(
        worker.picture(&request(&snapshot, 0, 0)),
        Err(Error::SourceChanged(_))
    ));
    assert!(worker.verify_sources().is_err());
    assert!(!pinned.picture.rgba().is_empty());
    assert_eq!(fs::read(retained).unwrap(), bytes);
    assert_eq!(fs::read(path).unwrap(), b"Outside replacement stays intact");
}
#[test]
fn isolated_and_in_process_sources_use_identical_gpu_temporal_color_and_output_evaluation() {
    let directory = tempfile::tempdir().unwrap();
    let initial = snapshot(&directory.path().join("GPU process source.mp4"));
    let mut project = (**initial.project()).clone();
    let stream = &project.sources[0].streams[0];
    let StreamFormat::Video {
        timing:
            PictureTiming::Variable {
                presentation_ticks,
                end_tick,
            },
        ..
    } = &stream.format
    else {
        panic!()
    };
    let duration = presentation_ticks.len() as u64;
    let mut points: Vec<_> = presentation_ticks
        .iter()
        .enumerate()
        .map(|(frame, tick)| TimePoint {
            frame: frame as u64,
            source_tick: *tick,
        })
        .collect();
    points.push(TimePoint {
        frame: duration,
        source_tick: *end_tick,
    });
    let clip = uuid::Uuid::new_v4();
    let node = uuid::Uuid::new_v4();
    let composition = uuid::Uuid::new_v4();
    project.compositions = vec![Composition {
        id: composition,
        name: "Ordinal source timing".into(),
        width: 32,
        height: 24,
        frame_rate: FrameRate::new(12, 1).unwrap(),
        duration,
        tracks: vec![Track {
            id: uuid::Uuid::new_v4(),
            name: "Selected picture".into(),
            kind: TrackKind::Video,
            enabled: true,
            clips: vec![Clip {
                id: clip,
                name: "Original".into(),
                range: FrameRange {
                    start: 0,
                    end: duration,
                },
                source: ClipSource::Media {
                    source: project.sources[0].id,
                    stream: 0,
                },
                linked: None,
                time_map: TimeMap { points },
            }],
        }],
        nodes: vec![TimedNode {
            id: node,
            range: FrameRange {
                start: 0,
                end: duration,
            },
            operation: NodeOperation::Source { clip },
            animation: vec![],
        }],
        picture: Some(node),
        audio: None,
    }];
    project.color.precision = FloatPrecision::Full;
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap());
    let cancel = Cancellation::new().unwrap();
    let provider = spawn_worker(snapshot.clone(), WorkerBudget::default(), cancel.clone());
    let mut isolated =
        GraphRenderer::with_provider(snapshot.clone(), provider, GraphBudget::default(), cancel)
            .unwrap();
    let raw = PictureCache::new(
        snapshot.clone(),
        PictureBudget::default(),
        Cancellation::new().unwrap(),
    )
    .unwrap();
    let mut local = GraphRenderer::with_provider(
        snapshot.clone(),
        raw,
        GraphBudget::default(),
        Cancellation::new().unwrap(),
    )
    .unwrap();
    for ordinal in [0, 22, 5] {
        let position = SourcePosition::new(ordinal, 1).unwrap();
        let frame = isolated.render(composition, position, false).unwrap();
        let reference = local.render(composition, position, false).unwrap();
        let actual = isolated.readback(&frame).unwrap();
        let expected = local.readback(&reference).unwrap();
        assert_eq!(actual.len(), expected.len());
        for (a, b) in actual.iter().zip(expected) {
            assert!(a.is_finite() && (a - b).abs() <= 0.00002);
        }
        let output = isolated.convert(&frame, ImageBoundary::Output).unwrap();
        assert!(
            isolated
                .readback(&output)
                .unwrap()
                .iter()
                .all(|v| v.is_finite())
        );
        let hit = isolated.render(composition, position, false).unwrap();
        assert!(Arc::ptr_eq(frame.image(), hit.image()));
    }
    isolated.verify_sources().unwrap();
    isolated.clear().unwrap();
    assert_eq!(isolated.stats().live_texture_bytes, 0);
    assert_eq!(
        isolated.picture_provider().transfer_stats().mapped_handles,
        0
    );
}
