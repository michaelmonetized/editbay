use super::Point;
use serde::Serialize;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    fraction: u64,
    cutoff: u64,
    first: i64,
    end: i64,
}

pub(super) struct Tap {
    pub relative: i64,
    pub weight: f64,
}

pub(super) struct Kernel {
    pub weights: Vec<Tap>,
    pub normalization: f64,
}

struct Entry {
    key: Key,
    used: u64,
    kernel: Kernel,
}

/// Actual retained coefficient storage and reuse, excluding allocator overhead.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct KernelStats {
    pub entries: usize,
    pub metadata_bytes: usize,
    pub coefficient_bytes: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub coefficients: u64,
}

pub(super) struct Kernels {
    entries: Vec<Entry>,
    byte_limit: usize,
    entry_limit: usize,
    serial: u64,
    stats: KernelStats,
}

impl Kernels {
    pub fn new(byte_limit: usize, entry_limit: usize) -> Self {
        Self {
            entries: Vec::new(),
            byte_limit,
            entry_limit,
            serial: 0,
            stats: KernelStats::default(),
        }
    }

    pub fn get(&mut self, point: &Point) -> Option<&Kernel> {
        let key = Key {
            fraction: point.fraction.to_bits(),
            cutoff: point.cutoff.to_bits(),
            first: point.start - point.origin,
            end: point.end - point.origin,
        };
        self.serial = self.serial.saturating_add(1);
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key) {
            self.stats.hits = self.stats.hits.saturating_add(1);
            self.entries[index].used = self.serial;
            return Some(&self.entries[index].kernel);
        }
        self.stats.misses = self.stats.misses.saturating_add(1);
        let metadata = self.entry_limit.checked_mul(size_of::<Entry>())?;
        let required = point.taps().checked_mul(size_of::<Tap>())?;
        if self.entry_limit == 0 || metadata.checked_add(required)? > self.byte_limit {
            return None;
        }
        if self.entries.capacity() == 0 {
            self.entries = Vec::with_capacity(self.entry_limit);
        }
        while self.entries.len() == self.entry_limit
            || self.stats.coefficient_bytes + metadata + required > self.byte_limit
        {
            let index = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(index, entry)| (entry.used, *index))
                .map(|(index, _)| index)?;
            let removed = self.entries.remove(index);
            self.stats.coefficient_bytes -= removed.kernel.weights.capacity() * size_of::<Tap>();
            self.stats.evictions = self.stats.evictions.saturating_add(1);
        }
        let mut normalization = 0.;
        let mut weights = Vec::with_capacity(point.taps());
        for relative in key.first..key.end {
            if let Some(weight) = point.weight(relative) {
                normalization += weight;
                weights.push(Tap { relative, weight });
            }
        }
        self.stats.coefficients = self.stats.coefficients.saturating_add(weights.len() as u64);
        self.stats.coefficient_bytes += weights.capacity() * size_of::<Tap>();
        self.entries.push(Entry {
            key,
            used: self.serial,
            kernel: Kernel {
                weights,
                normalization,
            },
        });
        Some(&self.entries.last()?.kernel)
    }

    pub fn stats(&self) -> KernelStats {
        KernelStats {
            entries: self.entries.len(),
            metadata_bytes: self.entries.capacity() * size_of::<Entry>(),
            ..self.stats
        }
    }

    pub fn clear(&mut self) {
        self.entries = Vec::new();
        self.stats.coefficient_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(origin: i64, fraction: f64, cutoff: f64) -> Point {
        let radius = (48. / cutoff).ceil() as i64;
        Point {
            origin,
            fraction,
            cutoff,
            start: origin - radius,
            end: origin + radius + 1,
            exact: false,
        }
    }

    #[test]
    fn exact_phase_keys_ignore_absolute_origin_but_preserve_cutoff_and_phase() {
        let mut cache = Kernels::new(512 * 1024, 2);
        let first = cache.get(&sample(0, 0.25, 0.95)).unwrap() as *const Kernel;
        for origin in [-(1i64 << 54), 1i64 << 54] {
            assert_eq!(
                first,
                cache.get(&sample(origin, 0.25, 0.95)).unwrap() as *const Kernel
            );
        }
        assert_eq!(cache.stats().hits, 2);
        let a = cache
            .get(&sample(0, 0.5, 0.95))
            .unwrap()
            .normalization
            .to_bits();
        let b = cache
            .get(&sample(0, 0.5, 0.475))
            .unwrap()
            .normalization
            .to_bits();
        assert_ne!(a, b);
        assert_eq!(cache.stats().entries, 2);
        assert_eq!(cache.stats().evictions, 1);
        cache.get(&sample(0, 0.25, 0.95)).unwrap();
        assert_eq!(cache.stats().misses, 4);
        assert_eq!(cache.stats().evictions, 2);
    }

    #[test]
    fn small_budgets_fall_back_without_retaining_any_storage() {
        for (bytes, entries) in [(0, 256), (512 * 1024, 0), (64, 1), (128, 256)] {
            let mut cache = Kernels::new(bytes, entries);
            assert!(cache.get(&sample(0, 0.5, 0.05)).is_none());
            assert_eq!(cache.stats().metadata_bytes, 0);
            assert_eq!(cache.stats().coefficient_bytes, 0);
        }
    }
}
