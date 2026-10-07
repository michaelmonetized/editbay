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
        }
    }
}

#[derive(Default)]
pub struct TimelinePane {
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
        self.selections
            .retain(|tab, _| workspace.tabs.iter().any(|item| item.id == *tab));
        let Some(task) = &self.task else {
            return;
        };
        ctx.request_repaint_after(Duration::from_millis(16));
        let owner = task.owner;
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
        self.task = Some(Task { owner, result });
        self.error = None;
        Ok(())
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
        self.task = Some(Task { owner, result });
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
        let busy = self.task.is_some();
        let mut action = None;
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
                ui.horizontal_wrapped(|ui| {
                    ui.label("Sound level");
                    ui.add(
                        egui::DragValue::new(&mut selected.controls.gain)
                            .speed(0.01)
                            .range(0. ..=16.),
                    );
                    ui.label("Opacity");
                    ui.add(
                        egui::DragValue::new(&mut selected.controls.opacity)
                            .speed(0.01)
                            .range(0. ..=1.),
                    );
                    ui.label("Scale X / Y");
                    for value in &mut selected.controls.scale {
                        ui.add(egui::DragValue::new(value).speed(0.01));
                    }
                    ui.label("Position X / Y");
                    for value in &mut selected.controls.translation {
                        ui.add(egui::DragValue::new(value).speed(1.));
                    }
                    ui.label("Rotation");
                    ui.add(egui::DragValue::new(&mut selected.controls.rotation).speed(1.));
                    ui.label("Exposure stops");
                    ui.add(
                        egui::DragValue::new(&mut selected.controls.exposure)
                            .speed(0.05)
                            .range(-10. ..=10.),
                    );
                    ui.label("Contrast");
                    ui.add(
                        egui::DragValue::new(&mut selected.controls.contrast)
                            .speed(0.01)
                            .range(0. ..=4.),
                    );
                    ui.label("Saturation");
                    ui.add(
                        egui::DragValue::new(&mut selected.controls.saturation)
                            .speed(0.01)
                            .range(0. ..=4.),
                    );
                    if button(ui, preview, "timeline-controls", "Apply clip controls") {
                        action = Some(TimelineAction::SetControls {
                            composition,
                            clip: clip.id,
                            controls: selected.controls,
                        });
                    }
                });
            }
        });
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
        project.compositions.push(Composition {
            id: composition,
            name: "Solid source".into(),
            width: 16,
            height: 16,
            frame_rate: rate,
            duration: 48,
            picture: Some(node),
            audio: None,
            tracks: vec![],
            nodes: vec![TimedNode {
                id: node,
                range: FrameRange { start: 0, end: 48 },
                operation: NodeOperation::Solid { rgba: [1.; 4] },
                animation: vec![],
            }],
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
}
