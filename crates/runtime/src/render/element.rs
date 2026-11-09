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
use renderer::{Bounds, Child, MouseButton, SharedString, point, px, relative, rgba, size};
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
    pub(super) siblings: &'a RefCell<HashMap<(usize, bool), Rc<Siblings>>>,
    pub(super) margins: &'a RefCell<Margins>,
    pub(super) blocks: &'a RefCell<rustc_hash::FxHashMap<usize, [f32; 2]>>,
    pub(super) ids: &'a Rc<super::effects::Ids>,
    pub(super) info: &'a [cull::Info],
    pub(super) zone: Option<Rc<cull::Zone>>,
    pub(super) top_layer: &'a [usize],
    pub(super) tree: &'a renderer::Tree,
    pub(super) culls: &'a std::cell::Cell<usize>,
    pub(super) resized: bool,
    pub(super) settled: bool,
    pub(super) reads: &'a RefCell<Vec<Read>>,
    // WARNING: moves when a node, a style, the focus, an animated value or the frame may have changed, so the page
    // reads of a record checked in this epoch still hold.
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

// WARNING: a box is kept while its subtree revision and call are the same and every read its build made beyond them
// gives what it gave then. A build reads other boxes, the host and the frame only through `Build::log`, or the box is
// never kept, see `element`. The queries up to `Shrunk` replay at the box whose record holds them, the rest name
// what they read.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
    Backdrop,
    Scroller,
    Editable,
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
    Value(u32),
    Viewport,
    Theme,
}

impl Query {
    // WARNING: the last layout, scroll handles and the system theme change without a new node or style, so these are
    // checked in every frame, the others only in a new epoch.
    fn host(self) -> bool {
        matches!(
            self,
            Query::Scroller
                | Query::Block(_)
                | Query::Scroll(_)
                | Query::Width(_)
                | Query::Track(..)
                | Query::Baselines(_)
                | Query::Atom(_)
                | Query::Image(_)
                | Query::Viewport
                | Query::Theme
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Read {
    query: Query,
    value: u64,
    // WARNING: in the build log, `scope` is one more than the depth of the box whose subtree revision covers the read,
    // 0 when none does, and `reader` is the depth of the box that read it. In a record, `scope` counts boxes up from
    // the record's box and `reader` is 1 for reads of the boxes below it.
    scope: u16,
    reader: u16,
}

// WARNING: what a text field was last built with. Its value is in no revision, so when only the value changed, the
// frame builds the field again from this in place, under its kept boxes, see `render_page`.
pub(super) struct Patch {
    pub(super) ancestors: Rc<[usize]>,
    pub(super) containing: [f32; 2],
    pub(super) container: [f32; 2],
    pub(super) slot: Option<([f32; 4], super::grid::Grid)>,
    pub(super) flex_parent: bool,
    pub(super) perspective: Option<(f32, String)>,
    pub(super) value: u64,
}

pub(super) struct Kept {
    pub(super) call: Call,
    pub(super) reads: Box<[Read]>,
    epoch: std::cell::Cell<u64>,
    pub(super) culled: bool,
    #[cfg(debug_assertions)]
    pub(super) print: u64,
    // WARNING: the children that the box's build culled, which its debug check leaves out too.
    #[cfg(debug_assertions)]
    plan: Rc<[usize]>,
}

#[derive(PartialEq)]
pub(super) struct Call {
    subtree: u64,
    containing: [f32; 2],
    container: [f32; 2],
    flex_parent: bool,
    cell: Option<([f32; 2], bool, bool, u64)>,
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
    doc.nodes[id]
        .parent
        .and_then(|parent| styles.values[parent].get(prop))
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

// WARNING: the nearest ancestor that makes a box, as `display: contents` makes none.
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

fn padding_left(styles: &ComputedStyles, parent: usize, width: f32) -> f32 {
    styles.lengths[parent].edges(&styles.values[parent], style::PADDING, width)[3].max(0.)
}

fn starts_line(doc: &Document, id: usize) -> bool {
    doc.nodes[id].parent.is_none_or(|parent| {
        let siblings = &doc.nodes[parent].children;
        siblings[..siblings.iter().position(|&child| child == id).unwrap_or(0)]
            .iter()
            .rfind(|&&child| {
                let node = &doc.nodes[child];
                node.tag != "#comment" && !(node.tag == "#text" && node.text.trim_ascii().is_empty())
            })
            .is_none_or(|&child| doc.nodes[child].tag == "br")
    })
}

fn insets(styles: &ComputedStyles, node: usize, padding: bool, width: f32) -> [f32; 4] {
    let (style, units) = (&styles.values[node], &styles.lengths[node]);
    let padding = if padding {
        units.edges(style, style::PADDING, width)
    } else {
        [0.; 4]
    };
    std::array::from_fn(|side| {
        let [size, kind, _] = style::BORDERS[side];
        padding[side]
            + if style
                .get(kind)
                .is_none_or(|kind| matches!(kind, "none" | "hidden"))
            {
                0.
            } else {
                style
                    .get(size)
                    .map_or(Some(3.), |v| units.resolve(v, width))
                    .unwrap_or(0.)
                    .max(0.)
            }
    })
}

// WARNING: the nearest opaque background from the box up, and how many boxes up it is.
fn backdrop(styles: &ComputedStyles, ancestors: &[usize], id: usize) -> Option<(usize, u32)> {
    std::iter::once(id)
        .chain(ancestors.iter().rev().copied())
        .enumerate()
        .find_map(|(up, node)| {
            let (ink, alpha) = style::color(styles.values[node].get(Prop::BACKGROUND_COLOR)?)?;
            (alpha >= 128).then_some((up, ink))
        })
}

// WARNING: native controls draw these node states, which no revision covers.
const LOOK: u16 = crate::dom::HOVER | crate::dom::ACTIVE;

// WARNING: the select of an option, and whether the option shows inside the select's box as a drop-down's choice
// instead of in a box of its own.
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

// WARNING: whether a grid without a set width shrinks to its content, and the farthest node the walk read. `first`
// is false when the walk goes on from a box above the grid.
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
        let Some(parent) = doc.nodes[child].parent else {
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
        // WARNING: a column stretches to its own width, which can come from its content, as in a popup.
        if !matches!(display, "block" | "inline-block" | "flex" | "inline-flex")
            || style.get(Prop::WIDTH).is_some_and(|v| v != "auto")
        {
            break (false, parent);
        }
        if display == "inline-block" {
            break (true, parent);
        }
        child = parent;
    }
}

// WARNING: what the `<use>` elements of an SVG take from the rest of the page, which its subtree revision does not
// cover, or `None` without such elements.
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
            && let Some(name) = current
                .attrs
                .get("href")
                .or_else(|| current.attrs.get("xlink:href"))
                .and_then(|value| value.strip_prefix('#'))
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

fn scrolled(handle: Option<&renderer::ScrollHandle>) -> Option<[bool; 2]> {
    handle.map(|handle| {
        let max = handle.max_offset();
        [max.x > px(0.5), max.y > px(0.5)]
    })
}

// WARNING: a raster image paints by node with `paint::Picture`, and an SVG image paints its source by node, so only
// their sizes count, and a new video frame of the same size keeps the box.
fn pictured(
    image: Option<&Arc<renderer::RenderImage>>,
    vector: Option<&(Rc<str>, [f32; 2])>,
) -> u64 {
    hashed((
        image.map(|image| {
            let size = image.size();
            [size.width.0, size.height.0]
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
    // WARNING: `scope` is one more than the depth of the box whose subtree revision covers the read, or 0 when none
    // does: `ancestors.len()` for the parent, one more for the reading box itself, which drops the read.
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

    // WARNING: the scope of a read of `node`, the box itself or one of its DOM ancestors: the nearest box of the chain
    // that holds it.
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
        slot: bool,
        dom: Option<&crate::dom::State>,
        read: &Read,
    ) -> Replay {
        let (doc, styles) = (self.doc, self.styles);
        let below = read.reader != 0;
        let now = match read.query {
            Query::Margins => {
                let mut units = styles.lengths[id];
                units.container = Some(container);
                hashed(margins(self, id, &units, width, self.flex_parent, slot).map(f32::to_bits))
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
            Query::FlatParentPadding => hashed(
                box_parent(doc, styles, id)
                    .map(|parent| padding_left(styles, parent, width).to_bits()),
            ),
            Query::StickyParent => hashed(
                self.ancestors
                    .last()
                    .map(|&parent| (parent, insets(styles, parent, true, width).map(f32::to_bits))),
            ),
            Query::Break => hashed(starts_line(doc, id)),
            Query::UserSelect => hashed(styles.user_select[id].0 as u8),
            Query::Positioner(fixed) => hashed(positioner(
                if fixed { "fixed" } else { "absolute" },
                self.ancestors.iter().rev().copied(),
                |ancestor| super::effects::establishes(&styles.values[ancestor]),
            )),
            Query::Backdrop => hashed(backdrop(styles, &self.ancestors, id).map(|(_, ink)| ink)),
            Query::Scroller => {
                let Some(handles) = dom.map(|dom| &dom.scroll) else {
                    return Replay::Changed;
                };
                // WARNING: a box above the sticky one may be `<html>` or `<body>`, which scroll only with a handle.
                let ancestors = self.ancestors.iter().rev().copied();
                let scroller = if below {
                    scroller(doc, styles, std::iter::once(id).chain(ancestors), handles)
                } else {
                    scroller(doc, styles, ancestors, handles)
                };
                hashed((
                    scroller,
                    insets(styles, scroller, false, width).map(f32::to_bits),
                    handles.contains_key(&scroller),
                ))
            }
            Query::Editable => hashed(content_editable(doc, id).is_some()),
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
                // WARNING: prepaint draws again only for a size more than half a pixel off the one the tracks used.
                let used = f32::from_bits(read.value as u32);
                return match dom
                    .and_then(|dom| dom.cull.widths.get(&(node as usize)))
                    .map(|size| size[usize::from(axis)])
                {
                    Some(size) if size.to_bits() == used.to_bits() => Replay::Same,
                    Some(size) if (size - used).abs() <= 0.5 => Replay::Close,
                    _ => Replay::Changed,
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
            Query::Value(node) => hashed(&doc.nodes[node as usize].value),
            Query::Viewport => hashed(styles.lengths[id].viewport.map(f32::to_bits)),
            Query::Theme => hashed(crate::desktop::native::theme_state() & 1),
        };
        if now == read.value {
            Replay::Same
        } else {
            Replay::Changed
        }
    }
}

// WARNING: an absolute box without such an ancestor takes the document, a fixed one the viewport (`None`).
pub(super) fn positioner(
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

// WARNING: a sticky box sticks in its nearest scroll container by style, not in any box with a handle, since
// scripts make handles for any box. `<html>` and `<body>` scroll the page unless they have their own handle.
pub(super) fn scroller(
    doc: &Document,
    styles: &ComputedStyles,
    mut ancestors: impl Iterator<Item = usize>,
    handles: &rustc_hash::FxHashMap<usize, renderer::ScrollHandle>,
) -> usize {
    ancestors
        .find(|&ancestor| {
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

type Siblings = (Vec<usize>, rustc_hash::FxHashMap<usize, usize>);

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
        // WARNING: resvg doesn't take CSS transforms, `transform-box` or `transform-origin`, so they become a matrix.
        if transform
            && element != root
            && [Prop::TRANSFORM, Prop::TRANSLATE, Prop::ROTATE, Prop::SCALE]
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
            let bounds = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
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
        // WARNING: `vector-effect: non-scaling-stroke` draws strokes in screen pixels, which resvg doesn't do, so their
        // widths and dashes scale back by the viewBox scale. resvg ignores `pathLength`, so dashes scale by the
        // real length over it.
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
            let Some(value) = own.get(prop) else { continue };
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
                // WARNING: resvg resolves `currentColor` with the `color` the element inherits in the source, which for
                // `<use>` content is the `<use>`'s color, not the color where the content is defined.
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
            // WARNING: resvg drops shapes without paint.
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
                && let Some(target) = node
                    .attrs
                    .get("href")
                    .or_else(|| node.attrs.get("xlink:href"))
                    .and_then(|value| value.strip_prefix('#'))
                    .and_then(|name| {
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
pub(super) type Svg = (
    Vec<Rc<style::Style>>,
    ([f32; 2], u64),
    String,
    crate::dom::SvgBoxes,
);
pub(super) type Margins = (
    rustc_hash::FxHashMap<usize, Rc<rustc_hash::FxHashMap<usize, [f32; 2]>>>,
    rustc_hash::FxHashMap<usize, ([f32; 2], [f32; 2], bool)>,
);

/// The geometry revision and width an `adjoining` result was made with.
pub(super) type Adjoined = ((u64, u32), ([f32; 2], [f32; 2], bool));

pub(super) type Walked = (
    (u64, u32, [bool; 2]),
    Rc<rustc_hash::FxHashMap<usize, [f32; 2]>>,
);

thread_local! {
    // WARNING: the lowest stack address that building boxes may reach: a quarter of the stack stays
    // for layout and paint, which recurse as deep.
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
        // WARNING: Windows builds link a 64 MB main stack, see build.rs.
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
    // WARNING: a 3D parent resolves the box's transform with its own units, so such boxes build again. The others
    // build from what no read records: animated values or their transforms.
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
        cell: slot.as_ref().map(|(bounds, grid)| {
            use std::hash::{Hash, Hasher};
            let mut tracks = rustc_hash::FxHasher::default();
            for value in grid.columns.iter().chain(&grid.rows).chain(&grid.gap) {
                value.to_bits().hash(&mut tracks);
            }
            (
                [bounds[2], bounds[3]],
                grid.baseline,
                grid.first_item.is_some(),
                tracks.finish(),
            )
        }),
    };
    // WARNING: `Some` when the box's record still holds, with `true` when a read held only within prepaint's
    // tolerance. The record goes into the log, so the boxes around a kept box keep its reads too.
    let keep = 'keep: {
        if !storable
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
        let slotted = slot.is_some();
        // WARNING: a kept subtree culls only what its build culled. One that culled keeps its spacers, which build
        // it again when they come near the visible range. Others are kept only while no box below them that a parent
        // could cull lies wholly outside the culling margin, also inside scroll boxes and in subtrees built without
        // culling.
        if !record.culled
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
        let held = record.call == call && {
            let fresh = record.epoch.get() == build.epoch;
            let host = build.view.host.borrow();
            let dom = host.dom.get(&build.view.id);
            record.reads.iter().all(|read| {
                fresh && !read.query.host()
                    || match build.replay(id, containing, container, slotted, dom, read) {
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
                            |read| build.replay(id, containing, container, slotted, dom, read)
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
        if record.culled {
            build.culls.set(build.culls.get() + 1);
        }
        let depth = build.ancestors.len();
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
                ..*read
            }));
        #[cfg(debug_assertions)]
        build.prints.borrow_mut().push(record.print);
        Some(close)
    };
    if let Some(_close) = keep {
        #[cfg(debug_assertions)]
        if !_close {
            check(build, id, containing, container, slot, mark, printed);
        }
        return Child::Kept(id.into());
    }
    let culls = build.culls.get();
    let patch = text_field(&build.doc.nodes[id]).then(|| Patch {
        ancestors: build.ancestors.clone(),
        containing,
        container,
        slot: slot.clone(),
        flex_parent: build.flex_parent,
        perspective: build.perspective.clone(),
        value: hashed(&build.doc.nodes[id].value),
    });
    #[cfg_attr(not(debug_assertions), allow(unused_mut))]
    let mut child = create(build, id, containing, container, slot);
    #[cfg(debug_assertions)]
    let plan = build.plan.take();
    #[cfg(debug_assertions)]
    if !build.checking
        && *TRACE
        && let Some(name) = build.doc.nodes[id].attrs.get("id")
    {
        eprintln!("BUILT {name}");
    }
    // WARNING: reads that the box's subtree revision covers leave the log, the others stay for the boxes around it. A
    // read that boxes below repeat stays once, as it gave them all the same. Repeats come from neighbors, so a long
    // record only looks back a few reads for them.
    let reads = {
        let depth = build.ancestors.len();
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
        storable.then(|| {
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
        })
    };
    #[cfg(debug_assertions)]
    let print = print(build, printed, &mut child);
    #[cfg(debug_assertions)]
    let checking = build.checking;
    #[cfg(not(debug_assertions))]
    let checking = false;
    if let Some(patch) = patch.filter(|_| !checking) {
        build.view.patches.borrow_mut().insert(id, patch);
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
                        epoch: build.epoch.into(),
                        culled: build.culls.get() != culls,
                        #[cfg(debug_assertions)]
                        print,
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

// WARNING: with `APPS_TRACE_BUILDS` set, debug builds print the `id` attribute of each box they build, so tests see
// which boxes were kept.
#[cfg(debug_assertions)]
static TRACE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var_os("APPS_TRACE_BUILDS").is_some());

// WARNING: debug builds build each kept box again, culling only the children its build culled, and it must read and
// build the same, or its build reads something that no record holds.
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
        doc: build.doc,
        styles: build.styles,
        text: build.text,
        pixel_ratio: build.pixel_ratio,
        view: build.view,
        ancestors: build.ancestors.clone(),
        flex_parent: build.flex_parent,
        measured: build.measured,
        perspective: build.perspective.clone(),
        siblings: build.siblings,
        margins: build.margins,
        blocks: build.blocks,
        ids: build.ids,
        info: build.info,
        zone: None,
        top_layer: build.top_layer,
        tree: build.tree,
        culls: &culls,
        resized: build.resized,
        settled: build.settled,
        reads: build.reads,
        epoch: build.epoch,
        prints: build.prints,
        checking: true,
        extra: Default::default(),
        plan: Default::default(),
    };
    let start = build.reads.borrow().len();
    let mut fresh = create(&shadow, id, containing, container, slot);
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
    let print = print(&shadow, printed + 1, &mut fresh);
    let mut prints = build.prints.borrow_mut();
    assert_eq!(
        prints[printed], print,
        "kept box {id} <{tag}> builds otherwise when built again"
    );
    prints.truncate(printed + 1);
}

// WARNING: a debug fingerprint of a box: its own style, what its hooks hold, and the fingerprints of the boxes built
// under it since `from`.
#[cfg(debug_assertions)]
fn print(build: &Build<'_>, from: usize, child: &mut Child) -> u64 {
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
        let _ = std::fmt::Write::write_fmt(&mut hasher, format_args!("{:?}", node.style()));
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

#[cfg(debug_assertions)]
impl std::fmt::Debug for Query {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Query::Margins => write!(f, "Margins"),
            Query::Parent(prop) => write!(f, "Parent({})", prop.name()),
            Query::ParentFont => write!(f, "ParentFont"),
            Query::FlatParent => write!(f, "FlatParent"),
            Query::FlatParentPadding => write!(f, "FlatParentPadding"),
            Query::StickyParent => write!(f, "StickyParent"),
            Query::Break => write!(f, "Break"),
            Query::UserSelect => write!(f, "UserSelect"),
            Query::Positioner(fixed) => write!(f, "Positioner({fixed})"),
            Query::Backdrop => write!(f, "Backdrop"),
            Query::Scroller => write!(f, "Scroller"),
            Query::Editable => write!(f, "Editable"),
            Query::Shrunk => write!(f, "Shrunk"),
            Query::Uses(node) => write!(f, "Uses({node})"),
            Query::Block(node) => write!(f, "Block({node})"),
            Query::Scroll(node) => write!(f, "Scroll({node})"),
            Query::Width(node) => write!(f, "Width({node})"),
            Query::Track(node, axis) => write!(f, "Track({node}, {axis})"),
            Query::Baselines(node) => write!(f, "Baselines({node})"),
            Query::Atom(node) => write!(f, "Atom({node})"),
            Query::Image(node) => write!(f, "Image({node})"),
            Query::Look(node) => write!(f, "Look({node})"),
            Query::Boxless(node) => write!(f, "Boxless({node})"),
            Query::Value(node) => write!(f, "Value({node})"),
            Query::Viewport => write!(f, "Viewport"),
            Query::Theme => write!(f, "Theme"),
        }
    }
}

fn create(
    build: &Build<'_>,
    id: usize,
    containing: [f32; 2],
    container: [f32; 2],
    slot: Option<([f32; 4], super::grid::Grid)>,
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
        margins: margin_cache,
        ids,
        info,
        ref zone,
        top_layer,
        tree,
        culls,
        resized,
        settled,
        reads,
        epoch,
        ..
    } = *build;
    let depth = ancestors.len();
    let node = &doc.nodes[id];
    let symbol = node.sf_symbol();
    let style = &styles.values[id];
    let display =
        style
            .get(Prop::DISPLAY)
            .unwrap_or(if node.tag.starts_with('#') && node.tag != "#text" {
                "block"
            } else {
                "inline"
            });
    // WARNING: like Chrome, a failed image with alt text is a box of a broken image icon and the text, which is an
    // inline block that ignores its size when the image is inline, as Chrome's UA sheet makes it.
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
    let order = style
        .get(Prop::Z_INDEX)
        .and_then(|value| value.parse::<i32>().ok());
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
    // WARNING: a box whose opacity animates keeps one stacking context and effect layer at every
    // value, so animation frames only patch the layer's opacity.
    let composited = info[id].flags & cull::COMPOSITED != 0;
    let made = super::effects::establishes(style);
    // WARNING: the renderer paints a z-indexed box in its nearest stacking ancestor, so every box that paints
    // through a layer must be one, or such boxes escape its transform, opacity or clip.
    let stacking = id == 0 || composited || made.stacking || layered;
    let chain: Rc<[usize]> = ancestors.iter().copied().chain([id]).collect();
    let child_build = Build {
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        measured,
        siblings,
        margins: margin_cache,
        blocks,
        ids,
        info,
        top_layer,
        tree,
        culls,
        resized,
        settled,
        reads,
        epoch,
        #[cfg(debug_assertions)]
        prints: build.prints,
        #[cfg(debug_assertions)]
        checking: build.checking,
        #[cfg(debug_assertions)]
        extra: Default::default(),
        #[cfg(debug_assertions)]
        plan: Default::default(),
        zone: zone
            .as_ref()
            .filter(|_| info[id].flags & (cull::CHANGED | cull::MOVES_CONTENT) == 0)
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
                // WARNING: zones hold last frame's places, and the children moved by the box's scroll since.
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
    };
    let build = &child_build;
    let mut units = styles.lengths[id];
    units.container = Some(container);
    let position = style.get(Prop::POSITION).unwrap_or("static");
    let parent = box_parent(doc, styles, id);
    let positioner = positioner(position, ancestors.iter().rev().copied(), |ancestor| {
        super::effects::establishes(&styles.values[ancestor])
    });
    if matches!(position, "absolute" | "fixed") {
        let scope = positioner
            .and_then(|block| ancestors.iter().rposition(|&ancestor| ancestor == block))
            .map_or(1, |index| index + 1);
        reader.log(Query::Positioner(position == "fixed"), scope, hashed(positioner));
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
    let viewport = || {
        reader.log(Query::Viewport, 0, hashed(units.viewport.map(f32::to_bits)));
        Some(units.viewport)
    };
    let block_size = if position == "fixed" && positioner.is_none() {
        viewport()
    } else {
        positioner
            .filter(|&block| Some(block) != parent)
            .and_then(|block| {
                if block == 0 {
                    return viewport();
                }
                // WARNING: the block's last layout comes before the estimate of its build, which can miss its
                // min and max sizes. Prepaint draws again when they differ, so an estimate first would draw
                // again in every frame.
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
    // WARNING: element building and layout recurse per level, so deep pages must not overflow the stack.
    if chain.len() > 256 || style.get(Prop::DISPLAY).is_some_and(|s| s == "none") {
        return renderer::node().hidden().into();
    }
    let opacity = super::effects::opacity(style);
    // WARNING: a box at opacity 0 draws nothing, so it takes the element opacity, which skips its primitives, and
    // not a layer: hidden truncation copies in a file tree cost two render passes each. It still lays out and takes
    // events.
    let effect_layer = (made.layer || opacity > 0. && opacity < 1. && !node.children.is_empty() || composited)
        .then_some(opacity);
    let selection_colors = style::selection_colors(style, styles.selections.get(&id));
    let face = style::font(style);
    let line_height = match style::line_height(style) {
        style::LineHeight::Fixed(_) => (units.font.lh * pixel_ratio * 64.).round(),
        _ => (units.font.lh * pixel_ratio * 64.).floor(),
    } / 64.
        / pixel_ratio;
    let above = || {
        f32::from(text.baseline(
            text.resolve_font(&style::font(style)),
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
        renderer::Child::Node(node) => node.style().align_self,
        renderer::Child::Kept(id) => build.tree.align_self(*id),
    };
    // WARNING: taffy moves an item that doesn't fit its line to a new line when any item is before it, even the
    // zero-width strut, so the strut and the first item are one item, aligned like the item. Like browsers, a first
    // item wider than its line then stays on the first line. A group that is not on the baseline takes the strut
    // out of the line's baseline, so a second strut goes before the next item: true is returned for it.
    let with_strut = |mut element: renderer::Child| {
        let align = align_of(&mut element);
        let mut group = renderer::node()
            .flex()
            .flex_row()
            .flex_shrink(1.)
            .child(strut())
            .child(element);
        group.style().align_items = Some(renderer::AlignItems::Baseline);
        group.style().align_self = align;
        group.style().inline = renderer::Inline::Lines;
        (group, align.is_some())
    };
    let mut strut_due = false;
    let mut strut_next = false;
    let mut el = renderer::node()
        .id(id)
        .flex()
        .flex_col()
        .text_size(px(units.font.em))
        .font_family(face.family)
        .font_weight(face.weight)
        .line_height(px(line_height));
    el.style().flex_shrink = if flex_parent { 1. } else { 0. };
    el.style().baseline = match display {
        "inline-block" if !flex_parent && slot.is_none() => renderer::Baseline::Last,
        "flex" | "inline-flex" | "grid" | "inline-grid" => renderer::Baseline::First,
        "table" | "inline-table" => renderer::Baseline::FirstOnly,
        _ => renderer::Baseline::Auto,
    };
    {
        let text = el.text_style();
        text.letter_spacing = Some(style::spacing(style, &units, Prop::LETTER_SPACING));
        text.word_spacing = Some(style::spacing(style, &units, Prop::WORD_SPACING));
        text.font_smoothing = Some(
            ![Prop::WEBKIT_FONT_SMOOTHING, Prop::MOZ_OSX_FONT_SMOOTHING]
                .into_iter()
                .any(|name| {
                    style
                        .get(name)
                        .is_some_and(|v| v == "antialiased" || v == "grayscale")
                }),
        );
        text.text_indent = Some(px(style
            .get(Prop::TEXT_INDENT)
            .and_then(|value| units.resolve(value, containing[0]))
            .unwrap_or(0.)));
    }
    if face.style != renderer::FontStyle::Normal {
        el.text_style().font_style = Some(face.style);
    }
    if node.tag == "br" {
        let empty = starts_line(doc, id);
        reader.log(Query::Break, depth, hashed(empty));
        let painter = view.clone();
        let ink = [Prop::CARET_COLOR, Prop::COLOR]
            .into_iter()
            .find_map(|prop| style::color(style.get(prop)?))
            .map_or(0x000000ff, |(color, alpha)| color << 8 | alpha as u32);
        let view = view.clone();
        let above = above();
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
                            f32::from(bounds.top()),
                            0.,
                            line_height,
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
                    super::input::paint_caret(
                        &painter,
                        [id, 0, 0],
                        Bounds::new(bounds.origin, size(px(1.), px(line_height))),
                        rgba(ink),
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
    if stacking {
        el = el.stacking().child(renderer::layers(true));
    }
    let flow = !matches!(display, "flex" | "inline-flex" | "grid" | "inline-grid");
    let mut flat = Vec::new();
    let mut pending: Vec<usize> =
        if symbol.is_some() || matches!(node.tag.as_str(), "svg" | "canvas" | "iframe") {
            Vec::new()
        } else {
            node.children.iter().rev().copied().collect()
        };
    while let Some(child) = pending.pop() {
        if styles.values[child]
            .get(Prop::DISPLAY)
            .is_some_and(|value| value == "contents")
        {
            pending.extend(doc.nodes[child].children.iter().rev());
        } else if !matches!(doc.nodes[child].tag.as_str(), "#comment" | "#doctype")
            && !(doc.nodes[child].tag == "br"
                && styles.values[child].get(Prop::DISPLAY) == Some("none"))
        {
            flat.push(child);
        }
    }
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
    let breaks = display != "inline"
        && flat.iter().any(|&child| doc.nodes[child].tag == "br")
        && measured
            .borrow_mut()
            .texts
            .entry(id)
            .or_insert_with(|| style::inline_text(doc, styles, id).map(Rc::new))
            .is_none();
    let all_inline = flow
        && !flat.is_empty()
        && !breaks
        && flat.iter().all(|&child| {
            inline_level(child) || styles.values[child].get(Prop::DISPLAY) == Some("none")
        });
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    let line_justify = match (style.get(Prop::TEXT_ALIGN), rtl) {
        (Some("right" | "-webkit-right"), _)
        | (Some("end"), false)
        | (None | Some("start"), true) => Some(renderer::JustifyContent::End),
        (Some("center" | "-webkit-center"), _) => Some(renderer::JustifyContent::Center),
        _ => None,
    };
    let line_align = Some(renderer::AlignItems::Baseline);
    if all_inline || display == "inline" {
        el = el.flex_row().flex_wrap();
        el.style().justify_content = line_justify;
        el.style().align_items = line_align;
        el.style().inline = renderer::Inline::Lines;
        let merged = !text_field(node)
            && symbol.is_none()
            && !matches!(node.tag.as_str(), "svg" | "canvas" | "iframe")
            && measured
                .borrow_mut()
                .texts
                .entry(id)
                .or_insert_with(|| style::inline_text(doc, styles, id).map(Rc::new))
                .is_some();
        if display != "inline"
            && !merged
            && flat.iter().any(|&child| {
                !doc.nodes[child].tag.starts_with('#')
                    && doc.nodes[child].tag != "br"
                    && styles.values[child].get(Prop::DISPLAY) != Some("inline")
                    && in_flow(doc, info, child)
                    && !style::makes_no_line(doc, styles, child)
            })
        {
            strut_due = true;
        }
    }
    // WARNING: like browsers, a canvas takes its natural size and ratio from its attributes, whatever it shows.
    // An auto height comes from the in-flow picture, which keeps that ratio in the content box, also in a flex
    // row, where taffy drops the box's own ratio for a percentage width. The drawing covers that picture, so it
    // stays in the content box too. Pictures paint from the node, so a drawing or a frame changes no box.
    let (mut natural, mut natural_ratio) = match node.tag.as_str() {
        "img" | "video" => (None, None),
        _ => natural_size(doc, id, None).map_or((None, None), |(size, ratio)| (Some(size), ratio)),
    };
    if node.tag == "canvas" {
        let picture = |source| {
            renderer::node().size_full().hooks(super::paint::Picture {
                id,
                view: view.clone(),
                source,
                fit: renderer::ObjectFit::Fill,
                cropped: None,
            })
        };
        let mut presented = picture(super::paint::Source::Presented)
            .child(picture(super::paint::Source::Drawn).absolute());
        presented.style().aspect_ratio = natural_ratio;
        el = el.child(presented);
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
                    [u32::from(size.width) as f32, u32::from(size.height) as f32]
                })
                .or(vector.as_ref().map(|(_, natural)| *natural)),
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
                el = el.child(svg_layer(id, false, view.clone()));
            }
        }
        if image.is_none() && natural_ratio.is_some() {
            let mut sized = renderer::node().size_full();
            sized.style().aspect_ratio = natural_ratio;
            el = el.child(sized);
        } else if natural.is_none() && !sizeless {
            // WARNING: images that are loading or failed to decode keep only their attribute size.
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
        // WARNING: like Chrome, a failed image with both sizes and no alt text has a silver frame, and its icon
        // shows only when 18 pixels fit.
        let framed = alt.is_none()
            && natural.is_some_and(|size| size != [0., 0.])
            && [("width", Prop::WIDTH), ("height", Prop::HEIGHT)]
                .iter()
                .all(|&(name, prop)| {
                    style.get(prop).is_some_and(|value| value != "auto")
                        || node.attrs.contains_key(name)
                });
        // WARNING: the icon is out of flow, so the text gives the box its baseline. Like Chrome's floated icon, it
        // moves only the first line, or the text goes below it when the first word doesn't fit beside it.
        el = if let Some(alt) = alt {
            let mut text = renderer::text(
                alt.clone(),
                Some(vec![style::text_run(style, &units, alt.len())]),
            );
            text.text_style().float = Some(px(16.));
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
                        window.paint_quad(renderer::quad(
                            bounds,
                            px(0.),
                            renderer::transparent_black(),
                            px(1.),
                            rgba(0xc0c0c0ff),
                            renderer::BorderStyle::Solid,
                        ));
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
        let [width, height] = [(Prop::WIDTH, "width"), (Prop::HEIGHT, "height")]
            .into_iter()
            .enumerate()
            .map(|(axis, (prop, attr))| match style.get(prop) {
                Some(value) => units.resolve(value, containing[axis]),
                None => node.attrs.get(attr).and_then(|v| dimension(v, false)),
            })
            .collect::<Vec<_>>()[..]
        else {
            unreachable!()
        };
        let ratio = node.attrs.get("viewBox").and_then(|value| {
            let parts: Vec<f32> = value
                .split(|c: char| c == ',' || c.is_ascii_whitespace())
                .filter_map(|part| part.parse().ok())
                .collect();
            (parts.len() == 4 && parts[2] > 0. && parts[3] > 0.).then(|| parts[2] / parts[3])
        });
        let [width, height] = match (width, height, ratio) {
            (None, Some(height), Some(ratio)) => [height * ratio, height],
            (Some(width), None, Some(ratio)) => [width, width / ratio],
            _ => [width.unwrap_or(24.), height.unwrap_or(24.)],
        }
        .map(|value| value.clamp(1., 4096.));
        // WARNING: a source embeds the SVG's subtree with the SVG properties of each element, so it is built
        // again when a node, the size or one of those values changes. The `<svg>` itself passes only inherited
        // values, so its own animated transform or opacity keeps the source.
        let mut scope = id;
        while let Some(parent) = doc.nodes[scope].parent {
            scope = parent;
        }
        let mut pending = vec![id];
        let mut visited = std::collections::HashSet::new();
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
            if child.tag == "use"
                && let Some(reference) = child
                    .attrs
                    .get("href")
                    .or_else(|| child.attrs.get("xlink:href"))
                    .and_then(|value| value.strip_prefix('#'))
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
        if let Some(value) = uses(doc, styles, ids, id) {
            reader.log(Query::Uses(id as u32), 1, value);
        }
        let key = ([width, height], std::hash::Hasher::finish(&hasher));
        let props = || style::SVG.iter().chain(&SVG_SOURCE);
        let kept = view
            .svgs
            .borrow()
            .get(&id)
            .and_then(|(old, at, source, boxes)| {
                (*at == key
                    && old.len() == painted.len()
                    && old
                        .iter()
                        .zip(&painted)
                        .enumerate()
                        .all(|(index, (old, new))| {
                            Rc::ptr_eq(old, new)
                                || props().all(|&prop| {
                                    index == 0 && !prop.inherited()
                                        || old.get(prop) == new.get(prop)
                                })
                        }))
                .then(|| (source.clone(), boxes.clone()))
            });
        let built = kept.is_none();
        let (source, boxes) = kept.unwrap_or_else(|| {
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
            let mut header = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\""
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
                scale: view_box
                    .as_ref()
                    .map_or(1., |parts| (width / parts[2]).min(height / parts[3])),
                view: view_box
                    .as_ref()
                    .map_or([width, height], |parts| [parts[2], parts[3]]),
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
            // WARNING: marks count from 0 in the source, so a picture has one source on all nodes and shares its
            // raster. `names` starts with their nodes.
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
            // WARNING: CSS keywords ignore case, but resvg only reads `currentColor`.
            let lower = source.to_ascii_lowercase();
            let mut end = 0;
            let mut replaced = String::with_capacity(source.len());
            for (start, _) in lower.match_indices("currentcolor") {
                replaced.push_str(&source[end..start]);
                replaced.push_str("currentColor");
                end = start + "currentcolor".len();
            }
            replaced.push_str(&source[end..]);
            let boxes = (String::new(), names, [0.; 2], Vec::new(), [0.; 4]);
            view.svgs
                .borrow_mut()
                .insert(id, (painted, key, replaced.clone(), boxes.clone()));
            (replaced, boxes)
        });
        let mut host = view.host.borrow_mut();
        let state = host.dom.entry(view.id).or_default();
        if built || !state.svg_sources.contains_key(&id) {
            // WARNING: paint only checks the raster size of inline SVGs, so a changed source drops the raster here.
            if state.svgs.get(&id).is_some_and(|(old, _)| *old != source)
                && let Some((_, image)) = state.svgs.remove(&id)
                && Arc::strong_count(&image) == 1
            {
                state.retired.push(image);
            }
            state.svg_boxes.insert(id, boxes);
            state.svg_sources.insert(id, source);
        }
        drop(host);
        el = el
            .w(px(width))
            .h(px(height))
            .child(svg_layer(id, true, view.clone()));
    }
    // WARNING: a flex line stretches an item with an auto size across it, also a replaced element or a control, so
    // their natural size gives way in that axis.
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
    let appkit = node.native_control();
    if appkit.is_some() {
        super::native::register(&view.controls, id);
    }
    let check = input.is_some_and(|kind| {
        kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio")
    });
    let editable = text_field(node);
    // WARNING: controls draw from the system theme when their `color-scheme` allows both, which no style holds.
    let dark = || {
        if style
            .get(Prop::COLOR_SCHEME)
            .is_some_and(|value| value.contains("dark") && value.contains("light"))
        {
            reader.log(
                Query::Theme,
                0,
                hashed(crate::desktop::native::theme_state() & 1),
            );
        }
        style::dark(style)
    };
    if input.is_some_and(|kind| kind.eq_ignore_ascii_case("file")) {
        let [fill, edge] = if dark() {
            [0x6b6b6b, 0x858585]
        } else {
            [0xefefef, 0x767676]
        };
        let mut button = renderer::node()
            .flex_shrink_0()
            .mr(px(4.))
            .pl(px(7.))
            .pr(px(7.))
            .pt(px(2.))
            .pb(px(2.))
            .border(px(1.))
            .border_color(rgba(edge << 8 | 0xff))
            .bg(rgba(fill << 8 | 0xff))
            .child("Choose File");
        button.style().corner_radii = renderer::Corners::all(px(2.).into()).into();
        el = el
            .flex_row()
            .items_center()
            .overflow_hidden()
            .child(button)
            .child(renderer::node().whitespace_nowrap().child("No file chosen"));
        if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
            let face = text.resolve_font(&style::font(style));
            let em = px(units.font.em);
            let width = |label: &str| {
                let run = renderer::TextRun {
                    len: label.len(),
                    font: style::font(style),
                    color: renderer::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                    letter_spacing: px(0.),
                    word_spacing: px(0.),
                    font_size: None,
                    inset: Default::default(),
                    nowrap: false,
                };
                f32::from(text.shape_line(label, em, &[run], None).width)
            };
            let zero = text
                .ch_advance(face, em)
                .map_or(units.font.em / 2., f32::from);
            let content = width("Choose File") + 16. + 4. + width("No file chosen");
            el = el.w(px(layout_ceil((zero * 34.).max(content), pixel_ratio)));
        }
    }
    // WARNING: an AppKit control covers the box, so the page draws none of it. A button is as wide as its title.
    if appkit == Some(renderer::ControlKind::Button) {
        let [padding, height] = super::native::size(renderer::ControlKind::Button);
        if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
            let label = node.attrs.get("value").map_or("", String::as_str);
            let run = style::text_run(style, &units, label.len());
            let width = text
                .shape_line(label, px(units.font.em), &[run], None)
                .width;
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
    if editable && node.tag != "textarea" {
        let above = f32::from(text.baseline(
            text.resolve_font(&style::font(style)),
            px(units.font.em),
            px(units.font.lh),
            pixel_ratio,
        ));
        el.style().justify_content = Some(renderer::JustifyContent::Center);
        el = el.child(
            renderer::node()
                .w(px(0.))
                .h(px(above))
                .mb(px(units.font.lh - above))
                .flex_shrink_0()
                .baseline(renderer::Baseline::Bottom),
        );
    }
    if editable {
        // WARNING: a field sized by its content lays out from its value, which no revision covers.
        if style.get(Prop::FIELD_SIZING) == Some("content") {
            reader.log(Query::Value(id as u32), 0, hashed(&node.value));
        }
        let text = node.shown_text(
            node.current_value()
                .map_or_else(|| doc.text(id), str::to_owned),
        );
        let placeholder = node.attrs.get("placeholder").cloned().unwrap_or_default();
        let face = style::font(style);
        let font_size = units.font.em;
        let line_height = units.font.lh;
        let text_color = style
            .get(Prop::COLOR)
            .and_then(style::color)
            .map(|(c, a)| rgba((c << 8) | a as u32))
            .unwrap_or(renderer::black());
        let caret_color = style
            .get(Prop::CARET_COLOR)
            .and_then(style::color)
            .map(|(color, alpha)| rgba((color << 8) | alpha as u32))
            .unwrap_or(text_color);
        let placeholder_color = styles
            .text_pseudos
            .get(&id)
            .and_then(|pseudos| style::color(pseudos[3].as_ref()?.get(Prop::COLOR)?))
            .map(|(color, alpha)| rgba((color << 8) | alpha as u32))
            .unwrap_or(rgba(0x757575ff));
        let single_line = node.tag != "textarea";
        let padding = units.edges(style, style::PADDING, containing[0]);
        let empty = text.is_empty();
        let shown: Rc<str> = if empty { placeholder } else { text }.into();
        let run = renderer::TextRun {
            len: shown.len(),
            font: face,
            color: if empty { placeholder_color } else { text_color },
            background_color: None,
            underline: None,
            strikethrough: None,
            letter_spacing: style::spacing(style, &units, Prop::LETTER_SPACING),
            word_spacing: px(0.),
            font_size: None,
            inset: Default::default(),
            nowrap: false,
        };
        if !single_line && style.get(Prop::FIELD_SIZING) == Some("content") {
            let mut sizer = renderer::text(shown.clone(), Some(vec![run.clone()]))
                .min_w(px(0.))
                .min_h(px(line_height))
                .flex_shrink_0();
            sizer.style().visibility = renderer::Visibility::Hidden;
            el = el.child(sizer);
        }
        // WARNING: the box that clips the text scrolls with the field's handle, so the field itself does not track it.
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
            caret: caret_color,
            selection: selection_colors,
            handle: handle.clone(),
        };
        let mut clip = renderer::node()
            .absolute()
            .track_scroll(&handle)
            .text_size(px(font_size))
            .line_height(px(line_height));
        clip.style().restrict_scroll_to_axis = true;
        clip.style().overscroll = overscroll(style);
        el = el
            .cursor(renderer::CursorStyle::IBeam)
            .child(if single_line {
                let line = renderer::node().flex_shrink_0().child(field);
                clip.top(px(padding[0]))
                    .right(px(padding[1]))
                    .bottom(px(padding[2]))
                    .left(px(padding[3]))
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .items_center()
                    .whitespace_nowrap()
                    .child(match line_justify {
                        Some(renderer::JustifyContent::End) => line.ml_auto(),
                        Some(renderer::JustifyContent::Center) => line.ml_auto().mr_auto(),
                        _ => line,
                    })
            } else {
                clip.style().overflow.x = renderer::Overflow::Scroll;
                clip.top_0()
                    .right(px(0.))
                    .bottom(px(0.))
                    .left_0()
                    .pt(px(padding[0]))
                    .pr(px(padding[1]))
                    .pb(px(padding[2]))
                    .pl(px(padding[3]))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(field)
            });
    }
    if let Some((ink, alpha)) = style.get(Prop::COLOR).and_then(style::color) {
        el = el.text_color(rgba((ink << 8) | alpha as u32));
    }
    let background = |style: &style::Style| {
        style
            .get(Prop::BACKGROUND_COLOR)
            .and_then(style::color)
            .filter(|(_, alpha)| *alpha > 0)
    };
    let inline_box = display == "inline"
        && symbol.is_none()
        && !node.tag.starts_with('#')
        && !flex_parent
        && slot.is_none()
        && !matches!(
            node.tag.as_str(),
            "img" | "svg" | "canvas" | "input" | "button" | "select" | "textarea" | "iframe"
        )
        && !style
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "absolute" | "fixed"));
    let mut inline_fill = renderer::transparent_black();
    let mut cell_background = background(style);
    if cell_background.is_none() && matches!(node.tag.as_str(), "td" | "th") {
        let mut parent = node.parent;
        while let Some(row) = parent.filter(|&row| {
            matches!(
                doc.nodes[row].tag.as_str(),
                "tr" | "thead" | "tbody" | "tfoot"
            )
        }) {
            cell_background = background(&styles.values[row]);
            if cell_background.is_some() {
                break;
            }
            parent = doc.nodes[row].parent;
        }
    }
    if let Some(color) = cell_background {
        let color = rgba((color.0 << 8) | color.1 as u32);
        let blended = style
            .get(Prop::BACKGROUND_BLEND_MODE)
            .is_some_and(|value| value.split(',').any(|mode| mode.trim() != "normal"))
            && style
                .get(Prop::BACKGROUND_IMAGE)
                .is_some_and(|value| value != "none");
        if blended {
        } else if inline_box {
            inline_fill = color;
        } else {
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
    let page = matches!(node.tag.as_str(), "html" | "body");
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
    // WARNING: a form field's text clips and scrolls in its own box, so the field keeps its outline unclipped.
    if overflow != [renderer::Overflow::Visible; 2] && !editable {
        el.style().overflow.x = overflow[0];
        el.style().overflow.y = overflow[1];
    }
    let scrolls = overflow.map(|axis| axis == renderer::Overflow::Scroll);
    let look = if scrolls.contains(&true) {
        let backdrop = backdrop(styles, ancestors, id);
        reader.log(
            Query::Backdrop,
            backdrop.map_or(1, |(up, _)| depth + 1 - up),
            hashed(backdrop.map(|(_, ink)| ink)),
        );
        reader.log(
            Query::Theme,
            0,
            hashed(crate::desktop::native::theme_state() & 1),
        );
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
        let host = view.host.borrow();
        let handle = host.dom.get(&view.id).and_then(|dom| dom.scroll.get(&id));
        reader.log(Query::Scroll(id as u32), 0, hashed(scrolled(handle)));
        let max = handle
            .map(renderer::ScrollHandle::max_offset)
            .unwrap_or_default();
        look.space(look.shown(scrolls, max))
    });
    let mut padding = units
        .edges(style, style::PADDING, containing[0])
        .map(|value| value.max(0.));
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
    el = el
        .pt(px(padding[0]))
        .pr(px(padding[1]))
        .pb(px(padding[2]))
        .pl(px(padding[3]));
    let mut margin = margins(
        build,
        id,
        &units,
        containing[0],
        flex_parent,
        slot.is_some(),
    );
    reader.log(Query::Margins, depth, hashed(margin.map(f32::to_bits)));
    el = el
        .mt(px(margin[0]))
        .mr(px(margin[1]))
        .mb(px(margin[2]))
        .ml(px(margin[3]));
    if let Some(fieldset) = node.parent
        && let Some((legend, height, _)) = legend(doc, styles, fieldset)
        && legend == id
    {
        let left =
            styles.lengths[fieldset].edges(&styles.values[fieldset], style::PADDING, containing[0])
                [3];
        el = el.absolute().top(px(-height)).left(px(left));
    }
    for (side, prop) in style::MARGIN[1..].iter().enumerate() {
        if style.get(*prop).is_some_and(|s| s == "auto") {
            el = match side {
                0 => el.mt_auto(),
                1 => el.mr_auto(),
                2 => el.mb_auto(),
                _ => el.ml_auto(),
            };
        }
    }
    // WARNING: right-to-left blocks put an over-constrained child at the right, so its left margin gives way.
    if !flex_parent
        && slot.is_none()
        && reader.parent(id, Prop::DIRECTION) == Some("rtl")
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
    let ink = style
        .get(Prop::COLOR)
        .and_then(style::color)
        .map_or(rgba(0x000000ff), |(ink, alpha)| {
            rgba(ink << 8 | alpha as u32)
        });
    let sides = style::BORDERS.map(|[width, kind, color]| (width, kind, color));
    let mut borders = sides.map(|(width, kind, _)| {
        if style
            .get(kind)
            .is_none_or(|kind| matches!(kind, "none" | "hidden"))
        {
            return 0.;
        }
        style
            .get(width)
            .map_or(Some(3.), |width| units.resolve(width, containing[0]))
            .unwrap_or(0.)
            .max(0.)
    });
    let mut border_colors = sides.map(|(_, _, name)| {
        style
            .get(name)
            .and_then(style::color)
            .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32))
    });
    let mut border_kinds = sides.map(|(_, kind, _)| style.get(kind).unwrap_or("none"));
    if inline_box {
        let [above, below] = [padding[0] + borders[0], padding[2] + borders[2]];
        // WARNING: like browsers, padding and borders of an inline box hang outside its line.
        margin[0] = -above;
        margin[2] = -below;
        el = el.mt(px(margin[0])).mb(px(margin[2]));
        let same = border_colors.iter().all(|color| *color == border_colors[0]);
        let border = if same && borders.iter().any(|width| *width > 0.) {
            border_colors[0]
        } else {
            renderer::transparent_black()
        };
        if border.a > 0. || inline_fill.a > 0. {
            let font = text.resolve_font(&style::font(style));
            let size_px = px(units.font.em);
            let height = f32::from(text.ascent(font, size_px) - text.descent(font, size_px));
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
                                break;
                            }
                            if top + quad.height < mask.top() {
                                continue;
                            }
                            window.paint_quad(renderer::quad(
                                Bounds::new(point(bounds.left() - px(sides[3]), top), quad),
                                radii.resolve(quad),
                                fill,
                                renderer::Edges {
                                    top: px(sides[0]),
                                    right: px(sides[1]),
                                    bottom: px(sides[2]),
                                    left: px(sides[3]),
                                },
                                border,
                                if dashed {
                                    renderer::BorderStyle::Dashed
                                } else {
                                    renderer::BorderStyle::Solid
                                },
                            ));
                        }
                    },
                )
                .absolute()
                .left(px(0.))
                .top(px(0.))
                .size_full(),
            );
            if same {
                border_colors = [renderer::transparent_black(); 4];
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
    let native =
        super::control::kind(&node.tag, input, list.is_some(), style).map(|(control, boxed)| {
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
                super::control::Control::Range => node
                    .current_value()
                    .and_then(|value| value.trim().parse::<f32>().ok())
                    .unwrap_or((min + max) / 2.),
                _ => number("value").unwrap_or(if control == super::control::Control::Progress {
                    -1.
                } else {
                    0.
                }),
            };
            let dark = dark();
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
                _ => style
                    .get(Prop::ACCENT_COLOR)
                    .and_then(style::color)
                    .map_or(if dark { 0x99c8ff } else { 0x0075ff }, |(color, _)| color),
            };
            super::control::Look {
                control,
                boxed,
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
        });
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
                    border_colors[side] = rgba(edge << 8 | 0xff);
                }
            }
            if let Some(fill) = fill {
                el = el.bg(rgba(fill << 8 | 0xff));
            }
        }
        if let Some(rows) = list
            && style.get(Prop::HEIGHT).is_none_or(|value| value == "auto")
        {
            el = el.h(px(rows as f32 * (units.font.lh + 1.).ceil()
                + padding[0]
                + padding[2]
                + borders[0]
                + borders[2]));
        }
        if look.control == super::control::Control::Menu {
            padding[1] += 16.;
            padding[3] += 4.;
            if list.is_none() && style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
                let (font, size) = (style::font(style), px(units.font.em));
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
                                color: renderer::black(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                                letter_spacing: px(0.),
                                word_spacing: px(0.),
                                font_size: None,
                                inset: Default::default(),
                                nowrap: false,
                            };
                            widest = widest
                                .max(f32::from(text.shape_line(&label, size, &[run], None).width));
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
                let face = text.resolve_font(&style::font(style));
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
        border_colors = [renderer::transparent_black(); 4];
        border_kinds = ["solid"; 4];
    }
    el = el
        .border_t(px(borders[0]))
        .border_r(px(borders[1]))
        .border_b(px(borders[2]))
        .border_l(px(borders[3]));
    let visible = (0..4)
        .filter(|&side| borders[side] > 0.)
        .collect::<Vec<_>>();
    let shaded = visible
        .iter()
        .any(|&side| matches!(border_kinds[side], "groove" | "ridge" | "inset" | "outset"));
    if let Some(&first) = visible.first() {
        if !shaded
            && visible
                .iter()
                .all(|&side| border_colors[side] == border_colors[first])
        {
            el = el.border_color(border_colors[first]);
        } else {
            el = el.border_color(renderer::transparent_black());
        }
        if visible
            .iter()
            .all(|&side| border_kinds[side] == border_kinds[first])
        {
            el.style().border_style = border_style(border_kinds[first]);
        } else if visible
            .iter()
            .any(|&side| matches!(border_kinds[side], "dashed" | "dotted"))
        {
            el.style().border_style = renderer::BorderStyle::Dashed;
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
    // WARNING: a flex item does not shrink below its natural size in its main axis while its minimum is `auto`, and
    // that minimum stays within its maximum.
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
            // WARNING: like a block, a field stretches between both insets when it is absolute.
            if !stretched(0)
                && !(matches!(position, "absolute" | "fixed")
                    && [Prop::LEFT, Prop::RIGHT]
                        .iter()
                        .all(|&prop| style.get(prop).is_some_and(|value| value != "auto")))
            {
                el = el.w(px(width + chrome[0]));
            }
            // WARNING: a textarea scrolls, so as a flex item it can shrink below its natural size.
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
    let radius = super::effects::radii(style, &units, basis);
    el.style().corner_radii = radius;
    if native.as_ref().is_some_and(|look| look.boxed) {
        el.style().corner_radii = renderer::Corners::all(px(2.).into()).into();
    }
    let mut inner_shadows = Vec::new();
    let mut intrinsic_width = None;
    let column_stretched = || {
        flex_parent
            && reader
                .parent(id, Prop::FLEX_DIRECTION)
                .is_some_and(|value| value.starts_with("column"))
            && style
                .get(Prop::ALIGN_SELF)
                .filter(|value| *value != "auto")
                .or_else(|| reader.parent(id, Prop::ALIGN_ITEMS))
                .is_none_or(|value| matches!(value, "normal" | "stretch"))
    };
    for (prop, value) in style.iter() {
        let value: &str = value;
        let vertical = matches!(
            prop,
            Prop::HEIGHT | Prop::MIN_HEIGHT | Prop::MAX_HEIGHT | Prop::TOP | Prop::BOTTOM
        );
        let base = containing[usize::from(vertical)];
        let length = || units.resolve(value, base);
        match prop {
            Prop::WIDTH | Prop::HEIGHT if sizeless => {}
            Prop::WIDTH
            | Prop::HEIGHT
            | Prop::MIN_WIDTH
            | Prop::MAX_WIDTH
            | Prop::MIN_HEIGHT
            | Prop::MAX_HEIGHT => {
                let axis = usize::from(vertical);
                let length: renderer::Length = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    match block_size {
                        Some(_) => px(percent.max(0.) / 100. * base + extra[axis]).into(),
                        None => renderer::Length::Calc(percent.max(0.) / 100., px(extra[axis])),
                    }
                } else if let Some((fraction, offset)) =
                    percent_calc(&units, value).filter(|_| block_size.is_none())
                {
                    renderer::Length::Calc(fraction, px(offset + extra[axis]))
                } else if let Some(length) = length() {
                    px(length.max(0.) + extra[axis]).into()
                } else if matches!(prop, Prop::WIDTH | Prop::HEIGHT)
                    && value == "auto"
                    && node.tag != "svg"
                    && !(editable && prop == Prop::WIDTH)
                    || prop == Prop::WIDTH
                        && node.tag == "table"
                        && value == "fit-content"
                        && column_stretched()
                    || prop == Prop::WIDTH
                        && value == "fit-content"
                        && matches!(position, "absolute" | "fixed")
                {
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
                        _ => max.min(min.max(containing[0])),
                    };
                    let size = layout_ceil((size - margin[1] - margin[3]).max(0.), pixel_ratio);
                    intrinsic_width = Some(size);
                    // WARNING: `containing` is only an estimate, so layout keeps a `fit-content` box inside its parent.
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
                    Prop::MIN_HEIGHT => el.min_h(length),
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
            // WARNING: `auto <ratio>` keeps a replaced element's natural ratio.
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
                el.style().flex_basis = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
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
                    "baseline" | "first baseline" | "last baseline" => {
                        renderer::AlignItems::Baseline
                    }
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
                    // WARNING: these ignore flex-direction reversal. `Start` is the left or top
                    // edge (rtl rows are `RowReverse`), and `left`/`right` act as `start` on a column.
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
                    ("right" | "-webkit-right", _) | ("end", false) | ("start", true) => {
                        el.text_right()
                    }
                    _ => el.text_left(),
                }
            }
            Prop::POSITION if matches!(value, "absolute" | "fixed") => el = el.absolute(),
            Prop::LEFT | Prop::RIGHT | Prop::TOP | Prop::BOTTOM
                if matches!(position, "relative" | "absolute") =>
            {
                let length: renderer::Length = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    match block_size {
                        Some(_) => px(percent / 100. * base).into(),
                        None => relative(percent / 100.),
                    }
                } else if let Some((fraction, offset)) =
                    percent_calc(&units, value).filter(|_| block_size.is_none())
                {
                    renderer::Length::Calc(fraction, px(offset))
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
                el.text_style().word_break = Some(style::word_break(style));
            }
            Prop::TEXT_WRAP => {
                el.text_style().text_wrap = Some(if value.contains("balance") {
                    renderer::TextWrap::Balance
                } else if value.contains("pretty") {
                    renderer::TextWrap::Pretty
                } else {
                    renderer::TextWrap::Wrap
                })
            }
            Prop::CURSOR => {
                if let Some(cursor) = style::cursor(value) {
                    el = el.cursor(cursor);
                }
            }
            _ => {}
        }
    }
    // WARNING: a replaced element or a control takes its natural size for an auto size, in the content box like its
    // natural ratio, unless a flex line stretches it. The ratio turns a set or stretched height into the width, and
    // the in-flow picture turns the used width into an auto height.
    if let Some(natural) = natural {
        let chrome = [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ];
        let props = [Prop::WIDTH, Prop::HEIGHT];
        // WARNING: the size attributes of images and videos only hint a size that no rule sets.
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
            hint[axis].is_some() || style.get(props[axis]).is_some_and(|value| value != "auto")
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
        let stretch = [stretched(0), stretched(1)];
        let css = el.style().aspect_ratio.is_some();
        // WARNING: the line of a single-line row with a set height is definite, so it carries the ratio.
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
        if sized {
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
        // WARNING: a line that is not definite takes its height from its items, so a stretched item keeps its
        // natural height as a minimum. An absolute box would stretch to both its insets, so it takes the height
        // of its ratio too.
        if let Some(height) = match natural_ratio {
            _ if set[1] || line.is_some() || css => None,
            None => Some(natural[1]),
            Some(ratio) if matches!(position, "absolute" | "fixed") => width.map(|width| width / ratio),
            Some(_) => None,
        } {
            let height = height + chrome[1];
            if stretch[1] && style.get(Prop::MIN_HEIGHT).is_none_or(|value| value == "auto") {
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
    // WARNING: like replaced elements, controls keep their own width as blocks.
    let replaced = natural.is_some()
        || symbol.is_some()
        || matches!(
            node.tag.as_str(),
            "svg" | "input" | "button" | "select" | "textarea" | "progress" | "meter"
        );
    let auto = [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT]
        .map(|prop| style.get(prop).is_some_and(|value| value == "auto"));
    if auto.contains(&true)
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
    if !flex_parent
        && slot.is_none()
        && !matches!(position, "absolute" | "fixed")
        && display.starts_with("inline")
        && let Some(parent) = node.parent
    {
        // WARNING: like shrink-to-fit, an inline block of auto width gets no wider than its line, which is always a
        // flex row, so its text wraps.
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
            _ if display == "inline" && symbol.is_none() => Some(units.font.lh),
            renderer::Length::Px(height) => Some(f32::from(height) + margin[0] - margin[2]),
            _ => None,
        };
        let sized = matches!(el.style().size.height, renderer::Length::Px(_));
        el.style().align_self = match align {
            "middle" if middle.is_none() => Some(renderer::AlignItems::Center),
            "top" => Some(renderer::AlignItems::FlexStart),
            "text-top" if !sized => Some(renderer::AlignItems::FlexStart),
            "bottom" => Some(renderer::AlignItems::FlexEnd),
            _ => None,
        };
        let below = |style: &style::Style, font: style::FontUnits| {
            let face = text.resolve_font(&style::font(style));
            font.lh - f32::from(text.baseline(face, px(font.em), px(font.lh), pixel_ratio))
        };
        let line = styles.lengths[parent].font;
        let replaced = symbol.is_some()
            || matches!(
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
            )
            || check
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
                && node.children.iter().all(|&child| {
                    doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty()
                });
        let replaced = replaced && alt.is_none();
        let field = editable && node.tag == "input";
        let swatch = native
            .as_ref()
            .is_some_and(|look| look.control == super::control::Control::Color);
        let parent_style = &styles.values[parent];
        let parent = below(parent_style, line);
        let edge = el.style().align_self.is_none()
            && (matches!(align, "text-top" | "text-bottom")
                || middle.is_some()
                || swatch
                || replaced && !field
                || display == "inline-grid"
                    && !node.children.iter().any(|&child| in_flow(doc, info, child)));
        if edge {
            el.style().baseline = renderer::Baseline::None;
        }
        let lift = if !edge {
            0.
        } else if matches!(align, "text-top" | "text-bottom") {
            let face = text.resolve_font(&style::font(parent_style));
            let [ascent, descent, _] = text
                .line_metrics(face, px(line.em * pixel_ratio))
                .map(|value| f32::from(value) / pixel_ratio);
            match el.style().size.height {
                renderer::Length::Px(height) if align == "text-top" => {
                    ascent - margin[0] - f32::from(height)
                }
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
            // WARNING: like Chrome, an empty button has its baseline at the bottom of its content box.
            -padding[2] - borders[2]
        } else if native.is_some() {
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
        let reserve = match (el.style().size.height, el.style().align_self) {
            (renderer::Length::Px(height), None) if display != "inline" => {
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
                _ if matches!(display, "inline-block" | "inline-grid") && !replaced => {
                    f32::INFINITY
                }
                _ if edge => 0.,
                _ if display == "inline" => below(style, units.font),
                _ => padding[2] + borders[2] + below(style, units.font),
            };
        // WARNING: the margin reserves the shift so the line box grows instead of overlapping its neighbors.
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
    if style
        .get(Prop::BACKGROUND_IMAGE)
        .is_some_and(|value| value != "none")
    {
        let base = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|state| state.base.clone());
        el = el.children(css_background(
            style,
            units,
            id,
            0,
            radius,
            view.clone(),
            base.as_ref(),
            &view.host.borrow().assets,
        ));
    }
    if !inner_shadows.is_empty() {
        el = el.child(inset_shadows(inner_shadows, radius, borders));
    }
    if cfg!(target_os = "macos")
        && let Some(symbol) = symbol
    {
        let symbol = symbol.replace('-', ".");
        el = el.child(
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| window.paint_symbol(&symbol, bounds, ink),
            )
            .absolute()
            .top(px(padding[0]))
            .right(px(padding[1]))
            .bottom(px(padding[2]))
            .left(px(padding[3])),
        );
    }
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
        });
        image.style().aspect_ratio = natural_ratio;
        if !cropped {
            image.style().corner_radii = renderer::Radii {
                x: corners.x.map(renderer::Length::Px),
                y: corners.y.map(renderer::Length::Px),
            };
        }
        el = el.child(image);
    }
    if let Some(lines) = style
        .get(Prop::WEBKIT_LINE_CLAMP)
        .or_else(|| style.get(Prop::LINE_CLAMP))
        .and_then(|value| value.parse::<usize>().ok())
    {
        let text = el.text_style();
        text.white_space = Some(renderer::WhiteSpace::Normal);
        text.line_clamp = Some(lines.max(1));
        text.text_overflow = Some("…".into());
    } else if style.get(Prop::TEXT_OVERFLOW) == Some("ellipsis")
        && matches!(style.get(Prop::WHITE_SPACE), Some("nowrap" | "pre"))
        && el.style().overflow.x != renderer::Overflow::Visible
    {
        el.text_style().text_overflow = Some("…".into());
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
    if let Some(width) = units
        .resolve(
            style.get(Prop::OUTLINE_WIDTH).unwrap_or("3px"),
            containing[0],
        )
        .filter(|width| {
            *width > 0.
                && style
                    .get(Prop::OUTLINE_STYLE)
                    .is_some_and(|kind| !matches!(kind, "none" | "hidden"))
        })
    {
        let grow = width
            + style
                .get(Prop::OUTLINE_OFFSET)
                .and_then(|v| units.resolve(v, containing[0]))
                .unwrap_or(0.);
        let color = style
            .get(Prop::OUTLINE_COLOR)
            .and_then(style::color)
            .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32));
        let mut outline = renderer::node()
            .absolute()
            .top(px(-grow - borders[0]))
            .right(px(-grow - borders[1]))
            .bottom(px(-grow - borders[2]))
            .left(px(-grow - borders[3]))
            .border(px(width))
            .border_color(color);
        let corners = radius.to_pixels(size(px(basis()[0]), px(basis()[1])));
        let grown = |radius: renderer::Pixels| renderer::Length::Px(radius + px(grow));
        outline.style().corner_radii = renderer::Radii {
            x: corners.x.map(grown),
            y: corners.y.map(grown),
        };
        outline.style().border_style =
            border_style(style.get(Prop::OUTLINE_STYLE).unwrap_or("solid"));
        el = el.child(outline);
    }
    if shaded
        || visible
            .iter()
            .any(|&side| border_colors[side] != border_colors[visible[0]])
    {
        let mut bands = vec![([0.; 4], borders, [renderer::transparent_black(); 4])];
        let split = (0..4).any(|side| matches!(border_kinds[side], "groove" | "ridge"));
        if split {
            bands.push(([0.; 4], [0.; 4], [renderer::transparent_black(); 4]));
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
        let kinds = border_kinds.map(border_style);
        el = el.child(
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| {
                    let outer = Bounds::new(
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
                        let bounds = Bounds::from_corners(
                            outer.origin + point(px(inset[3]), px(inset[0])),
                            outer.bottom_right() - point(px(inset[1]), px(inset[2])),
                        );
                        let corners = radius
                            .resolve(outer.size)
                            .inset(px(inset[0]), px(inset[1]), px(inset[2]), px(inset[3]))
                            .clamp_radii_for_quad_size(bounds.size);
                        for side in 0..4 {
                            if widths[side] <= 0. {
                                continue;
                            }
                            window.paint_quad(
                                renderer::quad(
                                    bounds,
                                    corners,
                                    renderer::transparent_black(),
                                    renderer::Edges {
                                        top: px(widths[0]),
                                        right: px(widths[1]),
                                        bottom: px(widths[2]),
                                        left: px(widths[3]),
                                    },
                                    colors[side],
                                    kinds[side],
                                )
                                .sides(1 << side),
                            );
                        }
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
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
        el = el.child(super::control::layer(id, look, view.clone()));
    }
    let overscroll = overscroll(style);
    // WARNING: `overflow: hidden` boxes track a scroll handle only once a script scrolls them, or
    // when `overscroll-behavior` stops scrolling at them.
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
            reader.log(Query::Scroll(id as u32), 0, hashed(scrolled(scroll.get(&id))));
            scroll.get(&id).cloned()
        } else {
            None
        }
    };
    let mut bars = None;
    if let Some(handle) = handle {
        if !editable {
            el = el.track_scroll(&handle);
            el.style().restrict_scroll_to_axis = true;
            el.style().overscroll = overscroll;
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
        });
    }
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
                .entry((parent, true))
                .or_insert_with(|| {
                    let items: Vec<usize> = doc.nodes[parent]
                        .children
                        .iter()
                        .copied()
                        .filter(|&child| {
                            styles.values[child]
                                .get(Prop::DISPLAY)
                                .is_some_and(|value| value == "list-item")
                        })
                        .collect();
                    let index = items.iter().enumerate().map(|(index, &item)| (item, index));
                    let index = index.collect();
                    Rc::new((items, index))
                })
                .clone()
        });
        let position = items
            .as_ref()
            .and_then(|items| items.1.get(&id).copied())
            .unwrap_or(0) as i64;
        let count = items.as_ref().map_or(1, |items| items.0.len()) as i64;
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
        let face = text.resolve_font(&style::font(marker_style.unwrap_or(style)));
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
            let own = text.resolve_font(&style::font(style));
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
                .map_or(size / 4., |space| f32::from(space.width));
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
        } else if inside {
            renderer::node()
                .flex_shrink_0()
                .mr(px(units.font.em * 0.3))
                .child(marker)
        } else if content.is_none() && matches!(kind, "disc" | "circle" | "square") {
            let color = marker_style
                .unwrap_or(style)
                .get(Prop::COLOR)
                .and_then(style::color)
                .map_or(rgba(0x000000ff), |(ink, alpha)| {
                    rgba(ink << 8 | alpha as u32)
                });
            let [ascent, descent, _] = metrics;
            let offset = (ascent * 2. / 3.).floor();
            let leading = ((line * pixel_ratio - ascent - descent) / 2.).floor();
            let dot = renderer::node()
                .absolute()
                .left(px(padding[3] - (offset + 7.) / pixel_ratio))
                .top(px(top
                    + (leading + (3. * (ascent - offset) / 2.).floor())
                        / pixel_ratio))
                .size(px(((offset + 1.) / 2.).floor() / pixel_ratio));
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
                .pr(px(units.font.em * 0.5))
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
            marker_box.text_style().font_style = Some(face.style);
            if let Some((ink, alpha)) = marker.get(Prop::COLOR).and_then(style::color) {
                marker_box = marker_box.text_color(rgba(ink << 8 | alpha as u32));
            }
        }
        el = el.child(marker_box);
    }
    // WARNING: an estimate of the content box, for a box that the last layout did not size.
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
    // WARNING: pseudo-elements query their originating element, so a container records its size even without
    // children, and its pseudo-elements and children resolve container units against it. The size from the last
    // layout wins over the estimate.
    let own = style
        .get(Prop::CONTAINER_TYPE)
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
            size
        });
    let mut after = None;
    if let Some(pseudos) = styles.pseudos.get(&id) {
        let mut units = units;
        units.container = own.or(units.container);
        for (index, pseudo) in pseudos.iter().enumerate() {
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
            let text = pseudo_content(doc, id, pseudo, content);
            let mut pseudo_units = units;
            if let Some(size) = pseudo
                .get(Prop::FONT_SIZE)
                .and_then(|v| units.resolve(v, units.font.em))
            {
                pseudo_units.font = style::FontUnits::new(size);
            }
            let face = style::font(pseudo);
            let mut decoration = renderer::node()
                .flex()
                .flex_row()
                .flex_shrink_0()
                .text_size(px(pseudo_units.font.em))
                .font_family(face.family)
                .font_weight(face.weight);
            match style::line_height(pseudo) {
                style::LineHeight::Fixed(height) => decoration = decoration.line_height(px(height)),
                style::LineHeight::Scale(scale) => {
                    decoration = decoration.line_height(px(scale * pseudo_units.font.em))
                }
                style::LineHeight::Normal => {}
            }
            if let Some((ink, alpha)) = pseudo.get(Prop::COLOR).and_then(style::color) {
                decoration = decoration.text_color(rgba(ink << 8 | alpha as u32));
            }
            let absolute = pseudo
                .get(Prop::POSITION)
                .is_some_and(|v| matches!(v, "absolute" | "fixed"));
            if absolute {
                decoration = decoration.absolute();
            } else if pseudo
                .get(Prop::DISPLAY)
                .is_some_and(|value| !value.starts_with("inline"))
                && (all_inline || display == "inline")
            {
                decoration = decoration.w_full();
            }
            let pseudo_padding = pseudo_units
                .edges(pseudo, style::PADDING, containing[0])
                .map(|v| v.max(0.));
            let pseudo_margin = pseudo_units.edges(pseudo, style::MARGIN, containing[0]);
            let pseudo_borders = sides.map(|(width, kind, _)| {
                if pseudo
                    .get(kind)
                    .is_none_or(|kind| matches!(kind, "none" | "hidden"))
                {
                    return 0.;
                }
                pseudo
                    .get(width)
                    .map_or(Some(3.), |width| pseudo_units.resolve(width, containing[0]))
                    .unwrap_or(0.)
                    .max(0.)
            });
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
            let pseudo_radius = super::effects::radii(pseudo, &pseudo_units, || containing);
            decoration.style().corner_radii = pseudo_radius;
            let colors = sides.map(|(_, _, color)| {
                pseudo
                    .get(color)
                    .and_then(style::color)
                    .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32))
            });
            let shown: Vec<usize> = (0..4).filter(|&side| pseudo_borders[side] > 0.).collect();
            if let Some(&first) = shown.first() {
                if shown.iter().all(|&side| colors[side] == colors[first]) {
                    decoration = decoration.border_color(colors[first]);
                } else {
                    decoration = decoration.border_color(renderer::transparent_black());
                    let kinds =
                        sides.map(|(_, kind, _)| border_style(pseudo.get(kind).unwrap_or("solid")));
                    let (borders, radius) = (pseudo_borders, pseudo_radius);
                    decoration = decoration.child(
                        renderer::canvas(
                            |_, _| (),
                            move |bounds, _, window| {
                                let outer = Bounds::new(
                                    point(
                                        bounds.left() - px(borders[3]),
                                        bounds.top() - px(borders[0]),
                                    ),
                                    size(
                                        bounds.size.width + px(borders[1] + borders[3]),
                                        bounds.size.height + px(borders[0] + borders[2]),
                                    ),
                                );
                                let corners = radius
                                    .resolve(outer.size)
                                    .clamp_radii_for_quad_size(outer.size);
                                for side in 0..4 {
                                    if borders[side] <= 0. {
                                        continue;
                                    }
                                    window.paint_quad(
                                        renderer::quad(
                                            outer,
                                            corners,
                                            renderer::transparent_black(),
                                            renderer::Edges {
                                                top: px(borders[0]),
                                                right: px(borders[1]),
                                                bottom: px(borders[2]),
                                                left: px(borders[3]),
                                            },
                                            colors[side],
                                            kinds[side],
                                        )
                                        .sides(1 << side),
                                    );
                                }
                            },
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    );
                }
            }
            let mut pseudo_shadows = Vec::new();
            for (prop, value) in pseudo.iter() {
                let value: &str = value;
                let vertical = matches!(
                    prop,
                    Prop::HEIGHT | Prop::MIN_HEIGHT | Prop::MAX_HEIGHT | Prop::TOP | Prop::BOTTOM
                );
                let base = containing[usize::from(vertical)];
                let length = || -> Option<renderer::Length> {
                    match value
                        .strip_suffix('%')
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                    {
                        Some(percent) => Some(relative(percent / 100.)),
                        None => match percent_calc(&pseudo_units, value) {
                            Some((fraction, offset)) => {
                                Some(renderer::Length::Calc(fraction, px(offset)))
                            }
                            None => pseudo_units
                                .resolve(value, base)
                                .map(|value| px(value).into()),
                        },
                    }
                };
                match prop {
                    Prop::WIDTH
                    | Prop::HEIGHT
                    | Prop::MIN_WIDTH
                    | Prop::MAX_WIDTH
                    | Prop::MIN_HEIGHT
                    | Prop::MAX_HEIGHT
                    | Prop::LEFT
                    | Prop::TOP
                    | Prop::RIGHT
                    | Prop::BOTTOM => {
                        let Some(length) = length() else { continue };
                        decoration = match prop {
                            Prop::WIDTH => decoration.w(length),
                            Prop::HEIGHT => decoration.h(length),
                            Prop::MIN_WIDTH => decoration.min_w(length),
                            Prop::MAX_WIDTH => decoration.max_w(length),
                            Prop::MIN_HEIGHT => decoration.min_h(length),
                            Prop::MAX_HEIGHT => decoration.max_h(length),
                            Prop::LEFT if absolute => decoration.left(length),
                            Prop::TOP if absolute => decoration.top(length),
                            Prop::RIGHT if absolute => decoration.right(length),
                            Prop::BOTTOM if absolute => decoration.bottom(length),
                            _ => decoration,
                        };
                    }
                    Prop::BACKGROUND_COLOR => {
                        if let Some((color, alpha)) = style::color(value) {
                            decoration = decoration.bg(rgba(color << 8 | alpha as u32));
                        }
                    }
                    Prop::OPACITY => {
                        if let Ok(opacity) = value.parse::<f32>() {
                            decoration = decoration.opacity(opacity.clamp(0., 1.));
                        }
                    }
                    Prop::BOX_SHADOW => {
                        decoration.style().box_shadow = pseudo_units.shadows(value, false);
                        pseudo_shadows = pseudo_units.shadows(value, true);
                    }
                    _ => {}
                }
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
                    view.clone(),
                    base.as_ref(),
                    &view.host.borrow().assets,
                ));
            }
            if !pseudo_shadows.is_empty() {
                decoration =
                    decoration.child(inset_shadows(pseudo_shadows, pseudo_radius, pseudo_borders));
            }
            let collapsed = text.trim_ascii().is_empty()
                && pseudo
                    .get(Prop::DISPLAY)
                    .is_some_and(|value| !value.starts_with("inline"))
                && pseudo.get(Prop::WHITE_SPACE).is_none_or(|value| {
                    !matches!(value, "pre" | "pre-wrap" | "pre-line" | "break-spaces")
                });
            if !text.is_empty() && !collapsed {
                decoration = decoration.child(SharedString::from(text));
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
                            bounds: Bounds::default(),
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
            let decoration = if [Prop::TRANSFORM, Prop::TRANSLATE, Prop::ROTATE, Prop::SCALE]
                .iter()
                .any(|&prop| pseudo.get(prop).is_some_and(|value| value != "none"))
            {
                decoration.hooks(super::paint::Transformed {
                    style: Rc::new(pseudo.clone()),
                    units: pseudo_units,
                    matrix: Default::default(),
                })
            } else {
                decoration
            };
            let order = pseudo
                .get(Prop::Z_INDEX)
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            let decoration = if order != 0
                && pseudo
                    .get(Prop::POSITION)
                    .is_some_and(|value| value != "static")
            {
                decoration.z_index(order)
            } else {
                decoration
            };
            if index == 0 {
                if std::mem::take(&mut strut_due) {
                    el = el.child(strut());
                }
                el = el.child(decoration);
            } else {
                after = Some(decoration);
            }
        }
    }
    let preserve = style
        .get(Prop::WHITE_SPACE)
        .is_some_and(|v| matches!(v, "pre" | "pre-wrap" | "break-spaces"))
        || node.tag == "pre";
    if (node.tag == "#text"
        || display == "inline" && symbol.is_none() && !flex_parent && slot.is_none())
        && style
            .get(Prop::WHITE_SPACE)
            .is_none_or(|value| !matches!(value, "pre" | "nowrap"))
    {
        el = el.min_w(px(0.)).max_w(relative(1.)).flex_shrink(1.);
    }
    if preserve && node.text == "\n" {
        el = el.min_h(px(units.font.lh));
    }
    let inline =
        if editable || symbol.is_some() || matches!(node.tag.as_str(), "svg" | "canvas" | "iframe")
        {
            None
        } else {
            measured
                .borrow_mut()
                .texts
                .entry(id)
                .or_insert_with(|| style::inline_text(doc, styles, id).map(Rc::new))
                .clone()
                .filter(|data| {
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
    if (hit_test || node.tag == "#text")
        && !drag
        && {
            // WARNING: an inherited mode is the same in all boxes from the one that sets it down, so a box above
            // replays it as its own. A box that sets its own mode reads its parent too, because an ancestor in an
            // exit transition gets `display: none` and hides the box with no new revision.
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
        el = el.cursor(renderer::CursorStyle::IBeam);
    }
    let mut plans = 0;
    if let Some(data) = inline {
        #[cfg(debug_assertions)]
        reader.extra.set(hashed(&*data.text));
        let atom_containing = [
            (containing[0]
                - margin[1]
                - margin[3]
                - padding[1]
                - padding[3]
                - borders[1]
                - borders[3])
                .max(0.),
            containing[1],
        ];
        let mut atom_elements: Vec<renderer::Child> = data
            .atoms
            .iter()
            .map(|atom| element(build, atom.node, atom_containing, container, None))
            .collect();
        let atom_slots: Rc<[super::text::AtomSlot]> = data
            .atoms
            .iter()
            .zip(&mut atom_elements)
            .map(|(atom, child)| {
                let last = view
                    .host
                    .borrow()
                    .dom
                    .get(&view.id)
                    .and_then(|dom| dom.atoms.get(&atom.node).copied());
                reader.log(
                    Query::Atom(atom.node as u32),
                    0,
                    hashed(last.map(|size| size.map(f32::to_bits))),
                );
                let size = last.unwrap_or_else(|| {
                    let [_, max, _, height] = super::grid::intrinsic(
                        doc,
                        atom.node,
                        styles,
                        Some(containing[0]),
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
            })
            .collect();
        // WARNING: `text-box` trims the block by moving its text with negative margins, so the glyphs stay where they would be.
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
            let font = system.resolve_font(&style::font(style));
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
        el = el.child(
            renderer::node()
                .min_w(px(0.))
                .max_w(relative(1.))
                .flex_shrink(1.)
                .mt(px(-trim_top))
                .mb(px(-trim_bottom))
                .child(InlineText {
                    data: data.clone(),
                    atoms: atom_slots.clone(),
                    view: view.clone(),
                    shaped: Default::default(),
                    prepainted: Default::default(),
                    justify: style
                        .get(Prop::TEXT_ALIGN)
                        .is_some_and(|value| value == "justify")
                        .then(|| {
                            style
                                .get(Prop::TEXT_ALIGN_LAST)
                                .is_some_and(|value| value == "justify")
                        }),
                }),
        );
        for (index, (atom, child)) in data.atoms.iter().zip(atom_elements).enumerate() {
            let mut wrapper = renderer::node()
                .absolute()
                .left(px(padding[3]))
                .top(px(padding[0]))
                .flex()
                .flex_row()
                .max_w(renderer::Length::Calc(1., px(-padding[1] - padding[3])));
            wrapper.style().align_items = Some(renderer::AlignItems::Baseline);
            el = el.child(
                wrapper
                    .hooks(super::text::AtomBox {
                        slots: atom_slots.clone(),
                        index,
                        node: atom.node,
                        view: view.clone(),
                    })
                    .child(child),
            );
        }
    } else if !node.children.is_empty()
        && node.tag != "canvas"
        && node.tag != "iframe"
        && node.tag != "svg"
        && !editable
    {
        let mut content = content_box();
        let container = own.unwrap_or(container);
        let flexed = style
            .get(Prop::DISPLAY)
            .is_some_and(|v| v.ends_with("grid"))
            && slot.is_none()
            && style.get(Prop::HEIGHT).is_none_or(|v| v == "auto")
            && reader
                .parent(id, Prop::DISPLAY)
                .is_some_and(|v| v.ends_with("flex"))
            && reader
                .parent(id, Prop::FLEX_DIRECTION)
                .is_some_and(|v| v.starts_with("column"))
            && (style
                .get(Prop::FLEX_GROW)
                .and_then(|v| v.parse::<f32>().ok())
                .is_some_and(|v| v > 0.)
                || style
                    .get(Prop::FLEX_BASIS)
                    .is_some_and(|v| v != "auto" && v != "content"));
        let shrunk = style.get(Prop::DISPLAY).is_some_and(|v| v == "grid")
            && slot.is_none()
            && style.get(Prop::WIDTH).is_none_or(|v| v == "auto")
            && {
                let (shrunk, farthest) = shrinks(doc, styles, id, true);
                reader.log(Query::Shrunk, reader.scope_of(id, farthest), hashed(shrunk));
                shrunk
            };
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
                // WARNING: tracks size the untransformed box, so they read its laid-out size, not its window bounds.
                // New content makes the last size meaningless. Most content fits, so tracks take its full width,
                // and prepaint draws again when the layout disagrees.
                let last = {
                    let mut host = view.host.borrow_mut();
                    let dom = host.dom.entry(view.id).or_default();
                    let same = dom.grid_widest.insert(id, max) == Some(max);
                    same.then(|| dom.cull.widths.get(&id).map(|size| size[0].max(0.)))
                        .flatten()
                };
                // WARNING: laid-out bounds are in layout units, so a fitting grid may look narrower.
                content[0] = last.map_or(content[0].min(max), |last| {
                    if last + 0.5 >= max {
                        last.max(max)
                    } else {
                        last
                    }
                });
                tracked[0] = Some((last.unwrap_or(content[0]), horizontal));
            }
            // WARNING: `containing` is an estimate that doesn't know flex siblings. A grid that fills its parent lets
            // layout size it, and its tracks read that width from the last layout. Prepaint draws again when it changes.
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
            }
            // WARNING: a positioned box with both insets takes its height from its containing block, as with a set height.
            let inset = |prop| {
                style
                    .get(prop)
                    .and_then(|value| units.resolve(value, containing[1]))
            };
            let stretched = matches!(position, "absolute" | "fixed")
                .then(|| Some(inset(Prop::TOP)? + inset(Prop::BOTTOM)?))
                .flatten();
            let height =
                if slot.is_some() || style.get(Prop::HEIGHT).is_some_and(|v| v != "auto") {
                    Some(content[1])
                } else if let Some(insets) = stretched {
                    Some((containing[1] - insets - margin[0] - margin[2] - vertical).max(0.))
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
            // WARNING: entries stay while the grid is kept, so its size is checked in each frame.
            let mut host = view.host.borrow_mut();
            let dom = host.dom.entry(view.id).or_default();
            for (axis, track) in tracked.iter().enumerate() {
                if let Some((used, _)) = track {
                    reader.log(Query::Track(id as u32, axis as u8), 0, used.to_bits().into());
                }
            }
            reader.log(Query::Baselines(id as u32), 0, baselines(doc, Some(&*dom), id));
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
            el = el.flex().flex_col();
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
            let auto = style.get(Prop::HEIGHT).is_none_or(|v| v == "auto");
            let rows = grid.rows.iter().sum::<f32>()
                + grid.rows.len().saturating_sub(1) as f32 * grid.gap[1];
            if auto && !flexed {
                el = el.h(px(rows + padding[0] + padding[2] + borders[0] + borders[2]));
            }
            if !flexed && made.absolute {
                blocks.borrow_mut().insert(
                    id,
                    [
                        content[0] + padding[1] + padding[3],
                        if auto { rows } else { content[1] } + padding[0] + padding[2],
                    ],
                );
            }
        }
        let mut children = if grid.is_some() {
            node.children.clone()
        } else {
            flat.clone()
        };
        // WARNING: each level of boxes is one large native frame, so levels past the stack budget are not drawn.
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
            children.sort_by_key(|child| {
                styles.values[*child]
                    .get(Prop::ORDER)
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            });
        }
        let inline_element = |child: Option<&usize>| {
            child.is_some_and(|&child| {
                let node = &doc.nodes[child];
                node.tag == "#text" && !node.text.trim_ascii().is_empty()
                    || !node.tag.starts_with('#') && node.tag != "br" && inline_level(child)
            })
        };
        let multicol = columns(style, &units, content[0]);
        let mut line: Option<renderer::Node> = None;
        let mut line_strut = false;
        let mut joined = usize::MAX;
        let mut floating = Vec::new();
        let mut group: Option<(Vec<Child>, Vec<Child>, Vec<Child>)> = None;
        let columns = multicol.filter(|_| flow && grid.is_none() && !all_inline);
        let mut collected: Vec<(Option<usize>, Child)> = Vec::new();
        let shown = |index: usize| {
            let child = &doc.nodes[children[index]];
            preserve
                || !(child.tag == "#text" && child.text.trim_ascii().is_empty())
                || flow
                    && grid.is_none()
                    && inline_element(index.checked_sub(1).and_then(|index| children.get(index)))
                    && inline_element(children.get(index + 1))
        };
        // WARNING: wrapped lines are culled whole, and only when the container's height is its content's, so `align-content` has no space to share.
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
                wrap || style
                    .get(Prop::FLEX_DIRECTION)
                    .is_some_and(|v| v == "column")
                    && style.get(Prop::FLEX_WRAP).is_none_or(|v| v == "nowrap")
                    && style.get(Prop::JUSTIFY_CONTENT).is_none_or(|v| {
                        matches!(v, "normal" | "flex-start" | "start" | "stretch")
                    })
            } else {
                flow && !children.iter().any(|&child| floated(child))
            };
        // WARNING: how this box culls its children, for `PageState::reach`: 1 as blocks, 2 as a flex column, which
        // keeps boxes with a set height, and none below a box that moves its content, whose subtree never culls.
        plans = if info[id].flags & cull::MOVES_CONTENT != 0 {
            u8::MAX
        } else if grid.is_some() || stacks && (!flexbox || wrap) {
            1
        } else {
            2 * u8::from(stacks)
        };
        let stack = build.zone.clone().filter(|_| stacks);
        // WARNING: a debug check builds a kept box again without culling, so it leaves out what the kept build culled.
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
                // WARNING: the first and last box of a run collapse their margins with siblings outside it, which may have changed.
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
                plan = (!runs.is_empty()).then_some((slots, runs, [left, width], zone));
            }
        }
        let mut culled = Vec::new();
        if let (Some(grid), Some(zone)) = (&grid, &build.zone) {
            if let Some(dom) = view.host.borrow().dom.get(&view.id) {
                for &(child, _, [x, y, width, height], _) in &grid.cells {
                    let placed = dom.cull.placed.get(child).copied().unwrap_or_default();
                    let [top, right, bottom, left] = placed.2;
                    // WARNING: the cell sizes come before layout snaps boxes to pixels, so they compare with the unsnapped cell sizes of the last layout.
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
                        // WARNING: layout truncates each inset and margin to layout units, so the offsets from the anchor do too.
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
                if std::mem::take(&mut strut_next) && group.is_none() && columns.is_none() {
                    el = el.child(strut());
                }
                if std::mem::take(&mut strut_due) {
                    el = el.child(strut());
                }
                match group.as_mut() {
                    Some((_, _, content)) => content.push(element),
                    None if columns.is_some() => collected.push(($child, element)),
                    None => el.push(element),
                }
            }};
        }
        macro_rules! close_group {
            () => {{
                if let Some(open) = line.take() {
                    place!(open, None);
                }
                if let Some((lefts, rights, content)) = group.take() {
                    let row = renderer::node()
                        .flex()
                        .flex_row()
                        .items_start()
                        .w_full()
                        .children(lefts)
                        .child(
                            renderer::node()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.))
                                .children(content),
                        )
                        .children(rights.into_iter().rev());
                    if columns.is_some() {
                        collected.push((None, row.into()));
                    } else {
                        el = el.child(row);
                    }
                }
            }};
        }
        let line_break = || {
            let mut el = renderer::node().w_full();
            el.style().align_self = Some(renderer::AlignSelf::FlexStart);
            el
        };
        let hangs = !preserve
            && style
                .get(Prop::WHITE_SPACE)
                .is_none_or(|value| value != "nowrap")
            && style
                .get(Prop::TEXT_WRAP)
                .is_none_or(|value| !value.contains("nowrap"));
        el.reserve(children.len());
        for (index, &child) in children.iter().enumerate() {
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
                    Some((
                        bounds,
                        super::grid::Grid {
                            columns: grid.columns.get(column..column + columns)?.to_vec(),
                            rows: grid.rows.get(row..row + rows)?.to_vec(),
                            gap: grid.gap,
                            baseline,
                            first_item: grid.first_item.filter(|&first| first == item),
                            ..Default::default()
                        },
                    ))
                });
                let cell = placement
                    .as_ref()
                    .map(|(bounds, grid)| (*bounds, grid.first_item.is_some()));
                let child_element = element(build, child, content, container, placement);
                let child_element: renderer::Child = match cell {
                    Some((bounds, first)) => {
                        let mut cell = renderer::node()
                            .absolute()
                            .left(px(bounds[0]))
                            .top(px(bounds[1]))
                            .w(px(bounds[2]))
                            .h(px(bounds[3]));
                        cell.style().baseline_source = first;
                        cell.child(child_element).into()
                    }
                    None => child_element,
                };
                let out_of_flow = styles.values[child]
                    .get(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"));
                let child_style = &styles.values[child];
                // WARNING: like browsers, a collapsible space after an inline box hangs at the end of its line instead
                // of starting the next one, so it joins the box in one item and shrinks first. There it doesn't move
                // a centered or end aligned line (`Inline::Hangs`).
                let child_element = match children.get(index + 1) {
                    Some(&next)
                        if hangs
                            && grid.is_none()
                            && !out_of_flow
                            && !floated(child)
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
                    _ => child_element,
                };
                if grid.is_none() && !out_of_flow {
                    let clear = child_style.get(Prop::CLEAR).unwrap_or("none");
                    if group.is_some() && clear != "none" {
                        close_group!();
                    }
                    if floated(child) {
                        if let Some(open) = line.take() {
                            place!(open, None);
                        }
                        let open = group.get_or_insert_with(Default::default);
                        let float = if child_style
                            .get(Prop::WIDTH)
                            .is_none_or(|value| value == "auto")
                        {
                            renderer::node()
                                .flex()
                                .flex_col()
                                .flex_shrink(1.)
                                .min_w(px(0.))
                                .child(child_element)
                                .into()
                        } else {
                            child_element
                        };
                        if float_side(child_style) == Some(true) {
                            open.1.push(float);
                        } else {
                            open.0.push(float);
                        }
                        continue;
                    }
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
                    // WARNING: the placeholder must come before the box, which reads its bounds in prepaint.
                    let block = child_style
                        .get(Prop::DISPLAY)
                        .is_some_and(|value| !value.starts_with("inline"));
                    let pending = flat
                        [..flat.iter().position(|&other| other == child).unwrap_or(0)]
                        .iter()
                        .rev()
                        .find(|&&other| in_flow(doc, info, other) && !floated(other))
                        .filter(|&&other| {
                            block && line.is_none() && !all_inline && self::block(build, other)
                        })
                        .map_or(0., |&other| {
                            let (_, [positive, negative], _) = adjoining(build, other, content[0]);
                            let used = margins(
                                build,
                                other,
                                &styles.lengths[other],
                                content[0],
                                false,
                                false,
                            )[2];
                            (positive + negative - used).max(0.)
                        });
                    let entity = view.clone();
                    let mut placeholder = renderer::canvas(
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
                                || all_inline
                                    && flat
                                        .iter()
                                        .any(|&other| other != child && in_flow(doc, info, other)))
                        {
                            units.font.lh
                        } else {
                            0.
                        },
                    ));
                    placeholder.style().align_self = Some(renderer::AlignSelf::FlexStart);
                    match line.take() {
                        Some(open) => line = Some(open.child(placeholder)),
                        None if all_inline || block => place!(placeholder, None),
                        None => {
                            let mut next = renderer::node().flex().flex_row().flex_wrap();
                            next.style().justify_content = line_justify;
                            next.style().inline = renderer::Inline::Lines;
                            line = Some(next.child(placeholder));
                            line_strut = false;
                        }
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
                        Some(open) => open.child(child_element),
                        None if doc.nodes[child].tag != "br" => {
                            let (shared, aligned) = with_strut(child_element);
                            line_strut = aligned;
                            renderer::node().flex().flex_row().flex_wrap().child(shared)
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
                    floating.push(child_element);
                } else {
                    if let Some(line) = line.take() {
                        place!(line, None);
                    }
                    let (child_element, aligned) = if strut_due && doc.nodes[child].tag != "br" {
                        strut_due = false;
                        let (shared, aligned) = with_strut(child_element);
                        (shared.into(), aligned)
                    } else {
                        (child_element, false)
                    };
                    let direct = group.is_none() && columns.is_none();
                    place!(child_element, Some(child));
                    strut_next = aligned && direct;
                    if flow && all_inline && doc.nodes[child].tag == "br" {
                        place!(line_break(), None);
                    }
                }
            }
        }
        close_group!();
        #[cfg(debug_assertions)]
        {
            culled_ids.sort_unstable();
            reader.plan.replace(culled_ids.into());
        }
        if let Some((count, gap, rule)) = columns {
            let width = ((content[0] - gap * (count - 1) as f32) / count as f32).max(0.);
            let heights: Vec<f32> = collected
                .iter()
                .map(|(child, _)| {
                    child.map_or(units.font.lh, |child| {
                        super::grid::intrinsic(
                            doc,
                            child,
                            styles,
                            Some(width),
                            None,
                            text,
                            &mut measured.borrow_mut(),
                        )[3]
                    })
                })
                .collect();
            let target = heights.iter().sum::<f32>() / count as f32;
            let mut columns: Vec<Vec<Child>> = (0..count).map(|_| Vec::new()).collect();
            let (mut column, mut filled) = (0, 0.);
            for ((_, item), height) in collected.into_iter().zip(heights) {
                if filled > 0. && filled + height / 2. > target && column + 1 < count {
                    column += 1;
                    filled = 0.;
                }
                filled += height;
                columns[column].push(item);
            }
            let mut row = renderer::node()
                .flex()
                .flex_row()
                .items_start()
                .w_full()
                .gap_x(px(gap));
            for items in columns {
                row = row.child(
                    renderer::node()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.))
                        .children(items),
                );
            }
            if let Some((rule_width, color, kind)) = rule {
                row = row.child(column_rules(count, gap, rule_width, color, kind));
            }
            el = el.child(row);
        }
        el = el.children(floating);
    }
    if let Some(after) = after {
        el = el.child(after);
    }
    if slot.is_some() && flow && matches!(node.tag.as_str(), "td" | "th") {
        let align = match style.get(Prop::VERTICAL_ALIGN) {
            Some("middle") => Some(renderer::AlignContent::Center),
            Some("bottom") => Some(renderer::AlignContent::FlexEnd),
            _ => None,
        };
        if all_inline {
            el.style().align_content = align;
        } else {
            el.style().justify_content = align;
        }
    }
    if let Some((bounds, grid)) = &slot {
        el.style().baseline_source = grid.first_item.is_some();
        el = el
            .absolute()
            .left(px(0.))
            .top(px(0.))
            .mt(px(margin[0]))
            .mr(px(margin[1]))
            .mb(px(margin[2]))
            .ml(px(margin[3]))
            .w(px(layout_ceil(
                (bounds[2] - margin[1] - margin[3]).max(0.),
                pixel_ratio,
            )))
            .h(px((bounds[3] - margin[0] - margin[2]).max(0.)));
    }
    // WARNING: Windows moves the window in the native hit test (`window_drag_area`), macOS and Linux on the first drag.
    if drag {
        el = el.window_drag_area().on_mouse_down(MouseButton::Left, {
            let view = view.clone();
            move |event: &renderer::MouseDownEvent, window, cx| {
                // WARNING: a drag box gets the presses of all its children, so the box under the pointer
                // decides: the nearest one that sets a region, as a `no-drag` button must get its click.
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
                    let at = [event.position.x, event.position.y].map(f32::from);
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
                cx.stop_propagation();
                if event.click_count != 2 {
                    view.moving.set(true);
                } else if cfg!(target_os = "macos") {
                    window.titlebar_double_click();
                } else {
                    window.zoom_window();
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
        if matches!(parent_display, "grid" | "inline-grid") {
            if axes[0] {
                el = el.left(px(0.));
            }
            if axes[1] {
                el = el.top(px(0.));
            }
        } else if !matches!(parent_display, "flex" | "inline-flex") {
            let block = !display.starts_with("inline");
            if block
                && axes[0]
                && let Some(parent) = parent
            {
                let left = padding_left(styles, parent, containing[0]);
                reader.log(Query::FlatParentPadding, depth, hashed(Some(left.to_bits())));
                el = el.left(px(left));
            }
            anchor = Some((axes, block, [margin[3], margin[0]]));
        }
    }
    if stacking {
        el = el.child(renderer::layers(false));
    }
    el = el.children(bars);
    let sticky = (style.get(Prop::POSITION).is_some_and(|v| v == "sticky"))
        .then(|| {
            let host = view.host.borrow();
            let scrolls = &host.dom.get(&view.id)?.scroll;
            let parent = scroller(doc, styles, ancestors.iter().rev().copied(), scrolls);
            let border = insets(styles, parent, false, containing[0]);
            reader.log(
                Query::Scroller,
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
            Some(super::paint::Sticky {
                scroll: scrolls.get(&parent)?.clone(),
                border,
                block,
            })
        })
        .flatten();
    // WARNING: blocks lay out as a stretched flex column, so a replaced element or a control, and a block whose width
    // comes from its ratio and a set height, must not stretch, and they start at the right in right-to-left blocks.
    if !flex_parent
        && slot.is_none()
        && !display.starts_with("inline")
        && el.style().align_self.is_none()
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
    let vertical = style
        .get(Prop::WRITING_MODE)
        .filter(|mode| mode.starts_with("vertical") || mode.starts_with("sideways"))
        .filter(|_| id != 0 && !node.tag.starts_with('#') && slot.is_none());
    let el = match vertical {
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
        placed: Default::default(),
        anchor,
        block: made.absolute.then_some(borders),
        container: positioner.filter(|&block| position == "fixed" || Some(block) != parent),
        sized: positioner
            .filter(|&block| block != 0 && Some(block) != parent)
            .map(|_| containing),
        style: style.clone(),
        effects: effect_layer,
        composited,
        transformed: info[id].flags & cull::TRANSFORMED != 0,
        perspective: parent_perspective.clone().zip(parent),
        units,
        revision: info[id].revision,
        sides: [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ],
        margin,
        cell: slot.as_ref().map(|(bounds, _)| [bounds[2], bounds[3]]),
        grid: slot
            .as_ref()
            .filter(|(_, grid)| grid.baseline || grid.first_item.is_some())
            .and_then(|(_, grid)| Some((node.parent?, grid.baseline))),
        shared: (matches!(node.tag.as_str(), "td" | "th")
            && std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
                .find(|&parent| doc.nodes[parent].tag == "table")
                .is_some_and(|table| {
                    styles.values[table].get(Prop::BORDER_COLLAPSE) == Some("collapse")
                }))
        .then(|| borders.map(|width| width / 2.)),
        boxless: node.tag == "option" && {
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
                    [first, texts.next_back().unwrap_or(first)],
                    [padding[0] + borders[0], padding[2] + borders[2]],
                ))
            })
            .flatten(),
        leading: (display == "inline" && !node.tag.starts_with('#')).then(|| {
            let face = text.resolve_font(&style::font(style));
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
        parts: if node.tag == "table" {
            let mut rows: Vec<(usize, Vec<usize>)> = Vec::new();
            let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
            for &cell in &node.children {
                let Some(row) = doc.nodes[cell]
                    .parent
                    .filter(|&row| row != id && doc.nodes[row].tag == "tr")
                else {
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
            rows.extend(groups);
            rows.into()
        } else {
            Rc::from([])
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
                hooks.sticky.as_ref().map(|sticky| (sticky.border, sticky.block)),
            ),
        )
    )));
    let element = el.hooks(hooks);
    // WARNING: `z-index` does nothing in the top layer, whose boxes paint in their own order.
    if layered
        && let Some(order) = order.filter(|order| *order != 0)
        && !top_layer.contains(&id)
    {
        element.z_index(order).into()
    } else {
        element.into()
    }
}

pub(super) fn pseudo_content(
    doc: &Document,
    id: usize,
    pseudo: &style::Style,
    content: &str,
) -> String {
    let node = &doc.nodes[id];
    let mut text = String::new();
    for part in style::components(content, ' ') {
        if let Some(name) = part.strip_prefix("attr(").and_then(|v| v.strip_suffix(')')) {
            text.push_str(node.attrs.get(name.trim()).map_or("", String::as_str));
        } else if let Some(close) = match part {
            "open-quote" => Some(false),
            "close-quote" => Some(true),
            _ => None,
        } {
            let depth = std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
                .filter(|&parent| doc.nodes[parent].tag == "q")
                .count();
            let quotes: Vec<String> = match pseudo.get(Prop::QUOTES) {
                None | Some("auto") => ["\u{201c}", "\u{201d}", "\u{2018}", "\u{2019}"]
                    .map(str::to_owned)
                    .to_vec(),
                Some(value) => style::components(value, ' ')
                    .into_iter()
                    .filter_map(|part| {
                        let mut input = cssparser::Parser::new(part);
                        input.expect_string().ok().map(|string| string.to_string())
                    })
                    .collect(),
            };
            let pair = (depth * 2).min(quotes.len().saturating_sub(2));
            text.push_str(
                quotes
                    .get(pair + usize::from(close))
                    .map_or("", String::as_str),
            );
        } else if let Some(string) = part
            .strip_prefix(['"', '\''])
            .and_then(|v| v.strip_suffix(['"', '\'']))
        {
            let mut chars = string.chars().peekable();
            while let Some(c) = chars.next() {
                if c != '\\' {
                    text.push(c);
                    continue;
                }
                let mut code = String::new();
                while code.len() < 6
                    && let Some(digit) = chars.next_if(char::is_ascii_hexdigit)
                {
                    code.push(digit);
                }
                if code.is_empty() {
                    text.extend(chars.next());
                    continue;
                }
                chars.next_if_eq(&' ');
                text.push(
                    u32::from_str_radix(&code, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .unwrap_or('\u{fffd}'),
                );
            }
        }
    }
    text
}

/// The ratio of an `aspect-ratio` value, like `5/4`, `1.25` or `auto 5 / 4`.
/// WARNING: taffy passes min and max sizes through the aspect ratio even when the other side is set.
/// CSS does that only for an auto size, so `max-width: 100%` must not limit the height of a sized image.
/// A set limit replaces these in the style loop.
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

pub(crate) fn aspect_ratio(value: &str) -> Option<f32> {
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

pub(super) fn text_field(node: &crate::dom::Node) -> bool {
    node.tag == "textarea"
        || node.tag == "input"
            && node.native_control().is_none()
            && ![
                "checkbox", "radio", "button", "submit", "reset", "image", "file", "color",
                "range", "hidden",
            ]
            .iter()
            .any(|other| node.input_type().eq_ignore_ascii_case(other))
}

// WARNING: an image without a picture stays a replaced element only with both size attributes, which give its
// natural size and ratio, or with neither a source nor alt text, at size 0. An object shows its children unless it
// has neither data nor fallback content. Sizes are in the content box.
pub(super) fn natural_size(
    doc: &Document,
    id: usize,
    decoded: Option<[f32; 2]>,
) -> Option<([f32; 2], Option<f32>)> {
    let node = &doc.nodes[id];
    let attr = |name: &str, default: f32| {
        node.attrs
            .get(name)
            .and_then(|value| dimension(value, node.tag == "canvas"))
            .unwrap_or(default)
    };
    let pictured = |size: [f32; 2]| (size, (size[0] > 0. && size[1] > 0.).then(|| size[0] / size[1]));
    let kind = if node.tag == "input" {
        node.input_type()
    } else {
        ""
    };
    if let Some(native) = node
        .native_control()
        .filter(|native| *native != renderer::ControlKind::Button)
    {
        return Some((super::native::size(native), None));
    }
    // WARNING: a failed image keeps the size of the last picture it decoded, which it no longer shows.
    match (node.tag.as_str(), decoded.filter(|_| !broken_image(node))) {
        ("canvas", _) => Some(pictured([attr("width", 300.), attr("height", 150.)])),
        ("img" | "video", Some(size)) => Some(pictured(size)),
        ("video", None) => Some(([300., 150.], None)),
        // WARNING: like Chrome, a failed image with alt text has no natural size, and one without shows a broken
        // image icon only when it has a source and no alt attribute.
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

// WARNING: like Chrome, an image without a source counts as failed.
pub(super) fn broken_image(node: &crate::dom::Node) -> bool {
    node.tag == "img"
        && (node.state & crate::dom::BROKEN != 0
            || !node.attrs.contains_key("src") && !node.attrs.contains_key("srcset"))
}

// WARNING: one raster per thread and scale, never freed, like a UA resource.
fn paint_broken_icon(origin: renderer::Point<renderer::Pixels>, window: &mut renderer::Window) {
    thread_local! {
        static ICON: RefCell<Option<(u32, Arc<renderer::RenderImage>)>> = const { RefCell::new(None) };
    }
    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><defs><linearGradient id="s" x2="0" y2="1"><stop offset="0" stop-color="#ebf2fd"/><stop offset="1" stop-color="#c3d4f3"/></linearGradient><clipPath id="c"><path d="M.25.25h10l5.5 5.5v10H.25z"/></clipPath></defs><g clip-path="url(#c)"><rect width="16" height="16" fill="url(#s)"/><ellipse cx="6" cy="4.5" rx="2.5" ry="1.25" fill="#fff"/><ellipse cx="7" cy="17" rx="7.5" ry="6.5" fill="#58ae39"/></g><path d="M.25.25h10l5.5 5.5v10H.25z" fill="none" stroke="#999" stroke-width=".5"/><path d="M10.25.25v5.5h5.5" fill="#f2f2f2" stroke="#999" stroke-width=".5"/><path d="M16.5 8 7.5 16.5" stroke="#fff" stroke-width="1.25"/></svg>"##;
    let raster = (16. * window.scale_factor()).ceil() as u32;
    let Some(image) = ICON.with_borrow_mut(|icon| {
        if icon.as_ref().is_none_or(|(size, _)| *size != raster) {
            *icon = super::canvas::svg(SVG, raster, raster, None)
                .ok()
                .map(|image| (raster, image));
        }
        icon.as_ref().map(|(_, image)| image.clone())
    }) else {
        return;
    };
    let bounds = Bounds::new(origin, size(px(16.), px(16.)));
    window.paint_image(bounds, bounds, px(0.), image, false);
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
        let shown = if value.is_empty() {
            node.attrs.get("placeholder").cloned().unwrap_or_default()
        } else {
            value
        };
        let run = style::text_run(style, units, shown.len());
        let lines = text
            .shape_text(
                shown.into(),
                px(units.font.em),
                &[run],
                wrap_width
                    .filter(|_| {
                        textarea && !matches!(style.get(Prop::WHITE_SPACE), Some("pre" | "nowrap"))
                    })
                    .map(px),
                None,
                style::word_break(style),
            )
            .unwrap_or_default();
        return [
            if width {
                lines
                    .iter()
                    .map(|line| f32::from(line.unwrapped_layout.width))
                    .fold(0., f32::max)
                    + 1.
            } else {
                0.
            },
            if textarea {
                lines
                    .iter()
                    .map(|line| 1 + line.wrap_boundaries.len())
                    .sum::<usize>()
                    .max(1) as f32
                    * units.font.lh
            } else {
                units.font.lh
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
        let id = text.resolve_font(&face);
        let scale = pixel_ratio;
        let (char_width, extra) = if "|american typewriter|arial hebrew|chalkboard|cochin|\
            corsiva hebrew|courier|euphemia ucas|geneva|gill sans|hei|helvetica|hoefler text|\
            inaimathi|kai|lucida grande|marker felt|monaco|mshtakan|new peninim mt|osaka|\
            raanana|stheiti|symbol|times|apple braille|apple ligothic|apple lisung|\
            apple symbols|applegothic|applemyungjo|#gungseo|#headlinea|#pcmyungjo|#pilgi|"
            .contains(&format!("|{family}|"))
        {
            let zero = text
                .ch_advance(id, em)
                .map_or(units.font.em / 2., f32::from);
            (zero * scale, 0.)
        } else {
            let average = if face.family.as_ref() == ".SystemUIFont" {
                (f32::from(text.x_height(id, em)) * scale).round()
            } else {
                text.advance(id, em, 'x')
                    .map_or(units.font.em / 2., |size| f32::from(size.width))
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
        if textarea { count("rows", 2) } else { 1. } * units.font.lh,
    ]
}

// WARNING: rounds a size up to layout units, so the content it was measured from doesn't wrap.
fn layout_ceil(size: f32, pixel_ratio: f32) -> f32 {
    (size * pixel_ratio * 64.).ceil() / 64. / pixel_ratio
}

fn in_flow(doc: &Document, info: &[cull::Info], id: usize) -> bool {
    let node = &doc.nodes[id];
    !matches!(node.tag.as_str(), "#comment" | "#doctype")
        && !(node.tag == "#text" && node.text.trim_ascii().is_empty())
        && info[id].flags & cull::OUT_OF_FLOW == 0
}

// WARNING: culled spacers take the margins of their first and last box from here, so these must stay the margins the box is laid out with.
pub(super) fn margins(
    build: &Build<'_>,
    id: usize,
    units: &style::Lengths,
    width: f32,
    flex_parent: bool,
    slot: bool,
) -> [f32; 4] {
    let mut margin = units.edges(&build.styles.values[id], style::MARGIN, width);
    let Some(parent) = build.doc.nodes[id].parent.filter(|_| !flex_parent && !slot) else {
        return margin;
    };
    let walked = build.margins.borrow().0.get(&parent).cloned();
    let walked = walked.unwrap_or_else(|| {
        let collapses = container(build, parent);
        let open =
            [0, 2].map(|side| collapses && margin_collapses_through(build, parent, side, width));
        // WARNING: animated values aren't in the revisions. Fonts, the viewport and container units change these
        // margins without a new revision, so `render_page` clears the kept walks with its measurements.
        let key = (build.info[parent].revision, width.to_bits(), open);
        let animated = build.info[parent].flags & cull::ANIMATED != 0;
        if let Some((old, used)) = build.view.walked.borrow().get(&parent)
            && *old == key
            && !animated
        {
            build.margins.borrow_mut().0.insert(parent, used.clone());
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
                    (leading, pending, emitted) = (false, [0.; 2], 0.);
                    continue;
                }
                let (top, bottom, empty) = adjoining(build, child, width);
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
                for child in children.into_iter().rev() {
                    let empty = block(build, child) && adjoining(build, child, width).2;
                    let Some(sides) = used.get_mut(&child) else {
                        break;
                    };
                    if empty {
                        *sides = [0.; 2];
                        continue;
                    }
                    sides[1] = 0.;
                    break;
                }
            }
        }
        let used = Rc::new(used);
        build.margins.borrow_mut().0.insert(parent, used.clone());
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

fn flow(build: &Build<'_>, id: usize) -> Vec<usize> {
    build.doc.nodes[id]
        .children
        .iter()
        .copied()
        .filter(|&child| {
            in_flow(build.doc, build.info, child)
                && float_side(&build.styles.values[child]).is_none()
        })
        .collect()
}

fn block(build: &Build<'_>, id: usize) -> bool {
    !build.doc.nodes[id].tag.starts_with('#')
        && build.doc.nodes[id].tag != "br"
        && build.styles.values[id]
            .get(Prop::DISPLAY)
            .is_some_and(|value| !value.starts_with("inline") && value != "contents")
        || style::holds_block_box(build.doc, build.styles, id)
}

fn container(build: &Build<'_>, id: usize) -> bool {
    build.styles.values[id]
        .get(Prop::DISPLAY)
        .is_none_or(|value| {
            !value.contains("flex") && !value.contains("grid") && value != "contents"
        })
}

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
    // WARNING: replaced elements and form controls have content of their own, so margins never collapse through
    // them or into their fallback children.
    id != 0
        && doc.nodes[id].sf_symbol().is_none()
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
        )
        && doc.nodes[id]
            .parent
            .is_none_or(|parent| container(build, parent))
        && style
            .get(Prop::DISPLAY)
            .is_some_and(|value| matches!(value, "block" | "list-item"))
        && float_side(style).is_none()
        && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
            .into_iter()
            .all(|key| style.get(key).is_none_or(|value| value == "visible"))
        && style
            .get(Prop::POSITION)
            .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
        && edge(padding) == 0.
        && (edge(border) == 0.
            || style
                .get(kind)
                .is_none_or(|value| matches!(value, "none" | "hidden")))
        && (side == 0
            || style.get(Prop::HEIGHT).is_none_or(|value| value == "auto")
                && style
                    .get(Prop::MIN_HEIGHT)
                    .is_none_or(|value| value == "auto" || edge(Prop::MIN_HEIGHT) == 0.))
}

// WARNING: `None` when `min()` or `max()` inside make the value not linear in the base.
fn percent_calc(units: &style::Lengths, value: &str) -> Option<(f32, f32)> {
    if !value.starts_with("calc(") || !value.contains('%') {
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

fn adjoining(build: &Build<'_>, id: usize, width: f32) -> ([f32; 2], [f32; 2], bool) {
    if let Some(&sets) = build.margins.borrow().1.get(&id) {
        return sets;
    }
    // WARNING: like the kept walks, this keys on the geometry revision, which animated values don't move.
    let key = (build.info[id].revision, width.to_bits());
    let animated = build.info[id].flags & cull::ANIMATED != 0;
    if !animated
        && let Some(&(old, sets)) = build.view.adjoined.borrow().get(&id)
        && old == key
    {
        build.margins.borrow_mut().1.insert(id, sets);
        return sets;
    }
    let (style, units) = (&build.styles.values[id], build.styles.lengths[id]);
    let own = |prop: Prop| {
        let value = style
            .get(prop)
            .and_then(|value| units.resolve(value, width))
            .unwrap_or(0.);
        [value.max(0.), value.min(0.)]
    };
    let (mut top, mut bottom) = (own(Prop::MARGIN_TOP), own(Prop::MARGIN_BOTTOM));
    let children = flow(build, id);
    // WARNING: this recurses into first and last children, so margins stop collapsing past the stack budget.
    let marker = 0u8;
    let deep = (&raw const marker as usize) < STACK_LIMIT.with(|limit| *limit);
    let blocks = container(build, id) && !deep;
    let [first, last] =
        [0, 2].map(|side| margin_collapses_through(build, id, side, width) && blocks);
    for (open, set, order) in [(first, &mut top, false), (last, &mut bottom, true)] {
        if !open {
            continue;
        }
        let mut list: Vec<usize> = children.clone();
        if order {
            list.reverse();
        }
        for child in list {
            if !block(build, child) {
                break;
            }
            let (child_top, child_bottom, empty) = adjoining(build, child, width);
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
        && last
        && style
            .get(Prop::HEIGHT)
            .is_none_or(|value| value == "auto" || units.resolve(value, width) == Some(0.))
        && children
            .iter()
            .all(|&child| block(build, child) && adjoining(build, child, width).2);
    if empty {
        top = [top[0].max(bottom[0]), top[1].min(bottom[1])];
        bottom = top;
    }
    build
        .margins
        .borrow_mut()
        .1
        .insert(id, (top, bottom, empty));
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

type ColumnRule = (f32, renderer::Rgba, renderer::BorderStyle);

fn columns(
    style: &style::Style,
    units: &style::Lengths,
    width: f32,
) -> Option<(usize, f32, Option<ColumnRule>)> {
    let mut count = style
        .get(Prop::COLUMN_COUNT)
        .and_then(|value| value.parse::<usize>().ok());
    let mut column_width = style
        .get(Prop::COLUMN_WIDTH)
        .and_then(|value| units.resolve(value, width));
    for part in style::components(style.get(Prop::COLUMNS).unwrap_or(""), ' ') {
        if let Ok(value) = part.parse::<usize>() {
            count = count.or(Some(value));
        } else if let Some(value) = units.resolve(part, width) {
            column_width = column_width.or(Some(value));
        }
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
    let (mut rule_width, mut rule_style, mut rule_color) = (
        style.get(Prop::COLUMN_RULE_WIDTH),
        style.get(Prop::COLUMN_RULE_STYLE),
        style.get(Prop::COLUMN_RULE_COLOR),
    );
    for part in style::components(style.get(Prop::COLUMN_RULE).unwrap_or(""), ' ') {
        if matches!(
            part,
            "none"
                | "hidden"
                | "dotted"
                | "dashed"
                | "solid"
                | "double"
                | "groove"
                | "ridge"
                | "inset"
                | "outset"
        ) {
            rule_style = rule_style.or(Some(part));
        } else if style::color(part).is_some() {
            rule_color = rule_color.or(Some(part));
        } else {
            rule_width = rule_width.or(Some(part));
        }
    }
    let rule = rule_style
        .filter(|kind| !matches!(*kind, "none" | "hidden"))
        .map(|kind| {
            let width = rule_width
                .map_or(Some(3.), |value| units.resolve(value, width))
                .unwrap_or(0.);
            let color = rule_color
                .or_else(|| style.get(Prop::COLOR))
                .and_then(style::color)
                .map_or(renderer::black(), |(ink, alpha)| {
                    rgba(ink << 8 | alpha as u32)
                });
            (width, color, border_style(kind))
        })
        .filter(|(width, ..)| *width > 0.);
    Some((count, gap, rule))
}

fn column_rules(
    count: usize,
    gap: f32,
    width: f32,
    color: renderer::Rgba,
    kind: renderer::BorderStyle,
) -> Child {
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            let column = (f32::from(bounds.size.width) - gap * (count - 1) as f32) / count as f32;
            for index in 1..count {
                let x = bounds.left() + px(index as f32 * (column + gap) - gap / 2. - width / 2.);
                window.paint_quad(renderer::quad(
                    Bounds::new(point(x, bounds.top()), size(px(width), bounds.size.height)),
                    renderer::Corners::default(),
                    renderer::transparent_black(),
                    renderer::Edges {
                        left: px(width),
                        ..Default::default()
                    },
                    color,
                    kind,
                ));
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
    colors: [renderer::Rgba; 4],
    kinds: [&str; 4],
    view: Rc<super::PageState>,
) -> Child {
    let kinds = kinds.map(str::to_owned);
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
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
                Bounds::from_corners(point(left, top), point(right, top + px(widths[0]))),
                Bounds::from_corners(point(right - px(widths[1]), top), point(right, bottom)),
                Bounds::from_corners(point(left, bottom - px(widths[2])), point(right, bottom)),
                Bounds::from_corners(point(left, top), point(left + px(widths[3]), bottom)),
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
                        Bounds::from_corners(point(rect.left(), start), point(rect.right(), end))
                    } else {
                        Bounds::from_corners(point(start, rect.top()), point(end, rect.bottom()))
                    };
                    let pieces = match gap.filter(|_| side == 0) {
                        Some([from, to]) => vec![
                            Bounds::from_corners(
                                part.origin,
                                point(from.max(part.left()), part.bottom()),
                            ),
                            Bounds::from_corners(
                                point(to.min(part.right()), part.top()),
                                part.bottom_right(),
                            ),
                        ],
                        None => vec![part],
                    };
                    for piece in pieces.into_iter().filter(|piece| piece.size.width > px(0.)) {
                        window.paint_quad(renderer::fill(piece, color));
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

fn border_style(kind: &str) -> renderer::BorderStyle {
    match kind {
        "dashed" => renderer::BorderStyle::Dashed,
        "dotted" => renderer::BorderStyle::Dotted,
        "double" => renderer::BorderStyle::Double,
        _ => renderer::BorderStyle::Solid,
    }
}

fn shade_border(color: renderer::Rgba, dark: bool) -> renderer::Rgba {
    let rgba = color;
    let value = rgba.r.max(rgba.g).max(rgba.b);
    if value <= 0. {
        let gray = if dark { 0.33 } else { 0.66 };
        return renderer::Rgba {
            r: gray,
            g: gray,
            b: gray,
            a: rgba.a,
        };
    }
    let end = if dark { 0. } else { 1. };
    let distance = (rgba.r - end).powi(2) + (rgba.g - end).powi(2) + (rgba.b - end).powi(2);
    if distance <= 3. * (if dark { 32. } else { 20. } / 255.0f32).powi(2) {
        return color;
    }
    let factor = if dark {
        ((value - 0.33) / value).max(0.)
    } else {
        (value + 0.33).min(1.) / value
    };
    renderer::Rgba {
        r: (rgba.r * factor).min(1.),
        g: (rgba.g * factor).min(1.),
        b: (rgba.b * factor).min(1.),
        a: rgba.a,
    }
}

fn inset_shadows(
    shadows: Vec<renderer::BoxShadow>,
    radius: renderer::Radii<renderer::Length>,
    borders: [f32; 4],
) -> Child {
    renderer::canvas(
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
            window.paint_inset_shadows(bounds, radii, &shadows)
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into()
}

fn svg_layer(id: usize, inline: bool, view: Rc<super::PageState>) -> Child {
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
                    // WARNING: paint draws the tree parsed here, so a changed source is parsed once.
                    // WARNING: a draw that repeats skips paint, so a tree of the old source can still wait here.
                    state.svg_trees.remove(&id);
                    let (size, content, boxes) = match state.svg_cache.get(source) {
                        Some((geometry, _)) => geometry.clone(),
                        None => {
                            let tree = super::canvas::svg_tree(source).ok();
                            let geometry = tree
                                .as_ref()
                                .map(super::canvas::svg_boxes)
                                .unwrap_or_default();
                            if let Some(tree) = tree {
                                state.svg_trees.insert(id, tree);
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
                    if let Some(kept) = geometry
                        .svgs
                        .borrow_mut()
                        .get_mut(&id)
                        .filter(|kept| kept.2 == *source)
                    {
                        kept.3 = entry.clone();
                    }
                }
                let [scale_x, scale_y] =
                    [0, 1].map(|axis| [width, height][axis] / entry.2[axis].max(f32::EPSILON));
                for &(node, [x, y, w, h], _) in &entry.3 {
                    let rect = window.transformed_bounds(Bounds::new(
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
            let raster =
                [width, height].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
            let view = &*view;
            let mut host = view.host.borrow_mut();
            let state = host.dom.entry(view.id).or_default();
            let source = if inline {
                state.svg_sources.get(&id).map(String::as_str)
            } else {
                state.svg_images.get(&id).map(|(source, _)| &**source)
            };
            let Some(source) = source else { return };
            if state.svgs.get(&id).is_none_or(|(old, image)| {
                !inline && old != source
                    || u32::from(image.size().width) != raster[0]
                    || u32::from(image.size().height) != raster[1]
            }) {
                let shared = state
                    .svg_cache
                    .get(source)
                    .filter(|_| inline)
                    .and_then(|(_, images)| {
                        images.iter().find(|image| {
                            [
                                u32::from(image.size().width),
                                u32::from(image.size().height),
                            ] == raster
                        })
                    })
                    .cloned();
                let image = match (&shared, state.svg_trees.remove(&id).filter(|_| inline)) {
                    (Some(image), _) => Ok(image.clone()),
                    (_, Some(tree)) => super::canvas::render_svg(&tree, raster[0], raster[1], None),
                    _ => super::canvas::svg(source, raster[0], raster[1], None),
                };
                match image {
                    Ok(image) => {
                        // WARNING: only icon sizes stay, so at most 256 of them hold under 10 MB.
                        if inline && shared.is_none() && raster[0] <= 96 && raster[1] <= 96 {
                            if let Some((_, images)) = state.svg_cache.get_mut(source) {
                                images.push(image.clone());
                            }
                            if state.svg_cache.len() > 256 {
                                for image in state.svg_cache.drain().flat_map(|(_, entry)| entry.1)
                                {
                                    if Arc::strong_count(&image) == 1 {
                                        window.drop_image(image);
                                    }
                                }
                            }
                        }
                        if let Some((_, old)) = state.svgs.insert(id, (source.to_owned(), image))
                            && Arc::strong_count(&old) == 1
                        {
                            window.drop_image(old);
                        }
                    }
                    Err(error) => eprintln!("apps: SVG: {error}"),
                }
            }
            let image = state.svgs.get(&id).map(|(_, image)| image.clone());
            drop(host);
            if let Some(image) = image {
                window.paint_image(bounds, bounds, renderer::Corners::default(), image, false);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
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
