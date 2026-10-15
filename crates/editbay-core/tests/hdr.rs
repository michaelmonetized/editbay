use editbay_core::{
    ColorConfiguration, DocumentCommand, DocumentEditor, DocumentVersion, HdrConfiguration,
    OutputTransfer, Project, WorkingGamut,
};

#[test]
fn craft_video_hdr_defaults_validate_undo_and_reopen_through_shared_commands() {
    let legacy = serde_json::to_value(ColorConfiguration::default()).unwrap();
    let mut without_hdr = legacy.clone();
    without_hdr.as_object_mut().unwrap().remove("hdr");
    assert_eq!(
        serde_json::from_value::<ColorConfiguration>(without_hdr).unwrap(),
        ColorConfiguration::default()
    );
    let project = Project::new("Explicit absolute color").unwrap();
    let mut editor = DocumentEditor::new(project).unwrap();
    let mut color = editor.project().color;
    color.hdr = HdrConfiguration {
        reference_white_nits: 203,
        peak_nits: 1000,
    };
    color.output_gamut = WorkingGamut::Bt2020;
    color.output_transfer = OutputTransfer::Pq;
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "HDR output".into(),
            &[DocumentCommand::SetColor {
                configuration: color,
            }],
        )
        .unwrap();
    assert_eq!(editor.project().color, color);
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().color, ColorConfiguration::default());
    editor.redo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().color, color);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("color.editbay");
    editbay_core::save_new(editor.project(), &path).unwrap();
    assert_eq!(editbay_core::load(&path).unwrap(), *editor.project());
    for invalid in [
        HdrConfiguration {
            reference_white_nits: 0,
            peak_nits: 1000,
        },
        HdrConfiguration {
            reference_white_nits: 1001,
            peak_nits: 1000,
        },
        HdrConfiguration {
            reference_white_nits: 203,
            peak_nits: 10001,
        },
    ] {
        let revision = DocumentVersion::of(editor.project());
        let mut rejected = color;
        rejected.hdr = invalid;
        assert!(
            editor
                .apply(
                    revision,
                    "Invalid HDR".into(),
                    &[DocumentCommand::SetColor {
                        configuration: rejected
                    }]
                )
                .is_err()
        );
        assert_eq!(DocumentVersion::of(editor.project()), revision);
        assert_eq!(editor.project().color, color);
    }
}
