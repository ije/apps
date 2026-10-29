mod background;
pub mod canvas;
pub(crate) mod effects;
mod element;
pub(crate) mod grid;
pub mod image;
mod input;
mod paint;
pub(crate) mod text;

use crate::{desktop::Desktop, host::Host, style};
use anyhow::{Result, ensure};
use element::{Build, element};
use gpui::{
    Context, ElementInputHandler, FocusHandle, KeyDownEvent, KeyUpEvent, MouseButton,
    MouseMoveEvent, MouseUpEvent, Window, div, prelude::*, rgba,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

pub(crate) struct WebView {
    pub(crate) id: u64,
    pub(crate) runtime: Rc<RefCell<Desktop>>,
    pub(crate) host: Rc<RefCell<Host>>,
    pub(crate) images: HashMap<usize, Arc<gpui::RenderImage>>,
    pub(crate) focus: FocusHandle,
    pub(crate) browsers: HashMap<usize, (crate::desktop::browser::Browser, String)>,
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
                match crate::desktop::browser::Browser::new(
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
            .and_then(|s| style::color(s))
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
