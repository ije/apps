pub mod aot;
pub(crate) mod context;
pub(crate) mod encoding;
pub(crate) mod payload;

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

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Role {
  Main,
  Preload(u64),
  Renderer(u64),
}

#[derive(Clone, Default)]
pub struct Waker(Option<std::sync::Arc<dyn Fn() + Send + Sync>>);

thread_local! { static WAKER: std::cell::RefCell<Waker> = std::cell::RefCell::default(); }

thread_local! {
    pub(crate) static IDLE: std::cell::Cell<(Option<std::time::Instant>, Option<std::time::Instant>, std::time::Duration)> = std::cell::Cell::default();
}

pub(crate) fn idle(waiting: bool) {
  IDLE.with(|idle| {
    let (start, since, total) = idle.get();
    let now = std::time::Instant::now();
    idle.set(if waiting {
      (start.or(Some(now)), Some(now), total)
    } else {
      (
        start,
        None,
        total + since.map_or_else(Default::default, |since| now - since),
      )
    });
  });
}

impl Waker {
  pub fn install(wake: impl Fn() + Send + Sync + 'static) {
    WAKER.set(Self(Some(std::sync::Arc::new(wake))));
  }

  pub(crate) fn current() -> Self {
    WAKER.with_borrow(Clone::clone)
  }

  pub(crate) fn wake(&self) {
    if let Some(wake) = &self.0 {
      wake();
    }
  }
}

pub(crate) struct WakeOnDrop(pub(crate) Waker);

impl Drop for WakeOnDrop {
  fn drop(&mut self) {
    self.0.wake();
  }
}

pub(crate) type Source = (&'static str, fn() -> &'static str);

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
const PAGE_SCRIPTS: [Source; 3] = [
  ("apps:typed-om", || js!("typed_om.js")),
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
pub(crate) type Callback = unsafe extern "C" fn(Ptr, Ptr, Ptr, usize, *const Ptr, *mut Ptr) -> Ptr;

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
  group: Ptr,
  group_release: unsafe extern "C" fn(Ptr),
  garbage_collect: unsafe extern "C" fn(Ptr),
  shrink: unsafe extern "C" fn(Ptr),
  drain: unsafe extern "C" fn(Ptr),
  evaluate_latin1:
    unsafe extern "C" fn(Ptr, *const c_char, usize, Ptr, Ptr, *const u8, usize, *mut Ptr) -> Ptr,
  create_bytecode: unsafe extern "C" fn(Ptr, *const c_char, usize, Ptr, *mut usize) -> *mut u8,
  busy: std::cell::Cell<std::time::Duration>,
  busy_mark: std::cell::Cell<(std::time::Instant, std::time::Duration)>,
  burst: std::cell::Cell<bool>,
  quiet: std::cell::Cell<u32>,
  shrunk: std::cell::Cell<std::time::Instant>,
  ticks: std::cell::Cell<(std::time::Duration, u32)>,
  inspectable: unsafe extern "C" fn(Ptr, bool),
  pub(crate) extra_memory: unsafe extern "C" fn(Ptr, usize),
  // An older library has no such functions. Buffers are then copied, and shared buffers can't be sent.
  contents: payload::Contents,
  terminate: unsafe extern "C" fn(Ptr),
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
  pub(crate) set: unsafe extern "C" fn(Ptr, Ptr, Ptr, Ptr, u32, *mut Ptr),
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
  pub(crate) get_index: unsafe extern "C" fn(Ptr, Ptr, u32, *mut Ptr) -> Ptr,
  pub(crate) set_index: unsafe extern "C" fn(Ptr, Ptr, u32, Ptr, *mut Ptr),
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
}

type Limit = unsafe extern "C" fn(Ptr, f64, Option<unsafe extern "C" fn(Ptr, Ptr) -> bool>, Ptr);
type InspectorMessage = unsafe extern "C" fn(Ptr, *const c_char, usize);
type InspectorPause = unsafe extern "C" fn(Ptr, *mut bool);

struct Inspector {
  connect: unsafe extern "C" fn(Ptr, InspectorMessage, InspectorPause, Ptr) -> Ptr,
  dispatch: unsafe extern "C" fn(Ptr, *const c_char, usize),
  disconnect: unsafe extern "C" fn(Ptr, Ptr),
}

// Every VM loads the same library, so one table serves all of them.
static INSPECTOR: std::sync::OnceLock<Option<Inspector>> = std::sync::OnceLock::new();

fn inspector() -> Result<&'static Inspector> {
  INSPECTOR
    .get()
    .and_then(Option::as_ref)
    .context("This JavaScriptCore has no inspector. Rebuild it with scripts/build-jsc.sh")
}

// Returns the connection, which `inspector_disconnect` closes.
pub(crate) fn inspector_connect(ctx: usize) -> Result<Ptr> {
  let inspector = inspector()?;
  Ok(unsafe { (inspector.connect)(ctx as Ptr, inspector_message, inspector_pause, ctx as Ptr) })
}

pub(crate) fn inspector_dispatch(ctx: usize, message: &str) -> Result<()> {
  let inspector = inspector()?;
  unsafe { (inspector.dispatch)(ctx as Ptr, message.as_ptr().cast(), message.len()) };
  Ok(())
}

pub(crate) fn inspector_disconnect(ctx: Ptr, connection: Ptr) {
  if let Some(inspector) = INSPECTOR.get().and_then(Option::as_ref) {
    unsafe { (inspector.disconnect)(ctx, connection) };
  }
}

pub fn library_name(platform: &str) -> Result<&'static str> {
  match platform {
    "macos" => Ok("libJavaScriptCore.dylib"),
    "linux" => Ok("libJavaScriptCore.so"),
    "windows" => Ok("JavaScriptCore.dll"),
    _ => bail!("Unsupported JavaScriptCore platform: {platform}"),
  }
}

// Keep module code outside the loader scope to hide internals such as `compile`. Expose only `$appsDefine`.
pub fn script_source(files: &BBTreeMap<String, String>, execute: bool) -> String {
  let app = files.get("app.js").map_or("", String::as_str);
  if execute && app.starts_with(aot::AOT_PREFIX) {
    return app.to_owned();
  }
  let modules = js!("modules.js");
  let mut source = String::with_capacity(modules.len() + app.len() + 64);
  source.push_str("(()=>{");
  source.push_str(modules);
  source.push_str("})()(($appsDefine)=>{\n");
  source.push_str(app);
  source.push_str(if execute { "\n},true);" } else { "\n},false);" });
  source
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

// JSC threads outlive every handle, so the library must never unmap. On Linux the runtime also links
// WebKitGTK, which loads the system JavaScriptCore with the same C++ symbols. Deep binding makes Bun's
// library resolve its own symbols first, not the system copy's.
#[cfg(unix)]
/// # Safety
/// The initializers of the library run.
pub unsafe fn open_library(path: &Path) -> std::result::Result<Library, libloading::Error> {
  use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};
  #[cfg(all(target_os = "linux", target_env = "gnu"))]
  let flags = RTLD_NOW | RTLD_LOCAL | libc::RTLD_NODELETE | libc::RTLD_DEEPBIND;
  #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
  let flags = RTLD_NOW | RTLD_LOCAL | libc::RTLD_NODELETE;
  unsafe { Library::open(Some(path), flags) }.map(Into::into)
}

#[cfg(not(unix))]
/// # Safety
/// The initializers of the library run.
pub unsafe fn open_library(path: &Path) -> std::result::Result<Library, libloading::Error> {
  unsafe { Library::new(path) }
}

impl JavaScriptCore {
  pub fn load() -> Result<Rc<Self>> {
    // One library for all engines, never unloaded, so tables of its functions stay valid.
    static LIBRARY: std::sync::OnceLock<(Library, PathBuf)> = std::sync::OnceLock::new();
    let (library, path) = match LIBRARY.get() {
      Some(loaded) => loaded,
      None => {
        let path = library_path(&std::env::current_exe()?, std::env::consts::OS)?;
        let library = unsafe { open_library(&path) }
          .with_context(|| format!("Cannot load Bun JavaScriptCore from {}", path.display()))?;
        let _ = LIBRARY.set((library, path));
        LIBRARY.get().unwrap()
      }
    };
    let image = if cfg!(target_os = "macos") {
      path.with_file_name("../Resources/app.dylib")
    } else {
      path.with_file_name(aot::image_name(std::env::consts::OS))
    };
    // The AOT runtime library is built without the inspector.
    INSPECTOR.get_or_init(|| unsafe {
      Some(Inspector {
        connect: *library.get(b"JSGlobalContextConnectInspector\0").ok()?,
        dispatch: *library
          .get(b"JSGlobalContextDispatchInspectorMessage\0")
          .ok()?,
        disconnect: *library.get(b"JSGlobalContextDisconnectInspector\0").ok()?,
      })
    });
    unsafe {
      let group_create: unsafe extern "C" fn() -> Ptr = *library.get(b"JSContextGroupCreate\0")?;
      // Set JSC options before the first group and before other threads start. Only then is set_var
      // sound.
      // JSC reads the environment of the C runtime, which `std::env` doesn't change on Windows.
      let set_option = |name: &std::ffi::CStr, value: &std::ffi::CStr| {
        #[cfg(windows)]
        {
          unsafe extern "C" {
            fn _putenv_s(name: *const std::ffi::c_char, value: *const std::ffi::c_char) -> i32;
          }
          _putenv_s(name.as_ptr(), value.as_ptr());
        }
        #[cfg(unix)]
        if value.is_empty() {
          libc::unsetenv(name.as_ptr());
        } else {
          libc::setenv(name.as_ptr(), value.as_ptr(), 1);
        }
      };
      static OPTIONS: std::sync::Once = std::sync::Once::new();
      let mut group = None;
      OPTIONS.call_once(|| {
        let options = [
          (c"JSC_alwaysUseSharedArrayBuffer", c"1"),
          // Limit heap growth to about 2x live size instead of 4x on machines with at least 16 GB.
          (c"JSC_heapGrowthMaxIncrease", c"1"),
          // Sweep after each collection: JSC's timer never fires on the generic event loop, leaving
          // dead cells resident.
          (c"JSC_sweepSynchronously", c"1"),
          // In JSC's cell cage every block the heap holds stays resident. AOT images use wide
          // references, which need no cage.
          (c"JSC_markedBlockCageSizeInMB", c"0"),
          // Start the baseline JIT 4x later, so code that runs only at startup keeps no machine code.
          // This saves about 4 MB.
          (c"JSC_thresholdForJITAfterWarmUp", c"2000"),
          (c"JSC_thresholdForJITSoon", c"400"),
        ];
        for (name, value) in options {
          set_option(name, value);
        }
        let _ = aot::load(library, &image);
        group = Some(group_create());
        for (name, _) in options {
          set_option(name, c"");
        }
      });
      Ok(Rc::new(Self {
        aot: aot::load(library, &image)?,
        group: group.unwrap_or_else(|| group_create()),
        inspectable: *library.get(b"JSGlobalContextSetInspectable\0")?,
        extra_memory: *library.get(b"JSReportExtraMemoryCost\0")?,
        terminate: *library.get(b"JSContextGroupTerminate\0")?,
        contents: payload::Contents {
          take: *library.get(b"JSObjectTakeContents\0")?,
          bytes: *library.get(b"JSContentsGetBytes\0")?,
          make: *library.get(b"JSObjectMakeWithContents\0")?,
          release: *library.get(b"JSContentsRelease\0")?,
        },
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
        ticks: Default::default(),
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
        rejection_callback: *library.get(b"JSGlobalContextSetUnhandledRejectionCallback\0")?,
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
      self
        .chars(ctx, value)
        .map_or_else(String::new, |chars| self.utf8_string(chars))
    }
  }

  // JSC keeps pointing at `source` and `bytecode`, so both must outlive the VM.
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

  pub(crate) fn runtime_module(&self, url: &str) -> Option<u32> {
    self.aot?.runtime(url)
  }

  // The AOT module of an app script that the build compiled.
  pub(crate) fn compiled_module(&self, source: &[u8]) -> Result<Option<u32>> {
    self.aot.map_or(Ok(None), |aot| aot.find(source))
  }

  // Call once per context, after the host installed its globals and before compiled code runs.
  pub(crate) fn attach_aot(&self, ctx: Ptr) {
    if let Some(aot) = self.aot {
      aot.attach(ctx);
    }
  }

  pub(crate) fn run_aot(&self, ctx: Ptr, module: u32, this: Ptr, exception: *mut Ptr) -> Ptr {
    let Some(aot) = self.aot else {
      return ptr::null_mut();
    };
    let function = aot.function(ctx, module, exception);
    if function.is_null() {
      return ptr::null_mut();
    }
    // entries read `process`, `require` and such from this argument. See the CLI's aot.rs.
    unsafe { (self.invoke)(ctx, function, this, 1, &(self.global)(ctx), exception) }
  }

  // Adds the time of a script run to the busy time.
  pub(crate) fn timed<R>(&self, run: impl FnOnce() -> R) -> R {
    let start = std::time::Instant::now();
    let result = run();
    self.busy.set(self.busy.get() + start.elapsed());
    result
  }

  // Counts watchdog ticks per busy value: JSC resets its budget per run but does not identify each tick's run.
  pub(crate) fn over_limit(&self) -> bool {
    let busy = self.busy.get();
    let ticks = match self.ticks.get() {
      (last, ticks) if last == busy => ticks + 1,
      _ => 1,
    };
    self.ticks.set((busy, ticks));
    let over = f64::from(ticks) * PAGE_TICK >= PAGE_LIMIT as f64;
    if over {
      eprintln!("apps: stopped a page script that ran for more than {PAGE_LIMIT} seconds");
    }
    over
  }

  pub(crate) fn terminator(&self) -> Terminator {
    Terminator(self.group as usize, self.terminate)
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
    self
      .burst
      .set(self.burst.get() || ran >= Duration::from_millis(100));
    if ran >= Duration::from_millis(20) {
      self.quiet.set(0);
      return ran < Duration::from_millis(100)
        && now.duration_since(self.shrunk.get()) >= Duration::from_secs(300);
    }
    // Short runs like clicks recompile code and leave garbage. Without a burst, shrink after 30 s of quiet.
    self.quiet.set(self.quiet.get() + 1);
    self.quiet.get() >= if self.burst.get() { 5 } else { 30 }
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
  let data =
    unsafe { (api.create_bytecode)(ctx, source.as_ptr().cast(), source.len(), name, &mut size) };
  unsafe { (api.string_release)(name) };
  if data.is_null() {
    &[]
  } else {
    unsafe { std::slice::from_raw_parts(data, size) }
  }
}

// Stops the scripts of a VM from another thread.
#[derive(Clone, Copy)]
pub(crate) struct Terminator(usize, unsafe extern "C" fn(Ptr));

impl Terminator {
  pub(crate) fn terminate(self) {
    unsafe { (self.1)(self.0 as Ptr) };
  }
}

impl JavaScriptCore {
  pub(crate) fn prebuild(&self, webgpu: bool, node: bool) {
    if self.aot.is_some() {
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
}

// The bytecode of a runtime script, or nothing yet. The first use builds it on another thread.
fn runtime_bytecode(url: &'static str, source: &'static str) -> &'static [u8] {
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
  bytecode
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
  webgpu: bool,
  app: &[(&'a str, &'a str)],
) -> Result<Vec<(&'a str, &'static [u8])>> {
  SCRIPTS.lock().unwrap().building = true;
  let api = JavaScriptCore::load()?;
  api.prebuild(webgpu, true);
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

type Registered = (Rc<JavaScriptCore>, Rc<dyn std::any::Any>);

thread_local! {
    // The engine of each context of this thread, and the data of the host that made the context, for callbacks
    // that get only the context.
    static CONTEXTS: RefCell<rustc_hash::FxHashMap<usize, Registered>> = RefCell::default();
    static INSPECTOR_MESSAGES: RefCell<Vec<(usize, String)>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn register(ctx: Ptr, api: Rc<JavaScriptCore>, data: Rc<dyn std::any::Any>) {
  CONTEXTS.with_borrow_mut(|contexts| contexts.insert(ctx as usize, (api, data)));
}

pub(crate) fn forget(ctx: Ptr) -> Option<Rc<dyn std::any::Any>> {
  CONTEXTS.with_borrow_mut(|contexts| contexts.remove(&(ctx as usize)).map(|(_, data)| data))
}

pub(crate) fn engine(ctx: Ptr) -> Option<Rc<JavaScriptCore>> {
  CONTEXTS.with_borrow(|contexts| contexts.get(&(ctx as usize)).map(|(api, _)| api.clone()))
}

pub(crate) fn data<T: 'static>(ctx: Ptr) -> Option<Rc<T>> {
  let data = CONTEXTS.with(|contexts| {
    let contexts = contexts.try_borrow().ok()?;
    Some(contexts.get(&(ctx as usize))?.1.clone())
  })?;
  data.downcast().ok()
}

// The first context of this thread whose data is a `T` that `found` accepts.
pub(crate) fn find<T: 'static>(found: impl Fn(&T) -> bool) -> Option<(Ptr, Rc<T>)> {
  CONTEXTS.with_borrow(|contexts| {
    contexts.iter().find_map(|(&ctx, (_, data))| {
      data.downcast_ref::<T>().filter(|value| found(value))?;
      Some((ctx as Ptr, data.clone().downcast().ok()?))
    })
  })
}

// The contexts of this thread whose data is a `T`.
pub(crate) fn contexts<T: 'static>() -> Vec<(Ptr, Rc<T>)> {
  CONTEXTS.with_borrow(|contexts| {
    contexts
      .iter()
      .filter_map(|(&ctx, (_, data))| Some((ctx as Ptr, data.clone().downcast().ok()?)))
      .collect()
  })
}

pub(crate) fn inspector_messages() -> Vec<(usize, String)> {
  INSPECTOR_MESSAGES.take()
}

unsafe extern "C" fn inspector_message(ctx: Ptr, message: *const c_char, length: usize) {
  let message = unsafe { std::slice::from_raw_parts(message.cast::<u8>(), length) };
  let message = String::from_utf8_lossy(message).into_owned();
  INSPECTOR_MESSAGES.with_borrow_mut(|messages| messages.push((ctx as usize, message)));
}

type PauseHandler = unsafe extern "C" fn(usize, *mut bool);

static PAUSE_HANDLER: std::sync::OnceLock<PauseHandler> = std::sync::OnceLock::new();

// The handler runs while JSC is paused in a context. It must dispatch protocol messages until `done` is set.
pub(crate) fn set_pause_handler(handler: PauseHandler) {
  let _ = PAUSE_HANDLER.set(handler);
}

unsafe extern "C" fn inspector_pause(ctx: Ptr, done: *mut bool) {
  match PAUSE_HANDLER.get() {
    Some(handler) => unsafe { handler(ctx as usize, done) },
    None => drop(inspector_dispatch(
      ctx as usize,
      r#"{"id":0,"method":"Debugger.resume"}"#,
    )),
  }
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

const PAGE_LIMIT: u64 = 10;
const PAGE_TICK: f64 = 0.1;

impl JavaScriptCore {
  pub(crate) fn throw(&self, ctx: Ptr, exception: *mut Ptr, message: &str) {
    if !exception.is_null() {
      let message = self.string(message);
      unsafe {
        *exception = (self.make_string)(ctx, message);
        (self.string_release)(message);
      }
    }
  }

  pub(crate) fn call_json<T>(
    &self,
    ctx: Ptr,
    name: &str,
    arguments: &[&str],
    read: impl FnOnce(Ptr) -> T,
  ) -> Result<T> {
    let key = self.string(name);
    let mut exception = ptr::null_mut();
    let function = unsafe { (self.get)(ctx, (self.global)(ctx), key, &mut exception) };
    unsafe { (self.string_release)(key) };
    ensure!(
      exception.is_null()
        && unsafe { (self.value_type)(ctx, function) } == 5
        && unsafe { (self.is_function)(ctx, function) },
      "Missing JavaScript function: {name}"
    );
    let values: Vec<Ptr> = arguments
      .iter()
      .map(|argument| {
        let text = self.string(argument);
        let value = unsafe { (self.json_value)(ctx, text) };
        unsafe {
          (self.string_release)(text);
          // Values kept in a heap Vec are invisible to JSC's stack scan.
          (self.protect)(ctx, value);
        }
        value
      })
      .collect();
    let result = if values.iter().any(|value| value.is_null()) {
      Err(anyhow::anyhow!("Invalid JSON argument for {name}"))
    } else {
      let result = self.timed(|| unsafe {
        (self.invoke)(
          ctx,
          function,
          ptr::null_mut(),
          values.len(),
          values.as_ptr(),
          &mut exception,
        )
      });
      if exception.is_null() {
        Ok(read(result))
      } else {
        Err(anyhow::anyhow!("{}", self.text(ctx, exception)))
      }
    };
    for value in values.into_iter().filter(|value| !value.is_null()) {
      unsafe { (self.unprotect)(ctx, value) };
    }
    result
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn lone_surrogates_become_replacements_and_heap_text_is_not_in_place() {
    let mut json = r#"["a\ud83d","\\ud83d","\n\udfff","\u001f"]"#.to_owned();
    well_formed(&mut json);
    assert_eq!(json, r#"["a\ufffd","\\ud83d","\n\ufffd","\u001f"]"#);
    assert_eq!(
      serde_json::from_str::<Vec<String>>(&json).unwrap(),
      ["a\u{fffd}", "\\ud83d", "\n\u{fffd}", "\u{1f}"]
    );
    assert!(!in_place(b"heap"));
  }
}
