use crate::{AudioBlock, Cancellation, Error, Result};
use rustix::fs::{Mode, OFlags, openat};
use std::{fs::File, os::unix::fs::FileExt, path::Path};

pub(super) struct Store {
    file: File,
    first: i64,
    end: i64,
    channels: usize,
}

impl Store {
    /// Own an empty anonymous PCM file in an explicitly selected directory.
    /// `first` and `channels` declare the canonical sample grid. Returns storage
    /// removed by the operating system when its last owned descriptor is dropped.
    pub fn new(directory: &Path, first: i64, channels: usize) -> Result<Self> {
        if !(1..=64).contains(&channels) {
            return Err(Error::Invalid(
                "canonical PCM channel count is invalid".into(),
            ));
        }
        let directory = File::open(directory)?;
        let file = File::from(
            openat(
                &directory,
                ".",
                OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)?,
        );
        Ok(Self {
            file,
            first,
            end: first,
            channels,
        })
    }

    pub fn end(&self) -> i64 {
        self.end
    }

    pub fn bytes(&self) -> u64 {
        (i128::from(self.end) - i128::from(self.first)) as u64 * self.channels as u64 * 4
    }

    /// Append only previously unprepared samples from sequential native decode.
    /// `block` retains original float bits; `remaining` is the provider's available
    /// disk allowance and `cancel` interrupts bounded writes. Returns added bytes.
    pub fn append(
        &mut self,
        block: &AudioBlock,
        remaining: u64,
        cancel: &Cancellation,
    ) -> Result<u64> {
        let first = block
            .first_sample
            .ok_or_else(|| Error::Invalid("canonical PCM has no sample clock".into()))?;
        if !block.samples.len().is_multiple_of(self.channels) {
            return Err(Error::Invalid(
                "canonical PCM block is not channel aligned".into(),
            ));
        }
        let end = first
            .checked_add((block.samples.len() / self.channels) as i64)
            .ok_or_else(|| Error::Invalid("canonical PCM block end overflow".into()))?;
        if first > self.end {
            return Err(Error::Invalid(
                "canonical PCM has an in-range presentation gap".into(),
            ));
        }
        if end <= self.end {
            return Ok(0);
        }
        let from = usize::try_from(i128::from(self.end) - i128::from(first))
            .map_err(|e| Error::Invalid(e.to_string()))?
            * self.channels;
        let samples = &block.samples[from..];
        let bytes = std::mem::size_of_val(samples) as u64;
        if bytes > remaining {
            return Err(Error::Invalid(
                "canonical PCM exceeds its disk byte budget".into(),
            ));
        }
        let initial = self.bytes();
        let mut offset = initial;
        let result = (|| -> Result<()> {
            for chunk in bytemuck::cast_slice::<f32, u8>(samples).chunks(64 * 1024) {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                self.file.write_all_at(chunk, offset)?;
                offset += chunk.len() as u64;
            }
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.file.set_len(initial)?;
            return Err(error);
        }
        self.end = end;
        Ok(bytes)
    }

    /// Copy a prepared interval without changing sample representation.
    /// `first`, `output` and `cancel` select an already prepared, channel-aligned
    /// interval and its lifetime. Returns after bounded, cancellable file reads.
    pub fn read(&self, first: i64, output: &mut [f32], cancel: &Cancellation) -> Result<()> {
        let end = i128::from(first) + (output.len() / self.channels) as i128;
        if first < self.first
            || end > i128::from(self.end)
            || !output.len().is_multiple_of(self.channels)
        {
            return Err(Error::Invalid(
                "canonical PCM interval is not prepared".into(),
            ));
        }
        let mut offset =
            (i128::from(first) - i128::from(self.first)) as u64 * self.channels as u64 * 4;
        for chunk in bytemuck::cast_slice_mut::<f32, u8>(output).chunks_mut(64 * 1024) {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            self.file.read_exact_at(chunk, offset)?;
            offset += chunk.len() as u64;
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_storage_preserves_bits_and_rejects_incomplete_or_cancelled_io() {
        let directory = tempfile::tempdir().unwrap();
        let cancel = Cancellation::new().unwrap();
        let mut store = Store::new(directory.path(), -2, 2).unwrap();
        let samples = vec![0., -0., f32::MIN_POSITIVE, f32::from_bits(1), -1.25, 3.5];
        let block = AudioBlock {
            source_tick: None,
            first_sample: Some(-2),
            samples: samples.clone(),
        };
        assert!(store.append(&block, 23, &cancel).is_err());
        assert_eq!(store.bytes(), 0);
        assert_eq!(store.append(&block, 24, &cancel).unwrap(), 24);
        assert_eq!(store.append(&block, 0, &cancel).unwrap(), 0);
        let mut actual = vec![0.; samples.len()];
        store.read(-2, &mut actual, &cancel).unwrap();
        assert!(
            actual
                .iter()
                .zip(&samples)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        assert!(store.read(-3, &mut actual, &cancel).is_err());
        assert!(store.read(-2, &mut actual[..5], &cancel).is_err());
        let gap = AudioBlock {
            first_sample: Some(2),
            source_tick: None,
            samples: samples.clone(),
        };
        assert!(store.append(&gap, 24, &cancel).is_err());
        let unaligned = AudioBlock {
            first_sample: Some(1),
            samples: vec![1.],
            source_tick: None,
        };
        assert!(store.append(&unaligned, 24, &cancel).is_err());
        cancel.cancel();
        let next = AudioBlock {
            first_sample: Some(1),
            ..block
        };
        assert!(matches!(
            store.append(&next, 24, &cancel),
            Err(Error::Cancelled)
        ));
        assert_eq!(store.bytes(), 24);
        assert_eq!(store.file.metadata().unwrap().len(), 24);
        assert!(matches!(
            store.read(-2, &mut actual, &cancel),
            Err(Error::Cancelled)
        ));
        store.file.set_len(20).unwrap();
        assert!(
            store
                .read(-2, &mut actual, &Cancellation::new().unwrap())
                .is_err()
        );
        drop(store);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
