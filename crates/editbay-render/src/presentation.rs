use crate::{ResidentImage, Result};
use editbay_core::{OutputTransfer, WorkingGamut};
use std::sync::Arc;

/// Native SDR presentation compiled on the application's own GPU device.
pub struct DisplayRenderer {
    device: wgpu::Device,
    pipeline: wgpu::RenderPipeline,
}

/// Immutable native draw retaining its charged resident picture.
pub struct DisplayFrame {
    device: wgpu::Device,
    pipeline: wgpu::RenderPipeline,
    bindings: wgpu::BindGroup,
    image: Arc<ResidentImage>,
}

impl DisplayRenderer {
    /// Compile a native surface draw away from the input thread.
    /// `device` owns the graph and `format` is the actual UI surface format.
    /// Returns an SDR sRGB draw over an opaque transparency checkerboard;
    /// unsupported surface formats fail before GPU resource creation.
    pub fn new(device: wgpu::Device, format: wgpu::TextureFormat) -> Result<Self> {
        if !matches!(
            format,
            wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Bgra8Unorm
                | wgpu::TextureFormat::Rgba8UnormSrgb
                | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            return Err("native viewer requires an 8-bit SDR surface".into());
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("EditBay native SDR presentation"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("presentation.wgsl")
                    .replace(
                        "SURFACE_SRGB",
                        if format.is_srgb() { "true" } else { "false" },
                    )
                    .into(),
            ),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("EditBay native picture"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("EditBay native picture"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Ok(Self { device, pipeline })
    }

    pub(crate) fn prepare(&self, image: Arc<ResidentImage>) -> Result<DisplayFrame> {
        if image.device != self.device {
            return Err("resident picture belongs to a different GPU device".into());
        }
        let (_, gamut, transfer) = image.interpretation();
        if gamut != WorkingGamut::Bt709 || transfer != OutputTransfer::Srgb {
            return Err(
                "native SDR viewer requires the declared BT.709/sRGB display boundary".into(),
            );
        }
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("EditBay owned native picture"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &image.texture.create_view(&Default::default()),
                ),
            }],
        });
        Ok(DisplayFrame {
            device: self.device.clone(),
            pipeline: self.pipeline.clone(),
            bindings,
            image,
        })
    }
}

impl DisplayFrame {
    /// Check the actual paint device before recording any draw.
    /// `device` comes from the native UI callback. Returns whether it owns this picture.
    pub fn accepts_device(&self, device: &wgpu::Device) -> bool {
        self.device == *device
    }

    /// Inspect native picture geometry without exposing a raw GPU handle.
    /// Takes no arguments. Returns width and height in pixels.
    pub fn dimensions(&self) -> [u32; 2] {
        self.image.dimensions()
    }

    /// Draw an owned picture into a matching-device native render pass.
    /// `pass` supplies the viewport and clip; `completed` runs after this exact
    /// submission finishes. Returns no value. Dropping an unsubmitted command
    /// releases the pin; submitted commands keep its bytes charged until done.
    pub fn paint(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        completed: impl FnOnce() + Send + 'static,
    ) {
        let pin = self.image.clone();
        pass.on_submitted_work_done(move || {
            drop(pin);
            completed();
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bindings, &[]);
        pass.draw(0..3, 0..1);
    }
}
