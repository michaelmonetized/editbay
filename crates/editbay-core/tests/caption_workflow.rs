use editbay_core::{captions::*, *};
use uuid::Uuid;
fn project() -> Project {
    let mut p = Project::new("Caption workflow").unwrap();
    let scene = Composition {
        id: Uuid::new_v4(),
        name: "Program".into(),
        width: 640,
        height: 360,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 240,
        tracks: vec![],
        nodes: vec![TimedNode {
            id: Uuid::new_v4(),
            range: FrameRange { start: 0, end: 240 },
            operation: NodeOperation::Solid {
                rgba: [0., 0., 0., 1.],
            },
            animation: vec![],
        }],
        picture: None,
        audio: None,
    };
    let mut scene = scene;
    scene.picture = Some(scene.nodes[0].id);
    p.sequences[0].composition = Some(scene.id);
    p.sequences[0].width = scene.width;
    p.sequences[0].height = scene.height;
    p.sequences[0].frame_rate = scene.frame_rate;
    p.compositions.push(scene);
    p
}
#[test]
fn captions_share_undo_stale_rejection_save_reopen_and_recovery() {
    let original = project();
    let mut track = CaptionTrack::import_srt(
        original.compositions[0].id,
        "Subtitles",
        "1\n00:00:00,001 --> 00:00:02,003\nCafé 世界\n",
    )
    .unwrap();
    let mut editor = DocumentEditor::new(original.clone()).unwrap();
    let initial = DocumentVersion::of(editor.project());
    let receipt = editor
        .apply(
            initial,
            "Import captions".into(),
            &[DocumentCommand::SetCaptionTrack {
                track: track.clone(),
            }],
        )
        .unwrap();
    assert_eq!(receipt.impact.compositions, vec![track.composition]);
    assert!(
        editor
            .apply(
                initial,
                "Stale".into(),
                &[DocumentCommand::RemoveCaptionTrack { id: track.id }]
            )
            .is_err()
    );
    let cue = track.cues[0].id;
    track
        .split_cue(cue, CaptionTime::from_milliseconds(1001), 4)
        .unwrap();
    track
        .retime_cue(
            track.cues[1].id,
            CaptionTime::from_milliseconds(1003),
            CaptionTime::from_milliseconds(2005),
        )
        .unwrap();
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Split and retime".into(),
            &[DocumentCommand::SetCaptionTrack {
                track: track.clone(),
            }],
        )
        .unwrap();
    let edited = editor.project().clone();
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().captions[0].cues.len(), 1);
    editor.redo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().captions, edited.captions);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("captions.editbay");
    save_new(editor.project(), &path).unwrap();
    assert_eq!(load(&path).unwrap(), *editor.project());
    let saved = std::fs::read(&path).unwrap();
    let snapshot = checkpoint(editor.project(), Some(&path), dir.path().join("recovery")).unwrap();
    let recovered_path = dir.path().join("recovered.editbay");
    let recovered = recover_copy(&snapshot, &recovered_path).unwrap();
    assert_eq!(recovered.captions, edited.captions);
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    let mut legacy = serde_json::to_value(original).unwrap();
    legacy.as_object_mut().unwrap().remove("captions");
    legacy.as_object_mut().unwrap().remove("transitions");
    assert!(
        serde_json::from_value::<Project>(legacy)
            .unwrap()
            .captions
            .is_empty()
    );
}
#[test]
fn caption_cache_identity_changes_for_rendered_text_and_font() {
    let mut p = project();
    let id = p.compositions[0].id;
    let baseline = p.composition_fingerprint(id).unwrap();
    let mut track =
        CaptionTrack::import_srt(id, "Captions", "1\n00:00:00,000 --> 00:00:01,000\nHello\n")
            .unwrap();
    track.delivery = CaptionDelivery::BurnIn;
    p.captions.push(track);
    assert_ne!(p.composition_fingerprint(id).unwrap(), baseline);
    let before = p.composition_fingerprint(id).unwrap();
    p.captions[0].cues[0].text = "Changed".into();
    assert_ne!(p.composition_fingerprint(id).unwrap(), before);
}
