//! Native keyboard controls retain the application's typed targets.
use eframe::egui::{self, Response, Ui};

/// Choose an existing target by name.
/// Args: UI, stable identity, visible label, selected value and live choices.
/// Returns the native combo response; arrows and Home/End select without a pointer.
pub fn choice<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    label: &str,
    selected: &mut T,
    choices: &[(T, String)],
) -> Response {
    ui.push_id(id, |ui| {
        let label_response = ui.label(label);
        let before = *selected;
        let mut response = egui::ComboBox::from_id_salt("choice")
            .selected_text(
                choices
                    .iter()
                    .find(|(value, _)| value == selected)
                    .map_or("Choose…", |(_, name)| name.as_str()),
            )
            .show_ui(ui, |ui| {
                for (value, name) in choices {
                    ui.selectable_value(selected, *value, name);
                }
            })
            .response
            .labelled_by(label_response.id);
        if response.has_focus() && response.enabled() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                );
            });
            if !egui::ComboBox::is_open(ui.ctx(), response.id) && !choices.is_empty() {
                let index = choices.iter().position(|(value, _)| value == selected);
                let mut next = None;
                ui.input_mut(|input| {
                    if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                        || input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                    {
                        next = Some(index.map_or(0, |i| (i + 1).min(choices.len() - 1)));
                    } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                        || input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                    {
                        next = Some(index.unwrap_or(0).saturating_sub(1));
                    } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
                        next = Some(0);
                    } else if input.consume_key(egui::Modifiers::NONE, egui::Key::End) {
                        next = Some(choices.len() - 1);
                    }
                });
                if let Some(index) = next {
                    *selected = choices[index].0;
                }
            }
        }
        if *selected != before {
            response.mark_changed();
        }
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::ComboBox,
                response.enabled(),
                format!(
                    "{label}: {}",
                    choices
                        .iter()
                        .find(|(value, _)| value == selected)
                        .map_or("Choose…", |(_, name)| name.as_str())
                ),
            )
        });
        focus(ui, &response);
        response
    })
    .inner
}

/// Draw a named action with stable native focus.
/// Args: UI, stable identity and visible label. Returns its actual button response.
pub fn button(ui: &mut Ui, id: impl std::hash::Hash, label: &str) -> Response {
    let response = ui.push_id(id, |ui| ui.button(label)).inner;
    focus(ui, &response);
    response
}

/// Edit a named text draft.
/// Args: UI, stable identity, label, draft and multiline mode. Returns its native response.
pub fn text(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    label: &str,
    value: &mut String,
    multiline: bool,
) -> Response {
    ui.push_id(id, |ui| {
        let label_response = ui.label(label);
        let edit = if multiline {
            egui::TextEdit::multiline(value)
        } else {
            egui::TextEdit::singleline(value)
        };
        let response = ui.add(edit.id_salt("text")).labelled_by(label_response.id);
        response.widget_info(|| {
            let mut info =
                egui::WidgetInfo::text_edit(response.enabled(), value.as_str(), value.as_str(), "");
            info.label = Some(label.into());
            info
        });
        focus(ui, &response);
        response
    })
    .inner
}

/// Show native keyboard focus.
/// Args: UI and its actual response. Returns no value and never requests focus.
pub fn focus(ui: &Ui, response: &Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.),
            2.,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Outside,
        );
        response.scroll_to_me(Some(egui::Align::Center));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn craft_keyboard_native_choices_tab_arrows_home_end_and_disabled_keep_identity() {
        let ctx = egui::Context::default();
        let choices = [(17, "Camera".into()), (42, "Title".into())];
        let mut selected = 17;
        let mut focused = false;
        let mut frame = |key, enabled| {
            ctx.run_ui(
                egui::RawInput {
                    events: key
                        .map(|key| egui::Event::Key {
                            key,
                            physical_key: Some(key),
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        })
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                |ui| {
                    ui.add_enabled_ui(enabled, |ui| {
                        focused =
                            choice(ui, "source", "Source", &mut selected, &choices).has_focus();
                    });
                },
            )
            .drop_without_applying_deltas();
            (selected, focused)
        };
        frame(None, true);
        frame(Some(egui::Key::Tab), true);
        assert_eq!(frame(None, true), (17, true));
        assert_eq!(frame(Some(egui::Key::ArrowDown), true), (42, true));
        assert_eq!(frame(Some(egui::Key::Home), true), (17, true));
        assert_eq!(frame(Some(egui::Key::End), true), (42, true));
        assert_eq!(frame(Some(egui::Key::ArrowUp), false).0, 42);
    }
}
