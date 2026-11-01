mod document;
mod entities;
mod selector;

pub use document::{Attrs, Document, Node, decode, escape};
pub(crate) use selector::{
    ACTIVE, FOCUS, FOCUS_VISIBLE, FOCUS_WITHIN, HOVER, Selector, SelectorCache, SelectorList,
};

use crate::style;
use crate::style::Prop;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

/// One visual row of wrapped text: its shift for alignment and indent, its right edge, the extra
/// space after each space when justified, its first byte, and how far it moves down.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Row {
    pub shift: renderer::Pixels,
    pub right: renderer::Pixels,
    pub gap: renderer::Pixels,
    pub start: usize,
    /// How far the row moves down: the extra height of the rows above, and its own extra space above.
    pub drop: renderer::Pixels,
    /// The extra space above the row, when its fonts or line heights need more than the paragraph's.
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

/// A decoded raster, or SVG text and its size.
type Decoded = Result<Result<renderer::RenderImage, (String, [f32; 2])>>;

pub(crate) struct LayoutNode {
    pub node: Node,
    /// The document revision that `node` was copied at.
    pub seen: u64,
    pub style: Rc<style::Style>,
    /// A hash of the units the style's lengths resolve with.
    pub units: u64,
    pub children: Vec<(usize, u64)>,
    pub revision: u64,
    /// The revision of the last change that could move or resize a box in the subtree.
    pub geometry: u64,
    /// The culling flags of the box's own style.
    pub flags: u16,
}

/// Node geometry from the last frame, shared with the page through an `ArrayBuffer`: 4 header
/// floats (stale, capacity, layout count), 8 floats per node (x, y, width, height, scroll left, scroll top,
/// scroll range x, scroll range y), then one byte of `MIRROR_*` flags per node.
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

/// Node states that scripts set: a checked checkbox, radio or option, whether that replaces the
/// attribute, and an open modal dialog.
pub(crate) const CHECKED: u8 = 16;
pub(crate) const DIRTY: u8 = 32;
pub(crate) const MODAL: u8 = 64;

impl Node {
    /// Whether a checkbox, radio or option is checked, by script state or else by its attribute.
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

    /// Whether the node is a password field, which shows a bullet for each code point like Chrome.
    pub(crate) fn password(&self) -> bool {
        self.tag == "input"
            && self
                .attrs
                .get("type")
                .is_some_and(|kind| kind.eq_ignore_ascii_case("password"))
    }

    /// The text a field shows, see [`bullets`] for the offsets of a password.
    pub(crate) fn shown_text(&self, text: String) -> String {
        if self.password() {
            "\u{2022}".repeat(text.chars().count())
        } else {
            text
        }
    }

    /// The current value of an `input`, or `None` for a `textarea` that shows its text.
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

    /// Counts a layout in the third header float, so pages know when cached box sizes are old.
    pub(crate) fn touch(&mut self) {
        self.reserve(0);
        if let Some(memory) = &self.memory {
            // WARNING: an f32 stops counting at 2^24, so the count wraps. Pages only compare it for equality.
            unsafe {
                let count = memory.0.as_ptr().add(2);
                *count = (*count + 1.) % 16_777_216.;
            }
        }
    }

    /// Marks the nodes that have no box, where `visible` differs from last time's `old`.
    pub(crate) fn hide(&mut self, visible: &[bool], old: &[bool]) {
        if let Some(last) = visible.len().checked_sub(1) {
            self.reserve(last);
        }
        for (id, &shown) in visible.iter().enumerate() {
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

    /// The memory for a page `ArrayBuffer`, its size and an owner for `release`.
    pub(crate) fn share(&mut self) -> (*mut std::ffi::c_void, usize, *mut std::ffi::c_void) {
        self.reserve(0);
        let memory = self.memory.clone().expect("Layout memory");
        (
            memory.0.as_ptr().cast(),
            (4 + self.capacity * 8) * 4 + self.capacity,
            Arc::into_raw(memory).cast_mut().cast(),
        )
    }

    /// Drops the owner from `share` when JSC collects the buffer, maybe on another thread.
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
    pub(crate) free: Vec<usize>,
    /// Nodes that lost their parent since the last release, as candidate roots of garbage subtrees.
    pub(crate) detached: Vec<usize>,
    live: HashSet<usize>,
    pub bounds: LayoutMirror,
    pub(crate) text_layouts:
        HashMap<usize, (renderer::TextLayout, Rc<str>, std::ops::Range<usize>, Rows)>,
    pub scroll: HashMap<usize, renderer::ScrollHandle>,
    scroll_offsets: HashMap<usize, renderer::Point<renderer::Pixels>>,
    pub(crate) computed: Option<Rc<style::ComputedStyles>>,
    pub(crate) grids: HashMap<
        usize,
        (
            f32,
            Option<f32>,
            Option<crate::render::grid::Grid>,
            crate::render::grid::Grid,
        ),
    >,
    /// Grids sized by a flex layout: per axis, the content size their tracks used and the
    /// padding and borders. Prepaint renders again when the laid-out size differs.
    pub(crate) flexed_grids: HashMap<usize, [Option<(f32, f32)>; 2]>,
    pub(crate) baselines: crate::render::grid::Baselines,
    pub(crate) layout_nodes: Vec<Option<LayoutNode>>,
    pub(crate) measurements: crate::render::grid::Measurements,
    pub(crate) inline_text: std::collections::HashMap<usize, Option<Rc<style::InlineText>>>,
    pub(crate) layout_revision: u64,
    /// Counts style recomputes, so `layout_flags` knows when the mirror's style bits are old.
    style_epoch: u64,
    layout_epoch: u64,
    /// Parsed `<style>` and media query text.
    pub(crate) sheets: HashMap<Rc<str>, Rc<style::Stylesheet>>,
    style_cache: style::StyleCache,
    style_viewport: [f32; 2],
    style_metrics: Option<f32>,
    style_reduced_motion: bool,
    style_theme: u32,
    pub focus: Option<usize>,
    interaction: [Vec<usize>; 3],
    pub selection: HashMap<usize, [usize; 2]>,
    pub selection_caret: Option<usize>,
    pub dirty: bool,
    pub frame_requested: bool,
    /// The page listens to wheel events, so native wheel events go to it first.
    pub(crate) wheel: bool,
    pub canvases: HashMap<usize, crate::render::canvas::Canvas>,
    pub images: HashMap<usize, Arc<renderer::RenderImage>>,
    pub shadows: rustc_hash::FxHashMap<usize, usize>,
    /// Shadow hosts by shadow root.
    pub(crate) hosts: HashMap<usize, usize>,
    /// Replaced or removed images whose GPU atlas tiles the renderer must drop.
    pub retired: Vec<Arc<renderer::RenderImage>>,
    /// Canvases and videos with a replaced image in `retired` or `recycled`.
    pub(crate) retired_canvases: HashSet<usize>,
    /// Replaced video frames by node, whose pixels go back to the media library once the renderer dropped them.
    pub(crate) recycled: Vec<(usize, Arc<renderer::RenderImage>)>,
    /// Images that workers decoded, by node and request.
    decoded: Arc<Mutex<Vec<(usize, u64, Decoded)>>>,
    /// The latest decode request by node.
    decoding: HashMap<usize, u64>,
    /// Empty nodes that the page reserved for `dom.batch` creates.
    pub(crate) reserved: HashSet<usize>,
    /// Inline SVG sources by `<svg>` node, with the size, the newest node revision and the style they were built at.
    pub(crate) svg_sources: HashMap<usize, ([f32; 2], u64, Rc<style::Style>, String)>,
    /// Inline SVG geometry by `<svg>` node: the source it came from, the nodes of elements with their own
    /// ids, the SVG's size, and each element's node with its canvas box and object box.
    pub(crate) svg_boxes: HashMap<usize, SvgBoxes>,
    pub(crate) svg_images: HashMap<usize, (Rc<str>, [f32; 2])>,
    /// Rasterized `clip-path` and `mask-image` shapes, by node.
    pub(crate) masks: HashMap<usize, (String, Arc<renderer::RenderImage>)>,
    pub svgs: HashMap<usize, (String, Arc<renderer::RenderImage>)>,
    pub backgrounds: HashMap<(usize, usize), (String, Arc<renderer::RenderImage>)>,
    pub font_aliases: HashMap<String, String>,
    /// The `@font-face` sources already loaded.
    pub(crate) fonts: std::collections::HashSet<String>,
    /// How many fonts were added, from `@font-face` or `FontFace`.
    pub(crate) font_loads: u32,
    pub text_system: Option<std::rc::Rc<renderer::TextSystem>>,
    pub pixel_ratio: f32,
    pub(crate) linted: bool,
    pub(crate) warned: HashSet<String>,
    pub(crate) motion: style::Motion,
    pub(crate) animated: HashMap<usize, style::Style>,
    pub(crate) animating: HashMap<usize, Rc<style::Style>>,
    /// Nodes with an animated color in the last frame, whose paragraphs draw it from their inline text.
    pub(crate) recolored: Vec<usize>,
    pub(crate) containers: HashMap<usize, [f32; 2]>,
    pub(crate) container_pass: bool,
    pub(crate) anchors: HashMap<usize, renderer::Bounds<renderer::Pixels>>,
    pub(crate) blocks: HashMap<usize, renderer::Bounds<renderer::Pixels>>,
    pub(crate) cull: crate::render::cull::Cull,
}

impl State {
    /// Copies scroll offsets and ranges into the layout mirror.
    pub(crate) fn sync_scroll(&mut self) {
        for (&id, handle) in &self.scroll {
            let (offset, max) = (scroll_offset(handle), handle.max_offset());
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

    pub(crate) fn update_containers(&mut self) -> bool {
        let changed = self
            .style_cache
            .set_containers(std::mem::take(&mut self.containers));
        if changed {
            self.computed = None;
        }
        changed
    }

    pub(crate) fn allocate(&mut self, doc: &mut Document, node: Node) -> usize {
        let id = self.free.pop().unwrap_or_else(|| {
            doc.nodes.push(Node::default());
            doc.nodes.len() - 1
        });
        self.place(doc, id, node);
        id
    }

    pub(crate) fn place(&mut self, doc: &mut Document, id: usize, node: Node) {
        doc.nodes[id] = node;
        self.detached.push(id);
        if let Some(cached) = &mut self.computed {
            if let Some(computed) = Rc::get_mut(cached) {
                computed
                    .values
                    .resize_with(doc.nodes.len(), Default::default);
                computed.values[id] = Rc::default();
                computed.flags.resize(doc.nodes.len(), 0);
                computed.flags[id] = 0;
                computed
                    .lengths
                    .resize(doc.nodes.len(), computed.lengths[0]);
                computed
                    .user_select
                    .resize(doc.nodes.len(), (style::UserSelect::None, 0));
                computed.user_select[id] = (style::UserSelect::None, 0);
            } else if id >= cached.values.len() {
                self.computed = None;
            }
            // WARNING: a frame keeps a clone of the styles, so they can't grow in place. Detached nodes have empty styles in them, and new nodes are detached, so only a new id makes them stale.
        }
    }
}

fn slots(
    doc: &Document,
    roots: impl IntoIterator<Item = (usize, usize)>,
) -> HashMap<usize, Vec<usize>> {
    let mut slots = HashMap::new();
    for (host, root) in roots {
        let mut names = HashMap::new();
        let mut pending = vec![root];
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
            {
                slots.get_mut(slot).unwrap().push(child);
            }
        }
    }
    slots
}

fn pixmap(image: &renderer::RenderImage) -> Option<resvg::tiny_skia::Pixmap> {
    let size = image.size();
    let mut pixels = resvg::tiny_skia::Pixmap::new(size.width.into(), size.height.into())?;
    for (out, pixel) in pixels
        .pixels_mut()
        .iter_mut()
        .zip(image.bytes().as_chunks::<4>().0)
    {
        *out = resvg::tiny_skia::ColorU8::from_rgba(pixel[2], pixel[1], pixel[0], pixel[3])
            .premultiply();
    }
    Some(pixels)
}

/// Shows a new frame of video `id`, or none. Returns the replaced frame when no window drew it.
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
                    let size = resvg::usvg::Tree::from_data(&bytes, &Default::default())
                        .map_err(|_| error)?
                        .size();
                    let text = if bytes.starts_with(&[0x1f, 0x8b]) {
                        resvg::usvg::decompress_svgz(&bytes)?
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
            | "dom.mediaMatches"
            | "dom.assignedNodes"
            | "dom.queryWithin"
            | "dom.html"
            | "dom.style"
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
            | "dom.image"
            | "dom.decoded"
            | "dom.measureText"
            | "dom.textRect"
            | "dom.textIndex"
            | "dom.svgBox"
            | "dom.textOffset"
            | "dom.textPoint"
            | "dom.font"
            | "dom.fontCheck"
            | "dom.hitTest"
            | "dom.css"
            | "dom.sheet"
            | "dom.constructed"
            | "dom.adopt"
            | "dom.supports"
            | "dom.registerProperty"
            | "dom.computed"
            | "dom.declare"
            | "dom.properties"
            | "dom.client"
            | "dom.innerText"
            | "dom.wheel"
    ) {
        return None;
    }
    Some((|| {
        if op == "dom.scrollEvents" {
            let mut changed = Vec::new();
            for (&id, handle) in &state.scroll {
                let offset = scroll_offset(handle);
                handle.set_offset(offset);
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
                            state.svg_images.remove(&id);
                            state
                                .retired
                                .extend(state.images.insert(id, Arc::new(image)));
                            json!([u32::from(size.width), u32::from(size.height)])
                        }
                        Ok(Err((text, size))) => {
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
        if op == "dom.wheel" {
            state.wheel = true;
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
        if op == "dom.declare" {
            return Ok(json!(style::declare(args[0].as_str().unwrap_or_default())));
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
            state.computed = None;
            state.dirty = true;
            return Ok(result.unwrap_or_else(|error| json!({ "error": error })));
        }
        if op == "dom.release" {
            // WARNING: the page already forgot these lists, so a bad id is skipped instead of failing the rest.
            for value in args[1].as_array().context("Expected live nodes")? {
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
            let mut roots = std::mem::take(&mut state.detached);
            roots.sort_unstable();
            roots.dedup();
            let mut freed = HashSet::new();
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
                    if matches!(doc.nodes[id].tag.as_str(), "canvas" | "video" | "audio") {
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
                    state.inline_text.remove(&id);
                    state.retired.extend(state.images.remove(&id));
                    state
                        .retired
                        .extend(state.canvases.remove(&id).map(|canvas| canvas.image));
                    state
                        .retired
                        .extend(state.svgs.remove(&id).map(|(_, image)| image));
                    state
                        .retired
                        .extend(state.masks.remove(&id).map(|(_, image)| image));
                    state.svg_sources.remove(&id);
                    state.svg_boxes.remove(&id);
                    state.svg_images.remove(&id);
                    state.decoding.remove(&id);
                    if let Some(root) = state.shadows.remove(&id) {
                        state.hosts.remove(&root);
                    }
                    state.hosts.remove(&id);
                    state.motion.forget(id);
                    state.anchors.remove(&id);
                    if state.focus == Some(id) {
                        state.focus = None;
                    }
                    state.selection.remove(&id);
                    if state.selection_caret == Some(id) {
                        state.selection_caret = None;
                    }
                    state.free.push(id);
                    freed.insert(id);
                }
            }
            if !freed.is_empty() {
                state
                    .measurements
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
            return Ok(json!(canvases));
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
            let first = args[2].as_bool().unwrap_or(false);
            let caret = args[3].as_bool().unwrap_or(false);
            let mut nearest = Vec::new();
            let mut target = None;
            let mut pending = vec![(0, [0., 0., viewport[0], viewport[1]], false)];
            let mut hits = Vec::new();
            while let Some((id, mut clip, hit)) = pending.pop() {
                if hit {
                    if caret {
                        if doc.nodes[id].tag == "#text" {
                            return Ok(json!(id));
                        }
                        target.get_or_insert(id);
                    } else if first {
                        return Ok(json!(id));
                    } else {
                        hits.push(id);
                    }
                    continue;
                }
                let node = &doc.nodes[id];
                let style = &computed.values[id];
                if style
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
                            .is_some_and(|value| {
                                matches!(value, "hidden" | "clip" | "auto" | "scroll")
                            })
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
                    pending.extend(children.iter().map(|&child| (child, clip, false)));
                }
            }
            if caret {
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
                return Ok(json!(text.or(target)));
            }
            if first && x >= 0. && y >= 0. && x < viewport[0] && y < viewport[1] {
                return Ok(json!(
                    doc.nodes[0]
                        .children
                        .iter()
                        .find(|&&id| !doc.nodes[id].tag.starts_with('#'))
                ));
            }
            return Ok(if first { Value::Null } else { json!(hits) });
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
            let used = state.style_cache.states & (flag | focus) != 0;
            // Form controls draw their own hover and pressed looks.
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
                if let Some(node) = state.focus.and_then(|id| doc.nodes.get_mut(id)) {
                    node.state &= !(FOCUS | FOCUS_VISIBLE);
                }
                state.focus = ids.first().copied();
                if let Some(id) = state.focus {
                    doc.nodes[id].state |= if args[1].as_bool() == Some(false) {
                        FOCUS
                    } else {
                        FOCUS | FOCUS_VISIBLE
                    };
                }
            }
            for &id in &state.interaction[index] {
                if let Some(node) = doc.nodes.get_mut(id) {
                    node.state &= !flag;
                }
            }
            for &id in &ids {
                doc.nodes[id].state |= flag;
            }
            state.interaction[index] = ids;
            if used {
                state.computed = None;
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
            "dom.source" => {
                Ok(json!(doc.nodes[id].stylesheet.as_ref().map_or_else(
                    || doc.nodes[id].text.clone(),
                    |sheet| sheet.source()
                )))
            }
            "dom.matches" => Ok(json!(
                Selector::try_parse(args[1].as_str().context("Expected selector")?)?
                    .iter()
                    .any(|s| s.matches_cached(doc, id, Some(id), &mut SelectorCache::default()))
            )),
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
                let mut svg = tag == "svg";
                let mut up = doc.nodes[id].parent;
                while let Some(node) = up.filter(|_| !svg) {
                    if doc.nodes[node].tag.eq_ignore_ascii_case("foreignobject") {
                        break;
                    }
                    svg = doc.nodes[node].tag == "svg";
                    up = doc.nodes[node].parent;
                }
                let parsed = match args[2].as_str() {
                    _ if tag == "#document" => Document::parse(source),
                    Some(context) => Document::fragment(source, context),
                    None if svg && !matches!(tag, "foreignObject" | "desc" | "title") => {
                        Document::fragment(source, "svg")
                    }
                    None if svg => Document::fragment(source, "body"),
                    // WARNING: template contents and shadow roots are the only fragments with markup, and template rules keep `<tr>` rows that Lit renders.
                    None if tag.starts_with('#') => Document::fragment(source, "template"),
                    None => Document::fragment(source, tag),
                };
                for child in std::mem::take(&mut doc.nodes[id].children) {
                    doc.nodes[child].parent = None;
                    state.detached.push(child);
                }
                doc.nodes[id].text.clear();
                let mut ids = vec![id];
                for _ in 1..parsed.nodes.len() {
                    ids.push(state.allocate(doc, Node::default()));
                }
                doc.nodes[id].children = parsed.nodes[0]
                    .children
                    .iter()
                    .map(|&child| ids[child])
                    .collect();
                for (index, mut node) in parsed.nodes.into_iter().enumerate().skip(1) {
                    node.parent = node.parent.map(|parent| ids[parent]);
                    for child in &mut node.children {
                        *child = ids[*child];
                    }
                    doc.nodes[ids[index]] = node;
                }
                state.dirty = true;
                Ok(tree(doc, id, false))
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
                let style = match pseudo {
                    "" => Some(
                        &**state
                            .animating
                            .get(&id)
                            .or_else(|| styles.authored.get(&id))
                            .unwrap_or(&styles.values[id]),
                    ),
                    "before" | "after" => styles
                        .pseudos
                        .get(&id)
                        .map(|values| &values[usize::from(pseudo == "after")]),
                    _ => None,
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
                let container = doc.nodes[id]
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
                    .unwrap_or(viewport);
                let registry = state.style_cache.registry();
                let parent = doc.nodes[id].parent.map(|parent| &*styles.values[parent]);
                Ok(match args[1].as_str() {
                    Some(name) => {
                        let value =
                            style::resolved(style, &units, name, size, container, &registry);
                        let tag = doc.nodes[id].tag.as_str();
                        // Tables lay out as grids, but report their table display like browsers.
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
                            // `auto` minimum sizes are 0 outside flex and grid containers.
                            ("min-width" | "min-height", "auto")
                                if parent.is_none_or(|parent| {
                                    parent.get(Prop::DISPLAY).is_none_or(|display| {
                                        !display.ends_with("flex") && !display.ends_with("grid")
                                    })
                                }) =>
                            {
                                "0px".to_owned()
                            }
                            // A relatively positioned box with `auto` offsets doesn't move.
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
                state.computed = None;
                state.dirty = true;
                Ok(result.unwrap_or_else(|error| json!({ "error": error })))
            }
            "dom.style" => {
                let system = state.text_system.clone();
                let styles = styles(
                    doc,
                    state,
                    viewport,
                    system.as_deref().map(|system| (system, state.pixel_ratio)),
                );
                let style = &styles.values[id];
                Ok(match args[1].as_array() {
                    Some(names) => json!(
                        names
                            .iter()
                            .filter_map(|name| {
                                let name = name.as_str()?;
                                Some((name, style.get(Prop::find(name)?)?))
                            })
                            .collect::<HashMap<_, _>>()
                    ),
                    None => json!(&**style),
                })
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
                    return Ok(json!(computed.user_select[id]));
                }
                let mut text = String::new();
                if let Some(ranges) = args[1].as_array() {
                    for range in ranges {
                        let node = range[0].as_u64().context("Expected selection node")? as usize;
                        ensure!(node < doc.nodes.len(), "Invalid selection node");
                        if computed.user_select[node].0 == style::UserSelect::None {
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
                let handle = state.scroll.entry(if page { 0 } else { id }).or_default();
                let previous = handle.offset();
                handle.set_offset(renderer::point(
                    renderer::px(
                        -args[1]
                            .as_f64()
                            .map(|v| v as f32)
                            .unwrap_or(-f32::from(previous.x)),
                    ),
                    renderer::px(
                        -args[2]
                            .as_f64()
                            .map(|v| v as f32)
                            .unwrap_or(-f32::from(previous.y)),
                    ),
                ));
                handle.set_offset(scroll_offset(handle));
                state.dirty |= handle.offset() != previous;
                state.sync_scroll();
                Ok(Value::Null)
            }
            "dom.canvas" => {
                if let Some(ops) = crate::jsc::payload::slice(&args[1]) {
                    let ops: Vec<f32> = ops?
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|bytes| f32::from_ne_bytes(*bytes))
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
                        None => crate::render::canvas::Canvas::new(
                            doc.nodes[id]
                                .attrs
                                .get("width")
                                .and_then(|v| crate::render::dimension(v, true))
                                .map_or(300, |v| v as u32),
                            doc.nodes[id]
                                .attrs
                                .get("height")
                                .and_then(|v| crate::render::dimension(v, true))
                                .map_or(150, |v| v as u32),
                        )?,
                    };
                    let old = canvas.image.clone();
                    let result = canvas.draw(
                        &ops,
                        args[2].as_str().unwrap_or(""),
                        &data,
                        &|source| match state.canvases.get(&source) {
                            Some(canvas) => Some(canvas.pixels.clone()),
                            None => pixmap(state.images.get(&source)?),
                        },
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
                    state
                        .retired
                        .extend(state.canvases.remove(&id).map(|canvas| canvas.image));
                }
                state.dirty = true;
                Ok(Value::Null)
            }
            "dom.canvasURL" => {
                use base64::Engine as _;
                let bytes = if let Some(canvas) = state.canvases.get(&id) {
                    canvas.pixels.encode_png()?
                } else if let Some(image) = state.images.get(&id) {
                    pixmap(image)
                        .context("Invalid image pixels")?
                        .encode_png()?
                } else {
                    let width = doc.nodes[id]
                        .attrs
                        .get("width")
                        .and_then(|v| crate::render::dimension(v, true))
                        .map_or(300, |v| v as u32);
                    let height = doc.nodes[id]
                        .attrs
                        .get("height")
                        .and_then(|v| crate::render::dimension(v, true))
                        .map_or(150, |v| v as u32);
                    crate::render::canvas::Canvas::new(width, height)?
                        .pixels
                        .encode_png()?
                };
                Ok(json!(format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                )))
            }
            "dom.canvasPixels" => {
                if let std::collections::hash_map::Entry::Vacant(entry) = state.canvases.entry(id) {
                    let width = doc.nodes[id]
                        .attrs
                        .get("width")
                        .and_then(|v| crate::render::dimension(v, true))
                        .map_or(300, |v| v as u32);
                    let height = doc.nodes[id]
                        .attrs
                        .get("height")
                        .and_then(|v| crate::render::dimension(v, true))
                        .map_or(150, |v| v as u32);
                    entry.insert(crate::render::canvas::Canvas::new(width, height)?);
                }
                let canvas = state.canvases.get(&id).unwrap();
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
                for row in 0..height {
                    for col in 0..width {
                        let sx = x + col as i64;
                        let sy = y + row as i64;
                        if sx < 0
                            || sy < 0
                            || sx >= i64::from(canvas.pixels.width())
                            || sy >= i64::from(canvas.pixels.height())
                        {
                            continue;
                        }
                        let color = canvas.pixels.pixels()
                            [sy as usize * canvas.pixels.width() as usize + sx as usize]
                            .demultiply();
                        let at = (row * width + col) * 4;
                        bytes[at..at + 4].copy_from_slice(&[
                            color.red(),
                            color.green(),
                            color.blue(),
                            color.alpha(),
                        ]);
                    }
                }
                Ok(crate::web::base64(&bytes))
            }
            "dom.image" => {
                let bytes = crate::web::bytes(&args[1])?;
                Ok(load_image(state, id, move || Ok(bytes)))
            }
            "dom.font" => {
                let family = args[1].as_str().context("Expected font family")?;
                add_font(state, family, crate::web::bytes(&args[2])?)?;
                Ok(Value::Null)
            }
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
                    json!([id, doc.nodes[id].children.len()])
                })
            }
            "dom.svgBox" => {
                // An SVG element's box in its own coordinates; the root's covers its content.
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
                if let Some((layout, text, range, rows)) = state.text_layouts.get(&id) {
                    let content = &text[range.clone()];
                    if op == "dom.textIndex" {
                        let position = renderer::point(
                            renderer::px(args[1].as_f64().unwrap_or(0.) as f32),
                            renderer::px(args[2].as_f64().unwrap_or(0.) as f32),
                        );
                        let index = crate::render::text::text_index(layout, text, rows, position)
                            .unwrap_or_else(|index| index)
                            .clamp(range.start, range.end);
                        return Ok(json!(real(text[range.start..index].encode_utf16().count())));
                    }
                    let a = crate::render::text::text_position(
                        layout,
                        text,
                        rows,
                        range.start + utf16_index(content, start),
                    );
                    let b = crate::render::text::text_position(
                        layout,
                        text,
                        rows,
                        range.start + utf16_index(content, end),
                    );
                    return Ok(json!(
                        a.zip(b)
                            .map(|(a, b)| vec![[
                                f32::from(a.x),
                                f32::from(a.y),
                                f32::from(b.x - a.x),
                                f32::from(layout.line_height())
                            ]])
                            .unwrap_or_default()
                    ));
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
                let line = crate::render::canvas::line(
                    system,
                    &state.font_aliases,
                    args[2].as_str().unwrap_or("10px sans-serif"),
                    args[1].as_str().context("Expected text")?,
                );
                Ok(
                    json!({"width":f32::from(line.width),"actualBoundingBoxLeft":0,"actualBoundingBoxRight":f32::from(line.width),"actualBoundingBoxAscent":f32::from(line.ascent),"actualBoundingBoxDescent":f32::from(line.descent),"fontBoundingBoxAscent":f32::from(line.ascent),"fontBoundingBoxDescent":f32::from(line.descent)}),
                )
            }
            "dom.client" => {
                let rect = state.bounds.get(&id).copied().unwrap_or_default();
                let system = state.text_system.clone();
                let computed = styles(
                    doc,
                    state,
                    viewport,
                    system.as_deref().map(|system| (system, state.pixel_ratio)),
                );
                let (style, units) = (&computed.values[id], &computed.lengths[id]);
                let tag = doc.nodes[id].tag.as_str();
                // Table rows and row groups have the box of their cells, without borders or padding.
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
                // A cell of a collapsed table owns half of each border it shares.
                let shared = matches!(tag, "td" | "th")
                    && std::iter::successors(doc.nodes[id].parent, |&parent| {
                        doc.nodes[parent].parent
                    })
                    .find(|&parent| doc.nodes[parent].tag == "table")
                    .is_some_and(|table| {
                        computed.values[table].get(Prop::BORDER_COLLAPSE) == Some("collapse")
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
                Ok(json!([
                    borders[3],
                    borders[0],
                    (rect[2] - borders[1] - borders[3] - bars[1]).max(0.),
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

/// The nodes of a subtree in document order, each as `id, parent, tag, text, attribute count, names and values`.
fn tree(doc: &Document, root: usize, inclusive: bool) -> Value {
    let mut list = Vec::new();
    let mut pending: Vec<_> = if inclusive {
        vec![root]
    } else {
        doc.nodes[root].children.iter().rev().copied().collect()
    };
    while let Some(id) = pending.pop() {
        let node = &doc.nodes[id];
        let inline = node.inline.as_ref().map(|inline| inline.source());
        // WARNING: script and style sources can be large, so pages read them with `dom.source` when needed.
        let lazy = node.stylesheet.is_some()
            || !node.text.is_empty()
                && node.parent.is_some_and(|parent| {
                    matches!(doc.nodes[parent].tag.as_str(), "script" | "style")
                });
        list.extend([
            json!(id),
            json!(node.parent),
            json!(node.tag),
            if lazy { Value::Null } else { json!(node.text) },
            json!(node.attrs.iter().count() + usize::from(inline.is_some())),
        ]);
        for (key, value) in &node.attrs {
            list.extend([json!(key), json!(value)]);
        }
        if let Some(inline) = inline {
            list.extend([json!("style"), json!(inline)]);
        }
        pending.extend(node.children.iter().rev());
    }
    Value::Array(list)
}

fn scroll_offset(handle: &renderer::ScrollHandle) -> renderer::Point<renderer::Pixels> {
    let mut offset = handle.offset();
    let bounds = handle.bounds();
    if bounds.size.width > renderer::px(0.) || bounds.size.height > renderer::px(0.) {
        let max = handle.max_offset();
        offset.x = offset.x.clamp(-max.x, renderer::px(0.));
        offset.y = offset.y.clamp(-max.y, renderer::px(0.));
    }
    offset
}

/// Adds font data for the CSS `family`, from `FontFace` or `@font-face`.
pub(crate) fn add_font(state: &mut State, family: &str, bytes: Vec<u8>) -> Result<()> {
    let system = state
        .text_system
        .as_ref()
        .context("Font loading requires a displayed window")?;
    ensure!(bytes.len() <= 32 * 1024 * 1024, "Font exceeds 32 MB limit");
    let mut fonts = crate::render::canvas::fonts(false);
    let database = std::sync::Arc::make_mut(&mut fonts.0);
    let ids = database.load_font_source(resvg::usvg::fontdb::Source::Binary(std::sync::Arc::new(
        bytes.clone(),
    )));
    let face = ids
        .first()
        .and_then(|id| database.face(*id))
        .context("Unsupported or invalid font data")?;
    // A face of the same family, like one weight of several, keeps the family itself.
    let name = face
        .families
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(family))
        .or(face.families.first())
        .context("Font has no family name")?
        .0
        .clone();
    system.add_fonts(vec![std::borrow::Cow::Owned(bytes)])?;
    state.font_loads += 1;
    style::FONT_NAMES.with(|names| *names.borrow_mut() = Default::default());
    state.style_cache = style::StyleCache::default();
    state.computed = None;
    if !name.eq_ignore_ascii_case(family) {
        state.font_aliases.insert(family.to_owned(), name);
    }
    state.dirty = true;
    Ok(())
}

/// Writes the overflow clip and positioned flags of current styles into the layout mirror.
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
            // WARNING: like CSSOM `offsetParent`, a transformed or filtered box contains its positioned descendants too.
            let positioned = position != "static"
                || [Prop::TRANSFORM, Prop::FILTER]
                    .into_iter()
                    .any(|prop| style.get(prop).is_some_and(|value| value != "none"));
            (if clip { MIRROR_CLIP } else { 0 })
                | if positioned { MIRROR_POSITIONED } else { 0 }
                | if position == "fixed" { MIRROR_FIXED } else { 0 }
        });
        state.bounds.reserve(id);
        unsafe {
            let flags = state.bounds.flags(id);
            *flags = (*flags & (MIRROR_BOUNDS | MIRROR_SCROLLER | MIRROR_HIDDEN)) | bits;
        }
    }
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
    let slots = slots(doc, state.shadows.iter().map(|(&host, &root)| (host, root)));
    let mut context = style::StyleContext {
        reduced_motion,
        theme,
        scale: scale.unwrap_or(1.),
        aliases: state.font_aliases.clone(),
        animated,
        ..Default::default()
    };
    if !state.shadows.is_empty() {
        let mut required = HashSet::from([0]);
        for &node in slots
            .keys()
            .chain(state.shadows.keys())
            .chain(state.shadows.values())
        {
            let mut current = Some(node);
            while let Some(node) = current {
                if !required.insert(node) {
                    break;
                }
                current = doc.nodes[node].parent;
            }
        }
        context.required = Some(required);
    }
    let mut result = style::compute_styles(
        doc,
        viewport[0],
        viewport[1],
        system,
        &mut state.sheets,
        &mut state.style_cache,
        Some(&context),
    );
    // WARNING: the first pass computes only hosts, roots, slots and their ancestors to collect `:host` declarations and slot styles; pass 1 recomputes every node with them.
    for pass in 0..if state.shadows.is_empty() { 0 } else { 2 } {
        if pass == 1 {
            context.required = None;
            result.pseudos.clear();
            result.text_pseudos.clear();
            result.scrollbars.clear();
            result.selections.clear();
            result.animations.clear();
            result.slotted.clear();
            result.parts.clear();
            result.starting.clear();
            style::cascade(
                doc,
                viewport,
                system,
                &mut state.sheets,
                &mut state.style_cache,
                Some(&context),
                &mut result,
            );
        }
        let mut pending = vec![0];
        context.parts.clear();
        while let Some(id) = pending.pop() {
            if let Some(&root) = state.shadows.get(&id) {
                if let Some(rules) = result.parts.get(&id).cloned() {
                    let mut batch: HashMap<usize, Vec<_>> = HashMap::new();
                    type Exported = Option<Rc<HashMap<String, String>>>;
                    let mut stack: Vec<(usize, Exported)> = vec![(root, None)];
                    while let Some((node, mapping)) = stack.pop() {
                        let names: Vec<String> = doc.nodes[node]
                            .attrs
                            .get("part")
                            .into_iter()
                            .flat_map(|names| names.split_ascii_whitespace())
                            .filter_map(|name| match &mapping {
                                Some(mapping) => mapping.get(name).cloned(),
                                None => Some(name.to_owned()),
                            })
                            .collect();
                        for (name, declarations) in rules.iter() {
                            if names.iter().any(|part| **part == **name) {
                                batch
                                    .entry(node)
                                    .or_default()
                                    .extend(declarations.iter().cloned());
                            }
                        }
                        stack.extend(
                            doc.nodes[node]
                                .children
                                .iter()
                                .map(|&child| (child, mapping.clone())),
                        );
                        if let (Some(&inner), Some(exported)) = (
                            state.shadows.get(&node),
                            doc.nodes[node].attrs.get("exportparts"),
                        ) {
                            let exported: HashMap<String, String> = exported
                                .split(',')
                                .filter_map(|item| {
                                    let (inner, outer) =
                                        item.split_once(':').unwrap_or((item, item));
                                    let outer = outer.trim();
                                    let outer = match &mapping {
                                        Some(mapping) => mapping.get(outer)?.clone(),
                                        None => outer.to_owned(),
                                    };
                                    Some((inner.trim().to_owned(), outer))
                                })
                                .collect();
                            stack.push((inner, Some(Rc::new(exported))));
                        }
                    }
                    for (node, declarations) in batch {
                        context
                            .parts
                            .entry(node)
                            .or_default()
                            .splice(0..0, declarations);
                    }
                }
                let mut scoped_root = doc.nodes[root].clone();
                scoped_root.parent = Some(0);
                scoped_root.tag = "apps-shadow-root".into();
                scoped_root.attrs = doc.nodes[id].attrs.clone();
                scoped_root.inline = None;
                scoped_root.attrs.remove("style");
                scoped_root.state = doc.nodes[id].state;
                let host = (result.values[id].clone(), result.lengths[id]);
                if pass == 0 {
                    context.inherited.insert(root, host);
                } else {
                    context.roots.insert(root, host);
                }
                let scoped_doc = Node {
                    tag: "#document".into(),
                    children: vec![root],
                    ..Default::default()
                };
                let document = std::mem::replace(&mut doc.nodes[0], scoped_doc);
                let shadow = std::mem::replace(&mut doc.nodes[root], scoped_root);
                let top = (result.values[0].clone(), result.lengths[0], result.flags[0]);
                style::cascade(
                    doc,
                    viewport,
                    system,
                    &mut state.sheets,
                    &mut state.style_cache,
                    Some(&context),
                    &mut result,
                );
                (result.values[0], result.lengths[0], result.flags[0]) = top;
                doc.nodes[0] = document;
                doc.nodes[root] = shadow;
                if pass == 0 {
                    context
                        .hosts
                        .insert(id, std::mem::take(&mut result.host_declarations));
                }
                pending.push(root);
            }
            pending.extend(&doc.nodes[id].children);
        }
        if pass == 0 {
            for (&slot, assigned) in &slots {
                for &child in assigned {
                    context
                        .inherited
                        .insert(child, (result.values[slot].clone(), result.lengths[slot]));
                }
                let Some(rules) = result.slotted.get(&slot).cloned() else {
                    continue;
                };
                for (compound, declarations) in rules.iter() {
                    let selectors = Selector::parse(compound, None);
                    for &child in assigned {
                        if selectors
                            .iter()
                            .any(|selector| selector.matches(doc, child))
                        {
                            context
                                .slotted
                                .entry(child)
                                .or_default()
                                .extend(declarations.iter().cloned());
                        }
                    }
                }
            }
            for declarations in context.slotted.values_mut() {
                declarations.sort_by_key(|(_, _, important)| *important);
            }
        }
    }
    state.style_cache.finish();
    state.sheets.retain(|text, _| Rc::strong_count(text) > 1);
    for (&host, &root) in &state.shadows {
        result
            .children
            .insert(host, doc.nodes[root].children.clone());
    }
    for (slot, assigned) in slots {
        if !assigned.is_empty() {
            result.children.insert(slot, assigned);
        }
    }
    let mut order = Vec::new();
    let mut pending = vec![(0, (style::UserSelect::Text, 0), false)];
    while let Some((id, inherited, hidden)) = pending.pop() {
        let node = &doc.nodes[id];
        let flags = result.flags[id];
        let hidden = hidden || flags & style::DISPLAY_NONE != 0;
        let mode = if hidden || flags & style::INVISIBLE != 0 {
            style::UserSelect::None
        } else if matches!(node.tag.as_str(), "input" | "textarea")
            || node.attrs.get("contenteditable").is_some_and(|v| {
                matches!(
                    v.to_ascii_lowercase().as_str(),
                    "" | "true" | "plaintext-only"
                )
            })
        {
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
        let selection = (mode, if mode == inherited.0 { inherited.1 } else { id });
        result.user_select[id] = selection;
        order.push(id);
        pending.extend(
            result
                .children
                .get(&id)
                .unwrap_or(&node.children)
                .iter()
                .map(|&child| (child, selection, hidden)),
        );
    }
    for id in order.into_iter().rev() {
        let children = result.children.get(&id).unwrap_or(&doc.nodes[id].children);
        let contents = |child: usize| result.flags[child] & style::DISPLAY_CONTENTS != 0;
        if children.iter().any(|&child| contents(child)) {
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
    if animating {
        return Rc::new(result);
    }
    state.animating.clear();
    state.style_epoch += 1;
    state.style_viewport = viewport;
    state.style_metrics = scale;
    state.style_reduced_motion = reduced_motion;
    state.style_theme = theme;
    let result = Rc::new(result);
    state.linted = false;
    state.computed = Some(result.clone());
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

/// The bullets that a password field shows before UTF-16 offset `offset` of its `value`.
pub(crate) fn bullets(value: &str, offset: usize) -> usize {
    value[..utf16_index(value, offset)].chars().count()
}

/// The UTF-16 offset in a password `value` after `bullets` of its bullets.
pub(crate) fn unbullet(value: &str, bullets: usize) -> usize {
    value.chars().take(bullets).map(char::len_utf16).sum()
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
    fn detached_nodes_keep_styles_that_a_frame_holds() {
        use crate::{
            host::{Host, Page, Role},
            jsc::{Api, Engine},
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
        let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
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
            jsc::{Api, Engine},
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
        let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
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
            jsc::{Api, Engine},
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
        let engine = Engine::new(Api::load().unwrap(), host.clone(), Role::Renderer(1)).unwrap();
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
}
