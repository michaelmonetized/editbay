use crate::{Result, hash, memory, metrics};
use editbay_core::*;
use editbay_media::{Cancellation, PictureBudget};
use editbay_render::{GraphBudget, GraphRenderer};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path, process::Command, sync::Arc, time::Instant};
use uuid::Uuid;

const BACKGROUND: [f64; 4] = [0.05, 0.01, 0.1, 1.];

fn alpha(points: &[[f64; 2]], at: [f64; 2], feather: f64) -> f64 {
    let mut crossings = 0;
    let mut distance = f64::INFINITY;
    for (a, b) in points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
    {
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let length = dx.hypot(dy);
        let along = if length == 0. {
            0.
        } else {
            ((at[0] - a[0]) * dx / length + (at[1] - a[1]) * dy / length).clamp(0., length)
        };
        let t = if length == 0. { 0. } else { along / length };
        distance = distance.min((at[0] - a[0] - t * dx).hypot(at[1] - a[1] - t * dy));
        if (a[1] > at[1]) != (b[1] > at[1]) && at[0] < a[0] + (at[1] - a[1]) * dx / dy {
            crossings += 1;
        }
    }
    let signed = if crossings % 2 == 1 {
        distance
    } else {
        -distance
    };
    let t = (0.5 + signed / feather.max(1.)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
fn snapshot(p: Project) -> Result<Arc<EvaluationSnapshot>> {
    Ok(Arc::new(EvaluationSnapshot::new(Arc::new(p))?))
}
fn encode(value: f64) -> u8 {
    let value = if value <= 0.0031308 {
        12.92 * value
    } else {
        1.055 * value.max(0.).powf(1. / 2.4) - 0.055
    };
    (value.clamp(0., 1.) * 255.).round() as u8
}

/// Author and deliver a real source graph with editable animated geometry.
/// `original` supplies a saved source sequence and `directory` must be new.
/// Returns GPU versus independent f64 mask math, history/recovery, decoded
/// master and unchanged-input evidence; source color evaluation is shared.
pub fn run(original: &Path, directory: &Path) -> Result<Value> {
    let original_hash = hash(original)?;
    let original_project = load(original)?;
    if original_project.color.working_gamut != WorkingGamut::Bt709
        || original_project.color.output_gamut != WorkingGamut::Bt709
        || original_project.color.output_transfer != OutputTransfer::Srgb
    {
        return Err(
            "Mask qualification requires the declared BT.709 working/sRGB output fixture".into(),
        );
    }
    let composition = original_project
        .sequences
        .iter()
        .find_map(|s| s.composition)
        .ok_or("Source sequence absent")?;
    let mut scene = original_project
        .compositions
        .iter()
        .find(|c| c.id == composition)
        .ok_or("Composition absent")?
        .clone();
    if scene.width > 1920 || scene.height > 1080 || scene.duration < 3 || scene.duration > 600 {
        return Err(
            "Mask qualification requires a 3–600 frame source no larger than 1920×1080".into(),
        );
    }
    fs::create_dir(directory)?;
    let source_hashes: Vec<_> = original_project
        .assets
        .iter()
        .map(|a| hash(&a.path))
        .collect::<Result<_>>()?;
    let points = vec![
        [
            (f64::from(scene.width) * 0.13).round() + 0.25,
            (f64::from(scene.height) * 0.12).round() + 0.25,
        ],
        [
            (f64::from(scene.width) * 0.87).round() + 0.25,
            (f64::from(scene.height) * 0.21).round() + 0.25,
        ],
        [
            (f64::from(scene.width) * 0.77).round() + 0.25,
            (f64::from(scene.height) * 0.9).round() + 0.25,
        ],
        [
            (f64::from(scene.width) * 0.21).round() + 0.25,
            (f64::from(scene.height) * 0.78).round() + 0.25,
        ],
    ];
    let geometry = Uuid::new_v4();
    let mask = Uuid::new_v4();
    let background = Uuid::new_v4();
    let over = Uuid::new_v4();
    let range = FrameRange {
        start: 0,
        end: scene.duration,
    };
    let node = |id, operation| TimedNode {
        id,
        range,
        operation,
        animation: vec![],
    };
    let mut mask_node = node(
        mask,
        NodeOperation::Mask {
            geometry,
            feather: 6.,
            inverted: false,
        },
    );
    mask_node.animation = vec![AnimationChannel {
        id: Uuid::new_v4(),
        property: AnimatedProperty::Feather,
        interpolation: Interpolation::Linear,
        keys: vec![
            Keyframe {
                frame: 0,
                value: 6.,
                in_tangent: 0.,
                out_tangent: 0.,
            },
            Keyframe {
                frame: scene.duration - 1,
                value: 32.,
                in_tangent: 0.,
                out_tangent: 0.,
            },
        ],
    }];
    scene.nodes.extend([
        node(
            geometry,
            NodeOperation::Polygon {
                points: points.clone(),
            },
        ),
        mask_node,
        node(background, NodeOperation::Solid { rgba: BACKGROUND }),
        node(
            over,
            NodeOperation::Over {
                foreground: scene.picture.ok_or("Picture root absent")?,
                background,
                mask: Some(mask),
            },
        ),
    ]);
    scene.picture = Some(over);
    let mut editor = DocumentEditor::new(original_project.clone())?;
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Feathered source mask".into(),
        &[
            DocumentCommand::SetComposition {
                composition: scene.clone(),
            },
            DocumentCommand::RenameProject {
                name: "Editable source mask".into(),
            },
        ],
    )?;
    let authored = editor.project().clone();
    editor.undo(DocumentVersion::of(editor.project()))?;
    if editor.project().compositions != original_project.compositions {
        return Err("Mask undo changed source graph".into());
    }
    editor.redo(DocumentVersion::of(editor.project()))?;
    if editor.project().compositions != authored.compositions {
        return Err("Mask redo changed authored graph".into());
    }
    let saved = directory.join("Mask.editbay");
    save_new(editor.project(), &saved)?;
    let reopened = load(&saved)?;
    let checkpoint = checkpoint(&reopened, Some(&saved), directory.join("Recovery"))?;
    let recovered = recover_copy(checkpoint, directory.join("Recovered.editbay"))?;
    if recovered.compositions != reopened.compositions
        || reopened.assets != original_project.assets
        || reopened.sources != original_project.sources
    {
        return Err("Mask persistence changed source content".into());
    }
    let targets = [0, scene.duration / 2, scene.duration - 1];
    let mut formats = Vec::new();
    let mut master_reference = BTreeMap::new();
    for (precision, tolerance) in [
        (FloatPrecision::Half, 0.002),
        (FloatPrecision::Full, 0.00002),
    ] {
        let mut baseline = original_project.clone();
        baseline.color.precision = precision;
        let mut changed = reopened.clone();
        changed.color.precision = precision;
        let changed = snapshot(changed)?;
        let mut worker = GraphRenderer::new(
            snapshot(baseline.clone())?,
            PictureBudget::default(),
            GraphBudget::default(),
            Cancellation::new()?,
        )?;
        let mut errors = Vec::new();
        let mut timings = Vec::new();
        let mut hits = Vec::new();
        for frame in targets {
            worker.rebind(snapshot(baseline.clone())?, Cancellation::new()?)?;
            let bare = worker.render(
                composition,
                SourcePosition::new(i64::try_from(frame)?, 1)?,
                false,
            )?;
            let input = worker.readback(&bare)?;
            drop(bare);
            worker.rebind(changed.clone(), Cancellation::new()?)?;
            let began = Instant::now();
            let rendered = worker.render(
                composition,
                SourcePosition::new(i64::try_from(frame)?, 1)?,
                false,
            )?;
            worker.finish()?;
            timings.push(began.elapsed().as_secs_f64() * 1000.);
            let pixels = worker.readback(&rendered)?;
            let feather = 6. + 26. * frame as f64 / (scene.duration - 1) as f64;
            let mut maximum = 0f64;
            let mut expected_bytes = Vec::with_capacity(pixels.len());
            for (index, (input, output)) in input
                .as_chunks::<4>()
                .0
                .iter()
                .zip(pixels.as_chunks::<4>().0.iter())
                .enumerate()
            {
                let x = index % scene.width as usize;
                let y = index / scene.width as usize;
                let alpha = alpha(&points, [x as f64 + 0.5, y as f64 + 0.5], feather);
                for c in 0..4 {
                    let expected = f64::from(input[c]) * alpha
                        + BACKGROUND[c] * (1. - f64::from(input[3]) * alpha);
                    if !output[c].is_finite() {
                        return Err("Nonfinite GPU mask output".into());
                    }
                    maximum = maximum.max((f64::from(output[c]) - expected).abs());
                    expected_bytes.push(if c == 3 {
                        (expected.clamp(0., 1.) * 255.).round() as u8
                    } else {
                        encode(expected)
                    });
                }
            }
            errors.push(json!({"frame":frame,"maximum_error":maximum,"tolerance":tolerance,"qualified":maximum<=tolerance}));
            if precision == reopened.color.precision {
                master_reference.insert(frame, expected_bytes);
            }
            let began = Instant::now();
            let hit = worker.render(
                composition,
                SourcePosition::new(i64::try_from(frame)?, 1)?,
                false,
            )?;
            worker.finish()?;
            hits.push(began.elapsed().as_secs_f64() * 1000.);
            if !Arc::ptr_eq(hit.image(), rendered.image()) {
                return Err("Mask exact repeat missed content ownership".into());
            }
        }
        worker.verify_sources()?;
        worker.clear()?;
        let stats = worker.stats();
        if stats.live_texture_bytes != 0 || stats.pending_submissions != 0 {
            return Err("Mask GPU pins survived cleanup".into());
        }
        formats.push(json!({"precision":precision,"pixels":errors,"incremental_mask_gpu":metrics(&mut timings),"exact_cache_hit":metrics(&mut hits),"retired":stats}));
    }
    let destination = directory.join("Master.mov");
    let delivery =
        crate::shared_delivery::run(&saved, &composition.to_string(), &destination, "full")?;
    let select = format!(
        "select=eq(n\\,{})+eq(n\\,{})+eq(n\\,{})",
        targets[0], targets[1], targets[2]
    );
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-threads", "2", "-i"])
        .arg(&destination)
        .args([
            "-map",
            "0:v:0",
            "-vf",
            &select,
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            "rgba",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()?;
    if !decoded.status.success() {
        return Err(format!(
            "Independent mask master decode failed: {}",
            String::from_utf8_lossy(&decoded.stderr)
        )
        .into());
    }
    let expected: Vec<_> = master_reference.values().flatten().copied().collect();
    if decoded.stdout.len() != expected.len() {
        return Err("Independent mask master frame count differs".into());
    }
    let master_error = decoded
        .stdout
        .iter()
        .zip(&expected)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .ok_or("Empty decoded master")?;
    let sources_unchanged = original_project
        .assets
        .iter()
        .zip(&source_hashes)
        .all(|(a, b)| hash(&a.path).is_ok_and(|v| v == *b));
    let gates = json!({"float_pixels":formats.iter().all(|f|f["pixels"].as_array().is_some_and(|p|p.iter().all(|v|v["qualified"]==true))),"incremental_gpu_p95_33_3ms":formats.iter().all(|f|f["incremental_mask_gpu"]["p95_ms"].as_f64().is_some_and(|v|v<=33.3)),"master_pixels_within_one_byte":master_error<=1,"sources_unchanged":sources_unchanged,"original_project_unchanged":hash(original)?==original_hash,"saved_recovered_equal":true});
    let report = json!({"kind":"procedural_gpu_mask","binary_sha256":hash(&std::env::current_exe()?)?,"qualified":gates.as_object().is_some_and(|g|g.values().all(|v|v==true)),"gates":gates,"composition":composition,"frames":scene.duration,"dimensions":[scene.width,scene.height],"points":points,"formats":formats,"master_maximum_byte_error":master_error,"reference":"Independent f64 polygon and compositing math over shared unmasked source-color GPU pixels; installed FFmpeg decodes delivered master","delivery":delivery,"memory":memory()?});
    fs::write(
        directory.join("qualification.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
}
