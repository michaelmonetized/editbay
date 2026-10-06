use crate::{Result, hash, metrics};
use editbay_core::{EvaluationSnapshot, Project, StreamFormat, TimeBase};
use editbay_media::{
    Cancellation, PcmProvider, SourceFile, StreamType,
    pcm_worker::{PcmWorker, PcmWorkerBudget},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc, time::Instant};

fn same_bits(left: &[f32], right: &[f32]) -> bool {
    left.iter()
        .map(|sample| sample.to_bits())
        .eq(right.iter().map(|sample| sample.to_bits()))
}

/// Qualify cold late and random original-channel reads against full independent decode.
/// `path` is preserved media and `executable` hosts the actual codec worker.
/// Returns exact PCM, unchanged budgets, cold/warm timing and owned cleanup receipts.
pub fn run(path: &Path, executable: &Path) -> Result<Value> {
    let cancel = Cancellation::new()?;
    let source = SourceFile::open(path, &cancel)?;
    let fingerprint = source.fingerprint().clone();
    let stream = source
        .probe(cancel.clone())?
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Audio && stream.decoder_available)
        .ok_or("No decodable sound stream")?
        .index;
    let imported = source.ingest(
        "Canonical source sound".into(),
        &[stream],
        cancel.clone(),
        |_, _| {},
    )?;
    let source_id = imported.source.id;
    let profile = &imported.source.streams[0];
    let StreamFormat::Audio {
        sample_rate,
        channels,
    } = &profile.format
    else {
        return Err("Sound profile missing".into());
    };
    let rate = *sample_rate;
    let channels = channels.clone();
    if profile.time_base
        != (TimeBase {
            numerator: 1,
            denominator: rate,
        })
    {
        return Err("Qualification requires normalized captured sample time".into());
    }
    let origin = profile.start_tick;
    let total = i64::try_from(profile.duration_ticks.ok_or("Sound duration missing")?)?;
    if total < 8192 {
        return Err("Qualification needs at least two PCM blocks".into());
    }
    let (reference, reference_hash) = crate::sound_blocks::independent_pcm(path, stream)?;
    let required = usize::try_from(total)?
        .checked_mul(channels.len())
        .ok_or("Reference size overflow")?;
    if reference.len() < required {
        return Err("Independent PCM ends before captured duration".into());
    }
    let mut project = Project::new("Canonical source qualification")?;
    project.assets.push(imported.asset);
    project.sources.push(imported.source);
    let snapshot = Arc::new(EvaluationSnapshot::new(Arc::new(project))?);
    let budget = PcmWorkerBudget::default();
    let mut cold = Vec::new();
    let mut warm = Vec::new();
    let mut comparisons = Vec::new();
    let mut every_exact = true;
    let mut cleanup = true;
    let mut disk_bytes = 0;
    let mut child_high_water_kib = 0u64;
    let mut steps = 0;
    let tail =
        &reference[(total as usize - 4096) * channels.len()..total as usize * channels.len()];
    for trial in 0..10 {
        let began = Instant::now();
        let mut worker =
            PcmWorker::new(executable, snapshot.clone(), budget, Cancellation::new()?)?;
        let late = worker.interval(source_id, stream, origin + total - 4096, 4096)?;
        cold.push(began.elapsed().as_secs_f64() * 1000.);
        every_exact &= same_bits(late.pcm().samples(), tail);
        let decoded_before = worker.transfer_stats().child.decoded_frames;
        for index in 0..32 {
            let first = match index % 8 {
                0 => 0,
                1 => total / 2,
                2 => total - 4097,
                3 => total / 4,
                4 => -16,
                5 => total - 16,
                6 => 1,
                _ => (index as i64 * 7919) % (total - 4096),
            };
            let began = Instant::now();
            let result = worker.interval(source_id, stream, origin + first, 4096)?;
            warm.push(began.elapsed().as_secs_f64() * 1000.);
            let mut expected = Vec::with_capacity(4096 * channels.len());
            for frame in first..first + 4096 {
                if (0..total).contains(&frame) {
                    expected.extend_from_slice(
                        &reference[frame as usize * channels.len()
                            ..(frame as usize + 1) * channels.len()],
                    );
                } else {
                    expected.resize(expected.len() + channels.len(), 0.);
                }
            }
            let exact = same_bits(result.pcm().samples(), &expected);
            every_exact &= exact;
            if trial == 0 {
                let actual: Vec<u8> = result
                    .pcm()
                    .samples()
                    .iter()
                    .flat_map(|sample| sample.to_le_bytes())
                    .collect();
                let expected: Vec<u8> = expected
                    .iter()
                    .flat_map(|sample| sample.to_le_bytes())
                    .collect();
                comparisons.push(json!({"first_sample":origin+first,"frames":4096,"exact":exact,"actual_sha256":format!("{:x}",Sha256::digest(&actual)),"expected_sha256":format!("{:x}",Sha256::digest(&expected))}));
            }
            worker.validate_result(&result)?;
        }
        every_exact &= worker.transfer_stats().child.decoded_frames == decoded_before;
        worker.verify_sources()?;
        let pid = worker.process_id().ok_or("Codec worker missing")?;
        child_high_water_kib = child_high_water_kib.max(
            std::fs::read_to_string(format!("/proc/{pid}/status"))?
                .lines()
                .find_map(|line| {
                    line.strip_prefix("VmHWM:")?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
                .ok_or("Codec RSS missing")?,
        );
        let observed = worker.transfer_stats();
        disk_bytes = disk_bytes.max(observed.child.store_bytes);
        steps = steps.max(observed.child.preparation_steps);
        worker.clear();
        cleanup &= !Path::new(&format!("/proc/{pid}")).exists()
            && worker.transfer_stats().child.store_bytes == 0
            && worker.transfer_stats().child.stores == 0;
        every_exact &= same_bits(late.pcm().samples(), tail);
        cleanup &= worker.validate_result(&late).is_err();
        drop(late);
        cleanup &= worker.transfer_stats().mapped_bytes == 0
            && worker.transfer_stats().mapped_handles == 0;
    }
    source.verify(&cancel)?;
    let cold = metrics(&mut cold);
    let warm = metrics(&mut warm);
    let gates = json!({"exact_pcm":every_exact,"cold_p95_1s":cold["p95_ms"].as_f64().unwrap()<=1000.,"warm_p95_250ms":warm["p95_ms"].as_f64().unwrap()<=250.,"bounded_disk":disk_bytes<=budget.pcm.store_bytes,"child_memory_256mib":child_high_water_kib<=256*1024,"cleanup":cleanup,"source_unchanged":true});
    Ok(
        json!({"kind":"canonical_pcm_qualification","qualified":gates.as_object().unwrap().values().all(|value|value==true),"gates":gates,"source":path,"fingerprint":fingerprint,"source_stream":stream,"sample_rate":rate,"channels":channels,"source_origin":origin,"source_frames":total,"independent_pcm_sha256":reference_hash,"cold_late_reads":cold,"warm_random_reads":warm,"comparisons":comparisons,"budget":budget,"disk_bytes":disk_bytes,"maximum_preparation_steps":steps,"child_high_water_kib":child_high_water_kib,"worker_sha256":hash(executable)?,"application_sha256":hash(&std::env::current_exe()?)?,"limits":["Source PCM and process ownership only; no physical audibility or sustained-clock qualification.","Disk bytes are counted separately from resident memory. Anonymous files live in the configured temporary directory; this run inherits project-local TMPDIR."]}),
    )
}
