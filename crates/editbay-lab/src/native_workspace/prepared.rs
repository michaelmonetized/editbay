use super::*;

/// Exercise real prepared-picture cancellation, worker death and underflow.
/// `binary`, `project` and `directory` select the app, saved source sequence and
/// new evidence folder. Returns actual process retirement and exact resume data.
pub fn run(binary: &Path, project: &Path, directory: &Path) -> Result<Value> {
    let binary = binary.canonicalize()?;
    let original = project.canonicalize()?;
    let original_hash = hash(&original)?;
    let seed = load(&original)?;
    fs::create_dir(directory)?;
    let directory = directory.canonicalize()?;
    let copy = directory.join("Preview.editbay");
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
    let first = picture(&mut trace, 0, 0)?;
    let stopped_pid = first["details"]["worker_pid"]
        .as_u64()
        .ok_or("Picture worker PID absent")?;
    command("kill", &["-STOP", &stopped_pid.to_string()])?;
    let began = click_control(&mut trace, "play-sequence")?;
    let preparing = sound_record(&mut trace, "stalled picture preparation", began, |d| {
        d["preparing_playback"] == true && d["sound_active"] == false
    })?;
    let paused_at = click_control(&mut trace, "pause-sequence")?;
    let cancelled = sound_record(
        &mut trace,
        "cancel stalled picture preparation",
        paused_at,
        |d| {
            d["preparing_playback"] == false
                && d["prepared_pictures"].is_null()
                && d["sound_active"] == false
                && d["retiring"] == 0
        },
    )?;
    let cancelled_ms = (cancelled["unix_us"]
        .as_u64()
        .ok_or("Missing cancellation time")?
        - paused_at) as f64
        / 1000.;
    if cancelled_ms > 2000.
        || Path::new(&format!("/proc/{stopped_pid}")).exists()
        || !cancelled["details"]["sound"].is_null()
    {
        return Err(
            "Preparing cancellation retained work, started sound or exceeded two seconds".into(),
        );
    }
    picture(&mut trace, 0, paused_at)?;
    let active = play_sound(&mut trace)?;
    let paused = pause_sound(&mut trace)?;
    let bookmark = paused["details"]["resume_sound"]["Sample"]["position"]
        .as_u64()
        .ok_or("Exact sample bookmark absent")?;
    let resumed = play_sound(&mut trace)?;
    if resumed["details"]["sound"]["start_sample"] != bookmark {
        return Err("Prepared playback rounded its resumed sample boundary".into());
    }
    pause_sound(&mut trace)?;
    let mut faults = Vec::new();
    for (signal, name) in [("-KILL", "worker-death"), ("-STOP", "picture-underflow")] {
        set_frame(&mut trace, 0)?;
        let playing = play_sound(&mut trace)?;
        let pid = playing["details"]["worker_pid"]
            .as_u64()
            .ok_or("Playing picture worker PID absent")?;
        let after = now();
        command("kill", &[signal, &pid.to_string()])?;
        let failed = sound_record(&mut trace, name, after, |d| {
            d["sound_active"] == false
                && d["preparing_playback"] == false
                && d["sound_retiring"] == 0
                && d["retiring"] == 0
                && d["error"].is_string()
        })?;
        let elapsed_ms =
            (failed["unix_us"].as_u64().ok_or("Missing fault time")? - after) as f64 / 1000.;
        if elapsed_ms > 2000. || Path::new(&format!("/proc/{pid}")).exists() {
            return Err(
                "Prepared picture failure retained its codec or exceeded two seconds".into(),
            );
        }
        if signal == "-STOP"
            && !failed["details"]["error"]
                .as_str()
                .unwrap_or_default()
                .contains("Pictures could not keep up")
        {
            return Err("Stalled picture codec did not expose a picture underflow".into());
        }
        trace.focus()?;
        command(
            "grim",
            &[
                "-g",
                "80,80 1440x900",
                directory.join(format!("{name}.png")).to_str().unwrap(),
            ],
        )?;
        let was_stopped = failed["details"]["stopped"] == true;
        faults.push(
            json!({"signal":signal,"playing":playing,"failed":failed,"retirement_ms":elapsed_ms}),
        );
        if was_stopped {
            click_control(&mut trace, "retry-viewer")?;
        }
        set_frame(&mut trace, 0)?;
    }
    let retry = play_sound(&mut trace)?;
    let retry_paused = pause_sound(&mut trace)?;
    trace.read()?;
    let mut pids = std::collections::BTreeSet::from([u64::from(app.0.id())]);
    for record in &trace.records {
        let d = &record["details"];
        for pid in [
            d["worker_pid"].as_u64(),
            d["sound"]["worker_pid"].as_u64(),
            d["sound"]["device_worker_pid"].as_u64(),
        ]
        .into_iter()
        .flatten()
        {
            pids.insert(pid);
        }
    }
    app.kill()?;
    let reaped_by = Instant::now() + Duration::from_secs(2);
    while pids
        .iter()
        .any(|pid| Path::new(&format!("/proc/{pid}")).exists())
    {
        if Instant::now() >= reaped_by {
            return Err("Prepared trial retained an owned child".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    if hash(&original)? != original_hash || load(&copy)? != seed {
        return Err("Prepared playback changed its saved project".into());
    }
    for asset in &seed.assets {
        if hash(&asset.path)? != asset.sha256 {
            return Err("Prepared playback changed source bytes".into());
        }
    }
    let receipt = json!({"kind":"native_prepared_picture_faults","application_sha256":hash(&binary)?,
        "original_project_sha256":original_hash,"first":first,"preparing":preparing,"cancelled":cancelled,
        "preparing_cancellation_ms":cancelled_ms,"active":active,"paused":paused,"resumed":resumed,
        "faults":faults,"retry":retry,"retry_paused":retry_paused,"exact_sample_resume":true,
        "source_project_preserved":true,"owned_processes_reaped":true,"qualified":true,
        "limits":["Native software/device-callback receipts; no physical audibility or hardware drift measurement"]});
    File::create_new(directory.join("qualification.json"))?
        .write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    Ok(receipt)
}
