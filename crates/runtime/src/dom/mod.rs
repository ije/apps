mod document;
mod entities;
mod selector;

pub use document::{Document, Node, escape};
pub(crate) use selector::{
    ACTIVE, Dependency, FOCUS, FOCUS_VISIBLE, FOCUS_WITHIN, HOVER, Named, Selector, SelectorCache,
    SelectorList,
};

use crate::style;
use crate::style::Prop;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::{
        Arc, LazyLock, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Row {
    pub shift: renderer::Pixels,
    pub right: renderer::Pixels,
    pub gap: renderer::Pixels,
    pub start: usize,
    pub drop: renderer::Pixels,
    pub above: renderer::Pixels,
}

pub(crate) type Rows = Rc<[Row]>;

pub(crate) type SvgBoxes = (
    String,
    Vec<(String, usize)>,
    [f32; 2],
    Vec<(usize, [f32; 4], [f32; 4])>,
    [f32; 4],
);

type Decoded = Result<Result<renderer::RenderImage, (String, [f32; 2])>>;

pub(crate) struct LayoutNode {
    pub shape: u64,
    pub attrs: u64,
    pub seen: u64,
    pub style: Rc<style::Style>,
    pub units: u64,
    pub list: u64,
    pub kids: u64,
    pub(crate) out: u64,
    pub revision: u64,
    pub geometry: u64,
    pub flags: u16,
    pub marks: u16,
}

// WARNING: Node geometry from the last frame, shared with the page through an `ArrayBuffer`: 4 header
// floats (stale, capacity, layout count, moved), 8 floats per node (x, y, width, height, scroll left, scroll top,
// scroll range x, scroll range y), then one byte of `MIRROR_*` flags per node.
#[derive(Default)]
pub struct LayoutMirror {
    memory: Option<Arc<MirrorMemory>>,
    capacity: usize,
}

struct MirrorMemory(std::ptr::NonNull<f32>, usize);

// WARNING: only the JS thread reads and writes the floats. Other threads only drop the last owner after a GC.
unsafe impl Send for MirrorMemory {}
unsafe impl Sync for MirrorMemory {}

impl Drop for MirrorMemory {
    fn drop(&mut self) {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(self.0.as_ptr(), self.1)) });
    }
}

#[derive(Default)]
pub(crate) struct Walk {
    tree: u64,
    composed: rustc_hash::FxHashMap<usize, Vec<usize>>,
    pub(crate) nodes: Rc<Vec<(u32, u32)>>,
}

impl Walk {
    pub(crate) fn update(
        &mut self,
        doc: &Document,
        composed: &rustc_hash::FxHashMap<usize, Vec<usize>>,
        shared: Option<&Walk>,
    ) -> bool {
        let tree = doc.nodes.tree();
        if self.tree == tree && !self.nodes.is_empty() && self.composed == *composed {
            return false;
        }
        self.tree = tree;
        self.composed.clone_from(composed);
        if let Some(shared) = shared
            .filter(|shared| shared.tree == tree && !shared.nodes.is_empty())
            .filter(|shared| shared.composed == *composed)
        {
            self.nodes = shared.nodes.clone();
            return true;
        }
        let mut nodes = Vec::with_capacity(self.nodes.len());
        let mut pending = vec![(0, u32::MAX)];
        while let Some((id, parent)) = pending.pop() {
            nodes.push((id, parent));
            pending.extend(
                composed
                    .get(&(id as usize))
                    .unwrap_or(&doc.nodes[id as usize].children)
                    .iter()
                    .rev()
                    .map(|&child| (child as u32, id)),
            );
        }
        self.nodes = Rc::new(nodes);
        true
    }
}

#[derive(Default)]
struct ShadowWalk {
    tree: u64,
    count: usize,
    slots: rustc_hash::FxHashMap<usize, Vec<usize>>,
    shadows: Rc<style::Shadows>,
}

pub(crate) const CHECKED: u16 = 16;
pub(crate) const DIRTY: u16 = 32;
pub(crate) const MODAL: u16 = 64;
pub(crate) const POPOVER: u16 = 256;
pub(crate) const FULLSCREEN: u16 = 512;
// WARNING: an image that failed to load, which shows its alt text.
pub(crate) const BROKEN: u16 = 1024;
pub(crate) const TOP_LAYER: u16 = MODAL | POPOVER | FULLSCREEN;
// WARNING: the top layer bits that a box lost, shifted, while it stays in the top layer until its `overlay`
// transition ends.
pub(crate) const LEAVING: u16 = TOP_LAYER << 5;

impl Node {
    pub(crate) fn sf_symbol(&self) -> Option<&str> {
        let name = self.tag.strip_prefix("sf-symbol-")?;
        (!name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            && !name.split('-').any(str::is_empty))
        .then_some(name)
    }

    pub(crate) fn checked(&self) -> bool {
        if self.state & DIRTY != 0 {
            self.state & CHECKED != 0
        } else {
            self.attrs.contains_key(if self.tag == "option" {
                "selected"
            } else {
                "checked"
            })
        }
    }

    pub(crate) fn password(&self) -> bool {
        self.tag == "input"
            && self
                .attrs
                .get("type")
                .is_some_and(|kind| kind.eq_ignore_ascii_case("password"))
    }

    pub(crate) fn shown_text(&self, text: String) -> String {
        if self.password() {
            "\u{2022}".repeat(text.chars().count())
        } else {
            text
        }
    }

    pub(crate) fn current_value(&self) -> Option<&str> {
        self.value.as_deref().or_else(|| {
            (self.tag == "input").then(|| self.attrs.get("value").map_or("", String::as_str))
        })
    }
}

pub(crate) const MIRROR_BOUNDS: u8 = 1;
pub(crate) const MIRROR_SCROLLER: u8 = 2;
pub(crate) const MIRROR_CLIP: u8 = 4;
pub(crate) const MIRROR_POSITIONED: u8 = 8;
pub(crate) const MIRROR_HIDDEN: u8 = 16;
pub(crate) const MIRROR_FIXED: u8 = 32;
// WARNING: inert, or hidden by the cascade while an exit transition or animation still paints it, so it takes no
// focus, as in browsers.
pub(crate) const MIRROR_INERT: u8 = 64;
const HIDDEN: u8 = 1;
const EDITABLE: u8 = 2;
const INERT: u8 = 4;

impl LayoutMirror {
    fn slot(&self, id: usize) -> *mut f32 {
        self.memory
            .as_ref()
            .map_or(std::ptr::NonNull::dangling().as_ptr(), |memory| {
                memory.0.as_ptr()
            })
            .wrapping_add(4 + id * 8)
    }

    fn flags(&self, id: usize) -> *mut u8 {
        self.slot(self.capacity).cast::<u8>().wrapping_add(id)
    }

    fn reserve(&mut self, id: usize) {
        if id < self.capacity {
            return;
        }
        let capacity = (id + 1).next_power_of_two().max(1024);
        let words = 4 + capacity * 8 + capacity / 4;
        let memory = std::ptr::NonNull::new(Box::into_raw(vec![0f32; words].into_boxed_slice()))
            .expect("Layout memory")
            .cast::<f32>();
        unsafe {
            *memory.as_ptr().add(1) = capacity as f32;
            if let Some(old) = &self.memory {
                std::ptr::copy_nonoverlapping(
                    self.slot(0),
                    memory.as_ptr().add(4),
                    self.capacity * 8,
                );
                std::ptr::copy_nonoverlapping(
                    self.flags(0),
                    memory.as_ptr().add(4 + capacity * 8).cast::<u8>(),
                    self.capacity,
                );
                *memory.as_ptr().add(2) = *old.0.as_ptr().add(2);
                // WARNING: pages keep reading the old buffer until they see this flag and fetch the new one.
                *old.0.as_ptr() = 1.;
            }
        }
        self.memory = Some(Arc::new(MirrorMemory(memory, words)));
        self.capacity = capacity;
    }

    pub fn get(&self, id: &usize) -> Option<&[f32; 4]> {
        (*id < self.capacity && unsafe { *self.flags(*id) } & MIRROR_BOUNDS != 0)
            .then(|| unsafe { &*self.slot(*id).cast::<[f32; 4]>() })
    }

    pub fn get_mut(&mut self, id: &usize) -> Option<&mut [f32; 4]> {
        (*id < self.capacity && unsafe { *self.flags(*id) } & MIRROR_BOUNDS != 0)
            .then(|| unsafe { &mut *self.slot(*id).cast::<[f32; 4]>() })
    }

    pub fn insert(&mut self, id: usize, rect: [f32; 4]) {
        self.reserve(id);
        unsafe {
            *self.slot(id).cast::<[f32; 4]>() = rect;
            *self.flags(id) |= MIRROR_BOUNDS;
        }
    }

    pub fn remove(&mut self, id: &usize) {
        if *id < self.capacity {
            unsafe {
                *self.slot(*id).cast::<[f32; 8]>() = [0.; 8];
                *self.flags(*id) = 0;
            }
        }
    }

    pub(crate) fn moved(&self) -> bool {
        self.memory
            .as_ref()
            .is_some_and(|memory| unsafe { *memory.0.as_ptr().add(3) } != 0.)
    }

    pub(crate) fn set_moved(&mut self) {
        if let Some(memory) = &self.memory {
            unsafe { *memory.0.as_ptr().add(3) = 1. };
        }
    }

    pub(crate) fn touch(&mut self) {
        self.reserve(0);
        if let Some(memory) = &self.memory {
            // WARNING: an f32 stops counting at 2^24, so the count wraps. Pages only compare it for equality.
            unsafe {
                let count = memory.0.as_ptr().add(2);
                *count = (*count + 1.) % 16_777_216.;
                *memory.0.as_ptr().add(3) = 0.;
            }
        }
    }

    // WARNING: `nodes` must hold every node shown in either list and every node past the end of `old`.
    pub(crate) fn hide(
        &mut self,
        visible: &[bool],
        old: &[bool],
        nodes: impl Iterator<Item = usize>,
    ) {
        if let Some(last) = visible.len().checked_sub(1) {
            self.reserve(last);
        }
        for id in nodes {
            let Some(&shown) = visible.get(id) else {
                continue;
            };
            if old.get(id).copied().unwrap_or(true) != shown {
                unsafe {
                    let flags = self.flags(id);
                    *flags = *flags & !MIRROR_HIDDEN | if shown { 0 } else { MIRROR_HIDDEN };
                }
            }
        }
    }

    fn scroll(&mut self, id: usize, values: [f32; 4]) {
        self.reserve(id);
        unsafe {
            *self.slot(id).add(4).cast::<[f32; 4]>() = values;
            *self.flags(id) |= MIRROR_SCROLLER;
        }
    }

    pub(crate) fn share(&mut self) -> (*mut std::ffi::c_void, usize, *mut std::ffi::c_void) {
        self.reserve(0);
        let memory = self.memory.clone().expect("Layout memory");
        (
            memory.0.as_ptr().cast(),
            (4 + self.capacity * 8) * 4 + self.capacity,
            Arc::into_raw(memory).cast_mut().cast(),
        )
    }

    pub(crate) unsafe extern "C" fn release(
        _: *mut std::ffi::c_void,
        owner: *mut std::ffi::c_void,
    ) {
        drop(unsafe { Arc::from_raw(owner.cast::<MirrorMemory>()) });
    }
}

impl Extend<(usize, [f32; 4])> for LayoutMirror {
    fn extend<T: IntoIterator<Item = (usize, [f32; 4])>>(&mut self, items: T) {
        for (id, rect) in items {
            self.insert(id, rect);
        }
    }
}

impl std::ops::Index<&usize> for LayoutMirror {
    type Output = [f32; 4];
    fn index(&self, id: &usize) -> &[f32; 4] {
        self.get(id).expect("Missing bounds")
    }
}

#[derive(Default)]
pub struct State {
    pub base: Option<url::Url>,
    pub(crate) free: std::collections::BinaryHeap<std::cmp::Reverse<usize>>,
    pub(crate) removed: Vec<usize>,
    pub(crate) created: usize,
    pub(crate) detached: Vec<usize>,
    live: rustc_hash::FxHashSet<usize>,
    pub bounds: LayoutMirror,
    #[allow(clippy::type_complexity)]
    pub(crate) text_layouts: rustc_hash::FxHashMap<
        usize,
        (
            renderer::TextLayout,
            Rc<str>,
            std::ops::Range<usize>,
            Rows,
            [renderer::Pixels; 2],
        ),
    >,
    pub scroll: rustc_hash::FxHashMap<usize, renderer::ScrollHandle>,
    scroll_offsets: rustc_hash::FxHashMap<usize, renderer::Point<renderer::Pixels>>,
    pub(crate) computed: Option<Rc<style::ComputedStyles>>,
    previous: Option<Rc<style::ComputedStyles>>,
    // WARNING: a list per node is megabytes on big pages, and a new one costs a page fault per 16 KB.
    old_styles: Option<Rc<style::ComputedStyles>>,
    pub(crate) style_walk: Walk,
    editable: Vec<usize>,
    inert: Vec<usize>,
    // WARNING: by node id, the `HIDDEN`, `EDITABLE` and `INERT` bits of the last style pass.
    select_marks: Vec<u8>,
    shadow_walk: Rc<ShadowWalk>,
    pub(crate) grids: rustc_hash::FxHashMap<
        usize,
        (
            f32,
            Option<f32>,
            Option<crate::render::grid::Grid>,
            crate::render::grid::Grid,
        ),
    >,
    pub(crate) flexed_grids: rustc_hash::FxHashMap<usize, [Option<(f32, f32)>; 2]>,
    pub(crate) grid_widest: rustc_hash::FxHashMap<usize, f32>,
    pub(crate) baselines: crate::render::grid::Baselines,
    pub(crate) layout_nodes: Vec<Option<LayoutNode>>,
    pub(crate) measurements: crate::render::grid::Measurements,
    pub painted: Vec<u64>,
    pub(crate) paint_count: u64,
    // WARNING: the last revision from the process-wide counter in `render`, so 0 means the state never drew.
    pub(crate) layout_revision: u64,
    style_epoch: u64,
    layout_epoch: u64,
    pub(crate) sheets: HashMap<Rc<str>, Rc<style::Stylesheet>>,
    pub(crate) inserted: Vec<(String, Rc<style::Stylesheet>, bool)>,
    pub(crate) style_cache: style::StyleCache,
    style_viewport: [f32; 2],
    style_metrics: Option<f32>,
    style_reduced_motion: bool,
    style_theme: u32,
    pub focus: Option<usize>,
    interaction: [Vec<usize>; 3],
    pub selection: rustc_hash::FxHashMap<usize, [usize; 2]>,
    pub selection_caret: Option<usize>,
    pub dirty: bool,
    pub frame_requested: bool,
    pub(crate) wheel: bool,
    pub canvases: rustc_hash::FxHashMap<usize, crate::render::canvas::Canvas>,
    pub(crate) sources: std::cell::RefCell<rustc_hash::FxHashMap<usize, CanvasSource>>,
    pub images: rustc_hash::FxHashMap<usize, Arc<renderer::RenderImage>>,
    pub shadows: rustc_hash::FxHashMap<usize, usize>,
    pub(crate) hosts: rustc_hash::FxHashMap<usize, usize>,
    pub retired: Vec<Arc<renderer::RenderImage>>,
    pub(crate) retired_canvases: HashSet<usize>,
    pub(crate) recycled: Vec<(usize, Arc<renderer::RenderImage>)>,
    decoded: Arc<Mutex<Vec<(usize, u64, Decoded)>>>,
    frames: Frames,
    pub(crate) placeholders: rustc_hash::FxHashMap<usize, Placeholder>,
    decoding: rustc_hash::FxHashMap<usize, u64>,
    pub(crate) reserved: rustc_hash::FxHashSet<usize>,
    pub(crate) svg_sources: rustc_hash::FxHashMap<usize, String>,
    pub(crate) svg_boxes: rustc_hash::FxHashMap<usize, SvgBoxes>,
    pub(crate) svg_trees: rustc_hash::FxHashMap<usize, usvg::Tree>,
    pub(crate) svg_images: rustc_hash::FxHashMap<usize, (Rc<str>, [f32; 2])>,
    pub(crate) masks: rustc_hash::FxHashMap<usize, (String, Option<Arc<renderer::RenderImage>>)>,
    pub svgs: rustc_hash::FxHashMap<usize, (String, Arc<renderer::RenderImage>)>,
    pub(crate) svg_cache: HashMap<
        String,
        (
            crate::render::canvas::SvgGeometry,
            Vec<Arc<renderer::RenderImage>>,
        ),
    >,
    pub backgrounds: HashMap<(usize, usize), (String, Arc<renderer::RenderImage>)>,
    pub(crate) gradients: Vec<(String, Arc<renderer::RenderImage>)>,
    pub font_aliases: HashMap<String, String>,
    pub(crate) fonts: HashMap<String, Option<String>>,
    pub(crate) font_loads: u32,
    pub(crate) font_text: (u64, Vec<u64>),
    pub text_system: Option<std::rc::Rc<renderer::TextSystem>>,
    pub(crate) window: renderer::WindowState,
    pub pixel_ratio: f32,
    pub(crate) linted: bool,
    pub(crate) warned: HashSet<String>,
    pub(crate) checked: rustc_hash::FxHashSet<(u8, style::Prop, Rc<str>)>,
    pub(crate) motion: style::Motion,
    pub(crate) animated: HashMap<usize, style::Style>,
    pub(crate) animating: HashMap<usize, Rc<style::Style>>,
    pub(crate) animating_pseudos: HashMap<usize, Rc<[style::Style; 2]>>,
    pub(crate) recolored: Vec<usize>,
    pub(crate) containers: rustc_hash::FxHashMap<usize, [f32; 2]>,
    pub(crate) atoms: rustc_hash::FxHashMap<usize, [f32; 3]>,
    pub(crate) container_pass: bool,
    pub(crate) anchors: rustc_hash::FxHashMap<usize, renderer::Bounds<renderer::Pixels>>,
    pub(crate) blocks: rustc_hash::FxHashMap<usize, renderer::Bounds<renderer::Pixels>>,
    pub(crate) cull: crate::render::cull::Cull,
    pub top_layer: Vec<usize>,
}

impl State {
    pub(crate) fn sync_scroll(&mut self) {
        for (&id, handle) in &self.scroll {
            let (offset, max) = (scroll_offset(handle, handle.offset()), handle.max_offset());
            self.bounds.scroll(
                id,
                [
                    -f32::from(offset.x),
                    -f32::from(offset.y),
                    f32::from(max.x),
                    f32::from(max.y),
                ],
            );
        }
    }

    // WARNING: such nodes wait for a GC to be freed, and a scrolling list makes them much faster than GCs run,
    // so they would outnumber the nodes in the document. A node that comes back computes them again.
    fn drop_removed(&mut self, doc: &Document) {
        let mut gone = Vec::new();
        for root in std::mem::take(&mut self.removed) {
            if doc.nodes.get(root).is_none_or(|node| node.parent.is_some())
                || self.hosts.contains_key(&root)
            {
                continue;
            }
            let mut next = gone.len();
            gone.push(root);
            while next < gone.len() {
                let id = gone[next];
                next += 1;
                gone.extend(&doc.nodes[id].children);
                gone.extend(self.shadows.get(&id));
            }
        }
        self.style_cache.forget(&gone);
        for id in gone {
            self.measurements.texts.remove(&id);
            if let Some(node) = self.layout_nodes.get_mut(id) {
                *node = None;
            }
        }
    }

    pub(crate) fn trim(&mut self, doc: &mut Document) -> bool {
        self.drop_removed(doc);
        let before = doc.nodes.len();
        let mut free = std::mem::take(&mut self.free).into_sorted_vec();
        let mut len = before;
        let cut = free
            .iter()
            .take_while(|id| {
                let last = id.0 + 1 == len;
                if last {
                    len = id.0;
                }
                last
            })
            .count();
        free.drain(..cut);
        free.shrink_to_fit();
        self.free = free.into();
        self.old_styles = None;
        self.style_walk = Default::default();
        self.select_marks = Vec::new();
        self.measurements.texts = Default::default();
        self.style_cache.trim(len);
        self.layout_nodes.truncate(len);
        self.layout_nodes.shrink_to_fit();
        self.painted.truncate(len);
        self.painted.shrink_to_fit();
        self.motion.trim(len);
        self.cull.trim(len);
        if len < before {
            doc.nodes.truncate(len);
            self.invalidate();
        }
        len < before
    }

    // WARNING: inertness stays out of `ComputedStyles.user_select`, because kept boxes read that with their
    // revision, and a modal dialog changes it without one.
    pub(crate) fn inert(&self, id: usize) -> bool {
        self.select_marks
            .get(id)
            .is_some_and(|marks| marks & INERT != 0)
    }

    pub(crate) fn invalidate(&mut self) {
        if let Some(computed) = self.computed.take() {
            self.previous = Some(computed);
        }
    }

    pub(crate) fn update_containers(&mut self, doc: &Document) -> bool {
        let changed = self.style_cache.set_containers(&self.containers, doc);
        if changed {
            self.invalidate();
        }
        changed
    }

    pub(crate) fn allocate(&mut self, doc: &mut Document, node: Node) -> usize {
        let id = self.slot(doc);
        self.place(doc, id, node);
        id
    }

    // An unused node id, freed or new, whose node is empty until `place` fills it.
    fn slot(&mut self, doc: &mut Document) -> usize {
        self.free.pop().map_or_else(
            || {
                doc.nodes.push(Node::default());
                doc.nodes.len() - 1
            },
            |id| id.0,
        )
    }

    pub(crate) fn place(&mut self, doc: &mut Document, id: usize, node: Node) {
        doc.nodes[id] = node;
        self.detached.push(id);
        if let Some(cached) = &mut self.computed {
            // WARNING: every per-node list must grow here, and a detached node gets what a full pass gives it, or
            // reads of the new node index past their end and incremental passes keep other values.
            if let Some(computed) = Rc::get_mut(cached) {
                let (style, vars) = style::empty();
                computed.values.resize(doc.nodes.len(), style.clone());
                computed.values[id] = style;
                computed.vars.resize(doc.nodes.len(), vars.clone());
                computed.vars[id] = vars;
                computed.flags.resize(doc.nodes.len(), 0);
                computed.flags[id] = 0;
                computed.lengths.grow(doc.nodes.len());
                computed.lengths.unset(id);
                computed
                    .user_select
                    .resize(doc.nodes.len(), (style::UserSelect::None, 0));
                computed.user_select[id] = (style::UserSelect::None, 0);
            } else if id >= cached.values.len() {
                self.invalidate();
            }
            // WARNING: a frame keeps a clone of the styles, so they can't grow in place. Detached nodes have empty styles in them, and new nodes are detached, so only a new id makes them stale.
        }
    }
}

fn slots(
    doc: &Document,
    roots: impl IntoIterator<Item = (usize, usize)>,
) -> rustc_hash::FxHashMap<usize, Vec<usize>> {
    let mut slots = rustc_hash::FxHashMap::default();
    let (mut names, mut pending) = (rustc_hash::FxHashMap::default(), Vec::new());
    for (host, root) in roots {
        names.clear();
        pending.push(root);
        while let Some(id) = pending.pop() {
            let node = &doc.nodes[id];
            if node.tag == "slot" {
                slots.insert(id, Vec::new());
                names
                    .entry(node.attrs.get("name").map_or("", String::as_str))
                    .or_insert(id);
            }
            pending.extend(node.children.iter().rev());
        }
        for &child in &doc.nodes[host].children {
            let node = &doc.nodes[child];
            if (node.tag == "#text" || !node.tag.starts_with('#'))
                && let Some(slot) = names.get(node.attrs.get("slot").map_or("", String::as_str))
                && let Some(assigned) = slots.get_mut(slot)
            {
                assigned.push(child);
            }
        }
    }
    slots
}

#[derive(Clone)]
pub(crate) struct Placeholder {
    frames: Frames,
    node: usize,
    wake: crate::host::Waker,
}

type Frames = Arc<Mutex<HashMap<usize, Arc<renderer::RenderImage>>>>;

impl Placeholder {
    pub(crate) fn show(&self, image: Arc<renderer::RenderImage>) {
        self.frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(self.node, image);
        self.wake.wake();
    }
}

static PLACEHOLDERS: LazyLock<Mutex<HashMap<u64, Placeholder>>> = LazyLock::new(Default::default);

fn canvas_size(node: &Node) -> (u32, u32) {
    let size = |name, default| {
        node.attrs
            .get(name)
            .and_then(|value| crate::render::dimension(value, true))
            .map_or(default, |value| value as u32)
    };
    (size("width", 300), size("height", 150))
}

fn new_canvas(node: &Node) -> Result<crate::render::canvas::Canvas> {
    let (width, height) = canvas_size(node);
    let mut canvas = crate::render::canvas::Canvas::new(width, height)?;
    canvas.present = node.tag == "canvas";
    Ok(canvas)
}

// WARNING: keyed by the image id, or by the SVG source address and drawn size. The entry holds the SVG source, so
// its address cannot be reused for another source while the key is kept.
pub(crate) type CanvasSource = (
    (u64, usize, [u32; 2]),
    Option<Rc<str>>,
    Rc<tiny_skia::Pixmap>,
);

// WARNING: images keep their premultiplied pixels for later draws, up to 64 MB in all, until the image or the
// drawn SVG size changes or the node goes. A changed source is not kept at once, so videos and SVGs drawn at
// changing sizes keep nothing.
fn source_pixels(
    state: &State,
    id: usize,
    size: Option<[u32; 2]>,
) -> Option<crate::render::canvas::Source<'_>> {
    use crate::render::canvas::{self, Source};
    if let Some(canvas) = state.canvases.get(&id) {
        return Some(Source::Pixels(&canvas.pixels));
    }
    let image = state.images.get(&id);
    let svg = state.svg_images.get(&id).filter(|_| image.is_none());
    let size = svg.map_or([0; 2], |(_, natural)| {
        size.unwrap_or(natural.map(|side| side.ceil() as u32))
    });
    let key = (
        image.map_or(0, |image| image.id),
        svg.map_or(0, |(source, _)| Rc::as_ptr(source) as *const u8 as usize),
        size,
    );
    let mut cache = state.sources.try_borrow_mut().ok()?;
    let fresh = match cache.get(&id) {
        Some((old, _, pixels)) if *old == key => return Some(Source::Shared(pixels.clone())),
        Some(_) => {
            cache.remove(&id);
            false
        }
        None => true,
    };
    let pixels = Rc::new(match (image, svg) {
        (Some(image), _) => canvas::pixmap(image)?,
        (_, Some((source, _))) => {
            canvas::svg_pixels(&canvas::svg_tree(source).ok()?, size[0], size[1], None).ok()?
        }
        _ => return None,
    });
    if fresh {
        let kept: usize = cache.values().map(|(.., pixels)| pixels.data().len()).sum();
        if kept + pixels.data().len() > 64 << 20 {
            cache.clear();
        }
        cache.insert(
            id,
            (key, svg.map(|(source, _)| source.clone()), pixels.clone()),
        );
    }
    Some(Source::Shared(pixels))
}

pub(crate) fn show_frame(
    state: &mut State,
    id: usize,
    image: Option<Arc<renderer::RenderImage>>,
) -> Option<Arc<renderer::RenderImage>> {
    if state.recycled.is_empty() {
        state.retired_canvases.clear();
    }
    // WARNING: a poster that decodes later must not replace the frame.
    state.decoding.remove(&id);
    let size = image.as_ref().map(|image| image.size());
    match size {
        Some(size) => state.measurements.naturals.insert(
            id,
            [u32::from(size.width) as f32, u32::from(size.height) as f32],
        ),
        None => state.measurements.naturals.remove(&id),
    };
    let old = match image {
        Some(image) => state.images.insert(id, image),
        None => state.images.remove(&id),
    };
    if old.as_ref().map(|old| old.size()) != size
        && let Some(node) = state.layout_nodes.get_mut(id)
    {
        *node = None;
        state.cull.seen = None;
    }
    state.dirty = true;
    // WARNING: only the first frame replaced since the renderer drained `recycled` can be on the GPU; later ones are returned so unrendered windows do not grow it.
    match old {
        Some(old) if state.retired_canvases.insert(id) => {
            state.recycled.push((id, old));
            None
        }
        old => old,
    }
}

pub(crate) fn load_image(
    state: &mut State,
    id: usize,
    read: impl FnOnce() -> Result<Vec<u8>> + Send + 'static,
) -> Value {
    type Job = Box<dyn FnOnce() + Send>;
    static JOBS: Mutex<(VecDeque<Job>, usize)> = Mutex::new((VecDeque::new(), 0));
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let request = NEXT.fetch_add(1, Ordering::Relaxed);
    state.decoding.insert(id, request);
    let (decoded, wake) = (state.decoded.clone(), crate::host::Waker::current());
    let mut jobs = JOBS.lock().unwrap_or_else(PoisonError::into_inner);
    jobs.0.push_back(Box::new(move || {
        let result = read().and_then(|bytes| {
            ensure!(bytes.len() <= 64 * 1024 * 1024, "Image is too large");
            crate::render::image::decode(&bytes)
                .map(Ok)
                .or_else(|error| {
                    let size = usvg::Tree::from_data(&bytes, &Default::default())
                        .map_err(|_| error)?
                        .size();
                    let text = if bytes.starts_with(&[0x1f, 0x8b]) {
                        usvg::decompress_svgz(&bytes)?
                    } else {
                        bytes
                    };
                    Ok(Err((
                        String::from_utf8_lossy(&text).into_owned(),
                        [size.width(), size.height()],
                    )))
                })
        });
        decoded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id, request, result));
        wake.wake();
    }));
    // WARNING: at most two images decode at once to bound their memory, and idle workers exit.
    if jobs.1 < 2
        && std::thread::Builder::new()
            .name("apps-image".into())
            .spawn(|| {
                while let Some(job) = {
                    let mut jobs = JOBS.lock().unwrap_or_else(PoisonError::into_inner);
                    let job = jobs.0.pop_front();
                    jobs.1 -= usize::from(job.is_none());
                    job
                } {
                    job();
                }
            })
            .is_ok()
    {
        jobs.1 += 1;
    }
    json!(request)
}

pub fn call(
    op: &str,
    args: &Value,
    doc: &mut Document,
    state: &mut State,
    viewport: [f32; 2],
) -> Option<Result<Value>> {
    if !matches!(
        op,
        "dom.requestFrame"
            | "dom.rangeNodes"
            | "dom.release"
            | "dom.tree"
            | "dom.source"
            | "dom.reserve"
            | "dom.matches"
            | "dom.closest"
            | "dom.mediaMatches"
            | "dom.assignedNodes"
            | "dom.queryWithin"
            | "dom.html"
            | "dom.focus"
            | "dom.hover"
            | "dom.active"
            | "dom.selection"
            | "dom.selectionText"
            | "dom.userSelect"
            | "dom.scroll"
            | "dom.scrollEvents"
            | "dom.canvas"
            | "dom.canvasPixels"
            | "dom.canvasURL"
            | "dom.canvasTake"
            | "dom.canvasAdopt"
            | "dom.canvasLink"
            | "dom.canvasBind"
            | "dom.frames"
            | "dom.image"
            | "dom.decoded"
            | "dom.measureText"
            | "dom.canvasQuery"
            | "dom.textRect"
            | "dom.textIndex"
            | "dom.svgBox"
            | "dom.textOffset"
            | "dom.textPoint"
            | "dom.font"
            | "dom.fontCheck"
            | "dom.fontSources"
            | "dom.hitTest"
            | "dom.css"
            | "dom.sheet"
            | "dom.constructed"
            | "dom.adopt"
            | "dom.supports"
            | "dom.registerProperty"
            | "dom.computed"
            | "dom.animation"
            | "dom.animated"
            | "dom.easing"
            | "dom.cssAnimations"
            | "dom.cssControl"
            | "dom.declare"
            | "dom.properties"
            | "dom.client"
            | "dom.innerText"
            | "dom.wheel"
            | "dom.iterations"
    ) {
        return None;
    }
    Some((|| {
        if op == "dom.scrollEvents" {
            let mut changed = Vec::new();
            for (&id, handle) in &state.scroll {
                let offset = scroll_offset(handle, handle.offset());
                // WARNING: `set_offset` stops a smooth scroll, so it only runs when the range clamps the offset.
                if offset != handle.offset() {
                    handle.set_offset(offset);
                }
                let previous = state.scroll_offsets.entry(id).or_default();
                if *previous != offset {
                    *previous = offset;
                    changed.push(id);
                }
            }
            changed.sort_unstable();
            state.sync_scroll();
            return Ok(json!(changed));
        }
        if op == "dom.decoded" {
            let decoded =
                std::mem::take(&mut *state.decoded.lock().unwrap_or_else(PoisonError::into_inner));
            let mut results = Vec::with_capacity(decoded.len());
            for (id, request, result) in decoded {
                if state.decoding.get(&id) != Some(&request) {
                    results.push(json!([request, null]));
                    continue;
                }
                state.decoding.remove(&id);
                state.dirty |= result.is_ok();
                // WARNING: a decoded image resizes its box, so its layout and the culled spacers around it are stale.
                if result.is_ok()
                    && let Some(node) = state.layout_nodes.get_mut(id)
                {
                    *node = None;
                    state.cull.seen = None;
                }
                results.push(json!([
                    request,
                    match result {
                        Ok(Ok(image)) => {
                            let size = image.size();
                            state.measurements.naturals.insert(
                                id,
                                [u32::from(size.width) as f32, u32::from(size.height) as f32],
                            );
                            state.svg_images.remove(&id);
                            state
                                .retired
                                .extend(state.images.insert(id, Arc::new(image)));
                            json!([u32::from(size.width), u32::from(size.height)])
                        }
                        Ok(Err((text, size))) => {
                            state.measurements.naturals.insert(id, size);
                            state.retired.extend(state.images.remove(&id));
                            state.svg_images.insert(id, (text.into(), size));
                            json!(size)
                        }
                        Err(error) => json!(error.to_string()),
                    }
                ]));
            }
            return Ok(json!(results));
        }
        if op == "dom.frames" {
            let frames =
                std::mem::take(&mut *state.frames.lock().unwrap_or_else(PoisonError::into_inner));
            let mut results = Vec::with_capacity(frames.len());
            if state.retired.is_empty() {
                state.retired_canvases.clear();
            }
            for (node, image) in frames {
                let size = image.size();
                results.push(json!([node, u32::from(size.width), u32::from(size.height)]));
                if let Some(old) = state.images.insert(node, image)
                    && state.retired_canvases.insert(node)
                {
                    state.retired.push(old);
                }
                state.dirty = true;
            }
            return Ok(json!(results));
        }
        if op == "dom.wheel" {
            state.wheel = true;
            return Ok(Value::Null);
        }
        if op == "dom.iterations" {
            state.motion.iterations = true;
            return Ok(Value::Null);
        }
        if op == "dom.cssControl" {
            let time = args[2].as_f64().unwrap_or(0.);
            if !time.is_finite() {
                return Ok(json!(false));
            }
            let system = state.text_system.clone();
            let styles = styles(
                doc,
                state,
                viewport,
                system.as_deref().map(|system| (system, state.pixel_ratio)),
            );
            state.dirty = true;
            return Ok(json!(state.motion.css_control(
                args[0].as_u64().unwrap_or(0) as u32,
                args[1].as_str().unwrap_or_default(),
                time,
                &styles,
            )));
        }
        if op == "dom.easing" {
            let easing = args[0].as_str().unwrap_or("linear");
            return Ok(json!(style::easing(easing, args[1].as_f64().unwrap_or(0.))));
        }
        if op == "dom.animation" {
            let id = args[0].as_u64().context("Expected an animation")? as u32;
            state.dirty = true;
            // WARNING: the last frame's animated style of the node is stale now, so getComputedStyle samples again.
            if let Some(node) = state.motion.unscript(id) {
                state.animating.remove(&node);
                state.animating_pseudos.remove(&node);
            }
            let spec = &args[2];
            let Some(node) = args[1]
                .as_u64()
                .map(|node| node as usize)
                .filter(|&node| spec.is_object() && node < doc.nodes.len())
            else {
                return Ok(Value::Null);
            };
            state.animating.remove(&node);
            state.animating_pseudos.remove(&node);
            let frames: Vec<(f64, &str, String)> = spec["frames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|frame| {
                    Some((
                        frame[0].as_f64()?,
                        frame[1].as_str()?,
                        frame[2].as_str().unwrap_or("linear").to_owned(),
                    ))
                })
                .collect();
            let number = |index: usize| spec["timing"][index].as_f64().unwrap_or(0.);
            let iterations = number(3);
            let animation = style::Scripted::new(
                id,
                &frames,
                [
                    number(0) / 1000.,
                    number(1) / 1000.,
                    number(2) / 1000.,
                    if iterations < 0. {
                        f64::INFINITY
                    } else {
                        iterations
                    },
                    number(4),
                ],
                [
                    spec["direction"].as_str().unwrap_or("normal"),
                    spec["fill"].as_str().unwrap_or("none"),
                    spec["easing"].as_str().unwrap_or("linear"),
                    spec["composite"].as_str().unwrap_or("replace"),
                ],
                match spec["pseudo"].as_str() {
                    Some("::before") => 1,
                    Some("::after") => 2,
                    _ => 0,
                },
            );
            state.motion.script(
                node,
                animation,
                spec["time"].as_f64().unwrap_or(0.) / 1000.,
                spec["rate"].as_f64(),
            );
            return Ok(Value::Null);
        }
        if op == "dom.requestFrame" {
            state.frame_requested = true;
            state.dirty = true;
            return Ok(json!(state.text_system.is_some()));
        }
        if op == "dom.mediaMatches" {
            let query = args[0].as_str().context("Expected a media query")?;
            let source = format!("@media {query} {{}}");
            let sheet = state
                .sheets
                .entry(source.as_str().into())
                .or_insert_with(|| Rc::new(style::Stylesheet::parse(&source)));
            let theme = crate::desktop::native::theme_state()
                | ((state.pixel_ratio.clamp(0., 600.) * 100.).round() as u32) << 16;
            return Ok(sheet.matches_media(viewport[0], viewport[1], theme).into());
        }
        if op == "dom.properties" {
            return Ok(json!(style::Prop::names()));
        }
        // WARNING: a flat list of name, value and priority becomes JavaScript strings directly, without JSON.
        if op == "dom.declare" {
            return Ok(json!(
                style::declare(args[0].as_str().unwrap_or_default()).concat()
            ));
        }
        if matches!(
            op,
            "dom.constructed" | "dom.adopt" | "dom.supports" | "dom.registerProperty"
        ) {
            let text = args[2].as_str().unwrap_or_default();
            let result = match op {
                "dom.supports" => {
                    let name = args[0].as_str().context("Expected CSS text")?;
                    return Ok(json!(match args[1].as_str() {
                        Some(value) =>
                            style::supports(&name.to_ascii_lowercase(), value) == Some(true),
                        None => style::supports_condition(name),
                    }));
                }
                "dom.registerProperty" => {
                    let name = args[0].as_str().context("Expected a name")?;
                    let syntax = args[1].as_str().unwrap_or("*");
                    let initial = args[3].as_str();
                    let rule = format!(
                        "@property {name}{{syntax:{};inherits:{};{}}}",
                        json!(syntax),
                        args[2].as_bool().unwrap_or(false),
                        initial.map_or(String::new(), |value| format!("initial-value:{value}"))
                    );
                    let registered = style::Stylesheet::parse(&rule).property();
                    // WARNING: `Prop::new` leaks the names it interns, so only a valid new registration interns one.
                    match registered {
                        _ if !name.starts_with("--") => Err("SyntaxError"),
                        _ if style::Prop::find(name)
                            .is_some_and(|prop| state.style_cache.scripted.contains_key(&prop)) =>
                        {
                            Err("InvalidModificationError")
                        }
                        Some(value) => {
                            state
                                .style_cache
                                .scripted
                                .insert(style::Prop::new(name), value);
                            Ok(Value::Null)
                        }
                        None => Err("SyntaxError"),
                    }
                }
                "dom.adopt" => {
                    let root = args[0].as_u64().context("Expected a root")? as usize;
                    let ids = args[1]
                        .as_array()
                        .context("Expected sheets")?
                        .iter()
                        .filter_map(|id| Some(id.as_u64()? as u32))
                        .collect::<Vec<_>>();
                    if ids.is_empty() {
                        state.style_cache.adopted.remove(&root);
                    } else {
                        state.style_cache.adopted.insert(root, ids);
                    }
                    Ok(Value::Null)
                }
                _ => {
                    let id = args[0].as_u64().context("Expected a sheet")? as u32;
                    match args[1].as_str().unwrap_or_default() {
                        "parse" => return Ok(style::Stylesheet::parse(text).cssom()),
                        "replace" => {
                            let sheet = style::Stylesheet::parse(text);
                            let tree = sheet.cssom();
                            state.style_cache.constructed.insert(id, Rc::new(sheet));
                            Ok(tree)
                        }
                        "drop" => {
                            state.style_cache.constructed.remove(&id);
                            Ok(Value::Null)
                        }
                        kind => {
                            let path = path(&args[3]);
                            let top = (matches!(kind, "insert" | "delete") && path.len() == 1)
                                .then(|| (path[0], kind == "insert"));
                            state.style_cache.edit_constructed(id, top, |sheet| {
                                sheet.edit(&path, edit(kind, text))
                            })
                        }
                    }
                }
            };
            state.invalidate();
            state.dirty = true;
            return Ok(result.unwrap_or_else(|error| json!({ "error": error })));
        }
        if op == "dom.release" {
            let wrapped = args[1].as_array().context("Expected live nodes")?;
            // WARNING: only a wrapper keeps a removed tree until a GC, so new wrappers, not new nodes, count for `report_nodes`.
            state.created += wrapped.len();
            // WARNING: the page already forgot these lists, so a bad id is skipped instead of failing the rest.
            for value in wrapped {
                if let Some(id) = value.as_u64().map(|id| id as usize)
                    && doc.nodes.get(id).is_some_and(|node| !node.tag.is_empty())
                {
                    state.live.insert(id);
                }
            }
            for value in args[0].as_array().context("Expected dead nodes")? {
                if let Some(id) = value.as_u64() {
                    state.live.remove(&(id as usize));
                }
            }
            state.drop_removed(doc);
            let mut roots = std::mem::take(&mut state.detached);
            roots.sort_unstable();
            roots.dedup();
            let mut freed = Vec::new();
            let mut canvases = Vec::new();
            let retired = state.retired.len();
            for root in roots {
                if root == 0
                    || doc
                        .nodes
                        .get(root)
                        .is_none_or(|node| node.parent.is_some() || node.tag.is_empty())
                    || state.hosts.contains_key(&root)
                {
                    continue;
                }
                let mut pending = vec![root];
                let mut subtree = Vec::new();
                let mut live = false;
                while let Some(id) = pending.pop() {
                    if state.live.contains(&id) {
                        live = true;
                        break;
                    }
                    subtree.push(id);
                    pending.extend(&doc.nodes[id].children);
                    if let Some(root) = state.shadows.get(&id) {
                        pending.push(*root);
                    }
                }
                if live {
                    state.detached.push(root);
                    continue;
                }
                for id in subtree {
                    if matches!(
                        doc.nodes[id].tag.as_str(),
                        "canvas" | "apps-canvas" | "video" | "audio"
                    ) {
                        canvases.push(id);
                    }
                    doc.nodes[id] = Node::default();
                    state.bounds.remove(&id);
                    state.cull.widths.remove(&id);
                    state.text_layouts.remove(&id);
                    state.scroll.remove(&id);
                    state.scroll_offsets.remove(&id);
                    state.cull.offsets.remove(&id);
                    state.grids.remove(&id);
                    state.baselines.remove(&id);
                    if let Some(node) = state.layout_nodes.get_mut(id) {
                        *node = None;
                    }
                    state.measurements.texts.remove(&id);
                    state.retired.extend(state.images.remove(&id));
                    state.retired.extend(
                        state
                            .canvases
                            .remove(&id)
                            .filter(|canvas| canvas.present)
                            .map(|canvas| canvas.image),
                    );
                    state.placeholders.remove(&id);
                    state.retired.extend(
                        state
                            .svgs
                            .remove(&id)
                            .map(|(_, image)| image)
                            .filter(|image| Arc::strong_count(image) == 1),
                    );
                    state
                        .retired
                        .extend(state.masks.remove(&id).and_then(|(_, image)| image));
                    state.svg_sources.remove(&id);
                    state.svg_boxes.remove(&id);
                    state.svg_trees.remove(&id);
                    state.svg_images.remove(&id);
                    state.measurements.naturals.remove(&id);
                    state.sources.get_mut().remove(&id);
                    state.decoding.remove(&id);
                    if let Some(root) = state.shadows.remove(&id) {
                        state.hosts.remove(&root);
                    }
                    state.hosts.remove(&id);
                    state.motion.forget(id);
                    state.anchors.remove(&id);
                    state.blocks.remove(&id);
                    state.containers.remove(&id);
                    state.flexed_grids.remove(&id);
                    state.grid_widest.remove(&id);
                    if state.focus == Some(id) {
                        state.focus = None;
                    }
                    state.selection.remove(&id);
                    state.atoms.remove(&id);
                    if state.selection_caret == Some(id) {
                        state.selection_caret = None;
                    }
                    if let Some(order) = state.painted.get_mut(id) {
                        *order = 0;
                    }
                    state.free.push(std::cmp::Reverse(id));
                    freed.push(id);
                }
            }
            // WARNING: the page drops the records of the freed nodes, so it doesn't walk the trees it detached again.
            let list = json!([canvases, freed]);
            if !freed.is_empty() {
                state.style_cache.forget(&freed);
                let freed: rustc_hash::FxHashSet<usize> = freed.into_iter().collect();
                state.top_layer.retain(|node| !freed.contains(node));
                state
                    .measurements
                    .sizes
                    .retain(|(node, ..), _| !freed.contains(node));
                state.backgrounds.retain(|(node, _), (_, image)| {
                    let keep = !freed.contains(node);
                    if !keep {
                        state.retired.push(image.clone());
                    }
                    keep
                });
                state.dirty |= state.retired.len() > retired;
            }
            return Ok(list);
        }
        if op == "dom.rangeNodes" {
            let start = args[0].as_u64().context("Expected start node")? as usize;
            let end = args[2].as_u64().context("Expected end node")? as usize;
            ensure!(
                start < doc.nodes.len() && end < doc.nodes.len(),
                "Invalid range node"
            );
            let from = args[1].as_u64().context("Expected start offset")? as usize;
            let to = args[3].as_u64().context("Expected end offset")? as usize;
            let mut positions = HashMap::new();
            let mut following = |mut id: usize| {
                loop {
                    let parent = doc.nodes[id].parent?;
                    let children = &doc.nodes[parent].children;
                    if !positions.contains_key(&id) {
                        positions.extend(
                            children
                                .iter()
                                .enumerate()
                                .map(|(index, &child)| (child, index)),
                        );
                    }
                    if let Some(&next) = children.get(*positions.get(&id)? + 1) {
                        return Some(next);
                    }
                    id = parent;
                }
            };
            let mut current = if doc.nodes[start].tag == "#text" {
                Some(start)
            } else {
                doc.nodes[start]
                    .children
                    .get(from)
                    .copied()
                    .or_else(|| following(start))
            };
            let stop = if doc.nodes[end].tag == "#text" {
                following(end)
            } else {
                doc.nodes[end]
                    .children
                    .get(to)
                    .copied()
                    .or_else(|| following(end))
            };
            let mut ranges = Vec::new();
            while let Some(id) = current.filter(|id| Some(*id) != stop) {
                let node = &doc.nodes[id];
                if node.tag == "#text" {
                    ranges.push([
                        id,
                        if id == start { from } else { 0 },
                        if id == end {
                            to
                        } else {
                            node.text.encode_utf16().count()
                        },
                    ]);
                }
                current = node.children.first().copied().or_else(|| following(id));
            }
            return Ok(json!(ranges));
        }
        if op == "dom.hitTest" {
            ensure!(!doc.nodes.is_empty(), "Document is unavailable");
            let x = args[0].as_f64().context("Expected x")? as f32;
            let y = args[1].as_f64().context("Expected y")? as f32;
            let system = state.text_system.clone();
            let computed = styles(
                doc,
                state,
                viewport,
                system.as_deref().map(|system| (system, state.pixel_ratio)),
            );
            let (first, caret) = (args[2].as_bool(), args[3].as_bool());
            return Ok(hit_test(
                doc,
                state,
                &computed,
                viewport,
                [x, y],
                first.unwrap_or(false),
                caret.unwrap_or(false),
            ));
        }
        if matches!(op, "dom.focus" | "dom.hover" | "dom.active") {
            let ids = args[0]
                .as_array()
                .context("Expected nodes")?
                .iter()
                .map(|id| {
                    id.as_u64()
                        .map(|id| id as usize)
                        .filter(|&id| id < doc.nodes.len())
                        .context("Invalid node id")
                })
                .collect::<Result<Vec<_>>>()?;
            let (index, flag) = match op {
                "dom.focus" => (0, FOCUS_WITHIN),
                "dom.hover" => (1, HOVER),
                _ => (2, ACTIVE),
            };
            let focus = if op == "dom.focus" { FOCUS } else { 0 };
            // WARNING: like Chrome, `:focus` and `:focus-within` don't match while the window is not focused, but the
            // focus stays, so fields keep their selection.
            let shown = args[2].as_bool() != Some(true);
            let used = state.style_cache.states & (flag | focus) != 0;
            let control = |id: &usize| {
                doc.nodes.get(*id).is_some_and(|node| {
                    matches!(
                        node.tag.as_str(),
                        "button" | "input" | "select" | "textarea"
                    )
                })
            };
            let repaint = focus == 0
                && (ids.iter().any(control) || state.interaction[index].iter().any(control));
            if op == "dom.focus" {
                if let Some(node) = state.focus.and_then(|id| doc.nodes.edit(id)) {
                    node.state &= !(FOCUS | FOCUS_VISIBLE);
                }
                state.focus = ids.first().copied();
                if let Some(node) = state
                    .focus
                    .filter(|_| shown)
                    .and_then(|id| doc.nodes.edit(id))
                {
                    node.state |= if args[1].as_bool() == Some(false) {
                        FOCUS
                    } else {
                        FOCUS | FOCUS_VISIBLE
                    };
                }
            }
            for &id in &state.interaction[index] {
                if let Some(node) = doc.nodes.edit(id) {
                    node.state &= !flag;
                }
            }
            for &id in ids.iter().filter(|_| shown) {
                if let Some(node) = doc.nodes.edit(id) {
                    node.state |= flag;
                }
            }
            state.interaction[index] = ids;
            if used {
                state.invalidate();
            }
            state.dirty |= used || focus != 0 || repaint;
            return Ok(Value::Null);
        }
        if op == "dom.reserve" {
            let ids: Vec<_> = (0..args[0].as_u64().context("Expected count")?.min(4096))
                .map(|_| state.allocate(doc, Node::default()))
                .collect();
            state.reserved.extend(&ids);
            return Ok(json!(ids));
        }
        let id = args[0].as_u64().context("Expected node id")? as usize;
        ensure!(id < doc.nodes.len(), "Invalid node id");
        // WARNING: the root element scrolls the page unless it is a scroll container itself.
        let page = doc.nodes[id].parent == Some(0)
            && doc.nodes[id].tag == "html"
            && !state.scroll.contains_key(&id);
        match op {
            "dom.assignedNodes" => {
                let mut roots = HashSet::new();
                let mut slotted = HashMap::new();
                let mut add = |slot: usize, slotted: &mut HashMap<usize, Vec<usize>>| {
                    let mut top = slot;
                    while let Some(parent) = doc.nodes[top].parent {
                        top = parent;
                    }
                    if let Some(&host) = state.hosts.get(&top)
                        && roots.insert(top)
                    {
                        slotted.extend(slots(doc, [(host, top)]));
                    }
                };
                add(id, &mut slotted);
                let mut nodes = slotted.get(&id).cloned().unwrap_or_default();
                if args[1].as_bool() == Some(true) && slotted.contains_key(&id) {
                    if nodes.is_empty() {
                        nodes = doc.nodes[id].children.clone();
                    }
                    let mut pending: Vec<_> = nodes.into_iter().rev().collect();
                    nodes = Vec::new();
                    while let Some(child) = pending.pop() {
                        if doc.nodes[child].tag == "slot" {
                            add(child, &mut slotted);
                        }
                        if let Some(assigned) = slotted.get(&child) {
                            pending.extend(
                                if assigned.is_empty() {
                                    &doc.nodes[child].children
                                } else {
                                    assigned
                                }
                                .iter()
                                .rev(),
                            );
                        } else if doc.nodes[child].tag != "#comment" {
                            nodes.push(child);
                        }
                    }
                }
                Ok(json!(nodes))
            }
            "dom.tree" => Ok(tree(doc, id, true)),
            // WARNING: only the text of a `<style>` keeps its sheet as text; a `<link>` sheet is not text.
            "dom.source" => {
                let node = &doc.nodes[id];
                Ok(json!(match &node.stylesheet {
                    Some(sheet) if node.tag == "#text" => sheet.source(),
                    _ => node.text.clone(),
                }))
            }
            "dom.matches" => Ok(json!(
                Selector::try_parse(args[1].as_str().context("Expected selector")?)?
                    .iter()
                    .any(|s| s.matches_cached(doc, id, Some(id), &mut SelectorCache::default()))
            )),
            "dom.closest" => {
                let selectors =
                    Selector::try_parse(args[1].as_str().context("Expected selector")?)?;
                let mut memo = SelectorCache::default();
                let mut current = Some(id);
                while let Some(node) = current.filter(|&node| !doc.nodes[node].tag.starts_with('#'))
                {
                    if selectors
                        .iter()
                        .any(|s| s.matches_cached(doc, node, Some(id), &mut memo))
                    {
                        return Ok(json!(node));
                    }
                    current = doc.nodes[node].parent;
                }
                Ok(Value::Null)
            }
            "dom.queryWithin" => {
                let selectors =
                    Selector::try_parse(args[1].as_str().context("Expected selector")?)?;
                let first = args[2].as_bool().unwrap_or(false);
                let mut pending: Vec<_> = doc.nodes[id].children.iter().rev().copied().collect();
                let mut found = Vec::new();
                let mut memo = SelectorCache::default();
                while let Some(node) = pending.pop() {
                    if selectors
                        .iter()
                        .any(|s| s.matches_cached(doc, node, Some(id), &mut memo))
                    {
                        if first {
                            return Ok(json!(node));
                        }
                        found.push(node);
                    }
                    if doc.nodes[node].tag != "template" {
                        pending.extend(doc.nodes[node].children.iter().rev());
                    }
                }
                Ok(if first { Value::Null } else { json!(found) })
            }
            "dom.html" => {
                let source = args[1].as_str().context("Expected HTML")?;
                let tag = doc.nodes[id].tag.as_str();
                // WARNING: only pages know namespaces, so they pass the context tag, `svg` for any SVG element outside an HTML integration point.
                let parsed = match args[2].as_str() {
                    _ if tag == "#document" => Document::parse(source),
                    Some(context) => Document::fragment(source, context),
                    // WARNING: template contents and shadow roots are the only fragments with markup, and template rules keep `<tr>` rows that Lit renders.
                    None if tag.starts_with('#') => Document::fragment(source, "template"),
                    None => Document::fragment(source, tag),
                };
                for child in std::mem::take(&mut doc.nodes[id].children) {
                    doc.nodes[child].parent = None;
                    state.detached.push(child);
                    state.removed.push(child);
                }
                doc.nodes[id].text.clear();
                let mut ids = Vec::with_capacity(parsed.nodes.len());
                ids.push(id);
                for _ in 1..parsed.nodes.len() {
                    ids.push(state.slot(doc));
                }
                doc.nodes[id].children = parsed.nodes[0]
                    .children
                    .iter()
                    .map(|&child| ids[child])
                    .collect();
                // WARNING: the new nodes hang below `id`, so they are not detached roots.
                let detached = state.detached.len();
                for (index, mut node) in parsed.nodes.into_iter().enumerate().skip(1) {
                    node.parent = node.parent.map(|parent| ids[parent]);
                    for child in &mut node.children {
                        *child = ids[*child];
                    }
                    state.place(doc, ids[index], node);
                }
                state.detached.truncate(detached);
                state.dirty = true;
                Ok(tree(doc, id, false))
            }
            "dom.cssAnimations" => {
                let system = state.text_system.clone();
                let styles = styles(
                    doc,
                    state,
                    viewport,
                    system.as_deref().map(|system| (system, state.pixel_ratio)),
                );
                let mut nodes = Vec::new();
                let mut pending = vec![id];
                while let Some(node) = pending.pop() {
                    nodes.push(node);
                    if args[1].as_bool() == Some(true) {
                        pending.extend(doc.nodes[node].children.iter().rev());
                        pending.extend(state.shadows.get(&node));
                    }
                }
                Ok(json!(state.motion.css(
                    &nodes,
                    args[2].as_bool() == Some(true),
                    &styles
                )))
            }
            "dom.animated" => {
                let system = state.text_system.clone();
                let styles = styles(
                    doc,
                    state,
                    viewport,
                    system.as_deref().map(|system| (system, state.pixel_ratio)),
                );
                Ok(state
                    .motion
                    .scripted(id, 0, &styles)
                    .map_or(Value::Null, |values| {
                        values
                            .iter()
                            .map(|(key, value)| (key.name().to_owned(), json!(&**value)))
                            .collect::<serde_json::Map<_, _>>()
                            .into()
                    }))
            }
            "dom.computed" => {
                let system = state.text_system.clone();
                let styles = styles(
                    doc,
                    state,
                    viewport,
                    system.as_deref().map(|system| (system, state.pixel_ratio)),
                );
                let pseudo = args[2].as_str().unwrap_or_default().trim_start_matches(':');
                if let Some(style) = styles.values.get(id) {
                    state.motion.read(id, style);
                }
                let slot = match pseudo {
                    "" => Some(0),
                    "before" => Some(1),
                    "after" => Some(2),
                    _ => None,
                };
                let scripted = slot.and_then(|slot| state.motion.scripted(id, slot, &styles));
                // WARNING: the compositor can play animations between frames, so they are sampled now.
                let current = state
                    .animating
                    .contains_key(&id)
                    .then(|| {
                        let now = std::time::Instant::now();
                        state.motion.timeline(id, &styles, &[now], &[]).pop()
                    })
                    .flatten();
                let mut sampled = None;
                let backdrop = (pseudo == "backdrop")
                    .then(|| state.motion.backdrop(id, styles.backdrops.get(&id)?))
                    .flatten();
                let style = match pseudo {
                    "" => Some(current.as_ref().unwrap_or_else(|| {
                        &**state
                            .animating
                            .get(&id)
                            .or_else(|| styles.authored.get(&id))
                            .unwrap_or(&styles.values[id])
                    })),
                    "before" | "after" => state
                        .animating_pseudos
                        .get(&id)
                        .or_else(|| styles.pseudos.get(&id))
                        .map(|values| &values[usize::from(pseudo == "after")]),
                    "backdrop" => backdrop
                        .as_ref()
                        .or_else(|| styles.backdrops.get(&id).map(|style| &**style)),
                    _ => None,
                };
                let style = match (style, scripted) {
                    (Some(style), Some(values)) => {
                        let mut style = style.clone();
                        style.extend(values.iter().map(|(key, value)| (key, value.clone())));
                        Some(&*sampled.insert(style))
                    }
                    (style, _) => style,
                };
                let mut full = None;
                let style = match style {
                    Some(style)
                        if style.vars().is_empty()
                            && !styles.vars[id].is_empty()
                            && args[1].as_str().is_none_or(|name| name.starts_with("--")) =>
                    {
                        let mut copy = style.clone();
                        copy.set_vars(styles.vars[id].clone());
                        Some(&*full.insert(copy))
                    }
                    style => style,
                };
                let Some(style) = style.filter(|style| !style.is_empty()) else {
                    return Ok(json!(if args[1].is_string() {
                        json!("")
                    } else {
                        json!([])
                    }));
                };
                let units = styles.lengths[id];
                let size = (pseudo.is_empty())
                    .then(|| state.bounds.get(&id).map(|rect| [rect[2], rect[3]]))
                    .flatten();
                let container = || {
                    doc.nodes[id]
                        .parent
                        .and_then(|parent| {
                            let rect = state.bounds.get(&parent)?;
                            let (style, units) = (&styles.values[parent], styles.lengths[parent]);
                            let padding = units.edges(style, style::PADDING, rect[2]);
                            Some([
                                rect[2] - padding[1] - padding[3],
                                rect[3] - padding[0] - padding[2],
                            ])
                        })
                        .unwrap_or(viewport)
                };
                let registry = state.style_cache.registry();
                let parent = doc.nodes[id].parent.map(|parent| &*styles.values[parent]);
                Ok(match args[1].as_str() {
                    Some(name) => {
                        let value =
                            style::resolved(style, &units, name, size, &container, &registry);
                        let tag = doc.nodes[id].tag.as_str();
                        json!(match (name, value.as_str()) {
                            ("display", "grid") if tag == "table" => "table".to_owned(),
                            ("gap" | "row-gap" | "column-gap", _) if tag == "table" =>
                                "normal".to_owned(),
                            ("display", "contents")
                                if matches!(tag, "thead" | "tbody" | "tfoot" | "tr") =>
                            {
                                match tag {
                                    "thead" => "table-header-group",
                                    "tbody" => "table-row-group",
                                    "tfoot" => "table-footer-group",
                                    _ => "table-row",
                                }
                                .to_owned()
                            }
                            ("display", "block") if matches!(tag, "td" | "th") =>
                                "table-cell".to_owned(),
                            ("display", "block") if tag == "caption" => "table-caption".to_owned(),
                            ("min-width" | "min-height", "auto")
                                if parent.is_none_or(|parent| {
                                    parent.get(Prop::DISPLAY).is_none_or(|display| {
                                        !display.ends_with("flex") && !display.ends_with("grid")
                                    })
                                }) =>
                            {
                                "0px".to_owned()
                            }
                            ("top" | "right" | "bottom" | "left", "auto")
                                if style.get(Prop::POSITION) == Some("relative") =>
                            {
                                "0px".to_owned()
                            }
                            _ => value,
                        })
                    }
                    None => json!(
                        style::computed_names()
                            .chain(style.vars().iter().map(|(prop, _)| prop.name()))
                            .collect::<Vec<_>>()
                    ),
                })
            }
            "dom.css" => {
                if doc.nodes[id].tag == "link" && doc.nodes[id].stylesheet.is_none() {
                    return Ok(Value::Null);
                }
                let sheet = state.style_cache.sheet(doc, id, &mut state.sheets);
                Ok(sheet.map_or_else(|| json!([]), |sheet| sheet.cssom()))
            }
            "dom.sheet" => {
                let kind = args[1].as_str().unwrap_or_default();
                let text = args[2].as_str().unwrap_or_default();
                let path = path(&args[3]);
                let top = (matches!(kind, "insert" | "delete") && path.len() == 1)
                    .then(|| (path[0], kind == "insert"));
                let result = state.style_cache.edit_sheet(
                    doc,
                    id,
                    &mut state.sheets,
                    top,
                    |sheet, disabled| {
                        if kind == "disabled" {
                            *disabled = args[2].as_bool().unwrap_or(false);
                            return Ok(Value::Null);
                        }
                        sheet.edit(&path, edit(kind, text))
                    },
                );
                state.invalidate();
                state.dirty = true;
                Ok(result.unwrap_or_else(|error| json!({ "error": error })))
            }
            "dom.selection" => {
                state.selection.clear();
                state.selection_caret = None;
                if let Some(ranges) = args[1].as_array() {
                    for range in ranges {
                        let node = range[0].as_u64().context("Expected selection node")? as usize;
                        ensure!(node < doc.nodes.len(), "Invalid selection node");
                        state.selection.insert(
                            node,
                            [
                                range[1].as_u64().unwrap_or(0) as usize,
                                range[2].as_u64().unwrap_or(0) as usize,
                            ],
                        );
                        if args[2].as_bool() == Some(true) {
                            state.selection_caret = Some(node);
                        }
                    }
                } else if id != 0 {
                    state.selection.insert(
                        id,
                        [
                            args[1].as_u64().unwrap_or(0) as usize,
                            args[2].as_u64().unwrap_or(0) as usize,
                        ],
                    );
                    if args[1] == args[2] {
                        state.selection_caret = Some(id);
                    }
                }
                state.dirty = true;
                Ok(Value::Null)
            }
            "dom.userSelect" | "dom.selectionText" => {
                let system = state.text_system.clone();
                let computed = styles(
                    doc,
                    state,
                    viewport,
                    system.as_deref().map(|system| (system, state.pixel_ratio)),
                );
                if op == "dom.userSelect" {
                    return Ok(if state.inert(id) {
                        json!((style::UserSelect::None, id))
                    } else {
                        json!(computed.user_select[id])
                    });
                }
                let mut text = String::new();
                if let Some(ranges) = args[1].as_array() {
                    for range in ranges {
                        let node = range[0].as_u64().context("Expected selection node")? as usize;
                        ensure!(node < doc.nodes.len(), "Invalid selection node");
                        if computed.user_select[node].0 == style::UserSelect::None
                            || state.inert(node)
                        {
                            continue;
                        }
                        let value = &doc.nodes[node].text;
                        let start = utf16_index(value, range[1].as_u64().unwrap_or(0) as usize);
                        let end = utf16_index(value, range[2].as_u64().unwrap_or(0) as usize);
                        if start < end {
                            text.push_str(&value[start..end]);
                        }
                    }
                }
                Ok(json!(text))
            }
            "dom.scroll" => {
                let handle = state
                    .scroll
                    .entry(if page { 0 } else { id })
                    .or_default()
                    .clone();
                let previous = handle.offset();
                let ratio = state.pixel_ratio.max(1.);
                let axis = |value: &Value, current: renderer::Pixels| {
                    renderer::px(
                        value
                            .as_f64()
                            .map_or(f32::from(current), |v| -(v as f32 * ratio).round() / ratio),
                    )
                };
                let target = scroll_offset(
                    &handle,
                    renderer::point(axis(&args[1], previous.x), axis(&args[2], previous.y)),
                );
                if target != handle.target() {
                    let smooth = handle.max_offset() != renderer::Point::default()
                        && match args[3].as_str() {
                            Some("smooth") => true,
                            Some("instant") => false,
                            _ => {
                                let system = state.text_system.clone();
                                styles(
                                    doc,
                                    state,
                                    viewport,
                                    system.as_deref().map(|system| (system, state.pixel_ratio)),
                                )
                                .values[id]
                                    .get(Prop::SCROLL_BEHAVIOR)
                                    == Some("smooth")
                            }
                        };
                    if smooth {
                        handle.animate_to(target, false);
                    } else {
                        handle.set_offset(target);
                    }
                }
                state.dirty |= target != previous;
                state.sync_scroll();
                Ok(json!([-f32::from(target.x), -f32::from(target.y)]))
            }
            "dom.canvas" => {
                if let Some(ops) = crate::jsc::payload::slice(&args[1]) {
                    let ops: Vec<f64> = ops?
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .map(|bytes| f64::from_ne_bytes(*bytes))
                        .collect();
                    let data = args
                        .as_array()
                        .into_iter()
                        .flatten()
                        .skip(3)
                        .map(|value| {
                            crate::jsc::payload::slice(value).context("Expected image data")?
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let mut canvas = match state.canvases.remove(&id) {
                        Some(canvas) => canvas,
                        None => {
                            let canvas = new_canvas(&doc.nodes[id])?;
                            state.created += canvas.pixels.data().len() >> 16;
                            canvas
                        }
                    };
                    let old = canvas.image.clone();
                    let result = canvas.draw(
                        &ops,
                        args[2].as_str().unwrap_or(""),
                        &data,
                        &|(source, size)| source_pixels(state, source, Some(size)),
                        state
                            .text_system
                            .as_deref()
                            .map(|system| (system, &state.font_aliases)),
                    );
                    let canvas = state.canvases.entry(id).insert_entry(canvas).into_mut();
                    result?;
                    if state.retired.is_empty() {
                        state.retired_canvases.clear();
                    }
                    // WARNING: only the first image replaced since the renderer drained `retired` can be on the GPU; later ones are dropped so unrendered windows do not grow it.
                    if !Arc::ptr_eq(&old, &canvas.image) && state.retired_canvases.insert(id) {
                        state.retired.push(old);
                    }
                } else {
                    state.retired.extend(
                        state
                            .canvases
                            .remove(&id)
                            .filter(|canvas| canvas.present)
                            .map(|canvas| canvas.image),
                    );
                    state.retired.extend(state.images.remove(&id));
                    state.sources.get_mut().remove(&id);
                }
                // WARNING: offscreen drawing changes nothing on the page, so it must not cost page frames.
                state.dirty |= doc.nodes[id].tag == "canvas";
                Ok(Value::Null)
            }
            "dom.canvasURL" => {
                use base64::Engine as _;
                let bytes = match source_pixels(state, id, None) {
                    Some(pixels) => pixels.encode_png()?,
                    None => new_canvas(&doc.nodes[id])?.pixels.encode_png()?,
                };
                Ok(if args[1] == true {
                    crate::jsc::payload::write(bytes)
                } else {
                    json!(format!(
                        "data:image/png;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(bytes)
                    ))
                })
            }
            "dom.canvasPixels" => {
                let pixels = match source_pixels(state, id, None) {
                    Some(pixels) => pixels,
                    None => crate::render::canvas::Source::Shared(Rc::new(
                        new_canvas(&doc.nodes[id])?.pixels,
                    )),
                };
                let x = args[1].as_i64().unwrap_or(0);
                let y = args[2].as_i64().unwrap_or(0);
                let width = args[3].as_u64().context("Expected width")? as usize;
                let height = args[4].as_u64().context("Expected height")? as usize;
                ensure!(
                    width > 0
                        && height > 0
                        && width <= 16384
                        && height <= 16384
                        && width * height <= 16_777_216,
                    "Invalid pixel region"
                );
                let mut bytes = vec![0u8; width * height * 4];
                // WARNING: with `true` the bytes stay premultiplied, as they are stored.
                let premultiplied = args[5] == true;
                for row in 0..height {
                    for col in 0..width {
                        let sx = x + col as i64;
                        let sy = y + row as i64;
                        if sx < 0
                            || sy < 0
                            || sx >= i64::from(pixels.width())
                            || sy >= i64::from(pixels.height())
                        {
                            continue;
                        }
                        let pixel =
                            pixels.pixels()[sy as usize * pixels.width() as usize + sx as usize];
                        let color = pixel.demultiply();
                        let at = (row * width + col) * 4;
                        bytes[at..at + 4].copy_from_slice(&if premultiplied {
                            [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
                        } else {
                            [color.red(), color.green(), color.blue(), color.alpha()]
                        });
                    }
                }
                Ok(crate::web::base64(&bytes))
            }
            // WARNING: `id` stays blank with its drawing state, as `transferToImageBitmap()` needs.
            "dom.canvasTake" => {
                let to = args[1].as_u64().context("Expected node id")? as usize;
                ensure!(to < doc.nodes.len(), "Invalid node id");
                let mut taken = match state.canvases.get_mut(&id) {
                    Some(canvas) => {
                        let mut taken = crate::render::canvas::Canvas::new(
                            canvas.pixels.width(),
                            canvas.pixels.height(),
                        )?;
                        std::mem::swap(&mut canvas.pixels, &mut taken.pixels);
                        taken
                    }
                    None => {
                        let mut taken = new_canvas(&doc.nodes[id])?;
                        if let Some(pixels) = source_pixels(state, id, None) {
                            taken.pixels = (*pixels).clone();
                        }
                        taken
                    }
                };
                taken.present = doc.nodes[to].tag == "canvas";
                if taken.present {
                    taken.image = crate::render::canvas::render_image(&taken.pixels);
                }
                state.created += taken.pixels.data().len() >> 16;
                state.dirty |= taken.present;
                if state.retired.is_empty() {
                    state.retired_canvases.clear();
                }
                if let Some(old) = state.canvases.insert(to, taken)
                    && old.present
                    && state.retired_canvases.insert(to)
                {
                    state.retired.push(old.image);
                }
                Ok(Value::Null)
            }
            "dom.canvasAdopt" => {
                let mut canvas = new_canvas(&doc.nodes[id])?;
                canvas.pixels = tiny_skia::Pixmap::from_vec(
                    crate::web::bytes(&args[1])?,
                    tiny_skia::IntSize::from_wh(canvas.pixels.width(), canvas.pixels.height())
                        .context("Invalid canvas size")?,
                )
                .context("Invalid canvas pixels")?;
                state.created += canvas.pixels.data().len() >> 16;
                state.canvases.insert(id, canvas);
                Ok(Value::Null)
            }
            "dom.canvasLink" => {
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let token = NEXT.fetch_add(1, Ordering::Relaxed);
                let placeholder = state
                    .placeholders
                    .remove(&id)
                    .unwrap_or_else(|| Placeholder {
                        frames: state.frames.clone(),
                        node: id,
                        wake: crate::host::Waker::current(),
                    });
                PLACEHOLDERS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(token, placeholder);
                Ok(json!(token))
            }
            "dom.canvasBind" => {
                let token = args[1].as_u64().context("Expected a token")?;
                let placeholder = PLACEHOLDERS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&token)
                    .context("The canvas was already transferred")?;
                state.placeholders.insert(id, placeholder);
                Ok(Value::Null)
            }
            "dom.image" => {
                let bytes = crate::web::bytes(&args[1])?;
                Ok(load_image(state, id, move || Ok(bytes)))
            }
            "dom.font" => {
                let family = args[1].as_str().context("Expected font family")?;
                Ok(match args[2].as_str() {
                    Some(name) => json!(local_font(state, family, name)),
                    None => json!(add_font(state, family, crate::web::bytes(&args[2])?)?),
                })
            }
            "dom.fontSources" => Ok(json!(crate::render::fonts::sources(
                args[1].as_str().context("Expected font sources")?
            ))),
            "dom.fontCheck" => {
                let family = args[1].as_str().context("Expected font family")?;
                let family = state
                    .font_aliases
                    .get(family)
                    .map(String::as_str)
                    .unwrap_or(family);
                Ok(json!(
                    matches!(family, "serif" | "sans-serif" | "monospace" | "system-ui")
                        || crate::render::canvas::fonts(true).0.faces().any(|face| face
                            .families
                            .iter()
                            .any(|(name, _)| name.eq_ignore_ascii_case(family)))
                ))
            }
            "dom.textOffset" | "dom.textPoint" => {
                let target = args[1].as_u64().unwrap_or(0) as usize;
                let mut offset = 0;
                let mut pending = vec![id];
                while let Some(node) = pending.pop() {
                    let current = &doc.nodes[node];
                    if op == "dom.textOffset" && node == target {
                        let index = args[2].as_u64().unwrap_or(0) as usize;
                        if current.tag == "#text" {
                            return Ok(json!(offset + index));
                        }
                        // WARNING: count only text nodes, like the walk above, so comments before the boundary add nothing.
                        pending.clear();
                        pending.extend(current.children.iter().take(index));
                        while let Some(node) = pending.pop() {
                            let current = &doc.nodes[node];
                            if current.tag == "#text" {
                                offset += current.text.encode_utf16().count();
                            } else {
                                pending.extend(&current.children);
                            }
                        }
                        return Ok(json!(offset));
                    }
                    if current.tag == "#text" {
                        let length = current.text.encode_utf16().count();
                        if op == "dom.textPoint" && target <= offset + length {
                            return Ok(json!([node, target.saturating_sub(offset)]));
                        }
                        offset += length;
                    } else {
                        pending.extend(current.children.iter().rev());
                    }
                }
                Ok(if op == "dom.textOffset" {
                    json!(offset)
                } else {
                    // WARNING: with no text the point is inside the first empty block, as text after `<p><br></p>` would make a new line.
                    let mut node = id;
                    while let Some(&child) = doc.nodes[node].children.first()
                        && !doc.nodes[child].children.is_empty()
                    {
                        node = child;
                    }
                    json!([node, 0])
                })
            }
            "dom.svgBox" => {
                let boxes = std::iter::successors(Some(id), |&node| doc.nodes[node].parent)
                    .find_map(|node| state.svg_boxes.get(&node).map(|boxes| (node, boxes)));
                Ok(json!(
                    boxes
                        .and_then(|(root, (.., boxes, content))| if root == id {
                            Some(*content)
                        } else {
                            boxes
                                .iter()
                                .find(|(node, ..)| *node == id)
                                .map(|(.., object)| *object)
                        })
                        .unwrap_or_default()
                ))
            }
            "dom.textRect" | "dom.textIndex" => {
                let Some(bounds) = state.bounds.get(&id) else {
                    return Ok(if op == "dom.textRect" {
                        json!([])
                    } else {
                        json!(0)
                    });
                };
                // WARNING: a password shows bullets, so offsets go from its value to the bullets and back.
                let password = doc.nodes[id]
                    .password()
                    .then(|| doc.nodes[id].current_value().unwrap_or("").to_owned());
                let shown = |offset| {
                    password
                        .as_deref()
                        .map_or(offset, |value| bullets(value, offset))
                };
                let real = |index| {
                    password
                        .as_deref()
                        .map_or(index, |value| unbullet(value, index))
                };
                let start = shown(args[1].as_u64().unwrap_or(0) as usize);
                let end = args[2].as_u64().map_or(start, |end| shown(end as usize));
                if let Some((layout, text, range, rows, font)) = state.text_layouts.get(&id) {
                    let content = &text[range.clone()];
                    let source = doc.nodes[id].text.as_str();
                    if op == "dom.textIndex" {
                        let position = renderer::point(
                            renderer::px(args[1].as_f64().unwrap_or(0.) as f32),
                            renderer::px(args[2].as_f64().unwrap_or(0.) as f32),
                        );
                        let index = crate::render::text::text_index(layout, text, rows, position)
                            .unwrap_or_else(|index| index)
                            .clamp(range.start, range.end);
                        return Ok(json!(real(source_offset(
                            source,
                            content,
                            index - range.start
                        ))));
                    }
                    return Ok(json!(crate::render::text::text_rects(
                        layout,
                        text,
                        rows,
                        [start, end].map(|index| range.start + shown_index(source, content, index)),
                        *font,
                    )));
                }
                let system = state
                    .text_system
                    .clone()
                    .context("Text geometry requires a displayed window")?;
                let [x, y, width, full] = *bounds;
                // WARNING: a focus change just before a click can drop the cached styles, so they are computed again here.
                let computed = styles(doc, state, viewport, Some((&system, state.pixel_ratio)));
                let (style, units) = (&computed.values[id], &computed.lengths[id]);
                let node = &doc.nodes[id];
                let text = node.shown_text(
                    node.current_value()
                        .map_or_else(|| doc.text(id), str::to_owned),
                );
                let (font_size, height) = (units.font.em, units.font.lh.max(1.));
                // WARNING: like render, the text starts in the content box, and an input centers its line there.
                let padding = units.edges(style, style::PADDING, width);
                let [top, right, bottom, left] = [0, 1, 2, 3].map(|side| {
                    let [size, kind, _] = style::BORDERS[side];
                    padding[side].max(0.)
                        + if style
                            .get(kind)
                            .is_none_or(|kind| matches!(kind, "none" | "hidden"))
                        {
                            0.
                        } else {
                            style
                                .get(size)
                                .map_or(Some(3.), |size| units.resolve(size, width))
                                .unwrap_or(0.)
                                .max(0.)
                        }
                });
                let single = node.tag == "input";
                let bounds = [
                    x + left,
                    y + top
                        + if single {
                            (full - top - bottom - height) / 2.
                        } else {
                            0.
                        },
                    width - left - right,
                ];
                let run = renderer::TextRun {
                    len: text.len(),
                    font: style::font(style),
                    color: renderer::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                    letter_spacing: renderer::px(0.),
                    word_spacing: renderer::px(0.),
                    font_size: None,
                    inset: Default::default(),
                    nowrap: false,
                };
                let wrap = (!single
                    && !style
                        .get(Prop::WHITE_SPACE)
                        .is_some_and(|s| matches!(s, "pre" | "nowrap")))
                .then_some(renderer::px(bounds[2]));
                let lines = system.shape_text(
                    text.clone().into(),
                    renderer::px(font_size),
                    &[run],
                    wrap,
                    None,
                    crate::style::word_break(style),
                )?;
                let mut top = 0.;
                let mut offset = 0;
                let mut rects: Vec<[f32; 4]> = Vec::new();
                for line in lines {
                    let rows = line.wrap_boundaries.len() + 1;
                    if op == "dom.textIndex" {
                        let y = args[2].as_f64().unwrap_or(0.) as f32 - bounds[1];
                        if y < top + rows as f32 * height {
                            let index = line.closest_index_for_position(
                                renderer::point(
                                    renderer::px(args[1].as_f64().unwrap_or(0.) as f32 - bounds[0]),
                                    renderer::px(y - top),
                                ),
                                renderer::px(height),
                            );
                            let index = index.unwrap_or_else(|index| index);
                            return Ok(json!(real(
                                offset + line.text[..index].encode_utf16().count()
                            )));
                        }
                    } else {
                        let length = line.text.encode_utf16().count();
                        if end >= offset && start <= offset + length {
                            let a = utf16_index(&line.text, start.saturating_sub(offset));
                            let b = utf16_index(&line.text, end.saturating_sub(offset));
                            for byte in line
                                .text
                                .char_indices()
                                .map(|(i, _)| i)
                                .chain(std::iter::once(line.text.len()))
                                .filter(|i| *i >= a && *i <= b)
                            {
                                if let Some(point) =
                                    line.position_for_index(byte, renderer::px(height))
                                {
                                    let x = bounds[0] + f32::from(point.x);
                                    let y = bounds[1] + top + f32::from(point.y);
                                    if let Some(rect) = rects.last_mut().filter(|rect| rect[1] == y)
                                    {
                                        rect[2] = x - rect[0];
                                    } else {
                                        rects.push([x, y, 0., height]);
                                    }
                                }
                            }
                        }
                    }
                    offset += line.text.encode_utf16().count() + 1;
                    top += rows as f32 * height;
                }
                Ok(if op == "dom.textRect" {
                    json!(rects)
                } else {
                    json!(real(text.encode_utf16().count()))
                })
            }
            "dom.measureText" => {
                let system = state
                    .text_system
                    .as_ref()
                    .context("Text measurement requires a displayed window")?;
                let (line, [ascent, descent, em_ascent, em_descent]) = crate::render::canvas::line(
                    system,
                    &state.font_aliases,
                    args[2].as_str().unwrap_or("10px sans-serif"),
                    args[1].as_str().context("Expected text")?,
                    [
                        args[3].as_f64().unwrap_or(0.) as f32,
                        args[4].as_f64().unwrap_or(0.) as f32,
                    ],
                );
                let [left, right, top, bottom] = crate::render::canvas::ink(system, &line);
                Ok(json!([
                    f32::from(line.width),
                    left,
                    right,
                    top,
                    bottom,
                    ascent,
                    descent,
                    em_ascent,
                    em_descent
                ]))
            }
            "dom.canvasQuery" => {
                let Some(canvas) = state.canvases.get(&id) else {
                    return Ok(json!(if args[1].is_number() {
                        json!(false)
                    } else {
                        json!([1, 0, 0, 1, 0, 0])
                    }));
                };
                Ok(match args[1].as_f64() {
                    Some(x) => json!(canvas.hit(
                        x as f32,
                        args[2].as_f64().unwrap_or(0.) as f32,
                        args[3].as_bool().unwrap_or(false),
                        args[4].as_bool().unwrap_or(false),
                    )),
                    None => json!(canvas.transform()),
                })
            }
            "dom.client" => {
                let rect = state.bounds.get(&id).copied().unwrap_or_default();
                // WARNING: resize observers measure the boxes of the last frame, so they read its styles too. Styling
                // the page that changed since then cost a pass, and another after each callback that changed it.
                let last = (args[1].as_bool() == Some(true))
                    .then(|| state.computed.clone().or_else(|| state.previous.clone()))
                    .flatten()
                    .filter(|last| id < last.values.len() && last.lengths.get(id).is_some());
                let computed = match last {
                    Some(last) => last,
                    None => {
                        let system = state.text_system.clone();
                        styles(
                            doc,
                            state,
                            viewport,
                            system.as_deref().map(|system| (system, state.pixel_ratio)),
                        )
                    }
                };
                let (style, units) = (&computed.values[id], &computed.lengths[id]);
                let tag = doc.nodes[id].tag.as_str();
                if matches!(tag, "tr" | "thead" | "tbody" | "tfoot")
                    && style.get(Prop::DISPLAY) == Some("contents")
                {
                    return Ok(json!([0., 0., rect[2], rect[3], 0., 0., 0., 0.]));
                }
                if tag.starts_with('#')
                    || style
                        .get(Prop::DISPLAY)
                        .is_none_or(|value| matches!(value, "inline" | "contents"))
                {
                    return Ok(json!([0.; 8].as_slice()));
                }
                let shared = matches!(tag, "td" | "th")
                    && std::iter::successors(doc.nodes[id].parent, |&parent| {
                        doc.nodes[parent].parent
                    })
                    .find(|&parent| doc.nodes[parent].tag == "table")
                    .is_some_and(|table| {
                        computed.values.get(table).and_then(|style| style.get(Prop::BORDER_COLLAPSE))
                            == Some("collapse")
                    });
                let base = doc.nodes[id]
                    .parent
                    .and_then(|parent| state.bounds.get(&parent))
                    .map_or(viewport[0], |rect| rect[2]);
                let borders = style::BORDERS.map(|[width, kind, _]| {
                    if style
                        .get(kind)
                        .is_none_or(|kind| matches!(kind, "none" | "hidden"))
                    {
                        return 0.;
                    }
                    style
                        .get(width)
                        .map_or(Some(3.), |width| units.resolve(width, base))
                        .unwrap_or(0.)
                        .max(0.)
                        / if shared { 2. } else { 1. }
                });
                let padding = units
                    .edges(style, style::PADDING, base)
                    .map(|value| value.max(0.));
                let bars = state
                    .scroll
                    .get(&id)
                    .and_then(|handle| {
                        let look = crate::render::scrollbar::Look::new(
                            style,
                            computed.scrollbars.get(&id).map(|parts| &**parts),
                            units,
                            None,
                        )?;
                        let scrolls = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y].map(|prop| {
                            style
                                .get(prop)
                                .or_else(|| style.get(Prop::OVERFLOW))
                                .is_some_and(|value| matches!(value, "auto" | "scroll"))
                        });
                        Some(look.space(look.shown(scrolls, handle.max_offset())))
                    })
                    .unwrap_or_default();
                let gutters = if style
                    .get(Prop::SCROLLBAR_GUTTER)
                    .is_some_and(|value| value.contains("both-edges"))
                {
                    2.
                } else {
                    1.
                };
                Ok(json!([
                    borders[3],
                    borders[0],
                    (rect[2] - borders[1] - borders[3] - bars[1] * gutters).max(0.),
                    (rect[3] - borders[0] - borders[2] - bars[0]).max(0.),
                    padding[0],
                    padding[1],
                    padding[2],
                    padding[3]
                ]))
            }
            "dom.innerText" => {
                let system = state.text_system.clone();
                let computed = styles(
                    doc,
                    state,
                    viewport,
                    system.as_deref().map(|system| (system, state.pixel_ratio)),
                );
                let mut text = String::new();
                let (mut breaks, mut collapsed) = (0, false);
                let mut pending = vec![(id, false)];
                while let Some((node, close)) = pending.pop() {
                    let current = &doc.nodes[node];
                    let style = &computed.values[node];
                    let display = style.get(Prop::DISPLAY).unwrap_or("inline");
                    let required = if current.tag == "p" {
                        2
                    } else if display.starts_with("inline")
                        || matches!(display, "contents" | "table-cell" | "ruby" | "none")
                    {
                        0
                    } else {
                        1
                    };
                    if close {
                        if display == "table-cell" {
                            text.push('\t');
                        } else if display == "table-row" && text.ends_with('\t') {
                            text.pop();
                        }
                        breaks = breaks.max(required);
                        continue;
                    }
                    if node != id && display == "none" || current.tag == "#comment" {
                        continue;
                    }
                    if current.tag == "br" {
                        text.push('\n');
                        (breaks, collapsed) = (0, false);
                        continue;
                    }
                    if current.tag == "#text" {
                        let parent = current
                            .parent
                            .map_or(style, |parent| &computed.values[parent]);
                        if parent
                            .get(Prop::VISIBILITY)
                            .is_some_and(|value| matches!(value, "hidden" | "collapse"))
                        {
                            continue;
                        }
                        let space = parent.get(Prop::WHITE_SPACE).unwrap_or("normal");
                        let keep = matches!(space, "pre" | "pre-wrap" | "break-spaces");
                        let mut chunk = String::with_capacity(current.text.len());
                        for character in current.text.chars() {
                            if keep || character == '\n' && space == "pre-line" {
                                chunk.push(character);
                            } else if matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0c') {
                                if !chunk.ends_with(' ') {
                                    chunk.push(' ');
                                }
                            } else {
                                chunk.push(character);
                            }
                        }
                        if chunk.is_empty() {
                            continue;
                        }
                        if breaks > 0 && !text.is_empty() {
                            if collapsed {
                                text.truncate(text.trim_end_matches(' ').len());
                            }
                            text.extend(std::iter::repeat_n('\n', breaks));
                        }
                        breaks = 0;
                        let chunk = if !keep && (text.is_empty() || text.ends_with([' ', '\n'])) {
                            chunk.trim_start_matches(' ')
                        } else {
                            &chunk
                        };
                        text.push_str(chunk);
                        collapsed = !keep;
                        continue;
                    }
                    if node != id {
                        breaks = breaks.max(required);
                    }
                    pending.push((node, true));
                    pending.extend(
                        computed
                            .children
                            .get(&node)
                            .unwrap_or(&current.children)
                            .iter()
                            .rev()
                            .map(|&child| (child, false)),
                    );
                }
                if collapsed {
                    text.truncate(text.trim_end_matches(' ').len());
                }
                Ok(json!(text))
            }
            _ => unreachable!(),
        }
    })())
}

// WARNING: pages parse this JSON text, which costs a fraction of building and serializing JSON values for every node.
fn tree(doc: &Document, root: usize, inclusive: bool) -> Value {
    let mut list = Vec::with_capacity(1024);
    let mut pending: Vec<_> = if inclusive {
        vec![root]
    } else {
        doc.nodes[root].children.iter().rev().copied().collect()
    };
    while let Some(id) = pending.pop() {
        let node = &doc.nodes[id];
        // WARNING: script and style sources can be large, so pages read them with `dom.source` when needed.
        let lazy = node.stylesheet.is_some() && node.tag == "#text"
            || !node.text.is_empty()
                && node.parent.is_some_and(|parent| {
                    matches!(doc.nodes[parent].tag.as_str(), "script" | "style")
                });
        let text = (!lazy).then_some(&node.text);
        let count = node.attrs.iter().count();
        // WARNING: each part is written as a JSON array, whose brackets become the commas of one flat list.
        let at = list.len();
        let _ = serde_json::to_writer(&mut list, &(id, node.parent, &node.tag, text, count));
        if at > 0 {
            list[at] = b',';
        }
        list.pop();
        for pair in &node.attrs {
            let at = list.len();
            let _ = serde_json::to_writer(&mut list, &pair);
            list[at] = b',';
            list.pop();
        }
        pending.extend(node.children.iter().rev());
    }
    if list.is_empty() {
        list.push(b'[');
    }
    list.push(b']');
    Value::String(String::from_utf8(list).unwrap_or_default())
}

fn scroll_offset(
    handle: &renderer::ScrollHandle,
    mut offset: renderer::Point<renderer::Pixels>,
) -> renderer::Point<renderer::Pixels> {
    let bounds = handle.bounds();
    if bounds.size.width > renderer::px(0.) || bounds.size.height > renderer::px(0.) {
        let max = handle.max_offset();
        offset.x = offset.x.clamp(-max.x, renderer::px(0.));
        offset.y = offset.y.clamp(-max.y, renderer::px(0.));
    }
    offset
}

pub(crate) fn add_font(state: &mut State, family: &str, bytes: Vec<u8>) -> Result<String> {
    const LIMIT: usize = 32 * 1024 * 1024;
    let system = state
        .text_system
        .as_ref()
        .context("Font loading requires a displayed window")?;
    ensure!(bytes.len() <= LIMIT, "Font exceeds 32 MB limit");
    // WARNING: fontdb and the text systems read sfnt data only, so WOFF and WOFF2 fonts are decoded first.
    let bytes = match bytes.get(..4) {
        Some(b"\0\x01\0\0" | b"true" | b"OTTO" | b"ttcf") => bytes,
        #[cfg(target_os = "macos")]
        Some(b"wOFF" | b"wOF2") => {
            #[allow(improper_ctypes)]
            unsafe extern "C" {
                fn apps_font_sfnt(
                    bytes: *const u8,
                    length: usize,
                    allocate: extern "C" fn(&mut Vec<u8>, usize) -> *mut u8,
                    font: &mut Vec<u8>,
                ) -> bool;
            }
            extern "C" fn allocate(font: &mut Vec<u8>, length: usize) -> *mut u8 {
                if length > LIMIT {
                    return std::ptr::null_mut();
                }
                *font = vec![0; length];
                font.as_mut_ptr()
            }
            let mut font = Vec::new();
            ensure!(
                unsafe { apps_font_sfnt(bytes.as_ptr(), bytes.len(), allocate, &mut font) },
                "Invalid or too large WOFF font"
            );
            font
        }
        #[cfg(not(target_os = "macos"))]
        Some(magic @ (b"wOFF" | b"wOF2")) => {
            let font = if magic == b"wOFF" {
                wuff::decompress_woff1(&bytes)
            } else {
                wuff::decompress_woff2(&bytes)
            }
            .map_err(|_| anyhow::anyhow!("Invalid WOFF font"))?;
            ensure!(font.len() <= LIMIT, "Font exceeds 32 MB limit");
            font
        }
        _ => anyhow::bail!("Unsupported font format"),
    };
    let mut fonts = crate::render::canvas::fonts(false);
    let database = std::sync::Arc::make_mut(&mut fonts.0);
    let ids = database.load_font_source(usvg::fontdb::Source::Binary(std::sync::Arc::new(
        bytes.clone(),
    )));
    let face = ids
        .first()
        .and_then(|id| database.face(*id))
        .context("Unsupported or invalid font data")?;
    let name = face
        .families
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(family))
        .or(face.families.first())
        .context("Font has no family name")?
        .0
        .clone();
    system.add_fonts(vec![std::borrow::Cow::Owned(bytes)])?;
    use_font(state, family, &name, true);
    Ok(name)
}

pub(crate) fn local_font(state: &mut State, family: &str, name: &str) -> bool {
    let Some(system) = state.text_system.clone() else {
        return false;
    };
    style::font_names(&system, &state.font_aliases);
    let Some(found) =
        style::FONT_NAMES.with(|names| names.borrow().0.get(&name.to_ascii_lowercase()).cloned())
    else {
        return false;
    };
    use_font(state, family, &found, false);
    true
}

pub(crate) fn use_font(state: &mut State, family: &str, name: &str, added: bool) {
    let same = name.eq_ignore_ascii_case(family);
    if !added && (same || state.font_aliases.contains_key(family)) {
        return;
    }
    state.font_loads += 1;
    style::FONT_NAMES.with(|names| *names.borrow_mut() = Default::default());
    // WARNING: a new style cache would drop constructed and CSSOM-edited sheets, so only the values that used font metrics go.
    state.style_cache.metrics = None;
    state.invalidate();
    if !same {
        state
            .font_aliases
            .insert(family.to_owned(), name.to_owned());
    }
    state.dirty = true;
}

pub(crate) fn layout_flags(doc: &mut Document, state: &mut State, viewport: [f32; 2]) {
    let system = state.text_system.clone();
    let computed = styles(
        doc,
        state,
        viewport,
        system.as_deref().map(|system| (system, state.pixel_ratio)),
    );
    if state.layout_epoch == state.style_epoch {
        return;
    }
    state.layout_epoch = state.style_epoch;
    let mut known = rustc_hash::FxHashMap::default();
    for (id, style) in computed.values.iter().enumerate() {
        let bits = *known.entry(Rc::as_ptr(style)).or_insert_with(|| {
            let clip = [Prop::OVERFLOW, Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                .into_iter()
                .any(|prop| {
                    style
                        .get(prop)
                        .is_some_and(|value| matches!(value, "hidden" | "clip" | "auto" | "scroll"))
                });
            let position = style.get(Prop::POSITION).unwrap_or("static");
            // WARNING: like CSSOM `offsetParent`, any box that contains its absolute descendants counts.
            let positioned = crate::render::effects::establishes(style).absolute;
            (if clip { MIRROR_CLIP } else { 0 })
                | if positioned { MIRROR_POSITIONED } else { 0 }
                | if position == "fixed" { MIRROR_FIXED } else { 0 }
        });
        state.bounds.reserve(id);
        unsafe {
            let flags = state.bounds.flags(id);
            *flags =
                (*flags & (MIRROR_BOUNDS | MIRROR_SCROLLER | MIRROR_HIDDEN | MIRROR_INERT)) | bits;
        }
    }
}

pub(crate) fn flat_parent(
    doc: &Document,
    state: &State,
    styles: &style::ComputedStyles,
    id: usize,
) -> Option<usize> {
    match doc.nodes[id].parent {
        Some(parent) if state.shadows.contains_key(&parent) => styles
            .children
            .iter()
            .find_map(|(&slot, kids)| (slot != parent && kids.contains(&id)).then_some(slot)),
        Some(parent) => Some(parent),
        None => state.hosts.get(&id).copied(),
    }
}

pub(crate) fn hit_test(
    doc: &Document,
    state: &State,
    computed: &style::ComputedStyles,
    viewport: [f32; 2],
    [x, y]: [f32; 2],
    first: bool,
    caret: bool,
) -> Value {
    let mut nearest = Vec::new();
    let screen = [0., 0., viewport[0], viewport[1]];
    let up = |id: usize| {
        doc.nodes[id]
            .parent
            .or_else(|| state.hosts.get(&id).copied())
    };
    // WARNING: a box leaving the top layer still paints there, and takes hits there while the cascade shows it.
    let top: Vec<usize> = state
        .top_layer
        .iter()
        .copied()
        .filter(|&id| {
            doc.nodes[id].state & (TOP_LAYER | LEAVING) != 0
                && doc.nodes[id].tag != "html"
                && std::iter::successors(Some(id), |&id| up(id)).all(|id| {
                    computed
                        .values
                        .get(id)
                        .is_none_or(|style| style.get(Prop::DISPLAY) != Some("none"))
                })
                && std::iter::successors(Some(id), |&id| up(id)).last() == Some(0)
        })
        .collect();
    let mut pending = vec![(0, screen, false)];
    pending.extend(top.iter().map(|&id| (id, screen, false)));
    let mut hits = Vec::new();
    while let Some((id, mut clip, hit)) = pending.pop() {
        if hit {
            hits.push(id);
            continue;
        }
        let node = &doc.nodes[id];
        // WARNING: the styles can be older than the tree, from the last frame. Every box inside an inert one is
        // inert too, but top layer boxes, which start their own walk.
        let Some(style) = computed.values.get(id) else {
            continue;
        };
        if state.inert(id)
            || style
                .get(Prop::DISPLAY)
                .is_some_and(|value| value == "none")
        {
            continue;
        }
        if let Some(&[left, top, width, height]) = state.bounds.get(&id) {
            let selectable = !style
                .get(Prop::POINTER_EVENTS)
                .is_some_and(|value| value == "none")
                && !style
                    .get(Prop::VISIBILITY)
                    .is_some_and(|value| matches!(value, "hidden" | "collapse"));
            if caret
                && node.tag == "#text"
                && selectable
                && height > 0.
                && x >= clip[0]
                && x < clip[2]
                && y >= clip[1]
                && y < clip[3]
            {
                let distance = (left - x).max(0.).max(x - left - width).powi(2)
                    + (top - y).max(0.).max(y - top - height).powi(2);
                nearest.push((id, distance));
            }
            if (caret || !node.tag.starts_with('#'))
                && width > 0.
                && height > 0.
                && x >= left
                && x < left + width
                && y >= top
                && y < top + height
                && x >= clip[0]
                && x < clip[2]
                && y >= clip[1]
                && y < clip[3]
                && selectable
            {
                pending.push((id, clip, true));
            }
            for (axis, prop) in [(0, Prop::OVERFLOW_X), (1, Prop::OVERFLOW_Y)] {
                if style
                    .get(prop)
                    .or_else(|| style.get(Prop::OVERFLOW))
                    .is_some_and(|value| matches!(value, "hidden" | "clip" | "auto" | "scroll"))
                {
                    clip[axis] = clip[axis].max([left, top][axis]);
                    clip[axis + 2] = clip[axis + 2].min([left + width, top + height][axis]);
                }
            }
        }
        if x >= clip[0] && x < clip[2] && y >= clip[1] && y < clip[3] {
            let children = computed
                .children
                .get(&id)
                .unwrap_or(&doc.nodes[id].children);
            pending.extend(
                children
                    .iter()
                    .filter(|child| !top.contains(child))
                    .map(|&child| (child, clip, false)),
            );
        }
    }
    hits.sort_by_key(|&id| {
        std::cmp::Reverse(
            std::iter::successors(Some(id), |&id| doc.nodes[id].parent)
                .find_map(|id| state.painted.get(id).copied().filter(|&order| order > 0))
                .unwrap_or(0),
        )
    });
    for &id in top.iter().rev() {
        if hits.first().is_some_and(|&hit| {
            std::iter::successors(Some(hit), |&hit| up(hit)).any(|hit| hit == id)
        }) {
            break;
        }
        if !state.inert(id)
            && computed.backdrops.get(&id).is_some_and(|backdrop| {
                backdrop.get(Prop::POINTER_EVENTS) != Some("none")
                    && backdrop
                        .get(Prop::VISIBILITY)
                        .is_none_or(|value| value == "visible")
                    && backdrop.get(Prop::DISPLAY) != Some("none")
            })
            && x >= 0.
            && y >= 0.
            && x < viewport[0]
            && y < viewport[1]
        {
            hits.insert(0, id);
            break;
        }
    }
    if caret {
        if let Some(&id) = hits.first()
            && doc.nodes[id].tag == "#text"
        {
            return json!(id);
        }
        let target = hits.first().copied();
        let text = nearest
            .into_iter()
            .filter(|(id, _)| {
                let mut parent = doc.nodes[*id].parent;
                while let Some(id) = parent {
                    if Some(id) == target {
                        return true;
                    }
                    parent = doc.nodes[id].parent;
                }
                false
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id);
        return json!(text.or(target));
    }
    if first && let Some(&id) = hits.first() {
        return json!(id);
    }
    if first && x >= 0. && y >= 0. && x < viewport[0] && y < viewport[1] {
        return json!(
            doc.nodes[0]
                .children
                .iter()
                .find(|&&id| !doc.nodes[id].tag.starts_with('#'))
        );
    }
    if first { Value::Null } else { json!(hits) }
}

pub(crate) fn styles(
    doc: &mut Document,
    state: &mut State,
    viewport: [f32; 2],
    system: Option<(&renderer::TextSystem, f32)>,
) -> Rc<style::ComputedStyles> {
    let (system, scale) = (system.map(|system| system.0), system.map(|system| system.1));
    let reduced_motion = style::reduced_motion();
    let theme = crate::desktop::native::theme_state()
        | ((state.pixel_ratio.clamp(0., 600.) * 100.).round() as u32) << 16;
    let animated = std::mem::take(&mut state.animated);
    let animating = !animated.is_empty();
    if !animating
        && state.style_viewport == viewport
        && state.style_reduced_motion == reduced_motion
        && state.style_theme == theme
        && state.style_metrics == scale
        && let Some(computed) = &state.computed
        && computed.values.len() == doc.nodes.len()
    {
        return computed.clone();
    }
    state.drop_removed(doc);
    if let Some(styles) = state
        .old_styles
        .take()
        .and_then(|styles| Rc::try_unwrap(styles).ok())
    {
        state.style_cache.spare = Some(styles.into());
    }
    if state.shadow_walk.tree != doc.nodes.tree() || state.shadow_walk.count != state.shadows.len()
    {
        let slots = slots(doc, state.shadows.iter().map(|(&host, &root)| (host, root)));
        let assigned: rustc_hash::FxHashSet<usize> = slots.values().flatten().copied().collect();
        let mut composed = rustc_hash::FxHashMap::default();
        for (&host, &root) in &state.shadows {
            composed.insert(
                host,
                std::iter::once(root)
                    .chain(
                        doc.nodes[host]
                            .children
                            .iter()
                            .copied()
                            .filter(|child| !assigned.contains(child)),
                    )
                    .collect(),
            );
        }
        for (&slot, nodes) in &slots {
            if !nodes.is_empty() {
                composed.insert(
                    slot,
                    nodes
                        .iter()
                        .chain(&doc.nodes[slot].children)
                        .copied()
                        .collect(),
                );
            }
        }
        let old = &state.shadow_walk.shadows.hosts;
        let hosts = if old.len() == state.hosts.len()
            && state
                .hosts
                .iter()
                .all(|(root, host)| old.get(root) == Some(host))
        {
            old.clone()
        } else {
            Rc::new(
                state
                    .hosts
                    .iter()
                    .map(|(&root, &host)| (root, host))
                    .collect(),
            )
        };
        state.shadow_walk = Rc::new(ShadowWalk {
            tree: doc.nodes.tree(),
            count: state.shadows.len(),
            slots,
            shadows: Rc::new(style::Shadows {
                roots: state.shadows.clone(),
                hosts,
                composed,
            }),
        });
    }
    let shadow_walk = state.shadow_walk.clone();
    let mut context = style::StyleContext {
        shadows: shadow_walk.shadows.clone(),
        reduced_motion,
        theme,
        scale: scale.unwrap_or(1.),
        aliases: state.font_aliases.clone(),
        animated,
        inserted: state
            .inserted
            .iter()
            .map(|(_, sheet, user)| (sheet.clone(), *user))
            .collect(),
        previous: state.computed.clone().or_else(|| state.previous.take()),
        top_layer: !state.top_layer.is_empty(),
    };
    let mut result = style::compute_styles(
        doc,
        viewport[0],
        viewport[1],
        system,
        &mut state.sheets,
        &mut state.style_cache,
        Some(&context),
    );
    state.style_cache.finish();
    state.sheets.retain(|text, _| Rc::strong_count(text) > 1);
    for (&host, &root) in &state.shadows {
        result
            .children
            .insert(host, doc.nodes[root].children.clone());
    }
    for (&slot, assigned) in &shadow_walk.slots {
        if !assigned.is_empty() {
            result.children.insert(slot, assigned.clone());
        }
    }
    if state
        .style_walk
        .update(doc, &result.children, state.style_cache.document_walk())
    {
        state.editable.clear();
        state.inert.clear();
        for &(id, _) in state.style_walk.nodes.iter() {
            let node = &doc.nodes[id as usize];
            if node.attrs.contains_key("inert") {
                state.inert.push(id as usize);
            }
            if matches!(node.tag.as_str(), "input" | "textarea")
                || node.attrs.get("contenteditable").is_some_and(|value| {
                    matches!(
                        value.to_ascii_lowercase().as_str(),
                        "" | "true" | "plaintext-only"
                    )
                })
            {
                state.editable.push(id as usize);
            }
        }
    }
    // WARNING: like HTML's "blocked by a modal dialog", the topmost modal dialog, or else the fullscreen element,
    // makes every node but its flat tree descendants inert. It escapes `inert` ancestors, as in Chromium.
    let subject = [MODAL, FULLSCREEN].into_iter().find_map(|bit| {
        state
            .top_layer
            .iter()
            .rev()
            .copied()
            .find(|&id| doc.nodes.get(id).is_some_and(|node| node.state & bit != 0))
    });
    let marks = &mut state.select_marks;
    marks.clear();
    marks.resize(doc.nodes.len(), 0);
    for &id in &state.editable {
        marks[id] = EDITABLE;
    }
    for &id in &state.inert {
        marks[id] |= INERT;
    }
    state.bounds.reserve(doc.nodes.len().saturating_sub(1));
    for &(id, parent) in state.style_walk.nodes.iter() {
        let id = id as usize;
        let (inherited, hidden, inert) = match parent {
            u32::MAX => ((style::UserSelect::Text, 0), false, false),
            parent => (
                result.user_select[parent as usize],
                marks[parent as usize] & HIDDEN != 0,
                Some(id) != subject
                    && (parent == 0 && subject.is_some() || marks[parent as usize] & INERT != 0),
            ),
        };
        let flags = result.flags[id];
        let hidden = hidden || flags & style::DISPLAY_NONE != 0;
        let inert = inert || marks[id] & INERT != 0;
        marks[id] |= u8::from(hidden) * HIDDEN | u8::from(inert) * INERT;
        unsafe {
            let mirror = state.bounds.flags(id);
            *mirror = *mirror & !MIRROR_INERT | u8::from(hidden || inert) * MIRROR_INERT;
        }
        let mode = if hidden || flags & style::INVISIBLE != 0 {
            style::UserSelect::None
        } else if marks[id] & EDITABLE != 0 {
            style::UserSelect::Text
        } else if flags & style::SELECT_NONE != 0 {
            style::UserSelect::None
        } else if flags & style::SELECT_ALL != 0 {
            style::UserSelect::All
        } else if flags & style::SELECT_TEXT != 0 {
            style::UserSelect::Text
        } else {
            inherited.0
        };
        result.user_select[id] = (mode, if mode == inherited.0 { inherited.1 } else { id });
    }
    if state
        .style_walk
        .nodes
        .iter()
        .any(|&(id, _)| result.flags[id as usize] & style::DISPLAY_CONTENTS != 0)
    {
        let contents = |child: usize| result.flags[child] & style::DISPLAY_CONTENTS != 0;
        let mut parents = vec![false; doc.nodes.len()];
        for &(id, parent) in state.style_walk.nodes.iter() {
            if parent != u32::MAX && contents(id as usize) {
                parents[parent as usize] = true;
            }
        }
        for &(id, _) in state.style_walk.nodes.iter().rev() {
            let id = id as usize;
            if parents[id] {
                let children = result.children.get(&id).unwrap_or(&doc.nodes[id].children);
                let mut flattened = Vec::new();
                for &child in children {
                    if contents(child) {
                        flattened.extend(
                            result
                                .children
                                .get(&child)
                                .unwrap_or(&doc.nodes[child].children),
                        );
                    } else {
                        flattened.push(child);
                    }
                }
                result.children.insert(id, flattened);
            }
        }
    }
    if animating {
        return Rc::new(result);
    }
    state.animating.clear();
    state.animating_pseudos.clear();
    state.style_epoch += 1;
    state.style_viewport = viewport;
    state.style_metrics = scale;
    state.style_reduced_motion = reduced_motion;
    state.style_theme = theme;
    let result = Rc::new(result);
    state.linted = false;
    state.computed = Some(result.clone());
    state.previous = None;
    state.old_styles = context.previous.take();
    result
}

fn path(value: &Value) -> Vec<usize> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| Some(item.as_u64()? as usize))
                .collect()
        })
        .unwrap_or_default()
}

fn edit<'a>(kind: &str, text: &'a str) -> style::Edit<'a> {
    match kind {
        "insert" => style::Edit::Insert(text),
        "style" => style::Edit::Declarations(text),
        "selector" => style::Edit::Selector(text),
        "keyframe" => style::Edit::Keyframe(text),
        _ => style::Edit::Delete,
    }
}

pub(crate) fn bullets(value: &str, offset: usize) -> usize {
    value[..utf16_index(value, offset)].chars().count()
}

pub(crate) fn unbullet(value: &str, bullets: usize) -> usize {
    value.chars().take(bullets).map(char::len_utf16).sum()
}

pub(crate) fn shown_index(source: &str, shown: &str, offset: usize) -> usize {
    if source.is_empty() || source == shown {
        return utf16_index(shown, offset);
    }
    let mut rest = shown.char_indices().peekable();
    let mut units = 0;
    for c in source.chars() {
        if units >= offset {
            break;
        }
        units += c.len_utf16();
        if rest
            .peek()
            .is_some_and(|&(_, next)| !c.is_ascii_whitespace() || next == ' ' || next == c)
        {
            rest.next();
        }
    }
    rest.peek().map_or(shown.len(), |&(byte, _)| byte)
}

pub(crate) fn source_offset(source: &str, shown: &str, byte: usize) -> usize {
    if source.is_empty() || source == shown {
        return shown[..byte].encode_utf16().count();
    }
    let (mut at, mut units) = (0, 0);
    for c in source.chars() {
        if at >= byte {
            break;
        }
        units += c.len_utf16();
        if let Some(next) = shown[at..].chars().next()
            && (!c.is_ascii_whitespace() || next == ' ' || next == c)
        {
            at += next.len_utf8();
        }
    }
    units
}

pub(crate) fn utf16_index(text: &str, index: usize) -> usize {
    let mut offset = 0;
    for (byte, c) in text.char_indices() {
        if offset >= index {
            return byte;
        }
        offset += c.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    #[test]
    fn sf_symbol_names_require_nonempty_ascii_segments() {
        for (tag, expected) in [
            ("sf-symbol-folder", Some("folder")),
            ("sf-symbol-square-and-arrow-up", Some("square-and-arrow-up")),
            ("sf-symbol-4k-tv", Some("4k-tv")),
            ("sf-symbol-arrow_uturn-right", Some("arrow_uturn-right")),
            ("sf-symbol-", None),
            ("sf-symbol--folder", None),
            ("sf-symbol-folder-", None),
            ("sf-symbol-folder--fill", None),
            ("sf-symbol-folder.fill", None),
            ("sf-symbol-étoile", None),
            ("sf-icon-folder", None),
            ("span", None),
        ] {
            let node = super::Node {
                tag: tag.into(),
                ..Default::default()
            };
            assert_eq!(node.sf_symbol(), expected, "{tag}");
        }
    }

    #[test]
    fn offsets_map_through_collapsed_white_space() {
        let (source, shown) = ("  a  b\n c ", "a b c");
        for (offset, byte) in [
            (0, 0),
            (2, 0),
            (3, 1),
            (5, 2),
            (6, 3),
            (8, 4),
            (9, 5),
            (10, 5),
        ] {
            assert_eq!(super::shown_index(source, shown, offset), byte, "{offset}");
        }
        assert_eq!(super::source_offset(source, shown, 0), 0);
        assert_eq!(super::source_offset(source, shown, 1), 3);
        assert_eq!(super::source_offset(source, shown, 2), 4);
        assert_eq!(super::source_offset(source, shown, 3), 6);
        assert_eq!(super::shown_index("héllo", "héllo", 2), 3);
        assert_eq!(super::shown_index("", "abc", 2), 2);
    }

    #[test]
    fn kept_containers_keep_their_sizes_between_frames() {
        let mut state = super::State::default();
        state.containers.insert(1, [150., 20.]);
        state.update_containers(&Document::default());
        assert_eq!(state.containers.get(&1), Some(&[150., 20.]));
    }

    #[test]
    fn password_fields_show_a_bullet_per_code_point() {
        let doc = Document::parse("<input type=password><input>");
        let [password, text] = [1, 2].map(|nth| {
            doc.nodes
                .iter()
                .filter(|node| node.tag == "input")
                .nth(nth - 1)
                .unwrap()
        });
        let value = "a\u{1F600}b";
        assert_eq!(password.shown_text(value.into()), "\u{2022}".repeat(3));
        assert_eq!(text.shown_text(value.into()), value);
        assert_eq!(
            [0, 1, 2, 3, 4].map(|offset| bullets(value, offset)),
            [0, 1, 2, 2, 3]
        );
        assert_eq!(
            [0, 1, 2, 3].map(|index| unbullet(value, index)),
            [0, 1, 3, 4]
        );
    }

    #[test]
    fn layout_count_wraps_before_f32_stops_counting() {
        let mut mirror = LayoutMirror::default();
        mirror.touch();
        let count =
            |mirror: &LayoutMirror| unsafe { mirror.memory.as_ref().unwrap().0.as_ptr().add(2) };
        assert_eq!(unsafe { *count(&mirror) }, 1.);
        unsafe { *count(&mirror) = 16_777_215. };
        mirror.touch();
        assert_eq!(unsafe { *count(&mirror) }, 0.);
    }

    #[test]
    fn font_faces_load_woff_and_list_css_faces() {
        use crate::{
            host::{Host, Page, Role},
            jsc::{JavaScriptCore, JsContext},
        };
        use std::cell::RefCell;

        let fixtures = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fonts"));
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut()
            .assets
            .insert("test.woff".into(), fixtures.join("test.woff"));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse(
                    r#"<style>@font-face{font-family:Css;src:url(test.woff) format("woff")}
                    @font-face{font-family:Later;src:url(later.woff);unicode-range:U+0400-04FF}</style><p>AB</p>"#,
                ),
                dirty: false,
            },
        );
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        {
            let mut host = host.borrow_mut();
            let state = host.dom.entry(1).or_default();
            state.text_system = Some(Rc::new(renderer::TextSystem::new()));
            state.base = url::Url::parse("apps:///index.html").ok();
        }
        let bytes = std::fs::read(fixtures.join("test-cff.woff2")).unwrap();
        let bytes: Vec<_> = bytes.iter().map(u8::to_string).collect();
        engine
            .eval(
                &format!(
                    r#"
            globalThis.log = [];
            const face = new FontFace('Bytes', new Uint8Array([{}]));
            document.fonts.add(face);
            face.load().then(() => log.push('bytes ' + face.status));
            new FontFace('Bad', 'url(a.eot) format("embedded-opentype")').load().catch(error => log.push('bad ' + error.name));
            new FontFace('Local', 'local("No Such Font 123")').load().catch(error => log.push('local ' + error.name));
            document.fonts.ready.then(fonts => {{
                log.push([...fonts].map(face => face.family + ' ' + face.status).join());
                const context = document.createElement('canvas').getContext('2d');
                for (const family of ['Bytes', 'Css']) {{ context.font = '10px ' + family; log.push(context.measureText('AB').width); }}
                log.push(document.fonts.check('10px Css'), document.fonts.check('10px Later'), document.fonts.check('10px Later', 'Ж'), document.fonts.has(face));
            }});
        "#,
                    bytes.join(",")
                ),
                "test",
            )
            .unwrap();
        for _ in 0..5 {
            engine.tick().unwrap();
        }
        assert_eq!(
            engine.eval("JSON.stringify(log)", "test").unwrap(),
            r#"["bytes loaded","bad SyntaxError","local NetworkError","Css loaded,Later unloaded,Bytes loaded",15,15,true,true,false,true]"#
        );
    }

    #[test]
    fn detached_nodes_keep_styles_that_a_frame_holds() {
        use crate::{
            host::{Host, Page, Role},
            jsc::{JavaScriptCore, JsContext},
        };
        use std::cell::RefCell;

        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse("<html><body><div id='editor'></div></body></html>"),
                dirty: false,
            },
        );
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        engine
            .eval(
                "const editor=document.getElementById('editor');document.createElement('i');getComputedStyle(editor).color;",
                "test",
            )
            .unwrap();
        let held = host.borrow().dom[&1].computed.clone().unwrap();
        engine
            .eval(
                "for(let i=0;i<20;i++)document.createElement('b').textContent='x';getComputedStyle(editor).color",
                "test",
            )
            .unwrap();
        assert!(Rc::ptr_eq(
            &held,
            host.borrow().dom[&1].computed.as_ref().unwrap()
        ));
    }
    use super::*;

    #[test]
    fn detached_editor_overlays_preserve_connected_styles() {
        use crate::{
            host::{Host, Page, Role},
            jsc::{JavaScriptCore, JsContext},
        };
        use std::cell::RefCell;

        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse(
                    "<html><body><div id='editor' style='color:red'></div></body></html>",
                ),
                dirty: false,
            },
        );
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        engine
            .eval(
                "const editor=document.getElementById('editor');getComputedStyle(editor).color;",
                "test",
            )
            .unwrap();
        let cached = Rc::as_ptr(host.borrow().dom[&1].computed.as_ref().unwrap());
        engine
            .eval(
                "editor.setAttribute('style','color:red');editor.removeAttribute('missing')",
                "test",
            )
            .unwrap();
        assert_eq!(
            cached,
            Rc::as_ptr(host.borrow().dom[&1].computed.as_ref().unwrap())
        );
        engine
            .eval(
                r#"
            const fragment=document.createDocumentFragment();
            for(let i=0;i<100;i++) {
                const overlay=document.createElement('div');
                overlay.style.cssText='position:absolute;color:blue';
                overlay.dataset.selectionRange='';
                overlay.textContent='selection';
                fragment.append(overlay);
                if(getComputedStyle(editor).color!=='rgb(255, 0, 0)')throw Error('Connected style changed');
            }
        "#,
                "test",
            )
            .unwrap();
        assert_eq!(
            cached,
            Rc::as_ptr(host.borrow().dom[&1].computed.as_ref().unwrap())
        );
        engine.eval(r#"
            editor.append(fragment);
            if(getComputedStyle(editor.firstChild).color!=='rgb(0, 0, 255)')throw Error('Inserted style was stale');
            fragment.append(editor.firstChild);
        "#, "test").unwrap();
        assert!(host.borrow().dom[&1].computed.is_none());
        engine.eval(r#"
            const root=editor.attachShadow({mode:'open'});
            root.innerHTML='<span style="color:green">text</span>';
            if(getComputedStyle(root.firstChild).color!=='rgb(0, 128, 0)')throw Error('Shadow style');
            root.firstChild.style.color='purple';
            if(getComputedStyle(root.firstChild).color!=='rgb(128, 0, 128)')throw Error('Shadow mutation was stale');
        "#, "test").unwrap();
    }

    #[test]
    fn invalidation_skips_text_edits_and_unused_states() {
        use crate::{
            host::{Host, Page, Role},
            jsc::{JavaScriptCore, JsContext},
        };
        use std::cell::RefCell;

        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse(
                    "<html><head><style id=s>p:empty{color:red}</style></head><body><p id=p>text</p><svg><text>a</text></svg></body></html>",
                ),
                dirty: false,
            },
        );
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        let computed = || host.borrow().dom[&1].computed.as_ref().map(Rc::as_ptr);
        let p = host.borrow().pages[&1]
            .doc
            .nodes
            .iter()
            .position(|node| node.tag == "p")
            .unwrap();
        let revision = || host.borrow().pages[&1].doc.nodes.latest();
        let hover = |ids: &[usize]| {
            host.borrow_mut()
                .call(Role::Renderer(1), "dom.hover", json!([ids]))
                .unwrap();
        };
        engine
            .eval(
                "const p=document.getElementById('p');getComputedStyle(p).color;",
                "test",
            )
            .unwrap();
        let cached = computed();
        assert!(cached.is_some());
        let built = revision();
        engine
            .eval(
                "p.firstChild.data='other';document.querySelector('svg text').firstChild.data='b';",
                "test",
            )
            .unwrap();
        hover(&[p]);
        hover(&[]);
        assert_eq!(computed(), cached);
        assert_ne!(revision(), built);
        engine
            .eval(
                r#"
            p.firstChild.data='';
            if(getComputedStyle(p).color!=='rgb(255, 0, 0)')throw Error('Empty text was stale');
            const sheet=document.getElementById('s');
            sheet.textContent='p:hover{color:blue}';
            sheet.firstChild.data='p:hover{color:green}';
            if(getComputedStyle(p).color==='rgb(0, 128, 0)')throw Error('Hover without state');
        "#,
                "test",
            )
            .unwrap();
        hover(&[p]);
        assert!(computed().is_none());
        engine
            .eval(
                "if(getComputedStyle(p).color!=='rgb(0, 128, 0)')throw Error('Hover state was stale')",
                "test",
            )
            .unwrap();
        host.borrow_mut()
            .dom
            .get_mut(&1)
            .unwrap()
            .font_aliases
            .insert("Brand".into(), "Real Font".into());
        engine
            .eval("p.style.fontFamily='Brand';if(getComputedStyle(p).fontFamily!=='Real Font')throw Error('Alias')", "test")
            .unwrap();
        let first = host.borrow().dom[&1].computed.as_ref().unwrap().values[p].clone();
        host.borrow_mut().dom.get_mut(&1).unwrap().computed = None;
        engine.eval("getComputedStyle(p).color", "test").unwrap();
        assert!(Rc::ptr_eq(
            &first,
            &host.borrow().dom[&1].computed.as_ref().unwrap().values[p]
        ));
    }

    #[test]
    fn incremental_styles_match_full_passes() {
        use crate::{
            host::{Host, Page, Role},
            jsc::{JavaScriptCore, JsContext},
        };
        use std::cell::RefCell;

        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse(
                    "<html><head><style>
                    html{font-size:16px} .on{color:red} .dark .leaf{color:blue} .a+.b{color:green}
                    .box:has(.on){background-color:yellow} li:nth-child(2n){color:purple} p:empty{color:teal}
                    .rem{padding-left:2rem} .em{padding-top:2em} .vars{color:var(--c,gray)} .x::before{content:'a'}
                    .s::selection{color:white} .h:hover{color:orange} .none{display:none} [data-k=v]{color:navy}
                    </style></head><body><div id=root class=box><span id=leaf class=leaf>x</span>
                    <i class=a id=a></i><i class=b></i><ul id=list><li>1</li><li>2</li><li>3</li></ul>
                    <p id=p>t</p><div id=sized class=em><b class=rem>r</b><b class=em>e</b></div>
                    <div id=vars class=vars><span class=vars>v</span></div><div id=hover class='h s'>h</div>
                    <div id=k>k</div></div></body></html>",
                ),
                dirty: false,
            },
        );
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        let hover = host.borrow().pages[&1]
            .doc
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").is_some_and(|id| id == "hover"))
            .unwrap();
        let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
        engine
            .eval(&format!("for(const id of ['root','leaf','a','list','p','sized','vars','hover','k'])globalThis[id]=document.getElementById(id);{read}"), "test")
            .unwrap();
        for step in [
            "leaf.classList.add('on')",
            "root.classList.add('dark')",
            "a.classList.remove('a')",
            "a.classList.add('a')",
            "leaf.classList.remove('on')",
            "list.prepend(document.createElement('li'))",
            "list.firstChild.remove()",
            "p.firstChild.data=''",
            "document.documentElement.style.fontSize='20px'",
            "sized.style.fontSize='30px'",
            "vars.style.setProperty('--c','red')",
            "vars.firstChild.style.setProperty('--c','blue')",
            "leaf.classList.add('x')",
            "hover.classList.remove('s')",
            "hover.classList.add('none')",
            "hover.classList.remove('none')",
            "k.dataset.k='v'",
            "list.append(sized)",
            "root.prepend(list)",
            "root.removeChild(p)",
            "root.append(p)",
            "document.documentElement.style.fontSize=''",
        ] {
            engine.eval(&format!("{step};{read}"), "test").unwrap();
        }
        for ids in [vec![hover], vec![]] {
            host.borrow_mut()
                .call(Role::Renderer(1), "dom.hover", json!([ids]))
                .unwrap();
            engine.eval(read, "test").unwrap();
        }
        assert!(host.borrow().dom[&1].style_cache.incremental >= 20);
    }

    #[test]
    fn incremental_styles_follow_class_changes() {
        use crate::{
            host::{Host, Page, Role},
            jsc::{JavaScriptCore, JsContext},
        };
        use std::cell::RefCell;

        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse(
                    "<html><head><style>
                    .on{color:red} .dark .leaf{color:blue} .a+.b{color:green} .box:has(.hit){background-color:yellow}
                    li:nth-child(1 of .pick){color:purple} .wrap:not(.off) .inner{color:teal} .nest{& .deep{color:navy}}
                    .row.sel{color:olive} .row b{padding-left:1px}
                    </style></head><body><div id=root class=box><div id=wrap class=wrap><span id=inner class=inner>i</span></div>
                    <span id=leaf class=leaf>x</span><i id=a></i><i class=b></i><ul><li id=l1>1</li><li id=l2>2</li></ul>
                    <div id=nest><b class=deep>d</b></div>
                    <div id=row class=row><b>r</b></div><em id=hit>h</em></div></body></html>",
                ),
                dirty: false,
            },
        );
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
        engine
            .eval(&format!("for(const id of ['root','wrap','leaf','a','l1','l2','nest','row','hit'])globalThis[id]=document.getElementById(id);{read}"), "test")
            .unwrap();
        for step in [
            "row.classList.add('sel')",
            "row.classList.add('zz')",
            "row.classList.remove('sel')",
            "root.classList.add('dark')",
            "a.classList.add('a')",
            "hit.classList.add('hit')",
            "l2.classList.add('pick')",
            "l1.classList.add('pick')",
            "wrap.classList.add('off')",
            "nest.classList.add('nest')",
            "row.className='other'",
            "root.classList.remove('dark')",
            "wrap.classList.remove('off')",
            "l1.classList.remove('pick')",
            "hit.classList.remove('hit')",
            "leaf.classList.add('on')",
        ] {
            engine.eval(&format!("{step};{read}"), "test").unwrap();
        }
        assert!(host.borrow().dom[&1].style_cache.incremental >= 15);
    }

    #[test]
    fn cousins_that_share_matches_restyle_with_their_own_ancestors() {
        use crate::{
            host::{Host, Page, Role},
            jsc::{JavaScriptCore, JsContext},
        };
        use std::cell::RefCell;

        let rows = "<div class=row><span class=x>1</span></div><div class=row><span class=x>2</span></div>".repeat(2);
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse(&format!(
                    "<html><head><style>.a .x{{color:red}} .g[data-k=v] .x{{padding-left:1px}} .row.on .x{{color:blue}}
                    .g:hover .x{{color:green}}</style></head><body><div id=list><div id=g1 class=g>{rows}</div>
                    <div id=g2 class=g>{rows}</div><div id=g3 class=g>{rows}</div></div></body></html>"
                )),
                dirty: false,
            },
        );
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
        engine
            .eval(&format!("for(const id of ['g1','g2','g3'])globalThis[id]=document.getElementById(id);{read}"), "test")
            .unwrap();
        for step in [
            "g2.classList.add('a')",
            "g3.children[3].classList.add('on')",
            "g1.dataset.k='v'",
            "g2.children[1].classList.add('on')",
            "g2.classList.remove('a')",
            "g3.append(g1.children[0])",
            "g1.dataset.k='w'",
        ] {
            engine.eval(&format!("{step};{read}"), "test").unwrap();
        }
        let g3 = host.borrow().pages[&1]
            .doc
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").is_some_and(|id| id == "g3"))
            .unwrap();
        host.borrow_mut()
            .call(Role::Renderer(1), "dom.hover", json!([[g3]]))
            .unwrap();
        engine.eval(read, "test").unwrap();
        assert!(host.borrow().dom[&1].style_cache.incremental >= 8);
    }

    #[test]
    fn incremental_styles_adjust_collapsed_tables_again() {
        use crate::{
            host::{Host, Page, Role},
            jsc::{JavaScriptCore, JsContext},
        };
        use std::cell::RefCell;

        let rows = "<tr><td>a</td><td>b</td></tr>".repeat(4);
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse(&format!(
                    "<html><head><style>table{{border-collapse:collapse}} td{{border:1px solid #ccc}}
                    tr.danger td{{background-color:#fdd}} tr.thick td{{border-bottom:3px solid red}} tr.wide{{border-top:4px solid blue}}
                    .x{{border-left:5px double green}} .blocky{{display:block}}</style></head><body><table id=t><thead><tr><th>h</th><th>i</th></tr></thead>
                    <tbody id=b>{rows}</tbody></table><table id=u style='border:2px solid'><tr><td>c</td></tr></table></body></html>"
                )),
                dirty: false,
            },
        );
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
        engine
            .eval(&format!("for(const id of ['t','b','u'])globalThis[id]=document.getElementById(id);{read}"), "test")
            .unwrap();
        for step in [
            "b.rows[1].className='danger'",
            "b.rows[1].className='thick'",
            "b.rows[2].className='wide'",
            "b.rows[0].cells[1].className='x'",
            "b.rows[3].cells[0].colSpan=2",
            "b.rows[3].deleteCell(1)",
            "b.insertRow().insertCell().textContent='n'",
            "b.rows[0].remove()",
            "t.style.border='6px solid'",
            "u.rows[0].insertCell().style.border='7px dashed'",
            "b.rows[1].className=''",
            "t.style.borderCollapse='separate'",
            "t.style.borderCollapse=''",
            "t.classList.add('blocky')",
            "t.classList.remove('blocky')",
            "b.rows[0].className='danger'",
        ] {
            engine.eval(&format!("{step};{read}"), "test").unwrap();
        }
        assert!(host.borrow().dom[&1].style_cache.incremental >= 12);
    }

    #[test]
    fn incremental_styles_match_full_passes_in_shadow_trees() {
        use crate::{
            host::{Host, Page, Role},
            jsc::{JavaScriptCore, JsContext},
        };
        use std::cell::RefCell;

        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: Document::parse(
                    "<html><head><style>
                    :root{--ink:navy} x-card.big{font-size:20px} x-card::part(title){color:green}
                    x-card.alt::part(title){color:olive} x-card::part(deep){padding-left:3px}
                    #b{margin-top:1px} .late{color:maroon}
                    </style></head><body><x-card id=card class=c><span id=a class=on>a<s id=s></s></span>
                    <b id=b slot=side>b</b><i id=i>i</i></x-card><x-card id=other></x-card>
                    <x-card id=third></x-card><div id=late class=late><p id=lp>late</p></div></body></html>",
                ),
                dirty: false,
            },
        );
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        let card = host.borrow().pages[&1]
            .doc
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").is_some_and(|id| id == "card"))
            .unwrap();
        let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
        engine
            .eval(&format!(r#"
                for (const id of ['card','a','b','i','other','third','late']) globalThis[id] = document.getElementById(id);
                globalThis.root = card.attachShadow({{mode:'open'}});
                root.innerHTML = '<style>:host{{display:block;color:var(--ink)}} :host(.on){{border-top:1px solid red}} :host(:hover){{color:orange}} ::slotted(.on){{color:red}} slot[name=side]::slotted(*){{padding-top:2px}} .t{{font-weight:700;font-size:2em}} slot{{color:var(--tone,inherit)}} slot.wide::slotted(*){{padding-left:9px}} :host(:has(> .flag)){{padding-right:2px}} ::slotted(:first-child){{padding-bottom:4px}} ::slotted(:has(> .mark)){{padding-right:5px}}</style><h1 part=title class=t id=t>t</h1><slot id=main></slot><slot name=side id=side>fallback</slot><x-inner id=inner exportparts="core: deep"></x-inner>';
                globalThis.inner = root.getElementById('inner').attachShadow({{mode:'open'}});
                inner.innerHTML = '<style>:host{{display:block}} b{{color:var(--ink)}}</style><b part=core id=core>c</b>';
                globalThis.sheet = new CSSStyleSheet();
                sheet.replaceSync(':host{{margin-left:4px}} p{{color:teal}}');
                for (const host of [other, third]) {{ const shadow = host.attachShadow({{mode:'open'}}); shadow.adoptedStyleSheets = [sheet]; shadow.innerHTML = '<p>o</p>'; }}
                {read}"#), "test")
            .unwrap();
        let mut full = Vec::new();
        for step in [
            "card.classList.add('on')",
            "card.classList.add('alt')",
            "card.classList.add('big')",
            "document.documentElement.style.setProperty('--ink','purple')",
            "card.style.setProperty('--ink','teal')",
            "card.style.setProperty('--tone','gray')",
            "a.classList.remove('on')",
            "a.classList.add('on')",
            "root.getElementById('main').classList.add('wide')",
            "root.getElementById('main').classList.remove('wide')",
            "a.classList.add('flag')",
            "a.classList.remove('flag')",
            "card.prepend(document.createElement('u'))",
            "document.getElementById('s').classList.add('mark')",
            "document.getElementById('s').classList.remove('mark')",
            "b.slot=''",
            "b.slot='side'",
            "root.getElementById('side').setAttribute('name','none')",
            "root.getElementById('side').setAttribute('name','side')",
            "root.getElementById('t').setAttribute('part','other')",
            "root.getElementById('t').setAttribute('part','title')",
            "root.getElementById('inner').setAttribute('exportparts','core')",
            "root.getElementById('inner').setAttribute('exportparts','core: deep')",
            "inner.getElementById('core').classList.add('x')",
            "card.append(document.createElement('em'))",
            "root.append(document.createElement('div'))",
            "i.remove()",
            "card.prepend(i)",
            "other.remove()",
            "document.body.append(other)",
            "late.attachShadow({mode:'open'}).adoptedStyleSheets=[sheet]",
            "late.shadowRoot.innerHTML='<slot></slot><p>s</p>'",
            "third.remove()",
            "card.classList.remove('big')",
            "card.classList.remove('on')",
            "sheet.replaceSync(':host{margin-left:5px} p{color:red}')",
            "card.classList.add('on')",
        ] {
            let before = host.borrow().dom[&1].style_cache.incremental;
            engine.eval(&format!("{step};{read}"), "test").unwrap();
            if host.borrow().dom[&1].style_cache.incremental == before {
                full.push(step);
            }
        }
        for ids in [vec![card], vec![]] {
            host.borrow_mut()
                .call(Role::Renderer(1), "dom.hover", json!([ids]))
                .unwrap();
            engine.eval(read, "test").unwrap();
        }
        assert_eq!(
            full,
            ["sheet.replaceSync(':host{margin-left:5px} p{color:red}')"]
        );
        let colors = engine
            .eval(
                "JSON.stringify([getComputedStyle(card).color, getComputedStyle(a).color, getComputedStyle(root.getElementById('t')).color, getComputedStyle(inner.getElementById('core')).paddingLeft, getComputedStyle(late.shadowRoot.querySelector('p')).color])",
                "test",
            )
            .unwrap();
        assert_eq!(
            colors,
            r#"["rgb(0, 128, 128)","rgb(255, 0, 0)","rgb(128, 128, 0)","3px","rgb(255, 0, 0)"]"#
        );
    }

    #[test]
    fn runtime_stylesheets_reuse_parses_and_evict_old_text() {
        let mut doc = Document::parse(
            &(0..200)
                .map(|index| format!("<style>.item{index}{{color:red}}</style>"))
                .collect::<String>(),
        );
        let mut state = State::default();
        styles(&mut doc, &mut state, [800., 600.], None);
        let sheets = state.sheets.clone();
        assert_eq!(sheets.len(), 200);
        state.computed = None;
        styles(&mut doc, &mut state, [800., 600.], None);
        for (source, sheet) in sheets {
            assert!(Rc::ptr_eq(&sheet, &state.sheets[&source]));
        }
        let sheet = doc
            .nodes
            .iter()
            .position(|node| node.tag == "style")
            .unwrap();
        let text = doc.nodes[sheet].children[0];
        doc.nodes[text].text = ".replacement{color:blue}".into();
        state.computed = None;
        styles(&mut doc, &mut state, [800., 600.], None);
        assert_eq!(state.sheets.len(), 200);
        assert!(!state.sheets.contains_key(".item0{color:red}"));
        let head = doc.nodes[sheet].parent.unwrap();
        doc.nodes[head].children.retain(|id| *id != sheet);
        doc.nodes[sheet].parent = None;
        state.computed = None;
        styles(&mut doc, &mut state, [800., 600.], None);
        assert_eq!(state.sheets.len(), 199);
        assert!(!state.sheets.contains_key(".replacement{color:blue}"));
    }

    #[test]
    fn removed_nodes_drop_caches_and_style_again_when_they_return() {
        let mut doc = Document::parse(
            "<style>.on p{color:red}p:first-child{width:1px}</style><div class=on><p>x</p></div><div id=b></div>",
        );
        let find = |doc: &Document, tag: &str| doc.nodes.iter().position(|node| node.tag == tag);
        let p = find(&doc, "p").unwrap();
        let on = doc.nodes[p].parent.unwrap();
        let mut state = State::default();
        let viewport = [800., 600.];
        styles(&mut doc, &mut state, viewport, None);
        doc.nodes[on].children.retain(|&id| id != p);
        doc.nodes[p].parent = None;
        state.removed.push(p);
        state.invalidate();
        styles(&mut doc, &mut state, viewport, None);
        assert!(state.removed.is_empty());
        doc.nodes[on].children.push(p);
        doc.nodes[p].parent = Some(on);
        state.invalidate();
        let returned = styles(&mut doc, &mut state, viewport, None);
        let fresh = styles(&mut doc, &mut State::default(), viewport, None);
        assert_eq!(returned.values[p], fresh.values[p]);
        assert_eq!(returned.values[p].get(Prop::COLOR), Some("red"));
    }

    #[test]
    fn new_nodes_grow_cached_styles_like_a_full_pass() {
        let mut doc = Document::parse("<p style='font-size:30px;--x:1'>a</p>");
        let mut state = State::default();
        let viewport = [800., 600.];
        styles(&mut doc, &mut state, viewport, None);
        let id = state.allocate(
            &mut doc,
            Node {
                tag: "div".into(),
                ..Default::default()
            },
        );
        let read = |doc: &mut Document, state: &mut State| {
            call("dom.computed", &json!([id, "color", ""]), doc, state, viewport)
                .unwrap()
                .unwrap()
        };
        assert_eq!(read(&mut doc, &mut state), json!(""));
        let cached = styles(&mut doc, &mut state, viewport, None);
        let fresh = styles(&mut doc, &mut State::default(), viewport, None);
        assert_eq!(cached.vars.len(), doc.nodes.len());
        for node in 0..doc.nodes.len() {
            assert!(cached.lengths[node] == fresh.lengths[node], "{node}");
        }
        let p = (0..doc.nodes.len())
            .find(|&node| doc.nodes[node].tag == "p")
            .unwrap();
        doc.nodes[p].attrs.insert("class".into(), "x".into());
        state.invalidate();
        styles(&mut doc, &mut state, viewport, None);
    }

    #[test]
    fn trim_cuts_the_free_end_of_the_arena_and_ids_come_back_lowest_first() {
        let mut doc = Document::parse("<div></div>");
        let mut state = State::default();
        let viewport = [800., 600.];
        let node = || Node {
            tag: "p".into(),
            ..Default::default()
        };
        let ids: Vec<usize> = (0..4).map(|_| state.allocate(&mut doc, node())).collect();
        styles(&mut doc, &mut state, viewport, None);
        for id in [ids[3], ids[1], ids[2]] {
            doc.nodes[id] = Node::default();
            state.free.push(std::cmp::Reverse(id));
        }
        assert!(state.trim(&mut doc));
        assert_eq!(doc.nodes.len(), ids[1]);
        assert_eq!(state.free.len(), 0);
        assert!(!state.trim(&mut doc));
        styles(&mut doc, &mut state, viewport, None);
        assert_eq!(state.allocate(&mut doc, node()), ids[1]);
        assert_eq!(state.allocate(&mut doc, node()), ids[1] + 1);
        doc.nodes[ids[1]] = Node::default();
        state.free.push(std::cmp::Reverse(ids[1]));
        doc.nodes[ids[0]] = Node::default();
        state.free.push(std::cmp::Reverse(ids[0]));
        assert_eq!(state.allocate(&mut doc, node()), ids[0]);
    }
}
