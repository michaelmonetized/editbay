use super::*;
use crate::fields::{FieldResponse, FieldState};
use crate::workflow_ui::keyboard_tests::key;
use std::{collections::HashMap, fmt::Display, ops::RangeInclusive};
use uuid::Uuid;

struct Native<N> {
    ctx: egui::Context,
    owner: Uuid,
    value: N,
    reset: N,
    range: RangeInclusive<N>,
    mixed: bool,
    locked: bool,
    error: Option<&'static str>,
    conditional: bool,
    field: Option<FieldResponse>,
    valid: bool,
    changes: usize,
    focus: String,
    nodes: HashMap<egui::accesskit::NodeId, egui::accesskit::Node>,
}

impl<N: egui::emath::Numeric + Display> Native<N> {
    fn new(value: N, reset: N, range: RangeInclusive<N>) -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Self {
            ctx,
            owner: Uuid::new_v4(),
            value,
            reset,
            range,
            mixed: false,
            locked: false,
            error: None,
            conditional: false,
            field: None,
            valid: true,
            changes: 0,
            focus: String::new(),
            nodes: HashMap::new(),
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        let output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700., 700.),
                )),
                events: with_native_modifiers(events),
                ..Default::default()
            },
            |ui| {
                if self.conditional {
                    ui.label("Conditional native number status");
                }
                let (field, valid) = native_number(
                    ui,
                    ("number-fixture", self.owner),
                    "Native number",
                    &mut self.value,
                    self.range.clone(),
                    None,
                    "",
                    Some(self.reset),
                    FieldState {
                        mixed: self.mixed,
                        locked: self.locked,
                        error: self.error,
                    },
                );
                self.changes += usize::from(field.changed);
                self.field = Some(field);
                self.valid = valid;
                button(ui, ("number-fixture-done", self.owner), "Done");
            },
        );
        for event in &output.platform_output.events {
            if let egui::output::OutputEvent::FocusGained(info) = event {
                self.focus = info.label.clone().unwrap_or_default();
            }
        }
        if self.ctx.memory(|memory| memory.focused().is_none()) {
            self.focus.clear();
        }
        if let Some(update) = &output.platform_output.accesskit_update {
            self.nodes.extend(update.nodes.iter().cloned());
        }
        output.drop_without_applying_deltas();
    }

    fn tab_to(&mut self, label: &str) {
        for _ in 0..24 {
            self.frame(vec![key(egui::Key::Tab, egui::Modifiers::NONE)]);
            self.frame(vec![]);
            if self.focus == label {
                if label == "Native number" {
                    assert_eq!(
                        self.ctx.memory(|memory| memory.focused()),
                        Some(self.field.as_ref().unwrap().response.id)
                    );
                }
                return;
            }
        }
        panic!(
            "Cannot reach {label:?} by native Tab; last focus {:?}",
            self.focus
        );
    }

    fn replace(&mut self, text: &str) {
        self.frame(vec![
            key(
                egui::Key::A,
                egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            ),
            egui::Event::Text(text.into()),
        ]);
    }

    fn node(&self) -> &egui::accesskit::Node {
        &self.nodes[&egui::accesskit::NodeId(self.field.as_ref().unwrap().response.id.value())]
    }
}

#[test]
fn craft_keyboard_common_number_exact_fractions_render_focus_blur_keep_value_and_native_metadata() {
    let mut single = Native::new(0.12345679_f32, 0.05_f32, 0.0..=0.4);
    let single_bits = single.value.to_bits();
    for _ in 0..6 {
        single.frame(vec![]);
    }
    single.tab_to("Native number");
    let id = single.field.as_ref().unwrap().response.id;
    assert_eq!(single.node().numeric_value(), Some(f64::from(single.value)));
    assert_eq!(
        single.node().value(),
        Some(single.value.to_string().as_str())
    );
    single.conditional = true;
    single.frame(vec![]);
    assert_eq!(single.ctx.memory(|memory| memory.focused()), Some(id));
    single.tab_to("Done");
    for _ in 0..6 {
        single.frame(vec![]);
    }
    assert_eq!(single.value.to_bits(), single_bits);
    assert_eq!(single.node().numeric_value(), Some(f64::from(single.value)));
    assert_eq!(
        single.changes, 0,
        "render/focus/blur must return no accepted edit"
    );

    let mut double = Native::new(203.12345678912346_f64, 100.0_f64, 1.0..=10_000.0);
    let double_bits = double.value.to_bits();
    for _ in 0..6 {
        double.frame(vec![]);
    }
    double.tab_to("Native number");
    assert_eq!(double.node().numeric_value(), Some(double.value));
    assert_eq!(
        double.node().value(),
        Some(double.value.to_string().as_str())
    );
    double.tab_to("Done");
    for _ in 0..6 {
        double.frame(vec![]);
    }
    assert_eq!(double.value.to_bits(), double_bits);
    assert_eq!(double.node().numeric_value(), Some(double.value));
    assert_eq!(double.changes, 0);

    let mut loaded = Native::new(700.1235_f32, 48.0_f32, 4.0..=512.0);
    let loaded_bits = loaded.value.to_bits();
    loaded.frame(vec![]);
    loaded.tab_to("Native number");
    loaded.tab_to("Done");
    loaded.frame(vec![]);
    assert_eq!(
        loaded.value.to_bits(),
        loaded_bits,
        "loaded model values are not edit requests"
    );
    assert_eq!(loaded.node().numeric_value(), Some(f64::from(loaded.value)));
    assert_eq!(loaded.changes, 0);
}

#[test]
fn craft_keyboard_common_number_mixed_invalid_escape_reset_disabled_stale_and_native_clamp() {
    let mut native = Native::new(0.031234567_f32, 0.05_f32, 0.0..=0.4);
    native.mixed = true;
    let original = native.value.to_bits();
    native.frame(vec![]);
    native.tab_to("Native number");
    let id = native.field.as_ref().unwrap().response.id;
    assert!(native.valid);
    assert_eq!(native.node().numeric_value(), None);
    assert_eq!(native.node().state_description(), Some("Mixed"));
    native.tab_to("Done");
    assert!(
        native.valid,
        "native formatter Mixed is untouched, valid model state"
    );
    assert_eq!(native.value.to_bits(), original);
    assert_eq!(native.changes, 0);
    native.tab_to("Native number");
    native.replace("-");
    assert!(!native.valid);
    assert!(!native.field.as_ref().unwrap().changed);
    assert_eq!(native.node().numeric_value(), None);
    assert_eq!(
        native.node().invalid(),
        Some(egui::accesskit::Invalid::True)
    );
    assert!(native.node().error_message().is_some());
    assert_eq!(native.value.to_bits(), original);
    native.frame(vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
    assert!(native.field.as_ref().unwrap().cancelled);
    assert!(native.valid);
    assert_eq!(native.value.to_bits(), original);
    native.tab_to("Native number");
    native.replace("0.23");
    assert!(native.valid && native.field.as_ref().unwrap().changed);
    assert_eq!(native.value, 0.23);
    assert_eq!(native.node().numeric_value(), Some(f64::from(0.23_f32)));
    native.mixed = false;
    native.tab_to("Reset Native number");
    native.frame(vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    assert!(native.field.as_ref().unwrap().changed);
    assert_eq!(native.value, 0.05);
    assert_eq!(native.field.as_ref().unwrap().response.id, id);
    native.locked = true;
    native.frame(vec![]);
    assert!(!native.field.as_ref().unwrap().response.enabled());
    native.replace("0.2");
    assert!(!native.field.as_ref().unwrap().changed);
    assert_eq!(native.value, 0.05);
    native.locked = false;
    native.tab_to("Native number");
    native.replace("-");
    assert!(!native.valid);
    native.owner = Uuid::new_v4();
    native.value = 0.37;
    native.frame(vec![]);
    assert!(native.valid);
    assert!(!native.field.as_ref().unwrap().changed);
    assert_ne!(native.field.as_ref().unwrap().response.id, id);
    assert_eq!(native.node().numeric_value(), Some(f64::from(0.37_f32)));
    native.tab_to("Native number");
    native.replace("9");
    assert!(native.valid && native.field.as_ref().unwrap().changed);
    assert_eq!(
        native.value, 0.4,
        "actual native edit keeps its original clamp"
    );
    native.error = Some("The number owner changed");
    native.replace("0.3");
    assert!(!native.valid && !native.field.as_ref().unwrap().changed);
    assert_eq!(native.value, 0.4);
    assert_eq!(native.node().numeric_value(), None);
}
