use crate::{
    Bounds, DevicePixels, Rgba, ScaledPixels, Size,
    atlas::{Atlas, AtlasBackend, Packer, TextureList, page_size},
    frame::{Command, Globals, RenderFrame, Target, TextParams, bytes},
    scene::{AtlasTextureId, AtlasTextureKind, AtlasTile, Scene},
};
use anyhow::{Context as _, Result, ensure};
use block2::RcBlock;
use objc2::{MainThreadMarker, Message, rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSArray, NSNumber, NSRange, NSString, NSValue, ns_string};
use objc2_metal::{
    MTLBlendFactor, MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLCommandBuffer,
    MTLCommandEncoder, MTLCommandQueue, MTLCopyAllDevices, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLDrawable, MTLGPUFamily, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat,
    MTLPrimitiveType, MTLRegion, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLResourceOptions, MTLScissorRect,
    MTLSize, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use objc2_quartz_core::{
    CACurrentMediaTime, CAKeyframeAnimation, CALayer, CAMediaTiming, CAMetalDrawable, CAMetalLayer,
    CATransaction, CATransform3D, NSValueCATransform3DAdditions, kCAAnimationDiscrete,
    kCAFillModeBoth,
};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem,
    ops::Range,
    ptr::{self, NonNull},
    rc::Rc,
    slice,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;
const MAX_TEXTURE_SIZE: u32 = 16384;
const INSTANCE_BUFFER_ALIGNMENT: usize = 256;
const MIN_INSTANCE_BUFFER_SIZE: usize = 2 * 1024 * 1024;
const MAX_INSTANCE_BUFFER_SIZE: usize = 256 * 1024 * 1024;
const SHRINK_FRAMES: u32 = 120;
const SPARE_FRAMES: u8 = 3;

pub(crate) type MetalAtlas = Atlas<AtlasTextures>;

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type Device = Retained<ProtocolObject<dyn MTLDevice>>;

struct BufferPool {
    size: usize,
    buffers: Vec<(Buffer, usize)>,
    small: u32,
}

thread_local! {
    static POOL: Rc<RefCell<BufferPool>> = Rc::new(RefCell::new(BufferPool {
        size: MIN_INSTANCE_BUFFER_SIZE,
        buffers: Vec::new(),
        small: 0,
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
            MTLResourceOptions::StorageModeShared | MTLResourceOptions::CPUCacheModeWriteCombined
        } else {
            MTLResourceOptions::StorageModeManaged
        };
        let buffer = device
            .newBufferWithLength_options(self.size, options)
            .with_context(|| format!("allocating {}-byte instance buffer", self.size))?;
        Ok((buffer, self.size))
    }

    fn resize(&mut self, size: usize) {
        if size != self.size {
            self.size = size;
            self.buffers.clear();
        }
        self.small = 0;
    }
}

pub(crate) struct Present(Option<super::Sendable<Box<dyn FnOnce()>>>);

impl Present {
    fn new(present: impl FnOnce() + 'static) -> Self {
        Self(Some(super::Sendable::new(Box::new(present))))
    }

    // WARNING: drops the drawables without showing them, so their layers can hand them out again.
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
    size: CGSize,
    unified: bool,
    presents_with_transaction: bool,
    present: Option<Present>,
    opaque: bool,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipelines: Pipelines,
    atlas: Rc<MetalAtlas>,
    pool: Vec<(Texture, [u32; 2], u8)>,
    targets: Vec<usize>,
    frame: RenderFrame,
    promoted: Vec<Promoted>,
    glass: Vec<NativeGlass>,
    glass_update: bool,
    glass_stale: bool,
    caret: Option<(
        Retained<CALayer>,
        Bounds<ScaledPixels>,
        Rgba,
        std::time::Instant,
    )>,
    restacked: bool,
    committed: u64,
    completed: Arc<AtomicU64>,
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
    copy: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

impl Pipelines {
    fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self> {
        #[cfg(not(renderer_shaders))]
        let library = device
            .newLibraryWithSource_options_error(
                &NSString::from_str(include_str!("shaders.metal")),
                Some(&objc2_metal::MTLCompileOptions::new()),
            )
            .context("building Metal library")?;
        #[cfg(renderer_shaders)]
        let library = {
            let compressed: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shaders.metallib"));
            let mut data = Vec::with_capacity(compressed.len() * 3);
            std::io::Read::read_to_end(
                &mut flate2::read::DeflateDecoder::new(compressed),
                &mut data,
            )?;
            device.newLibraryWithData_error(&dispatch2::DispatchData::from_boxed(data.into()))
        }
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
            // WARNING: colors are drawn unpremultiplied over targets that hold premultiplied colors.
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
            copy: pipeline("copy", Blend::Replace)?,
        })
    }

    // WARNING: on Apple GPUs a blit encoder makes the driver hold about 84 MB more until drawing stops, on top of
    // the 126 MB for render encoders, so copies are draws. `offset` goes from a target pixel to its source pixel.
    fn copy(
        &self,
        commands: &ProtocolObject<dyn MTLCommandBuffer>,
        source: &ProtocolObject<dyn MTLTexture>,
        offset: [i32; 2],
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
            .context("creating render command encoder")?;
        encoder.setRenderPipelineState(&self.copy);
        encoder.setScissorRect(MTLScissorRect {
            x: x as usize,
            y: y as usize,
            width: width as usize,
            height: height as usize,
        });
        // SAFETY: Metal copies the offset before this returns, and the command buffer retains the texture.
        unsafe {
            encoder.setFragmentBytes_length_atIndex(
                NonNull::from(&offset).cast::<c_void>(),
                mem::size_of_val(&offset),
                0,
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
        let device = MTLCopyAllDevices()
            .iter()
            .min_by_key(|device| (device.isRemovable(), !device.isLowPower()))
            .or_else(|| MTLCreateSystemDefaultDevice())
            .context("accessing a Metal device")?;
        let srgb = objc2_core_graphics::CGColorSpace::with_name(Some(unsafe {
            objc2_core_graphics::kCGColorSpaceSRGB
        }));
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&device));
        layer.setPixelFormat(FORMAT);
        // WARNING: without a color space the display shows CSS colors in its own wider gamut, more saturated than browsers.
        layer.setColorspace(srgb.as_deref());
        layer.setOpaque(!transparent);
        layer.setMaximumDrawableCount(3);
        layer.setFramebufferOnly(false);
        layer.setAllowsNextDrawableTimeout(false);
        layer.setNeedsDisplayOnBoundsChange(true);
        layer.setAutoresizingMask(
            objc2_quartz_core::CAAutoresizingMask::LayerWidthSizable
                | objc2_quartz_core::CAAutoresizingMask::LayerHeightSizable,
        );
        let unified = device.hasUnifiedMemory();
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
            size: CGSize::new(0., 0.),
            unified,
            presents_with_transaction: false,
            present: None,
            opaque: !transparent,
            queue,
            atlas,
            pool: Vec::new(),
            targets: Vec::new(),
            frame: RenderFrame::default(),
            promoted: Vec::new(),
            glass: Vec::new(),
            glass_update: false,
            glass_stale: false,
            caret: None,
            restacked: false,
            committed: 0,
            completed: Arc::default(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn promote(
        &mut self,
        key: u64,
        index: usize,
        region: [i32; 4],
        step: f64,
        opacities: &[f32],
        transforms: &[[f64; 16]],
    ) -> bool {
        CATransaction::setDisableActions(true);
        let scale = self.layer.contentsScale();
        let [x, y, w, h] = region.map(|value| value as f64 / scale);
        let height = self.layer.bounds().size.height;
        let flipped = self.layer.isGeometryFlipped();
        let mut fresh = false;
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
                let sublayer = CAMetalLayer::new();
                sublayer.setDevice(Some(&self.device));
                sublayer.setPixelFormat(FORMAT);
                sublayer.setColorspace(self.layer.colorspace().as_deref());
                sublayer.setFramebufferOnly(false);
                sublayer.setOpaque(false);
                sublayer.setPresentsWithTransaction(true);
                sublayer.setContentsScale(scale);
                sublayer.setDrawableSize(CGSize::new(region[2] as f64, region[3] as f64));
                sublayer.setAnchorPoint(objc2_core_foundation::CGPoint::new(0., 0.));
                sublayer.setBounds(objc2_core_foundation::CGRect::new(
                    objc2_core_foundation::CGPoint::new(0., 0.),
                    CGSize::new(w, h),
                ));
                sublayer.setPosition(objc2_core_foundation::CGPoint::new(
                    x,
                    if flipped { y } else { height - y - h },
                ));
                self.layer.addSublayer(&sublayer);
                self.promoted.push(Promoted {
                    key,
                    index,
                    sublayer,
                    drawn: false,
                });
                self.restacked = true;
                fresh = true;
                self.promoted.len() - 1
            }
        };
        let sublayer = &self.promoted[at].sublayer;
        let count = opacities.len().max(1);
        let times: Vec<_> = (0..count)
            .map(|k| NSNumber::new_f64(k as f64 / (count - 1).max(1) as f64))
            .collect();
        let times = NSArray::from_retained_slice(&times);
        let begin = CACurrentMediaTime();
        let play = |path: &NSString, values: &NSArray| {
            let animation = CAKeyframeAnimation::animationWithKeyPath(Some(path));
            // SAFETY: the values are what the key path animates: numbers or transform values.
            unsafe { animation.setValues(Some(values)) };
            animation.setKeyTimes(Some(&times));
            animation.setDuration(step * (count - 1) as f64);
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
            // WARNING: the superlayer is y-up unless flipped, and a sublayer turns about its anchor: a
            // sublayer point maps to the window by `place`, a window point to the superlayer by `lift`.
            let mul = |a: &[f64; 16], b: &[f64; 16]| -> [f64; 16] {
                std::array::from_fn(|i| (0..4).map(|k| a[k * 4 + i % 4] * b[i / 4 * 4 + k]).sum())
            };
            let shift = |dx: f64, dy: f64, flip: bool| {
                let mut m = [0.; 16];
                [m[0], m[5], m[10], m[15]] = [1., if flip { -1. } else { 1. }, 1., 1.];
                [m[12], m[13]] = [dx, dy];
                m
            };
            let (place, lift, anchor) = if flipped {
                (
                    shift(x, y, false),
                    shift(0., 0., false),
                    shift(-x, -y, false),
                )
            } else {
                (
                    shift(x, y + h, true),
                    shift(0., height, true),
                    shift(-x, -(height - y - h), false),
                )
            };
            let values: Vec<_> = transforms
                .iter()
                .map(|matrix| {
                    let m = mul(&anchor, &mul(&lift, &mul(matrix, &place)));
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
        fresh
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

    pub fn set_caret(&mut self, caret: Option<(Bounds<ScaledPixels>, Rgba, std::time::Instant)>) {
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
            f64::from(bounds.origin.x.0) / scale,
            f64::from(bounds.origin.y.0) / scale,
            f64::from(bounds.size.width.0) / scale,
            f64::from(bounds.size.height.0) / scale,
        );
        let layer = CALayer::new();
        layer.setAnchorPoint(objc2_core_foundation::CGPoint::new(0., 0.));
        layer.setBounds(objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(0., 0.),
            CGSize::new(w, h),
        ));
        layer.setPosition(objc2_core_foundation::CGPoint::new(
            x,
            if self.layer.isGeometryFlipped() {
                y
            } else {
                self.layer.bounds().size.height - y - h
            },
        ));
        let srgb = objc2_core_graphics::CGColor::new_srgb(
            f64::from(color.r),
            f64::from(color.g),
            f64::from(color.b),
            f64::from(color.a),
        );
        layer.setBackgroundColor(Some(&srgb));
        // WARNING: the timeline starts when the caret last moved, so the blink goes on from the frames that drew it.
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
        self.layer.addSublayer(&layer);
        self.caret = Some((layer, bounds, color, since));
    }

    pub fn atlas(&self) -> &Rc<MetalAtlas> {
        &self.atlas
    }

    pub fn layer(&self) -> &Retained<CAMetalLayer> {
        &self.layer
    }

    // WARNING: sets the size of the next drawables. Only the thread that draws changes it, so a frame always
    // fits its scene.
    pub fn set_drawable_size(&mut self, size: CGSize) {
        self.size = size;
    }

    pub fn take_present(&mut self) -> Option<Present> {
        self.present.take()
    }

    // WARNING: glass views and the frame must change in one Core Animation transaction of the main thread. While
    // AppKit waits for a resize frame, they go back with it, so they show in AppKit's transaction.
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
        if self.presents_with_transaction {
            self.present = Some(update);
            return None;
        }
        Some(update)
    }

    // WARNING: the glass views of a dropped frame never showed, so the next frame updates them even without glass.
    pub fn discard(&mut self, present: Present) {
        present.discard();
        self.glass_stale = true;
    }

    pub fn set_presents_with_transaction(&mut self, on: bool) {
        self.presents_with_transaction = on;
    }

    pub fn update_transparency(&mut self, transparent: bool) {
        self.opaque = !transparent;
        self.layer.setOpaque(!transparent);
    }

    // WARNING: frames often stop for a moment, so this frees only tiles that no frame reads anymore. Buffers,
    // layer textures and spare atlas pages wait for `release`, or each burst of frames would make them again.
    pub fn trim(&mut self) {
        if self.completed.load(Ordering::Acquire) == self.committed {
            self.atlas.free_retired(|| {});
        }
    }

    pub fn release(&mut self) {
        self.atlas.release_images();
        // WARNING: command buffers of one queue complete in order, so an empty one waits for the frames in flight.
        self.atlas.free_retired(|| {
            if let Some(commands) = self.queue.commandBuffer() {
                commands.commit();
                commands.waitUntilCompleted();
            }
        });
        let textures = &mut *self.atlas.backend.borrow_mut();
        textures.mono.trim(|texture| texture.packer.is_empty());
        textures.poly.trim(|texture| texture.packer.is_empty());
        self.pool.clear();
        POOL.with(|pool| {
            let mut pool = pool.borrow_mut();
            pool.resize(MIN_INSTANCE_BUFFER_SIZE);
            pool.buffers.clear();
        });
        // WARNING: the layer frees its spare drawables only when their size changes. It keeps showing its
        // last frame, and the next frame sets the size back.
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
            let mut candidates: Vec<_> = scene.layers[0]
                .effects
                .iter()
                .filter(|primitive| {
                    scene.layers[primitive.layer]
                        .effect
                        .as_ref()
                        .is_some_and(|effect| effect.glass.is_some())
                })
                .collect();
            candidates.sort_unstable_by_key(|primitive| (primitive.order, primitive.layer));
            for primitive in candidates.into_iter().rev() {
                let index = primitive.layer;
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
                    || bounds.intersect(&mask.bounds) != bounds
                    || mask.corner_radii != Default::default()
                    || scene.paints_over(index, bounds, |layer| {
                        glass.iter().any(|native| native.index == layer)
                    })
                    || bounds.size.width.0 > 4096.
                    || bounds.size.height.0 > 4096.
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
                        let sublayer = CAMetalLayer::new();
                        sublayer.setDevice(Some(&self.device));
                        sublayer.setPixelFormat(FORMAT);
                        sublayer.setColorspace(self.layer.colorspace().as_deref());
                        sublayer.setFramebufferOnly(false);
                        sublayer.setOpaque(false);
                        sublayer.setPresentsWithTransaction(true);
                        // WARNING: as many as the window layer, so a frame shown but not yet committed by the
                        // main thread doesn't make the next frame wait for a drawable of this layer.
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
        // WARNING: new or removed sublayers show in the Core Animation transaction, so the frame that
        // draws their layers elsewhere presents in it too.
        let resizing = self.presents_with_transaction;
        let transact = resizing || mem::take(&mut self.restacked) || native_transaction;
        self.layer.setPresentsWithTransaction(transact);
        let Some(drawable) = self.layer.nextDrawable() else {
            eprintln!("apps: no drawable for a {size:?} window");
            return capture.then(|| Err(anyhow::anyhow!("No drawable")));
        };
        let texture = drawable.texture();
        let window = [texture.width() as u32, texture.height() as u32];
        let (commands, sublayers) = match self.render_frame(scene, &texture, window) {
            Ok(commands) => commands,
            Err(error) => {
                eprintln!("apps: failed to render: {error:#}");
                return capture.then(|| Err(error));
            }
        };
        let readback =
            (capture && !native_capture).then(|| self.read_back(&commands, &texture, window));
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
        readback
            .map(|buffer| {
                let buffer = buffer?;
                commands.waitUntilCompleted();
                // SAFETY: the GPU finished writing the shared buffer.
                let bgra = unsafe {
                    slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), buffer.length())
                };
                let mut pixels = vec![0; bgra.len()];
                for (rgba, &[b, g, r, a]) in
                    pixels.as_chunks_mut().0.iter_mut().zip(bgra.as_chunks().0)
                {
                    *rgba = [r, g, b, a];
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
        let mut sublayers = Vec::new();
        if result.is_ok() {
            for &(index, target, region) in &frame.promoted {
                let Target::Layer(layer) = target else {
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
                let source = &self.pool[self.targets[frame.layers[layer].texture]].0;
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
        // WARNING: Metal calls the completion handler on its own thread, so the buffer goes back to
        // the pool in a task of the thread that drew.
        let done = Cell::new(Some(SendBuffer(writer.finish())));
        let thread = super::app_thread();
        self.committed += 1;
        let (frame, completed) = (self.committed, self.completed.clone());
        let block = RcBlock::new(move |_: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            completed.fetch_max(frame, Ordering::Release);
            if let Some(buffer) = done.take() {
                super::post(
                    thread,
                    Box::new(move || {
                        let buffer = buffer.into_inner();
                        let pool = POOL.with(Rc::clone);
                        let mut pool = pool.borrow_mut();
                        if buffer.1 == pool.size {
                            pool.buffers.push(buffer);
                        }
                    }),
                );
            }
        });
        drop(pool);
        // SAFETY: Metal copies the block and calls it once, after the frame completes.
        unsafe { commands.addCompletedHandler(RcBlock::as_ptr(&block)) };
        Ok((commands, sublayers))
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
        for command in &frame.commands {
            match command {
                Command::Pass(target, false)
                    if let Some(encoder) = &encoder
                        && frame.texture(*target) == frame.texture(current.0) =>
                {
                    set_globals(encoder, &frame.globals(*target, &text));
                    current = (*target, false);
                }
                // WARNING: targets are cleared right away, since a later pass may draw into them.
                Command::Pass(target, clear) => {
                    end!();
                    current = (*target, *clear);
                    if *clear {
                        pass!();
                    }
                }
                Command::Quads(range) => draw(pass!(), &pipelines.quads, 0, range),
                Command::Shadows(range) => draw(pass!(), &pipelines.shadows, 1, range),
                Command::Underlines(range) => draw(pass!(), &pipelines.underlines, 2, range),
                Command::Mono(id, range) => {
                    let encoder = pass!();
                    if atlas(encoder, *id) {
                        draw(encoder, &pipelines.mono, 3, range);
                    }
                }
                Command::Poly(id, range) => {
                    let encoder = pass!();
                    if atlas(encoder, *id) {
                        draw(encoder, &pipelines.poly, 4, range);
                    }
                }
                Command::Blur { src, dst, index } => {
                    end!();
                    let blur = begin(*dst, true)?;
                    // SAFETY: the layer textures live until the command buffer completes.
                    unsafe { blur.setFragmentTexture_atIndex(Some(texture(*src)), 0) };
                    draw(&blur, &pipelines.blurs, 6, &(*index..*index + 1));
                    blur.endEncoding();
                    current.1 = false;
                }
                Command::Copy { src, dst, region } => {
                    end!();
                    current.1 = false;
                    let (from, to) = (frame.origin(*src), frame.origin(*dst));
                    // WARNING: a pooled texture can hold other layers, so the rest of it is loaded.
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
        pool.resize(size);
        let buffer = pool.acquire(&self.device, self.unified)?;
        drop(pool);
        let filled = mem::replace(&mut self.current, buffer);
        self.filled.push((filled, self.offset));
        self.offset = 0;
        Ok(())
    }

    fn finish(self) -> (Buffer, usize) {
        let used = self.offset
            + self
                .filled
                .iter()
                .map(|(_, written)| written)
                .sum::<usize>();
        let mut pool = self.pool.borrow_mut();
        if used > pool.size / 4 || pool.size <= MIN_INSTANCE_BUFFER_SIZE {
            pool.small = 0;
        } else if pool.small + 1 < SHRINK_FRAMES {
            pool.small += 1;
        } else {
            let size = pool.size / 2;
            pool.resize(size);
        }
        drop(pool);
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
        // WARNING: Metal retains encoded buffers until the frame completes; only the last one is pooled.
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
                let texture_size = page_size(size);
                let descriptor = MTLTextureDescriptor::new();
                // SAFETY: the atlas keeps both sides within 1..=16384.
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

    fn max_side(&self) -> i32 {
        MAX_TEXTURE_SIZE as i32
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
