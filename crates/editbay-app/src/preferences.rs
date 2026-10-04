use eframe::egui;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Startup {
    #[default]
    Welcome,
    RestoreWorkspace,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shortcuts {
    pub new_project: char,
    pub open: char,
    pub save: char,
    pub undo: char,
    pub redo: char,
    pub redo_shift: bool,
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            new_project: 'N',
            open: 'O',
            save: 'S',
            undo: 'Z',
            redo: 'Z',
            redo_shift: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub startup: Startup,
    pub open_paths: Vec<PathBuf>,
    pub active_path: Option<PathBuf>,
    pub catalog_root: Option<PathBuf>,
    pub shortcuts: Shortcuts,
}

impl Preferences {
    /// Validate native workspace and shortcut settings before publication.
    /// Takes no arguments. Returns a visible error for excessive paths, invalid
    /// letters or colliding chords; it never mutates the caller's settings.
    pub fn validate(&self) -> Result<(), String> {
        if self.open_paths.len() > 128
            || self
                .open_paths
                .iter()
                .any(|path| path.as_os_str().is_empty())
        {
            return Err("Preferences support up to 128 nonempty project paths".into());
        }
        let s = &self.shortcuts;
        let mut chords = HashSet::new();
        for (key, shift) in [
            (s.new_project, false),
            (s.open, false),
            (s.save, false),
            (s.save, true),
            (s.undo, false),
            (s.redo, s.redo_shift),
        ] {
            if matches!(key, 'A' | 'C' | 'V' | 'X') {
                return Err("Ctrl+A, C, V and X belong to text selection and the clipboard".into());
            }
            if !key.is_ascii_uppercase() || !chords.insert((key, shift)) {
                return Err(
                    "Shortcuts require unique Ctrl+letter chords, with uppercase A–Z keys".into(),
                );
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    schema: u32,
    preferences: Preferences,
}

type Loaded = Result<(Preferences, Option<String>), String>;

pub struct PreferenceStore {
    pub current: Preferences,
    pub ready: bool,
    pub error: Option<String>,
    path: PathBuf,
    expected: Option<String>,
    loading: Option<Receiver<Loaded>>,
    saving: Option<Receiver<Result<String, String>>>,
    dirty: bool,
    wake: egui::Context,
}

impl PreferenceStore {
    /// Load independently versioned local settings on a worker.
    /// `path` is EditBay's preferences file and `wake` its native context.
    /// Returns defaults immediately; malformed files remain intact and visible.
    pub fn new(path: PathBuf, wake: egui::Context) -> Self {
        let mut store = Self {
            current: Preferences::default(),
            ready: false,
            error: None,
            path,
            expected: None,
            loading: None,
            saving: None,
            dirty: false,
            wake,
        };
        store.reload();
        store
    }

    /// Retry a failed read without overwriting its source.
    /// Takes no arguments. Returns immediately while the bounded worker reads.
    pub fn reload(&mut self) {
        if self.loading.is_some() || self.saving.is_some() {
            return;
        }
        let path = self.path.clone();
        let wake = self.wake.clone();
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("editbay-preferences".into())
            .spawn(move || {
                let _ = sender.send(read(&path));
                wake.request_repaint();
            }) {
            Ok(_) => self.loading = Some(receiver),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// Queue the latest valid settings, coalescing changes during an older save.
    /// `preferences` is an owned UI snapshot. Returns a validation failure before
    /// changing memory; publication remains conditional on the loaded file hash.
    pub fn update(&mut self, preferences: Preferences) -> Result<(), String> {
        if !self.ready {
            return Err("Preferences must load successfully before saving changes".into());
        }
        preferences.validate()?;
        if preferences != self.current {
            self.current = preferences;
            self.dirty = true;
        }
        Ok(())
    }

    /// Retry publication after a reported filesystem failure.
    /// Takes no arguments; returns immediately and preserves optimistic ownership.
    pub fn retry_save(&mut self) {
        self.dirty = true;
        self.error = None;
    }

    /// Consume worker results and publish only the newest queued valid snapshot.
    /// Takes no arguments; returns true once after a successful initial/retry load.
    /// Filesystem access never runs in this polling path.
    pub fn poll(&mut self) -> bool {
        let mut loaded = false;
        if let Some(receiver) = &self.loading {
            match receiver.try_recv() {
                Ok(Ok((preferences, expected))) => {
                    self.current = preferences;
                    self.expected = expected;
                    self.ready = true;
                    self.error = None;
                    self.loading = None;
                    self.dirty = false;
                    loaded = true;
                }
                Ok(Err(error)) => {
                    self.error = Some(error);
                    self.ready = false;
                    self.loading = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("Preferences worker stopped unexpectedly".into());
                    self.loading = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(receiver) = &self.saving {
            match receiver.try_recv() {
                Ok(Ok(hash)) => {
                    self.expected = Some(hash);
                    self.saving = None;
                    self.error = None;
                }
                Ok(Err(error)) => {
                    self.error = Some(error);
                    self.saving = None;
                    self.dirty = false;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("Preferences save worker stopped unexpectedly".into());
                    self.saving = None;
                    self.dirty = false;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.ready && self.dirty && self.saving.is_none() {
            let preferences = self.current.clone();
            let path = self.path.clone();
            let expected = self.expected.clone();
            let wake = self.wake.clone();
            let (sender, receiver) = mpsc::channel();
            match std::thread::Builder::new()
                .name("editbay-preferences-save".into())
                .spawn(move || {
                    let _ = sender.send(write(&path, &preferences, expected.as_deref()));
                    wake.request_repaint();
                }) {
                Ok(_) => {
                    self.saving = Some(receiver);
                    self.dirty = false;
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    self.dirty = false;
                }
            }
        }
        loaded
    }

    /// Inspect whether preference bytes are still being loaded or saved.
    /// Takes no arguments and returns true until the corresponding worker completes.
    pub fn busy(&self) -> bool {
        self.loading.is_some() || self.saving.is_some() || (self.ready && self.dirty)
    }
}

fn read(path: &Path) -> Loaded {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Preferences::default(), None));
        }
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file() {
        return Err("Preferences must be a regular file, not a symlink".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 128 * 1024 {
        return Err("Preferences exceed the 128 KiB budget".into());
    }
    let envelope: Envelope = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Preferences could not be read: {e}"))?;
    if envelope.schema != 1 {
        return Err(format!(
            "Unsupported preferences schema {}",
            envelope.schema
        ));
    }
    envelope.preferences.validate()?;
    Ok((
        envelope.preferences,
        Some(format!("{:x}", Sha256::digest(bytes))),
    ))
}

fn write(path: &Path, preferences: &Preferences, expected: Option<&str>) -> Result<String, String> {
    preferences.validate()?;
    let parent = path.parent().ok_or("Preferences need a parent folder")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let lock_path = parent.join(".workspace-preferences.lock");
    if fs::symlink_metadata(&lock_path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err("Preferences lock must be a regular file".into());
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| e.to_string())?;
    lock.try_lock_exclusive()
        .map_err(|e| format!("Preferences writer is busy: {e}"))?;
    if read(path)?.1.as_deref() != expected {
        return Err("Preferences changed in another window; reload before saving".into());
    }
    let mut bytes = serde_json::to_vec_pretty(&Envelope {
        schema: 1,
        preferences: preferences.clone(),
    })
    .map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let temporary = parent.join(format!(".editbay-preferences-{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<(), String> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        if expected.is_none() {
            fs::hard_link(&temporary, path).map_err(|e| e.to_string())?;
            fs::remove_file(&temporary).map_err(|e| e.to_string())?;
        } else {
            fs::rename(&temporary, path).map_err(|e| e.to_string())?;
        }
        File::open(parent)
            .map_err(|e| e.to_string())?
            .sync_all()
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn settle(store: &mut PreferenceStore) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while store.busy() {
            store.poll();
            assert!(Instant::now() < deadline, "preferences worker timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn atomic_preferences_coalesce_changes_and_detect_stale_writers() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state/workspace.json");
        let mut first = PreferenceStore::new(path.clone(), egui::Context::default());
        let mut second = PreferenceStore::new(path.clone(), egui::Context::default());
        assert!(first.update(Preferences::default()).is_err());
        settle(&mut first);
        settle(&mut second);
        let mut preferences = first.current.clone();
        preferences.startup = Startup::RestoreWorkspace;
        preferences.open_paths = vec![PathBuf::from("/tmp/client.editbay")];
        preferences.active_path = preferences.open_paths.first().cloned();
        first.update(preferences.clone()).unwrap();
        first.poll();
        preferences.shortcuts.new_project = 'P';
        first.update(preferences.clone()).unwrap();
        settle(&mut first);
        assert!(first.error.is_none());
        assert_eq!(read(&path).unwrap().0, preferences);
        let original = fs::read(&path).unwrap();
        let mut competing = second.current.clone();
        competing.shortcuts.open = 'L';
        second.update(competing).unwrap();
        settle(&mut second);
        assert!(
            second
                .error
                .as_ref()
                .unwrap()
                .contains("changed in another")
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        second.reload();
        settle(&mut second);
        assert_eq!(second.current, preferences);
        assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn invalid_preferences_preserve_the_source_and_colliding_chords_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspace.json");
        for bytes in [
            b"{broken".as_slice(),
            b"{\"schema\":9,\"preferences\":{}}".as_slice(),
        ] {
            fs::write(&path, bytes).unwrap();
            let mut store = PreferenceStore::new(path.clone(), egui::Context::default());
            settle(&mut store);
            assert!(!store.ready);
            assert!(store.error.is_some());
            assert!(store.update(Preferences::default()).is_err());
            store.retry_save();
            assert!(!store.busy());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        let mut preferences = Preferences::default();
        preferences.shortcuts.redo_shift = false;
        assert!(preferences.validate().is_err());
        preferences.shortcuts.redo = 'Y';
        preferences.validate().unwrap();
        preferences.shortcuts.new_project = 's';
        assert!(preferences.validate().is_err());
        preferences.shortcuts.new_project = 'V';
        assert!(preferences.validate().is_err());
        fs::remove_file(&path).unwrap();
        let outside = directory.path().join("outside");
        fs::write(&outside, b"untouched").unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(read(&path).is_err());
        assert!(write(&path, &Preferences::default(), None).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"untouched");
    }
}
