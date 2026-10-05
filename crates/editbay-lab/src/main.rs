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

mod gpu_graph;
mod inventory;
mod media_ingest;
mod native_workspace;
mod picture_cache;
mod picture_worker;
mod temporal;

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

fn rvm_probe(source: &Path, model: &Path, runtime: &Path, frames: usize) -> Result<Value> {
    use editbay_ai::{Cancellation, Rvm, RvmState};
    if !(2..=12).contains(&frames) {
        return Err("RVM probe needs 2..12 source frames".into());
    }
    editbay_ai::initialize(runtime)?;
    let source_hash = hash(source)?;
    let mut decoder = VideoReader::open(source)?;
    let width = decoder.info.width as usize;
    let height = decoder.info.height as usize;
    if width * height * 4 * frames > 32 * 1024 * 1024 {
        return Err(
            "RVM comparison pictures exceed the 32 MiB probe budget; reduce frame count".into(),
        );
    }
    let mut pictures = Vec::new();
    for _ in 0..frames {
        let Some(frame) = decoder.next_frame()? else {
            break;
        };
        pictures.push(frame.rgba);
    }
    if pictures.len() < 2 {
        return Err("RVM resume probe needs at least two decoded pictures".into());
    }
    let cancellation = Cancellation::default();
    let started = Instant::now();
    let mut rvm = Rvm::open(
        model,
        source_hash.clone(),
        width,
        height,
        0.25,
        &cancellation,
    )?;
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut times = Vec::new();
    let mut mattes = Vec::new();
    let boundary = pictures.len() / 2;
    let mut serialized = Vec::new();
    let mut min_alpha = 1.0f32;
    let mut max_alpha = 0.0f32;
    for (index, picture) in pictures.iter().enumerate() {
        let started = Instant::now();
        let matte = rvm.process(index as u64, picture, &cancellation)?;
        times.push(started.elapsed().as_secs_f64() * 1000.0);
        for &value in &matte.alpha {
            min_alpha = min_alpha.min(value);
            max_alpha = max_alpha.max(value);
        }
        mattes.push(matte);
        if index + 1 == boundary {
            serialized = serde_json::to_vec(&rvm.snapshot())?;
        }
    }
    let state: RvmState = serde_json::from_slice(&serialized)?;
    let mut resumed = Rvm::restore(model, state.clone(), &cancellation)?;
    let mut resume_error = 0.0f32;
    for (index, picture) in pictures.iter().enumerate().skip(boundary) {
        let result = resumed.process(index as u64, picture, &cancellation)?;
        for (a, b) in result.alpha.iter().chain(&result.foreground_rgb_chw).zip(
            mattes[index]
                .alpha
                .iter()
                .chain(&mattes[index].foreground_rgb_chw),
        ) {
            resume_error = resume_error.max((a - b).abs());
        }
    }
    let cancel = Cancellation::default();
    let mut cancelled_model = Rvm::restore(model, state.clone(), &cancellation)?;
    let started = Instant::now();
    let cancellation_result = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            cancel.cancel();
        });
        cancelled_model.process(boundary as u64, &pictures[boundary], &cancel)
    });
    let cancel_total_ms = started.elapsed().as_secs_f64() * 1000.0;
    let rejected = matches!(cancellation_result, Err(editbay_ai::Error::Cancelled));
    if !rejected || cancelled_model.snapshot() != state {
        return Err("cancelled inference advanced recurrent state".into());
    }
    if hash(source)? != source_hash {
        return Err("source changed during inference".into());
    }
    Ok(json!({
        "schema":1,"kind":"native_rvm_probe","source_sha256":source_hash,
        "model_sha256":editbay_ai::RVM_SHA256,"runtime":runtime,"runtime_sha256":hash(runtime)?,
        "backend":"ONNX Runtime CPU, two threads","input_shape":[1,3,height,width],
        "downsample_ratio":0.25,"frames":pictures.len(),"load_ms":load_ms,
        "inference":metrics(&mut times),"alpha_range":[min_alpha,max_alpha],
        "resume":{"boundary":boundary,"state_json_bytes":serialized.len(),"state_sha256":format!("{:x}",Sha256::digest(&serialized)),"max_alpha_foreground_error":resume_error,"pass":resume_error<=0.0001},
        "cancellation":{"returned_cancelled":rejected,"native_operator_termination_requested":cancel.native_termination_requested(),"request_delay_ms":20,"total_ms":cancel_total_ms,"state_preserved":true},
        "memory":memory()?,
        "limits":["human-matting candidate, not arbitrary-object selection","no ground-truth quality or independent PyTorch/TF reference comparison yet","CPU/ARM64 only measured","raw soft alpha/foreground; source-resolution finishing and editable project assets remain R4 work"]
    }))
}

fn sam2_probe(
    source: &Path,
    pack: &Path,
    runtime: &Path,
    frames: usize,
    x: f32,
    y: f32,
) -> Result<Value> {
    use editbay_ai::{Cancellation, PointPrompt, Sam2, Sam2State};
    if !(2..=8).contains(&frames) {
        return Err("SAM probe needs 2..8 frames".into());
    }
    editbay_ai::initialize(runtime)?;
    let source_hash = hash(source)?;
    let mut decoder = VideoReader::open(source)?;
    let width = decoder.info.width as usize;
    let height = decoder.info.height as usize;
    if width * height * 4 * frames > 32 * 1024 * 1024 {
        return Err("SAM input exceeds 32 MiB probe budget".into());
    }
    let mut pictures = Vec::new();
    for _ in 0..frames {
        let Some(frame) = decoder.next_frame()? else {
            break;
        };
        pictures.push(frame.rgba);
    }
    if pictures.len() < 2 {
        return Err("SAM probe needs at least two pictures".into());
    }
    let cancel = Cancellation::default();
    let initial = Sam2::fresh_state(
        source_hash.clone(),
        width,
        height,
        pictures.len() as u64,
        vec![PointPrompt {
            x,
            y,
            foreground: true,
        }],
    );
    let started = Instant::now();
    let mut sam = Sam2::open(pack, initial, &cancel)?;
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    let boundary = pictures.len() / 2;
    let mut outputs = Vec::new();
    let mut times = Vec::new();
    let mut summary = Vec::new();
    let mut serialized = Vec::new();
    for (i, picture) in pictures.iter().enumerate() {
        let started = Instant::now();
        let result = sam.process(i as u64, picture, &cancel)?;
        times.push(started.elapsed().as_secs_f64() * 1000.0);
        let mask: Vec<u8> = result.logits.iter().map(|&v| u8::from(v > 0.0)).collect();
        summary.push(json!({"frame":i,"foreground_pixels":mask.iter().map(|&v| v as u64).sum::<u64>(),
            "mask_sha256":format!("{:x}",Sha256::digest(&mask)),"predicted_iou":result.predicted_iou,"object_score":result.object_score}));
        outputs.push(result.logits);
        if i + 1 == boundary {
            serialized = serde_json::to_vec(&sam.snapshot())?;
        }
    }
    let state: Sam2State = serde_json::from_slice(&serialized)?;
    let mut resumed = Sam2::open(pack, state.clone(), &cancel)?;
    let mut max_error = 0.0f32;
    let mut worst_iou = 1.0f64;
    for (i, picture) in pictures.iter().enumerate().skip(boundary) {
        let result = resumed.process(i as u64, picture, &cancel)?;
        let mut intersection = 0u64;
        let mut union = 0u64;
        for (&a, &b) in result.logits.iter().zip(&outputs[i]) {
            max_error = max_error.max((a - b).abs());
            intersection += u64::from(a > 0.0 && b > 0.0);
            union += u64::from(a > 0.0 || b > 0.0);
        }
        if union > 0 {
            worst_iou = worst_iou.min(intersection as f64 / union as f64);
        }
    }
    let started = Instant::now();
    let invalid_end_rejected = resumed
        .process(pictures.len() as u64, &pictures[0], &cancel)
        .is_err();
    let range_check_ms = started.elapsed().as_secs_f64() * 1000.0;
    drop(sam);
    drop(resumed);
    let mut cancelled = Sam2::open(pack, state.clone(), &cancel)?;
    let cancellation = Cancellation::default();
    let started_cancel = Instant::now();
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            cancellation.cancel();
        });
        cancelled.process(boundary as u64, &pictures[boundary], &cancellation)
    });
    let cancellation_ms = started_cancel.elapsed().as_secs_f64() * 1000.0;
    if !matches!(result, Err(editbay_ai::Error::Cancelled)) || cancelled.snapshot() != state {
        return Err("cancelled SAM inference advanced model memory".into());
    }
    if hash(source)? != source_hash {
        return Err("source changed during SAM inference".into());
    }
    Ok(
        json!({"schema":1,"kind":"native_sam2_video_probe","source_sha256":source_hash,
        "model_revision":editbay_ai::SAM2_REVISION,"runtime_sha256":hash(runtime)?,
        "source_shape":[height,width],"frames":pictures.len(),"prompt":[x,y],
        "load_ms":load_ms,"inference":metrics(&mut times),"results":summary,
        "resume":{"boundary":boundary,"max_logit_error":max_error,"worst_mask_iou":worst_iou,"state_json_bytes":serialized.len(),"pass":max_error<=0.0001},
        "cancellation":{"request_delay_ms":20,"total_ms":cancellation_ms,"native_operator_termination_requested":cancellation.native_termination_requested(),"state_preserved":true},
        "invalid_range_rejected":invalid_end_rejected,"range_check_ms":range_check_ms,
        "memory":memory()?,"limits":["single object; point prompts on first frame only","fixed 1024-square bilinear RGB normalization; source logits are bilinearly mapped back","no independent reference agreement or human mask-quality claim yet","CPU/ARM64 only measured; no automatic editor asset publication"]}),
    )
}

fn moving_fixture(destination: &Path) -> Result<Value> {
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = NamedTempFile::new_in(parent)?;
    let profile = editbay_media::MediaInfo {
        width: 512,
        height: 288,
        rate_num: 6,
        rate_den: 1,
        ..Default::default()
    };
    let mut writer = VideoWriter::open_temporary(temporary.path(), profile)?;
    for frame in 0..6 {
        let mut rgba = Vec::with_capacity(512 * 288 * 4);
        for y in 0..288 {
            for x in 0..512 {
                let inside =
                    (120 + frame * 10..220 + frame * 10).contains(&x) && (75..195).contains(&y);
                let pixel = if inside {
                    [239, 114, 54, 255]
                } else {
                    [36, 48, 64, 255]
                };
                rgba.extend_from_slice(&pixel);
            }
        }
        writer.write(&rgba)?;
    }
    writer.finish()?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(destination)?;
    File::open(parent)?.sync_all()?;
    Ok(
        json!({"kind":"native_motion_fixture","source_sha256":hash(destination)?,"frames":6,
        "truth":"100x120 orange rectangle; x=120+10*frame, y=75; 512x288, 6 fps; no alpha ground truth"}),
    )
}

#[cfg(feature = "tract-reference")]
fn sam2_parity(
    source: &Path,
    pack: &Path,
    runtime: &Path,
    frames: usize,
    x: f32,
    y: f32,
    motion_truth: bool,
) -> Result<Value> {
    use editbay_ai::{Cancellation, PointPrompt, Sam2};
    if !(2..=8).contains(&frames) {
        return Err("SAM reference comparison needs 2..8 frames".into());
    }
    editbay_ai::initialize(runtime)?;
    let source_hash = hash(source)?;
    let mut decoder = VideoReader::open(source)?;
    let width = decoder.info.width as usize;
    let height = decoder.info.height as usize;
    if width * height * 4 * frames > 32 * 1024 * 1024 {
        return Err("SAM reference input exceeds 32 MiB".into());
    }
    let mut pictures = Vec::new();
    for _ in 0..frames {
        let Some(frame) = decoder.next_frame()? else {
            break;
        };
        pictures.push(frame.rgba);
    }
    if pictures.len() < 2 {
        return Err("SAM reference needs two source pictures".into());
    }
    let initial = Sam2::fresh_state(
        source_hash.clone(),
        width,
        height,
        pictures.len() as u64,
        vec![PointPrompt {
            x,
            y,
            foreground: true,
        }],
    );
    let cancel = Cancellation::default();
    let mut actual = Sam2::open(pack, initial.clone(), &cancel)?;
    let mut outputs = Vec::new();
    for (i, picture) in pictures.iter().enumerate() {
        outputs.push(actual.process(i as u64, picture, &cancel)?);
    }
    drop(actual);
    let mut truth = Vec::new();
    if motion_truth {
        if width != 512 || height != 288 || pictures.len() != 6 {
            return Err("motion reference requires the generated six-frame fixture".into());
        }
        for (i, output) in outputs.iter().enumerate() {
            let mut intersection = 0u64;
            let mut union = 0u64;
            for (pixel, &value) in output.logits.iter().enumerate() {
                let x = pixel % 512;
                let y = pixel / 512;
                let expected = (120 + i * 10..220 + i * 10).contains(&x) && (75..195).contains(&y);
                intersection += u64::from(expected && value > 0.0);
                union += u64::from(expected || value > 0.0);
            }
            let iou = intersection as f64 / union as f64;
            if iou < 0.99 {
                return Err(format!("SAM analytic motion mask IoU {iou} is below 0.99").into());
            }
            truth.push(json!({"frame":i,"mask_iou":iou}));
        }
    }
    let mut reference = Sam2::open(pack, initial, &cancel)?;
    reference.use_reference(pack, &cancel)?;
    let mut reports = Vec::new();
    let mut worst_iou = 1.0f64;
    let mut worst_mae = 0.0f64;
    let mut worst_max = 0.0f32;
    for (i, picture) in pictures.iter().enumerate() {
        let started = Instant::now();
        let result = reference.process(i as u64, picture, &cancel)?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        let mut intersection = 0u64;
        let mut union = 0u64;
        let mut sum = 0.0f64;
        let mut max = 0.0f32;
        for (&a, &b) in outputs[i].logits.iter().zip(&result.logits) {
            intersection += u64::from(a > 0.0 && b > 0.0);
            union += u64::from(a > 0.0 || b > 0.0);
            let error = (a - b).abs();
            sum += f64::from(error);
            max = max.max(error);
        }
        let iou = if union > 0 {
            intersection as f64 / union as f64
        } else {
            1.0
        };
        let mae = sum / result.logits.len() as f64;
        worst_iou = worst_iou.min(iou);
        worst_mae = worst_mae.max(mae);
        worst_max = worst_max.max(max);
        reports.push(json!({"frame":i,"mask_iou":iou,"logit_mae":mae,"max_logit_error":max,
            "reference_ms":elapsed,"object_score_error":(outputs[i].object_score-result.object_score).abs(),
            "predicted_iou_error":(outputs[i].predicted_iou-result.predicted_iou).abs()}));
    }
    if hash(source)? != source_hash {
        return Err("source changed during independent SAM reference".into());
    }
    if worst_iou < 0.99 {
        return Err(format!("SAM reference mask IoU {worst_iou} is below 0.99").into());
    }
    Ok(
        json!({"schema":1,"kind":"native_sam2_independent_parity","source_sha256":source_hash,
        "model_revision":editbay_ai::SAM2_REVISION,"reference_runtime":"Tract 0.23.8 Rust CPU kernels",
        "input_shape":[height,width],"prompt":[x,y],"frames":pictures.len(),"results":reports,
        "worst_mask_iou":worst_iou,"worst_logit_mae":worst_mae,"max_logit_error":worst_max,
        "reference_pass":true,"analytic_motion_truth":truth,"memory":memory()?,
        "limits":["independent graph kernels and recurrent results; shared Rust preprocessing/memory-bank orchestration",
            "no comparison with upstream PyTorch video orchestration yet","not annotated client-footage quality",
            "reference cancellation between graphs only; artist candidate uses ONNX Runtime operator termination"]}),
    )
}

#[cfg(feature = "tract-reference")]
fn sam2_motion_parity(pack: &Path, runtime: &Path) -> Result<Value> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("motion.mkv");
    let fixture = moving_fixture(&source)?;
    let mut result = sam2_parity(&source, pack, runtime, 6, 170.0, 135.0, true)?;
    result["fixture"] = fixture;
    Ok(result)
}

#[cfg(feature = "torch-reference")]
fn rvm_parity(
    source: &Path,
    model: &Path,
    script: &Path,
    runtime: &Path,
    frames: usize,
) -> Result<Value> {
    use editbay_ai::{Cancellation, Rvm};
    if !(2..=30).contains(&frames) {
        return Err("reference comparison needs 2..30 frames".into());
    }
    editbay_ai::initialize(runtime)?;
    let source_hash = hash(source)?;
    let mut decoder = VideoReader::open(source)?;
    let width = decoder.info.width as usize;
    let height = decoder.info.height as usize;
    let cancel = Cancellation::default();
    let mut onnx = Rvm::open(model, source_hash.clone(), width, height, 0.25, &cancel)?;
    let mut reference = editbay_reference::ReferenceRvm::open(script, width, height, 0.25)?;
    let mut reports = Vec::new();
    let mut worst_alpha_mae = 0.0f64;
    let mut worst_fgr_mae = 0.0f64;
    let mut worst_alpha_max = 0.0f32;
    let mut worst_fgr_max = 0.0f32;
    for frame in 0..frames {
        let Some(picture) = decoder.next_frame()? else {
            break;
        };
        let started = Instant::now();
        let actual = onnx.process(frame as u64, &picture.rgba, &cancel)?;
        let onnx_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        let (alpha, foreground) = reference.process(&picture.rgba)?;
        let reference_ms = started.elapsed().as_secs_f64() * 1000.0;
        let error = |a: &[f32], b: &[f32]| {
            let mut sum = 0.0f64;
            let mut max = 0.0f32;
            for (&a, &b) in a.iter().zip(b) {
                let e = (a - b).abs();
                sum += f64::from(e);
                max = max.max(e);
            }
            (sum / a.len() as f64, max)
        };
        let (alpha_mae, alpha_max) = error(&actual.alpha, &alpha);
        let (fgr_mae, fgr_max) = error(&actual.foreground_rgb_chw, &foreground);
        worst_alpha_mae = worst_alpha_mae.max(alpha_mae);
        worst_fgr_mae = worst_fgr_mae.max(fgr_mae);
        worst_alpha_max = worst_alpha_max.max(alpha_max);
        worst_fgr_max = worst_fgr_max.max(fgr_max);
        reports.push(
            json!({"frame":frame,"timestamp_ns":picture.timestamp_ns,"alpha_mae":alpha_mae,
            "alpha_max_error":alpha_max,"foreground_mae":fgr_mae,"foreground_max_error":fgr_max,
            "onnx_ms":onnx_ms,"libtorch_ms":reference_ms}),
        );
    }
    if reports.len() < 2 {
        return Err("reference comparison needs two decoded frames".into());
    }
    if hash(source)? != source_hash {
        return Err("source changed during reference comparison".into());
    }
    let pass = worst_alpha_mae <= 0.001;
    if !pass {
        return Err(format!("alpha reference error {worst_alpha_mae} exceeds 0.001").into());
    }
    Ok(
        json!({"schema":1,"kind":"native_rvm_independent_parity","source_sha256":source_hash,
        "onnx_sha256":editbay_ai::RVM_SHA256,"torchscript_sha256":editbay_reference::TORCHSCRIPT_SHA256,
        "runtime_sha256":hash(runtime)?,"libtorch_version":"2.10.0+cpu",
        "input_shape":[1,3,height,width],"downsample_ratio":0.25,"results":reports,
        "worst_alpha_mae":worst_alpha_mae,"worst_alpha_max_error":worst_alpha_max,
        "worst_foreground_mae":worst_fgr_mae,"worst_foreground_max_error":worst_fgr_max,"alpha_reference_pass":pass,
        "memory":memory()?,"limits":["native runtime agreement on identical decoded pictures; no ground-truth matte-quality claim","CPU ARM64 only; no Python installed or executed"]}),
    )
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
        ("media-ingest", 2) => media_ingest::run(Path::new(&args[1]))?,
        ("picture-cache", 2) => picture_cache::run(Path::new(&args[1]))?,
        ("render-graph", 2) => gpu_graph::run(Path::new(&args[1]))?,
        ("picture-worker", 2 | 3) => picture_worker::run(
            Path::new(&args[1]),
            args.get(2)
                .map(Path::new)
                .unwrap_or(&std::env::current_exe()?),
        )?,
        ("render-graph-worker", 2 | 3) => gpu_graph::run_process(
            Path::new(&args[1]),
            args.get(2)
                .map(Path::new)
                .unwrap_or(&std::env::current_exe()?),
        )?,
        ("evaluation", 3 | 4) => temporal::run(
            Path::new(&args[1]),
            number(&args[2])?,
            args.get(3).map(|value| number(value)).transpose()?,
        )?,
        ("playback", 3) => playback(Path::new(&args[1]), u32::try_from(number(&args[2])?)?)?,
        ("runtime-info", 2) => {
            let library = Path::new(&args[1]);
            editbay_ai::initialize(library)?;
            json!({"kind":"native_runtime_provenance","sha256":hash(library)?,"build":editbay_ai::runtime_build_info()?})
        }
        ("inventory", 3) => inventory::run(Path::new(&args[1]), Path::new(&args[2]))?,
        ("native-workspace", 4) => {
            native_workspace::run(Path::new(&args[1]), Path::new(&args[2]), number(&args[3])?)?
        }
        ("native-media", 4) => native_workspace::media(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
        )?,
        ("native-workspace-timing", 3) => {
            native_workspace::timing(Path::new(&args[1]), Path::new(&args[2]))?
        }
        ("native-workspace-errors", 3) => {
            native_workspace::errors(Path::new(&args[1]), Path::new(&args[2]))?
        }
        ("native-error-worker", 3) => {
            native_workspace::error_trial(Path::new(&args[1]), Path::new(&args[2]), true)?
        }
        ("rvm", 5) => rvm_probe(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            number(&args[4])?,
        )?,
        ("sam2", 7) => sam2_probe(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            number(&args[4])?,
            args[5].to_str().ok_or("point must be UTF-8")?.parse()?,
            args[6].to_str().ok_or("point must be UTF-8")?.parse()?,
        )?,
        ("sam2-fixture", 2) => moving_fixture(Path::new(&args[1]))?,
        #[cfg(feature = "tract-reference")]
        ("sam2-parity", 7) => sam2_parity(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            number(&args[4])?,
            args[5].to_str().ok_or("point must be UTF-8")?.parse()?,
            args[6].to_str().ok_or("point must be UTF-8")?.parse()?,
            false,
        )?,
        #[cfg(feature = "tract-reference")]
        ("sam2-motion-parity", 3) => sam2_motion_parity(Path::new(&args[1]), Path::new(&args[2]))?,
        #[cfg(feature = "torch-reference")]
        ("rvm-parity", 6) => rvm_parity(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            Path::new(&args[4]),
            number(&args[5])?,
        )?,
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
                "EditBay native feasibility lab\n  probe SOURCE FRAME_LIMIT\n  media-ingest SOURCE\n  picture-cache SOURCE\n  render-graph SOURCE\n  picture-worker SOURCE [WORKER_BINARY]\n  render-graph-worker SOURCE [WORKER_BINARY]\n  evaluation SOURCE ITERATIONS [SYNTHETIC_INDEX_PICTURES]\n  playback SOURCE SECONDS\n  export SOURCE NEW_MKV FRAME_LIMIT\n  cancel-export SOURCE NEW_MKV FRAME_LIMIT CANCEL_MS\n  runtime-info ORT_LIBRARY\n  inventory MEDIA_DIRECTORY NEW_JSON_REPORT\n  native-workspace APP_BINARY NEW_EVIDENCE_DIRECTORY TRIALS\n  native-media APP_BINARY CAMERA_SOURCE NEW_EVIDENCE_DIRECTORY\n  native-workspace-timing APP_BINARY NEW_EVIDENCE_DIRECTORY\n  native-workspace-errors APP_BINARY NEW_EVIDENCE_DIRECTORY\n  rvm SOURCE VERIFIED_MODEL ORT_LIBRARY FRAME_LIMIT\n  sam2 SOURCE VERIFIED_PACK ORT_LIBRARY FRAME_LIMIT X Y\n  sam2-fixture NEW_MKV"
            );
            #[cfg(feature = "torch-reference")]
            println!(
                "  rvm-parity SOURCE VERIFIED_MODEL VERIFIED_TORCHSCRIPT ORT_LIBRARY FRAME_LIMIT"
            );
            #[cfg(feature = "tract-reference")]
            println!(
                "  sam2-parity SOURCE VERIFIED_PACK ORT_LIBRARY FRAME_LIMIT X Y\n  sam2-motion-parity VERIFIED_PACK ORT_LIBRARY"
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
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--picture-worker")) {
        return match editbay_media::picture_worker::serve() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("editbay picture worker: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match run(std::env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("editbay-lab: {error}");
            ExitCode::FAILURE
        }
    }
}
