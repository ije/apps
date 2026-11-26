// The system web view of Linux, from libapps_webview.so. It loads on the first browser, so apps without one
// never load GTK, WebKitGTK and GStreamer.
use anyhow::{Context, Result, anyhow, bail};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::Value;
use std::ffi::{CStr, CString, c_char, c_ulong, c_void};
use std::sync::OnceLock;

const ABI: u32 = 1;

type Text = *mut c_char;

struct Library {
    new: unsafe extern "C" fn(c_ulong, f64, *const c_char, *mut Text) -> *mut c_void,
    attach: unsafe extern "C" fn(*const c_void, c_ulong, f64) -> Text,
    set_bounds: unsafe extern "C" fn(*const c_void, f64, f64, f64, f64) -> Text,
    command: unsafe extern "C" fn(*const c_void, *const c_char, *const c_char) -> Text,
    poll: unsafe extern "C" fn(*const c_void) -> Text,
    free: unsafe extern "C" fn(Text),
    destroy: unsafe extern "C" fn(*mut c_void),
}

static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static LOADED: OnceLock<Result<Library, String>> = OnceLock::new();

extern "C" fn wake() {
    if let Some(wake) = WAKE.get() {
        wake();
    }
}

fn library() -> Result<&'static Library> {
    LOADED
        .get_or_init(|| {
            (|| -> Result<Library> {
                let library = super::library::load(
                    super::webview_library("linux").unwrap(),
                    "web view",
                    b"apps_webview_abi\0",
                    ABI,
                    ". Build the workspace first with cargo build --workspace --release.",
                )?;
                unsafe {
                    let on_event: libloading::Symbol<extern "C" fn(extern "C" fn())> =
                        library.get(b"apps_webview_on_event\0")?;
                    on_event(wake);
                    Ok(Library {
                        new: *library.get(b"apps_webview_new\0")?,
                        attach: *library.get(b"apps_webview_attach\0")?,
                        set_bounds: *library.get(b"apps_webview_set_bounds\0")?,
                        command: *library.get(b"apps_webview_command\0")?,
                        poll: *library.get(b"apps_webview_poll\0")?,
                        free: *library.get(b"apps_webview_free\0")?,
                        destroy: *library.get(b"apps_webview_destroy\0")?,
                    })
                }
            })()
            .map_err(|error| format!("{error:#}"))
        })
        .as_ref()
        .map_err(|error| anyhow!("{error}"))
}

// Takes the error text of the library.
fn checked(library: &Library, text: Text) -> Result<()> {
    if text.is_null() {
        return Ok(());
    }
    let error = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    unsafe { (library.free)(text) };
    bail!("{error}")
}

fn x11(window: &impl HasWindowHandle) -> Result<c_ulong> {
    match window.window_handle()?.as_raw() {
        RawWindowHandle::Xlib(handle) => Ok(handle.window),
        RawWindowHandle::Xcb(handle) => Ok(handle.window.get().into()),
        _ => bail!(
            "WebKitGTK embedding requires X11 or XWayland; on Wayland, install XWayland or start the app with WAYLAND_DISPLAY unset"
        ),
    }
}

pub fn on_main_thread(_: fn(&mut (dyn FnMut() + Send)), _: fn(Box<dyn FnOnce() + Send>)) {}

pub fn on_event(wake: impl Fn() + Send + Sync + 'static) {
    let _ = WAKE.set(Box::new(wake));
}

pub struct Browser {
    handle: *mut c_void,
    library: &'static Library,
}

impl Browser {
    pub fn new(window: &impl HasWindowHandle, scale: f64, options: &Value) -> Result<Self> {
        Self::create(x11(window)?, scale, options)
    }

    pub fn detached(options: &Value) -> Result<Self> {
        Self::create(0, 1., options)
    }

    fn create(window: c_ulong, scale: f64, options: &Value) -> Result<Self> {
        let library = library()?;
        let options = CString::new(options.to_string())?;
        let mut error = std::ptr::null_mut();
        let handle = unsafe { (library.new)(window, scale, options.as_ptr(), &mut error) };
        if handle.is_null() {
            checked(library, error)?;
            bail!("The system web view failed to start");
        }
        Ok(Self { handle, library })
    }

    pub fn attach(&self, window: &impl HasWindowHandle, scale: f64) -> Result<()> {
        let window = x11(window)?;
        checked(self.library, unsafe {
            (self.library.attach)(self.handle, window, scale)
        })
    }

    pub fn set_bounds(&self, bounds: [f64; 4]) -> Result<()> {
        let [x, y, width, height] = bounds;
        checked(self.library, unsafe {
            (self.library.set_bounds)(self.handle, x, y, width, height)
        })
    }

    pub fn dock(&self, _: f64) -> Result<f64> {
        bail!("Docked browsers require macOS");
    }

    pub fn detach(&self) -> Result<()> {
        checked(self.library, unsafe {
            (self.library.attach)(self.handle, 0, 1.)
        })
    }

    pub fn command(&self, operation: &str, arguments: &Value) -> Result<()> {
        let operation = CString::new(operation)?;
        let arguments = CString::new(arguments.to_string())?;
        checked(self.library, unsafe {
            (self.library.command)(self.handle, operation.as_ptr(), arguments.as_ptr())
        })
    }

    pub fn load_url(&self, url: &str) -> Result<()> {
        self.command("loadURL", &Value::String(url.into()))
    }

    pub fn load_html(&self, html: &str, base_url: &str) -> Result<()> {
        self.command(
            "loadHTML",
            &serde_json::json!({"html":html,"baseURL":base_url}),
        )
    }

    pub fn evaluate(&self, id: u64, script: &str) -> Result<()> {
        self.command("evaluate", &serde_json::json!({"id":id,"script":script}))
    }

    pub fn poll(&self) -> Vec<Value> {
        let text = unsafe { (self.library.poll)(self.handle) };
        if text.is_null() {
            return Vec::new();
        }
        let events = serde_json::from_slice(unsafe { CStr::from_ptr(text) }.to_bytes())
            .context("Invalid web view events");
        unsafe { (self.library.free)(text) };
        events.unwrap_or_default()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        unsafe { (self.library.destroy)(self.handle) };
    }
}
