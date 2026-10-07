//! Explicit sRGB input, linear-float composition and sRGB output boundaries.

mod graph;
mod presentation;
mod text;
pub use graph::{
    GraphBudget, GraphRenderer, GraphStats, ImageBoundary, RenderedFrame, ResidentImage,
};
pub use presentation::{DisplayFrame, DisplayRenderer};

use half::f16;
use serde::Serialize;
use std::{sync::mpsc, time::Duration};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, Copy, Serialize)]
pub enum Precision {
    Half,
    Full,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum InputTransfer {
    Srgb,
    Bt709,
}

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    input: wgpu::Texture,
    output: wgpu::Texture,
    bindings: wgpu::BindGroup,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
    row_bytes: u32,
    pixel_bytes: u32,
    pub adapter: wgpu::AdapterInfo,
    pub precision: Precision,
    pub transfer: InputTransfer,
}

impl Compositor {
    /// Prepare bounded GPU composition resources.
    /// `width`, `height` set the picture size; `precision` selects FP16 or FP32 output.
    /// Returns a real adapter with persistent textures and a measured readback boundary.
    pub fn new(width: u32, height: u32, precision: Precision) -> Result<Self> {
        Self::with_transfer(width, height, precision, InputTransfer::Srgb)
    }

    /// Prepare a declared source transfer.
    /// `width`, `height` and `precision` set resources; `transfer` sets input decoding.
    /// Returns a compositor using linear BT.709/sRGB primaries.
    pub fn with_transfer(
        width: u32,
        height: u32,
        precision: Precision,
        transfer: InputTransfer,
    ) -> Result<Self> {
        if width == 0 || height == 0 || width > 8192 || height > 8192 {
            return Err("unsupported image dimensions".into());
        }
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let info = adapter.get_info();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let (format, name, pixel_bytes) = match precision {
            Precision::Half => (wgpu::TextureFormat::Rgba16Float, "rgba16float", 8),
            Precision::Full => (wgpu::TextureFormat::Rgba32Float, "rgba32float", 16),
        };
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let input = texture(
            "sRGB encoded RGBA8",
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let output = texture(
            "linear premultiplied opaque composite",
            format,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("linear exposure and alpha over"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("compose.wgsl")
                    .replace("OUTPUT_FORMAT", name)
                    .replace("INPUT_LINEAR", match transfer {
                        InputTransfer::Srgb => "select(pow((value + 0.055) / 1.055, vec3f(2.4)), value / 12.92, value <= vec3f(0.04045))",
                        InputTransfer::Bt709 => "select(pow((value + 0.099) / 1.099, vec3f(1.0 / 0.45)), value / 4.5, value < vec3f(0.081))",
                    }).into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sRGB to linear composition"),
            layout: None,
            module: &shader,
            entry_point: Some("compose"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &input.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &output.create_view(&Default::default()),
                    ),
                },
            ],
        });
        let row_bytes = (width * pixel_bytes).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("explicit export readback"),
            size: u64::from(row_bytes) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            input,
            output,
            bindings,
            readback,
            width,
            height,
            row_bytes,
            pixel_bytes,
            adapter: info,
            precision,
            transfer,
        })
    }

    /// Composite a decoded source picture.
    /// `rgba` is straight-alpha, full-range, sRGB RGBA8. Returns linear RGBA float pixels.
    /// This prototype explicitly uploads and reads back; normal preview will retain textures.
    pub fn compose(&self, rgba: &[u8]) -> Result<Vec<f32>> {
        if rgba.len() != self.width as usize * self.height as usize * 4 {
            return Err("source picture length mismatch".into());
        }
        self.queue.write_texture(
            self.input.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * 4),
                rows_per_image: Some(self.height),
            },
            self.input.size(),
        );
        let mut commands = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = commands.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bindings, &[]);
            pass.dispatch_workgroups(self.width.div_ceil(16), self.height.div_ceil(16), 1);
        }
        commands.copy_texture_to_buffer(
            self.output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.row_bytes),
                    rows_per_image: Some(self.height),
                },
            },
            self.output.size(),
        );
        let submitted = self.queue.submit([commands.finish()]);
        let (tx, rx) = mpsc::channel();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submitted),
            timeout: Some(Duration::from_secs(10)),
        })?;
        rx.recv_timeout(Duration::from_secs(10))??;
        let mapped = self.readback.slice(..).get_mapped_range()?;
        let mut output = Vec::with_capacity(rgba.len());
        for row in mapped.chunks_exact(self.row_bytes as usize) {
            let row = &row[..(self.width * self.pixel_bytes) as usize];
            match self.precision {
                Precision::Half => output.extend(
                    row.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|v| f16::from_le_bytes(*v).to_f32()),
                ),
                Precision::Full => output.extend(
                    row.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|v| f32::from_le_bytes(*v)),
                ),
            }
        }
        drop(mapped);
        self.readback.unmap();
        Ok(output)
    }
}

/// Decode the sRGB transfer function.
/// `encoded` is a normalized sRGB channel. Returns a linear channel.
pub fn to_linear(encoded: f32) -> f32 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// Encode the sRGB transfer function.
/// `linear` is a linear channel. Returns a clamped, normalized sRGB channel.
pub fn to_srgb(linear: f32) -> f32 {
    (if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    })
    .clamp(0.0, 1.0)
}

/// Evaluate the independent CPU color reference.
/// `rgba` is straight sRGB input. Returns linear exposure/alpha-over pixels.
pub fn reference(rgba: &[u8]) -> Vec<f32> {
    reference_with_transfer(rgba, InputTransfer::Srgb)
}

/// Evaluate the independent source-transfer reference.
/// `rgba` is straight full-range input; `transfer` declares source encoding.
/// Returns linear exposure/alpha-over pixels in BT.709/sRGB primaries.
pub fn reference_with_transfer(rgba: &[u8], transfer: InputTransfer) -> Vec<f32> {
    let decode = |value| match transfer {
        InputTransfer::Srgb => to_linear(value),
        InputTransfer::Bt709 => {
            if value < 0.081 {
                value / 4.5
            } else {
                ((value + 0.099) / 1.099).powf(1.0 / 0.45)
            }
        }
    };
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let alpha = f32::from(p[3]) / 255.0 * 0.8;
            [
                decode(f32::from(p[0]) / 255.0) * 1.5 * alpha + 0.02 * (1.0 - alpha),
                decode(f32::from(p[1]) / 255.0) * 1.5 * alpha + 0.02 * (1.0 - alpha),
                decode(f32::from(p[2]) / 255.0) * 1.5 * alpha + 0.02 * (1.0 - alpha),
                1.0,
            ]
        })
        .collect()
}

/// Encode the linear prototype composite for the codec boundary.
/// `linear` contains opaque linear RGBA. Returns full-range sRGB RGBA8.
pub fn encode(linear: &[f32]) -> Vec<u8> {
    linear
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            [
                (to_srgb(p[0]) * 255.0).round() as u8,
                (to_srgb(p[1]) * 255.0).round() as u8,
                (to_srgb(p[2]) * 255.0).round() as u8,
                255,
            ]
        })
        .collect()
}
