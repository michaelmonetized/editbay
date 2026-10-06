use super::*;

/// Inspect saved procedural masks in both actual native window sizes.
/// `binary`, `project` and `directory` select the application, authored project
/// and new evidence folder. Returns real draw/input/visible-viewer receipts.
pub fn run(binary: &Path, project: &Path, directory: &Path) -> Result<Value> {
    let binary = binary.canonicalize()?;
    let original = project.canonicalize()?;
    let original_hash = hash(&original)?;
    let seed = load(&original)?;
    let composition = seed
        .sequences
        .iter()
        .find_map(|s| s.composition)
        .ok_or("Source sequence absent")?;
    let last = seed
        .compositions
        .iter()
        .find(|c| c.id == composition)
        .ok_or("Composition absent")?
        .duration
        - 1;
    fs::create_dir(directory)?;
    let directory = directory.canonicalize()?;
    let copy = directory.join("Mask.editbay");
    save_new(&seed, &copy)?;
    let catalog = directory.join("catalog");
    fs::create_dir(&catalog)?;
    let (mut app, mut trace) = start(
        &binary,
        &directory.join("state"),
        &catalog,
        &directory.join("native.jsonl"),
        Some(&copy),
    )?;
    picture(&mut trace, 0, 0)?;
    click_control(&mut trace, "source-media")?;
    let native = window(|w| w["pid"] == app.0.id() && w["class"] == "editbay")?;
    let address = native["address"].as_str().ok_or("Window address absent")?;
    let mut views = Vec::new();
    let mut samples = Vec::new();
    for (width, height) in [(1440, 900), (800, 600)] {
        dispatch(&format!(
            "hl.dsp.window.resize({{x={width},y={height},relative=false,window=\"address:{address}\"}})"
        ))?;
        dispatch(&format!(
            "hl.dsp.window.move({{x=80,y=80,relative=false,window=\"address:{address}\"}})"
        ))?;
        thread::sleep(Duration::from_millis(350));
        for frame in [last / 2, last, 0, last / 2, 0] {
            let draw = set_frame(&mut trace, frame)?;
            samples.push(draw.clone());
        }
        trace.focus()?;
        command(
            "grim",
            &[
                "-g",
                &format!("80,80 {width}x{height}"),
                directory
                    .join(format!("native-{width}x{height}.png"))
                    .to_str()
                    .ok_or("Non-UTF8 evidence path")?,
            ],
        )?;
        let visible = set_frame(&mut trace, 1)?;
        let rect = &visible["details"]["controls"]["viewer-picture"];
        let height_visible = rect[3].as_f64().unwrap_or(0.) - rect[1].as_f64().unwrap_or(0.);
        let width_visible = rect[2].as_f64().unwrap_or(0.) - rect[0].as_f64().unwrap_or(0.);
        views.push(json!({"size":[width,height],"image":rect,"height":height_visible,"width":width_visible,"qualified":height_visible>=96. && width_visible>=160. && rect[3].as_f64().is_some_and(|y|y<=f64::from(height)),"observed":visible}));
    }
    let mut latency: Vec<_> = samples
        .iter()
        .map(|r| {
            r["details"]["gpu_draw_completed_us"]
                .as_f64()
                .unwrap_or(f64::INFINITY)
                / 1000.
        })
        .collect();
    let timings = metrics(&mut latency);
    app.kill()?;
    let report = json!({"kind":"native_procedural_masks","binary_sha256":hash(&binary)?,"qualified":views.iter().all(|v|v["qualified"]==true) && hash(&original)?==original_hash && timings["p95_ms"].as_f64().is_some_and(|v|v<=250.),"views":views,"gpu_draw":timings,"samples":samples,"original_unchanged":hash(&original)?==original_hash,"physical_display_verified":false});
    fs::write(
        directory.join("qualification.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
}
