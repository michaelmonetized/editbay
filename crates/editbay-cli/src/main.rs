use editbay_core::{
    DocumentCommand, DocumentEditor, DocumentVersion, Project, checkpoint, load, recover_copy,
    recovery_catalog, save_if_unchanged, save_new,
};
use std::{ffi::OsString, path::Path, process::ExitCode};

const HELP: &str = "EditBay Rust project foundation

Usage:
  editbay new FILE NAME
  editbay info FILE
  editbay rename FILE NAME
  editbay checkpoint FILE RECOVERY_DIRECTORY
  editbay recoveries RECOVERY_DIRECTORY
  editbay recover CHECKPOINT NEW_FILE
  editbay --version

The desktop editor/media engine are tracked in docs/ROADMAP.md.
New/recovered files never overwrite an existing file. Recovery keeps the original.
";

fn print(value: impl serde::Serialize) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn name(value: &OsString) -> Result<&str, Box<dyn std::error::Error>> {
    value
        .to_str()
        .ok_or_else(|| "project name must be valid UTF-8".into())
}

fn run(args: &[OsString]) -> Result<(), Box<dyn std::error::Error>> {
    let command = args
        .first()
        .and_then(|arg| arg.to_str())
        .unwrap_or("--help");
    match (command, args.len()) {
        ("--help" | "-h", 0 | 1) => print!("{HELP}"),
        ("--version", 1) => println!(
            "editbay {} (Rust project foundation)",
            env!("CARGO_PKG_VERSION")
        ),
        ("new", 3) => {
            let project = Project::new(name(&args[2])?)?;
            save_new(&project, Path::new(&args[1]))?;
            print(project)?;
        }
        ("info", 2) => print(load(Path::new(&args[1]))?)?,
        ("rename", 3) => {
            let path = Path::new(&args[1]);
            let expected = load(path)?;
            let mut editor = DocumentEditor::new(expected.clone())?;
            editor.apply(
                DocumentVersion::of(&expected),
                "Rename project".into(),
                &[DocumentCommand::RenameProject {
                    name: name(&args[2])?.into(),
                }],
            )?;
            save_if_unchanged(editor.project(), path, &expected)?;
            print(editor.project())?;
        }
        ("checkpoint", 3) => {
            let source = Path::new(&args[1]);
            let project = load(source)?;
            let path = checkpoint(&project, Some(source), Path::new(&args[2]))?;
            print(
                serde_json::json!({"checkpoint":path,"project_id":project.id,"revision":project.revision}),
            )?;
        }
        ("recoveries", 2) => print(recovery_catalog(Path::new(&args[1]))?)?,
        ("recover", 3) => print(recover_copy(Path::new(&args[1]), Path::new(&args[2]))?)?,
        _ => return Err(format!("invalid command or arguments\n\n{HELP}").into()),
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(&std::env::args_os().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("editbay: {error}");
            ExitCode::FAILURE
        }
    }
}
