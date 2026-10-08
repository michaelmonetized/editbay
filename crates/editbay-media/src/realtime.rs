use crate::{Error, Result};

/// Restore the caller's original scheduling when its audio lifetime ends.
pub struct Priority(Option<audio_thread_priority::RtPriorityHandle>);

impl Priority {
    /// Give an off-callback sound worker a bounded real-time scheduling allowance.
    /// `frames` and `rate` declare its audio deadline. Returns a thread-owned guard
    /// after the operating system grants priority, or a visible startup failure.
    pub fn acquire(frames: u32, rate: u32) -> Result<Self> {
        if !(1..=16384).contains(&frames) || !(8000..=384000).contains(&rate) {
            return Err(Error::Invalid("Invalid sound scheduling deadline".into()));
        }
        audio_thread_priority::promote_current_thread_to_real_time(frames, rate)
            .map(|handle| Self(Some(handle)))
            .map_err(|error| Error::Invalid(format!("Sound scheduling: {error}")))
    }
}

impl Drop for Priority {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            let _ = audio_thread_priority::demote_current_thread_from_real_time(handle);
        }
    }
}
