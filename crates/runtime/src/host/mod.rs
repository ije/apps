pub mod context;
pub(crate) mod link;
mod ops;
pub(crate) mod window;
pub(crate) mod worker;

use crate::dom::Document;
use crate::jsc::{JavaScriptCore, Role};
use anyhow::{Context, Result};
use context::JsContext;
use link::Target;
use serde_json::{Value, json};
use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

pub(crate) const WORKER_PAGE: u64 = 0;

#[derive(Default)]
pub struct Page {
    pub doc: Document,
    pub state: crate::render::PageState,
    pub dirty: bool,
}

#[derive(Default)]
pub struct Host {
    pub(crate) link: Option<link::Link>,
    pub app: crate::bundle::AppInfo,
    pub paths: HashMap<String, PathBuf>,
    pub relaunch: Option<Vec<String>>,
    pub argv: Option<Vec<String>>,
    pub commands: Vec<Value>,
    pub pages: HashMap<u64, Page>,
    pub bridge: HashMap<u64, HashMap<String, crate::jsc::payload::Payload>>,
    pub dropped: HashMap<u64, Vec<String>>,
    pub browsers: crate::native::browser::State,
    pub logs: std::collections::VecDeque<Value>,
    pub graphics: HashMap<u64, crate::web::gpu::Gpu>,
    pub media: HashMap<u64, crate::web::media::Media>,
    pub viewports: HashMap<u64, [f32; 2]>,
    pub resized: Vec<u64>,
    pub window_changes: Vec<u64>,
    pub backgrounds: HashMap<u64, (u32, u8)>,
    pub theme: u32,
    pub theme_changed: bool,
    // The state that views of a closed page still write to until they go. Nothing reads it back.
    pub(crate) closed: crate::render::PageState,
}

impl Host {
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

    // The state of the page in window `id`.
    pub fn state(&mut self, id: u64) -> &mut crate::render::PageState {
        match self.pages.get_mut(&id) {
            Some(page) => &mut page.state,
            None => &mut self.closed,
        }
    }

    // The document of the page in window `id`, when it is open, its state, and the app.
    pub(crate) fn page_parts(
        &mut self,
        id: u64,
    ) -> (
        Option<&Document>,
        &mut crate::render::PageState,
        &crate::bundle::AppInfo,
    ) {
        match self.pages.get_mut(&id) {
            Some(page) => (Some(&page.doc), &mut page.state, &self.app),
            None => (None, &mut self.closed, &self.app),
        }
    }

    // The URL of the page in window `id`.
    pub(crate) fn page_url(&self, id: u64) -> Option<&url::Url> {
        self.pages.get(&id).map(|page| &page.state)?.base.as_ref()
    }

    // The nodes of each page: in its tree, allocated, and free for reuse.
    pub(crate) fn memory(&self) -> Value {
        Value::Object(
            self.pages
                .iter()
                .map(|(id, page)| {
                    let (mut attached, mut pending) = (0, vec![0]);
                    while let Some(node) = pending.pop() {
                        attached += 1;
                        pending.extend(&page.doc.nodes[node].children);
                    }
                    let free = self.pages.get(id).map(|page| &page.state).map_or(0, |state| state.tree.free_count());
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
        self.pages.insert(
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
        self.pages.remove(&id);
        self.viewports.remove(&id);
        self.backgrounds.remove(&id);
    }

    fn forget_page(&mut self, id: u64) {
        self.graphics.remove(&id);
        self.media.remove(&id);
        self.bridge.remove(&id);
        self.dropped.remove(&id);
    }

    pub(crate) fn set_background(&mut self, id: u64, color: (u32, u8)) {
        self.backgrounds.insert(id, color);
        if let Some(page) = self.pages.get_mut(&id) {
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
        for page in self.pages.values_mut() {
            page.dirty = true;
            page.state.styles.invalidate();
        }
    }

    // Only dom.batch and dom.html can change page trees: dom.js mirrors them for reads.
    pub(crate) fn batch(&mut self, id: u64, codes: &[u8], text: &str) -> Result<Value> {
        let page = self.pages.get_mut(&id).context("Window is closed")?;
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
