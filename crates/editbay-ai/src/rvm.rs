use crate::{Cancellation, Error, FloatTensor, ModelSession, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const RVM_SHA256: &str = "88d4531297118f595bf2fd60f6f566aec2e559393802d1f436c380f0cbbd2828";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RvmState {
    pub schema: u32,
    pub model_sha256: String,
    pub source_sha256: String,
    pub width: usize,
    pub height: usize,
    pub downsample_ratio: f32,
    pub next_frame: u64,
    pub recurrent: [FloatTensor; 4],
}

impl RvmState {
    /// Validate resumable source-owned recurrent state.
    /// Returns success only for this exact model, finite bounded configuration and tensors.
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1
            || self.model_sha256 != RVM_SHA256
            || self.source_sha256.len() != 64
            || !self.source_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || self.width == 0
            || self.height == 0
            || self.width > 3840
            || self.height > 2160
            || !self.downsample_ratio.is_finite()
            || !(0.05..=1.0).contains(&self.downsample_ratio)
        {
            return Err(Error::Invalid("incompatible RVM recurrent state".into()));
        }
        let scaled_width = (self.width as f32 * self.downsample_ratio).floor() as usize;
        let scaled_height = (self.height as f32 * self.downsample_ratio).floor() as usize;
        if scaled_width == 0 || scaled_height == 0 {
            return Err(Error::Invalid(
                "RVM downsample produces an empty picture".into(),
            ));
        }
        for (index, tensor) in self.recurrent.iter().enumerate() {
            tensor.validate()?;
            let stride = 1 << (index + 1);
            let shape = [
                1,
                [16, 20, 40, 64][index],
                scaled_height.div_ceil(stride),
                scaled_width.div_ceil(stride),
            ];
            if (self.next_frame == 0 && (tensor.shape != [1, 1, 1, 1] || tensor.values != [0.0]))
                || (self.next_frame > 0 && tensor.shape != shape)
            {
                return Err(Error::Invalid(
                    "RVM recurrent geometry differs from shot configuration".into(),
                ));
            }
        }
        Ok(())
    }
}

pub struct Matte {
    pub frame: u64,
    pub width: usize,
    pub height: usize,
    pub alpha: Vec<f32>,
    pub foreground_rgb_chw: Vec<f32>,
}

pub struct Rvm {
    model: ModelSession,
    state: RvmState,
}

impl Rvm {
    /// Begin a human-matting shot.
    /// `model` must match the approved official FP32 artifact; `source_sha256`, dimensions
    /// and `downsample_ratio` own this ordered shot. Returns fresh zero recurrent state.
    pub fn open(
        model: &Path,
        source_sha256: String,
        width: usize,
        height: usize,
        downsample_ratio: f32,
        cancellation: &Cancellation,
    ) -> Result<Self> {
        let state = RvmState {
            schema: 1,
            model_sha256: RVM_SHA256.into(),
            source_sha256,
            width,
            height,
            downsample_ratio,
            next_frame: 0,
            recurrent: std::array::from_fn(|_| FloatTensor {
                shape: vec![1, 1, 1, 1],
                values: vec![0.0],
            }),
        };
        Self::restore(model, state, cancellation)
    }

    /// Restore a validated matting snapshot.
    /// `model` and `state` must retain the exact artifact and source/configuration identity.
    /// Returns a native session ready for `state.next_frame`; no prior frame is guessed.
    pub fn restore(model: &Path, state: RvmState, cancellation: &Cancellation) -> Result<Self> {
        state.validate()?;
        let model = ModelSession::load(model, RVM_SHA256, cancellation)?;
        Ok(Self { model, state })
    }

    /// Copy the current shot memory.
    /// Takes no arguments; returns source-owned state for serialization or validated resume.
    pub fn snapshot(&self) -> RvmState {
        self.state.clone()
    }

    /// Matte the next original source picture.
    /// `frame` must be the next source index; `rgba` is straight RGB/range in original
    /// coordinates. Returns soft alpha and foreground RGB. Failed/cancelled work cannot
    /// advance recurrent state, and shape/configuration changes require a new shot.
    pub fn process(
        &mut self,
        frame: u64,
        rgba: &[u8],
        cancellation: &Cancellation,
    ) -> Result<Matte> {
        cancellation.check()?;
        if frame != self.state.next_frame || rgba.len() != self.state.width * self.state.height * 4
        {
            return Err(Error::Invalid(
                "matting requires ordered source frames and unchanged dimensions".into(),
            ));
        }
        let next_frame = frame
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("source frame overflow".into()))?;
        let pixels = self.state.width * self.state.height;
        let mut rgb = vec![0.0; pixels * 3];
        for (index, pixel) in rgba.as_chunks::<4>().0.iter().enumerate() {
            for channel in 0..3 {
                rgb[channel * pixels + index] = f32::from(pixel[channel]) / 255.0;
            }
        }
        let mut inputs = vec![
            (
                "src",
                FloatTensor {
                    shape: vec![1, 3, self.state.height, self.state.width],
                    values: rgb,
                },
            ),
            (
                "downsample_ratio",
                FloatTensor {
                    shape: vec![1],
                    values: vec![self.state.downsample_ratio],
                },
            ),
        ];
        for (name, tensor) in ["r1i", "r2i", "r3i", "r4i"]
            .into_iter()
            .zip(&self.state.recurrent)
        {
            inputs.push((name, tensor.clone()));
        }
        let mut outputs = self.model.run(inputs, cancellation)?;
        let mut take = |name: &str| {
            outputs
                .remove(name)
                .ok_or_else(|| Error::Invalid(format!("model omitted {name}")))
        };
        let foreground = take("fgr")?;
        let alpha = take("pha")?;
        if foreground.shape != [1, 3, self.state.height, self.state.width]
            || alpha.shape != [1, 1, self.state.height, self.state.width]
            || alpha
                .values
                .iter()
                .chain(&foreground.values)
                .any(|&v| !(-0.0001..=1.0001).contains(&v))
        {
            return Err(Error::Invalid(
                "model returned incompatible alpha/foreground output".into(),
            ));
        }
        let recurrent = [take("r1o")?, take("r2o")?, take("r3o")?, take("r4o")?];
        let mut next_state = self.state.clone();
        next_state.recurrent = recurrent;
        next_state.next_frame = next_frame;
        next_state.validate()?;
        cancellation.check()?;
        self.state = next_state;
        Ok(Matte {
            frame,
            width: self.state.width,
            height: self.state.height,
            alpha: alpha.values,
            foreground_rgb_chw: foreground.values,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resume_cannot_substitute_zero_state_or_unrelated_recurrent_geometry() {
        let mut state = RvmState {
            schema: 1,
            model_sha256: RVM_SHA256.into(),
            source_sha256: "0".repeat(64),
            width: 1280,
            height: 720,
            downsample_ratio: 0.25,
            next_frame: 0,
            recurrent: std::array::from_fn(|_| FloatTensor {
                shape: vec![1, 1, 1, 1],
                values: vec![0.0],
            }),
        };
        assert!(state.validate().is_ok());
        state.recurrent[0].values[0] = 1.0;
        assert!(state.validate().is_err());
        state.recurrent[0].values[0] = 0.0;
        state.next_frame = 1;
        assert!(state.validate().is_err());
        state.recurrent = std::array::from_fn(|i| {
            let stride = 1 << (i + 1);
            let shape = vec![
                1,
                [16, 20, 40, 64][i],
                180usize.div_ceil(stride),
                320usize.div_ceil(stride),
            ];
            FloatTensor {
                values: vec![0.0; shape.iter().product()],
                shape,
            }
        });
        assert!(state.validate().is_ok());
        state.downsample_ratio = 0.5;
        assert!(state.validate().is_err());
    }
}
