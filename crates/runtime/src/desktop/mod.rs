pub mod app;
pub(crate) mod browser;
pub(crate) mod devtools;
pub(crate) mod link;
pub(crate) mod mcp;
pub(crate) mod native;

use crate::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{JavaScriptCore, JsContext},
    render::PageView,
};
use anyhow::{Context as _, Result, bail, ensure};
use bbtree::unsync::BBTreeMap;
use link::{Link, Main, PageQueue, ToMain, ToPage};
use renderer::{App, Rect, WindowChange, WindowId, WindowOptions, point, px, size};
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) struct View {
    pub(crate) handle: Option<WindowId>,
    pub(crate) data: Weak<crate::render::PageState>,
    pub(crate) title: Option<String>,
    pub(crate) preload: Option<Rc<JsContext>>,
    pub(crate) renderer: Option<Rc<JsContext>>,
    fullscreen: Option<[bool; 2]>,
    focused: bool,
}

impl View {
    fn engines(&self) -> impl Iterator<Item = &JsContext> {
        self.preload
            .iter()
            .chain(self.renderer.iter())
            .map(Rc::as_ref)
    }
}

pub struct Desktop {
    api: Rc<JavaScriptCore>,
    host: Rc<RefCell<Host>>,
    pub(crate) main: Option<Rc<Main>>,
    pub(crate) link: Link,
    queue: Arc<PageQueue>,
    pub(crate) views: HashMap<u64, View>,
    devtools: HashMap<u64, devtools::DevTools>,
    main_logs: Vec<serde_json::Value>,
    files: Arc<HashMap<String, String>>,
    template: Arc<Template>,
    owner: Option<u64>,
    ended: bool,
    quit: bool,
    wake: Option<(Instant, renderer::Timer)>,
    scheduled: bool,
}

type Reloaded = (
    Arc<HashMap<String, String>>,
    Arc<HashMap<String, bytes::Bytes>>,
);

static RELOADED: std::sync::Mutex<Option<Reloaded>> = std::sync::Mutex::new(None);

thread_local! {
    // Skip destruction: this needs HOSTS, which exits first.
    static PAGE_SIDE: std::mem::ManuallyDrop<RefCell<Option<Rc<RefCell<Desktop>>>>> = const { std::mem::ManuallyDrop::new(RefCell::new(None)) };
}

struct Template {
    rust_main: bool,
    version: Option<String>,
    disabled: u32,
    name: Option<String>,
    path: std::path::PathBuf,
    packaged: bool,
    scripts: Arc<HashMap<String, bytes::Bytes>>,
    assets: HashMap<String, std::path::PathBuf>,
    native_modules: HashMap<String, std::path::PathBuf>,
    files: Arc<HashMap<String, String>>,
    packed: Arc<HashMap<String, bytes::Bytes>>,
}

impl Template {
    fn host(&self, link: &Link) -> Host {
        Host {
            link: Some(link.clone()),
            rust_main: self.rust_main,
            app_version: self.version.clone(),
            disabled: self.disabled,
            app_name: self.name.clone(),
            app_path: self.path.clone(),
            packaged: self.packaged,
            scripts: self.scripts.clone(),
            assets: self.assets.clone(),
            native_modules: self.native_modules.clone(),
            ..Host::default()
        }
    }
}

impl Desktop {
    pub fn from_bundle(bundle: crate::bundle::Bundle) -> Result<Self> {
        ensure!(
            bundle.main.is_some() || bundle.files.contains_key("app.js"),
            "Start a Rust app with its Cargo executable"
        );
        Self::create(bundle, false)
    }

    pub fn reload_from(&self, mut input: impl std::io::BufRead + Send + 'static) {
        let link = self.link.clone();
        let _ = std::thread::Builder::new()
            .name("apps-reload".into())
            .spawn(move || {
                let mut line = Vec::new();
                while input
                    .read_until(b'\n', &mut line)
                    .is_ok_and(|read| read > 0)
                {
                    match serde_json::from_slice::<crate::bundle::Bundle>(&line) {
                        Ok(bundle) => {
                            let mut pages = HashMap::new();
                            let mut scripts = HashMap::new();
                            for (name, source) in bundle.files.iter() {
                                if name.ends_with(".html") {
                                    pages.insert(name.clone(), source.clone());
                                } else if name.starts_with("script-") {
                                    scripts.insert(name.clone(), source.clone().into());
                                }
                            }
                            *RELOADED.lock().unwrap_or_else(|error| error.into_inner()) =
                                Some((Arc::new(pages), Arc::new(scripts)));
                            link.page(ToPage::All(serde_json::json!({"op":"reload"})));
                        }
                        Err(error) => eprintln!("apps: cannot read the new bundle: {error}"),
                    }
                    line.clear();
                }
            });
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
        let files = files.clone();
        Self::create(
            crate::bundle::Bundle {
                files,
                ..Default::default()
            },
            false,
        )
    }

    pub(crate) fn create(bundle: crate::bundle::Bundle, rust: bool) -> Result<Self> {
        let crate::bundle::Bundle {
            packaged,
            root,
            files,
            pages,
            scripts,
            main,
        } = bundle;
        let files = &files;
        let api = JavaScriptCore::load()?;
        let windows = !pages.is_empty() || files.keys().any(|name| name.ends_with(".html"));
        // Preload the window server, Metal and CoreText here to avoid delays at launch and the first frame.
        #[cfg(target_os = "macos")]
        if windows {
            let _ = std::thread::Builder::new()
                .name("apps-warm".into())
                .spawn(|| {
                    use renderer::text::{Font, TextRun, TextSystem};
                    #[link(name = "CoreGraphics", kind = "framework")]
                    unsafe extern "C" {
                        fn CGMainDisplayID() -> u32;
                    }
                    #[link(name = "Metal", kind = "framework")]
                    unsafe extern "C" {
                        fn MTLCopyAllDevices() -> *const std::ffi::c_void;
                    }
                    #[link(name = "CoreFoundation", kind = "framework")]
                    unsafe extern "C" {
                        fn CFRelease(value: *const std::ffi::c_void);
                    }
                    unsafe { CGMainDisplayID() };
                    let text = TextSystem::new();
                    let run = TextRun {
                        len: 2,
                        font: Font::new(".SystemUIFont"),
                        ..Default::default()
                    };
                    let line = text.shape_line("Ag", renderer::px(13.), &run);
                    if let Some(glyph) = line.line.glyphs.first() {
                        text.glyph_pixels(glyph.font, glyph.id, glyph.size, 2., 0, false);
                    }
                    text.face(&Font {
                        weight: 600.,
                        ..Font::new(".SystemUIFont")
                    });
                    text.face(&Font::new("Menlo"));
                    text.face(&Font::new("Helvetica"));
                    let devices = unsafe { MTLCopyAllDevices() };
                    if !devices.is_null() {
                        unsafe { CFRelease(devices) };
                    }
                });
        }
        let link = Link::new();
        // Install the waker before the app runs, because native producers of this thread capture it then.
        crate::host::Waker::install({
            let link = link.clone();
            move || link.wake_main()
        });
        apps_webview::on_event({
            let link = link.clone();
            move || {
                link.wake_main();
                link.wake_pages();
            }
        });
        let package = files.get("package.json").map(String::as_str);
        let (_, name, version, _) = crate::bundle::package_metadata(package)?;
        let disabled = crate::bundle::disabled_features(package)?;
        if windows {
            crate::jsc::prebuild(
                &api,
                disabled & crate::bundle::feature_bit("webgpu") == 0,
                !rust,
            );
        }
        crate::web::LOCAL_NETWORK.store(
            disabled & crate::bundle::feature_bit("localNetwork") == 0,
            Ordering::Relaxed,
        );
        let path = if root.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_default()
        } else {
            root
        };
        let scripts = Arc::new(scripts);
        let assets: HashMap<String, std::path::PathBuf> = files
            .get("assets.json")
            .map(|source| serde_json::from_str(source))
            .transpose()
            .context("Invalid assets.json")?
            .unwrap_or_default();
        let _ = crate::web::ASSETS.set(assets.clone());
        let mut host = Host {
            link: Some(link.clone()),
            rust_main: rust,
            app_version: Some(version),
            disabled,
            app_name: Some(name),
            app_path: path,
            packaged,
            scripts,
            assets,
            ..Host::default()
        };
        host.install(files)?;
        let template = Arc::new(Template {
            rust_main: host.rust_main,
            version: host.app_version.clone(),
            disabled: host.disabled,
            name: host.app_name.clone(),
            path: host.app_path.clone(),
            packaged: host.packaged,
            scripts: host.scripts.clone(),
            assets: host.assets.clone(),
            native_modules: host.native_modules.clone(),
            files: Arc::new(
                files
                    .iter()
                    .filter(|(name, _)| name.ends_with(".html"))
                    .map(|(name, source)| (name.clone(), source.clone()))
                    .collect(),
            ),
            packed: Arc::new(pages),
        });
        let host = Rc::new(RefCell::new(host));
        let main_host = Rc::new(RefCell::new(template.host(&link)));
        let engine = if rust {
            None
        } else {
            let engine = JsContext::new(api.clone(), main_host.clone(), Role::Main)?;
            match &main {
                Some(main) => engine.load_main(
                    files,
                    std::str::from_utf8(main).context("Invalid app.bundle data")?,
                    "app.js",
                )?,
                None => engine.load_bundle(files, "app.js")?,
            }
            Some(engine)
        };
        Ok(Self {
            main: Some(Main::new(engine, main_host, link.clone(), api.clone())),
            api,
            host,
            queue: link.shared_queue(),
            link,
            views: HashMap::new(),
            devtools: HashMap::new(),
            main_logs: Vec::new(),
            files: template.files.clone(),
            template,
            owner: None,
            ended: false,
            quit: false,
            wake: None,
            scheduled: false,
        })
    }

    fn pump(
        &mut self,
        mut cx: Option<&mut App>,
        runtime: Option<&Rc<RefCell<Self>>>,
    ) -> Result<()> {
        for _ in 0..64 {
            if let Some(main) = &self.main {
                main.receive();
            }
            let messages = self.queue.take();
            if messages.is_empty() && self.link.quitting() {
                self.quit = true;
                return Ok(());
            }
            if messages.is_empty() {
                if runtime.is_none() {
                    self.titles(cx.as_deref_mut())?;
                }
                if !self.queue.pending() && !(self.main.is_some() && self.link.main_pending()) {
                    return Ok(());
                }
                continue;
            }
            for message in messages {
                let (id, command) = match message {
                    ToPage::Bridge(id, message) => {
                        if let Some(preload) = self.views.get(&id).and_then(|v| v.preload.clone())
                            && let Err(error) =
                                scripted(cx.as_deref_mut(), || preload.deliver(message))
                        {
                            eprintln!("apps: {error:#}");
                        }
                        continue;
                    }
                    ToPage::All(command) => {
                        if command["op"] == "theme" {
                            self.host.borrow_mut().sync_theme();
                        } else if command["op"] == "logs" && !self.devtools.is_empty() {
                            self.main_logs
                                .extend(command["args"].as_array().into_iter().flatten().cloned());
                        } else if command["op"] == "reload" {
                            let host = self.host.borrow();
                            for &id in self.views.keys() {
                                if let Some(url) =
                                    host.dom.get(&id).and_then(|dom| dom.base.as_ref())
                                    && url.as_str().starts_with("apps:///")
                                {
                                    self.link.page(ToPage::Window(
                                        id,
                                        serde_json::json!({"op":"window","args":["loadFile",id,url.as_str(),"page"]}),
                                    ));
                                }
                            }
                        }
                        continue;
                    }
                    ToPage::Window(id, command) => (id, command),
                };
                let args = &command["args"];
                let action = args[0].as_str().context("Missing window action")?;
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
                        let vibrancy = options["vibrancy"]
                            .as_i64()
                            .filter(|_| cfg!(target_os = "macos"));
                        let background = options["backgroundColor"]
                            .as_str()
                            .and_then(crate::style::color)
                            .unwrap_or(if transparent || vibrancy.is_some() {
                                (0, 0)
                            } else {
                                (0xffffff, 255)
                            });
                        self.host.borrow_mut().backgrounds.insert(id, background);
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
                            let child = options["parent"].is_u64();
                            let later = child || options["focusable"] == false;
                            let show = options["show"] != false;
                            let window_options = WindowOptions {
                                bounds: Rect::new(origin, outer),
                                outer_size: options["useContentSize"] != true,
                                title: options["title"].as_str().unwrap_or("apps").to_owned(),
                                titlebar_hidden: hidden,
                                buttons_at: traffic_light_position,
                                frameless: !frame,
                                transparent,
                                show: show && !later,
                                resizable: options["resizable"] != false,
                                minimizable: options["minimizable"] != false,
                                movable: options["movable"] != false,
                            };
                            let handle = cx.open_window(window_options, |window, _| {
                                let view = PageView::new(id, runtime, host, window.id());
                                data = Rc::downgrade(&view.0);
                                Box::new(view)
                            })?;
                            cx.update_window(handle, |window, _| {
                                self.link.main(ToMain::Window(
                                    id,
                                    window.id(),
                                    window.control().clone(),
                                ));
                                for (apply, change) in [
                                    (
                                        min != [0.; 2] || max != [0.; 2],
                                        WindowChange::SizeLimits(
                                            size(px(min[0]), px(min[1])),
                                            size(px(max[0]), px(max[1])),
                                        ),
                                    ),
                                    (options["alwaysOnTop"] == true, WindowChange::AlwaysOnTop(3)),
                                    (
                                        options["maximizable"] == false,
                                        WindowChange::Maximizable(false),
                                    ),
                                    (options["closable"] == false, WindowChange::Closable(false)),
                                    (
                                        true,
                                        WindowChange::Background(renderer::Color::hex(
                                            (background.0 << 8) | background.1 as u32,
                                        )),
                                    ),
                                    (
                                        options["opacity"].is_number(),
                                        WindowChange::Opacity(number("opacity", 1.)),
                                    ),
                                    (options["hasShadow"] == false, WindowChange::Shadow(false)),
                                    (
                                        options["focusable"] == false,
                                        WindowChange::Focusable(false),
                                    ),
                                    (
                                        vibrancy.is_some()
                                            || options["visualEffectState"].as_i64().unwrap_or(0)
                                                != 0,
                                        WindowChange::Vibrancy(
                                            vibrancy.map(|material| material as isize),
                                            options["visualEffectState"].as_i64().unwrap_or(0)
                                                as isize,
                                        ),
                                    ),
                                    (later && show && !child, WindowChange::Show(true)),
                                ] {
                                    if apply {
                                        window.change_window(change);
                                    }
                                }
                                if options["fullscreen"] == true {
                                    window.control().toggle_fullscreen();
                                }
                                let size = window.viewport_size();
                                self.host
                                    .borrow_mut()
                                    .viewports
                                    .insert(id, [f32::from(size.width), f32::from(size.height)]);
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
                                fullscreen: None,
                                focused: true,
                            },
                        );
                    }
                    "load" | "loadFile" => {
                        if let Some((files, scripts)) = RELOADED
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .clone()
                        {
                            self.files = files;
                            self.host.borrow_mut().scripts = scripts;
                        }
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
                                if let Some(bytes) = self.template.packed.get(name) {
                                    return crate::bundle::unpack_page(bytes)
                                        .map_err(|_| (-2, "ERR_FAILED"));
                                }
                                return match self.files.get(name) {
                                    Some(source) => Ok(crate::bundle::page(source)),
                                    None => Err(missing),
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
                                if self.host.borrow().rust_main && !page {
                                    bail!("Cannot load {value}: {description}");
                                }
                                if page {
                                    eprintln!(
                                        "apps: navigation: {description} ({code}) loading '{url}'"
                                    );
                                }
                                let event = serde_json::json!({"request":request,"url":url,"code":code,"description":description});
                                self.link.event(
                                    format!("__appsEvent('failed',{id},{event})"),
                                    "apps:failed",
                                );
                                continue;
                            }
                        };
                        renderer::text::warm(crate::style::font_lists(&doc));
                        // Only bundled pages can run Node preloads. Never enable them for user files or HTML
                        // strings.
                        let trusted = location.starts_with("apps:///")
                            || !page
                                && file.as_ref().is_some_and(|path| {
                                    std::fs::canonicalize(path).is_ok_and(|path| {
                                        std::fs::canonicalize(&root)
                                            .is_ok_and(|root| path.starts_with(root))
                                    })
                                });
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
                                    let text = doc.text(id);
                                    return (!text.is_empty()).then_some((kind, Cow::Owned(text)));
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
                        {
                            let mut host = self.host.borrow_mut();
                            reset(&mut host, id);
                            host.pages.insert(id, Page { doc, dirty: true });
                        }
                        let view = self.views.get_mut(&id).context("Window is closed")?;
                        view.focused = true;
                        if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                            cx.update_window(handle, |window, _| {
                                let mut host = self.host.borrow_mut();
                                let state = host.dom.entry(id).or_default();
                                state.text_system = Some(window.text_system().clone());
                                state.pixel_ratio = window.scale_factor();
                                state.window = window.window_state();
                                // Like Playwright, pages count as focused while the devtools MCP server runs, so tests in hidden windows get focus events.
                                view.focused = window.is_active() || mcp::enabled();
                            });
                        }
                        view.preload = None;
                        view.renderer = None;
                        let preload = (trusted
                            && scripts.iter().any(|(kind, _)| *kind == "preload"))
                        .then(|| {
                            JsContext::new(self.api.clone(), self.host.clone(), Role::Preload(id))
                        })
                        .transpose();
                        let renderer =
                            JsContext::new(self.api.clone(), self.host.clone(), Role::Renderer(id));
                        if (preload.is_err() || renderer.is_err()) && crate::jsc::stopping(id) {
                            continue;
                        }
                        let preload = preload?.map(Rc::new);
                        let renderer = Rc::new(renderer?);
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
                        if !view.focused {
                            renderer.eval(
                                "__appsDispatch('window:blur',[],{quiet:true})",
                                "apps:blur",
                            )?;
                        }
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
                        // Run one page script per __appsNext call so JSC drains microtasks between scripts.
                        use crate::jsc::NEXT;
                        for (mut source, url) in scripts
                            .iter()
                            .filter(|(kind, _)| *kind == "module")
                            .map(|(_, script)| (&**script, "apps:page"))
                            .chain([("__appsLoaded()", "apps:loaded")])
                        {
                            loop {
                                let more = match scripted(cx.as_deref_mut(), || page(source, url)) {
                                    Ok(more) => more == "true",
                                    Err(error) => {
                                        if cx.is_none() {
                                            return Err(error);
                                        }
                                        eprintln!("apps: page script: {error:#}");
                                        true
                                    }
                                };
                                if url != "apps:page" || source == NEXT && !more {
                                    break;
                                }
                                source = NEXT;
                            }
                        }
                        if let Some(preload) = &preload {
                            let ready = || {
                                preload.eval(
                                    "dispatchEvent(new Event('DOMContentLoaded'))",
                                    "apps:ready",
                                )
                            };
                            if let Err(error) = scripted(cx.as_deref_mut(), ready) {
                                eprintln!("apps: preload script: {error:#}");
                            }
                        }
                        view.preload = preload;
                        view.renderer = Some(renderer);
                        view.title = None;
                        if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                            cx.refresh_window(handle);
                        }
                        let event = serde_json::json!({"request":request,"url":location});
                        self.link
                            .event(format!("__appsEvent('loaded',{id},{event})"), "apps:loaded");
                    }
                    "deliver" => {
                        let name = match args[2].as_str() {
                            Some("webviewEvent") => "__appsWebviewEvent",
                            Some("webviewReply") => "__appsWebviewReply",
                            Some("filesPicked") => "__appsFilesPicked",
                            Some("selectPicked") => "__appsSelectPicked",
                            _ => continue,
                        };
                        if let Some(renderer) = self.views[&id].renderer.clone() {
                            let values = args[3]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(serde_json::Value::to_string)
                                .collect::<Vec<_>>();
                            let values = values.iter().map(String::as_str).collect::<Vec<_>>();
                            if let Err(error) =
                                scripted(cx.as_deref_mut(), || renderer.call(name, &values))
                            {
                                eprintln!("apps: page script: {error:#}");
                            }
                        }
                    }
                    "edit" => {
                        if let Some(renderer) = self.views[&id].renderer.clone() {
                            let command = args[2].as_str().unwrap_or_default();
                            let text = args[3].as_str().unwrap_or_default();
                            let app = cx.as_deref_mut().map(|cx| (cx, None));
                            crate::render::edit(&renderer, app, command, text);
                        }
                    }
                    "insertCSS" | "removeInsertedCSS" => {
                        let mut host = self.host.borrow_mut();
                        let host = &mut *host;
                        let Some(page) = host.pages.get_mut(&id) else {
                            continue;
                        };
                        let state = host.dom.entry(id).or_default();
                        let key = args[2].as_str().unwrap_or_default();
                        if action == "insertCSS" {
                            let css = args[3].as_str().unwrap_or_default();
                            let sheet = Rc::new(crate::style::Stylesheet::parse(css));
                            state
                                .inserted
                                .push((key.to_owned(), sheet, args[4] == true));
                        } else {
                            state.inserted.retain(|(old, ..)| old != key);
                        }
                        state.invalidate();
                        page.dirty = true;
                    }
                    "devtools" => {
                        if self.host.borrow().packaged {
                            continue;
                        }
                        let cx = cx
                            .as_deref_mut()
                            .context("Developer Tools require a native window")?;
                        let view = self.views.get(&id);
                        // The console or the picker opens the tools, and keeps open tools.
                        let show = args[2].as_str();
                        let fresh = !self.devtools.contains_key(&id);
                        let open = fresh || show.is_some();
                        if fresh {
                            let title = format!("Window {id}");
                            let tools = view
                                .and_then(|view| view.handle)
                                .and_then(|handle| {
                                    cx.update_window(handle, |window, _| {
                                        devtools::DevTools::new(
                                            &title,
                                            Role::Renderer(id),
                                            Some(window),
                                        )
                                    })
                                })
                                .unwrap_or_else(|| {
                                    devtools::DevTools::new(&title, Role::Renderer(id), None)
                                })?;
                            self.devtools.insert(id, tools);
                        } else if !open {
                            self.devtools.remove(&id);
                            if let Some(engine) = view.and_then(|view| view.renderer.as_deref()) {
                                devtools::release(engine)?;
                            }
                        }
                        if let Some(data) = view.and_then(|view| view.data.upgrade()) {
                            let mut inspect = data.inspect.get();
                            if fresh || !open {
                                inspect = crate::render::Inspect {
                                    open,
                                    ..Default::default()
                                };
                            }
                            if show == Some("pick") {
                                inspect.pick = true;
                            }
                            data.inspect.set(inspect);
                            if let Some(tools) = self.devtools.get(&id) {
                                tools.inspected(&data)?;
                                if show == Some("console") {
                                    tools
                                        .events(&[serde_json::json!({"type":"show","tab":show})])?;
                                }
                            }
                        }
                        if let Some(handle) = view.and_then(|view| view.handle) {
                            cx.refresh_window(handle);
                        }
                    }
                    // The FPS HUD of the window. Open developer tools show if it is on.
                    "hud" => {
                        if self.host.borrow().packaged {
                            continue;
                        }
                        let (Some(view), Some(cx)) = (self.views.get(&id), cx.as_deref_mut())
                        else {
                            continue;
                        };
                        let (Some(data), Some(handle)) = (view.data.upgrade(), view.handle) else {
                            continue;
                        };
                        cx.update_window(handle, |window, _| {
                            crate::render::Hud::toggle(&data, window)
                        });
                        if let Some(tools) = self.devtools.get(&id) {
                            tools.inspected(&data)?;
                        }
                    }
                    // The picker of the developer tools: the pointer moved to a place on the page, or
                    // pressed there. Without a place it turns the picker on or off.
                    "inspect" => {
                        let (Some(tools), Some(view)) =
                            (self.devtools.get(&id), self.views.get(&id))
                        else {
                            continue;
                        };
                        let (Some(data), Some(engine), Some(cx)) =
                            (view.data.upgrade(), &view.renderer, cx.as_deref_mut())
                        else {
                            continue;
                        };
                        let mut inspect = data.inspect.get();
                        if args[2].is_number() {
                            let at = serde_json::json!({"x":args[2],"y":args[3]});
                            let node = crate::render::scripted(cx, None, || {
                                devtools::agent(engine, "pick", &at)
                            })?
                            .as_u64()
                            .map(|node| node as usize);
                            if args[4] == true {
                                inspect.selected = node;
                            } else if inspect.hover == node {
                                continue;
                            }
                            inspect.hover = node;
                        } else {
                            inspect.pick = args[2].as_bool().unwrap_or(!inspect.pick);
                            inspect.hover = None;
                        }
                        data.inspect.set(inspect);
                        if !args[2].is_number() || args[4] == true {
                            tools.inspected(&data)?;
                        }
                        if let Some(handle) = view.handle {
                            cx.refresh_window(handle);
                        }
                    }
                    "close" => {
                        self.devtools.remove(&id);
                        if let Some(view) = self.views.remove(&id) {
                            if let Some(data) = view.data.upgrade() {
                                data.frames.borrow_mut().clear();
                            }
                            if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                                cx.update_window(handle, |window, _| window.close());
                            }
                            let mut host = self.host.borrow_mut();
                            reset(&mut host, id);
                            host.pages.remove(&id);
                            host.viewports.remove(&id);
                            host.backgrounds.remove(&id);
                            drop(host);
                            crate::web::storage::forget(id);
                            crate::jsc::page_stopped(id);
                            self.ended = self.owner == Some(id);
                            if self.ended {
                                mcp::uninstall();
                            }
                        }
                    }
                    "menuAfterFrame" => {
                        if let (Some(handle), Some(cx)) =
                            (self.views[&id].handle, cx.as_deref_mut())
                        {
                            let link = self.link.clone();
                            cx.update_window(handle, |window, _| {
                                window.next_frame(|window, _| {
                                    window.next_frame(move |_, _| {
                                        link.event("__appsInitMenu()".into(), "apps:menu")
                                    })
                                })
                            });
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
                            let link = self.link.clone();
                            cx.update_window(handle, |window, _| {
                                let scale = window.scale_factor() / window.zoom();
                                window.capture(move |frame, cx| {
                                    let data = match frame.and_then(|frame| png(frame, scale, &rect, false)) {
                                        Ok(([width, height], bytes)) => serde_json::json!({"id":request,"width":width,"height":height,"png":base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)}),
                                        Err(error) => serde_json::json!({"id":request,"error":error.to_string()}),
                                    };
                                    // The window is busy drawing here, so a capture requested by the reply must wait for the next drive.
                                    cx.defer(move |cx| {
                                        link.event(format!("__appsEvent('captured',{id},{data})"), "apps:capture");
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
                                scripted(cx.as_deref_mut(), || {
                                    renderer.eval_page(&source, "apps:execute")
                                })
                                .err()
                            }
                                .map(|error| serde_json::json!({"error":error.to_string()}).to_string().into()),
                        };
                        if let Some(data) = result {
                            self.link.event(
                                format!(
                                    "__appsEvent('executed',{id},{})",
                                    serde_json::json!({"request":request,"data":data})
                                ),
                                "apps:executed",
                            );
                        }
                    }
                    "htmlFullscreen" => {
                        let on = args[2] == true;
                        let view = self.views.get_mut(&id).context("Window is closed")?;
                        if on == view.fullscreen.is_some() {
                            continue;
                        }
                        let before = view.fullscreen.take().map(|[before, _]| before);
                        let mut was = false;
                        if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                            cx.update_window(handle, |window, _| {
                                was = window.window_state().fullscreen;
                                if on && !was || before == Some(false) && was {
                                    window.control().toggle_fullscreen();
                                }
                            });
                        }
                        view.fullscreen = on.then_some([was, was]);
                        self.link.event(
                            format!("__appsEvent('html-full-screen',{id},{on})"),
                            "apps:fullscreen",
                        );
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
                                window.change_window(WindowChange::Background(
                                    renderer::Color::hex((color.0 << 8) | color.1 as u32),
                                ));
                                window.refresh();
                            });
                        }
                    }
                    "zoom" => {
                        if let (Some(handle), Some(cx)) =
                            (self.views[&id].handle, cx.as_deref_mut())
                        {
                            cx.update_window(handle, |window, _| {
                                window.set_zoom(args[2].as_f64().unwrap_or(1.) as f32)
                            });
                        }
                    }
                    "vibrancy" => {
                        if let (Some(handle), Some(cx)) =
                            (self.views[&id].handle, cx.as_deref_mut())
                        {
                            cx.update_window(handle, |window, _| {
                                window.change_window(WindowChange::Vibrancy(
                                    args[2].as_i64().map(|material| material as isize),
                                    args[3].as_i64().unwrap_or(0) as isize,
                                ));
                            });
                        }
                    }
                    _ => bail!("Unsupported window action: {action}"),
                }
            }
        }
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
            if !self.host.borrow().rust_main {
                self.link.event(
                    format!("__appsEvent('title',{id},{})", serde_json::json!(text)),
                    "apps:title",
                );
            } else if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                cx.update_window(handle, |window, _| window.control().set_title(text));
            }
        }
        Ok(())
    }

    pub(crate) fn drive(runtime: Rc<RefCell<Self>>, cx: &mut App) {
        if runtime.try_borrow_mut().is_err() {
            return;
        }
        crate::host::idle(false);
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
                let zoom = app
                    .views
                    .get(&id)
                    .and_then(|view| view.handle)
                    .and_then(|handle| cx.update_window(handle, |window, _| window.zoom()))
                    .unwrap_or(1.);
                let event = format!(
                    "__appsEvent('resize',{id},{{width:{},height:{}}})",
                    (width * zoom).round(),
                    (height * zoom).round()
                );
                app.link.event(event, "apps:resize");
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
                let Some((focused, fullscreen)) = app
                    .views
                    .get(&id)
                    .and_then(|view| view.handle)
                    .and_then(|handle| {
                        cx.update_window(handle, |window, _| {
                            (window.is_active(), window.window_state().fullscreen)
                        })
                    })
                else {
                    continue;
                };
                let Some(view) = app.views.get_mut(&id) else {
                    continue;
                };
                let focused = focused || mcp::enabled();
                if view.focused != focused
                    && let Some(renderer) = &view.renderer
                {
                    view.focused = focused;
                    let script = if focused {
                        "__appsDispatch('window:focus',[])"
                    } else {
                        "__appsDispatch('window:blur',[])"
                    };
                    if let Err(error) =
                        crate::render::scripted(cx, None, || renderer.eval(script, "apps:focus"))
                    {
                        eprintln!("apps: {error}");
                    }
                }
                match &mut view.fullscreen {
                    Some([_, seen]) if fullscreen => *seen = true,
                    Some([_, true]) => {
                        view.fullscreen = None;
                        if let Some(renderer) = &view.renderer
                            && let Err(error) = crate::render::scripted(cx, None, || {
                                renderer
                                    .eval("__appsDispatch('fullscreen:exit',[])", "apps:fullscreen")
                            })
                        {
                            eprintln!("apps: {error}");
                        }
                        app.link.event(
                            format!("__appsEvent('html-full-screen',{id},false)"),
                            "apps:fullscreen",
                        );
                    }
                    _ => {}
                }
            }
            let mut delay = i64::MAX;
            for engine in app.views.values().flat_map(View::engines) {
                let next = match crate::render::scripted(cx, None, || engine.tick()) {
                    Ok(next) => next,
                    Err(error) => {
                        eprintln!("apps: page script: {error:#}");
                        continue;
                    }
                };
                if next >= 0 {
                    delay = delay.min(next);
                }
            }
            let start = Instant::now();
            loop {
                app.pump(Some(cx), Some(&runtime))?;
                if app.quit {
                    return Ok(None);
                }
                if !app.queue.pending() || start.elapsed() >= Duration::from_millis(4) {
                    break;
                }
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
                let Some(view) = app.views.get(&id) else {
                    continue;
                };
                let Some(engine) = view.renderer.as_deref() else {
                    continue;
                };
                let (commands, data) = (tools.commands(), view.data.upgrade());
                crate::render::scripted(cx, None, || -> Result<()> {
                    for command in &commands {
                        tools.answer(command, engine, &app.host, data.as_deref())?;
                    }
                    delay = delay.min(tools.poll(engine)?);
                    if tools.is_closed() {
                        devtools::release(engine)?;
                    }
                    Ok(())
                })?;
                if let Some(handle) = view.handle.filter(|_| !commands.is_empty()) {
                    cx.refresh_window(handle);
                }
                if let Some(data) = data.filter(|_| tools.is_closed()) {
                    data.inspect.take();
                }
            }
            app.devtools.retain(|_, tools| !tools.is_closed());
            let mcp = mcp::enabled();
            if !app.devtools.is_empty() || mcp {
                let logs: Vec<_> = app.host.borrow_mut().logs.drain(..).collect();
                let main_logs = std::mem::take(&mut app.main_logs);
                for tools in app.devtools.values() {
                    tools.events(&logs)?;
                    tools.events(&main_logs)?;
                }
                mcp::logs(&logs);
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
                    let frames = data.frames.borrow();
                    let active = !frames.is_empty();
                    let events = frames
                        .iter()
                        .flat_map(|(&node, frame)| {
                            frame
                                .browser
                                .poll()
                                .into_iter()
                                .map(move |event| (node, event))
                        })
                        .collect::<Vec<_>>();
                    drop(frames);
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
                    // Process native control events before the next frame acknowledges them, or stale page
                    // values can undo edits.
                    let controls = crate::render::native::events(&data);
                    for (node, event) in &controls {
                        let value = serde_json::to_string(&event.value).unwrap_or_default();
                        if let Some(renderer) = &view.renderer
                            && let Err(error) = crate::render::scripted(cx, None, || {
                                renderer.eval(
                                    &format!("__appsNativeControl({node},{value},{})", event.done),
                                    "apps:control",
                                )
                            })
                        {
                            eprintln!("apps: page script: {error:#}");
                        }
                    }
                    if !controls.is_empty()
                        && let Some(handle) = view.handle
                    {
                        cx.refresh_window(handle);
                    }
                }
            }
            app.titles(Some(cx))?;
            let host = app.host.borrow();
            if app.queue.pending() || !host.resized.is_empty() || !host.window_changes.is_empty() {
                delay = delay.min(0);
            }
            drop(host);
            // IPC, microtasks and the scripts above can add timers after an engine's tick, so read all delays again.
            for engine in app.views.values().flat_map(View::engines) {
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
                if frame
                    && let (Some(handle), Some(data)) = (view.handle, view.data.upgrade())
                    && cx
                        .update_window(handle, |window, _| {
                            crate::render::queue_animation_frame(data, window)
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
            Ok(if delay == i64::MAX {
                None
            } else {
                Some(delay.clamp(0, 1 << 31) as u64)
            })
        })();
        if runtime.try_borrow().is_ok_and(|app| app.ended) {
            cx.defer(|cx| {
                PAGE_SIDE.with(|side| side.take());
                cx.end_thread();
            });
        }
        let api = runtime.borrow().api.clone();
        if api.shrink_pending() && !api.idle.replace(true) {
            shrink_when_quiet(runtime.clone(), cx);
        }
        match result {
            Ok(Some(delay)) => {
                let wait = Duration::from_millis(delay);
                let at = Instant::now() + wait;
                let mut app = runtime.borrow_mut();
                // A replaced timer stays registered until it fires, so keep the armed one unless the new deadline is earlier.
                if app.wake.as_ref().is_none_or(|(armed, _)| {
                    *armed <= Instant::now() || at + Duration::from_micros(500) < *armed
                }) {
                    let wake = runtime.clone();
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
                runtime.borrow_mut().fail();
            }
        }
        crate::host::idle(true);
    }

    fn fail(&mut self) {
        self.quit = true;
        self.link.fail();
    }

    pub fn check(mut self) -> Result<i32> {
        let main = self.main.clone().context("Missing the main side")?;
        if let Some(main) = &main.engine {
            main.eval("__appsEvent('ready')", "apps:ready")?;
        }
        self.pump(None, None)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            for engine in main
                .engine
                .iter()
                .chain(self.views.values().flat_map(View::engines))
            {
                engine.tick()?;
            }
            self.pump(None, None)?;
            let mut pending = false;
            for engine in main
                .engine
                .iter()
                .chain(self.views.values().flat_map(View::engines))
            {
                pending |= engine.eval("__appsPending()", "apps:startup")? == "true";
            }
            pending |= self.queue.pending() || self.link.main_pending();
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
        Ok(self.link.exit_code())
    }

    pub fn run(self, devtools: bool, mcp: Option<u16>) -> Result<i32> {
        ensure!(
            !(devtools || mcp.is_some()) || !self.host.borrow().packaged,
            "Developer tools are unavailable in packaged apps"
        );
        let Self {
            api,
            host,
            main,
            link,
            template,
            ..
        } = self;
        drop((api, host));
        let main = main.context("Missing the main side")?;
        apps_webview::on_main_thread(|task| renderer::native::sync(task), renderer::native::post);
        if let Some(port) = mcp {
            mcp::start(port, link.clone())?;
        }
        if renderer::THREAD_PER_APP {
            link.set_per_window();
            let (template, link) = (template.clone(), link.clone());
            main.set_spawner(Box::new(move |id, queue| {
                let (template, link) = (template.clone(), link.clone());
                renderer::spawn(move |cx| Self::start(template, link, queue, Some(id), cx));
            }));
        }
        let exit = link.clone();
        renderer::run(move || {
            let pending = Arc::new(AtomicBool::new(false));
            link.set_wake_main(move || {
                if !pending.swap(true, Ordering::AcqRel) {
                    let pending = pending.clone();
                    renderer::native::post(Box::new(move || {
                        pending.store(false, Ordering::Release);
                        link::drive_main();
                    }));
                }
            });
            let events = main.clone();
            renderer::native::on_window_event(move |window, event| {
                events.window_event(window, event);
            });
            let frames = main.clone();
            renderer::native::on_will_change(move |window, kind, bounds| {
                frames
                    .page(window)
                    .is_some_and(|id| frames.will_change(id, kind, bounds))
            });
            let keys = main.clone();
            renderer::native::on_key(move |window, input, composing| {
                keys.page(window)
                    .is_some_and(|id| keys.before_input(id, input, composing))
            });
            let reopen = main.clone();
            renderer::native::on_reopen(move |visible| {
                reopen.link().event(
                    format!("__appsEvent('activate',0,{visible})"),
                    "apps:activate",
                );
                reopen.drive();
            });
            let terminate = main.clone();
            renderer::native::on_should_quit(move || {
                let quit = if terminate.engine.is_none() || terminate.link().quitting() {
                    true
                } else {
                    match terminate.eval("__appsEvent('before-quit')", "apps:quit") {
                        Err(error) => {
                            eprintln!("apps: {error}");
                            true
                        }
                        Ok(_) => {
                            terminate.drive();
                            terminate.link().quitting()
                        }
                    }
                };
                if quit {
                    crate::jsc::stop_pages();
                }
                quit
            });
            let opened = main.clone();
            renderer::native::on_open_urls(move |list| {
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
                    opened.link().event(event, "apps:open");
                }
                opened.drive();
            });
            let host = main.host.clone();
            let exit = link.clone();
            renderer::native::on_quit(move || {
                if let Ok(host) = host.try_borrow()
                    && let Some(command) = &host.relaunch
                {
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
                    let _ = std::process::Command::new(&command[0])
                        .args(&command[1..])
                        .spawn();
                }
                // AppKit exits with 0 after this hook, so a failure code must exit here.
                if exit.exit_code() != 0 {
                    std::process::exit(exit.exit_code());
                }
            });
            crate::jsc::BEFORE_EXIT.set(Some(|| {
                crate::jsc::stop_pages();
                renderer::native::before_exit();
            }));
            #[cfg(target_os = "macos")]
            native::observe();
            if main.engine.is_some()
                && let Err(error) = main.eval("__appsEvent('ready')", "apps:ready")
            {
                eprintln!("apps: {error}");
                link.fail();
            }
            main.drive();
            if !renderer::THREAD_PER_APP {
                let (link, queue) = (link.clone(), link.shared_queue());
                renderer::spawn(move |cx| Self::start(template, link, queue, None, cx));
            }
            let first = main.first_window();
            if devtools {
                match first {
                    Some(id) => link.page(ToPage::Window(
                        id,
                        serde_json::json!({"op":"window","args":["devtools",id]}),
                    )),
                    None => link.main(ToMain::OpenTools),
                }
            }
            // The first main menu costs AppKit 20-30 ms, so the default menu waits for the first frame.
            #[cfg(target_os = "macos")]
            match first {
                Some(id) => link.page(ToPage::Window(
                    id,
                    serde_json::json!({"op":"window","args":["menuAfterFrame",id]}),
                )),
                None => link.event("__appsInitMenu()".into(), "apps:menu"),
            }
            renderer::native::activate(true);
            std::mem::forget(main);
        });
        Ok(exit.exit_code())
    }

    fn start(
        template: Arc<Template>,
        link: Link,
        queue: Arc<PageQueue>,
        owner: Option<u64>,
        cx: &mut App,
    ) {
        let api = match JavaScriptCore::load() {
            Ok(api) => api,
            Err(error) => {
                eprintln!("apps: {error:#}");
                link.fail();
                return;
            }
        };
        crate::jsc::prebuild(
            &api,
            template.disabled & crate::bundle::feature_bit("webgpu") == 0,
            !template.rust_main,
        );
        let waker = cx.waker();
        queue.set_wake(move || waker.wake());
        // Without a thread per window this one also runs the main side, which must wake too.
        let (main, page) = (crate::host::Waker::current(), queue.clone());
        crate::host::Waker::install(move || {
            main.wake();
            page.wake();
        });
        crate::jsc::NATIVE.set(Some(|op, args| {
            renderer::native::sync(|| native::native_call(op, args))
        }));
        mcp::install(owner.unwrap_or(0));
        let runtime = Rc::new(RefCell::new(Self {
            api,
            host: Rc::new(RefCell::new(template.host(&link))),
            main: None,
            link,
            queue,
            views: HashMap::new(),
            devtools: HashMap::new(),
            main_logs: Vec::new(),
            files: template.files.clone(),
            template,
            owner,
            ended: false,
            quit: false,
            wake: None,
            scheduled: false,
        }));
        let woken = Rc::downgrade(&runtime);
        cx.on_wake(move |cx| {
            if let Some(runtime) = woken.upgrade() {
                Self::drive(runtime, cx);
            }
        });
        PAGE_SIDE.with(|side| side.replace(Some(runtime.clone())));
        Self::drive(runtime, cx);
    }
}

fn scripted<R>(cx: Option<&mut App>, f: impl FnOnce() -> R) -> R {
    match cx {
        Some(cx) => crate::render::scripted(cx, None, f),
        None => f(),
    }
}

fn reset(host: &mut Host, id: u64) {
    host.graphics.remove(&id);
    host.media.remove(&id);
    host.dom.remove(&id);
    host.bridge.remove(&id);
    host.dropped.remove(&id);
}

fn shrink_when_quiet(runtime: Rc<RefCell<Desktop>>, cx: &mut App) {
    let api = runtime.borrow().api.clone();
    let frames = |runtime: &Rc<RefCell<Desktop>>, cx: &mut App| -> u64 {
        let Ok(app) = runtime.try_borrow() else {
            return 0;
        };
        app.views
            .values()
            .filter_map(|view| view.handle)
            .filter_map(|handle| cx.update_window(handle, |window, _| window.frames_drawn()))
            .sum()
    };
    let before = frames(&runtime, cx);
    cx.timer(Duration::from_secs(1), move |cx| {
        // Defer GC during animation: it blocks rendering. Allow caret-only frames so focused fields cannot
        // prevent GC.
        let drawing = frames(&runtime, cx).saturating_sub(before) > 4;
        if drawing || !api.shrink_due() {
            return shrink_when_quiet(runtime, cx);
        }
        api.idle.set(false);
        api.shrink();
        if let Ok(app) = runtime.try_borrow() {
            for view in app.views.values() {
                if let Some(page) = &view.renderer {
                    let _ = page.eval("__appsDomSweep?.(true)", "apps:trim");
                }
            }
            let host = &mut *app.host.borrow_mut();
            for (id, dom) in &mut host.dom {
                if let Some(page) = host.pages.get_mut(id)
                    && dom.trim(&mut page.doc)
                {
                    page.dirty = true;
                }
            }
            #[cfg(target_os = "macos")]
            crate::malloc::flush();
        }
        Desktop::drive(runtime, cx);
        cx.timer(Duration::from_secs(1), |_| {
            std::thread::spawn(|| {
                #[cfg(target_os = "macos")]
                {
                    unsafe extern "C" {
                        fn malloc_zone_pressure_relief(
                            zone: *mut std::ffi::c_void,
                            goal: usize,
                        ) -> usize;
                    }
                    // SAFETY: a null zone means all zones, and a goal of 0 means as much as possible.
                    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
                }
                #[cfg(all(target_os = "linux", target_env = "gnu"))]
                // SAFETY: it only returns free heap memory.
                unsafe {
                    libc::malloc_trim(0);
                }
            });
        })
        .detach();
    })
    .detach();
}

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
                    | "frame" | "transparent" | "hasShadow" | "focusable" | "modal"
                    | "skipTaskbar" => value.is_boolean(),
                    "title" | "icon" => value.is_string(),
                    "opacity" => number(value, 0., 1.),
                    "parent" => value.is_u64(),
                    "vibrancy" => number(value, 0., 22.),
                    "visualEffectState" => number(value, 0., 2.),
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
        | "shadow" | "contentProtection" | "focusable" | "edited" => {
            ensure!(args[2].is_boolean(), "Expected a boolean")
        }
        "alwaysOnTop" => ensure!(
            args[2]
                .as_i64()
                .is_some_and(|level| i32::try_from(level).is_ok()),
            "Expected a window level"
        ),
        "ignoreMouse" | "workspaces" => ensure!(
            args[2].is_boolean() && args[3].is_boolean(),
            "Expected booleans"
        ),
        "opacity" => ensure!(number(&args[2], 0., 1.), "Opacity must be between 0 and 1"),
        "aspectRatio" => ensure!(
            number(&args[2], 0., 1e6)
                && number(&args[3], 0., 16384.)
                && number(&args[4], 0., 16384.),
            "Invalid aspect ratio"
        ),
        "parent" => ensure!(
            args[2].is_null() || args[2].as_u64().is_some_and(|parent| parent != args[1]),
            "Invalid parent window"
        ),
        "vibrancy" => ensure!(
            (args[2].is_null() || number(&args[2], 0., 22.)) && number(&args[3], 0., 2.),
            "Invalid vibrancy"
        ),
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
        "zoom" => ensure!(
            number(&args[2], 0.25, 5.),
            "Zoom must be between 0.25 and 5"
        ),
        "insertCSS" => ensure!(
            args[2].is_string() && args[3].is_string() && args[4].is_boolean(),
            "Invalid CSS"
        ),
        "removeInsertedCSS" => ensure!(args[2].is_string(), "Invalid CSS key"),
        "edit" => ensure!(
            matches!(
                args[2].as_str(),
                Some(
                    "undo"
                        | "redo"
                        | "cut"
                        | "copy"
                        | "paste"
                        | "pasteAndMatchStyle"
                        | "delete"
                        | "selectAll"
                        | "unselect"
                        | "replace"
                        | "insertText"
                )
            ) && (args[3].is_null() || args[3].is_string()),
            "Invalid edit command"
        ),
        "input" | "willChange" => ensure!(args[2].is_boolean(), "Expected a boolean"),
        "deliver" => ensure!(
            matches!(
                args[2].as_str(),
                Some("webviewEvent" | "webviewReply" | "filesPicked" | "selectPicked")
            ) && args[3].is_array(),
            "Invalid page delivery"
        ),
        "executed" => bail!("Unsupported window action: executed"),
        _ => {}
    }
    Ok(())
}

fn png(
    ([width, height], pixels): ([u32; 2], Vec<u8>),
    scale: f32,
    rect: &serde_json::Value,
    css: bool,
) -> Result<([u32; 2], Vec<u8>)> {
    use tiny_skia as skia;
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
    Ok((
        [image.width(), image.height()],
        crate::render::image::encode_png(&image)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Desktop {
        fn js(&self) -> &JsContext {
            self.main.as_ref().unwrap().engine.as_ref().unwrap()
        }

        fn command(&self, command: serde_json::Value) {
            let args = &command["args"];
            if link::native_action(args[0].as_str().unwrap()) {
                self.link.main(ToMain::Native(command));
            } else {
                let id = args[1].as_u64().unwrap();
                self.link.page(ToPage::Window(id, command));
            }
        }

        fn commands(&self) -> Vec<serde_json::Value> {
            let mut commands = self.link.native_commands();
            for message in self.queue.take() {
                if let ToPage::Window(_, command) = &message {
                    commands.push(command["args"].clone());
                }
                self.link.page(message);
            }
            commands
        }
    }

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
        let mut desktop = Desktop::create(
            crate::bundle::Bundle {
                files,
                root,
                ..Default::default()
            },
            false,
        )
        .unwrap();
        desktop.js().eval("__appsEvent('ready')", "test").unwrap();
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
                desktop.js().eval(script, "test").unwrap();
                desktop.pump(None, None).unwrap();
            }
            desktop.js().tick().unwrap();
            assert_eq!(desktop.views[&1].preload.is_some(), preload, "{script}");
            assert_eq!(
                desktop.js().eval("pwned", "test").unwrap(),
                pwned,
                "{script}"
            );
        };
        step(&mut desktop, "", false, "0");
        desktop
            .js()
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
        let eval = |desktop: &Desktop, script: &str| desktop.js().eval(script, "test").unwrap();
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
        for engine in [desktop.js(), desktop.views[&1].renderer.as_deref().unwrap()] {
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
        assert!(desktop.quit && desktop.link.exit_code() == 1);
        desktop.link.quit(3);
        desktop.fail();
        assert_eq!(desktop.link.exit_code(), 3);
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
            desktop.js().eval("ids", "test").unwrap(),
            "1,1,2,true,2,true"
        );
        assert_eq!(
            desktop.js().eval("log.join('|')", "test").unwrap(),
            "destroyed|closed|true|true|1|Object has been destroyed|Object has been destroyed|Object has been destroyed|Object has been destroyed"
        );
        assert!(!desktop.views.contains_key(&1) && desktop.views.contains_key(&2));
        desktop.js().eval("other.destroy()", "test").unwrap();
        assert_eq!(
            desktop.js().eval("log.at(-1)", "test").unwrap(),
            "all-closed"
        );
    }

    #[test]
    fn page_navigation_stays_local_and_never_quits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.html");
        std::fs::write(&path, "<script type='application/json'>{}</script><script type='text/javascript'>globalThis.ran=1</script><body>ok</body>").unwrap();
        let mut desktop = app(
            "__apps.app.whenReady().then(()=>{globalThis.win=new __apps.BrowserWindow();win.load('<body>first</body>')})",
            &[],
            std::path::PathBuf::new(),
        );
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
        let code = "__apps.app.whenReady().then(()=>{try{new __apps.BrowserWindow({unknownOption:1})}catch(e){globalThis.rejected=e.message}globalThis.win=new __apps.BrowserWindow();win.close()})";
        let mut desktop = app(code, &[], std::path::PathBuf::new());
        assert_eq!(
            desktop.js()
                .eval("rejected+':'+__apps.BrowserWindow.getAllWindows().length+':'+win.isDestroyed()", "test")
                .unwrap(),
            "Unsupported BrowserWindow option: unknownOption:0:true"
        );
        assert!(desktop.js().eval("win.show()", "test").is_err());
        desktop.command(serde_json::json!({"op":"window","args":["title",2,"stale"]}));
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
        let eval = |desktop: &Desktop, script: &str| desktop.js().eval(script, "test").unwrap();
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
        page(1)
            .eval("__appsExecuted(2, 'own', false)", "test")
            .unwrap();
        desktop.pump(None, None).unwrap();
        desktop.js().tick().unwrap();
        assert_eq!(eval(&desktop, "results.join()"), "own");
        assert_eq!(eval(&desktop, "a.getTitle()"), "a\u{fffd}");
        eval(&desktop, "a.close(); b.close()");
        desktop.pump(None, None).unwrap();
        assert_eq!(eval(&desktop, "log.join('|')"), "closed failed|all-closed");
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
        desktop.js().eval("__appsEvent('ready')", "test").unwrap();
        desktop.js().eval("__appsEvent('state',1,{frame:[0,0,800,600],content:[0,28,800,572],minimized:false,maximized:false,fullscreen:false,visible:true,focused:true,resizing:false});__appsEvent('activate',0,true);__appsEvent('open-url',0,'myapp://x');__appsEvent('open-file',0,'/tmp/a');__appsEvent('before-quit')", "test").unwrap();
        desktop.pump(None, None).unwrap();
        assert!(!desktop.quit);
        desktop
            .js()
            .eval("__appsEvent('before-quit')", "test")
            .unwrap();
        desktop.pump(None, None).unwrap();
        assert!(desktop.quit);
        assert_eq!(
            desktop.js().eval("state", "test").unwrap(),
            "true,true,on,App"
        );
        assert_eq!(
            desktop.js().eval("log.join()", "test").unwrap(),
            "will-finish-launching,ready,web-contents-created,browser-window-created,browser-window-focus,activate:true,open-url:myapp://x,open-file:/tmp/a,before-quit,before-quit,will-quit,quit:0"
        );
        for (listener, quits) in [
            ("", true),
            ("__apps.app.on('window-all-closed',()=>{});", false),
        ] {
            let code = format!(
                "{listener}__apps.app.whenReady().then(()=>new __apps.BrowserWindow().close())"
            );
            let desktop = app(&code, &[], std::path::PathBuf::new());
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
        let mut desktop = app(code, &[], std::path::PathBuf::new());
        let eval = |desktop: &Desktop, script: &str| desktop.js().eval(script, "test").unwrap();
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
        desktop.queue.take();
        assert_eq!(
            eval(
                &desktop,
                "win.setSize(50.4, 50); win.setContentBounds({y: 100}); JSON.stringify([win.getBounds(), win.getContentBounds()])"
            ),
            r#"[{"x":0,"y":72,"width":200,"height":100},{"x":0,"y":100,"width":200,"height":72}]"#
        );
        assert_eq!(
            desktop.commands()[1],
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
        eval(&desktop, "__appsEvent('close',1)");
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
    fn window_extras_follow_electron() {
        let code = r#"
            const {app, BrowserWindow} = __apps;
            app.on('window-all-closed', () => {});
            app.whenReady().then(() => {
                globalThis.parent = new BrowserWindow({width: 400, height: 300, opacity: 3, vibrancy: 'sidebar', hasShadow: false, focusable: false, skipTaskbar: true, icon: 'icon.png'});
                globalThis.child = new BrowserWindow({parent, show: false});
                globalThis.sheet = new BrowserWindow({parent, modal: true});
            });
        "#;
        let mut desktop = app(code, &[], std::path::PathBuf::new());
        let eval = |desktop: &Desktop, script: &str| desktop.js().eval(script, "test").unwrap();
        let thrown = |desktop: &Desktop, script: &str| {
            eval(
                desktop,
                &format!(
                    "(() => {{ try {{ {script}; return 'ok' }} catch (error) {{ return error.message }} }})()"
                ),
            )
        };
        assert_eq!(
            eval(
                &desktop,
                "JSON.stringify([parent.getOpacity(), parent.hasShadow(), parent.isFocusable(), parent.getChildWindows().map(win => win.id), child.getParentWindow() === parent, sheet.isModal(), child.isModal(), parent.getParentWindow(), parent.setIcon])"
            ),
            "[1,false,false,[2,3],true,true,false,null,null]"
        );
        if cfg!(target_os = "macos") {
            assert_eq!(desktop.host.borrow().backgrounds[&1], (0, 0));
        }
        for (script, error) in [
            (
                "sheet.setParentWindow(null)",
                "Can not be called for modal window",
            ),
            (
                "parent.setParentWindow(child)",
                "A window cannot be its own ancestor",
            ),
            (
                "child.setParentWindow({})",
                "Expected a BrowserWindow or null",
            ),
            ("child.setOpacity(NaN)", "Expected a number for opacity"),
            ("child.setMenu({})", "Invalid Menu"),
            (
                "new __apps.BrowserWindow({vibrancy: 'nope'})",
                "Invalid vibrancy type: nope",
            ),
            (
                "new __apps.BrowserWindow({visualEffectState: 'x'})",
                "Invalid visualEffectState: x",
            ),
            (
                "new __apps.BrowserWindow({opacity: '1'})",
                "Invalid BrowserWindow option opacity: \"1\"",
            ),
            (
                "new __apps.BrowserWindow({parent: {}})",
                "Expected a BrowserWindow for parent",
            ),
        ] {
            assert_eq!(thrown(&desktop, script), error, "{script}");
        }
        let state = |frame: [i32; 4], extra: &str| {
            let [x, y, width, height] = frame;
            format!(
                "__appsEvent('state',1,{{frame:[{x},{y},{width},{height}],content:[{x},{y},{width},{height}],minimized:false,maximized:false,fullscreen:false,visible:true,focused:true,resizing:false,{extra}}})"
            )
        };
        eval(&desktop, &state([10, 20, 400, 300], ""));
        eval(&desktop, &state([0, 0, 900, 700], "maximized:true"));
        assert_eq!(
            eval(
                &desktop,
                "JSON.stringify([parent.getNormalBounds(), parent.getBounds()])"
            ),
            r#"[{"x":10,"y":20,"width":400,"height":300},{"x":0,"y":0,"width":900,"height":700}]"#
        );
        eval(&desktop, &state([0, 0, 900, 700], ""));
        desktop.queue.take();
        eval(
            &desktop,
            "parent.setOpacity(0.5); parent.setAspectRatio(-1, {width: 5}); parent.setVibrancy(''); parent.setSkipTaskbar(true); parent.removeMenu(); parent.close()",
        );
        let commands = desktop.commands();
        assert_eq!(
            serde_json::Value::from(commands),
            serde_json::json!([
                ["opacity", 1, 0.5],
                ["aspectRatio", 1, 0, 5, 0],
                ["parent", 2, null],
                ["parent", 3, null],
                ["close", 1],
                ["vibrancy", 1, null, 0],
                ["background", 1, "#fff"]
            ])
        );
        desktop.pump(None, None).unwrap();
        assert_eq!(
            eval(
                &desktop,
                "JSON.stringify([parent.isDestroyed(), child.isDestroyed(), child.getParentWindow()])"
            ),
            "[true,false,null]"
        );
        assert_eq!(
            eval(
                &desktop,
                "globalThis.seen = []; child.on('always-on-top-changed', (event, on) => seen.push(on)).on('will-resize', (event, bounds, details) => { seen.push(bounds.width, details.edge); event.preventDefault(); }); for (const args of [[true, 'screen-saver', 1], [true], [true, 'normal'], [false], [true, 'toString']]) child.setAlwaysOnTop(...args); child.setDocumentEdited(true); seen.push(__appsWillChange(2, ['right', 2, 2, 30, 40]), child.isDocumentEdited(), child.isAlwaysOnTop()); child.removeAllListeners('will-resize'); JSON.stringify(seen)"
            ),
            r#"[true,false,30,"right",true,true,false]"#
        );
        assert_eq!(
            serde_json::Value::from(desktop.commands()),
            serde_json::json!([
                ["willChange", 2, true],
                ["alwaysOnTop", 2, 1001],
                ["alwaysOnTop", 2, 3],
                ["alwaysOnTop", 2, 0],
                ["alwaysOnTop", 2, 0],
                ["alwaysOnTop", 2, 0],
                ["edited", 2, true],
                ["willChange", 2, false]
            ])
        );
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
            let mut desktop = app(&code, &[], std::path::PathBuf::new());
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
            desktop.command(serde_json::json!({
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

    #[test]
    fn web_contents_zoom_css_and_edit_commands() {
        let page = "<style>p{color:red;margin:1px}</style><p id=p>x</p><input id=i value=hello><div id=e contenteditable>ab</div>";
        let code = format!(
            "__apps.app.whenReady().then(()=>{{globalThis.win=new __apps.BrowserWindow();win.load({})}})",
            serde_json::json!(page)
        );
        let mut desktop = app(&code, &[], std::path::PathBuf::new());
        let main = |desktop: &mut Desktop, script: &str| {
            let value = desktop.js().eval(script, "test").unwrap();
            desktop.js().tick().unwrap();
            desktop.pump(None, None).unwrap();
            value
        };
        let page = |desktop: &Desktop, script: &str| {
            let renderer = desktop.views[&1].renderer.as_ref().unwrap();
            renderer.eval(script, "test").unwrap()
        };
        assert_eq!(
            main(
                &mut desktop,
                "win.webContents.zoomLevel=1;[win.webContents.getZoomFactor(),win.webContents.zoomLevel,(win.webContents.setZoomFactor(1.5),win.webContents.zoomFactor),win.webContents.getZoomLevel().toFixed(3)].join()"
            ),
            "1.2,1,1.5,2.224"
        );
        assert!(
            main(
                &mut desktop,
                "try{win.webContents.setZoomFactor(0)}catch(e){e.message}"
            )
            .contains("greater than 0")
        );
        let style = "(s=>[s.color,s.marginTop,s.paddingTop].join())(getComputedStyle(document.getElementById('p')))";
        main(
            &mut desktop,
            "win.webContents.insertCSS('p{color:blue!important;margin:2px}',{cssOrigin:'user'}).then(key=>globalThis.user=key);win.webContents.insertCSS('p{padding:3px}')",
        );
        assert_eq!(page(&desktop, style), "rgb(0, 0, 255),1px,3px");
        main(&mut desktop, "win.webContents.removeInsertedCSS(user)");
        assert_eq!(page(&desktop, style), "rgb(255, 0, 0),1px,3px");
        main(&mut desktop, "win.reload()");
        assert_eq!(page(&desktop, style), "rgb(255, 0, 0),1px,0px");
        let field =
            "(i=>[i.value,i.selectionStart,i.selectionEnd].join())(document.getElementById('i'))";
        page(
            &desktop,
            "document.getElementById('i').focus();document.getElementById('i').setSelectionRange(5,5)",
        );
        main(
            &mut desktop,
            "const contents=win.webContents;contents.insertText(' world');contents.selectAll();contents.replace('hi');contents.undo();contents.undo();contents.redo();contents.delete()",
        );
        assert_eq!(page(&desktop, field), "hello worl,10,10");
        main(&mut desktop, "contents.selectAll();contents.unselect()");
        assert_eq!(page(&desktop, field), "hello worl,10,10");
        assert_eq!(
            page(
                &desktop,
                "(i=>[document.queryCommandSupported('insertText'),document.queryCommandSupported('bold'),document.execCommand('insertText',false,'!'),i.value,document.execCommand('undo'),i.value,document.execCommand('redo'),i.value,document.queryCommandEnabled('copy'),document.execCommand('copy'),document.execCommand('delete'),i.value].join())(document.getElementById('i'))"
            ),
            "true,false,true,hello worl!,true,hello worl,true,hello worl!,false,false,true,hello worl"
        );
        assert_eq!(
            page(
                &desktop,
                "(e=>{e.focus();getSelection().collapse(e.firstChild,2);document.execCommand('insertText',false,'c');const typed=e.textContent;document.execCommand('undo');return typed+','+e.textContent})(document.getElementById('e'))"
            ),
            "abc,ab"
        );
    }

    #[test]
    fn window_open_asks_the_open_handler() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("child.html"), "<title>child</title>").unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<a id=a href='child.html' target='_blank'>x</a>",
        )
        .unwrap();
        let code = r#"
            globalThis.log = [];
            __apps.app.whenReady().then(() => {
                globalThis.win = new __apps.BrowserWindow();
                win.webContents.on('did-create-window', (child, details) => log.push('created:' + child.id + ':' + details.disposition + ':' + details.options.width));
                win.loadFile('index.html');
            });
        "#;
        let mut desktop = app(code, &[], dir.path().to_path_buf());
        let page = |desktop: &mut Desktop, script: &str| {
            let renderer = desktop.views[&1].renderer.clone().unwrap();
            let value = renderer.eval(script, "test").unwrap();
            desktop.pump(None, None).unwrap();
            value
        };
        assert_eq!(
            page(
                &mut desktop,
                "globalThis.child=open('child.html','','width=300,height=200');[open('https://example.com/'),child===null,child.closed].join()"
            ),
            ",false,false"
        );
        assert!(desktop.views[&2].renderer.is_some());
        desktop
            .js()
            .eval(
                "win.webContents.setWindowOpenHandler(details => { log.push([details.url.endsWith('/child.html'), details.frameName, details.features, details.disposition, details.referrer.url.endsWith('/index.html')].join()); return details.frameName === 'deny' ? { action: 'deny' } : { action: 'allow', overrideBrowserWindowOptions: { width: 320 } }; })",
                "test",
            )
            .unwrap();
        assert_eq!(
            page(
                &mut desktop,
                "[open('child.html','deny'),document.getElementById('a').click(),child.close()].join()"
            ),
            ",,"
        );
        assert!(desktop.views.contains_key(&3) && !desktop.views.contains_key(&2));
        assert_eq!(page(&mut desktop, "child.closed"), "true");
        assert_eq!(
            desktop.js().eval("log.join('|')", "test").unwrap(),
            "created:2:new-window:300|true,deny,,foreground-tab,true|true,_blank,,foreground-tab,true|created:3:foreground-tab:320"
        );
    }

    #[test]
    fn dialogs_of_main_and_pages_show_a_message_box() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<p>x</p>").unwrap();
        let code =
            "__apps.app.whenReady().then(() => new __apps.BrowserWindow().loadFile('index.html'));";
        let desktop = app(code, &[], dir.path().to_path_buf());
        crate::jsc::NATIVE.set(Some(|op, args| {
            assert_eq!(op, "dialog.message");
            let buttons = args["buttons"].as_array().map_or(0, Vec::len);
            Ok(serde_json::json!({
                "response": u8::from(args["message"] == "no"),
                "input": format!("{}:{buttons}", args["input"].as_str().unwrap_or("-")),
            }))
        }));
        let script = "[alert('hi'),confirm('yes'),confirm('no'),prompt('name','x'),prompt('no','x'),prompt()].join()";
        let page = desktop.views[&1].renderer.clone().unwrap();
        for engine in [&*page, desktop.js()] {
            assert_eq!(engine.eval(script, "test").unwrap(), ",true,false,x:2,,:2");
        }
    }

    #[test]
    fn main_runs_before_input_event_only_while_it_listens() {
        let code = "__apps.app.whenReady().then(()=>{globalThis.win=new __apps.BrowserWindow();globalThis.contents=win.webContents;globalThis.f=e=>e.preventDefault();globalThis.calls=0;const real=__appsBeforeInput;globalThis.__appsBeforeInput=(...args)=>(calls++,real(...args));win.load('<body></body>')})";
        let mut desktop = app(code, &[], std::path::PathBuf::new());
        let main = desktop.main.clone().unwrap();
        let down = renderer::Event::KeyDown {
            key: renderer::Key::default(),
            modifiers: renderer::Modifiers::NONE,
            repeat: false,
            ime: false,
        };
        let mut key = |script: &str| {
            desktop.js().eval(script, "test").unwrap();
            desktop.pump(None, None).unwrap();
            let prevented = main.before_input(1, &down, &|| false);
            match desktop
                .js()
                .eval("[calls,calls=0][0]", "test")
                .unwrap()
                .as_str()
            {
                "0" => "skipped",
                _ if prevented => "prevented",
                _ => "passed",
            }
        };
        assert_eq!(key("contents.on('other',f)"), "skipped");
        for method in [
            "on",
            "once",
            "addListener",
            "prependListener",
            "prependOnceListener",
        ] {
            assert_eq!(
                key(&format!("contents.{method}('before-input-event',f)")),
                "prevented",
                "{method}"
            );
            assert_eq!(
                key("contents.off('before-input-event',f)"),
                "skipped",
                "{method}"
            );
        }
        assert_eq!(key("contents.on('before-input-event',()=>{})"), "passed");
        assert_eq!(key("contents.on('before-input-event',f)"), "prevented");
        assert_eq!(
            key("contents.removeAllListeners('before-input-event')"),
            "skipped"
        );
        assert_eq!(key("contents.once('before-input-event',f)"), "prevented");
        assert_eq!(key(""), "skipped");
    }

    #[test]
    fn closed_windows_never_reach_main() {
        let page =
            "<script type='preload'>globalThis.send=()=>__apps.ipcRenderer.send('late')</script>";
        let code = "globalThis.late=[];__apps.ipcMain.on('late',e=>late.push(e.sender===null));__apps.app.whenReady().then(()=>{globalThis.win=new __apps.BrowserWindow();win.loadFile('apps:///page.html')})";
        let mut desktop = app(code, &[("page.html", page)], std::path::PathBuf::new());
        let preload = desktop.views[&1].preload.clone().unwrap();
        preload.eval("send()", "test").unwrap();
        desktop.pump(None, None).unwrap();
        desktop.js().tick().unwrap();
        desktop.js().eval("win.close()", "test").unwrap();
        desktop.main.as_ref().unwrap().receive();
        preload.eval("send()", "test").unwrap();
        desktop.pump(None, None).unwrap();
        desktop.js().tick().unwrap();
        assert_eq!(desktop.js().eval("late.join()", "test").unwrap(), "false");
        assert!(!desktop.views.contains_key(&1));
    }

    #[test]
    fn main_answers_pages_with_data_for_fixed_functions() {
        let code = "__apps.app.whenReady().then(()=>{globalThis.win=new __apps.BrowserWindow();win.load('<body></body>')})";
        let mut desktop = app(code, &[], std::path::PathBuf::new());
        let renderer = desktop.views[&1].renderer.clone().unwrap();
        renderer
            .eval(
                "globalThis.__appsFilesPicked=(...args)=>{globalThis.picked=args}",
                "test",
            )
            .unwrap();
        let files = serde_json::json!([7, [{"name":"a\");evil(\"","data":""}]]);
        desktop
            .command(serde_json::json!({"op":"window","args":["deliver",1,"filesPicked",files]}));
        desktop.pump(None, None).unwrap();
        assert_eq!(
            renderer.eval("JSON.stringify(picked)", "test").unwrap(),
            files.to_string()
        );
        for args in [
            serde_json::json!(["deliver", 1, "eval", []]),
            serde_json::json!(["deliver", 1, "filesPicked", "[7]"]),
            serde_json::json!(["input", 1, 1]),
        ] {
            assert!(validate(&args).is_err(), "{args}");
        }
    }
}
