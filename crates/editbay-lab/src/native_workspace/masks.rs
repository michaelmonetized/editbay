use super::*;

/// Inspect saved procedural masks in both actual native window sizes.
/// `binary`, `project` and `directory` select the application, authored project
/// and new evidence folder. Returns real draw/input/visible-viewer receipts.
pub fn run(binary: &Path, project: &Path, directory: &Path) -> Result<Value> {
    let binary = binary.canonicalize()?;
    let original = project.canonicalize()?;
    let original_hash = hash(&original)?;
    let seed = load(&original)?;
    let reference = original
        .parent()
        .ok_or("Mask project parent absent")?
        .join("Master.mov");
    if !reference.is_file() {
        return Err(
            "Run mask-graph first; native qualification requires its verified Master.mov".into(),
        );
    }
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
    click_control(&mut trace, "timeline-mark-in")?;
    set_frame(&mut trace, 6)?;
    click_control(&mut trace, "timeline-mark-out")?;
    trace.wait("compact source marks", |r| {
        r["kind"] == "timeline"
            && r["details"]["source_range"]["start"] == 1
            && r["details"]["source_range"]["end"] == 7
    })?;
    let sent = click_control(&mut trace, "timeline-create")?;
    let created = trace.wait("compact cut commit", |r| {
        has_tab(r, &seed.name, seed.revision + 1)
    })?;
    let input_ms = (created["unix_us"].as_u64().ok_or("Commit time absent")? - sent) as f64 / 1000.;
    let timeline = timeline_record(&mut trace, seed.revision + 1)?;
    if timeline["clips"].as_array().is_none_or(|v| v.len() != 1)
        || timeline["clips"][0]["source"]["range"]["start"] != 1
        || timeline["clips"][0]["source"]["range"]["end"] != 7
    {
        return Err("Compact mask cut lost its selected source range".into());
    }
    trace.focus()?;
    key(44, true, false)?;
    trace.wait("compact cut undo", |r| {
        has_tab(r, &seed.name, seed.revision + 2)
    })?;
    key(44, true, true)?;
    trace.wait("compact cut redo", |r| {
        has_tab(r, &seed.name, seed.revision + 3)
    })?;
    key(31, true, false)?;
    trace.wait("compact cut save", |r| {
        has_tab(r, &seed.name, seed.revision + 3) && r["details"]["tabs"][0]["dirty"] == false
    })?;
    let saved = load(&copy)?;
    if saved.compositions.iter().find(|c| c.id == composition)
        != seed.compositions.iter().find(|c| c.id == composition)
        || saved.assets != seed.assets
        || saved.sources != seed.sources
    {
        return Err("Compact editing changed the editable mask source".into());
    }
    command(
        "grim",
        &[
            "-g",
            "80,80 800x600",
            directory
                .join("compact-cut.png")
                .to_str()
                .ok_or("Non-UTF8 evidence path")?,
        ],
    )?;
    let cut: uuid::Uuid = timeline["record"]
        .as_str()
        .ok_or("Cut composition absent")?
        .parse()?;
    let cut_sequence = saved
        .sequences
        .iter()
        .find(|s| s.composition == Some(cut))
        .ok_or("Cut sequence absent")?
        .id;
    click_control(&mut trace, "timeline-view-record")?;
    trace.wait("selected compact cut", |r| {
        r["kind"] == "preview" && r["details"]["sequence"] == cut_sequence.to_string()
    })?;
    let cut_picture = set_frame(&mut trace, 1)?;
    if cut_picture["details"]["sequence"] != cut_sequence.to_string() {
        return Err("Compact viewer did not render the saved cut".into());
    }
    command(
        "grim",
        &[
            "-g",
            "80,80 800x600",
            directory
                .join("compact-cut-drawn.png")
                .to_str()
                .ok_or("Non-UTF8 evidence path")?,
        ],
    )?;
    let destination = directory.join("Cut.mov");
    let after = now();
    choose_master(&mut trace, &destination)?;
    let completed = export_job(&mut trace, after, |job| {
        job["running"] == false && !job["receipt"].is_null()
    })?;
    if completed["receipt"]["version"]["revision"] != saved.revision
        || completed["receipt"]["file_sha256"] != hash(&destination)?
    {
        return Err("Compact export published a different revision or file".into());
    }
    let independent = crate::timeline_evidence::compare(&copy, cut, &reference, &destination)?;
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
    let (mut reopened, mut reopened_trace) = start(
        &binary,
        &directory.join("state"),
        &catalog,
        &directory.join("reopened.jsonl"),
        Some(&copy),
    )?;
    let reopened_picture = picture(&mut reopened_trace, 0, 0)?;
    reopened.kill()?;
    let report = json!({"kind":"native_procedural_masks","binary_sha256":hash(&binary)?,"qualified":views.iter().all(|v|v["qualified"]==true) && input_ms<=50. && hash(&original)?==original_hash && timings["p95_ms"].as_f64().is_some_and(|v|v<=250.) && independent["qualified"]==true,"views":views,"gpu_draw":timings,"samples":samples,"compact_cut":{"input_ms":input_ms,"created":created,"timeline":timeline,"saved_revision":saved.revision,"drawn":cut_picture,"export":completed,"independent":independent,"reopened":reopened_picture},"original_unchanged":hash(&original)?==original_hash,"physical_display_verified":false});
    fs::write(
        directory.join("qualification.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
}
