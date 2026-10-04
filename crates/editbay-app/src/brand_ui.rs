use crate::brand::{self, Asset, BankVersion, Catalog, Category, Metadata, Palette};
use eframe::egui::{self, Color32};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};

pub enum Action {
    Choose,
    Create,
    Import {
        root: PathBuf,
        version: BankVersion,
        category: Category,
    },
    Export {
        root: PathBuf,
        asset: Asset,
    },
}

pub enum Task {
    Load(PathBuf),
    Discover(PathBuf),
    Create(PathBuf),
    Import {
        root: PathBuf,
        version: BankVersion,
        source: PathBuf,
        category: Category,
    },
    Metadata {
        root: PathBuf,
        version: BankVersion,
        metadata: Metadata,
    },
    Export {
        root: PathBuf,
        asset: Asset,
        destination: PathBuf,
    },
}

struct Worker {
    result: Receiver<Result<(Option<Catalog>, String), String>>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct BankPane {
    pub opened: bool,
    pub catalog: Option<Catalog>,
    pub message: Option<String>,
    worker: Option<Worker>,
    draft: Option<Metadata>,
    category: Option<Category>,
    query: String,
    new_palette_name: String,
    new_palette_colors: String,
    palette_error: Option<String>,
}

impl BankPane {
    /// Start one owned asset operation without blocking native input.
    /// `task` holds the exact folder/version/source and `ctx` receives completion.
    /// Returns an error if another operation still owns the bank worker.
    pub fn run(&mut self, task: Task, ctx: &egui::Context) -> Result<(), String> {
        if self.worker.is_some() {
            return Err("Wait for the current bank operation to finish".into());
        }
        if self.dirty() {
            let changed_bank = match &task {
                Task::Load(root) | Task::Create(root) => self
                    .catalog
                    .as_ref()
                    .is_none_or(|catalog| &catalog.root != root),
                Task::Discover(_) => true,
                _ => false,
            };
            if changed_bank {
                return Err("Save or discard bank details before choosing another bank".into());
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let wake = ctx.clone();
        let (sender, result) = mpsc::channel();
        std::thread::Builder::new()
            .name("editbay-brand-bank".into())
            .spawn(move || {
                let _ = sender.send(execute(task, &token));
                wake.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        self.worker = Some(Worker { result, cancel });
        self.message = None;
        Ok(())
    }

    /// Inspect whether the underlying bank worker is still running.
    /// Takes no arguments and returns true until completion or cancellation is acknowledged.
    pub fn busy(&self) -> bool {
        self.worker.is_some()
    }

    /// Determine whether local bank details still need explicit publication.
    /// Takes no arguments and returns true while the draft differs from its loaded version.
    pub fn dirty(&self) -> bool {
        !self.new_palette_name.trim().is_empty()
            || !self.new_palette_colors.trim().is_empty()
            || self.draft.as_ref().is_some_and(|draft| {
                self.catalog
                    .as_ref()
                    .and_then(|catalog| catalog.metadata.as_ref())
                    != Some(draft)
            })
    }

    /// Restore only the loaded details after an explicit discard choice.
    /// Takes no arguments, returns no value and never deletes stored assets.
    pub fn discard(&mut self) {
        self.draft = self
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.metadata.clone());
        self.new_palette_name.clear();
        self.new_palette_colors.clear();
        self.palette_error = None;
    }

    /// Check the owner captured by a native import dialog.
    /// `root` and `version` identify its original bank. Returns false after a
    /// different bank or metadata version has been selected.
    pub fn owns(&self, root: &PathBuf, version: &BankVersion) -> bool {
        self.catalog.as_ref().is_some_and(|catalog| {
            &catalog.root == root && catalog.version.as_ref() == Some(version)
        }) && !self.busy()
    }

    /// Receive real worker results without touching the filesystem.
    /// Takes no arguments; failed mutations invalidate write ownership until refresh.
    pub fn poll(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        match worker.result.try_recv() {
            Ok(Ok((catalog, message))) => {
                if let Some(catalog) = catalog {
                    let mut updated = catalog.metadata.clone();
                    if let (Some(previous), Some(draft), Some(updated)) =
                        (&self.catalog, &self.draft, &mut updated)
                        && previous.root == catalog.root
                        && let Some(loaded) = &previous.metadata
                    {
                        if draft.client != loaded.client {
                            updated.client = draft.client.clone();
                        }
                        if draft.project != loaded.project {
                            updated.project = draft.project.clone();
                        }
                        if draft.notes != loaded.notes {
                            updated.notes = draft.notes.clone();
                        }
                        if draft.palettes != loaded.palettes {
                            updated.palettes = draft.palettes.clone();
                        }
                    }
                    if self
                        .catalog
                        .as_ref()
                        .is_none_or(|previous| previous.root != catalog.root)
                    {
                        self.new_palette_name.clear();
                        self.new_palette_colors.clear();
                        self.palette_error = None;
                    }
                    self.draft = updated;
                    self.catalog = Some(catalog);
                }
                self.message = (!message.is_empty()).then_some(message);
                self.worker = None;
            }
            Ok(Err(error)) => {
                self.message = Some(error);
                if let Some(catalog) = &mut self.catalog {
                    catalog.version = None;
                }
                self.worker = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.message =
                    Some("Brand worker stopped unexpectedly; refresh before editing".into());
                if let Some(catalog) = &mut self.catalog {
                    catalog.version = None;
                }
                self.worker = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Draw the working local asset bank and collect native dialog requests.
    /// `ctx` supplies the native window. Returns at most one explicit action;
    /// asset contents are stored/exported, never executed or silently converted.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Action> {
        if !self.opened {
            return None;
        }
        let mut open = true;
        let mut action = None;
        let mut task = None;
        let busy = self.busy();
        let dirty = self.dirty();
        let mut discard = false;
        egui::Window::new("Brand bank").open(&mut open).default_width(820.).default_height(560.).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy && !dirty, egui::Button::new("Choose bank…")).clicked() { action = Some(Action::Choose); }
                if ui.add_enabled(!busy && !dirty, egui::Button::new("Create bank…")).clicked() { action = Some(Action::Create); }
                if busy {
                    ui.spinner();
                    if ui.button("Cancel work").clicked() && let Some(worker) = &self.worker { worker.cancel.store(true, Ordering::Relaxed); }
                } else if let Some(catalog) = &self.catalog && ui.button("Refresh").clicked() { task = Some(Task::Load(catalog.root.clone())); }
            });
            if let Some(message) = &self.message { ui.label(message); }
            if let Some(error) = &self.palette_error { ui.colored_label(ui.visuals().error_fg_color, error); }
            if dirty { ui.horizontal(|ui| { ui.weak("Unsaved bank details"); if ui.add_enabled(!busy, egui::Button::new("Discard changes")).clicked() { discard = true; } }); }
            let Some(catalog) = &self.catalog else {
                ui.add_space(24.);
                ui.label("Choose a client folder with a .omabrand bank, or create one.");
                return;
            };
            ui.heading(&catalog.name);
            ui.weak(catalog.root.display().to_string());
            for warning in &catalog.warnings { ui.colored_label(Color32::from_rgb(228, 164, 86), warning); }
            if !catalog.complete { ui.label("This bank scan is incomplete. Refresh after resolving the errors."); }
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.columns(2, |columns| {
                    let ui = &mut columns[0];
                    ui.label("Stored assets");
                    ui.add(egui::TextEdit::singleline(&mut self.query).id(egui::Id::new(("bank-query", &catalog.root))).hint_text("Find an asset…"));
                    egui::ComboBox::from_id_salt("asset-category").selected_text(self.category.unwrap_or(Category::Artwork).label()).show_ui(ui, |ui| {
                        for category in [Category::Font, Category::Logo, Category::Artwork, Category::Title, Category::Lut] { ui.selectable_value(&mut self.category, Some(category), category.label()); }
                    });
                    if ui.add_enabled(!busy && catalog.version.is_some(), egui::Button::new("Add asset…")).clicked() && let Some(version) = &catalog.version { action = Some(Action::Import { root: catalog.root.clone(), version: version.clone(), category: self.category.unwrap_or(Category::Artwork) }); }
                    ui.weak("Files keep their original bytes. Names never overwrite existing assets.");
                    for asset in &catalog.assets {
                        if !asset.path.to_string_lossy().to_lowercase().contains(&self.query.to_lowercase()) { continue; }
                        ui.separator();
                        ui.label(asset.path.display().to_string());
                        ui.weak(format!("{} · {} bytes", asset.category.label(), asset.bytes));
                        ui.weak(format!("SHA-256 {}", &asset.verified_sha256[..16]));
                        if ui.add_enabled(!busy, egui::Button::new("Export copy…")).clicked() { action = Some(Action::Export { root: catalog.root.clone(), asset: asset.clone() }); }
                    }
                    let ui = &mut columns[1];
                    ui.label("Client and project");
                    if let Some(draft) = &mut self.draft {
                        ui.add(egui::TextEdit::singleline(&mut draft.client).id(egui::Id::new(("bank-client", &catalog.root))).hint_text("Client").char_limit(256));
                        ui.add(egui::TextEdit::singleline(&mut draft.project).id(egui::Id::new(("bank-project", &catalog.root))).hint_text("Project").char_limit(256));
                        ui.add(egui::TextEdit::multiline(&mut draft.notes).id(egui::Id::new(("bank-notes", &catalog.root))).hint_text("Notes").char_limit(8192).desired_rows(3));
                        ui.separator();
                        ui.label("Palettes");
                        let mut remove = None;
                        for (index, palette) in draft.palettes.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(&palette.name);
                                if ui.small_button("Remove").clicked() { remove = Some(index); }
                            });
                            ui.horizontal_wrapped(|ui| {
                                for color in &palette.colors {
                                    let rgb = u32::from_str_radix(&color[1..], 16).expect("validated palette");
                                    let (rect, response) = ui.allocate_exact_size(egui::vec2(26., 26.), egui::Sense::hover());
                                    ui.painter().rect_filled(rect, 3., Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8));
                                    response.on_hover_text(color);
                                }
                            });
                        }
                        if let Some(index) = remove { draft.palettes.remove(index); }
                        ui.add(egui::TextEdit::singleline(&mut self.new_palette_name).id(egui::Id::new(("bank-palette-name", &catalog.root))).hint_text("Palette name").char_limit(128));
                        ui.add(egui::TextEdit::singleline(&mut self.new_palette_colors).id(egui::Id::new(("bank-palette-colors", &catalog.root))).hint_text("#ff5c00 #161616").char_limit(512));
                        if ui.button("Add palette").clicked() {
                            let mut updated = draft.clone();
                            updated.palettes.push(Palette { name: self.new_palette_name.trim().to_owned(), colors: self.new_palette_colors.split_whitespace().map(str::to_owned).collect() });
                            match updated.validate() { Ok(()) => { *draft = updated; self.new_palette_name.clear(); self.new_palette_colors.clear(); self.palette_error = None; }, Err(error) => self.palette_error = Some(error) }
                        }
                        if ui.add_enabled(!busy && catalog.version.is_some() && catalog.metadata.as_ref() != Some(draft), egui::Button::new("Save bank details")).clicked() && let Some(version) = &catalog.version { task = Some(Task::Metadata { root: catalog.root.clone(), version: version.clone(), metadata: draft.clone() }); }
                    } else { ui.label("Repair the unsupported or corrupt EditBay manifest before editing these details."); }
                });
            });
        });
        self.opened = open;
        if discard {
            self.discard();
        }
        if let Some(task) = task
            && let Err(error) = self.run(task, ctx)
        {
            self.message = Some(error);
        }
        action
    }
}

impl Drop for BankPane {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn execute(task: Task, cancel: &AtomicBool) -> Result<(Option<Catalog>, String), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Brand work cancelled".into());
    }
    let (root, message) = match task {
        Task::Load(root) => (root, String::new()),
        Task::Discover(folder) => {
            let root = folder
                .ancestors()
                .find_map(|parent| {
                    let candidate = parent.join(".omabrand");
                    std::fs::symlink_metadata(&candidate)
                        .is_ok()
                        .then_some(candidate)
                })
                .ok_or("No brand bank found for this project. Choose a bank or create one.")?;
            (root, String::new())
        }
        Task::Create(folder) => {
            let name = folder.file_name().unwrap_or_default().to_string_lossy();
            (brand::create(&folder, &name)?, "Brand bank is ready".into())
        }
        Task::Import {
            root,
            version,
            source,
            category,
        } => {
            let receipt = brand::import(&root, &version, &source, category, cancel)?;
            (
                root,
                format!(
                    "Added {} · SHA-256 {}",
                    receipt.path.display(),
                    receipt.sha256
                ),
            )
        }
        Task::Metadata {
            root,
            version,
            metadata,
        } => {
            let revision = brand::save_metadata(&root, &version, metadata)?;
            (root, format!("Bank details saved · revision {revision}"))
        }
        Task::Export {
            root,
            asset,
            destination,
        } => {
            brand::export(&root, &asset, &destination, cancel)?;
            return Ok((None, format!("Exported {}", destination.display())));
        }
    };
    brand::scan(&root, cancel)
        .map(|catalog| (Some(catalog), message.clone()))
        .map_err(|error| {
            if message.is_empty() {
                error
            } else {
                format!("{message}. Refresh failed: {error}")
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{Duration, Instant},
    };

    fn settle(pane: &mut BankPane) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while pane.busy() {
            pane.poll();
            assert!(Instant::now() < deadline, "Bank operation timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn typing_keeps_focus_when_unsaved_status_changes_the_window_layout() {
        let directory = tempfile::tempdir().unwrap();
        let root = brand::create(directory.path(), "Client").unwrap();
        let ctx = egui::Context::default();
        let mut pane = BankPane::default();
        pane.run(Task::Load(root.clone()), &ctx).unwrap();
        settle(&mut pane);
        pane.opened = true;
        let input = |text: Option<&str>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440., 900.),
            )),
            events: text
                .map(|text| vec![egui::Event::Text(text.into())])
                .unwrap_or_default(),
            ..Default::default()
        };
        for _ in 0..2 {
            ctx.run_ui(input(None), |ui| {
                pane.show(ui.ctx());
            })
            .drop_without_applying_deltas();
        }
        let id = egui::Id::new(("bank-client", &root));
        ctx.memory_mut(|memory| memory.request_focus(id));
        for text in ["Mack", "s Shack"] {
            ctx.run_ui(input(Some(text)), |ui| {
                pane.show(ui.ctx());
            })
            .drop_without_applying_deltas();
            assert!(ctx.memory(|memory| memory.has_focus(id)));
        }
        assert_eq!(pane.draft.as_ref().unwrap().client, "Macks Shack");
        assert!(pane.dirty());
        assert!(
            pane.catalog
                .as_ref()
                .unwrap()
                .metadata
                .as_ref()
                .unwrap()
                .client
                .is_empty()
        );
    }

    #[test]
    fn asset_and_save_acknowledgements_preserve_unsaved_and_newer_bank_details() {
        let directory = tempfile::tempdir().unwrap();
        let root = brand::create(directory.path(), "Client").unwrap();
        let ctx = egui::Context::default();
        let mut pane = BankPane::default();
        pane.run(Task::Load(root.clone()), &ctx).unwrap();
        settle(&mut pane);
        let draft = pane.draft.as_mut().unwrap();
        draft.client = "Client draft".into();
        draft.palettes.push(Palette {
            name: "Mark".into(),
            colors: vec!["#ff5c00".into()],
        });
        let source = directory.path().join("mark.svg");
        fs::write(&source, b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
        let version = pane.catalog.as_ref().unwrap().version.clone().unwrap();
        pane.run(
            Task::Import {
                root: root.clone(),
                version,
                source,
                category: Category::Logo,
            },
            &ctx,
        )
        .unwrap();
        settle(&mut pane);
        let draft = pane.draft.as_ref().unwrap();
        assert_eq!(draft.client, "Client draft");
        assert_eq!(draft.palettes.len(), 1);
        assert_eq!(draft.assets.len(), 1);
        assert_eq!(draft.revision, 1);
        assert!(pane.dirty());
        let stored = brand::scan(&root, &AtomicBool::new(false))
            .unwrap()
            .metadata
            .unwrap();
        assert!(stored.client.is_empty());
        assert!(stored.palettes.is_empty());
        let other_folder = directory.path().join("Another");
        fs::create_dir(&other_folder).unwrap();
        let another = brand::create(&other_folder, "Another").unwrap();
        assert!(pane.run(Task::Load(another), &ctx).is_err());
        let version = pane.catalog.as_ref().unwrap().version.clone().unwrap();
        let metadata = pane.draft.clone().unwrap();
        pane.run(
            Task::Metadata {
                root: root.clone(),
                version,
                metadata,
            },
            &ctx,
        )
        .unwrap();
        pane.draft.as_mut().unwrap().notes = "Edited while saving".into();
        settle(&mut pane);
        assert_eq!(pane.draft.as_ref().unwrap().notes, "Edited while saving");
        assert!(pane.dirty());
        let stored = brand::scan(&root, &AtomicBool::new(false))
            .unwrap()
            .metadata
            .unwrap();
        assert_eq!(stored.client, "Client draft");
        assert!(stored.notes.is_empty());
        assert_eq!(stored.assets.len(), 1);
        pane.discard();
        assert!(!pane.dirty());
        assert_eq!(pane.draft.as_ref().unwrap(), &stored);
        pane.new_palette_name = "Unsubmitted palette".into();
        assert!(pane.dirty());
        pane.discard();
        assert!(!pane.dirty());
    }

    #[test]
    fn refresh_keeps_local_fields_and_imports_other_writers_unmodified_fields() {
        let directory = tempfile::tempdir().unwrap();
        let root = brand::create(directory.path(), "Client").unwrap();
        let ctx = egui::Context::default();
        let mut pane = BankPane::default();
        pane.run(Task::Load(root.clone()), &ctx).unwrap();
        settle(&mut pane);
        pane.draft.as_mut().unwrap().client = "Local client".into();
        let version = pane.catalog.as_ref().unwrap().version.as_ref().unwrap();
        let mut external = pane.catalog.as_ref().unwrap().metadata.clone().unwrap();
        external.project = "Outside project".into();
        brand::save_metadata(&root, version, external).unwrap();
        pane.run(Task::Load(root), &ctx).unwrap();
        settle(&mut pane);
        let draft = pane.draft.as_ref().unwrap();
        assert_eq!(draft.client, "Local client");
        assert_eq!(draft.project, "Outside project");
        assert_eq!(draft.revision, 1);
        assert!(pane.dirty());
    }
}
