use editbay_core::{DocumentEditor, DocumentVersion, PictureTiming, Project, StreamFormat};
use editbay_media::{Cancellation, Error, NativeAudioReader, SourceFile, StreamType, VideoReader};
use std::{path::Path, process::Command, time::Instant};
use tempfile::tempdir;

fn fixture(path: &Path) {
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=10:duration=1",
            "-f",
            "lavfi",
            "-i",
            "color=blue:size=32x16:rate=5:duration=1",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=0.1|0.2|-0.3|0.4|-0.5|0.6:s=44100:c=5.1:d=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=997:sample_rate=48000:duration=1",
            "-map",
            "0:v",
            "-map",
            "1:v",
            "-map",
            "2:a",
            "-map",
            "3:a",
            "-filter:v:0",
            "select='not(eq(n,2)+eq(n,5))'",
            "-fps_mode:v:0",
            "vfr",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_f32le",
            "-metadata",
            "reel_name=CAMERA_A",
            "-metadata:s:v:0",
            "timecode=01:02:03:04",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn explicit_streams_preserve_vfr_timecode_and_six_original_channels() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("multistream source.mkv");
    fixture(&path);
    let cancel = Cancellation::new().unwrap();
    let source = SourceFile::open(&path, &cancel).unwrap();
    let probe = source.probe(cancel.clone()).unwrap();
    assert_eq!(probe.streams.len(), 4);
    assert_eq!(
        probe
            .streams
            .iter()
            .filter(|v| v.kind == StreamType::Video)
            .count(),
        2
    );
    assert!(
        probe
            .metadata
            .iter()
            .any(|(key, value)| key.eq_ignore_ascii_case("reel_name") && value == "CAMERA_A")
    );
    assert!(
        probe.streams[0]
            .metadata
            .iter()
            .any(|(key, value)| key.eq_ignore_ascii_case("timecode") && value == "01:02:03:04")
    );
    assert_eq!(probe.streams[2].sample_rate, Some(44100));
    assert_eq!(
        probe.streams[2].channels,
        ["U0", "U1", "U2", "U3", "U4", "U5"]
    );
    assert!(!probe.streams[2].channel_order_declared);
    let index = source
        .index_video(&probe.streams[0], cancel.clone(), |_| {})
        .unwrap();
    assert_eq!(
        index.presentation_ticks,
        [0, 100, 300, 400, 600, 700, 800, 900]
    );
    assert_eq!(index.end_tick, 1000);
    let mut selected = VideoReader::open_stream(&source, 1, cancel.clone()).unwrap();
    assert_eq!((selected.info.width, selected.info.height), (32, 16));
    let blue = selected.next_frame().unwrap().unwrap();
    assert!(
        blue.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[2] > 240 && pixel[0] < 10 && pixel[1] < 10)
    );
    assert!(VideoReader::open_stream(&source, 2, cancel.clone()).is_err());
    assert!(NativeAudioReader::open_stream(&source, 1, cancel.clone()).is_err());
    assert!(VideoReader::open_stream(&source, 77, cancel.clone()).is_err());
    let mut audio = NativeAudioReader::open_stream(&source, 2, cancel.clone()).unwrap();
    assert_eq!(audio.info.sample_rate, 44100);
    assert_eq!(audio.info.channels, 6);
    let mut frames = 0;
    while let Some(block) = audio.next_block().unwrap() {
        assert_eq!(block.first_sample, Some(frames));
        for frame in block.samples.as_chunks::<6>().0 {
            for (actual, expected) in frame.iter().zip([0.1, 0.2, -0.3, 0.4, -0.5, 0.6]) {
                assert!((*actual - expected).abs() < 0.000001);
            }
        }
        frames += (block.samples.len() / 6) as i64;
    }
    assert_eq!(frames, 44100);
    assert!(audio.next_block().unwrap().is_none());
    let ingested = source
        .ingest("Camera A".into(), &[0, 1, 2, 3], cancel.clone(), |_, _| {})
        .unwrap();
    assert_eq!(ingested.source.streams.len(), 4);
    let video = &ingested.source.streams[0];
    assert_eq!(video.start_tick, 0);
    assert_eq!(video.duration_ticks, Some(1000));
    assert!(
        matches!(&video.format, StreamFormat::Video { timing: PictureTiming::Variable { presentation_ticks, .. }, .. } if presentation_ticks == &index.presentation_ticks)
    );
    assert_eq!(ingested.source.streams[2].time_base.denominator, 44100);
    assert_eq!(ingested.source.streams[2].duration_ticks, Some(44100));
    let mut editor = DocumentEditor::new(Project::new("Native ingest").unwrap()).unwrap();
    let initial = DocumentVersion::of(editor.project());
    editor
        .apply(initial, "Import source".into(), &ingested.commands())
        .unwrap();
    assert_eq!(editor.project().sources[0], ingested.source);
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert!(editor.project().sources.is_empty() && editor.project().assets.is_empty());
    editor.redo(DocumentVersion::of(editor.project())).unwrap();
    source.verify(&cancel).unwrap();
    assert!(
        source
            .ingest("Duplicate".into(), &[0, 0], cancel, |_, _| {})
            .is_err()
    );
}

#[test]
fn declared_wav_channel_order_and_float_headroom_survive_native_decode() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("surround.wav");
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=0.1|0.2|-0.3|0.4|-0.5|2.5:s=96000:c=5.1:d=0.1",
            "-c:a",
            "pcm_f32le",
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
    let source = SourceFile::open(&path, &cancel).unwrap();
    let probe = source.probe(cancel.clone()).unwrap();
    assert_eq!(
        probe.streams[0].channels,
        ["FL", "FR", "FC", "LFE", "BL", "BR"]
    );
    assert!(probe.streams[0].channel_order_declared);
    let mut reader = NativeAudioReader::open_stream(&source, 0, cancel.clone()).unwrap();
    assert_eq!((reader.info.sample_rate, reader.info.channels), (96000, 6));
    let mut count = 0;
    while let Some(block) = reader.next_block().unwrap() {
        assert_eq!(block.first_sample, Some(count));
        for frame in block.samples.as_chunks::<6>().0 {
            for (actual, expected) in frame.iter().zip([0.1, 0.2, -0.3, 0.4, -0.5, 2.5]) {
                assert!((*actual - expected).abs() < 0.000001);
            }
        }
        count += (block.samples.len() / 6) as i64;
    }
    assert_eq!(count, 9600);
    let ingested = source
        .ingest("Surround".into(), &[0], cancel, |_, _| {})
        .unwrap();
    assert_eq!(ingested.source.streams[0].duration_ticks, Some(9600));
}

#[test]
fn seeking_after_drain_and_path_replacement_never_substitutes_media() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("owned.mkv");
    fixture(&path);
    let cancel = Cancellation::new().unwrap();
    let source = SourceFile::open(&path, &cancel).unwrap();
    let mut reader = VideoReader::open_stream(&source, 0, cancel.clone()).unwrap();
    let mut pictures = Vec::new();
    while let Some(frame) = reader.next_frame().unwrap() {
        pictures.push(frame);
    }
    for index in [7, 0, 4, 1, 6, 2, 3, 5] {
        let original = &pictures[index];
        let sought = reader.frame_at(original.source_tick.unwrap()).unwrap();
        assert_eq!(sought.rgba, original.rgba);
    }
    assert!(reader.frame_at(200).is_err());
    assert_eq!(reader.frame_at(0).unwrap().rgba, pictures[0].rgba);
    let mut audio = NativeAudioReader::open_stream(&source, 2, cancel.clone()).unwrap();
    while audio.next_block().unwrap().is_some() {}
    audio.seek(0).unwrap();
    assert_eq!(audio.next_block().unwrap().unwrap().first_sample, Some(0));
    std::fs::rename(&path, directory.path().join("original.mkv")).unwrap();
    std::fs::write(&path, b"replacement media").unwrap();
    assert_eq!(reader.frame_at(300).unwrap().rgba, pictures[2].rgba);
    assert!(matches!(
        source.verify(&cancel),
        Err(Error::SourceChanged(_))
    ));
    assert!(matches!(source.probe(cancel), Err(Error::SourceChanged(_))));
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement media");
}

#[test]
fn native_cancellation_and_secondary_resource_denial_are_real() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("source.mkv");
    fixture(&path);
    let cancel = Cancellation::new().unwrap();
    let source = SourceFile::open(&path, &cancel).unwrap();
    let mut reader = VideoReader::open_stream(&source, 0, cancel.clone()).unwrap();
    reader.next_frame().unwrap().unwrap();
    let started = Instant::now();
    cancel.cancel();
    assert!(matches!(reader.next_frame(), Err(Error::Cancelled)));
    assert!(matches!(reader.seek(0), Err(Error::Cancelled)));
    assert!(started.elapsed().as_secs_f64() < 2.);
    assert!(matches!(
        SourceFile::open(&path, &cancel),
        Err(Error::Cancelled)
    ));
    let playlist = directory.path().join("source.ffconcat");
    std::fs::write(&playlist, "ffconcat version 1.0\nfile 'source.mkv'\n").unwrap();
    let token = Cancellation::new().unwrap();
    let resource = SourceFile::open(&playlist, &token).unwrap();
    assert!(resource.probe(token).is_err());
    assert!(VideoReader::open(directory.path()).is_err());
}

#[test]
fn single_still_sources_are_real_decoded_pictures_with_original_alpha() {
    let directory = tempdir().unwrap();
    for extension in ["png", "jpg"] {
        let path = directory.path().join(format!("original.{extension}"));
        let mut command = Command::new("ffmpeg");
        command.args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red@0.5:s=16x16:d=0.04,format=rgba",
            "-frames:v",
            "1",
        ]);
        if extension == "png" {
            command.args(["-pix_fmt", "rgba"]);
        }
        let output = command.arg(&path).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let original = std::fs::read(&path).unwrap();
        let cancel = Cancellation::new().unwrap();
        let source = SourceFile::open(&path, &cancel).unwrap();
        let probe = source.probe(cancel.clone()).unwrap();
        assert_eq!(probe.streams.len(), 1);
        assert!(probe.streams[0].decoder_available);
        let mut reader = VideoReader::open_stream(&source, 0, cancel.clone()).unwrap();
        let picture = reader.next_frame().unwrap().unwrap();
        assert_eq!((reader.info.width, reader.info.height), (16, 16));
        assert!(reader.next_frame().unwrap().is_none());
        for pixel in picture.rgba.as_chunks::<4>().0 {
            assert!(pixel[0] > 240 && pixel[1] < 10 && pixel[2] < 10);
            if extension == "png" {
                assert!((126..=128).contains(&pixel[3]));
            } else {
                assert_eq!(pixel[3], 255);
            }
        }
        let imported = source
            .ingest("Still".into(), &[0], cancel.clone(), |_, _| {})
            .unwrap();
        assert!(matches!(&imported.source.streams[0].format,
            StreamFormat::Video { timing: PictureTiming::Variable { presentation_ticks, .. }, .. }
            if presentation_ticks.len() == 1));
        source.verify(&cancel).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}
