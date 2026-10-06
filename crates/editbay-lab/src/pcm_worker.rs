use crate::{Result, metrics};
use editbay_core::{EvaluationSnapshot, Project, StreamFormat};
use editbay_media::{
    Cancellation, Error, PcmProvider, SourceFile, StreamType,
    pcm_worker::{PcmWorker, PcmWorkerBudget},
};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Command,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

fn high_water(pid: u32) -> Result<u64> {
    Ok(std::fs::read_to_string(format!("/proc/{pid}/status"))?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse().ok())
        })
        .ok_or("process high-water memory absent")?)
}
fn signal(pid: u32, signal: &str) -> Result<()> {
    if !Command::new("kill")
        .args([signal, &pid.to_string()])
        .status()?
        .success()
    {
        return Err("failed to signal codec child".into());
    }
    Ok(())
}

/// Qualify actual isolated original-channel PCM and process supervision.
/// `path` supplies read-only media; `executable` hosts --pcm-worker. Returns
/// independent samples, transfer timings, cancellation, retry and resource receipts.
pub fn run(path: &Path, executable: &Path) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let source = SourceFile::open(path, &cancel)?;
    let before = source.fingerprint().clone();
    let probe = source.probe(cancel.clone())?;
    let stream = probe
        .streams
        .iter()
        .find(|s| s.decoder_available && s.kind == StreamType::Audio)
        .ok_or("source has no decodable sound")?
        .index;
    let imported = source.ingest(
        "Original sound".into(),
        &[stream],
        cancel.clone(),
        |_, _| {},
    )?;
    let id = imported.source.id;
    let StreamFormat::Audio {
        sample_rate,
        channels,
    } = &imported.source.streams[0].format
    else {
        return Err("sound format absent".into());
    };
    let rate = *sample_rate;
    let channels = channels.clone();
    let first = imported.source.streams[0].start_tick;
    let mut project = Project::new("Original PCM codec transport")?;
    project.assets.push(imported.asset);
    project.sources.push(imported.source);
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(project))?);
    let (reference, reference_hash) = crate::sound_blocks::independent_pcm(path, stream)?;
    let frames = 4096u32;
    let count = (reference.len() / channels.len() / frames as usize).min(64);
    if count < 2 {
        return Err("PCM qualification requires two complete blocks".into());
    }
    let began = Instant::now();
    let mut worker = PcmWorker::new(
        executable,
        snapshot.clone(),
        PcmWorkerBudget::default(),
        Cancellation::new()?,
    )?;
    let startup_ms = began.elapsed().as_secs_f64() * 1000.;
    let mut cold = vec![];
    let mut max_error = 0f32;
    for n in 0..count {
        let start = first + n as i64 * i64::from(frames);
        let began = Instant::now();
        let result = worker.interval(id, stream, start, frames)?;
        cold.push(began.elapsed().as_secs_f64() * 1000.);
        let offset = n * frames as usize * channels.len();
        for (a, b) in result
            .pcm()
            .samples()
            .iter()
            .zip(&reference[offset..offset + frames as usize * channels.len()])
        {
            max_error = max_error.max((a - b).abs());
        }
        worker.validate_result(&result)?;
    }
    let held = worker.interval(id, stream, first, frames)?;
    let mut hits = vec![];
    let mut same_mapping = true;
    for _ in 0..1000 {
        let began = Instant::now();
        let result = worker.interval(id, stream, first, frames)?;
        hits.push(began.elapsed().as_secs_f64() * 1000.);
        same_mapping &= Arc::ptr_eq(held.pcm(), result.pcm());
    }
    worker.verify_sources()?;
    let child_high_water_kib = high_water(worker.process_id().ok_or("child absent")?)?;
    let transfer = worker.transfer_stats();
    let pid = worker.process_id().ok_or("child absent")?;
    signal(pid, "-KILL")?;
    let death_rejected = worker.interval(id, stream, first, frames).is_err();
    let death_reaped =
        !Path::new(&format!("/proc/{pid}")).exists() && worker.process_id().is_none();
    let old_rejected = worker.validate_result(&held).is_err();
    worker.rebind(snapshot.clone(), Cancellation::new()?)?;
    let fresh = worker.interval(id, stream, first, frames)?;
    let retry_equal = fresh.pcm().samples() == held.pcm().samples();
    worker.validate_result(&fresh)?;
    let retry_rejects_old = worker.validate_result(&held).is_err();
    worker.clear();
    let pins_survive_clear = worker.transfer_stats().mapped_handles == 2
        && worker.transfer_stats().mapped_bytes == frames as usize * channels.len() * 4 * 2;
    drop((held, fresh));
    let clean = worker.transfer_stats().mapped_handles == 0
        && worker.transfer_stats().mapped_bytes == 0
        && worker.stats().sources == 0
        && worker.stats().decoders == 0;
    let mut cancellation = vec![];
    for stop in [false, true] {
        let token = Cancellation::new()?;
        let child_token = token.clone();
        let document = snapshot.clone();
        let executable = executable.to_owned();
        let (sender, receiver) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || -> std::result::Result<(bool, bool), String> {
            let mut worker = PcmWorker::new(
                &executable,
                document,
                PcmWorkerBudget::default(),
                child_token,
            )
            .map_err(|e| e.to_string())?;
            let pid = worker.process_id().ok_or("child absent")?;
            if stop {
                signal(pid, "-STOP").map_err(|e| e.to_string())?;
            }
            sender.send(pid).map_err(|e| e.to_string())?;
            let rejected = matches!(
                worker.interval(id, stream, first, 131072),
                Err(Error::Cancelled)
            );
            worker.clear();
            Ok((
                rejected,
                worker.transfer_stats().mapped_bytes == 0
                    && worker.transfer_stats().mapped_handles == 0
                    && !Path::new(&format!("/proc/{pid}")).exists(),
            ))
        });
        let pid = receiver.recv_timeout(Duration::from_secs(10))?;
        std::thread::sleep(Duration::from_millis(1));
        let began = Instant::now();
        token.cancel();
        let (rejected, clean) = thread
            .join()
            .map_err(|_| "sound worker panicked")?
            .map_err(|e| format!("sound worker: {e}"))?;
        cancellation.push(json!({"stopped":stop,"pid":pid,"cancel_join_reap_ms":began.elapsed().as_secs_f64()*1000.,"rejected":rejected,"clean":clean}));
    }
    source.verify(&cancel)?;
    let source_unchanged = source.fingerprint() == &before;
    let parent_high_water_kib = high_water(std::process::id())?;
    let cold = metrics(&mut cold);
    let hits = metrics(&mut hits);
    let gates = json!({"independent_pcm_1e_6":max_error<=1e-6,"handoff_p95_10ms":hits["p95_ms"].as_f64().unwrap()<=10.,"combined_memory_512mib":parent_high_water_kib+child_high_water_kib<=512*1024,"same_mapping":same_mapping,"death_rejected":death_rejected,"death_reaped":death_reaped,"old_rejected":old_rejected,"retry_equal":retry_equal,"retry_rejects_old":retry_rejects_old,"pins_survive_clear":pins_survive_clear,"cleanup":clean,"source_unchanged":source_unchanged,"active_cancellation":cancellation.iter().all(|r|r["rejected"]==true && r["clean"]==true && r["cancel_join_reap_ms"].as_f64().unwrap()<=2000.)});
    Ok(
        json!({"schema":1,"kind":"isolated_native_pcm","source":path,"source_fingerprint":before,"stream":stream,"sample_rate":rate,"channels":channels,"first_sample":first,"worker_executable":executable,"worker_sha256":crate::hash(executable)?,"independent_reference":"sequential FFmpeg CLI original-channel f32le","independent_pcm_sha256":reference_hash,"distinct_blocks":count,"frames_per_block":frames,"maximum_absolute_pcm_error":max_error,"startup_ms":startup_ms,"first_pass":cold,"cached_handoff":hits,"transfers":transfer,"cancellation":cancellation,"parent_high_water_kib":parent_high_water_kib,"child_high_water_kib":child_high_water_kib,"combined_high_water_kib":parent_high_water_kib+child_high_water_kib,"budgets":{"handoff_p95_ms":10,"cancel_join_reap_ms":2000,"combined_high_water_kib":512*1024},"qualified":gates.as_object().unwrap().values().all(|v|v==&Value::Bool(true)),"gates":gates,"limits":["local PCM process transport only; native streaming callback/device playback/drift and complete delivery remain separate"]}),
    )
}
