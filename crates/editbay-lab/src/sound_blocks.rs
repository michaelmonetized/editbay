use crate::{Result, memory, metrics};
use editbay_audio::{SoundRenderBudget, SoundRenderer};
use editbay_core::*;
use editbay_media::{Cancellation, PcmBudget, SourceFile, StreamType};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    time::Instant,
};
use uuid::Uuid;
fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn time_map(frames: u64, start: i64, end: i64) -> TimeMap {
    TimeMap {
        points: vec![
            TimePoint {
                frame: 0,
                source_tick: start,
            },
            TimePoint {
                frame: frames,
                source_tick: end,
            },
        ],
    }
}
fn node(n: u128, frames: u64, operation: NodeOperation) -> TimedNode {
    TimedNode {
        id: id(n),
        range: FrameRange {
            start: 0,
            end: frames,
        },
        operation,
        animation: vec![],
    }
}
fn compile(p: Project, rate: u32, channels: &[String]) -> Result<Arc<SoundSnapshot>> {
    Ok(Arc::new(SoundSnapshot::new(
        Arc::new(EvaluationSnapshot::new(Arc::new(p))?),
        id(30),
        SoundProfile {
            sample_rate: rate,
            channels: channels.to_vec(),
        },
        SoundBudget::default(),
    )?))
}
fn renderer(s: Arc<SoundSnapshot>, cancel: Cancellation) -> Result<SoundRenderer> {
    Ok(SoundRenderer::new(
        s,
        PcmBudget::default(),
        SoundRenderBudget::default(),
        cancel,
    )?)
}
fn independent_pcm(path: &Path, stream: u32) -> Result<(Vec<f32>, String)> {
    let mut process = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-map",
            &format!("0:{stream}"),
            "-f",
            "f32le",
            "-c:a",
            "pcm_f32le",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut bytes = vec![];
    let read = process
        .stdout
        .take()
        .ok_or("reference pipe absent")?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > 64 * 1024 * 1024 {
        let _ = process.kill();
        let _ = process.wait();
        return Err("independent sound reference exceeds its 64 MiB qualification bound".into());
    }
    if !process.wait()?.success() || !bytes.len().is_multiple_of(4) {
        return Err("independent native PCM decode failed".into());
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    Ok((
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect(),
        digest,
    ))
}

/// Qualify actual sound, independent PCM, private owners and declared worker budgets.
/// `path` is read-only media. Returns measurements and gate results; no device or
/// UI playback, drift, callback scheduling or codec-process isolation is inferred.
pub fn run(path: &Path) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let source = SourceFile::open(path, &cancel)?;
    let before = source.fingerprint().clone();
    let probe = source.probe(cancel.clone())?;
    let selected = probe
        .streams
        .iter()
        .find(|s| s.decoder_available && s.kind == StreamType::Audio)
        .ok_or("source has no decodable sound stream")?
        .index;
    let imported = source.ingest(
        "Actual sound qualification".into(),
        &[selected],
        cancel.clone(),
        |_, _| {},
    )?;
    let source_id = imported.source.id;
    let stream = &imported.source.streams[0];
    let StreamFormat::Audio {
        sample_rate,
        channels,
    } = &stream.format
    else {
        return Err("ingested stream is not sound".into());
    };
    let rate = *sample_rate;
    let channels = channels.clone();
    let samples = stream.duration_ticks.ok_or("sound duration absent")?;
    let seconds = (samples / u64::from(rate)).min(12);
    if seconds < 3 || rate < 44100 {
        return Err("qualification requires at least three seconds at 44.1 kHz or higher".into());
    }
    let start = stream.start_tick;
    let end = i64::try_from(i128::from(start) + i128::from(seconds * u64::from(rate)))?;
    let root_frames = seconds * 24;
    let child_frames = seconds * 60;
    let mut p = Project::new("Actual nested sound graph")?;
    p.assets.push(imported.asset);
    p.sources.push(imported.source);
    let clip = Clip {
        id: id(41),
        name: "Native original".into(),
        range: FrameRange {
            start: 0,
            end: root_frames,
        },
        source: ClipSource::Media {
            source: source_id,
            stream: selected,
        },
        time_map: time_map(root_frames, start, end),
        linked: None,
    };
    let track = Track {
        id: id(40),
        name: "Original channels".into(),
        kind: TrackKind::Audio,
        enabled: true,
        clips: vec![clip],
    };
    let mut child_track = track.clone();
    child_track.id = id(42);
    child_track.clips[0].id = id(43);
    child_track.clips[0].range.end = child_frames;
    child_track.clips[0].time_map = time_map(child_frames, start, end);
    let child = Composition {
        id: id(31),
        name: "60 fps sound nest".into(),
        width: 16,
        height: 16,
        frame_rate: FrameRate::new(60, 1)?,
        duration: child_frames,
        tracks: vec![child_track],
        nodes: vec![
            node(53, child_frames, NodeOperation::Source { clip: id(43) }),
            node(
                54,
                child_frames,
                NodeOperation::Gain {
                    audio: id(53),
                    gain: 0.75,
                },
            ),
        ],
        picture: None,
        audio: Some(id(54)),
    };
    let nested = Track {
        id: id(44),
        name: "Nested contribution".into(),
        kind: TrackKind::Audio,
        enabled: true,
        clips: vec![Clip {
            id: id(45),
            name: "Natural nested time".into(),
            range: FrameRange {
                start: 0,
                end: root_frames,
            },
            source: ClipSource::Composition {
                composition: id(31),
            },
            time_map: time_map(root_frames, 0, child_frames as i64),
            linked: None,
        }],
    };
    let root = Composition {
        id: id(30),
        name: "Two-contribution sound mix".into(),
        width: 16,
        height: 16,
        frame_rate: FrameRate::new(24, 1)?,
        duration: root_frames,
        tracks: vec![track, nested],
        nodes: vec![
            node(50, root_frames, NodeOperation::Source { clip: id(41) }),
            node(
                51,
                root_frames,
                NodeOperation::Gain {
                    audio: id(50),
                    gain: 0.25,
                },
            ),
            node(55, root_frames, NodeOperation::Source { clip: id(45) }),
            node(
                52,
                root_frames,
                NodeOperation::Mix {
                    inputs: vec![id(51), id(55)],
                },
            ),
        ],
        picture: None,
        audio: Some(id(52)),
    };
    p.compositions.extend([root, child]);
    let compiled = compile(p.clone(), rate, &channels)?;
    let (reference, reference_hash) = independent_pcm(path, selected)?;
    let mut render = renderer(compiled.clone(), Cancellation::new()?)?;
    let frames = 4096;
    let count = (compiled.duration_samples() / u64::from(frames)).min(64) as usize;
    let mut preparation = vec![];
    let mut cold = vec![];
    let mut warm = vec![];
    let mut maximum_error = 0f32;
    let mut output_digest = Sha256::new();
    for n in 0..count {
        let first = n as u64 * u64::from(frames);
        let began = Instant::now();
        let plan = compiled.prepare(first, frames)?;
        preparation.push(began.elapsed().as_secs_f64() * 1000.);
        let began = Instant::now();
        let result = render.render(&plan)?;
        cold.push(began.elapsed().as_secs_f64() * 1000.);
        for (sample, original) in result.sound().samples().iter().zip(
            &reference[first as usize * channels.len()
                ..(first as usize + frames as usize) * channels.len()],
        ) {
            maximum_error = maximum_error.max((sample - original).abs());
            output_digest.update(sample.to_le_bytes());
        }
        render.validate_result(&result)?;
    }
    for n in 0..100 {
        let plan = compiled.prepare((n % count) as u64 * u64::from(frames), frames)?;
        let began = Instant::now();
        let result = render.render(&plan)?;
        warm.push(began.elapsed().as_secs_f64() * 1000.);
        render.validate_result(&result)?;
    }
    let first_native_ms = cold[0];
    let stats = render.pcm_stats();
    render.verify_sources()?;
    let held = render.render(&compiled.prepare(0, frames)?)?;
    let held_bytes = render.live_bytes();
    let mut foreign = renderer(compile(p.clone(), rate, &channels)?, Cancellation::new()?)?;
    let foreign_plan_rejected = foreign.render(&compiled.prepare(0, frames)?).is_err();
    let foreign_result_rejected = foreign.validate_result(&held).is_err();
    render.clear();
    let stale_result_rejected = render.validate_result(&held).is_err();
    let pinned_after_clear = render.live_bytes() == held_bytes && held_bytes > 0;
    drop(held);
    let cleanup = render.live_bytes() == 0
        && render.pcm_stats().live_bytes == 0
        && render.pcm_stats().sources == 0
        && render.pcm_stats().decoders == 0;
    let canceled = Cancellation::new()?;
    let mut canceled_renderer = renderer(compiled.clone(), canceled.clone())?;
    let plan = compiled.prepare(0, frames)?;
    let began = Instant::now();
    canceled.cancel();
    let cancelled_result_rejected = matches!(
        canceled_renderer.render(&plan),
        Err(editbay_media::Error::Cancelled)
    );
    let cancellation_ms = began.elapsed().as_secs_f64() * 1000.;
    let output_rate = 44100;
    let resampled = compile(p, output_rate, &channels)?;
    let mut converter = renderer(resampled.clone(), Cancellation::new()?)?;
    let mut sinc = vec![];
    for n in 0..30 {
        let plan = resampled.prepare(4096 + (n % 5) * 4096, frames)?;
        let began = Instant::now();
        let result = converter.render(&plan)?;
        sinc.push(began.elapsed().as_secs_f64() * 1000.);
        converter.validate_result(&result)?;
    }
    source.verify(&cancel)?;
    let source_unchanged = source.fingerprint() == &before;
    let preparation = metrics(&mut preparation);
    let cold = metrics(&mut cold);
    let warm = metrics(&mut warm);
    let sinc = metrics(&mut sinc);
    let active_token = Cancellation::new()?;
    let worker_token = active_token.clone();
    let (ready, began) = std::sync::mpsc::channel();
    let active_snapshot = resampled.clone();
    let thread = std::thread::spawn(move || -> std::result::Result<(bool, bool), String> {
        let mut engine =
            renderer(active_snapshot.clone(), worker_token).map_err(|e| e.to_string())?;
        let plan = active_snapshot
            .prepare(4096, 4096)
            .map_err(|e| e.to_string())?;
        drop(engine.render(&plan).map_err(|e| e.to_string())?);
        ready.send(()).map_err(|e| e.to_string())?;
        let rejected = matches!(engine.render(&plan), Err(editbay_media::Error::Cancelled));
        engine.clear();
        let clean = engine.live_bytes() == 0
            && engine.pcm_stats().live_bytes == 0
            && engine.pcm_stats().decoders == 0
            && engine.pcm_stats().sources == 0;
        Ok((rejected, clean))
    });
    began.recv_timeout(std::time::Duration::from_secs(10))?;
    std::thread::sleep(std::time::Duration::from_millis(1));
    let active_began = Instant::now();
    active_token.cancel();
    let (active_rejected, active_cleanup) = thread
        .join()
        .map_err(|_| "active sound worker panicked")?
        .map_err(|e| format!("active sound worker: {e}"))?;
    let active_cancellation_ms = active_began.elapsed().as_secs_f64() * 1000.;
    let memory = memory()?;
    let high_water_kib = std::fs::read_to_string("/proc/self/status")?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse::<u64>().ok())
        })
        .ok_or("process high-water memory absent")?;
    let gates = json!({"preparation_p95_5ms":preparation["p95_ms"].as_f64().unwrap() <= 5.,"warm_unity_p95_20ms":warm["p95_ms"].as_f64().unwrap() <= 20.,"sinc_p95_40ms":sinc["p95_ms"].as_f64().unwrap() <= 40.,"first_native_250ms":first_native_ms <= 250.,"pcm_error_1e_6":maximum_error <= 0.000001,"cancel_2s":cancelled_result_rejected && cancellation_ms <= 2000.,"memory_256mib":high_water_kib <= 256*1024,"foreign_plan_rejected":foreign_plan_rejected,"foreign_result_rejected":foreign_result_rejected,"stale_result_rejected":stale_result_rejected,"pins_survive_clear":pinned_after_clear,"cleanup":cleanup,"source_unchanged":source_unchanged});
    let mut gates = gates;
    gates["active_cancel_2s"] = json!(active_rejected && active_cancellation_ms <= 2000.);
    gates["active_cancel_cleanup"] = json!(active_cleanup);
    let mut receipt = json!({"schema":1,"kind":"native_sound_blocks","source":path,"source_fingerprint":before,"source_stream":selected,"sample_rate":rate,"channels":channels,"actual_source_samples":samples,"qualified_whole_seconds":seconds,"output_frames_per_block":frames,"contributions":2,"root_fps":24,"nested_fps":60,"independent_reference":"sequential FFmpeg CLI native f32le without resampling or channel conversion","independent_pcm_sha256":reference_hash,"rendered_blocks_sha256":format!("{:x}",output_digest.finalize()),"maximum_absolute_pcm_error":maximum_error,"preparation":preparation,"native_unity_first_pass":cold,"native_unity_warm":warm,"native_sinc_44100":sinc,"first_native_ms":first_native_ms,"cancellation_ms":cancellation_ms,"pcm_stats":stats,"memory":memory,"memory_high_water_kib":high_water_kib,"budgets":{"planning":5,"warm_render":20,"sinc_render":40,"first_native":250,"cancel":2000,"high_water_kib":256*1024},"gates":gates,"qualified":gates.as_object().unwrap().values().all(|v| v == &Value::Bool(true)),"limits":["sound-only worker evidence; no callback, device, UI playback, drift or codec-process isolation proof","source interval uses whole natural seconds; fractional source-sequence sound tails remain open","channel layout retained; no downmix or device routing"]});
    receipt["active_cancellation_ms"] = json!(active_cancellation_ms);
    Ok(receipt)
}
