mod document;
mod selector;

pub use document::{Attrs, Document, Node, decode, escape};
pub(crate) use selector::{
    ACTIVE, FOCUS, FOCUS_WITHIN, HOVER, Selector, SelectorCache, SelectorList,
};

use crate::style;
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
    pub shift: render::Pixels,
    pub right: render::Pixels,
    pub gap: render::Pixels,
    pub start: usize,
    /// How far the row moves down, when the first row is taller.
    pub drop: render::Pixels,
}

pub(crate) type Rows = Rc<[Row]>;

/// A decoded raster, or SVG text and its size.
type Decoded = Result<Result<render::RenderImage, (String, [f32; 2])>>;

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
/// floats (stale, capacity), 8 floats per node (x, y, width, height, scroll left, scroll top,
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

pub(crate) const MIRROR_BOUNDS: u8 = 1;
pub(crate) const MIRROR_SCROLLER: u8 = 2;
pub(crate) const MIRROR_CLIP: u8 = 4;
pub(crate) const MIRROR_POSITIONED: u8 = 8;

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
        HashMap<usize, (render::TextLayout, Rc<str>, std::ops::Range<usize>, Rows)>,
    pub scroll: HashMap<usize, render::ScrollHandle>,
    scroll_offsets: HashMap<usize, render::Point<render::Pixels>>,
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
    style_metrics: bool,
    style_reduced_motion: bool,
    style_theme: u32,
    pub focus: Option<usize>,
    interaction: [Vec<usize>; 3],
    pub selection: HashMap<usize, [usize; 2]>,
    pub selection_caret: Option<usize>,
    pub dirty: bool,
    pub frame_requested: bool,
    pub canvases: HashMap<usize, crate::render::canvas::Canvas>,
    pub images: HashMap<usize, Arc<render::RenderImage>>,
    pub shadows: rustc_hash::FxHashMap<usize, usize>,
    /// Shadow hosts by shadow root.
    pub(crate) hosts: HashMap<usize, usize>,
    /// Replaced or removed images whose GPU atlas tiles the renderer must drop.
    pub retired: Vec<Arc<render::RenderImage>>,
    /// Canvases with a redrawn image in `retired`.
    retired_canvases: HashSet<usize>,
    /// Images that workers decoded, by node and request.
    decoded: Arc<Mutex<Vec<(usize, u64, Decoded)>>>,
    /// The latest decode request by node.
    decoding: HashMap<usize, u64>,
    /// Empty nodes that the page reserved for `dom.batch` creates.
    pub(crate) reserved: HashSet<usize>,
    pub(crate) svg_sources: HashMap<usize, ([f32; 2], String, String)>,
    pub(crate) svg_images: HashMap<usize, (Rc<str>, [f32; 2])>,
    /// Rasterized `clip-path` and `mask-image` shapes, by node.
    pub(crate) masks: HashMap<usize, (String, Arc<render::RenderImage>)>,
    pub(crate) image_crops: HashMap<usize, (u64, Arc<render::RenderImage>)>,
    pub svgs: HashMap<usize, (String, Arc<render::RenderImage>)>,
    pub backgrounds: HashMap<(usize, usize), (String, Arc<render::RenderImage>)>,
    pub font_aliases: HashMap<String, String>,
    /// The `@font-face` sources already loaded.
    pub(crate) fonts: std::collections::HashSet<String>,
    /// How many fonts were added, from `@font-face` or `FontFace`.
    pub(crate) font_loads: u32,
    pub text_system: Option<std::rc::Rc<render::TextSystem>>,
    pub pixel_ratio: f32,
    pub(crate) linted: bool,
    pub(crate) warned: HashSet<String>,
    pub(crate) motion: style::Motion,
    pub(crate) containers: HashMap<usize, [f32; 2]>,
    pub(crate) container_pass: bool,
    pub(crate) anchors: HashMap<usize, render::Bounds<render::Pixels>>,
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
            } else {
                self.computed = None;
            }
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
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(16384);
            limits.max_image_height = Some(16384);
            limits.max_alloc = Some(256 * 1024 * 1024);
            Ok(match crate::render::image::decode(&bytes, limits) {
                Ok(image) => {
                    let mut pixels = image.into_rgba8();
                    for pixel in pixels.pixels_mut() {
                        pixel.0.swap(0, 2);
                    }
                    Ok(render::RenderImage::new(pixels.width(), pixels.height(), pixels.into_raw()))
                }
                Err(error) => {
                    let size = resvg::usvg::Tree::from_data(&bytes, &Default::default())
                        .map_err(|_| error)?
                        .size();
                    let text = if bytes.starts_with(&[0x1f, 0x8b]) {
                        resvg::usvg::decompress_svgz(&bytes)?
                    } else {
                        bytes
                    };
                    Err((
                        String::from_utf8_lossy(&text).into_owned(),
                        [size.width(), size.height()],
                    ))
                }
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
            | "dom.textOffset"
            | "dom.textPoint"
            | "dom.font"
            | "dom.fontCheck"
            | "dom.hitTest"
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
            return Ok(sheet.matches_media(viewport[0], viewport[1]).into());
        }
        if op == "dom.release" {
            for value in args[1].as_array().context("Expected live nodes")? {
                let id = value.as_u64().context("Invalid node id")? as usize;
                ensure!(
                    doc.nodes.get(id).is_some_and(|node| !node.tag.is_empty()),
                    "Invalid node id"
                );
                state.live.insert(id);
            }
            for value in args[0].as_array().context("Expected dead nodes")? {
                state
                    .live
                    .remove(&(value.as_u64().context("Invalid node id")? as usize));
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
                    if doc.nodes[id].tag == "canvas" {
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
                    state
                        .retired
                        .extend(state.image_crops.remove(&id).map(|(_, image)| image));
                    state.svg_sources.remove(&id);
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
                system.as_deref(),
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
                if style.get("display").is_some_and(|value| value == "none") {
                    continue;
                }
                if let Some(&[left, top, width, height]) = state.bounds.get(&id) {
                    let selectable = !style
                        .get("pointer-events")
                        .is_some_and(|value| value == "none")
                        && !style
                            .get("visibility")
                            .is_some_and(|value| matches!(value.as_str(), "hidden" | "collapse"));
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
                    for (axis, name) in [(0, "overflow-x"), (1, "overflow-y")] {
                        if style
                            .get(name)
                            .or_else(|| style.get("overflow"))
                            .is_some_and(|value| {
                                matches!(value.as_str(), "hidden" | "clip" | "auto" | "scroll")
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
            if op == "dom.focus" {
                if let Some(node) = state.focus.and_then(|id| doc.nodes.get_mut(id)) {
                    node.state &= !FOCUS;
                }
                state.focus = ids.first().copied();
                if let Some(id) = state.focus {
                    doc.nodes[id].state |= FOCUS;
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
            state.dirty |= used || focus != 0;
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
                Selector::parse(args[1].as_str().context("Expected selector")?, None)
                    .iter()
                    .any(|s| s.matches_cached(doc, id, Some(id), &mut SelectorCache::default()))
            )),
            "dom.queryWithin" => {
                let selectors =
                    Selector::parse(args[1].as_str().context("Expected selector")?, None);
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
                let parsed = Document::parse(args[1].as_str().context("Expected HTML")?);
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
            "dom.style" => {
                let system = state.text_system.clone();
                let styles = styles(
                    doc,
                    state,
                    viewport,
                    system.as_deref(),
                );
                let style = &styles.values[id];
                Ok(match args[1].as_array() {
                    Some(names) => json!(
                        names
                            .iter()
                            .filter_map(|name| name
                                .as_str()
                                .and_then(|name| style.get_key_value(name)))
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
                    system.as_deref(),
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
                handle.set_offset(render::point(
                    render::px(
                        -args[1]
                            .as_f64()
                            .map(|v| v as f32)
                            .unwrap_or(-f32::from(previous.x)),
                    ),
                    render::px(
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
                if args[1].is_null() {
                    state
                        .retired
                        .extend(state.canvases.remove(&id).map(|canvas| canvas.image));
                } else {
                    let width = doc.nodes[id]
                        .attrs
                        .get("width")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(300);
                    let height = doc.nodes[id]
                        .attrs
                        .get("height")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(150);
                    let canvas = match state.canvases.entry(id) {
                        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(crate::render::canvas::Canvas::new(width, height)?)
                        }
                    };
                    let old = canvas.image.clone();
                    canvas.draw(args[1].as_array().context("Expected canvas commands")?)?;
                    if state.retired.is_empty() {
                        state.retired_canvases.clear();
                    }
                    // WARNING: only the first image replaced since the renderer drained `retired` can be on the GPU; later ones are dropped so unrendered windows do not grow it.
                    if !Arc::ptr_eq(&old, &canvas.image) && state.retired_canvases.insert(id) {
                        state.retired.push(old);
                    }
                }
                state.dirty = true;
                Ok(Value::Null)
            }
            "dom.canvasURL" => {
                use base64::Engine as _;
                let bytes = if let Some(canvas) = state.canvases.get(&id) {
                    canvas.pixels.encode_png()?
                } else if let Some(image) = state.images.get(&id) {
                    let size = image.size();
                    let mut bytes = image.bytes().to_vec();
                    for pixel in bytes.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    let pixels =
                        image::RgbaImage::from_raw(size.width.into(), size.height.into(), bytes)
                            .context("Invalid image pixels")?;
                    let mut out = std::io::Cursor::new(Vec::new());
                    pixels.write_to(&mut out, image::ImageFormat::Png)?;
                    out.into_inner()
                } else {
                    let width = doc.nodes[id]
                        .attrs
                        .get("width")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(300);
                    let height = doc.nodes[id]
                        .attrs
                        .get("height")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(150);
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
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(300);
                    let height = doc.nodes[id]
                        .attrs
                        .get("height")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(150);
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
                        || crate::render::canvas::fonts()
                            .lock()
                            .unwrap()
                            .faces()
                            .any(|face| face
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
                        offset += if current.tag == "#text" {
                            index
                        } else {
                            current
                                .children
                                .iter()
                                .take(index)
                                .map(|child| doc.text(*child).encode_utf16().count())
                                .sum()
                        };
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
            "dom.textRect" | "dom.textIndex" => {
                let Some(bounds) = state.bounds.get(&id) else {
                    return Ok(if op == "dom.textRect" {
                        json!([])
                    } else {
                        json!(0)
                    });
                };
                if let Some((layout, text, range, rows)) = state.text_layouts.get(&id) {
                    let content = &text[range.clone()];
                    if op == "dom.textIndex" {
                        let position = render::point(
                            render::px(args[1].as_f64().unwrap_or(0.) as f32),
                            render::px(args[2].as_f64().unwrap_or(0.) as f32),
                        );
                        let index = crate::render::text::text_index(layout, text, rows, position)
                            .unwrap_or_else(|index| index)
                            .clamp(range.start, range.end);
                        return Ok(json!(text[range.start..index].encode_utf16().count()));
                    }
                    let start = args[1].as_u64().unwrap_or(0) as usize;
                    let end = args[2].as_u64().unwrap_or(start as u64) as usize;
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
                    .as_ref()
                    .context("Text geometry requires a displayed window")?;
                let Some(style) = state
                    .computed
                    .as_ref()
                    .and_then(|styles| styles.values.get(id))
                else {
                    return Ok(if op == "dom.textRect" {
                        json!([])
                    } else {
                        json!(0)
                    });
                };
                let text = if matches!(doc.nodes[id].tag.as_str(), "input" | "textarea") {
                    doc.nodes[id]
                        .attrs
                        .get("value")
                        .cloned()
                        .unwrap_or_else(|| doc.text(id))
                } else {
                    doc.text(id)
                };
                let font_size = style::length(
                    style.get("font-size").map(String::as_str).unwrap_or("16px"),
                    16.,
                    16.,
                    viewport[0],
                    viewport[1],
                );
                let height = style::length(
                    style
                        .get("line-height")
                        .map(String::as_str)
                        .unwrap_or("20px"),
                    font_size,
                    font_size,
                    viewport[0],
                    viewport[1],
                )
                .max(1.);
                let run = render::TextRun {
                    len: text.len(),
                    font: style::font(style),
                    color: render::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                    letter_spacing: render::px(0.),
                    word_spacing: render::px(0.),
                    font_size: None,
                };
                let wrap = (!style
                    .get("white-space")
                    .is_some_and(|s| matches!(s.as_str(), "pre" | "nowrap")))
                .then_some(render::px(bounds[2]));
                let lines = system.shape_text(
                    text.clone().into(),
                    render::px(font_size),
                    &[run],
                    wrap,
                    None,
                )?;
                let mut top = 0.;
                let mut offset = 0;
                let mut rects: Vec<[f32; 4]> = Vec::new();
                let start = args[1].as_u64().unwrap_or(0) as usize;
                let end = args[2].as_u64().unwrap_or(start as u64) as usize;
                for line in lines {
                    let rows = line.wrap_boundaries.len() + 1;
                    if op == "dom.textIndex" {
                        let y = args[2].as_f64().unwrap_or(0.) as f32 - bounds[1];
                        if y < top + rows as f32 * height {
                            let index = line.closest_index_for_position(
                                render::point(
                                    render::px(args[1].as_f64().unwrap_or(0.) as f32 - bounds[0]),
                                    render::px(y - top),
                                ),
                                render::px(height),
                            );
                            let index = index.unwrap_or_else(|index| index);
                            return Ok(json!(offset + line.text[..index].encode_utf16().count()));
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
                                if let Some(point) = line.position_for_index(byte, render::px(height))
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
                    json!(text.encode_utf16().count())
                })
            }
            "dom.measureText" => {
                let system = state
                    .text_system
                    .as_ref()
                    .context("Text measurement requires a displayed window")?;
                let text = args[1]
                    .as_str()
                    .context("Expected text")?
                    .replace(['\n', '\r', '\t'], " ");
                let font = args[2].as_str().unwrap_or("10px sans-serif");
                let (size, family) = font.split_once("px").unwrap_or(("10", "sans-serif"));
                let font_size = size
                    .split_whitespace()
                    .last()
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(10.);
                let mut face = style::font(&style::Style::from([
                    ("font-family".into(), family.trim().into()),
                    (
                        "font-weight".into(),
                        size.split_whitespace()
                            .rev()
                            .skip(1)
                            .find(|part| *part == "bold" || part.parse::<u16>().is_ok())
                            .unwrap_or("400")
                            .into(),
                    ),
                ]));
                if let Some(family) = state.font_aliases.get(face.family.as_ref()) {
                    face.family = family.clone().into();
                }
                let run = render::TextRun {
                    len: text.len(),
                    font: face,
                    color: render::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                    letter_spacing: render::px(0.),
                    word_spacing: render::px(0.),
                    font_size: None,
                };
                let line =
                    system.shape_line(text.to_owned().into(), render::px(font_size), &[run], None);
                Ok(
                    json!({"width":f32::from(line.width),"actualBoundingBoxLeft":0,"actualBoundingBoxRight":f32::from(line.width),"actualBoundingBoxAscent":f32::from(line.ascent),"actualBoundingBoxDescent":f32::from(line.descent),"fontBoundingBoxAscent":f32::from(line.ascent),"fontBoundingBoxDescent":f32::from(line.descent)}),
                )
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
        let lazy = matches!(node.tag.as_str(), "script" | "style")
            && (node.stylesheet.is_some() || !node.text.is_empty());
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

fn scroll_offset(handle: &render::ScrollHandle) -> render::Point<render::Pixels> {
    let mut offset = handle.offset();
    let bounds = handle.bounds();
    if bounds.size.width > render::px(0.) || bounds.size.height > render::px(0.) {
        let max = handle.max_offset();
        offset.x = offset.x.clamp(-max.x, render::px(0.));
        offset.y = offset.y.clamp(-max.y, render::px(0.));
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
    let mut database = crate::render::canvas::fonts().lock().unwrap();
    let database = std::sync::Arc::make_mut(&mut database);
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
        system.as_deref(),
    );
    if state.layout_epoch == state.style_epoch {
        return;
    }
    state.layout_epoch = state.style_epoch;
    let mut known = rustc_hash::FxHashMap::default();
    for (id, style) in computed.values.iter().enumerate() {
        let bits = *known.entry(Rc::as_ptr(style)).or_insert_with(|| {
            let clip = ["overflow", "overflow-x", "overflow-y"].iter().any(|name| {
                style.get(*name).is_some_and(|value| {
                    matches!(value.as_str(), "hidden" | "clip" | "auto" | "scroll")
                })
            });
            let positioned = style.get("position").is_some_and(|value| value != "static");
            (if clip { MIRROR_CLIP } else { 0 }) | if positioned { MIRROR_POSITIONED } else { 0 }
        });
        state.bounds.reserve(id);
        unsafe {
            let flags = state.bounds.flags(id);
            *flags = (*flags & (MIRROR_BOUNDS | MIRROR_SCROLLER)) | bits;
        }
    }
}

pub(crate) fn styles(
    doc: &mut Document,
    state: &mut State,
    viewport: [f32; 2],
    system: Option<&render::TextSystem>,
) -> Rc<style::ComputedStyles> {
    let reduced_motion = style::reduced_motion();
    let theme = crate::desktop::native::theme_state();
    if state.style_viewport == viewport
        && state.style_reduced_motion == reduced_motion
        && state.style_theme == theme
        && state.style_metrics == system.is_some()
        && let Some(computed) = &state.computed
        && computed.values.len() == doc.nodes.len()
    {
        return computed.clone();
    }
    state.svg_sources.clear();
    let slots = slots(doc, state.shadows.iter().map(|(&host, &root)| (host, root)));
    let mut context = style::StyleContext {
        reduced_motion,
        theme,
        aliases: state.font_aliases.clone(),
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
        while let Some(id) = pending.pop() {
            if let Some(&root) = state.shadows.get(&id) {
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
    state.style_epoch += 1;
    state.style_viewport = viewport;
    state.style_metrics = system.is_some();
    state.style_reduced_motion = reduced_motion;
    state.style_theme = theme;
    let result = Rc::new(result);
    state.linted = false;
    state.computed = Some(result.clone());
    result
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
                "const editor=document.getElementById('editor');getComputedStyle(editor);",
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
                if(getComputedStyle(editor).color!=='red')throw Error('Connected style changed');
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
            if(getComputedStyle(editor.firstChild).color!=='blue')throw Error('Inserted style was stale');
            fragment.append(editor.firstChild);
        "#, "test").unwrap();
        assert!(host.borrow().dom[&1].computed.is_none());
        engine.eval(r#"
            const root=editor.attachShadow({mode:'open'});
            root.innerHTML='<span style="color:green">text</span>';
            if(getComputedStyle(root.firstChild).color!=='green')throw Error('Shadow style');
            root.firstChild.style.color='purple';
            if(getComputedStyle(root.firstChild).color!=='purple')throw Error('Shadow mutation was stale');
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
        let [p, svg] = ["p", "svg"].map(|tag| {
            host.borrow().pages[&1]
                .doc
                .nodes
                .iter()
                .position(|node| node.tag == tag)
                .unwrap()
        });
        let hover = |ids: &[usize]| {
            host.borrow_mut()
                .call(Role::Renderer(1), "dom.hover", json!([ids]))
                .unwrap();
        };
        engine
            .eval(
                "const p=document.getElementById('p');getComputedStyle(p);",
                "test",
            )
            .unwrap();
        let cached = computed();
        assert!(cached.is_some());
        host.borrow_mut()
            .dom
            .get_mut(&1)
            .unwrap()
            .svg_sources
            .insert(svg, ([1., 1.], String::new(), String::new()));
        engine
            .eval(
                "p.firstChild.data='other';document.querySelector('svg text').firstChild.data='b';",
                "test",
            )
            .unwrap();
        hover(&[p]);
        hover(&[]);
        assert_eq!(computed(), cached);
        assert!(!host.borrow().dom[&1].svg_sources.contains_key(&svg));
        engine
            .eval(
                r#"
            p.firstChild.data='';
            if(getComputedStyle(p).color!=='red')throw Error('Empty text was stale');
            const sheet=document.getElementById('s');
            sheet.textContent='p:hover{color:blue}';
            sheet.firstChild.data='p:hover{color:green}';
            if(getComputedStyle(p).color==='green')throw Error('Hover without state');
        "#,
                "test",
            )
            .unwrap();
        hover(&[p]);
        assert!(computed().is_none());
        engine
            .eval(
                "if(getComputedStyle(p).color!=='green')throw Error('Hover state was stale')",
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
        engine.eval("getComputedStyle(p)", "test").unwrap();
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
        doc.nodes[sheet].text = ".replacement{color:blue}".into();
        state.computed = None;
        styles(&mut doc, &mut state, [800., 600.], None);
        assert_eq!(state.sheets.len(), 200);
        assert!(!state.sheets.contains_key(".item0{color:red}"));
        doc.nodes[0].children.retain(|id| *id != sheet);
        doc.nodes[sheet].parent = None;
        state.computed = None;
        styles(&mut doc, &mut state, [800., 600.], None);
        assert_eq!(state.sheets.len(), 199);
        assert!(!state.sheets.contains_key(".replacement{color:blue}"));
    }
}
