use crate::{Result, hash, timeline_evidence};
use editbay_core::{FrameRange, Project, load};
use editbay_delivery::{DeliveryControl, DeliveryRequest, Receipt, deliver};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command, sync::Arc};
use uuid::Uuid;

/// Compare a range master with independently decoded whole-master slices.
/// `reference`, `output` and `receipt` select real files and declared boundaries.
/// Returns exact pixel/PCM hashes, sample counts and zero-based output metadata.
pub fn compare(reference: &Path, output: &Path, receipt: &Receipt) -> Result<Value> {
    let profile = &receipt.profile;
    let frame_end = profile
        .first_frame
        .checked_add(profile.frames)
        .ok_or("Frame overflow")?;
    let boundary = |frame: u64| -> Result<u64> {
        let numerator = u128::from(frame)
            * u128::from(profile.sample_rate)
            * u128::from(profile.frame_rate.denominator);
        let denominator = u128::from(profile.frame_rate.numerator);
        if denominator == 0 {
            return Err("Reference rate is zero".into());
        }
        Ok(u64::try_from(
            numerator / denominator + u128::from(numerator % denominator != 0),
        )?)
    };
    let first_sample = boundary(profile.first_frame)?;
    let end_sample = boundary(frame_end)?;
    let mut streams = Vec::new();
    for picture in [true, false] {
        let (filter, count, unit) = if picture {
            (
                format!(
                    "trim=start_frame={}:end_frame={frame_end}",
                    profile.first_frame
                ),
                profile.frames,
                u64::from(profile.width) * u64::from(profile.height) * 4,
            )
        } else {
            (
                format!("atrim=start_sample={first_sample}:end_sample={end_sample}"),
                end_sample - first_sample,
                profile.channels.len() as u64 * 4,
            )
        };
        let mut expected = Sha256::new();
        let expected_bytes =
            timeline_evidence::decode(reference, picture, Some(&filter), |bytes| {
                expected.update(bytes)
            })?;
        let mut actual = Sha256::new();
        let actual_bytes =
            timeline_evidence::decode(output, picture, None, |bytes| actual.update(bytes))?;
        let expected = format!("{:x}", expected.finalize());
        let actual = format!("{:x}", actual.finalize());
        if expected_bytes != count * unit || actual_bytes != expected_bytes || expected != actual {
            return Err(format!("Independent range {} differs: {actual_bytes}/{expected_bytes} bytes, {actual}/{expected}", if picture { "picture" } else { "sound" }).into());
        }
        streams.push(json!({"kind":if picture {"picture"} else {"sound"},"units":count,"bytes":actual_bytes,"sha256":actual,"expected_sha256":expected,"exact":true}));
    }
    let inspected = crate::shared_delivery::inspect(output, receipt)?;
    for stream in inspected["metadata"]["streams"]
        .as_array()
        .ok_or("Output metadata missing")?
    {
        if stream["start_pts"] != 0 {
            return Err("Range output does not begin at timestamp zero".into());
        }
    }
    Ok(
        json!({"frames":{"start":profile.first_frame,"end":frame_end},"samples":{"start":first_sample,"end":end_sample},
        "reference_sha256":hash(reference)?,"output_sha256":hash(output)?,"streams":streams,"inspection":inspected,"exact":true}),
    )
}

fn master(
    project: Arc<Project>,
    composition: Uuid,
    sample_rate: u32,
    range: Option<FrameRange>,
    destination: &Path,
) -> Result<Receipt> {
    deliver(
        &std::env::current_exe()?,
        project,
        DeliveryRequest {
            composition,
            sample_rate,
            range,
        },
        destination,
        &DeliveryControl::new()?,
        |_, _| {},
    )
}

/// Qualify real whole and ranged graph masters on two PCM grids.
/// `cli`, `path`, `composition` and new `directory` select the executable, saved
/// project and outputs. Full references use the lab worker; ranges use the CLI.
/// Returns independent FFmpeg slice agreement while preserving every input asset.
pub fn run(cli: &Path, path: &Path, composition: Uuid, directory: &Path) -> Result<Value> {
    let project = Arc::new(load(path)?);
    let scene = project
        .compositions
        .iter()
        .find(|scene| scene.id == composition)
        .ok_or("Composition absent")?;
    if !(10..=600).contains(&scene.duration) || scene.width > 1920 || scene.height > 1080 {
        return Err("Range fixture needs 10–600 frames up to 1920×1080".into());
    }
    let duration = scene.duration;
    let original = hash(path)?;
    let sources = project
        .assets
        .iter()
        .map(|asset| Ok((asset.path.clone(), hash(&asset.path)?)))
        .collect::<Result<Vec<_>>>()?;
    fs::create_dir(directory)?;
    let mut reports = Vec::new();
    for rate in [48000, 44100] {
        let reference = directory.join(format!("Full-{rate}.mov"));
        let full = master(project.clone(), composition, rate, None, &reference)?;
        let full_inspection = crate::shared_delivery::inspect(&reference, &full)?;
        let mut ranges = Vec::new();
        for (first, end) in [
            (2, 7),
            (duration / 2, duration / 2 + 3),
            (duration - 4, duration),
        ] {
            let output = directory.join(format!("Range-{rate}-{first}-{end}.mov"));
            let result = Command::new(cli)
                .arg("export-range")
                .arg(path)
                .arg(composition.to_string())
                .arg(&output)
                .arg(first.to_string())
                .arg(end.to_string())
                .arg(rate.to_string())
                .output()?;
            if !result.status.success() {
                return Err(format!(
                    "Range CLI failed: {}",
                    String::from_utf8_lossy(&result.stderr)
                )
                .into());
            }
            let receipt: Receipt = serde_json::from_slice(&result.stdout)?;
            ranges.push(
                json!({"receipt":receipt,"comparison":compare(&reference, &output, &receipt)?}),
            );
        }
        reports.push(json!({"sample_rate":rate,"full":full,"full_inspection":full_inspection,"ranges":ranges}));
    }
    if hash(path)? != original
        || sources
            .iter()
            .any(|(path, expected)| !hash(path).is_ok_and(|actual| actual == *expected))
    {
        return Err("Range qualification changed source content".into());
    }
    let result = json!({"kind":"exact_range_delivery","qualified":true,"application_sha256":hash(&std::env::current_exe()?)?,
        "cli_sha256":hash(cli)?,"project_sha256":original,"composition":composition,"reports":reports,"originals_unchanged":true,
        "limits":["Actual supervised Rust rendering and native codecs, compared through independent FFmpeg decode and slice filters.","This is range selection proof against shared whole masters, not client approval or a new independent color/resampler qualification."]});
    fs::write(
        directory.join("qualification.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    Ok(result)
}
