use super::*;

fn state(
    trace: &mut Trace,
    after: u64,
    description: &str,
    predicate: impl Fn(&Value) -> bool,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        trace.read()?;
        if let Some(record) = trace.records.iter().rev().find(|r| {
            r["kind"] == "preview"
                && r["unix_us"].as_u64().is_some_and(|us| us >= after)
                && predicate(&r["details"]["picture_cache"])
        }) {
            return Ok(record.clone());
        }
        if Instant::now() >= deadline {
            return Err(format!("Native cache did not acknowledge {description}").into());
        }
        thread::sleep(Duration::from_millis(2));
    }
}

pub(super) fn prepare(trace: &mut Trace) -> Result<Value> {
    let after = click_control(trace, "prepare-picture-cache")?;
    let completed = state(trace, after, "complete preparation", |s| {
        s["ready"].is_object() || s["error"].is_string()
    })?;
    if completed["details"]["picture_cache"]["error"].is_string() {
        return Err(format!(
            "Picture preparation failed: {}",
            completed["details"]["picture_cache"]["error"]
        )
        .into());
    }
    let cache = &completed["details"]["picture_cache"];
    if cache["ready"]["bytes"] != cache["usage"]["bytes"]
        || cache["ready"]["pictures"] != cache["usage"]["entries"]
    {
        return Err("Prepared storage does not retain its exact reservation".into());
    }
    trace.wait("prepared viewer restart", |r| {
        r["kind"] == "preview"
            && r["unix_us"].as_u64().is_some_and(|us| us >= after)
            && r["details"]["worker_pid"].is_number()
            && r["details"]["gpu_draw_completed_us"]
                .as_u64()
                .is_some_and(|us| us > 0)
    })?;
    Ok(completed)
}

pub(super) fn stalled(trace: &mut Trace) -> Result<(u64, Value)> {
    let after = click_control(trace, "prepare-picture-cache")?;
    let preparing = state(trace, after, "preparation codec", |s| {
        s["preparing"] == true && s["progress"]["worker_pid"].is_number()
    })?;
    let pid = preparing["details"]["picture_cache"]["progress"]["worker_pid"]
        .as_u64()
        .ok_or("Preparation PID absent")?;
    command("kill", &["-STOP", &pid.to_string()])?;
    Ok((pid, preparing))
}

pub(super) fn retired(trace: &mut Trace, after: u64, pid: u64, failed: bool) -> Result<Value> {
    let completed = state(trace, after, "cancelled or failed preparation", |s| {
        s["preparing"] == false
            && s["retiring"] == 0
            && s["ready"].is_null()
            && s["usage"]["bytes"] == 0
            && (!failed || s["error"].is_string())
    })?;
    let elapsed = (completed["unix_us"]
        .as_u64()
        .ok_or("Cancellation time absent")?
        - after) as f64
        / 1000.;
    if elapsed > 2000. || Path::new(&format!("/proc/{pid}")).exists() {
        return Err("Preparation did not retire its codec/storage within two seconds".into());
    }
    Ok(json!({"elapsed_ms":elapsed,"record":completed,"pid":pid,"child_reaped":true}))
}

pub(super) fn faults(trace: &mut Trace) -> Result<Value> {
    let (pid, preparing) = stalled(trace)?;
    let after = click_control(trace, "cancel-picture-cache")?;
    let cancelled = retired(trace, after, pid, false)?;
    let (pid, before_death) = stalled(trace)?;
    let after = now();
    command("kill", &["-KILL", &pid.to_string()])?;
    let failed = retired(trace, after, pid, true)?;
    Ok(
        json!({"preparing":preparing,"cancelled":cancelled,"before_death":before_death,"failed":failed}),
    )
}
