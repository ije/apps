use crate::Size;
use crate::atlas::{self, Packer, PageKind, PageList, PageStore, Tile};
use crate::frame::{Command, RenderFrame, Target};
use crate::gpu::{ATTRIBUTES, Blur, Composite, Globals, Instance, Plain, bytes};
use crate::scene::Scene;
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use raw_window_handle::{DisplayHandle, HandleError, HasDisplayHandle};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::{cell::RefCell, mem::size_of, num::NonZeroU64, rc::Rc, sync::Arc};
use wgpu::{BindGroup, BindGroupLayout, BindGroupLayoutEntry, BindingResource, BindingType};
use wgpu::{Buffer, BufferUsages, Color, CurrentSurfaceTexture as Next, Device, Queue};
use wgpu::{Extent3d, Origin3d, RenderPipeline, ShaderStages, Surface, SurfaceConfiguration};
use wgpu::{Texture, TextureFormat, TextureUsages, TextureView};

// Glyph masks blend in sRGB here, which makes dark text look too heavy. Needs a visual check on a real display.
const TEXT_CONTRAST: f32 = 0.3;
const MAX_INSTANCE_BYTES: u64 = 256 << 20;
// Layer textures that no frame used for this many frames are dropped.
const IDLE_FRAMES: u8 = 3;

pub(crate) type Atlas = atlas::Atlas<Pages>;
pub(super) type Capture = Option<Result<([u32; 2], Vec<u8>)>>;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Drawn {
    Shown,
    Again,
    Skipped,
}

// wgpu needs the display connection for GL on Wayland.
#[derive(Debug)]
struct Connection(RawDisplayHandle);

// SAFETY: the connection outlives the GPU, and wgpu only reads the handle.
unsafe impl Send for Connection {}
// SAFETY: as above.
unsafe impl Sync for Connection {}

impl HasDisplayHandle for Connection {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: see `Send`.
        Ok(unsafe { DisplayHandle::borrow_raw(self.0) })
    }
}

struct Pipelines {
    draw: RenderPipeline,
    composite: RenderPipeline,
    blur: RenderPipeline,
}

// Made again after the device is lost.
struct Gpu {
    api: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: Device,
    queue: Queue,
    lost: Arc<AtomicBool>,
    // Color atlas pages hold BGRA, swizzled to RGBA where the adapter can't sample BGRA.
    color: TextureFormat,
    module: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    groups: [BindGroupLayout; 3],
    blank: TextureView,
    pipelines: RefCell<Vec<(TextureFormat, Rc<Pipelines>)>>,
}

thread_local! {
    static GPU: RefCell<Option<Rc<Gpu>>> = const { RefCell::new(None) };
}

fn texture(
    device: &Device,
    side: [u32; 2],
    format: TextureFormat,
    usage: TextureUsages,
) -> Texture {
    let [width, height] = side;
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: Extent3d {
            width,
            height,
            ..Default::default()
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn extent(width: i32, height: i32) -> Extent3d {
    Extent3d {
        width: width as u32,
        height: height as u32,
        ..Default::default()
    }
}

fn slot(binding: u32, size: Option<usize>) -> BindGroupLayoutEntry {
    let ty = match size {
        Some(size) => BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: NonZeroU64::new(size as u64),
        },
        None => BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
    };
    let visibility = ShaderStages::VERTEX_FRAGMENT;
    BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}

impl Gpu {
    // A new device must be able to draw to this window.
    fn surface(
        display: RawDisplayHandle,
        window: RawWindowHandle,
    ) -> Result<(Rc<Self>, Surface<'static>)> {
        let kept = GPU
            .with_borrow(Clone::clone)
            .filter(|gpu| !gpu.lost.load(Relaxed));
        let api = match &kept {
            Some(gpu) => gpu.api.clone(),
            None => {
                let connection = Box::new(Connection(display));
                let mut options = wgpu::InstanceDescriptor::new_with_display_handle(connection);
                options.backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
                wgpu::Instance::new(options)
            }
        };
        let raw_window_handle = window;
        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: None,
            raw_window_handle,
        };
        // SAFETY: the window owns the renderer, which drops the surface first.
        let surface = unsafe { api.create_surface_unsafe(target) }
            .map_err(|error| anyhow!("making the window surface: {error}"))?;
        if let Some(gpu) = kept {
            let formats = surface.get_capabilities(&gpu.adapter).formats;
            ensure!(!formats.is_empty(), "the GPU can't draw to this window");
            return Ok((gpu, surface));
        }
        // Integrated GPUs first, as they save power, and Vulkan before GL.
        let rank = |info: wgpu::AdapterInfo| {
            use wgpu::DeviceType::*;
            let order = [IntegratedGpu, DiscreteGpu, Other];
            let kind = order
                .iter()
                .position(|kind| *kind == info.device_type)
                .unwrap_or(3);
            (kind, info.backend != wgpu::Backend::Vulkan)
        };
        let mut found = pollster::block_on(api.enumerate_adapters(wgpu::Backends::all()));
        found.sort_by_cached_key(|candidate| rank(candidate.get_info()));
        let mut refusals = Vec::new();
        for adapter in found {
            let name = adapter.get_info().name;
            let Some(probe) = surface.get_default_config(&adapter, 64, 64) else {
                continue;
            };
            // GL ES 3 has no storage buffers, so the limits stay at what WebGL 2 has.
            let limits = adapter.limits();
            let wanted = wgpu::DeviceDescriptor {
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(limits.clone())
                    .using_alignment(limits),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            };
            let (device, queue) = match pollster::block_on(adapter.request_device(&wanted)) {
                Ok(pair) => pair,
                Err(error) => {
                    refusals.push(format!("{name}: {error}"));
                    continue;
                }
            };
            // Hybrid GPUs can list a surface that they then fail to configure.
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            surface.configure(&device, &probe);
            if let Some(error) = pollster::block_on(scope.pop()) {
                refusals.push(format!("{name}: {error}"));
                continue;
            }
            let gpu = Rc::new(Self::build(api, adapter, device, queue));
            GPU.with_borrow_mut(|slot| *slot = Some(gpu.clone()));
            return Ok((gpu, surface));
        }
        bail!("no GPU can draw to the window: {}", refusals.join("; "))
    }

    fn build(api: wgpu::Instance, adapter: wgpu::Adapter, device: Device, queue: Queue) -> Self {
        let lost = Arc::new(AtomicBool::new(false));
        let flag = lost.clone();
        device.set_device_lost_callback(move |why, message| {
            if why != wgpu::DeviceLostReason::Destroyed {
                eprintln!("apps: the GPU was lost: {message}");
                flag.store(true, Relaxed);
            }
        });
        device.on_uncaptured_error(Arc::new(|error| eprintln!("apps: GPU error: {error}")));
        let sampled = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST;
        let bgra = adapter.get_texture_format_features(TextureFormat::Bgra8Unorm);
        let layout = |entries: &[BindGroupLayoutEntry]| {
            let described = wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries,
            };
            device.create_bind_group_layout(&described)
        };
        let filtering = BindingType::Sampler(wgpu::SamplerBindingType::Filtering);
        let visibility = ShaderStages::VERTEX_FRAGMENT;
        let sampler = BindGroupLayoutEntry {
            binding: 1,
            visibility,
            ty: filtering,
            count: None,
        };
        let groups = [
            layout(&[slot(0, Some(size_of::<Globals>())), sampler]),
            layout(&[slot(0, None), slot(1, None), slot(2, None)]),
            layout(&[
                slot(0, Some(size_of::<Composite>())),
                slot(1, Some(size_of::<Blur>())),
            ]),
        ];
        let blank = texture(
            &device,
            [1, 1],
            TextureFormat::Rgba8Unorm,
            TextureUsages::TEXTURE_BINDING,
        );
        let source = wgpu::ShaderSource::Wgsl(include_str!("../../shaders.wgsl").into());
        let linear = wgpu::FilterMode::Linear;
        Self {
            module: device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source,
            }),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: linear,
                min_filter: linear,
                ..Default::default()
            }),
            color: match bgra.allowed_usages.contains(sampled) {
                true => TextureFormat::Bgra8Unorm,
                false => TextureFormat::Rgba8Unorm,
            },
            blank: blank.create_view(&Default::default()),
            api,
            adapter,
            device,
            queue,
            lost,
            groups,
            pipelines: RefCell::default(),
        }
    }

    fn pipelines(&self, format: TextureFormat) -> Rc<Pipelines> {
        if let Some((_, made)) = self
            .pipelines
            .borrow()
            .iter()
            .find(|(kept, _)| *kept == format)
        {
            return made.clone();
        }
        // An instance is one step of 13 vec4 attributes.
        let attributes: [wgpu::VertexAttribute; ATTRIBUTES] = std::array::from_fn(|at| {
            let (offset, shader_location) = (at as u64 * 16, at as u32);
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset,
                shader_location,
            }
        });
        let steps = [Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &attributes,
        })];
        let groups = self.groups.each_ref().map(Some);
        let make = |stage: &str, blend| {
            let (groups, buffers) = match stage {
                "draw" => (&groups[..2], &steps[..]),
                _ => (&groups[..], &[][..]),
            };
            let shape = wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: groups,
                immediate_size: 0,
            };
            let layout = self.device.create_pipeline_layout(&shape);
            let (vertex, fragment) = (format!("{stage}_vertex"), format!("{stage}_fragment"));
            let module = &self.module;
            let target = wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            };
            let compilation_options = wgpu::PipelineCompilationOptions::default();
            self.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(stage),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some(&vertex),
                        buffers,
                        compilation_options: compilation_options.clone(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some(&fragment),
                        targets: &[Some(target)],
                        compilation_options,
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: Default::default(),
                    multiview_mask: None,
                    cache: None,
                })
        };
        // Fragments are premultiplied. Blurs replace what the target holds.
        let over = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let (draw, composite) = (make("draw", over), make("composite", over));
        let made = Rc::new(Pipelines {
            draw,
            composite,
            blur: make("blur", None),
        });
        self.pipelines.borrow_mut().push((format, made.clone()));
        made
    }

    fn buffer(&self, size: u64, usage: BufferUsages) -> Buffer {
        let usage = usage | BufferUsages::COPY_DST;
        let described = wgpu::BufferDescriptor {
            label: None,
            size,
            usage,
            mapped_at_creation: false,
        };
        self.device.create_buffer(&described)
    }
}

fn slots<T: Plain>(items: &[T], stride: usize, out: &mut Vec<u8>) {
    for item in items {
        let start = out.len();
        out.extend_from_slice(bytes(std::slice::from_ref(item)));
        out.resize(start + stride, 0);
    }
}

fn configure(
    gpu: &Gpu,
    surface: &Surface<'static>,
    size: [u32; 2],
    transparent: bool,
    mailbox: bool,
) -> Result<SurfaceConfiguration> {
    let caps = surface.get_capabilities(&gpu.adapter);
    let max = gpu.device.limits().max_texture_dimension_2d;
    let [width, height] = size.map(|side| side.clamp(1, max));
    let mut config = surface
        .get_default_config(&gpu.adapter, width, height)
        .context("the window surface has no formats")?;
    // The shaders write sRGB values, which an sRGB surface would convert again.
    let unconverted = caps
        .formats
        .iter()
        .copied()
        .filter(|format| !format.is_srgb());
    let format = [TextureFormat::Bgra8Unorm, TextureFormat::Rgba8Unorm]
        .into_iter()
        .chain(unconverted)
        .find(|format| caps.formats.contains(format));
    config.format = format.unwrap_or(config.format);
    let alpha = match transparent {
        true => wgpu::CompositeAlphaMode::PreMultiplied,
        false => wgpu::CompositeAlphaMode::Opaque,
    };
    let modes = &caps.alpha_modes;
    let alpha = [alpha, wgpu::CompositeAlphaMode::Inherit]
        .into_iter()
        .find(|mode| modes.contains(mode));
    config.alpha_mode = alpha
        .or(modes.first().copied())
        .unwrap_or(config.alpha_mode);
    // A hidden Wayland window gets no frame callbacks, which blocks FIFO presents, so mailbox is used there.
    let mailbox = mailbox && caps.present_modes.contains(&wgpu::PresentMode::Mailbox);
    config.present_mode = match mailbox {
        true => wgpu::PresentMode::Mailbox,
        false => wgpu::PresentMode::Fifo,
    };
    config.usage |= caps.usages & TextureUsages::COPY_SRC;
    surface.configure(&gpu.device, &config);
    Ok(config)
}

pub(super) struct Renderer {
    gpu: Rc<Gpu>,
    display: RawDisplayHandle,
    window: RawWindowHandle,
    surface: Option<Surface<'static>>,
    config: SurfaceConfiguration,
    transparent: bool,
    mailbox: bool,
    pipelines: Rc<Pipelines>,
    atlas: Rc<Atlas>,
    instances: Option<Buffer>,
    uniforms: Option<(Buffer, BindGroup, BindGroup)>,
    pool: Vec<(Texture, TextureView, u8)>,
    targets: Vec<usize>,
    sources: (u64, FxHashMap<[Option<u32>; 2], BindGroup>),
    staging: Vec<u8>,
    frame: RenderFrame,
}

impl Renderer {
    pub fn new(
        display: RawDisplayHandle,
        window: RawWindowHandle,
        size: [u32; 2],
        transparent: bool,
        mailbox: bool,
    ) -> Result<Self> {
        let (gpu, surface) = Gpu::surface(display, window)?;
        let config = configure(&gpu, &surface, size, transparent, mailbox)?;
        Ok(Self {
            pipelines: gpu.pipelines(config.format),
            atlas: Rc::new(Atlas::new(Pages::new(gpu.clone()))),
            gpu,
            display,
            window,
            surface: Some(surface),
            config,
            transparent,
            mailbox,
            instances: None,
            uniforms: None,
            pool: Vec::new(),
            targets: Vec::new(),
            sources: (u64::MAX, FxHashMap::default()),
            staging: Vec::new(),
            frame: RenderFrame::default(),
        })
    }

    pub fn atlas(&self) -> &Rc<Atlas> {
        &self.atlas
    }

    pub fn max_side(&self) -> u32 {
        self.gpu.device.limits().max_texture_dimension_2d
    }

    pub fn resize(&mut self, size: [u32; 2]) {
        let max = self.max_side();
        let [width, height] = size.map(|side| side.clamp(1, max));
        if [width, height] != [self.config.width, self.config.height] {
            (self.config.width, self.config.height) = (width, height);
            if let Some(surface) = &self.surface {
                surface.configure(&self.gpu.device, &self.config);
            }
        }
    }

    // The atlas stays, since the last frame still reads it.
    pub fn release(&mut self) {
        self.pool.clear();
        self.instances = None;
        self.uniforms = None;
        self.staging = Vec::new();
    }

    pub fn draw(&mut self, scene: &Scene, capture: bool) -> (Drawn, Capture) {
        let failed = |message: &str| capture.then(|| Err(anyhow!("{message}")));
        if self.gpu.lost.load(Relaxed) || self.surface.is_none() {
            if let Err(error) = self.recover() {
                eprintln!("apps: the GPU can't recover: {error:#}");
            }
            return (Drawn::Again, failed("The GPU was lost"));
        }
        let Some(surface) = &self.surface else {
            return (Drawn::Skipped, None);
        };
        let output = match surface.get_current_texture() {
            Next::Success(output) => output,
            Next::Suboptimal(stale) => {
                // The texture must go before the surface can be configured again.
                drop(stale);
                surface.configure(&self.gpu.device, &self.config);
                return (Drawn::Again, failed("The window surface changed"));
            }
            Next::Outdated => {
                surface.configure(&self.gpu.device, &self.config);
                return (Drawn::Again, failed("The window surface changed"));
            }
            Next::Lost => {
                self.surface = None;
                return (Drawn::Again, failed("The window surface was lost"));
            }
            Next::Timeout | Next::Occluded => {
                return (Drawn::Skipped, failed("The window can't be seen"));
            }
            Next::Validation => return (Drawn::Skipped, failed("The window surface is not valid")),
        };
        let size = [output.texture.width(), output.texture.height()];
        let read = match self.encode(scene, &output.texture, capture) {
            Ok(read) => read,
            Err(error) => {
                eprintln!("apps: drawing failed: {error:#}");
                // Queue writes wait for a submit, which also releases them when the frame fails.
                self.gpu.queue.submit(None);
                return (Drawn::Again, capture.then_some(Err(error)));
            }
        };
        self.gpu.queue.present(output);
        (Drawn::Shown, read.map(|buffer| self.read(buffer?, size)))
    }

    fn recover(&mut self) -> Result<()> {
        // A window has one swapchain, so the old surface goes first.
        self.surface = None;
        let (gpu, surface) = Gpu::surface(self.display, self.window)?;
        if !Rc::ptr_eq(&gpu, &self.gpu) {
            self.atlas.clear(|pages| *pages = Pages::new(gpu.clone()));
            self.release();
            self.sources = (u64::MAX, FxHashMap::default());
            self.gpu = gpu;
        }
        let size = [self.config.width, self.config.height];
        self.config = configure(&self.gpu, &surface, size, self.transparent, self.mailbox)?;
        self.pipelines = self.gpu.pipelines(self.config.format);
        self.surface = Some(surface);
        Ok(())
    }

    fn read(&self, buffer: Buffer, [width, height]: [u32; 2]) -> Result<([u32; 2], Vec<u8>)> {
        let whole = buffer.slice(..);
        whole.map_async(wgpu::MapMode::Read, |_| {});
        let waited = self.gpu.device.poll(wgpu::PollType::wait_indefinitely());
        waited.map_err(|error| anyhow!("reading the frame: {error}"))?;
        let mapped = whole
            .get_mapped_range()
            .map_err(|error| anyhow!("reading the frame: {error}"))?;
        let row = width as usize * 4;
        let stride = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
        let lines = mapped.chunks_exact(stride).take(height as usize);
        let mut pixels: Vec<u8> = lines.flat_map(|line| &line[..row]).copied().collect();
        if self.config.format == TextureFormat::Bgra8Unorm {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        Ok(([width, height], pixels))
    }

    fn prepare(&mut self, frame: &RenderFrame) -> Result<[u64; 3]> {
        let gpu = &self.gpu;
        for (.., idle) in &mut self.pool {
            *idle = idle.saturating_add(1);
        }
        self.pool.retain(|(.., idle)| *idle <= IDLE_FRAMES);
        self.targets.clear();
        let usage = TextureUsages::RENDER_ATTACHMENT
            | TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_SRC
            | TextureUsages::COPY_DST;
        for &size in &frame.textures {
            let free = |(made, _, idle): &(Texture, TextureView, u8)| {
                *idle > 0 && [made.width(), made.height()] == size
            };
            let at = match self.pool.iter().position(free) {
                Some(at) => at,
                None => {
                    let made = texture(&gpu.device, size, self.config.format, usage);
                    let view = made.create_view(&Default::default());
                    self.pool.push((made, view, 0));
                    self.pool.len() - 1
                }
            };
            self.pool[at].2 = 0;
            self.targets.push(at);
        }

        let data = bytes(&frame.instances);
        let length = data.len() as u64;
        if length > 0 {
            if self
                .instances
                .as_ref()
                .is_none_or(|kept| kept.size() < length)
            {
                ensure!(
                    length <= MAX_INSTANCE_BYTES,
                    "a frame has too many instances"
                );
                let grown = gpu.buffer(length.next_power_of_two(), BufferUsages::VERTEX);
                self.instances = Some(grown);
            }
            if let Some(buffer) = &self.instances {
                gpu.queue.write_buffer(buffer, 0, data);
            }
        }

        // One buffer holds the globals of each target, then the composites, then the blurs.
        let align = gpu.device.limits().min_uniform_buffer_offset_alignment as usize;
        let sizes = [
            size_of::<Globals>(),
            size_of::<Composite>(),
            size_of::<Blur>(),
        ];
        let strides = sizes.map(|size| size.next_multiple_of(align));
        let targets =
            std::iter::once(Target::Frame).chain((0..frame.layers.len()).map(Target::Layer));
        let globals: Vec<Globals> = targets
            .map(|target| frame.globals(target, TEXT_CONTRAST))
            .collect();
        let staging = &mut self.staging;
        staging.clear();
        slots(&globals, strides[0], staging);
        slots(&frame.composites, strides[1], staging);
        slots(&frame.blurs, strides[2], staging);
        // Every binding reads a whole block, also at offset 0.
        staging.resize(staging.len().max(strides[1]), 0);
        let length = staging.len() as u64;
        if self
            .uniforms
            .as_ref()
            .is_none_or(|(kept, ..)| kept.size() < length)
        {
            let buffer = gpu.buffer(length.next_power_of_two(), BufferUsages::UNIFORM);
            let block = |size: usize| {
                let size = NonZeroU64::new(size as u64);
                BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size,
                })
            };
            let group = |layout, [first, second]: [BindingResource; 2]| {
                let entries = [
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: first,
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: second,
                    },
                ];
                let described = wgpu::BindGroupDescriptor {
                    label: None,
                    layout,
                    entries: &entries,
                };
                gpu.device.create_bind_group(&described)
            };
            let sampler = BindingResource::Sampler(&gpu.sampler);
            let shared = group(&gpu.groups[0], [block(sizes[0]), sampler]);
            let params = group(&gpu.groups[2], [block(sizes[1]), block(sizes[2])]);
            self.uniforms = Some((buffer, shared, params));
        }
        if let Some((buffer, ..)) = &self.uniforms {
            gpu.queue.write_buffer(buffer, 0, staging);
        }
        Ok(strides.map(|stride| stride as u64))
    }

    fn encode(
        &mut self,
        scene: &Scene,
        output: &Texture,
        capture: bool,
    ) -> Result<Option<Result<Buffer>>> {
        // Copies read the frame, which needs a surface that allows them.
        let copies = self.config.usage.contains(TextureUsages::COPY_SRC);
        let mut frame = std::mem::take(&mut self.frame);
        frame.record(
            scene,
            [output.width(), output.height()],
            self.max_side(),
            copies,
        );
        let strides = self.prepare(&frame);
        let result = strides.and_then(|strides| self.commands(&frame, output, strides, capture));
        self.frame = frame;
        result
    }

    fn commands(
        &mut self,
        frame: &RenderFrame,
        output: &Texture,
        strides: [u64; 3],
        capture: bool,
    ) -> Result<Option<Result<Buffer>>> {
        let gpu = self.gpu.clone();
        let pages = self.atlas.pages.borrow();
        if self.sources.0 != pages.generation {
            self.sources = (pages.generation, FxHashMap::default());
        }
        let Some((_, shared, params)) = &self.uniforms else {
            bail!("the frame has no uniforms");
        };
        let pipelines = &self.pipelines;
        let composites = strides[0] * (frame.layers.len() as u64 + 1);
        let blurs = composites + strides[1] * frame.composites.len() as u64;
        let globals_of = |target: Target| match target {
            Target::Frame => 0,
            Target::Layer(index) => (index as u64 + 1) * strides[0],
        } as u32;
        let frame_view = output.create_view(&Default::default());
        let (pool, targets) = (&self.pool, &self.targets);
        let pooled = |target: Target| match target {
            Target::Frame => None,
            Target::Layer(index) => Some(&pool[targets[frame.layers[index].texture]]),
        };
        let view = |target: Target| pooled(target).map_or(&frame_view, |(_, view, _)| view);
        let raw = |target: Target| pooled(target).map_or(output, |(made, ..)| made);
        let bind = |views: [&TextureView; 3]| {
            let entries: [_; 3] = std::array::from_fn(|at| {
                let resource = BindingResource::TextureView(views[at]);
                wgpu::BindGroupEntry {
                    binding: at as u32,
                    resource,
                }
            });
            let described = wgpu::BindGroupDescriptor {
                label: None,
                layout: &gpu.groups[1],
                entries: &entries,
            };
            gpu.device.create_bind_group(&described)
        };
        let page = |color: bool, index: Option<u32>| {
            index
                .and_then(|index| pages.view(color, index))
                .unwrap_or(&gpu.blank)
        };
        let background = if self.transparent {
            Color::TRANSPARENT
        } else {
            Color::BLACK
        };
        let open = |encoder: &mut wgpu::CommandEncoder, target: Target, wipe: bool| {
            let load = match (wipe, target) {
                (false, _) => wgpu::LoadOp::Load,
                (true, Target::Frame) => wgpu::LoadOp::Clear(background),
                (true, Target::Layer(_)) => wgpu::LoadOp::Clear(Color::TRANSPARENT),
            };
            let ops = wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            };
            let attachment = wgpu::RenderPassColorAttachment {
                view: view(target),
                depth_slice: None,
                resolve_target: None,
                ops,
            };
            let colors = [Some(attachment)];
            let described = wgpu::RenderPassDescriptor {
                color_attachments: &colors,
                ..Default::default()
            };
            let mut pass = encoder.begin_render_pass(&described).forget_lifetime();
            pass.set_bind_group(0, shared, &[globals_of(target)]);
            pass
        };
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let mut pass: Option<wgpu::RenderPass<'static>> = None;
        let mut current = (Target::Frame, true);
        for command in &frame.commands {
            match *command {
                Command::Pass(target, false)
                    if let Some(into) = &mut pass
                        && frame.texture(target) == frame.texture(current.0) =>
                {
                    into.set_bind_group(0, shared, &[globals_of(target)]);
                    current = (target, false);
                }
                // A target is cleared at once, since a later pass can draw into it.
                Command::Pass(target, wipe) => {
                    // WARNING: end the old pass first. Two open passes put the encoder in an error state.
                    drop(pass.take());
                    pass = wipe.then(|| open(&mut encoder, target, true));
                    current = (target, false);
                }
                Command::Draw {
                    ref instances,
                    mask_page,
                    color_page,
                } => {
                    let Some(buffer) = &self.instances else {
                        continue;
                    };
                    let wipe = std::mem::take(&mut current.1);
                    let into = pass.get_or_insert_with(|| open(&mut encoder, current.0, wipe));
                    let sources = self
                        .sources
                        .1
                        .entry([mask_page, color_page])
                        .or_insert_with(|| {
                            bind([page(false, mask_page), page(true, color_page), &gpu.blank])
                        });
                    into.set_pipeline(&pipelines.draw);
                    into.set_bind_group(1, &*sources, &[]);
                    into.set_vertex_buffer(0, buffer.slice(..));
                    into.draw(0..4, instances.clone());
                }
                Command::Blur { src, dst, index } => {
                    pass = None;
                    let mut blur = open(&mut encoder, dst, true);
                    blur.set_pipeline(&pipelines.blur);
                    blur.set_bind_group(1, &bind([view(src); 3]), &[]);
                    let offset = blurs + u64::from(index) * strides[2];
                    blur.set_bind_group(2, params, &[0, offset as u32]);
                    blur.draw(0..4, 0..1);
                }
                Command::Copy { src, dst, region } => {
                    pass = None;
                    let place = |target: Target| {
                        let [x, y] = frame.origin(target);
                        Origin3d {
                            x: (region[0] - x) as u32,
                            y: (region[1] - y) as u32,
                            z: 0,
                        }
                    };
                    let from = wgpu::TexelCopyTextureInfo {
                        origin: place(src),
                        ..raw(src).as_image_copy()
                    };
                    let to = wgpu::TexelCopyTextureInfo {
                        origin: place(dst),
                        ..raw(dst).as_image_copy()
                    };
                    encoder.copy_texture_to_texture(from, to, extent(region[2], region[3]));
                }
                Command::Composite {
                    src,
                    mask,
                    backdrop,
                    index,
                } => {
                    let wipe = std::mem::take(&mut current.1);
                    let into = pass.get_or_insert_with(|| open(&mut encoder, current.0, wipe));
                    let source = view(src);
                    let mask = mask.map_or(source, |mask| page(true, Some(mask)));
                    into.set_pipeline(&pipelines.composite);
                    into.set_bind_group(
                        1,
                        &bind([source, mask, backdrop.map_or(source, view)]),
                        &[],
                    );
                    let offset = composites + u64::from(index) * strides[1];
                    into.set_bind_group(2, params, &[offset as u32, 0]);
                    into.draw(0..4, 0..1);
                }
            }
        }
        drop(pass);
        let read = capture.then(|| {
            let readable = self.config.usage.contains(TextureUsages::COPY_SRC);
            ensure!(readable, "The window surface can't be read");
            let row = (output.width() * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let buffer = gpu.buffer(
                u64::from(row) * u64::from(output.height()),
                BufferUsages::MAP_READ,
            );
            let layout = wgpu::TexelCopyBufferLayout {
                bytes_per_row: Some(row),
                ..Default::default()
            };
            let into = wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout,
            };
            encoder.copy_texture_to_buffer(output.as_image_copy(), into, output.size());
            Ok(buffer)
        });
        gpu.queue.submit([encoder.finish()]);
        Ok(read)
    }
}

pub(crate) struct Pages {
    gpu: Rc<Gpu>,
    mask: PageList<Page>,
    color: PageList<Page>,
    // Changes whenever a page comes or goes, so cached bind groups never read a dropped texture.
    generation: u64,
}

struct Page {
    packer: Packer,
    texture: Texture,
    view: TextureView,
}

impl Pages {
    fn new(gpu: Rc<Gpu>) -> Self {
        Self {
            gpu,
            mask: PageList::default(),
            color: PageList::default(),
            generation: 0,
        }
    }

    fn view(&self, color: bool, index: u32) -> Option<&TextureView> {
        let list = if color { &self.color } else { &self.mask };
        Some(&list.get(index as usize)?.view)
    }
}

impl PageStore for Pages {
    fn insert(&mut self, kind: PageKind, size: Size<i32>, data: &[u8]) -> Result<Tile> {
        ensure!(
            size.width > 0 && size.height > 0,
            "atlas tiles must have a size"
        );
        let color = matches!(kind, PageKind::Color);
        let row = size.width as usize * if color { 4 } else { 1 };
        let data = data
            .get(..row * size.height as usize)
            .context("atlas upload is too short")?;
        let gpu = self.gpu.clone();
        let format = if color {
            gpu.color
        } else {
            TextureFormat::R8Unorm
        };
        let list = if color {
            &mut self.color
        } else {
            &mut self.mask
        };
        let mut grew = false;
        let (index, rect, id) = list.allocate(
            size,
            |page| &mut page.packer,
            |size| {
                grew = true;
                let side = [size.width as u32, size.height as u32];
                let usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST;
                let made = texture(&gpu.device, side, format, usage);
                let view = made.create_view(&Default::default());
                Ok(Page {
                    packer: Packer::new(size),
                    texture: made,
                    view,
                })
            },
        )?;
        self.generation += u64::from(grew);
        let page = list.get(index).context("atlas page")?;
        let swizzled: Vec<u8>;
        let data = match color && format == TextureFormat::Rgba8Unorm {
            true => {
                swizzled = data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|&[b, g, r, a]| [r, g, b, a])
                    .collect();
                &swizzled
            }
            false => data,
        };
        let origin = Origin3d {
            x: rect.origin.x as u32,
            y: rect.origin.y as u32,
            z: 0,
        };
        let target = wgpu::TexelCopyTextureInfo {
            origin,
            ..page.texture.as_image_copy()
        };
        let layout = wgpu::TexelCopyBufferLayout {
            bytes_per_row: Some(row as u32),
            ..Default::default()
        };
        gpu.queue
            .write_texture(target, data, layout, extent(size.width, size.height));
        Ok(Tile {
            page: index as u32,
            kind,
            id,
            rect,
        })
    }

    fn max_side(&self) -> i32 {
        self.gpu.device.limits().max_texture_dimension_2d as i32
    }

    fn remove(&mut self, tile: Tile) {
        let list = match tile.kind {
            PageKind::Mask => &mut self.mask,
            PageKind::Color => &mut self.color,
        };
        let index = tile.page as usize;
        let page = list.pages.get_mut(index).and_then(Option::as_mut);
        if page.is_some_and(|page| page.packer.free(tile.id)) {
            list.emptied(index, |page| page.packer.is_empty());
            self.generation += 1;
        }
    }
}
