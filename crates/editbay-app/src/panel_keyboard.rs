//! Panel operations use the shared workspace's native focus and sizing requests.
use crate::{
    fields, keyboard_ui,
    panels::{Place, Workspace},
};
use eframe::egui::{self, Ui};

#[derive(Clone, Copy, Default, PartialEq)]
enum Operation {
    #[default]
    Focus,
    Left,
    Right,
    Top,
    Bottom,
    Float,
    Tab,
    Close,
    Resize,
}

#[derive(Default)]
pub struct PanelKeyboard {
    draft: Option<Draft>,
}

struct Draft {
    workspace: String,
    panel: String,
    target: String,
    operation: Operation,
    size: [f32; 2],
    focus: bool,
    error: Option<String>,
}

impl PanelKeyboard {
    /// Operate the active workspace without a context menu.
    /// Args: UI, stable workspace ID, normalized layout, registry and interaction state.
    /// Returns no value; native focus and floating size requests are transient.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        workspace_id: &str,
        layout: &mut Workspace,
        registry: &[(&str, &str)],
        interactive: bool,
    ) {
        let open = interactive
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::F6));
        let button = ui
            .add_enabled_ui(interactive, |ui| {
                keyboard_ui::button(ui, "keyboard-panels", "Arrange panels… · Ctrl+F6")
            })
            .inner;
        if open || button.clicked() {
            let panel = layout
                .focus
                .as_deref()
                .filter(|id| registry.iter().any(|entry| entry.0 == *id))
                .or_else(|| registry.first().map(|entry| entry.0));
            if let Some(panel) = panel {
                self.draft = Some(Draft {
                    workspace: workspace_id.into(),
                    panel: panel.into(),
                    target: registry
                        .iter()
                        .find(|entry| entry.0 != panel)
                        .map_or(panel, |entry| entry.0)
                        .into(),
                    operation: Operation::Focus,
                    size: [320., 420.],
                    focus: true,
                    error: None,
                });
            }
        }
        if self
            .draft
            .as_ref()
            .is_some_and(|draft| draft.workspace != workspace_id)
            || !interactive
        {
            self.draft = None;
        }
        let Some(draft) = &mut self.draft else {
            return;
        };
        let mut close = false;
        let mut apply = false;
        egui::Modal::new(egui::Id::new("editbay-panel-arrangement")).show(ui.ctx(), |ui| {
            ui.set_max_width(360.);
            ui.heading("Arrange panels");
            let choices: Vec<_> = registry
                .iter()
                .enumerate()
                .map(|(index, entry)| (index, entry.1.to_string()))
                .collect();
            let mut panel = registry
                .iter()
                .position(|entry| entry.0 == draft.panel)
                .unwrap_or(0);
            let response = keyboard_ui::choice(ui, "panel", "Panel", &mut panel, &choices);
            if draft.focus && !ui.is_sizing_pass() {
                response.request_focus();
                draft.focus = false;
            }
            if response.changed() {
                draft.panel = registry[panel].0.into();
            }
            keyboard_ui::choice(
                ui,
                "operation",
                "Action",
                &mut draft.operation,
                &[
                    (Operation::Focus, "Focus or restore".into()),
                    (Operation::Left, "Move left".into()),
                    (Operation::Right, "Move right".into()),
                    (Operation::Top, "Move top".into()),
                    (Operation::Bottom, "Move bottom".into()),
                    (Operation::Float, "Float".into()),
                    (Operation::Tab, "Tab with another panel".into()),
                    (Operation::Close, "Close".into()),
                    (Operation::Resize, "Resize floating panel".into()),
                ],
            );
            if draft.operation == Operation::Tab {
                let targets: Vec<_> = registry
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| {
                        entry.0 != draft.panel
                            && layout
                                .groups
                                .iter()
                                .any(|group| group.panels.iter().any(|id| id == entry.0))
                    })
                    .map(|(index, entry)| (index, entry.1.to_string()))
                    .collect();
                let mut target = registry
                    .iter()
                    .position(|entry| entry.0 == draft.target)
                    .unwrap_or(usize::MAX);
                let response = keyboard_ui::choice(ui, "target", "Tab with", &mut target, &targets);
                if response.changed() {
                    draft.target = registry[target].0.into();
                }
            }
            if matches!(draft.operation, Operation::Float | Operation::Resize) {
                fields::number(
                    ui,
                    "width",
                    "Floating width",
                    &mut draft.size[0],
                    120. ..=4096.,
                    " px",
                    None,
                    fields::FieldState::default(),
                );
                fields::number(
                    ui,
                    "height",
                    "Floating height",
                    &mut draft.size[1],
                    80. ..=4096.,
                    " px",
                    None,
                    fields::FieldState::default(),
                );
            }
            if let Some(error) = &draft.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.horizontal(|ui| {
                apply = keyboard_ui::button(ui, "apply", "Apply").clicked();
                close = keyboard_ui::button(ui, "cancel", "Cancel").clicked();
            });
            close |=
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        });
        if apply {
            let panel = draft.panel.as_str();
            let success = match draft.operation {
                Operation::Close => {
                    layout.close(panel);
                    true
                }
                Operation::Tab => layout.tab_with(panel, &draft.target),
                Operation::Resize => layout.request_floating_size(
                    ui.ctx(),
                    workspace_id,
                    panel,
                    egui::vec2(draft.size[0], draft.size[1]),
                ),
                operation => {
                    match operation {
                        Operation::Focus => layout.restore(panel),
                        Operation::Left => layout.move_panel(panel, Place::Left),
                        Operation::Right => layout.move_panel(panel, Place::Right),
                        Operation::Top => layout.move_panel(panel, Place::Top),
                        Operation::Bottom => layout.move_panel(panel, Place::Bottom),
                        Operation::Float => layout.move_panel(panel, Place::Floating),
                        _ => unreachable!(),
                    }
                    operation != Operation::Float
                        || layout.request_floating_size(
                            ui.ctx(),
                            workspace_id,
                            panel,
                            egui::vec2(draft.size[0], draft.size[1]),
                        )
                }
            };
            if success {
                if draft.operation != Operation::Close {
                    layout.request_focus(ui.ctx(), workspace_id, panel);
                }
                close = true;
            } else {
                draft.error = Some(
                    "The panel target changed. Choose a visible target or float the panel first."
                        .into(),
                );
            }
        }
        if close {
            self.draft = None;
        }
    }

    /// Inspect the modal's ownership.
    /// Args: none. Returns true while keyboard panel operations own input.
    pub fn opened(&self) -> bool {
        self.draft.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::Book;

    const REGISTRY: &[(&str, &str)] = &[("media", "Media"), ("timeline", "Timeline")];

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn render(
        ctx: &egui::Context,
        layout: &mut Workspace,
        events: Vec<egui::Event>,
        mut keyboard: Option<&mut PanelKeyboard>,
    ) -> (egui::Rect, Option<egui::Id>, Vec<String>) {
        let mut body = egui::Rect::NOTHING;
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280., 900.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                if let Some(keyboard) = keyboard.as_deref_mut() {
                    keyboard.show(ui, "editbay-panel-test", layout, REGISTRY, true);
                }
                layout.show(ui, "editbay-panel-test", REGISTRY, |ui, panel| {
                    if panel == "media" {
                        body = ui.max_rect().intersect(ui.clip_rect());
                    }
                    ui.label(format!("{panel} content"));
                });
            },
        );
        let labels = output
            .platform_output
            .events
            .iter()
            .filter_map(|event| {
                if let egui::OutputEvent::FocusGained(info) = event {
                    info.label.clone()
                } else {
                    None
                }
            })
            .collect();
        output.drop_without_applying_deltas();
        (body, ctx.memory(|memory| memory.focused()), labels)
    }

    #[test]
    fn craft_keyboard_panel_modal_tab_arrows_restore_close_and_repeated_native_focus() {
        let ctx = egui::Context::default();
        let mut layout = Workspace::preset(&[("media", Place::Left), ("timeline", Place::Bottom)]);
        layout.normalize(&["media", "timeline"]);
        layout.close("media");
        let mut keyboard = PanelKeyboard::default();
        render(&ctx, &mut layout, vec![], Some(&mut keyboard));
        for _ in 0..2 {
            render(
                &ctx,
                &mut layout,
                vec![key(egui::Key::F6, egui::Modifiers::CTRL)],
                Some(&mut keyboard),
            );
            render(&ctx, &mut layout, vec![], Some(&mut keyboard));
            assert!(keyboard.opened());
            render(
                &ctx,
                &mut layout,
                vec![key(egui::Key::Home, egui::Modifiers::NONE)],
                Some(&mut keyboard),
            );
            render(
                &ctx,
                &mut layout,
                vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
                Some(&mut keyboard),
            );
            render(
                &ctx,
                &mut layout,
                vec![key(egui::Key::Home, egui::Modifiers::NONE)],
                Some(&mut keyboard),
            );
            render(
                &ctx,
                &mut layout,
                vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
                Some(&mut keyboard),
            );
            let applied = render(
                &ctx,
                &mut layout,
                vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
                Some(&mut keyboard),
            );
            let idle = render(&ctx, &mut layout, vec![], Some(&mut keyboard));
            assert!(!keyboard.opened());
            assert_eq!(layout.focus.as_deref(), Some("media"));
            assert!(!layout.hidden.contains_key("media"));
            let focused = ctx.memory(|memory| memory.focused());
            assert!(
                focused.is_some(),
                "explicit same-panel request must focus its actual tab"
            );
            assert!(
                applied
                    .2
                    .iter()
                    .chain(&idle.2)
                    .any(|label| label == "Media"),
                "native focus did not return to Media: {:?} {:?}",
                applied.2,
                idle.2
            );
        }
    }

    #[test]
    fn craft_keyboard_floating_size_changes_actual_body_across_frames_and_native_json_reopen() {
        for scale in [1., 1.5, 2.] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(scale);
            let mut layout =
                Workspace::preset(&[("media", Place::Floating), ("timeline", Place::Bottom)]);
            layout.normalize(&["media", "timeline"]);
            for _ in 0..3 {
                render(&ctx, &mut layout, vec![], None);
            }
            let before = render(&ctx, &mut layout, vec![], None).0;
            assert!(layout.request_floating_size(
                &ctx,
                "editbay-panel-test",
                "media",
                egui::vec2(520., 360.)
            ));
            for _ in 0..4 {
                render(&ctx, &mut layout, vec![], None);
            }
            let after = render(&ctx, &mut layout, vec![], None).0;
            assert!(
                after.width() - before.width() > 100.,
                "native content did not resize: {before:?} -> {after:?}"
            );
            assert!(after.height() > 200.);
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("workspace.json");
            let mut book = Book {
                active: "Edit".into(),
                ..Default::default()
            };
            book.tasks.insert("Edit".into(), layout);
            book.write(&path).unwrap();
            let reopened = Book::read(&path).unwrap();
            assert_eq!(book, reopened);
            let new_ctx = egui::Context::default();
            new_ctx.set_pixels_per_point(scale);
            let mut layout = reopened.tasks["Edit"].clone();
            for _ in 0..4 {
                render(&new_ctx, &mut layout, vec![], None);
            }
            let reopened_body = render(&new_ctx, &mut layout, vec![], None).0;
            assert!(
                (reopened_body.width() - after.width()).abs() < 3.,
                "{reopened_body:?} != {after:?}"
            );
            assert!((reopened_body.height() - after.height()).abs() < 3.);
        }
    }

    #[test]
    fn craft_keyboard_floating_request_releases_native_pointer_resize_after_one_frame() {
        let ctx = egui::Context::default();
        let mut layout = Workspace::preset(&[("media", Place::Floating)]);
        layout.normalize(&["media", "timeline"]);
        assert!(layout.request_floating_size(
            &ctx,
            "editbay-panel-test",
            "media",
            egui::vec2(400., 320.)
        ));
        for _ in 0..4 {
            render(&ctx, &mut layout, vec![], None);
        }
        let group = &layout.groups[0];
        let before = egui::Rect::from_min_size(
            egui::pos2(group.rect[0], group.rect[1]),
            egui::vec2(group.rect[2], group.rect[3]),
        );
        let corner = before.right_bottom() - egui::vec2(3., 3.);
        render(
            &ctx,
            &mut layout,
            vec![egui::Event::PointerMoved(corner)],
            None,
        );
        render(
            &ctx,
            &mut layout,
            vec![egui::Event::PointerButton {
                pos: corner,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            None,
        );
        let moved = corner + egui::vec2(80., 60.);
        render(
            &ctx,
            &mut layout,
            vec![egui::Event::PointerMoved(moved)],
            None,
        );
        render(
            &ctx,
            &mut layout,
            vec![egui::Event::PointerButton {
                pos: moved,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            None,
        );
        for _ in 0..3 {
            render(&ctx, &mut layout, vec![], None);
        }
        assert!(
            layout.groups[0].rect[2] > before.width() + 30.,
            "native manual resize stayed fixed: {:?}",
            layout.groups[0].rect
        );
        assert!(layout.groups[0].rect[3] > before.height() + 20.);
    }
}
