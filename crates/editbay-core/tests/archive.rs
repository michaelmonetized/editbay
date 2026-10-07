use editbay_core::{
    AssetKind, AssetReference, Project, archive_project, checkpoint, load, recover_copy,
    save_if_unchanged,
};
use sha2::{Digest, Sha256};
use std::fs;

#[test]
fn moved_archives_reopen_save_and_recover_without_the_original_media() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("Original.bin");
    fs::write(&source, b"retained original bytes").unwrap();
    let mut project = Project::new("Portable edit").unwrap();
    project.assets.push(AssetReference {
        id: uuid::Uuid::new_v4(),
        kind: AssetKind::Artwork,
        path: source.clone(),
        sha256: format!("{:x}", Sha256::digest(b"retained original bytes")),
        bytes: 23,
        provenance: "archive storage fixture".into(),
    });
    let archive = root.path().join("Archive");
    let saved = archive_project(&project, &archive).unwrap();
    assert!(
        fs::read_to_string(&saved)
            .unwrap()
            .contains(".editbay-media/")
    );
    assert!(archive_project(&project, &archive).is_err());
    fs::remove_file(&source).unwrap();
    let moved = root.path().join("Moved");
    fs::rename(&archive, &moved).unwrap();
    let saved = moved.join("Project.editbay");
    let original = load(&saved).unwrap();
    assert_eq!(
        fs::read(&original.assets[0].path).unwrap(),
        b"retained original bytes"
    );
    assert_eq!(original.id, project.id);
    let mut revised = original.clone();
    revised.rename("Portable revision").unwrap();
    save_if_unchanged(&revised, &saved, &original).unwrap();
    assert_eq!(load(&saved).unwrap(), revised);
    let snapshot = checkpoint(&revised, Some(&saved), root.path().join("recovery")).unwrap();
    let recovered = recover_copy(snapshot, root.path().join("Recovered.editbay")).unwrap();
    assert_eq!(
        fs::read(&recovered.assets[0].path).unwrap(),
        b"retained original bytes"
    );
    fs::write(&revised.assets[0].path, b"changed archive bytes").unwrap();
    let rejected = root.path().join("Rejected");
    assert!(archive_project(&revised, &rejected).is_err());
    assert!(!rejected.join("Project.editbay").exists());
}
