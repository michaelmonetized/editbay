use crate::{Cancellation, Error, FloatTensor, ModelSession, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

pub const SAM2_REVISION: &str = "3b2984dd865f6e9d2cc6aed0be6a5a5c2eb352ce";
pub const SAM2_ARTIFACTS: [(&str, &str); 5] = [
    (
        "vision_encoder.onnx",
        "aa7a8542942f042e235a993a1ab0ccf5f049918500577802a7f10ec1b39bb873",
    ),
    (
        "mask_decoder.onnx",
        "0461896de3db00936fe1643506f129d71cb6d5d2ae15754811756b7ea1b070c6",
    ),
    (
        "memory_encoder.onnx",
        "580d246c109de88838f600ba7c1c0d03d1fe267f7641b06f8c88c5f0dc5834cd",
    ),
    (
        "memory_attention.onnx",
        "791e648ce8f5ef91ad00ba06e83066ff261ae5a645f0b042b4f46a89fd054baf",
    ),
    (
        "pointer_tpos.onnx",
        "7e71df1d75dba09bc18dd4ae745c2a2cceebdd14d84eadea2fe65d0205f101fc",
    ),
];
pub const SAM2_CONSTANTS_SHA256: &str =
    "172edc70e892aab0aef6caedf97b8e4091fbf6c5d26dac5fd9ea69d093d927d3";

#[derive(Deserialize)]
struct Constants {
    memory_temporal_positional_encoding: Vec<Vec<f32>>,
    image_mean: [f32; 3],
    image_std: [f32; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PointPrompt {
    pub x: f32,
    pub y: f32,
    pub foreground: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct MemoryFrame {
    frame: u64,
    tokens: FloatTensor,
    position: FloatTensor,
    pointer: FloatTensor,
}

impl MemoryFrame {
    fn validate(&self) -> Result<()> {
        for tensor in [&self.tokens, &self.position, &self.pointer] {
            tensor.validate()?;
        }
        if self.tokens.shape != [4096, 1, 64]
            || self.position.shape != [4096, 1, 64]
            || self.pointer.shape != [1, 1, 256]
        {
            return Err(Error::Invalid("incompatible SAM memory geometry".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Sam2State {
    pub schema: u32,
    pub model_revision: String,
    pub source_sha256: String,
    pub width: usize,
    pub height: usize,
    pub total_frames: u64,
    pub next_frame: u64,
    pub prompts: Vec<PointPrompt>,
    seed: Option<MemoryFrame>,
    recent: Vec<MemoryFrame>,
}

impl Sam2State {
    /// Validate a source-owned, bounded single-object video snapshot.
    /// Returns success for the pinned pack, original coordinates and contiguous memory.
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1
            || self.model_revision != SAM2_REVISION
            || self.source_sha256.len() != 64
            || !self.source_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || self.width == 0
            || self.height == 0
            || self.width > 3840
            || self.height > 2160
            || !(2..=1_000_000).contains(&self.total_frames)
            || self.next_frame > self.total_frames
            || self.prompts.is_empty()
            || self.prompts.len() > 64
            || !self.prompts.iter().any(|p| p.foreground)
            || self.prompts.iter().any(|p| {
                !p.x.is_finite()
                    || !p.y.is_finite()
                    || p.x < 0.0
                    || p.y < 0.0
                    || p.x >= self.width as f32
                    || p.y >= self.height as f32
            })
        {
            return Err(Error::Invalid("incompatible SAM video state".into()));
        }
        if self.next_frame == 0 {
            if self.seed.is_some() || !self.recent.is_empty() {
                return Err(Error::Invalid("unstarted SAM state has memory".into()));
            }
        } else {
            let seed = self
                .seed
                .as_ref()
                .ok_or_else(|| Error::Invalid("SAM seed memory absent".into()))?;
            seed.validate()?;
            if seed.frame != 0 || self.recent.len() != (self.next_frame - 1).min(15) as usize {
                return Err(Error::Invalid("SAM memory is not contiguous".into()));
            }
            for (i, memory) in self.recent.iter().enumerate() {
                memory.validate()?;
                if memory.frame != self.next_frame - self.recent.len() as u64 + i as u64 {
                    return Err(Error::Invalid("SAM memory is not contiguous".into()));
                }
            }
        }
        Ok(())
    }
}

pub struct Segmentation {
    pub frame: u64,
    pub width: usize,
    pub height: usize,
    pub logits: Vec<f32>,
    pub predicted_iou: f32,
    pub object_score: f32,
}

pub struct Sam2 {
    vision: ModelSession,
    decoder: ModelSession,
    encoder: ModelSession,
    attention: ModelSession,
    pointer: ModelSession,
    constants: Constants,
    state: Sam2State,
}

impl Sam2 {
    /// Load or resume one prompted original-source shot.
    /// `pack` contains the five pinned graphs and constants; `state` owns the source,
    /// prompts and current memory. Returns the full native video pipeline at its next frame.
    pub fn open(pack: &Path, state: Sam2State, cancellation: &Cancellation) -> Result<Self> {
        state.validate()?;
        let constants = std::fs::read(pack.join("constants.json"))?;
        if format!("{:x}", Sha256::digest(&constants)) != SAM2_CONSTANTS_SHA256 {
            return Err(Error::Integrity);
        }
        let constants: Constants = serde_json::from_slice(&constants)?;
        if constants.memory_temporal_positional_encoding.len() != 7
            || constants
                .memory_temporal_positional_encoding
                .iter()
                .any(|row| row.len() != 64)
        {
            return Err(Error::Invalid(
                "SAM temporal constants shape differs".into(),
            ));
        }
        let mut sessions = Vec::new();
        for (name, digest) in SAM2_ARTIFACTS {
            sessions.push(ModelSession::load(&pack.join(name), digest, cancellation)?);
        }
        let mut sessions = sessions.into_iter();
        Ok(Self {
            vision: sessions.next().unwrap(),
            decoder: sessions.next().unwrap(),
            encoder: sessions.next().unwrap(),
            attention: sessions.next().unwrap(),
            pointer: sessions.next().unwrap(),
            constants,
            state,
        })
    }

    /// Construct fresh source state.
    /// Returns a snapshot with point prompts, an explicit selected range and empty memory.
    pub fn fresh_state(
        source_sha256: String,
        width: usize,
        height: usize,
        total_frames: u64,
        prompts: Vec<PointPrompt>,
    ) -> Sam2State {
        Sam2State {
            schema: 1,
            model_revision: SAM2_REVISION.into(),
            source_sha256,
            width,
            height,
            total_frames,
            next_frame: 0,
            prompts,
            seed: None,
            recent: Vec::new(),
        }
    }

    /// Copy the current video memory.
    /// Takes no arguments; returns source-owned state for serialization or validated resume.
    pub fn snapshot(&self) -> Sam2State {
        self.state.clone()
    }

    /// Select independent Rust kernels for numerical qualification.
    /// `pack` supplies the exact pinned graphs. Returns after replacing all sessions;
    /// failure preserves the existing complete pipeline. Reference cancellation is checked
    /// between graphs. This path is a lab comparator, not the cancellable artist worker.
    #[cfg(feature = "tract-reference")]
    pub fn use_reference(&mut self, pack: &Path, cancel: &Cancellation) -> Result<()> {
        let mut sessions = Vec::new();
        for (name, digest) in SAM2_ARTIFACTS {
            sessions.push(ModelSession::load_reference(
                &pack.join(name),
                digest,
                cancel,
            )?);
        }
        let mut sessions = sessions.into_iter();
        self.vision = sessions.next().unwrap();
        self.decoder = sessions.next().unwrap();
        self.encoder = sessions.next().unwrap();
        self.attention = sessions.next().unwrap();
        self.pointer = sessions.next().unwrap();
        Ok(())
    }

    /// Propagate one source picture through actual model memory.
    /// `frame` must be the next index; `rgba` retains original dimensions. Returns source
    /// logits and model confidence, distinct from opacity alpha. Cancellation/failure
    /// leaves memory untouched. New prompts, cuts and changed sources require a new shot.
    pub fn process(
        &mut self,
        frame: u64,
        rgba: &[u8],
        cancel: &Cancellation,
    ) -> Result<Segmentation> {
        cancel.check()?;
        if frame != self.state.next_frame
            || frame >= self.state.total_frames
            || rgba.len() != self.state.width * self.state.height * 4
        {
            return Err(Error::Invalid(
                "SAM requires ordered unchanged source pictures".into(),
            ));
        }
        let pixels = self.preprocess(rgba);
        let mut features = self.vision.run(vec![("pixel_values", pixels)], cancel)?;
        let raw = take(&mut features, "feats2", &[1, 256, 64, 64])?;
        let conditioned = if frame == 0 {
            take(&mut features, "feats2_no_mem", &[1, 256, 64, 64])?
        } else {
            let pos = take(&mut features, "vision_pos_embed", &[1, 256, 64, 64])?;
            self.condition(&raw, &pos, cancel)?
        };
        let (points, labels) = if frame == 0 {
            (
                self.state
                    .prompts
                    .iter()
                    .flat_map(|p| {
                        [
                            p.x * 1024.0 / self.state.width as f32,
                            p.y * 1024.0 / self.state.height as f32,
                        ]
                    })
                    .collect(),
                self.state
                    .prompts
                    .iter()
                    .map(|p| i32::from(p.foreground))
                    .collect::<Vec<_>>(),
            )
        } else {
            (vec![0.0, 0.0], vec![-1])
        };
        let point_count = labels.len();
        let mut masks = self.decoder.run_with_labels(
            vec![
                ("feats0", take(&mut features, "feats0", &[1, 32, 256, 256])?),
                ("feats1", take(&mut features, "feats1", &[1, 64, 128, 128])?),
                ("feats2_cond", conditioned),
                ("input_points", tensor(vec![1, 1, point_count, 2], points)),
            ],
            Some(("input_labels", vec![1, 1, point_count], labels)),
            cancel,
        )?;
        let mask = take(&mut masks, "high_res_mask", &[1, 1, 1024, 1024])?;
        let score = take(&mut masks, "object_score_logits", &[1, 1, 1])?.values[0];
        let iou = take(&mut masks, "iou", &[1, 1])?.values[0];
        let pointer = take(&mut masks, "object_pointer", &[1, 1, 256])?;
        let logits = resize_plane(
            &mask.values,
            1024,
            1024,
            self.state.width,
            self.state.height,
        );
        let mut memory = self.encoder.run(
            vec![
                ("feats2", raw),
                ("high_res_mask", mask),
                ("object_score_logits", tensor(vec![1, 1], vec![score])),
                (
                    "binarize",
                    tensor(vec![], vec![if frame == 0 { 1.0 } else { 0.0 }]),
                ),
            ],
            cancel,
        )?;
        let memory = MemoryFrame {
            frame,
            tokens: take(&mut memory, "memory_tokens", &[4096, 1, 64])?,
            position: take(&mut memory, "memory_pos", &[4096, 1, 64])?,
            pointer,
        };
        memory.validate()?;
        cancel.check()?;
        if frame == 0 {
            self.state.seed = Some(memory);
        } else {
            self.state.recent.push(memory);
            if self.state.recent.len() > 15 {
                self.state.recent.remove(0);
            }
        }
        self.state.next_frame += 1;
        Ok(Segmentation {
            frame,
            width: self.state.width,
            height: self.state.height,
            logits,
            predicted_iou: iou,
            object_score: score,
        })
    }

    fn preprocess(&self, rgba: &[u8]) -> FloatTensor {
        let mut data = Vec::with_capacity(3 * 1024 * 1024);
        for c in 0..3 {
            let original: Vec<f32> = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| f32::from(p[c]) / 255.0)
                .collect();
            let plane = resize_plane(&original, self.state.width, self.state.height, 1024, 1024);
            data.extend(
                plane
                    .iter()
                    .map(|p| (p - self.constants.image_mean[c]) / self.constants.image_std[c]),
            );
        }
        tensor(vec![1, 3, 1024, 1024], data)
    }

    fn condition(
        &mut self,
        raw: &FloatTensor,
        pos: &FloatTensor,
        cancel: &Cancellation,
    ) -> Result<FloatTensor> {
        let seed = self
            .state
            .seed
            .as_ref()
            .ok_or_else(|| Error::Invalid("missing seed memory".into()))?;
        let current = self.state.next_frame;
        let mut blocks = vec![(seed, 6)];
        for offset in (1..=6).rev() {
            if let Some(memory) = self
                .state
                .recent
                .iter()
                .find(|m| m.frame + offset == current)
            {
                blocks.push((memory, offset as usize - 1));
            }
        }
        while blocks.len() < 7 {
            blocks.push(*blocks.last().unwrap());
        }
        let mut tokens = Vec::with_capacity(28736 * 64);
        let mut positions = Vec::with_capacity(28736 * 64);
        for (memory, temporal) in blocks {
            tokens.extend_from_slice(&memory.tokens.values);
            positions.extend(memory.position.values.iter().enumerate().map(|(i, p)| {
                p + self.constants.memory_temporal_positional_encoding[temporal][i % 64]
            }));
        }
        let mut pointers = vec![(seed, current)];
        for offset in 1..16 {
            if let Some(memory) = self
                .state
                .recent
                .iter()
                .find(|m| m.frame + offset == current)
            {
                pointers.push((memory, offset));
            }
        }
        while pointers.len() < 16 {
            pointers.push(*pointers.last().unwrap());
        }
        let divisor = (self.state.total_frames.min(16) - 1) as f32;
        let differences = pointers
            .iter()
            .map(|(_, offset)| *offset as f32 / divisor)
            .collect();
        let mut temporal = self.pointer.run(
            vec![("normalized_diffs", tensor(vec![16], differences))],
            cancel,
        )?;
        let temporal = take(&mut temporal, "pointer_pos", &[16, 64])?;
        for (i, (memory, _)) in pointers.iter().enumerate() {
            tokens.extend_from_slice(&memory.pointer.values);
            for _ in 0..4 {
                positions.extend_from_slice(&temporal.values[i * 64..(i + 1) * 64]);
            }
        }
        let transpose = |data: &FloatTensor| {
            let mut values = vec![0.0; 4096 * 256];
            for c in 0..256 {
                for i in 0..4096 {
                    values[i * 256 + c] = data.values[c * 4096 + i];
                }
            }
            tensor(vec![4096, 1, 256], values)
        };
        let mut output = self.attention.run(
            vec![
                ("current_vision_features", transpose(raw)),
                ("current_vision_position_embeddings", transpose(pos)),
                ("memory", tensor(vec![28736, 1, 64], tokens)),
                ("memory_pos", tensor(vec![28736, 1, 64], positions)),
            ],
            cancel,
        )?;
        take(&mut output, "conditioned_feats", &[1, 256, 64, 64])
    }
}

fn tensor(shape: Vec<usize>, values: Vec<f32>) -> FloatTensor {
    FloatTensor { shape, values }
}

fn take(
    outputs: &mut BTreeMap<String, FloatTensor>,
    name: &str,
    shape: &[usize],
) -> Result<FloatTensor> {
    let tensor = outputs
        .remove(name)
        .ok_or_else(|| Error::Invalid(format!("SAM graph omitted {name}")))?;
    if tensor.shape != shape {
        return Err(Error::Invalid(format!("SAM {name} shape differs")));
    }
    Ok(tensor)
}

fn resize_plane(
    input: &[f32],
    width: usize,
    height: usize,
    output_width: usize,
    output_height: usize,
) -> Vec<f32> {
    let mut output = Vec::with_capacity(output_width * output_height);
    for y in 0..output_height {
        let fy = ((y as f32 + 0.5) * height as f32 / output_height as f32 - 0.5).max(0.0);
        let y0 = (fy.floor() as usize).min(height - 1);
        let y1 = (y0 + 1).min(height - 1);
        let wy = fy - y0 as f32;
        for x in 0..output_width {
            let fx = ((x as f32 + 0.5) * width as f32 / output_width as f32 - 0.5).max(0.0);
            let x0 = (fx.floor() as usize).min(width - 1);
            let x1 = (x0 + 1).min(width - 1);
            let wx = fx - x0 as f32;
            let top = input[y0 * width + x0] * (1.0 - wx) + input[y0 * width + x1] * wx;
            let bottom = input[y1 * width + x0] * (1.0 - wx) + input[y1 * width + x1] * wx;
            output.push(top * (1.0 - wy) + bottom * wy);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forged_memory_nonfinite_prompts_and_wrong_source_geometry_are_rejected() {
        let mut state = Sam2::fresh_state(
            "0".repeat(64),
            1280,
            720,
            20,
            vec![PointPrompt {
                x: 0.0,
                y: 0.0,
                foreground: true,
            }],
        );
        assert!(state.validate().is_ok());
        state.next_frame = 1;
        assert!(state.validate().is_err());
        state.next_frame = 0;
        state.prompts[0].x = f32::NAN;
        assert!(state.validate().is_err());
        state.prompts[0].x = 1280.0;
        assert!(state.validate().is_err());
    }
    #[test]
    fn source_coordinate_resampling_preserves_constants_and_pixel_centers() {
        assert_eq!(resize_plane(&[3.0; 6], 3, 2, 17, 11), vec![3.0; 187]);
        assert_eq!(resize_plane(&[0.0, 2.0], 2, 1, 4, 1), [0.0, 0.5, 1.5, 2.0]);
    }
}
