use super::{PlanSummary, StoreBudget, StorePlan};
use crate::{
    Cancellation, Error, PictureBudget, PictureProvider, Result,
    pictures::{Key, PictureHeader, selection},
};
use editbay_core::{DocumentVersion, EvaluationSnapshot, SourceRequest};
use rustix::fs::{Mode, OFlags, fcntl_getfl, openat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, MetadataExt},
    },
    path::Path,
    sync::{Arc, Mutex},
};

/// A shared disk allowance retained by preparation jobs and every live reader.
#[derive(Clone)]
pub struct StoreSpace(Arc<Space>);
struct Space {
    budget: StoreBudget,
    used: Mutex<StoreUsage>,
}

/// Reserved payload bytes and entries, including unpublished work and reader pins.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct StoreUsage {
    pub bytes: u64,
    pub entries: usize,
}
struct Reservation {
    space: StoreSpace,
    usage: StoreUsage,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut used = self.space.0.used.lock().unwrap_or_else(|e| e.into_inner());
        used.bytes -= self.usage.bytes;
        used.entries -= self.usage.entries;
    }
}
impl StoreSpace {
    /// Set the total allowance shared by this workspace's prepared picture jobs.
    /// `budget` supplies ceilings. Returns an empty, lifetime-accounted allowance.
    pub fn new(budget: StoreBudget) -> Result<Self> {
        budget.validate()?;
        Ok(Self(Arc::new(Space {
            budget,
            used: Mutex::new(StoreUsage::default()),
        })))
    }

    /// Inspect actual reservations without reading or changing any cache file.
    /// Takes this allowance. Returns totals including consumer-held preparations.
    pub fn usage(&self) -> StoreUsage {
        *self.0.used.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reserve(&self, bytes: u64, entries: usize) -> Result<Reservation> {
        let mut used = self.0.used.lock().unwrap_or_else(|e| e.into_inner());
        let next = StoreUsage {
            bytes: used
                .bytes
                .checked_add(bytes)
                .filter(|n| *n <= self.0.budget.bytes)
                .ok_or_else(|| {
                    Error::Invalid("picture storage bytes are pinned by existing work".into())
                })?,
            entries: used
                .entries
                .checked_add(entries)
                .filter(|n| *n <= self.0.budget.entries)
                .ok_or_else(|| {
                    Error::Invalid("picture storage entries are pinned by existing work".into())
                })?,
        };
        *used = next;
        Ok(Reservation {
            space: self.clone(),
            usage: StoreUsage { bytes, entries },
        })
    }
}

/// Completed exact pictures and payload bytes in an unpublished preparation.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct StoreProgress {
    pub pictures: usize,
    pub total_pictures: usize,
    pub bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) version: DocumentVersion,
    bytes: u64,
    entries: Vec<Record>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    request: SourceRequest,
    offset: u64,
    header: PictureHeader,
    sha256: [u8; 32],
}

/// A verified, read-only anonymous pack; cloning retains its storage reservation.
#[derive(Clone)]
pub struct PreparedStore(Arc<Ready>);
struct Ready {
    file: File,
    manifest: Manifest,
    summary: PlanSummary,
    _reservation: Reservation,
}
impl PreparedStore {
    /// Inspect the captured range and actual prepared resource requirements.
    /// Takes this completed preparation. Returns its immutable plan receipt.
    pub fn summary(&self) -> &PlanSummary {
        &self.0.summary
    }

    pub(crate) fn manifest(&self) -> Manifest {
        self.0.manifest.clone()
    }
    pub(crate) fn file(&self) -> Result<File> {
        Ok(self.0.file.try_clone()?)
    }
}

impl StorePlan {
    /// Prepare exact source pixels through the existing bounded picture provider.
    /// `directory` hosts an anonymous file; `space` accounts disk pins; `provider`
    /// must own this snapshot. `cancel` and `progress` control background work.
    /// Returns ready storage only after all pixels and used sources are verified.
    pub fn prepare<P: PictureProvider>(
        self,
        directory: &Path,
        space: &StoreSpace,
        provider: &mut P,
        cancel: &Cancellation,
        mut progress: impl FnMut(StoreProgress),
    ) -> Result<PreparedStore> {
        check(cancel)?;
        let summary = self.summary();
        if provider.version() != summary.version {
            return Err(Error::Invalid(
                "picture preparation provider has foreign ownership".into(),
            ));
        }
        let reservation = space.reserve(self.bytes, self.requests.len())?;
        let directory = File::open(directory)?;
        let mut file = File::from(
            openat(
                &directory,
                ".",
                OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)?,
        );
        let mut manifest = Manifest {
            version: summary.version,
            bytes: self.bytes,
            entries: Vec::with_capacity(self.requests.len()),
        };
        let mut offset = 0;
        progress(StoreProgress {
            pictures: 0,
            total_pictures: summary.pictures,
            bytes: 0,
            total_bytes: self.bytes,
        });
        for request in self.requests.values() {
            check(cancel)?;
            let result = provider
                .picture(request)?
                .ok_or_else(|| Error::Invalid("planned source picture is absent".into()))?;
            provider.validate_result(&result)?;
            let picture = &result.picture;
            let mut digest = Sha256::new();
            for chunk in picture.rgba().chunks(64 * 1024) {
                check(cancel)?;
                file.write_all(chunk)?;
                digest.update(chunk);
            }
            manifest.entries.push(Record {
                request: request.clone(),
                offset,
                header: PictureHeader::of(picture),
                sha256: digest.finalize().into(),
            });
            offset += picture.rgba().len() as u64;
            progress(StoreProgress {
                pictures: manifest.entries.len(),
                total_pictures: summary.pictures,
                bytes: offset,
                total_bytes: self.bytes,
            });
        }
        if offset != self.bytes {
            return Err(Error::Invalid(
                "prepared picture lengths differ from their plan".into(),
            ));
        }
        provider.verify_sources()?;
        for asset in &self.snapshot.project().assets {
            if self.sources.contains(&asset.id) {
                let source = crate::SourceFile::open(&asset.path, cancel)?;
                if source.fingerprint().sha256 != asset.sha256
                    || source.fingerprint().bytes != asset.bytes
                {
                    return Err(Error::SourceChanged(asset.path.display().to_string()));
                }
            }
        }
        check(cancel)?;
        let reader = File::open(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
        drop(file);
        StoreReader::new(
            reader.try_clone()?,
            manifest.clone(),
            &self.snapshot,
            self.pictures,
            cancel,
        )?;
        check(cancel)?;
        Ok(PreparedStore(Arc::new(Ready {
            file: reader,
            manifest,
            summary,
            _reservation: reservation,
        })))
    }
}

pub(crate) struct StoreReader {
    file: File,
    entries: BTreeMap<Key, Record>,
}
impl StoreReader {
    pub(crate) fn new(
        file: File,
        manifest: Manifest,
        snapshot: &EvaluationSnapshot,
        budget: PictureBudget,
        cancel: &Cancellation,
    ) -> Result<Self> {
        check(cancel)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 0
            || metadata.len() != manifest.bytes
            || (fcntl_getfl(&file).map_err(std::io::Error::from)? & OFlags::ACCMODE)
                != OFlags::RDONLY
            || manifest.bytes > StoreBudget::default().bytes
            || manifest.entries.len() > StoreBudget::default().entries
            || manifest.version != DocumentVersion::of(snapshot.project())
        {
            return Err(Error::Invalid(
                "picture storage descriptor or document ownership is invalid".into(),
            ));
        }
        let mut entries = BTreeMap::new();
        let mut offset = 0u64;
        for record in manifest.entries {
            check(cancel)?;
            let selected = selection(snapshot, &record.request, budget)?.ok_or_else(|| {
                Error::Invalid("stored source picture is outside its index".into())
            })?;
            let SourceRequest::Media { source, stream, .. } = &record.request else {
                return Err(Error::Invalid("stored picture has no media source".into()));
            };
            let (_, profile, _) = snapshot.source_stream(*source, *stream)?;
            let editbay_core::StreamFormat::Video { color, alpha, .. } = profile.format else {
                return Err(Error::Invalid("stored picture selected sound".into()));
            };
            let rotation = profile
                .metadata
                .get("editbay.display_rotation_degrees")
                .map(|value| value.parse::<f64>())
                .transpose()
                .map_err(|e| Error::Invalid(e.to_string()))?
                .unwrap_or(0.);
            if record.offset != offset
                || record.header.width != selected.width
                || record.header.height != selected.height
                || record.header.tick != selected.tick
                || !record.header.rotation_degrees.is_finite()
                || record.header.color != color
                || record.header.alpha != alpha
                || record.header.rotation_degrees != rotation
                || record.header.alpha_interpretation_required
                    != profile
                        .metadata
                        .contains_key("editbay.alpha_interpretation")
            {
                return Err(Error::Invalid(
                    "picture storage index has invalid geometry, time or offsets".into(),
                ));
            }
            offset = offset
                .checked_add(selected.size as u64)
                .filter(|n| *n <= manifest.bytes)
                .ok_or_else(|| {
                    Error::Invalid("picture storage index exceeds its payload".into())
                })?;
            if entries.insert(selected.key, record).is_some() {
                return Err(Error::Invalid(
                    "picture storage index repeats a source picture".into(),
                ));
            }
        }
        if offset != manifest.bytes {
            return Err(Error::Invalid(
                "picture storage index leaves unclaimed bytes".into(),
            ));
        }
        Ok(Self { file, entries })
    }

    pub(crate) fn read(
        &self,
        key: &Key,
        output: &mut [u8],
        cancel: &Cancellation,
    ) -> Result<Option<PictureHeader>> {
        let Some(record) = self.entries.get(key) else {
            return Ok(None);
        };
        if u64::from(record.header.width) * u64::from(record.header.height) * 4
            != output.len() as u64
        {
            return Err(Error::Invalid(
                "picture storage read has foreign geometry".into(),
            ));
        }
        let mut offset = record.offset;
        let mut digest = Sha256::new();
        for chunk in output.chunks_mut(64 * 1024) {
            check(cancel)?;
            self.file.read_exact_at(chunk, offset)?;
            digest.update(&*chunk);
            offset += chunk.len() as u64;
        }
        if <[u8; 32]>::from(digest.finalize()) != record.sha256 {
            return Err(Error::Invalid("prepared picture checksum failed".into()));
        }
        check(cancel)?;
        Ok(Some(record.header.clone()))
    }
}
fn check(cancel: &Cancellation) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
