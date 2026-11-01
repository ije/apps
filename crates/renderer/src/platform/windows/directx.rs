//! Direct3D 11 rendering into DirectComposition swap chains. All windows share one device,
//! one set of pipelines and one atlas.

use crate::{
    DevicePixels, Rgba, Size,
    atlas::{Atlas, AtlasBackend, MIN_TEXTURE_SIDE, Packer, TextureList},
    frame::{Cmd, RenderFrame, Target, TextParams, bytes},
    scene::{AtlasTextureId, AtlasTextureKind, AtlasTile, Scene},
};
use anyhow::{Context as _, Result, ensure};
use std::{borrow::Cow, cell::RefCell, mem, ops::Range, rc::Rc, slice, time::Duration};
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            DirectComposition::*,
            DirectWrite::*,
            Dxgi::{Common::*, *},
        },
    },
    core::Interface,
};

const FORMAT: DXGI_FORMAT = DXGI_FORMAT_B8G8R8A8_UNORM;
const BUFFERS: u32 = 2;
const MAX_INSTANCE_BUFFER_SIZE: usize = 256 * 1024 * 1024;

pub(crate) type DirectXAtlas = Atlas<AtlasTextures>;

thread_local! {
    static GPU: RefCell<Option<Rc<Gpu>>> = const { RefCell::new(None) };
}

/// The current device, made again after the old one was lost.
fn gpu() -> Result<Rc<Gpu>> {
    GPU.with_borrow_mut(|slot| {
        // SAFETY: plain device query.
        if let Some(gpu) = slot.as_ref()
            && unsafe { gpu.device.GetDeviceRemovedReason() }.is_ok()
        {
            return Ok(gpu.clone());
        }
        let gpu = Rc::new(Gpu::new(slot.take().map(|lost| lost.atlas.clone()))?);
        *slot = Some(gpu.clone());
        Ok(gpu)
    })
}

#[cfg(renderer_shaders)]
macro_rules! shader {
    ($name:literal) => {
        Cow::Borrowed(include_bytes!(concat!(env!("OUT_DIR"), "/", $name, ".cso")).as_slice())
    };
}

#[cfg(not(renderer_shaders))]
macro_rules! shader {
    ($name:literal) => {
        compile($name)?
    };
}

/// Compiles a shader at launch, when the build had no `fxc`.
#[cfg(not(renderer_shaders))]
fn compile(entry: &str) -> Result<Cow<'static, [u8]>> {
    use windows::{Win32::Graphics::Direct3D::Fxc::D3DCompile, core::PCSTR};
    const SOURCE: &str = include_str!("shaders.hlsl");
    let name = format!("{entry}\0");
    let target: &[u8] = if entry.ends_with("_vertex") {
        b"vs_4_1\0"
    } else {
        b"ps_4_1\0"
    };
    let (mut code, mut errors) = (None, None);
    // SAFETY: the source and the NUL-terminated names outlive the call.
    let result = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            PCSTR::null(),
            None,
            None::<&ID3DInclude>,
            PCSTR(name.as_ptr()),
            PCSTR(target.as_ptr()),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    // SAFETY: blobs hold their size in bytes.
    let blob = |blob: &ID3DBlob| unsafe {
        slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    };
    if let Err(error) = result {
        let message = errors
            .as_ref()
            .map(|errors| String::from_utf8_lossy(blob(errors)).into_owned());
        anyhow::bail!(
            "compiling {entry}: {}",
            message.unwrap_or_else(|| error.to_string())
        );
    }
    Ok(Cow::Owned(
        blob(&code.context("missing shader code")?).to_vec(),
    ))
}

struct Pipeline {
    vertex: ID3D11VertexShader,
    fragment: ID3D11PixelShader,
    blend: ID3D11BlendState,
}

/// An instance buffer, grown to fit the largest frame.
struct Instances {
    buffer: ID3D11Buffer,
    view: Option<ID3D11ShaderResourceView>,
    len: usize,
}

const QUADS: usize = 0;
const SHADOWS: usize = 1;
const UNDERLINES: usize = 2;
const MONO: usize = 3;
const POLY: usize = 4;
const COMPOSITES: usize = 5;
const BLURS: usize = 6;

struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    factory: IDXGIFactory2,
    composition: IDCompositionDevice,
    max_texture: u32,
    pipelines: [Pipeline; 7],
    instances: RefCell<[Option<Instances>; 7]>,
    globals: Option<ID3D11Buffer>,
    batch: Option<ID3D11Buffer>,
    sampler: Option<ID3D11SamplerState>,
    rasterizer: ID3D11RasterizerState,
    atlas: Rc<DirectXAtlas>,
    text: TextParams,
}

impl Gpu {
    fn new(atlas: Option<Rc<DirectXAtlas>>) -> Result<Self> {
        let create = |driver: D3D_DRIVER_TYPE| -> Result<_> {
            let (mut device, mut context, mut level) = (None, None, D3D_FEATURE_LEVEL::default());
            // SAFETY: the out pointers are locals.
            unsafe {
                D3D11CreateDevice(
                    None::<&IDXGIAdapter>,
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&[
                        D3D_FEATURE_LEVEL_11_1,
                        D3D_FEATURE_LEVEL_11_0,
                        D3D_FEATURE_LEVEL_10_1,
                    ]),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    Some(&mut level),
                    Some(&mut context),
                )?;
            }
            let device: ID3D11Device = device.context("missing device")?;
            let mut options = D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS::default();
            // SAFETY: the data matches the feature.
            unsafe {
                device.CheckFeatureSupport(
                    D3D11_FEATURE_D3D10_X_HARDWARE_OPTIONS,
                    (&raw mut options).cast(),
                    size_of_val(&options) as u32,
                )?;
            }
            ensure!(
                level.0 >= D3D_FEATURE_LEVEL_11_0.0
                    || options
                        .ComputeShaders_Plus_RawAndStructuredBuffers_Via_Shader_4_x
                        .as_bool(),
                "the GPU has no structured buffers"
            );
            Ok((device, context.context("missing device context")?, level))
        };
        // Software rendering keeps the app usable without a GPU driver, like in some VMs.
        let (device, context, level) =
            create(D3D_DRIVER_TYPE_HARDWARE).or_else(|_| create(D3D_DRIVER_TYPE_WARP))?;
        // SAFETY: plain COM calls on the new device.
        unsafe {
            let dxgi: IDXGIDevice = device.cast()?;
            let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let blend = |source: Option<D3D11_BLEND>| -> Result<ID3D11BlendState> {
                let mut desc = D3D11_BLEND_DESC::default();
                let target = &mut desc.RenderTarget[0];
                target.RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8;
                // Colors are drawn unpremultiplied over targets that hold premultiplied colors.
                if let Some(source) = source {
                    target.BlendEnable = true.into();
                    target.SrcBlend = source;
                    target.SrcBlendAlpha = D3D11_BLEND_ONE;
                    target.DestBlend = D3D11_BLEND_INV_SRC_ALPHA;
                    target.DestBlendAlpha = D3D11_BLEND_INV_SRC_ALPHA;
                    target.BlendOp = D3D11_BLEND_OP_ADD;
                    target.BlendOpAlpha = D3D11_BLEND_OP_ADD;
                }
                let mut state = None;
                device.CreateBlendState(&desc, Some(&mut state))?;
                state.context("missing blend state")
            };
            let (over, premultiplied, replace) = (
                blend(Some(D3D11_BLEND_SRC_ALPHA))?,
                blend(Some(D3D11_BLEND_ONE))?,
                blend(None)?,
            );
            let pipeline = |vertex: Cow<[u8]>,
                            fragment: Cow<[u8]>,
                            blend: &ID3D11BlendState|
             -> Result<Pipeline> {
                let (mut vs, mut ps) = (None, None);
                device.CreateVertexShader(&vertex, None, Some(&mut vs))?;
                device.CreatePixelShader(&fragment, None, Some(&mut ps))?;
                Ok(Pipeline {
                    vertex: vs.context("missing vertex shader")?,
                    fragment: ps.context("missing pixel shader")?,
                    blend: blend.clone(),
                })
            };
            let pipelines = [
                pipeline(shader!("quad_vertex"), shader!("quad_fragment"), &over)?,
                pipeline(shader!("shadow_vertex"), shader!("shadow_fragment"), &over)?,
                pipeline(
                    shader!("underline_vertex"),
                    shader!("underline_fragment"),
                    &over,
                )?,
                pipeline(
                    shader!("monochrome_sprite_vertex"),
                    shader!("monochrome_sprite_fragment"),
                    &over,
                )?,
                pipeline(
                    shader!("polychrome_sprite_vertex"),
                    shader!("polychrome_sprite_fragment"),
                    &over,
                )?,
                pipeline(
                    shader!("composite_vertex"),
                    shader!("composite_fragment"),
                    &premultiplied,
                )?,
                pipeline(shader!("blur_vertex"), shader!("blur_fragment"), &replace)?,
            ];
            let constants = |size: usize| -> Result<Option<ID3D11Buffer>> {
                let desc = D3D11_BUFFER_DESC {
                    ByteWidth: size as u32,
                    Usage: D3D11_USAGE_DYNAMIC,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                    ..Default::default()
                };
                let mut buffer = None;
                device.CreateBuffer(&desc, None, Some(&mut buffer))?;
                Ok(buffer)
            };
            let mut sampler = None;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    ComparisonFunc: D3D11_COMPARISON_ALWAYS,
                    MaxAnisotropy: 1,
                    MaxLOD: D3D11_FLOAT32_MAX,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            let mut rasterizer = None;
            device.CreateRasterizerState(
                &D3D11_RASTERIZER_DESC {
                    FillMode: D3D11_FILL_SOLID,
                    CullMode: D3D11_CULL_NONE,
                    DepthClipEnable: true.into(),
                    ..Default::default()
                },
                Some(&mut rasterizer),
            )?;
            let max_texture = if level.0 >= D3D_FEATURE_LEVEL_11_0.0 {
                16384
            } else {
                8192
            };
            let textures = AtlasTextures {
                device: device.clone(),
                context: context.clone(),
                max: max_texture as i32,
                mono: TextureList::default(),
                poly: TextureList::default(),
            };
            let atlas = match atlas {
                Some(atlas) => {
                    atlas.clear(|old| *old = textures);
                    atlas
                }
                None => Rc::new(Atlas::new(textures)),
            };
            let write: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let params: IDWriteRenderingParams1 = write.CreateRenderingParams()?.cast()?;
            Ok(Self {
                pipelines,
                instances: RefCell::default(),
                globals: constants(size_of::<crate::frame::Globals>())?,
                batch: constants(16)?,
                sampler,
                rasterizer: rasterizer.context("missing rasterizer state")?,
                atlas,
                text: TextParams {
                    gamma_ratios: gamma_ratios(params.GetGamma()),
                    grayscale_enhanced_contrast: params.GetGrayscaleEnhancedContrast(),
                    subpixel_enhanced_contrast: params.GetEnhancedContrast(),
                    is_bgr: params.GetPixelGeometry() == DWRITE_PIXEL_GEOMETRY_BGR,
                },
                device,
                context,
                factory,
                composition,
                max_texture,
            })
        }
    }

    /// Writes this frame's instances of one kind and returns their view.
    fn upload(
        &self,
        kind: usize,
        data: &[u8],
        stride: usize,
    ) -> Result<Option<ID3D11ShaderResourceView>> {
        if data.is_empty() {
            return Ok(None);
        }
        let mut instances = self.instances.borrow_mut();
        let slot = &mut instances[kind];
        let count = data.len() / stride;
        if slot.as_ref().is_none_or(|instances| instances.len < count) {
            ensure!(
                data.len() <= MAX_INSTANCE_BUFFER_SIZE,
                "frame instance data exceeds {MAX_INSTANCE_BUFFER_SIZE} bytes"
            );
            let len = count.next_power_of_two().max(16);
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: (len * stride) as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
                StructureByteStride: stride as u32,
            };
            let (mut buffer, mut view) = (None, None);
            // SAFETY: plain resource creation.
            unsafe {
                self.device.CreateBuffer(&desc, None, Some(&mut buffer))?;
                let buffer = buffer.as_ref().context("missing instance buffer")?;
                self.device
                    .CreateShaderResourceView(buffer, None, Some(&mut view))?;
            }
            *slot = Some(Instances {
                buffer: buffer.context("missing instance buffer")?,
                view,
                len,
            });
        }
        let instances = slot.as_ref().context("missing instance buffer")?;
        write(&self.context, &instances.buffer, data)?;
        Ok(instances.view.clone())
    }
}

/// Replaces the contents of a dynamic buffer.
fn write(context: &ID3D11DeviceContext, buffer: &ID3D11Buffer, data: &[u8]) -> Result<()> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: the buffer is at least `data.len()` bytes.
    unsafe {
        context.Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), mapped.pData.cast(), data.len());
        context.Unmap(buffer, 0);
    }
    Ok(())
}

/// The alpha correction ratios for a DirectWrite gamma, as in Windows Terminal.
fn gamma_ratios(gamma: f32) -> [f32; 4] {
    const RATIOS: [[f32; 4]; 13] = [
        [0.0000, 0.0000, 0.0000, 0.0000],
        [0.0166, -0.0807, 0.2227, -0.0751],
        [0.0350, -0.1760, 0.4325, -0.1370],
        [0.0543, -0.2821, 0.6302, -0.1876],
        [0.0739, -0.3963, 0.8167, -0.2287],
        [0.0933, -0.5161, 0.9926, -0.2616],
        [0.1121, -0.6395, 1.1588, -0.2877],
        [0.1300, -0.7649, 1.3159, -0.3080],
        [0.1469, -0.8911, 1.4644, -0.3234],
        [0.1627, -1.0170, 1.6051, -0.3347],
        [0.1773, -1.1420, 1.7385, -0.3426],
        [0.1908, -1.2652, 1.8650, -0.3476],
        [0.2031, -1.3864, 1.9851, -0.3501],
    ];
    const NORM13: f32 = ((0x10000 as f64) / (255.0 * 255.0)) as f32;
    const NORM24: f32 = ((0x100 as f64) / 255.0) as f32;
    let ratios = RATIOS[((gamma * 10.0).round() as usize).clamp(10, 22) - 10];
    [
        ratios[0] * NORM13,
        ratios[1] * NORM24,
        ratios[2] * NORM13,
        ratios[3] * NORM24,
    ]
}

/// An offscreen layer texture.
struct Layer {
    texture: ID3D11Texture2D,
    view: Option<ID3D11RenderTargetView>,
    srv: Option<ID3D11ShaderResourceView>,
    size: [u32; 2],
}

/// The swap chain of a window and its DirectComposition visual.
struct Surface {
    swap_chain: IDXGISwapChain1,
    /// The back buffer and its view, released while the swap chain resizes.
    back: Option<(ID3D11Texture2D, Option<ID3D11RenderTargetView>)>,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
}

impl Surface {
    fn new(gpu: &Gpu, hwnd: HWND, size: [u32; 2], opaque: bool) -> Result<Self> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size[0],
            Height: size[1],
            Format: FORMAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: BUFFERS,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: if opaque {
                DXGI_ALPHA_MODE_IGNORE
            } else {
                DXGI_ALPHA_MODE_PREMULTIPLIED
            },
            ..Default::default()
        };
        // SAFETY: plain COM calls; the window outlives its surface.
        unsafe {
            let swap_chain = gpu
                .factory
                .CreateSwapChainForComposition(&gpu.device, &desc, None)?;
            // Below child windows, like embedded web views.
            let target = gpu.composition.CreateTargetForHwnd(hwnd, false)?;
            let visual = gpu.composition.CreateVisual()?;
            visual.SetContent(&swap_chain)?;
            target.SetRoot(&visual)?;
            gpu.composition.Commit()?;
            Ok(Self {
                back: Some(back_buffer(gpu, &swap_chain)?),
                swap_chain,
                _target: target,
                _visual: visual,
            })
        }
    }
}

fn back_buffer(
    gpu: &Gpu,
    swap_chain: &IDXGISwapChain1,
) -> Result<(ID3D11Texture2D, Option<ID3D11RenderTargetView>)> {
    // SAFETY: plain COM calls.
    unsafe {
        let texture: ID3D11Texture2D = swap_chain.GetBuffer(0)?;
        let mut view = None;
        gpu.device
            .CreateRenderTargetView(&texture, None, Some(&mut view))?;
        Ok((texture, view))
    }
}

pub(crate) struct Renderer {
    gpu: Rc<Gpu>,
    hwnd: HWND,
    opaque: bool,
    clear: [f32; 4],
    size: [u32; 2],
    surface: Option<Surface>,
    /// The offscreen layer textures, kept across frames.
    pool: Vec<Layer>,
    frame: RenderFrame,
}

impl Renderer {
    pub fn new(hwnd: HWND, size: [u32; 2], transparent: bool) -> Result<Self> {
        let gpu = gpu()?;
        let size = size.map(|side| side.max(1));
        let surface = Surface::new(&gpu, hwnd, size, !transparent)?;
        Ok(Self {
            gpu,
            hwnd,
            opaque: !transparent,
            clear: [0., 0., 0., if transparent { 0. } else { 1. }],
            size,
            surface: Some(surface),
            pool: Vec::new(),
            frame: RenderFrame::default(),
        })
    }

    pub fn atlas(&self) -> &Rc<DirectXAtlas> {
        &self.gpu.atlas
    }

    /// The color behind the content.
    pub fn set_background(&mut self, color: Rgba) {
        self.clear = if self.opaque {
            [color.r, color.g, color.b, 1.]
        } else {
            [
                color.r * color.a,
                color.g * color.a,
                color.b * color.a,
                color.a,
            ]
        };
    }

    pub fn resize(&mut self, size: [u32; 2]) -> Result<()> {
        let size = size.map(|side| side.max(1));
        if size == self.size {
            return Ok(());
        }
        self.size = size;
        let Some(surface) = &mut self.surface else {
            return Ok(());
        };
        surface.back = None;
        // SAFETY: the back buffer and its view were released.
        unsafe {
            self.gpu.context.OMSetRenderTargets(None, None);
            surface.swap_chain.ResizeBuffers(
                BUFFERS,
                size[0],
                size[1],
                FORMAT,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        surface.back = Some(back_buffer(&self.gpu, &surface.swap_chain)?);
        Ok(())
    }

    /// Draws the scene. With `capture`, also returns the frame's size and RGBA rows.
    pub fn draw(&mut self, scene: &Scene, capture: bool) -> Option<Result<([u32; 2], Vec<u8>)>> {
        match self.render(scene, capture) {
            Ok(pixels) => pixels.map(|pixels| Ok((self.size, pixels))),
            Err(error) => {
                eprintln!("apps: failed to render: {error:#}");
                // SAFETY: plain device query.
                if unsafe { self.gpu.device.GetDeviceRemovedReason() }.is_err() {
                    // The next frames draw with a new device, once the driver is back.
                    self.surface = None;
                    super::run_after(Duration::from_millis(100), || {
                        crate::app::update(|app| {
                            for id in app.window_ids() {
                                app.refresh_window(id);
                            }
                        })
                    });
                }
                capture.then_some(Err(error))
            }
        }
    }

    fn render(&mut self, scene: &Scene, capture: bool) -> Result<Option<Vec<u8>>> {
        let gpu = gpu()?;
        if !Rc::ptr_eq(&gpu, &self.gpu) || self.surface.is_none() {
            // WARNING: a window has one composition target, so the old one goes first.
            self.surface = None;
            self.pool.clear();
            self.gpu = gpu;
            self.surface = Some(Surface::new(&self.gpu, self.hwnd, self.size, self.opaque)?);
        }
        let mut frame = mem::take(&mut self.frame);
        frame.record(scene, self.size, self.gpu.max_texture, true);
        let result = self.encode(&frame);
        self.frame = frame;
        result?;
        let surface = self.surface.as_ref().context("missing surface")?;
        let pixels = capture
            .then(|| {
                read_back(
                    &self.gpu,
                    &surface.back.as_ref().context("missing back buffer")?.0,
                    self.size,
                )
            })
            .transpose()?;
        // SAFETY: presents the drawn back buffer; frames are paced by the vsync thread.
        unsafe { surface.swap_chain.Present(0, DXGI_PRESENT(0)).ok()? };
        Ok(pixels)
    }

    fn encode(&mut self, frame: &RenderFrame) -> Result<()> {
        let gpu = &*self.gpu;
        let context = &gpu.context;
        for (index, size) in frame.textures.iter().enumerate() {
            if let Some(found) = self.pool[index..]
                .iter()
                .position(|layer| layer.size == *size)
            {
                self.pool.swap(index, index + found);
                continue;
            }
            let desc = D3D11_TEXTURE2D_DESC {
                Width: size[0],
                Height: size[1],
                MipLevels: 1,
                ArraySize: 1,
                Format: FORMAT,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                ..Default::default()
            };
            let (mut texture, mut view, mut srv) = (None, None, None);
            // SAFETY: plain resource creation.
            unsafe {
                gpu.device
                    .CreateTexture2D(&desc, None, Some(&mut texture))?;
                let texture = texture.as_ref().context("missing layer texture")?;
                gpu.device
                    .CreateRenderTargetView(texture, None, Some(&mut view))?;
                gpu.device
                    .CreateShaderResourceView(texture, None, Some(&mut srv))?;
            }
            self.pool.push(Layer {
                texture: texture.context("missing layer texture")?,
                view,
                srv,
                size: *size,
            });
            let last = self.pool.len() - 1;
            self.pool.swap(index, last);
        }
        self.pool.truncate(frame.textures.len());
        let views = [
            gpu.upload(QUADS, bytes(&frame.quads), size_of::<crate::scene::Quad>())?,
            gpu.upload(
                SHADOWS,
                bytes(&frame.shadows),
                size_of::<crate::scene::Shadow>(),
            )?,
            gpu.upload(
                UNDERLINES,
                bytes(&frame.underlines),
                size_of::<crate::scene::Underline>(),
            )?,
            gpu.upload(
                MONO,
                bytes(&frame.mono),
                size_of::<crate::scene::MonochromeSprite>(),
            )?,
            gpu.upload(
                POLY,
                bytes(&frame.poly),
                size_of::<crate::scene::PolychromeSprite>(),
            )?,
            gpu.upload(
                COMPOSITES,
                bytes(&frame.composites),
                size_of::<crate::frame::Composite>(),
            )?,
            gpu.upload(BLURS, bytes(&frame.blurs), size_of::<crate::frame::Blur>())?,
        ];
        let back = self
            .surface
            .as_ref()
            .and_then(|surface| surface.back.as_ref())
            .context("missing back buffer")?;
        let pool = &self.pool;
        let view = |target: Target| match target {
            Target::Frame => &back.1,
            Target::Layer(index) => &pool[frame.layers[index].texture].view,
        };
        let srv = |target: Target| match target {
            Target::Frame => None,
            Target::Layer(index) => pool[frame.layers[index].texture].srv.clone(),
        };
        let texture = |target: Target| -> &ID3D11Texture2D {
            match target {
                Target::Frame => &back.0,
                Target::Layer(index) => &pool[frame.layers[index].texture].texture,
            }
        };
        let globals = gpu.globals.as_ref().context("missing globals buffer")?;
        let batch = gpu.batch.as_ref().context("missing batch buffer")?;
        // Binds a render target with its viewport and globals.
        let bind = |target: Target| -> Result<()> {
            let rect = frame.rect(target);
            let viewport = D3D11_VIEWPORT {
                Width: rect[2],
                Height: rect[3],
                MaxDepth: 1.,
                ..Default::default()
            };
            // SAFETY: plain state changes on live resources.
            unsafe {
                context.VSSetShaderResources(0, Some(&[None]));
                context.PSSetShaderResources(0, Some(&[None, None, None, None]));
                context.OMSetRenderTargets(Some(slice::from_ref(view(target))), None);
                context.RSSetViewports(Some(&[viewport]));
            }
            write(context, globals, bytes(&[frame.globals(target, &gpu.text)]))
        };
        let clear = |target: Target| {
            let color = if target == Target::Frame {
                self.clear
            } else {
                [0.; 4]
            };
            if let Some(view) = view(target) {
                // SAFETY: the view is live.
                unsafe { context.ClearRenderTargetView(view, &color) };
            }
        };
        let draw = |kind: usize,
                    range: &Range<u32>,
                    texture: Option<ID3D11ShaderResourceView>|
         -> Result<()> {
            let pipeline = &gpu.pipelines[kind];
            write(context, batch, bytes(&[range.start, 0, 0, 0]))?;
            // SAFETY: the instance views hold this frame's data.
            unsafe {
                context.VSSetShaderResources(1, Some(slice::from_ref(&views[kind])));
                context.PSSetShaderResources(1, Some(slice::from_ref(&views[kind])));
                if texture.is_some() {
                    let texture = [texture];
                    context.VSSetShaderResources(0, Some(&texture));
                    context.PSSetShaderResources(0, Some(&texture));
                }
                context.VSSetShader(&pipeline.vertex, None);
                context.PSSetShader(&pipeline.fragment, None);
                context.OMSetBlendState(&pipeline.blend, None, 0xFFFF_FFFF);
                context.DrawInstanced(4, range.len() as u32, 0, 0);
            }
            Ok(())
        };
        let atlas = |id: AtlasTextureId| gpu.atlas.backend.borrow().view(id);
        // SAFETY: state shared by every draw of the frame.
        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            context.RSSetState(&gpu.rasterizer);
            context.VSSetConstantBuffers(0, Some(&[gpu.globals.clone(), gpu.batch.clone()]));
            context.PSSetConstantBuffers(0, Some(slice::from_ref(&gpu.globals)));
            context.PSSetSamplers(0, Some(slice::from_ref(&gpu.sampler)));
        }
        let mut current = Target::Frame;
        for cmd in &frame.cmds {
            match cmd {
                // Tiles of one texture keep it bound, with their own globals.
                Cmd::Pass(target, false) if frame.texture(*target) == frame.texture(current) => {
                    current = *target;
                    write(
                        context,
                        globals,
                        bytes(&[frame.globals(current, &gpu.text)]),
                    )?;
                }
                Cmd::Pass(target, clear_target) => {
                    current = *target;
                    bind(current)?;
                    if *clear_target {
                        clear(current);
                    }
                }
                Cmd::Quads(range) => draw(QUADS, range, None)?,
                Cmd::Shadows(range) => draw(SHADOWS, range, None)?,
                Cmd::Underlines(range) => draw(UNDERLINES, range, None)?,
                Cmd::Mono(id, range) => {
                    if let Some(texture) = atlas(*id) {
                        draw(MONO, range, Some(texture))?;
                    }
                }
                Cmd::Poly(id, range) => {
                    if let Some(texture) = atlas(*id) {
                        draw(POLY, range, Some(texture))?;
                    }
                }
                Cmd::Blur { src, dst, index } => {
                    bind(*dst)?;
                    clear(*dst);
                    draw(BLURS, &(*index..*index + 1), srv(*src))?;
                    bind(current)?;
                }
                Cmd::Copy { src, dst, region } => {
                    let (from, to) = (frame.origin(*src), frame.origin(*dst));
                    let (left, top) = ((region[0] - from[0]) as u32, (region[1] - from[1]) as u32);
                    // SAFETY: the region lies within both textures.
                    unsafe {
                        context.CopySubresourceRegion(
                            texture(*dst),
                            0,
                            (region[0] - to[0]) as u32,
                            (region[1] - to[1]) as u32,
                            0,
                            texture(*src),
                            0,
                            Some(&D3D11_BOX {
                                left,
                                top,
                                front: 0,
                                right: left + region[2] as u32,
                                bottom: top + region[3] as u32,
                                back: 1,
                            }),
                        );
                    }
                }
                Cmd::Composite {
                    src,
                    mask,
                    backdrop,
                    index,
                } => {
                    let mask = mask.and_then(atlas).or_else(|| srv(*src));
                    let backdrop = backdrop.map_or_else(|| srv(*src), srv);
                    // SAFETY: the textures are live.
                    unsafe { context.PSSetShaderResources(2, Some(&[mask, backdrop])) };
                    draw(COMPOSITES, &(*index..*index + 1), srv(*src))?;
                }
            }
        }
        // SAFETY: unbinds the layers, which later frames draw into.
        unsafe { context.PSSetShaderResources(0, Some(&[None, None, None, None])) };
        Ok(())
    }
}

/// The RGBA rows of a drawn frame.
fn read_back(gpu: &Gpu, texture: &ID3D11Texture2D, size: [u32; 2]) -> Result<Vec<u8>> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: size[0],
        Height: size[1],
        MipLevels: 1,
        ArraySize: 1,
        Format: FORMAT,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        ..Default::default()
    };
    let row = size[0] as usize * 4;
    let mut pixels = Vec::with_capacity(row * size[1] as usize);
    // SAFETY: the mapped rows hold `RowPitch` bytes each, of which `row` are read.
    unsafe {
        let mut staging = None;
        gpu.device
            .CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.context("missing staging texture")?;
        gpu.context.CopyResource(&staging, texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        for y in 0..size[1] as usize {
            pixels.extend_from_slice(slice::from_raw_parts(
                mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                row,
            ));
        }
        gpu.context.Unmap(&staging, 0);
    }
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(pixels)
}

pub(crate) struct AtlasTextures {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    max: i32,
    mono: TextureList<AtlasTexture>,
    poly: TextureList<AtlasTexture>,
}

struct AtlasTexture {
    id: AtlasTextureId,
    packer: Packer,
    texture: ID3D11Texture2D,
    view: Option<ID3D11ShaderResourceView>,
}

impl AtlasTextures {
    fn list(&mut self, kind: AtlasTextureKind) -> &mut TextureList<AtlasTexture> {
        match kind {
            AtlasTextureKind::Monochrome => &mut self.mono,
            AtlasTextureKind::Polychrome => &mut self.poly,
        }
    }

    fn view(&self, id: AtlasTextureId) -> Option<ID3D11ShaderResourceView> {
        let list = match id.kind {
            AtlasTextureKind::Monochrome => &self.mono,
            AtlasTextureKind::Polychrome => &self.poly,
        };
        list.get(id.index as usize)?.view.clone()
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
        let mut hit = None;
        for (index, texture) in self.list(kind).textures.iter_mut().enumerate().rev() {
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
                let side = |value: i32| value.clamp(MIN_TEXTURE_SIDE, self.max);
                let texture_size = Size::new(
                    DevicePixels(side(size.width.0)),
                    DevicePixels(side(size.height.0)),
                );
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: texture_size.width.0 as u32,
                    Height: texture_size.height.0 as u32,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: match kind {
                        AtlasTextureKind::Monochrome => DXGI_FORMAT_R8_UNORM,
                        AtlasTextureKind::Polychrome => DXGI_FORMAT_B8G8R8A8_UNORM,
                    },
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    ..Default::default()
                };
                let (mut texture, mut view) = (None, None);
                // SAFETY: plain resource creation.
                unsafe {
                    self.device
                        .CreateTexture2D(&desc, None, Some(&mut texture))?;
                    let texture = texture.as_ref().context("missing atlas texture")?;
                    self.device
                        .CreateShaderResourceView(texture, None, Some(&mut view))?;
                }
                let texture = texture.context("missing atlas texture")?;
                let list = self.list(kind);
                let index = list.push(|index| AtlasTexture {
                    id: AtlasTextureId {
                        index: index as u32,
                        kind,
                    },
                    packer: Packer::new(texture_size),
                    texture,
                    view,
                });
                let tile = list.textures[index]
                    .as_mut()
                    .and_then(|texture| texture.packer.allocate(size))
                    .context("allocating atlas tile")?;
                (index, tile)
            }
        };
        let context = self.context.clone();
        let texture = self.list(kind).textures[index]
            .as_ref()
            .context("atlas texture")?;
        // SAFETY: the bytes cover the tile, which lies within the texture.
        unsafe {
            context.UpdateSubresource(
                &texture.texture,
                0,
                Some(&D3D11_BOX {
                    left: bounds.origin.x.0 as u32,
                    top: bounds.origin.y.0 as u32,
                    front: 0,
                    right: (bounds.origin.x.0 + size.width.0) as u32,
                    bottom: (bounds.origin.y.0 + size.height.0) as u32,
                    back: 1,
                }),
                bytes.as_ptr().cast(),
                row as u32,
                0,
            );
        }
        Ok(AtlasTile {
            texture_id: texture.id,
            tile_id,
            padding: 0,
            bounds,
        })
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
