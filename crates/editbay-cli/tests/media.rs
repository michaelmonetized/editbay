use editbay_core::{Project, StreamFormat, load, save_new};
use editbay_media::worker::{self, Request, Response};
use serde_json::Value;
use std::{
    io::BufReader,
    path::Path,
    process::{Child, ChildStdin, Command, Output, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use tempfile::tempdir;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_editbay"))
        .args(args)
        .output()
        .unwrap()
}

fn successful(args: &[&str]) -> Value {
    let output = run(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn fixture(path: &Path, duration: &str) {
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=60",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100",
            "-t",
            duration,
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "pcm_s16le",
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

struct Worker {
    child: Child,
    input: ChildStdin,
    events: Receiver<Response>,
}

impl Worker {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_editbay"))
            .arg("--media-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let (sender, events) = mpsc::channel();
        std::thread::spawn(move || {
            while let Some(bytes) = worker::read_message(&mut output).unwrap() {
                if sender
                    .send(serde_json::from_slice(&bytes).unwrap())
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            events,
        }
    }

    fn send(&mut self, request: Request) {
        worker::write_message(&mut self.input, &request).unwrap();
    }
    fn next(&self) -> Response {
        self.events.recv_timeout(Duration::from_secs(10)).unwrap()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn cli_ingest_is_durable_and_recovery_preserves_real_stream_data() {
    let directory = tempdir().unwrap();
    let media = directory.path().join("camera source.mkv");
    fixture(&media, "0.2");
    let before = std::fs::read(&media).unwrap();
    let project = directory.path().join("paying edit.editbay");
    save_new(&Project::new("Client work").unwrap(), &project).unwrap();
    let probe = successful(&["probe-media", media.to_str().unwrap()]);
    assert_eq!(probe["streams"].as_array().unwrap().len(), 2);
    let imported = successful(&[
        "ingest",
        project.to_str().unwrap(),
        media.to_str().unwrap(),
        "0,1",
    ]);
    assert_eq!(imported["saved"], true);
    let saved = load(&project).unwrap();
    assert_eq!(
        (saved.revision, saved.assets.len(), saved.sources.len()),
        (1, 1, 1)
    );
    assert_eq!(saved.sources[0].streams.len(), 2);
    assert!(matches!(
        saved.sources[0].streams[1].format,
        StreamFormat::Audio {
            sample_rate: 44100,
            ..
        }
    ));
    let decoded = successful(&["decode-frame", media.to_str().unwrap(), "0", "0"]);
    assert_eq!(decoded["source_tick"], 0);
    assert_eq!(decoded["rgba_bytes"], 320 * 180 * 4);
    let recovery = directory.path().join("recovery");
    let checkpoint = successful(&[
        "checkpoint",
        project.to_str().unwrap(),
        recovery.to_str().unwrap(),
    ]);
    let recovered = directory.path().join("recovered.editbay");
    successful(&[
        "recover",
        checkpoint["checkpoint"].as_str().unwrap(),
        recovered.to_str().unwrap(),
    ]);
    let copy = load(&recovered).unwrap();
    assert_ne!(copy.id, saved.id);
    assert_eq!(copy.sources, saved.sources);
    assert_eq!(copy.assets, saved.assets);
    assert_eq!(std::fs::read(&media).unwrap(), before);
    let saved_bytes = std::fs::read(&project).unwrap();
    assert!(
        !run(&[
            "ingest",
            project.to_str().unwrap(),
            media.to_str().unwrap(),
            "99"
        ])
        .status
        .success()
    );
    assert_eq!(std::fs::read(&project).unwrap(), saved_bytes);
}

#[test]
fn isolated_worker_retains_source_ownership_and_cancels_active_native_decode() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("longer.mkv");
    fixture(&path, "10");
    let mut process = Worker::new();
    process.send(Request::Probe { path: path.clone() });
    assert!(matches!(process.next(), Response::Probed { .. }));
    let limits = std::fs::read_to_string(format!("/proc/{}/limits", process.child.id())).unwrap();
    for (name, expected) in [
        ("Max address space", worker::VIRTUAL_MEMORY_BYTES),
        ("Max cpu time", worker::CPU_SECONDS),
        ("Max open files", 512),
    ] {
        let actual = limits
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(actual <= expected);
    }
    process.send(Request::Ingest {
        name: "Cancellable source".into(),
        streams: vec![0],
    });
    assert!(matches!(process.next(), Response::Progress { completed, .. } if completed >= 32));
    let started = Instant::now();
    process.send(Request::Cancel);
    loop {
        match process.next() {
            Response::Progress { .. } => {}
            Response::Failed { cancelled, .. } => {
                assert!(cancelled);
                break;
            }
            other => panic!("cancelled native work returned {other:?}"),
        }
    }
    assert!(started.elapsed() < Duration::from_secs(2));
    let mut process = Worker::new();
    process.send(Request::Probe { path: path.clone() });
    assert!(matches!(process.next(), Response::Probed { .. }));
    std::fs::rename(&path, directory.path().join("owned.mkv")).unwrap();
    std::fs::write(&path, b"replacement source").unwrap();
    process.send(Request::Ingest {
        name: "Stale source".into(),
        streams: vec![0],
    });
    loop {
        match process.next() {
            Response::Progress { .. } => {}
            Response::Failed { error, .. } => {
                assert!(error.contains("source changed"));
                break;
            }
            other => panic!("stale source work returned {other:?}"),
        }
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement source");
}

#[test]
fn format_copy_migration_enforces_the_actual_document_read_budget() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("oversized.editbay");
    std::fs::File::create(&source)
        .unwrap()
        .set_len(editbay_core::MAX_DOCUMENT_BYTES + 1)
        .unwrap();
    let destination = directory.path().join("unpublished.editbay");
    let output = run(&[
        "migrate",
        source.to_str().unwrap(),
        destination.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("read budget"));
    assert!(!destination.exists());
}
