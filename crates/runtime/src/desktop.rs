use crate::{
    host::{Host, Page, Role},
    html::Document,
    jsc::{Api, Engine},
    render::{self, ComputedStyles},
};
use anyhow::{Context as _, Result, bail, ensure};
use bbtree::unsync::BBTreeMap;
use gpui::{
    AnyElement, App, Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    SharedString, TitlebarOptions, UTF16Selection, Window, WindowBounds, WindowHandle,
    WindowOptions, div, img, point, prelude::*, px, relative, rgba, size,
};
use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc, sync::Arc, time::Duration};

#[cfg(target_os = "macos")]
gpui::actions!(desktop, [Quit]);

struct View {
    handle: Option<WindowHandle<WebView>>,
    title: Option<String>,
    preload: Option<Rc<Engine>>,
    renderer: Option<Rc<Engine>>,
}
struct WebView {
    id: u64,
    runtime: Rc<RefCell<Desktop>>,
    host: Rc<RefCell<Host>>,
    images: HashMap<usize, Arc<gpui::RenderImage>>,
    focus: FocusHandle,
    browsers: HashMap<usize, (crate::browser::Browser, String)>,
}

pub struct Desktop {
    api: Rc<Api>,
    host: Rc<RefCell<Host>>,
    main: Engine,
    views: HashMap<u64, View>,
    devtools: HashMap<u64, crate::devtools::DevTools>,
    files: BBTreeMap<String, String>,
    pages: BBTreeMap<String, Document>,
    error: Option<anyhow::Error>,
    quit: bool,
    wake: Option<gpui::Task<()>>,
}

impl Desktop {
    pub fn from_bundle(bundle: crate::bundle::Bundle) -> Result<Self> {
        let mut desktop = Self::create(&bundle.files, bundle.packaged)?;
        desktop.pages = bundle.pages;
        Ok(desktop)
    }

    fn dispatch(
        runtime: Rc<RefCell<Self>>,
        id: u64,
        name: &str,
        path: &[usize],
        data: serde_json::Value,
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
        let result = renderer.eval(
            &format!(
                "__appsDispatch({},{},{})",
                serde_json::json!(name),
                serde_json::json!(path),
                data
            ),
            "apps:input",
        );
        let allowed = result.as_ref().is_ok_and(|value| value == "true");
        if let Err(error) = result {
            eprintln!("apps: {error}");
        }
        cx.defer(move |cx| Self::drive(runtime, cx));
        allowed
    }

    pub fn new(files: &BBTreeMap<String, String>) -> Result<Self> {
        Self::create(files, false)
    }

    fn create(files: &BBTreeMap<String, String>, packaged: bool) -> Result<Self> {
        let api = Api::load()?;
        let (_, name, version) =
            crate::bundle::package_metadata(files.get("package.json").map(String::as_str))?;
        let host = Rc::new(RefCell::new(Host {
            app_version: Some(version),
            app_name: Some(name.clone()),
            packaged,
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
        })
    }

    fn pump(
        &mut self,
        mut cx: Option<&mut App>,
        runtime: Option<&Rc<RefCell<Self>>>,
    ) -> Result<()> {
        for _ in 0..1024 {
            let commands = std::mem::take(&mut self.host.borrow_mut().commands);
            if commands.is_empty() {
                for (&id, view) in &mut self.views {
                    let host = self.host.borrow();
                    let Some(page) = host.pages.get(&id).filter(|page| page.dirty) else {
                        continue;
                    };
                    let title = page.doc.title();
                    if view.title != title {
                        if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                            handle.update(cx, |_, window, _| {
                                window.set_window_title(title.as_deref().unwrap_or(""))
                            })?;
                        }
                        view.title = title;
                    }
                }
                let attaches = self.host.borrow_mut().browsers.take_attaches();
                for (browser, parent) in attaches {
                    if let Some(cx) = cx.as_deref_mut()
                        && let Some(handle) = self.views.get(&parent).and_then(|view| view.handle)
                    {
                        handle.update(cx, |_, window, _| {
                            self.host.borrow().browsers.attach(browser, window)
                        })??;
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
                let action = args[0].as_str().context("Missing window action")?;
                let id = args[1].as_u64().context("Missing window id")?;
                if action != "create" && !self.views.contains_key(&id) {
                    continue;
                }
                match action {
                    "create" => {
                        ensure!(!self.views.contains_key(&id), "Window already exists");
                        let options = &args[2];
                        let width = options["width"].as_f64().unwrap_or(800.) as f32;
                        let height = options["height"].as_f64().unwrap_or(600.) as f32;
                        self.host.borrow_mut().viewports.insert(id, [width, height]);
                        let handle = if let Some(cx) = cx.as_deref_mut() {
                            let runtime = runtime.unwrap().clone();
                            let close_runtime = runtime.clone();
                            let host = self.host.clone();
                            let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
                            Some(cx.open_window(WindowOptions{window_bounds:Some(WindowBounds::Windowed(bounds)),titlebar:Some(TitlebarOptions{title:Some(options["title"].as_str().unwrap_or("apps").to_owned().into()),..Default::default()}),show:options["show"]!=false,is_resizable:options["resizable"]!=false,..Default::default()},move|window,cx|{
                                window.on_window_should_close(cx,move|_,cx|{let runtime=close_runtime.clone();cx.defer(move|cx|{runtime.borrow().host.borrow_mut().commands.push(serde_json::json!({"op":"window","args":["close",id]}));Desktop::drive(runtime,cx);});false});
                                cx.new(|cx|{let focus = cx.focus_handle(); window.focus(&focus, cx); WebView{id,runtime,host,images:HashMap::new(),focus,browsers:HashMap::new()}})
                            })?)
                        } else {
                            None
                        };
                        self.views.insert(
                            id,
                            View {
                                handle,
                                title: None,
                                preload: None,
                                renderer: None,
                            },
                        );
                    }
                    "loadURL" => {
                        let url = args[2].as_str().context("Expected URL")?.to_owned();
                        let view = self.views.get(&id).context("Window is closed")?;
                        if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                            handle.update(cx, |view, window, cx| -> Result<()> {
                                let browser = crate::browser::Browser::new(
                                    window,
                                    &serde_json::json!({"persistent":true,"inspectable":!view.host.borrow().packaged,"application":view.host.borrow().app_name}),
                                )?;
                                let size = window.viewport_size();
                                browser.set_bounds([
                                    0.,
                                    0.,
                                    f32::from(size.width) as f64,
                                    f32::from(size.height) as f64,
                                ])?;
                                browser.load_url(&url)?;
                                view.browsers.insert(usize::MAX, (browser, url));
                                cx.notify();
                                Ok(())
                            })??;
                        }
                    }
                    "load" | "loadFile" => {
                        self.host.borrow_mut().graphics.remove(&id);
                        let value = args[2].as_str().context("Expected HTML text or path")?;
                        let doc = if action == "load" {
                            Document::parse(value)
                        } else if let Some(name) = value.strip_prefix("apps:///") {
                            let name = name.split(['?', '#']).next().unwrap();
                            if let Some(doc) = self.pages.get(name) {
                                doc.clone()
                            } else {
                                Document::parse(
                                    self.files.get(name).context("Missing bundled HTML file")?,
                                )
                            }
                        } else {
                            let path = if value.starts_with("file:") {
                                url::Url::parse(value)?
                                    .to_file_path()
                                    .map_err(|_| anyhow::anyhow!("Invalid file URL"))?
                            } else {
                                value.into()
                            };
                            let source = std::fs::read_to_string(&path)
                                .with_context(|| format!("Cannot read {}", path.display()))?;
                            Document::parse(&source)
                        };
                        let scripts: Vec<_> = doc
                            .nodes
                            .iter()
                            .filter(|n| n.tag == "script")
                            .map(|n| {
                                (
                                    n.attrs
                                        .get("type")
                                        .cloned()
                                        .unwrap_or_else(|| "module".into()),
                                    n.text.clone(),
                                )
                            })
                            .collect();
                        self.host
                            .borrow_mut()
                            .pages
                            .insert(id, Page { doc, dirty: true });
                        self.host.borrow_mut().dom.remove(&id);
                        let view = self.views.get_mut(&id).context("Window is closed")?;
                        if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                            handle.update(cx, |_, window, _| {
                                let mut host = self.host.borrow_mut();
                                let state = host.dom.entry(id).or_default();
                                state.text_system = Some(window.text_system().clone());
                                state.pixel_ratio = window.scale_factor();
                            })?;
                        }
                        view.preload = None;
                        view.renderer = None;
                        self.host.borrow_mut().bridge.remove(&id);
                        let trusted = args[3] != "page" || value.starts_with("apps:");
                        let preload = (trusted
                            && scripts.iter().any(|(kind, _)| kind == "preload"))
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
                            if kind == "preload"
                                && let Some(preload) = &preload
                            {
                                preload.eval(script, "apps:preload")?;
                            }
                        }
                        let location = if action == "load" {
                            "apps://app/".to_owned()
                        } else if ["apps:", "file:", "http:", "https:"]
                            .iter()
                            .any(|scheme| value.starts_with(scheme))
                        {
                            value.to_owned()
                        } else {
                            url::Url::from_file_path(std::path::absolute(value)?)
                                .map_err(|_| anyhow::anyhow!("Invalid page path"))?
                                .to_string()
                        };
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
                        for (kind, script) in &scripts {
                            if kind == "module" {
                                if let Err(error) = renderer.eval(script, "apps:page") {
                                    if cx.is_none() {
                                        return Err(error);
                                    }
                                    eprintln!("apps: page script: {error:#}");
                                }
                            } else {
                                ensure!(kind == "preload", "Unsupported script type: {kind}");
                            }
                        }
                        view.preload = preload;
                        view.renderer = Some(renderer);
                        view.title = self.host.borrow().pages[&id].doc.title();
                        if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                            handle.update(cx, |_, window, cx| {
                                if let Some(title) = &view.title {
                                    window.set_window_title(title);
                                }
                                cx.notify();
                            })?;
                        }
                        self.main
                            .eval(&format!("__appsEvent('loaded',{id})"), "apps:loaded")?;
                    }
                    "devtools" => {
                        if self.host.borrow().packaged {
                            continue;
                        }
                        ensure!(cx.is_some(), "Developer Tools require a native window");
                        if self.devtools.remove(&id).is_none() {
                            let view = self.views.get(&id).context("Window is closed")?;
                            if view.renderer.is_none() {
                                if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                                    handle.update(cx, |view, _, _| {
                                        view.browsers
                                            .get(&usize::MAX)
                                            .context("Window has no document")?
                                            .0
                                            .inspect()
                                    })??;
                                }
                            } else {
                                self.devtools.insert(
                                    id,
                                    crate::devtools::DevTools::new(&format!("Window {id}"))?,
                                );
                            }
                        }
                    }
                    "close" => {
                        self.devtools.remove(&id);
                        if let Some(view) = self.views.remove(&id) {
                            self.host.borrow_mut().browsers.close_window(id)?;
                            if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                                handle.update(cx, |view, window, _| {
                                    view.browsers.clear();
                                    window.remove_window();
                                })?;
                            }
                            self.host.borrow_mut().pages.remove(&id);
                            self.host.borrow_mut().graphics.remove(&id);
                            self.host.borrow_mut().viewports.remove(&id);
                            self.host.borrow_mut().dom.remove(&id);
                            self.host.borrow_mut().bridge.remove(&id);
                            self.main
                                .eval(&format!("__appsEvent('closed',{id})"), "apps:closed")?;
                        }
                    }
                    "show" | "title" => {
                        let view = self.views.get(&id).context("Window is closed")?;
                        if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
                            handle.update(cx, |_, window, _| {
                                if action == "title" {
                                    window.set_window_title(args[2].as_str().unwrap_or(""));
                                } else {
                                    window.activate_window();
                                }
                            })?;
                        }
                    }
                    _ => bail!("Unsupported window action: {action}"),
                }
            }
        }
        bail!("Too many window commands in one turn")
    }

    fn drive(runtime: Rc<RefCell<Self>>, cx: &mut App) {
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
            let mut delay = i64::MAX;
            for (fatal, engine) in
                std::iter::once((true, &app.main)).chain(app.views.values().flat_map(|v| {
                    v.preload
                        .iter()
                        .chain(v.renderer.iter())
                        .map(|engine| (false, engine.as_ref()))
                }))
            {
                let next = match engine.tick() {
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
            app.pump(Some(cx), Some(&runtime))?;
            if app.quit {
                return Ok(None);
            }
            if std::mem::take(&mut app.host.borrow_mut().theme_changed) {
                delay = delay.min(0);
                for view in app.views.values() {
                    if let Some(renderer) = &view.renderer
                        && let Err(error) = renderer.eval("__appsMediaChanged()", "apps:theme")
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
                    match crate::devtools::dispatch(&command, engine, &app.host, id) {
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
            if !app.devtools.is_empty() {
                let logs: Vec<_> = app.host.borrow_mut().logs.drain(..).collect();
                for tools in app.devtools.values() {
                    tools.events(&logs)?;
                }
                delay = delay.min(16);
            }
            app.pump(Some(cx), Some(&runtime))?;
            if app.quit {
                return Ok(None);
            }
            for (&id, view) in &app.views {
                if let Some(handle) = view.handle {
                    let (active, events) = handle.update(cx, |view, _, _| {
                        (
                            !view.browsers.is_empty(),
                            view.browsers
                                .iter()
                                .flat_map(|(&node, (browser, _))| {
                                    browser.poll().into_iter().map(move |event| (node, event))
                                })
                                .collect::<Vec<_>>(),
                        )
                    })?;
                    if active {
                        delay = delay.min(16);
                    }
                    for (node, event) in events {
                        if node == usize::MAX {
                            if event["type"] == "did-finish-load" {
                                app.main
                                    .eval(&format!("__appsEvent('loaded',{id})"), "apps:browser")?;
                            }
                        } else if let Some(renderer) = &view.renderer {
                            renderer.eval(
                                &format!("__appsFrameEvent({node},{event})"),
                                "apps:iframe",
                            )?;
                        }
                    }
                }
            }
            for (&id, view) in &app.views {
                let dirty = app.host.borrow().pages.get(&id).is_some_and(|p| p.dirty);
                if dirty && let Some(handle) = view.handle {
                    handle.update(cx, |_, _, cx| cx.notify())?;
                }
            }
            Ok(if delay == i64::MAX || app.quit {
                None
            } else {
                Some(delay.max(1) as u64)
            })
        })();
        match result {
            Ok(Some(delay)) => {
                let wake = runtime.clone();
                let task = cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(delay))
                        .await;
                    cx.update(|cx| Self::drive(wake, cx));
                });
                runtime.borrow_mut().wake = Some(task);
            }
            Ok(None) => runtime.borrow_mut().wake = None,
            Err(error) => {
                eprintln!("apps: {error:#}");
                runtime.borrow_mut().error = Some(error);
                cx.quit();
            }
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

    pub fn run(self, devtools: bool) -> Result<i32> {
        ensure!(
            !devtools || !self.host.borrow().packaged,
            "Developer tools are unavailable in packaged apps"
        );
        let runtime = Rc::new(RefCell::new(self));
        let result = runtime.clone();
        let reopen = runtime.clone();
        let application = gpui_platform::application();
        application.on_reopen(move |cx| {
            if let Err(error) = reopen
                .borrow()
                .main
                .eval("__appsEvent('activate')", "apps:activate")
            {
                eprintln!("apps: {error}");
            }
            Self::drive(reopen.clone(), cx);
        });
        application.run(move |cx| {
            #[cfg(target_os = "macos")]
            {
                let mut changes = crate::browser::observe_theme();
                let weak = Rc::downgrade(&runtime);
                cx.spawn(async move |cx| {
                    while changes.recv().await.is_some() {
                        let Some(runtime) = weak.upgrade() else { break };
                        cx.update(|cx| Self::drive(runtime, cx));
                    }
                })
                .detach();
                let runtime = runtime.clone();
                cx.on_action(move |_: &Quit, cx| {
                    runtime.borrow_mut().quit = true;
                    cx.quit();
                });
                cx.bind_keys([gpui::KeyBinding::new("cmd-q", Quit, None)]);
            }
            let startup = if cfg!(target_os = "macos") {
                "__appsInitMenu();__appsEvent('ready')"
            } else {
                "__appsEvent('ready')"
            };
            if let Err(error) = runtime.borrow().main.eval(startup, "apps:ready") {
                eprintln!("apps: {error}");
                cx.quit();
                return;
            }
            Self::drive(runtime.clone(), cx);
            if devtools && !runtime.borrow().quit {
                let mut app = runtime.borrow_mut();
                let id = app.views.keys().copied().min().unwrap_or(0);
                let title = if id == 0 {
                    "Main".to_owned()
                } else {
                    format!("Window {id}")
                };
                match crate::devtools::DevTools::new(&title) {
                    Ok(tools) => {
                        app.devtools.insert(id, tools);
                    }
                    Err(error) => {
                        app.error = Some(error);
                        cx.quit();
                        return;
                    }
                }
                drop(app);
                Self::drive(runtime, cx);
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

impl Render for WebView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let runtime = self.runtime.clone();
        let id = self.id;
        let frame_requested = {
            let mut host = self.host.borrow_mut();
            let state = host.dom.entry(id).or_default();
            state.text_system = Some(window.text_system().clone());
            state.pixel_ratio = window.scale_factor();
            !state.container_pass && std::mem::take(&mut state.frame_requested)
        };
        if frame_requested && let Err(error) = self.evaluate("__appsAnimationFrame()") {
            eprintln!("apps: {error}");
        }
        if self
            .host
            .borrow()
            .dom
            .get(&id)
            .is_some_and(|state| state.frame_requested)
        {
            let entity = cx.entity();
            window.on_next_frame(move |_, cx| entity.update(cx, |_, cx| cx.notify()));
        }
        let pointer_events = gpui::canvas(|_, _, _| (), move |_, _, window, _| {
        let modifier_runtime = runtime.clone();
        window.on_modifiers_changed(move |event, _, cx| {
            let modifiers = &event.modifiers;
            Desktop::dispatch(modifier_runtime.clone(), id, "modifiers", &[], serde_json::json!({"ctrlKey":modifiers.control,"metaKey":modifiers.platform,"altKey":modifiers.alt,"shiftKey":modifiers.shift}), cx);
        });
        let exit_runtime = runtime.clone();
        window.on_mouse_event(move |event: &gpui::MouseExitEvent, phase, _, cx| {
            if phase.capture() {
                Desktop::dispatch(exit_runtime.clone(), id, "pointerexit", &[], serde_json::json!({"pointerId":1,"pointerType":"mouse","isPrimary":true,"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y)}), cx);
            }
        });
        let up_runtime = runtime.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase.capture() {
                let data = serde_json::json!({"pointerId":1,"pointerType":"mouse","isPrimary":true,"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y),"button":-1,"buttons":u8::from(event.dragging()),"ctrlKey":event.modifiers.control,"metaKey":event.modifiers.platform,"altKey":event.modifiers.alt,"shiftKey":event.modifiers.shift});
                Desktop::dispatch(runtime.clone(), id, "pointermove", &[], data, cx);
            }
        });
        let runtime = up_runtime;
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase.capture() && event.button == MouseButton::Left {
                Desktop::dispatch(runtime.clone(), id, "pointerup", &[], serde_json::json!({"pointerId":1,"pointerType":"mouse","isPrimary":true,"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y),"button":0,"buttons":0,"ctrlKey":event.modifiers.control,"metaKey":event.modifiers.platform,"altKey":event.modifiers.alt,"shiftKey":event.modifiers.shift}), cx);
            }
        });
        }).absolute().size_full();
        let entity = cx.entity();
        let focus = self.focus.clone();
        let input = gpui::canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                window.handle_input(&focus, ElementInputHandler::new(bounds, entity.clone()), cx);
            },
        )
        .absolute()
        .size_full();
        let mut host = self.host.borrow_mut();
        let size = window.viewport_size();
        let width = f32::from(size.width);
        let height = f32::from(size.height);
        let resized = host.viewports.insert(self.id, [width, height]) != Some([width, height]);
        if resized {
            let runtime = self.runtime.clone();
            let id = self.id;
            cx.defer(move |cx| {
                if let Err(error) = runtime.borrow().main.eval(
                    &format!(
                        "__appsEvent('resize',{id},{{width:{},height:{}}})",
                        width.round(),
                        height.round()
                    ),
                    "apps:resize",
                ) {
                    eprintln!("apps: {error}");
                }
                if let Some(renderer) = runtime
                    .borrow()
                    .views
                    .get(&id)
                    .and_then(|v| v.renderer.as_ref())
                    && let Err(error) =
                        renderer.eval("__appsDispatch('window:resize',[0])", "apps:resize")
                {
                    eprintln!("apps: {error}");
                }
                Desktop::drive(runtime, cx);
            });
        }
        let mut images = HashMap::new();
        let canvases: Vec<_> = host
            .pages
            .get(&self.id)
            .map(|p| {
                p.doc
                    .nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.tag == "canvas")
                    .map(|(id, _)| id)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(gpu) = host.graphics.get_mut(&self.id) {
            for id in canvases {
                match gpu.image(id) {
                    Ok(Some(image)) => {
                        images.insert(id, image);
                    }
                    Err(error) => eprintln!("apps: canvas presentation failed: {error}"),
                    _ => {}
                }
            }
        }
        for (id, old) in std::mem::replace(&mut self.images, images) {
            if self.images.get(&id).is_none_or(|image| image.id != old.id) {
                let _ = window.drop_image(old);
            }
        }
        if let Some((browser, _)) = self.browsers.get(&usize::MAX) {
            let _ = browser.set_bounds([0., 0., width as f64, height as f64]);
        }
        let inspectable = !host.packaged;
        let Some(page) = host.pages.get_mut(&self.id) else {
            return div().size_full().into_any_element();
        };
        page.dirty = false;
        let mut doc = std::mem::take(&mut page.doc);
        let dom = host.dom.entry(self.id).or_default();
        let mut styles = crate::dom::styles(&mut doc, dom, [width, height], Some(cx.text_system()));
        let children: Vec<_> = styles
            .children
            .iter()
            .map(|(&node, children)| {
                (
                    node,
                    std::mem::replace(&mut doc.nodes[node].children, children.clone()),
                )
            })
            .collect();
        let mut visible = std::collections::HashSet::new();
        let mut pending = vec![0];
        while let Some(node) = pending.pop() {
            if !styles.values[node]
                .get("display")
                .is_some_and(|value| value == "none")
            {
                visible.insert(node);
                pending.extend(&doc.nodes[node].children);
            }
        }
        let mut events = Vec::new();
        let (animated, running) = dom.motion.update(
            &doc,
            &styles,
            &visible,
            std::time::Instant::now(),
            dom.container_pass,
            &mut events,
        );
        if running {
            let entity = cx.entity();
            window.on_next_frame(move |_, cx| entity.update(cx, |_, cx| cx.notify()));
        }
        if !events.is_empty() {
            let runtime = self.runtime.clone();
            let id = self.id;
            cx.defer(move |cx| {
                let renderer = runtime
                    .borrow()
                    .views
                    .get(&id)
                    .and_then(|view| view.renderer.clone());
                for (node, kind, name, elapsed) in events {
                    let data = if kind == "animationend" {
                        serde_json::json!({"animationName": name, "elapsedTime": elapsed})
                    } else {
                        serde_json::json!({"propertyName": name, "elapsedTime": elapsed})
                    };
                    if let Some(renderer) = &renderer
                        && let Err(error) = renderer.eval(
                            &format!("__appsDispatch('{kind}',[{node}],{data})"),
                            "apps:animation",
                        )
                    {
                        eprintln!("apps: {error}");
                    }
                }
                Desktop::drive(runtime, cx);
            });
        }
        // WARNING: animated values replace `styles.values` only while this frame is built; `restore` puts the cascade back.
        let mut restore = Vec::new();
        let mut paint = Vec::new();
        if !animated.is_empty() {
            dom.computed = None;
            let values = &mut Rc::make_mut(&mut styles).values;
            for (id, style, layout) in animated {
                if layout {
                    restore.push((id, std::mem::replace(&mut values[id], style)));
                } else {
                    paint.push((id, style));
                }
            }
        }
        dom.svgs.retain(|id, (_, image)| {
            if visible.contains(id) {
                true
            } else {
                let _ = window.drop_image(image.clone());
                false
            }
        });
        dom.masks.retain(|id, (_, image)| {
            if visible.contains(id) {
                true
            } else {
                let _ = window.drop_image(image.clone());
                false
            }
        });
        dom.image_crops.retain(|id, (_, image)| {
            if visible.contains(id) {
                true
            } else {
                let _ = window.drop_image(image.clone());
                false
            }
        });
        dom.canvases.retain(|id, canvas| {
            if visible.contains(id) {
                true
            } else {
                let _ = window.drop_image(canvas.image.clone());
                false
            }
        });
        let frame_nodes: Vec<_> = doc
            .nodes
            .iter()
            .enumerate()
            .filter(|(id, node)| node.tag == "iframe" && visible.contains(id))
            .map(|(id, node)| {
                (
                    id,
                    node.attrs.get("srcdoc").cloned(),
                    node.attrs
                        .get("src")
                        .cloned()
                        .unwrap_or_else(|| "about:blank".into()),
                    node.attrs.get("sandbox").cloned(),
                )
            })
            .collect();
        self.browsers.retain(|id, _| {
            *id == usize::MAX || frame_nodes.iter().any(|(node, _, _, _)| node == id)
        });
        for (node, html, url, sandbox) in frame_nodes {
            let source = html.clone().unwrap_or_else(|| url.clone());
            if let std::collections::hash_map::Entry::Vacant(entry) = self.browsers.entry(node) {
                match crate::browser::Browser::new(
                    window,
                    &serde_json::json!({"javascript":sandbox.as_ref().is_none_or(|s|s.split_whitespace().any(|s|s=="allow-scripts")),"inspectable":inspectable}),
                ) {
                    Ok(browser) => {
                        entry.insert((browser, String::new()));
                    }
                    Err(error) => {
                        eprintln!("apps: iframe: {error}");
                        continue;
                    }
                }
            }
            let (browser, loaded) = self.browsers.get_mut(&node).unwrap();
            if source != *loaded {
                let result = if let Some(html) = html {
                    browser.load_html(&html, "about:blank")
                } else {
                    browser.load_url(&url)
                };
                if let Err(error) = result {
                    eprintln!("apps: iframe: {error}");
                }
                *loaded = source;
            }
            let bounds = dom.bounds.get(&node).copied().unwrap_or([0.; 4]);
            let _ = browser.set_bounds(bounds.map(f64::from));
        }
        let background = doc
            .nodes
            .iter()
            .position(|n| n.tag == "body")
            .and_then(|id| {
                styles.values[id]
                    .get("background-color")
                    .or_else(|| styles.values[id].get("background"))
            })
            .and_then(|s| render::color(s))
            .unwrap_or((0xffffff, 255));
        let mut order = Vec::new();
        let mut pending = vec![0];
        while let Some(id) = pending.pop() {
            order.push(id);
            pending.extend(&doc.nodes[id].children);
        }
        dom.layout_nodes.resize_with(doc.nodes.len(), || None);
        let mut changed = std::collections::HashSet::new();
        for id in order.into_iter().rev() {
            let node = &doc.nodes[id];
            let children: Vec<_> = node
                .children
                .iter()
                .filter(|&&child| {
                    !styles.values[child]
                        .get("display")
                        .is_some_and(|value| value == "none")
                        && !styles.values[child]
                            .get("position")
                            .is_some_and(|value| matches!(value.as_str(), "absolute" | "fixed"))
                })
                .map(|&child| {
                    (
                        child,
                        dom.layout_nodes[child]
                            .as_ref()
                            .map_or(0, |node| node.revision),
                    )
                })
                .collect();
            if dom.layout_nodes[id].as_ref().is_none_or(|old| {
                !Rc::ptr_eq(&old.style, &styles.values[id])
                    || old.node.tag != node.tag
                    || old.node.text != node.text
                    || old.node.attrs != node.attrs
                    || old.children != children
            }) {
                dom.layout_revision += 1;
                dom.layout_nodes[id] = Some(crate::dom::LayoutNode {
                    node: node.clone(),
                    style: styles.values[id].clone(),
                    children,
                    revision: dom.layout_revision,
                });
                changed.insert(id);
            }
        }
        if !paint.is_empty() {
            let values = &mut Rc::make_mut(&mut styles).values;
            for (id, style) in paint {
                restore.push((id, std::mem::replace(&mut values[id], style)));
            }
        }
        if !changed.is_empty() {
            dom.grids.retain(|id, _| !changed.contains(id));
            dom.measurements.retain(|(id, ..), _| !changed.contains(id));
            dom.inline_text.retain(|id, _| !changed.contains(id));
        }
        dom.text_layouts.clear();
        let mut warnings = Vec::new();
        if inspectable && !dom.linted {
            dom.linted = true;
            for (id, style) in styles.values.iter().enumerate() {
                let node = &doc.nodes[id];
                if node.tag.starts_with('#') {
                    continue;
                }
                let pseudos = styles.text_pseudos.get(&id);
                for (pseudo, values) in [
                    ("", Some(&**style)),
                    (
                        "::first-letter",
                        pseudos.and_then(|values| values[1].as_ref()),
                    ),
                    (
                        "::first-line",
                        pseudos.and_then(|values| values[2].as_ref()),
                    ),
                ] {
                    let Some(values) = values else { continue };
                    for (key, value) in values {
                        if !pseudo.is_empty() && style.get(key) == Some(value) {
                            continue;
                        }
                        let reason = match key.as_str() {
                            "line-height" | "vertical-align" if !pseudo.is_empty() => {
                                "line height and baseline can't change inside a line of text"
                            }
                            "transform-style" if value == "preserve-3d" => {
                                "3D children are flattened into the parent's plane"
                            }
                            "clip-path" if value.contains("shape(") => "shape() is not supported",
                            "filter" if value.contains("url(") && !value.contains("url(#") => {
                                "only filters defined in the page are supported"
                            }
                            _ => continue,
                        };
                        if dom.warned.insert(format!("{pseudo}{reason}")) {
                            let mut name = node.tag.clone();
                            if let Some(value) = node.attrs.get("id") {
                                name = format!("{name}#{value}");
                            }
                            for class in node
                                .attrs
                                .get("class")
                                .into_iter()
                                .flat_map(|value| value.split_whitespace())
                                .take(3)
                            {
                                name = format!("{name}.{class}");
                            }
                            warnings.push(format!(
                                "Unsupported CSS `{key}: {value}` on <{name}>{pseudo}: {reason}."
                            ));
                        }
                    }
                }
            }
        }
        dom.containers.clear();
        let measured = RefCell::new(std::mem::take(&mut dom.measurements));
        let inline_text = RefCell::new(std::mem::take(&mut dom.inline_text));
        for text in warnings {
            if host.logs.len() == 500 {
                host.logs.pop_front();
            }
            host.logs.push_back(serde_json::json!({"method":"Runtime.consoleAPICalled","params":{"type":"error","text":text,"context":crate::host::Role::Renderer(self.id)}}));
        }
        drop(host);
        let root = element(
            &Build {
                doc: &doc,
                styles: &styles,
                text: window.text_system(),
                pixel_ratio: window.scale_factor(),
                view: self,
                layers: Rc::default(),
                path: Rc::default(),
                flex_parent: false,
                measured: &measured,
                inline_text: &inline_text,
                perspective: None,
            },
            0,
            [width, height],
            [width, height],
            None,
            cx,
        );
        for (node, children) in children.into_iter().rev() {
            doc.nodes[node].children = children;
        }
        {
            let mut host = self.host.borrow_mut();
            if let Some(page) = host.pages.get_mut(&self.id) {
                page.doc = doc;
            }
            let dom = host.dom.entry(self.id).or_default();
            dom.measurements = measured.into_inner();
            dom.inline_text = inline_text.into_inner();
            if !restore.is_empty() {
                let values = &mut Rc::make_mut(&mut styles).values;
                for (id, style) in restore {
                    values[id] = style;
                }
                dom.computed = Some(styles);
            }
            // WARNING: one extra pass per frame at most, so container queries that resize their container cannot loop.
            let changed = dom.update_containers();
            let repass = changed && !dom.container_pass;
            dom.container_pass = repass;
            if repass {
                drop(host);
                return self.render(window, cx).into_any_element();
            }
        }
        let runtime = self.runtime.clone();
        let id = self.id;
        window.on_next_frame(move |_, cx| {
            let renderer = runtime
                .borrow()
                .views
                .get(&id)
                .and_then(|v| v.renderer.clone());
            if let Some(renderer) = renderer
                && let Err(error) = renderer.eval("__appsLayout()", "apps:layout")
            {
                eprintln!("apps: {error}");
            }
            cx.defer(move |cx| Desktop::drive(runtime, cx));
        });
        div()
            .id("apps-page")
            .size_full()
            .overflow_y_scroll()
            .bg(rgba((background.0 << 8) | background.1 as u32))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view,event:&KeyDownEvent,_,cx| {
                let key = event.keystroke.key.as_str();
                let modifiers = &event.keystroke.modifiers;
                if !view.host.borrow().packaged && (key == "f12" || (key == "i" && ((modifiers.platform && modifiers.alt) || (modifiers.control && modifiers.shift)))) {
                    view.host.borrow_mut().commands.push(serde_json::json!({"op":"window","args":["devtools",view.id]}));
                    let runtime = view.runtime.clone();
                    cx.defer(move |cx| Desktop::drive(runtime, cx));
                    cx.stop_propagation();
                    return;
                }
                let allowed = Desktop::dispatch(view.runtime.clone(),view.id,"keydown",&[],serde_json::json!({"nativeKey":event.keystroke.key,"keyChar":event.keystroke.key_char,"ctrlKey":modifiers.control,"metaKey":modifiers.platform,"altKey":modifiers.alt,"shiftKey":modifiers.shift,"repeat":event.is_held}),cx);
                if !allowed { cx.stop_propagation(); return; }
                let primary = if cfg!(target_os = "macos") { modifiers.platform && !modifiers.control } else { modifiers.control && !modifiers.platform };
                if primary && !modifiers.alt && matches!(key,"c"|"x"|"v") {
                    let text = if key == "v" { cx.read_from_clipboard().and_then(|item|item.text()).unwrap_or_default() } else { String::new() };
                    match view.evaluate(&format!("JSON.stringify(__appsClipboard({},{}))",serde_json::json!(if key=="c"{"copy"}else if key=="x"{"cut"}else{"paste"}),serde_json::json!(text))) {
                        Ok(value) => if key != "v" && let Ok(text) = serde_json::from_str::<String>(&value) { cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)); },
                        Err(error) => eprintln!("apps: {error}"),
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_key_up(cx.listener(|view,event:&KeyUpEvent,_,cx| {
                let modifiers = &event.keystroke.modifiers;
                Desktop::dispatch(view.runtime.clone(),view.id,"keyup",&[],serde_json::json!({"nativeKey":event.keystroke.key,"keyChar":event.keystroke.key_char,"ctrlKey":modifiers.control,"metaKey":modifiers.platform,"altKey":modifiers.alt,"shiftKey":modifiers.shift}),cx);
            }))
            .on_drop(cx.listener(|view,paths:&gpui::ExternalPaths,window,cx| {
                let mut files=Vec::new();
                for path in paths.paths(){let result=(||->Result<serde_json::Value>{let metadata=std::fs::metadata(path)?;ensure!(metadata.is_file()&&metadata.len()<=64*1024*1024,"Dropped file exceeds 64 MB or is not a file");let bytes=std::fs::read(path)?;Ok(serde_json::json!({"name":path.file_name().unwrap_or_default().to_string_lossy(),"path":path.to_string_lossy(),"bytes":bytes,"lastModified":metadata.modified().ok().and_then(|time|time.duration_since(std::time::UNIX_EPOCH).ok()).map(|time|time.as_millis() as u64).unwrap_or(0)}))})();match result{Ok(file)=>files.push(file),Err(error)=>eprintln!("apps: drop: {error}")}}
                let position=window.mouse_position();if let Err(error)=view.evaluate(&format!("__appsDrop({},{},{})",serde_json::json!(files),f32::from(position.x),f32::from(position.y))){eprintln!("apps: {error}");}
                let runtime=view.runtime.clone();cx.defer(move|cx|Desktop::drive(runtime,cx));cx.notify();
            }))
            .child(pointer_events)
            .child(input)
            .child(root)
            .into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
fn css_background(
    style: &render::Style,
    units: render::Lengths,
    id: usize,
    slot: usize,
    radius: [f32; 4],
    view: gpui::Entity<WebView>,
    base: Option<&url::Url>,
    assets: &HashMap<String, std::path::PathBuf>,
) -> Vec<AnyElement> {
    let list = |name: &str| {
        render::components(style.get(name).map_or("", String::as_str), ',')
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (sizes, positions, repeats) = (
        list("background-size"),
        list("background-position"),
        list("background-repeat"),
    );
    let pick = |values: &[String], index: usize| {
        values
            .get(index % values.len().max(1))
            .cloned()
            .unwrap_or_default()
    };
    let corners = gpui::Corners {
        top_left: px(radius[0]),
        top_right: px(radius[1]),
        bottom_right: px(radius[2]),
        bottom_left: px(radius[3]),
    };
    let mut layers = Vec::new();
    let mut indexes = Vec::new();
    for (index, image) in list("background-image").into_iter().enumerate().rev() {
        let (size, position, repeat) = (
            pick(&sizes, index),
            pick(&positions, index),
            pick(&repeats, index),
        );
        if let Some(source) = image.strip_prefix("url(").and_then(|v| v.strip_suffix(')')) {
            if let Some(path) = base
                .and_then(|base| base.join(source.trim().trim_matches(['"', '\''])).ok())
                .and_then(|url| match url.scheme() {
                    "file" => url.to_file_path().ok(),
                    "apps" => assets.get(url.path().trim_start_matches('/')).cloned(),
                    _ => None,
                })
            {
                layers.push(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .overflow_hidden()
                        .child(img(path).size_full().object_fit(match size.as_str() {
                            "cover" => gpui::ObjectFit::Cover,
                            "contain" => gpui::ObjectFit::Contain,
                            "" | "auto" | "auto auto" => gpui::ObjectFit::None,
                            _ => gpui::ObjectFit::Fill,
                        }))
                        .into_any_element(),
                );
                indexes.push(index);
            }
            continue;
        }
        let Some((kind, body)) = image
            .split_once('(')
            .filter(|(kind, _)| kind.ends_with("-gradient"))
            .and_then(|(kind, body)| Some((kind.to_owned(), body.strip_suffix(')')?.to_owned())))
        else {
            continue;
        };
        let view = view.clone();
        layers.push(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    let [width, height] =
                        [f32::from(bounds.size.width), f32::from(bounds.size.height)];
                    if width <= 0. || height <= 0. {
                        return;
                    }
                    let Some(source) = crate::effects::gradient_svg(
                        &kind, &body, &size, &position, &repeat, units, width, height,
                    ) else {
                        return;
                    };
                    let view = view.read(cx);
                    let mut host = view.host.borrow_mut();
                    let backgrounds = &mut host.dom.entry(view.id).or_default().backgrounds;
                    let scale = window.scale_factor();
                    let rw = (width * scale).ceil().clamp(1., 4096.) as u32;
                    let rh = (height * scale).ceil().clamp(1., 4096.) as u32;
                    let key = (id, slot * 64 + index);
                    if backgrounds.get(&key).is_none_or(|(old, image)| {
                        old != &source
                            || u32::from(image.size(0).width) != rw
                            || u32::from(image.size(0).height) != rh
                    }) && let Ok(image) = crate::canvas::svg(&source, rw, rh, None)
                        && let Some((_, old)) = backgrounds.insert(key, (source, image))
                    {
                        let _ = window.drop_image(old);
                    }
                    if let Some((_, image)) = backgrounds.get(&key) {
                        let _ =
                            window.paint_image(bounds, bounds, corners, image.clone(), 0, false);
                    }
                },
            )
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .into_any_element(),
        );
        indexes.push(index);
    }
    let modes = list("background-blend-mode");
    if modes
        .iter()
        .all(|mode| matches!(mode.trim(), "" | "normal"))
    {
        return layers;
    }
    // Blended layers mix with the layers and color below them, inside an isolated group.
    let mut group = div().absolute().top_0().left_0().size_full();
    if let Some((ink, alpha)) = style
        .get("background-color")
        .and_then(|value| render::color(value))
    {
        group = group.child(
            div()
                .absolute()
                .size_full()
                .rounded_tl(px(radius[0]))
                .rounded_tr(px(radius[1]))
                .rounded_br(px(radius[2]))
                .rounded_bl(px(radius[3]))
                .bg(rgba(ink << 8 | alpha as u32)),
        );
    }
    for (layer, index) in layers.into_iter().zip(indexes) {
        let blend_mode = crate::effects::blend_mode(&pick(&modes, index));
        group = group.child(Effected {
            effect: gpui::PaintEffect {
                blend_mode,
                ..Default::default()
            },
            child: layer,
        });
    }
    vec![
        Effected {
            effect: gpui::PaintEffect::default(),
            child: group.into_any_element(),
        }
        .into_any_element(),
    ]
}

/// A box turned a quarter turn for vertical writing modes.
struct Turned {
    child: AnyElement,
    clockwise: bool,
    block: f32,
    inline: f32,
}

impl Turned {
    fn matrix(&self, origin: Point<Pixels>) -> crate::effects::Matrix {
        let [x, y] = [f32::from(origin.x), f32::from(origin.y)];
        // Clockwise maps (u, v) to (block - v, u); counterclockwise to (v, inline - u).
        let mut m = crate::effects::IDENTITY;
        if self.clockwise {
            [m[0], m[1], m[4], m[5]] = [0., 1., -1., 0.];
            [m[12], m[13]] = [x + self.block + y, y - x];
        } else {
            [m[0], m[1], m[4], m[5]] = [0., -1., 1., 0.];
            [m[12], m[13]] = [x - y, y + self.inline + x];
        }
        m
    }
}

impl IntoElement for Turned {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for Turned {
    type RequestLayoutState = ();
    type PrepaintState = crate::effects::Matrix;
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, ()) {
        let child = self.child.request_layout(window, cx);
        let mut style = gpui::Style {
            position: gpui::Position::Absolute,
            ..Default::default()
        };
        style.size.width = px(self.block).into();
        style.size.height = px(self.inline).into();
        (window.request_layout(style, [child], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> crate::effects::Matrix {
        let matrix = self.matrix(bounds.origin);
        window.with_hit_transform(matrix, |window| self.child.prepaint(window, cx));
        matrix
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        matrix: &mut crate::effects::Matrix,
        window: &mut Window,
        cx: &mut App,
    ) {
        let effect = gpui::PaintEffect {
            transform: Some(*matrix),
            ..Default::default()
        };
        window.paint_effect(effect, |window| self.child.paint(window, cx));
    }
}

/// Paints a child through an effect layer.
struct Effected {
    effect: gpui::PaintEffect,
    child: AnyElement,
}

impl IntoElement for Effected {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for Effected {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.paint_effect(self.effect.clone(), |window| self.child.paint(window, cx));
    }
}

struct DomBox {
    inner: Option<gpui::Stateful<gpui::Div>>,
    anchor: Option<([bool; 2], bool, [f32; 2])>,
    id: usize,
    view: gpui::Entity<WebView>,
    sticky: Option<(gpui::ScrollHandle, [f32; 4])>,
    fixed: bool,
    inset: [Option<f32>; 4],
    style: Rc<render::Style>,
    /// The group opacity, when the box paints through an effect layer.
    effects: Option<f32>,
    /// The parent's perspective, and the parent.
    perspective: Option<((f32, String), usize)>,
    units: render::Lengths,
}

impl IntoElement for DomBox {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for DomBox {
    type RequestLayoutState = AnyElement;
    type PrepaintState = (Bounds<Pixels>, Option<crate::effects::Matrix>);
    fn id(&self) -> Option<gpui::ElementId> {
        Some(gpui::ElementId::Name(format!("dom-{}", self.id).into()))
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, AnyElement) {
        let mut inner = self.inner.take().unwrap().into_any_element();
        (inner.request_layout(window, cx), inner)
    }
    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        mut bounds: Bounds<Pixels>,
        inner: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let mut offset = point(px(0.), px(0.));
        if let Some((axes, block, margin)) = self.anchor
            && let Some(anchor) = {
                let view = self.view.read(cx);
                let host = view.host.borrow();
                host.dom
                    .get(&view.id)
                    .and_then(|dom| dom.anchors.get(&self.id).copied())
            }
        {
            if axes[0] && !block {
                offset.x = anchor.left() + px(margin[0]) - bounds.left();
            }
            if axes[1] {
                offset.y = if block { anchor.bottom() } else { anchor.top() } + px(margin[1])
                    - bounds.top();
            }
            bounds.origin += offset;
        }
        if let Some((scroll, border)) = &self.sticky {
            let mut viewport = scroll.bounds();
            viewport.origin += point(px(border[3]), px(border[0]));
            viewport.size.width -= px(border[1] + border[3]);
            viewport.size.height -= px(border[0] + border[2]);
            if let Some(left) = self.inset[3] {
                offset.x = (viewport.left() + px(left) - bounds.left()).max(px(0.));
            }
            if let Some(right) = self.inset[1] {
                offset.x = offset.x.min(viewport.right() - px(right) - bounds.right());
            }
            if let Some(top) = self.inset[0] {
                offset.y = (viewport.top() + px(top) - bounds.top()).max(px(0.));
            }
            if let Some(bottom) = self.inset[2] {
                offset.y = offset
                    .y
                    .min(viewport.bottom() - px(bottom) - bounds.bottom());
            }
            bounds.origin += offset;
        }
        if self.fixed {
            let viewport = window.viewport_size();
            if let Some(left) = self.inset[3] {
                offset.x = px(left) - bounds.left();
            } else if let Some(right) = self.inset[1] {
                offset.x = viewport.width - px(right) - bounds.right();
            }
            if let Some(top) = self.inset[0] {
                offset.y = px(top) - bounds.top();
            } else if let Some(bottom) = self.inset[2] {
                offset.y = viewport.height - px(bottom) - bounds.bottom();
            }
            bounds.origin += offset;
        }
        let perspective = self
            .perspective
            .as_ref()
            .and_then(|((distance, origin), parent)| {
                let view = self.view.read(cx);
                let host = view.host.borrow();
                let [x, y, width, height] = *host.dom.get(&view.id)?.bounds.get(parent)?;
                let [ox, oy, _] = crate::effects::origin(origin, &self.units, [width, height]);
                Some((*distance, [x + ox, y + oy]))
            });
        let mut matrix = crate::effects::transform(&self.style, &self.units, bounds, perspective);
        // Plain translations move the box instead of painting it through a layer.
        if let Some([x, y]) = matrix.as_ref().and_then(crate::effects::translation) {
            let translation = point(px(x), px(y));
            offset += translation;
            bounds.origin += translation;
            matrix = None;
        }
        let mut prepaint = |window: &mut Window| {
            let window_bounds = window.transformed_bounds(bounds);
            let view = self.view.read(cx);
            view.host
                .borrow_mut()
                .dom
                .entry(view.id)
                .or_default()
                .bounds
                .insert(
                    self.id,
                    [
                        window_bounds.origin.x.into(),
                        window_bounds.origin.y.into(),
                        window_bounds.size.width.into(),
                        window_bounds.size.height.into(),
                    ],
                );
            if let Some((browser, _)) = view.browsers.get(&self.id) {
                let _ = browser.set_bounds([
                    f32::from(window_bounds.origin.x) as f64,
                    f32::from(window_bounds.origin.y) as f64,
                    f32::from(window_bounds.size.width) as f64,
                    f32::from(window_bounds.size.height) as f64,
                ]);
            }
            window.with_element_offset(offset, |window| {
                inner.prepaint(window, cx);
            });
        };
        match matrix {
            Some(matrix) => window.with_hit_transform(matrix, &mut prepaint),
            None => prepaint(window),
        }
        (bounds, matrix)
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        inner: &mut AnyElement,
        (bounds, matrix): &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if matrix.is_none() && self.effects.is_none() {
            return inner.paint(window, cx);
        }
        let bounds = *bounds;
        if let Some(matrix) = matrix
            && self
                .style
                .get("backface-visibility")
                .is_some_and(|value| value == "hidden")
            && crate::effects::backface(matrix, bounds)
        {
            return;
        }
        let (clip, mut layers) = self.effects(bounds, *matrix, window, cx);
        let mut paint = |window: &mut Window| match &clip {
            Some(clip) => window.with_content_mask(Some(*clip), |window| inner.paint(window, cx)),
            None => inner.paint(window, cx),
        };
        nest_effects(&mut layers, window, &mut paint);
    }
}

impl DomBox {
    /// The rounded `clip-path` and the effect layers of the box, innermost first.
    fn effects(
        &self,
        bounds: Bounds<Pixels>,
        transform: Option<crate::effects::Matrix>,
        window: &mut Window,
        cx: &mut App,
    ) -> (Option<gpui::ContentMask<Pixels>>, Vec<gpui::PaintEffect>) {
        use crate::effects::{self, Clip};
        let style = &self.style;
        let units = self.units;
        let value = |name: &str| {
            style
                .get(name)
                .or_else(|| style.get(&format!("-webkit-{name}")))
                .filter(|value| !matches!(value.as_str(), "none" | "normal" | "auto"))
        };
        let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
        let view = self.view.read(cx);
        let mut host = view.host.borrow_mut();
        let empty = crate::html::Document::default();
        let host = &mut *host;
        let doc = host.pages.get(&view.id).map_or(&empty, |page| &page.doc);
        let mut layers = value("filter")
            .map(|value| effects::filters(value, &units, doc))
            .unwrap_or_default();
        let mut clip = None;
        let mut shapes = None;
        match value("clip-path").and_then(|value| effects::clip(value, &units, size, doc)) {
            Some(Clip::Rounded(rect, radii)) => {
                clip = Some(gpui::ContentMask::rounded(
                    Bounds::new(
                        bounds.origin + point(px(rect[0]), px(rect[1])),
                        gpui::size(px(rect[2]), px(rect[3])),
                    ),
                    effects::corners(radii),
                ))
            }
            Some(Clip::Shape(shape)) => shapes = Some(shape),
            None => {}
        }
        let base = host.dom.get(&view.id).and_then(|state| state.base.clone());
        let mask_layers = mask_svg(style, units, size, base.as_ref(), &host.assets);
        let mask = (shapes.is_some() || mask_layers.is_some()).then(|| {
            let [width, height] = size;
            let source = match (shapes, mask_layers) {
                (Some(shape), Some(layers)) => format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\"><defs><clipPath id=\"h2o-clip\">{shape}</clipPath></defs><g clip-path=\"url(#h2o-clip)\">{layers}</g></svg>"
                ),
                (Some(shape), None) => format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">{shape}</svg>"
                ),
                (None, layers) => format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">{}</svg>",
                    layers.unwrap_or_default()
                ),
            };
            let scale = window.scale_factor();
            let raster = [width, height].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
            let masks = &mut host.dom.entry(view.id).or_default().masks;
            if masks.get(&self.id).is_none_or(|(old, image)| {
                *old != source
                    || u32::from(image.size(0).width) != raster[0]
                    || u32::from(image.size(0).height) != raster[1]
            }) {
                match crate::canvas::svg(&source, raster[0], raster[1], None) {
                    Ok(image) => {
                        if let Some((_, old)) = masks.insert(self.id, (source, image)) {
                            let _ = window.drop_image(old);
                        }
                    }
                    Err(error) => eprintln!("apps: mask: {error}"),
                }
            }
            masks.get(&self.id).map(|(_, image)| (image.clone(), bounds))
        });
        let radii = || {
            effects::corners(
                [
                    "border-top-left-radius",
                    "border-top-right-radius",
                    "border-bottom-right-radius",
                    "border-bottom-left-radius",
                ]
                .map(|name| {
                    style
                        .get(name)
                        .and_then(|value| units.resolve(value, size[0]))
                        .unwrap_or(0.)
                        .max(0.)
                }),
            )
        };
        let backdrop = value("backdrop-filter")
            .and_then(|value| effects::backdrop(value, &units))
            .map(|(blur, color_matrix)| gpui::Backdrop {
                bounds,
                corner_radii: radii(),
                blur,
                color_matrix,
            });
        let outer = gpui::PaintEffect {
            transform,
            opacity: self.effects.unwrap_or(1.),
            blend_mode: value("mix-blend-mode")
                .map_or(gpui::BlendMode::Normal, |value| effects::blend_mode(value)),
            mask: mask.flatten(),
            backdrop,
            ..Default::default()
        };
        let isolate = style
            .get("isolation")
            .is_some_and(|value| value == "isolate");
        let needed = outer.transform.is_some()
            || outer.opacity < 1.
            || outer.blend_mode != gpui::BlendMode::Normal
            || outer.mask.is_some()
            || outer.backdrop.is_some()
            || isolate;
        if needed {
            // One filter function and no clip can share the outer layer.
            if layers.len() == 1 && clip.is_none() {
                let filter = layers.pop().unwrap();
                layers.push(gpui::PaintEffect {
                    color_matrix: filter.color_matrix,
                    blur: filter.blur,
                    shadow: filter.shadow,
                    ..outer
                });
            } else {
                layers.push(outer);
            }
        }
        (clip, layers)
    }
}

/// Paints through the effect layers, the last one outermost.
fn nest_effects(
    layers: &mut Vec<gpui::PaintEffect>,
    window: &mut Window,
    paint: &mut dyn FnMut(&mut Window),
) {
    match layers.pop() {
        Some(effect) => window.paint_effect(effect, |window| nest_effects(layers, window, paint)),
        None => paint(window),
    }
}

/// The layers of `mask-image` as SVG content covering a box, the first layer on top.
fn mask_svg(
    style: &render::Style,
    units: render::Lengths,
    [width, height]: [f32; 2],
    base: Option<&url::Url>,
    assets: &HashMap<String, std::path::PathBuf>,
) -> Option<String> {
    let get = |name: &str| {
        style
            .get(name)
            .or_else(|| style.get(&format!("-webkit-{name}")))
            .map_or("", String::as_str)
    };
    let images = get("mask-image");
    if images.is_empty() || images == "none" {
        return None;
    }
    let list = |name: &str| {
        render::components(get(name), ',')
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (sizes, positions, repeats) = (
        list("mask-size"),
        list("mask-position"),
        list("mask-repeat"),
    );
    let pick = |values: &[String], index: usize| {
        values
            .get(index % values.len().max(1))
            .cloned()
            .unwrap_or_default()
    };
    let mut content = String::new();
    for (index, image) in render::components(images, ',')
        .into_iter()
        .enumerate()
        .rev()
    {
        let image = image.trim();
        if let Some(source) = image.strip_prefix("url(").and_then(|v| v.strip_suffix(')')) {
            let Some(path) = base
                .and_then(|base| base.join(source.trim().trim_matches(['"', '\''])).ok())
                .and_then(|url| match url.scheme() {
                    "file" => url.to_file_path().ok(),
                    "apps" => assets.get(url.path().trim_start_matches('/')).cloned(),
                    _ => None,
                })
            else {
                continue;
            };
            let aspect = match pick(&sizes, index).as_str() {
                "cover" => "xMidYMid slice",
                "contain" => "xMidYMid meet",
                _ => "none",
            };
            content.push_str(&format!(
                "<image href=\"{}\" width=\"{width}\" height=\"{height}\" preserveAspectRatio=\"{aspect}\"/>",
                crate::html::escape(&path.to_string_lossy())
            ));
            continue;
        }
        let Some((kind, body)) = image
            .split_once('(')
            .filter(|(kind, _)| kind.ends_with("-gradient"))
            .and_then(|(kind, body)| Some((kind, body.strip_suffix(')')?)))
        else {
            continue;
        };
        let size = pick(&sizes, index);
        let position = pick(&positions, index);
        let repeat = pick(&repeats, index);
        if let Some(svg) = crate::effects::gradient_svg(
            kind,
            body,
            if size.is_empty() { "auto" } else { &size },
            if position.is_empty() {
                "0% 0%"
            } else {
                &position
            },
            if repeat.is_empty() { "repeat" } else { &repeat },
            units,
            width,
            height,
        ) && let Some(start) = svg.find('>')
            && let Some(end) = svg.rfind("</svg>")
        {
            // Each layer is its own SVG document, so its `id`s must not collide.
            content.push_str(
                &svg[start + 1..end]
                    .replace("id=\"", &format!("id=\"m{index}"))
                    .replace("url(#", &format!("url(#m{index}")),
            );
        }
    }
    (!content.is_empty()).then_some(content)
}

struct TextHighlight {
    range: Range<usize>,
    color: Option<gpui::Hsla>,
    background: gpui::Hsla,
}

#[allow(clippy::too_many_arguments)]
fn paint_text(
    line: &gpui::LineLayout,
    wraps: &[gpui::WrapBoundary],
    origin: Point<Pixels>,
    line_height: Pixels,
    runs: &[gpui::TextRun],
    offset: usize,
    highlights: &[TextHighlight],
    rows: &[crate::dom::Row],
    text: &str,
    ink: Option<gpui::Hsla>,
    window: &mut Window,
) {
    let shift = |row: usize| rows.get(row).map_or(px(0.), |row| row.shift);
    // Justified rows widen each space by the row gap.
    let justify = |row: usize, index: usize| {
        rows.get(row)
            .filter(|row| row.gap > px(0.))
            .map_or(px(0.), |row| {
                row.gap * spaces(text.get(row.start.min(index)..index).unwrap_or(""))
            })
    };
    window.paint_layer(
        Bounds::new(
            origin,
            size(
                rows.iter().fold(line.width, |max, row| {
                    max.max(line.width + row.shift).max(row.right)
                }),
                line_height * (wraps.len() + 1),
            ),
        ),
        |window| {
            // Larger first-letter or first-line runs must not move the baseline of the other rows.
            let (ascent, descent) = line
                .runs
                .iter()
                .rev()
                .find(|run| run.font_size == line.font_size)
                .map_or((line.ascent, line.descent), |run| {
                    let text = window.text_system();
                    (
                        text.ascent(run.font_id, run.font_size),
                        -text.descent(run.font_id, run.font_size),
                    )
                });
            let baseline = (line_height - ascent - descent) / 2. + ascent;
            let drop = |row: usize| rows.get(row).map_or(px(0.), |row| row.drop);
            let mut start = 0;
            let mut left = px(0.);
            for (row, boundary) in wraps.iter().map(Some).chain([None]).enumerate() {
                let (end, right) = boundary.map_or((line.len, line.width), |boundary| {
                    let glyph = &line.runs[boundary.run_ix].glyphs[boundary.glyph_ix];
                    (glyph.index, glyph.position.x)
                });
                let row_left = origin.x + shift(row) - left;
                let top = origin.y + line_height * row + drop(row);
                let mut run_start = 0;
                for run in runs {
                    if run_start >= offset + end {
                        break;
                    }
                    let (a, b) = (
                        run_start.max(offset + start),
                        (run_start + run.len).min(offset + end),
                    );
                    run_start += run.len;
                    if a >= b
                        || run.underline.is_none()
                            && run.strikethrough.is_none()
                            && run.background_color.is_none()
                    {
                        continue;
                    }
                    let x = line.x_for_index(a - offset) + justify(row, a);
                    let end_x = if b == offset + end {
                        right + justify(row, b)
                    } else {
                        line.x_for_index(b - offset) + justify(row, b)
                    };
                    if let Some(background) = run.background_color.filter(|_| ink.is_none()) {
                        window.paint_quad(gpui::fill(
                            Bounds::new(
                                point(row_left + x, top + baseline - line.ascent),
                                size(end_x - x, line.ascent + line.descent),
                            ),
                            background,
                        ));
                    }
                    if let Some(underline) = &run.underline {
                        window.paint_underline(
                            point(row_left + x, top + baseline + line.descent * 0.618),
                            end_x - x,
                            &gpui::UnderlineStyle {
                                color: ink.or(underline.color),
                                ..*underline
                            },
                        );
                    }
                    if let Some(strikethrough) = &run.strikethrough {
                        window.paint_strikethrough(
                            point(row_left + x, top + (line.ascent * 0.5 + baseline) * 0.5),
                            end_x - x,
                            &gpui::StrikethroughStyle {
                                color: ink.or(strikethrough.color),
                                ..*strikethrough
                            },
                        );
                    }
                }
                for highlight in highlights {
                    let a = highlight.range.start.max(offset + start);
                    let b = highlight.range.end.min(offset + end);
                    if a >= b {
                        continue;
                    }
                    let x = line.x_for_index(a - offset) + justify(row, a);
                    let end_x = if b == offset + end {
                        right + justify(row, b)
                    } else {
                        line.x_for_index(b - offset) + justify(row, b)
                    };
                    if end_x > x && highlight.background.a > 0. {
                        window.paint_quad(gpui::fill(
                            Bounds::new(point(row_left + x, top), size(end_x - x, line_height)),
                            highlight.background,
                        ));
                    }
                }
                start = end;
                left = right;
            }
            let mut wrap = 0;
            let mut left = px(0.);
            let mut run_index = 0;
            let mut run_end = runs.first().map_or(0, |run| run.len);
            for (run_ix, run) in line.runs.iter().enumerate() {
                let glyph_size = window
                    .text_system()
                    .bounding_box(run.font_id, run.font_size)
                    .size;
                for (glyph_ix, glyph) in run.glyphs.iter().enumerate() {
                    if wraps.get(wrap) == Some(&gpui::WrapBoundary { run_ix, glyph_ix }) {
                        wrap += 1;
                        left = glyph.position.x;
                    }
                    let index = offset + glyph.index;
                    while index >= run_end && run_index + 1 < runs.len() {
                        run_index += 1;
                        run_end += runs[run_index].len;
                    }
                    let color = ink.unwrap_or_else(|| {
                        highlights
                            .iter()
                            .find(|highlight| highlight.range.contains(&index))
                            .and_then(|highlight| highlight.color)
                            .unwrap_or(runs[run_index].color)
                    });
                    if color.a == 0. {
                        continue;
                    }
                    let position = point(
                        origin.x + shift(wrap) + glyph.position.x - left + justify(wrap, index),
                        origin.y + line_height * wrap + baseline + drop(wrap),
                    );
                    if !Bounds::new(point(position.x, position.y - baseline), glyph_size)
                        .intersects(&window.content_mask().bounds)
                    {
                        continue;
                    }
                    if glyph.is_emoji {
                        let _ = window.paint_emoji(position, run.font_id, glyph.id, run.font_size);
                    } else {
                        let _ = window.paint_glyph(
                            position,
                            run.font_id,
                            glyph.id,
                            run.font_size,
                            color,
                        );
                    }
                }
            }
        },
    );
}

fn spaces(text: &str) -> f32 {
    text.bytes().filter(|byte| *byte == b' ').count() as f32
}

/// How much taller the first row is than the line height, when larger first-letter or
/// first-line runs rise above the normal baseline.
fn first_row_drop(
    line: Option<&gpui::WrappedLineLayout>,
    line_height: Pixels,
    window: &Window,
) -> Pixels {
    let Some(line) = line else {
        return px(0.);
    };
    let layout = &line.unwrapped_layout;
    let text = window.text_system();
    let Some(normal) = layout
        .runs
        .iter()
        .rev()
        .find(|run| run.font_size == layout.font_size)
    else {
        return px(0.);
    };
    let (ascent, descent) = (
        text.ascent(normal.font_id, normal.font_size),
        -text.descent(normal.font_id, normal.font_size),
    );
    let baseline = (line_height - ascent - descent) / 2. + ascent;
    let end = line.wrap_boundaries.first().map_or(usize::MAX, |boundary| {
        layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index
    });
    layout
        .runs
        .iter()
        .filter(|run| {
            run.font_size > layout.font_size
                && run.glyphs.first().is_some_and(|glyph| glyph.index < end)
        })
        .map(|run| text.ascent(run.font_id, run.font_size) - baseline)
        .fold(px(0.), Pixels::max)
}

pub(crate) fn text_position(
    layout: &gpui::TextLayout,
    text: &str,
    rows: &[crate::dom::Row],
    index: usize,
) -> Option<Point<Pixels>> {
    let mut position = layout.position_for_index(index)?;
    let row = ((position.y - layout.bounds().top()) / layout.line_height()).round();
    if let Some(row) = rows.get(row as usize) {
        position.x +=
            row.shift + row.gap * spaces(text.get(row.start.min(index)..index).unwrap_or(""));
        position.y += row.drop;
    }
    Some(position)
}

pub(crate) fn text_index(
    layout: &gpui::TextLayout,
    text: &str,
    rows: &[crate::dom::Row],
    mut position: Point<Pixels>,
) -> Result<usize, usize> {
    position.y -= rows.first().map_or(px(0.), |row| row.drop);
    let row = ((position.y - layout.bounds().top()) / layout.line_height())
        .floor()
        .max(0.) as usize;
    let Some(placed) = rows.get(row) else {
        return layout.index_for_position(position);
    };
    if placed.gap > px(0.) {
        // Justified rows: pick the closest character boundary of the row.
        let end = rows.get(row + 1).map_or(text.len(), |next| next.start);
        return (placed.start..=end)
            .filter(|&index| text.is_char_boundary(index))
            .filter_map(|index| Some((index, text_position(layout, text, rows, index)?.x)))
            .min_by(|a, b| {
                f32::from((a.1 - position.x).abs()).total_cmp(&f32::from((b.1 - position.x).abs()))
            })
            .map(|(index, _)| index)
            .ok_or(placed.start);
    }
    position.x -= placed.shift;
    layout.index_for_position(position)
}

fn first_line(
    data: &render::InlineText,
    width: Option<Pixels>,
    font_size: Pixels,
    text_system: &gpui::WindowTextSystem,
) -> (SharedString, Vec<gpui::TextRun>) {
    let mut text = data.text.to_string();
    let Some((base, pseudo, mode)) = &data.first_line else {
        return (text.into(), data.runs.clone());
    };
    let len = text.find('\n').unwrap_or(text.len());
    let mut first = text[..len].to_owned();
    if let Some(mode) = mode {
        render::text_transform(&mut first, 0..len, mode);
    }
    let end = text_system
        .shape_text(
            first.into(),
            font_size,
            &render::restyle(&data.runs, 0..len, base, pseudo),
            width,
            Some(2),
        )
        .ok()
        .and_then(|lines| {
            let line = lines.first()?;
            let boundary = line.wrap_boundaries.first()?;
            Some(line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index)
        })
        .unwrap_or(len);
    if let Some(mode) = mode {
        render::text_transform(&mut text, 0..end, mode);
    }
    (
        text.into(),
        render::restyle(&data.runs, 0..end, base, pseudo),
    )
}

struct InlineText {
    inner: gpui::StyledText,
    data: Rc<render::InlineText>,
    view: gpui::Entity<WebView>,
    runs: Option<Vec<gpui::TextRun>>,
    /// For justified text, whether the last row is justified too.
    justify: Option<bool>,
    /// Whether the text was measured here instead of by `inner`.
    measured: bool,
}

impl IntoElement for InlineText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for InlineText {
    type RequestLayoutState = ();
    type PrepaintState = (Vec<(gpui::Hitbox, gpui::CursorStyle)>, crate::dom::Rows);
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, ()) {
        let sized = self.data.runs.iter().any(|run| run.font_size.is_some())
            || self
                .data
                .first_line
                .as_ref()
                .is_some_and(|(_, pseudo, _)| pseudo.font_size.is_some());
        self.measured = self.data.first_line.is_some() || sized;
        if !self.measured {
            return self.inner.request_layout(id, inspector, window, cx);
        }
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = style
            .line_height
            .to_pixels(font_size.into(), window.rem_size());
        let data = self.data.clone();
        let cached = std::cell::Cell::new(None);
        let layout = window.request_measured_layout(
            Default::default(),
            move |known, available, window, _| {
                let width = known
                    .width
                    .or(match available.width {
                        gpui::AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    })
                    .filter(|_| style.white_space == gpui::WhiteSpace::Normal);
                if let Some((_, size)) = cached.get().filter(|(cached, _)| *cached == width) {
                    return size;
                }
                let (text, runs) = first_line(&data, width, font_size, window.text_system());
                let mut size = gpui::Size::<Pixels>::default();
                let lines = window
                    .text_system()
                    .shape_text(text, font_size, &runs, width, style.line_clamp)
                    .unwrap_or_default();
                for line in &lines {
                    let line = line.size(line_height);
                    size.height += line.height;
                    size.width = size.width.max(line.width).ceil();
                }
                size.height +=
                    first_row_drop(lines.first().map(|line| &***line), line_height, window);
                cached.set(Some((width, size)));
                size
            },
        );
        (layout, ())
    }
    fn prepaint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let style = window.text_style();
        if self.measured {
            let width =
                (style.white_space == gpui::WhiteSpace::Normal).then_some(bounds.size.width);
            let font_size = style.font_size.to_pixels(window.rem_size());
            let (text, runs) = first_line(&self.data, width, font_size, window.text_system());
            self.inner = gpui::StyledText::new(text).with_runs(runs.clone());
            let (layout, _) = self.inner.request_layout(id, inspector, window, cx);
            window.compute_layout(
                layout,
                size(
                    gpui::AvailableSpace::Definite(bounds.size.width),
                    gpui::AvailableSpace::MinContent,
                ),
                cx,
            );
            self.runs = Some(runs);
        }
        self.inner
            .prepaint(id, inspector, bounds, state, window, cx);
        let layout = self.inner.layout();
        let mut rows = Vec::new();
        let indent = style.text_indent;
        let drop = layout.line_layout_for_index(0).map_or(px(0.), |line| {
            first_row_drop(Some(&line), layout.line_height(), window)
        });
        if style.text_align != gpui::TextAlign::Left
            || !self.data.backgrounds.is_empty()
            || indent != px(0.)
            || self.justify.is_some()
            || drop > px(0.)
        {
            let mut offset = 0;
            while let Some(line) = layout.line_layout_for_index(offset) {
                let glyphs = &line.unwrapped_layout.runs;
                let mut left = px(0.);
                let mut start = offset;
                for boundary in line.wrap_boundaries.iter().map(Some).chain([None]) {
                    let (right, end) = boundary.map_or(
                        (line.unwrapped_layout.width, line.unwrapped_layout.width),
                        |boundary| {
                            let x = glyphs[boundary.run_ix].glyphs[boundary.glyph_ix].position.x;
                            let previous = boundary
                                .glyph_ix
                                .checked_sub(1)
                                .map(|glyph| &glyphs[boundary.run_ix].glyphs[glyph])
                                .or_else(|| {
                                    let run = boundary.run_ix.checked_sub(1)?;
                                    glyphs[run].glyphs.last()
                                });
                            let hanging = previous.filter(|glyph| {
                                self.data.text.as_bytes().get(offset + glyph.index) == Some(&b' ')
                            });
                            (x, hanging.map_or(x, |glyph| glyph.position.x))
                        },
                    );
                    // The text indent only applies to the first row of the text.
                    let indent = if rows.is_empty() { indent } else { px(0.) };
                    let free = (bounds.size.width - indent - (end - left)).max(px(0.));
                    let next = boundary.map_or(offset + line.len(), |boundary| {
                        offset + glyphs[boundary.run_ix].glyphs[boundary.glyph_ix].index
                    });
                    let gaps = spaces(self.data.text.get(start..next).unwrap_or("").trim_end());
                    let justified = match self.justify {
                        Some(last) => (boundary.is_some() || last) && gaps > 0.,
                        None => false,
                    };
                    let (shift, gap) = if justified {
                        (indent, free / gaps)
                    } else {
                        let shift = match style.text_align {
                            gpui::TextAlign::Left => px(0.),
                            gpui::TextAlign::Center => free / 2.,
                            gpui::TextAlign::Right => free,
                        };
                        (indent + shift, px(0.))
                    };
                    rows.push(crate::dom::Row {
                        shift,
                        right: shift + end - left + gap * gaps,
                        gap,
                        start,
                        drop,
                    });
                    left = right;
                    start = next;
                }
                offset += line.len() + 1;
                if offset > self.data.text.len() {
                    break;
                }
            }
        }
        let rows: crate::dom::Rows = rows.into();
        let hitboxes = self
            .data
            .cursors
            .iter()
            .filter_map(|&(index, cursor)| {
                let range = &self.data.ranges.get(index)?.1;
                let (a, b) = text_position(layout, &self.data.text, &rows, range.start)
                    .zip(text_position(layout, &self.data.text, &rows, range.end))?;
                let rect = if a.y == b.y {
                    Bounds::new(a, size(b.x - a.x, layout.line_height()))
                } else {
                    Bounds::new(
                        point(bounds.left(), a.y),
                        size(bounds.size.width, b.y - a.y + layout.line_height()),
                    )
                };
                Some((
                    window.insert_hitbox(rect, gpui::HitboxBehavior::Normal),
                    cursor,
                ))
            })
            .collect();
        let view = self.view.read(cx);
        let mut host = view.host.borrow_mut();
        let dom = host.dom.entry(view.id).or_default();
        let layout = self.inner.layout();
        for (id, range) in &self.data.ranges {
            if let Some((a, b)) = text_position(layout, &self.data.text, &rows, range.start)
                .zip(text_position(layout, &self.data.text, &rows, range.end))
            {
                dom.bounds.insert(
                    *id,
                    [
                        if a.y == b.y {
                            a.x.into()
                        } else {
                            bounds.left().into()
                        },
                        a.y.into(),
                        if a.y == b.y {
                            (b.x - a.x).into()
                        } else {
                            bounds.size.width.into()
                        },
                        (b.y - a.y + layout.line_height()).into(),
                    ],
                );
                dom.text_layouts.insert(
                    *id,
                    (
                        layout.clone(),
                        self.data.text.clone(),
                        range.clone(),
                        rows.clone(),
                    ),
                );
            }
        }
        (hitboxes, rows)
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        (hitboxes, rows): &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        for (hitbox, cursor) in hitboxes.iter() {
            window.set_cursor_style(*cursor, hitbox);
        }
        let mut highlights = Vec::new();
        {
            let view = self.view.read(cx);
            let host = view.host.borrow();
            let dom = &host.dom[&view.id];
            let layout = self.inner.layout();
            if let Some(styles) = &dom.computed {
                for &(index, ink) in &self.data.backgrounds {
                    let (id, range) = &self.data.ranges[index];
                    let style = &styles.values[*id];
                    let units = styles.lengths[*id];
                    if let Some((a, b)) = text_position(layout, &self.data.text, rows, range.start)
                        .zip(text_position(layout, &self.data.text, rows, range.end))
                    {
                        let font = cx.text_system().resolve_font(&render::font(style));
                        let height = cx.text_system().ascent(font, px(units.font.em))
                            - cx.text_system().descent(font, px(units.font.em));
                        let radii = ["top-left", "top-right", "bottom-right", "bottom-left"].map(
                            |corner| {
                                px(style
                                    .get(&format!("border-{corner}-radius"))
                                    .or_else(|| style.get("border-radius"))
                                    .and_then(|v| units.resolve(v, (b.x - a.x).into()))
                                    .unwrap_or(0.))
                            },
                        );
                        let (top, line_height) = (layout.bounds().top(), layout.line_height());
                        let [first, last] = [a.y, b.y].map(|y| {
                            (((y - top) / line_height).round() as usize)
                                .min(rows.len().saturating_sub(1))
                        });
                        let clone = style
                            .get("box-decoration-break")
                            .is_some_and(|value| value == "clone");
                        for row in first..=last {
                            let (start, end) = (row == first, row == last);
                            let extent = rows
                                .get(row)
                                .map_or((px(0.), layout.bounds().size.width), |row| {
                                    (row.shift, row.right)
                                });
                            let x = if start {
                                a.x
                            } else {
                                layout.bounds().left() + extent.0
                            };
                            let right = if end {
                                b.x
                            } else {
                                layout.bounds().left() + extent.1
                            };
                            let [left_side, right_side] = [start || clone, end || clone];
                            window.paint_quad(
                                gpui::fill(
                                    Bounds::new(
                                        point(
                                            x,
                                            top + line_height * row + (line_height - height) / 2.,
                                        ),
                                        size(right - x, height),
                                    ),
                                    rgba(ink),
                                )
                                .corner_radii(gpui::Corners {
                                    top_left: if left_side { radii[0] } else { px(0.) },
                                    top_right: if right_side { radii[1] } else { px(0.) },
                                    bottom_right: if right_side { radii[2] } else { px(0.) },
                                    bottom_left: if left_side { radii[3] } else { px(0.) },
                                }),
                            );
                        }
                    }
                }
            }
            for (id, range) in &self.data.ranges {
                let Some([start, end]) = dom.selection.get(id) else {
                    continue;
                };
                let Some(styles) = &dom.computed else {
                    continue;
                };
                if styles.user_select[*id].0 == render::UserSelect::None {
                    continue;
                }
                let caret = dom.selection_caret == Some(*id);
                let text = &self.data.text[range.clone()];
                let start = range.start + crate::dom::utf16_index(text, *start);
                let end = range.start + crate::dom::utf16_index(text, *end);
                if caret {
                    let ink = styles.values[*id]
                        .get("caret-color")
                        .filter(|value| !matches!(value.as_str(), "auto" | "currentColor"))
                        .or_else(|| styles.values[*id].get("color"))
                        .and_then(|value| render::color(value))
                        .map(|(color, alpha)| color << 8 | alpha as u32)
                        .unwrap_or(0x000000ff);
                    if let Some(position) = text_position(layout, &self.data.text, rows, start) {
                        window.paint_quad(gpui::fill(
                            Bounds::new(position, size(px(1.), layout.line_height())),
                            rgba(ink),
                        ));
                    }
                } else if start != end {
                    let (color, background) =
                        render::selection_colors(&styles.values[*id], styles.selections.get(id));
                    highlights.push(TextHighlight {
                        range: start.min(end)..start.max(end),
                        color: color.map(|color| rgba(color).into()),
                        background: rgba(background).into(),
                    });
                }
            }
        }
        let layout = self.inner.layout();
        let runs = self.runs.as_deref().unwrap_or(&self.data.runs);
        let passes = self
            .data
            .shadows
            .iter()
            .map(|&(x, y, blur, color)| (point(px(x), px(y)), Some(rgba(color).into()), blur))
            .chain([(point(px(0.), px(0.)), None, 0.)]);
        for (shift, ink, blur) in passes {
            let paint = |window: &mut Window| {
                let mut offset = 0;
                let mut row = 0;
                let mut origin = layout.bounds().origin + shift;
                while let Some(line) = layout.line_layout_for_index(offset) {
                    let count = line.wrap_boundaries.len() + 1;
                    paint_text(
                        &line.unwrapped_layout,
                        &line.wrap_boundaries,
                        origin,
                        layout.line_height(),
                        runs,
                        offset,
                        if ink.is_some() { &[] } else { &highlights },
                        rows.get(row..row + count).unwrap_or_default(),
                        &self.data.text,
                        ink,
                        window,
                    );
                    row += count;
                    offset += line.len() + 1;
                    origin.y += line.size(layout.line_height()).height;
                    if offset > self.data.text.len() {
                        break;
                    }
                }
            };
            if blur > 0. {
                // The CSS blur radius is twice the gaussian deviation.
                let effect = gpui::PaintEffect {
                    blur: px(blur / 2.),
                    ..Default::default()
                };
                window.paint_effect(effect, paint);
            } else {
                paint(window);
            }
        }
    }
}

type PaintLayers = Rc<
    RefCell<
        Vec<(
            i32,
            Rc<RefCell<AnyElement>>,
            gpui::ContentMask<Pixels>,
            Rc<[usize]>,
        )>,
    >,
>;

struct Stacked {
    inner: Option<AnyElement>,
    layers: PaintLayers,
    order: i32,
    path: Rc<[usize]>,
}

impl IntoElement for Stacked {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for Stacked {
    type RequestLayoutState = Rc<RefCell<AnyElement>>;
    type PrepaintState = ();
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let mut inner = self.inner.take().unwrap();
        let id = inner.request_layout(window, cx);
        (id, Rc::new(RefCell::new(inner)))
    }
    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        inner: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        inner.borrow_mut().prepaint(window, cx);
        self.layers.borrow_mut().push((
            self.order,
            inner.clone(),
            window.content_mask(),
            self.path.clone(),
        ));
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
}

/// Re-enters the element id scopes between a stacking root and a stacked element, so paint sees the prepaint ids.
fn with_path(path: &[usize], window: &mut Window, paint: &mut dyn FnMut(&mut Window)) {
    match path.split_first() {
        Some((&id, rest)) => window.with_element_namespace(
            gpui::ElementId::Name(format!("dom-{id}").into()),
            |window| window.with_element_namespace(id, |window| with_path(rest, window, paint)),
        ),
        None => paint(window),
    }
}

fn paint_layers(layers: PaintLayers, negative: bool) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| (),
        move |_, _, window, cx| {
            let mut layers = layers.borrow_mut();
            layers.sort_by_key(|(order, ..)| *order);
            for (_, inner, mask, path) in
                layers.iter().filter(|(order, ..)| (*order < 0) == negative)
            {
                window.with_content_mask(Some(*mask), |window| {
                    with_path(path, window, &mut |window| {
                        inner.borrow_mut().paint(window, cx)
                    })
                });
            }
        },
    )
    .absolute()
    .w(px(0.))
    .h(px(0.))
}

struct Build<'a> {
    doc: &'a Document,
    styles: &'a ComputedStyles,
    text: &'a gpui::WindowTextSystem,
    pixel_ratio: f32,
    view: &'a WebView,
    layers: PaintLayers,
    path: Rc<[usize]>,
    flex_parent: bool,
    measured: &'a RefCell<crate::grid::Measurements>,
    inline_text: &'a RefCell<HashMap<usize, Option<Rc<render::InlineText>>>>,
    /// The parent's `perspective` distance and `perspective-origin`.
    perspective: Option<(f32, String)>,
}

fn element(
    build: &Build<'_>,
    id: usize,
    containing: [f32; 2],
    container: [f32; 2],
    slot: Option<([f32; 4], crate::grid::Grid)>,
    cx: &mut Context<WebView>,
) -> AnyElement {
    let Build {
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        ref layers,
        ref path,
        flex_parent,
        measured,
        inline_text,
        perspective: ref parent_perspective,
    } = *build;
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    let display = style.get("display").map_or(
        if node.tag.starts_with('#') && node.tag != "#text" {
            "block"
        } else {
            "inline"
        },
        String::as_str,
    );
    let order = style
        .get("z-index")
        .and_then(|value| value.parse::<i32>().ok());
    let positioned = style.get("position").is_some_and(|value| value != "static");
    let stacking = id == 0
        || style
            .get("isolation")
            .is_some_and(|value| value == "isolate")
        || style
            .get("position")
            .is_some_and(|value| matches!(value.as_str(), "fixed" | "sticky"))
        || positioned && order.is_some()
        || style.get("transform").is_some_and(|value| value != "none")
        || ["filter", "clip-path"]
            .iter()
            .any(|key| style.get(*key).is_some_and(|value| value != "none"))
        || style
            .get("opacity")
            .and_then(|value| value.parse::<f32>().ok())
            .is_some_and(|value| value < 1.);
    let child_build = Build {
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        measured,
        inline_text,
        layers: if stacking {
            Rc::default()
        } else {
            layers.clone()
        },
        path: if stacking {
            Rc::default()
        } else {
            path.iter().copied().chain([id]).collect()
        },
        flex_parent: matches!(display, "flex" | "inline-flex"),
        perspective: style
            .get("perspective")
            .filter(|value| *value != "none")
            .and_then(|value| styles.lengths[id].resolve(value, 0.))
            .map(|distance| {
                (
                    distance,
                    style
                        .get("perspective-origin")
                        .cloned()
                        .unwrap_or_else(|| "50% 50%".into()),
                )
            }),
    };
    let build = &child_build;
    let mut units = styles.lengths[id];
    units.container = Some(container);
    let mut ancestors = vec![id];
    let mut parent = node.parent;
    while let Some(id) = parent {
        ancestors.push(id);
        parent = doc.nodes[id].parent;
    }
    // WARNING: element building and gpui layout recurse per level, so deep pages must not overflow the stack.
    if ancestors.len() > 256 || style.get("display").is_some_and(|s| s == "none") {
        return div().hidden().into_any_element();
    }
    let opacity = style
        .get("opacity")
        .and_then(|value| {
            value
                .strip_suffix('%')
                .map_or(value.parse::<f32>().ok(), |v| {
                    v.parse::<f32>().ok().map(|v| v / 100.)
                })
        })
        .filter(|v| v.is_finite())
        .unwrap_or(1.)
        .clamp(0., 1.);
    // Group opacity and these properties paint the box through an offscreen layer.
    let effect_layer = ([
        "filter",
        "clip-path",
        "mask-image",
        "-webkit-mask-image",
        "backdrop-filter",
        "-webkit-backdrop-filter",
    ]
    .iter()
    .any(|name| style.get(*name).is_some_and(|value| value != "none"))
        || style
            .get("mix-blend-mode")
            .is_some_and(|value| value != "normal")
        || style
            .get("isolation")
            .is_some_and(|value| value == "isolate")
        || opacity < 1. && !node.children.is_empty())
    .then_some(opacity);
    let selection_colors = render::selection_colors(style, styles.selections.get(&id));
    let face = render::font(style);
    let mut el = div()
        .id(id)
        .flex()
        .flex_col()
        .text_size(px(units.font.em))
        .font_family(face.family)
        .font_weight(face.weight)
        .line_height(px(units.font.lh));
    el.style().flex_shrink = Some(if flex_parent { 1. } else { 0. });
    {
        let text = el.text_style();
        text.letter_spacing = Some(render::spacing(style, &units, "letter-spacing"));
        text.word_spacing = Some(render::spacing(style, &units, "word-spacing"));
        text.text_indent = Some(px(style
            .get("text-indent")
            .and_then(|value| units.resolve(value, containing[0]))
            .unwrap_or(0.)));
    }
    if face.style != gpui::FontStyle::Normal {
        el.text_style().font_style = Some(face.style);
    }
    if node.tag == "br" {
        return div().w_full().h(px(0.)).into_any_element();
    }
    if stacking {
        el = el.child(paint_layers(build.layers.clone(), true));
    }
    let flow = !matches!(display, "flex" | "inline-flex" | "grid" | "inline-grid");
    let mut flat = Vec::new();
    let mut pending: Vec<usize> = node.children.iter().rev().copied().collect();
    while let Some(child) = pending.pop() {
        if styles.values[child]
            .get("display")
            .is_some_and(|value| value == "contents")
        {
            pending.extend(doc.nodes[child].children.iter().rev());
        } else if doc.nodes[child].tag != "#comment" {
            flat.push(child);
        }
    }
    let floated = |child: usize| {
        flow && float_side(&styles.values[child]).is_some()
            && styles.values[child]
                .get("position")
                .is_none_or(|value| !matches!(value.as_str(), "absolute" | "fixed"))
    };
    let inline_level = |child: usize| {
        doc.nodes[child].tag.starts_with('#')
            || !floated(child)
                && styles.values[child]
                    .get("display")
                    .is_none_or(|value| value.starts_with("inline"))
            || styles.values[child]
                .get("position")
                .is_some_and(|value| matches!(value.as_str(), "absolute" | "fixed"))
    };
    let all_inline = flow && !flat.is_empty() && flat.iter().all(|&child| inline_level(child));
    let line_justify = match style.get("text-align").map(String::as_str) {
        Some("right" | "end") => Some(gpui::JustifyContent::End),
        Some("center" | "-webkit-center") => Some(gpui::JustifyContent::Center),
        _ => None,
    };
    let line_align = Some(
        if flat.iter().any(|&child| {
            styles.values[child]
                .get("vertical-align")
                .is_some_and(|value| value == "middle")
        }) {
            gpui::AlignItems::Center
        } else {
            gpui::AlignItems::Baseline
        },
    );
    if all_inline || display == "inline" {
        el = el.flex_row().flex_wrap();
        el.style().justify_content = line_justify;
        el.style().align_items = line_align;
    }
    if node.tag == "canvas" {
        el = el
            .w(px(node
                .attrs
                .get("width")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300.)))
            .h(px(node
                .attrs
                .get("height")
                .and_then(|v| v.parse().ok())
                .unwrap_or(150.)));
        if let Some(image) = view.images.get(&id) {
            el = el.child(img(image.clone()).size_full());
        }
        if let Some(image) = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|state| state.canvases.get(&id))
            .map(|canvas| canvas.image.clone())
        {
            el = el.child(img(image).absolute().size_full());
        }
    }
    let mut image_child = None;
    let mut cropped = None;
    if node.tag == "img" {
        let (image, vector) = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .map_or((None, None), |state| {
                (
                    state.images.get(&id).cloned(),
                    state.svg_images.get(&id).cloned(),
                )
            });
        if let Some(natural) = image
            .as_ref()
            .map(|image| {
                let size = image.size(0);
                [u32::from(size.width) as f32, u32::from(size.height) as f32]
            })
            .or(vector.as_ref().map(|(_, natural)| *natural))
        {
            let attr = |name: &str| {
                node.attrs
                    .get(name)
                    .and_then(|value| value.parse::<f32>().ok())
                    .filter(|value| value.is_finite() && *value >= 0.)
            };
            let auto = |name: &str| style.get(name).is_none_or(|value| value == "auto");
            if natural[1] > 0. {
                el.style().aspect_ratio = Some(natural[0] / natural[1]);
            }
            match (auto("width"), auto("height"), attr("width"), attr("height")) {
                (true, _, Some(width), _) => el = el.w(px(width)),
                (true, true, None, None) => el = el.w(px(natural[0])),
                _ => {}
            }
            if auto("height")
                && let Some(height) = attr("height")
            {
                el = el.h(px(height));
            }
            let fit = match style.get("object-fit").map(String::as_str) {
                Some("contain") => gpui::ObjectFit::Contain,
                Some("cover") => gpui::ObjectFit::Cover,
                Some("none") => gpui::ObjectFit::None,
                Some("scale-down") => gpui::ObjectFit::ScaleDown,
                _ => gpui::ObjectFit::Fill,
            };
            if let Some(image) = image {
                if matches!(fit, gpui::ObjectFit::Cover | gpui::ObjectFit::None) {
                    cropped = Some((image, fit));
                } else {
                    image_child = Some(img(image).size_full().object_fit(fit));
                }
            } else if let Some((source, _)) = vector {
                el = el.child(svg_layer(source.to_string(), id, cx.entity()));
            }
        } else if let Some(path) = node.attrs.get("src").and_then(|source| {
            let base = view
                .host
                .borrow()
                .dom
                .get(&view.id)
                .and_then(|state| state.base.clone())?;
            base.join(source)
                .ok()
                .filter(|url| url.scheme() == "file")?
                .to_file_path()
                .ok()
        }) {
            let auto = |name: &str| style.get(name).is_none_or(|value| value == "auto");
            for (name, horizontal) in [("width", true), ("height", false)] {
                if auto(name)
                    && let Some(length) = node
                        .attrs
                        .get(name)
                        .and_then(|value| value.parse::<f32>().ok())
                        .filter(|value| value.is_finite() && *value >= 0.)
                {
                    el = if horizontal {
                        el.w(px(length))
                    } else {
                        el.h(px(length))
                    };
                }
            }
            let sized = [
                !auto("width") || node.attrs.contains_key("width"),
                !auto("height") || node.attrs.contains_key("height"),
            ];
            let image = img(path).object_fit(match style.get("object-fit").map(String::as_str) {
                Some("contain") => gpui::ObjectFit::Contain,
                Some("cover") => gpui::ObjectFit::Cover,
                Some("none") => gpui::ObjectFit::None,
                Some("scale-down") => gpui::ObjectFit::ScaleDown,
                _ => gpui::ObjectFit::Fill,
            });
            el = el.overflow_hidden();
            image_child = Some(match sized {
                [true, true] => image.size_full(),
                [true, false] => image.w_full(),
                [false, true] => image.h_full(),
                _ => image,
            });
        }
    }
    if node.tag == "svg" {
        let width = style
            .get("width")
            .and_then(|v| units.resolve(v, containing[0]))
            .or_else(|| node.attrs.get("width").and_then(|v| v.parse().ok()))
            .unwrap_or(24.)
            .clamp(1., 4096.);
        let height = style
            .get("height")
            .and_then(|v| units.resolve(v, containing[1]))
            .or_else(|| node.attrs.get("height").and_then(|v| v.parse().ok()))
            .unwrap_or(24.)
            .clamp(1., 4096.);
        let cached = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|state| state.svg_sources.get(&id))
            .filter(|(size, color, _)| {
                *size == [width, height]
                    && color == &style.get("color").cloned().unwrap_or_default()
            })
            .map(|(_, _, source)| source.clone());
        let source = if let Some(source) = cached {
            source
        } else {
            let mut source = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\""
            );
            for (key, value) in &node.attrs {
                if !matches!(
                    key.as_str(),
                    "width" | "height" | "xmlns" | "xmlns:xlink" | "style"
                ) && !style.contains_key(key)
                {
                    source.push_str(&format!(" {key}=\"{}\"", crate::html::escape(value)));
                }
            }
            source.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
            for key in [
                "fill",
                "fill-opacity",
                "fill-rule",
                "stroke",
                "stroke-width",
                "stroke-opacity",
                "stroke-linecap",
                "stroke-linejoin",
                "color",
            ] {
                if let Some(value) = style.get(key) {
                    source.push_str(&format!(" {key}=\"{}\"", crate::html::escape(value)));
                }
            }
            source.push('>');
            let mut scope = id;
            while let Some(parent) = doc.nodes[scope].parent {
                scope = parent;
            }
            let mut pending = vec![id];
            let mut visited = std::collections::HashSet::new();
            source.push_str("<defs>");
            while let Some(child) = pending.pop() {
                if !visited.insert(child) {
                    continue;
                }
                let child = &doc.nodes[child];
                pending.extend(&child.children);
                if child.tag == "use"
                    && let Some(reference) = child
                        .attrs
                        .get("href")
                        .or_else(|| child.attrs.get("xlink:href"))
                        .and_then(|value| value.strip_prefix('#'))
                    && let Some((target, _)) =
                        doc.nodes.iter().enumerate().find(|(target, node)| {
                            if !node.attrs.get("id").is_some_and(|value| value == reference) {
                                return false;
                            }
                            let mut root = *target;
                            while let Some(parent) = doc.nodes[root].parent {
                                if parent == id {
                                    return false;
                                }
                                root = parent;
                            }
                            root == scope
                        })
                    && !visited.contains(&target)
                {
                    source.push_str(&doc.serialize(target));
                    pending.push(target);
                }
            }
            source.push_str("</defs>");
            for child in &node.children {
                source.push_str(&doc.serialize(*child));
            }
            source.push_str("</svg>");
            source = source.replace(
                "currentColor",
                style.get("color").map(String::as_str).unwrap_or("black"),
            );
            view.host
                .borrow_mut()
                .dom
                .entry(view.id)
                .or_default()
                .svg_sources
                .insert(
                    id,
                    (
                        [width, height],
                        style.get("color").cloned().unwrap_or_default(),
                        source.clone(),
                    ),
                );
            source
        };
        el = el
            .w(px(width))
            .h(px(height))
            .child(svg_layer(source, id, cx.entity()));
    }
    if node.tag == "iframe" {
        el = el.min_w(px(300.)).min_h(px(150.));
    }
    let input =
        (node.tag == "input").then(|| node.attrs.get("type").map_or("text", String::as_str));
    let check = input.is_some_and(|kind| {
        kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio")
    });
    let editable = node.tag == "textarea"
        || input.is_some_and(|kind| {
            ![
                "checkbox", "radio", "button", "submit", "reset", "image", "file", "color",
                "range", "hidden",
            ]
            .iter()
            .any(|other| kind.eq_ignore_ascii_case(other))
        });
    if input.is_some_and(|kind| {
        ["button", "submit", "reset"]
            .iter()
            .any(|other| kind.eq_ignore_ascii_case(other))
    }) {
        el = el.items_center().justify_center().child(SharedString::from(
            node.attrs.get("value").cloned().unwrap_or_else(|| {
                if input == Some("reset") {
                    "Reset".into()
                } else if input == Some("submit") {
                    "Submit".into()
                } else {
                    String::new()
                }
            }),
        ));
    }
    if editable {
        if !style.contains_key("width") {
            el = el.w(px(180.));
        }

        let text = node.attrs.get("value").cloned().unwrap_or_else(|| {
            if node.tag == "textarea" {
                doc.text(id)
            } else {
                String::new()
            }
        });
        let placeholder = node.attrs.get("placeholder").cloned().unwrap_or_default();
        let face = render::font(style);
        let font_size = units.font.em;
        let line_height = units.font.lh;
        let text_color = render::color(&style["color"])
            .map(|(c, a)| rgba((c << 8) | a as u32))
            .unwrap_or(gpui::black().into());
        let caret_color = style
            .get("caret-color")
            .filter(|value| !matches!(value.as_str(), "auto" | "currentColor"))
            .and_then(|value| render::color(value))
            .map(|(color, alpha)| rgba((color << 8) | alpha as u32))
            .unwrap_or(text_color);
        let placeholder_color = rgba(0x888888ff);
        let entity = cx.entity();
        el = el.cursor(gpui::CursorStyle::IBeam).child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    let view = entity.read(cx);
                    let host = view.host.borrow();
                    let state = host.dom.get(&view.id);
                    let selected = state.and_then(|s| s.selection.get(&id).copied());
                    let focused = state.and_then(|s| s.focus) == Some(id);
                    drop(host);
                    let shown = if text.is_empty() {
                        placeholder.clone()
                    } else {
                        text.clone()
                    };
                    let color = if text.is_empty() {
                        placeholder_color
                    } else {
                        text_color
                    };
                    let mut offset = 0;
                    for (row, text) in shown.split('\n').enumerate() {
                        let run = gpui::TextRun {
                            len: text.len(),
                            font: face.clone(),
                            color: color.into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                            letter_spacing: gpui::px(0.),
                            word_spacing: gpui::px(0.),
                            font_size: None,
                        };
                        let line = window.text_system().shape_line(
                            text.to_owned().into(),
                            px(font_size),
                            std::slice::from_ref(&run),
                            None,
                        );
                        let length = text.encode_utf16().count();
                        let top = bounds.top() + px(row as f32 * line_height);
                        let mut highlights = Vec::new();
                        if focused
                            && let Some([start, end]) = selected
                            && start.min(end) <= offset + length
                            && start.max(end) >= offset
                        {
                            let left = crate::dom::utf16_index(
                                text,
                                start.min(end).saturating_sub(offset),
                            );
                            let right = crate::dom::utf16_index(
                                text,
                                start.max(end).saturating_sub(offset),
                            );
                            let x = line.x_for_index(left);
                            let end_x = line.x_for_index(right);
                            if start == end {
                                window.paint_quad(gpui::fill(
                                    Bounds::new(
                                        point(bounds.left() + x, top),
                                        size(px(1.), px(line_height)),
                                    ),
                                    caret_color,
                                ));
                            } else if end_x > x {
                                highlights.push(TextHighlight {
                                    range: left..right,
                                    color: selection_colors.0.map(|color| rgba(color).into()),
                                    background: rgba(selection_colors.1).into(),
                                });
                            }
                        }
                        if highlights.is_empty() {
                            let _ = line.paint(
                                point(bounds.left(), top),
                                px(line_height),
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        } else {
                            paint_text(
                                &line,
                                &[],
                                point(bounds.left(), top),
                                px(line_height),
                                std::slice::from_ref(&run),
                                0,
                                &highlights,
                                &[],
                                text,
                                None,
                                window,
                            );
                        }
                        offset += length + 1;
                    }
                },
            )
            .absolute()
            .size_full(),
        );
    }
    if let Some((ink, alpha)) = render::color(&style["color"]) {
        el = el.text_color(rgba((ink << 8) | alpha as u32));
    }
    if let Some(color) = style
        .get("background-color")
        .and_then(|s| render::color(s))
        .filter(|(_, alpha)| *alpha > 0)
    {
        let color = rgba((color.0 << 8) | color.1 as u32);
        let blended = style
            .get("background-blend-mode")
            .is_some_and(|value| value.split(',').any(|mode| mode.trim() != "normal"))
            && style
                .get("background-image")
                .is_some_and(|value| value != "none");
        if blended {
            // The color is painted inside the blended background group.
        } else if display == "inline"
            && !node.tag.starts_with('#')
            && !matches!(
                node.tag.as_str(),
                "img" | "svg" | "canvas" | "input" | "button" | "select" | "textarea" | "iframe"
            )
            && !style
                .get("position")
                .is_some_and(|value| value == "absolute")
        {
            let text = cx.text_system();
            let font = text.resolve_font(&render::font(style));
            let font_size = px(units.font.em);
            let height = text.ascent(font, font_size) - text.descent(font, font_size);
            let line_height = px(units.font.lh);
            let radius = px(style
                .get("border-radius")
                .and_then(|value| units.resolve(value, containing[0]))
                .unwrap_or(0.));
            el = el.child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let mut top = bounds.top() + (line_height - height) / 2.;
                        while top < bounds.bottom() {
                            window.paint_quad(
                                gpui::fill(
                                    Bounds::new(
                                        point(bounds.left(), top),
                                        size(bounds.size.width, height),
                                    ),
                                    color,
                                )
                                .corner_radii(radius),
                            );
                            top += line_height.max(px(1.));
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        } else {
            el = el.bg(color);
        }
    }
    let padding = units
        .edges(style, "padding", containing[0])
        .map(|value| value.max(0.));
    el = el
        .pt(px(padding[0]))
        .pr(px(padding[1]))
        .pb(px(padding[2]))
        .pl(px(padding[3]));
    let mut margin = units.edges(style, "margin", containing[0]);
    let edge = |id: usize, name: &str| {
        styles.values[id]
            .get(name)
            .and_then(|value| styles.lengths[id].resolve(value, containing[0]))
            .unwrap_or(0.)
    };
    let in_flow = |id: usize| {
        let (node, style) = (&doc.nodes[id], &styles.values[id]);
        node.tag != "#comment"
            && !(node.tag == "#text" && node.text.trim().is_empty())
            && style.get("display").is_none_or(|value| value != "none")
            && style
                .get("position")
                .is_none_or(|value| !matches!(value.as_str(), "absolute" | "fixed"))
    };
    let block = |id: usize| {
        !doc.nodes[id].tag.starts_with('#')
            && styles.values[id]
                .get("display")
                .is_some_and(|value| !value.starts_with("inline") && value != "contents")
    };
    // WARNING: margins only collapse through plain block boxes; flex/grid items and scroll containers keep their own.
    let collapses = |id: usize, side: usize| {
        let style = &styles.values[id];
        let [padding, border, kind] = if side == 0 {
            ["padding-top", "border-top-width", "border-top-style"]
        } else {
            [
                "padding-bottom",
                "border-bottom-width",
                "border-bottom-style",
            ]
        };
        id != 0
            && doc.nodes[id].tag != "html"
            && doc.nodes[id].parent.is_none_or(|parent| {
                !styles.values[parent].get("display").is_some_and(|value| {
                    matches!(
                        value.as_str(),
                        "flex" | "inline-flex" | "grid" | "inline-grid"
                    )
                })
            })
            && style
                .get("display")
                .is_some_and(|value| matches!(value.as_str(), "block" | "list-item"))
            && ["overflow-x", "overflow-y"]
                .iter()
                .all(|key| style.get(*key).is_none_or(|value| value == "visible"))
            && style
                .get("position")
                .is_none_or(|value| !matches!(value.as_str(), "absolute" | "fixed"))
            && edge(id, padding) == 0.
            && (edge(id, border) == 0.
                || style
                    .get(kind)
                    .is_none_or(|value| matches!(value.as_str(), "none" | "hidden")))
            && (side == 0 || style.get("height").is_none_or(|value| value == "auto"))
    };
    if block(id)
        && in_flow(id)
        && slot.is_none()
        && !flex_parent
        && let Some(parent) = node.parent
    {
        let siblings: Vec<usize> = doc.nodes[parent]
            .children
            .iter()
            .copied()
            .filter(|&child| in_flow(child))
            .collect();
        let index = siblings.iter().position(|&child| child == id);
        if let Some(previous) = index
            .and_then(|index| index.checked_sub(1))
            .map(|index| siblings[index])
            .filter(|&previous| block(previous))
        {
            let (a, b) = (margin[0], edge(previous, "margin-bottom"));
            margin[0] = if a >= 0. && b >= 0. {
                a.max(b)
            } else if a < 0. && b < 0. {
                a.min(b)
            } else {
                a + b
            } - b;
        }
        if index == Some(0)
            && margin[0] >= 0.
            && edge(parent, "margin-top") >= 0.
            && collapses(parent, 0)
        {
            margin[0] = 0.;
        }
        if index.is_some_and(|index| index + 1 == siblings.len())
            && margin[2] >= 0.
            && edge(parent, "margin-bottom") >= 0.
            && collapses(parent, 2)
        {
            margin[2] = 0.;
        }
    }
    let children: Vec<usize> = node
        .children
        .iter()
        .copied()
        .filter(|&child| in_flow(child))
        .collect();
    if let Some(&first) = children
        .first()
        .filter(|&&first| block(first) && margin[0] >= 0. && collapses(id, 0))
    {
        margin[0] = margin[0].max(edge(first, "margin-top"));
    }
    if let Some(&last) = children
        .last()
        .filter(|&&last| block(last) && margin[2] >= 0. && collapses(id, 2))
    {
        margin[2] = margin[2].max(edge(last, "margin-bottom"));
    }
    el = el
        .mt(px(margin[0]))
        .mr(px(margin[1]))
        .mb(px(margin[2]))
        .ml(px(margin[3]));
    for side in ["top", "right", "bottom", "left"] {
        if style
            .get(&format!("margin-{side}"))
            .is_some_and(|s| s == "auto")
        {
            el = match side {
                "top" => el.mt_auto(),
                "right" => el.mr_auto(),
                "bottom" => el.mb_auto(),
                _ => el.ml_auto(),
            };
        }
    }
    let ink = render::color(&style["color"]).map_or(rgba(0x000000ff), |(ink, alpha)| {
        rgba(ink << 8 | alpha as u32)
    });
    let sides = [
        ("border-top-width", "border-top-style", "border-top-color"),
        (
            "border-right-width",
            "border-right-style",
            "border-right-color",
        ),
        (
            "border-bottom-width",
            "border-bottom-style",
            "border-bottom-color",
        ),
        (
            "border-left-width",
            "border-left-style",
            "border-left-color",
        ),
    ];
    let borders = sides.map(|(width, kind, _)| {
        if style
            .get(kind)
            .is_none_or(|kind| matches!(kind.as_str(), "none" | "hidden"))
        {
            return 0.;
        }
        style
            .get(width)
            .map_or(Some(3.), |width| units.resolve(width, containing[0]))
            .unwrap_or(0.)
            .max(0.)
    });
    let border_colors = sides.map(|(_, _, name)| {
        style
            .get(name)
            .and_then(|value| render::color(value))
            .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32))
    });
    el = el
        .border_t(px(borders[0]))
        .border_r(px(borders[1]))
        .border_b(px(borders[2]))
        .border_l(px(borders[3]));
    let visible = (0..4)
        .filter(|&side| borders[side] > 0.)
        .collect::<Vec<_>>();
    let border_kinds = sides.map(|(_, kind, _)| style.get(kind).map_or("none", String::as_str));
    let shaded = visible
        .iter()
        .any(|&side| matches!(border_kinds[side], "groove" | "ridge" | "inset" | "outset"));
    if let Some(&first) = visible.first() {
        if !shaded
            && visible
                .iter()
                .all(|&side| border_colors[side] == border_colors[first])
        {
            el = el.border_color(border_colors[first]);
        } else {
            el = el.border_color(gpui::transparent_black());
        }
        if visible
            .iter()
            .all(|&side| border_kinds[side] == border_kinds[first])
        {
            el.style().border_style = Some(border_style(border_kinds[first]));
        } else if visible
            .iter()
            .any(|&side| matches!(border_kinds[side], "dashed" | "dotted"))
        {
            el.style().border_style = Some(gpui::BorderStyle::Dashed);
        }
    }
    let extra = if style
        .get("box-sizing")
        .is_some_and(|value| value == "border-box")
    {
        [0.; 2]
    } else {
        [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ]
    };
    if editable && style.get("height").is_none_or(|value| value == "auto") {
        el = el.min_h(px(if node.tag == "textarea" {
            2. * units.font.lh
        } else {
            units.font.lh
        } + extra[1]));
    }
    let flexbox = matches!(display, "flex" | "inline-flex");
    let position = style.get("position").map_or("static", String::as_str);
    let mut radius = [0.; 4];
    let mut inner_shadows = Vec::new();
    let mut intrinsic_width = None;
    for (name, value) in
        style.range::<str, _>((std::ops::Bound::Included("a"), std::ops::Bound::Unbounded))
    {
        let value = value.as_str();
        let vertical = name.ends_with("height") || matches!(name.as_str(), "top" | "bottom");
        let base = containing[usize::from(vertical)];
        let length = || units.resolve(value, base);
        match name.as_str() {
            "width" | "height" | "min-width" | "max-width" | "min-height" | "max-height" => {
                let axis = usize::from(vertical);
                let length: gpui::Length = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    if extra[axis] > 0. && base > 0. {
                        px(percent.max(0.) / 100. * base + extra[axis]).into()
                    } else {
                        relative(percent.max(0.) / 100.).into()
                    }
                } else if let Some(length) = length() {
                    px(length.max(0.) + extra[axis]).into()
                } else if matches!(name.as_str(), "width" | "height") && value == "auto" {
                    gpui::Length::Auto
                } else if matches!(value, "stretch" | "-webkit-fill-available") {
                    relative(1.).into()
                } else if matches!(value, "fit-content" | "max-content" | "min-content")
                    && axis == 0
                {
                    let [min, max, ..] = crate::grid::intrinsic(
                        doc,
                        id,
                        styles,
                        Some(containing[0]),
                        None,
                        text,
                        &mut measured.borrow_mut(),
                    );
                    let size = match value {
                        "min-content" => min,
                        "max-content" => max,
                        _ => max.min(min.max(containing[0])),
                    };
                    intrinsic_width = Some((size - margin[1] - margin[3]).max(0.));
                    px((size - margin[1] - margin[3]).max(0.)).into()
                } else {
                    continue;
                };
                el = match name.as_str() {
                    "width" => el.w(length),
                    "height" => el.h(length),
                    "min-width" => el.min_w(length),
                    "max-width" => el.max_w(length),
                    "min-height" => el.min_h(length),
                    _ => el.max_h(length),
                };
            }
            "row-gap" | "column-gap" if flexbox => {
                let Some(length) = length() else { continue };
                el = if name == "row-gap" {
                    el.gap_y(px(length.max(0.)))
                } else {
                    el.gap_x(px(length.max(0.)))
                };
            }
            "border-top-left-radius"
            | "border-top-right-radius"
            | "border-bottom-right-radius"
            | "border-bottom-left-radius" => {
                let own = style
                    .get("width")
                    .and_then(|width| units.resolve(width, containing[0]))
                    .unwrap_or(containing[0]);
                let Some(length) = units.resolve(value, own) else {
                    continue;
                };
                let length = length.max(0.);
                let corner = match name.as_str() {
                    "border-top-left-radius" => 0,
                    "border-top-right-radius" => 1,
                    "border-bottom-right-radius" => 2,
                    _ => 3,
                };
                radius[corner] = length;
                el = match corner {
                    0 => el.rounded_tl(px(length)),
                    1 => el.rounded_tr(px(length)),
                    2 => el.rounded_br(px(length)),
                    _ => el.rounded_bl(px(length)),
                };
            }
            "box-shadow" => {
                el.style().box_shadow = Some(units.shadows(value, false));
                inner_shadows = units.shadows(value, true);
            }
            "aspect-ratio" => {
                let value = value.replace('/', " / ");
                let parts = render::components(&value, ' ');
                let numbers: Vec<f32> = parts.iter().filter_map(|part| part.parse().ok()).collect();
                let ratio = match numbers.as_slice() {
                    [a, b, ..] if parts.contains(&"/") => a / b,
                    [a, ..] => *a,
                    _ => continue,
                };
                if ratio.is_finite() && ratio > 0. {
                    el.style().aspect_ratio = Some(ratio);
                }
            }
            "display" if matches!(value, "grid" | "inline-grid") => el = el.grid(),
            "display" if matches!(value, "flex" | "inline-flex") => el = el.flex().flex_row(),
            "flex-direction" if flexbox => {
                el.style().flex_direction = Some(match value {
                    "row-reverse" => gpui::FlexDirection::RowReverse,
                    "column" => gpui::FlexDirection::Column,
                    "column-reverse" => gpui::FlexDirection::ColumnReverse,
                    _ => gpui::FlexDirection::Row,
                })
            }
            "flex-wrap" if flexbox => {
                el.style().flex_wrap = Some(match value {
                    "wrap" => gpui::FlexWrap::Wrap,
                    "wrap-reverse" => gpui::FlexWrap::WrapReverse,
                    _ => gpui::FlexWrap::NoWrap,
                })
            }
            "flex-grow" | "flex-shrink" => {
                if let Some(factor) = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.)
                {
                    if name == "flex-grow" {
                        el.style().flex_grow = Some(factor);
                    } else {
                        el.style().flex_shrink = Some(factor);
                    }
                }
            }
            "flex-basis" => {
                el.style().flex_basis = Some(
                    if let Some(percent) = value
                        .strip_suffix('%')
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                    {
                        relative(percent.max(0.) / 100.).into()
                    } else if let Some(length) = length() {
                        px(length.max(0.) + extra[0]).into()
                    } else {
                        gpui::Length::Auto
                    },
                )
            }
            "align-items" | "align-self" if flex_parent || flexbox => {
                let align = match value {
                    "center" => gpui::AlignItems::Center,
                    "flex-start" | "start" | "self-start" => gpui::AlignItems::FlexStart,
                    "flex-end" | "end" | "self-end" => gpui::AlignItems::FlexEnd,
                    "baseline" | "first baseline" | "last baseline" => gpui::AlignItems::Baseline,
                    "stretch" | "normal" => gpui::AlignItems::Stretch,
                    _ => continue,
                };
                if name == "align-self" {
                    el.style().align_self = Some(align);
                } else if flexbox {
                    el.style().align_items = Some(align);
                }
            }
            "align-content" | "justify-content" if flexbox => {
                let align = match value {
                    "center" => gpui::AlignContent::Center,
                    "flex-end" | "end" | "right" => gpui::AlignContent::FlexEnd,
                    "space-between" => gpui::AlignContent::SpaceBetween,
                    "space-around" => gpui::AlignContent::SpaceAround,
                    "space-evenly" => gpui::AlignContent::SpaceEvenly,
                    "stretch" => gpui::AlignContent::Stretch,
                    _ => gpui::AlignContent::FlexStart,
                };
                if name == "justify-content" {
                    el.style().justify_content = Some(align);
                } else {
                    el.style().align_content = Some(align);
                }
            }
            "text-align" => {
                el = match value {
                    "center" => el.text_center(),
                    "right" | "end" => el.text_right(),
                    _ => el.text_left(),
                }
            }
            "position" if matches!(value, "absolute" | "fixed") => el = el.absolute(),
            "left" | "right" | "top" | "bottom" if matches!(position, "relative" | "absolute") => {
                let length: gpui::Length = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    relative(percent / 100.).into()
                } else if let Some(length) = length() {
                    px(length).into()
                } else {
                    continue;
                };
                el = match name.as_str() {
                    "left" => el.left(length),
                    "right" => el.right(length),
                    "top" => el.top(length),
                    _ => el.bottom(length),
                };
            }
            "visibility" => {
                el.style().visibility = Some(if value == "hidden" || value == "collapse" {
                    gpui::Visibility::Hidden
                } else {
                    gpui::Visibility::Visible
                })
            }
            "opacity" if effect_layer.is_none() => el = el.opacity(opacity),
            "white-space" if matches!(value, "nowrap" | "pre") => {
                el = el.whitespace_nowrap().flex_nowrap()
            }
            "overflow-x" | "overflow-y" => {
                let overflow = match value {
                    "auto" | "scroll" => gpui::Overflow::Scroll,
                    "hidden" => gpui::Overflow::Hidden,
                    "clip" => gpui::Overflow::Clip,
                    _ => gpui::Overflow::Visible,
                };
                if name == "overflow-x" {
                    el.style().overflow.x = Some(overflow);
                } else {
                    el.style().overflow.y = Some(overflow);
                }
            }
            "cursor" => {
                if let Some(cursor) = render::cursor(value) {
                    el = el.cursor(cursor);
                }
            }
            _ => {}
        }
    }
    if !flex_parent
        && slot.is_none()
        && !matches!(position, "absolute" | "fixed")
        && display.starts_with("inline")
        && let Some(parent) = node.parent
    {
        let align = style
            .get("vertical-align")
            .map_or("baseline", String::as_str);
        el.style().align_self = match align {
            "middle" => Some(gpui::AlignItems::Center),
            "top" | "text-top" => Some(gpui::AlignItems::FlexStart),
            "bottom" => Some(gpui::AlignItems::FlexEnd),
            _ => None,
        };
        let below = |style: &render::Style, font: render::FontUnits| {
            let face = text.resolve_font(&render::font(style));
            let ascent = f32::from(text.ascent(face, px(font.em)));
            let descent = -f32::from(text.descent(face, px(font.em)));
            (font.lh - ascent - descent) / 2. + descent
        };
        let line = styles.lengths[parent].font;
        let replaced = matches!(
            node.tag.as_str(),
            "img"
                | "svg"
                | "canvas"
                | "video"
                | "audio"
                | "iframe"
                | "embed"
                | "object"
                | "progress"
                | "meter"
        ) || check
            || display != "inline"
                && !node.tag.starts_with('#')
                && node.children.iter().all(|&child| {
                    doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim().is_empty()
                });
        let field = editable && node.tag == "input";
        // WARNING: line boxes align text bottoms, so boxes without text are lifted by the parent's descent and half-leading.
        let lift =
            if (replaced || field) && el.style().align_self.is_none() && align != "text-bottom" {
                below(&styles.values[parent], line)
                    - if field {
                        padding[2] + borders[2] + below(style, units.font)
                    } else {
                        0.
                    }
            } else {
                0.
            };
        let raise = match align {
            _ if style.contains_key("top") || style.contains_key("bottom") => 0.,
            "super" => line.em / 3. + lift,
            "sub" => -line.em / 5. + lift,
            "baseline" => lift,
            value => units.resolve(value, units.font.lh).unwrap_or(0.) + lift,
        };
        // WARNING: the margin reserves the shift so the line box grows instead of overlapping its neighbors.
        if raise > 0. {
            el = el.mt(px(margin[0] + raise)).top(px(-raise));
        } else if raise < 0. {
            el = el.mb(px(margin[2] - raise)).top(px(-raise));
        }
    }
    if style
        .get("background-image")
        .is_some_and(|value| value != "none")
    {
        let base = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|state| state.base.clone());
        el = el.children(css_background(
            style,
            units,
            id,
            0,
            radius,
            cx.entity(),
            base.as_ref(),
            &view.host.borrow().assets,
        ));
    }
    if !inner_shadows.is_empty() {
        el = el.child(inset_shadows(inner_shadows, radius, borders));
    }
    if image_child.is_some() || cropped.is_some() {
        let inner = |corner: usize, a: usize, b: usize| {
            (radius[corner] - (borders[a] + padding[a]).max(borders[b] + padding[b])).max(0.)
        };
        let corners = [
            inner(0, 0, 3),
            inner(1, 0, 1),
            inner(2, 2, 1),
            inner(3, 2, 3),
        ];
        if let Some(image) = image_child {
            el = el.child(
                image
                    .rounded_tl(px(corners[0]))
                    .rounded_tr(px(corners[1]))
                    .rounded_br(px(corners[2]))
                    .rounded_bl(px(corners[3])),
            );
        } else if let Some((image, fit)) = cropped {
            let view = cx.entity();
            // WARNING: gpui rounds the fitted image bounds, so overflowing fits paint a cached crop of the visible part.
            el = el.child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let natural = image.size(0);
                        let [width, height] = [u32::from(natural.width), u32::from(natural.height)];
                        let fitted = fit.get_bounds(bounds, natural);
                        let visible = fitted.intersect(&bounds);
                        if width == 0
                            || height == 0
                            || visible.size.width <= px(0.)
                            || visible.size.height <= px(0.)
                        {
                            return;
                        }
                        let scale = [
                            width as f32 / f32::from(fitted.size.width),
                            height as f32 / f32::from(fitted.size.height),
                        ];
                        let x = ((f32::from(visible.left() - fitted.left()) * scale[0]).round()
                            as u32)
                            .min(width - 1);
                        let y = ((f32::from(visible.top() - fitted.top()) * scale[1]).round()
                            as u32)
                            .min(height - 1);
                        let crop = [
                            x,
                            y,
                            ((f32::from(visible.size.width) * scale[0]).round() as u32)
                                .clamp(1, width - x),
                            ((f32::from(visible.size.height) * scale[1]).round() as u32)
                                .clamp(1, height - y),
                        ];
                        let shown = if crop == [0, 0, width, height] {
                            Some(image.clone())
                        } else {
                            let view = view.read(cx);
                            let mut host = view.host.borrow_mut();
                            let state = host.dom.entry(view.id).or_default();
                            let key = {
                                use std::hash::{Hash, Hasher};
                                let mut hasher = std::hash::DefaultHasher::new();
                                (image.id, crop).hash(&mut hasher);
                                hasher.finish()
                            };
                            match state.image_crops.get(&id) {
                                Some((cached, shown)) if *cached == key => Some(shown.clone()),
                                _ => image.as_bytes(0).map(|bytes| {
                                    let pixels = image::RgbaImage::from_fn(
                                        crop[2],
                                        crop[3],
                                        |column, row| {
                                            let at = ((crop[1] + row) * width + crop[0] + column)
                                                as usize
                                                * 4;
                                            image::Rgba([
                                                bytes[at],
                                                bytes[at + 1],
                                                bytes[at + 2],
                                                bytes[at + 3],
                                            ])
                                        },
                                    );
                                    let shown =
                                        Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
                                            pixels,
                                        )]));
                                    if let Some((_, old)) =
                                        state.image_crops.insert(id, (key, shown.clone()))
                                    {
                                        let _ = window.drop_image(old);
                                    }
                                    shown
                                }),
                            }
                        };
                        if let Some(shown) = shown {
                            let corners = gpui::Corners {
                                top_left: px(corners[0]),
                                top_right: px(corners[1]),
                                bottom_right: px(corners[2]),
                                bottom_left: px(corners[3]),
                            }
                            .clamp_radii_for_quad_size(visible.size);
                            let _ = window.paint_image(visible, visible, corners, shown, 0, false);
                        }
                    },
                )
                .size_full(),
            );
        }
    }
    if let Some(lines) = style
        .get("-webkit-line-clamp")
        .or_else(|| style.get("line-clamp"))
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| {
            (style
                .get("text-overflow")
                .is_some_and(|value| value == "ellipsis")
                && style
                    .get("white-space")
                    .is_some_and(|value| matches!(value.as_str(), "nowrap" | "pre"))
                && el
                    .style()
                    .overflow
                    .x
                    .is_some_and(|overflow| overflow != gpui::Overflow::Visible))
            .then_some(1)
        })
    {
        let text = el.text_style();
        text.white_space = Some(gpui::WhiteSpace::Normal);
        text.line_clamp = Some(lines.max(1));
        text.text_overflow = Some(gpui::TextOverflow::Truncate("…".into()));
    }
    if position == "fixed" {
        let viewport = units.viewport;
        let inset = ["left", "right", "top", "bottom"].map(|name| {
            style.get(name).and_then(|v| {
                units.resolve(v, viewport[usize::from(matches!(name, "top" | "bottom"))])
            })
        });
        if style.get("width").is_none_or(|v| v == "auto")
            && let [Some(left), Some(right), ..] = inset
        {
            el = el.w(px((viewport[0] - left - right).max(0.)));
        }
        if style.get("height").is_none_or(|v| v == "auto")
            && let [_, _, Some(top), Some(bottom)] = inset
        {
            el = el.h(px((viewport[1] - top - bottom).max(0.)));
        }
    }
    if let Some(width) = units
        .resolve(
            style.get("outline-width").map_or("3px", String::as_str),
            containing[0],
        )
        .filter(|width| {
            *width > 0.
                && style
                    .get("outline-style")
                    .is_some_and(|kind| !matches!(kind.as_str(), "none" | "hidden"))
        })
    {
        let grow = width
            + style
                .get("outline-offset")
                .and_then(|v| units.resolve(v, containing[0]))
                .unwrap_or(0.);
        let color = style
            .get("outline-color")
            .and_then(|v| render::color(v))
            .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32));
        let mut outline = div()
            .absolute()
            .top(px(-grow - borders[0]))
            .right(px(-grow - borders[1]))
            .bottom(px(-grow - borders[2]))
            .left(px(-grow - borders[3]))
            .border(px(width))
            .border_color(color)
            .rounded_tl(px(radius[0] + grow))
            .rounded_tr(px(radius[1] + grow))
            .rounded_br(px(radius[2] + grow))
            .rounded_bl(px(radius[3] + grow));
        outline.style().border_style = Some(border_style(
            style.get("outline-style").map_or("solid", String::as_str),
        ));
        el = el.child(outline);
    }
    if shaded
        || visible
            .iter()
            .any(|&side| border_colors[side] != border_colors[visible[0]])
    {
        // Each side is painted alone; 3D styles shade it, and groove and ridge split it in two halves.
        let mut bands = vec![([0.; 4], borders, [gpui::transparent_black(); 4])];
        let split = (0..4).any(|side| matches!(border_kinds[side], "groove" | "ridge"));
        if split {
            bands.push(([0.; 4], [0.; 4], [gpui::transparent_black(); 4]));
        }
        for side in 0..4 {
            let color = gpui::Hsla::from(border_colors[side]);
            let raised = side == 0 || side == 3;
            let (outer, inner) = match border_kinds[side] {
                "inset" => (shade_border(color, raised), None),
                "outset" => (shade_border(color, !raised), None),
                "groove" => (
                    shade_border(color, raised),
                    Some(shade_border(color, !raised)),
                ),
                "ridge" => (
                    shade_border(color, !raised),
                    Some(shade_border(color, raised)),
                ),
                _ => (color, None),
            };
            bands[0].2[side] = outer;
            if let Some(inner) = inner {
                let half = borders[side] / 2.;
                bands[0].1[side] = half;
                bands[1].0[side] = half;
                bands[1].1[side] = half;
                bands[1].2[side] = inner;
            }
        }
        if split {
            bands[1].0 = std::array::from_fn(|side| {
                if bands[1].1[side] > 0. {
                    bands[1].0[side]
                } else {
                    borders[side]
                }
            });
        }
        let kinds = border_kinds.map(border_style);
        el = el.child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let outer = Bounds::new(
                        point(
                            bounds.left() - px(borders[3]),
                            bounds.top() - px(borders[0]),
                        ),
                        size(
                            bounds.size.width + px(borders[1] + borders[3]),
                            bounds.size.height + px(borders[0] + borders[2]),
                        ),
                    );
                    for (inset, widths, colors) in &bands {
                        let bounds = Bounds::from_corners(
                            outer.origin + point(px(inset[3]), px(inset[0])),
                            outer.bottom_right() - point(px(inset[1]), px(inset[2])),
                        );
                        let shrink = |corner: usize, a: usize, b: usize| {
                            px((radius[corner] - inset[a].max(inset[b])).max(0.))
                        };
                        let corners = gpui::Corners {
                            top_left: shrink(0, 0, 3),
                            top_right: shrink(1, 0, 1),
                            bottom_right: shrink(2, 2, 1),
                            bottom_left: shrink(3, 2, 3),
                        };
                        for side in 0..4 {
                            if widths[side] <= 0. {
                                continue;
                            }
                            let mut edges = [px(0.); 4];
                            edges[side] = px(widths[side]);
                            window.paint_quad(gpui::quad(
                                bounds,
                                corners,
                                gpui::transparent_black(),
                                gpui::Edges {
                                    top: edges[0],
                                    right: edges[1],
                                    bottom: edges[2],
                                    left: edges[3],
                                },
                                colors[side],
                                kinds[side],
                            ));
                        }
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
    }
    if check && node.attrs.contains_key("checked") {
        let accent = style
            .get("accent-color")
            .and_then(|value| render::color(value))
            .map_or(rgba(0x0a84ffff), |(ink, alpha)| {
                rgba(ink << 8 | alpha as u32)
            });
        let mark = rgba(0xffffffff);
        el = el
            .bg(accent)
            .border_color(accent)
            .items_center()
            .justify_center()
            .child(
                if input.is_some_and(|kind| kind.eq_ignore_ascii_case("radio")) {
                    div().size(px(units.font.em * 0.35)).rounded_full().bg(mark)
                } else {
                    div()
                        .text_color(mark)
                        .text_size(px(units.font.em * 0.75))
                        .line_height(px(units.font.em * 0.75))
                        .child("✓")
                },
            );
    }
    if el.style().overflow.x == Some(gpui::Overflow::Scroll)
        || el.style().overflow.y == Some(gpui::Overflow::Scroll)
    {
        let scroll = view
            .host
            .borrow_mut()
            .dom
            .entry(view.id)
            .or_default()
            .scroll
            .entry(id)
            .or_default()
            .clone();
        el = el.track_scroll(&scroll);
        el.style().restrict_scroll_to_axis = Some(true);
        el = el.child(
            div()
                .absolute()
                .left(px(padding[3] - borders[1] - borders[3]))
                .top(px(padding[0] - borders[0] - borders[2]))
                .size(px(0.)),
        );
    }
    let marker_style = styles
        .text_pseudos
        .get(&id)
        .and_then(|pseudos| pseudos[0].as_ref());
    let content = marker_style
        .and_then(|marker| marker.get("content"))
        .filter(|value| *value != "normal");
    if display == "list-item"
        && content.is_none_or(|value| value != "none")
        && let Some(kind) = style
            .get("list-style-type")
            .filter(|kind| kind.as_str() != "none" || content.is_some())
    {
        let list = node.parent.map(|parent| &doc.nodes[parent]);
        let items: Vec<usize> = list.map_or(vec![id], |list| {
            list.children
                .iter()
                .copied()
                .filter(|&child| {
                    styles.values[child]
                        .get("display")
                        .is_some_and(|value| value == "list-item")
                })
                .collect()
        });
        let position = items.iter().position(|&item| item == id).unwrap_or(0) as i64;
        let start = list
            .and_then(|list| list.attrs.get("start"))
            .and_then(|value| value.parse::<i64>().ok());
        let number = node
            .attrs
            .get("value")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(
                if list.is_some_and(|list| list.attrs.contains_key("reversed")) {
                    start.unwrap_or(items.len() as i64) - position
                } else {
                    start.unwrap_or(1) + position
                },
            );
        let marker = match kind.as_str() {
            "disc" => "•".to_owned(),
            "circle" => "◦".to_owned(),
            "square" => "▪".to_owned(),
            "disclosure-closed" => "▸".to_owned(),
            "disclosure-open" => "▾".to_owned(),
            "decimal" => format!("{number}."),
            "decimal-leading-zero" => format!("{number:02}."),
            "lower-alpha" | "lower-latin" | "upper-alpha" | "upper-latin" if number > 0 => {
                let mut letters = Vec::new();
                let mut rest = number;
                while rest > 0 {
                    rest -= 1;
                    letters.push((b'a' + (rest % 26) as u8) as char);
                    rest /= 26;
                }
                let letters: String = letters.into_iter().rev().collect();
                format!(
                    "{}.",
                    if kind.starts_with("upper") {
                        letters.to_ascii_uppercase()
                    } else {
                        letters
                    }
                )
            }
            "lower-roman" | "upper-roman" if (1..4000).contains(&number) => {
                let mut rest = number;
                let mut roman = String::new();
                for (value, digits) in [
                    (1000, "m"),
                    (900, "cm"),
                    (500, "d"),
                    (400, "cd"),
                    (100, "c"),
                    (90, "xc"),
                    (50, "l"),
                    (40, "xl"),
                    (10, "x"),
                    (9, "ix"),
                    (5, "v"),
                    (4, "iv"),
                    (1, "i"),
                ] {
                    while rest >= value {
                        roman.push_str(digits);
                        rest -= value;
                    }
                }
                format!(
                    "{}.",
                    if kind == "upper-roman" {
                        roman.to_ascii_uppercase()
                    } else {
                        roman
                    }
                )
            }
            other if other.starts_with(['"', '\'']) => other.trim_matches(['"', '\'']).to_owned(),
            _ => format!("{number}."),
        };
        let marker = content.map_or(marker, |content| {
            let mut text = String::new();
            for part in render::components(content, ' ') {
                if part.starts_with("counter(") || part.starts_with("counters(") {
                    text.push_str(&number.to_string());
                    continue;
                }
                let mut input = cssparser::Parser::new(part);
                if let Ok(string) = input.expect_string() {
                    text.push_str(string);
                }
            }
            text
        });
        let mut marker_box = if style
            .get("list-style-position")
            .is_some_and(|value| value == "inside")
        {
            div()
                .flex_shrink_0()
                .mr(px(units.font.em * 0.3))
                .child(marker)
        } else {
            div()
                .absolute()
                .top(px(padding[0]))
                .right(relative(1.))
                .pr(px(units.font.em * 0.5))
                .whitespace_nowrap()
                .child(marker)
        };
        if let Some(marker) = marker_style {
            let face = render::font(marker);
            marker_box = marker_box.font_family(face.family).font_weight(face.weight);
            marker_box.text_style().font_style = Some(face.style);
            if let Some((ink, alpha)) = marker.get("color").and_then(|value| render::color(value)) {
                marker_box = marker_box.text_color(rgba(ink << 8 | alpha as u32));
            }
            if let Some(size) = marker
                .get("font-size")
                .and_then(|value| units.resolve(value, units.font.em))
            {
                marker_box = marker_box.text_size(px(size));
            }
        }
        el = el.child(marker_box);
    }
    let mut after = None;
    if let Some(pseudos) = styles.pseudos.get(&id) {
        for (index, pseudo) in pseudos.iter().enumerate() {
            let Some(content) = pseudo
                .get("content")
                .filter(|v| !matches!(v.as_str(), "none" | "normal"))
            else {
                continue;
            };
            if pseudo.get("display").is_some_and(|value| value == "none") {
                continue;
            }
            let mut text = String::new();
            for part in render::components(content, ' ') {
                if let Some(name) = part.strip_prefix("attr(").and_then(|v| v.strip_suffix(')')) {
                    text.push_str(node.attrs.get(name.trim()).map_or("", String::as_str));
                } else if part == "open-quote" {
                    text.push('\u{201c}');
                } else if part == "close-quote" {
                    text.push('\u{201d}');
                } else if let Some(string) = part
                    .strip_prefix(['"', '\''])
                    .and_then(|v| v.strip_suffix(['"', '\'']))
                {
                    let mut chars = string.chars().peekable();
                    while let Some(c) = chars.next() {
                        if c != '\\' {
                            text.push(c);
                            continue;
                        }
                        let mut code = String::new();
                        while code.len() < 6
                            && let Some(digit) = chars.next_if(char::is_ascii_hexdigit)
                        {
                            code.push(digit);
                        }
                        if code.is_empty() {
                            text.extend(chars.next());
                            continue;
                        }
                        chars.next_if_eq(&' ');
                        text.push(
                            u32::from_str_radix(&code, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .unwrap_or('\u{fffd}'),
                        );
                    }
                }
            }
            let mut pseudo_units = units;
            if let Some(size) = pseudo
                .get("font-size")
                .and_then(|v| units.resolve(v, units.font.em))
            {
                pseudo_units.font = render::FontUnits::new(size);
            }
            let face = render::font(pseudo);
            let mut decoration = div()
                .flex()
                .flex_row()
                .flex_shrink_0()
                .text_size(px(pseudo_units.font.em))
                .font_family(face.family)
                .font_weight(face.weight);
            if let Some((ink, alpha)) = pseudo.get("color").and_then(|v| render::color(v)) {
                decoration = decoration.text_color(rgba(ink << 8 | alpha as u32));
            }
            let absolute = pseudo
                .get("position")
                .is_some_and(|v| matches!(v.as_str(), "absolute" | "fixed"));
            if absolute {
                decoration = decoration.absolute();
            } else if pseudo
                .get("display")
                .is_some_and(|value| !value.starts_with("inline"))
                && (all_inline || display == "inline")
            {
                decoration = decoration.w_full();
            }
            let pseudo_padding = pseudo_units
                .edges(pseudo, "padding", containing[0])
                .map(|v| v.max(0.));
            let pseudo_margin = pseudo_units.edges(pseudo, "margin", containing[0]);
            let pseudo_borders = sides.map(|(width, kind, _)| {
                if pseudo
                    .get(kind)
                    .is_none_or(|kind| matches!(kind.as_str(), "none" | "hidden"))
                {
                    return 0.;
                }
                pseudo
                    .get(width)
                    .map_or(Some(3.), |width| pseudo_units.resolve(width, containing[0]))
                    .unwrap_or(0.)
                    .max(0.)
            });
            decoration = decoration
                .pt(px(pseudo_padding[0]))
                .pr(px(pseudo_padding[1]))
                .pb(px(pseudo_padding[2]))
                .pl(px(pseudo_padding[3]))
                .mt(px(pseudo_margin[0]))
                .mr(px(pseudo_margin[1]))
                .mb(px(pseudo_margin[2]))
                .ml(px(pseudo_margin[3]))
                .border_t(px(pseudo_borders[0]))
                .border_r(px(pseudo_borders[1]))
                .border_b(px(pseudo_borders[2]))
                .border_l(px(pseudo_borders[3]));
            if let Some(side) = (0..4).find(|&side| pseudo_borders[side] > 0.) {
                decoration = decoration.border_color(
                    pseudo
                        .get(sides[side].2)
                        .and_then(|v| render::color(v))
                        .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32)),
                );
            }
            let mut pseudo_radius = [0.; 4];
            let mut pseudo_shadows = Vec::new();
            for (name, value) in pseudo {
                let vertical =
                    name.ends_with("height") || matches!(name.as_str(), "top" | "bottom");
                let base = containing[usize::from(vertical)];
                let length = || -> Option<gpui::Length> {
                    match value
                        .strip_suffix('%')
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                    {
                        Some(percent) => Some(relative(percent / 100.).into()),
                        None => pseudo_units
                            .resolve(value, base)
                            .map(|value| px(value).into()),
                    }
                };
                match name.as_str() {
                    "width" | "height" | "min-width" | "max-width" | "min-height"
                    | "max-height" | "left" | "top" | "right" | "bottom" => {
                        let Some(length) = length() else { continue };
                        decoration = match name.as_str() {
                            "width" => decoration.w(length),
                            "height" => decoration.h(length),
                            "min-width" => decoration.min_w(length),
                            "max-width" => decoration.max_w(length),
                            "min-height" => decoration.min_h(length),
                            "max-height" => decoration.max_h(length),
                            "left" if absolute => decoration.left(length),
                            "top" if absolute => decoration.top(length),
                            "right" if absolute => decoration.right(length),
                            "bottom" if absolute => decoration.bottom(length),
                            _ => decoration,
                        };
                    }
                    "border-top-left-radius"
                    | "border-top-right-radius"
                    | "border-bottom-right-radius"
                    | "border-bottom-left-radius" => {
                        let Some(length) = pseudo_units
                            .resolve(value, containing[0])
                            .map(|v| px(v.max(0.)))
                        else {
                            continue;
                        };
                        let corner = match name.as_str() {
                            "border-top-left-radius" => 0,
                            "border-top-right-radius" => 1,
                            "border-bottom-right-radius" => 2,
                            _ => 3,
                        };
                        pseudo_radius[corner] = f32::from(length);
                        decoration = match name.as_str() {
                            "border-top-left-radius" => decoration.rounded_tl(length),
                            "border-top-right-radius" => decoration.rounded_tr(length),
                            "border-bottom-right-radius" => decoration.rounded_br(length),
                            _ => decoration.rounded_bl(length),
                        };
                    }
                    "background-color" => {
                        if let Some((color, alpha)) = render::color(value) {
                            decoration = decoration.bg(rgba(color << 8 | alpha as u32));
                        }
                    }
                    "opacity" => {
                        if let Ok(opacity) = value.parse::<f32>() {
                            decoration = decoration.opacity(opacity.clamp(0., 1.));
                        }
                    }
                    "box-shadow" => {
                        decoration.style().box_shadow = Some(pseudo_units.shadows(value, false));
                        pseudo_shadows = pseudo_units.shadows(value, true);
                    }
                    _ => {}
                }
            }
            if pseudo
                .get("background-image")
                .is_some_and(|value| value != "none")
            {
                let base = view
                    .host
                    .borrow()
                    .dom
                    .get(&view.id)
                    .and_then(|state| state.base.clone());
                decoration = decoration.children(css_background(
                    pseudo,
                    units,
                    id,
                    index + 1,
                    [0.; 4],
                    cx.entity(),
                    base.as_ref(),
                    &view.host.borrow().assets,
                ));
            }
            if !pseudo_shadows.is_empty() {
                decoration =
                    decoration.child(inset_shadows(pseudo_shadows, pseudo_radius, pseudo_borders));
            }
            if !text.is_empty() {
                decoration = decoration.child(SharedString::from(text));
            }
            let order = pseudo
                .get("z-index")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            let decoration = if order != 0
                && pseudo
                    .get("position")
                    .is_some_and(|value| value != "static")
            {
                Stacked {
                    inner: Some(decoration.into_any_element()),
                    layers: build.layers.clone(),
                    order,
                    path: build.path.clone(),
                }
                .into_any_element()
            } else {
                decoration.into_any_element()
            };
            if index == 0 {
                el = el.child(decoration);
            } else {
                after = Some(decoration);
            }
        }
    }
    let preserve = style
        .get("white-space")
        .is_some_and(|v| matches!(v.as_str(), "pre" | "pre-wrap" | "break-spaces"))
        || node.tag == "pre";
    if node.tag == "#text"
        && style
            .get("white-space")
            .is_none_or(|value| !matches!(value.as_str(), "pre" | "nowrap"))
    {
        el = el.min_w(px(0.)).max_w(relative(1.)).flex_shrink(1.);
    }
    if preserve && node.text == "\n" {
        el = el.min_h(px(units.font.lh));
    }
    let inline = if editable {
        None
    } else {
        inline_text
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| render::inline_text(doc, styles, id).map(Rc::new))
            .clone()
    };
    let hit_test = inline.is_some();
    if (hit_test || node.tag == "#text")
        && styles.user_select[id].0 != render::UserSelect::None
        && style.get("cursor").is_none_or(|value| value == "auto")
    {
        el = el.cursor(gpui::CursorStyle::IBeam);
    }
    if let Some(data) = inline {
        if style
            .get("container-type")
            .is_some_and(|v| matches!(v.as_str(), "inline-size" | "size"))
        {
            let size = [0, 1].map(|axis| {
                (style
                    .get(["width", "height"][axis])
                    .and_then(|v| units.resolve(v, containing[axis]))
                    .map_or(
                        containing[axis] - [margin[1] + margin[3], 0.][axis],
                        |value| value + extra[axis],
                    )
                    - padding[1 - axis]
                    - padding[3 - axis]
                    - borders[1 - axis]
                    - borders[3 - axis])
                    .max(0.)
            });
            view.host
                .borrow_mut()
                .dom
                .entry(view.id)
                .or_default()
                .containers
                .insert(id, size);
        }
        let inner = gpui::StyledText::new(data.text.to_string()).with_runs(data.runs.clone());
        el = el.child(
            div()
                .min_w(px(0.))
                .max_w(relative(1.))
                .flex_shrink(1.)
                .child(InlineText {
                    inner,
                    data,
                    view: cx.entity(),
                    runs: None,
                    measured: false,
                    justify: style
                        .get("text-align")
                        .is_some_and(|value| value == "justify")
                        .then(|| {
                            style
                                .get("text-align-last")
                                .is_some_and(|value| value == "justify")
                        }),
                }),
        );
    } else if !node.children.is_empty()
        && node.tag != "canvas"
        && node.tag != "iframe"
        && node.tag != "svg"
        && !editable
    {
        let mut content = containing;
        for (axis, (name, min, max)) in [
            ("width", "min-width", "max-width"),
            ("height", "min-height", "max-height"),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(value) = style
                .get(name)
                .and_then(|v| units.resolve(v, containing[axis]))
                .map(|value| value + extra[axis])
                .or(intrinsic_width.filter(|_| axis == 0))
            {
                content[axis] = value;
            } else if axis == 0 {
                content[axis] -= margin[1] + margin[3];
            }
            if let Some(value) = style
                .get(max)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].min(value + extra[axis]);
            }
            if let Some(value) = style
                .get(min)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].max(value + extra[axis]);
            }
            content[axis] = (content[axis]
                - padding[1 - axis]
                - padding[3 - axis]
                - borders[1 - axis]
                - borders[3 - axis])
                .max(0.);
        }
        if let Some((bounds, _)) = &slot {
            content[0] = (bounds[2]
                - padding[1]
                - padding[3]
                - margin[1]
                - margin[3]
                - borders[1]
                - borders[3])
                .max(0.);
            content[1] = (bounds[3]
                - padding[0]
                - padding[2]
                - margin[0]
                - margin[2]
                - borders[0]
                - borders[2])
                .max(0.);
        }
        let container = if style
            .get("container-type")
            .is_some_and(|v| matches!(v.as_str(), "inline-size" | "size"))
        {
            view.host
                .borrow_mut()
                .dom
                .entry(view.id)
                .or_default()
                .containers
                .insert(id, content);
            content
        } else {
            container
        };
        let grid = if let Some(display) = style
            .get("display")
            .filter(|v| matches!(v.as_str(), "grid" | "inline-grid"))
        {
            if display == "inline-grid"
                && slot.is_none()
                && style.get("width").is_none_or(|v| v == "auto")
            {
                let extent =
                    padding[1] + padding[3] + borders[1] + borders[3] + margin[1] + margin[3];
                let [min, max, ..] = crate::grid::intrinsic(
                    doc,
                    id,
                    styles,
                    None,
                    None,
                    text,
                    &mut measured.borrow_mut(),
                );
                content[0] = (max.min(content[0] + extent).max(min) - extent).max(0.);
            }
            let height = (slot.is_some() || style.get("height").is_some_and(|v| v != "auto"))
                .then_some(content[1]);
            let cached = view
                .host
                .borrow()
                .dom
                .get(&view.id)
                .and_then(|state| state.grids.get(&id))
                .filter(|(width, cached, inherited, _)| {
                    *width == content[0]
                        && *cached == height
                        && inherited.as_ref() == slot.as_ref().map(|(_, grid)| grid)
                })
                .map(|(_, _, _, grid)| grid.clone());
            Some(cached.unwrap_or_else(|| {
                let grid = crate::grid::layout(
                    doc,
                    id,
                    styles,
                    Some(content[0]),
                    height,
                    slot.as_ref().map(|(_, grid)| grid),
                    text,
                    &mut measured.borrow_mut(),
                );
                view.host
                    .borrow_mut()
                    .dom
                    .entry(view.id)
                    .or_default()
                    .grids
                    .insert(
                        id,
                        (
                            content[0],
                            height,
                            slot.as_ref().map(|(_, grid)| grid.clone()),
                            grid.clone(),
                        ),
                    );
                grid
            }))
        } else {
            None
        };
        if let Some(grid) = &grid {
            el = el.flex().flex_col().w(px(content[0]
                + padding[1]
                + padding[3]
                + borders[1]
                + borders[3]));
            if style.get("height").is_none_or(|v| v == "auto") {
                el = el.h(px(grid.rows.iter().sum::<f32>()
                    + grid.rows.len().saturating_sub(1) as f32 * grid.gap[1]
                    + padding[0]
                    + padding[2]
                    + borders[0]
                    + borders[2]));
            }
        }
        let mut children = if grid.is_some() {
            node.children.clone()
        } else {
            flat.clone()
        };
        if grid.is_some() {
            children.sort_by_key(|child| {
                styles.values[*child]
                    .get("z-index")
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            });
        } else if flexbox {
            children.sort_by_key(|child| {
                styles.values[*child]
                    .get("order")
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            });
        }
        let inline_element = |child: Option<&usize>| {
            child.is_some_and(|&child| {
                !doc.nodes[child].tag.starts_with('#')
                    && doc.nodes[child].tag != "br"
                    && inline_level(child)
            })
        };
        let multicol = columns(style, &units, content[0]);
        let mut line: Option<gpui::Div> = None;
        let mut floating = Vec::new();
        // Floats and the content after them, until a `clear`, share a row: left floats, the
        // content, then right floats. Text does not flow under a float that ends early.
        let mut group: Option<(Vec<AnyElement>, Vec<AnyElement>, Vec<AnyElement>)> = None;
        // With multiple columns, block content is collected and split into columns at the end.
        let columns = multicol.filter(|_| flow && grid.is_none() && !all_inline);
        let mut collected: Vec<(Option<usize>, AnyElement)> = Vec::new();
        macro_rules! place {
            ($element:expr, $child:expr) => {{
                let element = $element.into_any_element();
                match group.as_mut() {
                    Some((_, _, content)) => content.push(element),
                    None if columns.is_some() => collected.push(($child, element)),
                    None => el = el.child(element),
                }
            }};
        }
        macro_rules! close_group {
            () => {{
                if let Some(open) = line.take() {
                    place!(open, None);
                }
                if let Some((lefts, rights, content)) = group.take() {
                    let row = div()
                        .flex()
                        .flex_row()
                        .items_start()
                        .w_full()
                        .children(lefts)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.))
                                .children(content),
                        )
                        .children(rights.into_iter().rev());
                    if columns.is_some() {
                        collected.push((None, row.into_any_element()));
                    } else {
                        el = el.child(row);
                    }
                }
            }};
        }
        for (index, &child) in children.iter().enumerate() {
            if preserve
                || !(doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim().is_empty())
                || flow
                    && grid.is_none()
                    && inline_element(index.checked_sub(1).and_then(|index| children.get(index)))
                    && inline_element(children.get(index + 1))
            {
                let placement = grid.as_ref().and_then(|grid| {
                    let index = grid
                        .cells
                        .binary_search_by_key(&child, |(id, ..)| *id)
                        .ok()?;
                    let (_, [column, row, columns, rows], mut bounds) = *grid.cells.get(index)?;
                    bounds[0] += padding[3];
                    bounds[1] += padding[0];
                    Some((
                        bounds,
                        crate::grid::Grid {
                            columns: grid.columns.get(column..column + columns)?.to_vec(),
                            rows: grid.rows.get(row..row + rows)?.to_vec(),
                            gap: grid.gap,
                            cells: Vec::new(),
                        },
                    ))
                });
                let child_element = element(build, child, content, container, placement, cx);
                let out_of_flow = styles.values[child]
                    .get("position")
                    .is_some_and(|value| matches!(value.as_str(), "absolute" | "fixed"));
                let child_style = &styles.values[child];
                if grid.is_none() && !out_of_flow {
                    let clear = child_style.get("clear").map_or("none", String::as_str);
                    if group.is_some() && clear != "none" {
                        close_group!();
                    }
                    if floated(child) {
                        if let Some(open) = line.take() {
                            place!(open, None);
                        }
                        let open = group.get_or_insert_with(Default::default);
                        // A float without a width shrinks to fit, but no wider than the row.
                        let float = if child_style.get("width").is_none_or(|value| value == "auto")
                        {
                            div()
                                .flex()
                                .flex_col()
                                .flex_shrink(1.)
                                .min_w(px(0.))
                                .child(child_element)
                                .into_any_element()
                        } else {
                            child_element
                        };
                        if float_side(child_style) == Some(true) {
                            open.1.push(float);
                        } else {
                            open.0.push(float);
                        }
                        continue;
                    }
                }
                if flow
                    && grid.is_none()
                    && child_style
                        .get("position")
                        .is_some_and(|value| value == "absolute")
                    && child_style
                        .get("display")
                        .is_none_or(|value| value != "none")
                    && [["left", "right"], ["top", "bottom"]].iter().any(|axis| {
                        axis.iter()
                            .all(|name| child_style.get(*name).is_none_or(|value| value == "auto"))
                    })
                {
                    // WARNING: the placeholder must come before the box, which reads its bounds in prepaint.
                    let block = child_style
                        .get("display")
                        .is_some_and(|value| !value.starts_with("inline"));
                    let entity = cx.entity();
                    let mut placeholder = gpui::canvas(
                        move |bounds, _, cx| {
                            let view = entity.read(cx);
                            view.host
                                .borrow_mut()
                                .dom
                                .entry(view.id)
                                .or_default()
                                .anchors
                                .insert(child, bounds);
                        },
                        |_, _, _, _| {},
                    )
                    .w(px(0.))
                    .flex_shrink_0()
                    .h(px(
                        if block
                            && (line.is_some()
                                || all_inline
                                    && flat.iter().any(|&other| other != child && in_flow(other)))
                        {
                            units.font.lh
                        } else {
                            0.
                        },
                    ));
                    placeholder.style().align_self = Some(gpui::AlignSelf::FlexStart);
                    match line.take() {
                        Some(open) => line = Some(open.child(placeholder)),
                        None if all_inline || block => place!(placeholder, None),
                        None => {
                            let mut next = div().flex().flex_row().flex_wrap();
                            next.style().justify_content = line_justify;
                            line = Some(next.child(placeholder));
                        }
                    }
                }
                if flow && !all_inline && grid.is_none() && inline_level(child) && !out_of_flow {
                    let mut next = line
                        .take()
                        .unwrap_or_else(|| div().flex().flex_row().flex_wrap());
                    next.style().justify_content = line_justify;
                    next.style().align_items = line_align;
                    line = Some(next.child(child_element));
                } else if out_of_flow {
                    floating.push(child_element);
                } else {
                    if let Some(line) = line.take() {
                        place!(line, None);
                    }
                    place!(child_element, Some(child));
                }
            }
        }
        close_group!();
        if let Some((count, gap, rule)) = columns {
            // Balance the columns by the measured height of each block.
            let width = ((content[0] - gap * (count - 1) as f32) / count as f32).max(0.);
            let heights: Vec<f32> = collected
                .iter()
                .map(|(child, _)| {
                    child.map_or(units.font.lh, |child| {
                        crate::grid::intrinsic(
                            doc,
                            child,
                            styles,
                            Some(width),
                            None,
                            text,
                            &mut measured.borrow_mut(),
                        )[3]
                    })
                })
                .collect();
            let target = heights.iter().sum::<f32>() / count as f32;
            let mut columns: Vec<Vec<AnyElement>> = (0..count).map(|_| Vec::new()).collect();
            let (mut column, mut filled) = (0, 0.);
            for ((_, item), height) in collected.into_iter().zip(heights) {
                if filled > 0. && filled + height / 2. > target && column + 1 < count {
                    column += 1;
                    filled = 0.;
                }
                filled += height;
                columns[column].push(item);
            }
            let mut row = div()
                .flex()
                .flex_row()
                .items_start()
                .w_full()
                .gap_x(px(gap));
            for items in columns {
                row = row.child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.))
                        .children(items),
                );
            }
            if let Some((rule_width, color, kind)) = rule {
                row = row.child(column_rules(count, gap, rule_width, color, kind));
            }
            el = el.child(row);
        }
        el = el.children(floating);
    }
    if let Some(after) = after {
        el = el.child(after);
    }
    if slot.is_some() && flow && matches!(node.tag.as_str(), "td" | "th") {
        let align = match style.get("vertical-align").map(String::as_str) {
            Some("middle") => Some(gpui::AlignContent::Center),
            Some("bottom") => Some(gpui::AlignContent::FlexEnd),
            _ => None,
        };
        if all_inline {
            el.style().align_content = align;
        } else {
            el.style().justify_content = align;
        }
    }
    if let Some((bounds, _)) = &slot {
        el = el
            .absolute()
            .left(px(bounds[0]))
            .top(px(bounds[1]))
            .mt(px(margin[0]))
            .mr(px(margin[1]))
            .mb(px(margin[2]))
            .ml(px(margin[3]))
            .w(px((bounds[2] - margin[1] - margin[3]).max(0.)))
            .h(px((bounds[3] - margin[0] - margin[2]).max(0.)));
    }
    if style
        .get("pointer-events")
        .is_none_or(|value| value != "none")
    {
        let path = ancestors.clone();
        el = el.on_mouse_down(MouseButton::Left, cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
            window.focus(&view.focus, cx);
            cx.stop_propagation();
            Desktop::dispatch(view.runtime.clone(), view.id, "pointerdown", if hit_test { &[] } else { &path }, serde_json::json!({"pointerId":1,"pointerType":"mouse","isPrimary":true,"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y),"button":0,"buttons":1,"detail":event.click_count,"ctrlKey":event.modifiers.control,"metaKey":event.modifiers.platform,"altKey":event.modifiers.alt,"shiftKey":event.modifiers.shift}), cx);
        }));
    }
    let mut parent = node.parent;
    while let Some(ancestor) = parent
        && styles.values[ancestor]
            .get("display")
            .is_some_and(|value| value == "contents")
    {
        parent = doc.nodes[ancestor].parent;
    }
    let parent_display = parent
        .and_then(|parent| styles.values[parent].get("display"))
        .map_or("block", String::as_str);
    let axes = [["left", "right"], ["top", "bottom"]].map(|axis| {
        axis.iter()
            .all(|name| style.get(*name).is_none_or(|v| v == "auto"))
    });
    let mut anchor = None;
    if position == "absolute" && slot.is_none() && axes.contains(&true) {
        if matches!(parent_display, "grid" | "inline-grid") {
            if axes[0] {
                el = el.left(px(0.));
            }
            if axes[1] {
                el = el.top(px(0.));
            }
        } else if !matches!(parent_display, "flex" | "inline-flex") {
            let block = !display.starts_with("inline");
            if block
                && axes[0]
                && let Some(parent) = parent
            {
                el = el.left(px(styles.lengths[parent].edges(
                    &styles.values[parent],
                    "padding",
                    containing[0],
                )[3]
                .max(0.)));
            }
            anchor = Some((axes, block, [margin[3], margin[0]]));
        }
    }
    if stacking {
        el = el.child(paint_layers(build.layers.clone(), false));
    }
    let scroll_ancestors = ancestors.clone();
    let el = if style
        .get("pointer-events")
        .is_some_and(|value| value == "none")
    {
        el
    } else {
        el.on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
            cx.stop_propagation();
            let runtime = view.runtime.clone();
            let result = {
                let app = runtime.borrow();
                app.views
                    .get(&view.id)
                    .and_then(|v| v.renderer.as_ref())
                    .map(|engine| {
                        engine.eval(
                            &format!(
                                "__appsDispatch('click',{},{})",
                                serde_json::to_string(if hit_test { &[][..] } else { &ancestors[..] }).unwrap(),
                                serde_json::json!({"clientX": f32::from(event.position().x), "clientY": f32::from(event.position().y), "detail":event.click_count(), "ctrlKey":event.modifiers().control,"metaKey":event.modifiers().platform,"altKey":event.modifiers().alt,"shiftKey":event.modifiers().shift})
                            ),
                            "apps:click",
                        )
                    })
            };
            if let Some(Err(error)) = result {
                eprintln!("apps: {error}");
            }
            cx.notify();
            cx.defer(move |cx| Desktop::drive(runtime, cx));
        }))
    };
    let sticky = (style.get("position").is_some_and(|v| v == "sticky"))
        .then(|| {
            let host = view.host.borrow();
            scroll_ancestors.into_iter().skip(1).find_map(|parent| {
                let scroll = host.dom.get(&view.id)?.scroll.get(&parent)?.clone();
                let style = &styles.values[parent];
                let border = [
                    "border-top-width",
                    "border-right-width",
                    "border-bottom-width",
                    "border-left-width",
                ]
                .map(|name| {
                    style
                        .get(name)
                        .or_else(|| style.get("border-width"))
                        .and_then(|v| styles.lengths[parent].resolve(v, containing[0]))
                        .unwrap_or(0.)
                        .max(0.)
                });
                Some((scroll, border))
            })
        })
        .flatten();
    // Vertical writing modes lay the box out horizontally at its inline size, then turn it.
    let vertical = style
        .get("writing-mode")
        .map(String::as_str)
        .filter(|mode| mode.starts_with("vertical") || mode.starts_with("sideways"))
        .filter(|_| id != 0 && !node.tag.starts_with('#') && slot.is_none());
    let el = match vertical {
        Some(mode) => {
            let [_, widest, ..] = crate::grid::intrinsic(
                doc,
                id,
                styles,
                None,
                None,
                text,
                &mut measured.borrow_mut(),
            );
            let inline = style
                .get("height")
                .and_then(|value| units.resolve(value, containing[1]))
                .map(|height| height + extra[1])
                .unwrap_or_else(|| widest.min(units.viewport[1]))
                .max(1.);
            let block = crate::grid::intrinsic(
                doc,
                id,
                styles,
                Some(inline),
                None,
                text,
                &mut measured.borrow_mut(),
            )[3];
            div()
                .id(("vertical", id))
                .relative()
                .flex_shrink_0()
                .w(px(block))
                .h(px(inline))
                .child(Turned {
                    child: el
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(px(inline))
                        .into_any_element(),
                    clockwise: mode != "sideways-lr",
                    block,
                    inline,
                })
        }
        None => el,
    };
    let element = DomBox {
        inner: Some(el),
        anchor,
        style: style.clone(),
        effects: effect_layer,
        perspective: parent_perspective.clone().zip(parent),
        units,
        id,
        view: cx.entity(),
        sticky,
        fixed: position == "fixed",
        inset: ["top", "right", "bottom", "left"].map(|name| {
            let axis = usize::from(matches!(name, "top" | "bottom"));
            style.get(name).and_then(|v| {
                units.resolve(
                    v,
                    if position == "fixed" {
                        units.viewport[axis]
                    } else {
                        containing[axis]
                    },
                )
            })
        }),
    }
    .into_any_element();
    if positioned && let Some(order) = order.filter(|order| *order != 0) {
        Stacked {
            inner: Some(element),
            layers: layers.clone(),
            order,
            path: path.clone(),
        }
        .into_any_element()
    } else {
        element
    }
}

impl WebView {
    fn evaluate(&self, script: &str) -> Result<String> {
        let renderer = self
            .runtime
            .borrow()
            .views
            .get(&self.id)
            .and_then(|view| view.renderer.clone())
            .context("Page is not loaded")?;
        renderer.eval(script, "apps:input")
    }

    fn editing(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.evaluate("JSON.stringify(__appsEditing())").ok()?)
            .ok()
            .filter(|value: &serde_json::Value| !value.is_null())
    }

    fn input(
        &mut self,
        text: &str,
        range: Option<Range<usize>>,
        marked: bool,
        selected: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        let start = range.as_ref().map(|r| r.start);
        let end = range.as_ref().map(|r| r.end);
        let selected = selected.map(|r| [r.start, r.end]);
        if let Err(error) = self.evaluate(&format!(
            "__appsInput({},{},{},{},{})",
            serde_json::json!(text),
            serde_json::json!(start),
            serde_json::json!(end),
            marked,
            serde_json::json!(selected)
        )) {
            eprintln!("apps: {error}");
        }
        let runtime = self.runtime.clone();
        cx.defer(move |cx| Desktop::drive(runtime, cx));
        cx.notify();
    }
}

impl EntityInputHandler for WebView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let state = self.editing()?;
        let text: Vec<_> = state["text"].as_str()?.encode_utf16().collect();
        let end = range.end.min(text.len());
        let range = range.start.min(end)..end;
        *actual = Some(range.clone());
        Some(String::from_utf16_lossy(&text[range]))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let state = self.editing()?;
        let start = state["start"].as_u64()? as usize;
        let end = state["end"].as_u64()? as usize;
        Some(UTF16Selection {
            range: start.min(end)..start.max(end),
            reversed: start > end,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let state = self.editing()?;
        Some(state["marked"][0].as_u64()? as usize..state["marked"][1].as_u64()? as usize)
            .filter(|range| range.start <= range.end)
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = self.editing()
            && let (Some(start), Some(end), Some(text)) = (
                state["marked"][0].as_u64(),
                state["marked"][1].as_u64(),
                state["text"].as_str(),
            )
        {
            let text: Vec<_> = text.encode_utf16().collect();
            if start <= end && (end as usize) <= text.len() {
                self.input(
                    &String::from_utf16_lossy(&text[start as usize..end as usize]),
                    Some(start as usize..end as usize),
                    false,
                    None,
                    cx,
                );
            }
        }
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input(text, range, false, None, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input(text, range, true, selected, cx);
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let state = self.editing()?;
        let id = state["id"].as_u64()? as usize;
        let host = self.host.borrow();
        let bounds = host.dom.get(&self.id)?.bounds.get(&id)?;
        Some(Bounds::new(
            point(px(bounds[0]), px(bounds[1])),
            size(px(bounds[2]), px(bounds[3])),
        ))
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let state = self.editing()?;
        let id = state["id"].as_u64()? as usize;
        let host = self.host.borrow();
        let dom = host.dom.get(&self.id)?;
        let bounds = dom.bounds.get(&id)?;
        let style = dom.computed.as_ref()?.values.get(id)?;
        let face = render::font(style);
        let text = state["text"].as_str()?;
        let font_size = render::length(
            style.get("font-size").map(String::as_str).unwrap_or("16px"),
            16.,
            16.,
            bounds[2],
            bounds[3],
        );
        let line_height = render::length(
            style
                .get("line-height")
                .map(String::as_str)
                .unwrap_or("20px"),
            font_size,
            font_size,
            bounds[2],
            bounds[3],
        )
        .max(1.);
        let row = ((f32::from(point.y) - bounds[1]).max(0.) / line_height) as usize;
        let mut offset = 0;
        let mut selected = "";
        for (index, line) in text.split('\n').enumerate() {
            selected = line;
            if index == row {
                break;
            }
            offset += line.encode_utf16().count() + 1;
        }
        let run = gpui::TextRun {
            len: selected.len(),
            font: face,
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
            letter_spacing: gpui::px(0.),
            word_spacing: gpui::px(0.),
            font_size: None,
        };
        let line = window.text_system().shape_line(
            selected.to_owned().into(),
            px(font_size),
            &[run],
            None,
        );
        let index = line.closest_index_for_x(point.x - px(bounds[0]));
        Some((offset + selected[..index].encode_utf16().count()).min(text.encode_utf16().count()))
    }
}

/// `Some(true)` for a box floated right, `Some(false)` for left.
fn float_side(style: &render::Style) -> Option<bool> {
    match style.get("float").map(String::as_str) {
        Some("left" | "inline-start") => Some(false),
        Some("right" | "inline-end") => Some(true),
        _ => None,
    }
}

/// The width, color and style of a column rule.
type ColumnRule = (f32, gpui::Hsla, gpui::BorderStyle);

/// The column count, gap and rule of a multi-column box with the given content width.
fn columns(
    style: &render::Style,
    units: &render::Lengths,
    width: f32,
) -> Option<(usize, f32, Option<ColumnRule>)> {
    let mut count = style
        .get("column-count")
        .and_then(|value| value.parse::<usize>().ok());
    let mut column_width = style
        .get("column-width")
        .and_then(|value| units.resolve(value, width));
    for part in render::components(style.get("columns").map_or("", String::as_str), ' ') {
        if let Ok(value) = part.parse::<usize>() {
            count = count.or(Some(value));
        } else if let Some(value) = units.resolve(part, width) {
            column_width = column_width.or(Some(value));
        }
    }
    let gap = style
        .get("column-gap")
        .filter(|value| *value != "normal")
        .and_then(|value| units.resolve(value, width))
        .unwrap_or(units.font.em)
        .max(0.);
    let fit = column_width
        .filter(|value| *value > 0.)
        .map(|value| (((width + gap) / (value + gap)).floor() as usize).max(1));
    let count = match (count, fit) {
        (Some(count), Some(fit)) => count.min(fit),
        (count, fit) => count.or(fit)?,
    }
    .clamp(1, 64);
    if count < 2 {
        return None;
    }
    // The `column-rule` shorthand is split here when the cascade kept it whole.
    let (mut rule_width, mut rule_style, mut rule_color) = (
        style.get("column-rule-width").map(String::as_str),
        style.get("column-rule-style").map(String::as_str),
        style.get("column-rule-color").map(String::as_str),
    );
    for part in render::components(style.get("column-rule").map_or("", String::as_str), ' ') {
        if matches!(
            part,
            "none"
                | "hidden"
                | "dotted"
                | "dashed"
                | "solid"
                | "double"
                | "groove"
                | "ridge"
                | "inset"
                | "outset"
        ) {
            rule_style = rule_style.or(Some(part));
        } else if render::color(part).is_some() {
            rule_color = rule_color.or(Some(part));
        } else {
            rule_width = rule_width.or(Some(part));
        }
    }
    let rule = rule_style
        .filter(|kind| !matches!(*kind, "none" | "hidden"))
        .map(|kind| {
            let width = rule_width
                .map_or(Some(3.), |value| units.resolve(value, width))
                .unwrap_or(0.);
            let color = rule_color
                .or_else(|| style.get("color").map(String::as_str))
                .and_then(render::color)
                .map_or(gpui::black(), |(ink, alpha)| {
                    rgba(ink << 8 | alpha as u32).into()
                });
            (width, color, border_style(kind))
        })
        .filter(|(width, ..)| *width > 0.);
    Some((count, gap, rule))
}

/// Vertical lines centered in the gaps between columns.
fn column_rules(
    count: usize,
    gap: f32,
    width: f32,
    color: gpui::Hsla,
    kind: gpui::BorderStyle,
) -> AnyElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let column = (f32::from(bounds.size.width) - gap * (count - 1) as f32) / count as f32;
            for index in 1..count {
                let x = bounds.left() + px(index as f32 * (column + gap) - gap / 2. - width / 2.);
                window.paint_quad(gpui::quad(
                    Bounds::new(point(x, bounds.top()), size(px(width), bounds.size.height)),
                    gpui::Corners::default(),
                    gpui::transparent_black(),
                    gpui::Edges {
                        left: px(width),
                        ..Default::default()
                    },
                    color,
                    kind,
                ));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

fn border_style(kind: &str) -> gpui::BorderStyle {
    match kind {
        "dashed" => gpui::BorderStyle::Dashed,
        "dotted" => gpui::BorderStyle::Dotted,
        "double" => gpui::BorderStyle::Double,
        _ => gpui::BorderStyle::Solid,
    }
}

/// The darker or lighter shade of a 3D border, like WebKit.
fn shade_border(color: gpui::Hsla, dark: bool) -> gpui::Hsla {
    let rgba = color.to_rgb();
    let value = rgba.r.max(rgba.g).max(rgba.b);
    let factor = if value <= 0. {
        return if dark {
            color
        } else {
            gpui::Hsla::from(gpui::Rgba {
                r: 0.33,
                g: 0.33,
                b: 0.33,
                a: rgba.a,
            })
        };
    } else if dark {
        ((value - 0.33) / value).max(0.)
    } else {
        (value + 0.33).min(1.) / value
    };
    gpui::Rgba {
        r: (rgba.r * factor).min(1.),
        g: (rgba.g * factor).min(1.),
        b: (rgba.b * factor).min(1.),
        a: rgba.a,
    }
    .into()
}

fn inset_shadows(shadows: Vec<gpui::BoxShadow>, radius: [f32; 4], borders: [f32; 4]) -> AnyElement {
    let inner =
        |corner: usize, a: usize, b: usize| (radius[corner] - borders[a].max(borders[b])).max(0.);
    let radii = crate::effects::corners([
        inner(0, 0, 3),
        inner(1, 0, 1),
        inner(2, 2, 1),
        inner(3, 2, 3),
    ]);
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| window.paint_inset_shadows(bounds, radii, &shadows),
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

fn svg_layer(source: String, id: usize, view: gpui::Entity<WebView>) -> AnyElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let [width, height] = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            if width <= 0. || height <= 0. {
                return;
            }
            let scale = window.scale_factor();
            let raster =
                [width, height].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
            let view = view.read(cx);
            let mut host = view.host.borrow_mut();
            let state = host.dom.entry(view.id).or_default();
            if state.svgs.get(&id).is_none_or(|(old, image)| {
                *old != source
                    || u32::from(image.size(0).width) != raster[0]
                    || u32::from(image.size(0).height) != raster[1]
            }) {
                match crate::canvas::svg(&source, raster[0], raster[1], None) {
                    Ok(image) => {
                        if let Some((_, old)) = state.svgs.insert(id, (source.clone(), image)) {
                            let _ = window.drop_image(old);
                        }
                    }
                    Err(error) => eprintln!("apps: SVG: {error}"),
                }
            }
            let image = state.svgs.get(&id).map(|(_, image)| image.clone());
            drop(host);
            if let Some(image) = image {
                let _ =
                    window.paint_image(bounds, bounds, gpui::Corners::default(), image, 0, false);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_trust_and_optional_preloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.html");
        std::fs::write(
            &path,
            "<script type='preload'>__apps.ipcRenderer.send('pwned')</script><body>safe</body>",
        )
        .unwrap();
        let files = BBTreeMap::from([(
            "app.js".into(),
            format!(
                "define({});",
                serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":"globalThis.pwned=0;__apps.ipcMain.on('pwned',()=>pwned++);__apps.app.whenReady().then(()=>{globalThis.win=new __apps.BrowserWindow();win.load('<body>first</body>')})"})
            ),
        )]);
        let mut desktop = Desktop::new(&files).unwrap();
        desktop.main.eval("__appsEvent('ready')", "test").unwrap();
        desktop.pump(None, None).unwrap();
        assert!(desktop.views[&1].preload.is_none());
        desktop
            .main
            .eval("win.webContents.send('unused',42)", "test")
            .unwrap();
        let url = url::Url::from_file_path(&path).unwrap();
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
        assert!(desktop.views[&1].preload.is_none());
        assert_eq!(desktop.main.eval("pwned", "test").unwrap(), "0");
        desktop
            .main
            .eval(
                &format!("win.loadFile({})", serde_json::json!(path)),
                "test",
            )
            .unwrap();
        desktop.pump(None, None).unwrap();
        assert!(desktop.views[&1].preload.is_some());
        assert_eq!(desktop.main.eval("pwned", "test").unwrap(), "1");
        assert_eq!(
            desktop.views[&1]
                .renderer
                .as_ref()
                .unwrap()
                .eval("location.protocol", "test")
                .unwrap(),
            "file:"
        );
    }

    #[test]
    fn check_reports_page_script_errors() {
        let files = BBTreeMap::from([(
            "app.js".into(),
            format!(
                "define({});",
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
                "define({});",
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
                    "define({});",
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
                "purple"
            );
        }
    }
}
