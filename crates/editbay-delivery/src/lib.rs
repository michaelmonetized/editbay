//! Supervised, verified delivery of the saved picture and sound graph.

mod engine;
mod job;
mod worker;

pub use job::{DeliveryControl, deliver};
pub use worker::serve;

use editbay_core::{DocumentVersion, FrameRange};
use editbay_media::LosslessMovProfile;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The saved composition, optional frame range and explicit PCM output rate.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRequest {
    pub composition: Uuid,
    pub sample_rate: u32,
    pub range: Option<FrameRange>,
}

impl DeliveryRequest {
    /// Resolve a nonempty half-open export interval within a captured composition.
    /// `duration` is the composition's exact exclusive end; returns the full
    /// interval when no range was selected, or an error before output work begins.
    pub fn frame_range(&self, duration: u64) -> Result<FrameRange> {
        let range = self.range.unwrap_or(FrameRange {
            start: 0,
            end: duration,
        });
        if range.start >= range.end || range.end > duration || range.end > i64::MAX as u64 {
            return Err("Export range is empty or outside the selected sequence".into());
        }
        Ok(range)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_requests_reject_empty_reversed_foreign_and_overflowed_intervals() {
        let mut request = DeliveryRequest {
            composition: Uuid::new_v4(),
            sample_rate: 48000,
            range: None,
        };
        assert_eq!(
            request.frame_range(10).unwrap(),
            FrameRange { start: 0, end: 10 }
        );
        assert!(request.frame_range(0).is_err());
        for (start, end) in [(0, 0), (7, 3), (0, 11), (u64::MAX - 1, u64::MAX)] {
            request.range = Some(FrameRange { start, end });
            assert!(request.frame_range(10).is_err());
        }
        request.range = Some(FrameRange { start: 2, end: 7 });
        assert_eq!(request.frame_range(10).unwrap(), request.range.unwrap());
        request.range = None;
        assert!(request.frame_range(u64::MAX).is_err());
    }
}
