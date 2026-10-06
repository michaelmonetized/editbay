use crate::{Result, Worker, hash};
use editbay_core::{load, timeline_clips};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
};
use uuid::Uuid;

/// Compare delivered cuts with independently selected uncut source-master bytes.
/// `project` and `composition` define the saved cut; `reference` is its uncut
/// source master and `output` the new delivery. Returns exact streaming picture
/// and original-channel PCM evidence, rejecting fractional sample boundaries.
pub fn compare(
    project: &Path,
    composition: Uuid,
    reference: &Path,
    output: &Path,
) -> Result<Value> {
    let project = load(project)?;
    let clips = timeline_clips(&project, composition)?;
    let first = clips.first().ok_or("Cannot qualify an empty cut")?;
    let source = project
        .compositions
        .iter()
        .find(|c| c.id == first.source.composition)
        .ok_or("Cut source absent")?;
    let snapshot = editbay_core::SoundSnapshot::at_output_rate(
        std::sync::Arc::new(editbay_core::EvaluationSnapshot::new(std::sync::Arc::new(
            project.clone(),
        ))?),
        composition,
        48000,
        editbay_core::SoundBudget::default(),
    )?;
    let per_frame = 48000u64 * u64::from(source.frame_rate.denominator);
    if !per_frame.is_multiple_of(u64::from(source.frame_rate.numerator)) {
        return Err("This independent slice comparison requires integer sample boundaries".into());
    }
    let samples_per_frame = per_frame / u64::from(source.frame_rate.numerator);
    let mut end = 0;
    for clip in &clips {
        if clip.range.start != end || clip.source.composition != source.id {
            return Err("Slice comparison needs a contiguous cut of one source master".into());
        }
        end = clip.range.end;
    }
    let mut streams = Vec::new();
    for picture in [true, false] {
        let mut expected_hash = Sha256::new();
        let mut expected_bytes = 0;
        let mut boundaries = Vec::new();
        for clip in &clips {
            let (filter, units, bytes_per_unit) = if picture {
                (
                    format!(
                        "trim=start_frame={}:end_frame={}",
                        clip.source.range.start, clip.source.range.end
                    ),
                    clip.range.end - clip.range.start,
                    u64::from(source.width) * u64::from(source.height) * 4,
                )
            } else {
                (
                    format!(
                        "atrim=start_sample={}:end_sample={}",
                        clip.source.range.start * samples_per_frame,
                        clip.source.range.end * samples_per_frame
                    ),
                    (clip.range.end - clip.range.start) * samples_per_frame,
                    snapshot.profile().channels.len() as u64 * 4,
                )
            };
            let mut boundary_hash = Sha256::new();
            let bytes = decode(reference, picture, Some(&filter), |bytes| {
                expected_hash.update(bytes);
                boundary_hash.update(bytes);
            })?;
            if bytes != units * bytes_per_unit {
                return Err("Independent source slice has the wrong duration".into());
            }
            expected_bytes += bytes;
            boundaries.push(json!({"record":clip.range,"source":clip.source.range,"bytes":bytes,"sha256":format!("{:x}",boundary_hash.finalize())}));
        }
        let mut actual_hash = Sha256::new();
        let actual_bytes = decode(output, picture, None, |bytes| actual_hash.update(bytes))?;
        let expected_hash = format!("{:x}", expected_hash.finalize());
        let actual_hash = format!("{:x}", actual_hash.finalize());
        if actual_bytes != expected_bytes || actual_hash != expected_hash {
            return Err(format!("Independent {} cut differs: {actual_bytes}/{expected_bytes} bytes, {actual_hash}/{expected_hash}",if picture {"picture"} else {"sound"}).into());
        }
        streams.push(json!({"kind":if picture {"picture"} else {"sound"},"bytes":actual_bytes,"sha256":actual_hash,"expected_sha256":expected_hash,"boundaries":boundaries,"exact":true}));
    }
    Ok(
        json!({"kind":"independent_timeline_slices","qualified":true,"composition":composition,"source":source.id,"frames":end,"sample_rate":48000,"samples_per_frame":samples_per_frame,"channels":snapshot.profile().channels,"reference_sha256":hash(reference)?,"output_sha256":hash(output)?,"streams":streams}),
    )
}

pub(crate) fn decode(
    path: &Path,
    picture: bool,
    filter: Option<&str>,
    mut consume: impl FnMut(&[u8]),
) -> Result<u64> {
    let mut command = Command::new("ffmpeg");
    command
        .args(["-v", "error", "-nostdin", "-threads", "2", "-i"])
        .arg(path);
    if picture {
        command.args(["-map", "0:v:0"]);
        if let Some(filter) = filter {
            command.args(["-vf", filter]);
        }
        command.args([
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            "rgba",
            "-f",
            "rawvideo",
            "pipe:1",
        ]);
    } else {
        command.args(["-map", "0:a:0"]);
        if let Some(filter) = filter {
            command.args(["-af", filter]);
        }
        command.args(["-c:a", "pcm_f32le", "-f", "f32le", "pipe:1"]);
    }
    let mut child = Worker(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?,
    );
    let mut output = child
        .0
        .stdout
        .take()
        .ok_or("Independent decoder has no output")?;
    let mut buffer = [0u8; 65536];
    let mut bytes = 0;
    loop {
        let count = output.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        consume(&buffer[..count]);
        bytes += count as u64;
    }
    if !child.0.wait()?.success() {
        return Err("Independent slice decoder failed".into());
    }
    Ok(bytes)
}

/// Reorder a saved two-clip cut through the actual version-owned CLI.
/// `cli`, `project`, `composition`, `reference` and new `directory` select
/// production executables and content. Returns stale-request preservation,
/// backward-seek delivery, and independent source-slice comparison evidence.
pub fn reorder(
    cli: &Path,
    project: &Path,
    composition: Uuid,
    reference: &Path,
    directory: &Path,
) -> Result<Value> {
    std::fs::create_dir(directory)?;
    let file = directory.join("Reordered.editbay");
    let original_hash = hash(project)?;
    let mut edited = load(project)?;
    editbay_core::save_new(&edited, &file)?;
    let clips = timeline_clips(&edited, composition)?;
    if clips.len() != 2 {
        return Err("Reorder qualification requires two linked groups".into());
    }
    let first = clips[0].id;
    let second = clips[1].id;
    let first_end = clips[1].range.end + 1;
    let second_length = clips[1].range.end - clips[1].range.start;
    let request_path = directory.join("Request.json");
    let mut receipts = Vec::new();
    for (clip, at) in [(first, first_end), (second, 0), (first, second_length)] {
        let request = json!({"expected":editbay_core::DocumentVersion::of(&edited),"action":{"kind":"move","composition":composition,"clip":clip,"at":at}});
        std::fs::write(&request_path, serde_json::to_vec(&request)?)?;
        let execute = || {
            Command::new(cli)
                .arg("timeline")
                .arg(&file)
                .arg(&request_path)
                .output()
        };
        let applied = execute()?;
        if !applied.status.success() {
            return Err(format!(
                "Timeline CLI failed: {}",
                String::from_utf8_lossy(&applied.stderr)
            )
            .into());
        }
        let saved_hash = hash(&file)?;
        let stale = execute()?;
        if stale.status.success()
            || hash(&file)? != saved_hash
            || !String::from_utf8_lossy(&stale.stderr).contains("stale")
        {
            return Err("Stale timeline CLI request modified the saved cut".into());
        }
        edited = load(&file)?;
        receipts.push(json!({"applied":serde_json::from_slice::<Value>(&applied.stdout)?,"stale_rejected":true,"saved_sha256":saved_hash}));
    }
    if hash(project)? != original_hash {
        return Err("CLI reorder modified its source project".into());
    }
    let destination = directory.join("Reordered.mov");
    let delivery =
        crate::shared_delivery::run(&file, &composition.to_string(), &destination, "full")?;
    let comparison = compare(&file, composition, reference, &destination)?;
    Ok(
        json!({"kind":"cli_timeline_reorder","qualified":true,"cli_sha256":hash(cli)?,"original_project_sha256":original_hash,"original_unchanged":true,"receipts":receipts,"delivery":delivery,"comparison":comparison}),
    )
}
