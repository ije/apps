//! WebGPU lives in `apps-webgpu`. macOS and Windows load it as a library that `apps build` copies only
//! for apps with the `webgpu` feature. Linux links it, because the render crate draws with wgpu there.

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::Value;
use std::{
    collections::HashMap,
    ffi::c_void,
    ptr::NonNull,
    sync::{Arc, OnceLock},
};

const ABI: u32 = 1;

type Alloc = extern "C" fn(*mut c_void, usize) -> *mut u8;

#[repr(C)]
struct Host {
    read: extern "C" fn(usize, *mut usize) -> *const u8,
    write: extern "C" fn(*const u8, usize) -> usize,
    waker: extern "C" fn() -> *mut c_void,
    wake: extern "C" fn(*mut c_void),
    drop_waker: extern "C" fn(*mut c_void),
}

static HOST: Host = Host {
    read,
    write,
    waker,
    wake,
    drop_waker,
};

extern "C" fn read(index: usize, length: *mut usize) -> *const u8 {
    let Some((bytes, size)) = crate::jsc::payload::get(index) else {
        return std::ptr::null();
    };
    unsafe { *length = size };
    bytes
}

extern "C" fn write(bytes: *const u8, length: usize) -> usize {
    crate::jsc::payload::write(unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec())["$appsBytes"]
        .as_u64()
        .map_or(usize::MAX, |index| index as usize)
}

extern "C" fn waker() -> *mut c_void {
    Box::into_raw(Box::new(crate::host::Waker::current())).cast()
}

extern "C" fn wake(waker: *mut c_void) {
    unsafe { &*waker.cast::<crate::host::Waker>() }.wake();
}

extern "C" fn drop_waker(waker: *mut c_void) {
    drop(unsafe { Box::from_raw(waker.cast::<crate::host::Waker>()) });
}

/// Bytes that the library writes. `alloc` leaves them uninitialized, and `finish` trusts that the
/// library wrote all of them.
#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    length: usize,
}

impl Output {
    fn finish(mut self) -> Vec<u8> {
        unsafe { self.bytes.set_len(self.length) };
        self.bytes
    }
}

extern "C" fn alloc(output: *mut c_void, length: usize) -> *mut u8 {
    let output = unsafe { &mut *output.cast::<Output>() };
    output.bytes.clear();
    output.bytes.reserve_exact(length);
    output.length = length;
    output.bytes.as_mut_ptr()
}

struct Library {
    new: unsafe extern "C" fn(*const c_void) -> *mut c_void,
    free: unsafe extern "C" fn(*mut c_void),
    call: unsafe extern "C" fn(
        *mut c_void,
        *const u8,
        usize,
        *const u8,
        usize,
        *mut c_void,
        Alloc,
    ) -> bool,
    image: unsafe extern "C" fn(*mut c_void, usize, *mut [u32; 2], *mut c_void, Alloc) -> i32,
    presented: unsafe extern "C" fn(*const c_void) -> bool,
    allocated: unsafe extern "C" fn(*mut c_void) -> u64,
}

/// The file name of the WebGPU library, or `None` where the runtime links WebGPU.
pub fn library_name(platform: &str) -> Option<&'static str> {
    match platform {
        "macos" => Some("libapps_webgpu.dylib"),
        "windows" => Some("apps_webgpu.dll"),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn load() -> Result<Library> {
    use apps_webgpu as webgpu;
    ensure!(webgpu::ABI == ABI, "WebGPU does not match this runtime");
    Ok(Library {
        new: webgpu::apps_webgpu_new,
        free: webgpu::apps_webgpu_free,
        call: webgpu::apps_webgpu_call,
        image: webgpu::apps_webgpu_image,
        presented: webgpu::apps_webgpu_presented,
        allocated: webgpu::apps_webgpu_allocated,
    })
}

#[cfg(not(target_os = "linux"))]
fn load() -> Result<Library> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let directory = executable
        .parent()
        .context("Missing executable directory")?;
    let name =
        library_name(std::env::consts::OS).context("WebGPU is unavailable on this platform")?;
    // WARNING: like JavaScriptCore, the library only loads from the app, never from the environment.
    let path = ["../Frameworks", "."]
        .iter()
        .map(|folder| directory.join(folder).join(name))
        .find(|path| path.is_file())
        .with_context(|| format!("The WebGPU library {name} is missing next to the runtime"))?;
    unsafe {
        let library = libloading::Library::new(&path)
            .with_context(|| format!("Cannot load WebGPU from {}", path.display()))?;
        let abi: libloading::Symbol<extern "C" fn() -> u32> = library.get(b"apps_webgpu_abi\0")?;
        ensure!(
            abi() == ABI,
            "{} does not match this runtime",
            path.display()
        );
        let functions = Library {
            new: *library.get(b"apps_webgpu_new\0")?,
            free: *library.get(b"apps_webgpu_free\0")?,
            call: *library.get(b"apps_webgpu_call\0")?,
            image: *library.get(b"apps_webgpu_image\0")?,
            presented: *library.get(b"apps_webgpu_presented\0")?,
            allocated: *library.get(b"apps_webgpu_allocated\0")?,
        };
        // WARNING: wgpu threads may outlive every device, so the library is never unloaded.
        std::mem::forget(library);
        Ok(functions)
    }
}

/// The WebGPU state of one page. The library loads on the first call.
#[derive(Default)]
pub struct Gpu {
    raw: Option<(NonNull<c_void>, &'static Library)>,
    images: HashMap<usize, Arc<renderer::RenderImage>>,
}

impl Gpu {
    /// Runs `op` with arguments as JSON text, which the library parses.
    pub fn call(&mut self, op: &str, args: &[u8]) -> Result<Value> {
        let (raw, library) = match self.raw {
            Some(raw) => raw,
            None => {
                static LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();
                let library = LIBRARY
                    .get_or_init(|| load().map_err(|error| format!("{error:#}")))
                    .as_ref()
                    .map_err(|error| anyhow!("{error}"))?;
                let raw = NonNull::new(unsafe { (library.new)((&raw const HOST).cast()) })
                    .context("Cannot create WebGPU state")?;
                *self.raw.insert((raw, library))
            }
        };
        let mut output = Output::default();
        let ok = unsafe {
            (library.call)(
                raw.as_ptr(),
                op.as_ptr(),
                op.len(),
                args.as_ptr(),
                args.len(),
                (&raw mut output).cast(),
                alloc,
            )
        };
        let bytes = output.finish();
        if !ok {
            bail!("{}", String::from_utf8_lossy(&bytes));
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn presented(&self) -> bool {
        self.raw
            .is_some_and(|(raw, library)| unsafe { (library.presented)(raw.as_ptr()) })
    }

    pub fn image(&mut self, node: usize) -> Result<Option<Arc<renderer::RenderImage>>> {
        let Some((raw, library)) = self.raw else {
            return Ok(None);
        };
        let (mut size, mut output) = ([0; 2], Output::default());
        let status = unsafe {
            (library.image)(
                raw.as_ptr(),
                node,
                &mut size,
                (&raw mut output).cast(),
                alloc,
            )
        };
        match status {
            0 => {
                self.images.remove(&node);
                Ok(None)
            }
            1 => Ok(self.images.get(&node).cloned()),
            2 => {
                let pixels = image::RgbaImage::from_raw(size[0], size[1], output.finish())
                    .context("Invalid canvas pixels")?;
                let image = Arc::new(renderer::RenderImage::new(
                    pixels.width(),
                    pixels.height(),
                    pixels.into_raw(),
                ));
                self.images.insert(node, image.clone());
                Ok(Some(image))
            }
            _ => bail!("{}", String::from_utf8_lossy(&output.finish())),
        }
    }

    /// Returns and resets the GPU bytes allocated since the last call.
    pub fn take_allocated(&mut self) -> u64 {
        self.raw.map_or(0, |(raw, library)| unsafe {
            (library.allocated)(raw.as_ptr())
        })
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        if let Some((raw, library)) = self.raw {
            unsafe { (library.free)(raw.as_ptr()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        dom::Document,
        host::{Host, Page, Role},
        jsc::{Api, Engine},
    };
    use std::{cell::RefCell, rc::Rc};

    fn page() -> (Rc<RefCell<Host>>, Engine) {
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse("<body></body>"),
                dirty: false,
            },
        );
        let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
        (host, engine)
    }

    fn stats(host: &Rc<RefCell<Host>>, key: &str) -> u64 {
        host.borrow_mut()
            .graphics
            .get_mut(&1)
            .unwrap()
            .call("stats", b"[]")
            .unwrap()[key]
            .as_u64()
            .unwrap()
    }

    #[test]
    #[ignore = "Requires a native GPU adapter"]
    fn sweep_destroys_unreachable_devices() {
        let (host, engine) = page();
        engine.eval("globalThis.created=0;(async()=>{const adapter=await navigator.gpu.requestAdapter();for(let i=0;i<100;i++){await adapter.requestDevice();created++}})()","test").unwrap();
        for _ in 0..30 {
            engine.collect_garbage();
            engine.tick().unwrap();
            if engine.eval("created", "test").unwrap() == "100" && stats(&host, "devices") == 0 {
                break;
            }
            engine
                .eval(
                    "(()=>{const data=Array.from({length:10000},()=>({}))})()",
                    "test",
                )
                .unwrap();
        }
        assert_eq!(engine.eval("created", "test").unwrap(), "100");
        assert_eq!(stats(&host, "devices"), 0);
    }

    #[test]
    #[ignore = "Requires a native GPU adapter"]
    fn reports_gpu_memory_so_unreachable_buffers_are_released() {
        let (host, engine) = page();
        engine.eval("(async()=>{globalThis.device=await (await navigator.gpu.requestAdapter()).requestDevice()})()", "test").unwrap();
        let mut peak = 0;
        for _ in 0..100 {
            engine
                .eval(
                    "for(let i=0;i<2;i++)device.createBuffer({size:16<<20,usage:GPUBufferUsage.COPY_DST})",
                    "test",
                )
                .unwrap();
            engine.tick().unwrap();
            peak = peak.max(stats(&host, "resources"));
        }
        assert!(peak < 100, "{peak} unreachable buffers stayed alive");
    }
}
