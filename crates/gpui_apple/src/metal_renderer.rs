use crate::metal_atlas::MetalAtlas;
use anyhow::{Context as _, Result};
use block2::RcBlock;
use gpui::{
    DevicePixels, Scene, Size,
    render::{Cmd, Globals, RenderFrame, Target, TextParams, bytes},
};
use objc2::{Message, rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::CGSize;
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceDisplayP3};
use objc2_foundation::{NSRange, NSString};
#[cfg(target_os = "macos")]
use objc2_metal::MTLCopyAllDevices;
use objc2_metal::{
    MTLBlendFactor, MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLCommandBuffer,
    MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLDrawable,
    MTLGPUFamily, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLResourceOptions, MTLSize, MTLStorageMode, MTLStoreAction,
    MTLTexture, MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use parking_lot::Mutex;

use std::{cell::Cell, ffi::c_void, mem, mem::MaybeUninit, ops::Range, ptr, slice, sync::Arc};

const FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;
// The smallest maximum texture size of the Metal GPU families on macOS.
const MAX_TEXTURE_SIZE: u32 = 16384;
// Use 4x MSAA, all devices support it.
// https://developer.apple.com/documentation/metal/mtldevice/1433355-supportstexturesamplecount
const PATH_SAMPLE_COUNT: u32 = 4;
/// Metal requires the offset a buffer is bound at to be 256-byte aligned.
const INSTANCE_BUFFER_ALIGNMENT: usize = 256;
const MAX_INSTANCE_BUFFER_SIZE: usize = 256 * 1024 * 1024;

pub type Context = Arc<Mutex<InstanceBufferPool>>;
pub type Renderer = MetalRenderer;

pub fn new_renderer(
    context: self::Context,
    transparent: bool,
    use_display_p3: bool,
) -> Result<Renderer> {
    MetalRenderer::new(context, transparent, use_display_p3)
}

pub struct InstanceBufferPool {
    buffer_size: usize,
    buffers: Vec<InstanceBuffer>,
}

impl Default for InstanceBufferPool {
    fn default() -> Self {
        Self {
            buffer_size: 2 * 1024 * 1024,
            buffers: Vec::new(),
        }
    }
}

struct InstanceBuffer {
    metal_buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    size: usize,
}

// SAFETY: Each buffer has one CPU writer and returns to the mutex-protected pool only after GPU completion.
unsafe impl Send for InstanceBuffer {}

impl InstanceBufferPool {
    pub(crate) fn reset(&mut self, buffer_size: usize) {
        self.buffer_size = buffer_size;
        self.buffers.clear();
    }

    fn acquire(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        unified_memory: bool,
    ) -> Result<InstanceBuffer> {
        if let Some(buffer) = self.buffers.pop() {
            return Ok(buffer);
        }
        let options = if unified_memory {
            MTLResourceOptions::StorageModeShared
                // Buffers are write only which can benefit from the combined cache
                // https://developer.apple.com/documentation/metal/mtlresourceoptions/cpucachemodewritecombined
                | MTLResourceOptions::CPUCacheModeWriteCombined
        } else {
            MTLResourceOptions::StorageModeManaged
        };
        let buffer = device
            .newBufferWithLength_options(self.buffer_size, options)
            .with_context(|| format!("allocating {}-byte instance buffer", self.buffer_size))?;
        Ok(InstanceBuffer {
            metal_buffer: buffer,
            size: self.buffer_size,
        })
    }

    fn release(&mut self, buffer: InstanceBuffer) {
        if buffer.size == self.buffer_size {
            self.buffers.push(buffer)
        }
    }
}

pub struct MetalRenderer {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    layer: Option<Retained<CAMetalLayer>>,
    is_apple_gpu: bool,
    is_unified_memory: bool,
    presents_with_transaction: bool,
    /// For headless rendering, tracks whether output should be opaque
    opaque: bool,
    command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipelines: Pipelines,
    instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    sprite_atlas: Arc<MetalAtlas>,
    path_intermediate_texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    path_intermediate_msaa_texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    /// The offscreen layer textures, kept across frames.
    pool: Vec<Retained<ProtocolObject<dyn MTLTexture>>>,
    frame: RenderFrame,
}

enum Blend {
    Over,
    Premultiplied,
    Replace,
}

struct Pipelines {
    quads: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    shadows: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    path_rasterization: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    path_sprites: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    underlines: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    mono_sprites: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    poly_sprites: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    composites: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    blurs: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

impl Pipelines {
    fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self> {
        #[cfg(not(gpui_shaders))]
        let library = device
            .newLibraryWithSource_options_error(
                &NSString::from_str(include_str!("shaders.metal")),
                Some(&objc2_metal::MTLCompileOptions::new()),
            )
            .context("building Metal library")?;
        #[cfg(gpui_shaders)]
        let library = device
            .newLibraryWithData_error(&dispatch2::DispatchData::from_static_bytes(include_bytes!(
                concat!(env!("OUT_DIR"), "/shaders.metallib")
            )))
            .context("loading Metal library")?;
        let pipeline = |name: &str, blend: Blend, samples: usize| -> Result<_> {
            let function = |stage: &str| {
                let function_name = format!("{name}_{stage}");
                library
                    .newFunctionWithName(&NSString::from_str(&function_name))
                    .with_context(|| format!("locating function {function_name}"))
            };
            let descriptor = MTLRenderPipelineDescriptor::new();
            descriptor.setLabel(Some(&NSString::from_str(name)));
            descriptor.setVertexFunction(Some(&*function("vertex")?));
            descriptor.setFragmentFunction(Some(&*function("fragment")?));
            descriptor.setRasterSampleCount(samples);
            // SAFETY: Index zero is within the pipeline's color attachment array.
            let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
            attachment.setPixelFormat(FORMAT);
            // Colors are drawn unpremultiplied over targets that hold premultiplied colors.
            let source = match blend {
                Blend::Over => Some(MTLBlendFactor::SourceAlpha),
                Blend::Premultiplied => Some(MTLBlendFactor::One),
                Blend::Replace => None,
            };
            if let Some(source) = source {
                attachment.setBlendingEnabled(true);
                attachment.setSourceRGBBlendFactor(source);
                attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            }
            device
                .newRenderPipelineStateWithDescriptor_error(&descriptor)
                .with_context(|| format!("creating {name} render pipeline"))
        };
        Ok(Self {
            quads: pipeline("quad", Blend::Over, 1)?,
            shadows: pipeline("shadow", Blend::Over, 1)?,
            path_rasterization: pipeline(
                "path_rasterization",
                Blend::Premultiplied,
                PATH_SAMPLE_COUNT as usize,
            )?,
            path_sprites: pipeline("path_sprite", Blend::Premultiplied, 1)?,
            underlines: pipeline("underline", Blend::Over, 1)?,
            mono_sprites: pipeline("monochrome_sprite", Blend::Over, 1)?,
            poly_sprites: pipeline("polychrome_sprite", Blend::Over, 1)?,
            composites: pipeline("composite", Blend::Premultiplied, 1)?,
            blurs: pipeline("blur", Blend::Replace, 1)?,
        })
    }
}

impl MetalRenderer {
    /// Creates a new MetalRenderer with a CAMetalLayer for window-based rendering.
    pub fn new(
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
        transparent: bool,
        use_display_p3: bool,
    ) -> Result<Self> {
        let device = Self::create_device()?;
        let layer = CAMetalLayer::new();
        Self::configure_layer(&layer, &device, transparent, use_display_p3);
        Self::new_internal(device, Some(layer), !transparent, instance_buffer_pool)
    }

    /// Creates a renderer for a CAMetalLayer owned by a platform view, such as
    /// the backing layer UIKit creates for a view whose `layerClass` is
    /// `CAMetalLayer`. The renderer retains the layer.
    pub fn from_layer(
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
        layer: &CAMetalLayer,
        transparent: bool,
    ) -> Result<Self> {
        let device = Self::create_device()?;
        let layer = layer.retain();
        Self::configure_layer(&layer, &device, transparent, false);
        Self::new_internal(device, Some(layer), !transparent, instance_buffer_pool)
    }

    fn configure_layer(
        layer: &CAMetalLayer,
        device: &ProtocolObject<dyn MTLDevice>,
        transparent: bool,
        use_display_p3: bool,
    ) {
        layer.setDevice(Some(device));
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        if use_display_p3 {
            // SAFETY: The constant is extern, from Core Graphics
            let display_p3_name = unsafe { kCGColorSpaceDisplayP3 };
            if let Some(display_p3_color_space) = CGColorSpace::with_name(Some(display_p3_name)) {
                layer.setColorspace(Some(&display_p3_color_space));
            } else {
                log::error!("unable to create the Display P3 color space");
            }
        }
        // Support direct-to-display rendering if the window is not transparent
        // https://developer.apple.com/documentation/metal/managing-your-game-window-for-metal-in-macos
        layer.setOpaque(!transparent);
        layer.setMaximumDrawableCount(3);
        // Backdrop filters and blend modes copy from the drawable.
        layer.setFramebufferOnly(false);
        // Allow texture reading for visual tests and UI automation screenshots
        // (captures without ScreenCaptureKit). Only debug builds pay the
        // presentation cost, even when `test-support` is compiled in.
        layer.setAllowsNextDrawableTimeout(false);
        layer.setNeedsDisplayOnBoundsChange(true);
        // UIKit sizes a view's backing layer itself; only AppKit-hosted
        // layers need to track their superlayer's bounds.
        #[cfg(target_os = "macos")]
        layer.setAutoresizingMask(
            objc2_quartz_core::CAAutoresizingMask::LayerWidthSizable
                | objc2_quartz_core::CAAutoresizingMask::LayerHeightSizable,
        );
    }

    #[cfg(target_os = "macos")]
    fn create_device() -> Result<Retained<ProtocolObject<dyn MTLDevice>>> {
        // Prefer low‐power integrated GPUs on Intel Mac. On Apple
        // Silicon, there is only ever one GPU, so this is equivalent to
        // `metal::Device::system_default()`.
        if let Some(device) = MTLCopyAllDevices()
            .iter()
            .min_by_key(|device| (device.isRemovable(), !device.isLowPower()))
        {
            Ok(device)
        } else {
            // For some reason `all()` can return an empty list, see https://github.com/zed-industries/zed/issues/37689
            // In that case, we fall back to the system default device.
            log::error!(
                "Unable to enumerate Metal devices; attempting to use system default device"
            );
            Self::system_default_device()
        }
    }

    fn system_default_device() -> Result<Retained<ProtocolObject<dyn MTLDevice>>> {
        MTLCreateSystemDefaultDevice().context("accessing a compatible Metal device")
    }

    fn new_internal(
        device: Retained<ProtocolObject<dyn MTLDevice>>,
        layer: Option<Retained<CAMetalLayer>>,
        opaque: bool,
        instance_buffer_pool: Arc<Mutex<InstanceBufferPool>>,
    ) -> Result<Self> {
        // Shared memory can be used only if CPU and GPU share the same memory space.
        // https://developer.apple.com/documentation/metal/setting-resource-storage-modes
        // iOS does not support managed resources. Its simulator may report a
        // non-unified host GPU even though resources must still use shared storage.
        let is_unified_memory = device.hasUnifiedMemory();
        // Apple GPU families support memoryless textures, which can significantly reduce
        // memory usage by keeping render targets in on-chip tile memory instead of
        // allocating backing store in system memory.
        // https://developer.apple.com/documentation/metal/mtlgpufamily
        let is_apple_gpu = device.supportsFamily(MTLGPUFamily::Apple1);

        let command_queue = device.newCommandQueue().context("creating command queue")?;
        let supports_shared_storage = is_apple_gpu;
        #[expect(
            clippy::arc_with_non_send_sync,
            reason = "PlatformWindow::sprite_atlas requires Arc, not Send or Sync"
        )]
        let sprite_atlas = Arc::new(MetalAtlas::new(device.clone(), supports_shared_storage));
        let pipelines = Pipelines::new(&device)?;
        Ok(Self {
            device,
            layer,
            presents_with_transaction: false,
            is_apple_gpu,
            is_unified_memory,
            opaque,
            pipelines,
            command_queue,
            instance_buffer_pool,
            sprite_atlas,
            path_intermediate_texture: None,
            path_intermediate_msaa_texture: None,
            pool: Vec::new(),
            frame: RenderFrame::default(),
        })
    }

    pub fn layer(&self) -> Option<&CAMetalLayer> {
        self.layer.as_deref()
    }

    pub fn layer_ptr(&self) -> *mut CAMetalLayer {
        self.layer
            .as_ref()
            .map(|layer| Retained::as_ptr(layer).cast_mut())
            .unwrap_or(ptr::null_mut())
    }

    pub fn sprite_atlas(&self) -> &Arc<MetalAtlas> {
        &self.sprite_atlas
    }

    pub fn set_presents_with_transaction(&mut self, presents_with_transaction: bool) {
        self.presents_with_transaction = presents_with_transaction;
        if let Some(layer) = &self.layer {
            layer.setPresentsWithTransaction(presents_with_transaction);
        }
    }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        if let Some(layer) = &self.layer {
            layer.setDrawableSize(CGSize::new(size.width.0 as f64, size.height.0 as f64));
        }
        if let Err(error) = self.update_path_intermediate_textures(size) {
            log::error!("failed to resize path textures: {error:#}");
        }
    }

    fn update_path_intermediate_textures(&mut self, size: Size<DevicePixels>) -> Result<()> {
        // We are uncertain when this happens, but sometimes size can be 0 here. Most likely before
        // the layout pass on window creation. Zero-sized texture creation causes SIGABRT.
        // https://github.com/zed-industries/zed/issues/36229
        self.path_intermediate_texture = None;
        self.path_intermediate_msaa_texture = None;
        if size.width.0 <= 0 || size.height.0 <= 0 {
            return Ok(());
        }

        let texture_descriptor = MTLTextureDescriptor::new();
        // SAFETY: The dimensions are positive and this descriptor creates a 2D texture.
        unsafe {
            texture_descriptor.setWidth(size.width.0 as usize);
            texture_descriptor.setHeight(size.height.0 as usize);
        }
        texture_descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        texture_descriptor.setStorageMode(MTLStorageMode::Private);
        texture_descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        let intermediate_texture = self
            .device
            .newTextureWithDescriptor(&texture_descriptor)
            .context("allocating path intermediate texture")?;

        let msaa_texture = if PATH_SAMPLE_COUNT > 1 {
            // https://developer.apple.com/documentation/metal/choosing-a-resource-storage-mode-for-apple-gpus
            // Rendering MSAA textures are done in a single pass, so we can use memory-less storage on Apple Silicon
            let storage_mode = if self.is_apple_gpu {
                MTLStorageMode::Memoryless
            } else {
                MTLStorageMode::Private
            };

            let msaa_descriptor = texture_descriptor;
            msaa_descriptor.setTextureType(MTLTextureType::Type2DMultisample);
            msaa_descriptor.setStorageMode(storage_mode);
            // SAFETY: The multisample texture uses the same sample count as its render pipeline.
            unsafe {
                msaa_descriptor.setSampleCount(PATH_SAMPLE_COUNT as _);
            }
            Some(
                self.device
                    .newTextureWithDescriptor(&msaa_descriptor)
                    .context("allocating multisample path texture")?,
            )
        } else {
            None
        };
        self.path_intermediate_texture = Some(intermediate_texture);
        self.path_intermediate_msaa_texture = msaa_texture;
        Ok(())
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        self.opaque = !transparent;
        if let Some(layer) = &self.layer {
            layer.setOpaque(!transparent);
        }
    }

    pub fn destroy(&self) {
        // nothing to do
    }

    pub fn draw(&mut self, scene: &Scene) {
        let Some(layer) = self.layer.clone() else {
            return;
        };
        let drawable_size = layer.drawableSize();
        let window = [
            drawable_size.width.ceil() as u32,
            drawable_size.height.ceil() as u32,
        ];
        if window[0] == 0 || window[1] == 0 {
            return;
        }
        if self.path_intermediate_texture.is_none()
            && let Err(error) = self.update_path_intermediate_textures(Size {
                width: DevicePixels(window[0] as i32),
                height: DevicePixels(window[1] as i32),
            })
        {
            return log::error!("failed to create path textures: {error:#}");
        }
        let Some(drawable) = layer.nextDrawable() else {
            return log::error!("failed to retrieve next drawable, drawable size: {window:?}");
        };
        let command_buffer = match self.render_frame(scene, &drawable.texture(), window) {
            Ok(command_buffer) => command_buffer,
            Err(error) => return log::error!("failed to render: {error:#}"),
        };
        if self.presents_with_transaction {
            command_buffer.commit();
            command_buffer.waitUntilScheduled();
            drawable.present();
        } else {
            command_buffer.presentDrawable(ProtocolObject::from_ref(&*drawable));
            command_buffer.commit();
        }
    }

    fn render_frame(
        &mut self,
        scene: &Scene,
        drawable: &ProtocolObject<dyn MTLTexture>,
        window: [u32; 2],
    ) -> Result<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        let mut frame = mem::take(&mut self.frame);
        frame.record(scene, window, MAX_TEXTURE_SIZE, true);
        self.pool.truncate(frame.textures.len());
        for (index, size) in frame.textures.iter().enumerate() {
            if self
                .pool
                .get(index)
                .is_some_and(|texture| [texture.width() as u32, texture.height() as u32] == *size)
            {
                continue;
            }
            let descriptor = MTLTextureDescriptor::new();
            // SAFETY: The dimensions are positive and this descriptor creates a 2D texture.
            unsafe {
                descriptor.setWidth(size[0] as usize);
                descriptor.setHeight(size[1] as usize);
            }
            descriptor.setPixelFormat(FORMAT);
            descriptor.setStorageMode(MTLStorageMode::Private);
            descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            let texture = self
                .device
                .newTextureWithDescriptor(&descriptor)
                .context("allocating layer texture")?;
            if index < self.pool.len() {
                self.pool[index] = texture;
            } else {
                self.pool.push(texture);
            }
        }

        let mut writer = InstanceBufferWriter::new(
            &self.device,
            &self.instance_buffer_pool,
            self.is_unified_memory,
        )?;
        let bindings = [
            writer.write(bytes(&frame.quads))?,
            writer.write(bytes(&frame.shadows))?,
            writer.write(bytes(&frame.vertices))?,
            writer.write(bytes(&frame.sprites))?,
            writer.write(bytes(&frame.underlines))?,
            writer.write(bytes(&frame.mono))?,
            writer.write(bytes(&frame.poly))?,
            writer.write(bytes(&frame.composites))?,
            writer.write(bytes(&frame.blurs))?,
        ];
        let command_buffer = self
            .command_queue
            .commandBuffer()
            .context("creating command buffer")?;
        let result = self.encode(&frame, &command_buffer, drawable, &bindings);
        self.frame = frame;
        result?;

        let instance_buffer_pool = self.instance_buffer_pool.clone();
        let instance_buffer = Cell::new(Some(writer.finish()));
        let block = RcBlock::new(
            move |_: ptr::NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                if let Some(instance_buffer) = instance_buffer.take() {
                    instance_buffer_pool.lock().release(instance_buffer);
                }
            },
        );
        // SAFETY: Metal copies the live block and invokes it once after this frame completes.
        // Its captures own the frame resources and use the pool mutex when returning the buffer.
        unsafe {
            command_buffer.addCompletedHandler(RcBlock::as_ptr(&block));
        }
        Ok(command_buffer)
    }

    fn encode(
        &self,
        frame: &RenderFrame,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        drawable: &ProtocolObject<dyn MTLTexture>,
        bindings: &[InstanceBinding; 9],
    ) -> Result<()> {
        let Some(path_texture) = &self.path_intermediate_texture else {
            return Ok(());
        };
        let texture = |target: Target| match target {
            Target::Frame => drawable,
            Target::Layer(index) => &*self.pool[frame.layers[index].texture],
        };
        let text = TextParams::default();
        let alpha = if self.opaque { 1. } else { 0. };
        let begin = |target: Target, clear: bool| -> Result<_> {
            let descriptor = MTLRenderPassDescriptor::new();
            // SAFETY: Index zero is within the render pass's color attachment array.
            let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
            attachment.setTexture(Some(texture(target)));
            attachment.setStoreAction(MTLStoreAction::Store);
            if clear {
                attachment.setLoadAction(MTLLoadAction::Clear);
                attachment.setClearColor(MTLClearColor {
                    red: 0.,
                    green: 0.,
                    blue: 0.,
                    alpha: if target == Target::Frame { alpha } else { 0. },
                });
            } else {
                attachment.setLoadAction(MTLLoadAction::Load);
            }
            let encoder = command_buffer
                .renderCommandEncoderWithDescriptor(&descriptor)
                .context("creating render command encoder")?;
            set_globals(&encoder, &frame.globals(target, &text));
            Ok(encoder)
        };
        let draw = |encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
                    pipeline: &ProtocolObject<dyn MTLRenderPipelineState>,
                    kind: usize,
                    instances: &Range<u32>| {
            let binding = &bindings[kind];
            encoder.setRenderPipelineState(pipeline);
            // SAFETY: the binding holds this frame's instances, and the shaders read them by index.
            unsafe {
                encoder.setVertexBuffer_offset_atIndex(Some(&binding.buffer), binding.offset, 0);
                encoder.setFragmentBuffer_offset_atIndex(Some(&binding.buffer), binding.offset, 0);
                encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                    MTLPrimitiveType::TriangleStrip,
                    0,
                    4,
                    instances.len(),
                    instances.start as usize,
                );
            }
        };
        let atlas = |encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>, id| {
            let texture = self.sprite_atlas.metal_texture(id);
            // SAFETY: the atlas texture outlives the command buffer, which retains it.
            unsafe {
                encoder.setVertexTexture_atIndex(texture.as_deref(), 0);
                encoder.setFragmentTexture_atIndex(texture.as_deref(), 0);
            }
            texture.is_some()
        };

        let mut encoder: Option<Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>> = None;
        let mut current = (Target::Frame, true);
        macro_rules! pass {
            () => {{
                if encoder.is_none() {
                    encoder = Some(begin(current.0, current.1)?);
                    current.1 = false;
                }
                encoder.as_deref().unwrap()
            }};
        }
        macro_rules! end {
            () => {
                if let Some(encoder) = encoder.take() {
                    encoder.endEncoding();
                }
            };
        }
        let pipelines = &self.pipelines;
        for cmd in &frame.cmds {
            match cmd {
                // Targets are cleared right away, since they may be drawn into by a later pass.
                Cmd::Pass(target, clear) => {
                    end!();
                    current = (*target, *clear);
                    if *clear {
                        pass!();
                    }
                }
                Cmd::Quads(range) => draw(pass!(), &pipelines.quads, 0, range),
                Cmd::Shadows(range) => draw(pass!(), &pipelines.shadows, 1, range),
                Cmd::Underlines(range) => draw(pass!(), &pipelines.underlines, 4, range),
                Cmd::Mono(texture_id, range) => {
                    let encoder = pass!();
                    if atlas(encoder, *texture_id) {
                        draw(encoder, &pipelines.mono_sprites, 5, range);
                    }
                }
                Cmd::Poly(texture_id, range) => {
                    let encoder = pass!();
                    if atlas(encoder, *texture_id) {
                        draw(encoder, &pipelines.poly_sprites, 6, range);
                    }
                }
                // macOS draws text with grayscale antialiasing.
                Cmd::Subpixel(..) => {}
                Cmd::Paths { vertices, sprites } => {
                    end!();
                    let descriptor = MTLRenderPassDescriptor::new();
                    // SAFETY: Index zero is within the render pass's color attachment array.
                    let attachment =
                        unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
                    match &self.path_intermediate_msaa_texture {
                        Some(msaa) => {
                            attachment.setTexture(Some(msaa));
                            attachment.setResolveTexture(Some(path_texture));
                            attachment.setStoreAction(MTLStoreAction::MultisampleResolve);
                        }
                        None => {
                            attachment.setTexture(Some(path_texture));
                            attachment.setStoreAction(MTLStoreAction::Store);
                        }
                    }
                    attachment.setLoadAction(MTLLoadAction::Clear);
                    attachment.setClearColor(MTLClearColor {
                        red: 0.,
                        green: 0.,
                        blue: 0.,
                        alpha: 0.,
                    });
                    let raster = command_buffer
                        .renderCommandEncoderWithDescriptor(&descriptor)
                        .context("creating path render command encoder")?;
                    set_globals(&raster, &frame.globals(Target::Frame, &text));
                    raster.setRenderPipelineState(&pipelines.path_rasterization);
                    let binding = &bindings[2];
                    // SAFETY: the binding holds this frame's path vertices.
                    unsafe {
                        raster.setVertexBuffer_offset_atIndex(
                            Some(&binding.buffer),
                            binding.offset,
                            0,
                        );
                        raster.setFragmentBuffer_offset_atIndex(
                            Some(&binding.buffer),
                            binding.offset,
                            0,
                        );
                        raster.drawPrimitives_vertexStart_vertexCount(
                            MTLPrimitiveType::Triangle,
                            vertices.start as usize,
                            vertices.len(),
                        );
                    }
                    raster.endEncoding();
                    let encoder = pass!();
                    // SAFETY: the path texture lives as long as the renderer.
                    unsafe { encoder.setFragmentTexture_atIndex(Some(path_texture), 0) };
                    draw(encoder, &pipelines.path_sprites, 3, sprites);
                }
                Cmd::Blur { src, dst, index } => {
                    end!();
                    let blur = begin(*dst, true)?;
                    // SAFETY: the layer textures live until the command buffer completes.
                    unsafe { blur.setFragmentTexture_atIndex(Some(texture(*src)), 0) };
                    draw(&blur, &pipelines.blurs, 8, &(*index..*index + 1));
                    blur.endEncoding();
                    current.1 = false;
                }
                Cmd::Copy { src, dst, region } => {
                    end!();
                    current.1 = false;
                    let (from, to) = (frame.origin(*src), frame.origin(*dst));
                    let blit = command_buffer
                        .blitCommandEncoder()
                        .context("creating blit command encoder")?;
                    // SAFETY: the region lies within both textures.
                    unsafe {
                        blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                            texture(*src),
                            0,
                            0,
                            MTLOrigin {
                                x: (region[0] - from[0]) as usize,
                                y: (region[1] - from[1]) as usize,
                                z: 0,
                            },
                            MTLSize {
                                width: region[2] as usize,
                                height: region[3] as usize,
                                depth: 1,
                            },
                            texture(*dst),
                            0,
                            0,
                            MTLOrigin {
                                x: (region[0] - to[0]) as usize,
                                y: (region[1] - to[1]) as usize,
                                z: 0,
                            },
                        );
                    }
                    blit.endEncoding();
                }
                Cmd::Composite {
                    src,
                    mask,
                    backdrop,
                    index,
                } => {
                    let encoder = pass!();
                    let mask = mask.and_then(|id| self.sprite_atlas.metal_texture(id));
                    // SAFETY: the textures live until the command buffer completes.
                    unsafe {
                        encoder.setFragmentTexture_atIndex(Some(texture(*src)), 0);
                        encoder.setFragmentTexture_atIndex(
                            Some(mask.as_deref().unwrap_or(texture(*src))),
                            1,
                        );
                        encoder.setFragmentTexture_atIndex(
                            Some(backdrop.map_or(texture(*src), texture)),
                            2,
                        );
                    }
                    draw(encoder, &pipelines.composites, 7, &(*index..*index + 1));
                }
            }
        }
        end!();
        Ok(())
    }
}

fn set_globals(encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>, globals: &Globals) {
    let data = ptr::NonNull::from(globals).cast::<c_void>();
    // SAFETY: Metal copies the globals before this call returns.
    unsafe {
        encoder.setVertexBytes_length_atIndex(data, mem::size_of::<Globals>(), 1);
        encoder.setFragmentBytes_length_atIndex(data, mem::size_of::<Globals>(), 1);
    }
}

#[derive(Clone)]
struct InstanceBinding {
    buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    offset: usize,
}

struct InstanceBufferWriter {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    pool: Arc<Mutex<InstanceBufferPool>>,
    unified_memory: bool,
    filled: Vec<(InstanceBuffer, usize)>,
    current: InstanceBuffer,
    offset: usize,
}

impl InstanceBufferWriter {
    fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        pool: &Arc<Mutex<InstanceBufferPool>>,
        unified_memory: bool,
    ) -> Result<Self> {
        let current = pool.lock().acquire(device, unified_memory)?;
        Ok(Self {
            device: device.retain(),
            pool: pool.clone(),
            unified_memory,
            filled: Vec::new(),
            current,
            offset: 0,
        })
    }

    fn allocate<T>(&mut self, count: usize) -> Result<(InstanceBinding, &mut [MaybeUninit<T>])> {
        let size = mem::size_of::<T>() * count;
        let mut offset = self.offset.next_multiple_of(INSTANCE_BUFFER_ALIGNMENT);
        if offset + size > self.current.size {
            self.grow(size)?;
            offset = 0;
        }
        self.offset = offset + size;

        let binding = InstanceBinding {
            buffer: self.current.metal_buffer.clone(),
            offset,
        };
        // SAFETY: The aligned reservation lies within this frame's exclusively owned buffer and does not overlap prior writes.
        let values = unsafe {
            let start = self
                .current
                .metal_buffer
                .contents()
                .as_ptr()
                .cast::<u8>()
                .add(offset);
            slice::from_raw_parts_mut(start.cast::<MaybeUninit<T>>(), count)
        };
        Ok((binding, values))
    }

    fn write<T>(&mut self, values: &[T]) -> Result<InstanceBinding> {
        let (binding, destination) = self.allocate::<T>(values.len())?;
        // SAFETY: The disjoint source and destination hold the same number of T values, and the destination stays MaybeUninit.
        unsafe {
            ptr::copy_nonoverlapping(
                values.as_ptr(),
                destination.as_mut_ptr().cast::<T>(),
                values.len(),
            );
        }
        Ok(binding)
    }

    fn grow(&mut self, required: usize) -> Result<()> {
        let mut pool = self.pool.lock();
        let buffer_size = (pool.buffer_size * 2)
            .max(required.next_power_of_two())
            .min(MAX_INSTANCE_BUFFER_SIZE);
        anyhow::ensure!(
            buffer_size >= required,
            "instance buffer needs {required} bytes, above the maximum of {MAX_INSTANCE_BUFFER_SIZE}"
        );
        anyhow::ensure!(
            buffer_size > self.current.size,
            "frame instance data exceeds the {MAX_INSTANCE_BUFFER_SIZE}-byte maximum"
        );
        if buffer_size != pool.buffer_size {
            log::info!("increased instance buffer size to {buffer_size}");
            pool.reset(buffer_size);
        }
        let buffer = pool.acquire(&self.device, self.unified_memory)?;
        drop(pool);

        let filled = mem::replace(&mut self.current, buffer);
        self.filled.push((filled, self.offset));
        self.offset = 0;
        Ok(())
    }

    fn finish(self) -> InstanceBuffer {
        let Self {
            unified_memory,
            filled,
            current,
            offset,
            ..
        } = self;

        if !unified_memory {
            for (buffer, written) in &filled {
                if *written == 0 {
                    continue;
                }
                buffer.metal_buffer.didModifyRange(NSRange {
                    location: 0,
                    length: *written,
                });
            }
            if offset > 0 {
                current.metal_buffer.didModifyRange(NSRange {
                    location: 0,
                    length: offset,
                });
            }
        }

        // Metal retains encoded resources until the command buffer completes.
        // Only the final, largest buffer is worth keeping in the pool.
        drop(filled);
        current
    }
}
