use crate::{CompositorGpuHint, DeviceErrorState, WgpuAtlas, WgpuContext};
use anyhow::{Context as _, Result};
use collections::FxHashMap;
use gpui::{
    AtlasTextureId, DevicePixels, GpuSpecs, Scene, Size, get_gamma_correction_ratios,
    render::{Cmd, Globals, RenderFrame, Target, TextParams, bytes},
};
use log::warn;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use smallvec::SmallVec;
use std::{cell::RefCell, mem, num::NonZeroU64, ops::Range, rc::Rc, slice, sync::Arc};

const MAX_INSTANCE_BUFFER_SIZE: u64 = 256 * 1024 * 1024;

const SHADERS: &str = include_str!("shaders.wgsl");

/// Subpixel text needs dual-source blending. The `enable` directive must precede all
/// declarations.
const SUBPIXEL_SHADERS: &str = concat!(
    "enable dual_source_blending;\n",
    include_str!("shaders.wgsl"),
    include_str!("shaders_subpixel.wgsl"),
);

pub struct WgpuSurfaceConfig {
    pub size: Size<DevicePixels>,
    pub transparent: bool,
    /// Preferred presentation mode, `Fifo` when `None` or unsupported.
    pub preferred_present_mode: Option<wgpu::PresentMode>,
}

struct WgpuPipelines {
    quads: wgpu::RenderPipeline,
    shadows: wgpu::RenderPipeline,
    path_rasterization: wgpu::RenderPipeline,
    paths: wgpu::RenderPipeline,
    underlines: wgpu::RenderPipeline,
    mono_sprites: wgpu::RenderPipeline,
    subpixel_sprites: Option<wgpu::RenderPipeline>,
    poly_sprites: wgpu::RenderPipeline,
    composites: wgpu::RenderPipeline,
    blurs: wgpu::RenderPipeline,
}

struct WgpuBindGroupLayouts {
    globals: wgpu::BindGroupLayout,
    instances: wgpu::BindGroupLayout,
    texture: wgpu::BindGroupLayout,
    composite: wgpu::BindGroupLayout,
}

/// Shared GPU context reference, used to coordinate device recovery across multiple windows.
pub type GpuContext = Rc<RefCell<Option<WgpuContext>>>;

/// The window sized texture paths are rasterized into.
struct PathTexture {
    view: wgpu::TextureView,
    msaa_view: Option<wgpu::TextureView>,
    bind_group: wgpu::BindGroup,
    size: [u32; 2],
}

struct CachedTextureBindGroup {
    texture_generation: u64,
    bind_group: wgpu::BindGroup,
}

/// GPU resources that are dropped together during device recovery.
struct WgpuRendererCore {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipelines: WgpuPipelines,
    layouts: WgpuBindGroupLayouts,
    sampler: wgpu::Sampler,
    atlas_bind_groups: FxHashMap<AtlasTextureId, CachedTextureBindGroup>,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    globals_stride: u64,
    instance_buffer: wgpu::Buffer,
    max_instance_size: u64,
    storage_alignment: u64,
    /// Bound in place of a missing mask or backdrop.
    empty: wgpu::TextureView,
    path_texture: Option<PathTexture>,
    /// The offscreen layer textures, kept across frames.
    pool: Vec<(wgpu::Texture, wgpu::TextureView)>,
    frame: RenderFrame,
    atlas: Arc<WgpuAtlas>,
    rendering_params: RenderingParameters,
    is_bgr: bool,
    adapter_info: wgpu::AdapterInfo,
    target_format: wgpu::TextureFormat,
    max_texture_size: u32,
}

pub struct WgpuRenderer {
    context: GpuContext,
    compositor_gpu: Option<CompositorGpuHint>,
    /// `None` while a device recovery has not succeeded yet.
    state: Option<(wgpu::Surface<'static>, WgpuRendererCore)>,
    surface_config: wgpu::SurfaceConfiguration,
    atlas: Arc<WgpuAtlas>,
    transparent_alpha_mode: wgpu::CompositeAlphaMode,
    opaque_alpha_mode: wgpu::CompositeAlphaMode,
    max_texture_size: u32,
    is_bgr: bool,
    failed_frame_count: u32,
    device_errors: Arc<DeviceErrorState>,
    observed_error_generation: u64,
    last_surface_error: Option<String>,
    needs_redraw: bool,
}

impl WgpuRenderer {
    /// Creates a new WgpuRenderer from raw window handles.
    ///
    /// The `gpu_context` is a shared reference that coordinates GPU context across
    /// multiple windows. The first window to create a renderer will initialize the
    /// context; subsequent windows will share it.
    ///
    /// # Safety
    /// The caller must ensure that the window handle remains valid for the lifetime
    /// of the returned renderer.
    pub fn new<W>(
        gpu_context: GpuContext,
        window: &W,
        config: WgpuSurfaceConfig,
        compositor_gpu: Option<CompositorGpuHint>,
    ) -> anyhow::Result<Self>
    where
        W: HasWindowHandle + HasDisplayHandle + std::fmt::Debug + Send + Sync + Clone + 'static,
    {
        let window_handle = window
            .window_handle()
            .map_err(|e| anyhow::anyhow!("Failed to get window handle: {e}"))?;

        // The surface must be created with the same instance that will be used for
        // adapter selection, otherwise wgpu will panic.
        let instance = gpu_context
            .borrow()
            .as_ref()
            .map(|ctx| ctx.instance.clone())
            .unwrap_or_else(|| WgpuContext::instance(Some(Box::new(window.clone()))));
        let surface = create_surface(&instance, window_handle.as_raw())?;

        let mut ctx_ref = gpu_context.borrow_mut();
        let context = match ctx_ref.as_mut() {
            Some(context) => {
                context.check_compatible_with_surface(&surface)?;
                context
            }
            None => ctx_ref.insert(WgpuContext::new(instance, &surface, compositor_gpu)?),
        };

        let atlas = Arc::new(WgpuAtlas::from_context(context));

        Self::new_internal(
            Rc::clone(&gpu_context),
            context,
            surface,
            config,
            compositor_gpu,
            atlas,
        )
    }

    fn new_internal(
        gpu_context: GpuContext,
        context: &WgpuContext,
        surface: wgpu::Surface<'static>,
        config: WgpuSurfaceConfig,
        compositor_gpu: Option<CompositorGpuHint>,
        atlas: Arc<WgpuAtlas>,
    ) -> anyhow::Result<Self> {
        let surface_caps = surface.get_capabilities(&context.adapter);
        let preferred_formats = [
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
        ];
        let surface_format = preferred_formats
            .iter()
            .find(|f| surface_caps.formats.contains(f))
            .copied()
            .or_else(|| {
                surface_caps
                    .formats
                    .iter()
                    .find(|format| !format.is_srgb())
                    .copied()
            })
            .or_else(|| surface_caps.formats.first().copied())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Surface reports no supported texture formats for adapter {:?}",
                    context.adapter.get_info().name
                )
            })?;

        let pick_alpha_mode =
            |preferences: &[wgpu::CompositeAlphaMode]| -> anyhow::Result<wgpu::CompositeAlphaMode> {
                preferences
                    .iter()
                    .find(|p| surface_caps.alpha_modes.contains(p))
                    .copied()
                    .or_else(|| surface_caps.alpha_modes.first().copied())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Surface reports no supported alpha modes for adapter {:?}",
                            context.adapter.get_info().name
                        )
                    })
            };

        let transparent_alpha_mode = pick_alpha_mode(&[
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::Inherit,
        ])?;

        let opaque_alpha_mode = pick_alpha_mode(&[
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::Inherit,
        ])?;

        let alpha_mode = if config.transparent {
            transparent_alpha_mode
        } else {
            opaque_alpha_mode
        };

        let max_texture_size = context.device.limits().max_texture_dimension_2d;

        let requested_width = config.size.width.0 as u32;
        let requested_height = config.size.height.0 as u32;
        let clamped_width = requested_width.min(max_texture_size);
        let clamped_height = requested_height.min(max_texture_size);

        if clamped_width != requested_width || clamped_height != requested_height {
            warn!(
                "Requested surface size ({}, {}) exceeds maximum texture dimension {}. \
                 Clamping to ({}, {}). Window content may not fill the entire window.",
                requested_width, requested_height, max_texture_size, clamped_width, clamped_height
            );
        }

        let surface_config = wgpu::SurfaceConfiguration {
            // Backdrops and blend modes copy from the frame when the surface allows it.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | (surface_caps.usages & wgpu::TextureUsages::COPY_SRC),
            format: surface_format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: clamped_width.max(1),
            height: clamped_height.max(1),
            present_mode: config
                .preferred_present_mode
                .filter(|mode| surface_caps.present_modes.contains(mode))
                .unwrap_or(wgpu::PresentMode::Fifo),
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        // The adapter selection process already validated that this adapter can
        // configure this surface.
        surface.configure(&context.device, &surface_config);

        let core = WgpuRendererCore::new(context, atlas.clone(), surface_format);

        Ok(Self {
            context: gpu_context,
            compositor_gpu,
            state: Some((surface, core)),
            surface_config,
            atlas,
            transparent_alpha_mode,
            opaque_alpha_mode,
            max_texture_size,
            is_bgr: false,
            failed_frame_count: 0,
            device_errors: Arc::clone(context.errors()),
            observed_error_generation: 0,
            last_surface_error: None,
            needs_redraw: false,
        })
    }

    /// Records the new drawable size and reconfigures the surface when one is present.
    /// The size is kept even without GPU resources so that recovery restores it.
    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        let width = size.width.0 as u32;
        let height = size.height.0 as u32;

        if width == self.surface_config.width && height == self.surface_config.height {
            return;
        }

        let clamped_width = width.min(self.max_texture_size);
        let clamped_height = height.min(self.max_texture_size);
        if clamped_width != width || clamped_height != height {
            warn!(
                "Requested surface size ({}, {}) exceeds maximum texture dimension {}. \
                 Clamping to ({}, {}). Window content may not fill the entire window.",
                width, height, self.max_texture_size, clamped_width, clamped_height
            );
        }
        self.surface_config.width = clamped_width.max(1);
        self.surface_config.height = clamped_height.max(1);

        if let Some((surface, core)) = &mut self.state {
            core.path_texture = None;
            surface.configure(&core.device, &self.surface_config);
        }
    }

    pub fn set_subpixel_layout(&mut self, is_bgr: bool) {
        self.is_bgr = is_bgr;
        if let Some((_, core)) = &mut self.state {
            core.is_bgr = is_bgr;
        }
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        let alpha_mode = if transparent {
            self.transparent_alpha_mode
        } else {
            self.opaque_alpha_mode
        };
        if alpha_mode == self.surface_config.alpha_mode {
            return;
        }
        self.surface_config.alpha_mode = alpha_mode;
        if let Some((surface, core)) = &self.state {
            surface.configure(&core.device, &self.surface_config);
        }
    }

    pub fn sprite_atlas(&self) -> &Arc<WgpuAtlas> {
        &self.atlas
    }

    /// Returns `None` while a device recovery has not succeeded yet.
    pub fn gpu_specs(&self) -> Option<GpuSpecs> {
        let adapter_info = &self.state.as_ref()?.1.adapter_info;
        Some(GpuSpecs {
            is_software_emulated: adapter_info.device_type == wgpu::DeviceType::Cpu,
            device_name: adapter_info.name.clone(),
            driver_name: adapter_info.driver.clone(),
            driver_info: adapter_info.driver_info.clone(),
        })
    }

    pub fn max_texture_size(&self) -> u32 {
        self.max_texture_size
    }

    pub fn draw(&mut self, scene: &Scene) -> bool {
        let Some((surface, core)) = &mut self.state else {
            return false;
        };

        if let Some(error) = self.last_surface_error.take().or_else(|| {
            self.device_errors
                .observe_error(&mut self.observed_error_generation)
        }) {
            self.failed_frame_count += 1;
            log::error!(
                "GPU error during frame (failure {} of 10): {error}",
                self.failed_frame_count
            );

            if self.failed_frame_count > 10 {
                panic!("Too many consecutive GPU errors. Last error: {error}");
            } else if self.failed_frame_count > 5 {
                core.path_texture = None;
                core.pool.clear();
                self.atlas.clear();
                self.needs_redraw = true;
                self.failed_frame_count = 0;
                return false;
            }
        } else {
            self.failed_frame_count = 0;
        }

        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                // Textures must be destroyed before the surface can be reconfigured.
                drop(frame);
                surface.configure(&core.device, &self.surface_config);
                return false;
            }
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                surface.configure(&core.device, &self.surface_config);
                return false;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return false;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                self.last_surface_error = Some("Surface texture validation error".to_string());
                return false;
            }
        };

        let copy = self
            .surface_config
            .usage
            .contains(wgpu::TextureUsages::COPY_SRC);
        if let Err(error) = core.render_frame(scene, &frame.texture, copy) {
            log::error!("{error:#}");
            return false;
        }

        core.queue.present(frame);
        true
    }

    /// Returns true if the GPU device was lost and recovery is needed.
    pub fn device_lost(&self) -> bool {
        self.device_errors.device_lost()
    }

    /// Returns true if a redraw is needed because GPU state was cleared.
    /// Calling this method clears the flag.
    pub fn needs_redraw(&mut self) -> bool {
        mem::take(&mut self.needs_redraw)
    }

    /// Recovers from a lost GPU device by recreating the renderer with a new context.
    ///
    /// Call this after detecting `device_lost()` returns true.
    ///
    /// This method coordinates recovery across multiple windows:
    /// - The first window to call this will recreate the shared context
    /// - Subsequent windows will adopt the already-recovered context
    pub fn recover<W>(&mut self, window: &W) -> anyhow::Result<()>
    where
        W: HasWindowHandle + HasDisplayHandle + std::fmt::Debug + Send + Sync + Clone + 'static,
    {
        let gpu_context = Rc::clone(&self.context);

        // Check if another window already recovered the context
        let needs_new_context = gpu_context
            .borrow()
            .as_ref()
            .is_none_or(|ctx| ctx.device_lost());

        let window_handle = window
            .window_handle()
            .map_err(|e| anyhow::anyhow!("Failed to get window handle: {e}"))?;

        let surface = if needs_new_context {
            log::warn!("GPU device lost, recreating context...");

            // Drop old resources to release Arc<Device>/Arc<Queue> and GPU resources
            self.state = None;
            *gpu_context.borrow_mut() = None;

            // Wait briefly for the GPU driver to stabilize, then try to
            // recreate the context without software renderers. If this fails
            // the caller should request another frame and retry — the real GPU
            // may need more time to come back (e.g. after suspend/resume).
            std::thread::sleep(std::time::Duration::from_millis(350));

            let instance = WgpuContext::instance(Some(Box::new(window.clone())));
            let surface = create_surface(&instance, window_handle.as_raw())?;
            let new_context =
                WgpuContext::new_rejecting_software(instance, &surface, self.compositor_gpu)?;
            *gpu_context.borrow_mut() = Some(new_context);
            surface
        } else {
            let ctx_ref = gpu_context.borrow();
            let instance = &ctx_ref.as_ref().unwrap().instance;
            create_surface(instance, window_handle.as_raw())?
        };

        let config = WgpuSurfaceConfig {
            size: Size {
                width: DevicePixels(self.surface_config.width as i32),
                height: DevicePixels(self.surface_config.height as i32),
            },
            transparent: self.surface_config.alpha_mode != wgpu::CompositeAlphaMode::Opaque,
            preferred_present_mode: Some(self.surface_config.present_mode),
        };
        let ctx_ref = gpu_context.borrow();
        let context = ctx_ref.as_ref().expect("context should exist");

        self.state = None;
        self.atlas.handle_device_lost(context);

        let is_bgr = self.is_bgr;
        *self = Self::new_internal(
            Rc::clone(&gpu_context),
            context,
            surface,
            config,
            self.compositor_gpu,
            self.atlas.clone(),
        )?;
        self.set_subpixel_layout(is_bgr);

        log::info!("GPU recovery complete");
        Ok(())
    }
}

impl WgpuRendererCore {
    fn new(
        context: &WgpuContext,
        atlas: Arc<WgpuAtlas>,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        let device = Arc::clone(&context.device);
        let rendering_params = RenderingParameters::new(&context.adapter, target_format);
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
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let layouts = WgpuBindGroupLayouts {
            globals: layout(
                "globals_layout",
                &[entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(mem::size_of::<Globals>() as u64),
                    },
                )],
            ),
            instances: layout(
                "instances_layout",
                &[entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                )],
            ),
            texture: layout("texture_layout", &[texture(0), sampler]),
            composite: layout(
                "composite_layout",
                &[texture(0), sampler, texture(2), texture(3)],
            ),
        };
        let pipelines = Self::create_pipelines(
            &device,
            &layouts,
            target_format,
            rendering_params.path_sample_count,
            context.supports_dual_source_blending(),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let limits = device.limits();
        let globals_stride = (mem::size_of::<Globals>() as u64)
            .next_multiple_of(u64::from(limits.min_uniform_buffer_offset_alignment));
        let globals_buffer = Self::create_buffer(
            &device,
            "globals_buffer",
            globals_stride * 8,
            wgpu::BufferUsages::UNIFORM,
        );
        let globals_bind_group = Self::globals_bind_group(&device, &layouts, &globals_buffer);
        let max_instance_size = limits
            .max_buffer_size
            .min(limits.max_storage_buffer_binding_size)
            .min(MAX_INSTANCE_BUFFER_SIZE);
        let instance_buffer = Self::create_buffer(
            &device,
            "instance_buffer",
            (2 * 1024 * 1024).min(max_instance_size),
            wgpu::BufferUsages::STORAGE,
        );
        let empty = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("empty"),
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

        Self {
            queue: Arc::clone(&context.queue),
            pipelines,
            layouts,
            sampler,
            atlas_bind_groups: FxHashMap::default(),
            globals_buffer,
            globals_bind_group,
            globals_stride,
            instance_buffer,
            max_instance_size,
            storage_alignment: u64::from(limits.min_storage_buffer_offset_alignment),
            empty,
            path_texture: None,
            pool: Vec::new(),
            frame: RenderFrame::default(),
            atlas,
            rendering_params,
            is_bgr: false,
            adapter_info: context.adapter.get_info(),
            target_format,
            max_texture_size: limits.max_texture_dimension_2d,
            device,
        }
    }

    fn create_pipelines(
        device: &wgpu::Device,
        layouts: &WgpuBindGroupLayouts,
        format: wgpu::TextureFormat,
        path_sample_count: u32,
        dual_source_blending: bool,
    ) -> WgpuPipelines {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpui_shaders"),
            source: wgpu::ShaderSource::Wgsl(SHADERS.into()),
        });
        let subpixel_module = (dual_source_blending
            && device
                .features()
                .contains(wgpu::Features::DUAL_SOURCE_BLENDING))
        .then(|| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("gpui_subpixel_shaders"),
                source: wgpu::ShaderSource::Wgsl(SUBPIXEL_SHADERS.into()),
            })
        });

        let pipeline = |name: &str,
                        module: &wgpu::ShaderModule,
                        texture: Option<&wgpu::BindGroupLayout>,
                        target: wgpu::ColorTargetState,
                        topology: wgpu::PrimitiveTopology,
                        sample_count: u32| {
            let mut bind_group_layouts = vec![Some(&layouts.globals), Some(&layouts.instances)];
            bind_group_layouts.extend(texture.map(Some));
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(name),
                bind_group_layouts: &bind_group_layouts,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(name),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(&format!("vs_{name}")),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(&format!("fs_{name}")),
                    targets: &[Some(target)],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let target = |blend| wgpu::ColorTargetState {
            format,
            blend,
            write_mask: wgpu::ColorWrites::ALL,
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
        let premultiplied = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let strip = wgpu::PrimitiveTopology::TriangleStrip;
        let texture = Some(&layouts.texture);

        WgpuPipelines {
            quads: pipeline("quad", &module, None, target(over), strip, 1),
            shadows: pipeline("shadow", &module, None, target(over), strip, 1),
            path_rasterization: pipeline(
                "path_rasterization",
                &module,
                None,
                target(premultiplied),
                wgpu::PrimitiveTopology::TriangleList,
                path_sample_count,
            ),
            paths: pipeline("path", &module, texture, target(premultiplied), strip, 1),
            underlines: pipeline("underline", &module, None, target(over), strip, 1),
            mono_sprites: pipeline("mono_sprite", &module, texture, target(over), strip, 1),
            subpixel_sprites: subpixel_module.map(|module| {
                let blend = wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Src1,
                        dst_factor: wgpu::BlendFactor::OneMinusSrc1,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                };
                let target = wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::COLOR,
                };
                pipeline("subpixel_sprite", &module, texture, target, strip, 1)
            }),
            poly_sprites: pipeline("poly_sprite", &module, texture, target(over), strip, 1),
            composites: pipeline(
                "composite",
                &module,
                Some(&layouts.composite),
                target(premultiplied),
                strip,
                1,
            ),
            blurs: pipeline("blur", &module, texture, target(None), strip, 1),
        }
    }

    fn create_buffer(
        device: &wgpu::Device,
        label: &str,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn globals_bind_group(
        device: &wgpu::Device,
        layouts: &WgpuBindGroupLayouts,
        buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals_bind_group"),
            layout: &layouts.globals,
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

    fn texture_bind_group(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("texture_bind_group"),
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

    fn render_frame(&mut self, scene: &Scene, output: &wgpu::Texture, copy: bool) -> Result<()> {
        self.atlas.before_frame();
        let mut frame = mem::take(&mut self.frame);
        frame.record(
            scene,
            [output.width(), output.height()],
            self.max_texture_size,
            copy,
        );
        let result = self.encode(&frame, output);
        self.frame = frame;
        if result.is_err() {
            // Queue writes are staged before encoding; flush them even if the frame fails.
            self.queue.submit(std::iter::empty());
        }
        result
    }

    /// Uploads the frame data and returns the instance bind groups, in the order of the arrays.
    fn prepare(&mut self, frame: &RenderFrame, size: [u32; 2]) -> Result<[wgpu::BindGroup; 10]> {
        let device = &self.device;
        if self
            .path_texture
            .as_ref()
            .is_none_or(|path| path.size != size)
        {
            let samples = self.rendering_params.path_sample_count;
            let texture = |sample_count, usage| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("path_texture"),
                        size: wgpu::Extent3d {
                            width: size[0],
                            height: size[1],
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count,
                        dimension: wgpu::TextureDimension::D2,
                        format: self.target_format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            let view = texture(1, wgpu::TextureUsages::TEXTURE_BINDING);
            self.path_texture = Some(PathTexture {
                msaa_view: (samples > 1).then(|| texture(samples, wgpu::TextureUsages::empty())),
                bind_group: self.texture_bind_group(&view),
                view,
                size,
            });
        }

        self.pool.truncate(frame.textures.len());
        for (index, size) in frame.textures.iter().enumerate() {
            if self
                .pool
                .get(index)
                .is_some_and(|(texture, _)| [texture.width(), texture.height()] == *size)
            {
                continue;
            }
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("layer_texture"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            if index < self.pool.len() {
                self.pool[index] = (texture, view);
            } else {
                self.pool.push((texture, view));
            }
        }

        // One globals entry for the window, then one for each offscreen layer.
        let text = TextParams {
            gamma_ratios: self.rendering_params.gamma_ratios,
            grayscale_enhanced_contrast: self.rendering_params.grayscale_enhanced_contrast,
            subpixel_enhanced_contrast: self.rendering_params.subpixel_enhanced_contrast,
            is_bgr: self.is_bgr,
        };
        let stride = self.globals_stride as usize;
        let mut globals = vec![0; stride * (frame.layers.len() + 1)];
        for (index, target) in std::iter::once(Target::Frame)
            .chain((0..frame.layers.len()).map(Target::Layer))
            .enumerate()
        {
            globals[index * stride..][..mem::size_of::<Globals>()]
                .copy_from_slice(bytes(slice::from_ref(&frame.globals(target, &text))));
        }
        if globals.len() as u64 > self.globals_buffer.size() {
            self.globals_buffer = Self::create_buffer(
                device,
                "globals_buffer",
                (globals.len() as u64).next_power_of_two(),
                wgpu::BufferUsages::UNIFORM,
            );
            self.globals_bind_group =
                Self::globals_bind_group(device, &self.layouts, &self.globals_buffer);
        }
        self.queue.write_buffer(&self.globals_buffer, 0, &globals);

        let arrays: [&[u8]; 10] = [
            bytes(&frame.quads),
            bytes(&frame.shadows),
            bytes(&frame.vertices),
            bytes(&frame.sprites),
            bytes(&frame.underlines),
            bytes(&frame.mono),
            bytes(&frame.subpixel),
            bytes(&frame.poly),
            bytes(&frame.composites),
            bytes(&frame.blurs),
        ];
        // wgpu rejects zero-sized bindings, so empty arrays still reserve 16 bytes.
        let mut offsets = [0; 10];
        let mut total = 0;
        for (offset, data) in offsets.iter_mut().zip(arrays) {
            *offset = total;
            total = (total + (data.len() as u64).max(16)).next_multiple_of(self.storage_alignment);
        }
        if total > self.instance_buffer.size() {
            anyhow::ensure!(
                total <= self.max_instance_size,
                "scene too large: {total} bytes of instance data, above the maximum of {}",
                self.max_instance_size
            );
            self.instance_buffer = Self::create_buffer(
                device,
                "instance_buffer",
                total.next_power_of_two().min(self.max_instance_size),
                wgpu::BufferUsages::STORAGE,
            );
        }
        for (offset, data) in offsets.iter().zip(arrays) {
            if !data.is_empty() {
                self.queue
                    .write_buffer(&self.instance_buffer, *offset, data);
            }
        }

        let mut texture_ids = SmallVec::<[AtlasTextureId; 8]>::new();
        for cmd in &frame.cmds {
            if let Cmd::Mono(id, _) | Cmd::Subpixel(id, _) | Cmd::Poly(id, _) = cmd
                && !texture_ids.contains(id)
            {
                texture_ids.push(*id);
            }
        }
        self.atlas_bind_groups
            .retain(|id, _| texture_ids.contains(id));
        for id in texture_ids {
            // The atlas has released this texture; its sprites belong to a stale paint.
            let Some(info) = self.atlas.get_texture_info(id) else {
                self.atlas_bind_groups.remove(&id);
                continue;
            };
            if self
                .atlas_bind_groups
                .get(&id)
                .is_some_and(|cached| cached.texture_generation == info.generation)
            {
                continue;
            }
            let bind_group = self.texture_bind_group(&info.view);
            self.atlas_bind_groups.insert(
                id,
                CachedTextureBindGroup {
                    texture_generation: info.generation,
                    bind_group,
                },
            );
        }

        Ok(std::array::from_fn(|index| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("instances_bind_group"),
                layout: &self.layouts.instances,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.instance_buffer,
                        offset: offsets[index],
                        size: NonZeroU64::new((arrays[index].len() as u64).max(16)),
                    }),
                }],
            })
        }))
    }

    fn encode(&mut self, frame: &RenderFrame, output: &wgpu::Texture) -> Result<()> {
        let instances = self.prepare(frame, [output.width(), output.height()])?;
        let path = self.path_texture.as_ref().context("missing path texture")?;
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
        let pipelines = &self.pipelines;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("main_encoder"),
            });
        let mut pass: Option<wgpu::RenderPass<'static>> = None;
        let mut current = (Target::Frame, true);
        // Begins a pass on the current target when none is open.
        macro_rules! pass {
            () => {{
                if pass.is_none() {
                    let (target, clear) = current;
                    current.1 = false;
                    let mut begun = encoder
                        .begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("main_pass"),
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
                    begun.set_bind_group(0, &self.globals_bind_group, &[globals_offset(target)]);
                    pass = Some(begun);
                }
                pass.as_mut().unwrap()
            }};
        }

        for cmd in &frame.cmds {
            match cmd {
                // Targets are cleared right away, since a later pass may draw into them.
                Cmd::Pass(target, clear) => {
                    pass = None;
                    current = (*target, *clear);
                    if *clear {
                        pass!();
                    }
                }
                Cmd::Quads(range) => draw(pass!(), &pipelines.quads, &instances[0], None, range),
                Cmd::Shadows(range) => {
                    draw(pass!(), &pipelines.shadows, &instances[1], None, range)
                }
                Cmd::Underlines(range) => {
                    draw(pass!(), &pipelines.underlines, &instances[4], None, range)
                }
                Cmd::Mono(id, range) | Cmd::Subpixel(id, range) | Cmd::Poly(id, range) => {
                    let Some(atlas) = self.atlas_bind_groups.get(id) else {
                        continue;
                    };
                    let (pipeline, data) = match cmd {
                        Cmd::Mono(..) => (&pipelines.mono_sprites, &instances[5]),
                        // WARNING: subpixel text writes no alpha, so it would vanish in a transparent layer.
                        Cmd::Subpixel(..) => (
                            pipelines
                                .subpixel_sprites
                                .as_ref()
                                .filter(|_| current.0 == Target::Frame)
                                .unwrap_or(&pipelines.mono_sprites),
                            &instances[6],
                        ),
                        _ => (&pipelines.poly_sprites, &instances[7]),
                    };
                    draw(pass!(), pipeline, data, Some(&atlas.bind_group), range);
                }
                Cmd::Paths { vertices, sprites } => {
                    pass = None;
                    let mut raster = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("path_rasterization_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: path.msaa_view.as_ref().unwrap_or(&path.view),
                            depth_slice: None,
                            resolve_target: path.msaa_view.as_ref().map(|_| &path.view),
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                    raster.set_pipeline(&pipelines.path_rasterization);
                    raster.set_bind_group(0, &self.globals_bind_group, &[0]);
                    raster.set_bind_group(1, &instances[2], &[]);
                    raster.draw(vertices.clone(), 0..1);
                    drop(raster);
                    draw(
                        pass!(),
                        &pipelines.paths,
                        &instances[3],
                        Some(&path.bind_group),
                        sprites,
                    );
                }
                Cmd::Blur { src, dst, index } => {
                    pass = None;
                    let saved = current.0;
                    current = (*dst, true);
                    let source = self.texture_bind_group(view(*src));
                    draw(
                        pass!(),
                        &pipelines.blurs,
                        &instances[9],
                        Some(&source),
                        &(*index..*index + 1),
                    );
                    pass = None;
                    current.0 = saved;
                }
                Cmd::Copy { src, dst, region } => {
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
                Cmd::Composite {
                    src,
                    mask,
                    backdrop,
                    index,
                } => {
                    let mask = mask.and_then(|id| self.atlas.get_texture_info(id));
                    let views = [
                        view(*src),
                        mask.as_ref().map_or(&self.empty, |info| &info.view),
                        backdrop.map_or(&self.empty, view),
                    ];
                    let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("composite_bind_group"),
                        layout: &self.layouts.composite,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(views[0]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
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
                        &instances[8],
                        Some(&group),
                        &(*index..*index + 1),
                    );
                }
            }
        }
        drop(pass);
        self.queue.submit([encoder.finish()]);
        Ok(())
    }
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

fn create_surface(
    instance: &wgpu::Instance,
    raw_window_handle: raw_window_handle::RawWindowHandle,
) -> anyhow::Result<wgpu::Surface<'static>> {
    // SAFETY: the platform window owns the renderer and drops the surface before the window.
    unsafe {
        instance
            .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                // Fall back to the display handle already provided via InstanceDescriptor::display.
                raw_display_handle: None,
                raw_window_handle,
            })
            .map_err(|e| anyhow::anyhow!("Failed to create surface: {e}"))
    }
}

struct RenderingParameters {
    path_sample_count: u32,
    gamma_ratios: [f32; 4],
    grayscale_enhanced_contrast: f32,
    subpixel_enhanced_contrast: f32,
}

impl RenderingParameters {
    fn new(adapter: &wgpu::Adapter, surface_format: wgpu::TextureFormat) -> Self {
        use std::env;

        let format_features = adapter.get_texture_format_features(surface_format);
        let path_sample_count = [4, 2, 1]
            .into_iter()
            .find(|&n| format_features.flags.sample_count_supported(n))
            .unwrap_or(1);

        let gamma = env::var("ZED_FONTS_GAMMA")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.8_f32)
            .clamp(1.0, 2.2);
        let gamma_ratios = get_gamma_correction_ratios(gamma);

        let grayscale_enhanced_contrast = env::var("ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0_f32)
            .max(0.0);

        let subpixel_enhanced_contrast = env::var("ZED_FONTS_SUBPIXEL_ENHANCED_CONTRAST")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.5_f32)
            .max(0.0);

        Self {
            path_sample_count,
            gamma_ratios,
            grayscale_enhanced_contrast,
            subpixel_enhanced_contrast,
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        MonochromeSprite, PolychromeSprite, Quad, Shadow, SubpixelSprite, Underline,
        render::{Blur, Composite, Globals, PathRasterizationVertex, PathSprite},
    };
    use std::mem::size_of;

    #[test]
    fn shaders_match_instance_layouts() {
        for source in [super::SHADERS, super::SUBPIXEL_SHADERS] {
            let module = naga::front::wgsl::parse_str(source).unwrap();
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap();
            let mut layouter = naga::proc::Layouter::default();
            layouter.update(module.to_ctx()).unwrap();
            for (name, size) in [
                ("Globals", size_of::<Globals>()),
                ("Quad", size_of::<Quad>()),
                ("Shadow", size_of::<Shadow>()),
                (
                    "PathRasterizationVertex",
                    size_of::<PathRasterizationVertex>(),
                ),
                ("PathSprite", size_of::<PathSprite>()),
                ("Underline", size_of::<Underline>()),
                ("MonochromeSprite", size_of::<MonochromeSprite>()),
                ("SubpixelSprite", size_of::<SubpixelSprite>()),
                ("PolychromeSprite", size_of::<PolychromeSprite>()),
                ("Composite", size_of::<Composite>()),
                ("Blur", size_of::<Blur>()),
            ] {
                let Some((handle, _)) = module
                    .types
                    .iter()
                    .find(|(_, ty)| ty.name.as_deref() == Some(name))
                else {
                    assert_eq!(name, "SubpixelSprite", "missing struct {name}");
                    continue;
                };
                assert_eq!(layouter[handle].size as usize, size, "{name}");
            }
        }
    }
}
