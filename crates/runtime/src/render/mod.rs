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
mod inspect;
pub(crate) mod native;
mod paint;
mod plan;
mod resvg;
pub(crate) mod scrollbar;
pub(crate) mod terminal;
pub(crate) mod text;

use crate::style::Prop;
use crate::{desktop::Desktop, host::Host, style};
pub(crate) use element::dimension;
use element::{Build, element};
pub(crate) use hud::Hud;
pub(crate) use inspect::Inspect;
use renderer::{App, Color, Event, Window, WindowId};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

pub(crate) struct PageState {
    pub(crate) id: u64,
    queued: Cell<bool>,
    pub(crate) runtime: Rc<RefCell<Desktop>>,
    pub(crate) host: Rc<RefCell<Host>>,
    images: RefCell<HashMap<usize, Arc<renderer::RenderImage>>>,
    pub(crate) frames: RefCell<HashMap<usize, Frame>>,
    controls: RefCell<native::Controls>,
    ids: RefCell<(u64, Rc<effects::Ids>)>,
    svgs: RefCell<HashMap<usize, element::Svg>>,
    fields: RefCell<HashMap<usize, input::Caret>>,
    decoding: RefCell<HashMap<String, (usize, background::Decoding)>>,
    moving: Cell<bool>,
    dropped: RefCell<Vec<PathBuf>>,
    buttons: Cell<u8>,
    origin: Cell<Option<renderer::Point<renderer::Pixels>>>,
    window: WindowId,
    reflowing: Cell<bool>,
    parked: RefCell<Vec<(usize, Clips)>>,
    flat: RefCell<rustc_hash::FxHashSet<usize>>,
    promote: Cell<Option<(std::time::Instant, bool)>>,
    pub(super) composited_transforms: RefCell<rustc_hash::FxHashMap<usize, CompositedTransform>>,
    reach: RefCell<Vec<[f32; 2]>>,
    reaching: RefCell<Vec<([f32; 2], u8)>>,
    caret: RefCell<Option<input::Blink>>,
    pub(crate) hud: RefCell<Option<hud::Hud>>,
    pub(crate) inspect: Cell<inspect::Inspect>,
    kept: RefCell<rustc_hash::FxHashMap<usize, element::Kept>>,
    plans: RefCell<plan::Plans>,
    repaint: RefCell<rustc_hash::FxHashSet<usize>>,
    built: RefCell<rustc_hash::FxHashSet<usize>>,
    animated: RefCell<Vec<usize>>,
    #[cfg(debug_assertions)]
    around: RefCell<rustc_hash::FxHashSet<usize>>,
    // Retain plans used as pointer keys to prevent address reuse.
    shared: RefCell<rustc_hash::FxHashMap<u64, Vec<element::Shared>>>,
    focused: Cell<Option<usize>>,
    patches: RefCell<rustc_hash::FxHashMap<usize, element::Patch>>,
    epoch: Cell<(u64, bool)>,
    reads: RefCell<Vec<element::Read>>,
    walked: RefCell<rustc_hash::FxHashMap<usize, element::Walked>>,
    adjoined: RefCell<rustc_hash::FxHashMap<usize, element::Adjoined>>,
    kept_frame: RefCell<([f32; 4], Vec<usize>)>,
    mismatched: RefCell<[Mismatches; 2]>,
    layout_queued: Cell<bool>,
    #[cfg(target_os = "macos")]
    canvas: Cell<(u32, u8)>,
    canvas_layers: Cell<Option<u64>>,
}

pub(crate) struct Frame {
    pub(crate) browser: crate::desktop::browser::Browser,
    loaded: String,
    pub(crate) bounds: [f64; 4],
    revision: u64,
    shown: bool,
}

type Clips = Vec<(usize, [bool; 2])>;
type Mismatches = rustc_hash::FxHashSet<(usize, [u32; 4])>;
type CompositedTransform = (renderer::Rect<renderer::Pixels>, Option<(f32, [f32; 2])>);

// Layout revisions are unique across pages and reloads, so a kept box never matches a node of another page.
static LAYOUT_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

thread_local! {
    static SCRIPTS: Cell<Option<(*mut App, Option<*mut Window>)>> = const { Cell::new(None) };
    static VIEWS: RefCell<HashMap<u64, std::rc::Weak<PageState>>> = RefCell::default();
}

// Do not use cx or window inside f: reflow accesses them through pointers.
pub(crate) fn scripted<R>(cx: &mut App, window: Option<&mut Window>, f: impl FnOnce() -> R) -> R {
    crate::jsc::REFLOW.set(Some(reflow));
    crate::jsc::CLIPBOARD.set(Some(clipboard));
    let previous = SCRIPTS.replace(Some((cx, window.map(|window| window as *mut Window))));
    let result = f();
    SCRIPTS.set(previous);
    result
}

fn clipboard(text: Option<String>) -> Option<String> {
    let (cx, _) = SCRIPTS.get()?;
    // SAFETY: the pointer comes from `scripted`, whose caller does not touch it until the script returns.
    let cx = unsafe { &*cx };
    match text {
        Some(text) => {
            cx.write_clipboard(text);
            Some(String::new())
        }
        None => Some(cx.read_clipboard().unwrap_or_default()),
    }
}

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
                .then(|| cx.read_clipboard().unwrap_or_default());
            let result = scripted(cx, window, || run(pasted.as_deref().unwrap_or(text)));
            if let Ok(copied) = &result
                && matches!(command, "copy" | "cut")
                && !copied.is_empty()
            {
                cx.write_clipboard(copied.clone());
            }
            result
        }
        None => run(text),
    };
    if let Err(error) = result {
        eprintln!("apps: {error}");
    }
}

fn reflow(id: u64) {
    let Some((cx, window)) = SCRIPTS.get() else {
        return;
    };
    let Some(view) = VIEWS.with_borrow(|views| views.get(&id).and_then(std::rc::Weak::upgrade))
    else {
        return;
    };
    // Reflow after composite frames even if the page is clean: boxes moved without layout.
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
    // SAFETY: the pointers come from `scripted`, whose caller does not touch them until the script returns.
    let cx = unsafe { &mut *cx };
    match window.map(|window| unsafe { &mut *window }) {
        Some(window) if window.id() == view.window => run(window, cx),
        _ => {
            cx.update_window(view.window, run);
        }
    }
}

fn queue_layout(view: Rc<PageState>, window: &mut Window) {
    if view.layout_queued.replace(true) {
        return;
    }
    window.next_frame(move |window, cx| {
        view.layout_queued.set(false);
        let renderer = view
            .runtime
            .borrow()
            .views
            .get(&view.id)
            .and_then(|view| view.renderer.clone());
        if let Some(renderer) = renderer
            && let Err(error) = scripted(cx, Some(window), || {
                renderer.eval("__appsLayout()", "apps:layout")
            })
        {
            eprintln!("apps: {error}");
        }
        let runtime = view.runtime.clone();
        cx.defer(move |cx| Desktop::drive(runtime, cx));
    });
}

// Run animation callbacks at most once per frame, or recursive requests accumulate.
pub(crate) fn queue_animation_frame(view: Rc<PageState>, window: &mut Window) {
    if !view.queued.replace(true) {
        window.next_frame(move |window, cx| animation_frame(view, window, cx));
    }
}

fn animation_frame(view: Rc<PageState>, window: &mut Window, cx: &mut App) {
    view.queued.set(false);
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
        queue_animation_frame(view.clone(), window);
    }
    let runtime = view.runtime.clone();
    cx.defer(move |cx| Desktop::drive(runtime, cx));
}

impl PageState {
    #[allow(clippy::too_many_arguments)]
    fn place(
        &self,
        doc: &crate::dom::Document,
        styles: &style::ComputedStyles,
        texts: &mut rustc_hash::FxHashMap<usize, Option<Rc<style::InlineText>>>,
        bounds: &crate::dom::LayoutMirror,
        tree: &renderer::Tree,
        near: Option<[f32; 2]>,
        id: usize,
    ) -> bool {
        let kept = self.kept.borrow();
        let mut node = id;
        let place = loop {
            if let Some(record) = kept.get(&node) {
                break record
                    .in_place()
                    .filter(|_| {
                        doc.nodes[node].tag != "svg"
                            && near.is_none_or(|[top, bottom]| {
                                bounds.get(&node).is_some_and(|rect| {
                                    rect[1] + rect[3] >= top && rect[1] <= bottom
                                })
                            })
                    })
                    .map(|ancestors| Some((node, ancestors.clone())));
            }
            let inline = doc.nodes[node].tag != "svg"
                && (doc.nodes[node].tag.starts_with('#')
                    || styles.values[node]
                        .get(Prop::DISPLAY)
                        .is_none_or(|value| matches!(value, "inline" | "contents")));
            match doc.nodes[node].parent {
                Some(parent) if inline => node = parent,
                _ => break (!tree.contains(node) && doc.nodes[node].tag != "svg").then_some(None),
            }
        };
        if let Some(Some((target, _ancestors))) = &place {
            self.repaint.borrow_mut().insert(*target);
            #[cfg(debug_assertions)]
            self.around.borrow_mut().extend(_ancestors.iter());
        }
        if place.is_some() {
            let mut path = Some(id);
            while let Some(at) = path {
                texts.remove(&at);
                path = doc.nodes[at].parent.filter(|_| at != node);
            }
        }
        place.is_some()
    }

    fn screen(&self, window: &Window, at: renderer::Point<renderer::Pixels>) -> [f32; 2] {
        let origin = self.origin.get().unwrap_or_else(|| {
            let origin = window.window_state().content.origin;
            self.origin.set(Some(origin));
            origin
        });
        let at = origin + at * window.zoom();
        [f32::from(at.x), f32::from(at.y)]
    }

    fn pointer(
        &self,
        window: &Window,
        at: renderer::Point<renderer::Pixels>,
        modifiers: renderer::Modifiers,
    ) -> serde_json::Value {
        let [x, y] = self.screen(window, at);
        serde_json::json!({"clientX":f32::from(at.x),"clientY":f32::from(at.y),"screenX":x,"screenY":y,"buttons":self.buttons.get(),"ctrlKey":modifiers.ctrl(),"metaKey":modifiers.command(),"altKey":modifiers.alt(),"shiftKey":modifiers.shift()})
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
            queued: Cell::new(false),
            runtime,
            host,
            images: RefCell::default(),
            frames: RefCell::default(),
            controls: RefCell::default(),
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
            composited_transforms: RefCell::default(),
            reach: RefCell::default(),
            reaching: RefCell::default(),
            promote: Cell::new(None),
            caret: RefCell::default(),
            hud: RefCell::default(),
            inspect: Cell::default(),
            kept: RefCell::default(),
            plans: RefCell::default(),
            repaint: RefCell::default(),
            built: RefCell::default(),
            animated: RefCell::default(),
            #[cfg(debug_assertions)]
            around: RefCell::default(),
            shared: RefCell::default(),
            focused: Cell::default(),
            patches: RefCell::default(),
            epoch: Cell::default(),
            reads: RefCell::default(),
            walked: RefCell::default(),
            adjoined: RefCell::default(),
            #[cfg(target_os = "macos")]
            canvas: Cell::default(),
            canvas_layers: Cell::default(),
            kept_frame: RefCell::default(),
            mismatched: RefCell::default(),
            layout_queued: Cell::default(),
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
            state.window = window.window_state();
            state.sync_scroll();
            state.bounds.touch();
        }
        let data = self.0.clone();
        // Presses go to the page in the bubble phase, after drag areas and scrollbars could stop them.
        let pointer_events = renderer::canvas(|_, _| (), move |_, _, window| {
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
            if data.inspect.get().pick {
                let view = data.clone();
                // The picker of the developer tools takes the pointer from the page.
                window.on_capture(move |event, _, cx| {
                    let (at, click) = match *event {
                        Event::PointerMove { at, .. } => (at, false),
                        Event::PointerDown { at, .. } => (at, true),
                        Event::PointerUp { .. } => return cx.stop_event(),
                        _ => return,
                    };
                    view.host.borrow_mut().command(
                        crate::desktop::link::Target::Window(view.id),
                        serde_json::json!({"op":"window","args":["inspect",view.id,f32::from(at.x),f32::from(at.y),click]}),
                    );
                    let runtime = view.runtime.clone();
                    cx.defer(move |cx| Desktop::drive(runtime, cx));
                    cx.stop_event();
                });
            }
            let view = data.clone();
            window.on_capture(move |event, window, cx| {
                let (Event::PointerLeave { at: position } | Event::PointerMove { at: position, .. } | Event::PointerUp { at: position, .. }) = *event else {
                    return;
                };
                let (kind, button) = match *event {
                    Event::PointerLeave { .. } => {
                        let [x, y] = view.screen(window, position);
                        Desktop::dispatch(view.runtime.clone(), view.id, "pointerexit", &[], serde_json::json!({"clientX":f32::from(position.x),"clientY":f32::from(position.y),"screenX":x,"screenY":y}), Some(window), cx);
                        return;
                    }
                    Event::PointerMove { held, .. } => {
                        if held.is_none() {
                            view.buttons.set(0);
                        }
                        ("pointermove", None)
                    }
                    Event::PointerUp { button, .. } => {
                        let button = button as u8;
                        view.buttons.set(view.buttons.get() & !BUTTONS[usize::from(button)]);
                        ("pointerup", Some(button))
                    }
                    _ => return,
                };
                let mut fields = view.pointer(window, position, event.modifiers().unwrap_or_default());
                if let Some(button) = button {
                    fields["button"] = button.into();
                }
                Desktop::dispatch(view.runtime.clone(), view.id, kind, &[], fields, Some(window), cx);
            });
            let view = data.clone();
            window.on_bubble(move |event, window, cx| {
                if let Event::PointerDown { at, button, modifiers, clicks, .. } = *event {
                    let button = button as u8;
                    view.buttons.set(view.buttons.get() | BUTTONS[usize::from(button)]);
                    let mut fields = view.pointer(window, at, modifiers);
                    (fields["button"], fields["detail"]) = (button.into(), clicks.into());
                    Desktop::dispatch(view.runtime.clone(), view.id, "pointerdown", &[], fields, Some(window), cx);
                }
            });
            let view = data.clone();
            window.on_capture(move |event, window, cx| {
                let Event::Wheel { at: position, delta, lines, phase, modifiers } = *event else {
                    return;
                };
                if let (renderer::WheelPhase::Began | renderer::WheelPhase::Ended, Ok(host)) =
                    (phase, view.host.try_borrow())
                    && let Some(link) = &host.link
                {
                    let kind = if phase == renderer::WheelPhase::Began {
                        "gestureScrollBegin"
                    } else {
                        "gestureScrollEnd"
                    };
                    link.event(
                        format!("__appsEvent('input-event',{},{{type:'{kind}'}})", view.id),
                        "apps:input",
                    );
                }
                queue_layout(view.clone(), window);
                let (mut target, mut wheel, mut terminal) = (None, false, None);
                if let Ok(host) = view.host.try_borrow() {
                    let viewport = *host.viewports.get(&view.id).unwrap_or(&[800., 600.]);
                    if let (Some(page), Some(state)) = (host.pages.get(&view.id), host.dom.get(&view.id)) {
                        let seen = state.cull.seen.as_ref().map(|seen| seen.2.clone()).or_else(|| state.computed.clone());
                        target = seen.as_ref().and_then(|computed| {
                            let at = [position.x, position.y].map(f32::from);
                            crate::dom::hit_test(&page.doc, state, computed, viewport, at, true, false).as_u64()
                        })
                        .map(|id| id as usize);
                        if let Some(target) = target {
                            let mut chain = Vec::new();
                            let mut node = Some(target);
                            while let Some(id) = node {
                                if terminal.is_none() && page.doc.nodes[id].tag == "terminal" {
                                    terminal = Some((id, state.terminals.clone()));
                                }
                                chain.extend(state.scroll.get(&id).cloned());
                                node = seen.as_ref().and_then(|computed| crate::dom::flat_parent(&page.doc, state, computed, id));
                            }
                            window.scroll_only(chain);
                        }
                        wheel = state.wheel;
                    }
                }
                if wheel {
                    let delta = if lines { delta.map(|value| value * 16.) } else { delta };
                    let mut fields = view.pointer(window, position, modifiers);
                    (fields["deltaX"], fields["deltaY"]) = ((-delta.x).into(), (-delta.y).into());
                    if !Desktop::dispatch(view.runtime.clone(), view.id, "wheel", target.as_slice(), fields, Some(window), cx) {
                        cx.stop_event();
                        return;
                    }
                }
                // A terminal scrolls its own rows, or sends the wheel to its program.
                if let Some((node, terminals)) = terminal
                    && let Some(terminal) = terminals.borrow_mut().shown(node)
                {
                    let mods = u32::from(modifiers.shift()) | u32::from(modifiers.ctrl()) << 1 | u32::from(modifiers.alt()) << 2 | u32::from(modifiers.command()) << 3;
                    let rows = delta.y * if lines { 16. } else { 1. };
                    terminal.wheel(-rows, mods, position.x.into(), position.y.into());
                    cx.stop_event();
                    window.refresh();
                }
            });
        }).absolute().size_full();
        let mut host = self.host.borrow_mut();
        let size = window.viewport_size();
        let width = f32::from(size.width);
        let height = f32::from(size.height);
        let resized = host.viewports.insert(self.id, [width, height]) != Some([width, height]);
        // The desktop is borrowed while it opens a window, so resize events go through a deferred
        // `Desktop::drive`.
        if resized && !host.resized.contains(&self.id) {
            host.resized.push(self.id);
            let runtime = self.runtime.clone();
            cx.defer(move |cx| Desktop::drive(runtime, cx));
        }
        let inspectable = !host.packaged;
        let Some(page) = host.pages.get_mut(&self.id) else {
            return renderer::node().size_full().into();
        };
        page.dirty = false;
        let mut doc = std::mem::take(&mut page.doc);
        let state = &mut *host;
        let dom = state.dom.entry(self.id).or_default();
        // Clear node-ID caches on reload or navigation: the new DOM reuses IDs. Stamp its layout revision on
        // the first frame.
        if dom.layout_revision == 0 {
            self.kept.take();
            self.patches.take();
            self.kept_frame.take();
            self.walked.take();
            self.adjoined.take();
            self.svgs.take();
            self.fields.take();
            self.decoding.take();
            self.flat.take();
            self.parked.take();
            self.caret.take();
            self.mismatched.take();
            self.frames.take();
        }
        {
            // Redraw once per mismatch until a pass has none, to stop alternating sizes from causing endless
            // frames.
            let [current, seen] = &mut *self.mismatched.borrow_mut();
            if current.is_empty() || seen.len() > 1024 {
                seen.clear();
            }
            seen.extend(current.drain());
        }
        for image in dom.retired.drain(..) {
            window.release_image(image);
        }
        for (node, image) in dom.recycled.drain(..) {
            window.release_image(image.clone());
            if let Ok(image) = Arc::try_unwrap(image)
                && let Some(media) = state.media.get_mut(&self.id)
            {
                media.recycle(node as u64, image);
            }
        }
        dom.retired_canvases.clear();
        let cached = dom.computed.clone();
        let mut styles = crate::dom::styles(&mut doc, dom, [width, height]);
        if cached.is_none_or(|cached| !std::rc::Rc::ptr_eq(&cached, &styles))
            && fonts::load(&doc, dom, &state.assets).0
        {
            styles = crate::dom::styles(&mut doc, dom, [width, height]);
        }
        let children: Vec<_> = styles
            .children
            .iter()
            .map(|(&node, children)| (node, doc.nodes.compose_children(node, children.clone())))
            .collect();
        let seen = (doc.nodes.latest(), dom.focus);
        // Remove cull.seen's extra Rc during the frame so animated styles can swap values without copying.
        let last = dom.cull.seen.take();
        let (walked, serial) = (dom.cull.walk.nodes.clone(), dom.cull.walk.serial);
        let same = last.as_ref().is_some_and(|(latest, focus, computed)| {
            (*latest, *focus) == seen && Rc::ptr_eq(computed, &styles)
        });
        let mut visible = std::mem::take(&mut dom.cull.visible);
        let mut previous = None;
        let patched = !same && walk(&doc, dom, &styles);
        let none = |style: &Rc<style::Style>| style.get(Prop::DISPLAY) == Some("none");
        let rewalked = !Rc::ptr_eq(&dom.cull.walk.nodes, &walked);
        // A patched walk changes the visibility of the nodes it touched only. Table cells also show their rows,
        // so their lists must be the same.
        let touched = dom.cull.walk.since(serial).filter(|_| patched);
        let steady = same
            || dom.cull.cascaded
                && (!rewalked || touched.is_some())
                && last.as_ref().is_some_and(|(.., computed)| {
                    styles
                        .values
                        .differing(&computed.values)
                        .flatten()
                        .all(|id| {
                            computed.values.get(id).is_none_or(|old| {
                                styles
                                    .values
                                    .get(id)
                                    .is_some_and(|new| none(old) == none(new))
                            })
                        })
                });
        // The nodes that a patched walk showed or hid.
        let mut flips = Vec::new();
        if steady {
            let start = visible.len();
            if start < doc.nodes.len() {
                visible.resize(doc.nodes.len(), false);
                dom.bounds.hide(&visible, &[], start..visible.len());
            }
            let walk = &dom.cull.walk;
            let mut placed: Vec<(u32, u32)> = Vec::new();
            for &id in touched.into_iter().flatten() {
                match walk.place(id as usize) {
                    Some(at) => placed.push((at, id)),
                    None if visible[id as usize] => {
                        visible[id as usize] = false;
                        flips.push(id as usize);
                    }
                    None => {}
                }
            }
            placed.sort_unstable();
            placed.dedup();
            for (at, id) in placed {
                let (id, parent) = (id as usize, walk.nodes[at as usize].1);
                let style = &styles.values[id];
                let shown = (parent == u32::MAX || visible[parent as usize])
                    && !match dom.layout_nodes.get(id).and_then(Option::as_ref) {
                        Some(old) if Rc::ptr_eq(&old.style, style) => old.flags & cull::HIDDEN != 0,
                        _ => none(style),
                    };
                if visible[id] != shown {
                    visible[id] = shown;
                    flips.push(id);
                }
            }
            for &id in &flips {
                dom.bounds.show(id, visible[id]);
            }
        } else {
            previous = Some(std::mem::replace(
                &mut visible,
                visibility(&doc, dom, &styles),
            ));
            dom.cull.cascaded = true;
        }
        #[cfg(debug_assertions)]
        if steady && !same {
            assert!(
                visible == visibility(&doc, dom, &styles),
                "kept visibility differs"
            );
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
        let mut images = HashMap::new();
        if let Some(gpu) = state.graphics.get_mut(&self.id) {
            for &id in &dom.cull.canvases {
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
                window.release_image(old);
            }
        }
        if let Some(painted) = dom.motion.painted.take() {
            previous.get_or_insert(std::mem::replace(&mut visible, painted));
            dom.cull.cascaded = false;
        }
        let flipped = previous.is_some();
        if let Some(previous) = previous {
            if walked.len() + dom.cull.walk.nodes.len() < visible.len() {
                let walks = walked.iter().chain(dom.cull.walk.nodes.iter());
                let nodes = walks
                    .map(|&(id, _)| id as usize)
                    .chain(previous.len()..visible.len());
                dom.bounds.hide(&visible, &previous, nodes);
            } else {
                dom.bounds.hide(&visible, &previous, 0..visible.len());
            }
        }
        let backdrops = std::mem::take(&mut dom.motion.backdrops);
        *self.parked.borrow_mut() = dom
            .motion
            .parked
            .iter()
            .map(|&id| (id, clips(&doc, &styles, id).collect()))
            .collect();
        // Units of nodes whose font size animates, like `em` lengths, swapped in for this frame.
        let mut lengths = Vec::new();
        if !dom.motion.custom.is_empty() {
            dom.animated = std::mem::take(&mut dom.motion.custom);
            let restyled = crate::dom::styles(&mut doc, dom, [width, height]);
            lengths = (0..restyled.values.len().min(styles.values.len()))
                .filter_map(|id| {
                    let new = *restyled.lengths.get(id)?;
                    (styles.lengths.get(id) != Some(&new)).then_some((id, new))
                })
                .collect();
            animated = restyled
                .values
                .differing(&styles.values)
                .flatten()
                .filter(|&id| {
                    styles
                        .values
                        .get(id)
                        .is_some_and(|old| !Rc::ptr_eq(&restyled.values[id], old))
                })
                .map(|id| (id, restyled.values[id].clone(), true))
                .collect();
        }
        let mut flat = self.flat.borrow_mut();
        *flat = animated
            .iter()
            .map(|(id, ..)| *id)
            .filter(|id| flat.contains(id))
            .collect();
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
                let turns = TRANSFORM_PROPS.iter().any(|&prop| {
                    style.get(prop).is_some_and(|value| value != "none")
                        || style.get(prop) != base.get(prop)
                });
                (*id, turns)
            })
            .collect();
        drop(flat);
        let compositing = !animated.is_empty()
            && composited.len() == animated.len()
            && dom.motion.pseudos.is_empty()
            && backdrops.is_empty();
        animate(&self.0, dom, next, now, compositing, window, cx);
        // Restore cascade values and pseudos after the frame: animations replace them temporarily.
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
            paint.sort_unstable_by_key(|(id, _)| *id);
        }
        let painting = |paint: &[(usize, Rc<style::Style>)], id: usize| {
            paint.binary_search_by_key(&id, |(id, _)| *id).ok()
        };
        if !lengths.is_empty() {
            dom.computed = None;
            let units = &mut Rc::make_mut(&mut styles).lengths;
            for (id, new) in &mut lengths {
                let old = *units.get(*id).unwrap_or(new);
                units.set(*id, std::mem::replace(new, old));
            }
        }
        let relaid = !restore.is_empty() || !lengths.is_empty();
        // Clear node-ID caches for hidden or removed nodes here, and freed nodes in dom.release. New nodes
        // can reuse their IDs.
        let shown = |id: usize| visible.get(id) == Some(&true);
        drop_hidden_images(dom, &visible, &styles, |image| window.release_image(image));
        for id in flips.into_iter().filter(|&id| !shown(id)) {
            self.svgs.borrow_mut().remove(&id);
            self.kept.borrow_mut().remove(&id);
            self.patches.borrow_mut().remove(&id);
            self.walked.borrow_mut().remove(&id);
            self.adjoined.borrow_mut().remove(&id);
            self.fields.borrow_mut().remove(&id);
            self.decoding
                .borrow_mut()
                .retain(|_, (node, _)| *node != id);
            dom.containers.remove(&id);
            dom.flexed_grids.remove(&id);
            dom.grid_widest.remove(&id);
            dom.anchors.remove(&id);
        }
        if flipped {
            self.svgs.borrow_mut().retain(|&id, _| shown(id));
            self.kept.borrow_mut().retain(|&id, _| shown(id));
            self.patches.borrow_mut().retain(|&id, _| shown(id));
            self.walked.borrow_mut().retain(|&id, _| shown(id));
            self.adjoined.borrow_mut().retain(|&id, _| shown(id));
            self.fields.borrow_mut().retain(|&id, _| shown(id));
            self.decoding.borrow_mut().retain(|_, (id, _)| shown(*id));
            dom.containers.retain(|&id, _| shown(id));
            dom.flexed_grids.retain(|&id, _| shown(id));
            dom.grid_widest.retain(|&id, _| shown(id));
            dom.anchors.retain(|&id, _| shown(id));
        }
        {
            let mut reach = self.reach.borrow_mut();
            if reach.len() > doc.nodes.len() {
                reach.truncate(doc.nodes.len());
                reach.shrink_to_fit();
            }
        }
        if self.focused.replace(dom.focus) != dom.focus
            && let Some(focus) = dom.focus.filter(|&id| id < doc.nodes.len())
        {
            let mut kept = self.kept.borrow_mut();
            let mut node = Some(focus);
            while let Some(id) = node {
                kept.remove(&id);
                node = crate::dom::flat_parent(&doc, dom, &styles, id);
            }
        }
        if !same {
            let unscrolled: Vec<usize> = dom
                .scroll
                .keys()
                .copied()
                .filter(|&id| {
                    shown(id)
                        && id != 0
                        && !matches!(doc.nodes[id].tag.as_str(), "html" | "body")
                        && !element::text_field(&doc.nodes[id])
                        && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                            .into_iter()
                            .all(|prop| {
                                styles.values[id]
                                    .get(prop)
                                    .is_none_or(|value| matches!(value, "visible" | "clip"))
                            })
                })
                .collect();
            // Pages read offsets from the layout mirror, which only handles write.
            for id in unscrolled {
                dom.scroll.remove(&id);
                dom.bounds.unscroll(id);
            }
        }
        native::sync(&self.controls, &doc);
        let iframes = &dom.cull.iframes;
        let frame_nodes = || iframes.iter().copied().filter(|&id| visible[id]);
        let mut frames = self.frames.borrow_mut();
        frames.retain(|id, _| frame_nodes().any(|node| node == *id));
        for node in frame_nodes() {
            let (attrs, revision) = (&doc.nodes[node].attrs, doc.nodes.revision(node));
            let frame = match frames.entry(node) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    match crate::desktop::browser::Browser::new(
                        window,
                        &serde_json::json!({"javascript":attrs.get("sandbox").is_none_or(|s|s.split_whitespace().any(|s|s=="allow-scripts")),"inspectable":inspectable}),
                    ) {
                        Ok(browser) => entry.insert(Frame {
                            browser,
                            loaded: String::new(),
                            bounds: [f64::NAN; 4],
                            revision: 0,
                            shown: true,
                        }),
                        Err(error) => {
                            eprintln!("apps: iframe: {error}");
                            continue;
                        }
                    }
                }
            };
            let mut shown = styles.values[node]
                .get(Prop::VISIBILITY)
                .is_none_or(|value| value == "visible");
            let mut current = Some(node);
            while let Some(id) = current
                && shown
            {
                let style = painting(&paint, id).map_or(&styles.values[id], |at| &paint[at].1);
                shown = effects::opacity(style) > 0.;
                current = doc.nodes[id].parent;
            }
            if frame.shown != shown {
                frame.shown = shown;
                let _ = frame.browser.command("visible", &serde_json::json!(shown));
            }
            if frame.revision == revision {
                continue;
            }
            frame.revision = revision;
            let html = attrs.get("srcdoc");
            let url = attrs.get("src").map_or("about:blank", String::as_str);
            let source = html.map_or(url, String::as_str);
            if source != frame.loaded {
                let result = match html {
                    Some(html) => frame.browser.load_html(html, "about:blank"),
                    None => frame.browser.load_url(url),
                };
                if let Err(error) = result {
                    eprintln!("apps: iframe: {error}");
                }
                frame.loaded = source.to_owned();
            }
        }
        drop(frames);
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
        let canvas = |id: usize| {
            styles.values[id]
                .get(Prop::BACKGROUND_COLOR)
                .and_then(style::color)
                .filter(|color| color.1 > 0)
        };
        let dark = html
            .and_then(|id| styles.values[id].get(Prop::COLOR_SCHEME))
            .is_some_and(|scheme| {
                scheme.contains("dark")
                    && (!scheme.contains("light") || crate::desktop::native::theme_state() & 1 != 0)
            });
        let source = background::canvas_source(&doc, &styles);
        // The html and body boxes read each other's background, which their kept builds don't track.
        let layers = source.map(|id| {
            let style = &styles.values[id];
            element::hashed((
                id,
                [
                    Prop::BACKGROUND_IMAGE,
                    Prop::BACKGROUND_COLOR,
                    Prop::BACKGROUND_SIZE,
                    Prop::BACKGROUND_POSITION,
                    Prop::BACKGROUND_REPEAT,
                    Prop::BACKGROUND_ORIGIN,
                    Prop::BACKGROUND_ATTACHMENT,
                    Prop::BACKGROUND_BLEND_MODE,
                ]
                .map(|prop| style.get(prop)),
                style.get(Prop::FONT_SIZE),
            ))
        });
        if self.canvas_layers.replace(layers) != layers {
            let mut kept = self.kept.borrow_mut();
            for id in [0].into_iter().chain(html).chain(body) {
                kept.remove(&id);
            }
        }
        let background = source
            .and_then(canvas)
            .or_else(|| match state.backgrounds.get(&self.id).copied() {
                Some(color) if color.1 == 0 => Some(color),
                _ if dark => Some((0x121212, 255)),
                color => color,
            })
            .unwrap_or((0xffffff, 255));
        // Never set a transparent window's native color: it prevents later transparency.
        #[cfg(target_os = "macos")]
        if background.1 == 255
            && state
                .backgrounds
                .get(&self.id)
                .is_none_or(|color| color.1 != 0)
            && self.canvas.replace(background) != background
        {
            window.change_window(renderer::WindowChange::Background(renderer::Color::hex(
                (background.0 << 8) | 255,
            )));
        }
        dom.layout_nodes.resize_with(doc.nodes.len(), || None);
        let frame = [width, height, window.scale_factor(), dom.font_loads as f32];
        let mut changed = Vec::new();
        let near = dom.cull.enabled.then(|| {
            let moved = dom.cull.scrolled.get(&0).copied().unwrap_or(0.);
            [moved - height, moved + 2. * height]
        });
        let mut info = std::mem::take(&mut dom.cull.info);
        let unchanged = same
            && !relaid
            && paint.iter().all(|(id, style)| {
                let flags = cull::flags(&doc.nodes[*id].tag, style, &styles.lengths[*id], height)
                    & (cull::MOVES_CONTENT | cull::UNCULLABLE);
                info.get(*id)
                    .is_some_and(|info| info.flags & flags == flags)
            });
        let settled = unchanged && !dom.container_pass;
        const PER_FRAME: u16 = cull::CHANGED
            | cull::CHILDREN_CHANGED
            | cull::GEOMETRY_CHANGED
            | cull::COMPOSITED
            | cull::TRANSFORMED
            | cull::ANIMATED;
        let mut fresh = match dom.cull.fresh.take() {
            Some(mut fresh) => {
                for id in fresh.drain(..) {
                    if let Some(info) = info.get_mut(id) {
                        info.flags &= !PER_FRAME;
                    }
                }
                Some(fresh)
            }
            None => {
                for info in &mut info {
                    info.flags &= !PER_FRAME;
                }
                Some(Vec::new())
            }
        };
        if unchanged {
            dom.cull.painted.extend(paint.iter().map(|(id, _)| *id));
            dom.cull.painted.sort_unstable();
            dom.cull.painted.dedup();
        } else {
            walk(&doc, dom, &styles);
            let walk = std::mem::take(&mut dom.cull.walk);
            let mut marked = std::mem::take(&mut dom.cull.marked);
            let (mut partial, mut list, mut placing) = (false, Vec::new(), false);
            let rewalked = !Rc::ptr_eq(&walk.nodes, &walked);
            // Composed children can change the walk of a node without a new revision. A patched walk lists
            // those nodes, another new walk doesn't.
            let touched = walk.since(serial);
            if let Some((latest, focus, computed)) = last.as_ref().filter(|(.., computed)| {
                (!rewalked
                    || touched.is_some()
                    || styles.children.is_empty() && computed.children.is_empty())
                    && dom.cull.frame == frame
                    && info.len() <= doc.nodes.len()
                    && dom.cull.parents.len() == doc.nodes.len()
            }) {
                // New nodes have a newer revision, so they get their flags below.
                info.resize(doc.nodes.len(), cull::Info::default());
                match touched {
                    Some(touched) => {
                        for &id in touched.iter().filter(|&&id| !walk.contains(id as usize)) {
                            info[id as usize] = cull::Info::default();
                        }
                    }
                    None if rewalked => {
                        let mut present = vec![false; doc.nodes.len()];
                        for &(id, _) in walk.nodes.iter() {
                            present[id as usize] = true;
                        }
                        for &(id, _) in walked.iter() {
                            if present.get(id as usize) == Some(&false) {
                                info[id as usize] = cull::Info::default();
                            }
                        }
                    }
                    None => {}
                }
                partial = true;
                marked.resize(doc.nodes.len(), false);
                let parents = &dom.cull.parents;
                let mut mark = |mut id: usize| {
                    while !marked[id] {
                        marked[id] = true;
                        list.push(id);
                        match parents[id] {
                            u32::MAX => break,
                            parent => id = parent as usize,
                        }
                    }
                };
                // Relies on a node getting a new revision when it leaves or joins the tree.
                // A node that moved keeps its flags. One that joined has none yet.
                let changed = walk.changed_since(serial).into_iter().flatten();
                let joined = touched.into_iter().flatten();
                let joined = joined.filter(|&&id| info[id as usize].subtree == 0);
                let patched = changed.chain(joined).map(|&id| id as usize);
                let newer = doc.nodes.changed_since(*latest);
                for id in newer.chain(patched).filter(|&id| walk.contains(id)) {
                    mark(id);
                }
                for id in styles.values.differing(&computed.values).flatten() {
                    if id < doc.nodes.len()
                        && !computed
                            .values
                            .get(id)
                            .is_some_and(|old| Rc::ptr_eq(old, &styles.values[id]))
                    {
                        mark(id);
                    }
                }
                for id in [*focus, dom.focus]
                    .into_iter()
                    .flatten()
                    .chain(dom.cull.painted.iter().copied())
                    .chain(paint.iter().map(|(id, _)| *id))
                {
                    if id < doc.nodes.len() {
                        mark(id);
                    }
                }
                for &id in &list {
                    info[id] = cull::Info::default();
                }
                placing = list.len() <= 256;
                fresh.get_or_insert_default().extend(&list);
            } else {
                info.clear();
                info.resize(doc.nodes.len(), cull::Info::default());
                fresh = None;
            }
            dom.cull.painted.clear();
            dom.cull.painted.extend(paint.iter().map(|(id, _)| *id));
            // Compare units too: equal style values can resolve differently after a root font change.
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
            let mut memo = rustc_hash::FxHashMap::default();
            let mut own_flags = |tag: &str, style: &Rc<style::Style>, id: usize| {
                let kind = if tag.starts_with('#') {
                    1
                } else {
                    u8::from(cull::replaced(tag)) * 2
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
                info[*id].flags = own_flags(&doc.nodes[*id].tag, style, *id)
                    & (cull::MOVES_CONTENT | cull::UNCULLABLE);
            }
            // Children come before their parents. A partial pass visits the marked nodes only.
            let mut order: Vec<(u32, usize)> = list
                .iter()
                .filter_map(|&id| Some((walk.place(id)?, id)))
                .collect();
            order.sort_unstable_by(|a, b| b.cmp(a));
            let mut some = order.iter().map(|place| place.1);
            let mut every = walk.nodes.iter().rev().map(|node| node.0 as usize);
            debug_assert!(
                !partial || some.clone().eq(every.clone().filter(|&id| marked[id])),
                "marked nodes differ"
            );
            let nodes: &mut dyn Iterator<Item = usize> = match partial {
                true => &mut some,
                false => &mut every,
            };
            for id in nodes {
                let node = &doc.nodes[id];
                let style = &styles.values[id];
                let revision = doc.nodes.revision(id);
                let old = dom.layout_nodes[id].as_ref();
                let (shape, attrs) = match old {
                    Some(old) if old.seen == revision => {
                        debug_assert!(
                            (old.shape, old.attrs) == cull::content(node),
                            "node {id} changed without a new revision"
                        );
                        (old.shape, old.attrs)
                    }
                    _ => cull::content(node),
                };
                let restyled =
                    old.is_none_or(|old| !Rc::ptr_eq(&old.style, style) || old.shape != shape);
                let own = restyled || old.is_some_and(|old| old.attrs != attrs);
                let reshaped = old.is_none_or(|old| {
                    old.shape != shape
                        || !Rc::ptr_eq(&old.style, style)
                            && (old.units != units(&styles.lengths[id])
                                || !cull::cosmetic(&old.style, style))
                });
                let (flags, marks) = match old {
                    Some(old) if !restyled => (old.flags, old.marks),
                    _ => {
                        let flags = own_flags(&node.tag, style, id);
                        let pseudos = styles.pseudos.get(&id);
                        // List every tag with unrecorded sibling, row, counter or quote dependencies.
                        let framed = matches!(
                            node.tag.as_str(),
                            "fieldset"
                                | "legend"
                                | "details"
                                | "summary"
                                | "dialog"
                                | "li"
                                | "table"
                                | "caption"
                                | "colgroup"
                                | "col"
                                | "q"
                                | "slot"
                        ) || style::table_box(style::role(&styles, id))
                            || style::role(&styles, id) == style::CAPTION
                            || pseudos.is_some_and(|pair| {
                                pair.iter().any(|pseudo| {
                                    pseudo.get(Prop::CONTENT).is_some_and(|value| {
                                        value.contains("counter") || value.contains("quote")
                                    })
                                })
                            });
                        let heighted =
                            pseudos.is_some_and(|pair| pair.iter().any(cull::reads_height));
                        (
                            flags,
                            (u16::from(framed) * cull::READS_LAST_FRAME)
                                | (u16::from(heighted) * cull::READS_HEIGHT),
                        )
                    }
                };
                let (mut list, mut kids) = (
                    rustc_hash::FxHasher::default(),
                    rustc_hash::FxHasher::default(),
                );
                let (mut out, mut moved, mut below) = (0u64, 0u64, 0u16);
                for &child in &node.children {
                    let child_info = &info[child];
                    below |= child_info.flags;
                    if child_info.flags & cull::OUT_OF_FLOW != 0 {
                        out = out.wrapping_add(child_info.subtree);
                    } else {
                        std::hash::Hasher::write_usize(&mut list, child);
                        std::hash::Hasher::write_usize(&mut kids, child);
                        std::hash::Hasher::write_u64(&mut kids, child_info.subtree);
                        moved = moved.max(child_info.revision);
                    }
                }
                let (list, kids) = (
                    std::hash::Hasher::finish(&list),
                    std::hash::Hasher::finish(&kids),
                );
                let repainted = placing
                    && restyled
                    && !reshaped
                    && old.is_some_and(|old| {
                        old.attrs == attrs
                            && old.out == out
                            && old.kids == kids
                            && (old.flags, old.marks) == (flags, marks)
                    })
                    && self.place(
                        &doc,
                        &styles,
                        &mut dom.measurements.texts,
                        &dom.bounds,
                        tree,
                        near,
                        id,
                    );
                let mut relisted = false;
                if own && !repainted || old.is_some_and(|old| old.out != out || old.kids != kids) {
                    relisted = old.is_none_or(|old| old.list != list);
                    let geometry = old
                        .filter(|old| !reshaped && !relisted && moved <= old.geometry)
                        .map(|old| old.geometry);
                    dom.layout_revision =
                        LAYOUT_REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    dom.layout_nodes[id] = Some(crate::dom::LayoutNode {
                        shape,
                        attrs,
                        seen: revision,
                        style: style.clone(),
                        units: units(&styles.lengths[id]),
                        list,
                        kids,
                        out,
                        revision: dom.layout_revision,
                        geometry: geometry.unwrap_or(dom.layout_revision),
                        flags,
                        marks,
                    });
                    changed.push(id);
                } else if let Some(old) = &mut dom.layout_nodes[id] {
                    old.seen = revision;
                    if repainted {
                        old.style = style.clone();
                    }
                }
                let layout = dom.layout_nodes[id].as_ref();
                info[id] = cull::Info {
                    revision: layout.map_or(0, |node| node.geometry),
                    subtree: layout.map_or(0, |node| node.revision),
                    flags: if flags & cull::HIDDEN != 0 {
                        flags
                    } else {
                        info[id].flags
                            | flags
                            | (u16::from(reshaped) * cull::CHANGED)
                            | (u16::from(relisted) * cull::CHILDREN_CHANGED)
                            | marks
                            | (u16::from(dom.focus == Some(id)) * cull::UNCULLABLE)
                            | below
                                & (cull::UNCULLABLE
                                    | cull::READS_VIEWPORT
                                    | cull::READS_LAST_FRAME
                                    | cull::READS_HEIGHT)
                    },
                };
            }
            dom.cull.walk = walk;
            for id in list {
                marked[id] = false;
            }
            dom.cull.marked = marked;
        }
        self.composited_transforms.borrow_mut().clear();
        self.promote.set(
            (!composited.is_empty()).then(|| (now + std::time::Duration::from_millis(300), false)),
        );
        for &(id, turns) in &composited {
            info[id].flags |= cull::COMPOSITED | (u16::from(turns) * cull::TRANSFORMED);
            fresh.iter_mut().for_each(|fresh| fresh.push(id));
        }
        let mut animate = |info: &mut [cull::Info], id: usize| {
            let mut node = Some(id);
            while let Some(id) = node.filter(|&id| id < info.len()) {
                if info[id].flags & cull::ANIMATED != 0 {
                    break;
                }
                info[id].flags |= cull::ANIMATED;
                fresh.iter_mut().for_each(|fresh| fresh.push(id));
                node = doc.nodes[id].parent;
            }
        };
        for &(id, _) in &composited {
            animate(&mut info, id);
        }
        let mut placed = std::mem::take(&mut *self.animated.borrow_mut());
        placed.retain(|&id| {
            id < doc.nodes.len()
                && painting(&paint, id).is_none()
                && self.place(
                    &doc,
                    &styles,
                    &mut dom.measurements.texts,
                    &dom.bounds,
                    tree,
                    near,
                    id,
                )
        });
        for (id, _) in &paint {
            if self.place(
                &doc,
                &styles,
                &mut dom.measurements.texts,
                &dom.bounds,
                tree,
                near,
                *id,
            ) {
                self.animated.borrow_mut().push(*id);
            } else {
                animate(&mut info, *id);
            }
        }
        placed.extend(self.animated.borrow().iter());
        for &(id, _) in &restore_pseudos {
            animate(&mut info, id);
        }
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
        if !same || !restore.is_empty() {
            dom.blocks
                .retain(|&id, _| shown(id) && effects::establishes(&styles.values[id]).absolute);
        }
        recolored.extend(&dom.recolored);
        placed.sort_unstable();
        recolored.retain(|id| placed.binary_search(id).is_err());
        for &id in &recolored {
            animate(&mut info, id);
        }
        for id in recolored {
            let mut node = Some(id).filter(|&id| id < styles.values.len());
            while let Some(id) = node {
                dom.measurements.texts.remove(&id);
                node = doc.nodes[id].parent.filter(|_| {
                    styles.values[id]
                        .get(Prop::DISPLAY)
                        .is_none_or(|value| matches!(value, "inline" | "contents"))
                });
            }
        }
        if dom.container_pass && dom.style_cache.units || dom.cull.frame != frame {
            dom.measurements.sizes.clear();
            dom.grids.clear();
            self.walked.borrow_mut().clear();
            self.adjoined.borrow_mut().clear();
        }
        let mut stale = false;
        let mut around = rustc_hash::FxHashSet::default();
        for &id in &changed {
            dom.measurements.texts.remove(&id);
            if dom.layout_nodes[id]
                .as_ref()
                .is_some_and(|node| node.geometry == node.revision)
            {
                let mut node = Some(id);
                while let Some(id) = node.filter(|&id| around.insert(id)) {
                    dom.grids.remove(&id);
                    info[id].flags |= cull::GEOMETRY_CHANGED;
                    fresh.iter_mut().for_each(|fresh| fresh.push(id));
                    node = crate::dom::flat_parent(&doc, dom, &styles, id);
                }
                stale = true;
            }
        }
        if stale {
            dom.measurements.sizes.retain(|&(id, ..), _| {
                info.get(id)
                    .is_none_or(|info| info.flags & cull::GEOMETRY_CHANGED == 0)
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
                if name.contains(':') {
                    if dom.warned.insert(name.clone()) {
                        warnings.push(format!(
                            "Unsupported CSS `{name}`: the value is not supported."
                        ));
                    }
                } else if dom.warned.insert(format!("@{name}")) {
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
        let page_scroll = dom.scroll.entry(0).or_default().clone();
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
        let page_overscroll = html.map_or_else(Default::default, |html| {
            element::overscroll(&styles.values[html])
        });
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
            dom.cull.scrolled.clear();
            for (&id, handle) in &dom.scroll {
                let offset = handle.offset();
                let last = dom.cull.offsets.insert(id, offset).unwrap_or(offset);
                moved += f32::from((offset.x - last.x).abs() + (offset.y - last.y).abs());
                if offset.y != last.y {
                    dom.cull.scrolled.insert(id, f32::from(last.y - offset.y));
                }
            }
            dom.cull.resized = dom.cull.frame[..2] != frame[..2];
            dom.cull.enabled =
                !std::mem::take(&mut dom.cull.off) && dom.cull.frame[2..] == frame[2..];
            if moved >= height / 2. {
                self.kept.borrow_mut().retain(|_, kept| !kept.culled);
            }
            dom.cull.frame = frame;
        }
        let zone = dom.cull.enabled.then(|| {
            let moved = dom.cull.scrolled.get(&0).copied().unwrap_or(0.);
            Rc::new(cull::Zone {
                top: moved,
                bottom: height + moved,
                clips: Vec::new(),
                resized: dom.cull.resized,
            })
        });
        let mut left = Vec::new();
        let top_layer: Vec<usize> = dom
            .top_layer
            .iter()
            .copied()
            .filter(|&id| {
                let state = doc.nodes[id].state;
                let kept = state & crate::dom::TOP_LAYER != 0
                    || visible.get(id) == Some(&true)
                        && styles.values[id].get(Prop::OVERLAY) == Some("auto");
                if !kept && state & crate::dom::LEAVING != 0 {
                    left.push(id);
                }
                kept && visible.get(id) == Some(&true) && doc.nodes[id].tag != "html"
            })
            .collect();
        dom.measurements.pixel_ratio = window.scale_factor();
        let measured = RefCell::new(std::mem::take(&mut dom.measurements));
        let walk = dom.style_walk.nodes.clone();
        for text in warnings {
            host.log(serde_json::json!({"method":"Runtime.consoleAPICalled","params":{"type":"error","text":text,"context":crate::host::Role::Renderer(self.id)}}));
        }
        drop(host);
        let text_system = window.text_system().clone();
        let ids = {
            let mut ids = self.ids.borrow_mut();
            if ids.0 != doc.nodes.latest() {
                *ids = (doc.nodes.latest(), Rc::new(effects::Ids::of(walk)));
            }
            ids.1.clone()
        };
        let reads = RefCell::new(std::mem::take(&mut *self.reads.borrow_mut()));
        let mut build = Build {
            doc: &doc,
            styles: &styles,
            text: &text_system,
            pixel_ratio: window.scale_factor(),
            view: &self.0,
            ancestors: Rc::default(),
            flex_parent: false,
            measured: &measured,
            perspective: None,
            siblings: &RefCell::default(),
            walks: &RefCell::default(),
            adjoins: &RefCell::default(),
            blocks: &RefCell::default(),
            ids: &ids,
            info: &info,
            zone,
            columned: false,
            top_layer: &top_layer,
            tree,
            culls: &Cell::new(0),
            resized: false,
            settled: false,
            reads: &reads,
            epoch: 0,
            #[cfg(debug_assertions)]
            prints: &RefCell::default(),
            #[cfg(debug_assertions)]
            checking: false,
            #[cfg(debug_assertions)]
            extra: Default::default(),
            #[cfg(debug_assertions)]
            plan: Default::default(),
        };
        {
            let mut kept = self.kept_frame.borrow_mut();
            if kept.0[2..] != frame[2..] || kept.1 != top_layer {
                kept.1 = top_layer.clone();
                self.kept.borrow_mut().clear();
                self.plans.borrow_mut().clear();
                self.shared.borrow_mut().clear();
            }
            {
                let mut records = self.kept.borrow_mut();
                if records.is_empty() {
                    records.reserve(doc.nodes.len().min(1 << 20) / 2);
                }
            }
            let swapped = !restore.is_empty() || !restore_pseudos.is_empty() || !lengths.is_empty();
            let (epoch, last) = self.epoch.get();
            build.epoch = epoch + u64::from(!same || swapped || last || kept.0 != frame);
            self.epoch.set((build.epoch, swapped));
            build.resized = kept.0[..2] != frame[..2];
            build.settled =
                settled && !build.resized && !window.redrawing() && !self.reflowing.get();
            kept.0 = frame;
        }
        #[cfg(debug_assertions)]
        let zone = build.zone.clone();
        let root = element(
            &build,
            0,
            [width - page_space[1], height],
            [width - page_space[1], height],
            None,
        );
        build.zone = None;
        let mut overlay: Vec<renderer::Child> = Vec::new();
        for &id in &top_layer {
            if let Some(backdrop) = backdrops
                .iter()
                .find_map(|(node, style)| (*node == id).then_some(style))
                .or_else(|| styles.backdrops.get(&id))
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
                        el = el.bg(crate::style::rgba((color, alpha)));
                    }
                    if backdrop.get(Prop::OPACITY).is_some() {
                        el = el.opacity(effects::opacity(backdrop));
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
                            [[0.; 4]; 2],
                            self.0.clone(),
                            host.dom.get(&self.id).and_then(|state| state.base.as_ref()),
                            &host.assets,
                            false,
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
                                    bounds: renderer::Rect::default(),
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
        if !same {
            let changed: Vec<usize> = self
                .patches
                .borrow()
                .iter()
                .filter(|&(&id, patch)| {
                    patch.value != element::hashed(&doc.nodes[id].value) && tree.contains(id)
                })
                .map(|(&id, _)| id)
                .collect();
            for id in changed {
                let Some(patch) = self.patches.borrow_mut().remove(&id) else {
                    continue;
                };
                let old = self.kept.borrow_mut().remove(&id);
                let mark = reads.borrow().len();
                let field = Build {
                    ancestors: patch.ancestors,
                    flex_parent: patch.flex_parent,
                    columned: patch.columned,
                    perspective: patch.perspective,
                    zone: None,
                    #[cfg(debug_assertions)]
                    extra: Default::default(),
                    #[cfg(debug_assertions)]
                    plan: Default::default(),
                    ..build
                };
                #[cfg(debug_assertions)]
                let printed = field.prints.borrow().len();
                let bounds = patch.slot.as_ref().map(|(bounds, _)| *bounds);
                let child = element(&field, id, patch.containing, patch.container, patch.slot);
                let child = match bounds {
                    Some(bounds) => {
                        let mut units = styles.lengths[id];
                        units.container = Some(patch.container);
                        let margin =
                            units.edges(&styles.values[id], style::MARGIN, patch.containing[0]);
                        element::place(child, margin, field.pixel_ratio, bounds)
                    }
                    None => child,
                };
                #[cfg(debug_assertions)]
                {
                    if let (Some(old), Some(new)) = (&old, self.kept.borrow().get(&id)) {
                        assert!(
                            old.call == new.call
                                && old.reads == new.reads
                                && old.print == new.print,
                            "patched field {id} builds otherwise"
                        );
                    }
                    field.prints.borrow_mut().truncate(printed);
                }
                #[cfg(not(debug_assertions))]
                drop(old);
                reads.borrow_mut().truncate(mark);
                if let renderer::Child::Node(node) = child {
                    window.patch(*node);
                }
            }
        }
        // Build in place from the outside in, so a box already built inside another one's build is skipped.
        let mut places: Vec<(usize, usize)> = {
            let (kept, built) = (self.kept.borrow(), self.built.take());
            self.repaint
                .borrow()
                .iter()
                .filter(|&&id| tree.contains(id))
                .filter_map(|&id| {
                    let ancestors = kept.get(&id)?.in_place()?;
                    (!ancestors
                        .last()
                        .is_some_and(|parent| built.contains(parent)))
                    .then_some((ancestors.len(), id))
                })
                .collect()
        };
        places.sort_unstable();
        let mut placed = Vec::new();
        for (_, id) in places {
            if !self.repaint.borrow().contains(&id) {
                continue;
            }
            if let Some(node) = element::repaint(&build, id) {
                placed.push(node);
            }
        }
        self.repaint.borrow_mut().clear();
        self.built.borrow_mut().clear();
        #[cfg(debug_assertions)]
        if !placed.is_empty() {
            let records = std::mem::take(&mut *self.kept.borrow_mut());
            let (printed, mark) = (build.prints.borrow().len(), reads.borrow().len());
            let culls = Cell::new(0);
            let full = Build {
                ancestors: build.ancestors.clone(),
                perspective: build.perspective.clone(),
                zone: zone.clone(),
                culls: &culls,
                checking: true,
                extra: Default::default(),
                plan: Default::default(),
                ..build
            };
            let fresh = element(
                &full,
                0,
                [width - page_space[1], height],
                [width - page_space[1], height],
                None,
            );
            build.prints.borrow_mut().truncate(printed);
            reads.borrow_mut().truncate(mark);
            element::same_in_place(&fresh, &placed, tree);
            drop(records);
        }
        #[cfg(debug_assertions)]
        self.around.borrow_mut().clear();
        for node in placed {
            window.patch(node);
        }
        drop(build);
        let mut reads = reads.into_inner();
        reads.clear();
        *self.reads.borrow_mut() = reads;
        for (node, children) in children.into_iter().rev() {
            doc.nodes.compose_children(node, children);
        }
        let repass = {
            let mut host = self.host.borrow_mut();
            let dom = host.dom.entry(self.id).or_default();
            #[cfg(debug_assertions)]
            if let Some(fresh) = &fresh {
                let fresh: rustc_hash::FxHashSet<usize> = fresh.iter().copied().collect();
                assert!(
                    info.iter()
                        .enumerate()
                        .all(|(id, info)| info.flags & PER_FRAME == 0 || fresh.contains(&id)),
                    "per-frame flags outside `fresh`"
                );
            }
            dom.cull.info = info;
            dom.cull.fresh = fresh;
            dom.cull.visible = visible;
            dom.measurements = measured.into_inner();
            dom.animating.clear();
            dom.animating_pseudos.clear();
            let restored =
                !restore.is_empty() || !restore_pseudos.is_empty() || !lengths.is_empty();
            if restored {
                let styles = Rc::make_mut(&mut styles);
                for (id, old) in lengths {
                    styles.lengths.set(id, old);
                }
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
            dom.cull.seen = (!relaid).then(|| (seen.0, seen.1, styles.clone()));
            if restored {
                dom.computed = Some(styles);
            }
            if !left.is_empty() {
                dom.top_layer.retain(|id| !left.contains(id));
                for id in left {
                    if let Some(node) = doc.nodes.edit(id) {
                        node.state &= !crate::dom::LEAVING;
                    }
                }
                dom.invalidate();
            }
            // Allow at most one extra pass per frame to bound container-query resize loops.
            let changed = dom.update_containers(&doc);
            let repass = changed && !dom.container_pass;
            dom.container_pass = repass;
            if let Some(page) = host.pages.get_mut(&self.id) {
                page.doc = doc;
            }
            repass
        };
        if repass {
            window.rebuild();
        }
        if !events.is_empty() {
            if self.reflowing.get() {
                let view = self.0.clone();
                cx.defer(move |cx| animation_events(&view, events, cx));
            } else {
                animation_events(&self.0, events, cx);
                window.restart();
            }
        }
        let id = self.id;
        if !window.redrawing() && !self.reflowing.get() && !repass {
            queue_layout(self.0.clone(), window);
        }
        let page = renderer::node().id(("apps-page", 0)).size_full();
        let mut page = if page_scrolls {
            page.overflow_y_scroll()
        } else {
            page.overflow_y_hidden()
        };
        page.style().overscroll = page_overscroll;
        let page = page
            .pr(renderer::px(page_space[1]))
            .restrict_scroll_to_axis()
            .track_scroll(&page_scroll)
            .bg(Color::hex((background.0 << 8) | background.1 as u32))
            .on_pointer({
                let view = self.0.clone();
                move |event, window, _| {
                    if let Event::PointerMove { held, .. } = event
                        && view.moving.take()
                        && *held == Some(renderer::Button::Left)
                    {
                        window.control().start_window_move();
                    }
                }
            })
            .child(pointer_events)
            .child(root)
            .child({
                // Refresh the layout mirror after prepaint: scroll ranges change after the initial
                // sync_scroll.
                let data = self.0.clone();
                renderer::canvas(
                    move |_, _| {
                        if let Ok(mut host) = data.host.try_borrow_mut()
                            && let Some(dom) = host.dom.get_mut(&data.id)
                        {
                            dom.sync_scroll();
                        }
                    },
                    |_, _, _| (),
                )
                .absolute()
            })
            .children(page_bar.map(|look| scrollbar::Scrollbar {
                id: 0,
                handle: page_scroll.clone(),
                look: Some(look),
                axes: [false, true],
                reserved: page_space,
                borders: [0.; 4],
                origin: [0.; 2],
                quiet: false,
            }))
            .children(crate::desktop::mcp::profiling().then(|| {
                renderer::canvas(
                    |_, _| (),
                    move |_, _, _| crate::desktop::mcp::frame(id, started),
                )
                .absolute()
            }))
            .children({
                let inspect = self.inspect.get();
                (inspect.hover.is_some() || inspect.selected.is_some()).then(|| {
                    let data = self.0.clone();
                    renderer::canvas(|_, _| (), move |_, _, window| inspect::paint(&data, window))
                        .absolute()
                })
            })
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

const BUTTONS: [u8; 5] = [1, 4, 2, 8, 16];
const TRANSFORM_PROPS: [Prop; 4] = [Prop::TRANSFORM, Prop::TRANSLATE, Prop::ROTATE, Prop::SCALE];
// Must list every property that `effects::opacity` and `effects::transform` read.
const COMPOSITED_PROPS: [Prop; 6] = [
    Prop::OPACITY,
    Prop::TRANSFORM,
    Prop::TRANSLATE,
    Prop::ROTATE,
    Prop::SCALE,
    Prop::TRANSFORM_ORIGIN,
];

fn composites(a: &style::Style, b: &style::Style) -> bool {
    let rest =
        |(prop, _): &(Prop, &Rc<str>)| *prop != Prop::OPACITY && !TRANSFORM_PROPS.contains(prop);
    a.iter().filter(rest).eq(b.iter().filter(rest))
}

fn animate(
    view: &Rc<PageState>,
    dom: &mut crate::dom::State,
    next: Option<std::time::Instant>,
    now: std::time::Instant,
    composite: bool,
    window: &mut Window,
    cx: &mut App,
) {
    match next {
        Some(at) if at <= now + std::time::Duration::from_millis(8) => {
            dom.motion.wake = None;
            let view = view.clone();
            window.next_frame(move |window, cx| match composite {
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
}

// Animation handlers can hide or remove a popup, so run them before its final frame is painted.
fn animation_events(
    view: &Rc<PageState>,
    events: Vec<(usize, &'static str, String, f64)>,
    cx: &mut App,
) {
    let runtime = view.runtime.clone();
    let renderer = runtime
        .borrow()
        .views
        .get(&view.id)
        .and_then(|view| view.renderer.clone());
    for (node, kind, name, elapsed) in events {
        let (pseudo, name) = name.split_once('\0').unwrap_or(("", &name));
        let data = if kind.starts_with("animation") {
            serde_json::json!({"animationName": name, "elapsedTime": elapsed, "pseudoElement": pseudo})
        } else {
            serde_json::json!({"propertyName": name, "elapsedTime": elapsed, "pseudoElement": pseudo})
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
}

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
    let composited_transforms = view.composited_transforms.borrow();
    let mut changes = Vec::new();
    let eligible = dom.motion.custom.is_empty()
        && dom.motion.pseudos.is_empty()
        && dom.motion.backdrops.is_empty()
        && animated.len() == dom.animating.len()
        && animated.iter().all(|(id, style, layout)| {
            let Some(old) = dom.animating.get(id) else {
                return false;
            };
            if Rc::ptr_eq(old, style) {
                return true;
            }
            let turns = TRANSFORM_PROPS
                .iter()
                .any(|&prop| old.get(prop) != style.get(prop));
            let matrix = match composited_transforms.get(id).filter(|_| turns) {
                Some((bounds, perspective)) => Some(
                    effects::transform(style, &styles.lengths[*id], *bounds, *perspective)
                        .unwrap_or(effects::IDENTITY),
                ),
                None if turns => return false,
                None => None,
            };
            changes.push((*id, effects::opacity(style), matrix));
            !layout && composites(old, style)
        });
    let renew = now + std::time::Duration::from_millis(1750);
    let mut fresh = false;
    let promoted = eligible
        && match view.promote.get() {
            Some((at, on)) if now < at => on,
            Some(_) => {
                fresh = true;
                let mut looped = true;
                let mut shared = rustc_hash::FxHashMap::default();
                let on = animated.iter().all(|(id, ..)| {
                    let base = &styles.values[*id];
                    let transformed = composited_transforms.get(id);
                    let origin = transformed
                        .filter(|(_, perspective)| perspective.is_none())
                        .map(|(bounds, ..)| bounds.origin);
                    let mut timing = (1. / 120., 240, 1., now, None);
                    match dom.motion.period(*id, &styles, now) {
                        Some((period, phase, left, shape)) => {
                            let count = (period * 120.).round().max(2.);
                            let step = period / count;
                            timing = (step, count as u32, left, now, None);
                            if let Some(shape) = shape.filter(|_| left >= 2. - phase / period)
                                && let Some(start) =
                                    now.checked_sub(std::time::Duration::from_secs_f64(phase))
                            {
                                let values = COMPOSITED_PROPS.map(|prop| base.get(prop));
                                let units = &styles.lengths[*id];
                                let size = transformed.map(|(bounds, perspective)| match origin {
                                    Some(_) => format!("{:?}", bounds.size),
                                    None => format!("{bounds:?}{perspective:?}"),
                                });
                                let key = format!("{shape}{values:?}{units:?}{size:?}");
                                timing =
                                    (step, count as u32, left + phase / period, start, Some(key));
                            }
                        }
                        None => looped = false,
                    }
                    let (step, count, repeat, start, key) = timing;
                    let first = match key {
                        Some(_) => {
                            start + std::time::Duration::from_secs_f64(step * f64::from(count))
                        }
                        None => start,
                    };
                    let local = key.is_some() && origin.is_some();
                    let sample = || {
                        let times: Vec<_> = (0..=count)
                            .map(|k| {
                                first + std::time::Duration::from_secs_f64(f64::from(k) * step)
                            })
                            .collect();
                        let timeline = dom.motion.timeline(*id, &styles, &times, &COMPOSITED_PROPS);
                        let opacities: Vec<f32> = timeline.iter().map(effects::opacity).collect();
                        let transforms: Option<Vec<_>> =
                            transformed.map(|(bounds, perspective)| {
                                let bounds = match local {
                                    true => renderer::Rect::new(Default::default(), bounds.size),
                                    false => *bounds,
                                };
                                timeline
                                    .iter()
                                    .map(|style| {
                                        effects::transform(
                                            style,
                                            &styles.lengths[*id],
                                            bounds,
                                            *perspective,
                                        )
                                        .unwrap_or(effects::IDENTITY)
                                    })
                                    .collect()
                            });
                        Rc::new((opacities, transforms))
                    };
                    let mut samples = match key {
                        Some(key) => shared.entry(key).or_insert_with(sample).clone(),
                        None => sample(),
                    };
                    if let Some(origin) = origin.filter(|_| local) {
                        let [x, y] = [f32::from(origin.x), f32::from(origin.y)];
                        let (there, back) =
                            (effects::translate(x, y, 0.), effects::translate(-x, -y, 0.));
                        let moved = samples.1.as_ref().map(|list: &Vec<effects::Matrix>| {
                            list.iter()
                                .map(|matrix| {
                                    effects::multiply(&there, &effects::multiply(matrix, &back))
                                })
                                .collect()
                        });
                        samples = Rc::new((samples.0.clone(), moved));
                    }
                    window.promote(
                        *id as u64,
                        start,
                        step as f32,
                        repeat as f32,
                        &samples.0,
                        samples.1.as_deref(),
                    ) || samples.1.is_none() && !shown(*id)
                });
                // Demote all layers if any promotion fails, or previously promoted layers stop with their
                // timeline.
                if !on {
                    window.demote();
                }
                let until = match looped {
                    true => now + std::time::Duration::from_secs(86400),
                    false => renew,
                };
                view.promote.set(on.then_some((until, true)));
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
    if patched
        && composited_transforms
            .keys()
            .any(|id| dom.animating.contains_key(id))
    {
        dom.bounds.set_moved();
    }
    drop((flat, composited_transforms));
    if patched {
        dom.animating
            .extend(animated.into_iter().map(|(id, style, _)| (id, style)));
    }
    let next = match view.promote.get() {
        Some((until, true)) if promoted => {
            let event = dom
                .animating
                .keys()
                .filter_map(|&id| dom.motion.next_event(id, &styles, now))
                .min()
                .map(|event| event.max(now + std::time::Duration::from_millis(16)));
            Some(event.map_or(until, |event| event.min(until)))
        }
        _ => next.filter(|_| patched),
    };
    animate(&view, dom, next, now, true, window, cx);
    drop(host);
    if patched && !promoted && crate::desktop::mcp::profiling() {
        crate::desktop::mcp::frame(view.id, started);
    }
    if !patched || (!promoted || fresh) && !window.present() {
        window.refresh();
    }
    // Handlers can change the page, and `Desktop::drive` can't refresh the window it runs in, so refresh here.
    if !events.is_empty() {
        animation_events(&view, events, cx);
        if view
            .host
            .try_borrow()
            .is_ok_and(|host| host.pages.get(&view.id).is_some_and(|page| page.dirty))
        {
            window.refresh();
        }
    }
}

fn in_view(
    bounds: &crate::dom::LayoutMirror,
    [width, height]: [f32; 2],
    id: usize,
    mut clips: impl Iterator<Item = (usize, [bool; 2])>,
) -> bool {
    let Some(&[x, y, w, h]) = bounds.get(&id).filter(|rect| rect[2] > 0. && rect[3] > 0.) else {
        return true;
    };
    let pad = w.max(h) / 2.;
    let [left, top, right, bottom] = [x - pad, y - pad, x + w + pad, y + h + pad];
    let overlaps = |[a, b, c, d]: [f32; 4], [horizontal, vertical]: [bool; 2]| {
        (!horizontal || left.max(a) < right.min(a + c))
            && (!vertical || top.max(b) < bottom.min(b + d))
    };
    overlaps([0., 0., width, height], [true; 2])
        && clips.all(|(clip, axes)| bounds.get(&clip).is_none_or(|&rect| overlaps(rect, axes)))
}

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

// Returns whether the walk changed by a patch that kept the lists of cells, canvases and iframes.
fn walk(
    doc: &crate::dom::Document,
    dom: &mut crate::dom::State,
    styles: &style::ComputedStyles,
) -> bool {
    let serial = dom.cull.walk.serial;
    let cull = &mut dom.cull;
    if !cull
        .walk
        .update(doc, &styles.children, Some(&dom.style_walk))
    {
        if cull.parents.len() < doc.nodes.len() {
            cull.parents.resize(doc.nodes.len(), u32::MAX);
            cull.kinds.resize(doc.nodes.len(), 0);
        }
        return false;
    }
    let kind = |id: usize| match doc.nodes[id].tag.as_str() {
        // Rows show when anything in them shows, as an anonymous cell can hold text or a block.
        _ if style::role(styles, id) == style::CELL
            || doc.nodes[id].parent.is_some_and(|row| {
                (style::ROW..=style::FOOTER).contains(&style::role(styles, row))
            }) =>
        {
            1
        }
        "canvas" => 2,
        "iframe" => 3,
        _ => 0,
    };
    let patched = cull.parents.len() == cull.kinds.len()
        && cull.kinds.len() <= doc.nodes.len()
        && cull.walk.since(serial).is_some_and(|touched| {
            touched.iter().all(|&id| {
                let id = id as usize;
                cull.kinds.get(id).is_none_or(|&old| old == 0)
                    && (!cull.walk.contains(id) || kind(id) == 0)
            })
        });
    if patched {
        cull.parents.resize(doc.nodes.len(), u32::MAX);
        cull.kinds.resize(doc.nodes.len(), 0);
        for &id in cull.walk.since(serial).into_iter().flatten() {
            cull.parents[id as usize] = cull.walk.parent(id as usize).unwrap_or(u32::MAX);
        }
    }
    if patched && !cfg!(debug_assertions) {
        return true;
    }
    let mut parents = vec![u32::MAX; doc.nodes.len()];
    let mut kinds = vec![0; doc.nodes.len()];
    let mut lists = [const { Vec::new() }; 3];
    for &(id, parent) in cull.walk.nodes.iter() {
        let id = id as usize;
        parents[id] = parent;
        kinds[id] = kind(id);
        if kinds[id] != 0 {
            lists[usize::from(kinds[id]) - 1].push(id);
        }
    }
    let [cells, canvases, iframes] = lists;
    assert!(
        !patched
            || (&parents, &kinds, &cells, &canvases, &iframes)
                == (
                    &cull.parents,
                    &cull.kinds,
                    &cull.cells,
                    &cull.canvases,
                    &cull.iframes
                ),
        "patched walk lists differ"
    );
    (cull.parents, cull.kinds) = (parents, kinds);
    (cull.cells, cull.canvases, cull.iframes) = (cells, canvases, iframes);
    patched
}

fn visibility(
    doc: &crate::dom::Document,
    dom: &crate::dom::State,
    styles: &style::ComputedStyles,
) -> Vec<bool> {
    let mut visible = vec![false; doc.nodes.len()];
    for &(node, parent) in dom.cull.walk.nodes.iter() {
        let node = node as usize;
        if parent != u32::MAX && !visible[parent as usize] {
            continue;
        }
        let style = &styles.values[node];
        visible[node] = !match dom.layout_nodes.get(node).and_then(Option::as_ref) {
            Some(old) if Rc::ptr_eq(&old.style, style) => old.flags & cull::HIDDEN != 0,
            _ => style
                .get(Prop::DISPLAY)
                .is_some_and(|value| value == "none"),
        };
    }
    for &node in &dom.cull.cells {
        if visible[node] {
            let mut parent = doc.nodes[node].parent;
            while let Some(id) = parent.filter(|&id| {
                !visible[id] && (style::ROW..=style::FOOTER).contains(&style::role(styles, id))
            }) {
                visible[id] = true;
                parent = doc.nodes[id].parent;
            }
        }
    }
    visible
}

fn drop_hidden_images(
    dom: &mut crate::dom::State,
    visible: &[bool],
    styles: &style::ComputedStyles,
    mut release: impl FnMut(Arc<renderer::RenderImage>),
) {
    dom.svgs.retain(|id, (_, image)| {
        visible.get(*id) == Some(&true) || {
            if Arc::strong_count(image) == 1 {
                release(image.clone());
            }
            false
        }
    });
    dom.masks.retain(|id, (_, image)| {
        visible.get(*id) == Some(&true) || {
            if let Some(image) = image {
                release(image.clone());
            }
            false
        }
    });
    for (id, canvas) in &dom.canvases {
        if canvas.present && visible.get(*id) != Some(&true) {
            release(canvas.image.clone());
        }
    }
    // Control rasters use layer usize::MAX minus control kind.
    dom.backgrounds.retain(|&(id, layer), (_, image)| {
        let keep = if layer > usize::MAX - 64 {
            visible.get(id) == Some(&true)
                && match layer.checked_sub(background::BORDER_IMAGE) {
                    Some(slot) if slot < 8 => match slot {
                        0 => styles.values.get(id).map(|style| &**style),
                        _ => styles
                            .pseudos
                            .get(&id)
                            .and_then(|pseudos| pseudos.get(slot - 1)),
                    }
                    .and_then(|style| style.get(Prop::BORDER_IMAGE_SOURCE))
                    .is_some_and(|value| value != "none"),
                    _ => true,
                }
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
    dom.bitmaps.retain(|_, image| {
        Arc::strong_count(image) > 1 || {
            release(image.clone());
            false
        }
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
        let styles = crate::dom::styles(&mut doc, &mut Default::default(), [800., 600.]);
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
        let styles = crate::dom::styles(&mut doc, &mut dom, [800., 600.]);
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
        dom.bitmaps.insert("a".into(), shared);
        dom.backgrounds.insert((second, 64), ("b".into(), image(2)));
        let mut released = Vec::new();
        let mut visible = vec![false; doc.nodes.len()];
        (visible[0], visible[second]) = (true, true);
        drop_hidden_images(&mut dom, &visible, &styles, |image| released.push(image.id));
        assert_eq!(dom.canvases[&canvas].pixels.pixels()[0].red(), 255);
        assert_eq!(released.len(), 2);
        assert!(released.contains(&dom.canvases[&canvas].image.id));
        assert!(dom.backgrounds.contains_key(&(second, 0)));
        assert!(!dom.backgrounds.contains_key(&(first, 0)));
        assert!(!dom.backgrounds.contains_key(&(second, 64)));
        released.clear();
        for _ in 0..2 {
            drop_hidden_images(&mut dom, &[], &styles, |image| released.push(image.id));
        }
        assert!(dom.backgrounds.is_empty() && dom.bitmaps.is_empty());
        assert!(dom.canvases.contains_key(&canvas));
    }

    #[test]
    fn exit_transitions_keep_images_until_they_end() {
        let mut doc = crate::dom::Document::parse(
            "<style>div{transition:display 1s allow-discrete}div.off{display:none}span{display:block;mask-image:url(m.svg);background-image:url(b.png)}</style><div><img src=a.svg width=200 height=200><span></span></div>",
        );
        let mut dom = crate::dom::State::default();
        let [div, img, span] = ["div", "img", "span"]
            .map(|tag| doc.nodes.iter().position(|node| node.tag == tag).unwrap());
        let image = |size: u32| {
            Arc::new(renderer::RenderImage::new(
                size,
                size,
                vec![0; (size * size * 4) as usize],
            ))
        };
        let decoded = image(4);
        dom.svgs.insert(img, ("a.svg".into(), image(200)));
        dom.masks.insert(span, ("m.svg".into(), Some(image(8))));
        dom.backgrounds
            .insert((span, 0), ("b.png".into(), decoded.clone()));
        dom.bitmaps.insert("b.png".into(), decoded);
        let ids = |dom: &crate::dom::State| {
            [
                dom.svgs[&img].1.id,
                dom.masks[&span].1.as_ref().unwrap().id,
                dom.backgrounds[&(span, 0)].1.id,
            ]
        };
        let mut kept = ids(&dom);
        let start = std::time::Instant::now();
        let mut released = Vec::new();
        let mut frame =
            |doc: &mut crate::dom::Document, dom: &mut crate::dom::State, seconds: f64| {
                let styles = crate::dom::styles(doc, dom, [800., 600.]);
                let mut visible = vec![false; doc.nodes.len()];
                for id in 0..doc.nodes.len() {
                    visible[id] = doc.nodes[id].parent.is_none_or(|parent| visible[parent])
                        && styles.values[id].get(Prop::DISPLAY) != Some("none");
                }
                let at = start + std::time::Duration::from_secs_f64(seconds);
                dom.motion.update(
                    doc,
                    &styles,
                    &visible,
                    &|_| true,
                    at,
                    false,
                    &mut Vec::new(),
                );
                let visible = dom.motion.painted.take().unwrap_or(visible);
                drop_hidden_images(dom, &visible, &styles, |image| released.push(image.id));
                visible[span]
            };
        assert!(frame(&mut doc, &mut dom, 0.));
        doc.nodes[div].attrs.insert("class".into(), "off".into());
        dom.computed = None;
        for seconds in [0.1, 0.4, 0.7, 1.] {
            assert!(frame(&mut doc, &mut dom, seconds), "{seconds}");
            assert_eq!(ids(&dom), kept, "{seconds}");
        }
        assert!(!frame(&mut doc, &mut dom, 1.2));
        frame(&mut doc, &mut dom, 1.3);
        assert!(
            dom.svgs.is_empty()
                && dom.masks.is_empty()
                && dom.backgrounds.is_empty()
                && dom.bitmaps.is_empty()
        );
        released.sort_unstable();
        kept.sort_unstable();
        assert_eq!(released, kept);
    }
}
