//! Native inference candidates with exact artifacts, finite tensors and cancellation.
//! Committed artist mattes and production quality qualification remain separate work.

#[cfg(feature = "tract-reference")]
mod reference;
mod runtime;
mod rvm;
mod sam2;

pub use runtime::{Cancellation, FloatTensor, ModelSession, initialize, runtime_build_info};
pub use rvm::{Matte, RVM_SHA256, Rvm, RvmState};
pub use sam2::{
    PointPrompt, SAM2_ARTIFACTS, SAM2_CONSTANTS_SHA256, SAM2_REVISION, Sam2, Sam2State,
    Segmentation,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("inference cancelled")]
    Cancelled,
    #[error("invalid inference request: {0}")]
    Invalid(String),
    #[error("native inference: {0}")]
    Runtime(String),
    #[error("model artifact failed SHA-256 verification")]
    Integrity,
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
    #[error("inference state: {0}")]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
