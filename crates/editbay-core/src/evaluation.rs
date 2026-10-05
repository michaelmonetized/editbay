use crate::{
    AnimatedProperty, AnimationChannel, ClipSource, ColorConfiguration, DocumentVersion, Error,
    FrameRate, NodeOperation, Project, Result, SocketType, SourcePosition,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
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
    /// Inspect one composition frame through the shared temporal evaluator.
    /// `composition_id` and `frame` select the scene and integer time.
    /// Returns typed source requests and resolved parameters; repeated production
    /// evaluation retains an EvaluationSnapshot rather than recompiling each frame.
    pub fn frame_plan(&self, composition_id: Uuid, frame: u64) -> Result<FramePlan> {
        crate::EvaluationSnapshot::new(Arc::new(self.clone()))?.frame_plan(composition_id, frame)
    }
}

impl Project {
    /// Fingerprint the semantic dependency closure of a reusable composition.
    /// `composition_id` selects a validated root. Returns a SHA-256 including
    /// nested graphs, source bytes/interpretation, masks, animation and color;
    /// project revisions, labels, metadata and unrelated scenes do not enter it.
    pub fn composition_fingerprint(&self, composition_id: Uuid) -> Result<String> {
        self.validate()?;
        self.composition_fingerprint_validated(composition_id)
    }

    /// Fingerprint a dependency closure after complete document validation.
    /// `composition_id` selects the retained scene. Returns the established
    /// semantic SHA-256 without repeating the document's validation scan.
    pub(crate) fn composition_fingerprint_validated(&self, composition_id: Uuid) -> Result<String> {
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

/// Resolve animated parameters on a validated node operation.
/// `operation`, `animation`, `position` and `before` identify its parameters/time.
/// Returns an owned operation with finite values at the requested boundary side.
pub(crate) fn resolve_operation(
    operation: &NodeOperation,
    animation: &[AnimationChannel],
    position: SourcePosition,
    before: bool,
) -> Result<NodeOperation> {
    let mut operation = operation.clone();
    for channel in animation {
        let value = channel.value_validated(position, before)?;
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
