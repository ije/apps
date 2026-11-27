use anyhow::{Result, ensure};
use serde_json::Value;
use std::{
  ffi::{CStr, c_char, c_void},
  sync::OnceLock,
  time::Instant,
};

const ABI: u32 = 4;

// The developer tools library, with the panel page, the MCP server and the FPS HUD. Built apps ship without
// it. It is never unloaded: its threads run and the panel uses its texts until the app quits.
pub(crate) fn load() -> Result<&'static libloading::Library> {
  static LOADED: OnceLock<&libloading::Library> = OnceLock::new();
  if let Some(library) = LOADED.get() {
    return Ok(library);
  }
  let name = libloading::library_filename("apps_devtools");
  let library = crate::native::library::load(
    &name.to_string_lossy(),
    "developer tools",
    b"apps_devtools_abi\0",
    ABI,
    ". Build the workspace with cargo build --workspace.",
  )?;
  Ok(LOADED.get_or_init(|| library))
}

// The page of the panel, or the script that inspects a context for it.
pub(crate) fn text(agent: bool) -> Result<&'static str> {
  let panel: libloading::Symbol<extern "C" fn(bool, &mut usize) -> *const u8> =
    unsafe { load()?.get(b"apps_devtools_panel\0")? };
  let mut len = 0;
  let text = panel(agent, &mut len);
  Ok(std::str::from_utf8(unsafe {
    std::slice::from_raw_parts(text, len)
  })?)
}

// The reply, ok, JSON value or error text, and an optional PNG.
pub(crate) type Done = extern "C" fn(*mut c_void, bool, *const u8, usize, *const u8, usize);

// The functions that the MCP server of the library calls. Match `apps_devtools::mcp::Host`.
#[repr(C)]
pub(crate) struct Host {
  pub(crate) connect: extern "C" fn(usize) -> i32,
  pub(crate) inspect: extern "C" fn(usize, *const u8, usize) -> bool,
  pub(crate) messages:
    extern "C" fn(*mut c_void, extern "C" fn(*mut c_void, usize, *const u8, usize)),
  pub(crate) contexts: extern "C" fn(*mut c_void, extern "C" fn(*mut c_void, usize, u32, u64)),
  pub(crate) wake: extern "C" fn(u64),
  pub(crate) call:
    extern "C" fn(*mut c_void, *const u8, usize, *const u8, usize, *mut c_void, Done),
}

struct Library {
  install: extern "C" fn(u64),
  uninstall: extern "C" fn(),
  poll: extern "C" fn(*mut c_void) -> bool,
  logs: unsafe extern "C" fn(*const u8, usize),
  profiling: extern "C" fn() -> bool,
  frame: extern "C" fn(u64, u64),
}

static LIBRARY: OnceLock<Library> = OnceLock::new();

// Starts the MCP server of the library with the functions of `host`, which the library calls.
pub(crate) fn start(port: u16, host: &'static Host) -> Result<()> {
  let library = load()?;
  let (start, paused, functions) = unsafe {
    let start: extern "C" fn(u16, &'static Host) -> *const c_char =
      *library.get(b"apps_devtools_mcp_start\0")?;
    let paused: unsafe extern "C" fn(usize, *mut bool) =
      *library.get(b"apps_devtools_mcp_paused\0")?;
    let functions = Library {
      install: *library.get(b"apps_devtools_mcp_install\0")?,
      uninstall: *library.get(b"apps_devtools_mcp_uninstall\0")?,
      poll: *library.get(b"apps_devtools_mcp_poll\0")?,
      logs: *library.get(b"apps_devtools_mcp_logs\0")?,
      profiling: *library.get(b"apps_devtools_mcp_profiling\0")?,
      frame: *library.get(b"apps_devtools_mcp_frame\0")?,
    };
    (start, paused, functions)
  };
  let error = start(port, host);
  ensure!(
    error.is_null(),
    "{}",
    unsafe { CStr::from_ptr(error) }.to_string_lossy()
  );
  crate::jsc::set_pause_handler(paused);
  let _ = LIBRARY.set(functions);
  Ok(())
}

pub(crate) fn enabled() -> bool {
  LIBRARY.get().is_some()
}

pub(crate) fn install(key: u64) {
  if let Some(library) = LIBRARY.get() {
    (library.install)(key);
  }
}

pub(crate) fn uninstall() {
  if let Some(library) = LIBRARY.get() {
    (library.uninstall)();
  }
}

pub(crate) fn poll_main() -> bool {
  poll(std::ptr::null_mut())
}

// `app` is what the library gives back to the host's `call`, or null on the main thread.
pub(crate) fn poll(app: *mut c_void) -> bool {
  LIBRARY.get().is_some_and(|library| (library.poll)(app))
}

pub(crate) fn logs(logs: &[Value]) {
  if let Some(library) = LIBRARY.get()
    && let Ok(logs) = serde_json::to_vec(logs)
  {
    unsafe { (library.logs)(logs.as_ptr(), logs.len()) };
  }
}

pub(crate) fn profiling() -> bool {
  LIBRARY.get().is_some_and(|library| (library.profiling)())
}

pub(crate) fn frame(window: u64, started: Instant) {
  if let Some(library) = LIBRARY.get() {
    (library.frame)(window, started.elapsed().as_nanos() as u64);
  }
}
