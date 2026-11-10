use super::{Desktop, link::Link, png, window_state};
use crate::{host::Role, jsc};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use renderer::{
    App, FileDropEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PlatformInput, ScrollDelta, ScrollPhase, ScrollWheelEvent, WindowId, point, px,
};
use serde_json::{Value, json};
use std::{
    ffi::{CStr, c_char, c_void},
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::Instant,
};

const ABI: u32 = 1;

type Done = extern "C" fn(*mut c_void, bool, *const u8, usize);

#[repr(C)]
struct Host {
    connect: extern "C" fn(usize) -> i32,
    inspect: extern "C" fn(usize, *const u8, usize) -> bool,
    messages: extern "C" fn(*mut c_void, extern "C" fn(*mut c_void, usize, *const u8, usize)),
    contexts: extern "C" fn(*mut c_void, extern "C" fn(*mut c_void, usize, u32, u64)),
    wake: extern "C" fn(u64),
    call: extern "C" fn(*mut c_void, *const u8, usize, *const u8, usize, *mut c_void, Done),
}

struct Library {
    install: extern "C" fn(u64),
    uninstall: extern "C" fn(),
    poll: extern "C" fn(*mut c_void) -> bool,
    paused: unsafe extern "C" fn(usize, *mut bool),
    logs: unsafe extern "C" fn(*const u8, usize),
    profiling: extern "C" fn() -> bool,
    frame: extern "C" fn(u64, u64),
}

static LIBRARY: OnceLock<Library> = OnceLock::new();
static LINK: OnceLock<Link> = OnceLock::new();
static HOST: Host = Host {
    connect,
    inspect,
    messages,
    contexts,
    wake,
    call,
};

pub(crate) fn start(port: u16, link: Link) -> Result<()> {
    let directory = super::app::library_exe()?
        .parent()
        .context("Missing executable directory")?
        .to_owned();
    let name = libloading::library_filename("apps_mcp");
    let path = [".", "deps"]
        .iter()
        .map(|folder| directory.join(folder).join(&name))
        .find(|path| path.is_file())
        .with_context(|| {
            format!(
                "The devtools MCP server {} is missing next to the runtime. Build the workspace with cargo build --workspace.",
                name.display()
            )
        })?;
    let (start, library) = unsafe {
        let library = libloading::Library::new(&path).with_context(|| {
            format!(
                "Cannot load the devtools MCP server from {}",
                path.display()
            )
        })?;
        let abi: libloading::Symbol<extern "C" fn() -> u32> = library.get(b"apps_mcp_abi\0")?;
        ensure!(
            abi() == ABI,
            "{} does not match this runtime",
            path.display()
        );
        let start: extern "C" fn(u16, &'static Host) -> *const c_char =
            *library.get(b"apps_mcp_start\0")?;
        let functions = Library {
            install: *library.get(b"apps_mcp_install\0")?,
            uninstall: *library.get(b"apps_mcp_uninstall\0")?,
            poll: *library.get(b"apps_mcp_poll\0")?,
            paused: *library.get(b"apps_mcp_paused\0")?,
            logs: *library.get(b"apps_mcp_logs\0")?,
            profiling: *library.get(b"apps_mcp_profiling\0")?,
            frame: *library.get(b"apps_mcp_frame\0")?,
        };
        // The server threads run until the app quits, so the library is never unloaded.
        std::mem::forget(library);
        (start, functions)
    };
    let _ = LINK.set(link);
    let error = start(port, &HOST);
    ensure!(
        error.is_null(),
        "{}",
        unsafe { CStr::from_ptr(error) }.to_string_lossy()
    );
    let _ = LIBRARY.set(library);
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
    LIBRARY
        .get()
        .is_some_and(|library| (library.poll)(std::ptr::null_mut()))
}

pub(crate) fn poll(desktop: &Desktop, cx: &mut App) -> bool {
    let mut app = (desktop, cx);
    LIBRARY
        .get()
        .is_some_and(|library| (library.poll)((&raw mut app).cast()))
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

// Runs while JSC is paused in `ctx`. It must dispatch protocol messages until `done` is set.
pub(crate) fn paused(ctx: usize, done: *mut bool) {
    match LIBRARY.get() {
        Some(library) => unsafe { (library.paused)(ctx, done) },
        None => drop(jsc::inspect(ctx, r#"{"id":0,"method":"Debugger.resume"}"#)),
    }
}

extern "C" fn connect(ctx: usize) -> i32 {
    jsc::connect_inspector(ctx).map_or(-1, i32::from)
}

extern "C" fn inspect(ctx: usize, message: *const u8, len: usize) -> bool {
    let message = unsafe { std::slice::from_raw_parts(message, len) };
    jsc::inspect(ctx, &String::from_utf8_lossy(message)).is_ok()
}

extern "C" fn messages(
    list: *mut c_void,
    push: extern "C" fn(*mut c_void, usize, *const u8, usize),
) {
    for (ctx, message) in jsc::inspector_messages() {
        push(list, ctx, message.as_ptr(), message.len());
    }
}

extern "C" fn contexts(list: *mut c_void, push: extern "C" fn(*mut c_void, usize, u32, u64)) {
    for (ctx, role) in jsc::contexts() {
        let (role, id) = match role {
            Role::Main => (0, 0),
            Role::Preload(id) => (1, id),
            Role::Renderer(id) => (2, id),
        };
        push(list, ctx, role, id);
    }
}

extern "C" fn wake(key: u64) {
    match LINK.get() {
        Some(link) if key == u64::MAX => link.wake_main(),
        Some(link) => link.wake_page(Some(key)),
        None => {}
    }
}

struct Slot(*mut c_void, Done);

unsafe impl Send for Slot {}

// Always reply, even on error: the library waits for each call.
impl Drop for Slot {
    fn drop(&mut self) {
        let text = "The window closed";
        (self.1)(self.0, false, text.as_ptr(), text.len());
    }
}

#[derive(Clone)]
struct Reply(Arc<Mutex<Option<Slot>>>);

impl Reply {
    fn send(&self, result: Result<Value>) {
        if let Some(slot) = self.0.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let (ok, data) = match result {
                Ok(value) => (true, value.to_string()),
                Err(error) => (false, format!("{error:#}")),
            };
            (slot.1)(slot.0, ok, data.as_ptr(), data.len());
            std::mem::forget(slot);
        }
    }
}

extern "C" fn call(
    app: *mut c_void,
    op: *const u8,
    op_len: usize,
    args: *const u8,
    args_len: usize,
    reply: *mut c_void,
    done: Done,
) {
    let reply = Reply(Arc::new(Mutex::new(Some(Slot(reply, done)))));
    let op = unsafe { std::slice::from_raw_parts(op, op_len) };
    let args: Value = serde_json::from_slice(unsafe { std::slice::from_raw_parts(args, args_len) })
        .unwrap_or_default();
    let app = unsafe { app.cast::<(&Desktop, &mut App)>().as_mut() };
    let result = match (op, app) {
        (b"cpu", _) => crate::node::system::call("process.cpu", &Value::Null).map(Some),
        (b"windows", Some((desktop, cx))) => windows(desktop, cx).map(Some),
        (b"screenshot", Some((desktop, cx))) => screenshot(&args, desktop, cx, &reply),
        (b"input", Some((desktop, cx))) => input(&args, desktop, cx, &reply),
        (b"memory", Some((desktop, _))) => memory(&args, desktop).map(Some),
        _ => Err(anyhow!("Unknown devtools operation")),
    };
    match result {
        Ok(Some(value)) => reply.send(Ok(value)),
        Ok(None) => {}
        Err(error) => reply.send(Err(error)),
    }
}

fn page(args: &Value, desktop: &Desktop) -> Result<(u64, WindowId)> {
    let window = match args["window"].as_u64() {
        Some(id) => id,
        None => desktop
            .views
            .keys()
            .copied()
            .min()
            .context("The app has no windows")?,
    };
    let handle = desktop
        .views
        .get(&window)
        .and_then(|view| view.handle)
        .context("No such window")?;
    Ok((window, handle))
}

fn windows(desktop: &Desktop, cx: &mut App) -> Result<Value> {
    let host = desktop.host.borrow();
    let mut ids: Vec<_> = desktop.views.keys().copied().collect();
    ids.sort_unstable();
    let mut list = Vec::new();
    for id in ids {
        let view = &desktop.views[&id];
        let state = view
            .handle
            .and_then(|handle| cx.update_window(handle, |window, _| window_state(window)));
        list.push(json!({
            "id": id,
            "title": view.title,
            "url": host.dom.get(&id).and_then(|dom| dom.base.as_ref()).map(url::Url::as_str),
            "state": state,
            "preload": view.preload.is_some(),
        }));
    }
    Ok(json!(list))
}

fn screenshot(
    args: &Value,
    desktop: &Desktop,
    cx: &mut App,
    reply: &Reply,
) -> Result<Option<Value>> {
    let (window, handle) = page(args, desktop)?;
    let (crop, css, reply) = (
        args["crop"].clone(),
        args["device_pixels"] != true,
        reply.clone(),
    );
    cx.update_window(handle, |view, _| {
        let scale = view.scale_factor();
        view.capture(move |frame, _| {
            let result = frame.and_then(|frame| png(frame, scale, &crop, css));
            reply.send(result.map(|([width, height], bytes)| {
                json!({"image": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes), "text": format!("Window {window}, {width}x{height} pixels")})
            }));
        });
    })
    .context("The window closed")?;
    Ok(None)
}

fn input(args: &Value, desktop: &Desktop, cx: &mut App, reply: &Reply) -> Result<Option<Value>> {
    let (window, handle) = page(args, desktop)?;
    let action = args["action"].as_str().context("Expected an action")?;
    let position = point(
        px(args["x"].as_f64().unwrap_or(0.) as f32),
        px(args["y"].as_f64().unwrap_or(0.) as f32),
    );
    let button = match args["button"].as_str() {
        Some("right") => MouseButton::Right,
        Some("middle") => MouseButton::Middle,
        _ => MouseButton::Left,
    };
    let count = args["count"].as_u64().unwrap_or(1).max(1) as usize;
    let modifiers = Modifiers::default();
    let mut events = Vec::new();
    let mut keys = Vec::new();
    match action {
        "click" => {
            events.push(PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers,
            }));
            for click_count in 1..=count {
                events.push(PlatformInput::MouseDown(MouseDownEvent {
                    button,
                    position,
                    modifiers,
                    click_count,
                    first_mouse: false,
                }));
                events.push(PlatformInput::MouseUp(MouseUpEvent {
                    button,
                    position,
                    modifiers,
                    click_count,
                }));
            }
        }
        "move" => events.push(PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers,
        })),
        "drop" => {
            let paths = args["paths"]
                .as_array()
                .context("Expected paths")?
                .iter()
                .filter_map(|path| path.as_str().map(std::path::PathBuf::from))
                .collect();
            events.push(PlatformInput::FileDrop(FileDropEvent::Entered {
                position,
                paths,
            }));
            let end = point(
                px(args["to_x"].as_f64().map_or(position.x.0, |x| x as f32)),
                px(args["to_y"].as_f64().map_or(position.y.0, |y| y as f32)),
            );
            events.push(PlatformInput::FileDrop(FileDropEvent::Pending {
                position: end,
            }));
            if args["hold"].as_bool() != Some(true) {
                events.push(PlatformInput::FileDrop(
                    if args["leave"].as_bool() == Some(true) {
                        FileDropEvent::Exited
                    } else {
                        FileDropEvent::Submit { position: end }
                    },
                ));
            }
        }
        "press" => events.push(PlatformInput::MouseDown(MouseDownEvent {
            button,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        })),
        "pressed_move" => events.push(PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: Some(button),
            modifiers,
        })),
        "release" => events.push(PlatformInput::MouseUp(MouseUpEvent {
            button,
            position,
            modifiers,
            click_count: 1,
        })),
        "drag" => {
            let end = point(
                px(args["to_x"].as_f64().context("Expected to_x")? as f32),
                px(args["to_y"].as_f64().context("Expected to_y")? as f32),
            );
            events.push(PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers,
            }));
            events.push(PlatformInput::MouseDown(MouseDownEvent {
                button,
                position,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }));
            for step in 1..=8 {
                events.push(PlatformInput::MouseMove(MouseMoveEvent {
                    position: position + (end - position) * (step as f32 / 8.),
                    pressed_button: Some(button),
                    modifiers,
                }));
            }
            events.push(PlatformInput::MouseUp(MouseUpEvent {
                button,
                position: end,
                modifiers,
                click_count: 1,
            }));
        }
        "scroll" => {
            let delta = point(
                -args["delta_x"].as_f64().unwrap_or(0.) as f32,
                -args["delta_y"].as_f64().unwrap_or(0.) as f32,
            );
            events.push(PlatformInput::ScrollWheel(ScrollWheelEvent {
                position,
                delta: if args["lines"].as_bool() == Some(true) {
                    ScrollDelta::Lines(delta)
                } else {
                    ScrollDelta::Pixels(delta.map(px))
                },
                phase: match args["phase"].as_str() {
                    Some("began") => ScrollPhase::Began,
                    Some("changed") => ScrollPhase::Changed,
                    Some("ended") => ScrollPhase::Ended,
                    Some("momentum") => ScrollPhase::Momentum,
                    Some("momentum_ended") => ScrollPhase::MomentumEnded,
                    _ => ScrollPhase::None,
                },
                modifiers,
            }));
        }
        "key" => {
            let key = args["key"].as_str().context("Expected a key")?;
            for part in key.split_whitespace() {
                keys.push(Keystroke::parse(part).with_context(|| format!("Invalid key: {part}"))?);
            }
        }
        "type" => {
            for character in args["text"].as_str().context("Expected text")?.chars() {
                let text = character.to_string();
                keys.push(Keystroke {
                    modifiers,
                    key: text.to_lowercase(),
                    key_char: Some(text),
                });
            }
        }
        _ => bail!("Unknown input action: {action}"),
    }
    let reply = reply.clone();
    let sent = format!("Sent {action} to window {window}");
    // Input handlers borrow the desktop, which `Desktop::drive` holds now.
    cx.defer(move |cx| {
        let result = cx.update_window(handle, |window, cx| {
            for event in events {
                window.dispatch_event(event, cx);
            }
            for key in keys {
                window.dispatch_keystroke(key, cx);
            }
        });
        reply.send(result.map(|_| sent.into()).context("The window closed"));
    });
    Ok(None)
}

fn memory(args: &Value, desktop: &Desktop) -> Result<Value> {
    let engine = desktop
        .views
        .iter()
        .filter_map(|(&id, view)| view.renderer.as_deref().map(|engine| (id, engine)))
        .min_by_key(|(id, _)| *id)
        .map(|(_, engine)| engine);
    if args["gc"] == true
        && let Some(engine) = engine
    {
        engine.collect_garbage();
    }
    let heap = engine
        .map(|engine| engine.heap())
        .transpose()?
        .unwrap_or(Value::Null);
    let host = desktop.host.borrow();
    let report = json!({
        "rss": crate::node::system::call("process.memory", &Value::Null)?,
        "heap": heap,
        "domNodes": host.pages.iter().map(|(id, page)| {
            let (mut attached, mut pending) = (0, vec![0]);
            while let Some(node) = pending.pop() {
                attached += 1;
                pending.extend(&page.doc.nodes[node].children);
            }
            let free = host.dom.get(id).map_or(0, |state| state.free.len());
            (format!("window {id}"), json!({"attached": attached, "allocated": page.doc.nodes.len(), "free": free}))
        }).collect::<serde_json::Map<_, _>>(),
    });
    Ok(report)
}

#[cfg(test)]
mod tests {
    use crate::{
        host::{Host, Role},
        jsc::{self, JavaScriptCore, JsContext},
    };

    #[test]
    fn inspector_evaluates_pauses_and_disconnects() {
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            std::rc::Rc::default(),
            Role::Main,
        )
        .unwrap();
        let [(ctx, Role::Main)] = jsc::contexts()[..] else {
            panic!("Expected one main context");
        };
        assert!(jsc::connect_inspector(ctx).unwrap());
        for message in [
            r#"{"id":1,"method":"Runtime.evaluate","params":{"expression":"6 * 7","returnByValue":true}}"#,
            r#"{"id":2,"method":"Debugger.enable","params":{}}"#,
            r#"{"id":3,"method":"Debugger.setPauseOnDebuggerStatements","params":{"enabled":true}}"#,
        ] {
            jsc::inspect(ctx, message).unwrap();
        }
        let messages: Vec<_> = jsc::inspector_messages()
            .into_iter()
            .map(|(_, message)| message)
            .collect();
        assert!(
            messages
                .iter()
                .any(|message| message.contains(r#""id":1"#) && message.contains(r#""value":42"#))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("apps:bootstrap"))
        );
        assert_eq!(engine.eval("debugger; 'after'", "test").unwrap(), "after");
        drop(engine);
        assert!(jsc::contexts().is_empty());
        let _ = Host::default();
    }
}
