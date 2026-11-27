// The C ABI of libapps_webview.so. The Linux runtime loads it on the first system web view, so apps without
// web views never load GTK and WebKitGTK.
use crate::{Browser, wry};
use raw_window_handle::{RawWindowHandle, XlibWindowHandle};
use serde_json::Value;
use std::ffi::{CStr, CString, c_char, c_ulong};

pub const ABI: u32 = 1;

#[unsafe(no_mangle)]
pub extern "C" fn apps_webview_abi() -> u32 {
  ABI
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_webview_on_event(wake: extern "C" fn()) {
  crate::on_event(move || wake());
}

fn text(text: String) -> *mut c_char {
  CString::new(text.replace('\0', " ")).unwrap().into_raw()
}

fn result(result: anyhow::Result<()>) -> *mut c_char {
  result.map_or_else(
    |error| text(format!("{error:#}")),
    |()| std::ptr::null_mut(),
  )
}

// A window of 0 makes a browser in its own window.
fn parent(window: c_ulong) -> Option<RawWindowHandle> {
  (window != 0).then(|| RawWindowHandle::Xlib(XlibWindowHandle::new(window)))
}

/// # Safety
/// `options` is a JSON C string, and `error` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webview_new(
  window: c_ulong,
  scale: f64,
  options: *const c_char,
  error: *mut *mut c_char,
) -> *mut Browser {
  let made = (|| -> anyhow::Result<Browser> {
    let options: Value = serde_json::from_slice(unsafe { CStr::from_ptr(options) }.to_bytes())?;
    Ok(Browser {
      backend: wry::Browser::new(parent(window), scale, &options)?,
    })
  })();
  match made {
    Ok(browser) => Box::into_raw(Box::new(browser)),
    Err(failure) => {
      unsafe { *error = text(format!("{failure:#}")) };
      std::ptr::null_mut()
    }
  }
}

/// # Safety
/// `browser` comes from `apps_webview_new` and is not destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webview_attach(
  browser: *const Browser,
  window: c_ulong,
  scale: f64,
) -> *mut c_char {
  let browser = unsafe { &*browser };
  result(match parent(window) {
    Some(parent) => browser.backend.window.attach(parent, scale),
    None => browser.backend.window.detach(),
  })
}

/// # Safety
/// As in `apps_webview_attach`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webview_set_bounds(
  browser: *const Browser,
  x: f64,
  y: f64,
  width: f64,
  height: f64,
) -> *mut c_char {
  result(unsafe { &*browser }.set_bounds([x, y, width, height]))
}

/// # Safety
/// As in `apps_webview_attach`, and both texts are C strings, the arguments in JSON.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webview_command(
  browser: *const Browser,
  operation: *const c_char,
  arguments: *const c_char,
) -> *mut c_char {
  result((|| {
    let operation = unsafe { CStr::from_ptr(operation) }.to_str()?;
    let arguments: Value = serde_json::from_slice(unsafe { CStr::from_ptr(arguments) }.to_bytes())?;
    unsafe { &*browser }.command(operation, &arguments)
  })())
}

/// Returns the new events as a JSON array, or null when there are none.
///
/// # Safety
/// As in `apps_webview_attach`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webview_poll(browser: *const Browser) -> *mut c_char {
  let events = unsafe { &*browser }.poll();
  if events.is_empty() {
    return std::ptr::null_mut();
  }
  text(Value::Array(events).to_string())
}

/// # Safety
/// `text` comes from this library and is freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webview_free(text: *mut c_char) {
  if !text.is_null() {
    drop(unsafe { CString::from_raw(text) });
  }
}

/// # Safety
/// `browser` comes from `apps_webview_new` and is destroyed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_webview_destroy(browser: *mut Browser) {
  drop(unsafe { Box::from_raw(browser) });
}
