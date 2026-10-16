use editbay_core::{
    AssetKind, AssetReference, Composition, DocumentCommand, DocumentEditor, DocumentVersion,
    EvaluationSnapshot, FrameRate, Project, Sequence, SourcePosition, TrackKind,
    captions::{CaptionCue, CaptionDelivery, CaptionSafeArea, CaptionTime, CaptionTrack},
    load,
    multicam::{MulticamAudioPolicy, MulticamSession, MulticamTimecode},
    save_new, sequence_from_video_with_audio,
};
use editbay_delivery::{DeliveryControl, DeliveryFormat, DeliveryRequest, Phase, Receipt, deliver};
use editbay_media::{
    Cancellation, LosslessMovProfile, LosslessMovWriter, NativeAudioReader, PictureBudget,
    SourceFile, StreamType, VideoReader,
};
use editbay_render::{
    GraphBudget, GraphRenderer,
    scopes::{ScopeIdentity, ScopeScale, ScopeSettings, sample_program_frame},
};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use uuid::Uuid;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
#[path = "../../editbay-media/tests/fixtures/icc.rs"]
mod icc_fixture;
const WIDTH: u32 = 16;
const HEIGHT: u32 = 16;
const INPUT_FRAMES: u64 = 12;
const OUTPUT_FRAMES: u64 = 10;
const SAMPLES_PER_FRAME: u64 = 2000;
const CUT_FRAME: u64 = 4;
const CAMERA_B_OFFSET: u64 = 2 * SAMPLES_PER_FRAME;
const COLORS: [[u8; 4]; 2] = [[255, 0, 0, 255], [0, 255, 0, 255]];
const DEADLINE: Duration = Duration::from_secs(30);

/// Require the measured software adapter for a native rendering claim.
/// `description` is actual graph/receipt adapter info; returns only for CPU Vulkan.
fn assert_cpu_adapter(description: &str) {
    let adapter = description.to_ascii_lowercase();
    assert!(
        adapter.contains("device_type: cpu")
            && adapter.contains("backend: vulkan")
            && (adapter.contains("llvmpipe") || adapter.contains("lavapipe")),
        "Expected CPU lavapipe, got {description}"
    );
}

struct FixtureDirectory {
    path: PathBuf,
    _temporary: Option<tempfile::TempDir>,
}

impl FixtureDirectory {
    /// Allocate test-owned storage beneath /home and retain requested evidence.
    /// Takes the optional receipt root from the environment; returns a fresh unique directory.
    fn new() -> TestResult<Self> {
        let requested = std::env::var_os("EDITBAY_WORKFLOW_RECEIPTS_DIR").map(PathBuf::from);
        let parent = requested
            .clone()
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts"));
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        assert!(
            parent.starts_with("/home"),
            "Fixture storage must remain under /home"
        );
        if requested.is_some() {
            let path = parent.join(format!("workflow-delivery-{}", Uuid::new_v4()));
            fs::create_dir(&path)?;
            eprintln!("Workflow delivery evidence: {}", path.display());
            Ok(Self {
                path,
                _temporary: None,
            })
        } else {
            let temporary = tempfile::Builder::new()
                .prefix("workflow-delivery-")
                .tempdir_in(parent)?;
            Ok(Self {
                path: temporary.path().into(),
                _temporary: Some(temporary),
            })
        }
    }
}

struct DeadlineGuard {
    started: Instant,
    expired: Arc<AtomicBool>,
    controls: Arc<Mutex<Vec<DeliveryControl>>>,
    media_tokens: Arc<Mutex<Vec<Cancellation>>>,
    done: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl DeadlineGuard {
    /// Bound this tiny qualification and cancel owned media/child work on expiry.
    /// `media` is the native input/readback token; returns a joined watchdog lifetime.
    fn new(media: Cancellation) -> TestResult<Self> {
        let expired = Arc::new(AtomicBool::new(false));
        let controls = Arc::new(Mutex::new(Vec::<DeliveryControl>::new()));
        let media_tokens = Arc::new(Mutex::new(vec![media]));
        let (done, completion) = mpsc::channel();
        let state = expired.clone();
        let jobs = controls.clone();
        let native = media_tokens.clone();
        let worker = std::thread::Builder::new()
            .name("workflow-delivery-deadline".into())
            .spawn(move || {
                if completion.recv_timeout(DEADLINE).is_err() {
                    state.store(true, Ordering::Release);
                    if let Ok(tokens) = native.lock() {
                        for token in tokens.iter() {
                            token.cancel();
                        }
                    }
                    if let Ok(jobs) = jobs.lock() {
                        for job in jobs.iter() {
                            job.cancel();
                        }
                    }
                }
            })?;
        Ok(Self {
            started: Instant::now(),
            expired,
            controls,
            media_tokens,
            done: Some(done),
            worker: Some(worker),
        })
    }

    /// Register one real delivery in the shared test deadline.
    /// Takes no arguments; returns fresh cancellation control or fails after the budget.
    fn control(&self) -> TestResult<DeliveryControl> {
        assert!(
            !self.expired.load(Ordering::Acquire),
            "Workflow delivery exceeded 30 seconds"
        );
        let control = DeliveryControl::new()?;
        self.controls.lock().unwrap().push(control.clone());
        if self.expired.load(Ordering::Acquire) {
            control.cancel();
        }
        Ok(control)
    }

    /// Register fresh native work without sharing an obsolete render generation's token.
    /// Takes no arguments; returns deadline-owned cancellation or fails after expiry.
    fn media(&self) -> TestResult<Cancellation> {
        assert!(
            !self.expired.load(Ordering::Acquire),
            "Workflow delivery exceeded 30 seconds"
        );
        let cancel = Cancellation::new()?;
        self.media_tokens.lock().unwrap().push(cancel.clone());
        if self.expired.load(Ordering::Acquire) {
            cancel.cancel();
        }
        Ok(cancel)
    }
}

impl Drop for DeadlineGuard {
    fn drop(&mut self) {
        if let Some(done) = self.done.take() {
            let _ = done.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Evaluate a sample-indexed, exactly representable independent camera signal.
/// `camera` is A=0/B=1 and `sample` is its original local index; returns mono float PCM.
fn camera_pcm(camera: usize, sample: u64) -> f32 {
    if camera == 0 {
        0.125 + (sample % 17) as f32 / 4096.0
    } else {
        -0.25 + (sample % 29) as f32 / 8192.0
    }
}

/// Write one actual tiny PNG/float-PCM MOV without touching user media.
/// `path`, `camera` and `cancel` select a new file, signal and lifetime; returns after the declared interval is closed.
fn write_camera(path: &Path, camera: usize, cancel: &Cancellation) -> TestResult<()> {
    let profile = LosslessMovProfile {
        width: WIDTH,
        height: HEIGHT,
        frame_rate: FrameRate::new(24, 1)?,
        first_frame: 0,
        frames: INPUT_FRAMES,
        sample_rate: 48_000,
        channels: vec!["FC".into()],
    };
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .read(true)
        .open(path)?;
    let mut writer = LosslessMovWriter::new(&file, profile.clone(), cancel.clone())?;
    let pixels = COLORS[camera].repeat((WIDTH * HEIGHT) as usize);
    let mut first = 0;
    for frame in 0..INPUT_FRAMES {
        writer.picture(frame, &pixels)?;
        let end = profile.samples_through(frame + 1)?;
        writer.sound(
            first,
            &(first..end)
                .map(|sample| camera_pcm(camera, sample))
                .collect::<Vec<_>>(),
        )?;
        first = end;
    }
    writer.finish()?;
    file.sync_all()?;
    Ok(())
}

/// Apply one actual bounded authoring group through the shared document editor.
/// `editor`, `label` and `commands` select the current owner; returns after validation.
fn apply(editor: &mut DocumentEditor, label: &str, commands: &[DocumentCommand]) -> TestResult<()> {
    editor.apply(
        DocumentVersion::of(editor.project()),
        label.into(),
        commands,
    )?;
    Ok(())
}

/// Ingest real camera bytes and create linked native authoring commands.
/// `editor`, `path` and `cancel` retain source ownership; returns the picture composition ID.
fn import_camera(
    editor: &mut DocumentEditor,
    path: &Path,
    cancel: &Cancellation,
) -> TestResult<Uuid> {
    let source = SourceFile::open(path, cancel)?;
    let probe = source.probe(cancel.clone())?;
    let video = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Video)
        .ok_or("camera video absent")?
        .index;
    let audio = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Audio)
        .ok_or("camera audio absent")?
        .index;
    let imported = source.ingest(
        path.file_stem().unwrap().to_string_lossy().into_owned(),
        &[video, audio],
        cancel.clone(),
        |_, _| {},
    )?;
    apply(editor, "Retain generated camera", &imported.commands())?;
    let commands = sequence_from_video_with_audio(
        editor.project(),
        imported.source.id,
        video,
        audio,
        FrameRate::new(24, 1)?,
    )?;
    let composition = commands
        .iter()
        .find_map(|command| match command {
            DocumentCommand::SetComposition { composition } if composition.picture.is_some() => {
                Some(composition.id)
            }
            _ => None,
        })
        .ok_or("authored camera picture absent")?;
    apply(editor, "Create linked camera sequence", &commands)?;
    let scene = editor
        .project()
        .compositions
        .iter()
        .find(|scene| scene.id == composition)
        .unwrap();
    assert_eq!(
        (scene.width, scene.height, scene.duration),
        (WIDTH, HEIGHT, INPUT_FRAMES)
    );
    assert!(scene.picture.is_some() && scene.audio.is_some());
    let video_clip = &scene
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .unwrap()
        .clips[0];
    let audio_clip = &scene
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Audio)
        .unwrap()
        .clips[0];
    assert_eq!(video_clip.linked, Some(audio_clip.id));
    assert_eq!(audio_clip.linked, Some(video_clip.id));
    Ok(composition)
}

/// Resolve a retained compiled output for exact Undo/Redo graph comparison.
/// `project` and `id` own the output; returns that composition or fails this qualification.
fn output(project: &Project, id: Uuid) -> &Composition {
    project
        .compositions
        .iter()
        .find(|scene| scene.id == id)
        .unwrap()
}

/// Check lossless output against independent per-frame/per-sample camera oracles.
/// The supplied master, receipt, policy and cancellation token retain actual decoded data.
/// Returns measured maxima/counts; caption checks also require a real safe-area pixel change.
fn inspect_master(
    path: &Path,
    receipt: &Receipt,
    policy: &str,
    captioned: bool,
    cancel: &Cancellation,
) -> TestResult<Value> {
    assert_cpu_adapter(&receipt.adapter);
    assert_eq!(receipt.profile.frames, OUTPUT_FRAMES);
    assert_eq!(receipt.profile.first_frame, 0);
    assert_eq!(receipt.profile.channels, ["FC"]);
    assert_eq!(receipt.profile.sample_rate, 48_000);
    assert_eq!(receipt.pixel_sha256, receipt.decoded_pixel_sha256);
    assert_eq!(receipt.pcm_sha256, receipt.decoded_pcm_sha256);
    let source = SourceFile::open(path, cancel)?;
    assert_eq!(source.fingerprint().sha256, receipt.file_sha256);
    assert_eq!(source.fingerprint().bytes, receipt.file_bytes);
    let probe = source.probe(cancel.clone())?;
    let video = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Video)
        .ok_or("master video absent")?;
    let audio = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Audio)
        .ok_or("master audio absent")?;
    let time_base = video.time_base.ok_or("master clock absent")?;
    let mut reader = VideoReader::open_stream(&source, video.index, cancel.clone())?;
    let mut frame = 0;
    let mut changed_caption_pixels = 0;
    let mut maximum_pixel_error = 0_u8;
    while let Some(decoded) = reader.next_frame()? {
        assert!(frame < OUTPUT_FRAMES, "unexpected extra frame");
        let tick = decoded
            .source_tick
            .ok_or("decoded picture timestamp absent")?;
        assert_eq!(
            i128::from(tick) * i128::from(time_base.numerator) * 24,
            i128::from(frame) * i128::from(time_base.denominator)
        );
        assert_eq!(decoded.rgba.len(), (WIDTH * HEIGHT * 4) as usize);
        let expected = COLORS[usize::from(frame >= CUT_FRAME)];
        let caption_active = captioned && (3..6).contains(&frame);
        let mut changed_this_frame = 0;
        for (index, pixel) in decoded.rgba.as_chunks::<4>().0.iter().enumerate() {
            let within_safe_area =
                (2..14).contains(&(index % 16)) && (2..14).contains(&(index / 16));
            assert_eq!(pixel[3], 255);
            let error = pixel
                .iter()
                .zip(expected)
                .map(|(actual, expected)| actual.abs_diff(expected))
                .max()
                .unwrap();
            if caption_active && within_safe_area {
                changed_this_frame += usize::from(error > 1);
            } else {
                maximum_pixel_error = maximum_pixel_error.max(error);
                assert!(
                    error <= 1,
                    "frame {frame}, pixel {index}: {pixel:?} != {expected:?}"
                );
            }
        }
        if caption_active {
            assert!(
                changed_this_frame > 0,
                "actual glyph pixels absent at frame {frame}"
            );
        }
        changed_caption_pixels += changed_this_frame;
        frame += 1;
    }
    assert_eq!(frame, OUTPUT_FRAMES);
    if captioned {
        assert!(changed_caption_pixels > 0);
    }
    let mut reader = NativeAudioReader::open_stream(&source, audio.index, cancel.clone())?;
    assert_eq!(reader.info.channels, 1);
    assert_eq!(reader.info.sample_rate, 48_000);
    let mut sample = 0_u64;
    let mut maximum_pcm_error = 0.0_f32;
    while let Some(block) = reader.next_block()? {
        assert_eq!(block.first_sample, Some(sample as i64));
        for actual in block.samples {
            assert!(
                sample < OUTPUT_FRAMES * SAMPLES_PER_FRAME,
                "unexpected PCM padding"
            );
            let a = camera_pcm(0, sample);
            let b = camera_pcm(1, sample + CAMERA_B_OFFSET);
            let expected = match policy {
                "fixed" => a,
                "follow" if sample < CUT_FRAME * SAMPLES_PER_FRAME => a,
                "follow" => b,
                "all" => a + b,
                _ => panic!("unknown expected audio policy"),
            };
            let error = (actual - expected).abs();
            maximum_pcm_error = maximum_pcm_error.max(error);
            assert!(
                actual.is_finite() && error <= 2e-6,
                "{policy} sample {sample}: {actual} != {expected}"
            );
            sample += 1;
        }
    }
    assert_eq!(sample, OUTPUT_FRAMES * SAMPLES_PER_FRAME);
    source.verify(cancel)?;
    Ok(
        json!({"pictures":frame,"mono_samples":sample,"maximum_color_error_bytes":maximum_pixel_error,"maximum_pcm_error":maximum_pcm_error,"changed_caption_pixels":changed_caption_pixels,"oracle":"A(sample)=1/8+(sample%17)/4096; B(sample)=-1/4+(sample%29)/8192; B is sampled 4000 samples later; cut is sample 8000"}),
    )
}

/// Create an owner-protected sentinel using new-file semantics.
/// `path` and bytes select test-owned output; returns after synchronized creation.
fn write_new(path: &Path, bytes: &[u8]) -> TestResult<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[test]
fn craft_video_multicam_and_captions_deliver_verified_lossless_media_on_cpu_vulkan()
-> TestResult<()> {
    let (cli, binding) = bound_cli()?;
    let directory = FixtureDirectory::new()?;
    let cancel = Cancellation::new()?;
    let deadline = DeadlineGuard::new(cancel.clone())?;
    let cli_file = SourceFile::open(&cli, &cancel)?;
    let binding_path = PathBuf::from(std::env::var_os("EDITBAY_TEST_CLI_BUILD_RECEIPT").unwrap());
    let revision = binding["revision"].as_str().unwrap();
    let mut editor = DocumentEditor::new(Project::new("Actual multicam and captions delivery")?)?;
    let mut cameras = Vec::new();
    for camera in 0..2 {
        let path = directory.path.join(format!("camera-{camera}.mov"));
        write_camera(&path, camera, &cancel)?;
        cameras.push(import_camera(&mut editor, &path, &cancel)?);
    }
    let originals = editor.project().compositions.clone();
    let mut session = MulticamSession::new(editor.project(), "Synced camera edit", &cameras)?;
    let angle_a = session.angles[0].id;
    let angle_b = session.angles[1].id;
    let rate = FrameRate::new(24, 1)?;
    session.sync_timecodes(
        editor.project(),
        &[
            (angle_a, MulticamTimecode::parse("01:00:00:02", rate)?),
            (angle_b, MulticamTimecode::parse("01:00:00:00", rate)?),
        ],
    )?;
    assert_eq!(session.angles[1].offset, SourcePosition::new(-2, 1)?);
    let clock_synced = session.clone();
    session.sync_markers(
        editor.project(),
        &[
            (angle_a, SourcePosition::new(2, 1)?),
            (angle_b, SourcePosition::new(4, 1)?),
        ],
    )?;
    assert_eq!(session, clock_synced);
    assert_eq!(session.overlap(editor.project())?.duration, OUTPUT_FRAMES);
    let program = session.composition;
    let program_sequence = Uuid::new_v4();
    apply(
        &mut editor,
        "Create synced multicam",
        &[
            DocumentCommand::SetMulticamSession {
                session: session.clone(),
            },
            DocumentCommand::SetSequence {
                sequence: Sequence {
                    id: program_sequence,
                    name: "Synced camera edit".into(),
                    width: WIDTH,
                    height: HEIGHT,
                    frame_rate: rate,
                    composition: Some(program),
                },
            },
            DocumentCommand::SetPrimarySequence {
                id: program_sequence,
            },
        ],
    )?;
    let uncut = output(editor.project(), program).clone();
    session.cut_at(editor.project(), CUT_FRAME, angle_b)?;
    apply(
        &mut editor,
        "Cut A to B",
        &[DocumentCommand::SetMulticamSession {
            session: session.clone(),
        }],
    )?;
    let cut = output(editor.project(), program).clone();
    editor.undo(DocumentVersion::of(editor.project()))?;
    assert_eq!(output(editor.project(), program), &uncut);
    editor.redo(DocumentVersion::of(editor.project()))?;
    assert_eq!(output(editor.project(), program), &cut);
    assert_eq!(editor.project().compositions[..originals.len()], originals);
    let request = DeliveryRequest {
        format: DeliveryFormat::LosslessMov,
        composition: program,
        sample_rate: 48_000,
        range: None,
    };
    let mut observations = Vec::new();
    for (name, policy) in [
        ("fixed", MulticamAudioPolicy::Fixed { angle: angle_a }),
        ("all", MulticamAudioPolicy::All),
        ("follow", MulticamAudioPolicy::FollowVideo),
    ] {
        session.audio_policy = policy;
        apply(
            &mut editor,
            "Choose multicam audio policy",
            &[DocumentCommand::SetMulticamSession {
                session: session.clone(),
            }],
        )?;
        let native = directory.path.join(format!("{name}.editbay"));
        save_new(editor.project(), &native)?;
        let reopened = load(&native)?;
        assert_eq!(reopened, *editor.project());
        assert_eq!(reopened.sequences[0].composition, Some(program));
        let master = directory.path.join(format!("{name}.mov"));
        let control = deadline.control()?;
        let receipt = deliver(
            &cli,
            Arc::new(reopened),
            request,
            &master,
            &control,
            |_, _| {},
        )?;
        let measured = inspect_master(&master, &receipt, name, false, &cancel)?;
        write_new(
            &directory.path.join(format!("{name}.receipt.json")),
            &serde_json::to_vec_pretty(&json!({"receipt":receipt,"numeric_oracle":measured}))?,
        )?;
        observations
            .push(json!({"policy":name,"native":native,"master":master,"numeric_oracle":measured}));
    }

    let font_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/liberation/LiberationSans-Regular.ttf")
        .canonicalize()?;
    let font = SourceFile::open(&font_path, &cancel)?;
    let asset = AssetReference {
        id: Uuid::new_v4(),
        kind: AssetKind::Font,
        path: font.path().into(),
        sha256: font.fingerprint().sha256.clone(),
        bytes: font.fingerprint().bytes,
        provenance: "Existing OFL Liberation fixture; no font is copied".into(),
    };
    let mut captions = CaptionTrack::new(program, "Retained burn-in and sidecar")?;
    captions.style.font = Some(asset.id);
    captions.style.font_size = 6.0;
    captions.style.line_height = 1.0;
    captions.style.safe_area = CaptionSafeArea {
        left: 0.125,
        top: 0.125,
        right: 0.125,
        bottom: 0.125,
    };
    captions.style.background = [0.; 4];
    captions.delivery = CaptionDelivery::BurnInAndVtt;
    captions.insert_cue(CaptionCue::new(
        CaptionTime::from_milliseconds(125),
        CaptionTime::from_milliseconds(250),
        "I",
    )?)?;
    let caption_id = captions.id;
    apply(
        &mut editor,
        "Caption delivery",
        &[
            DocumentCommand::SetAsset { asset },
            DocumentCommand::SetCaptionTrack { track: captions },
        ],
    )?;
    let native = directory.path.join("captioned.editbay");
    save_new(editor.project(), &native)?;
    let reopened = Arc::new(load(&native)?);
    assert_eq!(*reopened, *editor.project());

    let mut optional = (*reopened).clone();
    optional.captions[0].delivery = CaptionDelivery::SidecarVtt;
    optional.captions[0].cues[0].text = "<i>I</i>".into();
    let mut graph = GraphRenderer::new(
        Arc::new(EvaluationSnapshot::new(Arc::new(optional.clone()))?),
        PictureBudget::default(),
        GraphBudget::default(),
        deadline.media()?,
    )?;
    assert_eq!(optional.color, editbay_core::ColorConfiguration::default());
    let scope_settings = ScopeSettings {
        columns: 16,
        rows: 16,
        vector_size: 16,
        maximum_samples: (WIDTH * HEIGHT) as usize,
        scale: ScopeScale::Linear,
        ..Default::default()
    };
    let mut program_scopes = Vec::new();
    let mut previous_identity = None;
    for (frame, expected, luminance) in [
        (0, [1.0, 0.0, 0.0], 0.2126),
        (OUTPUT_FRAMES - 1, [0.0, 1.0, 0.0], 0.7152),
    ] {
        let rendered = graph.render(program, SourcePosition::new(frame as i64, 1)?, false)?;
        let pixels = graph.readback(&rendered)?;
        let identity = ScopeIdentity {
            version: DocumentVersion::of(&optional),
            composition: program,
            frame,
        };
        let report = sample_program_frame(
            &pixels,
            rendered.image().dimensions(),
            identity,
            optional.color,
            scope_settings,
        )?;
        assert_eq!(report.valid_pixels, u64::from(WIDTH * HEIGHT));
        assert_eq!(report.invalid_pixels, 0);
        assert_eq!(report.transparent_pixels, 0);
        for (range, expected) in report.statistics.linear_rgb.iter().zip(expected) {
            let range = range.ok_or("program scope channel absent")?;
            assert!((range.minimum - expected).abs() < 1.0e-4);
            assert!((range.maximum - expected).abs() < 1.0e-4);
        }
        let range = report
            .statistics
            .luminance_nits
            .ok_or("program luminance absent")?;
        assert!((range.minimum - luminance * 100.0).abs() < 0.02);
        assert!((range.maximum - luminance * 100.0).abs() < 0.02);
        assert!(report.matches(identity, optional.color, scope_settings));
        if let Some(previous) = previous_identity {
            assert!(!report.matches(previous, optional.color, scope_settings));
        }
        previous_identity = Some(identity);
        let paused_settings = ScopeSettings {
            scale: ScopeScale::Nits,
            reference_white_nits: 200.0,
            ..scope_settings
        };
        let paused = sample_program_frame(
            &pixels,
            rendered.image().dimensions(),
            identity,
            optional.color,
            paused_settings,
        )?;
        assert!(!report.matches(identity, optional.color, paused_settings));
        assert!(paused.matches(identity, optional.color, paused_settings));
        let range = paused
            .statistics
            .luminance_nits
            .ok_or("paused luminance absent")?;
        assert!((range.minimum - luminance * 200.0).abs() < 0.04);
        assert!((range.maximum - luminance * 200.0).abs() < 0.04);
        program_scopes.push(json!({"frame":frame,"linear":report,"same_frame_nits":paused}));
    }
    let position = SourcePosition::new(3, 1)?;
    let picture = graph.render(program, position, false)?;
    let (preview, warnings) = graph.preview_captions(&picture, program, position)?;
    assert!(
        Arc::ptr_eq(picture.image(), preview.image()),
        "an unsupported optional caption must preserve the program picture"
    );
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("explicit conversion"));
    optional.captions[0].delivery = CaptionDelivery::BurnIn;
    graph.rebind(
        Arc::new(EvaluationSnapshot::new(Arc::new(optional))?),
        deadline.media()?,
    )?;
    assert!(
        graph
            .render(program, position, false)
            .err()
            .expect("required burn-in must reject unsupported rich captions")
            .to_string()
            .contains("explicit conversion")
    );
    graph.finish()?;
    drop(graph);

    let blocked = directory.path.join("existing-master.mov");
    write_new(&blocked, b"owner master")?;
    let blocked_error = deliver(
        &cli,
        reopened.clone(),
        request,
        &blocked,
        &deadline.control()?,
        |_, _| {},
    )
    .expect_err("Existing master must reject publication");
    assert!(
        blocked_error
            .to_string()
            .contains("destination already exists")
    );
    assert_eq!(fs::read(&blocked)?, b"owner master");
    assert!(!blocked.with_extension(format!("{caption_id}.vtt")).exists());
    let blocked = directory.path.join("existing-sidecar.mov");
    let sidecar = blocked.with_extension(format!("{caption_id}.vtt"));
    write_new(&sidecar, b"owner sidecar")?;
    let blocked_error = deliver(
        &cli,
        reopened.clone(),
        request,
        &blocked,
        &deadline.control()?,
        |_, _| {},
    )
    .expect_err("Existing sidecar must reject publication");
    assert!(blocked_error.to_string().contains("sidecar already exists"));
    assert!(!blocked.exists());
    assert_eq!(fs::read(&sidecar)?, b"owner sidecar");

    let cancelled = directory.path.join("cancelled.mov");
    let control = deadline.control()?;
    let mut cancellation_accepted = false;
    let cancelled_result = deliver(
        &cli,
        reopened.clone(),
        request,
        &cancelled,
        &control,
        |progress, _| {
            if progress.phase == Phase::Rendering && progress.pictures >= 1 {
                cancellation_accepted |= control.cancel();
            }
        },
    );
    assert!(
        cancellation_accepted,
        "Cancellation must exercise an actual rendered private output"
    );
    let cancelled_error =
        cancelled_result.expect_err("Accepted cancellation must reject publication");
    assert!(
        cancelled_error
            .to_string()
            .to_ascii_lowercase()
            .contains("cancel")
    );
    assert!(!cancelled.exists());
    assert!(
        !cancelled
            .with_extension(format!("{caption_id}.vtt"))
            .exists()
    );

    let master = directory.path.join("captioned.mov");
    let receipt = deliver(
        &cli,
        reopened.clone(),
        request,
        &master,
        &deadline.control()?,
        |_, _| {},
    )?;
    let measured = inspect_master(&master, &receipt, "follow", true, &cancel)?;
    let sidecar = master.with_extension(format!("{caption_id}.vtt"));
    assert_eq!(
        receipt.caption_sidecars.as_slice(),
        std::slice::from_ref(&sidecar)
    );
    let text = fs::read_to_string(&sidecar)?;
    assert_eq!(text, "WEBVTT\n\n00:00:00.125 --> 00:00:00.250\nI\n\n");
    let parsed = CaptionTrack::import_vtt(program, "Reopened emitted sidecar", &text)?;
    assert_eq!(parsed.cues.len(), 1);
    assert_eq!(parsed.cues[0].start.to_milliseconds()?, 125);
    assert_eq!(parsed.cues[0].end.to_milliseconds()?, 250);
    write_new(
        &directory.path.join("captioned.receipt.json"),
        &serde_json::to_vec_pretty(&json!({"receipt":receipt,"numeric_oracle":measured}))?,
    )?;
    cli_file.verify(&cancel)?;
    for asset in &reopened.assets {
        let file = SourceFile::open(&asset.path, &cancel)?;
        assert_eq!(file.fingerprint().sha256, asset.sha256);
        assert_eq!(file.fingerprint().bytes, asset.bytes);
    }
    assert!(
        !deadline.expired.load(Ordering::Acquire),
        "Workflow delivery exceeded 30 seconds"
    );
    assert!(
        deadline.started.elapsed() <= DEADLINE,
        "Workflow delivery exceeded 30 seconds"
    );
    write_new(
        &directory.path.join("workflow-delivery.json"),
        &serde_json::to_vec_pretty(&json!({
            "schema":1,"status":"passed","elapsed_ms":deadline.started.elapsed().as_millis(),
            "cli":{"path":cli,"fingerprint":cli_file.fingerprint(),"build_receipt":binding_path,"source_revision":revision.trim()},
            "version":DocumentVersion::of(&reopened),"input_assets":reopened.assets,
            "program":program,"markers":[2,4],"camera_b_offset_frames":-2,"cut_frame":CUT_FRAME,
            "policy_deliveries":observations,"captioned_numeric_oracle":measured,"program_scopes":program_scopes,
            "preservation":{"original_camera_compositions":true,"undo_redo":true,"save_reopen":true,"source_hashes":true,"existing_master":true,"existing_sidecar":true,"cancel_before_publication":true},
            "proof_boundary":"Actual CPU lavapipe/Vulkan render, native MOV/PCM encode and decode with independent numeric frame/sample/working-linear scope oracles and the CLI worker. Same-frame scope settings are sampled from actual program pixels; native UI paused/playback updates remain unproven. Encode and decode share FFmpeg; this does not prove independent codec conformance. No desktop, audio output, physical display, audio alignment from a device, external NLE or physical hardware acceptance is claimed."
        }))?,
    )?;
    Ok(())
}

/// Bind software-only worker execution to the exact clean compiler receipt.
/// Takes explicit environment paths; returns the actual CLI and source packet.
fn bound_cli() -> TestResult<(PathBuf, Value)> {
    assert_eq!(
        std::env::var("WGPU_BACKEND").as_deref(),
        Ok("vulkan"),
        "Set WGPU_BACKEND=vulkan explicitly; this test must never choose a desktop GPU"
    );
    assert_eq!(
        std::env::var("VK_DRIVER_FILES").as_deref(),
        Ok("/usr/share/vulkan/icd.d/lvp_icd.json"),
        "Pin the CPU lavapipe ICD explicitly"
    );
    assert!(Path::new("/usr/share/vulkan/icd.d/lvp_icd.json").is_file());
    assert!(
        matches!(std::env::var("LP_NUM_THREADS").as_deref(), Ok("1" | "2")),
        "Set LP_NUM_THREADS=2 to bound CPU software rasterizer threads"
    );
    assert!(
        std::env::temp_dir().canonicalize()?.starts_with("/home"),
        "Set TMPDIR beneath /home: native anonymous PCM storage follows std::env::temp_dir()"
    );
    let cli = PathBuf::from(std::env::var_os("EDITBAY_TEST_CLI").ok_or("EDITBAY_TEST_CLI must point to the exact-current built editbay CLI; unavailable runtime is a failure")?).canonicalize()?;
    let metadata = cli.metadata()?;
    assert!(
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
        "EDITBAY_TEST_CLI must be executable"
    );
    let cancel = Cancellation::new()?;
    let cli_file = SourceFile::open(&cli, &cancel)?;
    let binding_path = PathBuf::from(std::env::var_os("EDITBAY_TEST_CLI_BUILD_RECEIPT").ok_or(
        "EDITBAY_TEST_CLI_BUILD_RECEIPT must identify the exact-current serialized CLI build",
    )?);
    let mut binding: Value = serde_json::from_slice(&fs::read(&binding_path)?)?;
    let git = Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
        .output()?;
    assert!(git.status.success());
    let revision = String::from_utf8(git.stdout)?;
    assert_eq!(binding["repository"], "editbay");
    let compiled = binding["revision"]
        .as_str()
        .ok_or("Compiler source revision absent")?;
    if compiled != revision.trim() {
        assert!(compiled.len() == 40 && compiled.bytes().all(|v| v.is_ascii_hexdigit()));
        let changes = Command::new("git")
            .args([
                "-C",
                env!("CARGO_MANIFEST_DIR"),
                "diff",
                "--name-only",
                compiled,
                revision.trim(),
                "--",
            ])
            .output()?;
        assert!(changes.status.success());
        let changed = String::from_utf8(changes.stdout)?;
        assert!(
            changed
                .lines()
                .all(|p| p.starts_with("crates/editbay-delivery/tests/")
                    || p.starts_with("crates/editbay-media/tests/")),
            "CLI product source changed since compiler receipt: {changed}"
        );
    }
    assert_eq!(binding["exit_code"], 0);
    assert_eq!(binding["dirty_before"], false);
    assert_eq!(binding["dirty_after"], false);
    assert!(binding["signal"].is_null());
    assert_eq!(
        binding["receipt_errors"]
            .as_array()
            .ok_or("missing receipt errors")?
            .len(),
        0
    );
    let executable = binding["executables"]
        .as_array()
        .ok_or("missing executable bindings")?
        .iter()
        .find(|entry| {
            entry["target"]["name"] == "editbay"
                && entry["path"].as_str().is_some_and(|p| Path::new(p) == cli)
        })
        .ok_or("serialized receipt does not bind this CLI")?;
    assert_eq!(executable["sha256"], cli_file.fingerprint().sha256);
    assert_eq!(executable["size"], cli_file.fingerprint().bytes);
    binding["test_source_revision"] = json!(revision.trim());
    binding["actual_cli_fingerprint"] = json!(cli_file.fingerprint());
    binding["actual_cli_compiler_packet"] = json!(binding_path);
    Ok((cli, binding))
}

/// Generate distinct low-bit color and alpha values independently of the media code.
/// `pixel` selects one of 256 samples and returns straight encoded RGBA16 components.
fn precision_pixel(pixel: usize) -> [u16; 4] {
    let n = pixel as u16;
    [
        n * 257 + u16::from(n < 255),
        n * 251,
        65535 - n * 193,
        [0, 1, 257, 4096, 32768, 65535][pixel % 6],
    ]
}

/// Independently decode BT.2100 encoded RGB into relative display light.
/// `code`, `hlg` and `white` define a 1000-nit display and return straight linear RGB.
fn hdr_light(code: [f64; 3], hlg: bool, white: f64) -> [f64; 3] {
    if !hlg {
        return code.map(|v| {
            let power = v.powf(32. / 2523.);
            ((power - 3424. / 4096.).max(0.) / (2413. / 128. - 2392. / 128. * power))
                .powf(16384. / 2610.)
                * 10000.
                / white
        });
    }
    let a = 0.17883277_f64;
    let b = 1. - 4. * a;
    let c = 0.5 - a * (4. * a).ln();
    let scene = code.map(|v| {
        if v <= 0.5 {
            v * v / 3.
        } else {
            (((v - c) / a).exp() + b) / 12.
        }
    });
    let luma = scene[0] * 0.2627 + scene[1] * 0.6780 + scene[2] * 0.0593;
    scene.map(|v| v * luma.powf(0.2) * 1000. / white)
}

#[test]
fn craft_video_icc_source_float_graph_assignment_reopen_and_atomic_master() -> TestResult<()> {
    use editbay_core::{AlphaMode, WorkingGamut, source_icc_command};
    use editbay_media::PixelEncoding;
    let (cli, binding) = bound_cli()?;
    let directory = FixtureDirectory::new()?;
    let cancel = Cancellation::new()?;
    let deadline = DeadlineGuard::new(cancel.clone())?;
    let profile_bytes = icc_fixture::original_profile()?;
    let input = directory.path.join("original-icc-rgb16.png");
    let profile_path = directory.path.join("original-matrix-trc.icc");
    write_new(&input, &icc_fixture::original_png(&profile_bytes)?)?;
    write_new(&profile_path, &profile_bytes)?;
    let source = SourceFile::open(&input, &cancel)?;
    let imported = source.ingest(
        "Original RGB16 ICC source".into(),
        &[0],
        cancel.clone(),
        |_, _| {},
    )?;
    let source_id = imported.source.id;
    let mut editor = DocumentEditor::new(Project::new("ICC through the native float graph")?)?;
    apply(&mut editor, "Import ICC source", &imported.commands())?;
    let commands =
        editbay_core::sequence_from_video(editor.project(), source_id, 0, FrameRate::new(25, 1)?)?;
    let composition = commands
        .iter()
        .find_map(|command| match command {
            DocumentCommand::SetComposition { composition } => Some(composition.id),
            _ => None,
        })
        .ok_or("ICC composition absent")?;
    apply(&mut editor, "Create ICC source sequence", &commands)?;
    let native = directory.path.join("icc-source.editbay");
    save_new(editor.project(), &native)?;
    let project = Arc::new(load(&native)?);
    assert_eq!(*project, *editor.project());
    let profile = project.sources[0].streams[0]
        .icc_profile
        .clone()
        .ok_or("Embedded ICC absent")?;
    assert_eq!(profile.bytes, profile_bytes);
    assert_eq!(
        project.sources[0].streams[0].metadata["editbay.native_pixel_model"],
        "rgb"
    );
    let snapshot = Arc::new(EvaluationSnapshot::new(project.clone())?);
    let mut graph = GraphRenderer::new(
        snapshot,
        PictureBudget::default(),
        GraphBudget::default(),
        deadline.media()?,
    )?;
    let graph_adapter = format!("{:?}", graph.adapter);
    assert_cpu_adapter(&graph_adapter);
    let frame = graph.render(composition, SourcePosition::new(0, 1)?, false)?;
    let actual = graph.readback(&frame)?;
    assert_eq!(
        actual.len(),
        (icc_fixture::WIDTH * icc_fixture::HEIGHT * 4) as usize
    );
    let mut maximum_working_error = 0_f64;
    for (pixel, original) in actual.as_chunks::<4>().0.iter().zip(icc_fixture::samples()) {
        let encoded = original.map(|value| f64::from(value) / 65535.);
        let (expected, allowance) = icc_fixture::expected(encoded, WorkingGamut::Bt709);
        for channel in 0..3 {
            let error = (f64::from(pixel[channel]) - expected[channel]).abs();
            maximum_working_error = maximum_working_error.max(error);
            assert!(
                error <= allowance[channel] + 2e-6,
                "ICC working sample {encoded:?}: {pixel:?} expected {expected:?} allowance {allowance:?}"
            );
        }
        assert_eq!(pixel[3], 1.);
    }
    let clear = source_icc_command(editor.project(), source_id, 0, None)?;
    apply(&mut editor, "Clear interpretation", &[clear])?;
    assert!(editor.project().sources[0].streams[0].icc_profile.is_none());
    editor.undo(DocumentVersion::of(editor.project()))?;
    assert_eq!(
        editor.project().sources[0].streams[0].icc_profile.as_ref(),
        Some(&profile)
    );
    graph.rebind(
        Arc::new(EvaluationSnapshot::new(editor.snapshot())?),
        deadline.media()?,
    )?;
    assert!(
        graph.readback(&frame).is_err(),
        "ICC source history must reject an obsolete render receipt"
    );
    let rebound = graph.render(composition, SourcePosition::new(0, 1)?, false)?;
    assert_eq!(graph.readback(&rebound)?, actual);
    graph.clear()?;
    let cleared = graph.render(composition, SourcePosition::new(0, 1)?, false)?;
    assert_eq!(graph.readback(&cleared)?, actual);
    drop(graph);

    let command_args = [
        "source-profile".to_owned(),
        native.to_string_lossy().into_owned(),
        source_id.to_string(),
        "0".into(),
    ];
    let cleared = Command::new(&cli)
        .args(&command_args)
        .arg("none")
        .output()?;
    assert!(
        cleared.status.success(),
        "{}",
        String::from_utf8_lossy(&cleared.stderr)
    );
    assert!(load(&native)?.sources[0].streams[0].icc_profile.is_none());
    let assigned = Command::new(&cli)
        .args(&command_args)
        .arg(&profile_path)
        .output()?;
    assert!(
        assigned.status.success(),
        "{}",
        String::from_utf8_lossy(&assigned.stderr)
    );
    let project = Arc::new(load(&native)?);
    assert_eq!(
        project.sources[0].streams[0].icc_profile.as_ref(),
        Some(&profile)
    );
    let malformed = directory.path.join("malformed.icc");
    write_new(&malformed, b"invalid ICC")?;
    let before_invalid = fs::read(&native)?;
    let refused = Command::new(&cli)
        .args(&command_args)
        .arg(&malformed)
        .output()?;
    assert!(!refused.status.success());
    assert_eq!(fs::read(&native)?, before_invalid);
    let request = DeliveryRequest {
        format: DeliveryFormat::Lossless16Mov,
        composition,
        sample_rate: 48000,
        range: None,
    };
    let master = directory.path.join("icc-converted-master.mov");
    let receipt = deliver(
        &cli,
        project.clone(),
        request,
        &master,
        &deadline.control()?,
        |_, _| {},
    )?;
    assert_cpu_adapter(&receipt.adapter);
    assert_eq!(receipt.pixel_sha256, receipt.decoded_pixel_sha256);
    let output = SourceFile::open(&master, &cancel)?;
    let probe = output.probe(cancel.clone())?;
    assert_eq!(
        (
            probe.streams[0].color.primaries,
            probe.streams[0].color.transfer
        ),
        (1, 13)
    );
    let mut reader = VideoReader::open_stream_with_encoding(
        &output,
        0,
        cancel.clone(),
        PixelEncoding::Rgba16Le,
    )?;
    let picture = reader
        .next_frame()?
        .ok_or("ICC converted master frame absent")?;
    assert!(reader.next_frame()?.is_none());
    let srgb = |linear: f64| -> f64 {
        if linear <= 0.0031308 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1. / 2.4) - 0.055
        }
    };
    let mut maximum_code_error = 0_f64;
    let mut expected_samples = Vec::new();
    for (pixel, original) in picture
        .rgba
        .as_chunks::<8>()
        .0
        .iter()
        .zip(icc_fixture::samples())
    {
        let (expected, allowance) = icc_fixture::expected(
            original.map(|value| f64::from(value) / 65535.),
            WorkingGamut::Bt709,
        );
        let mut codes = Vec::new();
        for channel in 0..3 {
            let actual = u16::from_le_bytes(pixel[channel * 2..channel * 2 + 2].try_into()?);
            let expected_code = (srgb(expected[channel]).clamp(0., 1.) * 65535.).round();
            maximum_code_error = maximum_code_error.max((f64::from(actual) - expected_code).abs());
            let low = (srgb(expected[channel] - allowance[channel] - 2e-6).clamp(0., 1.) * 65535.)
                .floor();
            let high =
                (srgb(expected[channel] + allowance[channel] + 2e-6).clamp(0., 1.) * 65535.).ceil();
            assert!(
                f64::from(actual) >= low && f64::from(actual) <= high,
                "ICC master code {actual}, allowed {low}..{high}"
            );
            codes.push(json!({"expected":expected_code,"minimum":low,"maximum":high}));
        }
        assert_eq!(u16::from_le_bytes(pixel[6..8].try_into()?), 65535);
        expected_samples.push(codes);
    }
    let cancelled = directory.path.join("icc-cancelled.mov");
    let control = deadline.control()?;
    let mut accepted = false;
    assert!(
        deliver(
            &cli,
            project,
            request,
            &cancelled,
            &control,
            |progress, _| {
                if (progress.phase == Phase::Rendering && progress.pictures >= 1)
                    || progress.phase == Phase::VerifyingPictures
                {
                    accepted |= control.cancel();
                }
            }
        )
        .is_err()
    );
    assert!(
        accepted,
        "Cancel actual private output after its sole picture was rendered"
    );
    assert!(!cancelled.exists());
    let preserved_master = fs::read(&master)?;
    assert!(
        deliver(
            &cli,
            Arc::new(load(&native)?),
            request,
            &master,
            &deadline.control()?,
            |_, _| {}
        )
        .is_err()
    );
    assert_eq!(fs::read(&master)?, preserved_master);
    source.verify(&cancel)?;
    write_new(
        &directory.path.join("icc-pipeline.json"),
        &serde_json::to_vec_pretty(&json!({
            "schema":"editbay.icc-pipeline.v1","status":"passed","source_revision":binding["test_source_revision"],
            "cli_source_revision":binding["revision"],"cli_build_receipt":binding["actual_cli_compiler_packet"],"cli_fingerprint":binding["actual_cli_fingerprint"],
        "input":input,"input_fingerprint":source.fingerprint(),"profile":profile_path,"profile_sha256":profile.sha256,"graph_adapter":graph_adapter,
            "native":native,"master":master,"receipt":receipt,"probe":probe,"dimensions":[icc_fixture::WIDTH,icc_fixture::HEIGHT],
            "maximum_working_error":maximum_working_error,"maximum_code_error":maximum_code_error,"expected_samples":expected_samples,
            "undo_rebind_clear_equal":true,"obsolete_frame_rejected":true,"cli_clear_assign_reopen":true,"malformed_cli_preserves_project":true,
            "post_render_cancel_no_publication":true,"existing_destination_preserved":true,"source_preserved":true,
            "generator_source_sha256":format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(include_bytes!("../../editbay-media/tests/fixtures/icc.rs"))),
            "gamma_bits":icc_fixture::GAMMA_BITS,"gamma":icc_fixture::GAMMA,"numeric_allowance":icc_fixture::NATIVE_ALLOWANCE,
            "format_references":[icc_fixture::ICC_SPEC,icc_fixture::PNG_SPEC],"alpha":AlphaMode::Opaque,
            "proof_boundary":"Actual native ICC PNG16 capture, same working-float graph, native16 master/QC and typed/CLI profile assignment. Independent analytic matrix/TRC oracle; no second CMM, output ICC embedding, general ICC conformance or physical display proof."
        }))?,
    )?;
    Ok(())
}

#[test]
fn craft_video_hdr_native_tags_float_light_and_high_depth_delivery() -> TestResult<()> {
    use editbay_core::{
        ContentLight, HdrConfiguration, HdrMetadata, MasteringDisplay, OutputTransfer, WorkingGamut,
    };
    use editbay_media::{DeliveryColor, PixelEncoding};
    let (cli, binding) = bound_cli()?;
    let directory = FixtureDirectory::new()?;
    let cancel = Cancellation::new()?;
    let deadline = DeadlineGuard::new(cancel.clone())?;
    let mut cases = Vec::new();
    let metadata = HdrMetadata {
        mastering_display: Some(MasteringDisplay {
            primaries_xy_50000: [[35400, 14600], [8500, 39850], [6550, 2300]],
            white_xy_50000: [15635, 16450],
            minimum_luminance_10000: 50,
            maximum_luminance_10000: 10_000_000,
        }),
        content_light: Some(ContentLight {
            maximum_content_light_nits: 1000,
            maximum_frame_average_light_nits: 400,
        }),
    };
    for (name, transfer, hlg) in [
        ("pq", OutputTransfer::Pq, false),
        ("hlg", OutputTransfer::Hlg, true),
    ] {
        let color = DeliveryColor {
            primaries: 9,
            transfer: if hlg { 18 } else { 16 },
            hdr_metadata: metadata,
        };
        let input = directory.path.join(format!("hdr-{name}.mov"));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(&input)?;
        let profile = LosslessMovProfile {
            width: WIDTH,
            height: HEIGHT,
            frame_rate: FrameRate::new(24, 1)?,
            first_frame: 0,
            frames: INPUT_FRAMES,
            sample_rate: 48000,
            channels: vec!["FC".into()],
        };
        let pixels: Vec<u8> = (0..256)
            .flat_map(precision_pixel)
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut writer = LosslessMovWriter::encoded_color(
            &file,
            profile.clone(),
            DeliveryFormat::Lossless16Mov,
            color,
            cancel.clone(),
        )?;
        let mut first = 0;
        for frame in 0..INPUT_FRAMES {
            writer.picture(frame, &pixels)?;
            let end = profile.samples_through(frame + 1)?;
            writer.sound(
                first,
                &(first..end).map(|n| camera_pcm(0, n)).collect::<Vec<_>>(),
            )?;
            first = end;
        }
        writer.finish()?;
        file.sync_all()?;
        let mut editor = DocumentEditor::new(Project::new(format!("Actual HDR {name}"))?)?;
        let composition = import_camera(&mut editor, &input, &cancel)?;
        assert_eq!(
            editor.project().sources[0].streams[0].hdr_metadata,
            metadata
        );
        let mut configuration = editor.project().color;
        configuration.working_gamut = WorkingGamut::Bt2020;
        configuration.output_gamut = WorkingGamut::Bt2020;
        configuration.output_transfer = transfer;
        configuration.hdr = HdrConfiguration {
            metadata,
            reference_white_nits: 203,
            peak_nits: 1000,
        };
        apply(
            &mut editor,
            "Declare HDR scale",
            &[DocumentCommand::SetColor { configuration }],
        )?;
        let native = directory.path.join(format!("hdr-{name}.editbay"));
        save_new(editor.project(), &native)?;
        let project = Arc::new(load(&native)?);
        assert_eq!(*project, *editor.project());
        let mut graph = GraphRenderer::new(
            Arc::new(EvaluationSnapshot::new(project.clone())?),
            PictureBudget::default(),
            GraphBudget::default(),
            deadline.media()?,
        )?;
        let graph_adapter = format!("{:?}", graph.adapter);
        assert_cpu_adapter(&graph_adapter);
        let frame = graph.render(composition, SourcePosition::new(0, 1)?, false)?;
        let actual = graph.readback(&frame)?;
        let mut max_relative_error = 0_f64;
        for (index, pixel) in actual.as_chunks::<4>().0.iter().enumerate() {
            let codes = precision_pixel(index);
            let alpha = f64::from(codes[3]) / 65535.;
            let expected = hdr_light(
                [
                    f64::from(codes[0]) / 65535.,
                    f64::from(codes[1]) / 65535.,
                    f64::from(codes[2]) / 65535.,
                ],
                hlg,
                203.,
            );
            for channel in 0..3 {
                let relative = (f64::from(pixel[channel]) - expected[channel] * alpha).abs()
                    / (expected[channel] * alpha).abs().max(0.01);
                max_relative_error = max_relative_error.max(relative);
            }
        }
        assert!(
            max_relative_error < 0.0003,
            "{name} EOTF/OOTF error {max_relative_error}"
        );
        let scopes = sample_program_frame(
            &actual,
            [WIDTH, HEIGHT],
            ScopeIdentity {
                version: DocumentVersion::of(&project),
                composition,
                frame: 0,
            },
            configuration,
            ScopeSettings {
                reference_white_nits: 203.,
                peak_nits: 1000.,
                scale: ScopeScale::Nits,
                ..ScopeSettings::default()
            },
        )?;
        assert_eq!(scopes.invalid_pixels, 0);
        assert!(
            scopes
                .clipping
                .above_reference_white_channels
                .iter()
                .sum::<u64>()
                > 0
        );
        let expected_max_nits = (0..256)
            .filter(|index| precision_pixel(*index)[3] != 0)
            .map(|index| {
                let codes = precision_pixel(index);
                let rgb = hdr_light(
                    [
                        f64::from(codes[0]) / 65535.,
                        f64::from(codes[1]) / 65535.,
                        f64::from(codes[2]) / 65535.,
                    ],
                    hlg,
                    203.,
                );
                (rgb[0] * 0.2627 + rgb[1] * 0.6780 + rgb[2] * 0.0593) * 203.
            })
            .fold(0_f64, f64::max);
        let actual_max_nits = scopes
            .statistics
            .luminance_nits
            .ok_or("HDR nits range absent")?
            .maximum;
        assert!((actual_max_nits - expected_max_nits).abs() / expected_max_nits < 0.0003);
        drop(graph);
        let master = directory.path.join(format!("hdr-{name}-master.mov"));
        let request = DeliveryRequest {
            format: DeliveryFormat::Lossless16Mov,
            composition,
            sample_rate: 48000,
            range: Some(editbay_core::FrameRange { start: 1, end: 3 }),
        };
        let receipt = deliver(
            &cli,
            project,
            request,
            &master,
            &deadline.control()?,
            |_, _| {},
        )?;
        assert_eq!(receipt.color, color);
        assert_cpu_adapter(&receipt.adapter);
        assert_eq!(receipt.pixel_sha256, receipt.decoded_pixel_sha256);
        let source = SourceFile::open(&master, &cancel)?;
        let probe = source.probe(cancel.clone())?;
        assert_eq!(probe.streams[0].hdr_metadata, metadata);
        assert_eq!(
            (
                probe.streams[0].color.primaries,
                probe.streams[0].color.transfer
            ),
            (9, color.transfer)
        );
        let mut read = VideoReader::open_stream_with_encoding(
            &source,
            0,
            cancel.clone(),
            PixelEncoding::Rgba16Le,
        )?;
        let mut frames = 0;
        let mut max_code_error = 0_u16;
        while let Some(decoded) = read.next_frame()? {
            for (index, pixel) in decoded.rgba.as_chunks::<8>().0.iter().enumerate() {
                let expected = precision_pixel(index);
                for (channel, bytes) in pixel.as_chunks::<2>().0.iter().enumerate() {
                    let expected = if expected[3] == 0 && channel != 3 {
                        0
                    } else {
                        expected[channel]
                    };
                    max_code_error =
                        max_code_error.max(u16::from_le_bytes(*bytes).abs_diff(expected));
                }
            }
            frames += 1;
        }
        assert_eq!(frames, 2);
        assert!(
            max_code_error <= 8,
            "{name} roundtrip code error {max_code_error}"
        );
        cases.push(json!({"case":name,"input":input,"native":native,"master":master,"receipt":receipt,"graph_adapter":graph_adapter,
            "maximum_relative_working_error":max_relative_error,"maximum_roundtrip_code_error":max_code_error,"probe":probe,"actual_program_scopes":scopes}));
    }
    write_new(
        &directory.path.join("hdr-pipeline.json"),
        &serde_json::to_vec_pretty(&json!({
            "schema":"editbay.hdr-pipeline.v2","status":"passed","source_revision":binding["test_source_revision"],"cli_source_revision":binding["revision"],"cli_build_receipt":binding["actual_cli_compiler_packet"],"cli_fingerprint":binding["actual_cli_fingerprint"],"cases":cases,"expected_static_metadata":metadata,
            "reference":"Independent f64 BT.2100 PQ EOTF and HLG inverse OETF/OOTF, 203-nit white and 1000-nit HLG display",
            "proof_boundary":"Actual CPU Vulkan and native high-depth color-tagged MOV/QC with exact static mastering/content-light tags. Declared metadata is not measured display/content luminance. No HDR display/device/physical proof or general codec conformance."
        }))?,
    )?;
    Ok(())
}

#[test]
fn craft_video_precision_decode_worker_store_float_graph_and_atomic_master() -> TestResult<()> {
    use editbay_core::{FrameRange, SourceRequest};
    use editbay_media::{
        PictureCache, PictureProvider, PixelEncoding,
        picture_store::{StoreBudget, StorePlan, StoreSpace},
        picture_worker::{PictureWorker, WorkerBudget},
    };
    use editbay_render::ImageBoundary;

    let (cli, binding) = bound_cli()?;
    let directory = FixtureDirectory::new()?;
    let cancel = Cancellation::new()?;
    let deadline = DeadlineGuard::new(cancel.clone())?;
    let input = directory.path.join("precision-input.mov");
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .read(true)
        .open(&input)?;
    let profile = LosslessMovProfile {
        width: WIDTH,
        height: HEIGHT,
        frame_rate: FrameRate::new(24, 1)?,
        first_frame: 0,
        frames: INPUT_FRAMES,
        sample_rate: 48000,
        channels: vec!["FC".into()],
    };
    let pixels: Vec<u8> = (0..256)
        .flat_map(precision_pixel)
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut writer = LosslessMovWriter::encoded(
        &file,
        profile.clone(),
        DeliveryFormat::Lossless16Mov,
        cancel.clone(),
    )?;
    let mut first = 0;
    for frame in 0..INPUT_FRAMES {
        writer.picture(frame, &pixels)?;
        let end = profile.samples_through(frame + 1)?;
        writer.sound(
            first,
            &(first..end).map(|n| camera_pcm(0, n)).collect::<Vec<_>>(),
        )?;
        first = end;
    }
    writer.finish()?;
    file.sync_all()?;
    let mut editor = DocumentEditor::new(Project::new("Native precision through one pipeline")?)?;
    let composition = import_camera(&mut editor, &input, &cancel)?;
    assert_eq!(
        editor.project().sources[0].streams[0].metadata["editbay.pixel_encoding"],
        "rgba16_le"
    );
    let native = directory.path.join("precision.editbay");
    save_new(editor.project(), &native)?;
    let project = Arc::new(load(&native)?);
    assert_eq!(*project, *editor.project());
    let snapshot = Arc::new(EvaluationSnapshot::new(project.clone())?);
    let source_request: SourceRequest = snapshot
        .prepare(composition, SourcePosition::new(0, 1)?, false)?
        .nodes
        .into_iter()
        .find_map(|node| {
            node.source.filter(|s| {
                matches!(
                    s,
                    SourceRequest::Media {
                        picture: Some(_),
                        ..
                    }
                )
            })
        })
        .ok_or("media request absent")?;
    let budget = PictureBudget {
        maximum_picture_bytes: (WIDTH * HEIGHT * 4) as usize,
        ..PictureBudget::default()
    };
    let mut limited = PictureCache::new(snapshot.clone(), budget, deadline.media()?)?;
    assert!(
        limited.picture(&source_request).is_err(),
        "admit actual 8-byte pixels, not 4-byte accounting"
    );
    assert_eq!(limited.stats().live_bytes, 0);
    let mut worker = PictureWorker::new(
        &cli,
        snapshot.clone(),
        WorkerBudget::default(),
        deadline.media()?,
    )?;
    let pid = worker.process_id().ok_or("worker PID absent")?;
    let decoded = worker
        .picture(&source_request)?
        .ok_or("native pixels absent")?;
    assert_eq!(decoded.picture.encoding, PixelEncoding::Rgba16Le);
    assert_eq!(decoded.picture.rgba(), pixels);
    assert_eq!(worker.transfer_stats().mapped_bytes, pixels.len());
    let stale = decoded;
    let store_budget = StoreBudget {
        bytes: 256 * 1024,
        entries: 32,
    };
    let plan = StorePlan::new(
        snapshot.clone(),
        composition,
        FrameRange {
            start: 0,
            end: INPUT_FRAMES,
        },
        PictureBudget::default(),
        store_budget,
        &cancel,
    )?;
    assert_eq!(plan.summary().bytes, INPUT_FRAMES * pixels.len() as u64);
    let space = StoreSpace::new(store_budget)?;
    let store = plan.prepare(&directory.path, &space, &mut worker, &cancel, |_| {})?;
    worker.clear()?;
    assert!(worker.validate_result(&stale).is_err());
    drop(stale);
    drop(worker);
    assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    let mut prepared = PictureWorker::new(
        &cli,
        snapshot.clone(),
        WorkerBudget::default(),
        deadline.media()?,
    )?;
    prepared.attach_store(store)?;
    let stored = prepared
        .picture(&source_request)?
        .ok_or("stored native picture absent")?;
    assert_eq!(stored.picture.encoding, PixelEncoding::Rgba16Le);
    assert_eq!(stored.picture.rgba(), pixels);
    assert_eq!(prepared.transfer_stats().child.stored_reads, 1);
    drop(stored);
    drop(prepared);
    let mut renderer = GraphRenderer::new(
        snapshot,
        PictureBudget::default(),
        GraphBudget::default(),
        deadline.media()?,
    )?;
    let frame = renderer.render(composition, SourcePosition::new(0, 1)?, false)?;
    let linear = renderer.readback(&frame)?;
    let mut max_linear_error = 0_f64;
    for (index, actual) in linear.as_chunks::<4>().0.iter().enumerate() {
        let expected = precision_pixel(index);
        let alpha = f64::from(expected[3]) / 65535.;
        for channel in 0..3 {
            let encoded = f64::from(expected[channel]) / 65535.;
            let linear = if encoded <= 0.04045 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            };
            max_linear_error =
                max_linear_error.max((f64::from(actual[channel]) - linear * alpha).abs());
        }
        assert!((f64::from(actual[3]) - alpha).abs() < 1e-6);
    }
    assert!(
        max_linear_error < 2e-6,
        "16-bit source must reach working float without 8-bit truncation: {max_linear_error}"
    );
    let encoded = renderer.convert(&frame, ImageBoundary::Output)?;
    assert_eq!(renderer.readback(&encoded)?.len(), 1024);
    drop(renderer);
    let request = DeliveryRequest {
        format: DeliveryFormat::Lossless16Mov,
        composition,
        sample_rate: 48000,
        range: Some(FrameRange { start: 2, end: 5 }),
    };
    let master = directory.path.join("precision-master.mov");
    let receipt = deliver(
        &cli,
        project.clone(),
        request,
        &master,
        &deadline.control()?,
        |_, _| {},
    )?;
    assert_eq!(receipt.pixel_sha256, receipt.decoded_pixel_sha256);
    assert_eq!(receipt.pcm_sha256, receipt.decoded_pcm_sha256);
    let mut reader = VideoReader::open_with_encoding(&master, PixelEncoding::Rgba16Le)?;
    let mut count = 0;
    let mut max_code_error = 0_u16;
    let mut retained_low_bits = 0;
    while let Some(frame) = reader.next_frame()? {
        for (index, actual) in frame.rgba.as_chunks::<8>().0.iter().enumerate() {
            let expected = precision_pixel(index);
            let codes: Vec<_> = actual
                .as_chunks::<2>()
                .0
                .iter()
                .map(|v| u16::from_le_bytes(*v))
                .collect();
            assert_eq!(codes[3], expected[3]);
            for channel in 0..3 {
                let expected = if expected[3] == 0 {
                    0
                } else {
                    expected[channel]
                };
                max_code_error = max_code_error.max(codes[channel].abs_diff(expected));
                retained_low_bits += usize::from(!codes[channel].is_multiple_of(257));
            }
        }
        count += 1;
    }
    assert_eq!(count, 3);
    assert!(
        max_code_error <= 2,
        "16-bit roundtrip error {max_code_error}"
    );
    assert!(retained_low_bits > 500);
    let existing = fs::read(&master)?;
    assert!(
        deliver(
            &cli,
            project.clone(),
            request,
            &master,
            &deadline.control()?,
            |_, _| {}
        )
        .is_err()
    );
    assert_eq!(fs::read(&master)?, existing);
    let cancelled = directory.path.join("precision-cancelled.mov");
    let control = deadline.control()?;
    let mut accepted = false;
    assert!(
        deliver(
            &cli,
            project,
            request,
            &cancelled,
            &control,
            |progress, _| {
                if progress.phase == Phase::Rendering && progress.pictures >= 1 {
                    accepted |= control.cancel();
                }
            }
        )
        .is_err()
    );
    assert!(accepted && !cancelled.exists());
    write_new(
        &directory.path.join("precision-pipeline.json"),
        &serde_json::to_vec_pretty(&json!({
            "schema":"editbay.native-precision.v1", "status":"passed", "source_revision":binding["test_source_revision"], "cli_source_revision":binding["revision"], "cli_build_receipt":binding["actual_cli_compiler_packet"], "cli_fingerprint":binding["actual_cli_fingerprint"],
            "input":input, "native_project":native, "master":master, "receipt":receipt,
            "pixel_encoding":"rgba16_le", "maximum_working_linear_error":max_linear_error,
            "maximum_encoded_code_error":max_code_error, "retained_non8bit_components":retained_low_bits,
            "prepared_bytes":INPUT_FRAMES * pixels.len() as u64, "stale_rejected":true,
            "actual_worker_joined":true, "cancel_after_render_no_publication":true,
            "proof_boundary":"CPU Vulkan native decode/cache/IPC/store/float render/16-bit encode/QC. No desktop/audio/device/physical proof. Native codec encode/decode share FFmpeg; analytic sample oracle is separate. ICC/HDR and other codec families are separate pending cuts."
        }))?,
    )?;
    Ok(())
}
