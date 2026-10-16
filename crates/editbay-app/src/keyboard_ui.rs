//! Native keyboard controls retain the application's typed targets.
use eframe::egui::{self, Response, Ui};

/// Supply native modifier transitions for a headless key frame.
/// Args: native events. Returns the same events with each key's modifier state.
#[cfg(test)]
pub(crate) fn with_native_modifiers(events: Vec<egui::Event>) -> Vec<egui::Event> {
    let mut native = vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)];
    for event in events {
        if let egui::Event::Key { modifiers, .. } = &event {
            native.push(egui::Event::ModifiersChanged(*modifiers));
        }
        native.push(event);
    }
    native
}

/// Decorate one native number without changing its type, step or edit precision.
/// Args: UI, owner ID, name, value, native range, optional speed, suffix, reset and field state.
/// Returns the actual widget's FieldResponse and this app-owned draft's current validity.
pub fn native_number<N: egui::emath::Numeric + std::fmt::Display>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    speed: Option<f64>,
    suffix: &str,
    reset: Option<N>,
    state: crate::fields::FieldState<'_>,
) -> (crate::fields::FieldResponse, bool) {
    let key = ui.make_persistent_id(("editbay-native-number", id));
    crate::fields::native_scope(ui, key, |ui| {
        let before = *value;
        let bounds = (*range.start(), *range.end());
        let enabled = ui.is_enabled() && !state.locked;
        let draft_key = key.with("editbay-rejected-number-draft");
        let attempted = std::cell::RefCell::new(None);
        let (text_input, arrow, escape) = ui.input(|input| {
            (
                input.events.iter().any(|event| matches!(event,
                    egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Cut | egui::Event::Ime(_)
                    | egui::Event::Key { key: egui::Key::Backspace | egui::Key::Delete, pressed: true, .. })),
                input.events.iter().any(|event| matches!(event,
                    egui::Event::Key { key: egui::Key::ArrowUp | egui::Key::ArrowDown, pressed: true, modifiers, .. } if !modifiers.any())),
                input.key_pressed(egui::Key::Escape),
            )
        });
        let mut reset_clicked = false;
        let mut editing = false;
        let mut response = ui.vertical(|ui| {
            let name = ui.label(label);
            ui.add_enabled_ui(enabled, |ui| {
                ui.horizontal(|ui| {
                    let mut number = egui::DragValue::new(value)
                        .range(range)
                        .clamp_existing_to_range(false)
                        .suffix(suffix)
                        .custom_formatter(|number, _| {
                            if state.mixed && number == before.to_f64() {
                                "Mixed".into()
                            } else if number == before.to_f64() {
                                before.to_string()
                            } else {
                                let mut number = N::from_f64(number);
                                if number != before {
                                    if number < bounds.0 { number = bounds.0; }
                                    if number > bounds.1 { number = bounds.1; }
                                }
                                number.to_string()
                            }
                        })
                        .custom_parser(|text| {
                            let parsed = parse_native_number(text);
                            attempted.replace(Some((text.to_owned(), parsed.is_some())));
                            parsed
                        });
                    if let Some(speed) = speed { number = number.speed(speed); }
                    editing = ui.memory(|memory| memory.has_focus(ui.next_auto_id()));
                    let response = ui.add(number).labelled_by(name.id);
                    if let Some(reset) = reset
                        && button(ui, (key, "reset"), &format!("Reset {label}")).clicked()
                    {
                        *value = reset;
                        reset_clicked = true;
                    }
                    response
                }).inner
            }).inner
        }).inner;
        let focused = response.has_focus() || response.lost_focus();
        let cancelled = escape && (focused || response.dragged());
        let (native_action, invalid_action) = ui.input(|input| {
            use egui::accesskit::{Action, ActionData};
            (
                [Action::SetValue, Action::Increment, Action::Decrement].into_iter().any(|action| input.accesskit_action_requests(response.id, action).next().is_some()),
                input.accesskit_action_requests(response.id, Action::SetValue).any(|request| matches!(request.data, Some(ActionData::NumericValue(value)) if !value.is_finite())),
            )
        });
        let actual_input = enabled && (reset_clicked || response.dragged() || native_action || ((editing || response.has_focus()) && (text_input || arrow)));
        let mut rejected = ui.data(|data| data.get_temp::<String>(draft_key));
        if cancelled || reset_clicked {
            rejected = None;
        } else if actual_input {
            if invalid_action || !value.to_f64().is_finite() {
                rejected = Some("Enter a finite number".into());
            } else if let Some((text, valid)) = attempted.into_inner() {
                let untouched_mixed = text == "Mixed" && !text_input && rejected.is_none();
                if !untouched_mixed { rejected = (!valid).then_some(text); }
            } else if response.changed() {
                rejected = None;
            }
        }
        if actual_input && value.to_f64().is_finite() {
            if *value < bounds.0 { *value = bounds.0; }
            if *value > bounds.1 { *value = bounds.1; }
        }
        if cancelled || !enabled { *value = before; }
        let valid = rejected.is_none() && value.to_f64().is_finite() && state.error.is_none();
        let changed = actual_input && !cancelled && valid && (response.changed() || reset_clicked);
        if !changed { *value = before; }
        ui.data_mut(|data| {
            if let Some(text) = &rejected { data.insert_temp(draft_key, text.clone()); }
            else { data.remove::<String>(draft_key); }
        });
        if reset_clicked && changed { response.mark_changed(); }
        let mixed = state.mixed && !changed;
        response.widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(egui::WidgetType::DragValue, response.enabled(), label);
            if valid && !mixed { info.value = Some(value.to_f64()); }
            info
        });
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_label(label);
            if mixed { node.set_state_description("Mixed"); }
            if mixed || !valid { node.clear_numeric_value(); }
            else { node.set_numeric_value(value.to_f64()); }
            if mixed && valid { node.set_value("Mixed"); }
        });
        if let Some(message) = state.error.or((!valid).then_some("Enter a finite number")) {
            error(ui, &[response.clone()], message);
        }
        if response.has_focus() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(response.id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() });
                if arrow { memory.move_focus(egui::FocusDirection::None); }
            });
        }
        focus(ui, &response);
        let committed = changed && (reset_clicked || response.drag_stopped() || response.lost_focus() || !response.has_focus());
        (crate::fields::FieldResponse { response, changed, committed, cancelled }, valid)
    }).inner
}

/// Read egui's native decimal grammar while rejecting nonfinite drafts.
/// Args: native edit text. Returns a finite number without changing its native range handling.
fn parse_native_number(text: &str) -> Option<f64> {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .map(|character| if character == '−' { '-' } else { character })
        .collect::<String>()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

#[cfg(test)]
#[path = "keyboard_number_tests.rs"]
mod number_tests;

/// Associate one visible error with its actual native controls.
/// Args: UI, rendered field responses and message. Returns no value.
pub fn error(ui: &mut Ui, responses: &[Response], message: &str) {
    let error = ui.colored_label(ui.visuals().error_fg_color, message);
    for response in responses {
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_invalid(egui::accesskit::Invalid::True);
            node.set_error_message(error.id.accesskit_id());
            node.push_described_by(error.id.accesskit_id());
        });
    }
}

/// Choose an existing target by name.
/// Args: UI, stable identity, visible label, selected value and live choices.
/// Returns the native combo response; arrows and Home/End select without a pointer.
pub fn choice<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    selected: &mut T,
    choices: &[(T, String)],
) -> Response {
    crate::fields::native_scope(ui, egui::Id::new(("editbay-native-choice", id)), |ui| {
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
pub fn button(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, label: &str) -> Response {
    let response =
        crate::fields::native_scope(ui, egui::Id::new(("editbay-native-button", id)), |ui| {
            ui.button(label)
        })
        .inner;
    focus(ui, &response);
    response
}

/// Edit a named text draft.
/// Args: UI, stable identity, label, draft and multiline mode. Returns its native response.
pub fn text(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    value: &mut String,
    multiline: bool,
) -> Response {
    crate::fields::native_scope(ui, egui::Id::new(("editbay-native-text", id)), |ui| {
        text_with_id(
            ui,
            ui.make_persistent_id("text"),
            label,
            value,
            multiline,
            usize::MAX,
        )
    })
    .inner
}

/// Edit a named draft while retaining an existing application identity.
/// Args: UI, native ID, label, draft, multiline mode and character limit.
/// Returns the actual text response; document and text history ownership stay unchanged.
pub fn text_with_id(
    ui: &mut Ui,
    id: egui::Id,
    label: &str,
    value: &mut String,
    multiline: bool,
    limit: usize,
) -> Response {
    let label_response = ui.label(label);
    let edit = if multiline {
        egui::TextEdit::multiline(value)
    } else {
        egui::TextEdit::singleline(value)
    };
    let response = ui
        .add(edit.id(id).char_limit(limit))
        .labelled_by(label_response.id);
    response.widget_info(|| {
        let mut info =
            egui::WidgetInfo::text_edit(response.enabled(), value.as_str(), value.as_str(), "");
        info.label = Some(label.into());
        info
    });
    focus(ui, &response);
    response
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
        let mut frame = |key: Option<egui::Key>, enabled: bool| {
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

    #[test]
    fn craft_keyboard_conditional_preceding_control_preserves_actual_native_id_and_focus() {
        let ctx = egui::Context::default();
        let choices = [(17, "Camera".into()), (42, "Title".into())];
        let mut selected = 17;
        let mut preceding = String::new();
        let mut native_id = None;
        let mut draw = |conditional: bool, key: Option<egui::Key>| {
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
                    if conditional {
                        ui.text_edit_singleline(&mut preceding);
                    }
                    let response = choice(ui, "stable-source", "Source", &mut selected, &choices);
                    if let Some(id) = native_id {
                        assert_eq!(response.id, id);
                    }
                    native_id = Some(response.id);
                },
            )
            .drop_without_applying_deltas();
            (selected, ctx.memory(|memory| memory.focused()) == native_id)
        };
        draw(false, None);
        draw(false, Some(egui::Key::Tab));
        assert_eq!(draw(false, None), (17, true));
        assert_eq!(draw(true, Some(egui::Key::ArrowDown)), (42, true));
        assert_eq!(draw(false, Some(egui::Key::ArrowUp)), (17, true));
    }
}
