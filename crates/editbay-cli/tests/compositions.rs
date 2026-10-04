use editbay_core::{
    ColorConfiguration, CommandGroup, Composition, DocumentCommand, DocumentVersion, FrameRange,
    NodeOperation, Project, TimedNode, load, save_new,
};
use std::{fs, process::Command};

#[test]
fn real_cli_builds_inspects_and_checkpoints_editable_composition_and_rejects_stale_requests() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("Actual CLI scene.editbay");
    let input = directory.path().join("Commands.json");
    let project = Project::new("CLI scene").unwrap();
    save_new(&project, &file).unwrap();
    let mut composition: Composition = serde_json::from_value(serde_json::json!({
        "id":"00000000-0000-0000-0000-000000000010","name":"Editable picture","width":1920,"height":1080,
        "frame_rate":{"numerator":24,"denominator":1},"duration":48,"tracks":[],"nodes":[],"picture":null,"audio":null
    })).unwrap();
    let node_id = "00000000-0000-0000-0000-000000000011".parse().unwrap();
    composition.nodes.push(TimedNode {
        id: node_id,
        range: FrameRange { start: 0, end: 48 },
        operation: NodeOperation::Solid {
            rgba: [1.0, 0.1, 0.0, 1.0],
        },
        animation: vec![],
    });
    composition.picture = Some(node_id);
    let mut sequence = project.sequences[0].clone();
    sequence.composition = Some(composition.id);
    let group = CommandGroup {
        expected: DocumentVersion::of(&project),
        label: "Author scene".into(),
        commands: vec![
            DocumentCommand::SetComposition {
                composition: composition.clone(),
            },
            DocumentCommand::SetSequence { sequence },
            DocumentCommand::SetColor {
                configuration: ColorConfiguration::default(),
            },
        ],
    };
    fs::write(&input, serde_json::to_vec(&group).unwrap()).unwrap();
    let run = |args: &[&std::ffi::OsStr]| {
        Command::new(env!("CARGO_BIN_EXE_editbay"))
            .args(args)
            .output()
            .unwrap()
    };
    let applied = run(&["apply".as_ref(), file.as_os_str(), input.as_os_str()]);
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(receipt["receipt"]["after"]["revision"], 1);
    let saved = fs::read(&file).unwrap();
    assert!(
        !run(&["apply".as_ref(), file.as_os_str(), input.as_os_str()])
            .status
            .success()
    );
    assert_eq!(fs::read(&file).unwrap(), saved);
    let composition_id = composition.id.to_string();
    let plan = run(&[
        "frame-plan".as_ref(),
        file.as_os_str(),
        composition_id.as_ref(),
        "24".as_ref(),
    ]);
    assert!(plan.status.success());
    let value: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(value["nodes"][0]["operation"]["rgba"][0], 1.0);
    assert_eq!(value["version"]["revision"], 1);
    let recovery = directory.path().join("Recovery");
    assert!(
        run(&[
            "checkpoint".as_ref(),
            file.as_os_str(),
            recovery.as_os_str()
        ])
        .status
        .success()
    );
    assert_eq!(load(&file).unwrap().compositions, vec![composition]);
}

#[test]
fn real_cli_migration_writes_a_separate_destination_and_preserves_legacy_source() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("Legacy.editbay");
    let target = directory.path().join("Migrated.editbay");
    let bytes = include_bytes!("../../../docs/evidence/r0-reference/fixtures/foundation.editbay");
    fs::write(&source, bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_editbay"))
        .arg("migrate")
        .arg(&source)
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["from_schema"], 1);
    assert_eq!(report["to_schema"], 2);
    assert_eq!(load(&source).unwrap(), load(&target).unwrap());
    assert_eq!(fs::read(&source).unwrap(), bytes);
    let migrated = fs::read(&target).unwrap();
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_editbay"))
            .arg("migrate")
            .arg(&source)
            .arg(&target)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(fs::read(&target).unwrap(), migrated);
}
