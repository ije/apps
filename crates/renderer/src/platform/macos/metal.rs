use crate::{
    Color, Rect, Size,
    atlas::{self, PageKind, PageList, PageStore, Tile},
    frame::{Command, Mask, Pool, RenderFrame, Target},
    gpu::{ATTRIBUTES, Instance, bytes},
    scene::Scene,
};
use anyhow::{Context as _, Result, ensure};
use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::CGSize;
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
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
        [dx, dy]: [i32; 2],
        target: &ProtocolObject<dyn MTLTexture>,
        [x, y, width, height]: [i32; 4],
        load: MTLLoadAction,
    ) -> Result<()> {
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
        encoder.endEncoding();
        Ok(())
    }
}

impl Renderer {
    pub fn new(transparent: bool) -> Result<Self> {
        // All windows share the device and the pipelines. Metal objects are thread-safe.
        static SHARED: OnceLock<(Device, Pipelines)> = OnceLock::new();
        let (device, pipelines) = match SHARED.get() {
            Some(shared) => shared,
            None => {
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
                SHARED.get_or_init(|| (device, pipelines))
            }
        };
        let device = device.clone();
        let layer = metal_layer(&device, !transparent);
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
                let sublayer = metal_layer(&self.device, false);
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
    // refresh. Draws that wait for a drawable in a row tell that the queue is full, a single one
    // does not.
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

    pub fn release(&mut self) {
        self.atlas.release_images();
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
        self.pool.clear();
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
                        let sublayer = metal_layer(&self.device, false);
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
        // Present the frame in the same transaction that adds or removes promoted sublayers.
        let resizing = self.in_transaction;
        let transact = resizing || mem::take(&mut self.restacked) || native_transaction;
        self.layer.setPresentsWithTransaction(transact);
        let asked = Instant::now();
        let next = self.layer.nextDrawable();
        self.waits = match asked.elapsed() > Duration::from_millis(1) {
            true => self.waits.saturating_add(1),
            false => 0,
        };
        let Some(drawable) = next else {
            eprintln!("apps: no drawable for a {size:?} window");
            return capture.then(|| Err(anyhow::anyhow!("No drawable")));
        };
        let frame_texture = drawable.texture();
        let window = [frame_texture.width() as u32, frame_texture.height() as u32];
        let (commands, sublayers) = match self.render_frame(scene, &frame_texture, window) {
            Ok(commands) => commands,
            Err(error) => {
                eprintln!("apps: failed to render: {error:#}");
                return capture.then(|| Err(error));
            }
        };
        let readback =
            (capture && !native_capture).then(|| self.read_back(&commands, &frame_texture, window));
        let shots: Vec<_> = (self.frame.shots.iter())
            .filter_map(|&(key, target, region)| {
                let size = [region[2] as u32, region[3] as u32];
                let layer = self.pool.get(&self.frame, target)?;
                // A blit encoder keeps much memory in the Apple GPU driver, so there a draw copies the layer
                // into a texture that the CPU can read.
                let read = if self.atlas.pages.borrow().shared {
                    new_texture(
                        &self.device,
                        size.map(|side| side as usize),
                        FORMAT,
                        MTLTextureUsage::RenderTarget,
                        MTLStorageMode::Shared,
                    )
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
            match read {
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
    ) -> Result<Rendered> {
        let mut frame = mem::take(&mut self.frame);
        frame.skip.clear();
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
        let result = self.encode(&frame, &commands, drawable, instances.as_deref());
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
        self.frame = frame;
        result?;
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
        let globals = |target: Target| frame.globals(target, 0.);
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
                    alpha: f64::from(u8::from(self.opaque && target == Target::Frame)),
                });
            } else {
                attachment.setLoadAction(MTLLoadAction::Load);
            }
            let encoder = commands
                .renderCommandEncoderWithDescriptor(&descriptor)
                .context("no render encoder")?;
            uniforms(&encoder, bytes(&[globals(target)]), 1);
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
        for command in &frame.commands {
            match command {
                Command::Pass(target, false)
                    if let Some(encoder) = &encoder
                        && frame.texture(*target) == frame.texture(current.0) =>
                {
                    uniforms(encoder, bytes(&[globals(*target)]), 1);
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
                    end!();
                    current.1 = false;
                    let (from, to) = (frame.origin(*src), frame.origin(*dst));
                    // A pooled texture can hold other layers, so the rest of it is loaded.
                    pipelines.copy(
                        commands,
                        texture(*src),
                        [to[0] - from[0], to[1] - from[1]],
                        texture(*dst),
                        [region[0] - to[0], region[1] - to[1], region[2], region[3]],
                        MTLLoadAction::Load,
                    )?;
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
        Ok(())
    }
}

// Every Metal layer draws in sRGB, so wide-gamut displays don't oversaturate CSS colors.
fn metal_layer(device: &ProtocolObject<dyn MTLDevice>, opaque: bool) -> Retained<CAMetalLayer> {
    let surface = CAMetalLayer::new();
    surface.setDevice(Some(device));
    surface.setPixelFormat(FORMAT);
    // SAFETY: a constant of Core Graphics.
    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
    surface.setColorspace(srgb.as_deref());
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
}
