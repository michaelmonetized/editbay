use editbay_app::{
    catalog::{CatalogEvent, CatalogScan},
    theme::Palette,
};
use editbay_core::{Project, save_new};
use std::{fs, time::Duration};

#[test]
fn incremental_catalog_finds_deep_nested_brands_and_visible_invalid_work() {
    let directory = tempfile::tempdir().unwrap();
    let mut nested = directory.path().join("Client");
    fs::create_dir_all(nested.join(".omabrand")).unwrap();
    let mut project = Project::new("Horizontal reference").unwrap();
    save_new(&project, nested.join("First.editbay")).unwrap();
    for _ in 0..32 {
        nested.push("Nested");
    }
    fs::create_dir_all(nested.join(".omabrand")).unwrap();
    project.rename("Deep project").unwrap();
    save_new(&project, nested.join("Deep.editbay")).unwrap();
    fs::write(directory.path().join("Broken.editbay"), b"{").unwrap();
    for name in [".hidden", "Trash"] {
        fs::create_dir(directory.path().join(name)).unwrap();
        save_new(
            &project,
            directory.path().join(name).join("Skipped.editbay"),
        )
        .unwrap();
    }
    std::os::unix::fs::symlink(directory.path(), nested.join("Loop")).unwrap();
    std::os::unix::fs::symlink(
        nested.join("Deep.editbay"),
        directory.path().join("Link.editbay"),
    )
    .unwrap();
    let scan = CatalogScan::start(directory.path().into()).unwrap();
    let mut documents = Vec::new();
    let mut folders = Vec::new();
    loop {
        match scan.events.recv_timeout(Duration::from_secs(2)).unwrap() {
            CatalogEvent::Document(document) => documents.push(document),
            CatalogEvent::Folder(folder) => folders.push(folder),
            CatalogEvent::Warning { error, .. } => panic!("{error}"),
            CatalogEvent::Finished { complete, .. } => {
                assert!(complete);
                break;
            }
        }
    }
    assert_eq!(documents.len(), 3);
    assert_eq!(folders.len(), 2);
    assert!(
        documents
            .iter()
            .any(|d| d.name == "Deep project" && d.version.unwrap().revision == 1)
    );
    assert_eq!(documents.iter().filter(|d| d.error.is_some()).count(), 1);
    assert!(folders.iter().any(|f| f.path == nested));
}

#[test]
fn cancellation_stops_a_backpressured_scan_and_missing_roots_report_incomplete() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::new("Document").unwrap();
    for index in 0..100 {
        save_new(&project, directory.path().join(format!("{index}.editbay"))).unwrap();
    }
    let scan = CatalogScan::start(directory.path().into()).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    scan.cancel();
    loop {
        match scan.events.recv_timeout(Duration::from_secs(1)) {
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(error) => panic!("Worker did not cancel: {error}"),
        }
    }
    let scan = CatalogScan::start(directory.path().join("Missing")).unwrap();
    assert!(matches!(
        scan.events.recv_timeout(Duration::from_secs(1)).unwrap(),
        CatalogEvent::Warning { .. }
    ));
    assert!(matches!(
        scan.events.recv_timeout(Duration::from_secs(1)).unwrap(),
        CatalogEvent::Finished {
            complete: false,
            ..
        }
    ));
}

#[test]
fn desktop_palettes_select_readable_light_and_dark_surfaces_and_reject_bad_colors() {
    let dark =
        Palette::parse("background='#1e1e2e'\nforeground='#cdd6f4'\naccent='#89b4fa'").unwrap();
    let light =
        Palette::parse("background='#ffffff'\nforeground='#202020'\naccent='#0055aa'").unwrap();
    assert!(dark.dark);
    assert!(!light.dark);
    assert_ne!(dark.panel, light.panel);
    assert_ne!(dark.foreground, dark.background);
    assert_ne!(light.foreground, light.background);
    assert!(Palette::parse("background='invalid'\nforeground='#ffffff'").is_none());
    let ctx = eframe::egui::Context::default();
    dark.apply(&ctx);
    assert_eq!(
        ctx.style_of(eframe::egui::Theme::Dark).visuals.panel_fill,
        dark.panel
    );
    light.apply(&ctx);
    assert_eq!(
        ctx.style_of(eframe::egui::Theme::Light).visuals.panel_fill,
        light.panel
    );
}
