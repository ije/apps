use super::{
    Desktop,
    link::{Link, Main, ToPage},
};
use crate::bundle::Bundle;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    collections::BTreeSet,
    rc::{Rc, Weak},
    sync::Arc,
};

static DEV_LIBRARIES: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

pub(crate) fn library_exe() -> std::io::Result<std::path::PathBuf> {
    match DEV_LIBRARIES.get() {
        Some(exe) => Ok(exe.clone()),
        None => std::env::current_exe()?.canonicalize(),
    }
}

pub(crate) type Handler = Arc<dyn Fn(u64, Vec<Value>) -> Result<Value> + Send + Sync>;

#[derive(Clone)]
pub struct App(Rc<State>);

struct State {
    main: Weak<Main>,
    link: Link,
    pages: BTreeSet<String>,
    next_id: Cell<u64>,
}

impl App {
    /// Registers a handler for `ipcRenderer.invoke()`. Handlers run on a small thread pool, so a
    /// handler that never returns should start its own thread and report with [`AppHandle`].
    pub fn handle(
        &self,
        channel: &str,
        handler: impl Fn(u64, Vec<Value>) -> Result<Value> + Send + Sync + 'static,
    ) -> Result<()> {
        let main = self.0.main.upgrade().context("App has exited")?;
        let mut handlers = main.handlers.borrow_mut();
        ensure!(
            !handlers.contains_key(channel),
            "Duplicate Rust handler {channel:?}"
        );
        handlers.insert(channel.to_owned(), Arc::new(handler));
        Ok(())
    }

    /// Returns a handle that other threads can use.
    pub fn app_handle(&self) -> AppHandle {
        AppHandle(self.0.link.clone())
    }

    pub fn quit(&self) {
        self.app_handle().quit();
    }
}

/// Reaches the app from any thread.
#[derive(Clone)]
pub struct AppHandle(Link);

impl AppHandle {
    /// Emits `channel` to `ipcRenderer.on()` listeners in the preload of `window`. The event is
    /// dropped when the window is closed or has no preload.
    pub fn send(&self, window: u64, channel: &str, value: Value) {
        self.0.page(ToPage::Bridge(
            window,
            json!({"kind":"event","sender":"main","channel":channel,"args":{"type":"value","value":[value]}})
                .into(),
        ));
    }

    pub fn quit(&self) {
        self.0.quit(0);
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
        let id = app.0.next_id.get();
        app.0.next_id.set(id + 1);
        app.0
            .link
            .create_window(json!({"op":"window","args":["create",id,options]}));
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
        self.app.0.link.page(ToPage::Command(
            json!({"op":"window","args":["loadFile", self.id, format!("apps:///{path}")]}),
        ));
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
    let dev =
        !resources.join("app.bundle").is_file() && std::env::var_os("APPS_RUST_DEV").is_some();
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
    let main = desktop.main.as_ref().context("Missing the main side")?;
    setup(&App(Rc::new(State {
        main: Rc::downgrade(main),
        link: desktop.link.clone(),
        pages,
        next_id: Cell::new(1),
    })))?;
    Ok(desktop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preload_invokes_rust_on_the_pool_and_threads_send_events() -> Result<()> {
        let page = r#"<h1></h1><script type="preload">
            const {ipcRenderer,contextBridge} = __apps;
            contextBridge.exposeInMainWorld('double', n => ipcRenderer.invoke('double', n));
            ipcRenderer.invoke('missing').catch(e => globalThis.missing = e.message);
            ipcRenderer.invoke('fails').catch(e => globalThis.failed = e.message);
            ipcRenderer.on('progress', (_event, value) => globalThis.progress = value);
        </script><script type="module">
            double(21).then(n => document.querySelector('h1').textContent = String(n));
        </script>"#;
        let mut bundle = Bundle::default();
        bundle.files.insert("index.html".into(), page.into());
        let thread = std::thread::current().id();
        let mut handle = None;
        let mut desktop = bootstrap(bundle, |app| {
            app.handle("double", move |id, args| {
                ensure!(id == 1 && thread != std::thread::current().id());
                Ok((args[0].as_i64().unwrap() * 2).into())
            })?;
            app.handle("fails", |_, _| anyhow::bail!("It failed"))?;
            assert!(app.handle("double", |_, _| Ok(Value::Null)).is_err());
            handle = Some(app.app_handle());
            let window = BrowserWindow::new(app, WindowOptions::default())?;
            assert!(window.load_file("missing.html").is_err());
            window.load_file("index.html")
        })?;
        assert!(desktop.main.as_ref().unwrap().engine.is_none());
        desktop.pump(None, None)?;
        let handle = handle.unwrap();
        std::thread::spawn(move || {
            handle.send(1, "progress", json!({"done":7}));
            handle.send(9, "progress", Value::Null);
        })
        .join()
        .unwrap();
        let view = &desktop.views[&1];
        let page = view.renderer.clone().unwrap();
        let preload = view.preload.clone().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while page.eval("document.querySelector('h1').textContent", "test")? != "42"
            || preload.eval("typeof failed + typeof missing", "test")? != "stringstring"
            || preload.eval("typeof progress", "test")? != "object"
        {
            ensure!(std::time::Instant::now() < deadline, "No reply from Rust");
            std::thread::sleep(std::time::Duration::from_millis(1));
            desktop.pump(None, None)?;
            preload.tick()?;
            page.tick()?;
        }
        assert_eq!(
            page.eval("typeof process + ':' + typeof __appsNative", "test")?,
            "undefined:undefined"
        );
        assert!(preload.eval("missing", "test")?.contains("No Rust handler"));
        assert!(preload.eval("failed", "test")?.contains("It failed"));
        assert_eq!(preload.eval("progress.done", "test")?, "7");
        Ok(())
    }
}
