use crate::{
    AnimatedProperty, ClipSource, DocumentVersion, Error, EvaluationSnapshot, FrameRange,
    FrameRate, NodeOperation, Result, SourcePosition, SourceStream, StreamFormat,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use uuid::Uuid;
mod index;

/// Explicit sound output with unchanged source channel identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SoundProfile {
    pub sample_rate: u32,
    pub channels: Vec<String>,
}

/// Separate bounds for retained paths and each prepared sound block.
/// `compiled_paths`, `compiled_steps` and `intervals` bound storage. `leaves`
/// bounds possibly active paths; position and operation limits charge only them.
#[derive(Debug, Clone, Copy)]
pub struct SoundBudget {
    pub compiled_paths: usize,
    pub compiled_steps: usize,
    pub intervals: usize,
    pub leaves: usize,
    pub path_steps: usize,
    pub block_frames: u32,
    pub positions: usize,
    pub operations: usize,
}

impl Default for SoundBudget {
    fn default() -> Self {
        Self {
            compiled_paths: 16_384,
            compiled_steps: 1_048_576,
            intervals: 65_536,
            leaves: 64,
            path_steps: 256,
            block_frames: 4096,
            positions: 262_144,
            operations: 1_048_576,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Step {
    Node(usize, usize),
    Clip(usize, usize, usize),
}

struct Leaf {
    source: Uuid,
    stream: u32,
    steps: Vec<Step>,
    linear: bool,
}

/// Retained sound path and interval counts, independent of playback duration.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SoundIndexStats {
    pub paths: usize,
    pub steps: usize,
    pub intervals: usize,
}

/// Actual bounded work selected for a single sound block.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SoundPreparationStats {
    pub lookup_nodes: usize,
    pub active_paths: usize,
    pub positions: usize,
    pub evaluated_positions: usize,
    pub linear_positions: usize,
    pub operations: usize,
}

/// Exact source sample center and gain for one output sample.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SoundSample {
    pub center: SourcePosition,
    pub gain: f64,
    pub reverse: bool,
    pub step: f64,
}

/// One source contribution, retaining silence at inactive output samples.
#[derive(Debug, Serialize)]
pub struct SoundSourcePlan {
    pub source: Uuid,
    pub stream: u32,
    pub asset: Uuid,
    pub bytes: u64,
    pub asset_sha256: String,
    pub stream_sha256: String,
    pub sample_rate: u32,
    pub samples: Vec<Option<SoundSample>>,
}

/// Immutable block plan bound privately to its compiled sound owner.
#[derive(Serialize)]
pub struct SoundBlockPlan {
    #[serde(skip)]
    owner: Arc<()>,
    version: DocumentVersion,
    composition: Uuid,
    profile: SoundProfile,
    first_sample: u64,
    frames: u32,
    sources: Vec<SoundSourcePlan>,
    work: SoundPreparationStats,
    sha256: String,
}

impl SoundBlockPlan {
    /// Inspect this block's captured document revision.
    /// Takes no arguments and returns the immutable publication owner.
    pub fn version(&self) -> DocumentVersion {
        self.version
    }
    /// Inspect this block's declared output layout.
    /// Takes no arguments and returns its rate and exact channel identities.
    pub fn profile(&self) -> &SoundProfile {
        &self.profile
    }
    /// Inspect the output sample interval.
    /// Takes no arguments and returns its first sample and frame count.
    pub fn interval(&self) -> (u64, u32) {
        (self.first_sample, self.frames)
    }
    /// Inspect resolved source contributions without granting mutation.
    /// Takes no arguments and returns possible contributors including per-sample
    /// silence. Paths outside the block's conservative interval can be absent.
    pub fn sources(&self) -> &[SoundSourcePlan] {
        &self.sources
    }
    /// Inspect charged work without exposing mutable planning state.
    /// Takes no arguments; returns interval visits and conservative active work.
    pub fn work(&self) -> SoundPreparationStats {
        self.work
    }
    /// Inspect this block's semantic content identity.
    /// Takes no arguments and returns its hash, independent of document labels/revision.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Compiled sound paths sharing the validated document's temporal primitives.
pub struct SoundSnapshot {
    snapshot: Arc<EvaluationSnapshot>,
    composition: Uuid,
    root: usize,
    profile: SoundProfile,
    budget: SoundBudget,
    leaves: Vec<Leaf>,
    index: index::Index,
    steps: usize,
    owner: Arc<()>,
    fingerprint: String,
    duration_samples: u64,
}

impl SoundSnapshot {
    /// Compile source, nested, gain and mix paths for repeated sound blocks.
    /// `snapshot` retains the validated document; `composition`, `profile` and
    /// `budget` select explicit output and bounds. Returns captured paths without
    /// media IO; channel conversion is rejected until an explicit routing node exists.
    pub fn new(
        snapshot: Arc<EvaluationSnapshot>,
        composition: Uuid,
        profile: SoundProfile,
        budget: SoundBudget,
    ) -> Result<Self> {
        Self::compile(snapshot, composition, profile, budget, false)
    }

    /// Retain the selected graph's original channels at a device output rate.
    /// `snapshot`, `composition`, `sample_rate` and `budget` select captured
    /// content and output bounds. Returns a compiled graph, rejecting mixed
    /// channel identities; a graph without sound produces stereo silence.
    pub fn at_output_rate(
        snapshot: Arc<EvaluationSnapshot>,
        composition: Uuid,
        sample_rate: u32,
        budget: SoundBudget,
    ) -> Result<Self> {
        Self::compile(
            snapshot,
            composition,
            SoundProfile {
                sample_rate,
                channels: vec!["FL".into(), "FR".into()],
            },
            budget,
            true,
        )
    }

    fn compile(
        snapshot: Arc<EvaluationSnapshot>,
        composition: Uuid,
        profile: SoundProfile,
        budget: SoundBudget,
        infer_channels: bool,
    ) -> Result<Self> {
        if !(8000..=384_000).contains(&profile.sample_rate)
            || profile.channels.is_empty()
            || profile.channels.len() > 64
            || profile
                .channels
                .iter()
                .any(|name| name.is_empty() || name.len() > 63)
            || profile.channels.iter().collect::<HashSet<_>>().len() != profile.channels.len()
            || budget.leaves == 0
            || budget.leaves > 256
            || budget.compiled_paths == 0
            || budget.compiled_paths > 65_536
            || budget.compiled_steps == 0
            || budget.compiled_steps > 8_388_608
            || budget.intervals == 0
            || budget.intervals > 1_048_576
            || budget.path_steps == 0
            || budget.path_steps > 1024
            || budget.block_frames == 0
            || budget.block_frames > 8192
            || budget.positions == 0
            || budget.positions > 1_048_576
            || budget.operations == 0
            || budget.operations > 8_388_608
        {
            return Err(invalid("invalid sound profile or preparation budget"));
        }
        let root = snapshot
            .project()
            .compositions
            .iter()
            .position(|scene| scene.id == composition)
            .ok_or_else(|| invalid("sound composition is absent"))?;
        let scene = &snapshot.project().compositions[root];
        let duration_samples = u64::try_from(
            (u128::from(scene.duration)
                * u128::from(profile.sample_rate)
                * u128::from(scene.frame_rate.denominator))
            .div_ceil(u128::from(scene.frame_rate.numerator)),
        )
        .map_err(|_| invalid("sound duration exceeds its integer range"))?;
        let mut compiled = Self {
            snapshot,
            composition,
            root,
            profile,
            budget,
            leaves: Vec::new(),
            index: index::Index::default(),
            steps: 0,
            owner: Arc::new(()),
            fingerprint: String::new(),
            duration_samples,
        };
        if let Some(audio) = compiled.snapshot.project().compositions[root].audio {
            compiled.walk(root, audio, Vec::new())?;
        }
        compiled.compile_index()?;
        if infer_channels && let Some(leaf) = compiled.leaves.first() {
            let (_, stream, _) = compiled.snapshot.source_stream(leaf.source, leaf.stream)?;
            let StreamFormat::Audio { channels, .. } = &stream.format else {
                return Err(invalid("sound source is not audio"));
            };
            compiled.profile.channels = channels.clone();
        }
        let mut fingerprint = Sha256::new();
        fingerprint.update(b"editbay-sound-graph-3");
        fingerprint.update(hash(&compiled.profile)?.as_bytes());
        fingerprint.update((compiled.leaves.len() as u64).to_le_bytes());
        let mut sources = HashMap::new();
        let mut steps = HashMap::new();
        for leaf in &compiled.leaves {
            let (asset, stream, _) = compiled.snapshot.source_stream(leaf.source, leaf.stream)?;
            if !matches!(&stream.format, StreamFormat::Audio { channels, .. } if *channels == compiled.profile.channels)
            {
                return Err(invalid(
                    "source channel identities require explicit routing",
                ));
            }
            let source = match sources.entry((leaf.source, leaf.stream)) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(hash(&(leaf.source, stream, asset.bytes, &asset.sha256))?)
                }
            };
            fingerprint.update(source.as_bytes());
            fingerprint.update((leaf.steps.len() as u64).to_le_bytes());
            for step in &leaf.steps {
                let stamp = match steps.entry(*step) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let stamp = match *step {
                            Step::Node(scene, node) => {
                                let node =
                                    &compiled.snapshot.project().compositions[scene].nodes[node];
                                hash(&(node.range, &node.operation, &node.animation))?
                            }
                            Step::Clip(scene, track, clip) => {
                                let track =
                                    &compiled.snapshot.project().compositions[scene].tracks[track];
                                let clip = &track.clips[clip];
                                hash(&(track.enabled, clip.range, clip.source, &clip.time_map))?
                            }
                        };
                        entry.insert(stamp)
                    }
                };
                fingerprint.update(stamp.as_bytes());
            }
        }
        compiled.fingerprint = format!("{:x}", fingerprint.finalize());
        Ok(compiled)
    }

    fn walk(&mut self, scene: usize, node_id: Uuid, mut steps: Vec<Step>) -> Result<()> {
        if steps.len() >= self.budget.path_steps {
            return Err(invalid("compiled sound path exceeds its step budget"));
        }
        let composition = &self.snapshot.project().compositions[scene];
        let index = composition
            .nodes
            .iter()
            .position(|node| node.id == node_id)
            .ok_or_else(|| invalid("sound node is absent"))?;
        steps.push(Step::Node(scene, index));
        match &composition.nodes[index].operation {
            NodeOperation::Source { clip: id } => {
                let (track_index, clip_index) = composition
                    .tracks
                    .iter()
                    .enumerate()
                    .find_map(|(t, track)| {
                        track
                            .clips
                            .iter()
                            .position(|clip| clip.id == *id)
                            .map(|c| (t, c))
                    })
                    .ok_or_else(|| invalid("sound clip is absent"))?;
                let source = composition.tracks[track_index].clips[clip_index].source;
                steps.push(Step::Clip(scene, track_index, clip_index));
                if steps.len() > self.budget.path_steps {
                    return Err(invalid("compiled sound path exceeds its step budget"));
                }
                match source {
                    ClipSource::Media { source, stream } => {
                        let (_, profile, _) = self.snapshot.source_stream(source, stream)?;
                        let StreamFormat::Audio { .. } = &profile.format else {
                            return Err(invalid("sound source is not audio"));
                        };
                        if self.leaves.len() >= self.budget.compiled_paths {
                            return Err(invalid("compiled sound graph exceeds its source budget"));
                        }
                        self.steps = self
                            .steps
                            .checked_add(steps.len())
                            .filter(|count| *count <= self.budget.compiled_steps)
                            .ok_or_else(|| {
                                invalid("compiled sound graph exceeds its total step budget")
                            })?;
                        let linear = steps.iter().all(|step| match *step {
                            Step::Node(scene, node) => self.snapshot.project().compositions[scene]
                                .nodes[node]
                                .animation
                                .is_empty(),
                            Step::Clip(scene, track, clip) => {
                                self.snapshot.project().compositions[scene].tracks[track].clips
                                    [clip]
                                    .time_map
                                    .points
                                    .len()
                                    == 2
                            }
                        });
                        self.leaves.push(Leaf {
                            linear,
                            source,
                            stream,
                            steps,
                        });
                    }
                    ClipSource::Composition { composition } => {
                        let child = self
                            .snapshot
                            .project()
                            .compositions
                            .iter()
                            .position(|scene| scene.id == composition)
                            .ok_or_else(|| invalid("nested sound composition is absent"))?;
                        let audio = self.snapshot.project().compositions[child]
                            .audio
                            .ok_or_else(|| invalid("nested sound output is absent"))?;
                        self.walk(child, audio, steps)?;
                    }
                }
            }
            NodeOperation::Gain { audio, .. } => self.walk(scene, *audio, steps)?,
            NodeOperation::Mix { inputs } => {
                let inputs = inputs.clone();
                for input in inputs {
                    self.walk(scene, input, steps.clone())?;
                }
            }
            _ => return Err(invalid("unsupported sound operation")),
        }
        Ok(())
    }

    fn compile_index(&mut self) -> Result<()> {
        let project = self.snapshot.project();
        let mut builder = index::Builder::new(self.budget.intervals);
        for (id, leaf) in self.leaves.iter().enumerate() {
            let mut spans = None;
            for step in leaf.steps.iter().rev() {
                spans = Some(match *step {
                    Step::Node(scene, node) => {
                        builder.intersect(spans, project.compositions[scene].nodes[node].range)?
                    }
                    Step::Clip(scene, track, clip) => {
                        let track = &project.compositions[scene].tracks[track];
                        if track.enabled {
                            builder.preimage(spans, &track.clips[clip])?
                        } else {
                            Vec::new()
                        }
                    }
                });
                if spans.as_ref().is_some_and(Vec::is_empty) {
                    break;
                }
            }
            self.index
                .add(id, spans.unwrap_or_default(), self.budget.intervals)?;
        }
        self.index.finish();
        Ok(())
    }

    /// Inspect bounded compiled storage before preparing sound.
    /// Takes no arguments; returns retained paths, path steps and root intervals.
    pub fn index_stats(&self) -> SoundIndexStats {
        SoundIndexStats {
            paths: self.leaves.len(),
            steps: self.steps,
            intervals: self.index.len(),
        }
    }

    /// Inspect the exact output duration without float rounding.
    /// Takes no arguments and returns the ceiling number of output samples.
    pub fn duration_samples(&self) -> u64 {
        self.duration_samples
    }

    /// Inspect the compiled output rate and channel identities.
    /// Takes no arguments and returns the immutable preparation profile.
    pub fn profile(&self) -> &SoundProfile {
        &self.profile
    }

    /// Retain the exact validated document used by this sound compiler.
    /// Takes no arguments and returns the shared evaluation owner for native IO.
    pub fn evaluation(&self) -> &Arc<EvaluationSnapshot> {
        &self.snapshot
    }

    /// Check a block's private owner before native sample evaluation.
    /// `plan` is a prepared block. Returns an error for another compiled owner,
    /// including an otherwise identical document/profile and public revision.
    pub fn validate_plan(&self, plan: &SoundBlockPlan) -> Result<()> {
        if !Arc::ptr_eq(&self.owner, &plan.owner) {
            return Err(invalid("sound plan belongs to another compiled owner"));
        }
        Ok(())
    }

    /// Prepare a bounded output interval at exact sample centers.
    /// `first_sample` and `frames` select the output sample interval. Returns
    /// immutable source-center/gain contributions, with silence for inactive
    /// tracks/ranges and freeze maps. No decoding or callback work occurs here.
    pub fn prepare(&self, first_sample: u64, frames: u32) -> Result<SoundBlockPlan> {
        if frames == 0
            || frames > self.budget.block_frames
            || first_sample
                .checked_add(u64::from(frames))
                .is_none_or(|end| end > self.duration_samples)
        {
            return Err(invalid(
                "sound block exceeds its interval or preparation budget",
            ));
        }
        let composition = &self.snapshot.project().compositions[self.root];
        let first = sample_center(
            first_sample,
            self.profile.sample_rate,
            composition.frame_rate,
        )?;
        let last = sample_center(
            first_sample + u64::from(frames) - 1,
            self.profile.sample_rate,
            composition.frame_rate,
        )?;
        let (paths, visited) = self.index.query(first, last, self.budget.leaves)?;
        self.prepare_paths(first_sample, frames, &paths, visited)
    }

    fn prepare_paths(
        &self,
        first_sample: u64,
        frames: u32,
        paths: &[index::Candidate],
        lookup_nodes: usize,
    ) -> Result<SoundBlockPlan> {
        let positions = paths
            .len()
            .checked_mul(frames as usize)
            .filter(|count| *count <= self.budget.positions)
            .ok_or_else(|| invalid("sound block exceeds its position budget"))?;
        let composition = &self.snapshot.project().compositions[self.root];
        let ranges = paths
            .iter()
            .map(|path| {
                path.samples(
                    first_sample,
                    frames,
                    self.profile.sample_rate,
                    composition.frame_rate,
                )
            })
            .collect::<Vec<_>>();
        let operations = paths
            .iter()
            .zip(&ranges)
            .map(|(path, range)| self.leaves[path.leaf].steps.len() * range.len())
            .sum::<usize>()
            .checked_add(lookup_nodes)
            .filter(|count| *count <= self.budget.operations)
            .ok_or_else(|| invalid("sound block exceeds its operation budget"))?;
        let mut work = SoundPreparationStats {
            lookup_nodes,
            active_paths: paths.len(),
            positions,
            evaluated_positions: ranges.iter().map(|range| range.len()).sum(),
            linear_positions: 0,
            operations,
        };
        let centers = if paths.is_empty() {
            Vec::new()
        } else {
            (first_sample..first_sample + u64::from(frames))
                .map(|index| {
                    let position =
                        sample_center(index, self.profile.sample_rate, composition.frame_rate)?;
                    Ok((
                        position,
                        contains(
                            FrameRange {
                                start: 0,
                                end: composition.duration,
                            },
                            position,
                            false,
                        )?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?
        };
        let mut sources = Vec::with_capacity(paths.len());
        for (path, range) in paths.iter().zip(ranges) {
            let leaf = &self.leaves[path.leaf];
            let (asset, stream, fingerprint) =
                self.snapshot.source_stream(leaf.source, leaf.stream)?;
            let StreamFormat::Audio { sample_rate, .. } = stream.format else {
                return Err(invalid("sound source is not audio"));
            };
            let mut samples = vec![None; frames as usize];
            let evaluate = |center| self.evaluate_sample(leaf, stream, sample_rate, center);
            let cropped = &centers[range.clone()];
            let linear = if leaf.linear && !cropped.is_empty() {
                fill_linear(
                    &mut samples[range.clone()],
                    evaluate(cropped[0])?,
                    evaluate(cropped[(cropped.len() - 1).min(1)])?,
                    evaluate(cropped[cropped.len() - 1])?,
                )
            } else {
                false
            };
            if !linear {
                for (sample, &center) in samples[range].iter_mut().zip(cropped) {
                    *sample = evaluate(center)?;
                }
            } else {
                work.linear_positions += cropped.len();
            }
            sources.push(SoundSourcePlan {
                source: leaf.source,
                stream: leaf.stream,
                asset: asset.id,
                bytes: asset.bytes,
                asset_sha256: asset.sha256.clone(),
                stream_sha256: fingerprint.into(),
                sample_rate,
                samples,
            });
        }
        let sha256 = block_hash(&self.fingerprint, first_sample, frames, &sources);
        Ok(SoundBlockPlan {
            owner: self.owner.clone(),
            version: DocumentVersion::of(self.snapshot.project()),
            composition: self.composition,
            profile: self.profile.clone(),
            first_sample,
            frames,
            sources,
            work,
            sha256,
        })
    }
    /// Resolve one center through the full temporal path.
    /// `leaf`, `stream`, `sample_rate` and root center select exact source time.
    /// Returns the contribution or explicit silence under all range/gain rules.
    fn evaluate_sample(
        &self,
        leaf: &Leaf,
        stream: &SourceStream,
        sample_rate: u32,
        (mut position, mut active): (SourcePosition, bool),
    ) -> Result<Option<SoundSample>> {
        let composition = &self.snapshot.project().compositions[self.root];
        let mut before = false;
        let mut gain = 1.;
        let mut sample_step = f64::from(composition.frame_rate.numerator)
            / (f64::from(composition.frame_rate.denominator) * f64::from(self.profile.sample_rate));
        for step in &leaf.steps {
            if !active {
                break;
            }
            match *step {
                Step::Node(scene, node) => {
                    let node = &self.snapshot.project().compositions[scene].nodes[node];
                    active &= contains(node.range, position, before)?;
                    if active && let NodeOperation::Gain { gain: value, .. } = node.operation {
                        gain *= node
                            .animation
                            .iter()
                            .find(|channel| channel.property == AnimatedProperty::Gain)
                            .map_or(Ok(value), |channel| {
                                channel.value_validated(position, before)
                            })?;
                        if !gain.is_finite() {
                            return Err(invalid("combined sound gain is not finite"));
                        }
                    }
                }
                Step::Clip(scene, track, clip) => {
                    let track = &self.snapshot.project().compositions[scene].tracks[track];
                    let clip = &track.clips[clip];
                    active &= track.enabled && contains(clip.range, position, before)?;
                    if active {
                        let local = position.relative_to(
                            i64::try_from(clip.range.start)
                                .map_err(|_| invalid("sound clip start overflow"))?,
                        )?;
                        let direction = clip.time_map.direction_validated(local, before)?;
                        sample_step *= clip.time_map.slope_validated(local, before)?;
                        active &= direction != 0;
                        position = clip.time_map.position_validated(local)?;
                        before = direction != 0 && ((direction < 0) != before);
                    }
                }
            }
        }
        Ok(if active {
            Some(SoundSample {
                center: stream
                    .time_base
                    .at_rate(position, FrameRate::new(sample_rate, 1)?)?,
                gain,
                reverse: before,
                step: sample_step * f64::from(stream.time_base.numerator) * f64::from(sample_rate)
                    / f64::from(stream.time_base.denominator),
            })
        } else {
            None
        })
    }
}

/// Step a proven constant-rate path in exact rational sample coordinates.
/// `output` is the active span; `first`, `second` and `last` are full evaluations.
/// Returns false for inactive endpoints, changed metadata or unsafe arithmetic,
/// allowing the caller to overwrite the span with the full temporal evaluator.
fn fill_linear(
    output: &mut [Option<SoundSample>],
    first: Option<SoundSample>,
    second: Option<SoundSample>,
    last: Option<SoundSample>,
) -> bool {
    let (Some(first), Some(second), Some(last)) = (first, second, last) else {
        return false;
    };
    if [second, last].iter().any(|sample| {
        sample.gain.to_bits() != first.gain.to_bits()
            || sample.step.to_bits() != first.step.to_bits()
            || sample.reverse != first.reverse
    }) {
        return false;
    }
    let mut a = first.center.denominator;
    let mut b = second.center.denominator;
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let Some(denominator) = (first.center.denominator / a).checked_mul(second.center.denominator)
    else {
        return false;
    };
    let start =
        i128::from(first.center.numerator) * i128::from(denominator / first.center.denominator);
    let next =
        i128::from(second.center.numerator) * i128::from(denominator / second.center.denominator);
    let Some(delta) = next.checked_sub(start) else {
        return false;
    };
    let Some(end) = delta
        .checked_mul(output.len().saturating_sub(1) as i128)
        .and_then(|offset| start.checked_add(offset))
    else {
        return false;
    };
    if SourcePosition::from_fraction(end, denominator).ok() != Some(last.center) {
        return false;
    }
    let mut position = start;
    for sample in output {
        let Ok(center) = SourcePosition::from_fraction(position, denominator) else {
            return false;
        };
        *sample = Some(SoundSample { center, ..first });
        let Some(next) = position.checked_add(delta) else {
            return false;
        };
        position = next;
    }
    true
}

fn sample_center(sample: u64, rate: u32, frames: FrameRate) -> Result<SourcePosition> {
    let numerator = (i128::from(sample) * 2 + 1) * i128::from(frames.numerator);
    let denominator = u64::from(rate) * u64::from(frames.denominator) * 2;
    SourcePosition::from_fraction(numerator, denominator)
}
fn contains(range: FrameRange, position: SourcePosition, before: bool) -> Result<bool> {
    let start = position
        .compare_tick(i64::try_from(range.start).map_err(|_| invalid("sound range overflow"))?)?;
    let end = position
        .compare_tick(i64::try_from(range.end).map_err(|_| invalid("sound range overflow"))?)?;
    Ok(if before {
        start.is_gt() && end.is_le()
    } else {
        start.is_ge() && end.is_lt()
    })
}
fn hash(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
/// Hash one complete block after temporal evaluation.
/// `fingerprint`, interval and sources supply fixed-width semantic bytes.
/// Returns one content digest without per-sample hashing or JSON encoding.
fn block_hash(fingerprint: &str, first: u64, frames: u32, sources: &[SoundSourcePlan]) -> String {
    let mut bytes = Vec::with_capacity(sources.len() * (frames as usize * 34 + 256));
    bytes.extend_from_slice(b"editbay-sound-block-2");
    bytes.extend_from_slice(fingerprint.as_bytes());
    bytes.extend_from_slice(&first.to_le_bytes());
    bytes.extend_from_slice(&frames.to_le_bytes());
    bytes.extend_from_slice(&(sources.len() as u64).to_le_bytes());
    for source in sources {
        bytes.extend_from_slice(source.source.as_bytes());
        bytes.extend_from_slice(&source.stream.to_le_bytes());
        bytes.extend_from_slice(source.asset.as_bytes());
        bytes.extend_from_slice(&source.bytes.to_le_bytes());
        bytes.extend_from_slice(source.asset_sha256.as_bytes());
        bytes.extend_from_slice(source.stream_sha256.as_bytes());
        bytes.extend_from_slice(&source.sample_rate.to_le_bytes());
        for sample in &source.samples {
            bytes.push(u8::from(sample.is_some()));
            if let Some(sample) = sample {
                bytes.extend_from_slice(&sample.center.numerator.to_le_bytes());
                bytes.extend_from_slice(&sample.center.denominator.to_le_bytes());
                bytes.extend_from_slice(&sample.gain.to_bits().to_le_bytes());
                bytes.push(u8::from(sample.reverse));
                bytes.extend_from_slice(&sample.step.to_bits().to_le_bytes());
            }
        }
    }
    format!("{:x}", Sha256::digest(bytes))
}
fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
