use super::Build;
use super::kept::{Query, hashed};
use super::margin::{float_side, viewport_overflow};
use crate::css;
use crate::css::properties::Prop;
use crate::{
    dom::Document,
    style::{self, ComputedStyles},
};
use renderer::{Color, px};
use std::{rc::Rc, sync::Arc};

pub(super) fn parent_style<'b>(
    doc: &Document,
    styles: &'b ComputedStyles,
    id: usize,
    prop: Prop,
) -> Option<&'b str> {
    box_parent(doc, styles, id).and_then(|parent| match (parent, prop) {
        (0, Prop::DIRECTION) => page_direction(doc, styles),
        _ => styles.values[parent].get(prop),
    })
}

// Like in Chrome, the viewport takes the direction of the body, or of the root when there is no body.
pub(crate) fn page_direction<'b>(doc: &Document, styles: &'b ComputedStyles) -> Option<&'b str> {
    let child = |parent: usize, tag: &str| {
        (doc.nodes[parent].children.iter().copied()).find(|&id| doc.nodes[id].tag == tag)
    };
    let html = child(0, "html")?;
    styles.values[child(html, "body").unwrap_or(html)].get(Prop::DIRECTION)
}

pub(super) fn rtl(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    let direction = match id {
        0 => page_direction(doc, styles),
        _ => styles.values[id].get(Prop::DIRECTION),
    };
    direction == Some("rtl")
}

pub(super) fn row_fill(doc: &Document, styles: &ComputedStyles, id: usize) -> Option<Color> {
    let fill = |node: usize| {
        styles.values[node]
            .get(Prop::BACKGROUND_COLOR)
            .and_then(css::value::color::paint)
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
    // Columns paint under the cells, after rows.
    let table = table?;
    let known = styles.columns.borrow().get(&table).cloned();
    let filled = known.unwrap_or_else(|| {
        // The columns of each `<col>` or group with a fill. Those of a `<col>` come before those of its group.
        let mut cols = Vec::new();
        let mut at = 0;
        for &group in &doc.nodes[table].children {
            if doc.nodes[group].tag != "colgroup" {
                continue;
            }
            let start = at;
            for &col in &doc.nodes[group].children {
                if doc.nodes[col].tag == "col" {
                    let end = at + style::span(&doc.nodes[col], "span");
                    cols.push((col, at..end));
                    at = end;
                }
            }
            if at == start {
                at += style::span(&doc.nodes[group], "span");
            }
            cols.push((group, start..at));
        }
        cols.retain(|(node, _)| fill(*node).is_some());
        let mut filled = rustc_hash::FxHashMap::default();
        if !cols.is_empty() {
            // Rows have no box, so the cells of all rows are the children of their group here.
            let (mut slots, mut index, mut row) = (style::Slots::default(), 0, None);
            for group in std::iter::once(table).chain(doc.nodes[table].children.iter().copied()) {
                for &child in &doc.nodes[group].children {
                    if style::role(styles, child) != style::CELL {
                        continue;
                    }
                    let next = doc.nodes[child].parent;
                    if row.replace(next).is_some_and(|row| row != next) {
                        index += 1;
                    }
                    let span = |name| style::span(&doc.nodes[child], name);
                    let column = slots.place(index, span("colspan"), span("rowspan"));
                    if let Some((col, _)) = cols.iter().find(|(_, range)| range.contains(&column)) {
                        filled.insert(child, *col);
                    }
                }
                // A cell ends with its row group.
                slots = Default::default();
                row = None;
                index = 0;
            }
        }
        let filled = std::rc::Rc::new(filled);
        (styles.columns.borrow_mut()).insert(table, filled.clone());
        filled
    });
    filled.get(&id).copied().and_then(fill)
}

pub(super) fn collapses(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    std::iter::successors(doc.nodes[id].parent, |&parent| doc.nodes[parent].parent)
        .find(|&parent| style::table_box(style::role(styles, parent)))
        .is_some_and(|table| styles.values[table].get(Prop::BORDER_COLLAPSE) == Some("collapse"))
}

pub(super) fn parent_font(
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

pub(crate) fn box_parent(doc: &Document, styles: &ComputedStyles, id: usize) -> Option<usize> {
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
pub(super) fn padding_start(styles: &ComputedStyles, parent: usize, width: f32) -> (f32, bool) {
    let style = &styles.values[parent];
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    let padding = styles.lengths[parent].edges(style, css::properties::PADDING, width);
    (padding[if rtl { 1 } else { 3 }].max(0.), rtl)
}

pub(super) fn starts_line(build: &Build<'_>, id: usize) -> bool {
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

pub(super) fn insets(styles: &ComputedStyles, node: usize, padding: bool, width: f32) -> [f32; 4] {
    let (style, units) = (&styles.values[node], &styles.lengths[node]);
    let borders = units.borders(style, width);
    if !padding {
        return borders;
    }
    let padding = units.edges(style, css::properties::PADDING, width);
    std::array::from_fn(|side| padding[side] + borders[side])
}

pub(super) fn backdrop(doc: &Document, styles: &ComputedStyles, id: usize) -> Option<(usize, u32)> {
    std::iter::successors(Some(id), |&node| doc.nodes[node].parent).find_map(|node| {
        let (ink, alpha) =
            css::value::color::color(styles.values[node].get(Prop::BACKGROUND_COLOR)?)?;
        (alpha >= 128).then_some((node, ink))
    })
}

pub(super) const LOOK: u32 =
    crate::css::selector::HOVER | crate::css::selector::ACTIVE | crate::css::selector::FOCUS;

pub(super) fn boxless(doc: &Document, id: usize) -> Option<(usize, bool)> {
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

pub(super) fn content_editable(doc: &Document, id: usize) -> Option<usize> {
    std::iter::successors(Some(id), |&node| doc.nodes[node].parent)
        .find(|&node| doc.nodes[node].attrs.contains_key("contenteditable"))
}

pub(super) fn shrinks(
    doc: &Document,
    styles: &ComputedStyles,
    id: usize,
    first: bool,
) -> (bool, usize) {
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

pub(super) fn uses(
    doc: &Document,
    styles: &ComputedStyles,
    ids: &super::super::effects::Ids,
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
                super::super::effects::index(ids, doc)
                    .get(name)
                    .into_iter()
                    .flatten()
                    .map(|&target| (target, true)),
            );
        }
    }
    found.then(|| hasher.finish())
}

pub(super) fn href(node: &crate::dom::Node) -> Option<&str> {
    node.attrs
        .get("href")
        .or_else(|| node.attrs.get("xlink:href"))
        .and_then(|value| value.strip_prefix('#'))
}

pub(super) fn scrolled(handle: Option<&renderer::ScrollRef>) -> Option<[bool; 2]> {
    handle.map(renderer::ScrollRef::bars)
}

pub(super) fn pictured(
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

pub(super) fn baselines(
    doc: &Document,
    dom: Option<&crate::render::PageState>,
    grid: usize,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    for child in &doc.nodes[grid].children {
        dom.and_then(|dom| dom.layout.baselines.get(child))
            .map(|baselines| baselines.map(|value| value.map(f32::to_bits)))
            .hash(&mut hasher);
    }
    hasher.finish()
}

// The axes, 1 for x and 2 for y, that the insets of an out-of-flow box set.
pub(super) fn inset_axes(style: &css::properties::Style) -> u8 {
    [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]]
        .iter()
        .enumerate()
        .filter(|(_, axis)| {
            axis.iter()
                .any(|&prop| style.get(prop).is_some_and(|value| value != "auto"))
        })
        .fold(0, |axes, (axis, _)| axes | 1 << axis)
}

// An empty box at the static place of an out-of-flow box, which layout moves there.
pub(super) fn static_mark(key: usize, drop: f32, inline: bool) -> renderer::Node {
    let mut mark = renderer::node().w(px(0.)).h(px(0.));
    mark.style().mark = Some(renderer::Mark {
        key: key as u64,
        drop: px(drop),
        inline,
    });
    mark
}

pub(crate) fn positioner(
    position: &str,
    mut ancestors: impl Iterator<Item = usize>,
    mut made: impl FnMut(usize) -> super::super::effects::Establishes,
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
pub(super) fn scroller(
    doc: &Document,
    styles: &ComputedStyles,
    mut ancestors: impl Iterator<Item = usize>,
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
                && !viewport_overflow(doc, styles, ancestor)
        })
        .unwrap_or(0)
}

// Past an absolute (1) or fixed (2) box, only its containing block chain scrolls it. Returns `None` when the walk
// skips `node`, else the state after it.
pub(super) fn escape_step(styles: &ComputedStyles, node: usize, escape: u8) -> Option<u8> {
    let style = &styles.values[node];
    if escape != 0 {
        let made = super::super::effects::establishes(style);
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
pub(super) fn passed(styles: &ComputedStyles, id: usize, query: Query, below: bool) -> Query {
    match query {
        Query::Scroller(escape) if below => {
            Query::Scroller(escape_step(styles, id, escape).unwrap_or(escape))
        }
        query => query,
    }
}

// The indents of the rows of a text in this width.
pub(crate) fn row_indents(
    style: &css::properties::Style,
    units: &css::value::units::Lengths,
    width: f32,
) -> renderer::Indent {
    let Some(value) = style.get(Prop::TEXT_INDENT) else {
        return Default::default();
    };
    let has = |word| value.split(' ').any(|part| part == word);
    let (indent, percent) = text_indent(style, units);
    renderer::TextStyle {
        text_indent: renderer::px(indent),
        text_indent_percent: percent,
        text_indent_rows: [has("hanging"), has("each-line")],
        ..Default::default()
    }
    .indent(renderer::px(width))
}

// `text-indent` as pixels and a fraction of the width of the paragraph, which the renderer resolves at layout.
pub(super) fn text_indent(
    style: &css::properties::Style,
    units: &css::value::units::Lengths,
) -> (f32, f32) {
    let Some(value) = style.get(Prop::TEXT_INDENT) else {
        return (0., 0.);
    };
    let value = value.split(" hanging").next().unwrap_or(value);
    let value = value.split(" each-line").next().unwrap_or(value);
    let pixels = units.resolve(value, 0.).unwrap_or(0.);
    match value.contains('%') {
        true => (
            pixels,
            (units.resolve(value, 100.).unwrap_or(pixels) - pixels) / 100.,
        ),
        false => (pixels, 0.),
    }
}

// Taffy applies min/max constraints through aspect ratio even with an explicit opposite size. Prevent
// max-width from changing explicit image heights.
pub(super) fn only_auto_ratio(
    mut el: renderer::Node,
    style: &css::properties::Style,
) -> renderer::Node {
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
