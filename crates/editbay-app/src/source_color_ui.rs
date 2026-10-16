use crate::workspace::{DocumentOwner, Workspace};
use editbay_core::{DocumentEditor, Project, StreamFormat};
use editbay_media::{Cancellation, ColorTransform};
use eframe::egui::{self, Ui};
use std::sync::{
    Arc,
    mpsc::{self, Receiver},
};
use uuid::Uuid;

#[cfg(test)]
#[path = "source_color_tests.rs"]
mod tests;

struct Task {
    owner: DocumentOwner,
    activation: u64,
    cancel: Cancellation,
    result: Receiver<Result<DocumentEditor, String>>,
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Saved source interpretation through the shared native media and document paths.
#[derive(Default)]
pub struct SourceColorPane {
    selection: Option<(Uuid, u32)>,
    path: String,
    task: Option<Task>,
    status: Option<String>,
}
impl SourceColorPane {
    /// Reject obsolete source-profile workers before accepting their single edit.
    /// `workspace` and `visible` identify the active document; returns without waiting.
    pub fn poll(&mut self, workspace: &mut Workspace, visible: bool) {
        let Some(task) = &self.task else { return };
        let stale = !visible
            || workspace.active != Some(task.owner.tab)
            || workspace.activation_generation != task.activation
            || !workspace.owns(task.owner)
            || task.cancel.is_cancelled();
        if stale {
            task.cancel.cancel();
        }
        let result = match task.result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("Source color worker stopped".into()),
        };
        let owner = task.owner;
        self.task = None;
        self.status = Some(if stale {
            "The project changed; the source profile result was discarded".into()
        } else {
            match result.and_then(|editor| {
                if editbay_core::DocumentVersion::of(editor.project()) == owner.version {
                    Ok(())
                } else {
                    workspace.commit_edit(owner, editor)
                }
            }) {
                Ok(()) => {
                    "Source profile applied. Save the project to retain this interpretation.".into()
                }
                Err(error) => error,
            }
        });
    }

    /// Draw explicit profile assignment for a captured native video stream.
    /// `ui`, `workspace`, `tab`, `project` and `interactive` supply the current owner.
    /// Returns immediately; profile IO and native parsing run on one bounded worker.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        workspace: &mut Workspace,
        tab: Uuid,
        project: Arc<Project>,
        interactive: bool,
    ) {
        ui.push_id(("source-color", tab, project.id), |ui| {
            ui.collapsing("Source ICC interpretation", |ui| {
                let choices: Vec<_> = project
                    .sources
                    .iter()
                    .flat_map(|source| {
                        source
                            .streams
                            .iter()
                            .filter(|stream| matches!(stream.format, StreamFormat::Video { .. }))
                            .map(move |stream| {
                                (
                                    (source.id, stream.index),
                                    format!("{} · video {}", source.name, stream.index),
                                    stream,
                                )
                            })
                    })
                    .collect();
                if choices.is_empty() {
                    ui.label("Import a video or image source to assign its ICC profile.");
                    return;
                }
                if !choices
                    .iter()
                    .any(|choice| Some(choice.0) == self.selection)
                {
                    self.selection = choices.first().map(|choice| choice.0);
                }
                let selected = choices
                    .iter()
                    .find(|choice| Some(choice.0) == self.selection)
                    .unwrap();
                let stream = selected.2;
                ui.label("Source video stream");
                egui::ComboBox::from_id_salt("stream")
                    .selected_text(&selected.1)
                    .show_ui(ui, |ui| {
                        for (identity, name, _) in &choices {
                            ui.selectable_value(&mut self.selection, Some(*identity), name);
                        }
                    })
                    .response
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::ComboBox,
                            true,
                            "Source ICC video stream",
                        )
                    });
                if let Some(profile) = &stream.icc_profile {
                    ui.label(format!(
                        "Captured RGB ICC: {} · {} bytes",
                        profile.sha256,
                        profile.bytes.len()
                    ));
                } else {
                    ui.label("Using the source color tags; no ICC profile is assigned.");
                }
                ui.label("Original RGB ICC file");
                ui.add(
                    egui::TextEdit::singleline(&mut self.path)
                        .id_salt("profile-path")
                        .hint_text("/home/…/source.icc"),
                )
                .widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::TextEdit,
                        true,
                        "Source ICC file path",
                    )
                });
                ui.add_enabled_ui(interactive && self.task.is_none(), |ui| {
                    if ui
                        .add_enabled(
                            !self.path.trim().is_empty(),
                            egui::Button::new("Assign source ICC"),
                        )
                        .clicked()
                    {
                        self.start(workspace, tab, ui.ctx(), false);
                    }
                    if ui
                        .add_enabled(
                            stream.icc_profile.is_some(),
                            egui::Button::new("Clear source ICC"),
                        )
                        .clicked()
                    {
                        self.start(workspace, tab, ui.ctx(), true);
                    }
                });
                if let Some(task) = &self.task {
                    ui.spinner();
                    if ui.button("Cancel source profile").clicked() {
                        task.cancel.cancel();
                    }
                }
                if let Some(status) = &self.status {
                    ui.label(status);
                }
            });
        });
    }

    fn start(&mut self, workspace: &Workspace, tab: Uuid, ctx: &egui::Context, clear: bool) {
        let Some((source, stream)) = self.selection else {
            return;
        };
        let begin = || -> Result<_, String> {
            let (owner, mut editor) = workspace.edit_snapshot(tab)?;
            let cancel = Cancellation::new().map_err(|error| error.to_string())?;
            let token = cancel.clone();
            let activation = workspace.activation_generation;
            let path = std::path::PathBuf::from(self.path.trim());
            let (sender, result) = mpsc::sync_channel(1);
            let wake = ctx.clone();
            std::thread::Builder::new()
                .name("editbay-source-color".into())
                .spawn(move || {
                    let work = || -> Result<_, Box<dyn std::error::Error>> {
                        let profile = if clear {
                            None
                        } else {
                            let profile = ColorTransform::read_profile(&path, &token)?;
                            ColorTransform::new(
                                &profile,
                                editor.project().color.working_gamut,
                                editbay_core::AlphaMode::Straight,
                                token.clone(),
                            )?;
                            Some(profile)
                        };
                        let command = editbay_core::source_icc_command(
                            editor.project(),
                            source,
                            stream,
                            profile,
                        )?;
                        editor.apply(
                            owner.version,
                            "Assign source ICC profile".into(),
                            &[command],
                        )?;
                        token.check()?;
                        Ok(editor)
                    };
                    let result = work().map_err(|error| error.to_string());
                    let _ = sender.send(result);
                    wake.request_repaint();
                })
                .map_err(|error| error.to_string())?;
            Ok(Task {
                owner,
                activation,
                cancel,
                result,
            })
        };
        match begin() {
            Ok(task) => {
                self.task = Some(task);
                self.status = None;
            }
            Err(error) => self.status = Some(error),
        }
    }
}
