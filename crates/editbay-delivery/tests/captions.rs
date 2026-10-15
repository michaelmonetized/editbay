use editbay_core::{captions::*, *};
use editbay_delivery::{DeliveryFormat, DeliveryRequest, caption_sidecars};
use uuid::Uuid;
#[test]
fn range_sidecar_exact_clipping_and_no_overwrite() {
    let mut p = Project::new("Delivery captions").unwrap();
    let composition = Uuid::new_v4();
    p.compositions.push(Composition {
        id: composition,
        name: "Program".into(),
        width: 640,
        height: 360,
        frame_rate: FrameRate::new(25, 1).unwrap(),
        duration: 100,
        tracks: vec![],
        nodes: vec![],
        picture: None,
        audio: None,
    });
    let mut track=CaptionTrack::import_srt(composition,"Captions","1\n00:00:00,001 --> 00:00:01,001\nBefore\n\n2\n00:00:01,003 --> 00:00:03,005\nCafé 世界\n\n3\n00:00:03,200 --> 00:00:03,500\nAfter\n").unwrap();
    track.delivery = CaptionDelivery::SidecarVtt;
    p.captions.push(track);
    let req = DeliveryRequest {
        format: DeliveryFormat::LosslessMov,
        composition,
        sample_rate: 48000,
        range: Some(FrameRange { start: 25, end: 75 }),
    };
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("master.mov");
    let outputs = caption_sidecars(&p, &req, &destination).unwrap();
    assert_eq!(outputs.len(), 1);
    let text = std::str::from_utf8(&outputs[0].1).unwrap();
    assert!(text.contains("00:00:00.000 --> 00:00:00.001"));
    assert!(text.contains("00:00:00.003 --> 00:00:02.000"));
    assert!(!text.contains("After"));
    let parsed = CaptionTrack::import_vtt(composition, "Independent reopen", text).unwrap();
    assert_eq!(parsed.cues.len(), 2);
    if let Some(root) = std::env::var_os("EDITBAY_WORKFLOW_RECEIPTS_DIR") {
        let root = std::path::PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("range.vtt"), &outputs[0].1).unwrap();
        std::fs::write(root.join("range.srt"), parsed.export_srt().unwrap()).unwrap();
    }
    std::fs::write(&outputs[0].0, b"owner output").unwrap();
    assert!(caption_sidecars(&p, &req, &destination).is_err());
    assert_eq!(std::fs::read(&outputs[0].0).unwrap(), b"owner output");
}
