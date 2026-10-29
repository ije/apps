use crate::html::{Document, Node};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::HashMap, io::Write, path::PathBuf};

#[derive(Clone, Copy, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Main,
    Preload(u64),
    Renderer(u64),
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
    pub native_modules: HashMap<String, PathBuf>,
    pub assets: HashMap<String, PathBuf>,
    pub commands: Vec<Value>,
    pub pages: HashMap<u64, Page>,
    pub dom: HashMap<u64, crate::dom::State>,
    pub bridge: HashMap<u64, HashMap<String, crate::binary::Payload>>,
    pub browsers: crate::browser::State,
    pub logs: std::collections::VecDeque<Value>,
    pub graphics: HashMap<u64, crate::gpu::Gpu>,
    pub viewports: HashMap<u64, [f32; 2]>,
    pub exit_code: i32,
    pub theme: u32,
    pub theme_changed: bool,
}

impl Host {
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
        let theme = crate::browser::theme_state();
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

    pub fn call(&mut self, role: Role, request: Value) -> Result<Value> {
        let op = request["op"].as_str().context("Missing operation")?;
        let args = &request["args"];
        if op == "log" {
            let text = args[0].as_str().unwrap_or("");
            let _ = if matches!(args[1].as_str(), Some("error" | "warn")) {
                writeln!(std::io::stderr(), "{text}")
            } else {
                writeln!(std::io::stdout(), "{text}")
            };
            if self.logs.len() == 500 {
                self.logs.pop_front();
            }
            self.logs.push_back(json!({"method":"Runtime.consoleAPICalled","params":{"type":args[1].as_str().unwrap_or("log"),"text":args[0],"context":role}}));
            return Ok(Value::Null);
        }
        if let Role::Renderer(id) = role {
            ensure!(
                op.starts_with("dom.") || op.starts_with("gpu."),
                "Host operation is unavailable in page scripts: {op}"
            );
            if op == "dom.navigate" {
                ensure!(self.pages.contains_key(&id), "Window is closed");
                let url = args[0].as_str().context("Expected navigation URL")?;
                ensure!(
                    url.starts_with("apps:")
                        || url.starts_with("file:")
                        || url.starts_with("https:")
                        || url.starts_with("http:"),
                    "Unsupported navigation scheme"
                );
                self.commands.push(json!({"op":"window", "args":[if url.starts_with("http") {"loadURL"} else {"loadFile"},id,url,"page"]}));
                return Ok(Value::Null);
            }
            let page = self.pages.get_mut(&id).context("Window is closed")?;
            if let Some(op) = op.strip_prefix("gpu.") {
                let result = self.graphics.entry(id).or_default().call(op, args)?;
                if matches!(op, "submit" | "resetCanvas") {
                    page.dirty = true;
                }
                return Ok(result);
            }
            if op == "dom.viewport" {
                let viewport = self.viewports.get(&id).unwrap_or(&[800., 600.]);
                return Ok(json!([
                    viewport[0],
                    viewport[1],
                    self.dom
                        .get(&id)
                        .map_or(1., |state| state.pixel_ratio.max(1.)),
                    crate::browser::theme_state()
                ]));
            }
            if matches!(op, "dom.setAttr" | "dom.removeAttr")
                && let Some(node) = args[0]
                    .as_u64()
                    .and_then(|id| page.doc.nodes.get(id as usize))
                && let Some(key) = args[1].as_str()
                && !(key == "style" && node.inline.is_some())
                && (op == "dom.setAttr"
                    && node.attrs.get(key).map(String::as_str) == args[2].as_str()
                    || op == "dom.removeAttr" && !node.attrs.contains_key(key))
            {
                return Ok(Value::Null);
            }
            let state = self.dom.entry(id).or_default();
            if op == "dom.imageURL" {
                let node = args[0].as_u64().context("Invalid node")? as usize;
                ensure!(node < page.doc.nodes.len(), "Invalid node id");
                let base = state.base.as_ref().context("Missing page URL")?;
                let url = base.join(args[1].as_str().context("Expected image URL")?)?;
                let path = match url.scheme() {
                    "apps" => self.assets.get(url.path().trim_start_matches('/')).cloned(),
                    "file" if base.scheme() == "file" => url.to_file_path().ok(),
                    _ => None,
                }
                .context("Image is not a bundled asset or local file")?;
                let metadata = std::fs::metadata(&path)?;
                ensure!(
                    metadata.is_file() && metadata.len() <= 64 * 1024 * 1024,
                    "Image is not a file or is too large"
                );
                let result = crate::dom::load_image(state, node, &std::fs::read(&path)?);
                page.dirty |= std::mem::take(&mut state.dirty);
                return result;
            }
            if matches!(
                op,
                "dom.setText"
                    | "dom.insert"
                    | "dom.remove"
                    | "dom.setAttr"
                    | "dom.removeAttr"
                    | "dom.html"
                    | "dom.shadow"
                    | "dom.font"
                    | "dom.focus"
            ) {
                let connected = |id: usize| {
                    let mut current = Some(id);
                    while let Some(id) = current {
                        if id == 0 {
                            return true;
                        }
                        current =
                            page.doc
                                .nodes
                                .get(id)
                                .and_then(|node| node.parent)
                                .or_else(|| {
                                    state
                                        .shadows
                                        .iter()
                                        .find_map(|(&host, &root)| (root == id).then_some(host))
                                });
                    }
                    false
                };
                if matches!(op, "dom.font" | "dom.focus")
                    || args[0].as_u64().is_some_and(|id| connected(id as usize))
                    || op == "dom.insert"
                        && args[1].as_u64().is_some_and(|id| connected(id as usize))
                {
                    state.computed = None;
                }
            }
            if let Some(result) = crate::dom::call(
                op,
                args,
                &mut page.doc,
                state,
                *self.viewports.get(&id).unwrap_or(&[800., 600.]),
            ) {
                page.dirty |= std::mem::take(&mut state.dirty);
                if op == "dom.release"
                    && let Ok(ids) = &result
                    && let Some(gpu) = self.graphics.get_mut(&id)
                {
                    for node in ids.as_array().into_iter().flatten() {
                        let _ = gpu.call("resetCanvas", &json!([node]));
                    }
                }
                return result;
            }
            if op == "dom.create" {
                let tag = args[0].as_str().context("Expected tag")?;
                ensure!(
                    tag == "#text" || tag.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
                    "Invalid tag"
                );
                ensure!(!tag.is_empty(), "Invalid tag");
                let node = state.allocate(
                    &mut page.doc,
                    Node {
                        tag: tag.to_ascii_lowercase(),
                        text: args[1].as_str().unwrap_or("").into(),
                        ..Node::default()
                    },
                );
                return Ok(json!(node));
            }
            let node = args[0].as_u64().context("Invalid node")? as usize;
            ensure!(
                page.doc
                    .nodes
                    .get(node)
                    .is_some_and(|node| !node.tag.is_empty()),
                "Invalid node"
            );
            return match op {
                "dom.node" => {
                    let node = &page.doc.nodes[node];
                    Ok(json!({"tag":node.tag,"parent":node.parent,"children":node.children}))
                }
                "dom.text" => Ok(json!(page.doc.text(node))),
                "dom.setText" => {
                    page.doc.nodes[node].text = args[1].as_str().context("Expected text")?.into();
                    page.doc.nodes[node].stylesheet = None;
                    for child in std::mem::take(&mut page.doc.nodes[node].children) {
                        page.doc.nodes[child].parent = None;
                    }
                    page.dirty = true;
                    Ok(Value::Null)
                }
                "dom.insert" => {
                    let child = args[1].as_u64().context("Invalid child")? as usize;
                    ensure!(child > 0 && child < page.doc.nodes.len(), "Invalid child");
                    ensure!(
                        !page.doc.nodes[node].tag.starts_with('#')
                            || node == 0
                            || page.doc.nodes[node].tag == "#fragment",
                        "Invalid parent"
                    );
                    let before = args[2].as_u64().map(|id| id as usize);
                    if let Some(before) = before {
                        ensure!(
                            page.doc.nodes[node].children.contains(&before),
                            "Reference is not a child"
                        );
                    }
                    let mut ancestor = Some(node);
                    while let Some(id) = ancestor {
                        ensure!(id != child, "Cannot create a DOM cycle");
                        ancestor = page.doc.nodes[id].parent;
                    }
                    if before != Some(child) {
                        if let Some(parent) = page.doc.nodes[child].parent {
                            page.doc.nodes[parent].children.retain(|&id| id != child);
                        }
                        let children = &mut page.doc.nodes[node].children;
                        let index = before.map_or(children.len(), |id| {
                            children.iter().position(|&c| c == id).unwrap()
                        });
                        children.insert(index, child);
                        page.doc.nodes[child].parent = Some(node);
                        page.dirty = true;
                    }
                    Ok(Value::Null)
                }
                "dom.remove" => {
                    let child = args[1].as_u64().context("Invalid child")? as usize;
                    ensure!(
                        page.doc.nodes[node].children.contains(&child),
                        "Node is not a child"
                    );
                    page.doc.nodes[node].children.retain(|&id| id != child);
                    page.doc.nodes[child].parent = None;
                    page.dirty = true;
                    Ok(Value::Null)
                }
                "dom.attr" => {
                    let key = args[1].as_str().context("Expected attribute")?;
                    let node = &page.doc.nodes[node];
                    if key == "style"
                        && let Some(inline) = &node.inline
                    {
                        Ok(json!(inline.source()))
                    } else {
                        Ok(json!(node.attrs.get(key)))
                    }
                }
                "dom.setAttr" => {
                    let key = args[1].as_str().context("Expected attribute")?;
                    page.doc.nodes[node].attrs.insert(
                        key.into(),
                        args[2].as_str().context("Expected attribute value")?.into(),
                    );
                    if key == "style" {
                        page.doc.nodes[node].inline = None;
                    }
                    page.dirty = true;
                    Ok(Value::Null)
                }
                "dom.removeAttr" => {
                    let key = args[1].as_str().context("Expected attribute")?;
                    page.doc.nodes[node].attrs.remove(key);
                    if key == "style" {
                        page.doc.nodes[node].inline = None;
                    }
                    page.dirty = true;
                    Ok(Value::Null)
                }
                _ => bail!("Unknown DOM operation: {op}"),
            };
        }
        match op {
            "process.write" => {
                let bytes = crate::node::bytes(&args[1])?;
                if args[0] == 2 {
                    std::io::stderr().write_all(&bytes)?;
                } else {
                    std::io::stdout().write_all(&bytes)?;
                }
                Ok(Value::Null)
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
                Ok(
                    json!({"platform":if cfg!(target_os="macos"){"darwin"}else if cfg!(windows){"win32"}else{"linux"},"arch":if cfg!(target_arch="aarch64"){"arm64"}else if cfg!(target_arch="x86_64"){"x64"}else{std::env::consts::ARCH},"execPath":std::env::current_exe().ok(),"locale":locale,"appPath":self.app_path,"version":crate::VERSION,"javascriptcore":if std::env::var_os("APPS_JSC_LIBRARY").is_some(){"custom"}else{"bun"},"isPackaged":self.packaged,"appName":self.app_name.as_deref().unwrap_or("apps"),"appVersion":self.app_version.as_deref().unwrap_or("0.0.0"),"pid":std::process::id(),"argv":std::env::args().collect::<Vec<_>>(),"env":std::env::vars().collect::<HashMap<_,_>>(),"homedir":std::env::var(if cfg!(windows){"USERPROFILE"}else{"HOME"}).unwrap_or_default(),"tmpdir":std::env::temp_dir(),"hostname":crate::node::hostname(),"parallelism":std::thread::available_parallelism().map_or(1,usize::from)}),
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
                if op == "window" && args[0] == "create" {
                    let options = &args[2];
                    if let Some(object) = options.as_object() {
                        for key in object.keys() {
                            ensure!(
                                matches!(
                                    key.as_str(),
                                    "width" | "height" | "title" | "show" | "resizable"
                                ),
                                "Unsupported BrowserWindow option: {key}"
                            );
                        }
                    }
                    for (key, default) in [("width", 800.), ("height", 600.)] {
                        ensure!(
                            (1.0..=16384.0).contains(&options[key].as_f64().unwrap_or(default)),
                            "Window dimensions must be between 1 and 16384"
                        );
                    }
                }
                self.commands.push(request);
                Ok(Value::Null)
            }
            _ => bail!("Unsupported host operation: {op}"),
        }
    }
}
