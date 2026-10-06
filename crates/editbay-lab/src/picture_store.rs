use crate::{Result, Worker, hash, metrics};
use editbay_core::{
    DocumentVersion, EvaluationSnapshot, FrameRange, PictureTiming, SourceRequest, StreamFormat,
};
use editbay_media::{
    Cancellation, PictureProvider,
    picture_store::{StoreBudget, StorePlan, StoreSpace},
    picture_worker::{PictureWorker, WorkerBudget},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use uuid::Uuid;

/// Qualify exact anonymous picture storage through the packaged codec process.
/// `project`, `composition`, new `directory` and `executable` select real inputs.
/// Returns independent pixel comparisons, declared bounds, faults and pin cleanup.
pub fn run(
    project: &Path,
    composition: Uuid,
    directory: &Path,
    executable: &Path,
) -> Result<Value> {
    fs::create_dir(directory)?;
    let original = hash(project)?;
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(editbay_core::load(
        project,
    )?))?);
    let end = snapshot
        .project()
        .compositions
        .iter()
        .find(|c| c.id == composition)
        .ok_or("Composition is absent")?
        .duration;
    let cancel = Cancellation::new()?;
    let budget = WorkerBudget::default();
    let plan = || {
        StorePlan::new(
            snapshot.clone(),
            composition,
            FrameRange { start: 0, end },
            budget.pictures,
            StoreBudget::default(),
            &Cancellation::new()?,
        )
    };
    let planned = plan()?;
    let requests: Vec<_> = planned.requests().cloned().collect();
    if requests.is_empty() {
        return Err("Qualification requires actual source pictures".into());
    }
    let summary = planned.summary();
    let space = StoreSpace::new(StoreBudget::default())?;
    let mut provider = PictureWorker::new(executable, snapshot.clone(), budget, cancel.clone())?;
    let preparation_pid = provider.process_id().ok_or("Preparation codec absent")?;
    let started = Instant::now();
    let store = planned.prepare(directory, &space, &mut provider, &cancel, |_| {})?;
    let preparation_ms = started.elapsed().as_secs_f64() * 1000.;
    let preparation_stats = provider.transfer_stats();
    drop(provider);
    if Path::new(&format!("/proc/{preparation_pid}")).exists() {
        return Err("Preparation child was not reaped".into());
    }
    let mut expected = BTreeMap::new();
    let mut streams = BTreeMap::new();
    for request in &requests {
        let SourceRequest::Media {
            source,
            stream,
            picture: Some(picture),
            ..
        } = request
        else {
            return Err("Preparation retained a non-picture request".into());
        };
        streams
            .entry((*source, *stream))
            .or_insert_with(Vec::new)
            .push(*picture);
    }
    for ((source, stream), pictures) in streams {
        let (asset, profile, _) = snapshot.source_stream(source, stream)?;
        let StreamFormat::Video {
            width,
            height,
            timing: PictureTiming::Variable {
                presentation_ticks, ..
            },
            ..
        } = &profile.format
        else {
            return Err("Source has no actual picture index".into());
        };
        let mut decoder = Worker(
            Command::new("ffmpeg")
                .args(["-v", "error", "-noautorotate", "-i"])
                .arg(&asset.path)
                .args([
                    "-map",
                    &format!("0:{stream}"),
                    "-fps_mode",
                    "passthrough",
                    "-pix_fmt",
                    "rgba",
                    "-f",
                    "rawvideo",
                    "pipe:1",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?,
        );
        let mut output = decoder.0.stdout.take().ok_or("Independent pixels absent")?;
        let mut bytes = vec![0; *width as usize * *height as usize * 4];
        for ordinal in 0..presentation_ticks.len() {
            output.read_exact(&mut bytes)?;
            if pictures.contains(&(ordinal as u64)) {
                expected.insert(
                    (source, stream, ordinal as u64),
                    format!("{:x}", Sha256::digest(&bytes)),
                );
            }
        }
        if output.read(&mut [0])? != 0 || !decoder.0.wait()?.success() {
            return Err("Independent picture count or decode failed".into());
        }
    }
    let mut worker =
        PictureWorker::new(executable, snapshot.clone(), budget, Cancellation::new()?)?;
    worker.attach_store(store.clone())?;
    let mut read_ms = Vec::new();
    let mut comparisons = Vec::new();
    for request in requests.iter().rev() {
        let started = Instant::now();
        let result = worker.picture(request)?.ok_or("Stored picture absent")?;
        read_ms.push(started.elapsed().as_secs_f64() * 1000.);
        worker.validate_result(&result)?;
        let SourceRequest::Media {
            source,
            stream,
            picture: Some(picture),
            ..
        } = request
        else {
            unreachable!()
        };
        let digest = format!("{:x}", Sha256::digest(result.picture.rgba()));
        if expected.get(&(*source, *stream, *picture)) != Some(&digest) {
            return Err("Prepared RGBA pixels differ from independent FFmpeg decode".into());
        }
        comparisons
            .push(json!({"source":source,"stream":stream,"ordinal":picture,"sha256":digest}));
    }
    worker.verify_sources()?;
    let stored_stats = worker.transfer_stats();
    if stored_stats.child.stored_reads as usize != requests.len()
        || stored_stats.child.decoders != 0
    {
        return Err("Stored route used a decoder or omitted planned reads".into());
    }
    let mut changed = (**snapshot.project()).clone();
    changed.revision += 1;
    let foreign = Arc::new(EvaluationSnapshot::new(Arc::new(changed))?);
    worker.rebind(foreign, Cancellation::new()?)?;
    if worker.attach_store(store.clone()).is_ok() {
        return Err("Stale prepared document was accepted".into());
    }
    worker.rebind(snapshot.clone(), Cancellation::new()?)?;
    worker.attach_store(store.clone())?;
    let killed_pid = worker.process_id().ok_or("Read codec absent")?;
    if !Command::new("kill")
        .args(["-KILL", &killed_pid.to_string()])
        .status()?
        .success()
    {
        return Err("Codec kill failed".into());
    }
    if worker.picture(&requests[0]).is_ok() || worker.process_id().is_some() {
        return Err("Dead prepared reader was accepted".into());
    }
    drop(worker);
    let cancel = Cancellation::new()?;
    let mut worker = PictureWorker::new(executable, snapshot.clone(), budget, cancel.clone())?;
    worker.attach_store(store.clone())?;
    let stopped_pid = worker.process_id().ok_or("Cancellation codec absent")?;
    if !Command::new("kill")
        .args(["-STOP", &stopped_pid.to_string()])
        .status()?
        .success()
    {
        return Err("Codec stop failed".into());
    }
    let request = requests[0].clone();
    let (sent, received) = mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        let result = worker.picture(&request);
        let cancelled = matches!(result, Err(editbay_media::Error::Cancelled));
        drop(worker);
        let _ = sent.send(cancelled);
    });
    let started = Instant::now();
    cancel.cancel();
    let cancelled = received.recv_timeout(Duration::from_secs(2))?;
    thread.join().map_err(|_| "Cancelled reader panicked")?;
    let cancellation_ms = started.elapsed().as_secs_f64() * 1000.;
    if !cancelled || Path::new(&format!("/proc/{stopped_pid}")).exists() {
        return Err("Stopped prepared reader did not cancel/reap".into());
    }
    let partial_cancel = Cancellation::new()?;
    let mut worker =
        PictureWorker::new(executable, snapshot.clone(), budget, partial_cancel.clone())?;
    let partial = plan()?.prepare(directory, &space, &mut worker, &partial_cancel, |p| {
        if p.pictures == 1 {
            partial_cancel.cancel();
        }
    });
    if !matches!(partial, Err(editbay_media::Error::Cancelled))
        || space.usage().bytes != summary.bytes
    {
        return Err("Partial preparation leaked its reservation".into());
    }
    drop(worker);
    let pinned = space.usage();
    drop(store);
    let released = space.usage();
    if released.bytes != 0
        || released.entries != 0
        || fs::read_dir(directory)?.count() != 0
        || hash(project)? != original
    {
        return Err("Prepared pictures did not clean up or altered the source project".into());
    }
    let receipt = json!({"kind":"exact_picture_store","qualified":true,"version":DocumentVersion::of(snapshot.project()),"project_sha256":original,
        "worker_sha256":hash(executable)?,"plan":summary,"preparation_ms":preparation_ms,"preparation_stats":preparation_stats,
        "reads":metrics(&mut read_ms),"stored_stats":stored_stats,"independent_pixels":comparisons,
        "stale_rejected":true,"killed_child_reaped":true,"stopped_reader_cancelled_ms":cancellation_ms,
        "partial_cancelled":true,"pinned":pinned,"released":released,"limits":["Exact CPU source pixels only; native playback and physical hardware gates remain separate"]});
    fs::write(
        directory.join("qualification.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(receipt)
}
