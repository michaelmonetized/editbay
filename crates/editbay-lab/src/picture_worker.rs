use crate::{Result, memory, metrics};
use editbay_core::{
    DocumentVersion, EvaluationSnapshot, PictureTiming, Project, SourcePosition, SourceRequest,
    StreamFormat,
};
use editbay_media::{
    Cancellation, Error, PictureProvider, SourceFile, StreamType, VideoReader,
    picture_worker::{PictureWorker, WorkerBudget},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    process::Command,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

fn process_memory(pid: u32) -> Result<Value> {
    let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
    Ok(
        json!({"pid":pid,"status":status.lines().filter(|l| l.starts_with("VmRSS:") || l.starts_with("VmHWM:")).collect::<Vec<_>>(),
        "descriptors":fs::read_dir(format!("/proc/{pid}/fd"))?.count()}),
    )
}

/// Qualify retained codec processes and sealed plane handoff on actual source media.
/// `path` selects read-only media; `executable` supports --picture-worker.
/// Returns independent pixels, miss/hit/startup timings, cancellation, recovery and
/// charged pin/handle evidence, without claiming native surface or sound playback.
pub fn run(path: &Path, executable: &Path) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let source = SourceFile::open(path, &cancel)?;
    let probe = source.probe(cancel.clone())?;
    let stream = probe
        .streams
        .iter()
        .find(|s| s.decoder_available && s.kind == StreamType::Video)
        .ok_or("no picture stream")?
        .index;
    let imported = source.ingest(
        "Real isolated picture qualification".into(),
        &[stream],
        cancel.clone(),
        |_, _| {},
    )?;
    let mut project = Project::new("Sealed native codec handoff")?;
    project.assets = vec![imported.asset];
    project.sources = vec![imported.source];
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(project))?);
    let (asset, profile, interpretation) =
        snapshot.source_stream(snapshot.project().sources[0].id, stream)?;
    let StreamFormat::Video {
        width,
        height,
        timing:
            PictureTiming::Variable {
                presentation_ticks,
                end_tick,
            },
        ..
    } = &profile.format
    else {
        return Err("actual index missing".into());
    };
    let budget = WorkerBudget::default();
    let bytes = *width as usize * *height as usize * 4;
    let count = (budget.pictures.cache_bytes / bytes)
        .min(25)
        .min(presentation_ticks.len());
    if count < 2 {
        return Err("need two budgeted real pictures".into());
    }
    let targets: Vec<_> = (0..count)
        .map(|i| i * (presentation_ticks.len() - 1) / (count - 1))
        .collect();
    let forward_targets: Vec<_> = [0, 3, 12, 22, 24, 23, 26, 0, 8, presentation_ticks.len() - 1]
        .into_iter()
        .filter(|ordinal| *ordinal < presentation_ticks.len())
        .collect();
    let request = |ordinal: usize| -> SourceRequest {
        SourceRequest::Media {
            source: snapshot.project().sources[0].id,
            stream,
            asset: asset.id,
            asset_sha256: asset.sha256.clone(),
            stream_sha256: interpretation.into(),
            position: SourcePosition {
                numerator: presentation_ticks[ordinal],
                denominator: 1,
            },
            reverse: false,
            picture: Some(ordinal as u64),
            sample: None,
        }
    };
    let mut reader = VideoReader::open_stream(&source, stream, cancel.clone())?;
    let mut expected = HashMap::new();
    let mut decoded = 0;
    while let Some(frame) = reader.next_frame()? {
        if frame.source_tick != presentation_ticks.get(decoded).copied() {
            return Err("independent source tick mismatch".into());
        }
        if targets.contains(&decoded) || forward_targets.contains(&decoded) {
            expected.insert(
                decoded,
                (
                    format!("{:x}", Sha256::digest(&frame.rgba)),
                    frame.color,
                    frame.alpha,
                ),
            );
        }
        decoded += 1;
    }
    if decoded != presentation_ticks.len() || reader.next_frame()?.is_some() {
        return Err("independent indexed EOF mismatch".into());
    }
    drop(reader);
    let started = Instant::now();
    let mut worker =
        PictureWorker::new(executable, snapshot.clone(), budget, Cancellation::new()?)?;
    let startup_ms = started.elapsed().as_secs_f64() * 1000.;
    let pid = worker.process_id().ok_or("codec did not start")?;
    let limits = fs::read_to_string(format!("/proc/{pid}/limits"))?;
    let mut misses = Vec::new();
    let mut pixel_checks = Vec::new();
    for target in &targets {
        let started = Instant::now();
        let result = worker
            .picture(&request(*target))?
            .ok_or("picture missing")?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.;
        let reference = &expected[target];
        let hash = format!("{:x}", Sha256::digest(result.picture.rgba()));
        if hash != reference.0
            || result.picture.color != reference.1
            || result.picture.alpha != reference.2
        {
            return Err("isolated pixels/color/alpha differ from sequential reference".into());
        }
        worker.validate_result(&result)?;
        misses.push(elapsed);
        pixel_checks.push(json!({"ordinal":target,"tick":result.picture.source_tick,"rgba_sha256":hash,"cache_hit":result.cache_hit}));
    }
    let mut reverse = request(decoded - 1);
    if let SourceRequest::Media {
        position, reverse, ..
    } = &mut reverse
    {
        *position = SourcePosition::new(*end_tick, 1)?;
        *reverse = true;
    }
    let last = worker
        .picture(&reverse)?
        .ok_or("reverse exclusive end missing")?;
    if format!("{:x}", Sha256::digest(last.picture.rgba())) != expected[&(decoded - 1)].0 {
        return Err("reverse end differs".into());
    }
    drop(last);
    let warm = worker.picture(&request(0))?.ok_or("warm picture missing")?;
    let before = worker.transfer_stats();
    let mut hits = Vec::new();
    for _ in 0..1000 {
        let started = Instant::now();
        let result = worker.picture(&request(0))?.ok_or("hit picture missing")?;
        hits.push(started.elapsed().as_secs_f64() * 1000.);
        if !Arc::ptr_eq(&warm.picture, &result.picture) || !result.cache_hit {
            return Err("IPC hit does not share verified content".into());
        }
        worker.validate_result(&result)?;
    }
    let after = worker.transfer_stats();
    if after.child.misses != before.child.misses
        || after.child.seeks != before.child.seeks
        || after.child.sequential_decodes != before.child.sequential_decodes
        || after.child.forward_decodes != before.child.forward_decodes
        || after.child.skipped_pictures != before.child.skipped_pictures
    {
        return Err("already-decoded handoff caused native decode".into());
    }
    let hit_times = metrics(&mut hits);
    let before_clear = worker.transfer_stats();
    let first_memory = process_memory(pid)?;
    worker.verify_sources()?;
    let old = worker
        .picture(&request(0))?
        .ok_or("owned receipt missing")?;
    if !Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()?
        .success()
    {
        return Err("owned codec kill failed".into());
    }
    let started = Instant::now();
    let death = worker.picture(&request(0));
    if death.is_ok() || worker.process_id().is_some() || worker.validate_result(&old).is_ok() {
        return Err("dead codec did not fail visibly".into());
    }
    let death_ms = started.elapsed().as_secs_f64() * 1000.;
    let mut renamed = (**snapshot.project()).clone();
    renamed.rename("Metadata revision after worker death")?;
    let next = Arc::new(EvaluationSnapshot::new(Arc::new(renamed))?);
    let started = Instant::now();
    worker.rebind(next.clone(), Cancellation::new()?)?;
    let fresh = worker
        .picture(&request(0))?
        .ok_or("retry picture missing")?;
    let retry_ms = started.elapsed().as_secs_f64() * 1000.;
    if worker.validate_result(&old).is_ok()
        || !Arc::ptr_eq(&old.picture, &fresh.picture)
        || fresh.version != DocumentVersion::of(next.project())
    {
        return Err("retry/stale ownership failed".into());
    }
    worker.validate_result(&fresh)?;
    worker.verify_sources()?;
    drop(warm);
    drop(old);
    drop(fresh);
    worker.clear()?;
    let mut sequential = Vec::new();
    for ordinal in 0..decoded.min(120) {
        let started = Instant::now();
        let result = worker
            .picture(&request(ordinal))?
            .ok_or("sequential picture missing")?;
        sequential.push(started.elapsed().as_secs_f64() * 1000.);
        worker.validate_result(&result)?;
    }
    let sequential_memory =
        process_memory(worker.process_id().ok_or("sequential worker stopped")?)?;
    worker.verify_sources()?;
    worker.clear()?;
    let cleanup = worker.transfer_stats();
    if cleanup.mapped_bytes != 0
        || cleanup.mapped_handles != 0
        || cleanup.child.sources != 0
        || cleanup.child.decoders != 0
    {
        return Err("isolated cleanup retained owned resources".into());
    }
    let cancellation = active_cancel(executable, snapshot.clone(), request(decoded - 1))?;
    let stopped = stopped_cancel(executable, snapshot.clone(), request(decoded - 1))?;
    let mut forward_budget = budget;
    forward_budget.pictures.cache_bytes = 0;
    forward_budget.pictures.cache_entries = 0;
    let mut forward = PictureWorker::new(
        executable,
        snapshot.clone(),
        forward_budget,
        Cancellation::new()?,
    )?;
    let forward_pid = forward.process_id().ok_or("forward codec absent")?;
    let mut next = 0;
    let (mut expected_sequential, mut expected_forward, mut expected_skipped, mut expected_seeks) =
        (0, 0, 0, 0);
    let mut gap_checks = Vec::new();
    let mut gap_times = Vec::new();
    for ordinal in forward_targets {
        let route = if ordinal == next {
            expected_sequential += 1;
            "next"
        } else if ordinal > next && ordinal - next <= 8 {
            expected_forward += 1;
            expected_skipped += ordinal - next;
            "forward"
        } else {
            expected_seeks += 1;
            "seek"
        };
        let started = Instant::now();
        let result = forward
            .picture(&request(ordinal))?
            .ok_or("forward picture absent")?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.;
        let hash = format!("{:x}", Sha256::digest(result.picture.rgba()));
        if hash != expected[&ordinal].0
            || result.picture.color != expected[&ordinal].1
            || result.picture.alpha != expected[&ordinal].2
            || result.picture.source_tick != presentation_ticks[ordinal]
            || result.cache_hit
        {
            return Err(
                "uncached forward pixels/metadata differ from independent reference".into(),
            );
        }
        forward.validate_result(&result)?;
        gap_checks.push(json!({"ordinal":ordinal,"route":route,"tick":result.picture.source_tick,"rgba_sha256":hash,"elapsed_ms":elapsed}));
        gap_times.push(elapsed);
        next = ordinal + 1;
    }
    let forward_stats = forward.transfer_stats();
    if forward_stats.child.sequential_decodes != expected_sequential
        || forward_stats.child.forward_decodes != expected_forward
        || forward_stats.child.skipped_pictures != expected_skipped as u64
        || forward_stats.child.seeks != expected_seeks
        || forward_stats.mapped_bytes != 0
        || forward_stats.mapped_handles != 0
    {
        return Err("forward route counters or uncached pin cleanup differ".into());
    }
    forward.verify_sources()?;
    let forward_memory = process_memory(forward_pid)?;
    forward.clear()?;
    if Path::new(&format!("/proc/{forward_pid}")).exists() {
        return Err("forward codec was not reaped".into());
    }
    let forward_receipt = json!({"checks":gap_checks,"timings":metrics(&mut gap_times),"before_clear":forward_stats,
        "cleanup":forward.transfer_stats(),"child_memory":forward_memory,"worker_reaped":true});
    source.verify(&cancel)?;
    let first_sequential_ms = sequential.remove(0);
    Ok(
        json!({"schema":1,"kind":"isolated_retained_sealed_picture_handoff","source":path,"fingerprint":source.fingerprint(),
        "codec_runtime":probe.codec_runtime,"selected_stream":stream,"geometry":[width,height],"rgba_bytes":bytes,"source_index_pictures":decoded,
        "worker_executable":executable,"budget":budget,"startup_bind_ms":startup_ms,"source_misses":metrics(&mut misses),
        "independent_pixels":pixel_checks,"reverse_after_eof_equal":true,"already_decoded_ipc_handoff":hit_times,
        "ipc_p95_budget_ms":10.,"ipc_budget_pass":hit_times["p95_ms"].as_f64().is_some_and(|v|v<=10.),
        "before_hits":before,"after_hits":after,"before_clear":before_clear,"worker_limits":limits,
        "worker_death":{"failed_visibly":true,"reaped":!Path::new(&format!("/proc/{pid}")).exists(),"error_latency_ms":death_ms,"retry_ms":retry_ms,"matching_content_shared":true,"old_receipt_rejected":true},
        "first_sequential_includes_restart_and_binding_ms":first_sequential_ms,"steady_sequential_pictures":metrics(&mut sequential),
        "active_cancel":cancellation,"stopped_cancel":stopped,"cleanup":cleanup,"forward_gaps":forward_receipt,"parent_memory":memory()?,
        "child_memory":[first_memory,sequential_memory],"includes_gpu_or_native_surface":false,"full_r2_gate_pass":false}),
    )
}

fn active_cancel(
    executable: &Path,
    snapshot: Arc<EvaluationSnapshot>,
    target: SourceRequest,
) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let child_cancel = cancel.clone();
    let executable = executable.to_owned();
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = std::thread::spawn(move || -> std::result::Result<_, String> {
        let run = || -> Result<_> {
            let mut worker =
                PictureWorker::new(&executable, snapshot, WorkerBudget::default(), child_cancel)?;
            let pid = worker.process_id().ok_or("active codec absent")?;
            ready_tx.send(pid)?;
            let result = worker.picture(&target);
            let cancelled = matches!(result, Err(Error::Cancelled));
            drop(result);
            worker.clear()?;
            Ok((
                cancelled,
                worker.transfer_stats(),
                !Path::new(&format!("/proc/{pid}")).exists(),
            ))
        };
        run().map_err(|e| e.to_string())
    });
    let ready = ready_rx.recv_timeout(Duration::from_secs(10));
    if ready.is_ok() {
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    cancel.cancel();
    let result = thread
        .join()
        .map_err(|_| "active codec thread panicked")?
        .map_err(std::io::Error::other)?;
    ready?;
    let ms = started.elapsed().as_secs_f64() * 1000.;
    if !result.0
        || !result.2
        || ms > 2000.
        || result.1.mapped_bytes != 0
        || result.1.mapped_handles != 0
    {
        return Err("active codec cancellation failed its gate".into());
    }
    Ok(
        json!({"phase":"outstanding first-binding/uncached picture IPC request; child native phase is not separately instrumented",
        "cancelled":true,"after_cancel_ms":ms,"budget_ms":2000.,"delay_ms":10,"reaped":true,"joined":true,"cleanup":result.1,"pass":true}),
    )
}

fn stopped_cancel(
    executable: &Path,
    snapshot: Arc<EvaluationSnapshot>,
    target: SourceRequest,
) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let mut worker = PictureWorker::new(
        executable,
        snapshot,
        WorkerBudget::default(),
        cancel.clone(),
    )?;
    let pid = worker.process_id().ok_or("stopped codec absent")?;
    if !Command::new("kill")
        .args(["-STOP", &pid.to_string()])
        .status()?
        .success()
    {
        return Err("owned codec stop failed".into());
    }
    let trigger = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        cancel.cancel();
        Instant::now()
    });
    let result = worker.picture(&target);
    let start = trigger.join().map_err(|_| "stop trigger panicked")?;
    let ms = start.elapsed().as_secs_f64() * 1000.;
    if !matches!(result, Err(Error::Cancelled))
        || ms > 2000.
        || Path::new(&format!("/proc/{pid}")).exists()
    {
        return Err("unresponsive codec cancellation did not reap".into());
    }
    worker.clear()?;
    Ok(
        json!({"phase":"SIGSTOP fault injection on the owned codec; EOF grace then kill/reap", "after_cancel_ms":ms,"budget_ms":2000.,"reaped":true,"pass":true}),
    )
}
