use crate::render::inspect::tool;
use crate::{
    host::context::JsContext,
    host::{
        Host,
        link::{Link, PageQueue, ToMain, ToPage},
    },
    jsc::JavaScriptCore,
    native::browser::Parent,
};
use anyhow::{Context, Result};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use renderer::{Rect, WindowChange, WindowControl, WindowId, native::WindowEvent, point, px, size};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::{Rc, Weak},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

type Job = Box<dyn FnOnce() + Send>;
type Spawner = Box<dyn Fn(u64, Arc<PageQueue>)>;
pub(crate) type Handler = Arc<dyn Fn(u64, Vec<Value>) -> Result<Value> + Send + Sync>;

#[derive(Clone, Copy)]
struct NativeHandle(RawWindowHandle);

impl HasWindowHandle for NativeHandle {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the page side reports a window's close before the view goes away.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

thread_local! { static MAIN: RefCell<Weak<Main>> = RefCell::default(); }

fn receive_main() {
    if let Some(main) = MAIN.with_borrow(Weak::upgrade) {
        main.receive_with(true);
    }
}

pub(crate) fn drive_main() {
    if let Some(main) = MAIN.with_borrow(Weak::upgrade) {
        main.drive();
    }
}

pub(crate) fn state_json(state: &renderer::WindowState, focused: bool) -> Value {
    let rect = |bounds: Rect<renderer::Pixels>| {
        [
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        ]
        .map(|value| f32::from(value).round())
    };
    json!({"frame":rect(state.frame),"content":rect(state.content),"minimized":state.minimized,"maximized":state.maximized,"fullscreen":state.fullscreen,"visible":state.visible,"focused":focused,"resizing":state.resizing})
}

#[derive(Default)]
struct Window {
    native: Option<(WindowId, WindowControl)>,
    input: bool,
    frames: bool,
}

pub(crate) struct Main {
    engine: Option<JsContext>,
    host: Rc<RefCell<Host>>,
    handlers: RefCell<HashMap<String, Handler>>,
    api: Rc<JavaScriptCore>,
    link: Link,
    pool: RefCell<Option<mpsc::Sender<Job>>>,
    windows: RefCell<HashMap<u64, Window>>,
    spawner: RefCell<Option<Spawner>>,
    waiting: RefCell<Vec<Value>>,
    changed: RefCell<Vec<u64>>,
    tools: RefCell<Option<super::panel::DevTools>>,
    wake: Cell<Option<Instant>>,
    driving: Cell<bool>,
    ended: Cell<bool>,
    quiet: Cell<bool>,
}

impl Main {
    pub(crate) fn new(
        engine: Option<JsContext>,
        host: Rc<RefCell<Host>>,
        link: Link,
        api: Rc<JavaScriptCore>,
    ) -> Rc<Self> {
        let main = Rc::new(Self {
            engine,
            host,
            handlers: RefCell::default(),
            api,
            link,
            pool: RefCell::default(),
            windows: RefCell::default(),
            spawner: RefCell::default(),
            waiting: RefCell::default(),
            changed: RefCell::default(),
            tools: RefCell::default(),
            wake: Cell::new(None),
            driving: Cell::new(false),
            ended: Cell::new(false),
            quiet: Cell::new(false),
        });
        MAIN.set(Rc::downgrade(&main));
        crate::host::link::set_receive_main(receive_main);
        main
    }

    pub(crate) fn handle(&self, channel: &str, handler: Handler) -> Result<()> {
        let mut handlers = self.handlers.borrow_mut();
        anyhow::ensure!(
            !handlers.contains_key(channel),
            "Duplicate Rust handler {channel:?}"
        );
        handlers.insert(channel.to_owned(), handler);
        Ok(())
    }

    // Starts each window on a page thread of its own.
    pub(crate) fn thread_per_window(&self, spawner: Spawner) {
        self.link.set_per_window();
        *self.spawner.borrow_mut() = Some(spawner);
    }

    pub(crate) fn ready(&self) -> Result<()> {
        if self.engine.is_some() {
            self.eval("__appsEvent('ready')", "apps:ready")?;
        }
        Ok(())
    }

    // Runs the due timers and tasks of the main script, for a check without windows.
    pub(crate) fn tick(&self) -> Result<()> {
        if let Some(engine) = &self.engine {
            engine.tick()?;
        }
        Ok(())
    }

    pub(crate) fn pending(&self) -> Result<bool> {
        Ok(match &self.engine {
            Some(engine) => engine.eval("__appsPending()", "apps:startup")? == "true",
            None => false,
        })
    }

    // An app event, like `activate` or `open-url`.
    pub(crate) fn event(&self, name: &str, data: Value, url: &'static str) {
        self.link.event(name, 0, Some(data), url);
    }

    // Asks the main script with `before-quit`, which can prevent it.
    pub(crate) fn should_quit(self: &Rc<Self>) -> bool {
        if self.engine.is_none() || self.link.quitting() {
            return true;
        }
        match self.eval("__appsEvent('before-quit')", "apps:quit") {
            Err(error) => {
                eprintln!("apps: {error}");
                true
            }
            Ok(_) => {
                self.drive();
                self.link.quitting()
            }
        }
    }

    // Starts the command that `app.relaunch()` asked for, after this process ends.
    pub(crate) fn relaunch(&self) {
        if let Ok(host) = self.host.try_borrow()
            && let Some(command) = &host.relaunch
        {
            #[cfg(unix)]
            let _ = std::os::unix::process::CommandExt::process_group(
                std::process::Command::new("/bin/sh")
                    .args([
                        "-c",
                        r#"while kill -0 "$0" 2>/dev/null; do sleep 0.1; done; exec "$@""#,
                        &std::process::id().to_string(),
                    ])
                    .args(command),
                0,
            )
            .stdin(std::process::Stdio::null())
            .spawn();
            #[cfg(not(unix))]
            let _ = std::process::Command::new(&command[0])
                .args(&command[1..])
                .spawn();
        }
    }

    #[cfg(test)]
    pub(super) fn engine(&self) -> Option<&JsContext> {
        self.engine.as_ref()
    }

    pub(crate) fn first_window(&self) -> Option<u64> {
        self.windows.borrow().keys().min().copied()
    }

    pub(crate) fn receive(&self) {
        self.receive_with(!self.driving.get());
    }

    // Nested AppKit loops must defer script messages until the outer script returns. Other messages can run.
    pub(crate) fn receive_with(&self, scripts: bool) -> bool {
        let messages = self.link.take_main();
        let busy = !messages.is_empty();
        let mut later = VecDeque::new();
        for message in messages {
            let sender = match &message {
                ToMain::Bridge(message) | ToMain::Sync(message, _) => {
                    message.value["sender"]["preload"].as_u64()
                }
                ToMain::Open(id, ..) => Some(*id),
                _ => None,
            };
            if sender.is_some_and(|id| !self.windows.borrow().contains_key(&id)) {
                continue;
            }
            if !scripts
                && self.engine.is_some()
                && matches!(
                    message,
                    ToMain::Event(..) | ToMain::Bridge(_) | ToMain::Sync(..) | ToMain::Open(..)
                )
            {
                later.push_back(message);
                continue;
            }
            match (message, &self.engine) {
                (ToMain::Event(function, args, url), Some(engine)) => {
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    if let Err(error) = engine.call_logged(function, &args, url) {
                        eprintln!("apps: {error}");
                    }
                }
                (ToMain::Bridge(message), Some(engine)) => {
                    if let Err(error) = engine.deliver(message) {
                        eprintln!("apps: {error:#}");
                    }
                }
                (ToMain::Bridge(message), None) => self.invoke(message.value),
                (ToMain::Sync(message, answer), Some(engine)) => {
                    let _ = answer.send(engine.receive(message));
                }
                (ToMain::Open(id, args, answer), Some(engine)) => {
                    let result = engine
                        .call("__appsWindowOpen", &[&id.to_string(), &args.to_string()])
                        .and_then(|text| Ok(serde_json::from_str::<Value>(&text)?.into()));
                    let _ = answer.send(result);
                }
                (ToMain::Spawn(id), _) => {
                    self.windows.borrow_mut().entry(id).or_default();
                    if let (Some(spawner), Some(queue)) =
                        (&*self.spawner.borrow(), self.link.queue_of(id))
                    {
                        spawner(id, queue);
                    }
                }
                (ToMain::Window(id, window, control), _) => {
                    // A window that closed on its way here must not stay retained.
                    if let Some(entry) = self.windows.borrow_mut().get_mut(&id) {
                        entry.native = Some((window, control));
                        let mut changed = self.changed.borrow_mut();
                        if !changed.contains(&id) {
                            changed.push(id);
                        }
                    }
                    let waiting = std::mem::take(&mut *self.waiting.borrow_mut());
                    for command in waiting {
                        self.native(command);
                    }
                }
                (ToMain::Native(command), _) => self.native(command),
                (ToMain::OpenTools, Some(engine)) => {
                    let tools = super::panel::DevTools::new("Main", crate::jsc::Role::Main, None);
                    // The agent keeps the logged values from now on, so their rows can expand.
                    match tools.and_then(|tools| {
                        super::panel::agent(engine, "poll", &Value::Null)?;
                        Ok(tools)
                    }) {
                        Ok(tools) => *self.tools.borrow_mut() = Some(tools),
                        Err(error) => eprintln!("apps: developer tools: {error:#}"),
                    }
                }
                (ToMain::OpenTools, None) => {}
                (ToMain::Event(..) | ToMain::Sync(..) | ToMain::Open(..), None) => {}
            }
        }
        if !later.is_empty() {
            self.link.defer_main(later);
        }
        // A nested loop can run this inside a host call of the main script, so attaches wait for that call.
        if let Ok(mut host) = self.host.try_borrow_mut() {
            host.browsers.attach_pending(|parent| {
                let windows = self.windows.borrow();
                match windows
                    .get(&parent)
                    .and_then(|window| window.native.as_ref())
                {
                    Some((_, control)) => match control.raw_window_handle() {
                        Ok(window) => {
                            #[cfg(target_os = "linux")]
                            let scale = control.scale_factor() as f64;
                            #[cfg(not(target_os = "linux"))]
                            let scale = 1.;
                            Parent::Ready(NativeHandle(window), scale)
                        }
                        Err(_) => Parent::Waiting,
                    },
                    None if windows.contains_key(&parent) || self.link.spawning(parent) => {
                        Parent::Waiting
                    }
                    // The parent closed, so the attach would wait forever.
                    None => Parent::Gone,
                }
            });
        }
        if !scripts {
            return busy;
        }
        let changed = std::mem::take(&mut *self.changed.borrow_mut());
        if let Some(engine) = &self.engine {
            for id in changed {
                let state = self
                    .windows
                    .borrow()
                    .get(&id)
                    .and_then(|window| window.native.as_ref())
                    .map(|(_, control)| (control.state(), control.is_active()));
                match state {
                    Some((Some(state), focused)) => {
                        let (id, state) = (id.to_string(), state_json(&state, focused).to_string());
                        if let Err(error) = engine.call_logged(
                            "__appsEvent",
                            &["\"state\"", &id, &state],
                            "apps:state",
                        ) {
                            eprintln!("apps: {error}");
                        }
                    }
                    Some((None, _)) => self.changed.borrow_mut().push(id),
                    None => {}
                }
            }
        }
        busy
    }

    pub(crate) fn page(&self, window: WindowId) -> Option<u64> {
        self.windows
            .borrow()
            .iter()
            .find_map(|(&id, entry)| (entry.native.as_ref()?.0 == window).then_some(id))
    }

    // Do not reenter a main script from a nested loop or host call. Let the key pass.
    pub(crate) fn before_input(
        &self,
        id: u64,
        input: &renderer::Event,
        composing: &dyn Fn() -> bool,
    ) -> bool {
        let (kind, key, modifiers, held, ime) = match input {
            renderer::Event::KeyDown {
                key,
                modifiers,
                repeat,
                ime,
            } => ("keyDown", key, *modifiers, *repeat, *ime),
            renderer::Event::KeyUp {
                key,
                modifiers,
                ime,
            } => ("keyUp", key, *modifiers, false, *ime),
            _ => return false,
        };
        let Some(engine) = &self.engine else {
            return false;
        };
        let Ok(packaged) = self.host.try_borrow().map(|host| host.app.packaged) else {
            return false;
        };
        if !self
            .windows
            .borrow()
            .get(&id)
            .is_some_and(|window| window.input)
            || self.driving.get()
            || !packaged && tool(&key.name, modifiers).is_some()
        {
            return false;
        }
        let input = json!({
            "type": kind,
            "key": if ime {
                "Process"
            } else {
                key.text.as_deref().unwrap_or(&key.name)
            },
            "code": key.name,
            "isAutoRepeat": held,
            "isComposing": composing(),
            "shift": modifiers.shift(),
            "control": modifiers.ctrl(),
            "alt": modifiers.alt(),
            "meta": modifiers.command(),
            "location": 0,
            "modifiers": [],
        });
        self.driving.set(true);
        let result = engine.call("__appsBeforeInput", &[&id.to_string(), &input.to_string()]);
        self.driving.set(false);
        // Listeners can add timers and IPC, which only a turn of main runs.
        self.link.wake_main();
        result
            .inspect_err(|error| eprintln!("apps: {error}"))
            .is_ok_and(|prevented| prevented == "true")
    }

    // Like `before_input`, this never reenters a main script.
    pub(crate) fn will_change(&self, id: u64, kind: &str, bounds: Rect) -> bool {
        let Some(engine) = &self.engine else {
            return false;
        };
        if !self
            .windows
            .borrow()
            .get(&id)
            .is_some_and(|window| window.frames)
            || self.driving.get()
            || self.host.try_borrow().is_err()
        {
            return false;
        }
        let [x, y, width, height] = [
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        ]
        .map(|value| f32::from(value).round());
        self.driving.set(true);
        let result = engine.call(
            "__appsWillChange",
            &[
                &id.to_string(),
                &json!([kind, x, y, width, height]).to_string(),
            ],
        );
        self.driving.set(false);
        self.link.wake_main();
        result
            .inspect_err(|error| eprintln!("apps: {error}"))
            .is_ok_and(|prevented| prevented == "true")
    }

    pub(crate) fn window_event(&self, window: WindowId, event: WindowEvent) {
        let Some(id) = self.page(window) else {
            return;
        };
        match event {
            WindowEvent::Changed => {
                let mut changed = self.changed.borrow_mut();
                if !changed.contains(&id) {
                    changed.push(id);
                }
                drop(changed);
                self.link.wake_main();
            }
            WindowEvent::CloseRequested if self.engine.is_some() => {
                self.link.event("close", id, None, "apps:close")
            }
            WindowEvent::CloseRequested => self.native(json!({"op":"window","args":["close",id]})),
        }
    }

    fn native(&self, command: Value) {
        let args = &command["args"];
        if command["op"] == "focus" {
            renderer::activate(args[0] == true);
            return;
        }
        let action = args[0].as_str().unwrap_or_default();
        let Some(id) = args[1].as_u64() else {
            return;
        };
        if action == "input" || action == "willChange" {
            if let Some(window) = self.windows.borrow_mut().get_mut(&id) {
                *match action {
                    "input" => &mut window.input,
                    _ => &mut window.frames,
                } = args[2] == true;
            }
            return;
        }
        if action == "close" {
            // A nested loop can run this inside a host call of the main script, so it waits for that call.
            if self.host.try_borrow_mut().is_err() {
                return self.link.main(ToMain::Native(command));
            }
            // On Windows a hide calls `window_event` at once, so the `windows` borrow must end first.
            let removed = self
                .windows
                .borrow_mut()
                .get_mut(&id)
                .and_then(|window| window.native.take());
            if let Some((_, control)) = removed {
                control.change(WindowChange::Hide);
            }
            self.waiting
                .borrow_mut()
                .retain(|waiting| waiting["args"][1].as_u64() != Some(id));
            crate::host::context::stop_page(id);
            if let Ok(mut host) = self.host.try_borrow_mut()
                && let Err(error) = host.browsers.close_window(id)
            {
                eprintln!("apps: {error:#}");
            }
            self.link.page(ToPage::Window(id, command));
            self.link.forget_window(id);
            self.link.event("closed", id, None, "apps:closed");
            let mut windows = self.windows.borrow_mut();
            windows.remove(&id);
            if self.engine.is_none() && windows.is_empty() {
                self.link.quit(0);
            }
            return;
        }
        // AppKit can run a nested loop in these calls, which runs main tasks, so no borrow lasts across them.
        let (window, parent) = {
            let windows = self.windows.borrow();
            let Some((_, window)) = windows.get(&id).and_then(|window| window.native.as_ref())
            else {
                drop(windows);
                self.waiting.borrow_mut().push(command);
                return;
            };
            let parent = args[2]
                .as_u64()
                .filter(|_| action == "parent")
                .map(|parent| {
                    let parent = windows.get(&parent);
                    parent.map(|window| window.native.as_ref().map(|(_, control)| control.clone()))
                });
            (window.clone(), parent)
        };
        let number = |index: usize| px(args[index].as_f64().unwrap_or(0.) as f32);
        let on = args[2] == true;
        let state = || window.state().unwrap_or_default();
        let change = match action {
            "title" => return window.set_title(args[2].as_str().unwrap_or("")),
            "bounds" if cfg!(target_os = "macos") => WindowChange::Frame(Rect::new(
                point(number(2), number(3)),
                size(number(4), number(5)),
            )),
            "bounds" => {
                let state = state();
                return window.resize(size(
                    number(4) - (state.frame.size.width - state.content.size.width),
                    number(5) - (state.frame.size.height - state.content.size.height),
                ));
            }
            "buttons" => return window.set_button_visibility(on),
            "buttonPosition" => {
                return window
                    .set_button_position(args[2].as_f64().map(|_| point(number(2), number(3))));
            }
            "minimize" => return window.minimize(),
            "maximize" | "unmaximize" => {
                if state().maximized != (action == "maximize") {
                    window.zoom();
                }
                return;
            }
            "fullscreen" => {
                if state().fullscreen != on {
                    window.toggle_fullscreen();
                }
                return;
            }
            "restore" if cfg!(target_os = "macos") => WindowChange::Restore,
            "show" | "focus" | "restore" => return window.activate(),
            "opacity" => WindowChange::Opacity(number(2).0),
            "shadow" => WindowChange::Shadow(on),
            "ignoreMouse" => WindowChange::IgnoreMouse(on, args[3] == true),
            "workspaces" => WindowChange::AllWorkspaces(on, args[3] == true),
            "contentProtection" => WindowChange::ContentProtection(on),
            "focusable" => WindowChange::Focusable(on),
            "edited" => WindowChange::Edited(on),
            "aspectRatio" => WindowChange::AspectRatio(number(2).0, size(number(3), number(4))),
            "parent" => {
                // Windows start on their own threads, so the parent can arrive after its child.
                if let Some(Some(None)) = &parent {
                    self.waiting.borrow_mut().push(command);
                    return;
                }
                window.change(WindowChange::Parent(
                    parent
                        .flatten()
                        .flatten()
                        .and_then(|control| control.raw_window_handle().ok()),
                    args[3] == true,
                ));
                if args[4] != true {
                    return;
                }
                WindowChange::Show(true)
            }
            "center" => WindowChange::Center,
            "blur" => WindowChange::Blur,
            "hide" => WindowChange::Hide,
            "showInactive" => WindowChange::Show(false),
            "limits" => {
                WindowChange::SizeLimits(size(number(2), number(3)), size(number(4), number(5)))
            }
            "resizable" => WindowChange::Resizable(on),
            "minimizable" => WindowChange::Minimizable(on),
            "maximizable" => WindowChange::Maximizable(on),
            "closable" => WindowChange::Closable(on),
            "movable" => WindowChange::Movable(on),
            "alwaysOnTop" => WindowChange::AlwaysOnTop(args[2].as_i64().unwrap_or(0) as isize),
            "composition" => WindowChange::CancelComposition,
            _ => return,
        };
        window.change(change);
    }

    fn invoke(&self, mut message: Value) {
        let Some(window) = message["sender"]["preload"].as_u64() else {
            return;
        };
        let id = message["id"].take();
        let channel = message["channel"].as_str().unwrap_or_default();
        let handler = self
            .handlers
            .borrow()
            .get(channel)
            .cloned()
            .with_context(|| format!("No Rust handler registered for {channel:?}"));
        let args = serde_json::from_str(message["args"].as_str().unwrap_or_default())
            .map_err(anyhow::Error::from);
        let link = self.link.clone();
        self.spawn(Box::new(move || {
            let result = handler.and_then(|handler| handler(window, args?));
            let mut reply = json!({"kind":"reply","sender":"main","id":id});
            match result {
                Ok(value) => reply["value"] = json!({"type":"value","value":value}),
                Err(error) => {
                    reply["error"] = json!({"type":"error","id":0,"name":"Error","value":format!("{error:#}"),"stack":""})
                }
            }
            link.page(ToPage::Bridge(window, reply.into()));
        }));
    }

    fn spawn(&self, job: Job) {
        let mut pool = self.pool.borrow_mut();
        let jobs = pool.get_or_insert_with(|| {
            let (jobs, receiver) = mpsc::channel::<Job>();
            let receiver = Arc::new(Mutex::new(receiver));
            for _ in 0..std::thread::available_parallelism().map_or(1, |n| n.get().min(4)) {
                let receiver = receiver.clone();
                let _ = std::thread::Builder::new()
                    .name("apps-handler".into())
                    .spawn(move || {
                        while let Ok(Ok(job)) = receiver.lock().map(|receiver| receiver.recv()) {
                            job();
                            // Threads wait in turn for the receiver, so return cached blocks now.
                            #[cfg(target_os = "macos")]
                            crate::malloc::flush();
                        }
                    });
            }
            jobs
        });
        let _ = jobs.send(job);
    }

    // Mark scripts outside a main turn as a turn to prevent reentry through nested loops.
    pub(crate) fn eval(&self, source: &str, url: &str) -> Result<String> {
        let engine = self.engine.as_ref().context("Missing the main script")?;
        let driving = self.driving.replace(true);
        let result = engine.eval(source, url);
        self.driving.set(driving);
        result
    }

    pub(crate) fn drive(self: &Rc<Self>) {
        if self.driving.replace(true) {
            self.receive_with(false);
            return;
        }
        crate::jsc::idle(false);
        let result = self.turn();
        self.driving.set(false);
        match result {
            Ok(Some(delay)) => {
                let at = Instant::now() + Duration::from_millis(delay);
                // A timer can't be cancelled, so keep the armed one unless this one is earlier.
                if self.wake.get().is_none_or(|armed| {
                    armed <= Instant::now() || at + Duration::from_micros(500) < armed
                }) {
                    self.wake.set(Some(at));
                    let main = Rc::downgrade(self);
                    renderer::post_main_after(Duration::from_millis(delay), move || {
                        if let Some(main) = main.upgrade() {
                            main.drive();
                        }
                    });
                }
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("apps: {error:#}");
                self.link.fail();
            }
        }
        if self.link.quitting() && !self.ended.replace(true) {
            crate::host::context::stop_pages();
            renderer::quit();
        }
        if self.api.shrink_pending() && !self.quiet.replace(true) {
            self.shrink_when_quiet();
        }
        crate::jsc::idle(true);
    }

    fn turn(&self) -> Result<Option<u64>> {
        {
            let mut host = self.host.borrow_mut();
            host.sync_theme();
            if std::mem::take(&mut host.theme_changed) {
                self.link.page(ToPage::All(json!({"op":"theme"})));
            }
        }
        let start = Instant::now();
        while self.receive_with(true) && start.elapsed() < Duration::from_millis(4) {}
        let mut delay = i64::MAX;
        if let Some(engine) = &self.engine {
            let next = engine.tick()?;
            if next >= 0 {
                delay = next;
            }
            self.receive_with(true);
            let logs: Vec<_> = self.host.borrow_mut().logs.drain(..).collect();
            if let Some(tools) = &*self.tools.borrow()
                && let Err(error) = tools
                    .commands()
                    .iter()
                    .try_for_each(|command| tools.answer(command, engine, &self.host, None))
                    .and_then(|()| tools.events(&logs))
            {
                eprintln!("apps: developer tools: {error:#}");
                tools.closed.set(true);
            }
            if !logs.is_empty() {
                crate::native::devtools::logs(&logs);
                if super::panel::pages_open() {
                    self.link
                        .page(ToPage::All(json!({"op":"logs","args":logs})));
                }
            }
        }
        if crate::native::devtools::poll_main() {
            delay = delay.min(0);
        }
        if let Some(engine) = &self.engine {
            // IPC, microtasks and the scripts above can add timers after the tick, so read the delay again.
            if let Ok(next) = engine
                .call("__appsDelay", &[])
                .map(|next| next.parse::<f64>().unwrap_or(-1.))
                && next >= 0.
            {
                delay = delay.min(next.ceil() as i64);
            }
        }
        let closed = self.tools.borrow_mut().take_if(|tools| tools.is_closed());
        if let (Some(_), Some(engine)) = (closed, &self.engine)
            && let Err(error) = super::panel::release(engine)
        {
            eprintln!("apps: developer tools: {error:#}");
        }
        if self.link.main_pending() {
            delay = delay.min(0);
        }
        Ok((delay != i64::MAX).then(|| delay.clamp(0, 1 << 31) as u64))
    }

    fn shrink_when_quiet(self: &Rc<Self>) {
        let main = Rc::downgrade(self);
        renderer::post_main_after(Duration::from_secs(1), move || {
            if let Some(main) = main.upgrade() {
                if main.api.shrink_due() {
                    main.quiet.set(false);
                    main.api.shrink();
                    #[cfg(target_os = "macos")]
                    crate::malloc::flush();
                } else {
                    main.shrink_when_quiet();
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsc::Role;

    #[test]
    fn events_call_main_with_their_values_and_drain_microtasks_after_each() {
        let api = JavaScriptCore::load().unwrap();
        let host = Rc::new(RefCell::new(Host::default()));
        let engine = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
        engine
            .eval(
                "globalThis.runs=[];globalThis.__appsEvent=function(name,id,data){if(name==='bad')throw new Error('bad event');runs.push(name+id+data+arguments.length);queueMicrotask(()=>runs.push('micro'))}",
                "test",
            )
            .unwrap();
        let link = Link::new();
        let main = Main::new(Some(engine), host, link.clone(), api);
        link.event(
            "title",
            1,
            Some("it's \"q\" \u{2028} \u{1f600}".into()),
            "test",
        );
        link.event("bad", 2, None, "test");
        link.event("close", 3, None, "test");
        main.receive();
        let runs = main
            .engine
            .as_ref()
            .unwrap()
            .eval("runs.join('|')", "test")
            .unwrap();
        assert_eq!(
            runs,
            "title1it's \"q\" \u{2028} \u{1f600}3|micro|close3undefined2|micro"
        );
    }

    #[test]
    fn nested_loops_defer_main_scripts_and_closed_windows_lose_their_messages() {
        let api = JavaScriptCore::load().unwrap();
        let host = Rc::new(RefCell::new(Host::default()));
        let engine = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
        engine
            .eval(
                "globalThis.runs=[];globalThis.pushEvent=()=>runs.push('event');globalThis.__appsWindowOpen=id=>{runs.push('open'+id);return 'null'}",
                "test",
            )
            .unwrap();
        let link = Link::new();
        let main = Main::new(Some(engine), host, link.clone(), api);
        link.main(ToMain::Spawn(901));
        link.call("pushEvent", Vec::new(), "test");
        link.main(ToMain::Spawn(902));
        main.driving.set(true);
        main.receive();
        let runs = || {
            main.engine
                .as_ref()
                .unwrap()
                .eval("runs.join()", "test")
                .unwrap()
        };
        assert!(main.windows.borrow().contains_key(&902) && link.main_pending());
        assert_eq!(runs(), "");
        main.driving.set(false);
        main.receive();
        assert_eq!(runs(), "event");
        // Use a unique page ID: close stops it process-wide.
        main.native(json!({"op":"window","args":["close",901]}));
        crate::host::context::page_stopped(901);
        for id in [901, 902] {
            let (answer, reply) = mpsc::sync_channel(1);
            link.main(ToMain::Open(id, json!(["open", "", "", "", ""]), answer));
            main.receive();
            assert_eq!(reply.try_recv().is_ok(), id == 902);
        }
        assert_eq!(runs(), "event,open902");
    }

    #[test]
    fn main_filters_the_keys_and_frames_of_listening_windows_outside_its_scripts() {
        use renderer::{Event, Key, Modifiers};
        let api = JavaScriptCore::load().unwrap();
        let host = Rc::new(RefCell::new(Host::default()));
        let engine = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
        engine
            .eval(
                "globalThis.keys=[];globalThis.__appsBeforeInput=(id,input)=>{keys.push(id+JSON.stringify(input));return input.code==='p'};globalThis.__appsWillChange=(id,change)=>{keys.push(id+JSON.stringify(change));return change[0]==='right'}",
                "test",
            )
            .unwrap();
        let main = Main::new(Some(engine), host.clone(), Link::new(), api);
        main.windows.borrow_mut().insert(5, Window::default());
        let key = |key: &str, modifiers: Modifiers| {
            let text = (key.len() == 1).then(|| key.to_uppercase());
            (
                Key {
                    name: key.into(),
                    text,
                },
                modifiers,
            )
        };
        let down = |(key, modifiers): (Key, Modifiers)| Event::KeyDown {
            key,
            modifiers,
            repeat: true,
            ime: false,
        };
        let shift = Modifiers::SHIFT;
        let keys = || {
            main.engine
                .as_ref()
                .unwrap()
                .eval("keys.splice(0).join('|')", "test")
                .unwrap()
        };
        let p = down(key("p", shift));
        assert!(!main.before_input(5, &p, &|| true));
        main.native(json!({"op":"window","args":["input",5,true]}));
        assert!(main.before_input(5, &p, &|| true));
        assert!(!main.before_input(
            5,
            &Event::KeyUp {
                key: key("q", Modifiers::NONE).0,
                modifiers: Modifiers::NONE,
                ime: false,
            },
            &|| false
        ));
        assert_eq!(
            keys(),
            r#"5{"type":"keyDown","key":"P","code":"p","isAutoRepeat":true,"isComposing":true,"shift":true,"control":false,"alt":false,"meta":false,"location":0,"modifiers":[]}|5{"type":"keyUp","key":"Q","code":"q","isAutoRepeat":false,"isComposing":false,"shift":false,"control":false,"alt":false,"meta":false,"location":0,"modifiers":[]}"#
        );
        let process = |ime| Event::KeyDown {
            key: key("p", Modifiers::NONE).0,
            modifiers: Modifiers::NONE,
            repeat: false,
            ime,
        };
        assert!(main.before_input(5, &process(true), &|| true));
        assert!(main.before_input(5, &process(false), &|| false));
        assert_eq!(
            keys(),
            r#"5{"type":"keyDown","key":"Process","code":"p","isAutoRepeat":false,"isComposing":true,"shift":false,"control":false,"alt":false,"meta":false,"location":0,"modifiers":[]}|5{"type":"keyDown","key":"P","code":"p","isAutoRepeat":false,"isComposing":false,"shift":false,"control":false,"alt":false,"meta":false,"location":0,"modifiers":[]}"#
        );
        let tools = Modifiers::COMMAND.plus(Modifiers::ALT);
        let primary = match cfg!(target_os = "macos") {
            true => Modifiers::COMMAND,
            false => Modifiers::CTRL,
        };
        for tool in [
            key("f12", Modifiers::NONE),
            key("i", tools),
            key("f", primary.plus(Modifiers::ALT)),
        ] {
            assert!(!main.before_input(5, &down(tool), &|| false));
        }
        assert_eq!(keys(), "");
        host.borrow_mut().update_app(|app| app.packaged = true);
        main.before_input(5, &down(key("f12", Modifiers::NONE)), &|| false);
        assert!(keys().contains(r#""code":"f12""#));
        main.driving.set(true);
        assert!(!main.before_input(5, &p, &|| false));
        main.driving.set(false);
        let borrowed = host.borrow_mut();
        assert!(!main.before_input(5, &p, &|| false));
        drop(borrowed);
        assert_eq!(keys(), "");
        main.native(json!({"op":"window","args":["input",5,false]}));
        assert!(!main.before_input(5, &p, &|| false));
        assert_eq!(keys(), "");
        let bounds = Rect::new(point(px(1.4), px(2.)), size(px(30.), px(40.)));
        assert!(!main.will_change(5, "right", bounds));
        main.native(json!({"op":"window","args":["willChange",5,true]}));
        assert!(main.will_change(5, "right", bounds) && !main.will_change(5, "move", bounds));
        main.driving.set(true);
        assert!(!main.will_change(5, "right", bounds));
        main.driving.set(false);
        assert_eq!(keys(), r#"5["right",1,2,30,40]|5["move",1,2,30,40]"#);
    }
}
