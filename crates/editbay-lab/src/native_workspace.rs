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
    fn focus(&self) -> Result<()> {
        let pid = self
            .records
            .first()
            .and_then(|record| record["pid"].as_u64())
            .ok_or("Missing owned application PID")?;
        let owned = window(|window| window["pid"] == pid && window["class"] == "editbay")?;
        let address = owned["address"]
            .as_str()
            .ok_or("Missing owned window address")?;
        if !address.starts_with("0x") || !address[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("Invalid owned window address".into());
        }
        dispatch(&format!("hl.dsp.focus({{window=\"address:{address}\"}})"))?;
        let observed = focused(pid)?;
        let mut receipt = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.with_extension("focus.jsonl"))?;
        writeln!(
            receipt,
            "{}",
            json!({"unix_us":now(),"owned_pid":pid,"observed_pid":observed["pid"],"keyboard_focus":true})
        )?;
        Ok(())
    }
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

fn focused(pid: u64) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let active: Value = serde_json::from_str(&command("hyprctl", &["activewindow", "-j"])?)?;
        if active["pid"] == pid {
            return Ok(active);
        }
        if Instant::now() >= deadline {
            return Err("Owned native window did not acquire keyboard focus".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn click(x: i64, y: i64) -> Result<()> {
    dispatch(&format!("hl.dsp.cursor.move({{x={x},y={y}}})"))?;
    thread::sleep(Duration::from_millis(125));
    command("ydotool", &["click", "-D", "8", "0xC0"])?;
    Ok(())
}

/// Inject one native key through the persistent Linux input device.
/// `code` is a US Linux key code; `control` and `shift` hold the named modifiers.
/// Returns only after every requested press and release has been submitted.
fn key(code: u16, control: bool, shift: bool) -> Result<()> {
    let mut arguments = vec!["key".to_owned(), "-d".to_owned(), "8".to_owned()];
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
    let native = window(|window| {
        window["pid"] == application.0.id()
            && window["class"] == "editbay"
            && window["mapped"] == true
    })?;
    let address = native["address"]
        .as_str()
        .ok_or("Native window has no identity")?;
    if !address.starts_with("0x") || !address[2..].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid native window identity".into());
    }
    for action in [
        format!(
            "hl.dsp.window.fullscreen_state({{internal=0,client=0,action=\"set\",layout_aware=false,window=\"address:{address}\"}})"
        ),
        format!("hl.dsp.window.float({{action=\"set\",window=\"address:{address}\"}})"),
        format!(
            "hl.dsp.window.resize({{x=1440,y=900,relative=false,window=\"address:{address}\"}})"
        ),
        format!("hl.dsp.window.move({{x=80,y=80,relative=false,window=\"address:{address}\"}})"),
        format!("hl.dsp.focus({{window=\"address:{address}\"}})"),
    ] {
        dispatch(&action)?;
    }
    focused(u64::from(application.0.id()))?;
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

fn choose_master(trace: &mut Trace, destination: &Path) -> Result<()> {
    click_control(trace, "export-sequence")?;
    let chooser = window(|window| {
        matches!(
            window["class"].as_str(),
            Some("org.omarchy.synchro" | "com.thisisgm.flea.picker")
        ) && window["mapped"] == true
            && window["title"]
                .as_str()
                .is_some_and(|title| title.starts_with("Export lossless 8-bit master"))
    })?;
    let address = chooser["address"]
        .as_str()
        .ok_or("Missing export chooser identity")?;
    dispatch(&format!("hl.dsp.focus({{window=\"address:{address}\"}})"))?;
    focused(chooser["pid"].as_u64().ok_or("Missing chooser PID")?)?;
    let folder = destination.parent().ok_or("Export folder missing")?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Export name must be UTF-8")?;
    let x = chooser["at"][0].as_i64().ok_or("Chooser x missing")?;
    let y = chooser["at"][1].as_i64().ok_or("Chooser y missing")?;
    if chooser["class"] == "com.thisisgm.flea.picker" {
        let pid = chooser["pid"].as_u64().unwrap().to_string();
        flea_folder(&pid, folder)?;
        let state: Value = serde_json::from_str(&command(
            "qs",
            &["ipc", "--pid", &pid, "call", "fleapicker", "saveState"],
        )?)?;
        flea_point(
            &pid,
            state["field"]
                .as_str()
                .ok_or("Export filename field missing")?,
        )?;
        key(30, true, false)?;
        command("wtype", &["-s", "40", name, "-s", "80"])?;
        key(28, false, false)?;
    } else {
        key(38, true, false)?;
        key(30, true, false)?;
        command(
            "wtype",
            &[
                "-s",
                "40",
                folder.to_str().ok_or("Export path must be UTF-8")?,
                "-s",
                "80",
            ],
        )?;
        key(28, false, false)?;
        thread::sleep(Duration::from_millis(200));
        let width = chooser["size"][0].as_i64().ok_or("Chooser width missing")?;
        let height = chooser["size"][1]
            .as_i64()
            .ok_or("Chooser height missing")?;
        click(x + 300, y + height - 51)?;
        key(30, true, false)?;
        command("wtype", &["-s", "40", name, "-s", "80"])?;
        click(x + width - 40, y + height - 18)?;
    }
    Ok(())
}

fn export_job(trace: &mut Trace, after: u64, predicate: impl Fn(&Value) -> bool) -> Result<Value> {
    let record = trace.wait("native export state", |record| {
        record["kind"] == "delivery"
            && record["unix_us"].as_u64().is_some_and(|time| time >= after)
            && record["details"]["jobs"]
                .as_array()
                .and_then(|jobs| jobs.last())
                .is_some_and(&predicate)
    })?;
    Ok(record["details"]["jobs"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone())
}

/// Qualify native export choice, cancellation, failure, retry and captured ownership.
/// `binary` is the real app, `project` is a saved sequence and `directory` is new
/// evidence storage. Returns actual window/process/file receipts and measured gates.
pub fn delivery(binary: &Path, project: &Path, directory: &Path) -> Result<Value> {
    let binary = binary.canonicalize()?;
    let original = project.canonicalize()?;
    let original_hash = hash(&original)?;
    let seed = load(&original)?;
    fs::create_dir(directory)?;
    let directory = directory.canonicalize()?;
    let catalog = directory.join("catalog");
    fs::create_dir(&catalog)?;
    let copy = directory.join("Export.editbay");
    save_new(&seed, &copy)?;
    let destination = directory.join("Master.mov");
    let (mut app, mut trace) = start(
        &binary,
        &directory.join("state"),
        &catalog,
        &directory.join("native.jsonl"),
        Some(&copy),
    )?;
    picture(&mut trace, 0, 0)?;
    let mut retired = Vec::new();
    let mut edits = Vec::new();
    let mut previous = None;
    for mode in ["cancel", "kill", "edit"] {
        let after = now();
        if let Some(id) = previous.as_ref() {
            click_control(&mut trace, &format!("retry-export:{id}"))?;
        } else {
            choose_master(&mut trace, &destination)?;
        }
        let running = export_job(&mut trace, after, |job| {
            job["running"] == true && job["worker_pid"].as_u64().is_some()
        })?;
        let pid = running["worker_pid"]
            .as_u64()
            .ok_or("Export child absent")?;
        let id = running["id"]
            .as_str()
            .ok_or("Export job absent")?
            .to_owned();
        command("kill", &["-STOP", &pid.to_string()])?;
        let cancelled_at = match mode {
            "cancel" => click_control(&mut trace, &format!("cancel-export:{id}"))?,
            "kill" => {
                let sent = now();
                command("kill", &["-KILL", &pid.to_string()])?;
                sent
            }
            _ => {
                let revision = seed.revision + 1;
                let edited = rename(&mut trace, "Revised export", revision, &mut edits)?;
                edited["unix_us"].as_u64().ok_or("Edit timestamp absent")?
            }
        };
        let done = export_job(&mut trace, cancelled_at, |job| {
            job["id"] == id && job["running"] == false && job["error"].as_str().is_some()
        })?;
        let ms = (now() - cancelled_at) as f64 / 1000.;
        if ms > 2000. || Path::new(&format!("/proc/{pid}")).exists() || destination.exists() {
            return Err(format!("Native {mode} failed retirement/publication gate").into());
        }
        retired.push(json!({"mode":mode,"retirement_ms":ms,"worker_reaped":true,"outcome":done}));
        previous = Some(id);
    }
    let after = click_control(&mut trace, &format!("retry-export:{}", previous.unwrap()))?;
    export_job(&mut trace, after, |job| {
        job["running"] == true && job["worker_pid"].as_u64().is_some()
    })?;
    let mut view_inputs = Vec::new();
    let mut viewed = Vec::new();
    for frame in [1, 0, 1, 0] {
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
            .ok_or("Viewer request time absent")?;
        view_inputs.push((accepted - sent) as f64 / 1000.);
        let active = trace
            .records
            .iter()
            .rev()
            .find(|record| {
                record["kind"] == "delivery"
                    && record["unix_us"]
                        .as_u64()
                        .is_some_and(|time| time <= accepted)
            })
            .is_some_and(|record| record["details"]["busy"] == true);
        viewed.push(json!({"frame":frame,"export_active_at_acceptance":active,"draw":drawn}));
    }
    let deadline = Instant::now() + Duration::from_secs(180);
    let completed = loop {
        trace.read()?;
        if let Some(job) = trace
            .records
            .iter()
            .rev()
            .filter(|record| {
                record["kind"] == "delivery"
                    && record["unix_us"].as_u64().is_some_and(|time| time >= after)
            })
            .find_map(|record| {
                record["details"]["jobs"]
                    .as_array()?
                    .last()
                    .filter(|job| job["running"] == false)
            })
        {
            if job["receipt"].is_null() {
                return Err(format!("Native export failed: {}", job["error"]).into());
            }
            break job.clone();
        }
        if Instant::now() >= deadline {
            return Err("Native export exceeded qualification deadline".into());
        }
        thread::sleep(Duration::from_millis(5));
    };
    if completed["receipt"]["version"]["revision"] != seed.revision + 1
        || hash(&destination)? != completed["receipt"]["file_sha256"]
    {
        return Err("Native retry exported the wrong document or file".into());
    }
    let saved_at = now();
    key(31, true, false)?;
    trace.wait("saved exported revision", |record| {
        has_tab(record, "Revised export", seed.revision + 1)
            && record["details"]["tabs"][0]["dirty"] == false
    })?;
    let preview = picture(&mut trace, 0, saved_at)?;
    trace.focus()?;
    thread::sleep(Duration::from_millis(100));
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("exported-native.png").to_str().unwrap(),
        ],
    )?;
    let reloaded = load(&copy)?;
    if reloaded.compositions != seed.compositions
        || reloaded.sources != seed.sources
        || hash(&original)? != original_hash
    {
        return Err("Native export changed original media/project content".into());
    }
    app.kill()?;
    let independent = crate::shared_delivery::inspect(
        &destination,
        &serde_json::from_value(completed["receipt"].clone())?,
    )?;
    let latency = metrics(&mut edits);
    let viewing = metrics(&mut view_inputs);
    let qualified = latency["p95_ms"].as_f64().is_some_and(|ms| ms <= 50.)
        && viewing["p95_ms"].as_f64().is_some_and(|ms| ms <= 50.)
        && viewed
            .iter()
            .any(|sample| sample["export_active_at_acceptance"] == true);
    let receipt = json!({"kind":"native_delivery_qualification","qualified":qualified,"application_sha256":hash(&binary)?,
        "original_project_sha256":original_hash,"completed":completed,"retirement":retired,"edit_input_latency":latency,
        "destination":destination,"source_project_unchanged":true,"saved_revision":reloaded.revision,"preview":preview,"independent":independent,"view_input_latency":viewing,"view_samples":viewed,
        "limits":["Software-injected native input and actual native chooser/window","No client acceptance or independent-user claim"]});
    File::create_new(directory.join("qualification.json"))?
        .write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    if !qualified {
        return Err("Native export input latency gate missed".into());
    }
    Ok(receipt)
}

fn control(trace: &mut Trace, name: &str) -> Result<[i64; 2]> {
    let record = trace.wait(name, |record| {
        record["kind"] == "preview"
            && record["details"]["controls"][name]
                .as_array()
                .is_some_and(|rect| rect.len() == 4)
    })?;
    let rect = record["details"]["controls"][name]
        .as_array()
        .ok_or("Native control has no geometry")?;
    let pid = trace
        .records
        .first()
        .and_then(|record| record["pid"].as_u64())
        .ok_or("Missing native control owner")?;
    let native = window(|window| window["pid"] == pid && window["class"] == "editbay")?;
    Ok([
        native["at"][0].as_i64().ok_or("Missing native window x")?
            + ((rect[0].as_f64().ok_or("Invalid control x")?
                + rect[2].as_f64().ok_or("Invalid control x")?)
                / 2.) as i64,
        native["at"][1].as_i64().ok_or("Missing native window y")?
            + ((rect[1].as_f64().ok_or("Invalid control y")?
                + rect[3].as_f64().ok_or("Invalid control y")?)
                / 2.) as i64,
    ])
}

fn click_control(trace: &mut Trace, name: &str) -> Result<u64> {
    trace.focus()?;
    let [x, y] = control(trace, name)?;
    dispatch(&format!("hl.dsp.cursor.move({{x={x},y={y}}})"))?;
    thread::sleep(Duration::from_millis(125));
    let sent = now();
    command("ydotool", &["click", "-D", "8", "0xC0"])?;
    Ok(sent)
}

fn picture(trace: &mut Trace, frame: u64, after: u64) -> Result<Value> {
    let record = trace.wait("native GPU draw completion", |record| {
        record["unix_us"].as_u64().is_some_and(|time| time >= after)
            && record["kind"] == "preview"
            && record["details"]["requested_frame"] == frame
            && record["details"]["displayed_frame"] == frame
            && record["details"]["displayed_serial"] == record["details"]["serial"]
            && record["details"]["gpu_draw_completed_us"]
                .as_u64()
                .is_some_and(|time| time > 0)
    })?;
    if record["details"]["stats"]["readbacks"] != 0 {
        return Err("Native viewer read pixels back to the CPU".into());
    }
    Ok(record)
}

fn set_frame(trace: &mut Trace, frame: u64) -> Result<Value> {
    let after = click_control(trace, "frame-number")?;
    click_control(trace, "frame-number")?;
    key(30, true, false)?;
    command("wtype", &["-s", "40", &frame.to_string(), "-s", "80"])?;
    key(28, false, false)?;
    picture(trace, frame, after)
}

fn sound_record(
    trace: &mut Trace,
    description: &str,
    after: u64,
    predicate: impl Fn(&Value) -> bool,
) -> Result<Value> {
    trace.wait(description, |record| {
        record["kind"] == "preview"
            && record["unix_us"].as_u64().is_some_and(|time| time >= after)
            && predicate(&record["details"])
    })
}

fn play_sound(trace: &mut Trace) -> Result<Value> {
    let after = click_control(trace, "play-sequence")?;
    sound_record(trace, "device playback callbacks", after, |details| {
        details["sound_active"] == true
            && details["sound"]["phase"] == "playing"
            && details["sound"]["callbacks"]
                .as_u64()
                .is_some_and(|count| count >= 8)
    })
}

fn pause_sound(trace: &mut Trace) -> Result<Value> {
    let after = click_control(trace, "pause-sequence")?;
    let result = sound_record(trace, "pause and codec retirement", after, |details| {
        details["sound_active"] == false && details["sound_retiring"] == 0
    })?;
    let elapsed = result["unix_us"].as_u64().ok_or("Missing pause time")? - after;
    if elapsed > 2_000_000 {
        return Err("Native sound pause exceeded two seconds".into());
    }
    Ok(result)
}

fn native_playback(trace: &mut Trace, directory: &Path, last: u64) -> Result<Value> {
    set_frame(trace, 0)?;
    let first = play_sound(trace)?;
    let first_at = first["unix_us"].as_u64().ok_or("Missing play time")?;
    let moving = sound_record(trace, "audio-clock picture advance", first_at, |details| {
        let sound = &details["sound"];
        details["displayed_frame"]
            .as_u64()
            .is_some_and(|frame| frame > 0)
            && sound["position_samples"]
                .as_u64()
                .zip(sound["device"]["sample_rate"].as_u64())
                .is_some_and(|(sample, rate)| sample >= rate / 2)
    })?;
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("playing.png").to_str().unwrap(),
        ],
    )?;
    let paused = pause_sound(trace)?;
    let bookmark = paused["details"]["resume_sound"]["Sample"]["position"]
        .as_u64()
        .ok_or("Pause lost exact sample position")?;
    let resumed = play_sound(trace)?;
    if resumed["details"]["sound"]["start_sample"] != bookmark {
        return Err("Native resume rounded away the paused sample".into());
    }
    let seek = set_frame(trace, 1)?;
    let seeked = sound_record(
        trace,
        "playing seek retirement",
        seek["unix_us"].as_u64().unwrap(),
        |details| {
            details["sound_active"] == false
                && details["sound_retiring"] == 0
                && details["resume_sound"].is_null()
        },
    )?;
    let sought = play_sound(trace)?;
    if sought["details"]["sound"]["start_sample"]
        .as_u64()
        .is_none_or(|sample| sample == 0 || sample >= bookmark)
    {
        return Err("Seek did not replace the exact paused sample boundary".into());
    }
    pause_sound(trace)?;
    let mut failures = Vec::new();
    for (signal, label) in [("-KILL", "codec-death"), ("-STOP", "underrun")] {
        set_frame(trace, 0)?;
        let active = play_sound(trace)?;
        let pid = active["details"]["sound"]["worker_pid"]
            .as_u64()
            .ok_or("Missing active PCM worker")?;
        let after = now();
        command("kill", &[signal, &pid.to_string()])?;
        let failed = sound_record(trace, label, after, |details| {
            details["sound_active"] == false
                && details["sound_retiring"] == 0
                && details["error"].is_string()
        })?;
        let elapsed_ms =
            (failed["unix_us"].as_u64().ok_or("Missing failure time")? - after) as f64 / 1000.;
        if elapsed_ms > 2000. || Path::new(&format!("/proc/{pid}")).exists() {
            return Err(format!("{label} exceeded two seconds or retained its process").into());
        }
        if signal == "-STOP"
            && !failed["details"]["error"]
                .as_str()
                .unwrap_or_default()
                .contains("fell behind")
        {
            return Err("Stalled codec did not expose the underrun".into());
        }
        command(
            "grim",
            &[
                "-g",
                "80,80 1440x900",
                directory
                    .join(format!("sound-{label}.png"))
                    .to_str()
                    .unwrap(),
            ],
        )?;
        failures.push(
            json!({"signal":signal,"active":active,"failed":failed,"retirement_ms":elapsed_ms}),
        );
    }
    set_frame(trace, last)?;
    let ending_at = click_control(trace, "play-sequence")?;
    let ended = sound_record(trace, "exact sequence end", ending_at, |details| {
        details["sound_active"] == false
            && details["sound_retiring"] == 0
            && details["sound"]["phase"] == "finished"
            && details["sound"]["position_samples"] == details["sound"]["end_sample"]
    })?;
    let active_before_edit = play_sound(trace)?;
    if active_before_edit["details"]["sound"]["start_sample"] != 0 {
        return Err("Play after the sequence end did not restart at zero".into());
    }
    Ok(
        json!({"first":first,"moving":moving,"paused":paused,"resumed":resumed,
        "seek":seeked,"sought":sought,"failures":failures,"ended":ended,"active_before_edit":active_before_edit,
        "exact_sample_resume":true,"replaced_seek_boundary":true,"worker_reaped":true}),
    )
}

/// Exercise saved source-sequence authoring and actual native GPU presentation.
/// `binary`, `source`, `directory`, `half` and `playback` select the app, media,
/// evidence folder, precision and device trials. Returns draw, ownership, cancellation,
/// recovery and layout receipts; source indexing is a real Rust seed, not UI ingest.
pub fn preview(
    binary: &Path,
    source: &Path,
    directory: &Path,
    half: bool,
    playback: bool,
) -> Result<Value> {
    use editbay_media::{Cancellation, SourceFile, StreamType};
    let binary = binary.canonicalize()?;
    let token = Cancellation::new()?;
    let owned = SourceFile::open(source, &token)?;
    let probe = owned.probe(token.clone())?;
    let stream = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Video && stream.decoder_available)
        .ok_or("Qualification needs a real video stream")?
        .index;
    let audio = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Audio && stream.decoder_available)
        .map(|s| s.index);
    let streams: Vec<_> = std::iter::once(stream).chain(audio).collect();
    let compositions = if audio.is_some() { 2 } else { 1 };
    let imported = owned.ingest(
        "Native source sequence".into(),
        &streams,
        token.clone(),
        |_, _| {},
    )?;
    fs::create_dir(directory)?;
    let directory = directory.canonicalize()?;
    let catalog = directory.join("catalog");
    fs::create_dir(&catalog)?;
    let state = directory.join("state");
    let original = directory.join("Preview.editbay");
    let mut initial = Project::new("Native source sequence")?;
    initial.color.precision = if half {
        editbay_core::FloatPrecision::Half
    } else {
        editbay_core::FloatPrecision::Full
    };
    let source_id = imported.source.id;
    initial.assets.push(imported.asset);
    initial.sources.push(imported.source);
    save_new(&initial, &original)?;
    let (mut application, mut trace) = start(
        &binary,
        &state,
        &catalog,
        &directory.join("native.jsonl"),
        Some(&original),
    )?;
    trace.wait("indexed source document open", |record| {
        has_tab(record, &initial.name, 0)
    })?;
    click_control(&mut trace, &format!("source:{source_id}"))?;
    thread::sleep(Duration::from_millis(200));
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("source-selection.png").to_str().unwrap(),
        ],
    )?;
    let create_at = click_control(&mut trace, &format!("create-sequence:{source_id}:{stream}"))?;
    let created = trace.wait("source sequence command accepted", |record| {
        has_tab(record, &initial.name, 1)
            && record["details"]["tabs"][0]["compositions"] == compositions
    })?;
    let first = picture(&mut trace, 0, create_at)?;
    click_control(&mut trace, "source-media")?;
    thread::sleep(Duration::from_millis(200));
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("native-1440x900.png").to_str().unwrap(),
        ],
    )?;
    let mut input_ms = Vec::new();
    let mut displayed_ms = Vec::new();
    let mut samples = Vec::new();
    for index in 0..40 {
        let frame = if index % 2 == 0 { 1 } else { 0 };
        let sent = click_control(
            &mut trace,
            if frame == 1 {
                "next-frame"
            } else {
                "previous-frame"
            },
        )?;
        let accepted = trace.wait("frame request accepted", |record| {
            record["details"]["request_accepted_unix_us"]
                .as_u64()
                .is_some_and(|time| time >= sent)
                && record["kind"] == "preview"
                && record["details"]["requested_frame"] == frame
        })?;
        let drawn = picture(&mut trace, frame, sent)?;
        input_ms.push(
            (accepted["details"]["request_accepted_unix_us"]
                .as_u64()
                .ok_or("Missing request timestamp")?
                - sent) as f64
                / 1000.,
        );
        samples.push(json!({"sample":index,"frame":frame,"injected_unix_us":sent,
            "accepted_unix_us":accepted["details"]["request_accepted_unix_us"],
            "gpu_draw_completed_us":drawn["details"]["gpu_draw_completed_us"],
            "cached":index > 1}));
        if index > 1 {
            displayed_ms.push(
                drawn["details"]["gpu_draw_completed_us"]
                    .as_u64()
                    .ok_or("Missing GPU completion")? as f64
                    / 1000.,
            );
        }
    }
    let warmed = picture(&mut trace, 0, create_at)?;
    let pid = warmed["details"]["worker_pid"]
        .as_u64()
        .ok_or("Missing native codec process")?;
    command("kill", &["-STOP", &pid.to_string()])?;
    let scrub_at = click_control(&mut trace, "next-frame")?;
    for _ in 0..5 {
        thread::sleep(Duration::from_millis(30));
        command("ydotool", &["click", "0xC0"])?;
    }
    let requested = trace.wait("coalesced scrub request", |record| {
        record["unix_us"]
            .as_u64()
            .is_some_and(|time| time >= scrub_at)
            && record["kind"] == "preview"
            && record["details"]["requested_frame"]
                .as_u64()
                .is_some_and(|frame| frame >= 4)
    })?;
    let target = requested["details"]["requested_frame"]
        .as_u64()
        .ok_or("Missing last scrub target")?;
    command("kill", &["-CONT", &pid.to_string()])?;
    let scrubbed = picture(&mut trace, target, scrub_at)?;
    if scrubbed["details"]["rejected_results"]
        .as_u64()
        .unwrap_or(0)
        == 0
    {
        return Err("Superseded scrub result was not rejected".into());
    }
    let killed_at = now();
    command("kill", &["-KILL", &pid.to_string()])?;
    let failed = trace.wait("idle native codec death", |record| {
        record["unix_us"]
            .as_u64()
            .is_some_and(|time| time >= killed_at)
            && record["kind"] == "preview"
            && record["details"]["stopped"] == true
            && record["details"]["error"].is_string()
    })?;
    thread::sleep(Duration::from_millis(150));
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("worker-failure.png").to_str().unwrap(),
        ],
    )?;
    let retried_at = click_control(&mut trace, "retry-viewer")?;
    let retried = picture(&mut trace, target, retried_at)?;
    let retry_pid = retried["details"]["worker_pid"]
        .as_u64()
        .ok_or("Missing retried codec process")?;
    if retry_pid == pid {
        return Err("Retry reused the dead process".into());
    }
    command("kill", &["-STOP", &retry_pid.to_string()])?;
    click_control(&mut trace, "next-frame")?;
    let cancel_at = click_control(&mut trace, "cancel-viewer")?;
    let cancelled = trace.wait("native active cancellation and cleanup", |record| {
        record["unix_us"]
            .as_u64()
            .is_some_and(|time| time >= cancel_at)
            && record["kind"] == "preview"
            && record["details"]["stopped"] == true
            && record["details"]["retiring"] == 0
    })?;
    let cancel_ms = (cancelled["unix_us"]
        .as_u64()
        .ok_or("Missing cancellation timestamp")?
        - cancel_at) as f64
        / 1000.;
    if cancel_ms > 2000.
        || Path::new(&format!("/proc/{retry_pid}")).exists()
        || load(&original)? != initial
    {
        return Err("Native viewer cancellation exceeded budget, leaked a process or changed the saved source project".into());
    }
    trace.focus()?;
    key(44, true, false)?;
    let undone = trace.wait("native sequence undo", |record| {
        has_tab(record, &initial.name, 2) && record["details"]["tabs"][0]["compositions"] == 0
    })?;
    trace.focus()?;
    key(44, true, true)?;
    let redone = trace.wait("native sequence redo", |record| {
        has_tab(record, &initial.name, 3)
            && record["details"]["tabs"][0]["compositions"] == compositions
    })?;
    trace.focus()?;
    key(31, true, false)?;
    trace.wait("native sequence durable save", |record| {
        has_tab(record, &initial.name, 3) && record["details"]["tabs"][0]["dirty"] == false
    })?;
    let saved = load(&original)?;
    let saved_hash = hash(&original)?;
    if saved.sources != initial.sources
        || saved.assets != initial.assets
        || saved.compositions.len() != compositions
    {
        return Err("Native authoring changed source assets or lost its composition".into());
    }
    let root = saved
        .compositions
        .iter()
        .find(|c| Some(c.id) == saved.sequences[0].composition)
        .ok_or("Missing saved source sequence")?;
    let playback_receipt = playback
        .then(|| native_playback(&mut trace, &directory, root.duration - 1))
        .transpose()?;
    let address = window(|window| {
        window["pid"] == application.0.id() && window["class"] == "editbay"
    })?["address"]
        .as_str()
        .ok_or("Missing preview window identity")?
        .to_owned();
    dispatch(&format!(
        "hl.dsp.window.resize({{x=800,y=600,relative=false,window=\"address:{address}\"}})"
    ))?;
    dispatch(&format!(
        "hl.dsp.window.move({{x=80,y=80,relative=false,window=\"address:{address}\"}})"
    ))?;
    thread::sleep(Duration::from_millis(350));
    command(
        "grim",
        &[
            "-g",
            "80,80 800x600",
            directory.join("native-800x600.png").to_str().unwrap(),
        ],
    )?;
    dispatch(&format!(
        "hl.dsp.window.resize({{x=1440,y=900,relative=false,window=\"address:{address}\"}})"
    ))?;
    dispatch(&format!(
        "hl.dsp.window.move({{x=80,y=80,relative=false,window=\"address:{address}\"}})"
    ))?;
    thread::sleep(Duration::from_millis(250));
    let mut rename_times = Vec::new();
    rename(
        &mut trace,
        "Recovered source sequence",
        4,
        &mut rename_times,
    )?;
    let acknowledged = trace.wait("authored media checkpoint", |record| {
        has_tab(record, "Recovered source sequence", 4)
            && record["details"]["tabs"][0]["recovery_revision"] == 4
    })?;
    let edit_cancel = if let Some(receipt) = &playback_receipt {
        let cancelled = trace.wait("edited sound ownership retirement", |record| {
            record["kind"] == "preview"
                && record["unix_us"].as_u64().is_some_and(|time| {
                    time >= receipt["active_before_edit"]["unix_us"].as_u64().unwrap()
                })
                && record["details"]["sound_active"] == false
                && record["details"]["sound_retiring"] == 0
                && record["details"]["sound"].is_null()
        })?;
        let pid = receipt["active_before_edit"]["details"]["sound"]["worker_pid"]
            .as_u64()
            .ok_or("Missing edited PCM worker")?;
        if Path::new(&format!("/proc/{pid}")).exists() {
            return Err("Edited playback retained its PCM worker".into());
        }
        Some(cancelled)
    } else {
        None
    };
    let memory = fs::read_to_string(format!("/proc/{}/status", application.0.id()))?
        .lines()
        .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    application.kill()?;
    let checkpoint = recovery_catalog(state.join("recovery"))?
        .valid
        .into_iter()
        .find(|item| item.project_id == initial.id && item.revision == 4)
        .ok_or("Missing authored checkpoint")?;
    let checkpoint_hash = hash(&checkpoint.path)?;
    let (mut restarted, mut recovered_trace) = start(
        &binary,
        &state,
        &catalog,
        &directory.join("recovery.jsonl"),
        None,
    )?;
    recover(&mut recovered_trace, &directory, 1)?;
    let recovered_path = directory.join("Recovered.editbay");
    let recovered = load(&recovered_path)?;
    let recovered_draw = picture(&mut recovered_trace, 0, 0)?;
    if recovered.compositions != saved.compositions
        || recovered.sources != saved.sources
        || recovered.assets != saved.assets
        || recovered.id == initial.id
        || recovered.recovered_from != Some(initial.id)
        || hash(&original)? != saved_hash
        || hash(&checkpoint.path)? != checkpoint_hash
    {
        return Err(
            "Native recovery changed authored content, identities or original files".into(),
        );
    }
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("recovered-native.png").to_str().unwrap(),
        ],
    )?;
    let root = recovered
        .compositions
        .iter()
        .find(|c| Some(c.id) == recovered.sequences[0].composition)
        .ok_or("Missing recovered sequence root")?;
    let recovered_tail = set_frame(&mut recovered_trace, root.duration - 1)?;
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("recovered-tail.png").to_str().unwrap(),
        ],
    )?;
    restarted.kill()?;
    let (mut reopened, mut reopened_trace) = start(
        &binary,
        &state,
        &catalog,
        &directory.join("reopen.jsonl"),
        Some(&original),
    )?;
    let reopened_draw = picture(&mut reopened_trace, 0, 0)?;
    reopened.kill()?;
    let saved_sound = audio
        .map(|audio| crate::natural_sound::verify(&saved, source, stream, audio, &binary))
        .transpose()?;
    let recovered_sound = audio
        .map(|audio| crate::natural_sound::verify(&recovered, source, stream, audio, &binary))
        .transpose()?;
    if saved_sound.as_ref().map(|v| &v["rendered_sha256"])
        != recovered_sound.as_ref().map(|v| &v["rendered_sha256"])
    {
        return Err("Recovered sound differs from saved sound".into());
    }
    owned.verify(&token)?;
    let input = metrics(&mut input_ms);
    let display = metrics(&mut displayed_ms);
    let pass = input["p95_ms"].as_f64().is_some_and(|v| v <= 50.)
        && display["p95_ms"].as_f64().is_some_and(|v| v <= 250.)
        && cancel_ms <= 2000.;
    let receipt = json!({"kind":if playback {"native_playback_qualification"} else {"native_preview_qualification"},"application_sha256":hash(&binary)?,"source":source.canonicalize()?,
        "source_fingerprint":owned.fingerprint(),"precision":initial.color.precision,"created":created,"first_native_draw":first,
        "saved_sound":saved_sound,"recovered_sound":recovered_sound,"recovered_tail":recovered_tail,
        "playback":playback_receipt,"edit_cancellation":edit_cancel,
        "input_injection_to_request_acceptance":input,"cached_request_to_gpu_draw_completion":display,
        "button_event_delay_ms":8,"samples":samples,
        "superseded_scrub":scrubbed,"idle_worker_failure":failed,"retry":retried,"active_cancel":cancelled,"active_cancel_ms":cancel_ms,
        "undo":undone,"redo":redone,"checkpoint":acknowledged,"checkpoint_sha256":checkpoint_hash,
        "saved_sha256":saved_hash,"recovered_sha256":hash(&recovered_path)?,"recovered_native_draw":recovered_draw,"reopened_native_draw":reopened_draw,
        "source_unchanged":true,"memory":memory,"pass":pass,"limits":["Indexing is a real native Rust seed; existing UI ingest has separate receipts",
            "GPU command completion and inspected native screenshots; no physical input or display photon timing",
            "Device submission is not physical audibility or two-hour drift; sustained 1080p/4K performance, delivery and other hardware remain open"]});
    File::create_new(directory.join("qualification.json"))?
        .write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    if !pass {
        return Err("Native preview performance gate missed; inspect qualification.json".into());
    }
    Ok(receipt)
}

fn has_tab(record: &Value, name: &str, revision: u64) -> bool {
    record["kind"] == "workspace"
        && record["details"]["tabs"].as_array().is_some_and(|tabs| {
            tabs.iter()
                .any(|tab| tab["name"] == name && tab["revision"] == revision)
        })
}

fn accept(trace: &mut Trace, name: &str, revision: u64, latencies: &mut Vec<f64>) -> Result<Value> {
    trace.focus()?;
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
    trace.focus()?;
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
    trace.focus()?;
    let requested = now();
    click_control(trace, "rename-project")?;
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

fn choose_camera(trace: &mut Trace, source: &Path) -> Result<Value> {
    trace.focus()?;
    let requested = now();
    click_control(trace, "import-media")?;
    let chooser = window(|window| {
        matches!(
            window["class"].as_str(),
            Some("org.omarchy.synchro" | "com.thisisgm.flea.picker")
        ) && window["mapped"] == true
            && window["title"]
                .as_str()
                .is_some_and(|title| title.contains("Import source media"))
    })?;
    let address = chooser["address"]
        .as_str()
        .ok_or("Missing source chooser identity")?;
    dispatch(&format!("hl.dsp.focus({{window=\"address:{address}\"}})"))?;
    let active: Value = serde_json::from_str(&command("hyprctl", &["activewindow", "-j"])?)?;
    if active["address"] != chooser["address"] {
        return Err("Source chooser did not acquire native input".into());
    }
    if chooser["class"] == "com.thisisgm.flea.picker" {
        let pid = chooser["pid"]
            .as_u64()
            .ok_or("Missing owned source picker PID")?
            .to_string();
        let source = source.canonicalize()?;
        flea_folder(&pid, source.parent().ok_or("Source has no folder")?)?;
        flea_entry(
            &pid,
            source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("Source needs UTF-8")?,
        )?;
    } else {
        key(38, true, false)?;
        thread::sleep(Duration::from_millis(100));
        key(30, true, false)?;
        command(
            "wtype",
            &[
                "-s",
                "40",
                source.to_str().ok_or("Source needs UTF-8")?,
                "-s",
                "80",
            ],
        )?;
        key(28, false, false)?;
        thread::sleep(Duration::from_millis(200));
        let coordinate = |field: &str, index: usize| -> Result<i64> {
            chooser[field][index]
                .as_i64()
                .ok_or_else(|| "Source chooser has no geometry".into())
        };
        click(
            coordinate("at", 0)? + coordinate("size", 0)? - 40,
            coordinate("at", 1)? + coordinate("size", 1)? - 18,
        )?;
    }
    trace.wait("actual camera stream selection", |record| {
        record["unix_us"]
            .as_u64()
            .is_some_and(|time| time >= requested)
            && record["kind"] == "media"
            && record["details"]["phase"] == "select"
    })
}

/// Qualify native camera ingest, worker failure, cancellation and recovery.
/// `binary` selects the app, `source` has one picture and one sound stream,
/// and `directory` must be new. Returns actual window/portal/process receipts;
/// decoded sources remain read-only and no playback performance is claimed.
pub fn media(binary: &Path, source: &Path, directory: &Path) -> Result<Value> {
    use editbay_media::{Cancellation, SourceFile, StreamType};
    let binary = binary.canonicalize()?;
    let source = source.canonicalize()?;
    let token = Cancellation::new()?;
    let owned = SourceFile::open(&source, &token)?;
    let probe = owned.probe(token.clone())?;
    if probe.streams.len() != 2
        || !probe.streams.iter().all(|stream| stream.decoder_available)
        || probe
            .streams
            .iter()
            .filter(|stream| stream.kind == StreamType::Video)
            .count()
            != 1
        || probe
            .streams
            .iter()
            .filter(|stream| stream.kind == StreamType::Audio)
            .count()
            != 1
        || probe
            .streams
            .iter()
            .any(|stream| stream.alpha_interpretation_required)
    {
        return Err("Native camera qualification needs one picture and one sound stream".into());
    }
    fs::create_dir(directory)?;
    let directory = directory.canonicalize()?;
    let catalog = directory.join("catalog");
    fs::create_dir(&catalog)?;
    let state = directory.join("state");
    let original = directory.join("Camera.editbay");
    let initial = Project::new("Native camera edit")?;
    save_new(&initial, &original)?;
    let (mut application, mut trace) = start(
        &binary,
        &state,
        &catalog,
        &directory.join("native.jsonl"),
        Some(&original),
    )?;
    trace.wait("camera document open", |record| {
        has_tab(record, &initial.name, 0)
    })?;
    let selected = choose_camera(&mut trace, &source)?;
    let worker = selected["details"]["worker_pid"]
        .as_u64()
        .ok_or("Missing actual codec worker")?;
    let limits = fs::read_to_string(format!("/proc/{worker}/limits"))?
        .lines()
        .filter(|line| {
            line.starts_with("Max cpu time")
                || line.starts_with("Max open files")
                || line.starts_with("Max address space")
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let killed_at = now();
    command("kill", &["-KILL", &worker.to_string()])?;
    let failed = trace.wait("idle worker failure without further input", |record| {
        record["unix_us"]
            .as_u64()
            .is_some_and(|time| time >= killed_at)
            && record["kind"] == "media"
            && record["details"]["phase"].is_null()
            && record["details"]["error"]
                .as_str()
                .is_some_and(|error| error.contains("stopped"))
    })?;
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("worker-failure.png").to_str().unwrap(),
        ],
    )?;
    if load(&original)? != initial {
        return Err("Worker crash modified the original".into());
    }
    choose_camera(&mut trace, &source)?;
    trace.focus()?;
    click(150, 440)?;
    let progressing = trace.wait("native decode has read actual source content", |record| {
        record["kind"] == "media"
            && record["details"]["phase"] == "ingest"
            && record["details"]["progress"][1]
                .as_u64()
                .is_some_and(|count| count >= 32)
    })?;
    let cancel_at = now();
    click(699, 110)?;
    let cancelled = trace.wait("native active import cancellation", |record| {
        record["unix_us"]
            .as_u64()
            .is_some_and(|time| time >= cancel_at)
            && record["kind"] == "media"
            && record["details"]["phase"].is_null()
            && record["details"]["completed_imports"] == 0
            && record["details"]["error"].is_null()
    })?;
    let cancel_ms = (cancelled["unix_us"]
        .as_u64()
        .ok_or("Missing cancellation timestamp")?
        - cancel_at) as f64
        / 1000.;
    if cancel_ms > 2000. || load(&original)? != initial {
        return Err("Native cancellation exceeded its budget or changed the original".into());
    }
    choose_camera(&mut trace, &source)?;
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("stream-selection.png").to_str().unwrap(),
        ],
    )?;
    trace.focus()?;
    let import_at = now();
    click(150, 440)?;
    let imported = trace.wait("native media document publication", |record| {
        has_tab(record, &initial.name, 1)
            && record["details"]["tabs"][0]["sources"] == 1
            && record["details"]["tabs"][0]["assets"] == 1
    })?;
    let import_ms = (imported["unix_us"]
        .as_u64()
        .ok_or("Missing import timestamp")?
        - import_at) as f64
        / 1000.;
    trace.focus()?;
    key(44, true, false)?;
    let undone = trace.wait("native import undo", |record| {
        has_tab(record, &initial.name, 2) && record["details"]["tabs"][0]["sources"] == 0
    })?;
    trace.focus()?;
    key(44, true, true)?;
    let redone = trace.wait("native import redo", |record| {
        has_tab(record, &initial.name, 3) && record["details"]["tabs"][0]["sources"] == 1
    })?;
    trace.focus()?;
    key(31, true, false)?;
    let saved = trace.wait("durable native media save", |record| {
        has_tab(record, &initial.name, 3) && record["details"]["tabs"][0]["dirty"] == false
    })?;
    let saved_project = load(&original)?;
    let saved_hash = hash(&original)?;
    if saved_project.revision != 3
        || saved_project.sources.len() != 1
        || saved_project.sources[0].streams.len() != 2
    {
        return Err("Native save lost selected camera streams".into());
    }
    let mut input_times = Vec::new();
    click(160, 329)?;
    let revised = "Native camera revision";
    rename(&mut trace, revised, 4, &mut input_times)?;
    let acknowledged = trace.wait("media checkpoint revision four", |record| {
        has_tab(record, revised, 4) && record["details"]["tabs"][0]["recovery_revision"] == 4
    })?;
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("imported-native.png").to_str().unwrap(),
        ],
    )?;
    let peak_memory = fs::read_to_string(format!("/proc/{}/status", application.0.id()))?
        .lines()
        .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    application.kill()?;
    let snapshot = recovery_catalog(state.join("recovery"))?
        .valid
        .into_iter()
        .find(|snapshot| snapshot.project_id == initial.id && snapshot.revision == 4)
        .ok_or("Acknowledged media checkpoint is missing")?;
    let checkpoint_hash = hash(&snapshot.path)?;
    let (mut restarted, mut recovery_trace) = start(
        &binary,
        &state,
        &catalog,
        &directory.join("recovery.jsonl"),
        None,
    )?;
    recover(&mut recovery_trace, &directory, 1)?;
    let recovered_path = directory.join("Recovered.editbay");
    let recovered = load(&recovered_path)?;
    if recovered.id == initial.id
        || recovered.recovered_from != Some(initial.id)
        || recovered.revision != 4
        || recovered.name != revised
        || recovered.assets != saved_project.assets
        || recovered.sources != saved_project.sources
        || hash(&original)? != saved_hash
        || hash(&snapshot.path)? != checkpoint_hash
    {
        return Err("Native media recovery changed identities, content or originals".into());
    }
    thread::sleep(Duration::from_millis(300));
    recovery_trace.focus()?;
    click(160, 329)?;
    thread::sleep(Duration::from_millis(100));
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("recovered-native.png").to_str().unwrap(),
        ],
    )?;
    restarted.kill()?;
    owned.verify(&token)?;
    let receipt = json!({"kind":"native_media_ingest_qualification","application_sha256":hash(&binary)?,
        "source":source,"source_fingerprint":owned.fingerprint(),"worker_limits":limits,
        "worker_crash":failed,"active_progress_before_cancel":progressing,"cancellation":cancelled,
        "cancellation_ms":cancel_ms,"import_ms":import_ms,"import":imported,"undo":undone,"redo":redone,
        "save":saved,"checkpoint":acknowledged,"checkpoint_sha256":checkpoint_hash,
        "saved_sha256":saved_hash,"recovered_sha256":hash(&recovered_path)?,
        "saved_project":saved_project.id,"recovered_project":recovered.id,
        "recovered_revision":recovered.revision,"source_unchanged":true,"memory":peak_memory,
        "pass":true,"production_playback_qualified":false});
    File::create_new(directory.join("qualification.json"))?
        .write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    Ok(receipt)
}

fn flea_state(pid: &str) -> Result<Value> {
    Ok(serde_json::from_str(&command(
        "timeout",
        &[
            "2",
            "qs",
            "ipc",
            "--pid",
            pid,
            "call",
            "fleapicker",
            "snapshot",
        ],
    )?)?)
}

fn flea_wait(pid: &str, predicate: impl Fn(&Value) -> bool) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = flea_state(pid)?;
        if predicate(&state) {
            return Ok(state);
        }
        if Instant::now() >= deadline {
            return Err("Owned Flea picker did not acknowledge native input".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn flea_point(pid: &str, point: &str) -> Result<()> {
    let point = point
        .split_whitespace()
        .map(str::parse::<i64>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if point.len() != 2 {
        return Err("Invalid native Flea control geometry".into());
    }
    let pid = pid.parse::<u64>()?;
    let native =
        window(|window| window["pid"] == pid && window["class"] == "com.thisisgm.flea.picker")?;
    click(
        native["at"][0].as_i64().ok_or("Missing Flea x")? + point[0],
        native["at"][1].as_i64().ok_or("Missing Flea y")? + point[1],
    )
}

fn flea_entry(pid: &str, name: &str) -> Result<()> {
    let mut state = flea_wait(pid, |state| state["state"] == "ready")?;
    if state["listFocus"] != true {
        let cursor = state["cursor"]
            .as_u64()
            .ok_or("Missing native cursor")?
            .to_string();
        let point = command(
            "qs",
            &[
                "ipc",
                "--pid",
                pid,
                "call",
                "fleapicker",
                "rowCentre",
                &cursor,
            ],
        )?;
        flea_point(pid, point.trim())?;
    }
    key(102, false, false)?;
    state = flea_wait(pid, |state| state["cursor"] == 0 && state["held"] == 0)?;
    for _ in 0..4096 {
        let cursor = state["cursor"].as_u64().ok_or("Missing native cursor")?;
        let rows = state["rows"].as_array().ok_or("Flea listing is absent")?;
        if let Some(offset) = rows.iter().position(|row| row["n"] == name) {
            let index = offset as u64
                + state["held"]
                    .as_u64()
                    .ok_or("Missing native listing offset")?;
            if index > 4096 {
                return Err("Native picker navigation exceeds qualification bounds".into());
            }
            for _ in 0..cursor.abs_diff(index) {
                key(if index >= cursor { 108 } else { 103 }, false, false)?;
            }
            flea_wait(pid, |state| state["cursorName"] == name)?;
            if rows[offset]["d"] == false || rows[offset]["d"] == 0 {
                key(57, false, false)?;
                flea_wait(pid, |state| {
                    state["marksBusy"] == false && state["canAccept"] == true
                })?;
            }
            return key(28, false, false);
        }
        let total = state["total"]
            .as_u64()
            .ok_or("Missing native listing size")?;
        if cursor >= 4096 || cursor + 1 >= total {
            return Err(format!("Requested native picker entry is absent: {name}").into());
        }
        key(109, false, false)?;
        state = flea_wait(pid, |state| {
            state["cursor"].as_u64().is_some_and(|next| next > cursor)
                && state["cursorName"]
                    .as_str()
                    .is_some_and(|name| !name.is_empty())
        })?;
    }
    Err("Native picker navigation exceeds qualification bounds".into())
}

fn flea_folder(pid: &str, folder: &Path) -> Result<()> {
    for _ in 0..64 {
        let state = flea_wait(pid, |state| state["state"] == "ready")?;
        let current = Path::new(state["path"].as_str().ok_or("Flea folder is absent")?);
        if current == folder {
            return Ok(());
        }
        let next = if folder.starts_with(current) {
            let name = folder
                .strip_prefix(current)?
                .components()
                .next()
                .ok_or("Missing folder component")?;
            let name = name
                .as_os_str()
                .to_str()
                .ok_or("Qualification folder needs UTF-8")?;
            let next = current.join(name);
            flea_entry(pid, name)?;
            next
        } else {
            let next = current
                .parent()
                .ok_or("Native folder navigation reached its root")?
                .to_path_buf();
            let parent = state["controls"]
                .as_array()
                .and_then(|controls| {
                    controls.iter().find(|control| {
                        control["name"] == "Parent folder" && control["enabled"] == true
                    })
                })
                .and_then(|control| control["centre"].as_str())
                .ok_or("Native parent-folder control is unavailable")?;
            flea_point(pid, parent)?;
            next
        };
        flea_wait(pid, |state| {
            state["path"].as_str() == next.to_str() && state["state"] == "ready"
        })?;
    }
    Err("Native folder navigation exceeds its depth bound".into())
}

fn recover(trace: &mut Trace, folder: &Path, minimum_checkpoints: u64) -> Result<()> {
    trace.focus()?;
    click(240, 224)?;
    trace.wait("recovery catalog", |record| {
        record["kind"] == "frame"
            && record["details"]["recovered"] == true
            && record["details"]["recoveries_valid"]
                .as_u64()
                .is_some_and(|count| count >= minimum_checkpoints)
    })?;
    click(380, 307)?;
    trace.wait("recovery preview", |record| {
        record["kind"] == "frame" && record["details"]["recovery_preview"] == true
    })?;
    click(665, 579)?;
    let chooser = window(|window| {
        matches!(
            window["class"].as_str(),
            Some("org.omarchy.synchro" | "com.thisisgm.flea.picker")
        ) && window["mapped"] == true
            && window["title"]
                .as_str()
                .is_some_and(|title| title.starts_with("Save EditBay project"))
    })?;
    let address = chooser["address"]
        .as_str()
        .ok_or("Missing recovery chooser identity")?;
    dispatch(&format!("hl.dsp.focus({{window=\"address:{address}\"}})"))?;
    let active = focused(
        chooser["pid"]
            .as_u64()
            .ok_or("Missing recovery chooser PID")?,
    )?;
    if active["address"] != chooser["address"] {
        return Err("Recovery chooser did not acquire native input".into());
    }
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
    if chooser["class"] == "com.thisisgm.flea.picker" {
        let pid = chooser["pid"]
            .as_u64()
            .ok_or("Missing owned Flea picker PID")?
            .to_string();
        flea_folder(&pid, folder)?;
        let state: Value = serde_json::from_str(&command(
            "qs",
            &["ipc", "--pid", &pid, "call", "fleapicker", "saveState"],
        )?)?;
        let field = state["field"]
            .as_str()
            .ok_or("Flea filename field has no geometry")?
            .split_whitespace()
            .map(str::parse::<i64>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if field.len() != 2 {
            return Err("Invalid Flea filename field geometry".into());
        }
        click(x + field[0], y + field[1])?;
        key(30, true, false)?;
        command("wtype", &["-s", "40", "Recovered.editbay", "-s", "80"])?;
        key(28, false, false)?;
    } else {
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
    }
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
        trace.focus()?;
        key(44, true, false)?;
        trace.wait("native undo", |record| has_tab(record, &active, 2))?;
        trace.focus()?;
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
        recover(&mut restarted_trace, &folder, 2)?;
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
    let receipt = json!({"schema":1,"kind":"native_workspace_qualification","application_sha256":hash(&binary)?,"architecture":std::env::consts::ARCH,"desktop":"Hyprland Wayland / installed native portal","catalog_documents":4000,"trials":count,"passed":reports.len(),"latest_revision":3,"input_injection_to_ui_acceptance":input,"cpu_frame_work":metrics(&mut frames),"checkpoint_main_thread_commit":metrics(&mut commits),"input_gate_pass":input["p95_ms"].as_f64().is_some_and(|latency|latency <= 50.),"reports":reports,"limits":["Software-injected Wayland text/portal keys and persistent Linux keyboard/pointer events; no physical-device input measurement","Native UI recovery uses the installed native file chooser","These fixtures qualify local workspace behavior, not completed client edits or media playback"]});
    File::create_new(directory.join("qualification.json"))?
        .write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    Ok(receipt)
}

/// Exercise linked editing through the real native window and shared export.
/// `binary`, `project` and new `directory` select actual content and evidence.
/// Returns native input receipts, saved/recovered identity and decoded output.
pub fn timeline(binary: &Path, project: &Path, directory: &Path) -> Result<Value> {
    let binary = binary.canonicalize()?;
    let original = project.canonicalize()?;
    let original_hash = hash(&original)?;
    let seed = load(&original)?;
    let source = seed
        .sequences
        .iter()
        .find_map(|s| s.composition)
        .ok_or("Source sequence absent")?;
    if seed
        .compositions
        .iter()
        .find(|c| c.id == source)
        .ok_or("Source absent")?
        .duration
        < 65
    {
        return Err("Timeline qualification needs at least 65 source frames".into());
    }
    fs::create_dir(directory)?;
    let directory = directory.canonicalize()?;
    let catalog = directory.join("catalog");
    fs::create_dir(&catalog)?;
    let copy = directory.join("Cut.editbay");
    save_new(&seed, &copy)?;
    let destination = directory.join("Cut.mov");
    let (mut app, mut trace) = start(
        &binary,
        &directory.join("state"),
        &catalog,
        &directory.join("native.jsonl"),
        Some(&copy),
    )?;
    picture(&mut trace, 0, 0)?;
    click_control(&mut trace, "source-media")?;
    set_frame(&mut trace, 4)?;
    key(23, false, false)?;
    trace.wait("source in shortcut", |r| {
        r["kind"] == "timeline" && r["details"]["source_range"]["start"] == 4
    })?;
    set_frame(&mut trace, 27)?;
    key(24, false, false)?;
    trace.wait("source out shortcut", |r| {
        r["kind"] == "timeline" && r["details"]["source_range"]["end"] == 28
    })?;
    let mut revision = seed.revision;
    let mut inputs = Vec::new();
    let mut edits = Vec::new();
    let mut perform = |trace: &mut Trace, control: &str| -> Result<Value> {
        let sent = if control == "timeline-split" || control == "timeline-append" {
            let sent = now();
            key(
                if control == "timeline-split" { 31 } else { 18 },
                false,
                false,
            )?;
            sent
        } else {
            click_control(trace, control)?
        };
        revision += 1;
        let committed = trace.wait("timeline command commit", |r| {
            has_tab(r, &seed.name, revision)
        })?;
        let latency = (committed["unix_us"]
            .as_u64()
            .ok_or("Commit timestamp absent")?
            - sent) as f64
            / 1000.;
        inputs.push(latency);
        let timeline = timeline_record(trace, revision)?;
        edits.push(
            json!({"control":control,"input_ms":latency,"committed":committed,"timeline":timeline}),
        );
        Ok(timeline)
    };
    perform(&mut trace, "timeline-create")?;
    set_timeline_input(&mut trace, "timeline-source-in", 40)?;
    set_timeline_input(&mut trace, "timeline-source-out", 64)?;
    let appended = perform(&mut trace, "timeline-append")?;
    let first = appended["clips"][0]["id"]
        .as_str()
        .ok_or("First clip absent")?
        .to_owned();
    click_control(&mut trace, &format!("timeline-clip:{first}"))?;
    set_timeline_input(&mut trace, "timeline-record-at", 12)?;
    perform(&mut trace, "timeline-split")?;
    click_control(&mut trace, &format!("timeline-clip:{first}"))?;
    let removed = perform(&mut trace, "timeline-remove")?;
    let last = removed["clips"][1]["id"]
        .as_str()
        .ok_or("Last clip absent")?
        .to_owned();
    click_control(&mut trace, &format!("timeline-clip:{last}"))?;
    set_timeline_input(&mut trace, "timeline-trim-in", 42)?;
    set_timeline_input(&mut trace, "timeline-trim-out", 65)?;
    let trimmed = perform(&mut trace, "timeline-trim")?;
    let final_clips = trimmed["clips"].clone();
    key(44, true, false)?;
    revision += 1;
    let undone = timeline_record(&mut trace, revision)?;
    if undone["clips"][1]["source"]["range"] != json!({"start":40,"end":64}) {
        return Err("Native undo did not restore the linked trim".into());
    }
    key(44, true, true)?;
    revision += 1;
    let redone = timeline_record(&mut trace, revision)?;
    if redone["clips"] != final_clips {
        return Err("Native redo changed linked identities".into());
    }
    if final_clips[0]["range"] != json!({"start":0,"end":12})
        || final_clips[0]["source"]["range"] != json!({"start":16,"end":28})
        || final_clips[1]["range"] != json!({"start":12,"end":35})
        || final_clips[1]["source"]["range"] != json!({"start":42,"end":65})
    {
        return Err("Native linked edit boundaries differ".into());
    }
    set_frame(&mut trace, 11)?;
    let before_cut = picture(&mut trace, 11, 0)?;
    let after_cut = set_frame(&mut trace, 12)?;
    set_frame(&mut trace, 0)?;
    let playing = play_sound(&mut trace)?;
    let paused = pause_sound(&mut trace)?;
    key(31, true, false)?;
    trace.wait("saved cut", |record| {
        has_tab(record, &seed.name, revision) && record["details"]["tabs"][0]["dirty"] == false
    })?;
    let saved = load(&copy)?;
    let composition: uuid::Uuid = redone["record"]
        .as_str()
        .ok_or("Record composition absent")?
        .parse()?;
    let record_sequence = saved
        .sequences
        .iter()
        .find(|sequence| sequence.composition == Some(composition))
        .ok_or("Saved record sequence absent")?
        .id;
    for observation in [&before_cut, &after_cut, &playing] {
        if observation["details"]["sequence"] != record_sequence.to_string() {
            return Err("Native cut qualification viewed or played the wrong sequence".into());
        }
    }
    let checkpoint =
        editbay_core::checkpoint(&saved, Some(&copy), directory.join("Verified recovery"))?;
    let recovered = editbay_core::recover_copy(checkpoint, directory.join("Recovered.editbay"))?;
    if recovered.compositions != saved.compositions
        || saved.assets != seed.assets
        || saved.sources != seed.sources
        || hash(&original)? != original_hash
    {
        return Err("Native timeline save/recovery changed identities or original content".into());
    }
    let sent = now();
    choose_master(&mut trace, &destination)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let completed = loop {
        trace.read()?;
        if let Some(job) = trace
            .records
            .iter()
            .rev()
            .filter(|r| {
                r["kind"] == "delivery" && r["unix_us"].as_u64().is_some_and(|time| time >= sent)
            })
            .find_map(|r| {
                r["details"]["jobs"]
                    .as_array()?
                    .last()
                    .filter(|job| job["running"] == false)
            })
        {
            if job["receipt"].is_null() {
                return Err(format!("Timeline export failed: {}", job["error"]).into());
            }
            break job.clone();
        }
        if Instant::now() >= deadline {
            return Err("Timeline export deadline exceeded".into());
        }
        thread::sleep(Duration::from_millis(5));
    };
    set_frame(&mut trace, 12)?;
    trace.focus()?;
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("timeline-native.png").to_str().unwrap(),
        ],
    )?;
    app.kill()?;
    let (mut reopened, mut reopened_trace) = start(
        &binary,
        &directory.join("reopened-state"),
        &catalog,
        &directory.join("reopened.jsonl"),
        Some(&directory.join("Recovered.editbay")),
    )?;
    picture(&mut reopened_trace, 0, 0)?;
    let reopened_timeline = timeline_record(&mut reopened_trace, recovered.revision)?;
    if reopened_timeline["clips"] != final_clips {
        return Err("Native recovered cut changed clip identities".into());
    }
    click_control(&mut reopened_trace, "source-media")?;
    thread::sleep(Duration::from_millis(350));
    click_control(&mut reopened_trace, "timeline-view-record")?;
    reopened_trace.wait("selected recovered cut", |r| {
        r["kind"] == "preview" && r["details"]["sequence"] == record_sequence.to_string()
    })?;
    let reopened_picture = set_frame(&mut reopened_trace, 12)?;
    if reopened_picture["details"]["sequence"] != record_sequence.to_string() {
        return Err("Reopened viewer changed sequences".into());
    }
    reopened_trace.focus()?;
    command(
        "grim",
        &[
            "-g",
            "80,80 1440x900",
            directory.join("timeline-reopened.png").to_str().unwrap(),
        ],
    )?;
    let address = window(|window| {
        window["pid"] == reopened.0.id() && window["class"] == "editbay"
    })?["address"]
        .as_str()
        .ok_or("Reopened native window absent")?
        .to_owned();
    dispatch(&format!(
        "hl.dsp.window.resize({{x=800,y=600,relative=false,window=\"address:{address}\"}})"
    ))?;
    dispatch(&format!(
        "hl.dsp.window.move({{x=80,y=80,relative=false,window=\"address:{address}\"}})"
    ))?;
    thread::sleep(Duration::from_millis(350));
    click_control(&mut reopened_trace, "edit-controls")?;
    let compact_picture = set_frame(&mut reopened_trace, 11)?;
    if compact_picture["details"]["sequence"] != record_sequence.to_string() {
        return Err("Compact viewer changed sequences".into());
    }
    command(
        "grim",
        &[
            "-g",
            "80,80 800x600",
            directory.join("timeline-800x600.png").to_str().unwrap(),
        ],
    )?;
    reopened.kill()?;
    let independent = crate::shared_delivery::inspect(
        &destination,
        &serde_json::from_value(completed["receipt"].clone())?,
    )?;
    let latency = metrics(&mut inputs);
    let qualified = latency["p95_ms"].as_f64().is_some_and(|ms| ms <= 50.);
    let receipt = json!({"kind":"native_timeline_qualification","qualified":qualified,"application_sha256":hash(&binary)?,"original_project_sha256":original_hash,
        "composition":composition,"source":source,"revision":revision,"clips":final_clips,"input_latency":latency,"edits":edits,
        "cut_before":before_cut,"cut_after":after_cut,"playing":playing,"paused":paused,"undo":undone,"redo":redone,
        "saved_recovered_equal":true,"reopened_timeline":reopened_timeline,"reopened_picture":reopened_picture,
        "compact_picture":compact_picture,
        "source_project_unchanged":true,"completed":completed,"independent":independent,
        "limits":["Software-injected native input","No physical audibility, client acceptance or independent-user claim"]});
    File::create_new(directory.join("qualification.json"))?
        .write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    if !qualified {
        return Err("Native timeline input gate missed".into());
    }
    Ok(receipt)
}

fn timeline_record(trace: &mut Trace, revision: u64) -> Result<Value> {
    Ok(trace.wait("prepared native timeline", |r| {
        r["kind"] == "timeline"
            && r["details"]["version"]["revision"] == revision
            && r["details"]["busy"] == false
            && r["details"]["error"].is_null()
    })?["details"]
        .clone())
}
fn set_timeline_input(trace: &mut Trace, name: &str, value: u64) -> Result<()> {
    click_control(trace, name)?;
    click_control(trace, name)?;
    key(30, true, false)?;
    command("wtype", &["-s", "40", &value.to_string(), "-s", "80"])?;
    key(28, false, false)?;
    Ok(())
}
