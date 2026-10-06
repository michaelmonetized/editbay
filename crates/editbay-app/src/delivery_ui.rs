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

impl Drop for Job {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

/// Bounded export history with one active native job and nonblocking retirement.
#[derive(Default)]
pub struct DeliveryPane {
    jobs: Vec<Job>,
}

impl DeliveryPane {
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
                        ui.label(format!("Revision {} exported and verified · {} pictures · {} Hz · {} channels", receipt.version.revision, receipt.profile.frames, receipt.profile.sample_rate, receipt.profile.channels.len()));
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
        serde_json::json!({"busy":self.busy(),"jobs":self.jobs.iter().map(|job|serde_json::json!({
            "id":job.id,"owner":job.owner.version,"tab":job.owner.tab,"request":job.request,"destination":job.destination,
            "running":job.thread.is_some(),"progress":job.progress,"worker_pid":job.pid,
            "receipt":job.receipt,"error":job.error,"cancellation":job.cancellation})).collect::<Vec<_>>()})
    }
}
