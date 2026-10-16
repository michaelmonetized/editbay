use super::*;
use editbay_core::{DocumentCommand, DocumentVersion, IccProfile, WorkingGamut};
use editbay_media::{SourceFile, StreamType};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[path = "../../editbay-media/tests/fixtures/icc.rs"]
mod fixture;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const WAIT: Duration = Duration::from_secs(5);

struct Directory {
    path: PathBuf,
    retained: bool,
    _temporary: Option<tempfile::TempDir>,
}

impl Directory {
    /// Allocate only test-generated media and native documents beneath /home.
    /// `case` identifies this worker route; returns uniquely owned fixture storage.
    fn new(case: &str) -> TestResult<Self> {
        let requested = std::env::var_os("EDITBAY_WORKFLOW_RECEIPTS_DIR").map(PathBuf::from);
        let root = requested
            .clone()
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts"));
        fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        assert!(root.starts_with("/home"));
        if requested.is_some() {
            let path = root.join(format!("source-color-{case}-{}", Uuid::new_v4()));
            fs::create_dir(&path)?;
            Ok(Self {
                path,
                retained: true,
                _temporary: None,
            })
        } else {
            let temporary = tempfile::Builder::new()
                .prefix("source-color-")
                .tempdir_in(root)?;
            Ok(Self {
                path: temporary.path().into(),
                retained: false,
                _temporary: Some(temporary),
            })
        }
    }
}

struct Harness {
    workspace: Workspace,
    tab: Uuid,
    selection: (Uuid, u32),
    profile: IccProfile,
    profile_path: PathBuf,
    input_path: PathBuf,
    input_fingerprint: editbay_media::SourceFingerprint,
    directory: Directory,
}

impl Harness {
    /// Seed a real workspace from the original generated PNG and native source ingestion.
    /// `case` owns storage; returns a video source cleared by a shared document command.
    fn new(case: &str) -> TestResult<Self> {
        let directory = Directory::new(case)?;
        let bytes = fixture::original_profile()?;
        let profile = IccProfile::new(bytes.clone())?;
        let profile_path = directory.path.join("original.icc");
        let input_path = directory.path.join("original-profiled-rgb16.png");
        write_new(&profile_path, &bytes)?;
        write_new(&input_path, &fixture::original_png(&bytes)?)?;
        let cancel = Cancellation::new()?;
        let source = SourceFile::open(&input_path, &cancel)?;
        let probe = source.probe(cancel.clone())?;
        let video = probe
            .streams
            .iter()
            .find(|stream| stream.kind == StreamType::Video)
            .ok_or("Native PNG video absent")?;
        let ingested = source.ingest(
            "Original ICC pane source".into(),
            &[video.index],
            cancel.clone(),
            |_, _| {},
        )?;
        let captured = ingested
            .source
            .streams
            .iter()
            .find(|stream| stream.index == video.index)
            .ok_or("Ingested PNG stream absent")?;
        assert_eq!(captured.icc_profile.as_ref(), Some(&profile));
        let selection = (ingested.source.id, captured.index);
        let mut workspace =
            Workspace::new(directory.path.join("recovery"), egui::Context::default());
        let tab = workspace.create(Project::new("ICC worker owner")?)?;
        workspace.apply(
            tab,
            DocumentVersion::of(project(&workspace, tab)?),
            "Native PNG ingest".into(),
            &ingested.commands(),
        )?;
        let clear = editbay_core::source_icc_command(
            project(&workspace, tab)?,
            selection.0,
            selection.1,
            None,
        )?;
        workspace.apply(
            tab,
            DocumentVersion::of(project(&workspace, tab)?),
            "Clear captured ICC for explicit assignment".into(),
            &[clear],
        )?;
        assert!(profile_in(&workspace, tab, selection)?.is_none());
        source.verify(&cancel)?;
        let input_fingerprint = source.fingerprint().clone();
        Ok(Self {
            workspace,
            tab,
            selection,
            profile,
            profile_path,
            input_path,
            input_fingerprint,
            directory,
        })
    }

    /// Retain actual input identities and completed pane assertions only on request.
    /// `case` and `evidence` describe the completed route; returns synchronized receipt status.
    fn passed(&self, case: &str, evidence: Value) -> TestResult<()> {
        if self.directory.retained {
            let (fixture_midpoint, fixture_allowance) =
                fixture::expected([32768.0 / 65535.0; 3], WorkingGamut::Bt709);
            write_new(
                &self.directory.path.join("source-color-pane.json"),
                &serde_json::to_vec_pretty(&json!({
                    "schema":"editbay.source-color-pane.v1","status":"passed","case":case,"evidence":evidence,
                    "input":self.input_path,"input_fingerprint":self.input_fingerprint,"profile":self.profile_path,
                    "profile_sha256":self.profile.sha256,"profile_bytes":self.profile.bytes.len(),
                    "fixture_generator_sha256":format!("{:x}",Sha256::digest(include_bytes!("../../editbay-media/tests/fixtures/icc.rs"))),
                    "format_references":[fixture::ICC_SPEC,fixture::PNG_SPEC],
                    "fixture_midpoint_oracle":{"linear_rgb":fixture_midpoint,"allowance":fixture_allowance,"boundary":"Fixture identity only; pane tests do not measure rendered color"},
                    "proof_boundary":"Actual SourceColorPane::start filesystem/native-profile worker, native PNG ingestion and shared Workspace commands/save/open only. Completion is observed through that worker's fresh egui repaint callback after its real result is queued. The live-worker case holds only this completion callback; it does not claim cancellation inside LCMS computation. No replacement result, synthesized Task, native window, display, GPU or audio device is used."
                }))?,
            )?;
        }
        Ok(())
    }
}

/// Write only new files inside this case's uniquely owned generated storage.
/// `path` and `bytes` supply output; returns after synchronization without replacement.
fn write_new(path: &Path, bytes: &[u8]) -> TestResult<()> {
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Borrow one actual workspace document without constructing a substitute model.
/// `workspace` and `tab` select the native session; returns its current project.
fn project(workspace: &Workspace, tab: Uuid) -> TestResult<&Project> {
    Ok(workspace
        .tabs
        .iter()
        .find(|item| item.id == tab)
        .ok_or("Workspace tab absent")?
        .editor
        .project())
}

/// Inspect the exact selected stream's retained profile in the actual workspace.
/// `workspace`, `tab` and `selection` name its owner; returns the saved interpretation.
fn profile_in(
    workspace: &Workspace,
    tab: Uuid,
    selection: (Uuid, u32),
) -> TestResult<Option<&IccProfile>> {
    Ok(project(workspace, tab)?
        .sources
        .iter()
        .find(|source| source.id == selection.0)
        .and_then(|source| {
            source
                .streams
                .iter()
                .find(|stream| stream.index == selection.1)
        })
        .ok_or("Selected native source stream absent")?
        .icc_profile
        .as_ref())
}

struct CompletionHook {
    entered: mpsc::Sender<String>,
    release: Mutex<mpsc::Receiver<()>>,
    dropped: mpsc::Sender<()>,
    hold: bool,
    timed_out: Arc<AtomicBool>,
}

impl CompletionHook {
    /// Observe the actual named worker after its channel send, optionally holding its lifetime.
    /// Takes this hook; returns at release or the finite deadline without consuming the result.
    fn arrive(&self) {
        let _ = self
            .entered
            .send(std::thread::current().name().unwrap_or("unnamed").into());
        if self.hold && self.release.lock().unwrap().recv_timeout(WAIT).is_err() {
            self.timed_out.store(true, Ordering::Release);
        }
    }
}

impl Drop for CompletionHook {
    fn drop(&mut self) {
        let _ = self.dropped.send(());
    }
}

struct WorkerGate {
    entered: mpsc::Receiver<String>,
    done: mpsc::Receiver<()>,
    release: Option<mpsc::Sender<()>>,
    cancel: Cancellation,
    timed_out: Arc<AtomicBool>,
    finished: bool,
}

impl WorkerGate {
    /// Start the pane's real native worker with a fresh completion-only egui context.
    /// `pane`, `harness`, `path` and `hold` select work/lifetime; returns owned bounded cleanup.
    fn start(
        pane: &mut SourceColorPane,
        harness: &Harness,
        path: &Path,
        hold: bool,
    ) -> TestResult<Self> {
        let (entered_tx, entered) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (dropped, done) = mpsc::channel();
        let timed_out = Arc::new(AtomicBool::new(false));
        let hook = CompletionHook {
            entered: entered_tx,
            release: Mutex::new(release_rx),
            dropped,
            hold,
            timed_out: timed_out.clone(),
        };
        let ctx = egui::Context::default();
        ctx.set_request_repaint_callback(move |_| hook.arrive());
        pane.selection = Some(harness.selection);
        pane.path = path.to_str().ok_or("Fixture path must be UTF-8")?.into();
        pane.start(&harness.workspace, harness.tab, &ctx, false);
        let task = pane.task.as_ref().ok_or_else(|| {
            pane.status
                .clone()
                .unwrap_or_else(|| "Pane worker did not start".into())
        })?;
        let cancel = task.cancel.clone();
        assert_eq!(task.owner, harness.workspace.edit_snapshot(harness.tab)?.0);
        assert_eq!(task.activation, harness.workspace.activation_generation);
        drop(ctx);
        Ok(Self {
            entered,
            done,
            release: Some(release),
            cancel,
            timed_out,
            finished: false,
        })
    }

    /// Confirm the real worker has queued its untouched result.
    /// Takes this gate; returns after the native worker's actual repaint callback begins.
    fn queued(&self) -> TestResult<()> {
        let name = self.entered.recv_timeout(WAIT)?;
        assert_eq!(name, "editbay-source-color");
        Ok(())
    }

    /// Release a held callback and await destruction of its worker-owned final context.
    /// Takes this gate; returns after owned callback cleanup, without touching Task.result.
    fn finish(&mut self) -> TestResult<()> {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        self.done.recv_timeout(WAIT)?;
        self.finished = true;
        assert!(
            !self.timed_out.load(Ordering::Acquire),
            "Real worker completion callback exceeded its bounded gate"
        );
        Ok(())
    }
}

impl Drop for WorkerGate {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if !self.finished {
            let _ = self.done.recv_timeout(WAIT);
        }
    }
}

/// Poll only the workspace's actual bounded file workers until its current queue settles.
/// `workspace` owns these jobs; returns within five seconds without scheduling idle recovery.
fn settle_workspace(workspace: &mut Workspace) -> TestResult<()> {
    let began = Instant::now();
    let poll_time = began;
    while workspace.busy() {
        if began.elapsed() >= WAIT {
            return Err("Workspace file jobs exceeded five seconds".into());
        }
        workspace.poll(poll_time);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        workspace.errors.is_empty(),
        "Workspace file errors: {:?}",
        workspace.errors
    );
    Ok(())
}

#[test]
fn craft_video_icc_source_pane_worker_assigns_noops_undoes_and_saves_reopens() -> TestResult<()> {
    let mut harness = Harness::new("assign")?;
    let mut pane = SourceColorPane::default();
    let baseline = DocumentVersion::of(project(&harness.workspace, harness.tab)?);
    let mut worker = WorkerGate::start(&mut pane, &harness, &harness.profile_path, false)?;
    worker.queued()?;
    worker.finish()?;
    pane.poll(&mut harness.workspace, true);
    assert!(pane.task.is_none());
    assert!(
        pane.status
            .as_deref()
            .is_some_and(|status| status.starts_with("Source profile applied."))
    );
    assert_eq!(
        profile_in(&harness.workspace, harness.tab, harness.selection)?,
        Some(&harness.profile)
    );
    let assigned = DocumentVersion::of(project(&harness.workspace, harness.tab)?);
    assert_eq!(assigned.revision, baseline.revision + 1);
    let mut noop = WorkerGate::start(&mut pane, &harness, &harness.profile_path, false)?;
    noop.queued()?;
    noop.finish()?;
    pane.poll(&mut harness.workspace, true);
    assert!(pane.task.is_none());
    assert!(
        pane.status
            .as_deref()
            .is_some_and(|status| status.starts_with("Source profile applied."))
    );
    assert_eq!(
        DocumentVersion::of(project(&harness.workspace, harness.tab)?),
        assigned
    );
    harness.workspace.history(harness.tab, false)?;
    assert!(
        profile_in(&harness.workspace, harness.tab, harness.selection)?.is_none(),
        "No-op assignment added an undo step"
    );
    harness.workspace.history(harness.tab, true)?;
    assert_eq!(
        profile_in(&harness.workspace, harness.tab, harness.selection)?,
        Some(&harness.profile)
    );
    let native = harness.directory.path.join("assigned-source.editbay");
    let saved = project(&harness.workspace, harness.tab)?.clone();
    harness
        .workspace
        .save(harness.tab, DocumentVersion::of(&saved), native.clone())?;
    settle_workspace(&mut harness.workspace)?;
    assert_eq!(editbay_core::load(&native)?, saved);
    assert!(
        !harness
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == harness.tab)
            .unwrap()
            .dirty()
    );
    harness.workspace.close(harness.tab, false)?;
    harness.workspace.open([native.clone()]);
    settle_workspace(&mut harness.workspace)?;
    let reopened = harness.workspace.active.ok_or("Reopened tab absent")?;
    assert_eq!(*project(&harness.workspace, reopened)?, saved);
    assert_eq!(
        profile_in(&harness.workspace, reopened, harness.selection)?,
        Some(&harness.profile)
    );
    harness.passed("assign_noop_undo_save_reopen", json!({"baseline":baseline,"assigned":assigned,"native_project":native,"reopened_tab":reopened,"status":pane.status,"noop_revision_unchanged":true,"one_undo_removes_assignment":true,"save_reopen_equal":true}))?;
    Ok(())
}

#[test]
fn craft_video_icc_source_pane_invalid_profile_preserves_document_and_recovers() -> TestResult<()> {
    let mut harness = Harness::new("invalid")?;
    let mut pane = SourceColorPane::default();
    let invalid = harness.directory.path.join("invalid.icc");
    write_new(&invalid, b"Original deliberately malformed ICC fixture")?;
    let baseline = project(&harness.workspace, harness.tab)?.clone();
    let mut worker = WorkerGate::start(&mut pane, &harness, &invalid, false)?;
    worker.queued()?;
    worker.finish()?;
    pane.poll(&mut harness.workspace, true);
    assert!(pane.task.is_none());
    assert_eq!(*project(&harness.workspace, harness.tab)?, baseline);
    assert!(
        pane.status
            .as_deref()
            .is_some_and(|status| status.contains("ICC profile") && !status.contains("applied"))
    );
    let invalid_status = pane.status.clone();
    let mut retry = WorkerGate::start(&mut pane, &harness, &harness.profile_path, false)?;
    retry.queued()?;
    retry.finish()?;
    pane.poll(&mut harness.workspace, true);
    assert!(pane.task.is_none());
    assert_eq!(
        profile_in(&harness.workspace, harness.tab, harness.selection)?,
        Some(&harness.profile)
    );
    assert_eq!(
        project(&harness.workspace, harness.tab)?.revision,
        baseline.revision + 1
    );
    harness.passed("invalid_profile_then_retry", json!({"invalid_input":invalid,"invalid_status":invalid_status,"invalid_document_unchanged":true,"retry_status":pane.status,"retry_profile_equal":true}))?;
    Ok(())
}

/// Change a real revision or activate another native file then return to the original tab.
/// `harness`, `activation` and `owner` name the change; returns its exact retained document.
fn change_owner(
    harness: &mut Harness,
    activation: bool,
    owner: DocumentOwner,
) -> TestResult<Project> {
    if activation {
        let other = harness.directory.path.join("other-tab.editbay");
        editbay_core::save_new(&Project::new("Other native tab")?, &other)?;
        let before = harness.workspace.activation_generation;
        harness.workspace.open([other]);
        settle_workspace(&mut harness.workspace)?;
        assert_ne!(harness.workspace.active, Some(harness.tab));
        assert_ne!(harness.workspace.activation_generation, before);
        harness.workspace.active = Some(harness.tab);
        assert!(
            harness.workspace.owns(owner),
            "Tab activation must preserve document revision ownership"
        );
    } else {
        harness.workspace.apply(
            harness.tab,
            owner.version,
            "Newer user revision".into(),
            &[DocumentCommand::RenameProject {
                name: "Newer user revision".into(),
            }],
        )?;
        assert!(!harness.workspace.owns(owner));
    }
    Ok(project(&harness.workspace, harness.tab)?.clone())
}

/// Exercise one untouched real worker result against both shared owner invalidation routes.
/// `hold` keeps its actual completion callback live; returns after finite owned cleanup.
fn reject_changed_owners(hold: bool) -> TestResult<()> {
    for activation in [false, true] {
        let case = format!(
            "{}-{}",
            if hold { "live" } else { "completed" },
            if activation { "activation" } else { "revision" }
        );
        let mut harness = Harness::new(&case)?;
        let mut pane = SourceColorPane::default();
        let mut worker = WorkerGate::start(&mut pane, &harness, &harness.profile_path, hold)?;
        let owner = pane.task.as_ref().unwrap().owner;
        worker.queued()?;
        if !hold {
            worker.finish()?;
        }
        let expected = change_owner(&mut harness, activation, owner)?;
        pane.poll(&mut harness.workspace, true);
        assert!(pane.task.is_none());
        assert!(worker.cancel.is_cancelled());
        assert_eq!(*project(&harness.workspace, harness.tab)?, expected);
        assert!(profile_in(&harness.workspace, harness.tab, harness.selection)?.is_none());
        assert!(
            pane.status
                .as_deref()
                .is_some_and(|status| status.contains("discarded"))
        );
        if hold {
            worker.finish()?;
        }
        harness.passed(&case, json!({"owner_before":owner.version,"result":"actual native worker result untouched until pane.poll","worker_callback_held_during_invalidation":hold,"activation_changed":activation,"current_revision":expected.revision,"status":pane.status,"newer_document_preserved":true,"worker_context_released":true,"cancellation_inside_lcms":"not proven"}))?;
    }
    Ok(())
}

#[test]
fn craft_video_icc_source_pane_completed_workers_reject_revision_and_activation_changes()
-> TestResult<()> {
    reject_changed_owners(false)
}

#[test]
fn craft_video_icc_source_pane_live_workers_reject_revision_and_activation_changes()
-> TestResult<()> {
    reject_changed_owners(true)
}
