use crate::{
    DeliveryRequest, Phase, Progress, Receipt, Result,
    worker::{Operation, Outcome, Owner, Request, Response},
};
use editbay_audio::{SoundRenderBudget, SoundRenderer};
use editbay_core::{DocumentVersion, EvaluationSnapshot, Project, SoundBudget, SoundSnapshot};
use editbay_media::{
    Cancellation, LosslessMovProfile, PcmBudget, SourceFile, native_job::NativeJob,
};
use std::{
    fs::File,
    os::fd::AsRawFd,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};
use uuid::Uuid;

#[derive(Clone)]
pub struct DeliveryControl {
    state: Arc<AtomicU8>,
    started: Arc<AtomicBool>,
    cancel: Cancellation,
}

impl DeliveryControl {
    /// Create a fresh cancellation/publication lifetime.
    /// Takes no arguments; returns control shared by UI and the preparation thread.
    pub fn new() -> Result<Self> {
        Ok(Self {
            state: Arc::new(AtomicU8::new(0)),
            started: Arc::new(AtomicBool::new(false)),
            cancel: Cancellation::new()?,
        })
    }

    /// Cancel before the atomic publication phase without waiting for native work.
    /// Takes no arguments; returns false after publication has begun. An accepted
    /// cancellation prevents any destination publication by this lifetime.
    pub fn cancel(&self) -> bool {
        match self
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(1) => {
                self.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    /// Inspect whether this lifetime can still accept cancellation.
    /// Takes no arguments; returns false once cancellation or publication begins.
    pub fn cancellable(&self) -> bool {
        self.state.load(Ordering::Acquire) == 0
    }

    fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(editbay_media::Error::Cancelled.into());
        }
        Ok(())
    }
}

struct Lifetime<'a>(&'a DeliveryControl);
impl Drop for Lifetime<'_> {
    fn drop(&mut self) {
        let _ = self
            .0
            .state
            .compare_exchange(0, 4, Ordering::AcqRel, Ordering::Acquire);
    }
}

/// Render, independently decode-check and publish one new sequence master.
/// `executable` hosts --delivery-worker; `project` and `request` capture content;
/// `destination` is a new MOV, `control` owns cancellation, and `progress` receives
/// bounded step counters/PID. Run off the UI thread. Returns a verified receipt;
/// errors/cancellation remove the unpublished temporary and reap the child.
pub fn deliver(
    executable: &Path,
    project: Arc<Project>,
    request: DeliveryRequest,
    destination: &Path,
    control: &DeliveryControl,
    mut progress: impl FnMut(&Progress, u32),
) -> Result<Receipt> {
    if control.started.swap(true, Ordering::AcqRel) {
        return Err("Delivery control already owns a job".into());
    }
    let _lifetime = Lifetime(control);
    control.check()?;
    if editbay_core::timeline_clips(&project, request.composition)
        .is_ok_and(|clips| clips.is_empty())
    {
        return Err("The cut is empty; append a source range before exporting".into());
    }
    let evaluation = Arc::new(EvaluationSnapshot::new(project.clone())?);
    let sound = SoundSnapshot::at_output_rate(
        evaluation,
        request.composition,
        request.sample_rate,
        SoundBudget::default(),
    )?;
    let composition = project
        .compositions
        .iter()
        .find(|c| c.id == request.composition)
        .ok_or("Delivery composition is absent")?;
    let range = request.frame_range(composition.duration)?;
    let expected = LosslessMovProfile {
        width: composition.width,
        height: composition.height,
        frame_rate: composition.frame_rate,
        first_frame: range.start,
        frames: range.end - range.start,
        sample_rate: request.sample_rate,
        channels: sound.profile().channels.clone(),
    };
    request.format.validate(&expected)?;
    let preparation = SoundRenderer::new(
        Arc::new(sound),
        PcmBudget::default(),
        SoundRenderBudget::default(),
        control.cancel.clone(),
    )?
    .source_preparation(
        expected.sample_origin()?,
        expected.samples_through(expected.frames)?,
    )?
    .progress()
    .total_samples;
    if destination
        .extension()
        .and_then(|s| s.to_str())
        .is_none_or(|s| !s.eq_ignore_ascii_case(request.format.extension()))
    {
        return Err(format!(
            "This export profile needs a .{} destination",
            request.format.extension()
        )
        .into());
    }
    match std::fs::symlink_metadata(destination) {
        Ok(_) => return Err("Delivery destination already exists".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let directory = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()?;
    let name = destination.file_name().ok_or("Delivery has no filename")?;
    let destination = directory.join(name);
    let directory_file = File::open(&directory)?;
    let temporary = private_file(&directory_file)?;
    let owner = Owner {
        job: Uuid::new_v4(),
        version: DocumentVersion::of(&project),
    };
    let mut child = NativeJob::<Response>::start(executable, "--delivery-worker")?;
    let pid = child.process_id();
    progress(
        &Progress {
            phase: Phase::Preparing,
            prepared_samples: 0,
            total_preparation_samples: preparation,
            pictures: 0,
            samples: 0,
            total_pictures: 0,
            total_samples: 0,
        },
        pid,
    );
    let mut serial = 0;
    let format = request.format;
    let mut previous = None;
    let mut operation = Operation::Begin {
        project: Box::new((*project).clone()),
        request,
    };
    let receipt = loop {
        let request = Request {
            owner,
            serial,
            operation,
        };
        let response = child.request(
            &request,
            (serial == 0).then_some(&temporary),
            &control.cancel,
        )?;
        validate_reply(
            &response,
            owner,
            serial,
            &expected,
            preparation,
            previous.as_ref(),
        )?;
        match response.outcome {
            Outcome::Failed { message } => return Err(message.into()),
            Outcome::Progress {
                progress: status,
                receipt,
            } => {
                control.check()?;
                previous = Some(status.clone());
                if status.phase == Phase::Complete {
                    let receipt = receipt.ok_or("Complete delivery has no verification receipt")?;
                    if receipt.version != owner.version || receipt.format != format {
                        return Err("Delivery receipt belongs to another document".into());
                    }
                    progress(
                        &Progress {
                            phase: Phase::VerifyingFile,
                            ..status
                        },
                        pid,
                    );
                    break *receipt;
                }
                progress(&status, pid);
                if receipt.is_some() {
                    return Err("Delivery returned an early publication receipt".into());
                }
            }
        }
        serial = serial.checked_add(1).ok_or("Delivery serial exhausted")?;
        operation = Operation::Step;
    };
    drop(child);
    let verified = SourceFile::private(&temporary, &control.cancel)?;
    if verified.fingerprint().sha256 != receipt.file_sha256
        || verified.fingerprint().bytes != receipt.file_bytes
    {
        return Err("Private delivery changed after child verification".into());
    }
    verified.verify(&control.cancel)?;
    control
        .state
        .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| editbay_media::Error::Cancelled)?;
    progress(
        &Progress {
            phase: Phase::Publishing,
            prepared_samples: preparation,
            total_preparation_samples: preparation,
            pictures: receipt.profile.frames,
            samples: receipt.profile.samples_through(receipt.profile.frames)?,
            total_pictures: receipt.profile.frames,
            total_samples: receipt.profile.samples_through(receipt.profile.frames)?,
        },
        pid,
    );
    publish(&temporary, &directory_file, name)?;
    control.state.store(3, Ordering::Release);
    directory_file.sync_all().map_err(|error| {
        format!(
            "Master published at {}, but directory durability could not be confirmed: {error}",
            destination.display()
        )
    })?;
    Ok(receipt)
}

fn validate_reply(
    response: &Response,
    owner: Owner,
    serial: u64,
    expected: &LosslessMovProfile,
    preparation: u64,
    previous: Option<&Progress>,
) -> Result<()> {
    if response.owner != owner || response.serial != serial {
        return Err("Delivery child returned stale or foreign work".into());
    }
    let Outcome::Progress { progress, receipt } = &response.outcome else {
        return Ok(());
    };
    let samples = expected.samples_through(expected.frames)?;
    let rank = |phase| match phase {
        Phase::Preparing => Some(0),
        Phase::Rendering => Some(1),
        Phase::VerifyingPictures => Some(2),
        Phase::VerifyingSound => Some(3),
        Phase::Complete => Some(4),
        _ => None,
    };
    let phase = rank(progress.phase).ok_or("Worker returned a parent-owned phase")?;
    if progress.total_pictures != expected.frames
        || progress.total_samples != samples
        || progress.pictures > expected.frames
        || progress.samples > samples
        || progress.total_preparation_samples != preparation
        || progress.prepared_samples > preparation
        || (progress.phase != Phase::Preparing && progress.prepared_samples != preparation)
    {
        return Err("Delivery returned invalid progress bounds".into());
    }
    if let Some(previous) = previous {
        let prior = rank(previous.phase).ok_or("Invalid previous delivery phase")?;
        if phase < prior
            || phase > prior + 1
            || progress.prepared_samples < previous.prepared_samples
            || (phase == prior
                && (progress.pictures < previous.pictures || progress.samples < previous.samples))
        {
            return Err("Delivery progress skipped or replayed work".into());
        }
    } else if phase != u8::from(preparation == 0)
        || progress.pictures != 0
        || progress.samples != 0
        || progress.prepared_samples != 0
    {
        return Err("Delivery did not begin with an empty render".into());
    }
    match progress.phase {
        Phase::Preparing if preparation == 0 || progress.pictures != 0 || progress.samples != 0 => {
            return Err("Sound preparation returned output before the selected range".into());
        }
        Phase::Rendering if progress.samples != expected.samples_through(progress.pictures)? => {
            return Err("Rendered picture and sound counters disagree".into());
        }
        Phase::VerifyingPictures if progress.samples != 0 => {
            return Err("Picture verification returned sound progress".into());
        }
        Phase::VerifyingSound | Phase::Complete if progress.pictures != expected.frames => {
            return Err("Sound verification preceded completed pictures".into());
        }
        _ => {}
    }
    if progress.phase == Phase::Complete {
        let receipt = receipt
            .as_ref()
            .ok_or("Complete delivery has no verification receipt")?;
        if progress.samples != samples
            || receipt.version != owner.version
            || receipt.profile != *expected
            || receipt.file_bytes == 0
            || [
                &receipt.pixel_sha256,
                &receipt.pcm_sha256,
                &receipt.decoded_pixel_sha256,
                &receipt.decoded_pcm_sha256,
                &receipt.file_sha256,
            ]
            .iter()
            .any(|value| {
                value.len() != 64
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err("Delivery receipt differs from the captured output".into());
        }
    } else if receipt.is_some() {
        return Err("Delivery returned an early publication receipt".into());
    }
    Ok(())
}

fn private_file(directory: &File) -> Result<File> {
    use rustix::fs::{Mode, OFlags, openat};
    openat(
        directory,
        ".",
        OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map(File::from)
    .map_err(|error| {
        format!("The export folder cannot create anonymous private files: {error}").into()
    })
}

fn publish(file: &File, directory: &File, name: &std::ffi::OsStr) -> Result<()> {
    use rustix::fs::{AtFlags, CWD, linkat};
    let source = format!("/proc/self/fd/{}", file.as_raw_fd());
    linkat(CWD, source, directory, name, AtFlags::SYMLINK_FOLLOW)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        os::unix::fs::{MetadataExt, symlink},
    };

    #[test]
    fn source_preparation_cannot_skip_work_or_claim_another_total() {
        let owner = Owner {
            job: Uuid::new_v4(),
            version: DocumentVersion::of(&Project::new("Range").unwrap()),
        };
        let profile = LosslessMovProfile {
            width: 32,
            height: 18,
            frame_rate: editbay_core::FrameRate::new(30, 1).unwrap(),
            first_frame: 4,
            frames: 3,
            sample_rate: 48000,
            channels: vec!["FL".into(), "FR".into()],
        };
        let begin = Progress {
            phase: Phase::Preparing,
            prepared_samples: 0,
            total_preparation_samples: 6400,
            pictures: 0,
            samples: 0,
            total_pictures: 3,
            total_samples: 4800,
        };
        let response = |progress| Response {
            owner,
            serial: 0,
            outcome: Outcome::Progress {
                progress,
                receipt: None,
            },
        };
        assert!(validate_reply(&response(begin.clone()), owner, 0, &profile, 6400, None).is_ok());
        let partial = Progress {
            prepared_samples: 4096,
            ..begin.clone()
        };
        assert!(
            validate_reply(
                &response(partial.clone()),
                owner,
                0,
                &profile,
                6400,
                Some(&begin)
            )
            .is_ok()
        );
        let ready = Progress {
            phase: Phase::Rendering,
            prepared_samples: 6400,
            ..begin.clone()
        };
        assert!(
            validate_reply(
                &response(ready.clone()),
                owner,
                0,
                &profile,
                6400,
                Some(&partial)
            )
            .is_ok()
        );
        for invalid in [
            Progress {
                prepared_samples: 4095,
                ..partial.clone()
            },
            Progress {
                prepared_samples: 6401,
                ..partial.clone()
            },
            Progress {
                total_preparation_samples: 6401,
                ..partial.clone()
            },
            Progress {
                phase: Phase::Rendering,
                ..partial.clone()
            },
            Progress {
                pictures: 1,
                ..partial.clone()
            },
            Progress {
                samples: 1,
                ..partial.clone()
            },
        ] {
            assert!(
                validate_reply(&response(invalid), owner, 0, &profile, 6400, Some(&partial))
                    .is_err()
            );
        }
        assert!(validate_reply(&response(ready), owner, 0, &profile, 6400, None).is_err());
    }

    #[test]
    fn replies_reject_foreign_owners_stale_serials_skipped_phases_and_malformed_receipts() {
        let owner = Owner {
            job: Uuid::new_v4(),
            version: DocumentVersion::of(&Project::new("Owner").unwrap()),
        };
        let profile = LosslessMovProfile {
            width: 32,
            height: 18,
            frame_rate: editbay_core::FrameRate::new(30, 1).unwrap(),
            first_frame: 0,
            frames: 3,
            sample_rate: 48000,
            channels: vec!["FL".into(), "FR".into()],
        };
        let start = Progress {
            phase: Phase::Rendering,
            prepared_samples: 0,
            total_preparation_samples: 0,
            pictures: 0,
            samples: 0,
            total_pictures: 3,
            total_samples: 4800,
        };
        let make = |progress: Progress, receipt: Option<Receipt>| Response {
            owner,
            serial: 0,
            outcome: Outcome::Progress {
                progress,
                receipt: receipt.map(Box::new),
            },
        };
        assert!(validate_reply(&make(start.clone(), None), owner, 0, &profile, 0, None).is_ok());
        let mut wrong = owner;
        wrong.job = Uuid::new_v4();
        assert!(validate_reply(&make(start.clone(), None), wrong, 0, &profile, 0, None).is_err());
        wrong = owner;
        wrong.version.revision += 1;
        assert!(validate_reply(&make(start.clone(), None), wrong, 0, &profile, 0, None).is_err());
        assert!(validate_reply(&make(start.clone(), None), owner, 1, &profile, 0, None).is_err());
        for phase in [
            Phase::Preparing,
            Phase::Publishing,
            Phase::VerifyingFile,
            Phase::VerifyingSound,
            Phase::Complete,
        ] {
            assert!(
                validate_reply(
                    &make(
                        Progress {
                            phase,
                            ..start.clone()
                        },
                        None
                    ),
                    owner,
                    0,
                    &profile,
                    0,
                    Some(&start)
                )
                .is_err()
            );
        }
        for (pictures, samples, total_pictures) in [(4, 6400, 3), (1, 1, 3), (0, 0, 4)] {
            assert!(
                validate_reply(
                    &make(
                        Progress {
                            pictures,
                            samples,
                            total_pictures,
                            ..start.clone()
                        },
                        None
                    ),
                    owner,
                    0,
                    &profile,
                    0,
                    Some(&start)
                )
                .is_err()
            );
        }
        let prior = Progress {
            phase: Phase::VerifyingSound,
            pictures: 3,
            samples: 4800,
            ..start.clone()
        };
        let complete = Progress {
            phase: Phase::Complete,
            ..prior.clone()
        };
        let receipt = Receipt {
            format: crate::DeliveryFormat::default(),
            version: owner.version,
            profile: profile.clone(),
            pixel_sha256: "a".repeat(64),
            pcm_sha256: "b".repeat(64),
            decoded_pixel_sha256: "a".repeat(64),
            decoded_pcm_sha256: "b".repeat(64),
            picture_quality: crate::SignalComparison::default(),
            sound_quality: crate::SignalComparison::default(),
            file_sha256: "c".repeat(64),
            file_bytes: 4096,
            clipped_picture_values: 0,
            adapter: "test".into(),
        };
        assert!(
            validate_reply(
                &make(complete.clone(), Some(receipt.clone())),
                owner,
                0,
                &profile,
                0,
                Some(&prior)
            )
            .is_ok()
        );
        assert!(
            validate_reply(
                &make(complete.clone(), None),
                owner,
                0,
                &profile,
                0,
                Some(&prior)
            )
            .is_err()
        );
        assert!(
            validate_reply(
                &make(start.clone(), Some(receipt.clone())),
                owner,
                0,
                &profile,
                0,
                None
            )
            .is_err()
        );
        let mut malformed = receipt.clone();
        malformed.file_sha256 = "wrong".into();
        assert!(
            validate_reply(
                &make(complete.clone(), Some(malformed)),
                owner,
                0,
                &profile,
                0,
                Some(&prior)
            )
            .is_err()
        );
        let mut malformed = receipt.clone();
        malformed.profile.first_frame = 1;
        assert!(
            validate_reply(
                &make(complete.clone(), Some(malformed)),
                owner,
                0,
                &profile,
                0,
                Some(&prior)
            )
            .is_err()
        );
        let mut malformed = receipt.clone();
        malformed.profile.channels = vec!["FC".into()];
        assert!(
            validate_reply(
                &make(complete.clone(), Some(malformed)),
                owner,
                0,
                &profile,
                0,
                Some(&prior)
            )
            .is_err()
        );
        let malformed = serde_json::json!({"owner":owner,"serial":0,"outcome":{"kind":"progress","progress":start,"receipt":null,"foreign":true}});
        assert!(serde_json::from_value::<Response>(malformed).is_err());
    }

    #[test]
    fn anonymous_output_has_no_cleanup_path_and_publication_never_clobbers() {
        let directory = tempfile::tempdir().unwrap();
        let parent = File::open(directory.path()).unwrap();
        let mut file = private_file(&parent).unwrap();
        file.write_all(b"verified master").unwrap();
        file.sync_all().unwrap();
        assert_eq!(file.metadata().unwrap().nlink(), 0);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        let token = Cancellation::new().unwrap();
        let source = SourceFile::private(&file, &token).unwrap();
        source.verify(&token).unwrap();
        let name = std::ffi::OsStr::new("master.mov");
        publish(&file, &parent, name).unwrap();
        assert_eq!(
            std::fs::read(directory.path().join(name)).unwrap(),
            b"verified master"
        );
        assert!(source.verify(&token).is_err());
        assert!(publish(&file, &parent, name).is_err());
        symlink("missing.mov", directory.path().join("link.mov")).unwrap();
        assert!(publish(&file, &parent, std::ffi::OsStr::new("link.mov")).is_err());
        assert_eq!(
            std::fs::read_link(directory.path().join("link.mov")).unwrap(),
            Path::new("missing.mov")
        );
        let mut abandoned = private_file(&parent).unwrap();
        abandoned.write_all(b"partial master").unwrap();
        drop(abandoned);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn private_verification_rejects_mutation_and_named_descriptors() {
        let directory = tempfile::tempdir().unwrap();
        let parent = File::open(directory.path()).unwrap();
        let mut file = private_file(&parent).unwrap();
        file.write_all(b"captured").unwrap();
        let token = Cancellation::new().unwrap();
        let source = SourceFile::private(&file, &token).unwrap();
        file.write_all(b"changed").unwrap();
        assert!(source.verify(&token).is_err());
        let named = File::create(directory.path().join("original.mov")).unwrap();
        assert!(SourceFile::private(&named, &token).is_err());
    }

    #[test]
    fn cancellation_and_publication_have_one_atomic_winner_and_cannot_be_reused() {
        for _ in 0..64 {
            let control = DeliveryControl::new().unwrap();
            let concurrent = control.clone();
            let thread = std::thread::spawn(move || concurrent.cancel());
            let publishing = control
                .state
                .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                .is_ok();
            let cancelled = thread.join().unwrap();
            assert_ne!(publishing, cancelled);
            assert_eq!(control.cancel.is_cancelled(), cancelled);
            assert!(!control.cancellable());
        }
        let directory = tempfile::tempdir().unwrap();
        let project = Arc::new(Project::new("Cancelled export").unwrap());
        let request = DeliveryRequest {
            format: crate::DeliveryFormat::default(),
            composition: Uuid::new_v4(),
            sample_rate: 48000,
            range: None,
        };
        let control = DeliveryControl::new().unwrap();
        assert!(control.cancel());
        let first = deliver(
            Path::new("unreachable"),
            project.clone(),
            request,
            &directory.path().join("master.mov"),
            &control,
            |_, _| {},
        )
        .unwrap_err();
        assert_eq!(
            first.to_string(),
            editbay_media::Error::Cancelled.to_string()
        );
        let second = deliver(
            Path::new("unreachable"),
            project,
            request,
            &directory.path().join("master.mov"),
            &control,
            |_, _| {},
        )
        .unwrap_err();
        assert!(second.to_string().contains("already owns"));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
