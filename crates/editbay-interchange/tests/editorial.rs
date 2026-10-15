use editbay_core as eb;
use editbay_interchange::*;
use std::io::Write;

/// Keep generated originals when a receipt directory was requested.
/// `name` identifies this case; returns its private directory, temporary otherwise.
fn fixture_dir(name: &str) -> tempfile::TempDir {
    if let Some(root) = std::env::var_os("EDITBAY_WORKFLOW_RECEIPTS_DIR") {
        std::fs::create_dir_all(&root).unwrap();
        tempfile::Builder::new()
            .prefix(&format!("inputs-{name}-"))
            .disable_cleanup(true)
            .tempdir_in(root)
            .unwrap()
    } else {
        tempfile::tempdir().unwrap()
    }
}

/// Retain the editable input and its verified member identities for an external runner.
/// `project`, `name`, and `directory` select the fixture; returns after writing only when requested.
fn record_fixture(project: &eb::Project, name: &str, directory: &std::path::Path) {
    if std::env::var_os("EDITBAY_WORKFLOW_RECEIPTS_DIR").is_some() {
        let native = directory.join("native.editbay");
        eb::save_new(project, &native).unwrap();
        std::fs::write(
            directory.join("fixture.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "editbay.editorial.fixture.v1",
                "case": name,
                "license": "MIT; original source-generated fixture",
                "native_project": native,
                "members": project.assets.iter().map(|a| serde_json::json!({
                    "path": a.path, "bytes": a.bytes, "sha256": a.sha256
                })).collect::<Vec<_>>()
            }))
            .unwrap(),
        )
        .unwrap();
    }
}
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
/// Build direct editable clips over two seconds of verified native media.
/// `media` owns source metadata and fingerprint; returns one linked 24fps program.
fn direct_project(media: editbay_media::IngestedSource) -> eb::Project {
    let mut p = eb::Project::new("Editorial exact time").unwrap();
    let rate = eb::FrameRate::new(24, 1).unwrap();
    let id = uuid::Uuid::new_v4();
    let range = eb::FrameRange { start: 0, end: 48 };
    let mut tracks = vec![];
    for stream in &media.source.streams {
        let kind = match stream.format {
            eb::StreamFormat::Video { .. } => eb::TrackKind::Video,
            eb::StreamFormat::Audio { .. } => eb::TrackKind::Audio,
        };
        let end = eb::TimeBase {
            numerator: 1,
            denominator: 24,
        }
        .at_rate(
            eb::SourcePosition::new(48, 1).unwrap(),
            eb::FrameRate::new(stream.time_base.denominator, stream.time_base.numerator).unwrap(),
        )
        .unwrap();
        assert_eq!(end.denominator, 1);
        tracks.push(eb::Track {
            id: uuid::Uuid::new_v4(),
            name: format!("{kind:?}"),
            kind,
            enabled: true,
            clips: vec![eb::Clip {
                id: uuid::Uuid::new_v4(),
                name: media.source.name.clone(),
                range,
                source: eb::ClipSource::Media {
                    source: media.source.id,
                    stream: stream.index,
                },
                linked: None,
                time_map: eb::TimeMap {
                    source_denominator: 1,
                    points: vec![
                        eb::TimePoint {
                            frame: 0,
                            source_tick: stream.start_tick,
                        },
                        eb::TimePoint {
                            frame: 48,
                            source_tick: stream.start_tick + end.numerator,
                        },
                    ],
                },
            }],
        });
    }
    if tracks.len() == 2 {
        let a = tracks[0].clips[0].id;
        let b = tracks[1].clips[0].id;
        tracks[0].clips[0].linked = Some(b);
        tracks[1].clips[0].linked = Some(a);
    }
    let mut scene = eb::Composition {
        id,
        name: "Program".into(),
        width: 16,
        height: 16,
        frame_rate: rate,
        duration: 48,
        tracks,
        nodes: vec![],
        picture: None,
        audio: None,
    };
    eb::rebuild_editorial(&mut scene, &[]).unwrap();
    p.assets.push(media.asset);
    p.sources.push(media.source);
    p.compositions.push(scene);
    p.sequences[0].width = 16;
    p.sequences[0].height = 16;
    p.sequences[0].frame_rate = rate;
    p.sequences[0].composition = Some(id);
    p.validate().unwrap();
    p
}
fn project(path: &std::path::Path) -> eb::Project {
    let cancel = editbay_media::Cancellation::new().unwrap();
    let file = editbay_media::SourceFile::open(path, &cancel).unwrap();
    direct_project(
        file.ingest("Interview 音声".into(), &[0], cancel, |_, _| {})
            .unwrap(),
    )
}
#[test]
fn real_audio_all_formats_and_saved_relinked_source() {
    let dir = fixture_dir("audio");
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
    eb::save_new(&p, &saved).unwrap();
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
    record_fixture(editor.project(), "audio-reverse-relinked", dir.path());
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
        assert!(
            !imported.sources.is_empty(),
            "{format:?} retained no sources: {loss:?}; export: {report:?}"
        );
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
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .next()
                .unwrap_or_else(|| panic!("{format:?} retained no audio clip: {loss:?}"));
            assert_eq!(clip.range.start, 6, "{format:?}");
            assert_eq!(clip.range.end, 30, "{format:?}");
            if format != Format::Aaf {
                assert_eq!(
                    clip.time_map.direction_at(0).unwrap(),
                    -1,
                    "{format:?} audio reverse"
                );
                assert_eq!(
                    clip.time_map.position(0).unwrap(),
                    eb::SourcePosition::new(48048, 1).unwrap(),
                    "{format:?} audio source start"
                );
                assert_eq!(
                    clip.time_map.position(24).unwrap(),
                    eb::SourcePosition::new(0, 1).unwrap(),
                    "{format:?} audio source end"
                );
            }
        }
        let reopened = dir.path().join(format!("{:?}.editbay", format));
        eb::save_new(&imported, &reopened).unwrap();
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

fn av_project(path: &std::path::Path) -> eb::Project {
    let profile = editbay_media::LosslessMovProfile {
        width: 16,
        height: 16,
        frame_rate: eb::FrameRate::new(24, 1).unwrap(),
        first_frame: 0,
        frames: 48,
        sample_rate: 48000,
        channels: vec!["FC".into()],
    };
    let file = std::fs::File::create(path).unwrap();
    let token = editbay_media::Cancellation::new().unwrap();
    let mut writer =
        editbay_media::LosslessMovWriter::new(&file, profile.clone(), token.clone()).unwrap();
    let mut first = 0;
    for frame in 0..48 {
        writer
            .picture(frame, &[64, 128, 192, 255].repeat(256))
            .unwrap();
        let end = profile.samples_through(frame + 1).unwrap();
        writer
            .sound(first, &vec![0.; (end - first) as usize])
            .unwrap();
        first = end;
    }
    writer.finish().unwrap();
    file.sync_all().unwrap();
    let source = editbay_media::SourceFile::open(path, &token)
        .unwrap()
        .ingest("Linked original".into(), &[0, 1], token, |_, _| {})
        .unwrap();
    let mut p = direct_project(source);
    let scene = &mut p.compositions[0];
    scene.duration = 28;
    for (index, track) in scene.tracks.iter_mut().enumerate() {
        let template = track.clips[0].clone();
        track.clips.clear();
        for (cut, range) in [
            eb::FrameRange { start: 4, end: 12 },
            eb::FrameRange { start: 16, end: 24 },
        ]
        .into_iter()
        .enumerate()
        {
            let mut c = template.clone();
            c.id = uuid::Uuid::from_u128(1000 + cut as u128 * 2 + index as u128);
            c.linked = Some(uuid::Uuid::from_u128(
                1000 + cut as u128 * 2 + (1 - index) as u128,
            ));
            c.range = range;
            let (a, b) = if cut == 0 { (4, 12) } else { (32, 24) };
            let a = template.time_map.position(a).unwrap();
            let b = template.time_map.position(b).unwrap();
            let d = a.denominator.max(b.denominator);
            c.time_map = eb::TimeMap {
                source_denominator: d.try_into().unwrap(),
                points: vec![
                    eb::TimePoint {
                        frame: 0,
                        source_tick: a.numerator * (d / a.denominator) as i64,
                    },
                    eb::TimePoint {
                        frame: 8,
                        source_tick: b.numerator * (d / b.denominator) as i64,
                    },
                ],
            };
            track.clips.push(c);
        }
    }
    eb::rebuild_editorial(scene, &[]).unwrap();
    p.validate().unwrap();
    p
}
#[test]
fn linked_av_reverse_internal_and_trailing_gaps_reopen_exactly_in_capable_formats() {
    let dir = fixture_dir("linked");
    let original = dir.path().join("linked.mov");
    let p = av_project(&original);
    let composition = p.compositions[0].id;
    record_fixture(&p, "linked", dir.path());
    for format in [
        Format::Edl,
        Format::Fcp7Xml,
        Format::Fcpxml,
        Format::Otio,
        Format::Aaf,
        Format::Omf,
    ] {
        let (bytes, report) =
            export_document(&p, composition, format, &ExportOptions::default()).unwrap();
        let options = ImportOptions {
            base_dir: Some(dir.path().to_str().unwrap().into()),
            edl_frame_rate: Some(eb::FrameRate::new(24, 1).unwrap()),
            ..Default::default()
        };
        let (imported, loss) = import_document(
            &bytes,
            format,
            &options,
            &editbay_media::Cancellation::new().unwrap(),
        )
        .unwrap_or_else(|e| panic!("{format:?}: {e}; {report:?}"));
        let scene = imported
            .compositions
            .iter()
            .find(|c| Some(c.id) == imported.sequences[0].composition)
            .unwrap();
        assert_eq!(scene.duration, 28, "{format:?} dropped trailing gap");
        let clips: Vec<_> = scene
            .tracks
            .iter()
            .filter(|t| t.kind == eb::TrackKind::Video)
            .flat_map(|t| &t.clips)
            .collect();
        if format != Format::Omf {
            assert_eq!(clips.len(), 2, "{format:?}");
            assert_eq!(clips[0].range, eb::FrameRange { start: 4, end: 12 });
            assert_eq!(clips[1].range, eb::FrameRange { start: 16, end: 24 });
        } else {
            assert!(
                report.mentions("picture tracks"),
                "OMF picture loss must be declared: {report:?}"
            );
        }
        if !matches!(format, Format::Aaf | Format::Omf) {
            let audio = scene
                .tracks
                .iter()
                .filter(|t| t.kind == eb::TrackKind::Audio)
                .flat_map(|t| &t.clips)
                .collect::<Vec<_>>();
            assert_eq!(audio.len(), 2, "{format:?} audio clips");
            assert_eq!(
                audio[1].time_map.direction_at(0).unwrap(),
                -1,
                "{format:?} linked audio reverse"
            );
            assert!(
                scene
                    .tracks
                    .iter()
                    .flat_map(|t| &t.clips)
                    .all(|c| c.linked.is_some()),
                "{format:?} links: {loss:?}"
            );
            assert_eq!(
                clips[1].time_map.direction_at(0).unwrap(),
                -1,
                "{format:?} reverse"
            );
            let stream = match clips[1].source {
                eb::ClipSource::Media { source, stream } => &imported
                    .sources
                    .iter()
                    .find(|s| s.id == source)
                    .unwrap()
                    .streams
                    .iter()
                    .find(|s| s.index == stream)
                    .unwrap(),
                _ => panic!("media expected"),
            };
            for (local, expected) in [(0, 32), (8, 24)] {
                assert_eq!(
                    stream
                        .time_base
                        .at_rate(
                            clips[1]
                                .time_map
                                .position(local)
                                .unwrap()
                                .relative_to(stream.start_tick)
                                .unwrap(),
                            eb::FrameRate::new(24, 1).unwrap()
                        )
                        .unwrap(),
                    eb::SourcePosition::new(expected, 1).unwrap(),
                    "{format:?} source boundary"
                );
            }
        } else {
            assert!(
                report.has_warnings(),
                "Binary audio retiming loss must be reported"
            );
        }
        if let Some(root) = std::env::var_os("EDITBAY_WORKFLOW_RECEIPTS_DIR") {
            let root = std::path::PathBuf::from(root);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join(format!("linked.{}", format.extension())), &bytes).unwrap();
            std::fs::write(
                root.join(format!("linked.{}.loss.json", format.extension())),
                serde_json::to_vec_pretty(&report).unwrap(),
            )
            .unwrap();
        }
    }
}

#[test]
fn aaf_picture_speed_reverse_repeat_use_standard_operations_and_exact_source_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("camera.mov");
    let original = av_project(&path);
    for (a, b) in [(4, 20), (20, 4), (12, 12)] {
        let mut project = original.clone();
        let scene = &mut project.compositions[0];
        let video = scene
            .tracks
            .iter_mut()
            .find(|t| t.kind == eb::TrackKind::Video)
            .unwrap();
        let clip = &mut video.clips[1];
        let map = clip.time_map.clone();
        let source = match clip.source {
            eb::ClipSource::Media { source, stream } => project
                .sources
                .iter()
                .find(|s| s.id == source)
                .unwrap()
                .streams
                .iter()
                .find(|s| s.index == stream)
                .unwrap(),
            _ => unreachable!(),
        };
        let position = |frame: i64| {
            eb::TimeBase {
                numerator: 1,
                denominator: 24,
            }
            .at_rate(
                eb::SourcePosition::new(frame, 1).unwrap(),
                eb::FrameRate::new(source.time_base.denominator, source.time_base.numerator)
                    .unwrap(),
            )
            .unwrap()
        };
        let (start, end) = (position(a), position(b));
        let denominator = start.denominator.max(end.denominator);
        clip.time_map = eb::TimeMap {
            source_denominator: denominator.try_into().unwrap(),
            points: vec![
                eb::TimePoint {
                    frame: 0,
                    source_tick: start.numerator * (denominator / start.denominator) as i64
                        + source.start_tick * denominator as i64,
                },
                eb::TimePoint {
                    frame: 8,
                    source_tick: end.numerator * (denominator / end.denominator) as i64
                        + source.start_tick * denominator as i64,
                },
            ],
        };
        assert_ne!(clip.time_map, map);
        let composition = scene.id;
        eb::rebuild_editorial(scene, &[]).unwrap();
        project.validate().unwrap();
        let (bytes, _) =
            export_document(&project, composition, Format::Aaf, &Default::default()).unwrap();
        let op = [
            if a == b { 0x91 } else { 0x90 },
            0xa8,
            0x2e,
            0x9d,
            0x68,
            0x09,
            0xd3,
            0x11,
            0x8a,
            0x38,
            0x00,
            0x50,
            0x04,
            0x0e,
            0xf7,
            0xd2,
        ];
        assert!(
            bytes.windows(16).any(|w| w == op),
            "AAF must contain standard operation AUID"
        );
        let (imported, report) = import_document(
            &bytes,
            Format::Aaf,
            &ImportOptions::default(),
            &editbay_media::Cancellation::new().unwrap(),
        )
        .unwrap();
        let scene = imported
            .compositions
            .iter()
            .find(|c| Some(c.id) == imported.sequences[0].composition)
            .unwrap();
        let video = scene
            .tracks
            .iter()
            .find(|t| t.kind == eb::TrackKind::Video)
            .unwrap();
        let clip = &video.clips[1];
        let source = match clip.source {
            eb::ClipSource::Media { source, stream } => imported
                .sources
                .iter()
                .find(|s| s.id == source)
                .unwrap()
                .streams
                .iter()
                .find(|s| s.index == stream)
                .unwrap(),
            _ => unreachable!(),
        };
        for (local, expected) in [(0, a), (8, b)] {
            let time = source
                .time_base
                .at_rate(
                    clip.time_map
                        .position(local)
                        .unwrap()
                        .relative_to(source.start_tick)
                        .unwrap(),
                    eb::FrameRate::new(24, 1).unwrap(),
                )
                .unwrap();
            assert_eq!(
                time,
                eb::SourcePosition::new(expected, 1).unwrap(),
                "AAF {a}→{b}: {report:?}"
            );
        }
    }
}

#[test]
fn dissolve_handles_round_trip_in_all_editorial_formats() {
    let dir = fixture_dir("dissolve");
    let mut project = av_project(&dir.path().join("dissolve.mov"));
    let scene = &mut project.compositions[0];
    let mut transitions = vec![];
    for track in &mut scene.tracks {
        for (index, clip) in track.clips.iter_mut().enumerate() {
            let (range, first, last) = if index == 0 {
                (eb::FrameRange { start: 4, end: 18 }, 0, 14)
            } else {
                (eb::FrameRange { start: 14, end: 24 }, 20, 30)
            };
            let eb::ClipSource::Media { source, stream } = clip.source else {
                unreachable!()
            };
            let source = project.sources.iter().find(|s| s.id == source).unwrap();
            let stream = source.streams.iter().find(|s| s.index == stream).unwrap();
            let convert = |frame: i64| {
                eb::TimeBase {
                    numerator: 1,
                    denominator: 24,
                }
                .at_rate(
                    eb::SourcePosition::new(frame, 1).unwrap(),
                    eb::FrameRate::new(stream.time_base.denominator, stream.time_base.numerator)
                        .unwrap(),
                )
                .unwrap()
            };
            let (a, b) = (convert(first), convert(last));
            assert_eq!(a.denominator, 1);
            assert_eq!(b.denominator, 1);
            clip.range = range;
            clip.time_map = eb::TimeMap {
                source_denominator: 1,
                points: vec![
                    eb::TimePoint {
                        frame: 0,
                        source_tick: stream.start_tick + a.numerator,
                    },
                    eb::TimePoint {
                        frame: range.end - range.start,
                        source_tick: stream.start_tick + b.numerator,
                    },
                ],
            };
        }
        transitions.push(eb::EditorialTransition {
            id: uuid::Uuid::new_v4(),
            composition: scene.id,
            range: eb::FrameRange { start: 14, end: 18 },
            cut: 16,
            from: Some(track.clips[0].id),
            to: Some(track.clips[1].id),
        });
    }
    eb::rebuild_editorial(scene, &transitions).unwrap();
    project.transitions = transitions;
    project.validate().unwrap();
    record_fixture(&project, "dissolve", dir.path());
    let composition = project.compositions[0].id;
    let mut failures = Vec::new();
    for format in Format::ALL {
        let result = (|| -> std::result::Result<(), String> {
            let (bytes, report) =
                export_document(&project, composition, format, &Default::default())
                    .map_err(|error| error.to_string())?;
            if let Some(root) = std::env::var_os("EDITBAY_WORKFLOW_RECEIPTS_DIR") {
                let root = std::path::PathBuf::from(root);
                std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
                std::fs::write(
                    root.join(format!("dissolve.{}", format.extension())),
                    &bytes,
                )
                .map_err(|error| error.to_string())?;
                std::fs::write(
                    root.join(format!("dissolve.{}.loss.json", format.extension())),
                    serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
            }
            let (imported, loss) = import_document(
                &bytes,
                format,
                &ImportOptions {
                    edl_frame_rate: Some(eb::FrameRate::new(24, 1).unwrap()),
                    ..Default::default()
                },
                &editbay_media::Cancellation::new().unwrap(),
            )
            .map_err(|error| format!("transition import: {error}; {report:?}"))?;
            if imported.transitions.is_empty() {
                return Err(format!("transition absent: {loss:?}"));
            }
            for transition in &imported.transitions {
                if transition.range != (eb::FrameRange { start: 14, end: 18 })
                    || transition.cut != 16
                    || transition.from.is_none()
                    || transition.to.is_none()
                {
                    return Err(format!(
                        "transition boundaries or endpoints changed: {transition:?}; {loss:?}"
                    ));
                }
            }
            let path = dir
                .path()
                .join(format!("transition-{}.editbay", format.extension()));
            eb::save_new(&imported, &path).map_err(|error| error.to_string())?;
            if eb::load(path).map_err(|error| error.to_string())? != imported {
                return Err("native save/reopen changed the imported edit".into());
            }
            Ok(())
        })();
        if let Err(error) = result {
            failures.push(format!("{format:?}: {error}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
