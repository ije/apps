//! The path between the main side of an app (its main script or Rust handlers) and the page side
//! (windows, pages and their preloads). The sides share no state and can run on different threads,
//! so all they say goes through here.

use crate::{host::Host, jsc::JavaScriptCore, jsc::JsContext, jsc::payload::Payload};
use anyhow::{Context, Result};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use renderer::{Bounds, WindowChange, WindowControl, WindowId, native::WindowEvent, point, px, size};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicI32, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

type Answer = mpsc::SyncSender<Result<Payload>>;
type Job = Box<dyn FnOnce() + Send>;
type Wake = Box<dyn Fn() + Send + Sync>;
type Spawner = Box<dyn Fn(u64, Arc<PageQueue>)>;

/// The view of a native window, for the web views that attach to it.
#[derive(Clone, Copy)]
struct NativeHandle(RawWindowHandle);

impl HasWindowHandle for NativeHandle {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the page side reports a window's close before the view goes away.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

pub(crate) enum ToMain {
    /// A script that delivers an app or window event.
    Event(String, &'static str),
    /// A bridge message from a preload.
    Bridge(Payload),
    /// `ipcRenderer.sendSync()`, while the preload waits.
    Sync(Payload, Answer),
    /// `window.open()`, or a read of the window it opened, while the page waits.
    Open(u64, Value, Answer),
    /// A new window needs its page side.
    Spawn(u64),
    /// A window opened.
    Window(u64, WindowId, WindowControl),
    /// A command for a native window, or for the app.
    Native(Value),
    /// The app has no page to inspect, so developer tools inspect the main script.
    OpenTools,
}

pub(crate) enum ToPage {
    /// A window or app command.
    Command(Value),
    /// A bridge message for the preload of a window.
    Bridge(u64, Payload),
}

/// What the main side sends to one page side.
#[derive(Default)]
pub(crate) struct PageQueue {
    queue: Mutex<VecDeque<ToPage>>,
    wake: OnceLock<Wake>,
}

impl PageQueue {
    fn push(&self, message: ToPage) {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(message);
        self.wake();
    }

    pub(crate) fn take(&self) -> VecDeque<ToPage> {
        std::mem::take(&mut self.queue.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub(crate) fn pending(&self) -> bool {
        !self
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    pub(crate) fn set_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        let _ = self.wake.set(Box::new(wake));
    }

    fn wake(&self) {
        if let Some(wake) = self.wake.get() {
            wake();
        }
    }

    pub(crate) fn wake_now(&self) {
        self.wake();
    }
}

struct Queues {
    main: Mutex<VecDeque<ToMain>>,
    /// The queue of the one page side that runs all windows, without threads per window.
    page: Arc<PageQueue>,
    /// By window, the queues of the page sides that run one window each.
    pages: Mutex<HashMap<u64, Arc<PageQueue>>>,
    per_window: AtomicBool,
    wake_main: OnceLock<Wake>,
    quit: AtomicBool,
    exit_code: AtomicI32,
}

#[derive(Clone)]
pub(crate) struct Link(Arc<Queues>);

impl Link {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Queues {
            main: Mutex::default(),
            page: Arc::default(),
            pages: Mutex::default(),
            per_window: AtomicBool::new(false),
            wake_main: OnceLock::new(),
            quit: AtomicBool::new(false),
            exit_code: AtomicI32::new(0),
        }))
    }

    pub(crate) fn set_wake_main(&self, wake: impl Fn() + Send + Sync + 'static) {
        let _ = self.0.wake_main.set(Box::new(wake));
    }

    pub(crate) fn wake_main(&self) {
        if let Some(wake) = self.0.wake_main.get() {
            wake();
        }
    }

    /// The queue of the page side that all windows share.
    pub(crate) fn shared_queue(&self) -> Arc<PageQueue> {
        self.0.page.clone()
    }

    fn queue_of(&self, window: Option<u64>) -> Option<Arc<PageQueue>> {
        if !self.0.per_window.load(Ordering::Acquire) {
            return Some(self.0.page.clone());
        }
        self.0
            .pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&window?)
            .cloned()
    }

    /// Gives each window its own page side from now on. What was sent so far moves to them.
    pub(crate) fn set_per_window(&self) {
        let waiting = self.0.page.take();
        self.0.per_window.store(true, Ordering::Release);
        for message in waiting {
            if let ToPage::Command(command) = &message
                && command["args"][0] == "create"
                && let Some(id) = command["args"][1].as_u64()
            {
                self.0
                    .pages
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(id, Arc::default());
            }
            self.page(message);
        }
    }

    /// Opens a window, from a `create` command.
    pub(crate) fn create_window(&self, command: Value) {
        let Some(id) = command["args"][1].as_u64() else {
            return;
        };
        if self.0.per_window.load(Ordering::Acquire) {
            self.0
                .pages
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(id, Arc::default());
        }
        let options = &command["args"][2];
        if let Some(parent) = options["parent"].as_u64() {
            // WARNING: only the main side knows the native window of another window, so it joins
            // the child to its parent and then shows it.
            self.main(ToMain::Native(json!({"op":"window","args":[
                "parent", id, parent, options["modal"] == true, options["show"] != false,
            ]})));
        }
        self.page(ToPage::Command(command));
        self.main(ToMain::Spawn(id));
    }

    pub(crate) fn wake_page(&self, window: Option<u64>) {
        if let Some(queue) = self.queue_of(window) {
            queue.wake();
        }
    }

    pub(crate) fn wake_pages(&self) {
        self.0.page.wake();
        for queue in self
            .0
            .pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            queue.wake();
        }
    }

    pub(crate) fn main(&self, message: ToMain) {
        self.0
            .main
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(message);
        self.wake_main();
    }

    pub(crate) fn event(&self, source: String, url: &'static str) {
        self.main(ToMain::Event(source, url));
    }

    /// Sends to the page side of the window that the message names.
    pub(crate) fn page(&self, message: ToPage) {
        let window = match &message {
            ToPage::Bridge(id, _) => Some(*id),
            ToPage::Command(command) => command["args"][1].as_u64(),
        };
        if let Some(queue) = self.queue_of(window) {
            queue.push(message);
        }
    }

    /// Sends a command to every page side.
    pub(crate) fn page_all(&self, command: Value) {
        if !self.0.per_window.load(Ordering::Acquire) {
            return self.0.page.push(ToPage::Command(command));
        }
        for queue in self
            .0
            .pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            queue.push(ToPage::Command(command.clone()));
        }
    }

    /// The window is closing: its queue stays with its page side, which ends after that.
    fn forget_page(&self, window: u64) {
        self.0
            .pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&window);
    }

    pub(crate) fn main_pending(&self) -> bool {
        !self
            .0
            .main
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    #[cfg(test)]
    pub(crate) fn native_commands(&self) -> Vec<Value> {
        self.0
            .main
            .lock()
            .unwrap()
            .iter()
            .filter_map(|message| match message {
                ToMain::Native(command) => Some(command["args"].clone()),
                _ => None,
            })
            .collect()
    }

    /// Asks the app to quit, from any thread. The main side ends it on its next turn.
    pub(crate) fn quit(&self, code: i32) {
        self.0.exit_code.store(code, Ordering::Release);
        self.0.quit.store(true, Ordering::Release);
        self.wake_main();
    }

    /// Asks the app to quit with a failure code, unless it already has one.
    pub(crate) fn fail(&self) {
        let _ = self
            .0
            .exit_code
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
        self.0.quit.store(true, Ordering::Release);
        self.wake_main();
    }

    pub(crate) fn quitting(&self) -> bool {
        self.0.quit.load(Ordering::Acquire)
    }

    pub(crate) fn exit_code(&self) -> i32 {
        self.0.exit_code.load(Ordering::Acquire)
    }

    /// Asks the main side and waits for its answer. Main must never wait for a page like this.
    pub(crate) fn ask(&self, message: impl FnOnce(Answer) -> ToMain) -> Result<Payload> {
        let (answer, reply) = mpsc::sync_channel(1);
        self.main(message(answer));
        // WARNING: without a render thread both sides share this thread, so the main side runs here.
        if let Some(main) = MAIN.with_borrow(Weak::upgrade) {
            main.receive();
        }
        reply.recv().context("The main process is unavailable")?
    }
}

thread_local! { static MAIN: RefCell<Weak<Main>> = RefCell::default(); }

/// Runs a turn of the main side of this thread.
pub(crate) fn drive_main() {
    if let Some(main) = MAIN.with_borrow(Weak::upgrade) {
        main.drive();
    }
}

/// Whether a command needs only the native window, so a busy page holds it up for nothing.
pub(crate) fn native_command(command: &Value) -> bool {
    command["op"] == "focus"
        || command["op"] == "window"
            && matches!(
                command["args"][0].as_str(),
                Some(
                    "close"
                        | "show"
                        | "title"
                        | "bounds"
                        | "center"
                        | "minimize"
                        | "maximize"
                        | "unmaximize"
                        | "restore"
                        | "fullscreen"
                        | "focus"
                        | "blur"
                        | "hide"
                        | "showInactive"
                        | "limits"
                        | "resizable"
                        | "minimizable"
                        | "maximizable"
                        | "closable"
                        | "movable"
                        | "alwaysOnTop"
                        | "buttons"
                        | "opacity"
                        | "shadow"
                        | "ignoreMouse"
                        | "workspaces"
                        | "contentProtection"
                        | "focusable"
                        | "aspectRatio"
                        | "parent"
                )
            )
}

/// A window state as `__appsEvent('state')` takes it.
pub(crate) fn state_json(state: &renderer::WindowState, focused: bool) -> Value {
    let rect = |bounds: Bounds<renderer::Pixels>| {
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

/// The main side: the main script, or the Rust handlers.
pub(crate) struct Main {
    pub(crate) engine: Option<JsContext>,
    pub(crate) host: Rc<RefCell<Host>>,
    pub(crate) handlers: RefCell<HashMap<String, super::app::Handler>>,
    api: Rc<JavaScriptCore>,
    link: Link,
    pool: RefCell<Option<mpsc::Sender<Job>>>,
    windows: RefCell<HashMap<u64, (WindowId, WindowControl)>>,
    /// The windows that exist or are on their way.
    opened: RefCell<std::collections::BTreeSet<u64>>,
    /// Starts the page side of a window on its own thread.
    spawner: RefCell<Option<Spawner>>,
    /// The commands of windows that are still opening.
    waiting: RefCell<Vec<Value>>,
    /// The windows whose state changed since the main script last heard.
    changed: RefCell<Vec<u64>>,
    tools: RefCell<Option<super::devtools::DevTools>>,
    /// The time of the armed timer of `drive`, and whether `drive` is running.
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
            opened: RefCell::default(),
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
        main
    }

    pub(crate) fn link(&self) -> &Link {
        &self.link
    }

    pub(crate) fn set_spawner(&self, spawner: Spawner) {
        *self.spawner.borrow_mut() = Some(spawner);
    }

    /// The window that opened first, of those that are open.
    pub(crate) fn first_window(&self) -> Option<u64> {
        self.opened.borrow().first().copied()
    }

    /// Runs what the page side sent, and returns whether there was anything.
    pub(crate) fn receive(&self) -> bool {
        let messages =
            std::mem::take(&mut *self.link.0.main.lock().unwrap_or_else(PoisonError::into_inner));
        let busy = !messages.is_empty();
        for message in messages {
            match (message, &self.engine) {
                (ToMain::Event(source, url), Some(engine)) => {
                    if let Err(error) = engine.eval(&source, url) {
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
                    self.opened.borrow_mut().insert(id);
                    if let (Some(spawner), Some(queue)) =
                        (&*self.spawner.borrow(), self.link.queue_of(Some(id)))
                    {
                        spawner(id, queue);
                    }
                }
                (ToMain::Window(id, window, control), _) => {
                    // A window that closed on its way here must not stay retained.
                    if self.opened.borrow().contains(&id) {
                        self.windows.borrow_mut().insert(id, (window, control));
                    }
                    let waiting = std::mem::take(&mut *self.waiting.borrow_mut());
                    for command in waiting {
                        self.native(command);
                    }
                }
                (ToMain::Native(command), _) => self.native(command),
                (ToMain::OpenTools, Some(_)) => match super::devtools::DevTools::new("Main") {
                    Ok(tools) => *self.tools.borrow_mut() = Some(tools),
                    Err(error) => {
                        eprintln!("apps: {error:#}");
                        self.link.fail();
                    }
                },
                (ToMain::OpenTools, None) => {}
                // A dropped answer tells the page that there is no main script.
                (ToMain::Event(..) | ToMain::Sync(..) | ToMain::Open(..), None) => {}
            }
        }
        // Web views attach to native windows, which can still be on their way.
        let mut host = self.host.borrow_mut();
        for (browser, parent) in host.browsers.take_attaches() {
            match self
                .windows
                .borrow()
                .get(&parent)
                .and_then(|(_, control)| control.raw_window_handle().ok())
            {
                Some(window) => {
                    if let Err(error) = host.browsers.attach(browser, &NativeHandle(window)) {
                        eprintln!("apps: {error:#}");
                    }
                }
                None => host.browsers.defer_attach(browser, parent),
            }
        }
        drop(host);
        let changed = std::mem::take(&mut *self.changed.borrow_mut());
        if let Some(engine) = &self.engine {
            for id in changed {
                let state = self
                    .windows
                    .borrow()
                    .get(&id)
                    .map(|(_, control)| (control.state(), control.is_active()));
                match state {
                    Some((Some(state), focused)) => {
                        let event =
                            format!("__appsEvent('state',{id},{})", state_json(&state, focused));
                        if let Err(error) = engine.eval(&event, "apps:state") {
                            eprintln!("apps: {error}");
                        }
                    }
                    // The window is busy, so its state comes on the next turn.
                    Some((None, _)) => self.changed.borrow_mut().push(id),
                    None => {}
                }
            }
        }
        busy
    }

    /// A native window told something, on the main thread.
    pub(crate) fn window_event(&self, window: WindowId, event: WindowEvent) {
        let id = self
            .windows
            .borrow()
            .iter()
            .find_map(|(&id, (other, _))| (*other == window).then_some(id));
        let Some(id) = id else {
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
            // A main script can keep the window open, and Rust apps always close it.
            WindowEvent::CloseRequested if self.engine.is_some() => self
                .link
                .event(format!("__appsEvent('close',{id})"), "apps:close"),
            WindowEvent::CloseRequested => self.native(json!({"op":"window","args":["close",id]})),
        }
    }

    /// Runs a command of `native_command` without the page side.
    fn native(&self, command: Value) {
        let args = &command["args"];
        if command["op"] == "focus" {
            renderer::native::activate(args[0] == true);
            return;
        }
        let action = args[0].as_str().unwrap_or_default();
        let Some(id) = args[1].as_u64() else {
            return;
        };
        if action == "close" {
            // WARNING: a page that runs a long script takes its time to go, so the window hides now
            // and the script stops at its next tick.
            // WARNING: on Windows a hide calls `window_event` at once, so the borrow must end first.
            let removed = self.windows.borrow_mut().remove(&id);
            if let Some((_, control)) = removed {
                control.change(WindowChange::Hide);
            }
            self.waiting
                .borrow_mut()
                .retain(|waiting| waiting["args"][1].as_u64() != Some(id));
            crate::jsc::stop_page(id);
            if let Err(error) = self.host.borrow_mut().browsers.close_window(id) {
                eprintln!("apps: {error:#}");
            }
            self.link.page(ToPage::Command(command));
            self.link.forget_page(id);
            self.link
                .event(format!("__appsEvent('closed',{id})"), "apps:closed");
            let mut opened = self.opened.borrow_mut();
            opened.remove(&id);
            if self.engine.is_none() && opened.is_empty() {
                self.link.quit(0);
            }
            return;
        }
        let windows = self.windows.borrow();
        let Some((_, window)) = windows.get(&id) else {
            drop(windows);
            self.waiting.borrow_mut().push(command);
            return;
        };
        let number = |index: usize| px(args[index].as_f64().unwrap_or(0.) as f32);
        let on = args[2] == true;
        let state = || window.state().unwrap_or_default();
        let change = match action {
            "title" => return window.set_title(args[2].as_str().unwrap_or("")),
            "bounds" if cfg!(target_os = "macos") => WindowChange::Frame(Bounds::new(
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
            "aspectRatio" => {
                WindowChange::AspectRatio(number(2).0, size(number(3), number(4)))
            }
            "parent" => {
                // WARNING: windows start on their own threads, so the parent can arrive after its child.
                if args[2].as_u64().is_some_and(|parent| {
                    !windows.contains_key(&parent) && self.opened.borrow().contains(&parent)
                }) {
                    self.waiting.borrow_mut().push(command);
                    return;
                }
                window.change(WindowChange::Parent(
                    args[2]
                        .as_u64()
                        .and_then(|parent| windows.get(&parent)?.1.raw_window_handle().ok()),
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
            "limits" => WindowChange::SizeLimits(
                size(number(2), number(3)),
                size(number(4), number(5)),
            ),
            "resizable" => WindowChange::Resizable(on),
            "minimizable" => WindowChange::Minimizable(on),
            "maximizable" => WindowChange::Maximizable(on),
            "closable" => WindowChange::Closable(on),
            "movable" => WindowChange::Movable(on),
            "alwaysOnTop" => WindowChange::AlwaysOnTop(on),
            _ => return,
        };
        window.change(change);
    }

    /// Runs a Rust handler on the pool, and sends its result to the preload that asked.
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
                        }
                    });
            }
            jobs
        });
        let _ = jobs.send(job);
    }

    /// One turn of the main side's loop, on the main thread.
    pub(crate) fn drive(self: &Rc<Self>) {
        // WARNING: AppKit runs nested loops under main scripts, like for a dialog. Those turns
        // only take messages, so a page that waits for an answer gets it.
        if self.driving.replace(true) {
            self.receive();
            return;
        }
        crate::host::idle(false);
        let result = self.turn();
        self.driving.set(false);
        match result {
            Ok(Some(delay)) => {
                let at = Instant::now() + Duration::from_millis(delay);
                // WARNING: a timer can't be cancelled, so keep the armed one unless this one is earlier.
                if self.wake.get().is_none_or(|armed| {
                    armed <= Instant::now() || at + Duration::from_micros(500) < armed
                }) {
                    self.wake.set(Some(at));
                    let main = Rc::downgrade(self);
                    renderer::native::after(Duration::from_millis(delay), move || {
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
            crate::jsc::stop_pages();
            renderer::native::quit();
        }
        // WARNING: JSC's GC timers never fire on the generic event loop, so after real JS work the
        // VM is collected once, when JS has been quiet for a second.
        if self.api.shrink_pending() && !self.quiet.replace(true) {
            self.shrink_when_quiet();
        }
        crate::host::idle(true);
    }

    fn turn(&self) -> Result<Option<u64>> {
        {
            let mut host = self.host.borrow_mut();
            host.sync_theme();
            if std::mem::take(&mut host.theme_changed) {
                self.link.page_all(json!({"op":"theme"}));
            }
        }
        let start = Instant::now();
        while self.receive() && start.elapsed() < Duration::from_millis(4) {}
        let mut delay = i64::MAX;
        if let Some(engine) = &self.engine {
            let next = engine.tick()?;
            if next >= 0 {
                delay = next;
            }
            // The tick can attach web views.
            self.receive();
            let logs: Vec<_> = self.host.borrow_mut().logs.drain(..).collect();
            if let Some(tools) = &*self.tools.borrow() {
                for command in tools.commands() {
                    let request = command["id"].as_u64().unwrap_or(0);
                    match super::devtools::dispatch(&command, engine, &self.host, 0) {
                        Ok(value) => tools.respond(request, value, None)?,
                        Err(error) => {
                            tools.respond(request, Value::Null, Some(&error.to_string()))?
                        }
                    }
                }
                tools.events(&logs)?;
                delay = delay.min(16);
            }
            if !logs.is_empty() {
                super::mcp::logs(&logs);
                self.link.page_all(json!({"op":"logs","args":logs}));
            }
        }
        // WARNING: a call without a window comes here, also in an app without a main script.
        if super::mcp::poll_main() {
            delay = delay.min(0);
        }
        if let Some(engine) = &self.engine {
            // WARNING: IPC, microtasks and the scripts above can add timers after the tick, so read the delay again.
            if let Ok(next) = engine
                .call("__appsDelay", &[])
                .map(|next| next.parse::<f64>().unwrap_or(-1.))
                && next >= 0.
            {
                delay = delay.min(next.ceil() as i64);
            }
        }
        self.tools.borrow_mut().take_if(|tools| tools.is_closed());
        if self.link.main_pending() {
            delay = delay.min(0);
        }
        Ok((delay != i64::MAX).then(|| delay.clamp(1, 1 << 31) as u64))
    }

    fn shrink_when_quiet(self: &Rc<Self>) {
        let busy = self.api.busy.get();
        let main = Rc::downgrade(self);
        renderer::native::after(Duration::from_secs(1), move || {
            if let Some(main) = main.upgrade() {
                if main.api.busy.get() == busy {
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
