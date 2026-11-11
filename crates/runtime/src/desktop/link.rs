use crate::{host::Host, jsc::JavaScriptCore, jsc::JsContext, jsc::payload::Payload};
use anyhow::{Context, Result};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use renderer::{Rect, WindowChange, WindowControl, WindowId, native::WindowEvent, point, px, size};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

type Answer = mpsc::SyncSender<Result<Payload>>;
type Job = Box<dyn FnOnce() + Send>;
type Wake = Box<dyn Fn() + Send + Sync>;
type Spawner = Box<dyn Fn(u64, Arc<PageQueue>)>;

#[derive(Clone, Copy)]
struct NativeHandle(RawWindowHandle);

impl HasWindowHandle for NativeHandle {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the page side reports a window's close before the view goes away.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

pub(crate) enum ToMain {
    Event(String, &'static str),
    Bridge(Payload),
    Sync(Payload, Answer),
    Open(u64, Value, Answer),
    Spawn(u64),
    Window(u64, WindowId, WindowControl),
    Native(Value),
    OpenTools,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Target {
    Window(u64),
    All,
    Main,
}

pub(crate) enum ToPage {
    Window(u64, Value),
    All(Value),
    Bridge(u64, Payload),
}

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

    pub(crate) fn wake(&self) {
        if let Some(wake) = self.wake.get() {
            wake();
        }
    }
}

struct Queues {
    main: Mutex<VecDeque<ToMain>>,
    page: Arc<PageQueue>,
    // Hold this lock on pushes so set_per_window cannot leave messages in the old shared queue. None uses
    // `page`.
    pages: Mutex<Option<HashMap<u64, Arc<PageQueue>>>>,
    created: AtomicU64,
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
            created: AtomicU64::new(0),
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

    pub(crate) fn shared_queue(&self) -> Arc<PageQueue> {
        self.0.page.clone()
    }

    fn pages(&self) -> std::sync::MutexGuard<'_, Option<HashMap<u64, Arc<PageQueue>>>> {
        self.0.pages.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn queue_of(&self, window: u64) -> Option<Arc<PageQueue>> {
        match &*self.pages() {
            None => Some(self.0.page.clone()),
            Some(pages) => pages.get(&window).cloned(),
        }
    }

    pub(crate) fn set_per_window(&self) {
        let mut pages = self.pages();
        let created = self.0.created.load(Ordering::Acquire);
        let mut queues = HashMap::<u64, Arc<PageQueue>>::new();
        for message in self.0.page.take() {
            if let ToPage::Window(id, _) | ToPage::Bridge(id, _) = message
                && id <= created
            {
                queues.entry(id).or_default().push(message);
            }
        }
        *pages = Some(queues);
    }

    pub(crate) fn create_window(&self, id: u64, command: Value) {
        self.0.created.fetch_max(id, Ordering::AcqRel);
        let options = &command["args"][2];
        if let Some(parent) = options["parent"].as_u64() {
            self.main(ToMain::Native(json!({"op":"window","args":[
                "parent", id, parent, options["modal"] == true, options["show"] != false,
            ]})));
        }
        match &mut *self.pages() {
            Some(pages) => pages
                .entry(id)
                .or_default()
                .push(ToPage::Window(id, command)),
            None => self.0.page.push(ToPage::Window(id, command)),
        }
        self.main(ToMain::Spawn(id));
    }

    pub(crate) fn wake_page(&self, window: u64) {
        match &*self.pages() {
            None => self.0.page.wake(),
            Some(pages) => pages
                .get(&window)
                .into_iter()
                .for_each(|queue| queue.wake()),
        }
    }

    pub(crate) fn wake_pages(&self) {
        match &*self.pages() {
            None => self.0.page.wake(),
            Some(pages) => pages.values().for_each(|queue| queue.wake()),
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

    pub(crate) fn page(&self, message: ToPage) {
        let pages = self.pages();
        let Some(pages) = &*pages else {
            return self.0.page.push(message);
        };
        match message {
            ToPage::All(command) => {
                for queue in pages.values() {
                    queue.push(ToPage::All(command.clone()));
                }
            }
            ToPage::Window(id, _) | ToPage::Bridge(id, _) => match pages.get(&id) {
                Some(queue) => queue.push(message),
                None => debug_assert!(
                    matches!(message, ToPage::Bridge(..))
                        || id <= self.0.created.load(Ordering::Acquire),
                    "No window {id} for a page command"
                ),
            },
        }
    }

    fn forget_page(&self, window: u64) {
        if let Some(pages) = &mut *self.pages() {
            pages.remove(&window);
        }
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

    pub(crate) fn quit(&self, code: i32) {
        self.0.exit_code.store(code, Ordering::Release);
        self.0.quit.store(true, Ordering::Release);
        self.wake_main();
    }

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

    // Blocks on main. Main must not wait on a page this way, or both can deadlock.
    pub(crate) fn ask(&self, message: impl FnOnce(Answer) -> ToMain) -> Result<Payload> {
        let (answer, reply) = mpsc::sync_channel(1);
        self.main(message(answer));
        // Without a render thread, run main's work here to avoid waiting on this same thread.
        if let Some(main) = MAIN.with_borrow(Weak::upgrade) {
            main.receive_with(true);
        }
        reply.recv().context("The main process is unavailable")?
    }
}

thread_local! { static MAIN: RefCell<Weak<Main>> = RefCell::default(); }

pub(crate) fn drive_main() {
    if let Some(main) = MAIN.with_borrow(Weak::upgrade) {
        main.drive();
    }
}

pub(crate) fn native_action(action: &str) -> bool {
    matches!(
        action,
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
            | "buttonPosition"
            | "opacity"
            | "shadow"
            | "ignoreMouse"
            | "workspaces"
            | "contentProtection"
            | "focusable"
            | "aspectRatio"
            | "parent"
            | "input"
    )
}

pub(crate) enum Tool {
    Hud,
    DevTools,
}

pub(crate) fn tool(key: &str, modifiers: renderer::Modifiers) -> Option<Tool> {
    let primary = if cfg!(target_os = "macos") {
        modifiers.command()
    } else {
        modifiers.ctrl()
    };
    if key == "f" && modifiers.alt() && primary {
        Some(Tool::Hud)
    } else if key == "f12"
        || (key == "i"
            && ((modifiers.command() && modifiers.alt())
                || (modifiers.ctrl() && modifiers.shift())))
    {
        Some(Tool::DevTools)
    } else {
        None
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

pub(crate) struct Main {
    pub(crate) engine: Option<JsContext>,
    pub(crate) host: Rc<RefCell<Host>>,
    pub(crate) handlers: RefCell<HashMap<String, super::app::Handler>>,
    api: Rc<JavaScriptCore>,
    link: Link,
    pool: RefCell<Option<mpsc::Sender<Job>>>,
    windows: RefCell<HashMap<u64, (WindowId, WindowControl)>>,
    opened: RefCell<std::collections::BTreeSet<u64>>,
    inputs: RefCell<std::collections::BTreeSet<u64>>,
    spawner: RefCell<Option<Spawner>>,
    waiting: RefCell<Vec<Value>>,
    changed: RefCell<Vec<u64>>,
    tools: RefCell<Option<super::devtools::DevTools>>,
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
            inputs: RefCell::default(),
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

    pub(crate) fn first_window(&self) -> Option<u64> {
        self.opened.borrow().first().copied()
    }

    pub(crate) fn receive(&self) -> bool {
        self.receive_with(!self.driving.get())
    }

    // Nested AppKit loops must defer script messages until the outer script returns. Other messages can run.
    pub(crate) fn receive_with(&self, scripts: bool) -> bool {
        let messages = std::mem::take(
            &mut *self
                .link
                .0
                .main
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
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
            if sender.is_some_and(|id| !self.opened.borrow().contains(&id)) {
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
                        (&*self.spawner.borrow(), self.link.queue_of(id))
                    {
                        spawner(id, queue);
                    }
                }
                (ToMain::Window(id, window, control), _) => {
                    // A window that closed on its way here must not stay retained.
                    if self.opened.borrow().contains(&id) {
                        self.windows.borrow_mut().insert(id, (window, control));
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
                (ToMain::OpenTools, Some(_)) => match super::devtools::DevTools::new("Main") {
                    Ok(tools) => *self.tools.borrow_mut() = Some(tools),
                    Err(error) => {
                        eprintln!("apps: {error:#}");
                        self.link.fail();
                    }
                },
                (ToMain::OpenTools, None) => {}
                (ToMain::Event(..) | ToMain::Sync(..) | ToMain::Open(..), None) => {}
            }
        }
        if !later.is_empty() {
            let mut queue = self
                .link
                .0
                .main
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            later.append(&mut queue);
            *queue = later;
        }
        // A nested loop can run this inside a host call of the main script, so attaches wait for that call.
        if let Ok(mut host) = self.host.try_borrow_mut() {
            for (browser, parent) in host.browsers.take_attaches() {
                let window = self
                    .windows
                    .borrow()
                    .get(&parent)
                    .and_then(|(_, control)| control.raw_window_handle().ok());
                match window {
                    Some(window) => {
                        if let Err(error) = host.browsers.attach(browser, &NativeHandle(window)) {
                            eprintln!("apps: {error:#}");
                        }
                    }
                    None => host.browsers.defer_attach(browser, parent),
                }
            }
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
                    .map(|(_, control)| (control.state(), control.is_active()));
                match state {
                    Some((Some(state), focused)) => {
                        let event =
                            format!("__appsEvent('state',{id},{})", state_json(&state, focused));
                        if let Err(error) = engine.eval(&event, "apps:state") {
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
            .find_map(|(&id, (other, _))| (*other == window).then_some(id))
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
        let Ok(packaged) = self.host.try_borrow().map(|host| host.packaged) else {
            return false;
        };
        if !self.inputs.borrow().contains(&id)
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
            WindowEvent::CloseRequested if self.engine.is_some() => self
                .link
                .event(format!("__appsEvent('close',{id})"), "apps:close"),
            WindowEvent::CloseRequested => self.native(json!({"op":"window","args":["close",id]})),
        }
    }

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
        if action == "input" {
            let mut inputs = self.inputs.borrow_mut();
            if args[2] == true {
                inputs.insert(id);
            } else {
                inputs.remove(&id);
            }
            return;
        }
        if action == "close" {
            // A nested loop can run this inside a host call of the main script, so it waits for that call.
            if self.host.try_borrow_mut().is_err() {
                return self.link.main(ToMain::Native(command));
            }
            // On Windows a hide calls `window_event` at once, so the `windows` borrow must end first.
            let removed = self.windows.borrow_mut().remove(&id);
            if let Some((_, control)) = removed {
                control.change(WindowChange::Hide);
            }
            self.waiting
                .borrow_mut()
                .retain(|waiting| waiting["args"][1].as_u64() != Some(id));
            crate::jsc::stop_page(id);
            if let Ok(mut host) = self.host.try_borrow_mut()
                && let Err(error) = host.browsers.close_window(id)
            {
                eprintln!("apps: {error:#}");
            }
            self.link.page(ToPage::Window(id, command));
            self.link.forget_page(id);
            self.link
                .event(format!("__appsEvent('closed',{id})"), "apps:closed");
            self.inputs.borrow_mut().remove(&id);
            let mut opened = self.opened.borrow_mut();
            opened.remove(&id);
            if self.engine.is_none() && opened.is_empty() {
                self.link.quit(0);
            }
            return;
        }
        // AppKit can run a nested loop in these calls, which runs main tasks, so no borrow lasts across them.
        let (window, parent) = {
            let windows = self.windows.borrow();
            let Some((_, window)) = windows.get(&id) else {
                drop(windows);
                self.waiting.borrow_mut().push(command);
                return;
            };
            let parent = args[2]
                .as_u64()
                .filter(|_| action == "parent")
                .map(|parent| {
                    (
                        parent,
                        windows.get(&parent).map(|(_, control)| control.clone()),
                    )
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
            "aspectRatio" => WindowChange::AspectRatio(number(2).0, size(number(3), number(4))),
            "parent" => {
                // Windows start on their own threads, so the parent can arrive after its child.
                if let Some((parent, None)) = &parent
                    && self.opened.borrow().contains(parent)
                {
                    self.waiting.borrow_mut().push(command);
                    return;
                }
                window.change(WindowChange::Parent(
                    parent.and_then(|(_, control)| control?.raw_window_handle().ok()),
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
            "alwaysOnTop" => WindowChange::AlwaysOnTop(on),
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
        crate::host::idle(false);
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
                self.link
                    .page(ToPage::All(json!({"op":"logs","args":logs})));
            }
        }
        if super::mcp::poll_main() {
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
        self.tools.borrow_mut().take_if(|tools| tools.is_closed());
        if self.link.main_pending() {
            delay = delay.min(0);
        }
        Ok((delay != i64::MAX).then(|| delay.clamp(0, 1 << 31) as u64))
    }

    fn shrink_when_quiet(self: &Rc<Self>) {
        let main = Rc::downgrade(self);
        renderer::native::after(Duration::from_secs(1), move || {
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
    use crate::host::Role;

    #[test]
    fn commands_route_by_target_before_and_after_windows_get_threads() {
        let link = Link::new();
        link.create_window(1, json!({"op":"window","args":["create",1,{}]}));
        link.page(ToPage::Bridge(1, json!("early").into()));
        link.page(ToPage::Bridge(9, json!("nobody").into()));
        link.set_per_window();
        link.create_window(2, json!({"op":"window","args":["create",2,{}]}));
        let mut host = Host {
            link: Some(link.clone()),
            ..Host::default()
        };
        host.command(Target::All, json!({"op":"theme"}));
        host.command(
            Target::Window(2),
            json!({"op":"window","args":["zoom",2,2]}),
        );
        let kinds = |id| {
            link.queue_of(id)
                .unwrap()
                .take()
                .into_iter()
                .map(|message| match message {
                    ToPage::Window(window, command) => format!("{window}:{}", command["args"][0]),
                    ToPage::All(command) => format!("all:{}", command["op"]),
                    ToPage::Bridge(window, payload) => format!("{window}:{}", payload.value),
                })
                .collect::<Vec<_>>()
                .join(",")
        };
        assert_eq!(kinds(1), r#"1:"create",1:"early",all:"theme""#);
        assert_eq!(kinds(2), r#"2:"create",all:"theme",2:"zoom""#);
        assert!(link.queue_of(9).is_none());
        host.call(Role::Main, "quit", json!([3])).unwrap();
        assert!(link.quitting() && link.exit_code() == 3);
        assert!(link.queue_of(1).unwrap().take().is_empty());
    }

    #[test]
    fn nested_loops_defer_main_scripts_and_closed_windows_lose_their_messages() {
        let api = JavaScriptCore::load().unwrap();
        let host = Rc::new(RefCell::new(Host::default()));
        let engine = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
        engine
            .eval(
                "globalThis.runs=[];globalThis.__appsWindowOpen=id=>{runs.push('open'+id);return 'null'}",
                "test",
            )
            .unwrap();
        let link = Link::new();
        let main = Main::new(Some(engine), host, link.clone(), api);
        link.main(ToMain::Spawn(901));
        link.event("runs.push('event')".into(), "test");
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
        assert!(main.opened.borrow().contains(&902) && link.main_pending());
        assert_eq!(runs(), "");
        main.driving.set(false);
        main.receive();
        assert_eq!(runs(), "event");
        // Use a unique page ID: close stops it process-wide.
        main.native(json!({"op":"window","args":["close",901]}));
        crate::jsc::page_stopped(901);
        for id in [901, 902] {
            let (answer, reply) = mpsc::sync_channel(1);
            link.main(ToMain::Open(id, json!(["open", "", "", "", ""]), answer));
            main.receive();
            assert_eq!(reply.try_recv().is_ok(), id == 902);
        }
        assert_eq!(runs(), "event,open902");
    }

    #[test]
    fn main_filters_the_keys_of_listening_windows_outside_its_scripts() {
        use renderer::{Event, Key, Modifiers};
        let api = JavaScriptCore::load().unwrap();
        let host = Rc::new(RefCell::new(Host::default()));
        let engine = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
        engine
            .eval(
                "globalThis.keys=[];globalThis.__appsBeforeInput=(id,input)=>{keys.push(id+JSON.stringify(input));return input.code==='p'}",
                "test",
            )
            .unwrap();
        let main = Main::new(Some(engine), host.clone(), Link::new(), api);
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
        for tool in [
            key("f12", Modifiers::NONE),
            key("i", tools),
            key("f", tools),
        ] {
            assert!(!main.before_input(5, &down(tool), &|| false));
        }
        assert_eq!(keys(), "");
        host.borrow_mut().packaged = true;
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
    }
}
