mod aot;
pub(crate) mod encoding;
pub(crate) mod payload;

use crate::host::{Host, Role};
use anyhow::{Context, Result, bail, ensure};
use bbtree::unsync::BBTreeMap;
use libloading::Library;
use std::{
    cell::RefCell,
    ffi::{c_char, c_void},
    path::{Path, PathBuf},
    ptr,
    rc::Rc,
};

type Source = (&'static str, fn() -> &'static str);

const NODE_SCRIPTS: [Source; 14] = [
    ("apps:ffi", || js!("node/ffi.js")),
    ("apps:sqlite", || js!("node/sqlite.js")),
    ("apps:events", || js!("events.js")),
    ("apps:child_process", || js!("node/child_process.js")),
    ("apps:serve", || js!("web/serve.js")),
    ("apps:node", || js!("node/index.js")),
    ("apps:node-streams", || js!("node/stream.js")),
    ("apps:node-io", || js!("node/io.js")),
    ("apps:node-http", || js!("node/http.js")),
    ("apps:net", || js!("node/net.js")),
    ("apps:pty", || js!("node/pty.js")),
    ("apps:node-compat", || js!("node/compat.js")),
    ("apps:browser", || js!("browser.js")),
    ("apps:electron", || js!("electron.js")),
];
const WEB_SCRIPTS: [Source; 11] = [
    ("apps:streams", || js!("web/streams.js")),
    ("apps:crypto", || js!("web/crypto.js")),
    ("apps:web", || js!("web/index.js")),
    ("apps:platform", || js!("web/platform.js")),
    ("apps:bridge", || js!("bridge.js")),
    ("apps:worker", || js!("web/worker.js")),
    ("apps:browser-environment", || js!("web/environment.js")),
    ("apps:inspect", || js!("inspect.js")),
    ("apps:bootstrap", || js!("bootstrap.js")),
    ("apps:wasm", || js!("web/wasm.js")),
    ("apps:file-system", || js!("web/file_system.js")),
];
const PAGE_SCRIPTS: [Source; 2] = [
    ("apps:gpu", || js!("web/gpu.js")),
    ("apps:dom", || js!("dom.js")),
];

pub const RUNTIME_SCRIPTS: &str = env!("APPS_RUNTIME_SCRIPTS");

pub fn runtime_scripts() -> impl Iterator<Item = (&'static str, &'static str)> {
    NODE_SCRIPTS
        .into_iter()
        .chain(WEB_SCRIPTS)
        .chain(PAGE_SCRIPTS)
        .map(|(url, source)| (url, source()))
}

pub(crate) type Ptr = *mut c_void;
type Callback = unsafe extern "C" fn(Ptr, Ptr, Ptr, usize, *const Ptr, *mut Ptr) -> Ptr;

struct Script {
    url: &'static str,
    source: &'static str,
    bytecode: Option<&'static [u8]>,
}
struct Scripts {
    list: Vec<Script>,
    building: bool,
}
static SCRIPTS: std::sync::Mutex<Scripts> = std::sync::Mutex::new(Scripts {
    list: Vec::new(),
    building: false,
});

pub struct JavaScriptCore {
    aot: Option<&'static aot::Image>,
    _library: Library,
    group: Ptr,
    group_release: unsafe extern "C" fn(Ptr),
    garbage_collect: unsafe extern "C" fn(Ptr),
    shrink: unsafe extern "C" fn(Ptr),
    drain: unsafe extern "C" fn(Ptr),
    evaluate_latin1: unsafe extern "C" fn(
        Ptr,
        *const c_char,
        usize,
        Ptr,
        Ptr,
        *const u8,
        usize,
        *mut Ptr,
    ) -> Ptr,
    create_bytecode: unsafe extern "C" fn(Ptr, *const c_char, usize, Ptr, *mut usize) -> *mut u8,
    busy: std::cell::Cell<std::time::Duration>,
    busy_mark: std::cell::Cell<(std::time::Instant, std::time::Duration)>,
    burst: std::cell::Cell<bool>,
    quiet: std::cell::Cell<u32>,
    shrunk: std::cell::Cell<std::time::Instant>,
    pub(crate) idle: std::cell::Cell<bool>,
    inspectable: unsafe extern "C" fn(Ptr, bool),
    pub(crate) extra_memory: unsafe extern "C" fn(Ptr, usize),
    set_name: unsafe extern "C" fn(Ptr, Ptr),
    execution_limit: Limit,
    create: unsafe extern "C" fn(Ptr, Ptr) -> Ptr,
    release: unsafe extern "C" fn(Ptr),
    pub(crate) string_create: unsafe extern "C" fn(*const u16, usize) -> Ptr,
    string_create_utf8: unsafe extern "C" fn(*const c_char) -> Ptr,
    pub(crate) string_release: unsafe extern "C" fn(Ptr),
    pub(crate) evaluate: unsafe extern "C" fn(Ptr, Ptr, Ptr, Ptr, i32, *mut Ptr) -> Ptr,
    pub(crate) to_string: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> Ptr,
    pub(crate) make_string: unsafe extern "C" fn(Ptr, Ptr) -> Ptr,
    json_value: unsafe extern "C" fn(Ptr, Ptr) -> Ptr,
    pub(crate) typed_bytes: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> Ptr,
    pub(crate) typed_length: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> usize,
    pub(crate) typed_offset: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> usize,
    pub(crate) make_buffer: unsafe extern "C" fn(
        Ptr,
        Ptr,
        usize,
        Option<unsafe extern "C" fn(Ptr, Ptr)>,
        Ptr,
        *mut Ptr,
    ) -> Ptr,
    pub(crate) invoke: unsafe extern "C" fn(Ptr, Ptr, Ptr, usize, *const Ptr, *mut Ptr) -> Ptr,
    pub(crate) protect: unsafe extern "C" fn(Ptr, Ptr),
    pub(crate) unprotect: unsafe extern "C" fn(Ptr, Ptr),
    object_make: unsafe extern "C" fn(Ptr, Ptr, Ptr) -> Ptr,
    pub(crate) global: unsafe extern "C" fn(Ptr) -> Ptr,
    pub(crate) function: unsafe extern "C" fn(Ptr, Ptr, Callback) -> Ptr,
    pub(crate) get: unsafe extern "C" fn(Ptr, Ptr, Ptr, *mut Ptr) -> Ptr,
    rejection_callback: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr),
    module_loader: unsafe extern "C" fn(Ptr, Ptr),
    set: unsafe extern "C" fn(Ptr, Ptr, Ptr, Ptr, u32, *mut Ptr),
    pub(crate) undefined: unsafe extern "C" fn(Ptr) -> Ptr,
    pub(crate) null: unsafe extern "C" fn(Ptr) -> Ptr,
    pub(crate) boolean: unsafe extern "C" fn(Ptr, bool) -> Ptr,
    pub(crate) number: unsafe extern "C" fn(Ptr, f64) -> Ptr,
    pub(crate) is_function: unsafe extern "C" fn(Ptr, Ptr) -> bool,
    pub(crate) value_type: unsafe extern "C" fn(Ptr, Ptr) -> u32,
    pub(crate) to_number: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> f64,
    pub(crate) to_boolean: unsafe extern "C" fn(Ptr, Ptr) -> bool,
    pub(crate) string_length: unsafe extern "C" fn(Ptr) -> usize,
    pub(crate) string_chars: unsafe extern "C" fn(Ptr) -> *const u16,
    pub(crate) typed_kind: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> u32,
    pub(crate) typed_buffer: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> Ptr,
    pub(crate) buffer_bytes: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> Ptr,
    pub(crate) buffer_length: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> usize,
    get_index: unsafe extern "C" fn(Ptr, Ptr, u32, *mut Ptr) -> Ptr,
    set_index: unsafe extern "C" fn(Ptr, Ptr, u32, Ptr, *mut Ptr),
    make_error: unsafe extern "C" fn(Ptr, usize, *const Ptr, *mut Ptr) -> Ptr,
    make_array: unsafe extern "C" fn(Ptr, usize, *const Ptr, *mut Ptr) -> Ptr,
    typed_from_buffer: unsafe extern "C" fn(Ptr, u32, Ptr, *mut Ptr) -> Ptr,
    memory: unsafe extern "C" fn(Ptr) -> Ptr,
    proxy_target: unsafe extern "C" fn(Ptr) -> Ptr,
    json_string: unsafe extern "C" fn(Ptr, Ptr, u32, *mut Ptr) -> Ptr,
    get_chars: unsafe extern "C" fn(Ptr, Ptr, *mut usize, *mut bool) -> *const c_void,
    get_bytes: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr, *mut usize) -> bool,
    eval_enabled: unsafe extern "C" fn(Ptr, bool, Ptr),
    make_latin1: unsafe extern "C" fn(Ptr, *const u8, usize) -> Ptr,
    make_utf16: unsafe extern "C" fn(Ptr, *const u16, usize) -> Ptr,
    make_utf8: unsafe extern "C" fn(Ptr, *const c_char, usize) -> Ptr,
    make_uint8: unsafe extern "C" fn(Ptr, usize, *mut Ptr, *mut Ptr) -> Ptr,
    to_utf8: unsafe extern "C" fn(*const c_void, usize, bool, *mut c_char) -> usize,
    inspector: Option<Inspector>,
}

type Limit = unsafe extern "C" fn(Ptr, f64, Option<unsafe extern "C" fn(Ptr, Ptr) -> bool>, Ptr);
type InspectorMessage = unsafe extern "C" fn(Ptr, *const c_char, usize);
type InspectorPause = unsafe extern "C" fn(Ptr, *mut bool);

struct Inspector {
    connect: unsafe extern "C" fn(Ptr, InspectorMessage, InspectorPause, Ptr) -> Ptr,
    dispatch: unsafe extern "C" fn(Ptr, *const c_char, usize),
    disconnect: unsafe extern "C" fn(Ptr, Ptr),
}

pub fn library_name(platform: &str) -> Result<&'static str> {
    match platform {
        "macos" => Ok("libJavaScriptCore.dylib"),
        "linux" => Ok("libJavaScriptCore.so"),
        "windows" => Ok("JavaScriptCore.dll"),
        _ => bail!("Unsupported JavaScriptCore platform: {platform}"),
    }
}

pub fn library_path(executable: &Path, platform: &str) -> Result<PathBuf> {
    let executable = executable.canonicalize()?;
    let directory = executable
        .parent()
        .context("Missing executable directory")?;
    let name = library_name(platform)?;
    // A built app must never load a library named by the environment or the build machine.
    if [
        "app.bundle",
        "app.js",
        "../Resources/app.bundle",
        "../Resources/app.js",
    ]
    .iter()
    .any(|bundle| directory.join(bundle).is_file())
        && executable == std::env::current_exe()?.canonicalize()?
    {
        return ["../Frameworks", "."]
            .iter()
            .map(|folder| directory.join(folder).join(name))
            .find(|path| path.is_file())
            .context("The app's JavaScriptCore library is missing");
    }
    if let Some(path) = std::env::var_os("APPS_JSC_LIBRARY") {
        let path = PathBuf::from(path);
        anyhow::ensure!(
            path.is_file(),
            "APPS_JSC_LIBRARY needs a standalone library file: {}",
            path.display()
        );
        return Ok(path.canonicalize()?);
    }
    let mut candidates = Vec::with_capacity(3);
    if platform == "macos" {
        candidates.push(directory.join("../Frameworks").join(name));
    }
    candidates.push(directory.join(name));
    candidates.push(Path::new(env!("APPS_JSC_BUILD_DIR")).join(name));
    for path in &candidates {
        if path.is_file() {
            return Ok(path.canonicalize()?);
        }
    }
    bail!(
        "Bun JavaScriptCore is unavailable. Run bash scripts/build-jsc.sh, or set APPS_JSC_LIBRARY to its shared library. Searched: {}",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

impl JavaScriptCore {
    pub fn load() -> Result<Rc<Self>> {
        let path = library_path(&std::env::current_exe()?, std::env::consts::OS)?;
        let library = unsafe { Library::new(&path) }
            .with_context(|| format!("Cannot load Bun JavaScriptCore from {}", path.display()))?;
        let image = path.with_file_name("../Resources/app.dylib");
        unsafe {
            let group_create: unsafe extern "C" fn() -> Ptr =
                *library.get(b"JSContextGroupCreate\0")?;
            // Set JSC options before the first group and before other threads start. Only then is set_var
            // sound.
            static OPTIONS: std::sync::Once = std::sync::Once::new();
            let mut group = None;
            OPTIONS.call_once(|| {
                std::env::set_var("JSC_useSharedArrayBuffer", "1");
                // Limit heap growth to about 2x live size instead of 4x on machines with at least 16 GB.
                std::env::set_var("JSC_heapGrowthMaxIncrease", "1");
                // Sweep after each collection: JSC's timer never fires on the generic event loop, leaving
                // dead cells resident.
                std::env::set_var("JSC_sweepSynchronously", "1");
                // In JSC's cell cage every block the heap holds stays resident. AOT images use wide
                // references, which need no cage.
                std::env::set_var("JSC_markedBlockCageSizeInMB", "0");
                let _ = aot::load(&library, &image);
                group = Some(group_create());
                std::env::remove_var("JSC_useSharedArrayBuffer");
                std::env::remove_var("JSC_heapGrowthMaxIncrease");
                std::env::remove_var("JSC_sweepSynchronously");
                std::env::remove_var("JSC_markedBlockCageSizeInMB");
            });
            Ok(Rc::new(Self {
                aot: aot::load(&library, &image)?,
                group: group.unwrap_or_else(|| group_create()),
                inspectable: *library.get(b"JSGlobalContextSetInspectable\0")?,
                extra_memory: *library.get(b"JSReportExtraMemoryCost\0")?,
                set_name: *library.get(b"JSGlobalContextSetName\0")?,
                execution_limit: *library.get(b"JSContextGroupSetExecutionTimeLimit\0")?,
                group_release: *library.get(b"JSContextGroupRelease\0")?,
                garbage_collect: *library.get(b"JSSynchronousGarbageCollectForDebugging\0")?,
                shrink: *library.get(b"JSContextGroupShrinkFootprint\0")?,
                drain: *library.get(b"JSGlobalContextDrainMicrotasks\0")?,
                evaluate_latin1: *library.get(b"JSEvaluateScriptLatin1\0")?,
                create_bytecode: *library.get(b"JSScriptCreateBytecodeLatin1\0")?,
                busy: Default::default(),
                busy_mark: std::cell::Cell::new((std::time::Instant::now(), Default::default())),
                burst: Default::default(),
                quiet: Default::default(),
                shrunk: std::cell::Cell::new(std::time::Instant::now()),
                idle: Default::default(),
                create: *library.get(b"JSGlobalContextCreateInGroup\0")?,
                release: *library.get(b"JSGlobalContextRelease\0")?,
                string_create: *library.get(b"JSStringCreateWithCharacters\0")?,
                string_create_utf8: *library.get(b"JSStringCreateWithUTF8CString\0")?,
                string_release: *library.get(b"JSStringRelease\0")?,
                evaluate: *library.get(b"JSEvaluateScript\0")?,
                to_string: *library.get(b"JSValueToStringCopy\0")?,
                make_string: *library.get(b"JSValueMakeString\0")?,
                json_value: *library.get(b"JSValueMakeFromJSONString\0")?,
                typed_bytes: *library.get(b"JSObjectGetTypedArrayBytesPtr\0")?,
                typed_length: *library.get(b"JSObjectGetTypedArrayByteLength\0")?,
                typed_offset: *library.get(b"JSObjectGetTypedArrayByteOffset\0")?,
                make_buffer: *library.get(b"JSObjectMakeArrayBufferWithBytesNoCopy\0")?,
                invoke: *library.get(b"JSObjectCallAsFunction\0")?,
                protect: *library.get(b"JSValueProtect\0")?,
                unprotect: *library.get(b"JSValueUnprotect\0")?,
                object_make: *library.get(b"JSObjectMake\0")?,
                global: *library.get(b"JSContextGetGlobalObject\0")?,
                function: *library.get(b"JSObjectMakeFunctionWithCallback\0")?,
                get: *library.get(b"JSObjectGetProperty\0")?,
                rejection_callback: *library
                    .get(b"JSGlobalContextSetUnhandledRejectionCallback\0")?,
                module_loader: *library.get(b"JSGlobalContextSetModuleLoader\0")?,
                set: *library.get(b"JSObjectSetProperty\0")?,
                undefined: *library.get(b"JSValueMakeUndefined\0")?,
                null: *library.get(b"JSValueMakeNull\0")?,
                boolean: *library.get(b"JSValueMakeBoolean\0")?,
                number: *library.get(b"JSValueMakeNumber\0")?,
                is_function: *library.get(b"JSObjectIsFunction\0")?,
                value_type: *library.get(b"JSValueGetType\0")?,
                to_number: *library.get(b"JSValueToNumber\0")?,
                to_boolean: *library.get(b"JSValueToBoolean\0")?,
                string_length: *library.get(b"JSStringGetLength\0")?,
                string_chars: *library.get(b"JSStringGetCharactersPtr\0")?,
                typed_kind: *library.get(b"JSValueGetTypedArrayType\0")?,
                typed_buffer: *library.get(b"JSObjectGetTypedArrayBuffer\0")?,
                buffer_bytes: *library.get(b"JSObjectGetArrayBufferBytesPtr\0")?,
                buffer_length: *library.get(b"JSObjectGetArrayBufferByteLength\0")?,
                get_index: *library.get(b"JSObjectGetPropertyAtIndex\0")?,
                set_index: *library.get(b"JSObjectSetPropertyAtIndex\0")?,
                make_error: *library.get(b"JSObjectMakeError\0")?,
                make_array: *library.get(b"JSObjectMakeArray\0")?,
                typed_from_buffer: *library.get(b"JSObjectMakeTypedArrayWithArrayBuffer\0")?,
                memory: *library.get(b"JSGetMemoryUsageStatistics\0")?,
                proxy_target: *library.get(b"JSObjectGetProxyTarget\0")?,
                json_string: *library.get(b"JSValueCreateJSONString\0")?,
                get_chars: *library.get(b"JSValueGetStringCharacters\0")?,
                get_bytes: *library.get(b"JSValueGetBytes\0")?,
                eval_enabled: *library.get(b"JSGlobalContextSetEvalEnabled\0")?,
                make_latin1: *library.get(b"JSValueMakeStringLatin1\0")?,
                make_utf16: *library.get(b"JSValueMakeStringUTF16\0")?,
                make_utf8: *library.get(b"JSValueMakeStringUTF8\0")?,
                make_uint8: *library.get(b"JSObjectMakeUint8ArrayUninitialized\0")?,
                to_utf8: *library.get(b"JSStringConvertCharactersToUTF8\0")?,
                inspector: (|| {
                    Some(Inspector {
                        connect: *library.get(b"JSGlobalContextConnectInspector\0").ok()?,
                        dispatch: *library
                            .get(b"JSGlobalContextDispatchInspectorMessage\0")
                            .ok()?,
                        disconnect: *library.get(b"JSGlobalContextDisconnectInspector\0").ok()?,
                    })
                })(),
                _library: library,
            }))
        }
    }

    pub(crate) fn string(&self, text: &str) -> Ptr {
        let mut buffer = [0u8; 128];
        if text.len() < buffer.len() && !text.as_bytes().contains(&0) {
            buffer[..text.len()].copy_from_slice(text.as_bytes());
            return unsafe { (self.string_create_utf8)(buffer.as_ptr().cast()) };
        }
        self.string_owned(text.to_owned())
    }

    pub(crate) fn string_owned(&self, text: String) -> Ptr {
        match std::ffi::CString::new(text) {
            Ok(text) => unsafe { (self.string_create_utf8)(text.as_ptr()) },
            Err(error) => {
                let utf16: Vec<_> = String::from_utf8_lossy(&error.into_vec())
                    .encode_utf16()
                    .collect();
                unsafe { (self.string_create)(utf16.as_ptr(), utf16.len()) }
            }
        }
    }

    pub(crate) fn text(&self, ctx: Ptr, value: Ptr) -> String {
        if let Some(chars) = self.chars(ctx, value) {
            return self.utf8_string(chars);
        }
        unsafe {
            let string = (self.to_string)(ctx, value, ptr::null_mut());
            if string.is_null() {
                return "JavaScript exception".into();
            }
            let value = (self.make_string)(ctx, string);
            (self.string_release)(string);
            self.chars(ctx, value)
                .map_or_else(String::new, |chars| self.utf8_string(chars))
        }
    }

    // WARNING: JSC keeps pointing at `source` and `bytecode`, so both must outlive the VM.
    unsafe fn evaluate_latin1(
        &self,
        ctx: Ptr,
        source: &[u8],
        this: Ptr,
        url: Ptr,
        bytecode: &[u8],
        exception: *mut Ptr,
    ) -> Ptr {
        let bytecode_ptr = if bytecode.is_empty() {
            ptr::null()
        } else {
            bytecode.as_ptr()
        };
        unsafe {
            (self.evaluate_latin1)(
                ctx,
                source.as_ptr().cast(),
                source.len(),
                this,
                url,
                bytecode_ptr,
                bytecode.len(),
                exception,
            )
        }
    }

    pub(crate) fn shrink_pending(&self) -> bool {
        self.busy.get() >= std::time::Duration::from_millis(20)
    }

    // JSC's GC timers never fire here. Ignore idle polls and timers when scheduling costly collections that
    // discard compiled code.
    pub(crate) fn shrink_due(&self) -> bool {
        use std::time::Duration;
        let (now, busy) = (std::time::Instant::now(), self.busy.get());
        let (at, before) = self.busy_mark.get();
        if !self.shrink_pending() || now.duration_since(at) < Duration::from_millis(900) {
            return false;
        }
        self.busy_mark.set((now, busy));
        let ran = busy.saturating_sub(before);
        self.burst
            .set(self.burst.get() || ran >= Duration::from_millis(100));
        if ran >= Duration::from_millis(20) {
            self.quiet.set(0);
            return ran < Duration::from_millis(100)
                && now.duration_since(self.shrunk.get()) >= Duration::from_secs(300);
        }
        self.quiet.set(self.quiet.get() + 1);
        self.burst.get() && self.quiet.get() >= 5
            || now.duration_since(self.shrunk.get()) >= Duration::from_secs(300)
    }

    pub(crate) fn shrink(&self) {
        self.busy.set(Default::default());
        self.burst.set(false);
        self.quiet.set(0);
        self.shrunk.set(std::time::Instant::now());
        unsafe { (self.shrink)(self.group) };
    }
}

impl Drop for JavaScriptCore {
    fn drop(&mut self) {
        unsafe { (self.group_release)(self.group) };
    }
}

// The bytecode is never freed, because decoded code points into it.
fn create_bytecode(api: &JavaScriptCore, ctx: Ptr, url: &str, source: &str) -> &'static [u8] {
    let name = api.string(url);
    let mut size = 0;
    let data = unsafe {
        (api.create_bytecode)(ctx, source.as_ptr().cast(), source.len(), name, &mut size)
    };
    unsafe { (api.string_release)(name) };
    if data.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, size) }
    }
}

pub(crate) fn prebuild(api: &JavaScriptCore, webgpu: bool, node: bool) {
    if api.aot.is_some() {
        return;
    }
    let mut scripts = SCRIPTS.lock().unwrap();
    let count = scripts.list.len();
    let node = NODE_SCRIPTS.into_iter().filter(|_| node);
    for (url, source) in WEB_SCRIPTS.into_iter().chain(PAGE_SCRIPTS).chain(node) {
        if (webgpu || url != "apps:gpu") && !scripts.list.iter().any(|script| script.url == url) {
            scripts.list.push(Script {
                url,
                source: source(),
                bytecode: None,
            });
        }
    }
    if scripts.building || scripts.list.len() == count {
        return;
    }
    scripts.building = true;
    drop(scripts);
    let _ = std::thread::Builder::new()
        .name("apps-bytecode".into())
        .spawn(build_bytecode);
}

fn build_bytecode() -> Result<()> {
    let api = JavaScriptCore::load()?;
    let ctx = unsafe { (api.create)(api.group, ptr::null_mut()) };
    loop {
        let next = {
            let mut scripts = SCRIPTS.lock().unwrap();
            let next = scripts.list.iter().find(|script| script.bytecode.is_none());
            let next = next.map(|script| (script.url, script.source));
            scripts.building = next.is_some();
            next
        };
        let Some((url, source)) = next else { break };
        let bytecode = create_bytecode(&api, ctx, url, source);
        let mut scripts = SCRIPTS.lock().unwrap();
        if let Some(script) = scripts.list.iter_mut().find(|script| script.url == url) {
            script.bytecode = Some(bytecode);
        }
    }
    unsafe { (api.release)(ctx) };
    Ok(())
}

pub fn bytecode<'a>(
    disabled: u32,
    app: &[(&'a str, &'a str)],
) -> Result<Vec<(&'a str, &'static [u8])>> {
    SCRIPTS.lock().unwrap().building = true;
    let api = JavaScriptCore::load()?;
    prebuild(
        &api,
        disabled & crate::bundle::feature_bit("webgpu") == 0,
        true,
    );
    build_bytecode()?;
    let mut bytecode: Vec<(&str, &[u8])> = SCRIPTS
        .lock()
        .unwrap()
        .list
        .iter()
        .filter_map(|script| Some((script.url, script.bytecode?)))
        .collect();
    let ctx = unsafe { (api.create)(api.group, ptr::null_mut()) };
    for &(name, source) in app.iter().filter(|(_, source)| source.is_ascii()) {
        bytecode.push((name, create_bytecode(&api, ctx, name, source)));
    }
    unsafe { (api.release)(ctx) };
    bytecode.retain(|(_, data)| !data.is_empty());
    Ok(bytecode)
}

pub(crate) fn add_bytecode(url: &'static str, source: &'static str, bytecode: &'static [u8]) {
    SCRIPTS.lock().unwrap().list.push(Script {
        url,
        source,
        bytecode: Some(bytecode),
    });
}

fn bundled(source: &[u8]) -> Option<&'static [u8]> {
    let scripts = SCRIPTS.lock().unwrap();
    let script = scripts
        .list
        .iter()
        .find(|script| !source.is_empty() && ptr::eq(script.source.as_bytes(), source))?;
    script.bytecode.filter(|bytecode| !bytecode.is_empty())
}

static MAPPED: std::sync::Mutex<Vec<std::sync::Weak<memmap2::Mmap>>> =
    std::sync::Mutex::new(Vec::new());

pub(crate) fn add_mapped(bundle: std::sync::Weak<memmap2::Mmap>) {
    let mut mapped = MAPPED.lock().unwrap();
    mapped.retain(|bundle| bundle.strong_count() > 0);
    mapped.push(bundle);
}

// JSC reads an ASCII script of a mapped bundle in place, so that mapping stays for the rest of the process.
fn in_place(source: &[u8]) -> bool {
    let range = source.as_ptr_range();
    let bundle = MAPPED
        .lock()
        .unwrap()
        .iter()
        .filter_map(std::sync::Weak::upgrade)
        .find(|bundle| {
            let bundle = bundle.as_ptr_range();
            bundle.start <= range.start && range.end <= bundle.end
        });
    match bundle {
        Some(bundle) if source.is_ascii() => {
            std::mem::forget(bundle);
            true
        }
        _ => false,
    }
}

struct Binding {
    api: Rc<JavaScriptCore>,
    host: Rc<RefCell<Host>>,
    role: Role,
    sqlite: RefCell<crate::node::sqlite::State>,
    web: RefCell<crate::web::State>,
    ffi: crate::node::ffi::State,
    node: RefCell<crate::node::State>,
    inspector: std::cell::Cell<Ptr>,
}
thread_local! {
    static HOSTS: RefCell<rustc_hash::FxHashMap<usize, Rc<Binding>>> = RefCell::default();
    static ARMED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static INSPECTOR_MESSAGES: RefCell<Vec<(usize, String)>> = const { RefCell::new(Vec::new()) };
    // The render loop sets this, so engines without windows, like the CLI's, don't link the renderer.
    pub(crate) static REFLOW: std::cell::Cell<Option<fn(u64)>> = const { std::cell::Cell::new(None) };
    pub(crate) static CLIPBOARD: std::cell::Cell<Option<Clipboard>> = const { std::cell::Cell::new(None) };
    // The render thread sets this, because only the main thread can make native calls.
    pub(crate) static NATIVE: std::cell::Cell<Option<NativeCall>> = const { std::cell::Cell::new(None) };
    // The render thread must stop before statics go, so the main thread sets this.
    pub(crate) static BEFORE_EXIT: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) };
    static RUN_TICKS: std::cell::Cell<(std::time::Duration, u32)> = const { std::cell::Cell::new((std::time::Duration::ZERO, 0)) };
}

type Clipboard = fn(Option<String>) -> Option<String>;
type NativeCall = fn(&str, &serde_json::Value) -> Result<serde_json::Value>;

fn bridge(
    api: &JavaScriptCore,
    context: Ptr,
    function: &str,
    payload: payload::Payload,
) -> Result<Ptr> {
    let name = api.string(function);
    let mut error = ptr::null_mut();
    let receiver = unsafe { (api.get)(context, (api.global)(context), name, &mut error) };
    unsafe {
        (api.string_release)(name);
    }
    // JSC casts call targets to objects without checking, so a primitive would crash.
    anyhow::ensure!(
        error.is_null()
            && unsafe { (api.value_type)(context, receiver) } == 5
            && unsafe { (api.is_function)(context, receiver) },
        "Bridge target is unavailable"
    );
    let argument = payload::output(api, context, payload);
    unsafe {
        (api.protect)(context, argument);
    }
    let result =
        unsafe { (api.invoke)(context, receiver, ptr::null_mut(), 1, &argument, &mut error) };
    unsafe {
        (api.unprotect)(context, argument);
    }
    if !error.is_null() {
        bail!("{}", api.text(context, error));
    }
    Ok(result)
}

fn receive(
    api: &JavaScriptCore,
    context: Ptr,
    payload: payload::Payload,
) -> Result<payload::Payload> {
    let result = bridge(api, context, "__appsBridgeReceiveBytes", payload)?;
    anyhow::ensure!(
        !result.is_null() && unsafe { (api.value_type)(context, result) } == 5,
        "Invalid bridge reply"
    );
    unsafe {
        (api.protect)(context, result);
    }
    let mut error = ptr::null_mut();
    let decoded = (|| -> Result<_> {
        let name = api.string("json");
        let json = unsafe { (api.get)(context, result, name, &mut error) };
        unsafe {
            (api.string_release)(name);
        }
        let mut json = api.text(context, json);
        well_formed(&mut json);
        let value = serde_json::from_str(&json)?;
        let name = api.string("buffers");
        let buffers = unsafe { (api.get)(context, result, name, &mut error) };
        unsafe {
            (api.string_release)(name);
        }
        let buffers = payload::input(api, context, buffers)?
            .0
            .iter()
            .map(|bytes| bytes::Bytes::copy_from_slice(bytes))
            .collect();
        Ok(payload::Payload { value, buffers })
    })();
    unsafe {
        (api.unprotect)(context, result);
    }
    decoded
}

pub(crate) fn contexts() -> Vec<(usize, Role)> {
    HOSTS.with_borrow(|hosts| {
        hosts
            .iter()
            .map(|(&ctx, binding)| (ctx, binding.role))
            .collect()
    })
}

pub(crate) fn connect_inspector(ctx: usize) -> Result<bool> {
    let binding = HOSTS
        .with_borrow(|hosts| hosts.get(&ctx).cloned())
        .context("The JavaScript context is closed")?;
    if !binding.inspector.get().is_null() {
        return Ok(false);
    }
    let inspector =
        binding.api.inspector.as_ref().context(
            "This JavaScriptCore has no inspector. Rebuild it with scripts/build-jsc.sh",
        )?;
    let connection =
        unsafe { (inspector.connect)(ctx as Ptr, inspector_message, inspector_pause, ctx as Ptr) };
    binding.inspector.set(connection);
    Ok(true)
}

pub(crate) fn inspect(ctx: usize, message: &str) -> Result<()> {
    connect_inspector(ctx)?;
    let api = HOSTS
        .with_borrow(|hosts| hosts.get(&ctx).map(|binding| binding.api.clone()))
        .context("The JavaScript context is closed")?;
    let inspector = api.inspector.as_ref().context("Missing inspector")?;
    unsafe { (inspector.dispatch)(ctx as Ptr, message.as_ptr().cast(), message.len()) };
    Ok(())
}

pub(crate) fn inspector_messages() -> Vec<(usize, String)> {
    INSPECTOR_MESSAGES.take()
}

unsafe extern "C" fn inspector_message(ctx: Ptr, message: *const c_char, length: usize) {
    let message = unsafe { std::slice::from_raw_parts(message.cast::<u8>(), length) };
    let message = String::from_utf8_lossy(message).into_owned();
    INSPECTOR_MESSAGES.with_borrow_mut(|messages| messages.push((ctx as usize, message)));
}

unsafe extern "C" fn inspector_pause(ctx: Ptr, done: *mut bool) {
    crate::desktop::mcp::paused(ctx as usize, done);
}

unsafe extern "C" fn compile(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let binding = HOSTS.with(|hosts| {
        hosts
            .borrow()
            .get(&(ctx as usize))
            .map(|binding| (binding.api.clone(), binding.host.clone(), binding.role))
    });
    let Some((api, host, role)) = binding else {
        return ptr::null_mut();
    };
    // Without jsExecute, pages compile only while Rust evaluates one of their own scripts, never from later strings.
    if matches!(role, Role::Renderer(_))
        && ARMED.get() != ctx as usize
        && !host
            .try_borrow()
            .is_ok_and(|host| host.enabled("jsExecute"))
    {
        throw(
            &api,
            ctx,
            exception,
            "EvalError: Code generation is disabled in pages",
        );
        return ptr::null_mut();
    }
    if count != 2 {
        return unsafe { (api.undefined)(ctx) };
    }
    let source = unsafe { (api.to_string)(ctx, *args, exception) };
    if source.is_null() {
        return ptr::null_mut();
    }
    let url = unsafe { (api.to_string)(ctx, *args.add(1), exception) };
    if url.is_null() {
        unsafe {
            (api.string_release)(source);
        }
        return ptr::null_mut();
    }
    let result = unsafe { (api.evaluate)(ctx, source, ptr::null_mut(), url, 1, exception) };
    unsafe {
        (api.string_release)(source);
        (api.string_release)(url);
    }
    result
}

fn throw(api: &JavaScriptCore, ctx: Ptr, exception: *mut Ptr, message: &str) {
    if !exception.is_null() {
        let message = api.string(message);
        unsafe {
            *exception = (api.make_string)(ctx, message);
            (api.string_release)(message);
        }
    }
}

// This runs only scripts of the app bundle, by name, and never arms code generation.
unsafe extern "C" fn chunk(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let binding = HOSTS.with(|hosts| {
        hosts
            .borrow()
            .get(&(ctx as usize))
            .map(|binding| (binding.api.clone(), binding.host.clone()))
    });
    let Some((api, host)) = binding else {
        return ptr::null_mut();
    };
    let name = if count == 1 {
        api.text(ctx, unsafe { *args })
    } else {
        String::new()
    };
    let source = host
        .try_borrow()
        .ok()
        .and_then(|host| host.scripts.get(&name).cloned());
    let Some(source) =
        source.filter(|source| source.is_ascii() || std::str::from_utf8(source).is_ok())
    else {
        throw(
            &api,
            ctx,
            exception,
            &format!("Error: Missing bundled script: {name}"),
        );
        return ptr::null_mut();
    };
    if let Some(aot) = api.aot {
        match aot.find(&source) {
            Ok(Some(module)) => return aot.call(&api, ctx, module, ptr::null_mut(), exception),
            Ok(None) => {}
            Err(error) => {
                throw(&api, ctx, exception, &format!("Error: {error:#}"));
                return ptr::null_mut();
            }
        }
    }
    let url = api.string(&format!("apps:///{name}"));
    // Keep these bytes alive for JSC. JSEvaluateScript copies source and uses UTF-16 for NUL or non-ASCII
    // input.
    let result = if source.is_ascii() {
        let bytecode = bundled(&source).unwrap_or_default();
        let source = std::mem::ManuallyDrop::new(source);
        unsafe { api.evaluate_latin1(ctx, &source[..], ptr::null_mut(), url, bytecode, exception) }
    } else {
        let script = api.string(&String::from_utf8_lossy(&source));
        let result = unsafe { (api.evaluate)(ctx, script, ptr::null_mut(), url, 1, exception) };
        unsafe { (api.string_release)(script) };
        result
    };
    unsafe { (api.string_release)(url) };
    result
}

// Replace lone-surrogate escapes with \ufffd: serde_json rejects them.
fn well_formed(json: &mut str) {
    let mut from = 0;
    while let Some(at) = json.get(from..).and_then(|rest| rest.find('\\')) {
        let at = from + at;
        // SAFETY: ASCII bytes replace ASCII bytes, so the text stays UTF-8.
        if let Some(escape) = unsafe { json.as_bytes_mut() }.get_mut(at + 1..at + 6)
            && escape[..2] == *b"ud"
            && matches!(escape[2], b'8'..=b'9' | b'a'..=b'f')
        {
            escape[1..].copy_from_slice(b"fffd");
        }
        from = at + 2;
    }
}

const MAIN: u8 = 1;
const PRELOAD: u8 = 2;
const PAGE: u8 = 4;
const NODE: u8 = MAIN | PRELOAD;
const ALL: u8 = NODE | PAGE;

// Deny unlisted host operations. The first match wins. A trailing `.` matches prefixes. Keep module-level
// checks too.
const ACCESS: &[(&str, u8)] = &[
    ("log", ALL),
    ("bridge.dispatch", ALL),
    ("bridge.expose", PRELOAD),
    ("bridge.preload", PRELOAD),
    ("bridge.file", PRELOAD),
    ("bridge.exposures", PAGE),
    ("dom.", PAGE),
    ("gpu.", PAGE),
    ("media.", PAGE),
    ("web.serve.", MAIN),
    ("web.tcp.", NODE),
    ("web.", ALL),
    ("native.clipboard.", NODE),
    ("native.shell.", NODE),
    ("native.image", NODE),
    ("native.", MAIN),
    ("browser.", MAIN),
    ("node.", NODE),
    ("pty.", NODE),
    ("child.", NODE),
    ("sqlite.", NODE),
    ("process.", NODE),
    ("info", NODE),
    ("cwd", NODE),
    ("chdir", NODE),
    ("app.", MAIN),
    ("window", MAIN),
    ("quit", MAIN),
    ("focus", MAIN),
];

fn access(role: Role, op: &str) -> Result<()> {
    let roles = ACCESS
        .iter()
        .find(|(name, _)| {
            if name.ends_with('.') {
                op.starts_with(name)
            } else {
                op == *name
            }
        })
        .map_or(0, |&(_, roles)| roles);
    match role {
        Role::Main if roles & MAIN != 0 => Ok(()),
        Role::Preload(_) if roles & PRELOAD != 0 => Ok(()),
        Role::Renderer(_) if roles & PAGE != 0 => Ok(()),
        Role::Renderer(_) => bail!("{op} is unavailable in page scripts"),
        _ if roles == MAIN => bail!("{op} is only available in the main context"),
        _ => bail!("Unsupported host operation: {op}"),
    }
}

unsafe extern "C" fn call(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    use serde_json::Value;
    let binding = HOSTS.with(|hosts| hosts.borrow().get(&(ctx as usize)).cloned());
    let Some(binding) = binding else {
        return ptr::null_mut();
    };
    let api = binding.api.clone();
    let values = if count == 0 || args.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(args, count) }
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Ptr> {
        let generic = values
            .first()
            .is_some_and(|&value| unsafe { (api.value_type)(ctx, value) } == 3);
        let mut name = [0u8; 64];
        let op = {
            let value = *values
                .get(usize::from(generic))
                .context("Missing host operation")?;
            let size = match api.chars(ctx, value) {
                Some(encoding::Chars::Latin1(op)) if op.len() <= name.len() => {
                    name[..op.len()].copy_from_slice(op);
                    op.len()
                }
                _ => 0,
            };
            std::str::from_utf8(&name[..size])
                .ok()
                .filter(|op| !op.is_empty())
                .context("Invalid host operation")?
        };
        access(binding.role, op)?;
        let run = |buffers, action: &mut dyn FnMut() -> Result<Value>| -> Result<Ptr> {
            let mut failure = None;
            let payload = payload::Payload::capture(buffers, || {
                action().unwrap_or_else(|error| {
                    failure = Some(error);
                    Value::Null
                })
            });
            match failure {
                Some(error) => Err(error),
                None => Ok(output(&api, ctx, payload)),
            }
        };
        if generic {
            let (buffers, _roots) = values
                .get(3)
                .map(|&buffers| payload::input(&api, ctx, buffers))
                .transpose()?
                .unzip();
            let buffers = buffers.unwrap_or_default();
            let mut json = api.text(ctx, *values.get(2).context("Expected JSON arguments")?);
            well_formed(&mut json);
            if let (Role::Renderer(id), Some(op)) = (binding.role, op.strip_prefix("gpu.")) {
                return run(buffers, &mut || gpu(&binding, ctx, id, op, json.as_bytes()));
            }
            let mut args = Some(serde_json::from_str(&json)?);
            let result = run(buffers, &mut || {
                dispatch(&binding, ctx, op, args.take().unwrap_or_default())
            });
            if let Role::Renderer(id) = binding.role {
                report_nodes(&binding, ctx, id);
            }
            return result;
        }
        let role = binding.role;
        // A forced layout builds the page, which borrows the host, so it runs before any borrow.
        if let Role::Renderer(id) = role
            && op == "dom.reflow"
        {
            if let Some(reflow) = REFLOW.get() {
                reflow(id);
            }
            return Ok(unsafe { (api.undefined)(ctx) });
        }
        if let Role::Renderer(id) = role
            && op == "dom.batch"
        {
            ensure!(values.len() == 4, "Expected DOM operations");
            let text = api.text(ctx, values[1]);
            let size = unsafe { (api.to_number)(ctx, values[3], ptr::null_mut()) } * 4.;
            let codes = payload::view(&api, ctx, values[2])?;
            let codes = (size >= 0. && size.fract() == 0.)
                .then(|| codes.get(..size as usize))
                .flatten()
                .context("Expected DOM operations")?;
            let errors = binding
                .host
                .try_borrow_mut()
                .context("Host is busy")?
                .batch(id, codes, &text)?;
            report_nodes(&binding, ctx, id);
            return Ok(output(&api, ctx, errors.into()));
        }
        if let Role::Renderer(id) = role
            && op == "dom.layout"
        {
            let mut host = binding.host.try_borrow_mut().context("Host is busy")?;
            let host = &mut *host;
            let page = host.pages.get_mut(&id).context("Window is closed")?;
            let state = host.dom.entry(id).or_default();
            let viewport = *host.viewports.get(&id).unwrap_or(&[800., 600.]);
            if values
                .get(2)
                .is_some_and(|&value| unsafe { (api.to_boolean)(ctx, value) })
            {
                crate::dom::layout_flags(&mut page.doc, state, viewport);
            }
            state.sync_scroll();
            if !values
                .get(1)
                .is_some_and(|&value| unsafe { (api.to_boolean)(ctx, value) })
            {
                return Ok(unsafe { (api.null)(ctx) });
            }
            let (pointer, size, owner) = state.bounds.share();
            return Ok(unsafe {
                (api.make_buffer)(
                    ctx,
                    pointer,
                    size,
                    Some(crate::dom::LayoutMirror::release),
                    owner,
                    ptr::null_mut(),
                )
            });
        }
        let mut buffers = Vec::new();
        let mut args = Vec::with_capacity(values.len() - 1);
        for &value in &values[1..] {
            args.push(match unsafe { (api.value_type)(ctx, value) } {
                2 => Value::Bool(unsafe { (api.to_boolean)(ctx, value) }),
                3 => {
                    let number = unsafe { (api.to_number)(ctx, value, ptr::null_mut()) };
                    // This matches `JSON.stringify` and serde, so ops see the same integers and non-finite numbers become null.
                    if number.fract() == 0. && number.abs() < 1e21 {
                        if (0. ..18446744073709551616.).contains(&number) {
                            Value::from(number as u64)
                        } else if number >= -9223372036854775808. {
                            Value::from(number as i64)
                        } else {
                            Value::from(number)
                        }
                    } else {
                        Value::from(number)
                    }
                }
                4 => Value::String(api.text(ctx, value)),
                5 if unsafe { (api.is_function)(ctx, value) } => Value::Null,
                5 => {
                    // These bytes are not static: they borrow the arguments, which live only until the call
                    // returns.
                    buffers.push(bytes::Bytes::from_static(payload::view(&api, ctx, value)?));
                    serde_json::json!({"$appsBytes": buffers.len() - 1})
                }
                7 => bail!("Do not know how to serialize a BigInt"),
                _ => Value::Null,
            });
        }
        let mut args = Some(Value::Array(args));
        let result = run(buffers, &mut || {
            dispatch(&binding, ctx, op, args.take().unwrap_or_default())
        });
        if let Role::Renderer(id) = role {
            report_nodes(&binding, ctx, id);
        }
        result
    }));
    let (message, code) = match result {
        Ok(Ok(value)) => return value,
        Ok(Err(error)) => (
            error.to_string(),
            error
                .downcast_ref::<std::io::Error>()
                .map(crate::node::code),
        ),
        Err(_) => ("Host call failed".into(), None),
    };
    unsafe {
        let message = api
            .utf8(ctx, message.as_bytes())
            .unwrap_or_else(|_| (api.undefined)(ctx));
        let error = (api.make_error)(ctx, 1, &message, ptr::null_mut());
        if let Some(code) = code {
            let key = api.string("code");
            let text = api.string(code);
            (api.set)(
                ctx,
                error,
                key,
                (api.make_string)(ctx, text),
                0,
                ptr::null_mut(),
            );
            (api.string_release)(key);
            (api.string_release)(text);
        }
        if !exception.is_null() {
            *exception = error;
        }
        (api.undefined)(ctx)
    }
}

fn dispatch(
    binding: &Binding,
    ctx: Ptr,
    op: &str,
    mut args: serde_json::Value,
) -> Result<serde_json::Value> {
    let Binding {
        host,
        role,
        sqlite,
        web,
        node,
        ..
    } = binding;
    let role = *role;
    let values = &args;
    if let Some(op) = op.strip_prefix("browser.") {
        anyhow::ensure!(
            matches!(role, Role::Main),
            "Browser views can only be managed by main"
        );
        let mut host = host.try_borrow_mut().context("Host is busy")?;
        if host.packaged {
            if op == "command" && values[1] == "inspect" {
                return Ok(serde_json::Value::Null);
            }
            if op == "create" {
                let mut values = values.clone();
                values[0]["inspectable"] = false.into();
                let host = &mut *host;
                return host.browsers.call(
                    op,
                    &values,
                    host.app_name.as_deref().unwrap_or_default(),
                );
            }
        }
        let host = &mut *host;
        return host
            .browsers
            .call(op, values, host.app_name.as_deref().unwrap_or_default());
    }
    if let Some((service @ ("node" | "pty" | "child"), op)) = op.split_once('.') {
        anyhow::ensure!(
            !matches!(role, Role::Renderer(_)),
            "Node APIs are unavailable in page scripts"
        );
        let mut node = node.try_borrow_mut().context("Node is busy")?;
        return match service {
            "pty" => node.pty.call(op, values),
            "child" => node.children.call(op, values),
            _ => node.call(op, &mut args),
        };
    }
    if let Some(op) = op.strip_prefix("native.") {
        anyhow::ensure!(
            matches!(role, Role::Main)
                || matches!(role, Role::Preload(_))
                    && (op.starts_with("clipboard.") || op.starts_with("shell.") || op == "image"),
            "Native desktop APIs require the main process"
        );
        let result = match NATIVE.get() {
            Some(native) => native(op, &values[0]),
            None => crate::desktop::native::native_call(op, &values[0]),
        }?;
        if op == "theme.set" {
            let mut host = host.borrow_mut();
            host.sync_theme();
            host.command(
                crate::desktop::link::Target::All,
                serde_json::json!({"op":"theme"}),
            );
        }
        return Ok(result);
    }
    if matches!(op, "bridge.expose" | "bridge.preload") {
        let Role::Preload(id) = role else {
            bail!("Only preload can expose APIs")
        };
        let (name, value) = if op == "bridge.preload" {
            ("__appsPreload", &values[0])
        } else {
            (
                values[0].as_str().context("Expected exposed name")?,
                &values[1],
            )
        };
        anyhow::ensure!(
            !matches!(name, "__proto__" | "constructor" | "prototype")
                && (!name.starts_with("__apps") || op == "bridge.preload"),
            "Reserved bridge name"
        );
        host.try_borrow_mut()
            .context("Host is busy")?
            .bridge
            .entry(id)
            .or_default()
            .insert(name.to_owned(), payload::Payload::snapshot(value.clone()));
        return Ok(serde_json::Value::Null);
    }
    if op == "bridge.file" {
        let path = match (role, values[0].as_u64()) {
            (Role::Preload(id), Some(index)) => host
                .borrow()
                .dropped
                .get(&id)
                .and_then(|paths| paths.get(index as usize).cloned()),
            _ => None,
        };
        return Ok(path.unwrap_or_default().into());
    }
    if op == "bridge.exposures" {
        let Role::Renderer(id) = role else {
            bail!("Only pages can read exposed APIs")
        };
        return Ok(serde_json::Value::Object(
            host.borrow()
                .bridge
                .get(&id)
                .into_iter()
                .flat_map(|values| values.iter())
                .map(|(name, value)| (name.clone(), value.clone().into_value()))
                .collect(),
        ));
    }
    if op == "bridge.dispatch" {
        let target = &values[0];
        if let Role::Renderer(id) = role {
            anyhow::ensure!(
                target["preload"] == id
                    && matches!(values[1]["kind"].as_str(), Some("call" | "reply")),
                "Page bridge calls must target their preload"
            );
        }
        let target = serde_json::from_value::<Role>(target.clone()).ok();
        let (link, rust) = {
            let host = host.try_borrow().context("Host is busy")?;
            (host.link.clone(), host.rust_main)
        };
        if let Some(link) = link {
            use crate::desktop::link::{ToMain, ToPage};
            use payload::Payload;
            match (role, target) {
                (Role::Main, Some(Role::Preload(id))) => {
                    let mut message = args[1].take();
                    message["sender"] = serde_json::to_value(role)?;
                    link.page(ToPage::Bridge(id, Payload::snapshot(message)));
                    return Ok(serde_json::Value::Null);
                }
                (Role::Preload(_), Some(Role::Main)) => {
                    let mut message = args[1].take();
                    message["sender"] = serde_json::to_value(role)?;
                    let kind = message["kind"].as_str().unwrap_or_default().to_owned();
                    anyhow::ensure!(rust == (kind == "rust"), "Bridge target is unavailable");
                    let message = Payload::snapshot(message);
                    return Ok(match kind.as_str() {
                        "sync" => link
                            .ask(|answer| ToMain::Sync(message, answer))?
                            .into_value(),
                        "event" => {
                            link.main(ToMain::Bridge(message));
                            serde_json::Value::Null
                        }
                        _ => {
                            link.main(ToMain::Bridge(message));
                            serde_json::json!({"pending":true})
                        }
                    });
                }
                _ => {}
            }
        }
        let destination = HOSTS.with(|hosts| {
            hosts.borrow().iter().find_map(|(&ctx, binding)| {
                if Rc::ptr_eq(host, &binding.host) && target == Some(binding.role) {
                    Some((ctx, binding.api.clone()))
                } else {
                    None
                }
            })
        });
        if destination.is_none() && matches!(role, Role::Main) && values[1]["kind"] == "event" {
            return Ok(serde_json::Value::Null);
        }
        let (destination, target_api) = destination.context("Bridge target is unavailable")?;
        let mut message = args[1].take();
        message["sender"] = serde_json::to_value(role)?;
        return Ok(receive(
            &target_api,
            destination as Ptr,
            payload::Payload::snapshot(message),
        )?
        .into_value());
    }
    if let Some(op) = op.strip_prefix("web.") {
        anyhow::ensure!(
            !op.starts_with("serve.") || matches!(role, Role::Main),
            "HTTP servers are only available in the main context"
        );
        anyhow::ensure!(
            !op.starts_with("tcp.") || !matches!(role, Role::Renderer(_)),
            "TCP sockets require a native context"
        );
        anyhow::ensure!(
            !matches!(role, Role::Renderer(_)) || op != "worker.start" || values[1] == false,
            "Page workers cannot request native APIs"
        );
        web.try_borrow_mut()
            .context("Web is busy")?
            .call(op, &mut args)
    } else if op.starts_with("sqlite.") {
        sqlite
            .try_borrow_mut()
            .context("SQLite is busy")?
            .call(role, op, &args)
    } else if let (Role::Renderer(id), Some(op)) = (role, op.strip_prefix("gpu.")) {
        gpu(binding, ctx, id, op, &serde_json::to_vec(&args)?)
    } else if let (Role::Renderer(id), Some(op)) = (role, op.strip_prefix("media.")) {
        host.try_borrow_mut()
            .context("Host is busy")?
            .media(id, op, &args)
    } else if let (Role::Renderer(id), "dom.contextMenu") = (role, op) {
        if let Some(link) = host.try_borrow().context("Host is busy")?.link.clone() {
            link.event(
                format!("__appsEvent('context-menu',{id},{})", values[0]),
                "apps:context-menu",
            );
        }
        Ok(serde_json::Value::Null)
    } else if let (Role::Renderer(id), "dom.endComposition") = (role, op) {
        host.try_borrow_mut().context("Host is busy")?.command(
            crate::desktop::link::Target::Main,
            serde_json::json!({"op":"window","args":["composition",id]}),
        );
        Ok(serde_json::Value::Null)
    } else if let (Role::Renderer(id), "dom.filePicker") = (role, op) {
        let picker = &values[0];
        anyhow::ensure!(
            picker["id"].is_u64()
                && picker["multiple"].is_boolean()
                && picker["directory"].is_boolean()
                && picker["accept"].is_string(),
            "Invalid file picker"
        );
        if let Some(link) = host.try_borrow().context("Host is busy")?.link.clone() {
            let picker = serde_json::json!({"id":picker["id"],"multiple":picker["multiple"],"directory":picker["directory"],"accept":picker["accept"]});
            link.event(
                format!("__appsEvent('file-picker',{id},{picker})"),
                "apps:file-picker",
            );
        }
        Ok(serde_json::Value::Null)
    } else if let (Role::Renderer(id), "dom.selectPopup") = (role, op) {
        let popup = &values[0];
        anyhow::ensure!(
            popup["id"].is_u64()
                && popup["selected"].is_i64()
                && ["x", "y", "width"]
                    .iter()
                    .all(|name| popup[name].is_number())
                && popup["items"].as_array().is_some_and(|items| items
                    .iter()
                    .all(|item| item.is_null() || item[0].is_string() && item[1].is_u64())),
            "Invalid select popup"
        );
        if let Some(link) = host.try_borrow().context("Host is busy")?.link.clone() {
            link.event(
                format!("__appsEvent('select-popup',{id},{popup})"),
                "apps:select-popup",
            );
        }
        Ok(serde_json::Value::Null)
    } else if let (Role::Renderer(id), "dom.open") = (role, op) {
        // Main runs these with page data, so only these shapes and `<webview>` methods pass.
        let data = &values[3];
        let valid = match (values[0].as_str(), values[1].as_str()) {
            (Some("open"), _) => (1..5).all(|index| values[index].is_string()),
            (Some("closed" | "close"), _) => values[1].is_u64(),
            (Some("webview"), Some("attach")) => {
                values[2].is_null() && data["src"].is_string() && data["partition"].is_string()
            }
            (Some("webview"), Some("bounds")) => {
                values[2].is_u64()
                    && ["x", "y", "width", "height"]
                        .iter()
                        .all(|key| data["bounds"][key].is_number())
                    && data["visible"].is_boolean()
            }
            (Some("webview"), Some("detach")) => values[2].is_u64(),
            (Some("webview"), Some("call")) => {
                values[2].is_u64()
                    && data["args"].is_array()
                    && (data["reply"].is_null() || data["reply"].is_u64())
                    && matches!(
                        data["method"].as_str(),
                        Some(
                            "loadURL"
                                | "getURL"
                                | "getTitle"
                                | "isLoading"
                                | "canGoBack"
                                | "canGoForward"
                                | "goBack"
                                | "goForward"
                                | "reload"
                                | "stop"
                                | "focus"
                                | "openDevTools"
                                | "setZoomFactor"
                                | "getZoomFactor"
                                | "setZoomLevel"
                                | "getZoomLevel"
                                | "executeJavaScript"
                                | "insertCSS"
                                | "getUserAgent"
                        )
                    )
            }
            _ => false,
        };
        anyhow::ensure!(valid, "Invalid window request");
        let link = host.try_borrow().context("Host is busy")?.link.clone();
        Ok(link
            .context("The main process is unavailable")?
            .ask(|answer| crate::desktop::link::ToMain::Open(id, args.clone(), answer))?
            .into_value())
    } else if let (Role::Renderer(_), "dom.clipboard") = (role, op) {
        // `execCommand` checks for user input before it calls this.
        Ok(CLIPBOARD
            .get()
            .and_then(|clipboard| clipboard(values[0].as_str().map(str::to_owned)))
            .map_or(serde_json::Value::Null, serde_json::Value::String))
    } else {
        host.try_borrow_mut()
            .context("Host is busy")?
            .call(role, op, args)
    }
}

// Account for native DOM memory so removed wrappers trigger GC before row churn exhausts memory.
fn report_nodes(binding: &Binding, ctx: Ptr, id: u64) {
    let created = binding
        .host
        .try_borrow_mut()
        .ok()
        .and_then(|mut host| Some(std::mem::take(&mut host.dom.get_mut(&id)?.created)));
    if let Some(created) = created.filter(|&created| created > 0) {
        // Count only wrapped nodes: JSC resets this cost only after a full GC.
        unsafe { (binding.api.extra_memory)(ctx, created * 65536) };
    }
}

fn gpu(binding: &Binding, ctx: Ptr, id: u64, op: &str, args: &[u8]) -> Result<serde_json::Value> {
    let mut host = binding.host.try_borrow_mut().context("Host is busy")?;
    let result = host.gpu(id, op, args);
    // JSC cannot see GPU memory, so wrappers that own it would wait for an unrelated GC.
    if let Some(gpu) = host.graphics.get_mut(&id)
        && let bytes = gpu.take_allocated()
        && bytes > 0
    {
        drop(host);
        unsafe { (binding.api.extra_memory)(ctx, bytes as usize) };
    }
    result
}

fn buffer(api: &JavaScriptCore, ctx: Ptr, bytes: bytes::Bytes, typed: bool) -> Ptr {
    // Copy small results into JSC memory. External buffers cost more and escape GC accounting.
    if typed
        && bytes.len() <= 16384
        && let Ok((array, out)) = api.uint8_array(ctx, bytes.len())
    {
        out.copy_from_slice(&bytes);
        return array;
    }
    let bytes = Box::new(Vec::from(bytes));
    unsafe {
        let array = (api.make_buffer)(
            ctx,
            bytes.as_ptr().cast_mut().cast(),
            bytes.len(),
            Some(payload::release),
            Box::into_raw(bytes).cast(),
            ptr::null_mut(),
        );
        if typed {
            (api.typed_from_buffer)(ctx, 3, array, ptr::null_mut())
        } else {
            array
        }
    }
}

fn output(api: &JavaScriptCore, ctx: Ptr, payload: payload::Payload) -> Ptr {
    use serde_json::Value;
    let payload::Payload { value, mut buffers } = payload;
    unsafe {
        match value {
            Value::Null => (api.null)(ctx),
            Value::Bool(value) => (api.boolean)(ctx, value),
            Value::Number(value) => (api.number)(ctx, value.as_f64().unwrap_or(f64::NAN)),
            Value::String(text) => api
                .utf8(ctx, text.as_bytes())
                .unwrap_or_else(|_| (api.null)(ctx)),
            Value::Array(values)
                if values.len() <= 32
                    && values
                        .iter()
                        .all(|value| !value.is_array() && !value.is_object()) =>
            {
                // The items stay on the stack, where JSC's conservative scan keeps them alive.
                let mut items = [ptr::null_mut(); 32];
                let count = values.len();
                for (item, value) in items.iter_mut().zip(values) {
                    *item = output(api, ctx, value.into());
                }
                (api.make_array)(ctx, count, items.as_ptr(), ptr::null_mut())
            }
            value => {
                if let Some(bytes) = value
                    .get("$appsBytes")
                    .and_then(Value::as_u64)
                    .and_then(|index| buffers.get_mut(index as usize))
                {
                    return buffer(api, ctx, std::mem::take(bytes), true);
                }
                // See `payload::raw`.
                let (text, value) = match value {
                    Value::Object(mut raw) if raw.contains_key("$appsRaw") => {
                        match (raw.remove("$appsRaw"), raw.remove("$appsBuffers")) {
                            (Some(Value::String(text)), Some(tree)) => (text, tree),
                            _ => return (api.null)(ctx),
                        }
                    }
                    value => (value.to_string(), value),
                };
                let markers = !buffers.is_empty();
                let json = api.string_owned(text);
                let root = (api.json_value)(ctx, json);
                (api.string_release)(json);
                if !markers || root.is_null() {
                    return root;
                }
                (api.protect)(ctx, root);
                let mut arrays: Vec<Ptr> = vec![ptr::null_mut(); buffers.len()];
                let mut pending = vec![(&value, root)];
                while let Some((value, object)) = pending.pop() {
                    let children: Vec<(Ptr, Option<u32>, &Value)> = match value {
                        Value::Array(values) => values
                            .iter()
                            .enumerate()
                            .map(|(index, value)| (ptr::null_mut(), Some(index as u32), value))
                            .collect(),
                        Value::Object(values) => values
                            .iter()
                            .map(|(key, value)| (api.string(key), None, value))
                            .collect(),
                        _ => Vec::new(),
                    };
                    for (key, index, child) in children {
                        if let Some(marker) = child.get("$appsBytes").and_then(Value::as_u64)
                            && let Some(bytes) = buffers.get_mut(marker as usize)
                        {
                            let array = &mut arrays[marker as usize];
                            if array.is_null() {
                                *array = buffer(api, ctx, std::mem::take(bytes), false);
                                (api.protect)(ctx, *array);
                            }
                            let view = (api.typed_from_buffer)(ctx, 3, *array, ptr::null_mut());
                            match index {
                                Some(index) => {
                                    (api.set_index)(ctx, object, index, view, ptr::null_mut())
                                }
                                None => (api.set)(ctx, object, key, view, 0, ptr::null_mut()),
                            }
                        } else if child.is_array() || child.is_object() {
                            let inner = match index {
                                Some(index) => (api.get_index)(ctx, object, index, ptr::null_mut()),
                                None => (api.get)(ctx, object, key, ptr::null_mut()),
                            };
                            if !inner.is_null() && (api.value_type)(ctx, inner) == 5 {
                                pending.push((child, inner));
                            }
                        }
                        if !key.is_null() {
                            (api.string_release)(key);
                        }
                    }
                }
                for array in arrays.into_iter().filter(|array| !array.is_null()) {
                    (api.unprotect)(ctx, array);
                }
                (api.unprotect)(ctx, root);
                root
            }
        }
    }
}
unsafe extern "C" fn now(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    _count: usize,
    _args: *const Ptr,
    _exception: *mut Ptr,
) -> Ptr {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let api = HOSTS.with(|hosts| {
        hosts
            .borrow()
            .get(&(ctx as usize))
            .map(|binding| binding.api.clone())
    });
    let Some(api) = api else {
        return ptr::null_mut();
    };
    unsafe {
        (api.number)(
            ctx,
            START
                .get_or_init(std::time::Instant::now)
                .elapsed()
                .as_secs_f64()
                * 1000.,
        )
    }
}

unsafe extern "C" fn is_proxy(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    _exception: *mut Ptr,
) -> Ptr {
    let Some(api) = HOSTS.with(|hosts| hosts.borrow().get(&(ctx as usize)).map(|b| b.api.clone()))
    else {
        return ptr::null_mut();
    };
    unsafe {
        let proxy =
            count > 0 && (api.value_type)(ctx, *args) == 5 && !(api.proxy_target)(*args).is_null();
        (api.boolean)(ctx, proxy)
    }
}

unsafe extern "C" fn ffi_call(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    let binding = HOSTS.with(|hosts| hosts.borrow().get(&(ctx as usize)).cloned());
    let Some(binding) = binding else {
        return ptr::null_mut();
    };
    let (api, role, ffi) = (&binding.api, binding.role, &binding.ffi);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || -> Result<crate::node::ffi::Output> {
            anyhow::ensure!(count > 0, "Missing FFI request");
            let values = unsafe { std::slice::from_raw_parts(args, count) };
            let request = serde_json::from_str(&api.text(ctx, values[0]))?;
            ffi.call(api, ctx, role, &request, &values[1..])
        },
    ));
    let value = match result {
        Ok(Ok(crate::node::ffi::Output::Object(value))) => return value,
        Ok(Ok(crate::node::ffi::Output::Json(value))) => serde_json::json!({"value":value}),
        Ok(Err(error)) => serde_json::json!({"error":error.to_string()}),
        Err(_) => serde_json::json!({"error":"FFI call failed"}),
    };
    let string = api.string(&value.to_string());
    unsafe {
        let result = (api.make_string)(ctx, string);
        (api.string_release)(string);
        if !exception.is_null() {
            *exception = ptr::null_mut();
        }
        result
    }
}

fn call_json<T>(
    api: &JavaScriptCore,
    ctx: Ptr,
    name: &str,
    arguments: &[&str],
    read: impl FnOnce(Ptr) -> T,
) -> Result<T> {
    let key = api.string(name);
    let mut exception = ptr::null_mut();
    let function = unsafe { (api.get)(ctx, (api.global)(ctx), key, &mut exception) };
    unsafe { (api.string_release)(key) };
    ensure!(
        exception.is_null()
            && unsafe { (api.value_type)(ctx, function) } == 5
            && unsafe { (api.is_function)(ctx, function) },
        "Missing JavaScript function: {name}"
    );
    let values: Vec<Ptr> = arguments
        .iter()
        .map(|argument| {
            let text = api.string(argument);
            let value = unsafe { (api.json_value)(ctx, text) };
            unsafe {
                (api.string_release)(text);
                // Values kept in a heap Vec are invisible to JSC's stack scan.
                (api.protect)(ctx, value);
            }
            value
        })
        .collect();
    let result = if values.iter().any(|value| value.is_null()) {
        Err(anyhow::anyhow!("Invalid JSON argument for {name}"))
    } else {
        let start = std::time::Instant::now();
        let result = unsafe {
            (api.invoke)(
                ctx,
                function,
                ptr::null_mut(),
                values.len(),
                values.as_ptr(),
                &mut exception,
            )
        };
        api.busy.set(api.busy.get() + start.elapsed());
        if exception.is_null() {
            Ok(read(result))
        } else {
            Err(anyhow::anyhow!("{}", api.text(ctx, exception)))
        }
    };
    for value in values.into_iter().filter(|value| !value.is_null()) {
        unsafe { (api.unprotect)(ctx, value) };
    }
    result
}

unsafe extern "C" fn page_limit(ctx: Ptr, _: Ptr) -> bool {
    let binding = HOSTS.with(|hosts| {
        hosts.try_borrow().ok().and_then(|hosts| {
            hosts
                .get(&(ctx as usize))
                .map(|binding| (binding.api.clone(), binding.role))
        })
    });
    let Some((api, role)) = binding else {
        return false;
    };
    let (Role::Renderer(id) | Role::Preload(id)) = role else {
        return false;
    };
    // The limit ends with this call, so a script that goes on needs a new one.
    let again = || {
        unsafe { (api.execution_limit)(api.group, PAGE_TICK, Some(page_limit), ptr::null_mut()) };
        false
    };
    if stopping(id) {
        return true;
    }
    if !matches!(role, Role::Renderer(_)) {
        return again();
    }
    // Count watchdog ticks per busy value: JSC resets its budget per run but does not identify each tick's
    // run.
    let busy = api.busy.get();
    let ticks = match RUN_TICKS.get() {
        (last, ticks) if last == busy => ticks + 1,
        _ => 1,
    };
    RUN_TICKS.set((busy, ticks));
    if f64::from(ticks) * PAGE_TICK >= PAGE_LIMIT as f64 {
        eprintln!("apps: stopped a page script that ran for more than {PAGE_LIMIT} seconds");
        return true;
    }
    again()
}

const PAGE_LIMIT: u64 = 10;
const PAGE_TICK: f64 = 0.1;

static STOPPING: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
static STOP_ALL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn stopping(id: u64) -> bool {
    STOP_ALL.load(std::sync::atomic::Ordering::Acquire)
        || STOPPING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&id)
}

pub(crate) fn stop_page(id: u64) {
    STOPPING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(id);
}

pub(crate) fn page_stopped(id: u64) {
    STOPPING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|&other| other != id);
}

pub(crate) fn stop_pages() {
    STOP_ALL.store(true, std::sync::atomic::Ordering::Release);
}

struct Interrupt {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    group: Ptr,
    limit: Limit,
}

// Bundles run one entry script per call of this source, so callers loop until it returns `false`.
pub(crate) const NEXT: &str = "this.__appsNext?.()";

pub struct JsContext {
    api: Rc<JavaScriptCore>,
    ctx: Ptr,
    napi: Option<Rc<crate::node::napi::Root>>,
    interrupt: Option<Box<Interrupt>>,
    scope: Ptr,
}

impl JsContext {
    pub fn new(api: Rc<JavaScriptCore>, host: Rc<RefCell<Host>>, role: Role) -> Result<Self> {
        Self::new_interruptible(api, host, role, None)
    }

    pub fn new_interruptible(
        api: Rc<JavaScriptCore>,
        host: Rc<RefCell<Host>>,
        role: Role,
        interrupt: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<Self> {
        let devtools = !host.borrow().packaged;
        let ctx = unsafe { (api.create)(api.group, ptr::null_mut()) };
        anyhow::ensure!(!ctx.is_null(), "Cannot create a JavaScriptCore context");
        unsafe {
            (api.inspectable)(ctx, devtools);
            let name = api.string(&format!("apps {role:?}"));
            (api.set_name)(ctx, name);
            (api.string_release)(name);
        }
        let binding = Binding {
            api: api.clone(),
            host: host.clone(),
            role,
            sqlite: RefCell::default(),
            web: RefCell::default(),
            ffi: Default::default(),
            node: RefCell::default(),
            inspector: std::cell::Cell::new(ptr::null_mut()),
        };
        binding.web.borrow_mut().renderer = matches!(role, Role::Renderer(_));
        let callback_api = api.clone();
        binding
            .sqlite
            .borrow_mut()
            .set_authorizer_callback(Rc::new(move |id, action, args| {
                call_json(
                    &callback_api,
                    ctx,
                    "__appsSqliteAuthorize",
                    &[
                        &id.to_string(),
                        &action.to_string(),
                        &serde_json::to_string(&args)?,
                    ],
                    |value| callback_api.text(ctx, value),
                )?
                .parse::<i32>()
                .context("Authorizer must return a SQLite authorization code")
            }));
        binding.web.borrow_mut().configure_storage(
            &host.borrow(),
            match role {
                Role::Renderer(id) | Role::Preload(id) => Some(id),
                Role::Main => None,
            },
        );
        HOSTS.with(|hosts| hosts.borrow_mut().insert(ctx as usize, Rc::new(binding)));
        let napi = if matches!(role, Role::Renderer(_)) {
            None
        } else {
            Some(crate::node::napi::Root::new(api.clone(), ctx, &host))
        };
        let mut engine = Self {
            api,
            ctx,
            napi,
            interrupt: None,
            scope: ptr::null_mut(),
        };
        if let Some(stop) = interrupt {
            unsafe extern "C" fn interrupted(_: Ptr, context: Ptr) -> bool {
                let state = unsafe { &*context.cast::<Interrupt>() };
                if state.stop.load(std::sync::atomic::Ordering::Acquire) {
                    return true;
                }
                unsafe { (state.limit)(state.group, 0.01, Some(interrupted), context) };
                false
            }
            let mut state = Box::new(Interrupt {
                stop,
                group: engine.api.group,
                limit: engine.api.execution_limit,
            });
            unsafe {
                (state.limit)(
                    state.group,
                    0.01,
                    Some(interrupted),
                    (&mut *state as *mut Interrupt).cast(),
                )
            };
            engine.interrupt = Some(state);
        } else if matches!(role, Role::Renderer(_)) {
            // The limit is per context group, which every engine of this thread shares.
            unsafe {
                (engine.api.execution_limit)(
                    engine.api.group,
                    PAGE_TICK,
                    Some(page_limit),
                    ptr::null_mut(),
                )
            };
        }
        encoding::install(&engine.api, ctx);
        let natives = [
            ("__appsNative", call as Callback),
            ("__appsNow", now),
            ("__appsLoadAddon", crate::node::napi::load),
            ("__appsFFINative", ffi_call),
            ("__appsIsProxy", is_proxy),
        ];
        let count = if matches!(role, Role::Renderer(_)) {
            2
        } else {
            natives.len()
        };
        for &(name, callback) in &natives[..count] {
            let name = engine.api.string(name);
            unsafe {
                let callback = (engine.api.function)(ctx, name, callback);
                (engine.api.set)(
                    ctx,
                    (engine.api.global)(ctx),
                    name,
                    callback,
                    0,
                    ptr::null_mut(),
                );
                (engine.api.string_release)(name);
            }
        }
        if devtools || matches!(role, Role::Renderer(_)) {
            let name = engine.api.string("__appsCompile");
            unsafe {
                let callback = (engine.api.function)(ctx, name, compile);
                // Pages get the compiler only as `this` of their own scripts, see `eval_page`.
                let target = if matches!(role, Role::Renderer(_)) {
                    engine.scope = (engine.api.object_make)(ctx, ptr::null_mut(), ptr::null_mut());
                    (engine.api.protect)(ctx, engine.scope);
                    let flag = engine.api.string("__appsDevtools");
                    (engine.api.set)(
                        ctx,
                        engine.scope,
                        flag,
                        (engine.api.boolean)(ctx, devtools),
                        0,
                        ptr::null_mut(),
                    );
                    (engine.api.string_release)(flag);
                    engine.scope
                } else {
                    (engine.api.global)(ctx)
                };
                (engine.api.set)(ctx, target, name, callback, 0, ptr::null_mut());
                (engine.api.string_release)(name);
            }
        }
        let name = engine.api.string("__appsChunk");
        unsafe {
            let callback = (engine.api.function)(ctx, name, chunk);
            let target = if engine.scope.is_null() {
                (engine.api.global)(ctx)
            } else {
                engine.scope
            };
            (engine.api.set)(ctx, target, name, callback, 0, ptr::null_mut());
            (engine.api.string_release)(name);
        }
        // Bootstrap scripts call into the host, so the host is only borrowed per check.
        let enabled = |feature| host.borrow().enabled(feature);
        let disabled: Vec<_> = crate::bundle::FEATURES
            .into_iter()
            .filter(|name| !enabled(name))
            .collect();
        engine.eval(
            &format!(
                "globalThis.__appsRole={};globalThis.__appsDisabled={};",
                serde_json::to_string(&role)?,
                serde_json::to_string(&disabled)?
            ),
            "apps:role",
        )?;
        if let Some(aot) = engine.api.aot {
            aot.attach(ctx);
        }
        if !matches!(role, Role::Renderer(_)) {
            for (url, source) in NODE_SCRIPTS {
                engine.run(source(), url)?;
            }
        }
        for (url, source) in WEB_SCRIPTS {
            engine.run(source(), url)?;
        }
        let name = engine.api.string("__appsRejection");
        unsafe {
            let callback = (engine.api.get)(ctx, (engine.api.global)(ctx), name, ptr::null_mut());
            (engine.api.rejection_callback)(ctx, callback, ptr::null_mut());
            (engine.api.string_release)(name);
        }
        let name = engine.api.string("__appsModuleLoader");
        unsafe {
            (engine.api.module_loader)(
                ctx,
                (engine.api.get)(ctx, (engine.api.global)(ctx), name, ptr::null_mut()),
            );
            (engine.api.string_release)(name);
        }
        engine.eval(
            "delete globalThis.__appsRejection;delete globalThis.__appsModuleLoader",
            "apps:rejections",
        )?;
        if matches!(role, Role::Renderer(_)) {
            for (url, source) in PAGE_SCRIPTS {
                if url != "apps:gpu" || enabled("webgpu") {
                    engine.run(source(), url)?;
                }
            }
            if !enabled("jsExecute") {
                let message = engine.api.string("Code generation is disabled in pages");
                unsafe {
                    (engine.api.eval_enabled)(ctx, false, message);
                    (engine.api.string_release)(message);
                }
            }
        }
        Ok(engine)
    }

    pub fn eval(&self, source: &str, url: &str) -> Result<String> {
        self.evaluate(source, url, ptr::null_mut())
    }

    fn run(&self, source: &'static str, url: &'static str) -> Result<()> {
        if let Some(aot) = self.api.aot
            && let Some(module) = aot.runtime(url)
        {
            let mut exception = ptr::null_mut();
            let start = std::time::Instant::now();
            let result = aot.call(&self.api, self.ctx, module, ptr::null_mut(), &mut exception);
            self.api.busy.set(self.api.busy.get() + start.elapsed());
            return self.checked(url, result, exception).map(drop);
        }
        let (bytecode, build) = {
            let mut scripts = SCRIPTS.lock().unwrap();
            let found = scripts
                .list
                .iter()
                .find(|script| script.url == url)
                .map(|script| script.bytecode);
            if found.is_none() {
                scripts.list.push(Script {
                    url,
                    source,
                    bytecode: None,
                });
            }
            let build = found == Some(None) && !scripts.building;
            scripts.building |= build;
            (found.flatten().unwrap_or_default(), build)
        };
        if build {
            let _ = std::thread::Builder::new()
                .name("apps-bytecode".into())
                .spawn(build_bytecode);
        }
        let source_url = self.api.string(url);
        let mut exception = ptr::null_mut();
        let start = std::time::Instant::now();
        let result = unsafe {
            self.api.evaluate_latin1(
                self.ctx,
                source.as_bytes(),
                ptr::null_mut(),
                source_url,
                bytecode,
                &mut exception,
            )
        };
        self.api.busy.set(self.api.busy.get() + start.elapsed());
        unsafe { (self.api.string_release)(source_url) };
        self.checked(url, result, exception).map(drop)
    }

    fn checked(&self, url: &str, result: Ptr, exception: Ptr) -> Result<Ptr> {
        if !exception.is_null() {
            let message = self.api.text(self.ctx, exception);
            self.log(url, &message);
            bail!("{message}");
        }
        anyhow::ensure!(!result.is_null(), "JSC returned no value");
        Ok(result)
    }

    pub fn eval_page(&self, source: &str, url: &str) -> Result<String> {
        struct Armed(usize);
        impl Drop for Armed {
            fn drop(&mut self) {
                ARMED.set(self.0);
            }
        }
        let _armed = Armed(ARMED.replace(self.ctx as usize));
        self.evaluate(source, url, self.scope)
    }

    pub fn addons_pending(&self) -> bool {
        self.napi.as_ref().is_some_and(|napi| napi.pending())
    }

    pub fn call(&self, name: &str, arguments: &[&str]) -> Result<String> {
        call_json(&self.api, self.ctx, name, arguments, |value| {
            self.api.text(self.ctx, value)
        })
    }

    fn evaluate(&self, source: &str, url: &str, this: Ptr) -> Result<String> {
        let compiled = match self.api.aot {
            Some(aot) => aot.find(source.as_bytes())?.map(|module| (aot, module)),
            None => None,
        };
        let source_url = self.api.string(url);
        let mut exception = ptr::null_mut();
        let start = std::time::Instant::now();
        let bytecode = bundled(source.as_bytes());
        let result = match compiled {
            Some((aot, module)) => aot.call(&self.api, self.ctx, module, this, &mut exception),
            None if bytecode.is_some() || in_place(source.as_bytes()) => unsafe {
                self.api.evaluate_latin1(
                    self.ctx,
                    source.as_bytes(),
                    this,
                    source_url,
                    bytecode.unwrap_or_default(),
                    &mut exception,
                )
            },
            _ => {
                let script = self.api.string(source);
                let result = unsafe {
                    (self.api.evaluate)(self.ctx, script, this, source_url, 1, &mut exception)
                };
                unsafe { (self.api.string_release)(script) };
                result
            }
        };
        self.api.busy.set(self.api.busy.get() + start.elapsed());
        unsafe { (self.api.string_release)(source_url) };
        Ok(self
            .api
            .text(self.ctx, self.checked(url, result, exception)?))
    }

    fn log(&self, url: &str, message: &str) {
        HOSTS.with(|hosts| {
            if let Some(binding) = hosts.borrow().get(&(self.ctx as usize))
                && let Ok(mut host) = binding.host.try_borrow_mut()
                && !host.packaged
            {
                host.log(serde_json::json!({"type":"error","message":format!("{url}: {message}")}));
            }
        });
    }

    pub(crate) fn receive(&self, payload: payload::Payload) -> Result<payload::Payload> {
        receive(&self.api, self.ctx, payload)
    }

    pub(crate) fn deliver(&self, payload: payload::Payload) -> Result<()> {
        bridge(&self.api, self.ctx, "__appsBridgeDeliver", payload).map(|_| ())
    }

    pub(crate) fn heap(&self) -> Result<serde_json::Value> {
        unsafe {
            let json =
                (self.api.json_string)(self.ctx, (self.api.memory)(self.ctx), 0, ptr::null_mut());
            ensure!(!json.is_null(), "Cannot read heap statistics");
            let text = self
                .api
                .text(self.ctx, (self.api.make_string)(self.ctx, json));
            (self.api.string_release)(json);
            Ok(serde_json::from_str(&text)?)
        }
    }

    pub fn collect_garbage(&self) {
        unsafe {
            (self.api.garbage_collect)(self.ctx);
        }
    }

    pub fn tick(&self) -> Result<i64> {
        if let Some(napi) = &self.napi {
            napi.sweep();
            napi.drain();
        }
        // Run one macrotask per call so JSC drains microtasks between tasks. -2 signals a completed
        // macrotask.
        loop {
            let delay = call_json(&self.api, self.ctx, "__appsTick", &[], |value| unsafe {
                (self.api.to_number)(self.ctx, value, ptr::null_mut())
            })
            .inspect_err(|error| self.log("apps:tick", &error.to_string()))?;
            if delay != -2. {
                // Delays can be fractional, so round up or the loop wakes before the timer is due.
                return Ok(if delay.is_finite() {
                    delay.ceil() as i64
                } else {
                    -1
                });
            }
        }
    }

    pub fn validate_bundle(&self, files: &BBTreeMap<String, String>) -> Result<()> {
        self.install(files)?;
        self.eval_page(&crate::bundle::script_source(files, false), "apps:validate")?;
        Ok(())
    }

    pub fn load_bundle(&self, files: &BBTreeMap<String, String>, entry: &str) -> Result<()> {
        self.load_main(files, &crate::bundle::script_source(files, true), entry)
    }

    // `source` is a main script from `script_source`. A built app keeps it in its mapped bundle.
    pub fn load_main(
        &self,
        files: &BBTreeMap<String, String>,
        source: &str,
        entry: &str,
    ) -> Result<()> {
        self.install(files)?;
        self.eval_page(source, entry)
            .context("Cannot execute the app bundle")?;
        while self.eval_page(NEXT, entry)? == "true" {}
        Ok(())
    }

    fn install(&self, files: &BBTreeMap<String, String>) -> Result<()> {
        HOSTS.with(|hosts| {
            let binding = &hosts.borrow()[&(self.ctx as usize)];
            let mut host = binding.host.borrow_mut();
            host.install(files)?;
            if !host.scripts.is_empty() {
                let window = match binding.role {
                    Role::Renderer(id) | Role::Preload(id) => Some(id),
                    Role::Main => None,
                };
                binding.web.borrow_mut().configure_storage(&host, window);
            }
            Ok(())
        })
    }
}

impl Drop for JsContext {
    fn drop(&mut self) {
        if let Some(napi) = &self.napi {
            napi.close();
        }
        if !self.scope.is_null() {
            unsafe { (self.api.unprotect)(self.ctx, self.scope) };
        }
        let binding = HOSTS.with(|hosts| hosts.borrow_mut().remove(&(self.ctx as usize)));
        if let Some(binding) = &binding {
            if let Role::Preload(id) = binding.role
                && let Ok(mut host) = binding.host.try_borrow_mut()
            {
                host.bridge.remove(&id);
            }
            let peers = HOSTS.with(|hosts| {
                hosts
                    .borrow()
                    .iter()
                    .filter(|(_, peer)| Rc::ptr_eq(&binding.host, &peer.host))
                    .map(|(&ctx, peer)| (ctx, peer.api.clone()))
                    .collect::<Vec<_>>()
            });
            for (ctx, api) in peers {
                let script = api.string(&format!(
                    "globalThis.__appsBridgeClosed?.({})",
                    serde_json::to_string(&binding.role).unwrap()
                ));
                unsafe {
                    (api.evaluate)(
                        ctx as Ptr,
                        script,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        1,
                        ptr::null_mut(),
                    );
                    (api.string_release)(script);
                }
            }
        }
        if let Some(connection) = binding
            .map(|binding| binding.inspector.get())
            .filter(|connection| !connection.is_null())
            && let Some(inspector) = &self.api.inspector
        {
            // A connected inspector keeps the global object alive.
            unsafe { (inspector.disconnect)(self.ctx, connection) };
        }
        unsafe {
            (self.api.release)(self.ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_host(id: u64, html: &str) -> Rc<RefCell<Host>> {
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            id,
            crate::host::Page {
                doc: crate::dom::Document::parse(html),
                dirty: false,
            },
        );
        host
    }

    #[test]
    fn bundled_chunks_keep_nul_bytes_and_non_ascii_text() {
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().scripts = std::sync::Arc::new(std::collections::HashMap::from([
            (
                "script-1.js".to_owned(),
                bytes::Bytes::from_static(
                    b"globalThis.a = `x\0y`.length; globalThis.f = function f() { return 1 };",
                ),
            ),
            (
                "script-2.js".to_owned(),
                bytes::Bytes::from("globalThis.b = '\u{4f60}\u{597d}';"),
            ),
        ]));
        let main = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Main).unwrap();
        let script = "__appsChunk('script-1.js'); __appsChunk('script-2.js'); \
            [a, f.toString(), b.length, b.charCodeAt(0)].join('|')";
        assert_eq!(
            main.eval(script, "test").unwrap(),
            "3|function f() { return 1 }|2|20320"
        );
    }

    #[test]
    fn mapped_scripts_run_in_place_and_keep_their_mapping() {
        let map = |source: &[u8]| {
            let mut map = memmap2::MmapMut::map_anon(source.len()).unwrap();
            map.copy_from_slice(source);
            let map = std::sync::Arc::new(map.make_read_only().unwrap());
            add_mapped(std::sync::Arc::downgrade(&map));
            map
        };
        let ascii = map(b"globalThis.f = function f() { return 'in place' }; f.toString()");
        let wide = map("globalThis.b = '\u{4f60}\u{597d}'; b.length".as_bytes());
        let main =
            JsContext::new(JavaScriptCore::load().unwrap(), Rc::default(), Role::Main).unwrap();
        let run = |source: &[u8]| main.eval(std::str::from_utf8(source).unwrap(), "test");
        assert_eq!(run(&ascii).unwrap(), "function f() { return 'in place' }");
        assert_eq!(run(&wide).unwrap(), "2");
        assert_eq!(std::sync::Arc::strong_count(&ascii), 2);
        assert_eq!(std::sync::Arc::strong_count(&wide), 1);
        assert!(!in_place(b"heap"));
    }

    #[test]
    fn dispatched_clicks_run_activation_behavior() {
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body></body>");
        let page = JsContext::new(api, host, Role::Renderer(1)).unwrap();
        let script = "const box = document.createElement('input'); box.type = 'checkbox'; \
            document.body.append(box); const seen = []; box.addEventListener('change', () => seen.push('change')); \
            box.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })); \
            const plain = document.createElement('input'); plain.type = 'checkbox'; \
            plain.dispatchEvent(new Event('click')); [box.checked, seen[0], plain.checked].join()";
        assert_eq!(page.eval(script, "test").unwrap(), "true,change,false");
    }

    #[test]
    fn channel_messages_are_tasks_and_file_readers_finish() {
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body></body>");
        let page = JsContext::new(api, host, Role::Renderer(1)).unwrap();
        let script = "globalThis.seen = []; const { port1, port2 } = new MessageChannel(); \
            port1.onmessage = () => seen.push('message'); port2.postMessage(0); \
            Promise.resolve().then(() => seen.push('microtask')); \
            const reader = new FileReader(); reader.onload = () => seen.push(reader.result); \
            reader.readAsDataURL(new Blob(['hi'], { type: 'text/plain' })); ''";
        page.eval(script, "test").unwrap();
        assert_eq!(page.eval("seen.join()", "test").unwrap(), "microtask");
        for _ in 0..200 {
            page.tick().unwrap();
            if page.eval("String(seen.length)", "test").unwrap() == "3" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            page.eval("seen.join()", "test").unwrap(),
            "microtask,message,data:text/plain;base64,aGk="
        );
    }

    #[test]
    fn contexts_isolate_privileges_and_share_dom_only() {
        let api = JavaScriptCore::load().unwrap();
        assert!(
            crate::node::ffi::State::default()
                .call(
                    &api,
                    ptr::null_mut(),
                    Role::Renderer(1),
                    &serde_json::json!({"op":"info"}),
                    &[]
                )
                .is_err()
        );
        let host = page_host(1, "<body><p id='value'>before</p></body>");
        let main = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
        let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
        let page = JsContext::new(api, host.clone(), Role::Renderer(1)).unwrap();
        assert_eq!(main.eval("typeof process.cwd", "test").unwrap(), "function");
        assert_eq!(
            preload.eval("process.versions.apps", "test").unwrap(),
            crate::VERSION
        );
        assert_eq!(
            page.eval(
                "typeof process + ':' + typeof __apps + ':' + typeof __appsNative + ':' + typeof __appsFFINative + ':' + typeof __appsFFI",
                "test"
            )
            .unwrap(),
            "undefined:undefined:undefined:undefined:undefined"
        );
        page.eval(
            "document.querySelector('#value').textContent = 'after'",
            "test",
        )
        .unwrap();
        assert!(host.borrow().pages[&1].dirty);
        assert_eq!(
            page.eval("document.querySelector('#value').textContent", "test")
                .unwrap(),
            "after"
        );
        assert!(
            host.borrow_mut()
                .call(
                    Role::Renderer(1),
                    "fs.read",
                    serde_json::json!(["Cargo.toml", "utf8"])
                )
                .is_err()
        );
        assert!(
            host.borrow_mut()
                .call(Role::Preload(1), "window", serde_json::json!([]))
                .is_err()
        );
    }
    #[test]
    fn pages_cannot_turn_strings_into_code() {
        let host = page_host(1, "<body></body>");
        host.borrow_mut().disabled = crate::bundle::disabled_features(None).unwrap();
        let page =
            JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
        for code in [
            "eval('1')",
            "globalThis['ev'+'al']('1')",
            "new Function('return 1')",
            "Function('return 1')",
            "(()=>{}).constructor('return 1')",
            "Object.getPrototypeOf(async function(){}).constructor('')",
            "Object.getPrototypeOf(function*(){}).constructor('')",
            "Object.getPrototypeOf(async function*(){}).constructor('')",
            "Reflect.construct(Function, ['return 1'])",
            "__appsCompile('1')",
            "this.__appsCompile('1')",
        ] {
            assert!(page.eval(code, "test").is_err(), "{code}");
        }
        assert_eq!(
            page.eval("(function(){}) instanceof Function && Function.prototype.constructor === Function && typeof Function", "test").unwrap(),
            "function"
        );
        page.eval("globalThis.imported='pending';import('data:text/javascript,globalThis.imported=1').then(()=>imported='ran',()=>imported='blocked');typeof ShadowRealm", "test").map(|kind| assert_eq!(kind, "undefined")).unwrap();
        page.tick().unwrap();
        assert_ne!(page.eval("imported", "test").unwrap(), "ran");
        assert_eq!(
            page.eval_page(
                "globalThis.later=this.__appsCompile;this.__appsCompile('20+22','page')",
                "page"
            )
            .unwrap(),
            "42"
        );
        assert!(page.eval("later('1','page')", "test").is_err());
        assert_eq!(page.eval_page("later('1','page')", "page").unwrap(), "1");
    }

    #[test]
    fn runaway_page_scripts_stop_but_main_keeps_running() {
        let api = JavaScriptCore::load().unwrap();
        // The stop list is global, so this page has an id no other test uses.
        let host = page_host(900, "<body></body>");
        let main = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
        let page = JsContext::new(api.clone(), host, Role::Renderer(900)).unwrap();
        unsafe { (api.execution_limit)(api.group, 0.2, Some(page_limit), ptr::null_mut()) };
        stop_page(900);
        assert!(page.eval("while(true){}", "test").is_err());
        page_stopped(900);
        assert_eq!(page.eval("1+1", "test").unwrap(), "2");
        let started = std::time::Instant::now();
        assert_eq!(
            main.eval(
                "let n=0;const end=performance.now()+500;while(performance.now()<end)n++;n>0",
                "test"
            )
            .unwrap(),
            "true"
        );
        assert!(started.elapsed() >= std::time::Duration::from_millis(500));
    }

    #[test]
    fn timers_and_node_path() {
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            Rc::new(RefCell::new(Host::default())),
            Role::Main,
        )
        .unwrap();
        assert_eq!(
            engine
                .eval("__apps.path.posix.normalize('/a//b/../c/')", "test")
                .unwrap(),
            "/a/c/"
        );
        assert_eq!(
            engine
                .eval("__apps.path.posix.relative('/a/b', '/a/c')", "test")
                .unwrap(),
            "../c"
        );
        assert_eq!(
            engine
                .eval("__apps.path.win32.join('C:\\\\a','b','..','c')", "test")
                .unwrap(),
            "C:\\a\\c"
        );
        engine
            .eval("globalThis.fired=0;setTimeout(()=>fired++,0)", "test")
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_eq!(engine.tick().unwrap(), -1);
        assert_eq!(engine.eval("fired", "test").unwrap(), "1");
        engine
            .eval("Date.now=()=>0;setTimeout(()=>fired++,5.5)", "test")
            .unwrap();
        let delay = engine.tick().unwrap();
        assert!((1..=6).contains(&delay), "{delay}");
        std::thread::sleep(std::time::Duration::from_millis(delay as u64));
        assert_eq!(engine.eval("__appsDelay()", "test").unwrap(), "0");
        assert_eq!(engine.tick().unwrap(), -1);
        assert_eq!(engine.eval("fired", "test").unwrap(), "2");
        assert_eq!(
            engine
                .eval("try{queueMicrotask(1)}catch(e){e.constructor.name}", "test")
                .unwrap(),
            "TypeError"
        );
        engine
            .eval("queueMicrotask(()=>{throw new Error('boom')})", "test")
            .unwrap();
        assert!(engine.tick().unwrap_err().to_string().contains("boom"));
        engine
            .eval("process.on('uncaughtException',e=>globalThis.caught=e.message);queueMicrotask(()=>{throw new Error('again')})", "test")
            .unwrap();
        engine.tick().unwrap();
        assert_eq!(engine.eval("caught", "test").unwrap(), "again");
    }

    #[test]
    fn page_timers_clamp_deep_nesting() {
        let host = page_host(1, "<body></body>");
        let page =
            JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
        page.eval(
            "globalThis.runs=0;const loop=()=>{runs++;setTimeout(loop,0)};setTimeout(loop,0);queueMicrotask(()=>{throw new Error('reported')})",
            "test",
        )
        .unwrap();
        for _ in 0..5 {
            assert_eq!(page.tick().unwrap(), 0);
        }
        let started = std::time::Instant::now();
        let delay = page.tick().unwrap();
        assert!(
            delay >= 3 || started.elapsed() >= std::time::Duration::from_millis(3),
            "{delay}"
        );
        assert_eq!(page.eval("runs", "test").unwrap(), "6");
    }

    #[test]
    fn json_from_lone_surrogates_and_rooted_bytes() {
        let mut json = r#"["a\ud83d","\\ud83d","\n\udfff","\u001f"]"#.to_owned();
        well_formed(&mut json);
        assert_eq!(json, r#"["a\ufffd","\\ud83d","\n\ufffd","\u001f"]"#);
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&json).unwrap(),
            ["a\u{fffd}", "\\ud83d", "\n\u{fffd}", "\u{1f}"]
        );
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body></body>");
        let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
        let page = JsContext::new(api.clone(), host, Role::Renderer(1)).unwrap();
        preload
            .eval(
                "__apps.contextBridge.exposeInMainWorld('api',{text:'a\\ud83d',echo:value=>value+'\\udc00'})",
                "test",
            )
            .unwrap();
        page.eval("__appsSyncBridge()", "test").unwrap();
        assert_eq!(
            page.eval("JSON.stringify([api.text, api.echo('b')])", "test")
                .unwrap(),
            "[\"a\u{fffd}\",\"b\u{fffd}\"]"
        );
        page.eval(
            "globalThis.items=[];Object.defineProperty(items,0,{get:()=>new Uint8Array(1<<16).fill(7)});Object.defineProperty(items,1,{get:()=>new Uint8Array(4)})",
            "test",
        )
        .unwrap();
        let items = unsafe {
            let name = api.string("items");
            let items = (api.get)(page.ctx, (api.global)(page.ctx), name, ptr::null_mut());
            (api.string_release)(name);
            items
        };
        let (buffers, roots) = payload::input(&api, page.ctx, items).unwrap();
        page.eval("delete globalThis.items", "test").unwrap();
        page.collect_garbage();
        page.eval(
            "globalThis.junk=Array.from({length:64},()=>new Uint8Array(1<<16).fill(1))",
            "test",
        )
        .unwrap();
        assert!(buffers[0].len() == 1 << 16 && buffers[0].iter().all(|&byte| byte == 7));
        drop(roots);
    }

    #[test]
    fn preload_document_handles_are_released() {
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body></body>");
        let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
        let page = JsContext::new(api, host, Role::Renderer(1)).unwrap();
        page.eval("globalThis.made=[];const create=document.createElement.bind(document);document.createElement=name=>{const node=create(name);made.push(new WeakRef(node));return node}", "test").unwrap();
        preload
            .eval("(()=>{for(let i=0;i<1000;i++)document.createElement('div').id='x'+i})();document.body===document.body", "test")
            .map(|same| assert_eq!(same, "true"))
            .unwrap();
        let live = || {
            page.eval("made.filter(made=>made.deref()).length", "test")
                .unwrap()
                .parse::<usize>()
                .unwrap()
        };
        assert_eq!(live(), 1000);
        for _ in 0..20 {
            preload.collect_garbage();
            preload.tick().unwrap();
            page.collect_garbage();
            page.eval("__appsDomSweep?.(true)", "test").unwrap();
            if live() < 500 {
                break;
            }
        }
        assert!(live() < 500, "{}", live());
        assert_eq!(
            preload.eval("document.body.tagName", "test").unwrap(),
            "BODY"
        );
    }

    #[test]
    fn replaced_rows_free_without_gc() {
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body><div id=list></div></body>");
        let page = JsContext::new(api, host.clone(), Role::Renderer(1)).unwrap();
        assert_eq!(
            page.eval("customElements.define('x-row', class extends HTMLElement {});const list = document.getElementById('list');for (let i = 0; i < 50; i++) { list.innerHTML = '<div><span>a</span><span>b</span></div>'.repeat(20); list.insertAdjacentHTML('beforeend', '<p><b>c</b></p>'.repeat(20)); __appsDomSweep(); }const kept = list.firstChild, observer = new MutationObserver(() => {});observer.observe(list, { childList: true });list.innerHTML = '<i>x</i>';const records = observer.takeRecords();__appsDomSweep();[kept.textContent, kept.isConnected, records.length, records[0].removedNodes.length, records[0].removedNodes[0] === kept, list.textContent].join()", "test").unwrap(),
            "ab,false,1,40,true,x"
        );
        let host = host.borrow();
        let used = host.pages[&1].doc.nodes.len() - host.dom[&1].free.len();
        assert!(used < 400, "{used}");
    }

    #[test]
    fn resize_observers_see_style_changes_after_the_next_layout() {
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(
            1,
            "<body><div id=box style='width:100px;padding:5px'></div></body>",
        );
        let page = JsContext::new(api, host.clone(), Role::Renderer(1)).unwrap();
        let id: usize = page
            .eval("const box = document.getElementById('box');globalThis.seen = [];new ResizeObserver(entries => seen.push(entries[0].contentRect.width)).observe(box);box._id", "test")
            .unwrap()
            .parse()
            .unwrap();
        let layout = || {
            let mut host = host.borrow_mut();
            let bounds = &mut host.dom.entry(1).or_default().bounds;
            bounds.insert(id, [0., 0., 110., 10.]);
            bounds.touch();
        };
        layout();
        page.eval("__appsLayout()", "test").unwrap();
        page.eval(
            "box.style.padding = '10px'; box.style.width = '90px'; __appsLayout()",
            "test",
        )
        .unwrap();
        page.eval("getComputedStyle(box).paddingLeft", "test")
            .unwrap();
        layout();
        assert_eq!(
            page.eval("__appsLayout(); seen.join()", "test").unwrap(),
            "100,90"
        );
    }

    #[test]
    fn contexts_reach_only_their_host_operations() {
        for (role, op, allowed) in [
            (Role::Main, "native.poll", true),
            (Role::Main, "dom.batch", false),
            (Role::Main, "unknown", false),
            (Role::Preload(1), "native.poll", false),
            (Role::Preload(1), "native.menu.popup", false),
            (Role::Preload(1), "native.shortcut.register", false),
            (Role::Preload(1), "native.theme.set", false),
            (Role::Preload(1), "native.clipboard.readText", true),
            (Role::Preload(1), "native.shell.openExternal", true),
            (Role::Preload(1), "native.image", true),
            (Role::Preload(1), "window", false),
            (Role::Preload(1), "app.path", false),
            (Role::Preload(1), "browser.create", false),
            (Role::Preload(1), "web.serve.listen", false),
            (Role::Preload(1), "node.fs.read", true),
            (Role::Preload(1), "info", true),
            (Role::Renderer(1), "native.clipboard.readText", false),
            (Role::Renderer(1), "node.fs.read", false),
            (Role::Renderer(1), "child.spawn", false),
            (Role::Renderer(1), "pty.spawn", false),
            (Role::Renderer(1), "web.tcp.connect", false),
            (Role::Renderer(1), "web.fetch", true),
            (Role::Renderer(1), "infox", false),
        ] {
            assert_eq!(access(role, op).is_ok(), allowed, "{role:?} {op}");
        }
        let api = JavaScriptCore::load().unwrap();
        let host = page_host(1, "<body></body>");
        let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
        assert_eq!(
            preload
                .eval(
                    "['app','BrowserWindow','webContents','session','dialog','Menu','Tray','globalShortcut','nativeTheme','screen','systemPreferences','powerMonitor','powerSaveBlocker','safeStorage','Notification'].filter(name=>__apps[name]!==undefined).join()",
                    "test"
                )
                .unwrap(),
            ""
        );
        assert_eq!(
            preload
                .eval(
                    "['clipboard','shell','nativeImage','contextBridge','ipcRenderer','webUtils','webFrame'].filter(name=>!__apps[name]).join()",
                    "test"
                )
                .unwrap(),
            ""
        );
        let page = JsContext::new(api, host, Role::Renderer(1)).unwrap();
        let binding = HOSTS.with(|hosts| hosts.borrow()[&(page.ctx as usize)].clone());
        let open = |args| {
            dispatch(&binding, page.ctx, "dom.open", args)
                .unwrap_err()
                .to_string()
        };
        for args in [
            serde_json::json!(["webview", "call", 1, {"method":"constructor","args":[]}]),
            serde_json::json!(["webview", "call", 1, {"method":"getURL","args":[],"reply":"1)"}]),
            serde_json::json!(["webview", "call", "1", {"method":"getURL","args":[]}]),
            serde_json::json!(["closed", "1"]),
        ] {
            assert_eq!(open(args.clone()), "Invalid window request", "{args}");
        }
        assert_eq!(
            open(serde_json::json!(["webview", "call", 1, {"method":"getURL","args":[]}])),
            "The main process is unavailable"
        );
        let picker = |picker| {
            dispatch(
                &binding,
                page.ctx,
                "dom.filePicker",
                serde_json::json!([picker]),
            )
            .is_ok()
        };
        assert!(!picker(
            serde_json::json!({"id":"1);evil(","multiple":false,"directory":false,"accept":""})
        ));
        assert!(picker(
            serde_json::json!({"id":1,"multiple":false,"directory":false,"accept":""})
        ));
        let popup = |items| {
            dispatch(
                &binding,
                page.ctx,
                "dom.selectPopup",
                serde_json::json!([{"id":1,"x":0,"y":0,"width":9,"selected":-1,"items":items}]),
            )
            .is_ok()
        };
        assert!(!popup(serde_json::json!([["a", "0);evil("]])));
        assert!(popup(serde_json::json!([["a", 1], null])));
        for op in ["node.poll", "child.spawn", "pty.spawn"] {
            let error = dispatch(&binding, page.ctx, op, serde_json::json!([])).unwrap_err();
            assert!(
                error.to_string().contains("unavailable in page scripts"),
                "{op}"
            );
        }
    }
}
