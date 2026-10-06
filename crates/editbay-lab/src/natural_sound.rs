use crate::{Result, metrics, sound_blocks::independent_pcm};
use editbay_audio::{SoundRenderBudget, SoundRenderer};
use editbay_core::*;
use editbay_media::{
    Cancellation, SourceFile, StreamType,
    pcm_worker::{PcmWorker, PcmWorkerBudget},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc, time::Instant};

/// Qualify a complete naturally timed source sequence against independent PCM.
/// `path` is read-only media and `executable` hosts the packaged PCM worker.
/// Returns every-sample comparison, exact tail counts and worker measurements.
pub fn run(path: &Path, executable: &Path) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let owned = SourceFile::open(path, &cancel)?;
    let probe = owned.probe(cancel.clone())?;
    let select = |kind| {
        probe
            .streams
            .iter()
            .find(|s| s.kind == kind && s.decoder_available)
            .map(|s| s.index)
            .ok_or("Qualification requires video and sound")
    };
    let video = select(StreamType::Video)?;
    let audio = select(StreamType::Audio)?;
    let imported = owned.ingest(
        "Natural source sequence".into(),
        &[video, audio],
        cancel.clone(),
        |_, _| {},
    )?;
    let mut project = Project::new("Natural source sequence")?;
    project.assets.push(imported.asset);
    project.sources.push(imported.source);
    let commands = sequence_from_video_with_audio(
        &project,
        project.sources[0].id,
        video,
        audio,
        FrameRate::new(30, 1)?,
    )?;
    let mut editor = DocumentEditor::new(project)?;
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Create source sequence".into(),
        &commands,
    )?;
    let result = verify(editor.project(), path, video, audio, executable)?;
    owned.verify(&cancel)?;
    Ok(result)
}

/// Compare an authored native project's complete sound to a separate decoder.
/// `project`, `path`, stream indexes and `executable` select retained content.
/// Returns a receipt or fails on any altered sample, missing tail or layout.
/// This exact-copy qualification requires sample-aligned source origins.
pub fn verify(
    project: &Project,
    path: &Path,
    video: u32,
    audio: u32,
    executable: &Path,
) -> Result<Value> {
    let source = project.sources.first().ok_or("Missing source")?;
    let video = source
        .streams
        .iter()
        .find(|s| s.index == video)
        .ok_or("Missing video")?;
    let audio = source
        .streams
        .iter()
        .find(|s| s.index == audio)
        .ok_or("Missing sound")?;
    let StreamFormat::Audio {
        sample_rate,
        channels,
    } = &audio.format
    else {
        return Err("Not sound".into());
    };
    let rate = FrameRate::new(*sample_rate, 1)?;
    let video_origin = video
        .time_base
        .at_rate(SourcePosition::new(video.start_tick, 1)?, rate)?;
    let audio_origin = audio
        .time_base
        .at_rate(SourcePosition::new(audio.start_tick, 1)?, rate)?;
    let sound_length = audio.time_base.at_rate(
        SourcePosition::new(
            i64::try_from(audio.duration_ticks.ok_or("Missing sound length")?)?,
            1,
        )?,
        rate,
    )?;
    if [video_origin, audio_origin, sound_length]
        .iter()
        .any(|p| p.denominator != 1)
    {
        return Err(format!("Exact-copy qualification requires sample-aligned source origins and end: video={video_origin:?}, audio={audio_origin:?}, length={sound_length:?}").into());
    }
    let offset = video_origin
        .numerator
        .checked_sub(audio_origin.numerator)
        .ok_or("Origin overflow")?;
    let composition = project
        .sequences
        .iter()
        .find_map(|s| s.composition)
        .ok_or("Missing sequence")?;
    let root = project
        .compositions
        .iter()
        .find(|c| c.id == composition)
        .ok_or("Missing root")?;
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(project.clone()))?);
    let sound = Arc::new(SoundSnapshot::new(
        snapshot.clone(),
        composition,
        SoundProfile {
            sample_rate: *sample_rate,
            channels: channels.clone(),
        },
        SoundBudget::default(),
    )?);
    let worker = PcmWorker::new(
        executable,
        snapshot,
        PcmWorkerBudget::default(),
        Cancellation::new()?,
    )?;
    let mut renderer =
        SoundRenderer::with_provider(sound.clone(), worker, SoundRenderBudget::default())?;
    let (reference, reference_hash) = independent_pcm(path, audio.index)?;
    if reference.len() / channels.len() < sound_length.numerator as usize {
        return Err("Independent decoder lost declared samples".into());
    }
    let mut prep = vec![];
    let mut render = vec![];
    let mut maximum_error = 0f32;
    let mut active_frames = 0u64;
    let mut silence_frames = 0u64;
    let mut digest = Sha256::new();
    let mut first = 0;
    while first < sound.duration_samples() {
        let count = (sound.duration_samples() - first).min(4096) as u32;
        let began = Instant::now();
        let plan = sound.prepare(first, count)?;
        prep.push(began.elapsed().as_secs_f64() * 1000.);
        let began = Instant::now();
        let result = renderer.render(&plan)?;
        render.push(began.elapsed().as_secs_f64() * 1000.);
        renderer.validate_result(&result)?;
        for (frame, values) in result
            .sound()
            .samples()
            .chunks_exact(channels.len())
            .enumerate()
        {
            let sample = i128::from(first) + frame as i128 + i128::from(offset);
            let active = (0..i128::from(sound_length.numerator)).contains(&sample);
            if active {
                active_frames += 1;
            } else {
                silence_frames += 1;
            }
            for (channel, value) in values.iter().enumerate() {
                let expected = if active {
                    reference[sample as usize * channels.len() + channel]
                } else {
                    0.
                };
                if !value.is_finite() {
                    return Err("Nonfinite rendered sample".into());
                }
                maximum_error = maximum_error.max((value - expected).abs());
                digest.update(value.to_le_bytes());
            }
        }
        first += u64::from(count);
    }
    renderer.verify_sources()?;
    let stats = renderer.pcm_stats();
    renderer.clear();
    let receipt = json!({"kind":"natural_source_sound", "source":path.canonicalize()?, "worker_sha256":crate::hash(executable)?,
        "source_sha256":project.assets.iter().find(|a| a.id == source.asset).ok_or("Missing asset")?.sha256,
        "video_stream":video.index,"audio_stream":audio.index,"sample_rate":sample_rate,"channels":channels,
        "sequence_rate":root.frame_rate,"sequence_frames":root.duration,"sequence_samples":sound.duration_samples(),
        "source_sample_offset":offset,"declared_source_samples":sound_length.numerator,"independent_decoded_samples":reference.len()/channels.len(),
        "active_frames":active_frames,"silence_frames":silence_frames,"maximum_absolute_pcm_error":maximum_error,
        "independent_pcm_sha256":reference_hash,"rendered_sha256":format!("{:x}",digest.finalize()),
        "preparation":metrics(&mut prep),"render":metrics(&mut render),"pcm_stats":stats,"qualified":maximum_error <= 1e-6,
        "limits":["Every output sample from the saved graph; no device, audibility, streaming or drift claim", "Exact-copy receipt uses sample-aligned origins; rational nonaligned timing is covered by core tests"]});
    if maximum_error > 1e-6 {
        return Err(format!("Natural source PCM error {maximum_error}: {receipt}").into());
    }
    Ok(receipt)
}
