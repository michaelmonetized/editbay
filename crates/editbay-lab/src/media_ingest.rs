use crate::{Result, memory, metrics};
use editbay_core::{DocumentEditor, DocumentVersion, PictureTiming, Project, StreamFormat};
use editbay_media::{Cancellation, NativeAudioReader, SourceFile, StreamType, VideoReader};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Instant};

/// Qualify actual source ingest and indexed random seeks.
/// `path` is read-only real media. Returns source/runtime hashes, exact decoded
/// extents, pixel-equal seek checks and measured timings; no playback claim is made.
pub fn run(path: &Path) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let started = Instant::now();
    let source = SourceFile::open(path, &cancel)?;
    let checksum_ms = started.elapsed().as_secs_f64() * 1000.;
    let started = Instant::now();
    let probe = source.probe(cancel.clone())?;
    let probe_ms = started.elapsed().as_secs_f64() * 1000.;
    let selected: Vec<_> = probe
        .streams
        .iter()
        .filter(|stream| {
            stream.decoder_available && matches!(stream.kind, StreamType::Video | StreamType::Audio)
        })
        .map(|stream| stream.index)
        .collect();
    let started = Instant::now();
    let imported = source.ingest(
        path.file_name()
            .ok_or("Source needs a filename")?
            .to_string_lossy()
            .into_owned(),
        &selected,
        cancel.clone(),
        |_, _| {},
    )?;
    let ingest_ms = started.elapsed().as_secs_f64() * 1000.;
    let mut editor = DocumentEditor::new(Project::new("Real media qualification")?)?;
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Import media".into(),
        &imported.commands(),
    )?;
    let mut results = Vec::new();
    for stream in &imported.source.streams {
        match &stream.format {
            StreamFormat::Video {
                timing:
                    PictureTiming::Variable {
                        presentation_ticks,
                        end_tick,
                    },
                ..
            } => {
                let mut reader = VideoReader::open_stream(&source, stream.index, cancel.clone())?;
                let mut targets = Vec::new();
                let mut times = Vec::new();
                let mut ordinal = 0;
                let mut checks = 0;
                while let Some(frame) = reader.next_frame()? {
                    if frame.source_tick != presentation_ticks.get(ordinal).copied() {
                        return Err("Indexed timestamps differ from sequential decode".into());
                    }
                    if ordinal == 0
                        || ordinal + 1 == presentation_ticks.len()
                        || ordinal.is_multiple_of((presentation_ticks.len() / 24).max(1))
                    {
                        targets.push((
                            frame.source_tick.unwrap(),
                            format!("{:x}", Sha256::digest(&frame.rgba)),
                        ));
                    }
                    ordinal += 1;
                }
                if ordinal != presentation_ticks.len() {
                    return Err("Sequential picture count differs from source index".into());
                }
                let mut receipts = Vec::new();
                for (tick, expected) in targets.into_iter().rev() {
                    let started = Instant::now();
                    let actual = reader.frame_at(tick)?;
                    times.push(started.elapsed().as_secs_f64() * 1000.);
                    if format!("{:x}", Sha256::digest(&actual.rgba)) != expected {
                        return Err("Sought picture differs from sequential decode".into());
                    }
                    checks += 1;
                    receipts.push(json!({"source_tick":tick,"rgba_sha256":expected}));
                }
                results.push(json!({"index":stream.index,"kind":"video","pictures":ordinal,
                    "start_tick":stream.start_tick,"end_tick":end_tick,"time_base":stream.time_base,
                    "pixel_equal_seek_checks":checks,"warm_cpu_seek":metrics(&mut times),"pictures_checked":receipts}));
            }
            StreamFormat::Audio {
                sample_rate,
                channels,
            } => {
                let mut reader =
                    NativeAudioReader::open_stream(&source, stream.index, cancel.clone())?;
                let mut samples = 0u64;
                let mut end = None;
                let mut digest = Sha256::new();
                while let Some(block) = reader.next_block()? {
                    let first = block
                        .first_sample
                        .ok_or("Sound block lacks a sample timestamp")?;
                    if end.is_some_and(|end| end != first) {
                        return Err("Decoded sound contains a discontinuity".into());
                    }
                    let count = block.samples.len() / channels.len();
                    end = Some(first + count as i64);
                    samples += count as u64;
                    for value in &block.samples {
                        digest.update(value.to_le_bytes());
                    }
                }
                if Some(samples) != stream.duration_ticks {
                    return Err("Source sound duration differs from decoded native samples".into());
                }
                results.push(
                    json!({"index":stream.index,"kind":"audio","sample_rate":sample_rate,
                    "channels":channels,"sample_frames":samples,"start_sample":stream.start_tick,
                    "end_sample":end,"decoded_f32le_sha256":format!("{:x}",digest.finalize())}),
                );
            }
            _ => return Err("Ingested video has no presentation index".into()),
        }
    }
    source.verify(&cancel)?;
    let serialized = serde_json::to_vec(editor.project())?;
    Ok(
        json!({"kind":"real_native_media_ingest","source":path,"fingerprint":source.fingerprint(),
        "codec_runtime":probe.codec_runtime,"checksum_ms":checksum_ms,"probe_ms":probe_ms,
        "ingest_ms":ingest_ms,"streams":results,"document_bytes":serialized.len(),
        "source_unchanged":true,"memory":memory()?,"production_playback_qualified":false}),
    )
}
