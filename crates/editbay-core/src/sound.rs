use crate::{
    AnimatedProperty, ClipSource, DocumentVersion, Error, EvaluationSnapshot, FrameRange,
    FrameRate, NodeOperation, Result, SourcePosition, StreamFormat,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, sync::Arc};
use uuid::Uuid;

/// Explicit sound output with unchanged source channel identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SoundProfile {
    pub sample_rate: u32,
    pub channels: Vec<String>,
}

/// Bounds for compiled paths and each prepared sound block.
#[derive(Debug, Clone, Copy)]
pub struct SoundBudget {
    pub leaves: usize,
    pub path_steps: usize,
    pub block_frames: u32,
    pub positions: usize,
    pub operations: usize,
}

impl Default for SoundBudget {
    fn default() -> Self {
        Self {
            leaves: 64,
            path_steps: 256,
            block_frames: 4096,
            positions: 262_144,
            operations: 1_048_576,
        }
    }
}

#[derive(Clone, Copy)]
enum Step {
    Node(usize, usize),
    Clip(usize, usize, usize),
}

struct Leaf {
    source: Uuid,
    stream: u32,
    steps: Vec<Step>,
}

/// Exact source sample center and gain for one output sample.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SoundSample {
    pub center: SourcePosition,
    pub gain: f64,
    pub reverse: bool,
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
    /// Takes no arguments and returns source-center/gain plans including silence.
    pub fn sources(&self) -> &[SoundSourcePlan] {
        &self.sources
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
            owner: Arc::new(()),
            fingerprint: String::new(),
            duration_samples,
        };
        if let Some(audio) = compiled.snapshot.project().compositions[root].audio {
            compiled.walk(root, audio, Vec::new())?;
        }
        let mut semantic = Vec::new();
        for leaf in &compiled.leaves {
            let (asset, stream, _) = compiled.snapshot.source_stream(leaf.source, leaf.stream)?;
            let steps = leaf.steps.iter().map(|step| match *step {
                Step::Node(scene, node) => {
                    let node = &compiled.snapshot.project().compositions[scene].nodes[node];
                    serde_json::json!({"range":node.range,"operation":node.operation,"animation":node.animation})
                }
                Step::Clip(scene, track, clip) => {
                    let track = &compiled.snapshot.project().compositions[scene].tracks[track];
                    let clip = &track.clips[clip];
                    serde_json::json!({"enabled":track.enabled,"range":clip.range,"source":clip.source,"time_map":clip.time_map})
                }
            }).collect::<Vec<_>>();
            semantic.push(serde_json::json!({"source":leaf.source,"stream":stream,"bytes":asset.bytes,"sha256":asset.sha256,"steps":steps}));
        }
        compiled.fingerprint = hash(&(compiled.profile.clone(), semantic))?;
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
                        let StreamFormat::Audio { channels, .. } = &profile.format else {
                            return Err(invalid("sound source is not audio"));
                        };
                        if *channels != self.profile.channels {
                            return Err(invalid(
                                "source channel identities require explicit routing",
                            ));
                        }
                        if self.leaves.len() >= self.budget.leaves {
                            return Err(invalid("compiled sound graph exceeds its source budget"));
                        }
                        self.leaves.push(Leaf {
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

    /// Inspect the exact output duration without float rounding.
    /// Takes no arguments and returns the ceiling number of output samples.
    pub fn duration_samples(&self) -> u64 {
        self.duration_samples
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
            || self
                .leaves
                .len()
                .checked_mul(frames as usize)
                .is_none_or(|count| count > self.budget.positions)
            || self
                .leaves
                .iter()
                .map(|leaf| leaf.steps.len())
                .sum::<usize>()
                .checked_mul(frames as usize)
                .is_none_or(|count| count > self.budget.operations)
        {
            return Err(invalid(
                "sound block exceeds its interval or preparation budget",
            ));
        }
        let composition = &self.snapshot.project().compositions[self.root];
        let mut sources = Vec::with_capacity(self.leaves.len());
        for leaf in &self.leaves {
            let (asset, stream, fingerprint) =
                self.snapshot.source_stream(leaf.source, leaf.stream)?;
            let StreamFormat::Audio { sample_rate, .. } = stream.format else {
                return Err(invalid("sound source is not audio"));
            };
            let mut samples = Vec::with_capacity(frames as usize);
            for index in first_sample..first_sample + u64::from(frames) {
                let position =
                    sample_center(index, self.profile.sample_rate, composition.frame_rate)?;
                let mut position = position;
                let mut before = false;
                let mut gain = 1.;
                let mut active = contains(
                    FrameRange {
                        start: 0,
                        end: composition.duration,
                    },
                    position,
                    false,
                )?;
                for step in &leaf.steps {
                    if !active {
                        break;
                    }
                    match *step {
                        Step::Node(scene, node) => {
                            let node = &self.snapshot.project().compositions[scene].nodes[node];
                            active &= contains(node.range, position, before)?;
                            if active
                                && let NodeOperation::Gain { gain: value, .. } = node.operation
                            {
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
                                active &= direction != 0;
                                position = clip.time_map.position_validated(local)?;
                                before = direction != 0 && ((direction < 0) != before);
                            }
                        }
                    }
                }
                samples.push(if active {
                    Some(SoundSample {
                        center: stream
                            .time_base
                            .at_rate(position, FrameRate::new(sample_rate, 1)?)?,
                        gain,
                        reverse: before,
                    })
                } else {
                    None
                });
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
        let sha256 = hash(&(
            "editbay-sound-block-1",
            &self.fingerprint,
            first_sample,
            frames,
            &sources,
        ))?;
        Ok(SoundBlockPlan {
            owner: self.owner.clone(),
            version: DocumentVersion::of(self.snapshot.project()),
            composition: self.composition,
            profile: self.profile.clone(),
            first_sample,
            frames,
            sources,
            sha256,
        })
    }
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
fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
