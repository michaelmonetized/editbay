use crate::{Result, hash, sound_blocks::independent_pcm};
use editbay_audio::{SoundRenderBudget, SoundRenderer};
use editbay_core::{
    ClipSource, EvaluationSnapshot, SoundBudget, SoundSnapshot, StreamFormat, TimeBase, load,
};
use editbay_media::{Cancellation, PcmBudget, SourceFile};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command, sync::Arc};
use uuid::Uuid;

/// Check a rendered master against independently decoded, uncompressed source sound.
/// `project_path`, `composition`, `master` and fresh `directory` capture a real
/// output and retained evidence. Returns original-bit agreement through the same
/// typed graph and mixer, with compressed decoder history removed from the oracle.
pub fn run(
    project_path: &Path,
    composition: Uuid,
    master: &Path,
    directory: &Path,
) -> Result<Value> {
    let mut project = load(project_path)?;
    let project_hash = hash(project_path)?;
    let master_hash = hash(master)?;
    let cancel = Cancellation::new()?;
    let output = SourceFile::open(master, &cancel)?;
    let probe = output.probe(cancel.clone())?;
    let audio = probe
        .streams
        .iter()
        .find(|stream| stream.kind == editbay_media::StreamType::Audio)
        .ok_or("Master sound missing")?;
    let rate = audio.sample_rate.ok_or("Master sound rate missing")?;
    let (actual, actual_hash) = independent_pcm(master, audio.index)?;
    let scene = project
        .compositions
        .iter()
        .find(|scene| scene.id == composition)
        .ok_or("Composition missing")?;
    let duration = scene.duration;
    let fps = scene.frame_rate;
    let mut selected = Vec::new();
    for source in &project.sources {
        let asset = project
            .assets
            .iter()
            .find(|asset| asset.id == source.asset)
            .ok_or("Source asset missing")?;
        for stream in &source.streams {
            if let StreamFormat::Audio { sample_rate, .. } = stream.format {
                if stream.time_base
                    != (TimeBase {
                        numerator: 1,
                        denominator: sample_rate,
                    })
                {
                    return Err("Oracle requires normalized source sample grids".into());
                }
                selected.push((source.id, asset.path.clone(), stream.clone()));
            }
        }
    }
    if selected.is_empty() || selected.len() > 8 {
        return Err("Oracle requires one to eight captured sound streams".into());
    }
    fs::create_dir(directory)?;
    let mut sources = Vec::new();
    for (index, (source_id, path, profile)) in selected.iter().enumerate() {
        let original_hash = hash(path)?;
        let (reference, reference_hash) = independent_pcm(path, profile.index)?;
        let pcm_path = directory.join(format!("source-{index}.wav"));
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-n", "-i"])
            .arg(path)
            .args(["-map", &format!("0:{}", profile.index), "-c:a", "pcm_f32le"])
            .arg(&pcm_path)
            .output()?;
        if !decoded.status.success() {
            return Err(format!(
                "Independent source decode failed: {}",
                String::from_utf8_lossy(&decoded.stderr)
            )
            .into());
        }
        let (uncompressed, uncompressed_hash) = independent_pcm(&pcm_path, 0)?;
        if reference_hash != uncompressed_hash || reference.len() != uncompressed.len() {
            return Err("Uncompressed oracle changed source PCM bits".into());
        }
        let mut imported = SourceFile::open(&pcm_path, &cancel)?.ingest(
            "Independent decoded source".into(),
            &[0],
            cancel.clone(),
            |_, _| {},
        )?;
        let replacement = &mut imported.source.streams[0];
        if replacement.format != profile.format
            || replacement.time_base != profile.time_base
            || replacement.start_tick != 0
            || replacement.duration_ticks < profile.duration_ticks
        {
            return Err("Independent PCM interpretation differs from captured sound".into());
        }
        replacement.duration_ticks = profile.duration_ticks;
        for scene in &mut project.compositions {
            for track in &mut scene.tracks {
                for clip in &mut track.clips {
                    if clip.source
                        == (ClipSource::Media {
                            source: *source_id,
                            stream: profile.index,
                        })
                    {
                        for point in &mut clip.time_map.points {
                            point.source_tick =
                                point
                                    .source_tick
                                    .checked_sub(profile.start_tick)
                                    .ok_or("Independent source origin offset overflow")?;
                        }
                        clip.source = ClipSource::Media {
                            source: imported.source.id,
                            stream: 0,
                        };
                    }
                }
            }
        }
        project.assets.push(imported.asset);
        project.sources.push(imported.source);
        if hash(path)? != original_hash {
            return Err("Oracle preparation changed original media".into());
        }
        sources.push(json!({"original":path,"original_sha256":original_hash,"original_stream":profile.index,"original_sample_origin":profile.start_tick,"reference_sample_origin":0,"independent_pcm_sha256":reference_hash,"uncompressed":pcm_path,"uncompressed_sha256":hash(&pcm_path)?,"sample_bits_equal":true}));
    }
    project.validate()?;
    let snapshot = Arc::new(SoundSnapshot::at_output_rate(
        Arc::new(EvaluationSnapshot::new(Arc::new(project))?),
        composition,
        rate,
        SoundBudget::default(),
    )?);
    let channels = snapshot.profile().channels.len();
    let mut renderer = SoundRenderer::new(
        snapshot.clone(),
        PcmBudget::default(),
        SoundRenderBudget::default(),
        cancel,
    )?;
    let mut digest = Sha256::new();
    let mut cursor = 0u64;
    let mut different = 0u64;
    let mut maximum_error = 0f32;
    let mut first_difference = None;
    for frame in 0..duration {
        let end = u64::try_from(
            (u128::from(frame + 1) * u128::from(rate) * u128::from(fps.denominator))
                .div_ceil(u128::from(fps.numerator)),
        )?;
        while cursor < end {
            let count = (end - cursor).min(4096) as u32;
            let plan = snapshot.prepare(cursor, count)?;
            let rendered = renderer.render(&plan)?;
            renderer.validate_result(&rendered)?;
            let start = usize::try_from(cursor)? * channels;
            let expected = rendered.sound().samples();
            let actual = actual
                .get(start..start + expected.len())
                .ok_or("Master ends before graph sound")?;
            for (offset, (expected, actual)) in expected.iter().zip(actual).enumerate() {
                digest.update(expected.to_le_bytes());
                if expected.to_bits() != actual.to_bits() {
                    different += 1;
                    first_difference.get_or_insert((start + offset) / channels);
                    maximum_error = maximum_error.max((expected - actual).abs());
                }
            }
            cursor += u64::from(count);
        }
    }
    renderer.verify_sources()?;
    if actual.len() != usize::try_from(cursor)? * channels
        || hash(project_path)? != project_hash
        || hash(master)? != master_hash
    {
        return Err("Oracle length or preserved input identity differs".into());
    }
    let result = json!({"kind":"independent_source_pcm_master","qualified":different==0,"project_sha256":project_hash,"master_sha256":master_hash,"master_pcm_sha256":actual_hash,"oracle_pcm_sha256":format!("{:x}",digest.finalize()),"sample_rate":rate,"channels":channels,"frames":cursor,"different_sample_values":different,"first_difference_frame":first_difference,"maximum_error":maximum_error,"sources":sources,"application_sha256":hash(&std::env::current_exe()?)?,"limits":["Original PCM comes from full FFmpeg CLI decode stored as verified uncompressed float WAV; graph timing and mixing use the shared Rust implementation.","This isolates compressed decoder history. It is not an independent resampler, physical audibility or client acceptance claim."]});
    fs::write(
        directory.join("qualification.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    Ok(result)
}
