use editbay_core::{EvaluationSnapshot, Project};
use editbay_media::{
    Cancellation, Error, NativeAudioReader, NativePcmCache, PcmBudget, PcmProgress, SourceFile,
};
use std::{fs, path::Path, process::Command, sync::Arc};

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
        .ingest("Native multichannel".into(), &[0], cancel, |_, _| {})
        .unwrap();
    let mut p = Project::new("PCM receipts").unwrap();
    p.assets.push(source.asset);
    p.sources.push(source.source);
    Arc::new(EvaluationSnapshot::new(Arc::new(p)).unwrap())
}
fn reference(path: &Path) -> Vec<f32> {
    let output = Command::new("ffmpeg")
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
    assert!(output.status.success());
    output
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

#[test]
fn original_channels_headroom_intervals_and_delayed_codec_match_native_sequential_pcm() {
    let dir = tempfile::tempdir().unwrap();
    for (name, codec) in [
        ("original.wav", "pcm_f32le"),
        ("delayed.m4a", "aac"),
        ("container-ticks.mkv", "pcm_f32le"),
    ] {
        let path = dir.path().join(name);
        let snapshot = fixture(&path, codec);
        let source = snapshot.project().sources[0].id;
        let expected = reference(&path);
        let mut cache = NativePcmCache::new(
            snapshot.clone(),
            PcmBudget::default(),
            Cancellation::new().unwrap(),
        )
        .unwrap();
        for (first, frames) in [
            (0, 4096),
            (4096, 4096),
            (47999, 8192),
            (160000, 4096),
            (24000, 4096),
            (100000, 4096),
        ] {
            let result = cache.interval(source, 0, first, frames).unwrap();
            assert_eq!(result.pcm().interval(), (first, frames as usize, 6));
            let a = &expected[first as usize * 6..(first as usize + frames as usize) * 6];
            let error = a
                .iter()
                .zip(result.pcm().samples())
                .map(|(a, b)| (a - b).abs())
                .fold(0f32, f32::max);
            assert!(error <= 1e-6, "{codec} {first}: {error}");
            cache.validate_result(&result).unwrap();
        }
        if codec == "pcm_f32le" {
            assert!(expected.iter().any(|v| v.abs() > 1.));
        }
        let edge = cache.interval(source, 0, -16, 32).unwrap();
        assert_eq!(&edge.pcm().samples()[..16 * 6], &[0.; 96]);
        cache.verify_sources().unwrap();
        let source_file = SourceFile::open(&path, &Cancellation::new().unwrap()).unwrap();
        let mut reader =
            NativeAudioReader::open_stream(&source_file, 0, Cancellation::new().unwrap()).unwrap();
        let mut decoded = vec![];
        while let Some(block) = reader.next_block().unwrap() {
            decoded.extend(block.samples);
        }
        assert_eq!(decoded.len(), 192000 * 6);
        assert_eq!(&decoded[..4096 * 6], &expected[..4096 * 6]);
    }
}

#[test]
fn consumer_pins_survive_eviction_cleanup_and_rebind_while_receipts_reject_foreign_owners() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = fixture(&dir.path().join("original.wav"), "pcm_f32le");
    let source = snapshot.project().sources[0].id;
    let cancel = Cancellation::new().unwrap();
    let budget = PcmBudget {
        cache_bytes: 96,
        live_bytes: 192,
        cache_entries: 1,
        ..PcmBudget::default()
    };
    let mut cache = NativePcmCache::new(snapshot.clone(), budget, cancel.clone()).unwrap();
    let a = cache.interval(source, 0, 0, 4).unwrap();
    let b = cache.interval(source, 0, 4, 4).unwrap();
    assert_eq!(cache.stats().live_bytes, 192);
    assert!(cache.interval(source, 0, 8, 4).is_err());
    cache.clear();
    assert_eq!(cache.stats().live_bytes, 192);
    assert_eq!(cache.stats().decoders, 0);
    assert_eq!(cache.stats().sources, 0);
    assert!(cache.validate_result(&a).is_err());
    drop(b);
    assert_eq!(cache.stats().live_bytes, 96);
    cache
        .rebind(snapshot.clone(), Cancellation::new().unwrap())
        .unwrap();
    assert!(cancel.is_cancelled());
    let c = cache.interval(source, 0, 0, 4).unwrap();
    assert!(cache.validate_result(&a).is_err());
    let foreign = NativePcmCache::new(snapshot, budget, Cancellation::new().unwrap()).unwrap();
    assert!(foreign.validate_result(&c).is_err());
    drop(a);
    drop(c);
    cache.clear();
    assert_eq!(cache.stats().live_bytes, 0);
}

#[test]
fn changed_sources_cached_hits_and_cancelled_work_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.wav");
    let snapshot = fixture(&path, "pcm_f32le");
    let source = snapshot.project().sources[0].id;
    let cancel = Cancellation::new().unwrap();
    let mut cache = NativePcmCache::new(snapshot, PcmBudget::default(), cancel.clone()).unwrap();
    let result = cache.interval(source, 0, 0, 32).unwrap();
    let bytes = fs::read(&path).unwrap();
    fs::rename(&path, dir.path().join("preserved.wav")).unwrap();
    fs::write(&path, bytes).unwrap();
    assert!(matches!(
        cache.interval(source, 0, 0, 32),
        Err(Error::SourceChanged(_))
    ));
    assert!(cache.validate_result(&result).is_err());
    cancel.cancel();
    assert!(matches!(
        cache.interval(source, 0, 0, 32),
        Err(Error::Cancelled)
    ));
    cache.clear();
    assert_eq!(cache.stats().entries, 0);
}

#[test]
fn nonzero_container_origin_retains_absolute_samples_after_normalized_ingest() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.wav");
    fixture(&original, "pcm_f32le");
    let shifted = dir.path().join("shifted.mkv");
    let result = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&original)
        .args(["-c:a", "copy", "-output_ts_offset", "2"])
        .arg(&shifted)
        .output()
        .unwrap();
    assert!(result.status.success());
    let cancel = Cancellation::new().unwrap();
    let imported = SourceFile::open(&shifted, &cancel)
        .unwrap()
        .ingest("Absolute sound origin".into(), &[0], cancel, |_, _| {})
        .unwrap();
    assert_eq!(imported.source.streams[0].start_tick, 96000);
    let source = imported.source.id;
    let mut p = Project::new("Container origin").unwrap();
    p.assets.push(imported.asset);
    p.sources.push(imported.source);
    let mut cache = NativePcmCache::new(
        Arc::new(EvaluationSnapshot::new(Arc::new(p)).unwrap()),
        PcmBudget::default(),
        Cancellation::new().unwrap(),
    )
    .unwrap();
    let decoded = cache.interval(source, 0, 96000, 4096).unwrap();
    assert_eq!(decoded.pcm().samples(), &reference(&shifted)[..4096 * 6]);
    assert_eq!(
        cache
            .interval(source, 0, 95984, 32)
            .unwrap()
            .pcm()
            .samples()[..96],
        [0.; 96]
    );
}

#[test]
fn late_aac_seeks_and_decoder_eviction_preserve_canonical_bits_with_bounded_steps() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("long.m4a");
    let snapshot = fixture_seconds(&path, "aac", 12);
    let expected = reference(&path);
    let mut project = (**snapshot.project()).clone();
    let first = project.sources[0].id;
    let mut alias = project.sources[0].clone();
    alias.id = uuid::Uuid::new_v4();
    let second = alias.id;
    project.sources.push(alias);
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap());
    let budget = PcmBudget {
        cache_entries: 0,
        decoder_handles: 1,
        decode_frames: 131072,
        ..PcmBudget::default()
    };
    let mut cache = NativePcmCache::new_in(
        snapshot.clone(),
        budget,
        Cancellation::new().unwrap(),
        directory.path(),
    )
    .unwrap();
    let mut previous = 0;
    let mut steps = 0;
    loop {
        let before = cache.stats().decoded_frames;
        let progress = cache.prepare_interval(first, 0, 480000, 4096).unwrap();
        let observed = PcmProgress {
            version: editbay_core::DocumentVersion::of(snapshot.project()),
            request: 1,
            preparation: progress,
        };
        assert!(observed.valid_after(None, snapshot.project(), budget));
        for invalid in [
            PcmProgress {
                request: 0,
                ..observed
            },
            PcmProgress {
                version: editbay_core::DocumentVersion {
                    project_id: uuid::Uuid::new_v4(),
                    ..observed.version
                },
                ..observed
            },
            PcmProgress {
                preparation: editbay_media::PcmPreparation {
                    source: uuid::Uuid::new_v4(),
                    ..progress
                },
                ..observed
            },
            PcmProgress {
                preparation: editbay_media::PcmPreparation {
                    required_frames: progress.required_frames + 1,
                    ..progress
                },
                ..observed
            },
            PcmProgress {
                preparation: editbay_media::PcmPreparation {
                    ready: !progress.ready,
                    ..progress
                },
                ..observed
            },
        ] {
            assert!(!invalid.valid_after(None, snapshot.project(), budget));
        }
        assert!(!observed.valid_after(
            Some(PcmProgress {
                request: 2,
                ..observed
            }),
            snapshot.project(),
            budget
        ));
        assert_eq!(progress.required_frames, 484096);
        assert!(progress.decoded_frames > previous);
        assert!(cache.stats().decoded_frames - before <= u64::from(budget.decode_frames));
        assert_eq!(
            progress.ready,
            progress.decoded_frames == progress.required_frames
        );
        previous = progress.decoded_frames;
        steps += 1;
        if progress.ready {
            break;
        }
    }
    assert!(steps > 1);
    for start in [480000, 1, 47999, 150000, 0, 480000] {
        let decoded_before = cache.stats().decoded_frames;
        let result = cache.interval(first, 0, start, 4096).unwrap();
        assert_eq!(
            result.pcm().samples(),
            &expected[start as usize * 6..(start as usize + 4096) * 6]
        );
        assert_eq!(cache.stats().decoded_frames, decoded_before);
    }
    let held = cache.interval(first, 0, 480000, 4096).unwrap();
    cache.interval(second, 0, 100000, 4096).unwrap();
    assert_eq!(cache.stats().decoders, 1);
    let extended = cache.interval(first, 0, 560000, 4096).unwrap();
    assert_eq!(extended.pcm().samples(), &expected[560000 * 6..564096 * 6]);
    assert_eq!(held.pcm().samples(), &expected[480000 * 6..484096 * 6]);
    assert!(cache.stats().store_bytes <= budget.store_bytes);
    assert_eq!(cache.stats().stores, 2);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    cache.clear();
    assert_eq!(cache.stats().stores, 0);
    assert_eq!(cache.stats().store_bytes, 0);
    assert_eq!(held.pcm().samples(), &expected[480000 * 6..484096 * 6]);
    assert!(cache.validate_result(&held).is_err());
}

#[test]
fn canonical_storage_limits_and_cancelled_preparation_preserve_sources_and_release_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("original.m4a");
    let snapshot = fixture(&path, "aac");
    let source = snapshot.project().sources[0].id;
    let bytes = fs::read(&path).unwrap();
    let budget = PcmBudget {
        store_bytes: 100000,
        ..PcmBudget::default()
    };
    let mut cache = NativePcmCache::new_in(
        snapshot.clone(),
        budget,
        Cancellation::new().unwrap(),
        directory.path(),
    )
    .unwrap();
    assert!(cache.prepare_interval(source, 0, 96000, 4096).is_err());
    assert_eq!(cache.stats().store_bytes, 0);
    assert_eq!(cache.stats().stores, 0);
    let cancel = Cancellation::new().unwrap();
    let mut cache = NativePcmCache::new_in(
        snapshot,
        PcmBudget {
            decode_frames: 65536,
            ..PcmBudget::default()
        },
        cancel.clone(),
        directory.path(),
    )
    .unwrap();
    let progress = cache.prepare_interval(source, 0, 160000, 4096).unwrap();
    assert!(!progress.ready);
    assert!(cache.stats().store_bytes > 0);
    cancel.cancel();
    assert!(matches!(
        cache.prepare_interval(source, 0, 160000, 4096),
        Err(Error::Cancelled)
    ));
    cache.clear();
    assert_eq!(cache.stats().store_bytes, 0);
    assert_eq!(cache.stats().stores, 0);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    assert_eq!(fs::read(&path).unwrap(), bytes);
}
