use crate::desktop::link::Target;
use crate::dom::{Document, Node};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, io::Write, path::PathBuf};

pub(crate) const WORKER_PAGE: u64 = 0;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Main,
    Preload(u64),
    Renderer(u64),
}

#[derive(Clone, Default)]
pub struct Waker(Option<std::sync::Arc<dyn Fn() + Send + Sync>>);

thread_local! { static WAKER: std::cell::RefCell<Waker> = std::cell::RefCell::default(); }
static CWD: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

thread_local! {
    pub(crate) static IDLE: std::cell::Cell<(Option<std::time::Instant>, Option<std::time::Instant>, std::time::Duration)> = std::cell::Cell::default();
}

pub(crate) fn idle(waiting: bool) {
    IDLE.with(|idle| {
        let (start, since, total) = idle.get();
        let now = std::time::Instant::now();
        idle.set(if waiting {
            (start.or(Some(now)), Some(now), total)
        } else {
            (
                start,
                None,
                total + since.map_or_else(Default::default, |since| now - since),
            )
        });
    });
}

impl Waker {
    pub fn install(wake: impl Fn() + Send + Sync + 'static) {
        WAKER.set(Self(Some(std::sync::Arc::new(wake))));
    }

    pub(crate) fn current() -> Self {
        WAKER.with_borrow(Clone::clone)
    }

    pub(crate) fn wake(&self) {
        if let Some(wake) = &self.0 {
            wake();
        }
    }
}

pub(crate) struct WakeOnDrop(pub(crate) Waker);

impl Drop for WakeOnDrop {
    fn drop(&mut self) {
        self.0.wake();
    }
}

fn connected(doc: &Document, hosts: &rustc_hash::FxHashMap<usize, usize>, mut id: usize) -> bool {
    loop {
        if id == 0 {
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
    pub(crate) link: Option<crate::desktop::link::Link>,
    pub rust_main: bool,
    pub app_name: Option<String>,
    pub app_version: Option<String>,
    pub app_path: PathBuf,
    pub paths: HashMap<String, PathBuf>,
    pub relaunch: Option<Vec<String>>,
    pub argv: Option<Vec<String>>,
    pub packaged: bool,
    pub disabled: u32,
    pub native_modules: HashMap<String, PathBuf>,
    pub assets: HashMap<String, PathBuf>,
    pub scripts: std::sync::Arc<HashMap<String, bytes::Bytes>>,
    pub commands: Vec<Value>,
    pub pages: HashMap<u64, Page>,
    pub dom: rustc_hash::FxHashMap<u64, crate::dom::State>,
    pub bridge: HashMap<u64, HashMap<String, crate::jsc::payload::Payload>>,
    pub dropped: HashMap<u64, Vec<String>>,
    pub browsers: crate::desktop::browser::State,
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
    pub(crate) fn install(
        &mut self,
        files: &bbtree::unsync::BBTreeMap<String, String>,
    ) -> Result<()> {
        let paths: HashMap<String, PathBuf> =
            serde_json::from_str(files.get("native.json").map_or("{}", String::as_str))?;
        self.native_modules.extend(paths);
        let mut scripts = files
            .iter()
            .filter(|(name, _)| name.starts_with("script-"))
            .peekable();
        if scripts.peek().is_some() {
            std::sync::Arc::make_mut(&mut self.scripts)
                .extend(scripts.map(|(name, source)| (name.clone(), source.clone().into())));
        }
        Ok(())
    }

    pub(crate) fn command(&mut self, target: Target, command: Value) {
        use crate::desktop::link::{ToMain, ToPage};
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

    pub fn enabled(&self, feature: &str) -> bool {
        self.disabled & crate::bundle::feature_bit(feature) == 0
    }

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
                .join(self.app_name.as_deref().unwrap_or("apps")),
            "sessionData" => self.app_dir("userData")?,
            "logs" if cfg!(target_os = "macos") => home
                .join("Library/Logs")
                .join(self.app_name.as_deref().unwrap_or("apps")),
            "logs" => self.app_dir("userData")?.join("logs"),
            _ => bail!("Unsupported app path: {name}"),
        })
    }

    pub(crate) fn sync_theme(&mut self) {
        let theme = crate::desktop::native::theme_state();
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
                        if !(current.tag == "#text"
                            && current.text.is_empty() == value.is_empty()
                            && current
                                .parent
                                .is_none_or(|parent| doc.nodes[parent].tag != "style"))
                            && state.computed.is_some()
                            && connected(doc, &state.hosts, id)
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
                        let mut ancestor = Some(parent);
                        while let Some(id) = ancestor {
                            ensure!(id != child, "Cannot create a DOM cycle");
                            ancestor = doc.nodes[id]
                                .parent
                                .or_else(|| state.hosts.get(&id).copied());
                        }
                        if state.computed.is_some()
                            && (connected(doc, &state.hosts, parent)
                                || connected(doc, &state.hosts, child))
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
                        if state.computed.is_some() && connected(doc, &state.hosts, parent) {
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
                        if state.computed.is_some() && connected(doc, &state.hosts, id) {
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
                        let mut current = Some(host);
                        while let Some(id) = current {
                            ensure!(id != root, "Cannot create a DOM cycle");
                            current = doc.nodes[id]
                                .parent
                                .or_else(|| state.hosts.get(&id).copied());
                        }
                        if state.computed.is_some() && connected(doc, &state.hosts, host) {
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
                        if state.computed.is_some()
                            && empty(current.value.as_ref()) != empty(value.as_ref())
                            && connected(doc, &state.hosts, id)
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
                        if state.computed.is_some() && connected(doc, &state.hosts, id) {
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

    pub fn gpu(&mut self, id: u64, op: &str, args: &[u8]) -> Result<Value> {
        ensure!(self.enabled("webgpu"), "The webgpu feature is disabled");
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
        let disabled = self.disabled;
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
                let enabled = |feature| disabled & crate::bundle::feature_bit(feature) == 0;
                let ffmpeg = enabled("media");
                ensure!(
                    ffmpeg || !video && enabled("simpleAudio"),
                    "The media feature is disabled. Add \"media\" to \"features\" in package.json"
                );
                if args[1].is_string() {
                    let path = local_file(
                        &self.assets,
                        state,
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
            if !self.packaged {
                if self.logs.len() == 500 {
                    self.logs.pop_front();
                }
                let text: String = text.chars().take(16384).collect();
                self.logs.push_back(json!({"method":"Runtime.consoleAPICalled","params":{"type":args[1].as_str().unwrap_or("log"),"text":text,"context":role,"ref":args[2]}}));
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
                    crate::desktop::native::theme_state()
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
                let system = state.text_system.clone();
                crate::dom::styles(
                    &mut page.doc,
                    state,
                    *self.viewports.get(&id).unwrap_or(&[800., 600.]),
                    system.as_deref().map(|system| (system, state.pixel_ratio)),
                );
                let loaded = crate::render::fonts::load(&page.doc, state, &self.assets);
                page.dirty |= std::mem::take(&mut state.dirty);
                return Ok(json!(loaded));
            }
            if op == "dom.imageURL" {
                let node = args[0].as_u64().context("Invalid node")? as usize;
                ensure!(node < page.doc.nodes.len(), "Invalid node id");
                let path = local_file(
                    &self.assets,
                    state,
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
                        .is_some_and(|id| connected(&page.doc, &state.hosts, id as usize))
            {
                state.invalidate();
            }
            let result = crate::dom::call(
                op,
                args,
                &mut page.doc,
                state,
                *self.viewports.get(&id).unwrap_or(&[800., 600.]),
            )
            .unwrap_or_else(|| Err(anyhow::anyhow!("Unknown DOM operation: {op}")));
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
                if let Some(before) = crate::jsc::BEFORE_EXIT.get() {
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
                if self.packaged {
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
                if !self.packaged
                    && !node
                    && let Some(end) = argv.iter().position(|arg| arg == "--")
                {
                    argv.drain(1..=end);
                }
                let env: HashMap<_, _> = std::env::vars_os()
                    .map(|(name, value)| (lossy(name), lossy(value)))
                    .collect();
                Ok(
                    json!({"node":node,"rustMain":self.rust_main,"platform":if cfg!(target_os="macos"){"darwin"}else if cfg!(windows){"win32"}else{"linux"},"arch":if cfg!(target_arch="aarch64"){"arm64"}else if cfg!(target_arch="x86_64"){"x64"}else{std::env::consts::ARCH},"execPath":std::env::current_exe().ok(),"locale":locale,"appPath":self.app_path,"version":crate::VERSION,"javascriptcore":if std::env::var_os("APPS_JSC_LIBRARY").is_some(){"custom"}else{"bun"},"isPackaged":self.packaged,"appName":self.app_name.as_deref().unwrap_or("apps"),"appVersion":self.app_version.as_deref().unwrap_or("0.0.0"),"pid":std::process::id(),"argv":argv,"env":env,"homedir":std::env::var(if cfg!(windows){"USERPROFILE"}else{"HOME"}).unwrap_or_default(),"tmpdir":std::env::temp_dir(),"hostname":crate::node::hostname(),"parallelism":std::thread::available_parallelism().map_or(1,usize::from)}),
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
                if self.packaged && op == "window" && args[0] == "devtools" {
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
                    _ => {
                        crate::desktop::validate(args)?;
                        let id = args[1].as_u64().context("Expected a window id")?;
                        if crate::desktop::link::native_action(args[0].as_str().unwrap_or("")) {
                            Target::Main
                        } else {
                            Target::Window(id)
                        }
                    }
                };
                self.command(target, json!({"op": op, "args": args}));
                Ok(Value::Null)
            }
            _ => bail!("Unsupported host operation: {op}"),
        }
    }
}

pub(crate) fn local_file(
    assets: &HashMap<String, PathBuf>,
    state: &crate::dom::State,
    url: &str,
) -> Result<Option<PathBuf>> {
    let base = state.base.as_ref().context("Missing page URL")?;
    Ok(local_path(assets, base, &base.join(url)?))
}

// The files a page may name: its bundled assets, and local files only for a page that is a local file itself.
// A file URL with a host is a network share, which is never read.
pub(crate) fn local_path(
    assets: &HashMap<String, PathBuf>,
    base: &url::Url,
    url: &url::Url,
) -> Option<PathBuf> {
    match url.scheme() {
        "apps" => assets.get(url.path().trim_start_matches('/')).cloned(),
        "file" if base.scheme() == "file" && url.host_str().is_none_or(str::is_empty) => {
            url.to_file_path().ok()
        }
        _ => None,
    }
}
