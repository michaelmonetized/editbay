use editbay_core::{
    ContentLight, DocumentCommand, DocumentEditor, DocumentVersion, HdrMetadata, MasteringDisplay,
    Project,
};

fn metadata() -> HdrMetadata {
    HdrMetadata {
        mastering_display: Some(MasteringDisplay {
            primaries_xy_50000: [[35400, 14600], [8500, 39850], [6550, 2300]],
            white_xy_50000: [15635, 16450],
            minimum_luminance_10000: 50,
            maximum_luminance_10000: 10_000_000,
        }),
        content_light: Some(ContentLight {
            maximum_content_light_nits: 1000,
            maximum_frame_average_light_nits: 400,
        }),
    }
}

#[test]
fn hdr_static_metadata_uses_exact_units_and_retains_undo_reopen_and_legacy_defaults() {
    let mut editor = DocumentEditor::new(Project::new("Static HDR declarations").unwrap()).unwrap();
    let mut configuration = editor.project().color;
    configuration.hdr.metadata = metadata();
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Declare static HDR metadata".into(),
            &[DocumentCommand::SetColor { configuration }],
        )
        .unwrap();
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert!(editor.project().color.hdr.metadata.is_empty());
    editor.redo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().color.hdr.metadata, metadata());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    let native = directory.path().join("declared.editbay");
    editbay_core::save_new(editor.project(), &native).unwrap();
    assert_eq!(editbay_core::load(&native).unwrap(), *editor.project());
    let mut legacy = serde_json::to_value(configuration.hdr).unwrap();
    legacy.as_object_mut().unwrap().remove("metadata");
    assert!(
        serde_json::from_value::<editbay_core::HdrConfiguration>(legacy)
            .unwrap()
            .metadata
            .is_empty()
    );
}

#[test]
fn hdr_static_metadata_rejects_impossible_units_and_preserves_unknown_light_values() {
    metadata().validate().unwrap();
    let valid = metadata().mastering_display.unwrap();
    for display in [
        MasteringDisplay {
            white_xy_50000: [50000, 50000],
            ..valid
        },
        MasteringDisplay {
            primaries_xy_50000: [[1, 1]; 3],
            ..valid
        },
        MasteringDisplay {
            primaries_xy_50000: [[0, u32::MAX], [u32::MAX, 0], [u32::MAX, u32::MAX]],
            ..valid
        },
        MasteringDisplay {
            minimum_luminance_10000: 10_000_000,
            ..valid
        },
        MasteringDisplay {
            maximum_luminance_10000: 100_000_001,
            ..valid
        },
    ] {
        assert!(
            HdrMetadata {
                mastering_display: Some(display),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    for light in [
        ContentLight {
            maximum_content_light_nits: 10001,
            maximum_frame_average_light_nits: 400,
        },
        ContentLight {
            maximum_content_light_nits: 100,
            maximum_frame_average_light_nits: 400,
        },
    ] {
        assert!(
            HdrMetadata {
                content_light: Some(light),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    HdrMetadata {
        content_light: Some(ContentLight {
            maximum_content_light_nits: 0,
            maximum_frame_average_light_nits: 0,
        }),
        ..Default::default()
    }
    .validate()
    .unwrap();
}
