use super::{Desktop, main::state_json, png};
use crate::{host::link::Link, jsc, jsc::Role};
use anyhow::{Context as _, Result, anyhow, bail};
use renderer::{App, Button, Event, Key, Modifiers, WheelPhase, WindowId, point, px};
use serde_json::{Value, json};
use std::{
    ffi::c_void,
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::Instant,
};

// The reply, ok, JSON value or error text, and an optional PNG.
type Done = extern "C" fn(*mut c_void, bool, *const u8, usize, *const u8, usize);

#[repr(C)]
struct Host {
    connect: extern "C" fn(usize) -> i32,
    inspect: extern "C" fn(usize, *const u8, usize) -> bool,
    messages: extern "C" fn(*mut c_void, extern "C" fn(*mut c_void, usize, *const u8, usize)),
    contexts: extern "C" fn(*mut c_void, extern "C" fn(*mut c_void, usize, u32, u64)),
    wake: extern "C" fn(u64),
    call: extern "C" fn(*mut c_void, *const u8, usize, *const u8, usize, *mut c_void, Done),
}

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
    let _ = LINK.set(link);
    crate::native::devtools::start(port, &HOST)
}

pub(crate) fn poll(desktop: &Desktop, cx: &mut App) -> bool {
    let mut app = (desktop, cx);
    crate::native::devtools::poll((&raw mut app).cast())
}

extern "C" fn connect(ctx: usize) -> i32 {
    crate::host::connect_inspector(ctx).map_or(-1, i32::from)
}

extern "C" fn inspect(ctx: usize, message: *const u8, len: usize) -> bool {
    let message = unsafe { std::slice::from_raw_parts(message, len) };
    crate::host::inspect(ctx, &String::from_utf8_lossy(message)).is_ok()
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
    for (ctx, role) in crate::host::contexts() {
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
        Some(link) => link.wake_page(key),
        None => {}
    }
}

struct Slot(*mut c_void, Done);

unsafe impl Send for Slot {}

// Always reply, even on error: the library waits for each call.
impl Drop for Slot {
    fn drop(&mut self) {
        let text = "The window closed";
        (self.1)(
            self.0,
            false,
            text.as_ptr(),
            text.len(),
            std::ptr::null(),
            0,
        );
    }
}

#[derive(Clone)]
struct Reply(Arc<Mutex<Option<Slot>>>);

impl Reply {
    fn send(&self, result: Result<Value>) {
        self.send_image(result.map(|value| (value, Vec::new())));
    }

    fn send_image(&self, result: Result<(Value, Vec<u8>)>) {
        if let Some(slot) = self.0.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let (ok, data, image) = match result {
                Ok((value, image)) => (true, value.to_string(), image),
                Err(error) => (false, format!("{error:#}"), Vec::new()),
            };
            let image_ptr = if image.is_empty() {
                std::ptr::null()
            } else {
                image.as_ptr()
            };
            (slot.1)(
                slot.0,
                ok,
                data.as_ptr(),
                data.len(),
                image_ptr,
                image.len(),
            );
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
        (b"windows", Some((desktop, cx))) => Ok(Some(desktop.windows(cx))),
        (b"screenshot", Some((desktop, cx))) => screenshot(&args, desktop, cx, &reply),
        (b"input", Some((desktop, cx))) => input(&args, desktop, cx, &reply),
        (b"memory", Some((desktop, _))) => desktop.memory(args["gc"] == true).map(Some),
        _ => Err(anyhow!("Unknown devtools operation")),
    };
    match result {
        Ok(Some(value)) => reply.send(Ok(value)),
        Ok(None) => {}
        Err(error) => reply.send(Err(error)),
    }
}

impl Desktop {
    // The window `id`, or the first one, and its native handle.
    fn window(&self, id: Option<u64>) -> Result<(u64, WindowId)> {
        let window = match id {
            Some(id) => id,
            None => self
                .views
                .keys()
                .copied()
                .min()
                .context("The app has no windows")?,
        };
        let handle = self
            .views
            .get(&window)
            .and_then(|view| view.handle)
            .context("No such window")?;
        Ok((window, handle))
    }

    fn windows(&self, cx: &mut App) -> Value {
        let host = self.host.borrow();
        let mut ids: Vec<_> = self.views.keys().copied().collect();
        ids.sort_unstable();
        let mut list = Vec::new();
        for id in ids {
            let view = &self.views[&id];
            let state = view.handle.and_then(|handle| {
                cx.update_window(handle, |window, _| {
                    state_json(&window.window_state(), window.is_active())
                })
            });
            list.push(json!({
                "id": id,
                "title": view.title,
                "url": host.url(id).map(url::Url::as_str),
                "state": state,
                "preload": view.preload.is_some(),
            }));
        }
        json!(list)
    }

    fn memory(&self, gc: bool) -> Result<Value> {
        let engine = self
            .views
            .iter()
            .filter_map(|(&id, view)| view.renderer.as_deref().map(|engine| (id, engine)))
            .min_by_key(|(id, _)| *id)
            .map(|(_, engine)| engine);
        if gc && let Some(engine) = engine {
            engine.collect_garbage();
        }
        let heap = engine
            .map(|engine| engine.heap())
            .transpose()?
            .unwrap_or(Value::Null);
        Ok(json!({
            "rss": crate::node::system::call("process.memory", &Value::Null)?,
            "heap": heap,
            "domNodes": self.host.borrow().memory(),
        }))
    }
}

fn screenshot(
    args: &Value,
    desktop: &Desktop,
    cx: &mut App,
    reply: &Reply,
) -> Result<Option<Value>> {
    let (window, handle) = desktop.window(args["window"].as_u64())?;
    let (crop, css, reply) = (
        args["crop"].clone(),
        args["device_pixels"] != true,
        reply.clone(),
    );
    cx.update_window(handle, |view, _| {
        let scale = view.scale_factor();
        view.capture(move |frame, _| {
            let result = frame.and_then(|frame| png(frame, scale, &crop, css));
            reply.send_image(result.map(|([width, height], bytes)| {
                (
                    format!("Window {window}, {width}x{height} pixels").into(),
                    bytes,
                )
            }));
        });
    })
    .context("The window closed")?;
    Ok(None)
}

fn input(args: &Value, desktop: &Desktop, cx: &mut App, reply: &Reply) -> Result<Option<Value>> {
    let (window, handle) = desktop.window(args["window"].as_u64())?;
    let action = args["action"].as_str().context("Expected an action")?;
    let at = point(
        px(args["x"].as_f64().unwrap_or(0.) as f32),
        px(args["y"].as_f64().unwrap_or(0.) as f32),
    );
    let button = match args["button"].as_str() {
        Some("right") => Button::Right,
        Some("middle") => Button::Middle,
        _ => Button::Left,
    };
    let count = args["count"].as_u64().unwrap_or(1).max(1) as usize;
    let modifiers = Modifiers::NONE;
    let moved = |at, held| Event::PointerMove {
        at,
        held,
        modifiers,
    };
    let down = |at, clicks| Event::PointerDown {
        at,
        button,
        modifiers,
        clicks,
    };
    let up = |at, clicks| Event::PointerUp {
        at,
        button,
        modifiers,
        clicks,
    };
    let mut events = Vec::new();
    let mut keys = Vec::new();
    match action {
        "click" => {
            events.push(moved(at, None));
            for clicks in 1..=count {
                events.push(down(at, clicks));
                events.push(up(at, clicks));
            }
        }
        "move" => events.push(moved(at, None)),
        "drop" => {
            let paths = args["paths"]
                .as_array()
                .context("Expected paths")?
                .iter()
                .filter_map(|path| path.as_str().map(std::path::PathBuf::from))
                .collect();
            events.push(Event::FileEnter { at, paths });
            let end = point(
                px(args["to_x"].as_f64().map_or(at.x.0, |x| x as f32)),
                px(args["to_y"].as_f64().map_or(at.y.0, |y| y as f32)),
            );
            events.push(Event::FileOver { at: end });
            if args["hold"].as_bool() != Some(true) {
                events.push(if args["leave"].as_bool() == Some(true) {
                    Event::FileLeave
                } else {
                    Event::FileDrop { at: end }
                });
            }
        }
        "press" => events.push(down(at, 1)),
        "pressed_move" => events.push(moved(at, Some(button))),
        "release" => events.push(up(at, 1)),
        "drag" => {
            let end = point(
                px(args["to_x"].as_f64().context("Expected to_x")? as f32),
                px(args["to_y"].as_f64().context("Expected to_y")? as f32),
            );
            events.push(moved(at, None));
            events.push(down(at, 1));
            for step in 1..=8 {
                events.push(moved(at + (end - at) * (step as f32 / 8.), Some(button)));
            }
            events.push(up(end, 1));
        }
        "scroll" => events.push(Event::Wheel {
            at,
            delta: point(
                -args["delta_x"].as_f64().unwrap_or(0.) as f32,
                -args["delta_y"].as_f64().unwrap_or(0.) as f32,
            ),
            lines: args["lines"].as_bool() == Some(true),
            phase: match args["phase"].as_str() {
                Some("began") => WheelPhase::Began,
                Some("changed") => WheelPhase::Changed,
                Some("ended") => WheelPhase::Ended,
                Some("momentum") => WheelPhase::Momentum,
                Some("momentum_ended") => WheelPhase::MomentumEnded,
                _ => WheelPhase::None,
            },
            modifiers,
        }),
        "key" => {
            let key = args["key"].as_str().context("Expected a key")?;
            for part in key.split_whitespace() {
                keys.push(Key::parse(part).with_context(|| format!("Invalid key: {part}"))?);
            }
        }
        "type" => {
            for character in args["text"].as_str().context("Expected text")?.chars() {
                let text = character.to_string();
                let name = text.to_lowercase();
                keys.push((
                    Key {
                        name,
                        text: Some(text),
                    },
                    modifiers,
                ));
            }
        }
        _ => bail!("Unknown input action: {action}"),
    }
    // A scroll with a phase moves like a trackpad gesture, spread over frames.
    let gesture = action == "scroll" && args["phase"].is_string();
    let reply = reply.clone();
    let sent = format!("Sent {action} to window {window}");
    // Input handlers borrow the desktop, which `Desktop::drive` holds now.
    cx.defer(move |cx| {
        let result = cx.update_window(handle, |window, cx| {
            for event in events {
                match gesture {
                    true => window.dispatch_event_at(event, Instant::now(), cx),
                    false => window.dispatch_event(event, cx),
                };
            }
            for (key, modifiers) in keys {
                window.press_key(key, modifiers, cx);
            }
        });
        reply.send(result.map(|_| sent.into()).context("The window closed"));
    });
    Ok(None)
}

#[cfg(test)]
mod tests {
    use crate::{
        host::Host,
        host::JsContext,
        jsc::Role,
        jsc::{self, JavaScriptCore},
    };

    #[test]
    fn inspector_evaluates_pauses_and_disconnects() {
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            std::rc::Rc::default(),
            Role::Main,
        )
        .unwrap();
        let [(ctx, Role::Main)] = crate::host::contexts()[..] else {
            panic!("Expected one main context");
        };
        assert!(crate::host::connect_inspector(ctx).unwrap());
        for message in [
            r#"{"id":1,"method":"Runtime.evaluate","params":{"expression":"6 * 7","returnByValue":true}}"#,
            r#"{"id":2,"method":"Debugger.enable","params":{}}"#,
            r#"{"id":3,"method":"Debugger.setPauseOnDebuggerStatements","params":{"enabled":true}}"#,
        ] {
            crate::host::inspect(ctx, message).unwrap();
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
        assert!(crate::host::contexts().is_empty());
        let _ = Host::default();
    }
}
