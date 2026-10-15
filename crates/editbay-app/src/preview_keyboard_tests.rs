use super::*;
use editbay_core::{MediaSource, SourceStream, StreamFormat, TimeBase};

#[test]
fn craft_keyboard_source_sound_native_choice_keeps_owner_focus_across_conditional_rows() {
    let ctx = egui::Context::default();
    let tab = Uuid::new_v4();
    let source = MediaSource {
        id: Uuid::new_v4(),
        asset: Uuid::new_v4(),
        name: "Imported sound choices".into(),
        metadata: Default::default(),
        streams: [3, 9]
            .into_iter()
            .map(|index| SourceStream {
                index,
                codec: "pcm_s16le".into(),
                time_base: TimeBase {
                    numerator: 1,
                    denominator: 48_000,
                },
                start_tick: 0,
                duration_ticks: Some(48_000),
                metadata: Default::default(),
                format: StreamFormat::Audio {
                    sample_rate: 48_000,
                    channels: vec!["L".into(), "R".into()],
                },
            })
            .collect(),
    };
    let mut pane = PreviewPane::default();
    let mut frame = |key: Option<egui::Key>, prefix: bool| {
        let mut focus = None;
        let output = ctx.run_ui(
            egui::RawInput {
                events: key
                    .into_iter()
                    .map(|key| egui::Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    })
                    .collect(),
                ..Default::default()
            },
            |ui| {
                if prefix {
                    ui.text_edit_singleline(&mut String::new());
                }
                pane.source_sound(ui, tab, &source);
            },
        );
        for event in &output.platform_output.events {
            if let egui::output::OutputEvent::FocusGained(info) = event {
                focus = info.label.clone();
            }
        }
        output.drop_without_applying_deltas();
        focus
    };
    frame(None, false);
    let mut reached = false;
    for _ in 0..8 {
        if frame(Some(egui::Key::Tab), false).as_deref() == Some("Sequence sound: Picture only") {
            reached = true;
            break;
        }
    }
    assert!(
        reached,
        "source sound selector is not reachable by native Tab"
    );
    let focused = ctx.memory(|memory| memory.focused()).unwrap();
    frame(None, true);
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(focused));
    frame(Some(egui::Key::End), true);
    drop(frame);
    assert_eq!(pane.source_audio[&(tab, source.id)], Some(9));
    assert!(pane.creation.is_none());
    assert!(pane.task.is_none());
}
