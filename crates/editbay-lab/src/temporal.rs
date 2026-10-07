use crate::{Result, memory, metrics};
use editbay_core::{
    Clip, ClipSource, EvaluationSnapshot, FrameRange, NodeOperation, PictureTiming, Project,
    SourcePosition, StreamFormat, TimeBase, TimeMap, TimePoint, TimedNode, Track, TrackKind,
};
use editbay_media::{Cancellation, IngestedSource, SourceFile, StreamType};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{hint::black_box, path::Path, sync::Arc, time::Instant};
use uuid::Uuid;

/// Measure retained temporal planning against one-shot document inspection.
/// `path` supplies actual media, `iterations` bounds repeated evaluation, and
/// `stress` optionally adds explicitly synthetic VFR index entries.
/// Returns actual source ownership, initialization/planning timings and limits.
pub fn run(path: &Path, iterations: usize, stress: Option<usize>) -> Result<Value> {
    if !(100..=100_000).contains(&iterations)
        || stress.is_some_and(|count| !(100..=500_000).contains(&count))
    {
        return Err(
            "evaluation needs 100..100000 iterations and 100..500000 stress pictures".into(),
        );
    }
    let cancel = Cancellation::new()?;
    let source = SourceFile::open(path, &cancel)?;
    let probe = source.probe(cancel.clone())?;
    let selected: Vec<_> = [StreamType::Video, StreamType::Audio]
        .into_iter()
        .map(|kind| {
            probe
                .streams
                .iter()
                .find(|stream| stream.decoder_available && stream.kind == kind)
                .map(|stream| stream.index)
                .ok_or("evaluation fixture needs picture and sound")
        })
        .collect::<std::result::Result<_, _>>()?;
    let started = Instant::now();
    let imported = source.ingest(
        "Temporal qualification source".into(),
        &selected,
        cancel.clone(),
        |_, _| {},
    )?;
    let ingest_ms = started.elapsed().as_secs_f64() * 1000.;
    let project = fixture(imported)?;
    let actual = measure(Arc::new(project.clone()), iterations)?;
    let synthetic = if let Some(count) = stress {
        let mut project = project;
        let stream = &mut project.sources[0].streams[0];
        let StreamFormat::Video { timing, .. } = &mut stream.format else {
            return Err("first indexed stream must be picture".into());
        };
        let mut tick = 0;
        let presentation_ticks = (0..count)
            .map(|index| {
                let current = tick;
                tick += if index % 2 == 0 { 33 } else { 34 };
                current
            })
            .collect();
        *timing = PictureTiming::Variable {
            presentation_ticks,
            end_tick: tick,
        };
        stream.time_base = TimeBase {
            numerator: 1,
            denominator: 1000,
        };
        stream.start_tick = 0;
        stream.duration_ticks = Some(tick as u64);
        project.compositions[0].tracks[0].clips[0].time_map = mapping(0, tick);
        Some(measure(Arc::new(project), iterations)?)
    } else {
        None
    };
    source.verify(&cancel)?;
    Ok(json!({
        "schema":1,"kind":"retained_temporal_evaluation",
        "application_version":env!("CARGO_PKG_VERSION"),
        "architecture":std::env::consts::ARCH,"source":path,
        "fingerprint":source.fingerprint(),"codec_runtime":probe.codec_runtime,
        "selected_streams":selected,"ingest_ms":ingest_ms,"source_unchanged":true,
        "actual_indexed_source":actual,"synthetic_index_stress":synthetic,
        "memory":memory()?,"production_playback_qualified":false,
        "limits":["CPU temporal planning only; no rendered pictures, decoded output cache, surface presentation or sound playback",
            "the ten-node masked/animated picture and gain graph is a synthetic edit referencing the actual imported source index",
            "optional stress timestamps are synthetic and do not claim actual source pictures or duration",
            "planning p95 budget is 1 ms; the complete R2 picture, seek, audio and hardware budgets remain unchanged"]
    }))
}

/// Build a repeatable graph referencing actual owned media interpretations.
/// `imported` supplies checked source bytes/streams. Returns the published motion
/// fixture with real media, native audio and ten reachable child nodes.
fn fixture(imported: IngestedSource) -> Result<Project> {
    let mut project: Project = serde_json::from_slice(include_bytes!(
        "../../../docs/evidence/r2-document/fixture.editbay"
    ))?;
    let mut tracks = Vec::new();
    for (index, stream) in imported.source.streams.iter().enumerate() {
        let end = i64::try_from(
            i128::from(stream.start_tick)
                + i128::from(
                    stream
                        .duration_ticks
                        .ok_or("indexed stream has no extent")?,
                ),
        )?;
        let clip = id(100 + index as u128 * 2);
        let kind = match stream.format {
            StreamFormat::Video { .. } => TrackKind::Video,
            StreamFormat::Audio { .. } => TrackKind::Audio,
        };
        tracks.push(Track {
            id: id(101 + index as u128 * 2),
            name: format!("Actual {kind:?} stream"),
            kind,
            enabled: true,
            clips: vec![Clip {
                id: clip,
                name: "Actual indexed extent".into(),
                range: range(),
                source: ClipSource::Media {
                    source: imported.source.id,
                    stream: stream.index,
                },
                time_map: mapping(stream.start_tick, end),
                linked: None,
            }],
        });
    }
    project.assets = vec![imported.asset];
    project.sources = vec![imported.source];
    let child = &mut project.compositions[0];
    child.tracks = tracks;
    child.nodes[1].operation = NodeOperation::Source { clip: id(100) };
    child.nodes.extend([
        node(104, NodeOperation::Source { clip: id(102) }),
        node(
            105,
            NodeOperation::Gain {
                audio: id(104),
                gain: 0.75,
            },
        ),
        node(106, NodeOperation::Scalar { value: 0.8 }),
        node(
            107,
            NodeOperation::Opacity {
                image: id(45),
                value: id(106),
            },
        ),
    ]);
    child.picture = Some(id(107));
    child.audio = Some(id(105));
    project.validate()?;
    Ok(project)
}

/// Measure one immutable document without including source IO in frame timing.
/// `project` owns validated input bytes; `iterations` bounds mixed fractional and
/// reverse-nested requests. Returns timings, inspection parity and output hashes.
fn measure(project: Arc<Project>, iterations: usize) -> Result<Value> {
    let bytes = serde_json::to_vec(&*project)?;
    let document_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let indexed_pictures = project.sources[0]
        .streams
        .iter()
        .find_map(|stream| match &stream.format {
            StreamFormat::Video {
                timing:
                    PictureTiming::Variable {
                        presentation_ticks, ..
                    },
                ..
            } => Some(presentation_ticks.len()),
            _ => None,
        })
        .ok_or("picture index missing")?;
    let started = Instant::now();
    let snapshot = EvaluationSnapshot::new(project.clone())?;
    let initialization_ms = started.elapsed().as_secs_f64() * 1000.;
    let mut times = Vec::with_capacity(iterations);
    let mut digest = Sha256::new();
    for index in 0..iterations + 100 {
        let position = SourcePosition::new(((index % 48) * 60 + 25) as i64, 60)?;
        let nested = index % 2 == 0;
        let started = Instant::now();
        let mut frame = snapshot.prepare(id(if nested { 31 } else { 30 }), position, false)?;
        if nested {
            let Some(editbay_core::SourceRequest::Composition {
                composition,
                position,
                reverse,
            }) = frame.nodes[0].source
            else {
                return Err("nested request missing".into());
            };
            frame = snapshot.prepare(composition, position, reverse)?;
        }
        black_box(&frame);
        let elapsed = started.elapsed().as_secs_f64() * 1000.;
        if index >= 100 {
            times.push(elapsed);
            digest.update(frame.sha256.as_bytes());
        }
    }
    let mut inspection = Vec::new();
    let mut legacy_digest = Sha256::new();
    for index in 0..20 {
        let started = Instant::now();
        let plan = project.frame_plan(id(30), index)?;
        inspection.push(started.elapsed().as_secs_f64() * 1000.);
        let retained = snapshot.frame_plan(id(30), index)?;
        if serde_json::to_vec(&plan)? != serde_json::to_vec(&retained)? {
            return Err("retained inspector differs from one-shot inspection".into());
        }
        legacy_digest.update(plan.sha256.as_bytes());
    }
    let planning = metrics(&mut times);
    let pass = planning["p95_ms"].as_f64().is_some_and(|value| value <= 1.);
    Ok(
        json!({"document_sha256":document_sha256,"document_bytes":bytes.len(),
        "indexed_pictures":indexed_pictures,"child_nodes":10,"parent_nodes":1,
        "initialization_ms":initialization_ms,"retained_planning":planning,
        "warmup_iterations":100,"planning_p95_budget_ms":1.,"planning_budget_pass":pass,
        "one_shot_inspection":metrics(&mut inspection),"inspection_parity_checks":20,
        "retained_frame_keys_sha256":format!("{:x}",digest.finalize()),
        "inspection_keys_sha256":format!("{:x}",legacy_digest.finalize())}),
    )
}

/// Construct stable fixture identities.
/// `value` is a unique fixture number. Returns its UUID.
fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

/// Select the fixture's two-second temporal range.
/// Takes no arguments. Returns the exclusive 48-frame range.
fn range() -> FrameRange {
    FrameRange { start: 0, end: 48 }
}

/// Construct one exact linear source map.
/// `start` and `end` select source boundaries. Returns a 48-frame mapping.
fn mapping(start: i64, end: i64) -> TimeMap {
    TimeMap {
        source_denominator: 1,
        points: vec![
            TimePoint {
                frame: 0,
                source_tick: start,
            },
            TimePoint {
                frame: 48,
                source_tick: end,
            },
        ],
    }
}

/// Construct one fixture operation without animation.
/// `value` supplies identity and `operation` supplies behavior. Returns a timed node.
fn node(value: u128, operation: NodeOperation) -> TimedNode {
    TimedNode {
        id: id(value),
        range: range(),
        operation,
        animation: Vec::new(),
    }
}
