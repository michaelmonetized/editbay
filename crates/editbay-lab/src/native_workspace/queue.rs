use super::*;

/// Exercise queued cancellation and automatic execution through the native UI.
/// `binary`, `project` and new `directory` select the real app and saved content.
/// Returns actual codec retirement and independently decoded range receipts.
pub fn run(binary: &Path, project: &Path, directory: &Path) -> Result<Value> {
    let binary = binary.canonicalize()?;
    let project = project.canonicalize()?;
    let original = hash(&project)?;
    let seed = load(&project)?;
    fs::create_dir(directory)?;
    let directory = directory.canonicalize()?;
    let catalog = directory.join("catalog");
    fs::create_dir(&catalog)?;
    let copy = directory.join("Queue.editbay");
    save_new(&seed, &copy)?;
    let (mut app, mut trace) = start(
        &binary,
        &directory.join("state"),
        &catalog,
        &directory.join("native.jsonl"),
        Some(&copy),
    )?;
    picture(&mut trace, 0, 0)?;
    let blocked_path = directory.join("Blocked.mov");
    let cancelled_path = directory.join("Cancelled.mov");
    let next_path = directory.join("Next.mov");
    let after = now();
    choose_master(&mut trace, &blocked_path)?;
    let first = export_job(&mut trace, after, |job| {
        job["running"] == true && job["worker_pid"].as_u64().is_some()
    })?;
    let pid = first["worker_pid"].as_u64().ok_or("Queue worker absent")?;
    let id = first["id"].as_str().ok_or("Queue job absent")?;
    command("kill", &["-STOP", &pid.to_string()])?;
    click_control(&mut trace, "close-exports")?;
    let after = now();
    ranges::choose(&mut trace, &cancelled_path)?;
    let second = export_job(&mut trace, after, |job| job["queued"] == true)?;
    if !second["worker_pid"].is_null() || second["running"] != false {
        return Err("Queued export spawned a competing worker".into());
    }
    let second_id = second["id"].as_str().ok_or("Queued job absent")?;
    let after = click_control(&mut trace, &format!("cancel-export:{second_id}"))?;
    let cancelled = export_job(&mut trace, after, |job| {
        job["id"] == second_id && job["queued"] == false && job["cancellation"].as_str().is_some()
    })?;
    if cancelled_path.exists() || !Path::new(&format!("/proc/{pid}")).exists() {
        return Err("Queued cancellation published a file or stopped the active job".into());
    }
    click_control(&mut trace, "close-exports")?;
    let after = now();
    ranges::choose(&mut trace, &next_path)?;
    let queued = export_job(&mut trace, after, |job| job["queued"] == true)?;
    let next_id = queued["id"].as_str().ok_or("Next queued job absent")?;
    let after = click_control(&mut trace, &format!("cancel-export:{id}"))?;
    let next = export_job(&mut trace, after, |job| {
        job["id"] == next_id && job["running"] == false && job["queued"] == false
    })?;
    if next["receipt"].is_null()
        || blocked_path.exists()
        || cancelled_path.exists()
        || Path::new(&format!("/proc/{pid}")).exists()
    {
        return Err(format!("Queue failed to reap then publish: {next}").into());
    }
    let receipt: editbay_delivery::Receipt = serde_json::from_value(next["receipt"].clone())?;
    if receipt.profile.first_frame != 2
        || receipt.profile.frames != 5
        || receipt.version != editbay_core::DocumentVersion::of(&seed)
    {
        return Err("Queued export changed its frame range or captured version".into());
    }
    let independent = crate::shared_delivery::inspect(&next_path, &receipt)?;
    if hash(&project)? != original || load(&copy)? != seed {
        return Err("Native queue changed original project content".into());
    }
    app.kill()?;
    let result = json!({"kind":"native_export_queue","qualified":true,"application_sha256":hash(&binary)?,"first":first,"queued_cancel":cancelled,"queued_next":queued,"completed":next,"independent":independent,"interrupted_worker_reaped":true,"original_unchanged":true,"limits":["Actual native windows, choosers and codec workers with software-injected input; no independent-user approval"]});
    File::create_new(directory.join("qualification.json"))?
        .write_all(&serde_json::to_vec_pretty(&result)?)?;
    Ok(result)
}
