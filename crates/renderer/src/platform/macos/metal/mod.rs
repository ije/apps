mod layers;
mod textures;

use crate::{
    Color, Rect, Size,
    atlas::{self, PageKind, PageList, PageStore, Tile},
    frame::{Command, Mask, Pool, RenderFrame, Target},
    gpu::{ATTRIBUTES, Globals, Instance, bytes},
    scene::Scene,
};
use anyhow::{Context as _, Result, ensure};
use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::{CFRetained, CGSize};
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceDisplayP3, kCGColorSpaceSRGB};
use objc2_foundation::{NSArray, NSNumber, NSRange, NSString, NSValue, ns_string};
use objc2_metal::{
    MTLBlendFactor, MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLCommandBuffer,
    MTLCommandEncoder, MTLCommandQueue, MTLCopyAllDevices, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLDrawable, MTLGPUFamily, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat,
    MTLPrimitiveType, MTLRegion, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLResourceOptions, MTLScissorRect,
    MTLSize, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
    MTLVertexDescriptor, MTLVertexFormat, MTLVertexStepFunction,
};
use objc2_quartz_core::{
    CAAutoresizingMask, CACurrentMediaTime, CAKeyframeAnimation, CALayer, CAMediaTiming,
    CAMetalDrawable, CAMetalLayer, CATransaction, CATransform3D, NSValueCATransform3DAdditions,
    kCAAnimationDiscrete, kCAFillModeBoth,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem,
    ptr::{self, NonNull},
    rc::Rc,
    slice,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use textures::{new_texture, page_texture, upload};

const FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;
const MAX_TEXTURE_SIZE: u32 = 16384;
const MIN_INSTANCE_BUFFER: usize = 2 << 20;
const MAX_INSTANCE_BUFFER: usize = 256 << 20;
const SHRINK_FRAMES: u32 = 120;

pub(crate) type Atlas = atlas::Atlas<Pages>;

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type Device = Retained<ProtocolObject<dyn MTLDevice>>;

// Instance buffers are reused only once the GPU is done with them.
struct Buffers {
    size: usize,
    spare: Vec<Buffer>,
    quiet: u32,
}

thread_local! {
    static BUFFERS: RefCell<Buffers> = const {
        RefCell::new(Buffers {
            size: MIN_INSTANCE_BUFFER,
            spare: Vec::new(),
            quiet: 0,
        })
    };
}

impl Buffers {
    fn reset(&mut self, size: usize) {
        self.size = size;
        self.spare.clear();
        self.quiet = 0;
    }

    fn fill(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        unified: bool,
        data: &[u8],
    ) -> Result<Buffer> {
        ensure!(
            data.len() <= MAX_INSTANCE_BUFFER,
            "a frame has more than {MAX_INSTANCE_BUFFER} bytes of instances"
        );
        if data.len() > self.size {
            self.reset(data.len().next_power_of_two());
        } else if data.len() > self.size / 4 || self.size <= MIN_INSTANCE_BUFFER {
            self.quiet = 0;
        } else {
            self.quiet += 1;
            if self.quiet == SHRINK_FRAMES {
                self.reset(self.size / 2);
            }
        }
        let buffer = match self.spare.pop() {
            Some(buffer) => buffer,
            None => device
                .newBufferWithLength_options(
                    self.size,
                    if unified {
                        MTLResourceOptions::StorageModeShared
                            | MTLResourceOptions::CPUCacheModeWriteCombined
                    } else {
                        MTLResourceOptions::StorageModeManaged
                    },
                )
                .with_context(|| format!("allocating an instance buffer of {} bytes", self.size))?,
        };
        // SAFETY: the GPU doesn't use a spare buffer, and the data fits in it.
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), buffer.contents().as_ptr().cast(), data.len());
        }
        if !unified {
            buffer.didModifyRange(NSRange::new(0, data.len()));
        }
        Ok(buffer)
    }
}

pub(crate) struct Present(Option<super::Sendable<Box<dyn FnOnce()>>>);

impl Present {
    fn new(present: impl FnOnce() + 'static) -> Self {
        Self(Some(super::Sendable::new(Box::new(present))))
    }

    fn discard(mut self) {
        self.0 = None;
    }
}

impl Drop for Present {
    fn drop(&mut self) {
        if let Some(present) = self.0.take() {
            present.take()();
        }
    }
}

pub(crate) struct Renderer {
    device: Device,
    layer: Retained<CAMetalLayer>,
    overlay: Retained<CALayer>,
    size: CGSize,
    unified: bool,
    in_transaction: bool,
    present: Option<Present>,
    opaque: bool,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipelines: &'static Pipelines,
    atlas: Rc<Atlas>,
    // Bound in place of an atlas page that a draw doesn't read.
    blank: Texture,
    pool: Pool<Texture>,
    frame: RenderFrame,
    promoted: Vec<Promoted>,
    glass: Vec<NativeGlass>,
    glass_update: bool,
    glass_stale: bool,
    waits: u8,
    caret: Option<(Retained<CALayer>, Rect<f32>, Color, std::time::Instant)>,
    restacked: bool,
    committed: u64,
    completed: Arc<AtomicU64>,
    pub snapshots: Vec<crate::Snapshot>,
    // Copies of the snapshot layers of the last frame that read them, for the next one with the same sizes.
    shot_copies: Vec<Texture>,
    // The drawable of the last frame, which a frame with a damage region copies.
    last: Option<Texture>,
    // The layer is Display P3.
    wide: bool,
    // Recent drawables with the frame that each holds, and the regions of recent frames, `None` when
    // whole. A drawable that comes back draws only what changed since its frame.
    drawables: Vec<(Texture, u64)>,
    regions: Vec<(u64, Option<Vec<[i32; 4]>>)>,
    frames: u64,
}

type Rendered = (
    Retained<ProtocolObject<dyn MTLCommandBuffer>>,
    Vec<Retained<ProtocolObject<dyn CAMetalDrawable>>>,
);

struct Promoted {
    key: u64,
    index: usize,
    sublayer: Retained<CAMetalLayer>,
    drawn: bool,
}

struct NativeGlass {
    index: usize,
    surface: super::glass::GlassSurface,
}

type Pipeline = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;

struct Pipelines {
    draw: Pipeline,
    composite: Pipeline,
    blur: Pipeline,
    copy: Pipeline,
}

impl Pipelines {
    fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self> {
        // build.rs translates shaders.wgsl to Metal and compiles it when it can.
        #[cfg(not(renderer_shaders))]
        let library = {
            let source =
                NSString::from_str(include_str!(concat!(env!("OUT_DIR"), "/shaders.metal")));
            device.newLibraryWithSource_options_error(&source, None)
        };
        #[cfg(renderer_shaders)]
        let library = {
            let packed: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/library.deflate"));
            let mut data = Vec::with_capacity(packed.len() * 3);
            std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(packed), &mut data)?;
            device.newLibraryWithData_error(&dispatch2::DispatchData::from_boxed(data.into()))
        };
        let library = library.context("the shaders don't build")?;
        // Shaders return premultiplied colors. Blurs and copies replace what the target holds.
        let pipeline =
            |name: &str, blend: bool, vertex: Option<&MTLVertexDescriptor>| -> Result<_> {
                let function = |stage: &str| {
                    let name = format!("{name}_{stage}");
                    library
                        .newFunctionWithName(&NSString::from_str(&name))
                        .with_context(|| format!("locating function {name}"))
                };
                let descriptor = MTLRenderPipelineDescriptor::new();
                descriptor.setVertexFunction(Some(&*function("vertex")?));
                descriptor.setFragmentFunction(Some(&*function("fragment")?));
                descriptor.setVertexDescriptor(vertex);
                // SAFETY: index zero is within the color attachment array.
                let attachment =
                    unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
                attachment.setPixelFormat(FORMAT);
                if blend {
                    attachment.setBlendingEnabled(true);
                    attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
                    attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                    attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                    attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                }
                device
                    .newRenderPipelineStateWithDescriptor_error(&descriptor)
                    .with_context(|| format!("the {name} pipeline"))
            };
        let instances = MTLVertexDescriptor::new();
        for index in 0..ATTRIBUTES {
            // SAFETY: Metal has 31 attribute slots, and every vec4 of an instance lies within its stride.
            unsafe {
                let attribute = instances.attributes().objectAtIndexedSubscript(index);
                attribute.setFormat(MTLVertexFormat::Float4);
                attribute.setOffset(index * 16);
                attribute.setBufferIndex(0);
            }
        }
        // SAFETY: buffer zero holds the instances, which are packed.
        let layout = unsafe { instances.layouts().objectAtIndexedSubscript(0) };
        // SAFETY: the stride is the size of an instance.
        unsafe { layout.setStride(mem::size_of::<Instance>()) };
        layout.setStepFunction(MTLVertexStepFunction::PerInstance);
        Ok(Self {
            draw: pipeline("draw", true, Some(&instances))?,
            composite: pipeline("composite", true, None)?,
            blur: pipeline("blur", false, None)?,
            copy: pipeline("copy", false, None)?,
        })
    }

    // Use draws for copies: a blit encoder retains about 84 MB in the Apple GPU driver until frames stop.
    fn copy(
        &self,
        commands: &ProtocolObject<dyn MTLCommandBuffer>,
        source: &ProtocolObject<dyn MTLTexture>,
        by: [i32; 2],
        target: &ProtocolObject<dyn MTLTexture>,
        area: [i32; 4],
        load: MTLLoadAction,
    ) -> Result<()> {
        let encoder = self.copy_pass(commands, target, load)?;
        self.copy_draw(&encoder, source, by, area);
        encoder.endEncoding();
        Ok(())
    }

    // A pass that copies into `target`. Each pass loads and stores the whole target, so copies into one target
    // share a pass.
    fn copy_pass(
        &self,
        commands: &ProtocolObject<dyn MTLCommandBuffer>,
        target: &ProtocolObject<dyn MTLTexture>,
        load: MTLLoadAction,
    ) -> Result<Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>> {
        let descriptor = MTLRenderPassDescriptor::new();
        // SAFETY: index zero is within the color attachment array.
        let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        attachment.setTexture(Some(target));
        attachment.setLoadAction(load);
        attachment.setStoreAction(MTLStoreAction::Store);
        let encoder = commands
            .renderCommandEncoderWithDescriptor(&descriptor)
            .context("no render encoder")?;
        encoder.setRenderPipelineState(&self.copy);
        Ok(encoder)
    }

    // Copies `area` of the target from `source`, `[dx, dy]` further.
    fn copy_draw(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        source: &ProtocolObject<dyn MTLTexture>,
        [dx, dy]: [i32; 2],
        [x, y, width, height]: [i32; 4],
    ) {
        let [x, y, width, height] = [x, y, width, height].map(|value| value as usize);
        encoder.setScissorRect(MTLScissorRect {
            x,
            y,
            width,
            height,
        });
        let offset = [dx, dy, 0, 0];
        // SAFETY: Metal copies the offset before this returns, and the command buffer retains the texture.
        unsafe {
            encoder.setFragmentBytes_length_atIndex(
                NonNull::from(&offset).cast::<c_void>(),
                mem::size_of_val(&offset),
                2,
            );
            encoder.setFragmentTexture_atIndex(Some(source), 0);
            encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::TriangleStrip, 0, 4);
        }
    }
}

// A copy that starts a frame: the source, its offset and the area.
type Prelude<'a> = (&'a ProtocolObject<dyn MTLTexture>, [i32; 2], [i32; 4]);

// All windows share the device and the pipelines. Metal objects are thread-safe. A window that opens while
// another thread builds them waits for that build.
fn shared() -> Result<&'static (Device, Pipelines)> {
    static SHARED: OnceLock<(Device, Pipelines)> = OnceLock::new();
    static BUILD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if let Some(shared) = SHARED.get() {
        return Ok(shared);
    }
    let _build = BUILD.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(shared) = SHARED.get() {
        return Ok(shared);
    }
    // Built-in GPUs first, and among those the integrated one, which saves power.
    let rank = |device: &ProtocolObject<dyn MTLDevice>| {
        2 * u8::from(device.isRemovable()) + u8::from(!device.isLowPower())
    };
    let device = MTLCopyAllDevices()
        .iter()
        .min_by_key(|device| rank(device))
        .or_else(|| MTLCreateSystemDefaultDevice())
        .context("no Metal device")?;
    let pipelines = Pipelines::new(&device)?;
    Ok(SHARED.get_or_init(|| (device, pipelines)))
}

// Builds the device and the pipelines before the first window needs them.
pub(crate) fn prewarm() {
    let _ = shared();
}

impl Renderer {
    pub fn new(transparent: bool) -> Result<Self> {
        let (device, pipelines) = shared()?;
        let device = device.clone();
        let wide = forced_p3() == Some(true);
        let layer = metal_layer(&device, !transparent, wide);
        layer.setMaximumDrawableCount(3);
        layer.setAllowsNextDrawableTimeout(false);
        layer.setNeedsDisplayOnBoundsChange(true);
        layer.setAutoresizingMask(
            CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
        );
        // AppKit derives layer orientation from the view. Flip the overlay so layers and the caret use
        // top-left coordinates.
        let overlay = CALayer::new();
        overlay.setGeometryFlipped(true);
        // Promoted layers can move past the window's content, which must not show them over the title bar.
        overlay.setMasksToBounds(true);
        let unified = device.hasUnifiedMemory();
        let shared = device.supportsFamily(MTLGPUFamily::Apple1);
        let queue = device.newCommandQueue().context("no Metal command queue")?;
        let pages = Pages {
            device: device.clone(),
            shared,
            pages: Default::default(),
        };
        let blank = page_texture(&device, shared, PageKind::Color, Size::new(1, 1))?;
        upload(
            &blank,
            Rect::new(Default::default(), Size::new(1, 1)),
            &[0; 4],
            4,
        );
        let atlas = Rc::new(Atlas::new(pages));
        Ok(Self {
            pipelines,
            device,
            layer,
            overlay,
            size: CGSize::new(0., 0.),
            unified,
            in_transaction: false,
            present: None,
            opaque: !transparent,
            queue,
            atlas,
            blank,
            pool: Pool::default(),
            frame: RenderFrame::default(),
            promoted: Vec::new(),
            glass: Vec::new(),
            glass_update: false,
            glass_stale: false,
            waits: 0,
            caret: None,
            restacked: false,
            committed: 0,
            completed: Arc::default(),
            snapshots: Vec::new(),
            shot_copies: Vec::new(),
            last: None,
            wide,
            drawables: Vec::new(),
            regions: Vec::new(),
            frames: 0,
        })
    }

    pub fn atlas(&self) -> &Rc<Atlas> {
        &self.atlas
    }

    pub fn layer(&self) -> &Retained<CAMetalLayer> {
        &self.layer
    }

    pub fn overlay(&self) -> &Retained<CALayer> {
        &self.overlay
    }

    // Only the drawing thread may change the drawable size, so a frame always fits its scene.
    pub fn set_drawable_size(&mut self, size: CGSize) {
        self.size = size;
    }

    // Frames queue when they are drawn faster than the display shows them: after a late frame, or
    // while a ProMotion display still speeds up. Each queued frame delays all later ones by a
    // refresh. Long draws that wait for a drawable in a row tell that the queue is full, a single one
    // does not. A skipped tick shows as a dropped frame, so one frame in the queue stays: its waits
    // are short.
    pub fn take_congested(&mut self) -> bool {
        let congested = self.waits >= 2;
        self.waits = self.waits.min(1);
        congested
    }

    pub fn take_present(&mut self) -> Option<Present> {
        self.present.take()
    }

    pub fn discard(&mut self, present: Present) {
        present.discard();
        self.glass_stale = true;
    }

    pub fn set_presents_with_transaction(&mut self, on: bool) {
        self.in_transaction = on;
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        self.last = None;
        self.drawables.clear();
        self.opaque = !transparent;
        self.layer.setOpaque(!transparent);
    }

    // Free only retired tiles here. Keep buffers, textures and spare pages until release to avoid churn
    // between frame bursts.
    pub fn trim(&mut self) {
        if self.completed.load(Ordering::Acquire) == self.committed {
            self.atlas.free_retired(|| {});
        }
    }

    pub fn release(&mut self, scene: &crate::Scene) {
        self.atlas.release_images(scene.texels());
        self.shot_copies.clear();
        // An empty command buffer waits for earlier frames on the same queue.
        self.atlas.free_retired(|| {
            if let Some(commands) = self.queue.commandBuffer() {
                commands.commit();
                commands.waitUntilCompleted();
            }
        });
        for list in &mut self.atlas.pages.borrow_mut().pages {
            list.trim();
        }
        self.frame.clear_pool(&mut self.pool);
        self.last = None;
        self.drawables.clear();
        BUFFERS.with(|buffers| buffers.borrow_mut().reset(MIN_INSTANCE_BUFFER));
        // Change size to release spare drawables. The layer keeps its last frame until the next draw restores
        // the size.
        self.layer.setDrawableSize(CGSize::new(1., 1.));
        for glass in &self.glass {
            glass.surface.layer.setDrawableSize(CGSize::new(1., 1.));
        }
    }

    pub fn draw(&mut self, scene: &Scene, capture: bool) -> Option<Result<([u32; 2], Vec<u8>)>> {
        let size = self.size;
        if size.width < 1. || size.height < 1. {
            return capture.then(|| Err(anyhow::anyhow!("The window has no size")));
        }
        if self.layer.drawableSize() != size {
            self.layer.setDrawableSize(size);
        }
        let wide =
            (self.layer.colorspace()).is_some_and(|space| ptr::eq(&*space, color_space(true)));
        if wide != self.wide {
            self.wide = wide;
            // Kept pixels are in the old color space.
            self.last = None;
            self.drawables.clear();
            self.frame.clear_pool(&mut self.pool);
            let space = Some(color_space(wide));
            for promoted in &self.promoted {
                promoted.sublayer.setColorspace(space);
            }
            for glass in &self.glass {
                glass.surface.layer.setColorspace(space);
            }
        }
        let had_glass = !self.glass.is_empty();
        let scale = self.layer.contentsScale();
        let mut glass: Vec<NativeGlass> = Vec::new();
        if super::glass::available() {
            // Later glass draws above earlier glass, so it is checked first.
            scene.native_glass(|index, bounds, config| {
                let region = crate::geometry::region(bounds);
                let sublayer = match self
                    .glass
                    .iter()
                    .position(|old| old.surface.key == config.key)
                {
                    Some(at) => self.glass.swap_remove(at).surface.layer,
                    None => {
                        let sublayer = metal_layer(&self.device, false, self.wide);
                        sublayer.setPresentsWithTransaction(true);
                        // Match the window's drawable count so an uncommitted frame cannot block the next
                        // one.
                        sublayer.setMaximumDrawableCount(3);
                        sublayer.setAllowsNextDrawableTimeout(false);
                        sublayer
                    }
                };
                sublayer.setContentsScale(scale);
                sublayer.setDrawableSize(CGSize::new(region[2] as f64, region[3] as f64));
                glass.push(NativeGlass {
                    index,
                    surface: super::glass::GlassSurface {
                        key: config.key,
                        layer: sublayer,
                        bounds: region.map(|value| f64::from(value) / scale),
                        radius: f64::from(config.corner_radius) / scale,
                        tint: config.tint,
                        clear: config.clear,
                        interactive: config.interactive,
                    },
                });
            });
        }
        glass.reverse();
        self.glass = glass;
        let native_capture = capture && !self.glass.is_empty();
        self.glass_stale |= had_glass || !self.glass.is_empty();
        let native_transaction = self.glass_stale;
        if capture && !native_capture {
            self.demote();
        }
        let window = [size.width as u32, size.height as u32];
        let last = self.last.take().filter(|last| {
            !capture
                && !self.in_transaction
                && !self.restacked
                && !native_transaction
                && self.glass.is_empty()
                && [last.width() as u32, last.height() as u32] == window
        });
        let damage = (last.as_ref()).and_then(|_| scene.damage_regions(window, &[]));
        // Nothing changed, so the window keeps its last frame.
        if damage.as_ref().is_some_and(Vec::is_empty)
            && self.promoted.iter().all(|promoted| promoted.drawn)
            && (self.promoted.iter().map(|promoted| promoted.index)).eq(self
                .frame
                .skip
                .iter()
                .copied())
            && !scene.shots.iter().any(|shot| shot.read)
        {
            self.last = last;
            return None;
        }
        let damage = damage.unwrap_or_default();
        let last = last.filter(|_| !damage.is_empty());
        // Present the frame in the same transaction that adds or removes promoted sublayers.
        let resizing = self.in_transaction;
        let transact = resizing || mem::take(&mut self.restacked) || native_transaction;
        self.layer.setPresentsWithTransaction(transact);
        let asked = Instant::now();
        let next = self.layer.nextDrawable();
        self.waits = match asked.elapsed() > Duration::from_millis(3) {
            true => self.waits.saturating_add(1),
            false => 0,
        };
        let Some(drawable) = next else {
            self.last = None;
            self.drawables.clear();
            eprintln!("apps: no drawable for a {size:?} window");
            return capture.then(|| Err(anyhow::anyhow!("No drawable")));
        };
        let frame_texture = drawable.texture();
        let drawn = [frame_texture.width() as u32, frame_texture.height() as u32];
        let (window, mut damage, mut last) = match drawn == window {
            true => (window, damage, last),
            false => (drawn, Vec::new(), None),
        };
        // A drawable still holds its own frame, so it needs no copy of the last one when the frames
        // since then have regions too.
        let since = (self.drawables.iter())
            .find(|(texture, _)| ptr::eq(&**texture, &*frame_texture))
            .map(|(_, frame)| *frame);
        // A scroll copies the last frame moved, so the regions of earlier frames don't tell what a drawable lacks.
        let shift = scene.shift.filter(|_| !damage.is_empty() && last.is_some());
        // WARNING: later frames add the regions of this frame, so it keeps only its own, or they grow each frame.
        let own = damage.clone();
        if !damage.is_empty()
            && shift.is_none()
            && let Some(since) = since
            && let Some(extra) = (since + 1..=self.frames)
                .map(|frame| {
                    let (_, regions) = self.regions.iter().find(|(at, _)| *at == frame)?;
                    regions.as_ref()
                })
                .collect::<Option<Vec<_>>>()
        {
            let extra: Vec<_> = extra.into_iter().flatten().copied().collect();
            if let Some(regions) = scene.damage_regions(window, &extra) {
                (damage, last) = (regions, None);
            }
        }
        // Captures are sRGB, so a Display P3 frame draws once more for them. A forced profile captures its pixels.
        let srgb = (capture && !native_capture && self.wide && forced_p3().is_none())
            .then(|| {
                let usage = MTLTextureUsage::RenderTarget;
                let size = window.map(|side| side as usize);
                new_texture(&self.device, size, FORMAT, usage, MTLStorageMode::Private)
            })
            .transpose();
        let srgb = match srgb {
            Ok(srgb) => srgb,
            Err(error) => return Some(Err(error)),
        };
        let rendered = self.render_frame(
            scene,
            &frame_texture,
            window,
            damage,
            last.as_deref().map(|last| (last, shift)),
            srgb.as_deref(),
        );
        self.last = Some(frame_texture.clone());
        self.frames += 1;
        let frames = self.frames;
        self.drawables
            .retain(|(texture, frame)| !ptr::eq(&**texture, &*frame_texture) && frames - frame < 4);
        self.drawables.push((frame_texture.clone(), frames));
        self.regions.retain(|(frame, _)| frames - frame < 4);
        let regions = (!self.frame.damage.is_empty() && shift.is_none()).then_some(own);
        self.regions.push((frames, regions));
        let (commands, sublayers) = match rendered {
            Ok(commands) => commands,
            Err(error) => {
                self.last = None;
                self.drawables.clear();
                eprintln!("apps: failed to render: {error:#}");
                return capture.then(|| Err(error));
            }
        };
        let readback = (capture && !native_capture)
            .then(|| self.read_back(&commands, srgb.as_deref().unwrap_or(&frame_texture), window));
        let mut copies = mem::take(&mut self.shot_copies);
        let shots: Vec<_> = (self.frame.shots.iter())
            .filter_map(|&(key, target, region)| {
                let size = [region[2] as u32, region[3] as u32];
                let layer = self.frame.pooled(&self.pool, target)?;
                // A blit encoder keeps much memory in the Apple GPU driver, so there a draw copies the layer
                // into a texture that the CPU can read.
                let read = if self.atlas.pages.borrow().shared {
                    let kept = (copies.iter())
                        .position(|copy| [copy.width() as u32, copy.height() as u32] == size);
                    kept.map(|at| Ok(copies.swap_remove(at)))
                        .unwrap_or_else(|| {
                            new_texture(
                                &self.device,
                                size.map(|side| side as usize),
                                FORMAT,
                                MTLTextureUsage::RenderTarget,
                                MTLStorageMode::Shared,
                            )
                        })
                        .and_then(|copy| {
                            let area = [0, 0, region[2], region[3]];
                            let load = MTLLoadAction::DontCare;
                            (self.pipelines).copy(&commands, layer, [0, 0], &copy, area, load)?;
                            Ok(Ok(copy))
                        })
                } else {
                    self.read_back(&commands, layer, size).map(Err)
                };
                match read {
                    Ok(read) => Some((key, size, read)),
                    Err(error) => {
                        eprintln!("apps: failed to read a layer: {error:#}");
                        None
                    }
                }
            })
            .collect();
        if transact {
            commands.commit();
            commands.waitUntilScheduled();
            let present = move || {
                drawable.present();
                for sublayer in &sublayers {
                    sublayer.present();
                }
            };
            if resizing || native_transaction {
                self.present = Some(Present::new(present));
            } else {
                present();
            }
        } else {
            commands.presentDrawable(ProtocolObject::from_ref(&*drawable));
            for sublayer in &sublayers {
                commands.presentDrawable(ProtocolObject::from_ref(&**sublayer));
            }
            commands.commit();
        }
        self.glass_update = native_transaction;
        if !shots.is_empty() {
            commands.waitUntilCompleted();
        }
        for (key, size, read) in shots {
            let row = size[0] as usize * 4;
            let mut pixels = vec![0u8; row * size[1] as usize];
            match &read {
                // SAFETY: the GPU finished the texture, and `pixels` holds every row of it.
                Ok(texture) => unsafe {
                    texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                        NonNull::from(&mut pixels[..]).cast::<c_void>(),
                        row,
                        MTLRegion {
                            origin: MTLOrigin { x: 0, y: 0, z: 0 },
                            size: MTLSize {
                                width: size[0] as usize,
                                height: size[1] as usize,
                                depth: 1,
                            },
                        },
                        0,
                    )
                },
                // SAFETY: the GPU finished writing the shared buffer, which is as long as `pixels`.
                Err(buffer) => pixels.copy_from_slice(unsafe {
                    slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), buffer.length())
                }),
            }
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            self.snapshots.push(crate::Snapshot { key, size, pixels });
            if let Ok(texture) = read {
                self.shot_copies.push(texture);
            }
        }
        readback
            .map(|buffer| {
                let buffer = buffer?;
                commands.waitUntilCompleted();
                // SAFETY: the GPU finished writing the shared buffer.
                let bgra = unsafe {
                    slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), buffer.length())
                };
                let mut pixels = bgra.to_vec();
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                Ok((window, pixels))
            })
            .or_else(|| {
                native_capture.then(|| {
                    Err(anyhow::anyhow!(
                        "capturePage does not support native Liquid Glass"
                    ))
                })
            })
    }

    fn read_back(
        &self,
        commands: &ProtocolObject<dyn MTLCommandBuffer>,
        drawn: &ProtocolObject<dyn MTLTexture>,
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
        let blit = commands.blitCommandEncoder().context("no blit encoder")?;
        // SAFETY: the buffer holds every row of the drawable.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                drawn,
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
        damage: Vec<[i32; 4]>,
        last: Option<(&ProtocolObject<dyn MTLTexture>, Option<crate::scene::Shift>)>,
        srgb: Option<&ProtocolObject<dyn MTLTexture>>,
    ) -> Result<Rendered> {
        let mut frame = mem::take(&mut self.frame);
        let skipped = mem::take(&mut frame.skip);
        frame.content.clear();
        for promoted in &self.promoted {
            frame.skip.push(promoted.index);
            if !promoted.drawn {
                frame.content.push(promoted.index);
            }
        }
        for glass in &self.glass {
            frame.skip.push(glass.index);
            frame.content.push(glass.index);
        }
        // A layer that left or joined Core Animation draws otherwise in the whole window.
        frame.damage = match frame.skip == skipped {
            true => damage,
            false => Vec::new(),
        };
        frame.fresh = srgb.is_some();
        frame.keeps_under = true;
        frame.take_spare(&self.pool);
        frame.record(scene, window, MAX_TEXTURE_SIZE, true);
        let device = &self.device;
        frame.fill_pool(&mut self.pool, |size| {
            new_texture(
                device,
                size.map(|side| side as usize),
                FORMAT,
                MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead,
                MTLStorageMode::Private,
            )
        })?;
        let instances = (!frame.instances.is_empty())
            .then(|| {
                BUFFERS.with(|buffers| {
                    buffers
                        .borrow_mut()
                        .fill(&self.device, self.unified, bytes(&frame.instances))
                })
            })
            .transpose()?;
        let commands = self.queue.commandBuffer().context("no command buffer")?;
        // The pool can give back the drawable of the last frame, which holds it already. Else the frame starts as a
        // copy of the last one, and a scroll copies the viewport from the pixels that were `by` further. The scene
        // damaged the edges that they don't cover.
        let mut prelude: Vec<Prelude> = Vec::new();
        let mut kept = None;
        if !frame.damage.is_empty()
            && let Some((last, shift)) = last
        {
            let whole = [0, 0, window[0] as i32, window[1] as i32];
            let moved = shift.map(|([x, y, width, height], by)| {
                let area = [
                    x + (-by[0]).max(0),
                    y + (-by[1]).max(0),
                    width - by[0].abs(),
                    height - by[1].abs(),
                ];
                (by, area)
            });
            let source = match ptr::eq(last, drawable) {
                false => last,
                // The last drawable can't read itself, so its pixels move through a copy.
                true if moved.is_some() => {
                    let size = [window[0] as usize, window[1] as usize];
                    let usage = MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead;
                    let copy =
                        new_texture(&self.device, size, FORMAT, usage, MTLStorageMode::Private)?;
                    (self.pipelines).copy(
                        &commands,
                        last,
                        [0, 0],
                        &copy,
                        whole,
                        MTLLoadAction::DontCare,
                    )?;
                    &**kept.insert(copy)
                }
                true => drawable,
            };
            match moved {
                Some((by, area)) if area[2] > 0 && area[3] > 0 => {
                    // Only the parts of the window outside the moved area copy in place.
                    let [x, y, width, height] = area;
                    let [right, bottom] = [whole[2], whole[3]];
                    for part in [
                        [0, 0, right, y],
                        [0, y + height, right, bottom - y - height],
                        [0, y, x, height],
                        [x + width, y, right - x - width, height],
                    ] {
                        if part[2] > 0 && part[3] > 0 && !ptr::eq(source, drawable) {
                            prelude.push((source, [0, 0], part));
                        }
                    }
                    prelude.push((source, by, area));
                }
                _ if !ptr::eq(source, drawable) => prelude.push((source, [0, 0], whole)),
                _ => {}
            }
        }
        // The sRGB copy draws first, so layers keep the pixels of the window.
        let result = srgb
            .map_or(Ok(()), |srgb| {
                self.encode(&frame, &commands, srgb, instances.as_deref(), false, &[])
            })
            .and_then(|()| {
                self.encode(
                    &frame,
                    &commands,
                    drawable,
                    instances.as_deref(),
                    self.wide,
                    &prelude,
                )
            });
        let mut sublayers = Vec::new();
        if result.is_ok() {
            for &(index, target, region) in &frame.promoted {
                let Some(source) = frame.pooled(&self.pool, target) else {
                    continue;
                };
                let promoted = self.promoted.iter_mut().find(|p| p.index == index);
                let native = self.glass.iter().find(|glass| glass.index == index);
                let Some(output) = promoted
                    .as_ref()
                    .map(|p| &p.sublayer)
                    .or_else(|| native.map(|glass| &glass.surface.layer))
                else {
                    continue;
                };
                let Some(sublayer) = output.nextDrawable() else {
                    continue;
                };
                self.pipelines.copy(
                    &commands,
                    source,
                    [0, 0],
                    &sublayer.texture(),
                    [0, 0, region[2], region[3]],
                    MTLLoadAction::DontCare,
                )?;
                if let Some(promoted) = promoted {
                    promoted.drawn = true;
                }
                sublayers.push(sublayer);
            }
        }
        // A frame that fails is never committed, so it keeps no cached layers for the next one.
        result?;
        self.frame = frame;
        // Metal completes on its own thread, while the buffers belong to the drawing thread.
        let done = Cell::new(instances.map(super::Sendable::new));
        let thread = super::app_thread();
        self.committed += 1;
        let (frame, completed) = (self.committed, self.completed.clone());
        let block = RcBlock::new(move |_: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            completed.fetch_max(frame, Ordering::Release);
            if let Some(buffer) = done.take() {
                super::post(
                    thread,
                    Box::new(move || {
                        let buffer = buffer.take();
                        BUFFERS.with(|buffers| {
                            let mut buffers = buffers.borrow_mut();
                            if buffer.length() == buffers.size {
                                buffers.spare.push(buffer);
                            }
                        });
                    }),
                );
            }
        });
        // SAFETY: Metal copies the block and calls it once, after the frame completes.
        unsafe { commands.addCompletedHandler(RcBlock::as_ptr(&block)) };
        Ok((commands, sublayers))
    }

    fn encode(
        &self,
        frame: &RenderFrame,
        commands: &ProtocolObject<dyn MTLCommandBuffer>,
        drawable: &ProtocolObject<dyn MTLTexture>,
        instances: Option<&ProtocolObject<dyn MTLBuffer>>,
        wide: bool,
        // Copies that start the frame, drawn when its first pass begins: (source, offset, area).
        prelude: &[Prelude],
    ) -> Result<()> {
        let texture =
            |target: Target| (frame.pooled(&self.pool, target)).map_or(drawable, |texture| texture);
        let pages = self.atlas.pages.borrow();
        let page = |kind, index: Option<u32>| {
            index
                .and_then(|index| pages.get(kind, index))
                .unwrap_or(&self.blank)
        };
        // Slot 1 holds the globals, slot 2 the composite or blur.
        let uniforms =
            |encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>, data: &[u8], slot| {
                let length = data.len();
                let data = NonNull::from(data).cast::<c_void>();
                // SAFETY: Metal copies the bytes before this returns.
                unsafe {
                    encoder.setVertexBytes_length_atIndex(data, length, slot);
                    encoder.setFragmentBytes_length_atIndex(data, length, slot);
                }
            };
        // Snapshots are sRGB, like the canvases that draw them.
        let wide = Cell::new(wide);
        let globals = |target: Target| Globals {
            wide: f32::from(u8::from(wide.get())),
            ..frame.globals(target, 0.)
        };
        // The first pass into the drawable draws the prelude, which covers all of it.
        let pending = Cell::new(!prelude.is_empty());
        let starts = |target: Target| pending.get() && ptr::eq(texture(target), drawable);
        let draw_prelude = |encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>| {
            pending.set(false);
            encoder.setRenderPipelineState(&self.pipelines.copy);
            for &(source, by, area) in prelude {
                self.pipelines.copy_draw(encoder, source, by, area);
            }
            encoder.setScissorRect(MTLScissorRect {
                x: 0,
                y: 0,
                width: drawable.width(),
                height: drawable.height(),
            });
        };
        // Draws stay in the texels of their layer, as a pooled texture can be larger.
        let scissor = |encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>, target: Target| {
            let full = texture(target);
            let [x, y, width, height] = frame
                .scissor(target)
                .unwrap_or([0, 0, u32::MAX, u32::MAX])
                .map(|value| value as usize);
            let [x, y] = [x.min(full.width()), y.min(full.height())];
            encoder.setScissorRect(MTLScissorRect {
                x,
                y,
                width: width.min(full.width() - x),
                height: height.min(full.height() - y),
            });
        };
        let begin = |target: Target, clear: bool| -> Result<_> {
            let descriptor = MTLRenderPassDescriptor::new();
            // SAFETY: index zero is within the color attachment array.
            let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
            attachment.setTexture(Some(texture(target)));
            attachment.setStoreAction(MTLStoreAction::Store);
            let first = starts(target);
            if first && !clear {
                attachment.setLoadAction(MTLLoadAction::DontCare);
            } else if clear {
                attachment.setLoadAction(MTLLoadAction::Clear);
                attachment.setClearColor(MTLClearColor {
                    red: 0.,
                    green: 0.,
                    blue: 0.,
                    alpha: f64::from(u8::from(self.opaque && frame.bases.contains(&target))),
                });
            } else {
                attachment.setLoadAction(MTLLoadAction::Load);
            }
            let encoder = commands
                .renderCommandEncoderWithDescriptor(&descriptor)
                .context("no render encoder")?;
            if first && !clear {
                draw_prelude(&encoder);
            }
            pending.set(pending.get() && !first);
            uniforms(&encoder, bytes(&[globals(target)]), 1);
            scissor(&encoder, target);
            Ok(encoder)
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
        let pipelines = self.pipelines;
        // The target of the open copy pass.
        let mut copying: Option<Target> = None;
        // Only the first pass into it skips the load.
        let mut covered = frame.covered;
        for (at, command) in frame.commands.iter().enumerate() {
            if at == frame.shot_commands && wide.replace(false) {
                end!();
                copying = None;
            }
            if copying.is_some() && !matches!(command, Command::Copy { .. }) {
                end!();
                copying = None;
            }
            match command {
                Command::Pass(target, false)
                    if let Some(encoder) = &encoder
                        && frame.texture(*target) == frame.texture(current.0) =>
                {
                    uniforms(encoder, bytes(&[globals(*target)]), 1);
                    scissor(encoder, *target);
                    current = (*target, false);
                }
                // Targets are cleared right away, since a later pass may draw into them.
                Command::Pass(target, clear) => {
                    end!();
                    current = (*target, *clear);
                    if *clear {
                        pass!();
                    }
                }
                Command::Draw {
                    instances: range,
                    mask_page,
                    color_page,
                } => {
                    let encoder = pass!();
                    encoder.setRenderPipelineState(&pipelines.draw);
                    // SAFETY: the buffer holds this frame's instances, and the command buffer retains the
                    // buffer and the textures.
                    unsafe {
                        encoder.setVertexBuffer_offset_atIndex(instances, 0, 0);
                        encoder
                            .setFragmentTexture_atIndex(Some(page(PageKind::Mask, *mask_page)), 0);
                        encoder.setFragmentTexture_atIndex(
                            Some(page(PageKind::Color, *color_page)),
                            1,
                        );
                        encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                            MTLPrimitiveType::TriangleStrip,
                            0,
                            4,
                            range.len(),
                            range.start as usize,
                        );
                    }
                }
                Command::Blur { src, dst, index } => {
                    end!();
                    let blur = begin(*dst, true)?;
                    blur.setRenderPipelineState(&pipelines.blur);
                    uniforms(&blur, bytes(&frame.blurs[*index as usize..][..1]), 2);
                    // SAFETY: the layer textures live until the command buffer completes.
                    unsafe {
                        blur.setFragmentTexture_atIndex(Some(texture(*src)), 0);
                        blur.drawPrimitives_vertexStart_vertexCount(
                            MTLPrimitiveType::TriangleStrip,
                            0,
                            4,
                        );
                    }
                    blur.endEncoding();
                    current.1 = false;
                }
                Command::Copy { src, dst, region } => {
                    if copying.is_none_or(|open| frame.texture(open) != frame.texture(*dst)) {
                        end!();
                        current.1 = false;
                        // A pooled texture can hold other layers, so the rest of it is loaded.
                        let first = starts(*dst);
                        let whole = covered
                            .take_if(|whole| frame.texture(*whole) == frame.texture(*dst))
                            .is_some();
                        let load = if first || whole {
                            MTLLoadAction::DontCare
                        } else {
                            MTLLoadAction::Load
                        };
                        let open = pipelines.copy_pass(commands, texture(*dst), load)?;
                        if first {
                            draw_prelude(&open);
                            open.setRenderPipelineState(&pipelines.copy);
                        }
                        encoder = Some(open);
                        copying = Some(*dst);
                    }
                    let (from, to) = (frame.origin(*src), frame.origin(*dst));
                    pipelines.copy_draw(
                        encoder.as_deref().unwrap(),
                        texture(*src),
                        [to[0] - from[0], to[1] - from[1]],
                        [region[0] - to[0], region[1] - to[1], region[2], region[3]],
                    );
                }
                Command::Composite {
                    src,
                    mask,
                    backdrop,
                    index,
                } => {
                    let encoder = pass!();
                    encoder.setRenderPipelineState(&pipelines.composite);
                    uniforms(encoder, bytes(&frame.composites[*index as usize..][..1]), 2);
                    let source = texture(*src);
                    // SAFETY: the textures live until the command buffer completes.
                    unsafe {
                        encoder.setFragmentTexture_atIndex(Some(source), 0);
                        encoder.setFragmentTexture_atIndex(
                            Some(match mask {
                                Some(Mask::Page(mask)) => page(PageKind::Color, Some(*mask)),
                                Some(Mask::Layer(mask)) => texture(*mask),
                                None => source,
                            }),
                            1,
                        );
                        encoder
                            .setFragmentTexture_atIndex(Some(backdrop.map_or(source, texture)), 2);
                        encoder.drawPrimitives_vertexStart_vertexCount(
                            MTLPrimitiveType::TriangleStrip,
                            0,
                            4,
                        );
                    }
                }
            }
        }
        end!();
        if pending.get() {
            let open = pipelines.copy_pass(commands, drawable, MTLLoadAction::DontCare)?;
            draw_prelude(&open);
            open.endEncoding();
        }
        Ok(())
    }
}

// Like Chrome's `--force-color-profile`, `APPS_FORCE_COLOR_PROFILE` is `srgb` or `display-p3`.
fn forced_p3() -> Option<bool> {
    static FORCED: OnceLock<Option<bool>> = OnceLock::new();
    *FORCED.get_or_init(
        || match std::env::var("APPS_FORCE_COLOR_PROFILE").as_deref() {
            Ok("srgb") => Some(false),
            Ok("display-p3") => Some(true),
            _ => None,
        },
    )
}

fn color_space(p3: bool) -> &'static CGColorSpace {
    struct Spaces([CFRetained<CGColorSpace>; 2]);
    // SAFETY: color spaces are immutable.
    unsafe impl Send for Spaces {}
    // SAFETY: as above.
    unsafe impl Sync for Spaces {}
    static SPACES: OnceLock<Spaces> = OnceLock::new();
    let spaces = SPACES.get_or_init(|| {
        // SAFETY: constants of Core Graphics.
        Spaces(
            unsafe { [kCGColorSpaceSRGB, kCGColorSpaceDisplayP3] }
                .map(|name| CGColorSpace::with_name(Some(name)).expect("a named color space")),
        )
    });
    &spaces.0[usize::from(p3)]
}

// Like Chrome, a window draws in Display P3 on a wide-gamut screen. Main sets it, and the render thread
// reads it from the layer.
pub(crate) fn set_gamut(layer: &CAMetalLayer, p3: bool) {
    let space = color_space(forced_p3().unwrap_or(p3));
    if !layer.colorspace().is_some_and(|old| ptr::eq(&*old, space)) {
        layer.setColorspace(Some(space));
    }
}

fn metal_layer(
    device: &ProtocolObject<dyn MTLDevice>,
    opaque: bool,
    wide: bool,
) -> Retained<CAMetalLayer> {
    let surface = CAMetalLayer::new();
    surface.setDevice(Some(device));
    surface.setPixelFormat(FORMAT);
    surface.setColorspace(Some(color_space(wide)));
    surface.setFramebufferOnly(false);
    surface.setOpaque(opaque);
    surface
}

pub(crate) struct Pages {
    device: Device,
    shared: bool,
    // Indexed by `PageKind`.
    pages: [PageList<Texture>; 2],
}

impl Pages {
    fn get(&self, kind: PageKind, index: u32) -> Option<&ProtocolObject<dyn MTLTexture>> {
        self.pages[kind as usize]
            .get(index as usize)
            .map(|page| &**page)
    }
}

// Mask pages keep coverage in red, which all backends sample alike.

impl PageStore for Pages {
    fn insert(
        &mut self,
        kind: PageKind,
        size: Size<i32>,
        bytes: &[u8],
    ) -> Result<(usize, Rect<i32>, u32)> {
        let (device, shared) = (&self.device, self.shared);
        let (index, page, rect, id) = self.pages[kind as usize]
            .allocate(size, |size| page_texture(device, shared, kind, size))?;
        upload(page, rect, bytes, bytes.len() / size.height as usize);
        Ok((index, rect, id))
    }

    fn max_side(&self) -> i32 {
        MAX_TEXTURE_SIZE as i32
    }

    fn remove(&mut self, tile: Tile) {
        self.pages[tile.kind as usize].free(tile.page as usize, tile.id);
    }
}

#[cfg(test)]
mod tests;
