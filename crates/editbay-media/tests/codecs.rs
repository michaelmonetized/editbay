use editbay_media::{AudioReader, MediaInfo, VideoReader, VideoWriter};
use std::process::Command;
use tempfile::tempdir;

#[test]
fn lossless_export_preserves_unaligned_rgba_and_rational_timestamps() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("unaligned picture.mkv");
    let profile = MediaInfo {
        width: 257,
        height: 17,
        rate_num: 30000,
        rate_den: 1001,
        ..Default::default()
    };
    let mut writer = VideoWriter::open_temporary(&path, profile).unwrap();
    let originals: Vec<Vec<u8>> = (0..7)
        .map(|frame| {
            (0..257 * 17 * 4)
                .map(|i| ((i * 17 + frame * 29) % 256) as u8)
                .collect()
        })
        .collect();
    assert!(writer.write(&[0; 8]).is_err());
    for original in &originals {
        writer.write(original).unwrap();
    }
    writer.finish().unwrap();
    let mut decoder = VideoReader::open(&path).unwrap();
    assert_eq!(decoder.info.width, 257);
    assert_eq!(decoder.info.rate_num, 30000);
    assert_eq!(decoder.info.rate_den, 1001);
    for (index, original) in originals.iter().enumerate() {
        let decoded = decoder.next_frame().unwrap().unwrap();
        assert_eq!(&decoded.rgba, original);
        let expected = index as i64 * 1001 * 1_000_000_000 / 30000;
        assert!((decoded.timestamp_ns.unwrap() - expected).abs() <= 500_001);
    }
    assert!(decoder.next_frame().unwrap().is_none());
    assert!(decoder.next_frame().unwrap().is_none());
}

#[test]
fn delayed_video_and_resampled_sound_drain_without_losing_the_tail() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("delayed sound and picture.mkv");
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=30000/1001",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100",
            "-t",
            "1.001",
            "-c:v",
            "libx264",
            "-bf",
            "3",
            "-c:a",
            "pcm_s16le",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut decoder = VideoReader::open(&path).unwrap();
    let mut frames = 0;
    let mut previous = None;
    while let Some(frame) = decoder.next_frame().unwrap() {
        let timestamp = frame.timestamp_ns.unwrap();
        assert!(previous.is_none_or(|old| timestamp > old));
        previous = Some(timestamp);
        frames += 1;
    }
    assert_eq!(frames, 30);
    let mut audio = AudioReader::open(&path).unwrap();
    let mut samples = Vec::new();
    while let Some(block) = audio.next_samples().unwrap() {
        samples.extend(block);
    }
    assert!(
        (samples.len() as i64 / 2 - 48048).abs() <= 1,
        "{}",
        samples.len()
    );
    assert!(samples.iter().all(|s| s.is_finite()));
    assert!(samples.iter().any(|s| s.abs() > 0.01));
    assert!(
        samples
            .as_chunks::<2>()
            .0
            .iter()
            .all(|p| (p[0] - p[1]).abs() < 0.00001)
    );
    assert!(audio.next_samples().unwrap().is_none());
}

#[test]
fn invalid_local_media_and_profiles_fail_explicitly() {
    let directory = tempdir().unwrap();
    let bad = directory.path().join("bad.mov");
    std::fs::write(&bad, b"invalid codec bytes").unwrap();
    assert!(VideoReader::open(&bad).is_err());
    assert!(AudioReader::open(&bad).is_err());
    assert!(VideoReader::open(directory.path()).is_err());
    assert!(
        VideoWriter::open_temporary(&directory.path().join("bad.mkv"), MediaInfo::default())
            .is_err()
    );
}
