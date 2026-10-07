use crate::{Result, hash};
use editbay_audio::{
    ClockObservation, MonitorRoute, PlaybackPhase, PlaybackStart, StreamingPlayback,
};
use editbay_core::*;
use editbay_media::{Cancellation, SourceFile, StreamType};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
    time::{Duration, Instant},
};

const MAX_SECONDS: u64 = 7200;
const MAX_POLLS: u64 = 75000;
const MAX_RECEIPT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RSS_KIB: u64 = 4 * 1024 * 1024;
const DRIFT_NS: i128 = 20_000_000;

fn flush_samples(output: &mut BufWriter<File>, recovered: &mut u64) -> Result<()> {
    for attempt in 0..4 {
        match output.flush() {
            Ok(()) => return Ok(()),
            Err(error) if attempt < 3 && error.raw_os_error() == Some(5) => {
                *recovered += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(format!("samples.jsonl flush: {error}").into()),
        }
    }
    unreachable!()
}

struct Playback(StreamingPlayback);
impl Drop for Playback {
    fn drop(&mut self) {
        self.0.stop();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.0.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.0.reap();
    }
}

struct Drift {
    first: Option<ClockObservation>,
    last: Option<ClockObservation>,
    count: u64,
    maximum: i128,
    final_ns: i128,
    host_maximum: i128,
    raw_maximum: i128,
    buckets: [u64; 2001],
}
impl Default for Drift {
    fn default() -> Self {
        Self {
            first: None,
            last: None,
            count: 0,
            maximum: 0,
            final_ns: 0,
            host_maximum: 0,
            raw_maximum: 0,
            buckets: [0; 2001],
        }
    }
}
impl Drift {
    fn record(&mut self, observed: ClockObservation, rate: u32) -> Result<()> {
        if self
            .last
            .is_some_and(|last| observed.callbacks <= last.callbacks)
        {
            return Ok(());
        }
        if observed.backend_elapsed_ns < 1_000_000_000 {
            return Ok(());
        }
        let first = *self.first.get_or_insert(observed);
        if first.backend_epoch != observed.backend_epoch {
            return Err("Backend timestamp epoch changed after settling".into());
        }
        let frames = observed
            .buffer_start_frames
            .checked_sub(first.buffer_start_frames)
            .ok_or("Callback samples regressed")?;
        let sample_ns = i128::from(frames) * 1_000_000_000 / i128::from(rate);
        let backend_ns =
            i128::from(observed.backend_elapsed_ns) - i128::from(first.backend_elapsed_ns);
        let host_ns =
            i128::from(observed.callback_elapsed_ns) - i128::from(first.callback_elapsed_ns);
        let latency_ns =
            i128::from(observed.reported_latency_ns) - i128::from(first.reported_latency_ns);
        let drift = sample_ns - backend_ns - latency_ns;
        self.final_ns = drift;
        self.maximum = self.maximum.max(drift.abs());
        self.host_maximum = self
            .host_maximum
            .max((sample_ns - host_ns - latency_ns).abs());
        self.raw_maximum = self.raw_maximum.max((sample_ns - backend_ns).abs());
        self.buckets[(drift.unsigned_abs().div_ceil(100_000).min(2000)) as usize] += 1;
        self.count += 1;
        self.last = Some(observed);
        Ok(())
    }

    fn receipt(&self) -> Value {
        let percentile = |numerator: u64| -> Option<f64> {
            if self.count == 0 {
                return None;
            }
            let target = (self.count * numerator).div_ceil(100);
            let mut count = 0;
            for (index, values) in self.buckets.iter().enumerate() {
                count += values;
                if count >= target {
                    return (index < 2000).then_some(index as f64 / 10.);
                }
            }
            None
        };
        json!({"observations":self.count,"first":self.first,"last":self.last,
            "settling_backend_seconds":1,"maximum_absolute_ms":self.maximum as f64 / 1e6,
            "final_signed_ms":self.final_ns as f64 / 1e6,
            "host_latency_adjusted_maximum_absolute_ms":self.host_maximum as f64 / 1e6,
            "raw_callback_maximum_absolute_ms":self.raw_maximum as f64 / 1e6,
            "absolute_p50_upper_ms":percentile(50),"absolute_p95_upper_ms":percentile(95),
            "histogram_resolution_ms":0.1,"histogram_overflow_at_ms":200,
            "histogram_overflow_observations":self.buckets[2000],
            "definition":"Elapsed device frames minus the elapsed sum of backend callback time and reported playback latency; relative to the first observed callback after one backend second. Host time and raw callback jitter are reported separately."})
    }
}

fn resources(observed: &mut BTreeSet<u32>) -> Result<u64> {
    let mut pending = vec![std::process::id()];
    let mut visited = BTreeSet::new();
    let mut total = 0;
    while let Some(pid) = pending.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if visited.len() > 512 || pending.len() > 512 {
            return Err("Process tree exceeded 512 entries".into());
        }
        let Some(status) = read_live_proc(&Path::new(&format!("/proc/{pid}")).join("status"))?
        else {
            continue;
        };
        observed.insert(pid);
        if observed.len() > 512 {
            return Err("Observed process set exceeded 512 entries".into());
        }
        total += status
            .lines()
            .find_map(|line| {
                line.strip_prefix("VmRSS:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or(0);
        let tasks = match fs::read_dir(format!("/proc/{pid}/task")) {
            Ok(tasks) => tasks,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("/proc/{pid}/task directory: {error}").into()),
        };
        for task in tasks {
            let task = task.map_err(|error| format!("/proc/{pid}/task entry: {error}"))?;
            if let Some(children) = read_live_proc(&task.path().join("children"))? {
                for child in children.split_whitespace() {
                    if pending.len() >= 512 {
                        return Err("Pending process set exceeded 512 entries".into());
                    }
                    pending.push(child.parse::<u32>()?);
                }
            }
        }
    }
    if total == 0 {
        return Err("Process memory sample is empty".into());
    }
    Ok(total)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct ThreadSchedule {
    process: u32,
    thread: u32,
    name: String,
    policy: u32,
    realtime_priority: u32,
    nice: i32,
}

fn thread_schedules(process: u32) -> Result<Vec<ThreadSchedule>> {
    let tasks = match fs::read_dir(format!("/proc/{process}/task")) {
        Ok(tasks) => tasks,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(format!("/proc/{process}/task directory: {error}").into()),
    };
    let mut result = Vec::new();
    for (index, task) in tasks.enumerate() {
        if index >= 256 {
            return Err("Device thread sample exceeded 256 entries".into());
        }
        let task = task.map_err(|error| format!("/proc/{process}/task entry: {error}"))?;
        let Some(stat) = read_live_proc(&task.path().join("stat"))? else {
            continue;
        };
        result.push(parse_schedule(process, &stat)?);
    }
    Ok(result)
}

/// Read a process sample while its independently retiring task still exists.
/// `path` selects a proc task field; returns none only for a vanished task.
/// A live-task read failure retains its exact path and fails qualification.
fn read_live_proc(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error)
            if matches!(error.raw_os_error(), Some(3 | 5))
                && path.parent().is_some_and(|parent| !parent.exists()) =>
        {
            Ok(None)
        }
        Err(error) => Err(format!("{}: {error}", path.display()).into()),
    }
}

fn parse_schedule(process: u32, stat: &str) -> Result<ThreadSchedule> {
    let (identity, fields) = stat.rsplit_once(") ").ok_or("Thread stat name absent")?;
    let (thread, name) = identity.split_once(" (").ok_or("Thread stat ID absent")?;
    let fields: Vec<_> = fields.split_whitespace().take(64).collect();
    if fields.len() < 39 || name.len() > 64 {
        return Err("Thread stat fields are incomplete or over budget".into());
    }
    Ok(ThreadSchedule {
        process,
        thread: thread.parse()?,
        name: name.into(),
        nice: fields[16].parse()?,
        realtime_priority: fields[37].parse()?,
        policy: fields[38].parse()?,
    })
}

fn host_pressure() -> Result<Value> {
    let memory =
        fs::read_to_string("/proc/meminfo").map_err(|error| format!("/proc/meminfo: {error}"))?;
    let amount = |name: &str| -> Result<u64> {
        Ok(memory
            .lines()
            .find_map(|line| line.strip_prefix(name)?.split_whitespace().next())
            .ok_or("Host memory field absent")?
            .parse()?)
    };
    let total = |path: &str, name: &str| -> Result<u64> {
        let pressure = fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
        Ok(pressure
            .lines()
            .find(|line| line.starts_with(name))
            .and_then(|line| {
                line.split_whitespace()
                    .find_map(|part| part.strip_prefix("total="))
            })
            .ok_or("Host pressure field absent")?
            .parse()?)
    };
    Ok(
        json!({"available_kib":amount("MemAvailable:")?,"swap_free_kib":amount("SwapFree:")?,
        "cpu_stall_us":total("/proc/pressure/cpu", "some ")?,
        "memory_stall_us":total("/proc/pressure/memory", "some ")?,
        "memory_full_stall_us":total("/proc/pressure/memory", "full ")?}),
    )
}

fn author(path: &Path, seconds: u64, directory: &Path) -> Result<(Project, uuid::Uuid, Value)> {
    let cancel = Cancellation::new()?;
    let owned = SourceFile::open(path, &cancel)?;
    let probe = owned.probe(cancel.clone())?;
    let select = |kind| {
        probe
            .streams
            .iter()
            .find(|s| s.kind == kind && s.decoder_available)
            .map(|s| s.index)
            .ok_or("Sustained fixture requires video and sound")
    };
    let video = select(StreamType::Video)?;
    let audio = select(StreamType::Audio)?;
    let imported = owned.ingest(
        "Sustained source".into(),
        &[video, audio],
        cancel.clone(),
        |_, _| {},
    )?;
    let mut project = Project::new("Sustained sound qualification")?;
    project.assets.push(imported.asset);
    project.sources.push(imported.source);
    let commands = sequence_from_video_with_audio(
        &project,
        project.sources[0].id,
        video,
        audio,
        FrameRate::new(30, 1)?,
    )?;
    let mut editor = DocumentEditor::new(project)?;
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Create source sequence".into(),
        &commands,
    )?;
    let source_id = editor.project().sequences[0]
        .composition
        .ok_or("Source composition absent")?;
    let scene = editor
        .project()
        .compositions
        .iter()
        .find(|scene| scene.id == source_id)
        .ok_or("Source composition absent")?;
    let source_duration = scene.duration;
    let rate = scene.frame_rate;
    let frames = (seconds * u64::from(rate.numerator)).div_ceil(u64::from(rate.denominator));
    let count = frames.div_ceil(source_duration);
    if count > 256 {
        return Err(
            "Sustained fixture exceeds 256 linked source intervals; use longer media".into(),
        );
    }
    let mut record = None;
    let mut cursor = 0;
    for _ in 0..count {
        let end = (frames - cursor).min(source_duration);
        let source = SourceSelection {
            composition: source_id,
            range: FrameRange { start: 0, end },
        };
        let action = match record {
            None => TimelineAction::Create {
                name: format!("Sustained {seconds} seconds"),
                source,
            },
            Some(composition) => TimelineAction::Insert {
                composition,
                at: cursor,
                source,
            },
        };
        let change = timeline_edit(editor.project(), &action)?;
        editor.apply(
            DocumentVersion::of(editor.project()),
            "Append sustained source interval".into(),
            &change.commands,
        )?;
        record = Some(change.composition);
        cursor += end;
    }
    let composition = record.ok_or("Sustained composition absent")?;
    let before = editor.project().compositions.clone();
    editor.undo(DocumentVersion::of(editor.project()))?;
    editor.redo(DocumentVersion::of(editor.project()))?;
    if before != editor.project().compositions {
        return Err("Sustained history changed content".into());
    }
    let destination = directory.join("Sustained.editbay");
    save_new(editor.project(), &destination)?;
    let project = load(&destination)?;
    if project.compositions != before
        || timeline_clips(&project, composition)?.len() as u64 != count
    {
        return Err("Sustained reopen changed linked intervals".into());
    }
    owned.verify(&cancel)?;
    let receipt = json!({"source":path.canonicalize()?,"fingerprint":owned.fingerprint(),"video_stream":video,"audio_stream":audio,
        "source_frames":source_duration,"composition":composition,"frames":frames,"rate":rate,"linked_intervals":count,
        "seconds_requested":seconds,"history_and_reopen_exact":true,"project_sha256":hash(&destination)?,"version":DocumentVersion::of(&project)});
    Ok((project, composition, receipt))
}

/// Measure real bounded device playback of an authored repeated source sequence.
/// `path` is preserved media, `seconds` is 3–7200, and `directory` must be new.
/// Returns explicit backend-clock gates; periodic receipts stream to a bounded
/// file. This is neither physical audibility nor simultaneous picture evidence.
pub fn run(path: &Path, seconds: u64, directory: &Path) -> Result<Value> {
    if !(3..=MAX_SECONDS).contains(&seconds) {
        return Err("Sustained duration must be 3–7200 seconds".into());
    }
    fs::create_dir(directory)?;
    let (project, composition, authored) = author(path, seconds, directory)?;
    fs::write(
        directory.join("authored.json"),
        serde_json::to_vec_pretty(&authored)?,
    )?;
    let mut output =
        BufWriter::with_capacity(65536, File::create_new(directory.join("samples.jsonl"))?);
    let mut playback = Playback(StreamingPlayback::start(
        std::sync::Arc::new(project),
        composition,
        PlaybackStart::Frame(0),
        MonitorRoute::Stereo,
    )?);
    let began = Instant::now();
    let mut drift = Drift::default();
    let mut observed = BTreeSet::new();
    let mut schedules = BTreeSet::new();
    let mut schedule_sample = Instant::now() - Duration::from_secs(1);
    let mut callback_min = u64::MAX;
    let mut callback_max = 0;
    let mut peak_rss = 0;
    let mut peak_prepared = 0;
    let mut previous_position = 0;
    let mut polls = 0;
    let mut bytes = 0;
    let mut recovered_storage_errors = 0;
    let mut failure = None;
    let mut last_heartbeat = Instant::now();
    let mut native_first = None;
    let mut native_last = None;
    let final_status = loop {
        let status = playback.0.status();
        let rss = resources(&mut observed)?;
        peak_rss = peak_rss.max(rss);
        if schedule_sample.elapsed() >= Duration::from_secs(1)
            && let Some(pid) = status.device_worker_pid
        {
            schedules.extend(thread_schedules(pid)?);
            if schedules.len() > 1024 {
                return Err("Observed thread schedules exceeded 1024 entries".into());
            }
            schedule_sample = Instant::now();
        }
        peak_prepared = peak_prepared.max(status.prepared_frames);
        if let Some(native) = status.native_output {
            let first = native_first.get_or_insert(native);
            if native.graph_clock_id != first.graph_clock_id
                || native.graph_xrun_ticks != first.graph_xrun_ticks
                || native.graph_flags & 2 != 0
                || native.dequeue_misses != 0
                || native.maximum_ticks_step > native.graph_duration
            {
                failure = Some("Native hardware graph continuity failed");
            }
            native_last = Some(native);
        }
        if let Some(position) = status.position_samples {
            if position < previous_position {
                failure = Some("Sound position regressed");
            }
            previous_position = position;
        }
        if let (Some(clock), Some(device)) = (status.clock_observation, status.device.as_ref()) {
            let frames = clock.submitted_frames - clock.buffer_start_frames;
            callback_min = callback_min.min(frames);
            callback_max = callback_max.max(frames);
            drift.record(clock, device.sample_rate)?;
        }
        let line = serde_json::to_vec(
            &json!({"elapsed_ns":began.elapsed().as_nanos() as u64,"status":status,"combined_rss_kib":rss,"host":host_pressure()?}),
        )?;
        bytes += line.len() as u64 + 1;
        polls += 1;
        if polls > MAX_POLLS || bytes > MAX_RECEIPT_BYTES {
            return Err("Sustained receipt exceeded its declared bound".into());
        }
        output
            .write_all(&line)
            .map_err(|error| format!("samples.jsonl record: {error}"))?;
        output
            .write_all(b"\n")
            .map_err(|error| format!("samples.jsonl newline: {error}"))?;
        flush_samples(&mut output, &mut recovered_storage_errors)?;
        if last_heartbeat.elapsed() >= Duration::from_secs(60) {
            let _ = writeln!(
                std::io::stderr().lock(),
                "Sustained {:.1}s: {:?}, sample {:?}, peak {} KiB",
                began.elapsed().as_secs_f64(),
                status.phase,
                status.position_samples,
                peak_rss
            );
            last_heartbeat = Instant::now();
        }
        if playback.0.is_finished() {
            playback.0.reap();
            break playback.0.status();
        }
        if status.prepared_frames > 16384
            || rss > MAX_RSS_KIB
            || began.elapsed() > Duration::from_secs(seconds + 32)
            || failure.is_some()
        {
            failure = failure.or(Some("Sustained resource or duration limit exceeded"));
            playback.0.stop();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !playback.0.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if !playback.0.reap() {
                return Err("Sustained playback exceeded retirement bound".into());
            }
            break playback.0.status();
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    flush_samples(&mut output, &mut recovered_storage_errors)?;
    output.get_ref().sync_all()?;
    let owned_reaped = observed
        .iter()
        .filter(|pid| **pid != std::process::id())
        .all(|pid| !Path::new(&format!("/proc/{pid}")).exists());
    let source_unchanged = hash(path)?
        == authored["fingerprint"]["sha256"]
            .as_str()
            .ok_or("Source fingerprint absent")?;
    let project_unchanged = hash(&directory.join("Sustained.editbay"))?
        == authored["project_sha256"]
            .as_str()
            .ok_or("Project fingerprint absent")?;
    let complete = final_status.phase == PlaybackPhase::Finished
        && final_status.position_samples == final_status.end_sample;
    let clock_pass = drift.count > 1 && drift.maximum <= DRIFT_NS && drift.host_maximum <= DRIFT_NS;
    let qualified = complete
        && clock_pass
        && failure.is_none()
        && owned_reaped
        && source_unchanged
        && project_unchanged
        && peak_rss > 0
        && peak_rss <= MAX_RSS_KIB
        && peak_prepared <= 16384;
    let receipt = json!({"kind":"sustained_native_sound","authored":authored,"application_sha256":hash(&std::env::current_exe()?)?,
        "elapsed_seconds":began.elapsed().as_secs_f64(),"final":final_status,"drift":drift.receipt(),
        "sampled_callback_frames_min":(callback_min != u64::MAX).then_some(callback_min),
        "sampled_callback_frames_max":callback_max,"device_thread_schedules":schedules,"thread_schedule_sample_interval_ms":1000,
        "sample_interval_ms":100,"samples":polls,"receipt_bytes":bytes,"receipt_limit_bytes":MAX_RECEIPT_BYTES,
        "recovered_storage_write_errors":recovered_storage_errors,
        "prepared_capacity_frames":16384,"peak_prepared_frames":peak_prepared,"peak_combined_rss_kib":peak_rss,
        "processes":observed,"owned_processes_reaped":owned_reaped,"source_unchanged":source_unchanged,"project_unchanged":project_unchanged,
        "complete":complete,"backend_clock_gate":clock_pass,"failure":failure,"qualified":qualified,
        "native_graph_first":native_first,"native_graph_last":native_last,
        "two_hour_run_complete":seconds == MAX_SECONDS && complete && began.elapsed() >= Duration::from_secs(MAX_SECONDS),
        "limits":["Actual native device callbacks; backend and host latency-adjusted clock estimates only. No physical speaker, display, audibility or independent-user proof.",
        "RSS includes the lab and observed descendants, may double count shared memory, and excludes unmapped page cache and unreported driver/device allocations.",
        "One backend second of startup is retained in raw samples and excluded from steady clock drift; 100 ms sampling does not observe every callback or memory spike. Thread schedules are sampled once per second; enabling priority support is not proof of promotion."]});
    fs::write(
        directory.join("qualification.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thread_schedule_handles_parentheses_and_rejects_truncation() {
        let mut fields = vec!["0"; 50];
        fields[0] = "S";
        fields[16] = "-5";
        fields[37] = "20";
        fields[38] = "1";
        let line = format!("42 (device (out)) {}", fields.join(" "));
        let parsed = parse_schedule(7, &line).unwrap();
        assert_eq!(
            parsed,
            ThreadSchedule {
                process: 7,
                thread: 42,
                name: "device (out)".into(),
                policy: 1,
                realtime_priority: 20,
                nice: -5
            }
        );
        assert!(parse_schedule(7, "42 (device) S 1 2").is_err());
        assert!(parse_schedule(7, "42 invalid").is_err());
    }

    #[test]
    fn backend_drift_distinguishes_queue_latency_from_clock_error() {
        let mut drift = Drift::default();
        let first = ClockObservation {
            callbacks: 101,
            buffer_start_frames: 48000,
            submitted_frames: 48480,
            callback_elapsed_ns: 1_010_000_000,
            backend_elapsed_ns: 1_000_000_000,
            backend_epoch: 0,
            reported_latency_ns: 30_000_000,
            valid_end_sample: 480000,
        };
        drift.record(first, 48000).unwrap();
        let mut next = ClockObservation {
            callbacks: 102,
            buffer_start_frames: 48480,
            submitted_frames: 48960,
            callback_elapsed_ns: 1_022_000_000,
            backend_elapsed_ns: 1_012_000_000,
            reported_latency_ns: 28_000_000,
            ..first
        };
        drift.record(next, 48000).unwrap();
        assert_eq!(drift.maximum, 0);
        assert_eq!(drift.raw_maximum, 2_000_000);
        next.callbacks = 103;
        next.buffer_start_frames += 480;
        next.backend_elapsed_ns += 40_000_000;
        next.callback_elapsed_ns += 40_000_000;
        drift.record(next, 48000).unwrap();
        assert_eq!(drift.maximum, 30_000_000);
        assert!(drift.maximum > DRIFT_NS);
        assert_eq!(drift.count, 3);
    }
}
