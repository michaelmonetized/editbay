use crate::{
    ClipSource, ColorConfiguration, DocumentVersion, Error, EvaluatedNode, FramePlan, FrameRate,
    NodeOperation, Project, Result, SocketType, SourcePosition, SourceRequest, StreamFormat,
    evaluation::resolve_operation,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};
use uuid::Uuid;

struct StreamIndex {
    media: usize,
    profile: usize,
    asset: usize,
    fingerprint: String,
}

struct CompiledNode {
    index: usize,
    socket: SocketType,
    operation: Arc<NodeOperation>,
    fingerprint: String,
    inputs: Vec<Uuid>,
}

struct Scene {
    index: usize,
    nodes: Vec<CompiledNode>,
    clips: HashMap<Uuid, (usize, usize)>,
    fingerprint: String,
}

/// Immutable document and compiled dependencies for repeated frame evaluation.
pub struct EvaluationSnapshot {
    project: Arc<Project>,
    assets: HashMap<Uuid, usize>,
    streams: HashMap<(Uuid, u32), StreamIndex>,
    scenes: HashMap<Uuid, Scene>,
}

/// Resolved node inputs with a reusable operation and content identity.
pub struct PreparedNode {
    pub id: Uuid,
    pub socket: SocketType,
    pub active: bool,
    pub operation: Arc<NodeOperation>,
    pub source: Option<SourceRequest>,
    pub sha256: String,
}

/// Exact temporal plan carrying both reusable content and publication ownership.
pub struct PreparedFrame {
    pub version: DocumentVersion,
    pub composition: Uuid,
    pub position: SourcePosition,
    pub before: bool,
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub color: ColorConfiguration,
    pub picture: Option<Uuid>,
    pub audio: Option<Uuid>,
    pub nodes: Vec<PreparedNode>,
    pub sha256: String,
}

impl EvaluationSnapshot {
    /// Compile one immutable document for repeated temporal evaluation.
    /// `project` is the shared document snapshot. Returns validated input orders,
    /// source interpretations and static operation hashes; performs no media IO.
    pub fn new(project: Arc<Project>) -> Result<Self> {
        project.validate()?;
        let assets: HashMap<_, _> = project
            .assets
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id, i))
            .collect();
        let mut streams = HashMap::new();
        for (media, source) in project.sources.iter().enumerate() {
            for (profile, stream) in source.streams.iter().enumerate() {
                streams.insert(
                    (source.id, stream.index),
                    StreamIndex {
                        media,
                        profile,
                        asset: assets[&source.asset],
                        fingerprint: hash(&(
                            stream.index,
                            &stream.codec,
                            stream.time_base,
                            stream.start_tick,
                            stream.duration_ticks,
                            &stream.format,
                        ))?,
                    },
                );
            }
        }
        let mut scenes = HashMap::new();
        for (index, composition) in project.compositions.iter().enumerate() {
            let clips: HashMap<_, _> = composition
                .tracks
                .iter()
                .enumerate()
                .flat_map(|(track, value)| {
                    value
                        .clips
                        .iter()
                        .enumerate()
                        .map(move |(clip, value)| (value.id, (track, clip)))
                })
                .collect();
            let kinds = clips
                .iter()
                .map(|(id, (track, _))| (*id, composition.tracks[*track].kind))
                .collect();
            let indices: HashMap<_, _> = composition
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| (n.id, i))
                .collect();
            let mut reachable = BTreeSet::new();
            let mut pending: Vec<_> = [composition.picture, composition.audio]
                .into_iter()
                .flatten()
                .collect();
            while let Some(id) = pending.pop() {
                if reachable.insert(id) {
                    pending.extend(
                        composition.nodes[indices[&id]]
                            .operation
                            .inputs()
                            .into_iter()
                            .map(|(id, _)| id),
                    );
                }
            }
            let mut counts: HashMap<_, _> = reachable
                .iter()
                .map(|id| (*id, composition.nodes[indices[id]].operation.inputs().len()))
                .collect();
            let mut dependants: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
            for id in &reachable {
                for (input, _) in composition.nodes[indices[id]].operation.inputs() {
                    dependants.entry(input).or_default().push(*id);
                }
            }
            let mut ready: BTreeSet<_> = counts
                .iter()
                .filter(|(_, count)| **count == 0)
                .map(|(id, _)| *id)
                .collect();
            let mut nodes = Vec::new();
            while let Some(id) = ready.pop_first() {
                let node = &composition.nodes[indices[&id]];
                nodes.push(CompiledNode {
                    index: indices[&id],
                    socket: node.operation.socket(&kinds)?,
                    operation: Arc::new(node.operation.clone()),
                    fingerprint: hash(&node.operation)?,
                    inputs: node
                        .operation
                        .inputs()
                        .into_iter()
                        .map(|(id, _)| id)
                        .collect(),
                });
                for dependant in dependants.get(&id).into_iter().flatten() {
                    let count = counts
                        .get_mut(dependant)
                        .ok_or_else(|| Error::Invalid("node dependency is absent".into()))?;
                    *count -= 1;
                    if *count == 0 {
                        ready.insert(*dependant);
                    }
                }
            }
            if nodes.len() != reachable.len() {
                return Err(Error::Invalid("graph did not resolve".into()));
            }
            scenes.insert(
                composition.id,
                Scene {
                    index,
                    nodes,
                    clips,
                    fingerprint: project.composition_fingerprint_validated(composition.id)?,
                },
            );
        }
        Ok(Self {
            project,
            assets,
            streams,
            scenes,
        })
    }

    /// Inspect the exact document retained by this evaluation context.
    /// Takes no arguments. Returns the immutable document, not a mutable editor.
    pub fn project(&self) -> &Arc<Project> {
        &self.project
    }

    /// Evaluate a composition at an exact temporal boundary.
    /// `composition` selects the scene, `position` is fractional scene time, and
    /// `before` selects the reverse side of boundaries. Returns shared operations,
    /// exact source requests and dependency keys without source-index rehashing.
    pub fn prepare(
        &self,
        composition: Uuid,
        position: SourcePosition,
        before: bool,
    ) -> Result<PreparedFrame> {
        let position = SourcePosition::new(position.numerator, position.denominator)?;
        let scene = self
            .scenes
            .get(&composition)
            .ok_or_else(|| Error::Invalid("composition is absent".into()))?;
        let composition = &self.project.compositions[scene.index];
        if !contains(0, composition.duration, position, before)? {
            return Err(Error::Invalid("frame is outside the composition".into()));
        }
        let mut evaluated: Vec<PreparedNode> = Vec::with_capacity(scene.nodes.len());
        let mut keys: HashMap<Uuid, String> = HashMap::new();
        for compiled in &scene.nodes {
            let node = &composition.nodes[compiled.index];
            let mut active = contains(node.range.start, node.range.end, position, before)?;
            let source = if let NodeOperation::Source { clip } = *compiled.operation {
                let (track, clip) = scene.clips[&clip];
                let track = &composition.tracks[track];
                let clip = &track.clips[clip];
                active &=
                    track.enabled && contains(clip.range.start, clip.range.end, position, before)?;
                if active {
                    let local = position.relative_to(clip.range.start as i64)?;
                    let direction = clip.time_map.direction_validated(local, before)?;
                    let source_position = clip.time_map.position_validated(local)?;
                    let reverse = direction != 0 && ((direction < 0) != before);
                    Some(self.source(clip.source, source_position, reverse)?)
                } else {
                    None
                }
            } else {
                None
            };
            let operation = if node.animation.is_empty() {
                compiled.operation.clone()
            } else {
                Arc::new(resolve_operation(
                    &compiled.operation,
                    &node.animation,
                    position,
                    before,
                )?)
            };
            let fingerprint = if node.animation.is_empty() {
                compiled.fingerprint.clone()
            } else {
                hash(&*operation)?
            };
            let inputs: Vec<_> = compiled
                .inputs
                .iter()
                .map(|id| {
                    keys.get(id)
                        .ok_or_else(|| Error::Invalid("node input order is incomplete".into()))
                })
                .collect::<Result<_>>()?;
            let source_key = match &source {
                Some(SourceRequest::Media {
                    asset,
                    asset_sha256,
                    stream_sha256,
                    picture,
                    sample,
                    position,
                    reverse,
                    ..
                }) => {
                    let bytes = self.project.assets[self.assets[asset]].bytes;
                    if compiled.socket == SocketType::Image {
                        hash(&(asset_sha256, bytes, stream_sha256, picture))?
                    } else {
                        hash(&(
                            asset_sha256,
                            bytes,
                            stream_sha256,
                            sample,
                            position,
                            reverse,
                        ))?
                    }
                }
                Some(SourceRequest::Composition {
                    composition,
                    position,
                    reverse,
                }) => hash(&(&self.scenes[composition].fingerprint, position, reverse))?,
                None => String::new(),
            };
            let asset_key = if let NodeOperation::MaskAsset { asset } = *operation {
                let asset = &self.project.assets[self.assets[&asset]];
                Some((&asset.sha256, asset.bytes))
            } else {
                None
            };
            let working = matches!(compiled.socket, SocketType::Image | SocketType::Mask)
                .then_some((
                    composition.width,
                    composition.height,
                    self.project.color.working_gamut,
                    self.project.color.precision,
                ));
            let key = if active {
                hash(&(
                    "editbay-node-1",
                    compiled.socket,
                    &fingerprint,
                    inputs,
                    source_key,
                    asset_key,
                    working,
                ))?
            } else {
                hash(&("editbay-neutral-1", compiled.socket, working))?
            };
            keys.insert(node.id, key.clone());
            evaluated.push(PreparedNode {
                id: node.id,
                socket: compiled.socket,
                active,
                operation,
                source,
                sha256: key,
            });
        }
        let sha256 = hash(&(
            "editbay-frame-1",
            composition.width,
            composition.height,
            composition.frame_rate,
            self.project.color,
            composition.picture.and_then(|id| keys.get(&id)),
            composition.audio.and_then(|id| keys.get(&id)),
        ))?;
        Ok(PreparedFrame {
            version: DocumentVersion::of(&self.project),
            composition: composition.id,
            position,
            before,
            width: composition.width,
            height: composition.height,
            frame_rate: composition.frame_rate,
            color: self.project.color,
            picture: composition.picture,
            audio: composition.audio,
            nodes: evaluated,
            sha256,
        })
    }

    /// Select a media picture/sample or an exact nested scene boundary.
    /// `reference`, `position` and `reverse` identify the retained source and side.
    /// Returns a request backed by the snapshot's validated source interpretation.
    fn source(
        &self,
        reference: ClipSource,
        position: SourcePosition,
        reverse: bool,
    ) -> Result<SourceRequest> {
        match reference {
            ClipSource::Composition { composition } => Ok(SourceRequest::Composition {
                composition,
                position,
                reverse,
            }),
            ClipSource::Media { source, stream } => {
                let index = self
                    .streams
                    .get(&(source, stream))
                    .ok_or_else(|| Error::Invalid("source stream is absent".into()))?;
                let profile = &self.project.sources[index.media].streams[index.profile];
                let asset = &self.project.assets[index.asset];
                let (picture, sample) = match &profile.format {
                    StreamFormat::Video { timing, .. } => (
                        if reverse {
                            timing.picture_before(
                                position,
                                profile.time_base,
                                profile.start_tick,
                            )?
                        } else {
                            timing.picture_at(position, profile.time_base, profile.start_tick)?
                        },
                        None,
                    ),
                    StreamFormat::Audio { sample_rate, .. } => {
                        let relative = position.relative_to(profile.start_tick)?;
                        let rate = FrameRate::new(*sample_rate, 1)?;
                        let sample = if reverse {
                            profile
                                .time_base
                                .boundary(
                                    SourcePosition::from_fraction(
                                        -i128::from(relative.numerator),
                                        relative.denominator,
                                    )?,
                                    rate,
                                )?
                                .checked_neg()
                                .and_then(|value| value.checked_sub(1))
                                .ok_or_else(|| {
                                    Error::Invalid("reverse sample boundary overflow".into())
                                })?
                        } else {
                            profile.time_base.boundary(relative, rate)?
                        };
                        (None, Some(sample))
                    }
                };
                Ok(SourceRequest::Media {
                    source,
                    stream,
                    asset: asset.id,
                    asset_sha256: asset.sha256.clone(),
                    stream_sha256: index.fingerprint.clone(),
                    position,
                    reverse,
                    picture,
                    sample,
                })
            }
        }
    }

    /// Produce the established integer-frame inspection format.
    /// `composition` and `frame` select the scene/frame. Returns the same typed
    /// inspection fields and fingerprint as the original document API.
    pub fn frame_plan(&self, composition: Uuid, frame: u64) -> Result<FramePlan> {
        let prepared = self.prepare(
            composition,
            SourcePosition::new(
                i64::try_from(frame)
                    .map_err(|_| Error::Invalid("frame is outside the composition".into()))?,
                1,
            )?,
            false,
        )?;
        let nodes: Vec<_> = prepared
            .nodes
            .into_iter()
            .map(|node| EvaluatedNode {
                id: node.id,
                socket: node.socket,
                active: node.active,
                operation: (*node.operation).clone(),
                source: node.source,
            })
            .collect();
        let sha256 = hash(&(
            composition,
            frame,
            prepared.width,
            prepared.height,
            prepared.frame_rate,
            prepared.color,
            prepared.picture,
            prepared.audio,
            &self.scenes[&composition].fingerprint,
            &nodes,
        ))?;
        Ok(FramePlan {
            version: prepared.version,
            composition,
            frame,
            width: prepared.width,
            height: prepared.height,
            frame_rate: prepared.frame_rate,
            color: prepared.color,
            picture: prepared.picture,
            audio: prepared.audio,
            nodes,
            sha256,
        })
    }
}

/// Test which side of a temporal boundary belongs to a range.
/// `start`, `end`, `position` and `before` select the interval and boundary side.
/// Returns forward [start,end) or reverse (start,end] membership without rounding.
fn contains(start: u64, end: u64, position: SourcePosition, before: bool) -> Result<bool> {
    let start = position.compare_tick(
        i64::try_from(start)
            .map_err(|_| Error::Invalid("frame range exceeds its budget".into()))?,
    )?;
    let end = position.compare_tick(
        i64::try_from(end).map_err(|_| Error::Invalid("frame range exceeds its budget".into()))?,
    )?;
    Ok(if before {
        start.is_gt() && end.is_le()
    } else {
        start.is_ge() && end.is_lt()
    })
}

/// Fingerprint a typed semantic value.
/// `value` supplies deterministic serialization. Returns its lowercase SHA-256.
fn hash(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
