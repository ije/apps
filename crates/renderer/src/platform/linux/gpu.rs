use crate::{
    DevicePixels, Size,
    atlas::{Atlas, AtlasBackend, Packer, TextureList, page_size},
    frame::{Command, Globals, RenderFrame, Target, TextParams, bytes},
    scene::{AtlasTextureId, AtlasTextureKind, AtlasTile, Scene},
};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use raw_window_handle as rwh;
use std::{
    cell::RefCell,
    mem,
    num::NonZeroU64,
    ops::Range,
    rc::Rc,
    slice,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const SHADERS: &str = include_str!("shaders.wgsl");
const MAX_INSTANCE_BUFFER_SIZE: u64 = 256 * 1024 * 1024;
const TEXT: TextParams = TextParams {
    gamma_ratios: [
        0.1469 * 65536. / 65025.,
        -0.8911 * 256. / 255.,
        1.4644 * 65536. / 65025.,
        -0.3234 * 256. / 255.,
    ],
    grayscale_enhanced_contrast: 1.,
    subpixel_enhanced_contrast: 0.,
    is_bgr: false,
};

pub(crate) type GpuAtlas = Atlas<AtlasTextures>;
pub(crate) type Capture = Option<Result<([u32; 2], Vec<u8>)>>;

#[derive(Debug)]
struct Display(rwh::RawDisplayHandle);

// SAFETY: the connection outlives every GPU object, and wgpu only reads the handle.
unsafe impl Send for Display {}
// SAFETY: as above.
unsafe impl Sync for Display {}

impl rwh::HasDisplayHandle for Display {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        // SAFETY: see `Send` above.
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(self.0) })
    }
}

struct Layouts {
    globals: wgpu::BindGroupLayout,
    instances: wgpu::BindGroupLayout,
    texture: wgpu::BindGroupLayout,
    composite: wgpu::BindGroupLayout,
}

struct Pipelines {
    quads: wgpu::RenderPipeline,
    shadows: wgpu::RenderPipeline,
    underlines: wgpu::RenderPipeline,
    mono: wgpu::RenderPipeline,
    poly: wgpu::RenderPipeline,
    composites: wgpu::RenderPipeline,
    blurs: wgpu::RenderPipeline,
}

struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    lost: Arc<AtomicBool>,
    color_format: wgpu::TextureFormat,
    layouts: Layouts,
    sampler: wgpu::Sampler,
    module: wgpu::ShaderModule,
    pipelines: RefCell<Vec<(wgpu::TextureFormat, Rc<Pipelines>)>>,
}

thread_local! {
    static GPU: RefCell<Option<Rc<Gpu>>> = const { RefCell::new(None) };
}

impl Gpu {
    fn get(
        display: rwh::RawDisplayHandle,
        window: rwh::RawWindowHandle,
    ) -> Result<(Rc<Self>, wgpu::Surface<'static>)> {
        let current = GPU
            .with_borrow(Clone::clone)
            .filter(|gpu| !gpu.lost.load(Ordering::Relaxed));
        let instance = match &current {
            Some(gpu) => gpu.instance.clone(),
            None => wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
                flags: wgpu::InstanceFlags::default(),
                backend_options: wgpu::BackendOptions::default(),
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
                display: Some(Box::new(Display(display))),
            }),
        };
        // SAFETY: the platform window owns the renderer, which drops the surface before the window.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: None,
                raw_window_handle: window,
            })
        }
        .map_err(|error| anyhow!("creating the window surface: {error}"))?;
        if let Some(gpu) = current {
            ensure!(
                !surface.get_capabilities(&gpu.adapter).formats.is_empty(),
                "the GPU cannot draw to this window"
            );
            return Ok((gpu, surface));
        }
        let gpu = Rc::new(Self::new(instance, &surface)?);
        GPU.with_borrow_mut(|slot| *slot = Some(gpu.clone()));
        Ok((gpu, surface))
    }

    fn new(instance: wgpu::Instance, surface: &wgpu::Surface<'_>) -> Result<Self> {
        let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        adapters.sort_by_key(|adapter| {
            let info = adapter.get_info();
            let kind = match info.device_type {
                wgpu::DeviceType::IntegratedGpu => 0,
                wgpu::DeviceType::DiscreteGpu => 1,
                wgpu::DeviceType::Other => 2,
                wgpu::DeviceType::VirtualGpu => 3,
                wgpu::DeviceType::Cpu => 4,
            };
            (kind, info.backend != wgpu::Backend::Vulkan)
        });
        let mut errors = Vec::new();
        // Hybrid GPUs can report a surface they fail to configure, so each one is tried.
        for adapter in adapters {
            let caps = surface.get_capabilities(&adapter);
            if caps.formats.is_empty() || caps.alpha_modes.is_empty() {
                continue;
            }
            let result = pollster::block_on(
                adapter.request_device(&wgpu::DeviceDescriptor {
                    label: None,
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits())
                        .using_alignment(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                    trace: wgpu::Trace::Off,
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                }),
            );
            let (device, queue) = match result {
                Ok(device) => device,
                Err(error) => {
                    errors.push(format!("{}: {error}", adapter.get_info().name));
                    continue;
                }
            };
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            surface.configure(
                &device,
                &wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: caps.formats[0],
                    color_space: wgpu::SurfaceColorSpace::Auto,
                    width: 64,
                    height: 64,
                    present_mode: wgpu::PresentMode::Fifo,
                    desired_maximum_frame_latency: 2,
                    alpha_mode: caps.alpha_modes[0],
                    view_formats: vec![],
                },
            );
            if let Some(error) = pollster::block_on(scope.pop()) {
                errors.push(format!("{}: {error}", adapter.get_info().name));
                continue;
            }
            return Ok(Self::with_device(instance, adapter, device, queue));
        }
        bail!("No GPU can draw to the window: {}", errors.join("; "))
    }

    fn with_device(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Self {
        let lost = Arc::new(AtomicBool::new(false));
        device.set_device_lost_callback({
            let lost = lost.clone();
            move |reason, message| {
                if reason != wgpu::DeviceLostReason::Destroyed {
                    eprintln!("apps: GPU lost: {message}");
                    lost.store(true, Ordering::Relaxed);
                }
            }
        });
        device.on_uncaptured_error(Arc::new(|error| eprintln!("apps: GPU error: {error}")));
        let usages = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let color_format = if adapter
            .get_texture_format_features(wgpu::TextureFormat::Bgra8Unorm)
            .allowed_usages
            .contains(usages)
        {
            wgpu::TextureFormat::Bgra8Unorm
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let texture = |binding| {
            entry(
                binding,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            )
        };
        let sampler = entry(
            1,
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        );
        let layout = |entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries,
            })
        };
        let layouts = Layouts {
            globals: layout(&[entry(
                0,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(mem::size_of::<Globals>() as u64),
                },
            )]),
            instances: layout(&[entry(
                0,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            )]),
            texture: layout(&[texture(0), sampler]),
            composite: layout(&[texture(0), sampler, texture(2), texture(3)]),
        };
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(SHADERS.into()),
        });
        Self {
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            instance,
            adapter,
            device,
            queue,
            lost,
            color_format,
            layouts,
            module,
            pipelines: RefCell::default(),
        }
    }

    fn pipelines(&self, format: wgpu::TextureFormat) -> Rc<Pipelines> {
        if let Some((_, pipelines)) = self
            .pipelines
            .borrow()
            .iter()
            .find(|(entry, _)| *entry == format)
        {
            return pipelines.clone();
        }
        let layouts = &self.layouts;
        let pipeline = |name: &str,
                        texture: Option<&wgpu::BindGroupLayout>,
                        blend: Option<wgpu::BlendState>| {
            let groups = [Some(&layouts.globals), Some(&layouts.instances), texture];
            let layout = self
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: None,
                    bind_group_layouts: &groups[..if texture.is_some() { 3 } else { 2 }],
                    immediate_size: 0,
                });
            self.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(name),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &self.module,
                        entry_point: Some(&format!("vs_{name}")),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &self.module,
                        entry_point: Some(&format!("fs_{name}")),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                })
        };
        // Colors are drawn unpremultiplied over targets that hold premultiplied colors.
        let over = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        });
        let texture = Some(&layouts.texture);
        let pipelines = Rc::new(Pipelines {
            quads: pipeline("quad", None, over),
            shadows: pipeline("shadow", None, over),
            underlines: pipeline("underline", None, over),
            mono: pipeline("mono_sprite", texture, over),
            poly: pipeline("poly_sprite", texture, over),
            composites: pipeline(
                "composite",
                Some(&layouts.composite),
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            ),
            blurs: pipeline("blur", texture, None),
        });
        self.pipelines
            .borrow_mut()
            .push((format, pipelines.clone()));
        pipelines
    }

    fn texture_group(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layouts.texture,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn buffer(&self, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Drawn {
    Presented,
    Repaint,
    Skipped,
}

pub(crate) struct Renderer {
    gpu: Rc<Gpu>,
    display: rwh::RawDisplayHandle,
    window: rwh::RawWindowHandle,
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    alpha_modes: [wgpu::CompositeAlphaMode; 2],
    pipelines: Rc<Pipelines>,
    atlas: Rc<GpuAtlas>,
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
    globals_stride: u64,
    instances: wgpu::Buffer,
    empty: wgpu::TextureView,
    pool: Vec<(wgpu::Texture, wgpu::TextureView)>,
    frame: RenderFrame,
}

impl Renderer {
    pub fn new(
        display: rwh::RawDisplayHandle,
        window: rwh::RawWindowHandle,
        size: Size<DevicePixels>,
        transparent: bool,
        mailbox: bool,
    ) -> Result<Self> {
        let (gpu, surface) = Gpu::get(display, window)?;
        let atlas = Rc::new(Atlas::new(AtlasTextures::new(gpu.clone())));
        Self::with_surface(
            gpu,
            surface,
            display,
            window,
            size,
            transparent,
            mailbox,
            atlas,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn with_surface(
        gpu: Rc<Gpu>,
        surface: wgpu::Surface<'static>,
        display: rwh::RawDisplayHandle,
        window: rwh::RawWindowHandle,
        size: Size<DevicePixels>,
        transparent: bool,
        mailbox: bool,
        atlas: Rc<GpuAtlas>,
    ) -> Result<Self> {
        let caps = surface.get_capabilities(&gpu.adapter);
        // An sRGB surface would convert the colors the shaders already write in sRGB.
        let format = [
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
        ]
        .into_iter()
        .find(|format| caps.formats.contains(format))
        .or_else(|| {
            caps.formats
                .iter()
                .copied()
                .find(|format| !format.is_srgb())
        })
        .or_else(|| caps.formats.first().copied())
        .context("the window surface has no formats")?;
        let alpha = |modes: &[wgpu::CompositeAlphaMode]| {
            modes
                .iter()
                .copied()
                .find(|mode| caps.alpha_modes.contains(mode))
                .unwrap_or(caps.alpha_modes[0])
        };
        let alpha_modes = [
            alpha(&[
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::Inherit,
            ]),
            alpha(&[
                wgpu::CompositeAlphaMode::PreMultiplied,
                wgpu::CompositeAlphaMode::Inherit,
            ]),
        ];
        let max = gpu.device.limits().max_texture_dimension_2d;
        let present_mode = if mailbox && caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | (caps.usages & wgpu::TextureUsages::COPY_SRC),
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: (size.width.0 as u32).clamp(1, max),
            height: (size.height.0 as u32).clamp(1, max),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: alpha_modes[transparent as usize],
            view_formats: vec![],
        };
        surface.configure(&gpu.device, &config);
        let limits = gpu.device.limits();
        let globals_stride = (mem::size_of::<Globals>() as u64)
            .next_multiple_of(u64::from(limits.min_uniform_buffer_offset_alignment));
        let globals = gpu.buffer(globals_stride * 8, wgpu::BufferUsages::UNIFORM);
        let empty = gpu
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Ok(Self {
            pipelines: gpu.pipelines(format),
            globals_group: globals_group(&gpu, &globals),
            instances: gpu.buffer(2 * 1024 * 1024, wgpu::BufferUsages::STORAGE),
            globals,
            globals_stride,
            empty,
            gpu,
            display,
            window,
            surface: Some(surface),
            config,
            alpha_modes,
            atlas,
            pool: Vec::new(),
            frame: RenderFrame::default(),
        })
    }

    pub fn atlas(&self) -> &Rc<GpuAtlas> {
        &self.atlas
    }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        let max = self.gpu.device.limits().max_texture_dimension_2d;
        let (width, height) = (
            (size.width.0 as u32).clamp(1, max),
            (size.height.0 as u32).clamp(1, max),
        );
        if (width, height) != (self.config.width, self.config.height) {
            self.config.width = width;
            self.config.height = height;
            if let Some(surface) = &self.surface {
                surface.configure(&self.gpu.device, &self.config);
            }
        }
    }

    pub fn max_texture_size(&self) -> u32 {
        self.gpu.device.limits().max_texture_dimension_2d
    }

    pub fn draw(&mut self, scene: &Scene, capture: bool) -> (Drawn, Capture) {
        if self.gpu.lost.load(Ordering::Relaxed) || self.surface.is_none() {
            let result = self.recover();
            if let Err(error) = &result {
                eprintln!("apps: GPU recovery failed: {error:#}");
            }
            return (
                Drawn::Repaint,
                capture.then(|| result.and(Err(anyhow!("The GPU was lost")))),
            );
        }
        let Some(surface) = &self.surface else {
            return (Drawn::Skipped, None);
        };
        let output = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output) => output,
            wgpu::CurrentSurfaceTexture::Suboptimal(output) => {
                // Textures must go before the surface can be configured again.
                drop(output);
                surface.configure(&self.gpu.device, &self.config);
                return (
                    Drawn::Repaint,
                    capture.then(|| Err(anyhow!("The window surface changed"))),
                );
            }
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                surface.configure(&self.gpu.device, &self.config);
                return (
                    Drawn::Repaint,
                    capture.then(|| Err(anyhow!("The window surface changed"))),
                );
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return (
                    Drawn::Skipped,
                    capture.then(|| Err(anyhow!("The window can't be seen"))),
                );
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return (
                    Drawn::Skipped,
                    capture.then(|| Err(anyhow!("The window surface is not valid"))),
                );
            }
        };
        let readback = match self.render_frame(scene, &output.texture, capture) {
            Ok(readback) => readback,
            Err(error) => {
                eprintln!("apps: failed to render: {error:#}");
                // Queue writes are staged before encoding, so they are flushed even when the frame fails.
                self.gpu.queue.submit(std::iter::empty());
                return (Drawn::Repaint, capture.then(|| Err(error)));
            }
        };
        let size = [output.texture.width(), output.texture.height()];
        self.gpu.queue.present(output);
        (
            Drawn::Presented,
            readback.map(|buffer| self.read_back(buffer?, size)),
        )
    }

    fn recover(&mut self) -> Result<()> {
        // A window has one swapchain at a time, so the old surface goes first.
        self.surface = None;
        let (gpu, surface) = Gpu::get(self.display, self.window)?;
        let transparent = self.config.alpha_mode == self.alpha_modes[1];
        let mailbox = self.config.present_mode == wgpu::PresentMode::Mailbox;
        let size = Size::new(
            DevicePixels(self.config.width as i32),
            DevicePixels(self.config.height as i32),
        );
        let atlas = self.atlas.clone();
        atlas.clear(|textures| *textures = AtlasTextures::new(gpu.clone()));
        *self = Self::with_surface(
            gpu,
            surface,
            self.display,
            self.window,
            size,
            transparent,
            mailbox,
            atlas,
        )?;
        Ok(())
    }

    fn read_back(&self, buffer: wgpu::Buffer, size: [u32; 2]) -> Result<([u32; 2], Vec<u8>)> {
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.gpu
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| anyhow!("reading the frame: {error}"))?;
        let padded = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) as usize;
        let mut pixels = Vec::with_capacity(size[0] as usize * 4 * size[1] as usize);
        let mapped = slice
            .get_mapped_range()
            .map_err(|error| anyhow!("reading the frame: {error}"))?;
        for row in mapped.chunks_exact(padded) {
            pixels.extend_from_slice(&row[..size[0] as usize * 4]);
        }
        if self.config.format == wgpu::TextureFormat::Bgra8Unorm {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        Ok((size, pixels))
    }

    fn render_frame(
        &mut self,
        scene: &Scene,
        output: &wgpu::Texture,
        capture: bool,
    ) -> Result<Option<Result<wgpu::Buffer>>> {
        let copy = self.config.usage.contains(wgpu::TextureUsages::COPY_SRC);
        let mut frame = mem::take(&mut self.frame);
        frame.record(
            scene,
            [output.width(), output.height()],
            self.max_texture_size(),
            copy,
        );
        let result = self.encode(&frame, output, capture);
        self.frame = frame;
        result
    }

    fn prepare(&mut self, frame: &RenderFrame) -> Result<[wgpu::BindGroup; 7]> {
        let gpu = &self.gpu;
        for (index, size) in frame.textures.iter().enumerate() {
            if let Some(found) = self.pool[index..]
                .iter()
                .position(|(texture, _)| [texture.width(), texture.height()] == *size)
            {
                self.pool.swap(index, index + found);
                continue;
            }
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.pool.push((texture, view));
            let last = self.pool.len() - 1;
            self.pool.swap(index, last);
        }
        self.pool.truncate(frame.textures.len());

        let stride = self.globals_stride as usize;
        let mut globals = vec![0; stride * (frame.layers.len() + 1)];
        for (index, target) in std::iter::once(Target::Frame)
            .chain((0..frame.layers.len()).map(Target::Layer))
            .enumerate()
        {
            globals[index * stride..][..mem::size_of::<Globals>()]
                .copy_from_slice(bytes(slice::from_ref(&frame.globals(target, &TEXT))));
        }
        if globals.len() as u64 > self.globals.size() {
            self.globals = gpu.buffer(
                (globals.len() as u64).next_power_of_two(),
                wgpu::BufferUsages::UNIFORM,
            );
            self.globals_group = globals_group(gpu, &self.globals);
        }
        gpu.queue.write_buffer(&self.globals, 0, &globals);

        let arrays: [&[u8]; 7] = [
            bytes(&frame.quads),
            bytes(&frame.shadows),
            bytes(&frame.underlines),
            bytes(&frame.mono),
            bytes(&frame.poly),
            bytes(&frame.composites),
            bytes(&frame.blurs),
        ];
        // wgpu rejects empty bindings, so empty arrays still take 16 bytes.
        let limits = gpu.device.limits();
        let alignment = u64::from(limits.min_storage_buffer_offset_alignment);
        let mut offsets = [0; 7];
        let mut total = 0;
        for (offset, data) in offsets.iter_mut().zip(arrays) {
            *offset = total;
            total = (total + (data.len() as u64).max(16)).next_multiple_of(alignment);
        }
        if total > self.instances.size() {
            let max = limits
                .max_buffer_size
                .min(limits.max_storage_buffer_binding_size)
                .min(MAX_INSTANCE_BUFFER_SIZE);
            ensure!(total <= max, "frame instance data exceeds {max} bytes");
            self.instances = gpu.buffer(
                total.next_power_of_two().min(max),
                wgpu::BufferUsages::STORAGE,
            );
        }
        for (offset, data) in offsets.iter().zip(arrays) {
            if !data.is_empty() {
                gpu.queue.write_buffer(&self.instances, *offset, data);
            }
        }
        Ok(std::array::from_fn(|index| {
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &gpu.layouts.instances,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.instances,
                        offset: offsets[index],
                        size: NonZeroU64::new((arrays[index].len() as u64).max(16)),
                    }),
                }],
            })
        }))
    }

    fn encode(
        &mut self,
        frame: &RenderFrame,
        output: &wgpu::Texture,
        capture: bool,
    ) -> Result<Option<Result<wgpu::Buffer>>> {
        let instances = self.prepare(frame)?;
        let gpu = &self.gpu;
        let frame_view = output.create_view(&Default::default());
        let view = |target: Target| match target {
            Target::Frame => &frame_view,
            Target::Layer(index) => &self.pool[frame.layers[index].texture].1,
        };
        let texture = |target: Target| match target {
            Target::Frame => output,
            Target::Layer(index) => &self.pool[frame.layers[index].texture].0,
        };
        let globals_offset = |target: Target| match target {
            Target::Frame => 0,
            Target::Layer(index) => (index as u64 + 1) * self.globals_stride,
        } as u32;
        let atlas = self.atlas.backend.borrow();
        let pipelines = &self.pipelines;
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let mut pass: Option<wgpu::RenderPass<'static>> = None;
        let mut current = (Target::Frame, true);
        macro_rules! pass {
            () => {{
                if pass.is_none() {
                    let (target, clear) = current;
                    current.1 = false;
                    let mut begun = encoder
                        .begin_render_pass(&wgpu::RenderPassDescriptor {
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: view(target),
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: if clear {
                                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                                    } else {
                                        wgpu::LoadOp::Load
                                    },
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            ..Default::default()
                        })
                        .forget_lifetime();
                    begun.set_bind_group(0, &self.globals_group, &[globals_offset(target)]);
                    pass = Some(begun);
                }
                pass.as_mut().unwrap()
            }};
        }
        for command in &frame.commands {
            match command {
                Command::Pass(target, false)
                    if let Some(pass) = &mut pass
                        && frame.texture(*target) == frame.texture(current.0) =>
                {
                    pass.set_bind_group(0, &self.globals_group, &[globals_offset(*target)]);
                    current = (*target, false);
                }
                // Targets are cleared right away, since a later pass may draw into them.
                Command::Pass(target, clear) => {
                    pass = None;
                    current = (*target, *clear);
                    if *clear {
                        pass!();
                    }
                }
                Command::Quads(range) => {
                    draw(pass!(), &pipelines.quads, &instances[0], None, range)
                }
                Command::Shadows(range) => {
                    draw(pass!(), &pipelines.shadows, &instances[1], None, range)
                }
                Command::Underlines(range) => {
                    draw(pass!(), &pipelines.underlines, &instances[2], None, range)
                }
                Command::Mono(id, range) => {
                    if let Some(texture) = atlas.get(*id) {
                        draw(
                            pass!(),
                            &pipelines.mono,
                            &instances[3],
                            Some(&texture.group),
                            range,
                        );
                    }
                }
                Command::Poly(id, range) => {
                    if let Some(texture) = atlas.get(*id) {
                        draw(
                            pass!(),
                            &pipelines.poly,
                            &instances[4],
                            Some(&texture.group),
                            range,
                        );
                    }
                }
                Command::Blur { src, dst, index } => {
                    pass = None;
                    let saved = current.0;
                    current = (*dst, true);
                    let source = gpu.texture_group(view(*src));
                    draw(
                        pass!(),
                        &pipelines.blurs,
                        &instances[6],
                        Some(&source),
                        &(*index..*index + 1),
                    );
                    pass = None;
                    current.0 = saved;
                }
                Command::Copy { src, dst, region } => {
                    pass = None;
                    let (from, to) = (frame.origin(*src), frame.origin(*dst));
                    encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: texture(*src),
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: (region[0] - from[0]) as u32,
                                y: (region[1] - from[1]) as u32,
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture: texture(*dst),
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: (region[0] - to[0]) as u32,
                                y: (region[1] - to[1]) as u32,
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::Extent3d {
                            width: region[2] as u32,
                            height: region[3] as u32,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                Command::Composite {
                    src,
                    mask,
                    backdrop,
                    index,
                } => {
                    let views = [
                        view(*src),
                        mask.and_then(|id| atlas.get(id))
                            .map_or(&self.empty, |texture| &texture.view),
                        backdrop.map_or(&self.empty, view),
                    ];
                    let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &gpu.layouts.composite,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(views[0]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(views[1]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::TextureView(views[2]),
                            },
                        ],
                    });
                    draw(
                        pass!(),
                        &pipelines.composites,
                        &instances[5],
                        Some(&group),
                        &(*index..*index + 1),
                    );
                }
            }
        }
        drop(pass);
        let readback = capture.then(|| {
            ensure!(
                self.config.usage.contains(wgpu::TextureUsages::COPY_SRC),
                "The window surface can't be read"
            );
            let row = (output.width() * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(row) * u64::from(output.height()),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                output.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: None,
                    },
                },
                output.size(),
            );
            Ok(buffer)
        });
        gpu.queue.submit([encoder.finish()]);
        Ok(readback)
    }
}

fn globals_group(gpu: &Gpu, buffer: &wgpu::Buffer) -> wgpu::BindGroup {
    gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &gpu.layouts.globals,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: NonZeroU64::new(mem::size_of::<Globals>() as u64),
            }),
        }],
    })
}

fn draw(
    pass: &mut wgpu::RenderPass<'static>,
    pipeline: &wgpu::RenderPipeline,
    instances: &wgpu::BindGroup,
    texture: Option<&wgpu::BindGroup>,
    range: &Range<u32>,
) {
    if range.is_empty() {
        return;
    }
    pass.set_pipeline(pipeline);
    pass.set_bind_group(1, instances, &[]);
    if let Some(texture) = texture {
        pass.set_bind_group(2, texture, &[]);
    }
    pass.draw(0..4, range.clone());
}

pub(crate) struct AtlasTextures {
    gpu: Rc<Gpu>,
    mono: TextureList<AtlasTexture>,
    poly: TextureList<AtlasTexture>,
}

struct AtlasTexture {
    id: AtlasTextureId,
    packer: Packer,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    group: wgpu::BindGroup,
}

impl AtlasTextures {
    fn new(gpu: Rc<Gpu>) -> Self {
        Self {
            gpu,
            mono: TextureList::default(),
            poly: TextureList::default(),
        }
    }

    fn list(&mut self, kind: AtlasTextureKind) -> &mut TextureList<AtlasTexture> {
        match kind {
            AtlasTextureKind::Monochrome => &mut self.mono,
            AtlasTextureKind::Polychrome => &mut self.poly,
        }
    }

    fn get(&self, id: AtlasTextureId) -> Option<&AtlasTexture> {
        match id.kind {
            AtlasTextureKind::Monochrome => &self.mono,
            AtlasTextureKind::Polychrome => &self.poly,
        }
        .get(id.index as usize)
    }

    fn allocate(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
    ) -> Result<(AtlasTile, &AtlasTexture)> {
        let list = self.list(kind);
        let mut hit = None;
        for (index, texture) in list.textures.iter_mut().enumerate().rev() {
            if let Some(texture) = texture
                && let Some(tile) = texture.packer.allocate(size)
            {
                hit = Some((index, tile));
                break;
            }
        }
        let (index, (bounds, tile_id)) = match hit {
            Some(hit) => hit,
            None => {
                let texture_size = page_size(size);
                let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: texture_size.width.0 as u32,
                        height: texture_size.height.0 as u32,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: match kind {
                        AtlasTextureKind::Monochrome => wgpu::TextureFormat::R8Unorm,
                        AtlasTextureKind::Polychrome => self.gpu.color_format,
                    },
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                let group = self.gpu.texture_group(&view);
                let list = self.list(kind);
                let index = list.push(|index| AtlasTexture {
                    id: AtlasTextureId {
                        index: index as u32,
                        kind,
                    },
                    packer: Packer::new(texture_size),
                    texture,
                    view,
                    group,
                });
                let tile = list.textures[index]
                    .as_mut()
                    .and_then(|texture| texture.packer.allocate(size))
                    .context("allocating atlas tile")?;
                (index, tile)
            }
        };
        let texture = self.list(kind).textures[index]
            .as_ref()
            .context("atlas texture")?;
        Ok((
            AtlasTile {
                texture_id: texture.id,
                tile_id,
                padding: 0,
                bounds,
            },
            texture,
        ))
    }
}

impl AtlasBackend for AtlasTextures {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile> {
        ensure!(
            size.width.0 > 0 && size.height.0 > 0,
            "atlas tiles must have a size"
        );
        let pixel = if kind == AtlasTextureKind::Monochrome {
            1
        } else {
            4
        };
        let row = size.width.0 as u32 * pixel;
        let bytes = &bytes[..(row * size.height.0 as u32) as usize];
        let rgba;
        let bytes = if kind == AtlasTextureKind::Polychrome
            && self.gpu.color_format == wgpu::TextureFormat::Rgba8Unorm
        {
            rgba = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[b, g, r, a]| [r, g, b, a])
                .collect::<Vec<_>>();
            &rgba[..]
        } else {
            bytes
        };
        let queue = self.gpu.queue.clone();
        let (tile, texture) = self.allocate(kind, size)?;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: tile.bounds.origin.x.0 as u32,
                    y: tile.bounds.origin.y.0 as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: size.width.0 as u32,
                height: size.height.0 as u32,
                depth_or_array_layers: 1,
            },
        );
        Ok(tile)
    }

    fn max_side(&self) -> i32 {
        self.gpu.device.limits().max_texture_dimension_2d as i32
    }

    fn remove(&mut self, tile: AtlasTile) {
        let list = self.list(tile.texture_id.kind);
        let index = tile.texture_id.index as usize;
        let empty = list
            .textures
            .get_mut(index)
            .and_then(Option::as_mut)
            .is_some_and(|texture| texture.packer.free(tile.tile_id));
        if empty {
            list.emptied(index, |texture| texture.packer.is_empty());
        }
    }
}
