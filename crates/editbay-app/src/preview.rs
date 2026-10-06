use crate::workspace::{DocumentOwner, Workspace};
use editbay_core::{
    DocumentCommand, DocumentEditor, DocumentVersion, EvaluationSnapshot, Project, SourcePosition,
};
use editbay_media::{
    Cancellation,
    picture_worker::{PictureWorker, WorkerBudget},
};
use editbay_render::{
    DisplayFrame, DisplayRenderer, GraphBudget, GraphRenderer, GraphStats, ImageBoundary,
};
use eframe::egui;
use egui_wgpu::wgpu;
use std::{
    collections::HashMap,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[derive(Clone)]
struct Gpu {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
}

#[cfg(test)]
mod tests {
    use super::*;
    use editbay_core::{
        AlphaMode, AssetKind, AssetReference, FrameRate, MediaSource, PictureTiming, SourceColor,
        SourceStream, StreamFormat, TimeBase,
    };
    use std::collections::BTreeMap;

    fn workspace() -> (tempfile::TempDir, Workspace, Uuid) {
        let directory = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(directory.path().join("recovery"), ctx);
        let mut project = Project::new("Background source authoring").unwrap();
        let asset = Uuid::new_v4();
        project.assets.push(AssetReference {
            id: asset,
            kind: AssetKind::Media,
            path: "Typed fixture.mkv".into(),
            bytes: 1,
            sha256: "a".repeat(64),
            provenance: "Document fixture only".into(),
        });
        project.sources.push(MediaSource {
            id: Uuid::new_v4(),
            name: "Video".into(),
            asset,
            metadata: BTreeMap::new(),
            streams: vec![SourceStream {
                index: 0,
                codec: "ffv1".into(),
                time_base: TimeBase {
                    numerator: 1,
                    denominator: 1000,
                },
                start_tick: 0,
                duration_ticks: Some(80),
                metadata: BTreeMap::new(),
                format: StreamFormat::Video {
                    width: 8,
                    height: 6,
                    sample_aspect: FrameRate::new(1, 1).unwrap(),
                    timing: PictureTiming::Variable {
                        presentation_ticks: vec![0, 40],
                        end_tick: 80,
                    },
                    color: SourceColor {
                        primaries: 1,
                        transfer: 1,
                        matrix: 1,
                        range: 1,
                    },
                    alpha: AlphaMode::Opaque,
                },
            }],
        });
        let tab = workspace.create(project).unwrap();
        (directory, workspace, tab)
    }

    fn finish(pane: &mut PreviewPane, workspace: &mut Workspace, visible: bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while pane.creation.is_some() {
            pane.poll(workspace, visible, &egui::Context::default());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn native_background_authoring_commits_one_group_and_undo_preserves_sources() {
        let (_directory, mut workspace, tab) = workspace();
        let source = workspace.tabs[0].editor.project().sources[0].id;
        let originals = workspace.tabs[0].editor.project().sources.clone();
        let mut pane = PreviewPane::default();
        pane.create_sequence(&workspace, tab, source, 0, None)
            .unwrap();
        finish(&mut pane, &mut workspace, true);
        assert!(pane.error.is_none());
        assert_eq!(workspace.tabs[0].editor.project().revision, 1);
        assert_eq!(workspace.tabs[0].editor.project().compositions.len(), 1);
        assert_eq!(workspace.tabs[0].editor.project().sequences.len(), 1);
        let owner = DocumentVersion::of(workspace.tabs[0].editor.project());
        workspace.tabs[0].editor.undo(owner).unwrap();
        assert!(workspace.tabs[0].editor.project().compositions.is_empty());
        assert_eq!(workspace.tabs[0].editor.project().sources, originals);
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |ui| {
            pane.show(ui, &workspace, tab, workspace.tabs[0].editor.snapshot());
        })
        .drop_without_applying_deltas();
        assert_eq!(pane.view_owner.unwrap().version.revision, 2);
    }

    #[test]
    fn finished_worker_keeps_its_specific_failure_during_concurrent_polling() {
        for _ in 0..256 {
            let result = Arc::new(Mutex::new(None));
            let output = result.clone();
            let thread = std::thread::spawn(move || {
                *output.lock().unwrap() = Some(Event::Failed("Source color is undeclared".into()));
            });
            loop {
                if let Some(event) = worker_event(&result, Some(&thread)) {
                    assert!(
                        matches!(event, Event::Failed(error) if error == "Source color is undeclared")
                    );
                    break;
                }
                std::thread::yield_now();
            }
            thread.join().unwrap();
        }
        let thread = std::thread::spawn(|| {});
        while !thread.is_finished() {
            std::thread::yield_now();
        }
        assert!(
            matches!(worker_event(&Mutex::new(None), Some(&thread)), Some(Event::Failed(error)) if error.contains("worker stopped"))
        );
        thread.join().unwrap();
    }

    #[test]
    fn authoring_after_edit_tab_switch_or_welcome_cannot_publish() {
        for change in 0..3 {
            let (_directory, mut workspace, tab) = workspace();
            let source = workspace.tabs[0].editor.project().sources[0].id;
            let mut pane = PreviewPane::default();
            pane.create_sequence(&workspace, tab, source, 0, None)
                .unwrap();
            match change {
                0 => {
                    let owner = DocumentVersion::of(workspace.tabs[0].editor.project());
                    workspace
                        .apply(
                            tab,
                            owner,
                            "Rename".into(),
                            &[DocumentCommand::RenameProject {
                                name: "Changed".into(),
                            }],
                        )
                        .unwrap();
                }
                1 => {
                    workspace
                        .create(Project::new("Other tab").unwrap())
                        .unwrap();
                }
                _ => {}
            }
            finish(&mut pane, &mut workspace, change != 2);
            assert!(
                pane.error
                    .as_ref()
                    .is_some_and(|error| error.contains("changed"))
            );
            assert!(workspace.tabs[0].editor.project().compositions.is_empty());
        }
    }
}
#[derive(Clone, Copy)]
struct Selection {
    sequence: Uuid,
    frame: u64,
}
struct Request {
    serial: u64,
    frame: u64,
    requested: Instant,
}
#[derive(Default)]
struct Mailbox {
    request: Option<Request>,
    stopped: bool,
}
struct Picture {
    draw: Arc<DisplayFrame>,
    serial: u64,
    frame: u64,
    requested: Instant,
    preparation_us: u64,
    completion_us: AtomicU64,
    incompatible: AtomicBool,
    stats: GraphStats,
    worker_pid: Option<u32>,
}
enum Event {
    Started(u32),
    Picture(Arc<Picture>),
    Failed(String),
}
struct Task {
    owner: DocumentOwner,
    sequence: Uuid,
    cancel: Cancellation,
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
    result: Arc<Mutex<Option<Event>>>,
    thread: Option<JoinHandle<()>>,
}

/// Read a worker's final event without racing its exit.
/// `result` owns the single event slot and `thread` reports completion. Returns
/// the published event first, or an explicit failure after an empty worker exit.
fn worker_event(result: &Mutex<Option<Event>>, thread: Option<&JoinHandle<()>>) -> Option<Event> {
    let mut result = match result.lock() {
        Ok(result) => result,
        Err(error) => return Some(Event::Failed(error.to_string())),
    };
    result.take().or_else(|| {
        thread
            .filter(|thread| thread.is_finished())
            .map(|_| Event::Failed("Viewer worker stopped; retry the viewer".into()))
    })
}

impl Task {
    fn stop(&self) {
        self.cancel.cancel();
        if let Ok(mut mailbox) = self.mailbox.0.lock() {
            mailbox.stopped = true;
            mailbox.request = None;
        }
        self.mailbox.1.notify_one();
    }
    fn request(&self, request: Request) -> Result<(), String> {
        let mut mailbox = self.mailbox.0.lock().map_err(|e| e.to_string())?;
        if mailbox.stopped {
            return Err("Viewer is stopping".into());
        }
        mailbox.request = Some(request);
        self.mailbox.1.notify_one();
        Ok(())
    }
    fn start(
        owner: DocumentOwner,
        project: Arc<Project>,
        sequence: Uuid,
        gpu: Gpu,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        if owner.version != DocumentVersion::of(&project) {
            return Err("Viewer document ownership changed before startup".into());
        }
        let cancel = Cancellation::new().map_err(|e| e.to_string())?;
        let mailbox = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let result = Arc::new(Mutex::new(None));
        let input = mailbox.clone();
        let output = result.clone();
        let token = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("editbay-native-picture".into())
            .spawn(move || {
                let publish = |event| {
                    if let Ok(mut result) = output.lock() {
                        *result = Some(event);
                    }
                    ctx.request_repaint();
                };
                let run = || -> Result<(), String> {
                    let snapshot =
                        Arc::new(EvaluationSnapshot::new(project).map_err(|e| e.to_string())?);
                    let composition = snapshot
                        .project()
                        .sequences
                        .iter()
                        .find(|item| item.id == sequence)
                        .and_then(|item| item.composition)
                        .ok_or("Sequence has no picture composition")?;
                    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
                    let provider = PictureWorker::new(
                        &executable,
                        snapshot.clone(),
                        WorkerBudget::default(),
                        token.clone(),
                    )
                    .map_err(|e| e.to_string())?;
                    if let Some(pid) = provider.process_id() {
                        publish(Event::Started(pid));
                    }
                    let display = DisplayRenderer::new(gpu.device.clone(), gpu.format)
                        .map_err(|e| e.to_string())?;
                    let mut graph = GraphRenderer::with_device(
                        snapshot,
                        provider,
                        GraphBudget::default(),
                        token.clone(),
                        &gpu.adapter,
                        gpu.device,
                        gpu.queue,
                    )
                    .map_err(|e| e.to_string())?;
                    loop {
                        if token.is_cancelled() {
                            return Ok(());
                        }
                        let request = {
                            let mut mailbox = input.0.lock().map_err(|e| e.to_string())?;
                            if mailbox.request.is_none() && !mailbox.stopped {
                                mailbox = input
                                    .1
                                    .wait_timeout(mailbox, Duration::from_millis(100))
                                    .map_err(|e| e.to_string())?
                                    .0;
                            }
                            if mailbox.stopped {
                                return Ok(());
                            }
                            mailbox.request.take()
                        };
                        if let Some(request) = request {
                            let working = graph
                                .render(
                                    composition,
                                    SourcePosition::new(
                                        i64::try_from(request.frame).map_err(|e| e.to_string())?,
                                        1,
                                    )
                                    .map_err(|e| e.to_string())?,
                                    false,
                                )
                                .map_err(|e| e.to_string())?;
                            let converted = graph
                                .convert(&working, ImageBoundary::Display)
                                .map_err(|e| e.to_string())?;
                            let draw = Arc::new(
                                graph
                                    .present(&converted, &display)
                                    .map_err(|e| e.to_string())?,
                            );
                            graph.finish().map_err(|e| e.to_string())?;
                            if !token.is_cancelled() {
                                publish(Event::Picture(Arc::new(Picture {
                                    draw,
                                    serial: request.serial,
                                    frame: request.frame,
                                    requested: request.requested,
                                    preparation_us: request.requested.elapsed().as_micros() as u64,
                                    completion_us: AtomicU64::new(0),
                                    incompatible: AtomicBool::new(false),
                                    stats: graph.stats(),
                                    worker_pid: graph.picture_provider().process_id(),
                                })));
                            }
                        }
                        graph.poll().map_err(|e| e.to_string())?;
                    }
                };
                if let Err(error) = run() {
                    publish(Event::Failed(error));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            owner,
            sequence,
            cancel,
            mailbox,
            result,
            thread: Some(thread),
        })
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Callback {
    picture: Arc<Picture>,
    wake: egui::Context,
    compatible: AtomicBool,
}
impl egui_wgpu::CallbackTrait for Callback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        _: &wgpu::Queue,
        _: &egui_wgpu::ScreenDescriptor,
        _: &mut wgpu::CommandEncoder,
        _: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let compatible = self.picture.draw.accepts_device(device);
        self.compatible.store(compatible, Ordering::Release);
        if !compatible {
            self.picture.incompatible.store(true, Ordering::Release);
            self.wake.request_repaint();
        }
        Vec::new()
    }
    fn paint(
        &self,
        _: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _: &egui_wgpu::CallbackResources,
    ) {
        if !self.compatible.load(Ordering::Acquire) {
            return;
        }
        let picture = self.picture.clone();
        let wake = self.wake.clone();
        self.picture.draw.paint(pass, move || {
            if picture
                .completion_us
                .compare_exchange(
                    0,
                    picture.requested.elapsed().as_micros() as u64,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                wake.request_repaint();
            }
        });
    }
}

struct Creation {
    owner: DocumentOwner,
    result: Receiver<Result<(DocumentEditor, Uuid), String>>,
}

#[derive(Default)]
pub struct PreviewPane {
    view_owner: Option<DocumentOwner>,
    gpu: Option<Gpu>,
    selections: HashMap<Uuid, Selection>,
    source_audio: HashMap<(Uuid, Uuid), Option<u32>>,
    task: Option<Task>,
    retiring: Vec<Task>,
    picture: Option<Arc<Picture>>,
    creation: Option<Creation>,
    serial: u64,
    requested_frame: Option<u64>,
    request_accepted_unix_us: Option<u64>,
    worker_pid: Option<u32>,
    stopped: bool,
    error: Option<String>,
    rejected: u64,
    controls: HashMap<String, [f32; 4]>,
}

impl PreviewPane {
    /// Reset opt-in layout observations for the current native pass.
    /// Takes no arguments. Returns no value and performs no filesystem IO.
    pub fn begin_frame(&mut self) {
        self.controls.clear();
    }

    /// Observe the clipped geometry of a real native control.
    /// `name`, `response` and `ui` identify a rendered control. Returns no value;
    /// observations grant no command authority and exclude offscreen controls.
    pub fn observe_control(&mut self, name: &str, response: &egui::Response, ui: &egui::Ui) {
        let rect = response.rect.intersect(ui.clip_rect());
        if rect.width() > 4. && rect.height() > 4. {
            self.controls.insert(
                name.into(),
                [rect.min.x, rect.min.y, rect.max.x, rect.max.y],
            );
        }
    }
    /// Attach the actual native render state without compiling or decoding.
    /// `state` belongs to eframe's window. Returns no value; all expensive work
    /// begins on the owning preview thread after a sequence is selected.
    pub fn attach(&mut self, state: &egui_wgpu::RenderState) {
        self.gpu = Some(Gpu {
            adapter: state.adapter.clone(),
            device: state.device.clone(),
            queue: state.queue.clone(),
            format: state.target_format,
        });
    }

    /// Choose the imported sound stream for a new source sequence.
    /// `ui`, `tab` and `source` identify this native media row; returns the
    /// selected stream or an explicit picture-only choice without changing assets.
    pub fn source_sound(
        &mut self,
        ui: &mut egui::Ui,
        tab: Uuid,
        source: &editbay_core::MediaSource,
    ) -> Option<u32> {
        let streams: Vec<_> = source
            .streams
            .iter()
            .filter(|s| matches!(s.format, editbay_core::StreamFormat::Audio { .. }))
            .collect();
        if streams.is_empty() {
            return None;
        }
        ui.label("Sequence sound");
        let selected = self
            .source_audio
            .entry((tab, source.id))
            .or_insert_with(|| (streams.len() == 1).then_some(streams[0].index));
        if selected.is_some_and(|index| !streams.iter().any(|stream| stream.index == index)) {
            *selected = None;
        }
        let label = |stream: &editbay_core::SourceStream| {
            let editbay_core::StreamFormat::Audio {
                sample_rate,
                channels,
            } = &stream.format
            else {
                unreachable!()
            };
            format!(
                "Stream {} · {} Hz · {}",
                stream.index,
                sample_rate,
                channels.join(" / ")
            )
        };
        let text = selected
            .and_then(|index| streams.iter().find(|s| s.index == index))
            .map_or_else(|| "Picture only".into(), |s| label(s));
        let response = egui::ComboBox::from_id_salt(("sequence-sound", tab, source.id))
            .selected_text(text)
            .show_ui(ui, |ui| {
                ui.selectable_value(selected, None, "Picture only");
                for stream in streams {
                    ui.selectable_value(selected, Some(stream.index), label(stream));
                }
            })
            .response;
        let choice = *selected;
        self.observe_control(&format!("sequence-sound:{}", source.id), &response, ui);
        choice
    }

    /// Prepare one source sequence through the shared command and undo path.
    /// `workspace`, `tab`, `source` and `stream` capture existing imported media.
    /// Returns after starting background validation; stale results cannot mutate a tab.
    pub fn create_sequence(
        &mut self,
        workspace: &Workspace,
        tab: Uuid,
        source: Uuid,
        stream: u32,
        audio: Option<u32>,
    ) -> Result<(), String> {
        if self.creation.is_some() {
            return Err("A sequence is already being prepared".into());
        }
        let (owner, mut editor) = workspace.edit_snapshot(tab)?;
        let (sender, result) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("editbay-sequence-command".into())
            .spawn(move || {
                let run = || -> Result<(DocumentEditor, Uuid), String> {
                    let rate = editor
                        .project()
                        .sequences
                        .first()
                        .ok_or("Project needs a sequence profile")?
                        .frame_rate;
                    let commands = match audio {
                        Some(audio) => editbay_core::sequence_from_video_with_audio(
                            editor.project(),
                            source,
                            stream,
                            audio,
                            rate,
                        ),
                        None => editbay_core::sequence_from_video(
                            editor.project(),
                            source,
                            stream,
                            rate,
                        ),
                    }
                    .map_err(|e| e.to_string())?;
                    let sequence = commands
                        .iter()
                        .find_map(|command| match command {
                            DocumentCommand::SetSequence { sequence } => Some(sequence.id),
                            _ => None,
                        })
                        .ok_or("Sequence command is absent")?;
                    editor
                        .apply(owner.version, "Create source sequence".into(), &commands)
                        .map_err(|e| e.to_string())?;
                    Ok((editor, sequence))
                };
                let _ = sender.send(run());
            })
            .map_err(|e| e.to_string())?;
        self.creation = Some(Creation { owner, result });
        self.error = None;
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.stop();
            self.retiring.push(task);
        }
        self.picture = None;
        self.requested_frame = None;
        self.request_accepted_unix_us = None;
        self.worker_pid = None;
    }

    fn observe_owner(&mut self, owner: Option<DocumentOwner>) {
        if owner != self.view_owner {
            self.stop();
            self.stopped = false;
            self.error = None;
            self.view_owner = owner;
        }
    }

    /// Receive worker outcomes and invalidate obsolete session ownership.
    /// `workspace` supplies the live command owner, `visible` is the workspace
    /// view, and `ctx` receives repaint requests. Returns without codec/GPU waits.
    pub fn poll(&mut self, workspace: &mut Workspace, visible: bool, ctx: &egui::Context) {
        let owner = if visible {
            workspace
                .active
                .and_then(|tab| workspace.edit_snapshot(tab).ok().map(|(owner, _)| owner))
        } else {
            None
        };
        self.observe_owner(owner);
        self.retiring.retain_mut(|task| {
            if task.thread.as_ref().is_some_and(JoinHandle::is_finished) {
                if let Some(thread) = task.thread.take() {
                    let _ = thread.join();
                }
                false
            } else {
                true
            }
        });
        if !self.retiring.is_empty() || self.creation.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if let Some(creation) = &self.creation {
            match creation.result.try_recv() {
                Ok(result) => {
                    let owner = creation.owner;
                    self.creation = None;
                    match result {
                        Ok((editor, sequence))
                            if visible
                                && workspace.active == Some(owner.tab)
                                && workspace.owns(owner) =>
                        {
                            match workspace.commit_edit(owner, editor) {
                                Ok(()) => {
                                    self.selections
                                        .insert(owner.tab, Selection { sequence, frame: 0 });
                                    self.stopped = false;
                                    self.view_owner = workspace
                                        .edit_snapshot(owner.tab)
                                        .ok()
                                        .map(|(owner, _)| owner);
                                }
                                Err(error) => self.error = Some(error),
                            }
                        }
                        Ok(_) => {
                            self.error = Some(
                                "Project changed while preparing this sequence; create it again"
                                    .into(),
                            )
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.creation = None;
                    self.error =
                        Some("Sequence worker stopped before returning its command".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        self.selections
            .retain(|id, _| workspace.tabs.iter().any(|tab| tab.id == *id));
        self.source_audio
            .retain(|(id, _), _| workspace.tabs.iter().any(|tab| tab.id == *id));
        if self.task.as_ref().is_some_and(|task| {
            !visible || workspace.active != Some(task.owner.tab) || !workspace.owns(task.owner)
        }) {
            self.stop();
            self.stopped = false;
            self.error = None;
        }
        let event = self
            .task
            .as_ref()
            .and_then(|task| worker_event(&task.result, task.thread.as_ref()));
        match event {
            Some(Event::Started(pid)) => self.worker_pid = Some(pid),
            Some(Event::Picture(picture))
                if picture.serial == self.serial && Some(picture.frame) == self.requested_frame =>
            {
                self.worker_pid = picture.worker_pid;
                self.picture = Some(picture);
            }
            Some(Event::Picture(_)) => self.rejected += 1,
            Some(Event::Failed(error)) => {
                self.stop();
                self.stopped = true;
                self.error = Some(error);
            }
            None => {}
        }
        if self
            .picture
            .as_ref()
            .is_some_and(|picture| picture.incompatible.load(Ordering::Acquire))
        {
            self.stop();
            self.stopped = true;
            self.error = Some(
                "Viewer picture belongs to a different native GPU device; retry the viewer".into(),
            );
        }
    }

    /// Show a real saved composition with exact frame stepping and scrubbing.
    /// `ui`, `workspace`, `tab` and `project` supply the native document view.
    /// Returns no value. No playback control is exposed before sound scheduling exists.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        tab: Uuid,
        project: Arc<Project>,
    ) {
        self.observe_owner(workspace.edit_snapshot(tab).ok().map(|(owner, _)| owner));
        let sequences: Vec<_> = project
            .sequences
            .iter()
            .filter(|item| item.composition.is_some())
            .collect();
        if sequences.is_empty() {
            ui.weak("Import video, then create a sequence from its source stream.");
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            return;
        }
        let mut selected = self
            .selections
            .get(&tab)
            .copied()
            .filter(|item| {
                sequences
                    .iter()
                    .any(|sequence| sequence.id == item.sequence)
            })
            .unwrap_or(Selection {
                sequence: sequences[0].id,
                frame: 0,
            });
        egui::ComboBox::from_id_salt(("picture-sequence", tab))
            .selected_text(
                sequences
                    .iter()
                    .find(|item| item.id == selected.sequence)
                    .unwrap()
                    .name
                    .clone(),
            )
            .show_ui(ui, |ui| {
                for sequence in &sequences {
                    ui.selectable_value(&mut selected.sequence, sequence.id, &sequence.name);
                }
            });
        let sequence = sequences
            .iter()
            .find(|item| item.id == selected.sequence)
            .unwrap();
        let Some(composition) = project
            .compositions
            .iter()
            .find(|item| Some(item.id) == sequence.composition)
        else {
            return;
        };
        let last = composition.duration - 1;
        selected.frame = selected.frame.min(last);
        ui.horizontal_wrapped(|ui| {
            let previous = ui.add_enabled(selected.frame > 0, egui::Button::new("Previous frame"));
            self.observe_control("previous-frame", &previous, ui);
            if previous.clicked() {
                selected.frame -= 1;
            }
            let next = ui.add_enabled(selected.frame < last, egui::Button::new("Next frame"));
            self.observe_control("next-frame", &next, ui);
            if next.clicked() {
                selected.frame += 1;
            }
            ui.label("Frame");
            let frame = ui.add(egui::DragValue::new(&mut selected.frame).range(0..=last));
            self.observe_control("frame-number", &frame, ui);
            ui.weak(format!(
                "/ {last} · {} × {} · {}/{} fps",
                composition.width,
                composition.height,
                composition.frame_rate.numerator,
                composition.frame_rate.denominator
            ));
        });
        if last <= 1_000_000_000 {
            ui.spacing_mut().slider_width = ui.available_width().clamp(100., 960.);
            let scrub = ui.add(egui::Slider::new(&mut selected.frame, 0..=last).show_value(false));
            self.observe_control("scrub", &scrub, ui);
        }
        if let Some(task) = &self.task
            && (task.owner.tab != tab || task.sequence != selected.sequence)
        {
            self.stop();
            self.stopped = false;
            self.error = None;
        }
        self.selections.insert(tab, selected);
        if !self.stopped && self.task.is_none() && self.retiring.is_empty() {
            let started = workspace.edit_snapshot(tab).and_then(|(owner, _)| {
                let gpu = self
                    .gpu
                    .clone()
                    .ok_or("Native GPU presentation is unavailable")?;
                Task::start(
                    owner,
                    project.clone(),
                    selected.sequence,
                    gpu,
                    ui.ctx().clone(),
                )
            });
            match started {
                Ok(task) => self.task = Some(task),
                Err(error) => {
                    self.stopped = true;
                    self.error = Some(error);
                }
            }
        }
        if self.task.is_some() && self.requested_frame != Some(selected.frame) {
            match self.serial.checked_add(1) {
                Some(serial) => {
                    self.serial = serial;
                    self.requested_frame = Some(selected.frame);
                    if let Err(error) = self.task.as_ref().unwrap().request(Request {
                        serial,
                        frame: selected.frame,
                        requested: Instant::now(),
                    }) {
                        self.stop();
                        self.stopped = true;
                        self.error = Some(error);
                    } else {
                        self.request_accepted_unix_us = Some(
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_micros() as u64,
                        );
                    }
                }
                None => {
                    self.stop();
                    self.stopped = true;
                    self.error =
                        Some("Viewer request counter exhausted; reopen the workspace".into());
                }
            }
        }
        ui.horizontal(|ui| {
            if !self.stopped {
                let cancel = ui.button("Cancel viewer");
                self.observe_control("cancel-viewer", &cancel, ui);
                if cancel.clicked() {
                    self.stop();
                    self.stopped = true;
                }
            } else {
                let retry = ui.button("Retry viewer");
                self.observe_control("retry-viewer", &retry, ui);
                if retry.clicked() {
                    self.stopped = false;
                    self.error = None;
                    ui.ctx().request_repaint();
                }
            }
        });
        let width = ui.available_width().clamp(1., 960.);
        let height = (ui.ctx().content_rect().height() - ui.cursor().min.y - 70.).clamp(96., 400.);
        let scale = (width / composition.width as f32).min(height / composition.height as f32);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(
                composition.width as f32 * scale,
                composition.height as f32 * scale,
            ),
            egui::Sense::hover(),
        );
        ui.painter()
            .rect_filled(rect, 0., egui::Color32::from_gray(20));
        if let Some(picture) = &self.picture {
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                rect,
                Callback {
                    picture: picture.clone(),
                    wake: ui.ctx().clone(),
                    compatible: AtomicBool::new(false),
                },
            ));
            if picture.serial == self.serial {
                ui.weak(format!("Showing frame {}", picture.frame));
            } else {
                ui.weak(format!(
                    "Loading frame {} · showing frame {}",
                    selected.frame, picture.frame
                ));
            }
        } else if !self.stopped {
            ui.weak(if self.retiring.is_empty() {
                "Preparing picture…"
            } else {
                "Stopping the previous viewer…"
            });
        }
        if self.creation.is_some() {
            ui.weak("Creating sequence…");
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    /// Inspect actual preview requests, completion and resource counters.
    /// Takes no arguments. Returns opt-in diagnostic metadata, without pixel IO.
    pub fn diagnostic_state(&self) -> serde_json::Value {
        serde_json::json!({"owner":self.task.as_ref().map(|task| task.owner.version),
            "tab":self.task.as_ref().map(|task|task.owner.tab), "sequence":self.task.as_ref().map(|task|task.sequence),
            "worker_pid":self.worker_pid,"serial":self.serial,"requested_frame":self.requested_frame,
            "request_accepted_unix_us":self.request_accepted_unix_us,
            "displayed_frame":self.picture.as_ref().map(|picture|picture.frame),
            "displayed_serial":self.picture.as_ref().map(|picture|picture.serial),
            "preparation_us":self.picture.as_ref().map(|picture|picture.preparation_us),
            "gpu_draw_completed_us":self.picture.as_ref().map(|picture|picture.completion_us.load(Ordering::Acquire)),
            "stats":self.picture.as_ref().map(|picture|picture.stats), "surface":self.gpu.as_ref().map(|gpu|format!("{:?}",gpu.format)),
            "rejected_results":self.rejected,"retiring":self.retiring.len(),"creating":self.creation.is_some(),"stopped":self.stopped,"error":self.error,"controls":self.controls})
    }
}
