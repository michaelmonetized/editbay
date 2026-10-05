//! Native project identity, validation, durable storage, and recovery.
//!
//! One typed media/composition document is shared by UI, CLI, automation and recovery.

mod authoring;
pub use authoring::sequence_from_video;
mod command;
mod composition;
mod document;
mod evaluation;
mod prepared;
mod sound;
mod storage;
mod time;

pub use command::{
    ChangeImpact, CommandGroup, CommandReceipt, DocumentCommand, DocumentEditor, DocumentVersion,
};
pub use composition::{
    AlphaMode, AnimatedProperty, AnimationChannel, AssetKind, AssetReference, Clip, ClipSource,
    ColorConfiguration, Composition, FloatPrecision, FrameRange, Interpolation, Keyframe,
    MediaSource, NodeOperation, OutputTransfer, PictureTiming, SocketType, SourceColor,
    SourceStream, StreamFormat, TimedNode, Track, TrackKind, WorkingGamut,
};
pub use document::{FrameRate, PROJECT_SCHEMA, Project, Sequence};
pub use evaluation::{EvaluatedNode, FramePlan, SourceRequest};
pub use prepared::{EvaluationSnapshot, PreparedFrame, PreparedNode};
pub use sound::{
    SoundBlockPlan, SoundBudget, SoundProfile, SoundSample, SoundSnapshot, SoundSourcePlan,
};
pub use storage::{
    MAX_DOCUMENT_BYTES, PreparedCheckpoint, RecoveryCatalog, RecoveryFailure, RecoveryRecord,
    checkpoint, load, load_bounded, prepare_checkpoint, recover_copy, recovery_catalog, save,
    save_if_unchanged, save_new,
};
pub use time::{SourcePosition, TimeBase, TimeMap, TimePoint};

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
