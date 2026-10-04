use editbay_core::{
    EvaluationSnapshot, PictureTiming, Project, SourcePosition, SourceRequest, StreamFormat,
};
use editbay_media::{Cancellation, Error, PictureBudget, PictureCache, SourceFile, VideoReader};
use std::{fs, path::Path, process::Command, sync::Arc};
use tempfile::tempdir;

fn fixture(path: &Path) -> Arc<EvaluationSnapshot> {
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=16x16:rate=6:duration=1",
            "-f",
            "lavfi",
            "-i",
            "color=blue:size=8x8:rate=3:duration=1",
            "-map",
            "0:v",
            "-map",
            "1:v",
            "-filter:v:0",
            "select='not(eq(n,2))'",
            "-fps_mode:v:0",
            "vfr",
            "-c:v",
            "ffv1",
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
        .ingest("Actual cache fixture".into(), &[0, 1], cancel, |_, _| {})
        .unwrap();
    let mut project = Project::new("Cache worker fixture").unwrap();
    project.assets = vec![imported.asset];
    project.sources = vec![imported.source];
    Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap())
}

fn request(snapshot: &EvaluationSnapshot, stream: u32, ordinal: usize) -> SourceRequest {
    let source = snapshot.project().sources[0].id;
    let (asset, profile, fingerprint) = snapshot.source_stream(source, stream).unwrap();
    let StreamFormat::Video {
        timing: PictureTiming::Variable {
            presentation_ticks, ..
        },
        ..
    } = &profile.format
    else {
        panic!("actual index missing")
    };
    SourceRequest::Media {
        source,
        stream,
        asset: asset.id,
        asset_sha256: asset.sha256.clone(),
        stream_sha256: fingerprint.into(),
        position: SourcePosition::new(presentation_ticks[ordinal], 1).unwrap(),
        reverse: false,
        picture: Some(ordinal as u64),
        sample: None,
    }
}

fn budget() -> PictureBudget {
    PictureBudget {
        cache_bytes: 2048,
        live_bytes: 3072,
        maximum_picture_bytes: 1024,
        cache_entries: 2,
        source_handles: 1,
        decoder_handles: 1,
    }
}

#[test]
fn revision_rebind_reuses_matching_content_and_rejects_previous_receipts() {
    let directory = tempdir().unwrap();
    let snapshot = fixture(&directory.path().join("Revision source.mkv"));
    let mut cache =
        PictureCache::new(snapshot.clone(), budget(), Cancellation::new().unwrap()).unwrap();
    let before = cache.picture(&request(&snapshot, 0, 0)).unwrap().unwrap();
    let mut project = (**snapshot.project()).clone();
    project.rename("Metadata edit").unwrap();
    let next = Arc::new(EvaluationSnapshot::new(Arc::new(project.clone())).unwrap());
    cache
        .rebind(next.clone(), Cancellation::new().unwrap())
        .unwrap();
    assert!(cache.validate_result(&before).is_err());
    let after = cache.picture(&request(&next, 0, 0)).unwrap().unwrap();
    assert!(after.cache_hit);
    assert!(Arc::ptr_eq(&before.picture, &after.picture));
    assert_ne!(before.version, after.version);
    assert_ne!(before.generation, after.generation);
    cache.validate_result(&after).unwrap();
    cache.verify_sources().unwrap();
    project.assets[0].bytes += 1;
    let changed = Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap());
    cache
        .rebind(changed.clone(), Cancellation::new().unwrap())
        .unwrap();
    assert_eq!(cache.stats().sources, 0);
    assert_eq!(cache.stats().decoders, 0);
    assert!(matches!(
        cache.picture(&request(&changed, 0, 0)),
        Err(Error::SourceChanged(_))
    ));
    assert!(cache.validate_result(&after).is_err());
}

#[test]
fn sequential_reverse_eof_and_decoder_eviction_match_independent_native_pixels() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("VFR source.mkv");
    let snapshot = fixture(&path);
    let cancel = Cancellation::new().unwrap();
    let source = SourceFile::open(&path, &cancel).unwrap();
    let mut reference = VideoReader::open_stream(&source, 0, cancel.clone()).unwrap();
    let mut expected = Vec::new();
    while let Some(frame) = reference.next_frame().unwrap() {
        expected.push(frame);
    }
    assert_eq!(expected.len(), 5);
    assert!(reference.next_frame().unwrap().is_none());
    let mut cache = PictureCache::new(snapshot.clone(), budget(), cancel).unwrap();
    for (ordinal, reference) in expected.iter().enumerate() {
        let actual = cache
            .picture(&request(&snapshot, 0, ordinal))
            .unwrap()
            .unwrap();
        assert!(!actual.cache_hit);
        assert_eq!(actual.picture.rgba(), reference.rgba);
        assert_eq!(Some(actual.picture.source_tick), reference.source_tick);
        cache.validate_result(&actual).unwrap();
    }
    assert_eq!(cache.stats().sequential_decodes, 5);
    for ordinal in (0..5).rev() {
        let actual = cache
            .picture(&request(&snapshot, 0, ordinal))
            .unwrap()
            .unwrap();
        assert_eq!(actual.picture.rgba(), expected[ordinal].rgba);
        assert_eq!(actual.cache_hit, ordinal >= 3);
    }
    let first = cache.picture(&request(&snapshot, 0, 0)).unwrap().unwrap();
    let hit = cache.picture(&request(&snapshot, 0, 0)).unwrap().unwrap();
    assert!(hit.cache_hit);
    assert!(Arc::ptr_eq(&first.picture, &hit.picture));
    drop(first);
    drop(hit);
    let blue = cache.picture(&request(&snapshot, 1, 0)).unwrap().unwrap();
    assert_eq!((blue.picture.width, blue.picture.height), (8, 8));
    assert!(
        blue.picture
            .rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[2] > 240 && pixel[0] < 10 && pixel[1] < 10)
    );
    assert_eq!(cache.stats().decoders, 1);
    assert!(
        cache.stats().entries <= 2
            && cache.stats().cache_bytes <= 2048
            && cache.stats().live_bytes <= 3072
    );
    assert!(cache.stats().seeks >= 3);
    cache.verify_sources().unwrap();
}

#[test]
fn consumer_pins_stay_charged_through_eviction_and_clear_invalidates_receipts() {
    let directory = tempdir().unwrap();
    let snapshot = fixture(&directory.path().join("Pinned source.mkv"));
    let mut cache =
        PictureCache::new(snapshot.clone(), budget(), Cancellation::new().unwrap()).unwrap();
    let first = cache.picture(&request(&snapshot, 0, 0)).unwrap().unwrap();
    let second = cache.picture(&request(&snapshot, 0, 1)).unwrap().unwrap();
    let third = cache.picture(&request(&snapshot, 0, 2)).unwrap().unwrap();
    assert_eq!(cache.stats().live_bytes, 3072);
    assert_eq!(cache.stats().cache_bytes, 2048);
    assert!(cache.picture(&request(&snapshot, 0, 3)).is_err());
    assert_eq!(cache.stats().live_bytes, 3072);
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(cache.stats().sequential_decodes, 3);
    drop(first);
    assert_eq!(cache.stats().live_bytes, 2048);
    let fourth = cache.picture(&request(&snapshot, 0, 3)).unwrap().unwrap();
    assert_eq!(cache.stats().sequential_decodes, 4);
    cache.verify_sources().unwrap();
    cache.validate_result(&fourth).unwrap();
    cache.clear().unwrap();
    assert_eq!(cache.stats().sources, 0);
    assert_eq!(cache.stats().decoders, 0);
    assert_eq!(cache.stats().live_bytes, 3072);
    assert!(cache.validate_result(&fourth).is_err());
    drop(second);
    drop(third);
    drop(fourth);
    assert_eq!(cache.stats().live_bytes, 0);
}

#[test]
fn hits_refuse_changed_sources_forged_interpretations_and_cancellation() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("Protected source.mkv");
    let snapshot = fixture(&path);
    let original = fs::read(&path).unwrap();
    let cancel = Cancellation::new().unwrap();
    let mut cache = PictureCache::new(snapshot.clone(), budget(), cancel.clone()).unwrap();
    let valid = request(&snapshot, 0, 0);
    let first = cache.picture(&valid).unwrap().unwrap();
    let mut forged = valid.clone();
    if let SourceRequest::Media { stream_sha256, .. } = &mut forged {
        *stream_sha256 = "0".repeat(64);
    }
    assert!(cache.picture(&forged).is_err());
    forged = valid.clone();
    if let SourceRequest::Media { picture, .. } = &mut forged {
        *picture = Some(1);
    }
    assert!(cache.picture(&forged).is_err());
    assert_eq!(cache.stats().hits, 0);
    let moved = directory.path().join("Owned original.mkv");
    fs::rename(&path, &moved).unwrap();
    fs::write(&path, b"separate replacement bytes").unwrap();
    assert!(matches!(
        cache.picture(&valid),
        Err(Error::SourceChanged(_))
    ));
    assert!(cache.verify_sources().is_err());
    assert_eq!(cache.stats().hits, 0);
    assert_eq!(fs::read(&moved).unwrap(), original);
    cancel.cancel();
    assert!(matches!(cache.picture(&valid), Err(Error::Cancelled)));
    assert!(matches!(
        cache.validate_result(&first),
        Err(Error::Cancelled)
    ));
    assert!(matches!(cache.verify_sources(), Err(Error::Cancelled)));
    assert_eq!(fs::read(&path).unwrap(), b"separate replacement bytes");
}

#[test]
fn oversize_cache_outputs_remain_bounded_and_geometry_is_checked_before_decode() {
    let directory = tempdir().unwrap();
    let snapshot = fixture(&directory.path().join("Budget source.mkv"));
    let mut configuration = budget();
    configuration.cache_bytes = 1;
    let mut cache = PictureCache::new(
        snapshot.clone(),
        configuration,
        Cancellation::new().unwrap(),
    )
    .unwrap();
    let actual = cache.picture(&request(&snapshot, 0, 0)).unwrap().unwrap();
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(cache.stats().live_bytes, 1024);
    drop(actual);
    assert_eq!(cache.stats().live_bytes, 0);
    let mut project = (**snapshot.project()).clone();
    if let StreamFormat::Video { width, height, .. } = &mut project.sources[0].streams[0].format {
        *width = 32;
        *height = 32;
    }
    let changed = Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap());
    let mut cache =
        PictureCache::new(changed.clone(), budget(), Cancellation::new().unwrap()).unwrap();
    assert!(cache.picture(&request(&changed, 0, 0)).is_err());
    assert_eq!(cache.stats().sources, 0);
    assert_eq!(cache.stats().live_bytes, 0);
    configuration.maximum_picture_bytes = 4096;
    configuration.live_bytes = 4096;
    let mut cache =
        PictureCache::new(changed.clone(), configuration, Cancellation::new().unwrap()).unwrap();
    assert!(cache.picture(&request(&changed, 0, 0)).is_err());
    assert_eq!(cache.stats().live_bytes, 0);
    assert_eq!(cache.stats().sequential_decodes, 0);
    configuration.live_bytes = 4095;
    assert!(configuration.validate().is_err());
}
