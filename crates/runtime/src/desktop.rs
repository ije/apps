use crate::{
    host::{Host, Page, Role},
    html::Document,
    jsc::{Api, Engine},
    render::{self, ComputedStyles},
};
use anyhow::{Context as _, Result, bail, ensure};
use bbtree::unsync::BBTreeMap;
use gpui::{
    AnyElement, App, Application, Bounds, Context, ElementInputHandler, EntityInputHandler,
    FocusHandle, KeyDownEvent, KeyUpEvent, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, SharedString, TitlebarOptions, UTF16Selection, Window, WindowBounds, WindowHandle,
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
                                cx.new(|cx|{let focus = cx.focus_handle(); window.focus(&focus); WebView{id,runtime,host,images:HashMap::new(),focus,browsers:HashMap::new()}})
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
                        renderer.eval("__appsSyncBridge()", "apps:bridge")?;
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
                    let _ = cx.update(|cx| Self::drive(wake, cx));
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
        let application = Application::new();
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
                        if cx.update(|cx| Self::drive(runtime, cx)).is_err() {
                            break;
                        }
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
            std::mem::take(&mut state.frame_requested)
        };
        if frame_requested {
            if let Err(error) = self.evaluate("__appsAnimationFrame()") {
                eprintln!("apps: {error}");
            }
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
        let styles = crate::dom::styles(&mut doc, dom, [width, height], Some(cx.text_system()));
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
        dom.svgs.retain(|id, (_, image)| {
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
        if !changed.is_empty() {
            dom.grids.retain(|id, _| !changed.contains(id));
            dom.measurements.retain(|(id, ..), _| !changed.contains(id));
            dom.inline_text.retain(|id, _| !changed.contains(id));
        }
        dom.text_layouts.clear();
        let measured = RefCell::new(std::mem::take(&mut dom.measurements));
        let inline_text = RefCell::new(std::mem::take(&mut dom.inline_text));
        drop(host);
        let root = element(
            &Build {
                doc: &doc,
                styles: &styles,
                text: window.text_system(),
                pixel_ratio: window.scale_factor(),
                view: self,
                layers: Rc::default(),
                measured: &measured,
                inline_text: &inline_text,
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

fn css_background(
    style: &render::Style,
    units: render::Lengths,
    id: usize,
    layer: usize,
    view: gpui::Entity<WebView>,
) -> AnyElement {
    let source = style.get("background-image").cloned().unwrap_or_default();
    let tile = style.get("background-size").cloned().unwrap_or_default();
    let position = style
        .get("background-position")
        .cloned()
        .unwrap_or_default();
    gpui::canvas(|_, _, _| (), move |bounds, _, window, cx| {
        let (repeating, body) = if let Some(body) = source.strip_prefix("repeating-linear-gradient(") { (true, body) } else if let Some(body) = source.strip_prefix("linear-gradient(") { (false, body) } else { return };
        let Some(body) = body.strip_suffix(')') else { return };
        let parts = render::components(body, ',');
        let Some(angle) = parts.first().and_then(|v| v.trim().strip_suffix("deg")).and_then(|v| v.parse::<f32>().ok()) else { return };
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        if width <= 0. || height <= 0. { return; }
        let dimensions = render::components(&tile, ' ');
        let tw = dimensions.first().and_then(|v| units.resolve(v, width)).unwrap_or(width).max(0.1);
        let th = dimensions.get(1).or(dimensions.first()).and_then(|v| units.resolve(v, height)).unwrap_or(height).max(0.1);
        let positions = render::components(&position, ' ');
        let x = positions.first().and_then(|v| units.resolve(v, width - tw)).unwrap_or(0.);
        let y = positions.get(1).and_then(|v| units.resolve(v, height - th)).unwrap_or(0.);
        let dx = angle.to_radians().sin();
        let dy = -angle.to_radians().cos();
        let length = tw * dx.abs() + th * dy.abs();
        let mut stops = Vec::new();
        let stop_count = parts.len().saturating_sub(1);
        for (index, stop) in parts.iter().skip(1).enumerate() {
            let parts = render::components(stop.trim(), ' ');
            let Some((color, alpha)) = render::color(parts[0]) else { return };
            let offset = parts.get(1).and_then(|v| units.resolve(v, length)).unwrap_or(index as f32 * length / (stop_count.max(2) - 1) as f32);
            stops.push((offset, color, alpha));
        }
        if stops.len() < 2 { return; }
        let start = if repeating { stops[0].0 } else { 0. };
        let period = if repeating { stops.last().unwrap().0 - start } else { length };
        if period <= 0. { return; }
        let x1 = tw / 2. - dx * length / 2. + dx * start;
        let y1 = th / 2. - dy * length / 2. + dy * start;
        let stops = stops.into_iter().map(|(offset, color, alpha)| format!("<stop offset=\"{}\" stop-color=\"#{color:06x}\" stop-opacity=\"{}\"/>", ((offset - start) / period).clamp(0., 1.), alpha as f32 / 255.)).collect::<String>();
        let source = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\"><defs><linearGradient id=\"g\" gradientUnits=\"userSpaceOnUse\" x1=\"{x1}\" y1=\"{y1}\" x2=\"{}\" y2=\"{}\" spreadMethod=\"{}\">{stops}</linearGradient><pattern id=\"p\" patternUnits=\"userSpaceOnUse\" width=\"{tw}\" height=\"{th}\" x=\"{x}\" y=\"{y}\"><rect width=\"{tw}\" height=\"{th}\" fill=\"url(#g)\"/></pattern></defs><rect width=\"100%\" height=\"100%\" fill=\"url(#p)\"/></svg>", x1 + dx * period, y1 + dy * period, if repeating { "repeat" } else { "pad" });
        let view = view.read(cx);
        let mut host = view.host.borrow_mut();
        let backgrounds = &mut host.dom.entry(view.id).or_default().backgrounds;
        let scale = window.scale_factor();
        let rw = (width * scale).ceil().clamp(1., 4096.) as u32;
        let rh = (height * scale).ceil().clamp(1., 4096.) as u32;
        if backgrounds.get(&(id, layer)).is_none_or(|(old, image)| old != &source || u32::from(image.size(0).width) != rw || u32::from(image.size(0).height) != rh)
            && let Ok(image) = crate::canvas::svg(&source, rw, rh)
            && let Some((_, old)) = backgrounds.insert((id, layer), (source, image)) { let _ = window.drop_image(old); }
        if let Some((_, image)) = backgrounds.get(&(id, layer)) { let _ = window.paint_image(bounds, gpui::Corners::default(), image.clone(), 0, false); }
    }).absolute().left_0().top_0().size_full().into_any_element()
}

struct DomBox {
    inner: Option<gpui::Stateful<gpui::Div>>,
    animations: Vec<render::Animation>,
    opacity: f32,
    visible: bool,
    id: usize,
    view: gpui::Entity<WebView>,
    sticky: Option<(gpui::ScrollHandle, [f32; 4])>,
    inset: [Option<f32>; 4],
    translate: Vec<[String; 2]>,
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
    type PrepaintState = ();
    fn id(&self) -> Option<gpui::ElementId> {
        Some(gpui::ElementId::Name(format!("dom-{}", self.id).into()))
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        global_id: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, AnyElement) {
        let mut inner = self.inner.take().unwrap();
        if self.animations.is_empty() {
            let mut inner = inner.into_any_element();
            return (inner.request_layout(window, cx), inner);
        }
        let opacity = window.with_element_state(global_id.unwrap(), |state, window| {
            let mut clocks: Vec<(String, f64, std::time::Instant, bool)> =
                state.unwrap_or_default();
            clocks.truncate(self.animations.len());
            let now = std::time::Instant::now();
            let mut opacity = self.opacity;
            for (index, animation) in self.animations.iter().enumerate() {
                if clocks
                    .get(index)
                    .is_none_or(|clock| clock.0 != animation.name)
                {
                    let clock = (animation.name.clone(), 0., now, animation.paused);
                    if index == clocks.len() {
                        clocks.push(clock);
                    } else {
                        clocks[index] = clock;
                    }
                }
                let clock = &mut clocks[index];
                if !clock.3 {
                    clock.1 += now.duration_since(clock.2).as_secs_f64();
                }
                clock.2 = now;
                clock.3 = animation.paused;
                let (value, running) = animation.sample(clock.1);
                if let Some(value) = value {
                    opacity = value;
                }
                if running && self.visible {
                    window.request_animation_frame();
                }
            }
            (opacity, clocks)
        });
        inner = inner.opacity(opacity);
        let mut inner = inner.into_any_element();
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
    ) {
        let mut offset = point(px(0.), px(0.));
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
        for [x, y] in &self.translate {
            let translation = point(
                px(self
                    .units
                    .resolve(x, bounds.size.width.into())
                    .unwrap_or(0.)),
                px(self
                    .units
                    .resolve(y, bounds.size.height.into())
                    .unwrap_or(0.)),
            );
            offset += translation;
            bounds.origin += translation;
        }
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
                    bounds.origin.x.into(),
                    bounds.origin.y.into(),
                    bounds.size.width.into(),
                    bounds.size.height.into(),
                ],
            );
        if let Some((browser, _)) = view.browsers.get(&self.id) {
            let _ = browser.set_bounds([
                f32::from(bounds.origin.x) as f64,
                f32::from(bounds.origin.y) as f64,
                f32::from(bounds.size.width) as f64,
                f32::from(bounds.size.height) as f64,
            ]);
        }
        window.with_element_offset(offset, |window| {
            inner.prepaint(window, cx);
        });
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        inner: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        inner.paint(window, cx);
    }
}

struct TextHighlight {
    range: Range<usize>,
    color: Option<gpui::Hsla>,
    background: gpui::Hsla,
}

fn paint_text(
    line: &gpui::LineLayout,
    wraps: &[gpui::WrapBoundary],
    origin: Point<Pixels>,
    line_height: Pixels,
    runs: &[gpui::TextRun],
    offset: usize,
    highlights: &[TextHighlight],
    window: &mut Window,
) {
    let mut start = 0;
    let mut left = px(0.);
    for (row, boundary) in wraps.iter().map(Some).chain([None]).enumerate() {
        let (end, right) = boundary.map_or((line.len, line.width), |boundary| {
            let glyph = &line.runs[boundary.run_ix].glyphs[boundary.glyph_ix];
            (glyph.index, glyph.position.x)
        });
        for highlight in highlights {
            let a = highlight.range.start.max(offset + start);
            let b = highlight.range.end.min(offset + end);
            if a >= b { continue; }
            let x = line.x_for_index(a - offset);
            let end_x = if b == offset + end { right } else { line.x_for_index(b - offset) };
            if end_x > x && highlight.background.a > 0. {
                window.paint_quad(gpui::fill(
                    Bounds::new(point(origin.x + x - left, origin.y + line_height * row), size(end_x - x, line_height)),
                    highlight.background,
                ));
            }
        }
        start = end;
        left = right;
    }
    let baseline = (line_height - line.ascent - line.descent) / 2. + line.ascent;
    let mut wrap = 0;
    let mut left = px(0.);
    let mut run_index = 0;
    let mut run_end = runs.first().map_or(0, |run| run.len);
    for (run_ix, run) in line.runs.iter().enumerate() {
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
            let color = highlights.iter().find(|highlight| highlight.range.contains(&index))
                .and_then(|highlight| highlight.color)
                .unwrap_or(runs[run_index].color);
            if color.a == 0. { continue; }
            let position = point(origin.x + glyph.position.x - left, origin.y + line_height * wrap + baseline);
            if glyph.is_emoji {
                let _ = window.paint_emoji(position, run.font_id, glyph.id, line.font_size);
            } else {
                let _ = window.paint_glyph(position, run.font_id, glyph.id, line.font_size, color);
            }
        }
    }
}

struct InlineText {
    inner: gpui::StyledText,
    data: Rc<render::InlineText>,
    view: gpui::Entity<WebView>,
}

impl IntoElement for InlineText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for InlineText {
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
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, ()) {
        self.inner.request_layout(id, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner
            .prepaint(id, inspector, bounds, state, window, cx);
        let view = self.view.read(cx);
        let mut host = view.host.borrow_mut();
        let dom = host.dom.entry(view.id).or_default();
        let layout = self.inner.layout();
        for (id, range) in &self.data.ranges {
            if let Some((a, b)) = layout
                .position_for_index(range.start)
                .zip(layout.position_for_index(range.end))
            {
                dom.bounds.insert(
                    *id,
                    [
                        if a.y == b.y { a.x.into() } else { bounds.left().into() },
                        a.y.into(),
                        if a.y == b.y { (b.x - a.x).into() } else { bounds.size.width.into() },
                        (b.y - a.y + layout.line_height()).into(),
                    ],
                );
                dom.text_layouts
                    .insert(*id, (layout.clone(), self.data.text.clone(), range.clone()));
            }
        }
    }
    fn paint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        painted: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
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
                    if let Some((a, b)) = layout
                        .position_for_index(range.start)
                        .zip(layout.position_for_index(range.end))
                    {
                        let font = cx.text_system().resolve_font(&render::font(style));
                        let height = cx.text_system().ascent(font, px(units.font.em))
                            - cx.text_system().descent(font, px(units.font.em));
                        let radii = ["top-left", "top-right", "bottom-right", "bottom-left"].map(|corner| {
                            px(style
                                .get(&format!("border-{corner}-radius"))
                                .or_else(|| style.get("border-radius"))
                                .and_then(|v| units.resolve(v, (b.x - a.x).into()))
                                .unwrap_or(0.))
                        });
                        window.paint_quad(
                            gpui::fill(
                                Bounds::new(
                                    point(a.x, a.y + (layout.line_height() - height) / 2.),
                                    size(b.x - a.x, height),
                                ),
                                rgba(ink),
                            )
                            .corner_radii(gpui::Corners {
                                top_left: radii[0],
                                top_right: radii[1],
                                bottom_right: radii[2],
                                bottom_left: radii[3],
                            }),
                        );
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
                    if let Some(position) = layout.position_for_index(start) {
                        window.paint_quad(gpui::fill(
                            Bounds::new(position, size(px(1.), layout.line_height())), rgba(ink),
                        ));
                    }
                } else if start != end {
                    let (color, background) = render::selection_colors(&styles.values[*id], styles.selections.get(id));
                    highlights.push(TextHighlight {
                        range: start.min(end)..start.max(end),
                        color: color.map(|color| rgba(color).into()),
                        background: rgba(background).into(),
                    });
                }
            }
        }
        if highlights.is_empty() {
            self.inner.paint(id, inspector, bounds, state, painted, window, cx);
        } else {
            let layout = self.inner.layout();
            let mut offset = 0;
            while let Some(line) = layout.line_layout_for_index(offset) {
                let origin = layout.position_for_index(offset).unwrap();
                paint_text(&line.unwrapped_layout, &line.wrap_boundaries, origin,
                    layout.line_height(), &self.data.runs, offset, &highlights, window);
                offset += line.len() + 1;
                if offset > self.data.text.len() { break; }
            }
        }
    }
}

type PaintLayers = Rc<RefCell<Vec<(i32, Rc<RefCell<AnyElement>>, gpui::ContentMask<Pixels>)>>>;

struct Stacked {
    inner: Option<AnyElement>,
    layers: PaintLayers,
    order: i32,
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
        self.layers
            .borrow_mut()
            .push((self.order, inner.clone(), window.content_mask()));
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

fn paint_layers(layers: PaintLayers, negative: bool) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| (),
        move |_, _, window, cx| {
            let mut layers = layers.borrow_mut();
            layers.sort_by_key(|(order, _, _)| *order);
            for (_, inner, mask) in layers
                .iter()
                .filter(|(order, _, _)| (*order < 0) == negative)
            {
                window.with_content_mask(Some(mask.clone()), |window| {
                    inner.borrow_mut().paint(window, cx)
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
    measured: &'a RefCell<crate::grid::Measurements>,
    inline_text: &'a RefCell<HashMap<usize, Option<Rc<render::InlineText>>>>,
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
        measured,
        inline_text,
    } = *build;
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    let order = style
        .get("z-index")
        .and_then(|value| value.parse::<i32>().ok());
    let positioned = style.get("position").is_some_and(|value| value != "static");
    let stacking = id == 0
        || style
            .get("isolation")
            .is_some_and(|value| value == "isolate")
        || positioned && order.is_some()
        || style.get("transform").is_some_and(|value| value != "none")
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
    let selection_colors = render::selection_colors(style, styles.selections.get(&id));
    let face = render::font(style);
    let mut el = div()
        .id(id)
        .flex()
        .flex_col()
        .flex_shrink_0()
        .text_size(px(units.font.em))
        .font_family(face.family)
        .font_weight(face.weight)
        .line_height(px(units.font.lh));
    if stacking {
        el = el.child(paint_layers(build.layers.clone(), true));
    }
    if !node.children.is_empty()
        && node.children.iter().all(|child| {
            matches!(
                doc.nodes[*child].tag.as_str(),
                "#text" | "span" | "strong" | "b" | "em" | "i" | "a" | "code" | "small" | "br"
            )
        })
    {
        el = el.flex_row().flex_wrap();
        if !style
            .get("display")
            .is_some_and(|value| matches!(value.as_str(), "flex" | "grid"))
        {
            el = match style.get("text-align").map(String::as_str) {
                Some("right" | "end") => el.justify_end(),
                Some("center") => el.justify_center(),
                _ => el,
            };
        }
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
    if node.tag == "img" {
        if let Some(image) = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|state| state.images.get(&id))
            .cloned()
        {
            let size = image.size(0);
            if !style.contains_key("width") {
                el = el.w(px(u32::from(size.width) as f32));
            }
            if !style.contains_key("height") {
                el = el.h(px(u32::from(size.height) as f32));
            }
            el = el.child(img(image).size_full());
        } else if let Some(source) = node.attrs.get("src") {
            el = el.child(img(source.clone()).size_full());
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
        let mut host = view.host.borrow_mut();
        let state = host.dom.entry(view.id).or_default();
        let raster_width = (width * pixel_ratio).ceil().clamp(1., 4096.) as u32;
        let raster_height = (height * pixel_ratio).ceil().clamp(1., 4096.) as u32;
        if state.svgs.get(&id).is_none_or(|(old, image)| {
            old != &source
                || u32::from(image.size(0).width) != raster_width
                || u32::from(image.size(0).height) != raster_height
        }) {
            match crate::canvas::svg(&source, raster_width, raster_height) {
                Ok(image) => {
                    state.svgs.insert(id, (source, image));
                }
                Err(error) => eprintln!("apps: SVG: {error}"),
            }
        }
        if let Some((_, image)) = state.svgs.get(&id) {
            el = el
                .w(px(width))
                .h(px(height))
                .child(img(image.clone()).absolute().size_full());
        }
    }
    if node.tag == "iframe" {
        el = el.min_w(px(300.)).min_h(px(150.));
    }
    let editable = matches!(node.tag.as_str(), "input" | "textarea");
    if editable {
        if !style.contains_key("width") {
            el = el.w(px(180.));
        }
        if !style.contains_key("height") {
            el = el.min_h(px(if node.tag == "textarea" {
                80.
            } else {
                units.font.lh
            }));
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
                        rgba(0x888888ff)
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
                                    Bounds::new(point(bounds.left() + x, top), size(px(1.), px(line_height))),
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
                            let _ = line.paint(point(bounds.left(), top), px(line_height), window, cx);
                        } else {
                            paint_text(&line, &[], point(bounds.left(), top), px(line_height),
                                std::slice::from_ref(&run), 0, &highlights, window);
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
        .or_else(|| style.get("background"))
        .and_then(|s| render::color(s))
    {
        let color = rgba((color.0 << 8) | color.1 as u32);
        if matches!(
            node.tag.as_str(),
            "span" | "strong" | "b" | "em" | "i" | "a" | "code" | "small"
        ) && style.get("display").is_none_or(|value| value == "inline")
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
    if style.contains_key("background-image") {
        el = el.child(css_background(style, units, id, 0, cx.entity()));
    }
    if let Some(pseudos) = styles.pseudos.get(&id) {
        for (index, pseudo) in pseudos.iter().enumerate() {
            let Some(content) = pseudo
                .get("content")
                .filter(|v| !matches!(v.as_str(), "none" | "normal"))
            else {
                continue;
            };
            let mut decoration = div().relative();
            if pseudo.get("position").is_some_and(|v| v == "absolute") {
                decoration = decoration.absolute();
            }
            for (name, value) in pseudo {
                let base = if matches!(name.as_str(), "height" | "top" | "bottom") {
                    containing[1]
                } else {
                    containing[0]
                };
                match name.as_str() {
                    "width" | "height" | "min-width" => {
                        let length: gpui::Length = if let Some(percent) =
                            value.strip_suffix('%').and_then(|v| v.parse::<f32>().ok())
                        {
                            relative(percent / 100.).into()
                        } else if let Some(value) = units.resolve(value, base) {
                            px(value).into()
                        } else {
                            continue;
                        };
                        decoration = match name.as_str() {
                            "width" => decoration.w(length),
                            "height" => decoration.h(length),
                            _ => decoration.min_w(length),
                        };
                    }
                    "left" | "top" | "right" | "bottom" => {
                        if let Some(value) = units.resolve(value, base) {
                            decoration = match name.as_str() {
                                "left" => decoration.left(px(value)),
                                "top" => decoration.top(px(value)),
                                "right" => decoration.right(px(value)),
                                _ => decoration.bottom(px(value)),
                            };
                        }
                    }
                    "background-color" => {
                        if let Some((color, alpha)) = render::color(value) {
                            decoration = decoration.bg(rgba(color << 8 | alpha as u32));
                        }
                    }
                    _ => {}
                }
            }
            if pseudo.contains_key("background-image") {
                decoration =
                    decoration.child(css_background(pseudo, units, id, index + 1, cx.entity()));
            }
            let content = content.trim_matches(['\"', '\'']);
            if !content.is_empty() {
                decoration = decoration.child(SharedString::from(content.to_owned()));
            }
            let order = pseudo
                .get("z-index")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            if order != 0
                && pseudo
                    .get("position")
                    .is_some_and(|value| value != "static")
            {
                el = el.child(Stacked {
                    inner: Some(decoration.into_any_element()),
                    layers: build.layers.clone(),
                    order,
                });
            } else {
                el = el.child(decoration);
            }
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
    let margin = units.edges(style, "margin", containing[0]);
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
    for (name, value) in
        style.range::<str, _>((std::ops::Bound::Included("a"), std::ops::Bound::Unbounded))
    {
        let value = value.as_str();
        let base = if name.ends_with("height") {
            containing[1]
        } else {
            containing[0]
        };
        let length = || units.resolve(value, base);
        match name.as_str() {
            "width" => {
                el = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    el.w(relative(percent.max(0.) / 100.))
                } else if value == "auto" {
                    el.w_auto()
                } else if let Some(length) = length() {
                    el.w(px(length.max(0.)))
                } else {
                    el
                }
            }
            "height" => {
                el = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    el.h(relative(percent.max(0.) / 100.))
                } else if value == "auto" {
                    el.h_auto()
                } else if let Some(length) = length() {
                    el.h(px(length.max(0.)))
                } else {
                    el
                }
            }
            "min-width" | "max-width" | "min-height" | "max-height" => {
                let length: gpui::Length = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    relative(percent.max(0.) / 100.).into()
                } else if let Some(length) = length() {
                    px(length.max(0.)).into()
                } else {
                    continue;
                };
                el = match name.as_str() {
                    "min-width" => el.min_w(length),
                    "max-width" => el.max_w(length),
                    "min-height" => el.min_h(length),
                    _ => el.max_h(length),
                };
            }
            "gap"
            | "row-gap"
            | "column-gap"
            | "border-top-left-radius"
            | "border-top-right-radius"
            | "border-bottom-right-radius"
            | "border-bottom-left-radius"
            | "border-width"
            | "border-top-width"
            | "border-right-width"
            | "border-bottom-width"
            | "border-left-width" => {
                let Some(length) = length() else { continue };
                let length = px(length.max(0.));
                el = match name.as_str() {
                    "gap" => el.gap(length),
                    "row-gap" => el.gap_y(length),
                    "column-gap" => el.gap_x(length),
                    "border-top-left-radius" => el.rounded_tl(length),
                    "border-top-right-radius" => el.rounded_tr(length),
                    "border-bottom-right-radius" => el.rounded_br(length),
                    "border-bottom-left-radius" => el.rounded_bl(length),
                    "border-top-width" => el.border_t(length),
                    "border-right-width" => el.border_r(length),
                    "border-bottom-width" => el.border_b(length),
                    "border-left-width" => el.border_l(length),
                    _ => el.border(length),
                };
            }
            "box-shadow" => el.style().box_shadow = Some(units.shadows(value)),
            "border-color" => {
                if let Some((ink, a)) = render::color(value) {
                    el = el.border_color(rgba((ink << 8) | a as u32));
                }
            }
            "display" if value == "grid" => el = el.grid(),
            "display" if value == "flex" => el = el.flex().flex_row(),
            "flex-direction" if value == "column" => el = el.flex_col(),
            "flex-wrap" if value == "wrap" => el = el.flex_wrap(),
            "flex-wrap" if value == "nowrap" => el = el.flex_nowrap(),
            "flex-grow" => {
                el.style().flex_grow = Some(value.parse().unwrap_or(0.));
            }
            "flex-shrink" => {
                el.style().flex_shrink = Some(value.parse().unwrap_or(1.));
            }
            "flex" if value == "1" => el = el.flex_1(),
            "align-items" => {
                el = match value {
                    "center" => el.items_center(),
                    "flex-end" | "end" => el.items_end(),
                    "flex-start" | "start" => el.items_start(),
                    _ => {
                        el.style().align_items = Some(gpui::AlignItems::Stretch);
                        el
                    }
                }
            }
            "justify-content" => {
                el = match value {
                    "center" => el.justify_center(),
                    "space-between" => el.justify_between(),
                    "space-around" => el.justify_around(),
                    "flex-end" | "end" => el.justify_end(),
                    _ => el.justify_start(),
                }
            }
            "text-align" => {
                el = match value {
                    "center" => el.text_center(),
                    "right" => el.text_right(),
                    _ => el.text_left(),
                }
            }
            "position" if matches!(value, "absolute" | "fixed") => el = el.absolute(),
            "left" | "right" | "top" | "bottom"
                if style.get("position").is_some_and(|v| v != "sticky") => {
                if let Some(length) = length() {
                    el = match name.as_str() {
                        "left" => el.left(px(length)),
                        "right" => el.right(px(length)),
                        "top" => el.top(px(length)),
                        _ => el.bottom(px(length)),
                    };
                }
            }
            "visibility" => {
                el.style().visibility = Some(if value == "hidden" || value == "collapse" {
                    gpui::Visibility::Hidden
                } else {
                    gpui::Visibility::Visible
                })
            }
            "opacity" => el = el.opacity(value.parse::<f32>().unwrap_or(1.).clamp(0., 1.)),
            "white-space" if matches!(value, "nowrap" | "pre") => {
                el = el.whitespace_nowrap().flex_nowrap()
            }
            "overflow" | "overflow-x" | "overflow-y" => {
                let overflow = match value {
                    "auto" | "scroll" => gpui::Overflow::Scroll,
                    "hidden" => gpui::Overflow::Hidden,
                    "clip" => gpui::Overflow::Clip,
                    _ => gpui::Overflow::Visible,
                };
                if name != "overflow-y" {
                    el.style().overflow.x = Some(overflow);
                }
                if name != "overflow-x" {
                    el.style().overflow.y = Some(overflow);
                }
            }
            "cursor" if value == "pointer" => el = el.cursor_pointer(),
            "cursor" if value == "grab" => el = el.cursor(gpui::CursorStyle::OpenHand),
            "cursor" if value == "grabbing" => el = el.cursor(gpui::CursorStyle::ClosedHand),
            _ => {}
        }
    }
    let borders = ["border-top-width", "border-right-width", "border-bottom-width", "border-left-width"].map(|name| {
        style
            .get(name)
            .or_else(|| style.get("border-width"))
            .and_then(|v| units.resolve(v, containing[0]))
            .unwrap_or(0.)
            .max(0.)
    });
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
    let preserve = style
        .get("white-space")
        .is_some_and(|v| matches!(v.as_str(), "pre" | "pre-wrap" | "break-spaces"))
        || node.tag == "pre";
    if !node.text.is_empty() && !editable {
        let text = if preserve && node.text == "\n" {
            el = el.min_h(px(units.font.lh));
            String::new()
        } else if preserve {
            node.text.clone()
        } else {
            node.text.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        if !text.is_empty() {
            let runs = vec![gpui::TextRun {
                len: text.len(),
                font: render::font(style),
                color: style.get("color").and_then(|value| render::color(value))
                    .map(|(color, alpha)| rgba(color << 8 | alpha as u32).into())
                    .unwrap_or_else(gpui::black),
                background_color: None,
                underline: None,
                strikethrough: None,
            }];
            el = el.child(InlineText {
                inner: gpui::StyledText::new(text.clone()).with_runs(runs.clone()),
                data: Rc::new(render::InlineText {
                    ranges: vec![(id, 0..text.len())],
                    text: text.into(),
                    runs,
                    backgrounds: Vec::new(),
                }),
                view: cx.entity(),
            });
        }
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
        let inner = gpui::StyledText::new(data.text.to_string()).with_runs(data.runs.clone());
        el = el.child(InlineText {
            inner,
            data,
            view: cx.entity(),
        });
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
            {
                content[axis] = value;
            } else if axis == 0 {
                content[axis] -= margin[1] + margin[3];
            }
            if let Some(value) = style
                .get(max)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].min(value);
            }
            if let Some(value) = style
                .get(min)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].max(value);
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
            content
        } else {
            container
        };
        let grid = if style.get("display").is_some_and(|v| v == "grid") {
            let cached = view
                .host
                .borrow()
                .dom
                .get(&view.id)
                .and_then(|state| state.grids.get(&id))
                .filter(|(width, height, inherited, _)| {
                    *width == content[0]
                        && *height == style.contains_key("height").then_some(content[1])
                        && inherited.as_ref() == slot.as_ref().map(|(_, grid)| grid)
                })
                .map(|(_, _, _, grid)| grid.clone());
            Some(cached.unwrap_or_else(|| {
                let grid = crate::grid::layout(
                    doc,
                    id,
                    styles,
                    content[0],
                    style.contains_key("height").then_some(content[1]),
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
                            style.contains_key("height").then_some(content[1]),
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
            if !style.contains_key("height") {
                el = el.h(px(grid.rows.iter().sum::<f32>()
                    + grid.rows.len().saturating_sub(1) as f32 * grid.gap[1]
                    + padding[0]
                    + padding[2]
                    + borders[0]
                    + borders[2]));
            }
        }
        let mut children = node.children.clone();
        if grid.is_some() {
            children.sort_by_key(|child| {
                styles.values[*child]
                    .get("z-index")
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            });
        }
        for child in children {
            if preserve
                || !(doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim().is_empty())
            {
                let placement = grid.as_ref().and_then(|grid| {
                    grid.cells
                        .iter()
                        .find(|(id, _)| *id == child)
                        .map(|(_, cell)| {
                            let mut bounds = grid.bounds(*cell);
                            bounds[0] += padding[3];
                            bounds[1] += padding[0];
                            let [column, row, columns, rows] = *cell;
                            (
                                bounds,
                                crate::grid::Grid {
                                    columns: grid.columns[column..column + columns].to_vec(),
                                    rows: grid.rows[row..row + rows].to_vec(),
                                    gap: grid.gap,
                                    cells: Vec::new(),
                                },
                            )
                        })
                });
                el = el.child(element(build, child, content, container, placement, cx));
            }
        }
    }
    if let Some((bounds, _)) = &slot {
        el = el
            .absolute()
            .left(px(bounds[0]))
            .top(px(bounds[1]))
            .w(px((bounds[2] - margin[1] - margin[3]).max(0.)))
            .h(px((bounds[3] - margin[0] - margin[2]).max(0.)));
    }
    if style
        .get("pointer-events")
        .is_none_or(|value| value != "none")
    {
        let path = ancestors.clone();
        el = el.on_mouse_down(MouseButton::Left, cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
            window.focus(&view.focus);
            cx.stop_propagation();
            Desktop::dispatch(view.runtime.clone(), view.id, "pointerdown", if hit_test { &[] } else { &path }, serde_json::json!({"pointerId":1,"pointerType":"mouse","isPrimary":true,"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y),"button":0,"buttons":1,"detail":event.click_count,"ctrlKey":event.modifiers.control,"metaKey":event.modifiers.platform,"altKey":event.modifiers.alt,"shiftKey":event.modifiers.shift}), cx);
        }));
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
            scroll_ancestors
                .into_iter()
                .skip(1)
                .find_map(|parent| {
                    let scroll = host.dom.get(&view.id)?.scroll.get(&parent)?.clone();
                    let style = &styles.values[parent];
                    let border = ["border-top-width", "border-right-width", "border-bottom-width", "border-left-width"].map(|name| {
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
    let mut translate = Vec::new();
    if let Some(value) = style.get("translate").filter(|value| *value != "none") {
        let parts = render::components(value, ' ');
        translate.push([
            parts[0].to_owned(),
            parts.get(1).unwrap_or(&"0").to_string(),
        ]);
    }
    if let Some(value) = style.get("transform") {
        for part in render::components(value, ' ') {
            let Some((name, arguments)) =
                part.strip_suffix(')').and_then(|part| part.split_once('('))
            else {
                continue;
            };
            let args = render::components(arguments, ',');
            match (name, args.as_slice()) {
                ("translateX", [x]) => translate.push([x.to_string(), "0".into()]),
                ("translateY", [y]) => translate.push(["0".into(), y.to_string()]),
                ("translate", [x]) => translate.push([x.to_string(), "0".into()]),
                ("translate", [x, y]) => translate.push([x.to_string(), y.to_string()]),
                _ => {}
            }
        }
    }
    let element = DomBox {
        inner: Some(el),
        animations: styles.animations.get(&id).cloned().unwrap_or_default(),
        opacity: style
            .get("opacity")
            .and_then(|value| value.parse().ok())
            .unwrap_or(1.),
        visible: style
            .get("visibility")
            .is_none_or(|value| value == "visible"),
        translate,
        units,
        id,
        view: cx.entity(),
        sticky,
        inset: ["top", "right", "bottom", "left"].map(|name| {
            style
                .get(name)
                .and_then(|v| units.resolve(v, containing[0]))
        }),
    }
    .into_any_element();
    if positioned && let Some(order) = order.filter(|order| *order != 0) {
        Stacked {
            inner: Some(element),
            layers: layers.clone(),
            order,
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
