use crate::{
    preview::PreviewPane,
    workspace::{DocumentOwner, Workspace},
};
use editbay_delivery::{DeliveryControl, DeliveryRequest, Phase, Progress, Receipt};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::Duration,
};
use uuid::Uuid;

struct Job {
    id: Uuid,
    owner: DocumentOwner,
    request: DeliveryRequest,
    destination: PathBuf,
    control: DeliveryControl,
    thread: Option<JoinHandle<()>>,
    updates: Arc<Mutex<Option<(Progress, u32)>>>,
    result: mpsc::Receiver<Result<Receipt, String>>,
    progress: Option<Progress>,
    pid: Option<u32>,
    receipt: Option<Receipt>,
    error: Option<String>,
    cancellation: Option<String>,
}

struct ExportChoice {
    owner: DocumentOwner,
    composition: Uuid,
    name: String,
    duration: u64,
    full: bool,
    start: String,
    end: String,
}

impl ExportChoice {
    fn request(&self) -> Result<DeliveryRequest, String> {
        let request = DeliveryRequest {
            composition: self.composition,
            sample_rate: 48000,
            range: if self.full {
                None
            } else {
                Some(editbay_core::FrameRange {
                    start: self
                        .start
                        .parse()
                        .map_err(|_| "Enter a whole start frame")?,
                    end: self.end.parse().map_err(|_| "Enter a whole end frame")?,
                })
            },
        };
        request
            .frame_range(self.duration)
            .map_err(|error| error.to_string())?;
        Ok(request)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

/// Bounded export history with one active native job and nonblocking retirement.
#[derive(Default)]
pub struct DeliveryPane {
    jobs: Vec<Job>,
    choice: Option<ExportChoice>,
}

impl DeliveryPane {
    /// Open export options for one captured native sequence.
    /// `owner`, `composition`, `name` and `duration` identify the saved selection.
    /// Returns immediately; destination choice follows a validated frame range.
    pub fn choose(&mut self, owner: DocumentOwner, composition: Uuid, name: String, duration: u64) {
        self.choice = Some(ExportChoice {
            owner,
            composition,
            name,
            duration,
            full: true,
            start: "0".into(),
            end: duration.to_string(),
        });
    }

    /// Draw full-sequence or explicit range options before choosing a new file.
    /// `ctx`, `workspace` and `preview` provide the window, owner checks and native
    /// diagnostic geometry. Returns only a valid, still-owned destination request.
    pub fn show_choice(
        &mut self,
        ctx: &egui::Context,
        workspace: &Workspace,
        preview: &mut PreviewPane,
    ) -> Option<(DocumentOwner, DeliveryRequest, String)> {
        let choice = self.choice.as_mut()?;
        let mut accepted = None;
        let mut cancelled = false;
        let modal = egui::Modal::new(egui::Id::new("export-options")).show(ctx, |ui| {
            ui.set_min_width(360.);
            ui.heading("Export sequence");
            ui.label("Lossless MOV · 48 kHz sound · original channels");
            ui.add_space(8.);
            let whole = ui.radio_value(&mut choice.full, true, "Full sequence");
            preview.observe_control("export-full", &whole, ui);
            let range = ui.radio_value(&mut choice.full, false, "Frame range");
            preview.observe_control("export-range", &range, ui);
            ui.add_enabled_ui(!choice.full, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Start");
                    let start =
                        ui.add(egui::TextEdit::singleline(&mut choice.start).desired_width(85.));
                    preview.observe_control("export-start", &start, ui);
                    ui.label("End");
                    let end =
                        ui.add(egui::TextEdit::singleline(&mut choice.end).desired_width(85.));
                    preview.observe_control("export-end", &end, ui);
                });
                ui.weak(format!(
                    "Frames start at 0. End is excluded. Sequence ends at {}.",
                    choice.duration
                ));
            });
            let request = choice.request();
            let owned = workspace.owns(choice.owner);
            if !owned {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "The project changed. Close these options and choose the sequence again.",
                );
            } else if let Err(error) = &request {
                ui.colored_label(ui.visuals().error_fg_color, error);
            } else if let Ok(request) = &request
                && let Ok(range) = request.frame_range(choice.duration)
            {
                ui.label(format!("{} pictures", range.end - range.start));
            }
            ui.add_space(8.);
            ui.horizontal(|ui| {
                let next = ui.add_enabled(
                    owned && request.is_ok(),
                    egui::Button::new("Choose destination…"),
                );
                preview.observe_control("export-continue", &next, ui);
                if next.clicked() {
                    accepted = request.ok();
                }
                let cancel = ui.button("Cancel");
                preview.observe_control("export-options-cancel", &cancel, ui);
                cancelled = cancel.clicked();
            });
        });
        if let Some(request) = accepted {
            let choice = self.choice.take()?;
            return Some((choice.owner, request, choice.name));
        }
        if cancelled || modal.should_close() {
            self.choice = None;
        }
        None
    }

    /// Inspect whether rendering, verification or worker retirement is active.
    /// Takes no arguments; returns true until its owning thread has been reaped.
    pub fn busy(&self) -> bool {
        self.jobs.iter().any(|job| job.thread.is_some())
    }

    /// Request cancellation for all owned jobs without waiting on their workers.
    /// Takes no arguments; jobs already publishing finish their verified write.
    pub fn cancel(&self) {
        for job in &self.jobs {
            job.control.cancel();
        }
    }

    /// Start the selected captured composition after a native destination choice.
    /// `workspace` and `owner` must still match; `request`, `destination` and `ctx`
    /// select content, a new output and repaint notifications. Returns immediately.
    pub fn start(
        &mut self,
        workspace: &mut Workspace,
        owner: DocumentOwner,
        request: DeliveryRequest,
        destination: PathBuf,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("Wait for the active export to finish or cancel it".into());
        }
        if !workspace.owns(owner) {
            return Err(
                "The project changed while choosing the export destination; choose again".into(),
            );
        }
        let (current, project) = workspace.edit_snapshot(owner.tab)?;
        if current != owner {
            return Err("Export document ownership changed".into());
        }
        let control = DeliveryControl::new().map_err(|e| e.to_string())?;
        workspace.guard_export(owner, control.clone())?;
        let updates = Arc::new(Mutex::new(None));
        let retained = updates.clone();
        let cancel = control.clone();
        let path = destination.clone();
        let wake = ctx.clone();
        let (sender, result) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("editbay-delivery-controller".into())
            .spawn(move || {
                let run = || -> Result<Receipt, String> {
                    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
                    editbay_delivery::deliver(
                        &executable,
                        project.snapshot(),
                        request,
                        &path,
                        &cancel,
                        |progress, pid| {
                            if let Ok(mut retained) = retained.lock() {
                                *retained = Some((progress.clone(), pid));
                            }
                            wake.request_repaint();
                        },
                    )
                    .map_err(|e| e.to_string())
                };
                let _ = sender.send(run());
                wake.request_repaint();
            })
            .map_err(|e| {
                control.cancel();
                e.to_string()
            })?;
        if self.jobs.len() == 8 {
            self.jobs.remove(0);
        }
        self.jobs.push(Job {
            id: Uuid::new_v4(),
            owner,
            request,
            destination,
            control,
            thread: Some(thread),
            updates,
            result,
            progress: None,
            pid: None,
            receipt: None,
            error: None,
            cancellation: None,
        });
        Ok(())
    }

    /// Receive bounded progress and cancel obsolete document ownership.
    /// `workspace` supplies live sessions and `ctx` schedules input-safe repainting.
    /// Returns after joining only already-finished threads.
    pub fn poll(&mut self, workspace: &Workspace, ctx: &egui::Context) {
        for job in &mut self.jobs {
            if job.thread.is_none() {
                continue;
            }
            if !workspace.owns(job.owner) && job.control.cancel() {
                job.cancellation = Some("Project changed or closed; export cancelled".into());
            }
            if let Ok(mut updates) = job.updates.try_lock()
                && let Some((progress, pid)) = updates.take()
            {
                job.progress = Some(progress);
                job.pid = Some(pid);
            }
            if job.thread.as_ref().is_some_and(JoinHandle::is_finished) {
                let joined = job.thread.take().unwrap().join();
                match (joined, job.result.try_recv()) {
                    (Ok(()), Ok(Ok(receipt))) => job.receipt = Some(receipt),
                    (Ok(()), Ok(Err(error))) => job.error = Some(error),
                    _ => job.error = Some("Export worker ended without a result".into()),
                }
            }
        }
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    /// Draw export progress, errors, cancellation and retry for the current session.
    /// `ui`, `workspace` and `preview` supply controls, owners and diagnostic geometry.
    /// Returns a visible startup error when retry cannot capture current content.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &mut Workspace,
        preview: &mut PreviewPane,
    ) -> Result<(), String> {
        if self.jobs.is_empty() {
            return Ok(());
        }
        let busy = self.busy();
        let mut retry = None;
        egui::CollapsingHeader::new("Exports").default_open(true).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("export-history").max_height(200.).show(ui, |ui| {
            for (index, job) in self.jobs.iter_mut().enumerate().rev() {
                ui.group(|ui| {
                    ui.label(job.destination.file_name().unwrap_or_default().to_string_lossy());
                    if let Some(receipt) = &job.receipt {
                        ui.label(format!("Revision {} exported and verified · frames {}–{} · {} Hz · {} channels", receipt.version.revision, receipt.profile.first_frame, receipt.profile.first_frame + receipt.profile.frames, receipt.profile.sample_rate, receipt.profile.channels.len()));
                        ui.weak(job.destination.to_string_lossy());
                        if receipt.clipped_picture_values > 0 { ui.colored_label(ui.visuals().warn_fg_color, "Picture values outside the 8-bit output range were clipped"); }
                    } else if job.thread.is_some() {
                        let label = if job.cancellation.is_some() { "Cancelling…" } else {
                            match job.progress.as_ref().map(|p|p.phase) {
                                None | Some(Phase::Preparing) => "Preparing…",
                                Some(Phase::Rendering) => "Rendering picture and sound…",
                                Some(Phase::VerifyingPictures) => "Checking every picture…",
                                Some(Phase::VerifyingSound) => "Checking every sound sample…",
                                Some(Phase::VerifyingFile) => "Checking the finished file…",
                                Some(Phase::Publishing | Phase::Complete) => "Finishing export…",
                            }
                        };
                        ui.label(label);
                        if let Some(progress) = &job.progress {
                            if progress.phase == Phase::Preparing && progress.total_preparation_samples > 0 {
                                ui.label(format!("Preparing source sound: {} / {} samples", progress.prepared_samples, progress.total_preparation_samples));
                            }
                            let fraction = match progress.phase {
                                Phase::Rendering => 0.8 * progress.pictures as f32 / progress.total_pictures.max(1) as f32,
                                Phase::VerifyingPictures => 0.8 + 0.1 * progress.pictures as f32 / progress.total_pictures.max(1) as f32,
                                Phase::VerifyingSound => 0.9 + 0.09 * progress.samples as f32 / progress.total_samples.max(1) as f32,
                                Phase::VerifyingFile | Phase::Publishing | Phase::Complete => 0.99,
                                Phase::Preparing => 0.,
                            };
                            ui.add(egui::ProgressBar::new(fraction).show_percentage());
                        }
                        let cancel = ui.add_enabled(job.control.cancellable(), egui::Button::new("Cancel export"));
                        preview.observe_control(&format!("cancel-export:{}", job.id), &cancel, ui);
                        if cancel.clicked() && job.control.cancel() { job.cancellation = Some("Export cancelled".into()); }
                    } else if let Some(error) = job.cancellation.as_ref().or(job.error.as_ref()) {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                        let button = ui.add_enabled(!busy && workspace.tabs.iter().any(|tab|tab.id == job.owner.tab), egui::Button::new("Retry current version"));
                        preview.observe_control(&format!("retry-export:{}", job.id), &button, ui);
                        if button.clicked() { retry = Some(index); }
                    }
                });
            }
            });
        });
        if let Some(index) = retry {
            let job = &self.jobs[index];
            let (owner, _) = workspace.edit_snapshot(job.owner.tab)?;
            self.start(
                workspace,
                owner,
                job.request,
                job.destination.clone(),
                ui.ctx(),
            )?;
        }
        Ok(())
    }

    /// Inspect opt-in job ownership, progress and outcomes without reading files.
    /// Takes no arguments; returns bounded diagnostic history, never media payloads.
    pub fn diagnostic_state(&self) -> serde_json::Value {
        serde_json::json!({"busy":self.busy(),"choice":self.choice.as_ref().map(|choice|serde_json::json!({
            "owner":choice.owner.version,"composition":choice.composition,"full":choice.full,"start":choice.start,"end":choice.end,"duration":choice.duration})),"jobs":self.jobs.iter().map(|job|serde_json::json!({
            "id":job.id,"owner":job.owner.version,"tab":job.owner.tab,"request":job.request,"destination":job.destination,
            "running":job.thread.is_some(),"progress":job.progress,"worker_pid":job.pid,
            "receipt":job.receipt,"error":job.error,"cancellation":job.cancellation})).collect::<Vec<_>>()})
    }
}
