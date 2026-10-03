use super::{Result, hash};
use editbay_media::VideoReader;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

/// Inventory local production sources without modifying them.
/// `root` is a regular directory; `destination` is a new report. Returns source
/// hashes and actual first-picture decode evidence, with explicit scan/decode failures.
pub(crate) fn run(root: &Path, destination: &Path) -> Result<Value> {
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err("inventory root must be a regular directory".into());
    }
    if destination.exists() {
        return Err("inventory report already exists".into());
    }
    let root = root.canonicalize()?;
    let mut pending = vec![(root.clone(), 0usize)];
    let mut candidates = Vec::new();
    let mut failures = Vec::new();
    let mut excluded = 0usize;
    let mut visited = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        if depth > 16 {
            failures.push(json!({"path":directory,"error":"directory depth exceeds 16"}));
            continue;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                failures.push(json!({"path":directory,"error":error.to_string()}));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    failures.push(json!({"path":directory,"error":error.to_string()}));
                    continue;
                }
            };
            visited += 1;
            if visited > 10000 {
                failures.push(json!({"path":root,"error":"inventory exceeds 10000 entries"}));
                pending.clear();
                break;
            }
            let path = entry.path();
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    failures.push(json!({"path":path,"error":error.to_string()}));
                    continue;
                }
            };
            if entry.file_name().as_encoded_bytes().starts_with(b".") || kind.is_symlink() {
                excluded += 1;
                continue;
            }
            if kind.is_dir() {
                pending.push((path, depth + 1));
                continue;
            }
            if !kind.is_file() {
                excluded += 1;
                continue;
            }
            let extension = path
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let video = matches!(
                extension.as_str(),
                "mp4" | "mov" | "mkv" | "webm" | "mxf" | "avi"
            );
            let still = matches!(
                extension.as_str(),
                "jpg" | "jpeg" | "png" | "tif" | "tiff" | "webp"
            );
            if video || still {
                candidates.push((path, video));
            } else {
                excluded += 1;
            }
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    let mut records = Vec::new();
    for (path, video) in candidates {
        match inspect(&root, &path, video) {
            Ok(record) => records.push(record),
            Err(error) => failures.push(json!({"path":path,"error":error.to_string()})),
        }
    }
    let report = json!({"schema":1,"kind":"local_production_source_inventory","root":root,
        "ffmpeg":editbay_media::version(),"sources":records,"failures":failures,
        "coverage_complete":failures.is_empty(),"entries_visited":visited,"excluded_entries":excluded,
        "policy":{"recursive_depth_limit":16,"entry_limit":10000,"symlinks_followed":false,"hidden_entries_included":false,
            "video_probe":"actual first decoded RGBA picture; not full-file playback/export qualification",
            "still_probe":"file bytes and checksum only; no still-image decode claim",
            "permission":"caller-selected local directory; read-only source access; no media publication or upload"}});
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec_pretty(&report)?)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(destination)?;
    File::open(parent)?.sync_all()?;
    Ok(
        json!({"report":destination,"report_sha256":hash(destination)?,"coverage_complete":report["coverage_complete"],
        "sources":report["sources"].as_array().unwrap().len(),"failures":report["failures"].as_array().unwrap().len()}),
    )
}

fn inspect(root: &Path, path: &Path, video: bool) -> Result<Value> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err("source changed from a regular file".into());
    }
    let source_hash = hash(path)?;
    let relative: PathBuf = path.strip_prefix(root)?.into();
    let mut record = json!({"path":relative,"bytes":metadata.len(),"source_sha256":source_hash,
        "kind":if video {"video"}else{"still"}});
    if video {
        let mut reader = VideoReader::open(path)?;
        let frame = reader
            .next_frame()?
            .ok_or("video contains no decoded pictures")?;
        record["video_info"] = serde_json::to_value(reader.info)?;
        record["first_picture"] = json!({"timestamp_ns":frame.timestamp_ns,"rgba_bytes":frame.rgba.len(),
            "rgba_sha256":format!("{:x}",Sha256::digest(&frame.rgba))});
    }
    let after = fs::symlink_metadata(path)?;
    if !after.is_file()
        || after.len() != metadata.len()
        || after.modified()? != metadata.modified()?
        || hash(path)? != source_hash
    {
        return Err("source changed during inventory".into());
    }
    Ok(record)
}
