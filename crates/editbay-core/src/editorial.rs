use crate::{
    AnimatedProperty, AnimationChannel, Composition, Error, FrameRange, Interpolation, Keyframe,
    NodeOperation, Project, Result, TimedNode, TrackKind,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A retained dissolve over source handles in a native composition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EditorialTransition {
    pub id: Uuid,
    pub composition: Uuid,
    pub range: FrameRange,
    pub cut: u64,
    pub from: Option<Uuid>,
    pub to: Option<Uuid>,
}

impl EditorialTransition {
    /// Validate retained transition ownership and available handles.
    /// `project` owns clips and ranges; returns an error for missing or short handles.
    pub fn validate(&self, project: &Project) -> Result<()> {
        let scene = project
            .compositions
            .iter()
            .find(|c| c.id == self.composition)
            .ok_or_else(|| Error::Invalid("Transition composition is absent".into()))?;
        self.range.validate(scene.duration)?;
        if self.id.is_nil()
            || self.cut < self.range.start
            || self.cut > self.range.end
            || (self.from.is_none() && self.to.is_none())
            || self.from == self.to
        {
            return Err(Error::Invalid(
                "Transition needs a cut and distinct clips".into(),
            ));
        }
        let mut owner = None;
        for id in [self.from, self.to].into_iter().flatten() {
            let (track, clip) = scene
                .tracks
                .iter()
                .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t, c)))
                .ok_or_else(|| Error::Invalid("Transition clip is absent".into()))?;
            if owner.is_some_and(|t| t != track.id)
                || clip.range.start > self.range.start
                || clip.range.end < self.range.end
            {
                return Err(Error::Invalid("Dissolve needs handles on one track".into()));
            }
            owner = Some(track.id);
        }
        Ok(())
    }
}

/// Build native picture and sound graphs from exact editorial clips.
/// `scene` owns placed source clips; `transitions` add retained linear dissolves.
/// Returns no value; replaces the derived graph without changing clips or timing.
pub fn rebuild_editorial(
    scene: &mut Composition,
    transitions: &[EditorialTransition],
) -> Result<()> {
    let full = FrameRange {
        start: 0,
        end: scene.duration,
    };
    let mut nodes = Vec::new();
    let mut video = None;
    let mut audio = Vec::new();
    for track in scene.tracks.iter().filter(|t| t.enabled) {
        let mut clips: Vec<_> = track.clips.iter().collect();
        clips.sort_by_key(|c| (c.range.start, c.id));
        for clip in clips {
            let source = Uuid::new_v4();
            nodes.push(TimedNode {
                id: source,
                range: clip.range,
                operation: NodeOperation::Source { clip: clip.id },
                animation: vec![],
            });
            let fades: Vec<_> = transitions
                .iter()
                .filter(|t| {
                    t.composition == scene.id
                        && (t.to == Some(clip.id)
                            || (t.from == Some(clip.id)
                                && (track.kind == TrackKind::Audio || t.to.is_none())))
                })
                .collect();
            let mut top = source;
            for fade in fades {
                let incoming = fade.to == Some(clip.id);
                let id = Uuid::new_v4();
                let property = if track.kind == TrackKind::Video {
                    AnimatedProperty::Opacity
                } else {
                    AnimatedProperty::Gain
                };
                let operation = if track.kind == TrackKind::Video {
                    NodeOperation::Transform {
                        image: top,
                        translation: [0.; 2],
                        scale: [1.; 2],
                        rotation: 0.,
                        opacity: 1.,
                    }
                } else {
                    NodeOperation::Gain {
                        audio: top,
                        gain: 1.,
                    }
                };
                nodes.push(TimedNode {
                    id,
                    range: clip.range,
                    operation,
                    animation: vec![AnimationChannel {
                        property,
                        interpolation: Interpolation::Linear,
                        keys: vec![
                            Keyframe {
                                frame: fade.range.start,
                                value: if incoming { 0. } else { 1. },
                                in_tangent: 0.,
                                out_tangent: 0.,
                            },
                            Keyframe {
                                frame: fade.range.end,
                                value: if incoming { 1. } else { 0. },
                                in_tangent: 0.,
                                out_tangent: 0.,
                            },
                        ],
                    }],
                });
                top = id;
            }
            if track.kind == TrackKind::Video {
                video = Some(if let Some(background) = video {
                    let id = Uuid::new_v4();
                    nodes.push(TimedNode {
                        id,
                        range: full,
                        operation: NodeOperation::Over {
                            foreground: top,
                            background,
                            mask: None,
                        },
                        animation: vec![],
                    });
                    id
                } else {
                    top
                });
            } else {
                audio.push(top);
            }
        }
    }
    scene.picture = video;
    scene.audio = if audio.len() == 1 {
        audio.first().copied()
    } else if audio.is_empty() {
        None
    } else {
        let id = Uuid::new_v4();
        nodes.push(TimedNode {
            id,
            range: full,
            operation: NodeOperation::Mix { inputs: audio },
            animation: vec![],
        });
        Some(id)
    };
    scene.nodes = nodes;
    Ok(())
}
