pub mod browser;
pub mod devtools;
pub(crate) mod native;

use crate::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{Api, Engine},
    render::WebView,
};
use anyhow::{Context as _, Result, bail, ensure};
use bbtree::unsync::BBTreeMap;
use gpui::{
    App, Bounds, TitlebarOptions, WindowBounds, WindowHandle, WindowOptions, prelude::*, px, size,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

#[cfg(target_os = "macos")]
gpui::actions!(desktop, [Quit]);

pub(crate) struct View {
    pub(crate) handle: Option<WindowHandle<WebView>>,
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
    wake: Option<gpui::Task<()>>,
}

impl Desktop {
    pub fn from_bundle(bundle: crate::bundle::Bundle) -> Result<Self> {
        let mut desktop = Self::create(&bundle.files, bundle.packaged, bundle.root)?;
        desktop.pages = bundle.pages;
        Ok(desktop)
    }

    pub(crate) fn dispatch(
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
        Self::create(files, false, std::path::PathBuf::new())
    }

    fn create(
        files: &BBTreeMap<String, String>,
        packaged: bool,
        root: std::path::PathBuf,
    ) -> Result<Self> {
        let api = Api::load()?;
        let (_, name, version) =
            crate::bundle::package_metadata(files.get("package.json").map(String::as_str))?;
        let host = Rc::new(RefCell::new(Host {
            app_version: Some(version),
            app_name: Some(name.clone()),
            app_path: if root.as_os_str().is_empty() {
                std::env::current_dir().unwrap_or_default()
            } else {
                root
            },
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
                                cx.new(|cx|{let focus = cx.focus_handle(); window.focus(&focus, cx); cx.observe_window_activation(window,|view: &mut WebView,window,cx|{let runtime=view.runtime.clone();let event=format!("__appsEvent('{}',{})",if window.is_window_active(){"focus"}else{"blur"},view.id);cx.defer(move|cx|{if let Ok(app)=runtime.try_borrow()&&let Err(error)=app.main.eval(&event,"apps:focus"){eprintln!("apps: {error}");}Desktop::drive(runtime,cx);});}).detach(); WebView{id,runtime,host,images:HashMap::new(),focus,browsers:HashMap::new()}})
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
                                let browser = browser::Browser::new(
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
                                self.devtools
                                    .insert(id, devtools::DevTools::new(&format!("Window {id}"))?);
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
                let mut app = runtime.borrow_mut();
                app.error = Some(error);
                app.quit = true;
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
        let terminate = runtime.clone();
        let (urls, mut opened) = tokio::sync::mpsc::unbounded_channel::<Vec<String>>();
        let application = gpui_platform::application().with_quit_mode(gpui::QuitMode::Explicit);
        application.on_reopen(move |cx, visible| {
            if let Err(error) = reopen.borrow().main.eval(
                &format!("__appsEvent('activate',0,{visible})"),
                "apps:activate",
            ) {
                eprintln!("apps: {error}");
            }
            Self::drive(reopen.clone(), cx);
        });
        application.on_should_quit(move |cx| {
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
        application.on_open_urls(move |list| {
            let _ = urls.send(list);
        });
        application.run(move |cx| {
            let host = runtime.borrow().host.clone();
            cx.on_app_quit(move |_| {
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
                        let _ = std::process::Command::new(&command[0])
                            .args(&command[1..])
                            .spawn();
                    }
                    // AppKit exits with 0 after this hook, so a failure code must exit here.
                    if host.exit_code != 0 {
                        std::process::exit(host.exit_code);
                    }
                }
                async {}
            })
            .detach();
            let weak = Rc::downgrade(&runtime);
            cx.spawn(async move |cx| {
                while let Some(list) = opened.recv().await {
                    let Some(runtime) = weak.upgrade() else { break };
                    cx.update(|cx| {
                        for url in list {
                            let event = match url::Url::parse(&url)
                                .ok()
                                .filter(|url| url.scheme() == "file")
                                .and_then(|url| url.to_file_path().ok())
                            {
                                Some(path) => format!(
                                    "__appsEvent('open-file',0,{})",
                                    serde_json::json!(path)
                                ),
                                None => {
                                    format!("__appsEvent('open-url',0,{})", serde_json::json!(url))
                                }
                            };
                            if let Err(error) = runtime.borrow().main.eval(&event, "apps:open") {
                                eprintln!("apps: {error}");
                            }
                        }
                        Self::drive(runtime, cx);
                    });
                }
            })
            .detach();
            #[cfg(target_os = "macos")]
            {
                let mut changes = native::observe_theme();
                let weak = Rc::downgrade(&runtime);
                cx.spawn(async move |cx| {
                    while changes.recv().await.is_some() {
                        let Some(runtime) = weak.upgrade() else { break };
                        cx.update(|cx| Self::drive(runtime, cx));
                    }
                })
                .detach();
                cx.on_action(|_: &Quit, cx| cx.quit());
                cx.bind_keys([gpui::KeyBinding::new("cmd-q", Quit, None)]);
            }
            let startup = if cfg!(target_os = "macos") {
                "__appsInitMenu();__appsEvent('ready')"
            } else {
                "__appsEvent('ready')"
            };
            if let Err(error) = runtime.borrow().main.eval(startup, "apps:ready") {
                eprintln!("apps: {error}");
                runtime.borrow_mut().quit = true;
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
                match devtools::DevTools::new(&title) {
                    Ok(tools) => {
                        app.devtools.insert(id, tools);
                    }
                    Err(error) => {
                        app.error = Some(error);
                        app.quit = true;
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
                "define({});",
                serde_json::json!({"id":0,"imports":[],"exports":[],"stars":[],"code":code})
            ),
        )]);
        let mut desktop = Desktop::new(&files).unwrap();
        desktop.main.eval("__appsEvent('ready')", "test").unwrap();
        desktop.main.eval("__appsEvent('focus',1);__appsEvent('activate',0,true);__appsEvent('open-url',0,'myapp://x');__appsEvent('open-file',0,'/tmp/a');__appsEvent('before-quit')", "test").unwrap();
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
                    "define({});",
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
