use editbay_core::{
    DocumentCommand, DocumentEditor, DocumentVersion, captions::CaptionTrack, load,
    save_if_unchanged,
};
use std::{ffi::OsString, io::Write, path::Path};

/// Import or export a retained caption track through shared commands.
/// `args` supplies project, composition/track and source/destination paths.
/// Returns a JSON receipt; output paths must be new and cue timing is never rounded.
pub(super) fn run(args: &[OsString]) -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(&args[1]);
    let project = load(path)?;
    let identity = super::name(&args[2])?.parse()?;
    let sidecar = Path::new(&args[3]);
    if super::name(&args[0])? == "captions-import" {
        let bytes = super::read_bounded(sidecar, 8 * 1024 * 1024)?;
        let input = std::str::from_utf8(&bytes)?;
        let track = if sidecar
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("vtt"))
        {
            CaptionTrack::import_vtt(identity, "Captions", input)?
        } else {
            CaptionTrack::import_srt(identity, "Captions", input)?
        };
        let track_id = track.id;
        let mut editor = DocumentEditor::new(project.clone())?;
        let receipt = editor.apply(
            DocumentVersion::of(&project),
            "Import captions".into(),
            &[DocumentCommand::SetCaptionTrack { track }],
        )?;
        save_if_unchanged(editor.project(), path, &project)?;
        super::print(serde_json::json!({"receipt":receipt,"track":track_id}))
    } else {
        let track = project
            .captions
            .iter()
            .find(|t| t.id == identity)
            .ok_or("Caption track absent")?;
        let text = if sidecar
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("vtt"))
        {
            track.export_vtt()?
        } else {
            track.export_srt()?
        };
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(sidecar)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        super::print(
            serde_json::json!({"sidecar":sidecar,"cues":track.cues.len(),"version":DocumentVersion::of(&project)}),
        )
    }
}
