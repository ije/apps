#[cfg(any(target_os = "windows", target_os = "linux"))]
mod wry;

use anyhow::{Context, Result, bail, ensure};
use raw_window_handle::HasWindowHandle;
use serde_json::Value;
#[cfg(target_os = "macos")]
use std::{
    collections::VecDeque,
    ffi::{CStr, CString, c_char, c_void},
    ptr::NonNull,
    sync::{Mutex, OnceLock, PoisonError},
};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn apps_browser_create(
        parent: *mut c_void,
        options: *const c_char,
        callback: unsafe extern "C" fn(*mut c_void, *const c_char),
        context: *mut c_void,
    ) -> *mut c_void;
    fn apps_browser_destroy(handle: *mut c_void);
    fn apps_browser_attach(handle: *mut c_void, parent: *mut c_void);
    fn apps_browser_bounds(handle: *mut c_void, x: f64, y: f64, width: f64, height: f64);
    fn apps_browser_command(
        handle: *mut c_void,
        operation: *const c_char,
        arguments: *const c_char,
    ) -> *mut c_char;
    fn apps_browser_free(text: *mut c_char);
    fn pthread_main_np() -> std::ffi::c_int;
}

type MainSync = fn(&mut (dyn FnMut() + Send));
type MainPost = fn(Box<dyn FnOnce() + Send>);

#[cfg(target_os = "macos")]
static MAIN_THREAD: OnceLock<(MainSync, MainPost)> = OnceLock::new();

/// Sets how browsers reach the main thread, which owns the native web views.
pub fn on_main_thread(sync: MainSync, post: MainPost) {
    #[cfg(target_os = "macos")]
    let _ = MAIN_THREAD.set((sync, post));
    #[cfg(not(target_os = "macos"))]
    let _ = (sync, post);
}

#[cfg(target_os = "macos")]
fn sync_main<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    let Some((sync, _)) = MAIN_THREAD.get() else {
        return f();
    };
    let (mut f, mut result) = (Some(f), None);
    sync(&mut || result = f.take().map(|f| f()));
    result.expect("The main thread dropped a browser call")
}

/// A pointer that the main thread uses for the thread that asked.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct Native(*mut c_void);
// SAFETY: only the main thread reads through it.
#[cfg(target_os = "macos")]
unsafe impl Send for Native {}
#[cfg(target_os = "macos")]
impl Native {
    fn get(self) -> *mut c_void {
        self.0
    }
}

static WAKE: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

/// Sets the callback that wakes the app loop when a browser queues an event.
pub fn on_event(wake: impl Fn() + Send + Sync + 'static) {
    let _ = WAKE.set(Box::new(wake));
}

pub(crate) fn wake() {
    if let Some(wake) = WAKE.get() {
        wake();
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn receive(context: *mut c_void, message: *const c_char) {
    if context.is_null() || message.is_null() {
        return;
    }
    let events = unsafe { &*context.cast::<Mutex<VecDeque<Value>>>() };
    let bytes = unsafe { CStr::from_ptr(message) }.to_bytes();
    if bytes.len() > 4 * 1024 * 1024 {
        return;
    }
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        let mut events = events.lock().unwrap_or_else(PoisonError::into_inner);
        // WARNING: only page events can flood the queue. Results and navigation events always queue, or their callers never settle.
        if events.len() >= 4096 && matches!(value["type"].as_str(), Some("message" | "window-open"))
        {
            return;
        }
        events.push_back(value);
        drop(events);
        wake();
    }
}

pub struct Browser {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    backend: wry::Browser,
    #[cfg(target_os = "macos")]
    handle: NonNull<c_void>,
    #[cfg(target_os = "macos")]
    events: Box<Mutex<VecDeque<Value>>>,
}

impl Browser {
    pub fn new(window: &impl HasWindowHandle, options: &Value) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            use raw_window_handle::RawWindowHandle;
            let RawWindowHandle::AppKit(handle) = window.window_handle()?.as_raw() else {
                bail!("Expected an AppKit window");
            };
            Self::create(handle.ns_view.as_ptr(), options)
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            Ok(Self {
                backend: wry::Browser::new(Some(window.window_handle()?.as_raw()), options)?,
            })
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = (window, options);
            bail!("Embedded browsers require macOS, Windows, or Linux");
        }
    }

    pub fn detached(options: &Value) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Self::create(std::ptr::null_mut(), options)
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            Ok(Self {
                backend: wry::Browser::new(None, options)?,
            })
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = options;
            bail!("Embedded browsers require macOS, Windows, or Linux");
        }
    }

    #[cfg(target_os = "macos")]
    fn create(parent: *mut c_void, options: &Value) -> Result<Self> {
        let mut events = Box::new(Mutex::new(VecDeque::new()));
        let options = CString::new(options.to_string())?;
        let (parent, context) = (
            Native(parent),
            Native((&mut *events as *mut Mutex<VecDeque<Value>>).cast()),
        );
        let handle = sync_main(move || {
            Native(unsafe {
                apps_browser_create(parent.get(), options.as_ptr(), receive, context.get())
            })
        })
        .get();
        Ok(Self {
            handle: NonNull::new(handle)
                .context("WebKit requires a running native app on the main thread")?,
            events,
        })
    }

    pub fn attach(&self, window: &impl HasWindowHandle) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            use raw_window_handle::RawWindowHandle;
            let RawWindowHandle::AppKit(handle) = window.window_handle()?.as_raw() else {
                bail!("Expected an AppKit window");
            };
            let (browser, parent) = (
                Native(self.handle.as_ptr()),
                Native(handle.ns_view.as_ptr()),
            );
            sync_main(move || unsafe { apps_browser_attach(browser.get(), parent.get()) });
            Ok(())
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            self.backend.attach(window.window_handle()?.as_raw())
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = window;
            bail!("Embedded browsers require macOS, Windows, or Linux");
        }
    }

    pub fn set_bounds(&self, bounds: [f64; 4]) -> Result<()> {
        ensure!(
            bounds.iter().all(|v| v.is_finite()) && bounds[2] >= 0. && bounds[3] >= 0.,
            "Invalid browser bounds"
        );
        #[cfg(target_os = "macos")]
        {
            let browser = Native(self.handle.as_ptr());
            let set = move || unsafe {
                apps_browser_bounds(browser.get(), bounds[0], bounds[1], bounds[2], bounds[3]);
            };
            // WARNING: a page calls this while it paints, so it never waits for the main thread. Its
            // `Drop` waits for the main thread, so this runs first. On the main thread `Drop` runs at
            // once, so this must too.
            match MAIN_THREAD
                .get()
                .filter(|_| unsafe { pthread_main_np() } == 0)
            {
                Some((_, post)) => post(Box::new(set)),
                None => set(),
            }
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        self.backend.set_bounds(bounds)?;
        Ok(())
    }

    pub fn detach(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let browser = Native(self.handle.as_ptr());
            sync_main(move || unsafe { apps_browser_attach(browser.get(), std::ptr::null_mut()) });
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        self.backend.detach()?;
        Ok(())
    }

    pub fn command(&self, operation: &str, arguments: &Value) -> Result<Value> {
        match operation {
            "loadURL" => {
                ensure!(arguments.is_string(), "Expected browser URL");
            }
            "loadHTML" => {
                ensure!(
                    arguments["html"].is_string()
                        && arguments.get("baseURL").is_none_or(Value::is_string),
                    "Expected HTML and optional base URL"
                );
            }
            "evaluate" => {
                ensure!(
                    arguments["id"].is_u64() && arguments["script"].is_string(),
                    "Expected evaluation id and script"
                );
            }
            "cookies.get" | "cookies.remove" => {
                ensure!(arguments["id"].is_u64(), "Expected cookie request id");
                let filter = arguments["filter"]
                    .as_object()
                    .context("Expected cookie filter")?;
                for name in ["name", "url", "domain", "path"] {
                    ensure!(
                        filter.get(name).is_none_or(Value::is_string),
                        "Expected cookie filter text: {name}"
                    );
                }
            }
            "cookies.set" => {
                ensure!(arguments["id"].is_u64(), "Expected cookie request id");
                let details = &arguments["details"];
                for name in ["name", "value", "url"] {
                    ensure!(
                        details[name].is_string(),
                        "Expected cookie property: {name}"
                    );
                }
                let url = url::Url::parse(details["url"].as_str().unwrap())?;
                ensure!(
                    matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
                    "Cookies require an HTTP URL"
                );
                for name in ["domain", "path"] {
                    ensure!(
                        details.get(name).is_none_or(Value::is_string),
                        "Expected cookie text: {name}"
                    );
                }
                for name in ["secure", "httpOnly"] {
                    ensure!(
                        details.get(name).is_none_or(Value::is_boolean),
                        "Expected cookie boolean: {name}"
                    );
                }
                ensure!(
                    details
                        .get("expirationDate")
                        .is_none_or(|v| v.as_f64().is_some_and(f64::is_finite)),
                    "Invalid cookie expiration"
                );
            }
            "clearStorage" => {
                ensure!(arguments["id"].is_u64(), "Expected storage request id");
            }
            "capture" => {
                ensure!(arguments["id"].is_u64(), "Expected capture id");
                if let Some(rect) = arguments.get("rect") {
                    for name in ["x", "y", "width", "height"] {
                        ensure!(
                            rect[name].as_f64().is_some_and(|v| v.is_finite()
                                && (!matches!(name, "width" | "height") || v > 0.)),
                            "Invalid capture bounds"
                        );
                    }
                }
            }
            "zoom" => {
                ensure!(
                    arguments.as_f64().is_some_and(|v| (0.25..=5.).contains(&v)),
                    "Zoom must be between 0.25 and 5"
                );
            }
            "visible" => ensure!(arguments.is_boolean(), "Expected visibility boolean"),
            "appearance" => ensure!(arguments.is_string(), "Expected appearance string"),
            "decide" => ensure!(
                arguments["id"].is_u64() && arguments["allow"].is_boolean(),
                "Expected navigation decision"
            ),
            "back" | "forward" | "reload" | "stop" | "focus" | "inspect" | "state" => {}
            _ => bail!("Unknown browser command: {operation}"),
        }
        #[cfg(target_os = "macos")]
        {
            let operation = CString::new(operation)?;
            let arguments = CString::new(arguments.to_string())?;
            let browser = Native(self.handle.as_ptr());
            let value = sync_main(move || -> Result<Value> {
                let pointer = unsafe {
                    apps_browser_command(browser.get(), operation.as_ptr(), arguments.as_ptr())
                };
                ensure!(!pointer.is_null(), "Browser operation returned no response");
                let result =
                    serde_json::from_slice::<Value>(unsafe { CStr::from_ptr(pointer) }.to_bytes());
                unsafe { apps_browser_free(pointer) };
                Ok(result?)
            })?;
            if let Some(error) = value["error"].as_str() {
                bail!("{error}");
            }
            Ok(value["value"].clone())
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            self.backend.command(operation, arguments)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = (operation, arguments);
            bail!("Embedded browsers require macOS, Windows, or Linux");
        }
    }

    pub fn load_url(&self, url: &str) -> Result<()> {
        self.command("loadURL", &Value::String(url.into()))
            .map(|_| ())
    }
    pub fn load_html(&self, html: &str, base_url: &str) -> Result<()> {
        self.command(
            "loadHTML",
            &serde_json::json!({"html":html,"baseURL":base_url}),
        )
        .map(|_| ())
    }
    pub fn evaluate(&self, id: u64, script: &str) -> Result<()> {
        self.command("evaluate", &serde_json::json!({"id":id,"script":script}))
            .map(|_| ())
    }
    pub fn inspect(&self) -> Result<()> {
        self.command("inspect", &Value::Null).map(|_| ())
    }
    pub fn poll(&self) -> Vec<Value> {
        #[cfg(target_os = "macos")]
        {
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .drain(..)
                .collect()
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            self.backend.poll()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Vec::new()
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let browser = Native(self.handle.as_ptr());
            sync_main(move || unsafe { apps_browser_destroy(browser.get()) });
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn invalid_bounds_fail_before_native_dispatch() {
        let browser = std::mem::ManuallyDrop::new(Browser {
            handle: NonNull::dangling(),
            events: Box::default(),
        });
        assert!(browser.set_bounds([0., 0., -1., 10.]).is_err());
        assert!(browser.set_bounds([f64::NAN, 0., 1., 1.]).is_err());
    }

    #[test]
    fn callback_preserves_message_data_and_bounds_queue() {
        let mut queue = Box::new(Mutex::new(VecDeque::<Value>::new()));
        let context = (&mut *queue as *mut Mutex<VecDeque<Value>>).cast();
        let message = CString::new(r#"{"type":"message","data":{"text":"你好"}}"#).unwrap();
        for _ in 0..4100 {
            unsafe { receive(context, message.as_ptr()) };
        }
        assert_eq!(queue.lock().unwrap().len(), 4096);
        assert_eq!(queue.lock().unwrap()[0]["data"]["text"], "你好");
        let result = CString::new(r#"{"type":"evaluation","id":7,"value":1}"#).unwrap();
        unsafe { receive(context, result.as_ptr()) };
        assert_eq!(queue.lock().unwrap().len(), 4097);
        assert_eq!(queue.lock().unwrap()[4096]["id"], 7);
    }
}
