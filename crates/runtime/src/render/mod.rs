pub(crate) mod effects;
mod element;
pub(crate) use element::margin::{trims, viewport_overflow};
pub mod fonts;
pub(crate) mod frame;
pub(crate) use frame::anchor::{Anchored, unplaced};
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

use crate::css::properties::Prop;
use crate::style;
pub(crate) use hud::Hud;
pub(crate) use inspect::Inspect;
pub use page::PageState;
use paint::{background, field, native, terminal, text};
use renderer::{App, Window};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
};
pub use store::{Page, Store};

type MaskSources = Vec<(String, background::Source)>;

// The window glue of a page view, in the desktop: it runs the page scripts that a frame calls for.
pub(crate) trait Pages {
    fn drive(&self, cx: &mut App);
    // Runs `__appsLayout` after the next frame.
    fn queue_layout(self: Rc<Self>, window: &mut Window);
    fn animation_events(&self, events: Vec<(usize, &'static str, String, f64)>, cx: &mut App);
    fn lint(&self, warnings: Vec<String>);
}

pub(crate) struct ViewState {
    pub(crate) id: u64,
    pub(crate) runtime: Rc<dyn Pages>,
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
    // A script forced this layout.
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
    // The queued animation frame: true when it can composite, false when it must draw the page again.
    motion_frame: Cell<Option<bool>>,
    #[cfg(target_os = "macos")]
    canvas: Cell<(u32, u8)>,
    canvas_layers: Cell<Option<u64>>,
    rtl: Cell<bool>,
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

// Layout revisions are unique across pages and reloads, so a kept box never matches a node of another page.
static LAYOUT_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

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

    // Keeps the paint order of boxes and the snapshots of drawable elements. It gives the keys of the changed
    // snapshots when the page gets a `paint` event.
    pub(crate) fn drawn(
        &self,
        snapshots: Vec<renderer::Snapshot>,
        window: &mut Window,
    ) -> Option<Vec<u64>> {
        // The paint order of boxes, which hit tests sort by.
        if let Ok(mut store) = self.store.try_borrow_mut() {
            let dom = store.state(self.id);
            dom.layout.painted.fill(0);
            let page = self.id;
            for (order, &mark) in window.marks().iter().enumerate() {
                let id = (mark & u64::from(u32::MAX)) as usize;
                if mark >> 32 != page {
                    continue;
                }
                if dom.layout.painted.len() <= id {
                    dom.layout.painted.resize(id + 1, 0);
                }
                dom.layout.painted[id] = order as u64 + 1;
            }
        }
        if snapshots.is_empty() && !self.paint.take() {
            return None;
        }
        let mut changed = Vec::with_capacity(snapshots.len());
        {
            let mut store = self.store.borrow_mut();
            let (doc, dom, _) = store.page_parts(self.id);
            // An element that is no longer drawable gives its pixels back.
            if let Some(doc) = doc.filter(|_| !snapshots.is_empty()) {
                let drawable = |id: &usize| {
                    (doc.nodes.get(*id)).is_some_and(|node| node.attrs.contains_key("drawable"))
                };
                dom.media.snapshots.retain(|id, _| drawable(id));
                dom.geometry.drawn.retain(|id, _| drawable(id));
            }
            for renderer::Snapshot { key, size, pixels } in snapshots {
                if let Some(pixels) = tiny_skia::IntSize::from_wh(size[0], size[1])
                    .and_then(|size| tiny_skia::Pixmap::from_vec(pixels, size))
                {
                    dom.media
                        .snapshots
                        .insert(key as usize, (Rc::new(pixels), window.scale_factor()));
                    changed.push(key);
                }
            }
        }
        self.paint.set(false);
        Some(changed)
    }
}

impl ViewState {
    // Lays out the page for a script that reads geometry.
    pub(crate) fn reflow(
        self: &Rc<Self>,
        window: &mut Window,
        cx: &mut App,
        pointer_events: impl Fn() -> renderer::Node,
    ) {
        self.reflowing.set(true);
        window.layout(cx, |tree, window, cx| {
            self.render_page(tree, window, cx, pointer_events())
        });
        self.reflowing.set(false);
    }

    // The cursor box of the terminal in element `id`, where it composes text.
    pub(crate) fn terminal_caret(&self, dom: &PageState, id: usize) -> Option<[f32; 4]> {
        let session = dom.media.terminals.borrow().session(id)?;
        Some(self.terminals.borrow().get(&session)?.caret)
    }

    pub(crate) fn new(id: u64, runtime: Rc<dyn Pages>, store: Rc<RefCell<Store>>) -> Rc<Self> {
        Rc::new(ViewState {
            id,
            runtime,
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
            rtl: Cell::default(),
            drawing: Cell::default(),
            paint: Cell::default(),
            kept_frame: RefCell::default(),
            mismatched: RefCell::default(),
            motion_frame: Cell::default(),
        })
    }
}
