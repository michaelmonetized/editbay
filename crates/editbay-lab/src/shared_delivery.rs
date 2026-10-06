use crate::{Result, hash, memory};
use editbay_delivery::{DeliveryControl, DeliveryRequest, Phase, deliver};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command, sync::Arc, time::Instant};

/// Exercise production delivery and process retirement on a saved document.
/// `project`, `composition` and `destination` select real content and a new file;
/// `mode` selects full, cancel, kill, stall, collision or cancel-verify evidence.
/// Returns measured receipts and fails if any declared acceptance gate fails.
pub fn run(project: &Path, composition: &str, destination: &Path, mode: &str) -> Result<Value> {
    run_range(project, composition, destination, mode, None)
}

/// Exercise the same delivery faults with an explicit original-grid selection.
/// `range` is a half-open composition interval; the other arguments identify
/// the saved inputs, new output and trial. Returns actual cleanup/file evidence.
pub fn run_range(
    project: &Path,
    composition: &str,
    destination: &Path,
    mode: &str,
    range: Option<editbay_core::FrameRange>,
) -> Result<Value> {
    if ![
        "full",
        "cancel",
        "kill",
        "stall",
        "collision",
        "cancel-verify",
        "cancel-preparation",
        "source-change",
    ]
    .contains(&mode)
    {
        return Err(
            "Choose full, cancel, kill, stall, collision, cancel-verify or source-change".into(),
        );
    }
    if destination.exists() {
        return Err("Qualification destination must be new".into());
    }
    let mut project = editbay_core::load(project)?;
    let private_sources = (mode == "source-change")
        .then(tempfile::tempdir)
        .transpose()?;
    let changed_source = if let Some(directory) = private_sources.as_ref() {
        let asset = project
            .assets
            .first_mut()
            .ok_or("Qualification needs a source asset")?;
        let path = directory.path().join("Private source.mov");
        fs::copy(&asset.path, &path)?;
        asset.path = path.clone();
        Some(path)
    } else {
        None
    };
    let project = Arc::new(project);
    let control = DeliveryControl::new()?;
    let request = DeliveryRequest {
        composition: composition.parse()?,
        sample_rate: 48000,
        range,
    };
    let began = Instant::now();
    let mut action = None;
    let mut action_error = None;
    let mut pid = None;
    let mut high_water_kib = 0;
    let mut observations = Vec::new();
    let mut delayed = None;
    let outcome = deliver(
        &std::env::current_exe()?,
        project,
        request,
        destination,
        &control,
        |status, child| {
            pid = Some(child);
            if let Ok(memory) = fs::read_to_string(format!("/proc/{child}/status")) {
                high_water_kib = high_water_kib.max(
                    memory
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("VmHWM:")?
                                .split_whitespace()
                                .next()?
                                .parse::<u64>()
                                .ok()
                        })
                        .unwrap_or(0),
                );
            }
            if observations.len() < 4096 {
                observations.push(
                    json!({"elapsed_ms":began.elapsed().as_secs_f64()*1000., "progress":status}),
                );
            }
            let trigger = if mode == "cancel-preparation" {
                status.phase == Phase::Preparing
                    && status.total_preparation_samples > 0
                    && status.total_pictures > 0
            } else if ["collision", "cancel-verify"].contains(&mode) {
                status.phase == Phase::VerifyingFile
            } else if mode == "source-change" {
                status.phase == Phase::VerifyingSound
            } else {
                status.phase == Phase::Rendering && status.pictures >= 2
            };
            if mode != "full" && action.is_none() && trigger {
                action = Some(Instant::now());
                let mut run = || -> Result<()> {
                    match mode {
                        "source-change" => {
                            use std::io::Write;
                            fs::OpenOptions::new()
                                .append(true)
                                .open(changed_source.as_ref().ok_or("Private source missing")?)?
                                .write_all(b"changed during verification")?;
                        }
                        "cancel" | "cancel-verify" | "cancel-preparation" => {
                            if !control.cancel() {
                                return Err("Cancellation was not accepted".into());
                            }
                        }
                        "collision" => {
                            use std::io::Write;
                            fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(destination)?
                                .write_all(b"Other user's existing master")?;
                        }
                        "kill" | "stall" => {
                            if !Command::new("kill")
                                .args([
                                    if mode == "kill" { "-KILL" } else { "-STOP" },
                                    &child.to_string(),
                                ])
                                .status()?
                                .success()
                            {
                                return Err("Could not signal export worker".into());
                            }
                            if mode == "stall" {
                                let control = control.clone();
                                delayed = Some(std::thread::spawn(move || {
                                    std::thread::sleep(std::time::Duration::from_millis(50));
                                    control.cancel()
                                }));
                            }
                        }
                        _ => {}
                    }
                    Ok(())
                };
                if let Err(error) = run() {
                    action_error = Some(error.to_string());
                    control.cancel();
                }
            }
        },
    );
    let retirement_ms = action.map(|time| time.elapsed().as_secs_f64() * 1000.);
    if let Some(thread) = delayed
        && !thread.join().map_err(|_| "Delayed cancellation panicked")?
    {
        return Err("Stalled cancellation was not accepted".into());
    }
    if let Some(error) = action_error {
        return Err(error.into());
    }
    let reaped = pid.is_some_and(|pid| !Path::new(&format!("/proc/{pid}")).exists());
    let published = destination.exists();
    let collision_preserved =
        mode != "collision" || fs::read(destination)? == b"Other user's existing master";
    let qualified = reaped
        && collision_preserved
        && match mode {
            "full" => outcome.is_ok() && published,
            "collision" => action.is_some() && outcome.is_err() && published,
            "source-change" => action.is_some() && outcome.is_err() && !published,
            _ => {
                action.is_some()
                    && outcome.is_err()
                    && !published
                    && retirement_ms.is_some_and(|ms| ms <= 2000.)
            }
        };
    let (receipt, error) = match outcome {
        Ok(receipt) => (Some(receipt), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let independent = receipt
        .as_ref()
        .map(|receipt| inspect(destination, receipt))
        .transpose()?;
    let report = json!({"schema":1,"kind":"shared_delivery_qualification","mode":mode,"range":range,"qualified":qualified,
        "binary_sha256":hash(&std::env::current_exe()?)?,"receipt":receipt,"independent":independent,"error":error,"elapsed_ms":began.elapsed().as_secs_f64()*1000.,
        "retirement_ms":retirement_ms,"worker_reaped":reaped,"worker_high_water_kib":high_water_kib,"memory":memory()?,
        "destination":destination,"published":published,"collision_preserved":collision_preserved,"observations":observations});
    if !qualified {
        return Err(format!("Shared delivery failed its gate: {report}").into());
    }
    Ok(report)
}

/// Independently decode a delivered master through the installed FFmpeg executable.
/// `path` names the completed output and `receipt` declares its expected inputs.
/// Returns bounded streaming hash/count/tail and container inspection evidence.
pub fn inspect(path: &Path, receipt: &editbay_delivery::Receipt) -> Result<Value> {
    use sha2::{Digest, Sha256};
    use std::{io::Read, process::Stdio};
    let mut streams = Vec::new();
    let picture_bytes = u64::from(receipt.profile.width) * u64::from(receipt.profile.height) * 4;
    for picture in [true, false] {
        let mut command = Command::new("ffmpeg");
        command.args(["-v", "error", "-nostdin", "-i"]).arg(path);
        if picture {
            command.args([
                "-map",
                "0:v:0",
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "pipe:1",
            ]);
        } else {
            command.args([
                "-map",
                "0:a:0",
                "-c:a",
                "pcm_f32le",
                "-f",
                "f32le",
                "pipe:1",
            ]);
        }
        let mut child = crate::Worker(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?,
        );
        let mut output = child
            .0
            .stdout
            .take()
            .ok_or("Independent decoder has no output")?;
        let mut buffer = [0u8; 65536];
        let mut bytes = 0;
        let mut digest = Sha256::new();
        let mut transparent_tail_pixels = 0u64;
        let tail = (receipt.profile.frames - 1) * picture_bytes;
        loop {
            let length = output.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            digest.update(&buffer[..length]);
            if picture && bytes + length as u64 > tail {
                for (index, byte) in buffer[..length].iter().enumerate() {
                    let position = bytes + index as u64;
                    if position >= tail && position % 4 == 3 && *byte == 0 {
                        transparent_tail_pixels += 1;
                    }
                }
            }
            bytes += length as u64;
        }
        if !child.0.wait()?.success() {
            return Err("Independent master decoder failed".into());
        }
        let actual = format!("{:x}", digest.finalize());
        let (expected_hash, expected_bytes) = if picture {
            (
                &receipt.pixel_sha256,
                picture_bytes * receipt.profile.frames,
            )
        } else {
            (
                &receipt.pcm_sha256,
                receipt.profile.samples_through(receipt.profile.frames)?
                    * receipt.profile.channels.len() as u64
                    * 4,
            )
        };
        if actual != *expected_hash || bytes != expected_bytes {
            return Err("Independent master content differs from graph receipt".into());
        }
        streams.push(json!({"kind":if picture {"picture"} else {"sound"},"sha256":actual,"bytes":bytes,"transparent_last_picture_pixels":picture.then_some(transparent_tail_pixels),"exact":true}));
    }
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()?;
    if !probe.status.success() {
        return Err("Independent output metadata probe failed".into());
    }
    Ok(json!({"streams":streams,"metadata":serde_json::from_slice::<Value>(&probe.stdout)?}))
}
