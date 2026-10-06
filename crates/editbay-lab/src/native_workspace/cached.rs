use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct Memory {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<Value>>,
}
impl Memory {
    pub(super) fn start(root: u32) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let token = stop.clone();
        let thread = thread::Builder::new().name("editbay-cache-memory".into()).spawn(move || {
            let mut observed = std::collections::BTreeSet::new();
            let mut peak = 0;
            let mut samples = 0;
            let mut truncated = false;
            while !token.load(Ordering::Acquire) {
                let mut pending = vec![root];
                let mut visited = std::collections::BTreeSet::new();
                let mut combined = 0u64;
                while let Some(pid) = pending.pop() {
                    if !visited.insert(pid) { continue; }
                    if visited.len() > 512 { truncated = true; break; }
                    if let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) {
                        observed.insert(pid);
                        combined += status.lines().find_map(|line| line.strip_prefix("VmRSS:")?.split_whitespace().next()?.parse::<u64>().ok()).unwrap_or(0);
                    }
                    if let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) {
                        for task in tasks.flatten() {
                            if let Ok(children) = fs::read_to_string(task.path().join("children")) {
                                pending.extend(children.split_whitespace().filter_map(|pid|pid.parse::<u32>().ok()));
                            }
                        }
                    }
                }
                if observed.len() > 512 { truncated = true; break; }
                samples += 1;
                peak = peak.max(combined);
                thread::park_timeout(Duration::from_millis(50));
            }
            json!({"sample_interval_ms":50,"samples":samples,"peak_combined_rss_kib":peak,"processes":observed,
                "truncated":truncated,"limits":"Sampled app/descendant RSS may double count shared pages; unmapped disk page cache and driver/device allocations excluded"})
        })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
    pub(super) fn finish(mut self) -> Result<Value> {
        self.stop.store(true, Ordering::Release);
        let thread = self.thread.take().ok_or("Memory sampler is absent")?;
        thread.thread().unpark();
        let receipt = thread.join().map_err(|_| "Memory sampler panicked")?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while receipt["processes"]
            .as_array()
            .ok_or("Observed process set absent")?
            .iter()
            .filter_map(Value::as_u64)
            .any(|pid| Path::new(&format!("/proc/{pid}")).exists())
        {
            if Instant::now() >= deadline {
                return Err("Native cache trial retained an observed process".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        if receipt["truncated"] != false
            || receipt["samples"].as_u64().is_none_or(|n| n == 0)
            || receipt["peak_combined_rss_kib"]
                .as_u64()
                .is_none_or(|n| n == 0)
        {
            return Err("Native memory evidence is incomplete".into());
        }
        Ok(receipt)
    }
}
impl Drop for Memory {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

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

fn attempt(trace: &mut Trace) -> Result<u64> {
    trace.read()?;
    trace
        .records
        .iter()
        .rev()
        .find(|r| r["kind"] == "preview")
        .and_then(|r| r["details"]["picture_cache"]["attempt"].as_u64())
        .ok_or_else(|| "Preparation attempt identity absent".into())
}

pub(super) fn prepare(trace: &mut Trace) -> Result<Value> {
    let previous = attempt(trace)?;
    let after = click_control(trace, "prepare-picture-cache")?;
    let completed = state(trace, after, "complete preparation", |s| {
        s["attempt"].as_u64().is_some_and(|id| id > previous)
            && (s["ready"].is_object() || s["error"].is_string())
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
    let previous = attempt(trace)?;
    let after = click_control(trace, "prepare-picture-cache")?;
    let preparing = state(trace, after, "preparation codec", |s| {
        s["attempt"].as_u64().is_some_and(|id| id > previous)
            && s["preparing"] == true
            && s["progress"]["worker_pid"].is_number()
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
