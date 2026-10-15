use crate::{DeliveryRequest, Result};
use editbay_core::{
    Project,
    captions::{CaptionTime, CaptionTrack},
};
use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};

/// Prepare exact sidecars for the selected export range before native rendering.
/// `project`, `request` and `destination` select retained tracks and filenames.
/// Returns new sibling paths and UTF-8 bytes; rejects existing paths and timing loss.
pub fn caption_sidecars(
    project: &Project,
    request: &DeliveryRequest,
    destination: &Path,
) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let scene = project
        .compositions
        .iter()
        .find(|c| c.id == request.composition)
        .ok_or("Caption composition absent")?;
    let range = request.frame_range(scene.duration)?;
    let start = CaptionTime::from_frame(range.start, scene.frame_rate)?;
    let end = CaptionTime::from_frame(range.end, scene.frame_rate)?;
    let mut outputs = vec![];
    for track in project
        .captions
        .iter()
        .filter(|t| t.composition == scene.id && t.enabled)
    {
        if track.delivery.burns_in() {
            track.validate_burn_in()?;
            if track.style.font.is_none() {
                return Err("Choose a retained font before caption burn-in".into());
            }
        }
        let Some(extension) = track.delivery.sidecar_extension() else {
            continue;
        };
        let mut sidecar: CaptionTrack = track.clone();
        sidecar.cues.clear();
        for cue in &track.cues {
            if cue.end.compare(start)? != Ordering::Greater
                || cue.start.compare(end)? != Ordering::Less
            {
                continue;
            }
            let mut cue = cue.clone();
            cue.start = subtract(
                if cue.start.compare(start)? == Ordering::Less {
                    start
                } else {
                    cue.start
                },
                start,
            )?;
            cue.end = subtract(
                if cue.end.compare(end)? == Ordering::Greater {
                    end
                } else {
                    cue.end
                },
                start,
            )?;
            sidecar.cues.push(cue);
        }
        let text = if extension == "srt" {
            sidecar.export_srt()?
        } else {
            sidecar.export_vtt()?
        };
        let path = destination.with_extension(format!("{}.{}", track.id, extension));
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                return Err(format!("Caption sidecar already exists: {}", path.display()).into());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        outputs.push((path, text.into_bytes()));
    }
    Ok(outputs)
}

/// Rebase one exact cue boundary without rounding.
/// `a` and `b` are ordered timestamps; returns their reduced nonnegative difference.
fn subtract(a: CaptionTime, b: CaptionTime) -> Result<CaptionTime> {
    let numerator = u128::from(a.numerator) * u128::from(b.denominator)
        - u128::from(b.numerator) * u128::from(a.denominator);
    let denominator = u128::from(a.denominator) * u128::from(b.denominator);
    let mut x = numerator;
    let mut y = denominator;
    while y != 0 {
        (x, y) = (y, x % y);
    }
    let g = x.max(1);
    Ok(CaptionTime::new(
        (numerator / g).try_into()?,
        (denominator / g).try_into()?,
    )?)
}
