use crate::{
    DevicePixels, Size,
    atlas::{Atlas, AtlasBackend, Packer, TextureList},
    frame::{Cmd, Globals, RenderFrame, Target, TextParams, bytes},
    scene::{AtlasTextureId, AtlasTextureKind, AtlasTile, Scene},
};
use anyhow::{Context as _, Result, ensure};
use block2::RcBlock;
use objc2::{Message, rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSRange, NSString};
use objc2_metal::{
    MTLBlendFactor, MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLCommandBuffer,
    MTLCommandEncoder, MTLCommandQueue, MTLCopyAllDevices, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLDrawable, MTLGPUFamily, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat,
    MTLPrimitiveType, MTLRegion, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLResourceOptions, MTLSize,
    MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem,
    ops::Range,
    ptr::{self, NonNull},
    rc::Rc,
    slice,
};

const FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;
// The smallest maximum texture size of the Metal GPU families on macOS.
const MAX_TEXTURE_SIZE: u32 = 16384;
/// Metal requires buffers to be bound at 256-byte aligned offsets.
const INSTANCE_BUFFER_ALIGNMENT: usize = 256;
const MAX_INSTANCE_BUFFER_SIZE: usize = 256 * 1024 * 1024;
/// Unused layer textures are kept this many frames, for layers that come back.
const SPARE_FRAMES: u8 = 3;

pub(crate) type MetalAtlas = Atlas<AtlasTextures>;

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type Device = Retained<ProtocolObject<dyn MTLDevice>>;

/// Instance buffers returned by finished frames, shared by every window.
struct BufferPool {
    size: usize,
    buffers: Vec<(Buffer, usize)>,
}

thread_local! {
    static POOL: Rc<RefCell<BufferPool>> = Rc::new(RefCell::new(BufferPool {
        size: 2 * 1024 * 1024,
        buffers: Vec::new(),
    }));
}

impl BufferPool {
    fn acquire(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        unified: bool,
    ) -> Result<(Buffer, usize)> {
        if let Some(buffer) = self.buffers.pop() {
            return Ok(buffer);
        }
        let options = if unified {
            // The CPU only writes these, which the write-combined cache speeds up.
            MTLResourceOptions::StorageModeShared | MTLResourceOptions::CPUCacheModeWriteCombined
        } else {
            MTLResourceOptions::StorageModeManaged
        };
        let buffer = device
            .newBufferWithLength_options(self.size, options)
            .with_context(|| format!("allocating {}-byte instance buffer", self.size))?;
        Ok((buffer, self.size))
    }
}

pub(crate) struct Renderer {
    device: Device,
    layer: Retained<CAMetalLayer>,
    unified: bool,
    presents_with_transaction: bool,
    opaque: bool,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipelines: Pipelines,
    atlas: Rc<MetalAtlas>,
    /// The offscreen layer textures with their size and the frames since they were last used.
    pool: Vec<(Texture, [u32; 2], u8)>,
    /// The pool index of each layer texture of the frame.
    targets: Vec<usize>,
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
    underlines: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    mono: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    poly: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    composites: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    blurs: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

impl Pipelines {
    fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self> {
        #[cfg(not(render_shaders))]
        let library = device
            .newLibraryWithSource_options_error(
                &NSString::from_str(include_str!("shaders.metal")),
                Some(&objc2_metal::MTLCompileOptions::new()),
            )
            .context("building Metal library")?;
        #[cfg(render_shaders)]
        let library = device
            .newLibraryWithData_error(&dispatch2::DispatchData::from_static_bytes(include_bytes!(
                concat!(env!("OUT_DIR"), "/shaders.metallib")
            )))
            .context("loading Metal library")?;
        let pipeline = |name: &str, blend: Blend| -> Result<_> {
            let function = |stage: &str| {
                let name = format!("{name}_{stage}");
                library
                    .newFunctionWithName(&NSString::from_str(&name))
                    .with_context(|| format!("locating function {name}"))
            };
            let descriptor = MTLRenderPipelineDescriptor::new();
            descriptor.setLabel(Some(&NSString::from_str(name)));
            descriptor.setVertexFunction(Some(&*function("vertex")?));
            descriptor.setFragmentFunction(Some(&*function("fragment")?));
            // SAFETY: index zero is within the color attachment array.
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
            quads: pipeline("quad", Blend::Over)?,
            shadows: pipeline("shadow", Blend::Over)?,
            underlines: pipeline("underline", Blend::Over)?,
            mono: pipeline("monochrome_sprite", Blend::Over)?,
            poly: pipeline("polychrome_sprite", Blend::Over)?,
            composites: pipeline("composite", Blend::Premultiplied)?,
            blurs: pipeline("blur", Blend::Replace)?,
        })
    }
}

impl Renderer {
    pub fn new(transparent: bool) -> Result<Self> {
        // Prefer a low power GPU on Intel Macs. Apple silicon has one GPU.
        let device = MTLCopyAllDevices()
            .iter()
            .min_by_key(|device| (device.isRemovable(), !device.isLowPower()))
            .or_else(|| MTLCreateSystemDefaultDevice())
            .context("accessing a Metal device")?;
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&device));
        layer.setPixelFormat(FORMAT);
        // An opaque layer can be shown directly by the display.
        layer.setOpaque(!transparent);
        layer.setMaximumDrawableCount(3);
        // Backdrop filters, blend modes and captures read from the drawable.
        layer.setFramebufferOnly(false);
        layer.setAllowsNextDrawableTimeout(false);
        layer.setNeedsDisplayOnBoundsChange(true);
        layer.setAutoresizingMask(
            objc2_quartz_core::CAAutoresizingMask::LayerWidthSizable
                | objc2_quartz_core::CAAutoresizingMask::LayerHeightSizable,
        );
        let unified = device.hasUnifiedMemory();
        // Apple GPUs can share atlas textures with the CPU.
        let shared = device.supportsFamily(MTLGPUFamily::Apple1);
        let queue = device.newCommandQueue().context("creating command queue")?;
        let atlas = Rc::new(Atlas::new(AtlasTextures {
            device: device.clone(),
            shared,
            mono: TextureList::default(),
            poly: TextureList::default(),
        }));
        Ok(Self {
            pipelines: Pipelines::new(&device)?,
            device,
            layer,
            unified,
            presents_with_transaction: false,
            opaque: !transparent,
            queue,
            atlas,
            pool: Vec::new(),
            targets: Vec::new(),
            frame: RenderFrame::default(),
        })
    }

    pub fn atlas(&self) -> &Rc<MetalAtlas> {
        &self.atlas
    }

    pub fn layer_ptr(&self) -> *mut CAMetalLayer {
        Retained::as_ptr(&self.layer).cast_mut()
    }

    /// Presents in the Core Animation transaction, so a live resize shows the new frame in step
    /// with the window.
    pub fn set_presents_with_transaction(&mut self, on: bool) {
        self.presents_with_transaction = on;
        self.layer.setPresentsWithTransaction(on);
    }

    pub fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        self.layer
            .setDrawableSize(CGSize::new(size.width.0 as f64, size.height.0 as f64));
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        self.opaque = !transparent;
        self.layer.setOpaque(!transparent);
    }

    /// Draws the scene. With `capture`, also returns the frame's size and RGBA rows.
    pub fn draw(&mut self, scene: &Scene, capture: bool) -> Option<Result<([u32; 2], Vec<u8>)>> {
        let size = self.layer.drawableSize();
        let window = [size.width.ceil() as u32, size.height.ceil() as u32];
        if window[0] == 0 || window[1] == 0 {
            return capture.then(|| Err(anyhow::anyhow!("The window has no size")));
        }
        let Some(drawable) = self.layer.nextDrawable() else {
            eprintln!("apps: no drawable for a {window:?} window");
            return capture.then(|| Err(anyhow::anyhow!("No drawable")));
        };
        let commands = match self.render_frame(scene, &drawable.texture(), window) {
            Ok(commands) => commands,
            Err(error) => {
                eprintln!("apps: failed to render: {error:#}");
                return capture.then(|| Err(error));
            }
        };
        let readback = capture.then(|| self.read_back(&commands, &drawable.texture(), window));
        if self.presents_with_transaction {
            commands.commit();
            commands.waitUntilScheduled();
            drawable.present();
        } else {
            commands.presentDrawable(ProtocolObject::from_ref(&*drawable));
            commands.commit();
        }
        readback.map(|buffer| {
            let buffer = buffer?;
            commands.waitUntilCompleted();
            // SAFETY: the GPU finished writing the shared buffer.
            let bgra = unsafe {
                slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), buffer.length())
            };
            let mut pixels = vec![0; bgra.len()];
            for (rgba, &[b, g, r, a]) in pixels.as_chunks_mut().0.iter_mut().zip(bgra.as_chunks().0)
            {
                *rgba = [r, g, b, a];
            }
            Ok((window, pixels))
        })
    }

    fn read_back(
        &self,
        commands: &ProtocolObject<dyn MTLCommandBuffer>,
        texture: &ProtocolObject<dyn MTLTexture>,
        window: [u32; 2],
    ) -> Result<Buffer> {
        let row = window[0] as usize * 4;
        let buffer = self
            .device
            .newBufferWithLength_options(
                row * window[1] as usize,
                MTLResourceOptions::StorageModeShared,
            )
            .context("allocating capture buffer")?;
        let blit = commands
            .blitCommandEncoder()
            .context("creating blit command encoder")?;
        // SAFETY: the buffer holds every row of the drawable.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                texture,
                0,
                0,
                MTLOrigin { x: 0, y: 0, z: 0 },
                MTLSize {
                    width: window[0] as usize,
                    height: window[1] as usize,
                    depth: 1,
                },
                &buffer,
                0,
                row,
                row * window[1] as usize,
            );
        }
        blit.endEncoding();
        Ok(buffer)
    }

    fn render_frame(
        &mut self,
        scene: &Scene,
        drawable: &ProtocolObject<dyn MTLTexture>,
        window: [u32; 2],
    ) -> Result<Retained<ProtocolObject<dyn MTLCommandBuffer>>> {
        let mut frame = mem::take(&mut self.frame);
        frame.record(scene, window, MAX_TEXTURE_SIZE, true);
        for (_, _, idle) in &mut self.pool {
            *idle = idle.saturating_add(1);
        }
        self.pool.retain(|(_, _, idle)| *idle <= SPARE_FRAMES);
        self.targets.clear();
        for size in &frame.textures {
            let index = match self
                .pool
                .iter()
                .position(|(_, pooled, idle)| *idle > 0 && pooled == size)
            {
                Some(index) => index,
                None => {
                    let descriptor = MTLTextureDescriptor::new();
                    // SAFETY: the dimensions are positive and the texture is 2D.
                    unsafe {
                        descriptor.setWidth(size[0] as usize);
                        descriptor.setHeight(size[1] as usize);
                    }
                    descriptor.setPixelFormat(FORMAT);
                    descriptor.setStorageMode(MTLStorageMode::Private);
                    descriptor
                        .setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
                    let texture = self
                        .device
                        .newTextureWithDescriptor(&descriptor)
                        .context("allocating layer texture")?;
                    self.pool.push((texture, *size, 0));
                    self.pool.len() - 1
                }
            };
            self.pool[index].2 = 0;
            self.targets.push(index);
        }
        let pool = POOL.with(Rc::clone);
        let mut writer = Writer::new(&self.device, pool.clone(), self.unified)?;
        let bindings = [
            writer.write(bytes(&frame.quads))?,
            writer.write(bytes(&frame.shadows))?,
            writer.write(bytes(&frame.underlines))?,
            writer.write(bytes(&frame.mono))?,
            writer.write(bytes(&frame.poly))?,
            writer.write(bytes(&frame.composites))?,
            writer.write(bytes(&frame.blurs))?,
        ];
        let commands = self
            .queue
            .commandBuffer()
            .context("creating command buffer")?;
        let result = self.encode(&frame, &commands, drawable, &bindings);
        self.frame = frame;
        result?;
        // WARNING: Metal may call the completion handler off the main thread, so the buffer goes
        // back to the pool from a main queue task.
        let done = Cell::new(Some(SendBuffer(writer.finish())));
        let block = RcBlock::new(move |_: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            if let Some(buffer) = done.take() {
                super::post(Box::new(move || {
                    let buffer = buffer.into_inner();
                    let pool = POOL.with(Rc::clone);
                    let mut pool = pool.borrow_mut();
                    if buffer.1 == pool.size {
                        pool.buffers.push(buffer);
                    }
                }));
            }
        });
        drop(pool);
        // SAFETY: Metal copies the block and calls it once, after the frame completes.
        unsafe { commands.addCompletedHandler(RcBlock::as_ptr(&block)) };
        Ok(commands)
    }

    fn encode(
        &self,
        frame: &RenderFrame,
        commands: &ProtocolObject<dyn MTLCommandBuffer>,
        drawable: &ProtocolObject<dyn MTLTexture>,
        bindings: &[Binding; 7],
    ) -> Result<()> {
        let texture = |target: Target| match target {
            Target::Frame => drawable,
            Target::Layer(index) => &*self.pool[self.targets[frame.layers[index].texture]].0,
        };
        let text = TextParams::default();
        let alpha = if self.opaque { 1. } else { 0. };
        let begin = |target: Target, clear: bool| -> Result<_> {
            let descriptor = MTLRenderPassDescriptor::new();
            // SAFETY: index zero is within the color attachment array.
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
            let encoder = commands
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
            // SAFETY: the binding holds this frame's instances, which the shaders read by index.
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
        let atlas_texture = |id: AtlasTextureId| self.atlas.backend.borrow().texture(id);
        let atlas = |encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>, id| {
            let texture = atlas_texture(id);
            // SAFETY: the command buffer retains the texture.
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
                // Tiles of one texture share its encoder, with their own globals.
                Cmd::Pass(target, false)
                    if let Some(encoder) = &encoder
                        && frame.texture(*target) == frame.texture(current.0) =>
                {
                    set_globals(encoder, &frame.globals(*target, &text));
                    current = (*target, false);
                }
                // Targets are cleared right away, since a later pass may draw into them.
                Cmd::Pass(target, clear) => {
                    end!();
                    current = (*target, *clear);
                    if *clear {
                        pass!();
                    }
                }
                Cmd::Quads(range) => draw(pass!(), &pipelines.quads, 0, range),
                Cmd::Shadows(range) => draw(pass!(), &pipelines.shadows, 1, range),
                Cmd::Underlines(range) => draw(pass!(), &pipelines.underlines, 2, range),
                Cmd::Mono(id, range) => {
                    let encoder = pass!();
                    if atlas(encoder, *id) {
                        draw(encoder, &pipelines.mono, 3, range);
                    }
                }
                Cmd::Poly(id, range) => {
                    let encoder = pass!();
                    if atlas(encoder, *id) {
                        draw(encoder, &pipelines.poly, 4, range);
                    }
                }
                Cmd::Blur { src, dst, index } => {
                    end!();
                    let blur = begin(*dst, true)?;
                    // SAFETY: the layer textures live until the command buffer completes.
                    unsafe { blur.setFragmentTexture_atIndex(Some(texture(*src)), 0) };
                    draw(&blur, &pipelines.blurs, 6, &(*index..*index + 1));
                    blur.endEncoding();
                    current.1 = false;
                }
                Cmd::Copy { src, dst, region } => {
                    end!();
                    current.1 = false;
                    let (from, to) = (frame.origin(*src), frame.origin(*dst));
                    let blit = commands
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
                    let mask = mask.and_then(atlas_texture);
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
                    draw(encoder, &pipelines.composites, 5, &(*index..*index + 1));
                }
            }
        }
        end!();
        Ok(())
    }
}

fn set_globals(encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>, globals: &Globals) {
    let data = NonNull::from(globals).cast::<c_void>();
    // SAFETY: Metal copies the globals before this returns.
    unsafe {
        encoder.setVertexBytes_length_atIndex(data, mem::size_of::<Globals>(), 1);
        encoder.setFragmentBytes_length_atIndex(data, mem::size_of::<Globals>(), 1);
    }
}

struct SendBuffer((Buffer, usize));

// SAFETY: the buffer is only moved to the main thread, where the pool lives.
unsafe impl Send for SendBuffer {}
// SAFETY: as above.
unsafe impl Sync for SendBuffer {}

impl SendBuffer {
    fn into_inner(self) -> (Buffer, usize) {
        self.0
    }
}

struct Binding {
    buffer: Buffer,
    offset: usize,
}

/// Packs a frame's instance data into pooled buffers.
struct Writer {
    device: Device,
    pool: Rc<RefCell<BufferPool>>,
    unified: bool,
    filled: Vec<((Buffer, usize), usize)>,
    current: (Buffer, usize),
    offset: usize,
}

impl Writer {
    fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        pool: Rc<RefCell<BufferPool>>,
        unified: bool,
    ) -> Result<Self> {
        let current = pool.borrow_mut().acquire(device, unified)?;
        Ok(Self {
            device: device.retain(),
            pool,
            unified,
            filled: Vec::new(),
            current,
            offset: 0,
        })
    }

    fn write(&mut self, data: &[u8]) -> Result<Binding> {
        let mut offset = self.offset.next_multiple_of(INSTANCE_BUFFER_ALIGNMENT);
        if offset + data.len() > self.current.1 {
            self.grow(data.len())?;
            offset = 0;
        }
        self.offset = offset + data.len();
        // SAFETY: the range lies within this frame's own buffer and after earlier writes.
        unsafe {
            ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.current.0.contents().as_ptr().cast::<u8>().add(offset),
                data.len(),
            );
        }
        Ok(Binding {
            buffer: self.current.0.clone(),
            offset,
        })
    }

    fn grow(&mut self, required: usize) -> Result<()> {
        let mut pool = self.pool.borrow_mut();
        let size = (pool.size * 2)
            .max(required.next_power_of_two())
            .min(MAX_INSTANCE_BUFFER_SIZE);
        ensure!(
            size >= required && size > self.current.1,
            "frame instance data exceeds {MAX_INSTANCE_BUFFER_SIZE} bytes"
        );
        if size != pool.size {
            pool.size = size;
            pool.buffers.clear();
        }
        let buffer = pool.acquire(&self.device, self.unified)?;
        drop(pool);
        let filled = mem::replace(&mut self.current, buffer);
        self.filled.push((filled, self.offset));
        self.offset = 0;
        Ok(())
    }

    fn finish(self) -> (Buffer, usize) {
        if !self.unified {
            for ((buffer, _), written) in &self.filled {
                if *written > 0 {
                    buffer.didModifyRange(NSRange::new(0, *written));
                }
            }
            if self.offset > 0 {
                self.current.0.didModifyRange(NSRange::new(0, self.offset));
            }
        }
        // Metal retains encoded buffers until the frame completes; only the last one is pooled.
        self.current
    }
}

pub(crate) struct AtlasTextures {
    device: Device,
    shared: bool,
    mono: TextureList<AtlasTexture>,
    poly: TextureList<AtlasTexture>,
}

struct AtlasTexture {
    id: AtlasTextureId,
    packer: Packer,
    texture: Texture,
}

impl AtlasTextures {
    fn list(&mut self, kind: AtlasTextureKind) -> &mut TextureList<AtlasTexture> {
        match kind {
            AtlasTextureKind::Monochrome => &mut self.mono,
            AtlasTextureKind::Polychrome => &mut self.poly,
        }
    }

    pub fn texture(&self, id: AtlasTextureId) -> Option<Texture> {
        let list = match id.kind {
            AtlasTextureKind::Monochrome => &self.mono,
            AtlasTextureKind::Polychrome => &self.poly,
        };
        Some(list.get(id.index as usize)?.texture.clone())
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
                let side = |value: i32| value.clamp(1024, MAX_TEXTURE_SIZE as i32);
                let texture_size = Size::new(
                    DevicePixels(side(size.width.0)),
                    DevicePixels(side(size.height.0)),
                );
                let descriptor = MTLTextureDescriptor::new();
                // SAFETY: both sides are within 1024..=16384.
                unsafe {
                    descriptor.setWidth(texture_size.width.0 as usize);
                    descriptor.setHeight(texture_size.height.0 as usize);
                }
                descriptor.setPixelFormat(match kind {
                    AtlasTextureKind::Monochrome => MTLPixelFormat::A8Unorm,
                    AtlasTextureKind::Polychrome => MTLPixelFormat::BGRA8Unorm,
                });
                descriptor.setUsage(MTLTextureUsage::ShaderRead);
                descriptor.setStorageMode(if self.shared {
                    MTLStorageMode::Shared
                } else {
                    MTLStorageMode::Managed
                });
                let texture = self
                    .device
                    .newTextureWithDescriptor(&descriptor)
                    .context("allocating atlas texture")?;
                let list = self.list(kind);
                let index = list.push(|index| AtlasTexture {
                    id: AtlasTextureId {
                        index: index as u32,
                        kind,
                    },
                    packer: Packer::new(texture_size),
                    texture,
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
        let row = size.width.0 as usize
            * if kind == AtlasTextureKind::Monochrome {
                1
            } else {
                4
            };
        ensure!(
            bytes.len() >= row * size.height.0 as usize,
            "atlas upload is too short"
        );
        let (tile, texture) = self.allocate(kind, size)?;
        let region = MTLRegion {
            origin: MTLOrigin {
                x: tile.bounds.origin.x.0 as usize,
                y: tile.bounds.origin.y.0 as usize,
                z: 0,
            },
            size: MTLSize {
                width: size.width.0 as usize,
                height: size.height.0 as usize,
                depth: 1,
            },
        };
        // SAFETY: the bytes cover the region, which lies within the CPU-accessible texture.
        unsafe {
            texture
                .texture
                .replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                    region,
                    0,
                    NonNull::from(bytes).cast(),
                    row,
                );
        }
        Ok(tile)
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
            list.textures[index] = None;
            list.free.push(index);
        }
    }
}
