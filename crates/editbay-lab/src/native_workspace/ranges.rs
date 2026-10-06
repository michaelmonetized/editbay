use super::*;

pub(super) fn choose(trace: &mut Trace, destination: &Path) -> Result<()> {
    click_control(trace, "export-sequence")?;
    click_control(trace, "export-range")?;
    input(trace, "export-start", 2)?;
    input(trace, "export-end", 7)?;
    click_control(trace, "export-continue")?;
    choose_destination(destination)
}

fn input(trace: &mut Trace, field: &str, value: u64) -> Result<()> {
    let after = click_control(trace, field)?;
    trace.wait("range field keyboard focus", |r| {
        r["kind"] == "frame"
            && r["unix_us"].as_u64().is_some_and(|time| time >= after)
            && r["details"]["text_input_focused"] == true
    })?;
    key(30, true, false)?;
    command("wtype", &["-s", "40", &value.to_string(), "-s", "80"])?;
    key(28, false, false)?;
    Ok(())
}

fn options(trace: &mut Trace, start: u64, end: u64, after: u64) -> Result<Value> {
    let start = start.to_string();
    let end = end.to_string();
    trace.wait("native range options", |r| {
        r["kind"] == "delivery"
            && r["unix_us"].as_u64().is_some_and(|time| time >= after)
            && r["details"]["choice"]["full"] == false
            && r["details"]["choice"]["start"].as_str() == Some(start.as_str())
            && r["details"]["choice"]["end"].as_str() == Some(end.as_str())
    })
}

/// Export a native frame selection and verify independently sliced pixels/PCM.
/// `binary`, `project`, `reference` and new `directory` identify the actual app,
/// saved source sequence, whole 48 kHz master and evidence folder. Returns real
/// chooser, compact-window, invalid-input, original-preservation and draw receipts.
pub fn run(binary: &Path, project: &Path, reference: &Path, directory: &Path) -> Result<Value> {
    let binary = binary.canonicalize()?;
    let project = project.canonicalize()?;
    let original = hash(&project)?;
    let seed = load(&project)?;
    let sources = seed
        .assets
        .iter()
        .map(|a| Ok((a.path.clone(), hash(&a.path)?)))
        .collect::<Result<Vec<_>>>()?;
    fs::create_dir(directory)?;
    let directory = directory.canonicalize()?;
    let copy = directory.join("Range.editbay");
    save_new(&seed, &copy)?;
    let copy_hash = hash(&copy)?;
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
    let native = window(|w| w["pid"] == app.0.id() && w["class"] == "editbay")?;
    let address = native["address"].as_str().ok_or("Window identity absent")?;
    let mut views = Vec::new();
    let mut latency = Vec::new();
    for (width, height) in [(1440, 900), (800, 600)] {
        dispatch(&format!(
            "hl.dsp.window.resize({{x={width},y={height},relative=false,window=\"address:{address}\"}})"
        ))?;
        dispatch(&format!(
            "hl.dsp.window.move({{x=80,y=80,relative=false,window=\"address:{address}\"}})"
        ))?;
        thread::sleep(Duration::from_millis(350));
        let after = click_control(&mut trace, "export-sequence")?;
        trace.wait("native export options opened", |r| {
            r["kind"] == "delivery"
                && r["unix_us"].as_u64().is_some_and(|t| t >= after)
                && r["details"]["choice"].is_object()
        })?;
        click_control(&mut trace, "export-range")?;
        input(&mut trace, "export-start", 7)?;
        input(&mut trace, "export-end", 2)?;
        let invalid = options(&mut trace, 7, 2, after)?;
        let blocked = click_control(&mut trace, "export-continue")?;
        trace.wait("disabled range action input", |r| {
            r["kind"] == "frame" && r["unix_us"].as_u64().is_some_and(|time| time >= blocked)
        })?;
        trace.read()?;
        let unchanged = trace
            .records
            .iter()
            .rev()
            .find(|r| r["kind"] == "delivery")
            .ok_or("Range options state absent")?;
        if unchanged["details"] != invalid["details"] {
            return Err("Invalid native range escaped its options".into());
        }
        input(&mut trace, "export-start", 2)?;
        input(&mut trace, "export-end", 7)?;
        let selected = options(&mut trace, 2, 7, blocked)?;
        trace.focus()?;
        command(
            "grim",
            &[
                "-g",
                &format!("80,80 {width}x{height}"),
                directory
                    .join(format!("range-{width}x{height}.png"))
                    .to_str()
                    .ok_or("Screenshot path invalid")?,
            ],
        )?;
        views.push(json!({"size":[width,height],"invalid":invalid,"selected":selected}));
        if width == 1440 {
            click_control(&mut trace, "export-options-cancel")?;
        }
    }
    let destination = directory.join("Range.mov");
    let after = click_control(&mut trace, "export-continue")?;
    choose_destination(&destination)?;
    let completed = export_job(&mut trace, after, |j| {
        j["running"] == false && (!j["receipt"].is_null() || !j["error"].is_null())
    })?;
    if completed["receipt"].is_null() {
        return Err(format!("Native range export failed: {}", completed["error"]).into());
    }
    let receipt: editbay_delivery::Receipt = serde_json::from_value(completed["receipt"].clone())?;
    if receipt.profile.first_frame != 2
        || receipt.profile.frames != 5
        || receipt.version != editbay_core::DocumentVersion::of(&seed)
    {
        return Err("Native range exported another selection or version".into());
    }
    for frame in [1, 0, 1, 0, 1] {
        let sent = click_control(
            &mut trace,
            if frame == 0 {
                "previous-frame"
            } else {
                "next-frame"
            },
        )?;
        let drawn = picture(&mut trace, frame, sent)?;
        let accepted = drawn["details"]["request_accepted_unix_us"]
            .as_u64()
            .ok_or("Input receipt absent")?;
        latency.push((accepted - sent) as f64 / 1000.);
    }
    trace.focus()?;
    command(
        "grim",
        &[
            "-g",
            "80,80 800x600",
            directory
                .join("exported.png")
                .to_str()
                .ok_or("Screenshot path invalid")?,
        ],
    )?;
    app.kill()?;
    let independent = crate::range_delivery::compare(reference, &destination, &receipt)?;
    if hash(&project)? != original
        || hash(&copy)? != copy_hash
        || sources
            .iter()
            .any(|(path, expected)| !hash(path).is_ok_and(|actual| actual == *expected))
    {
        return Err("Native range export changed project or media".into());
    }
    let input = metrics(&mut latency);
    let qualified = input["p95_ms"].as_f64().is_some_and(|ms| ms <= 50.);
    let result = json!({"kind":"native_range_delivery","qualified":qualified,"application_sha256":hash(&binary)?,"project_sha256":original,"completed":completed,"independent":independent,"views":views,"input":input,"originals_unchanged":true,"limits":["Actual native windows and file chooser with software-injected input.","No physical output, client approval or independent-user acceptance claim."]});
    fs::write(
        directory.join("qualification.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    if !qualified {
        return Err("Native range input latency exceeded 50 ms".into());
    }
    Ok(result)
}
