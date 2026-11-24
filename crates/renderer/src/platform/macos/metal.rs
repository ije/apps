use crate::{
    Color, Rect, Size,
    atlas::{self, PageKind, PageList, PageStore, Tile},
    frame::{self, Command, Mask, Pool, RenderFrame, Target},
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

    pub fn promote(
        &mut self,
        key: u64,
        index: usize,
        region: [i32; 4],
        [step, repeat, since]: [f64; 3],
        opacities: &[f32],
        transforms: &[[f64; 16]],
    ) {
        CATransaction::setDisableActions(true);
        let scale = self.layer.contentsScale();
        let [x, y, w, h] = region.map(|value| value as f64 / scale);
        if let Some(at) = self
            .promoted
            .iter()
            .position(|promoted| promoted.key == key && promoted.index != index)
        {
            self.promoted.remove(at).sublayer.removeFromSuperlayer();
        }
        let at = match self
            .promoted
            .iter()
            .position(|promoted| promoted.key == key)
        {
            Some(at) => at,
            None => {
                let sublayer = metal_layer(&self.device, false, self.wide);
                sublayer.setPresentsWithTransaction(true);
                sublayer.setContentsScale(scale);
                sublayer.setDrawableSize(CGSize::new(region[2] as f64, region[3] as f64));
                sublayer.setAnchorPoint(objc2_core_foundation::CGPoint::new(0., 0.));
                sublayer.setBounds(objc2_core_foundation::CGRect::new(
                    objc2_core_foundation::CGPoint::new(0., 0.),
                    CGSize::new(w, h),
                ));
                sublayer.setPosition(objc2_core_foundation::CGPoint::new(x, y));
                // Keep promoted layers below the caret: promotion does not account for it.
                self.overlay.insertSublayer_atIndex(&sublayer, 0);
                self.promoted.push(Promoted {
                    key,
                    index,
                    sublayer,
                    drawn: false,
                });
                self.restacked = true;
                self.promoted.len() - 1
            }
        };
        let sublayer = &self.promoted[at].sublayer;
        let count = opacities.len().max(1);
        let times: Vec<_> = (0..count)
            .map(|k| NSNumber::new_f64(k as f64 / (count - 1).max(1) as f64))
            .collect();
        let times = NSArray::from_retained_slice(&times);
        let begin = CACurrentMediaTime() - since;
        let play = |path: &NSString, values: &NSArray| {
            let animation = CAKeyframeAnimation::animationWithKeyPath(Some(path));
            // SAFETY: the values are what the key path animates: numbers or transform values.
            unsafe { animation.setValues(Some(values)) };
            animation.setKeyTimes(Some(&times));
            animation.setDuration(step * (count - 1) as f64);
            animation.setRepeatCount(repeat as f32);
            animation.setBeginTime(begin);
            // SAFETY: a constant of Core Animation.
            animation.setFillMode(unsafe { kCAFillModeBoth });
            animation.setRemovedOnCompletion(false);
            sublayer.addAnimation_forKey(&animation, Some(path));
        };
        let numbers: Vec<_> = opacities
            .iter()
            .map(|&value| NSNumber::new_f32(value))
            .collect();
        let numbers = NSArray::from_retained_slice(&numbers);
        // SAFETY: an array of numbers is an array of objects.
        play(ns_string!("opacity"), unsafe {
            &*Retained::as_ptr(&numbers).cast::<NSArray>()
        });
        sublayer.setOpacity(opacities.last().copied().unwrap_or(1.));
        if !transforms.is_empty() {
            let mul = |a: &[f64; 16], b: &[f64; 16]| -> [f64; 16] {
                std::array::from_fn(|i| (0..4).map(|k| a[k * 4 + i % 4] * b[i / 4 * 4 + k]).sum())
            };
            let shift = |dx: f64, dy: f64| {
                let mut m = [0.; 16];
                [m[0], m[5], m[10], m[15], m[12], m[13]] = [1., 1., 1., 1., dx, dy];
                m
            };
            let (place, anchor) = (shift(x, y), shift(-x, -y));
            let values: Vec<_> = transforms
                .iter()
                .map(|matrix| {
                    let m = mul(&anchor, &mul(matrix, &place));
                    // SAFETY: a `CATransform3D` is 16 `CGFloat`s, which are `f64`, column by column.
                    let t = unsafe { mem::transmute::<[f64; 16], CATransform3D>(m) };
                    // SAFETY: a plain struct value.
                    unsafe { NSValue::valueWithCATransform3D(t) }
                })
                .collect();
            if let Some(last) = values.last() {
                // SAFETY: the value holds a transform.
                sublayer.setTransform(unsafe { last.CATransform3DValue() });
            }
            let values = NSArray::from_retained_slice(&values);
            // SAFETY: an array of values is an array of objects.
            play(ns_string!("transform"), unsafe {
                &*Retained::as_ptr(&values).cast::<NSArray>()
            });
        }
    }

    pub fn demote(&mut self) {
        if self.promoted.is_empty() {
            return;
        }
        CATransaction::setDisableActions(true);
        for promoted in self.promoted.drain(..) {
            promoted.sublayer.removeFromSuperlayer();
        }
        self.restacked = true;
    }

    pub fn set_caret(&mut self, caret: Option<(Rect<f32>, Color, std::time::Instant)>) {
        if self
            .caret
            .as_ref()
            .map(|(_, bounds, color, since)| (*bounds, *color, *since))
            == caret
        {
            return;
        }
        CATransaction::setDisableActions(true);
        self.restacked = true;
        if let Some((layer, ..)) = self.caret.take() {
            layer.removeFromSuperlayer();
        }
        let Some((bounds, color, since)) = caret else {
            return;
        };
        let scale = self.layer.contentsScale();
        let (x, y, w, h) = (
            f64::from(bounds.origin.x) / scale,
            f64::from(bounds.origin.y) / scale,
            f64::from(bounds.size.width) / scale,
            f64::from(bounds.size.height) / scale,
        );
        let layer = CALayer::new();
        layer.setAnchorPoint(objc2_core_foundation::CGPoint::new(0., 0.));
        layer.setBounds(objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(0., 0.),
            CGSize::new(w, h),
        ));
        layer.setPosition(objc2_core_foundation::CGPoint::new(x, y));
        let srgb = objc2_core_graphics::CGColor::new_srgb(
            f64::from(color.r),
            f64::from(color.g),
            f64::from(color.b),
            f64::from(color.a),
        );
        layer.setBackgroundColor(Some(&srgb));
        let blink = crate::CARET_BLINK.as_secs_f64();
        let animation = CAKeyframeAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
        let values = NSArray::from_retained_slice(&[NSNumber::new_f32(1.), NSNumber::new_f32(0.)]);
        // SAFETY: an array of numbers is an array of objects, which is what this key path animates.
        unsafe { animation.setValues(Some(&*Retained::as_ptr(&values).cast::<NSArray>())) };
        animation.setKeyTimes(Some(&NSArray::from_retained_slice(&[
            NSNumber::new_f64(0.),
            NSNumber::new_f64(0.5),
            NSNumber::new_f64(1.),
        ])));
        // SAFETY: constants of Core Animation.
        animation.setCalculationMode(unsafe { kCAAnimationDiscrete });
        animation.setDuration(blink * 2.);
        animation.setRepeatCount(f32::INFINITY);
        animation.setBeginTime(CACurrentMediaTime() - since.elapsed().as_secs_f64());
        // SAFETY: constants of Core Animation.
        animation.setFillMode(unsafe { kCAFillModeBoth });
        animation.setRemovedOnCompletion(false);
        layer.addAnimation_forKey(&animation, Some(ns_string!("opacity")));
        self.overlay.addSublayer(&layer);
        self.caret = Some((layer, bounds, color, since));
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

    // Present glass and the frame in one main-thread Core Animation transaction, including AppKit's resize
    // transaction.
    pub fn take_glass_update(
        &mut self,
        apply: impl FnOnce(Vec<super::glass::GlassSurface>) + Send + 'static,
    ) -> Option<Present> {
        if !mem::take(&mut self.glass_update) {
            return None;
        }
        self.glass_stale = false;
        let surfaces = super::Sendable::new(
            self.glass
                .iter()
                .map(|glass| glass.surface.clone())
                .collect::<Vec<_>>(),
        );
        let frame = self.present.take().and_then(|mut present| present.0.take());
        let update = Present::new(move || {
            let run = move || {
                apply(surfaces.take());
                if let Some(frame) = frame {
                    frame.take()();
                }
            };
            if MainThreadMarker::new().is_some() {
                run();
            } else {
                super::native_post(Box::new(run));
            }
        });
        if self.in_transaction {
            self.present = Some(update);
            return None;
        }
        Some(update)
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
        self.atlas.release_images(scene);
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
        self.pool.clear(&mut self.frame);
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
            self.pool.clear(&mut self.frame);
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
            for nested in scene.layers[0].nested.iter().rev() {
                let index = nested.layer;
                let layer = &scene.layers[index];
                let Some(effect) = &layer.effect else {
                    continue;
                };
                let (Some(config), Some(bounds)) = (&effect.glass, layer.bounds) else {
                    continue;
                };
                let mask = &effect.content_mask;
                if effect.transform.is_some()
                    || effect.opacity != 1.
                    || effect.color_matrix.is_some()
                    || effect.blur != 0.
                    || effect.shadow.is_some()
                    || effect.blend_mode != crate::BlendMode::Normal
                    || effect.clip.is_some()
                    || effect.mask.is_some()
                    || effect.backdrop.is_some()
                    || bounds != config.bounds
                    || bounds.intersect(mask.bounds) != bounds
                    || mask.radii != Default::default()
                    || scene.paints_over(index, bounds, |layer| {
                        glass.iter().any(|native| native.index == layer)
                    })
                    || bounds.size.width > 4096.
                    || bounds.size.height > 4096.
                {
                    continue;
                }
                let region = crate::frame::region(bounds);
                if region[2] <= 0 || region[3] <= 0 {
                    continue;
                }
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
            }
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
        let damage = (last.as_ref()).and_then(|_| frame::damage_regions(scene, window, &[]));
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
            if let Some(regions) = frame::damage_regions(scene, window, &extra) {
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
                let layer = self.pool.get(&self.frame, target)?;
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
        self.pool.spare(&mut frame.spare);
        frame.record(scene, window, MAX_TEXTURE_SIZE, true);
        let device = &self.device;
        self.pool.fill(&frame, |size| {
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
                let Some(source) = self.pool.get(&frame, target) else {
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
        let texture = |target: Target| {
            self.pool
                .get(frame, target)
                .map_or(drawable, |texture| texture)
        };
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
        let mut blit: Option<Retained<ProtocolObject<dyn MTLBlitCommandEncoder>>> = None;
        for (at, command) in frame.commands.iter().enumerate() {
            if at == frame.shot_commands && wide.replace(false) {
                end!();
                copying = None;
            }
            if copying.is_some() && !matches!(command, Command::Copy { .. }) {
                end!();
                copying = None;
            }
            // A copy between layers blits only its region, since a pass loads and stores all of its target.
            let blits = matches!(command, Command::Copy { src, dst, .. }
                if frame.texture(*src).is_some() && frame.texture(*dst).is_some());
            if !blits && let Some(open) = blit.take() {
                open.endEncoding();
            }
            match command {
                Command::Copy { src, dst, region } if blits => {
                    if blit.is_none() {
                        end!();
                        copying = None;
                        current.1 = false;
                        blit = Some(commands.blitCommandEncoder().context("no blit encoder")?);
                    }
                    let (source, target) = (texture(*src), texture(*dst));
                    let (from, to) = (frame.origin(*src), frame.origin(*dst));
                    // The texels of the region in each texture, cut to both.
                    let [mut x, mut y, mut width, mut height] = *region;
                    for (origin, texture) in [(from, source), (to, target)] {
                        let [left, top] = [x.max(origin[0]), y.max(origin[1])];
                        let right = (x + width).min(origin[0] + texture.width() as i32);
                        let bottom = (y + height).min(origin[1] + texture.height() as i32);
                        [x, y, width, height] = [left, top, right - left, bottom - top];
                    }
                    if width > 0 && height > 0 {
                        let at = |origin: [i32; 2]| MTLOrigin {
                            x: (x - origin[0]) as usize,
                            y: (y - origin[1]) as usize,
                            z: 0,
                        };
                        let size = MTLSize {
                            width: width as usize,
                            height: height as usize,
                            depth: 1,
                        };
                        // SAFETY: the region is inside both textures, which share a format, and the command buffer
                        // retains them.
                        unsafe {
                            blit.as_deref().unwrap().copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                                source, 0, 0, at(from), size, target, 0, 0, at(to),
                            );
                        }
                    }
                }
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
                        let load = if first {
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
        if let Some(open) = blit {
            open.endEncoding();
        }
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

// Sides must be within 1..=16384.
fn new_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    [width, height]: [usize; 2],
    format: MTLPixelFormat,
    usage: MTLTextureUsage,
    storage: MTLStorageMode,
) -> Result<Texture> {
    let spec = MTLTextureDescriptor::new();
    // SAFETY: callers keep both sides within the limits of a 2D texture.
    unsafe {
        spec.setWidth(width);
        spec.setHeight(height);
    }
    spec.setPixelFormat(format);
    spec.setUsage(usage);
    spec.setStorageMode(storage);
    device
        .newTextureWithDescriptor(&spec)
        .with_context(|| format!("no {width}x{height} texture"))
}

fn upload(texture: &ProtocolObject<dyn MTLTexture>, rect: Rect<i32>, bytes: &[u8], row: usize) {
    let at = |value: i32| value as usize;
    // SAFETY: callers pass bytes that cover the rectangle, which lies within the CPU-accessible texture.
    unsafe {
        texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
            MTLRegion {
                origin: MTLOrigin {
                    x: at(rect.origin.x),
                    y: at(rect.origin.y),
                    z: 0,
                },
                size: MTLSize {
                    width: at(rect.size.width),
                    height: at(rect.size.height),
                    depth: 1,
                },
            },
            0,
            NonNull::from(bytes).cast(),
            row,
        );
    }
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
fn page_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    shared: bool,
    kind: PageKind,
    size: Size<i32>,
) -> Result<Texture> {
    new_texture(
        device,
        [size.width as usize, size.height as usize],
        match kind {
            PageKind::Mask => MTLPixelFormat::R8Unorm,
            PageKind::Color => FORMAT,
        },
        MTLTextureUsage::ShaderRead,
        if shared {
            MTLStorageMode::Shared
        } else {
            MTLStorageMode::Managed
        },
    )
}

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
mod tests {
    use crate::{Color, Rect, point, size};
    use objc2::rc::Retained;
    use objc2_quartz_core::{CALayer, CAMediaTiming};

    #[test]
    fn page_sublayers_stay_in_the_overlay_below_the_caret() {
        let mut renderer = super::Renderer::new(false).unwrap();
        let sublayers = |layer: &CALayer| -> Vec<*const CALayer> {
            // SAFETY: a plain layer property.
            unsafe { layer.sublayers() }.map_or_else(Vec::new, |list| {
                list.iter().map(|layer| Retained::as_ptr(&layer)).collect()
            })
        };
        let turn = [
            0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 1., 0., 65., -15., 0., 1.,
        ];
        let since = objc2_quartz_core::CACurrentMediaTime();
        renderer.promote(
            1,
            3,
            [20, 10, 40, 30],
            [0.5, f64::INFINITY, 0.25],
            &[1., 0.5],
            &[turn, turn],
        );
        let caret = Rect {
            origin: point(4., 6.),
            size: size(2., 16.),
        };
        let black = Color {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 1.,
        };
        renderer.set_caret(Some((caret, black, std::time::Instant::now())));
        renderer.promote(
            2,
            4,
            [100, 10, 40, 30],
            [1. / 120., 1., 0.],
            &[1., 0.5],
            &[],
        );
        let caret = Retained::as_ptr(&renderer.caret.as_ref().unwrap().0);
        let promoted: Vec<_> = renderer
            .promoted
            .iter()
            .map(|promoted| Retained::as_ptr(&promoted.sublayer).cast::<CALayer>())
            .collect();
        assert!(
            sublayers(&renderer.layer).is_empty(),
            "AppKit owns the frame layer's sublayers"
        );
        assert_eq!(
            sublayers(&renderer.overlay),
            [promoted[1], promoted[0], caret]
        );
        let first = &renderer.promoted[0].sublayer;
        assert_eq!((first.position().x, first.position().y), (20., 10.));
        let turned = first.transform();
        assert_eq!(
            [
                turned.m11, turned.m12, turned.m21, turned.m22, turned.m41, turned.m42
            ],
            [0., 1., -1., 0., 35., -5.],
            "a turn about the center of the sublayer, from its top left corner"
        );
        // SAFETY: a plain layer property.
        let looped =
            unsafe { first.animationForKey(objc2_foundation::ns_string!("transform")) }.unwrap();
        assert_eq!(
            (looped.repeatCount(), looped.duration()),
            (f32::INFINITY, 0.5)
        );
        assert!(
            (since - 0.25 - looped.beginTime()).abs() < 0.05,
            "it starts at the first sample"
        );
        assert!(renderer.overlay.masksToBounds());
        let position = renderer.caret.as_ref().unwrap().0.position();
        assert_eq!((position.x, position.y), (4., 6.));
        renderer.demote();
        assert_eq!(sublayers(&renderer.overlay), [caret]);
        renderer.set_caret(None);
        assert!(sublayers(&renderer.overlay).is_empty());
    }

    #[test]
    fn opacity_patches_redraw_only_their_region_like_a_full_frame() {
        use crate::{
            Clip,
            gpu::{BOX, Instance, rect},
            scene::{EffectState, Scene, effect},
        };
        use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLCommandQueue};
        let (send, receive) = std::sync::mpsc::channel();
        let job = move || {
            let mut renderer = super::Renderer::new(false).unwrap();
            renderer.set_drawable_size(objc2_core_foundation::CGSize::new(200., 120.));
            let mask = Clip::from(rect([0., 0., 200., 120.]));
            let quad = |bounds: [f32; 4], color: [f32; 4]| {
                let mut instance = Instance::new(BOX, rect(bounds), mask);
                instance.color = color;
                instance
            };
            let mut scene = Scene::default();
            scene.push(quad([0., 0., 200., 120.], [0.9, 0.9, 0.85, 1.]));
            scene.push(quad([150., 10., 40., 40.], [0.2, 0.4, 0.8, 1.]));
            let state = EffectState {
                opacity: 0.8,
                ..effect(mask)
            };
            scene.push_effect(state, Some(7));
            scene.push(quad([20., 20., 30., 30.], [0.9, 0.1, 0.1, 1.]));
            scene.pop_effect();
            scene.push(quad([40., 30., 50., 10.], [0.1, 0.1, 0.1, 0.7]));
            renderer.draw(&scene, false);
            scene.drawn();
            for opacity in [0.3, 0.55] {
                scene.patch(7, opacity, None, [None; 2]);
                renderer.draw(&scene, false);
                scene.drawn();
                let (damage, drawn) = (
                    renderer.frame.damage.clone(),
                    renderer.frame.instances.len(),
                );
                let last = renderer.last.clone().unwrap();
                let commands = renderer.queue.commandBuffer().unwrap();
                let buffer = renderer.read_back(&commands, &last, [200, 120]).unwrap();
                commands.commit();
                commands.waitUntilCompleted();
                // SAFETY: the GPU finished writing the shared buffer.
                let mut pixels = unsafe {
                    std::slice::from_raw_parts(
                        buffer.contents().as_ptr().cast::<u8>(),
                        buffer.length(),
                    )
                }
                .to_vec();
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                let (_, full) = renderer.draw(&scene, true).unwrap().unwrap();
                send.send((damage, drawn, pixels == full)).unwrap();
            }
            super::super::threads::end_thread();
        };
        super::super::threads::spawn_render(job, || {});
        for _ in 0..2 {
            let (damage, drawn, same) = receive.recv().unwrap();
            assert_eq!(damage, [[19, 19, 32, 32]]);
            assert!(drawn < 4, "the far box is not drawn again");
            assert!(same, "the region matches a full frame");
        }
    }

    #[test]
    fn rebuilt_scenes_redraw_only_what_changed_like_a_full_frame() {
        use crate::{
            Clip,
            atlas::TileKey,
            gpu::{BOX, IMAGE, Instance, rect},
            scene::{BackdropState, EffectState, Scene, effect},
        };
        use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLCommandQueue};
        #[derive(Clone, Copy)]
        struct Step {
            name: &'static str,
            box_color: f32,
            box_at: [f32; 2],
            inner: f32,
            opacity: f32,
            image: usize,
            turned: f32,
            shift: f32,
            corner: f32,
        }
        let first = Step {
            name: "first",
            box_color: 0.8,
            box_at: [150., 10.],
            inner: 0.1,
            opacity: 0.8,
            image: 0,
            turned: 0.2,
            shift: 0.,
            corner: 0.2,
        };
        let steps = [
            first,
            Step {
                name: "same",
                ..first
            },
            Step {
                name: "box color",
                box_color: 0.3,
                ..first
            },
            Step {
                name: "moved box",
                box_color: 0.3,
                box_at: [120., 14.],
                ..first
            },
            Step {
                name: "layer content",
                box_color: 0.3,
                box_at: [120., 14.],
                inner: 0.6,
                ..first
            },
            Step {
                name: "layer opacity",
                box_color: 0.3,
                box_at: [120., 14.],
                inner: 0.6,
                opacity: 0.5,
                ..first
            },
            Step {
                name: "image",
                box_color: 0.3,
                box_at: [120., 14.],
                inner: 0.6,
                opacity: 0.5,
                image: 1,
                ..first
            },
            Step {
                name: "turned layer content",
                box_color: 0.3,
                box_at: [120., 14.],
                inner: 0.6,
                opacity: 0.5,
                image: 1,
                turned: 0.9,
                ..first
            },
            Step {
                name: "below a backdrop",
                box_color: 0.3,
                box_at: [100., 50.],
                inner: 0.6,
                opacity: 0.5,
                image: 1,
                turned: 0.9,
                ..first
            },
            Step {
                name: "moved layer",
                box_color: 0.3,
                box_at: [100., 50.],
                inner: 0.6,
                opacity: 0.5,
                image: 1,
                turned: 0.9,
                shift: 3.,
                ..first
            },
            Step {
                name: "part of a shadowed layer",
                box_color: 0.3,
                box_at: [100., 50.],
                inner: 0.6,
                opacity: 0.5,
                image: 1,
                turned: 0.9,
                shift: 3.,
                corner: 0.7,
            },
        ];
        let (send, receive) = std::sync::mpsc::channel();
        let job = move || {
            let mut renderer = super::Renderer::new(false).unwrap();
            renderer.set_drawable_size(objc2_core_foundation::CGSize::new(200., 120.));
            let mask = Clip::from(rect([0., 0., 200., 120.]));
            let quad = |bounds: [f32; 4], color: [f32; 4]| {
                let mut instance = Instance::new(BOX, rect(bounds), mask);
                instance.color = color;
                instance
            };
            let tiles = [[230, 40, 40, 255], [40, 200, 90, 255]].map(|pixel: [u8; 4]| {
                let side = crate::size(4, 4);
                (renderer.atlas())
                    .get_or_insert_with(&TileKey::Image(u64::from(pixel[0])), &mut || {
                        let bytes = pixel.repeat(16).into();
                        Ok(Some((Rect::new(Default::default(), side), side, bytes)))
                    })
                    .unwrap()
                    .0
            });
            let build = |scene: &mut Scene, step: Step| {
                scene.push(quad([0., 0., 200., 120.], [0.9, 0.9, 0.85, 1.]));
                let [x, y] = step.box_at;
                scene.push(quad([x, y, 40., 40.], [0.2, 0.4, step.box_color, 1.]));
                let state = EffectState {
                    opacity: step.opacity,
                    ..effect(mask)
                };
                scene.push_effect(state, Some(7));
                let shift = step.shift;
                let inner = [0.9, step.inner, 0.1, 1.];
                scene.push(quad([20. + shift, 20. + shift, 30., 30.], inner));
                scene.push(quad(
                    [24. + shift, 60. + shift, 20., 10.],
                    [0.1, 0.5, 0.5, 1.],
                ));
                scene.pop_effect();
                let state = EffectState {
                    shadow: Some((
                        crate::point(2., 2.),
                        2.,
                        crate::Color {
                            r: 0.,
                            g: 0.,
                            b: 0.,
                            a: 0.5,
                        },
                    )),
                    ..effect(mask)
                };
                scene.push_effect(state, Some(8));
                scene.push(quad([150., 80., 45., 36.], [0.8, 0.8, 0.8, 1.]));
                scene.push(quad([186., 106., 6., 6.], [step.corner, 0.3, 0.3, 1.]));
                scene.pop_effect();
                // A blend mode reads only the pixel below, so a change below it doesn't draw all of it.
                let state = EffectState {
                    blend_mode: crate::BlendMode::Multiply,
                    ..effect(mask)
                };
                scene.push_effect(state, None);
                scene.push(quad([140., 70., 60., 50.], [0.9, 0.6, 0.9, 0.5]));
                scene.pop_effect();
                scene.push(quad([40., 30., 50., 10.], [0.1, 0.1, 0.1, 0.7]));
                let tile = tiles[step.image];
                let mut image = Instance::new(IMAGE, rect([60., 84., 16., 16.]), mask);
                image.aux = tile.rect.map(|value| value as f32).to_array();
                (image.info[2], image.info[3]) = (1., tile.page as f32);
                scene.push(image);
                let (sin, cos) = 0.3f32.sin_cos();
                let mut turn = crate::frame::IDENTITY;
                [turn[0], turn[1], turn[4], turn[5], turn[12], turn[13]] =
                    [cos, sin, -sin, cos, 10., -20.];
                let state = EffectState {
                    transform: Some(turn),
                    ..effect(mask)
                };
                scene.push_effect(state, None);
                scene.push(quad([150., 60., 20., 20.], [step.turned, 0.2, 0.6, 1.]));
                scene.pop_effect();
                let state = EffectState {
                    backdrop: Some(BackdropState {
                        bounds: rect([96., 64., 50., 40.]),
                        corner_radii: Default::default(),
                        blur: 3.,
                        color_matrix: None,
                    }),
                    ..effect(mask)
                };
                scene.push_effect(state, None);
                scene.push(quad([96., 64., 50., 40.], [1., 1., 1., 0.3]));
                scene.pop_effect();
            };
            let read = |renderer: &super::Renderer| {
                let last = renderer.last.clone().unwrap();
                let commands = renderer.queue.commandBuffer().unwrap();
                let buffer = renderer.read_back(&commands, &last, [200, 120]).unwrap();
                commands.commit();
                commands.waitUntilCompleted();
                // SAFETY: the GPU finished writing the shared buffer.
                let mut pixels = unsafe {
                    std::slice::from_raw_parts(
                        buffer.contents().as_ptr().cast::<u8>(),
                        buffer.length(),
                    )
                }
                .to_vec();
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                pixels
            };
            let mut scene = Scene::default();
            for step in steps {
                scene.clear();
                build(&mut scene, step);
                scene.diff(Some(&[]));
                let committed = renderer.committed;
                renderer.draw(&scene, false);
                scene.drawn();
                if step.name == "part of a shadowed layer" {
                    let frame = &renderer.frame;
                    // The shadow blurs the region of the layer.
                    let blurred = (frame.blurs.iter())
                        .map(|blur| blur.region[2] * blur.region[3])
                        .fold(0., f32::max);
                    assert!(
                        blurred > 0. && blurred < 40. * 30.,
                        "the layer draws whole: {blurred}"
                    );
                }
                let drawn = (renderer.committed != committed).then(|| {
                    let frame = &renderer.frame;
                    (frame.damage.clone(), frame.instances.len())
                });
                let pixels = read(&renderer);
                let (_, full) = renderer.draw(&scene, true).unwrap().unwrap();
                send.send((step.name, drawn, pixels == full)).unwrap();
                // The next rebuild draws again where a patch drew, though its scene draws as before.
                if step.name == "layer opacity" {
                    scene.patch(7, 0.25, None, [None; 2]);
                    renderer.draw(&scene, false);
                    scene.drawn();
                    let drawn = Some((renderer.frame.damage.clone(), 0));
                    let pixels = read(&renderer);
                    let (_, full) = renderer.draw(&scene, true).unwrap().unwrap();
                    send.send(("patched", drawn, pixels == full)).unwrap();
                }
            }
            super::super::threads::end_thread();
        };
        super::super::threads::spawn_render(job, || {});
        for _ in 0..=steps.len() {
            let (name, drawn, same) = receive.recv().unwrap();
            assert!(same, "{name}: the frame matches a full frame");
            let expect: Option<(&[[i32; 4]], usize)> = match name {
                "first" => Some((&[], 11)),
                "same" => None,
                "box color" => Some((&[[149, 9, 42, 42]], 2)),
                "moved box" => Some((&[[119, 9, 72, 46]], 2)),
                // The layer draws its boxes again, the window its background and the bar over it.
                "layer content" => Some((&[[19, 19, 32, 32]], 4)),
                // The layer keeps its pixels.
                "layer opacity" => Some((&[[19, 19, 32, 52]], 2)),
                "patched" => Some((&[[19, 19, 32, 52]], 0)),
                // The patched layer draws again, and joins the image in one region.
                "image" => Some((&[[19, 19, 58, 82]], 3)),
                // The layer turns into the backdrop, which reads all of its region again.
                "turned layer content" => Some((&[[96, 64, 60, 44]], 2)),
                "below a backdrop" => Some((&[[96, 13, 65, 91]], 2)),
                // The layer moves with its pixels.
                "moved layer" => Some((&[[19, 19, 35, 55]], 2)),
                // The layer draws only the part that reaches the region.
                "part of a shadowed layer" => Some((&[[181, 101, 19, 19]], 3)),
                _ => unreachable!(),
            };
            assert_eq!(
                drawn.as_ref().map(|(damage, drawn)| (&damage[..], *drawn)),
                expect,
                "{name}"
            );
        }
    }

    #[test]
    fn returned_drawables_draw_what_changed_since_their_frame() {
        use crate::{
            Clip,
            gpu::{BOX, Instance, rect},
            scene::Scene,
        };
        use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLCommandQueue};
        let (send, receive) = std::sync::mpsc::channel();
        let job = move || {
            let size = objc2_core_foundation::CGSize::new(200., 120.);
            let [mut renderer, mut reference] = [(), ()].map(|_| {
                let mut renderer = super::Renderer::new(false).unwrap();
                renderer.set_drawable_size(size);
                renderer
            });
            let mask = Clip::from(rect([0., 0., 200., 120.]));
            let quad = |bounds: [f32; 4], color: [f32; 4]| {
                let mut instance = Instance::new(BOX, rect(bounds), mask);
                instance.color = color;
                instance
            };
            let mut scene = Scene::default();
            let (mut uncopied, mut widest) = (0, 0);
            for frame in 0..12 {
                scene.clear();
                scene.push(quad([0., 0., 200., 120.], [0.9, 0.9, 0.85, 1.]));
                let x = 10. + 12. * frame as f32;
                scene.push(quad([x, 10., 10., 10.], [0.2, 0.4, 0.8, 1.]));
                let shade = (frame / 3) as f32 / 4.;
                scene.push(quad([150., 80., 20., 20.], [shade, 0.5, 0.2, 1.]));
                scene.diff(Some(&[]));
                renderer.draw(&scene, false);
                scene.drawn();
                let damaged = !renderer.frame.damage.is_empty();
                // A drawable from frames ago draws the steps of the box since then.
                // One frame moves the box 12px, so its own region is 24px wide. A drawable comes back after 3
                // frames at most, so a region never holds more than 3 moves.
                uncopied += u32::from(renderer.frame.damage.iter().any(|region| region[2] > 24));
                widest = (renderer.frame.damage.iter())
                    .fold(widest, |widest, region| widest.max(region[2]));
                let last = renderer.last.clone().unwrap();
                let commands = renderer.queue.commandBuffer().unwrap();
                let buffer = renderer.read_back(&commands, &last, [200, 120]).unwrap();
                commands.commit();
                commands.waitUntilCompleted();
                // SAFETY: the GPU finished writing the shared buffer.
                let mut pixels = unsafe {
                    std::slice::from_raw_parts(
                        buffer.contents().as_ptr().cast::<u8>(),
                        buffer.length(),
                    )
                }
                .to_vec();
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                let (_, full) = reference.draw(&scene, true).unwrap().unwrap();
                send.send((frame, damaged, pixels == full)).unwrap();
            }
            send.send((u32::MAX, uncopied > 4 && widest <= 36, true))
                .unwrap();
            super::super::threads::end_thread();
        };
        super::super::threads::spawn_render(job, || {});
        for _ in 0..=12 {
            let (frame, damaged, same) = receive.recv().unwrap();
            assert!(same, "frame {frame} matches a full frame");
            assert!(damaged || frame == 0, "frame {frame} draws its damage");
        }
    }

    #[test]
    fn scrolled_content_copies_the_last_frame_moved() {
        use crate::{
            Clip,
            gpu::{BOX, Instance, rect},
            point,
            scene::{EffectState, Scene, effect},
        };
        use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLCommandQueue};
        let (send, receive) = std::sync::mpsc::channel();
        // Scroll offsets in device pixels, and a change: 1 and 2 recolor a row (2 one under the bar), 3 the bar. A
        // part of a pixel can't be copied, and a change inside the content draws it. The kept window turns past its
        // viewport height both ways.
        let steps: [(f32, u8); 15] = [
            (0., 0),
            (6., 0),
            (12., 0),
            (30., 0),
            (30.5, 0),
            (40., 0),
            (52., 1),
            (46., 0),
            (20., 0),
            (20., 2),
            (14., 0),
            (14., 3),
            (8., 0),
            (48., 0),
            (88., 0),
        ];
        let job = move || {
            let size = objc2_core_foundation::CGSize::new(200., 120.);
            let [mut renderer, mut reference] = [(), ()].map(|_| {
                let mut renderer = super::Renderer::new(false).unwrap();
                renderer.set_drawable_size(size);
                renderer
            });
            let window = Clip::from(rect([0., 0., 200., 120.]));
            let viewport = Clip::from(rect([10., 10., 180., 100.]));
            let quad = |bounds: [f32; 4], clip: Clip<f32>, color: [f32; 4]| {
                let mut instance = Instance::new(BOX, rect(bounds), clip);
                instance.color = color;
                instance
            };
            let read = |renderer: &super::Renderer| {
                let last = renderer.last.clone().unwrap();
                let commands = renderer.queue.commandBuffer().unwrap();
                let buffer = renderer.read_back(&commands, &last, [200, 120]).unwrap();
                commands.commit();
                commands.waitUntilCompleted();
                // SAFETY: the GPU finished writing the shared buffer.
                let mut pixels = unsafe {
                    std::slice::from_raw_parts(
                        buffer.contents().as_ptr().cast::<u8>(),
                        buffer.length(),
                    )
                }
                .to_vec();
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                pixels
            };
            let mut scene = Scene::default();
            for (offset, changed) in steps {
                scene.clear();
                scene.push(quad([0., 0., 200., 120.], window, [0.9, 0.9, 0.85, 1.]));
                scene.begin_scroll(7, viewport, point(0., offset), false);
                for row in 0..12 {
                    let y = 14. + 22. * row as f32 - offset;
                    let shade = row as f32 / 12.;
                    let color = match (changed, row) {
                        (1, 4) | (2, 1) => [0.9, 0.1, 0.1, 1.],
                        _ => [shade, 0.4, 1. - shade, 1.],
                    };
                    // A composited opacity animation, which patch frames draw without a new diff.
                    if row == 3 {
                        let state = EffectState {
                            opacity: 0.8,
                            ..effect(viewport)
                        };
                        scene.push_effect(state, Some(9));
                    }
                    scene.push(quad([20., y, 120., 16.], viewport, color));
                    if row == 3 {
                        scene.pop_effect();
                    }
                    scene.push(quad(
                        [150., y + 4., 30., 8.],
                        viewport,
                        [0.2, 0.2, 0.2, 0.5],
                    ));
                }
                // A bar that stays over the top of the viewport, painted inside it as a fixed box is.
                let bar = match changed {
                    3 => [0.6, 0.2, 0.5, 0.7],
                    _ => [0.1, 0.5, 0.3, 0.8],
                };
                scene.pause_cuts(0);
                scene.push(quad([0., 0., 200., 24.], window, bar));
                scene.resume_cuts();
                // Content after the bar, as a scroll bar is.
                scene.push(quad(
                    [184., 60. - offset, 6., 40.],
                    viewport,
                    [0.3, 0.3, 0.3, 0.6],
                ));
                scene.end_scroll();
                scene.diff(Some(&[]));
                let shifted = scene.shift;
                renderer.draw(&scene, false);
                scene.drawn();
                let kept = renderer.frame.keeps_under_now();
                let area: i32 = renderer
                    .frame
                    .damage
                    .iter()
                    .map(|region| region[2] * region[3])
                    .sum();
                let pixels = read(&renderer);
                let (_, full) = reference.draw(&scene, true).unwrap().unwrap();
                // A patch frame after the scroll must not copy the scroll again.
                assert!(scene.patch(9, 0.4, None, [None; 2]));
                renderer.draw(&scene, false);
                scene.drawn();
                let patched = read(&renderer);
                let (_, patched_full) = reference.draw(&scene, true).unwrap().unwrap();
                send.send((
                    offset,
                    shifted,
                    area,
                    pixels == full,
                    kept,
                    patched == patched_full,
                ))
                .unwrap();
            }
            super::super::threads::end_thread();
        };
        super::super::threads::spawn_render(job, || {});
        let mut last = 0.;
        for (at, (offset, changed)) in steps.into_iter().enumerate() {
            let (got, shifted, area, same, kept, patched) = receive.recv().unwrap();
            assert_eq!(got, offset);
            assert!(
                same,
                "offset {offset}, change {changed}: the frame matches a full frame"
            );
            assert!(
                patched,
                "offset {offset}, change {changed}: the patch frame matches a full frame"
            );
            // The first scroll keeps the window below the bar, and later frames keep it up to date.
            assert_eq!(kept, at > 0, "offset {offset}, change {changed}");
            let by = offset - last;
            let copies = by != 0. && by.fract() == 0. && last.fract() == 0.;
            assert_eq!(
                shifted.map(|(_, by)| by),
                copies.then_some([0, by as i32]),
                "offset {offset}"
            );
            if copies && changed == 0 {
                // The edge, and the bar where it is and where the copy put it.
                assert!(
                    area <= 200 * (24 + 2) * 2 + 182 * (by.abs() as i32 + 2),
                    "offset {offset}: {area}"
                );
            }
            last = offset;
        }
    }
}
