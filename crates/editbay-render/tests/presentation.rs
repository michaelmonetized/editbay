use editbay_core::*;
use editbay_media::{Cancellation, PictureBudget, PictureCache};
use editbay_render::{DisplayRenderer, GraphBudget, GraphRenderer, ImageBoundary};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use uuid::Uuid;

fn fixture(precision: FloatPrecision) -> Arc<EvaluationSnapshot> {
    let mut project = Project::new("Native presentation fixture").unwrap();
    project.color.precision = precision;
    let source = Uuid::from_u128(2);
    let transform = Uuid::from_u128(3);
    let range = FrameRange { start: 0, end: 2 };
    project.compositions.push(Composition {
        id: Uuid::from_u128(1),
        name: "Orientation and alpha".into(),
        width: 8,
        height: 6,
        frame_rate: FrameRate::new(24, 1).unwrap(),
        duration: 2,
        tracks: vec![],
        audio: None,
        picture: Some(transform),
        nodes: vec![
            TimedNode {
                id: source,
                range,
                animation: vec![],
                operation: NodeOperation::Solid {
                    rgba: [0.6, 0.2, 0.1, 0.5],
                },
            },
            TimedNode {
                id: transform,
                range,
                animation: vec![],
                operation: NodeOperation::Transform {
                    image: source,
                    translation: [2., 1.],
                    scale: [1., 1.],
                    rotation: 0.,
                    opacity: 1.,
                },
            },
        ],
    });
    Arc::new(EvaluationSnapshot::new(Arc::new(project)).unwrap())
}

fn renderer(
    snapshot: Arc<EvaluationSnapshot>,
    adapter: &wgpu::Adapter,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> GraphRenderer {
    let cancel = Cancellation::new().unwrap();
    let provider =
        PictureCache::new(snapshot.clone(), PictureBudget::default(), cancel.clone()).unwrap();
    GraphRenderer::with_device(
        snapshot,
        provider,
        GraphBudget::default(),
        cancel,
        adapter,
        device.clone(),
        queue.clone(),
    )
    .unwrap()
}

#[test]
fn native_surface_pixels_match_orientation_alpha_scaling_and_color_reference() {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    for precision in [FloatPrecision::Half, FloatPrecision::Full] {
        let mut graph = renderer(fixture(precision), &adapter, &device, &queue);
        let working = graph
            .render(
                Uuid::from_u128(1),
                SourcePosition::new(0, 1).unwrap(),
                false,
            )
            .unwrap();
        let display = graph.convert(&working, ImageBoundary::Display).unwrap();
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ] {
            let presenter = DisplayRenderer::new(device.clone(), format).unwrap();
            let picture = graph.present(&display, &presenter).unwrap();
            assert!(picture.accepts_device(&device));
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 32,
                    height: 24,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256 * 24,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target.create_view(&Default::default()),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                picture.paint(&mut pass, || {});
            }
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(24),
                    },
                },
                target.size(),
            );
            let (tx, rx) = mpsc::channel();
            encoder.map_buffer_on_submit(&readback, wgpu::MapMode::Read, .., move |result| {
                tx.send(result).unwrap();
            });
            queue.submit([encoder.finish()]);
            graph.finish().unwrap();
            rx.recv().unwrap().unwrap();
            let mapped = readback.slice(..).get_mapped_range().unwrap();
            for y in 0..24 {
                for x in 0..32 {
                    let sx = (x as f32 + 0.5) / 4. - 0.5;
                    let sy = (y as f32 + 0.5) / 4. - 0.5;
                    let coverage = (sx - 1.).clamp(0., 1.) * sy.clamp(0., 1.);
                    let background = if (x / 16 + y / 16) % 2 == 0 {
                        0.16
                    } else {
                        0.22
                    };
                    let alpha = 0.5 * coverage;
                    let expected = [0.6, 0.2, 0.1].map(|linear| {
                        (editbay_render::to_srgb(linear) * alpha + background * (1. - alpha)) * 255.
                    });
                    let pixel = &mapped[y * 256 + x * 4..y * 256 + x * 4 + 4];
                    for channel in 0..3 {
                        let index = if matches!(
                            format,
                            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
                        ) {
                            2 - channel
                        } else {
                            channel
                        };
                        assert!(
                            (f32::from(pixel[index]) - expected[channel]).abs() <= 1.2,
                            "{precision:?}/{format:?} at {x},{y}: {pixel:?} vs {expected:?}"
                        );
                    }
                    assert_eq!(pixel[3], 255);
                }
            }
            drop(mapped);
            readback.unmap();
        }
        assert_eq!(graph.stats().readbacks, 0);
    }
}

#[test]
fn unsubmitted_and_submitted_native_draws_keep_allocations_charged() {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    for submit in [false, true] {
        let snapshot = fixture(FloatPrecision::Full);
        let mut graph = renderer(snapshot.clone(), &adapter, &device, &queue);
        let presenter =
            DisplayRenderer::new(device.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
        let frame = graph
            .render(
                Uuid::from_u128(1),
                SourcePosition::new(0, 1).unwrap(),
                false,
            )
            .unwrap();
        assert!(graph.present(&frame, &presenter).is_err());
        let display = graph.convert(&frame, ImageBoundary::Display).unwrap();
        let other = renderer(snapshot, &adapter, &device, &queue);
        assert!(other.present(&display, &presenter).is_err());
        let (foreign, _) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let foreign_presenter =
            DisplayRenderer::new(foreign, wgpu::TextureFormat::Rgba8Unorm).unwrap();
        assert!(graph.present(&display, &foreign_presenter).is_err());
        let picture = graph.present(&display, &presenter).unwrap();
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 8,
                height: 6,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        let completed = Arc::new(AtomicBool::new(false));
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.create_view(&Default::default()),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let flag = completed.clone();
            picture.paint(&mut pass, move || {
                flag.store(true, Ordering::Release);
            });
        }
        drop(picture);
        drop(display);
        drop(frame);
        graph.clear().unwrap();
        assert_eq!(graph.stats().live_texture_bytes, 8 * 6 * 16);
        assert!(!completed.load(Ordering::Acquire));
        let command = encoder.finish();
        if submit {
            queue.submit([command]);
        } else {
            drop(command);
        }
        graph.finish().unwrap();
        assert_eq!(completed.load(Ordering::Acquire), submit);
        assert_eq!(graph.stats().live_texture_bytes, 0);
    }
}
