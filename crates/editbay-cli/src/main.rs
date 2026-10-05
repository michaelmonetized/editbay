use editbay_core::{
    CommandGroup, DocumentCommand, DocumentEditor, DocumentVersion, Project, checkpoint, load,
    recover_copy, recovery_catalog, save_if_unchanged, save_new,
};
use editbay_media::{Cancellation, SourceFile, VideoReader};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, io::Read, path::Path, process::ExitCode};

const HELP: &str = "EditBay Rust project foundation

Usage:
  editbay new FILE NAME
  editbay info FILE
  editbay rename FILE NAME
  editbay apply FILE COMMAND_GROUP_JSON
  editbay migrate SOURCE NEW_FILE
  editbay frame-plan FILE COMPOSITION_ID FRAME
  editbay probe-media SOURCE
  editbay ingest FILE SOURCE STREAM_INDICES
  editbay decode-frame SOURCE STREAM_INDEX SOURCE_TICK
  editbay checkpoint FILE RECOVERY_DIRECTORY
  editbay recoveries RECOVERY_DIRECTORY
  editbay recover CHECKPOINT NEW_FILE
  editbay --version

The native workspace and media engine are tracked in docs/ROADMAP.md.
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

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(format!("request exceeds its {limit}-byte read budget").into());
    }
    Ok(bytes)
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
        ("apply", 3) => {
            let path = Path::new(&args[1]);
            let expected = load(path)?;
            let bytes = read_bounded(Path::new(&args[2]), 8 * 1024 * 1024)?;
            let group: CommandGroup = serde_json::from_slice(&bytes)?;
            let mut editor = DocumentEditor::new(expected.clone())?;
            let receipt = editor.apply(group.expected, group.label, &group.commands)?;
            save_if_unchanged(editor.project(), path, &expected)?;
            print(serde_json::json!({"receipt":receipt,"saved":true}))?;
        }
        ("migrate", 3) => {
            let source = Path::new(&args[1]);
            let source_bytes = read_bounded(source, editbay_core::MAX_DOCUMENT_BYTES)?;
            let old: Project = serde_json::from_slice(&source_bytes)?;
            let from_schema = old.schema;
            let project = old.migrate()?;
            save_new(&project, Path::new(&args[2]))?;
            print(
                serde_json::json!({"from_schema":from_schema,"to_schema":project.schema,
                "project_id":project.id,"revision":project.revision,"source_preserved":true,
                "conversion_losses":[],"destination":Path::new(&args[2])}),
            )?;
        }
        ("frame-plan", 4) => {
            let project = load(Path::new(&args[1]))?;
            let composition = name(&args[2])?.parse()?;
            let frame = name(&args[3])?.parse()?;
            print(project.frame_plan(composition, frame)?)?;
        }
        ("probe-media", 2) => {
            let cancel = Cancellation::new()?;
            let source = SourceFile::open(Path::new(&args[1]), &cancel)?;
            print(source.probe(cancel)?)?;
        }
        ("ingest", 4) => {
            let path = Path::new(&args[1]);
            let expected = load(path)?;
            let mut editor = DocumentEditor::new(expected.clone())?;
            let indices: Vec<u32> = name(&args[3])?
                .split(',')
                .map(str::parse)
                .collect::<Result<_, _>>()?;
            let cancel = Cancellation::new()?;
            let source = SourceFile::open(Path::new(&args[2]), &cancel)?;
            let source_name = source
                .path()
                .file_name()
                .ok_or("source has no filename")?
                .to_string_lossy()
                .into_owned();
            let imported = source.ingest(source_name, &indices, cancel.clone(), |_, _| {})?;
            let receipt = editor.apply(
                DocumentVersion::of(&expected),
                "Import media".into(),
                &imported.commands(),
            )?;
            source.verify(&cancel)?;
            save_if_unchanged(editor.project(), path, &expected)?;
            print(
                serde_json::json!({"receipt":receipt,"source":imported.source.id,
                "asset":imported.asset.id,"fingerprint":source.fingerprint(),"saved":true}),
            )?;
        }
        ("decode-frame", 4) => {
            let cancel = Cancellation::new()?;
            let source = SourceFile::open(Path::new(&args[1]), &cancel)?;
            let stream = name(&args[2])?.parse()?;
            let tick = name(&args[3])?.parse()?;
            let mut reader = VideoReader::open_stream(&source, stream, cancel.clone())?;
            let frame = reader.frame_at(tick)?;
            source.verify(&cancel)?;
            print(
                serde_json::json!({"fingerprint":source.fingerprint(),"stream":stream,
                "source_tick":frame.source_tick,"timestamp_ns":frame.timestamp_ns,
                "width":reader.info.width,"height":reader.info.height,"format":"rgba8",
                "rgba_bytes":frame.rgba.len(),"rgba_sha256":format!("{:x}",Sha256::digest(&frame.rgba))}),
            )?;
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
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--picture-worker")) {
        return match editbay_media::picture_worker::serve() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("editbay picture worker: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--media-worker")) {
        return match editbay_media::worker::serve() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("editbay media worker: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match run(&std::env::args_os().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("editbay: {error}");
            ExitCode::FAILURE
        }
    }
}
