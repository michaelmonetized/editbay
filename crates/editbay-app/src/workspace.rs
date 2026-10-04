use editbay_core::{
    DocumentCommand, DocumentEditor, DocumentVersion, PreparedCheckpoint, Project, RecoveryCatalog,
    load_bounded, prepare_checkpoint, recover_copy, recovery_catalog, save_if_unchanged, save_new,
};
use eframe::egui;
use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};
use uuid::Uuid;

pub struct Tab {
    pub id: Uuid,
    pub editor: DocumentEditor,
    pub path: Option<PathBuf>,
    pub saved: Option<Project>,
    pub recovery_revision: Option<u64>,
    pub recovery_error: Option<String>,
    generation: u64,
    last_edit: Instant,
    recovery_since: Instant,
    retry_after: Instant,
}

impl Tab {
    /// Determine whether this document still needs a manual save.
    /// Takes no arguments; returns true for untitled or changed documents,
    /// independently of whether a recovery snapshot already exists.
    pub fn dirty(&self) -> bool {
        self.saved.as_ref() != Some(self.editor.project())
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
    },
    Copy,
    Recoveries,
}

enum Outcome {
    Opened { path: PathBuf, project: Project },
    Saved { path: PathBuf, project: Project },
    Prepared(PreparedCheckpoint),
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
    pub recovery_commit_times: Vec<Duration>,
    recovery_root: PathBuf,
    pending: HashMap<Uuid, Pending>,
    open_queue: VecDeque<PathBuf>,
    sender: Sender<(Uuid, Result<Outcome, String>)>,
    receiver: Receiver<(Uuid, Result<Outcome, String>)>,
    wake: egui::Context,
    catalog_dirty: bool,
}

impl Workspace {
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
            recovery_commit_times: Vec::new(),
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
        self.add(project, None)
    }

    fn add(&mut self, project: Project, path: Option<PathBuf>) -> Result<Uuid, String> {
        if self.tabs.len() >= 128 {
            return Err(
                "The workspace supports 128 open projects; close a tab to open another".into(),
            );
        }
        let now = Instant::now();
        let id = Uuid::new_v4();
        let saved = path.as_ref().map(|_| project.clone());
        self.tabs.push(Tab {
            id,
            editor: DocumentEditor::new(project).map_err(|e| e.to_string())?,
            path,
            saved,
            recovery_revision: None,
            recovery_error: None,
            generation: 0,
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
        let receipt = tab
            .editor
            .apply(expected, label, commands)
            .map_err(|e| e.to_string())?;
        if receipt.changed {
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
        if redo {
            tab.editor.redo(expected)
        } else {
            tab.editor.undo(expected)
        }
        .map_err(|e| e.to_string())?;
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
        let project = tab.editor.project().clone();
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
                            .as_ref()
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
        self.tab_mut(id)?.generation = generation;
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
        let mut project = self
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .ok_or("This document is no longer open")?
            .editor
            .project()
            .clone();
        if DocumentVersion::of(&project) != expected {
            return Err("Copy dialog belongs to an older document revision".into());
        }
        project.id = Uuid::new_v4();
        project.recovered_from = None;
        self.spawn(Pending::Copy, move || {
            save_new(&project, &destination).map_err(|e| e.to_string())?;
            Ok(Outcome::Copy(destination))
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
                project,
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
        let project = tab.editor.project().clone();
        let original = tab.path.clone();
        let root = self.recovery_root.clone();
        self.spawn(
            Pending::Recovery {
                tab: id,
                generation: tab.generation,
                version: DocumentVersion::of(&project),
            },
            move || {
                Ok(Outcome::Prepared(
                    prepare_checkpoint(&project, original.as_deref(), root)
                        .map_err(|e| e.to_string())?,
                ))
            },
        )
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
    /// `now` supplies monotonic time. Returns immediately except the short
    /// publication of already synchronized recovery bytes after ownership checks.
    /// Inactive/untitled work uses 1-second idle and 10-second maximum scheduling.
    pub fn poll(&mut self, now: Instant) {
        for _ in 0..8 {
            let Ok((id, result)) = self.receiver.try_recv() else {
                break;
            };
            let Some(pending) = self.pending.remove(&id) else {
                continue;
            };
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
                Ok(Outcome::Opened { path, project })
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
            let tab = self.tabs.iter().find(|tab| tab.id == id).unwrap();
            let project = tab.editor.project().clone();
            let original = tab.path.clone();
            let root = self.recovery_root.clone();
            let job = Pending::Recovery {
                tab: id,
                generation: tab.generation,
                version: DocumentVersion::of(&project),
            };
            if let Err(error) = self.spawn(job, move || {
                Ok(Outcome::Prepared(
                    prepare_checkpoint(&project, original.as_deref(), root)
                        .map_err(|e| e.to_string())?,
                ))
            }) {
                self.error(error);
            }
        }
    }

    /// Inspect outstanding filesystem work for status and close guards.
    /// Takes no arguments and returns true while queued or running work exists.
    pub fn busy(&self) -> bool {
        !self.pending.is_empty() || !self.open_queue.is_empty()
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
                },
                result,
            ) => {
                let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab) else {
                    return Ok(());
                };
                if tab.generation != generation
                    || tab.editor.project().id != version.project_id
                    || tab.editor.project().revision < version.revision
                    || tab
                        .recovery_revision
                        .is_some_and(|revision| revision >= version.revision)
                {
                    return Ok(());
                }
                match result {
                    Ok(Outcome::Prepared(prepared)) => {
                        let started = Instant::now();
                        let result = prepared.commit();
                        self.recovery_commit_times.push(started.elapsed());
                        if self.recovery_commit_times.len() > 1024 {
                            self.recovery_commit_times.remove(0);
                        }
                        match result {
                            Ok(_) => {
                                tab.recovery_revision = Some(version.revision);
                                tab.recovery_error = None;
                                tab.recovery_since = now;
                            }
                            Err(error) => {
                                tab.retry_after = now + Duration::from_secs(2);
                                tab.recovery_error = Some(error.to_string());
                                return Err(error.to_string());
                            }
                        }
                    }
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
            (Pending::Open, Ok(Outcome::Opened { path, project })) => {
                self.activation_generation = self.activation_generation.wrapping_add(1);
                if project.recovered_from.is_some() {
                    self.catalog_dirty = true;
                }
                if let Some(tab) = self
                    .tabs
                    .iter()
                    .find(|tab| tab.path.as_ref() == Some(&path))
                {
                    self.active = Some(tab.id);
                } else {
                    self.add(project, Some(path))?;
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
