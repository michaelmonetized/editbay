use editbay_core::{
    DocumentCommand, DocumentEditor, DocumentVersion, captions::CaptionTrack, load,
    save_if_unchanged,
};
use std::{ffi::OsString, path::Path};

/// Import or export a retained caption track through shared commands.
/// `args` supplies project, composition/track and source/destination paths.
/// Returns a JSON receipt; output paths must be new and cue timing is never rounded.
pub(super) fn run(args: &[OsString]) -> Result<(), Box<dyn std::error::Error>> {
    let sidecar = Path::new(&args[3]);
    let vtt = caption_vtt(sidecar)?;
    let path = Path::new(&args[1]);
    let project = load(path)?;
    let identity = super::name(&args[2])?.parse()?;
    if super::name(&args[0])? == "captions-import" {
        let bytes = super::read_bounded(sidecar, 8 * 1024 * 1024)?;
        let input = std::str::from_utf8(&bytes)?;
        let track = if vtt {
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
        let text = if vtt {
            track.export_vtt()?
        } else {
            track.export_srt()?
        };
        editbay_core::write_new_bytes(sidecar, text.as_bytes())?;
        super::print(
            serde_json::json!({"sidecar":sidecar,"cues":track.cues.len(),"version":DocumentVersion::of(&project)}),
        )
    }
}

/// Require the requested caption format before any project or sidecar IO.
/// `path` supplies its extension; returns true for VTT, false for SRT or an explicit error.
fn caption_vtt(path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    match path.extension() {
        Some(extension) if extension.eq_ignore_ascii_case("srt") => Ok(false),
        Some(extension) if extension.eq_ignore_ascii_case("vtt") => Ok(true),
        _ => Err("Caption path must end in .srt or .vtt".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn craft_video_caption_cli_accepts_only_explicit_srt_and_vtt_paths() {
        for (path, vtt) in [
            ("caption.srt", false),
            ("caption.SRT", false),
            ("caption.VtT", true),
        ] {
            assert_eq!(caption_vtt(Path::new(path)).unwrap(), vtt);
        }
        for path in ["caption", "caption.txt", "caption.vtt.bak", "caption."] {
            assert!(caption_vtt(Path::new(path)).is_err());
        }
    }

    #[test]
    fn craft_video_caption_cli_rejects_unknown_formats_before_any_io() {
        for command in ["captions-import", "captions-export"] {
            let args = [
                command,
                "missing-project.editbay",
                "not-a-uuid",
                "caption.txt",
            ]
            .map(OsString::from);
            let error = run(&args).unwrap_err().to_string();
            assert!(
                error.contains(".srt or .vtt"),
                "Expected format validation before project IO or identity parsing: {error}"
            );
        }
    }
}
