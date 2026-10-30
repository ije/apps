//! WebGPU for pages. The runtime calls it through the C ABI below.

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    ffi::c_void,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU8, AtomicUsize, Ordering},
        mpsc::Sender,
    },
};
use wgpu::*;

/// Version of the C ABI. The runtime rejects a library with another version.
pub const ABI: u32 = 1;

/// Returns a runtime buffer of `length` bytes that the library fills.
pub type Alloc = extern "C" fn(output: *mut c_void, length: usize) -> *mut u8;

/// Runtime callbacks. Only `wake` and `drop_waker` run on other threads.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Host {
    /// Returns the bytes of the `$appsBytes` argument `index` of the current call, or null.
    pub read: extern "C" fn(index: usize, length: *mut usize) -> *const u8,
    /// Adds bytes to the current call result and returns their `$appsBytes` index, or `usize::MAX`.
    pub write: extern "C" fn(bytes: *const u8, length: usize) -> usize,
    pub waker: extern "C" fn() -> *mut c_void,
    pub wake: extern "C" fn(waker: *mut c_void),
    pub drop_waker: extern "C" fn(waker: *mut c_void),
}

static HOST: OnceLock<Host> = OnceLock::new();

fn host() -> &'static Host {
    HOST.get().expect("apps_webgpu_new sets the host")
}

struct Waker(*mut c_void);

// WARNING: runtime wakers are `Send + Sync`, so wgpu threads can wake the loop with them.
unsafe impl Send for Waker {}
unsafe impl Sync for Waker {}

impl Waker {
    fn current() -> Self {
        Self((host().waker)())
    }

    fn wake(&self) {
        (host().wake)(self.0)
    }
}

impl Drop for Waker {
    fn drop(&mut self) {
        (host().drop_waker)(self.0)
    }
}

fn base64(bytes: &[u8]) -> Value {
    match (host().write)(bytes.as_ptr(), bytes.len()) {
        usize::MAX => Value::String(STANDARD.encode(bytes)),
        index => json!({"$appsBytes": index}),
    }
}

fn output(output: *mut c_void, alloc: Alloc, bytes: &[u8]) {
    let target = alloc(output, bytes.len());
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len()) };
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_webgpu_abi() -> u32 {
    ABI
}

/// # Safety
/// `host` points to a `Host`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webgpu_new(host: *const c_void) -> *mut c_void {
    HOST.get_or_init(|| unsafe { *host.cast::<Host>() });
    Box::into_raw(Box::<Gpu>::default()).cast()
}

/// # Safety
/// `gpu` comes from `apps_webgpu_new` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webgpu_free(gpu: *mut c_void) {
    drop(unsafe { Box::from_raw(gpu.cast::<Gpu>()) });
}

/// Writes the JSON result, or the error message when it returns false.
///
/// # Safety
/// `gpu` comes from `apps_webgpu_new`, and `op` and `args` point to UTF-8 text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webgpu_call(
    gpu: *mut c_void,
    op: *const u8,
    op_length: usize,
    args: *const u8,
    args_length: usize,
    out: *mut c_void,
    alloc: Alloc,
) -> bool {
    let gpu = unsafe { &mut *gpu.cast::<Gpu>() };
    let result = (|| -> Result<Vec<u8>> {
        let op = std::str::from_utf8(unsafe { std::slice::from_raw_parts(op, op_length) })?;
        let args =
            serde_json::from_slice(unsafe { std::slice::from_raw_parts(args, args_length) })?;
        Ok(serde_json::to_vec(&gpu.call(op, &args)?)?)
    })();
    match result {
        Ok(json) => {
            output(out, alloc, &json);
            true
        }
        Err(error) => {
            output(out, alloc, error.to_string().as_bytes());
            false
        }
    }
}

/// Returns 0 when the canvas has no image, 1 when its last image is current, 2 after writing a new
/// BGRA frame of `size`, and -1 after writing an error message.
///
/// # Safety
/// `gpu` comes from `apps_webgpu_new`, and `size` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webgpu_image(
    gpu: *mut c_void,
    node: usize,
    size: *mut [u32; 2],
    out: *mut c_void,
    alloc: Alloc,
) -> i32 {
    let gpu = unsafe { &mut *gpu.cast::<Gpu>() };
    match gpu.image(node, unsafe { &mut *size }, out, alloc) {
        Ok(None) => 0,
        Ok(Some(false)) => 1,
        Ok(Some(true)) => 2,
        Err(error) => {
            output(out, alloc, error.to_string().as_bytes());
            -1
        }
    }
}

/// # Safety
/// `gpu` comes from `apps_webgpu_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webgpu_presented(gpu: *const c_void) -> bool {
    unsafe { &*gpu.cast::<Gpu>() }.presented()
}

/// Returns and resets the GPU bytes allocated since the last call.
///
/// # Safety
/// `gpu` comes from `apps_webgpu_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webgpu_allocated(gpu: *mut c_void) -> u64 {
    std::mem::take(&mut unsafe { &mut *gpu.cast::<Gpu>() }.allocated)
}

const FEATURES: &[(&str, Features)] = &[
    ("depth-clip-control", Features::DEPTH_CLIP_CONTROL),
    ("depth32float-stencil8", Features::DEPTH32FLOAT_STENCIL8),
    ("texture-compression-bc", Features::TEXTURE_COMPRESSION_BC),
    (
        "texture-compression-etc2",
        Features::TEXTURE_COMPRESSION_ETC2,
    ),
    (
        "texture-compression-astc",
        Features::TEXTURE_COMPRESSION_ASTC,
    ),
    ("shader-f16", Features::SHADER_F16),
    ("float32-filterable", Features::FLOAT32_FILTERABLE),
    (
        "rg11b10ufloat-renderable",
        Features::RG11B10UFLOAT_RENDERABLE,
    ),
    ("bgra8unorm-storage", Features::BGRA8UNORM_STORAGE),
];

#[derive(Default)]
struct Gpu {
    instance: Option<Instance>,
    adapters: HashMap<u64, Adapter>,
    devices: HashMap<u64, (Device, Queue, Sender<()>)>,
    resources: HashMap<u64, (u64, Resource)>,
    canvases: HashMap<usize, Canvas>,
    textures: HashMap<u64, HashSet<u64>>,
    mapped: HashMap<u64, std::ops::Range<u64>>,
    maps: HashMap<u64, (u64, std::ops::Range<u64>)>,
    done: Arc<Done>,
    allocated: u64,
    next: u64,
}

const IDLE: u8 = 0;
const PENDING: u8 = 1;
const READY: u8 = 2;
const FAILED: u8 = 3;

#[derive(Default)]
struct Done {
    list: Mutex<Vec<(u64, bool)>>,
    pending: AtomicUsize,
}

impl Done {
    fn start(self: &Arc<Self>, token: u64) -> impl FnOnce(bool) + Send + 'static {
        self.pending.fetch_add(1, Ordering::AcqRel);
        let (done, wake) = (self.clone(), Waker::current());
        move |ok| {
            done.list.lock().unwrap().push((token, ok));
            done.pending.fetch_sub(1, Ordering::AcqRel);
            wake.wake();
        }
    }
}

struct Canvas {
    device: u64,
    texture: u64,
    opaque: bool,
    dirty: bool,
    /// Whether the runtime holds an image of this canvas.
    image: bool,
    readbacks: [(Buffer, Arc<AtomicU8>, u64); 2],
    frames: u64,
}

enum Resource {
    Buffer(Buffer),
    Texture(Texture),
    View(TextureView),
    Sampler(Sampler),
    Shader(ShaderModule),
    BindLayout(BindGroupLayout),
    BindGroup(BindGroup),
    PipelineLayout(PipelineLayout),
    RenderPipeline(RenderPipeline),
    ComputePipeline(ComputePipeline),
}

macro_rules! resource {
    ($self:expr, $device:expr, $id:expr, $kind:ident) => {{
        match $self.resources.get(&number($id)?) {
            Some((owner, Resource::$kind(value))) if *owner == $device => value,
            _ => bail!(
                "Invalid {} or resource belongs to another device",
                stringify!($kind)
            ),
        }
    }};
}

fn number(value: &Value) -> Result<u64> {
    value.as_u64().context("Expected an unsigned integer")
}

fn decode<T: DeserializeOwned>(value: &Value, default: Value) -> Result<T> {
    Ok(serde_json::from_value(if value.is_null() {
        default
    } else {
        value.clone()
    })?)
}

fn bytes(value: &Value) -> Result<Vec<u8>> {
    if let Some(index) = value.get("$appsBytes").and_then(Value::as_u64) {
        let mut length = 0;
        let data = (host().read)(index as usize, &mut length);
        ensure!(!data.is_null(), "Invalid binary argument");
        return Ok(unsafe { std::slice::from_raw_parts(data, length) }.to_vec());
    }
    Ok(match value.as_str() {
        Some(text) => STANDARD.decode(text)?,
        None => Vec::deserialize(value)?,
    })
}

fn unmap(device: &Device, buffer: &Buffer) {
    // WARNING: wgpu keeps a failed mapping reserved until unmap, which then reports a validation error.
    let scope = device.push_error_scope(ErrorFilter::Validation);
    buffer.unmap();
    let _ = pollster::block_on(scope.pop());
}

fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn extent(value: &Value) -> Result<Extent3d> {
    Ok(Extent3d {
        width: u32::try_from(number(if value.is_array() {
            &value[0]
        } else {
            &value["width"]
        })?)?,
        height: u32::try_from(
            if value.is_array() {
                &value[1]
            } else {
                &value["height"]
            }
            .as_u64()
            .unwrap_or(1),
        )?,
        depth_or_array_layers: u32::try_from(
            if value.is_array() {
                &value[2]
            } else {
                &value["depthOrArrayLayers"]
            }
            .as_u64()
            .unwrap_or(1),
        )?,
    })
}

fn color(value: &Value) -> Color {
    Color {
        r: if value.is_array() {
            &value[0]
        } else {
            &value["r"]
        }
        .as_f64()
        .unwrap_or(0.),
        g: if value.is_array() {
            &value[1]
        } else {
            &value["g"]
        }
        .as_f64()
        .unwrap_or(0.),
        b: if value.is_array() {
            &value[2]
        } else {
            &value["b"]
        }
        .as_f64()
        .unwrap_or(0.),
        a: if value.is_array() {
            &value[3]
        } else {
            &value["a"]
        }
        .as_f64()
        .unwrap_or(0.),
    }
}

fn layout(value: &Value) -> Result<TexelCopyBufferLayout> {
    Ok(TexelCopyBufferLayout {
        offset: value["offset"].as_u64().unwrap_or(0),
        bytes_per_row: decode(&value["bytesPerRow"], Value::Null)?,
        rows_per_image: decode(&value["rowsPerImage"], Value::Null)?,
    })
}

impl Gpu {
    fn insert(&mut self, device: u64, resource: Resource) -> Value {
        self.next += 1;
        self.resources.insert(self.next, (device, resource));
        json!(self.next)
    }

    fn copy_texture<'a>(&'a self, device: u64, value: &Value) -> Result<TexelCopyTextureInfo<'a>> {
        let origin = &value["origin"];
        Ok(TexelCopyTextureInfo {
            texture: resource!(self, device, &value["texture"], Texture),
            mip_level: decode(&value["mipLevel"], json!(0))?,
            origin: Origin3d {
                x: decode(
                    if origin.is_array() {
                        &origin[0]
                    } else {
                        &origin["x"]
                    },
                    json!(0),
                )?,
                y: decode(
                    if origin.is_array() {
                        &origin[1]
                    } else {
                        &origin["y"]
                    },
                    json!(0),
                )?,
                z: decode(
                    if origin.is_array() {
                        &origin[2]
                    } else {
                        &origin["z"]
                    },
                    json!(0),
                )?,
            },
            aspect: decode(&value["aspect"], json!("all"))?,
        })
    }

    fn call(&mut self, op: &str, args: &Value) -> Result<Value> {
        if op == "stats" {
            return Ok(json!({"devices": self.devices.len(), "resources": self.resources.len()}));
        }
        if op == "adapter" {
            let instance = self.instance.get_or_insert_with(|| {
                let mut descriptor = InstanceDescriptor::new_without_display_handle();
                descriptor.backends = Backends::METAL | Backends::VULKAN | Backends::DX12;
                Instance::new(descriptor)
            });
            let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
                power_preference: decode(&args[0]["powerPreference"], json!("none"))?,
                force_fallback_adapter: args[0]["forceFallbackAdapter"] == true,
                compatible_surface: None,
                apply_limit_buckets: false,
            }));
            let Ok(adapter) = adapter else {
                return Ok(Value::Null);
            };
            self.next += 1;
            let info = adapter.get_info();
            let mut features: Vec<_> = FEATURES
                .iter()
                .filter(|(_, f)| adapter.features().contains(*f))
                .map(|(n, _)| *n)
                .collect();
            features.push("core-features-and-limits");
            let result = json!({"id":self.next,"features":features,"limits":adapter.limits(),"info":{"vendor":format!("{:x}",info.vendor),"architecture":"","device":format!("{:x}",info.device),"description":info.name,"isFallbackAdapter":info.device_type==DeviceType::Cpu}});
            self.adapters.insert(self.next, adapter);
            return Ok(result);
        }
        if op == "device" {
            let adapter = self
                .adapters
                .get(&number(&args[0])?)
                .context("Invalid adapter")?;
            let desc = &args[1];
            let mut features = Features::empty();
            for name in list(&desc["requiredFeatures"]) {
                if name == "core-features-and-limits" {
                    continue;
                }
                features |= FEATURES
                    .iter()
                    .find(|(n, _)| name == *n)
                    .context("Unsupported WebGPU feature")?
                    .1;
            }
            let limits: Limits = decode(&desc["requiredLimits"], json!({}))?;
            let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
                label: desc["label"].as_str(),
                required_features: features,
                required_limits: limits,
                memory_hints: MemoryHints::MemoryUsage,
                trace: Trace::Off,
                experimental_features: ExperimentalFeatures::disabled(),
            }))?;
            device.on_uncaptured_error(std::sync::Arc::new(|error| {
                eprintln!("apps: WebGPU error: {error}")
            }));
            self.next += 1;
            let mut names: Vec<_> = FEATURES
                .iter()
                .filter(|(_, f)| features.contains(*f))
                .map(|(n, _)| *n)
                .collect();
            names.push("core-features-and-limits");
            let result = json!({"id":self.next,"features":names,"limits":device.limits()});
            let (signal, wait) = std::sync::mpsc::channel::<()>();
            let (poller, done) = (device.clone(), self.done.clone());
            std::thread::Builder::new()
                .name("apps-gpu".into())
                .spawn(move || {
                    while wait.recv().is_ok() {
                        // WARNING: a waiting poll holds the device lock that `destroy()` needs, so each wait stays short.
                        while done.pending.load(Ordering::Acquire) > 0 {
                            let wait = PollType::Wait {
                                submission_index: None,
                                timeout: Some(std::time::Duration::from_millis(1)),
                            };
                            // WARNING: `Timeout` is the normal result while work is pending.
                            if let Ok(PollStatus::QueueEmpty) = poller.poll(wait) {
                                break;
                            }
                        }
                    }
                })?;
            self.devices.insert(self.next, (device, queue, signal));
            return Ok(result);
        }
        if op == "releaseAdapter" {
            self.adapters.remove(&number(&args[0])?);
            return Ok(Value::Null);
        }
        if op == "poll" {
            let done = std::mem::take(&mut *self.done.list.lock().unwrap());
            let mut results = Vec::with_capacity(done.len());
            for (token, ok) in done {
                let Some((key, range)) = self.maps.remove(&token) else {
                    results.push(json!([token, ok]));
                    continue;
                };
                let Some((owner, Resource::Buffer(buffer))) = self.resources.get(&key) else {
                    results.push(json!([token, null]));
                    continue;
                };
                match ok
                    .then(|| buffer.slice(range.clone()).get_mapped_range().ok())
                    .flatten()
                {
                    Some(view) => {
                        results.push(json!([token, base64(&view)]));
                        self.mapped.insert(key, range);
                    }
                    None => {
                        if let Some((device, ..)) = self.devices.get(owner) {
                            unmap(device, buffer);
                        }
                        results.push(json!([token, null]));
                    }
                }
            }
            return Ok(json!(results));
        }
        if op == "resetCanvas" {
            if let Some(canvas) = self.canvases.remove(&(number(&args[0])? as usize)) {
                self.resources.remove(&canvas.texture);
            }
            return Ok(Value::Null);
        }
        let id = number(&args[0])?;
        let (device, queue, _) = self
            .devices
            .get(&id)
            .context("GPU device is destroyed or invalid")?
            .clone();
        let validation = device.push_error_scope(ErrorFilter::Validation);
        let oom = device.push_error_scope(ErrorFilter::OutOfMemory);
        let result = self.execute(op, id, &device, &queue, &args[1]);
        let oom = pollster::block_on(oom.pop());
        let validation = pollster::block_on(validation.pop());
        if let Some(error) = oom.or(validation) {
            if op == "canvasTexture"
                && let Some(node) = args[1]["canvas"].as_u64()
            {
                self.canvases.remove(&(node as usize));
            } else if let Some(key) = result
                .as_ref()
                .ok()
                .and_then(Value::as_u64)
                .and_then(|key| match op {
                    "createBuffer" => self.mapped.remove(&key).map(|_| key),
                    "mapBuffer" => self.maps.remove(&key).map(|(key, _)| key),
                    _ => None,
                })
                && let Some((_, Resource::Buffer(buffer))) = self.resources.get(&key)
            {
                unmap(&device, buffer);
            }
            let kind = if matches!(error, Error::OutOfMemory { .. }) {
                "out-of-memory"
            } else {
                "validation"
            };
            return Ok(
                json!({"gpuError":error.to_string(),"kind":kind,"value":result.unwrap_or(Value::Null)}),
            );
        }
        result
    }

    fn execute(
        &mut self,
        op: &str,
        id: u64,
        device: &Device,
        queue: &Queue,
        p: &Value,
    ) -> Result<Value> {
        let label = p["label"].as_str();
        let resource = match op {
            "createBuffer" => {
                let size = number(&p["size"])?;
                let buffer = device.create_buffer(&BufferDescriptor {
                    label,
                    size,
                    usage: BufferUsages::from_bits(number(&p["usage"])? as u32)
                        .context("Invalid buffer usage")?,
                    mapped_at_creation: p["mappedAtCreation"] == true,
                });
                self.allocated += size;
                let key = self.insert(id, Resource::Buffer(buffer));
                if p["mappedAtCreation"] == true {
                    self.mapped.insert(self.next, 0..size);
                }
                return Ok(key);
            }
            "createTexture" => {
                let formats: Vec<TextureFormat> = decode(&p["viewFormats"], json!([]))?;
                let texture = device.create_texture(&TextureDescriptor {
                    label,
                    size: extent(&p["size"])?,
                    mip_level_count: decode(&p["mipLevelCount"], json!(1))?,
                    sample_count: decode(&p["sampleCount"], json!(1))?,
                    dimension: decode(&p["dimension"], json!("2d"))?,
                    format: decode(&p["format"], Value::Null)?,
                    usage: TextureUsages::from_bits(number(&p["usage"])? as u32)
                        .context("Invalid texture usage")?,
                    view_formats: &formats,
                });
                self.allocated += texture
                    .format()
                    .theoretical_memory_footprint(texture.size())
                    * texture.sample_count() as u64;
                Resource::Texture(texture)
            }
            "createView" => Resource::View(resource!(self, id, &p[0], Texture).create_view(
                &TextureViewDescriptor {
                    label: p[1]["label"].as_str(),
                    format: decode(&p[1]["format"], Value::Null)?,
                    dimension: decode(&p[1]["dimension"], Value::Null)?,
                    aspect: decode(&p[1]["aspect"], json!("all"))?,
                    base_mip_level: decode(&p[1]["baseMipLevel"], json!(0))?,
                    mip_level_count: decode(&p[1]["mipLevelCount"], Value::Null)?,
                    base_array_layer: decode(&p[1]["baseArrayLayer"], json!(0))?,
                    array_layer_count: decode(&p[1]["arrayLayerCount"], Value::Null)?,
                    usage: None,
                },
            )),
            "createSampler" => Resource::Sampler(device.create_sampler(&SamplerDescriptor {
                label,
                address_mode_u: decode(&p["addressModeU"], json!("clamp-to-edge"))?,
                address_mode_v: decode(&p["addressModeV"], json!("clamp-to-edge"))?,
                address_mode_w: decode(&p["addressModeW"], json!("clamp-to-edge"))?,
                mag_filter: decode(&p["magFilter"], json!("nearest"))?,
                min_filter: decode(&p["minFilter"], json!("nearest"))?,
                mipmap_filter: decode(&p["mipmapFilter"], json!("nearest"))?,
                lod_min_clamp: decode(&p["lodMinClamp"], json!(0))?,
                lod_max_clamp: decode(&p["lodMaxClamp"], json!(32))?,
                compare: decode(&p["compare"], Value::Null)?,
                anisotropy_clamp: decode(&p["maxAnisotropy"], json!(1))?,
                border_color: None,
            })),
            "getCompilationInfo" => {
                let info =
                    pollster::block_on(resource!(self, id, p, Shader).get_compilation_info());
                return Ok(
                    json!({"messages":info.messages.into_iter().map(|message| json!({
                    "message":message.message,
                    "type":match message.message_type { CompilationMessageType::Error => "error", CompilationMessageType::Warning => "warning", CompilationMessageType::Info => "info" },
                    "lineNum":message.location.map_or(0, |l| l.line_number),
                    "linePos":0,"offset":0,"length":0
                })).collect::<Vec<_>>()}),
                );
            }
            "createShaderModule" => {
                Resource::Shader(device.create_shader_module(ShaderModuleDescriptor {
                    label,
                    source: ShaderSource::Wgsl(
                        p["code"].as_str().context("Missing WGSL shader")?.into(),
                    ),
                }))
            }
            "createBindGroupLayout" => {
                let entries = list(&p["entries"])
                    .iter()
                    .map(|e| -> Result<_> {
                        let ty = if e["buffer"].is_object() {
                            BindingType::Buffer {
                                ty: match e["buffer"]["type"].as_str().unwrap_or("uniform") {
                                    "uniform" => BufferBindingType::Uniform,
                                    "storage" => BufferBindingType::Storage { read_only: false },
                                    "read-only-storage" => {
                                        BufferBindingType::Storage { read_only: true }
                                    }
                                    _ => bail!("Invalid buffer binding type"),
                                },
                                has_dynamic_offset: e["buffer"]["hasDynamicOffset"] == true,
                                min_binding_size: BufferSize::new(
                                    e["buffer"]["minBindingSize"].as_u64().unwrap_or(0),
                                ),
                            }
                        } else if e["sampler"].is_object() {
                            BindingType::Sampler(decode(&e["sampler"]["type"], json!("filtering"))?)
                        } else if e["texture"].is_object() {
                            BindingType::Texture {
                                sample_type: match e["texture"]["sampleType"]
                                    .as_str()
                                    .unwrap_or("float")
                                {
                                    "float" => TextureSampleType::Float { filterable: true },
                                    "unfilterable-float" => {
                                        TextureSampleType::Float { filterable: false }
                                    }
                                    "depth" => TextureSampleType::Depth,
                                    "sint" => TextureSampleType::Sint,
                                    "uint" => TextureSampleType::Uint,
                                    _ => bail!("Invalid texture sample type"),
                                },
                                view_dimension: decode(
                                    &e["texture"]["viewDimension"],
                                    json!("2d"),
                                )?,
                                multisampled: e["texture"]["multisampled"] == true,
                            }
                        } else if e["storageTexture"].is_object() {
                            BindingType::StorageTexture {
                                access: decode(
                                    &e["storageTexture"]["access"],
                                    json!("write-only"),
                                )?,
                                format: decode(&e["storageTexture"]["format"], Value::Null)?,
                                view_dimension: decode(
                                    &e["storageTexture"]["viewDimension"],
                                    json!("2d"),
                                )?,
                            }
                        } else {
                            bail!("Unsupported binding layout")
                        };
                        Ok(BindGroupLayoutEntry {
                            binding: decode(&e["binding"], Value::Null)?,
                            visibility: ShaderStages::from_bits(number(&e["visibility"])? as u32)
                                .context("Invalid shader visibility")?,
                            ty,
                            count: None,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Resource::BindLayout(device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                    label,
                    entries: &entries,
                }))
            }
            "createPipelineLayout" => {
                let layouts = list(&p["bindGroupLayouts"])
                    .iter()
                    .map(|v| -> Result<_> {
                        Ok(if v.is_null() {
                            None
                        } else {
                            Some(resource!(self, id, v, BindLayout))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Resource::PipelineLayout(device.create_pipeline_layout(&PipelineLayoutDescriptor {
                    label,
                    bind_group_layouts: &layouts,
                    immediate_size: 0,
                }))
            }
            "createBindGroup" => {
                let entries = list(&p["entries"])
                    .iter()
                    .map(|e| -> Result<_> {
                        let r = &e["resource"];
                        let resource = if r.is_object() {
                            BindingResource::Buffer(BufferBinding {
                                buffer: resource!(self, id, &r["buffer"], Buffer),
                                offset: r["offset"].as_u64().unwrap_or(0),
                                size: r["size"].as_u64().and_then(BufferSize::new),
                            })
                        } else {
                            match self.resources.get(&number(r)?) {
                                Some((owner, Resource::View(view))) if *owner == id => {
                                    BindingResource::TextureView(view)
                                }
                                Some((owner, Resource::Sampler(sampler))) if *owner == id => {
                                    BindingResource::Sampler(sampler)
                                }
                                _ => bail!("Invalid bind group resource"),
                            }
                        };
                        Ok(BindGroupEntry {
                            binding: decode(&e["binding"], Value::Null)?,
                            resource,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Resource::BindGroup(device.create_bind_group(&BindGroupDescriptor {
                    label,
                    layout: resource!(self, id, &p["layout"], BindLayout),
                    entries: &entries,
                }))
            }
            "createRenderPipeline" => {
                let vertex = &p["vertex"];
                let fragment = &p["fragment"];
                let attributes = list(&vertex["buffers"])
                    .iter()
                    .map(|b| decode::<Vec<VertexAttribute>>(&b["attributes"], json!([])))
                    .collect::<Result<Vec<_>>>()?;
                let buffers = list(&vertex["buffers"])
                    .iter()
                    .zip(&attributes)
                    .map(|(b, attrs)| -> Result<_> {
                        if b.is_null() {
                            return Ok(None);
                        }
                        Ok(Some(VertexBufferLayout {
                            array_stride: number(&b["arrayStride"])?,
                            step_mode: decode(&b["stepMode"], json!("vertex"))?,
                            attributes: attrs,
                        }))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let targets = list(&fragment["targets"])
                    .iter()
                    .map(|t| -> Result<_> {
                        if t.is_null() {
                            return Ok(None);
                        }
                        let blend = if t["blend"].is_null() {
                            None
                        } else {
                            let component = |c: &Value| -> Result<_> {
                                Ok(BlendComponent {
                                    src_factor: decode(&c["srcFactor"], json!("one"))?,
                                    dst_factor: decode(&c["dstFactor"], json!("zero"))?,
                                    operation: decode(&c["operation"], json!("add"))?,
                                })
                            };
                            Some(BlendState {
                                color: component(&t["blend"]["color"])?,
                                alpha: component(&t["blend"]["alpha"])?,
                            })
                        };
                        Ok(Some(ColorTargetState {
                            format: decode(&t["format"], Value::Null)?,
                            blend,
                            write_mask: ColorWrites::from_bits(
                                t["writeMask"].as_u64().unwrap_or(15) as u32,
                            )
                            .context("Invalid color write mask")?,
                        }))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let primitive = &p["primitive"];
                let depth = &p["depthStencil"];
                let stencil = |v: &Value| -> Result<_> {
                    Ok(StencilFaceState {
                        compare: decode(&v["compare"], json!("always"))?,
                        fail_op: decode(&v["failOp"], json!("keep"))?,
                        depth_fail_op: decode(&v["depthFailOp"], json!("keep"))?,
                        pass_op: decode(&v["passOp"], json!("keep"))?,
                    })
                };
                let vertex_constants: HashMap<String, f64> =
                    decode(&vertex["constants"], json!({}))?;
                let fragment_constants: HashMap<String, f64> =
                    decode(&fragment["constants"], json!({}))?;
                let vertex_constants: Vec<_> = vertex_constants
                    .iter()
                    .map(|(k, v)| (k.as_str(), *v))
                    .collect();
                let fragment_constants: Vec<_> = fragment_constants
                    .iter()
                    .map(|(k, v)| (k.as_str(), *v))
                    .collect();
                Resource::RenderPipeline(device.create_render_pipeline(&RenderPipelineDescriptor {
                    label,
                    layout: if p["layout"].is_number() {
                        Some(resource!(self, id, &p["layout"], PipelineLayout))
                    } else {
                        None
                    },
                    vertex: VertexState {
                        module: resource!(self, id, &vertex["module"], Shader),
                        entry_point: vertex["entryPoint"].as_str(),
                        compilation_options: PipelineCompilationOptions {
                            constants: &vertex_constants,
                            ..Default::default()
                        },
                        buffers: &buffers,
                    },
                    fragment: if fragment.is_null() {
                        None
                    } else {
                        Some(FragmentState {
                            module: resource!(self, id, &fragment["module"], Shader),
                            entry_point: fragment["entryPoint"].as_str(),
                            compilation_options: PipelineCompilationOptions {
                                constants: &fragment_constants,
                                ..Default::default()
                            },
                            targets: &targets,
                        })
                    },
                    primitive: PrimitiveState {
                        topology: decode(&primitive["topology"], json!("triangle-list"))?,
                        strip_index_format: decode(&primitive["stripIndexFormat"], Value::Null)?,
                        front_face: decode(&primitive["frontFace"], json!("ccw"))?,
                        cull_mode: if primitive["cullMode"] == "none" {
                            None
                        } else {
                            decode(&primitive["cullMode"], Value::Null)?
                        },
                        unclipped_depth: primitive["unclippedDepth"] == true,
                        ..Default::default()
                    },
                    depth_stencil: if depth.is_null() {
                        None
                    } else {
                        Some(DepthStencilState {
                            format: decode(&depth["format"], Value::Null)?,
                            depth_write_enabled: Some(depth["depthWriteEnabled"] == true),
                            depth_compare: decode(&depth["depthCompare"], json!("always"))?,
                            stencil: StencilState {
                                front: stencil(&depth["stencilFront"])?,
                                back: stencil(&depth["stencilBack"])?,
                                read_mask: decode(&depth["stencilReadMask"], json!(u32::MAX))?,
                                write_mask: decode(&depth["stencilWriteMask"], json!(u32::MAX))?,
                            },
                            bias: DepthBiasState {
                                constant: decode(&depth["depthBias"], json!(0))?,
                                slope_scale: decode(&depth["depthBiasSlopeScale"], json!(0))?,
                                clamp: decode(&depth["depthBiasClamp"], json!(0))?,
                            },
                        })
                    },
                    multisample: MultisampleState {
                        count: decode(&p["multisample"]["count"], json!(1))?,
                        mask: p["multisample"]["mask"].as_u64().unwrap_or(u64::MAX),
                        alpha_to_coverage_enabled: p["multisample"]["alphaToCoverageEnabled"]
                            == true,
                    },
                    multiview_mask: None,
                    cache: None,
                }))
            }
            "createComputePipeline" => {
                let constants: HashMap<String, f64> =
                    decode(&p["compute"]["constants"], json!({}))?;
                Resource::ComputePipeline(
                    device.create_compute_pipeline(&ComputePipelineDescriptor {
                        label,
                        layout: if p["layout"].is_number() {
                            Some(resource!(self, id, &p["layout"], PipelineLayout))
                        } else {
                            None
                        },
                        module: resource!(self, id, &p["compute"]["module"], Shader),
                        entry_point: p["compute"]["entryPoint"].as_str(),
                        compilation_options: PipelineCompilationOptions {
                            constants: &constants
                                .iter()
                                .map(|(k, v)| (k.as_str(), *v))
                                .collect::<Vec<_>>(),
                            ..Default::default()
                        },
                        cache: None,
                    }),
                )
            }
            "getBindGroupLayout" => {
                let index = decode(&p[1], Value::Null)?;
                let value = match self.resources.get(&number(&p[0])?) {
                    Some((owner, Resource::RenderPipeline(pipeline))) if *owner == id => {
                        pipeline.get_bind_group_layout(index)
                    }
                    Some((owner, Resource::ComputePipeline(pipeline))) if *owner == id => {
                        pipeline.get_bind_group_layout(index)
                    }
                    _ => bail!("Invalid pipeline"),
                };
                Resource::BindLayout(value)
            }
            "writeBuffer" => {
                queue.write_buffer(
                    resource!(self, id, &p[0], Buffer),
                    number(&p[1])?,
                    &bytes(&p[2])?,
                );
                return Ok(Value::Null);
            }
            "writeTexture" => {
                queue.write_texture(
                    self.copy_texture(id, &p[0])?,
                    &bytes(&p[1])?,
                    layout(&p[2])?,
                    extent(&p[3])?,
                );
                for canvas in self
                    .canvases
                    .values_mut()
                    .filter(|canvas| p[0]["texture"] == canvas.texture)
                {
                    canvas.dirty = true;
                }
                return Ok(Value::Null);
            }
            "mapBuffer" => {
                let key = number(&p[0])?;
                let buffer = resource!(self, id, &p[0], Buffer);
                let offset = number(&p[2])?;
                let size = number(&p[3])?;
                let end = offset.checked_add(size).context("Buffer range overflow")?;
                ensure!(
                    !self.mapped.contains_key(&key) && !self.maps.values().any(|(k, _)| *k == key),
                    "Buffer is already mapped"
                );
                ensure!(
                    size > 0 && offset % 8 == 0 && size % 4 == 0 && end <= buffer.size(),
                    "Invalid mapped buffer range"
                );
                queue.submit([]);
                self.next += 1;
                self.maps.insert(self.next, (key, offset..end));
                let done = self.done.start(self.next);
                buffer.slice(offset..end).map_async(
                    if p[1] == 1 {
                        MapMode::Read
                    } else {
                        MapMode::Write
                    },
                    move |result| done(result.is_ok()),
                );
                if let Some((.., signal)) = self.devices.get(&id) {
                    let _ = signal.send(());
                }
                return Ok(json!(self.next));
            }
            "unmapBuffer" => {
                let key = number(&p[0])?;
                let buffer = resource!(self, id, &p[0], Buffer);
                let before = self.maps.len();
                self.maps.retain(|_, (k, _)| *k != key);
                if self.maps.len() < before {
                    buffer.unmap();
                    return Ok(Value::Null);
                }
                let Some(mapped) = self.mapped.get(&key) else {
                    return Ok(Value::Null);
                };
                for range in list(&p[1]) {
                    let bytes = bytes(&range[1])?;
                    let offset = number(&range[0])?;
                    if !bytes.is_empty() {
                        let end = offset
                            .checked_add(bytes.len() as u64)
                            .context("Buffer range overflow")?;
                        ensure!(
                            offset % 8 == 0
                                && bytes.len() % 4 == 0
                                && mapped.start <= offset
                                && end <= mapped.end,
                            "Invalid mapped buffer range"
                        );
                        buffer
                            .slice(offset..end)
                            .get_mapped_range_mut()?
                            .copy_from_slice(&bytes);
                    }
                }
                buffer.unmap();
                self.mapped.remove(&key);
                return Ok(Value::Null);
            }
            "destroyBuffer" => {
                resource!(self, id, p, Buffer).destroy();
                self.resources.remove(&number(p)?);
                return Ok(Value::Null);
            }
            "destroyTexture" => {
                resource!(self, id, p, Texture).destroy();
                self.resources.remove(&number(p)?);
                self.canvases.retain(|_, canvas| *p != canvas.texture);
                return Ok(Value::Null);
            }
            "release" => {
                for value in list(p) {
                    let key = number(value)?;
                    if self
                        .resources
                        .get(&key)
                        .is_some_and(|(owner, _)| *owner == id)
                    {
                        self.resources.remove(&key);
                        self.textures.remove(&key);
                    }
                }
                return Ok(Value::Null);
            }
            "destroyDevice" => {
                device.destroy();
                self.resources.retain(|_, (owner, _)| *owner != id);
                self.textures
                    .retain(|key, _| self.resources.contains_key(key));
                self.mapped
                    .retain(|key, _| self.resources.contains_key(key));
                self.canvases.retain(|_, c| c.device != id);
                self.devices.remove(&id);
                return Ok(Value::Null);
            }
            "workDone" => {
                self.next += 1;
                let done = self.done.start(self.next);
                queue.on_submitted_work_done(move || done(true));
                if let Some((.., signal)) = self.devices.get(&id) {
                    let _ = signal.send(());
                }
                return Ok(json!(self.next));
            }
            "canvasTexture" => {
                let node = number(&p["canvas"])? as usize;
                let width = decode(&p["width"], Value::Null)?;
                let height = decode(&p["height"], Value::Null)?;
                ensure!(
                    width > 0
                        && height > 0
                        && width <= device.limits().max_texture_dimension_2d
                        && height <= device.limits().max_texture_dimension_2d,
                    "Canvas dimensions exceed GPU limits"
                );
                let format: TextureFormat = decode(&p["format"], Value::Null)?;
                ensure!(
                    matches!(
                        format,
                        TextureFormat::Bgra8Unorm | TextureFormat::Rgba8Unorm
                    ),
                    "Canvas format must be bgra8unorm or rgba8unorm"
                );
                let usage = TextureUsages::from_bits(p["usage"].as_u64().unwrap_or(16) as u32)
                    .context("Invalid canvas usage")?
                    | TextureUsages::COPY_SRC;
                if let Some(canvas) = self.canvases.get(&node)
                    && canvas.device == id
                    && let Some((_, Resource::Texture(t))) = self.resources.get(&canvas.texture)
                    && t.width() == width
                    && t.height() == height
                    && t.format() == format
                    && t.usage() == usage
                {
                    return Ok(json!(canvas.texture));
                }
                if let Some(old) = self.canvases.remove(&node) {
                    self.resources.remove(&old.texture);
                }
                let texture = device.create_texture(&TextureDescriptor {
                    label: Some("canvas"),
                    size: Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                });
                let key = self.insert(id, Resource::Texture(texture));
                let size = (width * 4).div_ceil(COPY_BYTES_PER_ROW_ALIGNMENT)
                    * COPY_BYTES_PER_ROW_ALIGNMENT
                    * height;
                self.allocated += width as u64 * height as u64 * 4 + 2 * size as u64;
                let readback = || {
                    let buffer = device.create_buffer(&BufferDescriptor {
                        label: Some("canvas readback"),
                        size: size as u64,
                        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    (buffer, Arc::default(), 0)
                };
                self.canvases.insert(
                    node,
                    Canvas {
                        device: id,
                        texture: number(&key)?,
                        opaque: p["alphaMode"] != "premultiplied",
                        dirty: true,
                        image: false,
                        readbacks: [readback(), readback()],
                        frames: 0,
                    },
                );
                return Ok(key);
            }
            "submit" => {
                let mut touched = HashSet::new();
                let mut pending: Vec<_> = list(p).iter().flat_map(list).collect();
                while let Some(command) = pending.pop() {
                    match command[0].as_str() {
                        Some("render" | "compute") => {
                            for attachment in list(&command[1]["colorAttachments"]) {
                                for key in ["view", "resolveTarget"] {
                                    if let Some(textures) = attachment[key]
                                        .as_u64()
                                        .and_then(|id| self.textures.get(&id))
                                    {
                                        touched.extend(textures);
                                    }
                                }
                            }
                            if let Some(textures) = command[1]["depthStencilAttachment"]["view"]
                                .as_u64()
                                .and_then(|id| self.textures.get(&id))
                            {
                                touched.extend(textures);
                            }
                            pending.extend(list(&command[2]));
                        }
                        Some("executeBundles") => {
                            pending.extend(list(&command[1]).iter().flat_map(list))
                        }
                        Some("setBindGroup") => {
                            if let Some(textures) =
                                command[2].as_u64().and_then(|id| self.textures.get(&id))
                            {
                                touched.extend(textures);
                            }
                        }
                        Some("copyTextureToTexture" | "copyBufferToTexture") => {
                            if let Some(texture) = command[2]["texture"].as_u64() {
                                touched.insert(texture);
                            }
                        }
                        _ => {}
                    }
                }
                let mut buffers = Vec::new();
                for commands in list(p) {
                    let mut encoder =
                        device.create_command_encoder(&CommandEncoderDescriptor::default());
                    for c in list(commands) {
                        let op = c[0].as_str().context("Missing GPU command")?;
                        match op {
                            "render" => {
                                let d = &c[1];
                                ensure!(
                                    d["occlusionQuerySet"].is_null()
                                        && d["timestampWrites"].is_null(),
                                    "GPU queries are not supported"
                                );
                                let colors = list(&d["colorAttachments"])
                                    .iter()
                                    .map(|a| -> Result<_> {
                                        if a.is_null() {
                                            return Ok(None);
                                        }
                                        Ok(Some(RenderPassColorAttachment {
                                            view: resource!(self, id, &a["view"], View),
                                            depth_slice: None,
                                            resolve_target: if a["resolveTarget"].is_null() {
                                                None
                                            } else {
                                                Some(resource!(self, id, &a["resolveTarget"], View))
                                            },
                                            ops: Operations {
                                                load: match a["loadOp"].as_str() {
                                                    Some("clear") => {
                                                        LoadOp::Clear(color(&a["clearValue"]))
                                                    }
                                                    Some("load") => LoadOp::Load,
                                                    _ => bail!("Invalid color loadOp"),
                                                },
                                                store: decode(&a["storeOp"], Value::Null)?,
                                            },
                                        }))
                                    })
                                    .collect::<Result<Vec<_>>>()?;
                                let dsa = &d["depthStencilAttachment"];
                                let depth_stencil_attachment = if dsa.is_null() {
                                    None
                                } else {
                                    Some(RenderPassDepthStencilAttachment {
                                        view: resource!(self, id, &dsa["view"], View),
                                        depth_ops: if dsa["depthReadOnly"] == true
                                            || dsa["depthLoadOp"].is_null()
                                        {
                                            None
                                        } else {
                                            Some(Operations {
                                                load: if dsa["depthLoadOp"] == "clear" {
                                                    LoadOp::Clear(decode(
                                                        &dsa["depthClearValue"],
                                                        json!(1),
                                                    )?)
                                                } else {
                                                    LoadOp::Load
                                                },
                                                store: decode(&dsa["depthStoreOp"], Value::Null)?,
                                            })
                                        },
                                        stencil_ops: if dsa["stencilReadOnly"] == true
                                            || dsa["stencilLoadOp"].is_null()
                                        {
                                            None
                                        } else {
                                            Some(Operations {
                                                load: if dsa["stencilLoadOp"] == "clear" {
                                                    LoadOp::Clear(decode(
                                                        &dsa["stencilClearValue"],
                                                        json!(0),
                                                    )?)
                                                } else {
                                                    LoadOp::Load
                                                },
                                                store: decode(&dsa["stencilStoreOp"], Value::Null)?,
                                            })
                                        },
                                    })
                                };
                                let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                                    label: d["label"].as_str(),
                                    color_attachments: &colors,
                                    depth_stencil_attachment,
                                    timestamp_writes: None,
                                    occlusion_query_set: None,
                                    multiview_mask: None,
                                });
                                for command in list(&c[2]) {
                                    self.render_command(id, &mut pass, command)?;
                                }
                            }
                            "compute" => {
                                ensure!(
                                    c[1]["timestampWrites"].is_null(),
                                    "GPU queries are not supported"
                                );
                                let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                                    label: c[1]["label"].as_str(),
                                    timestamp_writes: None,
                                });
                                for cmd in list(&c[2]) {
                                    match cmd[0].as_str().context("Missing compute command")? {
                                        "setPipeline" => pass.set_pipeline(resource!(
                                            self,
                                            id,
                                            &cmd[1],
                                            ComputePipeline
                                        )),
                                        "setBindGroup" => {
                                            let offsets: Vec<u32> = decode(&cmd[3], json!([]))?;
                                            pass.set_bind_group(
                                                decode(&cmd[1], Value::Null)?,
                                                resource!(self, id, &cmd[2], BindGroup),
                                                &offsets,
                                            );
                                        }
                                        "dispatchWorkgroups" => pass.dispatch_workgroups(
                                            decode(&cmd[1], Value::Null)?,
                                            decode(&cmd[2], json!(1))?,
                                            decode(&cmd[3], json!(1))?,
                                        ),
                                        "dispatchWorkgroupsIndirect" => pass
                                            .dispatch_workgroups_indirect(
                                                resource!(self, id, &cmd[1], Buffer),
                                                number(&cmd[2])?,
                                            ),
                                        "pushDebugGroup" => {
                                            pass.push_debug_group(cmd[1].as_str().unwrap_or(""))
                                        }
                                        "popDebugGroup" => pass.pop_debug_group(),
                                        "insertDebugMarker" => {
                                            pass.insert_debug_marker(cmd[1].as_str().unwrap_or(""))
                                        }
                                        name => bail!("Unsupported compute command: {name}"),
                                    }
                                }
                            }
                            "copyBufferToBuffer" => encoder.copy_buffer_to_buffer(
                                resource!(self, id, &c[1], Buffer),
                                number(&c[2])?,
                                resource!(self, id, &c[3], Buffer),
                                number(&c[4])?,
                                number(&c[5])?,
                            ),
                            "copyTextureToTexture" => encoder.copy_texture_to_texture(
                                self.copy_texture(id, &c[1])?,
                                self.copy_texture(id, &c[2])?,
                                extent(&c[3])?,
                            ),
                            "copyTextureToBuffer" => encoder.copy_texture_to_buffer(
                                self.copy_texture(id, &c[1])?,
                                TexelCopyBufferInfo {
                                    buffer: resource!(self, id, &c[2]["buffer"], Buffer),
                                    layout: layout(&c[2])?,
                                },
                                extent(&c[3])?,
                            ),
                            "copyBufferToTexture" => encoder.copy_buffer_to_texture(
                                TexelCopyBufferInfo {
                                    buffer: resource!(self, id, &c[1]["buffer"], Buffer),
                                    layout: layout(&c[1])?,
                                },
                                self.copy_texture(id, &c[2])?,
                                extent(&c[3])?,
                            ),
                            "clearBuffer" => encoder.clear_buffer(
                                resource!(self, id, &c[1], Buffer),
                                c[2].as_u64().unwrap_or(0),
                                c[3].as_u64(),
                            ),
                            "pushDebugGroup" => {
                                encoder.push_debug_group(c[1].as_str().unwrap_or(""))
                            }
                            "popDebugGroup" => encoder.pop_debug_group(),
                            "insertDebugMarker" => {
                                encoder.insert_debug_marker(c[1].as_str().unwrap_or(""))
                            }
                            name => bail!("Unsupported GPU command: {name}"),
                        }
                    }
                    buffers.push(encoder.finish());
                }
                queue.submit(buffers);
                for canvas in self
                    .canvases
                    .values_mut()
                    .filter(|c| c.device == id && touched.contains(&c.texture))
                {
                    canvas.dirty = true;
                }
                return Ok(Value::Null);
            }
            _ => bail!("Unsupported WebGPU operation: {op}"),
        };
        let textures = match op {
            "createView" => HashSet::from([number(&p[0])?]),
            "createBindGroup" => list(&p["entries"])
                .iter()
                .filter_map(|entry| {
                    entry["resource"]
                        .as_u64()
                        .and_then(|id| self.textures.get(&id))
                })
                .flatten()
                .copied()
                .collect(),
            _ => HashSet::new(),
        };
        let key = self.insert(id, resource);
        if !textures.is_empty() {
            self.textures.insert(number(&key)?, textures);
        }
        Ok(key)
    }

    fn render_command(&self, id: u64, pass: &mut RenderPass<'_>, c: &Value) -> Result<()> {
        match c[0].as_str().context("Missing render command")? {
            "setPipeline" => pass.set_pipeline(resource!(self, id, &c[1], RenderPipeline)),
            "setBindGroup" => {
                let offsets: Vec<u32> = decode(&c[3], json!([]))?;
                pass.set_bind_group(
                    decode(&c[1], Value::Null)?,
                    resource!(self, id, &c[2], BindGroup),
                    &offsets,
                );
            }
            "setVertexBuffer" => {
                let buffer = resource!(self, id, &c[2], Buffer);
                let offset = c[3].as_u64().unwrap_or(0);
                let end = c[4]
                    .as_u64()
                    .map(|s| s.checked_add(offset).context("Buffer range overflow"))
                    .transpose()?
                    .unwrap_or(buffer.size());
                ensure!(offset < end && end <= buffer.size(), "Invalid buffer slice");
                pass.set_vertex_buffer(decode(&c[1], Value::Null)?, buffer.slice(offset..end));
            }
            "setIndexBuffer" => {
                let buffer = resource!(self, id, &c[1], Buffer);
                let offset = c[3].as_u64().unwrap_or(0);
                let end = c[4]
                    .as_u64()
                    .map(|s| s.checked_add(offset).context("Buffer range overflow"))
                    .transpose()?
                    .unwrap_or(buffer.size());
                ensure!(offset < end && end <= buffer.size(), "Invalid buffer slice");
                pass.set_index_buffer(buffer.slice(offset..end), decode(&c[2], Value::Null)?);
            }
            "draw" | "drawIndexed" => {
                let count: u32 = decode(&c[1], Value::Null)?;
                let instances: u32 = decode(&c[2], json!(1))?;
                let first: u32 = decode(&c[3], json!(0))?;
                let indexed = c[0] == "drawIndexed";
                let first_instance: u32 = decode(&c[if indexed { 5 } else { 4 }], json!(0))?;
                let range = first..first.checked_add(count).context("Draw range overflow")?;
                let instances = first_instance
                    ..first_instance
                        .checked_add(instances)
                        .context("Instance range overflow")?;
                if indexed {
                    pass.draw_indexed(range, decode(&c[4], json!(0))?, instances);
                } else {
                    pass.draw(range, instances);
                }
            }
            "drawIndirect" => {
                pass.draw_indirect(resource!(self, id, &c[1], Buffer), number(&c[2])?)
            }
            "drawIndexedIndirect" => {
                pass.draw_indexed_indirect(resource!(self, id, &c[1], Buffer), number(&c[2])?)
            }
            "setViewport" => pass.set_viewport(
                decode(&c[1], Value::Null)?,
                decode(&c[2], Value::Null)?,
                decode(&c[3], Value::Null)?,
                decode(&c[4], Value::Null)?,
                decode(&c[5], Value::Null)?,
                decode(&c[6], Value::Null)?,
            ),
            "setScissorRect" => pass.set_scissor_rect(
                decode(&c[1], Value::Null)?,
                decode(&c[2], Value::Null)?,
                decode(&c[3], Value::Null)?,
                decode(&c[4], Value::Null)?,
            ),
            "setStencilReference" => pass.set_stencil_reference(decode(&c[1], Value::Null)?),
            "setBlendConstant" => pass.set_blend_constant(color(&c[1])),
            "pushDebugGroup" => pass.push_debug_group(c[1].as_str().unwrap_or("")),
            "popDebugGroup" => pass.pop_debug_group(),
            "insertDebugMarker" => pass.insert_debug_marker(c[1].as_str().unwrap_or("")),
            "executeBundles" => {
                for bundle in list(&c[1]) {
                    for command in list(bundle) {
                        self.render_command(id, pass, command)?;
                    }
                }
            }
            name => bail!("Unsupported render command: {name}"),
        }
        Ok(())
    }

    fn presented(&self) -> bool {
        self.canvases.values().any(|canvas| {
            canvas
                .readbacks
                .iter()
                .any(|(_, state, _)| state.load(Ordering::Acquire) >= READY)
        })
    }

    /// Returns `None` until the canvas has an image, then whether a new frame of `size` went to `alloc`.
    fn image(
        &mut self,
        node: usize,
        size: &mut [u32; 2],
        out: *mut c_void,
        alloc: Alloc,
    ) -> Result<Option<bool>> {
        let Some(canvas) = self.canvases.get_mut(&node) else {
            return Ok(None);
        };
        let (device, queue, signal) = self
            .devices
            .get(&canvas.device)
            .context("Canvas device is destroyed")?;
        let Some((_, Resource::Texture(texture))) = self.resources.get(&canvas.texture) else {
            return Ok(None);
        };
        let (width, height) = (texture.width(), texture.height());
        let stride =
            (width * 4).div_ceil(COPY_BYTES_PER_ROW_ALIGNMENT) * COPY_BYTES_PER_ROW_ALIGNMENT;
        let mut fresh = false;
        let newest = canvas
            .readbacks
            .iter()
            .filter(|(_, state, _)| state.load(Ordering::Acquire) == READY)
            .map(|(_, _, frame)| *frame)
            .max();
        for (buffer, state, frame) in &canvas.readbacks {
            let status = state.load(Ordering::Acquire);
            if status < READY {
                continue;
            }
            let written = (newest == Some(*frame))
                .then(|| buffer.slice(..).get_mapped_range())
                .map(|view| -> Result<_> {
                    let view = view?;
                    let row = width as usize * 4;
                    let length = row * height as usize;
                    // WARNING: the runtime buffer is uninitialized until every byte is written here.
                    let pixels =
                        unsafe { std::slice::from_raw_parts_mut(alloc(out, length), length) };
                    for (source, target) in view.chunks(stride as usize).zip(pixels.chunks_mut(row))
                    {
                        target.copy_from_slice(&source[..row]);
                    }
                    let rgba = texture.format() == TextureFormat::Rgba8Unorm;
                    let alpha = if canvas.opaque { 0xff00_0000 } else { 0 };
                    if rgba || canvas.opaque {
                        for pixel in pixels.as_chunks_mut::<4>().0 {
                            let value = u32::from_le_bytes(*pixel);
                            let value = if rgba {
                                value & 0xff00_ff00 | (value & 0xff) << 16 | (value >> 16) & 0xff
                            } else {
                                value
                            };
                            *pixel = (value | alpha).to_le_bytes();
                        }
                    }
                    Ok(())
                });
            if status == FAILED {
                unmap(device, buffer);
            } else {
                buffer.unmap();
            }
            state.store(IDLE, Ordering::Release);
            if let Some(written) = written {
                written?;
                *size = [width, height];
                fresh = true;
            }
        }
        if canvas.dirty
            && let Some((buffer, state, frame)) = canvas
                .readbacks
                .iter_mut()
                .find(|(_, state, _)| state.load(Ordering::Acquire) == IDLE)
        {
            canvas.dirty = false;
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                TexelCopyBufferInfo {
                    buffer,
                    layout: TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(stride),
                        rows_per_image: Some(height),
                    },
                },
                texture.size(),
            );
            queue.submit([encoder.finish()]);
            canvas.frames += 1;
            *frame = canvas.frames;
            state.store(PENDING, Ordering::Release);
            self.done.pending.fetch_add(1, Ordering::AcqRel);
            let (state, done, wake) = (state.clone(), self.done.clone(), Waker::current());
            buffer.slice(..).map_async(MapMode::Read, move |result| {
                state.store(
                    if result.is_ok() { READY } else { FAILED },
                    Ordering::Release,
                );
                done.pending.fetch_sub(1, Ordering::AcqRel);
                wake.wake();
            });
            let _ = signal.send(());
        }
        canvas.image |= fresh;
        Ok(canvas.image.then_some(fresh))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn read(_: usize, _: *mut usize) -> *const u8 {
        std::ptr::null()
    }
    extern "C" fn write(_: *const u8, _: usize) -> usize {
        usize::MAX
    }
    extern "C" fn waker() -> *mut c_void {
        std::ptr::null_mut()
    }
    extern "C" fn wake(_: *mut c_void) {}

    #[test]
    #[ignore = "Requires a native GPU adapter"]
    fn submits_only_dirty_used_canvases() {
        HOST.get_or_init(|| Host {
            read,
            write,
            waker,
            wake,
            drop_waker: wake,
        });
        let mut gpu = Gpu::default();
        let adapter = gpu.call("adapter", &json!([{}])).unwrap()["id"].clone();
        let device = gpu.call("device", &json!([adapter, {}])).unwrap()["id"].clone();
        let texture = gpu
            .call(
                "canvasTexture",
                &json!([device,{"canvas":1,"width":4,"height":4,"format":"rgba8unorm","usage":16}]),
            )
            .unwrap();
        gpu.call(
            "canvasTexture",
            &json!([device,{"canvas":2,"width":4,"height":4,"format":"rgba8unorm","usage":16}]),
        )
        .unwrap();
        for canvas in gpu.canvases.values_mut() {
            canvas.dirty = false;
        }
        gpu.call("submit", &json!([device, [[["compute", {}, []]]]]))
            .unwrap();
        assert!(gpu.canvases.values().all(|canvas| !canvas.dirty));
        let view = gpu
            .call("createView", &json!([device, [texture, {}]]))
            .unwrap();
        gpu.call("submit", &json!([device,[[["render",{"colorAttachments":[{"view":view,"loadOp":"clear","storeOp":"store","clearValue":[1,0,0,1]}]},[]]]]])).unwrap();
        assert!(gpu.canvases[&1].dirty);
        assert!(!gpu.canvases[&2].dirty);
        gpu.call("destroyDevice", &json!([device, null])).unwrap();
    }
}
