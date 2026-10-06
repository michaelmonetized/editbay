use super::*;
use crate::{PictureCache, SourceFile};
use editbay_core::{DocumentEditor, FrameRange, FrameRate, Project, SourcePosition};
use std::{fs, process::Command};
use tempfile::TempDir;

fn fixture() -> (
    TempDir,
    Arc<EvaluationSnapshot>,
    Vec<SourceRequest>,
    Vec<u8>,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Exact pictures.mkv");
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=16x16:rate=5:duration=1",
            "-c:v",
            "ffv1",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cancel = Cancellation::new().unwrap();
    let imported = SourceFile::open(&path, &cancel)
        .unwrap()
        .ingest("Stored native pictures".into(), &[0], cancel, |_, _| {})
        .unwrap();
    let mut project = Project::new("Prepared picture fixture").unwrap();
    project.assets.push(imported.asset);
    project.sources.push(imported.source);
    let commands = editbay_core::sequence_from_video(
        &project,
        project.sources[0].id,
        0,
        FrameRate::new(5, 1).unwrap(),
    )
    .unwrap();
    let mut editor = DocumentEditor::new(project).unwrap();
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Create sequence".into(),
            &commands,
        )
        .unwrap();
    let snapshot = Arc::new(EvaluationSnapshot::new(editor.snapshot()).unwrap());
    let composition = snapshot.project().compositions[0].id;
    let requests = (0..5)
        .map(|frame| {
            snapshot
                .prepare(composition, SourcePosition::new(frame, 1).unwrap(), false)
                .unwrap()
                .nodes[0]
                .source
                .clone()
                .unwrap()
        })
        .collect();
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&path)
        .args([
            "-map", "0:v:0", "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 5 * 1024);
    (directory, snapshot, requests, output.stdout)
}

fn plan(snapshot: Arc<EvaluationSnapshot>, budget: StoreBudget) -> Result<StorePlan> {
    StorePlan::new(
        snapshot.clone(),
        snapshot.project().compositions[0].id,
        FrameRange { start: 0, end: 5 },
        PictureBudget::default(),
        budget,
        &Cancellation::new().unwrap(),
    )
}

fn prepare(
    directory: &Path,
    snapshot: Arc<EvaluationSnapshot>,
    space: &StoreSpace,
) -> PreparedStore {
    let cancel = Cancellation::new().unwrap();
    let mut provider =
        PictureCache::new(snapshot.clone(), PictureBudget::default(), cancel.clone()).unwrap();
    plan(snapshot, StoreBudget::default())
        .unwrap()
        .prepare(directory, space, &mut provider, &cancel, |_| {})
        .unwrap()
}

#[test]
fn exact_disk_pixels_match_independent_decode_and_keep_storage_pinned() {
    let (directory, snapshot, requests, independent) = fixture();
    let space = StoreSpace::new(StoreBudget {
        bytes: 5120,
        entries: 5,
    })
    .unwrap();
    let store = prepare(directory.path(), snapshot.clone(), &space);
    assert_eq!(store.summary().pictures, 5);
    assert_eq!(space.usage().bytes, 5120);
    let budget = PictureBudget {
        cache_bytes: 0,
        cache_entries: 0,
        live_bytes: 1024,
        maximum_picture_bytes: 1024,
        ..PictureBudget::default()
    };
    let mut reader =
        PictureCache::new(snapshot.clone(), budget, Cancellation::new().unwrap()).unwrap();
    reader
        .attach_store(store.file().unwrap(), store.manifest())
        .unwrap();
    for index in [4, 0, 3, 1, 2, 4] {
        let result = reader.picture(&requests[index]).unwrap().unwrap();
        assert_eq!(
            result.picture.rgba(),
            &independent[index * 1024..(index + 1) * 1024]
        );
        assert!(
            reader.picture(&requests[0]).is_err(),
            "consumer pin must still charge live bytes"
        );
    }
    assert_eq!(reader.stats().stored_reads, 6);
    assert_eq!(reader.stats().decoders, 0);
    assert_eq!(reader.stats().seeks, 0);
    assert_eq!(reader.stats().live_bytes, 0);
    let pin = store.clone();
    drop(store);
    let mut provider =
        PictureCache::new(snapshot.clone(), budget, Cancellation::new().unwrap()).unwrap();
    assert!(
        plan(snapshot.clone(), StoreBudget::default())
            .unwrap()
            .prepare(
                directory.path(),
                &space,
                &mut provider,
                &Cancellation::new().unwrap(),
                |_| {}
            )
            .is_err()
    );
    reader.clear().unwrap();
    drop(pin);
    assert_eq!((space.usage().bytes, space.usage().entries), (0, 0));
    let store = prepare(directory.path(), snapshot.clone(), &space);
    reader
        .attach_store(store.file().unwrap(), store.manifest())
        .unwrap();
    reader.picture(&requests[0]).unwrap().unwrap();
    let original = &snapshot.project().assets[0].path;
    let writer = fs::OpenOptions::new().write(true).open(original).unwrap();
    writer.write_all_at(b"changed", 0).unwrap();
    assert!(
        matches!(reader.picture(&requests[0]), Err(Error::SourceChanged(_))),
        "prepared pixels cannot hide changed source bytes"
    );
    fs::remove_file(&snapshot.project().assets[0].path).unwrap();
    assert!(
        reader.picture(&requests[0]).is_err(),
        "stored pixels cannot hide a missing source"
    );
    assert_eq!(
        fs::read_dir(directory.path()).unwrap().count(),
        0,
        "anonymous packs leave no paths"
    );
}

#[test]
fn preparation_cancels_releases_partial_storage_and_rejects_foreign_providers() {
    let (directory, snapshot, _, _) = fixture();
    let space = StoreSpace::new(StoreBudget::default()).unwrap();
    let cancel = Cancellation::new().unwrap();
    let mut provider =
        PictureCache::new(snapshot.clone(), PictureBudget::default(), cancel.clone()).unwrap();
    let stopped = cancel.clone();
    let result = plan(snapshot.clone(), StoreBudget::default())
        .unwrap()
        .prepare(
            directory.path(),
            &space,
            &mut provider,
            &cancel,
            |progress| {
                if progress.pictures == 1 {
                    stopped.cancel();
                }
            },
        );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!((space.usage().bytes, space.usage().entries), (0, 0));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    let foreign =
        Arc::new(EvaluationSnapshot::new(Arc::new(Project::new("Other").unwrap())).unwrap());
    let mut provider = PictureCache::new(
        foreign,
        PictureBudget::default(),
        Cancellation::new().unwrap(),
    )
    .unwrap();
    assert!(
        plan(snapshot.clone(), StoreBudget::default())
            .unwrap()
            .prepare(
                directory.path(),
                &space,
                &mut provider,
                &Cancellation::new().unwrap(),
                |_| {}
            )
            .is_err()
    );
    assert!(
        plan(
            snapshot.clone(),
            StoreBudget {
                bytes: 5119,
                entries: 5
            }
        )
        .is_err()
    );
    assert!(
        plan(
            snapshot.clone(),
            StoreBudget {
                bytes: 5120,
                entries: 4
            }
        )
        .is_err()
    );
    assert!(
        StorePlan::new(
            snapshot.clone(),
            snapshot.project().compositions[0].id,
            FrameRange { start: 0, end: 5 },
            PictureBudget::default(),
            StoreBudget::default(),
            &cancel
        )
        .is_err()
    );
}

#[test]
fn readonly_anonymous_storage_rejects_foreign_malformed_corrupt_and_truncated_data() {
    let (directory, snapshot, requests, _) = fixture();
    let space = StoreSpace::new(StoreBudget::default()).unwrap();
    let store = prepare(directory.path(), snapshot.clone(), &space);
    let cancel = Cancellation::new().unwrap();
    let open = |file, manifest| {
        StoreReader::new(file, manifest, &snapshot, PictureBudget::default(), &cancel)
    };
    for change in 0..10 {
        let mut manifest = store.manifest();
        match change {
            0 => manifest.version.revision += 1,
            1 => manifest.entries[0].offset = 1,
            2 => manifest.entries[0].header.tick += 1,
            3 => manifest.entries[0].header.width += 1,
            4 => manifest.entries[1].request = manifest.entries[0].request.clone(),
            5 => manifest.bytes += 1,
            6 => manifest.entries[0].header.color.primaries += 1,
            7 => manifest.entries[0].header.alpha = editbay_core::AlphaMode::Premultiplied,
            8 => manifest.entries[0].header.rotation_degrees = 90.,
            _ => manifest.entries[0].header.alpha_interpretation_required = true,
        }
        assert!(open(store.file().unwrap(), manifest).is_err());
    }
    let fd = store.file().unwrap();
    let writer = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        .unwrap();
    assert!(open(writer.try_clone().unwrap(), store.manifest()).is_err());
    let named = directory.path().join("Named cache");
    fs::write(&named, vec![0; 5120]).unwrap();
    assert!(open(File::open(named).unwrap(), store.manifest()).is_err());
    let reader = open(fd, store.manifest()).unwrap();
    let selected = selection(&snapshot, &requests[0], PictureBudget::default())
        .unwrap()
        .unwrap();
    let mut bytes = vec![0; 1024];
    reader
        .read(&selected.key, &mut bytes, &cancel)
        .unwrap()
        .unwrap();
    writer.write_all_at(&[bytes[0] ^ 1], 0).unwrap();
    assert!(reader.read(&selected.key, &mut bytes, &cancel).is_err());
    writer.set_len(5119).unwrap();
    assert!(open(store.file().unwrap(), store.manifest()).is_err());
    let selected = selection(&snapshot, &requests[4], PictureBudget::default())
        .unwrap()
        .unwrap();
    assert!(reader.read(&selected.key, &mut bytes, &cancel).is_err());
}

#[test]
fn nested_reverse_plan_deduplicates_and_sorts_without_changing_time() {
    let (_directory, original, _, _) = fixture();
    let mut project = (**original.project()).clone();
    let original_id = project.compositions[0].id;
    let mut nested = project.compositions[0].clone();
    nested.id = uuid::Uuid::new_v4();
    nested.name = "Nested reverse".into();
    nested.tracks[0].id = uuid::Uuid::new_v4();
    nested.tracks[0].clips[0].id = uuid::Uuid::new_v4();
    nested.tracks[0].clips[0].source = editbay_core::ClipSource::Composition {
        composition: original_id,
    };
    nested.tracks[0].clips[0].time_map.points = vec![
        editbay_core::TimePoint {
            frame: 0,
            source_tick: 5,
        },
        editbay_core::TimePoint {
            frame: 5,
            source_tick: 0,
        },
    ];
    nested.nodes[0].id = uuid::Uuid::new_v4();
    nested.nodes[0].operation = editbay_core::NodeOperation::Source {
        clip: nested.tracks[0].clips[0].id,
    };
    nested.picture = Some(nested.nodes[0].id);
    let nested_id = nested.id;
    project.compositions.push(nested);
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap());
    let plan = StorePlan::new(
        snapshot,
        nested_id,
        FrameRange { start: 0, end: 5 },
        PictureBudget::default(),
        StoreBudget::default(),
        &Cancellation::new().unwrap(),
    )
    .unwrap();
    assert_eq!((plan.summary().pictures, plan.summary().bytes), (5, 5120));
    let ordinals: Vec<_> = plan
        .requests
        .values()
        .map(|r| match r {
            SourceRequest::Media { picture, .. } => picture.unwrap(),
            _ => panic!("unresolved dependency"),
        })
        .collect();
    assert_eq!(ordinals, vec![0, 1, 2, 3, 4]);
}
