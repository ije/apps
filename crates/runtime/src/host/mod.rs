pub mod context;
pub(crate) mod link;
mod ops;
pub(crate) mod window;
pub(crate) mod worker;

use crate::dom::Document;
use crate::jsc::{JavaScriptCore, Role};
use crate::render::{Page, Store};
use anyhow::{Context, Result};
use context::JsContext;
use link::Target;
use serde_json::{Value, json};
use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

pub(crate) const WORKER_PAGE: u64 = 0;

#[derive(Default)]
pub struct Host {
    pub(crate) link: Option<link::Link>,
    pub app: crate::bundle::AppInfo,
    pub paths: HashMap<String, PathBuf>,
    pub relaunch: Option<Vec<String>>,
    pub argv: Option<Vec<String>>,
    pub commands: Vec<Value>,
    pub store: Rc<RefCell<Store>>,
    pub bridge: HashMap<u64, HashMap<String, crate::jsc::payload::Payload>>,
    pub dropped: HashMap<u64, Vec<String>>,
    pub browsers: crate::native::browser::State,
    pub logs: std::collections::VecDeque<Value>,
    pub window_changes: Vec<u64>,
    pub theme: u32,
    pub theme_changed: bool,
}

impl Host {
    pub(crate) fn new(app: crate::bundle::AppInfo) -> Self {
        let host = Self::default();
        host.store.borrow_mut().set_app(&app);
        Self { app, ..host }
    }

    pub(crate) fn command(&mut self, target: Target, command: Value) {
        use link::{ToMain, ToPage};
        let Some(link) = &self.link else {
            return self.commands.push(command);
        };
        match target {
            Target::Main => link.main(ToMain::Native(command)),
            Target::All => link.page(ToPage::All(command)),
            Target::Window(id) if command["args"][0] == "create" => link.create_window(id, command),
            Target::Window(id) => link.page(ToPage::Window(id, command)),
        }
    }

    // The URL of the page in window `id`.
    pub(crate) fn page_url(&self, id: u64) -> Option<url::Url> {
        self.store.borrow().page_state(id)?.base.clone()
    }

    // The nodes of each page: in its tree, allocated, and free for reuse.
    pub(crate) fn memory(&self) -> Value {
        Value::Object(
            self.store
                .borrow()
                .pages
                .iter()
                .map(|(id, page)| {
                    let (mut attached, mut pending) = (0, vec![0]);
                    while let Some(node) = pending.pop() {
                        attached += 1;
                        pending.extend(&page.doc.nodes[node].children);
                    }
                    let free = page.state.tree.free_count();
                    (
                        format!("window {id}"),
                        json!({"attached": attached, "allocated": page.doc.nodes.len(), "free": free}),
                    )
                })
                .collect(),
        )
    }

    // Puts a new page in window `id`, without the resources of the old one.
    pub(crate) fn load_page(&mut self, id: u64, doc: Document) {
        self.forget_page(id);
        self.store.borrow_mut().pages.insert(
            id,
            Page {
                doc,
                dirty: true,
                ..Default::default()
            },
        );
    }

    pub(crate) fn close_page(&mut self, id: u64) {
        self.forget_page(id);
        let mut store = self.store.borrow_mut();
        store.pages.remove(&id);
        store.closed = Default::default();
        store.viewports.remove(&id);
        store.backgrounds.remove(&id);
    }

    fn forget_page(&mut self, id: u64) {
        let mut store = self.store.borrow_mut();
        store.graphics.remove(&id);
        store.media.remove(&id);
        drop(store);
        self.bridge.remove(&id);
        self.dropped.remove(&id);
    }

    pub(crate) fn set_background(&mut self, id: u64, color: (u32, u8)) {
        let mut store = self.store.borrow_mut();
        store.backgrounds.insert(id, color);
        if let Some(page) = store.pages.get_mut(&id) {
            page.dirty = true;
        }
    }

    pub(crate) fn log(&mut self, entry: Value) {
        if self.logs.len() == 500 {
            self.logs.pop_front();
        }
        self.logs.push_back(entry);
    }

    pub(crate) fn sync_theme(&mut self) {
        let theme = crate::native::theme_state();
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        self.theme_changed = true;
        for page in self.store.borrow_mut().pages.values_mut() {
            page.dirty = true;
            page.state.styles.invalidate();
        }
    }

    // Only dom.batch and dom.html can change page trees: dom.js mirrors them for reads.
    pub(crate) fn batch(&mut self, id: u64, codes: &[u8], text: &str) -> Result<Value> {
        let mut store = self.store.try_borrow_mut().context("Host is busy")?;
        let page = store.pages.get_mut(&id).context("Window is closed")?;
        let (changed, errors) = page.state.batch(&mut page.doc, codes, text);
        page.dirty |= changed;
        Ok(if errors.is_empty() {
            Value::Null
        } else {
            json!(errors)
        })
    }
}

// The contexts of the app put together: their host calls go to the router, and their workers run in contexts
// of their own.
impl JsContext {
    pub fn new(api: Rc<JavaScriptCore>, host: Rc<RefCell<Host>>, role: Role) -> Result<Self> {
        Self::new_interruptible(api, host, role, None)
    }

    pub fn new_interruptible(
        api: Rc<JavaScriptCore>,
        host: Rc<RefCell<Host>>,
        role: Role,
        interrupt: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<Self> {
        Self::create(api, host, role, interrupt, ops::call, worker::run)
    }
}
