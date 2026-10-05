use editbay_core::{EvaluationSnapshot, Project};
use editbay_media::{
    Cancellation, Error, NativeAudioReader, NativePcmCache, PcmBudget, SourceFile,
};
use std::{fs, path::Path, process::Command, sync::Arc};

fn fixture(path: &Path, codec: &str) -> Arc<EvaluationSnapshot> {
    let output = Command::new("ffmpeg").args(["-v", "error", "-f", "lavfi", "-i", "aevalsrc=1.25*sin(2*PI*137*t)|0.7*cos(2*PI*263*t)|0.25*sin(2*PI*701*t)|0.1*cos(2*PI*67*t)|0.3*sin(2*PI*997*t)|0.4*cos(2*PI*1103*t):s=48000:d=4:c=5.1", "-c:a", codec]).arg(path).output().unwrap();
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
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
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
            assert_eq!(result.pcm.interval(), (first, frames as usize, 6));
            let a = &expected[first as usize * 6..(first as usize + frames as usize) * 6];
            let error = a
                .iter()
                .zip(result.pcm.samples())
                .map(|(a, b)| (a - b).abs())
                .fold(0f32, f32::max);
            assert!(error <= 1e-6, "{codec} {first}: {error}");
            cache.validate_result(&result).unwrap();
        }
        if codec == "pcm_f32le" {
            assert!(expected.iter().any(|v| v.abs() > 1.));
        }
        let edge = cache.interval(source, 0, -16, 32).unwrap();
        assert_eq!(&edge.pcm.samples()[..16 * 6], &[0.; 96]);
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
