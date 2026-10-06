use editbay_app::workspace::Workspace;
use editbay_core::{
    DocumentCommand, DocumentVersion, Project, load, recover_copy, recovery_catalog, save, save_new,
};
use std::{
    fs,
    time::{Duration, Instant},
};

fn settle(workspace: &mut Workspace) {
    let start = Instant::now();
    while workspace.busy() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "Filesystem work did not finish"
        );
        workspace.poll(Instant::now());
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn rename(workspace: &mut Workspace, id: uuid::Uuid, name: &str) {
    let expected = DocumentVersion::of(
        workspace
            .tabs
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .editor
            .project(),
    );
    workspace
        .apply(
            id,
            expected,
            "Rename".into(),
            &[DocumentCommand::RenameProject { name: name.into() }],
        )
        .unwrap();
}

#[test]
fn document_mutations_cancel_exports_before_return_without_needing_a_ui_poll() {
    use editbay_delivery::DeliveryControl;
    let directory = tempfile::tempdir().unwrap();
    let mut workspace = Workspace::new(
        directory.path().join("Recovery"),
        eframe::egui::Context::default(),
    );
    let id = workspace.create(Project::new("Captured").unwrap()).unwrap();
    let other = workspace
        .create(Project::new("Independent").unwrap())
        .unwrap();
    let bind = |workspace: &mut Workspace| {
        let (owner, _) = workspace.edit_snapshot(id).unwrap();
        let control = DeliveryControl::new().unwrap();
        workspace.guard_export(owner, control.clone()).unwrap();
        control
    };
    let first = bind(&mut workspace);
    rename(&mut workspace, other, "Other revision");
    rename(&mut workspace, id, "Captured");
    assert!(first.cancellable());
    let (stale, _) = workspace.edit_snapshot(id).unwrap();
    rename(&mut workspace, id, "Revised");
    assert!(!first.cancellable());
    assert!(
        workspace
            .guard_export(stale, DeliveryControl::new().unwrap())
            .is_err()
    );
    let history = bind(&mut workspace);
    workspace.history(id, false).unwrap();
    assert!(!history.cancellable());
    let history = bind(&mut workspace);
    workspace.history(id, true).unwrap();
    assert!(!history.cancellable());
    let background = bind(&mut workspace);
    let (owner, mut editor) = workspace.edit_snapshot(id).unwrap();
    editor
        .apply(
            owner.version,
            "Background rename".into(),
            &[DocumentCommand::RenameProject {
                name: "Background".into(),
            }],
        )
        .unwrap();
    workspace.commit_edit(owner, editor).unwrap();
    assert!(!background.cancellable());
    let saving = bind(&mut workspace);
    let (owner, _) = workspace.edit_snapshot(id).unwrap();
    workspace
        .save(id, owner.version, directory.path().join("Saved.editbay"))
        .unwrap();
    assert!(!saving.cancellable());
    settle(&mut workspace);
    let closing = bind(&mut workspace);
    workspace.close(id, false).unwrap();
    assert!(!closing.cancellable());
}

#[test]
fn one_hundred_inactive_and_untitled_documents_keep_latest_acknowledged_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Recovery");
    let mut workspace = Workspace::new(root.clone(), eframe::egui::Context::default());
    for index in 0..100 {
        let id = workspace
            .create(Project::new(format!("Job {index}")).unwrap())
            .unwrap();
        rename(&mut workspace, id, &format!("Latest {index}"));
    }
    let start = Instant::now();
    loop {
        workspace.poll(Instant::now() + Duration::from_secs(11));
        if workspace
            .tabs
            .iter()
            .all(|tab| tab.recovery_revision == Some(1))
        {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(workspace.errors.is_empty(), "{:?}", workspace.errors);
    drop(workspace);
    let catalog = recovery_catalog(&root).unwrap();
    assert_eq!(catalog.valid.len(), 100);
    for (index, record) in catalog.valid.iter().enumerate() {
        assert_eq!(record.revision, 1);
        let restored = recover_copy(
            &record.path,
            directory.path().join(format!("{index}.editbay")),
        )
        .unwrap();
        assert_eq!(restored.name, record.name);
        assert_ne!(restored.id, record.project_id);
    }
}

#[test]
fn late_preparations_after_close_or_manual_save_never_publish_and_newer_edits_stay_dirty() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Recovery");
    let mut workspace = Workspace::new(root.clone(), eframe::egui::Context::default());
    let closed = workspace.create(Project::new("Closed").unwrap()).unwrap();
    workspace.poll(Instant::now() + Duration::from_secs(11));
    workspace.close(closed, true).unwrap();
    settle(&mut workspace);
    assert!(recovery_catalog(&root).unwrap().valid.is_empty());
    let id = workspace.create(Project::new("Saved").unwrap()).unwrap();
    rename(&mut workspace, id, "Save snapshot");
    workspace.poll(Instant::now() + Duration::from_secs(11));
    let version = DocumentVersion::of(workspace.tabs[0].editor.project());
    let path = directory.path().join("Saved.editbay");
    workspace.save(id, version, path.clone()).unwrap();
    rename(&mut workspace, id, "Newer unsaved edit");
    assert!(workspace.close(id, true).is_err());
    settle(&mut workspace);
    assert_eq!(load(&path).unwrap().name, "Save snapshot");
    assert_eq!(load(&path).unwrap().revision, 1);
    assert_eq!(
        workspace.tabs[0].editor.project().name,
        "Newer unsaved edit"
    );
    assert!(workspace.tabs[0].dirty());
    assert!(recovery_catalog(&root).unwrap().valid.is_empty());
    workspace.poll(Instant::now() + Duration::from_secs(11));
    settle(&mut workspace);
    assert_eq!(recovery_catalog(&root).unwrap().valid[0].revision, 2);
    assert_eq!(
        recovery_catalog(&root).unwrap().valid[0].name,
        "Newer unsaved edit"
    );
}

#[test]
fn checked_save_copy_and_source_loss_recovery_preserve_originals_and_visible_errors() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Recovery");
    let path = directory.path().join("Original.editbay");
    let mut external = Project::new("Original").unwrap();
    save_new(&external, &path).unwrap();
    let mut workspace = Workspace::new(root.clone(), eframe::egui::Context::default());
    workspace.open([path.clone()]);
    workspace.poll(Instant::now());
    settle(&mut workspace);
    let id = workspace.active.unwrap();
    workspace.open([path.clone()]);
    workspace.poll(Instant::now());
    settle(&mut workspace);
    assert_eq!(workspace.tabs.len(), 1);
    rename(&mut workspace, id, "Local revision");
    let expected = DocumentVersion::of(workspace.tabs[0].editor.project());
    let stale = DocumentVersion::of(&external);
    assert!(workspace.save(id, stale, path.clone()).is_err());
    external.rename("Outside revision").unwrap();
    save(&external, &path).unwrap();
    workspace.save(id, expected, path.clone()).unwrap();
    settle(&mut workspace);
    assert_eq!(load(&path).unwrap(), external);
    assert!(workspace.tabs[0].dirty());
    assert!(!workspace.errors.is_empty());
    let copy = directory.path().join("Independent copy.editbay");
    workspace.save_copy(id, expected, copy.clone()).unwrap();
    settle(&mut workspace);
    assert_eq!(load(&copy).unwrap().name, "Local revision");
    assert_ne!(load(&copy).unwrap().id, external.id);
    assert_eq!(workspace.tabs[0].path.as_ref(), Some(&path));
    fs::remove_file(&path).unwrap();
    workspace.save(id, expected, path.clone()).unwrap();
    settle(&mut workspace);
    assert!(!path.exists());
    workspace.poll(Instant::now() + Duration::from_secs(11));
    settle(&mut workspace);
    let snapshot = recovery_catalog(&root).unwrap().valid.pop().unwrap().path;
    let rescued = directory.path().join("Recovered copy.editbay");
    workspace.recover(snapshot, rescued.clone()).unwrap();
    settle(&mut workspace);
    assert_eq!(load(&rescued).unwrap().name, "Local revision");
    assert!(!path.exists());
    assert_eq!(workspace.tabs.len(), 2);
}
