pub mod context;
pub(crate) mod link;
mod ops;
pub(crate) mod window;
pub(crate) mod worker;

use crate::dom::{Document, Node};
use crate::jsc::{JavaScriptCore, Role};
use anyhow::{Context, Result, bail, ensure};
use context::JsContext;
use link::Target;
use serde_json::{Value, json};
use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

pub(crate) const WORKER_PAGE: u64 = 0;

pub(crate) fn reaches(
    doc: &Document,
    hosts: &rustc_hash::FxHashMap<usize, usize>,
    mut id: usize,
    to: usize,
) -> bool {
    loop {
        if id == to {
            return true;
        }
        match doc
            .nodes
            .get(id)
            .and_then(|node| node.parent)
            .or_else(|| hosts.get(&id).copied())
        {
            Some(parent) => id = parent,
            None => return false,
        }
    }
}

#[derive(Default)]
pub struct Page {
    pub doc: Document,
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
    pub dom: rustc_hash::FxHashMap<u64, crate::dom::State>,
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

    // The URL of the page in window `id`.
    pub(crate) fn page_url(&self, id: u64) -> Option<&url::Url> {
        self.dom.get(&id)?.base.as_ref()
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
                    let free = self.dom.get(id).map_or(0, |state| state.free.len());
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
        self.pages.insert(id, Page { doc, dirty: true });
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
        self.dom.remove(&id);
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
        }
        for state in self.dom.values_mut() {
            state.invalidate();
        }
    }

    // Only dom.batch and dom.html can change page trees: dom.js mirrors them for reads.
    pub(crate) fn batch(&mut self, id: u64, codes: &[u8], mut text: &str) -> Result<Value> {
        let node = |doc: &Document, value: i32| {
            usize::try_from(value)
                .ok()
                .filter(|&id| doc.nodes.get(id).is_some_and(|node| !node.tag.is_empty()))
                .context("Invalid node")
        };
        fn prune(doc: &mut Document, parent: usize) {
            let mut children = std::mem::take(&mut doc.nodes[parent].children);
            children.retain(|&child| doc.nodes[child].parent == Some(parent));
            doc.nodes[parent].children = children;
        }
        fn insert(
            doc: &mut Document,
            (parent, before): (usize, Option<usize>),
            run: &mut Vec<usize>,
        ) {
            let children = &mut doc.nodes[parent].children;
            let index = before
                .and_then(|before| children.iter().position(|&id| id == before))
                .unwrap_or(children.len());
            children.splice(index..index, run.drain(..));
        }
        let page = self.pages.get_mut(&id).context("Window is closed")?;
        let state = self.dom.entry(id).or_default();
        let doc = &mut page.doc;
        let mut errors = Vec::new();
        // Parent links are current during a batch. `stale` has obsolete child entries. `run` has children
        // awaiting insertion.
        let (mut stale, mut run, mut runs) = (None, None, Vec::new());
        'ops: for op in codes.as_chunks::<16>().0 {
            let [code, a, b, c] =
                [0, 4, 8, 12].map(|i| i32::from_ne_bytes([op[i], op[i + 1], op[i + 2], op[i + 3]]));
            let mut parts = ["", ""];
            for (part, units) in parts.iter_mut().zip([b, c]).take(match code {
                0 | 4 => 2,
                1 | 5 | 7 => 1,
                _ => 0,
            }) {
                let units = usize::try_from(units).unwrap_or(usize::MAX);
                let end = if text.as_bytes().get(..units).is_some_and(<[u8]>::is_ascii) {
                    units
                } else {
                    let (mut end, mut count) = (0, 0);
                    for character in text.chars() {
                        if count >= units {
                            break;
                        }
                        count += character.len_utf16();
                        end += character.len_utf8();
                    }
                    if count != units {
                        errors.push("Invalid DOM batch".into());
                        break 'ops;
                    }
                    end
                };
                (*part, text) = text.split_at(end);
            }
            let result = (|| -> Result<bool> {
                Ok(match code {
                    0 => {
                        let (id, tag) = (usize::try_from(a).context("Invalid node")?, parts[0]);
                        ensure!(
                            matches!(
                                tag,
                                "#text" | "#comment" | "#fragment" | "#document" | "#doctype"
                            ) || !tag.is_empty()
                                && tag.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
                            "Invalid tag"
                        );
                        ensure!(state.reserved.remove(&id), "Invalid node");
                        state.place(
                            doc,
                            id,
                            // Pages lower-case HTML names themselves; SVG and other namespaces keep their case.
                            Node {
                                tag: tag.into(),
                                text: parts[1].into(),
                                ..Node::default()
                            },
                        );
                        false
                    }
                    1 => {
                        let (id, value) = (node(doc, a)?, parts[0]);
                        if let Some(pending) = run.take_if(|&mut (parent, _)| parent == id) {
                            insert(doc, pending, &mut runs);
                        }
                        if stale.take_if(|&mut parent| parent == id).is_some() {
                            prune(doc, id);
                        }
                        let current = &doc.nodes[id];
                        if current.children.is_empty() && current.text == value {
                            return Ok(false);
                        }
                        if !(current.tag == "#text"
                            && current.text.is_empty() == value.is_empty()
                            && current
                                .parent
                                .is_none_or(|parent| doc.nodes[parent].tag != "style"))
                            && state.styles.computed.is_some()
                            && reaches(doc, &state.hosts, id, 0)
                        {
                            state.invalidate();
                        }
                        if doc.nodes[id].children.is_empty()
                            && let Some(current) = doc.nodes.edit(id)
                        {
                            current.text = value.into();
                            current.stylesheet = None;
                            return Ok(true);
                        }
                        doc.nodes[id].text = value.into();
                        doc.nodes[id].stylesheet = None;
                        for child in std::mem::take(&mut doc.nodes[id].children) {
                            doc.nodes[child].parent = None;
                            state.detached.push(child);
                            state.removed.push(child);
                        }
                        true
                    }
                    2 => {
                        let (parent, child) = (node(doc, a)?, node(doc, b)?);
                        ensure!(child > 0, "Invalid child");
                        let tag = &doc.nodes[parent].tag;
                        ensure!(
                            !tag.starts_with('#') || tag == "#document" || tag == "#fragment",
                            "Invalid parent"
                        );
                        let before = usize::try_from(c).ok();
                        ensure!(
                            before.is_none_or(|before| doc
                                .nodes
                                .get(before)
                                .is_some_and(|node| node.parent == Some(parent))),
                            "Reference is not a child"
                        );
                        ensure!(
                            !reaches(doc, &state.hosts, parent, child),
                            "Cannot create a DOM cycle"
                        );
                        if state.styles.computed.is_some()
                            && (reaches(doc, &state.hosts, parent, 0)
                                || reaches(doc, &state.hosts, child, 0))
                        {
                            state.invalidate();
                        }
                        if before == Some(child) {
                            return Ok(false);
                        }
                        if let Some(pending) = run.take_if(|&mut key| key != (parent, before)) {
                            insert(doc, pending, &mut runs);
                        }
                        if stale.take_if(|&mut stale| stale == parent).is_some() {
                            prune(doc, parent);
                        }
                        match doc.nodes[child].parent {
                            Some(old) if old == parent => {
                                if let Some(pending) = run.take() {
                                    insert(doc, pending, &mut runs);
                                }
                                let children = &mut doc.nodes[parent].children;
                                if let Some(index) = children.iter().position(|&id| id == child) {
                                    children.remove(index);
                                }
                                let index = before
                                    .and_then(|before| children.iter().position(|&id| id == before))
                                    .unwrap_or(children.len());
                                children.insert(index, child);
                                return Ok(true);
                            }
                            Some(old) => {
                                if let Some(previous) = stale.replace(old)
                                    && previous != old
                                {
                                    prune(doc, previous);
                                }
                            }
                            None => {}
                        }
                        doc.nodes[child].parent = Some(parent);
                        run = Some((parent, before));
                        runs.push(child);
                        true
                    }
                    3 => {
                        let parent = node(doc, a)?;
                        let child = usize::try_from(b)
                            .ok()
                            .filter(|&child| {
                                doc.nodes
                                    .get(child)
                                    .is_some_and(|node| node.parent == Some(parent))
                            })
                            .context("Node is not a child")?;
                        if let Some(pending) = run.take() {
                            insert(doc, pending, &mut runs);
                        }
                        if let Some(previous) = stale.replace(parent)
                            && previous != parent
                        {
                            prune(doc, previous);
                        }
                        if state.styles.computed.is_some() && reaches(doc, &state.hosts, parent, 0)
                        {
                            state.invalidate();
                        }
                        doc.nodes[child].parent = None;
                        state.detached.push(child);
                        state.removed.push(child);
                        true
                    }
                    4 | 5 => {
                        let (id, key) = (node(doc, a)?, parts[0]);
                        let value = (code == 4).then_some(parts[1]);
                        if doc.nodes[id].attrs.get(key).map(String::as_str) == value {
                            return Ok(false);
                        }
                        if state.styles.computed.is_some() && reaches(doc, &state.hosts, id, 0) {
                            state.invalidate();
                        }
                        // Kept walks of the tree read these attributes, so their edits must use `get_mut` and
                        // count as tree changes.
                        let current =
                            if matches!(key, "slot" | "name" | "contenteditable" | "inert") {
                                doc.nodes.get_mut(id)
                            } else {
                                doc.nodes.edit(id)
                            };
                        if let Some(current) = current {
                            match value {
                                Some(value) => current.attrs.set(key, value),
                                None => drop(current.attrs.remove(key)),
                            }
                            if key == "style" {
                                current.inline = None;
                            }
                        }
                        true
                    }
                    6 => {
                        let (host, root) = (node(doc, a)?, node(doc, b)?);
                        ensure!(
                            !doc.nodes[host].tag.starts_with('#')
                                && doc.nodes[root].tag == "#fragment"
                                && doc.nodes[root].parent.is_none(),
                            "Invalid shadow root"
                        );
                        ensure!(
                            !state.shadows.contains_key(&host) && !state.hosts.contains_key(&root),
                            "Shadow root already exists"
                        );
                        ensure!(
                            !reaches(doc, &state.hosts, host, root),
                            "Cannot create a DOM cycle"
                        );
                        if state.styles.computed.is_some() && reaches(doc, &state.hosts, host, 0) {
                            state.invalidate();
                        }
                        state.shadows.insert(host, root);
                        state.hosts.insert(root, host);
                        true
                    }
                    7 => {
                        let id = node(doc, a)?;
                        let value = (c == 0).then(|| parts[0].to_owned());
                        let current = &doc.nodes[id];
                        if current.value == value {
                            return Ok(false);
                        }
                        let fallback = current.attrs.get("value").map(String::as_str);
                        let empty = |value: Option<&String>| {
                            value
                                .map(String::as_str)
                                .or(fallback)
                                .is_none_or(str::is_empty)
                        };
                        if state.styles.computed.is_some()
                            && empty(current.value.as_ref()) != empty(value.as_ref())
                            && reaches(doc, &state.hosts, id, 0)
                        {
                            state.invalidate();
                        }
                        if let Some(current) = doc.nodes.edit(id) {
                            current.value = value;
                        }
                        true
                    }
                    8 => {
                        use crate::dom::{
                            BROKEN, CHECKED, DIRTY, FULLSCREEN, INDETERMINATE, INVALID, LEAVING,
                            MODAL, POPOVER, TOP_LAYER,
                        };
                        let id = node(doc, a)?;
                        let bits = [
                            CHECKED,
                            DIRTY,
                            MODAL,
                            POPOVER,
                            FULLSCREEN,
                            BROKEN,
                            INDETERMINATE,
                            INVALID,
                        ]
                        .into_iter()
                        .enumerate()
                        .fold(0, |bits, (index, bit)| {
                            bits | if b & 1 << index != 0 { bit } else { 0 }
                        });
                        let mask = CHECKED
                            | DIRTY
                            | MODAL
                            | POPOVER
                            | FULLSCREEN
                            | BROKEN
                            | INDETERMINATE
                            | INVALID;
                        let old = doc.nodes[id].state;
                        if old & mask == bits {
                            return Ok(false);
                        }
                        let leaving = if bits & TOP_LAYER == 0
                            && state.text_system.is_some()
                            && state.top_layer.contains(&id)
                        {
                            old & LEAVING | (old & TOP_LAYER) << 5
                        } else {
                            0
                        };
                        if state.styles.computed.is_some() && reaches(doc, &state.hosts, id, 0) {
                            state.invalidate();
                        }
                        if let Some(node) = doc.nodes.edit(id) {
                            node.state = node.state & !(mask | LEAVING) | bits | leaving;
                        }
                        true
                    }
                    9 => {
                        let id = node(doc, a)?;
                        if b != 0 || doc.nodes[id].state & crate::dom::LEAVING == 0 {
                            state.top_layer.retain(|&other| other != id);
                        }
                        if b != 0 {
                            state.top_layer.push(id);
                        }
                        true
                    }
                    // A copy of a node for `cloneNode`: tag, attributes, and the text of text and comments.
                    10 => {
                        let (id, from) =
                            (usize::try_from(a).context("Invalid node")?, node(doc, b)?);
                        ensure!(state.reserved.remove(&id), "Invalid node");
                        let from = &doc.nodes[from];
                        let text = match &from.stylesheet {
                            Some(sheet) if from.tag == "#text" => sheet.source(),
                            _ if from.tag.starts_with('#') => from.text.clone(),
                            _ => Default::default(),
                        };
                        let copy = Node {
                            tag: from.tag.clone(),
                            text,
                            attrs: from.attrs.clone(),
                            ..Node::default()
                        };
                        state.place(doc, id, copy);
                        false
                    }
                    _ => bail!("Unknown DOM operation"),
                })
            })();
            match result {
                // Offscreen canvas changes must not dirty the page or schedule frames.
                Ok(changed) => {
                    page.dirty |= changed
                        && usize::try_from(a)
                            .ok()
                            .and_then(|id| doc.nodes.get(id))
                            .is_none_or(|node| node.tag != "apps-canvas");
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
        if let Some(pending) = run {
            insert(doc, pending, &mut runs);
        }
        if let Some(parent) = stale {
            prune(doc, parent);
        }
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
