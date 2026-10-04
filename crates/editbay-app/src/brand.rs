//! Versioned local asset banks. Call every filesystem operation on a worker.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

const MANIFEST: &str = "editbay.v1.json";
const MAX_FILE: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Font,
    Logo,
    Artwork,
    Title,
    Lut,
}

impl Category {
    /// Name the portable folder for one asset category.
    /// Takes no arguments; returns a fixed shared or EditBay-owned folder name.
    pub fn folder(self) -> &'static str {
        match self {
            Self::Font => "fonts",
            Self::Logo => "logos",
            Self::Artwork => "artwork",
            Self::Title => "editbay-titles",
            Self::Lut => "luts",
        }
    }

    /// Describe a stored asset in the local bank.
    /// Takes no arguments and returns a plain label without claiming an importer.
    pub fn label(self) -> &'static str {
        match self {
            Self::Font => "Font",
            Self::Logo => "Logo",
            Self::Artwork => "Artwork",
            Self::Title => "Title package",
            Self::Lut => "LUT",
        }
    }

    fn accepts(self, path: &Path) -> bool {
        let extension = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        match self {
            Self::Font => matches!(extension.as_str(), "ttf" | "otf" | "ttc"),
            Self::Logo | Self::Artwork => matches!(
                extension.as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff" | "bmp" | "gif" | "svg" | "oma"
            ),
            Self::Title => matches!(
                extension.as_str(),
                "oma" | "editbay" | "editbay-title" | "zip"
            ),
            Self::Lut => matches!(
                extension.as_str(),
                "cube" | "3dl" | "spi1d" | "spi3d" | "clf" | "ctf"
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    pub name: String,
    pub colors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetReceipt {
    pub path: PathBuf,
    pub category: Category,
    pub sha256: String,
    pub bytes: u64,
    pub source_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub schema: u32,
    pub revision: u64,
    pub client: String,
    pub project: String,
    pub notes: String,
    pub palettes: Vec<Palette>,
    pub assets: Vec<AssetReceipt>,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            schema: 1,
            revision: 0,
            client: String::new(),
            project: String::new(),
            notes: String::new(),
            palettes: Vec::new(),
            assets: Vec::new(),
        }
    }
}

impl Metadata {
    /// Check the independent EditBay bank schema and portable references.
    /// Takes no arguments; returns an error before any bytes are published.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!("Unsupported EditBay brand schema {}", self.schema));
        }
        if self.client.len() > 256
            || self.project.len() > 256
            || self.notes.len() > 8192
            || self.palettes.len() > 64
            || self.assets.len() > 2048
        {
            return Err("Brand metadata exceeds its storage budget".into());
        }
        let mut names = HashSet::new();
        for palette in &self.palettes {
            if palette.name.trim().is_empty()
                || palette.name.len() > 128
                || !names.insert(&palette.name)
                || palette.colors.is_empty()
                || palette.colors.len() > 64
                || palette.colors.iter().any(|color| {
                    color.len() != 7
                        || !color.starts_with('#')
                        || !color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
                })
            {
                return Err("Palettes need a unique name and 1–64 #RRGGBB colors".into());
            }
        }
        let mut paths = HashSet::new();
        for asset in &self.assets {
            relative(&asset.path)?;
            if !paths.insert(&asset.path)
                || asset.sha256.len() != 64
                || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                || asset.bytes > MAX_FILE
                || asset.source_name.len() > 1024
                || !asset.category.accepts(&asset.path)
                || !asset.path.starts_with(asset.category.folder())
            {
                return Err("Invalid or duplicate brand asset receipt".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub path: PathBuf,
    pub bytes: u64,
    pub category: Category,
    pub verified_sha256: String,
}

#[derive(Clone, Debug)]
pub struct Catalog {
    pub root: PathBuf,
    pub name: String,
    pub metadata: Option<Metadata>,
    pub version: Option<BankVersion>,
    pub assets: Vec<Asset>,
    pub warnings: Vec<String>,
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BankVersion {
    device: u64,
    inode: u64,
    manifest: Option<String>,
}

fn ownership(root: &Path, manifest: Option<String>) -> Result<BankVersion, String> {
    let info = fs::symlink_metadata(root).map_err(|e| e.to_string())?;
    if !info.is_dir() {
        return Err("Brand directory was replaced".into());
    }
    Ok(BankVersion {
        device: info.dev(),
        inode: info.ino(),
        manifest,
    })
}

fn relative(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Brand references must stay inside their bank".into());
    }
    Ok(())
}

fn root(folder: &Path) -> Result<PathBuf, String> {
    let path = if folder.file_name().is_some_and(|name| name == ".omabrand") {
        folder.to_owned()
    } else {
        folder.join(".omabrand")
    };
    if !fs::symlink_metadata(&path)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        return Err(
            "A brand bank must be a regular .omabrand directory; legacy markers remain untouched"
                .into(),
        );
    }
    path.canonicalize().map_err(|e| e.to_string())
}

fn safe(root: &Path, path: &Path) -> Result<PathBuf, String> {
    relative(path)?;
    let mut current = root.to_owned();
    for component in path.components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!("Brand symlink is excluded: {}", path.display()));
        }
    }
    if !current.starts_with(root) {
        return Err("Brand path escaped its bank".into());
    }
    Ok(current)
}

fn read_bytes(path: &Path, budget: u64) -> Result<Vec<u8>, String> {
    read_data(path, budget, None)
}

fn read_data(path: &Path, budget: u64, cancel: Option<&AtomicBool>) -> Result<Vec<u8>, String> {
    let before = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !before.is_file() {
        return Err("Expected a regular asset or manifest file".into());
    }
    let mut bytes = Vec::with_capacity(before.len().min(budget) as usize);
    let file = File::open(path).map_err(|e| e.to_string())?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    if (before.dev(), before.ino()) != (opened.dev(), opened.ino()) {
        return Err("File changed before reading".into());
    }
    let mut file = file.take(budget + 1);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return Err("Brand work cancelled".into());
        }
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > budget {
            return Err(format!("File exceeds its {budget}-byte budget"));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let after = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !after.is_file()
        || (
            before.dev(),
            before.ino(),
            before.len(),
            before.modified().ok(),
        ) != (
            after.dev(),
            after.ino(),
            bytes.len() as u64,
            after.modified().ok(),
        )
    {
        return Err("File changed while reading".into());
    }
    Ok(bytes)
}

fn metadata(root: &Path) -> Result<(Metadata, Option<String>), String> {
    if fs::symlink_metadata(root.join(MANIFEST))
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok((Metadata::default(), None));
    }
    let bytes = read_bytes(&safe(root, Path::new(MANIFEST))?, 1024 * 1024)?;
    let metadata: Metadata = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    metadata.validate()?;
    Ok((metadata, Some(format!("{:x}", Sha256::digest(bytes)))))
}

fn sync(folder: &Path) -> Result<(), String> {
    File::open(folder)
        .map_err(|e| e.to_string())?
        .sync_all()
        .map_err(|e| e.to_string())
}

fn publish(path: &Path, bytes: &[u8], replace: bool) -> Result<(), String> {
    let parent = path.parent().ok_or("Bank file needs a parent")?;
    let temporary = parent.join(format!(".editbay-bank-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        if replace {
            fs::rename(&temporary, path).map_err(|e| e.to_string())?;
        } else {
            fs::hard_link(&temporary, path).map_err(|e| e.to_string())?;
            fs::remove_file(&temporary).map_err(|e| e.to_string())?;
        }
        sync(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn lock(root: &Path) -> Result<File, String> {
    let path = root.join(".editbay-bank.lock");
    if fs::symlink_metadata(&path).is_ok_and(|m| !m.is_file()) {
        return Err("Brand lock must be a regular file".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.try_lock_exclusive()
        .map_err(|e| format!("Brand writer is busy: {e}"))?;
    Ok(file)
}

fn hash(path: &Path, cancel: &AtomicBool) -> Result<(String, u64), String> {
    let before = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !before.is_file() || before.len() > MAX_FILE {
        return Err("Asset must be a regular file of at most 256 MiB".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    if (before.dev(), before.ino()) != (opened.dev(), opened.ino()) {
        return Err("Asset changed before verification".into());
    }
    let mut digest = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Brand work cancelled".into());
        }
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > MAX_FILE {
            return Err("Asset grew beyond the 256 MiB budget".into());
        }
        digest.update(&buffer[..count]);
    }
    let after = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !after.is_file()
        || before.len() != bytes
        || before.modified().ok() != after.modified().ok()
        || (before.dev(), before.ino()) != (after.dev(), after.ino())
    {
        return Err("Asset changed while it was being verified".into());
    }
    Ok((format!("{:x}", digest.finalize()), bytes))
}

/// Create compatible bank metadata without replacing an existing shared manifest.
/// `folder` is a client folder or bank directory and `name` names a new bank.
/// Returns the bank path; existing legacy marker files and schemas stay intact.
pub fn create(folder: &Path, name: &str) -> Result<PathBuf, String> {
    if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err("Choose a bank name of 1–256 bytes".into());
    }
    let parent = folder.canonicalize().map_err(|e| e.to_string())?;
    let bank = if parent.file_name().is_some_and(|name| name == ".omabrand") {
        parent.clone()
    } else {
        parent.join(".omabrand")
    };
    match fs::create_dir(&bank) {
        Ok(()) => {
            sync(&parent)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }
    let bank = root(&bank)?;
    let _lock = lock(&bank)?;
    if fs::symlink_metadata(bank.join("brand.json"))
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        publish(
            &bank.join("brand.json"),
            &serde_json::to_vec_pretty(&serde_json::json!({"version":1,"name":name.trim()}))
                .map_err(|e| e.to_string())?,
            false,
        )?;
    }
    metadata(&bank)?;
    Ok(bank)
}

/// Verify and inspect shared assets without decoding or running their contents.
/// `folder` selects a bank; `cancel` ends reads between bounded chunks.
/// Returns real file hashes, validated palettes, and visible partial/error states.
pub fn scan(folder: &Path, cancel: &AtomicBool) -> Result<Catalog, String> {
    let root = root(folder)?;
    let mut catalog = Catalog {
        name: root
            .parent()
            .and_then(Path::file_name)
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        root: root.clone(),
        metadata: None,
        version: None,
        assets: Vec::new(),
        warnings: Vec::new(),
        complete: true,
    };
    if root.join("brand.json").exists() {
        match safe(&root, Path::new("brand.json"))
            .and_then(|path| read_bytes(&path, 16 * 1024))
            .and_then(|bytes| {
                serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| e.to_string())
            }) {
            Ok(value)
                if value["version"] == 1
                    && value["name"]
                        .as_str()
                        .is_some_and(|name| !name.trim().is_empty()) =>
            {
                catalog.name = value["name"].as_str().unwrap().to_owned();
            }
            Ok(_) => catalog
                .warnings
                .push("Unknown shared brand manifest; its bytes are preserved".into()),
            Err(error) => catalog
                .warnings
                .push(format!("Shared brand manifest: {error}")),
        }
    }
    match metadata(&root) {
        Ok((metadata, version)) => {
            catalog.metadata = Some(metadata);
            catalog.version = Some(ownership(&root, version)?);
        }
        Err(error) => catalog.warnings.push(format!(
            "EditBay brand metadata is read-only until repaired: {error}"
        )),
    }
    let mut pending = vec![root.clone()];
    let mut visited = 0;
    while let Some(folder) = pending.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Brand work cancelled".into());
        }
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) => {
                catalog.complete = false;
                catalog
                    .warnings
                    .push(format!("{}: {error}", folder.display()));
                continue;
            }
        };
        for entry in entries {
            visited += 1;
            if visited > MAX_ENTRIES || catalog.assets.len() >= 2048 {
                catalog.complete = false;
                catalog
                    .warnings
                    .push("Brand scan reached its 10,000-entry or 2,048-asset budget".into());
                return Ok(catalog);
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    catalog.complete = false;
                    catalog.warnings.push(error.to_string());
                    continue;
                }
            };
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let info = match fs::symlink_metadata(&path) {
                Ok(info) => info,
                Err(error) => {
                    catalog.complete = false;
                    catalog.warnings.push(error.to_string());
                    continue;
                }
            };
            if info.is_dir() {
                pending.push(path);
                continue;
            }
            if !info.is_file() {
                continue;
            }
            let relative_path = path
                .strip_prefix(&root)
                .map_err(|e| e.to_string())?
                .to_path_buf();
            let category = catalog
                .metadata
                .as_ref()
                .and_then(|metadata| {
                    metadata
                        .assets
                        .iter()
                        .find(|asset| asset.path == relative_path)
                        .map(|asset| asset.category)
                })
                .or_else(|| {
                    [Category::Font, Category::Lut, Category::Artwork]
                        .into_iter()
                        .find(|kind| kind.accepts(&path))
                });
            let Some(category) = category else {
                continue;
            };
            match safe(&root, &relative_path).and_then(|path| hash(&path, cancel)) {
                Ok((sha256, bytes)) => {
                    if let Some(receipt) = catalog.metadata.as_ref().and_then(|metadata| {
                        metadata
                            .assets
                            .iter()
                            .find(|asset| asset.path == relative_path)
                    }) && (receipt.sha256 != sha256 || receipt.bytes != bytes)
                    {
                        catalog.warnings.push(format!(
                            "{} changed since import; choose its new version explicitly",
                            relative_path.display()
                        ));
                    }
                    catalog.assets.push(Asset {
                        path: relative_path,
                        bytes,
                        category,
                        verified_sha256: sha256,
                    });
                }
                Err(error) => {
                    catalog.complete = false;
                    catalog
                        .warnings
                        .push(format!("{}: {error}", relative_path.display()));
                }
            }
        }
    }
    if let Some(metadata) = &catalog.metadata {
        for asset in &metadata.assets {
            if !catalog.assets.iter().any(|found| found.path == asset.path) {
                catalog.warnings.push(format!(
                    "Missing or unreadable imported asset: {}",
                    asset.path.display()
                ));
            }
        }
    }
    catalog.assets.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(catalog)
}

/// Publish client metadata and palettes through a checked, independently versioned write.
/// `folder` owns the bank, `expected` is its loaded manifest hash, and `updated`
/// contains the proposed metadata. Returns its new revision; shared files are untouched.
pub fn save_metadata(
    folder: &Path,
    expected: &BankVersion,
    mut updated: Metadata,
) -> Result<u64, String> {
    let root = root(folder)?;
    let _lock = lock(&root)?;
    let (current, version) = metadata(&root)?;
    if ownership(&root, version.clone())? != *expected
        || updated.revision != current.revision
        || updated.assets != current.assets
    {
        return Err("Brand metadata changed; refresh before editing".into());
    }
    updated.revision = current
        .revision
        .checked_add(1)
        .ok_or("Brand revision overflow")?;
    updated.validate()?;
    publish(
        &root.join(MANIFEST),
        &serde_json::to_vec_pretty(&updated).map_err(|e| e.to_string())?,
        version.is_some(),
    )?;
    Ok(updated.revision)
}

/// Copy an asset into its bank with a source hash and explicit category.
/// `folder` and `expected` bind ownership, `source` remains read-only, `category`
/// names its storage folder, and `cancel` stops the actual copy before publication.
/// Returns a verified receipt. Name collisions fail without replacing either file.
pub fn import(
    folder: &Path,
    expected: &BankVersion,
    source: &Path,
    category: Category,
    cancel: &AtomicBool,
) -> Result<AssetReceipt, String> {
    if !category.accepts(source) {
        return Err(format!("Unsupported {} file extension", category.label()));
    }
    let root = root(folder)?;
    let _lock = lock(&root)?;
    let (mut metadata, version) = metadata(&root)?;
    if ownership(&root, version.clone())? != *expected {
        return Err("Brand metadata changed; refresh before importing".into());
    }
    let (sha256, bytes) = hash(source, cancel)?;
    let name = source.file_name().ok_or("Asset needs a file name")?;
    let destination_folder = root.join(category.folder());
    match fs::create_dir(&destination_folder) {
        Ok(()) => sync(&root)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }
    if !fs::symlink_metadata(&destination_folder)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        return Err("Asset destination must be a regular directory".into());
    }
    let destination_folder = safe(&root, Path::new(category.folder()))?;
    let destination = destination_folder.join(name);
    let data = read_data(source, MAX_FILE, Some(cancel))?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Brand work cancelled".into());
    }
    if data.len() as u64 != bytes || format!("{:x}", Sha256::digest(&data)) != sha256 {
        return Err("Asset changed before import; retry its new version".into());
    }
    let receipt = AssetReceipt {
        path: PathBuf::from(category.folder()).join(name),
        category,
        sha256,
        bytes,
        source_name: name.to_string_lossy().into_owned(),
    };
    metadata.assets.push(receipt.clone());
    metadata.revision = metadata
        .revision
        .checked_add(1)
        .ok_or("Brand revision overflow")?;
    metadata.validate()?;
    let serialized = serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?;
    publish(&destination, &data, false)?;
    publish(&root.join(MANIFEST), &serialized, version.is_some()).map_err(|error| format!("Asset bytes were copied to {}; its receipt could not be saved: {error}. Refresh the bank to inspect the retained asset.", destination.display()))?;
    Ok(receipt)
}

/// Export the exact selected bank version to a new external file.
/// `folder` owns `asset`, its verified hash prevents a changed source export,
/// `destination` must not exist, and `cancel` ends the underlying verification.
/// Returns success only after synced bytes are published without overwriting.
pub fn export(
    folder: &Path,
    asset: &Asset,
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let root = root(folder)?;
    let source = safe(&root, &asset.path)?;
    let (sha256, bytes) = hash(&source, cancel)?;
    if sha256 != asset.verified_sha256 || bytes != asset.bytes {
        return Err("Asset changed; refresh and choose its new version".into());
    }
    let data = read_data(&source, MAX_FILE, Some(cancel))?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Brand work cancelled".into());
    }
    if format!("{:x}", Sha256::digest(&data)) != sha256 {
        return Err("Asset changed during export".into());
    }
    publish(destination, &data, false)
}
