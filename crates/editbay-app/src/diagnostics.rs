use crate::workspace::Workspace;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, PartialEq, Eq, Serialize)]
struct Tab {
    session: uuid::Uuid,
    project: uuid::Uuid,
    revision: u64,
    name: String,
    path: Option<PathBuf>,
    dirty: bool,
    recovery_revision: Option<u64>,
    recovery_error: Option<String>,
    sources: usize,
    compositions: usize,
    assets: usize,
}

pub struct Diagnostics {
    sender: SyncSender<Value>,
    errors: Receiver<String>,
    previous: Vec<Tab>,
    previous_active: Option<uuid::Uuid>,
    dropped: u64,
    previous_media: Value,
    previous_preview: Value,
    previous_delivery: Value,
}

impl Diagnostics {
    /// Record opt-in native state and timing without blocking application input.
    /// `path` is a new local JSONL file and `ctx` wakes for visible write errors.
    /// Returns a bounded worker channel; it cannot issue commands or alter documents.
    pub fn new(path: PathBuf, ctx: eframe::egui::Context) -> Result<Self, String> {
        let (sender, records) = mpsc::sync_channel::<Value>(256);
        let (error_sender, errors) = mpsc::channel();
        std::thread::Builder::new()
            .name("editbay-diagnostics".into())
            .spawn(move || {
                let result = (|| -> Result<(), String> {
                    let mut options = OpenOptions::new();
                    options.write(true).create_new(true);
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                    let mut file = BufWriter::new(options.open(path).map_err(|e| e.to_string())?);
                    let mut written = 0;
                    for record in records {
                        let mut bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
                        bytes.push(b'\n');
                        written += bytes.len();
                        if written > 16 * 1024 * 1024 {
                            return Err("Native diagnostics reached its 16 MiB budget".into());
                        }
                        file.write_all(&bytes)
                            .and_then(|_| file.flush())
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    let _ = error_sender.send(format!("Native diagnostics: {error}"));
                    ctx.request_repaint();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sender,
            errors,
            previous: Vec::new(),
            previous_active: None,
            dropped: 0,
            previous_media: Value::Null,
            previous_preview: Value::Null,
            previous_delivery: Value::Null,
        })
    }

    /// Report native input delivery and measured CPU frame work.
    /// `kind` identifies the observation; `details` contains actual local metrics.
    /// Takes no filesystem action and drops excess diagnostics instead of blocking.
    pub fn record(&mut self, kind: &str, details: Value) {
        let unix_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        if self.sender.try_send(json!({"unix_us":unix_us,"pid":std::process::id(),"kind":kind,"details":details,"dropped_before":self.dropped})).is_err() { self.dropped += 1; }
    }

    /// Publish changed native document state after real UI actions and worker acknowledgements.
    /// `workspace` is the live owner. Returns no value; checkpoint revisions reflect
    /// completed durable publication, never a scheduled or prepared job.
    pub fn observe(
        &mut self,
        workspace: &Workspace,
        media: &crate::media_ui::MediaPane,
        preview: &crate::preview::PreviewPane,
        delivery: &crate::delivery_ui::DeliveryPane,
    ) {
        let current: Vec<_> = workspace
            .tabs
            .iter()
            .map(|tab| Tab {
                session: tab.id,
                project: tab.editor.project().id,
                revision: tab.editor.project().revision,
                name: tab.editor.project().name.clone(),
                path: tab.path.clone(),
                dirty: tab.dirty(),
                recovery_revision: tab.recovery_revision,
                recovery_error: tab.recovery_error.clone(),
                sources: tab.editor.project().sources.len(),
                compositions: tab.editor.project().compositions.len(),
                assets: tab.editor.project().assets.len(),
            })
            .collect();
        if current != self.previous || workspace.active != self.previous_active {
            self.record("workspace", json!({"tabs":&current,"active":workspace.active,"recovery_publications":workspace.recovery_commit_times.len(),"last_recovery_commit_us":workspace.recovery_commit_times.last().map(|time|time.as_micros() as u64)}));
            self.previous = current;
            self.previous_active = workspace.active;
        }
        let current_media = media.diagnostic_state();
        if current_media != self.previous_media {
            self.record("media", current_media.clone());
            self.previous_media = current_media;
        }
        let current_preview = preview.diagnostic_state();
        if current_preview != self.previous_preview {
            self.record("preview", current_preview.clone());
            self.previous_preview = current_preview;
        }
        let current_delivery = delivery.diagnostic_state();
        if current_delivery != self.previous_delivery {
            self.record("delivery", current_delivery.clone());
            self.previous_delivery = current_delivery;
        }
    }

    /// Receive a visible diagnostics failure from the worker.
    /// Takes no arguments and returns the next local storage/budget error, if any.
    pub fn error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }
}
