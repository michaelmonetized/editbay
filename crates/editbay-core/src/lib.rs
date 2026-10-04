//! Native project identity, validation, durable storage, and recovery.
//!
//! This initial schema does not yet represent the media timeline/compositor.

mod command;
mod document;
mod storage;

pub use command::{CommandReceipt, DocumentCommand, DocumentEditor, DocumentVersion};
pub use document::{FrameRate, PROJECT_SCHEMA, Project, Sequence};
pub use storage::{
    PreparedCheckpoint, RecoveryCatalog, RecoveryFailure, RecoveryRecord, checkpoint, load,
    load_bounded, prepare_checkpoint, recover_copy, recovery_catalog, save, save_if_unchanged,
    save_new,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid project JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported project schema {0}; this build supports schema {PROJECT_SCHEMA}")]
    Schema(u32),
    #[error("invalid project: {0}")]
    Invalid(String),
    #[error("destination already exists: {0}")]
    Exists(std::path::PathBuf),
    #[error("another writer owns this destination: {0}")]
    Busy(std::path::PathBuf),
    #[error("project changed on disk; reload before saving: {0}")]
    Conflict(std::path::PathBuf),
    #[error("stale command revision {expected}; current revision is {current}")]
    StaleCommand { expected: u64, current: u64 },
    #[error("recovery checkpoint failed its integrity check")]
    Integrity,
    #[error("recovery must use a destination separate from the original project")]
    OriginalDestination,
}

pub type Result<T> = std::result::Result<T, Error>;
