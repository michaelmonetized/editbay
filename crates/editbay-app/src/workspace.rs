use editbay_core::{
    DocumentCommand, DocumentEditor, DocumentVersion, Project, RecoveryCatalog, load_bounded,
    prepare_checkpoint, recover_copy, recovery_catalog, save_if_unchanged, save_new,
};
use eframe::egui;
use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

mod recovery;

pub struct Tab {
    pub id: Uuid,
    pub editor: DocumentEditor,
    pub path: Option<PathBuf>,
    pub saved: Option<Arc<Project>>,
    pub recovery_revision: Option<u64>,
    pub recovery_error: Option<String>,
    generation: u64,
    exports: Vec<editbay_delivery::DeliveryControl>,
    last_edit: Instant,
    recovery_since: Instant,
    retry_after: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentOwner {
    pub tab: Uuid,
    pub version: DocumentVersion,
    generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RecoveryWork {
    pub job: Uuid,
    pub tab: Uuid,
    pub version: DocumentVersion,
    pub phase: &'static str,
}

impl Tab {
    fn cancel_exports(&mut self) {
        for export in self.exports.drain(..) {
            export.cancel();
        }
    }

    /// Determine whether this document still needs a manual save.
    /// Takes no arguments; returns true for untitled or changed documents,
    /// independently of whether a recovery snapshot already exists.
    pub fn dirty(&self) -> bool {
        self.saved.as_deref().map(DocumentVersion::of)
            != Some(DocumentVersion::of(self.editor.project()))
    }
}

enum Pending {
    Open,
    Save {
        tab: Uuid,
        generation: u64,
    },
    Recovery {
        tab: Uuid,
        generation: u64,
        version: DocumentVersion,
        publication: recovery::Publication,
    },
    Copy,
    Recoveries,
}

enum Outcome {
    Opened {
        path: PathBuf,
        editor: DocumentEditor,
    },
    Saved {
        path: PathBuf,
        project: Arc<Project>,
    },
    Prepared,
    Checkpoint(Duration),
    RecoveryCancelled,
    Copy(PathBuf),
    Recoveries(RecoveryCatalog),
}

pub struct Workspace {
    pub tabs: Vec<Tab>,
    pub active: Option<Uuid>,
    pub activation_generation: u64,
    pub recoveries: RecoveryCatalog,
    pub errors: VecDeque<String>,
    pub last_saved_copy: Option<PathBuf>,
    pub recovery_publication_times: Vec<Duration>,
    pub recovery_accept_times: Vec<Duration>,
    recovery_root: PathBuf,
    pending: HashMap<Uuid, Pending>,
    open_queue: VecDeque<PathBuf>,
    sender: Sender<(Uuid, Result<Outcome, String>)>,
    receiver: Receiver<(Uuid, Result<Outcome, String>)>,
    wake: egui::Context,
    catalog_dirty: bool,
}

impl Workspace {
    /// Bind export cancellation to this exact document before mutation.
    /// `owner` must still be current; `control` shares its atomic publication
    /// barrier. Returns an error without registration for stale or excess jobs.
    pub fn guard_export(
        &mut self,
        owner: DocumentOwner,
        control: editbay_delivery::DeliveryControl,
    ) -> Result<(), String> {
        if !self.owns(owner) {
            return Err("Export document ownership changed".into());
        }
        let tab = self.tab_mut(owner.tab)?;
        tab.exports.retain(|job| job.cancellable());
        if tab.exports.len() >= 8 {
            return Err("Document export job limit reached".into());
        }
        tab.exports.push(control);
        Ok(())
    }

    /// Capture a document for a background authoring operation.
    /// `id` selects its independent session. Returns ownership and a cheap
    /// immutable editor clone; save/replacement/edit/close invalidates ownership.
    pub fn edit_snapshot(&self, id: Uuid) -> Result<(DocumentOwner, DocumentEditor), String> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .ok_or("This document is no longer open")?;
        Ok((
            DocumentOwner {
                tab: id,
                version: DocumentVersion::of(tab.editor.project()),
                generation: tab.generation,
            },
            tab.editor.clone(),
        ))
    }

    /// Check that a worker still owns its starting document session.
    /// `owner` was captured before work. Returns false after an edit, save,
    /// replacement or close; switching to another tab does not invalidate it.
    pub fn owns(&self, owner: DocumentOwner) -> bool {
        self.tabs.iter().any(|tab| {
            tab.id == owner.tab
                && tab.generation == owner.generation
                && DocumentVersion::of(tab.editor.project()) == owner.version
        })
    }

    /// Publish an already validated background command result.
    /// `owner` captures the starting session and `editor` contains one newer
    /// revision. Returns a stale error before mutation if ownership changed.
    pub fn commit_edit(
        &mut self,
        owner: DocumentOwner,
        editor: DocumentEditor,
    ) -> Result<(), String> {
        if !self.owns(owner) {
            return Err("Background change belongs to an older project state; try again".into());
        }
        if editor.project().id != owner.version.project_id
            || editor.project().revision
                != owner
                    .version
                    .revision
                    .checked_add(1)
                    .ok_or("Document revision overflow")?
        {
            return Err("Worker returned an invalid document revision".into());
        }
        let tab = self.tab_mut(owner.tab)?;
        let was_dirty = tab.dirty();
        tab.cancel_exports();
        tab.editor = editor;
        tab.last_edit = Instant::now();
        if !was_dirty {
            tab.recovery_since = tab.last_edit;
        }
        self.wake.request_repaint();
        Ok(())
    }

    /// Own local documents and bounded filesystem workers.
    /// `recovery_root` selects EditBay's independent recovery storage; `wake`
    /// receives worker repaint requests. Returns an empty offline workspace.
    pub fn new(recovery_root: PathBuf, wake: egui::Context) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            tabs: Vec::new(),
            active: None,
            activation_generation: 0,
            recoveries: RecoveryCatalog::default(),
            errors: VecDeque::new(),
            last_saved_copy: None,
            recovery_publication_times: Vec::new(),
            recovery_accept_times: Vec::new(),
            recovery_root,
            pending: HashMap::new(),
            open_queue: VecDeque::new(),
            sender,
            receiver,
            wake,
            catalog_dirty: false,
        }
    }

    /// Open a fresh validated document as dirty untitled work.
    /// `project` comes from the core constructor. Returns its independent tab
    /// identity; it is recoverable even before a destination has been chosen.
    pub fn create(&mut self, project: Project) -> Result<Uuid, String> {
        self.add(
            DocumentEditor::new(project).map_err(|error| error.to_string())?,
            None,
        )
    }

    fn add(&mut self, editor: DocumentEditor, path: Option<PathBuf>) -> Result<Uuid, String> {
        if self.tabs.len() >= 128 {
            return Err(
                "The workspace supports 128 open projects; close a tab to open another".into(),
            );
        }
        let now = Instant::now();
        let id = Uuid::new_v4();
        let saved = path.as_ref().map(|_| editor.snapshot());
        self.tabs.push(Tab {
            id,
            editor,
            path,
            saved,
            recovery_revision: None,
            recovery_error: None,
            generation: 0,
            exports: Vec::new(),
            last_edit: now,
            recovery_since: now,
            retry_after: now,
        });
        self.active = Some(id);
        Ok(id)
    }

    /// Queue files for explicit multi-open without blocking input.
    /// `paths` contains caller-selected local files. Returns no value; failures
    /// remain visible and an already opened canonical path activates its tab.
    pub fn open(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if self.open_queue.len() + self.tabs.len() >= 128 {
                self.error(
                    "The workspace supports 128 open projects; close a tab to open another".into(),
                );
                break;
            }
            if !self.open_queue.contains(&path) {
                self.open_queue.push_back(path);
            }
        }
        self.wake.request_repaint();
    }

    /// Apply a validated command to an owned tab.
    /// `id` selects the tab; `expected` prevents a late UI/dialog action from
    /// changing another revision; `label` and `commands` form one undo group.
    /// Returns success only after the core editor accepts the complete group.
    pub fn apply(
        &mut self,
        id: Uuid,
        expected: DocumentVersion,
        label: String,
        commands: &[DocumentCommand],
    ) -> Result<(), String> {
        let tab = self.tab_mut(id)?;
        let was_dirty = tab.dirty();
        let mut editor = tab.editor.clone();
        let receipt = editor
            .apply(expected, label, commands)
            .map_err(|e| e.to_string())?;
        if receipt.changed {
            tab.cancel_exports();
            tab.editor = editor;
            tab.last_edit = Instant::now();
            if !was_dirty {
                tab.recovery_since = tab.last_edit;
            }
        }
        Ok(())
    }

    /// Undo or redo through the same monotonic core command history.
    /// `id` selects the owned tab and `redo` chooses direction. Returns a
    /// visible failure when no step exists; successful changes become dirty.
    pub fn history(&mut self, id: Uuid, redo: bool) -> Result<(), String> {
        let tab = self.tab_mut(id)?;
        let was_dirty = tab.dirty();
        let expected = DocumentVersion::of(tab.editor.project());
        let mut editor = tab.editor.clone();
        if redo {
            editor.redo(expected)
        } else {
            editor.undo(expected)
        }
        .map_err(|e| e.to_string())?;
        tab.cancel_exports();
        tab.editor = editor;
        tab.last_edit = Instant::now();
        if !was_dirty {
            tab.recovery_since = tab.last_edit;
        }
        Ok(())
    }

    /// Save one immutable revision on a filesystem worker.
    /// `id` selects an open document, `expected` owns its revision, and `destination` selects its current path
    /// or a separate new file. Returns a job ID; acknowledgement arrives in poll.
    /// New destinations refuse overwrite and existing saves compare loaded state.
    pub fn save(
        &mut self,
        id: Uuid,
        expected: DocumentVersion,
        destination: PathBuf,
    ) -> Result<Uuid, String> {
        if self.saving(id) {
            return Err("This project is already saving".into());
        }
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .ok_or("This document is no longer open")?;
        let project = tab.editor.snapshot();
        if DocumentVersion::of(&project) != expected {
            return Err("Save dialog belongs to an older document revision".into());
        }
        let generation = tab
            .generation
            .checked_add(1)
            .ok_or("Document session generation overflow")?;
        let current = tab.path.clone();
        let saved = tab.saved.clone();
        let job = self.spawn(
            Pending::Save {
                tab: id,
                generation,
            },
            move || {
                let destination = absolute_destination(&destination)?;
                if current.as_ref() == Some(&destination) {
                    save_if_unchanged(
                        &project,
                        &destination,
                        saved
                            .as_deref()
                            .ok_or("Saved project has no loaded version")?,
                    )
                    .map_err(|e| e.to_string())?;
                } else {
                    save_new(&project, &destination).map_err(|e| e.to_string())?;
                }
                Ok(Outcome::Saved {
                    path: destination,
                    project,
                })
            },
        )?;
        self.cancel_recovery(id);
        let tab = self.tab_mut(id)?;
        tab.cancel_exports();
        tab.generation = generation;
        Ok(job)
    }

    /// Write an independent project copy without changing the opened document.
    /// `id` selects a tab, `expected` owns its revision; `destination` must be a new file.
    /// Returns a worker job ID. The copy receives a new project identity.
    pub fn save_copy(
        &mut self,
        id: Uuid,
        expected: DocumentVersion,
        destination: PathBuf,
    ) -> Result<Uuid, String> {
        let captured = self
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .ok_or("This document is no longer open")?
            .editor
            .snapshot();
        if DocumentVersion::of(&captured) != expected {
            return Err("Copy dialog belongs to an older document revision".into());
        }
        self.spawn(Pending::Copy, move || {
            let mut project = (*captured).clone();
            project.id = Uuid::new_v4();
            project.recovered_from = None;
            save_new(&project, &destination).map_err(|e| e.to_string())?;
            Ok(Outcome::Copy(destination))
        })
    }

    /// Archive a captured document with all verified media on a disk worker.
    /// `id` and `expected` own the selection; `destination` is a new folder.
    /// Returns a worker job ID and opens the portable copy after publication.
    pub fn archive_copy(
        &mut self,
        id: Uuid,
        expected: DocumentVersion,
        destination: PathBuf,
    ) -> Result<Uuid, String> {
        let (_, editor) = self.edit_snapshot(id)?;
        let captured = editor.snapshot();
        if DocumentVersion::of(&captured) != expected {
            return Err("Archive dialog belongs to an older document revision".into());
        }
        self.spawn(Pending::Open, move || {
            let path = editbay_core::archive_project(&captured, &destination)
                .map_err(|e| e.to_string())?;
            let project =
                load_bounded(&path, editbay_core::MAX_DOCUMENT_BYTES).map_err(|e| e.to_string())?;
            Ok(Outcome::Opened {
                path,
                editor: DocumentEditor::new(project).map_err(|e| e.to_string())?,
            })
        })
    }

    /// Recover an intact snapshot into a separate new file.
    /// `snapshot` identifies a verified recovery candidate; `destination` is
    /// a new copy. Returns a job ID and opens the result only after durable success.
    pub fn recover(&mut self, snapshot: PathBuf, destination: PathBuf) -> Result<Uuid, String> {
        self.spawn(Pending::Open, move || {
            let project = recover_copy(snapshot, &destination).map_err(|e| e.to_string())?;
            Ok(Outcome::Opened {
                path: absolute_destination(&destination)?,
                editor: DocumentEditor::new(project).map_err(|error| error.to_string())?,
            })
        })
    }

    /// Refresh checkpoint history without reading the directory in a UI frame.
    /// Takes no arguments and returns the scan job ID; invalid checkpoints are
    /// kept alongside older valid candidates in the returned catalog.
    pub fn refresh_recoveries(&mut self) -> Result<Uuid, String> {
        let root = self.recovery_root.clone();
        self.spawn(Pending::Recoveries, move || {
            Ok(Outcome::Recoveries(
                recovery_catalog(root).map_err(|e| e.to_string())?,
            ))
        })
    }

    /// Prepare an explicit checkpoint of the selected document.
    /// `id` selects the current revision. Returns a bounded worker job ID;
    /// publication still requires tab/path ownership when the worker completes.
    pub fn checkpoint_now(&mut self, id: Uuid) -> Result<Uuid, String> {
        if self
            .pending
            .values()
            .any(|job| matches!(job,Pending::Recovery{tab,..} if *tab==id))
        {
            return Err("A recovery snapshot is already being prepared".into());
        }
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .ok_or("This document is no longer open")?;
        let project = tab.editor.snapshot();
        let original = tab.path.clone();
        let root = self.recovery_root.clone();
        if self.pending.len() >= 4 {
            return Err("Finishing other disk work; try this action again shortly".into());
        }
        let job = Uuid::new_v4();
        let (publication, permit) = recovery::Publication::new();
        let pending = Pending::Recovery {
            tab: id,
            generation: tab.generation,
            version: DocumentVersion::of(&project),
            publication,
        };
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        std::thread::Builder::new()
            .name("editbay-recovery".into())
            .spawn(move || {
                let result = (|| {
                    let prepared = prepare_checkpoint(&project, original.as_deref(), root)
                        .map_err(|e| e.to_string())?;
                    permit.prepared();
                    sender
                        .send((job, Ok(Outcome::Prepared)))
                        .map_err(|e| e.to_string())?;
                    wake.request_repaint();
                    permit
                        .publish(|| {
                            let started = Instant::now();
                            prepared.commit().map_err(|e| e.to_string())?;
                            Ok(Outcome::Checkpoint(started.elapsed()))
                        })
                        .unwrap_or(Ok(Outcome::RecoveryCancelled))
                })();
                let _ = sender.send((job, result));
                wake.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        self.pending.insert(job, pending);
        Ok(job)
    }

    /// Close clean work or discard only after an explicit user choice.
    /// `id` selects the tab; `discard` records that choice. Returns a refusal
    /// for dirty work without approval or while an acknowledged save is pending.
    pub fn close(&mut self, id: Uuid, discard: bool) -> Result<(), String> {
        if self.saving(id) {
            return Err("Wait for this project's save to finish".into());
        }
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .ok_or("This document is no longer open")?;
        if tab.dirty() && !discard {
            return Err("Save or discard this project's changes before closing".into());
        }
        self.cancel_recovery(id);
        self.tab_mut(id)?.cancel_exports();
        self.tabs.retain(|tab| tab.id != id);
        if self.active == Some(id) {
            self.active = self.tabs.last().map(|tab| tab.id);
        }
        Ok(())
    }

    /// Inspect whether a manual save still owns this document.
    /// `id` selects a tab. Returns true until its filesystem result is accepted.
    pub fn saving(&self, id: Uuid) -> bool {
        self.pending
            .values()
            .any(|job| matches!(job,Pending::Save{tab,..} if *tab==id))
    }

    /// Consume bounded worker results and schedule due recovery for every tab.
    /// `now` supplies monotonic time. Returns after ownership checks and metadata
    /// acceptance; recovery publication and cleanup remain on their worker.
    /// Inactive/untitled work uses 1-second idle and 10-second maximum scheduling.
    pub fn poll(&mut self, now: Instant) {
        for _ in 0..8 {
            let Ok((id, result)) = self.receiver.try_recv() else {
                break;
            };
            let Some(pending) = self.pending.remove(&id) else {
                continue;
            };
            if matches!(result, Ok(Outcome::Prepared))
                && let Pending::Recovery {
                    tab,
                    generation,
                    version,
                    publication,
                } = &pending
            {
                if self.recovery_current(*tab, *generation, *version) {
                    if let Err(error) = publication.approve() {
                        publication.cancel();
                        self.error(error);
                    }
                } else {
                    publication.cancel();
                }
                self.pending.insert(id, pending);
                continue;
            }
            if let Err(error) = self.accept(pending, result, now) {
                self.error(error);
            }
        }
        while self.pending.len() < 4 {
            let Some(path) = self.open_queue.pop_front() else {
                break;
            };
            let result = self.spawn(Pending::Open, move || {
                if !fs::symlink_metadata(&path)
                    .map_err(|e| e.to_string())?
                    .is_file()
                {
                    return Err("Open requires a regular project, not a symlink".into());
                }
                let path = path.canonicalize().map_err(|e| e.to_string())?;
                let project = load_bounded(&path, 8 * 1024 * 1024).map_err(|e| e.to_string())?;
                Ok(Outcome::Opened {
                    path,
                    editor: DocumentEditor::new(project).map_err(|error| error.to_string())?,
                })
            });
            if let Err(error) = result {
                self.error(error);
                break;
            }
        }
        let due: Vec<_> = self
            .tabs
            .iter()
            .filter(|tab| {
                tab.dirty()
                    && tab.recovery_revision != Some(tab.editor.project().revision)
                    && now >= tab.retry_after
                    && (now.saturating_duration_since(tab.last_edit) >= Duration::from_secs(1)
                        || now.saturating_duration_since(tab.recovery_since)
                            >= Duration::from_millis(9_750))
                    && !self.saving(tab.id)
                    && !self
                        .pending
                        .values()
                        .any(|job| matches!(job,Pending::Recovery{tab:id,..} if *id==tab.id))
            })
            .map(|tab| tab.id)
            .collect();
        for id in due {
            if self.pending.len() >= 4 {
                break;
            }
            if let Err(error) = self.checkpoint_now(id) {
                self.error(error);
            }
        }
    }

    /// Inspect outstanding filesystem work for status and close guards.
    /// Takes no arguments and returns true while queued or running work exists.
    pub fn busy(&self) -> bool {
        !self.pending.is_empty() || !self.open_queue.is_empty()
    }

    /// Inspect the bounded recovery lifetimes without worker or filesystem waits.
    /// Takes no arguments; returns at most four captured owners and current phases.
    pub fn recovery_jobs(&self) -> Vec<RecoveryWork> {
        let mut jobs: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(job, pending)| match pending {
                Pending::Recovery {
                    tab,
                    version,
                    publication,
                    ..
                } => Some(RecoveryWork {
                    job: *job,
                    tab: *tab,
                    version: *version,
                    phase: publication.phase(),
                }),
                _ => None,
            })
            .collect();
        jobs.sort_by_key(|job| job.job);
        jobs
    }

    /// Consume a request to refresh filesystem discovery after publication.
    /// Takes no arguments; returns true once after an actual project save/copy.
    pub fn take_catalog_dirty(&mut self) -> bool {
        std::mem::take(&mut self.catalog_dirty)
    }

    fn tab_mut(&mut self, id: Uuid) -> Result<&mut Tab, String> {
        self.tabs
            .iter_mut()
            .find(|tab| tab.id == id)
            .ok_or_else(|| "This document is no longer open".into())
    }

    fn cancel_recovery(&self, id: Uuid) {
        for pending in self.pending.values() {
            if let Pending::Recovery {
                tab, publication, ..
            } = pending
                && *tab == id
            {
                publication.cancel();
            }
        }
    }

    fn recovery_current(&self, id: Uuid, generation: u64, version: DocumentVersion) -> bool {
        self.tabs.iter().any(|tab| {
            tab.id == id
                && tab.generation == generation
                && tab.editor.project().id == version.project_id
                && tab.editor.project().revision >= version.revision
                && !tab
                    .recovery_revision
                    .is_some_and(|revision| revision >= version.revision)
        })
    }

    fn error(&mut self, error: String) {
        self.errors.push_back(error);
        while self.errors.len() > 20 {
            self.errors.pop_front();
        }
    }

    fn spawn(
        &mut self,
        pending: Pending,
        work: impl FnOnce() -> Result<Outcome, String> + Send + 'static,
    ) -> Result<Uuid, String> {
        if self.pending.len() >= 4 {
            return Err("Finishing other disk work; try this action again shortly".into());
        }
        let id = Uuid::new_v4();
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        std::thread::Builder::new()
            .name("editbay-files".into())
            .spawn(move || {
                let _ = sender.send((id, work()));
                wake.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        self.pending.insert(id, pending);
        Ok(id)
    }

    fn accept(
        &mut self,
        pending: Pending,
        result: Result<Outcome, String>,
        now: Instant,
    ) -> Result<(), String> {
        match (pending, result) {
            (
                Pending::Recovery {
                    tab,
                    generation,
                    version,
                    ..
                },
                result,
            ) => {
                if !self.recovery_current(tab, generation, version) {
                    return Ok(());
                }
                let started = Instant::now();
                let tab = self.tabs.iter_mut().find(|t| t.id == tab).unwrap();
                match result {
                    Ok(Outcome::Checkpoint(elapsed)) => {
                        tab.recovery_revision = Some(version.revision);
                        tab.recovery_error = None;
                        tab.recovery_since = now;
                        self.recovery_publication_times.push(elapsed);
                        self.recovery_accept_times.push(started.elapsed());
                        if self.recovery_publication_times.len() > 1024 {
                            self.recovery_publication_times.remove(0);
                            self.recovery_accept_times.remove(0);
                        }
                    }
                    Ok(Outcome::RecoveryCancelled) => {}
                    Err(error) => {
                        tab.retry_after = now + Duration::from_secs(2);
                        tab.recovery_error = Some(error.clone());
                        return Err(error);
                    }
                    _ => return Err("Recovery worker returned the wrong result".into()),
                }
            }
            (Pending::Save { tab, generation }, Ok(Outcome::Saved { path, project })) => {
                let tab = self.tab_mut(tab)?;
                if tab.generation != generation || tab.editor.project().id != project.id {
                    return Err("Save result belongs to a replaced document".into());
                }
                tab.saved = Some(project);
                tab.path = Some(path);
                tab.recovery_revision = None;
                tab.recovery_since = now;
                self.catalog_dirty = true;
            }
            (Pending::Open, Ok(Outcome::Opened { path, editor })) => {
                self.activation_generation = self.activation_generation.wrapping_add(1);
                if editor.project().recovered_from.is_some() {
                    self.catalog_dirty = true;
                }
                if let Some(tab) = self
                    .tabs
                    .iter()
                    .find(|tab| tab.path.as_ref() == Some(&path))
                {
                    self.active = Some(tab.id);
                } else {
                    self.add(editor, Some(path))?;
                }
            }
            (Pending::Copy, Ok(Outcome::Copy(path))) => {
                self.last_saved_copy = Some(path);
                self.catalog_dirty = true;
            }
            (Pending::Recoveries, Ok(Outcome::Recoveries(catalog))) => self.recoveries = catalog,
            (_, Err(error)) => return Err(error),
            _ => return Err("Filesystem worker returned the wrong result".into()),
        }
        Ok(())
    }
}

fn absolute_destination(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    Ok(parent
        .canonicalize()
        .map_err(|e| e.to_string())?
        .join(path.file_name().ok_or("Destination needs a filename")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(workspace: &mut Workspace) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while workspace.busy() {
            assert!(Instant::now() < deadline);
            workspace.poll(Instant::now());
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn failed_publication_is_not_acknowledged_and_retry_preserves_older_history() {
        let directory = tempfile::tempdir().unwrap();
        let mut workspace = Workspace::new(directory.path().to_owned(), egui::Context::default());
        let tab = workspace.create(Project::new("Original").unwrap()).unwrap();
        workspace.checkpoint_now(tab).unwrap();
        settle(&mut workspace);
        let original = recovery_catalog(directory.path()).unwrap().valid.remove(0);
        let version = DocumentVersion::of(workspace.tabs[0].editor.project());
        workspace
            .apply(
                tab,
                version,
                "Rename".into(),
                &[DocumentCommand::RenameProject {
                    name: "Latest".into(),
                }],
            )
            .unwrap();
        let job = workspace.checkpoint_now(tab).unwrap();
        let (ready, result) = workspace
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(ready, job);
        assert!(matches!(result, Ok(Outcome::Prepared)));
        let folder = directory.path().join(version.project_id.to_string());
        let temporary = fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|extension| extension == "tmp"))
            .unwrap();
        fs::remove_file(temporary).unwrap();
        workspace.sender.send((ready, result)).unwrap();
        settle(&mut workspace);
        assert_eq!(workspace.tabs[0].recovery_revision, Some(0));
        assert!(workspace.tabs[0].recovery_error.is_some());
        assert!(!workspace.errors.is_empty());
        let history = recovery_catalog(directory.path()).unwrap();
        assert_eq!(history.valid.len(), 1);
        assert_eq!(history.valid[0].path, original.path);
        workspace.poll(Instant::now() + Duration::from_secs(3));
        settle(&mut workspace);
        assert_eq!(workspace.tabs[0].recovery_revision, Some(1));
        assert!(workspace.tabs[0].recovery_error.is_none());
        let history = recovery_catalog(directory.path()).unwrap();
        assert_eq!(history.valid.len(), 2);
        assert!(
            history
                .valid
                .iter()
                .any(|record| record.path == original.path)
        );
        assert!(
            history
                .valid
                .iter()
                .any(|record| record.revision == 1 && record.name == "Latest")
        );
    }

    #[test]
    fn awaiting_approval_keeps_worker_slots_bounded_and_close_releases_them() {
        let directory = tempfile::tempdir().unwrap();
        let mut workspace = Workspace::new(directory.path().to_owned(), egui::Context::default());
        let tabs: Vec<_> = (0..5)
            .map(|i| {
                workspace
                    .create(Project::new(format!("Job {i}")).unwrap())
                    .unwrap()
            })
            .collect();
        for &tab in &tabs[..4] {
            workspace.checkpoint_now(tab).unwrap();
        }
        let mut ready = Vec::new();
        for _ in 0..4 {
            let result = workspace
                .receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            assert!(matches!(&result.1, Ok(Outcome::Prepared)));
            ready.push(result);
        }
        assert!(workspace.checkpoint_now(tabs[4]).is_err());
        assert!(recovery_catalog(directory.path()).unwrap().valid.is_empty());
        for &tab in &tabs[..4] {
            workspace.close(tab, true).unwrap();
        }
        for result in ready {
            workspace.sender.send(result).unwrap();
        }
        settle(&mut workspace);
        assert!(workspace.errors.is_empty());
        assert!(recovery_catalog(directory.path()).unwrap().valid.is_empty());
        workspace.checkpoint_now(tabs[4]).unwrap();
        settle(&mut workspace);
        assert_eq!(recovery_catalog(directory.path()).unwrap().valid.len(), 1);
    }
}
