use serde_json::Value;
use std::{
    ffi::OsStr,
    fs,
    process::{Command, Output},
};
use tempfile::tempdir;

fn run(args: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_editbay"))
        .args(args)
        .output()
        .unwrap()
}

fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn project_edit_checkpoint_and_source_loss_recovery_work_through_the_binary() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("Client Project/cut.editbay");
    let root = directory.path().join("Recovery Library");
    let destination = directory.path().join("Recovered/copy.editbay");
    let created = json(run(&[
        "new".as_ref(),
        source.as_os_str(),
        "Client spot".as_ref(),
    ]));
    assert_eq!(created["name"], "Client spot");
    assert_eq!(created["revision"], 0);
    assert_eq!(json(run(&["info".as_ref(), source.as_os_str()])), created);
    let edited = json(run(&[
        "rename".as_ref(),
        source.as_os_str(),
        "Revised spot".as_ref(),
    ]));
    assert_eq!(edited["id"], created["id"]);
    assert_eq!(edited["revision"], 1);
    let snapshot = json(run(&[
        "checkpoint".as_ref(),
        source.as_os_str(),
        root.as_os_str(),
    ]));
    let checkpoint = OsStr::new(snapshot["checkpoint"].as_str().unwrap());
    let catalog = json(run(&["recoveries".as_ref(), root.as_os_str()]));
    assert_eq!(catalog["valid"].as_array().unwrap().len(), 1);
    assert!(catalog["invalid"].as_array().unwrap().is_empty());
    fs::remove_file(&source).unwrap();
    assert!(
        !run(&["recover".as_ref(), checkpoint, source.as_os_str()])
            .status
            .success()
    );
    assert!(!source.exists());
    let recovered = json(run(&[
        "recover".as_ref(),
        checkpoint,
        destination.as_os_str(),
    ]));
    assert_eq!(recovered["name"], "Revised spot");
    assert_ne!(recovered["id"], edited["id"]);
    assert_eq!(recovered["recovered_from"], edited["id"]);
    let preserved = fs::read(&destination).unwrap();
    assert!(
        !run(&["recover".as_ref(), checkpoint, destination.as_os_str()])
            .status
            .success()
    );
    assert!(
        !run(&["new".as_ref(), destination.as_os_str(), "Other".as_ref()])
            .status
            .success()
    );
    assert_eq!(fs::read(&destination).unwrap(), preserved);
}

#[test]
fn invalid_commands_and_names_report_failure_without_creating_work() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("cut.editbay");
    let output = run(&["new".as_ref(), path.as_os_str(), "   ".as_ref()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("nonempty name"));
    assert!(!path.exists());
    let output = run(&["render".as_ref()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid command"));
    let output = run(&["--version".as_ref()]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Rust project foundation"));
}
