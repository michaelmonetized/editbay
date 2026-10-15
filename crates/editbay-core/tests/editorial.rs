use editbay_core::*;
use uuid::Uuid;
fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn solid(n: u128, color: [f64; 4]) -> Composition {
    Composition {
        id: id(n),
        name: format!("Source {n}"),
        width: 16,
        height: 16,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 20,
        tracks: vec![],
        nodes: vec![TimedNode {
            id: id(n + 1),
            range: FrameRange { start: 0, end: 20 },
            operation: NodeOperation::Solid { rgba: color },
            animation: vec![],
        }],
        picture: Some(id(n + 1)),
        audio: None,
    }
}
#[test]
fn dissolve_with_equal_handle_starts_keeps_semantic_layer_order_and_halfway_weight() {
    let mut p = Project::new("Dissolve handles").unwrap();
    p.compositions = vec![solid(10, [1., 0., 0., 1.]), solid(20, [0., 0., 1., 1.])];
    let mut scene = Composition {
        id: id(100),
        name: "Dissolve".into(),
        width: 16,
        height: 16,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 6,
        tracks: vec![Track {
            id: id(101),
            name: "Picture".into(),
            kind: TrackKind::Video,
            enabled: true,
            clips: vec![
                Clip {
                    id: id(999),
                    name: "Outgoing".into(),
                    range: FrameRange { start: 0, end: 4 },
                    source: ClipSource::Composition {
                        composition: id(10),
                    },
                    time_map: TimeMap {
                        source_denominator: 1,
                        points: vec![
                            TimePoint {
                                frame: 0,
                                source_tick: 0,
                            },
                            TimePoint {
                                frame: 4,
                                source_tick: 4,
                            },
                        ],
                    },
                    linked: None,
                },
                Clip {
                    id: id(998),
                    name: "Incoming".into(),
                    range: FrameRange { start: 0, end: 6 },
                    source: ClipSource::Composition {
                        composition: id(20),
                    },
                    time_map: TimeMap {
                        source_denominator: 1,
                        points: vec![
                            TimePoint {
                                frame: 0,
                                source_tick: 0,
                            },
                            TimePoint {
                                frame: 6,
                                source_tick: 6,
                            },
                        ],
                    },
                    linked: None,
                },
            ],
        }],
        nodes: vec![],
        picture: None,
        audio: None,
    };
    let transition = EditorialTransition {
        id: id(200),
        composition: id(100),
        range: FrameRange { start: 0, end: 4 },
        cut: 2,
        from: Some(id(999)),
        to: Some(id(998)),
    };
    rebuild_editorial(&mut scene, std::slice::from_ref(&transition)).unwrap();
    p.compositions.push(scene);
    p.transitions.push(transition);
    p.validate().unwrap();
    let plan = p.frame_plan(id(100), 2).unwrap();
    let root = plan
        .nodes
        .iter()
        .find(|n| Some(n.id) == plan.picture)
        .unwrap();
    let NodeOperation::Over { foreground, .. } = root.operation else {
        panic!("Expected native over")
    };
    let incoming = plan.nodes.iter().find(|n| n.id == foreground).unwrap();
    assert!(
        matches!(incoming.operation,NodeOperation::Transform{opacity,..}if (opacity-0.5).abs()<1e-12)
    );
    let NodeOperation::Transform { image, .. } = incoming.operation else {
        unreachable!()
    };
    assert!(
        plan.nodes
            .iter()
            .any(|n| n.id == image && n.operation == NodeOperation::Source { clip: id(998) })
    );
    let mut editor = DocumentEditor::new(p.clone()).unwrap();
    let mut changed = p.transitions[0].clone();
    changed.range.start = 2;
    changed.cut = 3;
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Retime dissolve".into(),
            &[DocumentCommand::SetTransition {
                transition: changed,
            }],
        )
        .unwrap();
    let changed_plan = editor.project().frame_plan(id(100), 2).unwrap();
    assert!(
        changed_plan.nodes.iter().any(
            |n| matches!(n.operation, NodeOperation::Transform { opacity, .. } if opacity == 0.)
        )
    );
    editor.undo(DocumentVersion::of(editor.project())).unwrap();
    assert_eq!(editor.project().transitions, p.transitions);
    assert_eq!(editor.project().compositions, p.compositions);
    editor
        .apply(
            DocumentVersion::of(editor.project()),
            "Remove dissolve".into(),
            &[DocumentCommand::RemoveTransition { id: id(200) }],
        )
        .unwrap();
    assert!(editor.project().transitions.is_empty());
    assert!(
        !editor
            .project()
            .frame_plan(id(100), 2)
            .unwrap()
            .nodes
            .iter()
            .any(|n| matches!(n.operation, NodeOperation::Transform { .. }))
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dissolve.editbay");
    save_new(&p, &path).unwrap();
    assert_eq!(load(path).unwrap(), p);
}
