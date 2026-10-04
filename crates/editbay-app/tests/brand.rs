use editbay_app::brand::{self, Category, Metadata, Palette};
use std::{fs, sync::atomic::AtomicBool};

#[test]
fn shared_bank_roundtrips_bytes_palettes_and_client_metadata_without_replacing_omadesign() {
    let directory = tempfile::tempdir().unwrap();
    let bank = brand::create(directory.path(), "Client bank").unwrap();
    let manifest = bank.join("brand.json");
    let original_manifest = fs::read(&manifest).unwrap();
    let shared: serde_json::Value = serde_json::from_slice(&original_manifest).unwrap();
    assert_eq!(shared["version"], 1);
    assert_eq!(shared["name"], "Client bank");
    let cancel = AtomicBool::new(false);
    let catalog = brand::scan(&bank, &cancel).unwrap();
    let mut metadata = catalog.metadata.unwrap();
    metadata.client = "Mack's Shack".into();
    metadata.project = "Commercial".into();
    metadata.palettes.push(Palette {
        name: "Primary".into(),
        colors: vec!["#ff5c00".into(), "#161616".into()],
    });
    assert_eq!(
        brand::save_metadata(&bank, catalog.version.as_ref().unwrap(), metadata.clone()).unwrap(),
        1
    );
    assert!(brand::save_metadata(&bank, catalog.version.as_ref().unwrap(), metadata).is_err());
    let source = directory.path().join("Logo.svg");
    let bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\" height=\"32\"><rect width=\"32\" height=\"32\" fill=\"#ff5c00\"/></svg>";
    fs::write(&source, bytes).unwrap();
    let catalog = brand::scan(&bank, &cancel).unwrap();
    let receipt = brand::import(
        &bank,
        catalog.version.as_ref().unwrap(),
        &source,
        Category::Logo,
        &cancel,
    )
    .unwrap();
    let catalog = brand::scan(&bank, &cancel).unwrap();
    assert!(catalog.complete);
    assert!(catalog.warnings.is_empty());
    assert_eq!(catalog.assets.len(), 1);
    assert_eq!(catalog.assets[0].verified_sha256, receipt.sha256);
    assert_eq!(catalog.metadata.as_ref().unwrap().revision, 2);
    assert_eq!(catalog.metadata.as_ref().unwrap().client, "Mack's Shack");
    assert!(
        brand::import(
            &bank,
            catalog.version.as_ref().unwrap(),
            &source,
            Category::Logo,
            &cancel
        )
        .is_err()
    );
    let exported = directory.path().join("exported.svg");
    brand::export(&bank, &catalog.assets[0], &exported, &cancel).unwrap();
    assert!(brand::export(&bank, &catalog.assets[0], &exported, &cancel).is_err());
    assert_eq!(fs::read(&exported).unwrap(), bytes);
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert_eq!(fs::read(&manifest).unwrap(), original_manifest);
    fs::write(bank.join(&receipt.path), b"<svg/>").unwrap();
    assert!(
        brand::export(
            &bank,
            &catalog.assets[0],
            &directory.path().join("changed.svg"),
            &cancel
        )
        .is_err()
    );
    let changed = brand::scan(&bank, &cancel).unwrap();
    assert!(
        changed
            .warnings
            .iter()
            .any(|warning| warning.contains("changed since import"))
    );
    assert!(!directory.path().join("changed.svg").exists());
}

#[test]
fn unknown_shared_manifests_legacy_markers_and_invalid_namespaced_metadata_are_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let bank = directory.path().join(".omabrand");
    fs::create_dir(&bank).unwrap();
    let future = br#"{"version":19,"name":"Future Oma","custom":[1,2,3]}"#;
    fs::write(bank.join("brand.json"), future).unwrap();
    brand::create(directory.path(), "Replacement").unwrap();
    let cancel = AtomicBool::new(false);
    let catalog = brand::scan(&bank, &cancel).unwrap();
    assert!(
        catalog
            .warnings
            .iter()
            .any(|warning| warning.contains("Unknown shared"))
    );
    assert_eq!(fs::read(bank.join("brand.json")).unwrap(), future);
    fs::write(bank.join("editbay.v1.json"), b"{broken").unwrap();
    let catalog = brand::scan(&bank, &cancel).unwrap();
    assert!(catalog.metadata.is_none());
    assert!(catalog.version.is_none());
    assert_eq!(fs::read(bank.join("editbay.v1.json")).unwrap(), b"{broken");
    assert!(brand::create(&bank, "Repair").is_err());
    let legacy = directory.path().join("Legacy");
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join(".omabrand"), b"legacy project context").unwrap();
    assert!(brand::create(&legacy, "New bank").is_err());
    assert_eq!(
        fs::read(legacy.join(".omabrand")).unwrap(),
        b"legacy project context"
    );
    let mut metadata = Metadata::default();
    metadata.palettes.push(Palette {
        name: "Invalid".into(),
        colors: vec!["orange".into()],
    });
    assert!(metadata.validate().is_err());
}

#[test]
fn cancellation_symlinks_and_replaced_bank_ownership_cannot_publish() {
    let directory = tempfile::tempdir().unwrap();
    let bank = brand::create(directory.path(), "Client").unwrap();
    let cancel = AtomicBool::new(false);
    let catalog = brand::scan(&bank, &cancel).unwrap();
    let source = directory.path().join("Font.ttf");
    fs::write(&source, b"stored font bytes").unwrap();
    assert!(
        brand::import(
            &bank,
            catalog.version.as_ref().unwrap(),
            &source,
            Category::Font,
            &AtomicBool::new(true)
        )
        .is_err()
    );
    assert!(!bank.join("fonts/Font.ttf").exists());
    let outside = directory.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, bank.join("fonts")).unwrap();
    assert!(
        brand::import(
            &bank,
            catalog.version.as_ref().unwrap(),
            &source,
            Category::Font,
            &cancel
        )
        .is_err()
    );
    assert!(!outside.join("Font.ttf").exists());
    assert!(brand::scan(&bank, &cancel).unwrap().assets.is_empty());
    fs::rename(&bank, directory.path().join("Old bank")).unwrap();
    fs::create_dir(&bank).unwrap();
    assert!(
        brand::import(
            &bank,
            catalog.version.as_ref().unwrap(),
            &source,
            Category::Font,
            &cancel
        )
        .is_err()
    );
    assert!(
        brand::save_metadata(
            &bank,
            catalog.version.as_ref().unwrap(),
            Metadata::default()
        )
        .is_err()
    );
    assert!(!bank.join("fonts/Font.ttf").exists());
    assert_eq!(fs::read(&source).unwrap(), b"stored font bytes");
}
