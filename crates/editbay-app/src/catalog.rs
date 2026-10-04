use editbay_core::{FrameRate, Project};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    time::{Duration, SystemTime},
};

#[derive(Clone, Debug)]
pub struct DocumentEntry {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub name: String,
    pub version: Option<editbay_core::DocumentVersion>,
    pub width: u32,
    pub height: u32,
    pub rate: Option<FrameRate>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProjectFolder {
    pub path: PathBuf,
    pub modified: SystemTime,
}

#[derive(Debug)]
pub enum CatalogEvent {
    Document(DocumentEntry),
    Folder(ProjectFolder),
    Warning { path: PathBuf, error: String },
    Finished { visited: usize, complete: bool },
}

pub struct CatalogScan {
    pub events: Receiver<CatalogEvent>,
    cancelled: Arc<AtomicBool>,
}

impl CatalogScan {
    /// Scan actual local work incrementally on a bounded worker channel.
    /// `root` selects the local catalog scope. Returns a cancellable scan;
    /// invalid projects and incomplete traversal remain visible as events.
    pub fn start(root: PathBuf) -> std::io::Result<Self> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, events) = mpsc::sync_channel(32);
        let token = cancelled.clone();
        std::thread::Builder::new()
            .name("editbay-catalog".into())
            .spawn(move || scan(root, &sender, &token))?;
        Ok(Self { events, cancelled })
    }

    /// Stop traversal and blocked event delivery.
    /// Takes no arguments and returns immediately; the worker checks this token
    /// between entries and file-read chunks, without joining on the UI thread.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl Drop for CatalogScan {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn send(sender: &SyncSender<CatalogEvent>, token: &AtomicBool, mut event: CatalogEvent) -> bool {
    while !token.load(Ordering::Acquire) {
        match sender.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                event = returned;
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    false
}

fn scan(root: PathBuf, sender: &SyncSender<CatalogEvent>, token: &AtomicBool) {
    let mut pending = vec![root];
    let mut visited = 0;
    let mut complete = true;
    let warning =
        |path: PathBuf, error: String| send(sender, token, CatalogEvent::Warning { path, error });
    while let Some(directory) = pending.pop() {
        if token.load(Ordering::Acquire) {
            return;
        }
        match fs::symlink_metadata(&directory) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                complete = false;
                if !warning(
                    directory,
                    "Catalog root changed or is not a regular directory".into(),
                ) {
                    return;
                }
                continue;
            }
            Err(error) => {
                complete = false;
                if !warning(directory, error.to_string()) {
                    return;
                }
                continue;
            }
        }
        let marker = directory.join(".omabrand");
        if let Ok(meta) = fs::symlink_metadata(&marker)
            && (meta.is_file() || meta.is_dir())
            && !send(
                sender,
                token,
                CatalogEvent::Folder(ProjectFolder {
                    path: directory.clone(),
                    modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                }),
            )
        {
            return;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                complete = false;
                if !warning(directory, error.to_string()) {
                    return;
                }
                continue;
            }
        };
        for entry in entries {
            if token.load(Ordering::Acquire) {
                return;
            }
            visited += 1;
            if visited > 500_000 {
                complete = false;
                warning(
                    directory.clone(),
                    "Catalog reached its 500000-entry budget; results are incomplete".into(),
                );
                pending.clear();
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    complete = false;
                    if !warning(directory.clone(), error.to_string()) {
                        return;
                    }
                    continue;
                }
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || matches!(name.to_lowercase().as_str(), "trash" | "$recycle.bin")
            {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(kind) if kind.is_file() && path.extension().is_some_and(|v| v == "editbay") => {
                    if !send(sender, token, CatalogEvent::Document(inspect(path, token))) {
                        return;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    complete = false;
                    if !warning(path, error.to_string()) {
                        return;
                    }
                }
            }
        }
    }
    send(sender, token, CatalogEvent::Finished { visited, complete });
}

fn inspect(path: PathBuf, token: &AtomicBool) -> DocumentEntry {
    let mut entry = DocumentEntry {
        name: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        path,
        modified: SystemTime::UNIX_EPOCH,
        version: None,
        width: 16,
        height: 9,
        rate: None,
        error: None,
    };
    let result = (|| -> Result<Project, Box<dyn std::error::Error>> {
        let meta = fs::symlink_metadata(&entry.path)?;
        if !meta.is_file() || meta.len() > 8 * 1024 * 1024 {
            return Err("Project must be a regular file no larger than 8 MiB".into());
        }
        entry.modified = meta.modified()?;
        let mut file = fs::File::open(&entry.path)?;
        let mut bytes = Vec::new();
        let mut chunk = [0; 65536];
        loop {
            if token.load(Ordering::Acquire) {
                return Err("Catalog cancelled".into());
            }
            let read = file.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            if bytes.len() + read > 8 * 1024 * 1024 {
                return Err("Project grew beyond the 8 MiB catalog budget".into());
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
        let project: Project = serde_json::from_slice(&bytes)?;
        project.validate()?;
        Ok(project)
    })();
    match result {
        Ok(project) => {
            entry.name = project.name.clone();
            entry.version = Some(editbay_core::DocumentVersion::of(&project));
            entry.width = project.sequences[0].width;
            entry.height = project.sequences[0].height;
            entry.rate = Some(project.sequences[0].frame_rate);
        }
        Err(error) => entry.error = Some(error.to_string()),
    }
    entry
}
