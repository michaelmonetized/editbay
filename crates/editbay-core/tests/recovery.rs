use editbay_core::{
    Error, FrameRate, Project, checkpoint, load, recover_copy, recovery_catalog, save,
    save_if_unchanged, save_new,
};
use fs2::FileExt;
use std::fs;
use tempfile::tempdir;

#[test]
fn rational_time_has_no_fractional_rate_drift() {
    let rate = FrameRate::new(30_000, 1001).unwrap();
    assert_eq!(rate.frame_nanoseconds(30_000).unwrap(), 1_001_000_000_000);
    assert!(FrameRate::new(24, 0).is_err());
    assert!(FrameRate::new(0, 1).is_err());
    assert!(rate.frame_nanoseconds(u64::MAX).is_ok());
}

#[test]
fn save_reopen_preserves_identity_revision_and_profiles() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("client/cut.editbay");
    let mut project = Project::new("Client spot").unwrap();
    project.rename("Client spot v2").unwrap();
    project.sequences[0].frame_rate = FrameRate::new(24_000, 1001).unwrap();
    save_new(&project, &path).unwrap();
    assert_eq!(load(&path).unwrap(), project);
    assert_eq!(project.revision, 1);
    project.rename("Client spot v2").unwrap();
    assert_eq!(project.revision, 1);
}

#[test]
fn invalid_edits_cannot_replace_a_valid_saved_project() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("cut.editbay");
    let mut project = Project::new("Good").unwrap();
    save(&project, &path).unwrap();
    let original = fs::read(&path).unwrap();
    project.sequences[0].frame_rate.denominator = 0;
    assert!(save(&project, &path).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn creation_refuses_to_replace_existing_work() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("cut.editbay");
    let original = Project::new("Original").unwrap();
    save_new(&original, &path).unwrap();
    assert!(matches!(
        save_new(&Project::new("Other").unwrap(), &path),
        Err(Error::Exists(_))
    ));
    assert_eq!(load(&path).unwrap(), original);
}

#[test]
fn stale_edits_cannot_replace_newer_work_or_recreate_a_deleted_project() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("cut.editbay");
    let original = Project::new("Original").unwrap();
    save_new(&original, &path).unwrap();
    let mut newer = original.clone();
    newer.rename("Newer edit").unwrap();
    save_if_unchanged(&newer, &path, &original).unwrap();
    let mut stale = original.clone();
    stale.rename("Stale edit").unwrap();
    assert!(matches!(
        save_if_unchanged(&stale, &path, &original),
        Err(Error::Conflict(_))
    ));
    assert_eq!(load(&path).unwrap(), newer);
    fs::remove_file(&path).unwrap();
    assert!(matches!(
        save_if_unchanged(&stale, &path, &original),
        Err(Error::Conflict(_))
    ));
    assert!(!path.exists());
}

#[test]
fn checked_saves_require_identity_and_revision_continuity() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("cut.editbay");
    let original = Project::new("Original").unwrap();
    save_new(&original, &path).unwrap();
    assert!(save_if_unchanged(&Project::new("Other").unwrap(), &path, &original).is_err());
    let mut unversioned = original.clone();
    unversioned.name = "Changed without revision".into();
    assert!(save_if_unchanged(&unversioned, &path, &original).is_err());
    assert_eq!(load(&path).unwrap(), original);
    save_if_unchanged(&original, &path, &original).unwrap();
}

#[test]
fn checkpoint_survives_source_loss_and_recovers_to_an_independent_copy() {
    let directory = tempdir().unwrap();
    let original = directory.path().join("cut.editbay");
    let root = directory.path().join("recovery");
    let mut project = Project::new("Saved").unwrap();
    save(&project, &original).unwrap();
    project.rename("Recovered edit").unwrap();
    let snapshot = checkpoint(&project, Some(&original), &root).unwrap();
    fs::remove_file(&original).unwrap();
    assert!(matches!(
        recover_copy(&snapshot, &original),
        Err(Error::OriginalDestination)
    ));
    let copy = directory.path().join("Recovered.editbay");
    let recovered = recover_copy(&snapshot, &copy).unwrap();
    assert_eq!(recovered.name, "Recovered edit");
    assert_eq!(recovered.revision, project.revision);
    assert_eq!(recovered.recovered_from, Some(project.id));
    assert_ne!(recovered.id, project.id);
    assert_eq!(load(copy).unwrap(), recovered);
    assert!(snapshot.exists());
}

#[test]
fn untitled_project_can_be_checkpointed_without_a_saved_original() {
    let directory = tempdir().unwrap();
    let project = Project::new("Untitled work").unwrap();
    let snapshot = checkpoint(&project, None, directory.path()).unwrap();
    let catalog = recovery_catalog(directory.path()).unwrap();
    assert_eq!(catalog.valid.len(), 1);
    assert!(catalog.valid[0].original.is_none());
    assert_eq!(catalog.valid[0].project_id, project.id);
    assert!(snapshot.exists());
}

#[test]
fn corrupt_latest_checkpoint_keeps_older_recoverable_work_visible() {
    let directory = tempdir().unwrap();
    let mut project = Project::new("Earlier").unwrap();
    let earlier = checkpoint(&project, None, directory.path()).unwrap();
    project.rename("Later").unwrap();
    let later = checkpoint(&project, None, directory.path()).unwrap();
    fs::write(&later, b"interrupted JSON").unwrap();
    let catalog = recovery_catalog(directory.path()).unwrap();
    assert_eq!(catalog.valid.len(), 1);
    assert_eq!(catalog.valid[0].path, earlier);
    assert_eq!(catalog.invalid.len(), 1);
    assert_eq!(catalog.invalid[0].path, later);
    let copy = directory.path().join("copy.editbay");
    assert_eq!(recover_copy(earlier, copy).unwrap().name, "Earlier");
}

#[test]
fn changed_checkpoint_payload_fails_integrity_without_creating_a_file() {
    let directory = tempdir().unwrap();
    let project = Project::new("Keep").unwrap();
    let snapshot = checkpoint(&project, None, directory.path()).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&snapshot).unwrap()).unwrap();
    value["snapshot"]["project"]["name"] = "Changed".into();
    fs::write(&snapshot, serde_json::to_vec(&value).unwrap()).unwrap();
    let destination = directory.path().join("copy.editbay");
    assert!(matches!(
        recover_copy(snapshot, &destination),
        Err(Error::Integrity)
    ));
    assert!(!destination.exists());
}

#[test]
fn recovery_does_not_overwrite_another_project() {
    let directory = tempdir().unwrap();
    let snapshot = checkpoint(&Project::new("Recover").unwrap(), None, directory.path()).unwrap();
    let destination = directory.path().join("other.editbay");
    let other = Project::new("Other client").unwrap();
    save(&other, &destination).unwrap();
    assert!(matches!(
        recover_copy(snapshot, &destination),
        Err(Error::Exists(_))
    ));
    assert_eq!(load(destination).unwrap(), other);
}

#[test]
fn incomplete_temporary_files_are_not_recovery_candidates() {
    let directory = tempdir().unwrap();
    let snapshot = checkpoint(&Project::new("Good").unwrap(), None, directory.path()).unwrap();
    fs::write(
        snapshot.parent().unwrap().join("interrupted.tmp"),
        b"unfinished",
    )
    .unwrap();
    let catalog = recovery_catalog(directory.path()).unwrap();
    assert_eq!(catalog.valid.len(), 1);
    assert!(catalog.invalid.is_empty());
}

#[test]
fn newer_or_legacy_schemas_fail_explicitly() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("unknown.editbay");
    let mut project = Project::new("Future").unwrap();
    for schema in [0, 2] {
        project.schema = schema;
        fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
        assert!(matches!(load(&path), Err(Error::Schema(value)) if value == schema));
    }
}

#[test]
fn duplicate_ids_and_empty_sequences_are_invalid() {
    let mut project = Project::new("Valid").unwrap();
    project.sequences.push(project.sequences[0].clone());
    assert!(project.validate().is_err());
    project.sequences.clear();
    assert!(project.validate().is_err());
}

#[test]
fn a_competing_writer_is_reported_without_touching_saved_work() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("cut.editbay");
    let project = Project::new("Saved").unwrap();
    save(&project, &path).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.path().join(".cut.editbay.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    assert!(matches!(
        save(&Project::new("Competing").unwrap(), &path),
        Err(Error::Busy(_))
    ));
    assert_eq!(load(path).unwrap(), project);
}

#[cfg(unix)]
#[test]
fn symlink_destinations_are_rejected_and_permissions_survive_save() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempdir().unwrap();
    let path = directory.path().join("cut.editbay");
    let project = Project::new("Saved").unwrap();
    save(&project, &path).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    save(&project, &path).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let alias = directory.path().join("alias.editbay");
    symlink(&path, &alias).unwrap();
    assert!(save(&Project::new("Changed").unwrap(), alias).is_err());
    assert_eq!(load(path).unwrap(), project);
}
