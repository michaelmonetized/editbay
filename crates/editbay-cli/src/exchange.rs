use editbay_interchange::{ExportOptions, Format, ImportOptions};
use std::{ffi::OsString, io::Write, path::Path};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn format(value: &OsString) -> Result<Format> {
    Format::from_extension(super::name(value)?)
        .ok_or_else(|| "Choose edl, xml, fcpxml, otio, aaf or omf".into())
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Execute editorial interchange through the native shared document.
/// `args` names import/export/relink with explicit files; returns JSON receipts.
/// Every export retains a new loss report and never overwrites user files.
pub(super) fn run(args: &[OsString]) -> Result<()> {
    let convert = |e: Box<dyn std::error::Error + Send + Sync>| -> Box<dyn std::error::Error> { e };
    match super::name(&args[0])? {
        "exchange-import" => {
            let source = Path::new(&args[1]);
            let destination = Path::new(&args[2]);
            let bytes = super::read_bounded(source, 32 * 1024 * 1024)?;
            let fmt = format(&args[3])?;
            let options = if let Some(path) = args.get(4) {
                serde_json::from_slice::<ImportOptions>(&super::read_bounded(
                    Path::new(path),
                    1024 * 1024,
                )?)?
            } else {
                ImportOptions {
                    base_dir: source.parent().map(|p| p.to_string_lossy().into()),
                    ..Default::default()
                }
            };
            let (project, report) = editbay_interchange::import_document(
                &bytes,
                fmt,
                &options,
                &editbay_media::Cancellation::new()?,
            )
            .map_err(convert)?;
            let report_path = destination.with_extension("import-loss.json");
            if destination.exists() || report_path.exists() {
                return Err("Import project or loss report destination exists".into());
            }
            write_new(&report_path, &serde_json::to_vec_pretty(&report)?)?;
            editbay_core::save_new(destination, &project)?;
            super::print(
                serde_json::json!({"project":destination,"loss_report":report_path,"sequences":project.sequences,"report":report}),
            )
        }
        "exchange-export" => {
            let project = editbay_core::load(Path::new(&args[1]))?;
            let composition = super::name(&args[2])?.parse()?;
            let destination = Path::new(&args[3]);
            let fmt = format(&args[4])?;
            let (bytes, report) = editbay_interchange::export_document(
                &project,
                composition,
                fmt,
                &ExportOptions::default(),
            )
            .map_err(convert)?;
            let report_path = destination.with_extension(format!("{}.loss.json", fmt.extension()));
            if destination.exists() || report_path.exists() {
                return Err("Exchange or loss report destination exists".into());
            }
            write_new(&report_path, &serde_json::to_vec_pretty(&report)?)?;
            write_new(destination, &bytes)?;
            super::print(
                serde_json::json!({"document":destination,"loss_report":report_path,"bytes":bytes.len(),"report":report}),
            )
        }
        "relink" => {
            let path = Path::new(&args[1]);
            let project = editbay_core::load(path)?;
            let command = editbay_interchange::relink(
                &project,
                super::name(&args[2])?.parse()?,
                Path::new(&args[3]),
                &editbay_media::Cancellation::new()?,
            )
            .map_err(convert)?;
            let mut editor = editbay_core::DocumentEditor::new(project.clone())?;
            let receipt = editor.apply(
                editbay_core::DocumentVersion::of(&project),
                "Relink original".into(),
                &[command],
            )?;
            editbay_core::save_if_unchanged(path, editor.project(), Some(&project))?;
            super::print(receipt)
        }
        _ => Err("Unknown interchange command".into()),
    }
}
