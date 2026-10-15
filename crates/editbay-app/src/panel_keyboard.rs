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
    instance: uuid::Uuid,
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
        if (open || button.clicked()) && self.draft.is_none() {
            let panel = layout
                .focus
                .as_deref()
                .filter(|id| registry.iter().any(|entry| entry.0 == *id))
                .or_else(|| registry.first().map(|entry| entry.0));
            if let Some(panel) = panel {
                self.draft = Some(Draft {
                    instance: uuid::Uuid::new_v4(),
                    workspace: workspace_id.into(),
                    panel: panel.into(),
                    target: registry
                        .iter()
                        .find(|entry| entry.0 != panel)
                        .map_or(panel, |entry| entry.0)
                        .into(),
                    operation: Operation::Focus,
                    size: layout
                        .groups
                        .iter()
                        .find(|group| group.panels.iter().any(|id| id == panel))
                        .or_else(|| layout.hidden.get(panel))
                        .map_or([320., 420.], |group| [group.rect[2], group.rect[3]]),
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
        let mut dimensions_valid = true;
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
                draft.size = layout
                    .groups
                    .iter()
                    .find(|group| group.panels.iter().any(|id| id == &draft.panel))
                    .or_else(|| layout.hidden.get(&draft.panel))
                    .map_or([320., 420.], |group| [group.rect[2], group.rect[3]]);
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
                let width = fields::number(
                    ui,
                    ("width", workspace_id, &draft.panel, draft.instance),
                    "Floating width",
                    &mut draft.size[0],
                    120. ..=4096.,
                    " px",
                    None,
                    fields::FieldState::default(),
                );
                let height = fields::number(
                    ui,
                    ("height", workspace_id, &draft.panel, draft.instance),
                    "Floating height",
                    &mut draft.size[1],
                    80. ..=4096.,
                    " px",
                    None,
                    fields::FieldState::default(),
                );
                dimensions_valid =
                    fields::number_is_valid(&width) && fields::number_is_valid(&height);
            }
            if let Some(error) = &draft.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.horizontal(|ui| {
                apply = ui
                    .add_enabled_ui(dimensions_valid, |ui| {
                        keyboard_ui::button(ui, "apply", "Apply")
                    })
                    .inner
                    .clicked();
                close = keyboard_ui::button(ui, "cancel", "Cancel").clicked();
            });
            close |=
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        });
        if apply && dimensions_valid {
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
            if !apply && let Some(panel) = layout.focus.clone() {
                layout.request_focus(ui.ctx(), workspace_id, &panel);
            }
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
    ) -> (
        egui::Rect,
        Option<egui::Id>,
        Vec<String>,
        Vec<egui::Rect>,
        Vec<String>,
    ) {
        let mut body = egui::Rect::NOTHING;
        let mut focus_trace = vec![];
        let events = crate::keyboard_ui::with_native_modifiers(events);
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
                    let focused = ui.memory(|memory| memory.focused());
                    let response = focused.and_then(|id| ui.ctx().read_response(id));
                    focus_trace.push(format!(
                        "{panel}: focus={focused:?}, enabled={:?}, layer_allows={:?}, modal={:?}",
                        response.as_ref().map(egui::Response::enabled),
                        response.as_ref().map(|response| ui
                            .memory(|memory| memory.allows_interaction(response.layer_id))),
                        ui.memory(|memory| memory.top_modal_layer())
                    ));
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
                if let egui::output::OutputEvent::FocusGained(info) = event {
                    info.label.clone()
                } else {
                    None
                }
            })
            .collect();
        fn painted_rectangles(shape: &egui::epaint::Shape, rects: &mut Vec<egui::Rect>) {
            match shape {
                egui::epaint::Shape::Rect(rect) => rects.push(rect.rect),
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        painted_rectangles(shape, rects);
                    }
                }
                _ => {}
            }
        }
        let mut painted = vec![];
        for shape in &output.shapes {
            painted_rectangles(&shape.shape, &mut painted);
        }
        output.drop_without_applying_deltas();
        (
            body,
            ctx.memory(|memory| memory.focused()),
            labels,
            painted,
            focus_trace,
        )
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
            let mut native_labels = applied.2;
            let mut focus_trace = applied.4;
            for _ in 0..3 {
                let next = render(&ctx, &mut layout, vec![], Some(&mut keyboard));
                native_labels.extend(next.2);
                focus_trace.extend(next.4);
            }
            assert!(!keyboard.opened());
            assert_eq!(layout.focus.as_deref(), Some("media"));
            assert!(!layout.hidden.contains_key("media"));
            let focused = ctx.memory(|memory| memory.focused());
            assert!(
                focused.is_some(),
                "explicit same-panel request must focus its actual tab: {focus_trace:?}"
            );
            assert!(
                native_labels.iter().any(|label| label == "Media"),
                "native focus did not return to Media: {:?}",
                native_labels
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
            let first = render(&ctx, &mut layout, vec![], None);
            let rect = layout
                .groups
                .iter()
                .find(|group| group.panels.iter().any(|panel| panel == "media"))
                .unwrap()
                .rect;
            assert!(
                (rect[2] - 520.).abs() < 2. && (rect[3] - 360.).abs() < 2.,
                "native shown group missed the requested first-frame footprint: {rect:?}"
            );
            assert!(
                first
                    .3
                    .iter()
                    .any(|rect| (rect.width() - 520.).abs() < 2.
                        && (rect.height() - 360.).abs() < 2.),
                "native window frame was not actually painted at the requested size: {:?}",
                first.3
            );
            for _ in 0..4 {
                let next = render(&ctx, &mut layout, vec![], None);
                assert!(
                    (next.0.width() - first.0.width()).abs() < 3.
                        && (next.0.height() - first.0.height()).abs() < 3.,
                    "actual visible body changed after the transient request: {:?} -> {:?}",
                    first.0,
                    next.0
                );
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

    #[test]
    fn craft_keyboard_malformed_floating_dimension_skips_disabled_apply_and_preserves_geometry() {
        let ctx = egui::Context::default();
        let mut layout = Workspace::preset(&[("media", Place::Floating)]);
        layout.normalize(&["media", "timeline"]);
        let mut keyboard = PanelKeyboard::default();
        for _ in 0..3 {
            render(&ctx, &mut layout, vec![], Some(&mut keyboard));
        }
        let before = layout.groups[0].rect;
        render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::F6, egui::Modifiers::CTRL)],
            Some(&mut keyboard),
        );
        render(&ctx, &mut layout, vec![], Some(&mut keyboard));
        render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
            Some(&mut keyboard),
        );
        render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::End, egui::Modifiers::NONE)],
            Some(&mut keyboard),
        );
        let width = render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
            Some(&mut keyboard),
        );
        assert!(width.2.iter().any(|label| label == "Floating width"));
        render(
            &ctx,
            &mut layout,
            vec![
                key(
                    egui::Key::A,
                    egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
                ),
                egui::Event::Text("-".into()),
            ],
            Some(&mut keyboard),
        );
        render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
            Some(&mut keyboard),
        );
        let next = render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
            Some(&mut keyboard),
        );
        assert!(
            next.2.iter().any(|label| label == "Cancel"),
            "malformed field left Apply focusable: {:?}",
            next.2
        );
        assert!(!next.2.iter().any(|label| label == "Apply"));
        assert!(keyboard.opened());
        assert_eq!(layout.groups[0].rect, before);
        render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
            Some(&mut keyboard),
        );
        assert!(!keyboard.opened());
        render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::F6, egui::Modifiers::CTRL)],
            Some(&mut keyboard),
        );
        render(&ctx, &mut layout, vec![], Some(&mut keyboard));
        render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
            Some(&mut keyboard),
        );
        render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::End, egui::Modifiers::NONE)],
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
            vec![
                key(
                    egui::Key::A,
                    egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
                ),
                egui::Event::Text("480".into()),
            ],
            Some(&mut keyboard),
        );
        render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
            Some(&mut keyboard),
        );
        let recovered = render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
            Some(&mut keyboard),
        );
        assert!(
            recovered.2.iter().any(|label| label == "Apply"),
            "fresh valid draft inherited canceled text: {:?}",
            recovered.2
        );
        render(
            &ctx,
            &mut layout,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
            Some(&mut keyboard),
        );
        for _ in 0..3 {
            render(&ctx, &mut layout, vec![], Some(&mut keyboard));
        }
        assert!(!keyboard.opened());
        assert!(layout.groups[0].rect[2] > before[2] + 100.);
    }
}
