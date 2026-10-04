use editbay_core::{Project, prepare_checkpoint, recover_copy, recovery_catalog};

#[test]
fn recovery_preparation_is_invisible_until_owned_commit_and_stale_drop_removes_it() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Recovery");
    let project = Project::new("Unsaved work").unwrap();
    let stale = prepare_checkpoint(&project, None, &root).unwrap();
    assert!(recovery_catalog(&root).unwrap().valid.is_empty());
    assert_eq!(
        std::fs::read_dir(root.join(project.id.to_string()))
            .unwrap()
            .count(),
        1
    );
    drop(stale);
    assert_eq!(
        std::fs::read_dir(root.join(project.id.to_string()))
            .unwrap()
            .count(),
        0
    );
    let prepared = prepare_checkpoint(&project, None, &root).unwrap();
    let path = prepared.commit().unwrap();
    assert_eq!(recovery_catalog(&root).unwrap().valid.len(), 1);
    let recovered = recover_copy(path, directory.path().join("Recovered.editbay")).unwrap();
    assert_eq!(recovered.name, project.name);
    assert_eq!(recovered.sequences, project.sequences);
}

#[test]
fn deleted_original_directory_does_not_prevent_rescuing_dirty_work_or_allow_original_recreation() {
    let directory = tempfile::tempdir().unwrap();
    let source_directory = directory.path().join("Lost/Client");
    std::fs::create_dir_all(&source_directory).unwrap();
    let source = source_directory.join("Original.editbay");
    let project = Project::new("Unsaved revision").unwrap();
    editbay_core::save_new(&project, &source).unwrap();
    std::fs::remove_dir_all(directory.path().join("Lost")).unwrap();
    let snapshot = prepare_checkpoint(&project, Some(&source), directory.path().join("Recovery"))
        .unwrap()
        .commit()
        .unwrap();
    assert!(recover_copy(&snapshot, &source).is_err());
    assert!(!source.exists());
    let rescued = recover_copy(snapshot, directory.path().join("Rescued.editbay")).unwrap();
    assert_eq!(rescued.name, project.name);
}
#[test]
fn bounded_reads_reject_large_documents_without_replacing_the_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Project.editbay");
    let project = editbay_core::Project::new("Bounded native read").unwrap();
    editbay_core::save_new(&project, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        editbay_core::load_bounded(&path, bytes.len() as u64).unwrap(),
        project
    );
    assert!(editbay_core::load_bounded(&path, bytes.len() as u64 - 1).is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
