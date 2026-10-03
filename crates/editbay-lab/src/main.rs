use editbay_media::{AudioReader, VideoReader, VideoWriter};
use editbay_render::{Compositor, InputTransfer, Precision};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    process::{Child, Command, ExitCode},
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn source_transfer(info: editbay_media::MediaInfo) -> Result<InputTransfer> {
    if ![1, 2].contains(&info.color_primaries) {
        return Err("probe supports BT.709/sRGB or unspecified source primaries only".into());
    }
    match info.color_transfer {
        13 => Ok(InputTransfer::Srgb),
        1 | 2 | 6 => Ok(InputTransfer::Bt709),
        _ => Err("unsupported source transfer; HDR/log requires the managed R2/R7 route".into()),
    }
}

fn hash(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn metrics(values: &mut [f64]) -> Value {
    values.sort_by(f64::total_cmp);
    let p = |fraction: f64| values[((values.len() - 1) as f64 * fraction).ceil() as usize];
    json!({"count":values.len(),"p50_ms":p(0.5),"p95_ms":p(0.95),"max_ms":p(1.0)})
}

fn memory() -> Result<Value> {
    let status = fs::read_to_string("/proc/self/status")?;
    Ok(json!(
        status
            .lines()
            .filter(|l| l.starts_with("VmRSS:") || l.starts_with("VmHWM:"))
            .collect::<Vec<_>>()
    ))
}

fn probe(source: &Path, frames: usize) -> Result<Value> {
    if !(1..=3600).contains(&frames) {
        return Err("probe frame limit must be 1..3600".into());
    }
    let mut decoder = VideoReader::open(source)?;
    let info = decoder.info;
    let transfer = source_transfer(info)?;
    let half = Compositor::with_transfer(
        info.width as u32,
        info.height as u32,
        Precision::Half,
        transfer,
    )?;
    let full = Compositor::with_transfer(
        info.width as u32,
        info.height as u32,
        Precision::Full,
        transfer,
    )?;
    let mut decode_times = Vec::new();
    let mut half_times = Vec::new();
    let mut full_times = Vec::new();
    let mut half_error = 0.0f32;
    let mut full_error = 0.0f32;
    let mut source_digest = Sha256::new();
    let mut first_timestamp = None;
    let mut last_timestamp = None;
    let mut nonmonotonic = 0;
    for _ in 0..frames {
        let started = Instant::now();
        let Some(frame) = decoder.next_frame()? else {
            break;
        };
        decode_times.push(started.elapsed().as_secs_f64() * 1000.0);
        if first_timestamp.is_none() {
            first_timestamp = frame.timestamp_ns;
        }
        if let (Some(previous), Some(next)) = (last_timestamp, frame.timestamp_ns)
            && next <= previous
        {
            nonmonotonic += 1;
        }
        last_timestamp = frame.timestamp_ns;
        source_digest.update(&frame.rgba);
        let expected = editbay_render::reference_with_transfer(&frame.rgba, transfer);
        for (renderer, times, error) in [
            (&half, &mut half_times, &mut half_error),
            (&full, &mut full_times, &mut full_error),
        ] {
            let started = Instant::now();
            let actual = renderer.compose(&frame.rgba)?;
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            for (a, b) in actual.iter().zip(&expected) {
                *error = error.max((a - b).abs());
            }
        }
    }
    if decode_times.is_empty() {
        return Err("source contains no decoded frames".into());
    }
    let count = decode_times.len();
    Ok(json!({
        "schema": 1, "kind": "native_engine_probe",
        "application_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS, "architecture": std::env::consts::ARCH,
        "ffmpeg": editbay_media::version(), "source": source,
        "source_sha256": hash(source)?,
        "decoded_rgba_sha256": format!("{:x}",source_digest.finalize()),
        "profile": info, "input_transfer": transfer,
        "unspecified_color_assumed_bt709": info.color_transfer == 2 || info.color_primaries == 2,
        "adapter": {"name":half.adapter.name,"backend":format!("{:?}",half.adapter.backend),"device_type":format!("{:?}",half.adapter.device_type),"driver":half.adapter.driver,"driver_info":half.adapter.driver_info},
        "decode": metrics(&mut decode_times),
        "fp16_upload_compose_readback": metrics(&mut half_times),
        "fp32_upload_compose_readback": metrics(&mut full_times),
        "max_linear_error": {"fp16":half_error,"fp32":full_error},
        "color_pass": half_error<=0.002 && full_error<=0.00002,
        "timestamps": {"first_ns":first_timestamp,"last_ns":last_timestamp,"nonmonotonic":nonmonotonic},
        "copies": {"cpu_decode_rgba_per_frame":1,"gpu_upload_per_frame":2,"gpu_readback_per_frame":2,"bytes_uploaded":count as u64*info.width as u64*info.height as u64*8,"float_working_space":"linear BT.709/sRGB primaries, opaque alpha-over, FP16 and FP32"},
        "memory": memory()?,
        "limits": ["CPU codec decode; no hardware handoff", "BT.709/sRGB SDR input only; no HDR/log management", "unspecified input transfer/primaries assume BT.709; unspecified YUV matrix assumes BT.709 at HD, BT.601 below HD", "readback is deliberate prototype/export boundary", "no surface presentation or two-hour device drift qualification"]
    }))
}

fn playback(source: &Path, seconds: u32) -> Result<Value> {
    if !(1..=30).contains(&seconds) {
        return Err("playback probe duration must be 1..30 s".into());
    }
    let mut audio = AudioReader::open(source)?;
    let limit = seconds as usize * 48_000 * 2;
    let mut samples = Vec::with_capacity(limit);
    while samples.len() < limit {
        let Some(block) = audio.next_samples()? else {
            break;
        };
        samples.extend_from_slice(&block[..block.len().min(limit - samples.len())]);
    }
    let count = samples.len();
    let mut decoder = VideoReader::open(source)?;
    let mut next = decoder.next_frame()?;
    let origin = next.as_ref().and_then(|v| v.timestamp_ns).unwrap_or(0);
    let clock = editbay_audio::Playback::start(samples)?;
    let started = Instant::now();
    let mut selected = 0;
    let mut dropped = 0;
    let mut schedule_error_ms = Vec::new();
    while !clock.finished() {
        if clock.failed() {
            return Err("audio output callback failed".into());
        }
        if started.elapsed() > Duration::from_secs(u64::from(seconds) + 5) {
            return Err("audio callback clock did not finish within probe deadline".into());
        }
        let now = clock.nanoseconds() as i128;
        let mut due = Vec::new();
        while let Some(frame) = &next {
            let pts = frame
                .timestamp_ns
                .ok_or("playback requires source timestamps")?;
            if i128::from(pts - origin) > now {
                break;
            }
            due.push(pts);
            next = decoder.next_frame()?;
        }
        if let Some(last) = due.last() {
            dropped += due.len() - 1;
            selected += 1;
            schedule_error_ms.push((now - i128::from(*last - origin)) as f64 / 1_000_000.0);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    clock.stop()?;
    Ok(json!({
        "schema": 1, "kind": "device_clock_playback_probe",
        "source_sha256": hash(source)?, "device": clock.profile,
        "decoded_audio_samples": count, "device_callbacks": clock.callbacks(),
        "device_clock_ns": clock.nanoseconds().to_string(),
        "backend_reported_latency_ns": clock.reported_latency_ns(),
        "wall_ms": started.elapsed().as_secs_f64()*1000.0,
        "frames_selected": selected, "frames_dropped": dropped,
        "schedule_lateness": if schedule_error_ms.is_empty(){Value::Null}else{metrics(&mut schedule_error_ms)},
        "callback_failed": clock.failed(),
        "limits": ["predecoded sound capped at 30 seconds", "linear rate conversion for feasibility only", "frames selected on interpolated device clock with backend latency; no window presentation", "physical audibility and speaker latency are unmeasured"]
    }))
}

fn export_worker(source: &Path, temporary: &Path, frames: usize) -> Result<Value> {
    let mut decoder = VideoReader::open(source)?;
    let renderer = Compositor::with_transfer(
        decoder.info.width as u32,
        decoder.info.height as u32,
        Precision::Half,
        source_transfer(decoder.info)?,
    )?;
    let mut encoder = VideoWriter::open_temporary(temporary, decoder.info)?;
    let mut count = 0;
    let mut composed_hash = Sha256::new();
    let started = Instant::now();
    while count < frames {
        let Some(frame) = decoder.next_frame()? else {
            break;
        };
        let encoded = editbay_render::encode(&renderer.compose(&frame.rgba)?);
        composed_hash.update(&encoded);
        encoder.write(&encoded)?;
        count += 1;
    }
    if count == 0 {
        return Err("no exportable pictures".into());
    }
    encoder.finish()?;
    File::open(temporary)?.sync_all()?;
    let mut verify = VideoReader::open(temporary)?;
    let mut verified = 0;
    let mut decoded_hash = Sha256::new();
    while let Some(frame) = verify.next_frame()? {
        decoded_hash.update(&frame.rgba);
        verified += 1;
    }
    if verified != count {
        return Err("encoded frame count failed decode verification".into());
    }
    if composed_hash.finalize() != decoded_hash.finalize() {
        return Err("decoded export pixels differ from the composed codec input".into());
    }
    Ok(
        json!({"kind":"native_export_worker","frames":count,"decoded_output_frames":verified,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"output_sha256":hash(temporary)?,"memory":memory()?,"limits":["prototype picture-only FFV1/Matroska output","constant-rate output; VFR mapping is not yet supported","FP16 composition then sRGB RGBA8 codec boundary"]}),
    )
}

fn export(
    source: &Path,
    destination: &Path,
    frames: usize,
    cancel_ms: Option<u64>,
) -> Result<Value> {
    if !(1..=3600).contains(&frames) {
        return Err("export frame limit must be 1..3600".into());
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if destination.exists() {
        return Err("export destination already exists".into());
    }
    let temporary = NamedTempFile::new_in(parent)?;
    let report = NamedTempFile::new_in(parent)?;
    let source_hash = hash(source)?;
    let started = Instant::now();
    let mut child = Worker(
        Command::new(std::env::current_exe()?)
            .arg("export-worker")
            .arg(source)
            .arg(temporary.path())
            .arg(frames.to_string())
            .stdout(report.reopen()?)
            .spawn()?,
    );
    loop {
        if let Some(status) = child.0.try_wait()? {
            if !status.success() {
                return Err(format!("export worker failed: {status}").into());
            }
            let mut receipt: Value = serde_json::from_slice(&fs::read(report.path())?)?;
            if hash(source)? != source_hash {
                return Err("source changed during export".into());
            }
            temporary.persist_noclobber(destination)?;
            File::open(parent)?.sync_all()?;
            receipt["destination"] = json!(destination);
            receipt["source_sha256"] = json!(source_hash);
            receipt["decoded_pixels_match_composition"] = json!(true);
            receipt["background_process"] = json!(true);
            return Ok(receipt);
        }
        if cancel_ms.is_some_and(|limit| started.elapsed().as_millis() >= u128::from(limit)) {
            let requested = Instant::now();
            child.0.kill()?;
            child.0.wait()?;
            return Ok(
                json!({"kind":"export_cancel","underlying_process_stopped":true,"cancel_ms":requested.elapsed().as_secs_f64()*1000.0,"destination_published":false}),
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn number(value: &OsString) -> Result<usize> {
    Ok(value.to_str().ok_or("number must be UTF-8")?.parse()?)
}

fn run(args: Vec<OsString>) -> Result<()> {
    let command = args.first().and_then(|v| v.to_str()).unwrap_or("--help");
    let receipt = match (command, args.len()) {
        ("probe", 3) => probe(Path::new(&args[1]), number(&args[2])?)?,
        ("playback", 3) => playback(Path::new(&args[1]), u32::try_from(number(&args[2])?)?)?,
        ("export", 4) => export(
            Path::new(&args[1]),
            Path::new(&args[2]),
            number(&args[3])?,
            None,
        )?,
        ("cancel-export", 5) => export(
            Path::new(&args[1]),
            Path::new(&args[2]),
            number(&args[3])?,
            Some(u64::try_from(number(&args[4])?)?),
        )?,
        ("export-worker", 4) => {
            export_worker(Path::new(&args[1]), Path::new(&args[2]), number(&args[3])?)?
        }
        ("--help", 0 | 1) => {
            println!(
                "EditBay native feasibility lab\n  probe SOURCE FRAME_LIMIT\n  playback SOURCE SECONDS\n  export SOURCE NEW_MKV FRAME_LIMIT\n  cancel-export SOURCE NEW_MKV FRAME_LIMIT CANCEL_MS"
            );
            return Ok(());
        }
        _ => return Err("invalid lab command or arguments".into()),
    };
    std::io::stdout().write_all(serde_json::to_string_pretty(&receipt)?.as_bytes())?;
    println!();
    Ok(())
}

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("editbay-lab: {error}");
            ExitCode::FAILURE
        }
    }
}
