use crate::{Result, hash, metrics};
use editbay_core::{Project, load, recovery_catalog, save_new};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Application(Child);

impl Application {
    fn kill(&mut self) -> Result<()> {
        self.0.kill()?;
        self.0.wait()?;
        Ok(())
    }
}

impl Drop for Application {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Trace {
    path: PathBuf,
    offset: u64,
    records: Vec<Value>,
}

impl Trace {
    fn read(&mut self) -> Result<()> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(self.offset))?;
        loop {
            let mut line = String::new();
            let length = reader.read_line(&mut line)?;
            if length == 0 || !line.ends_with('\n') {
                break;
            }
            let record: Value = serde_json::from_str(&line)?;
            if record["dropped_before"] != 0 {
                return Err("Native diagnostics dropped observations".into());
            }
            self.records.push(record);
            self.offset += length as u64;
        }
        Ok(())
    }

    fn wait(&mut self, description: &str, predicate: impl Fn(&Value) -> bool) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.read()?;
            if let Some(record) = self.records.iter().rev().find(|record| predicate(record)) {
                return Ok(record.clone());
            }
            if Instant::now() >= deadline {
                return Err(format!("Native UI did not acknowledge {description}").into());
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
}

fn command(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed ({}): {}{}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn dispatch(action: &str) -> Result<()> {
    let result = command("hyprctl", &["dispatch", action])?;
    if result.trim() != "ok" {
        return Err(format!("Native window operation failed: {result}").into());
    }
    Ok(())
}

fn window(predicate: impl Fn(&Value) -> bool) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let clients: Value = serde_json::from_str(&command("hyprctl", &["clients", "-j"])?)?;
        if let Some(window) = clients
            .as_array()
            .ok_or("Invalid native window inventory")?
            .iter()
            .find(|window| predicate(window))
        {
            return Ok(window.clone());
        }
        if Instant::now() > deadline {
            return Err("Expected native window did not appear".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn click(x: i64, y: i64) -> Result<()> {
    dispatch(&format!("hl.dsp.cursor.move({{x={x},y={y}}})"))?;
    thread::sleep(Duration::from_millis(25));
    command("ydotool", &["click", "0xC0"])?;
    Ok(())
}

/// Inject one native key through the persistent Linux input device.
/// `code` is a US Linux key code; `control` and `shift` hold the named modifiers.
/// Returns only after every requested press and release has been submitted.
fn key(code: u16, control: bool, shift: bool) -> Result<()> {
    let mut arguments = vec!["key".to_owned(), "-d".to_owned(), "2".to_owned()];
    if control {
        arguments.push("29:1".into());
    }
    if shift {
        arguments.push("42:1".into());
    }
    arguments.extend([format!("{code}:1"), format!("{code}:0")]);
    if shift {
        arguments.push("42:0".into());
    }
    if control {
        arguments.push("29:0".into());
    }
    command(
        "ydotool",
        &arguments.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    Ok(())
}

fn start(
    binary: &Path,
    state: &Path,
    catalog: &Path,
    trace: &Path,
    original: Option<&Path>,
) -> Result<(Application, Trace)> {
    let log = File::create_new(trace.with_extension("stderr.log"))?;
    let mut process = Command::new(binary);
    process
        .env("EDITBAY_STATE_DIR", state)
        .env("EDITBAY_CATALOG_ROOT", catalog)
        .env("EDITBAY_DIAGNOSTICS_PATH", trace)
        .stderr(Stdio::from(log.try_clone()?))
        .stdout(Stdio::from(log));
    if let Some(path) = original {
        process.arg(path);
    }
    let application = Application(process.spawn()?);
    let native =
        window(|window| window["pid"] == application.0.id() && window["class"] == "editbay")?;
    let address = native["address"]
        .as_str()
        .ok_or("Native window has no identity")?;
    if !address.starts_with("0x") || !address[2..].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid native window identity".into());
    }
    for action in [
        format!("hl.dsp.window.float({{action=\"on\",window=\"address:{address}\"}})"),
        format!(
            "hl.dsp.window.resize({{x=1440,y=900,relative=false,window=\"address:{address}\"}})"
        ),
        format!("hl.dsp.window.move({{x=80,y=80,relative=false,window=\"address:{address}\"}})"),
        format!("hl.dsp.focus({{window=\"address:{address}\"}})"),
    ] {
        dispatch(&action)?;
    }
    let active: Value = serde_json::from_str(&command("hyprctl", &["activewindow", "-j"])?)?;
    if active["pid"] != application.0.id() {
        return Err("The native application does not hold keyboard focus; release desktop input capture before running qualification".into());
    }
    thread::sleep(Duration::from_millis(350));
    let mut trace = Trace {
        path: trace.to_owned(),
        offset: 0,
        records: Vec::new(),
    };
    trace.wait("initial desktop typography", |record| {
        record["kind"] == "frame"
            && record["details"]["desktop_font_ready"] == true
            && record["details"]["cpu_us"]
                .as_u64()
                .is_some_and(|time| time < 20_000)
    })?;
    Ok((application, trace))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn has_tab(record: &Value, name: &str, revision: u64) -> bool {
    record["kind"] == "workspace"
        && record["details"]["tabs"].as_array().is_some_and(|tabs| {
            tabs.iter()
                .any(|tab| tab["name"] == name && tab["revision"] == revision)
        })
}

fn accept(trace: &mut Trace, name: &str, revision: u64, latencies: &mut Vec<f64>) -> Result<Value> {
    let sent = now();
    key(28, false, false)?;
    let record = trace.wait(name, |record| {
        record["unix_us"].as_u64().is_some_and(|time| time >= sent)
            && has_tab(record, name, revision)
    })?;
    latencies.push(
        (record["unix_us"]
            .as_u64()
            .ok_or("Missing native action timestamp")?
            - sent) as f64
            / 1000.,
    );
    Ok(record)
}

fn create(trace: &mut Trace, name: &str, latencies: &mut Vec<f64>) -> Result<Value> {
    let requested = now();
    key(49, true, false)?;
    trace.wait("new-project name field", |record| {
        record["unix_us"]
            .as_u64()
            .is_some_and(|time| time >= requested)
            && record["kind"] == "frame"
            && record["details"]["new_project_name"].as_str().is_some()
    })?;
    key(30, true, false)?;
    command("wtype", &["-s", "40", name, "-s", "80"])?;
    trace.wait("typed project name", |record| {
        record["unix_us"]
            .as_u64()
            .is_some_and(|time| time >= requested)
            && record["kind"] == "frame"
            && record["details"]["new_project_name"] == name
    })?;
    accept(trace, name, 0, latencies)
}

fn rename(trace: &mut Trace, name: &str, revision: u64, latencies: &mut Vec<f64>) -> Result<Value> {
    let requested = now();
    click(286, 205)?;
    trace.wait("rename name field", |record| {
        record["unix_us"]
            .as_u64()
            .is_some_and(|time| time >= requested)
            && record["kind"] == "frame"
            && record["details"]["rename_name"].as_str().is_some()
    })?;
    key(30, true, false)?;
    command("wtype", &["-s", "40", name, "-s", "80"])?;
    trace.wait("typed revised name", |record| {
        record["unix_us"]
            .as_u64()
            .is_some_and(|time| time >= requested)
            && record["kind"] == "frame"
            && record["details"]["rename_name"] == name
    })?;
    accept(trace, name, revision, latencies)
}

/// Measure real idle and continuous-edit recovery through the native window.
/// `binary` is the compiled app and `directory` a new evidence folder. Returns
/// actual durable-publication timings and fails if either scheduling gate misses.
pub fn timing(binary: &Path, directory: &Path) -> Result<Value> {
    let binary = binary.canonicalize()?;
    fs::create_dir(directory)?;
    let catalog = directory.join("catalog");
    fs::create_dir(&catalog)?;
    let state = directory.join("state");
    let (mut application, mut trace) = start(
        &binary,
        &state,
        &catalog,
        &directory.join("native.jsonl"),
        None,
    )?;
    let mut latencies = Vec::new();
    let first = create(&mut trace, "Timing 000 active", &mut latencies)?;
    let first_time = first["unix_us"].as_u64().ok_or("Missing dirty timestamp")?;
    let mut last_time = first_time;
    let mut revision = 0;
    let mut gaps = Vec::new();
    while now() - first_time < 11_500_000 {
        revision += 1;
        let name = format!("Timing {revision:03} active");
        let record = rename(&mut trace, &name, revision, &mut latencies)?;
        let time = record["unix_us"].as_u64().ok_or("Missing edit timestamp")?;
        gaps.push((time - last_time) as f64 / 1000.);
        last_time = time;
        thread::sleep(Duration::from_millis(200));
    }
    let final_record = trace.wait("idle publication of the latest revision", |record| {
        record["kind"] == "workspace"
            && record["details"]["tabs"].as_array().is_some_and(|tabs| {
                tabs.iter()
                    .any(|tab| tab["revision"] == revision && tab["recovery_revision"] == revision)
            })
    })?;
    let first_checkpoint = trace
        .records
        .iter()
        .find(|record| {
            record["kind"] == "workspace"
                && record["details"]["tabs"].as_array().is_some_and(|tabs| {
                    tabs.iter()
                        .any(|tab| tab["recovery_revision"].as_u64().is_some())
                })
        })
        .ok_or("No continuous-edit checkpoint was published")?;
    let continuous_ms = (first_checkpoint["unix_us"]
        .as_u64()
        .ok_or("Missing publication timestamp")?
        - first_time) as f64
        / 1000.;
    let idle_ms = (final_record["unix_us"]
        .as_u64()
        .ok_or("Missing idle timestamp")?
        - last_time) as f64
        / 1000.;
    let intervals = metrics(&mut gaps);
    let pass = continuous_ms <= 10_000.
        && (1_000. ..=1_250.).contains(&idle_ms)
        && intervals["max_ms"].as_f64().is_some_and(|gap| gap < 1_000.);
    application.kill()?;
    let receipt = json!({"schema":1,"kind":"native_recovery_timing","application_sha256":hash(&binary)?,"revisions":revision,"first_continuous_publication_ms":continuous_ms,"latest_idle_publication_ms":idle_ms,"edit_intervals":intervals,"input_injection_to_ui_acceptance":metrics(&mut latencies),"pass":pass,"first_publication":first_checkpoint,"last_publication":final_record,"limits":["Observed UI acknowledgements include event scheduling and durable filesystem publication","The idle gate measures the 1 s debounce plus up to 250 ms for worker/UI publication; continuous edits require publication within 10 s"]});
    File::create_new(directory.join("timing.json"))?
        .write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    if !pass {
        return Err(
            "Native idle or maximum-delay recovery gate failed; inspect timing.json".into(),
        );
    }
    Ok(receipt)
}

/// Exercise native recovery failures on real permission-denied and full filesystems.
/// `binary` selects the app and `directory` a new receipt folder. Returns the two
/// actual error/retry receipts; the full tmpfs exists only in a private namespace.
pub fn errors(binary: &Path, directory: &Path) -> Result<Value> {
    let binary = binary.canonicalize()?;
    fs::create_dir(directory)?;
    let permission = error_trial(&binary, &directory.join("permission"), false)?;
    let full = directory.join("full");
    let output = Command::new("unshare")
        .args(["--user", "--map-root-user", "--mount", "--"])
        .arg(std::env::current_exe()?)
        .arg("native-error-worker")
        .arg(&binary)
        .arg(&full)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "Private full-filesystem qualification failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let full: Value = serde_json::from_slice(&output.stdout)?;
    let receipt = json!({"schema":1,"kind":"native_storage_errors","application_sha256":hash(&binary)?,"permission":permission,"disk_full":full,"pass":true});
    File::create_new(directory.join("errors.json"))?
        .write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    Ok(receipt)
}

/// Run a filesystem failure trial without simulating application results.
/// `binary`, `directory` and `full` select the app, owned evidence and a private
/// 4 KiB tmpfs. Returns the native error acknowledgement and successful retry.
pub fn error_trial(binary: &Path, directory: &Path, full: bool) -> Result<Value> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir(directory)?;
    let catalog = directory.join("catalog");
    fs::create_dir(&catalog)?;
    let state = directory.join("state");
    let recovery = state.join("recovery");
    fs::create_dir_all(&recovery)?;
    if full {
        let result = Command::new("mount")
            .args(["-t", "tmpfs", "-o", "size=4096,mode=700", "tmpfs"])
            .arg(&recovery)
            .output()?;
        if !result.status.success() {
            return Err(format!(
                "Private tmpfs mount failed: {}",
                String::from_utf8_lossy(&result.stderr)
            )
            .into());
        }
        let mut filler = File::create_new(recovery.join(".full"))?;
        filler.write_all(&[0; 4096])?;
        filler.sync_all()?;
    } else {
        fs::set_permissions(&recovery, fs::Permissions::from_mode(0o500))?;
    }
    let (mut application, mut trace) = start(
        binary,
        &state,
        &catalog,
        &directory.join("native.jsonl"),
        None,
    )?;
    let mut latencies = Vec::new();
    create(&mut trace, "Storage failure QA", &mut latencies)?;
    let expected = if full { "os error 28" } else { "os error 13" };
    let failure = trace.wait("visible recovery filesystem failure", |record| {
        record["kind"] == "workspace"
            && record["details"]["tabs"].as_array().is_some_and(|tabs| {
                tabs.iter().any(|tab| {
                    tab["dirty"] == true
                        && tab["recovery_revision"].is_null()
                        && tab["recovery_error"]
                            .as_str()
                            .is_some_and(|error| error.contains(expected))
                })
            })
    })?;
    let native =
        window(|window| window["pid"] == application.0.id() && window["class"] == "editbay")?;
    let region = format!(
        "{},{} {}x{}",
        native["at"][0], native["at"][1], native["size"][0], native["size"][1]
    );
    command(
        "grim",
        &[
            "-g",
            &region,
            directory
                .join("visible-error.png")
                .to_str()
                .ok_or("Evidence path needs UTF-8")?,
        ],
    )?;
    if !recovery_catalog(&recovery)?.valid.is_empty() {
        return Err("Failed recovery falsely published a checkpoint".into());
    }
    if full {
        fs::remove_file(recovery.join(".full"))?;
    } else {
        fs::set_permissions(&recovery, fs::Permissions::from_mode(0o700))?;
    }
    let retry = trace.wait("successful recovery retry", |record| {
        record["kind"] == "workspace"
            && record["details"]["tabs"].as_array().is_some_and(|tabs| {
                tabs.iter()
                    .any(|tab| tab["recovery_revision"] == 0 && tab["recovery_error"].is_null())
            })
    })?;
    let snapshots = recovery_catalog(&recovery)?;
    let snapshot = snapshots
        .valid
        .first()
        .ok_or("Retry acknowledged a missing checkpoint")?;
    let receipt = json!({"kind":if full { "real_enospc_private_tmpfs" } else { "real_eacces_owned_directory" },"error":failure,"retry":retry,"checkpoint_sha256":hash(&snapshot.path)?,"valid_checkpoints":snapshots.valid.len(),"false_checkpoint_acknowledgement":false,"input_injection_to_ui_acceptance":metrics(&mut latencies)});
    application.kill()?;
    File::create_new(directory.join("error.json"))?
        .write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    Ok(receipt)
}

fn recover(trace: &mut Trace, folder: &Path) -> Result<()> {
    click(240, 224)?;
    trace.wait("recovery catalog", |record| {
        record["kind"] == "frame"
            && record["details"]["recovered"] == true
            && record["details"]["recoveries_valid"]
                .as_u64()
                .is_some_and(|count| count >= 2)
    })?;
    click(380, 307)?;
    trace.wait("recovery preview", |record| {
        record["kind"] == "frame" && record["details"]["recovery_preview"] == true
    })?;
    click(665, 579)?;
    let chooser = window(|window| {
        window["class"] == "org.omarchy.synchro"
            && window["title"]
                .as_str()
                .is_some_and(|title| title.starts_with("Save EditBay project"))
    })?;
    let x = chooser["at"][0]
        .as_i64()
        .ok_or("Chooser has no native position")?;
    let y = chooser["at"][1]
        .as_i64()
        .ok_or("Chooser has no native position")?;
    let width = chooser["size"][0]
        .as_i64()
        .ok_or("Chooser has no native width")?;
    let height = chooser["size"][1]
        .as_i64()
        .ok_or("Chooser has no native height")?;
    thread::sleep(Duration::from_millis(150));
    key(38, true, false)?;
    thread::sleep(Duration::from_millis(100));
    key(30, true, false)?;
    command(
        "wtype",
        &[
            "-s",
            "40",
            folder.to_str().ok_or("Qualification path needs UTF-8")?,
            "-s",
            "80",
        ],
    )?;
    key(28, false, false)?;
    thread::sleep(Duration::from_millis(200));
    click(x + 300, y + height - 51)?;
    thread::sleep(Duration::from_millis(80));
    key(30, true, false)?;
    command("wtype", &["-s", "40", "Recovered.editbay", "-s", "80"])?;
    click(x + width - 40, y + height - 18)?;
    let destination = folder.join("Recovered.editbay");
    trace.wait("recovered copy open", |record| {
        record["kind"] == "workspace"
            && record["details"]["tabs"].as_array().is_some_and(|tabs| {
                tabs.iter().any(|tab| {
                    tab["path"].as_str() == destination.to_str() && tab["dirty"] == false
                })
            })
    })?;
    Ok(())
}

/// Exercise the real native workspace through process kills and UI recovery.
/// `binary` selects the built application, `directory` must be new, and `count`
/// is 1–100 trials. Returns receipts from actual keyboard/window/file-dialog paths.
pub fn run(binary: &Path, directory: &Path, count: usize) -> Result<Value> {
    if !(1..=100).contains(&count) {
        return Err("Native qualification requires 1–100 trials".into());
    }
    let binary = binary.canonicalize()?;
    fs::create_dir(directory)?;
    let directory = directory.canonicalize()?;
    let catalog = directory.join("Client projects");
    fs::create_dir(&catalog)?;
    for client in ["Client A", "Client B"] {
        let folder = catalog.join(client);
        fs::create_dir(&folder)?;
        fs::create_dir(folder.join(".omabrand"))?;
        for index in 0..2000 {
            save_new(
                &Project::new(format!("{client} work {index}"))?,
                folder.join(format!("{index}.editbay")),
            )?;
        }
    }
    println!("Native qualification: 4,000 real catalog documents ready");
    let mut reports = Vec::new();
    let mut latencies = Vec::new();
    let mut frames = Vec::new();
    let mut commits = Vec::new();
    for index in 0..count {
        let folder = directory.join(format!("trial-{index:03}"));
        fs::create_dir(&folder)?;
        let state = folder.join("state");
        let inactive = format!("Trial {index:03} inactive");
        let active = format!("Trial {index:03} active");
        let revised = format!("Trial {index:03} revised");
        let original = folder.join("Original.editbay");
        let saved = index % 2 != 0;
        let original_hash = if saved {
            save_new(&Project::new(&active)?, &original)?;
            Some(hash(&original)?)
        } else {
            None
        };
        let trace_path = folder.join("before-kill.jsonl");
        let (mut application, mut trace) = start(
            &binary,
            &state,
            &catalog,
            &trace_path,
            saved.then_some(original.as_path()),
        )?;
        if saved {
            trace.wait("original project open", |record| {
                has_tab(record, &active, 0)
            })?;
        }
        create(&mut trace, &inactive, &mut latencies)?;
        if saved {
            let selected = now();
            click(180, 139)?;
            trace.wait("saved original tab active", |record| {
                record["unix_us"]
                    .as_u64()
                    .is_some_and(|time| time >= selected)
                    && record["kind"] == "workspace"
                    && record["details"]["tabs"].as_array().is_some_and(|tabs| {
                        tabs.iter().any(|tab| {
                            tab["name"] == active && tab["session"] == record["details"]["active"]
                        })
                    })
            })?;
        } else {
            create(&mut trace, &active, &mut latencies)?;
        }
        rename(&mut trace, &revised, 1, &mut latencies)?;
        key(44, true, false)?;
        trace.wait("native undo", |record| has_tab(record, &active, 2))?;
        key(44, true, true)?;
        trace.wait("native redo", |record| has_tab(record, &revised, 3))?;
        let acknowledged = trace.wait("active and inactive checkpoints", |record| {
            record["kind"] == "workspace"
                && record["details"]["tabs"].as_array().is_some_and(|tabs| {
                    tabs.len() == 2
                        && tabs
                            .iter()
                            .all(|tab| tab["recovery_revision"] == tab["revision"])
                })
        })?;
        let pid = application.0.id();
        let memory = fs::read_to_string(format!("/proc/{pid}/status"))?
            .lines()
            .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        application.kill()?;
        trace.read()?;
        let snapshots = recovery_catalog(state.join("recovery"))?;
        let selected = snapshots
            .valid
            .iter()
            .find(|record| record.name == revised && record.revision == 3)
            .ok_or("Latest native checkpoint did not survive the process kill")?;
        let inactive_checkpoint = snapshots
            .valid
            .iter()
            .find(|record| record.name == inactive && record.revision == 0)
            .ok_or("Inactive native checkpoint did not survive the process kill")?;
        let active_id = selected.project_id;
        let checkpoint_hash = hash(&selected.path)?;
        let inactive_hash = hash(&inactive_checkpoint.path)?;
        if let Some(expected) = &original_hash
            && hash(&original)? != *expected
        {
            return Err("Native recovery changed the saved original".into());
        }
        let restarted_path = folder.join("after-kill.jsonl");
        let (mut restarted, mut restarted_trace) =
            start(&binary, &state, &catalog, &restarted_path, None)?;
        recover(&mut restarted_trace, &folder)?;
        let recovered_path = folder.join("Recovered.editbay");
        let recovered = load(&recovered_path)?;
        if recovered.name != revised
            || recovered.revision != 3
            || recovered.id == active_id
            || recovered.recovered_from != Some(active_id)
        {
            return Err(
                "Native UI recovery lost the acknowledged revision or separate-copy identity"
                    .into(),
            );
        }
        if hash(&selected.path)? != checkpoint_hash
            || hash(&inactive_checkpoint.path)? != inactive_hash
        {
            return Err("Native UI recovery modified immutable checkpoints".into());
        }
        if let Some(expected) = &original_hash
            && hash(&original)? != *expected
        {
            return Err("Recover-copy modified the saved original".into());
        }
        restarted.kill()?;
        let (mut reopened, mut reopen_trace) = start(
            &binary,
            &state,
            &catalog,
            &folder.join("reopened.jsonl"),
            Some(&recovered_path),
        )?;
        reopen_trace.wait("recovered file reopen", |record| {
            has_tab(record, &revised, 3)
        })?;
        reopened.kill()?;
        for record in &trace.records {
            if record["kind"] == "frame"
                && let Some(cpu) = record["details"]["cpu_us"].as_u64()
            {
                frames.push(cpu as f64 / 1000.);
            }
            if record["kind"] == "workspace"
                && let Some(time) = record["details"]["last_recovery_commit_us"].as_u64()
            {
                commits.push(time as f64 / 1000.);
            }
        }
        let report = json!({"index":index,"saved_original":saved,"original_sha256":original_hash,"checkpoint_sha256":checkpoint_hash,"inactive_checkpoint_sha256":inactive_hash,"recovered_sha256":hash(&recovered_path)?,"original_project":active_id,"recovered_project":recovered.id,"revision":recovered.revision,"checkpoint_acknowledged_unix_us":acknowledged["unix_us"],"memory":memory,"native_ui_recovery":true,"native_reopen":true});
        File::create_new(folder.join("receipt.json"))?
            .write_all(&serde_json::to_vec_pretty(&report)?)?;
        reports.push(report);
        if (index + 1) % 10 == 0 || count < 10 {
            println!(
                "Native qualification: {}/{} kill/recover/reopen trials passed",
                index + 1,
                count
            );
        }
    }
    let input = metrics(&mut latencies);
    let receipt = json!({"schema":1,"kind":"native_workspace_qualification","application_sha256":hash(&binary)?,"architecture":std::env::consts::ARCH,"desktop":"Hyprland Wayland / Synchro portal","catalog_documents":4000,"trials":count,"passed":reports.len(),"latest_revision":3,"input_injection_to_ui_acceptance":input,"cpu_frame_work":metrics(&mut frames),"checkpoint_main_thread_commit":metrics(&mut commits),"input_gate_pass":input["p95_ms"].as_f64().is_some_and(|latency|latency <= 50.),"reports":reports,"limits":["Software-injected Wayland text/portal keys and persistent Linux keyboard/pointer events; no physical-device input measurement","Native UI recovery uses the installed Synchro file chooser","These fixtures qualify local workspace behavior, not completed client edits or media playback"]});
    File::create_new(directory.join("qualification.json"))?
        .write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    Ok(receipt)
}
