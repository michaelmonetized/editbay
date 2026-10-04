use crate::{Cancellation, IngestedSource, MediaProbe, SourceFile, SourceFingerprint};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
};

pub const MESSAGE_BYTES: u64 = 8 * 1024 * 1024;
pub const VIRTUAL_MEMORY_BYTES: u64 = 3 * 1024 * 1024 * 1024;
pub const CPU_SECONDS: u64 = 900;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Probe { path: PathBuf },
    Ingest { name: String, streams: Vec<u32> },
    Verify,
    Cancel,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Probed { probe: MediaProbe },
    Progress { stream: u32, completed: u64 },
    Ingested { imported: IngestedSource },
    Verified { fingerprint: SourceFingerprint },
    Failed { error: String, cancelled: bool },
}

/// Read one bounded media-worker message.
/// `reader` owns a private pipe. Returns one newline-delimited message or EOF;
/// oversized/truncated input fails before JSON parsing.
pub fn read_message(reader: &mut impl BufRead) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    reader
        .take(MESSAGE_BYTES + 1)
        .read_until(b'\n', &mut bytes)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() as u64 > MESSAGE_BYTES || bytes.last() != Some(&b'\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "media worker message exceeds its budget or is truncated",
        ));
    }
    Ok(Some(bytes))
}

/// Write one complete bounded media-worker message.
/// `writer` owns a private pipe and `value` is a typed request/response.
/// Returns after flushing; oversized output is refused before writing any bytes.
pub fn write_message(writer: &mut impl Write, value: &impl Serialize) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    if bytes.len() as u64 >= MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "media worker result exceeds its 8 MiB budget",
        ));
    }
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

fn send(writer: &Arc<Mutex<std::io::Stdout>>, response: &Response) -> std::io::Result<()> {
    write_message(
        &mut *writer
            .lock()
            .map_err(|_| std::io::Error::other("worker output lock failed"))?,
        response,
    )
}

/// Run the isolated native ingest endpoint of a packaged application.
/// Takes typed requests on private stdin and writes bounded responses on stdout.
/// Returns after EOF/cancellation; requests retain the explicitly probed source.
pub fn serve() -> Result<(), Box<dyn std::error::Error>> {
    crate::ffi::worker_limits()?;
    let cancel = Cancellation::new()?;
    let input_cancel = cancel.clone();
    let writer = Arc::new(Mutex::new(std::io::stdout()));
    let (sender, receiver) = mpsc::sync_channel(2);
    std::thread::Builder::new()
        .name("editbay-media-control".into())
        .spawn(move || {
            let mut reader = BufReader::new(std::io::stdin());
            loop {
                let message = match read_message(&mut reader) {
                    Ok(Some(bytes)) => {
                        serde_json::from_slice::<Request>(&bytes).map_err(|e| e.to_string())
                    }
                    Ok(None) => {
                        input_cancel.cancel();
                        break;
                    }
                    Err(error) => {
                        input_cancel.cancel();
                        let _ = sender.send(Err(error.to_string()));
                        break;
                    }
                };
                if matches!(&message, Ok(Request::Cancel)) {
                    input_cancel.cancel();
                    break;
                }
                if sender.send(message).is_err() {
                    break;
                }
            }
        })?;
    let mut source: Option<SourceFile> = None;
    for request in receiver {
        if cancel.is_cancelled() {
            break;
        }
        let result: Result<Response, Box<dyn std::error::Error>> =
            (|| match request.map_err(std::io::Error::other)? {
                Request::Probe { path } => {
                    if source.is_some() {
                        return Err("media worker already owns a source".into());
                    }
                    let file = SourceFile::open(&path, &cancel)?;
                    let probe = file.probe(cancel.clone())?;
                    source = Some(file);
                    Ok(Response::Probed { probe })
                }
                Request::Ingest { name, streams } => {
                    let file = source.as_ref().ok_or("probe a source before ingest")?;
                    let output = writer.clone();
                    let progress_cancel = cancel.clone();
                    let mut progress_error = None;
                    let mut last_progress = std::time::Instant::now();
                    let mut last_stream = None;
                    let imported =
                        file.ingest(name, &streams, cancel.clone(), |stream, completed| {
                            if last_stream == Some(stream)
                                && last_progress.elapsed() < std::time::Duration::from_millis(100)
                            {
                                return;
                            }
                            last_stream = Some(stream);
                            last_progress = std::time::Instant::now();
                            if progress_error.is_none()
                                && let Err(error) =
                                    send(&output, &Response::Progress { stream, completed })
                            {
                                progress_error = Some(error);
                                progress_cancel.cancel();
                            }
                        })?;
                    if let Some(error) = progress_error {
                        return Err(error.into());
                    }
                    Ok(Response::Ingested { imported })
                }
                Request::Verify => {
                    let file = source
                        .as_ref()
                        .ok_or("probe a source before verification")?;
                    file.verify(&cancel)?;
                    Ok(Response::Verified {
                        fingerprint: file.fingerprint().clone(),
                    })
                }
                Request::Cancel => unreachable!(),
            })();
        let response = result.unwrap_or_else(|error| Response::Failed {
            error: error.to_string(),
            cancelled: cancel.is_cancelled(),
        });
        if let Err(error) = send(&writer, &response) {
            let _ = send(
                &writer,
                &Response::Failed {
                    error: error.to_string(),
                    cancelled: cancel.is_cancelled(),
                },
            );
            return Err(error.into());
        }
        if cancel.is_cancelled() {
            break;
        }
    }
    Ok(())
}
