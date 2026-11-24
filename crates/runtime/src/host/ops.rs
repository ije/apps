use super::context::{Binding, CLIPBOARD, NATIVE, REFLOW, binding};
use super::{Host, WORKER_PAGE, link::Target, reaches, window};
use crate::jsc::{self, Ptr, Role, encoding, payload};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{collections::HashMap, io::Write, path::PathBuf};
use std::{ptr, rc::Rc};

const MAIN: u8 = 1;
const PRELOAD: u8 = 2;
const PAGE: u8 = 4;
const NODE: u8 = MAIN | PRELOAD;
const ALL: u8 = NODE | PAGE;

// Deny unlisted host operations. Keep module-level checks too.
pub(super) fn access(role: Role, op: &str) -> Result<()> {
    let roles = match op.split_once('.') {
        None => match op {
            "log" => ALL,
            "info" | "cwd" | "chdir" => NODE,
            "window" | "quit" | "focus" => MAIN,
            _ => 0,
        },
        Some((service, rest)) => match service {
            "bridge" => match rest {
                "dispatch" => ALL,
                "expose" | "preload" | "file" => PRELOAD,
                "exposures" => PAGE,
                _ => 0,
            },
            "dom" if rest == "open" => PAGE | PRELOAD,
            "dom" | "gpu" | "media" => PAGE,
            "terminal" => PAGE | PRELOAD,
            "web" if rest.starts_with("serve.") => MAIN,
            "web" if rest.starts_with("tcp.") => NODE,
            "web" => ALL,
            "native"
                if rest.starts_with("clipboard.")
                    || rest.starts_with("shell.")
                    || rest == "image" =>
            {
                NODE
            }
            "native" | "browser" | "app" => MAIN,
            "node" | "pty" | "child" | "sqlite" | "process" => NODE,
            _ => 0,
        },
    };
    match role {
        Role::Main if roles & MAIN != 0 => Ok(()),
        Role::Preload(_) if roles & PRELOAD != 0 => Ok(()),
        Role::Renderer(_) if roles & PAGE != 0 => Ok(()),
        Role::Renderer(_) => bail!("{op} is unavailable in page scripts"),
        _ if roles == MAIN => bail!("{op} is only available in the main context"),
        _ => bail!("Unsupported host operation: {op}"),
    }
}

// This matches `JSON.stringify` and serde, so ops see the same integers and non-finite numbers become null.
fn number_value(number: f64) -> serde_json::Value {
    if number.fract() == 0. && number.abs() < 1e21 {
        if (0. ..18446744073709551616.).contains(&number) {
            return (number as u64).into();
        } else if number >= -9223372036854775808. {
            return (number as i64).into();
        }
    }
    number.into()
}

pub(super) unsafe extern "C" fn call(
    ctx: Ptr,
    _function: Ptr,
    _this: Ptr,
    count: usize,
    args: *const Ptr,
    exception: *mut Ptr,
) -> Ptr {
    use serde_json::Value;
    let Some(binding) = binding(ctx) else {
        return ptr::null_mut();
    };
    let api = binding.api.clone();
    let values = if count == 0 || args.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(args, count) }
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Ptr> {
        let generic = values
            .first()
            .is_some_and(|&value| api.kind(ctx, value) == 3);
        let mut name = [0u8; 64];
        let op = {
            let value = *values
                .get(usize::from(generic))
                .context("Missing host operation")?;
            let size = match api.chars(ctx, value) {
                Some(encoding::Chars::Latin1(op)) if op.len() <= name.len() => {
                    name[..op.len()].copy_from_slice(op);
                    op.len()
                }
                _ => 0,
            };
            std::str::from_utf8(&name[..size])
                .ok()
                .filter(|op| !op.is_empty())
                .context("Invalid host operation")?
        };
        access(binding.role, op)?;
        // This one needs the buffer itself, not its bytes. A number gives back memory that was never sent.
        if op == "web.buffer.take" {
            let value = *values.get(1).context("Expected a buffer")?;
            let id = match api.kind(ctx, value) {
                3 => {
                    payload::held(api.to_f64(ctx, value) as u64);
                    None
                }
                5 => payload::take(&api, ctx, value),
                _ => None,
            };
            return Ok(match id {
                Some(id) => api.make_number(ctx, id as f64),
                None => api.make_undefined(ctx),
            });
        }
        let run = |buffers, action: &mut dyn FnMut() -> Result<Value>| -> Result<Ptr> {
            let mut failure = None;
            let payload = payload::Payload::capture(buffers, || {
                action().unwrap_or_else(|error| {
                    failure = Some(error);
                    Value::Null
                })
            });
            match failure {
                Some(error) => Err(error),
                None => Ok(payload::result(&api, ctx, payload)),
            }
        };
        if generic {
            let (buffers, _roots) = values
                .get(3)
                .map(|&buffers| payload::input(&api, ctx, buffers))
                .transpose()?
                .unzip();
            let buffers = buffers.unwrap_or_default();
            let json = api.json(ctx, *values.get(2).context("Expected JSON arguments")?);
            if let (Role::Renderer(id), Some(op)) = (binding.role, op.strip_prefix("gpu.")) {
                return run(buffers, &mut || gpu(&binding, ctx, id, op, json.as_bytes()));
            }
            let mut args = Some(serde_json::from_str(&json)?);
            let result = run(buffers, &mut || {
                dispatch(&binding, ctx, op, args.take().unwrap_or_default())
            });
            if let Role::Renderer(id) = binding.role {
                report_nodes(&binding, ctx, id);
            }
            return result;
        }
        let role = binding.role;
        if op == "web.realm" {
            ensure!(values.len() == 2, "Expected a module loader");
            ensure!(
                binding
                    .host
                    .try_borrow()
                    .is_ok_and(|host| host.app.features.has(crate::bundle::Feature::JsExecute)),
                "ShadowRealm needs the jsExecute feature"
            );
            return Ok(api.realm(values[1]));
        }
        // A forced layout builds the page, which borrows the host, so it runs before any borrow.
        if let Role::Renderer(id) = role
            && op == "dom.reflow"
        {
            if let Some(reflow) = REFLOW.get() {
                reflow(id);
            }
            return Ok(api.make_undefined(ctx));
        }
        if let Role::Renderer(id) = role
            && op == "dom.batch"
        {
            ensure!(values.len() == 4, "Expected DOM operations");
            let text = api.text(ctx, values[1]);
            let size = api.to_f64(ctx, values[3]) * 4.;
            let codes = payload::view(&api, ctx, values[2])?;
            let codes = (size >= 0. && size.fract() == 0.)
                .then(|| codes.get(..size as usize))
                .flatten()
                .context("Expected DOM operations")?;
            let errors = binding
                .host
                .try_borrow_mut()
                .context("Host is busy")?
                .batch(id, codes, &text)?;
            report_nodes(&binding, ctx, id);
            return Ok(payload::result(&api, ctx, errors.into()));
        }
        if let Role::Renderer(id) = role
            && op == "dom.layout"
        {
            let mut host = binding.host.try_borrow_mut().context("Host is busy")?;
            let host = &mut *host;
            let page = host.pages.get_mut(&id).context("Window is closed")?;
            let state = host.dom.entry(id).or_default();
            let viewport = *host.viewports.get(&id).unwrap_or(&[800., 600.]);
            if values.get(2).is_some_and(|&value| api.to_bool(ctx, value)) {
                crate::dom::layout_flags(&mut page.doc, state, viewport);
            }
            state.sync_scroll();
            if !values.get(1).is_some_and(|&value| api.to_bool(ctx, value)) {
                return Ok(api.make_null(ctx));
            }
            let (pointer, size, owner) = state.bounds.share();
            return Ok(api.external_buffer(
                ctx,
                pointer,
                size,
                crate::dom::LayoutMirror::release,
                owner,
            ));
        }
        // fs.readSync reads straight into the caller's buffer. No JavaScript runs during the read.
        if op == "node.readInto" {
            ensure!(values.len() == 6, "Expected read arguments");
            let arg = |index: usize| match api.kind(ctx, values[index]) {
                3 => number_value(api.to_f64(ctx, values[index])),
                _ => Value::Null,
            };
            let out = api.bytes(ctx, values[2]).context("Expected a buffer")?;
            let offset = arg(3).as_u64().unwrap_or(0) as usize;
            let length = arg(4).as_u64().context("Expected read size")? as usize;
            let out = out
                .get_mut(offset..offset.saturating_add(length))
                .context("Expected a buffer range")?;
            let count = binding
                .node
                .try_borrow()
                .context("Node is busy")?
                .files
                .read_into(&arg(1), out, arg(5).as_u64())?;
            return Ok(api.make_number(ctx, count as f64));
        }
        let mut buffers = Vec::new();
        let mut args = Vec::with_capacity(values.len() - 1);
        for &value in &values[1..] {
            args.push(match api.kind(ctx, value) {
                2 => Value::Bool(api.to_bool(ctx, value)),
                3 => number_value(api.to_f64(ctx, value)),
                4 => Value::String(api.text(ctx, value)),
                5 if api.is_callable(ctx, value) => Value::Null,
                5 => {
                    // These bytes are not static: they borrow the arguments, which live only until the call
                    // returns.
                    buffers.push(bytes::Bytes::from_static(payload::view(&api, ctx, value)?));
                    serde_json::json!({"$appsBytes": buffers.len() - 1})
                }
                7 => bail!("Do not know how to serialize a BigInt"),
                _ => Value::Null,
            });
        }
        let mut args = Some(Value::Array(args));
        let result = run(buffers, &mut || {
            dispatch(&binding, ctx, op, args.take().unwrap_or_default())
        });
        if let Role::Renderer(id) = role {
            report_nodes(&binding, ctx, id);
        }
        result
    }));
    let (message, code) = match result {
        Ok(Ok(value)) => return value,
        Ok(Err(error)) => (
            error.to_string(),
            error
                .downcast_ref::<std::io::Error>()
                .map(crate::node::code),
        ),
        Err(_) => ("Host call failed".into(), None),
    };
    api.throw_error(ctx, exception, &message, code);
    api.make_undefined(ctx)
}

pub(super) fn dispatch(
    binding: &Binding,
    ctx: Ptr,
    op: &str,
    mut args: serde_json::Value,
) -> Result<serde_json::Value> {
    let Binding {
        host,
        role,
        sqlite,
        web,
        node,
        ..
    } = binding;
    let role = *role;
    let values = &args;
    if let Some(op) = op.strip_prefix("browser.") {
        anyhow::ensure!(
            matches!(role, Role::Main),
            "Browser views can only be managed by main"
        );
        let mut host = host.try_borrow_mut().context("Host is busy")?;
        if host.app.packaged {
            if op == "command" && values[1] == "inspect" {
                return Ok(serde_json::Value::Null);
            }
            if op == "create" {
                let mut values = values.clone();
                values[0]["inspectable"] = false.into();
                let host = &mut *host;
                return host.browsers.call(
                    op,
                    &values,
                    host.app.name.as_deref().unwrap_or_default(),
                );
            }
        }
        let host = &mut *host;
        return host
            .browsers
            .call(op, values, host.app.name.as_deref().unwrap_or_default());
    }
    if let Some((service @ ("node" | "pty" | "child"), op)) = op.split_once('.') {
        anyhow::ensure!(
            !matches!(role, Role::Renderer(_)),
            "Node APIs are unavailable in page scripts"
        );
        let mut node = node.try_borrow_mut().context("Node is busy")?;
        return match service {
            "pty" => node.pty.call(op, values),
            "child" => node.children.call(op, values),
            _ => node.call(op, &mut args),
        };
    }
    if let Some(op) = op.strip_prefix("native.") {
        anyhow::ensure!(
            matches!(role, Role::Main)
                || matches!(role, Role::Preload(_))
                    && (op.starts_with("clipboard.") || op.starts_with("shell.") || op == "image"),
            "Native desktop APIs require the main process"
        );
        let result = match NATIVE.get() {
            Some(native) => native(op, &values[0]),
            None => crate::native::native_call(op, &values[0]),
        }?;
        if op == "theme.set" {
            let mut host = host.borrow_mut();
            host.sync_theme();
            host.command(
                crate::host::link::Target::All,
                serde_json::json!({"op":"theme"}),
            );
        }
        return Ok(result);
    }
    if matches!(op, "bridge.expose" | "bridge.preload") {
        let Role::Preload(id) = role else {
            bail!("Only preload can expose APIs")
        };
        let (name, value) = if op == "bridge.preload" {
            ("__appsPreload", &values[0])
        } else {
            (
                values[0].as_str().context("Expected exposed name")?,
                &values[1],
            )
        };
        anyhow::ensure!(
            !matches!(name, "__proto__" | "constructor" | "prototype")
                && (!name.starts_with("__apps") || op == "bridge.preload"),
            "Reserved bridge name"
        );
        let mut host = host.try_borrow_mut().context("Host is busy")?;
        let exposed = host.bridge.entry(id).or_default();
        anyhow::ensure!(
            op == "bridge.preload" || !exposed.contains_key(name),
            "Cannot bind an API on top of an existing property on the window object"
        );
        exposed.insert(name.to_owned(), payload::Payload::snapshot(value.clone()));
        return Ok(serde_json::Value::Null);
    }
    if op == "bridge.file" {
        let path = match (role, values[0].as_u64()) {
            (Role::Preload(id), Some(index)) => host
                .borrow()
                .dropped
                .get(&id)
                .and_then(|paths| paths.get(index as usize).cloned()),
            _ => None,
        };
        return Ok(path.unwrap_or_default().into());
    }
    if op == "bridge.exposures" {
        let Role::Renderer(id) = role else {
            bail!("Only pages can read exposed APIs")
        };
        return Ok(serde_json::Value::Object(
            host.borrow()
                .bridge
                .get(&id)
                .into_iter()
                .flat_map(|values| values.iter())
                .map(|(name, value)| (name.clone(), value.clone().into_value()))
                .collect(),
        ));
    }
    if op == "bridge.dispatch" {
        let target = &values[0];
        if let Role::Renderer(id) = role {
            anyhow::ensure!(
                target["preload"] == id
                    && matches!(
                        values[1]["kind"].as_str(),
                        Some("call" | "reply" | "release")
                    ),
                "Page bridge calls must target their preload"
            );
        }
        let target = serde_json::from_value::<Role>(target.clone()).ok();
        let (link, rust) = {
            let host = host.try_borrow().context("Host is busy")?;
            (host.link.clone(), host.app.rust_main)
        };
        if let Some(link) = link {
            use crate::host::link::{ToMain, ToPage};
            use payload::Payload;
            match (role, target) {
                (Role::Main, Some(Role::Preload(id))) => {
                    let mut message = args[1].take();
                    message["sender"] = serde_json::to_value(role)?;
                    link.page(ToPage::Bridge(id, Payload::snapshot(message)));
                    return Ok(serde_json::Value::Null);
                }
                (Role::Preload(_), Some(Role::Main)) => {
                    let mut message = args[1].take();
                    message["sender"] = serde_json::to_value(role)?;
                    let kind = message["kind"].as_str().unwrap_or_default().to_owned();
                    anyhow::ensure!(rust == (kind == "rust"), "Bridge target is unavailable");
                    let message = Payload::snapshot(message);
                    return Ok(match kind.as_str() {
                        "sync" => link
                            .ask(|answer| ToMain::Sync(message, answer))?
                            .into_value(),
                        "event" => {
                            link.main(ToMain::Bridge(message));
                            serde_json::Value::Null
                        }
                        _ => {
                            link.main(ToMain::Bridge(message));
                            serde_json::json!({"pending":true})
                        }
                    });
                }
                _ => {}
            }
        }
        let destination = jsc::contexts::<Binding>()
            .into_iter()
            .find(|(_, binding)| Rc::ptr_eq(host, &binding.host) && target == Some(binding.role));
        if destination.is_none() && matches!(role, Role::Main) && values[1]["kind"] == "event" {
            return Ok(serde_json::Value::Null);
        }
        let (destination, target) = destination.context("Bridge target is unavailable")?;
        let mut message = args[1].take();
        message["sender"] = serde_json::to_value(role)?;
        return Ok(payload::receive(
            &target.api,
            destination,
            payload::Payload::snapshot(message),
        )?
        .into_value());
    }
    if let Some(op) = op.strip_prefix("web.") {
        anyhow::ensure!(
            !op.starts_with("serve.") || matches!(role, Role::Main),
            "HTTP servers are only available in the main context"
        );
        anyhow::ensure!(
            !op.starts_with("tcp.") || !matches!(role, Role::Renderer(_)),
            "TCP sockets require a native context"
        );
        anyhow::ensure!(
            !matches!(role, Role::Renderer(_)) || op != "worker.start" || values[1] == false,
            "Page workers cannot request native APIs"
        );
        web.try_borrow_mut()
            .context("Web is busy")?
            .call(op, &mut args)
    } else if op.starts_with("sqlite.") {
        sqlite
            .try_borrow_mut()
            .context("SQLite is busy")?
            .call(role, op, &args)
    } else if let (Role::Renderer(id), Some(op)) = (role, op.strip_prefix("gpu.")) {
        gpu(binding, ctx, id, op, &serde_json::to_vec(&args)?)
    } else if let (Role::Renderer(id), Some(op)) = (role, op.strip_prefix("media.")) {
        host.try_borrow_mut()
            .context("Host is busy")?
            .media(id, op, &args)
    } else if let (Role::Renderer(id) | Role::Preload(id), Some(op)) =
        (role, op.strip_prefix("terminal."))
    {
        host.try_borrow_mut().context("Host is busy")?.terminal(
            id,
            matches!(role, Role::Preload(_)),
            op,
            &args,
        )
    } else if let (Role::Renderer(id), "dom.contextMenu") = (role, op) {
        if let Some(link) = host.try_borrow().context("Host is busy")?.link.clone() {
            link.event(
                "context-menu",
                id,
                Some(values[0].clone()),
                "apps:context-menu",
            );
        }
        Ok(serde_json::Value::Null)
    } else if let (Role::Renderer(id), "dom.endComposition") = (role, op) {
        host.try_borrow_mut().context("Host is busy")?.command(
            crate::host::link::Target::Main,
            serde_json::json!({"op":"window","args":["composition",id]}),
        );
        Ok(serde_json::Value::Null)
    } else if let (Role::Renderer(id), "dom.filePicker") = (role, op) {
        let picker = &values[0];
        anyhow::ensure!(
            picker["id"].is_u64()
                && picker["multiple"].is_boolean()
                && picker["directory"].is_boolean()
                && picker["accept"].is_string(),
            "Invalid file picker"
        );
        if let Some(link) = host.try_borrow().context("Host is busy")?.link.clone() {
            let picker = serde_json::json!({"id":picker["id"],"multiple":picker["multiple"],"directory":picker["directory"],"accept":picker["accept"]});
            link.event("file-picker", id, Some(picker), "apps:file-picker");
        }
        Ok(serde_json::Value::Null)
    } else if let (Role::Renderer(id), "dom.selectPopup") = (role, op) {
        let popup = &values[0];
        anyhow::ensure!(
            popup["id"].is_u64()
                && popup["selected"].is_i64()
                && ["x", "y", "width"]
                    .iter()
                    .all(|name| popup[name].is_number())
                && popup["items"].as_array().is_some_and(|items| items
                    .iter()
                    .all(|item| item.is_null() || item[0].is_string() && item[1].is_u64())),
            "Invalid select popup"
        );
        if let Some(link) = host.try_borrow().context("Host is busy")?.link.clone() {
            link.event("select-popup", id, Some(popup.clone()), "apps:select-popup");
        }
        Ok(serde_json::Value::Null)
    } else if let (Role::Renderer(id) | Role::Preload(id), "dom.open") = (role, op) {
        // Main runs these with page data, so only these shapes and `<webview>` methods pass.
        let data = &values[3];
        let valid = match (values[0].as_str(), values[1].as_str()) {
            (Some("dialog"), Some("alert" | "confirm" | "prompt")) => {
                values[2].is_string() && values[3].is_string()
            }
            _ if matches!(role, Role::Preload(_)) => false,
            (Some("open"), _) => (1..5).all(|index| values[index].is_string()),
            (Some("closed" | "close"), _) => values[1].is_u64(),
            (Some("webview"), Some("attach")) => {
                values[2].is_null() && data["src"].is_string() && data["partition"].is_string()
            }
            (Some("webview"), Some("bounds")) => {
                values[2].is_u64()
                    && ["x", "y", "width", "height"]
                        .iter()
                        .all(|key| data["bounds"][key].is_number())
                    && data["visible"].is_boolean()
            }
            (Some("webview"), Some("detach")) => values[2].is_u64(),
            (Some("webview"), Some("call")) => {
                values[2].is_u64()
                    && data["args"].is_array()
                    && (data["reply"].is_null() || data["reply"].is_u64())
                    && matches!(
                        data["method"].as_str(),
                        Some(
                            "loadURL"
                                | "getURL"
                                | "getTitle"
                                | "isLoading"
                                | "canGoBack"
                                | "canGoForward"
                                | "goBack"
                                | "goForward"
                                | "reload"
                                | "stop"
                                | "focus"
                                | "openDevTools"
                                | "setZoomFactor"
                                | "getZoomFactor"
                                | "setZoomLevel"
                                | "getZoomLevel"
                                | "executeJavaScript"
                                | "insertCSS"
                                | "getUserAgent"
                        )
                    )
            }
            _ => false,
        };
        anyhow::ensure!(valid, "Invalid window request");
        let link = host.try_borrow().context("Host is busy")?.link.clone();
        Ok(link
            .context("The main process is unavailable")?
            .ask(|answer| crate::host::link::ToMain::Open(id, args.clone(), answer))?
            .into_value())
    } else if let (Role::Renderer(_), "dom.clipboard") = (role, op) {
        // `execCommand` checks for user input before it calls this.
        Ok(CLIPBOARD
            .get()
            .and_then(|clipboard| clipboard(values[0].as_str().map(str::to_owned)))
            .map_or(serde_json::Value::Null, serde_json::Value::String))
    } else {
        host.try_borrow_mut()
            .context("Host is busy")?
            .call(role, op, args)
    }
}

// Account for native DOM memory so removed wrappers trigger GC before row churn exhausts memory.
fn report_nodes(binding: &Binding, ctx: Ptr, id: u64) {
    let created = binding
        .host
        .try_borrow_mut()
        .ok()
        .and_then(|mut host| Some(std::mem::take(&mut host.dom.get_mut(&id)?.created)));
    if let Some(created) = created.filter(|&created| created > 0) {
        // Count only wrapped nodes: JSC resets this cost only after a full GC.
        binding.api.add_memory(ctx, created * 65536);
    }
}

fn gpu(binding: &Binding, ctx: Ptr, id: u64, op: &str, args: &[u8]) -> Result<serde_json::Value> {
    let mut host = binding.host.try_borrow_mut().context("Host is busy")?;
    let result = host.gpu(id, op, args);
    // JSC cannot see GPU memory, so wrappers that own it would wait for an unrelated GC.
    if let Some(gpu) = host.graphics.get_mut(&id)
        && let bytes = gpu.take_allocated()
        && bytes > 0
    {
        drop(host);
        binding.api.add_memory(ctx, bytes as usize);
    }
    result
}

// Makes the buffer of these bytes, or the object that they stand for. The flag tells if it is a buffer.

static CWD: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

impl Host {
    fn app_dir(&self, name: &str) -> Result<PathBuf> {
        if let Some(path) = self.paths.get(name) {
            return Ok(path.clone());
        }
        let home = PathBuf::from(
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .unwrap_or_default(),
        );
        Ok(match name {
            "home" => home,
            "temp" => std::env::temp_dir(),
            "exe" => std::env::current_exe()?,
            "desktop" => home.join("Desktop"),
            "documents" => home.join("Documents"),
            "downloads" => home.join("Downloads"),
            "music" => home.join("Music"),
            "pictures" => home.join("Pictures"),
            "videos" if cfg!(target_os = "macos") => home.join("Movies"),
            "videos" => home.join("Videos"),
            "appData" if cfg!(target_os = "macos") => home.join("Library/Application Support"),
            "appData" if cfg!(windows) => std::env::var_os("APPDATA").map_or(home, PathBuf::from),
            "appData" => std::env::var_os("XDG_CONFIG_HOME")
                .map_or_else(|| home.join(".config"), PathBuf::from),
            "userData" => self
                .app_dir("appData")?
                .join(self.app.name.as_deref().unwrap_or("apps")),
            "sessionData" => self.app_dir("userData")?,
            "logs" if cfg!(target_os = "macos") => home
                .join("Library/Logs")
                .join(self.app.name.as_deref().unwrap_or("apps")),
            "logs" => self.app_dir("userData")?.join("logs"),
            _ => bail!("Unsupported app path: {name}"),
        })
    }

    pub fn gpu(&mut self, id: u64, op: &str, args: &[u8]) -> Result<Value> {
        ensure!(
            self.app.features.has(crate::bundle::Feature::WebGpu),
            "The webgpu feature is disabled"
        );
        let page = self.pages.get_mut(&id).context("Window is closed")?;
        let result = self.graphics.entry(id).or_default().call(op, args)?;
        if op == "resetCanvas"
            && let Ok([node]) = serde_json::from_slice::<[usize; 1]>(args)
            && let Some(state) = self.dom.get_mut(&id)
        {
            state.images.remove(&node);
        }
        if matches!(op, "submit" | "writeTexture" | "resetCanvas") {
            page.dirty = true;
        }
        Ok(result)
    }

    pub fn media(&mut self, id: u64, op: &str, args: &Value) -> Result<Value> {
        let features = self.app.features;
        let page = self.pages.get_mut(&id).context("Window is closed")?;
        let state = self.dom.entry(id).or_default();
        let media = self.media.entry(id).or_default();
        if op == "poll" {
            let events: Value = serde_json::from_slice(&media.poll())?;
            for event in events.as_array().into_iter().flatten() {
                if event[1] == "frame"
                    && let Some(node) = event[0].as_u64()
                    && let Some(image) = media.frame(node)
                    && let Some(old) = crate::dom::show_frame(
                        state,
                        node as usize,
                        Some(std::sync::Arc::new(image)),
                    )
                    && let Ok(old) = std::sync::Arc::try_unwrap(old)
                {
                    media.recycle(node, old);
                }
            }
            page.dirty |= std::mem::take(&mut state.dirty);
            return Ok(events);
        }
        let node = args[0].as_u64().context("Invalid node")?;
        ensure!((node as usize) < page.doc.nodes.len(), "Invalid node id");
        match op {
            "open" => {
                let video = args[2].as_bool().unwrap_or(false);
                let ffmpeg = features.has(crate::bundle::Feature::Media);
                ensure!(
                    ffmpeg || !video,
                    "The media feature is disabled. Add \"media\" to \"features\" in package.json"
                );
                if args[1].is_string() {
                    let path = crate::bundle::local_file(
                        &self.app.assets,
                        state.base.as_ref(),
                        args[1].as_str().context("Expected a URL")?,
                    )?
                    .context("Media is not a bundled asset or local file")?;
                    media.open(node, Ok(&path), video, ffmpeg)?;
                } else {
                    media.open(node, Err(crate::node::bytes(&args[1])?), video, ffmpeg)?;
                }
            }
            "control" => media.control(
                node,
                args[1].as_u64().context("Expected a media operation")? as u32,
                args[2].as_f64().unwrap_or(0.),
            ),
            "time" => return Ok(json!(media.time(node))),
            "close" => {
                media.close(node);
                crate::dom::show_frame(state, node as usize, None);
                page.dirty |= std::mem::take(&mut state.dirty);
            }
            _ => bail!("Unknown media operation: {op}"),
        }
        Ok(Value::Null)
    }

    pub fn call(&mut self, role: Role, op: &str, args: Value) -> Result<Value> {
        let args = &args;
        if op == "log" {
            let text = args[0].as_str().unwrap_or("");
            let _ = if matches!(args[1].as_str(), Some("error" | "warn")) {
                writeln!(std::io::stderr(), "{text}")
            } else {
                writeln!(std::io::stdout(), "{text}")
            };
            if !self.app.packaged {
                let text: String = text.chars().take(16384).collect();
                self.log(json!({"method":"Runtime.consoleAPICalled","params":{"type":args[1].as_str().unwrap_or("log"),"text":text,"context":role,"ref":args[2]}}));
            }
            return Ok(Value::Null);
        }
        if let Role::Renderer(id) = role {
            ensure!(
                op.starts_with("dom."),
                "Host operation is unavailable in page scripts: {op}"
            );
            if op == "dom.navigate" {
                ensure!(self.pages.contains_key(&id), "Window is closed");
                let url = args[0].as_str().context("Expected navigation URL")?;
                ensure!(
                    url.starts_with("apps:///") || url.starts_with("file:///"),
                    "Pages can only navigate to bundled or local files"
                );
                self.command(
                    Target::Window(id),
                    json!({"op":"window", "args":["loadFile",id,url,"page"]}),
                );
                return Ok(Value::Null);
            }
            if op == "dom.fullscreen" {
                ensure!(self.pages.contains_key(&id), "Window is closed");
                self.command(
                    Target::Window(id),
                    json!({"op":"window", "args":["htmlFullscreen",id,args[0] == true]}),
                );
                return Ok(Value::Null);
            }
            if op == "dom.executed" {
                ensure!(
                    args[0].is_u64() && args[1].is_string(),
                    "Invalid script result"
                );
                self.command(
                    Target::Window(id),
                    json!({"op":"window", "args":["executed",id,args[0],args[1]]}),
                );
                return Ok(Value::Null);
            }
            let page = self.pages.get_mut(&id).context("Window is closed")?;
            if op == "dom.viewport" {
                let viewport = self.viewports.get(&id).unwrap_or(&[800., 600.]);
                return Ok(json!([
                    viewport[0].round(),
                    viewport[1].round(),
                    self.dom
                        .get(&id)
                        .map(|state| state.pixel_ratio)
                        .filter(|&ratio| ratio > 0.)
                        .unwrap_or(1.),
                    crate::native::theme_state()
                ]));
            }
            let state = self.dom.entry(id).or_default();
            if id == WORKER_PAGE
                && matches!(op, "dom.canvas" | "dom.measureText")
                && state.text_system.is_none()
            {
                state.text_system = Some(std::rc::Rc::new(renderer::TextSystem::new()));
            }
            // Workers use timer-based animation frames even when their canvas creates a text system.
            if id == WORKER_PAGE && op == "dom.requestFrame" {
                return Ok(json!(false));
            }
            if op == "dom.canvasCommit" {
                let node = args[0].as_u64().context("Expected node id")? as usize;
                let image = if let Some(canvas) = state.canvases.get(&node) {
                    state
                        .placeholders
                        .contains_key(&node)
                        .then(|| crate::render::canvas::render_image(&canvas.pixels, Vec::new()))
                } else if let Some(gpu) = self.graphics.get_mut(&id) {
                    let start = std::time::Instant::now();
                    loop {
                        match gpu.image(node)? {
                            Some(image)
                                if state
                                    .images
                                    .get(&node)
                                    .is_none_or(|old| !std::sync::Arc::ptr_eq(old, &image)) =>
                            {
                                state.images.insert(node, image.clone());
                                break Some(image);
                            }
                            _ if args[1] != true
                                || start.elapsed() >= std::time::Duration::from_millis(100) =>
                            {
                                break None;
                            }
                            _ => std::thread::sleep(std::time::Duration::from_millis(1)),
                        }
                    }
                } else {
                    None
                };
                if let (Some(image), Some(placeholder)) = (image, state.placeholders.get(&node)) {
                    placeholder.show(image);
                }
                return Ok(Value::Null);
            }
            if op == "dom.screen" {
                let (display, work, frame) = (
                    state.window.display,
                    state.window.work_area,
                    state.window.frame,
                );
                let viewport = self.viewports.get(&id).unwrap_or(&[800., 600.]);
                return Ok(if f32::from(display.size.width) > 0. {
                    json!(
                        [
                            display.size.width,
                            display.size.height,
                            work.origin.x,
                            work.origin.y,
                            work.size.width,
                            work.size.height,
                            frame.origin.x,
                            frame.origin.y,
                            frame.size.width,
                            frame.size.height
                        ]
                        .map(|value| f32::from(value).round())
                    )
                } else {
                    json!([
                        viewport[0],
                        viewport[1],
                        0,
                        0,
                        viewport[0],
                        viewport[1],
                        0,
                        0,
                        viewport[0],
                        viewport[1]
                    ])
                });
            }
            if op == "dom.fonts" {
                crate::dom::styles(
                    &mut page.doc,
                    state,
                    *self.viewports.get(&id).unwrap_or(&[800., 600.]),
                );
                let loaded = crate::render::fonts::load(&page.doc, state, &self.app.assets);
                page.dirty |= std::mem::take(&mut state.dirty);
                return Ok(json!(loaded));
            }
            if op == "dom.imageURL" {
                let node = args[0].as_u64().context("Invalid node")? as usize;
                ensure!(node < page.doc.nodes.len(), "Invalid node id");
                let path = crate::bundle::local_file(
                    &self.app.assets,
                    state.base.as_ref(),
                    args[1].as_str().context("Expected a URL")?,
                )?
                .context("Image is not a bundled asset or local file")?;
                return Ok(crate::dom::load_image(state, node, move || {
                    let metadata = std::fs::metadata(&path)?;
                    ensure!(
                        metadata.is_file() && metadata.len() <= 64 * 1024 * 1024,
                        "Image is not a file or is too large"
                    );
                    Ok(std::fs::read(&path)?)
                }));
            }
            if op == "dom.font"
                || op == "dom.html"
                    && args[0]
                        .as_u64()
                        .is_some_and(|id| reaches(&page.doc, &state.hosts, id as usize, 0))
            {
                state.invalidate();
            }
            let result = crate::dom::call(
                op,
                args,
                &mut page.doc,
                state,
                *self.viewports.get(&id).unwrap_or(&[800., 600.]),
            );
            page.dirty |= std::mem::take(&mut state.dirty);
            // Clear retired worker images here: workers have no renderer to release them.
            if id == WORKER_PAGE {
                state.retired.clear();
            }
            if op == "dom.release"
                && let Ok(ids) = &result
            {
                for node in ids[0].as_array().into_iter().flatten() {
                    if let Some(gpu) = self.graphics.get_mut(&id) {
                        let _ = gpu.call("resetCanvas", format!("[{node}]").as_bytes());
                    }
                    if let (Some(media), Some(node)) = (self.media.get_mut(&id), node.as_u64()) {
                        media.close(node);
                    }
                    if let (Some(state), Some(node)) = (self.dom.get(&id), node.as_u64()) {
                        state.terminals.borrow_mut().release(node as usize);
                    }
                }
            }
            return result;
        }
        match op {
            "process.write" => {
                let bytes = crate::node::bytes(&args[1])?;
                if args[0] == 2 {
                    std::io::stderr().write_all(bytes)?;
                } else {
                    std::io::stdout().write_all(bytes)?;
                }
                Ok(Value::Null)
            }
            "process.exit" => {
                let code = args[0].as_f64().unwrap_or(0.) as i32;
                if crate::web::worker::exit(code) {
                    return Ok(Value::Null);
                }
                ensure!(
                    matches!(role, Role::Main),
                    "process.exit() is only available in the main process and workers"
                );
                if let Some(before) = crate::host::context::BEFORE_EXIT.get() {
                    before();
                }
                let _ = std::io::stdout().flush();
                let _ = std::io::stderr().flush();
                std::process::exit(code)
            }
            "process.kill" => {
                #[cfg(unix)]
                {
                    let pid = args[0].as_i64().context("Expected PID")?;
                    ensure!(
                        pid != 0 && pid >= i32::MIN as i64 && pid <= i32::MAX as i64,
                        "Invalid PID"
                    );
                    let signal = match (args[1].as_i64(), args[1].as_str()) {
                        (Some(signal), _) => i32::try_from(signal)?,
                        (_, Some(name)) => {
                            crate::node::SIGNALS
                                .iter()
                                .find(|(signal, _)| *signal == name)
                                .context("Unknown signal")?
                                .1
                        }
                        _ => bail!("Unknown signal"),
                    };
                    if unsafe { libc::kill(pid as i32, signal) } != 0 {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    Ok(json!(true))
                }
                #[cfg(not(unix))]
                bail!("process.kill is unavailable on this platform")
            }
            "app.path" => Ok(json!(
                self.app_dir(args[0].as_str().context("Expected path name")?)?
            )),
            "app.setPath" => {
                let name = args[0].as_str().context("Expected path name")?;
                let path = PathBuf::from(args[1].as_str().context("Expected path")?);
                self.app_dir(name)?;
                ensure!(path.is_absolute(), "Path must be absolute");
                self.paths.insert(name.to_owned(), path);
                Ok(Value::Null)
            }
            "app.relaunch" => {
                ensure!(
                    matches!(role, Role::Main),
                    "Only the main context can relaunch the app"
                );
                let command: Vec<String> = serde_json::from_value(args[0].clone())
                    .ok()
                    .filter(|command: &Vec<String>| !command.is_empty())
                    .context("Expected relaunch command")?;
                if self.app.packaged {
                    self.relaunch = Some(command);
                } else {
                    eprintln!("apps: app.relaunch() only works in built apps");
                }
                Ok(Value::Null)
            }
            "info" => {
                let system = ["LC_ALL", "LC_MESSAGES", "LANG"]
                    .iter()
                    .find_map(|name| {
                        std::env::var(name)
                            .ok()
                            .filter(|value| !matches!(value.as_str(), "" | "C" | "POSIX"))
                    })
                    .map_or("en-US".into(), |value| {
                        value
                            .split(['.', '@'])
                            .next()
                            .unwrap_or("")
                            .replace('_', "-")
                    });
                let languages: Vec<_> = std::env::var("LANGUAGE")
                    .unwrap_or_default()
                    .split(':')
                    .filter(|value| !value.is_empty())
                    .map(|value| value.replace('_', "-"))
                    .collect();
                let locale = json!({"system":system,"languages":if languages.is_empty() {vec![system.clone()]} else {languages}});
                let lossy = |value: std::ffi::OsString| value.to_string_lossy().into_owned();
                let node = self.argv.is_some();
                let mut argv: Vec<_> = self
                    .argv
                    .clone()
                    .unwrap_or_else(|| std::env::args_os().map(lossy).collect());
                if !self.app.packaged
                    && !node
                    && let Some(end) = argv.iter().position(|arg| arg == "--")
                {
                    argv.drain(1..=end);
                }
                let env: HashMap<_, _> = std::env::vars_os()
                    .map(|(name, value)| (lossy(name), lossy(value)))
                    .collect();
                Ok(
                    json!({"node":node,"rustMain":self.app.rust_main,"platform":if cfg!(target_os="macos"){"darwin"}else if cfg!(windows){"win32"}else{"linux"},"arch":if cfg!(target_arch="aarch64"){"arm64"}else if cfg!(target_arch="x86_64"){"x64"}else{std::env::consts::ARCH},"execPath":std::env::current_exe().ok(),"locale":locale,"appPath":self.app.path,"version":crate::VERSION,"javascriptcore":if std::env::var_os("APPS_JSC_LIBRARY").is_some(){"custom"}else{"bun"},"isPackaged":self.app.packaged,"appName":self.app.name.as_deref().unwrap_or("apps"),"appVersion":self.app.version.as_deref().unwrap_or("0.0.0"),"pid":std::process::id(),"argv":argv,"env":env,"homedir":std::env::var(if cfg!(windows){"USERPROFILE"}else{"HOME"}).unwrap_or_default(),"tmpdir":std::env::temp_dir(),"hostname":crate::node::hostname(),"parallelism":std::thread::available_parallelism().map_or(1,usize::from)}),
                )
            }
            // getcwd takes about 9 µs on macOS, so like Node the process keeps the path until `chdir`.
            "cwd" => {
                let mut cwd = CWD.lock().unwrap_or_else(|error| error.into_inner());
                if cwd.is_none() {
                    *cwd = Some(std::env::current_dir()?);
                }
                Ok(json!(*cwd))
            }
            "chdir" => {
                // The cwd belongs to the whole process, and preloads and workers are threads in it.
                ensure!(
                    matches!(role, Role::Main) && !crate::web::worker::in_worker(),
                    "process.chdir() is only available in the main process"
                );
                let mut cwd = CWD.lock().unwrap_or_else(|error| error.into_inner());
                *cwd = None;
                std::env::set_current_dir(args[0].as_str().context("Expected path")?)?;
                Ok(Value::Null)
            }
            "window" | "quit" | "focus" => {
                ensure!(
                    matches!(role, Role::Main),
                    "Only the main context can control windows"
                );
                if self.app.packaged && op == "window" && args[0] == "devtools" {
                    return Ok(Value::Null);
                }
                let target = match op {
                    "quit" => {
                        if let Some(link) = &self.link {
                            link.quit(args[0].as_i64().unwrap_or(0) as i32);
                            return Ok(Value::Null);
                        }
                        Target::Main
                    }
                    "focus" => Target::Main,
                    _ => window::route(args)?,
                };
                self.command(target, json!({"op": op, "args": args}));
                Ok(Value::Null)
            }
            _ => bail!("Unsupported host operation: {op}"),
        }
    }
}
