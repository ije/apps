use crate::dom::{Document, Node};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, io::Write, path::PathBuf};

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Main,
    Preload(u64),
    Renderer(u64),
}

/// Wakes the event loop of the thread that started a native operation.
#[derive(Clone, Default)]
pub struct Waker(Option<std::sync::Arc<dyn Fn() + Send + Sync>>);

thread_local! { static WAKER: std::cell::RefCell<Waker> = std::cell::RefCell::default(); }

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

/// Wakes when dropped, for state that `poll` only sees after a task ends or is cancelled.
pub(crate) struct WakeOnDrop(pub(crate) Waker);

impl Drop for WakeOnDrop {
    fn drop(&mut self) {
        self.0.wake();
    }
}

fn connected(doc: &Document, hosts: &HashMap<usize, usize>, mut id: usize) -> bool {
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
    pub app_name: Option<String>,
    pub app_version: Option<String>,
    pub app_path: PathBuf,
    pub paths: HashMap<String, PathBuf>,
    pub relaunch: Option<Vec<String>>,
    pub packaged: bool,
    /// Features that package.json disables, as bits by `bundle::FEATURES` index.
    pub disabled: u32,
    pub native_modules: HashMap<String, PathBuf>,
    pub assets: HashMap<String, PathBuf>,
    pub scripts: std::sync::Arc<HashMap<String, bytes::Bytes>>,
    pub commands: Vec<Value>,
    pub pages: HashMap<u64, Page>,
    pub dom: HashMap<u64, crate::dom::State>,
    pub bridge: HashMap<u64, HashMap<String, crate::jsc::payload::Payload>>,
    pub browsers: crate::desktop::browser::State,
    pub logs: std::collections::VecDeque<Value>,
    pub graphics: HashMap<u64, crate::web::gpu::Gpu>,
    pub media: HashMap<u64, crate::web::media::Media>,
    pub viewports: HashMap<u64, [f32; 2]>,
    pub resized: Vec<u64>,
    pub window_changes: Vec<u64>,
    pub backgrounds: HashMap<u64, (u32, u8)>,
    pub exit_code: i32,
    pub theme: u32,
    pub theme_changed: bool,
}

impl Host {
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
            state.computed = None;
        }
    }

    /// Applies the page writes that `dom.js` queued: `code, a, b, c` runs of 32-bit integers, where string arguments are UTF-16 lengths of consecutive parts of `text`.
    // WARNING: `dom.js` mirrors the tree for reads, so only this batch and `dom.html` may change nodes, attributes or text of a page.
    pub(crate) fn batch(&mut self, id: u64, codes: &[u8], mut text: &str) -> Result<Value> {
        let node = |doc: &Document, value: i32| {
            usize::try_from(value)
                .ok()
                .filter(|&id| doc.nodes.get(id).is_some_and(|node| !node.tag.is_empty()))
                .context("Invalid node")
        };
        /// Drops the children that left `parent` while its list was stale.
        fn prune(doc: &mut Document, parent: usize) {
            let mut children = std::mem::take(&mut doc.nodes[parent].children);
            children.retain(|&child| doc.nodes[child].parent == Some(parent));
            doc.nodes[parent].children = children;
        }
        /// Inserts the children of a run before `before`, or at the end.
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
        // WARNING: removals from one parent and inserts at one place edit the children list once per run, so emptying or filling a list is O(n). Parent links are always current, while `stale` still lists children that left it and `run` holds children not in their parent's list yet.
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
            // WARNING: pages validate writes before they queue them, so a failed operation is skipped and reported instead of stopping the batch.
            let result = (|| -> Result<bool> {
                Ok(match code {
                    0 => {
                        let (id, tag) = (usize::try_from(a).context("Invalid node")?, parts[0]);
                        ensure!(
                            matches!(tag, "#text" | "#comment" | "#fragment" | "#document")
                                || !tag.is_empty()
                                    && tag.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
                            "Invalid tag"
                        );
                        ensure!(state.reserved.remove(&id), "Invalid node");
                        state.place(
                            doc,
                            id,
                            // WARNING: pages lower-case HTML names themselves; SVG and other namespaces keep their case.
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
                        // WARNING: selectors only see whether text is empty and layout diffs the text itself, so other text edits keep the styles.
                        if !(current.tag == "#text"
                            && current.text.is_empty() == value.is_empty()
                            && current
                                .parent
                                .is_none_or(|parent| doc.nodes[parent].tag != "style"))
                            && state.computed.is_some()
                            && connected(doc, &state.hosts, id)
                        {
                            state.computed = None;
                        }
                        doc.nodes[id].text = value.into();
                        doc.nodes[id].stylesheet = None;
                        for child in std::mem::take(&mut doc.nodes[id].children) {
                            doc.nodes[child].parent = None;
                            state.detached.push(child);
                        }
                        true
                    }
                    2 => {
                        let (parent, child) = (node(doc, a)?, node(doc, b)?);
                        ensure!(child > 0, "Invalid child");
                        let tag = &doc.nodes[parent].tag;
                        ensure!(
                            !tag.starts_with('#') || parent == 0 || tag == "#fragment",
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
                            state.computed = None;
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
                            state.computed = None;
                        }
                        doc.nodes[child].parent = None;
                        state.detached.push(child);
                        true
                    }
                    4 | 5 => {
                        let (id, key) = (node(doc, a)?, parts[0]);
                        let value = (code == 4).then_some(parts[1]);
                        let current = &doc.nodes[id];
                        if !(key == "style" && current.inline.is_some())
                            && current.attrs.get(key).map(String::as_str) == value
                        {
                            return Ok(false);
                        }
                        if state.computed.is_some() && connected(doc, &state.hosts, id) {
                            state.computed = None;
                        }
                        let current = &mut doc.nodes[id];
                        match value {
                            Some(value) => current.attrs.set(key, value),
                            None => drop(current.attrs.remove(key)),
                        }
                        if key == "style" {
                            current.inline = None;
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
                            state.computed = None;
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
                        // WARNING: selectors only see whether a value is empty, so other edits keep the styles.
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
                            state.computed = None;
                        }
                        doc.nodes[id].value = value;
                        true
                    }
                    8 => {
                        let id = node(doc, a)?;
                        let bits = [crate::dom::CHECKED, crate::dom::DIRTY, crate::dom::MODAL]
                            .into_iter()
                            .enumerate()
                            .fold(0, |bits, (index, bit)| {
                                bits | if b & 1 << index != 0 { bit } else { 0 }
                            });
                        let mask = crate::dom::CHECKED | crate::dom::DIRTY | crate::dom::MODAL;
                        if doc.nodes[id].state & mask == bits {
                            return Ok(false);
                        }
                        if state.computed.is_some() && connected(doc, &state.hosts, id) {
                            state.computed = None;
                        }
                        let node = &mut doc.nodes[id];
                        node.state = node.state & !mask | bits;
                        true
                    }
                    _ => bail!("Unknown DOM operation"),
                })
            })();
            match result {
                Ok(changed) => page.dirty |= changed,
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

    /// Runs WebGPU `op` of page `id` with arguments as JSON text.
    pub fn gpu(&mut self, id: u64, op: &str, args: &[u8]) -> Result<Value> {
        ensure!(self.enabled("webgpu"), "The webgpu feature is disabled");
        let page = self.pages.get_mut(&id).context("Window is closed")?;
        let result = self.graphics.entry(id).or_default().call(op, args)?;
        if matches!(op, "submit" | "writeTexture" | "resetCanvas") {
            page.dirty = true;
        }
        Ok(result)
    }

    /// Runs media `op` of page `id`. Frames that are due go to the page images.
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
                let feature = if video { "video" } else { "audio" };
                ensure!(
                    disabled & crate::bundle::feature_bit(feature) == 0,
                    "The {feature} feature is disabled. Add \"{feature}\" to \"features\" in package.json"
                );
                if args[1].is_string() {
                    let path = local_file(&self.assets, state, &args[1])?
                        .context("Media is not a bundled asset or local file")?;
                    media.open(node, Ok(&path), video)?;
                } else {
                    media.open(node, Err(crate::node::bytes(&args[1])?), video)?;
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
            // WARNING: only developer tools drain logs, and packaged apps have none.
            if !self.packaged {
                if self.logs.len() == 500 {
                    self.logs.pop_front();
                }
                let text: String = text.chars().take(16384).collect();
                self.logs.push_back(json!({"method":"Runtime.consoleAPICalled","params":{"type":args[1].as_str().unwrap_or("log"),"text":text,"context":role}}));
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
                // WARNING: like `loadURL`, pages cannot navigate the window to remote or non-local URLs.
                ensure!(
                    url.starts_with("apps:///") || url.starts_with("file:///"),
                    "Pages can only navigate to bundled or local files"
                );
                self.commands
                    .push(json!({"op":"window", "args":["loadFile",id,url,"page"]}));
                return Ok(Value::Null);
            }
            if op == "dom.executed" {
                ensure!(
                    args[0].is_u64() && args[1].is_string(),
                    "Invalid script result"
                );
                self.commands
                    .push(json!({"op":"window", "args":["executed",id,args[0],args[1]]}));
                return Ok(Value::Null);
            }
            let page = self.pages.get_mut(&id).context("Window is closed")?;
            if op == "dom.viewport" {
                let viewport = self.viewports.get(&id).unwrap_or(&[800., 600.]);
                return Ok(json!([
                    viewport[0],
                    viewport[1],
                    self.dom
                        .get(&id)
                        .map_or(1., |state| state.pixel_ratio.max(1.)),
                    crate::desktop::native::theme_state()
                ]));
            }
            let state = self.dom.entry(id).or_default();
            if op == "dom.imageURL" {
                let node = args[0].as_u64().context("Invalid node")? as usize;
                ensure!(node < page.doc.nodes.len(), "Invalid node id");
                let path = local_file(&self.assets, state, &args[1])?
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
                state.computed = None;
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
            if op == "dom.release"
                && let Ok(ids) = &result
            {
                for node in ids.as_array().into_iter().flatten() {
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
                let mut argv: Vec<_> = std::env::args_os().map(lossy).collect();
                if !self.packaged
                    && let Some(end) = argv.iter().position(|arg| arg == "--")
                {
                    argv.drain(1..=end);
                }
                let env: HashMap<_, _> = std::env::vars_os()
                    .map(|(name, value)| (lossy(name), lossy(value)))
                    .collect();
                Ok(
                    json!({"platform":if cfg!(target_os="macos"){"darwin"}else if cfg!(windows){"win32"}else{"linux"},"arch":if cfg!(target_arch="aarch64"){"arm64"}else if cfg!(target_arch="x86_64"){"x64"}else{std::env::consts::ARCH},"execPath":std::env::current_exe().ok(),"locale":locale,"appPath":self.app_path,"version":crate::VERSION,"javascriptcore":if std::env::var_os("APPS_JSC_LIBRARY").is_some(){"custom"}else{"bun"},"isPackaged":self.packaged,"appName":self.app_name.as_deref().unwrap_or("apps"),"appVersion":self.app_version.as_deref().unwrap_or("0.0.0"),"pid":std::process::id(),"argv":argv,"env":env,"homedir":std::env::var(if cfg!(windows){"USERPROFILE"}else{"HOME"}).unwrap_or_default(),"tmpdir":std::env::temp_dir(),"hostname":crate::node::hostname(),"parallelism":std::thread::available_parallelism().map_or(1,usize::from)}),
                )
            }
            "cwd" => Ok(json!(std::env::current_dir()?)),
            "chdir" => {
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
                if op == "window" {
                    crate::desktop::validate(args)?;
                }
                self.commands.push(json!({"op": op, "args": args}));
                Ok(Value::Null)
            }
            _ => bail!("Unsupported host operation: {op}"),
        }
    }
}

/// The file of a bundled asset URL, or of a local file URL in a local page.
fn local_file(
    assets: &HashMap<String, PathBuf>,
    state: &crate::dom::State,
    url: &Value,
) -> Result<Option<PathBuf>> {
    let base = state.base.as_ref().context("Missing page URL")?;
    let url = base.join(url.as_str().context("Expected a URL")?)?;
    Ok(match url.scheme() {
        "apps" => assets.get(url.path().trim_start_matches('/')).cloned(),
        "file" if base.scheme() == "file" && url.host_str().is_none_or(str::is_empty) => {
            url.to_file_path().ok()
        }
        _ => None,
    })
}
