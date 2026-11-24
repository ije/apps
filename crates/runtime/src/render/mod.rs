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
mod store;
mod view;

use crate::css::properties::Prop;
use crate::{host::Host, style};
pub(crate) use hud::Hud;
pub(crate) use inspect::Inspect;
pub use page::PageState;
use paint::{background, field, native, terminal, text};
use renderer::{App, Window, WindowId};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};
pub use store::{Page, Store};

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
    pub(crate) store: Rc<RefCell<Store>>,
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
    if view.store.try_borrow_mut().map_or(true, |mut store| {
        let shifted = !store.pages.get(&id).is_some_and(|page| page.dirty)
            && store
                .page_state_mut(id)
                .is_some_and(|dom| dom.shift_scrolls());
        shifted
            || !store.pages.get(&id).is_some_and(|page| page.dirty)
                && !store
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
    if view.store.try_borrow().map_or(true, |store| {
        store.pages.get(&view.id).is_some_and(|page| page.dirty)
            || store
                .graphics
                .get(&view.id)
                .is_some_and(|gpu| gpu.presented())
    }) {
        window.refresh();
    }
    if view
        .store
        .try_borrow_mut()
        .ok()
        .and_then(|mut store| {
            store
                .page_state_mut(view.id)
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
        let store = host.borrow().store.clone();
        let view = Rc::new(ViewState {
            id,
            queued: Cell::new(false),
            runtime,
            host,
            store,
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
}
