use crate::workspace::{DocumentOwner, Workspace};
use editbay_core::DocumentEditor;
use editbay_media::{
    MediaProbe, StreamType,
    worker::{self, Request, Response},
};
use eframe::egui;
use std::{
    collections::{HashSet, VecDeque},
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Probe,
    Select,
    Ingest,
    Prepare,
    Verify,
}

struct Task {
    child: Option<Child>,
    input: ChildStdin,
    events: Receiver<Result<Response, String>>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    owner: DocumentOwner,
    editor: Option<DocumentEditor>,
    cancelled: Option<Instant>,
    phase: Phase,
    project_name: String,
}

impl Task {
    fn send(&mut self, request: &Request) -> Result<(), String> {
        worker::write_message(&mut self.input, request).map_err(|e| e.to_string())
    }

    fn cancel(&mut self) {
        if self.cancelled.is_none() {
            let _ = self.send(&Request::Cancel);
            self.cancelled = Some(Instant::now());
        }
    }

    fn failure(&self, error: &str) -> String {
        let detail = self
            .stderr
            .lock()
            .ok()
            .map(|bytes| {
                String::from_utf8_lossy(&bytes.iter().copied().collect::<Vec<_>>()).into_owned()
            })
            .unwrap_or_default();
        if detail.trim().is_empty() {
            error.into()
        } else {
            format!("{error}: {}", detail.trim())
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = std::thread::Builder::new()
                .name("editbay-media-reap".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }
}

#[derive(Default)]
pub struct MediaPane {
    task: Option<Task>,
    probe: Option<MediaProbe>,
    selected: HashSet<u32>,
    preparation: Option<Receiver<Result<DocumentEditor, String>>>,
    prepared: Option<DocumentEditor>,
    status: Option<String>,
    progress: Option<(u32, u64)>,
    pub error: Option<String>,
    pub completed_imports: u64,
}

impl MediaPane {
    /// Observe source job state without granting control.
    /// Takes no arguments. Returns the actual process, phase, owner and outcome
    /// for opt-in native diagnostics; it performs no IO or document mutation.
    pub fn diagnostic_state(&self) -> serde_json::Value {
        serde_json::json!({"worker_pid":self.task.as_ref().and_then(|task| task.child.as_ref().map(Child::id)),
            "phase":self.task.as_ref().map(|task| match task.phase {
                Phase::Probe => "probe", Phase::Select => "select", Phase::Ingest => "ingest",
                Phase::Prepare => "prepare", Phase::Verify => "verify" }),
            "owner":self.task.as_ref().map(|task| task.owner.version),
            "cancelled":self.task.as_ref().is_some_and(|task| task.cancelled.is_some()),
            "progress":self.progress,"completed_imports":self.completed_imports,"error":self.error})
    }

    /// Start one isolated source probe owned by a document session.
    /// `workspace` and `tab` capture revision ownership, `path` is the portal
    /// selection, `executable` is the packaged worker host, and `ctx` wakes input.
    /// Returns after spawning; codec work runs in a separate process.
    pub fn start(
        &mut self,
        workspace: &Workspace,
        tab: uuid::Uuid,
        path: PathBuf,
        executable: &Path,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        if self.task.is_some() {
            return Err("Finish or cancel the current media import first".into());
        }
        let (owner, editor) = workspace.edit_snapshot(tab)?;
        let mut child = Command::new(executable)
            .arg("--media-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let input = child
            .stdin
            .take()
            .ok_or("Media worker has no control pipe")?;
        let output = child
            .stdout
            .take()
            .ok_or("Media worker has no result pipe")?;
        let mut error_output = child
            .stderr
            .take()
            .ok_or("Media worker has no error pipe")?;
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr.clone();
        std::thread::Builder::new()
            .name("editbay-media-errors".into())
            .spawn(move || {
                let mut buffer = [0; 4096];
                while let Ok(count) = error_output.read(&mut buffer) {
                    if count == 0 {
                        break;
                    }
                    if let Ok(mut tail) = tail.lock() {
                        tail.extend(&buffer[..count]);
                        while tail.len() > 8192 {
                            tail.pop_front();
                        }
                    }
                }
            })
            .map_err(|e| {
                let _ = child.kill();
                let _ = child.wait();
                e.to_string()
            })?;
        let (sender, events) = mpsc::sync_channel(16);
        let wake = ctx.clone();
        std::thread::Builder::new()
            .name("editbay-media-results".into())
            .spawn(move || {
                let mut reader = BufReader::new(output);
                loop {
                    let event = match worker::read_message(&mut reader) {
                        Ok(Some(bytes)) => {
                            serde_json::from_slice::<Response>(&bytes).map_err(|e| e.to_string())
                        }
                        Ok(None) => break,
                        Err(error) => Err(error.to_string()),
                    };
                    let terminal = !matches!(&event, Ok(Response::Progress { .. }));
                    if terminal {
                        if sender.send(event).is_err() {
                            break;
                        }
                    } else if matches!(
                        sender.try_send(event),
                        Err(mpsc::TrySendError::Disconnected(_))
                    ) {
                        break;
                    }
                    wake.request_repaint();
                }
                drop(sender);
                wake.request_repaint();
            })
            .map_err(|e| {
                let _ = child.kill();
                let _ = child.wait();
                e.to_string()
            })?;
        let project_name = editor.project().name.clone();
        let mut task = Task {
            child: Some(child),
            input,
            events,
            stderr,
            owner,
            editor: Some(editor),
            cancelled: None,
            phase: Phase::Probe,
            project_name,
        };
        task.send(&Request::Probe { path })?;
        self.task = Some(task);
        self.error = None;
        self.probe = None;
        self.selected.clear();
        self.preparation = None;
        self.prepared = None;
        self.status = Some("Inspecting source…".into());
        self.progress = None;
        Ok(())
    }

    /// Inspect whether source work is still active.
    /// Takes no arguments. Returns true through cancellation and preparation.
    pub fn busy(&self) -> bool {
        self.task.is_some()
    }

    /// Distinguish active worker operations from an idle stream choice.
    /// Takes no arguments. Returns true while actual work or cancellation runs.
    pub fn working(&self) -> bool {
        self.task
            .as_ref()
            .is_some_and(|task| task.phase != Phase::Select || task.cancelled.is_some())
    }

    /// Inspect the source returned by the actual native worker.
    /// Takes no arguments. Returns bounded metadata while the import owns it.
    pub fn inspected(&self) -> Option<&MediaProbe> {
        self.probe.as_ref()
    }

    /// Choose explicit supported container streams for the current source.
    /// `indices` contains distinct picture/sound indices. Returns a visible
    /// error for missing, duplicate, unsupported or obsolete selections.
    pub fn select_streams(&mut self, indices: &[u32]) -> Result<(), String> {
        let probe = self
            .probe
            .as_ref()
            .ok_or("Inspect a source before selecting streams")?;
        if self
            .task
            .as_ref()
            .is_none_or(|task| task.phase != Phase::Select)
        {
            return Err("Media worker is no longer choosing streams".into());
        }
        let selected: HashSet<_> = indices.iter().copied().collect();
        if selected.len() != indices.len()
            || selected.is_empty()
            || selected.iter().any(|index| {
                !probe.streams.iter().any(|s| {
                    s.index == *index
                        && s.decoder_available
                        && matches!(s.kind, StreamType::Video | StreamType::Audio)
                })
            })
        {
            return Err("Choose distinct supported picture or sound streams".into());
        }
        self.selected = selected;
        Ok(())
    }

    /// Import the user's current source selection through the native worker.
    /// Takes no arguments. Returns after sending the bounded request; actual
    /// decode, validation and source verification precede document publication.
    pub fn import_selected(&mut self) -> Result<(), String> {
        let probe = self
            .probe
            .as_ref()
            .ok_or("Inspect a source before importing")?;
        let task = self
            .task
            .as_mut()
            .ok_or("Media worker is no longer active")?;
        if task.phase != Phase::Select || self.selected.is_empty() {
            return Err("Choose source streams before importing".into());
        }
        let mut streams: Vec<_> = self.selected.iter().copied().collect();
        streams.sort_unstable();
        let name = probe
            .path
            .file_name()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Media source".into());
        task.send(&Request::Ingest { name, streams })?;
        task.phase = Phase::Ingest;
        self.status = Some("Reading source streams…".into());
        Ok(())
    }

    /// Cancel source work without changing the document.
    /// Takes no arguments. Native cancellation is requested immediately; a worker
    /// that does not stop within two seconds is killed and reaped off the UI thread.
    pub fn cancel(&mut self) {
        if let Some(task) = &mut self.task {
            task.cancel();
        }
        self.probe = None;
        self.status = Some("Cancelling media work…".into());
    }

    fn clear(&mut self) {
        self.task = None;
        self.probe = None;
        self.selected.clear();
        self.preparation = None;
        self.prepared = None;
        self.status = None;
        self.progress = None;
    }

    /// Accept worker results only for their captured session and revision.
    /// `workspace` receives an already validated editor; `ctx` schedules input.
    /// Returns no value; stale, cancelled, crashed and malformed work stays visible.
    pub fn poll(&mut self, workspace: &mut Workspace, ctx: &egui::Context) {
        if let Some(task) = &mut self.task {
            if !workspace.owns(task.owner) && task.cancelled.is_none() {
                self.error = Some(
                    "The project changed during media import; the result was discarded".into(),
                );
                task.cancel();
                self.probe = None;
            }
            if task
                .cancelled
                .is_some_and(|since| since.elapsed() >= Duration::from_secs(2))
            {
                self.clear();
                return;
            }
        }
        if let Some(receiver) = &self.preparation {
            match receiver.try_recv() {
                Ok(Ok(editor)) => {
                    self.preparation = None;
                    if self
                        .task
                        .as_ref()
                        .is_some_and(|task| task.cancelled.is_none())
                    {
                        self.prepared = Some(editor);
                        self.status = Some("Verifying source bytes…".into());
                        if let Some(task) = &mut self.task {
                            if let Err(error) = task.send(&Request::Verify) {
                                self.error = Some(error);
                                self.clear();
                                return;
                            }
                            task.phase = Phase::Verify;
                        }
                    }
                }
                Ok(Err(error)) => {
                    self.error = Some(error);
                    self.clear();
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("Media command worker stopped before publication".into());
                    self.clear();
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        for _ in 0..32 {
            let Some(task) = &mut self.task else {
                break;
            };
            let event = task.events.try_recv();
            if task.cancelled.is_some() {
                if matches!(
                    event,
                    Err(mpsc::TryRecvError::Disconnected) | Ok(Ok(Response::Failed { .. }))
                ) {
                    self.clear();
                }
                break;
            }
            let expected_response = match &event {
                Ok(Ok(Response::Probed { .. })) => task.phase == Phase::Probe,
                Ok(Ok(Response::Progress { .. } | Response::Ingested { .. })) => {
                    task.phase == Phase::Ingest
                }
                Ok(Ok(Response::Verified { .. })) => task.phase == Phase::Verify,
                _ => true,
            };
            if !expected_response {
                self.error = Some("Media worker returned a result out of order".into());
                self.clear();
                break;
            }
            match event {
                Ok(Ok(Response::Probed { probe })) => {
                    self.selected = probe
                        .streams
                        .iter()
                        .filter(|s| {
                            s.decoder_available
                                && matches!(s.kind, StreamType::Video | StreamType::Audio)
                        })
                        .map(|s| s.index)
                        .collect();
                    task.phase = Phase::Select;
                    self.status = None;
                    self.probe = Some(probe);
                }
                Ok(Ok(Response::Progress { stream, completed })) => {
                    self.progress = Some((stream, completed));
                    let units = self
                        .probe
                        .as_ref()
                        .and_then(|p| p.streams.iter().find(|s| s.index == stream))
                        .map_or("items", |s| {
                            if s.kind == StreamType::Video {
                                "pictures"
                            } else {
                                "samples"
                            }
                        });
                    self.status = Some(format!("Reading stream {stream} · {completed} {units}"));
                }
                Ok(Ok(Response::Ingested { imported })) => {
                    let matches_source = self.probe.as_ref().is_some_and(|probe| {
                        imported.asset.path == probe.path
                            && imported.asset.sha256 == probe.fingerprint.sha256
                            && imported.asset.bytes == probe.fingerprint.bytes
                            && imported.source.streams.len() == self.selected.len()
                            && imported
                                .source
                                .streams
                                .iter()
                                .all(|stream| self.selected.contains(&stream.index))
                    });
                    if !matches_source {
                        self.error =
                            Some("Media worker returned different sources or streams".into());
                        self.clear();
                        break;
                    }
                    let Some(mut editor) = task.editor.take() else {
                        self.error = Some("Media worker returned a duplicate import".into());
                        self.clear();
                        break;
                    };
                    let expected = task.owner.version;
                    let (sender, receiver) = mpsc::sync_channel(1);
                    let wake = ctx.clone();
                    match std::thread::Builder::new().name("editbay-media-commands".into()).spawn(move || {
                        let result = (|| {
                            editor.apply(expected, "Import media".into(), &imported.commands()).map_err(|e| e.to_string())?;
                            let bytes = serde_json::to_vec(editor.project()).map_err(|e| e.to_string())?;
                            if bytes.len() > 8 * 1024 * 1024 { return Err("Imported document exceeds the native workspace's 8 MiB budget".into()); }
                            Ok(editor)
                        })();
                        let _ = sender.send(result); wake.request_repaint();
                    }) {
                        Ok(_) => { task.phase = Phase::Prepare; self.preparation = Some(receiver); self.status = Some("Preparing editable media…".into()); }
                        Err(error) => { self.error = Some(error.to_string()); self.clear(); break; }
                    }
                }
                Ok(Ok(Response::Verified { fingerprint })) => {
                    let expected = self.probe.as_ref().map(|probe| &probe.fingerprint);
                    if expected != Some(&fingerprint) {
                        self.error = Some("Media worker verified different source bytes".into());
                        self.clear();
                        break;
                    }
                    let owner = task.owner;
                    if let Some(editor) = self.prepared.take() {
                        match workspace.commit_edit(owner, editor) {
                            Ok(()) => self.completed_imports += 1,
                            Err(error) => self.error = Some(error),
                        }
                    } else {
                        self.error =
                            Some("Media verification arrived before its editable document".into());
                    }
                    self.clear();
                    break;
                }
                Ok(Ok(Response::Failed { error, .. })) | Ok(Err(error)) => {
                    self.error = Some(task.failure(&error));
                    self.clear();
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error =
                        Some(task.failure("Native media worker stopped before completing import"));
                    self.clear();
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if self
            .task
            .as_ref()
            .is_some_and(|task| task.phase != Phase::Select || task.cancelled.is_some())
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    /// Display real source selection, progress and cancellation.
    /// `ui` is the native workspace. Returns no value; Add source sends only the
    /// explicitly selected supported streams to the owned child process.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let mut import = false;
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(status) = &self.status {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(status);
            });
        }
        if let Some(task) = &self.task {
            ui.weak(format!("Media import · {}", task.project_name));
        }
        if let Some(probe) = &self.probe
            && self.status.is_none()
        {
            ui.group(|ui| {
                ui.heading("Choose source streams");
                ui.label(probe.path.display().to_string());
                ui.weak(format!(
                    "{} bytes · SHA-256 {}",
                    probe.fingerprint.bytes,
                    &probe.fingerprint.sha256[..12.min(probe.fingerprint.sha256.len())]
                ));
                egui::ScrollArea::vertical()
                    .max_height(240.)
                    .show(ui, |ui| {
                        for stream in &probe.streams {
                            let supported = stream.decoder_available
                                && matches!(stream.kind, StreamType::Video | StreamType::Audio);
                            let mut selected = self.selected.contains(&stream.index);
                            let mut description = match stream.kind {
                                StreamType::Video => format!(
                                    "{} × {} · nominal {} fps",
                                    stream.width.unwrap_or(0),
                                    stream.height.unwrap_or(0),
                                    stream
                                        .nominal_rate
                                        .map(|r| format!("{}/{}", r.numerator, r.denominator))
                                        .unwrap_or_else(|| "unknown".into())
                                ),
                                StreamType::Audio => format!(
                                    "{} Hz · {}",
                                    stream.sample_rate.unwrap_or(0),
                                    stream.channels.join(" / ")
                                ),
                                other => format!("{other:?} · unsupported for ingest"),
                            };
                            if !stream.decoder_available {
                                description.push_str(" · decoder unavailable");
                            }
                            if ui
                                .add_enabled(
                                    supported,
                                    egui::Checkbox::new(
                                        &mut selected,
                                        format!(
                                            "Stream {} · {} · {description}",
                                            stream.index, stream.codec
                                        ),
                                    ),
                                )
                                .changed()
                            {
                                if selected {
                                    self.selected.insert(stream.index);
                                } else {
                                    self.selected.remove(&stream.index);
                                }
                            }
                            if stream.alpha_interpretation_required {
                                ui.weak("Source alpha is unspecified; imported as straight alpha.");
                            }
                        }
                    });
                if ui
                    .add_enabled(!self.selected.is_empty(), egui::Button::new("Add source"))
                    .clicked()
                {
                    import = true;
                }
            });
        }
        if import && let Err(error) = self.import_selected() {
            self.error = Some(error);
        }
        if self.task.is_some() && ui.button("Cancel media import").clicked() {
            self.cancel();
        }
    }
}
