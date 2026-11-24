use crate::jsc::payload::Payload;
use anyhow::{Context, Result};
use renderer::{WindowControl, WindowId};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
        mpsc,
    },
};

pub(crate) type Answer = mpsc::SyncSender<Result<Payload>>;
type Wake = Box<dyn Fn() + Send + Sync>;

pub(crate) enum ToMain {
    Event(&'static str, Vec<String>, &'static str),
    Bridge(Payload),
    Sync(Payload, Answer),
    Open(u64, Value, Answer),
    Spawn(u64),
    Window(u64, WindowId, WindowControl),
    Native(Value),
    OpenTools,
}

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

    pub(crate) fn queue_of(&self, window: u64) -> Option<Arc<PageQueue>> {
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

    // Main calls the function with these JSON arguments.
    pub(crate) fn call(&self, function: &'static str, args: Vec<String>, url: &'static str) {
        self.main(ToMain::Event(function, args, url));
    }

    pub(crate) fn event(&self, name: &str, id: u64, data: Option<Value>, url: &'static str) {
        let mut args = vec![Value::from(name).to_string(), id.to_string()];
        args.extend(data.map(|data| data.to_string()));
        self.call("__appsEvent", args, url);
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

    pub(crate) fn take_main(&self) -> VecDeque<ToMain> {
        std::mem::take(&mut self.0.main.lock().unwrap_or_else(PoisonError::into_inner))
    }

    // Puts deferred messages back before the ones that came since.
    pub(crate) fn defer_main(&self, mut later: VecDeque<ToMain>) {
        let mut queue = self.0.main.lock().unwrap_or_else(PoisonError::into_inner);
        later.append(&mut queue);
        *queue = later;
    }

    pub(crate) fn spawning(&self, window: u64) -> bool {
        self.0
            .main
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|message| matches!(message, ToMain::Spawn(id) if *id == window))
    }

    pub(crate) fn forget_window(&self, window: u64) {
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
        if let Some(receive) = RECEIVE_MAIN.get() {
            receive();
        }
        reply.recv().context("The main process is unavailable")?
    }
}

thread_local! {
    // Main's receive, on the thread that runs main.
    static RECEIVE_MAIN: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn set_receive_main(receive: fn()) {
    RECEIVE_MAIN.set(Some(receive));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{host::Host, jsc::Role};

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
}
