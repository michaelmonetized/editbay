use crate::{Cancellation, Error, Result, planes};
use editbay_core::DocumentVersion;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::Read,
    net::Shutdown,
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use uuid::Uuid;
const STDERR_BYTES: usize = 8192;

pub(crate) struct Process<T: DeserializeOwned + Send + 'static> {
    pub(crate) child: Child,
    socket: UnixStream,
    responses: mpsc::Receiver<Result<(T, Option<OwnedFd>)>>,
    reader: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<()>>,
    tail: Arc<Mutex<Vec<u8>>>,
}
impl<T: DeserializeOwned + Send + 'static> Process<T> {
    /// Start a packaged codec endpoint under bounded private transport.
    /// `executable` and `endpoint` select the child; returns owned supervision,
    /// a bounded response reader and an 8 KiB retained stderr tail.
    pub(crate) fn start(executable: &Path, endpoint: &str) -> Result<Self> {
        let (socket, inherited) = UnixStream::pair()?;
        socket.set_write_timeout(Some(Duration::from_millis(250)))?;
        let mut command = Command::new(executable);
        command
            .arg(endpoint)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = planes::spawn(&mut command, &inherited)?;
        drop(inherited);
        let mut output = child
            .stderr
            .take()
            .ok_or_else(|| Error::Invalid("codec worker lacks stderr".into()))?;
        let tail = Arc::new(Mutex::new(Vec::new()));
        let retained = tail.clone();
        let mut process = Self {
            child,
            socket,
            responses: mpsc::channel().1,
            reader: None,
            stderr: None,
            tail,
        };
        process.stderr = Some(
            std::thread::Builder::new()
                .name("editbay-codec-stderr".into())
                .spawn(move || {
                    let mut buffer = [0u8; 2048];
                    while let Ok(count) = output.read(&mut buffer) {
                        if count == 0 {
                            break;
                        }
                        if let Ok(mut tail) = retained.lock() {
                            tail.extend_from_slice(&buffer[..count]);
                            let excess = tail.len().saturating_sub(STDERR_BYTES);
                            tail.drain(..excess);
                        }
                    }
                })?,
        );
        let mut input = process.socket.try_clone()?;
        let (sender, receiver) = mpsc::sync_channel(1);
        process.responses = receiver;
        process.reader = Some(
            std::thread::Builder::new()
                .name("editbay-codec-receive".into())
                .spawn(move || {
                    loop {
                        let packet = planes::receive(&mut input).and_then(|packet| {
                            let (bytes, descriptor) = packet.ok_or_else(|| {
                                Error::Invalid("codec worker closed its socket".into())
                            })?;
                            let response = serde_json::from_slice(&bytes)
                                .map_err(|e| Error::Invalid(e.to_string()))?;
                            Ok((response, descriptor))
                        });
                        let failed = packet.is_err();
                        if sender.try_send(packet).is_err() || failed {
                            break;
                        }
                    }
                })?,
        );
        Ok(process)
    }
    /// Exchange one bounded request with the owned codec child.
    /// `request` supplies serializable metadata; `cancel` interrupts the wait.
    /// Returns a typed reply and optional sealed payload; callers reap on failure.
    pub(crate) fn exchange(
        &mut self,
        request: &impl Serialize,
        cancel: &Cancellation,
    ) -> Result<(T, Option<OwnedFd>)> {
        self.exchange_file(request, cancel, None)
    }

    pub(crate) fn exchange_file(
        &mut self,
        request: &impl Serialize,
        cancel: &Cancellation,
        descriptor: Option<&OwnedFd>,
    ) -> Result<(T, Option<OwnedFd>)> {
        self.exchange_file_timeout(request, cancel, descriptor, Duration::from_secs(120))
    }

    pub(crate) fn exchange_file_timeout(
        &mut self,
        request: &impl Serialize,
        cancel: &Cancellation,
        descriptor: Option<&OwnedFd>,
        timeout: Duration,
    ) -> Result<(T, Option<OwnedFd>)> {
        if timeout.is_zero() || timeout > Duration::from_secs(120) {
            return Err(Error::Invalid("invalid native request deadline".into()));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let bytes = serde_json::to_vec(request).map_err(|e| Error::Invalid(e.to_string()))?;
        planes::send(&mut self.socket, &bytes, descriptor).map_err(|e| self.error(e))?;
        let started = Instant::now();
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if started.elapsed() > timeout {
                return Err(self.error(format!("response exceeded {} ms", timeout.as_millis())));
            }
            match self.responses.recv_timeout(Duration::from_millis(2)) {
                Ok(Ok(packet)) => return Ok(packet),
                Ok(Err(error)) => return Err(self.error(error)),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self.error("response reader stopped"));
                }
            }
        }
    }
    fn stop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
        let start = Instant::now();
        while matches!(self.child.try_wait(), Ok(None))
            && start.elapsed() < Duration::from_millis(100)
        {
            std::thread::sleep(Duration::from_millis(2));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(stderr) = self.stderr.take() {
            let _ = stderr.join();
        }
    }
    fn error(&self, message: impl std::fmt::Display) -> Error {
        let tail = self
            .tail
            .lock()
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default();
        Error::Invalid(format!("codec worker failed: {message}; {tail}"))
    }
}
impl<T: DeserializeOwned + Send + 'static> Drop for Process<T> {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ownership {
    pub(crate) session: Uuid,
    pub(crate) job: Uuid,
    pub(crate) worker: Uuid,
    pub(crate) version: DocumentVersion,
    pub(crate) generation: u64,
}
