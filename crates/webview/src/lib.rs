#[cfg(any(target_os = "windows", target_os = "linux"))]
mod wry;

use anyhow::{Context, Result, bail, ensure};
use raw_window_handle::HasWindowHandle;
use serde_json::Value;
#[cfg(target_os = "macos")]
use std::{
    cell::RefCell,
    collections::VecDeque,
    ffi::{CStr, CString, c_char, c_void},
    ptr::NonNull,
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
    let events = unsafe { &*context.cast::<RefCell<VecDeque<Value>>>() };
    let bytes = unsafe { CStr::from_ptr(message) }.to_bytes();
    if bytes.len() > 4 * 1024 * 1024 {
        return;
    }
    if let Ok(value) = serde_json::from_slice(bytes) {
        let mut events = events.borrow_mut();
        if events.len() == 4096 {
            events.pop_front();
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
    events: Box<RefCell<VecDeque<Value>>>,
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
        let mut events = Box::new(RefCell::new(VecDeque::new()));
        let options = CString::new(options.to_string())?;
        let handle = unsafe {
            apps_browser_create(
                parent,
                options.as_ptr(),
                receive,
                (&mut *events as *mut RefCell<VecDeque<Value>>).cast(),
            )
        };
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
            unsafe { apps_browser_attach(self.handle.as_ptr(), handle.ns_view.as_ptr()) };
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
        unsafe {
            apps_browser_bounds(
                self.handle.as_ptr(),
                bounds[0],
                bounds[1],
                bounds[2],
                bounds[3],
            );
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        self.backend.set_bounds(bounds)?;
        Ok(())
    }

    pub fn detach(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        unsafe {
            apps_browser_attach(self.handle.as_ptr(), std::ptr::null_mut());
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
            let pointer = unsafe {
                apps_browser_command(self.handle.as_ptr(), operation.as_ptr(), arguments.as_ptr())
            };
            ensure!(!pointer.is_null(), "Browser operation returned no response");
            let result =
                serde_json::from_slice::<Value>(unsafe { CStr::from_ptr(pointer) }.to_bytes());
            unsafe { apps_browser_free(pointer) };
            let value = result?;
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
            self.events.borrow_mut().drain(..).collect()
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
        unsafe {
            apps_browser_destroy(self.handle.as_ptr());
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
        let mut queue = Box::new(RefCell::new(VecDeque::<Value>::new()));
        let context = (&mut *queue as *mut RefCell<VecDeque<Value>>).cast();
        let message = CString::new(r#"{"type":"message","data":{"text":"你好"}}"#).unwrap();
        for _ in 0..4100 {
            unsafe { receive(context, message.as_ptr()) };
        }
        assert_eq!(queue.borrow().len(), 4096);
        assert_eq!(queue.borrow()[0]["data"]["text"], "你好");
    }
}
