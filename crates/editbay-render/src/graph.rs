use crate::Result;
use editbay_core::{
    AlphaMode, DocumentVersion, EvaluationSnapshot, FloatPrecision, NodeOperation, OutputTransfer,
    PreparedFrame, SocketType, SourcePosition, SourceRequest, StreamFormat, WorkingGamut,
};
use editbay_media::{
    Cancellation, PictureBudget, PictureCache, PictureCacheStats, PictureProvider,
};
use half::f16;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;
use wgpu::util::DeviceExt;

const KERNEL: &str = "editbay-resident-sdr-picture-v1";

/// Separate working-image residency and bounded evaluation limits.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct GraphBudget {
    pub cache_bytes: usize,
    pub live_bytes: usize,
    pub maximum_texture_bytes: usize,
    pub cache_entries: usize,
    pub nodes_per_request: usize,
    pub nesting_depth: usize,
    pub pending_submissions: usize,
}
impl Default for GraphBudget {
    fn default() -> Self {
        Self {
            cache_bytes: 512 * 1024 * 1024,
            live_bytes: 768 * 1024 * 1024,
            maximum_texture_bytes: 128 * 1024 * 1024,
            cache_entries: 256,
            nodes_per_request: 4096,
            nesting_depth: 32,
            pending_submissions: 16,
        }
    }
}
impl GraphBudget {
    /// Check the caller's texture and graph limits.
    /// Takes this configuration. Returns success for bounded supported limits;
    /// zero cache bytes or entries permits uncached resident results.
    pub fn validate(self) -> Result<()> {
        if self.cache_bytes > 2 * 1024 * 1024 * 1024
            || self.live_bytes > 3 * 1024 * 1024 * 1024
            || self.live_bytes < self.maximum_texture_bytes
            || !(16..=8192 * 8192 * 16).contains(&self.maximum_texture_bytes)
            || self.cache_entries > 4096
            || !(1..=32768).contains(&self.nodes_per_request)
            || !(1..=64).contains(&self.nesting_depth)
            || !(1..=64).contains(&self.pending_submissions)
        {
            return Err("GPU graph budgets exceed supported limits".into());
        }
        Ok(())
    }
}
struct Allocation {
    bytes: usize,
    live: Arc<AtomicUsize>,
}
impl Drop for Allocation {
    fn drop(&mut self) {
        self.live.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Immutable resident picture with charged ownership through GPU completion.
pub struct ResidentImage {
    texture: wgpu::Texture,
    key: String,
    precision: FloatPrecision,
    gamut: WorkingGamut,
    transfer: OutputTransfer,
    _allocation: Allocation,
}
impl ResidentImage {
    /// Inspect resident image geometry without exposing an uncharged GPU handle.
    /// Takes no arguments. Returns width and height in pixels.
    pub fn dimensions(&self) -> [u32; 2] {
        [self.texture.width(), self.texture.height()]
    }
    /// Inspect the explicit image precision and encoding.
    /// Takes no arguments. Returns storage precision, primaries and transfer;
    /// every image is premultiplied in its declared encoding.
    pub fn interpretation(&self) -> (FloatPrecision, WorkingGamut, OutputTransfer) {
        (self.precision, self.gamut, self.transfer)
    }
}

/// Revision and worker-owned receipt over a reusable resident GPU picture.
pub struct RenderedFrame {
    image: Arc<ResidentImage>,
    version: DocumentVersion,
    worker: Uuid,
    generation: u64,
}
impl RenderedFrame {
    /// Inspect publication ownership without permitting forged receipt fields.
    /// Takes no arguments. Returns the captured document version.
    pub fn version(&self) -> DocumentVersion {
        self.version
    }
    /// Retain immutable GPU content while keeping its allocation charged.
    /// Takes no arguments. Returns the shared image without mutable GPU handles.
    pub fn image(&self) -> &Arc<ResidentImage> {
        &self.image
    }
}

/// Explicit destination encoding applied after working-picture evaluation.
#[derive(Debug, Clone, Copy, Serialize)]
pub enum ImageBoundary {
    Display,
    Output,
}

/// Actual graph cache, submitted GPU work and raw-picture residency.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct GraphStats {
    pub cache_bytes: usize,
    pub live_texture_bytes: usize,
    pub entries: usize,
    pub pending_submissions: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub uploads: u64,
    pub dispatches: u64,
    pub readbacks: u64,
    pub pictures: PictureCacheStats,
}
struct Entry {
    image: Arc<ResidentImage>,
    used: u64,
}
#[derive(Clone)]
enum Value {
    Image(Arc<ResidentImage>),
    Scalar(f32, String),
}
impl Value {
    fn key(&self) -> &str {
        match self {
            Self::Image(v) => &v.key,
            Self::Scalar(_, k) => k,
        }
    }
    fn image(&self) -> Result<Arc<ResidentImage>> {
        match self {
            Self::Image(v) => Ok(v.clone()),
            _ => Err("graph input is not an image".into()),
        }
    }
}

/// Single owning worker for typed SDR picture graphs and resident float caches.
pub struct GraphRenderer<P: PictureProvider = PictureCache> {
    snapshot: Arc<EvaluationSnapshot>,
    pictures: P,
    cancel: Cancellation,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: [wgpu::ComputePipeline; 2],
    budget: GraphBudget,
    entries: HashMap<String, Entry>,
    bytes: usize,
    live: Arc<AtomicUsize>,
    pending: Arc<AtomicUsize>,
    serial: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
    uploads: u64,
    dispatches: u64,
    readbacks: u64,
    worker: Uuid,
    generation: u64,
    pub adapter: wgpu::AdapterInfo,
}
impl GraphRenderer {
    /// Create a headless native GPU worker over one immutable document.
    /// `snapshot`, `pictures` and `budget` declare document/resource ownership;
    /// `cancel` interrupts decode and bounded GPU waits. Returns compiled FP16/
    /// FP32 kernels; device creation and all evaluation belong off the UI thread.
    pub fn new(
        snapshot: Arc<EvaluationSnapshot>,
        pictures: PictureBudget,
        budget: GraphBudget,
        cancel: Cancellation,
    ) -> Result<Self> {
        let provider = PictureCache::new(snapshot.clone(), pictures, cancel.clone())?;
        Self::with_provider(snapshot, provider, budget, cancel)
    }
}
impl<P: PictureProvider> GraphRenderer<P> {
    /// Create the shared GPU graph using a declared source-owned picture route.
    /// `snapshot`, `provider`, `budget` and `cancel` capture document and resource
    /// ownership. Returns compiled kernels; isolated and in-process sources use
    /// identical temporal, color, cache and evaluation semantics.
    pub fn with_provider(
        snapshot: Arc<EvaluationSnapshot>,
        provider: P,
        budget: GraphBudget,
        cancel: Cancellation,
    ) -> Result<Self> {
        budget.validate()?;
        if provider.version() != DocumentVersion::of(snapshot.project()) {
            return Err("picture provider does not own this graph's document version".into());
        }
        if cancel.is_cancelled() {
            return Err(editbay_media::Error::Cancelled.into());
        }
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
        for format in [
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureFormat::Rgba32Float,
        ] {
            let required = wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC;
            if !adapter
                .get_texture_format_features(format)
                .allowed_usages
                .contains(required)
            {
                return Err("adapter cannot store/sample declared float picture formats".into());
            }
        }
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
        let pipeline = |precision| {
            let entries = [
                texture_binding(0),
                texture_binding(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: format(precision),
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ];
            let bindings = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(KERNEL),
                entries: &entries,
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(KERNEL),
                bind_group_layouts: &[Some(&bindings)],
                immediate_size: 0,
            });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(KERNEL),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("graph.wgsl")
                        .replace(
                            "OUTPUT_FORMAT",
                            if precision == FloatPrecision::Half {
                                "rgba16float"
                            } else {
                                "rgba32float"
                            },
                        )
                        .into(),
                ),
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(KERNEL),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("evaluate"),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pipelines = [
            pipeline(FloatPrecision::Half),
            pipeline(FloatPrecision::Full),
        ];
        Ok(Self {
            pictures: provider,
            snapshot,
            cancel,
            device,
            queue,
            pipelines,
            budget,
            entries: HashMap::new(),
            bytes: 0,
            live: Arc::new(AtomicUsize::new(0)),
            pending: Arc::new(AtomicUsize::new(0)),
            serial: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
            uploads: 0,
            dispatches: 0,
            readbacks: 0,
            worker: Uuid::new_v4(),
            generation: 0,
            adapter: info,
        })
    }

    /// Evaluate one exact picture through the shared typed graph.
    /// `composition`, `position` and `before` select temporal ownership. Returns
    /// a premultiplied linear resident image without CPU readback; unsupported
    /// active image/mask/color operations fail, and audio is evaluated separately.
    pub fn render(
        &mut self,
        composition: Uuid,
        position: SourcePosition,
        before: bool,
    ) -> Result<RenderedFrame> {
        self.check()?;
        self.device.poll(wgpu::PollType::Poll)?;
        let mut remaining = self.budget.nodes_per_request;
        let image = self.scene(composition, position, before, 0, &mut remaining)?;
        self.check()?;
        Ok(self.receipt(image))
    }

    fn scene(
        &mut self,
        composition: Uuid,
        position: SourcePosition,
        before: bool,
        depth: usize,
        remaining: &mut usize,
    ) -> Result<Arc<ResidentImage>> {
        if depth >= self.budget.nesting_depth {
            return Err("composition nesting exceeds the GPU worker budget".into());
        }
        let frame = self.snapshot.prepare(composition, position, before)?;
        let by_id: HashMap<_, _> = frame.nodes.iter().map(|n| (n.id, n)).collect();
        let mut reachable = HashSet::new();
        let mut todo: Vec<_> = frame.picture.into_iter().collect();
        while let Some(id) = todo.pop() {
            if reachable.insert(id) {
                todo.extend(inputs(&by_id[&id].operation)?);
            }
        }
        if reachable.len() > *remaining {
            return Err("picture graph exceeds the worker's node budget".into());
        }
        *remaining -= reachable.len();
        let mut uses: HashMap<Uuid, usize> = HashMap::new();
        for node in frame.nodes.iter().filter(|n| reachable.contains(&n.id)) {
            for input in inputs(&node.operation)? {
                *uses.entry(input).or_default() += 1;
            }
        }
        let mut values: HashMap<Uuid, Value> = HashMap::new();
        for node in frame.nodes.iter().filter(|n| reachable.contains(&n.id)) {
            self.check()?;
            let dependency_ids = inputs(&node.operation)?;
            let dependencies: Vec<_> = dependency_ids
                .iter()
                .map(|id| values.get(id).ok_or("picture dependency is absent"))
                .collect::<std::result::Result<_, _>>()?;
            let keys: Vec<_> = dependencies.iter().map(|v| v.key()).collect();
            let mut key = fingerprint(&(KERNEL, &node.sha256, keys))?;
            let value = if node.socket == SocketType::Data {
                let value = match &*node.operation {
                    NodeOperation::Scalar { value } if node.active => scalar(*value)?,
                    NodeOperation::Scalar { .. } => 0.,
                    _ => return Err("unsupported picture data operation".into()),
                };
                Value::Scalar(value, key)
            } else if !node.active {
                Value::Image(self.blank(&frame)?)
            } else {
                let mut p = Parameters::default();
                let mut images = Vec::new();
                match &*node.operation {
                    NodeOperation::Source { .. } => {
                        match node.source.as_ref() {
                            Some(request @ SourceRequest::Media { source, stream, .. }) => {
                                let snapshot = self.snapshot.clone();
                                let (_, profile, _) = snapshot.source_stream(*source, *stream)?;
                                let StreamFormat::Video {
                                    color,
                                    alpha,
                                    sample_aspect,
                                    ..
                                } = profile.format
                                else {
                                    return Err("picture source selected sound".into());
                                };
                                let rotation = profile
                                    .metadata
                                    .get("editbay.display_rotation_degrees")
                                    .map(|v| v.parse::<f64>())
                                    .transpose()?;
                                if rotation.is_some_and(|v| !v.is_finite() || v != 0.) {
                                    return Err(
                                        "source display rotation requires the orientation renderer"
                                            .into(),
                                    );
                                }
                                if ![0, 1, 4, 5, 6, 7, 9].contains(&color.matrix)
                                    || ![1, 2].contains(&color.range)
                                {
                                    return Err("source matrix/range needs a supported native interpretation".into());
                                }
                                let gamut = match color.primaries {
                                    1 => WorkingGamut::Bt709,
                                    9 => WorkingGamut::Bt2020,
                                    12 => WorkingGamut::DisplayP3,
                                    _ => return Err(
                                        "source primaries need a supported explicit interpretation"
                                            .into(),
                                    ),
                                };
                                p.transfer = match color.transfer { 8 => OutputTransfer::Linear, 13 => OutputTransfer::Srgb, 1 => OutputTransfer::Bt709, _ => return Err("source transfer requires the HDR/log renderer or an explicit interpretation".into()) };
                                let decoded = self.pictures.picture(request)?;
                                if let Some(decoded) = decoded {
                                    self.pictures.validate_result(&decoded)?;
                                    if decoded.picture.color != color
                                        || decoded.picture.alpha != alpha
                                        || decoded.picture.alpha_interpretation_required
                                        || decoded.picture.rotation_degrees != 0.
                                    {
                                        return Err("source color/alpha override requires a native interpretation adapter".into());
                                    }
                                    key = fingerprint(&(
                                        KERNEL,
                                        &node.sha256,
                                        color,
                                        alpha,
                                        sample_aspect,
                                    ))?;
                                    if let Some(hit) = self.cached(&key) {
                                        values.insert(node.id, Value::Image(hit));
                                        continue;
                                    }
                                    p.operation = Kernel::Source;
                                    p.alpha = alpha;
                                    let upload = self.allocate(
                                        decoded.picture.width,
                                        decoded.picture.height,
                                        frame.color.precision,
                                        gamut,
                                        OutputTransfer::Linear,
                                        wgpu::TextureFormat::Rgba8Unorm,
                                        "native RGBA8 upload".into(),
                                    )?;
                                    if self.pending.load(Ordering::Acquire)
                                        >= self.budget.pending_submissions
                                    {
                                        self.wait(true)?;
                                    }
                                    self.queue.write_texture(
                                        upload.texture.as_image_copy(),
                                        decoded.picture.rgba(),
                                        wgpu::TexelCopyBufferLayout {
                                            offset: 0,
                                            bytes_per_row: Some(decoded.picture.width * 4),
                                            rows_per_image: Some(decoded.picture.height),
                                        },
                                        upload.texture.size(),
                                    );
                                    let commands =
                                        self.device.create_command_encoder(&Default::default());
                                    self.submit(commands, vec![upload.clone()])?;
                                    self.uploads = self.uploads.saturating_add(1);
                                    let sar = f64::from(sample_aspect.numerator)
                                        / f64::from(sample_aspect.denominator);
                                    fit(
                                        &mut p,
                                        upload.dimensions(),
                                        [frame.width, frame.height],
                                        sar,
                                    )?;
                                    set_gamut(&mut p, gamut, frame.color.working_gamut);
                                    images.push(upload);
                                } else {
                                    values.insert(node.id, Value::Image(self.blank(&frame)?));
                                    continue;
                                }
                            }
                            Some(SourceRequest::Composition {
                                composition,
                                position,
                                reverse,
                            }) => {
                                let child = self.scene(
                                    *composition,
                                    *position,
                                    *reverse,
                                    depth + 1,
                                    remaining,
                                )?;
                                key = fingerprint(&(KERNEL, &node.sha256, &child.key))?;
                                p.operation = Kernel::Nested;
                                fit(&mut p, child.dimensions(), [frame.width, frame.height], 1.)?;
                                set_gamut(&mut p, child.gamut, frame.color.working_gamut);
                                images.push(child);
                            }
                            None => {
                                values.insert(node.id, Value::Image(self.blank(&frame)?));
                                continue;
                            }
                        }
                    }
                    NodeOperation::Solid { rgba } => {
                        for (target, value) in p.solid.iter_mut().zip(rgba) {
                            if value.abs() > 4096. {
                                return Err(
                                    "solid exceeds the declared finite SDR working range".into()
                                );
                            }
                            *target = scalar(*value)?;
                        }
                    }
                    NodeOperation::Transform {
                        image,
                        translation,
                        scale,
                        rotation,
                        opacity,
                    } => {
                        images.push(values[image].image()?);
                        p.operation = Kernel::Transform;
                        p.opacity = scalar(*opacity)?;
                        affine(
                            &mut p,
                            [frame.width, frame.height],
                            *translation,
                            *scale,
                            *rotation,
                        )?;
                    }
                    NodeOperation::Over {
                        foreground,
                        background,
                        mask: None,
                    } => {
                        p.operation = Kernel::Over;
                        images.push(values[foreground].image()?);
                        images.push(values[background].image()?);
                    }
                    NodeOperation::Opacity { image, value } => {
                        p.operation = Kernel::Opacity;
                        images.push(values[image].image()?);
                        p.opacity = match values[value] {
                            Value::Scalar(value, _) => value.clamp(0., 1.),
                            _ => return Err("opacity input is not scalar data".into()),
                        };
                    }
                    _ => return Err("unsupported picture operation".into()),
                }
                let image = if let Some(hit) = self.cached(&key) {
                    hit
                } else {
                    self.evaluate(
                        key,
                        frame.width,
                        frame.height,
                        frame.color.precision,
                        frame.color.working_gamut,
                        OutputTransfer::Linear,
                        &p,
                        images,
                    )?
                };
                Value::Image(image)
            };
            values.insert(node.id, value);
            for id in dependency_ids {
                let count = uses.get_mut(&id).ok_or("dependency use count missing")?;
                *count -= 1;
                if *count == 0 {
                    values.remove(&id);
                }
            }
        }
        match frame.picture {
            Some(root) => values
                .remove(&root)
                .ok_or("picture root is absent")?
                .image(),
            None => self.blank(&frame),
        }
    }

    /// Convert a working receipt into its explicit display or output encoding.
    /// `frame` must belong to this worker/version; `boundary` selects captured
    /// color settings. Returns another resident premultiplied float image; PQ/
    /// HLG fail until absolute luminance/tone-mapping contracts are implemented.
    pub fn convert(
        &mut self,
        frame: &RenderedFrame,
        boundary: ImageBoundary,
    ) -> Result<RenderedFrame> {
        self.validate_result(frame)?;
        if frame.image.transfer != OutputTransfer::Linear {
            return Err("boundary conversion requires a linear working picture".into());
        }
        let color = self.snapshot.project().color;
        let (gamut, transfer) = match boundary {
            ImageBoundary::Display => (color.display_gamut, color.display_transfer),
            ImageBoundary::Output => (color.output_gamut, color.output_transfer),
        };
        let mut p = Parameters {
            operation: Kernel::Boundary,
            transfer,
            ..Parameters::default()
        };
        transfer_code(transfer)?;
        set_gamut(&mut p, frame.image.gamut, gamut);
        let key = fingerprint(&(KERNEL, "boundary", &frame.image.key, gamut, transfer))?;
        let image = if let Some(hit) = self.cached(&key) {
            hit
        } else {
            let [width, height] = frame.image.dimensions();
            self.evaluate(
                key,
                width,
                height,
                frame.image.precision,
                gamut,
                transfer,
                &p,
                vec![frame.image.clone()],
            )?
        };
        self.check()?;
        Ok(self.receipt(image))
    }

    /// Read a resident picture at an explicit inspection/export boundary.
    /// `frame` must belong to this worker/version. Returns premultiplied float
    /// pixels in its declared encoding; source integrity is checked separately.
    pub fn readback(&mut self, frame: &RenderedFrame) -> Result<Vec<f32>> {
        self.validate_result(frame)?;
        let image = &frame.image;
        let [width, height] = image.dimensions();
        let pixel_bytes = pixel_bytes(format(image.precision));
        let row_bytes = (width * pixel_bytes).div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("explicit graph readback"),
            size: u64::from(row_bytes) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut commands = self.device.create_command_encoder(&Default::default());
        commands.copy_texture_to_buffer(
            image.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(height),
                },
            },
            image.texture.size(),
        );
        self.submit(commands, vec![image.clone()])?;
        let (tx, rx) = mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.wait(true)?;
        rx.recv_timeout(Duration::from_secs(2))??;
        let mapped = readback.slice(..).get_mapped_range()?;
        let mut output = Vec::with_capacity(width as usize * height as usize * 4);
        for row in mapped.chunks_exact(row_bytes as usize) {
            let row = &row[..(width * pixel_bytes) as usize];
            match image.precision {
                FloatPrecision::Half => output.extend(
                    row.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|v| f16::from_le_bytes(*v).to_f32()),
                ),
                FloatPrecision::Full => output.extend(
                    row.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|v| f32::from_le_bytes(*v)),
                ),
            }
        }
        drop(mapped);
        readback.unmap();
        self.readbacks = self.readbacks.saturating_add(1);
        self.check()?;
        if output.iter().any(|v| !v.is_finite()) {
            return Err("rendered picture contains nonfinite channels".into());
        }
        Ok(output)
    }

    /// Reject cancelled, foreign-worker or stale revision/generation receipts.
    /// `frame` supplies captured ownership. Returns success for this worker;
    /// receiving UI session/job ownership and final source verification remain required.
    pub fn validate_result(&self, frame: &RenderedFrame) -> Result<()> {
        self.check()?;
        if frame.worker != self.worker
            || frame.version != DocumentVersion::of(self.snapshot.project())
            || frame.generation != self.generation
        {
            return Err("render receipt belongs to a stale or different worker".into());
        }
        Ok(())
    }
    /// Verify all sources used in the current generation before publication.
    /// Takes no arguments. Returns success after full file checksum checks.
    pub fn verify_sources(&mut self) -> Result<()> {
        self.pictures.verify_sources()?;
        Ok(())
    }
    /// Wait for submitted work and inspect actual residency counters.
    /// Takes no arguments. Returns once GPU completion releases in-flight pins.
    pub fn finish(&self) -> Result<()> {
        self.wait(true)
    }
    /// Inspect charged cache, live textures and decode/output activity.
    /// Takes no arguments. Returns counters without waiting or readback.
    pub fn stats(&self) -> GraphStats {
        GraphStats {
            cache_bytes: self.bytes,
            live_texture_bytes: self.live.load(Ordering::Acquire),
            entries: self.entries.len(),
            pending_submissions: self.pending.load(Ordering::Acquire),
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
            uploads: self.uploads,
            dispatches: self.dispatches,
            readbacks: self.readbacks,
            pictures: self.pictures.stats(),
        }
    }
    /// Inspect provider-specific transport and ownership counters.
    /// Takes no arguments. Returns a shared provider borrow without GPU handles.
    pub fn picture_provider(&self) -> &P {
        &self.pictures
    }
    /// Release owned caches while externally held pictures remain charged.
    /// Takes no arguments. Returns after bounded GPU completion; previous receipts
    /// become stale. This cleanup remains usable after cancellation.
    pub fn clear(&mut self) -> Result<()> {
        let next = self
            .generation
            .checked_add(1)
            .ok_or("render generation exhausted")?;
        self.wait(false)?;
        self.entries.clear();
        self.bytes = 0;
        self.pictures.clear()?;
        self.generation = next;
        Ok(())
    }
    /// Rebind reusable content to a new captured revision and cancellation token.
    /// `snapshot` is validated immutable work and `cancel` must be fresh. Returns
    /// after old GPU work completes; old receipts are invalid, matching content reusable.
    pub fn rebind(
        &mut self,
        snapshot: Arc<EvaluationSnapshot>,
        cancel: Cancellation,
    ) -> Result<()> {
        let next = self
            .generation
            .checked_add(1)
            .ok_or("render generation exhausted")?;
        self.wait(false)?;
        self.pictures.rebind(snapshot.clone(), cancel.clone())?;
        self.snapshot = snapshot;
        self.cancel = cancel;
        self.generation = next;
        Ok(())
    }

    fn receipt(&self, image: Arc<ResidentImage>) -> RenderedFrame {
        RenderedFrame {
            image,
            version: DocumentVersion::of(self.snapshot.project()),
            worker: self.worker,
            generation: self.generation,
        }
    }
    fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(editbay_media::Error::Cancelled.into())
        } else {
            Ok(())
        }
    }
    fn wait(&self, cancellable: bool) -> Result<()> {
        let started = Instant::now();
        loop {
            if cancellable {
                self.check()?;
            }
            match self.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_millis(20)),
            }) {
                Ok(_) => return Ok(()),
                Err(wgpu::PollError::Timeout) if started.elapsed() < Duration::from_secs(2) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn evict(&mut self) -> bool {
        let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.used)
            .map(|(k, _)| k.clone())
        else {
            return false;
        };
        if let Some(entry) = self.entries.remove(&key) {
            self.bytes -= entry.image._allocation.bytes;
            self.evictions = self.evictions.saturating_add(1);
        }
        true
    }
    fn cached(&mut self, key: &str) -> Option<Arc<ResidentImage>> {
        self.serial = self.serial.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(key) {
            entry.used = self.serial;
            self.hits = self.hits.saturating_add(1);
            Some(entry.image.clone())
        } else {
            self.misses = self.misses.saturating_add(1);
            None
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn allocate(
        &mut self,
        width: u32,
        height: u32,
        precision: FloatPrecision,
        gamut: WorkingGamut,
        transfer: OutputTransfer,
        format: wgpu::TextureFormat,
        key: String,
    ) -> Result<Arc<ResidentImage>> {
        let bytes = width as usize * height as usize * pixel_bytes(format) as usize;
        if width == 0
            || height == 0
            || width > 8192
            || height > 8192
            || width > self.device.limits().max_texture_dimension_2d
            || height > self.device.limits().max_texture_dimension_2d
            || bytes > self.budget.maximum_texture_bytes
        {
            return Err("picture exceeds GPU geometry/texture budget".into());
        }
        self.device.poll(wgpu::PollType::Poll)?;
        while self.live.load(Ordering::Acquire).saturating_add(bytes) > self.budget.live_bytes
            && self.evict()
        {}
        if self.live.load(Ordering::Acquire).saturating_add(bytes) > self.budget.live_bytes
            && self.pending.load(Ordering::Acquire) > 0
        {
            self.wait(true)?;
        }
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes)
                    .filter(|next| *next <= self.budget.live_bytes)
            })
            .map_err(|_| "GPU live texture budget exhausted by retained outputs")?;
        let allocation = Allocation {
            bytes,
            live: self.live.clone(),
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(KERNEL),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | if format == wgpu::TextureFormat::Rgba8Unorm {
                    wgpu::TextureUsages::empty()
                } else {
                    wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC
                },
            view_formats: &[],
        });
        Ok(Arc::new(ResidentImage {
            texture,
            key,
            precision,
            gamut,
            transfer,
            _allocation: allocation,
        }))
    }
    fn blank(&mut self, frame: &PreparedFrame) -> Result<Arc<ResidentImage>> {
        let key = fingerprint(&(
            KERNEL,
            "transparent",
            frame.width,
            frame.height,
            frame.color.working_gamut,
            frame.color.precision,
        ))?;
        if let Some(hit) = self.cached(&key) {
            return Ok(hit);
        }
        self.evaluate(
            key,
            frame.width,
            frame.height,
            frame.color.precision,
            frame.color.working_gamut,
            OutputTransfer::Linear,
            &Parameters::default(),
            vec![],
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn evaluate(
        &mut self,
        key: String,
        width: u32,
        height: u32,
        precision: FloatPrecision,
        gamut: WorkingGamut,
        transfer: OutputTransfer,
        p: &Parameters,
        mut images: Vec<Arc<ResidentImage>>,
    ) -> Result<Arc<ResidentImage>> {
        self.check()?;
        if images.is_empty() {
            let dummy = self.allocate(
                1,
                1,
                precision,
                gamut,
                OutputTransfer::Linear,
                format(precision),
                "bound neutral input".into(),
            )?;
            images.push(dummy);
        }
        let output = self.allocate(
            width,
            height,
            precision,
            gamut,
            transfer,
            format(precision),
            key.clone(),
        )?;
        let bytes = p.bytes()?;
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(KERNEL),
                contents: &bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let pipeline = &self.pipelines[if precision == FloatPrecision::Half {
            0
        } else {
            1
        }];
        let first = images[0].texture.create_view(&Default::default());
        let second = images
            .get(1)
            .unwrap_or(&images[0])
            .texture
            .create_view(&Default::default());
        let destination = output.texture.create_view(&Default::default());
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(KERNEL),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&first),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&second),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&destination),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let mut commands = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = commands.begin_compute_pass(&Default::default());
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
        }
        images.push(output.clone());
        self.submit(commands, images)?;
        self.dispatches = self.dispatches.saturating_add(1);
        if output._allocation.bytes <= self.budget.cache_bytes && self.budget.cache_entries > 0 {
            while (self.bytes.saturating_add(output._allocation.bytes) > self.budget.cache_bytes
                || self.entries.len() >= self.budget.cache_entries)
                && self.evict()
            {}
            self.serial = self.serial.saturating_add(1);
            self.bytes += output._allocation.bytes;
            self.entries.insert(
                key,
                Entry {
                    image: output.clone(),
                    used: self.serial,
                },
            );
        }
        Ok(output)
    }
    fn submit(
        &mut self,
        commands: wgpu::CommandEncoder,
        pins: Vec<Arc<ResidentImage>>,
    ) -> Result<()> {
        if self.pending.load(Ordering::Acquire) >= self.budget.pending_submissions {
            self.wait(true)?;
        }
        self.pending.fetch_add(1, Ordering::AcqRel);
        self.queue.submit([commands.finish()]);
        let pending = self.pending.clone();
        self.queue.on_submitted_work_done(move || {
            drop(pins);
            pending.fetch_sub(1, Ordering::AcqRel);
        });
        Ok(())
    }
}

impl<P: PictureProvider> Drop for GraphRenderer<P> {
    fn drop(&mut self) {
        self.cancel.cancel();
        let _ = self.wait(false);
    }
}

fn texture_binding(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
fn format(precision: FloatPrecision) -> wgpu::TextureFormat {
    match precision {
        FloatPrecision::Half => wgpu::TextureFormat::Rgba16Float,
        FloatPrecision::Full => wgpu::TextureFormat::Rgba32Float,
    }
}
fn pixel_bytes(format: wgpu::TextureFormat) -> u32 {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => 4,
        wgpu::TextureFormat::Rgba16Float => 8,
        _ => 16,
    }
}
fn fingerprint(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn scalar(value: f64) -> Result<f32> {
    let result = value as f32;
    if result.is_finite() {
        Ok(result)
    } else {
        Err("graph parameter exceeds float precision".into())
    }
}
fn inputs(op: &NodeOperation) -> Result<Vec<Uuid>> {
    Ok(match op {
        NodeOperation::Source { .. }
        | NodeOperation::Solid { .. }
        | NodeOperation::Scalar { .. } => vec![],
        NodeOperation::Transform { image, .. } => vec![*image],
        NodeOperation::Over {
            foreground,
            background,
            mask: None,
        } => vec![*foreground, *background],
        NodeOperation::Opacity { image, value } => vec![*image, *value],
        _ => {
            return Err(
                "mask/geometry/audio operation is not supported by the SDR picture worker".into(),
            );
        }
    })
}
#[derive(Clone, Copy)]
#[repr(u8)]
enum Kernel {
    Solid = 0,
    Source = 1,
    Transform = 2,
    Over = 3,
    Opacity = 4,
    Nested = 5,
    Boundary = 6,
}
struct Parameters {
    operation: Kernel,
    transfer: OutputTransfer,
    alpha: AlphaMode,
    opacity: f32,
    inverse_x: [f32; 4],
    inverse_y: [f32; 4],
    solid: [f32; 4],
    gamut: [[f32; 4]; 3],
}
impl Default for Parameters {
    fn default() -> Self {
        Self {
            operation: Kernel::Solid,
            transfer: OutputTransfer::Linear,
            alpha: AlphaMode::Opaque,
            opacity: 1.,
            inverse_x: [1., 0., 0., 0.],
            inverse_y: [0., 1., 0., 0.],
            solid: [0.; 4],
            gamut: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
        }
    }
}
impl Parameters {
    fn bytes(&self) -> Result<Vec<u8>> {
        let mode = [
            self.operation as u8 as f32,
            transfer_code(self.transfer)?,
            match self.alpha {
                AlphaMode::Opaque => 0.,
                AlphaMode::Straight => 1.,
                AlphaMode::Premultiplied => 2.,
            },
            self.opacity,
        ];
        Ok([
            mode,
            self.inverse_x,
            self.inverse_y,
            self.solid,
            self.gamut[0],
            self.gamut[1],
            self.gamut[2],
        ]
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect())
    }
}
fn transfer_code(transfer: OutputTransfer) -> Result<f32> {
    match transfer {
        OutputTransfer::Linear => Ok(0.),
        OutputTransfer::Srgb => Ok(1.),
        OutputTransfer::Bt709 => Ok(2.),
        _ => Err("HDR display/output requires absolute luminance and tone-mapping support".into()),
    }
}
fn fit(p: &mut Parameters, source: [u32; 2], destination: [u32; 2], sar: f64) -> Result<()> {
    let scale = (f64::from(destination[0]) / (f64::from(source[0]) * sar))
        .min(f64::from(destination[1]) / f64::from(source[1]));
    let x = 1. / (scale * sar);
    let y = 1. / scale;
    p.inverse_x = [
        scalar(x)?,
        0.,
        scalar(f64::from(source[0]) * 0.5 - f64::from(destination[0]) * 0.5 * x)?,
        0.,
    ];
    p.inverse_y = [
        0.,
        scalar(y)?,
        scalar(f64::from(source[1]) * 0.5 - f64::from(destination[1]) * 0.5 * y)?,
        0.,
    ];
    projection(p, destination)
}
fn affine(
    p: &mut Parameters,
    size: [u32; 2],
    translation: [f64; 2],
    scale: [f64; 2],
    rotation: f64,
) -> Result<()> {
    if scale.contains(&0.) {
        p.opacity = 0.;
        return Ok(());
    }
    let (sin, cos) = rotation.to_radians().sin_cos();
    let center = [f64::from(size[0]) * 0.5, f64::from(size[1]) * 0.5];
    let x = [cos / scale[0], sin / scale[0]];
    let y = [-sin / scale[1], cos / scale[1]];
    p.inverse_x = [
        scalar(x[0])?,
        scalar(x[1])?,
        scalar(
            center[0] - x[0] * (center[0] + translation[0]) - x[1] * (center[1] + translation[1]),
        )?,
        0.,
    ];
    p.inverse_y = [
        scalar(y[0])?,
        scalar(y[1])?,
        scalar(
            center[1] - y[0] * (center[0] + translation[0]) - y[1] * (center[1] + translation[1]),
        )?,
        0.,
    ];
    projection(p, size)
}
fn projection(p: &Parameters, size: [u32; 2]) -> Result<()> {
    let extent = [f64::from(size[0]), f64::from(size[1]), 1.];
    for row in [p.inverse_x, p.inverse_y] {
        let maximum: f64 = row
            .iter()
            .zip(extent)
            .map(|(value, extent)| f64::from(value.abs()) * extent)
            .sum();
        if maximum > f64::from(f32::MAX) * 0.5 {
            return Err("inverse transform exceeds finite GPU coordinates".into());
        }
    }
    Ok(())
}
fn set_gamut(p: &mut Parameters, source: WorkingGamut, destination: WorkingGamut) {
    if source == destination {
        return;
    }
    let a = xyz(source);
    let b = inverse(xyz(destination));
    for (row, source) in p.gamut.iter_mut().zip(b) {
        for (j, target) in row.iter_mut().enumerate().take(3) {
            *target = source
                .iter()
                .zip(a)
                .map(|(factor, xyz)| factor * xyz[j])
                .sum::<f64>() as f32;
        }
    }
}
fn xyz(gamut: WorkingGamut) -> [[f64; 3]; 3] {
    match gamut {
        WorkingGamut::Bt709 => [
            [0.4123907993, 0.3575843394, 0.1804807884],
            [0.2126390059, 0.7151686788, 0.0721923154],
            [0.0193308187, 0.1191947798, 0.9505321522],
        ],
        WorkingGamut::Bt2020 => [
            [0.6369580483, 0.1446169036, 0.1688809752],
            [0.2627002120, 0.6779980715, 0.0593017165],
            [0., 0.0280726930, 1.0609850577],
        ],
        WorkingGamut::DisplayP3 => [
            [0.4865709486, 0.2656676932, 0.1982172852],
            [0.2289745641, 0.6917385218, 0.0792869141],
            [0., 0.0451133819, 1.0439443689],
        ],
    }
}
fn inverse(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let determinant = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let r = [(j + 1) % 3, (j + 2) % 3];
            let c = [(i + 1) % 3, (i + 2) % 3];
            (m[r[0]][c[0]] * m[r[1]][c[1]] - m[r[0]][c[1]] * m[r[1]][c[0]]) / determinant
        })
    })
}
