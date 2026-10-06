use crate::{Cancellation, Error, Result, codec_process::Process, planes};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::File,
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::Path,
    time::Duration,
};

/// A supervised packaged job using bounded messages and one optional owned file.
pub struct NativeJob<R: DeserializeOwned + Send + 'static>(Process<R>);

impl<R: DeserializeOwned + Send + 'static> NativeJob<R> {
    /// Start a packaged Rust job with a private socket and bounded stderr.
    /// `executable` and `endpoint` select its entry point. Returns supervision;
    /// dropping it closes transport, kills stalled work and reaps the child.
    pub fn start(executable: &Path, endpoint: &str) -> Result<Self> {
        Process::start(executable, endpoint).map(Self)
    }

    /// Inspect the owned process identity for diagnostics and cancellation tests.
    /// Takes no arguments; returns the live or most recently owned child PID.
    pub fn process_id(&self) -> u32 {
        self.0.child.id()
    }

    /// Exchange a bounded request off the UI thread.
    /// `request` is typed metadata; `file` optionally transfers one private file;
    /// `cancel` interrupts the wait. Returns one typed reply; unsolicited files fail.
    pub fn request(
        &mut self,
        request: &impl Serialize,
        file: Option<&File>,
        cancel: &Cancellation,
    ) -> Result<R> {
        self.request_with_timeout(request, file, cancel, Duration::from_secs(120))
    }

    /// Exchange one operation with a caller-declared bounded response deadline.
    /// `request`, `file` and `cancel` retain ordinary job ownership; `timeout`
    /// must be positive and at most 120 seconds. Returns a typed reply or failure.
    pub fn request_with_timeout(
        &mut self,
        request: &impl Serialize,
        file: Option<&File>,
        cancel: &Cancellation,
        timeout: Duration,
    ) -> Result<R> {
        let descriptor: Option<OwnedFd> = file.map(File::try_clone).transpose()?.map(Into::into);
        let (reply, file) =
            self.0
                .exchange_file_timeout(request, cancel, descriptor.as_ref(), timeout)?;
        if file.is_some() {
            return Err(Error::Invalid(
                "job returned an unexpected descriptor".into(),
            ));
        }
        Ok(reply)
    }
}

/// The child's single inherited private job channel.
pub struct JobChannel(UnixStream);

impl JobChannel {
    /// Take the inherited socket once inside a packaged worker entry point.
    /// Takes no arguments; returns ownership after validating the descriptor type.
    pub fn inherited() -> Result<Self> {
        planes::inherited().map(Self)
    }

    /// Receive one bounded typed operation and optional descriptor.
    /// Takes no arguments; returns EOF after its parent closes the channel.
    pub fn receive<T: DeserializeOwned>(&mut self) -> Result<Option<(T, Option<File>)>> {
        planes::receive(&mut self.0)?
            .map(|(bytes, descriptor)| {
                let value =
                    serde_json::from_slice(&bytes).map_err(|e| Error::Invalid(e.to_string()))?;
                Ok((value, descriptor.map(File::from)))
            })
            .transpose()
    }

    /// Return one bounded typed outcome to the supervising parent.
    /// `value` is metadata only. Returns after writing; no output file is published.
    pub fn send(&mut self, value: &impl Serialize) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|e| Error::Invalid(e.to_string()))?;
        planes::send(&mut self.0, &bytes, None)
    }
}
