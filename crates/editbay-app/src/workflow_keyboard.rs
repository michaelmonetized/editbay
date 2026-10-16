use super::*;
use crate::{color_ui::ScopePanel, delivery_ui::DeliveryPane, multicam_ui::MulticamPane};
use editbay_core::{Composition, FrameRange, NodeOperation, Sequence, TimedNode};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(crate) enum Page {
    Workflow,
    Multicam,
    Scopes,
    Delivery,
}

pub(crate) struct Author {
    pub(crate) ctx: egui::Context,
    pub(crate) workspace: Workspace,
    pub(crate) preview: PreviewPane,
    pub(crate) workflow: WorkflowPane,
    pub(crate) multicam: MulticamPane,
    pub(crate) scopes: ScopePanel,
    pub(crate) delivery: DeliveryPane,
    pub(crate) delivered: Option<editbay_delivery::DeliveryRequest>,
    pub(crate) tab: Uuid,
    pub(crate) page: Page,
    pub(crate) focus: String,
    nodes: HashMap<egui::accesskit::NodeId, egui::accesskit::Node>,
    pub(crate) enabled: bool,
    pub(crate) poll_workers: bool,
    preceding_row: bool,
}

pub(crate) fn directory() -> tempfile::TempDir {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/keyboard-fields");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("native-")
        .tempdir_in(root)
        .unwrap()
}

pub(crate) fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }
}

impl Author {
    pub(crate) fn new(root: &std::path::Path, page: Page) -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut workspace = Workspace::new(root.join("recovery"), ctx.clone());
        let tab = workspace
            .create(Project::new("Native field routes").unwrap())
            .unwrap();
        Self {
            ctx,
            workspace,
            preview: PreviewPane::default(),
            workflow: WorkflowPane::new(root.join("loss")),
            multicam: MulticamPane::default(),
            scopes: ScopePanel::default(),
            delivery: DeliveryPane::default(),
            delivered: None,
            tab,
            page,
            focus: String::new(),
            nodes: HashMap::new(),
            enabled: true,
            poll_workers: true,
            preceding_row: false,
        }
    }

    pub(crate) fn camera(&mut self, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        let picture = Uuid::new_v4();
        let rate = editbay_core::FrameRate::new(24, 1).unwrap();
        let first = self.project().compositions.is_empty();
        let sequence = if first {
            self.project().sequences[0].id
        } else {
            Uuid::new_v4()
        };
        let mut commands = vec![
            DocumentCommand::SetComposition {
                composition: Composition {
                    id,
                    name: name.into(),
                    width: 64,
                    height: 36,
                    frame_rate: rate,
                    duration: 240,
                    tracks: vec![],
                    nodes: vec![TimedNode {
                        id: picture,
                        range: FrameRange { start: 0, end: 240 },
                        operation: NodeOperation::Solid {
                            rgba: [0.2, 0.3, 0.4, 1.0],
                        },
                        animation: vec![],
                    }],
                    picture: Some(picture),
                    audio: None,
                },
            },
            DocumentCommand::SetSequence {
                sequence: Sequence {
                    id: sequence,
                    name: name.into(),
                    width: 64,
                    height: 36,
                    frame_rate: rate,
                    composition: Some(id),
                },
            },
        ];
        if first {
            commands.push(DocumentCommand::SetPrimarySequence { id: sequence });
        }
        self.workspace
            .apply(
                self.tab,
                DocumentVersion::of(self.project()),
                "Available procedural camera".into(),
                &commands,
            )
            .unwrap();
        id
    }

    pub(crate) fn project(&self) -> &Project {
        self.workspace
            .tabs
            .iter()
            .find(|tab| tab.id == self.tab)
            .unwrap()
            .editor
            .project()
    }

    pub(crate) fn frame(&mut self, events: Vec<egui::Event>) {
        if self.poll_workers {
            self.workflow.poll(&mut self.workspace, true);
            self.multicam.poll(&mut self.workspace, true);
        }
        let project = self
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == self.tab)
            .unwrap()
            .editor
            .snapshot();
        let output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1800., 6000.),
                )),
                events: keyboard_ui::with_native_modifiers(events),
                ..Default::default()
            },
            |ui| {
                if self.preceding_row {
                    ui.label("Conditional document status");
                }
                ui.add_enabled_ui(self.enabled, |ui| match self.page {
                    Page::Workflow => self.workflow.show(
                        ui,
                        &mut self.workspace,
                        &mut self.preview,
                        self.tab,
                        project.clone(),
                    ),
                    Page::Multicam => self.multicam.show(
                        ui,
                        &mut self.workspace,
                        &mut self.preview,
                        self.tab,
                        project.clone(),
                    ),
                    Page::Scopes => {
                        self.scopes.show(ui, None);
                    }
                    Page::Delivery => {
                        if let Some((_, request, _)) =
                            self.delivery
                                .show_choice(ui.ctx(), &self.workspace, &mut self.preview)
                        {
                            self.delivered = Some(request);
                        }
                    }
                });
            },
        );
        for event in &output.platform_output.events {
            if let egui::output::OutputEvent::FocusGained(info) = event {
                self.focus = info.label.clone().unwrap_or_default();
            }
        }
        if let Some(update) = &output.platform_output.accesskit_update {
            self.nodes.extend(update.nodes.iter().cloned());
        }
        if self.ctx.memory(|memory| memory.focused().is_none()) {
            self.focus.clear();
        }
        output.drop_without_applying_deltas();
    }

    pub(crate) fn press(&mut self, key: egui::Key) {
        self.frame(vec![self::key(key, egui::Modifiers::NONE)]);
        self.frame(vec![]);
    }

    pub(crate) fn tab_to(&mut self, label: &str) {
        for _ in 0..200 {
            self.press(egui::Key::Tab);
            if self.focus == label && self.ctx.memory(|memory| memory.focused().is_some()) {
                return;
            }
        }
        panic!(
            "Cannot reach {label:?} using Tab; last native focus {:?}",
            self.focus
        );
    }

    pub(crate) fn tab_prefix(&mut self, label: &str) {
        for _ in 0..200 {
            self.press(egui::Key::Tab);
            if self.focus.starts_with(label) && self.ctx.memory(|memory| memory.focused().is_some())
            {
                return;
            }
        }
        panic!(
            "Cannot reach {label:?} using Tab; last native focus {:?}",
            self.focus
        );
    }

    pub(crate) fn replace(&mut self, text: &str) {
        let mut events = vec![key(
            egui::Key::A,
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        )];
        events.push(if text.is_empty() {
            key(egui::Key::Backspace, egui::Modifiers::NONE)
        } else {
            egui::Event::Text(text.into())
        });
        self.frame(events);
    }

    pub(crate) fn open(&mut self, label: &str) {
        self.tab_to(label);
        self.press(egui::Key::Enter);
    }

    pub(crate) fn settled(&mut self, revision: u64) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.frame(vec![]);
            if self.project().revision == revision {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "wanted revision {revision}, actual {}, workflow={:?}, multicam={:?}, native_focus={:?}, native_name={:?}, policies={:?}",
                self.project().revision,
                self.workflow.status,
                self.multicam.keyboard_worker_state(),
                self.ctx.memory(|memory| memory.focused()),
                self.focus,
                self.project()
                    .multicam
                    .iter()
                    .map(|session| (session.id, session.audio_policy))
                    .collect::<Vec<_>>(),
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub(crate) fn invalid(&self, id: egui::Id) {
        let field = &self.nodes[&egui::accesskit::NodeId(id.value())];
        assert_eq!(field.invalid(), Some(egui::accesskit::Invalid::True));
        let error = field
            .error_message()
            .expect("actual field must identify its visible error");
        assert!(self.nodes.contains_key(&error));
    }

    pub(crate) fn disabled(&self, label: &str) -> bool {
        self.nodes
            .values()
            .find(|node| node.label() == Some(label))
            .expect("native control must be rendered")
            .is_disabled()
    }

    pub(crate) fn saved(&self, root: &std::path::Path, name: &str) {
        let path = root.join(name);
        editbay_core::save_new(self.project(), &path).unwrap();
        assert_eq!(*self.project(), editbay_core::load(&path).unwrap());
    }
}

/// Retain one real camera sound source for the native policy fixture.
/// Args: author, existing camera and new WAV path. Returns the ingested source identity.
fn retain_camera_sound(author: &mut Author, camera: Uuid, path: &std::path::Path) -> Uuid {
    use editbay_core::{Clip, ClipSource, StreamFormat, TimeMap, TimePoint, Track, TrackKind};
    use editbay_media::{Cancellation, SourceFile, StreamType};
    let mut scene = author
        .project()
        .compositions
        .iter()
        .find(|scene| scene.id == camera)
        .unwrap()
        .clone();
    let sample_rate = 48_000_u32;
    let samples = u128::from(scene.duration)
        * u128::from(scene.frame_rate.denominator)
        * u128::from(sample_rate);
    assert_eq!(samples % u128::from(scene.frame_rate.numerator), 0);
    let samples = u32::try_from(samples / u128::from(scene.frame_rate.numerator)).unwrap();
    let sample_bytes = samples.checked_mul(2).unwrap();
    let mut wav = Vec::with_capacity(sample_bytes as usize + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(sample_bytes + 36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&sample_bytes.to_le_bytes());
    for sample in 0..samples {
        let value = if sample % 2 == 0 { 8192_i16 } else { -8192_i16 };
        wav.extend_from_slice(&value.to_le_bytes());
    }
    editbay_core::write_new_bytes(path, &wav).unwrap();
    let cancel = Cancellation::new().unwrap();
    let file = SourceFile::open(path, &cancel).unwrap();
    let probe = file.probe(cancel.clone()).unwrap();
    let stream = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Audio)
        .unwrap()
        .index;
    let ingested = file
        .ingest(
            "Camera A recorded sound".into(),
            &[stream],
            cancel,
            |_, _| {},
        )
        .unwrap();
    let retained = ingested
        .source
        .streams
        .iter()
        .find(|retained| retained.index == stream)
        .unwrap();
    assert!(matches!(
        &retained.format,
        StreamFormat::Audio {
            sample_rate: 48_000,
            ..
        }
    ));
    assert_eq!(
        retained.time_base,
        editbay_core::TimeBase {
            numerator: 1,
            denominator: sample_rate
        }
    );
    assert_eq!(retained.duration_ticks, Some(u64::from(samples)));
    let source = ingested.source.id;
    let clip = Uuid::new_v4();
    let node = Uuid::new_v4();
    let range = FrameRange {
        start: 0,
        end: scene.duration,
    };
    let end_tick = retained
        .start_tick
        .checked_add(i64::try_from(retained.duration_ticks.unwrap()).unwrap())
        .unwrap();
    scene.tracks.push(Track {
        id: Uuid::new_v4(),
        name: "Original camera sound".into(),
        kind: TrackKind::Audio,
        enabled: true,
        clips: vec![Clip {
            id: clip,
            name: "Camera A recorded sound".into(),
            range,
            source: ClipSource::Media { source, stream },
            linked: None,
            time_map: TimeMap {
                source_denominator: 1,
                points: vec![
                    TimePoint {
                        frame: 0,
                        source_tick: retained.start_tick,
                    },
                    TimePoint {
                        frame: scene.duration,
                        source_tick: end_tick,
                    },
                ],
            },
        }],
    });
    scene.nodes.push(TimedNode {
        id: node,
        range,
        operation: NodeOperation::Source { clip },
        animation: vec![],
    });
    scene.audio = Some(node);
    let mut commands = ingested.commands().to_vec();
    commands.push(DocumentCommand::SetComposition { composition: scene });
    author
        .workspace
        .apply(
            author.tab,
            DocumentVersion::of(author.project()),
            "Retain actual camera sound".into(),
            &commands,
        )
        .unwrap();
    source
}

#[test]
fn craft_keyboard_common_captions_create_invalid_recover_retime_undo_native_reopen() {
    let directory = directory();
    let mut author = Author::new(directory.path(), Page::Workflow);
    let composition = author.camera("Program");
    author.frame(vec![]);
    author.open("Captions");
    author.open("New caption track");
    author.settled(2);
    assert_eq!(author.project().captions.len(), 1);
    assert!(author.project().captions[0].cues.is_empty());
    author.tab_to("Cue start (ms)");
    author.replace("-");
    let malformed = author.ctx.memory(|memory| memory.focused()).unwrap();
    author.invalid(malformed);
    author.press(egui::Key::Tab);
    author.press(egui::Key::Tab);
    assert_ne!(author.focus, "Add cue");
    assert!(author.project().captions[0].cues.is_empty());
    author.tab_to("Cue start (ms)");
    author.replace("100");
    author.press(egui::Key::Enter);
    author.open("Add cue");
    author.settled(3);
    let track = author.project().captions[0].id;
    let cue = author.project().captions[0].cues[0].id;
    assert_eq!(author.project().captions[0].composition, composition);
    assert_eq!(
        author.project().captions[0].cues[0].start,
        CaptionTime::from_milliseconds(100)
    );
    author.tab_to("Cue 1 text");
    let text_owner = author.ctx.memory(|memory| memory.focused()).unwrap();
    author.preceding_row = true;
    author.replace("");
    assert_eq!(
        author.ctx.memory(|memory| memory.focused()),
        Some(text_owner)
    );
    author.invalid(text_owner);
    author.press(egui::Key::Tab);
    assert_ne!(author.focus, "Apply text");
    assert_eq!(author.project().captions[0].cues[0].text, "Caption");
    author.tab_to("Cue 1 text");
    author.replace("First line\nSecond line");
    author.press(egui::Key::Tab);
    author.settled(4);
    assert_eq!(
        author.project().captions[0].cues[0].text,
        "First line\nSecond line"
    );
    author.tab_to("Cue end (ms)");
    author.replace("1500");
    author.press(egui::Key::Enter);
    author.open("Retime to fields");
    author.settled(5);
    let retained = author.project().captions[0].clone();
    assert_eq!(retained.id, track);
    assert_eq!(retained.cues[0].id, cue);
    assert_eq!(retained.cues[0].end, CaptionTime::from_milliseconds(1500));
    author.workspace.history(author.tab, false).unwrap();
    assert_eq!(
        author.project().captions[0].cues[0].end,
        CaptionTime::from_milliseconds(1000)
    );
    author.workspace.history(author.tab, true).unwrap();
    assert_eq!(author.project().captions[0], retained);
    author.saved(directory.path(), "caption-native.editbay");
}

#[test]
fn craft_keyboard_common_edl_conditional_rows_keep_native_id_and_reject_invalid_rate() {
    let directory = directory();
    let mut author = Author::new(directory.path(), Page::Workflow);
    author.frame(vec![]);
    author.open("Editorial exchange");
    author.tab_to("Source or new destination path");
    author.replace("/home/unused-keyboard-source.edl");
    author.tab_prefix("Editorial format:");
    author.press(egui::Key::Home);
    assert_eq!(author.workflow.format, editbay_interchange::Format::Edl);
    author.tab_to("EDL rate denominator");
    author.replace("-");
    author.press(egui::Key::Tab);
    assert_ne!(author.focus, "Import timeline");
    assert!(author.workflow.task.is_none());
    author.tab_to("EDL rate denominator");
    author.replace("1001");
    author.press(egui::Key::Enter);
    author.tab_to("Import timeline");
    let action = author.ctx.memory(|memory| memory.focused()).unwrap();
    author.tab_prefix("Editorial format:");
    author.press(egui::Key::End);
    author.tab_to("Import timeline");
    assert_eq!(author.ctx.memory(|memory| memory.focused()), Some(action));
    author.preceding_row = true;
    author.frame(vec![]);
    assert_eq!(author.ctx.memory(|memory| memory.focused()), Some(action));
    author.enabled = false;
    author.press(egui::Key::Enter);
    assert!(author.workflow.task.is_none());
    assert_eq!(author.project().revision, 0);
}

#[test]
fn craft_keyboard_common_caption_style_invalid_draft_blocks_font_and_recovers_accepted_edit() {
    let directory = directory();
    let mut author = Author::new(directory.path(), Page::Workflow);
    author.camera("Program");
    author.frame(vec![]);
    author.open("Captions");
    author.open("New caption track");
    author.settled(2);
    author.tab_to("Caption font file");
    author.replace(&directory.path().join("font.ttf").to_string_lossy());
    author.tab_to("Caption font size");
    author.replace("-");
    let field = author.ctx.memory(|memory| memory.focused()).unwrap();
    author.invalid(field);
    for _ in 0..3 {
        author.press(egui::Key::Tab);
        assert_ne!(author.focus, "Use font for burn-in");
    }
    assert!(author.workflow.task.is_none());
    assert_eq!(author.project().captions[0].style.font_size, 48.0);
    assert_eq!(author.project().revision, 2);
    author.tab_to("Caption font size");
    author.replace("72");
    author.press(egui::Key::Enter);
    author.settled(3);
    author.tab_to("Use font for burn-in");
    assert_eq!(author.project().captions[0].style.font_size, 72.0);
    assert!(
        author.workflow.task.is_none(),
        "recovery reaches the real action without reading a font"
    );
    author.workspace.history(author.tab, false).unwrap();
    assert_eq!(author.project().captions[0].style.font_size, 48.0);
    author.workspace.history(author.tab, true).unwrap();
    author.saved(directory.path(), "caption-style-native.editbay");
}

#[test]
fn craft_keyboard_common_scopes_disabled_invalid_recovery_and_native_owner() {
    let directory = directory();
    let mut author = Author::new(directory.path(), Page::Scopes);
    author.frame(vec![]);
    for _ in 0..6 {
        author.press(egui::Key::Tab);
        assert!(!author.focus.starts_with("Levels:"));
        assert!(!author.focus.starts_with("Sample limit:"));
    }
    author.open("Live scopes");
    assert!(author.scopes.enabled);
    author.tab_prefix("Levels:");
    author.press(egui::Key::End);
    assert_eq!(
        author.scopes.settings.scale,
        editbay_render::scopes::ScopeScale::Nits
    );
    author.tab_prefix("Sample limit:");
    author.press(egui::Key::Home);
    assert_eq!(author.scopes.settings.maximum_samples, 16_384);
    author.open("Luminance settings");
    author.tab_to("Reference white");
    let native = author.ctx.memory(|memory| memory.focused()).unwrap();
    author.preceding_row = true;
    author.replace("-");
    assert_eq!(author.ctx.memory(|memory| memory.focused()), Some(native));
    author.invalid(native);
    assert_eq!(author.scopes.settings.reference_white_nits, 100.0);
    author.replace("203");
    author.press(egui::Key::Enter);
    assert_eq!(author.scopes.settings.reference_white_nits, 203.0);
    author.tab_to("Peak");
    author.replace("1000");
    author.press(egui::Key::Enter);
    assert_eq!(author.scopes.settings.peak_nits, 1000.0);
    assert_eq!(
        author.project().revision,
        0,
        "scope settings are viewer-owned"
    );
}

#[test]
fn craft_keyboard_common_caption_mixed_safe_area_changes_only_after_accepted_native_edit() {
    let directory = directory();
    let mut author = Author::new(directory.path(), Page::Workflow);
    author.camera("Program");
    author.frame(vec![]);
    author.open("Captions");
    author.open("New caption track");
    author.settled(2);
    let mut track = author.project().captions[0].clone();
    track.style.safe_area = CaptionSafeArea {
        left: 0.02,
        right: 0.03,
        top: 0.04,
        bottom: 0.05,
    };
    let retained = track.clone();
    author
        .workspace
        .apply(
            author.tab,
            DocumentVersion::of(author.project()),
            "Retain independent caption insets".into(),
            &[DocumentCommand::SetCaptionTrack { track }],
        )
        .unwrap();
    author.tab_to("Caption safe margin");
    let native = author.ctx.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        author.nodes[&egui::accesskit::NodeId(native.value())].state_description(),
        Some("Mixed")
    );
    author.press(egui::Key::Tab);
    assert_eq!(author.project().captions[0], retained);
    author.tab_to("Caption safe margin");
    author.replace("-");
    author.invalid(native);
    assert_eq!(author.project().captions[0], retained);
    author.replace("0.1");
    author.press(egui::Key::Enter);
    author.settled(4);
    let area = author.project().captions[0].style.safe_area;
    assert_eq!([area.left, area.right, area.top, area.bottom], [0.1; 4]);
    assert_eq!(author.project().captions[0].id, retained.id);
    author.workspace.history(author.tab, false).unwrap();
    assert_eq!(author.project().captions[0], retained);
    author.workspace.history(author.tab, true).unwrap();
    author.saved(directory.path(), "caption-mixed-native.editbay");
}

#[test]
fn craft_keyboard_common_multicam_choose_cameras_create_policy_sync_undo_native_reopen() {
    let directory = directory();
    let mut author = Author::new(directory.path(), Page::Multicam);
    let a = author.camera("Camera A");
    let b = author.camera("Camera B");
    let sound = retain_camera_sound(&mut author, a, &directory.path().join("camera-a.wav"));
    assert_eq!(author.project().revision, 3);
    assert!(
        author
            .project()
            .compositions
            .iter()
            .find(|scene| scene.id == a)
            .unwrap()
            .audio
            .is_some()
    );
    assert!(
        author
            .project()
            .compositions
            .iter()
            .find(|scene| scene.id == b)
            .unwrap()
            .audio
            .is_none()
    );
    let original_sources = author.project().sources.clone();
    let original_assets = author.project().assets.clone();
    assert_eq!(original_sources[0].id, sound);
    author.frame(vec![]);
    author.open("Create from camera sequences");
    author.tab_prefix("Camera source: Camera A");
    author.press(egui::Key::Space);
    author.tab_prefix("Camera source: Camera B");
    author.press(egui::Key::Space);
    author.tab_to("Multicam name");
    author.replace("");
    let name = author.ctx.memory(|memory| memory.focused()).unwrap();
    author.invalid(name);
    author.press(egui::Key::Tab);
    assert_ne!(author.focus, "Create multicam edit");
    assert!(author.project().multicam.is_empty());
    author.tab_to("Multicam name");
    author.replace("Keyboard cameras");
    author.open("Create multicam edit");
    author.settled(4);
    let original_cameras: Vec<_> = author
        .project()
        .compositions
        .iter()
        .filter(|scene| scene.id == a || scene.id == b)
        .cloned()
        .collect();
    let session = author.project().multicam[0].clone();
    assert_eq!(session.name, "Keyboard cameras");
    assert_eq!(
        session.audio_policy,
        editbay_core::multicam::MulticamAudioPolicy::Fixed {
            angle: session.angles[0].id
        }
    );
    assert_eq!(
        session
            .angles
            .iter()
            .map(|angle| angle.composition)
            .collect::<Vec<_>>(),
        vec![a, b]
    );
    author.tab_prefix("2 · Camera B");
    author.press(egui::Key::Enter);
    author.settled(5);
    assert_eq!(author.project().multicam[0].cuts[0].frame, 0);
    assert_eq!(
        author.project().multicam[0].cuts[0].angle,
        session.angles[1].id
    );
    author.tab_prefix("Multicam sound:");
    assert_eq!(author.focus, "Multicam sound: Keep Camera A sound");
    let policy_id = author.ctx.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        author.nodes[&egui::accesskit::NodeId(policy_id.value())].label(),
        Some("Multicam sound: Keep Camera A sound")
    );
    assert!(!egui::ComboBox::is_open(&author.ctx, policy_id));
    let status = author.multicam.keyboard_worker_state().2.map(str::to_owned);
    author.press(egui::Key::End);
    assert_eq!(
        author.ctx.memory(|memory| memory.focused()),
        Some(policy_id)
    );
    assert_eq!(
        author.project().revision,
        5,
        "silent Camera B is not a fixed-sound target"
    );
    assert!(author.multicam.keyboard_worker_state().1.is_none());
    assert_eq!(author.multicam.keyboard_worker_state().2, status.as_deref());
    let (owner, _) = author.workspace.edit_snapshot(author.tab).unwrap();
    author.frame(vec![key(egui::Key::Home, egui::Modifiers::NONE)]);
    assert_eq!(
        author.ctx.memory(|memory| memory.focused()),
        Some(policy_id)
    );
    assert_eq!(author.multicam.keyboard_worker_state().0, Some(session.id));
    assert_eq!(author.multicam.keyboard_worker_state().1, Some(owner));
    assert_eq!(
        author.project().revision,
        5,
        "actual policy worker has not yet published"
    );
    author.settled(6);
    assert!(author.multicam.keyboard_worker_state().1.is_none());
    assert_eq!(
        author.multicam.keyboard_worker_state().2,
        Some("Change multicam sound policy")
    );
    assert_eq!(
        author.project().multicam[0].audio_policy,
        editbay_core::multicam::MulticamAudioPolicy::All
    );
    author.open("Synchronize cameras");
    author.tab_to("Camera A start timecode");
    author.replace("00:00:00:99");
    let clock = author.ctx.memory(|memory| memory.focused()).unwrap();
    author.invalid(clock);
    author.tab_to("Camera B start timecode");
    author.press(egui::Key::Tab);
    assert_eq!(author.focus, "Sync marked event");
    author.press(egui::Key::Tab);
    assert!(author.focus.starts_with("Reference camera:"));
    author.tab_to("Camera A start timecode");
    author.replace("00:00:00:00");
    author.tab_to("Camera A marker frame or fraction");
    author.replace("1/0");
    let marker = author.ctx.memory(|memory| memory.focused()).unwrap();
    author.invalid(marker);
    author.tab_to("Camera B start timecode");
    author.press(egui::Key::Tab);
    assert_ne!(author.focus, "Sync marked event");
    assert_eq!(author.project().revision, 6);
    author.tab_to("Camera A marker frame or fraction");
    author.replace("1/2");
    author.open("Sync marked event");
    author.settled(7);
    let synced = author.project().multicam[0].clone();
    assert_eq!(synced.id, session.id);
    assert_eq!(synced.angles[0].id, session.angles[0].id);
    assert_eq!(synced.angles[0].offset, session.angles[0].offset);
    assert_ne!(synced.angles[1].offset, session.angles[1].offset);
    author.tab_to("Analyze first");
    author.replace("-");
    let audio = author.ctx.memory(|memory| memory.focused()).unwrap();
    author.invalid(audio);
    author.press(egui::Key::Tab);
    author.press(egui::Key::Tab);
    assert_ne!(author.focus, "Sync decoded audio");
    assert_eq!(author.project().revision, 7);
    author.tab_to("Analyze first");
    author.replace("10");
    author.press(egui::Key::Enter);
    author.tab_to("Maximum offset");
    author.replace("3");
    author.press(egui::Key::Enter);
    author.press(egui::Key::Tab);
    assert_ne!(
        author.focus, "Sync decoded audio",
        "Camera B has no recorded audio"
    );
    assert_eq!(
        author
            .project()
            .compositions
            .iter()
            .filter(|scene| scene.id == a || scene.id == b)
            .cloned()
            .collect::<Vec<_>>(),
        original_cameras
    );
    author.workspace.history(author.tab, false).unwrap();
    assert_eq!(author.project().multicam[0].angles, session.angles);
    author.workspace.history(author.tab, true).unwrap();
    assert_eq!(author.project().multicam[0], synced);
    assert_eq!(author.project().sources, original_sources);
    assert_eq!(author.project().assets, original_assets);
    author.saved(directory.path(), "multicam-native.editbay");
}

#[test]
fn craft_keyboard_common_owned_number_arrows_preserve_margin_white_peak_and_font_native_steps() {
    let directory = directory();
    let mut author = Author::new(directory.path(), Page::Workflow);
    author.camera("Program");
    author.frame(vec![]);
    author.open("Captions");
    author.open("New caption track");
    author.settled(2);
    author.tab_to("Caption safe margin");
    author.replace("0");
    author.press(egui::Key::Enter);
    author.settled(3);
    author.tab_to("Caption safe margin");
    let margin = author.ctx.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        author.nodes[&egui::accesskit::NodeId(margin.value())].numeric_value_step(),
        Some(0.01)
    );
    author.press(egui::Key::ArrowUp);
    author.settled(4);
    let area = author.project().captions[0].style.safe_area;
    assert_eq!(
        [area.left, area.top, area.right, area.bottom],
        [0.01_f32; 4]
    );
    assert_eq!(author.ctx.memory(|memory| memory.focused()), Some(margin));
    assert_eq!(
        author.nodes[&egui::accesskit::NodeId(margin.value())].numeric_value(),
        Some(f64::from(0.01_f32))
    );
    author.tab_to("Caption font size");
    author.press(egui::Key::ArrowUp);
    author.settled(5);
    assert_eq!(author.project().captions[0].style.font_size, 49.0_f32);

    let mut scopes = Author::new(directory.path(), Page::Scopes);
    scopes.frame(vec![]);
    scopes.open("Live scopes");
    scopes.open("Luminance settings");
    scopes.tab_to("Reference white");
    let white = scopes.ctx.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        scopes.nodes[&egui::accesskit::NodeId(white.value())].numeric_value_step(),
        Some(1.0)
    );
    scopes.press(egui::Key::ArrowUp);
    assert_eq!(scopes.scopes.settings.reference_white_nits, 101.0);
    assert_eq!(scopes.ctx.memory(|memory| memory.focused()), Some(white));
    scopes.tab_to("Peak");
    scopes.replace("1000");
    scopes.press(egui::Key::Enter);
    scopes.tab_to("Peak");
    let peak = scopes.ctx.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        scopes.nodes[&egui::accesskit::NodeId(peak.value())].numeric_value_step(),
        Some(10.0)
    );
    scopes.press(egui::Key::ArrowUp);
    assert_eq!(scopes.scopes.settings.peak_nits, 1010.0);
    assert_eq!(scopes.ctx.memory(|memory| memory.focused()), Some(peak));
    assert_eq!(
        scopes.nodes[&egui::accesskit::NodeId(peak.value())].numeric_value(),
        Some(1010.0)
    );
}

#[test]
fn craft_keyboard_common_loaded_fractions_render_focus_blur_preserve_document_and_numeric_metadata()
{
    let directory = directory();
    let mut author = Author::new(directory.path(), Page::Workflow);
    author.camera("Program");
    author.frame(vec![]);
    author.open("Captions");
    author.open("New caption track");
    author.settled(2);
    let mut loaded = author.project().captions[0].clone();
    loaded.style.font_size = 72.12346_f32;
    let margin = 0.031234568_f32;
    loaded.style.safe_area = CaptionSafeArea {
        left: margin,
        right: margin,
        top: margin,
        bottom: margin,
    };
    author
        .workspace
        .apply(
            author.tab,
            DocumentVersion::of(author.project()),
            "Load valid fractional caption style".into(),
            &[DocumentCommand::SetCaptionTrack {
                track: loaded.clone(),
            }],
        )
        .unwrap();
    for _ in 0..8 {
        author.frame(vec![]);
    }
    author.tab_to("Caption font size");
    let font_id = author.ctx.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        author.nodes[&egui::accesskit::NodeId(font_id.value())].numeric_value(),
        Some(f64::from(loaded.style.font_size))
    );
    author.tab_to("Caption safe margin");
    let margin_id = author.ctx.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        author.nodes[&egui::accesskit::NodeId(margin_id.value())].numeric_value(),
        Some(f64::from(margin))
    );
    author.tab_to("Caption font file");
    for _ in 0..8 {
        author.frame(vec![]);
    }
    assert_eq!(author.project().captions[0], loaded);
    assert_eq!(author.project().revision, 3);
    author.saved(directory.path(), "fractional-style-native.editbay");

    let mut scopes = Author::new(directory.path(), Page::Scopes);
    scopes.scopes.settings.reference_white_nits = 203.12345678912346;
    scopes.scopes.settings.peak_nits = 1000.9876543219876;
    let retained = scopes.scopes.settings;
    for _ in 0..8 {
        scopes.frame(vec![]);
    }
    scopes.open("Live scopes");
    scopes.open("Luminance settings");
    scopes.tab_to("Reference white");
    let white_id = scopes.ctx.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        scopes.nodes[&egui::accesskit::NodeId(white_id.value())].numeric_value(),
        Some(retained.reference_white_nits)
    );
    scopes.tab_to("Peak");
    let peak_id = scopes.ctx.memory(|memory| memory.focused()).unwrap();
    assert_eq!(
        scopes.nodes[&egui::accesskit::NodeId(peak_id.value())].numeric_value(),
        Some(retained.peak_nits)
    );
    scopes.tab_to("Live scopes");
    for _ in 0..8 {
        scopes.frame(vec![]);
    }
    assert_eq!(scopes.scopes.settings, retained);
    assert_eq!(scopes.project().revision, 0);
}
