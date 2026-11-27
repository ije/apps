use super::{Desktop, RELOADED, View, scripted, view::PageView};
use crate::host::link::ToMain;
use crate::{dom::Document, host::context::JsContext, jsc::Role};
use anyhow::{Context as _, Result, bail, ensure};
use renderer::{App, Rect, WindowChange, WindowOptions, point, px, size};
use std::{
    borrow::Cow,
    cell::RefCell,
    rc::{Rc, Weak},
};

impl Desktop {
    // Opens the window of a new BrowserWindow.
    pub(super) fn open(
        &mut self,
        id: u64,
        args: &serde_json::Value,
        cx: Option<&mut App>,
        runtime: Option<&Rc<RefCell<Self>>>,
    ) -> Result<()> {
        ensure!(!self.views.contains_key(&id), "Window already exists");
        let options = &args[2];
        let number = |key: &str, default: f64| options[key].as_f64().unwrap_or(default) as f32;
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
        (self.host.borrow().store.borrow_mut())
            .viewports
            .insert(id, [width, height]);
        let transparent = options["transparent"] == true;
        let vibrancy = options["vibrancy"]
            .as_i64()
            .filter(|_| cfg!(target_os = "macos"));
        let background = options["backgroundColor"]
            .as_str()
            .and_then(crate::css::value::color::color)
            .unwrap_or(if transparent || vibrancy.is_some() {
                (0, 0)
            } else {
                (0xffffff, 255)
            });
        self.host.borrow_mut().set_background(id, background);
        let (mut data, mut input) = (Weak::new(), Weak::new());
        let handle = if let Some(cx) = cx {
            let runtime = runtime.unwrap().clone();
            let host = self.host.clone();
            let frame = options["frame"] != false;
            let style = options["titleBarStyle"].as_str().unwrap_or("default");
            let hidden = !frame || style != "default";
            let lights = &options["trafficLightPosition"];
            let traffic_light_position = match (lights["x"].as_f64(), lights["y"].as_f64()) {
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
                let view = PageView::new(runtime, host, id, window.id());
                (data, input) = (Rc::downgrade(&view.page), Rc::downgrade(&view.input));
                Box::new(view)
            })?;
            cx.update_window(handle, |window, _| {
                self.link
                    .main(ToMain::Window(id, window.id(), window.control().clone()));
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
                            || options["visualEffectState"].as_i64().unwrap_or(0) != 0,
                        WindowChange::Vibrancy(
                            vibrancy.map(|material| material as isize),
                            options["visualEffectState"].as_i64().unwrap_or(0) as isize,
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
                (self.host.borrow().store.borrow_mut())
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
                input,
                title: None,
                titled: 0,
                preload: None,
                renderer: None,
                fullscreen: super::HtmlFullscreen::Off,
                focused: true,
            },
        );
        Ok(())
    }

    // Loads a page into window `id`: HTML text for `load`, a bundled page or a file for `loadFile`.
    pub(super) fn load(
        &mut self,
        id: u64,
        action: &str,
        args: &serde_json::Value,
        mut cx: Option<&mut App>,
    ) -> Result<()> {
        if let Some((files, scripts)) = RELOADED
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            self.files = files;
            self.host
                .borrow_mut()
                .update_app(|app| app.scripts = scripts);
        }
        let value = args[2].as_str().context("Expected HTML text or path")?;
        let page = args[3] == "page";
        let request = args[3].as_u64();
        let root = self.host.borrow().app.path.clone();
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
                    return crate::bundle::unpack_page(bytes).map_err(|_| (-2, "ERR_FAILED"));
                }
                return match self.files.get(name) {
                    Some(source) => Ok(crate::bundle::page(source)),
                    None => Err(missing),
                };
            }
            let path = file.as_deref().unwrap();
            match std::fs::metadata(path) {
                Ok(metadata) if metadata.is_file() && metadata.len() <= 64 << 20 => {
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
                if self.host.borrow().app.rust_main && !page {
                    bail!("Cannot load {value}: {description}");
                }
                if page {
                    eprintln!("apps: navigation: {description} ({code}) loading '{url}'");
                }
                let event = serde_json::json!({"request":request,"url":url,"code":code,"description":description});
                self.link.event("failed", id, Some(event), "apps:failed");
                return Ok(());
            }
        };
        renderer::warm(crate::style::font_lists(&doc));
        // Only bundled pages can run Node preloads. Never enable them for user files or HTML
        // strings.
        let trusted = location.starts_with("apps:///")
            || !page
                && file.as_ref().is_some_and(|path| {
                    std::fs::canonicalize(path).is_ok_and(|path| {
                        std::fs::canonicalize(&root).is_ok_and(|root| path.starts_with(root))
                    })
                });
        let bundled = self.host.borrow().app.scripts.clone();
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
                    None | Some("" | "module" | "text/javascript" | "application/javascript") => {
                        "module"
                    }
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
            host.load_page(id, doc);
            // Preload scripts see the page URL, like page scripts.
            host.store.borrow_mut().state(id).base = url::Url::parse(&location).ok();
        }
        let view = self.views.get_mut(&id).context("Window is closed")?;
        view.focused = true;
        if let (Some(handle), Some(cx)) = (view.handle, cx.as_deref_mut()) {
            cx.update_window(handle, |window, _| {
                let host = self.host.borrow();
                host.store.borrow_mut().state(id).attach_window(window);
                // Like Playwright, pages count as focused while the devtools MCP server runs, so tests in hidden windows get focus events.
                view.focused = window.is_active() || crate::native::devtools::enabled();
            });
        }
        view.preload = None;
        view.renderer = None;
        let preload = (trusted && scripts.iter().any(|(kind, _)| *kind == "preload"))
            .then(|| JsContext::new(self.api.clone(), self.host.clone(), Role::Preload(id)))
            .transpose()?
            .map(Rc::new);
        let renderer = Rc::new(JsContext::new(
            self.api.clone(),
            self.host.clone(),
            Role::Renderer(id),
        )?);
        if cx.is_none() {
            renderer.eval(
                    "addEventListener('error', event => queueMicrotask(() => { if (!event.defaultPrevented) __appsTrackModule(Promise.reject(event.error)); }))",
                    "apps:check",
                )?;
        }
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
            renderer.eval("__appsDispatch('window:blur',[],{quiet:true})", "apps:blur")?;
        }
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
        use crate::host::context::NEXT;
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
            let ready =
                || preload.eval("dispatchEvent(new Event('DOMContentLoaded'))", "apps:ready");
            if let Err(error) = scripted(cx.as_deref_mut(), ready) {
                eprintln!("apps: preload script: {error:#}");
            }
        }
        view.preload = preload;
        view.renderer = Some(renderer);
        view.title = None;
        if let (Some(handle), Some(cx)) = (view.handle, cx) {
            cx.refresh_window(handle);
        }
        let event = serde_json::json!({"request":request,"url":location});
        self.link.event("loaded", id, Some(event), "apps:loaded");
        Ok(())
    }
}
