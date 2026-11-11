use crate::{
    Color, Size,
    atlas::{self, Packer, PageKind, PageList, PageStore, Tile},
    frame::{Command, RenderFrame, Target},
    gpu::{ATTRIBUTES, Blur, Composite, Globals, Instance, Plain, bytes},
    scene::Scene,
};
use anyhow::{Context as _, Result, bail, ensure};
use std::{borrow::Cow, cell::RefCell, mem, rc::Rc, slice, time::Duration};
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            DirectComposition::*,
            Dxgi::{Common::*, *},
        },
    },
    core::{Interface, PCSTR},
};

const FORMAT: DXGI_FORMAT = DXGI_FORMAT_B8G8R8A8_UNORM;
const SWAP_BUFFERS: u32 = 2;
const MAX_INSTANCE_BYTES: usize = 256 << 20;
const SPARE_FRAMES: u8 = 3;
// DirectWrite gives linear coverage, and the targets blend in sRGB, which makes dark text heavy and light
// text thin. This moves coverage back toward the ink.
const TEXT_CONTRAST: f32 = 0.3;

pub(crate) type Atlas = atlas::Atlas<Pages>;

type Stage = (ID3D11VertexShader, ID3D11PixelShader);

struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    factory: IDXGIFactory2,
    composition: IDCompositionDevice,
    max_texture: u32,
    layout: ID3D11InputLayout,
    draw: Stage,
    composite: Stage,
    blur: Stage,
    over: ID3D11BlendState,
    replace: ID3D11BlendState,
    sampler: ID3D11SamplerState,
    rasterizer: ID3D11RasterizerState,
    // Globals, the composite and the blur, in registers b0 to b2.
    uniforms: [Option<ID3D11Buffer>; 3],
    instances: RefCell<Option<(ID3D11Buffer, usize)>>,
    atlas: Rc<Atlas>,
}

thread_local! {
    static GPU: RefCell<Option<Rc<Gpu>>> = const { RefCell::new(None) };
}

// Made again after the device was lost. The atlas stays the same object, emptied.
fn gpu() -> Result<Rc<Gpu>> {
    GPU.with_borrow_mut(|slot| {
        if let Some(gpu) = slot.as_ref()
            // SAFETY: a plain device query.
            && unsafe { gpu.device.GetDeviceRemovedReason() }.is_ok()
        {
            return Ok(gpu.clone());
        }
        // A failed attempt keeps the lost device here, and with it the atlas, for the next try.
        let atlas = slot.as_ref().map(|lost| lost.atlas.clone());
        let gpu = Rc::new(Gpu::new(atlas)?);
        *slot = Some(gpu.clone());
        Ok(gpu)
    })
}

#[cfg(renderer_shaders)]
fn bytecode(entry: &str) -> Result<Cow<'static, [u8]>> {
    macro_rules! built {
        ($($name:literal)*) => {
            match entry {
                $($name => include_bytes!(concat!(env!("OUT_DIR"), "/", $name, ".cso")).as_slice(),)*
                _ => bail!("no shader {entry}"),
            }
        };
    }
    Ok(Cow::Borrowed(built!(
        "draw_vertex" "draw_fragment" "composite_vertex" "composite_fragment" "blur_vertex" "blur_fragment"
    )))
}

// Debug builds compile the shaders when they start. build.rs compiles them ahead for release builds.
#[cfg(not(renderer_shaders))]
fn bytecode(entry: &str) -> Result<Cow<'static, [u8]>> {
    use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
    let source = include_str!("shaders.hlsl");
    let name = format!("{entry}\0");
    let target: &[u8] = match entry.ends_with("_vertex") {
        true => b"vs_4_0\0",
        false => b"ps_4_0\0",
    };
    let (mut code, mut errors) = (None, None);
    // SAFETY: the source and the terminated names outlive the call, which writes the two blobs.
    let compiled = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR::null(),
            None,
            None::<&ID3DInclude>,
            PCSTR(name.as_ptr()),
            PCSTR(target.as_ptr()),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    // SAFETY: a blob holds as many bytes as it reports.
    let read = |blob: &ID3DBlob| unsafe {
        slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    if let Err(error) = compiled {
        let log = errors.map(|log| String::from_utf8_lossy(read(&log)).into_owned());
        bail!(
            "shader {entry}: {}",
            log.unwrap_or_else(|| error.to_string())
        );
    }
    Ok(Cow::Owned(read(&code.context("no shader code")?).to_vec()))
}

fn made<T>(value: Option<T>, what: &str) -> Result<T> {
    value.with_context(|| format!("no {what}"))
}

impl Gpu {
    fn new(atlas: Option<Rc<Atlas>>) -> Result<Self> {
        let open = |driver| -> Result<_> {
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
                        D3D_FEATURE_LEVEL_10_0,
                    ]),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    Some(&mut level),
                    Some(&mut context),
                )?;
            }
            Ok((
                made(device, "Direct3D device")?,
                made(context, "device context")?,
                level,
            ))
        };
        let (device, context, level) =
            open(D3D_DRIVER_TYPE_HARDWARE).or_else(|_| open(D3D_DRIVER_TYPE_WARP))?;
        // SAFETY: COM calls on the new device. Descriptions are locals that outlive the calls.
        unsafe {
            let dxgi: IDXGIDevice = device.cast()?;
            let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let stage = |name: &str| -> Result<(Stage, Cow<'static, [u8]>)> {
                let vertex = bytecode(&format!("{name}_vertex"))?;
                let fragment = bytecode(&format!("{name}_fragment"))?;
                let (mut vs, mut ps) = (None, None);
                device.CreateVertexShader(&vertex, None, Some(&mut vs))?;
                device.CreatePixelShader(&fragment, None, Some(&mut ps))?;
                Ok(((made(vs, name)?, made(ps, name)?), vertex))
            };
            let (draw, draw_code) = stage("draw")?;
            let attributes: Vec<_> = (0..ATTRIBUTES as u32)
                .map(|index| D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: PCSTR(c"I".as_ptr().cast()),
                    SemanticIndex: index,
                    Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: index * 16,
                    InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
                    InstanceDataStepRate: 1,
                })
                .collect();
            let mut layout = None;
            device.CreateInputLayout(&attributes, &draw_code, Some(&mut layout))?;
            // Shaders return premultiplied colors. Blurs replace what the target holds.
            let blend = |on: bool| -> Result<ID3D11BlendState> {
                let target = D3D11_RENDER_TARGET_BLEND_DESC {
                    BlendEnable: on.into(),
                    SrcBlend: D3D11_BLEND_ONE,
                    DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                    BlendOp: D3D11_BLEND_OP_ADD,
                    SrcBlendAlpha: D3D11_BLEND_ONE,
                    DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                    BlendOpAlpha: D3D11_BLEND_OP_ADD,
                    RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
                };
                let mut made_state = None;
                let all = D3D11_BLEND_DESC {
                    RenderTarget: [target; 8],
                    ..Default::default()
                };
                device.CreateBlendState(&all, Some(&mut made_state))?;
                made(made_state, "blend state")
            };
            // Pages and layers have one level, so the sampler reads level 0 only.
            let edges = D3D11_TEXTURE_ADDRESS_CLAMP;
            let mut sampler = None;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_LINEAR_MIP_POINT,
                    AddressU: edges,
                    AddressV: edges,
                    AddressW: edges,
                    MaxAnisotropy: 1,
                    ComparisonFunc: D3D11_COMPARISON_NEVER,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            // Composite transforms can mirror quads, so no side is culled.
            let mut rasterizer = None;
            device.CreateRasterizerState(
                &D3D11_RASTERIZER_DESC {
                    FillMode: D3D11_FILL_SOLID,
                    CullMode: D3D11_CULL_NONE,
                    ..Default::default()
                },
                Some(&mut rasterizer),
            )?;
            let uniform =
                |bytes: usize| dynamic(&device, bytes, D3D11_BIND_CONSTANT_BUFFER).map(Some);
            let max_texture = if level.0 >= D3D_FEATURE_LEVEL_11_0.0 {
                16384
            } else {
                8192
            };
            let pages = Pages {
                device: device.clone(),
                context: context.clone(),
                max: max_texture as i32,
                mask: PageList::default(),
                color: PageList::default(),
            };
            let atlas = match atlas {
                Some(atlas) => {
                    atlas.clear(|old| *old = pages);
                    atlas
                }
                None => Rc::new(Atlas::new(pages)),
            };
            Ok(Self {
                layout: made(layout, "input layout")?,
                draw,
                composite: stage("composite")?.0,
                blur: stage("blur")?.0,
                over: blend(true)?,
                replace: blend(false)?,
                sampler: made(sampler, "sampler")?,
                rasterizer: made(rasterizer, "rasterizer state")?,
                uniforms: [
                    uniform(size_of::<Globals>())?,
                    uniform(size_of::<Composite>())?,
                    uniform(size_of::<Blur>())?,
                ],
                instances: RefCell::new(None),
                atlas,
                device,
                context,
                factory,
                composition,
                max_texture,
            })
        }
    }

    fn write<T: Plain>(&self, buffer: &Option<ID3D11Buffer>, value: &T) -> Result<()> {
        let buffer = buffer.as_ref().context("no uniform buffer")?;
        fill(&self.context, buffer, bytes(slice::from_ref(value)))
    }

    fn instances(&self, data: &[u8]) -> Result<Option<ID3D11Buffer>> {
        if data.is_empty() {
            return Ok(None);
        }
        ensure!(
            data.len() <= MAX_INSTANCE_BYTES,
            "a frame has more than {MAX_INSTANCE_BYTES} bytes of instances"
        );
        let mut slot = self.instances.borrow_mut();
        if slot.as_ref().is_none_or(|(_, room)| *room < data.len()) {
            let room = data.len().next_power_of_two().max(64 << 10);
            let buffer = dynamic(&self.device, room, D3D11_BIND_VERTEX_BUFFER)?;
            *slot = Some((buffer, room));
        }
        let (buffer, _) = slot.as_ref().context("no instance buffer")?;
        fill(&self.context, buffer, data)?;
        Ok(Some(buffer.clone()))
    }
}

fn dynamic(device: &ID3D11Device, bytes: usize, bind: D3D11_BIND_FLAG) -> Result<ID3D11Buffer> {
    let mut buffer = None;
    // SAFETY: a plain resource creation from a local description.
    unsafe {
        device.CreateBuffer(
            &D3D11_BUFFER_DESC {
                ByteWidth: bytes as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: bind.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            },
            None,
            Some(&mut buffer),
        )?;
    }
    made(buffer, "buffer")
}

// Callers unmap it.
fn map(
    context: &ID3D11DeviceContext,
    resource: impl windows::core::Param<ID3D11Resource>,
    how: D3D11_MAP,
) -> Result<D3D11_MAPPED_SUBRESOURCE> {
    let mut mapped = Default::default();
    // SAFETY: the out struct is a local.
    unsafe { context.Map(resource, 0, how, 0, Some(&mut mapped))? };
    Ok(mapped)
}

// The buffer must hold at least `data`.
fn fill(context: &ID3D11DeviceContext, buffer: &ID3D11Buffer, data: &[u8]) -> Result<()> {
    let open = map(context, buffer, D3D11_MAP_WRITE_DISCARD)?;
    // SAFETY: the mapping covers the whole buffer, which callers size for the data.
    unsafe {
        data.as_ptr()
            .copy_to_nonoverlapping(open.pData.cast(), data.len());
        context.Unmap(buffer, 0);
    }
    Ok(())
}

const ONE_SAMPLE: DXGI_SAMPLE_DESC = DXGI_SAMPLE_DESC {
    Count: 1,
    Quality: 0,
};

// The CPU reads it when `bind` is empty.
fn texture(
    device: &ID3D11Device,
    [width, height]: [u32; 2],
    format: DXGI_FORMAT,
    bind: D3D11_BIND_FLAG,
) -> Result<ID3D11Texture2D> {
    let readable = bind.0 == 0;
    let spec = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: ONE_SAMPLE,
        Usage: if readable {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: bind.0 as u32,
        CPUAccessFlags: if readable {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    };
    let mut made_texture = None;
    // SAFETY: a plain resource creation from a local description.
    unsafe { device.CreateTexture2D(&spec, None, Some(&mut made_texture))? };
    made(made_texture, "texture")
}

struct Image {
    pixels: ID3D11Texture2D,
    draw_to: Option<ID3D11RenderTargetView>,
    read: Option<ID3D11ShaderResourceView>,
}

impl Image {
    fn new(device: &ID3D11Device, pixels: ID3D11Texture2D, bind: D3D11_BIND_FLAG) -> Result<Self> {
        let (mut draw_to, mut read) = (None, None);
        // SAFETY: views of a live texture that has these bindings.
        unsafe {
            if bind.contains(D3D11_BIND_RENDER_TARGET) {
                device.CreateRenderTargetView(&pixels, None, Some(&mut draw_to))?;
            }
            if bind.contains(D3D11_BIND_SHADER_RESOURCE) {
                device.CreateShaderResourceView(&pixels, None, Some(&mut read))?;
            }
        }
        Ok(Self {
            pixels,
            draw_to,
            read,
        })
    }
}

struct Layer {
    image: Image,
    size: [u32; 2],
    idle: u8,
}

// A window has one composition target.
struct Surface {
    chain: IDXGISwapChain1,
    back: Option<Image>,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
}

fn back_buffer(gpu: &Gpu, chain: &IDXGISwapChain1) -> Result<Image> {
    // SAFETY: a plain query of a live swap chain.
    let pixels = unsafe { chain.GetBuffer(0)? };
    Image::new(&gpu.device, pixels, D3D11_BIND_RENDER_TARGET)
}

impl Surface {
    fn new(gpu: &Gpu, hwnd: HWND, size: [u32; 2], opaque: bool) -> Result<Self> {
        let chain_spec = DXGI_SWAP_CHAIN_DESC1 {
            Width: size[0],
            Height: size[1],
            Format: FORMAT,
            SampleDesc: ONE_SAMPLE,
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: SWAP_BUFFERS,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: match opaque {
                true => DXGI_ALPHA_MODE_IGNORE,
                false => DXGI_ALPHA_MODE_PREMULTIPLIED,
            },
            ..Default::default()
        };
        // SAFETY: COM calls with a live device, and a window that outlives its surface.
        unsafe {
            let chain =
                gpu.factory
                    .CreateSwapChainForComposition(&gpu.device, &chain_spec, None)?;
            let target = gpu.composition.CreateTargetForHwnd(hwnd, false)?;
            let visual = gpu.composition.CreateVisual()?;
            visual.SetContent(&chain)?;
            target.SetRoot(&visual)?;
            gpu.composition.Commit()?;
            Ok(Self {
                back: Some(back_buffer(gpu, &chain)?),
                chain,
                _target: target,
                _visual: visual,
            })
        }
    }
}

pub(crate) struct Renderer {
    gpu: Rc<Gpu>,
    hwnd: HWND,
    opaque: bool,
    background: [f32; 4],
    size: [u32; 2],
    surface: Option<Surface>,
    pool: Vec<Layer>,
    slots: Vec<usize>,
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
            background: [0., 0., 0., f32::from(u8::from(!transparent))],
            size,
            surface: Some(surface),
            pool: Vec::new(),
            slots: Vec::new(),
            frame: RenderFrame::default(),
        })
    }

    pub fn atlas(&self) -> &Rc<Atlas> {
        &self.gpu.atlas
    }

    pub fn set_background(&mut self, color: Color) {
        self.background = match self.opaque {
            true => [color.r, color.g, color.b, 1.],
            false => [
                color.r * color.a,
                color.g * color.a,
                color.b * color.a,
                color.a,
            ],
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
        // The swap chain resizes only once nothing holds its buffers.
        surface.back = None;
        // SAFETY: the back buffer and its view are released and unbound.
        unsafe {
            self.gpu.context.OMSetRenderTargets(None, None);
            surface.chain.ResizeBuffers(
                SWAP_BUFFERS,
                size[0],
                size[1],
                FORMAT,
                DXGI_SWAP_CHAIN_FLAG::default(),
            )?;
        }
        surface.back = Some(back_buffer(&self.gpu, &surface.chain)?);
        Ok(())
    }

    // Layers go; the swap chain keeps the last frame on screen.
    pub fn release(&mut self) {
        self.pool.clear();
        self.slots.clear();
    }

    pub fn draw(&mut self, scene: &Scene, capture: bool) -> Option<Result<([u32; 2], Vec<u8>)>> {
        match self.render(scene, capture) {
            Ok(pixels) => pixels.map(|pixels| Ok((self.size, pixels))),
            Err(error) => {
                eprintln!("apps: failed to render: {error:#}");
                // SAFETY: a plain device query.
                if unsafe { self.gpu.device.GetDeviceRemovedReason() }.is_err() {
                    // The next frame makes a new device. Every window must draw again on it.
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
            // The old composition target must go before the window takes a new one.
            self.surface = None;
            self.pool.clear();
            self.gpu = gpu;
            self.surface = Some(Surface::new(&self.gpu, self.hwnd, self.size, self.opaque)?);
        }
        let mut frame = mem::take(&mut self.frame);
        frame.record(scene, self.size, self.gpu.max_texture, true);
        let encoded = self.encode(&frame);
        self.frame = frame;
        encoded?;
        let surface = self.surface.as_ref().context("no surface")?;
        let back = surface.back.as_ref().context("no back buffer")?;
        let pixels = capture
            .then(|| read_back(&self.gpu, &back.pixels, self.size))
            .transpose()?;
        // The pacing thread times frames, so presents don't wait for a vertical blank.
        // SAFETY: presents the back buffer that was just drawn.
        unsafe { surface.chain.Present(0, DXGI_PRESENT(0)) }.ok()?;
        Ok(pixels)
    }

    fn layers(&mut self, sizes: &[[u32; 2]]) -> Result<()> {
        for layer in &mut self.pool {
            layer.idle = layer.idle.saturating_add(1);
        }
        self.pool.retain(|layer| layer.idle <= SPARE_FRAMES);
        self.slots.clear();
        for &size in sizes {
            let reuse = self
                .pool
                .iter()
                .position(|layer| layer.idle > 0 && layer.size == size);
            let index = match reuse {
                Some(index) => index,
                None => {
                    let bind = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
                    let device = &self.gpu.device;
                    let image = Image::new(device, texture(device, size, FORMAT, bind)?, bind)?;
                    self.pool.push(Layer {
                        image,
                        size,
                        idle: 0,
                    });
                    self.pool.len() - 1
                }
            };
            self.pool[index].idle = 0;
            self.slots.push(index);
        }
        Ok(())
    }

    fn encode(&mut self, frame: &RenderFrame) -> Result<()> {
        self.layers(&frame.textures)?;
        let gpu = &*self.gpu;
        let context = &gpu.context;
        let instances = gpu.instances(bytes(&frame.instances))?;
        let back = self
            .surface
            .as_ref()
            .and_then(|surface| surface.back.as_ref())
            .context("no back buffer")?;
        let image = |target: Target| match target {
            Target::Frame => back,
            Target::Layer(index) => &self.pool[self.slots[frame.layers[index].texture]].image,
        };
        let source = |target: Target| image(target).read.clone();
        let pages = gpu.atlas.pages.borrow();
        let page = |kind, index: Option<u32>| index.and_then(|index| pages.view(kind, index));
        let none: [Option<ID3D11ShaderResourceView>; 3] = Default::default();
        let bind = |target: Target, clear: bool| -> Result<()> {
            let view = &image(target).draw_to;
            let [width, height] = frame.pixels(target);
            // SAFETY: state changes on live resources. Sources unbind first, since the target can be one.
            unsafe {
                context.PSSetShaderResources(0, Some(&none));
                context.OMSetRenderTargets(Some(slice::from_ref(view)), None);
                context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                    Width: width,
                    Height: height,
                    MaxDepth: 1.,
                    ..Default::default()
                }]));
                if clear && let Some(view) = view {
                    let color = match target {
                        Target::Frame => self.background,
                        Target::Layer(_) => [0.; 4],
                    };
                    context.ClearRenderTargetView(view, &color);
                }
            }
            gpu.write(&gpu.uniforms[0], &frame.globals(target, TEXT_CONTRAST))
        };
        let quad = |stage: &Stage,
                    blend: &ID3D11BlendState,
                    sources: &[Option<ID3D11ShaderResourceView>]| {
            // SAFETY: state changes and a draw with live shaders and views.
            unsafe {
                context.VSSetShader(&stage.0, None);
                context.PSSetShader(&stage.1, None);
                context.OMSetBlendState(blend, None, u32::MAX);
                context.PSSetShaderResources(0, Some(sources));
                context.Draw(4, 0);
            }
        };
        let stride = size_of::<Instance>() as u32;
        // SAFETY: state that every draw of the frame shares.
        unsafe {
            context.IASetInputLayout(&gpu.layout);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            context.IASetVertexBuffers(0, 1, Some(&instances), Some(&stride), Some(&0u32));
            context.RSSetState(&gpu.rasterizer);
            context.VSSetConstantBuffers(0, Some(&gpu.uniforms));
            context.PSSetConstantBuffers(0, Some(&gpu.uniforms));
            context.PSSetSamplers(0, Some(&[Some(gpu.sampler.clone())]));
        }
        let (mut current, mut bound) = (Target::Frame, false);
        for command in &frame.commands {
            match *command {
                Command::Pass(target, false)
                    if bound && frame.texture(target) == frame.texture(current) =>
                {
                    current = target;
                    gpu.write(&gpu.uniforms[0], &frame.globals(target, TEXT_CONTRAST))?;
                }
                // Targets clear at once, since a later pass can draw into them.
                Command::Pass(target, clear) => {
                    current = target;
                    bind(target, clear)?;
                    bound = true;
                }
                Command::Draw {
                    ref instances,
                    mask_page,
                    color_page,
                } => {
                    if !bound {
                        bind(current, false)?;
                        bound = true;
                    }
                    // SAFETY: the instance buffer holds this frame, and the pages are live.
                    unsafe {
                        context.VSSetShader(&gpu.draw.0, None);
                        context.PSSetShader(&gpu.draw.1, None);
                        context.OMSetBlendState(&gpu.over, None, u32::MAX);
                        context.PSSetShaderResources(
                            0,
                            Some(&[
                                page(PageKind::Mask, mask_page),
                                page(PageKind::Color, color_page),
                            ]),
                        );
                        context.DrawInstanced(4, instances.len() as u32, 0, instances.start);
                    }
                }
                Command::Blur { src, dst, index } => {
                    bind(dst, true)?;
                    gpu.write(&gpu.uniforms[2], &frame.blurs[index as usize])?;
                    quad(&gpu.blur, &gpu.replace, &[source(src)]);
                    bound = false;
                }
                Command::Copy { src, dst, region } => {
                    let (from, to) = (frame.origin(src), frame.origin(dst));
                    let [left, top] = [region[0] - from[0], region[1] - from[1]].map(|v| v as u32);
                    // SAFETY: the region lies within both textures, which have the same format.
                    unsafe {
                        context.CopySubresourceRegion(
                            &image(dst).pixels,
                            0,
                            (region[0] - to[0]) as u32,
                            (region[1] - to[1]) as u32,
                            0,
                            &image(src).pixels,
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
                Command::Composite {
                    src,
                    mask,
                    backdrop,
                    index,
                } => {
                    if !bound {
                        bind(current, false)?;
                        bound = true;
                    }
                    gpu.write(&gpu.uniforms[1], &frame.composites[index as usize])?;
                    let layer = source(src);
                    let mask = mask
                        .and_then(|mask| page(PageKind::Color, Some(mask)))
                        .or_else(|| layer.clone());
                    let below = backdrop.map_or_else(|| layer.clone(), source);
                    quad(&gpu.composite, &gpu.over, &[layer, mask, below]);
                }
            }
        }
        // SAFETY: unbinds the layers, which the next frame draws into.
        unsafe { context.PSSetShaderResources(0, Some(&none)) };
        Ok(())
    }
}

fn read_back(gpu: &Gpu, source: &ID3D11Texture2D, size: [u32; 2]) -> Result<Vec<u8>> {
    let copy = texture(&gpu.device, size, FORMAT, D3D11_BIND_FLAG(0))?;
    // SAFETY: the copy has the size and format of the source.
    unsafe { gpu.context.CopyResource(&copy, source) };
    let open = map(&gpu.context, &copy, D3D11_MAP_READ)?;
    let row = size[0] as usize * 4;
    let mut pixels = vec![0u8; row * size[1] as usize];
    for (y, line) in pixels.chunks_exact_mut(row).enumerate() {
        // SAFETY: each mapped row holds `RowPitch` bytes, of which the first `row` are pixels.
        unsafe {
            let start = open.pData.cast::<u8>().add(y * open.RowPitch as usize);
            line.copy_from_slice(slice::from_raw_parts(start, row));
        }
    }
    // SAFETY: ends the mapping made above.
    unsafe { gpu.context.Unmap(&copy, 0) };
    for bgra in pixels.as_chunks_mut::<4>().0 {
        bgra.swap(0, 2);
    }
    Ok(pixels)
}

struct Page {
    packer: Packer,
    image: Image,
}

// Atlas pages: mask pages keep coverage in red, color pages are BGRA like the frame.
pub(crate) struct Pages {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    max: i32,
    mask: PageList<Page>,
    color: PageList<Page>,
}

impl Pages {
    fn list(&mut self, kind: PageKind) -> &mut PageList<Page> {
        match kind {
            PageKind::Mask => &mut self.mask,
            PageKind::Color => &mut self.color,
        }
    }

    fn view(&self, kind: PageKind, index: u32) -> Option<ID3D11ShaderResourceView> {
        let list = match kind {
            PageKind::Mask => &self.mask,
            PageKind::Color => &self.color,
        };
        list.get(index as usize)?.image.read.clone()
    }
}

impl PageStore for Pages {
    fn insert(&mut self, kind: PageKind, size: Size<i32>, bytes: &[u8]) -> Result<Tile> {
        ensure!(
            size.width > 0 && size.height > 0,
            "atlas tiles must have a size"
        );
        let (format, pixel) = match kind {
            PageKind::Mask => (DXGI_FORMAT_R8_UNORM, 1),
            PageKind::Color => (FORMAT, 4),
        };
        let row = size.width as usize * pixel;
        ensure!(
            bytes.len() >= row * size.height as usize,
            "atlas upload is too short"
        );
        let device = self.device.clone();
        let (index, rect, id) = self.list(kind).allocate(
            size,
            |page| &mut page.packer,
            |size| {
                let side = [size.width as u32, size.height as u32];
                let bind = D3D11_BIND_SHADER_RESOURCE;
                Ok(Page {
                    packer: Packer::new(size),
                    image: Image::new(&device, texture(&device, side, format, bind)?, bind)?,
                })
            },
        )?;
        let context = self.context.clone();
        let page = self.list(kind).get(index).context("no atlas page")?;
        let (x, y) = (rect.origin.x as u32, rect.origin.y as u32);
        // SAFETY: the bytes cover the tile, which lies within the page.
        unsafe {
            context.UpdateSubresource(
                &page.image.pixels,
                0,
                Some(&D3D11_BOX {
                    left: x,
                    top: y,
                    front: 0,
                    right: x + size.width as u32,
                    bottom: y + size.height as u32,
                    back: 1,
                }),
                bytes.as_ptr().cast(),
                row as u32,
                0,
            );
        }
        Ok(Tile {
            page: index as u32,
            kind,
            id,
            rect,
        })
    }

    fn remove(&mut self, tile: Tile) {
        let list = self.list(tile.kind);
        let index = tile.page as usize;
        if list
            .pages
            .get_mut(index)
            .and_then(Option::as_mut)
            .is_some_and(|page| page.packer.free(tile.id))
        {
            list.emptied(index, |page| page.packer.is_empty());
        }
    }

    fn max_side(&self) -> i32 {
        self.max
    }
}
