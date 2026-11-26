// The desktop services of Linux: the clipboard of the display, and portals and the file manager on the session bus.
use super::dbus::{Bus, Value};
use anyhow::{Context as _, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as base64};
use serde_json::{Value as Json, json};
use std::path::Path;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, Once, OnceLock, PoisonError};
use std::time::Duration;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const DESKTOP: &str = "/org/freedesktop/portal/desktop";

static EVENTS: Mutex<Vec<Json>> = Mutex::new(Vec::new());
static WAKER: OnceLock<crate::jsc::Waker> = OnceLock::new();
// The theme of the system: 1 is dark and 2 high contrast. None until the portal answers.
static SYSTEM: (Mutex<Option<u32>>, Condvar) = (Mutex::new(None), Condvar::new());
// `system`, `light` or `dark`.
static SOURCE: AtomicU8 = AtomicU8::new(0);
const SOURCES: [&str; 3] = ["system", "light", "dark"];

fn emit(event: Json) {
    EVENTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(event);
    if let Some(waker) = WAKER.get() {
        waker.wake();
    }
}

pub(crate) fn observe() {
    let _ = WAKER.set(crate::jsc::Waker::current());
}

pub(crate) fn theme_state() -> u32 {
    static WATCH: Once = Once::new();
    WATCH.call_once(|| {
        let watcher = std::thread::Builder::new()
            .name("apps-theme".into())
            .stack_size(128 << 10);
        let started = watcher.spawn(|| {
            if let Err(error) = watch_theme() {
                set_system(0);
                eprintln!("apps: theme: {error:#}");
            }
        });
        if started.is_err() {
            set_system(0);
        }
    });
    let (lock, ready) = &SYSTEM;
    let system = lock.lock().unwrap_or_else(PoisonError::into_inner);
    // The first page waits for the portal, so a dark desktop doesn't first show a light page.
    let (system, _) = ready
        .wait_timeout_while(system, Duration::from_millis(200), |system| {
            system.is_none()
        })
        .unwrap_or_else(PoisonError::into_inner);
    let system = system.unwrap_or(0);
    match SOURCE.load(Ordering::Relaxed) {
        1 => system & !1,
        2 => system | 1,
        _ => system,
    }
}

fn set_system(state: u32) {
    let (lock, ready) = &SYSTEM;
    let old = lock
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .replace(state);
    ready.notify_all();
    if old.is_some_and(|old| old != state) {
        emit(json!({"type": "theme"}));
    }
}

fn watch_theme() -> Result<()> {
    let mut bus = Bus::session()?;
    bus.watch("type='signal',interface='org.freedesktop.portal.Settings',member='SettingChanged',arg0='org.freedesktop.appearance'")?;
    let mut read = |key: &str| {
        let args = [Value::str("org.freedesktop.appearance"), Value::str(key)];
        let value = bus.call(
            PORTAL,
            DESKTOP,
            "org.freedesktop.portal.Settings",
            "ReadOne",
            &args,
        );
        value
            .ok()
            .and_then(|value| value.first()?.as_u32())
            .unwrap_or(0)
    };
    // `color-scheme` is 1 for dark, `contrast` is 1 for high.
    let (mut scheme, mut contrast) = (read("color-scheme"), read("contrast"));
    loop {
        set_system(u32::from(scheme == 1) | u32::from(contrast == 1) << 1);
        let signal = bus.signal()?;
        let value = signal.body.get(2).and_then(Value::as_u32).unwrap_or(0);
        match signal.body.get(1).and_then(Value::as_str) {
            Some("color-scheme") => scheme = value,
            Some("contrast") => contrast = value,
            _ => {}
        }
    }
}

fn file_uri(path: &str) -> Result<String> {
    let path = std::path::absolute(path)?;
    Ok(url::Url::from_file_path(&path)
        .ok()
        .context("Expected a file path")?
        .into())
}

fn xdg_open(target: &str) -> bool {
    std::process::Command::new("xdg-open")
        .arg(target)
        .spawn()
        .is_ok_and(|mut child| {
            std::thread::spawn(move || child.wait());
            true
        })
}

// The portal opens only web and mail links for apps outside a sandbox, and desktops without it still have xdg-open.
fn open_uri(uri: &str) -> bool {
    let portal = Bus::session().and_then(|mut bus| {
        let args = [Value::str(""), Value::str(uri), Value::dict(vec![])];
        bus.call(
            PORTAL,
            DESKTOP,
            "org.freedesktop.portal.OpenURI",
            "OpenURI",
            &args,
        )
    });
    portal.is_ok() || xdg_open(uri)
}

fn dialog(open: bool, args: &Json) -> Result<Json> {
    static TOKENS: AtomicU32 = AtomicU32::new(0);
    let mut bus = Bus::session()?;
    let token = format!("apps{}", TOKENS.fetch_add(1, Ordering::Relaxed));
    let sender = bus.name.trim_start_matches(':').replace('.', "_");
    let request = format!("{DESKTOP}/request/{sender}/{token}");
    bus.watch(&format!("type='signal',interface='org.freedesktop.portal.Request',member='Response',path='{request}'"))?;
    let properties = |name: &str| {
        args["properties"]
            .as_array()
            .is_some_and(|list| list.iter().any(|item| item == name))
    };
    let folder = |path: &Path| {
        let mut bytes = path.as_os_str().as_encoded_bytes().to_vec();
        bytes.push(0);
        Value::Array("y".into(), bytes.into_iter().map(Value::Byte).collect())
    };
    let mut options = vec![
        ("handle_token", Value::str(token)),
        ("modal", Value::Bool(true)),
    ];
    if let Some(label) = args["buttonLabel"].as_str() {
        options.push(("accept_label", Value::str(label)));
    }
    let filters = args["filters"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|filter| {
            let extensions: Vec<&str> = filter["extensions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
                .collect();
            let name = filter["name"]
                .as_str()
                .map_or_else(|| extensions.join(", "), str::to_owned);
            let patterns = extensions.iter().map(|extension| {
                let glob = if *extension == "*" {
                    "*".into()
                } else {
                    format!("*.{extension}")
                };
                Value::Struct(vec![Value::U32(0), Value::Str(glob)])
            });
            Value::Struct(vec![
                Value::Str(name),
                Value::Array("(us)".into(), patterns.collect()),
            ])
        });
    let filters: Vec<Value> = filters.collect();
    if !filters.is_empty() {
        options.push(("filters", Value::Array("(sa(us))".into(), filters)));
    }
    let default = args["defaultPath"]
        .as_str()
        .map(std::path::absolute)
        .transpose()?;
    if open {
        options.push(("multiple", Value::Bool(properties("multiSelections"))));
        options.push(("directory", Value::Bool(properties("openDirectory"))));
        if let Some(path) = default.as_deref().and_then(|path| {
            if path.is_dir() {
                Some(path)
            } else {
                path.parent()
            }
        }) {
            options.push(("current_folder", folder(path)));
        }
    } else if let Some(path) = &default {
        if let Some(name) = path.file_name() {
            options.push(("current_name", Value::str(name.to_string_lossy())));
        }
        if let Some(parent) = path.parent().filter(|parent| parent.is_dir()) {
            options.push(("current_folder", folder(parent)));
        }
    }
    let title = args["title"]
        .as_str()
        .unwrap_or(if open { "Open" } else { "Save" });
    let method = if open { "OpenFile" } else { "SaveFile" };
    let call = [Value::str(""), Value::str(title), Value::dict(options)];
    bus.call(
        PORTAL,
        DESKTOP,
        "org.freedesktop.portal.FileChooser",
        method,
        &call,
    )?;
    let mut wait = move || -> Result<Json> {
        loop {
            let signal = bus.signal()?;
            if signal.path != request || signal.member != "Response" {
                continue;
            }
            let chosen = signal.body.first().and_then(Value::as_u32) == Some(0);
            let uris = signal.body.get(1).and_then(|results| results.get("uris"));
            let paths: Vec<String> = uris
                .map(Value::items)
                .unwrap_or_default()
                .iter()
                .filter_map(|uri| url::Url::parse(uri.as_str()?).ok()?.to_file_path().ok())
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            let canceled = !chosen || paths.is_empty();
            return Ok(match open {
                true => json!({"canceled": canceled, "filePaths": paths}),
                false => json!({"canceled": canceled, "filePath": paths.first()}),
            });
        }
    };
    let Some(id) = args["async"].as_u64() else {
        return wait();
    };
    std::thread::spawn(move || {
        let value = wait().unwrap_or_else(|error| {
            eprintln!("apps: dialog: {error:#}");
            json!({"canceled": true, "filePaths": [], "filePath": null})
        });
        emit(json!({"type": "reply", "id": id, "value": value}));
    });
    Ok(Json::Null)
}

// The clipboard API names the formats of macOS, and Linux names them by MIME type.
const TYPES: [(&str, &str); 5] = [
    ("public.utf8-plain-text", "text/plain"),
    ("public.html", "text/html"),
    ("public.rtf", "text/rtf"),
    ("public.png", "image/png"),
    ("public.file-url", "text/uri-list"),
];

fn mime(kind: &str) -> &str {
    TYPES
        .iter()
        .find(|(name, _)| *name == kind)
        .map_or(kind, |(_, mime)| mime)
}

/// The operations of `native_call` that Linux has, or `None`.
pub(crate) fn call(operation: &str, args: &Json) -> Result<Option<Json>> {
    use renderer::clipboard;
    let text = || args.as_str().unwrap_or_default();
    Ok(Some(match operation {
        "poll" => {
            std::mem::take(&mut *EVENTS.lock().unwrap_or_else(PoisonError::into_inner)).into()
        }
        "clipboard.readText" => clipboard::text().unwrap_or_default().into(),
        "clipboard.readHTML" => {
            String::from_utf8_lossy(&clipboard::read("text/html").unwrap_or_default()).into()
        }
        "clipboard.read" => base64
            .encode(clipboard::read(mime(text())).unwrap_or_default())
            .into(),
        "clipboard.formats" => {
            let types = clipboard::types().into_iter();
            let named = |kind: String| {
                TYPES
                    .iter()
                    .find(|(_, mime)| *mime == kind)
                    .map_or(kind, |(name, _)| (*name).to_owned())
            };
            types.map(named).collect::<Vec<_>>().into()
        }
        "clipboard.clear" => {
            clipboard::write(Vec::new());
            Json::Null
        }
        "clipboard.write" => {
            let items = args.as_object().into_iter().flatten();
            let items = items.map(|(kind, data)| {
                Ok((
                    mime(kind).to_owned(),
                    base64.decode(data.as_str().unwrap_or_default())?,
                ))
            });
            clipboard::write(items.collect::<Result<_>>()?);
            Json::Null
        }
        "shell.openExternal" => {
            let url = url::Url::parse(text())
                .ok()
                .filter(|url| matches!(url.scheme(), "http" | "https" | "mailto" | "tel"));
            open_uri(url.context("Unsupported external URL scheme")?.as_str()).into()
        }
        "shell.openPath" => (Path::new(text()).exists() && xdg_open(text())).into(),
        "shell.showItemInFolder" => {
            let uri = file_uri(text())?;
            let shown = Bus::session().and_then(|mut bus| {
                let args = [
                    Value::Array("s".into(), vec![Value::str(&uri)]),
                    Value::str(""),
                ];
                bus.call(
                    "org.freedesktop.FileManager1",
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1",
                    "ShowItems",
                    &args,
                )
            });
            if shown.is_err() {
                let parent = Path::new(text()).parent().context("Expected a file path")?;
                xdg_open(&parent.to_string_lossy());
            }
            Json::Null
        }
        "theme.get" => {
            let state = theme_state();
            let dark = state & 1 != 0;
            json!({
                "themeSource": SOURCES[usize::from(SOURCE.load(Ordering::Relaxed))],
                "shouldUseDarkColors": dark, "shouldUseHighContrastColors": state & 2 != 0,
                "shouldUseDarkColorsForSystemIntegratedUI": dark, "shouldUseInvertedColorScheme": false,
                "inForcedColorsMode": false, "prefersReducedTransparency": false, "shouldDifferentiateWithoutColor": false,
            })
        }
        "theme.set" => {
            let source = SOURCES
                .iter()
                .position(|source| *source == text())
                .context("Invalid theme source")?;
            let old = theme_state();
            SOURCE.store(source as u8, Ordering::Relaxed);
            if theme_state() != old {
                emit(json!({"type": "theme"}));
            }
            Json::Null
        }
        "dialog.open" | "dialog.save" => dialog(operation == "dialog.open", args)?,
        "dialog.message" => bail!("dialog.showMessageBox() is not supported on Linux"),
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn paths_become_file_uris() {
        assert_eq!(
            super::file_uri("/tmp/a b%.txt").unwrap(),
            "file:///tmp/a%20b%25.txt"
        );
    }
}
