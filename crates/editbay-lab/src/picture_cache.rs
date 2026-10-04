use crate::{Result, memory, metrics};
use editbay_core::{
    DocumentVersion, EvaluationSnapshot, PictureTiming, Project, SourcePosition, SourceRequest,
    StreamFormat,
};
use editbay_media::{
    Cancellation, Error, PictureBudget, PictureCache, SourceFile, StreamType, VideoReader,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

/// Qualify source-owned raw-picture caching with actual native decoded pixels.
/// `path` selects read-only media. Returns independent pixel comparisons, empty
/// cache misses, immutable cache hits, sequential timings, budgets and cancellation.
pub fn run(path: &Path) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let source = SourceFile::open(path, &cancel)?;
    let probe = source.probe(cancel.clone())?;
    let selected = probe
        .streams
        .iter()
        .find(|stream| stream.decoder_available && stream.kind == StreamType::Video)
        .ok_or("source has no decodable picture stream")?
        .index;
    let imported = source.ingest(
        "Real picture-cache qualification".into(),
        &[selected],
        cancel.clone(),
        |_, _| {},
    )?;
    let mut project = Project::new("Source-owned picture worker")?;
    project.assets = vec![imported.asset];
    project.sources = vec![imported.source];
    let source_id = project.sources[0].id;
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(project))?);
    let (asset, profile, interpretation) = snapshot.source_stream(source_id, selected)?;
    let StreamFormat::Video {
        width,
        height,
        timing: PictureTiming::Variable {
            presentation_ticks, ..
        },
        ..
    } = &profile.format
    else {
        return Err("actual picture index missing".into());
    };
    let budget = PictureBudget::default();
    let bytes = *width as usize * *height as usize * 4;
    let target_count = (budget.cache_bytes / bytes)
        .min(25)
        .min(presentation_ticks.len());
    if target_count < 2 {
        return Err("cache qualification needs two budgeted pictures".into());
    }
    let targets: Vec<_> = (0..target_count)
        .map(|index| index * (presentation_ticks.len() - 1) / (target_count - 1))
        .collect();
    let requests: Vec<_> = targets
        .iter()
        .map(|index| SourceRequest::Media {
            source: source_id,
            stream: selected,
            asset: asset.id,
            asset_sha256: asset.sha256.clone(),
            stream_sha256: interpretation.into(),
            position: SourcePosition {
                numerator: presentation_ticks[*index],
                denominator: 1,
            },
            reverse: false,
            picture: Some(*index as u64),
            sample: None,
        })
        .collect();
    let mut reference = VideoReader::open_stream(&source, selected, cancel.clone())?;
    let mut expected = HashMap::new();
    let mut ordinal = 0;
    while let Some(frame) = reference.next_frame()? {
        if frame.source_tick != presentation_ticks.get(ordinal).copied() {
            return Err("independent decode differs from source index".into());
        }
        if targets.contains(&ordinal) {
            expected.insert(ordinal, format!("{:x}", Sha256::digest(&frame.rgba)));
        }
        ordinal += 1;
    }
    if ordinal != presentation_ticks.len() {
        return Err("independent picture count differs from index".into());
    }
    drop(reference);
    let mut cache = PictureCache::new(snapshot.clone(), budget, cancel.clone())?;
    let mut miss_times = Vec::new();
    let mut receipts = Vec::new();
    let mut pointers = HashMap::new();
    let mut slowest = (0., 0);
    for index in (0..targets.len()).rev() {
        let started = Instant::now();
        let actual = cache
            .picture(&requests[index])?
            .ok_or("indexed picture is absent")?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.;
        miss_times.push(elapsed);
        if actual.cache_hit {
            return Err("empty target cache unexpectedly hit".into());
        }
        if format!("{:x}", Sha256::digest(actual.picture.rgba())) != expected[&targets[index]] {
            return Err("cached miss differs from independent pixels".into());
        }
        pointers.insert(index, Arc::downgrade(&actual.picture));
        cache.validate_result(&actual)?;
        receipts.push(
            json!({"ordinal":targets[index],"source_tick":actual.picture.source_tick,
            "rgba_sha256":expected[&targets[index]],"miss_ms":elapsed}),
        );
        if targets[index] > 1 && elapsed > slowest.0 {
            slowest = (elapsed, targets[index]);
        }
    }
    let populated = cache.stats();
    let mut hit_times = Vec::new();
    for index in 0..1000 {
        let index = index % requests.len();
        let started = Instant::now();
        let actual = cache
            .picture(&requests[index])?
            .ok_or("cached picture is absent")?;
        hit_times.push(started.elapsed().as_secs_f64() * 1000.);
        let original = pointers[&index]
            .upgrade()
            .ok_or("target output was unexpectedly evicted")?;
        if !actual.cache_hit || !Arc::ptr_eq(&original, &actual.picture) {
            return Err("warm hit does not share the verified immutable pixels".into());
        }
        cache.validate_result(&actual)?;
    }
    cache.verify_sources()?;
    let warmed = cache.stats();
    cache.clear()?;
    if cache.stats().live_bytes != 0 {
        return Err("cleared outputs retained an unexpected allocation".into());
    }
    let mut sequential_times = Vec::new();
    for (index, tick) in presentation_ticks.iter().enumerate().take(120) {
        let request = SourceRequest::Media {
            source: source_id,
            stream: selected,
            asset: asset.id,
            asset_sha256: asset.sha256.clone(),
            stream_sha256: interpretation.into(),
            position: SourcePosition {
                numerator: *tick,
                denominator: 1,
            },
            reverse: false,
            picture: Some(index as u64),
            sample: None,
        };
        let started = Instant::now();
        let actual = cache
            .picture(&request)?
            .ok_or("sequential picture missing")?;
        sequential_times.push(started.elapsed().as_secs_f64() * 1000.);
        if actual.picture.source_tick != *tick {
            return Err("sequential cached decoder differs from index".into());
        }
    }
    cache.verify_sources()?;
    let sequenced = cache.stats();
    cache.clear()?;
    let cleanup = cache.stats();
    let cancellation = active_cancel(
        snapshot.clone(),
        requests[targets
            .iter()
            .position(|value| *value == slowest.1)
            .ok_or("no seek target")?]
        .clone(),
        requests[0].clone(),
    )?;
    source.verify(&cancel)?;
    let hits = metrics(&mut hit_times);
    let misses = metrics(&mut miss_times);
    Ok(
        json!({"schema":1,"kind":"source_owned_decoded_picture_cache",
        "application_version":env!("CARGO_PKG_VERSION"),"architecture":std::env::consts::ARCH,
        "source":path,"fingerprint":source.fingerprint(),"codec_runtime":probe.codec_runtime,
        "selected_stream":selected,"stream_sha256":interpretation,"version":DocumentVersion::of(snapshot.project()),
        "geometry":[width,height],"actual_indexed_pictures":ordinal,"budget":budget,
        "independent_pixel_checks":receipts,"empty_output_cache_requests":misses,
        "immutable_shared_hits":hits,"hit_p95_budget_ms":250.,
        "hit_budget_pass":hits["p95_ms"].as_f64().is_some_and(|value|value<=250.),
        "populated":populated,"warmed":warmed,"sequential":metrics(&mut sequential_times),
        "sequenced":sequenced,"cleanup":cleanup,"active_cancel":cancellation,
        "source_unchanged":true,"memory":memory()?,"full_R2_seek_gate_qualified":false,
        "limits":["CPU RGBA8 payload cache only; native decoder buffers and allocator overhead are not its output-byte accounting",
            "empty output cache measurements retain OS page-cache warmth and are not physical cold-disk proof",
            "hit measurements share the independently verified Arc; no pixel copies, GPU upload or surface presentation",
            "source binding/checksum/open costs appear on the first miss; final full verification occurs before job publication",
            "sequential requests are capped at 120 actual frames and do not prove long native playback",
            "source handles stay retained up to the declared per-job cap; decoder and picture entries use LRU eviction",
            "native UI IPC, graph output, working/HDR color, hardware texture handoff, full seek and audio drift remain open"]}),
    )
}

/// Cancel a real uncached native seek after source binding and initial decode.
/// `snapshot`, `target` and `first` retain exact ownership and indexed requests.
/// Returns only after the decoder worker is joined, with issued-seek/cancel timing.
fn active_cancel(
    snapshot: Arc<EvaluationSnapshot>,
    target: SourceRequest,
    first: SourceRequest,
) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let worker_cancel = cancel.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || -> editbay_media::Result<_> {
        let mut cache = PictureCache::new(snapshot, PictureBudget::default(), worker_cancel)?;
        drop(
            cache
                .picture(&first)?
                .ok_or_else(|| Error::Invalid("first cancellation picture absent".into()))?,
        );
        ready_tx
            .send(())
            .map_err(|_| Error::Invalid("cancellation observer closed".into()))?;
        let started = Instant::now();
        let result = cache.picture(&target);
        Ok((
            matches!(result, Err(Error::Cancelled)),
            cache.stats(),
            started.elapsed().as_secs_f64() * 1000.,
        ))
    });
    let ready = ready_rx.recv_timeout(Duration::from_secs(10));
    if ready.is_ok() {
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    cancel.cancel();
    let result = worker
        .join()
        .map_err(|_| "native cache cancellation worker panicked")??;
    let after_cancel_ms = started.elapsed().as_secs_f64() * 1000.;
    ready?;
    if !result.0 || result.1.seeks == 0 || after_cancel_ms > 2000. {
        return Err("native cache seek was not actively cancelled within 2 seconds".into());
    }
    Ok(
        json!({"cancelled":true,"native_seek_issued":true,"delay_ms":10,
        "after_cancel_ms":after_cancel_ms,"work_wall_ms":result.2,"stats":result.1,
        "worker_joined":true,"budget_ms":2000.,"pass":true}),
    )
}
