use crate::{Error, Project, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const RECOVERY_SCHEMA: u32 = 1;

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
    let project: Project = serde_json::from_slice(&fs::read(path)?)?;
    project.validate()?;
    Ok(project)
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
    let mut bytes = serde_json::to_vec_pretty(project)?;
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
    Ok(fs::canonicalize(parent(path))?.join(filename))
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
    let sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(&snapshot)?));
    let envelope = Envelope { snapshot, sha256 };
    let path = root.as_ref().join(project.id.to_string()).join(format!(
        "{}-{}.checkpoint",
        project.revision,
        Uuid::new_v4()
    ));
    atomic_write(&path, &serde_json::to_vec_pretty(&envelope)?, true, None)?;
    Ok(path)
}

fn read_snapshot(path: &Path) -> Result<Snapshot> {
    let envelope: Envelope = serde_json::from_slice(&fs::read(path)?)?;
    if envelope.snapshot.schema != RECOVERY_SCHEMA {
        return Err(Error::Invalid(format!(
            "unsupported recovery schema {}",
            envelope.snapshot.schema
        )));
    }
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&envelope.snapshot)?)
    );
    if digest != envelope.sha256 {
        return Err(Error::Integrity);
    }
    envelope.snapshot.project.validate()?;
    Ok(envelope.snapshot)
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
