pub mod app;
pub(crate) mod main;
pub(crate) mod mcp;
mod page;
pub(crate) mod panel;
#[cfg(test)]
mod tests;

use crate::bundle::Feature;
use crate::host::link::{Link, PageQueue, ToMain, ToPage};
use crate::{host::Host, host::JsContext, jsc::JavaScriptCore, jsc::Role};
use anyhow::{Context as _, Result, bail, ensure};
use bbtree::unsync::BBTreeMap;
use main::Main;
use renderer::{App, WindowChange, WindowId};
use std::{
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
    handle: Option<WindowId>,
    data: Weak<crate::render::PageState>,
    title: Option<String>,
    preload: Option<Rc<JsContext>>,
    renderer: Option<Rc<JsContext>>,
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

type Reloaded = (
    Arc<HashMap<String, String>>,
    Arc<HashMap<String, bytes::Bytes>>,
);

static RELOADED: std::sync::Mutex<Option<Reloaded>> = std::sync::Mutex::new(None);

pub struct Desktop {
    api: Rc<JavaScriptCore>,
    host: Rc<RefCell<Host>>,
    main: Option<Rc<Main>>,
    link: Link,
    queue: Arc<PageQueue>,
    views: HashMap<u64, View>,
    devtools: HashMap<u64, panel::DevTools>,
    main_logs: Vec<serde_json::Value>,
    files: Arc<HashMap<String, String>>,
    template: Arc<Template>,
    owner: Option<u64>,
    ended: bool,
    quit: bool,
    wake: Option<(Instant, renderer::Timer)>,
    scheduled: bool,
}

// The page views reach their desktop through this.
struct Pages(Rc<RefCell<Desktop>>);

impl crate::render::Pages for Pages {
    fn renderer(&self, id: u64) -> Option<Rc<JsContext>> {
        let desktop = self.0.try_borrow().ok()?;
        desktop.views.get(&id)?.renderer.clone()
    }

    fn dispatch(
        &self,
        id: u64,
        name: &str,
        path: &[usize],
        data: serde_json::Value,
        window: Option<&mut renderer::Window>,
        cx: &mut App,
    ) -> bool {
        Desktop::dispatch(self.0.clone(), id, name, path, data, window, cx)
    }

    fn drive(&self, cx: &mut App) {
        Desktop::drive(self.0.clone(), cx);
    }
}

thread_local! {
    // Skip destruction: this needs HOSTS, which exits first.
    static PAGE_SIDE: std::mem::ManuallyDrop<RefCell<Option<Rc<RefCell<Desktop>>>>> = const { std::mem::ManuallyDrop::new(RefCell::new(None)) };
}

struct Template {
    app: crate::bundle::AppInfo,
    files: Arc<HashMap<String, String>>,
    packed: Arc<HashMap<String, bytes::Bytes>>,
}

impl Template {
    fn host(&self, link: &Link) -> Host {
        Host {
            link: Some(link.clone()),
            app: self.app.clone(),
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

    pub(crate) fn create(mut bundle: crate::bundle::Bundle, rust: bool) -> Result<Self> {
        let package =
            crate::bundle::Package::parse(bundle.files.get("package.json").map(String::as_str))?;
        let app = crate::bundle::AppInfo::new(&mut bundle, &package, rust)?;
        let crate::bundle::Bundle {
            files, pages, main, ..
        } = bundle;
        let files = &files;
        let windows = !pages.is_empty() || files.keys().any(|name| name.ends_with(".html"));
        // Preload Metal, the window server and CoreText here, while JavaScriptCore loads, to avoid delays at
        // launch and the first frame.
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
                    renderer::native::prewarm();
                    unsafe { CGMainDisplayID() };
                    let text = TextSystem::new();
                    let run = TextRun {
                        len: 2,
                        font: Font::new(".SystemUIFont"),
                        ..Default::default()
                    };
                    let line = text.shape_line("Ag", renderer::px(13.), &run);
                    if let Some(glyph) = line.line.glyphs.first() {
                        text.glyph_pixels(glyph.font, glyph.id, glyph.size, 2., 0, false, 0.);
                    }
                    text.face(&Font {
                        weight: 600.,
                        ..Font::new(".SystemUIFont")
                    });
                    text.face(&Font::new("Menlo"));
                    text.face(&Font::new("Helvetica"));
                });
        }
        let api = JavaScriptCore::load()?;
        let link = Link::new();
        // Install the waker before the app runs, because native producers of this thread capture it then.
        crate::jsc::Waker::install({
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
        if windows {
            api.prebuild(app.features.has(Feature::WebGpu), !rust);
        }
        crate::web::LOCAL_NETWORK.store(app.features.has(Feature::LocalNetwork), Ordering::Relaxed);
        let template = Arc::new(Template {
            app,
            files: Arc::new(
                files
                    .iter()
                    .filter(|(name, _)| name.ends_with(".html"))
                    .map(|(name, source)| (name.clone(), source.clone()))
                    .collect(),
            ),
            packed: Arc::new(pages),
        });
        let host = template.host(&link);
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
                                if let Some(url) = host.url(id)
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
                    "create" => self.open(id, args, cx.as_deref_mut(), runtime)?,
                    "load" | "loadFile" => self.load(id, action, args, cx.as_deref_mut())?,
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
                    "devtools" | "hud" | "inspect" => {
                        self.tools(action, id, args, cx.as_deref_mut())?
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
                            self.host.borrow_mut().close_page(id);
                            crate::web::storage::forget(id);
                            crate::host::page_stopped(id);
                            self.ended = self.owner == Some(id);
                            if self.ended {
                                crate::devtools::uninstall();
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
                                        link.call("__appsInitMenu", Vec::new(), "apps:menu")
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
                                        link.event("captured", id, Some(data), "apps:capture");
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
                                "executed",
                                id,
                                Some(serde_json::json!({"request":request,"data":data})),
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
                        self.link
                            .event("html-full-screen", id, Some(on.into()), "apps:fullscreen");
                    }
                    "background" => {
                        let color = args[2]
                            .as_str()
                            .and_then(crate::style::color)
                            .unwrap_or((0xffffff, 255));
                        self.host.borrow_mut().set_background(id, color);
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
            if !self.host.borrow().app.rust_main {
                self.link
                    .event("title", id, Some(text.into()), "apps:title");
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
        crate::jsc::idle(false);
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
                let size = serde_json::json!({"width": (width * zoom).round(), "height": (height * zoom).round()});
                app.link.event("resize", id, Some(size), "apps:resize");
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
                let focused = focused || crate::devtools::enabled();
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
                            "html-full-screen",
                            id,
                            Some(false.into()),
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
                        panel::release(engine)?;
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
            let mcp = crate::devtools::enabled();
            if !app.devtools.is_empty() || mcp {
                let logs: Vec<_> = app.host.borrow_mut().logs.drain(..).collect();
                let main_logs = std::mem::take(&mut app.main_logs);
                for tools in app.devtools.values() {
                    tools.events(&logs)?;
                    tools.events(&main_logs)?;
                }
                crate::devtools::logs(&logs);
            }
            // Page scripts that the tools run can force a layout, like other page tasks.
            let raw: *mut App = cx;
            // SAFETY: a forced layout uses `cx` only inside a script call, while `poll` waits for it.
            if mcp
                && crate::render::scripted(unsafe { &mut *raw }, None, || {
                    mcp::poll(&app, unsafe { &mut *raw })
                })
            {
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
        if api.schedule_shrink() {
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
        crate::jsc::idle(true);
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
            !(devtools || mcp.is_some()) || !self.host.borrow().app.packaged,
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
                        main::drive_main();
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
                reopen
                    .link()
                    .event("activate", 0, Some(visible.into()), "apps:activate");
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
                    crate::host::stop_pages();
                }
                quit
            });
            let opened = main.clone();
            renderer::native::on_open_urls(move |list| {
                for url in list {
                    let (name, data) = match url::Url::parse(&url)
                        .ok()
                        .filter(|url| url.scheme() == "file")
                        .and_then(|url| url.to_file_path().ok())
                    {
                        Some(path) => ("open-file", serde_json::json!(path)),
                        None => ("open-url", serde_json::json!(url)),
                    };
                    opened.link().event(name, 0, Some(data), "apps:open");
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
            crate::host::BEFORE_EXIT.set(Some(|| {
                crate::host::stop_pages();
                renderer::native::before_exit();
            }));
            #[cfg(target_os = "macos")]
            crate::native::observe();
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
                None => link.call("__appsInitMenu", Vec::new(), "apps:menu"),
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
        api.prebuild(
            template.app.features.has(Feature::WebGpu),
            !template.app.rust_main,
        );
        let waker = cx.waker();
        queue.set_wake(move || waker.wake());
        // Without a thread per window this one also runs the main side, which must wake too.
        let (main, page) = (crate::jsc::Waker::current(), queue.clone());
        crate::jsc::Waker::install(move || {
            main.wake();
            page.wake();
        });
        crate::host::NATIVE.set(Some(|op, args| {
            renderer::native::sync(|| crate::native::native_call(op, args))
        }));
        crate::devtools::install(owner.unwrap_or(0));
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
