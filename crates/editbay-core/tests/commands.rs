use editbay_core::{DocumentCommand, DocumentEditor, DocumentVersion, Error, Project};

fn rename(name: &str) -> DocumentCommand {
    DocumentCommand::RenameProject { name: name.into() }
}

#[test]
fn groups_are_atomic_and_undo_redo_never_reuse_an_old_revision() {
    let original = Project::new("Original").unwrap();
    let mut editor = DocumentEditor::new(original.clone()).unwrap();
    let before = DocumentVersion::of(editor.project());
    assert!(
        editor
            .apply(
                before,
                "Invalid group".into(),
                &[rename("Intermediate"), rename(" ")]
            )
            .is_err()
    );
    assert_eq!(editor.project(), &original);
    assert_eq!(editor.history(), (None, None));
    let receipt = editor
        .apply(
            before,
            "Rename twice".into(),
            &[rename("Intermediate"), rename("Final")],
        )
        .unwrap();
    assert_eq!(receipt.after.revision, 1);
    assert_eq!(editor.project().name, "Final");
    assert!(matches!(
        editor.undo(before),
        Err(Error::StaleCommand { .. })
    ));
    editor.undo(receipt.after).unwrap();
    assert_eq!(editor.project().revision, 2);
    assert_eq!(editor.project().name, "Original");
    editor.redo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().revision, 3);
    assert_eq!(editor.project().name, "Final");
}

#[test]
fn no_op_preserves_history_and_new_edits_discard_the_redo_branch() {
    let mut editor = DocumentEditor::new(Project::new("A").unwrap()).unwrap();
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "First".into(),
            &[rename("B")],
        )
        .unwrap();
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    let receipt = editor
        .apply(
            DocumentVersion::of(editor.project()),
            "No change".into(),
            &[rename("C"), rename("A")],
        )
        .unwrap();
    assert!(!receipt.changed);
    assert_eq!(editor.history(), (None, Some("First")));
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Branch".into(),
            &[rename("D")],
        )
        .unwrap();
    assert_eq!(editor.history(), (Some("Branch"), None));
    assert!(editor.redo(DocumentVersion::of(editor.project())).is_err());
    let wrong = DocumentVersion::of(&Project::new("Other").unwrap());
    assert!(
        editor
            .apply(wrong, "Wrong owner".into(), &[rename("X")])
            .is_err()
    );
    assert_eq!(editor.project().name, "D");
}

#[test]
fn a_group_consumes_one_revision_at_the_counter_boundary_and_undo_failure_preserves_state() {
    let mut project = Project::new("A").unwrap();
    project.revision = u64::MAX - 1;
    let mut editor = DocumentEditor::new(project).unwrap();
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Last revision".into(),
            &[rename("B"), rename("C")],
        )
        .unwrap();
    assert_eq!(editor.project().revision, u64::MAX);
    assert_eq!(editor.project().name, "C");
    let before = editor.project().clone();
    assert!(editor.undo(DocumentVersion::of(editor.project())).is_err());
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.history(), (Some("Last revision"), None));
}
