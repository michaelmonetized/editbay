use editbay_media::{MediaInfo, VideoReader, VideoWriter};
use serde_json::Value;
use std::{fs, process::Command};
use tempfile::tempdir;

fn source(directory: &std::path::Path) -> std::path::PathBuf {
    let path = directory.join("source with spaces.mkv");
    let profile = MediaInfo {
        width: 257,
        height: 17,
        rate_num: 24,
        rate_den: 1,
        ..Default::default()
    };
    let mut writer = VideoWriter::open_temporary(&path, profile).unwrap();
    for frame in 0..12 {
        let rgba: Vec<u8> = (0..257 * 17 * 4)
            .map(|i| {
                if i % 4 == 3 {
                    255
                } else {
                    ((i * 7 + frame * 13) % 256) as u8
                }
            })
            .collect();
        writer.write(&rgba).unwrap();
    }
    writer.finish().unwrap();
    path
}

#[test]
fn actual_background_export_verifies_pixels_and_preserves_existing_work() {
    let directory = tempdir().unwrap();
    let source = source(directory.path());
    let original = fs::read(&source).unwrap();
    let destination = directory.path().join("new delivery.mkv");
    let output = Command::new(env!("CARGO_BIN_EXE_editbay-lab"))
        .arg("export")
        .arg(&source)
        .arg(&destination)
        .arg("12")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["frames"], 12);
    assert_eq!(receipt["decoded_output_frames"], 12);
    assert_eq!(receipt["decoded_pixels_match_composition"], true);
    assert_eq!(receipt["background_process"], true);
    let saved = fs::read(&destination).unwrap();
    let mut decoder = VideoReader::open(&destination).unwrap();
    assert!(decoder.next_frame().unwrap().is_some());
    let repeat = Command::new(env!("CARGO_BIN_EXE_editbay-lab"))
        .arg("export")
        .arg(&source)
        .arg(&destination)
        .arg("12")
        .output()
        .unwrap();
    assert!(!repeat.status.success());
    assert_eq!(fs::read(&destination).unwrap(), saved);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn cancellation_and_codec_failure_leave_no_partial_destination_or_worker_files() {
    let directory = tempdir().unwrap();
    let source = source(directory.path());
    let destination = directory.path().join("cancelled.mkv");
    let cancelled = Command::new(env!("CARGO_BIN_EXE_editbay-lab"))
        .arg("cancel-export")
        .arg(&source)
        .arg(&destination)
        .args(["3600", "0"])
        .output()
        .unwrap();
    assert!(
        cancelled.status.success(),
        "{}",
        String::from_utf8_lossy(&cancelled.stderr)
    );
    let receipt: Value = serde_json::from_slice(&cancelled.stdout).unwrap();
    assert_eq!(receipt["underlying_process_stopped"], true);
    assert_eq!(receipt["destination_published"], false);
    assert!(!destination.exists());
    let bad = directory.path().join("bad.mov");
    fs::write(&bad, b"bad media").unwrap();
    let failed = Command::new(env!("CARGO_BIN_EXE_editbay-lab"))
        .arg("export")
        .arg(&bad)
        .arg(&destination)
        .arg("12")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}
