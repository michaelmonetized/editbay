use editbay_media::{Cancellation, NativeAudioReader, SourceFile};
use std::process::Command;
use tempfile::tempdir;

#[test]
fn aac_priming_and_packet_timestamps_do_not_introduce_a_false_sound_gap() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("primed sound.m4a");
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100:duration=0.23",
            "-c:a",
            "aac",
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
    let mut reader = NativeAudioReader::open_stream(&source, 0, cancel.clone()).unwrap();
    let mut end = 0;
    while let Some(block) = reader.next_block().unwrap() {
        assert_eq!(block.first_sample, Some(end));
        end += block.samples.len() as i64;
    }
    let imported = source
        .ingest("Primed sound".into(), &[0], cancel, |_, _| {})
        .unwrap();
    assert_eq!(imported.source.streams[0].start_tick, 0);
    assert_eq!(imported.source.streams[0].duration_ticks, Some(end as u64));
    assert_eq!(end, 10143);
}
