use editbay_core::{DocumentVersion, SoundBlockPlan, SoundProfile, SoundSample, SoundSnapshot};
use editbay_media::{
    Cancellation, Error, NativePcmCache, PcmBudget, PcmProvider, PcmStats, Result,
};
use serde::Serialize;
mod kernels;
pub use kernels::KernelStats;
use kernels::Kernels;
use std::{
    f64::consts::PI,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Bounds for worker-side sound mixing and band-limited interpolation.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SoundRenderBudget {
    pub live_bytes: usize,
    pub operations: usize,
    pub kernel_bytes: usize,
    pub kernel_entries: usize,
}
impl Default for SoundRenderBudget {
    fn default() -> Self {
        Self {
            live_bytes: 32 * 1024 * 1024,
            operations: 16 * 1024 * 1024,
            kernel_bytes: 512 * 1024,
            kernel_entries: 256,
        }
    }
}
struct Charge {
    bytes: usize,
    live: Arc<AtomicUsize>,
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.live.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Immutable rendered float sound with retained consumer memory accounting.
pub struct SoundBuffer {
    samples: Vec<f32>,
    profile: SoundProfile,
    first_sample: u64,
    _charge: Charge,
}
impl SoundBuffer {
    /// Inspect rendered samples without channel conversion or clipping.
    /// Takes no arguments; returns interleaved float PCM in the declared layout.
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }
    /// Inspect the explicit rate and channel identities.
    /// Takes no arguments; returns the immutable output profile.
    pub fn profile(&self) -> &SoundProfile {
        &self.profile
    }
    /// Inspect the exact output interval.
    /// Takes no arguments; returns first output sample and frame count.
    pub fn interval(&self) -> (u64, usize) {
        (
            self.first_sample,
            self.samples.len() / self.profile.channels.len(),
        )
    }
}

/// A privately owned, revision-bound sound publication.
/// Payload replacement cannot preserve a valid publication receipt.
/// ```compile_fail,E0616
/// fn replace(a: &mut editbay_audio::SoundResult, b: editbay_audio::SoundResult) {
///     a.sound = b.sound;
/// }
/// ```
pub struct SoundResult {
    sound: Arc<SoundBuffer>,
    version: DocumentVersion,
    content_sha256: String,
    owner: Arc<()>,
}
impl SoundResult {
    /// Retain this receipt's immutable rendered sound.
    /// Takes no arguments; returns the exact charged buffer without payload mutation.
    pub fn sound(&self) -> &Arc<SoundBuffer> {
        &self.sound
    }
    /// Inspect the captured document publication owner.
    /// Takes no arguments; returns this receipt's immutable document version.
    pub fn version(&self) -> DocumentVersion {
        self.version
    }
    /// Inspect the immutable sound block's semantic identity.
    /// Takes no arguments; returns its content hash separately from revision ownership.
    pub fn sha256(&self) -> &str {
        &self.content_sha256
    }
}

/// Retained native sound evaluator shared by preview and durable delivery workers.
pub struct SoundRenderer<P: PcmProvider = NativePcmCache> {
    snapshot: Arc<SoundSnapshot>,
    pcm: P,
    budget: SoundRenderBudget,
    live: Arc<AtomicUsize>,
    cancel: Cancellation,
    owner: Arc<()>,
    kernels: Kernels,
}
impl SoundRenderer {
    /// Bind one compiled sound graph to bounded native PCM reads.
    /// `snapshot` owns the graph, both budgets bound memory/work and `cancel`
    /// interrupts decoding and mixing. Returns an empty worker-side evaluator.
    pub fn new(
        snapshot: Arc<SoundSnapshot>,
        pcm_budget: PcmBudget,
        budget: SoundRenderBudget,
        cancel: Cancellation,
    ) -> Result<Self> {
        let pcm = NativePcmCache::new(snapshot.evaluation().clone(), pcm_budget, cancel.clone())?;
        Self::with_provider(snapshot, pcm, budget)
    }
}
impl<P: PcmProvider> SoundRenderer<P> {
    /// Inspect the source provider for supervision and resource measurements.
    /// Takes no arguments; returns the retained provider without ownership mutation.
    pub fn pcm_provider(&self) -> &P {
        &self.pcm
    }
    /// Bind a captured sound graph to an existing native or isolated PCM provider.
    /// `snapshot` and `pcm` must own the same evaluation snapshot; `budget`
    /// bounds rendering. Returns an evaluator sharing the provider's cancellation.
    pub fn with_provider(
        snapshot: Arc<SoundSnapshot>,
        pcm: P,
        budget: SoundRenderBudget,
    ) -> Result<Self> {
        let cancel = pcm.cancellation().clone();
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !Arc::ptr_eq(snapshot.evaluation(), pcm.snapshot()) {
            return Err(Error::Invalid(
                "sound provider owns a different source snapshot".into(),
            ));
        }
        if budget.live_bytes == 0
            || budget.live_bytes > 512 * 1024 * 1024
            || budget.operations == 0
            || budget.operations > 128 * 1024 * 1024
            || budget.kernel_bytes > 16 * 1024 * 1024
            || budget.kernel_entries > 4096
        {
            return Err(Error::Invalid(
                "sound render budgets exceed supported limits".into(),
            ));
        }
        Ok(Self {
            snapshot,
            pcm,
            budget,
            live: Arc::new(AtomicUsize::new(0)),
            cancel,
            owner: Arc::new(()),
            kernels: Kernels::new(budget.kernel_bytes, budget.kernel_entries),
        })
    }

    /// Evaluate an owned block using unchanged channels and finite float headroom.
    /// `plan` comes from this exact compiler. Returns summed original-channel PCM;
    /// fractional/retimed centers use a 48-lobe Blackman-windowed sinc with a
    /// 0.95 downsampling cutoff. Integer unity-rate samples are copied exactly.
    /// Freeze is declared silence; unsupported slopes above 16 are rejected.
    pub fn render(&mut self, plan: &SoundBlockPlan) -> Result<SoundResult> {
        self.check()?;
        self.snapshot.validate_plan(plan)?;
        let (first_sample, frames) = plan.interval();
        let channels = plan.profile().channels.len();
        let count = frames as usize * channels;
        let bytes = count * 4;
        let mut operations = 0usize;
        for source in plan.sources() {
            for sample in source.samples.iter().flatten() {
                let point = point(*sample)?;
                operations =
                    operations
                        .checked_add(point.taps().checked_mul(channels).ok_or_else(|| {
                            Error::Invalid("sound operation count overflow".into())
                        })?)
                        .ok_or_else(|| Error::Invalid("sound operation count overflow".into()))?;
            }
        }
        if operations > self.budget.operations {
            return Err(Error::Invalid(
                "sound interpolation exceeds its operation budget".into(),
            ));
        }
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes)
                    .filter(|total| *total <= self.budget.live_bytes)
            })
            .map_err(|_| {
                Error::Invalid("rendered sound is pinned by consumers; release blocks".into())
            })?;
        let charge = Charge {
            bytes,
            live: self.live.clone(),
        };
        let mut mixed = vec![0f64; count];
        for source in plan.sources() {
            self.check()?;
            let points = source
                .samples
                .iter()
                .map(|s| s.map(point).transpose())
                .collect::<Result<Vec<_>>>()?;
            let Some(start) = points.iter().flatten().map(|p| p.start).min() else {
                continue;
            };
            let end = points.iter().flatten().map(|p| p.end).max().unwrap();
            let length = u32::try_from(i128::from(end) - i128::from(start)).map_err(|_| {
                Error::Invalid("sound source window exceeds its bounded interval".into())
            })?;
            let input = self
                .pcm
                .interval(source.source, source.stream, start, length)?;
            if input.pcm().interval().2 != channels {
                return Err(Error::Invalid("sound channel routing is undeclared".into()));
            }
            for (frame, point) in points.iter().enumerate() {
                if frame.is_multiple_of(32) {
                    self.check()?;
                }
                let Some(point) = point else {
                    continue;
                };
                let gain = source.samples[frame].unwrap().gain;
                let out = &mut mixed[frame * channels..(frame + 1) * channels];
                if point.exact {
                    let index = (point.start - start) as usize * channels;
                    for (value, sample) in out
                        .iter_mut()
                        .zip(&input.pcm().samples()[index..index + channels])
                    {
                        *value += f64::from(*sample) * gain;
                    }
                } else {
                    let mut sum = [0f64; 64];
                    let mut normalization = 0.;
                    let mut accumulate = |relative: i64, weight: f64| {
                        let offset = (point.origin + relative - start) as usize * channels;
                        for (channel, value) in sum[..channels].iter_mut().enumerate() {
                            *value += f64::from(input.pcm().samples()[offset + channel]) * weight;
                        }
                    };
                    if let Some(kernel) = self.kernels.get(point) {
                        normalization = kernel.normalization;
                        for tap in kernel.weights.iter() {
                            accumulate(tap.relative, tap.weight);
                        }
                    } else {
                        for relative in point.start - point.origin..point.end - point.origin {
                            if let Some(weight) = point.weight(relative) {
                                normalization += weight;
                                accumulate(relative, weight);
                            }
                        }
                    }
                    if !normalization.is_finite() || normalization.abs() < 0.5 {
                        return Err(Error::Invalid("invalid sound interpolation kernel".into()));
                    }
                    for channel in 0..channels {
                        out[channel] += sum[channel] * gain / normalization;
                    }
                }
            }
            self.pcm.validate_result(&input)?;
        }
        self.check()?;
        let samples = mixed.into_iter().map(|v| v as f32).collect::<Vec<_>>();
        if samples.iter().any(|v| !v.is_finite()) {
            return Err(Error::Invalid(
                "mixed sound exceeds finite float headroom".into(),
            ));
        }
        Ok(SoundResult {
            sound: Arc::new(SoundBuffer {
                samples,
                profile: plan.profile().clone(),
                first_sample,
                _charge: charge,
            }),
            version: plan.version(),
            content_sha256: plan.sha256().into(),
            owner: self.owner.clone(),
        })
    }

    /// Reject foreign or obsolete rendered publications.
    /// `result` is a rendered block; returns success only under this worker owner.
    pub fn validate_result(&mut self, result: &SoundResult) -> Result<()> {
        self.check()?;
        if !Arc::ptr_eq(&self.owner, &result.owner) {
            return Err(Error::Invalid(
                "sound result belongs to a foreign or obsolete renderer".into(),
            ));
        }
        self.pcm.check_sources()
    }
    /// Recheck all decoded source bytes before durable delivery publication.
    /// Takes no arguments; returns after every retained source checksum matches.
    pub fn verify_sources(&mut self) -> Result<()> {
        self.pcm.verify_sources()
    }
    /// Check an idle provider and the identities of its retained sources.
    /// Takes no arguments; returns a visible cancellation, source or worker error.
    /// This runs on the preparation worker, never on a device callback.
    pub fn poll(&mut self) -> Result<()> {
        self.check()?;
        self.pcm.check_sources()
    }
    /// Inspect retained native sample accounting.
    /// Takes no arguments; returns PCM cache, consumer and decoder resource use.
    pub fn pcm_stats(&self) -> PcmStats {
        self.pcm.stats()
    }
    /// Inspect lifetime-charged rendered sound memory.
    /// Takes no arguments; returns bytes held by all consumers of this renderer.
    pub fn live_bytes(&self) -> usize {
        self.live.load(Ordering::Acquire)
    }
    /// Inspect bounded interpolation storage and exact coefficient reuse.
    /// Takes no arguments; returns actual entry metadata and coefficient bytes
    /// separately from rendered buffers and native PCM ownership.
    pub fn kernel_stats(&self) -> KernelStats {
        self.kernels.stats()
    }
    /// Release native source/cache ownership and invalidate sound publications.
    /// Takes no arguments; consumer-held sound remains immutable and charged.
    pub fn clear(&mut self) {
        self.pcm.clear();
        self.kernels.clear();
        self.owner = Arc::new(());
    }
    fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
struct Point {
    origin: i64,
    fraction: f64,
    cutoff: f64,
    start: i64,
    end: i64,
    exact: bool,
}
impl Point {
    fn taps(&self) -> usize {
        (self.end - self.start) as usize
    }
    fn weight(&self, relative: i64) -> Option<f64> {
        let distance = (relative as f64 - self.fraction) * self.cutoff;
        if distance.abs() >= 48. {
            return None;
        }
        let phase = distance / 48.;
        let window = 0.42 + 0.5 * (PI * phase).cos() + 0.08 * (2. * PI * phase).cos();
        let sinc = if distance.abs() < 1e-12 {
            1.
        } else {
            (PI * distance).sin() / (PI * distance)
        };
        Some(sinc * window * self.cutoff)
    }
}
fn point(sample: SoundSample) -> Result<Point> {
    if !sample.step.is_finite()
        || sample.step == 0.
        || sample.step.abs() > 16.
        || sample.center.denominator == 0
    {
        return Err(Error::Invalid("unsupported sound resampling slope".into()));
    }
    let numerator = i128::from(sample.center.numerator) * 2 - i128::from(sample.center.denominator);
    let denominator = i128::from(sample.center.denominator) * 2;
    let integer = numerator.div_euclid(denominator);
    let exact = sample.step.abs() <= 1. && numerator.rem_euclid(denominator) == 0;
    let origin = i64::try_from(integer)
        .map_err(|_| Error::Invalid("sound sample origin overflow".into()))?;
    let fraction = numerator.rem_euclid(denominator) as f64 / denominator as f64;
    let cutoff = if sample.step.abs() > 1. {
        0.95 / sample.step.abs()
    } else {
        1.
    };
    let radius = if exact {
        0
    } else {
        (48. / cutoff).ceil() as i128
    };
    let start = i64::try_from(integer - radius)
        .map_err(|_| Error::Invalid("sound interpolation start overflow".into()))?;
    let end = i64::try_from(integer + radius + 1)
        .map_err(|_| Error::Invalid("sound interpolation end overflow".into()))?;
    Ok(Point {
        origin,
        fraction,
        cutoff,
        start,
        end,
        exact,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_kernel_phase_survives_large_absolute_source_origins() {
        let origin = 1i64 << 54;
        let prepared = point(SoundSample {
            center: editbay_core::SourcePosition::new(origin, 1).unwrap(),
            gain: 1.,
            reverse: false,
            step: 1.5,
        })
        .unwrap();
        assert_eq!(prepared.origin, origin - 1);
        assert_eq!(prepared.fraction, 0.5);
        assert!(!prepared.exact);
        let distance = ((i128::from(origin) - i128::from(prepared.origin)) as f64
            - prepared.fraction)
            * prepared.cutoff;
        assert!((distance - 0.95 / 3.).abs() < 1e-15);
    }
}
