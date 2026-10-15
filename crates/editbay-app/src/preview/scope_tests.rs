use super::*;
use editbay_core::{
    AnimatedProperty, AnimationChannel, ColorConfiguration, Composition, FrameRange, FrameRate,
    Interpolation, Keyframe, NodeOperation, Sequence, TimedNode,
};
use editbay_media::SourceFile;
use editbay_render::scopes::{ScopeIdentity, ScopeReport, ScopeScale, ScopeSettings};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const FRAMES: u64 = 4;
const SIDE: u32 = 16;
const LIMIT: Duration = Duration::from_secs(30);

struct EvidenceDirectory {
    path: PathBuf,
    _temporary: Option<tempfile::TempDir>,
}

impl EvidenceDirectory {
    /// Allocate only test-owned storage beneath /home.
    /// Takes the optional receipt directory; returns retained or temporary storage.
    fn new() -> TestResult<Self> {
        let requested = std::env::var_os("EDITBAY_WORKFLOW_RECEIPTS_DIR").map(PathBuf::from);
        let root = requested
            .clone()
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts"));
        fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        assert!(root.starts_with("/home"));
        if requested.is_some() {
            let path = root.join(format!("viewer-scopes-{}", Uuid::new_v4()));
            fs::create_dir(&path)?;
            Ok(Self {
                path,
                _temporary: None,
            })
        } else {
            let temporary = tempfile::Builder::new()
                .prefix("viewer-scopes-")
                .tempdir_in(root)?;
            Ok(Self {
                path: temporary.path().into(),
                _temporary: Some(temporary),
            })
        }
    }
}

struct Watchdog {
    began: Instant,
    expired: Arc<AtomicBool>,
    tokens: Arc<Mutex<Vec<Cancellation>>>,
    done: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    /// Cancel all test-owned native work at the shared deadline.
    /// `cancel` owns CLI hashing; returns a joined watchdog with no device access.
    fn new(cancel: Cancellation) -> TestResult<Self> {
        let began = Instant::now();
        let expired = Arc::new(AtomicBool::new(false));
        let tokens = Arc::new(Mutex::new(vec![cancel]));
        let state = expired.clone();
        let native = tokens.clone();
        let (done, finished) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("viewer-scopes-deadline".into())
            .spawn(move || {
                if finished.recv_timeout(LIMIT).is_err() {
                    state.store(true, Ordering::Release);
                    if let Ok(tokens) = native.lock() {
                        for token in tokens.iter() {
                            token.cancel();
                        }
                    }
                }
            })?;
        Ok(Self {
            began,
            expired,
            tokens,
            done: Some(done),
            thread: Some(thread),
        })
    }

    /// Retain cancellation for an actual viewer task.
    /// `task` supplies its owned lifetime; returns an error after the shared limit.
    fn register(&self, task: &Task) -> TestResult<()> {
        self.tokens.lock().unwrap().push(task.cancel.clone());
        if self.expired.load(Ordering::Acquire) {
            task.stop();
        }
        self.check()
    }

    /// Bound each asynchronous wait and every final receipt.
    /// Takes this guard; returns an explicit failure after thirty seconds.
    fn check(&self) -> TestResult<()> {
        if self.expired.load(Ordering::Acquire) || self.began.elapsed() >= LIMIT {
            return Err("Native viewer scope qualification exceeded 30 seconds".into());
        }
        Ok(())
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        if let Some(done) = self.done.take() {
            let _ = done.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct OwnedViewer {
    pane: PreviewPane,
}

impl Drop for OwnedViewer {
    fn drop(&mut self) {
        self.pane.stop();
        for task in &self.pane.retiring {
            task.stop();
        }
        for mut task in self.pane.retiring.drain(..) {
            if let Some(thread) = task.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

/// Bind the actual worker executable to its successful exact-source build receipt.
/// `cancel` owns hashing; returns the canonical CLI and its retained provenance.
fn verified_cli(cancel: &Cancellation) -> TestResult<(PathBuf, Value)> {
    let cli =
        PathBuf::from(std::env::var_os("EDITBAY_TEST_CLI").ok_or("EDITBAY_TEST_CLI is required")?)
            .canonicalize()?;
    let metadata = cli.metadata()?;
    assert!(metadata.is_file() && metadata.permissions().mode() & 0o111 != 0);
    let receipt_path = PathBuf::from(
        std::env::var_os("EDITBAY_TEST_CLI_BUILD_RECEIPT")
            .ok_or("EDITBAY_TEST_CLI_BUILD_RECEIPT is required")?,
    );
    let receipt: Value = serde_json::from_slice(&fs::read(&receipt_path)?)?;
    let git = Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
        .output()?;
    assert!(git.status.success());
    let revision = String::from_utf8(git.stdout)?;
    assert_eq!(receipt["repository"], "editbay");
    assert_eq!(receipt["revision"], revision.trim());
    assert_eq!(receipt["exit_code"], 0);
    assert_eq!(receipt["dirty_before"], false);
    assert_eq!(receipt["dirty_after"], false);
    assert!(receipt["signal"].is_null());
    assert!(
        receipt["receipt_errors"]
            .as_array()
            .ok_or("Missing executable receipt errors")?
            .is_empty()
    );
    let entry = receipt["executables"]
        .as_array()
        .ok_or("Missing executable bindings")?
        .iter()
        .find(|entry| {
            entry["target"]["name"] == "editbay"
                && entry["path"].as_str().is_some_and(|p| Path::new(p) == cli)
        })
        .ok_or("Build receipt does not identify this CLI")?;
    let actual = SourceFile::open(&cli, cancel)?;
    assert_eq!(entry["sha256"], actual.fingerprint().sha256);
    assert_eq!(entry["size"], actual.fingerprint().bytes);
    let binding = json!({"path":cli,"fingerprint":actual.fingerprint(),"build_receipt":receipt_path,"source_revision":revision.trim()});
    Ok((cli, binding))
}

/// Author exact quarter-step animation through the production shared commands.
/// `directory` retains the native input; returns reopened content and output identities.
fn authored_project(directory: &Path) -> TestResult<(Project, Uuid, Uuid)> {
    let mut editor = DocumentEditor::new(Project::new("Original native viewer scopes")?)?;
    let composition = Uuid::new_v4();
    let sequence = Uuid::new_v4();
    let node = Uuid::new_v4();
    let rate = FrameRate::new(24, 1)?;
    let animation = [AnimatedProperty::Red, AnimatedProperty::Green]
        .into_iter()
        .map(|property| AnimationChannel {
            id: Uuid::new_v4(),
            property,
            interpolation: Interpolation::Step,
            keys: (0..FRAMES)
                .map(|frame| Keyframe {
                    frame,
                    value: if property == AnimatedProperty::Red {
                        frame as f64 / 4.0
                    } else {
                        (3 - frame) as f64 / 4.0
                    },
                    in_tangent: 0.0,
                    out_tangent: 0.0,
                })
                .collect(),
        })
        .collect();
    let commands = [
        DocumentCommand::SetComposition {
            composition: Composition {
                id: composition,
                name: "Four original animated colors".into(),
                width: SIDE,
                height: SIDE,
                frame_rate: rate,
                duration: FRAMES,
                tracks: vec![],
                nodes: vec![TimedNode {
                    id: node,
                    range: FrameRange {
                        start: 0,
                        end: FRAMES,
                    },
                    operation: NodeOperation::Solid {
                        rgba: [0.0, 0.75, 0.125, 1.0],
                    },
                    animation,
                }],
                picture: Some(node),
                audio: None,
            },
        },
        DocumentCommand::SetSequence {
            sequence: Sequence {
                id: sequence,
                name: "Native viewer color program".into(),
                width: SIDE,
                height: SIDE,
                frame_rate: rate,
                composition: Some(composition),
            },
        },
        DocumentCommand::SetPrimarySequence { id: sequence },
    ];
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Author original viewer fixture".into(),
        &commands,
    )?;
    let path = directory.join("original.editbay");
    editbay_core::save_new(editor.project(), &path)?;
    let project = editbay_core::load(&path)?;
    assert_eq!(project, *editor.project());
    Ok((project, sequence, composition))
}

/// Inspect a real worker event without consuming it before the pane polls.
/// `pane`, `serial` and `deadline` identify the request; returns the actual picture.
fn wait_for_picture(
    pane: &PreviewPane,
    serial: u64,
    frame: u64,
    deadline: &Watchdog,
) -> TestResult<Arc<Picture>> {
    loop {
        deadline.check()?;
        let task = pane.task.as_ref().ok_or("Viewer task disappeared")?;
        let event = task.result.lock().unwrap();
        match event.as_ref() {
            Some(Event::Picture(picture)) if picture.serial == serial && picture.frame == frame => {
                return Ok(picture.clone());
            }
            Some(Event::Failed(error)) => return Err(error.clone().into()),
            _ => {}
        }
        assert!(
            !task.thread.as_ref().is_some_and(JoinHandle::is_finished),
            "Actual viewer worker exited without the requested picture"
        );
        drop(event);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Await only pictures actually published by the native preparation worker.
/// The pane, queue, count and deadline bind this preparation; returns without drawing.
fn wait_for_prepared(
    pane: &PreviewPane,
    pictures: &PreparedPictures,
    count: u64,
    deadline: &Watchdog,
) -> TestResult<()> {
    loop {
        deadline.check()?;
        if pictures.lock().unwrap().state().prepared == count {
            return Ok(());
        }
        let task = pane.task.as_ref().ok_or("Viewer task disappeared")?;
        if let Some(Event::Failed(error)) = task.result.lock().unwrap().as_ref() {
            return Err(error.clone().into());
        }
        assert!(!task.thread.as_ref().is_some_and(JoinHandle::is_finished));
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Check independently known working RGB and BT.709 luminance on every sample.
/// `picture`, `owner`, `composition` and `settings` bind the real worker capture.
/// Returns its raw report after checking the analytic frame color and density totals.
fn checked_report(
    picture: &Picture,
    owner: DocumentOwner,
    composition: Uuid,
    settings: ScopeSettings,
) -> TestResult<ScopeReport> {
    let report = picture
        .scopes
        .as_ref()
        .ok_or("Actual scope report absent")?;
    let identity = ScopeIdentity {
        version: owner.version,
        composition,
        frame: picture.frame,
    };
    assert!(report.matches(identity, ColorConfiguration::default(), settings));
    assert_eq!(report.dimensions, [SIDE, SIDE]);
    assert_eq!(report.sampled_pixels, u64::from(SIDE * SIDE));
    assert_eq!(report.valid_pixels, u64::from(SIDE * SIDE));
    assert_eq!(report.invalid_pixels, 0);
    assert_eq!(report.transparent_pixels, 0);
    let frame = picture.frame as f64;
    let expected = [frame * 0.25, 0.75 - frame * 0.25, 0.125];
    for (range, expected) in report.statistics.linear_rgb.iter().zip(expected) {
        let range = range.ok_or("Scope working channel absent")?;
        assert!((range.minimum - expected).abs() < 1.0e-5);
        assert!((range.maximum - expected).abs() < 1.0e-5);
    }
    let luminance = (0.2126 * expected[0] + 0.7152 * expected[1] + 0.0722 * expected[2])
        * settings.reference_white_nits;
    let actual = report
        .statistics
        .luminance_nits
        .ok_or("Scope luminance absent")?;
    assert!((actual.minimum - luminance).abs() < 0.001);
    assert!((actual.maximum - luminance).abs() < 0.001);
    for plot in std::iter::once(&report.waveform)
        .chain(report.parade.iter())
        .chain(std::iter::once(&report.vectorscope))
    {
        assert_eq!(plot.counts.iter().map(|n| u64::from(*n)).sum::<u64>(), 256);
        assert_eq!(plot.outside, 0);
    }
    assert!(
        picture
            .worker_pid
            .is_some_and(|pid| pid != std::process::id())
    );
    assert!(picture.caption_warnings.is_empty());
    Ok(report.clone())
}

/// Complete the real native display primitive on an offscreen CPU-Vulkan target.
/// `picture`, `gpu`, `log` and `deadline` retain ownership; returns after its GPU callback.
/// CompletionLog receives only the actual submission completion, never a fabricated event.
fn complete_offscreen(
    picture: Arc<Picture>,
    gpu: &Gpu,
    log: &mut CompletionLog,
    deadline: &Watchdog,
) -> TestResult<()> {
    assert!(picture.draw.accepts_device(&gpu.device));
    assert_eq!(picture.draw.dimensions(), [SIDE, SIDE]);
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Native viewer scope qualification target"),
        size: wgpu::Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: gpu.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let finished = Arc::new(AtomicBool::new(false));
    let completed = finished.clone();
    let sender = log.publisher();
    let captured = picture.clone();
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Actual native viewer display draw"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        picture.draw.paint(&mut pass, move || {
            let elapsed = captured.requested.elapsed().as_micros() as u64;
            if captured
                .completion_us
                .compare_exchange(0, elapsed, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                sender.completed(
                    captured.frame,
                    captured.serial,
                    elapsed,
                    elapsed.saturating_sub(captured.selected_us.load(Ordering::Acquire)),
                );
            }
            completed.store(true, Ordering::Release);
        });
    }
    gpu.queue.submit([encoder.finish()]);
    while !finished.load(Ordering::Acquire) {
        deadline.check()?;
        gpu.device.poll(wgpu::PollType::Poll)?;
        std::thread::sleep(Duration::from_millis(1));
    }
    log.poll();
    Ok(())
}

#[test]
#[ignore = "requires exact-current CLI receipt and explicit CPU Vulkan runtime"]
fn craft_video_native_viewer_scopes_pause_prepare_and_reject_obsolete_owner() -> TestResult<()> {
    assert_eq!(std::env::var("WGPU_BACKEND").as_deref(), Ok("vulkan"));
    assert_eq!(
        std::env::var("VK_DRIVER_FILES").as_deref(),
        Ok("/usr/share/vulkan/icd.d/lvp_icd.json")
    );
    assert!(Path::new("/usr/share/vulkan/icd.d/lvp_icd.json").is_file());
    assert_eq!(std::env::var("LP_NUM_THREADS").as_deref(), Ok("2"));
    assert!(std::env::temp_dir().canonicalize()?.starts_with("/home"));
    let directory = EvidenceDirectory::new()?;
    let cancel = Cancellation::new()?;
    let deadline = Watchdog::new(cancel.clone())?;
    let (cli, cli_binding) = verified_cli(&cancel)?;
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let (gpu, adapter_info) = runtime.block_on(async {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                force_fallback_adapter: true,
                ..Default::default()
            })
            .await?;
        let info = adapter.get_info();
        assert_eq!(info.device_type, wgpu::DeviceType::Cpu);
        assert_eq!(info.backend, wgpu::Backend::Vulkan);
        let name = info.name.to_ascii_lowercase();
        assert!(name.contains("llvmpipe") || name.contains("lavapipe"));
        let (device, queue) = adapter.request_device(&Default::default()).await?;
        Ok::<_, Box<dyn std::error::Error>>((
            Gpu {
                adapter,
                device,
                queue,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
            },
            format!("{info:?}"),
        ))
    })?;
    deadline.check()?;
    let (project, sequence, composition) = authored_project(&directory.path)?;
    assert_eq!(project.color, ColorConfiguration::default());
    let ctx = egui::Context::default();
    let mut workspace = Workspace::new(directory.path.join("recovery"), ctx.clone());
    let tab = workspace.create(project)?;
    let (owner, editor) = workspace.edit_snapshot(tab)?;
    let settings = ScopeSettings {
        columns: 8,
        rows: 16,
        vector_size: 16,
        maximum_samples: 256,
        scale: ScopeScale::Linear,
        ..Default::default()
    };
    let mut viewer = OwnedViewer {
        pane: PreviewPane {
            view_owner: Some(owner),
            gpu: Some(gpu.clone()),
            serial: 1,
            requested_frame: Some(0),
            ..Default::default()
        },
    };
    viewer.pane.color.enabled = true;
    viewer.pane.color.settings = settings;
    viewer.pane.task = Some(Task::start(
        owner,
        editor.snapshot(),
        sequence,
        gpu.clone(),
        ctx.clone(),
        RenderOptions {
            executable: cli.clone(),
            store: None,
            scopes: Some(settings),
        },
    )?);
    deadline.register(viewer.pane.task.as_ref().unwrap())?;
    viewer.pane.task.as_ref().unwrap().request(Request {
        serial: 1,
        frame: 0,
        requested: Instant::now(),
    })?;
    let initial = wait_for_picture(&viewer.pane, 1, 0, &deadline)?;
    let paused_before = checked_report(&initial, owner, composition, settings)?;
    let native_pid = initial.worker_pid.unwrap();
    viewer.pane.poll(&mut workspace, true, &ctx);
    assert!(viewer.pane.error.is_none());
    assert!(Arc::ptr_eq(viewer.pane.picture.as_ref().unwrap(), &initial));
    let changed_settings = ScopeSettings {
        scale: ScopeScale::Nits,
        reference_white_nits: 200.0,
        ..settings
    };
    viewer.pane.color.settings = changed_settings;
    viewer.pane.refresh_scopes()?;
    assert!(viewer.pane.requested_frame.is_none());
    viewer.pane.serial = 2;
    viewer.pane.requested_frame = Some(0);
    viewer.pane.task.as_ref().unwrap().request(Request {
        serial: 2,
        frame: 0,
        requested: Instant::now(),
    })?;
    let refreshed = wait_for_picture(&viewer.pane, 2, 0, &deadline)?;
    let paused_after = checked_report(&refreshed, owner, composition, changed_settings)?;
    assert!(!paused_before.matches(paused_after.identity, paused_after.color, changed_settings));
    assert_eq!(refreshed.worker_pid, Some(native_pid));
    viewer.pane.poll(&mut workspace, true, &ctx);
    assert!(Arc::ptr_eq(
        viewer.pane.picture.as_ref().unwrap(),
        &refreshed
    ));
    assert!(!viewer.pane.task.as_ref().unwrap().cancel.is_cancelled());

    viewer.pane.task.as_ref().unwrap().request(Request {
        serial: 1,
        frame: 0,
        requested: Instant::now(),
    })?;
    let obsolete_serial_picture = wait_for_picture(&viewer.pane, 1, 0, &deadline)?;
    assert_eq!(obsolete_serial_picture.worker_pid, Some(native_pid));
    let rejected_before = viewer.pane.rejected;
    viewer.pane.poll(&mut workspace, true, &ctx);
    assert_eq!(viewer.pane.rejected, rejected_before + 1);
    assert!(Arc::ptr_eq(
        viewer.pane.picture.as_ref().unwrap(),
        &refreshed
    ));

    let pictures = Arc::new(Mutex::new(Prepared::new(0, FRAMES, SIDE, SIDE)?));
    viewer.pane.picture = None;
    viewer.pane.serial = 3;
    viewer.pane.requested_frame = None;
    viewer.pane.prepared = Some(pictures.clone());
    viewer.pane.completions = Some(CompletionLog::new(
        completion::Scope {
            tab,
            version: owner.version,
            sequence,
        },
        0,
        FRAMES,
    )?);
    viewer.pane.task.as_ref().unwrap().prepare(Preparation {
        serial: 3,
        pictures: pictures.clone(),
    })?;
    wait_for_prepared(&viewer.pane, &pictures, FRAMES, &deadline)?;
    let prepared_state = json!(pictures.lock().unwrap().state());
    assert_eq!(pictures.lock().unwrap().state().queued, FRAMES as usize);
    let prepared_session = pictures.lock().unwrap().state().session;
    let refreshed_settings = ScopeSettings {
        scale: ScopeScale::Linear,
        reference_white_nits: 203.0,
        ..changed_settings
    };
    let mut playback_reports = Vec::new();
    for frame in 0..FRAMES {
        viewer.pane.advance_picture(frame)?;
        let queued = viewer.pane.picture.as_ref().unwrap().clone();
        assert_eq!(queued.frame, frame);
        assert_eq!(queued.serial, 3);
        let queued_report = checked_report(&queued, owner, composition, changed_settings)?;
        if frame == 0 {
            viewer.pane.color.settings = refreshed_settings;
            viewer.pane.refresh_scopes()?;
        }
        assert_eq!(viewer.pane.serial, 3);
        assert_eq!(viewer.pane.requested_frame, Some(frame));
        let refreshed_picture = wait_for_picture(&viewer.pane, 3, frame, &deadline)?;
        let report = checked_report(&refreshed_picture, owner, composition, refreshed_settings)?;
        assert!(!queued_report.matches(report.identity, report.color, refreshed_settings));
        viewer.pane.poll(&mut workspace, true, &ctx);
        assert!(viewer.pane.error.is_none());
        let picture = viewer.pane.picture.as_ref().unwrap().clone();
        assert!(Arc::ptr_eq(&picture, &refreshed_picture));
        assert_eq!(picture.frame, frame);
        assert_eq!(picture.serial, 3);
        assert_eq!(picture.worker_pid, Some(native_pid));
        assert!(!viewer.pane.task.as_ref().unwrap().cancel.is_cancelled());
        assert!(picture.selected_us.load(Ordering::Acquire) > 0);
        let queue = pictures.lock().unwrap().state();
        assert_eq!(queue.session, prepared_session);
        assert_eq!(queue.prepared, FRAMES);
        assert_eq!(queue.queued, (FRAMES - frame - 1) as usize);
        assert!(queue.open);
        playback_reports.push(
            json!({"frame":frame,"queued_report":queued_report,"accepted_refreshed_report":report}),
        );
        complete_offscreen(
            picture,
            &gpu,
            viewer.pane.completions.as_mut().unwrap(),
            &deadline,
        )?;
    }
    assert_eq!(viewer.pane.skipped_frames, 0);
    let completions = viewer.pane.completions.as_ref().unwrap();
    let summary = completions.summary();
    assert_eq!(summary.received, FRAMES);
    assert_eq!(summary.unique_completed, FRAMES);
    assert_eq!(summary.repeated, 0);
    assert_eq!(summary.rejected, 0);
    assert_eq!(summary.overflow, 0);
    assert!(!summary.cancelled);
    assert!(summary.observed_sound_end_us.is_none());
    let (_, events) = completions.events();
    assert_eq!(
        events.iter().map(|event| event.frame).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert!(events.iter().all(|event| event.request_serial == 3));
    let completion_evidence = json!({"summary":summary,"events":events});

    let initially_unscoped = Arc::new(Mutex::new(Prepared::new(0, 1, SIDE, SIDE)?));
    viewer.pane.picture = None;
    viewer.pane.requested_frame = None;
    viewer.pane.serial = 4;
    viewer.pane.prepared = Some(initially_unscoped.clone());
    viewer.pane.completions = Some(CompletionLog::new(
        completion::Scope {
            tab,
            version: owner.version,
            sequence,
        },
        0,
        1,
    )?);
    viewer.pane.color.enabled = false;
    viewer.pane.refresh_scopes()?;
    viewer.pane.task.as_ref().unwrap().prepare(Preparation {
        serial: 4,
        pictures: initially_unscoped.clone(),
    })?;
    wait_for_prepared(&viewer.pane, &initially_unscoped, 1, &deadline)?;
    viewer.pane.color.enabled = true;
    viewer.pane.refresh_scopes()?;
    assert!(viewer.pane.picture.is_none());
    viewer.pane.advance_picture(0)?;
    assert!(viewer.pane.picture.as_ref().unwrap().scopes.is_none());
    let first_scoped = wait_for_picture(&viewer.pane, 4, 0, &deadline)?;
    let first_scoped_report =
        checked_report(&first_scoped, owner, composition, refreshed_settings)?;
    viewer.pane.poll(&mut workspace, true, &ctx);
    assert!(Arc::ptr_eq(
        viewer.pane.picture.as_ref().unwrap(),
        &first_scoped
    ));
    assert_eq!(first_scoped.worker_pid, Some(native_pid));
    assert_eq!(viewer.pane.serial, 4);
    assert_eq!(initially_unscoped.lock().unwrap().state().prepared, 1);
    complete_offscreen(
        first_scoped,
        &gpu,
        viewer.pane.completions.as_mut().unwrap(),
        &deadline,
    )?;
    assert_eq!(
        viewer
            .pane
            .completions
            .as_ref()
            .unwrap()
            .summary()
            .unique_completed,
        1
    );

    viewer.pane.task.as_ref().unwrap().request(Request {
        serial: 4,
        frame: 0,
        requested: Instant::now(),
    })?;
    let obsolete_owner_picture = wait_for_picture(&viewer.pane, 4, 0, &deadline)?;
    assert_eq!(obsolete_owner_picture.serial, viewer.pane.serial);
    assert_eq!(
        Some(obsolete_owner_picture.frame),
        viewer.pane.requested_frame
    );
    assert_eq!(obsolete_owner_picture.worker_pid, Some(native_pid));
    workspace.apply(
        tab,
        owner.version,
        "Invalidate actual viewer owner".into(),
        &[DocumentCommand::RenameProject {
            name: "Changed while the native picture was queued".into(),
        }],
    )?;
    assert!(!workspace.owns(owner));
    let task_cancel = viewer.pane.task.as_ref().unwrap().cancel.clone();
    viewer.pane.poll(&mut workspace, true, &ctx);
    assert!(viewer.pane.task.is_none());
    assert!(viewer.pane.picture.is_none());
    assert!(viewer.pane.prepared.is_none());
    assert!(task_cancel.is_cancelled());
    assert!(!initially_unscoped.lock().unwrap().state().open);
    assert!(
        viewer
            .pane
            .completions
            .as_ref()
            .unwrap()
            .summary()
            .cancelled
    );
    assert_ne!(viewer.pane.view_owner, Some(owner));
    assert!(viewer.pane.sound.is_none() && viewer.pane.starting.is_none());
    while !viewer.pane.retiring.is_empty() {
        deadline.check()?;
        viewer.pane.poll(&mut workspace, true, &ctx);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(viewer.pane.error.is_none());
    let final_cli = SourceFile::open(&cli, &cancel)?;
    assert_eq!(
        cli_binding["fingerprint"]["sha256"],
        final_cli.fingerprint().sha256
    );
    assert_eq!(
        cli_binding["fingerprint"]["bytes"],
        final_cli.fingerprint().bytes
    );
    final_cli.verify(&cancel)?;
    deadline.check()?;
    let receipt = json!({
        "schema":"editbay.native-viewer-scopes.v1","status":"passed",
        "adapter":adapter_info,"cli":cli_binding,"worker_pid":native_pid,
        "native_project":directory.path.join("original.editbay"),"owner":{"tab":owner.tab,"version":owner.version},
        "composition":composition,"sequence":sequence,"dimensions":[SIDE,SIDE],
        "paused_before":paused_before,"paused_same_frame_new_settings":paused_after,
        "prepared_queue":prepared_state,"prepared_frames":playback_reports,"display_completions":completion_evidence,
        "first_picture_enabled_after_unscoped_preparation":{"old_report":null,"accepted_report":first_scoped_report},
        "stale_serial_rejections":viewer.pane.rejected-rejected_before,
        "stale_owner":{"queued_serial":obsolete_owner_picture.serial,"queued_frame":obsolete_owner_picture.frame,"discarded":true,"cancelled":task_cancel.is_cancelled()},
        "worker_joined":true,"elapsed_ms":deadline.began.elapsed().as_millis(),
        "oracle":"frame f has working RGB [f/4,3/4-f/4,1/8], alpha1; BT709 Y=.2126R+.7152G+.0722B; nits=Y*explicit reference white",
        "proof_boundary":"Actual native Task/PictureWorker CLI, CPU lavapipe Vulkan, working-image scope readback, same-transport paused settings refresh, full Prepared queue same-frame settings refresh and continued exact frame selection. CompletionLog records actual DisplayFrame offscreen submission callbacks. No window, audio output, sound clock, physical display, visible UI controls, device or hardware acceptance is claimed."
    });
    let path = directory.path.join("viewer-scopes.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(&serde_json::to_vec_pretty(&receipt)?)?;
    file.sync_all()?;
    deadline.check()?;
    eprintln!("Actual native viewer scope evidence: {}", path.display());
    Ok(())
}
