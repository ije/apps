pub(crate) mod effects;
mod element;
pub mod fonts;
pub(crate) mod frame;
pub(crate) mod hit;
mod hud;
pub(crate) mod inline;
pub(crate) mod inspect;
pub mod layout;
pub mod media;
mod page;
pub(crate) mod paint;
pub mod raster;
mod view;

use crate::css;
use crate::css::properties::Prop;
use crate::{host::Host, style};
use element::{Build, element};
use frame::motion::{animate, animation_events};
use frame::visibility::{clips, in_view};
pub(crate) use hud::Hud;
pub(crate) use inspect::Inspect;
use layout::cull;
pub use page::PageState;
use paint::{background, field, native, scrollbar, spacer, terminal, text};
use renderer::{App, Color, Event, Window, WindowId};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

type MaskSources = Vec<(String, background::Source)>;

// The app side of the page views: it runs page scripts and their events.
pub(crate) trait Pages {
    // The page context of view `id`. It expects the desktop free, where `try_renderer` skips a busy one.
    fn renderer(&self, id: u64) -> Option<Rc<crate::host::context::JsContext>>;
    fn try_renderer(&self, id: u64) -> Option<Rc<crate::host::context::JsContext>>;
    fn dispatch(
        &self,
        id: u64,
        name: &str,
        path: &[usize],
        data: serde_json::Value,
        window: Option<&mut Window>,
        cx: &mut App,
    ) -> bool;
    fn drive(&self, cx: &mut App);
}

pub(crate) struct ViewState {
    pub(crate) id: u64,
    queued: Cell<bool>,
    pub(crate) runtime: Rc<dyn Pages>,
    pub(crate) host: Rc<RefCell<Host>>,
    images: RefCell<HashMap<usize, Arc<renderer::RenderImage>>>,
    pub(crate) frames: RefCell<HashMap<usize, Frame>>,
    controls: RefCell<native::Controls>,
    ids: RefCell<(u64, Rc<effects::Ids>)>,
    svgs: RefCell<HashMap<usize, element::svg::Svg>>,
    fields: RefCell<HashMap<usize, field::Caret>>,
    decoding: RefCell<HashMap<String, (usize, background::Decoding)>>,
    // The decoded files of each mask, so a new mask size doesn't decode them again.
    mask_sources: RefCell<rustc_hash::FxHashMap<(usize, u8), MaskSources>>,
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
    caret: RefCell<Option<text::Blink>>,
    /// The drawing of each terminal session, kept from frame to frame.
    terminals: RefCell<rustc_hash::FxHashMap<u32, terminal::Drawn>>,
    pub(crate) hud: RefCell<Option<hud::Hud>>,
    pub(crate) inspect: Cell<inspect::Inspect>,
    kept: RefCell<rustc_hash::FxHashMap<usize, element::kept::Kept>>,
    plans: RefCell<element::plan::Plans>,
    repaint: RefCell<rustc_hash::FxHashSet<usize>>,
    built: RefCell<rustc_hash::FxHashSet<usize>>,
    animated: RefCell<Vec<usize>>,
    #[cfg(debug_assertions)]
    around: RefCell<rustc_hash::FxHashSet<usize>>,
    // Retain plans used as pointer keys to prevent address reuse.
    shared: RefCell<rustc_hash::FxHashMap<u64, Vec<element::kept::Shared>>>,
    focused: Cell<Option<usize>>,
    patches: RefCell<rustc_hash::FxHashMap<usize, element::kept::Patch>>,
    // How each absolutely positioned box was last built, to build it again in place.
    outs: RefCell<rustc_hash::FxHashMap<usize, element::kept::Patch>>,
    epoch: Cell<(u64, bool)>,
    reads: RefCell<Vec<element::kept::Read>>,
    walked: RefCell<rustc_hash::FxHashMap<usize, element::Walked>>,
    adjoined: RefCell<rustc_hash::FxHashMap<usize, element::Adjoined>>,
    kept_frame: RefCell<([f32; 4], Vec<usize>)>,
    mismatched: RefCell<[Mismatches; 2]>,
    layout_queued: Cell<bool>,
    // The queued animation frame: true when it can composite, false when it must draw the page again.
    motion_frame: Cell<Option<bool>>,
    #[cfg(target_os = "macos")]
    canvas: Cell<(u32, u8)>,
    canvas_layers: Cell<Option<u64>>,
    // The content box of the canvas whose drawable content prepaints.
    drawing: Cell<renderer::Point<renderer::Pixels>>,
    // The page asked for a `paint` event after the frame.
    paint: Cell<bool>,
}

pub(crate) struct Frame {
    pub(crate) browser: apps_webview::Browser,
    loaded: String,
    pub(crate) bounds: [f64; 4],
    revision: u64,
    shown: bool,
}

type Clips = Vec<(usize, [bool; 2])>;
type Mismatches = rustc_hash::FxHashSet<(usize, [u32; 4])>;
type CompositedTransform = (renderer::Rect<renderer::Pixels>, Option<(f32, [f32; 2])>);

const BUTTONS: [u8; 5] = [1, 4, 2, 8, 16];

// Layout revisions are unique across pages and reloads, so a kept box never matches a node of another page.
static LAYOUT_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

thread_local! {
    static SCRIPTS: Cell<Option<(*mut App, Option<*mut Window>)>> = const { Cell::new(None) };
    static VIEWS: RefCell<HashMap<u64, std::rc::Weak<ViewState>>> = RefCell::default();
}

// Do not use cx or window inside f: reflow accesses them through pointers.
pub(crate) fn scripted<R>(cx: &mut App, window: Option<&mut Window>, f: impl FnOnce() -> R) -> R {
    crate::host::context::REFLOW.set(Some(reflow));
    crate::host::context::CLIPBOARD.set(Some(clipboard));
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
    renderer: &crate::host::context::JsContext,
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
    // Reflow after composite frames even if the page is clean: boxes moved without layout. User scrolls move the
    // mirror without one.
    if view.host.try_borrow_mut().map_or(true, |mut host| {
        let shifted = !host.pages.get(&id).is_some_and(|page| page.dirty)
            && host
                .page_state_mut(id)
                .is_some_and(|dom| dom.shift_scrolls());
        shifted
            || !host.pages.get(&id).is_some_and(|page| page.dirty)
                && !host
                    .page_state(id)
                    .is_some_and(|dom| dom.geometry.mirror.moved() || dom.layout.cull.reskipped)
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

fn queue_layout(view: Rc<ViewState>, window: &mut Window) {
    if view.layout_queued.replace(true) {
        return;
    }
    window.next_frame(move |window, cx| {
        view.layout_queued.set(false);
        let renderer = view.runtime.renderer(view.id);
        if let Some(renderer) = renderer
            && let Err(error) = scripted(cx, Some(window), || {
                renderer.eval("__appsLayout()", "apps:layout")
            })
        {
            eprintln!("apps: {error}");
        }
        let runtime = view.runtime.clone();
        cx.defer(move |cx| runtime.drive(cx));
    });
}

// Run animation callbacks at most once per frame, or recursive requests accumulate.
pub(crate) fn queue_animation_frame(view: Rc<ViewState>, window: &mut Window) {
    if !view.queued.replace(true) {
        window.next_frame(move |window, cx| animation_frame(view, window, cx));
    }
}

fn animation_frame(view: Rc<ViewState>, window: &mut Window, cx: &mut App) {
    view.queued.set(false);
    let renderer = view.runtime.try_renderer(view.id);
    if let Some(renderer) = renderer
        && let Err(error) = scripted(cx, Some(window), || {
            renderer.eval("__appsAnimationFrame()", "apps:frame")
        })
    {
        eprintln!("apps: {error}");
    }
    // A frame whose callbacks changed nothing draws nothing.
    if view.host.try_borrow().map_or(true, |host| {
        host.pages.get(&view.id).is_some_and(|page| page.dirty)
            || host
                .graphics
                .get(&view.id)
                .is_some_and(|gpu| gpu.presented())
    }) {
        window.refresh();
    }
    if view
        .host
        .try_borrow_mut()
        .ok()
        .and_then(|mut host| {
            host.page_state_mut(view.id)
                .map(|dom| std::mem::take(&mut dom.frame_requested))
        })
        .unwrap_or(false)
    {
        queue_animation_frame(view.clone(), window);
    }
    let runtime = view.runtime.clone();
    cx.defer(move |cx| runtime.drive(cx));
}

impl ViewState {
    #[allow(clippy::too_many_arguments)]
    fn place(
        &self,
        doc: &crate::dom::Document,
        styles: &style::ComputedStyles,
        texts: &mut rustc_hash::FxHashMap<usize, Option<Rc<crate::render::inline::InlineText>>>,
        bounds: &crate::dom::LayoutMirror,
        tree: &renderer::Tree,
        near: Option<[f32; 2]>,
        id: usize,
    ) -> Result<(), usize> {
        let kept = self.kept.borrow();
        let mut node = id;
        let place = loop {
            // An inline box that holds a block splits around it or not by its style, which its block decides. An
            // inline box that lost what gave it a box of its own goes back into the text of its block.
            if let Some(record) = kept.get(&node).filter(|_| {
                !crate::render::inline::holds_block_box(doc, styles, node)
                    && crate::render::inline::own_box(doc, styles, node)
            }) {
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
        let mut path = Some(id);
        while let Some(at) = path {
            texts.remove(&at);
            // An inline box can still be a kept box of its own, like the only box of a run, so it builds again too.
            if place.is_some() && at != node && kept.contains_key(&at) {
                self.repaint.borrow_mut().insert(at);
            }
            path = doc.nodes[at].parent.filter(|_| at != node);
        }
        place.map(|_| ()).ok_or(node)
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

pub(crate) struct PageView(pub(crate) Rc<ViewState>);

impl std::ops::Deref for PageView {
    type Target = ViewState;
    fn deref(&self) -> &ViewState {
        &self.0
    }
}

impl PageView {
    pub(crate) fn new(
        id: u64,
        runtime: Rc<dyn Pages>,
        host: Rc<RefCell<Host>>,
        window: WindowId,
    ) -> Self {
        let view = Rc::new(ViewState {
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
            mask_sources: RefCell::default(),
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
            terminals: RefCell::default(),
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
            outs: RefCell::default(),
            epoch: Cell::default(),
            reads: RefCell::default(),
            walked: RefCell::default(),
            adjoined: RefCell::default(),
            #[cfg(target_os = "macos")]
            canvas: Cell::default(),
            canvas_layers: Cell::default(),
            drawing: Cell::default(),
            paint: Cell::default(),
            kept_frame: RefCell::default(),
            mismatched: RefCell::default(),
            layout_queued: Cell::default(),
            motion_frame: Cell::default(),
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
            let state = host.state(self.id);
            state.attach_window(window);
            state.geometry.unshift();
            state.geometry.sync_scroll();
            state.geometry.mirror.touch();
            frame::changes::repaint(state, window);
        }
        let pointer_events = frame::pointer::events(self.0.clone());
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
            cx.defer(move |cx| runtime.drive(cx));
        }
        let inspectable = !host.app.packaged;
        let state = &mut *host;
        let Some(page) = state.pages.get_mut(&self.id) else {
            return renderer::node().size_full().into();
        };
        page.dirty = false;
        let mut doc = std::mem::take(&mut page.doc);
        let dom = &mut page.state;
        // Clear node-ID caches on reload or navigation: the new DOM reuses IDs. Stamp its layout revision on
        // the first frame.
        if dom.layout.layout_revision == 0 {
            window.forget_states();
            self.kept.take();
            self.patches.take();
            self.outs.take();
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
            self.terminals.take();
        }
        // The drawings of ended terminal sessions go now, with their images and timers.
        if !self.terminals.borrow().is_empty() {
            let sessions = dom.media.terminals.borrow();
            self.terminals
                .borrow_mut()
                .retain(|session, _| sessions.contains(*session));
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
        for image in dom.media.retired.drain(..) {
            window.release_image(image);
        }
        for (node, image) in dom.media.recycled.drain(..) {
            window.release_image(image.clone());
            if let Ok(image) = Arc::try_unwrap(image)
                && let Some(media) = state.media.get_mut(&self.id)
            {
                media.recycle(node as u64, image);
            }
        }
        dom.media.retired_canvases.clear();
        if std::mem::take(&mut dom.paint_requested) {
            self.paint.set(true);
        }
        let cached = dom.styles.computed.clone();
        let mut styles = dom.restyle(&mut doc, [width, height]);
        if cached.is_none_or(|cached| !std::rc::Rc::ptr_eq(&cached, &styles))
            && dom.load_fonts(&doc, &state.app.assets).0
        {
            styles = dom.restyle(&mut doc, [width, height]);
        }
        frame::visibility::wake(&self.0, &doc, dom, &styles);
        let mut children: Vec<_> = styles
            .children
            .iter()
            .map(|(&node, children)| (node, doc.nodes.compose_children(node, children.clone())))
            .collect();
        let seen = (doc.nodes.latest(), dom.interaction.focus);
        // Remove cull.seen's extra Rc during the frame so animated styles can swap values without copying.
        let last = dom.layout.cull.seen.take();
        let (walked, serial) = (dom.layout.cull.walk.snapshot(), dom.layout.cull.walk.serial);
        // Animated values that the last frame swapped in are gone from `styles`, so their nodes changed.
        let same = dom.layout.cull.swapped.is_empty()
            && last.as_ref().is_some_and(|(latest, focus, computed)| {
                (*latest, *focus) == seen && Rc::ptr_eq(computed, &styles)
            });
        let (mut visible, mut previous, flips) =
            frame::visibility::update(&doc, dom, &styles, last.as_ref(), same, &walked, serial);
        let mut events = Vec::new();
        let now = std::time::Instant::now();
        let bounds = &dom.geometry.mirror;
        let side = |id| {
            bounds
                .get(&id)
                .map_or(f32::INFINITY, |rect| rect[2].min(rect[3]))
        };
        let crate::style::Frame {
            mut animated,
            next,
            custom,
            pseudos,
            backdrops,
            mut painted,
        } = dom.styles.motion.update(
            &doc,
            &styles,
            &visible,
            &|id| in_view(bounds, [width, height], id, clips(&doc, &styles, id)),
            &side,
            now,
            dom.layout.container_pass,
            &mut events,
        );
        let mut images = HashMap::new();
        if let Some(gpu) = state.graphics.get_mut(&self.id) {
            for &id in &dom.layout.cull.canvases {
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
        if let Some(painted) = painted.take() {
            previous.get_or_insert(std::mem::replace(&mut visible, painted));
            dom.layout.cull.cascaded = false;
        }
        let flipped = previous.is_some();
        if let Some(previous) = previous {
            if walked.len() + dom.layout.cull.walk.order().len() < visible.len() {
                let walks = walked.iter().chain(dom.layout.cull.walk.order().iter());
                let nodes = walks
                    .map(|&(id, _)| id as usize)
                    .chain(previous.len()..visible.len());
                dom.geometry.mirror.hide(&visible, &previous, nodes);
            } else {
                dom.geometry
                    .mirror
                    .hide(&visible, &previous, 0..visible.len());
            }
        }
        *self.parked.borrow_mut() = dom
            .styles
            .motion
            .parked()
            .iter()
            .map(|&id| (id, clips(&doc, &styles, id).collect()))
            .collect();
        let mut lengths = frame::motion::restyle(
            &mut doc,
            dom,
            &styles,
            &mut children,
            [width, height],
            custom,
            &mut animated,
        );
        let (composited, pseudo_layers) =
            frame::motion::layers(&self.0, &doc, &styles, &animated, &pseudos);
        let compositing = !animated.is_empty()
            && composited.len() == animated.len()
            && pseudo_layers.len() == pseudos.len()
            && backdrops.is_empty();
        animate(&self.0, dom, next, now, compositing, window, cx);
        // Paint and the next frame read the cascade values, so they go back to `computed` after the frame.
        let taken = !pseudos.is_empty() || !animated.is_empty() || !lengths.is_empty();
        let (mut restore, restore_pseudos, paint) =
            frame::motion::swap(dom, &mut styles, animated, pseudos, &mut lengths);
        let painting = |paint: &[(usize, Rc<css::properties::Style>)], id: usize| {
            paint.binary_search_by_key(&id, |(id, _)| *id).ok()
        };
        let relaid = !restore.is_empty() || !lengths.is_empty();
        let frame = [width, height, window.scale_factor(), dom.fonts.loads as f32];
        let near = dom.layout.cull.enabled.then(|| {
            let moved = dom.layout.cull.scrolled.get(&0).copied().unwrap_or(0.);
            [moved - height, moved + 2. * height]
        });
        let fcx = frame::Cx {
            view: &self.0,
            doc: &doc,
            tree,
            frame,
            near,
            last: last.as_ref(),
            walked: &walked,
            serial,
            same,
            visible: &visible,
            composited: &composited,
            pseudo_layers: &pseudo_layers,
            restore_pseudos: &restore_pseudos,
        };
        frame::visibility::forget(&fcx, dom, &styles, flips, flipped, window);
        native::sync(&self.controls, &doc, &styles.values);
        let iframes = &dom.layout.cull.iframes;
        let frame_nodes = || iframes.iter().copied().filter(|&id| visible[id]);
        let mut frames = self.frames.borrow_mut();
        frames.retain(|id, _| frame_nodes().any(|node| node == *id));
        for node in frame_nodes() {
            let (attrs, revision) = (&doc.nodes[node].attrs, doc.nodes.revision(node));
            let frame = match frames.entry(node) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    match apps_webview::Browser::new(
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
                .and_then(css::value::color::color)
                .filter(|color| color.1 > 0)
        };
        let dark = html
            .and_then(|id| styles.values[id].get(Prop::COLOR_SCHEME))
            .is_some_and(|scheme| {
                scheme.contains("dark")
                    && (!scheme.contains("light") || crate::native::theme_state() & 1 != 0)
            });
        let source = background::canvas_source(&doc, &styles);
        // The html and body boxes read each other's background, which their kept builds don't track.
        let layers = source.map(|id| {
            let style = &styles.values[id];
            element::kept::hashed((
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
        dom.layout
            .layout_nodes
            .resize_with(doc.nodes.len(), || None);
        let mut info = std::mem::take(&mut dom.layout.cull.info);
        let unchanged = same
            && !relaid
            && paint.iter().all(|(id, style)| {
                let flags = cull::flags(&doc.nodes[*id].tag, style, &styles.lengths[*id], height)
                    & (cull::MOVES_CONTENT | cull::UNCULLABLE);
                info.get(*id)
                    .is_some_and(|info| info.flags & flags == flags)
            });
        let settled = unchanged && !dom.layout.container_pass;
        const PER_FRAME: u32 = cull::CHANGED
            | cull::CHILDREN_CHANGED
            | cull::GEOMETRY_CHANGED
            | cull::COMPOSITED
            | cull::COMPOSITED_PSEUDO
            | cull::TRANSFORMED
            | cull::ANIMATED;
        let mut fresh = match dom.layout.cull.fresh.take() {
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
        let (changed, sources, framed) = if unchanged {
            dom.layout
                .cull
                .painted
                .extend(paint.iter().map(|(id, _)| *id));
            dom.layout.cull.painted.sort_unstable();
            dom.layout.cull.painted.dedup();
            (Vec::new(), None, false)
        } else {
            frame::changes::detect(&fcx, dom, &styles, &paint, &mut info, &mut fresh)
        };
        // Changes built in place move no box, so boxes that read the last frame can keep it, also in a reflow.
        let quiet = (unchanged
            || sources.is_some_and(|ids: Vec<usize>| {
                ids.iter().all(|&id| dom.layout.cull.walk.contains(id))
            }))
            && !relaid
            && !flipped
            && !framed
            && changed.is_empty()
            && !dom.layout.container_pass
            && paint
                .iter()
                .map(|(id, _)| *id)
                .chain(restore_pseudos.iter().map(|(id, _)| *id))
                .all(|id| {
                    dom.layout
                        .layout_nodes
                        .get(id)
                        .and_then(Option::as_ref)
                        .is_some_and(|node| (node.flags | node.marks) & cull::READS_LAST_FRAME == 0)
                });
        self.composited_transforms.borrow_mut().clear();
        self.promote.set(
            (!composited.is_empty()).then(|| (now + std::time::Duration::from_millis(300), false)),
        );
        let relayouts = frame::changes::animate(
            &fcx,
            dom,
            &mut styles,
            &mut info,
            &mut fresh,
            paint,
            &mut restore,
        );
        frame::changes::remeasure(&fcx, dom, &styles, &changed, &mut info, &mut fresh);
        let warnings = if inspectable && !dom.styles.linted {
            frame::lint::lint(&doc, dom, &styles, &changed)
        } else {
            Vec::new()
        };
        let page_scroll = dom.geometry.scroll.entry(0).or_default().clone();
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
        let [overflow_x, overflow] = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y].map(|prop| {
            source
                .and_then(|id| styles.values[id].get(prop))
                .unwrap_or("visible")
        });
        let [page_scrolls_x, page_scrolls] =
            [overflow_x, overflow].map(|value| !matches!(value, "hidden" | "clip"));
        // Like Chrome, the viewport takes the direction of the root element, so an RTL page scrolls from its right.
        let page_rtl =
            html.is_some_and(|html| styles.values[html].get(Prop::DIRECTION) == Some("rtl"));
        let page_overscroll = html.map_or_else(Default::default, |html| {
            element::margin::overscroll(&styles.values[html])
        });
        let page_bar = html
            .filter(|_| page_scrolls || page_scrolls_x)
            .and_then(|html| {
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
                look.forced = [
                    overflow_x == "scroll" || look.forced[0],
                    overflow == "scroll" || look.forced[1],
                ];
                Some(look)
            });
        let page_space = page_bar.as_ref().map_or([0.; 2], |look| {
            look.space(look.shown([page_scrolls_x, page_scrolls], page_scroll.extent()))
        });
        if !dom.layout.container_pass {
            let mut moved = 0.;
            dom.layout.cull.scrolled.clear();
            for (&id, handle) in &dom.geometry.scroll {
                let offset = handle.offset();
                let last = dom.layout.cull.offsets.insert(id, offset).unwrap_or(offset);
                moved += f32::from((offset.x - last.x).abs() + (offset.y - last.y).abs());
                if offset.y != last.y {
                    dom.layout
                        .cull
                        .scrolled
                        .insert(id, f32::from(last.y - offset.y));
                }
            }
            dom.layout.cull.resized = dom.layout.cull.frame[..2] != frame[..2];
            dom.layout.cull.enabled = !std::mem::take(&mut dom.layout.cull.off)
                && dom.layout.cull.frame[2..] == frame[2..];
            if moved >= height / 2. {
                self.kept.borrow_mut().retain(|_, kept| !kept.culled);
            }
            dom.layout.cull.frame = frame;
        }
        let zone = dom.layout.cull.enabled.then(|| {
            let moved = dom.layout.cull.scrolled.get(&0).copied().unwrap_or(0.);
            Rc::new(cull::Zone {
                top: moved,
                bottom: height + moved,
                clips: Vec::new(),
                resized: dom.layout.cull.resized,
            })
        });
        let mut left = Vec::new();
        let top_layer: Vec<usize> = dom
            .interaction
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
        dom.layout.measurements.pixel_ratio = window.scale_factor();
        let measured = &dom.layout.measurements;
        for (&id, &shown) in &measured.shown {
            if let Some(info) = info.get_mut(id) {
                info.flags = info.flags & !cull::SHOWN | (u32::from(shown) * cull::SHOWN);
            }
        }
        // A script reads geometry in a forced box, so neither it nor a box around it becomes a spacer.
        for &id in &measured.forced {
            let mut node = Some(id);
            while let Some(id) = node {
                if let Some(info) = info.get_mut(id) {
                    info.flags |= cull::SHOWN;
                }
                node = dom.tree.flat_parent(&doc, &styles.children, id);
            }
        }
        let autos = !dom.layout.autos.is_empty();
        let measured = RefCell::new(std::mem::take(&mut dom.layout.measurements));
        let walk = dom.styles.walk.snapshot();
        for text in warnings {
            host.log(serde_json::json!({"method":"Runtime.consoleAPICalled","params":{"type":"error","text":text,"context":crate::jsc::Role::Renderer(self.id)}}));
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
            breaks: &RefCell::default(),
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
            build.settled = (settled && !self.reflowing.get() || quiet)
                && !build.resized
                && !window.redrawing();
            kept.0 = frame;
        }
        #[cfg(debug_assertions)]
        let zone = build.zone.clone();
        let root = element(
            &build,
            0,
            [width - page_space[1], height - page_space[0]],
            [width - page_space[1], height - page_space[0]],
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
                    if let Some(color) = backdrop
                        .get(Prop::BACKGROUND_COLOR)
                        .and_then(css::value::color::paint)
                    {
                        el = el.bg(color);
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
                            [[0.; 4]; 3],
                            self.0.clone(),
                            host.page_state(self.id)
                                .and_then(|state| state.base.as_ref()),
                            &host.app.assets,
                            false,
                        ));
                    }
                }
                overlay.push(
                    match [Prop::BACKDROP_FILTER, Prop::WEBKIT_BACKDROP_FILTER]
                        .into_iter()
                        .find_map(|prop| backdrop.get(prop))
                        .and_then(|value| effects::filter::backdrop(value, &units))
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
            frame::patch::patch(&build, window);
        }
        frame::patch::place(
            &build,
            window,
            #[cfg(debug_assertions)]
            zone,
            #[cfg(debug_assertions)]
            [width - page_space[1], height - page_space[0]],
        );
        drop(build);
        let mut reads = reads.into_inner();
        reads.clear();
        *self.reads.borrow_mut() = reads;
        for (node, children) in children.into_iter().rev() {
            doc.nodes.compose_children(node, children);
        }
        let repass = {
            let mut host = self.host.borrow_mut();
            let dom = host.state(self.id);
            for id in dom.styles.unanchored.drain(..) {
                window.suppress_anchoring(id);
            }
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
            dom.layout.cull.info = info;
            dom.layout.cull.fresh = fresh;
            dom.layout.cull.visible = visible;
            dom.layout.measurements = measured.into_inner();
            dom.styles.animating.clear();
            dom.styles.animating_pseudos.clear();
            let restored =
                !restore.is_empty() || !restore_pseudos.is_empty() || !lengths.is_empty();
            // Animated values go back to the cascade values here, and the next frame visits their nodes again.
            // Animated units are not in the values, so a frame after them visits all nodes.
            dom.layout.cull.swapped = (restore[..relayouts].iter().map(|(id, _)| *id))
                .chain(restore_pseudos.iter().map(|(id, _)| *id))
                .collect();
            let units = !lengths.is_empty();
            // Animated padding and borders change content boxes that resize observers read.
            if relayouts > 0 || units {
                dom.geometry.mirror.restyled();
            }
            if restored {
                let styles = Rc::make_mut(&mut styles);
                for (id, old) in lengths {
                    styles.lengths.set(id, old);
                }
                for (id, style) in restore {
                    dom.styles
                        .animating
                        .insert(id, std::mem::replace(&mut styles.values[id], style));
                }
                for (id, pair) in restore_pseudos {
                    if let Some(animated) = styles.pseudos.insert(id, pair) {
                        dom.styles.animating_pseudos.insert(id, animated);
                    }
                }
            }
            dom.layout.cull.seen = (!units).then(|| (seen.0, seen.1, styles.clone()));
            if taken {
                dom.styles.computed = Some(styles);
            }
            if !left.is_empty() {
                dom.interaction.top_layer.retain(|id| !left.contains(id));
                for id in left {
                    if let Some(node) = doc.nodes.edit(id) {
                        node.state &= !crate::dom::LEAVING;
                    }
                }
                dom.styles.invalidate();
            }
            // Allow at most one extra pass per frame to bound container-query resize loops.
            let changed = dom.update_containers(&doc);
            let repass = changed && !dom.layout.container_pass;
            dom.layout.container_pass = repass;
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
        page.style().overflow.x = match page_scrolls_x {
            true => renderer::Overflow::Scroll,
            false => renderer::Overflow::Hidden,
        };
        if page_rtl {
            page.style().flow |= renderer::Flow::RTL_BLOCK;
        }
        page.style().overscroll = page_overscroll;
        let page = page
            .pr(renderer::px(page_space[1]))
            .pb(renderer::px(page_space[0]))
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
                            && let Some(dom) = host.page_state_mut(data.id)
                        {
                            dom.geometry.sync_scroll();
                            dom.geometry.prepainted(dom.pixel_ratio);
                        }
                    },
                    |_, _, _| (),
                )
                .absolute()
            })
            .children(autos.then(|| spacer::autos(self.0.clone())))
            .children(page_bar.map(|look| scrollbar::Scrollbar {
                id: 0,
                handle: page_scroll.clone(),
                look: Some(look),
                axes: [page_scrolls_x, page_scrolls],
                reserved: page_space,
                borders: [0.; 4],
                origin: [0.; 2],
                quiet: false,
            }))
            .children(crate::native::devtools::profiling().then(|| {
                renderer::canvas(
                    |_, _| (),
                    move |_, _, window| {
                        window.paint_volatile();
                        crate::native::devtools::frame(id, started)
                    },
                )
                .absolute()
            }))
            .children({
                let inspect = self.inspect.get();
                (inspect.hover.is_some() || inspect.selected.is_some()).then(|| {
                    let data = self.0.clone();
                    renderer::canvas(
                        |_, _| (),
                        move |_, _, window| {
                            window.paint_volatile();
                            inspect::paint(&data, window)
                        },
                    )
                    .absolute()
                })
            })
            .children(self.hud.borrow().is_some().then(|| {
                let data = self.0.clone();
                renderer::canvas(
                    |_, _| (),
                    move |_, _, window| {
                        window.paint_volatile();
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
