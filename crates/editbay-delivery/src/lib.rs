//! Supervised, verified delivery of the saved picture and sound graph.

mod engine;
mod job;
mod worker;

pub use job::{DeliveryControl, deliver};
pub use worker::serve;

use editbay_core::DocumentVersion;
use editbay_media::LosslessMovProfile;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The saved composition and explicit PCM rate for one full-sequence master.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRequest {
    pub composition: Uuid,
    pub sample_rate: u32,
}

/// One observable phase of bounded background rendering and decode verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preparing,
    Rendering,
    VerifyingPictures,
    VerifyingSound,
    VerifyingFile,
    Publishing,
    Complete,
}

/// Cumulative counters from the captured job; verification is explicit work.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub phase: Phase,
    pub pictures: u64,
    pub samples: u64,
    pub total_pictures: u64,
    pub total_samples: u64,
}

/// A finished private file whose decoded content matches the shared graph.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub version: DocumentVersion,
    pub profile: LosslessMovProfile,
    pub pixel_sha256: String,
    pub pcm_sha256: String,
    pub file_sha256: String,
    pub file_bytes: u64,
    pub clipped_picture_values: u64,
    pub adapter: String,
}
