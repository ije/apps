use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
#[cfg(target_os = "macos")]
use std::ffi::{CStr, CString, c_char};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn apps_native_call(operation: *const c_char, arguments: *const c_char) -> *mut c_char;
    fn apps_native_free(text: *mut c_char);
    fn apps_theme_state() -> u32;
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

#[cfg(target_os = "macos")]
pub(crate) fn observe() {
    static WAKER: std::sync::OnceLock<crate::host::Waker> = std::sync::OnceLock::new();
    extern "C" fn wake() {
        if let Some(waker) = WAKER.get() {
            waker.wake();
        }
    }
    let _ = WAKER.set(crate::host::Waker::current());
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

pub fn native_call(operation: &str, arguments: &Value) -> Result<Value> {
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
        | "app.hasInstance"
        | "app.locale"
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
            for name in ["x", "y"] {
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
        | "clipboard.clear"
        | "theme.get"
        | "safeStorage.available" => {}
        "clipboard.writeText"
        | "clipboard.writeImage"
        | "clipboard.writeHTML"
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
        let _ = (operation, arguments);
        bail!("This native desktop operation is currently available on macOS");
    }
}
