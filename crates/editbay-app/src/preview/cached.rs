use crate::workspace::DocumentOwner;
use editbay_core::{DocumentVersion, EvaluationSnapshot, FrameRange, Project};
use editbay_media::{
    Cancellation,
    picture_store::{
        PlanSummary, PreparedStore, StoreBudget, StorePlan, StoreProgress, StoreSpace,
    },
    picture_worker::{PictureWorker, WorkerBudget},
};
use eframe::egui;
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver},
    },
    thread::JoinHandle,
};
use uuid::Uuid;

#[derive(Default, Clone, Serialize)]
struct Progress {
    plan: Option<PlanSummary>,
    completed: Option<StoreProgress>,
    worker_pid: Option<u32>,
}
struct Job {
    cancel: Cancellation,
    thread: Option<JoinHandle<()>>,
    result: Receiver<Result<PreparedStore, String>>,
}
impl Job {
    fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
pub(super) struct Cache {
    directory: Option<PathBuf>,
    space: Option<StoreSpace>,
    scope: Option<(DocumentOwner, Uuid)>,
    job: Option<Job>,
    retiring: Vec<Job>,
    ready: Option<PreparedStore>,
    progress: Arc<Mutex<Progress>>,
    pub(super) error: Option<String>,
}
impl Cache {
    pub(super) fn new(directory: PathBuf) -> Self {
        match StoreSpace::new(StoreBudget::default()) {
            Ok(space) => Self {
                directory: Some(directory),
                space: Some(space),
                ..Self::default()
            },
            Err(error) => Self {
                error: Some(error.to_string()),
                ..Self::default()
            },
        }
    }
    pub(super) fn available(&self) -> bool {
        self.space.is_some()
    }
    pub(super) fn busy(&self) -> bool {
        self.job.is_some() || !self.retiring.is_empty()
    }
    pub(super) fn preparing(&self) -> bool {
        self.job.is_some()
    }
    pub(super) fn ready(&self, owner: DocumentOwner, sequence: Uuid) -> Option<PreparedStore> {
        (self.scope == Some((owner, sequence)))
            .then(|| self.ready.clone())
            .flatten()
    }
    pub(super) fn invalidate(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.cancel();
            self.retiring.push(job);
        }
        self.ready = None;
        self.scope = None;
        self.error = None;
    }
    pub(super) fn observe(&mut self, owner: DocumentOwner, sequence: Uuid) {
        if self.scope.is_some_and(|scope| scope != (owner, sequence)) {
            self.invalidate();
        }
    }
    pub(super) fn start(
        &mut self,
        owner: DocumentOwner,
        project: Arc<Project>,
        sequence: Uuid,
        ctx: egui::Context,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("Picture preparation is still stopping".into());
        }
        if owner.version != DocumentVersion::of(&project) {
            return Err("Picture preparation document changed".into());
        }
        let directory = self
            .directory
            .clone()
            .ok_or("Picture storage is unavailable")?;
        let space = self.space.clone().ok_or("Picture storage is unavailable")?;
        let cancel = Cancellation::new().map_err(|e| e.to_string())?;
        let token = cancel.clone();
        let progress = Arc::new(Mutex::new(Progress::default()));
        let output = progress.clone();
        let (sender, result) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("editbay-prepare-pictures".into())
            .spawn(move || {
                let run = || -> Result<PreparedStore, String> {
                    let snapshot =
                        Arc::new(EvaluationSnapshot::new(project).map_err(|e| e.to_string())?);
                    let composition = snapshot
                        .project()
                        .sequences
                        .iter()
                        .find(|item| item.id == sequence)
                        .and_then(|item| item.composition)
                        .ok_or("Sequence has no picture composition")?;
                    let duration = snapshot
                        .project()
                        .compositions
                        .iter()
                        .find(|item| item.id == composition)
                        .ok_or("Picture composition is absent")?
                        .duration;
                    let budget = WorkerBudget::default();
                    let plan = StorePlan::new(
                        snapshot.clone(),
                        composition,
                        FrameRange {
                            start: 0,
                            end: duration,
                        },
                        budget.pictures,
                        StoreBudget::default(),
                        &token,
                    )
                    .map_err(|e| e.to_string())?;
                    output.lock().map_err(|e| e.to_string())?.plan = Some(plan.summary());
                    ctx.request_repaint();
                    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
                    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
                    let mut provider =
                        PictureWorker::new(&executable, snapshot, budget, token.clone())
                            .map_err(|e| e.to_string())?;
                    output.lock().map_err(|e| e.to_string())?.worker_pid = provider.process_id();
                    plan.prepare(&directory, &space, &mut provider, &token, |progress| {
                        if let Ok(mut output) = output.lock() {
                            output.completed = Some(progress);
                        }
                        ctx.request_repaint();
                    })
                    .map_err(|e| e.to_string())
                };
                let outcome = run();
                let _ = sender.send(outcome);
                ctx.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        self.ready = None;
        self.scope = Some((owner, sequence));
        self.error = None;
        self.progress = progress;
        self.job = Some(Job {
            cancel,
            thread: Some(thread),
            result,
        });
        Ok(())
    }
    pub(super) fn poll(&mut self) -> bool {
        self.retiring.retain(|job| !job.finished());
        if self.job.as_ref().is_none_or(|job| !job.finished()) {
            return false;
        }
        let job = self.job.take().unwrap();
        match job.result.try_recv() {
            Ok(Ok(store))
                if !job.cancel.is_cancelled()
                    && self
                        .scope
                        .is_some_and(|(owner, _)| owner.version == store.summary().version) =>
            {
                self.ready = Some(store);
                true
            }
            Ok(Err(error)) => {
                self.error = Some(error);
                false
            }
            _ => {
                self.error = Some("Picture preparation stopped without a current result".into());
                false
            }
        }
    }
    pub(super) fn label(&self) -> String {
        if !self.retiring.is_empty() {
            return "Stopping picture preparation…".into();
        }
        if let Some(store) = &self.ready {
            return format!(
                "Prepared · {} pictures · {:.1} MiB",
                store.summary().pictures,
                store.summary().bytes as f64 / (1024. * 1024.)
            );
        }
        if self.job.is_some() {
            if let Ok(progress) = self.progress.lock()
                && let Some(completed) = progress.completed
            {
                return format!(
                    "Preparing pictures · {} / {}",
                    completed.pictures, completed.total_pictures
                );
            }
            return "Planning picture preparation…".into();
        }
        String::new()
    }
    pub(super) fn diagnostic(&self) -> serde_json::Value {
        serde_json::json!({"preparing":self.preparing(),"retiring":self.retiring.len(),
            "ready":self.ready.as_ref().map(PreparedStore::summary),"error":self.error,
            "progress":self.progress.lock().ok().map(|p|p.clone()),"usage":self.space.as_ref().map(StoreSpace::usage)})
    }
}
