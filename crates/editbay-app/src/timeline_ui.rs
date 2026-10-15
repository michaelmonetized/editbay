use crate::fields::{self, FieldState};
use crate::{
    preview::PreviewPane,
    workspace::{DocumentOwner, Workspace},
};
use editbay_core::{
    DocumentEditor, FrameRange, Project, SourceSelection, TimelineAction, TimelineClip,
    TimelineControls,
};
use eframe::egui::{self, Ui};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    time::Duration,
};
use uuid::Uuid;

struct Assembly {
    composition: Uuid,
    clips: Vec<TimelineClip>,
}
struct Catalog {
    owner: DocumentOwner,
    assemblies: Vec<Assembly>,
}
enum Completed {
    Catalog(Vec<Assembly>),
    Edit {
        editor: DocumentEditor,
        composition: Uuid,
        clip: Option<Uuid>,
        frame: u64,
        trim: Option<FrameRange>,
    },
}
struct Task {
    draft: Option<u64>,
    owner: DocumentOwner,
    result: Receiver<Result<Completed, String>>,
}
struct Selection {
    title: String,
    font: std::path::PathBuf,
    size: f64,
    baseline: [f64; 2],
    title_duration: u64,
    fade_in: u64,
    fade_out: u64,
    track: Option<Uuid>,
    controls: TimelineControls,
    slip: i64,
    source: Option<Uuid>,
    record: Option<Uuid>,
    clip: Option<Uuid>,
    source_range: FrameRange,
    trim_range: FrameRange,
    at: u64,
    name: String,
    ripple: bool,
    multiple: Vec<Uuid>,
}
impl Default for Selection {
    fn default() -> Self {
        Self {
            title: "Title".into(),
            font: std::env::var_os("EDITBAY_TITLE_FONT")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| "/usr/share/fonts/liberation/LiberationSans-Regular.ttf".into()),
            size: 64.,
            baseline: [80., 160.],
            title_duration: 72,
            fade_in: 12,
            fade_out: 12,
            track: None,
            controls: TimelineControls::default(),
            slip: 0,
            source: None,
            record: None,
            clip: None,
            source_range: FrameRange { start: 0, end: 1 },
            trim_range: FrameRange { start: 0, end: 1 },
            at: 0,
            name: "New cut".into(),
            ripple: true,
            multiple: Vec::new(),
        }
    }
}

struct Draft {
    owner: DocumentOwner,
    actions: Vec<TimelineAction>,
    serial: u64,
    sent: u64,
    finished: bool,
    ready: Option<DocumentEditor>,
}

#[derive(Default)]
pub struct TimelinePane {
    draft: Option<Draft>,
    serial: u64,
    handles: crate::timeline_handles::Handles,
    selections: HashMap<Uuid, Selection>,
    catalog: Option<Catalog>,
    task: Option<Task>,
    error: Option<String>,
}

impl TimelinePane {
    /// Inspect native editing state for opt-in qualification traces.
    /// Takes no arguments and returns observed ownership, selections and clips;
    /// this grants no command authority and performs no graph evaluation.
    pub fn diagnostic_state(&self) -> serde_json::Value {
        let catalog = self.catalog.as_ref();
        let selection = catalog.and_then(|c| self.selections.get(&c.owner.tab));
        let record = selection.and_then(|s| s.record);
        let clips =
            catalog.and_then(|c| c.assemblies.iter().find(|a| Some(a.composition) == record));
        serde_json::json!({"version":catalog.map(|c|c.owner.version),"busy":self.task.is_some(),"error":self.error,
            "record":record,"clip":selection.and_then(|s|s.clip),"at":selection.map(|s|s.at),
            "source_range":selection.map(|s|s.source_range),"trim_range":selection.map(|s|s.trim_range),"clips":clips.map(|a|&a.clips)})
    }
    /// Publish prepared edits only to their captured tab and document version.
    /// `workspace`, `preview` and `ctx` own the native session. Returns no value;
    /// stale results fail visibly and each successful edit is one undo group.
    pub fn poll(
        &mut self,
        workspace: &mut Workspace,
        preview: &mut PreviewPane,
        visible: bool,
        ctx: &egui::Context,
    ) {
        if self.draft.as_ref().is_some_and(|draft| {
            !visible || workspace.active != Some(draft.owner.tab) || !workspace.owns(draft.owner)
        }) {
            self.draft = None;
            preview.set_draft(None);
        }
        self.selections
            .retain(|tab, _| workspace.tabs.iter().any(|item| item.id == *tab));
        if self
            .draft
            .as_ref()
            .is_some_and(|draft| draft.finished && draft.ready.is_some())
        {
            self.finish_draft(workspace, preview);
        }
        if self.task.is_none() {
            self.prepare_draft(workspace);
        }
        let Some(task) = &self.task else {
            return;
        };
        ctx.request_repaint_after(Duration::from_millis(16));
        let owner = task.owner;
        let draft_serial = task.draft;
        let result = match task.result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Timeline worker stopped before returning its result".into())
            }
        };
        self.task = None;
        if !visible || workspace.active != Some(owner.tab) || !workspace.owns(owner) {
            if matches!(result, Ok(Completed::Edit { .. })) {
                self.error = Some("Project changed while preparing this edit; try it again".into());
            }
            return;
        }
        if let Some(serial) = draft_serial {
            if let Some(draft) = &mut self.draft {
                if draft.serial == serial && draft.owner == owner {
                    match result {
                        Ok(Completed::Edit {
                            editor,
                            composition,
                            frame,
                            ..
                        }) => {
                            let project = editor.snapshot();
                            let _ = preview.select(owner.tab, &project, composition, frame);
                            preview.set_draft(Some((owner, project)));
                            draft.ready = Some(editor);
                            self.error = None;
                        }
                        Err(error) => {
                            self.error = Some(error);
                            draft.ready = None;
                        }
                        _ => {}
                    }
                    if draft.finished {
                        if let Some(editor) = draft.ready.take() {
                            if let Err(error) = workspace.commit_edit(owner, editor) {
                                self.error = Some(error);
                            }
                            self.catalog = None;
                        }
                        self.draft = None;
                        preview.set_draft(None);
                    }
                }
            }
            self.prepare_draft(workspace);
            return;
        }
        match result {
            Ok(Completed::Catalog(assemblies)) => {
                if let Some(selected) = self.selections.get_mut(&owner.tab) {
                    let clip = assemblies
                        .iter()
                        .find(|a| Some(a.composition) == selected.record)
                        .and_then(|a| a.clips.iter().find(|c| Some(c.id) == selected.clip));
                    if let Some(clip) = clip {
                        selected.trim_range = clip.source.range;
                        selected.controls = clip.controls;
                    } else {
                        selected.clip = None;
                    }
                }
                self.catalog = Some(Catalog { owner, assemblies })
            }
            Ok(Completed::Edit {
                editor,
                composition,
                clip,
                frame,
                trim,
            }) => {
                let project = editor.snapshot();
                match workspace.commit_edit(owner, editor) {
                    Ok(()) => {
                        let selected = self.selections.entry(owner.tab).or_default();
                        if editbay_core::timeline_clips(&project, composition).is_err() {
                            selected.source = Some(composition);
                            if let Some(scene) =
                                project.compositions.iter().find(|s| s.id == composition)
                            {
                                selected.source_range = FrameRange {
                                    start: 0,
                                    end: scene.duration,
                                };
                            }
                        }
                        selected.record = Some(composition);
                        selected.clip = clip;
                        selected.at = frame;
                        if let Some(range) = trim {
                            selected.trim_range = range;
                        }
                        if let Err(error) = preview.select(owner.tab, &project, composition, frame)
                        {
                            self.error = Some(error);
                        }
                        self.catalog = None;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn edit(
        &mut self,
        workspace: &Workspace,
        tab: Uuid,
        action: TimelineAction,
    ) -> Result<(), String> {
        if self.task.is_some() {
            return Err("Timeline is still preparing the previous request".into());
        }
        let (owner, mut editor) = workspace.edit_snapshot(tab)?;
        let (sender, result) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("editbay-timeline-edit".into())
            .spawn(move || {
                let run = || {
                    let change = editbay_core::timeline_edit(editor.project(), &action)
                        .map_err(|e| e.to_string())?;
                    editor
                        .apply(owner.version, "Timeline edit".into(), &change.commands)
                        .map_err(|e| e.to_string())?;
                    let selected = change.clip.and_then(|id| {
                        editbay_core::timeline_clips(editor.project(), change.composition)
                            .ok()?
                            .into_iter()
                            .find(|clip| clip.id == id)
                    });
                    let frame = selected.as_ref().map_or(0, |clip| clip.range.start);
                    let trim = selected.map(|clip| clip.source.range);
                    Ok(Completed::Edit {
                        editor,
                        composition: change.composition,
                        clip: change.clip,
                        frame,
                        trim,
                    })
                };
                let _ = sender.send(run());
            })
            .map_err(|e| e.to_string())?;
        self.task = Some(Task {
            draft: None,
            owner,
            result,
        });
        self.error = None;
        Ok(())
    }

    fn update_draft(
        &mut self,
        workspace: &Workspace,
        tab: Uuid,
        action: TimelineAction,
        finished: bool,
    ) {
        self.update_drafts(workspace, tab, vec![action], finished);
    }

    fn update_drafts(
        &mut self,
        workspace: &Workspace,
        tab: Uuid,
        actions: Vec<TimelineAction>,
        finished: bool,
    ) {
        let Ok((owner, _)) = workspace.edit_snapshot(tab) else {
            return;
        };
        self.serial = self.serial.wrapping_add(1);
        let sent = self.draft.as_ref().map_or(0, |draft| draft.sent);
        self.draft = Some(Draft {
            owner,
            actions,
            serial: self.serial,
            sent,
            finished,
            ready: None,
        });
        self.prepare_draft(workspace);
    }

    fn prepare_draft(&mut self, workspace: &Workspace) {
        if self.task.is_some() {
            return;
        }
        let Some(draft) = &mut self.draft else {
            return;
        };
        if draft.sent == draft.serial {
            return;
        }
        let Ok((owner, mut editor)) = workspace.edit_snapshot(draft.owner.tab) else {
            return;
        };
        if owner != draft.owner {
            return;
        }
        let actions = draft.actions.clone();
        let serial = draft.serial;
        let (sender, result) = mpsc::sync_channel(1);
        let run = std::thread::Builder::new()
            .name("editbay-timeline-draft".into())
            .spawn(move || {
                let result = (|| {
                    let mut simulation = editor.clone();
                    let mut commands = Vec::new();
                    let mut final_change = None;
                    for action in &actions {
                        let change = editbay_core::timeline_edit(simulation.project(), action)
                            .map_err(|e| e.to_string())?;
                        simulation
                            .apply(
                                editbay_core::DocumentVersion::of(simulation.project()),
                                "Draft".into(),
                                &change.commands,
                            )
                            .map_err(|e| e.to_string())?;
                        commands.extend(change.commands.clone());
                        final_change = Some(change);
                    }
                    let change = final_change.ok_or("Select a clip before editing")?;
                    editor
                        .apply(owner.version, "Timeline gesture".into(), &commands)
                        .map_err(|e| e.to_string())?;
                    let clip = change.clip.and_then(|id| {
                        editbay_core::timeline_clips(editor.project(), change.composition)
                            .ok()?
                            .into_iter()
                            .find(|c| c.id == id)
                    });
                    Ok(Completed::Edit {
                        editor,
                        composition: change.composition,
                        clip: change.clip,
                        frame: clip.as_ref().map_or(0, |c| c.range.start),
                        trim: clip.map(|c| c.source.range),
                    })
                })();
                let _ = sender.send(result);
            });
        match run {
            Ok(_) => {
                draft.sent = serial;
                self.task = Some(Task {
                    draft: Some(serial),
                    owner,
                    result,
                });
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn finish_draft(&mut self, workspace: &mut Workspace, preview: &mut PreviewPane) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        draft.finished = true;
        if let Some(editor) = draft.ready.take() {
            if let Err(error) = workspace.commit_edit(draft.owner, editor) {
                self.error = Some(error);
            }
            self.catalog = None;
            self.draft = None;
            preview.set_draft(None);
        }
    }

    fn refresh(&mut self, owner: DocumentOwner, project: Arc<Project>) -> Result<(), String> {
        let (sender, result) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("editbay-timeline-inspect".into())
            .spawn(move || {
                let assemblies = project
                    .sequences
                    .iter()
                    .filter_map(|sequence| {
                        let composition = sequence.composition?;
                        editbay_core::timeline_clips(&project, composition)
                            .ok()
                            .map(|clips| Assembly { composition, clips })
                    })
                    .collect();
                let _ = sender.send(Ok(Completed::Catalog(assemblies)));
            })
            .map_err(|e| e.to_string())?;
        self.task = Some(Task {
            draft: None,
            owner,
            result,
        });
        Ok(())
    }

    /// Draw source marks, linked tracks and implemented edit controls.
    /// `ui`, `workspace`, `preview`, `tab` and `project` identify current content.
    /// Returns immediately; graph inspection and command validation run off-thread.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        workspace: &Workspace,
        preview: &mut PreviewPane,
        tab: Uuid,
        project: Arc<Project>,
        interactive: bool,
    ) {
        let Ok((owner, _)) = workspace.edit_snapshot(tab) else {
            return;
        };
        if self
            .catalog
            .as_ref()
            .is_none_or(|catalog| catalog.owner != owner)
        {
            if self.task.is_none()
                && let Err(error) = self.refresh(owner, project.clone())
            {
                self.error = Some(error);
            }
            ui.weak("Preparing timeline…");
            return;
        }
        let scenes: Vec<_> = project
            .sequences
            .iter()
            .filter_map(|sequence| {
                project
                    .compositions
                    .iter()
                    .find(|scene| Some(scene.id) == sequence.composition)
            })
            .collect();
        if scenes.is_empty() {
            return;
        }
        let catalog = self.catalog.as_ref().unwrap();
        let selected = self.selections.entry(tab).or_default();
        if selected
            .source
            .is_none_or(|id| !scenes.iter().any(|scene| scene.id == id))
        {
            selected.source = Some(scenes[0].id);
            selected.source_range = FrameRange {
                start: 0,
                end: scenes[0].duration,
            };
        }
        if selected.record.is_none_or(|id| {
            !catalog
                .assemblies
                .iter()
                .any(|assembly| assembly.composition == id)
        }) {
            selected.record = catalog
                .assemblies
                .iter()
                .find(|assembly| Some(assembly.composition) == project.sequences[0].composition)
                .or_else(|| catalog.assemblies.first())
                .map(|assembly| assembly.composition);
            selected.clip = None;
        }
        let draft_project = preview.draft_snapshot(owner);
        let busy = self.task.as_ref().is_some_and(|task| task.draft.is_none());
        let mut draft_action = None;
        let mut draft_actions = None;
        let mut finish_gesture = false;
        let mut cancel_gesture = false;
        let mut action = None;
        let escape = interactive && ui.input(|i| i.key_pressed(egui::Key::Escape));
        let enter = interactive
            && !ui.ctx().text_edit_focused()
            && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let shortcut = |key| {
            interactive
                && !busy
                && !ui.ctx().text_edit_focused()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key))
        };
        let mark_in = shortcut(egui::Key::I);
        let mark_out = shortcut(egui::Key::O);
        let append = shortcut(egui::Key::E);
        let insert = shortcut(egui::Key::W);
        let split = shortcut(egui::Key::S);
        let remove = shortcut(egui::Key::Delete);
        ui.separator();
        ui.heading("Timeline");
        ui.add_enabled_ui(!busy && interactive, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Source");
                let before = selected.source;
                egui::ComboBox::from_id_salt(("timeline-source", tab))
                    .selected_text(
                        scenes
                            .iter()
                            .find(|s| Some(s.id) == selected.source)
                            .unwrap()
                            .name
                            .clone(),
                    )
                    .show_ui(ui, |ui| {
                        for scene in &scenes {
                            ui.selectable_value(&mut selected.source, Some(scene.id), &scene.name);
                        }
                    });
                let source = scenes
                    .iter()
                    .find(|s| Some(s.id) == selected.source)
                    .unwrap();
                if before != selected.source {
                    selected.source_range = FrameRange {
                        start: 0,
                        end: source.duration,
                    };
                    if let Some(editbay_core::NodeOperation::Text {
                        text,
                        size,
                        position,
                        ..
                    }) = source
                        .nodes
                        .iter()
                        .map(|node| &node.operation)
                        .find(|op| matches!(op, editbay_core::NodeOperation::Text { .. }))
                    {
                        selected.title = text.clone();
                        selected.size = *size;
                        selected.baseline = *position;
                    }
                }
                if button(ui, preview, "timeline-view-source", "View source")
                    && let Err(error) = preview.select(
                        tab,
                        &project,
                        source.id,
                        selected.source_range.start.min(source.duration - 1),
                    )
                {
                    self.error = Some(error);
                }
            });
            let source = scenes
                .iter()
                .find(|s| Some(s.id) == selected.source)
                .unwrap();
            ui.horizontal_wrapped(|ui| {
                let position = preview
                    .position(tab, &project)
                    .filter(|(id, _)| *id == source.id)
                    .map(|(_, frame)| frame);
                if (ui
                    .add_enabled_ui(position.is_some(), |ui| {
                        button(ui, preview, "timeline-mark-in", "Mark in · I")
                    })
                    .inner
                    || mark_in)
                    && let Some(frame) = position
                {
                    selected.source_range.start = frame;
                }
                if (ui
                    .add_enabled_ui(position.is_some(), |ui| {
                        button(ui, preview, "timeline-mark-out", "Mark out · O")
                    })
                    .inner
                    || mark_out)
                    && let Some(frame) = position
                {
                    selected.source_range.end = frame + 1;
                }
                frame_input(
                    ui,
                    preview,
                    "timeline-source-in",
                    "In",
                    &mut selected.source_range.start,
                    source.duration,
                );
                frame_input(
                    ui,
                    preview,
                    "timeline-source-out",
                    "Out exclusive",
                    &mut selected.source_range.end,
                    source.duration,
                );
            });
            ui.collapsing("Titles and fades", |ui| {
                ui.label("Editable title · ASCII text · retained font");
                ui.text_edit_multiline(&mut selected.title);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Font file");
                    let mut path = selected.font.to_string_lossy().into_owned();
                    if ui.text_edit_singleline(&mut path).changed() {
                        selected.font = path.into();
                    }
                    ui.label("Size");
                    ui.add(egui::DragValue::new(&mut selected.size).range(1. ..=512.));
                    ui.label("Baseline X / Y");
                    for value in &mut selected.baseline {
                        ui.add(egui::DragValue::new(value));
                    }
                    ui.label("Frames");
                    ui.add(egui::DragValue::new(&mut selected.title_duration).range(1..=u64::MAX));
                    if ui.button("Create title source").clicked() {
                        action = Some(TimelineAction::Title {
                            profile: selected.record.unwrap_or(source.id),
                            name: selected.name.clone(),
                            text: selected.title.clone(),
                            font: selected.font.clone(),
                            size: selected.size,
                            position: selected.baseline,
                            rgba: [1.; 4],
                            duration: selected.title_duration,
                        });
                    }
                    if source.nodes.iter().any(|node| {
                        matches!(node.operation, editbay_core::NodeOperation::Text { .. })
                    }) && ui.button("Update selected title").clicked()
                    {
                        action = Some(TimelineAction::EditTitle {
                            composition: source.id,
                            text: selected.title.clone(),
                            size: selected.size,
                            position: selected.baseline,
                            rgba: [1.; 4],
                        });
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Picture and sound fade in / out frames");
                    ui.add(egui::DragValue::new(&mut selected.fade_in).range(0..=source.duration));
                    ui.add(egui::DragValue::new(&mut selected.fade_out).range(0..=source.duration));
                    if ui.button("Create faded source").clicked() {
                        action = Some(TimelineAction::Fade {
                            source: source.id,
                            name: format!("{} · fades", source.name),
                            fade_in: selected.fade_in,
                            fade_out: selected.fade_out,
                        });
                    }
                });
            });
            let source = SourceSelection {
                composition: selected.source.unwrap(),
                range: selected.source_range,
            };
            ui.horizontal_wrapped(|ui| {
                let name =
                    ui.add(egui::TextEdit::singleline(&mut selected.name).desired_width(130.));
                preview.observe_control("timeline-name", &name, ui);
                if button(ui, preview, "timeline-create", "Create cut") {
                    action = Some(TimelineAction::Create {
                        name: selected.name.clone(),
                        source,
                    });
                }
                if let Some(composition) = selected.record {
                    if let Some(record) = scenes.iter().find(|s| s.id == composition)
                        && button(
                            ui,
                            preview,
                            "timeline-conform",
                            "Conform source to record profile",
                        )
                    {
                        action = Some(TimelineAction::Conform {
                            source: source.composition,
                            width: record.width,
                            height: record.height,
                            frame_rate: record.frame_rate,
                            name: format!(
                                "{} · conformed",
                                scenes
                                    .iter()
                                    .find(|s| s.id == source.composition)
                                    .unwrap()
                                    .name
                            ),
                        });
                    }
                    egui::ComboBox::from_id_salt(("timeline-record", tab))
                        .selected_text(
                            scenes
                                .iter()
                                .find(|s| s.id == composition)
                                .map_or("Record", |s| s.name.as_str()),
                        )
                        .show_ui(ui, |ui| {
                            for assembly in &catalog.assemblies {
                                let name = scenes
                                    .iter()
                                    .find(|s| s.id == assembly.composition)
                                    .map_or("Record", |s| s.name.as_str());
                                if ui
                                    .selectable_value(
                                        &mut selected.record,
                                        Some(assembly.composition),
                                        name,
                                    )
                                    .changed()
                                {
                                    selected.clip = None;
                                }
                            }
                        });
                }
            });
            let Some(composition) = selected.record else {
                ui.weak("Mark a source range, then create a cut. Picture and sound stay linked.");
                return;
            };
            let assembly = catalog
                .assemblies
                .iter()
                .find(|a| a.composition == composition)
                .unwrap();
            let scene = scenes.iter().find(|scene| scene.id == composition).unwrap();
            if selected.track.is_none_or(|id| {
                !scene
                    .tracks
                    .iter()
                    .any(|track| track.id == id && track.kind == editbay_core::TrackKind::Video)
            }) {
                selected.track = Some(scene.tracks[0].id);
            }
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("record-track")
                    .selected_text(
                        scene
                            .tracks
                            .iter()
                            .find(|track| Some(track.id) == selected.track)
                            .unwrap()
                            .name
                            .clone(),
                    )
                    .show_ui(ui, |ui| {
                        for track in scene
                            .tracks
                            .iter()
                            .filter(|track| track.kind == editbay_core::TrackKind::Video)
                        {
                            ui.selectable_value(&mut selected.track, Some(track.id), &track.name);
                        }
                    });
                if button(ui, preview, "timeline-add-track", "Add track pair") {
                    action = Some(TimelineAction::AddTrack {
                        composition,
                        name: format!("Track {}", scene.tracks.len() / 2 + 1),
                    });
                }
                if button(ui, preview, "timeline-place", "Place on track") {
                    action = Some(TimelineAction::Place {
                        composition,
                        track: selected.track.unwrap(),
                        at: selected.at,
                        source,
                        audio_only: false,
                    });
                }
                if button(ui, preview, "timeline-place-sound", "Place sound only") {
                    let index = scene
                        .tracks
                        .iter()
                        .position(|track| Some(track.id) == selected.track)
                        .unwrap();
                    action = Some(TimelineAction::Place {
                        composition,
                        track: scene.tracks[index + 1].id,
                        at: selected.at,
                        source,
                        audio_only: true,
                    });
                }
                if button(ui, preview, "timeline-overwrite", "Overwrite V1") {
                    action = Some(TimelineAction::Overwrite {
                        composition,
                        at: selected.at,
                        source,
                    });
                }
            });
            let end = assembly
                .clips
                .iter()
                .map(|clip| clip.range.end)
                .max()
                .unwrap_or(0);
            ui.horizontal_wrapped(|ui| {
                frame_input(
                    ui,
                    preview,
                    "timeline-record-at",
                    "Record frame",
                    &mut selected.at,
                    i64::MAX as u64,
                );
                let position = preview
                    .position(tab, &project)
                    .filter(|(id, _)| *id == composition);
                if ui
                    .add_enabled_ui(position.is_some(), |ui| {
                        button(ui, preview, "timeline-use-playhead", "Use playhead")
                    })
                    .inner
                    && let Some((_, frame)) = position
                {
                    selected.at = frame;
                }
                if button(ui, preview, "timeline-view-record", "View record")
                    && let Err(error) = preview.select(
                        tab,
                        &project,
                        composition,
                        selected.at.min(end.saturating_sub(1)),
                    )
                {
                    self.error = Some(error);
                }
                if button(ui, preview, "timeline-append", "Append · E") || append {
                    action = Some(TimelineAction::Insert {
                        composition,
                        at: end,
                        source,
                    });
                }
                if button(ui, preview, "timeline-insert", "Insert · W") || insert {
                    action = Some(TimelineAction::Insert {
                        composition,
                        at: selected.at,
                        source,
                    });
                }
                ui.checkbox(&mut selected.ripple, "Ripple following clips");
            });
            let mut visual_clips = assembly.clips.clone();
            if let Some(scene) = draft_project
                .as_ref()
                .and_then(|p| p.compositions.iter().find(|c| c.id == composition))
            {
                for clip in &mut visual_clips {
                    if let Some(raw) = scene
                        .tracks
                        .iter()
                        .flat_map(|t| &t.clips)
                        .find(|c| c.id == clip.id)
                    {
                        clip.range = raw.range;
                    }
                }
            }
            if let Some(change) = self.handles.show(
                ui,
                composition,
                &visual_clips,
                |id| {
                    project
                        .compositions
                        .iter()
                        .find(|c| c.id == id)
                        .map_or(1, |c| c.duration)
                },
                selected.clip,
                selected.at,
                selected.ripple,
            ) {
                if selected.clip != Some(change.clip)
                    && let Some(clip) = assembly.clips.iter().find(|c| c.id == change.clip)
                {
                    selected.controls = clip.controls;
                    selected.trim_range = clip.source.range;
                    selected.at = clip.range.start;
                    selected.multiple.clear();
                }
                selected.clip = Some(change.clip);
                if !change.actions.is_empty() {
                    draft_actions = Some(change.actions);
                    finish_gesture |= change.finished;
                }
            }
            if !ui.ctx().text_edit_focused()
                && let Some(clip) = assembly.clips.iter().find(|c| Some(c.id) == selected.clip)
            {
                let modifiers = ui.input(|i| i.modifiers);
                let delta = if ui.input_mut(|i| i.consume_key(modifiers, egui::Key::ArrowLeft)) {
                    -1
                } else if ui.input_mut(|i| i.consume_key(modifiers, egui::Key::ArrowRight)) {
                    1
                } else {
                    0
                };
                if delta != 0 {
                    let tool = if modifiers.alt {
                        crate::timeline_handles::Tool::Slip
                    } else if modifiers.ctrl || modifiers.command {
                        crate::timeline_handles::Tool::Roll
                    } else {
                        self.handles.tool
                    };
                    let duration = project
                        .compositions
                        .iter()
                        .find(|c| c.id == clip.source.composition)
                        .map_or(1, |c| c.duration);
                    draft_actions = Some(crate::timeline_handles::actions(
                        composition,
                        clip,
                        tool,
                        delta * if modifiers.shift { 10 } else { 1 },
                        selected.ripple,
                        duration,
                    ));
                    finish_gesture = true;
                }
            }
            egui::ScrollArea::vertical()
                .id_salt("linked-clips")
                .max_height(112.)
                .show(ui, |ui| {
                    if assembly.clips.is_empty() {
                        ui.weak("Empty cut. Append a source range to begin.");
                    }
                    for clip in &assembly.clips {
                        let response = ui.selectable_label(
                            selected.clip == Some(clip.id),
                            format!(
                                "{} {}  {}–{}  ← {}–{}  {}",
                                scene
                                    .tracks
                                    .iter()
                                    .find(|track| track.id == clip.track)
                                    .unwrap()
                                    .name,
                                if clip.linked.is_some() {
                                    "+ A1 🔗"
                                } else {
                                    ""
                                },
                                clip.range.start,
                                clip.range.end,
                                clip.source.range.start,
                                clip.source.range.end,
                                clip.name
                            ),
                        );
                        preview.observe_control(
                            &format!("timeline-clip:{}", clip.id),
                            &response,
                            ui,
                        );
                        if response.clicked() {
                            if ui.input(|i| i.modifiers.command || i.modifiers.ctrl) {
                                if selected.multiple.is_empty() {
                                    selected.multiple.extend(selected.clip);
                                }
                                if selected.multiple.contains(&clip.id) {
                                    selected.multiple.retain(|id| *id != clip.id);
                                } else {
                                    selected.multiple.push(clip.id);
                                }
                            } else {
                                selected.multiple.clear();
                            }
                            selected.clip = Some(clip.id);
                            selected.trim_range = clip.source.range;
                            selected.at = clip.range.start;
                            selected.controls = clip.controls;
                        }
                    }
                });
            if let Some(clip) = assembly
                .clips
                .iter()
                .find(|clip| Some(clip.id) == selected.clip)
            {
                ui.horizontal_wrapped(|ui| {
                    let duration = project
                        .compositions
                        .iter()
                        .find(|c| c.id == clip.source.composition)
                        .unwrap()
                        .duration;
                    frame_input(
                        ui,
                        preview,
                        "timeline-trim-in",
                        "Trim source in",
                        &mut selected.trim_range.start,
                        duration,
                    );
                    frame_input(
                        ui,
                        preview,
                        "timeline-trim-out",
                        "out exclusive",
                        &mut selected.trim_range.end,
                        duration,
                    );
                    if button(ui, preview, "timeline-trim", "Apply trim") {
                        action = Some(TimelineAction::Trim {
                            composition,
                            clip: clip.id,
                            source_range: selected.trim_range,
                            ripple: selected.ripple,
                        });
                    }
                    if button(ui, preview, "timeline-split", "Split at record frame · S") || split
                    {
                        action = Some(TimelineAction::Split {
                            composition,
                            clip: clip.id,
                            at: selected.at,
                        });
                    }
                    if button(ui, preview, "timeline-move", "Move to record frame") {
                        action = Some(TimelineAction::Move {
                            composition,
                            clip: clip.id,
                            at: selected.at,
                        });
                    }
                    if button(ui, preview, "timeline-remove", "Remove · Delete") || remove {
                        action = Some(TimelineAction::Remove {
                            composition,
                            clip: clip.id,
                            ripple: selected.ripple,
                        });
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Slip frames");
                    ui.add(egui::DragValue::new(&mut selected.slip).speed(1));
                    if button(ui, preview, "timeline-slip", "Slip source") {
                        action = Some(TimelineAction::Slip {
                            composition,
                            clip: clip.id,
                            frames: selected.slip,
                        });
                    }
                    if button(ui, preview, "timeline-roll", "Roll cut to record frame") {
                        action = Some(TimelineAction::Roll {
                            composition,
                            clip: clip.id,
                            at: selected.at,
                        });
                    }
                    if button(ui, preview, "timeline-slide", "Slide to record frame") {
                        action = Some(TimelineAction::Slide {
                            composition,
                            clip: clip.id,
                            at: selected.at,
                        });
                    }
                });
                fields::section(ui, "Clip properties", |ui| {
                    ui.label(if selected.multiple.len() > 1 {
                        format!("{} clips · mixed properties", selected.multiple.len())
                    } else {
                        clip.name.clone()
                    });
                    let locked = !interactive || busy;
                    let state = FieldState {
                        locked,
                        ..Default::default()
                    };
                    let before = selected.controls;
                    let mixed_controls: Vec<_> = assembly
                        .clips
                        .iter()
                        .filter(|c| selected.multiple.contains(&c.id))
                        .map(|c| c.controls)
                        .collect();
                    let mut changed = false;
                    let mut changed_fields = Vec::new();
                    let mut finished = false;
                    let mut field = |ui: &mut Ui,
                                     id: &'static str,
                                     label: &str,
                                     value: &mut f64,
                                     range,
                                     unit,
                                     reset| {
                        let result = fields::number(
                            ui,
                            (tab, id),
                            label,
                            value,
                            range,
                            unit,
                            Some(reset),
                            FieldState {
                                mixed: mixed_controls
                                    .iter()
                                    .any(|c| control_value(id, *c) != control_value(id, before)),
                                locked: state.locked
                                    || if id == "timeline-gain" {
                                        !(clip.audio_only || clip.linked.is_some())
                                    } else {
                                        clip.audio_only
                                    },
                                ..state
                            },
                        );
                        preview.observe_control(id, &result.response, ui);
                        if result.changed {
                            changed = true;
                            changed_fields.push(id);
                        }
                        finished |= result.response.drag_stopped()
                            || result.response.lost_focus()
                            || (result.changed
                                && !result.response.dragged()
                                && !result.response.has_focus());
                        cancel_gesture |= result.cancelled;
                    };

                    let columns = if ui.available_width() < 220. { 1 } else { 2 };
                    let width = ((ui.available_width() - 8. * (columns - 1) as f32)
                        / columns as f32)
                        .max(76.);
                    let [scale_x, scale_y] = &mut selected.controls.scale;
                    let [position_x, position_y] = &mut selected.controls.translation;
                    let entries = [
                        (
                            "timeline-gain",
                            "Sound level",
                            &mut selected.controls.gain,
                            0.,
                            16.,
                            " ×",
                            1.,
                        ),
                        (
                            "timeline-opacity",
                            "Opacity",
                            &mut selected.controls.opacity,
                            0.,
                            1.,
                            " ×",
                            1.,
                        ),
                        (
                            "timeline-scale-X",
                            "Scale X",
                            scale_x,
                            -100.,
                            100.,
                            " ×",
                            1.,
                        ),
                        (
                            "timeline-scale-Y",
                            "Scale Y",
                            scale_y,
                            -100.,
                            100.,
                            " ×",
                            1.,
                        ),
                        (
                            "timeline-position-X",
                            "Position X",
                            position_x,
                            -1_000_000.,
                            1_000_000.,
                            " px",
                            0.,
                        ),
                        (
                            "timeline-position-Y",
                            "Position Y",
                            position_y,
                            -1_000_000.,
                            1_000_000.,
                            " px",
                            0.,
                        ),
                        (
                            "timeline-rotation",
                            "Rotation",
                            &mut selected.controls.rotation,
                            -36000.,
                            36000.,
                            "°",
                            0.,
                        ),
                        (
                            "timeline-exposure",
                            "Exposure",
                            &mut selected.controls.exposure,
                            -10.,
                            10.,
                            " stops",
                            0.,
                        ),
                        (
                            "timeline-contrast",
                            "Contrast",
                            &mut selected.controls.contrast,
                            0.,
                            4.,
                            " ×",
                            1.,
                        ),
                        (
                            "timeline-saturation",
                            "Saturation",
                            &mut selected.controls.saturation,
                            0.,
                            4.,
                            " ×",
                            1.,
                        ),
                    ];
                    egui::Grid::new(("clip-properties", tab))
                        .num_columns(columns)
                        .min_col_width(width)
                        .max_col_width(width)
                        .spacing([8., 8.])
                        .show(ui, |ui| {
                            for (index, (id, label, value, min, max, unit, reset)) in
                                entries.into_iter().enumerate()
                            {
                                ui.scope(|ui| {
                                    ui.set_width(width);
                                    field(ui, id, label, value, min..=max, unit, reset);
                                });
                                if (index + 1) % columns == 0 {
                                    ui.end_row();
                                }
                            }
                        });
                    if changed && selected.controls != before {
                        draft_action = Some(TimelineAction::SetControls {
                            composition,
                            clip: clip.id,
                            controls: selected.controls,
                        });
                    }
                    if changed && selected.multiple.len() > 1 {
                        draft_action = None;
                        draft_actions = Some(
                            assembly
                                .clips
                                .iter()
                                .filter(|c| selected.multiple.contains(&c.id))
                                .map(|c| TimelineAction::SetControls {
                                    composition,
                                    clip: c.id,
                                    controls: patch_controls(
                                        selected.controls,
                                        c.controls,
                                        &changed_fields,
                                    ),
                                })
                                .collect(),
                        );
                    }
                    finish_gesture |= finished;
                    ui.horizontal_wrapped(|ui| {
                        if fields::action(ui, "Reset clip properties", !locked).clicked() {
                            selected.controls = TimelineControls::default();
                            draft_action = Some(TimelineAction::SetControls {
                                composition,
                                clip: clip.id,
                                controls: selected.controls,
                            });
                            if selected.multiple.len() > 1 {
                                draft_action = None;
                                draft_actions = Some(
                                    assembly
                                        .clips
                                        .iter()
                                        .filter(|c| selected.multiple.contains(&c.id))
                                        .map(|c| TimelineAction::SetControls {
                                            composition,
                                            clip: c.id,
                                            controls: TimelineControls::default(),
                                        })
                                        .collect(),
                                );
                            }
                            finish_gesture = true;
                        }
                    });
                });
            }
        });
        if escape || cancel_gesture {
            self.draft = None;
            preview.set_draft(None);
            if let Some(selected) = self.selections.get_mut(&tab)
                && let Some(clip) = self
                    .catalog
                    .as_ref()
                    .and_then(|c| {
                        c.assemblies
                            .iter()
                            .find(|a| Some(a.composition) == selected.record)
                    })
                    .and_then(|a| a.clips.iter().find(|c| Some(c.id) == selected.clip))
            {
                selected.controls = clip.controls;
            }
        } else if let Some(actions) = draft_actions {
            self.update_drafts(workspace, tab, actions, finish_gesture || enter);
        } else if let Some(action) = draft_action {
            self.update_draft(workspace, tab, action, finish_gesture || enter);
        } else if (finish_gesture || enter)
            && let Some(draft) = &mut self.draft
        {
            draft.finished = true;
            ui.ctx().request_repaint();
        }
        if let Some(action) = action
            && let Err(error) = self.edit(workspace, tab, action)
        {
            self.error = Some(error);
        }
        if busy {
            ui.weak("Preparing edit…");
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }
}

fn control_value(id: &str, c: TimelineControls) -> f64 {
    match id {
        "timeline-gain" => c.gain,
        "timeline-opacity" => c.opacity,
        "timeline-scale-X" => c.scale[0],
        "timeline-scale-Y" => c.scale[1],
        "timeline-position-X" => c.translation[0],
        "timeline-position-Y" => c.translation[1],
        "timeline-rotation" => c.rotation,
        "timeline-exposure" => c.exposure,
        "timeline-contrast" => c.contrast,
        _ => c.saturation,
    }
}

fn patch_controls(
    after: TimelineControls,
    mut current: TimelineControls,
    fields: &[&str],
) -> TimelineControls {
    for field in fields {
        match *field {
            "timeline-gain" => current.gain = after.gain,
            "timeline-opacity" => current.opacity = after.opacity,
            "timeline-scale-X" => current.scale[0] = after.scale[0],
            "timeline-scale-Y" => current.scale[1] = after.scale[1],
            "timeline-position-X" => current.translation[0] = after.translation[0],
            "timeline-position-Y" => current.translation[1] = after.translation[1],
            "timeline-rotation" => current.rotation = after.rotation,
            "timeline-exposure" => current.exposure = after.exposure,
            "timeline-contrast" => current.contrast = after.contrast,
            "timeline-saturation" => current.saturation = after.saturation,
            _ => unreachable!("Unknown clip property field"),
        }
    }
    current
}

fn button(ui: &mut Ui, preview: &mut PreviewPane, name: &str, label: &str) -> bool {
    let response = ui.add(egui::Button::new(label).wrap_mode(egui::TextWrapMode::Extend));
    preview.observe_control(name, &response, ui);
    response.clicked()
}
fn frame_input(
    ui: &mut Ui,
    preview: &mut PreviewPane,
    name: &str,
    label: &str,
    value: &mut u64,
    end: u64,
) {
    ui.add(egui::Label::new(label).wrap_mode(egui::TextWrapMode::Extend));
    let response = ui.add(egui::DragValue::new(value).range(0..=end));
    preview.observe_control(name, &response, ui);
}

#[cfg(test)]
mod tests {
    use super::*;
    use editbay_core::{
        Composition, DocumentCommand, DocumentVersion, FrameRate, NodeOperation, Sequence,
        TimedNode,
    };
    use std::{thread, time::Instant};

    fn workspace() -> (tempfile::TempDir, Workspace, Uuid, Uuid) {
        let directory = tempfile::tempdir().unwrap();
        let mut workspace =
            Workspace::new(directory.path().join("recovery"), egui::Context::default());
        let mut project = Project::new("Timeline ownership").unwrap();
        let composition = Uuid::new_v4();
        let node = Uuid::new_v4();
        let rate = FrameRate::new(24, 1).unwrap();
        use sha2::Digest;
        let path = directory.path().join("source.wav");
        let sample_bytes = 192000u32;
        let mut wav = Vec::with_capacity(sample_bytes as usize + 44);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(sample_bytes + 36).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48000u32.to_le_bytes());
        wav.extend_from_slice(&96000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&sample_bytes.to_le_bytes());
        wav.resize(sample_bytes as usize + 44, 0);
        std::fs::write(&path, &wav).unwrap();
        let asset = Uuid::new_v4();
        let source = Uuid::new_v4();
        let audio_clip = Uuid::new_v4();
        let audio_node = Uuid::new_v4();
        project.assets.push(editbay_core::AssetReference {
            id: asset,
            kind: editbay_core::AssetKind::Media,
            path,
            sha256: format!("{:x}", sha2::Sha256::digest(&wav)),
            bytes: wav.len() as u64,
            provenance: "Generated PCM source for typed command tests; no device playback claim"
                .into(),
        });
        project.sources.push(editbay_core::MediaSource {
            id: source,
            name: "Generated PCM".into(),
            asset,
            metadata: Default::default(),
            streams: vec![editbay_core::SourceStream {
                index: 0,
                codec: "pcm_s16le".into(),
                time_base: editbay_core::TimeBase {
                    numerator: 1,
                    denominator: 48000,
                },
                start_tick: 0,
                duration_ticks: Some(96000),
                format: editbay_core::StreamFormat::Audio {
                    sample_rate: 48000,
                    channels: vec!["FC".into()],
                },
                metadata: Default::default(),
            }],
        });

        project.compositions.push(Composition {
            id: composition,
            name: "Solid source".into(),
            width: 16,
            height: 16,
            frame_rate: rate,
            duration: 48,
            picture: Some(node),
            audio: Some(audio_node),
            tracks: vec![editbay_core::Track {
                id: Uuid::new_v4(),
                name: "Source sound".into(),
                kind: editbay_core::TrackKind::Audio,
                enabled: true,
                clips: vec![editbay_core::Clip {
                    id: audio_clip,
                    name: "PCM".into(),
                    range: FrameRange { start: 0, end: 48 },
                    source: editbay_core::ClipSource::Media { source, stream: 0 },
                    time_map: editbay_core::TimeMap {
                        source_denominator: 1,
                        points: vec![
                            editbay_core::TimePoint {
                                frame: 0,
                                source_tick: 0,
                            },
                            editbay_core::TimePoint {
                                frame: 48,
                                source_tick: 96000,
                            },
                        ],
                    },
                    linked: None,
                }],
            }],
            nodes: vec![
                TimedNode {
                    id: audio_node,
                    range: FrameRange { start: 0, end: 48 },
                    operation: NodeOperation::Source { clip: audio_clip },
                    animation: vec![],
                },
                TimedNode {
                    id: node,
                    range: FrameRange { start: 0, end: 48 },
                    operation: NodeOperation::Solid { rgba: [1.; 4] },
                    animation: vec![],
                },
            ],
        });
        project.sequences = vec![Sequence {
            id: Uuid::new_v4(),
            name: "Source".into(),
            width: 16,
            height: 16,
            frame_rate: rate,
            composition: Some(composition),
        }];
        let tab = workspace.create(project).unwrap();
        (directory, workspace, tab, composition)
    }
    fn finish(pane: &mut TimelinePane, workspace: &mut Workspace, visible: bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while pane.task.is_some() {
            pane.poll(
                workspace,
                &mut PreviewPane::default(),
                visible,
                &egui::Context::default(),
            );
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn background_cut_commits_one_group_and_initializes_exact_trim_selection() {
        let (_dir, mut workspace, tab, composition) = workspace();
        let original = workspace.tabs[0].editor.snapshot();
        let mut pane = TimelinePane::default();
        pane.edit(
            &workspace,
            tab,
            TimelineAction::Create {
                name: "Cut".into(),
                source: SourceSelection {
                    composition,
                    range: FrameRange { start: 4, end: 12 },
                },
            },
        )
        .unwrap();
        finish(&mut pane, &mut workspace, true);
        assert!(pane.error.is_none(), "{:?}", pane.error);
        assert_eq!(workspace.tabs[0].editor.project().revision, 1);
        assert_eq!(
            pane.selections[&tab].trim_range,
            FrameRange { start: 4, end: 12 }
        );
        workspace.history(tab, false).unwrap();
        assert_eq!(
            workspace.tabs[0].editor.project().compositions,
            original.compositions
        );
    }
    #[test]
    fn changed_revision_closed_tab_or_hidden_workspace_cannot_receive_prepared_cut() {
        for change in 0..4 {
            let (_dir, mut workspace, tab, composition) = workspace();
            let mut pane = TimelinePane::default();
            pane.edit(
                &workspace,
                tab,
                TimelineAction::Create {
                    name: "Cut".into(),
                    source: SourceSelection {
                        composition,
                        range: FrameRange { start: 4, end: 12 },
                    },
                },
            )
            .unwrap();
            match change {
                0 => workspace
                    .apply(
                        tab,
                        DocumentVersion::of(workspace.tabs[0].editor.project()),
                        "Rename".into(),
                        &[DocumentCommand::RenameProject {
                            name: "Changed".into(),
                        }],
                    )
                    .unwrap(),
                1 => {
                    workspace.create(Project::new("Other").unwrap()).unwrap();
                }
                2 => {
                    workspace.tabs.clear();
                    workspace.active = None;
                }
                _ => {}
            }
            finish(&mut pane, &mut workspace, change != 3);
            assert!(
                pane.error
                    .as_ref()
                    .is_some_and(|error| error.contains("changed"))
            );
            assert!(
                workspace
                    .tabs
                    .iter()
                    .all(|tab| tab.editor.project().compositions.len() <= 1)
            );
        }
    }
    fn cut(
        pane: &mut TimelinePane,
        workspace: &mut Workspace,
        tab: Uuid,
        source: Uuid,
    ) -> (Uuid, Uuid) {
        pane.edit(
            workspace,
            tab,
            TimelineAction::Create {
                name: "Gesture cut".into(),
                source: SourceSelection {
                    composition: source,
                    range: FrameRange { start: 4, end: 20 },
                },
            },
        )
        .unwrap();
        finish(pane, workspace, true);
        let selection = &pane.selections[&tab];
        (selection.record.unwrap(), selection.clip.unwrap())
    }

    #[test]
    fn craft_gesture_preview_cancel_commit_undo_and_save_reopen_keep_typed_controls() {
        let (dir, mut workspace, tab, source) = workspace();
        let mut pane = TimelinePane::default();
        let (composition, clip) = cut(&mut pane, &mut workspace, tab, source);
        let original = workspace.tabs[0].editor.snapshot();
        let controls = TimelineControls {
            opacity: 0.25,
            gain: 0.5,
            exposure: 1.0,
            ..Default::default()
        };
        let action = TimelineAction::SetControls {
            composition,
            clip,
            controls,
        };
        pane.update_draft(&workspace, tab, action.clone(), false);
        finish(&mut pane, &mut workspace, true);
        assert_eq!(
            workspace.tabs[0].editor.project().revision,
            original.revision
        );
        assert!(pane.draft.as_ref().unwrap().ready.is_some());
        let draft = pane
            .draft
            .as_ref()
            .unwrap()
            .ready
            .as_ref()
            .unwrap()
            .project();
        assert_eq!(
            editbay_core::timeline_clips(draft, composition).unwrap()[0].controls,
            controls
        );
        pane.draft = None;
        assert_eq!(
            workspace.tabs[0].editor.project().compositions,
            original.compositions
        );
        pane.update_draft(&workspace, tab, action, true);
        finish(&mut pane, &mut workspace, true);
        assert_eq!(
            workspace.tabs[0].editor.project().revision,
            original.revision + 1
        );
        let path = dir.path().join("Gesture.editbay");
        editbay_core::save_new(workspace.tabs[0].editor.project(), &path).unwrap();
        let reopened = editbay_core::load(&path).unwrap();
        assert_eq!(
            editbay_core::timeline_clips(&reopened, composition).unwrap()[0].controls,
            controls
        );
        workspace.history(tab, false).unwrap();
        assert_eq!(
            workspace.tabs[0].editor.project().compositions,
            original.compositions
        );
        workspace.history(tab, true).unwrap();
        assert_eq!(
            editbay_core::timeline_clips(workspace.tabs[0].editor.project(), composition).unwrap()
                [0]
            .controls,
            controls
        );
    }

    #[test]
    fn craft_coalesced_gesture_commits_only_last_request_and_rejects_stale_owner() {
        for stale in [false, true] {
            let (_dir, mut workspace, tab, source) = workspace();
            let mut pane = TimelinePane::default();
            let (composition, clip) = cut(&mut pane, &mut workspace, tab, source);
            for gain in [0.1, 0.4, 0.8] {
                pane.update_draft(
                    &workspace,
                    tab,
                    TimelineAction::SetControls {
                        composition,
                        clip,
                        controls: TimelineControls {
                            gain,
                            ..Default::default()
                        },
                    },
                    gain == 0.8,
                );
            }
            if stale {
                workspace
                    .apply(
                        tab,
                        DocumentVersion::of(workspace.tabs[0].editor.project()),
                        "Unrelated edit".into(),
                        &[DocumentCommand::RenameProject {
                            name: "Changed".into(),
                        }],
                    )
                    .unwrap();
            }
            finish(&mut pane, &mut workspace, true);
            let actual =
                editbay_core::timeline_clips(workspace.tabs[0].editor.project(), composition)
                    .unwrap()[0]
                    .controls
                    .gain;
            assert_eq!(actual, if stale { 1.0 } else { 0.8 });
            assert_eq!(workspace.tabs[0].editor.project().revision, 2);
        }
    }

    #[test]
    fn craft_actual_mixed_field_reset_persists_and_undoes_when_primary_is_default() {
        fn text_bounds(shape: &egui::epaint::Shape, output: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::epaint::Shape::Vec(children) => {
                    for child in children {
                        text_bounds(child, output);
                    }
                }
                egui::epaint::Shape::Text(text) => {
                    output.push((text.galley.text().into(), shape.visual_bounding_rect()))
                }
                _ => {}
            }
        }
        let (directory, mut workspace, tab, source) = workspace();
        let mut pane = TimelinePane::default();
        let (composition, first) = cut(&mut pane, &mut workspace, tab, source);
        pane.edit(
            &workspace,
            tab,
            TimelineAction::Insert {
                composition,
                at: 16,
                source: SourceSelection {
                    composition: source,
                    range: FrameRange { start: 4, end: 20 },
                },
            },
        )
        .unwrap();
        finish(&mut pane, &mut workspace, true);
        let clips =
            editbay_core::timeline_clips(workspace.tabs[0].editor.project(), composition).unwrap();
        let second = clips.iter().find(|clip| clip.id != first).unwrap().id;
        let other = TimelineControls {
            opacity: 0.25,
            gain: 2.0,
            translation: [12.0, 24.0],
            ..Default::default()
        };
        pane.edit(
            &workspace,
            tab,
            TimelineAction::SetControls {
                composition,
                clip: second,
                controls: other,
            },
        )
        .unwrap();
        finish(&mut pane, &mut workspace, true);
        let (owner, editor) = workspace.edit_snapshot(tab).unwrap();
        pane.refresh(owner, editor.snapshot()).unwrap();
        finish(&mut pane, &mut workspace, true);
        let selected = pane.selections.get_mut(&tab).unwrap();
        selected.clip = Some(first);
        selected.controls = TimelineControls::default();
        selected.multiple = vec![first, second];
        let before = editor.snapshot();
        let ctx = egui::Context::default();
        let mut preview = PreviewPane::default();
        let raw = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900., 4000.),
            )),
            events,
            ..Default::default()
        };
        let mut bounds = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(raw(vec![]), |ui| {
                pane.show(ui, &workspace, &mut preview, tab, before.clone(), true);
            });
            output.textures_delta.clear();
            bounds.clear();
            for shape in &output.shapes {
                text_bounds(&shape.shape, &mut bounds);
            }
            output.drop_without_applying_deltas();
        }
        let opacity = bounds
            .iter()
            .find(|(label, _)| label == "Opacity")
            .unwrap()
            .1;
        let reset = bounds
            .iter()
            .filter(|(label, rect)| {
                label == "Reset"
                    && rect.left() >= opacity.left()
                    && rect.top() >= opacity.bottom()
                    && rect.top() < opacity.bottom() + 45.
            })
            .min_by(|a, b| a.1.top().total_cmp(&b.1.top()))
            .expect("The actual opacity Reset widget must be visible")
            .1;
        let point = reset.center();
        for pressed in [true, false] {
            ctx.run_ui(
                raw(vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]),
                |ui| pane.show(ui, &workspace, &mut preview, tab, before.clone(), true),
            )
            .drop_without_applying_deltas();
        }
        finish(&mut pane, &mut workspace, true);
        assert!(pane.error.is_none(), "{:?}", pane.error);
        assert_eq!(
            workspace.tabs[0].editor.project().revision,
            before.revision + 1
        );
        let expected = TimelineControls {
            opacity: 1.0,
            ..other
        };
        let controls = |project: &Project, id| {
            editbay_core::timeline_clips(project, composition)
                .unwrap()
                .into_iter()
                .find(|clip| clip.id == id)
                .unwrap()
                .controls
        };
        assert_eq!(
            controls(workspace.tabs[0].editor.project(), first),
            TimelineControls::default()
        );
        assert_eq!(
            controls(workspace.tabs[0].editor.project(), second),
            expected
        );
        let file = directory.path().join("Mixed reset.editbay");
        editbay_core::save_new(workspace.tabs[0].editor.project(), &file).unwrap();
        let reopened = editbay_core::load(&file).unwrap();
        assert_eq!(controls(&reopened, second), expected);
        workspace.history(tab, false).unwrap();
        assert_eq!(
            workspace.tabs[0].editor.project().compositions,
            before.compositions
        );
        workspace.history(tab, true).unwrap();
        assert_eq!(
            controls(workspace.tabs[0].editor.project(), second),
            expected
        );
        if let Some(destination) = std::env::var_os("EDITBAY_CRAFT_EVIDENCE") {
            use sha2::Digest;
            let destination = std::path::PathBuf::from(destination);
            std::fs::create_dir_all(&destination).unwrap();
            let mut saved = reopened.clone();
            for asset in &mut saved.assets {
                let target = destination.join("generated-source.wav");
                std::fs::copy(&asset.path, &target).unwrap();
                asset.path = target;
            }
            let file = destination.join("mixed-reset.editbay");
            editbay_core::save(&saved, &file).unwrap();
            let bytes = std::fs::read(&file).unwrap();
            let fixture = serde_json::json!({
                "app":"EditBay", "kind":"actual headless widget observations",
                "revision":std::env::var("EDITBAY_CRAFT_REVISION").ok(),
                "viewport_logical":[900,4000], "scale":1.0,
                "selection":[first,second], "primary_before":TimelineControls::default(),
                "secondary_before":other, "secondary_after":expected,
                "widget":{"label":"Reset Opacity", "pointer":[point.x,point.y], "pressed_then_released":true},
                "observed":{"typed_edit":true,"one_undo":true,"redo":true,"native_save_reopen":true,"unrelated_fields_preserved":true},
                "native_document":{"path":file,"sha256":format!("{:x}",sha2::Sha256::digest(&bytes))},
                "boundary":"Observed real egui widget pointer events and native-format persistence; no desktop, GPU, physical audio or appearance golden proof. Retained document assets are relocated to the retained identical PCM source."
            });
            std::fs::write(
                destination.join("ui-fixture.json"),
                serde_json::to_vec_pretty(&fixture).unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn craft_mixed_property_patch_preserves_unedited_clip_values() {
        let before = TimelineControls::default();
        let after = TimelineControls {
            opacity: 0.5,
            ..before
        };
        let other = TimelineControls {
            gain: 3.0,
            translation: [12.0, 24.0],
            ..before
        };
        assert_eq!(
            patch_controls(after, other, &["timeline-opacity"]),
            TimelineControls {
                opacity: 0.5,
                ..other
            }
        );
    }
    #[test]
    fn direct_handle_actions_preview_commit_and_undo_through_existing_commands() {
        use crate::timeline_handles::{Tool, actions};
        for tool in [
            Tool::Move,
            Tool::TrimStart,
            Tool::TrimEnd,
            Tool::Slip,
            Tool::Roll,
            Tool::Slide,
        ] {
            let (_dir, mut workspace, tab, source) = workspace();
            let mut pane = TimelinePane::default();
            let (composition, first) = cut(&mut pane, &mut workspace, tab, source);
            for at in [16, 32] {
                pane.edit(
                    &workspace,
                    tab,
                    TimelineAction::Insert {
                        composition,
                        at,
                        source: SourceSelection {
                            composition: source,
                            range: FrameRange { start: 4, end: 20 },
                        },
                    },
                )
                .unwrap();
                finish(&mut pane, &mut workspace, true);
            }
            let original = workspace.tabs[0].editor.snapshot();
            let clips = editbay_core::timeline_clips(&original, composition).unwrap();
            let clip = if tool == Tool::Slide {
                &clips[1]
            } else {
                clips.iter().find(|c| c.id == first).unwrap()
            };
            let delta = if tool == Tool::Move { 50 } else { 1 };
            pane.update_drafts(
                &workspace,
                tab,
                actions(composition, clip, tool, delta, false, 48),
                true,
            );
            finish(&mut pane, &mut workspace, true);
            assert!(pane.error.is_none(), "{tool:?}: {:?}", pane.error);
            assert_eq!(
                workspace.tabs[0].editor.project().revision,
                original.revision + 1
            );
            assert_ne!(
                workspace.tabs[0].editor.project().compositions,
                original.compositions
            );
            workspace.history(tab, false).unwrap();
            assert_eq!(
                workspace.tabs[0].editor.project().compositions,
                original.compositions,
                "{tool:?}"
            );
        }
    }

    #[test]
    fn compact_inspector_draws_empty_single_mixed_and_locked_at_three_scales() {
        for scale in [1., 1.5, 2.] {
            let (_dir, mut workspace, tab, source) = workspace();
            let mut pane = TimelinePane::default();
            let (composition, clip) = cut(&mut pane, &mut workspace, tab, source);
            pane.edit(
                &workspace,
                tab,
                TimelineAction::Insert {
                    composition,
                    at: 16,
                    source: SourceSelection {
                        composition: source,
                        range: FrameRange { start: 4, end: 20 },
                    },
                },
            )
            .unwrap();
            finish(&mut pane, &mut workspace, true);
            let (owner, editor) = workspace.edit_snapshot(tab).unwrap();
            pane.refresh(owner, editor.snapshot()).unwrap();
            finish(&mut pane, &mut workspace, true);
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(scale);
            for state in 0..4 {
                let selected = pane.selections.get_mut(&tab).unwrap();
                selected.clip = if state == 0 { None } else { Some(clip) };
                selected.multiple = if state == 2 {
                    editbay_core::timeline_clips(editor.project(), composition)
                        .unwrap()
                        .iter()
                        .map(|c| c.id)
                        .collect()
                } else {
                    Vec::new()
                };
                let mut preview = PreviewPane::default();
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(480., 1400.),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        pane.show(
                            ui,
                            &workspace,
                            &mut preview,
                            tab,
                            editor.snapshot(),
                            state != 3,
                        );
                    },
                )
                .drop_without_applying_deltas();
                assert!(
                    pane.error.is_none(),
                    "scale {scale} state {state}: {:?}",
                    pane.error
                );
                assert_eq!(workspace.tabs[0].editor.project().revision, 2);
            }
        }
    }
    #[test]
    fn craft_idle_mixed_and_disabled_fields_preserve_imported_out_of_range_values() {
        for original in [1.2e7_f64, -6e6, 2e6, 720.] {
            for (mixed, locked, enabled) in [
                (false, false, true),
                (true, false, true),
                (false, true, true),
                (false, false, false),
            ] {
                let ctx = egui::Context::default();
                let mut value = original;
                for _ in 0..2 {
                    ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(360., 240.),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            ui.add_enabled_ui(enabled, |ui| {
                                let response = fields::number(
                                    ui,
                                    ("imported", original.to_bits()),
                                    "Imported value",
                                    &mut value,
                                    -360. ..=360.,
                                    " px",
                                    Some(0.),
                                    FieldState {
                                        mixed,
                                        locked,
                                        error: None,
                                    },
                                );
                                assert!(!response.changed);
                                assert!(!response.committed);
                            });
                        },
                    )
                    .drop_without_applying_deltas();
                    assert_eq!(
                        value, original,
                        "mixed={mixed} locked={locked} enabled={enabled}"
                    );
                }
            }
        }
    }
    #[test]
    fn craft_real_inspector_labels_fit_at_display_scales_for_selection_states() {
        fn labels(shape: &egui::epaint::Shape, clip: egui::Rect, count: &mut usize) {
            match shape {
                egui::epaint::Shape::Vec(children) => {
                    for child in children {
                        labels(child, clip, count);
                    }
                }
                egui::epaint::Shape::Text(text)
                    if [
                        "Sound level",
                        "Opacity",
                        "Scale X",
                        "Scale Y",
                        "Position X",
                        "Position Y",
                        "Rotation",
                        "Exposure",
                        "Contrast",
                        "Saturation",
                    ]
                    .contains(&text.galley.text()) =>
                {
                    let bounds = shape.visual_bounding_rect();
                    assert!(
                        clip.expand(1.).contains_rect(bounds),
                        "{} clipped: {bounds:?} outside {clip:?}",
                        text.galley.text()
                    );
                    *count += 1;
                }
                _ => {}
            }
        }
        for scale in [1., 1.5, 2.] {
            let (_dir, mut workspace, tab, source) = workspace();
            let mut pane = TimelinePane::default();
            let (composition, first) = cut(&mut pane, &mut workspace, tab, source);
            pane.edit(
                &workspace,
                tab,
                TimelineAction::Insert {
                    composition,
                    at: 16,
                    source: SourceSelection {
                        composition: source,
                        range: FrameRange { start: 4, end: 20 },
                    },
                },
            )
            .unwrap();
            finish(&mut pane, &mut workspace, true);
            let clips =
                editbay_core::timeline_clips(workspace.tabs[0].editor.project(), composition)
                    .unwrap();
            let second = clips.iter().find(|c| c.id != first).unwrap().id;
            pane.edit(
                &workspace,
                tab,
                TimelineAction::SetControls {
                    composition,
                    clip: second,
                    controls: TimelineControls {
                        gain: 0.5,
                        ..Default::default()
                    },
                },
            )
            .unwrap();
            finish(&mut pane, &mut workspace, true);
            let (owner, editor) = workspace.edit_snapshot(tab).unwrap();
            pane.refresh(owner, editor.snapshot()).unwrap();
            finish(&mut pane, &mut workspace, true);
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(scale);
            for state in 0..4 {
                let selected = pane.selections.get_mut(&tab).unwrap();
                selected.clip = if state == 0 { None } else { Some(first) };
                selected.controls = TimelineControls::default();
                selected.multiple = if state == 2 {
                    vec![first, second]
                } else {
                    vec![]
                };
                let mut preview = PreviewPane::default();
                let mut observed = 0;
                for _ in 0..3 {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(480. / scale, 4000. / scale),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            pane.show(
                                ui,
                                &workspace,
                                &mut preview,
                                tab,
                                editor.snapshot(),
                                state != 3,
                            )
                        },
                    );
                    output.textures_delta.clear();
                    let mut count = 0;
                    for clipped in &output.shapes {
                        labels(&clipped.shape, clipped.clip_rect, &mut count);
                    }
                    observed = count;
                    if state != 0 && count > 0 {
                        assert_eq!(count, 10, "scale {scale} state {state}");
                    }
                    output.drop_without_applying_deltas();
                }
                assert_eq!(
                    observed,
                    if state == 0 { 0 } else { 10 },
                    "scale {scale} state {state}: missing property labels"
                );
                assert_eq!(workspace.tabs[0].editor.project().revision, 3);
            }
        }
    }
}
