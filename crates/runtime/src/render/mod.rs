mod background;
pub mod canvas;
mod control;
pub(crate) mod cull;
pub(crate) mod effects;
mod element;
pub(crate) mod fonts;
pub(crate) mod grid;
mod hud;
pub mod image;
mod input;
mod paint;
pub(crate) mod scrollbar;
pub(crate) mod text;

use crate::style::Prop;
use crate::{desktop::Desktop, host::Host, style};
pub(crate) use element::dimension;
use element::{Build, element};
use renderer::{
    App, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent,
    ScrollWheelEvent, Window, WindowId, rgba,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

/// The page of one window, shared with the elements that paint it.
pub(crate) struct PageState {
    pub(crate) id: u64,
    pub(crate) runtime: Rc<RefCell<Desktop>>,
    pub(crate) host: Rc<RefCell<Host>>,
    pub(crate) images: RefCell<HashMap<usize, Arc<renderer::RenderImage>>>,
    pub(crate) browsers: RefCell<HashMap<usize, (crate::desktop::browser::Browser, String)>>,
    /// By browser, the bounds it was given and the node revision its source was checked at.
    pub(crate) frames: RefCell<HashMap<usize, ([f64; 4], u64, bool)>>,
    /// The `canvas` and `iframe` nodes, found again when the document changes.
    tagged: RefCell<(u64, Vec<usize>, Vec<usize>)>,
    /// The `id` index of the document, kept while the document is unchanged.
    ids: RefCell<(u64, Rc<effects::Ids>)>,
    /// By inline SVG: its style, size and content hash, its source, and its parsed boxes.
    svgs: RefCell<HashMap<usize, element::Svg>>,
    /// By form field, the selection while focused and the text length it drew last, to scroll a moved caret into view.
    fields: RefCell<HashMap<usize, input::Caret>>,
    /// Background image files that workers decode, with the node that wants them and the bitmap once it is done.
    decoding: RefCell<HashMap<String, (usize, background::Decoding)>>,
    /// A drag area was pressed, so the next drag moves the window.
    pub(crate) moving: Cell<bool>,
    /// The files dragged over the window.
    pub(crate) dropped: RefCell<Vec<PathBuf>>,
    /// The pressed mouse buttons, as DOM `buttons` bits.
    buttons: Cell<u8>,
    /// The screen position of the content area, until the window moves.
    pub(crate) origin: Cell<Option<renderer::Point<renderer::Pixels>>>,
    window: WindowId,
    /// A script read geometry, so this build only lays the page out.
    reflowing: Cell<bool>,
    /// Boxes whose animations wait until they come into view, with their clipping ancestors.
    parked: RefCell<Vec<(usize, Clips)>>,
    /// Animated boxes in view that painted no keyed layer, so their opacity can't be patched.
    flat: RefCell<rustc_hash::FxHashSet<usize>>,
    /// From when composite frames may promote their layers to the compositor, and whether they are.
    promote: Cell<Option<(std::time::Instant, bool)>>,
    /// By box whose keyed layer keeps a transform: the bounds and parent perspective its transform
    /// resolves with, and the transform drawn last.
    pub(super) spun: RefCell<rustc_hash::FxHashMap<usize, Spun>>,
    /// The caret drawn last: its node, offset and text length, when it moved, and its blink timer by phase.
    caret: RefCell<Option<input::Blink>>,
    /// The FPS HUD of `apps run`.
    hud: RefCell<Option<hud::Hud>>,
    /// By element, what its kept boxes were built from, and the frame they hold for: viewport size,
    /// scale, font loads and the top layer.
    kept: RefCell<rustc_hash::FxHashMap<usize, element::Reuse>>,
    kept_frame: RefCell<([f32; 4], Vec<usize>)>,
}

/// Clipping ancestors with their clipped axes, see [`clips`].
type Clips = Vec<(usize, [bool; 2])>;
type Spun = (
    renderer::Bounds<renderer::Pixels>,
    Option<(f32, [f32; 2])>,
    effects::Matrix,
);

thread_local! {
    /// The app and the window of the page scripts that run now.
    static SCRIPTS: Cell<Option<(*mut App, Option<*mut Window>)>> = const { Cell::new(None) };
    static VIEWS: RefCell<HashMap<u64, std::rc::Weak<PageState>>> = RefCell::default();
}

/// Runs page script `f`, which may lay out its page with `cx` and `window`, see [`reflow`].
// WARNING: `f` must not use `cx` or `window`: a reflow reaches them through pointers while it runs.
pub(crate) fn scripted<R>(cx: &mut App, window: Option<&mut Window>, f: impl FnOnce() -> R) -> R {
    crate::jsc::REFLOW.set(Some(reflow));
    crate::jsc::CLIPBOARD.set(Some(clipboard));
    let previous = SCRIPTS.replace(Some((cx, window.map(|window| window as *mut Window))));
    let result = f();
    SCRIPTS.set(previous);
    result
}

/// Writes `text` to the clipboard, or reads it without text, for the page scripts that run now.
fn clipboard(text: Option<String>) -> Option<String> {
    let (cx, _) = SCRIPTS.get()?;
    // WARNING: the pointer comes from `scripted`, whose caller does not touch it until the script returns.
    let cx = unsafe { &*cx };
    match text {
        Some(text) => {
            cx.write_to_clipboard(text);
            None
        }
        None => cx.read_from_clipboard(),
    }
}

/// Runs edit `command` of `webContents` in the page, like the keyboard: copy and cut write
/// what the page copied to the clipboard, and paste reads it. Without an app it skips the clipboard.
pub(crate) fn edit(
    renderer: &crate::jsc::JsContext,
    app: Option<(&mut App, Option<&mut Window>)>,
    command: &str,
    text: &str,
) {
    let run = |text: &str| {
        let [command, text] = [command, text].map(|value| serde_json::json!(value).to_string());
        renderer.call("__appsEdit", &[&command, &text])
    };
    let result = match app {
        Some((cx, window)) => {
            let pasted = matches!(command, "paste" | "pasteAndMatchStyle")
                .then(|| cx.read_from_clipboard().unwrap_or_default());
            let result = scripted(cx, window, || run(pasted.as_deref().unwrap_or(text)));
            if let Ok(copied) = &result
                && matches!(command, "copy" | "cut")
                && !copied.is_empty()
            {
                cx.write_to_clipboard(copied.clone());
            }
            result
        }
        None => run(text),
    };
    if let Err(error) = result {
        eprintln!("apps: {error}");
    }
}

/// Lays page `id` out now when it changed since its last layout, like a browser's forced reflow.
pub(crate) fn reflow(id: u64) {
    let Some((cx, window)) = SCRIPTS.get() else {
        return;
    };
    let Some(view) = VIEWS.with_borrow(|views| views.get(&id).and_then(std::rc::Weak::upgrade))
    else {
        return;
    };
    // WARNING: composite frames turn boxes without a layout, so a read after them lays the page out.
    if view.host.try_borrow_mut().map_or(true, |host| {
        !host.pages.get(&id).is_some_and(|page| page.dirty)
            && !host.dom.get(&id).is_some_and(|dom| dom.bounds.moved())
    }) {
        return;
    }
    let run = |window: &mut Window, cx: &mut App| {
        view.reflowing.set(true);
        window.layout(cx, |tree, window, cx| {
            PageView(view.clone()).render_page(tree, window, cx)
        });
        view.reflowing.set(false);
        window.refresh();
    };
    // WARNING: the pointers come from `scripted`, whose caller does not touch them until the script returns.
    let cx = unsafe { &mut *cx };
    match window.map(|window| unsafe { &mut *window }) {
        Some(window) if window.id() == view.window => run(window, cx),
        _ => {
            cx.update_window(view.window, run);
        }
    }
}

/// Runs the page's animation frame callbacks before the frame lays out, and again on the next
/// frame while the page asks for one.
pub(crate) fn animation_frame(view: Rc<PageState>, window: &mut Window, cx: &mut App) {
    let renderer = view.runtime.try_borrow().ok().and_then(|app| {
        app.views
            .get(&view.id)
            .and_then(|view| view.renderer.clone())
    });
    if let Some(renderer) = renderer
        && let Err(error) = scripted(cx, Some(window), || {
            renderer.eval("__appsAnimationFrame()", "apps:frame")
        })
    {
        eprintln!("apps: {error}");
    }
    window.refresh();
    if view
        .host
        .try_borrow_mut()
        .ok()
        .and_then(|mut host| {
            host.dom
                .get_mut(&view.id)
                .map(|dom| std::mem::take(&mut dom.frame_requested))
        })
        .unwrap_or(false)
    {
        let view = view.clone();
        window.on_next_frame(move |window, cx| animation_frame(view, window, cx));
    }
    let runtime = view.runtime.clone();
    cx.defer(move |cx| Desktop::drive(runtime, cx));
}

/// The DOM `button` of a mouse button.
fn button(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::Navigate(renderer::NavigationDirection::Back) => 3,
        MouseButton::Navigate(_) => 4,
    }
}

impl PageState {
    /// The `screenX` and `screenY` of window position `at`, in screen points.
    fn screen(&self, window: &Window, at: renderer::Point<renderer::Pixels>) -> [f32; 2] {
        let origin = self.origin.get().unwrap_or_else(|| {
            let origin = window.window_state().content.origin;
            self.origin.set(Some(origin));
            origin
        });
        let at = origin + at * window.zoom();
        [f32::from(at.x), f32::from(at.y)]
    }
}

pub(crate) struct PageView(pub(crate) Rc<PageState>);

impl std::ops::Deref for PageView {
    type Target = PageState;
    fn deref(&self) -> &PageState {
        &self.0
    }
}

impl PageView {
    pub(crate) fn new(
        id: u64,
        runtime: Rc<RefCell<Desktop>>,
        host: Rc<RefCell<Host>>,
        window: WindowId,
    ) -> Self {
        let view = Rc::new(PageState {
            id,
            runtime,
            host,
            images: RefCell::default(),
            browsers: RefCell::default(),
            frames: RefCell::default(),
            tagged: RefCell::default(),
            ids: RefCell::default(),
            svgs: RefCell::default(),
            fields: RefCell::default(),
            decoding: RefCell::default(),
            moving: Cell::new(false),
            dropped: RefCell::default(),
            buttons: Cell::new(0),
            origin: Cell::new(None),
            window,
            reflowing: Cell::new(false),
            parked: RefCell::default(),
            flat: RefCell::default(),
            spun: RefCell::default(),
            promote: Cell::new(None),
            caret: RefCell::default(),
            hud: RefCell::default(),
            kept: RefCell::default(),
            kept_frame: RefCell::default(),
        });
        VIEWS.with_borrow_mut(|views| {
            views.retain(|_, view| view.strong_count() > 0);
            views.insert(id, Rc::downgrade(&view));
        });
        Self(view)
    }

    fn render_page(
        &mut self,
        tree: &renderer::Tree,
        window: &mut Window,
        cx: &mut App,
    ) -> renderer::Child {
        let started = std::time::Instant::now();
        {
            let mut host = self.host.borrow_mut();
            let state = host.dom.entry(self.id).or_default();
            state.text_system = Some(window.text_system().clone());
            state.pixel_ratio = window.scale_factor();
            state.sync_scroll();
            state.bounds.touch();
        }
        let data = self.0.clone();
        // WARNING: presses go to the page in the bubble phase, after drag areas and scrollbars could stop them.
        let pointer_events = renderer::canvas(|_, _| (), move |_, _, window| {
            // WARNING: the page was laid out by now, so a parked box that came into view animates from the next frame.
            let parked = data.parked.take();
            let size = window.viewport_size();
            if !parked.is_empty()
                && data.host.try_borrow().is_ok_and(|host| {
                    host.dom.get(&data.id).is_some_and(|dom| {
                        parked.iter().any(|(id, clips)| {
                            in_view(&dom.bounds, [f32::from(size.width), f32::from(size.height)], *id, clips.iter().copied())
                        })
                    })
                })
            {
                window.request_animation_frame();
            }
            let view = data.clone();
            window.on_mouse_event(move |event: &MouseExitEvent, phase, window, cx| {
                if phase.capture() {
                    let [x, y] = view.screen(window, event.position);
                    Desktop::dispatch(view.runtime.clone(), view.id, "pointerexit", &[], serde_json::json!({"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y),"screenX":x,"screenY":y}), Some(window), cx);
                }
            });
            let view = data.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase.capture() {
                    if event.pressed_button.is_none() {
                        view.buttons.set(0);
                    }
                    let [x, y] = view.screen(window, event.position);
                    Desktop::dispatch(view.runtime.clone(), view.id, "pointermove", &[], serde_json::json!({"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y),"screenX":x,"screenY":y,"buttons":view.buttons.get(),"ctrlKey":event.modifiers.control,"metaKey":event.modifiers.platform,"altKey":event.modifiers.alt,"shiftKey":event.modifiers.shift}), Some(window), cx);
                }
            });
            let view = data.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase.bubble() {
                    let button = button(event.button);
                    view.buttons.set(view.buttons.get() | 1 << [0, 2, 1, 3, 4][usize::from(button)]);
                    let [x, y] = view.screen(window, event.position);
                    Desktop::dispatch(view.runtime.clone(), view.id, "pointerdown", &[], serde_json::json!({"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y),"screenX":x,"screenY":y,"button":button,"buttons":view.buttons.get(),"detail":event.click_count,"ctrlKey":event.modifiers.control,"metaKey":event.modifiers.platform,"altKey":event.modifiers.alt,"shiftKey":event.modifiers.shift}), Some(window), cx);
                }
            });
            let view = data.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if phase.capture() {
                    let button = button(event.button);
                    view.buttons.set(view.buttons.get() & !(1 << [0, 2, 1, 3, 4][usize::from(button)]));
                    let [x, y] = view.screen(window, event.position);
                    Desktop::dispatch(view.runtime.clone(), view.id, "pointerup", &[], serde_json::json!({"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y),"screenX":x,"screenY":y,"button":button,"buttons":view.buttons.get(),"ctrlKey":event.modifiers.control,"metaKey":event.modifiers.platform,"altKey":event.modifiers.alt,"shiftKey":event.modifiers.shift}), Some(window), cx);
                }
            });
            let view = data.clone();
            // WARNING: wheel events reach the page only once it listens to them, and a canceled one does not scroll.
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase.capture() && view.host.try_borrow().is_ok_and(|host| host.dom.get(&view.id).is_some_and(|dom| dom.wheel)) {
                    let delta = event.delta.pixel_delta(renderer::px(16.));
                    let [x, y] = view.screen(window, event.position);
                    if !Desktop::dispatch(view.runtime.clone(), view.id, "wheel", &[], serde_json::json!({"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y),"screenX":x,"screenY":y,"deltaX":-f32::from(delta.x),"deltaY":-f32::from(delta.y),"buttons":view.buttons.get(),"ctrlKey":event.modifiers.control,"metaKey":event.modifiers.platform,"altKey":event.modifiers.alt,"shiftKey":event.modifiers.shift}), Some(window), cx) {
                        cx.stop_propagation();
                    }
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
        let mut tagged = self.tagged.borrow_mut();
        if let Some(page) = host.pages.get(&self.id)
            && tagged.0 != page.doc.nodes.latest()
        {
            tagged.0 = page.doc.nodes.latest();
            tagged.1.clear();
            tagged.2.clear();
            for (id, node) in page.doc.nodes.iter().enumerate() {
                match node.tag.as_str() {
                    "canvas" => tagged.1.push(id),
                    "iframe" => tagged.2.push(id),
                    _ => {}
                }
            }
        }
        let mut images = HashMap::new();
        if let Some(gpu) = host.graphics.get_mut(&self.id) {
            for &id in &tagged.1 {
                match gpu.image(id) {
                    Ok(Some(image)) => {
                        images.insert(id, image);
                    }
                    Err(error) => eprintln!("apps: canvas presentation failed: {error}"),
                    _ => {}
                }
            }
        }
        drop(tagged);
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
            let zoom = window.zoom() as f64;
            let bounds = [0., 0., width as f64 * zoom, height as f64 * zoom];
            let mut frames = self.frames.borrow_mut();
            let set = &mut frames
                .entry(usize::MAX)
                .or_insert(([f64::NAN; 4], 0, true))
                .0;
            if *set != bounds {
                *set = bounds;
                let _ = browser.set_bounds(bounds);
            }
        }
        let inspectable = !host.packaged;
        let Some(page) = host.pages.get_mut(&self.id) else {
            return renderer::node().size_full().into();
        };
        page.dirty = false;
        let mut doc = std::mem::take(&mut page.doc);
        let state = &mut *host;
        let dom = state.dom.entry(self.id).or_default();
        for image in dom.retired.drain(..) {
            window.drop_image(image);
        }
        // WARNING: the atlas copied the pixels of a drawn frame, so the media library can reuse them for its next frames.
        for (node, image) in dom.recycled.drain(..) {
            window.drop_image(image.clone());
            if let Ok(image) = Arc::try_unwrap(image)
                && let Some(media) = state.media.get_mut(&self.id)
            {
                media.recycle(node as u64, image);
            }
        }
        dom.retired_canvases.clear();
        let cached = dom.computed.clone();
        let mut styles = crate::dom::styles(
            &mut doc,
            dom,
            [width, height],
            Some((window.text_system(), window.scale_factor())),
        );
        if cached.is_none_or(|cached| !std::rc::Rc::ptr_eq(&cached, &styles))
            && fonts::load(&doc, dom, &state.assets).0
        {
            styles = crate::dom::styles(
                &mut doc,
                dom,
                [width, height],
                Some((window.text_system(), window.scale_factor())),
            );
        }
        let children: Vec<_> = styles
            .children
            .iter()
            .map(|(&node, children)| (node, doc.nodes.compose_children(node, children.clone())))
            .collect();
        let seen = (doc.nodes.latest(), dom.focus);
        // WARNING: `cull.seen` holds the styles only between frames, so animated values swap in and out of them without a copy.
        let same = dom
            .cull
            .seen
            .take()
            .is_some_and(|(latest, focus, computed)| {
                (latest, focus) == seen && Rc::ptr_eq(&computed, &styles)
            });
        let mut visible = std::mem::take(&mut dom.cull.visible);
        if !same {
            let mut shown = std::mem::take(&mut visible);
            visible.clear();
            visible.resize(doc.nodes.len(), false);
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
            // Table rows and row groups are flattened out of the tree above; they show when their cells do.
            for node in 0..visible.len() {
                if visible[node] && matches!(doc.nodes[node].tag.as_str(), "td" | "th") {
                    let mut parent = doc.nodes[node].parent;
                    while let Some(id) = parent.filter(|&id| {
                        !visible[id]
                            && matches!(
                                doc.nodes[id].tag.as_str(),
                                "tr" | "thead" | "tbody" | "tfoot"
                            )
                    }) {
                        visible[id] = true;
                        parent = doc.nodes[id].parent;
                    }
                }
            }
            dom.bounds.hide(&visible, &shown);
            shown.clear();
        }
        let mut events = Vec::new();
        let now = std::time::Instant::now();
        let bounds = &dom.bounds;
        let (mut animated, next) = dom.motion.update(
            &doc,
            &styles,
            &visible,
            &|id| in_view(bounds, [width, height], id, clips(&doc, &styles, id)),
            now,
            dom.container_pass,
            &mut events,
        );
        *self.parked.borrow_mut() = dom
            .motion
            .parked
            .iter()
            .map(|&id| (id, clips(&doc, &styles, id).collect()))
            .collect();
        // WARNING: an animated custom property can change any value that uses it, so the frame cascades the animated values again.
        if !dom.motion.custom.is_empty() {
            dom.animated = std::mem::take(&mut dom.motion.custom);
            let base = dom.computed.take();
            let restyled = crate::dom::styles(
                &mut doc,
                dom,
                [width, height],
                Some((window.text_system(), window.scale_factor())),
            );
            dom.computed = base;
            animated = (0..styles.values.len())
                .filter(|&id| !Rc::ptr_eq(&restyled.values[id], &styles.values[id]))
                .map(|id| (id, restyled.values[id].clone(), true))
                .collect();
        }
        let mut flat = self.flat.borrow_mut();
        flat.retain(|id| animated.iter().any(|(animated, ..)| animated == id));
        let composited: Vec<(usize, bool)> = animated
            .iter()
            .filter(|(id, style, layout)| {
                !layout
                    && !flat.contains(id)
                    && composites(style, &styles.values[*id])
                    && [Prop::BACKDROP_FILTER, Prop::WEBKIT_BACKDROP_FILTER]
                        .into_iter()
                        .all(|prop| style.get(prop).is_none_or(|value| value == "none"))
                    && style.get(Prop::BACKFACE_VISIBILITY) != Some("hidden")
            })
            .map(|(id, style, _)| {
                let base = &styles.values[*id];
                let turns = TURNS.iter().any(|&prop| {
                    style.get(prop).is_some_and(|value| value != "none")
                        || style.get(prop) != base.get(prop)
                });
                (*id, turns)
            })
            .collect();
        drop(flat);
        let compositing = !animated.is_empty()
            && composited.len() == animated.len()
            && dom.motion.pseudos.is_empty();
        animate(&self.0, dom, next, now, compositing, events, window, cx);
        // WARNING: animated values replace `styles.values` and `styles.pseudos` only while this frame is built;
        // `restore` and `restore_pseudos` put the cascade back.
        let mut restore = Vec::new();
        let mut restore_pseudos = Vec::new();
        let pseudos = std::mem::take(&mut dom.motion.pseudos);
        if !pseudos.is_empty() {
            dom.computed = None;
            let map = &mut Rc::make_mut(&mut styles).pseudos;
            for (id, pair) in pseudos {
                if let Some(old) = map.insert(id, pair) {
                    restore_pseudos.push((id, old));
                }
            }
        }
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
        let relaid = !restore.is_empty();
        retire(dom, &visible, &styles, |image| window.drop_image(image));
        self.svgs
            .borrow_mut()
            .retain(|&id, _| visible.get(id) == Some(&true));
        self.fields
            .borrow_mut()
            .retain(|&id, _| visible.get(id) == Some(&true));
        self.decoding
            .borrow_mut()
            .retain(|_, (id, _)| visible.get(*id) == Some(&true));
        // WARNING: iframe bounds are set where they are laid out, see `paint.rs`.
        let tagged = self.tagged.borrow();
        let frame_nodes = || tagged.2.iter().copied().filter(|&id| visible[id]);
        let mut browsers = self.browsers.borrow_mut();
        let mut frames = self.frames.borrow_mut();
        browsers.retain(|id, _| *id == usize::MAX || frame_nodes().any(|node| node == *id));
        frames.retain(|id, _| browsers.contains_key(id));
        for node in frame_nodes() {
            let (attrs, revision) = (&doc.nodes[node].attrs, doc.nodes.revision(node));
            if let std::collections::hash_map::Entry::Vacant(entry) = browsers.entry(node) {
                match crate::desktop::browser::Browser::new(
                    window,
                    &serde_json::json!({"javascript":attrs.get("sandbox").is_none_or(|s|s.split_whitespace().any(|s|s=="allow-scripts")),"inspectable":inspectable}),
                ) {
                    Ok(browser) => {
                        entry.insert((browser, String::new()));
                        frames.insert(node, ([f64::NAN; 4], 0, true));
                    }
                    Err(error) => {
                        eprintln!("apps: iframe: {error}");
                        continue;
                    }
                }
            }
            let (Some((browser, loaded)), Some(frame)) =
                (browsers.get_mut(&node), frames.get_mut(&node))
            else {
                continue;
            };
            // WARNING: the native view draws above the page, so CSS can only hide it, when it or an ancestor has no opacity.
            let mut shown = styles.values[node]
                .get(Prop::VISIBILITY)
                .is_none_or(|value| value == "visible");
            let mut current = Some(node);
            while let Some(id) = current
                && shown
            {
                let style = paint
                    .iter()
                    .find(|(painted, _)| *painted == id)
                    .map_or(&styles.values[id], |(_, style)| style);
                shown = style
                    .get(Prop::OPACITY)
                    .and_then(|value| value.parse::<f32>().ok())
                    .is_none_or(|value| value > 0.);
                current = doc.nodes[id].parent;
            }
            if frame.2 != shown {
                frame.2 = shown;
                let _ = browser.command("visible", &serde_json::json!(shown));
            }
            if frame.1 == revision {
                continue;
            }
            frame.1 = revision;
            let html = attrs.get("srcdoc");
            let url = attrs.get("src").map_or("about:blank", String::as_str);
            let source = html.map_or(url, String::as_str);
            if source != loaded {
                let result = match html {
                    Some(html) => browser.load_html(html, "about:blank"),
                    None => browser.load_url(url),
                };
                if let Err(error) = result {
                    eprintln!("apps: iframe: {error}");
                }
                *loaded = source.to_owned();
            }
        }
        drop((browsers, frames, tagged));
        let root = doc.nodes[0]
            .children
            .iter()
            .copied()
            .find(|&id| doc.nodes[id].tag == "html");
        let canvas = |id: usize| {
            styles.values[id]
                .get(Prop::BACKGROUND_COLOR)
                .and_then(style::color)
                .filter(|color| color.1 > 0)
        };
        let dark = root
            .and_then(|id| styles.values[id].get(Prop::COLOR_SCHEME))
            .is_some_and(|scheme| {
                scheme.contains("dark")
                    && (!scheme.contains("light") || crate::desktop::native::theme_state() & 1 != 0)
            });
        let background = root
            .and_then(canvas)
            .or_else(|| {
                doc.nodes
                    .iter()
                    .position(|n| n.tag == "body")
                    .and_then(canvas)
            })
            // Like Chromium, a dark color scheme paints the canvas dark unless the window is transparent.
            .or_else(|| match state.backgrounds.get(&self.id).copied() {
                Some(color) if color.1 == 0 => Some(color),
                _ if dark => Some((0x121212, 255)),
                color => color,
            })
            .unwrap_or((0xffffff, 255));
        dom.layout_nodes.resize_with(doc.nodes.len(), || None);
        let frame = [width, height, window.scale_factor(), dom.font_loads as f32];
        let mut changed = Vec::new();
        let mut info = std::mem::take(&mut dom.cull.info);
        // WARNING: scroll-only and paint animation frames change no node, style or focus, so last
        // frame's flags hold without its per-frame change bits, while they have the animated flags.
        let unchanged = same
            && !relaid
            && paint.iter().all(|(id, style)| {
                let flags = cull::flags(&doc.nodes[*id].tag, style, &styles.lengths[*id], height)
                    & (cull::MOVED | cull::BLOCKED);
                info.get(*id)
                    .is_some_and(|info| info.flags & flags == flags)
            });
        if unchanged {
            for info in &mut info {
                info.flags &= !(cull::CHANGED
                    | cull::RELISTED
                    | cull::STALE
                    | cull::COMPOSITED
                    | cull::TRANSFORMED
                    | cull::ANIMATED);
            }
        } else {
            let mut order = Vec::new();
            let mut pending = vec![0];
            while let Some(id) = pending.pop() {
                order.push(id);
                pending.extend(&doc.nodes[id].children);
            }
            // WARNING: a style with the same values can resolve to other sizes, like `rem` after the root font changed.
            // A new viewport size is left to `cull::VIEWPORT`, as for kept styles, so a resize that restyles colors
            // keeps culling.
            let units = |lengths: &style::Lengths| {
                let [font, root] = [lengths.font, lengths.root];
                let mut hasher = rustc_hash::FxHasher::default();
                std::hash::Hash::hash(
                    &[
                        font.em, font.ch, font.ex, font.cap, font.ic, font.lh, root.em, root.ch,
                        root.ex, root.cap, root.ic, root.lh,
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
                // WARNING: a form control's value and checked state draw like its text.
                let form = |old: &crate::dom::Node| {
                    old.value != node.value
                        || (old.state ^ node.state) & (crate::dom::CHECKED | crate::dom::DIRTY) != 0
                };
                let edited = old.is_some_and(|old| {
                    !same
                        && (old.node.text != node.text
                            || old.node.attrs != node.attrs
                            || form(&old.node))
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
                            || form(&old.node)
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
                // WARNING: these boxes build from their siblings, native state or counters, which no revision
                // tracks, so the window never keeps them.
                let volatile = matches!(
                    node.tag.as_str(),
                    "br" | "img"
                        | "video"
                        | "audio"
                        | "canvas"
                        | "svg"
                        | "iframe"
                        | "webview"
                        | "embed"
                        | "object"
                        | "input"
                        | "textarea"
                        | "select"
                        | "button"
                        | "option"
                        | "optgroup"
                        | "fieldset"
                        | "legend"
                        | "details"
                        | "summary"
                        | "dialog"
                        | "li"
                        | "table"
                        | "caption"
                        | "tr"
                        | "td"
                        | "th"
                        | "thead"
                        | "tbody"
                        | "tfoot"
                        | "colgroup"
                        | "col"
                        | "q"
                        | "progress"
                        | "meter"
                        | "slot"
                ) || styles.pseudos.get(&id).is_some_and(|pair| {
                    pair.iter().any(|pseudo| {
                        pseudo.get(Prop::CONTENT).is_some_and(|value| {
                            value.contains("counter") || value.contains("quote")
                        })
                    })
                });
                info[id] = cull::Info {
                    revision: dom.layout_nodes[id]
                        .as_ref()
                        .map_or(0, |node| node.geometry),
                    subtree: dom.layout_nodes[id]
                        .as_ref()
                        .map_or(0, |node| node.revision),
                    flags: if flags & cull::HIDDEN != 0 {
                        flags
                    } else {
                        node.children.iter().fold(
                            info[id].flags
                                | flags
                                | (u16::from(reshaped) * cull::CHANGED)
                                | (u16::from(relisted) * cull::RELISTED)
                                | (u16::from(volatile) * cull::VOLATILE)
                                | (u16::from(dom.focus == Some(id)) * cull::BLOCKED),
                            |flags, &child| {
                                flags
                                    | info[child].flags
                                        & (cull::BLOCKED | cull::VIEWPORT | cull::VOLATILE)
                            },
                        )
                    },
                };
            }
        }
        self.spun.borrow_mut().clear();
        // A drawn frame takes promoted layers back, see `composite`.
        self.promote.set(
            (!composited.is_empty()).then(|| (now + std::time::Duration::from_millis(300), false)),
        );
        for &(id, turns) in &composited {
            info[id].flags |= cull::COMPOSITED | (u16::from(turns) * cull::TRANSFORMED);
        }
        // WARNING: animated values are swapped in after the revisions were taken, so boxes with them, and the boxes
        // around, are built again each frame.
        let animate = |info: &mut [cull::Info], id: usize| {
            let mut node = Some(id);
            while let Some(id) = node.filter(|&id| id < info.len()) {
                if info[id].flags & cull::ANIMATED != 0 {
                    break;
                }
                info[id].flags |= cull::ANIMATED;
                node = doc.nodes[id].parent;
            }
        };
        for &(id, _) in &composited {
            animate(&mut info, id);
        }
        for (id, _) in &paint {
            animate(&mut info, *id);
        }
        for &(id, _) in &restore_pseudos {
            animate(&mut info, id);
        }
        // WARNING: the inline text of a paragraph holds the colors of its text, so a new color drops it this
        // frame and the next, which draws the color without animation again.
        let mut recolored = std::mem::take(&mut dom.recolored);
        if !paint.is_empty() {
            let values = &mut Rc::make_mut(&mut styles).values;
            for (id, style) in paint {
                if style.get(Prop::COLOR) != values[id].get(Prop::COLOR) {
                    dom.recolored.push(id);
                }
                restore.push((id, std::mem::replace(&mut values[id], style)));
            }
        }
        recolored.extend(&dom.recolored);
        for &id in &recolored {
            animate(&mut info, id);
        }
        for id in recolored {
            let mut node = Some(id).filter(|&id| id < styles.values.len());
            while let Some(id) = node {
                dom.inline_text.remove(&id);
                node = doc.nodes[id].parent.filter(|_| {
                    styles.values[id]
                        .get(Prop::DISPLAY)
                        .is_none_or(|value| matches!(value, "inline" | "contents"))
                });
            }
        }
        // WARNING: fonts, the viewport and container sizes change measurements without moving any box's geometry revision.
        if dom.container_pass || dom.cull.frame != frame {
            dom.measurements.sizes.clear();
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
            dom.measurements.sizes.retain(|&(id, ..), _| {
                info.get(id)
                    .is_none_or(|info| info.flags & cull::STALE == 0)
            });
        }
        dom.text_layouts.clear();
        let mut warnings = Vec::new();
        if inspectable && !dom.linted {
            dom.linted = true;
            if dom.checked.len() > 65536 {
                dom.checked.clear();
            }
            for name in style::DROPPED.take() {
                if dom.warned.insert(format!("@{name}")) {
                    warnings.push(format!(
                        "Unsupported CSS at-rule `@{name}`: its rules are ignored."
                    ));
                }
            }
            for &id in &changed {
                let (node, style) = (&doc.nodes[id], &styles.values[id]);
                if node.tag.starts_with('#') {
                    continue;
                }
                let pseudos = styles.text_pseudos.get(&id);
                for (index, (pseudo, values)) in [
                    ("", Some(&**style)),
                    (
                        "::first-letter",
                        pseudos.and_then(|values| values[1].as_ref()),
                    ),
                    (
                        "::first-line",
                        pseudos.and_then(|values| values[2].as_ref()),
                    ),
                ]
                .into_iter()
                .enumerate()
                {
                    let Some(values) = values else { continue };
                    for (key, value) in values.own() {
                        // WARNING: each declaration is checked once, so restyles don't parse every value again.
                        if !pseudo.is_empty() && style.value(key) == Some(value)
                            || !dom.checked.insert((index as u8, key, value.clone()))
                        {
                            continue;
                        }
                        let (value, name) = (&**value, key.name());
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
                            // WARNING: these change nothing in a desktop window, so they are not reported.
                            _ if name.ends_with("text-size-adjust")
                                || name.ends_with("print-color-adjust")
                                || name.starts_with("page-break-")
                                || name.starts_with("break-")
                                || matches!(
                                    name,
                                    "orphans"
                                        | "widows"
                                        | "speak"
                                        | "forced-color-adjust"
                                        | "touch-action"
                                ) =>
                            {
                                continue;
                            }
                            _ => match style::supports(name, value) {
                                None => "the property is not supported",
                                Some(false) => "the value is not supported",
                                Some(true) => continue,
                            },
                        };
                        let unknown = reason == "the property is not supported";
                        if dom.warned.insert(if unknown {
                            format!("{pseudo}{name}")
                        } else {
                            format!("{pseudo}{name}:{value}")
                        }) {
                            let mut element = node.tag.clone();
                            if let Some(value) = node.attrs.get("id") {
                                element = format!("{element}#{value}");
                            }
                            for class in node
                                .attrs
                                .get("class")
                                .into_iter()
                                .flat_map(|value| value.split_whitespace())
                                .take(3)
                            {
                                element = format!("{element}.{class}");
                            }
                            warnings.push(format!(
                                "Unsupported CSS `{name}: {value}` on <{element}>{pseudo}: {reason}."
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
            // WARNING: a scroll farther than the overscan could bring culled boxes into view. A new
            // viewport size keeps culling: spacers whose parent changed width lay out again, and
            // `cull::VIEWPORT` subtrees are not culled.
            dom.cull.resized = dom.cull.frame[..2] != frame[..2];
            dom.cull.enabled = !std::mem::take(&mut dom.cull.off)
                && dom.cull.frame[2..] == frame[2..]
                && moved < height / 2.;
            dom.cull.frame = frame;
        }
        let zone = dom.cull.enabled.then(|| {
            Rc::new(cull::Zone {
                top: 0.,
                bottom: height,
                clips: Vec::new(),
                resized: dom.cull.resized,
            })
        });
        // The root never leaves the page: a fullscreen root is the page at the window size.
        let top_layer: Vec<usize> = dom
            .top_layer
            .iter()
            .copied()
            .filter(|&id| {
                visible.get(id) == Some(&true)
                    && doc.nodes[id].state & crate::dom::TOP_LAYER != 0
                    && doc.nodes[id].tag != "html"
            })
            .collect();
        dom.containers.clear();
        dom.flexed_grids.clear();
        dom.measurements.pixel_ratio = window.scale_factor();
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
        let ids = {
            let mut ids = self.ids.borrow_mut();
            if ids.0 != doc.nodes.latest() {
                *ids = (doc.nodes.latest(), Rc::default());
            }
            ids.1.clone()
        };
        let mut build = Build {
            doc: &doc,
            styles: &styles,
            text: &text_system,
            pixel_ratio: window.scale_factor(),
            view: &self.0,
            ancestors: Rc::default(),
            flex_parent: false,
            measured: &measured,
            inline_text: &inline_text,
            perspective: None,
            siblings: &RefCell::default(),
            margins: &RefCell::default(),
            ids: &ids,
            info: &info,
            zone,
            top_layer: &top_layer,
            tree,
            culls: &Cell::new(0),
            resized: false,
        };
        {
            // WARNING: the scale and fonts resize boxes without new revisions, and the top layer takes boxes out
            // of their parents, so a change builds every box again. A new viewport size builds the subtrees with
            // viewport units, see `cull::VIEWPORT`; other boxes key on their containing block.
            let mut kept = self.kept_frame.borrow_mut();
            if kept.0[2..] != frame[2..] || kept.1 != top_layer {
                kept.1 = top_layer.clone();
                self.kept.borrow_mut().clear();
            }
            build.resized = kept.0[..2] != frame[..2];
            kept.0 = frame;
        }
        let root = element(
            &build,
            0,
            [width - page_space[1], height],
            [width - page_space[1], height],
            None,
        );
        // Each box of the top layer paints over the page and the boxes before it, after its `::backdrop`,
        // laid out in the viewport and never culled.
        build.zone = None;
        let mut overlay: Vec<renderer::Child> = Vec::new();
        for &id in &top_layer {
            if let Some(backdrop) = styles
                .backdrops
                .get(&id)
                .filter(|style| style.get(Prop::DISPLAY) != Some("none"))
            {
                let units = styles.lengths[id];
                let mut el = renderer::node().absolute().top_0().left_0().size_full();
                if backdrop
                    .get(Prop::VISIBILITY)
                    .is_none_or(|value| value == "visible")
                {
                    if let Some((color, alpha)) =
                        backdrop.get(Prop::BACKGROUND_COLOR).and_then(style::color)
                    {
                        el = el.bg(rgba(color << 8 | alpha as u32));
                    }
                    if let Some(opacity) = backdrop
                        .get(Prop::OPACITY)
                        .and_then(|value| value.parse::<f32>().ok())
                    {
                        el = el.opacity(opacity.clamp(0., 1.));
                    }
                    if backdrop
                        .get(Prop::BACKGROUND_IMAGE)
                        .is_some_and(|value| value != "none")
                    {
                        let host = self.host.borrow();
                        el = el.children(background::css_background(
                            backdrop,
                            units,
                            id,
                            3,
                            Default::default(),
                            self.0.clone(),
                            host.dom.get(&self.id).and_then(|state| state.base.as_ref()),
                            &host.assets,
                        ));
                    }
                }
                overlay.push(
                    match [Prop::BACKDROP_FILTER, Prop::WEBKIT_BACKDROP_FILTER]
                        .into_iter()
                        .find_map(|prop| backdrop.get(prop))
                        .and_then(|value| effects::backdrop(value, &units))
                    {
                        Some((blur, color_matrix)) => el.hooks(paint::Effected {
                            effect: renderer::PaintEffect {
                                backdrop: Some(renderer::Backdrop {
                                    bounds: renderer::Bounds::default(),
                                    corner_radii: Default::default(),
                                    blur,
                                    color_matrix,
                                }),
                                ..Default::default()
                            },
                            backdrop: Some(Default::default()),
                        }),
                        None => el,
                    }
                    .into(),
                );
            }
            // The boxes of the top layer with a z-index paint around it, as in a stacking context.
            overlay.push(
                renderer::node()
                    .absolute()
                    .size_full()
                    .stacking()
                    .child(renderer::layers(true))
                    .child(element(&build, id, [width, height], [width, height], None))
                    .child(renderer::layers(false))
                    .into(),
            );
        }
        drop(build);
        for (node, children) in children.into_iter().rev() {
            doc.nodes.compose_children(node, children);
        }
        {
            let mut host = self.host.borrow_mut();
            if let Some(page) = host.pages.get_mut(&self.id) {
                page.doc = doc;
            }
            let dom = host.dom.entry(self.id).or_default();
            dom.cull.info = info;
            dom.cull.visible = visible;
            dom.measurements = measured.into_inner();
            dom.inline_text = inline_text.into_inner();
            dom.animating.clear();
            dom.animating_pseudos.clear();
            let restored = !restore.is_empty() || !restore_pseudos.is_empty();
            if restored {
                let styles = Rc::make_mut(&mut styles);
                for (id, style) in restore {
                    dom.animating
                        .insert(id, std::mem::replace(&mut styles.values[id], style));
                }
                for (id, pair) in restore_pseudos {
                    if let Some(animated) = styles.pseudos.insert(id, pair) {
                        dom.animating_pseudos.insert(id, animated);
                    }
                }
            }
            // WARNING: animated layout values change the boxes again next frame, so its flags are not kept.
            dom.cull.seen = (!relaid).then(|| (seen.0, seen.1, styles.clone()));
            if restored {
                dom.computed = Some(styles);
            }
            // WARNING: one extra pass per frame at most, so container queries that resize their container cannot loop.
            let changed = dom.update_containers();
            let repass = changed && !dom.container_pass;
            dom.container_pass = repass;
            if repass {
                drop(host);
                return self.render_page(tree, window, cx);
            }
        }
        let runtime = self.runtime.clone();
        let id = self.id;
        if !window.redrawing() && !self.reflowing.get() {
            window.on_next_frame(move |window, cx| {
                let renderer = runtime
                    .borrow()
                    .views
                    .get(&id)
                    .and_then(|v| v.renderer.clone());
                if let Some(renderer) = renderer
                    && let Err(error) = scripted(cx, Some(window), || {
                        renderer.eval("__appsLayout()", "apps:layout")
                    })
                {
                    eprintln!("apps: {error}");
                }
                cx.defer(move |cx| Desktop::drive(runtime, cx));
            });
        }
        let page = renderer::node().id(("apps-page", 0)).size_full();
        let page = if page_scrolls {
            page.overflow_y_scroll()
        } else {
            page.overflow_y_hidden()
        };
        let page = page
            .pr(renderer::px(page_space[1]))
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
                renderer::canvas(
                    |_, _| (),
                    move |_, _, _| crate::desktop::mcp::frame(id, started),
                )
                .absolute()
            }))
            .children(self.hud.borrow().is_some().then(|| {
                let data = self.0.clone();
                renderer::canvas(
                    |_, _| (),
                    move |_, _, window| {
                        if let Some(hud) = &*data.hud.borrow() {
                            hud.paint(window);
                        }
                    },
                )
                .absolute()
            }));
        // WARNING: the top layer stays out of the page's scroll container, so scrolling never moves it.
        if overlay.is_empty() {
            page.into()
        } else {
            renderer::node()
                .size_full()
                .child(page)
                .children(overlay)
                .into()
        }
    }
}

/// The properties that turn a composited box, see `cull::TRANSFORMED`.
const TURNS: [Prop; 4] = [Prop::TRANSFORM, Prop::TRANSLATE, Prop::ROTATE, Prop::SCALE];

/// Whether two styles differ in nothing but `opacity` and transforms, which composite frames patch.
fn composites(a: &style::Style, b: &style::Style) -> bool {
    let rest = |(prop, _): &(Prop, &Rc<str>)| *prop != Prop::OPACITY && !TURNS.contains(prop);
    a.iter().filter(rest).eq(b.iter().filter(rest))
}

/// Wakes the view for the next animation change, with an opacity-only frame while `composite`,
/// and sends the animation events.
#[allow(clippy::too_many_arguments)]
fn animate(
    view: &Rc<PageState>,
    dom: &mut crate::dom::State,
    next: Option<std::time::Instant>,
    now: std::time::Instant,
    composite: bool,
    events: Vec<(usize, &'static str, String, f64)>,
    window: &mut Window,
    cx: &mut App,
) {
    match next {
        Some(at) if at <= now + std::time::Duration::from_millis(8) => {
            dom.motion.wake = None;
            let view = view.clone();
            window.on_next_frame(move |window, cx| match composite {
                true => self::composite(view, window, cx),
                false => window.refresh(),
            });
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
                let view = view.clone();
                let task = cx.timer(delay, move |cx| {
                    if composite {
                        cx.update_window(window_id, |window, cx| self::composite(view, window, cx));
                    } else {
                        cx.refresh_window(window_id);
                    }
                });
                dom.motion.wake = Some((at, task));
            }
        }
        None => dom.motion.wake = None,
    }
    if !events.is_empty() {
        let runtime = view.runtime.clone();
        let id = view.id;
        cx.defer(move |cx| {
            let renderer = runtime
                .borrow()
                .views
                .get(&id)
                .and_then(|view| view.renderer.clone());
            for (node, kind, name, elapsed) in events {
                // Pseudo-element animations are named like `::before\0spin`.
                let (pseudo, name) = name.split_once('\0').unwrap_or(("", &name));
                let data = if kind.starts_with("animation") {
                    serde_json::json!({"animationName": name, "elapsedTime": elapsed, "pseudoElement": pseudo})
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
}

/// An animation frame that only changes the opacity and transform of boxes with keyed layers: it
/// samples the animations, patches those layers in the last frame's scene and shows it again. Any
/// other change, or a transform that moves a box's center, draws the whole frame.
fn composite(view: Rc<PageState>, window: &mut Window, cx: &mut App) {
    if window.will_draw() {
        return;
    }
    let started = std::time::Instant::now();
    let size = window.viewport_size();
    let viewport = [f32::from(size.width), f32::from(size.height)];
    let Ok(mut host) = view.host.try_borrow_mut() else {
        return window.refresh();
    };
    let state = &mut *host;
    let (Some(page), Some(dom)) = (state.pages.get(&view.id), state.dom.get_mut(&view.id)) else {
        return window.refresh();
    };
    let doc = &page.doc;
    // WARNING: the page must be as the last frame drew it: no DOM, style, focus or viewport change.
    let Some((.., styles)) = dom.cull.seen.clone().filter(|(latest, focus, _)| {
        !page.dirty
            && !dom.dirty
            && (*latest, *focus) == (doc.nodes.latest(), dom.focus)
            && state.viewports.get(&view.id) == Some(&viewport)
            && view.hud.borrow().is_none()
    }) else {
        return window.refresh();
    };
    let mut events = Vec::new();
    let now = std::time::Instant::now();
    let bounds = &dom.bounds;
    let shown = |id| in_view(bounds, viewport, id, clips(doc, &styles, id));
    let (animated, next) = dom.motion.update(
        doc,
        &styles,
        &dom.cull.visible,
        &shown,
        now,
        false,
        &mut events,
    );
    *view.parked.borrow_mut() = dom
        .motion
        .parked
        .iter()
        .map(|&id| (id, clips(doc, &styles, id).collect()))
        .collect();
    let mut spun = view.spun.borrow_mut();
    // WARNING: a box whose center moves could change what it covers and intersects, so only spins
    // and pulses about the center keep their layout.
    let centered =
        |bounds: &renderer::Bounds<renderer::Pixels>, a: &effects::Matrix, b: &effects::Matrix| {
            let [x, y] = [
                f32::from(bounds.origin.x + bounds.size.width / 2.),
                f32::from(bounds.origin.y + bounds.size.height / 2.),
            ];
            let center = |m: &effects::Matrix| {
                let w = m[3] * x + m[7] * y + m[15];
                [
                    (m[0] * x + m[4] * y + m[12]) / w,
                    (m[1] * x + m[5] * y + m[13]) / w,
                ]
            };
            let ([a, b], [c, d]) = (center(a), center(b));
            (a - c).abs() < 0.5 && (b - d).abs() < 0.5
        };
    let mut changes = Vec::new();
    let eligible = dom.motion.custom.is_empty()
        && dom.motion.pseudos.is_empty()
        && animated.len() == dom.animating.len()
        && animated.iter().all(|(id, style, layout)| {
            let Some(old) = dom.animating.get(id) else {
                return false;
            };
            if Rc::ptr_eq(old, style) {
                return true;
            }
            let turns = TURNS.iter().any(|&prop| old.get(prop) != style.get(prop));
            let matrix = match spun.get_mut(id).filter(|_| turns) {
                Some((bounds, perspective, last)) => {
                    let matrix =
                        effects::transform(style, &styles.lengths[*id], *bounds, *perspective)
                            .unwrap_or(effects::IDENTITY);
                    if !centered(bounds, last, &matrix) {
                        return false;
                    }
                    *last = matrix;
                    Some(matrix)
                }
                None if turns => return false,
                None => None,
            };
            changes.push((*id, effects::opacity(style), matrix));
            !layout && composites(old, style)
        });
    // WARNING: the compositor plays promoted layers until the next drawn frame, so the view only wakes
    // to give it more of their timeline and for events.
    let renew = now + std::time::Duration::from_millis(1750);
    let promoted = eligible
        && match view.promote.get() {
            Some((at, _)) if now < at => view.promote.get().is_some_and(|(_, on)| on),
            Some(_) => {
                let times: Vec<_> = (0..=240)
                    .map(|k| now + std::time::Duration::from_secs_f64(f64::from(k) / 120.))
                    .collect();
                let on = animated.iter().all(|(id, ..)| {
                    let timeline = dom.motion.timeline(*id, &styles, &times);
                    let opacities: Vec<f32> = timeline.iter().map(effects::opacity).collect();
                    let transforms = match spun.get(id) {
                        Some((bounds, perspective, last)) => {
                            let transforms: Vec<_> = timeline
                                .iter()
                                .map(|style| {
                                    effects::transform(
                                        style,
                                        &styles.lengths[*id],
                                        *bounds,
                                        *perspective,
                                    )
                                    .unwrap_or(effects::IDENTITY)
                                })
                                .collect();
                            if !transforms
                                .iter()
                                .all(|matrix| centered(bounds, last, matrix))
                            {
                                return false;
                            }
                            Some(transforms)
                        }
                        None => None,
                    };
                    !shown(*id)
                        || window.promote(*id as u64, 1. / 120., &opacities, transforms.as_deref())
                });
                // WARNING: layers promoted before one that could not be would stop with their timeline.
                if !on {
                    window.demote();
                }
                view.promote.set(on.then_some((renew, true)));
                on
            }
            None => false,
        };
    let mut flat = view.flat.borrow_mut();
    let patched = eligible
        && (promoted
            || changes.into_iter().all(|(id, opacity, matrix)| {
                window.patch_effect(id as u64, opacity, matrix) || !shown(id) || {
                    flat.insert(id);
                    false
                }
            }));
    if patched && spun.keys().any(|id| dom.animating.contains_key(id)) {
        dom.bounds.set_moved();
    }
    drop((flat, spun));
    if patched {
        dom.animating
            .extend(animated.into_iter().map(|(id, style, _)| (id, style)));
    }
    let next = match view.promote.get() {
        Some((renew, true)) if promoted => {
            let event = dom
                .animating
                .keys()
                .filter_map(|&id| dom.motion.next_event(id, &styles, now))
                .min();
            Some(event.map_or(renew, |event| event.min(renew)))
        }
        _ => next.filter(|_| patched),
    };
    animate(&view, dom, next, now, true, events, window, cx);
    drop(host);
    // WARNING: like a drawn frame, the time ends before the frame goes to the GPU.
    if patched && !promoted && crate::desktop::mcp::profiling() {
        crate::desktop::mcp::frame(view.id, started);
    }
    // WARNING: new promoted layers show with one frame that leaves them out, see `Window::promote`.
    let promoting = promoted && view.promote.get().is_some_and(|(at, _)| at == renew);
    if !patched || (!promoted || promoting) && !window.present() {
        window.refresh();
    }
}

/// Whether box `id` was in the viewport and in its clipping ancestors `clips` on their clipped axes at
/// the last layout. A box without bounds or without area counts as seen.
fn in_view(
    bounds: &crate::dom::LayoutMirror,
    [width, height]: [f32; 2],
    id: usize,
    mut clips: impl Iterator<Item = (usize, [bool; 2])>,
) -> bool {
    let Some(&[x, y, w, h]) = bounds.get(&id).filter(|rect| rect[2] > 0. && rect[3] > 0.) else {
        return true;
    };
    // WARNING: a box that rotates or scales up to 2x around its center stays within half its larger side around it.
    let pad = w.max(h) / 2.;
    let [left, top, right, bottom] = [x - pad, y - pad, x + w + pad, y + h + pad];
    let overlaps = |[a, b, c, d]: [f32; 4], [horizontal, vertical]: [bool; 2]| {
        (!horizontal || left.max(a) < right.min(a + c))
            && (!vertical || top.max(b) < bottom.min(b + d))
    };
    overlaps([0., 0., width, height], [true; 2])
        && clips.all(|(clip, axes)| bounds.get(&clip).is_none_or(|&rect| overlaps(rect, axes)))
}

/// The ancestors that clip box `id`, with their clipped axes. The list ends at a positioned box, which can escape them.
fn clips<'a>(
    doc: &'a crate::dom::Document,
    styles: &'a style::ComputedStyles,
    id: usize,
) -> impl Iterator<Item = (usize, [bool; 2])> + 'a {
    let mut node = Some(id);
    std::iter::from_fn(move || {
        loop {
            let child = node?;
            node = doc.nodes[child].parent.filter(|_| {
                !matches!(
                    styles.values[child].get(Prop::POSITION),
                    Some("absolute" | "fixed")
                )
            });
            let parent = node?;
            let style = &styles.values[parent];
            let axes = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                .map(|prop| style.get(prop).is_some_and(|value| value != "visible"));
            if axes != [false; 2]
                && !matches!(doc.nodes[parent].tag.as_str(), "html" | "body")
                && style
                    .get(Prop::DISPLAY)
                    .is_some_and(|value| !matches!(value, "inline" | "contents"))
            {
                return Some((parent, axes));
            }
        }
    })
}

/// Drops the cached images of nodes that are not displayed. Canvases keep their pixels.
fn retire(
    dom: &mut crate::dom::State,
    visible: &[bool],
    styles: &style::ComputedStyles,
    mut release: impl FnMut(Arc<renderer::RenderImage>),
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
    for (id, canvas) in &dom.canvases {
        if visible.get(*id) != Some(&true) {
            release(canvas.image.clone());
        }
    }
    // WARNING: `url()` layers share one decoded image per file, kept under `usize::MAX` while a layer uses it.
    // Control layers are kept under `usize::MAX` minus their kind.
    dom.backgrounds.retain(|&(id, layer), (_, image)| {
        let keep = if id == usize::MAX {
            Arc::strong_count(image) > 1
        } else if layer > usize::MAX - 64 {
            visible.get(id) == Some(&true)
        } else {
            visible.get(id) == Some(&true)
                && match layer / 64 {
                    0 => styles.values.get(id).map(|style| &**style),
                    3 => styles.backdrops.get(&id).map(|style| &**style),
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
    fn boxes_out_of_view_or_in_closed_clips_are_not_in_view() {
        let mut doc = crate::dom::Document::parse(
            "<div id=panel style='height:0;overflow:hidden'><i id=a></i><i id=b style='position:absolute'></i></div><b id=c></b><span style='overflow:hidden'><b id=d></b></span>",
        );
        let styles = crate::dom::styles(&mut doc, &mut Default::default(), [800., 600.], None);
        let id = |name: &str| {
            (0..doc.nodes.len())
                .find(|&id| {
                    doc.nodes[id]
                        .attrs
                        .get("id")
                        .is_some_and(|value| value == name)
                })
                .unwrap()
        };
        let mut bounds = crate::dom::LayoutMirror::default();
        let check = |bounds: &crate::dom::LayoutMirror, name| {
            in_view(
                bounds,
                [800., 600.],
                id(name),
                clips(&doc, &styles, id(name)),
            )
        };
        assert!(check(&bounds, "a"));
        bounds.insert(id("panel"), [0., 0., 800., 0.]);
        for name in ["a", "b", "c", "d"] {
            bounds.insert(id(name), [10., 10., 16., 16.]);
        }
        assert_eq!(
            ["a", "b", "c", "d"].map(|name| check(&bounds, name)),
            [false, true, true, true]
        );
        bounds.insert(id("panel"), [0., 0., 800., 40.]);
        bounds.insert(id("c"), [10., 620., 16., 16.]);
        assert!(check(&bounds, "a") && !check(&bounds, "c"));
        bounds.insert(id("c"), [10., 600., 16., 0.]);
        assert!(check(&bounds, "c"));
    }

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
            .draw(
                &[4., 7., 21., 0., 0., 2., 2.],
                "#ff0000",
                &[],
                &|_| None,
                None,
            )
            .unwrap();
        dom.canvases.insert(canvas, drawn);
        let image = |size: u32| {
            Arc::new(renderer::RenderImage::new(
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
