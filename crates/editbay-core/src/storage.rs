use crate::{Error, Project, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const RECOVERY_SCHEMA: u32 = 1;
pub const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    schema: u32,
    saved_at_unix_ms: u64,
    original: Option<PathBuf>,
    project: Project,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    snapshot: Snapshot,
    sha256: String,
}

#[derive(Debug, Serialize)]
pub struct RecoveryRecord {
    pub path: PathBuf,
    pub project_id: Uuid,
    pub name: String,
    pub revision: u64,
    pub saved_at_unix_ms: u64,
    pub original: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    pub frame_rate: crate::FrameRate,
    pub sequence_count: usize,
}

#[derive(Debug, Serialize)]
pub struct RecoveryFailure {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Default, Serialize)]
pub struct RecoveryCatalog {
    pub valid: Vec<RecoveryRecord>,
    pub invalid: Vec<RecoveryFailure>,
}

pub fn load(path: impl AsRef<Path>) -> Result<Project> {
    load_bounded(path, MAX_DOCUMENT_BYTES)
}

/// Read a validated project within a caller's explicit memory budget.
/// `path` selects the read-only source and `bytes` limits its serialized size.
/// Returns the shared typed document, or a visible limit/validation error.
pub fn load_bounded(path: impl AsRef<Path>, bytes: u64) -> Result<Project> {
    let mut data = Vec::new();
    File::open(path)?
        .take(bytes.saturating_add(1))
        .read_to_end(&mut data)?;
    if data.len() as u64 > bytes {
        return Err(Error::Invalid(format!(
            "project exceeds its {bytes}-byte read budget"
        )));
    }
    let project: Project = serde_json::from_slice(&data)?;
    project.migrate()
}

pub fn save(project: &Project, path: impl AsRef<Path>) -> Result<()> {
    write_project(project, path.as_ref(), false, None)
}

/// Create a project without replacing an existing destination.
pub fn save_new(project: &Project, path: impl AsRef<Path>) -> Result<()> {
    write_project(project, path.as_ref(), true, None)
}

/// Save an edit only while the project on disk still matches the loaded version.
pub fn save_if_unchanged(
    project: &Project,
    path: impl AsRef<Path>,
    expected: &Project,
) -> Result<()> {
    expected.validate()?;
    if project.id != expected.id
        || project.revision < expected.revision
        || (project != expected && project.revision == expected.revision)
    {
        return Err(Error::Invalid(
            "an edit must retain identity and advance its revision when changed".into(),
        ));
    }
    write_project(project, path.as_ref(), false, Some(expected))
}

fn write_project(
    project: &Project,
    path: &Path,
    new_only: bool,
    expected: Option<&Project>,
) -> Result<()> {
    project.validate()?;
    let mut bytes = serialize_bounded(project, true)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes, new_only, expected)
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn absolute_destination(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .ok_or_else(|| Error::Invalid("destination needs a filename".into()))?;
    let mut ancestor = parent(path);
    let mut missing = Vec::new();
    let mut absolute = loop {
        match fs::canonicalize(ancestor) {
            Ok(absolute) => break absolute,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let component = ancestor.components().next_back().ok_or(error)?;
                missing.push(component.as_os_str().to_owned());
                ancestor = ancestor
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
            }
            Err(error) => return Err(error.into()),
        }
    };
    for component in missing.into_iter().rev() {
        if component == ".." {
            absolute.pop();
        } else if component != "." {
            absolute.push(component);
        }
    }
    Ok(absolute.join(filename))
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn create_parents(path: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut candidate = parent(path);
    while !candidate.is_dir() {
        missing.push(candidate.to_owned());
        let next = parent(candidate);
        if next == candidate {
            return Err(Error::Invalid(
                "destination has no existing ancestor directory".into(),
            ));
        }
        candidate = next;
    }
    for directory in missing.into_iter().rev() {
        match fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists && directory.is_dir() => {}
            Err(error) => return Err(error.into()),
        }
        File::open(parent(&directory))?.sync_all()?;
    }
    Ok(())
}

fn atomic_write(
    path: &Path,
    bytes: &[u8],
    new_only: bool,
    expected: Option<&Project>,
) -> Result<()> {
    create_parents(path)?;
    let filename = path
        .file_name()
        .ok_or_else(|| Error::Invalid("destination needs a filename".into()))?;
    let mut lock_name = OsString::from(".");
    lock_name.push(filename);
    lock_name.push(".lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(parent(path).join(lock_name))?;
    lock.try_lock_exclusive().map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Error::Busy(path.to_owned())
        } else {
            Error::Io(error)
        }
    })?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(meta) => Some(meta),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if new_only && metadata.is_some() {
        return Err(Error::Exists(path.to_owned()));
    }
    if metadata.as_ref().is_some_and(|meta| !meta.is_file()) {
        return Err(Error::Invalid(
            "destination must be a regular file, not a symlink or directory".into(),
        ));
    }
    if let Some(expected) = expected
        && (metadata.is_none() || load(path)? != *expected)
    {
        return Err(Error::Conflict(path.to_owned()));
    }
    let temporary = parent(path).join(format!(".editbay-write-{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = private_options().open(&temporary)?;
        if let Some(metadata) = metadata {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if new_only {
            fs::hard_link(&temporary, path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::Exists(path.to_owned())
                } else {
                    Error::Io(error)
                }
            })?;
            fs::remove_file(&temporary)?;
        } else {
            fs::rename(&temporary, path)?;
        }
        File::open(parent(path))?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    // The persistent sidecar avoids replacing a lock inode while another writer waits.
    drop(lock);
    result
}

pub fn checkpoint(
    project: &Project,
    original: Option<&Path>,
    root: impl AsRef<Path>,
) -> Result<PathBuf> {
    prepare_checkpoint(project, original, root)?.commit()
}

pub struct PreparedCheckpoint {
    temporary: PathBuf,
    destination: PathBuf,
}

impl PreparedCheckpoint {
    /// Publish a completely prepared immutable snapshot.
    /// Consumes this preparation after the caller checks document ownership.
    /// Returns the synchronized checkpoint path; an existing destination is refused.
    pub fn commit(self) -> Result<PathBuf> {
        fs::hard_link(&self.temporary, &self.destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Error::Exists(self.destination.clone())
            } else {
                Error::Io(error)
            }
        })?;
        fs::remove_file(&self.temporary)?;
        File::open(parent(&self.destination))?.sync_all()?;
        Ok(self.destination.clone())
    }
}

impl Drop for PreparedCheckpoint {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.temporary);
    }
}

/// Prepare recovery bytes without publishing an obsolete worker result.
/// `project` is an immutable validated revision, `original` its optional saved
/// path, and `root` the recovery directory. Returns a synchronized private file;
/// dropping it removes the file, while commit publishes after an ownership check.
pub fn prepare_checkpoint(
    project: &Project,
    original: Option<&Path>,
    root: impl AsRef<Path>,
) -> Result<PreparedCheckpoint> {
    project.validate()?;
    let original = original.map(absolute_destination).transpose()?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::Invalid(error.to_string()))?
        .as_millis();
    let snapshot = Snapshot {
        schema: RECOVERY_SCHEMA,
        saved_at_unix_ms: u64::try_from(millis)
            .map_err(|error| Error::Invalid(error.to_string()))?,
        original,
        project: project.clone(),
    };
    let sha256 = format!("{:x}", Sha256::digest(serialize_bounded(&snapshot, false)?));
    let envelope = Envelope { snapshot, sha256 };
    let bytes = serialize_bounded(&envelope, true)?;
    let path = root.as_ref().join(project.id.to_string()).join(format!(
        "{}-{}.checkpoint",
        project.revision,
        Uuid::new_v4()
    ));
    create_parents(&path)?;
    let prepared = PreparedCheckpoint {
        temporary: parent(&path).join(format!(".editbay-write-{}.tmp", Uuid::new_v4())),
        destination: path,
    };
    let mut file = private_options().open(&prepared.temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    Ok(prepared)
}

fn read_snapshot(path: &Path) -> Result<Snapshot> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(Error::Invalid(
            "recovery exceeds its 64 MiB read budget".into(),
        ));
    }
    let mut envelope: Envelope = serde_json::from_slice(&bytes)?;
    if envelope.snapshot.schema != RECOVERY_SCHEMA {
        return Err(Error::Invalid(format!(
            "unsupported recovery schema {}",
            envelope.snapshot.schema
        )));
    }
    let digest = format!(
        "{:x}",
        Sha256::digest(serialize_bounded(&envelope.snapshot, false)?)
    );
    if digest != envelope.sha256 {
        return Err(Error::Integrity);
    }
    envelope.snapshot.project = envelope.snapshot.project.migrate()?;
    Ok(envelope.snapshot)
}

struct JsonBuffer {
    bytes: Vec<u8>,
}

impl Write for JsonBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) >= MAX_DOCUMENT_BYTES as usize {
            return Err(std::io::Error::other(
                "document exceeds its 64 MiB serialization budget",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn serialize_bounded(value: &impl Serialize, pretty: bool) -> Result<Vec<u8>> {
    let mut buffer = JsonBuffer { bytes: Vec::new() };
    if pretty {
        serde_json::to_writer_pretty(&mut buffer, value)?;
    } else {
        serde_json::to_writer(&mut buffer, value)?;
    }
    Ok(buffer.bytes)
}

/// Recover into a new independent project; never replace the original or another file.
pub fn recover_copy(snapshot: impl AsRef<Path>, destination: impl AsRef<Path>) -> Result<Project> {
    let snapshot = read_snapshot(snapshot.as_ref())?;
    let destination = destination.as_ref();
    create_parents(destination)?;
    if snapshot.original.as_ref().is_some_and(|original| {
        absolute_destination(destination).is_ok_and(|candidate| &candidate == original)
    }) {
        return Err(Error::OriginalDestination);
    }
    let mut project = snapshot.project;
    project.recovered_from = Some(project.id);
    project.id = Uuid::new_v4();
    save_new(&project, destination)?;
    Ok(project)
}

pub fn recovery_catalog(root: impl AsRef<Path>) -> Result<RecoveryCatalog> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RecoveryCatalog::default());
        }
        Err(error) => return Err(error.into()),
    };
    let mut catalog = RecoveryCatalog::default();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        for candidate in fs::read_dir(entry.path())? {
            let candidate = candidate?;
            let path = candidate.path();
            if !candidate.file_type()?.is_file()
                || path.extension().is_none_or(|ext| ext != "checkpoint")
            {
                continue;
            }
            match read_snapshot(&path) {
                Ok(snapshot) => catalog.valid.push(RecoveryRecord {
                    path,
                    project_id: snapshot.project.id,
                    name: snapshot.project.name,
                    revision: snapshot.project.revision,
                    saved_at_unix_ms: snapshot.saved_at_unix_ms,
                    original: snapshot.original,
                    width: snapshot.project.sequences[0].width,
                    height: snapshot.project.sequences[0].height,
                    frame_rate: snapshot.project.sequences[0].frame_rate,
                    sequence_count: snapshot.project.sequences.len(),
                }),
                Err(error) => catalog.invalid.push(RecoveryFailure {
                    path,
                    error: error.to_string(),
                }),
            }
        }
    }
    catalog.valid.sort_by(|a, b| {
        (b.saved_at_unix_ms, b.revision, &b.path).cmp(&(a.saved_at_unix_ms, a.revision, &a.path))
    });
    catalog.invalid.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(catalog)
}
