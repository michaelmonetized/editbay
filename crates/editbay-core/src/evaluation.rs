use crate::{
    AnimatedProperty, ClipSource, ColorConfiguration, DocumentVersion, Error, FrameRate,
    NodeOperation, Project, Result, SocketType, SourcePosition, StreamFormat, TimedNode,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceRequest {
    Media {
        source: Uuid,
        stream: u32,
        asset: Uuid,
        asset_sha256: String,
        stream_sha256: String,
        position: SourcePosition,
        reverse: bool,
        picture: Option<u64>,
        sample: Option<i64>,
    },
    Composition {
        composition: Uuid,
        position: SourcePosition,
        reverse: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvaluatedNode {
    pub id: Uuid,
    pub socket: SocketType,
    pub active: bool,
    pub operation: NodeOperation,
    pub source: Option<SourceRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FramePlan {
    pub version: DocumentVersion,
    pub composition: Uuid,
    pub frame: u64,
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub color: ColorConfiguration,
    pub picture: Option<Uuid>,
    pub audio: Option<Uuid>,
    pub nodes: Vec<EvaluatedNode>,
    pub sha256: String,
}

impl Project {
    /// Evaluate typed dependencies and exact source requests for one frame.
    /// `composition_id` selects the scene; `frame` is an integer composition frame.
    /// Returns deterministic input-first nodes, resolved animation, source pictures/
    /// samples and revision ownership. This is an evaluation plan, not rendered pixels.
    pub fn frame_plan(&self, composition_id: Uuid, frame: u64) -> Result<FramePlan> {
        self.validate()?;
        let composition = self
            .compositions
            .iter()
            .find(|composition| composition.id == composition_id)
            .ok_or_else(|| Error::Invalid("composition is absent".into()))?;
        if frame >= composition.duration {
            return Err(Error::Invalid("frame is outside the composition".into()));
        }
        let nodes: HashMap<_, _> = composition
            .nodes
            .iter()
            .map(|node| (node.id, node))
            .collect();
        let clips: HashMap<_, _> = composition
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter().map(move |clip| (clip.id, (track, clip))))
            .collect();
        let clip_types = clips
            .iter()
            .map(|(id, (track, _))| (*id, track.kind))
            .collect();
        let mut reachable = BTreeSet::new();
        let mut pending: Vec<_> = [composition.picture, composition.audio]
            .into_iter()
            .flatten()
            .collect();
        while let Some(id) = pending.pop() {
            if reachable.insert(id) {
                pending.extend(nodes[&id].operation.inputs().into_iter().map(|(id, _)| id));
            }
        }
        let mut remaining: HashMap<_, _> = reachable
            .iter()
            .map(|id| (*id, nodes[id].operation.inputs().len()))
            .collect();
        let mut dependants: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for id in &reachable {
            for (input, _) in nodes[id].operation.inputs() {
                dependants.entry(input).or_default().push(*id);
            }
        }
        let mut ready: BTreeSet<_> = remaining
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(id, _)| *id)
            .collect();
        let mut evaluated = Vec::with_capacity(reachable.len());
        while let Some(id) = ready.pop_first() {
            let node = nodes[&id];
            let mut active = node.range.contains(frame);
            let source = if let NodeOperation::Source { clip } = node.operation {
                let (track, clip) = clips[&clip];
                active &= track.enabled && clip.range.contains(frame);
                if active {
                    let local = frame - clip.range.start;
                    let position = clip.time_map.position(local)?;
                    let reverse = clip.time_map.direction_at(local)? < 0;
                    Some(match clip.source {
                        ClipSource::Composition { composition } => SourceRequest::Composition {
                            composition,
                            position,
                            reverse,
                        },
                        ClipSource::Media { source, stream } => {
                            let media = self
                                .sources
                                .iter()
                                .find(|media| media.id == source)
                                .ok_or_else(|| Error::Invalid("source is absent".into()))?;
                            let profile = media
                                .streams
                                .iter()
                                .find(|profile| profile.index == stream)
                                .ok_or_else(|| Error::Invalid("stream is absent".into()))?;
                            let asset = self
                                .assets
                                .iter()
                                .find(|asset| asset.id == media.asset)
                                .ok_or_else(|| Error::Invalid("source asset is absent".into()))?;
                            let (picture, sample) = match &profile.format {
                                StreamFormat::Video { timing, .. } => (
                                    if reverse {
                                        timing.picture_before(
                                            position,
                                            profile.time_base,
                                            profile.start_tick,
                                        )?
                                    } else {
                                        timing.picture_at(
                                            position,
                                            profile.time_base,
                                            profile.start_tick,
                                        )?
                                    },
                                    None,
                                ),
                                StreamFormat::Audio { sample_rate, .. } => {
                                    let relative = position.relative_to(profile.start_tick)?;
                                    let rate = FrameRate::new(*sample_rate, 1)?;
                                    let sample = if reverse {
                                        let negative = SourcePosition::from_fraction(
                                            -i128::from(relative.numerator),
                                            relative.denominator,
                                        )?;
                                        profile
                                            .time_base
                                            .boundary(negative, rate)?
                                            .checked_neg()
                                            .and_then(|sample| sample.checked_sub(1))
                                            .ok_or_else(|| {
                                                Error::Invalid(
                                                    "reverse sample boundary overflow".into(),
                                                )
                                            })?
                                    } else {
                                        profile.time_base.boundary(relative, rate)?
                                    };
                                    (None, Some(sample))
                                }
                            };
                            let stream_sha256 = format!(
                                "{:x}",
                                Sha256::digest(serde_json::to_vec(&(
                                    profile.index,
                                    &profile.codec,
                                    profile.time_base,
                                    profile.start_tick,
                                    profile.duration_ticks,
                                    &profile.format
                                ))?)
                            );
                            SourceRequest::Media {
                                source,
                                stream,
                                asset: asset.id,
                                asset_sha256: asset.sha256.clone(),
                                stream_sha256,
                                position,
                                reverse,
                                picture,
                                sample,
                            }
                        }
                    })
                } else {
                    None
                }
            } else {
                None
            };
            evaluated.push(EvaluatedNode {
                id,
                socket: node.operation.socket(&clip_types)?,
                active,
                operation: resolve(node, frame)?,
                source,
            });
            for dependant in dependants.get(&id).into_iter().flatten() {
                let count = remaining
                    .get_mut(dependant)
                    .ok_or_else(|| Error::Invalid("node dependency is absent".into()))?;
                *count -= 1;
                if *count == 0 {
                    ready.insert(*dependant);
                }
            }
        }
        if evaluated.len() != reachable.len() {
            return Err(Error::Invalid("graph did not resolve".into()));
        }
        let dependencies = self.composition_fingerprint(composition_id)?;
        let sha256 = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(
                composition.id,
                frame,
                composition.width,
                composition.height,
                composition.frame_rate,
                self.color,
                composition.picture,
                composition.audio,
                dependencies,
                &evaluated
            ))?)
        );
        Ok(FramePlan {
            version: DocumentVersion::of(self),
            composition: composition.id,
            frame,
            width: composition.width,
            height: composition.height,
            frame_rate: composition.frame_rate,
            color: self.color,
            picture: composition.picture,
            audio: composition.audio,
            nodes: evaluated,
            sha256,
        })
    }
}

impl Project {
    /// Fingerprint the semantic dependency closure of a reusable composition.
    /// `composition_id` selects a validated root. Returns a SHA-256 including
    /// nested graphs, source bytes/interpretation, masks, animation and color;
    /// project revisions, labels, metadata and unrelated scenes do not enter it.
    pub fn composition_fingerprint(&self, composition_id: Uuid) -> Result<String> {
        self.validate()?;
        let compositions: HashMap<_, _> = self
            .compositions
            .iter()
            .map(|composition| (composition.id, composition))
            .collect();
        let mut reachable = BTreeSet::new();
        let mut pending = vec![composition_id];
        while let Some(id) = pending.pop() {
            let composition = compositions
                .get(&id)
                .ok_or_else(|| Error::Invalid("composition dependency is absent".into()))?;
            if reachable.insert(id) {
                pending.extend(
                    composition
                        .tracks
                        .iter()
                        .flat_map(|track| &track.clips)
                        .filter_map(|clip| match clip.source {
                            ClipSource::Composition { composition } => Some(composition),
                            _ => None,
                        }),
                );
            }
        }
        let mut semantic = Vec::new();
        let mut sources = BTreeSet::new();
        let mut masks = BTreeSet::new();
        for id in reachable {
            let composition = compositions[&id];
            let tracks: Vec<_> = composition
                .tracks
                .iter()
                .map(|track| {
                    let clips: Vec<_> = track
                        .clips
                        .iter()
                        .map(|clip| {
                            if let ClipSource::Media { source, stream } = clip.source {
                                sources.insert((source, stream));
                            }
                            (clip.id, clip.range, clip.source, &clip.time_map)
                        })
                        .collect();
                    (track.id, track.kind, track.enabled, clips)
                })
                .collect();
            for node in &composition.nodes {
                if let NodeOperation::MaskAsset { asset } = node.operation {
                    masks.insert(asset);
                }
            }
            semantic.push(serde_json::to_value((
                id,
                composition.width,
                composition.height,
                composition.frame_rate,
                composition.duration,
                tracks,
                &composition.nodes,
                composition.picture,
                composition.audio,
            ))?);
        }
        let mut interpretations = Vec::new();
        for (id, index) in sources {
            let source = self
                .sources
                .iter()
                .find(|source| source.id == id)
                .ok_or_else(|| Error::Invalid("source dependency is absent".into()))?;
            let asset = self
                .assets
                .iter()
                .find(|asset| asset.id == source.asset)
                .ok_or_else(|| Error::Invalid("asset dependency is absent".into()))?;
            let stream = source
                .streams
                .iter()
                .find(|stream| stream.index == index)
                .ok_or_else(|| Error::Invalid("stream dependency is absent".into()))?;
            interpretations.push(serde_json::to_value((
                id,
                source.asset,
                &asset.sha256,
                asset.bytes,
                stream.index,
                &stream.codec,
                stream.time_base,
                stream.start_tick,
                stream.duration_ticks,
                &stream.format,
            ))?);
        }
        let mut mask_hashes = Vec::new();
        for id in masks {
            let asset = self
                .assets
                .iter()
                .find(|asset| asset.id == id)
                .ok_or_else(|| Error::Invalid("mask dependency is absent".into()))?;
            mask_hashes.push((id, &asset.sha256, asset.bytes));
        }
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(
                composition_id,
                self.color,
                semantic,
                interpretations,
                mask_hashes
            ))?)
        ))
    }
}

fn resolve(node: &TimedNode, frame: u64) -> Result<NodeOperation> {
    let mut operation = node.operation.clone();
    for channel in &node.animation {
        let value = channel.value_at(frame)?;
        use AnimatedProperty::*;
        match (&mut operation, channel.property) {
            (NodeOperation::Transform { translation, .. }, TranslateX) => translation[0] = value,
            (NodeOperation::Transform { translation, .. }, TranslateY) => translation[1] = value,
            (NodeOperation::Transform { scale, .. }, ScaleX) => scale[0] = value,
            (NodeOperation::Transform { scale, .. }, ScaleY) => scale[1] = value,
            (NodeOperation::Transform { rotation, .. }, Rotation) => *rotation = value,
            (NodeOperation::Transform { opacity, .. }, Opacity) => *opacity = value,
            (NodeOperation::Solid { rgba }, Red) => rgba[0] = value,
            (NodeOperation::Solid { rgba }, Green) => rgba[1] = value,
            (NodeOperation::Solid { rgba }, Blue) => rgba[2] = value,
            (NodeOperation::Solid { rgba }, Alpha) => rgba[3] = value,
            (NodeOperation::Gain { gain, .. }, Gain) => *gain = value,
            (NodeOperation::Mask { feather, .. }, Feather) => *feather = value,
            (NodeOperation::Scalar { value: scalar }, Scalar) => *scalar = value,
            _ => {
                return Err(Error::Invalid(
                    "animation property has no matching node parameter".into(),
                ));
            }
        }
    }
    Ok(operation)
}
