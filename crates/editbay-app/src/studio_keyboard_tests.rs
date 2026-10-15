use super::*;

fn input(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn frame(ctx: &egui::Context, studio: &mut Studio, events: Vec<egui::Event>) -> Vec<String> {
    let modifiers = events
        .iter()
        .rev()
        .find_map(|event| {
            if let egui::Event::Key { modifiers, .. } = event {
                Some(*modifiers)
            } else {
                None
            }
        })
        .unwrap_or_default();
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(1440., 1000.),
            )),
            events,
            modifiers,
            ..Default::default()
        },
        |ui| {
            studio.shortcuts(ui.ctx());
            studio.modals(ui.ctx());
        },
    );
    let labels = output
        .platform_output
        .events
        .iter()
        .filter_map(|event| {
            if let egui::output::OutputEvent::FocusGained(info) = event {
                info.label.clone()
            } else {
                None
            }
        })
        .collect();
    output.drop_without_applying_deltas();
    labels
}

fn tab_to(ctx: &egui::Context, studio: &mut Studio, label: &str) {
    for _ in 0..32 {
        let labels = frame(
            ctx,
            studio,
            vec![input(egui::Key::Tab, egui::Modifiers::NONE)],
        );
        let idle = frame(ctx, studio, vec![]);
        if labels.iter().chain(&idle).any(|value| value == label) {
            return;
        }
    }
    panic!("Cannot reach native {label} by Tab");
}

#[test]
fn craft_keyboard_new_project_numeric_profile_and_rate_use_actual_focused_controls() {
    let directory = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut studio = Studio::new(
        ctx.clone(),
        directory.path().into(),
        directory.path().into(),
        directory.path().join("state"),
        Vec::new(),
        rfd::AsyncFileDialog::new(),
    );
    frame(
        &ctx,
        &mut studio,
        vec![input(
            egui::Key::N,
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        )],
    );
    frame(&ctx, &mut studio, vec![]);
    frame(
        &ctx,
        &mut studio,
        vec![
            input(
                egui::Key::A,
                egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            ),
            egui::Event::Text("Keyboard profile".into()),
        ],
    );
    tab_to(&ctx, &mut studio, "Picture width");
    frame(
        &ctx,
        &mut studio,
        vec![
            input(
                egui::Key::A,
                egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            ),
            egui::Event::Text("1080".into()),
            input(egui::Key::Enter, egui::Modifiers::NONE),
        ],
    );
    assert!(studio.create_name.is_some());
    tab_to(&ctx, &mut studio, "Picture height");
    frame(
        &ctx,
        &mut studio,
        vec![
            input(
                egui::Key::A,
                egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            ),
            egui::Event::Text("1920".into()),
            input(egui::Key::Enter, egui::Modifiers::NONE),
        ],
    );
    tab_to(&ctx, &mut studio, "Frame rate: 24");
    frame(
        &ctx,
        &mut studio,
        vec![input(egui::Key::End, egui::Modifiers::NONE)],
    );
    assert_eq!(studio.new_profile, [1080, 1920, 60, 1]);
    tab_to(&ctx, &mut studio, "Create");
    frame(
        &ctx,
        &mut studio,
        vec![input(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    let project = studio.workspace.tabs[0].editor.project();
    assert_eq!(project.name, "Keyboard profile");
    assert_eq!(
        (project.sequences[0].width, project.sequences[0].height),
        (1080, 1920)
    );
    assert_eq!(
        (
            project.sequences[0].frame_rate.numerator,
            project.sequences[0].frame_rate.denominator
        ),
        (60, 1)
    );
    assert!(project.compositions.is_empty());
    assert!(!ctx.text_edit_focused());
}
