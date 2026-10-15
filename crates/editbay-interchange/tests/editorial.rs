use editbay_core as eb;
use editbay_interchange::*;
use std::io::Write;
fn wav(path: &std::path::Path) {
    let samples = 96_000u32;
    let bytes = samples * 2;
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + bytes).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&48000u32.to_le_bytes()).unwrap();
    f.write_all(&96000u32.to_le_bytes()).unwrap();
    f.write_all(&2u16.to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&bytes.to_le_bytes()).unwrap();
    f.write_all(&vec![0; bytes as usize]).unwrap();
}
fn project(path: &std::path::Path) -> eb::Project {
    let cancel = editbay_media::Cancellation::new().unwrap();
    let file = editbay_media::SourceFile::open(path, &cancel).unwrap();
    let media = file
        .ingest("Interview 音声".into(), &[0], cancel, |_, _| {})
        .unwrap();
    let mut p = eb::Project::new("Editorial exact time").unwrap();
    p.assets.push(media.asset);
    p.sources.push(media.source);
    let commands = eb::sequence_from_audio(&p, p.sources[0].id, 0).unwrap();
    let mut editor = eb::DocumentEditor::new(p).unwrap();
    editor
        .apply(
            eb::DocumentVersion::of(editor.project()),
            "source".into(),
            &commands,
        )
        .unwrap();
    editor.project().clone()
}
#[test]
fn real_audio_all_formats_and_saved_relinked_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("Original 音声.wav");
    wav(&source);
    let mut p = project(&source);
    let scene = &mut p.compositions[0];
    scene.frame_rate = eb::FrameRate::new(24000, 1001).unwrap();
    scene.duration = 36;
    scene.tracks[0].clips[0].range = eb::FrameRange { start: 6, end: 30 };
    scene.tracks[0].clips[0].time_map = eb::TimeMap {
        source_denominator: 1,
        points: vec![
            eb::TimePoint {
                frame: 0,
                source_tick: 48048,
            },
            eb::TimePoint {
                frame: 24,
                source_tick: 0,
            },
        ],
    };
    eb::rebuild_editorial(scene, &[]).unwrap();
    p.sequences[0].frame_rate = scene.frame_rate;
    let composition = scene.id;
    p.validate().unwrap();
    let saved = dir.path().join("cut.editbay");
    eb::save_new(&saved, &p).unwrap();
    let p = eb::load(&saved).unwrap();
    let moved = dir.path().join("Moved.wav");
    std::fs::rename(&source, &moved).unwrap();
    let command = relink(
        &p,
        p.assets[0].id,
        &moved,
        &editbay_media::Cancellation::new().unwrap(),
    )
    .unwrap();
    let mut editor = eb::DocumentEditor::new(p).unwrap();
    editor
        .apply(
            eb::DocumentVersion::of(editor.project()),
            "Relink".into(),
            &[command],
        )
        .unwrap();
    for format in Format::ALL {
        let (bytes, report) = export_document(
            editor.project(),
            composition,
            format,
            &ExportOptions::default(),
        )
        .unwrap();
        assert!(!bytes.is_empty());
        if format == Format::Otio {
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(
                value["OTIO_SCHEMA"]
                    .as_str()
                    .unwrap()
                    .starts_with("Timeline.")
            );
            assert!(String::from_utf8_lossy(&bytes).contains("Moved.wav"));
        }
        if format == Format::Aaf {
            assert_eq!(
                &bytes[..8],
                &[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]
            );
        }
        let options = ImportOptions {
            base_dir: Some(dir.path().to_str().unwrap().into()),
            edl_frame_rate: Some(eb::FrameRate::new(24000, 1001).unwrap()),
            ..Default::default()
        };
        let (imported, loss) = import_document(
            &bytes,
            format,
            &options,
            &editbay_media::Cancellation::new().unwrap(),
        )
        .unwrap_or_else(|e| panic!("{format:?}: {e}; export report {report:?}"));
        imported.validate().unwrap();
        assert_eq!(
            imported.sources[0].streams[0].time_base,
            editor.project().sources[0].streams[0].time_base
        );
        assert!(!loss.entries.iter().any(|e| e.message.is_empty()));
        if format != Format::Omf {
            let clip = &imported
                .compositions
                .iter()
                .find(|c| c.id == imported.sequences[0].composition.unwrap())
                .unwrap()
                .tracks[0]
                .clips[0];
            assert_eq!(clip.range.start, 6, "{format:?}");
            assert_eq!(clip.range.end, 30, "{format:?}");
        }
        let reopened = dir.path().join(format!("{:?}.editbay", format));
        eb::save_new(&reopened, &imported).unwrap();
        assert_eq!(eb::load(&reopened).unwrap(), imported);
    }
}
#[test]
fn relink_rejects_changed_bytes_and_missing_original() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.wav");
    wav(&path);
    let p = project(&path);
    std::fs::write(&path, b"changed").unwrap();
    assert!(
        relink(
            &p,
            p.assets[0].id,
            &path,
            &editbay_media::Cancellation::new().unwrap()
        )
        .is_err()
    );
    assert!(
        relink(
            &p,
            p.assets[0].id,
            &dir.path().join("missing"),
            &editbay_media::Cancellation::new().unwrap()
        )
        .is_err()
    );
}
