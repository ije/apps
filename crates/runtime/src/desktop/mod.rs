pub mod browser;
pub mod devtools;
pub(crate) mod mcp;
pub(crate) mod native;

use crate::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{Api, Engine},
    render::WebView,
};
use anyhow::{Context as _, Result, bail, ensure};
use bbtree::unsync::BBTreeMap;
use renderer::{App, Bounds, WindowChange, WindowId, WindowOptions, point, px, size};
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

pub(crate) struct View {
    pub(crate) handle: Option<WindowId>,
    pub(crate) data: Weak<crate::render::ViewData>,
    pub(crate) title: Option<String>,
    pub(crate) preload: Option<Rc<Engine>>,
    pub(crate) renderer: Option<Rc<Engine>>,
}

pub struct Desktop {
    api: Rc<Api>,
    host: Rc<RefCell<Host>>,
    pub(crate) main: Engine,
    pub(crate) views: HashMap<u64, View>,
    devtools: HashMap<u64, devtools::DevTools>,
    files: BBTreeMap<String, String>,
    pages: BBTreeMap<String, Document>,
    error: Option<anyhow::Error>,
    quit: bool,
    wake: Option<(Instant, renderer::Timer)>,
    /// The waker of the running app, which async ops started before it ran use too.
    waker: Arc<OnceLock<renderer::Waker>>,
    scheduled: bool,
}

impl Desktop {
    pub fn from_bundle(bundle: crate::bundle::Bundle) -> Result<Self> {
        let mut desktop =
            Self::create(&bundle.files, bundle.scripts, bundle.packaged, bundle.root)?;
        desktop.pages = bundle.pages;
        Ok(desktop)
    }

    pub(crate) fn dispatch(
        runtime: Rc<RefCell<Self>>,
        id: u64,
        name: &str,
        path: &[usize],
        data: serde_json::Value,
        window: Option<&mut renderer::Window>,
        cx: &mut App,
    ) -> bool {
        let renderer = runtime
            .borrow()
            .views
            .get(&id)
            .and_then(|v| v.renderer.clone());
        let Some(renderer) = renderer else {
            return false;
        };
        let result = crate::render::scripted(cx, window, || {
            renderer.call(
                "__appsDispatch",
                &[
                    &serde_json::json!(name).to_string(),
                    &serde_json::json!(path).to_string(),
                    &data.to_string(),
                ],
            )
        });
        let allowed = result.as_ref().is_ok_and(|value| value == "true");
        if let Err(error) = result {
            eprintln!("apps: {error}");
        }
        let schedule = runtime
            .try_borrow_mut()
            .is_ok_and(|mut app| !std::mem::replace(&mut app.scheduled, true));
        if schedule {
            cx.defer(move |cx| {
                if let Ok(mut app) = runtime.try_borrow_mut() {
                    app.scheduled = false;
                }
                Self::drive(runtime, cx)
            });
        }
        allowed
    }

    pub fn new(files: &BBTreeMap<String, String>) -> Result<Self> {
        Self::create(files, HashMap::new(), false, std::path::PathBuf::new())
    }

    fn create(
        files: &BBTreeMap<String, String>,
        scripts: HashMap<String, bytes::Bytes>,
        packaged: bool,
        root: std::path::PathBuf,
    ) -> Result<Self> {
        let api = Api::load()?;
        let waker = Arc::new(OnceLock::<renderer::Waker>::new());
        crate::host::Waker::install({
            let waker = waker.clone();
            move || {
                if let Some(waker) = waker.get() {
                    waker.wake();
                }
            }
        });
        apps_webview::on_event(|| crate::host::Waker::current().wake());
        let package = files.get("package.json").map(String::as_str);
        let (_, name, version) = crate::bundle::package_metadata(package)?;
        let host = Rc::new(RefCell::new(Host {
            app_version: Some(version),
            disabled: crate::bundle::disabled_features(package)?,
            app_name: Some(name.clone()),
            app_path: if root.as_os_str().is_empty() {
                std::env::current_dir().unwrap_or_default()
            } else {
                root
            },
            packaged,
            scripts: Arc::new(scripts),
            assets: files
                .get("assets.json")
                .map(|source| serde_json::from_str(source))
                .transpose()
                .context("Invalid assets.json")?
                .unwrap_or_default(),
            ..Host::default()
        }));
        host.borrow_mut().browsers.application = name;
        let main = Engine::new(api.clone(), host.clone(), Role::Main)?;
        main.bundle(files, "app.js")?;
        Ok(Self {
            pages: BBTreeMap::new(),
            api,
            host,
            main,
            views: HashMap::new(),
            devtools: HashMap::new(),
            files: files
                .iter()
                .filter(|(name, _)| name.ends_with(".html"))
                .map(|(name, source)| (name.clone(), source.clone()))
                .collect(),
            error: None,
            quit: false,
            wake: None,
            waker,
            scheduled: false,
        })
    }

    fn pump(
        &mut self,
        mut cx: Option<&mut App>,
        runtime: Option<&Rc<RefCell<Self>>>,
    ) -> Result<()> {
        for _ in 0..64 {
            let commands = std::mem::take(&mut self.host.borrow_mut().commands);
            if commands.is_empty() {
                // WARNING: titles walk the document, so windowed apps sync them once per drive.
                if runtime.is_none() {
                    self.titles(cx.as_deref_mut())?;
                }
                let attaches = self.host.borrow_mut().browsers.take_attaches();
                for (browser, parent) in attaches {
                    if let Some(cx) = cx.as_deref_mut()
                        && let Some(handle) = self.views.get(&parent).and_then(|view| view.handle)
                        && let Some(result) = cx.update_window(handle, |window, _| {
                            self.host.borrow().browsers.attach(browser, window)
                        })
                    {
                        result?;
                    }
                }
                return Ok(());
            }
            for command in commands {
                let args = &command["args"];
                if command["op"] == "quit" {
                    self.host.borrow_mut().exit_code = args[0].as_i64().unwrap_or(0) as i32;
                    self.quit = true;
                    if let Some(cx) = cx.as_deref_mut() {
                        cx.quit();
                    }
                    return Ok(());
                }
                if command["op"] == "focus" {
                    if let Some(cx) = cx.as_deref_mut() {
                        cx.activate(args[0] == true);
                    }
                    continue;
                }
                let action = args[0].as_str().context("Missing window action")?;
                let id = args[1].as_u64().context("Missing window id")?;
                if action != "create" && !self.views.contains_key(&id) {
                    continue;
                }
                match action {
                    "create" => {
                        ensure!(!self.views.contains_key(&id), "Window already exists");
                        let options = &args[2];
                        let number = |key: &str, default: f64| {
                            options[key].as_f64().unwrap_or(default) as f32
                        };
                        let min = [number("minWidth", 0.), number("minHeight", 0.)];
                        let max = [number("maxWidth", 0.), number("maxHeight", 0.)];
                        let limit = |value: f32, index: usize| {
                            let value = value.max(min[index]);
                            if max[index] > 0. {
                                value.min(max[index])
                            } else {
                                value
                            }
                        };
                        let width = limit(number("width", 800.), 0);
                        let height = limit(number("height", 600.), 1);
                        self.host.borrow_mut().viewports.insert(id, [width, height]);
                        let transparent = options["transparent"] == true;
                        let background = options["backgroundColor"]
                            .as_str()
                            .and_then(crate::style::color)
                            .unwrap_or(if transparent { (0, 0) } else { (0xffffff, 255) });
                        self.host.borrow_mut().backgrounds.insert(id, background);
                        let mut baseline = None;
                        let mut data = Weak::new();
                        let handle = if let Some(cx) = cx.as_deref_mut() {
                            let runtime = runtime.unwrap().clone();
                            let host = self.host.clone();
                            let frame = options["frame"] != false;
                            let style = options["titleBarStyle"].as_str().unwrap_or("default");
                            let hidden = !frame || style != "default";
                            let lights = &options["trafficLightPosition"];
                            let traffic_light_position =
                                match (lights["x"].as_f64(), lights["y"].as_f64()) {
                                    (Some(x), Some(y)) => Some(point(px(x as f32), px(y as f32))),
                                    _ => (style == "hiddenInset").then(|| point(px(20.), px(18.))),
                                }
                                .filter(|_| frame && hidden);
                            let outer = size(px(width), px(height));
                            let origin = match (options["x"].as_f64(), options["y"].as_f64()) {
                                (Some(x), Some(y)) => point(px(x as f32), px(y as f32)),
                                _ => cx.centered(outer).origin,
                            };
                            let window_options = WindowOptions {
                                bounds: Bounds::new(origin, outer),
                                outer_size: options["useContentSize"] != true,
                                title: options["title"].as_str().unwrap_or("apps").to_owned(),
                                titlebar_hidden: hidden,
                                traffic_light_position,
                                frameless: !frame,
                                transparent,
                                show: options["show"] != false,
                                resizable: options["resizable"] != false,
                                minimizable: options["minimizable"] != false,
                                movable: options["movable"] != false,
                            };
                            let handle = cx.open_window(window_options, |window, _| {
                                let view = WebView::new(id, runtime, host, window.id());
                                data = Rc::downgrade(&view.0);
                                Box::new(view)
                            })?;
                            cx.update_window(handle, |window, _| {
                                for (apply, change) in [
                                    (
                                        min != [0.; 2] || max != [0.; 2],
                                        WindowChange::SizeLimits(
                                            size(px(min[0]), px(min[1])),
                                            size(px(max[0]), px(max[1])),
                                        ),
                                    ),
                                    (
                                        options["alwaysOnTop"] == true,
                                        WindowChange::AlwaysOnTop(true),
                                    ),
                                    (
                                        options["maximizable"] == false,
                                        WindowChange::Maximizable(false),
                                    ),
                                    (options["closable"] == false, WindowChange::Closable(false)),
                                    (
                                        true,
                                        WindowChange::Background(renderer::rgba(
                                            (background.0 << 8) | background.1 as u32,
                                        )),
                                    ),
                                ] {
                                    if apply {
                                        window.change_window(change);
                                    }
                                }
                                if options["fullscreen"] == true {
                                    window.toggle_fullscreen();
                                }
                                let size = window.viewport_size();
                                self.host
                                    .borrow_mut()
                                    .viewports
                                    .insert(id, [f32::from(size.width), f32::from(size.height)]);
                                baseline = Some(window_state(window));
                            });
                            Some(handle)
                        } else {
                            None
                        };
                        self.views.insert(
                            id,
                            View {
                                handle,
                                data,
                                title: None,
                                preload: None,
                                renderer: None,
                            },
                        );
                        if let Some(state) = baseline
                            && let Err(error) = self
                                .main
                                .eval(&format!("__appsEvent('state',{id},{state})"), "apps:state")
                        {
                            eprintln!("apps: {error}");
                        }
                    }
                    "load" | "loadFile" => {
                        let value = args[2].as_str().context("Expected HTML text or path")?;
                        let page = args[3] == "page";
                        let request = args[3].as_u64();
                        let root = self.host.borrow().app_path.clone();
                        let mut file = None;
                        let location = if action == "load" {
                            Ok("apps://app/".to_owned())
                        } else if value.starts_with("apps:///") {
                            Ok(value.to_owned())
                        } else if value.starts_with("apps:") {
                            Err((-300, "ERR_INVALID_URL"))
                        } else if value.starts_with("file:") {
                            url::Url::parse(value)
                                .ok()
                                .filter(|url| url.host_str().is_none_or(str::is_empty))
                                .and_then(|url| url.to_file_path().ok())
                                .map(|path| {
                                    file = Some(path);
                                    value.to_owned()
                                })
                                .ok_or((-300, "ERR_INVALID_URL"))
                        } else {
                            let path = root.join(value);
                            let url = url::Url::from_file_path(&path)
                                .map(String::from)
                                .map_err(|_| (-300, "ERR_INVALID_URL"));
                            file = Some(path);
                            url
                        };
                        let read = || {
                            let missing = (-6, "ERR_FILE_NOT_FOUND");
                            if action == "load" {
                                return Ok(Document::parse(value));
                            }
                            if let Some(name) = value.strip_prefix("apps:///") {
                                let name = name.split(['?', '#']).next().unwrap();
                                return match (self.pages.get(name), self.files.get(name)) {
                                    (Some(doc), _) => Ok(doc.clone()),
                                    (_, Some(source)) => Ok(Document::parse(source)),
                                    _ => Err(missing),
                                };
                            }
                            let path = file.as_deref().unwrap();
                            match std::fs::metadata(path) {
                                Ok(metadata)
                                    if metadata.is_file() && metadata.len() <= 64 << 20 =>
                                {
                                    std::fs::read_to_string(path)
                                        .map(|source| Document::parse(&source))
                                        .map_err(|_| (-2, "ERR_FAILED"))
                                }
                                Ok(_) => Err((-2, "ERR_FAILED")),
                                Err(_) => Err(missing),
                            }
                        };
                        let loaded = location
                            .clone()
                            .and_then(|location| read().map(|doc| (location, doc)));
                        let (location, doc) = match loaded {
                            Ok(loaded) => loaded,
                            Err((code, description)) => {
                                let url = location.unwrap_or_else(|_| value.to_owned());
                                if page {
                                    eprintln!(
                                        "apps: navigation: {description} ({code}) loading '{url}'"
                                    );
                                }
                                let event = serde_json::json!({"request":request,"url":url,"code":code,"description":description});
                                if let Err(error) = self.main.eval(
                                    &format!("__appsEvent('failed',{id},{event})"),
                                    "apps:failed",
                                ) {
                                    eprintln!("apps: {error}");
                                }
                                continue;
                            }
                        };
                        // WARNING: only pages that ship with the app run `<script type="preload">` with Node; user files and HTML strings never do.
                        let trusted = location.starts_with("apps:///")
                            || !page
                                && file.as_ref().is_some_and(|path| {
                                    std::fs::canonicalize(path).is_ok_and(|path| {
                                        std::fs::canonicalize(&root)
                                            .is_ok_and(|root| path.starts_with(root))
                                    })
                                });
                        self.host.borrow_mut().graphics.remove(&id);
                        self.host.borrow_mut().media.remove(&id);
                        let bundled = self.host.borrow().scripts.clone();
                        let scripts = doc
                            .nodes
                            .iter()
                            .enumerate()
                            .filter(|(_, n)| n.tag == "script")
                            .filter_map(|(id, n)| {
                                let kind = match n
                                    .attrs
                                    .get("type")
                                    .map(|kind| kind.trim().to_ascii_lowercase())
                                    .as_deref()
                                {
                                    None
                                    | Some(
                                        ""
                                        | "module"
                                        | "text/javascript"
                                        | "application/javascript",
                                    ) => "module",
                                    Some("preload") => "preload",
                                    _ => return None,
                                };
                                let Some(name) = n
                                    .attrs
                                    .get("src")
                                    .and_then(|src| src.strip_prefix("apps:///"))
                                else {
                                    return Some((kind, Cow::Owned(doc.text(id))));
                                };
                                let source = bundled
                                    .get(name)
                                    .and_then(|source| std::str::from_utf8(source).ok())
                                    .map(|source| (kind, Cow::Borrowed(source)));
                                if source.is_none() {
                                    eprintln!("apps: missing bundled script {name}");
                                }
                                source
                            })
                            .collect::<Vec<_>>();
                        self.host
                            .borrow_mut()
                            .pages
                            .insert(id, Page { doc, dirty: true });
                        self.host.borrow_mut().dom.remove(&id);
                        let view = self.views.get_mut(&id).context("Window is closed")?;
                        if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                            cx.update_window(handle, |window, _| {
                                let mut host = self.host.borrow_mut();
                                let state = host.dom.entry(id).or_default();
                                state.text_system = Some(window.text_system().clone());
                                state.pixel_ratio = window.scale_factor();
                            });
                        }
                        view.preload = None;
                        view.renderer = None;
                        self.host.borrow_mut().bridge.remove(&id);
                        let preload = (trusted
                            && scripts.iter().any(|(kind, _)| *kind == "preload"))
                        .then(|| {
                            Engine::new(self.api.clone(), self.host.clone(), Role::Preload(id))
                        })
                        .transpose()?
                        .map(Rc::new);
                        let renderer = Rc::new(Engine::new(
                            self.api.clone(),
                            self.host.clone(),
                            Role::Renderer(id),
                        )?);
                        for (kind, script) in &scripts {
                            if *kind == "preload"
                                && let Some(preload) = &preload
                                && let Err(error) = preload.eval(script, "apps:preload")
                            {
                                if cx.is_none() {
                                    return Err(error);
                                }
                                eprintln!("apps: preload script: {error:#}");
                            }
                        }
                        renderer.eval(
                            &format!("__appsSetLocation({})", serde_json::json!(location)),
                            "apps:location",
                        )?;
                        self.host.borrow_mut().dom.entry(id).or_default().base =
                            url::Url::parse(&location).ok();
                        renderer.eval("__appsSyncBridge()", "apps:bridge")?;
                        if let Err(error) = renderer.eval("__appsImages(document)", "apps:images") {
                            eprintln!("apps: {error}");
                        }
                        let page = |source: &str, url: &str| {
                            if source == "__appsLoaded()" {
                                renderer.eval(source, url)
                            } else {
                                renderer.eval_page(source, url)
                            }
                        };
                        for (source, url) in scripts
                            .iter()
                            .filter(|(kind, _)| *kind == "module")
                            .map(|(_, script)| (&**script, "apps:page"))
                            .chain([("__appsLoaded()", "apps:loaded")])
                        {
                            if let Err(error) = match cx.as_deref_mut() {
                                Some(cx) => crate::render::scripted(cx, None, || page(source, url)),
                                None => page(source, url),
                            } {
                                if cx.is_none() {
                                    return Err(error);
                                }
                                eprintln!("apps: page script: {error:#}");
                            }
                        }
                        if let Some(preload) = &preload {
                            let ready = || {
                                preload.eval(
                                    "dispatchEvent(new Event('DOMContentLoaded'))",
                                    "apps:ready",
                                )
                            };
                            if let Err(error) = match cx.as_deref_mut() {
                                Some(cx) => crate::render::scripted(cx, None, ready),
                                None => ready(),
                            } {
                                eprintln!("apps: preload script: {error:#}");
                            }
                        }
                        view.preload = preload;
                        view.renderer = Some(renderer);
                        // WARNING: `titles` sets the new page's title after `page-title-updated`.
                        view.title = None;
                        if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                            cx.refresh_window(handle);
                        }
                        let event = serde_json::json!({"request":request,"url":location});
                        if let Err(error) = self.main.eval(
                            &format!("__appsEvent('loaded',{id},{event})"),
                            "apps:loaded",
                        ) {
                            eprintln!("apps: {error}");
                        }
                    }
                    "devtools" => {
                        if self.host.borrow().packaged {
                            continue;
                        }
                        ensure!(cx.is_some(), "Developer Tools require a native window");
                        if self.devtools.remove(&id).is_none() {
                            self.devtools
                                .insert(id, devtools::DevTools::new(&format!("Window {id}"))?);
                        }
                    }
                    "close" => {
                        self.devtools.remove(&id);
                        if let Some(view) = self.views.remove(&id) {
                            self.host.borrow_mut().browsers.close_window(id)?;
                            if let Some(data) = view.data.upgrade() {
                                data.browsers.borrow_mut().clear();
                            }
                            if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                                cx.update_window(handle, |window, _| window.remove_window());
                            }
                            self.host.borrow_mut().pages.remove(&id);
                            self.host.borrow_mut().graphics.remove(&id);
                            self.host.borrow_mut().media.remove(&id);
                            self.host.borrow_mut().viewports.remove(&id);
                            self.host.borrow_mut().backgrounds.remove(&id);
                            self.host.borrow_mut().dom.remove(&id);
                            self.host.borrow_mut().bridge.remove(&id);
                            crate::web::storage::forget(id);
                            if let Err(error) = self
                                .main
                                .eval(&format!("__appsEvent('closed',{id})"), "apps:closed")
                            {
                                eprintln!("apps: {error}");
                            }
                        }
                    }
                    "capture" => {
                        let request = args[2].as_u64().context("Missing capture request")?;
                        let rect = args[3].clone();
                        let view = self.views.get(&id).context("Window is closed")?;
                        if let (Some(handle), Some(cx), Some(runtime)) =
                            (view.handle, cx.as_deref_mut(), runtime)
                        {
                            let runtime = runtime.clone();
                            cx.update_window(handle, |window, _| {
                                let scale = window.scale_factor();
                                window.capture(move |frame, cx| {
                                    let data = match frame.and_then(|frame| png(frame, scale, &rect, false)) {
                                        Ok(([width, height], bytes)) => serde_json::json!({"id":request,"width":width,"height":height,"png":base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)}),
                                        Err(error) => serde_json::json!({"id":request,"error":error.to_string()}),
                                    };
                                    // WARNING: the window is busy drawing here, so a capture requested by the reply must wait for the next drive.
                                    cx.defer(move |cx| {
                                        if let Ok(app) = runtime.try_borrow()
                                            && let Err(error) = app.main.eval(&format!("__appsEvent('captured',{id},{data})"), "apps:capture")
                                        {
                                            eprintln!("apps: {error}");
                                        }
                                        Desktop::drive(runtime, cx);
                                    });
                                });
                            });
                        }
                    }
                    "execute" | "executed" => {
                        let request = &args[2];
                        let result = match (action, self.views[&id].renderer.as_ref()) {
                            ("executed", _) => Some(args[3].clone()),
                            (_, None) => Some(serde_json::json!({"error":"The window has no page"}).to_string().into()),
                            (_, Some(renderer)) => {
                                let source = format!("(s=>{{try{{__appsExecuted({request},s.__appsCompile({},'apps:execute'),false)}}catch(e){{__appsExecuted({request},e,true)}}}})(this)", args[3]);
                                let run = || renderer.eval_page(&source, "apps:execute");
                                match cx.as_deref_mut() {
                                    Some(cx) => crate::render::scripted(cx, None, run),
                                    None => run(),
                                }
                                .err()
                            }
                                .map(|error| serde_json::json!({"error":error.to_string()}).to_string().into()),
                        };
                        if let Some(data) = result
                            && let Err(error) = self.main.eval(
                                &format!(
                                    "__appsEvent('executed',{id},{})",
                                    serde_json::json!({"request":request,"data":data})
                                ),
                                "apps:executed",
                            )
                        {
                            eprintln!("apps: {error}");
                        }
                    }
                    "closing" => {
                        if let Err(error) = self
                            .main
                            .eval(&format!("__appsEvent('close',{id})"), "apps:close")
                        {
                            eprintln!("apps: {error}");
                        }
                    }
                    "background" => {
                        let color = args[2]
                            .as_str()
                            .and_then(crate::style::color)
                            .unwrap_or((0xffffff, 255));
                        let mut host = self.host.borrow_mut();
                        host.backgrounds.insert(id, color);
                        if let Some(page) = host.pages.get_mut(&id) {
                            page.dirty = true;
                        }
                        drop(host);
                        if let (Some(handle), Some(cx)) =
                            (self.views[&id].handle, cx.as_deref_mut())
                        {
                            cx.update_window(handle, |window, _| {
                                window.change_window(WindowChange::Background(renderer::rgba(
                                    (color.0 << 8) | color.1 as u32,
                                )));
                                window.refresh();
                            });
                        }
                    }
                    "show" | "title" | "bounds" | "center" | "minimize" | "maximize"
                    | "unmaximize" | "restore" | "fullscreen" | "focus" | "blur" | "hide"
                    | "showInactive" | "limits" | "resizable" | "minimizable" | "maximizable"
                    | "closable" | "movable" | "alwaysOnTop" | "buttons" => {
                        if let (Some(handle), Some(cx)) =
                            (self.views[&id].handle, cx.as_deref_mut())
                        {
                            let number =
                                |index: usize| px(args[index].as_f64().unwrap_or(0.) as f32);
                            let on = args[2] == true;
                            cx.update_window(handle, |window, _| {
                                let change = match action {
                                    "title" => {
                                        window.set_window_title(args[2].as_str().unwrap_or(""));
                                        return;
                                    }
                                    "bounds" if cfg!(target_os = "macos") => {
                                        WindowChange::Frame(Bounds::new(
                                            renderer::point(number(2), number(3)),
                                            size(number(4), number(5)),
                                        ))
                                    }
                                    "bounds" => {
                                        let state = window.window_state();
                                        return window.resize(size(
                                            number(4)
                                                - (state.frame.size.width
                                                    - state.content.size.width),
                                            number(5)
                                                - (state.frame.size.height
                                                    - state.content.size.height),
                                        ));
                                    }
                                    "buttons" => {
                                        #[cfg(target_os = "macos")]
                                        window.set_window_button_visibility(on);
                                        return;
                                    }
                                    "minimize" => return window.minimize_window(),
                                    "maximize" | "unmaximize" => {
                                        if window.window_state().maximized != (action == "maximize")
                                        {
                                            window.zoom_window();
                                        }
                                        return;
                                    }
                                    "fullscreen" => {
                                        if window.is_fullscreen() != on {
                                            window.toggle_fullscreen();
                                        }
                                        return;
                                    }
                                    "restore" if cfg!(target_os = "macos") => WindowChange::Restore,
                                    "show" | "focus" | "restore" => {
                                        return window.activate_window();
                                    }
                                    "center" => WindowChange::Center,
                                    "blur" => WindowChange::Blur,
                                    "hide" => WindowChange::Hide,
                                    "showInactive" => WindowChange::Show(false),
                                    "limits" => WindowChange::SizeLimits(
                                        size(number(2), number(3)),
                                        size(number(4), number(5)),
                                    ),
                                    "resizable" => WindowChange::Resizable(on),
                                    "minimizable" => WindowChange::Minimizable(on),
                                    "maximizable" => WindowChange::Maximizable(on),
                                    "closable" => WindowChange::Closable(on),
                                    "movable" => WindowChange::Movable(on),
                                    _ => WindowChange::AlwaysOnTop(on),
                                };
                                window.change_window(change);
                            });
                        }
                    }
                    _ => bail!("Unsupported window action: {action}"),
                }
            }
        }
        // WARNING: a page redirect loop must not quit the app, so later commands wait for the next turn.
        Ok(())
    }

    fn titles(&mut self, mut cx: Option<&mut App>) -> Result<()> {
        for (&id, view) in &mut self.views {
            let Some(title) = self
                .host
                .borrow()
                .pages
                .get(&id)
                .filter(|page| page.dirty)
                .map(|page| page.doc.title())
            else {
                continue;
            };
            if view.title == title {
                continue;
            }
            let text = title.as_deref().unwrap_or("");
            view.title = title.clone();
            let allowed = self
                .main
                .eval(
                    &format!("__appsEvent('title',{id},{})", serde_json::json!(text)),
                    "apps:title",
                )
                .inspect_err(|error| eprintln!("apps: {error}"))
                .is_ok_and(|allowed| allowed != "false");
            if allowed && let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                cx.update_window(handle, |window, _| window.set_window_title(text));
            }
        }
        Ok(())
    }

    pub(crate) fn drive(runtime: Rc<RefCell<Self>>, cx: &mut App) {
        if runtime.try_borrow_mut().is_err() {
            return;
        }
        let result = (|| -> Result<Option<u64>> {
            let mut app = runtime.borrow_mut();
            app.host.borrow_mut().sync_theme();
            app.pump(Some(cx), Some(&runtime))?;
            if app.quit {
                return Ok(None);
            }
            let resized = std::mem::take(&mut app.host.borrow_mut().resized);
            for id in resized {
                let Some([width, height]) = app.host.borrow().viewports.get(&id).copied() else {
                    continue;
                };
                let event = format!(
                    "__appsEvent('resize',{id},{{width:{},height:{}}})",
                    width.round(),
                    height.round()
                );
                if let Err(error) = app.main.eval(&event, "apps:resize") {
                    eprintln!("apps: {error}");
                }
                if let Some(renderer) = app.views.get(&id).and_then(|v| v.renderer.as_ref())
                    && let Err(error) = crate::render::scripted(cx, None, || {
                        renderer.eval("__appsDispatch('window:resize',[0])", "apps:resize")
                    })
                {
                    eprintln!("apps: {error}");
                }
            }
            let changed = std::mem::take(&mut app.host.borrow_mut().window_changes);
            for id in changed {
                if let Some(handle) = app.views.get(&id).and_then(|view| view.handle)
                    && let Some(state) = cx.update_window(handle, |window, _| window_state(window))
                    && let Err(error) = app
                        .main
                        .eval(&format!("__appsEvent('state',{id},{state})"), "apps:state")
                {
                    eprintln!("apps: {error}");
                }
            }
            let mut delay = i64::MAX;
            for (fatal, engine) in
                std::iter::once((true, &app.main)).chain(app.views.values().flat_map(|v| {
                    v.preload
                        .iter()
                        .chain(v.renderer.iter())
                        .map(|engine| (false, engine.as_ref()))
                }))
            {
                let next = match crate::render::scripted(cx, None, || engine.tick()) {
                    Ok(next) => next,
                    Err(error) if !fatal => {
                        eprintln!("apps: page script: {error:#}");
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if next >= 0 {
                    delay = delay.min(next);
                }
            }
            // WARNING: IPC listeners run in a later task than their sender, so queued messages are delivered here in rounds, for at most 4 ms; the rest waits for the next wake.
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(4) {
                let mut busy = false;
                for engine in std::iter::once(&app.main).chain(
                    app.views
                        .values()
                        .filter_map(|view| view.preload.as_deref()),
                ) {
                    busy |=
                        crate::render::scripted(cx, None, || engine.call("__appsBridgeDrain", &[]))
                            .is_ok_and(|drained| drained == "true");
                }
                if !busy {
                    break;
                }
            }
            app.pump(Some(cx), Some(&runtime))?;
            if app.quit {
                return Ok(None);
            }
            if std::mem::take(&mut app.host.borrow_mut().theme_changed) {
                delay = delay.min(0);
                for view in app.views.values() {
                    if let Some(renderer) = &view.renderer
                        && let Err(error) = crate::render::scripted(cx, None, || {
                            renderer.eval("__appsMediaChanged()", "apps:theme")
                        })
                    {
                        eprintln!("apps: page script: {error:#}");
                    }
                }
            }
            for (&id, tools) in &app.devtools {
                let engine = app
                    .views
                    .get(&id)
                    .and_then(|view| view.renderer.as_deref())
                    .unwrap_or(&app.main);
                for command in tools.commands() {
                    let request = command["id"].as_u64().unwrap_or(0);
                    match devtools::dispatch(&command, engine, &app.host, id) {
                        Ok(value) => tools.respond(request, value, None)?,
                        Err(error) => tools.respond(
                            request,
                            serde_json::Value::Null,
                            Some(&error.to_string()),
                        )?,
                    }
                }
            }
            app.devtools.retain(|_, tools| !tools.is_closed());
            let mcp = mcp::enabled();
            if !app.devtools.is_empty() || mcp {
                let logs: Vec<_> = app.host.borrow_mut().logs.drain(..).collect();
                for tools in app.devtools.values() {
                    tools.events(&logs)?;
                }
                mcp::logs(&logs);
                if !app.devtools.is_empty() {
                    delay = delay.min(16);
                }
            }
            if mcp && mcp::poll(&app, cx) {
                delay = delay.min(0);
            }
            app.pump(Some(cx), Some(&runtime))?;
            if app.quit {
                return Ok(None);
            }
            for view in app.views.values() {
                if let Some(data) = view.data.upgrade() {
                    let browsers = data.browsers.borrow();
                    let active = !browsers.is_empty();
                    let events = browsers
                        .iter()
                        .flat_map(|(&node, (browser, _))| {
                            browser.poll().into_iter().map(move |event| (node, event))
                        })
                        .collect::<Vec<_>>();
                    drop(browsers);
                    if active && cfg!(target_os = "linux") {
                        delay = delay.min(16);
                    }
                    for (node, event) in events {
                        if let Some(renderer) = &view.renderer
                            && let Err(error) = crate::render::scripted(cx, None, || {
                                renderer.eval(
                                    &format!("__appsFrameEvent({node},{event})"),
                                    "apps:iframe",
                                )
                            })
                        {
                            eprintln!("apps: page script: {error:#}");
                        }
                    }
                }
            }
            let host = app.host.borrow();
            if !host.commands.is_empty()
                || !host.resized.is_empty()
                || !host.window_changes.is_empty()
            {
                delay = delay.min(0);
            }
            drop(host);
            app.titles(Some(cx))?;
            // WARNING: IPC, microtasks and the scripts above can add timers after an engine's tick, so read all delays again.
            for engine in std::iter::once(&app.main).chain(
                app.views
                    .values()
                    .flat_map(|v| v.preload.iter().chain(v.renderer.iter()).map(Rc::as_ref)),
            ) {
                if let Ok(next) = engine
                    .call("__appsDelay", &[])
                    .map(|next| next.parse::<f64>().unwrap_or(-1.))
                    && next >= 0.
                {
                    delay = delay.min(next.ceil() as i64);
                }
            }
            for (&id, view) in &app.views {
                let (dirty, frame) = {
                    let host = app.host.borrow();
                    (
                        host.pages.get(&id).is_some_and(|p| p.dirty)
                            || host.graphics.get(&id).is_some_and(|gpu| gpu.presented()),
                        host.dom.get(&id).is_some_and(|dom| dom.frame_requested),
                    )
                };
                // WARNING: animation frame callbacks run before the frame lays out, like in browsers.
                if frame
                    && let (Some(handle), Some(data)) = (view.handle, view.data.upgrade())
                    && cx
                        .update_window(handle, |window, _| {
                            window.on_next_frame(move |window, cx| {
                                crate::render::animation_frame(data, window, cx)
                            })
                        })
                        .is_some()
                    && let Some(dom) = app.host.borrow_mut().dom.get_mut(&id)
                {
                    dom.frame_requested = false;
                }
                if dirty && let Some(handle) = view.handle {
                    cx.refresh_window(handle);
                }
            }
            Ok(if delay == i64::MAX || app.quit {
                None
            } else {
                Some(delay.clamp(1, 1 << 31) as u64)
            })
        })();
        // WARNING: JSC's GC timers never fire on the generic event loop, so after real JS work the VM
        // is collected once, when JS has been quiet for a second.
        let api = runtime.borrow().api.clone();
        if api.shrink_pending() && !api.idle.replace(true) {
            shrink_when_quiet(api, cx);
        }
        match result {
            Ok(Some(delay)) => {
                let wait = Duration::from_millis(delay);
                let at = Instant::now() + wait;
                let mut app = runtime.borrow_mut();
                // WARNING: a replaced timer stays registered until it fires, so keep the armed one unless the new deadline is earlier.
                if app.wake.as_ref().is_none_or(|(armed, _)| {
                    *armed <= Instant::now() || at + Duration::from_micros(500) < *armed
                }) {
                    let wake = runtime.clone();
                    // WARNING: native dialogs and popup menus run a nested loop while the app is busy, so the timer runs after it.
                    let task = cx.timer(wait, move |cx| {
                        if let Ok(mut app) = wake.try_borrow_mut() {
                            app.wake = None;
                        }
                        Self::drive(wake, cx)
                    });
                    app.wake = Some((at, task));
                }
            }
            Ok(None) => runtime.borrow_mut().wake = None,
            Err(error) => {
                eprintln!("apps: {error:#}");
                let mut app = runtime.borrow_mut();
                app.fail();
                app.error = Some(error);
                cx.quit();
            }
        }
    }

    /// Quits with a failure code, because AppKit exits with 0 after its quit hook.
    fn fail(&mut self) {
        self.quit = true;
        let mut host = self.host.borrow_mut();
        if host.exit_code == 0 {
            host.exit_code = 1;
        }
    }

    pub fn check(mut self) -> Result<i32> {
        self.main.eval("__appsEvent('ready')", "apps:ready")?;
        self.pump(None, None)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            for engine in std::iter::once(&self.main).chain(self.views.values().flat_map(|view| {
                view.preload
                    .iter()
                    .chain(view.renderer.iter())
                    .map(Rc::as_ref)
            })) {
                engine.tick()?;
            }
            self.pump(None, None)?;
            let mut pending = false;
            for engine in std::iter::once(&self.main).chain(self.views.values().flat_map(|view| {
                view.preload
                    .iter()
                    .chain(view.renderer.iter())
                    .map(Rc::as_ref)
            })) {
                pending |= engine.eval("__appsPending()", "apps:startup")? == "true";
            }
            if !pending || self.quit {
                break;
            }
            ensure!(
                std::time::Instant::now() < deadline,
                "Async startup did not finish within 30 seconds"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        for view in self.views.values() {
            if let Some(engine) = &view.renderer {
                ensure!(
                    engine.eval(
                        "typeof process + ':' + typeof __appsNative",
                        "apps:isolation"
                    )? == "undefined:undefined",
                    "Renderer isolation failed"
                );
            }
        }
        println!("apps: checked {} window(s)", self.views.len());
        Ok(self.host.borrow().exit_code)
    }

    pub fn run(self, devtools: bool, mcp: Option<u16>) -> Result<i32> {
        ensure!(
            !(devtools || mcp.is_some()) || !self.host.borrow().packaged,
            "Developer tools are unavailable in packaged apps"
        );
        if let Some(port) = mcp {
            mcp::start(port)?;
        }
        let runtime = Rc::new(RefCell::new(self));
        let result = runtime.clone();
        renderer::run(move |cx| {
            let reopen = runtime.clone();
            cx.on_reopen(move |cx, visible| {
                if let Err(error) = reopen.borrow().main.eval(
                    &format!("__appsEvent('activate',0,{visible})"),
                    "apps:activate",
                ) {
                    eprintln!("apps: {error}");
                }
                Self::drive(reopen.clone(), cx);
            });
            let terminate = runtime.clone();
            cx.on_should_quit(move |cx| {
                let Ok(app) = terminate.try_borrow() else {
                    return true;
                };
                if app.quit {
                    return true;
                }
                if let Err(error) = app.main.eval("__appsEvent('before-quit')", "apps:quit") {
                    eprintln!("apps: {error}");
                    return true;
                }
                drop(app);
                Self::drive(terminate.clone(), cx);
                terminate.try_borrow().is_ok_and(|app| app.quit)
            });
            let opened = Rc::downgrade(&runtime);
            cx.on_open_urls(move |cx, list| {
                let Some(runtime) = opened.upgrade() else {
                    return;
                };
                for url in list {
                    let event = match url::Url::parse(&url)
                        .ok()
                        .filter(|url| url.scheme() == "file")
                        .and_then(|url| url.to_file_path().ok())
                    {
                        Some(path) => {
                            format!("__appsEvent('open-file',0,{})", serde_json::json!(path))
                        }
                        None => format!("__appsEvent('open-url',0,{})", serde_json::json!(url)),
                    };
                    if let Err(error) = runtime.borrow().main.eval(&event, "apps:open") {
                        eprintln!("apps: {error}");
                    }
                }
                Self::drive(runtime, cx);
            });
            let host = runtime.borrow().host.clone();
            cx.on_quit(move |_| {
                if let Ok(host) = host.try_borrow() {
                    if let Some(command) = &host.relaunch {
                        #[cfg(unix)]
                        let _ = std::os::unix::process::CommandExt::process_group(
                            std::process::Command::new("/bin/sh")
                                .args([
                                    "-c",
                                    r#"while kill -0 "$0" 2>/dev/null; do sleep 0.1; done; exec "$@""#,
                                    &std::process::id().to_string(),
                                ])
                                .args(command),
                            0,
                        )
                        .stdin(std::process::Stdio::null())
                        .spawn();
                        #[cfg(not(unix))]
                        let _ = std::process::Command::new(&command[0]).args(&command[1..]).spawn();
                    }
                    // AppKit exits with 0 after this hook, so a failure code must exit here.
                    if host.exit_code != 0 {
                        std::process::exit(host.exit_code);
                    }
                }
            });
            let woken = Rc::downgrade(&runtime);
            cx.on_wake(move |cx| {
                if let Some(runtime) = woken.upgrade() {
                    Self::drive(runtime, cx);
                }
            });
            let _ = runtime.borrow().waker.set(cx.waker());
            #[cfg(target_os = "macos")]
            native::observe();
            if let Err(error) = runtime
                .borrow()
                .main
                .eval("__appsEvent('ready')", "apps:ready")
            {
                eprintln!("apps: {error}");
                runtime.borrow_mut().fail();
                cx.quit();
                return;
            }
            Self::drive(runtime.clone(), cx);
            // WARNING: the first main menu costs AppKit 20-30 ms, so the default menu waits for the first frame.
            #[cfg(target_os = "macos")]
            {
                let menu = runtime.clone();
                let init = move || {
                    if let Ok(app) = menu.try_borrow()
                        && let Err(error) = app.main.eval("__appsInitMenu()", "apps:menu")
                    {
                        eprintln!("apps: {error}");
                    }
                };
                let handle = runtime.borrow().views.values().find_map(|view| view.handle);
                match handle {
                    Some(handle) => {
                        cx.update_window(handle, |window, _| {
                            window
                                .on_next_frame(|window, _| window.on_next_frame(move |_, _| init()))
                        });
                    }
                    None => init(),
                }
            }
            if devtools && !runtime.borrow().quit {
                let mut app = runtime.borrow_mut();
                let id = app.views.keys().copied().min().unwrap_or(0);
                let title = if id == 0 {
                    "Main".to_owned()
                } else {
                    format!("Window {id}")
                };
                match devtools::DevTools::new(&title) {
                    Ok(tools) => {
                        app.devtools.insert(id, tools);
                    }
                    Err(error) => {
                        app.error = Some(error);
                        app.fail();
                        cx.quit();
                        return;
                    }
                }
                drop(app);
                Self::drive(runtime.clone(), cx);
            }
            cx.activate(true);
        });
        let mut app = result.borrow_mut();
        if let Some(error) = app.error.take() {
            return Err(error);
        }
        Ok(app.host.borrow().exit_code)
    }
}

/// Collects garbage once JS has been quiet for a second.
fn shrink_when_quiet(api: Rc<Api>, cx: &mut App) {
    let busy = api.busy.get();
    cx.timer(Duration::from_secs(1), move |cx| {
        if api.busy.get() == busy {
            api.idle.set(false);
            api.shrink();
        } else {
            shrink_when_quiet(api, cx);
        }
    })
    .detach();
}

/// Checks a window host call, so bad input throws in JS and never reaches `Desktop::pump`.
pub(crate) fn validate(args: &serde_json::Value) -> Result<()> {
    let number = |value: &serde_json::Value, low: f64, high: f64| {
        value
            .as_f64()
            .is_some_and(|value| (low..=high).contains(&value))
    };
    let position = |value: &serde_json::Value| number(value, -1e6, 1e6);
    let size = |value: &serde_json::Value| number(value, 1., 16384.);
    match args[0].as_str().unwrap_or("") {
        "create" => {
            let options = &args[2];
            for (key, value) in options.as_object().into_iter().flatten() {
                let valid = match key.as_str() {
                    "width" | "height" => size(value),
                    "minWidth" | "minHeight" | "maxWidth" | "maxHeight" => {
                        number(value, 0., 16384.)
                    }
                    "x" | "y" => position(value),
                    "useContentSize" | "center" | "resizable" | "movable" | "minimizable"
                    | "maximizable" | "closable" | "alwaysOnTop" | "fullscreen" | "show"
                    | "frame" | "transparent" => value.is_boolean(),
                    "title" => value.is_string(),
                    "titleBarStyle" => {
                        matches!(value.as_str(), Some("default" | "hidden" | "hiddenInset"))
                    }
                    "backgroundColor" => value.as_str().and_then(crate::style::color).is_some(),
                    "trafficLightPosition" => position(&value["x"]) && position(&value["y"]),
                    _ => bail!("Unsupported BrowserWindow option: {key}"),
                };
                ensure!(valid, "Invalid BrowserWindow option {key}: {value}");
            }
        }
        "bounds" => ensure!(
            position(&args[2]) && position(&args[3]) && size(&args[4]) && size(&args[5]),
            "Invalid window bounds"
        ),
        "limits" => ensure!(
            (2..6).all(|index| number(&args[index], 0., 16384.)),
            "Window size limits must be between 0 and 16384"
        ),
        "fullscreen" | "resizable" | "minimizable" | "maximizable" | "closable" | "movable"
        | "alwaysOnTop" => ensure!(args[2].is_boolean(), "Expected a boolean"),
        "background" => ensure!(
            args[2].as_str().and_then(crate::style::color).is_some(),
            "Invalid background color"
        ),
        "load" | "loadFile" => ensure!(
            args[2].is_string() && (args[3].is_null() || args[3].is_u64()),
            "Invalid page load"
        ),
        "execute" => ensure!(
            args[2].is_u64() && args[3].is_string(),
            "Invalid script request"
        ),
        "capture" => ensure!(args[2].is_u64(), "Invalid capture request"),
        "executed" => bail!("Unsupported window action: executed"),
        _ => {}
    }
    Ok(())
}

/// The window state that `BrowserWindow` caches, in screen points.
fn window_state(window: &renderer::Window) -> serde_json::Value {
    let state = window.window_state();
    let rect = |bounds: Bounds<renderer::Pixels>| {
        [
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        ]
        .map(|value| f32::from(value).round())
    };
    serde_json::json!({"frame":rect(state.frame),"content":rect(state.content),"minimized":state.minimized,"maximized":state.maximized,"fullscreen":state.fullscreen,"visible":state.visible,"focused":window.is_window_active(),"resizing":state.resizing})
}

/// Crops a captured frame to `rect` (in window points) and encodes it as PNG, in CSS pixels when `css` is set.
fn png(
    ([width, height], pixels): ([u32; 2], Vec<u8>),
    scale: f32,
    rect: &serde_json::Value,
    css: bool,
) -> Result<([u32; 2], Vec<u8>)> {
    use resvg::tiny_skia as skia;
    let mut image = skia::IntSize::from_wh(width, height)
        .and_then(|size| skia::Pixmap::from_vec(pixels, size))
        .context("Invalid frame")?;
    if rect.is_object() {
        let value = |key: &str| {
            (rect[key].as_f64().unwrap_or(0.) as f32 * scale)
                .round()
                .max(0.) as u32
        };
        let (x, y) = (value("x").min(width), value("y").min(height));
        image = skia::IntRect::from_xywh(
            x as i32,
            y as i32,
            value("width").min(width - x),
            value("height").min(height - y),
        )
        .and_then(|rect| image.clone_rect(rect))
        .context("Empty capture area")?;
    }
    if css && scale > 1. {
        let size = |value: u32| ((value as f32 / scale).round() as u32).max(1);
        let mut scaled = skia::Pixmap::new(size(image.width()), size(image.height()))
            .context("Invalid frame")?;
        scaled.draw_pixmap(
            0,
            0,
            image.as_ref(),
            &skia::PixmapPaint {
                quality: skia::FilterQuality::Bicubic,
                ..Default::default()
            },
            skia::Transform::from_scale(
                scaled.width() as f32 / image.width() as f32,
                scaled.height() as f32 / image.height() as f32,
            ),
            None,
        );
        image = scaled;
    }
    Ok(([image.width(), image.height()], image.encode_png()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(code: &str, extra: &[(&str, &str)], root: std::path::PathBuf) -> Desktop {
        let mut files = BBTreeMap::from([(
            "app.js".into(),
            format!(
                "$appsDefine({});",
                serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":code})
            ),
        )]);
        for (name, source) in extra {
            files.insert((*name).into(), (*source).into());
        }
        let mut desktop = Desktop::create(&files, HashMap::new(), false, root).unwrap();
        desktop.main.eval("__appsEvent('ready')", "test").unwrap();
        desktop.pump(None, None).unwrap();
        desktop
    }

    #[test]
    fn navigation_trust_and_optional_preloads() {
        let dir = tempfile::tempdir().unwrap();
        let (root, user) = (dir.path().join("app"), dir.path().join("user"));
        let page =
            "<script type='preload'>__apps.ipcRenderer.send('pwned')</script><body>safe</body>";
        for directory in [&root, &user] {
            std::fs::create_dir(directory).unwrap();
            std::fs::write(directory.join("page.html"), page).unwrap();
        }
        let code = format!(
            "globalThis.pwned=0;__apps.ipcMain.on('pwned',()=>pwned++);__apps.app.whenReady().then(()=>{{globalThis.win=new __apps.BrowserWindow();win.load({})}})",
            serde_json::json!(page)
        );
        let mut desktop = app(&code, &[("bundled.html", page)], root.clone());
        let step = |desktop: &mut Desktop, script: &str, preload: bool, pwned: &str| {
            if !script.is_empty() {
                desktop.main.eval(script, "test").unwrap();
                desktop.pump(None, None).unwrap();
            }
            desktop.main.tick().unwrap();
            assert_eq!(desktop.views[&1].preload.is_some(), preload, "{script}");
            assert_eq!(
                desktop.main.eval("pwned", "test").unwrap(),
                pwned,
                "{script}"
            );
        };
        step(&mut desktop, "", false, "0");
        desktop
            .main
            .eval("win.webContents.send('unused',42)", "test")
            .unwrap();
        let url = url::Url::from_file_path(root.join("page.html")).unwrap();
        desktop.views[&1]
            .renderer
            .as_ref()
            .unwrap()
            .eval(
                &format!("location.href={}", serde_json::json!(url.as_str())),
                "test",
            )
            .unwrap();
        desktop.pump(None, None).unwrap();
        step(&mut desktop, "", false, "0");
        let load = |path: &std::path::Path| format!("win.loadFile({})", serde_json::json!(path));
        step(&mut desktop, &load(&user.join("page.html")), false, "0");
        step(
            &mut desktop,
            &load(&root.join("../user/page.html")),
            false,
            "0",
        );
        step(&mut desktop, &load(&root.join("page.html")), true, "1");
        step(&mut desktop, "win.loadFile('page.html')", true, "2");
        assert_eq!(
            desktop.views[&1]
                .renderer
                .as_ref()
                .unwrap()
                .eval("location.href", "test")
                .unwrap(),
            url.as_str()
        );
        step(
            &mut desktop,
            "win.loadFile('apps:///bundled.html')",
            true,
            "3",
        );
        step(&mut desktop, "win.reload()", true, "4");
    }

    #[test]
    fn loads_return_promises_and_failures_never_quit() {
        let code = r#"
            globalThis.log = [];
            __apps.app.whenReady().then(() => {
                globalThis.win = new __apps.BrowserWindow();
                for (const name of ['did-start-loading', 'did-navigate', 'dom-ready', 'did-finish-load', 'did-stop-loading', 'did-fail-load', 'destroyed'])
                    win.webContents.on(name, (event, ...args) => log.push(name + (name === 'did-fail-load' || name === 'did-navigate' ? ':' + args.slice(0, 3).join(' ') : '')));
                win.loadFile('missing.html').then(() => log.push('resolved'), error => log.push('rejected:' + [error.message, error.code, error.errno].join(' ')));
            });
        "#;
        let mut desktop = app(code, &[], std::path::PathBuf::from("/app"));
        let eval = |desktop: &Desktop, script: &str| desktop.main.eval(script, "test").unwrap();
        assert!(!desktop.quit && desktop.views.contains_key(&1));
        assert_eq!(
            eval(&desktop, "log.join('|')"),
            "did-fail-load:-6 ERR_FILE_NOT_FOUND file:///app/missing.html|did-stop-loading|rejected:ERR_FILE_NOT_FOUND (-6) loading 'file:///app/missing.html' ERR_FILE_NOT_FOUND -6"
        );
        eval(
            &desktop,
            "log.length = 0; win.load('<p>first</p>').catch(error => log.push(error.code)); win.load('<title>T</title><p>second</p>').then(() => log.push('resolved:' + win.webContents.getURL() + ':' + win.webContents.isLoading()))",
        );
        assert_eq!(
            eval(&desktop, "log.join('|')+':'+win.webContents.isLoading()"),
            "ERR_ABORTED:true"
        );
        desktop.pump(None, None).unwrap();
        assert_eq!(
            eval(&desktop, "log.slice(1).join('|')"),
            "did-start-loading|did-navigate:apps://app/ -1 |dom-ready|did-finish-load|did-stop-loading|did-start-loading|did-navigate:apps://app/ -1 |dom-ready|did-finish-load|did-stop-loading|resolved:apps://app/:false"
        );
        eval(
            &desktop,
            "globalThis.results = []; for (const code of ['document.title', 'new Promise(r => setTimeout(() => r({list: [1, 2]}), 0))', 'throw new TypeError(\"inner\")', 'undefined']) win.webContents.executeJavaScript(code).then(v => results.push(JSON.stringify(v) ?? 'undefined'), e => results.push('error:' + e.message))",
        );
        desktop.pump(None, None).unwrap();
        for engine in [
            &desktop.main,
            desktop.views[&1].renderer.as_deref().unwrap(),
        ] {
            engine.tick().unwrap();
        }
        desktop.pump(None, None).unwrap();
        assert_eq!(
            eval(&desktop, "results.join('|')"),
            r#""T"|error:inner|undefined|{"list":[1,2]}"#
        );
        assert!(
            desktop.views[&1]
                .renderer
                .as_ref()
                .unwrap()
                .eval("eval('1')", "test")
                .is_err(),
            "executeJavaScript must not unlock eval for the page"
        );
        assert!(
            eval(
                &desktop,
                "try { win.webContents.executeJavaScript('1'); win.load(1); 'ok' } catch (e) { e.message }"
            ) == "ok"
        );
        assert!(!desktop.quit);
    }

    #[test]
    fn fatal_errors_quit_with_a_failure_code() {
        let mut desktop = app("", &[], std::path::PathBuf::new());
        desktop.fail();
        assert!(desktop.quit && desktop.host.borrow().exit_code == 1);
        desktop.host.borrow_mut().exit_code = 3;
        desktop.fail();
        assert_eq!(desktop.host.borrow().exit_code, 3);
    }

    #[test]
    fn destroy_is_synchronous_and_web_contents_are_real() {
        let code = r#"
            const {app, BrowserWindow, webContents} = __apps;
            globalThis.log = [];
            app.on('window-all-closed', () => log.push('all-closed'));
            app.whenReady().then(() => {
                globalThis.win = new BrowserWindow();
                globalThis.other = new BrowserWindow();
                for (const name of ['close', 'closed']) win.on(name, () => log.push(name));
                win.webContents.on('destroyed', () => log.push('destroyed'));
                globalThis.ids = [win.id, win.webContents.id, other.webContents.id, webContents.fromId(other.webContents.id) === other.webContents, webContents.getAllWebContents().length, BrowserWindow.fromWebContents(win.webContents) === win].join();
                win.destroy();
                log.push(win.isDestroyed(), win.webContents.isDestroyed(), BrowserWindow.getAllWindows().length);
                for (const call of [() => win.setTitle('x'), () => win.close(), () => win.webContents.send('x'), () => win.loadFile('x.html')]) try { call(); log.push('no throw'); } catch (error) { log.push(error.message); }
                win.destroy();
            });
        "#;
        let desktop = app(code, &[], std::path::PathBuf::new());
        assert_eq!(
            desktop.main.eval("ids", "test").unwrap(),
            "1,1,2,true,2,true"
        );
        assert_eq!(
            desktop.main.eval("log.join('|')", "test").unwrap(),
            "destroyed|closed|true|true|1|Object has been destroyed|Object has been destroyed|Object has been destroyed|Object has been destroyed"
        );
        assert!(!desktop.views.contains_key(&1) && desktop.views.contains_key(&2));
        desktop.main.eval("other.destroy()", "test").unwrap();
        assert_eq!(
            desktop.main.eval("log.at(-1)", "test").unwrap(),
            "all-closed"
        );
    }

    #[test]
    fn page_navigation_stays_local_and_never_quits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.html");
        std::fs::write(&path, "<script type='application/json'>{}</script><script type='text/javascript'>globalThis.ran=1</script><body>ok</body>").unwrap();
        let files = BBTreeMap::from([(
            "app.js".into(),
            format!(
                "$appsDefine({});",
                serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":"__apps.app.whenReady().then(()=>{globalThis.win=new __apps.BrowserWindow();win.load('<body>first</body>')})"})
            ),
        )]);
        let mut desktop = Desktop::new(&files).unwrap();
        desktop.main.eval("__appsEvent('ready')", "test").unwrap();
        desktop.pump(None, None).unwrap();
        let navigate = |desktop: &Desktop, url: &str| {
            desktop.host.borrow_mut().call(
                Role::Renderer(1),
                "dom.navigate",
                serde_json::json!([url]),
            )
        };
        for url in [
            "https://example.com/",
            "apps:/../../x.html",
            "file://host/share/x.html",
        ] {
            assert!(navigate(&desktop, url).is_err(), "{url}");
        }
        navigate(&desktop, "file:///missing/page.html").unwrap();
        desktop.pump(None, None).unwrap();
        assert!(!desktop.quit);
        let url = url::Url::from_file_path(&path).unwrap();
        navigate(&desktop, url.as_str()).unwrap();
        desktop.pump(None, None).unwrap();
        let renderer = desktop.views[&1].renderer.clone().unwrap();
        assert_eq!(renderer.eval("globalThis.ran", "test").unwrap(), "1");
        assert!(desktop.views[&1].preload.is_none());
    }

    #[test]
    fn check_reports_page_script_errors() {
        let files = BBTreeMap::from([(
            "app.js".into(),
            format!(
                "$appsDefine({});",
                serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":"__apps.app.whenReady().then(()=>new __apps.BrowserWindow().load(`<script>throw Error('page failure')</script>`))"})
            ),
        )]);
        assert!(
            Desktop::new(&files)
                .unwrap()
                .check()
                .unwrap_err()
                .to_string()
                .contains("page failure")
        );
    }

    #[test]
    fn window_errors_are_synchronous_and_closed_windows_are_ignored() {
        let code = "__apps.app.whenReady().then(()=>{try{new __apps.BrowserWindow({webPreferences:{}})}catch(e){globalThis.rejected=e.message}globalThis.win=new __apps.BrowserWindow();win.close()})";
        let files = BBTreeMap::from([(
            "app.js".into(),
            format!(
                "$appsDefine({});",
                serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":code})
            ),
        )]);
        let mut desktop = Desktop::new(&files).unwrap();
        desktop.main.eval("__appsEvent('ready')", "test").unwrap();
        desktop.pump(None, None).unwrap();
        assert_eq!(
            desktop
                .main
                .eval("rejected+':'+__apps.BrowserWindow.getAllWindows().length+':'+win.isDestroyed()", "test")
                .unwrap(),
            "Unsupported BrowserWindow option: webPreferences:0:true"
        );
        assert!(desktop.main.eval("win.show()", "test").is_err());
        desktop
            .host
            .borrow_mut()
            .commands
            .push(serde_json::json!({"op":"window","args":["title",2,"stale"]}));
        desktop.pump(None, None).unwrap();
    }

    #[test]
    fn pages_cannot_settle_other_windows_and_closed_errors_never_quit() {
        let code = r#"
            const {app, BrowserWindow} = __apps;
            globalThis.log = [];
            process.on('uncaughtException', error => log.push(error.message));
            app.on('window-all-closed', () => log.push('all-closed'));
            app.whenReady().then(() => {
                globalThis.a = new BrowserWindow({title: 'a\ud83d'});
                globalThis.b = new BrowserWindow();
                a.load('<p>a</p>');
                b.load('<p>b</p>');
                a.on('closed', () => { throw new Error('closed failed'); });
            });
        "#;
        let mut desktop = app(code, &[], std::path::PathBuf::new());
        let eval = |desktop: &Desktop, script: &str| desktop.main.eval(script, "test").unwrap();
        eval(
            &desktop,
            "globalThis.results = []; for (const contents of [a.webContents, a.webContents]) contents.executeJavaScript('new Promise(() => {})').then(value => results.push(value)); a.webContents.capturePage().then(value => results.push(typeof value), error => results.push(error.message))",
        );
        desktop.pump(None, None).unwrap();
        let page = |id: u64| desktop.views[&id].renderer.clone().unwrap();
        page(2)
            .eval("__appsExecuted(1, 'forged', false)", "test")
            .unwrap();
        page(1)
            .eval("__appsExecuted(3, 'capture', false)", "test")
            .unwrap();
        page(1).eval("__appsExecuted(2, 'own', false)", "test").unwrap();
        desktop.pump(None, None).unwrap();
        desktop.main.tick().unwrap();
        assert_eq!(eval(&desktop, "results.join()"), "own");
        assert_eq!(eval(&desktop, "a.getTitle()"), "a\u{fffd}");
        eval(&desktop, "a.close(); b.close()");
        desktop.pump(None, None).unwrap();
        assert_eq!(
            eval(&desktop, "log.join('|')"),
            "closed failed|all-closed"
        );
        assert!(!desktop.quit);
    }

    #[test]
    fn app_lifecycle_follows_electron() {
        let code = r#"
            const {app, BrowserWindow} = __apps;
            globalThis.log = [];
            for (const name of ['will-finish-launching', 'ready', 'web-contents-created', 'browser-window-created', 'browser-window-focus', 'activate', 'open-url', 'open-file', 'window-all-closed', 'will-quit', 'quit'])
                app.on(name, (_event, value) => log.push(value === undefined || typeof value === 'object' ? name : name + ':' + value));
            let blocked = true;
            app.on('before-quit', event => { log.push('before-quit'); if (blocked) event.preventDefault(); blocked = false; });
            app.commandLine.appendSwitch('flag', 'on');
            app.setPath('userData', app.getPath('temp'));
            globalThis.state = [app.getPath('sessionData') === app.getPath('temp'), !!app.getAppPath(), app.commandLine.getSwitchValue('flag'), app.name].join();
            app.whenReady().then(() => new BrowserWindow());
        "#;
        let files = BBTreeMap::from([(
            "app.js".into(),
            format!(
                "$appsDefine({});",
                serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":code})
            ),
        )]);
        let mut desktop = Desktop::new(&files).unwrap();
        desktop.main.eval("__appsEvent('ready')", "test").unwrap();
        desktop.main.eval("__appsEvent('state',1,{frame:[0,0,800,600],content:[0,28,800,572],minimized:false,maximized:false,fullscreen:false,visible:true,focused:true,resizing:false});__appsEvent('activate',0,true);__appsEvent('open-url',0,'myapp://x');__appsEvent('open-file',0,'/tmp/a');__appsEvent('before-quit')", "test").unwrap();
        desktop.pump(None, None).unwrap();
        assert!(!desktop.quit);
        desktop
            .main
            .eval("__appsEvent('before-quit')", "test")
            .unwrap();
        desktop.pump(None, None).unwrap();
        assert!(desktop.quit);
        assert_eq!(
            desktop.main.eval("state", "test").unwrap(),
            "true,true,on,App"
        );
        assert_eq!(
            desktop.main.eval("log.join()", "test").unwrap(),
            "will-finish-launching,ready,web-contents-created,browser-window-created,browser-window-focus,activate:true,open-url:myapp://x,open-file:/tmp/a,before-quit,before-quit,will-quit,quit:0"
        );
        for (listener, quits) in [
            ("", true),
            ("__apps.app.on('window-all-closed',()=>{});", false),
        ] {
            let code = format!(
                "{listener}__apps.app.whenReady().then(()=>new __apps.BrowserWindow().close())"
            );
            let files = BBTreeMap::from([(
                "app.js".into(),
                format!(
                    "$appsDefine({});",
                    serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":code})
                ),
            )]);
            let mut desktop = Desktop::new(&files).unwrap();
            desktop.main.eval("__appsEvent('ready')", "test").unwrap();
            desktop.pump(None, None).unwrap();
            assert_eq!(desktop.quit, quits);
        }
    }

    #[test]
    fn window_state_events_and_close_follow_electron() {
        let code = r#"
            const {app, BrowserWindow} = __apps;
            globalThis.log = [];
            app.on('window-all-closed', () => {});
            app.whenReady().then(() => {
                globalThis.win = new BrowserWindow({width: 400, height: 300, minWidth: 200, minHeight: 100, title: 'Start', backgroundColor: '#80112233'});
                for (const name of ['resize', 'resized', 'move', 'moved', 'focus', 'blur', 'maximize', 'unmaximize', 'minimize', 'restore', 'enter-full-screen', 'leave-full-screen', 'show', 'hide', 'close', 'closed', 'page-title-updated'])
                    win.on(name, () => log.push(name));
                win.on('page-title-updated', event => event.preventDefault());
                win.on('close', event => { if (!globalThis.allow) event.preventDefault(); });
                win.load('<title>Page</title>');
            });
        "#;
        let files = BBTreeMap::from([(
            "app.js".into(),
            format!(
                "$appsDefine({});",
                serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":code})
            ),
        )]);
        let mut desktop = Desktop::new(&files).unwrap();
        desktop.main.eval("__appsEvent('ready')", "test").unwrap();
        desktop.pump(None, None).unwrap();
        let eval = |desktop: &Desktop, script: &str| desktop.main.eval(script, "test").unwrap();
        assert_eq!(
            eval(
                &desktop,
                "JSON.stringify([win.getBounds(), win.getTitle(), log])"
            ),
            r#"[{"x":0,"y":0,"width":400,"height":300},"Start",["page-title-updated"]]"#
        );
        assert_eq!(desktop.views[&1].title.as_deref(), Some("Page"));
        assert_eq!(desktop.host.borrow().backgrounds[&1], (0x112233, 0x80));
        let state = |frame: [i32; 4], extra: &str| {
            let [x, y, width, height] = frame;
            format!(
                "__appsEvent('state',1,{{frame:[{x},{y},{width},{height}],content:[{x},{},{width},{}],minimized:false,maximized:false,fullscreen:false,visible:true,focused:true,resizing:false,{extra}}})",
                y + 28,
                height - 28
            )
        };
        for (script, events) in [
            (state([10, 20, 400, 300], ""), "focus"),
            (state([10, 20, 500, 300], "resizing:true"), "resize"),
            (state([10, 20, 500, 300], ""), "resized"),
            (state([30, 40, 500, 300], ""), "move,moved"),
            (
                state([0, 0, 900, 700], "maximized:true"),
                "maximize,resize,resized,move,moved",
            ),
            (
                state(
                    [0, 0, 900, 700],
                    "minimized:true,visible:false,focused:false",
                ),
                "minimize,unmaximize,hide,blur",
            ),
            (
                state([0, 0, 900, 700], "fullscreen:true"),
                "restore,enter-full-screen,show,focus",
            ),
        ] {
            eval(&desktop, &format!("log.length = 0; {script}"));
            assert_eq!(eval(&desktop, "log.join()"), events, "{script}");
        }
        assert_eq!(
            eval(
                &desktop,
                "JSON.stringify([win.getBounds(), win.getContentSize(), win.isFullScreen(), win.isNormal(), win.isFocused(), __apps.BrowserWindow.getFocusedWindow() === win, win.contentView.getBounds()])"
            ),
            r#"[{"x":0,"y":0,"width":900,"height":700},[900,672],true,false,true,true,{"x":0,"y":0,"width":900,"height":672}]"#
        );
        desktop.host.borrow_mut().commands.clear();
        assert_eq!(
            eval(
                &desktop,
                "win.setSize(50.4, 50); win.setContentBounds({y: 100}); JSON.stringify([win.getBounds(), win.getContentBounds()])"
            ),
            r#"[{"x":0,"y":72,"width":200,"height":100},{"x":0,"y":100,"width":200,"height":72}]"#
        );
        assert_eq!(
            desktop.host.borrow().commands[1]["args"],
            serde_json::json!(["bounds", 1, 0, 72, 200, 100])
        );
        for script in [
            "win.setSize('x', 1)",
            "win.setMinimumSize(-1, 0)",
            "win.setBackgroundColor('nope')",
            "new __apps.BrowserWindow({frame: 'no'})",
            "new __apps.BrowserWindow({x: Infinity})",
        ] {
            assert_eq!(
                eval(
                    &desktop,
                    &format!("try {{ {script}; 'ok' }} catch {{ 'threw' }}")
                ),
                "threw",
                "{script}"
            );
        }
        eval(&desktop, "__apps.app.quit()");
        desktop
            .host
            .borrow_mut()
            .commands
            .push(serde_json::json!({"op":"window","args":["closing",1]}));
        desktop.pump(None, None).unwrap();
        assert!(!desktop.quit && desktop.views.contains_key(&1));
        assert_eq!(
            eval(&desktop, "log.filter(name => name === 'close').length"),
            "2"
        );
        eval(&desktop, "allow = true; __apps.app.quit()");
        desktop.pump(None, None).unwrap();
        assert!(desktop.quit && !desktop.views.contains_key(&1));
        assert_eq!(eval(&desktop, "log.slice(-2).join()"), "close,closed");
    }

    #[test]
    fn window_titles_follow_the_document() {
        for (html, title) in [
            ("<h1>Page</h1>", None),
            (
                "<html><head><title> Initial   title </title></head><body></body></html>",
                Some("Initial title"),
            ),
            (
                "<title>Initial</title><script>document.title = 'Startup'</script>",
                Some("Startup"),
            ),
        ] {
            let code = format!(
                "__apps.app.whenReady().then(()=>{{new __apps.BrowserWindow().load({});new __apps.BrowserWindow().load('<title>Other</title>')}})",
                serde_json::json!(html)
            );
            let files = BBTreeMap::from([(
                "app.js".into(),
                format!(
                    "$appsDefine({});",
                    serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":code})
                ),
            )]);
            let mut desktop = Desktop::new(&files).unwrap();
            desktop.main.eval("__appsEvent('ready')", "test").unwrap();
            desktop.pump(None, None).unwrap();
            assert_eq!(desktop.views[&1].title.as_deref(), title);
            let renderer = desktop.views[&1].renderer.clone().unwrap();
            for (script, title) in [
                ("document.title = 'New title'", Some("New title")),
                (
                    "document.querySelector('title').textContent = 'DOM title'",
                    Some("DOM title"),
                ),
                (
                    "document.querySelector('title').firstChild.data = 'Text title'",
                    Some("Text title"),
                ),
                (
                    "document.title = '  Spaced \\n title  '",
                    Some("Spaced title"),
                ),
                (
                    "document.implementation.createHTMLDocument('Detached')",
                    Some("Spaced title"),
                ),
                ("document.querySelector('title').remove()", None),
                ("document.title = 'Restored'", Some("Restored")),
                ("document.title = ''", Some("")),
            ] {
                renderer.eval(script, "test").unwrap();
                desktop.pump(None, None).unwrap();
                assert_eq!(desktop.views[&1].title.as_deref(), title, "{script}");
                assert_eq!(desktop.views[&2].title.as_deref(), Some("Other"));
                assert_eq!(
                    renderer.eval("document.title", "test").unwrap(),
                    title.unwrap_or("")
                );
            }
            renderer
                .eval(
                    "getComputedStyle(document.querySelector('title')).color",
                    "test",
                )
                .unwrap();
            assert!(desktop.host.borrow().dom[&1].computed.is_some());
            desktop.host.borrow_mut().commands.push(serde_json::json!({
                "op":"window", "args":["load",1,"<html><body style='color:purple'>New page</body></html>"]
            }));
            desktop.pump(None, None).unwrap();
            let renderer = desktop.views[&1].renderer.as_ref().unwrap();
            assert_eq!(
                renderer
                    .eval("getComputedStyle(document.body).color", "test")
                    .unwrap(),
                "rgb(128, 0, 128)"
            );
        }
    }
}
