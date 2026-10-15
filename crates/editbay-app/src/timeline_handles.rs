//! Direct timeline gestures project into the existing typed actions.
use editbay_core::{FrameRange, TimelineAction, TimelineClip};
use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Move,
    TrimStart,
    TrimEnd,
    Slip,
    Roll,
    Slide,
}

pub struct Handles {
    pub tool: Tool,
    pub snapping: bool,
    drag: Option<(TimelineClip, Tool)>,
}

impl Default for Handles {
    fn default() -> Self {
        Self {
            tool: Tool::Move,
            snapping: true,
            drag: None,
        }
    }
}

pub struct Change {
    pub clip: Uuid,
    pub actions: Vec<TimelineAction>,
    pub finished: bool,
}

/// Snap a proposed frame to a nearby cut or playhead.
/// Args: proposed frame, candidate frames and tolerance. Returns the closest frame,
/// preferring the smaller candidate for deterministic ties.
pub fn snap(frame: u64, candidates: &[u64], tolerance: u64) -> u64 {
    candidates
        .iter()
        .copied()
        .filter(|at| at.abs_diff(frame) <= tolerance)
        .min_by_key(|at| (at.abs_diff(frame), *at))
        .unwrap_or(frame)
}

/// Convert one gesture into typed timeline actions.
/// Args: composition, original clip, tool, frame delta, ripple mode and source duration.
/// Returns validated-range actions; document validation remains owned by the timeline worker.
pub fn actions(
    composition: Uuid,
    clip: &TimelineClip,
    tool: Tool,
    delta: i64,
    ripple: bool,
    source_duration: u64,
) -> Vec<TimelineAction> {
    let shifted =
        |at: u64| (i128::from(at) + i128::from(delta)).clamp(0, i128::from(u64::MAX)) as u64;
    match tool {
        Tool::Move => vec![TimelineAction::Move {
            composition,
            clip: clip.id,
            at: shifted(clip.range.start),
        }],
        Tool::Slip => vec![TimelineAction::Slip {
            composition,
            clip: clip.id,
            frames: delta,
        }],
        Tool::Roll => vec![TimelineAction::Roll {
            composition,
            clip: clip.id,
            at: shifted(clip.range.end),
        }],
        Tool::Slide => vec![TimelineAction::Slide {
            composition,
            clip: clip.id,
            at: shifted(clip.range.start),
        }],
        Tool::TrimStart => {
            let start = shifted(clip.source.range.start).min(clip.source.range.end - 1);
            let delta = i128::from(start) - i128::from(clip.source.range.start);
            let mut result = vec![TimelineAction::Trim {
                composition,
                clip: clip.id,
                source_range: FrameRange {
                    start,
                    end: clip.source.range.end,
                },
                ripple,
            }];
            if !ripple {
                result.push(TimelineAction::Move {
                    composition,
                    clip: clip.id,
                    at: (i128::from(clip.range.start) + delta).clamp(0, i128::from(u64::MAX))
                        as u64,
                });
            }
            result
        }
        Tool::TrimEnd => vec![TimelineAction::Trim {
            composition,
            clip: clip.id,
            source_range: FrameRange {
                start: clip.source.range.start,
                end: shifted(clip.source.range.end)
                    .clamp(clip.source.range.start + 1, source_duration),
            },
            ripple,
        }],
    }
}

impl Handles {
    /// Draw source-aware clip bodies and trim edges.
    /// Args: UI, composition, clips, source durations, selection, playhead and ripple mode.
    /// Returns a coalescible typed gesture update; release commits and Escape cancels upstream.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        composition: Uuid,
        clips: &[TimelineClip],
        source_duration: impl Fn(Uuid) -> u64,
        selected: Option<Uuid>,
        playhead: u64,
        ripple: bool,
    ) -> Option<Change> {
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt(("timeline-tool", composition))
                .selected_text(format!("{:?}", self.tool))
                .show_ui(ui, |ui| {
                    for (tool, label) in [
                        (Tool::Move, "Move / edge trim"),
                        (Tool::Slip, "Slip"),
                        (Tool::Roll, "Roll outgoing cut"),
                        (Tool::Slide, "Slide"),
                    ] {
                        ui.selectable_value(&mut self.tool, tool, label);
                    }
                });
            ui.checkbox(&mut self.snapping, "Snap to cuts / playhead");
        });
        ui.weak("Drag a clip or its edges · Alt: slip · Shift: slide · Ctrl: roll · arrows: nudge · Enter: commit · Esc: cancel");
        let end = clips
            .iter()
            .map(|c| c.range.end)
            .max()
            .unwrap_or(48)
            .max(48);
        let pixels = ui.available_width().max(1.) / (end as f32 * 1.15);
        let mut tracks: Vec<_> = clips.iter().map(|clip| clip.track).collect();
        tracks.sort();
        tracks.dedup();
        let (area, _) = ui.allocate_exact_size(
            vec2(
                ui.available_width(),
                (tracks.len().max(1) as f32 * 38.) + 24.,
            ),
            Sense::hover(),
        );
        let ruler = Rect::from_min_max(area.min, pos2(area.right(), area.top() + 20.));
        ui.painter()
            .rect_filled(area, 3., ui.visuals().extreme_bg_color);
        for i in 0..=8 {
            let frame = end * i / 8;
            let x = area.left() + frame as f32 * pixels;
            ui.painter().text(
                pos2(x, ruler.center().y),
                Align2::LEFT_CENTER,
                frame.to_string(),
                FontId::proportional(10.),
                ui.visuals().weak_text_color(),
            );
        }
        let mut change = None;
        for clip in clips {
            let row = tracks
                .iter()
                .position(|track| *track == clip.track)
                .unwrap_or(0);
            let rect = Rect::from_min_max(
                pos2(
                    area.left() + clip.range.start as f32 * pixels,
                    area.top() + 24. + row as f32 * 38.,
                ),
                pos2(
                    area.left() + clip.range.end as f32 * pixels,
                    area.top() + 58. + row as f32 * 38.,
                ),
            );
            let width = rect.width().min(14.).max(2.) / 2.;
            let left = Rect::from_min_max(rect.min, pos2(rect.left() + width, rect.bottom()));
            let right = Rect::from_min_max(pos2(rect.right() - width, rect.top()), rect.max);
            let body = Rect::from_min_max(
                pos2(left.right(), rect.top()),
                pos2(right.left(), rect.bottom()),
            );
            let color = if selected == Some(clip.id) {
                ui.visuals().selection.bg_fill
            } else {
                Color32::from_rgb(55, 70, 85)
            };
            ui.painter().rect_filled(rect.shrink(1.), 3., color);
            ui.painter().with_clip_rect(rect.shrink(4.)).text(
                rect.left_center() + vec2(7., 0.),
                Align2::LEFT_CENTER,
                &clip.name,
                FontId::proportional(11.),
                ui.visuals().text_color(),
            );
            for edge in [left, right] {
                ui.painter()
                    .rect_filled(edge.shrink2(vec2(1., 6.)), 1., ui.visuals().text_color());
            }
            let modifier = ui.input(|i| i.modifiers);
            for (zone, base_tool, key) in [
                (body, self.tool, "body"),
                (left, Tool::TrimStart, "in"),
                (right, Tool::TrimEnd, "out"),
            ] {
                let response =
                    ui.interact(zone, ui.id().with((clip.id, key)), Sense::click_and_drag());
                if response.clicked() {
                    change = Some(Change {
                        clip: clip.id,
                        actions: Vec::new(),
                        finished: false,
                    });
                }
                if response.drag_started() {
                    let tool = if modifier.alt {
                        Tool::Slip
                    } else if modifier.ctrl || modifier.command {
                        Tool::Roll
                    } else if modifier.shift {
                        Tool::Slide
                    } else {
                        base_tool
                    };
                    self.drag = Some((clip.clone(), tool));
                }
                if (response.dragged() || response.drag_stopped())
                    && let Some((original, tool)) = &self.drag
                {
                    let delta = (response.drag_delta().x / pixels).round() as i64;
                    let anchor = match tool {
                        Tool::TrimEnd | Tool::Roll => original.range.end,
                        _ => original.range.start,
                    };
                    let proposed = (i128::from(anchor) + i128::from(delta))
                        .clamp(0, i128::from(u64::MAX)) as u64;
                    let mut candidates = vec![0, playhead];
                    candidates.extend(
                        clips
                            .iter()
                            .filter(|c| c.id != original.id && c.linked != Some(original.id))
                            .flat_map(|c| [c.range.start, c.range.end]),
                    );
                    if *tool == Tool::Move {
                        let duration = original.range.end - original.range.start;
                        candidates.extend(
                            candidates
                                .clone()
                                .into_iter()
                                .filter_map(|at| at.checked_sub(duration)),
                        );
                    }
                    let at = if self.snapping && !modifier.alt {
                        snap(proposed, &candidates, (8. / pixels).ceil() as u64)
                    } else {
                        proposed
                    };
                    let delta = (i128::from(at) - i128::from(anchor))
                        .clamp(i128::from(i64::MIN), i128::from(i64::MAX))
                        as i64;
                    let x = area.left() + at as f32 * pixels;
                    ui.painter().line_segment(
                        [pos2(x, area.top()), pos2(x, area.bottom())],
                        Stroke::new(1., ui.visuals().selection.stroke.color),
                    );
                    change = Some(Change {
                        clip: original.id,
                        actions: actions(
                            composition,
                            original,
                            *tool,
                            delta,
                            ripple,
                            source_duration(original.source.composition),
                        ),
                        finished: response.drag_stopped(),
                    });
                }
                if response.drag_stopped() {
                    self.drag = None;
                }
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.drag = None;
        }
        change
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapping_is_bounded_and_deterministic() {
        assert_eq!(snap(12, &[14, 10, 60], 2), 10);
        assert_eq!(snap(12, &[14, 10, 60], 1), 12);
        assert_eq!(snap(u64::MAX, &[0, u64::MAX - 1], 2), u64::MAX - 1);
    }
    #[test]
    fn direct_trim_and_move_keep_source_range_and_typed_ripple_semantics() {
        let clip = TimelineClip {
            track: Uuid::new_v4(),
            audio_only: false,
            controls: Default::default(),
            id: Uuid::new_v4(),
            linked: None,
            name: "Clip".into(),
            range: FrameRange { start: 20, end: 40 },
            source: editbay_core::SourceSelection {
                composition: Uuid::new_v4(),
                range: FrameRange { start: 10, end: 30 },
            },
        };
        let c = Uuid::new_v4();
        let edit = actions(c, &clip, Tool::TrimStart, 5, false, 100);
        assert!(matches!(
            &edit[0],
            TimelineAction::Trim {
                source_range: FrameRange { start: 15, end: 30 },
                ripple: false,
                ..
            }
        ));
        assert!(matches!(&edit[1], TimelineAction::Move { at: 25, .. }));
        assert_eq!(actions(c, &clip, Tool::TrimStart, 5, true, 100).len(), 1);
        assert!(matches!(
            &actions(c, &clip, Tool::TrimEnd, 1000, false, 100)[0],
            TimelineAction::Trim {
                source_range: FrameRange {
                    start: 10,
                    end: 100
                },
                ..
            }
        ));
    }
}
