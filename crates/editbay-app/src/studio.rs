use crate::{
    brand::{Asset, BankVersion, Category},
    brand_ui::{Action as BankAction, BankPane, Task as BankTask},
    catalog::{CatalogEvent, CatalogScan, DocumentEntry, ProjectFolder},
    delivery_ui::DeliveryPane,
    diagnostics::Diagnostics,
    media_ui::MediaPane,
    preferences::{PreferenceStore, Preferences, Startup},
    preview::PreviewPane,
    theme::LiveTheme,
    timeline_ui::TimelinePane,
    workspace::{DocumentOwner, Workspace},
};
use editbay_core::{DocumentCommand, DocumentVersion, Project};
use eframe::egui::{self, Align2, FontFamily, FontId, RichText, Sense, Stroke, Ui, vec2};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

enum DialogAction {
    Open,
    Delivery {
        owner: DocumentOwner,
        request: editbay_delivery::DeliveryRequest,
    },
    ImportMedia {
        owner: DocumentOwner,
    },
    CatalogRoot,
    BrandFolder {
        create: bool,
    },
    BrandImport {
        root: PathBuf,
        version: BankVersion,
        category: Category,
    },
    BrandExport {
        root: PathBuf,
        asset: Asset,
    },
    Save {
        tab: Uuid,
        expected: DocumentVersion,
        copy: bool,
    },
    Archive {
        tab: Uuid,
        expected: DocumentVersion,
    },
    Recover {
        snapshot: PathBuf,
    },
}

struct DialogResult {
    action: DialogAction,
    paths: Vec<PathBuf>,
    error: Option<String>,
}

pub struct Studio {
    pub workspace: Workspace,
    theme: LiveTheme,
    home: PathBuf,
    root: PathBuf,
    scan: Option<CatalogScan>,
    documents: Vec<DocumentEntry>,
    projects: Vec<ProjectFolder>,
    project_counts: HashMap<PathBuf, usize>,
    scan_errors: Vec<String>,
    scan_complete: Option<bool>,
    visited: usize,
    project_scope: Option<PathBuf>,
    selection: HashSet<PathBuf>,
    query: String,
    portrait: Option<bool>,
    welcome: bool,
    recovered: bool,
    dialog: Option<Receiver<DialogResult>>,
    dialog_template: rfd::AsyncFileDialog,
    dialog_runtime: Result<Arc<tokio::runtime::Runtime>, String>,
    preferences: PreferenceStore,
    settings: Option<Preferences>,
    bank: BankPane,
    media: MediaPane,
    preview: PreviewPane,
    delivery: DeliveryPane,
    timeline: TimelinePane,
    preferences_applied: bool,
    explicit_paths: bool,
    restoring: bool,
    remembered_workspace: (Vec<(Uuid, Option<PathBuf>)>, Option<Uuid>),
    focus_name: bool,
    name_events: Vec<egui::Event>,
    create_name: Option<String>,
    new_profile: [u32; 4],
    rename: Option<(Uuid, DocumentVersion, String)>,
    close_guard: Option<Uuid>,
    close_after_save: bool,
    quit: bool,
    recovery_preview: Option<PathBuf>,
    manual: bool,
    message: Option<String>,
    diagnostics: Option<Diagnostics>,
    frame_started: Instant,
}

impl Studio {
    /// Create the native offline front door and its worker-owned workspace.
    /// `ctx` receives wakeups, `home` supplies desktop settings, `root` scopes
    /// local discovery, `state` owns EditBay recovery, and `paths` are explicit opens.
    /// Returns immediately with background discovery and recovery scans running.
    pub fn new(
        ctx: egui::Context,
        home: PathBuf,
        root: PathBuf,
        state: PathBuf,
        paths: Vec<PathBuf>,
        dialog_template: rfd::AsyncFileDialog,
    ) -> Self {
        let theme = LiveTheme::new(home.clone(), &ctx);
        let preferences = PreferenceStore::new(state.join("workspace.json"), ctx.clone());
        let explicit_paths = !paths.is_empty();
        let mut workspace = Workspace::new(state.join("recovery"), ctx.clone());
        workspace.open(paths);
        let mut message = workspace.refresh_recoveries().err();
        let dialog_runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("editbay-portal")
            .enable_all()
            .build()
            .map(Arc::new)
            .map_err(|error| error.to_string());
        if let Err(error) = &dialog_runtime {
            message = Some(error.clone());
        }
        let diagnostics = std::env::var_os("EDITBAY_DIAGNOSTICS_PATH").and_then(|path| {
            match Diagnostics::new(path.into(), ctx.clone()) {
                Ok(diagnostics) => Some(diagnostics),
                Err(error) => {
                    message = Some(error);
                    None
                }
            }
        });
        let (scan, error) = match CatalogScan::start(root.clone()) {
            Ok(scan) => (Some(scan), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            workspace,
            theme,
            home,
            root,
            scan,
            documents: Vec::new(),
            projects: Vec::new(),
            project_counts: HashMap::new(),
            scan_errors: error.into_iter().collect(),
            scan_complete: None,
            visited: 0,
            project_scope: None,
            selection: HashSet::new(),
            query: String::new(),
            portrait: None,
            welcome: true,
            recovered: false,
            dialog: None,
            dialog_template,
            dialog_runtime,
            preferences,
            settings: None,
            bank: BankPane::default(),
            media: MediaPane::default(),
            preview: PreviewPane::new(state.join("picture-cache")),
            delivery: DeliveryPane::default(),
            timeline: TimelinePane::default(),
            preferences_applied: false,
            explicit_paths,
            restoring: false,
            remembered_workspace: (Vec::new(), None),
            focus_name: false,
            name_events: Vec::new(),
            create_name: None,
            new_profile: [1920, 1080, 24, 1],
            rename: None,
            close_guard: None,
            close_after_save: false,
            quit: false,
            recovery_preview: None,
            manual: false,
            message,
            diagnostics,
            frame_started: Instant::now(),
        }
    }

    /// Attach the native window's shared GPU without doing input-thread work.
    /// `state` is eframe's actual device and surface. Returns no value.
    pub fn attach_gpu(&mut self, state: &egui_wgpu::RenderState) {
        self.preview.attach(state);
    }

    fn tick(&mut self, ctx: &egui::Context) {
        if let Some(diagnostics) = &self.diagnostics
            && let Some(error) = diagnostics.error()
        {
            self.message = Some(error);
        }
        self.theme.poll(ctx);
        self.bank.poll();
        if self.preferences.poll() {
            if let Some(root) = &self.preferences.current.catalog_root {
                self.root = root.clone();
                self.rescan();
            }
            if !self.preferences_applied
                && !self.explicit_paths
                && self.preferences.current.startup == Startup::RestoreWorkspace
            {
                self.workspace
                    .open(self.preferences.current.open_paths.clone());
                self.restoring = true;
            }
            self.preferences_applied = true;
        }
        let previous = self.workspace.activation_generation;
        self.workspace.poll(Instant::now());
        self.media.poll(&mut self.workspace, ctx);
        self.delivery.poll(&self.workspace, ctx);
        if self.workspace.activation_generation != previous && self.workspace.active.is_some() {
            self.welcome = false;
        }
        self.preview.poll(&mut self.workspace, !self.welcome, ctx);
        self.timeline
            .poll(&mut self.workspace, &mut self.preview, !self.welcome, ctx);
        if self.workspace.take_catalog_dirty() {
            self.rescan();
        }
        if self.restoring && !self.workspace.busy() {
            if let Some(path) = &self.preferences.current.active_path
                && let Some(tab) = self
                    .workspace
                    .tabs
                    .iter()
                    .find(|tab| tab.path.as_ref() == Some(path))
            {
                self.workspace.active = Some(tab.id);
            }
            self.restoring = false;
            self.remembered_workspace = self.workspace_signature();
        }
        if self.preferences.ready && !self.restoring {
            let signature = self.workspace_signature();
            if signature != self.remembered_workspace {
                self.remembered_workspace = signature;
                let mut preferences = self.preferences.current.clone();
                preferences.open_paths = self
                    .workspace
                    .tabs
                    .iter()
                    .filter_map(|tab| tab.path.clone())
                    .collect();
                preferences.active_path = self
                    .workspace
                    .tabs
                    .iter()
                    .find(|tab| Some(tab.id) == self.workspace.active)
                    .and_then(|tab| tab.path.clone());
                if let Err(error) = self.preferences.update(preferences) {
                    self.message = Some(error);
                }
            }
        }
        if let Some(error) = self.workspace.errors.pop_front() {
            self.message = Some(error);
        }
        if let Some(receiver) = &self.dialog {
            match receiver.try_recv() {
                Ok(result) => {
                    self.dialog = None;
                    if let Some(error) = result.error {
                        self.message = Some(error);
                        self.close_after_save = false;
                    }
                    if let Some(path) = result.paths.first() {
                        let outcome = match result.action {
                            DialogAction::Open => {
                                self.workspace.open(result.paths);
                                Ok(())
                            }
                            DialogAction::Delivery { owner, request } => self.delivery.start(
                                &mut self.workspace,
                                owner,
                                request,
                                path.clone(),
                                ctx,
                            ),
                            DialogAction::ImportMedia { owner } => {
                                if self.workspace.owns(owner) {
                                    std::env::current_exe().map_err(|e| e.to_string()).and_then(
                                        |executable| {
                                            self.media.start(
                                                &self.workspace,
                                                owner.tab,
                                                path.clone(),
                                                &executable,
                                                ctx,
                                            )
                                        },
                                    )
                                } else {
                                    Err("The project changed while choosing media; choose again"
                                        .into())
                                }
                            }
                            DialogAction::CatalogRoot => {
                                let mut preferences = self.preferences.current.clone();
                                preferences.catalog_root = Some(path.clone());
                                self.preferences.update(preferences).map(|()| {
                                    self.root = path.clone();
                                    self.project_scope = None;
                                    self.rescan();
                                    if let Some(settings) = &mut self.settings {
                                        settings.catalog_root = Some(path.clone());
                                    }
                                })
                            }
                            DialogAction::BrandFolder { create } => self.bank.run(
                                if create {
                                    BankTask::Create(path.clone())
                                } else {
                                    BankTask::Load(path.clone())
                                },
                                ctx,
                            ),
                            DialogAction::BrandImport {
                                root,
                                version,
                                category,
                            } => {
                                if self.bank.owns(&root, &version) {
                                    self.bank.run(
                                        BankTask::Import {
                                            root,
                                            version,
                                            source: path.clone(),
                                            category,
                                        },
                                        ctx,
                                    )
                                } else {
                                    Err("The brand bank changed while choosing an asset; choose again".into())
                                }
                            }
                            DialogAction::BrandExport { root, asset } => self.bank.run(
                                BankTask::Export {
                                    root,
                                    asset,
                                    destination: path.clone(),
                                },
                                ctx,
                            ),
                            DialogAction::Save {
                                tab,
                                expected,
                                copy,
                            } => {
                                if copy {
                                    self.workspace
                                        .save_copy(tab, expected, path.clone())
                                        .map(|_| ())
                                } else {
                                    self.workspace.save(tab, expected, path.clone()).map(|_| ())
                                }
                            }
                            DialogAction::Recover { snapshot } => {
                                self.workspace.recover(snapshot, path.clone()).map(|_| ())
                            }
                            DialogAction::Archive { tab, expected } => self
                                .workspace
                                .archive_copy(tab, expected, path.clone())
                                .map(|_| ()),
                        };
                        if let Err(error) = outcome {
                            self.message = Some(error);
                            self.close_after_save = false;
                        }
                    } else {
                        self.close_after_save = false;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.dialog = None;
                    self.message = Some("File chooser stopped unexpectedly".into());
                    self.close_after_save = false;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(scan) = &self.scan {
            let mut changed = false;
            for _ in 0..64 {
                match scan.events.try_recv() {
                    Ok(CatalogEvent::Document(document)) => {
                        if let Some(parent) = document.path.parent() {
                            for folder in parent.ancestors() {
                                *self.project_counts.entry(folder.to_path_buf()).or_default() += 1;
                            }
                        }
                        self.documents.push(document);
                        changed = true;
                    }
                    Ok(CatalogEvent::Folder(project)) => {
                        self.projects.push(project);
                        changed = true;
                    }
                    Ok(CatalogEvent::Warning { path, error }) => self
                        .scan_errors
                        .push(format!("{}: {error}", path.display())),
                    Ok(CatalogEvent::Finished { visited, complete }) => {
                        self.visited = visited;
                        self.scan_complete = Some(complete);
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if self.scan_complete.is_none() {
                            self.scan_errors
                                .push("Catalog worker stopped before completing".into());
                            self.scan_complete = Some(false);
                        }
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            if changed {
                self.documents.sort_by(|a, b| {
                    b.modified
                        .cmp(&a.modified)
                        .then_with(|| a.path.cmp(&b.path))
                });
                self.projects.sort_by(|a, b| {
                    b.modified
                        .cmp(&a.modified)
                        .then_with(|| a.path.cmp(&b.path))
                });
            }
        }
        if self.scan_complete.is_some() {
            self.scan = None;
        }
        if self.close_after_save
            && self.dialog.is_none()
            && let Some(id) = self.close_guard
            && !self.workspace.saving(id)
        {
            self.close_after_save = false;
            if self
                .workspace
                .tabs
                .iter()
                .find(|tab| tab.id == id)
                .is_some_and(|tab| !tab.dirty())
            {
                self.finish_close(id, ctx);
            }
        }
        if self.workspace.busy()
            || self.scan.is_some()
            || self.dialog.is_some()
            || self.preferences.busy()
            || self.bank.busy()
        {
            ctx.request_repaint_after(Duration::from_millis(20));
        } else if self.workspace.tabs.iter().any(|tab| tab.dirty()) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn workspace_signature(&self) -> (Vec<(Uuid, Option<PathBuf>)>, Option<Uuid>) {
        (
            self.workspace
                .tabs
                .iter()
                .map(|tab| (tab.id, tab.path.clone()))
                .collect(),
            self.workspace.active,
        )
    }

    fn choose(&mut self, action: DialogAction, name: &str, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        let original = match &action {
            DialogAction::Delivery { owner, .. } => self
                .workspace
                .tabs
                .iter()
                .find(|tab| tab.id == owner.tab)
                .and_then(|tab| tab.path.as_ref()),
            DialogAction::Save { tab, .. } | DialogAction::Archive { tab, .. } => self
                .workspace
                .tabs
                .iter()
                .find(|item| item.id == *tab)
                .and_then(|item| item.path.as_ref()),
            DialogAction::Recover { snapshot } => self
                .workspace
                .recoveries
                .valid
                .iter()
                .find(|item| item.path == *snapshot)
                .and_then(|item| item.original.as_ref()),
            _ => None,
        };
        let folder = original
            .and_then(|path| path.parent())
            .map(PathBuf::from)
            .or_else(|| self.project_scope.clone())
            .unwrap_or_else(|| self.home.clone());
        let name = if matches!(action, DialogAction::BrandExport { .. }) {
            name.to_owned()
        } else if let DialogAction::Delivery { request, .. } = &action {
            PathBuf::from(file_name(name))
                .with_extension(request.format.extension())
                .to_string_lossy()
                .into_owned()
        } else {
            file_name(name)
        };
        let template = self.dialog_template.clone();
        let runtime = match &self.dialog_runtime {
            Ok(runtime) => runtime.clone(),
            Err(error) => {
                self.message = Some(error.clone());
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        let wake = ctx.clone();
        match std::thread::Builder::new()
            .name("editbay-dialog".into())
            .spawn(move || {
                let paths = runtime.block_on(async {
                    let dialog = template.set_directory(folder);
                    if matches!(action, DialogAction::Archive { .. }) {
                        let stem = PathBuf::from(&name).with_extension("");
                        return dialog
                            .set_title("New portable archive folder")
                            .set_file_name(format!("{}-archive", stem.display()))
                            .save_file()
                            .await
                            .into_iter()
                            .map(|file| file.path().to_path_buf())
                            .collect();
                    }
                    if let DialogAction::Delivery { request, .. } = &action {
                        return dialog
                            .set_title("Export sequence")
                            .add_filter("Video delivery", &[request.format.extension()])
                            .set_file_name(name)
                            .save_file()
                            .await
                            .into_iter()
                            .map(|file| file.path().to_path_buf())
                            .collect();
                    }
                    if matches!(
                        action,
                        DialogAction::CatalogRoot | DialogAction::BrandFolder { .. }
                    ) {
                        return dialog
                            .set_title("Choose EditBay folder")
                            .pick_folder()
                            .await
                            .into_iter()
                            .map(|file| file.path().to_path_buf())
                            .collect();
                    }
                    if matches!(action, DialogAction::BrandImport { .. }) {
                        return dialog
                            .set_title("Add brand asset")
                            .pick_file()
                            .await
                            .into_iter()
                            .map(|file| file.path().to_path_buf())
                            .collect();
                    }
                    if matches!(action, DialogAction::ImportMedia { .. }) {
                        return dialog
                            .set_title("Import source media")
                            .pick_file()
                            .await
                            .into_iter()
                            .map(|file| file.path().to_path_buf())
                            .collect();
                    }
                    if matches!(action, DialogAction::BrandExport { .. }) {
                        return dialog
                            .set_title("Export brand asset copy")
                            .set_file_name(name)
                            .save_file()
                            .await
                            .into_iter()
                            .map(|file| file.path().to_path_buf())
                            .collect();
                    }
                    let dialog = dialog.add_filter("EditBay project", &["editbay"]);
                    if matches!(action, DialogAction::Open) {
                        dialog
                            .set_title("Open EditBay projects")
                            .pick_files()
                            .await
                            .unwrap_or_default()
                            .into_iter()
                            .map(|file| file.path().to_path_buf())
                            .collect()
                    } else {
                        dialog
                            .set_title("Save EditBay project")
                            .set_file_name(name)
                            .save_file()
                            .await
                            .into_iter()
                            .map(|file| file.path().to_path_buf())
                            .collect()
                    }
                });
                let _ = sender.send(DialogResult {
                    action,
                    paths,
                    error: None,
                });
                wake.request_repaint();
            }) {
            Ok(_) => self.dialog = Some(receiver),
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn save_tab(&mut self, id: Uuid, save_as: bool, copy: bool, ctx: &egui::Context) {
        let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let version = DocumentVersion::of(tab.editor.project());
        if !save_as
            && !copy
            && let Some(path) = &tab.path
        {
            if let Err(error) = self.workspace.save(id, version, path.clone()) {
                self.message = Some(error);
                self.close_after_save = false;
            }
        } else {
            let name = tab.editor.project().name.clone();
            self.choose(
                DialogAction::Save {
                    tab: id,
                    expected: version,
                    copy,
                },
                &name,
                ctx,
            );
        }
    }

    fn request_close(&mut self, id: Uuid, ctx: &egui::Context) {
        if self
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .is_some_and(|tab| tab.dirty())
        {
            self.close_guard = Some(id);
        } else {
            self.finish_close(id, ctx);
        }
    }

    fn finish_close(&mut self, id: Uuid, ctx: &egui::Context) {
        if let Err(error) = self.workspace.close(id, false) {
            self.message = Some(error);
            return;
        }
        self.close_guard = None;
        if self.quit {
            if let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.dirty()) {
                self.close_guard = Some(tab.id);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if self.workspace.tabs.is_empty() {
            self.welcome = true;
        }
    }

    fn rescan(&mut self) {
        self.scan = None;
        self.documents.clear();
        self.projects.clear();
        self.project_counts.clear();
        self.scan_errors.clear();
        self.scan_complete = None;
        match CatalogScan::start(self.root.clone()) {
            Ok(scan) => self.scan = Some(scan),
            Err(error) => {
                self.scan_errors.push(error.to_string());
                self.scan_complete = Some(false);
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let history_keys = &self.preferences.current.shortcuts;
        let history_events: Vec<_> = ctx.input(|input| input.events.iter().filter_map(|event| {
            if let egui::Event::Key { key, pressed: true, modifiers, .. } = event
                && [history_keys.undo, history_keys.redo].iter().any(|letter| egui::Key::from_name(&letter.to_string()) == Some(*key))
            {
                Some(serde_json::json!({"key":key.name(),"ctrl":modifiers.ctrl,"command":modifiers.command,"shift":modifiers.shift,"mac_cmd":modifiers.mac_cmd,"alt":modifiers.alt}))
            } else { None }
        }).collect());
        if !history_events.is_empty()
            && let Some(diagnostics) = &mut self.diagnostics
        {
            diagnostics.record("history_shortcut", serde_json::json!({"events":history_events,"text_focused":ctx.text_edit_focused(),"focus":ctx.memory(|memory|memory.focused().map(|id|format!("{id:?}"))),"create":self.create_name.is_some(),"rename":self.rename.is_some(),"close_guard":self.close_guard.is_some(),"dialog":self.dialog.is_some(),"settings":self.settings.is_some(),"manual":self.manual,"bank":self.bank.opened}));
        }
        if self.create_name.is_some()
            || self.rename.is_some()
            || self.close_guard.is_some()
            || self.dialog.is_some()
            || self.manual
            || self.settings.is_some()
            || self.bank.opened
        {
            return;
        }
        let pressed = |key, shift| {
            ctx.input_mut(|input| {
                input.consume_key(
                    egui::Modifiers {
                        command: true,
                        shift,
                        ..Default::default()
                    },
                    key,
                )
            })
        };
        let shortcuts = self.preferences.current.shortcuts.clone();
        let key =
            |letter: char| egui::Key::from_name(&letter.to_string()).expect("validated shortcut");
        if pressed(key(shortcuts.new_project), false) {
            self.create_name = Some("Untitled project".into());
            self.focus_name = true;
        }
        if pressed(key(shortcuts.open), false) {
            self.choose(DialogAction::Open, "", ctx);
        }
        if let Some(id) = self.workspace.active {
            if pressed(key(shortcuts.save), true) {
                self.save_tab(id, true, false, ctx);
            } else if pressed(key(shortcuts.save), false) {
                self.save_tab(id, false, false, ctx);
            }
            if !ctx.text_edit_focused() && pressed(key(shortcuts.redo), shortcuts.redo_shift) {
                if let Err(error) = self.workspace.history(id, true) {
                    self.message = Some(error);
                }
            } else if !ctx.text_edit_focused()
                && pressed(key(shortcuts.undo), false)
                && let Err(error) = self.workspace.history(id, false)
            {
                self.message = Some(error);
            }
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let compact = ui.available_width() < 1100.;
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(self.welcome, "EditBay").clicked() {
                self.welcome = true;
            }
            ui.separator();
            if ui.button("New").clicked() {
                self.create_name = Some("Untitled project".into());
                self.focus_name = true;
            }
            if ui
                .add_enabled(self.dialog.is_none(), egui::Button::new("Open…"))
                .clicked()
            {
                self.choose(DialogAction::Open, "", &ctx);
            }
            if let Some(id) = self.workspace.active {
                if ui
                    .add_enabled(!self.workspace.saving(id), egui::Button::new("Save"))
                    .clicked()
                {
                    self.save_tab(id, false, false, &ctx);
                }
                ui.menu_button("File", |ui| {
                    if ui.button("Save as…").clicked() {
                        self.save_tab(id, true, false, &ctx);
                        ui.close();
                    }
                    if ui.button("Save a copy…").clicked() {
                        self.save_tab(id, true, true, &ctx);
                        ui.close();
                    }
                    if ui.button("Archive with media…").clicked() {
                        if let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.id == id) {
                            self.choose(
                                DialogAction::Archive {
                                    tab: id,
                                    expected: DocumentVersion::of(tab.editor.project()),
                                },
                                &tab.editor.project().name.clone(),
                                &ctx,
                            );
                        }
                        ui.close();
                    }
                    if ui.button("Close project").clicked() {
                        self.request_close(id, &ctx);
                        ui.close();
                    }
                });
                if let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.id == id) {
                    let (undo, redo) = tab.editor.history();
                    let undo_enabled = undo.is_some();
                    let redo_enabled = redo.is_some();
                    if ui
                        .add_enabled(undo_enabled, egui::Button::new("Undo"))
                        .clicked()
                        && let Err(error) = self.workspace.history(id, false)
                    {
                        self.message = Some(error);
                    }
                    if ui
                        .add_enabled(redo_enabled, egui::Button::new("Redo"))
                        .clicked()
                        && let Err(error) = self.workspace.history(id, true)
                    {
                        self.message = Some(error);
                    }
                }
                let import = ui.add_enabled(
                    self.dialog.is_none() && !self.media.busy(),
                    egui::Button::new("Import media…"),
                );
                self.preview.observe_control("import-media", &import, ui);
                if import.clicked() {
                    match self.workspace.edit_snapshot(id) {
                        Ok((owner, _)) => {
                            self.choose(DialogAction::ImportMedia { owner }, "", &ctx)
                        }
                        Err(error) => self.message = Some(error),
                    }
                }
                if self.media.busy() && ui.button("Cancel import").clicked() {
                    self.media.cancel();
                }
                let selected = self
                    .workspace
                    .edit_snapshot(id)
                    .ok()
                    .and_then(|(owner, editor)| {
                        self.preview
                            .selected_composition(id, editor.project())
                            .and_then(|composition| {
                                editor
                                    .project()
                                    .compositions
                                    .iter()
                                    .find(|scene| scene.id == composition)
                                    .map(|scene| {
                                        (
                                            owner,
                                            composition,
                                            editor.project().name.clone(),
                                            scene.duration,
                                        )
                                    })
                            })
                    });
                let export = ui
                    .add_enabled(
                        self.dialog.is_none() && selected.is_some(),
                        egui::Button::new("Export…"),
                    )
                    .on_hover_text(
                        "Full sequence or frame range · lossless MOV · original sound channels",
                    );
                self.preview.observe_control("export-sequence", &export, ui);
                if export.clicked()
                    && let Some((owner, composition, name, duration)) = selected
                {
                    self.delivery.choose(owner, composition, name, duration);
                }
                self.delivery.activity_button(ui);
            }
            if !compact {
                self.workspace_tools(ui, &ctx);
            }
        });
        if compact {
            ui.horizontal(|ui| self.workspace_tools(ui, &ctx));
        }
        ui.add_space(6.);
        let tabs: Vec<_> = self
            .workspace
            .tabs
            .iter()
            .map(|tab| (tab.id, tab.editor.project().name.clone(), tab.dirty()))
            .collect();
        egui::ScrollArea::horizontal()
            .id_salt("tabs")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (id, name, dirty) in tabs {
                        let label = format!("{}{}", name, if dirty { " •" } else { "" });
                        if ui
                            .selectable_label(
                                !self.welcome && self.workspace.active == Some(id),
                                label,
                            )
                            .clicked()
                        {
                            self.workspace.active = Some(id);
                            self.welcome = false;
                        }
                        if icon(ui, "\u{E4F6}", "Close project").clicked() {
                            self.request_close(id, &ctx);
                        }
                    }
                })
            });
        ui.separator();
    }

    fn workspace_tools(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(env!("CARGO_PKG_VERSION")).small().weak());
            if self.workspace.busy() || self.dialog.is_some() || self.media.working() {
                ui.spinner();
            }
            if ui.button("Help").clicked() {
                self.manual = true;
            }
            if ui.button("Settings").clicked() {
                self.settings = Some(self.preferences.current.clone());
            }
            if ui.button("Brand bank").clicked() {
                self.bank.opened = true;
                if self.bank.catalog.is_none() && !self.bank.busy() {
                    let folder = self.project_scope.clone().or_else(|| {
                        self.workspace
                            .tabs
                            .iter()
                            .find(|tab| Some(tab.id) == self.workspace.active)
                            .and_then(|tab| tab.path.as_ref())
                            .and_then(|path| path.parent())
                            .map(|path| path.to_path_buf())
                    });
                    if let Some(folder) = folder
                        && let Err(error) = self.bank.run(BankTask::Discover(folder), ctx)
                    {
                        self.bank.message = Some(error);
                    }
                }
            }
        });
    }

    fn welcome(&mut self, ui: &mut Ui) {
        let width = ui.available_width();
        if width >= 1000. {
            ui.horizontal_top(|ui| {
                let side = (width - 240. - 32.) / 2.;
                ui.allocate_ui_with_layout(
                    vec2(side, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.work_browser(ui),
                );
                ui.add_space(16.);
                ui.allocate_ui_with_layout(
                    vec2(240., ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.creation(ui),
                );
                ui.add_space(16.);
                ui.allocate_ui_with_layout(
                    vec2(side, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.project_browser(ui),
                );
            });
        } else {
            self.creation(ui);
            ui.add_space(12.);
            ui.columns(2, |columns| {
                self.work_browser(&mut columns[0]);
                self.project_browser(&mut columns[1]);
            });
        }
    }

    fn creation(&mut self, ui: &mut Ui) {
        ui.add_space(24.);
        ui.heading("Make room for your work.");
        ui.label(RichText::new("Your projects. Your machine.").weak());
        ui.add_space(20.);
        if ui
            .add_sized(
                [ui.available_width(), 38.],
                egui::Button::new("New project"),
            )
            .clicked()
        {
            self.create_name = Some("Untitled project".into());
            self.focus_name = true;
        }
        if ui
            .add_enabled(
                self.dialog.is_none(),
                egui::Button::new("Open existing work…").min_size(vec2(ui.available_width(), 34.)),
            )
            .clicked()
        {
            self.choose(DialogAction::Open, "", ui.ctx());
        }
        if ui.button("Read the workspace guide").clicked() {
            self.manual = true;
        }
        ui.add_space(20.);
        ui.label(RichText::new("Work stays local.").small().weak());
        if self.dialog.is_some() {
            ui.label("Choosing a file…");
        }
    }

    fn work_browser(&mut self, ui: &mut Ui) {
        ui.heading("Your Work");
        ui.horizontal(|ui| {
            if ui.selectable_label(!self.recovered, "All work").clicked() {
                self.recovered = false;
            }
            if ui.selectable_label(self.recovered, "Recovered").clicked() {
                self.recovered = true;
                let result = self.workspace.refresh_recoveries();
                if let Err(error) = result {
                    self.message = Some(error);
                }
            }
            if icon(ui, "\u{E094}", "Refresh catalog").clicked() {
                self.rescan();
                let _ = self.workspace.refresh_recoveries();
            }
        });
        if self.recovered {
            self.recovery_browser(ui);
            return;
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .id(egui::Id::new("find-work"))
                .hint_text("Find work…")
                .desired_width(f32::INFINITY),
        );
        ui.horizontal_wrapped(|ui| {
            for (label, value) in [
                ("All", None),
                ("Horizontal", Some(false)),
                ("Vertical", Some(true)),
            ] {
                ui.selectable_value(&mut self.portrait, value, label);
            }
        });
        if !self.selection.is_empty() {
            ui.horizontal(|ui| {
                if ui
                    .button(format!("Open {} selected", self.selection.len()))
                    .clicked()
                {
                    self.workspace.open(self.selection.drain());
                }
                if ui.button("Clear").clicked() {
                    self.selection.clear();
                }
            });
        }
        if self.scan.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(format!("Finding work · {} files", self.documents.len()));
            });
        }
        if !self.scan_errors.is_empty() {
            ui.collapsing(format!("{} catalog issues", self.scan_errors.len()), |ui| {
                for error in &self.scan_errors {
                    ui.colored_label(self.theme.palette.warning, error);
                }
            });
        }
        let query = self.query.to_lowercase();
        let items: Vec<_> = self
            .documents
            .iter()
            .filter(|doc| {
                query.split_whitespace().all(|word| {
                    doc.name.to_lowercase().contains(word)
                        || doc.path.to_string_lossy().to_lowercase().contains(word)
                }) && self
                    .portrait
                    .is_none_or(|portrait| portrait == (doc.height > doc.width))
            })
            .cloned()
            .collect();
        if items.is_empty() {
            ui.add_space(20.);
            ui.weak(if self.scan.is_some() {
                "Looking for your projects…"
            } else {
                "No matching work. Create a project or open an existing file."
            });
        }
        self.cards(ui, &items, "your-work");
    }

    fn cards(&mut self, ui: &mut Ui, items: &[DocumentEntry], salt: &str) {
        let pal = self.theme.palette;
        egui::ScrollArea::vertical()
            .id_salt(salt)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width();
                let count = ((width / 160.).floor() as usize).clamp(1, 3);
                let gap = 10.;
                let card_width = (width - gap * (count - 1) as f32) / count as f32;
                let origin = ui.cursor().min;
                let mut heights = vec![0_f32; count];
                for doc in items {
                    let column = heights
                        .iter()
                        .enumerate()
                        .min_by(|a, b| a.1.total_cmp(b.1))
                        .unwrap()
                        .0;
                    let ratio = doc.width as f32 / doc.height.max(1) as f32;
                    let height = (card_width / ratio).clamp(48., 420.);
                    let rect = egui::Rect::from_min_size(
                        origin + vec2(column as f32 * (card_width + gap), heights[column]),
                        vec2(card_width, height),
                    );
                    heights[column] += height + gap;
                    if !rect.intersects(ui.clip_rect()) {
                        continue;
                    }
                    let selected = self.selection.contains(&doc.path);
                    let response = ui.interact(rect, ui.id().with(&doc.path), Sense::click());
                    ui.painter().rect_filled(rect, 6., pal.canvas);
                    ui.painter().rect_stroke(
                        rect,
                        6.,
                        Stroke::new(
                            if selected { 2. } else { 1. },
                            if selected { pal.accent } else { pal.border },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        format!("{} × {}", doc.width, doc.height),
                        FontId::proportional(11.),
                        pal.muted,
                    );
                    if doc.error.is_some() {
                        ui.painter().text(
                            rect.left_top() + vec2(8., 8.),
                            Align2::LEFT_TOP,
                            "Could not read",
                            FontId::proportional(11.),
                            pal.warning,
                        );
                    }
                    if response.hovered() || selected {
                        let bar = egui::Rect::from_min_size(
                            rect.left_bottom() - vec2(0., 28.),
                            vec2(card_width, 28.),
                        );
                        ui.painter().rect_filled(bar, 4., pal.panel);
                        ui.painter().with_clip_rect(bar.shrink(5.)).text(
                            bar.left_center() + vec2(5., 0.),
                            Align2::LEFT_CENTER,
                            &doc.name,
                            FontId::proportional(11.),
                            pal.foreground,
                        );
                    }
                    if response.clicked() {
                        if ui.input(|i| i.modifiers.shift) || !self.selection.is_empty() {
                            if !self.selection.insert(doc.path.clone()) {
                                self.selection.remove(&doc.path);
                            }
                        } else {
                            self.workspace.open([doc.path.clone()]);
                        }
                    }
                    response.on_hover_text(doc.error.as_deref().unwrap_or(&doc.name));
                }
                ui.allocate_space(vec2(width, heights.into_iter().fold(0_f32, f32::max)));
            });
    }

    fn project_browser(&mut self, ui: &mut Ui) {
        ui.heading("Projects");
        if let Some(scope) = self.project_scope.clone() {
            ui.horizontal_wrapped(|ui| {
                if ui.button("← Projects").clicked() {
                    self.project_scope = None;
                    self.selection.clear();
                }
                ui.weak(scope.file_name().unwrap_or_default().to_string_lossy());
            });
            if let Some(parent) = self
                .projects
                .iter()
                .filter(|project| scope.starts_with(&project.path) && project.path != scope)
                .max_by_key(|project| project.path.components().count())
            {
                let path = parent.path.clone();
                if ui.button("↑ Parent project").clicked() {
                    self.project_scope = Some(path);
                    self.selection.clear();
                }
            }
            let nested: Vec<_> = self
                .projects
                .iter()
                .filter(|project| project.path.starts_with(&scope) && project.path != scope)
                .map(|project| project.path.clone())
                .collect();
            for path in nested {
                if ui
                    .button(format!(
                        "\u{25A1} {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ))
                    .clicked()
                {
                    self.project_scope = Some(path);
                    self.selection.clear();
                }
            }
            let items: Vec<_> = self
                .documents
                .iter()
                .filter(|doc| doc.path.starts_with(&scope))
                .cloned()
                .collect();
            if items.is_empty() {
                ui.weak("This project has no EditBay work yet.");
            }
            self.cards(ui, &items, "project-work");
        } else {
            ui.weak("Shared .omabrand folders on this machine");
            let paths: Vec<_> = self
                .projects
                .iter()
                .map(|project| project.path.clone())
                .collect();
            egui::ScrollArea::vertical()
                .id_salt("projects")
                .show(ui, |ui| {
                    if paths.is_empty() {
                        ui.add_space(24.);
                        ui.weak(if self.scan.is_some() {
                            "Finding project folders…"
                        } else {
                            "No project folders found."
                        });
                    }
                    for path in paths {
                        let count = self.project_counts.get(&path).copied().unwrap_or_default();
                        if ui
                            .add_sized(
                                [ui.available_width(), 64.],
                                egui::Button::new(format!(
                                    "{}\n{} {}",
                                    path.file_name().unwrap_or_default().to_string_lossy(),
                                    count,
                                    if count == 1 { "file" } else { "files" }
                                )),
                            )
                            .clicked()
                        {
                            self.project_scope = Some(path);
                            self.selection.clear();
                        }
                    }
                });
        }
    }

    fn recovery_browser(&mut self, ui: &mut Ui) {
        if ui
            .add_enabled(
                !self.workspace.busy(),
                egui::Button::new("Refresh recovery history"),
            )
            .clicked()
            && let Err(error) = self.workspace.refresh_recoveries()
        {
            self.message = Some(error);
        }
        egui::ScrollArea::vertical()
            .id_salt("recoveries")
            .show(ui, |ui| {
                if self.workspace.recoveries.valid.is_empty() {
                    ui.add_space(20.);
                    ui.weak("No recoverable work yet.");
                }
                for record in &self.workspace.recoveries.valid {
                    if ui
                        .add_sized(
                            [ui.available_width(), 58.],
                            egui::Button::new(format!(
                                "{}\nRevision {} · {} × {}",
                                record.name, record.revision, record.width, record.height
                            )),
                        )
                        .clicked()
                    {
                        self.recovery_preview = Some(record.path.clone());
                    }
                }
                for failure in &self.workspace.recoveries.invalid {
                    ui.colored_label(
                        self.theme.palette.warning,
                        format!("{}: {}", failure.path.display(), failure.error),
                    );
                }
            });
    }

    fn document(&mut self, ui: &mut Ui) {
        let Some(id) = self.workspace.active else {
            self.welcome = true;
            return;
        };
        let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let project = tab.editor.snapshot();
        let dirty = tab.dirty();
        let path = tab.path.clone();
        let recovery = tab.recovery_revision;
        let error = tab.recovery_error.clone();
        ui.horizontal(|ui| {
            ui.heading(&project.name);
            let rename = ui.button("Rename…");
            self.preview.observe_control("rename-project", &rename, ui);
            if rename.clicked() {
                self.rename = Some((id, DocumentVersion::of(&project), project.name.clone()));
                self.focus_name = true;
            }
            ui.weak(if dirty { "Unsaved changes" } else { "Saved" });
        });
        let location = path.map_or_else(
            || "Untitled local work".into(),
            |path| path.display().to_string(),
        );
        ui.add(egui::Label::new(egui::RichText::new(&location).weak()).truncate())
            .on_hover_text(location);
        ui.add_space(12.);
        self.media.show(ui);
        if !project.compositions.is_empty() || !project.sources.is_empty() {
            let count = |size, name| format!("{size} {name}{}", if size == 1 { "" } else { "s" });
            ui.weak(format!(
                "{} · {} · {}",
                count(project.sources.len(), "source"),
                count(project.compositions.len(), "composition"),
                count(project.assets.len(), "asset")
            ));
        }
        if !project.sources.is_empty() {
            let header = egui::CollapsingHeader::new("Source media")
                .default_open(true)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(180.)
                        .show(ui, |ui| {
                            for source in &project.sources {
                                let header = egui::CollapsingHeader::new(&source.name)
                                    .id_salt(source.id)
                                    .show(ui, |ui| {
                                        let selected_audio =
                                            self.preview.source_sound(ui, id, source);
                                        for stream in &source.streams {
                                            let description = match &stream.format {
                                                editbay_core::StreamFormat::Video {
                                                    width,
                                                    height,
                                                    timing,
                                                    ..
                                                } => {
                                                    let timing = match timing {
                                                        editbay_core::PictureTiming::Variable {
                                                            presentation_ticks,
                                                            ..
                                                        } => format!(
                                                            "{} indexed pictures",
                                                            presentation_ticks.len()
                                                        ),
                                                        editbay_core::PictureTiming::Constant {
                                                            rate,
                                                        } => format!(
                                                            "{}/{} fps",
                                                            rate.numerator, rate.denominator
                                                        ),
                                                    };
                                                    format!("{width} × {height} · {timing}")
                                                }
                                                editbay_core::StreamFormat::Audio {
                                                    sample_rate,
                                                    channels,
                                                } => format!(
                                                    "{sample_rate} Hz · {}",
                                                    channels.join(" / ")
                                                ),
                                            };
                                            ui.label(format!(
                                                "Stream {} · {} · {description}",
                                                stream.index, stream.codec
                                            ));
                                            {
                                                let video = matches!(
                                                    stream.format,
                                                    editbay_core::StreamFormat::Video { .. }
                                                );
                                                let create = ui.button(if video {
                                                    "Create sequence from video"
                                                } else {
                                                    "Create sound sequence"
                                                });
                                                self.preview.observe_control(
                                                    &format!(
                                                        "create-sequence:{}:{}",
                                                        source.id, stream.index
                                                    ),
                                                    &create,
                                                    ui,
                                                );
                                                if create.clicked()
                                                    && let Err(error) =
                                                        self.preview.create_sequence(
                                                            &self.workspace,
                                                            id,
                                                            source.id,
                                                            stream.index,
                                                            selected_audio,
                                                        )
                                                {
                                                    self.message = Some(error);
                                                }
                                            }
                                        }
                                    });
                                self.preview.observe_control(
                                    &format!("source:{}", source.id),
                                    &header.header_response,
                                    ui,
                                );
                            }
                        });
                });
            self.preview
                .observe_control("source-media", &header.header_response, ui);
        }
        egui::ScrollArea::vertical()
            .id_salt("document")
            .show(ui, |ui| {
                let interactive = self.dialog.is_none()
                    && self.create_name.is_none()
                    && self.rename.is_none()
                    && self.close_guard.is_none()
                    && self.settings.is_none()
                    && !self.manual
                    && !self.bank.opened;
                if ui.available_width() >= 640. {
                    ui.columns(2, |columns| {
                        let height = columns[1].available_height();
                        egui::ScrollArea::vertical()
                            .id_salt("timeline-controls")
                            .max_height(height)
                            .show(&mut columns[1], |ui| {
                                self.timeline.show(
                                    ui,
                                    &self.workspace,
                                    &mut self.preview,
                                    id,
                                    project.clone(),
                                    interactive,
                                );
                            });
                        self.preview
                            .show(&mut columns[0], &self.workspace, id, project.clone());
                    });
                } else {
                    let edit_height = (ui.available_height() * 0.2).clamp(48., 120.);
                    let controls = egui::CollapsingHeader::new("Edit controls")
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("compact-edit-controls")
                                .max_height(edit_height)
                                .show(ui, |ui| {
                                    self.timeline.show(
                                        ui,
                                        &self.workspace,
                                        &mut self.preview,
                                        id,
                                        project.clone(),
                                        interactive,
                                    );
                                });
                        });
                    self.preview
                        .observe_control("edit-controls", &controls.header_response, ui);
                    self.preview.show(ui, &self.workspace, id, project.clone());
                }
                for sequence in project
                    .sequences
                    .iter()
                    .filter(|sequence| sequence.composition.is_none())
                {
                    ui.heading(&sequence.name);
                    ui.label(format!(
                        "{} × {} · {}/{} fps",
                        sequence.width,
                        sequence.height,
                        sequence.frame_rate.numerator,
                        sequence.frame_rate.denominator
                    ));
                    let scale = ((ui.available_width() - 24.).min(960.) / sequence.width as f32)
                        .min(400. / sequence.height as f32);
                    let width = sequence.width as f32 * scale;
                    let height = sequence.height as f32 * scale;
                    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 4., self.theme.palette.canvas);
                    ui.painter().rect_stroke(
                        rect,
                        4.,
                        Stroke::new(1., self.theme.palette.border),
                        egui::StrokeKind::Inside,
                    );
                    ui.add_space(16.);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(recovery.map_or_else(
                        || "No checkpoint for this session".into(),
                        |revision| format!("Recovery saved · revision {revision}"),
                    ));
                    if ui
                        .add_enabled(
                            recovery != Some(project.revision),
                            egui::Button::new("Checkpoint now"),
                        )
                        .clicked()
                        && let Err(error) = self.workspace.checkpoint_now(id)
                    {
                        self.message = Some(error);
                    }
                    if ui.button("Recovery history").clicked() {
                        self.welcome = true;
                        self.recovered = true;
                        let _ = self.workspace.refresh_recoveries();
                    }
                });
                if let Some(error) = error {
                    ui.colored_label(self.theme.palette.error, error);
                }
            });
    }

    fn modals(&mut self, ctx: &egui::Context) {
        if let Some((owner, request, name)) =
            self.delivery
                .show_choice(ctx, &self.workspace, &mut self.preview)
        {
            self.choose(DialogAction::Delivery { owner, request }, &name, ctx);
        }
        if let Some(action) = self.bank.show(ctx) {
            match action {
                BankAction::Choose => {
                    self.choose(DialogAction::BrandFolder { create: false }, "", ctx)
                }
                BankAction::Create => {
                    self.choose(DialogAction::BrandFolder { create: true }, "", ctx)
                }
                BankAction::Import {
                    root,
                    version,
                    category,
                } => self.choose(
                    DialogAction::BrandImport {
                        root,
                        version,
                        category,
                    },
                    "",
                    ctx,
                ),
                BankAction::Export { root, asset } => {
                    let name = asset
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    self.choose(DialogAction::BrandExport { root, asset }, &name, ctx);
                }
            }
        }
        if let Some(mut name) = self.create_name.take() {
            let mut keep = true;
            let modal = egui::Modal::new(egui::Id::new("new-project")).show(ctx, |ui| {
                ui.heading("New project");
                ui.label("Name");
                preserve_name_input(ui, &mut self.name_events);
                let name_id = egui::Id::new("new-project-name");
                let focus_name = self.focus_name && !ui.is_sizing_pass();
                if focus_name {
                    ui.memory_mut(|memory| memory.request_focus(name_id));
                }
                let focused = ui.memory(|memory| memory.has_focus(name_id));
                let input = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .id(name_id)
                        .char_limit(256),
                );
                let enter = (focused || input.lost_focus())
                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if focus_name {
                    self.focus_name = false;
                    if !enter {
                        input.request_focus();
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Picture");
                    ui.add(egui::DragValue::new(&mut self.new_profile[0]).range(1..=16_384));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut self.new_profile[1]).range(1..=16_384));
                });
                ui.horizontal(|ui| {
                    if ui.small_button("1080p").clicked() {
                        self.new_profile[0] = 1920;
                        self.new_profile[1] = 1080;
                    }
                    if ui.small_button("Vertical").clicked() {
                        self.new_profile[0] = 1080;
                        self.new_profile[1] = 1920;
                    }
                    if ui.small_button("4K").clicked() {
                        self.new_profile[0] = 3840;
                        self.new_profile[1] = 2160;
                    }
                });
                egui::ComboBox::from_label("Frame rate")
                    .selected_text(format!(
                        "{}/{} fps",
                        self.new_profile[2], self.new_profile[3]
                    ))
                    .show_ui(ui, |ui| {
                        for (label, numerator, denominator) in [
                            ("23.976", 24_000, 1_001),
                            ("24", 24, 1),
                            ("25", 25, 1),
                            ("29.97", 30_000, 1_001),
                            ("30", 30, 1),
                            ("50", 50, 1),
                            ("59.94", 60_000, 1_001),
                            ("60", 60, 1),
                        ] {
                            if ui
                                .selectable_label(
                                    (self.new_profile[2], self.new_profile[3])
                                        == (numerator, denominator),
                                    label,
                                )
                                .clicked()
                            {
                                self.new_profile[2] = numerator;
                                self.new_profile[3] = denominator;
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!name.trim().is_empty(), egui::Button::new("Create"))
                        .clicked()
                        || (enter && !name.trim().is_empty())
                    {
                        match Project::new(name.clone())
                            .map(|mut project| {
                                project.sequences[0].width = self.new_profile[0];
                                project.sequences[0].height = self.new_profile[1];
                                project.sequences[0].frame_rate.numerator = self.new_profile[2];
                                project.sequences[0].frame_rate.denominator = self.new_profile[3];
                                project
                            })
                            .map_err(|e| e.to_string())
                            .and_then(|project| self.workspace.create(project))
                        {
                            Ok(_) => {
                                self.welcome = false;
                                keep = false;
                            }
                            Err(error) => self.message = Some(error),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });
            keep &= !modal.should_close();
            if keep {
                self.create_name = Some(name);
            } else {
                self.name_events.clear();
                ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("new-project-name")));
            }
        }
        if let Some((id, expected, mut name)) = self.rename.take() {
            let mut keep = true;
            let modal = egui::Modal::new(egui::Id::new("rename-project")).show(ctx, |ui| {
                ui.heading("Rename project");
                preserve_name_input(ui, &mut self.name_events);
                let name_id = egui::Id::new("rename-project-name");
                let focus_name = self.focus_name && !ui.is_sizing_pass();
                if focus_name {
                    ui.memory_mut(|memory| memory.request_focus(name_id));
                }
                let focused = ui.memory(|memory| memory.has_focus(name_id));
                let input = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .id(name_id)
                        .char_limit(256),
                );
                let enter = (focused || input.lost_focus())
                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if focus_name {
                    self.focus_name = false;
                    if !enter {
                        input.request_focus();
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!name.trim().is_empty(), egui::Button::new("Apply"))
                        .clicked()
                        || (enter && !name.trim().is_empty())
                    {
                        match self.workspace.apply(
                            id,
                            expected,
                            "Rename project".into(),
                            &[DocumentCommand::RenameProject { name: name.clone() }],
                        ) {
                            Ok(()) => keep = false,
                            Err(error) => self.message = Some(error),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });
            keep &= !modal.should_close();
            if keep {
                self.rename = Some((id, expected, name));
            } else {
                self.name_events.clear();
                ctx.memory_mut(|memory| {
                    memory.surrender_focus(egui::Id::new("rename-project-name"))
                });
            }
        }
        if let Some(id) = self.close_guard {
            let name = self
                .workspace
                .tabs
                .iter()
                .find(|tab| tab.id == id)
                .map(|tab| tab.editor.project().name.clone())
                .unwrap_or_default();
            egui::Modal::new(egui::Id::new("close-project")).show(ctx, |ui| {
                ui.heading("Save your changes?");
                ui.label(name);
                ui.horizontal(|ui| {
                    let available = !self.workspace.saving(id) && self.dialog.is_none();
                    if ui
                        .add_enabled(available, egui::Button::new("Save"))
                        .clicked()
                    {
                        self.close_after_save = true;
                        self.save_tab(id, false, false, ctx);
                    }
                    if ui
                        .add_enabled(available, egui::Button::new("Discard"))
                        .clicked()
                    {
                        if let Err(error) = self.workspace.close(id, true) {
                            self.message = Some(error);
                        } else {
                            self.close_guard = None;
                            if self.quit {
                                if let Some(tab) =
                                    self.workspace.tabs.iter().find(|tab| tab.dirty())
                                {
                                    self.close_guard = Some(tab.id);
                                } else {
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                            }
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.quit = false;
                        self.close_guard = None;
                        self.close_after_save = false;
                    }
                });
            });
        }
        if let Some(path) = &self.recovery_preview {
            let record = self
                .workspace
                .recoveries
                .valid
                .iter()
                .find(|record| &record.path == path)
                .map(|record| {
                    (
                        record.name.clone(),
                        record.revision,
                        record.width,
                        record.height,
                        record.frame_rate,
                        record.sequence_count,
                        record.path.clone(),
                    )
                });
            if let Some((name, revision, width, height, rate, count, path)) = record {
                egui::Modal::new(egui::Id::new("recovery-preview")).show(ctx, |ui| {
                    ui.heading(&name);
                    ui.label(format!(
                        "Revision {revision} · {count} {}",
                        if count == 1 { "sequence" } else { "sequences" }
                    ));
                    ui.label(format!(
                        "{width} × {height} · {}/{} fps",
                        rate.numerator, rate.denominator
                    ));
                    ui.label("Recover to a new file. Your original stays intact.");
                    ui.horizontal(|ui| {
                        if ui.button("Recover copy…").clicked() {
                            self.choose(
                                DialogAction::Recover { snapshot: path },
                                &format!("{name} Recovered"),
                                ctx,
                            );
                            self.recovery_preview = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.recovery_preview = None;
                        }
                    });
                });
            } else {
                self.recovery_preview = None;
            }
        }
        if let Some(mut settings) = self.settings.take() {
            let mut keep = true;
            egui::Modal::new(egui::Id::new("workspace-settings")).show(ctx, |ui| {
                ui.set_min_width(340.);
                ui.heading("Workspace settings");
                ui.label("At startup");
                ui.radio_value(&mut settings.startup, Startup::Welcome, "Show welcome");
                ui.radio_value(
                    &mut settings.startup,
                    Startup::RestoreWorkspace,
                    "Reopen saved tabs",
                );
                ui.weak("Unsaved work stays in Recovered until you recover a copy.");
                ui.separator();
                ui.label("Your Work folder");
                ui.weak(self.root.display().to_string());
                if ui
                    .add_enabled(
                        self.preferences.ready && self.dialog.is_none(),
                        egui::Button::new("Choose folder…"),
                    )
                    .clicked()
                {
                    self.choose(DialogAction::CatalogRoot, "", ctx);
                }
                ui.separator();
                ui.label("Shortcuts");
                egui::Grid::new("shortcut-settings")
                    .spacing([16., 8.])
                    .show(ui, |ui| {
                        for (label, key) in [
                            ("New project", &mut settings.shortcuts.new_project),
                            ("Open", &mut settings.shortcuts.open),
                            ("Save", &mut settings.shortcuts.save),
                            ("Undo", &mut settings.shortcuts.undo),
                            ("Redo", &mut settings.shortcuts.redo),
                        ] {
                            ui.label(label);
                            ui.label("Ctrl +");
                            let mut text = key.to_string();
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut text)
                                        .id(egui::Id::new(("shortcut-key", label)))
                                        .char_limit(1)
                                        .desired_width(40.),
                                )
                                .changed()
                            {
                                *key = text.chars().next().unwrap_or(' ').to_ascii_uppercase();
                            }
                            ui.end_row();
                        }
                    });
                ui.checkbox(&mut settings.shortcuts.redo_shift, "Use Shift for Redo");
                ui.weak("Save as uses Ctrl + Shift + your Save key.");
                let valid = settings.validate();
                if let Err(error) = &valid {
                    ui.colored_label(self.theme.palette.warning, error);
                }
                if let Some(error) = self.preferences.error.clone() {
                    ui.colored_label(self.theme.palette.error, error);
                    ui.horizontal(|ui| {
                        if ui.button("Reload settings").clicked() {
                            self.preferences.reload();
                            keep = false;
                        }
                        if self.preferences.ready && ui.button("Retry save").clicked() {
                            self.preferences.retry_save();
                        }
                    });
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            valid.is_ok() && self.preferences.ready,
                            egui::Button::new("Apply"),
                        )
                        .clicked()
                    {
                        let mut updated = self.preferences.current.clone();
                        updated.startup = settings.startup;
                        updated.shortcuts = settings.shortcuts.clone();
                        match self.preferences.update(updated) {
                            Ok(()) => keep = false,
                            Err(error) => self.message = Some(error),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });
            if keep {
                self.settings = Some(settings);
            }
        }
        if self.manual {
            let mut open = true;
            egui::Window::new("Your local workspace")
                .open(&mut open)
                .default_width(600.)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.label(include_str!("../../../docs/LOCAL_WORKSPACE.md"));
                    })
                });
            self.manual = open;
        }
        if let Some(message) = self.message.clone() {
            egui::Window::new("EditBay notice")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.set_max_width(500.);
                    ui.label(message);
                    if ui.button("OK").clicked() {
                        self.message = None;
                    }
                });
        }
    }
}

impl eframe::App for Studio {
    fn raw_input_hook(&mut self, _: &egui::Context, input: &mut egui::RawInput) {
        if let Some(diagnostics) = &mut self.diagnostics
            && !input.events.is_empty()
        {
            let shortcut_keys: Vec<_> = input.events.iter().filter_map(|event| {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = event
                    && (modifiers.ctrl || modifiers.command || matches!(key, egui::Key::N | egui::Key::Z | egui::Key::Enter))
                {
                    Some(serde_json::json!({"key":key.name(),"ctrl":modifiers.ctrl,"command":modifiers.command,"shift":modifiers.shift,"mac_cmd":modifiers.mac_cmd,"alt":modifiers.alt}))
                } else { None }
            }).collect();
            diagnostics.record("input", serde_json::json!({"events":input.events.len(),"ime_preedit_events":input.events.iter().filter(|event|matches!(event,egui::Event::Ime(egui::ImeEvent::Preedit{..}))).count(),"ime_commit_events":input.events.iter().filter(|event|matches!(event,egui::Event::Ime(egui::ImeEvent::Commit(_)))).count(),"shortcut_keys":shortcut_keys}));
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.frame_started = Instant::now();
        self.tick(ctx);
        if ctx.input(|input| input.viewport().close_requested()) {
            self.quit = true;
            self.delivery.cancel();
            if self.bank.dirty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.quit = false;
                self.bank.opened = true;
                self.message = Some("Save or discard bank details before closing EditBay".into());
            } else if let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.dirty()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.close_guard = Some(tab.id);
            } else if self.dialog.is_some()
                || self.preferences.busy()
                || self.workspace.busy()
                || self.bank.busy()
                || self.delivery.busy()
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        if self.quit
            && self.close_guard.is_none()
            && self.dialog.is_none()
            && !self.preferences.busy()
            && !self.workspace.busy()
            && !self.bank.busy()
            && !self.delivery.busy()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.shortcuts(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.preview.begin_frame();
        egui::Frame::new()
            .fill(self.theme.palette.background)
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.toolbar(ui);
                if let Err(error) = self
                    .delivery
                    .show(ui, &mut self.workspace, &mut self.preview)
                {
                    self.message = Some(error);
                }
                ui.add_space(12.);
                if self.welcome {
                    self.welcome(ui);
                } else {
                    self.document(ui);
                }
            });
        self.modals(&ctx);
        if let Some(diagnostics) = &mut self.diagnostics {
            diagnostics.observe(
                &self.workspace,
                &self.media,
                &self.preview,
                &self.delivery,
                &self.timeline,
            );
            diagnostics.record("frame", serde_json::json!({"cpu_us":self.frame_started.elapsed().as_micros() as u64,"catalog_running":self.scan.is_some(),"workspace_busy":self.workspace.busy(),"bank_busy":self.bank.busy(),"welcome":self.welcome,"recovered":self.recovered,"recovery_preview":self.recovery_preview.is_some(),"recoveries_valid":self.workspace.recoveries.valid.len(),"dialog_pending":self.dialog.is_some(),"new_project_name":self.create_name,"rename_name":self.rename.as_ref().map(|(_,_,name)|name),"desktop_font_ready":self.theme.font_path.is_some(),"text_input_focused":ctx.text_edit_focused()}));
        }
    }
}

/// Retain keyboard edits while a new modal measures its disabled first pass.
/// `ui` is the native name form; `deferred` belongs to that same dialog.
/// Returns no value and replays edits in order on its first interactive pass.
fn preserve_name_input(ui: &mut Ui, deferred: &mut Vec<egui::Event>) {
    if ui.is_sizing_pass() {
        ui.input(|input| {
            deferred.extend(
                input
                    .events
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            egui::Event::Key { .. }
                                | egui::Event::Text(_)
                                | egui::Event::Paste(_)
                                | egui::Event::Copy
                                | egui::Event::Cut
                                | egui::Event::Ime(_)
                        )
                    })
                    .cloned(),
            );
        });
    } else if !deferred.is_empty() {
        ui.input_mut(|input| {
            input.events.splice(0..0, deferred.drain(..));
        });
    }
}

fn file_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\'))
        .take(120)
        .collect();
    format!(
        "{}.editbay",
        if safe.trim().is_empty() {
            "Project"
        } else {
            safe.trim()
        }
    )
}

fn icon(ui: &mut Ui, glyph: &str, tip: &str) -> egui::Response {
    ui.add(egui::Button::new(
        RichText::new(glyph)
            .family(FontFamily::Name("phosphor".into()))
            .size(16.),
    ))
    .on_hover_text(tip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editing_input(shortcut: Option<egui::Key>, name: &str) -> egui::RawInput {
        let modifiers = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        };
        let mut events = Vec::new();
        if let Some(shortcut) = shortcut {
            events.push(key(shortcut, modifiers));
        }
        events.extend([
            key(egui::Key::A, modifiers),
            egui::Event::Text(name.into()),
            key(egui::Key::Enter, egui::Modifiers::NONE),
        ]);
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(1440., 900.),
            )),
            events,
            ..Default::default()
        }
    }

    #[test]
    fn batched_native_shortcut_text_and_enter_create_and_rename_without_losing_input() {
        let directory = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut studio = Studio::new(
            ctx.clone(),
            directory.path().into(),
            directory.path().into(),
            directory.path().join("state"),
            Vec::new(),
            rfd::AsyncFileDialog::new(),
        );
        ctx.run_ui(
            editing_input(Some(egui::Key::N), "Batched native input"),
            |ui| {
                studio.shortcuts(ui.ctx());
                studio.modals(ui.ctx());
            },
        )
        .drop_without_applying_deltas();
        ctx.run_ui(egui::RawInput::default(), |ui| studio.modals(ui.ctx()))
            .drop_without_applying_deltas();
        assert_eq!(
            studio.workspace.tabs.len(),
            1,
            "draft={:?}, deferred={:?}, focus={}",
            studio.create_name,
            studio.name_events,
            studio.focus_name
        );
        assert_eq!(
            studio.workspace.tabs[0].editor.project().name,
            "Batched native input"
        );
        assert_eq!(studio.workspace.tabs[0].editor.project().revision, 0);
        assert!(
            !ctx.text_edit_focused(),
            "closed create dialog still owns keyboard input"
        );
        let tab = &studio.workspace.tabs[0];
        studio.rename = Some((
            tab.id,
            DocumentVersion::of(tab.editor.project()),
            tab.editor.project().name.clone(),
        ));
        studio.focus_name = true;
        ctx.run_ui(editing_input(None, "Renamed in one native frame"), |ui| {
            studio.modals(ui.ctx())
        })
        .drop_without_applying_deltas();
        ctx.run_ui(egui::RawInput::default(), |ui| studio.modals(ui.ctx()))
            .drop_without_applying_deltas();
        assert_eq!(
            studio.workspace.tabs[0].editor.project().name,
            "Renamed in one native frame"
        );
        assert_eq!(studio.workspace.tabs[0].editor.project().revision, 1);
        assert!(
            !ctx.text_edit_focused(),
            "closed rename dialog still owns keyboard input"
        );
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Z,
                    physical_key: Some(egui::Key::Z),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
                }],
                ..Default::default()
            },
            |ui| studio.shortcuts(ui.ctx()),
        )
        .drop_without_applying_deltas();
        assert_eq!(
            studio.workspace.tabs[0].editor.project().name,
            "Batched native input"
        );
        assert_eq!(studio.workspace.tabs[0].editor.project().revision, 2);
    }

    #[test]
    fn project_undo_works_after_button_focus_and_text_fields_keep_their_history() {
        let directory = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut studio = Studio::new(
            ctx.clone(),
            directory.path().into(),
            directory.path().into(),
            directory.path().join("state"),
            Vec::new(),
            rfd::AsyncFileDialog::new(),
        );
        let id = studio
            .workspace
            .create(Project::new("Original").unwrap())
            .unwrap();
        let expected = DocumentVersion::of(studio.workspace.tabs[0].editor.project());
        studio
            .workspace
            .apply(
                id,
                expected,
                "Rename".into(),
                &[DocumentCommand::RenameProject {
                    name: "Revised".into(),
                }],
            )
            .unwrap();
        let undo_input = || egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: Some(egui::Key::Z),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            }],
            ..Default::default()
        };
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("focused-button")));
        ctx.run_ui(undo_input(), |ui| studio.shortcuts(ui.ctx()))
            .drop_without_applying_deltas();
        assert_eq!(studio.workspace.tabs[0].editor.project().name, "Original");
        assert_eq!(studio.workspace.tabs[0].editor.project().revision, 2);
        studio.workspace.history(id, true).unwrap();
        let mut query = String::from("Find a project");
        ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut query).id(egui::Id::new("find-project")))
                .request_focus();
        })
        .drop_without_applying_deltas();
        ctx.run_ui(undo_input(), |ui| {
            studio.shortcuts(ui.ctx());
            ui.add(egui::TextEdit::singleline(&mut query).id(egui::Id::new("find-project")));
        })
        .drop_without_applying_deltas();
        assert_eq!(studio.workspace.tabs[0].editor.project().name, "Revised");
        assert_eq!(studio.workspace.tabs[0].editor.project().revision, 3);
    }
}
