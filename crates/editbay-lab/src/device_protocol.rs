use crate::Result;
use editbay_media::{Cancellation, native_job::NativeJob};
use serde_json::{Value, json};
use std::{
    fs::File,
    path::Path,
    time::{Duration, Instant},
};
use uuid::Uuid;

/// Reject invalid operations on the actual packaged sound device endpoint.
/// `project` and `composition` bind saved content. Returns protocol rejection,
/// unchanged project identity and bounded process retirement observations.
pub fn run(project: &Path, composition: &str) -> Result<Value> {
    let original = crate::hash(project)?;
    let project_data = editbay_core::load(project)?;
    let mut reports = Vec::new();
    for mode in [
        "malformed",
        "unknown-field",
        "poll-extra-field",
        "poll-before-begin",
        "foreign-begin-version",
        "nil-session",
        "begin-file",
        "stale",
        "foreign",
        "foreign-version",
        "extra-file",
        "duplicate-begin",
    ] {
        let cancel = Cancellation::new()?;
        let file = File::open(project)?;
        let mut child =
            NativeJob::<Value>::start(&std::env::current_exe()?, "--sound-device-worker")?;
        let pid = child.process_id();
        let owner = json!({"session":Uuid::new_v4(),"version":editbay_core::DocumentVersion::of(&project_data)});
        let begin = json!({"owner":owner,"serial":0,"operation":{"kind":"begin","project":project_data,"composition":composition,"start":{"Frame":u64::MAX},"route":"Stereo"}});
        let (request, descriptor) = match mode {
            "malformed" => (json!("invalid"), None),
            "unknown-field" => {
                let mut request = begin.clone();
                request["extra"] = json!(true);
                (request, None)
            }
            "poll-extra-field" => (
                json!({"owner":owner,"serial":0,"operation":{"kind":"poll","extra":true}}),
                None,
            ),
            "poll-before-begin" => (
                json!({"owner":owner,"serial":0,"operation":{"kind":"poll"}}),
                None,
            ),
            "foreign-begin-version" => {
                let mut request = begin.clone();
                request["owner"]["version"]["revision"] = json!(project_data.revision + 1);
                (request, None)
            }
            "nil-session" => {
                let mut request = begin.clone();
                request["owner"]["session"] = json!(Uuid::nil());
                (request, None)
            }
            "begin-file" => (begin, Some(&file)),
            _ => {
                let reply = child.request_with_timeout(
                    &begin,
                    None,
                    &cancel,
                    Duration::from_millis(500),
                )?;
                if reply["owner"] != owner || reply["serial"] != 0 {
                    return Err("Device protocol could not bind".into());
                }
                let mut request = json!({"owner":owner,"serial":1,"operation":{"kind":"poll"}});
                match mode {
                    "stale" => request["serial"] = json!(0),
                    "foreign" => request["owner"]["session"] = json!(Uuid::new_v4()),
                    "foreign-version" => {
                        request["owner"]["version"]["revision"] = json!(project_data.revision + 1)
                    }
                    "duplicate-begin" => request["operation"] = begin["operation"].clone(),
                    _ => {}
                }
                (request, (mode == "extra-file").then_some(&file))
            }
        };
        let began = Instant::now();
        let reply =
            child.request_with_timeout(&request, descriptor, &cancel, Duration::from_millis(500));
        let rejected = reply.is_err();
        let detail = match reply {
            Ok(reply) => reply,
            Err(error) => json!(error.to_string()),
        };
        drop(child);
        let reaped = !Path::new(&format!("/proc/{pid}")).exists();
        if !rejected || !reaped || began.elapsed() > Duration::from_secs(2) {
            return Err(format!("Device protocol {mode} failed: {detail}").into());
        }
        reports.push(json!({"mode":mode,"rejected":rejected,"reaped":reaped,"elapsed_ms":began.elapsed().as_secs_f64()*1000.,"detail":detail}));
    }
    if crate::hash(project)? != original {
        return Err("Device protocol changed its source project".into());
    }
    Ok(
        json!({"kind":"sound_device_protocol","qualified":true,"project_sha256":original,"application_sha256":crate::hash(&std::env::current_exe()?)?,"reports":reports}),
    )
}

/// Reap only an explicitly owned orphan after a device/controller death trial.
/// `pid` identifies the recorded descendant and `deadline` bounds cleanup.
/// Returns after that process disappears; it never waits for unrelated children.
pub(crate) fn reap_orphan(pid: u32, deadline: Instant) -> Result<()> {
    let owned = rustix::process::Pid::from_raw(i32::try_from(pid)?).ok_or("Invalid owned PID")?;
    while Path::new(&format!("/proc/{pid}")).exists() {
        match rustix::process::waitpid(Some(owned), rustix::process::WaitOptions::NOHANG) {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(rustix::io::Errno::CHILD) if !Path::new(&format!("/proc/{pid}")).exists() => break,
            Err(error) => return Err(error.into()),
        }
        if Instant::now() >= deadline {
            return Err("Owned device descendant exceeded its retirement deadline".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

/// Run the real production supervisor in a separately killable lab controller.
/// `project`, `composition` and new `receipt` capture actual active child IDs.
/// Returns only on normal sound completion; the parent trial interrupts it.
pub fn parent_worker(project: &Path, composition: &str, receipt: &Path) -> Result<Value> {
    let mut sound = editbay_audio::StreamingPlayback::start(
        std::sync::Arc::new(editbay_core::load(project)?),
        composition.parse()?,
        editbay_audio::PlaybackStart::Frame(0),
        editbay_audio::MonitorRoute::Stereo,
    )?;
    let began = Instant::now();
    let mut published = false;
    loop {
        let status = sound.status();
        if !published && status.callbacks >= 10 {
            serde_json::to_writer(File::create_new(receipt)?, &status)?;
            published = true;
        }
        if sound.is_finished() {
            sound.reap();
            return Err(format!(
                "Controller completed before being interrupted: {:?}",
                sound.status()
            )
            .into());
        }
        if began.elapsed() > Duration::from_secs(30) {
            return Err("Controller preparation timed out".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Kill an active controller and observe both generations of actual child exit.
/// `project`, `composition` and new `directory` select saved playback and logs.
/// Returns SIGINT/SIGTERM/SIGKILL evidence with a two-second retirement gate.
pub fn parent(project: &Path, composition: &str, directory: &Path) -> Result<Value> {
    use std::process::{Command, Stdio};
    rustix::process::set_child_subreaper(Some(rustix::process::Pid::INIT))?;
    std::fs::create_dir(directory)?;
    let mut reports = Vec::new();
    for signal in ["INT", "TERM", "KILL"] {
        let receipt = directory.join(format!("{signal}.json"));
        let child = Command::new(std::env::current_exe()?)
            .arg("device-parent-worker")
            .arg(project)
            .arg(composition)
            .arg(&receipt)
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create_new(
                directory.join(format!("{signal}.log")),
            )?))
            .spawn()?;
        let mut parent = crate::Worker(child);
        let deadline = Instant::now() + Duration::from_secs(30);
        let active: Value = loop {
            if parent.0.try_wait()?.is_some() {
                return Err("Device controller died before observation".into());
            }
            if let Ok(bytes) = std::fs::read(&receipt)
                && let Ok(status) = serde_json::from_slice(&bytes)
            {
                break status;
            }
            if Instant::now() > deadline {
                return Err("Controller never reached callbacks".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let before = Instant::now();
        if !Command::new("kill")
            .args([&format!("-{signal}"), &parent.0.id().to_string()])
            .status()?
            .success()
        {
            return Err("Controller signal failed".into());
        }
        let deadline = before + Duration::from_secs(2);
        let exit = loop {
            if let Some(exit) = parent.0.try_wait()? {
                break exit;
            }
            if Instant::now() >= deadline {
                return Err("Device controller survived signal".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        for key in ["device_worker_pid", "worker_pid"] {
            reap_orphan(
                u32::try_from(active[key].as_u64().ok_or("Missing active child PID")?)?,
                deadline,
            )?;
        }
        if exit.success() {
            return Err("Interrupted controller reported success".into());
        }
        reports.push(json!({"signal":signal,"active":active,"exit":exit.to_string(),"retirement_ms":before.elapsed().as_secs_f64()*1000.,"device_and_pcm_reaped":true}));
    }
    Ok(
        json!({"kind":"sound_device_controller_death","qualified":true,"application_sha256":crate::hash(&std::env::current_exe()?)?,"reports":reports}),
    )
}
