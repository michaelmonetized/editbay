use editbay_app::{media_ui::MediaPane, workspace::Workspace};
use editbay_core::{DocumentCommand, DocumentVersion, Project};
use eframe::egui;
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use tempfile::tempdir;

fn fixture(path: &Path) {
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=24:duration=0.25",
            "-c:v",
            "ffv1",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn poll(
    workspace: &mut Workspace,
    media: &mut MediaPane,
    ctx: &egui::Context,
    mut done: impl FnMut(&MediaPane) -> bool,
) {
    let started = Instant::now();
    while !done(media) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            media.error
        );
        media.poll(workspace, ctx);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn isolated_native_import_uses_normal_undo_and_rejects_late_ownership() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source.mkv");
    fixture(&source);
    let ctx = egui::Context::default();
    let mut workspace = Workspace::new(directory.path().join("recovery"), ctx.clone());
    let tab = workspace
        .create(Project::new("Client edit").unwrap())
        .unwrap();
    let mut media = MediaPane::default();
    media
        .start(
            &workspace,
            tab,
            source.clone(),
            Path::new(env!("CARGO_BIN_EXE_editbay-studio")),
            &ctx,
        )
        .unwrap();
    poll(&mut workspace, &mut media, &ctx, |m| {
        m.inspected().is_some()
    });
    assert!(media.select_streams(&[99]).is_err());
    media.select_streams(&[0]).unwrap();
    media.import_selected().unwrap();
    poll(&mut workspace, &mut media, &ctx, |m| !m.busy());
    assert!(media.error.is_none(), "{:?}", media.error);
    assert_eq!(media.completed_imports, 1);
    assert_eq!(workspace.tabs[0].editor.project().sources.len(), 1);
    assert_eq!(workspace.tabs[0].editor.project().revision, 1);
    workspace.history(tab, false).unwrap();
    assert!(workspace.tabs[0].editor.project().sources.is_empty());
    workspace.history(tab, true).unwrap();
    assert_eq!(workspace.tabs[0].editor.project().sources.len(), 1);
    media
        .start(
            &workspace,
            tab,
            source,
            Path::new(env!("CARGO_BIN_EXE_editbay-studio")),
            &ctx,
        )
        .unwrap();
    poll(&mut workspace, &mut media, &ctx, |m| {
        m.inspected().is_some()
    });
    media.import_selected().unwrap();
    let version = DocumentVersion::of(workspace.tabs[0].editor.project());
    workspace
        .apply(
            tab,
            version,
            "Newer artist edit".into(),
            &[DocumentCommand::RenameProject {
                name: "Keep this edit".into(),
            }],
        )
        .unwrap();
    poll(&mut workspace, &mut media, &ctx, |m| !m.busy());
    assert_eq!(workspace.tabs[0].editor.project().sources.len(), 1);
    assert_eq!(workspace.tabs[0].editor.project().name, "Keep this edit");
    assert!(media.error.as_ref().unwrap().contains("discarded"));
}

#[test]
fn cancellation_and_worker_failure_preserve_the_live_document() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source.mkv");
    fixture(&source);
    let ctx = egui::Context::default();
    let mut workspace = Workspace::new(directory.path().join("recovery"), ctx.clone());
    let tab = workspace
        .create(Project::new("Original edit").unwrap())
        .unwrap();
    let initial = workspace.tabs[0].editor.snapshot();
    let mut media = MediaPane::default();
    media
        .start(
            &workspace,
            tab,
            source,
            Path::new(env!("CARGO_BIN_EXE_editbay-studio")),
            &ctx,
        )
        .unwrap();
    let started = Instant::now();
    media.cancel();
    poll(&mut workspace, &mut media, &ctx, |m| !m.busy());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(workspace.tabs[0].editor.project(), initial.as_ref());
    media
        .start(
            &workspace,
            tab,
            directory.path().join("missing source.mkv"),
            Path::new(env!("CARGO_BIN_EXE_editbay-studio")),
            &ctx,
        )
        .unwrap();
    poll(&mut workspace, &mut media, &ctx, |m| !m.busy());
    assert!(media.error.is_some());
    assert_eq!(workspace.tabs[0].editor.project(), initial.as_ref());
    let source = directory.path().join("source.mkv");
    media
        .start(
            &workspace,
            tab,
            source,
            Path::new(env!("CARGO_BIN_EXE_editbay-studio")),
            &ctx,
        )
        .unwrap();
    poll(&mut workspace, &mut media, &ctx, |m| {
        m.inspected().is_some()
    });
    let worker = media.diagnostic_state()["worker_pid"].as_u64().unwrap();
    assert!(
        Command::new("kill")
            .args(["-KILL", &worker.to_string()])
            .status()
            .unwrap()
            .success()
    );
    poll(&mut workspace, &mut media, &ctx, |m| !m.busy());
    assert!(media.error.as_ref().unwrap().contains("stopped"));
    assert_eq!(workspace.tabs[0].editor.project(), initial.as_ref());
    let (owner, mut candidate) = workspace.edit_snapshot(tab).unwrap();
    candidate
        .apply(
            owner.version,
            "Detached edit".into(),
            &[DocumentCommand::RenameProject {
                name: "Do not revive".into(),
            }],
        )
        .unwrap();
    workspace.close(tab, true).unwrap();
    assert!(workspace.commit_edit(owner, candidate).is_err());
    assert!(workspace.tabs.is_empty());
}
