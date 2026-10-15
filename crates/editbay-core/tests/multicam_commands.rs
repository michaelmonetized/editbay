use editbay_core::{multicam::*, *};
use uuid::Uuid;
fn scene(name: &str) -> Composition {
    let root = Uuid::new_v4();
    Composition {
        id: Uuid::new_v4(),
        name: name.into(),
        width: 640,
        height: 360,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 96,
        tracks: vec![],
        nodes: vec![TimedNode {
            id: root,
            range: FrameRange { start: 0, end: 96 },
            operation: NodeOperation::Solid {
                rgba: [0.2, 0.4, 0.6, 1.],
            },
            animation: vec![],
        }],
        picture: Some(root),
        audio: None,
    }
}
#[test]
fn retained_multicam_command_is_atomic_reversible_and_reopenable() {
    let mut p = Project::new("Multicam command").unwrap();
    p.compositions = vec![scene("A"), scene("B")];
    let originals = p.compositions.clone();
    let session = MulticamSession::new(
        &p,
        "Cut",
        &p.compositions.iter().map(|c| c.id).collect::<Vec<_>>(),
    )
    .unwrap();
    let output = session.composition;
    let mut editor = DocumentEditor::new(p).unwrap();
    let initial = DocumentVersion::of(editor.project());
    editor
        .apply(
            initial,
            "Create multicam".into(),
            &[DocumentCommand::SetMulticamSession {
                session: session.clone(),
            }],
        )
        .unwrap();
    let stable_version = DocumentVersion::of(editor.project());
    let stable_history = editor.history_usage();
    let repeated = editor
        .apply(
            stable_version,
            "Repeat unchanged session".into(),
            &[DocumentCommand::SetMulticamSession {
                session: session.clone(),
            }],
        )
        .unwrap();
    assert!(!repeated.changed);
    assert_eq!(DocumentVersion::of(editor.project()), stable_version);
    assert_eq!(editor.history_usage(), stable_history);
    let mut changed = session.clone();
    changed
        .cut_at(editor.project(), 24, changed.angles[1].id)
        .unwrap();
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Cut camera".into(),
            &[DocumentCommand::SetMulticamSession {
                session: changed.clone(),
            }],
        )
        .unwrap();
    assert_eq!(editor.project().multicam[0].cuts, changed.cuts);
    assert_eq!(editor.project().compositions[..2], originals);
    assert!(
        editor
            .apply(
                initial,
                "Stale".into(),
                &[DocumentCommand::RemoveMulticamSession { id: session.id }]
            )
            .is_err()
    );
    let saved = editor.project().clone();
    let mut tampered = saved.clone();
    let scene = tampered
        .compositions
        .iter_mut()
        .find(|c| c.id == output)
        .unwrap();
    let node = scene
        .nodes
        .iter_mut()
        .find(|n| Some(n.id) == scene.picture)
        .unwrap();
    node.operation = NodeOperation::Solid {
        rgba: [1., 0., 0., 1.],
    };
    assert!(
        tampered.validate().is_err(),
        "retained cuts must determine the actual delivered graph"
    );
    let first = compile(&saved, &saved.multicam[0]).unwrap();
    let second = compile(&saved, &saved.multicam[0]).unwrap();
    assert_eq!(first, second);
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().multicam[0].cuts.len(), 1);
    editor.redo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(
        editor
            .project()
            .compositions
            .iter()
            .find(|c| c.id == output)
            .unwrap()
            .tracks,
        saved
            .compositions
            .iter()
            .find(|c| c.id == output)
            .unwrap()
            .tracks
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multicam.editbay");
    save_new(editor.project(), &path).unwrap();
    assert_eq!(load(path).unwrap(), *editor.project());
}
