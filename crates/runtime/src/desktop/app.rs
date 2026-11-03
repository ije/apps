use super::Desktop;
use crate::{bundle::Bundle, host::Host};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    rc::{Rc, Weak},
};

static DEV_LIBRARIES: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

pub(crate) fn library_exe() -> std::io::Result<std::path::PathBuf> {
    match DEV_LIBRARIES.get() {
        Some(exe) => Ok(exe.clone()),
        None => std::env::current_exe()?.canonicalize(),
    }
}

pub(crate) type Handler = Rc<dyn Fn(u64, Vec<Value>) -> Result<Value>>;

#[derive(Clone)]
pub struct App(Rc<State>);

struct State {
    host: Weak<RefCell<Host>>,
    pages: BTreeSet<String>,
    next_id: Cell<u64>,
}

impl App {
    pub fn handle(
        &self,
        channel: &str,
        handler: impl Fn(u64, Vec<Value>) -> Result<Value> + 'static,
    ) -> Result<()> {
        let host = self.0.host.upgrade().context("App has exited")?;
        let mut host = host.borrow_mut();
        let handlers = host.rust_handlers.as_mut().unwrap();
        ensure!(
            !handlers.contains_key(channel),
            "Duplicate Rust handler {channel:?}"
        );
        handlers.insert(channel.to_owned(), Rc::new(handler));
        Ok(())
    }

    pub fn quit(&self) {
        if let Some(host) = self.0.host.upgrade() {
            host.borrow_mut()
                .commands
                .push(json!({"op":"quit","args":[0]}));
        }
    }
}

#[derive(serde::Serialize)]
pub struct WindowOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "apps".into(),
            width: 800,
            height: 600,
        }
    }
}

pub struct BrowserWindow {
    app: App,
    pub id: u64,
}

impl BrowserWindow {
    pub fn new(app: &App, options: WindowOptions) -> Result<Self> {
        let host = app.0.host.upgrade().context("App has exited")?;
        let id = app.0.next_id.get();
        app.0.next_id.set(id + 1);
        host.borrow_mut()
            .commands
            .push(json!({"op":"window","args":["create",id,options]}));
        Ok(Self {
            app: app.clone(),
            id,
        })
    }

    pub fn load_file(&self, path: &str) -> Result<()> {
        let path = path.strip_prefix("./").unwrap_or(path);
        ensure!(
            self.app.0.pages.contains(path),
            "Page {path:?} is not bundled"
        );
        let host = self.app.0.host.upgrade().context("App has exited")?;
        host.borrow_mut()
            .commands
            .push(json!({"op":"window","args":["loadFile", self.id, format!("apps:///{path}")]}));
        Ok(())
    }
}

pub fn run(setup: impl FnOnce(&App) -> Result<()>) -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let dir = exe.parent().context("Missing executable directory")?;
    let resources = if dir.ends_with("Contents/MacOS") {
        dir.join("../Resources")
    } else {
        dir.to_owned()
    };
    let dev = !resources.join("app.h2o").is_file() && std::env::var_os("APPS_RUST_DEV").is_some();
    let bundle: Bundle = if dev {
        if let Some(path) = std::env::var_os("APPS_RUST_LIBRARIES") {
            let _ = DEV_LIBRARIES.set(path.into());
        }
        serde_json::from_reader(std::io::BufReader::new(std::io::stdin().lock()))
            .context("Cannot read renderer bundle from apps run")?
    } else {
        crate::bundle::read(&resources).context("Start this app with apps run or apps build")?
    };
    let desktop = bootstrap(bundle, setup)?;
    let mcp = if dev {
        std::env::var("APPS_RUST_MCP_PORT")
            .ok()
            .map(|port| port.parse())
            .transpose()?
    } else {
        None
    };
    let code = desktop.run(dev && std::env::var_os("APPS_RUST_DEVTOOLS").is_some(), mcp)?;
    if code != 0 {
        std::process::exit(code)
    }
    Ok(())
}

fn bootstrap(bundle: Bundle, setup: impl FnOnce(&App) -> Result<()>) -> Result<Desktop> {
    let pages = bundle
        .pages
        .keys()
        .chain(bundle.files.keys().filter(|name| name.ends_with(".html")))
        .cloned()
        .collect();
    let mut desktop = Desktop::create(
        &bundle.files,
        bundle.scripts,
        bundle.packaged,
        bundle.root,
        true,
    )?;
    desktop.pages = bundle.pages;
    setup(&App(Rc::new(State {
        host: Rc::downgrade(&desktop.host),
        pages,
        next_id: Cell::new(1),
    })))?;
    Ok(desktop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preload_invokes_rust_on_the_app_thread() -> Result<()> {
        let page = r#"<h1></h1><script type="preload">
            const {ipcRenderer,contextBridge} = __apps;
            contextBridge.exposeInMainWorld('double', n => ipcRenderer.invoke('double', n));
            ipcRenderer.invoke('missing').catch(e => globalThis.missing = e.message);
        </script><script type="module">
            double(21).then(n => document.querySelector('h1').textContent = String(n));
        </script>"#;
        let mut bundle = Bundle::default();
        bundle.files.insert("index.html".into(), page.into());
        let thread = std::thread::current().id();
        let mut desktop = bootstrap(bundle, |app| {
            app.handle("double", move |id, args| {
                assert_eq!(id, 1);
                assert_eq!(thread, std::thread::current().id());
                Ok((args[0].as_i64().unwrap() * 2).into())
            })?;
            assert!(app.handle("double", |_, _| Ok(Value::Null)).is_err());
            let window = BrowserWindow::new(app, WindowOptions::default())?;
            assert!(window.load_file("missing.html").is_err());
            window.load_file("index.html")
        })?;
        assert!(desktop.main.is_none());
        desktop.pump(None, None)?;
        let view = &desktop.views[&1];
        let page = view.renderer.as_ref().unwrap();
        let preload = view.preload.as_ref().unwrap();
        preload.tick()?;
        page.tick()?;
        assert_eq!(
            page.eval("document.querySelector('h1').textContent", "test")?,
            "42"
        );
        assert_eq!(
            page.eval("typeof process + ':' + typeof __appsNative", "test")?,
            "undefined:undefined"
        );
        assert!(preload.eval("missing", "test")?.contains("No Rust handler"));
        Ok(())
    }
}
