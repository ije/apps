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
pub(crate) fn observe_theme() -> tokio::sync::mpsc::Receiver<()> {
    thread_local! {
        static WAKE: std::cell::RefCell<Option<tokio::sync::mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) };
    }
    extern "C" fn wake() {
        WAKE.with_borrow(|sender| {
            if let Some(sender) = sender {
                let _ = sender.try_send(());
            }
        });
    }
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    WAKE.with_borrow_mut(|slot| *slot = Some(sender));
    unsafe { apps_theme_observe(wake) };
    receiver
}

fn validate_menu(template: &Value) -> Result<()> {
    for item in template.as_array().context("Expected menu template")? {
        let options = item.as_object().context("Expected menu item")?;
        for name in ["label", "role", "type", "accelerator"] {
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
    match operation {
        "poll"
        | "power.init"
        | "power.idle"
        | "power.thermal"
        | "power.battery"
        | "app.releaseInstance"
        | "app.hasInstance"
        | "app.locale"
        | "app.state" => {}
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
        "app.activationPolicy" => ensure!(
            arguments.is_null()
                || matches!(
                    arguments.as_str(),
                    Some("regular" | "accessory" | "prohibited")
                ),
            "Invalid activation policy"
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
        "menu.popup" => {
            validate_menu(&arguments["template"])?;
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
        | "clipboard.writeHTML"
        | "shell.openExternal"
        | "shell.openPath"
        | "shell.showItemInFolder"
        | "safeStorage.encrypt"
        | "safeStorage.decrypt"
        | "notification.close"
        | "app.badge"
        | "app.icon"
        | "app.downloadFinished" => {
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
