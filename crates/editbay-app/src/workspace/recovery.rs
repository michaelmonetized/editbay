use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
    mpsc::{self, Receiver, SyncSender},
};

pub(super) struct Publication {
    state: Arc<AtomicU8>,
    prepared: Arc<AtomicU8>,
    approval: SyncSender<()>,
}

pub(super) struct Permit {
    state: Arc<AtomicU8>,
    prepared: Arc<AtomicU8>,
    approval: Receiver<()>,
}

impl Publication {
    /// Create a single-use ownership decision for a filesystem worker.
    /// Takes no arguments; returns UI control and the worker's publication permit.
    pub(super) fn new() -> (Self, Permit) {
        let state = Arc::new(AtomicU8::new(0));
        let prepared = Arc::new(AtomicU8::new(0));
        let (sender, receiver) = mpsc::sync_channel(1);
        (
            Self {
                state: state.clone(),
                prepared: prepared.clone(),
                approval: sender,
            },
            Permit {
                state,
                prepared,
                approval: receiver,
            },
        )
    }

    /// Release prepared work after the UI checks its captured tab and path.
    /// Takes no arguments; returns an error if the worker no longer accepts work.
    pub(super) fn approve(&self) -> Result<(), String> {
        self.prepared.store(2, Ordering::Release);
        self.approval.try_send(()).map_err(|e| e.to_string())
    }

    /// Inspect the pending worker without waiting for its filesystem operation.
    /// Takes no arguments; returns a diagnostic phase, never a durable acknowledgement.
    pub(super) fn phase(&self) -> &'static str {
        match self.state.load(Ordering::Acquire) {
            1 => "cancelled",
            2 => "publishing",
            _ => match self.prepared.load(Ordering::Acquire) {
                0 => "preparing",
                1 => "awaiting_approval",
                _ => "approved",
            },
        }
    }

    /// Revoke unclaimed publication without waiting for the worker or filesystem.
    /// Takes no arguments and returns immediately; an already claimed commit finishes.
    pub(super) fn cancel(&self) {
        let _ = self
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
        let _ = self.approval.try_send(());
    }
}

impl Drop for Publication {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Permit {
    /// Report that private synchronized bytes are waiting for ownership approval.
    /// Takes no arguments and publishes no file; returns no value.
    pub(super) fn prepared(&self) {
        self.prepared.store(1, Ordering::Release);
    }
    /// Wait on the worker for approval, then claim publication against cancellation.
    /// `commit` owns all filesystem work and cleanup. Returns its result only when
    /// publication wins; dropping rejected work also happens on this worker.
    pub(super) fn publish<T>(self, commit: impl FnOnce() -> T) -> Option<T> {
        self.approval.recv().ok()?;
        self.state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(commit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editbay_core::{Project, prepare_checkpoint, recovery_catalog};
    use std::time::Duration;

    #[test]
    fn cancellation_or_drop_before_publication_keeps_prepared_files_private() {
        for approved in [false, true] {
            for dropped in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let prepared =
                    prepare_checkpoint(&Project::new("Private").unwrap(), None, root.path())
                        .unwrap();
                let (control, permit) = Publication::new();
                if approved {
                    control.approve().unwrap();
                }
                if dropped {
                    drop(control);
                } else {
                    control.cancel();
                }
                assert!(permit.publish(|| prepared.commit()).is_none());
                assert!(recovery_catalog(root.path()).unwrap().valid.is_empty());
                let project = std::fs::read_dir(root.path())
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap();
                assert_eq!(std::fs::read_dir(project.path()).unwrap().count(), 0);
            }
        }
    }

    #[test]
    fn publication_waits_for_approval_and_cancellation_never_waits_for_commit_io() {
        let root = tempfile::tempdir().unwrap();
        let prepared =
            prepare_checkpoint(&Project::new("Durable").unwrap(), None, root.path()).unwrap();
        let (control, permit) = Publication::new();
        assert_eq!(control.phase(), "preparing");
        permit.prepared();
        assert_eq!(control.phase(), "awaiting_approval");
        let (started, entered) = mpsc::sync_channel(1);
        let (release, blocked) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            permit.publish(|| {
                started.send(()).unwrap();
                blocked.recv().unwrap();
                prepared.commit().unwrap()
            })
        });
        assert!(entered.recv_timeout(Duration::from_millis(25)).is_err());
        assert!(recovery_catalog(root.path()).unwrap().valid.is_empty());
        control.approve().unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(control.phase(), "publishing");
        control.cancel();
        drop(control);
        assert!(recovery_catalog(root.path()).unwrap().valid.is_empty());
        release.send(()).unwrap();
        let path = worker.join().unwrap().unwrap();
        assert_eq!(recovery_catalog(root.path()).unwrap().valid[0].path, path);
    }
}
