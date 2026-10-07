use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Component, Path, PathBuf},
    process::ExitCode,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    version: String,
    architecture: String,
    files: Vec<Entry>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: PathBuf,
    bytes: u64,
    sha256: String,
    executable: bool,
}

/// Hash one bounded file while retaining its read descriptor.
/// `path` and `limit` define the read. Returns a digest, length and rewound file.
fn hash(path: &Path, limit: u64) -> Result<(String, u64, File)> {
    use std::io::{Seek, SeekFrom};
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(format!("Not a regular bundle file: {}", path.display()).into());
    }
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    let mut size = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .filter(|size| *size <= limit)
            .ok_or("Bundle file exceeds its read limit")?;
        digest.update(&buffer[..count]);
    }
    file.seek(SeekFrom::Start(0))?;
    Ok((format!("{:x}", digest.finalize()), size, file))
}

/// Read and validate a finite release manifest.
/// `directory` owns its manifest and files; `expected` optionally pins its hash.
/// Returns declared entries after geometry, architecture and path checks.
fn manifest(directory: &Path, expected: Option<&str>) -> Result<Bundle> {
    let (sha, _, mut file) = hash(&directory.join("manifest.json"), 1024 * 1024)?;
    if expected.is_some_and(|expected| expected != sha) {
        return Err("Release manifest checksum does not match".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let bundle: Bundle = serde_json::from_slice(&bytes)?;
    if bundle.version.is_empty()
        || bundle.version.len() > 64
        || !bundle
            .version
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-_".contains(&c))
        || bundle.architecture != std::env::consts::ARCH
        || bundle.files.is_empty()
        || bundle.files.len() > 2048
    {
        return Err("Release version, architecture or entry count is invalid".into());
    }
    let mut paths = HashSet::new();
    let mut bytes = 0u64;
    for entry in &bundle.files {
        if entry.path.as_os_str().is_empty()
            || !entry
                .path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            || entry.path == Path::new("manifest.json")
            || !paths.insert(&entry.path)
            || entry.sha256.len() != 64
            || !entry.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err("Release contains an unsafe or duplicate path or invalid checksum".into());
        }
        bytes = bytes
            .checked_add(entry.bytes)
            .filter(|bytes| *bytes <= 4 * 1024 * 1024 * 1024)
            .ok_or("Release exceeds its size limit")?;
    }
    for required in [
        "bin/editbay",
        "bin/editbay-studio",
        "bin/editbay-cli",
        "bin/editbay-install",
    ] {
        if !bundle
            .files
            .iter()
            .any(|entry| entry.path == Path::new(required) && entry.executable)
        {
            return Err(format!("Release is missing {required}").into());
        }
    }
    Ok(bundle)
}

/// Verify every declared installed byte and executable permission.
/// `directory` and `bundle` identify one immutable version. Returns integrity.
fn verify(directory: &Path, bundle: &Bundle) -> Result<()> {
    for entry in &bundle.files {
        let path = directory.join(&entry.path);
        let (sha, bytes, file) = hash(&path, entry.bytes)?;
        if sha != entry.sha256
            || bytes != entry.bytes
            || (file.metadata()?.permissions().mode() & 0o111 != 0) != entry.executable
        {
            return Err(
                format!("Release file failed verification: {}", entry.path.display()).into(),
            );
        }
    }
    Ok(())
}

/// Replace a version selector atomically and synchronize its directory.
/// `prefix`, `name` and relative `target` select a link. Returns durable publication.
fn select(prefix: &Path, name: &str, target: &Path) -> Result<()> {
    let temporary = prefix.join(format!(".{name}-{}", uuid::Uuid::new_v4()));
    symlink(target, &temporary)?;
    if let Err(error) = fs::rename(&temporary, prefix.join(name)) {
        fs::remove_file(temporary)?;
        return Err(error.into());
    }
    File::open(prefix)?.sync_all()?;
    Ok(())
}

/// Inspect a local version selector without following an arbitrary outside link.
/// `prefix` and `name` identify current/previous; returns a confined relative target.
fn selected(prefix: &Path, name: &str) -> Result<Option<PathBuf>> {
    let target = match fs::read_link(prefix.join(name)) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let parts: Vec<_> = target.components().collect();
    if parts.len() != 2
        || parts[0] != Component::Normal("versions".as_ref())
        || !matches!(parts[1], Component::Normal(_))
    {
        return Err("Installed selector leaves the version directory".into());
    }
    Ok(Some(target))
}

/// Install a checksum-pinned release before switching the current selector.
/// `bundle_path`, `prefix` and `expected` identify reviewed bytes and installation.
/// Returns the installed version. Existing versions, projects and recovery files
/// are preserved; any copy or integrity failure leaves current unchanged.
fn install(bundle_path: &Path, prefix: &Path, expected: &str) -> Result<String> {
    let source = fs::canonicalize(bundle_path)?;
    let bundle = manifest(&source, Some(expected))?;
    verify(&source, &bundle)?;
    fs::create_dir_all(prefix.join("versions"))?;
    let target = PathBuf::from("versions").join(&bundle.version);
    let destination = prefix.join(&target);
    if destination.exists() {
        let (sha, _, _) = hash(&destination.join("manifest.json"), 1024 * 1024)?;
        if sha != expected {
            return Err("Version already exists with different contents".into());
        }
        verify(&destination, &bundle)?;
    } else {
        let staging = prefix
            .join("versions")
            .join(format!(".staging-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&staging)?;
        let copied = (|| -> Result<()> {
            for entry in &bundle.files {
                let output = staging.join(&entry.path);
                fs::create_dir_all(output.parent().ok_or("Bundle parent is absent")?)?;
                let (_, _, mut input) = hash(&source.join(&entry.path), entry.bytes)?;
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output)?;
                std::io::copy(&mut input, &mut file)?;
                file.set_permissions(fs::Permissions::from_mode(if entry.executable {
                    0o755
                } else {
                    0o644
                }))?;
                file.sync_all()?;
            }
            let data = fs::read(source.join("manifest.json"))?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(staging.join("manifest.json"))?;
            file.write_all(&data)?;
            file.sync_all()?;
            manifest(&staging, Some(expected))?;
            verify(&staging, &bundle)?;
            sync_directories(&staging)?;
            fs::rename(&staging, &destination)?;
            File::open(prefix.join("versions"))?.sync_all()?;
            Ok(())
        })();
        if copied.is_err() && staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        copied?;
    }
    if let Some(current) = selected(prefix, "current")?
        && current != target
    {
        select(prefix, "previous", &current)?;
    }
    select(prefix, "current", &target)?;
    Ok(bundle.version)
}

/// Verify the previous release and atomically restore it.
/// `prefix` identifies the installation. Returns the restored version, preserving
/// the displaced version for another rollback and all user projects and recovery.
fn rollback(prefix: &Path) -> Result<String> {
    let previous = selected(prefix, "previous")?.ok_or("No previous release is installed")?;
    let current = selected(prefix, "current")?.ok_or("No current release is installed")?;
    if current == previous {
        return Err("Previous selector is already current".into());
    }
    let directory = prefix.join(&previous);
    let bundle = manifest(&directory, None)?;
    verify(&directory, &bundle)?;
    select(prefix, "current", &previous)?;
    select(prefix, "previous", &current)?;
    Ok(bundle.version)
}

/// Freeze built binaries and their native library closure into a new release.
/// `build`, `destination` and `version` select local Cargo outputs and new package.
/// Returns the manifest digest. The package uses host glibc, GPU drivers and
/// PipeWire service; it supplies codec, UI and audio client shared libraries.
fn pack(build: &Path, destination: &Path, version: &str) -> Result<String> {
    fs::create_dir(destination)?;
    fs::create_dir(destination.join("bin"))?;
    fs::create_dir(destination.join("lib"))?;
    let mut libraries = std::collections::BTreeSet::new();
    for (source, name) in [
        ("editbay-studio", "editbay-studio"),
        ("editbay", "editbay-cli"),
        ("editbay-install", "editbay-install"),
    ] {
        let path = build.join(source);
        libraries.extend(native_libraries(&path)?);
        fs::copy(path, destination.join("bin").join(name))?;
    }
    let libdir = std::process::Command::new("pkg-config")
        .args(["--variable=libdir", "libpipewire-0.3"])
        .output()?;
    if !libdir.status.success() {
        return Err("Cannot resolve PipeWire client modules".into());
    }
    let libdir = PathBuf::from(String::from_utf8(libdir.stdout)?.trim());
    let mut plugins = Vec::new();
    for name in [
        "rt",
        "protocol-native",
        "client-node",
        "client-device",
        "adapter",
        "metadata",
        "session-manager",
    ] {
        plugins.push(PathBuf::from("pipewire-0.3").join(format!("libpipewire-module-{name}.so")));
    }
    for name in ["audioconvert", "support", "videoconvert"] {
        plugins.push(
            PathBuf::from("spa-0.2")
                .join(name)
                .join(format!("libspa-{name}.so")),
        );
    }
    for plugin in plugins {
        let source = libdir.join(&plugin);
        let output = destination.join("lib").join(&plugin);
        fs::create_dir_all(output.parent().ok_or("Plugin parent is absent")?)?;
        fs::copy(&source, output)?;
        libraries.extend(native_libraries(&source)?);
        libraries.insert(source);
    }
    for library in &libraries {
        fs::copy(
            library,
            destination
                .join("lib")
                .join(library.file_name().ok_or("Library name is absent")?),
        )?;
    }
    fs::create_dir_all(destination.join("share/pipewire"))?;
    fs::copy(
        "/usr/share/pipewire/client.conf",
        destination.join("share/pipewire/client.conf"),
    )?;
    fs::create_dir_all(destination.join("share/fonts"))?;
    fs::write(
        destination.join("share/fonts/LiberationSans-Regular.ttf"),
        include_bytes!("../../../assets/liberation/LiberationSans-Regular.ttf"),
    )?;
    fs::write(
        destination.join("share/fonts/LICENSE.txt"),
        include_bytes!("../../../assets/liberation/LICENSE.txt"),
    )?;
    fs::create_dir_all(destination.join("share/editbay/audio"))?;
    fs::write(
        destination.join("share/editbay/audio/residency.conf"),
        include_bytes!("../../../assets/audio/residency.conf"),
    )?;
    fs::write(
        destination.join("share/editbay/audio/memlock-service.conf"),
        include_bytes!("../../../assets/audio/memlock-service.conf"),
    )?;
    let launcher = "#!/bin/sh\nset -eu\nrelease_dir=$(CDPATH= cd -- \"$(dirname -- \"$(readlink -f -- \"$0\")\")/..\" && pwd -P)\nexport LD_LIBRARY_PATH=\"$release_dir/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}\"\nexport PIPEWIRE_MODULE_DIR=\"$release_dir/lib/pipewire-0.3\"\nexport EDITBAY_TITLE_FONT=\"$release_dir/share/fonts/LiberationSans-Regular.ttf\"\nexport SPA_PLUGIN_DIR=\"$release_dir/lib/spa-0.2\"\nexport PIPEWIRE_CONFIG_DIR=\"$release_dir/share/pipewire\"\ncase ${1-} in\n  new|info|rename|apply|timeline|build-job|archive|clips|migrate|frame-plan|probe-media|ingest|decode-frame|export|export-range|export-profile|checkpoint|recoveries|recover|--help|-h|--version|-V) exec \"$release_dir/bin/editbay-cli\" \"$@\" ;;\n  *) exec \"$release_dir/bin/editbay-studio\" \"$@\" ;;\nesac\n";
    fs::write(destination.join("bin/editbay"), launcher)?;
    fs::set_permissions(
        destination.join("bin/editbay"),
        fs::Permissions::from_mode(0o755),
    )?;
    let metadata = std::process::Command::new("ldd")
        .arg("--version")
        .output()?;
    fs::write(destination.join("host-runtime.txt"), metadata.stdout)?;
    let mut packages = std::collections::BTreeSet::new();
    for library in &libraries {
        let owner = std::process::Command::new("pacman")
            .arg("-Qoq")
            .arg(library)
            .output()?;
        if owner.status.success() {
            packages.extend(String::from_utf8(owner.stdout)?.lines().map(str::to_owned));
        }
    }
    let mut provenance = String::new();
    for package in packages {
        let info = std::process::Command::new("pacman")
            .arg("-Qi")
            .arg(&package)
            .output()?;
        if !info.status.success() {
            return Err("Cannot retain native package provenance".into());
        }
        provenance.push_str(&String::from_utf8(info.stdout)?);
        let licenses = Path::new("/usr/share/licenses").join(&package);
        if licenses.exists() {
            copy_licenses(&licenses, &destination.join("licenses").join(&package), 0)?;
        }
    }
    fs::write(destination.join("native-packages.txt"), provenance)?;
    let common = ["/usr/share/licenses/spdx", "/usr/share/licenses/common"]
        .into_iter()
        .map(Path::new)
        .find(|path| path.is_dir())
        .ok_or("Installed shared license texts are absent")?;
    copy_licenses(common, &destination.join("licenses/common"), 0)?;
    rust_notices(destination)?;
    let mut paths = Vec::new();
    collect(destination, destination, &mut paths, 0)?;
    paths.sort();
    let mut files = Vec::new();
    for path in paths {
        let (sha256, bytes, file) = hash(&destination.join(&path), 4 * 1024 * 1024 * 1024)?;
        files.push(Entry {
            path,
            sha256,
            bytes,
            executable: file.metadata()?.permissions().mode() & 0o111 != 0,
        });
    }
    fs::write(
        destination.join("manifest.json"),
        serde_json::to_vec_pretty(&Bundle {
            version: version.into(),
            architecture: std::env::consts::ARCH.into(),
            files,
        })?,
    )?;
    let digest = hash(&destination.join("manifest.json"), 1024 * 1024)?.0;
    let bundle = manifest(destination, Some(&digest))?;
    verify(destination, &bundle)?;
    Ok(digest)
}

/// Resolve the shared library closure while retaining host loader requirements.
/// `path` is a built ELF file; returns all resolved non-glibc dependencies.
fn native_libraries(path: &Path) -> Result<std::collections::BTreeSet<PathBuf>> {
    let status = std::process::Command::new("ldd").arg(path).output()?;
    if !status.status.success() {
        return Err(format!("Cannot resolve native libraries for {}", path.display()).into());
    }
    let mut libraries = std::collections::BTreeSet::new();
    for line in String::from_utf8(status.stdout)?.lines() {
        if line.contains("not found") {
            return Err(format!("Missing native dependency: {line}").into());
        }
        let path = line
            .split("=>")
            .nth(1)
            .unwrap_or(line)
            .split_whitespace()
            .next()
            .unwrap_or("");
        if !path.starts_with('/') {
            continue;
        }
        let path = PathBuf::from(path);
        let name = path
            .file_name()
            .ok_or("Native library filename is absent")?
            .to_string_lossy();
        if [
            "libc.so",
            "libm.so",
            "libdl.so",
            "libpthread.so",
            "librt.so",
            "libresolv.so",
            "ld-linux",
            "libnss_",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        libraries.insert(path);
    }
    Ok(libraries)
}

/// Synchronize every directory before publishing an installed version.
/// `directory` is the verified staging tree; returns durable directory entries.
fn sync_directories(directory: &Path) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_directories(&entry.path())?;
        }
    }
    File::open(directory)?.sync_all()?;
    Ok(())
}

/// Retain bounded installed dependency licenses as ordinary package files.
/// `source`, `destination` and `depth` identify the traversal; returns copy status.
fn copy_licenses(source: &Path, destination: &Path, depth: usize) -> Result<()> {
    if depth > 8 {
        return Err("Native license tree exceeds depth limit".into());
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let output = destination.join(entry.file_name());
        if path.is_dir() {
            copy_licenses(&path, &output, depth + 1)?;
        } else if fs::metadata(&path)?.len() <= 1024 * 1024 {
            fs::copy(&path, output)?;
        }
    }
    Ok(())
}

/// Retain actual Rust dependency notices from the locked source workspace.
/// `destination` owns the new bundle. Returns copied notices and package provenance;
/// packaging runs from the EditBay checkout with its dependencies already fetched.
fn rust_notices(destination: &Path) -> Result<()> {
    let compiler = std::process::Command::new("rustc").arg("-vV").output()?;
    let compiler = String::from_utf8(compiler.stdout)?;
    let target = compiler
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("Rust compiler host target is absent")?;
    let metadata = std::process::Command::new("cargo")
        .args([
            "metadata",
            "--locked",
            "--offline",
            "--format-version",
            "1",
            "--filter-platform",
            target,
        ])
        .output()?;
    if !metadata.status.success() || metadata.stdout.len() > 16 * 1024 * 1024 {
        return Err(
            "Packaging needs the locked EditBay source workspace and cached dependencies".into(),
        );
    }
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout)?;
    let root = Path::new(
        metadata["workspace_root"]
            .as_str()
            .ok_or("Workspace root is absent")?,
    );
    let packages = metadata["packages"]
        .as_array()
        .ok_or("Rust package list is absent")?;
    if packages.len() > 2048 || !packages.iter().any(|p| p["name"] == "editbay-cli") {
        return Err("Packaging source is not a bounded EditBay workspace".into());
    }
    let licenses = destination.join("licenses");
    fs::write(
        licenses.join("EDITBAY-MIT.txt"),
        include_bytes!("../../../LICENSE"),
    )?;
    fs::write(
        licenses.join("PHOSPHOR-MIT.txt"),
        include_bytes!("../../../assets/phosphor/LICENSE-MIT"),
    )?;
    fs::write(
        licenses.join("OMADESIGN-MIT.txt"),
        include_bytes!("../../../vendor/OMADESIGN-LICENSE-MIT"),
    )?;
    fs::copy(
        root.join("Cargo.lock"),
        destination.join("source-Cargo.lock"),
    )?;
    let mut provenance = Vec::new();
    for package in packages {
        let name = package["name"]
            .as_str()
            .ok_or("Rust package name is absent")?;
        let version = package["version"]
            .as_str()
            .ok_or("Rust package version is absent")?;
        let path = Path::new(
            package["manifest_path"]
                .as_str()
                .ok_or("Rust manifest path is absent")?,
        );
        let folder = path.parent().ok_or("Rust manifest parent is absent")?;
        let directory = licenses.join("rust").join(format!("{name}-{version}"));
        fs::create_dir_all(&directory)?;
        fs::copy(path, directory.join("Cargo.toml"))?;
        if let Some(path) = package["license_file"].as_str() {
            let path = folder.join(path);
            let (_, _, mut source) = hash(&path, 1024 * 1024)?;
            std::io::copy(
                &mut source,
                &mut File::create_new(directory.join("declared-license.txt"))?,
            )?;
        }
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let label = entry.file_name().to_string_lossy().to_ascii_uppercase();
            if ["LICENSE", "COPYING", "NOTICE", "COPYRIGHT"]
                .iter()
                .any(|prefix| label.starts_with(prefix))
            {
                if entry.file_type()?.is_dir() {
                    copy_licenses(&entry.path(), &directory.join(entry.file_name()), 0)?;
                } else if entry.file_type()?.is_file() {
                    let (_, _, mut source) = hash(&entry.path(), 1024 * 1024)?;
                    std::io::copy(
                        &mut source,
                        &mut File::create_new(directory.join(entry.file_name()))?,
                    )?;
                }
            }
        }
        provenance.push(serde_json::json!({"name":name,"version":version,
            "license":package["license"],"source":package["source"],"repository":package["repository"]}));
    }
    fs::write(
        destination.join("rust-packages.json"),
        serde_json::to_vec_pretty(&provenance)?,
    )?;
    Ok(())
}

/// Enumerate bounded regular package files for manifest generation.
/// `root`, `directory`, `paths` and `depth` define traversal. Returns confined files.
fn collect(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>, depth: usize) -> Result<()> {
    if depth > 16 || paths.len() > 2048 {
        return Err("Release file tree exceeds limits".into());
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect(root, &entry.path(), paths, depth + 1)?;
        } else if entry.file_type()?.is_file() {
            paths.push(entry.path().strip_prefix(root)?.to_owned());
        } else {
            return Err("Release contains an unsupported file type".into());
        }
    }
    Ok(())
}

/// Dispatch one locked install, rollback or verification request.
/// Takes process arguments; returns a printable result or visible error.
fn run() -> Result<String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let [action, build, destination, version] = args.as_slice()
        && action == "pack"
    {
        return pack(
            Path::new(build),
            Path::new(destination),
            version.to_str().ok_or("Version must be UTF-8")?,
        );
    }
    let (action, prefix) = match args.as_slice() {
        [action, _, prefix, _] if action == "install" => ("install", PathBuf::from(prefix)),
        [action, prefix] if action == "rollback" => ("rollback", PathBuf::from(prefix)),
        [action, prefix] if action == "verify" => ("verify", PathBuf::from(prefix)),
        _ => return Err("Usage: editbay-install pack BUILD NEW_BUNDLE VERSION | install BUNDLE PREFIX MANIFEST_SHA256 | rollback PREFIX | verify PREFIX".into()),
    };
    fs::create_dir_all(&prefix)?;
    let prefix = fs::canonicalize(prefix)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(prefix.join(".install.lock"))?;
    lock.try_lock_exclusive()
        .map_err(|_| "Another installation owns this prefix")?;
    match action {
        "install" => install(
            Path::new(&args[1]),
            &prefix,
            args[3].to_str().ok_or("Checksum must be UTF-8")?,
        ),
        "rollback" => rollback(&prefix),
        _ => {
            let target = selected(&prefix, "current")?.ok_or("No current release is installed")?;
            let directory = prefix.join(target);
            let bundle = manifest(&directory, None)?;
            verify(&directory, &bundle)?;
            Ok(bundle.version)
        }
    }
}
fn main() -> ExitCode {
    match run() {
        Ok(version) => {
            println!("EditBay {version}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("editbay-install: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bundle(root: &Path, version: &str) -> String {
        fs::create_dir_all(root.join("bin")).unwrap();
        let mut files = vec![];
        for name in [
            "editbay",
            "editbay-studio",
            "editbay-cli",
            "editbay-install",
        ] {
            let path = PathBuf::from("bin").join(name);
            fs::write(
                root.join(&path),
                format!("Installer unit fixture {version} {name}"),
            )
            .unwrap();
            fs::set_permissions(root.join(&path), fs::Permissions::from_mode(0o755)).unwrap();
            let (sha256, bytes, _) = hash(&root.join(&path), 4096).unwrap();
            files.push(Entry {
                path,
                sha256,
                bytes,
                executable: true,
            });
        }
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&Bundle {
                version: version.into(),
                architecture: std::env::consts::ARCH.into(),
                files,
            })
            .unwrap(),
        )
        .unwrap();
        hash(&root.join("manifest.json"), 1024 * 1024).unwrap().0
    }
    #[test]
    fn corrupt_update_never_switches_and_rollback_preserves_user_data() {
        let directory = tempfile::tempdir().unwrap();
        let prefix = directory.path().join("installed");
        fs::create_dir_all(prefix.join("projects")).unwrap();
        fs::write(
            prefix.join("projects/client.editbay"),
            "User data remains unchanged",
        )
        .unwrap();
        let first = directory.path().join("first");
        let a = bundle(&first, "a");
        let second = directory.path().join("second");
        let b = bundle(&second, "b");
        install(&first, &prefix, &a).unwrap();
        fs::write(second.join("bin/editbay-cli"), "corrupt").unwrap();
        assert!(install(&second, &prefix, &b).is_err());
        assert_eq!(
            selected(&prefix, "current").unwrap().unwrap(),
            Path::new("versions/a")
        );
        let b = bundle(&second, "b");
        install(&second, &prefix, &b).unwrap();
        assert_eq!(rollback(&prefix).unwrap(), "a");
        assert_eq!(
            fs::read_to_string(prefix.join("projects/client.editbay")).unwrap(),
            "User data remains unchanged"
        );
        assert!(prefix.join("versions/b").exists());
    }
}
