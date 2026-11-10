use super::{JavaScriptCore, Ptr};
use anyhow::{Context, Result, anyhow, ensure};
use libloading::Library;
use std::{
    collections::HashMap,
    ffi::{CStr, c_char},
    path::Path,
    ptr,
    sync::OnceLock,
};

#[derive(serde::Deserialize)]
struct Manifest {
    version: u32,
    runtime: String,
    build: String,
    scripts: HashMap<String, u32>,
}

pub(super) struct Image {
    _library: Library,
    build: String,
    modules: HashMap<String, u32>,
    attach: unsafe extern "C" fn(Ptr),
    function: unsafe extern "C" fn(Ptr, u32, *mut Ptr) -> Ptr,
}

// Register before the first context group: JSC reads the image options during registration.
pub(super) fn load(jsc: &Library, path: &Path) -> Result<Option<&'static Image>> {
    // Never unload this library: JSC retains the image in process-wide tables.
    static IMAGE: OnceLock<std::result::Result<Option<Image>, String>> = OnceLock::new();
    let image = IMAGE.get_or_init(|| read(jsc, path).map_err(|error| format!("{error:#}")));
    image
        .as_ref()
        .map(Option::as_ref)
        .map_err(|error| anyhow!("{error}"))
}

fn read(jsc: &Library, path: &Path) -> Result<Option<Image>> {
    if !path.is_file() {
        return Ok(None);
    }
    let library = unsafe { Library::new(path) }?;
    unsafe {
        let manifest = std::slice::from_raw_parts(
            *library.get::<*const u8>(b"apps_aot_manifest\0")?,
            **library.get::<*const usize>(b"apps_aot_manifest_size\0")?,
        );
        let manifest: Manifest =
            serde_json::from_slice(manifest).context("Invalid AOT manifest")?;
        ensure!(manifest.version == 1, "Unsupported AOT manifest version");
        ensure!(
            manifest.runtime == super::RUNTIME_SCRIPTS,
            "app.dylib was built for another runtime"
        );
        ensure!(
            manifest.scripts.values().all(|&module| module != 0),
            "Invalid AOT module ID"
        );
        let register: unsafe extern "C" fn(*const u8, usize, *mut c_char, usize) -> bool = *jsc
            .get(b"JSLoadAOT\0")
            .context("app.dylib needs the JavaScriptCore of its build")?;
        let mut error = [0; 1024];
        ensure!(
            register(
                *library.get::<*const u8>(b"apps_aot_image\0")?,
                **library.get::<*const usize>(b"apps_aot_image_size\0")?,
                error.as_mut_ptr(),
                error.len(),
            ),
            "{}",
            CStr::from_ptr(error.as_ptr()).to_string_lossy()
        );
        Ok(Some(Image {
            attach: *jsc.get(b"JSAttachAOT\0")?,
            function: *jsc.get(b"JSGetAOTFunction\0")?,
            _library: library,
            build: manifest.build,
            modules: manifest.scripts,
        }))
    }
}

impl Image {
    // Call once per context, after the host installed its globals and before compiled code runs.
    pub(super) fn attach(&self, ctx: Ptr) {
        unsafe { (self.attach)(ctx) }
    }

    pub(super) fn runtime(&self, url: &str) -> Option<u32> {
        self.modules.get(url).copied()
    }

    pub(super) fn find(&self, source: &[u8]) -> Result<Option<u32>> {
        let Some(reference) = source.strip_prefix(crate::bundle::AOT_PREFIX.as_bytes()) else {
            return Ok(None);
        };
        let reference = reference
            .split(|&byte| byte == b'"')
            .next()
            .unwrap_or_default();
        let (build, name) = std::str::from_utf8(reference)
            .ok()
            .and_then(|reference| reference.split_once(':'))
            .context("Invalid AOT reference")?;
        ensure!(build == self.build, "app.dylib does not match app.bundle");
        self.modules
            .get(name)
            .copied()
            .map(Some)
            .with_context(|| format!("Missing AOT code for {name}"))
    }

    pub(super) fn call(
        &self,
        api: &JavaScriptCore,
        ctx: Ptr,
        module: u32,
        this: Ptr,
        exception: *mut Ptr,
    ) -> Ptr {
        unsafe {
            let function = (self.function)(ctx, module, exception);
            if function.is_null() {
                return ptr::null_mut();
            }
            (api.invoke)(ctx, function, this, 0, ptr::null(), exception)
        }
    }
}
