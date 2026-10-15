//! Keyboard authoring uses the same owned timeline commands as pointer editing.
use super::*;
use crate::keyboard_ui;
use editbay_core::{Composition, DocumentCommand, NodeOperation, TimedNode, Track, TrackKind};

impl TimelinePane {
    /// Create the document's empty sequence through its captured editor.
    /// Args: live workspace and tab. Returns after dispatch; stale ownership rejects publication.
    pub(super) fn create_empty(&mut self, workspace: &Workspace, tab: Uuid) -> Result<(), String> {
        if self.task.is_some() || self.draft.is_some() {
            return Err("Finish the current timeline edit first".into());
        }
        let (owner, mut editor) = workspace.edit_snapshot(tab)?;
        let mut sequence = editor
            .project()
            .sequences
            .iter()
            .find(|sequence| sequence.composition.is_none())
            .cloned()
            .ok_or("Choose an empty sequence profile")?;
        let composition = Uuid::new_v4();
        let node = Uuid::new_v4();
        sequence.composition = Some(composition);
        let sequence_id = sequence.id;
        let commands = vec![
            DocumentCommand::SetComposition {
                composition: Composition {
                    id: composition,
                    name: sequence.name.clone(),
                    width: sequence.width,
                    height: sequence.height,
                    frame_rate: sequence.frame_rate,
                    duration: 1,
                    tracks: [(TrackKind::Video, "Video 1"), (TrackKind::Audio, "Sound 1")]
                        .into_iter()
                        .map(|(kind, name)| Track {
                            id: Uuid::new_v4(),
                            kind,
                            name: name.into(),
                            enabled: true,
                            clips: vec![],
                        })
                        .collect(),
                    nodes: vec![TimedNode {
                        id: node,
                        range: FrameRange { start: 0, end: 1 },
                        operation: NodeOperation::Solid { rgba: [0.; 4] },
                        animation: vec![],
                    }],
                    picture: Some(node),
                    audio: None,
                },
            },
            DocumentCommand::SetSequence { sequence },
            DocumentCommand::SetPrimarySequence { id: sequence_id },
        ];
        let (sender, result) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("editbay-empty-sequence".into())
            .spawn(move || {
                let result = editor
                    .apply(owner.version, "Create empty sequence".into(), &commands)
                    .map(|_| Completed::Edit {
                        editor,
                        composition,
                        clip: None,
                        frame: 0,
                        trim: None,
                    })
                    .map_err(|error| error.to_string());
                let _ = sender.send(result);
            })
            .map_err(|error| error.to_string())?;
        self.task = Some(Task {
            draft: None,
            owner,
            result,
        });
        self.error = None;
        Ok(())
    }
}

pub(super) fn title_controls(
    ui: &mut Ui,
    selected: &mut Selection,
    source: &Composition,
    tab: Uuid,
) -> Option<TimelineAction> {
    let mut action = None;
    ui.push_id(("title-authoring", tab), |ui| {
        let text = keyboard_ui::text(ui, "text", "Title text", &mut selected.title, true);
        let valid =
            !selected.title.is_empty() && selected.title.len() <= 1024 && selected.title.is_ascii();
        if !valid {
            let error = ui.colored_label(
                ui.visuals().error_fg_color,
                "Enter 1–1024 ASCII characters.",
            );
            text.labelled_by(error.id);
        }
        let mut path = selected.font.to_string_lossy().into_owned();
        let response = keyboard_ui::text(ui, "font", "Font file", &mut path, false);
        if response.changed() {
            selected.font = path.into();
        }
        fields::number(
            ui,
            "size",
            "Title size",
            &mut selected.size,
            1. ..=512.,
            " px",
            None,
            FieldState::default(),
        );
        fields::number(
            ui,
            "x",
            "Title baseline X",
            &mut selected.baseline[0],
            -1e6..=1e6,
            " px",
            None,
            FieldState::default(),
        );
        fields::number(
            ui,
            "y",
            "Title baseline Y",
            &mut selected.baseline[1],
            -1e6..=1e6,
            " px",
            None,
            FieldState::default(),
        );
        fields::number(
            ui,
            "duration",
            "Title duration",
            &mut selected.title_duration,
            1..=u64::MAX,
            " frames",
            None,
            FieldState::default(),
        );
        if ui
            .add_enabled_ui(valid, |ui| {
                keyboard_ui::button(ui, "create-title", "Create title source")
            })
            .inner
            .clicked()
        {
            action = Some(TimelineAction::Title {
                profile: selected.record.unwrap_or(source.id),
                name: selected.name.clone(),
                text: selected.title.clone(),
                font: selected.font.clone(),
                size: selected.size,
                position: selected.baseline,
                rgba: [1.; 4],
                duration: selected.title_duration,
            });
        }
        if source
            .nodes
            .iter()
            .any(|node| matches!(node.operation, NodeOperation::Text { .. }))
            && ui
                .add_enabled_ui(valid, |ui| {
                    keyboard_ui::button(ui, "edit-title", "Update selected title")
                })
                .inner
                .clicked()
        {
            action = Some(TimelineAction::EditTitle {
                composition: source.id,
                text: selected.title.clone(),
                size: selected.size,
                position: selected.baseline,
                rgba: [1.; 4],
            });
        }
    });
    action
}

pub(super) fn select_clips(
    ui: &mut Ui,
    preview: &mut PreviewPane,
    selected: &mut Selection,
    scene: &Composition,
    assembly: &Assembly,
    tab: Uuid,
) -> bool {
    let mut responses = Vec::new();
    let mut choice = None;
    egui::ScrollArea::vertical()
        .id_salt(("linked-clips", tab))
        .max_height(112.)
        .show(ui, |ui| {
            if assembly.clips.is_empty() {
                ui.weak("Empty cut. Append a source range to begin.");
            }
            for (index, clip) in assembly.clips.iter().enumerate() {
                let track = scene
                    .tracks
                    .iter()
                    .find(|track| track.id == clip.track)
                    .map_or("Track", |track| track.name.as_str());
                let label = format!(
                    "{track} {}  {}–{}  ← {}–{}  {}",
                    if clip.linked.is_some() {
                        "+ sound 🔗"
                    } else {
                        ""
                    },
                    clip.range.start,
                    clip.range.end,
                    clip.source.range.start,
                    clip.source.range.end,
                    clip.name
                );
                let response = ui
                    .push_id(("timeline-clip", tab, clip.id), |ui| {
                        ui.selectable_label(
                            selected.clip == Some(clip.id) || selected.multiple.contains(&clip.id),
                            &label,
                        )
                    })
                    .inner;
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        response.enabled(),
                        selected.clip == Some(clip.id) || selected.multiple.contains(&clip.id),
                        format!(
                            "Clip {} · {track} · frames {} to {}",
                            clip.name, clip.range.start, clip.range.end
                        ),
                    )
                });
                preview.observe_control(&format!("timeline-clip:{}", clip.id), &response, ui);
                keyboard_ui::focus(ui, &response);
                if response.clicked() {
                    choice = Some(index);
                }
                if response.has_focus() {
                    ui.memory_mut(|memory| {
                        memory.set_focus_lock_filter(
                            response.id,
                            egui::EventFilter {
                                horizontal_arrows: true,
                                vertical_arrows: true,
                                ..Default::default()
                            },
                        )
                    });
                    let modifiers = ui.input(|input| input.modifiers);
                    ui.input_mut(|input| {
                        if input.consume_key(modifiers, egui::Key::ArrowDown) {
                            choice = Some((index + 1).min(assembly.clips.len() - 1));
                        } else if input.consume_key(modifiers, egui::Key::ArrowUp) {
                            choice = Some(index.saturating_sub(1));
                        } else if input.consume_key(modifiers, egui::Key::Home) {
                            choice = Some(0);
                        } else if input.consume_key(modifiers, egui::Key::End) {
                            choice = Some(assembly.clips.len() - 1);
                        }
                    });
                }
                responses.push(response);
            }
        });
    let focused = responses.iter().any(egui::Response::has_focus);
    if ui.is_enabled()
        && (ui.ctx().memory(|memory| {
            memory.focused().is_none()
                || memory.has_focus(egui::Id::new(("timeline-content", scene.id)))
        }) || focused)
        && !assembly.clips.is_empty()
    {
        let current = assembly
            .clips
            .iter()
            .position(|clip| Some(clip.id) == selected.clip);
        let modifiers = ui.input(|input| input.modifiers);
        ui.input_mut(|input| {
            if input.consume_key(modifiers, egui::Key::PageDown) {
                choice = Some(current.map_or(0, |index| (index + 1).min(assembly.clips.len() - 1)));
            } else if input.consume_key(modifiers, egui::Key::PageUp) {
                choice =
                    Some(current.map_or(assembly.clips.len() - 1, |index| index.saturating_sub(1)));
            }
        });
    }
    if let Some(index) = choice {
        let clip = &assembly.clips[index];
        if ui
            .input(|input| input.modifiers.shift || input.modifiers.command || input.modifiers.ctrl)
        {
            if selected.multiple.is_empty() {
                selected.multiple.extend(selected.clip);
            }
            if !selected.multiple.contains(&clip.id) {
                selected.multiple.push(clip.id);
            }
        } else {
            selected.multiple.clear();
        }
        selected.clip = Some(clip.id);
        selected.trim_range = clip.source.range;
        selected.at = clip.range.start;
        selected.controls = clip.controls;
        responses[index].request_focus();
        responses[index].scroll_to_me(Some(egui::Align::Center));
    }
    focused || choice.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    struct Author {
        ctx: egui::Context,
        workspace: Workspace,
        pane: TimelinePane,
        preview: PreviewPane,
        tab: Uuid,
        focus: String,
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    impl Author {
        fn new(root: &std::path::Path) -> Self {
            let ctx = egui::Context::default();
            let mut workspace = Workspace::new(root.join("recovery"), ctx.clone());
            let tab = workspace
                .create(Project::new("Keyboard authoring").unwrap())
                .unwrap();
            Self {
                ctx,
                workspace,
                pane: TimelinePane::default(),
                preview: PreviewPane::default(),
                tab,
                focus: String::new(),
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            self.pane
                .poll(&mut self.workspace, &mut self.preview, true, &self.ctx);
            let project = self.workspace.tabs[0].editor.snapshot();
            let output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200., 3000.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    self.pane.show(
                        ui,
                        &self.workspace,
                        &mut self.preview,
                        self.tab,
                        project.clone(),
                        true,
                    )
                },
            );
            for event in &output.platform_output.events {
                if let egui::OutputEvent::FocusGained(info) = event {
                    self.focus = info.label.clone().unwrap_or_default();
                }
            }
            output.drop_without_applying_deltas();
        }

        fn press(&mut self, code: egui::Key) {
            self.frame(vec![key(code, egui::Modifiers::NONE)]);
        }

        fn settled(&mut self, revision: u64) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                self.frame(vec![]);
                let project = self.workspace.tabs[0].editor.project();
                if project.revision == revision
                    && self.pane.task.is_none()
                    && self.pane.catalog.as_ref().is_some_and(|catalog| {
                        catalog.owner.version == editbay_core::DocumentVersion::of(project)
                    })
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "revision={} wanted={revision}, error={:?}",
                    project.revision,
                    self.pane.error
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn tab_to(&mut self, label: &str) {
            for _ in 0..160 {
                self.press(egui::Key::Tab);
                self.frame(vec![]);
                if self.focus == label {
                    return;
                }
            }
            panic!(
                "Cannot reach {label:?} by Tab; last native focus {:?}",
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

        fn empty(&mut self) {
            self.settled(0);
            self.tab_to("Create empty sequence");
            self.press(egui::Key::Enter);
            self.settled(1);
        }

        fn title(&mut self) -> Uuid {
            self.tab_to("Titles and fades");
            self.press(egui::Key::Enter);
            self.frame(vec![]);
            self.tab_to("Title text");
            self.replace("Keyboard title");
            self.tab_to("Create title source");
            self.press(egui::Key::Enter);
            self.settled(2);
            self.workspace.tabs[0].editor.project().compositions.iter()
                .find(|scene| scene.nodes.iter().any(|node| matches!(&node.operation, NodeOperation::Text { text, .. } if text == "Keyboard title"))).unwrap().id
        }
    }

    #[test]
    fn craft_keyboard_empty_title_cut_track_clip_edit_undo_and_native_save_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let mut author = Author::new(directory.path());
        assert!(author.pane.selections.is_empty());
        assert!(
            author.workspace.tabs[0]
                .editor
                .project()
                .compositions
                .is_empty()
        );
        author.empty();
        let title = author.title();
        assert_eq!(author.pane.selections[&author.tab].source, Some(title));
        author.tab_to("Create cut");
        author.press(egui::Key::Enter);
        author.settled(3);
        let record = author.pane.selections[&author.tab].record.unwrap();
        let clip = editbay_core::timeline_clips(author.workspace.tabs[0].editor.project(), record)
            .unwrap()[0]
            .clone();
        assert_eq!(clip.source.composition, title);
        assert_eq!(clip.source.range, FrameRange { start: 0, end: 72 });
        author.tab_to("Add track pair");
        author.press(egui::Key::Enter);
        author.settled(4);
        author.tab_to("Target track: Video 1");
        author.press(egui::Key::End);
        let target = author.pane.selections[&author.tab].track.unwrap();
        assert_ne!(target, clip.track);
        author.tab_to("Place on track");
        author.press(egui::Key::Enter);
        author.settled(5);
        author.press(egui::Key::F8);
        author.press(egui::Key::PageUp);
        assert_eq!(author.pane.selections[&author.tab].clip, Some(clip.id));
        author.press(egui::Key::ArrowRight);
        author.settled(6);
        let saved = author.workspace.tabs[0].editor.snapshot();
        let clips = editbay_core::timeline_clips(&saved, record).unwrap();
        assert_eq!(
            clips
                .iter()
                .find(|item| item.id == clip.id)
                .unwrap()
                .range
                .start,
            1
        );
        assert_eq!(
            clips
                .iter()
                .find(|item| item.track == target)
                .unwrap()
                .range
                .start,
            0
        );
        author.workspace.history(author.tab, false).unwrap();
        assert_eq!(
            editbay_core::timeline_clips(author.workspace.tabs[0].editor.project(), record)
                .unwrap()
                .iter()
                .find(|item| item.id == clip.id)
                .unwrap()
                .range
                .start,
            0
        );
        author.workspace.history(author.tab, true).unwrap();
        let saved = author.workspace.tabs[0].editor.snapshot();
        let path = directory.path().join("keyboard-authored.editbay");
        editbay_core::save_new(&saved, &path).unwrap();
        assert_eq!(*saved, editbay_core::load(&path).unwrap());
        assert!(saved.sources.is_empty());
        assert_eq!(saved.assets.len(), 1);
    }

    #[test]
    fn craft_keyboard_focused_fields_buttons_and_disabled_timeline_do_not_nudge_or_delete() {
        let directory = tempfile::tempdir().unwrap();
        let mut author = Author::new(directory.path());
        author.empty();
        author.title();
        author.tab_to("Create cut");
        author.press(egui::Key::Enter);
        author.settled(3);
        let before = author.workspace.tabs[0].editor.snapshot();
        author.tab_to("Cut name");
        author.replace("EWSIO");
        author.press(egui::Key::ArrowRight);
        author.press(egui::Key::Delete);
        author.frame(vec![]);
        assert_eq!(author.workspace.tabs[0].editor.project(), &*before);
        author.tab_to("Add track pair");
        author.press(egui::Key::ArrowRight);
        author.press(egui::Key::Delete);
        author.frame(vec![]);
        assert_eq!(author.workspace.tabs[0].editor.project(), &*before);
        assert!(author.pane.task.is_none());
        author.press(egui::Key::F8);
        let project = author.workspace.tabs[0].editor.snapshot();
        author
            .ctx
            .run_ui(
                egui::RawInput {
                    events: vec![
                        key(egui::Key::ArrowRight, egui::Modifiers::NONE),
                        key(egui::Key::Delete, egui::Modifiers::NONE),
                    ],
                    ..Default::default()
                },
                |ui| {
                    author.pane.show(
                        ui,
                        &author.workspace,
                        &mut author.preview,
                        author.tab,
                        project.clone(),
                        false,
                    )
                },
            )
            .drop_without_applying_deltas();
        assert!(author.pane.task.is_none());
        assert!(author.pane.draft.is_none());
        assert_eq!(author.workspace.tabs[0].editor.project(), &*before);
    }

    #[test]
    fn craft_keyboard_empty_creation_rejects_a_changed_or_closed_owner() {
        for close in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut author = Author::new(directory.path());
            author.settled(0);
            author.tab_to("Create empty sequence");
            author.press(egui::Key::Enter);
            assert!(author.pane.task.is_some());
            if close {
                author.workspace.close(author.tab, true).unwrap();
            } else {
                let version =
                    editbay_core::DocumentVersion::of(author.workspace.tabs[0].editor.project());
                author
                    .workspace
                    .apply(
                        author.tab,
                        version,
                        "Concurrent rename".into(),
                        &[DocumentCommand::RenameProject {
                            name: "Newer work".into(),
                        }],
                    )
                    .unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while author.pane.task.is_some() {
                author.pane.poll(
                    &mut author.workspace,
                    &mut author.preview,
                    true,
                    &author.ctx,
                );
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                author
                    .pane
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("changed"))
            );
            assert!(
                author.workspace.tabs.iter().all(|tab| tab
                    .editor
                    .project()
                    .compositions
                    .is_empty())
            );
            if !close {
                assert_eq!(author.workspace.tabs[0].editor.project().name, "Newer work");
            }
        }
    }

    #[test]
    fn craft_keyboard_existing_marks_append_insert_split_and_delete_author_without_pointer_setup() {
        let directory = tempfile::tempdir().unwrap();
        let mut author = Author::new(directory.path());
        author.empty();
        let source = author.title();
        author.tab_to("View source");
        author.press(egui::Key::Enter);
        author.press(egui::Key::F8);
        author.press(egui::Key::I);
        author.press(egui::Key::O);
        assert_eq!(
            author.pane.selections[&author.tab].source_range,
            FrameRange { start: 0, end: 1 }
        );
        author.tab_to("Out exclusive");
        author.replace("12");
        author.press(egui::Key::Enter);
        assert_eq!(author.pane.selections[&author.tab].source_range.end, 12);
        author.press(egui::Key::F8);
        author.press(egui::Key::E);
        author.settled(3);
        let record = author.pane.selections[&author.tab].record.unwrap();
        author.press(egui::Key::F8);
        author.press(egui::Key::E);
        author.settled(4);
        author.press(egui::Key::F8);
        author.press(egui::Key::W);
        author.settled(5);
        let clips = editbay_core::timeline_clips(author.workspace.tabs[0].editor.project(), record)
            .unwrap();
        assert_eq!(clips.len(), 3);
        assert!(clips.iter().all(|clip| clip.source
            == SourceSelection {
                composition: source,
                range: FrameRange { start: 0, end: 12 }
            }));
        assert_eq!(
            clips
                .iter()
                .map(|clip| clip.range.start)
                .collect::<Vec<_>>(),
            vec![0, 12, 24]
        );
        author.press(egui::Key::F8);
        author.press(egui::Key::PageUp);
        author.tab_to("Record frame");
        author.replace("6");
        author.press(egui::Key::Enter);
        author.press(egui::Key::F8);
        author.press(egui::Key::S);
        author.settled(6);
        let split = author.workspace.tabs[0].editor.snapshot();
        assert_eq!(
            editbay_core::timeline_clips(&split, record).unwrap().len(),
            4
        );
        author.press(egui::Key::F8);
        author.press(egui::Key::Delete);
        author.settled(7);
        let remaining =
            editbay_core::timeline_clips(author.workspace.tabs[0].editor.project(), record)
                .unwrap();
        assert_eq!(remaining.len(), 3);
        assert_eq!(remaining.iter().map(|clip| clip.range.end).max(), Some(30));
        author.workspace.history(author.tab, false).unwrap();
        assert_eq!(
            author.workspace.tabs[0].editor.project().compositions,
            split.compositions
        );
    }

    #[test]
    fn craft_keyboard_shift_clip_selection_edits_actual_mixed_property_in_one_undo_group() {
        let directory = tempfile::tempdir().unwrap();
        let mut author = Author::new(directory.path());
        author.empty();
        author.title();
        author.tab_to("Create cut");
        author.press(egui::Key::Enter);
        author.settled(3);
        author.press(egui::Key::F8);
        author.press(egui::Key::E);
        author.settled(4);
        author.tab_to("Opacity");
        author.replace("0.6");
        author.press(egui::Key::Enter);
        author.settled(5);
        let record = author.pane.selections[&author.tab].record.unwrap();
        let before = author.workspace.tabs[0].editor.snapshot();
        let originals = editbay_core::timeline_clips(&before, record).unwrap();
        assert_ne!(originals[0].controls.opacity, originals[1].controls.opacity);
        author.press(egui::Key::F8);
        author.press(egui::Key::PageUp);
        author.frame(vec![key(egui::Key::PageDown, egui::Modifiers::SHIFT)]);
        assert_eq!(author.pane.selections[&author.tab].multiple.len(), 2);
        author.tab_to("Opacity");
        author.replace("0.4");
        author.press(egui::Key::Enter);
        author.settled(6);
        let edited = author.workspace.tabs[0].editor.snapshot();
        let clips = editbay_core::timeline_clips(&edited, record).unwrap();
        for (clip, original) in clips.iter().zip(&originals) {
            assert_eq!(
                (clip.id, clip.track, clip.range, clip.source),
                (original.id, original.track, original.range, original.source)
            );
            assert_eq!(clip.controls.opacity, 0.4);
            assert_eq!(clip.controls.gain, original.controls.gain);
            assert_eq!(clip.controls.translation, original.controls.translation);
        }
        let path = directory.path().join("mixed-keyboard.editbay");
        editbay_core::save_new(&edited, &path).unwrap();
        assert_eq!(editbay_core::load(&path).unwrap(), *edited);
        author.workspace.history(author.tab, false).unwrap();
        assert_eq!(
            author.workspace.tabs[0].editor.project().compositions,
            before.compositions
        );
    }
}
