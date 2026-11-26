// The services of the operating system, and the optional native libraries next to the runtime.
pub(crate) mod browser;
pub(crate) mod devtools;
pub mod library;
#[cfg(target_os = "linux")]
pub mod webview;

// Linux loads the system web view from this library, next to the runtime.
pub fn webview_library(platform: &str) -> Option<&'static str> {
    (platform == "linux").then_some("libapps_webview.so")
}

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
#[cfg(target_os = "macos")]
use std::ffi::{CStr, CString, c_char};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn apps_native_call(operation: *const c_char, arguments: *const c_char) -> *mut c_char;
    fn apps_native_free(text: *mut c_char);
    fn apps_theme_state() -> u32;
    fn apps_date_patterns() -> *mut c_char;
    fn apps_theme_observe(wake: extern "C" fn());
}

pub(crate) fn theme_state() -> u32 {
    #[cfg(target_os = "macos")]
    unsafe {
        apps_theme_state()
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

/// The patterns of dates, times and months, the names of the halves of the day and of the months, one on each line.
pub(crate) fn date_patterns() -> String {
    #[cfg(target_os = "macos")]
    unsafe {
        let text = apps_date_patterns();
        let lines = CStr::from_ptr(text).to_string_lossy().into_owned();
        apps_native_free(text);
        lines
    }
    #[cfg(not(target_os = "macos"))]
    {
        let locale = ["LC_ALL", "LC_TIME", "LANG"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
            .unwrap_or_default();
        let (date, time) = match locale.starts_with("en_US") {
            true => ("MM/dd/yyyy", "hh:mm a"),
            false => ("dd/MM/yyyy", "HH:mm"),
        };
        format!(
            "{date}\n{time}\nMMMM yyyy\nAM\nPM\nJanuary\nFebruary\nMarch\nApril\nMay\nJune\nJuly\nAugust\n\
             September\nOctober\nNovember\nDecember"
        )
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn observe() {
    static WAKER: std::sync::OnceLock<crate::jsc::Waker> = std::sync::OnceLock::new();
    extern "C" fn wake() {
        if let Some(waker) = WAKER.get() {
            waker.wake();
        }
    }
    let _ = WAKER.set(crate::jsc::Waker::current());
    unsafe { apps_theme_observe(wake) };
}

fn validate_menu(template: &Value) -> Result<()> {
    for item in template.as_array().context("Expected menu template")? {
        let options = item.as_object().context("Expected menu item")?;
        for name in [
            "label",
            "role",
            "type",
            "accelerator",
            "toolTip",
            "sublabel",
            "icon",
        ] {
            ensure!(
                options.get(name).is_none_or(Value::is_string),
                "Expected menu text: {name}"
            );
        }
        for name in ["enabled", "visible", "checked"] {
            ensure!(
                options.get(name).is_none_or(Value::is_boolean),
                "Expected menu boolean: {name}"
            );
        }
        ensure!(
            options.get("click").is_none_or(Value::is_u64),
            "Expected menu callback id"
        );
        if let Some(submenu) = options.get("submenu") {
            validate_menu(submenu)?;
        }
    }
    Ok(())
}

pub(crate) fn native_call(operation: &str, arguments: &Value) -> Result<Value> {
    if operation.starts_with("tray.") {
        ensure!(
            arguments["id"].as_u64().is_some_and(|id| id > 0),
            "Expected tray id"
        );
    }
    match operation {
        "poll"
        | "power.init"
        | "power.idle"
        | "power.thermal"
        | "power.battery"
        | "app.releaseInstance"
        | "app.locale"
        | "app.emojiPanel"
        | "app.state"
        | "screen.displays"
        | "screen.cursor"
        | "prefs.appearance"
        | "prefs.animation"
        | "prefs.swipe"
        | "prefs.touchID" => {}
        "power.block" | "prefs.trusted" => {
            ensure!(arguments.is_boolean(), "Expected boolean argument")
        }
        "power.unblock" => ensure!(
            arguments.as_u64().is_some_and(|v| v <= u32::MAX as u64),
            "Expected an assertion id"
        ),
        "prefs.color" => ensure!(
            arguments.as_str().is_some_and(
                |name| name.ends_with("Color") && name.bytes().all(|c| c.is_ascii_alphabetic())
            ),
            "Invalid color name"
        ),
        "prefs.media" => ensure!(
            matches!(arguments.as_str(), Some("microphone" | "camera" | "screen")),
            "Invalid media type"
        ),
        "prefs.askMedia" => ensure!(
            matches!(arguments["type"].as_str(), Some("microphone" | "camera"))
                && arguments["async"].is_u64(),
            "Invalid media type"
        ),
        "prefs.promptTouchID" => ensure!(
            arguments["reason"].as_str().is_some_and(|v| !v.is_empty())
                && arguments["async"].is_u64(),
            "reason must be non-empty"
        ),
        "prefs.getDefault" | "prefs.setDefault" => ensure!(
            arguments["key"].is_string()
                && matches!(
                    arguments["type"].as_str(),
                    Some(
                        "string"
                            | "boolean"
                            | "integer"
                            | "float"
                            | "double"
                            | "url"
                            | "array"
                            | "dictionary"
                    )
                ),
            "Invalid user default"
        ),
        "prefs.registerDefaults" => ensure!(arguments.is_object(), "Expected user defaults"),
        "app.hidden" => ensure!(arguments.is_boolean(), "Expected boolean argument"),
        "app.loginItem" => ensure!(
            arguments.is_null() || arguments.is_boolean(),
            "Expected login item state"
        ),
        "app.attention" => ensure!(
            matches!(arguments.as_str(), Some("critical" | "informational")),
            "Invalid attention type"
        ),
        "app.cancelAttention" => ensure!(arguments.is_u64(), "Expected attention request id"),
        "app.progress" => ensure!(arguments.is_number(), "Expected a progress number"),
        "app.activationPolicy" => ensure!(
            arguments.is_null()
                || matches!(
                    arguments.as_str(),
                    Some("regular" | "accessory" | "prohibited")
                ),
            "Invalid activation policy"
        ),
        "app.recentDocument" => ensure!(
            arguments.is_null() || arguments.is_string(),
            "Expected a document path"
        ),
        "app.protocolClient" => ensure!(arguments["scheme"].is_string(), "Expected a scheme"),
        "image" => ensure!(
            arguments["data"].is_string() && arguments["op"].is_string(),
            "Expected image data"
        ),
        "app.about" => {
            if !arguments.is_null() {
                let options = arguments
                    .as_object()
                    .context("Expected about panel options")?;
                for name in [
                    "applicationName",
                    "applicationVersion",
                    "version",
                    "copyright",
                    "credits",
                    "iconPath",
                ] {
                    ensure!(
                        options.get(name).is_none_or(Value::is_string),
                        "Expected text option: {name}"
                    );
                }
            }
        }
        "app.singleInstance" => {
            ensure!(
                arguments["id"].as_str().is_some_and(|v| !v.is_empty())
                    && arguments["cwd"].is_string()
                    && arguments["argv"]
                        .as_array()
                        .is_some_and(|v| v.iter().all(Value::is_string)),
                "Invalid application instance request"
            );
        }
        "menu.set" => {
            if !arguments.is_null() {
                validate_menu(arguments)?;
            }
        }
        "tray.create" | "tray.image" | "tray.pressedImage" => {
            ensure!(arguments["image"].is_string(), "Expected a tray image path");
            ensure!(
                arguments.get("guid").is_none_or(Value::is_string),
                "Expected a GUID"
            );
        }
        "tray.menu" | "tray.popup" => {
            if !arguments["template"].is_null() {
                validate_menu(&arguments["template"])?;
            }
            if operation == "tray.popup" {
                ensure!(
                    arguments["request"].as_u64().is_some_and(|id| id > 0),
                    "Expected tray popup id"
                );
            }
        }
        "tray.toolTip" | "tray.title" => {
            ensure!(arguments["text"].is_string(), "Expected tray text");
            ensure!(
                arguments.get("fontType").is_none_or(Value::is_string),
                "Expected tray font type"
            );
        }
        "tray.ignoreDoubleClick" => ensure!(arguments["ignore"].is_boolean(), "Expected boolean"),
        "tray.destroy"
        | "tray.close"
        | "tray.getTitle"
        | "tray.guid"
        | "tray.bounds"
        | "tray.getIgnoreDoubleClick" => {}
        "menu.popup" => {
            validate_menu(&arguments["template"])?;
            ensure!(
                arguments["request"].as_u64().is_some_and(|id| id > 0),
                "Expected menu popup id"
            );
            for name in ["x", "y", "positioningItem", "minWidth"] {
                ensure!(
                    arguments
                        .get(name)
                        .is_none_or(|v| v.as_f64().is_some_and(f64::is_finite)),
                    "Invalid menu position"
                );
            }
        }
        "shortcut.register" => {
            ensure!(
                arguments["id"]
                    .as_u64()
                    .is_some_and(|v| v > 0 && v <= u32::MAX as u64)
                    && arguments["accelerator"].is_string(),
                "Invalid shortcut registration"
            );
        }
        "shortcut.unregister" => {
            ensure!(arguments.is_u64(), "Expected shortcut id");
        }

        "clipboard.readText"
        | "clipboard.readHTML"
        | "clipboard.formats"
        | "clipboard.clear"
        | "theme.get"
        | "safeStorage.available" => {}
        "clipboard.write" => ensure!(
            arguments
                .as_object()
                .is_some_and(|items| items.values().all(Value::is_string)),
            "Expected clipboard data"
        ),
        "clipboard.read"
        | "shell.openExternal"
        | "shell.openPath"
        | "shell.showItemInFolder"
        | "safeStorage.encrypt"
        | "safeStorage.decrypt"
        | "notification.close"
        | "app.badge"
        | "app.icon"
        | "app.downloadFinished"
        | "prefs.removeDefault" => {
            ensure!(arguments.is_string(), "Expected text argument");
        }
        "theme.set" => {
            ensure!(
                matches!(arguments.as_str(), Some("system" | "dark" | "light")),
                "Invalid theme source"
            );
        }
        "dialog.open" | "dialog.save" | "dialog.message" | "notification.show" => {
            let options = arguments.as_object().context("Expected native options")?;
            for name in [
                "title",
                "defaultPath",
                "message",
                "detail",
                "body",
                "id",
                "checkboxLabel",
                "buttonLabel",
                "nameFieldLabel",
                "type",
                "input",
            ] {
                ensure!(
                    options.get(name).is_none_or(Value::is_string),
                    "Expected text option: {name}"
                );
            }
            for name in ["properties", "buttons"] {
                ensure!(
                    options.get(name).is_none_or(|v| v
                        .as_array()
                        .is_some_and(|items| items.iter().all(Value::is_string))),
                    "Expected text array option: {name}"
                );
            }
            for name in ["silent", "checkboxChecked"] {
                ensure!(
                    options.get(name).is_none_or(Value::is_boolean),
                    "Expected boolean option: {name}"
                );
            }
            for name in ["async", "defaultId", "cancelId"] {
                ensure!(
                    options.get(name).is_none_or(Value::is_u64),
                    "Expected integer option: {name}"
                );
            }
            ensure!(
                options
                    .get("filters")
                    .is_none_or(|filters| filters.as_array().is_some_and(|filters| filters
                        .iter()
                        .all(|filter| filter["extensions"]
                            .as_array()
                            .is_some_and(|items| items.iter().all(Value::is_string))))),
                "Expected dialog filters with extensions"
            );
        }
        _ => bail!("Unknown native operation: {operation}"),
    }
    #[cfg(target_os = "macos")]
    {
        let operation = CString::new(operation)?;
        let arguments = CString::new(arguments.to_string())?;
        let pointer = unsafe { apps_native_call(operation.as_ptr(), arguments.as_ptr()) };
        ensure!(!pointer.is_null(), "Native operation returned no response");
        let result = serde_json::from_slice::<Value>(unsafe { CStr::from_ptr(pointer) }.to_bytes());
        unsafe { apps_native_free(pointer) };
        let value = result?;
        if let Some(error) = value["error"].as_str() {
            bail!("{error}");
        }
        Ok(value["value"].clone())
    }
    #[cfg(not(target_os = "macos"))]
    {
        if operation == "image" {
            return image(arguments);
        }
        bail!("This native desktop operation is currently available on macOS");
    }
}

// The image operations of desktop.m. `fileIcon` needs the system.
#[cfg(not(target_os = "macos"))]
fn image(arguments: &Value) -> Result<Value> {
    use crate::render::raster::image::{encode_png, pixels};
    use base64::{Engine as _, engine::general_purpose::STANDARD as base64};
    use image::{RgbImage, RgbaImage, codecs::jpeg::JpegEncoder, imageops};
    let data = base64.decode(arguments["data"].as_str().unwrap_or_default())?;
    let number = |name: &str| arguments[name].as_f64().unwrap_or(0.) as u32;
    let (width, height) = (number("width"), number("height"));
    let operation = arguments["op"].as_str().unwrap_or_default();
    ensure!(width <= 16384 && height <= 16384, "Image is too large");
    // The bitmaps of `fromBitmap` and `bitmap` are premultiplied BGRA.
    let (mut bytes, width_in, height_in) = match operation {
        "fromBitmap" => (data, width as usize, height as usize),
        _ => pixels(&data)?,
    };
    ensure!(
        width_in > 0 && height_in > 0 && bytes.len() >= width_in * height_in * 4,
        "Cannot read the image"
    );
    bytes.truncate(width_in * height_in * 4);
    for pixel in bytes.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
        let alpha = u32::from(pixel[3]);
        if alpha != 255 && operation != "jpeg" {
            for value in &mut pixel[..3] {
                *value = match operation {
                    "fromBitmap" => (*value).min(alpha as u8),
                    _ => ((u32::from(*value) * alpha + 127) / 255) as u8,
                };
            }
        }
    }
    let mut image = RgbaImage::from_raw(width_in as u32, height_in as u32, bytes)
        .context("Cannot read the image")?;
    match operation {
        "decode" | "fromBitmap" => {}
        "bitmap" => {
            let mut bytes = image.into_raw();
            for pixel in bytes.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            return Ok(serde_json::json!({ "bytes": base64.encode(bytes) }));
        }
        "jpeg" => {
            let (width, height) = image.dimensions();
            let rgb = image
                .pixels()
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]]);
            let rgb = RgbImage::from_raw(width, height, rgb.collect()).context("Invalid image")?;
            let mut bytes = Vec::new();
            let quality = arguments["quality"].as_f64().unwrap_or(90.).clamp(1., 100.) as u8;
            rgb.write_with_encoder(JpegEncoder::new_with_quality(&mut bytes, quality))?;
            return Ok(serde_json::json!({
                "png": base64.encode(bytes), "width": width, "height": height
            }));
        }
        "resize" => {
            ensure!(width > 0 && height > 0, "Invalid image size");
            image = imageops::resize(&image, width, height, imageops::FilterType::Triangle);
        }
        "crop" => {
            let (x, y) = (number("x"), number("y"));
            image = imageops::crop_imm(&image, x, y, width, height).to_image();
            ensure!(image.width() > 0 && image.height() > 0, "Empty crop area");
        }
        _ => bail!("This native desktop operation is currently available on macOS"),
    }
    let (width, height) = image.dimensions();
    let image = tiny_skia::IntSize::from_wh(width, height)
        .and_then(|size| tiny_skia::Pixmap::from_vec(image.into_raw(), size))
        .context("Cannot read the image")?;
    Ok(serde_json::json!({
        "png": base64.encode(encode_png(&image)?), "width": width, "height": height
    }))
}
