mod cached;
mod completion;
mod prepared;

use crate::workspace::{DocumentOwner, Workspace};
use completion::CompletionLog;
use editbay_audio::{
    MonitorRoute, PlaybackPhase, PlaybackStart, StreamingPlayback, StreamingStatus,
};
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
use prepared::Prepared;
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

    #[test]
    fn cancelling_preparation_or_changing_owner_retires_queue_without_starting_sound() {
        for changed_owner in [false, true] {
            let (_directory, workspace, tab) = workspace();
            let (owner, editor) = workspace.edit_snapshot(tab).unwrap();
            let sequence = Uuid::new_v4();
            let pictures = Arc::new(Mutex::new(Prepared::new(0, 10, 1280, 720).unwrap()));
            pictures.lock().unwrap().reserve().unwrap();
            let mut pane = PreviewPane {
                view_owner: Some(owner),
                prepared: Some(pictures.clone()),
                completions: Some(
                    CompletionLog::new(
                        completion::Scope {
                            tab,
                            version: owner.version,
                            sequence,
                        },
                        0,
                        10,
                    )
                    .unwrap(),
                ),
                starting: Some(PendingSound {
                    owner,
                    project: editor.snapshot(),
                    sequence,
                    composition: Uuid::new_v4(),
                    rate: FrameRate::new(24, 1).unwrap(),
                    duration: 10,
                    start: PlaybackStart::Frame(0),
                    route: MonitorRoute::Stereo,
                    began: Instant::now(),
                }),
                ..Default::default()
            };
            if changed_owner {
                pane.observe_owner(None);
            } else {
                pane.stop_sound();
            }
            assert!(pane.sound.is_none());
            assert!(pane.starting.is_none());
            assert!(pane.prepared.is_none());
            assert!(!pictures.lock().unwrap().state().open);
            assert!(pane.completions.as_ref().unwrap().summary().cancelled);
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Selection {
    sequence: Uuid,
    frame: u64,
}
struct SoundTask {
    owner: DocumentOwner,
    sequence: Uuid,
    rate: editbay_core::FrameRate,
    duration: u64,
    playback: StreamingPlayback,
}
impl SoundTask {
    fn frame(&self, status: &StreamingStatus) -> Option<u64> {
        let sample = status.position_samples?;
        let rate = status.device.as_ref()?.sample_rate;
        Some(
            (u128::from(sample) * u128::from(self.rate.numerator)
                / (u128::from(rate) * u128::from(self.rate.denominator)))
            .min(u128::from(self.duration - 1)) as u64,
        )
    }
}
#[derive(Clone, Copy)]
struct ResumeSound {
    owner: DocumentOwner,
    sequence: Uuid,
    start: PlaybackStart,
}
struct PendingSound {
    owner: DocumentOwner,
    project: Arc<Project>,
    sequence: Uuid,
    composition: Uuid,
    rate: editbay_core::FrameRate,
    duration: u64,
    start: PlaybackStart,
    route: MonitorRoute,
    began: Instant,
}
type PreparedPictures = Arc<Mutex<Prepared<Arc<Picture>>>>;
#[derive(Clone)]
struct Preparation {
    serial: u64,
    pictures: PreparedPictures,
}
struct Request {
    serial: u64,
    frame: u64,
    requested: Instant,
}
#[derive(Default)]
struct Mailbox {
    request: Option<Request>,
    preparation: Option<Preparation>,
    stopped: bool,
}
#[derive(Clone, Copy, serde::Serialize)]
struct PictureWork {
    queued_us: u64,
    render_us: u64,
    convert_us: u64,
    present_us: u64,
    finish_us: u64,
}
struct Picture {
    draw: Arc<DisplayFrame>,
    serial: u64,
    frame: u64,
    requested: Instant,
    preparation_us: u64,
    work: PictureWork,
    completion_us: AtomicU64,
    selected_us: AtomicU64,
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
    coalesced: Arc<AtomicU64>,
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
            if let Some(preparation) = mailbox.preparation.take()
                && let Ok(mut pictures) = preparation.pictures.lock()
            {
                pictures.cancel();
            }
        }
        self.mailbox.1.notify_one();
    }
    fn request(&self, request: Request) -> Result<(), String> {
        let mut mailbox = self.mailbox.0.lock().map_err(|e| e.to_string())?;
        if mailbox.stopped {
            return Err("Viewer is stopping".into());
        }
        mailbox.request = Some(request);
        mailbox.preparation = None;
        self.mailbox.1.notify_one();
        Ok(())
    }
    fn prepare(&self, preparation: Preparation) -> Result<(), String> {
        let mut mailbox = self.mailbox.0.lock().map_err(|e| e.to_string())?;
        if mailbox.stopped {
            return Err("Viewer is stopping".into());
        }
        mailbox.request = None;
        mailbox.preparation = Some(preparation);
        self.mailbox.1.notify_one();
        Ok(())
    }
    fn start(
        owner: DocumentOwner,
        project: Arc<Project>,
        sequence: Uuid,
        gpu: Gpu,
        ctx: egui::Context,
        store: Option<editbay_media::picture_store::PreparedStore>,
    ) -> Result<Self, String> {
        if owner.version != DocumentVersion::of(&project) {
            return Err("Viewer document ownership changed before startup".into());
        }
        let cancel = Cancellation::new().map_err(|e| e.to_string())?;
        let mailbox = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let result = Arc::new(Mutex::new(None));
        let coalesced = Arc::new(AtomicU64::new(0));
        let replaced = coalesced.clone();
        let input = mailbox.clone();
        let output = result.clone();
        let token = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("editbay-native-picture".into())
            .spawn(move || {
                let publish = |event| {
                    if let Ok(mut result) = output.lock() {
                        if matches!(result.as_ref(), Some(Event::Picture(_))) {
                            replaced.fetch_add(1, Ordering::AcqRel);
                        }
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
                    let mut provider = PictureWorker::new(
                        &executable,
                        snapshot.clone(),
                        WorkerBudget::default(),
                        token.clone(),
                    )
                    .map_err(|e| e.to_string())?;
                    if let Some(store) = store {
                        provider.attach_store(store).map_err(|e| e.to_string())?;
                    }
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
                            if mailbox.stopped {
                                return Ok(());
                            }
                            let request = if let Some(preparation) = &mailbox.preparation {
                                let reserved = preparation
                                    .pictures
                                    .lock()
                                    .map_err(|e| e.to_string())?
                                    .reserve();
                                reserved.map(|reservation| {
                                    (
                                        Request {
                                            serial: preparation.serial,
                                            frame: reservation.frame,
                                            requested: Instant::now(),
                                        },
                                        Some((preparation.pictures.clone(), reservation)),
                                    )
                                })
                            } else {
                                mailbox.request.take().map(|request| (request, None))
                            };
                            if request.is_none() {
                                mailbox = input
                                    .1
                                    .wait_timeout(mailbox, Duration::from_millis(100))
                                    .map_err(|e| e.to_string())?
                                    .0;
                            }
                            if mailbox.stopped {
                                return Ok(());
                            }
                            request
                        };
                        if let Some((request, preparation)) = request {
                            let began = Instant::now();
                            let queued_us = request.requested.elapsed().as_micros() as u64;
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
                            let render_us = began.elapsed().as_micros() as u64;
                            let began = Instant::now();
                            let converted = graph
                                .convert(&working, ImageBoundary::Display)
                                .map_err(|e| e.to_string())?;
                            let convert_us = began.elapsed().as_micros() as u64;
                            let began = Instant::now();
                            let draw = Arc::new(
                                graph
                                    .present(&converted, &display)
                                    .map_err(|e| e.to_string())?,
                            );
                            let present_us = began.elapsed().as_micros() as u64;
                            let began = Instant::now();
                            graph.finish().map_err(|e| e.to_string())?;
                            let finish_us = began.elapsed().as_micros() as u64;
                            if !token.is_cancelled() {
                                let picture = Arc::new(Picture {
                                    draw,
                                    serial: request.serial,
                                    frame: request.frame,
                                    requested: request.requested,
                                    preparation_us: request.requested.elapsed().as_micros() as u64,
                                    work: PictureWork {
                                        queued_us,
                                        render_us,
                                        convert_us,
                                        present_us,
                                        finish_us,
                                    },
                                    completion_us: AtomicU64::new(0),
                                    selected_us: AtomicU64::new(0),
                                    incompatible: AtomicBool::new(false),
                                    stats: graph.stats(),
                                    worker_pid: graph.picture_provider().process_id(),
                                });
                                if let Some((pictures, reservation)) = preparation {
                                    pictures
                                        .lock()
                                        .map_err(|e| e.to_string())?
                                        .publish(reservation, picture);
                                    ctx.request_repaint();
                                } else {
                                    publish(Event::Picture(picture));
                                }
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
            coalesced,
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
    completion: Option<completion::Publisher>,
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
        let completion = self.completion.clone();
        let wake = self.wake.clone();
        self.picture.draw.paint(pass, move || {
            let elapsed_us = picture.requested.elapsed().as_micros() as u64;
            if picture
                .completion_us
                .compare_exchange(0, elapsed_us, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if let Some(completion) = completion {
                    completion.completed(
                        picture.frame,
                        picture.serial,
                        elapsed_us,
                        elapsed_us.saturating_sub(picture.selected_us.load(Ordering::Acquire)),
                    );
                }
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
    cached: cached::Cache,
    view_owner: Option<DocumentOwner>,
    gpu: Option<Gpu>,
    selections: HashMap<Uuid, Selection>,
    source_audio: HashMap<(Uuid, Uuid), Option<u32>>,
    sound: Option<SoundTask>,
    starting: Option<PendingSound>,
    prepared: Option<PreparedPictures>,
    sound_retiring: Vec<SoundTask>,
    sound_status: Option<StreamingStatus>,
    resume_sound: Option<ResumeSound>,
    monitor_route: MonitorRoute,
    skipped_frames: u64,
    completions: Option<CompletionLog>,
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
    /// Configure bounded background picture storage in the application's state directory.
    /// `directory` is the explicit cache location. Returns a viewer without starting IO.
    pub fn new(directory: std::path::PathBuf) -> Self {
        Self {
            cached: cached::Cache::new(directory),
            ..Default::default()
        }
    }
    /// Select an authored composition and exact frame in the shared viewer.
    /// `tab`, `project`, `composition` and `frame` identify document content.
    /// Returns an error if absent; changing selection stops current playback.
    pub fn select(
        &mut self,
        tab: Uuid,
        project: &Project,
        composition: Uuid,
        frame: u64,
    ) -> Result<(), String> {
        let sequence = project
            .sequences
            .iter()
            .find(|s| s.composition == Some(composition))
            .ok_or("Composition has no sequence")?;
        let scene = project
            .compositions
            .iter()
            .find(|s| s.id == composition)
            .ok_or("Composition is absent")?;
        if frame >= scene.duration {
            return Err("Frame is beyond the sequence".into());
        }
        self.stop_sound();
        self.resume_sound = None;
        self.sound_status = None;
        self.selections.insert(
            tab,
            Selection {
                sequence: sequence.id,
                frame,
            },
        );
        self.stopped = false;
        Ok(())
    }

    /// Read the current viewer frame for source marks or record edits.
    /// `tab` and `project` resolve the viewer. Returns composition and frame.
    pub fn position(&self, tab: Uuid, project: &Project) -> Option<(Uuid, u64)> {
        Some((
            self.selected_composition(tab, project)?,
            self.selections.get(&tab).map_or(0, |s| s.frame),
        ))
    }
    /// Resolve the composition selected by this tab's actual viewer.
    /// `tab` and `project` identify current content. Returns its composition ID,
    /// falling back to the first authored sequence before the viewer is drawn.
    pub fn selected_composition(&self, tab: Uuid, project: &Project) -> Option<Uuid> {
        self.selections
            .get(&tab)
            .and_then(|selection| {
                project
                    .sequences
                    .iter()
                    .find(|sequence| sequence.id == selection.sequence)
                    .and_then(|sequence| sequence.composition)
            })
            .or_else(|| {
                project
                    .sequences
                    .iter()
                    .find_map(|sequence| sequence.composition)
            })
    }
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
        self.stop_sound();
        if let Some(task) = self.task.take() {
            task.stop();
            self.retiring.push(task);
        }
        self.picture = None;
        self.requested_frame = None;
        self.request_accepted_unix_us = None;
        self.worker_pid = None;
    }

    fn stop_sound(&mut self) -> Option<u64> {
        self.starting = None;
        if let Some(prepared) = self.prepared.take() {
            let in_flight = if let Ok(mut pictures) = prepared.lock() {
                let active = pictures.state().in_flight;
                pictures.cancel();
                active
            } else {
                true
            };
            self.requested_frame = None;
            if in_flight && let Some(task) = self.task.take() {
                task.stop();
                self.retiring.push(task);
                self.worker_pid = None;
            } else if let Some(task) = &self.task {
                task.mailbox.1.notify_one();
            }
        }
        if let Some(completions) = &mut self.completions {
            completions.cancel();
        }
        let mut task = self.sound.take()?;
        let status = task.playback.status();
        let frame = task.frame(&status);
        if let (Some(position), Some(device)) = (status.position_samples, &status.device) {
            self.resume_sound = Some(ResumeSound {
                owner: task.owner,
                sequence: task.sequence,
                start: if status.end_sample == Some(position) {
                    PlaybackStart::Frame(0)
                } else {
                    PlaybackStart::Sample {
                        position,
                        sample_rate: device.sample_rate,
                    }
                },
            });
        }
        if let Some(frame) = frame {
            self.selections.insert(
                task.owner.tab,
                Selection {
                    sequence: task.sequence,
                    frame,
                },
            );
        }
        self.sound_status = Some(status);
        task.playback.stop();
        self.sound_retiring.push(task);
        frame
    }

    fn advance_picture(&mut self, frame: u64) -> Result<(), String> {
        let Some(prepared) = &self.prepared else {
            return Ok(());
        };
        if !self
            .completions
            .as_ref()
            .is_some_and(|log| log.contains_frame(frame))
        {
            return Err("Sound clock is outside its prepared playback range".into());
        }
        self.requested_frame = Some(frame);
        if self
            .picture
            .as_ref()
            .is_some_and(|picture| picture.frame == frame)
        {
            return Ok(());
        }
        let next = prepared.lock().map_err(|e| e.to_string())?.take(frame);
        if let Some(task) = &self.task {
            task.mailbox.1.notify_one();
        }
        if let Some(picture) = next {
            if picture.serial != self.serial || picture.frame != frame {
                return Err("Prepared picture belongs to an obsolete playback".into());
            }
            if let Some(previous) = &self.picture {
                self.skipped_frames = self
                    .skipped_frames
                    .saturating_add(frame.saturating_sub(previous.frame).saturating_sub(1));
            }
            self.worker_pid = picture.worker_pid;
            picture.selected_us.store(
                picture.requested.elapsed().as_micros() as u64,
                Ordering::Release,
            );
            self.picture = Some(picture);
            Ok(())
        } else if self.starting.is_some() {
            Ok(())
        } else {
            Err("Pictures could not keep up with sound; playback stopped. Retry playback".into())
        }
    }

    fn start_prepared_sound(&mut self) -> Result<(), String> {
        let Some(pending) = &self.starting else {
            return Ok(());
        };
        let prepared = self
            .prepared
            .as_ref()
            .ok_or("Prepared pictures are absent")?;
        let first = prepared.lock().map_err(|e| e.to_string())?.state().first;
        let expired = pending.began.elapsed() > Duration::from_secs(10);
        self.advance_picture(first)?;
        let ready = self
            .prepared
            .as_ref()
            .unwrap()
            .lock()
            .map_err(|e| e.to_string())?
            .ready()
            && self
                .picture
                .as_ref()
                .is_some_and(|picture| picture.completion_us.load(Ordering::Acquire) > 0);
        if ready {
            let pending = self.starting.take().unwrap();
            let playback = StreamingPlayback::start(
                pending.project,
                pending.composition,
                pending.start,
                pending.route,
            )?;
            self.sound = Some(SoundTask {
                owner: pending.owner,
                sequence: pending.sequence,
                rate: pending.rate,
                duration: pending.duration,
                playback,
            });
        } else if expired {
            return Err("Pictures did not become ready within ten seconds; retry playback".into());
        }
        Ok(())
    }

    fn observe_owner(&mut self, owner: Option<DocumentOwner>) {
        if owner != self.view_owner {
            self.stop();
            self.cached.invalidate();
            self.stopped = false;
            self.error = None;
            self.resume_sound = None;
            self.sound_status = None;
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
        if self.cached.poll() {
            self.stop();
            self.stopped = false;
            self.error = None;
        }
        if self.cached.busy() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if let Some(completions) = &mut self.completions {
            completions.poll();
            let state = completions.summary();
            if !state.cancelled
                && (state.overflow > 0 || state.rejected > 0 || state.outside_history > 0)
            {
                self.stop_sound();
                self.error = Some(
                    "Viewer fell behind or completed an invalid picture; retry playback".into(),
                );
            }
        }
        let mut retired_status = None;
        self.sound_retiring.retain_mut(|task| {
            let status = task.playback.status();
            if Some(task.owner) == owner {
                retired_status = Some(status);
            }
            !task.playback.reap()
        });
        if self.sound.is_none()
            && let Some(status) = retired_status
        {
            self.sound_status = Some(status);
        }
        let mut sound_finished = false;
        let mut sound_error = None;
        let mut sound_frame = None;
        if let Some(task) = &mut self.sound {
            let status = task.playback.status();
            if status.version != task.owner.version {
                sound_error = Some("Sound belongs to an obsolete document".into());
            } else if let Some(frame) = task.frame(&status) {
                sound_frame = Some(frame);
                self.selections.insert(
                    task.owner.tab,
                    Selection {
                        sequence: task.sequence,
                        frame,
                    },
                );
            }
            match status.phase {
                PlaybackPhase::Failed => sound_error = status.error.clone(),
                PlaybackPhase::Finished => sound_finished = true,
                _ => {}
            }
            self.sound_status = Some(status);
        }
        if let Some(frame) = sound_frame
            && let Err(error) = self.advance_picture(frame)
        {
            sound_error = Some(error);
        }
        if let Err(error) = self.start_prepared_sound() {
            sound_error = Some(error);
        }
        if let Some(error) = sound_error {
            self.stop_sound();
            self.error = Some(error);
        } else if sound_finished {
            if let Some(completions) = &mut self.completions {
                completions.observe_end();
            }
            if let Some(task) = self.sound.take() {
                self.sound_retiring.push(task);
            }
            self.resume_sound = None;
        }
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
        if !self.retiring.is_empty()
            || self.creation.is_some()
            || self.sound.is_some()
            || self.starting.is_some()
            || !self.sound_retiring.is_empty()
        {
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
                if self.prepared.is_none()
                    && picture.serial == self.serial
                    && Some(picture.frame) == self.requested_frame =>
            {
                self.worker_pid = picture.worker_pid;
                picture.selected_us.store(
                    picture.requested.elapsed().as_micros() as u64,
                    Ordering::Release,
                );
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
    /// Returns no value. Playback uses the captured sound graph's device clock.
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
        let before_interaction = selected;
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
        if let Some(owner) = self.view_owner {
            self.cached.observe(owner, selected.sequence);
        }
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
        if selected != before_interaction {
            self.stop_sound();
            self.resume_sound = None;
            self.sound_status = None;
        }
        ui.horizontal_wrapped(|ui| {
            if self.sound.is_some() || self.starting.is_some() {
                let pause = ui.button("Pause");
                self.observe_control("pause-sequence", &pause, ui);
                if pause.clicked() && let Some(frame) = self.stop_sound() {
                    selected.frame = frame;
                }
            } else {
                let play = ui.add_enabled(
                    !self.stopped && self.task.is_some() && self.sound_retiring.is_empty() && !self.cached.busy(),
                    egui::Button::new("Play"),
                );
                self.observe_control("play-sequence", &play, ui);
                if play.clicked() {
                    let started = workspace.edit_snapshot(tab).and_then(|(owner, project)| {
                        if self.sound_status.as_ref().is_some_and(|s| s.phase == PlaybackPhase::Finished)
                            && selected.frame == last
                        {
                            selected.frame = 0;
                        }
                        let start = self.resume_sound
                            .filter(|resume| resume.owner == owner && resume.sequence == selected.sequence)
                            .map_or(PlaybackStart::Frame(selected.frame), |resume| resume.start);
                        let completions = CompletionLog::new(completion::Scope {
                            tab: owner.tab, version: owner.version, sequence: selected.sequence,
                        }, selected.frame, composition.duration)?;
                        let prepared = Arc::new(Mutex::new(Prepared::new(selected.frame, composition.duration,
                            composition.width, composition.height)?));
                        let serial = self.serial.checked_add(1).ok_or("Viewer request counter exhausted")?;
                        let task = self.task.as_ref().ok_or("Viewer is unavailable")?;
                        if task.owner != owner || task.sequence != selected.sequence {
                            return Err("Viewer sequence changed; retry playback".into());
                        }
                        task.prepare(Preparation { serial, pictures:prepared.clone() })?;
                        Ok((PendingSound {
                                owner,
                                project:project.snapshot(),
                                sequence: selected.sequence,
                                composition:composition.id,
                                rate: composition.frame_rate,
                                duration: composition.duration,
                                start, route:self.monitor_route, began:Instant::now(),
                            }, completions, prepared, serial))
                    });
                    match started {
                        Ok((pending, completions, prepared, serial)) => {
                            if let Some(previous) = self.prepared.replace(prepared)
                                && let Ok(mut pictures) = previous.lock()
                            {
                                pictures.cancel();
                            }
                            self.starting = Some(pending);
                            self.serial = serial;
                            self.completions = Some(completions);
                            self.sound_status = None;
                            self.error = None;
                            self.skipped_frames = 0;
                            self.picture = None;
                            self.requested_frame = None;
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
            }
            ui.add_enabled_ui(self.sound.is_none() && self.starting.is_none() && self.sound_retiring.is_empty(), |ui| {
                let response = egui::ComboBox::from_id_salt("monitor-route")
                    .selected_text(match self.monitor_route {
                        MonitorRoute::Stereo => "Stereo monitor",
                        MonitorRoute::Original => "Original channels",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.monitor_route, MonitorRoute::Stereo, "Stereo monitor");
                        ui.selectable_value(&mut self.monitor_route, MonitorRoute::Original, "Original channels");
                    }).response.on_hover_text("Stereo monitor sends mono to both speakers, reduces center/surround channels by 3 dB and omits LFE. Original source and delivery channels stay unchanged.");
                self.observe_control("monitor-route", &response, ui);
            });
            if self.starting.is_some() {
                ui.weak("Preparing pictures…");
            }
            if let Some(status) = &self.sound_status {
                if let Some(device) = &status.device {
                    ui.weak(format!("{} · {} Hz · {} channels", device.name, device.sample_rate, device.channels));
                }
                if status.phase == PlaybackPhase::Preparing {
                    ui.weak("Preparing sound…");
                }
                if status.clipped_monitor_samples > 0 {
                    ui.colored_label(ui.visuals().warn_fg_color, "Monitor clipping");
                }
            }
            if !self.sound_retiring.is_empty() {
                ui.weak("Stopping sound…");
            }
        });
        ui.horizontal_wrapped(|ui| {
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
            if self.cached.preparing() {
                let cancel = ui.button("Cancel preparation");
                self.observe_control("cancel-picture-cache", &cancel, ui);
                if cancel.clicked() { self.cached.invalidate(); }
            } else {
                let prepare = ui.add_enabled(self.cached.available() && !self.cached.busy() && self.sound.is_none() && self.starting.is_none(), egui::Button::new("Prepare playback"))
                    .on_hover_text("Cache exact source pictures for this sequence. Editing or switching sequences discards preparation.");
                self.observe_control("prepare-picture-cache", &prepare, ui);
                if prepare.clicked() {
                    self.stop();
                    self.cached.invalidate();
                    let result = workspace.edit_snapshot(tab).and_then(|(owner, editor)| self.cached.start(owner, editor.snapshot(), selected.sequence, ui.ctx().clone()));
                    if let Err(error) = result { self.cached.error = Some(error); }
                }
            }
        });
        let cache_label = self.cached.label();
        if !cache_label.is_empty() {
            ui.weak(cache_label);
        }
        if let Some(error) = &self.cached.error {
            ui.add(
                egui::Label::new(egui::RichText::new(error).color(ui.visuals().error_fg_color))
                    .wrap(),
            );
        }
        if self.sound.is_some() || self.starting.is_some() || !self.sound_retiring.is_empty() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        if let Some(task) = &self.task
            && (task.owner.tab != tab || task.sequence != selected.sequence)
        {
            self.stop();
            self.stopped = false;
            self.error = None;
        }
        self.selections.insert(tab, selected);
        if !self.stopped && self.task.is_none() && self.retiring.is_empty() && !self.cached.busy() {
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
                    self.cached.ready(owner, selected.sequence),
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
        if self.task.is_some()
            && self.prepared.is_none()
            && self.requested_frame != Some(selected.frame)
        {
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
        let width = ui.available_width().clamp(1., 960.);
        let height = (ui.ctx().content_rect().height() - ui.cursor().min.y - 70.).clamp(96., 400.);
        let scale = (width / composition.width as f32).min(height / composition.height as f32);
        let (rect, mut image_response) = ui.allocate_exact_size(
            egui::vec2(
                composition.width as f32 * scale,
                composition.height as f32 * scale,
            ),
            egui::Sense::hover(),
        );
        image_response.rect = rect;
        self.observe_control("viewer-picture", &image_response, ui);
        ui.painter()
            .rect_filled(rect, 0., egui::Color32::from_gray(20));
        if let Some(picture) = &self.picture {
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                rect,
                Callback {
                    picture: picture.clone(),
                    completion: self.completions.as_ref().map(CompletionLog::publisher),
                    wake: ui.ctx().clone(),
                    compatible: AtomicBool::new(false),
                },
            ));
            if self.sound.is_some() {
                ui.weak(format!("Playing · frame {}", picture.frame));
            } else if picture.serial == self.serial {
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

    /// Inspect bounded actual display events for the native diagnostics writer.
    /// Takes this pane. Returns a private play session and its latest numbered
    /// receipts; consumers use their indices to avoid repeating observations.
    pub(crate) fn completed_pictures(
        &self,
    ) -> Option<(Uuid, &std::collections::VecDeque<completion::Completed>)> {
        self.completions.as_ref().map(CompletionLog::events)
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
            "work":self.picture.as_ref().map(|picture|picture.work),
            "gpu_draw_completed_us":self.picture.as_ref().map(|picture|picture.completion_us.load(Ordering::Acquire)),
            "stats":self.picture.as_ref().map(|picture|picture.stats), "surface":self.gpu.as_ref().map(|gpu|format!("{:?}",gpu.format)),
            "adapter":self.gpu.as_ref().map(|gpu|format!("{:?}",gpu.adapter.get_info())),
            "rejected_results":self.rejected,"retiring":self.retiring.len(),"creating":self.creation.is_some(),"stopped":self.stopped,"error":self.error,"controls":self.controls,
            "coalesced_results":self.task.as_ref().map(|task|task.coalesced.load(Ordering::Acquire)),
            "picture_cache":self.cached.diagnostic(),
            "sound":self.sound_status,"sound_active":self.sound.is_some(),"sound_retiring":self.sound_retiring.len(),"skipped_frames":self.skipped_frames,
            "preparing_playback":self.starting.is_some(),"prepared_pictures":self.prepared.as_ref().and_then(|pictures|pictures.lock().ok().map(|pictures|pictures.state())),
            "accepted_picture_gaps":self.skipped_frames,"display":self.completions.as_ref().map(CompletionLog::summary),
            "resume_sound":self.resume_sound.map(|resume|resume.start)})
    }
}
