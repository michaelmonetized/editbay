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
fn source_inventory_checks_real_pictures_and_reports_failures_without_following_symlinks_or_replacing_files()
 {
    let directory = tempdir().unwrap();
    let sources = directory.path().join("client sources");
    fs::create_dir(&sources).unwrap();
    let video = source(&sources);
    let original = fs::read(&video).unwrap();
    let hidden = sources.join(".hidden");
    fs::create_dir(&hidden).unwrap();
    fs::write(hidden.join("private.mp4"), b"not media").unwrap();
    std::os::unix::fs::symlink(&video, sources.join("linked.mp4")).unwrap();
    fs::write(sources.join("bad.mp4"), b"damaged media").unwrap();
    let report = directory.path().join("new inventory.json");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_editbay-lab"))
            .arg("inventory")
            .arg(&sources)
            .arg(&report)
            .output()
            .unwrap()
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(manifest["coverage_complete"], false);
    assert_eq!(manifest["sources"].as_array().unwrap().len(), 1);
    assert_eq!(
        manifest["sources"][0]["first_picture"]["rgba_bytes"],
        257 * 17 * 4
    );
    assert_eq!(manifest["failures"].as_array().unwrap().len(), 1);
    assert_eq!(manifest["excluded_entries"], 2);
    let saved = fs::read(&report).unwrap();
    assert!(!run().status.success());
    assert_eq!(fs::read(&report).unwrap(), saved);
    assert_eq!(fs::read(&video).unwrap(), original);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
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
