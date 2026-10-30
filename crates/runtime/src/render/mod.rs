mod background;
pub mod canvas;
pub(crate) mod cull;
pub(crate) mod effects;
mod element;
mod fonts;
pub(crate) mod grid;
pub mod image;
mod input;
mod paint;
mod scrollbar;
pub(crate) mod text;

use crate::style::Prop;
use crate::{desktop::Desktop, host::Host, style};
use element::{Build, element};
use render::{
    App, MouseButton, MouseExitEvent, MouseMoveEvent, MouseUpEvent, Window, div, prelude::*, rgba,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

/// The page of one window, shared with the elements that paint it.
pub(crate) struct ViewData {
    pub(crate) id: u64,
    pub(crate) runtime: Rc<RefCell<Desktop>>,
    pub(crate) host: Rc<RefCell<Host>>,
    pub(crate) images: RefCell<HashMap<usize, Arc<render::RenderImage>>>,
    pub(crate) browsers: RefCell<HashMap<usize, (crate::desktop::browser::Browser, String)>>,
    /// A drag area was pressed, so the next drag moves the window.
    pub(crate) moving: Cell<bool>,
    /// The files dragged over the window.
    pub(crate) dropped: RefCell<Vec<PathBuf>>,
}

/// The view of a window.
pub(crate) struct WebView(pub(crate) Rc<ViewData>);

impl std::ops::Deref for WebView {
    type Target = ViewData;
    fn deref(&self) -> &ViewData {
        &self.0
    }
}

impl WebView {
    pub(crate) fn new(id: u64, runtime: Rc<RefCell<Desktop>>, host: Rc<RefCell<Host>>) -> Self {
        Self(Rc::new(ViewData {
            id,
            runtime,
            host,
            images: RefCell::default(),
            browsers: RefCell::default(),
            moving: Cell::new(false),
            dropped: RefCell::default(),
        }))
    }

    fn render_page(&mut self, window: &mut Window, cx: &mut App) -> render::AnyElement {
        let started = std::time::Instant::now();
        let runtime = self.runtime.clone();
        let id = self.id;
        let frame_requested = {
            let mut host = self.host.borrow_mut();
            let state = host.dom.entry(id).or_default();
            state.text_system = Some(window.text_system().clone());
            state.pixel_ratio = window.scale_factor();
            state.sync_scroll();
            // WARNING: a redraw replaces a frame that already ran the animation frame callbacks.
            !state.container_pass
                && !window.redrawing()
                && std::mem::take(&mut state.frame_requested)
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
            window.on_next_frame(|window, _| window.refresh());
        }
        let pointer_events = render::canvas(|_, _| (), move |_, _, window| {
        let exit_runtime = runtime.clone();
        window.on_mouse_event(move |event: &MouseExitEvent, phase, _, cx| {
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
        let mut host = self.host.borrow_mut();
        let size = window.viewport_size();
        let width = f32::from(size.width);
        let height = f32::from(size.height);
        let resized = host.viewports.insert(self.id, [width, height]) != Some([width, height]);
        // WARNING: the desktop is borrowed while it opens a window, so `Desktop::drive` sends the resize events.
        if resized && !host.resized.contains(&self.id) {
            host.resized.push(self.id);
            let runtime = self.runtime.clone();
            cx.defer(move |cx| Desktop::drive(runtime, cx));
        }
        let mut images = HashMap::new();
        if let (Some(gpu), Some(page)) = {
            let host = &mut *host;
            (host.graphics.get_mut(&self.id), host.pages.get(&self.id))
        } {
            for (id, _) in page
                .doc
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.tag == "canvas")
            {
                match gpu.image(id) {
                    Ok(Some(image)) => {
                        images.insert(id, image);
                    }
                    Err(error) => eprintln!("apps: canvas presentation failed: {error}"),
                    _ => {}
                }
            }
        }
        let old = std::mem::replace(&mut *self.images.borrow_mut(), images);
        for (id, old) in old {
            if self
                .images
                .borrow()
                .get(&id)
                .is_none_or(|image| image.id != old.id)
            {
                window.drop_image(old);
            }
        }
        if let Some((browser, _)) = self.browsers.borrow().get(&usize::MAX) {
            let _ = browser.set_bounds([0., 0., width as f64, height as f64]);
        }
        let inspectable = !host.packaged;
        let Some(page) = host.pages.get_mut(&self.id) else {
            return div().size_full().into_any_element();
        };
        page.dirty = false;
        let mut doc = std::mem::take(&mut page.doc);
        let state = &mut *host;
        let dom = state.dom.entry(self.id).or_default();
        for image in dom.retired.drain(..) {
            window.drop_image(image);
        }
        let cached = dom.computed.clone();
        let mut styles =
            crate::dom::styles(&mut doc, dom, [width, height], Some(window.text_system()));
        if cached.is_none_or(|cached| !std::rc::Rc::ptr_eq(&cached, &styles))
            && fonts::load(&doc, dom, &state.assets)
        {
            styles = crate::dom::styles(&mut doc, dom, [width, height], Some(window.text_system()));
        }
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
        let mut visible = vec![false; doc.nodes.len()];
        let mut pending = vec![0];
        while let Some(node) = pending.pop() {
            let style = &styles.values[node];
            if !match dom.layout_nodes.get(node).and_then(Option::as_ref) {
                Some(old) if Rc::ptr_eq(&old.style, style) => old.flags & cull::HIDDEN != 0,
                _ => style
                    .get(Prop::DISPLAY)
                    .is_some_and(|value| value == "none"),
            } {
                visible[node] = true;
                pending.extend(&doc.nodes[node].children);
            }
        }
        let mut events = Vec::new();
        let now = std::time::Instant::now();
        let (animated, next) = dom.motion.update(
            &doc,
            &styles,
            &visible,
            now,
            dom.container_pass,
            &mut events,
        );
        match next {
            Some(at) if at <= now + std::time::Duration::from_millis(8) => {
                dom.motion.wake = None;
                window.on_next_frame(|window, _| window.refresh());
            }
            Some(at) => {
                if dom
                    .motion
                    .wake
                    .as_ref()
                    .is_none_or(|(old, _)| *old <= now || at < *old)
                {
                    let delay = at.saturating_duration_since(now);
                    let window_id = window.id();
                    let task = cx.timer(delay, move |cx| cx.refresh_window(window_id));
                    dom.motion.wake = Some((at, task));
                }
            }
            None => dom.motion.wake = None,
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
        retire(dom, &visible, &styles, |image| window.drop_image(image));
        let frame_nodes: Vec<_> = doc
            .nodes
            .iter()
            .enumerate()
            .filter(|&(id, node)| node.tag == "iframe" && visible[id])
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
        let mut browsers = self.browsers.borrow_mut();
        browsers.retain(|id, _| {
            *id == usize::MAX || frame_nodes.iter().any(|(node, _, _, _)| node == id)
        });
        for (node, html, url, sandbox) in frame_nodes {
            let source = html.clone().unwrap_or_else(|| url.clone());
            if let std::collections::hash_map::Entry::Vacant(entry) = browsers.entry(node) {
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
            let Some((browser, loaded)) = browsers.get_mut(&node) else {
                continue;
            };
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
        drop(browsers);
        let background = doc
            .nodes
            .iter()
            .position(|n| n.tag == "body")
            .and_then(|id| {
                styles.values[id]
                    .get(Prop::BACKGROUND_COLOR)
                    .or_else(|| styles.values[id].get(Prop::BACKGROUND))
            })
            .and_then(style::color)
            .or_else(|| state.backgrounds.get(&self.id).copied())
            .unwrap_or((0xffffff, 255));
        dom.layout_nodes.resize_with(doc.nodes.len(), || None);
        let frame = [width, height, window.scale_factor(), dom.font_loads as f32];
        let mut changed = Vec::new();
        let mut info = std::mem::take(&mut dom.cull.info);
        // WARNING: scroll-only frames change no node, style or focus, so last frame's flags hold
        // without its per-frame change bits.
        let seen = (doc.nodes.latest(), dom.focus);
        let unchanged = paint.is_empty()
            && dom
                .cull
                .seen
                .as_ref()
                .is_some_and(|(latest, focus, computed)| {
                    (*latest, *focus) == seen && Rc::ptr_eq(computed, &styles)
                });
        dom.cull.seen = paint.is_empty().then(|| (seen.0, seen.1, styles.clone()));
        if unchanged {
            for info in &mut info {
                info.flags &= !(cull::CHANGED | cull::RELISTED | cull::STALE);
            }
        } else {
            let mut order = Vec::new();
            let mut pending = vec![0];
            while let Some(id) = pending.pop() {
                order.push(id);
                pending.extend(&doc.nodes[id].children);
            }
            // WARNING: a style with the same values can resolve to other sizes, like `rem` after the root font changed.
            let units = |lengths: &style::Lengths| {
                let [font, root] = [lengths.font, lengths.root];
                let mut hasher = rustc_hash::FxHasher::default();
                std::hash::Hash::hash(
                    &[
                        font.em,
                        font.ch,
                        font.ex,
                        font.cap,
                        font.ic,
                        font.lh,
                        root.em,
                        root.ch,
                        root.ex,
                        root.cap,
                        root.ic,
                        root.lh,
                        lengths.viewport[0],
                        lengths.viewport[1],
                    ]
                    .map(f32::to_bits),
                    &mut hasher,
                );
                std::hash::Hasher::finish(&hasher)
            };
            info.clear();
            info.resize(doc.nodes.len(), cull::Info::default());
            // WARNING: lengths are cached with their style, so the style and the tag kind decide the flags within a frame.
            let mut memo = rustc_hash::FxHashMap::default();
            let mut own_flags = |tag: &str, style: &Rc<style::Style>, id: usize| {
                let kind = if tag.starts_with('#') {
                    1
                } else {
                    u8::from(matches!(
                        tag,
                        "iframe" | "webview" | "canvas" | "video" | "embed" | "object"
                    )) * 2
                };
                memo.entry((Rc::as_ptr(style), kind))
                    .or_insert_with(|| {
                        (
                            style.clone(),
                            cull::flags(tag, style, &styles.lengths[id], height),
                        )
                    })
                    .1
            };
            for (id, style) in &paint {
                info[*id].flags =
                    own_flags(&doc.nodes[*id].tag, style, *id) & (cull::MOVED | cull::BLOCKED);
            }
            for id in order.into_iter().rev() {
                let node = &doc.nodes[id];
                let style = &styles.values[id];
                let revision = doc.nodes.revision(id);
                let old = dom.layout_nodes[id].as_ref();
                let same = old.is_some_and(|old| old.seen == revision);
                let edited = old.is_some_and(|old| {
                    !same && (old.node.text != node.text || old.node.attrs != node.attrs)
                });
                debug_assert!(
                    !same
                        || old.is_some_and(|old| old.node.tag == node.tag
                            && old.node.text == node.text
                            && old.node.attrs == node.attrs),
                    "node {id} changed without a new revision"
                );
                let restyled = old.is_none_or(|old| {
                    !Rc::ptr_eq(&old.style, style) || !same && old.node.tag != node.tag
                });
                let own = restyled || edited;
                let reshaped = old.is_none_or(|old| {
                    !same
                        && (old.node.tag != node.tag
                            || old.node.text != node.text
                            || old.node.attrs != node.attrs
                                && !cull::cosmetic_attrs(&old.node.attrs, &node.attrs))
                        || !Rc::ptr_eq(&old.style, style)
                            && (old.units != units(&styles.lengths[id])
                                || !cull::cosmetic(&old.style, style))
                });
                let flags = match old {
                    Some(old) if !restyled => old.flags,
                    _ => own_flags(&node.tag, style, id),
                };
                let flowed = || {
                    node.children
                        .iter()
                        .copied()
                        .filter(|&child| info[child].flags & cull::OUT == 0)
                };
                let children = || {
                    flowed().map(|child| {
                        (
                            child,
                            dom.layout_nodes[child]
                                .as_ref()
                                .map_or(0, |node| node.revision),
                        )
                    })
                };
                let mut relisted = false;
                if own || old.is_some_and(|old| !old.children.iter().copied().eq(children())) {
                    relisted = old.is_none_or(|old| {
                        !old.children.iter().map(|&(child, _)| child).eq(flowed())
                    });
                    // WARNING: the geometry revision ignores paint-only changes, so culled spacers survive them.
                    let geometry = old
                        .filter(|old| {
                            !reshaped
                                && !relisted
                                && flowed().all(|child| info[child].revision <= old.geometry)
                        })
                        .map(|old| old.geometry);
                    let children = children().collect();
                    dom.layout_revision += 1;
                    dom.layout_nodes[id] = Some(crate::dom::LayoutNode {
                        node: node.clone(),
                        seen: revision,
                        style: style.clone(),
                        units: units(&styles.lengths[id]),
                        children,
                        revision: dom.layout_revision,
                        geometry: geometry.unwrap_or(dom.layout_revision),
                        flags,
                    });
                    changed.push(id);
                } else if let Some(old) = &mut dom.layout_nodes[id] {
                    old.seen = revision;
                }
                info[id] = cull::Info {
                    revision: dom.layout_nodes[id]
                        .as_ref()
                        .map_or(0, |node| node.geometry),
                    flags: if flags & cull::HIDDEN != 0 {
                        flags
                    } else {
                        node.children.iter().fold(
                            info[id].flags
                                | flags
                                | (u16::from(reshaped) * cull::CHANGED)
                                | (u16::from(relisted) * cull::RELISTED)
                                | (u16::from(dom.focus == Some(id)) * cull::BLOCKED),
                            |flags, &child| flags | info[child].flags & cull::BLOCKED,
                        )
                    },
                };
            }
        }
        if !paint.is_empty() {
            let values = &mut Rc::make_mut(&mut styles).values;
            for (id, style) in paint {
                restore.push((id, std::mem::replace(&mut values[id], style)));
            }
        }
        // WARNING: fonts, the viewport and container sizes change measurements without moving any box's geometry revision.
        if dom.container_pass || dom.cull.frame != frame {
            dom.measurements.clear();
            dom.grids.clear();
        }
        let mut stale = false;
        for &id in &changed {
            dom.inline_text.remove(&id);
            if dom.layout_nodes[id]
                .as_ref()
                .is_some_and(|node| node.geometry == node.revision)
            {
                dom.grids.remove(&id);
                info[id].flags |= cull::STALE;
                stale = true;
            }
        }
        if stale {
            dom.measurements.retain(|&(id, ..), _| {
                info.get(id)
                    .is_none_or(|info| info.flags & cull::STALE == 0)
            });
        }
        dom.text_layouts.clear();
        let mut warnings = Vec::new();
        if inspectable && !dom.linted {
            dom.linted = true;
            for &id in &changed {
                let (node, style) = (&doc.nodes[id], &styles.values[id]);
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
                    for (key, value) in [
                        Prop::LINE_HEIGHT,
                        Prop::VERTICAL_ALIGN,
                        Prop::TRANSFORM_STYLE,
                        Prop::CLIP_PATH,
                        Prop::FILTER,
                    ]
                    .into_iter()
                    .filter_map(|key| Some((key, values.get(key)?)))
                    {
                        if !pseudo.is_empty() && style.get(key) == Some(value) {
                            continue;
                        }
                        let reason = match key {
                            Prop::LINE_HEIGHT | Prop::VERTICAL_ALIGN if !pseudo.is_empty() => {
                                "line height and baseline can't change inside a line of text"
                            }
                            Prop::TRANSFORM_STYLE if value == "preserve-3d" => {
                                "3D children are flattened into the parent's plane"
                            }
                            Prop::CLIP_PATH if value.contains("shape(") => {
                                "shape() is not supported"
                            }
                            Prop::FILTER if value.contains("url(") && !value.contains("url(#") => {
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
                                "Unsupported CSS `{}: {value}` on <{name}>{pseudo}: {reason}.",
                                key.name()
                            ));
                        }
                    }
                }
            }
        }
        // WARNING: the page scroll is kept under the document node, which never renders as an element.
        let page_scroll = dom.scroll.entry(0).or_default().clone();
        // The page scrolls unless the root, or the body when the root's overflow is visible, hides it.
        let html = doc.nodes[0]
            .children
            .iter()
            .copied()
            .find(|&id| doc.nodes[id].tag == "html");
        let body = html.and_then(|html| {
            doc.nodes[html]
                .children
                .iter()
                .copied()
                .find(|&id| doc.nodes[id].tag == "body")
        });
        let source = html
            .filter(|&id| {
                [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                    .into_iter()
                    .any(|name| {
                        styles.values[id]
                            .get(name)
                            .is_some_and(|value| value != "visible")
                    })
            })
            .or(body);
        let overflow = source
            .and_then(|id| styles.values[id].get(Prop::OVERFLOW_Y))
            .unwrap_or("visible");
        let page_scrolls = !matches!(overflow, "hidden" | "clip");
        // Like Chromium, the page's bar takes the body's `::-webkit-scrollbar` styles, then the root's.
        let page_bar = html.filter(|_| page_scrolls).and_then(|html| {
            let parts = body
                .and_then(|body| styles.scrollbars.get(&body))
                .filter(|parts| parts[0].is_some())
                .or_else(|| styles.scrollbars.get(&html));
            let mut look = scrollbar::Look::new(
                &styles.values[html],
                parts.map(|parts| &**parts),
                &styles.lengths[html],
                (background.1 >= 128).then_some(background.0),
            )?;
            look.forced = [false, overflow == "scroll" || look.forced[1]];
            Some(look)
        });
        let page_space = page_bar.as_ref().map_or([0.; 2], |look| {
            look.space(look.shown([false, true], page_scroll.max_offset()))
        });
        if !dom.container_pass {
            let mut moved = 0.;
            for (&id, handle) in &dom.scroll {
                let offset = handle.offset();
                let last = dom.cull.offsets.insert(id, offset).unwrap_or(offset);
                moved += f32::from((offset.x - last.x).abs() + (offset.y - last.y).abs());
            }
            // WARNING: a scroll farther than the overscan could bring culled boxes into view.
            dom.cull.enabled = !std::mem::take(&mut dom.cull.off)
                && dom.cull.frame == frame
                && moved < height / 2.;
            dom.cull.frame = frame;
        }
        let zone = dom.cull.enabled.then(|| {
            Rc::new(cull::Zone {
                top: 0.,
                bottom: height,
                clips: Vec::new(),
            })
        });
        dom.containers.clear();
        dom.flexed_grids.clear();
        let measured = RefCell::new(std::mem::take(&mut dom.measurements));
        let inline_text = RefCell::new(std::mem::take(&mut dom.inline_text));
        for text in warnings {
            if host.logs.len() == 500 {
                host.logs.pop_front();
            }
            host.logs.push_back(serde_json::json!({"method":"Runtime.consoleAPICalled","params":{"type":"error","text":text,"context":crate::host::Role::Renderer(self.id)}}));
        }
        drop(host);
        let text_system = window.text_system().clone();
        let root = element(
            &Build {
                doc: &doc,
                styles: &styles,
                text: &text_system,
                pixel_ratio: window.scale_factor(),
                view: &self.0,
                layers: Rc::default(),
                ancestors: Rc::default(),
                stacking: 0,
                flex_parent: false,
                measured: &measured,
                inline_text: &inline_text,
                perspective: None,
                siblings: &RefCell::default(),
                ids: &Rc::default(),
                info: &info,
                zone,
            },
            0,
            [width - page_space[1], height],
            [width - page_space[1], height],
            None,
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
            dom.cull.info = info;
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
                return self.render_page(window, cx);
            }
        }
        let runtime = self.runtime.clone();
        let id = self.id;
        if !window.redrawing() {
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
        }
        div()
            .id(("apps-page", 0))
            .size_full()
            .when(page_scrolls, |page| page.overflow_y_scroll())
            .when(!page_scrolls, |page| page.overflow_y_hidden())
            .pr(render::px(page_space[1]))
            .restrict_scroll_to_axis()
            .track_scroll(&page_scroll)
            .bg(rgba((background.0 << 8) | background.1 as u32))
            .on_mouse_move({
                let view = self.0.clone();
                move |event: &MouseMoveEvent, window, _| {
                    if view.moving.take() && event.dragging() {
                        window.start_window_move();
                    }
                }
            })
            .child(pointer_events)
            .child(root)
            .children(page_bar.map(|look| scrollbar::Scrollbar {
                id: 0,
                handle: page_scroll.clone(),
                look: Some(look),
                axes: [false, true],
                reserved: page_space,
                borders: [0.; 4],
                origin: [0.; 2],
            }))
            .children(crate::desktop::mcp::profiling().then(|| {
                render::canvas(
                    |_, _| (),
                    move |_, _, _| crate::desktop::mcp::frame(id, started),
                )
                .absolute()
            }))
            .into_any_element()
    }
}

/// Drops the cached images of nodes that are not displayed. Canvases keep their pixels.
fn retire(
    dom: &mut crate::dom::State,
    visible: &[bool],
    styles: &style::ComputedStyles,
    mut release: impl FnMut(Arc<render::RenderImage>),
) {
    dom.svgs.retain(|id, (_, image)| {
        visible.get(*id) == Some(&true) || {
            release(image.clone());
            false
        }
    });
    dom.masks.retain(|id, (_, image)| {
        visible.get(*id) == Some(&true) || {
            release(image.clone());
            false
        }
    });
    dom.image_crops.retain(|id, (_, image)| {
        visible.get(*id) == Some(&true) || {
            release(image.clone());
            false
        }
    });
    for (id, canvas) in &dom.canvases {
        if visible.get(*id) != Some(&true) {
            release(canvas.image.clone());
        }
    }
    // WARNING: `url()` layers share one decoded image per file, kept under `usize::MAX` while a layer uses it.
    dom.backgrounds.retain(|&(id, layer), (_, image)| {
        let keep = if id == usize::MAX {
            Arc::strong_count(image) > 1
        } else {
            visible.get(id) == Some(&true)
                && match layer / 64 {
                    0 => styles.values.get(id).map(|style| &**style),
                    slot => styles
                        .pseudos
                        .get(&id)
                        .and_then(|pseudos| pseudos.get(slot - 1)),
                }
                .and_then(|style| style.get(Prop::BACKGROUND_IMAGE))
                .is_some_and(|value| value != "none")
        };
        if !keep && Arc::strong_count(image) == 1 {
            release(image.clone());
        }
        keep
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_nodes_release_textures_but_canvases_keep_pixels() {
        let mut doc = crate::dom::Document::parse(
            "<style>p{background-image:linear-gradient(red,blue)}</style><canvas></canvas><p></p><p></p>",
        );
        let mut dom = crate::dom::State::default();
        let canvas = doc
            .nodes
            .iter()
            .position(|node| node.tag == "canvas")
            .unwrap();
        let paragraphs: Vec<usize> = (0..doc.nodes.len())
            .filter(|&id| doc.nodes[id].tag == "p")
            .collect();
        let [first, second] = [paragraphs[0], paragraphs[1]];
        let styles = crate::dom::styles(&mut doc, &mut dom, [800., 600.], None);
        let mut drawn = canvas::Canvas::new(2, 2).unwrap();
        drawn
            .draw(&[serde_json::json!({"op":"fillRect","x":0,"y":0,"width":2,"height":2,"fillStyle":"#ff0000"})])
            .unwrap();
        dom.canvases.insert(canvas, drawn);
        let image = |size: u32| {
            Arc::new(render::RenderImage::new(
                size,
                size,
                vec![0; (size * size * 4) as usize],
            ))
        };
        let shared = image(1);
        dom.backgrounds
            .insert((first, 0), ("a".into(), shared.clone()));
        dom.backgrounds
            .insert((second, 0), ("a".into(), shared.clone()));
        dom.backgrounds
            .insert((usize::MAX, 7), ("a".into(), shared));
        dom.backgrounds.insert((second, 64), ("b".into(), image(2)));
        let mut released = Vec::new();
        let mut visible = vec![false; doc.nodes.len()];
        (visible[0], visible[second]) = (true, true);
        retire(&mut dom, &visible, &styles, |image| released.push(image.id));
        assert_eq!(dom.canvases[&canvas].pixels.pixels()[0].red(), 255);
        assert_eq!(released.len(), 2);
        assert!(released.contains(&dom.canvases[&canvas].image.id));
        assert!(dom.backgrounds.contains_key(&(second, 0)));
        assert!(!dom.backgrounds.contains_key(&(first, 0)));
        assert!(!dom.backgrounds.contains_key(&(second, 64)));
        released.clear();
        for _ in 0..2 {
            retire(&mut dom, &[], &styles, |image| released.push(image.id));
        }
        assert!(dom.backgrounds.is_empty());
        assert!(dom.canvases.contains_key(&canvas));
    }
}
