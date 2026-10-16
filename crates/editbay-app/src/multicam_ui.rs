use crate::{
    fields::{self, FieldState},
    keyboard_ui,
    preview::PreviewPane,
    workspace::{DocumentOwner, Workspace},
};
use editbay_audio::{SoundRenderBudget, SoundRenderer};
use editbay_core::{
    DocumentCommand, DocumentEditor, DocumentVersion, EvaluationSnapshot, Project, Sequence,
    SoundBudget, SoundSnapshot, SourcePosition, TimeBase,
    multicam::{
        AudioSyncOptions, MulticamAngle, MulticamAudioPolicy, MulticamSession, MulticamTimecode,
        correlate_audio,
    },
};
use editbay_media::{Cancellation, NativePcmCache, PcmBudget};
use eframe::egui::{self, Ui};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::Duration,
};
use uuid::Uuid;

struct Completion {
    editor: DocumentEditor,
    session: Uuid,
    focus: Option<(Uuid, u64)>,
    message: String,
}

struct Task {
    owner: DocumentOwner,
    activation: u64,
    cancel: Cancellation,
    result: Receiver<Result<Completion, String>>,
    discarded: bool,
}

impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[derive(Default)]
struct SyncFields {
    marker: String,
    timecode: String,
}

pub struct MulticamPane {
    chosen_sources: HashSet<Uuid>,
    name: String,
    selected: Option<Uuid>,
    fields: HashMap<Uuid, SyncFields>,
    frames: HashMap<Uuid, u64>,
    audio_reference: Option<Uuid>,
    audio_candidate: Option<Uuid>,
    audio_seconds: u32,
    audio_offset_seconds: u32,
    cut_mode: bool,
    focus: Option<(Uuid, Uuid, u64)>,
    status: Option<String>,
    task: Option<Task>,
}

impl Default for MulticamPane {
    fn default() -> Self {
        Self {
            chosen_sources: HashSet::new(),
            name: "Multicam edit".into(),
            selected: None,
            fields: HashMap::new(),
            frames: HashMap::new(),
            audio_reference: None,
            audio_candidate: None,
            audio_seconds: 20,
            audio_offset_seconds: 5,
            cut_mode: true,
            focus: None,
            status: None,
            task: None,
        }
    }
}

impl MulticamPane {
    /// Publish only a worker that still owns the visible tab and document generation.
    /// `workspace` and `visible` identify the native session; returns without waiting.
    pub fn poll(&mut self, workspace: &mut Workspace, visible: bool) {
        let Some(task) = &mut self.task else {
            return;
        };
        if !visible
            || workspace.active != Some(task.owner.tab)
            || workspace.activation_generation != task.activation
            || !workspace.owns(task.owner)
            || task.cancel.is_cancelled()
        {
            task.cancel.cancel();
            task.discarded = true;
        }
        let result = match task.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("Multicam worker stopped before completing the edit".into())
            }
        };
        let owner = task.owner;
        let discarded = task.discarded;
        self.task = None;
        if discarded {
            self.status = Some(
                "Multicam work was cancelled or its project changed; the result was discarded"
                    .into(),
            );
            return;
        }
        match result.and_then(|done| {
            if DocumentVersion::of(done.editor.project()) != owner.version {
                workspace.commit_edit(owner, done.editor)?;
            }
            Ok((done.session, done.focus, done.message))
        }) {
            Ok((session, focus, message)) => {
                self.selected = Some(session);
                self.focus = focus.map(|(composition, frame)| (owner.tab, composition, frame));
                self.status = Some(message);
            }
            Err(error) => self.status = Some(error),
        }
    }

    /// Draw real multicam sources, sync controls and shared-command angle edits.
    /// `ui`, `workspace`, `preview`, `tab` and `project` identify the visible document.
    /// Expensive compilation and native audio decoding run only on one owned worker.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        workspace: &mut Workspace,
        preview: &mut PreviewPane,
        tab: Uuid,
        project: Arc<Project>,
    ) {
        if let Some((owner, composition, frame)) = self.focus.take()
            && owner == tab
            && let Err(error) = preview.select(tab, &project, composition, frame)
        {
            self.status = Some(error);
        }
        let owner = ("multicam", tab, project.id);
        ui.heading("Multicam");
        if let Some(message) = &self.status {
            ui.label(message);
        }
        if let Some(task) = &self.task {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(if task.discarded {
                    "Stopping stale multicam work…"
                } else {
                    "Preparing multicam edit…"
                });
                if keyboard_ui::button(ui, (owner, "cancel"), "Cancel multicam work").clicked() {
                    task.cancel.cancel();
                }
            });
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        ui.add_enabled_ui(self.task.is_none(), |ui| {
            ui.collapsing("Create from camera sequences", |ui| {
                ui.label("Select independent camera compositions");
                self.chosen_sources.retain(|id| project.compositions.iter().any(|scene| scene.id == *id));
                for scene in project.compositions.iter().filter(|scene| scene.picture.is_some() && !project.multicam.iter().any(|session| session.composition == scene.id)) {
                    let mut checked = self.chosen_sources.contains(&scene.id);
                    let response = fields::native_scope(ui, egui::Id::new((owner, scene.id, "camera-source")), |ui| ui.checkbox(&mut checked, format!("Camera source: {} · {} × {} · {}/{} fps", scene.name, scene.width, scene.height, scene.frame_rate.numerator, scene.frame_rate.denominator))).inner;
                    keyboard_ui::focus(ui, &response);
                    if response.changed() {
                        if checked { self.chosen_sources.insert(scene.id); } else { self.chosen_sources.remove(&scene.id); }
                    }
                }
                let response = keyboard_ui::text(ui, (owner, "name"), "Multicam name", &mut self.name, false);
                let name_valid = !self.name.trim().is_empty() && self.name.len() <= 1024 && !self.name.contains(['\n', '\r', '\0']);
                if !name_valid { keyboard_ui::error(ui, &[response], "Enter a single-line multicam name of 1–1024 bytes"); }
                if ui.add_enabled_ui(self.chosen_sources.len() >= 2 && name_valid, |ui| keyboard_ui::button(ui, (owner, "create"), "Create multicam edit")).inner.clicked() {
                    let sources: Vec<_> = project.compositions.iter().filter(|scene| self.chosen_sources.contains(&scene.id)).map(|scene| scene.id).collect();
                    let name = self.name.clone();
                    self.start(workspace, tab, ui.ctx(), move |mut editor, project, cancel| {
                        check(&cancel)?;
                        let session = MulticamSession::new(&project, name, &sources).map_err(|error| error.to_string())?;
                        let id = session.id;
                        let composition = session.composition;
                        let sequence = Sequence { id: Uuid::new_v4(), name: session.name.clone(), width: session.width, height: session.height, frame_rate: session.frame_rate, composition: Some(composition) };
                        apply(&mut editor, "Create multicam edit", &[DocumentCommand::SetMulticamSession { session }, DocumentCommand::SetSequence { sequence }], &cancel)?;
                        Ok(Completion { editor, session: id, focus: Some((composition, 0)), message: "Created an editable multicam sequence; original cameras are retained".into() })
                    });
                }
                ui.weak("Camera sizes must match. Use Conform before combining different sizes.");
            });
        });
        if project.multicam.is_empty() {
            ui.weak("Create a multicam edit to synchronize cameras and cut angles.");
            return;
        }
        if !self
            .selected
            .is_some_and(|id| project.multicam.iter().any(|session| session.id == id))
        {
            self.selected = project.multicam.first().map(|session| session.id);
        }
        keyboard_ui::choice(
            ui,
            (owner, "session"),
            "Multicam session",
            &mut self.selected,
            &project
                .multicam
                .iter()
                .map(|session| (Some(session.id), session.name.clone()))
                .collect::<Vec<_>>(),
        );
        let Some(session) = project
            .multicam
            .iter()
            .find(|session| Some(session.id) == self.selected)
        else {
            return;
        };
        let overlap = match session.overlap(&project) {
            Ok(overlap) => overlap,
            Err(error) => {
                self.status = Some(error.to_string());
                return;
            }
        };
        let viewer = preview.position(tab, &project);
        let program_visible =
            viewer.is_some_and(|(composition, _)| composition == session.composition);
        if let Some((composition, frame)) = viewer
            && composition == session.composition
        {
            self.frames.insert(session.id, frame);
        }
        let frame = self
            .frames
            .get(&session.id)
            .copied()
            .unwrap_or(0)
            .min(overlap.duration.saturating_sub(1));
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Program frame {frame} / {} · common origin {}",
                overlap.duration, overlap.origin
            ));
            if keyboard_ui::button(ui, (owner, session.id, "view-program"), "View program")
                .clicked()
            {
                self.view(
                    workspace,
                    preview,
                    tab,
                    &project,
                    (session.id, session.composition, frame),
                    ui.ctx(),
                );
            }
        });
        ui.add_enabled_ui(self.task.is_none(), |ui| {
            ui.horizontal_wrapped(|ui| {
                for (mode, name) in [(true, "Cut at playhead"), (false, "Switch whole segment")] {
                    let response = fields::native_scope(ui, egui::Id::new((owner, session.id, "cut-mode", mode)), |ui| ui.radio_value(&mut self.cut_mode, mode, name)).inner;
                    keyboard_ui::focus(ui, &response);
                }
            });
            if !program_visible { ui.weak("View the program to cut or switch at its playhead."); }
            let active = session.cuts.iter().rev().find(|cut| cut.frame <= frame).map(|cut| cut.angle);
            egui::Grid::new(("multicam-angle-grid", session.id)).num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
                for (index, angle) in session.angles.iter().enumerate() {
                    ui.group(|ui| {
                        let time = angle_position(&project, session, angle, overlap.origin, frame);
                        let position_label = time.as_ref().map(|position| format!("Source frame {}/{}", position.numerator, position.denominator)).unwrap_or_else(|error| error.clone());
                        let name = format!("{} · {}{}\n{}", index + 1, angle.name, if active == Some(angle.id) { " · Active" } else { "" }, position_label);
                        let response = fields::native_scope(ui, egui::Id::new((owner, session.id, angle.id, "camera-cut")), |ui| ui.add_enabled(program_visible && time.is_ok(), egui::Button::new(name).selected(active == Some(angle.id)))).inner;
                        keyboard_ui::focus(ui, &response);
                        if response.clicked() {
                            let mut next = session.clone();
                            let cut_mode = self.cut_mode;
                            let angle = angle.id;
                            self.start(workspace, tab, ui.ctx(), move |mut editor, project, cancel| {
                                if cut_mode { next.cut_at(&project, frame, angle) } else { next.switch_at(&project, frame, angle) }.map_err(|error| error.to_string())?;
                                apply(&mut editor, if cut_mode { "Cut multicam angle" } else { "Switch multicam segment" }, &[DocumentCommand::SetMulticamSession { session: next.clone() }], &cancel)?;
                                Ok(Completion { editor, session: next.id, focus: None, message: if cut_mode { format!("Camera cut recorded at frame {frame}") } else { "Camera changed for the existing segment".into() } })
                            });
                        }
                        if ui.add_enabled_ui(time.as_ref().is_ok_and(|position| position.numerator >= 0), |ui| keyboard_ui::button(ui, (owner, session.id, angle.id, "view-source"), &format!("View {} source frame", angle.name))).inner.clicked() {
                            match time.and_then(|position| {
                                u64::try_from(position.numerator).map(|numerator| numerator / position.denominator).map_err(|_| "Source frame is before the camera start".to_owned())
                            }) {
                                Ok(source_frame) => self.view(workspace, preview, tab, &project, (session.id, angle.composition, source_frame), ui.ctx()),
                                Err(error) => self.status = Some(error),
                            }
                        }
                    });
                    if index % 2 == 1 { ui.end_row(); }
                }
            });
            ui.weak("The grid shows exact source positions. View source opens that position's containing frame in the native viewer.");
            self.audio_policy(ui, workspace, tab, &project, session);
            self.sync_controls(ui, workspace, tab, &project, session);
            ui.collapsing("Recorded camera cuts", |ui| {
                for cut in &session.cuts {
                    if let Ok(angle) = session.angle(cut.angle) {
                        ui.label(format!("Frame {} · {}", cut.frame, angle.name));
                    }
                }
            });
        });
    }

    /// Draw and commit the retained linked-audio policy.
    /// `ui`, `workspace`, `tab`, `project` and `session` select eligible shared-command edits.
    fn audio_policy(
        &mut self,
        ui: &mut Ui,
        workspace: &mut Workspace,
        tab: Uuid,
        project: &Project,
        session: &MulticamSession,
    ) {
        let mut policy = session.audio_policy;
        let sound_angles: Vec<_> = session
            .angles
            .iter()
            .filter(|angle| {
                project
                    .compositions
                    .iter()
                    .any(|camera| camera.id == angle.composition && camera.audio.is_some())
            })
            .collect();
        let mut choices = Vec::new();
        if sound_angles.len() == session.angles.len() {
            choices.push((
                MulticamAudioPolicy::FollowVideo,
                "Sound follows camera".into(),
            ));
        }
        choices.push((
            MulticamAudioPolicy::All,
            "Mix all available camera sound".into(),
        ));
        choices.extend(sound_angles.iter().map(|angle| {
            (
                MulticamAudioPolicy::Fixed { angle: angle.id },
                format!("Keep {} sound", angle.name),
            )
        }));
        keyboard_ui::choice(
            ui,
            ("multicam-audio-policy", tab, session.id),
            "Multicam sound",
            &mut policy,
            &choices,
        );
        if sound_angles.len() != session.angles.len() {
            ui.weak("Sound follows camera needs sound on every camera. Fixed choices include cameras with sound.");
        }
        if policy != session.audio_policy {
            let mut next = session.clone();
            next.audio_policy = policy;
            self.set_session(
                workspace,
                tab,
                ui.ctx(),
                next,
                "Change multicam sound policy",
            );
        }
    }

    /// Inspect the existing multicam owner and result for native-key diagnostics.
    /// Args: this pane. Returns selected session, captured worker owner and current status.
    #[cfg(test)]
    pub(crate) fn keyboard_worker_state(
        &self,
    ) -> (Option<Uuid>, Option<DocumentOwner>, Option<&str>) {
        (
            self.selected,
            self.task.as_ref().map(|task| task.owner),
            self.status.as_deref(),
        )
    }

    /// Draw marker, camera-clock and real decoded-audio synchronization.
    /// The supplied UI and document ownership select exact retained session changes.
    fn sync_controls(
        &mut self,
        ui: &mut Ui,
        workspace: &mut Workspace,
        tab: Uuid,
        project: &Arc<Project>,
        session: &MulticamSession,
    ) {
        ui.collapsing("Synchronize cameras", |ui| {
            let owner = ("multicam-sync", tab, project.id, session.id);
            let mut markers_valid = true;
            let mut clocks_valid = true;
            egui::Grid::new(("multicam-sync-fields", session.id)).num_columns(3).show(ui, |ui| {
                ui.label("Camera"); ui.label("Marker frame or fraction"); ui.label("Start timecode"); ui.end_row();
                for angle in &session.angles {
                    let fields = self.fields.entry(angle.id).or_insert_with(|| SyncFields { marker: "0".into(), timecode: "00:00:00:00".into() });
                    ui.label(&angle.name);
                    ui.vertical(|ui| {
                        let response = keyboard_ui::text(ui, (owner, angle.id, "marker"), &format!("{} marker frame or fraction", angle.name), &mut fields.marker, false);
                        let marker = parse_position(&fields.marker).and_then(|position| {
                            let camera = project.compositions.iter().find(|scene| scene.id == angle.composition).ok_or("Camera composition is absent")?;
                            let end = i64::try_from(camera.duration).map_err(|_| "Camera duration exceeds integer time")?;
                            if position.numerator < 0 || position.compare_tick(end).map_err(|error| error.to_string())? != std::cmp::Ordering::Less {
                                return Err("Marker must be inside its camera composition".into());
                            }
                            Ok(position)
                        });
                        if let Err(error) = marker {
                            markers_valid = false;
                            keyboard_ui::error(ui, &[response], &error);
                        }
                    });
                    ui.vertical(|ui| {
                        let response = keyboard_ui::text(ui, (owner, angle.id, "timecode"), &format!("{} start timecode", angle.name), &mut fields.timecode, false);
                        let clock = project.compositions.iter().find(|scene| scene.id == angle.composition)
                            .ok_or_else(|| "Camera composition is absent".to_owned())
                            .and_then(|scene| MulticamTimecode::parse(&fields.timecode, scene.frame_rate).map_err(|error| error.to_string()));
                        if let Err(error) = clock {
                            clocks_valid = false;
                            keyboard_ui::error(ui, &[response], &error);
                        }
                    });
                    ui.end_row();
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled_ui(markers_valid, |ui| keyboard_ui::button(ui, (owner, "markers-apply"), "Sync marked event")).inner.clicked() && markers_valid {
                    let values: Result<Vec<_>, String> = session.angles.iter().map(|angle| parse_position(&self.fields[&angle.id].marker).map(|position| (angle.id, position))).collect();
                    match values {
                        Ok(markers) => {
                            let mut next = session.clone();
                            self.start(workspace, tab, ui.ctx(), move |mut editor, project, cancel| {
                                next.sync_markers(&project, &markers).map_err(|error| error.to_string())?;
                                apply(&mut editor, "Sync multicam markers", &[DocumentCommand::SetMulticamSession { session: next.clone() }], &cancel)?;
                                Ok(Completion { editor, session: next.id, focus: Some((next.composition, 0)), message: "Camera markers aligned exactly".into() })
                            });
                        }
                        Err(error) => self.status = Some(error),
                    }
                }
                if ui.add_enabled_ui(clocks_valid, |ui| keyboard_ui::button(ui, (owner, "clocks-apply"), "Sync start timecodes")).inner.clicked() && clocks_valid {
                    let clocks: Result<Vec<_>, String> = session.angles.iter().map(|angle| {
                        let rate = project.compositions.iter().find(|scene| scene.id == angle.composition).ok_or("Camera composition is absent")?.frame_rate;
                        MulticamTimecode::parse(&self.fields[&angle.id].timecode, rate).map(|clock| (angle.id, clock)).map_err(|error| error.to_string())
                    }).collect();
                    match clocks {
                        Ok(clocks) => {
                            let mut next = session.clone();
                            self.start(workspace, tab, ui.ctx(), move |mut editor, project, cancel| {
                                next.sync_timecodes(&project, &clocks).map_err(|error| error.to_string())?;
                                apply(&mut editor, "Sync multicam timecodes", &[DocumentCommand::SetMulticamSession { session: next.clone() }], &cancel)?;
                                Ok(Completion { editor, session: next.id, focus: Some((next.composition, 0)), message: "Camera start timecodes aligned exactly".into() })
                            });
                        }
                        Err(error) => self.status = Some(error),
                    }
                }
            });
            ui.weak("Markers identify the same event in each source. Timecodes use each camera's frame rate; ;FF supports 29.97/59.94 drop-frame.");
            if !self.audio_reference.is_some_and(|id| session.angles.iter().any(|angle| angle.id == id)) { self.audio_reference = session.angles.first().map(|angle| angle.id); }
            if !self.audio_candidate.is_some_and(|id| Some(id) != self.audio_reference && session.angles.iter().any(|angle| angle.id == id)) { self.audio_candidate = session.angles.iter().find(|angle| Some(angle.id) != self.audio_reference).map(|angle| angle.id); }
            ui.horizontal_wrapped(|ui| {
                angle_choice(ui, (owner, "audio-reference"), "Reference camera", &mut self.audio_reference, session);
                angle_choice(ui, (owner, "audio-candidate"), "Match camera", &mut self.audio_candidate, session);
            });
            fields::native_scope(ui, egui::Id::new((owner, "audio-window")), |ui| {
                let (_, seconds_valid) = keyboard_ui::native_number(ui, "seconds", "Analyze first", &mut self.audio_seconds, 1..=30, None, " seconds", None, FieldState::default());
                let (_, offset_valid) = keyboard_ui::native_number(ui, "offset", "Maximum offset", &mut self.audio_offset_seconds, 1..=10, None, " seconds", None, FieldState::default());
                let recorded = [self.audio_reference, self.audio_candidate].into_iter().all(|id| id
                    .and_then(|id| session.angles.iter().find(|angle| angle.id == id))
                    .and_then(|angle| project.compositions.iter().find(|scene| scene.id == angle.composition))
                    .is_some_and(|camera| camera.audio.is_some()));
                let audio_valid = seconds_valid && offset_valid
                    && recorded && self.audio_reference != self.audio_candidate;
                if !recorded { ui.weak("Audio synchronization needs recorded sound on both selected cameras."); }
                if ui.add_enabled_ui(audio_valid, |ui| keyboard_ui::button(ui, (owner, "audio-apply"), "Sync decoded audio")).inner.clicked() && audio_valid
                    && let (Some(reference), Some(candidate)) = (self.audio_reference, self.audio_candidate)
                {
                    let next = session.clone();
                    let seconds = self.audio_seconds;
                    let max_offset = self.audio_offset_seconds;
                    self.start(workspace, tab, ui.ctx(), move |mut editor, project, cancel| {
                        let reference_composition = next.angle(reference).map_err(|error| error.to_string())?.composition;
                        let candidate_composition = next.angle(candidate).map_err(|error| error.to_string())?.composition;
                        let evaluation = Arc::new(EvaluationSnapshot::new(project.clone()).map_err(|error| error.to_string())?);
                        let directory = pcm_directory()?;
                        let (a, mut a_renderer) = decode_envelope(evaluation.clone(), reference_composition, seconds, &directory, &cancel)?;
                        let (b, mut b_renderer) = decode_envelope(evaluation, candidate_composition, seconds, &directory, &cancel)?;
                        check(&cancel)?;
                        let max_offset_samples = (max_offset * 200).min((a.len().min(b.len()) / 2).saturating_sub(1) as u32);
                        let sync = correlate_audio(&a, &b, AudioSyncOptions { sample_rate: 200, max_offset_samples, min_overlap_samples: 64, min_correlation: 0.85, min_separation: 0.02 }).map_err(|error| error.to_string())?;
                        a_renderer.verify_sources().map_err(|error| error.to_string())?;
                        b_renderer.verify_sources().map_err(|error| error.to_string())?;
                        check(&cancel)?;
                        let message = format!("Audio aligned at lag {}/200 s · correlation {:.3} · separation {:.3} · searched ±{:.2} s", sync.lag_samples(), sync.correlation(), sync.separation(), f64::from(max_offset_samples) / 200.0);
                        let mut next = next;
                        next.sync_audio_offset(&project, reference, candidate, sync).map_err(|error| error.to_string())?;
                        apply(&mut editor, "Sync multicam decoded audio", &[DocumentCommand::SetMulticamSession { session: next.clone() }], &cancel)?;
                        Ok(Completion { editor, session: next.id, focus: Some((next.composition, 0)), message })
                    });
                }
            });
            ui.weak("Audio is decoded in the background into a 200 Hz loudness envelope. Uncertain matches are rejected. No output device is opened.");
        });
    }

    /// Submit a single retained session update through the shared document command.
    /// The supplied owner, session and label produce one undoable revision.
    fn set_session(
        &mut self,
        workspace: &Workspace,
        tab: Uuid,
        ctx: &egui::Context,
        session: MulticamSession,
        label: &'static str,
    ) {
        self.start(workspace, tab, ctx, move |mut editor, _, cancel| {
            apply(
                &mut editor,
                label,
                &[DocumentCommand::SetMulticamSession {
                    session: session.clone(),
                }],
                &cancel,
            )?;
            Ok(Completion {
                editor,
                session: session.id,
                focus: None,
                message: label.into(),
            })
        });
    }

    /// Open a real camera or program composition in the native viewer.
    /// The supplied identity and frame reuse a sequence or create one through shared commands.
    fn view(
        &mut self,
        workspace: &Workspace,
        preview: &mut PreviewPane,
        tab: Uuid,
        project: &Arc<Project>,
        target: (Uuid, Uuid, u64),
        ctx: &egui::Context,
    ) {
        let (session, composition, frame) = target;
        if project
            .sequences
            .iter()
            .any(|sequence| sequence.composition == Some(composition))
        {
            if let Err(error) = preview.select(tab, project, composition, frame) {
                self.status = Some(error);
            }
            return;
        }
        self.start(workspace, tab, ctx, move |mut editor, project, cancel| {
            let scene = project
                .compositions
                .iter()
                .find(|scene| scene.id == composition)
                .ok_or("Camera composition is absent")?;
            let sequence = Sequence {
                id: Uuid::new_v4(),
                name: scene.name.clone(),
                width: scene.width,
                height: scene.height,
                frame_rate: scene.frame_rate,
                composition: Some(composition),
            };
            apply(
                &mut editor,
                "Open multicam source sequence",
                &[DocumentCommand::SetSequence { sequence }],
                &cancel,
            )?;
            Ok(Completion {
                editor,
                session,
                focus: Some((composition, frame)),
                message: "Opened the source in the native viewer".into(),
            })
        });
    }

    /// Start at most one revision-bound, cancellable multicam worker.
    /// `workspace`, `tab`, `ctx` and `work` capture immutable ownership and wake completion.
    fn start(
        &mut self,
        workspace: &Workspace,
        tab: Uuid,
        ctx: &egui::Context,
        work: impl FnOnce(DocumentEditor, Arc<Project>, Cancellation) -> Result<Completion, String>
        + Send
        + 'static,
    ) {
        if self.task.is_some() {
            return;
        }
        let (owner, editor) = match workspace.edit_snapshot(tab) {
            Ok(value) => value,
            Err(error) => {
                self.status = Some(error);
                return;
            }
        };
        let cancel = match Cancellation::new() {
            Ok(cancel) => cancel,
            Err(error) => {
                self.status = Some(error.to_string());
                return;
            }
        };
        let token = cancel.clone();
        let activation = workspace.activation_generation;
        let project = editor.snapshot();
        let (sender, result) = mpsc::sync_channel(1);
        let wake = ctx.clone();
        match std::thread::Builder::new()
            .name("editbay-multicam-command".into())
            .spawn(move || {
                let result = work(editor, project, token);
                let _ = sender.send(result);
                wake.request_repaint();
            }) {
            Ok(_) => {
                self.task = Some(Task {
                    owner,
                    activation,
                    cancel,
                    result,
                    discarded: false,
                });
                self.status = None;
            }
            Err(error) => self.status = Some(error.to_string()),
        }
    }
}

/// Apply one complete worker edit after checking cancellation on both sides.
/// `editor`, `label`, `commands` and `cancel` produce a validated undoable revision.
fn apply(
    editor: &mut DocumentEditor,
    label: &str,
    commands: &[DocumentCommand],
    cancel: &Cancellation,
) -> Result<(), String> {
    check(cancel)?;
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            label.into(),
            commands,
        )
        .map_err(|error| error.to_string())?;
    check(cancel)
}

/// Check worker cancellation without opening a native output device.
/// `cancel` identifies the owned job; returns a visible cancellation result.
fn check(cancel: &Cancellation) -> Result<(), String> {
    if cancel.is_cancelled() {
        Err("Multicam work was cancelled".into())
    } else {
        Ok(())
    }
}

/// Draw an angle selector backed only by retained camera identities.
/// The supplied UI, label and session update `selected` without touching the document.
fn angle_choice(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    selected: &mut Option<Uuid>,
    session: &MulticamSession,
) {
    keyboard_ui::choice(
        ui,
        id,
        label,
        selected,
        &session
            .angles
            .iter()
            .map(|angle| (Some(angle.id), angle.name.clone()))
            .collect::<Vec<_>>(),
    );
}

/// Read an exact editable marker coordinate.
/// `text` is an integer or numerator/denominator; returns reduced source frames.
fn parse_position(text: &str) -> Result<SourcePosition, String> {
    let text = text.trim();
    let (numerator, denominator) = text.split_once('/').unwrap_or((text, "1"));
    let numerator = numerator
        .trim()
        .parse::<i64>()
        .map_err(|_| "Marker frame must be an integer or exact fraction".to_owned())?;
    let denominator = denominator
        .trim()
        .parse::<u64>()
        .map_err(|_| "Marker denominator must be a positive integer".to_owned())?;
    SourcePosition::new(numerator, denominator).map_err(|error| error.to_string())
}

/// Resolve each camera's true local source time at the program playhead.
/// The supplied document, session, camera, common origin and frame preserve rational rates.
/// Returns exact source frames without pretending the native viewer supports subframe images.
fn angle_position(
    project: &Project,
    session: &MulticamSession,
    angle: &MulticamAngle,
    origin: i64,
    frame: u64,
) -> Result<SourcePosition, String> {
    let camera = project
        .compositions
        .iter()
        .find(|scene| scene.id == angle.composition)
        .ok_or("Camera composition is absent")?;
    if angle.offset.denominator == 0 {
        return Err("Camera offset denominator is zero".into());
    }
    let global = i128::from(origin) + i128::from(frame);
    let numerator = global
        .checked_mul(i128::from(angle.offset.denominator))
        .and_then(|value| value.checked_sub(i128::from(angle.offset.numerator)))
        .ok_or("Camera position exceeds integer time")?;
    let mut a = numerator.unsigned_abs();
    let mut b = u128::from(angle.offset.denominator);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let factor = i128::try_from(a).map_err(|_| "Camera position exceeds integer time")?;
    let local = SourcePosition::new(
        i64::try_from(numerator / factor).map_err(|_| "Camera position exceeds integer time")?,
        angle.offset.denominator / a as u64,
    )
    .map_err(|error| error.to_string())?;
    TimeBase {
        numerator: session.frame_rate.denominator,
        denominator: session.frame_rate.numerator,
    }
    .at_rate(local, camera.frame_rate)
    .map_err(|error| error.to_string())
}

/// Choose explicit user-owned storage for anonymous native PCM files.
/// Takes no arguments; returns an application cache directory beneath the user's cache root.
fn pcm_directory() -> Result<PathBuf, String> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .ok_or("A home cache directory is required for multicam audio")?;
    if !cache.is_absolute() {
        return Err("Multicam cache directory must be absolute".into());
    }
    let directory = cache.join("editbay/multicam");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    Ok(directory)
}

/// Decode real native sound into a bounded channel-independent loudness envelope.
/// `evaluation`, `composition`, `seconds`, `directory` and `cancel` bind sources, storage and lifetime.
/// Returns 200 Hz absolute-mean bins and a retained renderer for final source verification.
fn decode_envelope(
    evaluation: Arc<EvaluationSnapshot>,
    composition: Uuid,
    seconds: u32,
    directory: &std::path::Path,
    cancel: &Cancellation,
) -> Result<(Vec<f32>, SoundRenderer), String> {
    check(cancel)?;
    let scene = evaluation
        .project()
        .compositions
        .iter()
        .find(|scene| scene.id == composition)
        .ok_or("Camera composition is absent")?;
    if scene.audio.is_none() {
        return Err("Audio synchronization needs recorded sound on both cameras".into());
    }
    let snapshot = Arc::new(
        SoundSnapshot::at_output_rate(
            evaluation.clone(),
            composition,
            48_000,
            SoundBudget::default(),
        )
        .map_err(|error| error.to_string())?,
    );
    let frames = snapshot
        .duration_samples()
        .min(u64::from(seconds.clamp(1, 30)) * 48_000)
        / 240
        * 240;
    if frames < 240 * 64 {
        return Err("Audio sync needs at least 0.32 seconds of sound on each camera".into());
    }
    let pcm = NativePcmCache::new_in(
        evaluation,
        PcmBudget {
            store_bytes: 256 * 1024 * 1024,
            ..PcmBudget::default()
        },
        cancel.clone(),
        directory,
    )
    .map_err(|error| error.to_string())?;
    let mut renderer =
        SoundRenderer::with_provider(snapshot.clone(), pcm, SoundRenderBudget::default())
            .map_err(|error| error.to_string())?;
    let channels = snapshot.profile().channels.len();
    let mut envelope = Vec::with_capacity((frames / 240) as usize);
    for first in (0..frames).step_by(1920) {
        check(cancel)?;
        let count = (frames - first).min(1920) as u32;
        let plan = snapshot
            .prepare(first, count)
            .map_err(|error| error.to_string())?;
        let rendered = renderer.render(&plan).map_err(|error| error.to_string())?;
        renderer
            .validate_result(&rendered)
            .map_err(|error| error.to_string())?;
        append_envelope(&mut envelope, rendered.sound().samples(), channels)?;
    }
    renderer
        .verify_sources()
        .map_err(|error| error.to_string())?;
    Ok((envelope, renderer))
}

/// Reduce complete 240-frame blocks without cancelling opposite channel polarities.
/// `output`, interleaved `samples` and `channels` append exact 200 Hz absolute-mean bins.
fn append_envelope(output: &mut Vec<f32>, samples: &[f32], channels: usize) -> Result<(), String> {
    if channels == 0 || !samples.len().is_multiple_of(channels * 240) {
        return Err("Audio envelope block is not channel and bin aligned".into());
    }
    for bin in samples.chunks_exact(channels * 240) {
        let mean = bin.iter().try_fold(0.0_f64, |sum, value| {
            if value.is_finite() {
                Ok(sum + f64::from(value.abs()))
            } else {
                Err("Decoded camera sound contains a nonfinite sample".to_owned())
            }
        })? / bin.len() as f64;
        output.push(mean as f32);
    }
    Ok(())
}

#[cfg(test)]
mod keyboard_tests {
    use super::*;
    use crate::workflow_ui::keyboard_tests::{Author, Page, directory, key};
    use std::time::Instant;

    #[test]
    fn craft_keyboard_common_multicam_busy_and_changed_owner_reject_real_creation() {
        let directory = directory();
        let mut author = Author::new(directory.path(), Page::Multicam);
        author.camera("Camera A");
        author.camera("Camera B");
        author.frame(vec![]);
        author.open("Create from camera sequences");
        author.tab_prefix("Camera source: Camera A");
        author.press(egui::Key::Space);
        author.tab_prefix("Camera source: Camera B");
        author.press(egui::Key::Space);
        author.tab_to("Create multicam edit");
        author.frame(vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
        let pending = author
            .multicam
            .task
            .as_ref()
            .expect("native action dispatched the production worker")
            .owner;
        author.poll_workers = false;
        author.press(egui::Key::Enter);
        assert!(author.disabled("Create multicam edit"));
        assert_eq!(author.multicam.task.as_ref().unwrap().owner, pending);
        assert_eq!(author.project().revision, 2);
        assert!(author.project().multicam.is_empty());
        author
            .workspace
            .apply(
                author.tab,
                DocumentVersion::of(author.project()),
                "External document edit".into(),
                &[DocumentCommand::RenameProject {
                    name: "Newer owner revision".into(),
                }],
            )
            .unwrap();
        author.poll_workers = true;
        let deadline = Instant::now() + Duration::from_secs(5);
        while author.multicam.task.is_some() {
            author.frame(vec![]);
            assert!(
                Instant::now() < deadline,
                "production worker did not retire"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            author
                .multicam
                .status
                .as_ref()
                .unwrap()
                .contains("discarded")
        );
        assert!(author.project().multicam.is_empty());
        assert_eq!(author.project().name, "Newer owner revision");
        assert_eq!(author.project().revision, 3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evaluate the public analytic audio fixture without decoding an output.
    /// `bin` is a 200 Hz event coordinate; returns an exactly representable loudness.
    fn fixture_loudness(bin: u32) -> f32 {
        let mut value = bin.wrapping_add(1).wrapping_mul(0x9e37_79b9);
        value ^= value >> 16;
        value = value.wrapping_mul(0x85eb_ca6b);
        value ^= value >> 13;
        ((value % 127) + 1) as f32 / 256.0
    }

    /// Retain one tiny real camera through the production media/authoring route.
    /// `editor`, `path`, `event_offset` and `cancel` select new bytes; returns its composition.
    fn fixture_camera(
        editor: &mut DocumentEditor,
        path: &std::path::Path,
        event_offset: u32,
        cancel: &Cancellation,
    ) -> Result<Uuid, Box<dyn std::error::Error>> {
        use editbay_core::{FrameRate, sequence_from_video_with_audio};
        use editbay_media::{LosslessMovProfile, LosslessMovWriter, SourceFile, StreamType};
        let rate = FrameRate::new(24, 1)?;
        let profile = LosslessMovProfile {
            width: 8,
            height: 8,
            frame_rate: rate,
            first_frame: 0,
            frames: 32,
            sample_rate: 48_000,
            channels: vec!["FC".into()],
        };
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)?;
        let mut writer = LosslessMovWriter::new(&file, profile.clone(), cancel.clone())?;
        let pixels = [48, 96, 144, 255].repeat(64);
        let mut first = 0;
        for frame in 0..profile.frames {
            writer.picture(frame, &pixels)?;
            let end = profile.samples_through(frame + 1)?;
            let samples: Vec<_> = (first..end)
                .map(|sample| {
                    let amplitude = fixture_loudness((sample / 240) as u32 + event_offset);
                    if sample % 2 == 0 {
                        amplitude
                    } else {
                        -amplitude
                    }
                })
                .collect();
            writer.sound(first, &samples)?;
            first = end;
        }
        writer.finish()?;
        file.sync_all()?;
        let source = SourceFile::open(path, cancel)?;
        let probe = source.probe(cancel.clone())?;
        let video = probe
            .streams
            .iter()
            .find(|stream| stream.kind == StreamType::Video)
            .ok_or("camera picture missing")?
            .index;
        let audio = probe
            .streams
            .iter()
            .find(|stream| stream.kind == StreamType::Audio)
            .ok_or("camera sound missing")?
            .index;
        let ingested = source.ingest(
            "Analytic camera".into(),
            &[video, audio],
            cancel.clone(),
            |_, _| {},
        )?;
        apply(editor, "Retain camera", &ingested.commands(), cancel)?;
        let commands = sequence_from_video_with_audio(
            editor.project(),
            ingested.source.id,
            video,
            audio,
            rate,
        )?;
        let composition = commands
            .iter()
            .find_map(|command| match command {
                DocumentCommand::SetComposition { composition }
                    if composition.picture.is_some() =>
                {
                    Some(composition.id)
                }
                _ => None,
            })
            .ok_or("camera composition missing")?;
        apply(editor, "Author linked camera", &commands, cancel)?;
        source.verify(cancel)?;
        Ok(composition)
    }

    #[test]
    fn craft_video_real_camera_audio_decodes_syncs_and_reopens_without_a_device()
    -> Result<(), Box<dyn std::error::Error>> {
        let requested = std::env::var_os("EDITBAY_WORKFLOW_RECEIPTS_DIR").map(PathBuf::from);
        let root = requested
            .clone()
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts"));
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        assert!(root.starts_with("/home"));
        let mut temporary = Some(
            tempfile::Builder::new()
                .prefix("multicam-audio-")
                .tempdir_in(&root)?,
        );
        let directory = if requested.is_some() {
            temporary.take().unwrap().keep()
        } else {
            temporary.as_ref().unwrap().path().to_path_buf()
        };
        let cancel = Cancellation::new()?;
        let mut editor = DocumentEditor::new(Project::new("Actual camera audio sync")?)?;
        let a = fixture_camera(&mut editor, &directory.join("camera-a.mov"), 0, &cancel)?;
        let b = fixture_camera(&mut editor, &directory.join("camera-b.mov"), 25, &cancel)?;
        let snapshot = Arc::new(EvaluationSnapshot::new(editor.snapshot())?);
        let cache = directory.join("pcm");
        std::fs::create_dir(&cache)?;
        let (reference, mut a_renderer) = decode_envelope(snapshot.clone(), a, 2, &cache, &cancel)?;
        let (candidate, mut b_renderer) = decode_envelope(snapshot, b, 2, &cache, &cancel)?;
        assert_eq!(reference.len(), 266);
        assert_eq!(candidate.len(), 266);
        for (offset, envelope) in [(0, &reference), (25, &candidate)] {
            for (bin, actual) in envelope.iter().enumerate() {
                assert!(
                    (actual - fixture_loudness(bin as u32 + offset)).abs() <= 1.0e-6,
                    "camera offset {offset}, bin {bin}: {actual}"
                );
            }
        }
        let options = AudioSyncOptions {
            sample_rate: 200,
            max_offset_samples: 40,
            min_overlap_samples: 64,
            min_correlation: 0.85,
            min_separation: 0.02,
        };
        let sync = correlate_audio(&reference, &candidate, options)?;
        assert_eq!(sync.lag_samples(), -25);
        assert!(sync.correlation() > 0.999_999);
        let mut session = MulticamSession::new(editor.project(), "Audio aligned", &[a, b])?;
        let reference_angle = session.angles[0].id;
        let candidate_angle = session.angles[1].id;
        session.sync_audio_offset(editor.project(), reference_angle, candidate_angle, sync)?;
        assert_eq!(session.angles[1].offset, SourcePosition::new(3, 1)?);
        apply(
            &mut editor,
            "Sync decoded camera audio",
            &[DocumentCommand::SetMulticamSession {
                session: session.clone(),
            }],
            &cancel,
        )?;
        editor.undo(DocumentVersion::of(editor.project()))?;
        assert!(editor.project().multicam.is_empty());
        editor.redo(DocumentVersion::of(editor.project()))?;
        let project_file = directory.join("synced.editbay");
        editbay_core::save_new(editor.project(), &project_file)?;
        assert_eq!(editbay_core::load(&project_file)?, *editor.project());
        a_renderer.verify_sources()?;
        b_renderer.verify_sources()?;
        cancel.cancel();
        assert!(
            decode_envelope(
                Arc::new(EvaluationSnapshot::new(editor.snapshot())?),
                a,
                2,
                &cache,
                &cancel
            )
            .is_err()
        );
        let receipt = serde_json::json!({
            "schema":"editbay.multicam.audio-sync.v1", "status":"passed", "license":"MIT analytic fixture",
            "inputs":editor.project().sources, "reference_envelope":reference, "candidate_envelope":candidate,
            "envelope_rate":200, "expected_lag_bins":-25, "actual_lag_bins":sync.lag_samples(),
            "correlation":sync.correlation(), "separation":sync.separation(), "camera_offset_frames":session.angles[1].offset,
            "save_reopen":true, "undo_redo":true, "cancel_rejected":true,
            "proof_boundary":"Software native encode/decode and independent analytic loudness/offset oracle; no sound device or physical timing"
        });
        std::fs::write(
            directory.join("audio-sync.json"),
            serde_json::to_vec_pretty(&receipt)?,
        )?;
        Ok(())
    }

    #[test]
    fn craft_video_markers_keep_fractions_and_reject_invalid_scales() {
        assert_eq!(
            parse_position(" 15/10 ").unwrap(),
            SourcePosition::new(3, 2).unwrap()
        );
        assert_eq!(
            parse_position("-24").unwrap(),
            SourcePosition::new(-24, 1).unwrap()
        );
        assert!(parse_position("1/0").is_err());
        assert!(parse_position("1.5").is_err());
    }

    #[test]
    fn craft_video_native_envelopes_preserve_loudness_across_opposite_channels() {
        let mut samples = Vec::new();
        for _ in 0..240 {
            samples.extend([0.75, -0.75]);
        }
        for _ in 0..240 {
            samples.extend([0.25, -0.25]);
        }
        let mut envelope = Vec::new();
        append_envelope(&mut envelope, &samples, 2).unwrap();
        assert_eq!(envelope, [0.75, 0.25]);
        assert!(append_envelope(&mut envelope, &[0.0; 239], 1).is_err());
        assert!(append_envelope(&mut envelope, &[f32::NAN; 240], 1).is_err());
    }

    #[test]
    fn craft_video_multicam_completed_noop_preserves_revision_and_reports_success() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
        std::fs::create_dir_all(&root).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("multicam-noop-cpu-")
            .tempdir_in(root)
            .unwrap();
        let mut workspace =
            Workspace::new(directory.path().join("recovery"), egui::Context::default());
        let tab = workspace.create(Project::new("Original").unwrap()).unwrap();
        let (owner, editor) = workspace.edit_snapshot(tab).unwrap();
        let session = Uuid::new_v4();
        let (sender, result) = mpsc::sync_channel(1);
        let mut pane = MulticamPane {
            task: Some(Task {
                owner,
                activation: workspace.activation_generation,
                cancel: Cancellation::new().unwrap(),
                result,
                discarded: false,
            }),
            ..Default::default()
        };
        assert!(
            sender
                .send(Ok(Completion {
                    editor,
                    session,
                    focus: None,
                    message: "The selected camera is already active".into(),
                }))
                .is_ok()
        );
        pane.poll(&mut workspace, true);
        assert!(pane.task.is_none());
        assert_eq!(pane.selected, Some(session));
        assert_eq!(
            pane.status.as_deref(),
            Some("The selected camera is already active")
        );
        assert!(workspace.owns(owner));
        assert_eq!(
            DocumentVersion::of(workspace.tabs[0].editor.project()),
            owner.version
        );
    }

    #[test]
    fn craft_video_multicam_results_reject_changed_switched_hidden_cancelled_and_latched_owners() {
        for change in 0..6 {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
            std::fs::create_dir_all(&root).unwrap();
            let directory = tempfile::Builder::new()
                .prefix("multicam-owner-cpu-")
                .tempdir_in(root)
                .unwrap();
            let mut workspace =
                Workspace::new(directory.path().join("recovery"), egui::Context::default());
            let tab = workspace.create(Project::new("Original").unwrap()).unwrap();
            let (owner, mut editor) = workspace.edit_snapshot(tab).unwrap();
            editor
                .apply(
                    owner.version,
                    "Worker edit".into(),
                    &[DocumentCommand::RenameProject {
                        name: "Worker result".into(),
                    }],
                )
                .unwrap();
            let (sender, result) = mpsc::sync_channel(1);
            let mut pane = MulticamPane {
                task: Some(Task {
                    owner,
                    activation: workspace.activation_generation,
                    cancel: Cancellation::new().unwrap(),
                    result,
                    discarded: false,
                }),
                ..Default::default()
            };
            match change {
                1 => {
                    workspace
                        .apply(
                            tab,
                            owner.version,
                            "New edit".into(),
                            &[DocumentCommand::RenameProject {
                                name: "Newer".into(),
                            }],
                        )
                        .unwrap();
                }
                2 => {
                    workspace
                        .create(Project::new("Other tab").unwrap())
                        .unwrap();
                }
                4 => pane.task.as_ref().unwrap().cancel.cancel(),
                5 => {
                    pane.poll(&mut workspace, false);
                    assert!(pane.task.as_ref().unwrap().discarded);
                }
                _ => {}
            }
            assert!(
                sender
                    .send(Ok(Completion {
                        editor,
                        session: Uuid::new_v4(),
                        focus: None,
                        message: "Accepted worker edit".into()
                    }))
                    .is_ok()
            );
            pane.poll(&mut workspace, change != 3);
            assert!(pane.task.is_none());
            let original = workspace
                .tabs
                .iter()
                .find(|candidate| candidate.id == tab)
                .unwrap()
                .editor
                .project();
            assert_eq!(
                original.name,
                match change {
                    0 => "Worker result",
                    1 => "Newer",
                    _ => "Original",
                }
            );
            if change == 0 {
                assert_eq!(pane.status.as_deref(), Some("Accepted worker edit"));
            } else {
                assert!(pane.status.as_ref().unwrap().contains("discarded"));
            }
        }
    }
}
