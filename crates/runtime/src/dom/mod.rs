mod document;
mod selector;

pub use document::{Attrs, Document, Node, decode, escape};
pub(crate) use selector::{
    ACTIVE, FOCUS, FOCUS_WITHIN, HOVER, Selector, SelectorCache, SelectorList,
};

use crate::style;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::HashMap, rc::Rc};

/// One visual row of wrapped text: its shift for alignment and indent, its right edge, the extra
/// space after each space when justified, its first byte, and how far it moves down.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Row {
    pub shift: gpui::Pixels,
    pub right: gpui::Pixels,
    pub gap: gpui::Pixels,
    pub start: usize,
    /// How far the row moves down, when the first row is taller.
    pub drop: gpui::Pixels,
}

pub(crate) type Rows = Rc<[Row]>;

pub(crate) struct LayoutNode {
    pub node: Node,
    pub style: Rc<style::Style>,
    pub children: Vec<(usize, u64)>,
    pub revision: u64,
}

#[derive(Default)]
pub struct State {
    pub base: Option<url::Url>,
    pub(crate) free: Vec<usize>,
    live: std::collections::HashSet<usize>,
    pub bounds: HashMap<usize, [f32; 4]>,
    pub(crate) text_layouts:
        HashMap<usize, (gpui::TextLayout, Rc<str>, std::ops::Range<usize>, Rows)>,
    pub scroll: HashMap<usize, gpui::ScrollHandle>,
    scroll_offsets: HashMap<usize, gpui::Point<gpui::Pixels>>,
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
    pub(crate) layout_nodes: Vec<Option<LayoutNode>>,
    pub(crate) measurements: crate::render::grid::Measurements,
    pub(crate) inline_text: std::collections::HashMap<usize, Option<Rc<style::InlineText>>>,
    pub(crate) layout_revision: u64,
    pub(crate) sheets: HashMap<String, Rc<style::Stylesheet>>,
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
    pub images: HashMap<usize, std::sync::Arc<gpui::RenderImage>>,
    pub shadows: HashMap<usize, usize>,
    pub(crate) svg_sources: HashMap<usize, ([f32; 2], String, String)>,
    pub(crate) svg_images: HashMap<usize, (Rc<str>, [f32; 2])>,
    /// Rasterized `clip-path` and `mask-image` shapes, by node.
    pub(crate) masks: HashMap<usize, (String, std::sync::Arc<gpui::RenderImage>)>,
    pub(crate) image_crops: HashMap<usize, (u64, std::sync::Arc<gpui::RenderImage>)>,
    pub svgs: HashMap<usize, (String, std::sync::Arc<gpui::RenderImage>)>,
    pub backgrounds: HashMap<(usize, usize), (String, std::sync::Arc<gpui::RenderImage>)>,
    pub font_aliases: HashMap<String, String>,
    pub text_system: Option<std::rc::Rc<gpui::WindowTextSystem>>,
    pub pixel_ratio: f32,
    pub(crate) linted: bool,
    pub(crate) warned: std::collections::HashSet<String>,
    pub(crate) motion: style::Motion,
    pub(crate) containers: HashMap<usize, [f32; 2]>,
    pub(crate) container_pass: bool,
    pub(crate) anchors: HashMap<usize, gpui::Bounds<gpui::Pixels>>,
}

impl State {
    pub(crate) fn update_containers(&mut self) -> bool {
        let changed = self
            .style_cache
            .set_containers(std::mem::take(&mut self.containers));
        if changed {
            self.computed = None;
        }
        changed
    }

    fn slots(&self, doc: &Document) -> HashMap<usize, Vec<usize>> {
        let mut slots = HashMap::new();
        for (&host, &root) in &self.shadows {
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

    pub(crate) fn allocate(&mut self, doc: &mut Document, node: Node) -> usize {
        let id = if let Some(id) = self.free.pop() {
            doc.nodes[id] = node;
            id
        } else {
            let id = doc.nodes.len();
            doc.nodes.push(node);
            id
        };
        if let Some(cached) = &mut self.computed {
            if let Some(computed) = Rc::get_mut(cached) {
                computed
                    .values
                    .resize_with(doc.nodes.len(), Default::default);
                computed.values[id] = Rc::default();
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
        id
    }
}

pub(crate) fn load_image(state: &mut State, id: usize, bytes: &[u8]) -> Result<Value> {
    ensure!(bytes.len() <= 64 * 1024 * 1024, "Image is too large");
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    state.dirty = true;
    let mut pixels = match crate::render::image::decode(bytes, limits) {
        Ok(image) => image.into_rgba8(),
        Err(error) => {
            let size = resvg::usvg::Tree::from_data(bytes, &Default::default())
                .map_err(|_| error)?
                .size();
            state.images.remove(&id);
            let text = if bytes.starts_with(&[0x1f, 0x8b]) {
                resvg::usvg::decompress_svgz(bytes)?
            } else {
                bytes.to_vec()
            };
            state.svg_images.insert(
                id,
                (
                    String::from_utf8_lossy(&text).into(),
                    [size.width(), size.height()],
                ),
            );
            return Ok(json!([size.width(), size.height()]));
        }
    };
    let dimensions = [pixels.width(), pixels.height()];
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    state.svg_images.remove(&id);
    state.images.insert(
        id,
        std::sync::Arc::new(gpui::RenderImage::new(vec![image::Frame::new(pixels)])),
    );
    Ok(json!(dimensions))
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
            | "dom.snapshot"
            | "dom.matches"
            | "dom.mediaMatches"
            | "dom.assignedSlot"
            | "dom.assignedNodes"
            | "dom.queryWithin"
            | "dom.html"
            | "dom.createSpecial"
            | "dom.rect"
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
            | "dom.measureText"
            | "dom.textRect"
            | "dom.textIndex"
            | "dom.textOffset"
            | "dom.textPoint"
            | "dom.font"
            | "dom.fontCheck"
            | "dom.shadow"
            | "dom.hitTest"
            | "dom.sibling"
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
            return Ok(json!(changed));
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
                .entry(source.clone())
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
            let roots: Vec<_> = doc
                .nodes
                .iter()
                .enumerate()
                .filter(|(id, node)| {
                    *id != 0
                        && node.parent.is_none()
                        && !node.tag.is_empty()
                        && !state.shadows.values().any(|root| root == id)
                })
                .map(|(id, _)| id)
                .collect();
            let mut freed = Vec::new();
            for root in roots {
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
                    continue;
                }
                for id in subtree {
                    doc.nodes[id] = Node::default();
                    state.bounds.remove(&id);
                    state.text_layouts.remove(&id);
                    state.scroll.remove(&id);
                    state.scroll_offsets.remove(&id);
                    state.grids.remove(&id);
                    if let Some(node) = state.layout_nodes.get_mut(id) {
                        *node = None;
                    }
                    state.measurements.retain(|(node, ..), _| *node != id);
                    state.inline_text.remove(&id);
                    state.images.remove(&id);
                    state.canvases.remove(&id);
                    state.svgs.remove(&id);
                    state.svg_sources.remove(&id);
                    state.svg_images.remove(&id);
                    state.image_crops.remove(&id);
                    state.shadows.remove(&id);
                    state.motion.forget(id);
                    state.anchors.remove(&id);
                    state.backgrounds.retain(|(node, _), _| *node != id);
                    if state.focus == Some(id) {
                        state.focus = None;
                    }
                    state.selection.remove(&id);
                    if state.selection_caret == Some(id) {
                        state.selection_caret = None;
                    }
                    state.free.push(id);
                    freed.push(id);
                }
            }
            if !freed.is_empty() {
                state.computed = None;
                state.dirty = true;
            }
            return Ok(json!(freed));
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
            let following = |mut id: usize| {
                loop {
                    let parent = doc.nodes[id].parent?;
                    let children = &doc.nodes[parent].children;
                    let index = children.iter().position(|&child| child == id)?;
                    if let Some(&next) = children.get(index + 1) {
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
                system.as_deref().map(|system| &***system),
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
            state.computed = None;
            state.dirty = true;
            return Ok(Value::Null);
        }
        if op == "dom.createSpecial" {
            let tag = args[0].as_str().context("Expected node kind")?;
            ensure!(
                matches!(tag, "#comment" | "#fragment" | "#document"),
                "Invalid node kind"
            );
            let id = state.allocate(
                doc,
                Node {
                    tag: tag.into(),
                    text: args[1].as_str().unwrap_or("").into(),
                    ..Default::default()
                },
            );
            return Ok(json!(id));
        }
        let id = args[0].as_u64().context("Expected node id")? as usize;
        ensure!(id < doc.nodes.len(), "Invalid node id");
        match op {
            "dom.assignedSlot" => {
                Ok(json!(state.slots(doc).into_iter().find_map(
                    |(slot, children)| children.contains(&id).then_some(slot)
                )))
            }
            "dom.assignedNodes" => {
                let slots = state.slots(doc);
                let mut nodes = slots.get(&id).cloned().unwrap_or_default();
                if args[1].as_bool() == Some(true) && slots.contains_key(&id) {
                    if nodes.is_empty() {
                        nodes = doc.nodes[id].children.clone();
                    }
                    let mut pending: Vec<_> = nodes.into_iter().rev().collect();
                    nodes = Vec::new();
                    while let Some(child) = pending.pop() {
                        if let Some(assigned) = slots.get(&child) {
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
            "dom.snapshot" => {
                let node = &doc.nodes[id];
                let attrs: Vec<_> = node.attrs.iter().collect();
                Ok(
                    json!({"tag":node.tag,"text":node.text,"attrs":attrs,"parent":node.parent,"children":node.children}),
                )
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
                Ok(Value::Null)
            }
            "dom.sibling" => Ok(json!(doc.nodes[id].parent.and_then(|parent| {
                let siblings = &doc.nodes[parent].children;
                let index = siblings.iter().position(|&child| child == id)?;
                if args[1].as_i64() == Some(-1) {
                    index.checked_sub(1).map(|index| siblings[index])
                } else {
                    siblings.get(index + 1).copied()
                }
            }))),
            "dom.rect" => {
                let bounds = state.bounds.get(&id).copied().unwrap_or([0.; 4]);
                let scroll = state.scroll.get(&id);
                let offset = scroll.map(scroll_offset).unwrap_or_default();
                let max = scroll.map(|s| s.max_offset()).unwrap_or_default();
                Ok(
                    json!({"x":bounds[0],"y":bounds[1],"left":bounds[0],"top":bounds[1],"width":bounds[2],"height":bounds[3],"right":bounds[0]+bounds[2],"bottom":bounds[1]+bounds[3],"scrollLeft":-f32::from(offset.x),"scrollTop":-f32::from(offset.y),"scrollWidth":bounds[2]+f32::from(max.x),"scrollHeight":bounds[3]+f32::from(max.y)}),
                )
            }
            "dom.style" => {
                let system = state.text_system.clone();
                let styles = styles(
                    doc,
                    state,
                    viewport,
                    system.as_deref().map(|system| &***system),
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
                    None => json!(style.iter().collect::<HashMap<_, _>>()),
                })
            }
            "dom.shadow" => {
                let root = args[1].as_u64().context("Expected shadow root")? as usize;
                ensure!(
                    doc.nodes
                        .get(root)
                        .is_some_and(|node| node.tag == "#fragment"),
                    "Invalid shadow root"
                );
                ensure!(
                    !state.shadows.contains_key(&id),
                    "Shadow root already exists"
                );
                state.shadows.insert(id, root);
                state.dirty = true;
                Ok(Value::Null)
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
                    system.as_deref().map(|system| &***system),
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
                let handle = state.scroll.entry(id).or_default();
                let previous = handle.offset();
                handle.set_offset(gpui::point(
                    gpui::px(
                        -args[1]
                            .as_f64()
                            .map(|v| v as f32)
                            .unwrap_or(-f32::from(previous.x)),
                    ),
                    gpui::px(
                        -args[2]
                            .as_f64()
                            .map(|v| v as f32)
                            .unwrap_or(-f32::from(previous.y)),
                    ),
                ));
                handle.set_offset(scroll_offset(handle));
                state.dirty |= handle.offset() != previous;
                Ok(Value::Null)
            }
            "dom.canvas" => {
                if args[1].is_null() {
                    state.canvases.remove(&id);
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
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        state.canvases.entry(id)
                    {
                        entry.insert(crate::render::canvas::Canvas::new(width, height)?);
                    }
                    state
                        .canvases
                        .get_mut(&id)
                        .unwrap()
                        .draw(args[1].as_array().context("Expected canvas commands")?)?;
                }
                state.dirty = true;
                Ok(Value::Null)
            }
            "dom.canvasURL" => {
                use base64::Engine as _;
                let bytes = if let Some(canvas) = state.canvases.get(&id) {
                    canvas.pixels.encode_png()?
                } else if let Some(image) = state.images.get(&id) {
                    let size = image.size(0);
                    let mut bytes = image.as_bytes(0).context("Image has no pixels")?.to_vec();
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
            "dom.image" => load_image(state, id, &crate::web::bytes(&args[1])?),
            "dom.font" => {
                let system = state
                    .text_system
                    .as_ref()
                    .context("Font loading requires a displayed window")?;
                let bytes = crate::web::bytes(&args[2])?;
                ensure!(bytes.len() <= 32 * 1024 * 1024, "Font exceeds 32 MB limit");
                let mut database = crate::render::canvas::fonts().lock().unwrap();
                let database = std::sync::Arc::make_mut(&mut database);
                let ids = database.load_font_source(resvg::usvg::fontdb::Source::Binary(
                    std::sync::Arc::new(bytes.clone()),
                ));
                let face = ids
                    .first()
                    .and_then(|id| database.face(*id))
                    .context("Unsupported or invalid font data")?;
                let family = face
                    .families
                    .first()
                    .context("Font has no family name")?
                    .0
                    .clone();
                system.add_fonts(vec![std::borrow::Cow::Owned(bytes)])?;
                style::FONT_NAMES.with(|names| *names.borrow_mut() = Default::default());
                state.style_cache = style::StyleCache::default();
                state.computed = None;
                state.font_aliases.insert(
                    args[1].as_str().context("Expected font family")?.to_owned(),
                    family,
                );
                state.dirty = true;
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
                        let position = gpui::point(
                            gpui::px(args[1].as_f64().unwrap_or(0.) as f32),
                            gpui::px(args[2].as_f64().unwrap_or(0.) as f32),
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
                let run = gpui::TextRun {
                    len: text.len(),
                    font: style::font(style),
                    color: gpui::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                    letter_spacing: gpui::px(0.),
                    word_spacing: gpui::px(0.),
                    font_size: None,
                };
                let wrap = (!style
                    .get("white-space")
                    .is_some_and(|s| matches!(s.as_str(), "pre" | "nowrap")))
                .then_some(gpui::px(bounds[2]));
                let lines = system.shape_text(
                    text.clone().into(),
                    gpui::px(font_size),
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
                                gpui::point(
                                    gpui::px(args[1].as_f64().unwrap_or(0.) as f32 - bounds[0]),
                                    gpui::px(y - top),
                                ),
                                gpui::px(height),
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
                                if let Some(point) = line.position_for_index(byte, gpui::px(height))
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
                let run = gpui::TextRun {
                    len: text.len(),
                    font: face,
                    color: gpui::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                    letter_spacing: gpui::px(0.),
                    word_spacing: gpui::px(0.),
                    font_size: None,
                };
                let line =
                    system.shape_line(text.to_owned().into(), gpui::px(font_size), &[run], None);
                Ok(
                    json!({"width":f32::from(line.width),"actualBoundingBoxLeft":0,"actualBoundingBoxRight":f32::from(line.width),"actualBoundingBoxAscent":f32::from(line.ascent),"actualBoundingBoxDescent":f32::from(line.descent),"fontBoundingBoxAscent":f32::from(line.ascent),"fontBoundingBoxDescent":f32::from(line.descent)}),
                )
            }
            _ => unreachable!(),
        }
    })())
}

fn scroll_offset(handle: &gpui::ScrollHandle) -> gpui::Point<gpui::Pixels> {
    let mut offset = handle.offset();
    let bounds = handle.bounds();
    if bounds.size.width > gpui::px(0.) || bounds.size.height > gpui::px(0.) {
        let max = handle.max_offset();
        offset.x = offset.x.clamp(-max.x, gpui::px(0.));
        offset.y = offset.y.clamp(-max.y, gpui::px(0.));
    }
    offset
}

pub(crate) fn styles(
    doc: &mut Document,
    state: &mut State,
    viewport: [f32; 2],
    system: Option<&gpui::TextSystem>,
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
    let mut sources = std::collections::HashSet::new();
    let mut pending = vec![0];
    while let Some(id) = pending.pop() {
        let node = &doc.nodes[id];
        pending.extend(&node.children);
        if let Some(root) = state.shadows.get(&id) {
            pending.push(*root);
        }
        if node.tag == "style" && node.stylesheet.is_none() {
            sources.insert(doc.text(id));
        }
    }
    state.sheets.retain(|source, _| sources.contains(source));
    state.svg_sources.clear();
    let slots = state.slots(doc);
    let mut context = style::StyleContext::default();
    let mut required = std::collections::HashSet::from([0]);
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
    let mut result = style::compute_styles(
        doc,
        viewport[0],
        viewport[1],
        system,
        &mut state.sheets,
        &mut state.style_cache,
        None,
    );
    for pass in 0..if state.shadows.is_empty() { 1 } else { 2 } {
        if pass == 1 {
            context.required = None;
            result = style::compute_styles(
                doc,
                viewport[0],
                viewport[1],
                system,
                &mut state.sheets,
                &mut state.style_cache,
                Some(&context),
            );
        }
        let mut pending = vec![0];
        while let Some(id) = pending.pop() {
            if let Some(root) = state.shadows.get(&id) {
                let mut scoped_root = doc.nodes[*root].clone();
                scoped_root.parent = Some(0);
                scoped_root.tag = "apps-shadow-root".into();
                scoped_root.attrs = doc.nodes[id].attrs.clone();
                scoped_root.inline = None;
                scoped_root.attrs.remove("style");
                scoped_root.state = doc.nodes[id].state;
                if pass == 1 {
                    context
                        .roots
                        .insert(*root, (result.values[id].clone(), result.lengths[id]));
                }
                let inherit = result.values[id]
                    .iter()
                    .filter(|(key, _)| {
                        key.starts_with("--")
                            || matches!(
                                key.as_str(),
                                "color"
                                    | "font-family"
                                    | "font-size"
                                    | "font-weight"
                                    | "line-height"
                                    | "text-align"
                                    | "white-space"
                                    | "color-scheme"
                                    | "pointer-events"
                                    | "caret-color"
                                    | "cursor"
                                    | "visibility"
                            )
                    })
                    .map(|(key, value)| format!("{key}:{value}"))
                    .collect::<Vec<_>>()
                    .join(";");
                let mut scoped_doc = Node {
                    tag: "#document".into(),
                    children: vec![*root],
                    ..Default::default()
                };
                scoped_doc.attrs.insert("style".into(), inherit);
                let document = std::mem::replace(&mut doc.nodes[0], scoped_doc);
                let shadow = std::mem::replace(&mut doc.nodes[*root], scoped_root);
                let mut scoped = style::compute_styles(
                    doc,
                    viewport[0],
                    viewport[1],
                    system,
                    &mut state.sheets,
                    &mut state.style_cache,
                    Some(&context),
                );
                doc.nodes[0] = document;
                doc.nodes[*root] = shadow;
                if pass == 0 {
                    context
                        .hosts
                        .insert(id, std::mem::take(&mut scoped.host_declarations));
                }
                let mut children = vec![*root];
                while let Some(child) = children.pop() {
                    if let Some(animations) = scoped.animations.remove(&child) {
                        result.animations.insert(child, animations);
                    }
                    if let Some(selection) = scoped.selections.remove(&child) {
                        result.selections.insert(child, selection);
                    }
                    if let Some(pseudos) = scoped.pseudos.remove(&child) {
                        result.pseudos.insert(child, pseudos);
                    }
                    if let Some(pseudos) = scoped.text_pseudos.remove(&child) {
                        result.text_pseudos.insert(child, pseudos);
                    }
                    result.values[child] = std::mem::take(&mut scoped.values[child]);
                    result.lengths[child] = scoped.lengths[child];
                    children.extend(&doc.nodes[child].children);
                }
                pending.push(*root);
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
    for style in &mut result.values {
        if let Some(family) = style
            .get("font-family")
            .and_then(|family| state.font_aliases.get(family.trim_matches(['\'', '"'])))
            .cloned()
        {
            Rc::make_mut(style).insert("font-family".into(), family);
        }
    }
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
        let style = &result.values[id];
        let hidden = hidden || style.get("display").is_some_and(|value| value == "none");
        let mode = if hidden
            || style
                .get("visibility")
                .is_some_and(|v| matches!(v.as_str(), "hidden" | "collapse"))
        {
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
        } else {
            match style.get("user-select").map(String::as_str) {
                Some("none") => style::UserSelect::None,
                Some("all") => style::UserSelect::All,
                Some("text") => style::UserSelect::Text,
                _ => inherited.0,
            }
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
        if children.iter().any(|child| {
            result.values[*child]
                .get("display")
                .is_some_and(|v| v == "contents")
        }) {
            let mut flattened = Vec::new();
            for &child in children {
                if result.values[child]
                    .get("display")
                    .is_some_and(|v| v == "contents")
                {
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
