use super::{
    PageState,
    background::css_background,
    cull,
    paint::{DomBox, Turned},
    scrollbar,
    text::InlineText,
};
use crate::style::Prop;
use crate::{
    dom::Document,
    style::{self, ComputedStyles},
};
use renderer::{Child, Color, Rect, SharedString, point, px, relative, size};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

pub(super) struct Build<'a> {
    pub(super) doc: &'a Document,
    pub(super) styles: &'a ComputedStyles,
    pub(super) text: &'a renderer::TextSystem,
    pub(super) pixel_ratio: f32,
    pub(super) view: &'a Rc<PageState>,
    pub(super) ancestors: Rc<[usize]>,
    pub(super) flex_parent: bool,
    pub(super) measured: &'a RefCell<super::grid::Measurements>,
    pub(super) perspective: Option<(f32, String)>,
    pub(super) siblings: &'a RefCell<HashMap<usize, Rc<Siblings>>>,
    // The index of each child of a parent with a `<br>`.
    pub(super) breaks: &'a RefCell<rustc_hash::FxHashMap<usize, Siblings>>,
    pub(super) walks: &'a RefCell<rustc_hash::FxHashMap<usize, Used>>,
    pub(super) adjoins: &'a RefCell<rustc_hash::FxHashMap<usize, Sets>>,
    pub(super) blocks: &'a RefCell<rustc_hash::FxHashMap<usize, [f32; 2]>>,
    pub(super) ids: &'a Rc<super::effects::Ids>,
    pub(super) info: &'a [cull::Info],
    pub(super) zone: Option<Rc<cull::Zone>>,
    // Inside a multi-column box, where boxes take break flags.
    pub(super) columned: bool,
    pub(super) top_layer: &'a [usize],
    pub(super) tree: &'a renderer::Tree,
    pub(super) culls: &'a std::cell::Cell<usize>,
    pub(super) resized: bool,
    pub(super) settled: bool,
    pub(super) reads: &'a RefCell<Vec<Read>>,
    pub(super) epoch: u64,
    #[cfg(debug_assertions)]
    pub(super) prints: &'a RefCell<Vec<u64>>,
    #[cfg(debug_assertions)]
    pub(super) checking: bool,
    #[cfg(debug_assertions)]
    pub(super) extra: std::cell::Cell<u64>,
    #[cfg(debug_assertions)]
    pub(super) plan: RefCell<Rc<[usize]>>,
}

// Log external reads through Build::log, or disable reuse of this box.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(debug_assertions, derive(Debug))]
pub(super) enum Query {
    Margins,
    Parent(Prop),
    ParentFont,
    FlatParent,
    FlatParentPadding,
    StickyParent,
    Break,
    UserSelect,
    Positioner(bool),
    Backdrop(u32),
    // The state of the scroller walk when it reaches the box that holds the read.
    Scroller(u8),
    Editable,
    Cell,
    Shrunk,
    Uses(u32),
    Block(u32),
    Scroll(u32),
    Width(u32),
    Track(u32, u8),
    Baselines(u32),
    Atom(u32),
    Image(u32),
    Look(u32),
    Boxless(u32),
    RowFill,
    Collapse,
    Value(u32),
    Viewport,
    Theme,
    LineBase(u32),
    Shrinks(u32),
    Spliced(u32),
}

impl Query {
    fn moved(self, from: u32, to: u32) -> Option<Self> {
        let at = |node: u32| (node == from).then_some(to);
        Some(match self {
            Query::Uses(node) => Query::Uses(at(node)?),
            Query::Block(node) => Query::Block(at(node)?),
            Query::Scroll(node) => Query::Scroll(at(node)?),
            Query::Width(node) => Query::Width(at(node)?),
            Query::Backdrop(node) => Query::Backdrop(at(node)?),
            Query::Track(node, axis) => Query::Track(at(node)?, axis),
            Query::Baselines(node) => Query::Baselines(at(node)?),
            Query::Atom(node) => Query::Atom(at(node)?),
            Query::Image(node) => Query::Image(at(node)?),
            Query::Look(node) => Query::Look(at(node)?),
            Query::Boxless(node) => Query::Boxless(at(node)?),
            Query::Value(node) => Query::Value(at(node)?),
            Query::LineBase(node) => Query::LineBase(at(node)?),
            Query::Shrinks(node) => Query::Shrinks(at(node)?),
            Query::Spliced(node) => Query::Spliced(at(node)?),
            query => query,
        })
    }

    fn host(self) -> bool {
        matches!(
            self,
            Query::Scroller(_)
                | Query::Block(_)
                | Query::Scroll(_)
                | Query::Width(_)
                | Query::Track(..)
                | Query::Baselines(_)
                | Query::Atom(_)
                | Query::Image(_)
                | Query::Viewport
                | Query::Theme
                | Query::Cell
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Read {
    query: Query,
    value: u64,
    scope: u16,
    reader: u16,
}

pub(super) struct Patch {
    pub(super) ancestors: Rc<[usize]>,
    pub(super) containing: [f32; 2],
    pub(super) container: [f32; 2],
    pub(super) slot: Option<([f32; 4], super::grid::Grid)>,
    pub(super) flex_parent: bool,
    pub(super) columned: bool,
    pub(super) perspective: Option<(f32, String)>,
    pub(super) value: u64,
}

pub(super) struct Shared {
    plan: Rc<super::plan::Plan>,
    reads: Box<[Read]>,
    style: Rc<renderer::Style>,
}

pub(super) struct Kept {
    pub(super) call: Call,
    pub(super) reads: Box<[Read]>,
    ancestors: Rc<[usize]>,
    epoch: std::cell::Cell<u64>,
    // The call drops a height that the style doesn't read, but a build in place needs it: a stretched image
    // takes its width from it.
    containing: std::cell::Cell<[f32; 2]>,
    pub(super) culled: bool,
    // The cell of a grid item, so it can build in place.
    slot: Option<([f32; 4], super::grid::Grid)>,
    #[cfg(debug_assertions)]
    pub(super) print: u64,
    // The print that the records around hold. It stays when the box builds in place.
    #[cfg(debug_assertions)]
    pub(super) seen: std::cell::Cell<u64>,
    #[cfg(debug_assertions)]
    plan: Rc<[usize]>,
}

impl Kept {
    pub(super) fn in_place(&self) -> Option<&Rc<[usize]>> {
        (!self.culled && (self.call.cell.is_none() || self.slot.is_some()))
            .then_some(&self.ancestors)
    }

    // The revision of the boxes that the record was built from.
    pub(super) fn subtree(&self) -> u64 {
        self.call.subtree
    }

    pub(super) fn same_reads(&self, other: &Self) -> bool {
        self.reads == other.reads
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Call {
    subtree: u64,
    containing: [f32; 2],
    container: [f32; 2],
    flex_parent: bool,
    columned: bool,
    cell: Option<(bool, bool, u64)>,
}

#[derive(PartialEq)]
enum Replay {
    Same,
    Close,
    Changed,
}

pub(super) fn hashed(value: impl std::hash::Hash) -> u64 {
    use std::hash::BuildHasher;
    rustc_hash::FxBuildHasher.hash_one(value)
}

fn parent_style<'b>(
    doc: &Document,
    styles: &'b ComputedStyles,
    id: usize,
    prop: Prop,
) -> Option<&'b str> {
    box_parent(doc, styles, id).and_then(|parent| styles.values[parent].get(prop))
}

fn row_fill(doc: &Document, styles: &ComputedStyles, id: usize) -> Option<Color> {
    let fill = |node: usize| {
        styles.values[node]
            .get(Prop::BACKGROUND_COLOR)
            .and_then(style::paint)
            .filter(|color| color.a > 0.)
    };
    let mut table = None;
    for row in std::iter::successors(doc.nodes[id].parent, |&parent| doc.nodes[parent].parent) {
        if !(style::ROW..=style::FOOTER).contains(&style::role(styles, row)) {
            table = Some(row);
            break;
        }
        if let Some(fill) = fill(row) {
            return Some(fill);
        }
    }
    // Columns paint under the cells, after rows. Rows spanned from above are not counted.
    let (table, row) = (table?, doc.nodes[id].parent?);
    let span = |node: usize, name: &str| {
        doc.nodes[node]
            .attrs
            .get(name)
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 1000)
    };
    let column: usize = doc.nodes[row]
        .children
        .iter()
        .take_while(|&&cell| cell != id)
        .filter(|&&cell| style::role(styles, cell) == style::CELL)
        .map(|&cell| span(cell, "colspan"))
        .sum();
    let mut at = 0;
    for &group in &doc.nodes[table].children {
        if doc.nodes[group].tag != "colgroup" {
            continue;
        }
        let start = at;
        let mut found = None;
        for &col in &doc.nodes[group].children {
            if doc.nodes[col].tag == "col" {
                if (at..at + span(col, "span")).contains(&column) {
                    found = fill(col);
                }
                at += span(col, "span");
            }
        }
        if at == start {
            at += span(group, "span");
        }
        if (start..at).contains(&column) {
            return found.or_else(|| fill(group));
        }
    }
    None
}

fn collapses(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    std::iter::successors(doc.nodes[id].parent, |&parent| doc.nodes[parent].parent)
        .find(|&parent| style::table_box(style::role(styles, parent)))
        .is_some_and(|table| styles.values[table].get(Prop::BORDER_COLLAPSE) == Some("collapse"))
}

fn parent_font(
    doc: &Document,
    styles: &ComputedStyles,
    id: usize,
) -> Option<([u32; 6], renderer::Font)> {
    doc.nodes[id].parent.map(|parent| {
        let font = styles.lengths[parent].font;
        (
            [font.em, font.ch, font.ex, font.cap, font.ic, font.lh].map(f32::to_bits),
            style::font(&styles.values[parent]),
        )
    })
}

fn box_parent(doc: &Document, styles: &ComputedStyles, id: usize) -> Option<usize> {
    let mut parent = doc.nodes[id].parent;
    while let Some(ancestor) = parent
        && styles.values[ancestor]
            .get(Prop::DISPLAY)
            .is_some_and(|value| value == "contents")
    {
        parent = doc.nodes[ancestor].parent;
    }
    parent
}

/// The padding at the start side of a box, and whether that is the right side.
fn padding_start(styles: &ComputedStyles, parent: usize, width: f32) -> (f32, bool) {
    let style = &styles.values[parent];
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    let padding = styles.lengths[parent].edges(style, style::PADDING, width);
    (padding[if rtl { 1 } else { 3 }].max(0.), rtl)
}

fn starts_line(build: &Build<'_>, id: usize) -> bool {
    let doc = build.doc;
    doc.nodes[id].parent.is_none_or(|parent| {
        let siblings = &doc.nodes[parent].children;
        let index = build
            .breaks
            .borrow_mut()
            .entry(parent)
            .or_insert_with(|| {
                siblings
                    .iter()
                    .enumerate()
                    .map(|(index, &child)| (child, index))
                    .collect()
            })
            .get(&id)
            .copied()
            .unwrap_or(0);
        siblings[..index]
            .iter()
            .rfind(|&&child| {
                let node = &doc.nodes[child];
                node.tag != "#comment"
                    && !(node.tag == "#text" && node.text.trim_ascii().is_empty())
            })
            .is_none_or(|&child| doc.nodes[child].tag == "br")
    })
}

fn insets(styles: &ComputedStyles, node: usize, padding: bool, width: f32) -> [f32; 4] {
    let (style, units) = (&styles.values[node], &styles.lengths[node]);
    let borders = units.borders(style, width);
    if !padding {
        return borders;
    }
    let padding = units.edges(style, style::PADDING, width);
    std::array::from_fn(|side| padding[side] + borders[side])
}

fn backdrop(doc: &Document, styles: &ComputedStyles, id: usize) -> Option<(usize, u32)> {
    std::iter::successors(Some(id), |&node| doc.nodes[node].parent).find_map(|node| {
        let (ink, alpha) = style::color(styles.values[node].get(Prop::BACKGROUND_COLOR)?)?;
        (alpha >= 128).then_some((node, ink))
    })
}

const LOOK: u16 = crate::dom::HOVER | crate::dom::ACTIVE;

fn boxless(doc: &Document, id: usize) -> Option<(usize, bool)> {
    std::iter::successors(doc.nodes[id].parent, |&parent| doc.nodes[parent].parent)
        .take(2)
        .find(|&parent| doc.nodes[parent].tag == "select")
        .map(|select| {
            let attrs = &doc.nodes[select].attrs;
            (
                select,
                !attrs.contains_key("multiple")
                    && !attrs
                        .get("size")
                        .is_some_and(|size| size.trim().parse::<u32>().is_ok_and(|size| size > 1)),
            )
        })
}

fn content_editable(doc: &Document, id: usize) -> Option<usize> {
    std::iter::successors(Some(id), |&node| doc.nodes[node].parent)
        .find(|&node| doc.nodes[node].attrs.contains_key("contenteditable"))
}

fn shrinks(doc: &Document, styles: &ComputedStyles, id: usize, first: bool) -> (bool, usize) {
    let mut child = id;
    loop {
        let own = &styles.values[child];
        let inset = |prop| own.get(prop).is_some_and(|v| v != "auto");
        if float_side(own).is_some()
            || own
                .get(Prop::POSITION)
                .is_some_and(|v| matches!(v, "absolute" | "fixed"))
                && !(inset(Prop::LEFT) && inset(Prop::RIGHT))
        {
            break (true, child);
        }
        let Some(parent) = box_parent(doc, styles, child) else {
            break (false, child);
        };
        let style = &styles.values[parent];
        let display = style.get(Prop::DISPLAY).unwrap_or("block");
        if display.ends_with("flex")
            && style
                .get(Prop::FLEX_DIRECTION)
                .is_none_or(|v| v.starts_with("row"))
        {
            break (
                first && child == id
                    || own
                        .get(Prop::FLEX_GROW)
                        .is_none_or(|v| v.parse::<f32>().is_ok_and(|grow| grow == 0.)),
                parent,
            );
        }
        if !matches!(
            display,
            "block" | "list-item" | "flow-root" | "inline-block" | "flex" | "inline-flex"
        ) || style.get(Prop::WIDTH).is_some_and(|v| v != "auto")
        {
            break (false, parent);
        }
        if display == "inline-block" {
            break (true, parent);
        }
        child = parent;
    }
}

fn uses(
    doc: &Document,
    styles: &ComputedStyles,
    ids: &super::effects::Ids,
    svg: usize,
) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    let mut found = false;
    let mut pending = vec![(svg, false)];
    let mut visited = rustc_hash::FxHashSet::default();
    while let Some((node, outside)) = pending.pop() {
        if !visited.insert(node) {
            continue;
        }
        if outside {
            (
                node,
                doc.nodes.revision(node),
                Rc::as_ptr(&styles.values[node]),
            )
                .hash(&mut hasher);
        }
        let current = &doc.nodes[node];
        pending.extend(current.children.iter().map(|&child| (child, outside)));
        if current.tag == "use"
            && let Some(name) = href(current)
        {
            found = true;
            name.hash(&mut hasher);
            pending.extend(
                super::effects::index(ids, doc)
                    .get(name)
                    .into_iter()
                    .flatten()
                    .map(|&target| (target, true)),
            );
        }
    }
    found.then(|| hasher.finish())
}

fn href(node: &crate::dom::Node) -> Option<&str> {
    node.attrs
        .get("href")
        .or_else(|| node.attrs.get("xlink:href"))
        .and_then(|value| value.strip_prefix('#'))
}

fn scrolled(handle: Option<&renderer::ScrollRef>) -> Option<[bool; 2]> {
    handle.map(|handle| {
        let max = handle.extent();
        [max.x > px(0.5), max.y > px(0.5)]
    })
}

fn pictured(
    image: Option<&Arc<renderer::RenderImage>>,
    vector: Option<&(Rc<str>, [f32; 2])>,
) -> u64 {
    hashed((
        image.map(|image| {
            let size = image.size();
            [size.width, size.height]
        }),
        vector.map(|(_, natural)| natural.map(f32::to_bits)),
    ))
}

fn baselines(doc: &Document, dom: Option<&crate::dom::State>, grid: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    for child in &doc.nodes[grid].children {
        dom.and_then(|dom| dom.baselines.get(child))
            .map(|baselines| baselines.map(|value| value.map(f32::to_bits)))
            .hash(&mut hasher);
    }
    hasher.finish()
}

impl<'a> Build<'a> {
    #[inline(never)]
    fn log(&self, query: Query, scope: usize, value: u64) {
        self.reads.borrow_mut().push(Read {
            query,
            value,
            scope: scope as u16,
            reader: self.ancestors.len() as u16,
        });
    }

    fn parent(&self, id: usize, prop: Prop) -> Option<&'a str> {
        let value = parent_style(self.doc, self.styles, id, prop);
        self.log(Query::Parent(prop), self.ancestors.len(), hashed(value));
        value
    }

    fn scope_of(&self, id: usize, node: usize) -> usize {
        std::iter::successors(Some(node), |&node| self.doc.nodes[node].parent)
            .find_map(|node| {
                if node == id {
                    Some(self.ancestors.len() + 1)
                } else {
                    self.ancestors
                        .iter()
                        .rposition(|&ancestor| ancestor == node)
                        .map(|index| index + 1)
                }
            })
            .unwrap_or(0)
    }

    fn block_size(&self, dom: Option<&crate::dom::State>, block: usize) -> Option<[f32; 2]> {
        dom.and_then(|dom| dom.blocks.get(&block))
            .map(|bounds| [f32::from(bounds.size.width), f32::from(bounds.size.height)])
            .or_else(|| self.blocks.borrow().get(&block).copied())
    }

    fn replay(
        &self,
        id: usize,
        [width, _]: [f32; 2],
        container: [f32; 2],
        cell: Option<[f32; 2]>,
        dom: Option<&crate::dom::State>,
        read: &Read,
    ) -> Replay {
        let (doc, styles) = (self.doc, self.styles);
        let below = read.reader != 0;
        let now = match read.query {
            Query::Margins => {
                let mut units = styles.lengths[id];
                units.container = Some(container);
                hashed(
                    margins(self, id, &units, width, self.flex_parent, cell.is_some())
                        .map(f32::to_bits),
                )
            }
            Query::Parent(prop) => hashed(parent_style(doc, styles, id, prop)),
            Query::ParentFont => hashed(parent_font(doc, styles, id)),
            Query::FlatParent => {
                let parent = box_parent(doc, styles, id);
                hashed((
                    parent,
                    parent.and_then(|parent| styles.values[parent].get(Prop::DISPLAY)),
                ))
            }
            Query::FlatParentPadding => hashed(box_parent(doc, styles, id).map(|parent| {
                let (padding, rtl) = padding_start(styles, parent, width);
                (padding.to_bits(), rtl)
            })),
            Query::StickyParent => hashed(self.ancestors.last().map(|&parent| {
                (
                    parent,
                    insets(styles, parent, true, width).map(f32::to_bits),
                )
            })),
            Query::Break => hashed(starts_line(self, id)),
            Query::UserSelect => hashed(styles.user_select[id].0 as u8),
            Query::Positioner(fixed) => hashed(positioner(
                if fixed { "fixed" } else { "absolute" },
                self.ancestors.iter().rev().copied(),
                |ancestor| super::effects::establishes(&styles.values[ancestor]),
            )),
            Query::Backdrop(node) => hashed(backdrop(doc, styles, node as usize)),
            Query::Scroller(escape) => {
                let Some(handles) = dom.map(|dom| &dom.scroll) else {
                    return Replay::Changed;
                };
                let ancestors = self.ancestors.iter().rev().copied();
                let scroller = if below {
                    scroller(
                        doc,
                        styles,
                        std::iter::once(id).chain(ancestors),
                        handles,
                        escape,
                    )
                } else {
                    scroller(doc, styles, ancestors, handles, escape)
                };
                hashed((
                    scroller,
                    insets(styles, scroller, false, width).map(f32::to_bits),
                    handles.contains_key(&scroller),
                ))
            }
            Query::Editable => hashed(content_editable(doc, id).is_some()),
            Query::Cell => hashed(cell.map(|size| size.map(f32::to_bits))),
            Query::Shrunk => hashed(shrinks(doc, styles, id, !below).0),
            Query::Uses(svg) => hashed(uses(doc, styles, self.ids, svg as usize)),
            Query::Block(block) => hashed(
                self.block_size(dom, block as usize)
                    .map(|size| size.map(f32::to_bits)),
            ),
            Query::Scroll(node) => hashed(scrolled(
                dom.and_then(|dom| dom.scroll.get(&(node as usize))),
            )),
            Query::Width(node) => hashed(
                dom.and_then(|dom| dom.cull.widths.get(&(node as usize)))
                    .map(|size| size.map(f32::to_bits)),
            ),
            Query::Track(node, axis) => {
                // The 0.5 px must match prepaint, which draws again only for a size more than half a pixel
                // off.
                let used = f32::from_bits(read.value as u32);
                return match dom
                    .and_then(|dom| dom.cull.widths.get(&(node as usize)))
                    .map(|size| size[usize::from(axis)])
                {
                    // A node keeps its size, so a read that finds none was made without one too.
                    None => Replay::Same,
                    Some(size) if size.to_bits() == used.to_bits() => Replay::Same,
                    Some(size) if (size - used).abs() <= 0.5 => Replay::Close,
                    Some(_) => Replay::Changed,
                };
            }
            Query::Baselines(grid) => baselines(doc, dom, grid as usize),
            Query::Atom(node) => hashed(
                dom.and_then(|dom| dom.atoms.get(&(node as usize)))
                    .map(|size| size.map(f32::to_bits)),
            ),
            Query::Image(node) => {
                let node = node as usize;
                let dom = dom.filter(|_| !broken_image(&doc.nodes[node]));
                pictured(
                    dom.and_then(|dom| dom.images.get(&node)),
                    dom.and_then(|dom| dom.svg_images.get(&node)),
                )
            }
            Query::Look(node) => hashed(doc.nodes[node as usize].state & LOOK),
            Query::Boxless(node) => hashed(boxless(doc, node as usize).map(|select| select.1)),
            Query::Spliced(node) => hashed(style::spliced(doc, styles, node as usize)),
            Query::RowFill => {
                hashed(row_fill(doc, styles, id).map(|fill| fill.to_array().map(f32::to_bits)))
            }
            Query::Collapse => hashed(collapses(doc, styles, id)),
            Query::Value(node) => hashed(&doc.nodes[node as usize].value),
            Query::Viewport => hashed(styles.lengths[id].viewport.map(f32::to_bits)),
            Query::Theme => hashed(crate::native::theme_state() & 1),
            Query::LineBase(node) => hashed(definite_line(doc, styles, node as usize)),
            Query::Shrinks(node) => hashed(shrinks_around(doc, styles, node as usize)),
        };
        if now == read.value {
            Replay::Same
        } else {
            Replay::Changed
        }
    }
}

// The axes, 1 for x and 2 for y, that the insets of an out-of-flow box set.
fn inset_axes(style: &style::Style) -> u8 {
    [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]]
        .iter()
        .enumerate()
        .filter(|(_, axis)| {
            axis.iter()
                .any(|&prop| style.get(prop).is_some_and(|value| value != "auto"))
        })
        .fold(0, |axes, (axis, _)| axes | 1 << axis)
}

// An empty box at the static place of an absolute box, which puts its place in `anchors` for the box to follow.
fn static_mark(view: &Rc<super::PageState>, key: usize) -> renderer::Node {
    let view = view.clone();
    renderer::canvas_watching(
        move |bounds, _| {
            let mut host = view.host.borrow_mut();
            host.dom
                .entry(view.id)
                .or_default()
                .anchors
                .insert(key, bounds);
        },
        |_, _, _| {},
    )
    .w(px(0.))
    .h(px(0.))
}

fn positioner(
    position: &str,
    mut ancestors: impl Iterator<Item = usize>,
    mut made: impl FnMut(usize) -> super::effects::Establishes,
) -> Option<usize> {
    let fixed = match position {
        "absolute" => false,
        "fixed" => true,
        _ => return None,
    };
    ancestors
        .find(|&ancestor| {
            let made = made(ancestor);
            if fixed { made.fixed } else { made.absolute }
        })
        .or((!fixed).then_some(0))
}

// Find the scroll container by style, not by handle, since scripts make handles for any box. `escape` is the state
// of the walk when it reaches the first ancestor, as `escape_step` gives it.
fn scroller(
    doc: &Document,
    styles: &ComputedStyles,
    mut ancestors: impl Iterator<Item = usize>,
    handles: &rustc_hash::FxHashMap<usize, renderer::ScrollRef>,
    mut escape: u8,
) -> usize {
    ancestors
        .find(|&ancestor| {
            let Some(next) = escape_step(styles, ancestor, escape) else {
                return false;
            };
            escape = next;
            ancestor != 0
                && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                    .into_iter()
                    .any(|prop| {
                        styles.values[ancestor]
                            .get(prop)
                            .is_some_and(|value| matches!(value, "auto" | "scroll" | "hidden"))
                    })
                && (!matches!(doc.nodes[ancestor].tag.as_str(), "html" | "body")
                    || handles.contains_key(&ancestor))
        })
        .unwrap_or(0)
}

// Past an absolute (1) or fixed (2) box, only its containing block chain scrolls it. Returns `None` when the walk
// skips `node`, else the state after it.
fn escape_step(styles: &ComputedStyles, node: usize, escape: u8) -> Option<u8> {
    let style = &styles.values[node];
    if escape != 0 {
        let made = super::effects::establishes(style);
        if !(if escape == 2 {
            made.fixed
        } else {
            made.absolute
        }) {
            return None;
        }
    }
    Some(match style.get(Prop::POSITION) {
        Some("absolute") => 1,
        Some("fixed") => 2,
        _ => 0,
    })
}

// A read as the parent of `id` holds it: the scroller walk of a box inside `id` goes through `id`.
fn passed(styles: &ComputedStyles, id: usize, query: Query, below: bool) -> Query {
    match query {
        Query::Scroller(escape) if below => {
            Query::Scroller(escape_step(styles, id, escape).unwrap_or(escape))
        }
        query => query,
    }
}

// The place of each list item among the list items of its parent.
type Siblings = rustc_hash::FxHashMap<usize, usize>;

const SVG_SOURCE: [Prop; 13] = [
    Prop::TRANSLATE,
    Prop::ROTATE,
    Prop::SCALE,
    Prop::TRANSFORM_BOX,
    Prop::CX,
    Prop::CY,
    Prop::R,
    Prop::RX,
    Prop::RY,
    Prop::X,
    Prop::Y,
    Prop::WIDTH,
    Prop::HEIGHT,
];

// `text-indent` as pixels and a fraction of the width of the paragraph, which the renderer resolves at layout.
fn text_indent(style: &style::Style, units: &style::Lengths) -> (f32, f32) {
    let Some(value) = style.get(Prop::TEXT_INDENT) else {
        return (0., 0.);
    };
    let pixels = units.resolve(value, 0.).unwrap_or(0.);
    match value.contains('%') {
        true => (
            pixels,
            (units.resolve(value, 100.).unwrap_or(pixels) - pixels) / 100.,
        ),
        false => (pixels, 0.),
    }
}

struct SvgSource<'a> {
    doc: &'a Document,
    styles: &'a ComputedStyles,
    root: usize,
    scale: f32,
    view: [f32; 2],
    header: String,
    ids: &'a super::effects::Ids,
}

impl SvgSource<'_> {
    fn attrs(&self, element: usize, paint: bool, transform: bool) -> Vec<(&'static str, String)> {
        let (doc, root) = (self.doc, self.root);
        let own = &self.styles.values[element];
        let units = &self.styles.lengths[element];
        let tag = doc.nodes[element].tag.as_str();
        let mut attrs = Vec::new();
        if element != root {
            for prop in &SVG_SOURCE[4..] {
                let applies = match *prop {
                    Prop::CX | Prop::CY => matches!(tag, "circle" | "ellipse"),
                    Prop::R => tag == "circle",
                    Prop::RX | Prop::RY => matches!(tag, "ellipse" | "rect"),
                    _ => matches!(tag, "rect" | "image" | "foreignObject"),
                };
                if let Some(value) = own.get(*prop).filter(|value| applies && *value != "auto") {
                    let value = if value.ends_with('%') || value.parse::<f32>().is_ok() {
                        value.to_owned()
                    } else {
                        units
                            .resolve(value, f32::NAN)
                            .map_or_else(|| value.to_owned(), |length| length.to_string())
                    };
                    attrs.push((prop.name(), value));
                }
            }
        }
        // resvg doesn't take CSS transforms, `transform-box` or `transform-origin`, so they become a matrix.
        if transform
            && element != root
            && super::TRANSFORM_PROPS
                .iter()
                .any(|&prop| own.get(prop).is_some())
        {
            let fill = own
                .get(Prop::TRANSFORM_BOX)
                .is_some_and(|value| value != "view-box");
            let [x, y, w, h] = fill
                .then(|| self.shape(element))
                .flatten()
                .map_or([0., 0., self.view[0], self.view[1]], |(rect, _)| rect);
            let bounds = Rect::new(point(px(x), px(y)), size(px(w), px(h)));
            attrs.push((
                "transform",
                super::effects::transform(own, units, bounds, None).map_or(String::new(), |m| {
                    format!(
                        "matrix({} {} {} {} {} {})",
                        m[0], m[1], m[4], m[5], m[12], m[13]
                    )
                }),
            ));
        }
        if !paint {
            return attrs;
        }
        let parent = doc.nodes[element]
            .parent
            .map(|parent| &self.styles.values[parent]);
        // resvg ignores `vector-effect: non-scaling-stroke` and `pathLength`, so stroke widths and dashes are
        // scaled here.
        let width = if element != root
            && self.scale != 1.
            && own.get(Prop::VECTOR_EFFECT) == Some("non-scaling-stroke")
        {
            1. / self.scale
        } else {
            1.
        };
        let dashed = own
            .get(Prop::STROKE_DASHARRAY)
            .is_some_and(|value| value != "none")
            || own
                .get(Prop::STROKE_DASHOFFSET)
                .is_some_and(|value| value.trim_end_matches("px").parse::<f32>() != Ok(0.));
        let dashes = width
            * doc.nodes[element]
                .attrs
                .get("pathLength")
                .and_then(|value| value.trim().parse::<f32>().ok())
                .filter(|length| *length > 0. && dashed && element != root)
                .and_then(|length| Some(self.shape(element)?.1 / length))
                .filter(|ratio| *ratio > 0.)
                .unwrap_or(1.);
        let mut result = String::new();
        for prop in style::SVG {
            // A style holds the lines of a text decoration, and SVG reads the short form. The lines include the
            // ones of ancestors, which don't reach into an image, so only a change from the parent counts.
            let value = own.get(prop).or_else(|| {
                let outer = parent.and_then(|parent| parent.get(Prop::TEXT_DECORATION_LINE));
                own.get(Prop::TEXT_DECORATION_LINE)
                    .filter(|line| prop == Prop::TEXT_DECORATION && outer != Some(*line))
            });
            let Some(value) = value else { continue };
            let factor = match prop {
                Prop::TRANSFORM | Prop::TRANSFORM_ORIGIN => continue,
                Prop::STROKE_WIDTH => width,
                Prop::STROKE_DASHARRAY | Prop::STROKE_DASHOFFSET => dashes,
                _ => 1.,
            };
            if if element == root {
                !prop.inherited()
            } else {
                prop.inherited()
                    && factor == 1.
                    && !doc.nodes[element].attrs.contains_key(prop.name())
                    && parent.and_then(|parent| parent.get(prop)) == Some(value)
            } {
                continue;
            }
            let value = match prop {
                Prop::DISPLAY if value != "none" => continue,
                Prop::CLIP_PATH | Prop::MASK | Prop::FILTER if !value.starts_with("url(") => {
                    continue;
                }
                Prop::VECTOR_EFFECT => continue,
                Prop::FONT_SIZE => format!("{}px", units.font.em),
                // Resolve currentColor from the <use> element's inherited color, not the referenced
                // definition.
                Prop::FILL
                | Prop::STROKE
                | Prop::STOP_COLOR
                | Prop::FLOOD_COLOR
                | Prop::LIGHTING_COLOR
                    if value.eq_ignore_ascii_case("currentcolor") =>
                {
                    "currentColor".to_owned()
                }
                Prop::FILL
                | Prop::STROKE
                | Prop::COLOR
                | Prop::STOP_COLOR
                | Prop::FLOOD_COLOR
                | Prop::LIGHTING_COLOR => style::color(value)
                    .map_or(value.to_owned(), |(rgb, alpha)| {
                        format!("#{rgb:06x}{alpha:02x}")
                    }),
                _ if factor != 1. => value
                    .split(|c: char| c == ',' || c.is_ascii_whitespace())
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        part.strip_suffix("px")
                            .unwrap_or(part)
                            .parse::<f32>()
                            .map_or(part.to_owned(), |length| (length * factor).to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => value.to_owned(),
            };
            result.push_str(prop.name());
            result.push(':');
            result.push_str(&value);
            result.push(';');
        }
        if !result.is_empty() {
            attrs.push(("style", result));
        }
        attrs
    }

    fn shape(&self, element: usize) -> Option<([f32; 4], f32)> {
        let attrs = |node| {
            let mut attrs = self.attrs(node, false, node != element);
            // resvg drops shapes without paint.
            attrs.push(("fill", "#000".into()));
            if node == element {
                attrs.extend([("id", "apps-shape".into()), ("transform", String::new())]);
                if self.doc.nodes[node].tag == "use" {
                    attrs.extend([("x", String::new()), ("y", String::new())]);
                }
            }
            attrs
        };
        let (mut targets, mut pending) = (Vec::new(), vec![element]);
        while let Some(node) = pending.pop() {
            let node = &self.doc.nodes[node];
            if node.tag == "use"
                && let Some(target) = href(node).and_then(|name| {
                    super::effects::index(self.ids, self.doc)
                        .get(name)?
                        .first()
                        .copied()
                })
                && !targets.contains(&target)
            {
                targets.push(target);
                pending.push(target);
            }
            pending.extend(&node.children);
        }
        let mut source = format!("{}><defs>", self.header);
        for target in targets {
            source.push_str(&self.doc.serialize_svg(target, false, &attrs));
        }
        source.push_str("</defs>");
        source.push_str(&self.doc.serialize_svg(element, false, &attrs));
        source.push_str("</svg>");
        super::canvas::svg_shape(&source)
    }
}
pub(super) type Svg = (Vec<Rc<style::Style>>, ([f32; 2], u64));

type Used = Rc<rustc_hash::FxHashMap<usize, [f32; 2]>>;
type Sets = ([f32; 2], [f32; 2], bool);
pub(super) type Adjoined = ((u64, u32, Option<[u32; 2]>), Sets);
pub(super) type Walked = ((u64, u32, [bool; 2], Option<[u32; 2]>), Used);

thread_local! {
    // Reserve a quarter of the stack for layout and paint, which recurse as deeply as build.
    static STACK_LIMIT: usize = {
        let marker = 0u8;
        let here = &raw const marker as usize;
        #[cfg(target_os = "macos")]
        let (low, size) = unsafe {
            let size = libc::pthread_get_stacksize_np(libc::pthread_self());
            (libc::pthread_get_stackaddr_np(libc::pthread_self()) as usize - size, size)
        };
        #[cfg(target_os = "linux")]
        let (low, size) = unsafe {
            let mut attr = std::mem::zeroed();
            let (mut low, mut size) = (std::ptr::null_mut(), 0);
            if libc::pthread_getattr_np(libc::pthread_self(), &mut attr) == 0 {
                libc::pthread_attr_getstack(&attr, &mut low, &mut size);
                libc::pthread_attr_destroy(&mut attr);
            }
            (low as usize, size)
        };
        // Assumes the 64 MB main stack that build.rs links on Windows.
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let (low, size) = (here.saturating_sub(60 << 20), 64 << 20);
        (low + size / 4).min(here)
    };
}

pub(super) fn element(
    build: &Build<'_>,
    id: usize,
    containing: [f32; 2],
    container: [f32; 2],
    slot: Option<([f32; 4], super::grid::Grid)>,
) -> Child {
    let info = build.info[id];
    let depth = build.ancestors.len();
    let mark = build.reads.borrow().len();
    #[cfg(debug_assertions)]
    let printed = build.prints.borrow().len();
    let text = build.doc.nodes[id].tag.starts_with('#');
    let storable = !text
        && build.perspective.is_none()
        && depth < 256
        && info.flags
            & (cull::ANIMATED
                | cull::HIDDEN
                | cull::MOVES_CONTENT
                | cull::COMPOSITED
                | cull::TRANSFORMED)
            == 0;
    let call = Call {
        subtree: info.subtree,
        containing: if info.flags & cull::READS_HEIGHT != 0 {
            containing
        } else {
            [containing[0], 0.]
        },
        container,
        flex_parent: build.flex_parent,
        columned: build.columned,
        cell: slot.as_ref().map(|(_, grid)| {
            use std::hash::{Hash, Hasher};
            let mut tracks = rustc_hash::FxHasher::default();
            for value in grid.columns.iter().chain(&grid.rows).chain(&grid.gap) {
                value.to_bits().hash(&mut tracks);
            }
            (grid.baseline, grid.first_item.is_some(), tracks.finish())
        }),
    };
    // Debug checks must not remove repaint entries, or the real build can skip the box.
    #[cfg(debug_assertions)]
    let repainted = if build.checking {
        build.view.repaint.borrow().contains(&id)
    } else {
        build.view.repaint.borrow_mut().remove(&id)
    };
    #[cfg(not(debug_assertions))]
    let repainted = build.view.repaint.borrow_mut().remove(&id);
    let keep = 'keep: {
        if !storable
            || repainted
            || build.resized && info.flags & cull::READS_VIEWPORT != 0
            || !build.settled && info.flags & cull::READS_LAST_FRAME != 0
            || !build.tree.contains(id)
        {
            break 'keep None;
        }
        let kept = build.view.kept.borrow();
        let Some(record) = kept.get(&id) else {
            break 'keep None;
        };
        let cell = slot.as_ref().map(|(bounds, _)| [bounds[2], bounds[3]]);
        // A float is never culled, so it stays kept far from the view.
        if !record.culled
            && float_side(&build.styles.values[id]).is_none()
            && let Some(zone) = &build.zone
        {
            let margin = build.styles.lengths[id].viewport[1];
            let reach = build.view.reach.borrow().get(id).copied();
            if !reach.is_some_and(|[bottom, top]| {
                bottom >= zone.top - margin && top <= zone.bottom + margin
            }) {
                break 'keep None;
            }
        }
        let mut close = false;
        // A debug check of a kept box keeps the positioned boxes in it that build in place after the build.
        #[cfg(debug_assertions)]
        let inside = build.checking
            && info.flags & cull::OUT_OF_FLOW != 0
            && build.view.outs.borrow().contains_key(&id)
            && Call {
                subtree: record.call.subtree,
                cell: call.cell,
                ..call
            } == record.call;
        #[cfg(not(debug_assertions))]
        let inside = false;
        let held = (record.call == call || inside) && {
            let fresh = record.epoch.get() == build.epoch;
            let host = build.view.host.borrow();
            let dom = host.dom.get(&build.view.id);
            record.reads.iter().all(|read| {
                fresh && !read.query.host()
                    || match build.replay(id, containing, container, cell, dom, read) {
                        Replay::Same => true,
                        Replay::Close => {
                            close = true;
                            true
                        }
                        Replay::Changed => false,
                    }
            })
        };
        #[cfg(debug_assertions)]
        assert!(
            held || !build.checking,
            "box {id} <{}> builds again inside a kept box: {}",
            build.doc.nodes[id].tag,
            if record.call == call {
                let host = build.view.host.borrow();
                let dom = host.dom.get(&build.view.id);
                format!(
                    "{:?} changed",
                    record
                        .reads
                        .iter()
                        .filter(
                            |read| build.replay(id, containing, container, cell, dom, read)
                                == Replay::Changed
                        )
                        .map(|read| read.query)
                        .collect::<Vec<_>>()
                )
            } else {
                "its call changed".into()
            }
        );
        if !held {
            break 'keep None;
        }
        record.epoch.set(build.epoch);
        record.containing.set(containing);
        if record.culled {
            build.culls.set(build.culls.get() + 1);
        }
        build
            .reads
            .borrow_mut()
            .extend(record.reads.iter().map(|read| Read {
                scope: if read.scope == 0 {
                    0
                } else {
                    (depth + 1).saturating_sub(usize::from(read.scope)).max(1) as u16
                },
                reader: depth as u16 + read.reader,
                query: passed(build.styles, id, read.query, read.reader != 0),
                ..*read
            }));
        #[cfg(debug_assertions)]
        build.prints.borrow_mut().push(match inside {
            true => record.seen.get(),
            false => {
                if !build.checking {
                    record.seen.set(record.print);
                }
                record.print
            }
        });
        // A record with culled boxes can come from a debug build in a zone, which a check without a zone can't
        // repeat. The zone checks of `plan` cover such boxes.
        Some(close || inside || record.culled)
    };
    if let Some(_close) = keep {
        #[cfg(debug_assertions)]
        if !_close && !build.view.around.borrow().contains(&id) {
            check(build, id, containing, container, slot, mark, printed);
        }
        return Child::Kept(id.into());
    }
    let culls = build.culls.get();
    #[cfg(debug_assertions)]
    let checking = build.checking;
    #[cfg(not(debug_assertions))]
    let checking = false;
    if !checking && !build.view.repaint.borrow().is_empty() {
        build.view.built.borrow_mut().insert(id);
    }
    let field = text_field(&build.doc.nodes[id], &build.styles.values[id]);
    let positioned = storable && info.flags & cull::OUT_OF_FLOW != 0;
    let patch = (field || positioned).then(|| Patch {
        ancestors: build.ancestors.clone(),
        containing,
        container,
        slot: slot.clone(),
        flex_parent: build.flex_parent,
        columned: build.columned,
        perspective: build.perspective.clone(),
        value: hashed(&build.doc.nodes[id].value),
    });
    let cell = slot.clone().filter(|_| storable);
    let child = create(build, id, containing, container, slot, None);
    #[cfg(debug_assertions)]
    let plan = build.plan.take();
    #[cfg(debug_assertions)]
    if !build.checking
        && *TRACE
        && let Some(name) = build.doc.nodes[id].attrs.get("id")
    {
        eprintln!("BUILT {name}");
    }
    let reads = {
        let below = |read: &Read| usize::from(read.reader) > depth;
        let mut log = build.reads.borrow_mut();
        let mut end = mark;
        for at in mark..log.len() {
            let read = log[at];
            if usize::from(read.scope) <= depth
                && !log[end.saturating_sub(16).max(mark)..end]
                    .iter()
                    .any(|old| old.query == read.query && below(old) == below(&read))
            {
                log[end] = read;
                end += 1;
            }
        }
        log.truncate(end);
        let reads = storable.then(|| {
            log[mark..]
                .iter()
                .map(|read| Read {
                    scope: if read.scope == 0 {
                        0
                    } else {
                        (depth + 1 - usize::from(read.scope)) as u16
                    },
                    reader: u16::from(below(read)),
                    ..*read
                })
                .collect()
        });
        for read in &mut log[mark..] {
            read.query = passed(build.styles, id, read.query, below(read));
        }
        reads
    };
    #[cfg(debug_assertions)]
    let print = print(build, printed, &child);
    if let Some(patch) = patch.filter(|_| !checking) {
        match field {
            true => build.view.patches.borrow_mut().insert(id, patch),
            false => build.view.outs.borrow_mut().insert(id, patch),
        };
    }
    if !text {
        let mut kept = build.view.kept.borrow_mut();
        match reads {
            Some(reads) => {
                kept.insert(
                    id,
                    Kept {
                        call,
                        reads,
                        ancestors: build.ancestors.clone(),
                        epoch: build.epoch.into(),
                        containing: containing.into(),
                        culled: build.culls.get() != culls,
                        slot: cell,
                        #[cfg(debug_assertions)]
                        print,
                        #[cfg(debug_assertions)]
                        seen: print.into(),
                        #[cfg(debug_assertions)]
                        plan,
                    },
                );
            }
            None => {
                kept.remove(&id);
            }
        }
    }
    child
}

pub(super) fn repaint(build: &Build<'_>, id: usize) -> Option<renderer::Node> {
    let old = build.view.kept.borrow_mut().remove(&id)?;
    let (ancestors, containing, container, flex_parent) = (
        old.ancestors.clone(),
        old.containing.get(),
        old.call.container,
        old.call.flex_parent,
    );
    let mark = build.reads.borrow().len();
    let place = Build {
        ancestors: ancestors.clone(),
        flex_parent,
        columned: old.call.columned,
        perspective: None,
        zone: None,
        #[cfg(debug_assertions)]
        extra: Default::default(),
        #[cfg(debug_assertions)]
        plan: Default::default(),
        ..*build
    };
    #[cfg(debug_assertions)]
    let printed = place.prints.borrow().len();
    let bounds = old.slot.as_ref().map(|(bounds, _)| *bounds);
    let child = element(&place, id, containing, container, old.slot.clone());
    // A grid item takes its place in its cell, as its parent would give it.
    let child = match bounds {
        Some(bounds) => {
            let mut units = build.styles.lengths[id];
            units.container = Some(container);
            let margin = units.edges(&build.styles.values[id], style::MARGIN, containing[0]);
            self::place(child, margin, build.pixel_ratio, bounds)
        }
        None => child,
    };
    #[cfg(debug_assertions)]
    place.prints.borrow_mut().truncate(printed);
    build.reads.borrow_mut().truncate(mark);
    let mut kept = build.view.kept.borrow_mut();
    if kept.get(&id).is_none_or(|record| record.reads != old.reads) {
        for ancestor in ancestors.iter() {
            kept.remove(ancestor);
        }
    }
    match child {
        Child::Node(node) => Some(*node),
        Child::Kept(_) | Child::Placed(..) => None,
    }
}

#[cfg(debug_assertions)]
pub(super) fn same_in_place(fresh: &Child, placed: &[renderer::Node], tree: &renderer::Tree) {
    fn find(child: &Child, id: renderer::NodeKey) -> Option<&renderer::Node> {
        let Child::Node(node) = child else {
            return None;
        };
        if node.element_id() == Some(id) {
            return Some(node);
        }
        node.kids().iter().find_map(|kid| find(kid, id))
    }
    fn same(fresh: &renderer::Node, placed: &renderer::Node, tree: &renderer::Tree) {
        let id = fresh.element_id();
        assert!(
            fresh.style_ref() == placed.style_ref(),
            "box {id:?} built in place has another style than a full build gives"
        );
        assert!(
            fresh.text_content() == placed.text_content(),
            "box {id:?} built in place has other text than a full build gives"
        );
        assert_eq!(
            fresh.kids().len(),
            placed.kids().len(),
            "box {id:?} built in place has other children than a full build gives"
        );
        for (fresh, placed) in fresh.kids().iter().zip(placed.kids()) {
            match (fresh, placed) {
                (Child::Node(fresh), Child::Node(placed)) => same(fresh, placed, tree),
                (Child::Node(fresh), Child::Kept(kept) | Child::Placed(kept, _)) => assert!(
                    tree.style_of(*kept) == Some(fresh.style_ref()),
                    "box {kept:?} kept in a box built in place has another style than a full build gives"
                ),
                _ => {}
            }
        }
    }
    for node in placed {
        if let Some(id) = node.element_id()
            && let Some(fresh) = find(fresh, id)
        {
            same(fresh, node, tree);
        }
    }
}

#[cfg(debug_assertions)]
static TRACE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var_os("APPS_TRACE_BUILDS").is_some());

#[cfg(debug_assertions)]
fn check(
    build: &Build<'_>,
    id: usize,
    containing: [f32; 2],
    container: [f32; 2],
    slot: Option<([f32; 4], super::grid::Grid)>,
    mark: usize,
    printed: usize,
) {
    let culls = std::cell::Cell::new(0);
    let shadow = Build {
        ancestors: build.ancestors.clone(),
        perspective: build.perspective.clone(),
        zone: None,
        culls: &culls,
        checking: true,
        extra: Default::default(),
        plan: Default::default(),
        ..*build
    };
    let start = build.reads.borrow().len();
    let fresh = create(&shadow, id, containing, container, slot, None);
    let depth = build.ancestors.len();
    let set = |reads: &[Read]| {
        let mut set: Vec<_> = reads
            .iter()
            .filter(|read| usize::from(read.scope) <= depth)
            .map(|read| (read.query, usize::from(read.reader) > depth, read.value))
            .collect();
        set.sort_unstable_by_key(|&(query, below, _)| (query, below));
        set.dedup_by_key(|&mut (query, below, _)| (query, below));
        set
    };
    let mut log = build.reads.borrow_mut();
    // A kept box passes its reads on as its parent holds them.
    for read in &mut log[start..] {
        read.query = passed(
            build.styles,
            id,
            read.query,
            usize::from(read.reader) > depth,
        );
    }
    let (kept, now) = (set(&log[mark..start]), set(&log[start..]));
    log.truncate(start);
    drop(log);
    let tag = &build.doc.nodes[id].tag;
    assert!(
        kept == now,
        "kept box {id} <{tag}> reads otherwise when built again: {:?} instead of {:?}",
        now.iter().map(|read| read.0).collect::<Vec<_>>(),
        kept.iter().map(|read| read.0).collect::<Vec<_>>()
    );
    let print = print(&shadow, printed + 1, &fresh);
    let mut prints = build.prints.borrow_mut();
    assert_eq!(
        prints[printed], print,
        "kept box {id} <{tag}> builds otherwise when built again"
    );
    prints.truncate(printed + 1);
}

#[cfg(debug_assertions)]
fn print(build: &Build<'_>, from: usize, child: &Child) -> u64 {
    use std::hash::Hasher;
    struct Hashed(rustc_hash::FxHasher);
    impl std::fmt::Write for Hashed {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0.write(text.as_bytes());
            Ok(())
        }
    }
    let mut hasher = Hashed(Default::default());
    if let Child::Node(node) = child {
        let _ = std::fmt::Write::write_fmt(&mut hasher, format_args!("{:?}", node.style_ref()));
    }
    hasher.0.write_u64(build.extra.take());
    let mut prints = build.prints.borrow_mut();
    for print in prints.drain(from..) {
        hasher.0.write_u64(print);
    }
    let print = hasher.0.finish();
    prints.push(print);
    print
}

/// Builds the box of `id`, or with `pseudo` only that block pseudo-element of a spliced inline box.
fn create(
    build: &Build<'_>,
    id: usize,
    containing: [f32; 2],
    container: [f32; 2],
    slot: Option<([f32; 4], super::grid::Grid)>,
    pseudo: Option<usize>,
) -> Child {
    let reader = build;
    let Build {
        blocks,
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        ref ancestors,
        flex_parent,
        measured,
        perspective: ref parent_perspective,
        siblings,
        ids,
        info,
        ref zone,
        columned,
        top_layer,
        culls,
        reads,
        ..
    } = *build;
    let depth = ancestors.len();
    let mark = reads.borrow().len();
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    let display =
        style
            .get(Prop::DISPLAY)
            .unwrap_or(if node.tag.starts_with('#') && node.tag != "#text" {
                "block"
            } else {
                "inline"
            });
    // Like Chrome, an inline broken image with alt text ignores its size, also when only the UA sheet made it
    // an inline block.
    let broken = broken_image(node);
    let alt = broken
        .then(|| node.attrs.get("alt").filter(|alt| !alt.is_empty()))
        .flatten();
    let sizeless = alt.is_some()
        && (display == "inline"
            || display == "inline-block" && styles.flags[id] & style::AGENT_DISPLAY != 0);
    let display = if sizeless {
        "inline-block"
    } else if display == "inline" && style::holds_block_box(doc, styles, id) {
        "block"
    } else {
        display
    };
    let mut units = styles.lengths[id];
    units.container = Some(container);
    let parsed = view.plans.borrow_mut().get(style, &units, pixel_ratio);
    let order = parsed.order;
    let positioned = style
        .get(Prop::POSITION)
        .is_some_and(|value| value != "static");
    let layered = order.is_some()
        && (positioned
            || flex_parent
            || matches!(
                reader.parent(id, Prop::DISPLAY),
                Some("grid" | "inline-grid")
            ));
    // Keep one stacking context and layer throughout opacity animations so frames only patch opacity.
    let composited = info[id].flags & cull::COMPOSITED != 0;
    let made = parsed.made;
    // Every layer needs a stacking context so z-indexed descendants cannot escape its transform, opacity or
    // clip.
    let stacking = id == 0 || composited || made.stacking || layered;
    let chain: Rc<[usize]> = ancestors.iter().copied().chain([id]).collect();
    // A column width can give columns for a wider content box than the containing one, so it always counts here.
    let multicol = columns(style, &units, containing[0]).is_some()
        || style
            .get(Prop::COLUMN_WIDTH)
            .is_some_and(|value| value != "auto");
    let child_build = Build {
        columned: columned || multicol,
        #[cfg(debug_assertions)]
        extra: Default::default(),
        #[cfg(debug_assertions)]
        plan: Default::default(),
        // Columns move boxes away from their flow, and a spacer for a box could not break between columns.
        zone: zone
            .as_ref()
            .filter(|_| info[id].flags & (cull::CHANGED | cull::MOVES_CONTENT) == 0)
            .filter(|_| !multicol)
            // The contents of a box that skips them at times keep no spacers: their records could outlive a skip.
            .filter(|_| {
                info[id].flags & cull::SKIPS == 0
                    || style
                        .get(Prop::CONTENT_VISIBILITY)
                        .is_none_or(|value| value == "visible")
            })
            .and_then(|zone| {
                if style
                    .get(Prop::OVERFLOW_Y)
                    .is_none_or(|value| value == "visible")
                {
                    return Some(zone.clone());
                }
                let host = view.host.borrow();
                let dom = host.dom.get(&view.id)?;
                let rect = *dom.bounds.get(&id)?;
                let moved = dom.cull.scrolled.get(&id).copied().unwrap_or(0.);
                Some(Rc::new(cull::Zone {
                    top: zone.top.max(rect[1]) + moved,
                    bottom: zone.bottom.min(rect[1] + rect[3]) + moved,
                    clips: zone.clips.iter().copied().chain([id]).collect(),
                    resized: zone.resized,
                }))
            }),
        ancestors: chain.clone(),
        flex_parent: matches!(display, "flex" | "inline-flex"),
        perspective: style
            .get(Prop::PERSPECTIVE)
            .filter(|value| *value != "none")
            .and_then(|value| styles.lengths[id].resolve(value, 0.))
            .map(|distance| {
                (
                    distance,
                    style
                        .get(Prop::PERSPECTIVE_ORIGIN)
                        .map(str::to_owned)
                        .unwrap_or_else(|| "50% 50%".into()),
                )
            }),
        ..*reader
    };
    let build = &child_build;
    let position = style.get(Prop::POSITION).unwrap_or("static");
    let parent = box_parent(doc, styles, id);
    let positioner = positioner(position, ancestors.iter().rev().copied(), |ancestor| {
        super::effects::establishes(&styles.values[ancestor])
    });
    if matches!(position, "absolute" | "fixed") {
        let scope = positioner
            .and_then(|block| ancestors.iter().rposition(|&ancestor| ancestor == block))
            .map_or(1, |index| index + 1);
        reader.log(
            Query::Positioner(position == "fixed"),
            scope,
            hashed(positioner),
        );
        reader.log(
            Query::FlatParent,
            depth,
            hashed((
                parent,
                parent.and_then(|parent| styles.values[parent].get(Prop::DISPLAY)),
            )),
        );
    }
    let positioner = positioner.filter(|_| id != 0);
    // An inline box can be a part of the text of its block, so its out-of-flow boxes follow the containing block it
    // gives them even when it is their parent. When it has a box of its own, that is the same place.
    let parent_box = parent.filter(|&parent| {
        !matches!(position, "absolute" | "fixed")
            || doc.nodes[parent].tag.starts_with('#')
            || styles.values[parent].get(Prop::DISPLAY).unwrap_or("inline") != "inline"
    });
    let viewport = || {
        reader.log(Query::Viewport, 0, hashed(units.viewport.map(f32::to_bits)));
        Some(units.viewport)
    };
    let block_size = if position == "fixed" && positioner.is_none() {
        viewport()
    } else {
        positioner
            .filter(|&block| Some(block) != parent_box)
            .and_then(|block| {
                if block == 0 {
                    return viewport();
                }
                // Prefer the last layout to the estimate: missed min/max constraints can cause endless
                // redraws.
                let size = reader.block_size(view.host.borrow().dom.get(&view.id), block);
                reader.log(
                    Query::Block(block as u32),
                    0,
                    hashed(size.map(|size| size.map(f32::to_bits))),
                );
                size
            })
    };
    let containing = block_size.unwrap_or(containing);
    // Element building and layout recurse per level, so deep pages must not overflow the stack.
    if chain.len() > 256 || style.get(Prop::DISPLAY).is_some_and(|s| s == "none") {
        return renderer::node().hidden().into();
    }
    let opacity = parsed.opacity;
    // Use element opacity at zero. A hidden layer still costs two render passes.
    // A leaf fades each of its paints alone, which is right only when they don't overlap. Paints outside the border
    // box (outer shadows, an outline) can overlap only each other, and paints inside it only each other.
    let overlapping = || {
        let shadows = |inset| {
            style.get(Prop::BOX_SHADOW).map_or(0, |value| {
                (units.shadows(value, inset).iter())
                    .filter(|shadow| !shadow.color.is_transparent())
                    .count()
            })
        };
        let images = style
            .get(Prop::BACKGROUND_IMAGE)
            .filter(|value| *value != "none")
            .map_or(0, |value| style::components(value, ',').len());
        let border = (units.borders(style, 0.).into_iter())
            .zip(parsed.border_colors)
            .any(|(width, color)| width > 0. && color.a > 0.);
        let outline = style.get(Prop::OUTLINE_STYLE).and_then(|kind| {
            let offset = style
                .get(Prop::OUTLINE_OFFSET)
                .and_then(|v| units.resolve(v, containing[0]))
                .unwrap_or(0.);
            match kind {
                "none" | "hidden" => None,
                // A focus ring moves 1px into a border.
                "auto" => Some(offset < 0. || offset < 1. && border),
                _ => Some(offset < 0.),
            }
        });
        let content = cull::replaced(&node.tag)
            || matches!(
                node.tag.as_str(),
                "img" | "input" | "textarea" | "select" | "progress" | "meter" | "terminal"
            )
            || styles.pseudos.get(&id).is_some_and(|pseudos| {
                pseudos.iter().any(|pseudo| {
                    pseudo
                        .get(Prop::CONTENT)
                        .is_some_and(|value| !matches!(value, "none" | "normal"))
                        && pseudo.get(Prop::DISPLAY) != Some("none")
                })
            });
        let inside = shadows(true)
            + images
            + [
                parsed.background.is_some(),
                border,
                content,
                outline == Some(true),
            ]
            .into_iter()
            .filter(|&paints| paints)
            .count();
        inside > 1 || shadows(false) + usize::from(outline == Some(false)) > 1
    };
    let effect_layer = (made.layer
        || opacity > 0. && opacity < 1. && (!node.children.is_empty() || overlapping())
        || composited)
        .then_some(opacity);
    let selection_colors = match styles.selections.get(&id) {
        Some(selection) => style::selection_colors(style, Some(selection)),
        None => parsed.selection,
    };
    let face = &parsed.face;
    let line_height = parsed.line_height;
    let above = || {
        f32::from(text.baseline(
            text.face(face),
            px(units.font.em),
            px(line_height),
            pixel_ratio,
        ))
    };
    let strut = || {
        let above = above();
        renderer::node()
            .w(px(0.))
            .h(px(above))
            .mb(px(line_height - above))
            .flex_shrink_0()
            .baseline(renderer::Baseline::Bottom)
    };
    let align_of = |element: &mut renderer::Child| match element {
        renderer::Child::Node(node) => node.style_ref().align_self,
        renderer::Child::Kept(id) | renderer::Child::Placed(id, _) => {
            build.tree.style_of(*id).and_then(|style| style.align_self)
        }
    };
    // The width of an inline-level box as a share of its line plus a length, with its side margins. `None`
    // inside: the box has percent sizes that are no such share, like a percent width with a fixed limit.
    // A wrapper that shrinks to the box gives these percentages nothing to resolve against.
    let percent_width = |element: &renderer::Child| {
        let style = match element {
            renderer::Child::Node(node) => node.style_ref(),
            renderer::Child::Kept(id) | renderer::Child::Placed(id, _) => {
                build.tree.style_of(*id)?
            }
        };
        // CSS percentages are `Calc` lengths, or bounds that are no such share. A plain fraction is a limit that
        // the box builder sets itself.
        let share = |length| match length {
            renderer::Length::Calc(fraction, offset) => Some((fraction, offset)),
            _ => None,
        };
        let limits = [style.min_size.width, style.max_size.width];
        if [style.size.width, limits[0], limits[1]]
            .iter()
            .any(|length| matches!(length, renderer::Length::Bound(_)))
        {
            return Some(None);
        }
        let Some((mut fraction, mut offset)) = share(style.size.width) else {
            return limits
                .iter()
                .any(|&limit| share(limit).is_some())
                .then_some(None);
        };
        // A limit changes the share only when it is a percentage of the same box.
        for (limit, max) in limits.into_iter().zip([false, true]) {
            match share(limit) {
                Some((limit, same)) if same == offset => {
                    fraction = if max {
                        fraction.min(limit)
                    } else {
                        fraction.max(limit)
                    }
                }
                None if !matches!(limit, renderer::Length::Px(_)) => {}
                _ => return Some(None),
            }
        }
        for margin in [style.margin.left, style.margin.right] {
            match (margin, share(margin)) {
                (_, Some((share, length))) => {
                    (fraction, offset) = (fraction + share, offset + length)
                }
                (renderer::Length::Px(length), _) => offset += length,
                _ => {}
            }
        }
        Some(Some((fraction, offset)))
    };
    // Makes a wrapper as wide as its line, so the percent width of the box in it takes the line. Negative
    // margins cut the wrapper back to the box, on the sides where the line has its free space.
    let spread = |style: &mut renderer::Style,
                  (fraction, offset): (f32, renderer::Pixels),
                  justify: Option<renderer::AlignContent>| {
        let spare = |margin: renderer::Length, share: f32| {
            let own = match margin {
                renderer::Length::Px(own) => own,
                _ => px(0.),
            };
            renderer::Length::Calc((fraction - 1.) * share, offset * share + own)
        };
        (style.size.width, style.flex_shrink) = (relative(1.), 0.);
        style.justify_content = justify;
        match justify {
            Some(renderer::AlignContent::Center) => {
                style.margin.left = spare(style.margin.left, 0.5);
                style.margin.right = spare(style.margin.right, 0.5);
            }
            Some(_) => style.margin.left = spare(style.margin.left, 1.),
            None => style.margin.right = spare(style.margin.right, 1.),
        }
    };
    // Combine the zero-width strut with the first item, or Taffy can wrap the first item after the strut.
    // An item whose percent sizes are no share of the line stays apart, so they resolve against the line.
    let with_strut = |mut element: renderer::Child, justify: Option<renderer::AlignContent>| {
        let align = align_of(&mut element);
        let percent = percent_width(&element);
        if percent == Some(None) {
            return (strut().into(), Some(element), align.is_some());
        }
        let mut group = renderer::node()
            .flex()
            .flex_row()
            .flex_shrink(1.)
            .child(strut())
            .child(element);
        group.style().align_items = Some(renderer::AlignItems::Baseline);
        group.style().align_self = align;
        group.style().inline = renderer::Inline::Lines;
        if let Some(Some(share)) = percent {
            spread(group.style(), share, justify);
        }
        (renderer::Child::from(group), None, align.is_some())
    };
    let mut strut_due = false;
    // Each line has the strut: each box of a line of boxes carries one, or the space that it joins.
    let mut struts = false;
    let mut strut_next = false;
    if node.tag == "br" {
        let empty = starts_line(build, id);
        reader.log(Query::Break, depth, hashed(empty));
        let painter = view.clone();
        let ink = [Prop::CARET_COLOR, Prop::COLOR]
            .into_iter()
            .find_map(|prop| style::color(style.get(prop)?))
            .map_or(Color::hex(0x000000ff), style::rgba);
        let view = view.clone();
        let above = above();
        // Like Chromium, the box of a `<br>` is the content area of its font, not the line.
        let [ascent, descent, _] = text
            .line_metrics(text.face(face), px(units.font.em * pixel_ratio))
            .map(|value| f32::from(value) / pixel_ratio);
        let mut br = renderer::canvas(
            move |bounds, _| {
                let view = &*view;
                view.host
                    .borrow_mut()
                    .dom
                    .entry(view.id)
                    .or_default()
                    .bounds
                    .insert(
                        id,
                        [
                            f32::from(bounds.left()),
                            f32::from(bounds.top()) + above - ascent,
                            0.,
                            ascent + descent,
                        ],
                    );
            },
            move |bounds, _, window| {
                let caret = painter.host.try_borrow().is_ok_and(|host| {
                    host.dom
                        .get(&painter.id)
                        .is_some_and(|dom| dom.selection_caret == Some(id))
                });
                if caret {
                    window.paint_volatile();
                    super::input::paint_caret(
                        &painter,
                        [id, 0, 0],
                        Rect::new(bounds.origin, size(px(1.), px(line_height))),
                        ink,
                        window,
                    );
                }
            },
        )
        .h(px(above))
        .mb(px(line_height - above))
        .flex_shrink_0()
        .baseline(renderer::Baseline::Bottom);
        br = if empty { br.w_full() } else { br.w(px(0.)) };
        return br.into();
    }
    let flow = !matches!(display, "flex" | "inline-flex" | "grid" | "inline-grid");
    // A box that skips its contents sizes as if empty, or as its `contain-intrinsic-size`. For a script that reads
    // the geometry of the contents, it builds them but keeps the size it had without them.
    let (skipped, forced, intrinsic, last) = if info[id].flags & cull::SKIPS != 0 {
        let last = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|dom| dom.cull.widths.get(&id).copied());
        let mut measured = measured.borrow_mut();
        let skips = cull::skips(info[id].flags, style, id, &measured, false);
        let forced = skips && measured.forced.contains(&id);
        // A box remembers the last size it had with its contents.
        let remembered = measured.remembered.entry(id).or_insert((None, true));
        let was = remembered.1;
        if skips && !was {
            remembered.0 = last.or(remembered.0);
        }
        remembered.1 = skips;
        let remembered = remembered.0;
        (
            skips && !forced,
            forced,
            skips.then(|| super::grid::intrinsic_size(style, &units, remembered)),
            // The last size is the size without the contents only if the last build skipped them.
            last.filter(|_| was),
        )
    } else {
        (false, false, None, None)
    };
    let mut flat = Vec::new();
    let drawable = style::drawable_canvas(node, style);
    let mut pending: Vec<usize> = if skipped {
        Vec::new()
    } else if drawable {
        let boxes = |child: &&usize| !doc.nodes[**child].tag.starts_with('#');
        node.children.iter().rev().filter(boxes).copied().collect()
    } else if matches!(
        node.tag.as_str(),
        "svg" | "canvas" | "iframe" | "progress" | "meter" | "terminal"
    ) {
        Vec::new()
    } else {
        node.children.iter().rev().copied().collect()
    };
    // Spliced inline boxes, with the range of their content in `flat`. The end of a box is `!index` in `pending`.
    let mut cuts: Vec<(usize, usize, usize)> = Vec::new();
    // The parts of the spliced boxes: their place in `flat`, and the key of their text or their first and last block.
    let mut cut_parts: Vec<(usize, usize, super::paint::Piece)> = Vec::new();
    // The block `::before` and `::after` of spliced boxes: their place in `flat` and their index.
    let mut cut_pseudos: Vec<(usize, usize)> = Vec::new();
    while let Some(child) = pending.pop() {
        if child > usize::MAX / 2 {
            let cut = cuts[!child].0;
            if style::pseudo_blocks(styles, cut).is_some_and(|[_, after]| after) {
                cut_pseudos.push((flat.len(), 1));
                flat.push(cut);
            }
            cuts[!child].2 = flat.len();
        // A plain block is an element with a box, and its flags are closer than its node.
        } else if info[child].flags & cull::PLAIN_BLOCK != 0 {
            flat.push(child);
        } else if styles.values[child]
            .get(Prop::DISPLAY)
            .is_some_and(|value| value == "contents")
        {
            pending.extend(doc.nodes[child].children.iter().rev());
        } else if flow && !drawable && style::holds_block_box(doc, styles, child) && {
            let spliced = style::spliced(doc, styles, child);
            reader.log(Query::Spliced(child as u32), depth, hashed(spliced));
            spliced
        } {
            pending.push(!cuts.len());
            cuts.push((child, flat.len(), flat.len()));
            pending.extend(doc.nodes[child].children.iter().rev());
            if style::pseudo_blocks(styles, child).is_some_and(|[before, _]| before) {
                cut_pseudos.push((flat.len(), 0));
                flat.push(child);
            }
        } else if !matches!(doc.nodes[child].tag.as_str(), "#comment" | "#doctype")
            && !(doc.nodes[child].tag == "br"
                && styles.values[child].get(Prop::DISPLAY) == Some("none"))
            && !(doc.nodes[child].tag == "#text"
                && (matches!(node.tag.as_str(), "select" | "optgroup")
                    // A closed <details> shows only its summary.
                    || node.tag == "details" && !node.attrs.contains_key("open")))
        {
            flat.push(child);
        }
    }
    if matches!(display, "flex" | "inline-flex" | "grid" | "inline-grid") {
        measured
            .borrow_mut()
            .note_runs(doc, styles, id, &flat, true);
    }
    let cut_ids: rustc_hash::FxHashSet<usize> = cuts.iter().map(|&(cut, ..)| cut).collect();
    let floated = |child: usize| {
        flow && float_side(&styles.values[child]).is_some()
            && styles.values[child]
                .get(Prop::POSITION)
                .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
    };
    let inline_level = |child: usize| {
        doc.nodes[child].tag.starts_with('#')
            || doc.nodes[child].tag == "br"
            || !floated(child)
                && styles.values[child]
                    .get(Prop::DISPLAY)
                    .is_none_or(|value| value.starts_with("inline"))
                && !style::holds_block_box(doc, styles, child)
            || styles.values[child]
                .get(Prop::POSITION)
                .is_some_and(|value| matches!(value, "absolute" | "fixed"))
    };
    let inline_text = || {
        measured
            .borrow_mut()
            .texts
            .entry(id)
            .or_insert_with(|| {
                style::inline_text(doc, styles, id, &[], &[], [true; 2]).map(Rc::new)
            })
            .clone()
    };
    let breaks = display != "inline"
        && flat.iter().any(|&child| doc.nodes[child].tag == "br")
        && inline_text().is_none();
    // A multi-column box puts its lines into the columns, so its children never make the lines of the box itself.
    let all_inline = flow
        && !multicol
        && !flat.is_empty()
        && !breaks
        && flat.iter().all(|&child| {
            inline_level(child) || styles.values[child].get(Prop::DISPLAY) == Some("none")
        });
    // Inline content between the blocks of a box, which lays out as one paragraph.
    let in_run = |child: usize| {
        let node = &doc.nodes[child];
        match node.tag.as_str() {
            "#text" | "#comment" => true,
            _ => {
                let own = &styles.values[child];
                own.get(Prop::DISPLAY) == Some("none")
                    || inline_level(child)
                        && !floated(child)
                        && own
                            .get(Prop::POSITION)
                            .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
                    // A float in a spliced box goes beside the line that holds its place.
                    || floated(child)
                        && !cuts.is_empty()
                        && std::iter::successors(node.parent, |&up| doc.nodes[up].parent)
                            .take_while(|&up| up != id)
                            .any(|up| cut_ids.contains(&up))
            }
        }
    };
    // The text of the run of `flat` from `index` to `end`, which the first and last runs share with the
    // pseudo-elements of the box. Only boxes with spliced boxes in them hold those in their runs.
    let content = |child: usize| {
        in_run(child) || !floated(child) && info[child].flags & cull::OUT_OF_FLOW == 0
    };
    let first_content = flat.iter().position(|&child| content(child));
    let last_content = flat.iter().rposition(|&child| content(child));
    let run_texts: RefCell<rustc_hash::FxHashMap<usize, Option<Rc<style::InlineText>>>> =
        RefCell::default();
    let run_text = |index: usize, end: usize| {
        let run = &flat[index..end];
        let parts = run_cuts(build, &cuts, &flat, [index, end], in_run);
        let found = run_texts
            .borrow_mut()
            .entry(index)
            .or_insert_with(|| {
                let ends = [
                    !run.is_empty() && first_content == Some(index),
                    !run.is_empty() && last_content == Some(end.saturating_sub(1)),
                ];
                (run.is_empty()
                    || run.iter().any(|&kid| {
                        doc.nodes[kid].tag == "#text"
                            && !doc.nodes[kid].text.trim_ascii().is_empty()
                    })
                    // The edges of a spliced box make a line also with only white space.
                    || parts.iter().any(|&(cut, flags)| {
                        flags & style::CUT_FIRST != 0 && edge(build, cut, 3)
                            || flags & style::CUT_LAST != 0 && edge(build, cut, 1)
                    }))
                .then(|| style::inline_text(doc, styles, id, run, &parts, ends))
                .flatten()
                .map(|mut data| {
                    // Far above node ids, and unique per parent, so these lines keep their keys.
                    if run.is_empty() {
                        data.key = !(id << 20 | index);
                    }
                    Rc::new(data)
                })
            })
            .clone();
        (found, parts)
    };
    // The runs that start and end the content of a box with spliced boxes.
    let edge_text = |index: usize| {
        let at = [first_content, last_content][index]?;
        if cuts.is_empty() || !in_run(flat[at]) {
            return None;
        }
        let [mut start, mut end] = [at, at + 1];
        while start > 0 && in_run(flat[start - 1]) {
            start -= 1;
        }
        while end < flat.len() && in_run(flat[end]) {
            end += 1;
        }
        run_text(start, end).0
    };
    // An inline box with only blank text or comments, which can make a line of its own.
    let empty_inline = || {
        display == "inline"
            && node.children.iter().all(|&child| {
                doc.nodes[child].tag.starts_with('#') && style::makes_no_line(doc, styles, child)
            })
    };
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    let line_justify = match (style.get(Prop::TEXT_ALIGN), rtl) {
        (Some("right" | "-webkit-right"), _)
        | (Some("end"), false)
        | (None | Some("start" | "justify"), true) => Some(renderer::AlignContent::End),
        (Some("center" | "-webkit-center"), _) => Some(renderer::AlignContent::Center),
        _ => None,
    };
    let line_align = Some(renderer::AlignItems::Baseline);
    // Use the in-flow image's ratio for auto height: Taffy drops a flex-row item's ratio with percentage
    // width.
    let (mut natural, mut natural_ratio) = match node.tag.as_str() {
        "img" | "video" => (None, None),
        _ => natural_size(doc, id, None, style)
            .map_or((None, None), |(size, ratio)| (Some(size), ratio)),
    };
    #[cfg(debug_assertions)]
    let checking = reader.checking;
    #[cfg(not(debug_assertions))]
    let checking = false;
    // Include every shared-style input in the key: plan, call, own reads and child facts.
    // A box whose children make no line, like spaces and hidden elements, has no baseline of its own.
    let blank = || {
        node.children.iter().all(|&child| {
            let kid = &doc.nodes[child];
            match kid.tag.as_str() {
                "#text" => kid.text.trim_ascii().is_empty(),
                "#comment" => true,
                _ => styles.values[child].get(Prop::DISPLAY) == Some("none"),
            }
        })
    };
    let share = (parsed.shareable
        && !checking
        && info[id].flags & cull::SKIPS == 0
        && slot.is_none()
        && parent_perspective.is_none()
        && natural.is_none()
        && id != 0
        && !broken
        && node.native_control(style).is_none()
        && !styles.pseudos.contains_key(&id)
        && !top_layer.contains(&id)
        && matches!(
            node.tag.as_str(),
            "div"
                | "span"
                | "a"
                | "p"
                | "section"
                | "article"
                | "header"
                | "footer"
                | "nav"
                | "main"
                | "aside"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "b"
                | "i"
                | "strong"
                | "em"
                | "small"
                | "label"
                | "ul"
                | "ol"
                | "li"
                | "code"
                | "form"
        ))
    .then(|| {
        use std::hash::{Hash, Hasher};
        let texted = inline_text().map(|data| (data.text.is_empty(), data.greedy));
        let mut hasher = rustc_hash::FxHasher::default();
        (
            Rc::as_ptr(&parsed),
            containing.map(f32::to_bits),
            container.map(f32::to_bits),
            block_size.is_some(),
            flex_parent,
            columned,
            info[id].flags & !(cull::CHANGED | cull::CHILDREN_CHANGED | cull::GEOMETRY_CHANGED),
            (
                all_inline,
                node.children.is_empty(),
                texted,
                blank(),
                empty_inline(),
            ),
            // A box at the top of a shadow tree takes no floats from outside, so it gets no Flow::BLOCK.
            node.parent
                .is_none_or(|parent| doc.nodes[parent].tag.starts_with('#')),
        )
            .hash(&mut hasher);
        hasher.finish()
    });
    let hit = share.and_then(|key| {
        let shared = view.shared.borrow();
        let host = view.host.borrow();
        let dom = host.dom.get(&view.id);
        let here = |read: &Read| Read {
            query: read.query.moved(u32::MAX, id as u32).unwrap_or(read.query),
            ..*read
        };
        let found = shared.get(&key)?.iter().rev().find(|entry| {
            Rc::ptr_eq(&entry.plan, &parsed)
                && entry.reads.iter().all(|read| {
                    reader.replay(id, containing, container, None, dom, &here(read)) == Replay::Same
                })
        })?;
        reads
            .borrow_mut()
            .extend(found.reads.iter().map(|read| Read {
                scope: if read.scope == 0 {
                    0
                } else {
                    (depth + 2).saturating_sub(usize::from(read.scope)).max(1) as u16
                },
                reader: depth as u16,
                ..here(read)
            }));
        Some(found.style.clone())
    });
    let mut el = match &hit {
        Some(style) => renderer::shared(style.clone()).id(id),
        None => {
            let mut el = renderer::node()
                .id(id)
                .flex()
                .flex_col()
                .text_size(px(units.font.em))
                .font_family(face.family.clone())
                .font_weight(face.weight)
                .line_height(px(line_height));
            el.style().flex_shrink = if flex_parent { 1. } else { 0. };
            el.style().baseline = match display {
                "inline-block" if !flex_parent && slot.is_none() => renderer::Baseline::Last,
                "flex" | "inline-flex" | "grid" | "inline-grid" => renderer::Baseline::First,
                _ => renderer::Baseline::Auto,
            };
            el.style().block = matches!(
                display,
                "block" | "inline-block" | "list-item" | "flow-root"
            );
            el.style().contains = (u8::from(made.absolute) * renderer::Escape::ABSOLUTE)
                | (u8::from(made.fixed) * renderer::Escape::FIXED);
            el.style().escape = match position {
                "absolute" => renderer::Escape::ABSOLUTE,
                "fixed" => renderer::Escape::FIXED,
                _ => 0,
            };
            if position == "absolute" {
                el.style().placed = inset_axes(style);
            }
            el.style().scroll_anchor = (u8::from(style.get(Prop::OVERFLOW_ANCHOR) == Some("none"))
                * renderer::ScrollAnchor::NONE)
                | (u8::from(position == "sticky") * renderer::ScrollAnchor::STICKY);
            // Only boxes in columns break, so others skip these reads.
            if columned {
                use renderer::Break;
                for (props, forced, avoid) in [
                    (
                        [Prop::BREAK_BEFORE, Prop::PAGE_BREAK_BEFORE],
                        Break::BEFORE,
                        Break::AVOID_BEFORE,
                    ),
                    (
                        [Prop::BREAK_AFTER, Prop::PAGE_BREAK_AFTER],
                        Break::AFTER,
                        Break::AVOID_AFTER,
                    ),
                    (
                        [Prop::BREAK_INSIDE, Prop::PAGE_BREAK_INSIDE],
                        0,
                        Break::AVOID_INSIDE,
                    ),
                ] {
                    match props.iter().find_map(|&prop| style.get(prop)) {
                        Some("column") => el.style().breaks |= forced,
                        Some(value) if value.starts_with("avoid") => el.style().breaks |= avoid,
                        _ => {}
                    }
                }
                if matches!(
                    node.tag.as_str(),
                    "img"
                        | "svg"
                        | "canvas"
                        | "video"
                        | "audio"
                        | "iframe"
                        | "embed"
                        | "object"
                        | "input"
                        | "textarea"
                        | "select"
                        | "button"
                        | "meter"
                        | "progress"
                        | "webview"
                ) || display.starts_with("inline-")
                {
                    // Like Chrome, atomic inline boxes are monolithic.
                    el.style().breaks |= Break::WHOLE;
                }
                let kept = [Prop::ORPHANS, Prop::WIDOWS].map(|prop| {
                    style
                        .get(prop)
                        .and_then(|value| value.parse::<u8>().ok())
                        .unwrap_or(2)
                });
                if kept != [2, 2] {
                    el.text_style().set_lines_kept(kept);
                }
            }
            {
                let text = el.text_style();
                text.set_letter_spacing(parsed.spacing[0]);
                text.set_word_spacing(parsed.spacing[1]);
                text.set_font_smoothing(!parsed.smoothing);
                let (indent, percent) = text_indent(style, &units);
                text.set_text_indent(px(indent));
                text.set_text_indent_percent(percent);
            }
            if face.style != renderer::FontStyle::Normal {
                el.text_style().set_font_style(face.style);
            }
            if face.synthesis != [true; 2] {
                el.text_style().set_font_synthesis(face.synthesis);
            }
            el
        }
    };
    if stacking {
        if hit.is_none() {
            el = el.stacking();
        }
        el = el.child(renderer::layers(true));
    }
    if all_inline || display == "inline" {
        if hit.is_none() {
            el = el.flex_row().flex_wrap();
            el.style().justify_content = line_justify;
            el.style().align_items = line_align;
            // A box that is stretched taller keeps its lines at the top, without space between them.
            el.style().align_content = Some(renderer::AlignContent::FlexStart);
            el.style().inline = renderer::Inline::Lines;
        }
        let merged = !text_field(node, style)
            && !matches!(node.tag.as_str(), "svg" | "canvas" | "iframe" | "terminal")
            && inline_text().is_some();
        if !merged
            && flat.iter().any(|&child| {
                !doc.nodes[child].tag.starts_with('#')
                    && doc.nodes[child].tag != "br"
                    && styles.values[child].get(Prop::DISPLAY) != Some("inline")
                    && in_flow(doc, info, child)
                    && !style::makes_no_line(doc, styles, child)
            })
        {
            (strut_due, struts) = (display != "inline", true);
        }
    }
    // The content of replaced elements paints above the background layers, which are built later.
    let mut content_layers: Vec<Child> = Vec::new();
    if node.tag == "canvas" {
        let picture = |source| {
            renderer::node().size_full().hooks(super::paint::Picture {
                id,
                view: view.clone(),
                source,
                fit: renderer::ObjectFit::Fill,
                cropped: None,
                position: None,
                pixelated: matches!(
                    style.get(Prop::IMAGE_RENDERING),
                    Some("pixelated" | "crisp-edges")
                ),
            })
        };
        let mut presented = picture(super::paint::Source::Presented)
            .child(picture(super::paint::Source::Drawn).absolute());
        presented.style().aspect_ratio = natural_ratio;
        content_layers.push(presented.into());
    }
    let mut grid_bar = None;
    if node.tag == "terminal" {
        let host = view.host.borrow();
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !host.enabled("terminal") && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!(
                "apps: The terminal feature is disabled. Add \"terminal\" to \"features\" in package.json"
            );
        }
        if let Some(state) = host.dom.get(&view.id).filter(|_| host.enabled("terminal")) {
            let look = super::terminal::look(style, styles.selections.get(&id), &units);
            // The bar of the scrollback.
            grid_bar = Some(scrollbar::Scrollbar {
                id,
                handle: (state.terminals.borrow_mut().bars.entry(id).or_default().0).clone(),
                look: scrollbar::Look::new(style, None, &units, Some(look.background())),
                axes: [false, true],
                reserved: [0.; 2],
                borders: insets(styles, id, false, containing[0]),
                origin: [0.; 2],
                quiet: true,
            });
            content_layers.push(
                renderer::node()
                    .size_full()
                    .hooks(super::terminal::Grid {
                        id,
                        view: view.clone(),
                        terminals: state.terminals.clone(),
                        look: Rc::new(look),
                        edges: insets(styles, id, true, containing[0]),
                    })
                    .into(),
            );
        }
    }
    let mut picture = None;
    if node.tag == "img" || node.tag == "video" {
        let (image, vector) = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .filter(|_| !broken)
            .map_or((None, None), |state| {
                (
                    state.images.get(&id).cloned(),
                    state.svg_images.get(&id).cloned(),
                )
            });
        reader.log(
            Query::Image(id as u32),
            0,
            pictured(image.as_ref(), vector.as_ref()),
        );
        (natural, natural_ratio) = natural_size(
            doc,
            id,
            image
                .as_ref()
                .map(|image| {
                    let size = image.size();
                    [size.width as u32 as f32, size.height as u32 as f32]
                })
                .or(vector.as_ref().map(|(_, natural)| *natural)),
            style,
        )
        .map_or((None, None), |(size, ratio)| (Some(size), ratio));
        if image.is_some() || vector.is_some() {
            let fit = match style.get(Prop::OBJECT_FIT) {
                Some("contain") => renderer::ObjectFit::Contain,
                Some("cover") => renderer::ObjectFit::Cover,
                Some("none") => renderer::ObjectFit::None,
                Some("scale-down") => renderer::ObjectFit::ScaleDown,
                _ => renderer::ObjectFit::Fill,
            };
            if image.is_some() {
                picture = Some(fit);
            } else {
                let padding = units.edges(style, style::PADDING, containing[0]);
                content_layers.push(svg_layer(id, false, false, view.clone(), padding));
            }
        }
        if image.is_none() && natural_ratio.is_some() {
            let mut sized = renderer::node().size_full();
            sized.style().aspect_ratio = natural_ratio;
            el = el.child(sized);
        } else if natural.is_none() && !sizeless {
            for (name, prop, horizontal) in [
                ("width", Prop::WIDTH, true),
                ("height", Prop::HEIGHT, false),
            ] {
                if style.get(prop).is_none_or(|value| value == "auto")
                    && let Some(length) = node
                        .attrs
                        .get(name)
                        .and_then(|value| dimension(value, false))
                {
                    el = if horizontal {
                        el.w(px(length))
                    } else {
                        el.h(px(length))
                    };
                }
            }
        }
    }
    if broken {
        let icon = || {
            renderer::canvas(
                |_, _| (),
                |bounds, _, window| paint_broken_icon(bounds.origin, window),
            )
            .w(px(16.))
            .h(px(16.))
            .flex_shrink_0()
        };
        let framed = alt.is_none()
            && natural.is_some_and(|size| size != [0., 0.])
            && [("width", Prop::WIDTH), ("height", Prop::HEIGHT)]
                .iter()
                .all(|&(name, prop)| {
                    style.get(prop).is_some_and(|value| value != "auto")
                        || node.attrs.contains_key(name)
                });
        el = if let Some(alt) = alt {
            let mut text = renderer::text(
                alt.clone(),
                Some(vec![style::text_run(style, &units, alt.len())]),
            );
            text.text_style().set_float(px(16.));
            el.child(
                renderer::node()
                    .min_h(px(16.))
                    .child(text)
                    .child(icon().absolute().top_0().left_0()),
            )
        } else if framed {
            el.child(
                renderer::canvas(
                    |_, _| (),
                    |bounds, _, window| {
                        window.draw_box(
                            bounds,
                            px(0.),
                            renderer::Color::TRANSPARENT,
                            renderer::Border {
                                widths: px(1.).into(),
                                color: Color::hex(0xc0c0c0ff),
                                style: renderer::BorderStyle::Solid,
                                sides: 0,
                            },
                        );
                        if bounds.size.width >= px(18.) && bounds.size.height >= px(18.) {
                            paint_broken_icon(bounds.origin + point(px(2.), px(2.)), window);
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
        } else if natural == Some([16., 16.]) {
            el.child(icon())
        } else {
            el
        };
    }
    if node.tag == "svg" {
        // Padding and borders, which the drawing doesn't fill.
        let frame = {
            let padding = units.edges(style, style::PADDING, containing[0]);
            let borders = style::BORDERS.map(|[width, kind, _]| {
                style
                    .get(kind)
                    .filter(|kind| !matches!(*kind, "none" | "hidden"))
                    .and_then(|_| units.resolve(style.get(width).unwrap_or("3px"), containing[0]))
                    .unwrap_or(0.)
                    .max(0.)
            });
            [
                padding[1] + padding[3] + borders[1] + borders[3],
                padding[0] + padding[2] + borders[0] + borders[2],
            ]
        };
        let border_box = style.get(Prop::BOX_SIZING) == Some("border-box");
        let [width, height] = [(Prop::WIDTH, "width"), (Prop::HEIGHT, "height")]
            .into_iter()
            .enumerate()
            .map(|(axis, (prop, attr))| {
                match style.get(prop) {
                    Some(value) => units.resolve(value, containing[axis]),
                    None => node.attrs.get(attr).and_then(|v| dimension(v, false)),
                }
                .map(|size| if border_box { size - frame[axis] } else { size })
            })
            .collect::<Vec<_>>()[..]
        else {
            unreachable!()
        };
        let view_box = node
            .attrs
            .get("viewBox")
            .map(|value| {
                value
                    .split(|c: char| c == ',' || c.is_ascii_whitespace())
                    .filter_map(|part| part.parse::<f32>().ok())
                    .collect::<Vec<_>>()
            })
            .filter(|parts| parts.len() == 4 && parts[2] > 0. && parts[3] > 0.);
        let ratio = view_box.as_ref().map(|parts| parts[2] / parts[3]);
        // Like browsers: an SVG with a ratio and no size fills the available width, and one with neither is
        // 300 by 150.
        let available = || {
            let margins = units.edges(style, style::MARGIN, containing[0]);
            let insets = if matches!(position, "absolute" | "fixed") {
                [Prop::LEFT, Prop::RIGHT].map(|prop| {
                    style
                        .get(prop)
                        .and_then(|value| units.resolve(value, containing[0]))
                        .unwrap_or(0.)
                })
            } else {
                [0.; 2]
            };
            containing[0] - margins[1] - margins[3] - insets[0] - insets[1]
        };
        let [width, height] = match (width, height, ratio) {
            (None, Some(height), Some(ratio)) => [height * ratio, height],
            (Some(width), None, Some(ratio)) => [width, width / ratio],
            // A stretched flex item in a row of definite height takes that height.
            (None, None, Some(ratio))
                if flex_parent
                    && reader
                        .parent(id, Prop::FLEX_DIRECTION)
                        .is_none_or(|value| value.starts_with("row"))
                    && reader
                        .parent(id, Prop::HEIGHT)
                        .is_some_and(|value| value.ends_with("px"))
                    && style
                        .get(Prop::ALIGN_SELF)
                        .filter(|value| *value != "auto")
                        .or_else(|| reader.parent(id, Prop::ALIGN_ITEMS))
                        .is_none_or(|value| matches!(value, "normal" | "stretch"))
                    && containing[1] > 0. =>
            {
                let margins = units.edges(style, style::MARGIN, containing[0]);
                let height = containing[1] - margins[0] - margins[2] - frame[1];
                [height * ratio, height]
            }
            (None, None, Some(ratio)) => {
                let width = available() - frame[0];
                [width, width / ratio]
            }
            _ => [width.unwrap_or(300.), height.unwrap_or(150.)],
        }
        .map(|value| value.clamp(1., 4096.));
        let [inner_width, inner_height] = [width, height];
        let [width, height] = [width + frame[0], height + frame[1]];
        let mut scope = id;
        while let Some(parent) = doc.nodes[scope].parent {
            scope = parent;
        }
        let mut pending = vec![id];
        let mut visited = rustc_hash::FxHashSet::default();
        let mut linked = false;
        let mut targets = Vec::new();
        let mut painted = Vec::new();
        let mut hasher = rustc_hash::FxHasher::default();
        while let Some(child) = pending.pop() {
            if !visited.insert(child) {
                continue;
            }
            std::hash::Hash::hash(&(child, doc.nodes.revision(child)), &mut hasher);
            painted.push(styles.values[child].clone());
            let child = &doc.nodes[child];
            pending.extend(&child.children);
            linked |= child.tag == "use";
            if child.tag == "use"
                && let Some(reference) = href(child)
                && let Some(target) =
                    super::effects::index(ids, doc)
                        .get(reference)
                        .and_then(|targets| {
                            targets.iter().copied().find(|&target| {
                                let mut root = target;
                                while let Some(parent) = doc.nodes[root].parent {
                                    if parent == id {
                                        return false;
                                    }
                                    root = parent;
                                }
                                root == scope
                            })
                        })
                && !visited.contains(&target)
            {
                targets.push(target);
                pending.push(target);
            }
        }
        // `uses` finds a link only through a `use` element of the subtree just walked.
        let used = if linked {
            uses(doc, styles, ids, id)
        } else {
            None
        };
        if used.is_some() {
            reader.log(Query::Uses(id as u32), 1, hashed(used));
        }
        let key = (
            [inner_width, inner_height],
            std::hash::Hasher::finish(&hasher),
        );
        let props = || style::SVG.iter().chain(&SVG_SOURCE);
        let kept = view.svgs.borrow().get(&id).is_some_and(|(old, at)| {
            *at == key
                && old.len() == painted.len()
                && old
                    .iter()
                    .zip(&painted)
                    .enumerate()
                    .all(|(index, (old, new))| {
                        Rc::ptr_eq(old, new)
                            || props().all(|&prop| {
                                index == 0 && !prop.inherited() || old.get(prop) == new.get(prop)
                            })
                    })
        }) && view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .is_some_and(|state| state.svg_sources.contains_key(&id));
        if !kept {
            let mut header = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{inner_width}\" height=\"{inner_height}\""
            );
            for (key, value) in &node.attrs {
                if !matches!(
                    key.as_str(),
                    "width" | "height" | "xmlns" | "xmlns:xlink" | "style"
                ) && style::presentation(key).is_none()
                {
                    header.push_str(&format!(" {key}=\"{}\"", crate::dom::escape(value)));
                }
            }
            header.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
            let svg = SvgSource {
                doc,
                styles,
                root: id,
                scale: view_box.as_ref().map_or(1., |parts| {
                    (inner_width / parts[2]).min(inner_height / parts[3])
                }),
                view: view_box
                    .as_ref()
                    .map_or([inner_width, inner_height], |parts| [parts[2], parts[3]]),
                header,
                ids,
            };
            let attrs = |element| svg.attrs(element, true, true);
            let mut source = svg.header.clone();
            for (name, value) in attrs(id) {
                source.push_str(&format!(" {name}=\"{}\"", crate::dom::escape(&value)));
            }
            source.push('>');
            source.push_str("<defs>");
            for &target in &targets {
                source.push_str(&doc.serialize_svg(target, false, &attrs));
            }
            source.push_str("</defs>");
            for child in &node.children {
                source.push_str(&doc.serialize_svg(*child, true, &attrs));
            }
            source.push_str("</svg>");
            let mut names = Vec::new();
            let mut counted = String::with_capacity(source.len());
            let mut rest = &*source;
            const MARK: &str = " id=\"apps-";
            while let Some(start) = rest.find(MARK) {
                let (before, after) = rest.split_at(start + MARK.len());
                counted.push_str(before);
                rest = after;
                if let Some(end) = after.find('"')
                    && let Ok(marked) = after[..end].parse()
                {
                    counted.push_str(&names.len().to_string());
                    names.push((String::new(), marked));
                    rest = &after[end..];
                }
            }
            counted.push_str(rest);
            let source = counted;
            let mut pending = node.children.clone();
            while let Some(child) = pending.pop() {
                if let Some(name) = doc.nodes[child].attrs.get("id") {
                    names.push((name.clone(), child));
                }
                pending.extend(&doc.nodes[child].children);
            }
            // CSS keywords ignore case, but resvg only reads `currentColor`.
            let lower = source.to_ascii_lowercase();
            let mut end = 0;
            let mut replaced = String::with_capacity(source.len());
            for (start, _) in lower.match_indices("currentcolor") {
                replaced.push_str(&source[end..start]);
                replaced.push_str("currentColor");
                end = start + "currentcolor".len();
            }
            replaced.push_str(&source[end..]);
            let source: Rc<str> = replaced.into();
            view.svgs.borrow_mut().insert(id, (painted, key));
            let mut host = view.host.borrow_mut();
            let state = host.dom.entry(view.id).or_default();
            // Discard the raster on source changes: paint checks only inline SVG dimensions.
            if state.svgs.get(&id).is_some_and(|(old, _)| *old != source)
                && let Some((_, image)) = state.svgs.remove(&id)
                && Arc::strong_count(&image) == 1
            {
                state.retired.push(image);
            }
            state.svg_boxes.insert(
                id,
                (
                    "".into(),
                    names,
                    [0.; 2],
                    Vec::new(),
                    [0.; 4],
                    [0.; 4],
                    [0.; 4],
                ),
            );
            state.svg_sources.insert(id, source);
        }
        el = el.w(px(width)).h(px(height));
        // As in Chrome, an `<svg>` with `overflow: visible` paints outside its box.
        let spill = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
            .into_iter()
            .all(|prop| style.get(prop).is_none_or(|value| value == "visible"));
        content_layers.push(svg_layer(
            id,
            true,
            spill,
            view.clone(),
            units.edges(style, style::PADDING, containing[0]),
        ));
    }
    let stretched = |axis: usize| {
        let [size, start, end] = [
            [Prop::WIDTH, Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT],
            [Prop::HEIGHT, Prop::MARGIN_TOP, Prop::MARGIN_BOTTOM],
        ][axis];
        flex_parent
            && reader
                .parent(id, Prop::FLEX_DIRECTION)
                .is_some_and(|value| value.starts_with("column"))
                == (axis == 0)
            && !matches!(position, "absolute" | "fixed")
            && style.get(size).is_none_or(|value| value == "auto")
            && [start, end]
                .iter()
                .all(|&prop| style.get(prop) != Some("auto"))
            && style
                .get(Prop::ALIGN_SELF)
                .filter(|value| *value != "auto")
                .or_else(|| reader.parent(id, Prop::ALIGN_ITEMS))
                .is_none_or(|value| matches!(value, "normal" | "stretch"))
    };
    let input = (node.tag == "input").then(|| node.input_type());
    let appkit = node.native_control(style);
    if appkit.is_some() {
        super::native::register(&view.controls, id);
    }
    let check = input.is_some_and(|kind| {
        kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio")
    });
    let editable = text_field(node, style);
    let dark = || {
        if style
            .get(Prop::COLOR_SCHEME)
            .is_some_and(|value| value.contains("dark") && value.contains("light"))
        {
            reader.log(Query::Theme, 0, hashed(crate::native::theme_state() & 1));
        }
        style::dark(style)
    };
    if input.is_some_and(|kind| kind.eq_ignore_ascii_case("file"))
        && let Some(part) = styles
            .scrollbars
            .get(&id)
            .and_then(|parts| parts[1].as_ref())
    {
        // The button takes its box from `::file-selector-button`, which the agent sheet always gives.
        let [padding, margin] =
            [style::PADDING, style::MARGIN].map(|sides| units.edges(part, sides, 0.));
        let borders = units.borders(part, 0.);
        let color = |prop| part.get(prop).and_then(style::paint);
        let radius = part
            .get(Prop::BORDER_TOP_LEFT_RADIUS)
            .and_then(|value| units.resolve(value, 0.))
            .unwrap_or(0.);
        let mut button = renderer::node()
            .flex_shrink_0()
            .mt(px(margin[0]))
            .mr(px(margin[1]))
            .mb(px(margin[2]))
            .ml(px(margin[3]))
            .pt(px(padding[0]))
            .pr(px(padding[1]))
            .pb(px(padding[2]))
            .pl(px(padding[3]))
            .border_t(px(borders[0]))
            .border_r(px(borders[1]))
            .border_b(px(borders[2]))
            .border_l(px(borders[3]))
            .border_color(color(Prop::BORDER_TOP_COLOR).unwrap_or(renderer::Color::TRANSPARENT))
            .bg(color(Prop::BACKGROUND_COLOR).unwrap_or(renderer::Color::TRANSPARENT))
            .child("Choose File");
        if let Some(ink) = color(Prop::COLOR) {
            button = button.text_color(ink);
        }
        if let Some(size) = part
            .get(Prop::FONT_SIZE)
            .and_then(|value| units.resolve(value, units.font.em))
            .filter(|size| *size != units.font.em)
        {
            let text = button.text_style();
            text.set_font_size(px(size));
            text.set_line_height(px(units.font.lh * size / units.font.em));
        }
        if let Some(weight) = part
            .get(Prop::FONT_WEIGHT)
            .filter(|weight| Some(*weight) != style.get(Prop::FONT_WEIGHT))
            .and_then(|weight| match weight {
                "bold" => Some(700.),
                "normal" => Some(400.),
                weight => weight.parse().ok(),
            })
        {
            button.text_style().set_font_weight(weight);
        }
        button.style().corner_radii = renderer::Corners::all(px(radius).into()).into();
        el = el
            .flex_row()
            .items_center()
            .overflow_hidden()
            .child(button)
            .child(renderer::node().whitespace_nowrap().child("No file chosen"));
        if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
            el = el.w(px(file_size(style, part, &units, text, pixel_ratio)[0]));
        }
    }
    if appkit == Some(renderer::ControlKind::Button) {
        let [padding, height] = super::native::size(renderer::ControlKind::Button);
        if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
            let label = node.button_label();
            let run = style::text_run(style, &units, label.len());
            let width = text.shape_line(label, px(units.font.em), &run).line.width;
            el = el.w(px(layout_ceil(f32::from(width) + padding, pixel_ratio)));
        }
        if style.get(Prop::HEIGHT).is_none_or(|value| value == "auto") {
            el = el.h(px(height));
        }
    }
    if appkit.is_none()
        && input.is_some_and(|kind| {
            ["button", "submit", "reset"]
                .iter()
                .any(|other| kind.eq_ignore_ascii_case(other))
        })
    {
        el = el.items_center().justify_center().child(SharedString::from(
            node.attrs.get("value").cloned().unwrap_or_else(|| {
                if input.is_some_and(|kind| kind.eq_ignore_ascii_case("reset")) {
                    "Reset".into()
                } else if input.is_some_and(|kind| kind.eq_ignore_ascii_case("submit")) {
                    "Submit".into()
                } else {
                    String::new()
                }
            }),
        ));
    }
    let field_line_height = if editable {
        field_line(node, style, &units, text, pixel_ratio)
    } else {
        0.
    };
    if editable && node.tag != "textarea" {
        let above = f32::from(text.baseline(
            text.face(&parsed.face),
            px(units.font.em),
            px(field_line_height),
            pixel_ratio,
        ));
        el.style().justify_content = Some(renderer::AlignContent::Center);
        el = el.child(
            renderer::node()
                .w(px(0.))
                .h(px(above))
                .mb(px(field_line_height - above))
                .flex_shrink_0()
                .baseline(renderer::Baseline::Bottom),
        );
    }
    let mut field_layer = None;
    if editable {
        if style.get(Prop::FIELD_SIZING) == Some("content") {
            reader.log(Query::Value(id as u32), 0, hashed(&node.value));
        }
        let text = node.shown_text(
            node.current_value()
                .map_or_else(|| doc.text(id), str::to_owned),
        );
        let placeholder = node.attrs.get("placeholder").cloned().unwrap_or_default();
        let font_size = units.font.em;
        let text_color = style
            .get(Prop::COLOR)
            .and_then(style::paint)
            .unwrap_or(renderer::Color::BLACK);
        let caret_color = style
            .get(Prop::CARET_COLOR)
            .and_then(style::paint)
            .unwrap_or(text_color);
        let pseudo = styles
            .text_pseudos
            .get(&id)
            .and_then(|pseudos| pseudos[3].as_ref());
        let mut placeholder_color = pseudo
            .and_then(|pseudo| style::paint(pseudo.get(Prop::COLOR)?))
            .unwrap_or(Color::hex(0x757575ff));
        placeholder_color.a *= pseudo
            .and_then(|pseudo| pseudo.get(Prop::OPACITY)?.parse::<f32>().ok())
            .map_or(1., |opacity| opacity.clamp(0., 1.));
        let single_line = node.tag != "textarea";
        let padding = units.edges(style, style::PADDING, containing[0]);
        let empty = text.is_empty();
        let face = match pseudo {
            Some(pseudo) if empty => style::font(pseudo),
            _ => parsed.face.clone(),
        };
        let mut shown = if empty { placeholder } else { text };
        if let Some(mode) = pseudo
            .filter(|_| empty)
            .unwrap_or(style)
            .get(Prop::TEXT_TRANSFORM)
            .filter(|mode| *mode != "none")
        {
            let len = shown.len();
            style::text_transform(&mut shown, 0..len, mode);
        }
        let shown: Rc<str> = shown.into();
        let run = renderer::TextRun {
            len: shown.len(),
            font: face,
            color: if empty { placeholder_color } else { text_color },
            background_color: None,
            underline: None,
            strikethrough: None,
            overline: None,
            letter_spacing: style::spacing(style, &units, Prop::LETTER_SPACING),
            word_spacing: px(0.),
            font_size: None,
            inset: Default::default(),
            nowrap: false,
            tab_size: 8.,
            stroke: None,
        };
        if !single_line && style.get(Prop::FIELD_SIZING) == Some("content") {
            // The full width of the content box, so the text wraps as in the field.
            let mut sizer = renderer::text(shown.clone(), Some(vec![run.clone()]))
                .w(relative(1.))
                .min_h(px(field_line_height))
                .flex_shrink_0();
            sizer.style().visibility = renderer::Visibility::Hidden;
            el = el.child(sizer);
        }
        let handle = view
            .host
            .borrow_mut()
            .dom
            .entry(view.id)
            .or_default()
            .scroll
            .entry(id)
            .or_default()
            .clone();
        let field = super::input::FieldText {
            selected: Default::default(),
            id,
            view: view.clone(),
            placeholder: empty,
            password: (node.password() && !empty)
                .then(|| node.current_value().unwrap_or("").to_owned()),
            text: shown,
            run,
            single_line,
            rtl: rtl && line_justify == Some(renderer::AlignContent::End),
            rows: Default::default(),
            caret: caret_color,
            selection: selection_colors,
            handle: handle.clone(),
        };
        let mut clip = renderer::node()
            .absolute()
            .track_scroll(&handle)
            .text_size(px(font_size))
            .line_height(px(field_line_height));
        clip.style().restrict_scroll_to_axis = true;
        clip.style().overscroll = overscroll(style);
        el = el.cursor(renderer::Cursor::Text);
        // Added after the background layers, which are absolute too and would paint over the text.
        field_layer = Some(if single_line {
            let line = renderer::node().flex_shrink_0().child(field);
            // A right-to-left row starts at the right, so text that overflows goes to the left and scrolls from there.
            let line = match (line_justify, rtl) {
                (Some(renderer::AlignContent::Center), _) => line.ml_auto().mr_auto(),
                (Some(renderer::AlignContent::End), false) => line.ml_auto(),
                (None, true) => line.mr_auto(),
                _ => line,
            };
            let mut clip = clip
                .top(px(padding[0]))
                .right(px(padding[1]))
                .bottom(px(padding[2]))
                .left(px(padding[3]))
                .overflow_hidden()
                .flex()
                .flex_row()
                .items_center()
                .whitespace_nowrap();
            if rtl {
                clip.style().flex_direction = renderer::FlexDirection::RowReverse;
            }
            clip.child(line)
        } else {
            if rtl && line_justify == Some(renderer::AlignContent::End) {
                clip.style().flow |= renderer::Flow::RTL_BLOCK;
            }
            clip.style().overflow.x = renderer::Overflow::Scroll;
            clip.top_0()
                .right(px(0.))
                .bottom(px(0.))
                .left_0()
                .pt(px(padding[0]))
                .pb(px(padding[2]))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                // The text takes the side padding, so like the inner editor of Chrome, the end padding doesn't
                // follow text that overflows.
                .child(
                    renderer::Node::from(field)
                        .pl(px(padding[3]))
                        .pr(px(padding[1])),
                )
        });
    }
    if hit.is_none()
        && let Some(ink) = style
            .get(Prop::WEBKIT_TEXT_FILL_COLOR)
            .and_then(style::paint)
            .or(parsed.ink)
    {
        el = el.text_color(ink);
    }
    let inline_box = display == "inline"
        && !node.tag.starts_with('#')
        && !flex_parent
        && slot.is_none()
        && !matches!(
            node.tag.as_str(),
            "img" | "svg" | "canvas" | "input" | "button" | "select" | "textarea" | "iframe"
        )
        && !matches!(position, "absolute" | "fixed");
    let mut inline_fill = renderer::Color::TRANSPARENT;
    let mut clipped_fill = None;
    // With `background-clip: text`, the background paints in one layer that shows through the text.
    let text_clip = !matches!(node.tag.as_str(), "html" | "body")
        && style
            .get(Prop::BACKGROUND_CLIP)
            .is_some_and(|value| value.contains("text"));
    let mut text_fill = None;
    // The root element paints the background of the canvas, also when it comes from the body.
    let source = matches!(node.tag.as_str(), "html" | "body")
        .then(|| super::background::canvas_source(doc, styles))
        .flatten();
    let propagated = node.tag == "body" && source == Some(id);
    let canvas = source.filter(|_| node.tag == "html" && node.parent == Some(0));
    let mut cell_background = parsed.background.filter(|_| !propagated);
    if cell_background.is_none() && style::role(styles, id) == style::CELL {
        cell_background = row_fill(doc, styles, id);
        let table = std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
            .find(|&parent| !(style::ROW..=style::FOOTER).contains(&style::role(styles, parent)));
        reader.log(
            Query::RowFill,
            table.map_or(0, |table| reader.scope_of(id, table)),
            hashed(cell_background.map(|fill| fill.to_array().map(f32::to_bits))),
        );
    }
    if let Some(color) = cell_background {
        let blended = style
            .get(Prop::BACKGROUND_BLEND_MODE)
            .is_some_and(|value| value.split(',').any(|mode| mode.trim() != "normal"))
            && style
                .get(Prop::BACKGROUND_IMAGE)
                .is_some_and(|value| value != "none");
        // The color fills the clip box of the bottom layer.
        let clip = style.get(Prop::BACKGROUND_CLIP).unwrap_or("");
        let clip = if clip.contains(',') {
            let clips = style::components(clip, ',');
            let layers = style
                .get(Prop::BACKGROUND_IMAGE)
                .map_or(1, |value| style::components(value, ',').len());
            clips
                .get((layers.max(1) - 1) % clips.len().max(1))
                .map_or("", |clip| clip.trim())
        } else {
            clip.trim()
        };
        if blended {
        } else if text_clip {
            text_fill = Some(color);
        } else if inline_box {
            inline_fill = color;
        } else if matches!(clip, "padding-box" | "content-box") {
            clipped_fill = Some((color, clip == "content-box"));
        } else if hit.is_none() {
            el = el.bg(color);
        }
    }
    let overflow = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y].map(|prop| {
        style.get(prop).unwrap_or(if node.tag == "textarea" {
            "auto"
        } else {
            "visible"
        })
    });
    let overflow = if node.tag == "body"
        && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
            .into_iter()
            .all(|prop| {
                reader
                    .parent(id, prop)
                    .is_none_or(|value| value == "visible")
            })
        && body_overflow_moves(doc, styles, id)
    {
        ["visible"; 2]
    } else {
        overflow
    };
    let page = matches!(node.tag.as_str(), "html" | "body");
    // `content-visibility` contains the paint of the contents, like `overflow: clip`.
    let overflow = if info[id].flags & cull::SKIPS != 0
        && !page
        && style
            .get(Prop::CONTENT_VISIBILITY)
            .is_some_and(|v| v != "visible")
    {
        overflow.map(|value| if value == "visible" { "clip" } else { value })
    } else {
        overflow
    };
    let boxed = !page
        && overflow
            .iter()
            .any(|value| !matches!(*value, "visible" | "clip"));
    let overflow = overflow.map(|value| match value {
        "auto" | "scroll" => renderer::Overflow::Scroll,
        "hidden" => renderer::Overflow::Hidden,
        "clip" if boxed => renderer::Overflow::Hidden,
        "clip" => renderer::Overflow::Clip,
        _ if boxed => renderer::Overflow::Scroll,
        _ => renderer::Overflow::Visible,
    });
    if hit.is_none() && overflow != [renderer::Overflow::Visible; 2] && !editable {
        el.style().overflow.x = overflow[0];
        el.style().overflow.y = overflow[1];
    }
    let scrolls = overflow.map(|axis| axis == renderer::Overflow::Scroll);
    let look = if scrolls.contains(&true) {
        let backdrop = backdrop(doc, styles, id);
        reader.log(
            Query::Backdrop(id as u32),
            backdrop.map_or(1, |(provider, _)| reader.scope_of(id, provider)),
            hashed(backdrop),
        );
        reader.log(Query::Theme, 0, hashed(crate::native::theme_state() & 1));
        scrollbar::Look::new(
            style,
            styles.scrollbars.get(&id).map(|parts| &**parts),
            &units,
            backdrop.map(|(_, ink)| ink),
        )
        .map(|mut look| {
            if page {
                look.forced = [false; 2];
            }
            look
        })
    } else {
        None
    };
    let reserved = look.as_ref().map_or([0.; 2], |look| {
        // Make the handle before the read, or the read logs none and its replay finds the handle.
        let mut host = view.host.borrow_mut();
        let dom = host.dom.entry(view.id).or_default();
        let handle = dom.scroll.entry(id).or_default();
        reader.log(Query::Scroll(id as u32), 0, hashed(scrolled(Some(handle))));
        look.space(look.shown(scrolls, handle.extent()))
    });
    let mut padding = parsed.padding.unwrap_or_else(|| {
        units
            .edges(style, style::PADDING, containing[0])
            .map(|value| value.max(0.))
    });
    padding[1] += reserved[1];
    padding[2] += reserved[0];
    let opposite = if style
        .get(Prop::SCROLLBAR_GUTTER)
        .is_some_and(|value| value.contains("both-edges"))
    {
        reserved[1]
    } else {
        0.
    };
    padding[3] += opposite;
    if hit.is_none() {
        el = el
            .pt(px(padding[0]))
            .pr(px(padding[1]))
            .pb(px(padding[2]))
            .pl(px(padding[3]));
    }
    let mut margin = margins(
        build,
        id,
        &units,
        containing[0],
        flex_parent,
        slot.is_some(),
    );
    if slot.is_none() {
        reader.log(Query::Margins, depth, hashed(margin.map(f32::to_bits)));
    }
    if hit.is_none() {
        el = el
            .mt(px(margin[0]))
            .mr(px(margin[1]))
            .mb(px(margin[2]))
            .ml(px(margin[3]));
    }
    if node.tag == "legend"
        && let Some(fieldset) = node.parent
        && doc.nodes[fieldset].tag == "fieldset"
        && let Some((legend, height, _)) = legend(doc, styles, fieldset)
        && legend == id
    {
        let left =
            styles.lengths[fieldset].edges(&styles.values[fieldset], style::PADDING, containing[0])
                [3];
        el = el.absolute().top(px(-height)).left(px(left));
    }
    for (side, prop) in style::MARGIN.iter().enumerate() {
        if hit.is_none() && style.get(*prop).is_some_and(|s| s == "auto") {
            el = match side {
                0 => el.mt_auto(),
                1 => el.mr_auto(),
                2 => el.mb_auto(),
                _ => el.ml_auto(),
            };
        }
    }
    let rtl_parent = slot.is_none() && reader.parent(id, Prop::DIRECTION) == Some("rtl");
    if rtl_parent && hit.is_none() {
        el.style().flow |= renderer::Flow::RTL;
    }
    if hit.is_none() && style.get(Prop::DIRECTION) == Some("rtl") {
        el.style().flow |= renderer::Flow::RTL_BLOCK;
    }
    if hit.is_none()
        && !flex_parent
        && rtl_parent
        && style.get(Prop::DISPLAY).is_some_and(|value| {
            matches!(
                value,
                "block" | "list-item" | "table" | "flex" | "grid" | "flow-root"
            )
        })
        && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
        && style
            .get(Prop::POSITION)
            .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
        && style.get(Prop::WIDTH).is_some_and(|value| value != "auto")
        && [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT]
            .into_iter()
            .all(|prop| style.get(prop).is_none_or(|value| value != "auto"))
    {
        el = el.ml_auto();
    }
    let ink = parsed.ink.unwrap_or(Color::BLACK);
    let mut borders = parsed
        .borders
        .unwrap_or_else(|| units.borders(style, containing[0]));
    let sides = style::BORDERS.map(|[width, kind, color]| (width, kind, color));
    let mut border_colors = parsed.border_colors;
    let mut border_kinds = sides.map(|(_, kind, _)| style.get(kind).unwrap_or("none"));
    if inline_box {
        let [above, below] = [padding[0] + borders[0], padding[2] + borders[2]];
        // Like browsers, padding and borders of an inline box hang outside its line, hence the negative
        // margins.
        margin[0] = -above;
        margin[2] = -below;
        if hit.is_none() {
            el = el.mt(px(margin[0])).mb(px(margin[2]));
            // Like Chrome, an empty inline box with inline margins, padding or borders makes a line, so it
            // takes the line height and its baseline.
            if !styles.pseudos.contains_key(&id)
                && empty_inline()
                && !style::makes_no_line(doc, styles, id)
            {
                let [ascent, descent, _] = text
                    .line_metrics(text.face(&parsed.face), px(units.font.em * pixel_ratio))
                    .map(f32::from);
                let lead = ((units.font.lh * pixel_ratio - ascent - descent) / 2.).floor();
                let baseline = (lead + ascent) / pixel_ratio;
                let mut mark = renderer::node()
                    .h(px(baseline))
                    .mb(px(-baseline))
                    .baseline(renderer::Baseline::Bottom);
                mark.style().baseline_source = true;
                el = el.min_h(px(units.font.lh + above + below)).child(mark);
            }
        }
        let same = border_colors.iter().all(|color| *color == border_colors[0]);
        // A border image replaces the normal border.
        let imaged = style
            .get(Prop::BORDER_IMAGE_SOURCE)
            .is_some_and(|value| value != "none");
        let border = if same && !imaged && borders.iter().any(|width| *width > 0.) {
            border_colors[0]
        } else {
            renderer::Color::TRANSPARENT
        };
        if border.a > 0. || inline_fill.a > 0. {
            let font = text.face(&parsed.face);
            let size_px = px(units.font.em);
            let metrics = text.metrics(font);
            let height = metrics.ascent * size_px.0 - metrics.descent * size_px.0;
            let line_height = units.font.lh;
            let radii = super::effects::radii(style, &units, || containing);
            let (sides, pad_y, fill) = (borders, padding[0] + padding[2], inline_fill);
            let dashed = matches!(border_kinds[0], "dashed");
            el = el.child(
                renderer::canvas(
                    |_, _| (),
                    move |bounds, _, window| {
                        let step = px(line_height.max(1.));
                        let mask = window.content_mask().bounds;
                        let quad = size(
                            bounds.size.width + px(sides[1] + sides[3]),
                            px(height + above + below),
                        );
                        let start = bounds.top() + px((line_height - height) / 2. - sides[0]);
                        let lines =
                            (((bounds.size.height - px(pad_y)) / step).round() as usize).max(1);
                        for line in 0..lines {
                            let top = start + step * line as f32;
                            if top > mask.bottom() {
                                window.cull();
                                break;
                            }
                            if top + quad.height < mask.top() {
                                window.cull();
                                continue;
                            }
                            window.draw_box(
                                Rect::new(point(bounds.left() - px(sides[3]), top), quad),
                                radii.resolve(quad),
                                fill,
                                renderer::Border {
                                    widths: renderer::Edges {
                                        top: px(sides[0]),
                                        right: px(sides[1]),
                                        bottom: px(sides[2]),
                                        left: px(sides[3]),
                                    },
                                    color: border,
                                    style: if dashed {
                                        renderer::BorderStyle::Dashed
                                    } else {
                                        renderer::BorderStyle::Solid
                                    },
                                    sides: 0,
                                },
                            );
                        }
                    },
                )
                .absolute()
                .left(px(0.))
                .top(px(0.))
                .size_full(),
            );
            if same {
                border_colors = [renderer::Color::TRANSPARENT; 4];
            }
        }
    }
    let list = (node.tag == "select")
        .then(|| {
            match node
                .attrs
                .get("size")
                .and_then(|size| size.trim().parse::<usize>().ok())
            {
                Some(rows) if rows > 1 => Some(rows),
                _ => node.attrs.contains_key("multiple").then_some(4),
            }
        })
        .flatten();
    let native = super::control::kind(&node.tag, input, list.is_some(), style).map(
        |(control, boxed, plain)| {
            let number = |name: &str| {
                node.attrs
                    .get(name)
                    .and_then(|value| value.trim().parse::<f32>().ok())
                    .filter(|value| value.is_finite())
            };
            let [min, max] = match control {
                super::control::Control::Range => {
                    [number("min").unwrap_or(0.), number("max").unwrap_or(100.)]
                }
                super::control::Control::Meter => {
                    [number("min").unwrap_or(0.), number("max").unwrap_or(1.)]
                }
                _ => [0., number("max").filter(|&max| max > 0.).unwrap_or(1.)],
            };
            let max = max.max(min);
            let value = match control {
                super::control::Control::Range => {
                    let value = node
                        .current_value()
                        .and_then(|value| value.trim().parse::<f32>().ok())
                        .map_or((min + max) / 2., |value| value.clamp(min, max));
                    // Like the value property, the thumb snaps to a step from `min`.
                    match node.attrs.get("step").map(|step| step.trim()) {
                        Some(step) if step.eq_ignore_ascii_case("any") => value,
                        step => {
                            let step = step
                                .and_then(|step| step.parse::<f32>().ok())
                                .filter(|step| *step > 0.)
                                .unwrap_or(1.);
                            let steps = ((value - min) / step).round();
                            min + if min + steps * step > max {
                                steps - 1.
                            } else {
                                steps
                            } * step
                        }
                    }
                }
                _ => number("value").unwrap_or(if control == super::control::Control::Progress {
                    -1.
                } else {
                    0.
                }),
            };
            let mut dark = dark();
            let accent = match control {
                super::control::Control::Meter => {
                    let low = number("low").unwrap_or(min).clamp(min, max);
                    let high = number("high").unwrap_or(max).clamp(low, max);
                    let region = |value: f32| u8::from(value >= low) + u8::from(value > high);
                    let optimum = region(
                        number("optimum")
                            .unwrap_or((min + max) / 2.)
                            .clamp(min, max),
                    );
                    let current = region(value.clamp(min, max));
                    let colors = if dark {
                        [0x74b374, 0xf2c812, 0xe98f6d]
                    } else {
                        [0x107c10, 0xffb900, 0xd83b01]
                    };
                    if optimum == current {
                        colors[0]
                    } else if optimum == 1 || current == 1 {
                        colors[1]
                    } else {
                        colors[2]
                    }
                }
                super::control::Control::Color => node
                    .current_value()
                    .and_then(style::color)
                    .map_or(0, |(color, _)| color),
                _ if node.attrs.contains_key("disabled") => 0x767676,
                _ => match style.get(Prop::ACCENT_COLOR).and_then(style::color) {
                    Some((color, _)) => {
                        dark = super::control::accent_dark(control, node.checked(), color, dark);
                        color
                    }
                    None if dark => 0x99c8ff,
                    None => 0x0075ff,
                },
            };
            super::control::Look {
                control,
                boxed,
                plain,
                checked: node.checked(),
                disabled: node.attrs.contains_key("disabled"),
                hover: node.state & crate::dom::HOVER != 0,
                active: node.state & crate::dom::ACTIVE != 0,
                dark,
                accent,
                ink: style
                    .get(Prop::COLOR)
                    .and_then(style::color)
                    .map_or(0, |(color, _)| color),
                value: if value < min || max <= min && control != super::control::Control::Progress
                {
                    if control == super::control::Control::Progress {
                        -1.
                    } else {
                        0.
                    }
                } else {
                    ((value - min) / (max - min).max(f32::EPSILON)).clamp(0., 1.)
                },
            }
        },
    );
    let native = native.filter(|_| appkit.is_none());
    if native.is_some() {
        reader.log(Query::Look(id as u32), 0, hashed(node.state & LOOK));
    }
    #[cfg(debug_assertions)]
    if let Some(look) = &native {
        reader.extra.set(hashed((
            reader.extra.get(),
            [
                look.hover,
                look.active,
                look.checked,
                look.disabled,
                look.dark,
            ],
            [look.accent, look.ink, look.value.to_bits()],
        )));
    }
    if let Some(look) = &native {
        if look.boxed {
            let (edge, fill) = super::control::box_colors(look);
            for side in 0..4 {
                padding[side] += (borders[side] - 1.).max(0.);
                borders[side] = borders[side].min(1.);
                border_kinds[side] = "solid";
                if let Some(edge) = edge {
                    border_colors[side] = Color::hex(edge << 8 | 0xff);
                }
            }
            if let Some(fill) = fill {
                el = el.bg(Color::hex(fill << 8 | 0xff));
            }
        }
        if let Some(rows) = list
            && style.get(Prop::HEIGHT).is_none_or(|value| value == "auto")
        {
            // Options have `line-height: normal` and `min-height: 1.2em`.
            let normal = text
                .line_metrics(text.face(&parsed.face), px(units.font.em * pixel_ratio))
                .map(f32::from)
                .iter()
                .sum::<f32>()
                / pixel_ratio;
            let row = (normal.max(1.2 * units.font.em) * 64.).round() / 64. + 1.;
            el = el.h(px(rows as f32 * row
                + padding[0]
                + padding[2]
                + borders[0]
                + borders[2]));
        }
        if look.control == super::control::Control::Menu {
            padding[1] += 16.;
            padding[3] += 4.;
            if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
                let (font, size) = (parsed.face.clone(), px(units.font.em));
                let mut widest = 0f32;
                let mut stack = node.children.clone();
                while let Some(child) = stack.pop() {
                    match doc.nodes[child].tag.as_str() {
                        "optgroup" => stack.extend(&doc.nodes[child].children),
                        "option" => {
                            let label = doc
                                .text(child)
                                .split_ascii_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ");
                            let run = renderer::TextRun {
                                len: label.len(),
                                font: font.clone(),
                                color: renderer::Color::BLACK,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                                overline: None,
                                letter_spacing: px(0.),
                                word_spacing: px(0.),
                                font_size: None,
                                inset: Default::default(),
                                nowrap: false,
                                tab_size: 8.,
                                stroke: None,
                            };
                            widest = widest
                                .max(f32::from(text.shape_line(&label, size, &run).line.width));
                        }
                        _ => {}
                    }
                }
                el = el.min_w(px((widest * pixel_ratio).ceil() / pixel_ratio
                    + padding[1]
                    + padding[3]
                    + borders[1]
                    + borders[3]));
            }
            if !node.attrs.contains_key("multiple")
                && node
                    .attrs
                    .get("size")
                    .is_none_or(|size| size.trim().parse::<u32>().is_ok_and(|size| size <= 1))
                && style.get(Prop::HEIGHT).is_none_or(|value| value == "auto")
            {
                let face = text.face(&parsed.face);
                let [ascent, descent, _] = text
                    .line_metrics(face, px(units.font.em * pixel_ratio))
                    .map(f32::from);
                let height = px((ascent + descent) / pixel_ratio
                    + 2.
                    + padding[0]
                    + padding[2]
                    + borders[0]
                    + borders[2]);
                el = if stretched(1) {
                    el.min_h(height)
                } else {
                    el.h(height)
                }
                .justify_center();
            }
        }
        el = el
            .pt(px(padding[0]))
            .pr(px(padding[1]))
            .pb(px(padding[2]))
            .pl(px(padding[3]));
    }
    let legend = (node.tag == "fieldset")
        .then(|| legend(doc, styles, id))
        .flatten()
        .filter(|&(_, height, _)| height > borders[0]);
    if let Some((legend, height, middle)) = legend {
        el = el.child(fieldset_border(
            legend,
            height,
            middle,
            borders,
            border_colors,
            border_kinds,
            view.clone(),
        ));
        borders[0] = height;
        border_colors = [renderer::Color::TRANSPARENT; 4];
        border_kinds = ["solid"; 4];
    }
    let border_image = hit
        .is_none()
        .then(|| super::background::border_image(style, units, id, 0, borders, view))
        .flatten();
    if border_image.is_some() {
        border_colors = [renderer::Color::TRANSPARENT; 4];
    }
    if hit.is_none() {
        el = el
            .border_t(px(borders[0]))
            .border_r(px(borders[1]))
            .border_b(px(borders[2]))
            .border_l(px(borders[3]));
    }
    let visible = (0..4).filter(|&side| borders[side] > 0.);
    let first = visible.clone().next();
    // Borders draw as their own child when the box can't draw them: shaded or mixed styles, or image layers
    // that reach under the border and must paint below it.
    let shaded = visible.clone().any(|side| {
        matches!(border_kinds[side], "groove" | "ridge" | "inset" | "outset")
            || first.is_some_and(|first| border_kinds[side] != border_kinds[first])
    }) || first.is_some()
        && style
            .get(Prop::BACKGROUND_IMAGE)
            .is_some_and(|value| value != "none")
        && style::components(
            style
                .get(Prop::BACKGROUND_CLIP)
                .filter(|clip| !clip.is_empty())
                .unwrap_or("border-box"),
            ',',
        )
        .iter()
        .any(|clip| !matches!(clip.trim(), "padding-box" | "content-box"));
    if hit.is_none()
        && let Some(first) = first
    {
        if !shaded
            && visible
                .clone()
                .all(|side| border_colors[side] == border_colors[first])
        {
            el = el.border_color(border_colors[first]);
            el.style().border_style = border_style(border_kinds[first]);
        } else {
            el = el.border_color(renderer::Color::TRANSPARENT);
        }
    }
    let extra = if style
        .get(Prop::BOX_SIZING)
        .is_some_and(|value| value == "border-box")
    {
        [0.; 2]
    } else {
        [
            padding[1] + padding[3] + borders[1] + borders[3] - reserved[1] - opposite,
            padding[0] + padding[2] + borders[0] + borders[2] - reserved[0],
        ]
    };
    let floor = |axis: usize, size: f32| {
        let [min, max] = [
            [Prop::MIN_WIDTH, Prop::MAX_WIDTH],
            [Prop::MIN_HEIGHT, Prop::MAX_HEIGHT],
        ][axis];
        if !flex_parent
            || style.get(min).is_some_and(|value| value != "auto")
            || reader
                .parent(id, Prop::FLEX_DIRECTION)
                .is_some_and(|value| value.starts_with("column"))
                != (axis == 1)
        {
            return None;
        }
        match style.get(max).filter(|value| *value != "none") {
            Some(value) if value.contains('%') => None,
            Some(value) => units
                .resolve(value, containing[axis])
                .map(|max| size.min(max + extra[axis])),
            None => Some(size),
        }
    };
    if editable {
        let chrome = [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ];
        let auto =
            [Prop::WIDTH, Prop::HEIGHT].map(|prop| style.get(prop).is_none_or(|v| v == "auto"));
        let percent = style.get(Prop::WIDTH).is_some_and(|v| v.ends_with('%'));
        let [width, height] = field_size(
            doc,
            id,
            style,
            &units,
            text,
            pixel_ratio,
            auto[0] || percent,
            None,
        );
        if auto[0] {
            if !stretched(0)
                && !(matches!(position, "absolute" | "fixed")
                    && [Prop::LEFT, Prop::RIGHT]
                        .iter()
                        .all(|&prop| style.get(prop).is_some_and(|value| value != "auto")))
            {
                el = el.w(px(width + chrome[0]));
            }
            if let Some(min) = floor(0, width + chrome[0]).filter(|_| node.tag != "textarea") {
                el = el.min_w(px(min));
            }
        } else if percent {
            el = el.child(super::input::natural_width(px(width)));
        }
        if auto[1] && (node.tag != "textarea" || style.get(Prop::FIELD_SIZING) != Some("content")) {
            el = el.min_h(px(height + chrome[1]));
        }
    }
    let flexbox = matches!(display, "flex" | "inline-flex");
    let basis = || {
        [Prop::WIDTH, Prop::HEIGHT].map(|prop| {
            let base = containing[usize::from(prop == Prop::HEIGHT)];
            style
                .get(prop)
                .and_then(|length| units.resolve(length, base))
                .unwrap_or(base)
        })
    };
    let radius = parsed
        .radii
        .unwrap_or_else(|| super::effects::radii(style, &units, basis));
    if hit.is_none() {
        el.style().corner_radii = radius;
    }
    if native.as_ref().is_some_and(|look| look.boxed) {
        el.style().corner_radii = renderer::Corners::all(px(2.).into()).into();
    }
    let mut inner_shadows = Vec::new();
    let mut intrinsic_width = None;
    let column_stretched = || {
        slot.is_some()
            && style
                .get(Prop::JUSTIFY_SELF)
                .filter(|value| *value != "auto")
                .or_else(|| reader.parent(id, Prop::JUSTIFY_ITEMS))
                .is_none_or(|value| matches!(value, "normal" | "stretch"))
            || flex_parent
                && reader
                    .parent(id, Prop::FLEX_DIRECTION)
                    .is_some_and(|value| value.starts_with("column"))
                && style
                    .get(Prop::ALIGN_SELF)
                    .filter(|value| *value != "auto")
                    .or_else(|| reader.parent(id, Prop::ALIGN_ITEMS))
                    .is_none_or(|value| matches!(value, "normal" | "stretch"))
    };
    if hit.is_some()
        && let Some(value) = style.get(Prop::BOX_SHADOW)
    {
        inner_shadows = units.shadows(value, true);
    }
    // The left and right insets of a positioned box take room from its width.
    let inset_x = [Prop::LEFT, Prop::RIGHT].map(|prop| {
        style
            .get(prop)
            .filter(|_| matches!(position, "absolute" | "fixed"))
            .and_then(|v| units.resolve(v, containing[0]))
    });
    let percentage = |value: &str| {
        value
            .strip_suffix('%')
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
    };
    // Like Chrome, a box in a line takes percent heights from the height of its block: `Some(true)` when it is a
    // fixed length, and `Some(false)` when it is `auto`, which makes them `auto` too.
    let line_base = (display.starts_with("inline-")
        && [Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT]
            .iter()
            .any(|&prop| style.get(prop).is_some_and(|value| value.contains('%')))
        && !flex_parent
        && slot.is_none()
        && !matches!(position, "absolute" | "fixed"))
    .then(|| {
        let definite = definite_line(doc, styles, id);
        reader.log(Query::LineBase(id as u32), 0, hashed(definite));
        definite
    });
    // The content of a details box is in a slot of auto height, so its percent heights and offsets are `auto`.
    let in_details = !flex_parent
        && slot.is_none()
        && node.tag != "summary"
        && !matches!(position, "absolute" | "fixed")
        && parent.is_some_and(|parent| doc.nodes[parent].tag == "details");
    // The initial `start` follows the direction of the box.
    if hit.is_none()
        && style.get(Prop::TEXT_ALIGN).is_none()
        && rtl != (reader.parent(id, Prop::DIRECTION) == Some("rtl"))
    {
        el = if rtl { el.text_right() } else { el.text_left() };
    }
    for (prop, value) in style.iter().filter(|_| hit.is_none()) {
        let value: &str = value;
        let vertical = matches!(
            prop,
            Prop::HEIGHT | Prop::MIN_HEIGHT | Prop::MAX_HEIGHT | Prop::TOP | Prop::BOTTOM
        );
        if in_details && vertical && value.contains('%') {
            continue;
        }
        let base = containing[usize::from(vertical)];
        let length = || units.resolve(value, base);
        // The root element takes percent heights from the viewport, which its parent box does not fix. Like
        // Chrome, an inline-level box takes them from the fixed height of its block, which its line doesn't have.
        // A grid item takes them from its grid area.
        let page_size = || match vertical && parent == Some(0) {
            true => viewport(),
            false => (vertical && line_base == Some(true)
                || slot.is_some() && !style::table_part(doc, styles, id))
            .then_some(containing),
        };
        match prop {
            Prop::WIDTH | Prop::HEIGHT if sizeless => {}
            Prop::WIDTH
            | Prop::HEIGHT
            | Prop::MIN_WIDTH
            | Prop::MAX_WIDTH
            | Prop::MIN_HEIGHT
            | Prop::MAX_HEIGHT => {
                if vertical && line_base == Some(false) && value.contains('%') {
                    continue;
                }
                let axis = usize::from(vertical);
                let length: renderer::Length = if let Some(percent) = percentage(value) {
                    match block_size.or_else(page_size) {
                        Some(_) => px(percent.max(0.) / 100. * base + extra[axis]).into(),
                        None => renderer::Length::Calc(percent.max(0.) / 100., px(extra[axis])),
                    }
                } else if let Some((fraction, offset)) =
                    percent_calc(&units, value).filter(|_| block_size.is_none())
                {
                    match page_size() {
                        Some(_) => px(fraction * base + offset + extra[axis]).into(),
                        None => renderer::Length::Calc(fraction, px(offset + extra[axis])),
                    }
                } else if let Some(length) = (block_size.is_none() && page_size().is_none())
                    .then(|| percent_bound(&units, value, extra[axis], el.style()))
                    .flatten()
                {
                    length
                } else if let Some(length) = length() {
                    px(length.max(0.) + extra[axis]).into()
                } else if matches!(prop, Prop::WIDTH | Prop::HEIGHT)
                    && value == "auto"
                    && node.tag != "svg"
                    && !(editable && prop == Prop::WIDTH)
                    || prop == Prop::WIDTH
                        && style::table_box(style::role(styles, id))
                        && value == "fit-content"
                        && column_stretched()
                    || prop == Prop::WIDTH
                        && value == "fit-content"
                        && matches!(position, "absolute" | "fixed")
                        && inset_x.contains(&None)
                {
                    renderer::Length::Auto
                } else if prop == Prop::WIDTH
                    && style::table_box(style::role(styles, id))
                    && value == "fit-content"
                {
                    // The grid's measure child gives the content sizes, and these bounds clamp a stretched width.
                    let [min, max, ..] = super::grid::intrinsic(
                        doc,
                        id,
                        styles,
                        None,
                        None,
                        text,
                        &mut measured.borrow_mut(),
                    );
                    let [min, max] = [min, max].map(|size| {
                        layout_ceil((size - margin[1] - margin[3]).max(0.), pixel_ratio)
                    });
                    intrinsic_width = Some(max.min(min.max(containing[0] - margin[1] - margin[3])));
                    if style.get(Prop::MIN_WIDTH).is_none() && style::role(styles, id) != 3 {
                        el = el.min_w(px(min));
                    }
                    if style.get(Prop::MAX_WIDTH).is_none() {
                        el = el.max_w(px(max));
                    }
                    renderer::Length::Auto
                } else if matches!(value, "stretch" | "-webkit-fill-available") {
                    relative(1.)
                } else if matches!(value, "fit-content" | "max-content" | "min-content")
                    && axis == 0
                {
                    let [min, max, ..] = super::grid::intrinsic(
                        doc,
                        id,
                        styles,
                        Some(containing[0]),
                        None,
                        text,
                        &mut measured.borrow_mut(),
                    );
                    let size = match value {
                        "min-content" => min,
                        "max-content" => max,
                        _ => {
                            max.min(min.max(containing[0] - inset_x.iter().flatten().sum::<f32>()))
                        }
                    };
                    let size = layout_ceil((size - margin[1] - margin[3]).max(0.), pixel_ratio);
                    intrinsic_width = Some(size);
                    if value == "fit-content" && style.get(Prop::MAX_WIDTH).is_none() {
                        el = el.max_w(renderer::Length::Calc(1., px(-margin[1] - margin[3])));
                    }
                    px(size).into()
                } else {
                    continue;
                };
                el = match prop {
                    Prop::WIDTH => el.w(length),
                    Prop::HEIGHT => el.h(length),
                    Prop::MIN_WIDTH => el.min_w(length),
                    Prop::MAX_WIDTH => el.max_w(length),
                    // A field's rows give its auto height, which an author min-height only raises.
                    Prop::MIN_HEIGHT => match (el.style().min_size.height, length) {
                        (renderer::Length::Px(rows), renderer::Length::Px(min)) if editable => {
                            el.min_h(rows.max(min))
                        }
                        _ => el.min_h(length),
                    },
                    _ => el.max_h(length),
                };
            }
            Prop::ROW_GAP | Prop::COLUMN_GAP if flexbox => {
                let Some(length) = length() else { continue };
                el = if prop == Prop::ROW_GAP {
                    el.gap_y(px(length.max(0.)))
                } else {
                    el.gap_x(px(length.max(0.)))
                };
            }
            Prop::BOX_SHADOW => {
                el.style().box_shadow = units.shadows(value, false);
                inner_shadows = units.shadows(value, true);
            }
            Prop::ASPECT_RATIO => {
                if !(value.contains("auto") && natural_ratio.is_some())
                    && let Some(ratio) = aspect_ratio(value)
                {
                    el.style().aspect_ratio = Some(ratio);
                    el = only_auto_ratio(el, style);
                }
            }
            Prop::DISPLAY if matches!(value, "grid" | "inline-grid") => el = el.grid(),
            Prop::DISPLAY if matches!(value, "flex" | "inline-flex") => {
                el = el.flex().flex_row();
                if rtl {
                    el.style().flex_direction = renderer::FlexDirection::RowReverse;
                }
            }
            Prop::FLEX_DIRECTION if flexbox => {
                if rtl && value.starts_with("column") {
                    el.style().flow |= renderer::Flow::RTL_ITEMS;
                }
                el.style().flex_direction = match (value, rtl) {
                    ("row-reverse", false) | ("row", true) => renderer::FlexDirection::RowReverse,
                    ("column", _) => renderer::FlexDirection::Column,
                    ("column-reverse", _) => renderer::FlexDirection::ColumnReverse,
                    _ => renderer::FlexDirection::Row,
                }
            }
            Prop::FLEX_WRAP if flexbox => {
                el.style().flex_wrap = match value {
                    "wrap" => renderer::FlexWrap::Wrap,
                    "wrap-reverse" => renderer::FlexWrap::WrapReverse,
                    _ => renderer::FlexWrap::NoWrap,
                }
            }
            Prop::FLEX_GROW | Prop::FLEX_SHRINK if flex_parent => {
                if let Some(factor) = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.)
                {
                    if prop == Prop::FLEX_GROW {
                        el.style().flex_grow = factor;
                    } else {
                        el.style().flex_shrink = factor;
                    }
                }
            }
            Prop::FLEX_BASIS if flex_parent => {
                let axis = usize::from(
                    reader
                        .parent(id, Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v.starts_with("column")),
                );
                let base = containing[axis];
                el.style().flex_basis = if let Some(percent) = percentage(value) {
                    renderer::Length::Calc(percent.max(0.) / 100., px(extra[axis]))
                } else if let Some(length) = units.resolve(value, base) {
                    px(length.max(0.) + extra[axis]).into()
                } else {
                    renderer::Length::Auto
                };
            }
            Prop::ALIGN_ITEMS | Prop::ALIGN_SELF if flex_parent || flexbox => {
                let align = match value {
                    "center" => renderer::AlignItems::Center,
                    "flex-start" | "start" | "self-start" => renderer::AlignItems::FlexStart,
                    "flex-end" | "end" | "self-end" => renderer::AlignItems::FlexEnd,
                    "baseline" | "first baseline" => renderer::AlignItems::Baseline,
                    "last baseline" => renderer::AlignItems::LastBaseline,
                    "stretch" | "normal" => renderer::AlignItems::Stretch,
                    _ => continue,
                };
                if prop == Prop::ALIGN_SELF {
                    el.style().align_self = Some(align);
                } else if flexbox {
                    el.style().align_items = Some(align);
                }
            }
            Prop::ALIGN_CONTENT | Prop::JUSTIFY_CONTENT if flexbox => {
                let inline = prop == Prop::JUSTIFY_CONTENT
                    && !style
                        .get(Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v.starts_with("column"));
                let align = match value {
                    "center" => renderer::AlignContent::Center,
                    "flex-end" => renderer::AlignContent::FlexEnd,
                    // Taffy's Start/End ignore flex reversal. RTL rows use RowReverse. In columns, left/right
                    // behave as start.
                    "start" | "end" | "left" | "right" => {
                        if match value {
                            "left" => false,
                            "right" => inline,
                            _ => (value == "end") != (inline && rtl),
                        } {
                            renderer::AlignContent::End
                        } else {
                            renderer::AlignContent::Start
                        }
                    }
                    "space-between" => renderer::AlignContent::SpaceBetween,
                    "space-around" => renderer::AlignContent::SpaceAround,
                    "space-evenly" => renderer::AlignContent::SpaceEvenly,
                    "stretch" => renderer::AlignContent::Stretch,
                    _ => renderer::AlignContent::FlexStart,
                };
                if prop == Prop::JUSTIFY_CONTENT {
                    el.style().justify_content = Some(align);
                } else {
                    el.style().align_content = Some(align);
                }
            }
            Prop::TEXT_ALIGN => {
                el = match (value, rtl) {
                    ("center" | "-webkit-center", _) => el.text_center(),
                    ("right" | "-webkit-right", _)
                    | ("end", false)
                    | ("start" | "justify", true) => el.text_right(),
                    _ => el.text_left(),
                }
            }
            Prop::POSITION if matches!(value, "absolute" | "fixed") => el = el.absolute(),
            Prop::LEFT | Prop::RIGHT | Prop::TOP | Prop::BOTTOM
                if matches!(position, "relative" | "absolute") =>
            {
                // Like Chrome, a percent offset of a relative box against a parent of auto height is `auto`.
                let fraction = !(position == "relative"
                    && vertical
                    && slot.is_none()
                    && reader.parent(id, Prop::HEIGHT).is_none_or(|height| {
                        matches!(
                            height,
                            "auto" | "fit-content" | "min-content" | "max-content"
                        ) || height.contains('%')
                            && parent.is_none_or(|parent| doc.nodes[parent].parent != Some(0))
                    }));
                let length: renderer::Length = if let Some(percent) = percentage(value) {
                    if !fraction {
                        continue;
                    }
                    match block_size {
                        Some(_) => px(percent / 100. * base).into(),
                        None => relative(percent / 100.),
                    }
                } else if let Some((share, offset)) =
                    percent_calc(&units, value).filter(|_| block_size.is_none())
                {
                    if !fraction {
                        continue;
                    }
                    renderer::Length::Calc(share, px(offset))
                } else if let Some(length) = length() {
                    px(length).into()
                } else {
                    continue;
                };
                el = match prop {
                    Prop::LEFT => el.left(length),
                    Prop::RIGHT => el.right(length),
                    Prop::TOP => el.top(length),
                    _ => el.bottom(length),
                };
            }
            // An empty cell draws no background and borders.
            Prop::EMPTY_CELLS
                if value == "hide"
                    && style::role(styles, id) == style::CELL
                    && node.children.iter().all(|&child| {
                        doc.nodes[child].tag == "#text"
                            && doc.nodes[child].text.trim_ascii().is_empty()
                    }) =>
            {
                el.style().visibility = renderer::Visibility::Hidden
            }
            Prop::VISIBILITY => {
                el.style().visibility = if value == "hidden" || value == "collapse" {
                    renderer::Visibility::Hidden
                } else {
                    renderer::Visibility::Visible
                }
            }
            Prop::OPACITY if effect_layer.is_none() => el = el.opacity(opacity),
            Prop::WHITE_SPACE if matches!(value, "nowrap" | "pre") => {
                el = el.whitespace_nowrap().flex_nowrap()
            }
            Prop::TEXT_WRAP if value.contains("nowrap") => {
                el = el.whitespace_nowrap().flex_nowrap()
            }
            Prop::WORD_BREAK | Prop::OVERFLOW_WRAP => {
                el.text_style().set_word_break(style::word_break(style));
            }
            Prop::TEXT_WRAP => {
                el.text_style().set_text_wrap(if value.contains("balance") {
                    renderer::TextWrap::Balance
                } else if value.contains("pretty") {
                    renderer::TextWrap::Pretty
                } else {
                    renderer::TextWrap::Wrap
                });
            }
            Prop::CURSOR => {
                if let Some(cursor) = style::cursor(value) {
                    el = el.cursor(cursor);
                }
            }
            _ => {}
        }
    }
    if let Some(natural) = natural {
        let chrome = [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ];
        let props = [Prop::WIDTH, Prop::HEIGHT];
        let hint = [0, 1].map(|axis| {
            node.attrs
                .get(["width", "height"][axis])
                .filter(|_| {
                    style.get(props[axis]).is_none()
                        && matches!(node.tag.as_str(), "img" | "video")
                        && !(broken && natural == [16., 16.])
                })
                .and_then(|value| dimension(value, false))
                .map(|length| length + extra[axis])
        });
        if let Some(width) = hint[0] {
            el = el.w(px(width));
        }
        if let Some(height) = hint[1] {
            el = el.h(px(height));
        }
        let set = [0, 1].map(|axis| {
            hint[axis].is_some()
                || style.get(props[axis]).is_some_and(|value| {
                    value != "auto"
                        && !(axis == 1 && line_base == Some(false) && value.contains('%'))
                })
        });
        let [content_width, content_height] = [0, 1].map(|axis| {
            hint[axis]
                .or_else(|| {
                    style
                        .get(props[axis])
                        .filter(|value| !value.contains('%'))
                        .and_then(|value| units.resolve(value, containing[axis]))
                        .map(|length| length + extra[axis])
                })
                .map(|length| (length - chrome[axis]).max(0.))
        });
        // A flex item's automatic minimum is the smaller of its specified and natural widths.
        if flex_parent
            && style
                .get(Prop::MIN_WIDTH)
                .is_none_or(|value| value == "auto")
            && reader
                .parent(id, Prop::FLEX_DIRECTION)
                .is_none_or(|value| !value.starts_with("column"))
            && let Some(percent) = style
                .get(Prop::WIDTH)
                .and_then(|value| value.strip_suffix('%'))
                .and_then(|value| value.parse::<f32>().ok())
        {
            el = el.min_w(px(
                (percent.max(0.) / 100. * containing[0] + extra[0]).min(natural[0] + chrome[0])
            ));
        }
        let stretch = [stretched(0), stretched(1)];
        let css = el.style().aspect_ratio.is_some();
        let line = (stretch[1]
            && reader
                .parent(id, Prop::FLEX_WRAP)
                .is_none_or(|value| value == "nowrap")
            && reader
                .parent(id, Prop::HEIGHT)
                .is_some_and(|value| value != "auto" && !value.contains('%')))
        .then(|| (containing[1] - margin[0] - margin[2] - chrome[1]).max(0.));
        let sized = !set[0] && !stretch[0] && !(css && set[1]);
        let width = if set[0] {
            content_width
        } else if !sized {
            None
        } else {
            match natural_ratio.filter(|_| !css) {
                Some(ratio) if set[1] => content_height.map(|height| height * ratio),
                Some(ratio) => Some(line.map_or(natural[0], |line| line * ratio)),
                None => Some(natural[0]),
            }
        };
        // Like Chrome, a video without a frame has no natural size when CSS gives it a ratio, so it fills the line.
        if sized
            && css
            && natural_ratio.is_none()
            && node.tag == "video"
            && !matches!(position, "absolute" | "fixed")
        {
            match el.style().aspect_ratio {
                Some(ratio) => {
                    // A box around it that shrinks to its content makes it as narrow as its frame.
                    let shrinks = shrinks_around(doc, styles, id);
                    reader.log(Query::Shrinks(id as u32), 0, hashed(shrinks));
                    let limit = |prop| {
                        style
                            .get(prop)
                            .and_then(|value| units.resolve(value, containing[0]))
                            .map(|value| value + extra[0])
                    };
                    let width = if shrinks {
                        chrome[0]
                    } else {
                        containing[0] - margin[1] - margin[3]
                    }
                    .min(limit(Prop::MAX_WIDTH).unwrap_or(f32::INFINITY))
                    .max(limit(Prop::MIN_WIDTH).unwrap_or(0.));
                    // The ratio of the width and height attributes is for the content box.
                    let height = if style.get(Prop::BOX_SIZING) == Some("border-box")
                        && style
                            .get(Prop::ASPECT_RATIO)
                            .is_some_and(|v| !v.contains("auto"))
                    {
                        width / ratio
                    } else {
                        (width - chrome[0]).max(0.) / ratio + chrome[1]
                    };
                    el = el.w(px(width)).h(px(height));
                }
                None => el = el.w(renderer::Length::Calc(1., px(-margin[1] - margin[3]))),
            }
        } else if sized {
            if let Some(width) = width {
                el = el.w(px(width + chrome[0]));
                if let Some(min) = floor(0, width + chrome[0]) {
                    el = el.min_w(px(min));
                }
            } else {
                el.style().aspect_ratio = natural_ratio;
                el = only_auto_ratio(el, style);
            }
        }
        if let Some(height) = match natural_ratio {
            _ if set[1] || line.is_some() || css => None,
            None => Some(natural[1]),
            Some(ratio) if matches!(position, "absolute" | "fixed") => {
                width.map(|width| width / ratio)
            }
            Some(_) => None,
        } {
            let height = height + chrome[1];
            if stretch[1]
                && style
                    .get(Prop::MIN_HEIGHT)
                    .is_none_or(|value| value == "auto")
            {
                el = el.min_h(px(height));
            } else {
                el = el.h(px(height));
                if let Some(min) = floor(1, height) {
                    el = el.min_h(px(min));
                }
            }
        }
    }
    if block_size.is_some() && natural.is_none() {
        for (axis, size, sides) in [(0, Prop::WIDTH, [3, 1]), (1, Prop::HEIGHT, [0, 2])] {
            let [start, end] = sides.map(|side| {
                style
                    .get([Prop::TOP, Prop::RIGHT, Prop::BOTTOM, Prop::LEFT][side])
                    .and_then(|value| units.resolve(value, containing[axis]))
            });
            if style.get(size).is_none_or(|value| value == "auto")
                && let (Some(start), Some(end)) = (start, end)
            {
                let length =
                    px(
                        (containing[axis] - start - end - margin[sides[0]] - margin[sides[1]])
                            .max(0.),
                    );
                el = if axis == 0 {
                    el.w(length)
                } else {
                    el.h(length)
                };
            }
        }
    }
    let replaced = natural.is_some()
        || matches!(
            node.tag.as_str(),
            "svg" | "input" | "button" | "select" | "textarea" | "progress" | "meter"
        );
    let auto = [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT]
        .map(|prop| style.get(prop).is_some_and(|value| value == "auto"));
    if hit.is_none()
        && auto.contains(&true)
        && !flex_parent
        && slot.is_none()
        && matches!(
            display,
            "block" | "flex" | "grid" | "list-item" | "flow-root"
        )
        && style.get(Prop::WIDTH).is_none_or(|value| value == "auto")
        && !matches!(position, "absolute" | "fixed")
        && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
        && !replaced
    {
        if auto == [true, true] || margin[1] + margin[3] == 0. {
            el = el.w(relative(1.));
        } else {
            let style = el.style();
            (style.margin.left, style.margin.right) = (px(margin[3]).into(), px(margin[1]).into());
        }
    }
    if flex_parent
        && matches!(
            node.tag.as_str(),
            "img" | "video" | "canvas" | "iframe" | "svg" | "embed" | "object"
        )
        && [Prop::WIDTH, Prop::MAX_WIDTH]
            .into_iter()
            .any(|prop| style.get(prop).is_some_and(|value| value.contains('%')))
        && style
            .get(Prop::FLEX_BASIS)
            .is_none_or(|value| value == "auto")
        && node.parent.is_some()
        && reader
            .parent(id, Prop::FLEX_DIRECTION)
            .is_none_or(|value| value.starts_with("row"))
        && let width @ (renderer::Length::Px(_) | renderer::Length::Calc(..)) =
            el.style().size.width
    {
        el.style().flex_basis = width;
        el.style().size.width = renderer::Length::Auto;
    }
    if hit.is_none()
        && !flex_parent
        && slot.is_none()
        && !matches!(position, "absolute" | "fixed")
        && display.starts_with("inline")
        && let Some(parent) = node.parent
    {
        if display != "inline"
            && el.style().size.width == renderer::Length::Auto
            && (!replaced || node.tag == "button")
            && float_side(style).is_none()
        {
            el.style().flex_shrink = 1.;
        }
        reader.log(
            Query::ParentFont,
            depth,
            hashed(parent_font(doc, styles, id)),
        );
        let align = style.get(Prop::VERTICAL_ALIGN).unwrap_or("baseline");
        let middle = match el.style().size.height {
            _ if align != "middle" => None,
            _ if display == "inline" => Some(units.font.lh),
            renderer::Length::Px(height) => Some(f32::from(height) + margin[0] - margin[2]),
            _ => None,
        };
        let sized = matches!(el.style().size.height, renderer::Length::Px(_));
        // Like Chrome, a middle box puts the middle of its margin box half an x-height above the baseline.
        let centered = align == "middle" && middle.is_none();
        el.style().align_self = match align {
            "top" => Some(renderer::AlignItems::FlexStart),
            "text-top" if !sized && display != "inline" => Some(renderer::AlignItems::FlexStart),
            "bottom" => Some(renderer::AlignItems::FlexEnd),
            _ => None,
        };
        let below = |style: &style::Style, font: style::FontUnits| {
            let face = text.face(&style::font(style));
            font.lh - f32::from(text.baseline(face, px(font.em), px(font.lh), pixel_ratio))
        };
        let line = styles.lengths[parent].font;
        let replaced = matches!(
            node.tag.as_str(),
            "img"
                | "svg"
                | "canvas"
                | "video"
                | "audio"
                | "iframe"
                | "embed"
                | "object"
                | "progress"
                | "meter"
                | "textarea"
        ) || check
            || display == "inline-block"
                && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                    .into_iter()
                    .any(|prop| style.get(prop).is_some_and(|value| value != "visible"))
            || display != "inline"
                && !node.tag.starts_with('#')
                && !input.is_some_and(|kind| {
                    ["button", "submit", "reset", "file"]
                        .iter()
                        .any(|other| kind.eq_ignore_ascii_case(other))
                })
                && blank();
        let replaced = replaced && alt.is_none();
        let field = editable && node.tag == "input";
        let swatch = native
            .as_ref()
            .is_some_and(|look| look.control == super::control::Control::Color);
        let parent_style = &styles.values[parent];
        let parent = below(parent_style, line);
        let edge = el.style().align_self.is_none()
            && !centered
            && (matches!(align, "text-top" | "text-bottom")
                || middle.is_some()
                || swatch
                || replaced && !field
                || display == "inline-grid"
                    && !node.children.iter().any(|&child| in_flow(doc, info, child)));
        if centered {
            el.style().baseline = renderer::Baseline::Middle;
        } else if edge {
            el.style().baseline = renderer::Baseline::None;
        }
        let lift = if centered {
            line.ex / 2. + (margin[2] - margin[0]) / 2.
        } else if !edge {
            0.
        } else if matches!(align, "text-top" | "text-bottom") {
            let face = text.face(&style::font(parent_style));
            let [ascent, descent, _] = text
                .line_metrics(face, px(line.em * pixel_ratio))
                .map(|value| f32::from(value) / pixel_ratio);
            // The inline box of a span is as tall as its line height.
            match el.style().size.height {
                renderer::Length::Px(height) if align == "text-top" => {
                    ascent - margin[0] - f32::from(height)
                }
                _ if align == "text-top" => ascent - units.font.lh,
                _ => margin[2] - descent,
            }
        } else if let Some(height) = middle {
            line.ex / 2. - height / 2.
        } else if swatch {
            -padding[2] - borders[2] - 4.
        } else if node.tag == "button"
            && node.children.iter().all(|&child| {
                doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty()
            })
        {
            -padding[2] - borders[2]
        } else if native.as_ref().is_some_and(|look| {
            // Like Chrome, a range sits on the bottom of its border box, a progress bar without its native look
            // on the bottom of its margin box.
            !look.plain || look.control == super::control::Control::Range
        }) {
            0.
        } else {
            margin[2]
        };
        let inset = |prop| style.get(prop).filter(|value| *value != "auto");
        let offset = match (inset(Prop::TOP), inset(Prop::BOTTOM)) {
            _ if position != "relative" => Some(0.),
            (Some(top), _) => units.resolve(top, f32::NAN),
            (None, Some(bottom)) => units.resolve(bottom, f32::NAN).map(|bottom| -bottom),
            (None, None) => Some(0.),
        };
        let raise = match align {
            _ if offset.is_none() => 0.,
            "super" => line.em / 3. + 1. / pixel_ratio + lift,
            "sub" => -line.em / 5. - 1. / pixel_ratio + lift,
            "baseline" => lift,
            value => units.resolve(value, units.font.lh).unwrap_or(0.) + lift,
        };
        // A box with a line of text has its baseline there, also when the text overflows its height.
        let reserve = match (el.style().size.height, el.style().align_self) {
            (renderer::Length::Px(height), None) if display != "inline" && replaced => {
                line.lh - parent - f32::from(height)
            }
            (renderer::Length::Auto, None) if field => {
                line.lh - parent - units.font.lh - padding[0] - borders[0]
                    + below(style, units.font)
            }
            _ => f32::NEG_INFINITY,
        };
        let floor = margin[2].min(0.) + parent
            - match () {
                _ if centered
                    || matches!(display, "inline-block" | "inline-flex" | "inline-grid")
                        && !replaced =>
                {
                    f32::INFINITY
                }
                // A top or bottom box sits on a line edge, so the strut below the baseline does not reach it.
                _ if matches!(
                    el.style().align_self,
                    Some(renderer::AlignItems::FlexStart | renderer::AlignItems::FlexEnd)
                ) =>
                {
                    f32::INFINITY
                }
                _ if edge => 0.,
                _ if display == "inline" => below(style, units.font),
                _ => padding[2] + borders[2] + below(style, units.font),
            };
        if !style::makes_no_line(doc, styles, id)
            && (raise != 0. || reserve > margin[0] || margin[2] < floor)
        {
            el = el
                .mt(px((margin[0] + raise).max(reserve)))
                .mb(px((margin[2] - raise).max(floor)));
        }
        if let Some(offset) = offset
            && (raise != 0. || offset != 0.)
        {
            el = el.top(px(offset - raise));
        }
    }
    if let Some((color, content)) = clipped_fill {
        let inset = std::array::from_fn::<_, 4, _>(|side| {
            px(borders[side] + if content { padding[side] } else { 0. })
        });
        let mut fill = renderer::canvas(
            |_, _| (),
            move |bounds, _, window| {
                let outer = Rect::between(
                    bounds.origin - point(px(borders[3]), px(borders[0])),
                    bounds.bottom_right() + point(px(borders[1]), px(borders[2])),
                );
                let fill = Rect::between(
                    outer.origin + point(inset[3], inset[0]),
                    outer.bottom_right() - point(inset[1], inset[2]),
                );
                let corners = radius
                    .resolve(outer.size)
                    .inset(inset[0], inset[1], inset[2], inset[3])
                    .fit(fill.size);
                window.draw_box(fill, corners, color, renderer::Border::default());
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        if boxed {
            fill.style().escape = renderer::Escape::OUTLINE;
        }
        el = el.child(fill);
    }
    let layers = canvas.map_or(style, |source| &styles.values[source]);
    let mut background = Vec::new();
    if let Some(color) = text_fill {
        background.push(
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| {
                    let outer = Rect::between(
                        bounds.origin - point(px(borders[3]), px(borders[0])),
                        bounds.bottom_right() + point(px(borders[1]), px(borders[2])),
                    );
                    window.draw_box(
                        outer,
                        radius.resolve(outer.size),
                        color,
                        renderer::Border::default(),
                    );
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into(),
        );
    }
    if !propagated
        && layers
            .get(Prop::BACKGROUND_IMAGE)
            .is_some_and(|value| value != "none")
    {
        let base = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|state| state.base.clone());
        background.extend(css_background(
            layers,
            canvas.map_or(units, |source| styles.lengths[source]),
            id,
            0,
            if canvas.is_some() {
                Default::default()
            } else {
                radius
            },
            [borders, padding, [0.; 4]],
            view.clone(),
            base.as_ref(),
            &view.host.borrow().assets,
            canvas.is_some(),
        ));
    }
    // As in Chrome, the background of a scroll container doesn't scroll with the content or add to its scroll
    // range, unless it is `local`.
    if boxed
        && !text_clip
        && !layers
            .get(Prop::BACKGROUND_ATTACHMENT)
            .is_some_and(|value| value.contains("local"))
    {
        for layer in &mut background {
            if let Child::Node(node) = layer {
                node.style().escape = renderer::Escape::OUTLINE;
            }
        }
    }
    if text_clip {
        el = el.child(
            renderer::node()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .children(background)
                .hooks(super::paint::Effected {
                    effect: renderer::PaintEffect {
                        text_mask: true,
                        ..Default::default()
                    },
                    backdrop: None,
                }),
        );
    } else {
        el = el.children(background);
    }
    if !inner_shadows.is_empty() {
        el = el.child(inset_shadows(inner_shadows, radius, borders));
    }
    el = el.children(border_image);
    el = el.children(field_layer);
    el = el.children(content_layers);
    if let Some(fit) = picture {
        let inset = |side: usize| px(borders[side] + padding[side]);
        let corners = radius
            .to_pixels(size(px(basis()[0]), px(basis()[1])))
            .inset(inset(0), inset(1), inset(2), inset(3));
        let cropped = matches!(fit, renderer::ObjectFit::Cover | renderer::ObjectFit::None);
        let mut image = renderer::node().size_full().hooks(super::paint::Picture {
            id,
            view: view.clone(),
            source: super::paint::Source::Element,
            fit,
            cropped: cropped.then_some(corners),
            position: style
                .get(Prop::OBJECT_POSITION)
                .filter(|value| !matches!(*value, "50% 50%" | "center" | "center center"))
                .map(|value| (value.into(), units)),
            pixelated: matches!(
                style.get(Prop::IMAGE_RENDERING),
                Some("pixelated" | "crisp-edges")
            ),
        });
        image.style().aspect_ratio = natural_ratio;
        if !cropped {
            image.style().corner_radii = renderer::Radii {
                x: corners.x.map(renderer::Length::Px),
                y: corners.y.map(renderer::Length::Px),
                shape: corners.shape,
            };
        }
        el = el.child(image);
    }
    if let Some(lines) = style
        .get(Prop::WEBKIT_LINE_CLAMP)
        .or_else(|| style.get(Prop::LINE_CLAMP))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|_| hit.is_none())
    {
        let text = el.text_style();
        text.set_white_space(renderer::WhiteSpace::Normal);
        text.set_line_clamp(Some(lines.max(1)));
        text.set_text_overflow(Some("…".into()));
    } else if hit.is_none()
        && style.get(Prop::TEXT_OVERFLOW) == Some("ellipsis")
        && matches!(style.get(Prop::WHITE_SPACE), Some("nowrap" | "pre"))
        && el.style_ref().overflow.x != renderer::Overflow::Visible
    {
        el.text_style().set_text_overflow(Some("…".into()));
    }
    if position == "fixed" {
        let viewport = units.viewport;
        let inset = [Prop::LEFT, Prop::RIGHT, Prop::TOP, Prop::BOTTOM].map(|prop| {
            style.get(prop).and_then(|v| {
                units.resolve(
                    v,
                    viewport[usize::from(matches!(prop, Prop::TOP | Prop::BOTTOM))],
                )
            })
        });
        if style.get(Prop::WIDTH).is_none_or(|v| v == "auto")
            && let [Some(left), Some(right), ..] = inset
        {
            el = el.w(px((viewport[0] - left - right).max(0.)));
        }
        if style.get(Prop::HEIGHT).is_none_or(|v| v == "auto")
            && let [_, _, Some(top), Some(bottom)] = inset
        {
            el = el.h(px((viewport[1] - top - bottom).max(0.)));
        }
    }
    if style.get(Prop::OUTLINE_STYLE) == Some("auto") {
        // Like Chrome, a focus ring ignores the outline width: a 2px ring of the outline color, centered on the border
        // edge moved out by `outline-offset`, or 1px inside it with a border, above a 1px ring outside it of white, or
        // of #101010 in a dark scheme. Its corners follow the border radius, or else have a radius of 3px.
        let offset = style
            .get(Prop::OUTLINE_OFFSET)
            .and_then(|v| units.resolve(v, containing[0]))
            .unwrap_or(0.)
            .trunc()
            - if borders.iter().all(|width| *width >= 1.) {
                1.
            } else {
                0.
            };
        let color = style
            .get(Prop::OUTLINE_COLOR)
            .and_then(style::paint)
            .unwrap_or(ink);
        let under = renderer::Color::hex(if dark() { 0x101010ff } else { 0xffffffff });
        let mut ring = renderer::canvas(
            |_, _| (),
            move |bounds, _, window| {
                let outer = Rect::between(
                    bounds.origin - point(px(borders[3]), px(borders[0])),
                    bounds.bottom_right() + point(px(borders[1]), px(borders[2])),
                );
                let corners = radius.resolve(outer.size).fit(outer.size);
                let corners = if corners.is_zero() {
                    renderer::Radii::from(px(3.))
                } else {
                    let grown = |radius: renderer::Pixels| (radius + px(offset)).max(px(0.));
                    renderer::Radii {
                        x: corners.x.map(grown),
                        y: corners.y.map(grown),
                        shape: corners.shape,
                    }
                };
                for (at, color) in [(offset + 1., under), (offset, color)] {
                    let path = Rect::between(
                        outer.origin - point(px(at), px(at)),
                        outer.bottom_right() + point(px(at), px(at)),
                    );
                    if path.size.width <= px(0.) || path.size.height <= px(0.) {
                        continue;
                    }
                    let rect = Rect::between(
                        path.origin - point(px(1.), px(1.)),
                        path.bottom_right() + point(px(1.), px(1.)),
                    );
                    let corners = corners.inset(px(-1.), px(-1.), px(-1.), px(-1.));
                    window.draw_box(
                        rect,
                        corners.fit(rect.size),
                        renderer::Color::TRANSPARENT,
                        renderer::Border {
                            widths: renderer::Edges::all(px(2.)),
                            color,
                            style: renderer::BorderStyle::Solid,
                            sides: 0,
                        },
                    );
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        ring.style().escape = renderer::Escape::OUTLINE;
        el = el.child(ring);
    } else if let Some(width) = style
        .get(Prop::OUTLINE_STYLE)
        .filter(|kind| !matches!(*kind, "none" | "hidden"))
        .and_then(|_| {
            units.resolve(
                style.get(Prop::OUTLINE_WIDTH).unwrap_or("3px"),
                containing[0],
            )
        })
        .filter(|width| *width > 0.)
    {
        let grow = width
            + style
                .get(Prop::OUTLINE_OFFSET)
                .and_then(|v| units.resolve(v, containing[0]))
                .unwrap_or(0.);
        let color = style
            .get(Prop::OUTLINE_COLOR)
            .and_then(style::paint)
            .unwrap_or(ink);
        let name = style.get(Prop::OUTLINE_STYLE).unwrap_or("solid");
        let kind = border_style(name);
        // The top and left sides, then the right and bottom ones, of each band from the outer edge in.
        let bands: Vec<[renderer::Color; 2]> = match name {
            "inset" => vec![[true, false].map(|dark| shade_border(color, dark))],
            "outset" => vec![[false, true].map(|dark| shade_border(color, dark))],
            "groove" => vec![
                [true, false].map(|dark| shade_border(color, dark)),
                [false, true].map(|dark| shade_border(color, dark)),
            ],
            "ridge" => vec![
                [false, true].map(|dark| shade_border(color, dark)),
                [true, false].map(|dark| shade_border(color, dark)),
            ],
            _ => vec![[color; 2]],
        };
        // Like the border box, the outline is square at corners without a radius.
        let mut outline = renderer::canvas(
            |_, _| (),
            move |bounds, _, window| {
                let outer = Rect::between(
                    bounds.origin - point(px(borders[3]), px(borders[0])),
                    bounds.bottom_right() + point(px(borders[1]), px(borders[2])),
                );
                let corners = radius.resolve(outer.size);
                let grown = |radius: renderer::Pixels| {
                    if radius > px(0.) {
                        (radius + px(grow)).max(px(0.))
                    } else {
                        radius
                    }
                };
                let corners = corners.moved_out(
                    px(grow),
                    renderer::Radii {
                        x: corners.x.map(grown),
                        y: corners.y.map(grown),
                        shape: corners.shape,
                    },
                );
                let band = width / bands.len() as f32;
                for (at, colors) in bands.iter().enumerate() {
                    let inset = px(band * at as f32 - grow);
                    let rect = Rect::between(
                        outer.origin + point(inset, inset),
                        outer.bottom_right() - point(inset, inset),
                    );
                    let inner = px(band * at as f32);
                    let corners = corners.inset(inner, inner, inner, inner).fit(rect.size);
                    let split = colors[0] != colors[1];
                    for (sides, color) in [(if split { 9 } else { 0 }, colors[0]), (6, colors[1])]
                        .into_iter()
                        .take(1 + usize::from(split))
                    {
                        window.draw_box(
                            rect,
                            corners,
                            renderer::Color::TRANSPARENT,
                            renderer::Border {
                                widths: renderer::Edges::all(px(band)),
                                color,
                                style: kind,
                                sides,
                            },
                        );
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        // The outline takes the clips of its box, not the overflow clip of the box itself.
        outline.style().escape = renderer::Escape::OUTLINE;
        el = el.child(outline);
    }
    if shaded
        || first.is_some_and(|first| {
            visible
                .clone()
                .any(|side| border_colors[side] != border_colors[first])
        })
    {
        let mut bands = vec![([0.; 4], borders, [renderer::Color::TRANSPARENT; 4])];
        let split = (0..4).any(|side| matches!(border_kinds[side], "groove" | "ridge"));
        if split {
            bands.push(([0.; 4], [0.; 4], [renderer::Color::TRANSPARENT; 4]));
        }
        for side in 0..4 {
            let color = border_colors[side];
            let raised = side == 0 || side == 3;
            let (outer, inner) = match border_kinds[side] {
                "inset" => (shade_border(color, raised), None),
                "outset" => (shade_border(color, !raised), None),
                "groove" => (
                    shade_border(color, raised),
                    Some(shade_border(color, !raised)),
                ),
                "ridge" => (
                    shade_border(color, !raised),
                    Some(shade_border(color, raised)),
                ),
                _ => (color, None),
            };
            bands[0].2[side] = outer;
            if let Some(inner) = inner {
                let half = borders[side] / 2.;
                bands[0].1[side] = half;
                bands[1].0[side] = half;
                bands[1].1[side] = half;
                bands[1].2[side] = inner;
            }
        }
        if split {
            bands[1].0 = std::array::from_fn(|side| {
                if bands[1].1[side] > 0. {
                    bands[1].0[side]
                } else {
                    borders[side]
                }
            });
        }
        el = el.child(border_canvas(
            bands,
            borders,
            radius,
            border_kinds.map(border_style),
            false,
        ));
    }
    let resizer = (node.tag == "textarea"
        && style.get(Prop::RESIZE).is_none_or(|value| value != "none"))
    .then_some(super::control::Look {
        control: super::control::Control::Resizer,
        ..Default::default()
    });
    if let Some(look) = native
        .filter(|look| {
            !matches!(
                look.control,
                super::control::Control::Button | super::control::Control::Field
            )
        })
        .or(resizer)
    {
        el = el.child(if look.plain {
            let parts = styles.scrollbars.get(&id);
            let range = look.control == super::control::Control::Range;
            let ink = style::rgba(
                style
                    .get(Prop::COLOR)
                    .and_then(style::color)
                    .unwrap_or((0, 255)),
            );
            // Size, margin-top, border width, radius, fill and border color of the track and thumb, or of the
            // bar and value of a progress bar, with the defaults of Chrome. Both have `box-sizing: border-box`.
            let [track, thumb] = [1, 2].map(|slot| {
                let part = parts.and_then(|parts| parts[slot].as_ref());
                let get = |prop| part.and_then(|part| part.get(prop));
                let length = |prop| get(prop).and_then(|value| units.resolve(value, f32::NAN));
                let color = |prop| get(prop).and_then(style::paint);
                let native = range
                    && slot == 2
                    && [Prop::APPEARANCE, Prop::WEBKIT_APPEARANCE]
                        .into_iter()
                        .all(|prop| get(prop).is_none_or(|value| value != "none"));
                let border = get(Prop::BORDER_TOP_STYLE)
                    .is_some_and(|value| !matches!(value, "none" | "hidden"))
                    .then(|| length(Prop::BORDER_TOP_WIDTH))
                    .flatten()
                    .unwrap_or(0.);
                let fill = match (range, slot) {
                    _ if native => Some(style::rgba((look.accent, 255))),
                    (false, 1) => Some(renderer::Color::hex(0x808080ff)),
                    (false, _) => Some(renderer::Color::hex(0x008000ff)),
                    _ => None,
                };
                (
                    [Prop::WIDTH, Prop::HEIGHT].map(|prop| length(prop).or(native.then_some(16.))),
                    length(Prop::MARGIN_TOP).unwrap_or(0.),
                    border,
                    if native {
                        8.
                    } else {
                        get(Prop::BORDER_TOP_LEFT_RADIUS).map_or(0., |value| {
                            // Only round ends matter here, and the box clamps large radii.
                            if value.contains('%') {
                                1e4
                            } else {
                                units.resolve(value, f32::NAN).unwrap_or(0.)
                            }
                        })
                    },
                    color(Prop::BACKGROUND_COLOR).or(fill),
                    color(Prop::BORDER_TOP_COLOR).unwrap_or(ink),
                )
            });
            let (value, inset) = (look.value, padding);
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| {
                    let content = Rect::between(
                        bounds.origin + point(px(inset[3]), px(inset[0])),
                        bounds.bottom_right() - point(px(inset[1]), px(inset[2])),
                    );
                    let [width, height] = [content.size.width, content.size.height].map(f32::from);
                    let thumb_size = thumb.0.map(|side| side.unwrap_or(0.));
                    let track_height = match track.0[1] {
                        Some(side) if range => side,
                        _ if range => thumb_size[1] + 2. * track.2,
                        _ => height,
                    };
                    let track_top = (height - track_height) / 2.;
                    let inner = width - 2. * track.2;
                    let rects = [
                        (track, [0., track_top, width, track_height]),
                        if range {
                            (
                                thumb,
                                [
                                    track.2 + (inner - thumb_size[0]) * value,
                                    track_top + track.2 + thumb.1,
                                    thumb_size[0],
                                    thumb_size[1],
                                ],
                            )
                        } else {
                            (
                                thumb,
                                [
                                    track.2,
                                    track.2,
                                    inner * value.max(0.),
                                    height - 2. * track.2,
                                ],
                            )
                        },
                    ];
                    for ((_, _, border, radius, fill, edge), [x, y, w, h]) in rects {
                        if w <= 0. || h <= 0. {
                            continue;
                        }
                        let rect =
                            Rect::new(content.origin + point(px(x), px(y)), size(px(w), px(h)));
                        window.draw_box(
                            rect,
                            renderer::Radii::from(px(radius)).fit(rect.size),
                            fill.unwrap_or(renderer::Color::TRANSPARENT),
                            renderer::Border {
                                widths: px(border).into(),
                                color: edge,
                                style: renderer::BorderStyle::Solid,
                                sides: 0,
                            },
                        );
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into()
        } else {
            super::control::layer(id, look, view.clone())
        });
    }
    let overscroll = overscroll(style);
    let handle = {
        let mut host = view.host.borrow_mut();
        let scroll = &mut host.dom.entry(view.id).or_default().scroll;
        if scrolls.contains(&true)
            || !page
                && overflow.contains(&renderer::Overflow::Hidden)
                && overscroll != renderer::Point::default()
        {
            Some(scroll.entry(id).or_default().clone())
        } else if overflow.contains(&renderer::Overflow::Hidden) {
            reader.log(
                Query::Scroll(id as u32),
                0,
                hashed(scrolled(scroll.get(&id))),
            );
            scroll.get(&id).cloned()
        } else {
            None
        }
    };
    let mut bars = None;
    if let Some(handle) = handle {
        if !editable {
            el = el.track_scroll(&handle);
            if hit.is_none() {
                el.style().restrict_scroll_to_axis = true;
                el.style().overscroll = overscroll;
            }
        }
        bars = Some(scrollbar::Scrollbar {
            id,
            handle,
            look,
            axes: scrolls,
            reserved,
            borders,
            origin: [
                padding[3] - borders[1] - borders[3],
                padding[0] - borders[0] - borders[2],
            ],
            quiet: false,
        });
    }
    let bars = grid_bar.or(bars);
    let marker_style = styles
        .text_pseudos
        .get(&id)
        .and_then(|pseudos| pseudos[0].as_ref());
    let content = marker_style
        .and_then(|marker| marker.get(Prop::CONTENT))
        .filter(|value| *value != "normal");
    if display == "list-item"
        && content.is_none_or(|value| value != "none")
        && let Some(kind) = style
            .get(Prop::LIST_STYLE_TYPE)
            .filter(|kind| *kind != "none" || content.is_some())
    {
        let list = node.parent.map(|parent| &doc.nodes[parent]);
        let items = node.parent.map(|parent| {
            siblings
                .borrow_mut()
                .entry(parent)
                .or_insert_with(|| {
                    let items = doc.nodes[parent].children.iter().copied().filter(|&child| {
                        styles.values[child]
                            .get(Prop::DISPLAY)
                            .is_some_and(|value| value == "list-item")
                    });
                    Rc::new(
                        items
                            .enumerate()
                            .map(|(index, item)| (item, index))
                            .collect(),
                    )
                })
                .clone()
        });
        let position = items
            .as_ref()
            .and_then(|items| items.get(&id).copied())
            .unwrap_or(0) as i64;
        let count = items.as_ref().map_or(1, |items| items.len()) as i64;
        let start = list
            .and_then(|list| list.attrs.get("start"))
            .and_then(|value| value.parse::<i64>().ok());
        let number = node
            .attrs
            .get("value")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(
                if list.is_some_and(|list| list.attrs.contains_key("reversed")) {
                    start.unwrap_or(count) - position
                } else {
                    start.unwrap_or(1) + position
                },
            );
        let number = styles.list_items.get(&id).copied().unwrap_or(number);
        let marker = match kind {
            "disc" | "circle" | "square" => style::counter_text(kind, number),
            "disclosure-closed" => "▸".to_owned(),
            "disclosure-open" => "▾".to_owned(),
            other if other.starts_with(['"', '\'']) => other.trim_matches(['"', '\'']).to_owned(),
            "cjk-decimal"
            | "cjk-ideographic"
            | "trad-chinese-informal"
            | "simp-chinese-informal"
            | "hiragana"
            | "katakana" => format!("{}、", style::counter_text(kind, number)),
            _ => format!("{}.", style::counter_text(kind, number)),
        };
        let marker = content.map_or(marker, |content| {
            let mut text = String::new();
            for part in style::components(content, ' ') {
                if part.starts_with("counter(") || part.starts_with("counters(") {
                    text.push_str(&number.to_string());
                    continue;
                }
                let mut input = cssparser::Parser::new(part);
                if let Ok(string) = input.expect_string() {
                    text.push_str(string);
                }
            }
            text
        });
        let inside = style
            .get(Prop::LIST_STYLE_POSITION)
            .is_some_and(|value| value == "inside");
        let (indent, percent) = text_indent(style, &units);
        let indent = indent + percent * containing[0];
        let face = text.face(&style::font(marker_style.unwrap_or(style)));
        let size = marker_style
            .and_then(|marker| marker.get(Prop::FONT_SIZE))
            .and_then(|value| units.resolve(value, units.font.em))
            .unwrap_or(units.font.em);
        let metrics = text
            .line_metrics(face, px(size * pixel_ratio))
            .map(f32::from);
        let line = match marker_style.map(style::line_height) {
            Some(style::LineHeight::Fixed(line)) => line,
            Some(style::LineHeight::Scale(scale)) => scale * size,
            Some(style::LineHeight::Normal) => metrics.iter().sum::<f32>() / pixel_ratio,
            None => units.font.lh,
        };
        let mut top = padding[0];
        if !inside && marker_style.is_some() {
            let own = text.face(&parsed.face);
            let rise = f32::from(text.baseline(face, px(size), px(line), pixel_ratio))
                - f32::from(text.baseline(own, px(units.font.em), px(units.font.lh), pixel_ratio));
            if rise > 0. {
                el = el.pt(px(padding[0] + rise));
            } else {
                top -= rise;
            }
        }
        let mut marker_box = if content.is_none() && kind.starts_with("disclosure-") {
            let [ascent, descent, _] = metrics;
            let side = (ascent * 2. / 3.).floor();
            let leading = ((line * pixel_ratio - ascent - descent) / 2.).floor();
            let space = text
                .advance(face, px(size), ' ')
                .map_or(size / 4., f32::from);
            let look = super::control::Look {
                control: super::control::Control::Disclosure,
                checked: kind == "disclosure-open",
                ink: marker_style
                    .unwrap_or(style)
                    .get(Prop::COLOR)
                    .and_then(style::color)
                    .map_or(0, |(ink, _)| ink),
                ..Default::default()
            };
            let triangle = renderer::node()
                .absolute()
                .left_0()
                .top(px((leading + ascent - side) / pixel_ratio))
                .size(px(side / pixel_ratio))
                .child(super::control::layer(id, look, view.clone()));
            let above = (leading + ascent) / pixel_ratio;
            let marker = renderer::node()
                .flex_shrink_0()
                .w(px(side / pixel_ratio + space))
                .h(px(above))
                .mb(px(line - above))
                .baseline(renderer::Baseline::Bottom)
                .child(triangle);
            if inside {
                marker
            } else {
                marker.absolute().top(px(top)).right(relative(1.))
            }
        } else if inside && !(content.is_none() && matches!(kind, "disc" | "circle" | "square")) {
            // The marker starts the first line, so the text of that line is indented by its width.
            let marker = if marker.ends_with(' ') {
                marker
            } else {
                marker + " "
            };
            let run = style::text_run(marker_style.unwrap_or(style), &units, marker.len());
            let width = f32::from(text.shape_line(&marker, px(size), &run).line.width);
            el.text_style()
                .set_text_indent(px(indent + width))
                .set_text_indent_percent(0.);
            renderer::node()
                .absolute()
                .top(px(top))
                .left(px(padding[3] + indent))
                .whitespace_nowrap()
                .child(marker)
        } else if content.is_none() && matches!(kind, "disc" | "circle" | "square") {
            let color = marker_style
                .unwrap_or(style)
                .get(Prop::COLOR)
                .and_then(style::paint)
                .unwrap_or(Color::hex(0x000000ff));
            let [ascent, descent, _] = metrics;
            let offset = (ascent * 2. / 3.).floor();
            let leading = ((line * pixel_ratio - ascent - descent) / 2.).floor();
            let side = ((offset + 1.) / 2.).floor() / pixel_ratio;
            // Like Chrome, an inside symbol takes its size, 1px and 1em before the text.
            if inside {
                el.text_style()
                    .set_text_indent(px(indent + side + 1. + size))
                    .set_text_indent_percent(0.);
            }
            let dot = renderer::node()
                .absolute()
                .left(px(if inside {
                    padding[3] + indent
                } else {
                    padding[3] - (offset + 7.) / pixel_ratio
                }))
                .top(px(top
                    + (leading + (3. * (ascent - offset) / 2.).floor())
                        / pixel_ratio))
                .size(px(side));
            match kind {
                "disc" => dot.rounded_full().bg(color),
                "circle" => dot
                    .rounded_full()
                    .border(px(1. / pixel_ratio))
                    .border_color(color),
                _ => dot.bg(color),
            }
        } else {
            renderer::node()
                .absolute()
                .top(px(top))
                .right(relative(1.))
                // Like Chrome, a counter marker ends with a space, and strings and `、` end as they are.
                .pr(px(
                    if content.is_some()
                        || marker.ends_with(['、', ' '])
                        || kind.starts_with(['"', '\''])
                    {
                        0.
                    } else {
                        text.advance(face, px(size), ' ')
                            .map_or(size / 4., f32::from)
                    },
                ))
                .whitespace_nowrap()
                .child(marker)
        };
        if let Some(marker) = marker_style {
            let face = style::font(marker);
            marker_box = marker_box
                .font_family(face.family)
                .font_weight(face.weight)
                .text_size(px(size))
                .line_height(px(line));
            marker_box
                .text_style()
                .set_font_style(face.style)
                .set_font_synthesis(face.synthesis);
            if let Some(ink) = marker.get(Prop::COLOR).and_then(style::color) {
                marker_box = marker_box.text_color(style::rgba(ink));
            }
        }
        // Like Chrome's `::marker`, numbers take tabular widths.
        marker_box
            .text_style()
            .set_font_features(Rc::from([("tnum".to_owned(), 1)]));
        // Like Chrome, an empty item keeps the line of its marker.
        if !inside
            && node.children.iter().all(|&child| {
                doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty()
            })
            && style.get(Prop::HEIGHT).is_none_or(|v| v == "auto")
            && style.get(Prop::MIN_HEIGHT).is_none_or(|v| v == "auto")
        {
            let edges = padding[0] + padding[2] + borders[0] + borders[2];
            el = el.min_h(px(line.max(units.font.lh) + edges));
        }
        el = el.child(marker_box);
    }
    let content_box = || {
        let mut content = containing;
        for (axis, (prop, min, max)) in [
            (Prop::WIDTH, Prop::MIN_WIDTH, Prop::MAX_WIDTH),
            (Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(value) = style
                .get(prop)
                .and_then(|v| units.resolve(v, containing[axis]))
                .map(|value| value + extra[axis])
                .or(intrinsic_width.filter(|_| axis == 0))
            {
                content[axis] = value;
            } else if axis == 0 {
                content[axis] -= margin[1] + margin[3];
                content[axis] -= inset_x.iter().flatten().sum::<f32>();
            } else if let Some(ratio) = style
                .get(Prop::ASPECT_RATIO)
                .filter(|value| !(value.contains("auto") && natural_ratio.is_some()))
                .and_then(aspect_ratio)
            {
                // Like the layout, the ratio applies to the border box.
                content[1] =
                    (content[0] + padding[1] + padding[3] + borders[1] + borders[3]) / ratio;
            }
            if let Some(value) = style
                .get(max)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].min(value + extra[axis]);
            }
            if let Some(value) = style
                .get(min)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].max(value + extra[axis]);
            }
            content[axis] = (content[axis]
                - padding[1 - axis]
                - padding[3 - axis]
                - borders[1 - axis]
                - borders[3 - axis])
                .max(0.);
        }
        if let Some((bounds, _)) = &slot {
            reader.log(
                Query::Cell,
                depth,
                hashed(Some([bounds[2], bounds[3]].map(f32::to_bits))),
            );
            content[0] = (bounds[2]
                - padding[1]
                - padding[3]
                - margin[1]
                - margin[3]
                - borders[1]
                - borders[3])
                .max(0.);
            content[1] = (bounds[3]
                - padding[0]
                - padding[2]
                - margin[0]
                - margin[2]
                - borders[0]
                - borders[2])
                .max(0.);
        }
        content
    };
    let kind = style.get(Prop::CONTAINER_TYPE);
    let own = kind
        .is_some_and(|v| matches!(v, "inline-size" | "size"))
        .then(|| {
            let mut host = view.host.borrow_mut();
            let dom = host.dom.entry(view.id).or_default();
            let last = dom.cull.widths.get(&id).copied();
            reader.log(
                Query::Width(id as u32),
                0,
                hashed(last.map(|size| size.map(f32::to_bits))),
            );
            let size = last.unwrap_or_else(content_box);
            dom.containers.insert(id, size);
            // Block-axis units of an inline-size container use the next size container, or the viewport.
            if kind == Some("inline-size") {
                [size[0], container[1]]
            } else {
                size
            }
        });
    let mut after = Vec::new();
    // Inline pseudo-elements of a block that also holds blocks, which join the first and last lines.
    let mut lines: [Option<renderer::Node>; 2] = [None, None];
    // The placeholder of an out-of-flow `::after` at its static place, at the end of the last line.
    let mut static_anchor: Option<renderer::Node> = None;
    // In-flow pseudo-elements of a grid or flex box are its first and last items, which the grid places and
    // flex orders among the children.
    let mut items = [None, None];
    // Pseudo-elements that are atoms of the element's text, built here and placed with the text below.
    let mut pseudo_atoms: [Option<renderer::Node>; 2] = [None, None];
    let mut pseudo_origin: Option<Rc<std::cell::Cell<renderer::Point<renderer::Pixels>>>> = None;
    if let Some(pseudos) = styles.pseudos.get(&id).filter(|_| !skipped) {
        let mut units = units;
        units.container = own.or(units.container);
        let keyed = info[id].flags & cull::COMPOSITED_PSEUDO != 0;
        for (index, style_of) in pseudos.iter().enumerate() {
            if pseudo.is_some_and(|only| only != index) {
                continue;
            }
            let pseudo_only = pseudo.is_some();
            let pseudo = style_of;
            let Some(content) = pseudo
                .get(Prop::CONTENT)
                .filter(|v| !matches!(*v, "none" | "normal"))
            else {
                continue;
            };
            if pseudo
                .get(Prop::DISPLAY)
                .is_some_and(|value| value == "none")
            {
                continue;
            }
            // The text holds an inline pseudo-element: plain text as runs, a box as an atom.
            let held = inline_text().or_else(|| edge_text(index));
            let merged = !editable
                && !matches!(node.tag.as_str(), "svg" | "canvas" | "iframe" | "terminal")
                && held.as_ref().is_some_and(|data| data.pseudos[index]);
            let atom = merged
                && held.is_some_and(|data| {
                    data.atoms
                        .iter()
                        .any(|atom| usize::from(atom.pseudo) == index + 1)
                });
            if merged && !atom {
                continue;
            }
            let words = style::pseudo_content(doc, id, pseudo, content);
            let pseudo_units = pseudo_lengths(&units, style, pseudo, text, pixel_ratio);
            let face = style::font(pseudo);
            let own = pseudo.get(Prop::DISPLAY).unwrap_or("inline");
            let mut decoration = renderer::node()
                .flex()
                .flex_col()
                .text_size(px(pseudo_units.font.em))
                .font_family(face.family.clone())
                .font_weight(face.weight);
            decoration.style().flex_shrink = if flexbox { 1. } else { 0. };
            if face.style != renderer::FontStyle::Normal {
                decoration.text_style().set_font_style(face.style);
            }
            if face.synthesis != [true; 2] {
                decoration.text_style().set_font_synthesis(face.synthesis);
            }
            match own {
                "flex" | "inline-flex" => {
                    decoration = decoration.flex_row();
                    if pseudo
                        .get(Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v.starts_with("column"))
                    {
                        decoration = decoration.flex_col();
                    }
                }
                // The text of a grid pseudo-element is its only item, so a column places it like one cell.
                "grid" | "inline-grid" => {
                    for (prop, axis) in [
                        (Prop::JUSTIFY_ITEMS, Prop::ALIGN_ITEMS),
                        (Prop::ALIGN_ITEMS, Prop::JUSTIFY_CONTENT),
                    ] {
                        let place = |value| match value {
                            "center" => Some(renderer::AlignItems::Center),
                            "end" | "flex-end" | "self-end" => Some(renderer::AlignItems::FlexEnd),
                            "start" | "flex-start" | "self-start" => {
                                Some(renderer::AlignItems::FlexStart)
                            }
                            _ => None,
                        };
                        match (pseudo.get(prop).and_then(place), axis) {
                            (Some(align), Prop::ALIGN_ITEMS) => {
                                decoration.style().align_items = Some(align)
                            }
                            (Some(align), _) => {
                                decoration.style().justify_content = Some(match align {
                                    renderer::AlignItems::Center => renderer::AlignContent::Center,
                                    renderer::AlignItems::FlexEnd => {
                                        renderer::AlignContent::FlexEnd
                                    }
                                    _ => renderer::AlignContent::FlexStart,
                                })
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            match style::line_height(pseudo) {
                style::LineHeight::Fixed(height) => decoration = decoration.line_height(px(height)),
                style::LineHeight::Scale(scale) => {
                    decoration = decoration.line_height(px(scale * pseudo_units.font.em))
                }
                style::LineHeight::Normal => {}
            }
            if let Some(ink) = pseudo.get(Prop::COLOR).and_then(style::color) {
                decoration = decoration.text_color(style::rgba(ink));
            }
            let absolute = pseudo
                .get(Prop::POSITION)
                .is_some_and(|v| matches!(v, "absolute" | "fixed"));
            // A float goes to its side of the first line. Text doesn't flow around it.
            let floated = float_side(pseudo)
                .filter(|_| !absolute && !flexbox && !matches!(display, "grid" | "inline-grid"));
            let own_line = !absolute
                && floated.is_none()
                && !own.starts_with("inline")
                && (all_inline || display == "inline");
            // In a column, layout places the float. In a line, it goes to its side of the first line.
            let column = !all_inline && display != "inline" && !flexbox;
            if !absolute {
                decoration.style().flow = clear_bits(pseudo)
                    | match floated {
                        Some(true) if column => renderer::Flow::RIGHT,
                        Some(false) if column => renderer::Flow::LEFT,
                        _ => 0,
                    }
                    | if rtl { renderer::Flow::RTL } else { 0 };
            }
            let floated = floated.filter(|_| !column);
            if floated.is_some() {
                decoration.style().align_self = Some(renderer::AlignItems::FlexStart);
            }
            if floated == Some(true) {
                decoration = decoration
                    .absolute()
                    .top(px(padding[0]))
                    .right(px(padding[1]));
            }
            let grid_item = !absolute && matches!(display, "grid" | "inline-grid");
            if absolute || grid_item {
                decoration = decoration.absolute();
            }
            // Percentages of a pseudo-element take the width of its element, the padding box when absolute.
            let base = content_box()[0]
                + if absolute {
                    padding[1] + padding[3]
                } else {
                    0.
                };
            let pseudo_padding = pseudo_units
                .edges(pseudo, style::PADDING, base)
                .map(|v| v.max(0.));
            let pseudo_margin = pseudo_units.edges(pseudo, style::MARGIN, base);
            let pseudo_borders = pseudo_units.borders(pseudo, base);
            decoration = decoration
                .pt(px(pseudo_padding[0]))
                .pr(px(pseudo_padding[1]))
                .pb(px(pseudo_padding[2]))
                .pl(px(pseudo_padding[3]))
                .mt(px(pseudo_margin[0]))
                .mr(px(pseudo_margin[1]))
                .mb(px(pseudo_margin[2]))
                .ml(px(pseudo_margin[3]))
                .border_t(px(pseudo_borders[0]))
                .border_r(px(pseudo_borders[1]))
                .border_b(px(pseudo_borders[2]))
                .border_l(px(pseudo_borders[3]));
            for (side, prop) in style::MARGIN.iter().enumerate() {
                if pseudo.get(*prop) == Some("auto") {
                    decoration = match side {
                        0 => decoration.mt_auto(),
                        1 => decoration.mr_auto(),
                        2 => decoration.mb_auto(),
                        _ => decoration.ml_auto(),
                    };
                }
            }
            let pseudo_extra = if pseudo.get(Prop::BOX_SIZING) == Some("border-box") {
                [0.; 2]
            } else {
                [
                    pseudo_padding[1] + pseudo_padding[3] + pseudo_borders[1] + pseudo_borders[3],
                    pseudo_padding[0] + pseudo_padding[2] + pseudo_borders[0] + pseudo_borders[2],
                ]
            };
            let pseudo_radius = super::effects::radii(pseudo, &pseudo_units, || containing);
            decoration.style().corner_radii = pseudo_radius;
            let colors =
                sides.map(|(_, _, color)| pseudo.get(color).and_then(style::paint).unwrap_or(ink));
            let border_image = super::background::border_image(
                pseudo,
                pseudo_units,
                id,
                index + 1,
                pseudo_borders,
                view,
            );
            let mut shown = (0..4).filter(|&side| pseudo_borders[side] > 0.);
            if border_image.is_some() {
                decoration = decoration.border_color(renderer::Color::TRANSPARENT);
            } else if let Some(first) = shown.next() {
                if shown.all(|side| colors[side] == colors[first]) {
                    decoration = decoration.border_color(colors[first]);
                } else {
                    decoration = decoration.border_color(renderer::Color::TRANSPARENT);
                    let kinds =
                        sides.map(|(_, kind, _)| border_style(pseudo.get(kind).unwrap_or("solid")));
                    decoration = decoration.child(border_canvas(
                        vec![([0.; 4], pseudo_borders, colors)],
                        pseudo_borders,
                        pseudo_radius,
                        kinds,
                        true,
                    ));
                }
            }
            let mut pseudo_shadows = Vec::new();
            let placed = absolute || pseudo.get(Prop::POSITION) == Some("relative");
            // An out-of-flow pseudo-element whose element makes no containing block for it goes to the one of an
            // ancestor: the viewport (`usize::MAX`), the initial block (0) or a box, with its padding box size.
            let fixed = pseudo.get(Prop::POSITION) == Some("fixed");
            let gives = if fixed { made.fixed } else { made.absolute };
            // Built alone, an out-of-flow pseudo-element is beside the text that holds its inline box, so it
            // follows its containing block from there, also when its element gives it.
            let lifted = pseudo_only && absolute;
            let escaped = (absolute && id != 0 && (!gives || lifted)).then(|| {
                let block = match gives {
                    true => Some(id),
                    false => self::positioner(
                        if fixed { "fixed" } else { "absolute" },
                        ancestors.iter().rev().copied(),
                        |ancestor| super::effects::establishes(&styles.values[ancestor]),
                    ),
                };
                let scope = block
                    .and_then(|block| ancestors.iter().rposition(|&a| a == block))
                    .map_or(1, |index| index + 1);
                if !gives {
                    reader.log(Query::Positioner(fixed), scope, hashed(block));
                }
                match block {
                    None | Some(0) => (
                        block.unwrap_or(usize::MAX),
                        viewport().unwrap_or(units.viewport),
                        false,
                    ),
                    Some(block) => {
                        let size = reader.block_size(view.host.borrow().dom.get(&view.id), block);
                        reader.log(
                            Query::Block(block as u32),
                            0,
                            hashed(size.map(|size| size.map(f32::to_bits))),
                        );
                        (block, size.unwrap_or(containing), true)
                    }
                }
            });
            let own_flex = matches!(own, "flex" | "inline-flex");
            for (prop, value) in pseudo.iter() {
                let value: &str = value;
                let vertical = matches!(
                    prop,
                    Prop::HEIGHT | Prop::MIN_HEIGHT | Prop::MAX_HEIGHT | Prop::TOP | Prop::BOTTOM
                );
                let base = escaped.map_or(containing, |(_, size, _)| size)[usize::from(vertical)];
                // Layout takes percentages from the element, so those of another block resolve here.
                let length = |extra: f32, own: &mut renderer::Style| -> Option<renderer::Length> {
                    if escaped.is_some() {
                        return pseudo_units
                            .resolve(value, base)
                            .map(|value| px(value + extra).into());
                    }
                    match percentage(value) {
                        Some(percent) if extra == 0. => Some(relative(percent / 100.)),
                        Some(percent) => Some(renderer::Length::Calc(percent / 100., px(extra))),
                        None => match percent_calc(&pseudo_units, value) {
                            Some((fraction, offset)) => {
                                Some(renderer::Length::Calc(fraction, px(offset + extra)))
                            }
                            None => percent_bound(&pseudo_units, value, extra, own).or_else(|| {
                                pseudo_units
                                    .resolve(value, base)
                                    .map(|value| px(value + extra).into())
                            }),
                        },
                    }
                };
                match prop {
                    Prop::WIDTH
                    | Prop::HEIGHT
                    | Prop::MIN_WIDTH
                    | Prop::MAX_WIDTH
                    | Prop::MIN_HEIGHT
                    | Prop::MAX_HEIGHT => {
                        let Some(length) =
                            length(pseudo_extra[usize::from(vertical)], decoration.style())
                        else {
                            continue;
                        };
                        decoration = match prop {
                            Prop::WIDTH => decoration.w(length),
                            Prop::HEIGHT => decoration.h(length),
                            Prop::MIN_WIDTH => decoration.min_w(length),
                            Prop::MAX_WIDTH => decoration.max_w(length),
                            Prop::MIN_HEIGHT => decoration.min_h(length),
                            _ => decoration.max_h(length),
                        };
                    }
                    Prop::LEFT | Prop::TOP | Prop::RIGHT | Prop::BOTTOM if placed => {
                        let Some(length) = length(0., decoration.style()) else {
                            continue;
                        };
                        decoration = match prop {
                            Prop::LEFT => decoration.left(length),
                            Prop::TOP => decoration.top(length),
                            Prop::RIGHT => decoration.right(length),
                            _ => decoration.bottom(length),
                        };
                    }
                    Prop::ASPECT_RATIO => {
                        if let Some(ratio) = aspect_ratio(value) {
                            decoration.style().aspect_ratio = Some(ratio);
                            decoration = only_auto_ratio(decoration, pseudo);
                        }
                    }
                    Prop::FLEX_GROW | Prop::FLEX_SHRINK if flexbox => {
                        if let Some(factor) = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite() && *v >= 0.)
                        {
                            if prop == Prop::FLEX_GROW {
                                decoration.style().flex_grow = factor;
                            } else {
                                decoration.style().flex_shrink = factor;
                            }
                        }
                    }
                    Prop::FLEX_BASIS if flexbox => {
                        let axis = usize::from(
                            style
                                .get(Prop::FLEX_DIRECTION)
                                .is_some_and(|v| v.starts_with("column")),
                        );
                        decoration.style().flex_basis = match value
                            .strip_suffix('%')
                            .and_then(|v| v.parse::<f32>().ok())
                            .filter(|v| v.is_finite())
                        {
                            Some(percent) => renderer::Length::Calc(
                                percent.max(0.) / 100.,
                                px(pseudo_extra[axis]),
                            ),
                            None => pseudo_units
                                .resolve(value, containing[axis])
                                .map_or(renderer::Length::Auto, |length| {
                                    px(length.max(0.) + pseudo_extra[axis]).into()
                                }),
                        };
                    }
                    Prop::ALIGN_SELF | Prop::ALIGN_ITEMS
                        if prop == Prop::ALIGN_SELF && flexbox || own_flex =>
                    {
                        let align = match value {
                            "center" => renderer::AlignItems::Center,
                            "flex-start" | "start" | "self-start" => {
                                renderer::AlignItems::FlexStart
                            }
                            "flex-end" | "end" | "self-end" => renderer::AlignItems::FlexEnd,
                            "baseline" | "first baseline" => renderer::AlignItems::Baseline,
                            "last baseline" => renderer::AlignItems::LastBaseline,
                            "stretch" | "normal" => renderer::AlignItems::Stretch,
                            _ => continue,
                        };
                        if prop == Prop::ALIGN_SELF {
                            decoration.style().align_self = Some(align);
                        } else {
                            decoration.style().align_items = Some(align);
                        }
                    }
                    Prop::JUSTIFY_CONTENT if own_flex => {
                        decoration.style().justify_content = Some(match value {
                            "center" => renderer::AlignContent::Center,
                            "flex-end" | "end" | "right" => renderer::AlignContent::FlexEnd,
                            "space-between" => renderer::AlignContent::SpaceBetween,
                            "space-around" => renderer::AlignContent::SpaceAround,
                            "space-evenly" => renderer::AlignContent::SpaceEvenly,
                            _ => renderer::AlignContent::FlexStart,
                        });
                    }
                    Prop::TEXT_ALIGN => {
                        decoration = match (value, rtl) {
                            ("center" | "-webkit-center", _) => decoration.text_center(),
                            ("right" | "-webkit-right", _)
                            | ("end", false)
                            | ("start" | "justify", true) => decoration.text_right(),
                            _ => decoration.text_left(),
                        }
                    }
                    Prop::WHITE_SPACE if matches!(value, "nowrap" | "pre") => {
                        decoration = decoration.whitespace_nowrap().flex_nowrap()
                    }
                    Prop::OVERFLOW_X | Prop::OVERFLOW_Y if value != "visible" => {
                        let overflow = renderer::Overflow::Hidden;
                        if prop == Prop::OVERFLOW_X {
                            decoration.style().overflow.x = overflow;
                        } else {
                            decoration.style().overflow.y = overflow;
                        }
                    }
                    Prop::VISIBILITY if matches!(value, "hidden" | "collapse") => {
                        decoration.style().visibility = renderer::Visibility::Hidden;
                    }
                    Prop::BACKGROUND_COLOR => {
                        if let Some(color) = style::paint(value) {
                            decoration = decoration.bg(color);
                        }
                    }
                    Prop::OPACITY if !keyed => {
                        decoration = decoration.opacity(super::effects::opacity(pseudo));
                    }
                    Prop::BOX_SHADOW => {
                        decoration.style().box_shadow = pseudo_units.shadows(value, false);
                        pseudo_shadows = pseudo_units.shadows(value, true);
                    }
                    _ => {}
                }
            }
            if pseudo.get(Prop::TEXT_OVERFLOW) == Some("ellipsis")
                && matches!(pseudo.get(Prop::WHITE_SPACE), Some("nowrap" | "pre"))
                && decoration.style_ref().overflow.x != renderer::Overflow::Visible
            {
                decoration.text_style().set_text_overflow(Some("…".into()));
            }
            if pseudo
                .get(Prop::BACKGROUND_IMAGE)
                .is_some_and(|value| value != "none")
            {
                let base = view
                    .host
                    .borrow()
                    .dom
                    .get(&view.id)
                    .and_then(|state| state.base.clone());
                decoration = decoration.children(css_background(
                    pseudo,
                    units,
                    id,
                    index + 1,
                    pseudo_radius,
                    // Pseudo-element borders paint below their layers, so the layers keep out of them.
                    [[0.; 4], pseudo_padding, pseudo_borders],
                    view.clone(),
                    base.as_ref(),
                    &view.host.borrow().assets,
                    false,
                ));
            }
            if !pseudo_shadows.is_empty() {
                decoration =
                    decoration.child(inset_shadows(pseudo_shadows, pseudo_radius, pseudo_borders));
            }
            decoration = decoration.children(border_image);
            let blank = words.trim_ascii().is_empty();
            let collapsed = blank
                && pseudo
                    .get(Prop::DISPLAY)
                    .is_some_and(|value| !value.starts_with("inline"))
                && pseudo.get(Prop::WHITE_SPACE).is_none_or(|value| {
                    !matches!(value, "pre" | "pre-wrap" | "pre-line" | "break-spaces")
                });
            if !words.is_empty() && !collapsed {
                decoration = decoration.child(SharedString::from(words));
            }
            // Vertical alignment in a line. A line aligns text baselines, or the bottom border edge of a box
            // without one, so a relative offset moves the box and margins make room for it in the line.
            if !absolute && own.starts_with("inline") && (all_inline || display == "inline" || atom)
            {
                let align = pseudo.get(Prop::VERTICAL_ALIGN).unwrap_or("baseline");
                let height = match decoration.style().size.height {
                    renderer::Length::Px(height) => Some(f32::from(height)),
                    _ => None,
                };
                let line = units.font;
                let [ascent, descent, _] = if own == "inline" {
                    text.line_metrics(text.face(&face), px(pseudo_units.font.em * pixel_ratio))
                        .map(f32::from)
                } else {
                    [0.; 3]
                };
                let centered = align == "middle" && height.is_none() && own != "inline";
                decoration.style().align_self = match align {
                    "top" => Some(renderer::AlignItems::FlexStart),
                    "text-top" if height.is_none() => Some(renderer::AlignItems::FlexStart),
                    "bottom" => Some(renderer::AlignItems::FlexEnd),
                    _ => None,
                };
                let edge = decoration.style().align_self.is_none()
                    && own != "inline"
                    && !centered
                    && (matches!(align, "text-top" | "text-bottom" | "middle") || blank);
                if centered {
                    decoration.style().baseline = renderer::Baseline::Middle;
                } else if edge {
                    decoration.style().baseline = renderer::Baseline::None;
                }
                let lift = match (align, height) {
                    _ if decoration.style().align_self.is_some() => 0.,
                    _ if centered => line.ex / 2. + (pseudo_margin[2] - pseudo_margin[0]) / 2.,
                    ("middle", _) if own == "inline" => {
                        line.ex / 2. - (ascent - descent) / pixel_ratio / 2.
                    }
                    _ if !edge => 0.,
                    ("text-top" | "text-bottom", _) => {
                        let [above, below, _] = text
                            .line_metrics(text.face(&parsed.face), px(line.em * pixel_ratio))
                            .map(|value| f32::from(value) / pixel_ratio);
                        match height {
                            Some(height) if align == "text-top" => {
                                above - pseudo_margin[0] - height
                            }
                            _ => pseudo_margin[2] - below,
                        }
                    }
                    ("middle", Some(height)) => line.ex / 2. - height / 2.,
                    _ => pseudo_margin[2],
                };
                let raise = match align {
                    "super" => line.em / 3. + 1. / pixel_ratio + lift,
                    "sub" => -line.em / 5. - 1. / pixel_ratio + lift,
                    "baseline" | "middle" | "top" | "bottom" | "text-top" | "text-bottom" => lift,
                    value => {
                        pseudo_units
                            .resolve(value, pseudo_units.font.lh)
                            .unwrap_or(0.)
                            + lift
                    }
                };
                // Like browsers, an inline box paints its content area, and its leading, padding and borders
                // hang outside its line.
                let [top, bottom] = if own == "inline" {
                    let leading = pseudo_units.font.lh * pixel_ratio - ascent - descent;
                    let above = (leading / 2.).floor();
                    decoration = decoration.line_height(px((ascent + descent) / pixel_ratio));
                    [
                        above / pixel_ratio - pseudo_padding[0] - pseudo_borders[0],
                        (leading - above) / pixel_ratio - pseudo_padding[2] - pseudo_borders[2],
                    ]
                } else {
                    [pseudo_margin[0], pseudo_margin[2]]
                };
                if raise != 0. || own == "inline" {
                    decoration = decoration.mt(px(top + raise)).mb(px(bottom - raise));
                }
                if raise != 0. && !placed {
                    decoration = decoration.relative().top(px(-raise));
                }
            }
            if let Some(value) = [Prop::FILTER, Prop::WEBKIT_FILTER]
                .into_iter()
                .find_map(|prop| pseudo.get(prop))
                .filter(|value| *value != "none")
            {
                for effect in super::effects::filters(value, &pseudo_units, doc, ids) {
                    decoration = decoration.hooks(super::paint::Effected {
                        effect,
                        backdrop: None,
                    });
                }
            }
            if [Prop::MASK_IMAGE, Prop::WEBKIT_MASK_IMAGE]
                .into_iter()
                .find_map(|prop| pseudo.get(prop))
                .is_some_and(|value| value != "none")
            {
                decoration = decoration.hooks(super::paint::Masked {
                    view: view.clone(),
                    key: (id, index as u8 + 1),
                    style: Rc::new(pseudo.clone()),
                    units: pseudo_units,
                });
            }
            if let Some(value) = pseudo
                .get(Prop::MIX_BLEND_MODE)
                .filter(|value| *value != "normal")
            {
                decoration = decoration.hooks(super::paint::Effected {
                    effect: renderer::PaintEffect {
                        blend_mode: super::effects::blend_mode(value),
                        ..Default::default()
                    },
                    backdrop: None,
                });
            }
            // An opacity animation keeps one keyed layer, so its frames only patch the layer's opacity.
            if keyed {
                decoration = decoration.hooks(super::paint::Effected {
                    effect: renderer::PaintEffect {
                        opacity: super::effects::opacity(pseudo),
                        key: Some(super::pseudo_key(id, index as u8 + 1)),
                        ..Default::default()
                    },
                    backdrop: None,
                });
            }
            let decoration = match [Prop::BACKDROP_FILTER, Prop::WEBKIT_BACKDROP_FILTER]
                .into_iter()
                .find_map(|prop| pseudo.get(prop))
                .filter(|_| {
                    pseudo
                        .get(Prop::VISIBILITY)
                        .is_none_or(|value| value == "visible")
                })
                .and_then(|value| super::effects::backdrop(value, &pseudo_units))
            {
                Some((blur, color_matrix)) => decoration.hooks(super::paint::Effected {
                    effect: renderer::PaintEffect {
                        backdrop: Some(renderer::Backdrop {
                            bounds: Rect::default(),
                            corner_radii: Default::default(),
                            blur,
                            color_matrix,
                        }),
                        ..Default::default()
                    },
                    backdrop: Some(pseudo_radius),
                }),
                None => decoration,
            };
            let decoration = if super::TRANSFORM_PROPS
                .iter()
                .any(|&prop| pseudo.get(prop).is_some_and(|value| value != "none"))
            {
                decoration.hooks(super::paint::Transformed {
                    style: Rc::new(pseudo.clone()),
                    units: pseudo_units,
                    matrix: Default::default(),
                    local: Default::default(),
                })
            } else {
                decoration
            };
            let mut contained = None;
            let decoration = match escaped {
                Some((container, size, sized)) => {
                    let inset = [Prop::TOP, Prop::RIGHT, Prop::BOTTOM, Prop::LEFT].map(|prop| {
                        let axis = usize::from(matches!(prop, Prop::TOP | Prop::BOTTOM));
                        pseudo
                            .get(prop)
                            .and_then(|v| pseudo_units.resolve(v, size[axis]))
                    });
                    // Both insets of an axis stretch an auto size across the containing block.
                    let mut decoration = decoration;
                    for (axis, [start, end]) in [(0, [3, 1]), (1, [0, 2])] {
                        let prop = if axis == 0 { Prop::WIDTH } else { Prop::HEIGHT };
                        if let (Some(first), Some(last)) = (inset[start], inset[end])
                            && pseudo.get(prop).is_none_or(|value| value == "auto")
                        {
                            let length = px((size[axis]
                                - first
                                - last
                                - pseudo_margin[start]
                                - pseudo_margin[end])
                                .max(0.));
                            decoration = if axis == 0 {
                                decoration.w(length)
                            } else {
                                decoration.h(length)
                            };
                        }
                    }
                    decoration.style().escape = if fixed {
                        renderer::Escape::FIXED
                    } else {
                        renderer::Escape::ABSOLUTE
                    };
                    decoration.style().placed = inset_axes(pseudo);
                    contained = Some(super::paint::PseudoBlock {
                        view: view.clone(),
                        anchor: None,
                        place: Some(super::paint::Contained {
                            key: super::pseudo_key(id, index as u8 + 1) as usize,
                            container,
                            viewport: units.viewport,
                            sized: sized.then_some(size),
                            inset,
                            margin: pseudo_margin,
                            rtl: styles.values[if container == usize::MAX {
                                0
                            } else {
                                container
                            }]
                            .get(Prop::DIRECTION)
                                == Some("rtl"),
                        }),
                        offset: Default::default(),
                    });
                    decoration
                }
                None => decoration,
            };
            // Like an out-of-flow child, an `::after` at its static place in the last line follows a placeholder there.
            if index == 1 && style::static_after(styles, id) {
                let key = style::atom_key(id, 2);
                if flow
                    && !flexbox
                    && !matches!(display, "grid" | "inline-grid")
                    && !node.children.is_empty()
                    && inline_text().is_none()
                {
                    let mut placeholder = static_mark(view, key).flex_shrink_0();
                    placeholder.style().align_self = Some(renderer::AlignItems::FlexStart);
                    static_anchor = Some(placeholder);
                }
                let axes = [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]].map(|axis| {
                    axis.iter()
                        .all(|&prop| pseudo.get(prop).is_none_or(|value| value == "auto"))
                });
                // The direction of the containing block: the element's, or the one of the block it escapes to.
                let rtl = match escaped {
                    Some((usize::MAX, ..)) => styles.values[0].get(Prop::DIRECTION) == Some("rtl"),
                    Some((block, ..)) => styles.values[block].get(Prop::DIRECTION) == Some("rtl"),
                    None => rtl,
                };
                let anchor = Some((
                    axes,
                    key,
                    [pseudo_margin[3], pseudo_margin[0], pseudo_margin[1]],
                    rtl,
                ));
                match &mut contained {
                    Some(contained) => contained.anchor = anchor,
                    None => {
                        contained = Some(super::paint::PseudoBlock {
                            view: view.clone(),
                            place: None,
                            anchor,
                            offset: Default::default(),
                        })
                    }
                }
            }
            let order = pseudo
                .get(Prop::Z_INDEX)
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            // Like positioned elements, positioned pseudo-elements with `z-index: auto` or 0 paint after the in-flow
            // boxes of their stacking context.
            let decoration = if pseudo
                .get(Prop::POSITION)
                .is_some_and(|value| value != "static")
                || order != 0 && grid_item
            {
                decoration.z_index(order)
            } else {
                decoration
            };
            // Only boxes that `getComputedStyle()` reads keep their place.
            let decoration = if matches!(own, "inline" | "none" | "contents") {
                decoration
            } else {
                decoration.hooks(super::paint::PseudoBox {
                    view: view.clone(),
                    element: id,
                    origin: pseudo_origin.get_or_insert_default().clone(),
                    shift: contained.as_ref().map(|contained| contained.offset.clone()),
                    mirrored: lifted,
                    key: style::atom_key(id, index as u8 + 1),
                })
            };
            // Outside `PseudoBox`, which reads its move.
            let decoration = match contained {
                Some(contained) => decoration.hooks(contained),
                None => decoration,
            };
            if lifted {
                return decoration.into();
            }
            // The part of a spliced box gives the box its bounds.
            if pseudo_only {
                return decoration
                    .hooks(super::paint::PseudoPart {
                        view: view.clone(),
                        node: id,
                        key: style::atom_key(id, index as u8 + 1),
                    })
                    .into();
            }
            // A block pseudo-element in a line of text takes a line of its own.
            let decoration = if own_line {
                renderer::node()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(decoration)
            } else {
                decoration
            };
            // Children with an `order` are sorted, so an ordered pseudo-element only moves to the far end.
            let flex_order = pseudo
                .get(Prop::ORDER)
                .filter(|_| flexbox)
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            let flex_order = if floated == Some(false) {
                i32::MIN
            } else {
                flex_order
            };
            if atom && !absolute {
                pseudo_atoms[index] = Some(decoration);
            } else if grid_item || flexbox && !absolute {
                items[index] = Some((flex_order, decoration, pseudo_margin));
            } else if flow
                && !all_inline
                && display != "inline"
                && !absolute
                && own.starts_with("inline")
                && float_side(pseudo).is_none()
                && !node.children.is_empty()
                && !editable
                && !matches!(node.tag.as_str(), "svg" | "canvas" | "iframe" | "terminal")
                && inline_text().is_none()
            {
                lines[index] = Some(decoration);
            } else if index == 0 && flex_order <= 0 || index == 1 && flex_order < 0 {
                if std::mem::take(&mut strut_due) {
                    el = el.child(strut());
                }
                el = el.child(decoration);
            } else {
                after.push((flex_order, decoration));
            }
        }
    }
    let preserve = style
        .get(Prop::WHITE_SPACE)
        .is_some_and(|v| matches!(v, "pre" | "pre-wrap" | "break-spaces"))
        || node.tag == "pre";
    if hit.is_none()
        && (node.tag == "#text" || display == "inline" && !flex_parent && slot.is_none())
        && style
            .get(Prop::WHITE_SPACE)
            .is_none_or(|value| !matches!(value, "pre" | "nowrap"))
    {
        el = el.min_w(px(0.)).max_w(relative(1.)).flex_shrink(1.);
    }
    if preserve && node.text == "\n" {
        el = el.min_h(px(units.font.lh));
    }
    let inline = if skipped
        || editable
        || matches!(node.tag.as_str(), "svg" | "canvas" | "iframe" | "terminal")
    {
        None
    } else {
        inline_text().filter(|data| {
            !data.text.is_empty() || {
                let found = content_editable(doc, id);
                reader.log(
                    Query::Editable,
                    found.map_or(1, |node| reader.scope_of(id, node)),
                    hashed(found.is_some()),
                );
                found.is_some()
            }
        })
    };
    let hit_test = inline.is_some();
    let drag = style
        .get(Prop::APP_REGION)
        .is_some_and(|value| value == "drag");
    if hit.is_none()
        && (hit_test || node.tag == "#text")
        && !drag
        && {
            // Read the parent even with an explicit mode: an ancestor exit can set display: none without a
            // new revision.
            let (mode, origin) = styles.user_select[id];
            let origin = if origin == id {
                node.parent.unwrap_or(id)
            } else {
                origin
            };
            reader.log(
                Query::UserSelect,
                reader.scope_of(id, origin),
                hashed(mode as u8),
            );
            mode != style::UserSelect::None
        }
        && style.get(Prop::CURSOR).is_none_or(|value| value == "auto")
    {
        el = el.cursor(renderer::Cursor::Text);
    }
    let mut plans = 0;
    // The size of an inline box in a text as of the last frame, or a guess before it has one.
    let atom_slot =
        |atom: &style::Atom, child: &mut Child, containing: f32, pseudo: &dyn Fn() -> [f32; 3]| {
            let last = view
                .host
                .borrow()
                .dom
                .get(&view.id)
                .and_then(|dom| dom.atoms.get(&atom.key()).copied());
            reader.log(
                Query::Atom(atom.key() as u32),
                0,
                hashed(last.map(|size| size.map(f32::to_bits))),
            );
            let size = last.unwrap_or_else(|| {
                if atom.pseudo != 0 {
                    return pseudo();
                }
                let [_, max, _, height] = super::grid::intrinsic(
                    doc,
                    atom.node,
                    styles,
                    Some(containing),
                    None,
                    text,
                    &mut measured.borrow_mut(),
                );
                let line = styles.lengths[atom.node].font.lh;
                [max, height.max(line), line * 0.8]
            });
            super::text::AtomSlot {
                size,
                align: align_of(child),
                at: std::cell::Cell::new(point(px(0.), px(0.))),
            }
        };
    // The box that places an inline box where its text puts it, from the padding box of the text's box.
    let atom_box = |index: usize,
                    atom: &style::Atom,
                    child: Child,
                    slots: &Rc<[super::text::AtomSlot]>,
                    padding: [f32; 4]| {
        let mut wrapper = renderer::node()
            .absolute()
            .left(px(padding[3]))
            .top(px(padding[0]))
            .flex()
            .flex_row();
        wrapper.style().align_items = Some(renderer::AlignItems::Baseline);
        // A box that shrinks to its atom gives percent sizes of the atom nothing to resolve against.
        let atom_style = match atom.pseudo {
            0 => &*styles.values[atom.node],
            pseudo => &styles.pseudos[&id][usize::from(pseudo - 1)],
        };
        let rest = [Prop::WIDTH, Prop::MIN_WIDTH, Prop::MAX_WIDTH]
            .iter()
            .any(|&prop| {
                atom_style
                    .get(prop)
                    .is_some_and(|value| value.contains('%'))
            })
            .then(Rc::<std::cell::Cell<renderer::Pixels>>::default);
        wrapper = match &rest {
            Some(_) => wrapper.right(px(padding[1])),
            None => wrapper.max_w(renderer::Length::Calc(1., px(-padding[1] - padding[3]))),
        };
        wrapper = wrapper
            .hooks(super::text::AtomBox {
                slots: slots.clone(),
                index,
                node: atom.key(),
                view: view.clone(),
                rest: rest.clone(),
            })
            .child(child);
        if let Some(rest) = rest {
            wrapper = wrapper.child(
                renderer::node()
                    .flex_1()
                    .min_w(px(0.))
                    .hooks(super::text::AtomRest(rest)),
            );
        }
        wrapper
    };
    let cell = slot.is_some() && style::role(styles, id) == style::CELL;
    // As in browsers, a button centers its content in its height, and `align-content` aligns it in a block.
    // Content that overflows starts at the top, unless the value says `unsafe`.
    let content = match style.get(Prop::ALIGN_CONTENT) {
        _ if !flow || cell => None,
        _ if node.tag == "button" => Some(renderer::AlignContent::Center),
        _ if display == "inline" => None,
        Some(value) if value.ends_with("center") || value.starts_with("space-") => {
            (value != "space-between").then_some(renderer::AlignContent::Center)
        }
        Some(value) if value.ends_with("end") => Some(renderer::AlignContent::FlexEnd),
        _ => None,
    };
    if let Some(data) = inline {
        #[cfg(debug_assertions)]
        reader.extra.set(hashed(&*data.text));
        let atom_containing = content_box();
        // An absolute box at its static place in a line has an empty atom there, which gives it its place.
        let marker = |atom: &style::Atom| {
            style::static_inline(match atom.pseudo {
                0 => &styles.values[atom.node],
                pseudo => &styles.pseudos[&id][usize::from(pseudo - 1)],
            })
        };
        let mut atom_elements: Vec<renderer::Child> = data
            .atoms
            .iter()
            .map(|atom| match atom.pseudo {
                _ if marker(atom) => static_mark(view, atom.key()).into(),
                0 => element(build, atom.node, atom_containing, container, None),
                pseudo => pseudo_atoms[usize::from(pseudo - 1)]
                    .take()
                    .unwrap_or_default()
                    .into(),
            })
            .collect();
        let atom_slots: Rc<[super::text::AtomSlot]> = data
            .atoms
            .iter()
            .zip(&mut atom_elements)
            .map(|(atom, child)| {
                if marker(atom) {
                    // At the top of its line, as Chrome places the box.
                    return super::text::AtomSlot {
                        size: [0.; 3],
                        align: Some(renderer::AlignItems::FlexStart),
                        at: Default::default(),
                    };
                }
                atom_slot(atom, child, containing[0], &|| {
                    super::grid::pseudo_atom(
                        doc,
                        id,
                        usize::from(atom.pseudo - 1),
                        styles,
                        content_box()[0],
                        Some(containing[0]),
                        text,
                        pixel_ratio,
                    )
                    .unwrap_or_default()
                })
            })
            .collect();
        let text_box = style.get(Prop::TEXT_BOX).unwrap_or("");
        let trim = style.get(Prop::TEXT_BOX_TRIM).unwrap_or(text_box);
        let edge = style.get(Prop::TEXT_BOX_EDGE).unwrap_or(text_box);
        let (over, under) = (
            trim.contains("trim-both") || trim.contains("trim-start"),
            trim.contains("trim-both") || trim.contains("trim-end"),
        );
        let (mut trim_top, mut trim_bottom) = (0., 0.);
        if over || under {
            let system = build.text;
            let font = system.face(&parsed.face);
            let (size, height) = (px(units.font.em), px(units.font.lh));
            let baseline = f32::from(system.baseline(font, size, height, build.pixel_ratio));
            let [ascent, descent, _] = system.line_metrics(font, size).map(f32::from);
            if over {
                trim_top = baseline
                    - if edge.contains("cap") {
                        units.font.cap
                    } else if edge.contains("ex") {
                        units.font.ex
                    } else {
                        ascent
                    };
            }
            if under {
                trim_bottom = units.font.lh
                    - baseline
                    - if edge.contains("alphabetic") {
                        0.
                    } else {
                        descent
                    };
            }
        }
        // The floats of the paragraph come first, so layout places them before its lines. Out-of-flow boxes
        // that the text leaves out come after it.
        let mut placed = Vec::new();
        for &child in &flat {
            if floated(child) {
                let float = element(build, child, content_box(), container, None);
                el = el.child(float);
            } else if doc.nodes[child].parent == Some(id)
                && styles.values[child].get(Prop::DISPLAY) != Some("none")
                && style::placed_out_of_flow(&styles.values[child])
                && !top_layer.contains(&child)
            {
                placed.push(child);
            }
        }
        if data.greedy && hit.is_none() {
            el.text_style().set_text_wrap(renderer::TextWrap::Wrap);
        }
        let mut paragraph = renderer::Node::from(InlineText {
            data: data.clone(),
            mixed: data.mixed(build.text),
            atoms: atom_slots.clone(),
            view: view.clone(),
            shaped: Default::default(),
            prepainted: Default::default(),
            rtl,
            justify: style
                .get(Prop::TEXT_ALIGN)
                .is_some_and(|value| value == "justify")
                .then(|| {
                    style
                        .get(Prop::TEXT_ALIGN_LAST)
                        .is_some_and(|value| value == "justify")
                }),
        });
        let alone = el.kids().is_empty()
            && placed.is_empty()
            && data.lifted.is_empty()
            && data.atoms.is_empty()
            && trim_top == 0.
            && trim_bottom == 0.
            && after.is_empty()
            && !stacking
            && slot.is_none()
            && (flex_parent || display != "inline")
            && !rtl
            && line_justify.is_none()
            && content.is_none()
            && matches!(position, "static" | "relative")
            && el.style_ref().overflow.x == renderer::Overflow::Visible
            && el.style_ref().overflow.y == renderer::Overflow::Visible
            && super::TRANSFORM_PROPS
                .iter()
                .all(|&prop| style.get(prop).is_none_or(|value| value == "none"))
            && style
                .get(Prop::WRITING_MODE)
                .is_none_or(|mode| mode.starts_with("horizontal"));
        if !alone {
            paragraph = paragraph.id(("text", data.key));
        }
        paragraph.style().flow = renderer::Flow::BLOCK;
        // Text straight in a multi-column box breaks between its rows.
        let multicol = (flow && data.atoms.is_empty())
            .then(|| {
                let content = content_box();
                columns(style, &units, content[0])
                    .filter(|_| multicol)
                    .map(|set| (set, content))
            })
            .flatten();
        el = if let Some(((count, gap, rule), content)) = multicol {
            let paragraph = paragraph.min_w(px(0.)).flex_shrink(1.);
            el.child(column_set(
                style,
                content,
                count,
                gap,
                rule,
                true,
                [paragraph],
            ))
        } else if alone {
            el.adopt(paragraph)
        } else {
            el.child(
                paragraph
                    .min_w(px(0.))
                    .max_w(relative(1.))
                    .flex_shrink(1.)
                    .mt(px(-trim_top))
                    .mb(px(-trim_bottom)),
            )
        };
        for (index, (atom, child)) in data.atoms.iter().zip(atom_elements).enumerate() {
            let atom_box = atom_box(index, atom, child, &atom_slots, padding);
            el = el.child(match inline_fade(doc, styles, atom.node, id) {
                fade if fade < 1. => atom_box.hooks(super::paint::Effected {
                    effect: renderer::PaintEffect {
                        opacity: fade,
                        ..Default::default()
                    },
                    backdrop: None,
                }),
                _ => atom_box,
            });
        }
        for child in placed {
            // The containing block of a positioned child is the padding box.
            let content = content_box();
            let containing = match styles.values[child].get(Prop::POSITION) {
                Some("fixed") if !made.fixed => content,
                Some("absolute") if !made.absolute => content,
                _ => [
                    content[0] + padding[1] + padding[3],
                    content[1] + padding[0] + padding[2],
                ],
            };
            el = el.child(element(build, child, containing, container, None));
        }
        // Out-of-flow boxes inside the inline boxes of the text, and at their static place: they lay out beside it,
        // with the inline boxes around them as ancestors.
        for &(node, pseudo) in &data.lifted {
            if top_layer.contains(&node) {
                continue;
            }
            let path: Vec<usize> =
                std::iter::successors(doc.nodes[node].parent, |&up| doc.nodes[up].parent)
                    .take_while(|&up| up != id)
                    .collect();
            let lifted = Build {
                ancestors: build
                    .ancestors
                    .iter()
                    .copied()
                    .chain(path.into_iter().rev())
                    .collect(),
                flex_parent: false,
                perspective: None,
                zone: None,
                #[cfg(debug_assertions)]
                extra: Default::default(),
                #[cfg(debug_assertions)]
                plan: Default::default(),
                ..*build
            };
            let content = content_box();
            let padded = [
                content[0] + padding[1] + padding[3],
                content[1] + padding[0] + padding[2],
            ];
            let containing = if made.absolute { padded } else { content };
            el = el.child(match pseudo {
                0 => element(&lifted, node, containing, container, None),
                pseudo => create(
                    &lifted,
                    node,
                    containing,
                    container,
                    None,
                    Some(usize::from(pseudo - 1)),
                ),
            });
        }
    } else if drawable {
        // The children of a canvas with drawable content lay out as blocks at its content box, and take no room
        // in the canvas. The window doesn't show them: the page draws them with `drawElementImage()`.
        let mut content = content_box();
        for axis in 0..2 {
            if style
                .get([Prop::WIDTH, Prop::HEIGHT][axis])
                .is_none_or(|value| value == "auto")
                && let Some(natural) = natural
            {
                content[axis] = natural[axis];
            }
        }
        let boxes = flat
            .iter()
            .map(|&child| element(build, child, content, container, None));
        let holder = renderer::node()
            .absolute()
            .flex()
            .flex_col()
            .top(px(padding[0]))
            .right(px(padding[1]))
            .left(px(padding[3]))
            .children(boxes);
        el = el.child(holder.hooks(super::paint::Undrawn(view.clone())));
    } else if (!node.children.is_empty() || items.iter().any(Option::is_some))
        && !skipped
        && node.tag != "canvas"
        && node.tag != "terminal"
        && node.tag != "iframe"
        && node.tag != "svg"
        && !editable
    {
        let mut content = content_box();
        let container = own.unwrap_or(container);
        // Content-sized heights size a grid like `auto`.
        let auto_height = style
            .get(Prop::HEIGHT)
            .is_none_or(|v| matches!(v, "auto" | "fit-content" | "max-content" | "min-content"));
        let flexed = style
            .get(Prop::DISPLAY)
            .is_some_and(|v| v.ends_with("grid"))
            && slot.is_none()
            && auto_height
            && reader
                .parent(id, Prop::DISPLAY)
                .is_some_and(|v| v.ends_with("flex"))
            && if reader
                .parent(id, Prop::FLEX_DIRECTION)
                .is_some_and(|v| v.starts_with("column"))
            {
                style
                    .get(Prop::FLEX_GROW)
                    .and_then(|v| v.parse::<f32>().ok())
                    .is_some_and(|v| v > 0.)
                    || style
                        .get(Prop::FLEX_BASIS)
                        .is_some_and(|v| v != "auto" && v != "content")
            } else {
                // A flex row stretches the item to its line.
                let stretched = match style.get(Prop::ALIGN_SELF).unwrap_or("auto") {
                    "auto" => reader
                        .parent(id, Prop::ALIGN_ITEMS)
                        .is_none_or(|v| matches!(v, "normal" | "stretch")),
                    value => matches!(value, "normal" | "stretch"),
                };
                stretched
                    && [Prop::MARGIN_TOP, Prop::MARGIN_BOTTOM]
                        .into_iter()
                        .all(|prop| style.get(prop) != Some("auto"))
            };
        let shrunk = style.get(Prop::DISPLAY).is_some_and(|v| v == "grid")
            && slot.is_none()
            && ((style::table_box(style::role(styles, id))
                || matches!(position, "absolute" | "fixed"))
                && style.get(Prop::WIDTH) == Some("fit-content")
                // Like Chrome, a button is as wide as its content, also as a block.
                || style.get(Prop::WIDTH).is_none_or(|v| v == "auto") && (node.tag == "button" || {
                    let (shrunk, farthest) = shrinks(doc, styles, id, true);
                    reader.log(Query::Shrunk, reader.scope_of(id, farthest), hashed(shrunk));
                    shrunk
                }));
        let mut widest = None;
        let fills = style.get(Prop::DISPLAY).is_some_and(|v| v == "grid")
            && !shrunk
            && slot.is_none()
            && !flexed
            && style
                .get(Prop::WIDTH)
                .is_none_or(|v| matches!(v, "auto" | "100%"))
            && !matches!(position, "absolute" | "fixed")
            && node.parent.is_some()
            && match reader.parent(id, Prop::DISPLAY).unwrap_or("block") {
                "block" | "flow-root" | "list-item" => true,
                "flex" | "inline-flex" => {
                    reader
                        .parent(id, Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v.starts_with("column"))
                        && reader
                            .parent(id, Prop::ALIGN_ITEMS)
                            .is_none_or(|v| matches!(v, "normal" | "stretch"))
                        && style
                            .get(Prop::ALIGN_SELF)
                            .is_none_or(|v| matches!(v, "auto" | "stretch"))
                }
                _ => false,
            };
        // The height that insets or a ratio give a grid, which holds even without rows.
        let mut definite = 0f32;
        let grid = if let Some(display) = style
            .get(Prop::DISPLAY)
            .filter(|v| matches!(*v, "grid" | "inline-grid"))
        {
            if display == "inline-grid"
                && slot.is_none()
                && style.get(Prop::WIDTH).is_none_or(|v| v == "auto")
            {
                let extent =
                    padding[1] + padding[3] + borders[1] + borders[3] + margin[1] + margin[3];
                let [min, max, ..] = super::grid::intrinsic(
                    doc,
                    id,
                    styles,
                    None,
                    None,
                    text,
                    &mut measured.borrow_mut(),
                );
                content[0] = (max.min(content[0] + extent).max(min) - extent).max(0.);
            }
            let horizontal = padding[1] + padding[3] + borders[1] + borders[3];
            let vertical = padding[0] + padding[2] + borders[0] + borders[2];
            let mut tracked = [None; 2];
            if shrunk {
                let [min, max, ..] = super::grid::intrinsic(
                    doc,
                    id,
                    styles,
                    None,
                    None,
                    text,
                    &mut measured.borrow_mut(),
                );
                let [min, max] =
                    [min, max].map(|value| (value - horizontal - margin[1] - margin[3]).max(0.));
                widest = Some([min, max]);
                let last = {
                    let mut host = view.host.borrow_mut();
                    let dom = host.dom.entry(view.id).or_default();
                    let same = dom.grid_widest.insert(id, max) == Some(max);
                    same.then(|| dom.cull.widths.get(&id).map(|size| size[0].max(0.)))
                        .flatten()
                };
                // Laid-out bounds are in layout units, so a fitting grid may look narrower.
                content[0] = last.map_or(content[0].min(max), |last| {
                    if last + 0.5 >= max {
                        last.max(max)
                    } else {
                        last
                    }
                });
                tracked[0] = Some((last.unwrap_or(content[0]), horizontal));
            }
            if fills {
                if let Some(last) = view
                    .host
                    .borrow()
                    .dom
                    .get(&view.id)
                    .and_then(|dom| dom.cull.widths.get(&id))
                {
                    content[0] = last[0].max(0.);
                }
                tracked[0] = Some((content[0], horizontal));
                // A table is never narrower than its content, which also sets the minimum of flex items.
                // A `<table>` box scrolls the table inside, so it can be narrower.
                if matches!(style::role(styles, id), 1 | 2 | 10..=12)
                    && style.get(Prop::MIN_WIDTH).is_none()
                {
                    let [min, ..] = super::grid::intrinsic(
                        doc,
                        id,
                        styles,
                        None,
                        None,
                        text,
                        &mut measured.borrow_mut(),
                    );
                    el = el.min_w(px(layout_ceil(
                        (min - margin[1] - margin[3]).max(0.),
                        pixel_ratio,
                    )));
                }
            }
            let inset = |prop| {
                style
                    .get(prop)
                    .and_then(|value| units.resolve(value, containing[1]))
            };
            let stretched = (matches!(position, "absolute" | "fixed")
                && style.get(Prop::HEIGHT).is_none_or(|v| v == "auto"))
            .then(|| Some(inset(Prop::TOP)? + inset(Prop::BOTTOM)?))
            .flatten();
            // Like Chrome, a ratio sizes the box of `box-sizing` from the used width, and the rows can make it taller.
            let ratio = style
                .get(Prop::ASPECT_RATIO)
                .filter(|_| auto_height && slot.is_none())
                .and_then(aspect_ratio)
                .map(|ratio| {
                    let border_box = style.get(Prop::BOX_SIZING) == Some("border-box");
                    let extra = if border_box {
                        [horizontal, vertical]
                    } else {
                        [0.; 2]
                    };
                    let height = (content[0] + extra[0]) / ratio;
                    let limit = |prop| {
                        style
                            .get(prop)
                            .and_then(|v| units.resolve(v, containing[1]))
                    };
                    let height = height.min(limit(Prop::MAX_HEIGHT).unwrap_or(f32::INFINITY));
                    (height.max(limit(Prop::MIN_HEIGHT).unwrap_or(0.)) - extra[1]).max(0.)
                });
            let height =
                if slot.is_some() || !auto_height {
                    Some(content[1])
                } else if ratio.is_some() {
                    definite = ratio.unwrap_or_default();
                    ratio
                } else if let Some(insets) = stretched {
                    definite = (containing[1] - insets - margin[0] - margin[2] - vertical).max(0.);
                    Some(definite)
                } else if flexed {
                    let height =
                        view.host.borrow().dom.get(&view.id).and_then(|state| {
                            state.cull.widths.get(&id).map(|size| size[1].max(0.))
                        });
                    tracked[1] = Some((height.unwrap_or(f32::NAN), vertical));
                    height
                } else {
                    None
                };
            let mut host = view.host.borrow_mut();
            let dom = host.dom.entry(view.id).or_default();
            for (axis, track) in tracked.iter().enumerate() {
                if let Some((used, _)) = track {
                    reader.log(
                        Query::Track(id as u32, axis as u8),
                        0,
                        used.to_bits().into(),
                    );
                }
            }
            reader.log(
                Query::Baselines(id as u32),
                0,
                baselines(doc, Some(&*dom), id),
            );
            if tracked.iter().any(Option::is_some) {
                dom.flexed_grids.insert(id, tracked);
            } else {
                dom.flexed_grids.remove(&id);
            }
            drop(host);
            let cached = view
                .host
                .borrow()
                .dom
                .get(&view.id)
                .and_then(|state| state.grids.get(&id))
                .filter(|(width, cached, inherited, _)| {
                    *width == content[0]
                        && *cached == height
                        && inherited.as_ref() == slot.as_ref().map(|(_, grid)| grid)
                })
                .map(|(_, _, _, grid)| grid.clone());
            Some(cached.unwrap_or_else(|| {
                let grid = super::grid::layout(
                    doc,
                    id,
                    styles,
                    Some(content[0]),
                    height,
                    slot.as_ref().map(|(_, grid)| grid),
                    view.host
                        .borrow()
                        .dom
                        .get(&view.id)
                        .map(|dom| &dom.baselines),
                    text,
                    &mut measured.borrow_mut(),
                );
                view.host
                    .borrow_mut()
                    .dom
                    .entry(view.id)
                    .or_default()
                    .grids
                    .insert(
                        id,
                        (
                            content[0],
                            height,
                            slot.as_ref().map(|(_, grid)| grid.clone()),
                            grid.clone(),
                        ),
                    );
                grid
            }))
        } else {
            None
        };
        if let Some(grid) = &grid {
            let mut host = view.host.borrow_mut();
            let tracks = &mut host.dom.entry(view.id).or_default().tracks;
            if tracks
                .get(&id)
                .is_none_or(|[columns, rows]| **columns != *grid.columns || **rows != *grid.rows)
            {
                tracks.insert(
                    id,
                    [grid.columns.as_slice().into(), grid.rows.as_slice().into()],
                );
            }
            drop(host);
            el = el.flex().flex_col();
            for (index, item) in items.iter_mut().enumerate() {
                let key = !(id * 2 + index);
                let Ok(at) = grid.cells.binary_search_by_key(&key, |cell| cell.0) else {
                    continue;
                };
                let Some((order, mut decoration, mut margin)) = item.take() else {
                    continue;
                };
                // Like a grid item, a pseudo-element takes percentages from its grid area.
                if let Some(pseudo) = styles
                    .pseudos
                    .get(&id)
                    .and_then(|pseudos| pseudos.get(index))
                {
                    let base = grid.area(grid.cells[at].1)[0];
                    let pseudo_units = pseudo_lengths(&units, style, pseudo, text, pixel_ratio);
                    let [padding, borders] = [
                        pseudo_units
                            .edges(pseudo, style::PADDING, base)
                            .map(|v| v.max(0.)),
                        pseudo_units.borders(pseudo, base),
                    ];
                    margin = pseudo_units.edges(pseudo, style::MARGIN, base);
                    let own = decoration.style();
                    own.padding = renderer::Edges {
                        top: px(padding[0]).into(),
                        right: px(padding[1]).into(),
                        bottom: px(padding[2]).into(),
                        left: px(padding[3]).into(),
                    };
                    let side =
                        |at: usize, old: renderer::Length| match pseudo.get(style::MARGIN[at]) {
                            Some("auto") => old,
                            _ => px(margin[at]).into(),
                        };
                    own.margin = renderer::Edges {
                        top: side(0, own.margin.top),
                        right: side(1, own.margin.right),
                        bottom: side(2, own.margin.bottom),
                        left: side(3, own.margin.left),
                    };
                    own.border_widths = renderer::Edges {
                        top: px(borders[0]),
                        right: px(borders[1]),
                        bottom: px(borders[2]),
                        left: px(borders[3]),
                    };
                }
                let mut bounds = grid.cells[at].2;
                bounds[0] += padding[3];
                bounds[1] += padding[0];
                let Child::Node(placed) = place(decoration.into(), margin, pixel_ratio, bounds)
                else {
                    continue;
                };
                // `::after` paints after the children.
                if index == 0 {
                    el = el.child(*placed);
                } else {
                    *item = Some((order, *placed, margin));
                }
            }
            el = match widest {
                Some(widest) => el.child(renderer::node().measure(
                    widest,
                    |&[min, max], _, known, available, _| {
                        let width = known.width.unwrap_or(match available.width {
                            renderer::AvailableSpace::MinContent => px(min),
                            renderer::AvailableSpace::MaxContent => px(max),
                            renderer::AvailableSpace::Definite(width) => {
                                width.min(px(max)).max(px(min))
                            }
                        });
                        (renderer::size(width, px(0.)), None)
                    },
                )),
                None if fills => el,
                None => el.w(px(content[0]
                    + padding[1]
                    + padding[3]
                    + borders[1]
                    + borders[3])),
            };
            let rows = grid.extent(1).max(definite);
            let vertical = padding[0] + padding[2] + borders[0] + borders[2];
            if auto_height && !flexed {
                el = el.h(px(rows + vertical));
                el.style().aspect_ratio = None;
            }
            // The items are out of flow, so the rows give the min-content height for `min-height: auto` in a column.
            if !flexed
                && reader
                    .parent(id, Prop::FLEX_DIRECTION)
                    .is_some_and(|v| v.starts_with("column"))
                && reader
                    .parent(id, Prop::DISPLAY)
                    .is_some_and(|v| v.ends_with("flex"))
            {
                el = el.child(renderer::node().h(px(rows)));
            }
            // Absolute grid items need an in-flow content-height box for flex basis and min-height: auto.
            // Previously stretched rows cannot provide it.
            if flexed {
                let [.., height] = super::grid::intrinsic(
                    doc,
                    id,
                    styles,
                    Some(
                        content[0]
                            + padding[1]
                            + padding[3]
                            + borders[1]
                            + borders[3]
                            + margin[1]
                            + margin[3],
                    ),
                    None,
                    text,
                    &mut measured.borrow_mut(),
                );
                el = el.child(
                    renderer::node().h(px((height - vertical - margin[0] - margin[2]).max(0.))),
                );
            }
            if !flexed && made.absolute {
                blocks.borrow_mut().insert(
                    id,
                    [
                        content[0] + padding[1] + padding[3],
                        if auto_height { rows } else { content[1] } + padding[0] + padding[2],
                    ],
                );
            }
        }
        let mut children = if grid.is_some() {
            node.children.clone()
        } else {
            flat.clone()
        };
        // Each level of boxes is one large native frame, so levels past the stack budget are not drawn.
        let marker = 0u8;
        if (&raw const marker as usize) < STACK_LIMIT.with(|limit| *limit) {
            children.clear();
        }
        if grid.is_some() {
            children.sort_by_key(|child| {
                styles.values[*child]
                    .get(Prop::Z_INDEX)
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            });
        } else if flexbox {
            let order = |child: &usize| {
                styles.values[*child]
                    .get(Prop::ORDER)
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            };
            if children.iter().any(|child| order(child) != 0) {
                children.sort_by_key(order);
            }
        }
        let inline_element = |child: Option<&usize>| {
            child.is_some_and(|&child| {
                let node = &doc.nodes[child];
                node.tag == "#text" && !node.text.trim_ascii().is_empty()
                    || !node.tag.starts_with('#') && node.tag != "br" && inline_level(child)
            })
        };
        let multicol = columns(style, &units, content[0]).filter(|_| multicol);
        let mut line_strut = false;
        let open_line = |decoration: renderer::Node, line_strut: &mut bool| {
            let (first, rest, aligned) = with_strut(decoration.into(), line_justify);
            *line_strut = aligned;
            let mut next = renderer::node()
                .flex()
                .flex_row()
                .flex_wrap()
                .child(first)
                .children(rest);
            next.style().justify_content = line_justify;
            next.style().align_items = line_align;
            next.style().inline = renderer::Inline::Lines;
            next
        };
        let [lead, tail] = std::mem::take(&mut lines);
        let mut line = None;
        if let Some(lead) = lead {
            if grid.is_some() {
                el = el.child(lead);
            } else {
                line = Some(open_line(lead, &mut line_strut));
            }
        }
        let mut lone = (0, None);
        let mut joined = usize::MAX;
        let mut floating = Vec::new();
        let columns = multicol.filter(|_| flow && grid.is_none() && !all_inline);
        let mut collected: Vec<(Option<usize>, Child)> = Vec::new();
        let shown = |index: usize| {
            // Columns only size and paint the cells of a table.
            if grid.is_some()
                && style::table_box(style::role(styles, id))
                && (matches!(doc.nodes[children[index]].tag.as_str(), "col" | "colgroup")
                    || styles.values[children[index]]
                        .get(Prop::DISPLAY)
                        .is_some_and(|v| v.starts_with("table-column")))
            {
                return false;
            }
            // A plain block is an element, and its flags are closer than its node.
            if info[children[index]].flags & cull::PLAIN_BLOCK != 0 {
                return true;
            }
            let child = &doc.nodes[children[index]];
            if (flexbox || grid.is_some()) && child.tag == "#text" {
                return measured.borrow().text_item(doc, children[index], preserve);
            }
            preserve
                || !(child.tag == "#text" && child.text.trim_ascii().is_empty())
                || flow
                    && grid.is_none()
                    && inline_element(index.checked_sub(1).and_then(|index| children.get(index)))
                    && inline_element(children.get(index + 1))
        };
        let wrap = display == "flex"
            && style.get(Prop::FLEX_DIRECTION).is_none_or(|v| v == "row")
            && style.get(Prop::FLEX_WRAP).is_some_and(|v| v == "wrap")
            && style
                .get(Prop::ALIGN_CONTENT)
                .is_none_or(|v| matches!(v, "normal" | "stretch" | "flex-start" | "start"))
            && style
                .get(Prop::ALIGN_ITEMS)
                .is_none_or(|v| !v.contains("baseline"))
            && [Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT]
                .into_iter()
                .all(|name| {
                    style
                        .get(name)
                        .is_none_or(|v| matches!(v, "auto" | "none" | "0" | "0px"))
                })
            && !flex_parent
            && slot.is_none()
            && info[id].flags & cull::CHILDREN_CHANGED == 0;
        let stacks = grid.is_none()
            && columns.is_none()
            && !all_inline
            && display != "inline"
            && if flexbox {
                // An ordered pseudo-element goes between the children, maybe inside a run.
                items.iter().flatten().all(|(order, ..)| *order == 0)
                    && (wrap
                        || style
                            .get(Prop::FLEX_DIRECTION)
                            .is_some_and(|v| v == "column")
                            && style.get(Prop::FLEX_WRAP).is_none_or(|v| v == "nowrap")
                            && style.get(Prop::JUSTIFY_CONTENT).is_none_or(|v| {
                                matches!(v, "normal" | "flex-start" | "start" | "stretch")
                            }))
            } else {
                // Floats are uncullable, so a run of blocks becomes a spacer only between them.
                flow
            };
        plans = if info[id].flags & cull::MOVES_CONTENT != 0 {
            u8::MAX
        } else if grid.is_some() || stacks && (!flexbox || wrap) {
            1
        } else {
            2 * u8::from(stacks)
        };
        let stack = build.zone.clone().filter(|_| stacks);
        #[cfg(debug_assertions)]
        let checked = build
            .checking
            .then(|| view.kept.borrow().get(&id).map(|kept| kept.plan.clone()))
            .flatten();
        #[cfg(debug_assertions)]
        let mut culled_ids = Vec::new();
        let mut plan = None;
        if let Some(zone) = stack {
            let host = view.host.borrow();
            if let Some(dom) = host.dom.get(&view.id)
                && let Some(&[left, ..]) = dom.bounds.get(&id)
                && let Some(&[width, _]) = dom.cull.widths.get(&id)
            {
                let gap = if flexbox {
                    style
                        .get(Prop::ROW_GAP)
                        .and_then(|v| units.resolve(v, containing[0]))
                        .unwrap_or(0.)
                        .max(0.)
                } else {
                    0.
                };
                let (slots, mut runs) = if wrap {
                    cull::lines(
                        &children,
                        |index| {
                            shown(index) && info[children[index]].flags & cull::OUT_OF_FLOW == 0
                        },
                        |child| {
                            let placed = *dom.cull.placed.get(child)?;
                            let bounds = dom.bounds.get(&child)?;
                            let [top, _, bottom, left] = placed.2;
                            let member = cull::member(
                                info[child],
                                placed,
                                Some(bounds),
                                &zone,
                                units.viewport[1],
                                false,
                            );
                            Some((
                                member.is_some(),
                                [
                                    bounds[0] - left,
                                    bounds[1] - top,
                                    bounds[0] - left + placed.1[0],
                                    bounds[1] + bounds[3] + bottom,
                                ],
                            ))
                        },
                        gap,
                    )
                } else {
                    cull::plan(
                        &children,
                        shown,
                        |child| {
                            cull::member(
                                info[child],
                                dom.cull.placed.get(child).copied().unwrap_or_default(),
                                dom.bounds.get(&child),
                                &zone,
                                units.viewport[1],
                                flexbox,
                            )
                            .map(|(bounds, margins)| {
                                (bounds, margins, clear_bits(&styles.values[child]))
                            })
                        },
                        gap,
                    )
                };
                if wrap {
                    let mut watch = Vec::new();
                    for (index, &child) in children.iter().enumerate() {
                        match slots[index] {
                            0 => {
                                let placed =
                                    dom.cull.placed.get(child).copied().unwrap_or_default();
                                if placed.0 != info[child].revision {
                                    watch.push((child, placed.1[0]));
                                }
                            }
                            slot => {
                                if let Some(run) = runs[slot - 1]
                                    .as_mut()
                                    .filter(|run| run.members[0] == child)
                                {
                                    run.width = width;
                                    run.watch = std::mem::take(&mut watch);
                                }
                            }
                        }
                    }
                }
                if !flexbox {
                    let collapsed = |child: usize, side: usize| {
                        let mut units = styles.lengths[child];
                        units.container = Some(container);
                        margins(build, child, &units, content[0], false, false)[side]
                    };
                    for run in runs.iter_mut().flatten() {
                        run.margins = [
                            collapsed(run.members[0], 0),
                            collapsed(run.members[run.members.len() - 1], 2),
                        ];
                    }
                }
                // The x of the content: the box moves its blocks, and so does its own scroll.
                let x = left + f32::from(dom.mirror_offset(id).x);
                plan = (!runs.is_empty()).then_some((slots, runs, [x, width], zone));
            }
        }
        let mut culled = Vec::new();
        if let (Some(grid), Some(zone)) = (&grid, &build.zone) {
            if let Some(dom) = view.host.borrow().dom.get(&view.id) {
                for &(child, _, [x, y, width, height], _) in &grid.cells {
                    if super::grid::pseudo_item(child).is_some() {
                        continue;
                    }
                    let placed = dom.cull.placed.get(child).copied().unwrap_or_default();
                    let [top, right, bottom, left] = placed.2;
                    // Compare unsnapped cell sizes: layout snaps them later.
                    if placed.1 == [width, height]
                        && cull::member(
                            info[child],
                            placed,
                            dom.bounds.get(&child),
                            zone,
                            units.viewport[1],
                            false,
                        )
                        .is_some()
                    {
                        // Layout truncates each inset and margin to layout units, so the offsets from the anchor do too.
                        let snap =
                            |value: f32| renderer::to_layout_unit(value, pixel_ratio) / pixel_ratio;
                        culled.push((
                            child,
                            [
                                snap(x + padding[3]) + snap(left),
                                snap(y + padding[0]) + snap(top),
                                width - left - right,
                                height - top - bottom,
                            ],
                        ));
                    }
                }
            }
            if !culled.is_empty() {
                culls.set(culls.get() + 1);
                #[cfg(debug_assertions)]
                culled_ids.extend(culled.iter().map(|&(child, _)| child));
                el = el.child(cull::anchor(
                    culled.clone(),
                    build.ancestors.clone(),
                    zone.clone(),
                    view.clone(),
                ));
            }
            if let Some(baseline) = grid.first_baseline.filter(|_| {
                grid.first_item
                    .is_some_and(|item| culled.binary_search_by_key(&item, |&(id, _)| id).is_ok())
            }) {
                let baseline = baseline.max(0.);
                let mut line = renderer::node()
                    .w(px(0.))
                    .h(px(baseline))
                    .mb(px(-baseline))
                    .flex_shrink_0()
                    .baseline(renderer::Baseline::Bottom);
                line.style().baseline_source = true;
                el = el.child(line);
            }
        }
        macro_rules! place {
            ($element:expr, $child:expr) => {{
                let element: renderer::Child = $element.into();
                if std::mem::take(&mut strut_next) && columns.is_none() {
                    el = el.child(strut());
                }
                if std::mem::take(&mut strut_due) {
                    el = el.child(strut());
                }
                match columns {
                    Some(_) => collected.push(($child, element)),
                    None => el.push(element),
                }
            }};
        }
        let line_break = || {
            let mut el = renderer::node().w_full();
            el.style().align_self = Some(renderer::AlignItems::FlexStart);
            el
        };
        let hangs = flow
            && !preserve
            && style
                .get(Prop::WHITE_SPACE)
                .is_none_or(|value| value != "nowrap")
            && style
                .get(Prop::TEXT_WRAP)
                .is_none_or(|value| !value.contains("nowrap"));
        el.reserve(children.len());
        let mut item_margins =
            rustc_hash::FxHashMap::<(*const style::Style, u32), [f32; 4]>::default();
        let (mut scanned, mut last_flow, mut seen, mut flowed) = (0, None, 0, false);
        if children.len() == flat.len() {
            for &(cut, from, to) in &cuts {
                let mut at = from;
                while at < to {
                    let blocky = |child: usize| {
                        !in_run(child)
                            && !floated(child)
                            && info[child].flags & cull::OUT_OF_FLOW == 0
                    };
                    if !blocky(children[at]) {
                        at += 1;
                        continue;
                    }
                    let mut last = at;
                    while last + 1 < to
                        && (blocky(children[last + 1]) || !in_run(children[last + 1]))
                    {
                        last += 1;
                    }
                    while !blocky(children[last]) {
                        last -= 1;
                    }
                    let key = |at: usize| match cut_pseudos.binary_search_by_key(&at, |part| part.0)
                    {
                        Ok(found) => style::atom_key(children[at], cut_pseudos[found].1 as u8 + 1),
                        Err(_) => children[at],
                    };
                    cut_parts.push((
                        cut,
                        2 * at + 1,
                        super::paint::Piece::Blocks(key(at), key(last), {
                            let last = children[last];
                            styles.lengths[last].edges(&styles.values[last], style::MARGIN, 0.)[2]
                        }),
                    ));
                    at = last + 1;
                }
            }
        }
        // A spliced box, and an inline box in the text, has no box of its own, so its blocks and atoms take its
        // opacity.
        let fade = |child: usize| inline_fade(doc, styles, child, id);
        let faded = |child: usize, element: Child| {
            let fade = fade(child);
            match fade < 1. {
                true => renderer::node()
                    .flex()
                    .flex_col()
                    .child(element)
                    .hooks(super::paint::Effected {
                        effect: renderer::PaintEffect {
                            opacity: fade,
                            ..Default::default()
                        },
                        backdrop: None,
                    })
                    .into(),
                false => element,
            }
        };
        let (mut run_end, mut tried) = (0, 0);
        for index in 0..=children.len() {
            if index < run_end {
                continue;
            }
            let child = children.get(index).copied();
            let paragraphs = index >= tried
                && flow
                && !all_inline
                && !flexbox
                && grid.is_none()
                && line.is_none()
                && culled.is_empty();
            // The edges of spliced boxes next to a block and no other line take a line of their own.
            let edges = if paragraphs
                && !cuts.is_empty()
                && child.is_none_or(|child| !in_run(child))
                && (index == 0 || !in_run(children[index - 1]))
            {
                run_cuts(build, &cuts, &children, [index, index], in_run)
            } else {
                Vec::new()
            };
            if paragraphs && (child.is_some_and(in_run) || !edges.is_empty()) {
                let end = index
                    + children[index..]
                        .iter()
                        .take_while(|&&kid| in_run(kid))
                        .count();
                tried = end;
                let run = &children[index..end];
                let (found, parts) = run_text(index, end);
                if let Some(data) = found {
                    #[cfg(debug_assertions)]
                    reader.extra.set(hashed((reader.extra.get(), &*data.text)));
                    cut_parts
                        .extend((parts.iter()).map(|&(cut, _)| {
                            (cut, 2 * index, super::paint::Piece::Text(data.key))
                        }));

                    let content = content_box();
                    let mut atom_elements: Vec<Child> = data
                        .atoms
                        .iter()
                        .map(|atom| match atom.pseudo {
                            0 => element(build, atom.node, content, container, None),
                            pseudo => pseudo_atoms[usize::from(pseudo - 1)]
                                .take()
                                .unwrap_or_default()
                                .into(),
                        })
                        .collect();
                    let atom_slots: Rc<[super::text::AtomSlot]> = data
                        .atoms
                        .iter()
                        .zip(&mut atom_elements)
                        .map(|(atom, child)| atom_slot(atom, child, content[0], &|| [0.; 3]))
                        .collect();
                    let mut paragraph = renderer::Node::from(InlineText {
                        data: data.clone(),
                        mixed: data.mixed(build.text),
                        atoms: atom_slots.clone(),
                        view: view.clone(),
                        shaped: Default::default(),
                        prepainted: Default::default(),
                        rtl,
                        justify: style
                            .get(Prop::TEXT_ALIGN)
                            .is_some_and(|value| value == "justify")
                            .then(|| {
                                style
                                    .get(Prop::TEXT_ALIGN_LAST)
                                    .is_some_and(|value| value == "justify")
                            }),
                    });
                    // Text keeps its box when runs come before it, so it can stay a scroll anchor.
                    paragraph = paragraph.id(("text", data.key));
                    paragraph.style().flow = renderer::Flow::BLOCK;
                    if data.greedy {
                        paragraph
                            .text_style()
                            .set_text_wrap(renderer::TextWrap::Wrap);
                    }
                    let mut block = renderer::node()
                        .flex()
                        .flex_col()
                        .child(paragraph.min_w(px(0.)).max_w(relative(1.)).flex_shrink(1.));
                    for (at, (atom, child)) in data.atoms.iter().zip(atom_elements).enumerate() {
                        let atom_box = atom_box(at, atom, child, &atom_slots, [0.; 4]);
                        block = block.child(match fade(atom.node) {
                            fade if fade < 1. => atom_box.hooks(super::paint::Effected {
                                effect: renderer::PaintEffect {
                                    opacity: fade,
                                    ..Default::default()
                                },
                                backdrop: None,
                            }),
                            _ => atom_box,
                        });
                    }
                    let struts = (
                        std::mem::take(&mut strut_due),
                        std::mem::take(&mut strut_next),
                    );
                    // Floats of the line come first, so layout places them before it.
                    for &kid in run.iter().filter(|&&kid| floated(kid)) {
                        place!(
                            element(build, kid, content_box(), container, None),
                            Some(kid)
                        );
                    }
                    place!(block, None);
                    (strut_due, strut_next) = struts;
                    if !run.is_empty() {
                        run_end = end;
                        continue;
                    }
                } else {
                    // A run that lays out as boxes gives the spliced boxes in it the bounds of their boxes.
                    for &(cut, from, to) in &cuts {
                        let nodes: Vec<usize> = (from.max(index)..to.min(end))
                            .map(|at| children[at])
                            .collect();
                        if !nodes.is_empty() {
                            cut_parts.push((cut, 2 * index, super::paint::Piece::Nodes(nodes)));
                        }
                    }
                }
            }
            let Some(child) = child else {
                break;
            };
            if let Ok(found) = cut_pseudos.binary_search_by_key(&index, |part| part.0) {
                if let Some(open) = line.take() {
                    place!(open, None);
                }
                let part = cut_pseudos[found].1;
                let decoration = create(build, child, content_box(), container, None, Some(part));
                // The pseudo-element is in its box, which `faded` leaves out.
                let fade = super::effects::opacity(&styles.values[child]);
                let decoration = match fade < 1. {
                    true => renderer::node()
                        .flex()
                        .flex_col()
                        .child(decoration)
                        .hooks(super::paint::Effected {
                            effect: renderer::PaintEffect {
                                opacity: fade,
                                ..Default::default()
                            },
                            backdrop: None,
                        })
                        .into(),
                    false => decoration,
                };
                place!(faded(child, decoration), None);
                continue;
            }
            // Flex places a pseudo-element before the children of a higher order, and `::before` also before
            // those of its own.
            if flexbox && items.iter().any(Option::is_some) {
                let order = styles.values[child]
                    .get(Prop::ORDER)
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0);
                let first = usize::from(
                    items[1]
                        .as_ref()
                        .zip(items[0].as_ref())
                        .is_some_and(|((after, ..), (before, ..))| after < before),
                );
                for pending in [first, 1 - first] {
                    if items[pending]
                        .as_ref()
                        .is_some_and(|(own, ..)| *own < order || pending == 0 && *own == order)
                        && let Some((_, decoration, _)) = items[pending].take()
                    {
                        place!(decoration, None);
                    }
                }
            }
            if index != joined && shown(index) {
                if let Some((slots, runs, then, zone)) = plan.as_mut()
                    && let Some(&slot) = slots.get(index).filter(|&&slot| slot > 0)
                {
                    if let Some(run) = runs[slot - 1].take() {
                        culls.set(culls.get() + 1);
                        #[cfg(debug_assertions)]
                        culled_ids.extend(&run.members);
                        if let Some(open) = line.take() {
                            place!(open, None);
                        }
                        place!(
                            cull::spacer(
                                run,
                                build.ancestors.clone(),
                                *then,
                                zone.clone(),
                                view.clone()
                            ),
                            None
                        );
                    }
                    continue;
                }
                if culled.binary_search_by_key(&child, |&(id, _)| id).is_ok()
                    || top_layer.contains(&child)
                {
                    continue;
                }
                #[cfg(debug_assertions)]
                if checked
                    .as_ref()
                    .is_some_and(|ids| ids.binary_search(&child).is_ok())
                {
                    continue;
                }
                let placement = grid.as_ref().and_then(|grid| {
                    let index = grid
                        .cells
                        .binary_search_by_key(&child, |(id, ..)| *id)
                        .ok()?;
                    let (item, [column, row, columns, rows], mut bounds, baseline) =
                        *grid.cells.get(index)?;
                    bounds[0] += padding[3];
                    bounds[1] += padding[0];
                    let area = grid.area([column, row, columns, rows]);
                    let [columns, rows] = [
                        (
                            Prop::GRID_TEMPLATE_COLUMNS,
                            &grid.columns,
                            column..column + columns,
                        ),
                        (Prop::GRID_TEMPLATE_ROWS, &grid.rows, row..row + rows),
                    ]
                    .map(|(prop, tracks, range)| {
                        let subgrid = styles.values[child]
                            .get(prop)
                            .is_some_and(|v| v.starts_with("subgrid"));
                        tracks
                            .get(range)
                            .map(|tracks| if subgrid { tracks.to_vec() } else { Vec::new() })
                    });
                    Some((
                        bounds,
                        super::grid::Grid {
                            columns: columns?,
                            rows: rows?,
                            gap: grid.gap,
                            baseline,
                            first_item: grid.first_item.filter(|&first| first == item),
                            ..Default::default()
                        },
                        area,
                    ))
                });
                let (placement, area) = match placement {
                    Some((bounds, grid, area)) => (Some((bounds, grid)), Some(area)),
                    None => (None, None),
                };
                // The containing block of a grid item is its grid area. Table parts take it from their table.
                let own = area.filter(|_| !style::table_part(doc, styles, child));
                if let Some(own) = own {
                    let mut host = view.host.borrow_mut();
                    host.dom
                        .entry(view.id)
                        .or_default()
                        .areas
                        .insert(child, own);
                }
                let bounds = placement.as_ref().map(|(bounds, _)| *bounds);
                // A kept text field builds again from its patch, which must hold its new place.
                let field = placement
                    .as_ref()
                    .filter(|_| view.patches.borrow().contains_key(&child))
                    .cloned();
                // The containing block of a positioned child is the padding box.
                let positioned = match styles.values[child].get(Prop::POSITION) {
                    Some("absolute") => made.absolute,
                    Some("fixed") => made.fixed,
                    _ => false,
                };
                let containing = if positioned {
                    [
                        content[0] + padding[1] + padding[3],
                        content[1] + padding[0] + padding[2],
                    ]
                } else if let Some(own) = own {
                    own
                } else if let Some((count, gap, _)) = columns {
                    // Boxes in columns take percentages from the width of a column.
                    let count = count as f32;
                    [
                        ((content[0] - gap * (count - 1.)) / count).max(0.),
                        content[1],
                    ]
                } else {
                    content
                };
                let child_element = element(build, child, containing, container, placement);
                let child_element = match inline_level(child)
                    || floated(child)
                    || info[child].flags & cull::OUT_OF_FLOW != 0
                {
                    true => child_element,
                    false => faded(child, child_element),
                };
                if bounds.is_some()
                    && let Child::Kept(_) = child_element
                    && let Some(patch) = view.patches.borrow_mut().get_mut(&child)
                {
                    patch.slot = field;
                }
                // A kept grid item can take another place, which a build in place must keep.
                if let (Some(bounds), Child::Kept(_)) = (bounds, &child_element)
                    && let Some(record) = view.kept.borrow_mut().get_mut(&child)
                    && let Some(slot) = &mut record.slot
                {
                    slot.0 = bounds;
                }
                let child_element = match bounds {
                    Some(mut bounds) => {
                        let style = &styles.values[child];
                        // Like Chrome, the offsets of a relative grid item take percentages of its grid area.
                        if style.get(Prop::POSITION) == Some("relative")
                            && let Some(area) = area
                        {
                            let units = &styles.lengths[child];
                            for (axis, [start, end]) in
                                [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]]
                                    .into_iter()
                                    .enumerate()
                            {
                                let inset = |prop| {
                                    style
                                        .get(prop)
                                        .and_then(|value| units.resolve(value, area[axis]))
                                };
                                // With both offsets, `left` wins, and `right` in right-to-left text.
                                let end = inset(end).map(|value| -value);
                                bounds[axis] += match end {
                                    Some(end)
                                        if axis == 0
                                            && style.get(Prop::DIRECTION) == Some("rtl") =>
                                    {
                                        Some(end)
                                    }
                                    _ => inset(start).or(end),
                                }
                                .unwrap_or(0.);
                            }
                        }
                        let base = own.map_or(content[0], |area| area[0]);
                        let key = (Rc::as_ptr(style), base.to_bits());
                        let margin = *item_margins.entry(key).or_insert_with(|| {
                            let mut units = styles.lengths[child];
                            units.container = Some(container);
                            units.edges(style, style::MARGIN, base)
                        });
                        place(child_element, margin, pixel_ratio, bounds)
                    }
                    None => child_element,
                };
                let out_of_flow = styles.values[child]
                    .get(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"));
                let child_style = &styles.values[child];
                let child_element = match children.get(index + 1) {
                    Some(&next)
                        if hangs
                            && grid.is_none()
                            && !out_of_flow
                            && !floated(child)
                            && percent_width(&child_element).is_none()
                            && doc.nodes[next].tag == "#text"
                            && doc.nodes[next].text.trim_ascii().is_empty()
                            && shown(index + 1) =>
                    {
                        joined = index + 1;
                        let mut child_element = child_element;
                        let mut space = element(build, next, content, container, None);
                        if let renderer::Child::Node(node) = &mut space {
                            node.style().flex_shrink = 1e6;
                        }
                        let mut joint = renderer::node().flex().flex_row().flex_shrink(1.);
                        joint.style().align_items = line_align;
                        joint.style().align_self = align_of(&mut child_element);
                        joint.style().inline = renderer::Inline::Hangs;
                        joint.child(child_element).child(space).into()
                    }
                    // A space of its own also hangs at the end of a line.
                    _ if hangs
                        && grid.is_none()
                        && doc.nodes[child].tag == "#text"
                        && doc.nodes[child].text.trim_ascii().is_empty() =>
                    {
                        let mut space = child_element;
                        if let renderer::Child::Node(node) = &mut space {
                            node.style().inline = renderer::Inline::Hangs;
                        }
                        space
                    }
                    _ => child_element,
                };
                // A float is a box of the column, and layout places it beside the lines and blocks after it.
                if floated(child) {
                    if let Some(open) = line.take() {
                        place!(open, None);
                    }
                    place!(child_element, None);
                    continue;
                }
                if flow
                    && grid.is_none()
                    && child_style
                        .get(Prop::POSITION)
                        .is_some_and(|value| value == "absolute")
                    && child_style
                        .get(Prop::DISPLAY)
                        .is_none_or(|value| value != "none")
                    && [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]]
                        .iter()
                        .any(|axis| {
                            axis.iter().all(|prop| {
                                child_style.get(*prop).is_none_or(|value| value == "auto")
                            })
                        })
                {
                    // The placeholder must come before the box, which reads its bounds in prepaint.
                    let block = child_style
                        .get(Prop::DISPLAY)
                        .is_some_and(|value| !value.starts_with("inline"))
                        && !child_style.contains(Prop::INTERNAL_STATIC_INLINE);
                    // Outside a line, a box that was inline starts where a new line would.
                    let pending = if line.is_none() && !all_inline {
                        for &other in &children[scanned..index] {
                            if in_flow(doc, info, other) && !floated(other) {
                                last_flow = Some(other);
                            }
                        }
                        scanned = index;
                        last_flow
                    } else {
                        None
                    };
                    let pending =
                        pending
                            .filter(|&other| self::block(build, other))
                            .map_or(0., |other| {
                                let (_, [positive, negative], empty) =
                                    adjoining(build, other, content[0], Some(container));
                                let mut units = styles.lengths[other];
                                units.container = Some(container);
                                let used = margins(build, other, &units, content[0], false, false);
                                // An empty box passes its top margin on too.
                                let used = used[2] + if empty { used[0] } else { 0. };
                                (positive + negative - used).max(0.)
                            });
                    let entity = view.clone();
                    // Its place in the window goes to `anchors`, so each move prepares it again.
                    let mut placeholder = renderer::canvas_watching(
                        move |mut bounds, _| {
                            bounds.origin.y += px(pending);
                            let view = &*entity;
                            view.host
                                .borrow_mut()
                                .dom
                                .entry(view.id)
                                .or_default()
                                .anchors
                                .insert(child, bounds);
                        },
                        |_, _, _| {},
                    )
                    .w(px(0.))
                    .flex_shrink_0()
                    .h(px(
                        if block
                            && (line.is_some()
                                || all_inline && {
                                    for &other in &children[seen..index] {
                                        flowed |= in_flow(doc, info, other);
                                    }
                                    seen = index;
                                    flowed
                                })
                        {
                            units.font.lh
                        } else {
                            0.
                        },
                    ));
                    placeholder.style().align_self = Some(renderer::AlignItems::FlexStart);
                    match line.take() {
                        Some(open) => line = Some(open.child(placeholder)),
                        None => place!(placeholder, None),
                    }
                }
                if flow
                    && !all_inline
                    && grid.is_none()
                    && line.is_none()
                    && !out_of_flow
                    && style::makes_no_line(doc, styles, child)
                {
                    place!(
                        renderer::node().flex().flex_row().child(child_element),
                        None
                    );
                } else if flow
                    && !all_inline
                    && grid.is_none()
                    && inline_level(child)
                    && !out_of_flow
                {
                    let mut next = match line.take() {
                        Some(open) if std::mem::take(&mut line_strut) => {
                            open.child(strut()).child(child_element)
                        }
                        // Like a line box, each row has the strut, also a row that starts inside an inline element.
                        Some(open) if !doc.nodes[child].tag.starts_with('#') => {
                            let (first, rest, _) = with_strut(child_element, line_justify);
                            open.child(first).children(rest)
                        }
                        Some(open) => open.child(child_element),
                        None if doc.nodes[child].tag != "br" => {
                            let (first, rest, aligned) = with_strut(child_element, line_justify);
                            line_strut = aligned;
                            renderer::node()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .child(first)
                                .children(rest)
                        }
                        None => renderer::node()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .child(strut())
                            .child(child_element),
                    };
                    next.style().justify_content = line_justify;
                    next.style().align_items = line_align;
                    next.style().inline = renderer::Inline::Lines;
                    if doc.nodes[child].tag == "br" {
                        place!(next, None);
                    } else {
                        line = Some(next);
                    }
                } else if out_of_flow {
                    // Like Chrome, an absolute box of a grid without insets takes its place in the padding box by
                    // its alignment, as the only item of a grid area. Grid lines make that area its containing
                    // block.
                    let child_element = if matches!(display, "grid" | "inline-grid")
                        && made.absolute
                        && child_style.get(Prop::POSITION) == Some("absolute")
                        && ([[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]]
                            .iter()
                            .any(|axis| {
                                axis.iter().all(|prop| {
                                    child_style.get(*prop).is_none_or(|value| value == "auto")
                                })
                            })
                            || [
                                Prop::GRID_COLUMN_START,
                                Prop::GRID_COLUMN_END,
                                Prop::GRID_ROW_START,
                                Prop::GRID_ROW_END,
                            ]
                            .iter()
                            .any(|prop| child_style.get(*prop).is_some_and(|v| v != "auto")))
                    {
                        let align = |own: Prop, items: Prop| {
                            let value = child_style
                                .get(own)
                                .filter(|value| *value != "auto")
                                .or_else(|| style.get(items))
                                .unwrap_or("normal");
                            let justify = own == Prop::JUSTIFY_SELF;
                            let rtl = justify && style.get(Prop::DIRECTION) == Some("rtl");
                            match value
                                .trim_start_matches("unsafe ")
                                .trim_start_matches("safe ")
                            {
                                "center" => 1,
                                "left" if justify => 0,
                                "right" if justify => 2,
                                "end" | "flex-end" | "self-end" => 2 * usize::from(!rtl),
                                _ => 2 * usize::from(rtl),
                            }
                        };
                        let mut area = renderer::node()
                            .absolute()
                            .top(px(0.))
                            .right(px(0.))
                            .bottom(px(0.))
                            .left(px(0.))
                            .flex()
                            .flex_col();
                        // Grid lines of the box place its area on the tracks, and `auto` lines take the padding edges.
                        if let Some(grid) = &grid {
                            let rtl = style.get(Prop::DIRECTION) == Some("rtl");
                            for (axis, [start, end], tracks, content_prop) in [
                                (
                                    0,
                                    [Prop::GRID_COLUMN_START, Prop::GRID_COLUMN_END],
                                    &grid.columns,
                                    Prop::JUSTIFY_CONTENT,
                                ),
                                (
                                    1,
                                    [Prop::GRID_ROW_START, Prop::GRID_ROW_END],
                                    &grid.rows,
                                    Prop::ALIGN_CONTENT,
                                ),
                            ] {
                                let [start, end] = [start, end]
                                    .map(|prop| child_style.get(prop).unwrap_or("auto"));
                                let gap = grid.gap[axis];
                                let total = tracks.iter().sum::<f32>()
                                    + gap * tracks.len().saturating_sub(1) as f32;
                                let free = if axis == 1 && auto_height {
                                    0.
                                } else {
                                    (content[axis] - total).max(0.)
                                };
                                let shift = free
                                    * match style.get(content_prop).unwrap_or("normal") {
                                        "center" => 0.5,
                                        "end" | "flex-end" => 1.,
                                        _ => 0.,
                                    };
                                // The distance of a grid line from the start padding edge.
                                let edge = |line: usize| {
                                    [padding[3], padding[0]][axis]
                                        + shift
                                        + tracks[..line.min(tracks.len())].iter().sum::<f32>()
                                        + gap * line as f32
                                };
                                // Lines past the explicit grid are `auto`.
                                let (line, span) =
                                    super::grid::placement(start, end, &[], tracks.len());
                                let line = line.filter(|line| line + span <= tracks.len());
                                let first = line.filter(|_| start != "auto").map(edge);
                                let last = match (line, end) {
                                    (_, "auto") => None,
                                    (Some(line), _) => Some(edge(line + span) - gap),
                                    (None, _) => {
                                        super::grid::placement(end, "auto", &[], tracks.len())
                                            .0
                                            .map(|line| edge(line) - gap)
                                    }
                                };
                                if first.is_none() && last.is_none() {
                                    continue;
                                }
                                let at = first.unwrap_or(0.);
                                let size = last.map(|last| (last - at).max(0.));
                                let style = area.style();
                                let (near, far, length) = match axis {
                                    0 if rtl => (
                                        &mut style.inset.right,
                                        &mut style.inset.left,
                                        &mut style.size.width,
                                    ),
                                    0 => (
                                        &mut style.inset.left,
                                        &mut style.inset.right,
                                        &mut style.size.width,
                                    ),
                                    _ => (
                                        &mut style.inset.top,
                                        &mut style.inset.bottom,
                                        &mut style.size.height,
                                    ),
                                };
                                *near = px(at).into();
                                if let Some(size) = size {
                                    (*far, *length) = (renderer::Length::Auto, px(size).into());
                                }
                            }
                        }
                        area.style().align_items = Some(
                            [
                                renderer::AlignItems::FlexStart,
                                renderer::AlignItems::Center,
                                renderer::AlignItems::FlexEnd,
                            ][align(Prop::JUSTIFY_SELF, Prop::JUSTIFY_ITEMS)],
                        );
                        area.style().justify_content = Some(
                            [
                                renderer::AlignContent::FlexStart,
                                renderer::AlignContent::Center,
                                renderer::AlignContent::FlexEnd,
                            ][align(Prop::ALIGN_SELF, Prop::ALIGN_ITEMS)],
                        );
                        area.style().contains = renderer::Escape::ABSOLUTE;
                        if style.get(Prop::DIRECTION) == Some("rtl") {
                            area.style().flow |= renderer::Flow::RTL_BLOCK;
                        }
                        area.child(child_element).into()
                    } else {
                        child_element
                    };
                    // In place, out-of-flow boxes paint in tree order with the positioned boxes around them.
                    if line.is_none() && columns.is_none() {
                        el.push(child_element);
                    } else {
                        floating.push(child_element);
                    }
                } else {
                    if let Some(line) = line.take() {
                        place!(line, None);
                    }
                    // An inline-block with a line of text in the font of the line is as tall as the strut above and
                    // below its baseline, so it needs none.
                    let covers = || {
                        let own = &styles.values[child];
                        let Some(built) = (match &child_element {
                            renderer::Child::Node(node) => Some(node.style_ref()),
                            renderer::Child::Kept(id) | renderer::Child::Placed(id, _) => {
                                build.tree.style_of(*id)
                            }
                        }) else {
                            return false;
                        };
                        let below = |length: renderer::Length| match length {
                            renderer::Length::Px(value) => value.0 < 0.,
                            renderer::Length::Calc(fraction, offset) => {
                                fraction < 0. || offset.0 < 0.
                            }
                            renderer::Length::Fraction(fraction) => fraction < 0.,
                            renderer::Length::Auto | renderer::Length::Bound(_) => false,
                        };
                        own.get(Prop::DISPLAY) == Some("inline-block")
                            && own
                                .get(Prop::VERTICAL_ALIGN)
                                .is_none_or(|value| value == "baseline")
                            && own
                                .get(Prop::OVERFLOW_X)
                                .is_none_or(|value| value == "visible")
                            && own
                                .get(Prop::OVERFLOW_Y)
                                .is_none_or(|value| value == "visible")
                            && own.get(Prop::CONTAIN).is_none_or(|value| value == "none")
                            && own
                                .get(Prop::CONTENT_VISIBILITY)
                                .is_none_or(|value| value == "visible")
                            && built.size.height == renderer::Length::Auto
                            && built.max_size.height == renderer::Length::Auto
                            && built.aspect_ratio.is_none()
                            && !below(built.margin.top)
                            && !below(built.margin.bottom)
                            && own.get(Prop::FONT_FAMILY) == style.get(Prop::FONT_FAMILY)
                            && styles.lengths[child].font.em == units.font.em
                            && styles.lengths[child].font.lh == line_height
                            && doc.nodes[child].children.iter().any(|&kid| {
                                doc.nodes[kid].tag == "#text"
                                    && !doc.nodes[kid].text.trim_ascii().is_empty()
                            })
                    };
                    let (child_element, aligned) = if (strut_due
                        || struts
                            && all_inline
                            && joined != index + 1
                            && !doc.nodes[child].tag.starts_with('#')
                            && (display != "inline" || percent_width(&child_element).is_none())
                            && !covers())
                        && doc.nodes[child].tag != "br"
                    {
                        strut_due = false;
                        let (first, rest, aligned) = with_strut(child_element, line_justify);
                        match rest {
                            Some(element) => {
                                place!(first, None);
                                (element, aligned)
                            }
                            None => (first, aligned),
                        }
                    } else {
                        (child_element, false)
                    };
                    let direct = columns.is_none();
                    if display == "inline" && !doc.nodes[child].tag.starts_with('#') {
                        lone = (lone.0 + 1, percent_width(&child_element).flatten());
                    }
                    place!(child_element, Some(child));
                    strut_next = aligned && direct;
                    if flow && all_inline && doc.nodes[child].tag == "br" {
                        place!(line_break(), None);
                    }
                }
            }
        }
        if let Some(anchor) = static_anchor.take() {
            match line.take() {
                Some(open) => line = Some(open.child(anchor)),
                None if all_inline => place!(anchor, None),
                None => line = Some(open_line(anchor, &mut line_strut)),
            }
        }
        match (line.take(), tail) {
            (Some(open), Some(tail)) if grid.is_none() => {
                let (first, rest, _) = with_strut(tail.into(), line_justify);
                place!(open.child(first).children(rest), None);
            }
            (open, tail) => {
                if let Some(open) = open {
                    place!(open, None);
                }
                if let Some(tail) = tail {
                    match grid {
                        Some(_) => place!(tail, None),
                        None => place!(open_line(tail, &mut line_strut), None),
                    }
                }
            }
        }
        // An inline box around one box with a percent width must not stand between that box and the line.
        if let (1, Some(share)) = lone
            && hit.is_none()
        {
            spread(el.style(), share, line_justify);
            el.style().size.width =
                renderer::Length::Calc(1., px(padding[1] + padding[3] + borders[1] + borders[3]));
        }
        #[cfg(debug_assertions)]
        {
            culled_ids.sort_unstable();
            reader.plan.replace(culled_ids.into());
        }
        if let Some((count, gap, rule)) = columns {
            // A `column-span: all` child splits the columns into sets above and below it.
            let spanner = |child: &Option<usize>| {
                child
                    .is_some_and(|child| styles.values[child].get(Prop::COLUMN_SPAN) == Some("all"))
            };
            let alone = !collected.iter().any(|(child, _)| spanner(child));
            let mut set = Vec::new();
            for (child, item) in collected {
                if !spanner(&child) {
                    set.push(item);
                    continue;
                }
                if !set.is_empty() {
                    let items = std::mem::take(&mut set);
                    el = el.child(column_set(style, content, count, gap, rule, alone, items));
                }
                el = el.child(item);
            }
            if !set.is_empty() {
                el = el.child(column_set(style, content, count, gap, rule, alone, set));
            }
        }
        el = el.children(floating);
    }
    if let Some([width, height]) = intrinsic.filter(|size| skipped && size != &[0.; 2]) {
        el = el.child(renderer::node().w(px(width)).h(px(height)).flex_shrink_0());
    }
    if let Some([_, height]) = intrinsic.filter(|_| forced) {
        let sides = [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ];
        // The last frame laid out the box without its contents. Before one, only the height is known.
        let size = last.map_or([None, Some(height)], |size| size.map(Some));
        let style = el.style();
        for (axis, value) in size.into_iter().enumerate() {
            if let Some(value) = value {
                let value = renderer::Length::Px(px(value + sides[axis]));
                let [size, min, max] = if axis == 0 {
                    [
                        &mut style.size.width,
                        &mut style.min_size.width,
                        &mut style.max_size.width,
                    ]
                } else {
                    [
                        &mut style.size.height,
                        &mut style.min_size.height,
                        &mut style.max_size.height,
                    ]
                };
                (*size, *min, *max) = (value, value, value);
            }
        }
    }
    if info[id].flags & cull::SKIPS != 0 && style.get(Prop::CONTENT_VISIBILITY) == Some("auto") {
        let clips = ancestors
            .iter()
            .copied()
            .filter(|&ancestor| {
                !matches!(doc.nodes[ancestor].tag.as_str(), "html" | "body")
                    && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                        .into_iter()
                        .any(|prop| {
                            styles.values[ancestor]
                                .get(prop)
                                .is_some_and(|value| value != "visible")
                        })
            })
            .collect();
        if let Some(dom) = view.host.borrow_mut().dom.get_mut(&view.id) {
            let last = dom
                .autos
                .get(&id)
                .map_or((0, [f32::NAN; 2]), |entry| entry.2);
            dom.autos.insert(id, (chain.clone(), clips, last));
        }
    }
    after.extend(
        items
            .into_iter()
            .flatten()
            .map(|(order, decoration, _)| (order, decoration)),
    );
    after.sort_by_key(|(order, _)| *order);
    el = el.children(after.into_iter().map(|(_, decoration)| decoration));
    if hit.is_none() && (cell && flow || content.is_some()) {
        el.style().safe = node.tag == "button"
            || !cell
                && style
                    .get(Prop::ALIGN_CONTENT)
                    .is_some_and(|value| !value.starts_with("unsafe"));
        let align = match style.get(Prop::VERTICAL_ALIGN) {
            _ if !cell => content,
            Some("middle") => Some(renderer::AlignContent::Center),
            Some("bottom") => Some(renderer::AlignContent::FlexEnd),
            _ => None,
        };
        if all_inline {
            el.style().align_content = align.or(Some(renderer::AlignContent::FlexStart));
        } else {
            el.style().justify_content = align;
        }
    }
    if let Some((_, grid)) = &slot {
        el.style().baseline_source = grid.first_item.is_some();
        el = el
            .absolute()
            .mt(px(margin[0]))
            .mr(px(margin[1]))
            .mb(px(margin[2]))
            .ml(px(margin[3]));
    }
    // Windows moves the window in the native hit test (`window_drag_area`), macOS and Linux on the first drag.
    if drag {
        el = el.window_drag_area().on_pointer({
            let view = view.clone();
            move |event, window, cx| {
                let renderer::Event::PointerDown {
                    at: position,
                    button: renderer::Button::Left,
                    clicks,
                    ..
                } = *event
                else {
                    return;
                };
                // Use the nearest drag-region box under the pointer so no-drag children keep their clicks.
                let dragged = view.host.try_borrow().is_ok_and(|host| {
                    let viewport = *host.viewports.get(&view.id).unwrap_or(&[800., 600.]);
                    let (Some(page), Some(state)) =
                        (host.pages.get(&view.id), host.dom.get(&view.id))
                    else {
                        return true;
                    };
                    let Some(computed) = &state.computed else {
                        return true;
                    };
                    let at = [position.x, position.y].map(f32::from);
                    crate::dom::hit_test(&page.doc, state, computed, viewport, at, true, false)
                        .as_u64()
                        .is_none_or(|target| {
                            std::iter::successors(Some(target as usize), |&node| {
                                page.doc.nodes[node].parent
                            })
                            .find_map(|node| {
                                computed.values[node]
                                    .get(Prop::APP_REGION)
                                    .filter(|value| *value != "none")
                            }) == Some("drag")
                        })
                });
                if !dragged {
                    return;
                }
                cx.stop_event();
                if clicks != 2 {
                    view.moving.set(true);
                } else if cfg!(target_os = "macos") {
                    window.control().titlebar_double_click();
                } else {
                    window.control().zoom();
                }
            }
        });
    }
    let parent_display = parent
        .and_then(|parent| styles.values[parent].get(Prop::DISPLAY))
        .unwrap_or("block");
    let axes = [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]].map(|axis| {
        axis.iter()
            .all(|prop| style.get(*prop).is_none_or(|v| v == "auto"))
    });
    let mut anchor = None;
    if position == "absolute" && slot.is_none() && axes.contains(&true) {
        // A grid places a box that it contains by its alignment.
        if matches!(parent_display, "grid" | "inline-grid") {
            if positioner != parent {
                if axes[0] {
                    el = el.left(px(0.));
                }
                if axes[1] {
                    el = el.top(px(0.));
                }
            }
        } else if !matches!(parent_display, "flex" | "inline-flex") {
            let block =
                !display.starts_with("inline") && !style.contains(Prop::INTERNAL_STATIC_INLINE);
            if block
                && axes[0]
                && let Some(parent) = parent
            {
                // The static position is at the start of the parent's content box.
                let (start, rtl) = padding_start(styles, parent, containing[0]);
                reader.log(
                    Query::FlatParentPadding,
                    depth,
                    hashed(Some((start.to_bits(), rtl))),
                );
                el = if rtl {
                    el.right(px(start))
                } else {
                    el.left(px(start))
                };
            }
            anchor = Some((axes, block, [margin[3], margin[0]]));
        }
    }
    if stacking {
        el = el.child(renderer::layers(false));
    }
    el = el.children(bars);
    let sticky = (position == "sticky")
        .then(|| {
            let host = view.host.borrow();
            let scrolls = &host.dom.get(&view.id)?.scroll;
            let parent = scroller(doc, styles, ancestors.iter().rev().copied(), scrolls, 0);
            let border = insets(styles, parent, false, containing[0]);
            reader.log(
                Query::Scroller(0),
                ancestors
                    .iter()
                    .rposition(|&ancestor| ancestor == parent)
                    .map_or(1, |index| index + 1),
                hashed((
                    parent,
                    border.map(f32::to_bits),
                    scrolls.contains_key(&parent),
                )),
            );
            let block = ancestors
                .last()
                .map(|&parent| (parent, insets(styles, parent, true, containing[0])));
            reader.log(
                Query::StickyParent,
                depth,
                hashed(block.map(|(parent, insets)| (parent, insets.map(f32::to_bits)))),
            );
            Some(Box::new(super::paint::Sticky {
                scroll: scrolls.get(&parent)?.clone(),
                border,
                block,
            }))
        })
        .flatten();
    // Set alignment on replaced elements, controls and ratio-sized blocks to prevent flex-column stretching.
    if hit.is_none()
        && !flex_parent
        && slot.is_none()
        && !display.starts_with("inline")
        && el.style_ref().align_self.is_none()
        && (replaced
            || el.style().aspect_ratio.is_some()
                && matches!(el.style().size.width, renderer::Length::Auto)
                && !matches!(el.style().size.height, renderer::Length::Auto))
    {
        el.style().align_self = Some(if reader.parent(id, Prop::DIRECTION) == Some("rtl") {
            renderer::AlignItems::FlexEnd
        } else {
            renderer::AlignItems::FlexStart
        });
    }
    // An absolute box with both insets of an axis aligns in its inset-modified containing block. Start and end
    // follow the direction of the containing block, and `auto` is `normal`.
    let rtl_block = matches!(position, "absolute" | "fixed")
        && styles.values[positioner.unwrap_or(0)].get(Prop::DIRECTION) == Some("rtl");
    if hit.is_none() && matches!(position, "absolute" | "fixed") {
        let own = style.get(Prop::DIRECTION) == Some("rtl");
        for (prop, insets, inline) in [
            (Prop::JUSTIFY_SELF, [Prop::LEFT, Prop::RIGHT], true),
            (Prop::ALIGN_SELF, [Prop::TOP, Prop::BOTTOM], false),
        ] {
            if insets
                .iter()
                .any(|&inset| style.get(inset).is_none_or(|v| v == "auto"))
            {
                continue;
            }
            let value = style.get(prop).unwrap_or("auto");
            let start = match value.rsplit(' ').next().unwrap_or(value) {
                "center" => Some(renderer::AlignItems::Center),
                "stretch" => Some(renderer::AlignItems::Stretch),
                "start" | "flex-start" | "baseline" => Some(renderer::AlignItems::FlexStart),
                "end" | "flex-end" => Some(renderer::AlignItems::FlexEnd),
                "self-start" | "self-end" | "left" | "right" => {
                    let start = match value {
                        "self-start" => !inline || own == rtl_block,
                        "self-end" => inline && own != rtl_block,
                        "left" => !inline || !rtl_block,
                        _ => inline && rtl_block,
                    };
                    Some(if start {
                        renderer::AlignItems::FlexStart
                    } else {
                        renderer::AlignItems::FlexEnd
                    })
                }
                _ => None,
            };
            if inline {
                el.style().justify_self = start;
            } else {
                el.style().align_self = start;
            }
        }
    }
    let vertical = style
        .get(Prop::WRITING_MODE)
        .filter(|mode| mode.starts_with("vertical") || mode.starts_with("sideways"))
        .filter(|_| id != 0 && !node.tag.starts_with('#') && slot.is_none());
    let mut el = match vertical {
        Some(mode) => {
            let [_, widest, ..] = super::grid::intrinsic(
                doc,
                id,
                styles,
                None,
                None,
                text,
                &mut measured.borrow_mut(),
            );
            let inline = style
                .get(Prop::HEIGHT)
                .and_then(|value| units.resolve(value, containing[1]))
                .map(|height| height + extra[1])
                .unwrap_or_else(|| widest.min(units.viewport[1]))
                .max(1.);
            let block = super::grid::intrinsic(
                doc,
                id,
                styles,
                Some(inline),
                None,
                text,
                &mut measured.borrow_mut(),
            )[3];
            renderer::node()
                .id(("vertical", id))
                .relative()
                .flex_shrink_0()
                .w(px(block))
                .h(px(inline))
                .child(Turned::node(
                    el.absolute().top_0().left_0().w(px(inline)).into(),
                    mode != "sideways-lr",
                    block,
                    inline,
                ))
        }
        None => el,
    };
    let hooks = DomBox {
        text_clip,
        drawn: node.attrs.contains_key("drawable")
            && style.get(Prop::CANVAS_CONTENT) == Some("drawable")
            && !drawable,
        placed: Default::default(),
        reached_at: Default::default(),
        local: Default::default(),
        offset: Default::default(),
        frame: node.tag == "iframe",
        anchor,
        block: made.absolute.then_some(borders),
        container: positioner.filter(|&block| position == "fixed" || Some(block) != parent_box),
        rtl: rtl_block,
        sized: positioner
            .filter(|&block| block != 0 && Some(block) != parent_box)
            .map(|_| containing),
        style: style.clone(),
        effects: effect_layer,
        composited,
        transformed: info[id].flags & cull::TRANSFORMED != 0,
        perspective: parent_perspective.clone().zip(parent).map(Box::new),
        units,
        revision: info[id].revision,
        sides: [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ],
        margin,
        cell: slot.as_ref().and(parent),
        grid: slot
            .as_ref()
            .filter(|(_, grid)| grid.baseline || grid.first_item.is_some())
            .and_then(|(_, grid)| Some((node.parent?, grid.baseline))),
        shared: (style::role(styles, id) == style::CELL && {
            let table = std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
                .find(|&parent| style::table_box(style::role(styles, parent)));
            let collapsed = collapses(doc, styles, id);
            reader.log(
                Query::Collapse,
                table.map_or(0, |table| reader.scope_of(id, table)),
                hashed(collapsed),
            );
            collapsed
        })
        .then(|| borders.map(|width| width / 2.)),
        boxless: matches!(node.tag.as_str(), "option" | "optgroup") && {
            let found = boxless(doc, id);
            reader.log(
                Query::Boxless(id as u32),
                found.map_or(0, |(select, _)| reader.scope_of(id, select)),
                hashed(found.map(|select| select.1)),
            );
            found.is_some_and(|select| select.1)
        },
        text: (display == "inline" && !node.tag.starts_with('#'))
            .then(|| {
                let mut texts = node.children.iter().copied().filter(|&child| {
                    doc.nodes[child].tag == "#text"
                        && !doc.nodes[child].text.trim_ascii().is_empty()
                });
                let first = texts.next()?;
                Some((
                    [first, texts.next_back().unwrap_or(first)].map(|id| id as u32),
                    [padding[0] + borders[0], padding[2] + borders[2]],
                ))
            })
            .flatten(),
        leading: (display == "inline" && !node.tag.starts_with('#')).then(|| {
            let face = text.face(&parsed.face);
            let [ascent, descent, _] = text
                .line_metrics(face, px(units.font.em * pixel_ratio))
                .map(f32::from);
            let line = units.font.lh * pixel_ratio;
            let above = ((line - ascent - descent) / 2.).floor();
            let edges = padding[0] + padding[2] + borders[0] + borders[2];
            [
                units.font.lh + edges,
                above / pixel_ratio,
                (line - above - ascent - descent) / pixel_ratio,
            ]
        }),
        parts: if style::table_box(style::role(styles, id)) {
            let mut rows: Vec<(usize, Vec<usize>)> = Vec::new();
            let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
            for &cell in &node.children {
                let Some(row) = doc.nodes[cell].parent.filter(|&row| {
                    row != id && (style::ROW..=style::FOOTER).contains(&style::role(styles, row))
                }) else {
                    continue;
                };
                match rows.last_mut() {
                    Some((last, cells)) if *last == row => cells.push(cell),
                    _ => {
                        rows.push((row, vec![cell]));
                        if let Some(group) = doc.nodes[row].parent.filter(|&group| group != id) {
                            match groups.last_mut() {
                                Some((last, members)) if *last == group => members.push(row),
                                _ => groups.push((group, vec![row])),
                            }
                        }
                    }
                }
            }
            let rows = rows.into_iter().map(|(row, cells)| (row, true, cells));
            let rows = rows.chain(groups.into_iter().map(|(group, rows)| (group, false, rows)));
            Some(Rc::new(super::paint::Parts::Rows(rows.collect())))
        } else {
            (!cuts.is_empty()).then(|| {
                cut_parts.sort_by_key(|&(_, at, _)| at);
                let at: rustc_hash::FxHashMap<usize, usize> = cuts
                    .iter()
                    .enumerate()
                    .map(|(at, &(cut, ..))| (cut, at))
                    .collect();
                let mut boxes: Vec<_> = cuts.iter().map(|&(cut, ..)| (cut, Vec::new())).collect();
                for (cut, _, piece) in cut_parts {
                    boxes[at[&cut]].1.push(piece);
                }
                Rc::new(super::paint::Parts::Cuts {
                    sides: (!multicol)
                        .then_some([padding[3] + borders[3], padding[1] + borders[1]]),
                    boxes,
                })
            })
        },
        id,
        plans,
        ids: ids.clone(),
        view: view.clone(),
        sticky,
        fixed: position == "fixed" && positioner.is_none(),
        inset: [Prop::TOP, Prop::RIGHT, Prop::BOTTOM, Prop::LEFT].map(|prop| {
            let axis = usize::from(matches!(prop, Prop::TOP | Prop::BOTTOM));
            style
                .get(prop)
                .and_then(|v| units.resolve(v, containing[axis]))
        }),
    };
    #[cfg(debug_assertions)]
    reader.extra.set(hashed(format!(
        "{} {:?}",
        reader.extra.get(),
        (
            (
                hooks.anchor,
                hooks.block,
                hooks.container,
                hooks.sized,
                hooks.inset,
                hooks.effects,
                hooks.composited,
                hooks.revision,
                hooks.sides,
                hooks.margin,
                hooks.drawn,
            ),
            (
                hooks.cell,
                hooks.grid,
                &hooks.parts,
                hooks.shared,
                hooks.leading,
                hooks.boxless,
                hooks.text,
                hooks.fixed,
                hooks
                    .sticky
                    .as_ref()
                    .map(|sticky| (sticky.border, sticky.block)),
            ),
        )
    )));
    if hit.is_none() {
        let mut flow = clear_bits(style);
        let placed =
            !flex_parent && slot.is_none() && matches!(position, "static" | "relative" | "sticky");
        let root = node
            .parent
            .is_none_or(|parent| doc.nodes[parent].tag.starts_with('#'));
        match float_side(style) {
            Some(right) if placed => {
                flow |= if right {
                    renderer::Flow::RIGHT
                } else {
                    renderer::Flow::LEFT
                };
            }
            // A block that makes no formatting context of its own: floats around it reach into it.
            None if placed
                && !root
                && matches!(display, "block" | "list-item")
                && el.style_ref().overflow.x == renderer::Overflow::Visible
                && el.style_ref().overflow.y == renderer::Overflow::Visible
                && (el.style_ref().flex_direction == renderer::FlexDirection::Column
                    || el.style_ref().inline == renderer::Inline::Lines)
                && style.get(Prop::CONTAIN).is_none_or(|value| value == "none")
                && style
                    .get(Prop::CONTENT_VISIBILITY)
                    .is_none_or(|value| value == "visible")
                && style
                    .get(Prop::COLUMN_COUNT)
                    .is_none_or(|value| value == "auto")
                && style
                    .get(Prop::COLUMN_WIDTH)
                    .is_none_or(|value| value == "auto")
                && style
                    .get(Prop::ALIGN_CONTENT)
                    .is_none_or(|value| value == "normal") =>
            {
                flow |= renderer::Flow::BLOCK;
            }
            _ => {}
        }
        el.style().flow |= flow;
    }
    let mut element = el.hooks(hooks);
    if let Some(origin) = pseudo_origin {
        element = element.hooks(super::paint::ElementOrigin(origin));
    }
    // Like Chrome, positioned boxes and stacking contexts with `z-index: auto` or 0 paint after the in-flow
    // boxes of their stacking context, in tree order.
    if hit.is_none()
        && let Some(order) = order
            .filter(|_| layered)
            .or((positioned || stacking).then_some(0))
        && id != 0
        && !top_layer.contains(&id)
    {
        element = element.z_index(order);
    }
    match (share, &hit) {
        (Some(key), None) => 'store: {
            let own: Option<Box<[Read]>> = reads.borrow()[mark..]
                .iter()
                .filter(|read| usize::from(read.reader) == depth)
                .map(|read| {
                    Some(Read {
                        query: read.query.moved(id as u32, u32::MAX)?,
                        // Unlike kept records, entries hold the box's own reads, so its own scope is 1 and 0
                        // stays the page.
                        scope: if read.scope == 0 {
                            0
                        } else {
                            (depth + 2 - usize::from(read.scope)) as u16
                        },
                        reader: 0,
                        ..*read
                    })
                })
                .collect();
            let Some(own) = own else {
                break 'store;
            };
            let mut shared = view.shared.borrow_mut();
            if shared.len() >= 4096 {
                shared.clear();
            }
            let list = shared.entry(key).or_default();
            if list.len() == 4 {
                list.remove(0);
            }
            list.push(Shared {
                plan: parsed.clone(),
                reads: own,
                style: element.shared_style(),
            });
        }
        #[cfg(debug_assertions)]
        (Some(_), Some(style)) => {
            assert!(
                element.shares(style),
                "box {id} <{}> sets its style after it took a shared one",
                node.tag
            );
            let start = reads.borrow().len();
            let printed = reader.prints.borrow().len();
            let culls = std::cell::Cell::new(0);
            let shadow = Build {
                ancestors: ancestors.clone(),
                perspective: None,
                zone: zone.clone(),
                culls: &culls,
                checking: true,
                extra: Default::default(),
                plan: Default::default(),
                ..*reader
            };
            let fresh = create(&shadow, id, containing, container, None, None);
            reader.prints.borrow_mut().truncate(printed);
            let own = |reads: &[Read]| {
                let mut own: Vec<_> = reads
                    .iter()
                    .filter(|read| usize::from(read.reader) == depth)
                    .map(|read| (read.query, read.value))
                    .collect();
                own.sort_unstable_by_key(|&(query, _)| query);
                own.dedup_by_key(|&mut (query, _)| query);
                own
            };
            let mut log = reads.borrow_mut();
            let (took, built) = (own(&log[mark..start]), own(&log[start..]));
            log.truncate(start);
            assert!(
                took == built,
                "box {id} <{}> reads otherwise than the box whose style it took: {:?} instead of {:?}",
                node.tag,
                took.iter().map(|read| read.0).collect::<Vec<_>>(),
                built.iter().map(|read| read.0).collect::<Vec<_>>()
            );
            let renderer::Child::Node(fresh) = fresh else {
                unreachable!()
            };
            assert!(
                *fresh.style_ref() == **style,
                "box {id} <{}> builds a style that differs from the one it took",
                node.tag
            );
        }
        _ => {}
    }
    element.into()
}

// Font units from the pseudo-element's own font, as the style pass gives them to an element.
pub(super) fn pseudo_lengths(
    units: &style::Lengths,
    style: &style::Style,
    pseudo: &style::Style,
    text: &renderer::TextSystem,
    pixel_ratio: f32,
) -> style::Lengths {
    let mut lengths = *units;
    let size = pseudo
        .get(Prop::FONT_SIZE)
        .and_then(|value| units.resolve(value, units.font.em))
        .unwrap_or(units.font.em);
    if size == units.font.em
        && [
            Prop::FONT_FAMILY,
            Prop::FONT_WEIGHT,
            Prop::FONT_STYLE,
            Prop::LINE_HEIGHT,
        ]
        .into_iter()
        .all(|prop| pseudo.get(prop) == style.get(prop))
    {
        return lengths;
    }
    let face = text.face(&style::font(pseudo));
    let metrics = text.metrics(face);
    let advance =
        |glyph, default: f32| text.advance(face, px(1.), glyph).map_or(default, f32::from) * size;
    lengths.font = style::FontUnits {
        em: size,
        ch: advance('0', 0.5),
        ex: metrics.x_height * size,
        cap: metrics.cap_height * size,
        ic: advance('\u{6c34}', 1.),
        lh: match style::line_height(pseudo) {
            style::LineHeight::Fixed(height) => height,
            style::LineHeight::Scale(scale) => scale * size,
            style::LineHeight::Normal => {
                text.line_metrics(face, px(size * pixel_ratio))
                    .into_iter()
                    .map(f32::from)
                    .sum::<f32>()
                    / pixel_ratio
            }
        },
        absolute: units.font.absolute,
    };
    lengths
}

// Taffy applies min/max constraints through aspect ratio even with an explicit opposite size. Prevent
// max-width from changing explicit image heights.
fn only_auto_ratio(mut el: renderer::Node, style: &style::Style) -> renderer::Node {
    let set = |prop| style.get(prop).is_some_and(|value| value != "auto");
    let limited = |prop| style.get(prop).is_some_and(|value| value != "none");
    if set(Prop::HEIGHT) {
        if !limited(Prop::MAX_HEIGHT) {
            el = el.max_h(px(1e7));
        }
        if !set(Prop::MIN_HEIGHT) {
            el = el.min_h(px(0.));
        }
    }
    if set(Prop::WIDTH) {
        if !limited(Prop::MAX_WIDTH) {
            el = el.max_w(px(1e7));
        }
        if !set(Prop::MIN_WIDTH) {
            el = el.min_w(px(0.));
        }
    }
    el
}

pub(super) fn aspect_ratio(value: &str) -> Option<f32> {
    let value = value.replace('/', " / ");
    let parts = style::components(&value, ' ');
    let numbers: Vec<f32> = parts.iter().filter_map(|part| part.parse().ok()).collect();
    let ratio = match numbers.as_slice() {
        [a, b, ..] if parts.contains(&"/") => a / b,
        [a, ..] => *a,
        _ => return None,
    };
    (ratio.is_finite() && ratio > 0.).then_some(ratio)
}

pub(crate) fn dimension(value: &str, integer: bool) -> Option<f32> {
    let value = value.trim_start_matches(['\t', '\n', '\x0c', '\r', ' ']);
    let value = if integer {
        value.strip_prefix('+').unwrap_or(value)
    } else {
        value
    };
    let digits = |from: usize| {
        value.as_bytes()[from..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    let whole = digits(0);
    let end = match value.as_bytes().get(whole) {
        Some(b'.') if !integer => whole + 1 + digits(whole + 1),
        _ => whole,
    };
    if whole == 0 || !integer && value.as_bytes().get(end) == Some(&b'%') {
        return None;
    }
    if integer {
        return value[..end]
            .parse::<u32>()
            .ok()
            .filter(|&value| value <= i32::MAX as u32)
            .map(|value| value as f32);
    }
    value[..end]
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
}

pub(super) fn text_field(node: &crate::dom::Node, style: &style::Style) -> bool {
    node.tag == "textarea"
        || node.tag == "input"
            && node.native_control(style).is_none()
            && ![
                "checkbox", "radio", "button", "submit", "reset", "image", "file", "color",
                "range", "hidden",
            ]
            .iter()
            .any(|other| node.input_type().eq_ignore_ascii_case(other))
}

pub(super) fn natural_size(
    doc: &Document,
    id: usize,
    decoded: Option<[f32; 2]>,
    style: &style::Style,
) -> Option<([f32; 2], Option<f32>)> {
    let node = &doc.nodes[id];
    let attr = |name: &str, default: f32| {
        node.attrs
            .get(name)
            .and_then(|value| dimension(value, node.tag == "canvas"))
            .unwrap_or(default)
    };
    let pictured = |size: [f32; 2]| {
        (
            size,
            (size[0] > 0. && size[1] > 0.).then(|| size[0] / size[1]),
        )
    };
    let kind = if node.tag == "input" {
        node.input_type()
    } else {
        ""
    };
    if let Some(native) = node
        .native_control(style)
        .filter(|native| *native != renderer::ControlKind::Button)
    {
        return Some((super::native::size(native), None));
    }
    match (node.tag.as_str(), decoded.filter(|_| !broken_image(node))) {
        ("canvas", _) => Some(pictured([attr("width", 300.), attr("height", 150.)])),
        ("img" | "video", Some(size)) => Some(pictured(size)),
        ("video", None) => Some(([300., 150.], None)),
        ("img", None) => {
            let (broken, alt) = (broken_image(node), node.attrs.get("alt"));
            match ["width", "height"].map(|name| {
                node.attrs
                    .get(name)
                    .and_then(|value| dimension(value, false))
            }) {
                [Some(width), Some(height)] if !broken || alt.is_none_or(|alt| alt.is_empty()) => {
                    Some(pictured([width, height]))
                }
                _ if !broken || alt.is_some_and(|alt| !alt.is_empty()) => None,
                _ if alt.is_none() && node.attrs.contains_key("src") => Some(([16., 16.], None)),
                _ => Some(([0., 0.], None)),
            }
        }
        ("object", _)
            if node.attrs.contains_key("data")
                || node.children.iter().any(|&child| {
                    let child = &doc.nodes[child];
                    !matches!(child.tag.as_str(), "param" | "#comment")
                        && (child.tag != "#text" || !child.text.trim_ascii().is_empty())
                }) =>
        {
            None
        }
        ("iframe" | "object" | "embed", _) => {
            Some(([attr("width", 300.), attr("height", 150.)], None))
        }
        ("input", _) if kind.eq_ignore_ascii_case("range") => Some(([129., 16.], None)),
        ("input", _)
            if kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio") =>
        {
            Some(([13., 13.], None))
        }
        _ => None,
    }
}

pub(super) fn broken_image(node: &crate::dom::Node) -> bool {
    node.tag == "img"
        && (node.state & crate::dom::BROKEN != 0
            || !node.attrs.contains_key("src") && !node.attrs.contains_key("srcset"))
}

fn paint_broken_icon(origin: renderer::Point<renderer::Pixels>, window: &mut renderer::Window) {
    thread_local! {
        static ICON: RefCell<Option<(u32, Arc<renderer::RenderImage>)>> = const { RefCell::new(None) };
    }
    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><defs><linearGradient id="s" x2="0" y2="1"><stop offset="0" stop-color="#ebf2fd"/><stop offset="1" stop-color="#c3d4f3"/></linearGradient><clipPath id="c"><path d="M.25.25h10l5.5 5.5v10H.25z"/></clipPath></defs><g clip-path="url(#c)"><rect width="16" height="16" fill="url(#s)"/><ellipse cx="6" cy="4.5" rx="2.5" ry="1.25" fill="#fff"/><ellipse cx="7" cy="17" rx="7.5" ry="6.5" fill="#58ae39"/></g><path d="M.25.25h10l5.5 5.5v10H.25z" fill="none" stroke="#999" stroke-width=".5"/><path d="M10.25.25v5.5h5.5" fill="#f2f2f2" stroke="#999" stroke-width=".5"/><path d="M16.5 8 7.5 16.5" stroke="#fff" stroke-width="1.25"/></svg>"##;
    let raster = (16. * window.scale_factor()).ceil() as u32;
    let Some(image) = ICON.with_borrow_mut(|icon| {
        if icon.as_ref().is_none_or(|(size, _)| *size != raster) {
            *icon = super::canvas::svg(SVG, raster, raster)
                .ok()
                .map(|image| (raster, image));
        }
        icon.as_ref().map(|(_, image)| image.clone())
    }) else {
        return;
    };
    let bounds = Rect::new(origin, size(px(16.), px(16.)));
    window.draw_image(bounds, bounds, px(0.), image, false, false);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn field_size(
    doc: &Document,
    id: usize,
    style: &style::Style,
    units: &style::Lengths,
    text: &renderer::TextSystem,
    pixel_ratio: f32,
    width: bool,
    wrap_width: Option<f32>,
) -> [f32; 2] {
    let node = &doc.nodes[id];
    let textarea = node.tag == "textarea";
    if style.get(Prop::FIELD_SIZING) == Some("content") {
        let value = node.shown_text(
            node.current_value()
                .map_or_else(|| doc.text(id), str::to_owned),
        );
        let mut shown = if value.is_empty() {
            node.attrs.get("placeholder").cloned().unwrap_or_default()
        } else {
            value
        };
        if let Some(mode) = style
            .get(Prop::TEXT_TRANSFORM)
            .filter(|mode| *mode != "none")
        {
            let len = shown.len();
            style::text_transform(&mut shown, 0..len, mode);
        }
        let run = style::text_run(style, units, shown.len());
        let lines = text.shape_text(
            &shown,
            px(units.font.em),
            &[run],
            &renderer::Wrapping {
                width: wrap_width
                    .filter(|_| {
                        textarea && !matches!(style.get(Prop::WHITE_SPACE), Some("pre" | "nowrap"))
                    })
                    .map(px),
                word_break: style::word_break(style),
                ..Default::default()
            },
        );
        return [
            if width {
                lines
                    .iter()
                    .map(|line| f32::from(line.line.width))
                    .fold(0., f32::max)
                    + 1.
            } else {
                0.
            },
            if textarea {
                lines.iter().map(|line| line.rows()).sum::<usize>().max(1) as f32 * units.font.lh
            } else {
                field_line(node, style, units, text, pixel_ratio)
            },
        ];
    }
    let count = |name, default| {
        node.attrs
            .get(name)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|&value| value > 0)
            .unwrap_or(default) as f32
    };
    let width = width.then(|| {
        let face = style::font(style);
        let em = px(units.font.em);
        let family = style
            .get(Prop::FONT_FAMILY)
            .and_then(|value| value.split(',').next())
            .map(|value| value.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
            .unwrap_or_default();
        let id = text.face(&face);
        let scale = pixel_ratio;
        let (char_width, extra) = if "|american typewriter|arial hebrew|chalkboard|cochin|\
            corsiva hebrew|courier|euphemia ucas|geneva|gill sans|hei|helvetica|hoefler text|\
            inaimathi|kai|lucida grande|marker felt|monaco|mshtakan|new peninim mt|osaka|\
            raanana|stheiti|symbol|times|apple braille|apple ligothic|apple lisung|\
            apple symbols|applegothic|applemyungjo|#gungseo|#headlinea|#pcmyungjo|#pilgi|"
            .contains(&format!("|{family}|"))
        {
            let zero = text
                .advance(id, em, '0')
                .map_or(units.font.em / 2., f32::from);
            (zero * scale, 0.)
        } else {
            let average = if face.family.as_ref() == ".SystemUIFont" {
                (text.metrics(id).x_height * em.0 * scale).round()
            } else {
                text.advance(id, em, 'x')
                    .map_or(units.font.em / 2., f32::from)
                    * scale
            };
            let ascent = f32::from(text.line_metrics(id, em * scale)[0]);
            (average, if textarea { 0. } else { ascent - average })
        };
        if textarea {
            let bar = match style.get(Prop::SCROLLBAR_WIDTH) {
                _ if style.get(Prop::OVERFLOW_Y).is_some_and(|v| v == "hidden") => 0.,
                Some("none") => 0.,
                Some("thin") => 14.,
                _ => 16.,
            };
            (char_width * count("cols", 20)).ceil() / scale + bar
        } else {
            (char_width * count("size", 20) + extra).ceil() / scale
        }
    });
    [
        width.unwrap_or(0.),
        if textarea {
            count("rows", 2) * units.font.lh
        } else {
            field_line(node, style, units, text, pixel_ratio)
        },
    ]
}

/// The line height of a field. Like Chrome, a single-line field whose `line-height` is not above its font size, in
/// device pixels, takes the normal line height.
pub(super) fn field_line(
    node: &crate::dom::Node,
    style: &style::Style,
    units: &style::Lengths,
    text: &renderer::TextSystem,
    pixel_ratio: f32,
) -> f32 {
    let pixel_ratio = pixel_ratio.max(1.);
    if node.tag == "textarea"
        || (units.font.lh * pixel_ratio).floor() > (units.font.em * pixel_ratio).round()
    {
        return units.font.lh;
    }
    text.line_metrics(
        text.face(&style::font(style)),
        px(units.font.em * pixel_ratio),
    )
    .into_iter()
    .map(f32::from)
    .sum::<f32>()
        / pixel_ratio
}

/// The natural content size of a file input with the box of its button: the button and label, or 34 digits.
pub(super) fn file_size(
    style: &style::Style,
    button: &style::Style,
    units: &style::Lengths,
    text: &renderer::TextSystem,
    pixel_ratio: f32,
) -> [f32; 2] {
    let em = px(units.font.em);
    let width = |label: &str| {
        f32::from(
            text.shape_line(label, em, &style::text_run(style, units, label.len()))
                .line
                .width,
        )
    };
    let zero = text
        .advance(text.face(&style::font(style)), em, '0')
        .map_or(units.font.em / 2., f32::from);
    let [padding, margin] =
        [style::PADDING, style::MARGIN].map(|sides| units.edges(button, sides, 0.));
    let borders = units.borders(button, 0.);
    let edges = |side: usize| padding[side] + margin[side] + borders[side];
    // The button text takes the font size of `::file-selector-button`.
    let scale = button
        .get(Prop::FONT_SIZE)
        .and_then(|value| units.resolve(value, units.font.em))
        .map_or(1., |size| size / units.font.em);
    let content = width("Choose File") * scale + edges(1) + edges(3) + width("No file chosen");
    [
        layout_ceil((zero * 34.).max(content), pixel_ratio),
        units
            .font
            .lh
            .max(units.font.lh * scale + edges(0) + edges(2)),
    ]
}

pub(super) fn layout_ceil(size: f32, pixel_ratio: f32) -> f32 {
    (size * pixel_ratio * 64.).ceil() / 64. / pixel_ratio
}

pub(super) fn place(
    child: Child,
    [top, right, bottom, left]: [f32; 4],
    pixel_ratio: f32,
    [x, y, cell_width, cell_height]: [f32; 4],
) -> Child {
    let place = Rect {
        origin: point(px(x), px(y)),
        size: size(
            px(layout_ceil(
                (cell_width - right - left).max(0.),
                pixel_ratio,
            )),
            px((cell_height - top - bottom).max(0.)),
        ),
    };
    match child {
        Child::Node(mut node) => {
            let style = node.style();
            style.inset.left = place.origin.x.into();
            style.inset.top = place.origin.y.into();
            style.size = place.size.map(Into::into);
            Child::Node(node)
        }
        Child::Kept(id) | Child::Placed(id, _) => Child::Placed(id, place),
    }
}

/// Whether an inline box has padding, a border or a margin on a side.
fn edge(build: &Build<'_>, id: usize, side: usize) -> bool {
    let (style, units) = (&build.styles.values[id], &build.styles.lengths[id]);
    units.edges(style, style::PADDING, 0.)[side]
        + units.edges(style, style::MARGIN, 0.)[side]
        + units.borders(style, 0.)[side]
        != 0.
}

/// The spliced inline boxes with a part in the run `[start, end)` of `children`, with their `CUT_*` flags.
fn run_cuts(
    build: &Build<'_>,
    cuts: &[(usize, usize, usize)],
    children: &[usize],
    [start, end]: [usize; 2],
    in_run: impl Fn(usize) -> bool,
) -> Vec<(usize, u8)> {
    let blocky = |child: usize| {
        !in_run(child)
            && float_side(&build.styles.values[child]).is_none()
            && build.styles.values[child]
                .get(Prop::POSITION)
                .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
    };
    // An empty run is a line of its own, only for the edges of the boxes next to it.
    let edge = |cut: usize, side: usize| start < end || edge(build, cut, side);
    let mut parts = Vec::new();
    let near = cuts.partition_point(|&(_, from, _)| from <= end);
    for &(cut, from, to) in cuts[..near].iter().filter(|&&(_, _, to)| to >= start) {
        let (Some(before), Some(after)) = (
            children.get(from..start.max(from)),
            children.get(end.min(to)..to),
        ) else {
            continue;
        };
        let content = |child: &usize| in_run(*child) || blocky(*child);
        let flags = if from < end && to > start && start < end {
            (u8::from(!before.iter().any(content)) * style::CUT_FIRST)
                | (u8::from(!after.iter().any(content)) * style::CUT_LAST)
        // Like Chrome, a box that starts or ends with a block has an empty part in the line next to it.
        } else if to == start && to > from && blocky(children[to - 1]) && edge(cut, 1) {
            style::CUT_LAST | style::CUT_BEFORE
        } else if from == end && from < to && blocky(children[from]) && edge(cut, 3) {
            style::CUT_FIRST | style::CUT_AFTER
        } else {
            continue;
        };
        parts.push((cut, flags));
    }
    parts
}

fn in_flow(doc: &Document, info: &[cull::Info], id: usize) -> bool {
    let node = &doc.nodes[id];
    !matches!(node.tag.as_str(), "#comment" | "#doctype")
        && !(node.tag == "#text" && node.text.trim_ascii().is_empty())
        && info[id].flags & cull::OUT_OF_FLOW == 0
}

// Keep these margins equal to layout margins: culled spacers use them.
fn margins(
    build: &Build<'_>,
    id: usize,
    units: &style::Lengths,
    width: f32,
    flex_parent: bool,
    slot: bool,
) -> [f32; 4] {
    let mut margin = units.edges(&build.styles.values[id], style::MARGIN, width);
    // The blocks of a spliced inline box are in the flow of its block.
    let parent = std::iter::successors(build.doc.nodes[id].parent, |&up| {
        style::spliced(build.doc, build.styles, up)
            .then(|| build.doc.nodes[up].parent)
            .flatten()
    })
    .find(|&up| !style::spliced(build.doc, build.styles, up));
    let Some(parent) = parent.filter(|_| !flex_parent && !slot) else {
        return margin;
    };
    let walked = build.walks.borrow().get(&parent).cloned();
    let walked = walked.unwrap_or_else(|| {
        let collapses = container(build, parent);
        let open =
            [0, 2].map(|side| collapses && margin_collapses_through(build, parent, side, width));
        // Skip animated boxes. Clear walks when fonts or the viewport change: the key excludes them.
        let key = (
            build.info[parent].revision,
            width.to_bits(),
            open,
            units.container.map(|size| size.map(f32::to_bits)),
        );
        let animated = build.info[parent].flags & cull::ANIMATED != 0;
        if let Some((old, used)) = build.view.walked.borrow().get(&parent)
            && *old == key
            && !animated
        {
            build.walks.borrow_mut().insert(parent, used.clone());
            return used.clone();
        }
        let mut used = rustc_hash::FxHashMap::default();
        if collapses {
            let children = flow(build, parent);
            let value = |set: [f32; 2]| set[0] + set[1];
            let mut pending = [0f32; 2];
            let mut emitted = 0.;
            let mut leading = open[0];
            for &child in &children {
                if !block(build, child) {
                    // Like Chrome, margins collapse across inline content that makes no line.
                    if !style::makes_no_line(build.doc, build.styles, child) {
                        (leading, pending, emitted) = (false, [0.; 2], 0.);
                    }
                    continue;
                }
                let (top, bottom, empty) = adjoining(build, child, width, units.container);
                let mut sides = [0f32; 2];
                if leading {
                    if empty {
                        used.insert(child, sides);
                        continue;
                    }
                    leading = false;
                } else {
                    pending = [pending[0].max(top[0]), pending[1].min(top[1])];
                    sides[0] = value(pending) - emitted;
                    emitted += sides[0];
                }
                if empty {
                    pending = [pending[0].max(bottom[0]), pending[1].min(bottom[1])];
                    sides[1] = value(pending) - emitted;
                    emitted += sides[1];
                } else {
                    pending = bottom;
                    sides[1] = value(bottom);
                    emitted = sides[1];
                }
                used.insert(child, sides);
            }
            if open[1] {
                let mut trailing = Vec::new();
                for child in children.into_iter().rev() {
                    if !block(build, child) && style::makes_no_line(build.doc, build.styles, child)
                    {
                        continue;
                    }
                    let sets = adjoining(build, child, width, units.container);
                    let empty = block(build, child) && sets.2;
                    let Some(sides) = used.get_mut(&child) else {
                        break;
                    };
                    if empty {
                        *sides = [0.; 2];
                        trailing.push((child, sets));
                        continue;
                    }
                    sides[1] = 0.;
                    // Like Chrome, the empty boxes after the last box sit below the margins before them, and
                    // their margins pass to the parent.
                    let mut pending = sets.1;
                    for (child, (top, bottom, _)) in trailing.into_iter().rev() {
                        pending = [pending[0].max(top[0]), pending[1].min(top[1])];
                        let at = value(pending);
                        used.insert(child, [at, -at]);
                        pending = [pending[0].max(bottom[0]), pending[1].min(bottom[1])];
                    }
                    break;
                }
            }
        }
        let used = Rc::new(used);
        build.walks.borrow_mut().insert(parent, used.clone());
        if !animated {
            build
                .view
                .walked
                .borrow_mut()
                .insert(parent, (key, used.clone()));
        }
        used
    });
    if let Some(&[top, bottom]) = walked.get(&id) {
        margin[0] = top;
        margin[2] = bottom;
    }
    margin
}

/// The opacity of the inline boxes between `child` and its block `id`, which are parts of the text of the block.
fn inline_fade(doc: &Document, styles: &style::ComputedStyles, child: usize, id: usize) -> f32 {
    std::iter::successors(doc.nodes.get(child).and_then(|node| node.parent), |&up| {
        doc.nodes[up].parent
    })
    .take_while(|&up| up != id)
    .filter(|&up| {
        !doc.nodes[up].tag.starts_with('#')
            && styles.values[up].get(Prop::DISPLAY) != Some("contents")
    })
    .map(|up| super::effects::opacity(&styles.values[up]))
    .product()
}

fn flow(build: &Build<'_>, id: usize) -> Vec<usize> {
    let mut flow = Vec::new();
    let mut pending: Vec<usize> = build.doc.nodes[id].children.iter().rev().copied().collect();
    while let Some(child) = pending.pop() {
        if child > usize::MAX / 2 {
            flow.push(!child);
        // A plain block is an element in the flow that doesn't float.
        } else if build.info[child].flags & cull::PLAIN_BLOCK != 0 {
            flow.push(child);
        } else if style::spliced(build.doc, build.styles, child) {
            // The edges of the box make lines, which keep margins apart.
            let edges = [3, 1].map(|side| edge(build, child, side));
            if edges[1] {
                pending.push(!child);
            }
            pending.extend(build.doc.nodes[child].children.iter().rev());
            if edges[0] {
                flow.push(child);
            }
        // Preserved white space makes a line.
        } else if (in_flow(build.doc, build.info, child)
            || build.doc.nodes[child].tag == "#text"
                && !style::makes_no_line(build.doc, build.styles, child))
            && float_side(&build.styles.values[child]).is_none()
        {
            flow.push(child);
        }
    }
    flow
}

fn block(build: &Build<'_>, id: usize) -> bool {
    !build.doc.nodes[id].tag.starts_with('#')
        && build.doc.nodes[id].tag != "br"
        && build.styles.values[id]
            .get(Prop::DISPLAY)
            .is_some_and(|value| !value.starts_with("inline") && value != "contents")
        || style::holds_block_box(build.doc, build.styles, id)
            && !style::spliced(build.doc, build.styles, id)
}

fn container(build: &Build<'_>, id: usize) -> bool {
    build.styles.values[id]
        .get(Prop::DISPLAY)
        .is_none_or(|value| {
            !value.contains("flex") && !value.contains("grid") && value != "contents"
        })
}

// The overflow of the body goes to the viewport when the html box has none, so the body itself doesn't clip.
fn body_overflow_moves(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    doc.nodes[id].tag == "body"
        && doc.nodes[id].parent.is_some_and(|parent| {
            doc.nodes[parent].tag == "html"
                && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                    .into_iter()
                    .all(|prop| {
                        styles.values[parent]
                            .get(prop)
                            .is_none_or(|value| value == "visible")
                    })
        })
}

// `side` 3 is the bottom edge of a box that margins collapse through, which can also have a zero height.
fn margin_collapses_through(build: &Build<'_>, id: usize, side: usize, width: f32) -> bool {
    let (doc, styles) = (build.doc, build.styles);
    let style = &styles.values[id];
    let edge = |prop: Prop| {
        style
            .get(prop)
            .and_then(|value| styles.lengths[id].resolve(value, width))
            .unwrap_or(0.)
    };
    let [padding, border, kind] = if side == 0 {
        [
            Prop::PADDING_TOP,
            Prop::BORDER_TOP_WIDTH,
            Prop::BORDER_TOP_STYLE,
        ]
    } else {
        [
            Prop::PADDING_BOTTOM,
            Prop::BORDER_BOTTOM_WIDTH,
            Prop::BORDER_BOTTOM_STYLE,
        ]
    };
    id != 0
        && !matches!(
            doc.nodes[id].tag.as_str(),
            "html"
                | "img"
                | "svg"
                | "canvas"
                | "video"
                | "audio"
                | "iframe"
                | "embed"
                | "input"
                | "button"
                | "select"
                | "textarea"
                | "progress"
                | "meter"
                | "fieldset"
        )
        && doc.nodes[id]
            .parent
            .is_none_or(|parent| container(build, parent))
        && style
            .get(Prop::DISPLAY)
            .is_some_and(|value| matches!(value, "block" | "list-item"))
        && float_side(style).is_none()
        // A pseudo-element in the flow, like a clearfix, holds the margin of its side.
        && styles.pseudos.get(&id).is_none_or(|pseudos| {
            let pseudo = &pseudos[side / 2];
            pseudo
                .get(Prop::CONTENT)
                .is_none_or(|value| matches!(value, "none" | "normal"))
                || pseudo.get(Prop::DISPLAY) == Some("none")
                || pseudo
                    .get(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"))
        })
        && (body_overflow_moves(doc, styles, id)
            || [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                .into_iter()
                .all(|key| style.get(key).is_none_or(|value| value == "visible")))
        // A multi-column box and a `content-visibility` box hold the margins of their content.
        && style
            .get(Prop::CONTENT_VISIBILITY)
            .is_none_or(|value| value == "visible")
        && [Prop::COLUMN_COUNT, Prop::COLUMN_WIDTH]
            .into_iter()
            .all(|key| style.get(key).is_none_or(|value| value == "auto"))
        // So does a box with `align-content` or a size container, which make a formatting context of their own.
        && style
            .get(Prop::ALIGN_CONTENT)
            .is_none_or(|value| value == "normal")
        && style
            .get(Prop::CONTAINER_TYPE)
            .is_none_or(|value| value == "normal")
        && style
            .get(Prop::POSITION)
            .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
        && edge(padding) == 0.
        && (edge(border) == 0.
            || style
                .get(kind)
                .is_none_or(|value| matches!(value, "none" | "hidden")))
        && (side == 0
            || (side == 3 || style.get(Prop::HEIGHT).is_none_or(|value| value == "auto"))
                && style
                    .get(Prop::MIN_HEIGHT)
                    .is_none_or(|value| value == "auto" || edge(Prop::MIN_HEIGHT) == 0.))
}

// Whether the block around the line of a box has a definite height, which its percent heights resolve against.
fn definite_line(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    let block =
        std::iter::successors(doc.nodes[id].parent, |&node| doc.nodes[node].parent).find(|&node| {
            styles.values[node]
                .get(Prop::DISPLAY)
                .is_some_and(|value| !matches!(value, "inline" | "contents"))
        });
    definite_height(doc, styles, block)
}

fn definite_height(doc: &Document, styles: &ComputedStyles, id: Option<usize>) -> bool {
    let Some(id) = id else { return true };
    let (style, parent) = (&styles.values[id], doc.nodes[id].parent);
    let parent_style = |prop| parent.and_then(|parent| styles.values[parent].get(prop));
    let set = |prop| style.get(prop).is_some_and(|value| value != "auto");
    let stretched = || {
        style
            .get(Prop::ALIGN_SELF)
            .filter(|value| *value != "auto")
            .or_else(|| parent_style(Prop::ALIGN_ITEMS))
            .is_none_or(|value| matches!(value, "normal" | "stretch"))
    };
    match style.get(Prop::HEIGHT) {
        // The root element takes percentages from the viewport.
        Some(height) if height.contains('%') => {
            parent.is_none_or(|parent| parent == 0) || definite_height(doc, styles, parent)
        }
        Some(height) if height != "auto" => styles.lengths[id].resolve(height, 0.).is_some(),
        _ if style
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "absolute" | "fixed")) =>
        {
            set(Prop::TOP) && set(Prop::BOTTOM)
        }
        // A cell takes the height of its table.
        _ if style.get(Prop::DISPLAY) == Some("table-cell") => definite_height(
            doc,
            styles,
            std::iter::successors(parent, |&node| doc.nodes[node].parent)
                .find(|&node| styles.values[node].get(Prop::DISPLAY) == Some("table")),
        ),
        // Stretched grid items, and stretched items of a flex row or growing items of a flex column of a definite
        // height.
        _ => match parent_style(Prop::DISPLAY) {
            Some("grid" | "inline-grid") => stretched(),
            Some("flex" | "inline-flex")
                if parent_style(Prop::FLEX_DIRECTION)
                    .is_none_or(|value| value.starts_with("row")) =>
            {
                stretched() && definite_height(doc, styles, parent)
            }
            Some("flex" | "inline-flex") => {
                style
                    .get(Prop::FLEX_GROW)
                    .and_then(|value| value.parse::<f32>().ok())
                    .is_some_and(|grow| grow > 0.)
                    && definite_height(doc, styles, parent)
            }
            _ => false,
        },
    }
}

// Whether a box around this one shrinks to its content before a box of a fixed width does.
fn shrinks_around(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    std::iter::successors(doc.nodes[id].parent, |&node| doc.nodes[node].parent)
        .take_while(|&node| node != 0)
        .find_map(|node| {
            let style = &styles.values[node];
            let width = style.get(Prop::WIDTH).unwrap_or("auto");
            if styles.lengths[node].resolve(width, 0.).is_some() {
                return Some(false);
            }
            (float_side(style).is_some()
                || style
                    .get(Prop::DISPLAY)
                    .is_some_and(|value| value.starts_with("inline-"))
                || style
                    .get(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"))
                || matches!(width, "max-content" | "min-content" | "fit-content"))
            .then_some(true)
        })
        .unwrap_or(false)
}

fn percent_calc(units: &style::Lengths, value: &str) -> Option<(f32, f32)> {
    // Bounds can look linear at the samples, so `percent_bound` takes them.
    if !value.starts_with("calc(")
        || !value.contains('%')
        || ["min(", "max(", "clamp("]
            .iter()
            .any(|name| value.contains(name))
    {
        return None;
    }
    let low = units.resolve(value, 1000.)?;
    let high = units.resolve(value, 2000.)?;
    let check = units.resolve(value, 3000.)?;
    ((check - high - (high - low)).abs() < 0.01).then(|| {
        let fraction = (high - low) / 1000.;
        (fraction, low - fraction * 1000.)
    })
}

// `min()`, `max()` and `clamp()` with percentages resolve at layout, against the real basis. The bound goes into
// `style`, which can hold 256.
fn percent_bound(
    units: &style::Lengths,
    value: &str,
    extra: f32,
    style: &mut renderer::Style,
) -> Option<renderer::Length> {
    use renderer::Bound;
    enum Value {
        Number(f32),
        Length(Bound),
    }
    // Sums and products spread over the terms of a bound: min(a, b) + c is min(a + c, b + c).
    fn add(a: Bound, b: Bound) -> Bound {
        match (a, b) {
            (Bound::Term(s1, o1), Bound::Term(s2, o2)) => Bound::Term(s1 + s2, o1 + o2),
            (Bound::Min(all), other) | (other, Bound::Min(all)) => {
                Bound::Min(all.into_iter().map(|a| add(a, other.clone())).collect())
            }
            (Bound::Max(all), other) | (other, Bound::Max(all)) => {
                Bound::Max(all.into_iter().map(|a| add(a, other.clone())).collect())
            }
        }
    }
    fn scale(bound: Bound, by: f32) -> Bound {
        let all = |all: Vec<Bound>| all.into_iter().map(|a| scale(a, by)).collect();
        match bound {
            Bound::Term(share, offset) => Bound::Term(share * by, offset * by),
            Bound::Min(list) if by >= 0. => Bound::Min(all(list)),
            Bound::Min(list) => Bound::Max(all(list)),
            Bound::Max(list) if by >= 0. => Bound::Max(all(list)),
            Bound::Max(list) => Bound::Min(all(list)),
        }
    }
    struct Parser<'a> {
        text: &'a str,
        at: usize,
        units: &'a style::Lengths,
    }
    impl Parser<'_> {
        fn peek(&mut self) -> Option<u8> {
            let rest = &self.text[self.at..];
            self.at += rest.len() - rest.trim_start().len();
            self.text.as_bytes().get(self.at).copied()
        }
        fn eat(&mut self, byte: u8) -> Option<()> {
            (self.peek()? == byte).then(|| self.at += 1)
        }
        fn sum(&mut self) -> Option<Value> {
            let mut value = self.product()?;
            loop {
                // A sign between terms has spaces around it.
                let sign = match self.peek() {
                    Some(b'+') => 1.,
                    Some(b'-') => -1.,
                    _ => return Some(value),
                };
                if !self.text.as_bytes()[self.at - 1].is_ascii_whitespace() {
                    return Some(value);
                }
                self.at += 1;
                value = match (value, self.product()?) {
                    (Value::Number(a), Value::Number(b)) => Value::Number(a + sign * b),
                    (Value::Length(a), Value::Length(b)) => Value::Length(add(a, scale(b, sign))),
                    _ => return None,
                };
            }
        }
        fn product(&mut self) -> Option<Value> {
            let mut value = self.factor()?;
            loop {
                let divide = match self.peek() {
                    Some(b'*') => false,
                    Some(b'/') => true,
                    _ => return Some(value),
                };
                self.at += 1;
                value = match (value, self.factor()?, divide) {
                    (Value::Number(a), Value::Number(b), false) => Value::Number(a * b),
                    (Value::Number(a), Value::Number(b), true) => Value::Number(a / b),
                    (Value::Length(a), Value::Number(b), false)
                    | (Value::Number(b), Value::Length(a), false) => Value::Length(scale(a, b)),
                    (Value::Length(a), Value::Number(b), true) if b != 0. => {
                        Value::Length(scale(a, 1. / b))
                    }
                    _ => return None,
                };
            }
        }
        fn factor(&mut self) -> Option<Value> {
            if self.eat(b'(').is_some() {
                let value = self.sum()?;
                return self.eat(b')').map(|_| value);
            }
            let rest = &self.text[self.at..];
            let end = rest
                .find(|c: char| c.is_ascii_whitespace() || matches!(c, ',' | '(' | ')' | '*' | '/'))
                .unwrap_or(rest.len());
            let token = &rest[..end];
            self.at += end;
            if self.text.as_bytes().get(self.at) != Some(&b'(') {
                if let Ok(number) = token.parse::<f32>() {
                    return Some(Value::Number(number));
                }
                // A plain length or percentage, which is linear in its basis.
                let [low, high] = [1000., 2000.].map(|basis| self.units.resolve(token, basis));
                let (low, high) = (low?, high?);
                let share = (high - low) / 1000.;
                return Some(Value::Length(Bound::Term(share, low - share * 1000.)));
            }
            self.at += 1;
            let mut args = vec![self.sum()?];
            while self.eat(b',').is_some() {
                args.push(self.sum()?);
            }
            self.eat(b')')?;
            if token == "calc" && args.len() == 1 {
                return args.pop();
            }
            let mut args = args
                .into_iter()
                .map(|arg| match arg {
                    Value::Length(bound) => Some(bound),
                    Value::Number(_) => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Value::Length(match token {
                "min" => Bound::Min(args),
                "max" => Bound::Max(args),
                "clamp" if args.len() == 3 => {
                    let high = args.pop()?;
                    let value = args.pop()?;
                    args.push(Bound::Min(vec![value, high]));
                    Bound::Max(args)
                }
                _ => return None,
            }))
        }
    }
    if !value.contains('%') || !value.starts_with(['m', 'c']) {
        return None;
    }
    let index = u8::try_from(style.bounds.len()).ok()?;
    let mut parser = Parser {
        text: value,
        at: 0,
        units,
    };
    let Value::Length(bound) = parser.sum()? else {
        return None;
    };
    if parser.peek().is_some() {
        return None;
    }
    style.bounds.push(add(bound, Bound::Term(0., extra)));
    Some(renderer::Length::Bound(index))
}

// `around` is the size of the container around the box, for container units.
fn adjoining(build: &Build<'_>, id: usize, width: f32, around: Option<[f32; 2]>) -> Sets {
    if let Some(&sets) = build.adjoins.borrow().get(&id) {
        return sets;
    }
    let key = (
        build.info[id].revision,
        width.to_bits(),
        around.map(|size| size.map(f32::to_bits)),
    );
    let animated = build.info[id].flags & cull::ANIMATED != 0;
    if !animated
        && let Some(&(old, sets)) = build.view.adjoined.borrow().get(&id)
        && old == key
    {
        build.adjoins.borrow_mut().insert(id, sets);
        return sets;
    }
    let (style, mut units) = (&build.styles.values[id], build.styles.lengths[id]);
    units.container = around.or(units.container);
    let own = |prop: Prop| {
        let value = style
            .get(prop)
            .and_then(|value| units.resolve(value, width))
            .unwrap_or(0.);
        [value.max(0.), value.min(0.)]
    };
    let (mut top, mut bottom) = (own(Prop::MARGIN_TOP), own(Prop::MARGIN_BOTTOM));
    // This recurses into first and last children, so margins stop collapsing past the stack budget.
    let marker = 0u8;
    let deep = (&raw const marker as usize) < STACK_LIMIT.with(|limit| *limit);
    let blocks = container(build, id) && !deep;
    let [first, last] =
        [0, 2].map(|side| margin_collapses_through(build, id, side, width) && blocks);
    let children = if first || last {
        flow(build, id)
    } else {
        Vec::new()
    };
    for (open, set, order) in [(first, &mut top, false), (last, &mut bottom, true)] {
        if !open {
            continue;
        }
        for index in 0..children.len() {
            let child = children[if order {
                children.len() - 1 - index
            } else {
                index
            }];
            if !block(build, child) {
                if style::makes_no_line(build.doc, build.styles, child) {
                    continue;
                }
                break;
            }
            let (child_top, child_bottom, empty) = adjoining(build, child, width, around);
            for adjoining in if order {
                [child_bottom, child_top]
            } else {
                [child_top, child_bottom]
            }
            .into_iter()
            .take(if empty { 2 } else { 1 })
            {
                *set = [set[0].max(adjoining[0]), set[1].min(adjoining[1])];
            }
            if !empty {
                break;
            }
        }
    }
    let empty = first
        && (last || blocks && margin_collapses_through(build, id, 3, width))
        && style
            .get(Prop::HEIGHT)
            .is_none_or(|value| value == "auto" || units.resolve(value, width) == Some(0.))
        && children.iter().all(|&child| {
            if block(build, child) {
                adjoining(build, child, width, around).2
            } else {
                style::makes_no_line(build.doc, build.styles, child)
            }
        });
    // An empty box keeps its own top set, which places it, and its bottom set holds all its margins.
    if empty {
        bottom = [top[0].max(bottom[0]), top[1].min(bottom[1])];
    }
    build.adjoins.borrow_mut().insert(id, (top, bottom, empty));
    if !animated && !deep {
        build
            .view
            .adjoined
            .borrow_mut()
            .insert(id, (key, (top, bottom, empty)));
    }
    (top, bottom, empty)
}

pub(super) fn overscroll(style: &style::Style) -> renderer::Point<renderer::Overscroll> {
    let axis = |prop| match style.get(prop) {
        Some("contain") => renderer::Overscroll::Contain,
        Some("none") => renderer::Overscroll::None,
        _ => renderer::Overscroll::Auto,
    };
    point(
        axis(Prop::OVERSCROLL_BEHAVIOR_X),
        axis(Prop::OVERSCROLL_BEHAVIOR_Y),
    )
}

fn clear_bits(style: &style::Style) -> u8 {
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    match style.get(Prop::CLEAR) {
        Some("left") => renderer::Flow::CLEAR_LEFT,
        Some("right") => renderer::Flow::CLEAR_RIGHT,
        Some("both") => renderer::Flow::CLEAR_LEFT | renderer::Flow::CLEAR_RIGHT,
        Some("inline-start") if rtl => renderer::Flow::CLEAR_RIGHT,
        Some("inline-start") => renderer::Flow::CLEAR_LEFT,
        Some("inline-end") if rtl => renderer::Flow::CLEAR_LEFT,
        Some("inline-end") => renderer::Flow::CLEAR_RIGHT,
        _ => 0,
    }
}

fn float_side(style: &style::Style) -> Option<bool> {
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    match style.get(Prop::FLOAT) {
        Some("left") => Some(false),
        Some("right") => Some(true),
        Some("inline-start") => Some(rtl),
        Some("inline-end") => Some(!rtl),
        _ => None,
    }
}

type ColumnRule = (f32, renderer::Color, renderer::BorderStyle);

fn columns(
    style: &style::Style,
    units: &style::Lengths,
    width: f32,
) -> Option<(usize, f32, Option<ColumnRule>)> {
    let count = style
        .get(Prop::COLUMN_COUNT)
        .and_then(|value| value.parse::<usize>().ok());
    let column_width = style
        .get(Prop::COLUMN_WIDTH)
        .and_then(|value| units.resolve(value, width));
    if count.is_none() && column_width.is_none() {
        return None;
    }
    let gap = style
        .get(Prop::COLUMN_GAP)
        .filter(|value| *value != "normal")
        .and_then(|value| units.resolve(value, width))
        .unwrap_or(units.font.em)
        .max(0.);
    let fit = column_width
        .filter(|value| *value > 0.)
        .map(|value| (((width + gap) / (value + gap)).floor() as usize).max(1));
    let count = match (count, fit) {
        (Some(count), Some(fit)) => count.min(fit),
        (count, fit) => count.or(fit)?,
    }
    .clamp(1, 64);
    if count < 2 {
        return None;
    }
    let (rule_width, rule_style, rule_color) = (
        style.get(Prop::COLUMN_RULE_WIDTH),
        style.get(Prop::COLUMN_RULE_STYLE),
        style.get(Prop::COLUMN_RULE_COLOR),
    );
    let rule = rule_style
        .filter(|kind| !matches!(*kind, "none" | "hidden"))
        .map(|kind| {
            let width = rule_width
                .map_or(Some(3.), |value| units.resolve(value, width))
                .unwrap_or(0.);
            let color = rule_color
                .or_else(|| style.get(Prop::COLOR))
                .and_then(style::paint)
                .unwrap_or(renderer::Color::BLACK);
            (width, color, border_style(kind))
        })
        .filter(|(width, ..)| *width > 0.);
    Some((count, gap, rule))
}

// A set of columns: its first child holds the flow of content, as wide as one column, which the renderer breaks.
fn column_set(
    style: &style::Style,
    content: [f32; 2],
    count: usize,
    gap: f32,
    rule: Option<ColumnRule>,
    alone: bool,
    items: impl IntoIterator<Item = impl Into<Child>>,
) -> renderer::Node {
    // A height or max-height limits the columns, which then overflow to the right.
    let fixed = style.get(Prop::HEIGHT).is_some_and(|value| value != "auto");
    let limited = fixed
        || style
            .get(Prop::MAX_HEIGHT)
            .is_some_and(|value| value != "none");
    let mut set = renderer::node().flex().flex_col().w_full().flex_shrink_0();
    set.style().columns = Some(Box::new(renderer::Columns {
        count: count as u16,
        gap: px(gap),
        height: (limited && alone).then_some(px(content[1])),
        fill: fixed && style.get(Prop::COLUMN_FILL) == Some("auto"),
    }));
    let share = renderer::Length::Calc(
        1. / count as f32,
        px(-gap * (count - 1) as f32 / count as f32),
    );
    set = set.child(renderer::node().flex().flex_col().w(share).children(items));
    match rule {
        Some((width, color, kind)) => set.child(column_rules(count, gap, width, color, kind)),
        None => set,
    }
}

fn column_rules(
    count: usize,
    gap: f32,
    width: f32,
    color: renderer::Color,
    kind: renderer::BorderStyle,
) -> Child {
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            let column = (f32::from(bounds.size.width) - gap * (count - 1) as f32) / count as f32;
            for index in 1..count {
                let x = bounds.left() + px(index as f32 * (column + gap) - gap / 2. - width / 2.);
                window.draw_box(
                    Rect::new(point(x, bounds.top()), size(px(width), bounds.size.height)),
                    renderer::Corners::default(),
                    renderer::Color::TRANSPARENT,
                    renderer::Border {
                        widths: renderer::Edges {
                            left: px(width),
                            ..Default::default()
                        },
                        color,
                        style: kind,
                        sides: 0,
                    },
                );
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into()
}

pub(super) fn legend(
    doc: &Document,
    styles: &style::ComputedStyles,
    fieldset: usize,
) -> Option<(usize, f32, f32)> {
    let corners = [
        Prop::BORDER_TOP_LEFT_RADIUS,
        Prop::BORDER_TOP_RIGHT_RADIUS,
        Prop::BORDER_BOTTOM_RIGHT_RADIUS,
        Prop::BORDER_BOTTOM_LEFT_RADIUS,
    ];
    if corners.into_iter().any(|corner| {
        styles.values[fieldset]
            .get(corner)
            .is_some_and(|value| !matches!(value, "0" | "0px"))
    }) {
        return None;
    }
    let legend = doc.nodes[fieldset]
        .children
        .iter()
        .copied()
        .find(|&child| {
            !doc.nodes[child].tag.starts_with('#')
                && styles.values[child].get(Prop::DISPLAY) != Some("none")
        })?;
    let style = &styles.values[legend];
    if doc.nodes[legend].tag != "legend"
        || style.get(Prop::FLOAT).is_some_and(|value| value != "none")
        || style
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "absolute" | "fixed"))
    {
        return None;
    }
    let units = styles.lengths[legend];
    let padding = units.edges(style, style::PADDING, 0.);
    let border = |[width, kind, _]: [Prop; 3]| {
        if style
            .get(kind)
            .is_none_or(|kind| matches!(kind, "none" | "hidden"))
        {
            0.
        } else {
            style
                .get(width)
                .and_then(|value| units.resolve(value, 0.))
                .unwrap_or(3.)
        }
    };
    let height = units.font.lh
        + padding[0]
        + padding[2]
        + border(style::BORDERS[0])
        + border(style::BORDERS[2]);
    let margin = units.edges(style, style::MARGIN, 0.);
    Some((
        legend,
        margin[0] + height + margin[2],
        margin[0] + height / 2.,
    ))
}

fn fieldset_border(
    legend: usize,
    height: f32,
    middle: f32,
    widths: [f32; 4],
    colors: [renderer::Color; 4],
    kinds: [&str; 4],
    view: Rc<super::PageState>,
) -> Child {
    let kinds = kinds.map(str::to_owned);
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            // The legend can move without a new box here.
            window.paint_volatile();
            let gap = view
                .host
                .borrow()
                .dom
                .get(&view.id)
                .and_then(|dom| dom.bounds.get(&legend).copied())
                .map(|[x, _, width, _]| [px(x), px(x + width)]);
            let top = bounds.top() + px(middle - widths[0] / 2.);
            let [left, right, bottom] = [bounds.left(), bounds.right(), bounds.bottom()];
            let sides = [
                Rect::between(point(left, top), point(right, top + px(widths[0]))),
                Rect::between(point(right - px(widths[1]), top), point(right, bottom)),
                Rect::between(point(left, bottom - px(widths[2])), point(right, bottom)),
                Rect::between(point(left, top), point(left + px(widths[3]), bottom)),
            ];
            for (side, rect) in sides.into_iter().enumerate() {
                if widths[side] <= 0. {
                    continue;
                }
                let raised = side == 0 || side == 3;
                let (outer, inner) = match kinds[side].as_str() {
                    "groove" => (
                        shade_border(colors[side], raised),
                        shade_border(colors[side], !raised),
                    ),
                    "ridge" => (
                        shade_border(colors[side], !raised),
                        shade_border(colors[side], raised),
                    ),
                    "inset" => (
                        shade_border(colors[side], raised),
                        shade_border(colors[side], raised),
                    ),
                    "outset" => (
                        shade_border(colors[side], !raised),
                        shade_border(colors[side], !raised),
                    ),
                    _ => (colors[side], colors[side]),
                };
                let half = px(widths[side] / 2.);
                let halves = match side {
                    0 => [
                        (rect.top(), rect.top() + half),
                        (rect.top() + half, rect.bottom()),
                    ],
                    2 => [
                        (rect.bottom() - half, rect.bottom()),
                        (rect.top(), rect.bottom() - half),
                    ],
                    1 => [
                        (rect.right() - half, rect.right()),
                        (rect.left(), rect.right() - half),
                    ],
                    _ => [
                        (rect.left(), rect.left() + half),
                        (rect.left() + half, rect.right()),
                    ],
                };
                for ((start, end), color) in halves.into_iter().zip([outer, inner]) {
                    let part = if side % 2 == 0 {
                        Rect::between(point(rect.left(), start), point(rect.right(), end))
                    } else {
                        Rect::between(point(start, rect.top()), point(end, rect.bottom()))
                    };
                    let pieces = match gap.filter(|_| side == 0) {
                        Some([from, to]) => vec![
                            Rect::between(part.origin, point(from.max(part.left()), part.bottom())),
                            Rect::between(
                                point(to.min(part.right()), part.top()),
                                part.bottom_right(),
                            ),
                        ],
                        None => vec![part],
                    };
                    for piece in pieces.into_iter().filter(|piece| piece.size.width > px(0.)) {
                        window.draw_rect(piece, color);
                    }
                }
            }
        },
    )
    .absolute()
    .top(px(-height))
    .left(px(-widths[3]))
    .right(px(-widths[1]))
    .bottom(px(-widths[2]))
    .into()
}

pub(super) fn border_style(kind: &str) -> renderer::BorderStyle {
    match kind {
        "dashed" => renderer::BorderStyle::Dashed,
        "dotted" => renderer::BorderStyle::Dotted,
        "double" => renderer::BorderStyle::Double,
        _ => renderer::BorderStyle::Solid,
    }
}

fn shade_border(color: renderer::Color, dark: bool) -> renderer::Color {
    let value = color.r.max(color.g).max(color.b);
    if value <= 0. {
        let gray = if dark { 0.33 } else { 0.66 };
        return renderer::Color {
            r: gray,
            g: gray,
            b: gray,
            a: color.a,
        };
    }
    let end = if dark { 0. } else { 1. };
    let distance = (color.r - end).powi(2) + (color.g - end).powi(2) + (color.b - end).powi(2);
    if distance <= 3. * (if dark { 32. } else { 20. } / 255.0f32).powi(2) {
        return color;
    }
    let factor = if dark {
        ((value - 0.33) / value).max(0.)
    } else {
        (value + 0.33).min(1.) / value
    };
    renderer::Color {
        r: (color.r * factor).min(1.),
        g: (color.g * factor).min(1.),
        b: (color.b * factor).min(1.),
        a: color.a,
    }
}

fn border_canvas(
    bands: Vec<([f32; 4], [f32; 4], [renderer::Color; 4])>,
    borders: [f32; 4],
    radius: renderer::Radii<renderer::Length>,
    kinds: [renderer::BorderStyle; 4],
    pseudo: bool,
) -> renderer::Node {
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            let outer = Rect::new(
                point(
                    bounds.left() - px(borders[3]),
                    bounds.top() - px(borders[0]),
                ),
                size(
                    bounds.size.width + px(borders[1] + borders[3]),
                    bounds.size.height + px(borders[0] + borders[2]),
                ),
            );
            for (inset, widths, colors) in &bands {
                // Pseudo-element borders paint on `outer`, box borders on the rect between the insets: the two
                // differ by an ulp at fractional positions.
                let bounds = if pseudo {
                    outer
                } else {
                    Rect::between(
                        outer.origin + point(px(inset[3]), px(inset[0])),
                        outer.bottom_right() - point(px(inset[1]), px(inset[2])),
                    )
                };
                let corners = radius
                    .resolve(outer.size)
                    .inset(px(inset[0]), px(inset[1]), px(inset[2]), px(inset[3]))
                    .fit(bounds.size);
                // Sides that look the same draw at once, so their joins have no seams.
                let same = |a: usize, b: usize| {
                    (colors[a], kinds[a]) == (colors[b], kinds[b]) && widths[b] > 0.
                };
                for side in 0..4 {
                    if widths[side] <= 0. || (0..side).any(|before| same(side, before)) {
                        continue;
                    }
                    let sides = (side..4)
                        .filter(|&other| same(side, other))
                        .fold(0, |sides, other| sides | 1 << other);
                    window.draw_box(
                        bounds,
                        corners,
                        renderer::Color::TRANSPARENT,
                        renderer::Border {
                            widths: renderer::Edges {
                                top: px(widths[0]),
                                right: px(widths[1]),
                                bottom: px(widths[2]),
                                left: px(widths[3]),
                            },
                            color: colors[side],
                            style: kinds[side],
                            sides: if sides == 15 { 0 } else { sides },
                        },
                    );
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn inset_shadows(
    shadows: Vec<renderer::BoxShadow>,
    radius: renderer::Radii<renderer::Length>,
    borders: [f32; 4],
) -> Child {
    let mut node = renderer::canvas_within(
        |_, _| (),
        move |bounds, _, window| {
            let outer = size(
                bounds.size.width + px(borders[1] + borders[3]),
                bounds.size.height + px(borders[0] + borders[2]),
            );
            let radii = radius.resolve(outer).inset(
                px(borders[0]),
                px(borders[1]),
                px(borders[2]),
                px(borders[3]),
            );
            window.draw_shadows(bounds, radii, &shadows, true)
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    // Like the background, the shadow doesn't scroll with the content or add to its scroll range.
    node.style().escape = renderer::Escape::OUTLINE;
    node.into()
}

/// `padding` places the drawing in the content box, as the layer is absolute in the padding box.
fn svg_layer(
    id: usize,
    inline: bool,
    spill: bool,
    view: Rc<super::PageState>,
    padding: [f32; 4],
) -> Child {
    let geometry = view.clone();
    renderer::canvas(
        move |bounds, window| {
            let [width, height] = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            if !inline || width <= 0. || height <= 0. {
                return;
            }
            let mut host = geometry.host.borrow_mut();
            let state = host.dom.entry(geometry.id).or_default();
            if let Some(source) = state.svg_sources.get(&id)
                && let Some(entry) = state.svg_boxes.get_mut(&id)
            {
                if entry.0 != *source {
                    // Drop stale svg_trees entries here: repeated draws can skip paint and leave old sources
                    // cached.
                    state.svg_trees.remove(&id);
                    let (size, content, boxes, painted) = match state.svg_cache.get(source) {
                        Some((geometry, _)) => geometry.clone(),
                        None => {
                            let tree = super::canvas::svg_tree(source, false).ok();
                            let geometry = tree
                                .as_ref()
                                .map(super::canvas::svg_boxes)
                                .unwrap_or_default();
                            if let Some(tree) = tree {
                                state.svg_trees.insert(id, tree);
                                // Bound the cache here: this is the only place that adds sources.
                                if state.svg_cache.len() >= 256 {
                                    for image in
                                        state.svg_cache.drain().flat_map(|(_, entry)| entry.1)
                                    {
                                        if Arc::strong_count(&image) == 1 {
                                            window.release_image(image);
                                        }
                                    }
                                }
                                state
                                    .svg_cache
                                    .insert(source.clone(), (geometry.clone(), Vec::new()));
                            }
                            geometry
                        }
                    };
                    let names = &entry.1;
                    entry.2 = size;
                    entry.4 = content;
                    entry.5 = painted;
                    entry.3 = boxes
                        .into_iter()
                        .filter_map(|(name, canvas, object)| {
                            let node = match name.strip_prefix("apps-") {
                                Some(mark) => names.get(mark.parse::<usize>().ok()?)?.1,
                                None => names.iter().find(|(other, _)| *other == name)?.1,
                            };
                            Some((node, canvas, object))
                        })
                        .collect();
                    entry.0 = source.clone();
                }
                let [scale_x, scale_y] =
                    [0, 1].map(|axis| [width, height][axis] / entry.2[axis].max(f32::EPSILON));
                let [x, y, w, h] = entry.5;
                let reach = (-x * scale_x)
                    .max(-y * scale_y)
                    .max((x + w) * scale_x - width)
                    .max((y + h) * scale_y - height);
                // Like the cull margin of a box shadow.
                let far = spill && reach >= f32::from(window.viewport_size().height);
                if far != state.cull.spills.contains(&id) {
                    match far {
                        true => state.cull.spills.insert(id),
                        false => state.cull.spills.remove(&id),
                    };
                    state.cull.swapped.push(id);
                    state.cull.respilled.push(id);
                    window.redraw();
                }
                for &(node, [x, y, w, h], _) in &entry.3 {
                    let rect = window.transformed_bounds(Rect::new(
                        point(
                            bounds.left() + px(x * scale_x),
                            bounds.top() + px(y * scale_y),
                        ),
                        size(px(w * scale_x), px(h * scale_y)),
                    ));
                    state.bounds.insert(
                        node,
                        [
                            rect.origin.x.into(),
                            rect.origin.y.into(),
                            rect.size.width.into(),
                            rect.size.height.into(),
                        ],
                    );
                }
            }
        },
        move |bounds, _, window| {
            let [width, height] = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            if width <= 0. || height <= 0. {
                return;
            }
            let scale = window.scale_factor();
            let view = &*view;
            let mut host = view.host.borrow_mut();
            let state = host.dom.entry(view.id).or_default();
            // The painted area in the box, in CSS pixels on device pixel edges. A long spill paints only near
            // the content mask, in steps, so it keeps the device scale.
            let mask = window.content_mask().bounds;
            let mut cut = false;
            let area = match state.svg_boxes.get(&id).filter(|_| spill) {
                Some((_, _, [sw, sh], _, _, [x, y, w, h], _)) => {
                    let [kx, ky] = [width / sw.max(f32::EPSILON), height / sh.max(f32::EPSILON)];
                    let mut edges = [
                        (x * kx).min(0.),
                        (y * ky).min(0.),
                        ((x + w) * kx).max(width),
                        ((y + h) * ky).max(height),
                    ];
                    if mask.size.width < px(1e5) {
                        const STEP: f32 = 256.;
                        let [left, top] = [mask.left() - bounds.left(), mask.top() - bounds.top()]
                            .map(|at| ((f32::from(at) - STEP) / STEP).floor() * STEP);
                        let [right, bottom] =
                            [mask.right() - bounds.left(), mask.bottom() - bounds.top()]
                                .map(|at| ((f32::from(at) + STEP) / STEP).ceil() * STEP);
                        let near = [
                            edges[0].max(left),
                            edges[1].max(top),
                            edges[2].min(right),
                            edges[3].min(bottom),
                        ];
                        cut = near != edges;
                        edges = near;
                    }
                    let [left, top] = [edges[0], edges[1]].map(|at| (at * scale).floor() / scale);
                    let [right, bottom] =
                        [edges[2], edges[3]].map(|at| (at * scale).ceil() / scale);
                    [left, top, right - left, bottom - top]
                }
                _ => [0., 0., width, height],
            };
            if cut {
                window.paint_volatile();
            }
            if area[2] <= 0. || area[3] <= 0. {
                return;
            }
            let moved = area != [0., 0., width, height];
            let raster =
                [area[2], area[3]].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
            let source = if inline {
                state.svg_sources.get(&id).cloned()
            } else {
                state.svg_images.get(&id).map(|(source, _)| source.clone())
            };
            let Some(source) = source else { return };
            if state.svgs.get(&id).is_none_or(|(old, image)| {
                !inline && *old != source
                    || (image.size().width as u32) != raster[0]
                    || (image.size().height as u32) != raster[1]
            }) || moved
                && state
                    .svg_boxes
                    .get(&id)
                    .is_some_and(|entry| entry.6 != area)
            {
                if let Some(entry) = state.svg_boxes.get_mut(&id) {
                    entry.6 = area;
                }
                let shared = state
                    .svg_cache
                    .get(&source)
                    .filter(|_| inline && !moved)
                    .and_then(|(_, images)| {
                        images.iter().find(|image| {
                            [image.size().width as u32, image.size().height as u32] == raster
                        })
                    })
                    .cloned();
                let image = match (&shared, state.svg_trees.remove(&id).filter(|_| inline)) {
                    (Some(image), _) => Ok(image.clone()),
                    (_, Some(tree)) if !moved => {
                        super::canvas::render_svg(&tree, raster[0], raster[1], None)
                    }
                    (_, tree) if moved => tree
                        .map_or_else(|| super::canvas::svg_tree(&source, false), Ok)
                        .and_then(|tree| {
                            let [kx, ky] = [raster[0] as f32 / area[2], raster[1] as f32 / area[3]];
                            super::canvas::render_svg(
                                &tree,
                                raster[0],
                                raster[1],
                                Some((
                                    [width * kx, height * ky],
                                    [1., 0., 0., 1., -area[0] * kx, -area[1] * ky],
                                )),
                            )
                        }),
                    _ => super::canvas::svg(&source, raster[0], raster[1]),
                };
                match image {
                    Ok(image) => {
                        if inline
                            && shared.is_none()
                            && raster[0] <= 96
                            && raster[1] <= 96
                            && !moved
                            && let Some((_, images)) = state.svg_cache.get_mut(&source)
                        {
                            // Keep the rasters of the last few sizes, so a resized icon doesn't grow the list.
                            if images.len() >= 4 {
                                let old = images.remove(0);
                                if Arc::strong_count(&old) == 1 {
                                    window.release_image(old);
                                }
                            }
                            images.push(image.clone());
                        }
                        if let Some((_, old)) = state.svgs.insert(id, (source, image))
                            && Arc::strong_count(&old) == 1
                        {
                            window.release_image(old);
                        }
                    }
                    Err(error) => eprintln!("apps: SVG: {error}"),
                }
            }
            let image = state.svgs.get(&id).map(|(_, image)| image.clone());
            drop(host);
            if let Some(image) = image {
                let target = Rect::new(
                    point(bounds.left() + px(area[0]), bounds.top() + px(area[1])),
                    size(px(area[2]), px(area[3])),
                );
                window.draw_image(
                    target,
                    target,
                    renderer::Corners::default(),
                    image,
                    false,
                    false,
                );
            }
        },
    )
    .absolute()
    .top(px(padding[0]))
    .right(px(padding[1]))
    .bottom(px(padding[2]))
    .left(px(padding[3]))
    .into()
}

#[cfg(test)]
mod tests {
    #[test]
    fn dimension_attributes_parse_like_html() {
        for (value, integer, expected) in [
            ("300", true, Some(300.)),
            (" 100px", true, Some(100.)),
            ("1e9", true, Some(1.)),
            ("2.5", true, Some(2.)),
            ("2.5", false, Some(2.5)),
            ("NaN", true, None),
            ("inf", false, None),
            ("-50", true, None),
            ("50%", false, None),
            ("50%", true, Some(50.)),
            ("+7", true, Some(7.)),
            ("+7", false, None),
            ("2147483648", true, None),
            ("", true, None),
            ("99999999999", true, None),
        ] {
            assert_eq!(super::dimension(value, integer), expected, "{value}");
        }
    }
}
