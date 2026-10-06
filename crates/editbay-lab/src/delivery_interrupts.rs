use crate::{Result, hash};
use rustix::process::{Pid, WaitOptions, set_child_subreaper, waitpid};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Interrupt real CLI exports, including abrupt controller death.
/// `executable`, `project`, `composition` and new `directory` select the trial.
/// Returns actual signal, cleanup and reap timings; all media stays private.
pub fn run(
    executable: &Path,
    project: &Path,
    composition: &str,
    directory: &Path,
) -> Result<Value> {
    set_child_subreaper(Some(Pid::INIT))?;
    fs::create_dir(directory)?;
    let mut reports = Vec::new();
    for signal in ["INT", "TERM", "KILL"] {
        let output = directory.join(format!("{signal}.mov"));
        let log = directory.join(format!("{signal}.log"));
        let child = Command::new(executable)
            .arg("export")
            .arg(project)
            .arg(composition)
            .arg(&output)
            .stdout(Stdio::from(File::create_new(
                directory.join(format!("{signal}.json")),
            )?))
            .stderr(Stdio::from(File::create_new(&log)?))
            .spawn()?;
        let mut parent = crate::Worker(child);
        let started = Instant::now();
        let pid = loop {
            if parent.0.try_wait()?.is_some() {
                return Err("CLI ended before the interrupt trial".into());
            }
            let children = fs::read_to_string(format!(
                "/proc/{}/task/{}/children",
                parent.0.id(),
                parent.0.id()
            ))?;
            if fs::read_to_string(&log)?.contains("Rendering:")
                && let Some(pid) = children.split_whitespace().next()
            {
                break pid.parse::<u32>()?;
            }
            if started.elapsed() > Duration::from_secs(30) {
                return Err("CLI failed to start active rendering".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let before = Instant::now();
        if !Command::new("kill")
            .args([&format!("-{signal}"), &parent.0.id().to_string()])
            .status()?
            .success()
        {
            return Err("CLI interrupt failed".into());
        }
        let status = loop {
            if let Some(status) = parent.0.try_wait()? {
                break status;
            }
            if before.elapsed() > Duration::from_secs(2) {
                return Err("CLI interrupt exceeded two seconds".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        if signal == "KILL" {
            let owned = Pid::from_raw(i32::try_from(pid)?).ok_or("Invalid owned worker PID")?;
            loop {
                if waitpid(Some(owned), WaitOptions::NOHANG)?.is_some() {
                    break;
                }
                if before.elapsed() > Duration::from_secs(2) {
                    return Err("Parent death failed to retire the export worker".into());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let retired = !Path::new(&format!("/proc/{pid}")).exists();
        if status.success() || !retired || output.exists() {
            return Err("Interrupted export published output or retained a worker".into());
        }
        reports.push(json!({"signal":signal,"retirement_ms":before.elapsed().as_secs_f64()*1000.,"worker_reaped":retired,"published":false,"status":status.to_string()}));
    }
    let entries = fs::read_dir(directory)?.count();
    if entries != 6 {
        return Err("Interrupted exports left unexpected files".into());
    }
    Ok(
        json!({"kind":"shared_delivery_controller_interrupts","qualified":true,"application_sha256":hash(executable)?,"reports":reports,"directory_entries":entries,"unpublished_output_files":0}),
    )
}
