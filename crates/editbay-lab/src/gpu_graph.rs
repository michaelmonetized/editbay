use crate::{Result, memory, metrics};
use editbay_core::*;
use editbay_media::{
    Cancellation, PictureBudget, PictureCache, PictureProvider, SourceFile, StreamType,
    VideoReader,
    picture_worker::{PictureWorker, WorkerBudget},
};
use editbay_render::{GraphBudget, GraphRenderer, ImageBoundary};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use uuid::Uuid;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn node(n: u128, operation: NodeOperation, count: u64) -> TimedNode {
    TimedNode {
        id: id(n),
        operation,
        animation: vec![],
        range: FrameRange {
            start: 0,
            end: count,
        },
    }
}
fn linear(v: u8, transfer: i32) -> Result<f32> {
    let v = f32::from(v) / 255.;
    Ok(match transfer {
        13 => {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        1 => {
            if v < 0.081 {
                v / 4.5
            } else {
                ((v + 0.099) / 1.099).powf(1. / 0.45)
            }
        }
        8 => v,
        _ => return Err("qualification needs known SDR input transfer".into()),
    })
}

/// Qualify shared typed GPU evaluation on actual read-only source pictures.
/// `path` selects media. Returns FP16/FP32 pixel errors, completed GPU timings,
/// shared-hit ownership and cache cleanup; never claims native playback or delivery.
pub fn run(path: &Path) -> Result<Value> {
    run_route(path, None)
}

/// Qualify identical temporal/color GPU evaluation over a packaged codec process.
/// `path` selects real media and `executable` supplies --picture-worker.
/// Returns scoped headless graph evidence, excluding presentation/audio/delivery.
pub fn run_process(path: &Path, executable: &Path) -> Result<Value> {
    run_route(path, Some(executable))
}

fn run_route(path: &Path, executable: Option<&Path>) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let owned = SourceFile::open(path, &cancel)?;
    let probe = owned.probe(cancel.clone())?;
    let stream = probe
        .streams
        .iter()
        .find(|s| s.decoder_available && s.kind == StreamType::Video)
        .ok_or("source has no decodable picture stream")?;
    let imported = owned.ingest(
        "Real typed GPU qualification".into(),
        &[stream.index],
        cancel.clone(),
        |_, _| {},
    )?;
    let profile = &imported.source.streams[0];
    let StreamFormat::Video {
        width,
        height,
        sample_aspect,
        timing:
            PictureTiming::Variable {
                presentation_ticks,
                end_tick,
            },
        color,
        alpha,
    } = &profile.format
    else {
        return Err("actual picture index missing".into());
    };
    if color.primaries != 1
        || *alpha != AlphaMode::Opaque
        || sample_aspect.numerator != sample_aspect.denominator
    {
        return Err("qualification requires declared opaque square-pixel BT.709 primaries".into());
    }
    let count = presentation_ticks.len().min(120);
    if count < 2 {
        return Err("qualification needs two real pictures".into());
    }
    let count_u64 = count as u64;
    let targets = [0, count / 2, count - 1];
    let mut reference = VideoReader::open_stream(&owned, stream.index, cancel.clone())?;
    let mut expected = HashMap::new();
    for (ordinal, tick) in presentation_ticks.iter().enumerate().take(count) {
        let frame = reference
            .next_frame()?
            .ok_or("independent sequential picture missing")?;
        if frame.source_tick != Some(*tick) {
            return Err("independent timestamp differs from captured index".into());
        }
        if targets.contains(&ordinal) {
            let pixels: Vec<f32> = frame
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    [
                        linear(p[0], color.transfer),
                        linear(p[1], color.transfer),
                        linear(p[2], color.transfer),
                    ]
                    .into_iter()
                    .map(|v| v.map(|v| v * 0.8 + 0.004))
                    .chain([Ok(1.)])
                })
                .collect::<Result<_>>()?;
            expected.insert(
                ordinal,
                (format!("{:x}", Sha256::digest(&frame.rgba)), pixels),
            );
        }
    }
    drop(reference);
    let mut base = Project::new("Source plus typed opacity and background")?;
    base.assets = vec![imported.asset.clone()];
    base.sources = vec![imported.source.clone()];
    let mut points: Vec<_> = presentation_ticks
        .iter()
        .take(count)
        .enumerate()
        .map(|(frame, tick)| TimePoint {
            frame: frame as u64,
            source_tick: *tick,
        })
        .collect();
    points.push(TimePoint {
        frame: count_u64,
        source_tick: presentation_ticks.get(count).copied().unwrap_or(*end_tick),
    });
    base.compositions = vec![Composition {
        id: id(100),
        name: "Ordinal-mapped source qualification".into(),
        width: *width,
        height: *height,
        frame_rate: stream.nominal_rate.unwrap_or(FrameRate::new(30, 1)?),
        duration: count_u64,
        tracks: vec![Track {
            id: id(20),
            name: "Original selected stream".into(),
            kind: TrackKind::Video,
            enabled: true,
            clips: vec![Clip {
                id: id(21),
                name: "Exact indexed original".into(),
                range: FrameRange {
                    start: 0,
                    end: count_u64,
                },
                source: ClipSource::Media {
                    source: imported.source.id,
                    stream: stream.index,
                },
                linked: None,
                time_map: TimeMap {
                    source_denominator: 1,
                    points,
                },
            }],
        }],
        nodes: vec![
            node(1, NodeOperation::Source { clip: id(21) }, count_u64),
            node(
                2,
                NodeOperation::Solid {
                    rgba: [0.02, 0.02, 0.02, 1.],
                },
                count_u64,
            ),
            node(3, NodeOperation::Scalar { value: 0.8 }, count_u64),
            node(
                4,
                NodeOperation::Opacity {
                    image: id(1),
                    value: id(3),
                },
                count_u64,
            ),
            node(
                5,
                NodeOperation::Over {
                    foreground: id(4),
                    background: id(2),
                    mask: None,
                },
                count_u64,
            ),
        ],
        picture: Some(id(5)),
        audio: None,
    }];
    let budget = GraphBudget::default();
    let picture_budget = PictureBudget::default();
    let mut qualifications = Vec::new();
    for (precision, tolerance) in [
        (FloatPrecision::Half, 0.002),
        (FloatPrecision::Full, 0.00002),
    ] {
        let mut project = base.clone();
        project.color.precision = precision;
        let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(project))?);
        let token = Cancellation::new()?;
        let provider: Box<dyn PictureProvider> = if let Some(executable) = executable {
            Box::new(PictureWorker::new(
                executable,
                snapshot.clone(),
                WorkerBudget::default(),
                token.clone(),
            )?)
        } else {
            Box::new(PictureCache::new(
                snapshot.clone(),
                picture_budget,
                token.clone(),
            )?)
        };
        let mut worker = GraphRenderer::with_provider(snapshot, provider, budget, token)?;
        let mut checks = Vec::new();
        for ordinal in targets {
            let frame = worker.render(id(100), SourcePosition::new(ordinal as i64, 1)?, false)?;
            let actual = worker.readback(&frame)?;
            let expected = &expected[&ordinal];
            if actual.len() != expected.1.len() {
                return Err("GPU geometry differs from independent reference".into());
            }
            let error = actual
                .iter()
                .zip(&expected.1)
                .map(|(a, b)| (a - b).abs())
                .fold(0_f32, f32::max);
            if !error.is_finite() || error > tolerance {
                return Err(format!("GPU pixel error {error} exceeds {tolerance}").into());
            }
            checks.push(json!({"ordinal":ordinal,"source_tick":presentation_ticks[ordinal],"source_rgba_sha256":expected.0,"maximum_linear_error":error,"tolerance":tolerance}));
        }
        worker.verify_sources()?;
        worker.clear()?;
        let warm = worker.render(id(100), SourcePosition::new(0, 1)?, false)?;
        worker.finish()?;
        let before_hits = worker.stats();
        let mut hits = Vec::new();
        for _ in 0..1000 {
            let started = Instant::now();
            let frame = worker.render(id(100), SourcePosition::new(0, 1)?, false)?;
            worker.finish()?;
            hits.push(started.elapsed().as_secs_f64() * 1000.);
            if !Arc::ptr_eq(warm.image(), frame.image()) {
                return Err("GPU cache hit does not share verified resident content".into());
            }
            worker.validate_result(&frame)?;
        }
        let after_hits = worker.stats();
        if before_hits.dispatches != after_hits.dispatches
            || before_hits.uploads != after_hits.uploads
            || before_hits.readbacks != after_hits.readbacks
        {
            return Err("GPU hit unexpectedly evaluated/uploaded/read back".into());
        }
        drop(warm);
        worker.verify_sources()?;
        worker.clear()?;
        let mut first_ms = None;
        let mut times = Vec::new();
        let readbacks_before = worker.stats().readbacks;
        for ordinal in 0..count {
            let started = Instant::now();
            let frame = worker.render(id(100), SourcePosition::new(ordinal as i64, 1)?, false)?;
            worker.finish()?;
            let elapsed = started.elapsed().as_secs_f64() * 1000.;
            if ordinal == 0 {
                first_ms = Some(elapsed);
            } else {
                times.push(elapsed);
            }
            worker.validate_result(&frame)?;
        }
        let sequenced = worker.stats();
        if sequenced.readbacks != readbacks_before {
            return Err("resident sequence performed CPU readback".into());
        }
        worker.verify_sources()?;
        let working = worker.render(id(100), SourcePosition::new((count - 1) as i64, 1)?, false)?;
        let display = worker.convert(&working, ImageBoundary::Display)?;
        let output = worker.convert(&working, ImageBoundary::Output)?;
        if !Arc::ptr_eq(display.image(), output.image()) {
            return Err("equal display/output settings do not share their converted result".into());
        }
        let encoded = worker.readback(&output)?;
        let pixels = &expected[&(count - 1)].1;
        let encoded_error = encoded
            .iter()
            .zip(pixels)
            .enumerate()
            .map(|(index, (a, b))| {
                if index % 4 == 3 {
                    (a - b).abs()
                } else {
                    (a - editbay_render::to_srgb(*b)).abs()
                }
            })
            .fold(0_f32, f32::max);
        if encoded_error
            > if precision == FloatPrecision::Half {
                0.004
            } else {
                0.00004
            }
        {
            return Err(
                "explicit encoded output differs from independent transfer reference".into(),
            );
        }
        drop(working);
        drop(display);
        drop(output);
        worker.clear()?;
        let cleanup = worker.stats();
        if cleanup.live_texture_bytes != 0
            || cleanup.pending_submissions != 0
            || cleanup.pictures.live_bytes != 0
            || cleanup.pictures.sources != 0
            || cleanup.pictures.decoders != 0
        {
            return Err("GPU/raw resource cleanup retained owned payloads or handles".into());
        }
        let timings = metrics(&mut times);
        qualifications.push(json!({"precision":precision,"adapter":{"name":worker.adapter.name,"backend":format!("{:?}",worker.adapter.backend),"device_type":format!("{:?}",worker.adapter.device_type),"driver":worker.adapter.driver,"driver_info":worker.adapter.driver_info},"independent_pixels":checks,"immutable_hits":metrics(&mut hits),"before_hits":before_hits,"after_hits":after_hits,"first_sequential_picture_ms":first_ms,"steady_completed_gpu_pictures":timings,"per_picture_budget_ms":33.3,"steady_budget_pass":timings["p95_ms"].as_f64().is_some_and(|v|v<=33.3),"sequenced":sequenced,"encoded_output_maximum_error":encoded_error,"cleanup":cleanup}));
    }
    let cancellation = if executable.is_none() {
        active_cancel(
            Arc::new(EvaluationSnapshot::new(Arc::new(base.clone()))?),
            count - 1,
        )?
    } else {
        json!({"route":"isolated cancellation is qualified by picture-worker, separately from GPU completion"})
    };
    owned.verify(&cancel)?;
    Ok(
        json!({"schema":1,"kind":"typed_resident_sdr_gpu_picture_graph","architecture":std::env::consts::ARCH,"source":path,"fingerprint":owned.fingerprint(),"codec_runtime":probe.codec_runtime,"selected_stream":stream.index,"geometry":[width,height],"source_color":color,"composition_rate":base.compositions[0].frame_rate,"mapping":"one composition frame per actual source picture ordinal; not natural VFR wall-clock playback","measured_pictures_per_precision":count,"gpu_budget":budget,"picture_budget":picture_budget,"qualifications":qualifications,"active_cancel":cancellation,"memory":memory()?,"includes_native_presentation":false,"includes_codec_process_ipc":executable.is_some(),"includes_audio_or_long_playback":false,"full_r2_gate_pass":false}),
    )
}

fn active_cancel(snapshot: Arc<EvaluationSnapshot>, ordinal: usize) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let worker_cancel = cancel.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || -> std::result::Result<_, String> {
        let run = || -> Result<_> {
            let mut worker = GraphRenderer::new(
                snapshot,
                PictureBudget::default(),
                GraphBudget::default(),
                worker_cancel,
            )?;
            drop(worker.render(id(100), SourcePosition::new(0, 1)?, false)?);
            worker.finish()?;
            ready_tx.send(())?;
            let started = Instant::now();
            let result = worker.render(id(100), SourcePosition::new(ordinal as i64, 1)?, false);
            let cancelled = result.as_ref().err().is_some_and(|error| {
                error
                    .downcast_ref::<editbay_media::Error>()
                    .is_some_and(|error| matches!(error, editbay_media::Error::Cancelled))
            });
            let elapsed = started.elapsed().as_secs_f64() * 1000.;
            drop(result);
            let stats = worker.stats();
            worker.clear()?;
            Ok((cancelled, stats, worker.stats(), elapsed))
        };
        run().map_err(|error| error.to_string())
    });
    let ready = ready_rx.recv_timeout(Duration::from_secs(10));
    if ready.is_ok() {
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    cancel.cancel();
    let result = worker
        .join()
        .map_err(|_| "GPU graph cancellation worker panicked")?
        .map_err(|error| format!("GPU graph cancellation worker: {error}"))?;
    let after_cancel_ms = started.elapsed().as_secs_f64() * 1000.;
    ready?;
    if !result.0
        || result.1.pictures.seeks == 0
        || after_cancel_ms > 2000.
        || result.2.live_texture_bytes != 0
        || result.2.pictures.live_bytes != 0
        || result.2.pending_submissions != 0
    {
        return Err("active graph native seek cancellation/cleanup failed its budget".into());
    }
    Ok(
        json!({"cancelled":true,"native_seek_issued":true,"phase":"native source cache miss inside the shared graph; GPU work is not preempted","delay_ms":10,"after_cancel_ms":after_cancel_ms,"work_wall_ms":result.3,"stats":result.1,"cleanup":result.2,"worker_joined":true,"budget_ms":2000.,"pass":true}),
    )
}
