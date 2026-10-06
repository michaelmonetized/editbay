use crate::{Result, hash, memory, metrics, sound_blocks::independent_pcm};
use editbay_audio::{
    MonitorRoute, PlaybackPhase, PlaybackStart, SoundRenderBudget, SoundRenderer, StreamingPlayback,
};
use editbay_core::*;
use editbay_media::{
    Cancellation,
    pcm_worker::{PcmWorker, PcmWorkerBudget},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

/// Author and exercise a longer saved cut using real source media and sound.
/// `project`, `reference`, new `directory` and `count` select a preserved source,
/// independently decoded uncut master, private output and 65–256 linked edits.
/// Returns exact PCM, bounded preparation, persistence and actual device evidence.
pub fn run(project: &Path, reference: &Path, directory: &Path, count: usize) -> Result<Value> {
    if !(65..=256).contains(&count) {
        return Err("Long-cut qualification needs 65–256 edits".into());
    }
    let original_hash = hash(project)?;
    let original = load(project)?;
    let source = original
        .sequences
        .iter()
        .find_map(|sequence| sequence.composition)
        .ok_or("Missing source sequence")?;
    let scene = original
        .compositions
        .iter()
        .find(|scene| scene.id == source)
        .ok_or("Missing source composition")?;
    if scene.duration < 3 {
        return Err("Long-cut fixture needs at least three frames".into());
    }
    let source_duration = scene.duration;
    let per_frame = 48000u64 * u64::from(scene.frame_rate.denominator);
    if !per_frame.is_multiple_of(u64::from(scene.frame_rate.numerator)) {
        return Err("Independent long-cut PCM requires integer frame sample boundaries".into());
    }
    let per_frame = per_frame / u64::from(scene.frame_rate.numerator);
    std::fs::create_dir(directory)?;
    let mut editor = DocumentEditor::new(original.clone())?;
    let mut record = None;
    let mut edits = Vec::new();
    for index in 0..count {
        let first = index as u64 * 37 % (source_duration - 1);
        let source = SourceSelection {
            composition: source,
            range: FrameRange {
                start: first,
                end: first + 1,
            },
        };
        let action = match record {
            None => TimelineAction::Create {
                name: "Long cut".into(),
                source,
            },
            Some(composition) => TimelineAction::Insert {
                composition,
                at: index as u64,
                source,
            },
        };
        let began = Instant::now();
        let change = timeline_edit(editor.project(), &action)?;
        editor.apply(
            DocumentVersion::of(editor.project()),
            "Append long-cut selection".into(),
            &change.commands,
        )?;
        record = Some(change.composition);
        edits.push(began.elapsed().as_secs_f64() * 1000.);
    }
    let record = record.ok_or("Long cut missing")?;
    let clips = timeline_clips(editor.project(), record)?;
    if clips.len() != count {
        return Err("Long cut lost linked groups".into());
    }
    let final_project = editor.project().clone();
    editor.undo(DocumentVersion::of(editor.project()))?;
    if timeline_clips(editor.project(), record)?.len() != count - 1 {
        return Err("Long-cut undo did not remove the last linked group".into());
    }
    editor.redo(DocumentVersion::of(editor.project()))?;
    if editor.project().compositions != final_project.compositions {
        return Err("Long-cut redo changed content or identities".into());
    }
    let saved = directory.join("Long.editbay");
    save_new(editor.project(), &saved)?;
    let reopened = load(&saved)?;
    let checkpoint = checkpoint(&reopened, Some(&saved), directory.join("Recovery"))?;
    let recovered = recover_copy(checkpoint, directory.join("Recovered.editbay"))?;
    if recovered.compositions != reopened.compositions
        || reopened.assets != original.assets
        || reopened.sources != original.sources
    {
        return Err("Long-cut persistence changed original content".into());
    }
    let began = Instant::now();
    let sound = Arc::new(SoundSnapshot::at_output_rate(
        Arc::new(EvaluationSnapshot::new(Arc::new(reopened.clone()))?),
        record,
        48000,
        SoundBudget::default(),
    )?);
    let compilation_ms = began.elapsed().as_secs_f64() * 1000.;
    let compiled_memory = memory()?;
    let high_water = compiled_memory
        .as_array()
        .ok_or("Compiled sound memory is not an array")?
        .iter()
        .filter_map(Value::as_str)
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok())
        })
        .ok_or("Compiled sound memory missing")?;
    let (reference_pcm, reference_pcm_sha256) = independent_pcm(reference, 1)?;
    let channels = sound.profile().channels.len();
    let worker = PcmWorker::new(
        &std::env::current_exe()?,
        sound.evaluation().clone(),
        PcmWorkerBudget::default(),
        Cancellation::new()?,
    )?;
    let pid = worker.process_id().ok_or("PCM worker missing")?;
    let mut renderer =
        SoundRenderer::with_provider(sound.clone(), worker, SoundRenderBudget::default())?;
    let mut preparation = Vec::new();
    let mut rendering = Vec::new();
    let mut observations = Vec::new();
    let mut maximum_error = 0f32;
    let mut digest = Sha256::new();
    let mut first = 0;
    while first < sound.duration_samples() {
        let frames = (sound.duration_samples() - first).min(4096) as u32;
        let began = Instant::now();
        let plan = sound.prepare(first, frames)?;
        let prepare_ms = began.elapsed().as_secs_f64() * 1000.;
        preparation.push(prepare_ms);
        let began = Instant::now();
        let result = renderer.render(&plan)?;
        rendering.push(began.elapsed().as_secs_f64() * 1000.);
        renderer.validate_result(&result)?;
        for (offset, values) in result.sound().samples().chunks_exact(channels).enumerate() {
            let position = first + offset as u64;
            let cut = &clips[(position / per_frame) as usize];
            let expected = cut.source.range.start * per_frame + position % per_frame;
            for (channel, sample) in values.iter().enumerate() {
                let expected = *reference_pcm
                    .get(expected as usize * channels + channel)
                    .ok_or("Reference master lost source sound")?;
                if !sample.is_finite() {
                    return Err("Nonfinite long-cut PCM".into());
                }
                maximum_error = maximum_error.max((sample - expected).abs());
                digest.update(sample.to_le_bytes());
            }
        }
        observations.push(json!({"first_sample":first,"frames":frames,"work":plan.work(),"prepare_ms":prepare_ms}));
        first += u64::from(frames);
    }
    renderer.clear();
    drop(renderer);
    let pcm_reaped = !Path::new(&format!("/proc/{pid}")).exists();
    let playback = playback(Arc::new(reopened), record)?;
    let preparation = metrics(&mut preparation);
    let rendering = metrics(&mut rendering);
    let gates = json!({"pcm_exact":maximum_error==0.,"planning_p95_5ms":preparation["p95_ms"].as_f64().is_some_and(|value|value<=5.),"compiled_process_256mib":high_water<=262144,"active_paths_bounded":observations.iter().all(|value|value["work"]["active_paths"].as_u64().is_some_and(|count|count<=5)),"pcm_reaped":pcm_reaped,"source_project_unchanged":hash(project)?==original_hash,"saved_recovered_equal":true,"device_finished":true});
    Ok(
        json!({"kind":"indexed_long_cut_sound","qualified":gates.as_object().unwrap().values().all(|value|*value==Value::Bool(true)),"gates":gates,"application_sha256":hash(&std::env::current_exe()?)?,"original_project_sha256":original_hash,"reference_sha256":hash(reference)?,"reference_pcm_sha256":reference_pcm_sha256,"project":saved,"composition":record,"clips":count,"samples":sound.duration_samples(),"channels":sound.profile().channels,"index":sound.index_stats(),"compilation_ms":compilation_ms,"compiled_memory":compiled_memory,"edit_command_ms":metrics(&mut edits),"preparation":preparation,"rendering":rendering,"maximum_absolute_pcm_error":maximum_error,"pcm_sha256":format!("{:x}",digest.finalize()),"blocks":observations,"playback":playback,"limits":["Real saved-cut sound and backend callbacks; no physical audibility or two-hour drift claim","Sound qualification; native viewing and full picture master require separate receipts"]}),
    )
}

fn playback(project: Arc<Project>, composition: Uuid) -> Result<Value> {
    let mut sound = StreamingPlayback::start(
        project,
        composition,
        PlaybackStart::Frame(0),
        MonitorRoute::Stereo,
    )?;
    let began = Instant::now();
    let mut observed = 0;
    let mut maximum_prepared = 0;
    let final_status = loop {
        let status = sound.status();
        if status
            .position_samples
            .is_some_and(|position| position < observed)
        {
            return Err("Long-cut clock moved backward".into());
        }
        observed = status.position_samples.unwrap_or(observed);
        maximum_prepared = maximum_prepared.max(status.prepared_frames);
        if sound.is_finished() {
            sound.reap();
            break sound.status();
        }
        if began.elapsed() > Duration::from_secs(120) {
            return Err("Long-cut playback exceeded its deadline".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    if final_status.phase != PlaybackPhase::Finished
        || final_status.position_samples != final_status.end_sample
        || maximum_prepared > 16384
        || final_status
            .device_worker_pid
            .is_none_or(|pid| Path::new(&format!("/proc/{pid}")).exists())
        || final_status
            .worker_pid
            .is_none_or(|pid| Path::new(&format!("/proc/{pid}")).exists())
    {
        return Err(format!("Long-cut device failed: {final_status:?}").into());
    }
    Ok(
        json!({"elapsed_seconds":began.elapsed().as_secs_f64(),"maximum_prepared_frames":maximum_prepared,"final":final_status,"device_and_pcm_reaped":true}),
    )
}
