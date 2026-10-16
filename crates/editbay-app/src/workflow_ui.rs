use crate::{
    fields::{self, FieldState},
    keyboard_ui,
    preview::PreviewPane,
    workspace::{DocumentOwner, Workspace},
};
use editbay_core::{DocumentCommand, DocumentEditor, DocumentVersion, Project, captions::*};
use eframe::egui::{self, Ui};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
};
use uuid::Uuid;

enum Completion {
    Imported {
        project: Box<Project>,
        report_path: PathBuf,
    },
    Edited(DocumentEditor),
    Exported(String),
}
struct Task {
    owner: DocumentOwner,
    activation: u64,
    cancel: editbay_media::Cancellation,
    result: Receiver<Result<Completion, String>>,
}

impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct CaptionTextDraft {
    baseline: String,
    text: String,
    error: Option<String>,
}

impl CaptionTextDraft {
    /// Retain editable text independently of the validated source cue.
    /// `text` supplies the source baseline; returns an initially clean draft.
    fn new(text: &str) -> Self {
        Self {
            baseline: text.into(),
            text: text.into(),
            error: None,
        }
    }

    /// Keep typing through unrelated edits and replace externally changed text.
    /// `text` is the current validated source; returns without committing a draft.
    fn rebase(&mut self, text: &str) {
        if self.baseline != text {
            *self = Self::new(text);
        }
    }

    /// Validate one complete proposed cue without changing its source or baseline.
    /// `cue` supplies retained identity/timing; returns changed valid text or an inline error.
    fn validated_text(&mut self, cue: &CaptionCue) -> Option<String> {
        let mut candidate = cue.clone();
        candidate.text = self.text.clone();
        match candidate.validate() {
            Ok(()) => {
                self.error = None;
                (candidate.text != cue.text).then_some(candidate.text)
            }
            Err(error) => {
                self.error = Some(error.to_string());
                None
            }
        }
    }
}

#[derive(Default)]
struct CaptionDrafts {
    owner: Option<(Uuid, Uuid, Uuid)>,
    cues: HashMap<Uuid, CaptionTextDraft>,
}

impl CaptionDrafts {
    /// Drop drafts when the visible document owner changes, without using revision.
    /// `tab` and `project` identify the document; returns no value and changes no source.
    fn observe_document(&mut self, tab: Uuid, project: Uuid) {
        if self.owner.is_some_and(|(owner_tab, owner_project, _)| {
            owner_tab != tab || owner_project != project
        }) {
            *self = Self::default();
        }
    }

    /// Rebase one selected track while preserving unfinished text through other edits.
    /// `tab`, `project` and `track` identify current source cues; returns synchronized drafts.
    fn sync(&mut self, tab: Uuid, project: Uuid, track: &CaptionTrack) {
        let owner = Some((tab, project, track.id));
        if self.owner != owner {
            self.cues.clear();
            self.owner = owner;
        }
        let ids: HashSet<_> = track.cues.iter().map(|cue| cue.id).collect();
        self.cues.retain(|id, _| ids.contains(id));
        for cue in &track.cues {
            self.cues
                .entry(cue.id)
                .or_insert_with(|| CaptionTextDraft::new(&cue.text))
                .rebase(&cue.text);
        }
    }
}

pub struct WorkflowPane {
    path: String,
    loss_root: PathBuf,
    format: editbay_interchange::Format,
    edl_rate: [u32; 2],
    relink_asset: Option<Uuid>,
    font: String,
    selected: Option<Uuid>,
    drafts: CaptionDrafts,
    start: u64,
    end: u64,
    split_offset: usize,
    status: Option<String>,
    task: Option<Task>,
}
impl Default for WorkflowPane {
    fn default() -> Self {
        Self {
            path: String::new(),
            loss_root: PathBuf::new(),
            format: editbay_interchange::Format::Otio,
            edl_rate: [24, 1],
            relink_asset: None,
            font: String::new(),
            selected: None,
            drafts: CaptionDrafts::default(),
            start: 0,
            end: 1000,
            split_offset: 1,
            status: None,
            task: None,
        }
    }
}
impl WorkflowPane {
    /// Retain import loss reports in the existing native application state.
    /// `loss_root` selects durable report storage; returns an idle authoring pane.
    pub fn new(loss_root: PathBuf) -> Self {
        Self {
            loss_root,
            ..Default::default()
        }
    }
    /// Accept only worker results that still own the visible document session.
    /// `workspace` and `visible` supply current ownership; returns without waiting.
    pub fn poll(&mut self, workspace: &mut Workspace, visible: bool) {
        let Some(task) = &self.task else { return };
        let stale = task.cancel.is_cancelled()
            || !visible
            || workspace.active != Some(task.owner.tab)
            || workspace.activation_generation != task.activation
            || !workspace.owns(task.owner);
        if stale {
            task.cancel.cancel();
        }
        let result = match task.result.try_recv() {
            Ok(value) => value,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Workflow worker stopped before returning a result".into())
            }
        };
        let owner = task.owner;
        self.task = None;
        if stale {
            self.status = Some("The project changed; the background result was discarded".into());
            return;
        }
        let accepted = result.and_then(|result| match result {
            Completion::Imported {
                project,
                report_path,
            } => {
                self.status = Some(format!(
                    "Imported timeline; loss report: {}",
                    report_path.display()
                ));
                workspace.create(*project).map(|_| ())
            }
            Completion::Edited(editor) => {
                if DocumentVersion::of(editor.project()) == owner.version {
                    Ok(())
                } else {
                    workspace.commit_edit(owner, editor)
                }
            }
            Completion::Exported(message) => {
                self.status = Some(message);
                Ok(())
            }
        });
        if let Err(error) = accepted {
            self.status = Some(error);
        }
    }

    /// Draw caption authoring and editorial exchange against the selected program.
    /// `ui`, `workspace`, `preview` and `tab` identify the native document session.
    /// All accepted edits use shared commands; file work runs on a bounded worker.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        workspace: &mut Workspace,
        preview: &mut PreviewPane,
        tab: Uuid,
        project: Arc<Project>,
    ) {
        self.drafts.observe_document(tab, project.id);
        let composition = preview
            .selected_composition(tab, &project)
            .unwrap_or(Uuid::nil());
        let scene = project.compositions.iter().find(|c| c.id == composition);
        let owner = ("workflow", tab, project.id);
        ui.collapsing("Editorial exchange", |ui| {
            keyboard_ui::text(ui, (owner, "exchange-path"), "Source or new destination path", &mut self.path, false);
            keyboard_ui::choice(ui, (owner, "exchange-format"), "Editorial format", &mut self.format,
                &editbay_interchange::Format::ALL.into_iter().map(|format| (format, format.name().into())).collect::<Vec<_>>());
            let mut rate_valid = true;
            if self.format == editbay_interchange::Format::Edl {
                rate_valid = fields::native_scope(ui, egui::Id::new((owner, "edl-rate")), |ui| {
                    let (_, numerator_valid) = keyboard_ui::native_number(ui, "numerator", "EDL rate numerator", &mut self.edl_rate[0], 1..=u32::MAX, None, "", None, FieldState::default());
                    let (_, denominator_valid) = keyboard_ui::native_number(ui, "denominator", "EDL rate denominator", &mut self.edl_rate[1], 1..=u32::MAX, None, "", None, FieldState::default());
                    numerator_valid && denominator_valid
                }).inner;
                ui.weak("EDL files do not store their frame rate. Set the exact source rate before importing.");
            }
            ui.add_enabled_ui(self.task.is_none(), |ui| {
                ui.horizontal(|ui| {
                    if ui.add_enabled_ui(rate_valid && !self.path.trim().is_empty(), |ui| keyboard_ui::button(ui, (owner, "import"), "Import timeline")).inner.clicked() {
                        let path = PathBuf::from(&self.path);
                        let format = self.format;
                        let edl_rate = self.edl_rate;
                        let loss_root = self.loss_root.clone();
                        self.start(workspace, tab, ui.ctx(), move |_, _, cancel| {
                            let bytes = read(&path, 32 * 1024 * 1024)?;
                            let options = editbay_interchange::ImportOptions {
                                base_dir: path.parent().map(|p| p.to_string_lossy().into()),
                                edl_frame_rate: (format == editbay_interchange::Format::Edl)
                                    .then(|| editbay_core::FrameRate::new(edl_rate[0], edl_rate[1]))
                                    .transpose().map_err(|error| error.to_string())?,
                                ..Default::default()
                            };
                            let (project, report) = editbay_interchange::import_document(
                                &bytes, format, &options, &cancel,
                            )
                            .map_err(|e| e.to_string())?;
                            std::fs::create_dir_all(&loss_root).map_err(|e|e.to_string())?;
                            let report_path = loss_root.join(format!("{}.json",project.id));
                            write_new(
                                &report_path,
                                &serde_json::to_vec_pretty(&serde_json::json!({"version":DocumentVersion::of(&project),"source":path,"format":format.name(),"report":report})).map_err(|e|e.to_string())?,
                            )?;
                            Ok(Completion::Imported{project:Box::new(project),report_path})
                        });
                    }
                    if ui.add_enabled_ui(scene.is_some() && !self.path.trim().is_empty(), |ui| keyboard_ui::button(ui, (owner, "export"), "Export timeline")).inner.clicked() {
                        let path = PathBuf::from(&self.path);
                        let format = self.format;
                        self.start(workspace, tab, ui.ctx(), move |_, project, _cancel| {
                            let (bytes, report) = editbay_interchange::export_document(
                                &project,
                                composition,
                                format,
                                &Default::default(),
                            )
                            .map_err(|e| e.to_string())?;
                            let report_path =
                                path.with_extension(format!("{}.loss.json", format.extension()));
                            if path.exists() || report_path.exists() {
                                return Err("Destination or loss report exists".into());
                            }
                            write_new(
                                &report_path,
                                &serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
                            )?;
                            write_new(&path, &bytes)?;
                            Ok(Completion::Exported(format!(
                                "Saved {} and {}",
                                path.display(),
                                report_path.display()
                            )))
                        });
                    }
                });
                let assets: Vec<_> = project.assets.iter().filter(|asset| asset.kind == editbay_core::AssetKind::Media).collect();
                if !assets.iter().any(|asset| Some(asset.id) == self.relink_asset) {
                    self.relink_asset = assets.first().map(|asset| asset.id);
                }
                ui.add_enabled_ui(!assets.is_empty(), |ui| keyboard_ui::choice(ui, (owner, "relink-asset"), "Original media", &mut self.relink_asset,
                    &assets.iter().map(|asset| (Some(asset.id), asset.path.display().to_string())).collect::<Vec<_>>()));
                if ui.add_enabled_ui(self.relink_asset.is_some() && !self.path.trim().is_empty(), |ui| keyboard_ui::button(ui, (owner, "relink"), "Relink moved original to path")).inner.clicked()
                    && let Some(asset) = self.relink_asset
                {
                    let path = PathBuf::from(&self.path);
                    self.start(workspace, tab, ui.ctx(), move |mut editor, project, cancel| {
                        let command = editbay_interchange::relink(&project, asset, &path, &cancel).map_err(|error| error.to_string())?;
                        editor.apply(DocumentVersion::of(editor.project()), "Relink moved original".into(), &[command]).map_err(|error| error.to_string())?;
                        Ok(Completion::Edited(editor))
                    });
                }
            });
            ui.weak(
                "Imports open an editable project. Exports retain format losses beside the file.",
            );
        });
        let Some(scene) = scene else {
            return;
        };
        ui.collapsing("Captions", |ui| {
            let path_response = keyboard_ui::text(
                ui,
                (owner, composition, "caption-path"),
                "SRT/VTT source or new sidecar path",
                &mut self.path,
                false,
            );
            let path_valid = caption_vtt(std::path::Path::new(&self.path)).is_ok();
            if !self.path.is_empty() && !path_valid {
                keyboard_ui::error(
                    ui,
                    &[path_response],
                    "Caption path must end in .srt or .vtt",
                );
            }
            if ui
                .add_enabled_ui(self.task.is_none() && path_valid, |ui| {
                    keyboard_ui::button(
                        ui,
                        (owner, composition, "caption-import"),
                        "Import captions",
                    )
                })
                .inner
                .clicked()
            {
                let path = PathBuf::from(&self.path);
                self.start(workspace, tab, ui.ctx(), move |mut editor, _, _cancel| {
                    let vtt = caption_vtt(&path)?;
                    let bytes = read(&path, 8 * 1024 * 1024)?;
                    let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
                    let track = if vtt {
                        CaptionTrack::import_vtt(composition, "Captions", text)
                    } else {
                        CaptionTrack::import_srt(composition, "Captions", text)
                    }
                    .map_err(|e| e.to_string())?;
                    editor
                        .apply(
                            DocumentVersion::of(editor.project()),
                            "Import captions".into(),
                            &[DocumentCommand::SetCaptionTrack { track }],
                        )
                        .map_err(|e| e.to_string())?;
                    Ok(Completion::Edited(editor))
                });
            }
            let tracks: Vec<_> = project
                .captions
                .iter()
                .filter(|t| t.composition == composition)
                .collect();
            if !tracks.iter().any(|t| Some(t.id) == self.selected) {
                self.selected = tracks.first().map(|t| t.id);
            }
            ui.add_enabled_ui(!tracks.is_empty(), |ui| {
                keyboard_ui::choice(
                    ui,
                    (owner, composition, "caption-track"),
                    "Caption track",
                    &mut self.selected,
                    &tracks
                        .iter()
                        .map(|track| (Some(track.id), track.name.clone()))
                        .collect::<Vec<_>>(),
                )
            });
            if keyboard_ui::button(ui, (owner, composition, "caption-new"), "New caption track")
                .clicked()
            {
                match CaptionTrack::new(composition, "Captions") {
                    Ok(track) => {
                        self.selected = Some(track.id);
                        self.apply(
                            workspace,
                            tab,
                            vec![DocumentCommand::SetCaptionTrack { track }],
                            "New captions",
                        );
                    }
                    Err(e) => self.status = Some(e.to_string()),
                }
            }
            let Some(track) = tracks.into_iter().find(|t| Some(t.id) == self.selected) else {
                self.drafts = CaptionDrafts::default();
                return;
            };
            self.drafts.sync(tab, project.id, track);
            let track_owner = (owner, composition, track.id);
            let mut edited = track.clone();
            let visible = fields::native_scope(ui, egui::Id::new((track_owner, "visible")), |ui| {
                ui.checkbox(&mut edited.enabled, "Caption visible")
            })
            .inner;
            keyboard_ui::focus(ui, &visible);
            let mut changed = visible.changed();
            changed |= keyboard_ui::choice(
                ui,
                (track_owner, "delivery"),
                "Caption delivery",
                &mut edited.delivery,
                &[
                    (CaptionDelivery::None, "Preview only"),
                    (CaptionDelivery::SidecarSrt, "SRT sidecar"),
                    (CaptionDelivery::SidecarVtt, "VTT sidecar"),
                    (CaptionDelivery::BurnIn, "Burn in"),
                    (CaptionDelivery::BurnInAndSrt, "Burn in + SRT"),
                    (CaptionDelivery::BurnInAndVtt, "Burn in + VTT"),
                ]
                .map(|(choice, label)| (choice, label.into())),
            )
            .changed();
            let style_valid =
                fields::native_scope(ui, egui::Id::new((track_owner, "style")), |ui| {
                    let (size, size_valid) = keyboard_ui::native_number(
                        ui,
                        "font-size",
                        "Caption font size",
                        &mut edited.style.font_size,
                        4.0..=512.0,
                        None,
                        "",
                        None,
                        FieldState::default(),
                    );
                    changed |= size.changed;
                    let mut margin = edited.style.safe_area.left;
                    let mixed = [
                        edited.style.safe_area.right,
                        edited.style.safe_area.top,
                        edited.style.safe_area.bottom,
                    ]
                    .into_iter()
                    .any(|value| value != margin);
                    let (inset, inset_valid) = keyboard_ui::native_number(
                        ui,
                        "safe-margin",
                        "Caption safe margin",
                        &mut margin,
                        0.0..=0.4,
                        Some(0.01),
                        "",
                        None,
                        FieldState {
                            mixed,
                            ..Default::default()
                        },
                    );
                    if inset.changed {
                        edited.style.safe_area = CaptionSafeArea {
                            left: margin,
                            right: margin,
                            top: margin,
                            bottom: margin,
                        };
                        changed = true;
                    }
                    size_valid && inset_valid
                })
                .inner;
            let mut font_request = None;
            keyboard_ui::text(
                ui,
                (track_owner, "font"),
                "Caption font file",
                &mut self.font,
                false,
            )
            .on_hover_text("Retained TrueType/OpenType font path for burn-in");
            if ui
                .add_enabled_ui(
                    self.task.is_none() && style_valid && !self.font.trim().is_empty(),
                    |ui| keyboard_ui::button(ui, (track_owner, "use-font"), "Use font for burn-in"),
                )
                .inner
                .clicked()
            {
                font_request = Some(PathBuf::from(&self.font));
            }
            let range_valid =
                fields::native_scope(ui, egui::Id::new((track_owner, "cue-range")), |ui| {
                    let (start, start_valid) = keyboard_ui::native_number(
                        ui,
                        "start",
                        "Cue start (ms)",
                        &mut self.start,
                        0..=u64::MAX,
                        None,
                        "",
                        None,
                        FieldState::default(),
                    );
                    let (end, end_valid) = keyboard_ui::native_number(
                        ui,
                        "end",
                        "Cue end (ms)",
                        &mut self.end,
                        0..=u64::MAX,
                        None,
                        "",
                        None,
                        FieldState::default(),
                    );
                    let valid = start_valid && end_valid;
                    if self.start >= self.end {
                        keyboard_ui::error(
                            ui,
                            &[start.response, end.response],
                            "Cue end must follow its start",
                        );
                    }
                    valid && self.start < self.end
                })
                .inner;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled_ui(range_valid, |ui| {
                        keyboard_ui::button(ui, (track_owner, "cue-add"), "Add cue")
                    })
                    .inner
                    .clicked()
                    && range_valid
                {
                    match CaptionCue::new(
                        CaptionTime::from_milliseconds(self.start),
                        CaptionTime::from_milliseconds(self.end),
                        "Caption",
                    )
                    .and_then(|c| edited.insert_cue(c))
                    {
                        Ok(()) => changed = true,
                        Err(e) => self.status = Some(e.to_string()),
                    }
                }
            });
            let playhead = preview.position(tab, &project).map_or(0, |(_, f)| f);
            let mut remove = None;
            let mut retime = None;
            for (index, cue) in edited.cues.iter_mut().enumerate() {
                let draft = self
                    .drafts
                    .cues
                    .entry(cue.id)
                    .or_insert_with(|| CaptionTextDraft::new(&cue.text));
                ui.push_id((tab, track.id, cue.id), |ui| {
                    ui.weak(format!(
                        "{}/{}s → {}/{}s",
                        cue.start.numerator,
                        cue.start.denominator,
                        cue.end.numerator,
                        cue.end.denominator
                    ));
                    let cue_owner = (track_owner, cue.id);
                    let response = keyboard_ui::text(
                        ui,
                        (cue_owner, "text"),
                        &format!("Cue {} text", index + 1),
                        &mut draft.text,
                        true,
                    );
                    if response.changed() {
                        draft.error = None;
                    }
                    let mut apply_text = response.lost_focus();
                    let mut candidate = cue.clone();
                    candidate.text = draft.text.clone();
                    let text_error = candidate.validate().err().map(|error| error.to_string());
                    ui.horizontal(|ui| {
                        apply_text |= ui
                            .add_enabled_ui(draft.text != cue.text && text_error.is_none(), |ui| {
                                keyboard_ui::button(ui, (cue_owner, "apply"), "Apply text")
                            })
                            .inner
                            .clicked();
                        if ui
                            .add_enabled_ui(range_valid, |ui| {
                                keyboard_ui::button(ui, (cue_owner, "retime"), "Retime to fields")
                            })
                            .inner
                            .clicked()
                            && range_valid
                        {
                            retime = Some(cue.id);
                        }
                        if keyboard_ui::button(ui, (cue_owner, "remove"), "Remove cue").clicked() {
                            remove = Some(cue.id);
                        }
                    });
                    if apply_text && let Some(text) = draft.validated_text(cue) {
                        cue.text = text;
                        changed = true;
                    }
                    if let Some(error) = text_error.as_ref().or(draft.error.as_ref()) {
                        keyboard_ui::error(ui, &[response], error);
                    }
                });
            }
            if let Some(id) = retime {
                match edited.retime_cue(
                    id,
                    CaptionTime::from_milliseconds(self.start),
                    CaptionTime::from_milliseconds(self.end),
                ) {
                    Ok(()) => changed = true,
                    Err(e) => self.status = Some(e.to_string()),
                }
            }
            if let Some(id) = remove {
                edited.cues.retain(|c| c.id != id);
                changed = true;
            }
            fields::native_scope(ui, egui::Id::new((track_owner, "split")), |ui| {
                let (_, split_valid) = keyboard_ui::native_number(
                    ui,
                    "offset",
                    "Split text after character",
                    &mut self.split_offset,
                    0..=usize::MAX,
                    None,
                    "",
                    None,
                    FieldState::default(),
                );
                if ui
                    .add_enabled_ui(split_valid, |ui| {
                        keyboard_ui::button(
                            ui,
                            (track_owner, "split-apply"),
                            "Split active cue at playhead",
                        )
                    })
                    .inner
                    .clicked()
                    && split_valid
                {
                    let time = CaptionTime::from_frame(playhead, scene.frame_rate);
                    let split = time.and_then(|time| {
                        let id = edited
                            .active_at(time)?
                            .first()
                            .ok_or_else(|| {
                                editbay_core::Error::Invalid("No caption at playhead".into())
                            })?
                            .id;
                        edited.split_cue(id, time, self.split_offset)
                    });
                    match split {
                        Ok(_) => changed = true,
                        Err(e) => self.status = Some(e.to_string()),
                    }
                }
            });
            if changed {
                self.apply(
                    workspace,
                    tab,
                    vec![DocumentCommand::SetCaptionTrack {
                        track: edited.clone(),
                    }],
                    "Edit captions",
                );
            }
            if let Some(font) = font_request {
                let mut track = edited.clone();
                self.start(workspace, tab, ui.ctx(), move |mut editor, _, cancel| {
                    let file = editbay_media::SourceFile::open(&font, &cancel)
                        .map_err(|e| e.to_string())?;
                    let asset = editbay_core::AssetReference {
                        id: Uuid::new_v4(),
                        kind: editbay_core::AssetKind::Font,
                        path: file.path().into(),
                        sha256: file.fingerprint().sha256.clone(),
                        bytes: file.fingerprint().bytes,
                        provenance: "Caption font selected by owner".into(),
                    };
                    track.style.font = Some(asset.id);
                    editor
                        .apply(
                            DocumentVersion::of(editor.project()),
                            "Caption font".into(),
                            &[
                                DocumentCommand::SetAsset { asset },
                                DocumentCommand::SetCaptionTrack { track },
                            ],
                        )
                        .map_err(|e| e.to_string())?;
                    Ok(Completion::Edited(editor))
                });
            }
            if ui
                .add_enabled_ui(self.task.is_none() && path_valid, |ui| {
                    keyboard_ui::button(
                        ui,
                        (track_owner, "sidecar-export"),
                        "Export selected sidecar",
                    )
                })
                .inner
                .clicked()
            {
                let path = PathBuf::from(&self.path);
                let track = edited.clone();
                self.start(workspace, tab, ui.ctx(), move |_, _, _cancel| {
                    let text = if caption_vtt(&path)? {
                        track.export_vtt()
                    } else {
                        track.export_srt()
                    }
                    .map_err(|e| e.to_string())?;
                    write_new(&path, text.as_bytes())?;
                    Ok(Completion::Exported(format!("Saved {}", path.display())))
                });
            }
        });
        if self.task.is_some() {
            ui.weak("Preparing workflow…");
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(30));
        }
        if let Some(message) = &self.status {
            ui.label(message);
        }
    }
    /// Apply a small authoring group to the captured native document.
    /// `workspace`, `tab`, `commands` and `label` select the revision and edit; returns no value.
    fn apply(
        &mut self,
        workspace: &mut Workspace,
        tab: Uuid,
        commands: Vec<DocumentCommand>,
        label: &str,
    ) {
        let result = workspace.edit_snapshot(tab).and_then(|(owner, _)| {
            workspace
                .apply(tab, owner.version, label.into(), &commands)
                .map(|_| ())
        });
        if let Err(error) = result {
            self.status = Some(error);
        }
    }
    /// Start one cancellable authoring worker with immutable document ownership.
    /// `workspace`, `tab`, `ctx` and `work` supply ownership, wakeup and work; returns immediately.
    fn start(
        &mut self,
        workspace: &Workspace,
        tab: Uuid,
        ctx: &egui::Context,
        work: impl FnOnce(
            DocumentEditor,
            Arc<Project>,
            editbay_media::Cancellation,
        ) -> Result<Completion, String>
        + Send
        + 'static,
    ) {
        if self.task.is_some() {
            return;
        }
        let (owner, editor) = match workspace.edit_snapshot(tab) {
            Ok(s) => s,
            Err(e) => {
                self.status = Some(e);
                return;
            }
        };
        let project = editor.snapshot();
        let cancel = match editbay_media::Cancellation::new() {
            Ok(c) => c,
            Err(e) => {
                self.status = Some(e.to_string());
                return;
            }
        };
        let token = cancel.clone();
        let activation = workspace.activation_generation;
        let (sender, result) = mpsc::sync_channel(1);
        let wake = ctx.clone();
        match std::thread::Builder::new()
            .name("editbay-editorial-command".into())
            .spawn(move || {
                let result = if token.is_cancelled() {
                    Err("Workflow cancelled".into())
                } else {
                    work(editor, project, token.clone())
                };
                let _ = sender.send(if token.is_cancelled() {
                    Err("Workflow cancelled".into())
                } else {
                    result
                });
                wake.request_repaint();
            }) {
            Ok(_) => {
                self.task = Some(Task {
                    owner,
                    activation,
                    cancel,
                    result,
                });
                self.status = None;
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }
}
/// Read a bounded workflow source without changing it.
/// `path` and `limit` select input; returns its bytes or a visible read/budget error.
fn read(path: &std::path::Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Workflow file exceeds read budget".into());
    }
    Ok(bytes)
}
/// Require an explicit caption format before reading or creating a file.
/// `path` supplies its suffix; returns true for VTT, false for SRT or a visible error.
fn caption_vtt(path: &std::path::Path) -> Result<bool, String> {
    match path.extension() {
        Some(extension) if extension.eq_ignore_ascii_case("srt") => Ok(false),
        Some(extension) if extension.eq_ignore_ascii_case("vtt") => Ok(true),
        _ => Err("Caption path must end in .srt or .vtt".into()),
    }
}

/// Write and synchronize one new workflow output.
/// `path` and `bytes` select content; returns an error if user output already exists.
fn write_new(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    editbay_core::write_new_bytes(path, bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "workflow_keyboard.rs"]
pub(crate) mod keyboard_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn craft_video_caption_draft_allows_clearing_retyping_and_a_second_line() {
        let mut cue = CaptionCue::new(
            CaptionTime::from_milliseconds(0),
            CaptionTime::from_milliseconds(1000),
            "Original",
        )
        .unwrap();
        let mut draft = CaptionTextDraft::new(&cue.text);
        for incomplete in ["", "First line\n", "First line\n\nSecond line"] {
            draft.text = incomplete.into();
            assert!(draft.validated_text(&cue).is_none());
            assert!(draft.error.is_some());
            assert_eq!(draft.text, incomplete);
            assert_eq!(draft.baseline, "Original");
            assert_eq!(cue.text, "Original");
        }
        draft.text = "First line\nSecond line".into();
        let text = draft.validated_text(&cue).unwrap();
        assert_eq!(text, "First line\nSecond line");
        assert!(draft.error.is_none());
        assert_eq!(cue.text, "Original");
        assert_eq!(draft.baseline, "Original");
        cue.text = text;
        draft.rebase(&cue.text);
        assert_eq!(draft.baseline, cue.text);
        assert!(
            draft.validated_text(&cue).is_none(),
            "an unchanged draft adds no command"
        );
    }

    #[test]
    fn craft_video_caption_drafts_preserve_unrelated_edits_and_reset_changed_baselines() {
        let tab = Uuid::new_v4();
        let project = Uuid::new_v4();
        let mut track = CaptionTrack::new(Uuid::new_v4(), "Captions").unwrap();
        track
            .insert_cue(
                CaptionCue::new(
                    CaptionTime::from_milliseconds(0),
                    CaptionTime::from_milliseconds(1000),
                    "Original",
                )
                .unwrap(),
            )
            .unwrap();
        let cue = track.cues[0].id;
        let mut drafts = CaptionDrafts::default();
        drafts.sync(tab, project, &track);
        drafts.cues.get_mut(&cue).unwrap().text = "Typing\n".into();
        track.style.font_size = 72.0;
        track.cues[0].end = CaptionTime::from_milliseconds(2000);
        drafts.sync(tab, project, &track);
        assert_eq!(drafts.cues[&cue].text, "Typing\n");
        assert_eq!(drafts.cues[&cue].baseline, "Original");
        assert!(
            drafts
                .cues
                .get_mut(&cue)
                .unwrap()
                .validated_text(&track.cues[0])
                .is_none()
        );
        assert!(drafts.cues[&cue].error.is_some());
        track.cues[0].text = "External text".into();
        drafts.sync(tab, project, &track);
        assert_eq!(drafts.cues[&cue].text, "External text");
        assert_eq!(drafts.cues[&cue].baseline, "External text");
        assert!(drafts.cues[&cue].error.is_none());
        drafts.cues.get_mut(&cue).unwrap().text = "Other unfinished input".into();
        drafts.observe_document(Uuid::new_v4(), project);
        assert!(drafts.cues.is_empty());
        drafts.sync(tab, project, &track);
        drafts.cues.get_mut(&cue).unwrap().text = "Pending".into();
        drafts.sync(tab, Uuid::new_v4(), &track);
        assert_eq!(drafts.cues[&cue].text, "External text");
        drafts.cues.get_mut(&cue).unwrap().text = "Pending".into();
        track.id = Uuid::new_v4();
        drafts.sync(tab, project, &track);
        assert_eq!(drafts.cues[&cue].text, "External text");
    }

    #[test]
    fn craft_video_caption_drafts_prune_removed_cues_and_initialize_inserted_cues() {
        let mut track = CaptionTrack::new(Uuid::new_v4(), "Captions").unwrap();
        let cue = CaptionCue::new(
            CaptionTime::from_milliseconds(0),
            CaptionTime::from_milliseconds(1000),
            "First",
        )
        .unwrap();
        let first = cue.id;
        track.insert_cue(cue).unwrap();
        let owner = (Uuid::new_v4(), Uuid::new_v4());
        let mut drafts = CaptionDrafts::default();
        drafts.sync(owner.0, owner.1, &track);
        drafts.cues.get_mut(&first).unwrap().text.clear();
        track.cues.clear();
        let cue = CaptionCue::new(
            CaptionTime::from_milliseconds(1000),
            CaptionTime::from_milliseconds(2000),
            "Second",
        )
        .unwrap();
        let second = cue.id;
        track.insert_cue(cue).unwrap();
        drafts.sync(owner.0, owner.1, &track);
        assert!(!drafts.cues.contains_key(&first));
        assert_eq!(drafts.cues[&second].text, "Second");
        assert_eq!(drafts.cues[&second].baseline, "Second");
    }

    #[test]
    fn craft_video_caption_ui_paths_require_explicit_srt_or_vtt_extensions() {
        for (path, vtt) in [
            ("caption.srt", false),
            ("caption.SRT", false),
            ("caption.VtT", true),
        ] {
            assert_eq!(caption_vtt(std::path::Path::new(path)).unwrap(), vtt);
        }
        for path in ["caption", "caption.txt", "caption.vtt.bak", "caption."] {
            assert!(caption_vtt(std::path::Path::new(path)).is_err());
        }
    }

    #[test]
    fn craft_video_completed_authoring_rejects_hidden_switched_and_changed_owners() {
        for change in 0..5 {
            let directory = tempfile::tempdir().unwrap();
            let mut workspace =
                Workspace::new(directory.path().join("recovery"), egui::Context::default());
            let tab = workspace.create(Project::new("Original").unwrap()).unwrap();
            let (owner, mut editor) = workspace.edit_snapshot(tab).unwrap();
            let activation = workspace.activation_generation;
            if change != 4 {
                editor
                    .apply(
                        owner.version,
                        "Worker edit".into(),
                        &[DocumentCommand::RenameProject {
                            name: "Worker result".into(),
                        }],
                    )
                    .unwrap();
            }
            let (sender, result) = mpsc::sync_channel(1);
            assert!(sender.send(Ok(Completion::Edited(editor))).is_ok());
            let mut pane = WorkflowPane {
                task: Some(Task {
                    owner,
                    activation,
                    cancel: editbay_media::Cancellation::new().unwrap(),
                    result,
                }),
                ..Default::default()
            };
            if change == 1 {
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
            if change == 2 {
                workspace
                    .create(Project::new("Other tab").unwrap())
                    .unwrap();
            }
            pane.poll(&mut workspace, change != 3);
            assert!(pane.task.is_none());
            let original = workspace
                .tabs
                .iter()
                .find(|t| t.id == tab)
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
            if (1..4).contains(&change) {
                assert!(pane.status.as_ref().unwrap().contains("discarded"));
            } else {
                assert!(pane.status.is_none());
                if change == 4 {
                    assert!(workspace.owns(owner));
                }
            }
        }
    }
}
