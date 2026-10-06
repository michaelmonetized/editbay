use crate::{Result, memory};
use editbay_audio::{MonitorRoute, PlaybackPhase, PlaybackStart, StreamingPlayback};
use editbay_core::*;
use editbay_media::{Cancellation, SourceFile, StreamType};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

/// Exercise actual device callbacks through the production streaming worker.
/// `path` selects read-only media; `mode` is full, cancel, kill or underrun.
/// Returns bounded-buffer, clock and retirement evidence, never physical audibility.
pub fn run(path: &Path, mode: &str) -> Result<Value> {
    if ![
        "full",
        "cancel",
        "kill",
        "underrun",
        "kill-device",
        "stall-device",
        "cancel-stalled-device",
        "prepare-cancel",
    ]
    .contains(&mode)
    {
        return Err("Choose full, cancel, kill, underrun, kill-device, stall-device, cancel-stalled-device or prepare-cancel".into());
    }
    rustix::process::set_child_subreaper(Some(rustix::process::Pid::INIT))?;
    let cancel = Cancellation::new()?;
    let owned = SourceFile::open(path, &cancel)?;
    let probe = owned.probe(cancel.clone())?;
    let video = probe
        .streams
        .iter()
        .find(|s| s.kind == StreamType::Video && s.decoder_available)
        .ok_or("Streaming qualification needs video")?
        .index;
    let audio = probe
        .streams
        .iter()
        .find(|s| s.kind == StreamType::Audio && s.decoder_available)
        .map(|s| s.index);
    let streams: Vec<_> = std::iter::once(video).chain(audio).collect();
    let imported = owned.ingest(
        "Native streaming source".into(),
        &streams,
        cancel.clone(),
        |_, _| {},
    )?;
    let mut project = Project::new("Native streaming qualification")?;
    project.assets.push(imported.asset);
    project.sources.push(imported.source);
    let commands = match audio {
        Some(audio) => sequence_from_video_with_audio(
            &project,
            project.sources[0].id,
            video,
            audio,
            FrameRate::new(30, 1)?,
        )?,
        None => sequence_from_video(
            &project,
            project.sources[0].id,
            video,
            FrameRate::new(30, 1)?,
        )?,
    };
    let mut editor = DocumentEditor::new(project)?;
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Create streaming sequence".into(),
        &commands,
    )?;
    let composition = editor.project().sequences[0]
        .composition
        .ok_or("Missing authored sequence")?;
    let mut playback = StreamingPlayback::start(
        editor.snapshot(),
        composition,
        PlaybackStart::Frame(0),
        MonitorRoute::Stereo,
    )?;
    let began = Instant::now();
    let mut requested = None;
    let mut pid = None;
    let mut device_pid = None;
    let mut samples = Vec::new();
    let mut last_position = 0;
    let final_status = loop {
        let status = playback.status();
        if status.prepared_frames > u64::from(status.prepared_capacity_frames) {
            return Err("Prepared sound exceeded capacity".into());
        }
        if let Some(position) = status.position_samples {
            if position < last_position {
                return Err("Streaming clock moved backward".into());
            }
            last_position = position;
        }
        if status.device_worker_pid.is_some() {
            device_pid = status.device_worker_pid;
        }
        if status.worker_pid.is_some() {
            pid = status.worker_pid;
        }
        samples.push(json!({"elapsed_ms":began.elapsed().as_secs_f64()*1000., "phase":status.phase,"position":status.position_samples,
            "callbacks":status.callbacks,"prepared_frames":status.prepared_frames,"latency_ns":status.reported_latency_ns}));
        if mode != "full"
            && requested.is_none()
            && (status.callbacks >= 10 || (mode == "prepare-cancel" && device_pid.is_some()))
        {
            requested = Some(Instant::now());
            if matches!(mode, "cancel" | "prepare-cancel") {
                playback.stop();
            } else {
                let pid = if mode.contains("device") {
                    device_pid
                } else {
                    pid
                }
                .ok_or("Missing supervised worker")?;
                let signal = if matches!(mode, "kill" | "kill-device") {
                    "-KILL"
                } else {
                    "-STOP"
                };
                if !Command::new("kill")
                    .args([signal, &pid.to_string()])
                    .status()?
                    .success()
                {
                    return Err("Could not signal owned sound worker".into());
                }
                if mode == "cancel-stalled-device" {
                    playback.stop();
                }
            }
        }
        if playback.is_finished() {
            playback.reap();
            break playback.status();
        }
        if began.elapsed() > Duration::from_secs(120)
            || requested.is_some_and(|t| t.elapsed() > Duration::from_secs(2))
        {
            playback.stop();
            return Err("Streaming exceeded its qualification/retirement budget".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let deadline = requested.unwrap_or_else(Instant::now) + Duration::from_secs(2);
    let adopted: Vec<u32> =
        std::fs::read_to_string(format!("/proc/self/task/{}/children", std::process::id()))?
            .split_whitespace()
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()?;
    if adopted.len() > 1
        || adopted
            .first()
            .is_some_and(|child| pid.is_some_and(|pid| pid != *child))
    {
        return Err("Playback retained an unexpected descendant".into());
    }
    for child in &adopted {
        crate::device_protocol::reap_orphan(*child, deadline)?;
    }
    if let Some(pid) = pid {
        crate::device_protocol::reap_orphan(pid, deadline)?;
    }
    let retirement_ms = requested.map(|t| t.elapsed().as_secs_f64() * 1000.);
    let reaped = pid.is_none_or(|pid| !Path::new(&format!("/proc/{pid}")).exists());
    let device_reaped = device_pid.is_none_or(|pid| !Path::new(&format!("/proc/{pid}")).exists());
    owned.verify(&cancel)?;
    let expected = match mode {
        "full" => PlaybackPhase::Finished,
        "cancel" | "cancel-stalled-device" | "prepare-cancel" => PlaybackPhase::Stopped,
        _ => PlaybackPhase::Failed,
    };
    let qualified = final_status.phase == expected
        && (final_status.callbacks > 0 || mode == "prepare-cancel")
        && reaped
        && device_reaped
        && (mode != "full" || final_status.position_samples == final_status.end_sample)
        && (mode != "underrun"
            || final_status
                .error
                .as_ref()
                .is_some_and(|e| e.contains("fell behind")));
    let receipt = json!({"kind":"native_streaming_sound","source":path.canonicalize()?,"source_fingerprint":owned.fingerprint(),"mode":mode,
        "application_sha256":crate::hash(&std::env::current_exe()?)?,"elapsed_seconds":began.elapsed().as_secs_f64(),"retirement_ms":retirement_ms,
        "final":final_status,"samples":samples,"worker_reaped":reaped,"device_worker_reaped":device_reaped,"adopted_pcm_reaped":adopted,"source_unchanged":true,"memory":memory()?,"qualified":qualified,
        "limits":["Actual default output device callbacks and backend latency estimate; no physical audibility or speaker timing claim",
            "Lab-authored graph; native window controls and long hardware drift require separate evidence"]});
    if !qualified {
        return Err(format!("Streaming qualification failed: {receipt}").into());
    }
    Ok(receipt)
}
