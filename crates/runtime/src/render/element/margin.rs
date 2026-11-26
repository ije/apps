use super::super::layout::cull;
use super::STACK_LIMIT;
use super::{Build, Sets};
use crate::css;
use crate::css::properties::Prop;
use crate::{
    dom::Document,
    style::{self, ComputedStyles},
};
use renderer::point;
use std::rc::Rc;

/// Whether an inline box has padding, a border or a margin on a side.
pub(super) fn edge(build: &Build<'_>, id: usize, side: usize) -> bool {
    let (style, units) = (&build.styles.values[id], &build.styles.lengths[id]);
    units.edges(style, css::properties::PADDING, 0.)[side]
        + units.edges(style, css::properties::MARGIN, 0.)[side]
        + units.borders(style, 0.)[side]
        != 0.
}

/// The spliced inline boxes with a part in the run `[start, end)` of `children`, with their `CUT_*` flags.
pub(super) fn run_cuts(
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
            (u8::from(!before.iter().any(content)) * crate::render::inline::CUT_FIRST)
                | (u8::from(!after.iter().any(content)) * crate::render::inline::CUT_LAST)
        // Like Chrome, a box that starts or ends with a block has an empty part in the line next to it.
        } else if to == start && to > from && blocky(children[to - 1]) && edge(cut, 1) {
            crate::render::inline::CUT_LAST | crate::render::inline::CUT_BEFORE
        } else if from == end && from < to && blocky(children[from]) && edge(cut, 3) {
            crate::render::inline::CUT_FIRST | crate::render::inline::CUT_AFTER
        } else {
            continue;
        };
        parts.push((cut, flags));
    }
    parts
}

pub(super) fn in_flow(doc: &Document, info: &[cull::Info], id: usize) -> bool {
    let node = &doc.nodes[id];
    !matches!(node.tag.as_str(), "#comment" | "#doctype")
        && !(node.tag == "#text" && node.text.trim_ascii().is_empty())
        && info[id].flags & cull::OUT_OF_FLOW == 0
}

// Keep these margins equal to layout margins: culled spacers use them.
pub(super) fn margins(
    build: &Build<'_>,
    id: usize,
    units: &css::value::units::Lengths,
    width: f32,
    flex_parent: bool,
    slot: bool,
) -> [f32; 4] {
    let mut margin = units.edges(&build.styles.values[id], css::properties::MARGIN, width);
    // The blocks of a spliced inline box are in the flow of its block.
    let parent = std::iter::successors(build.doc.nodes[id].parent, |&up| {
        crate::render::inline::spliced(build.doc, build.styles, up)
            .then(|| build.doc.nodes[up].parent)
            .flatten()
    })
    .find(|&up| !crate::render::inline::spliced(build.doc, build.styles, up));
    let Some(parent) = parent.filter(|_| !flex_parent && !slot) else {
        return margin;
    };
    let walked = build.walks.borrow().get(&parent).cloned();
    let walked = walked.unwrap_or_else(|| {
        let collapses = container(build, parent);
        let mut open =
            [0, 2].map(|side| collapses && margin_collapses_through(build, parent, side, width));
        // Like Chrome, a content height is without the last margin, also where it doesn't pass on.
        let sized = |prop| {
            build.styles.values[parent]
                .get(prop)
                .is_some_and(css::value::content_size)
        };
        open[1] |= collapses
            && [Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT]
                .into_iter()
                .any(sized)
            && margin_collapses_through(build, parent, 3, width);
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
            // The box trims the margins that adjoin its edges.
            let style = &build.styles.values[parent];
            let mut leading = open[0] || trims(style, 0);
            for &child in &children {
                if !block(build, child) {
                    // Like Chrome, margins collapse across inline content that makes no line.
                    if !crate::render::inline::makes_no_line(build.doc, build.styles, child) {
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
            if open[1] || trims(style, 2) {
                let mut trailing = Vec::new();
                for child in children.into_iter().rev() {
                    if !block(build, child)
                        && crate::render::inline::makes_no_line(build.doc, build.styles, child)
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
pub(super) fn inline_fade(
    doc: &Document,
    styles: &style::ComputedStyles,
    child: usize,
    id: usize,
) -> f32 {
    std::iter::successors(doc.nodes.get(child).and_then(|node| node.parent), |&up| {
        doc.nodes[up].parent
    })
    .take_while(|&up| up != id)
    .filter(|&up| {
        !doc.nodes[up].tag.starts_with('#')
            && styles.values[up].get(Prop::DISPLAY) != Some("contents")
    })
    .map(|up| super::super::effects::opacity(&styles.values[up]))
    .product()
}

pub(super) fn flow(build: &Build<'_>, id: usize) -> Vec<usize> {
    let mut flow = Vec::new();
    let mut pending: Vec<usize> = build.doc.nodes[id].children.iter().rev().copied().collect();
    while let Some(child) = pending.pop() {
        if child > usize::MAX / 2 {
            flow.push(!child);
        // A plain block is an element in the flow that doesn't float.
        } else if build.info[child].flags & cull::PLAIN_BLOCK != 0 {
            flow.push(child);
        } else if crate::render::inline::spliced(build.doc, build.styles, child) {
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
                && !crate::render::inline::makes_no_line(build.doc, build.styles, child))
            && float_side(&build.styles.values[child]).is_none()
        {
            flow.push(child);
        }
    }
    flow
}

pub(super) fn block(build: &Build<'_>, id: usize) -> bool {
    !build.doc.nodes[id].tag.starts_with('#')
        && build.doc.nodes[id].tag != "br"
        && build.styles.values[id]
            .get(Prop::DISPLAY)
            .is_some_and(|value| !value.starts_with("inline") && value != "contents")
        || crate::render::inline::holds_block_box(build.doc, build.styles, id)
            && !crate::render::inline::spliced(build.doc, build.styles, id)
}

pub(super) fn container(build: &Build<'_>, id: usize) -> bool {
    build.styles.values[id]
        .get(Prop::DISPLAY)
        .is_none_or(|value| {
            !value.contains("flex") && !value.contains("grid") && value != "contents"
        })
}

// The overflow of the body goes to the viewport when the html box has none, so the body itself doesn't clip.
// The viewport takes the overflow of the root, or of the body when the root has none.
pub(crate) fn viewport_overflow(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    doc.nodes[id].tag == "html" && doc.nodes[id].parent == Some(0)
        || body_overflow_moves(doc, styles, id)
}

pub(super) fn body_overflow_moves(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
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

/// Whether `margin-trim` drops the margins at the top, side 0, or the bottom of a block.
pub(crate) fn trims(style: &css::properties::Style, side: usize) -> bool {
    match style.get(Prop::MARGIN_TRIM) {
        Some("block") => true,
        Some("block-start") => side == 0,
        Some("block-end") => side != 0,
        _ => false,
    }
}

// Whether the first line of a list item holds only its inside marker, because a block follows it.
pub(super) fn marker_line(build: &Build<'_>, id: usize) -> bool {
    crate::render::layout::size::inside_marker(build.styles, id)
        && flow(build, id)
            .first()
            .is_some_and(|&child| block(build, child))
}

// `side` 3 is the bottom edge of a box that margins collapse through, which can also have a zero height.
pub(super) fn margin_collapses_through(
    build: &Build<'_>,
    id: usize,
    side: usize,
    width: f32,
) -> bool {
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
        && !(side == 0 && marker_line(build, id))
        && !trims(style, side)
        // A pseudo-element in the flow, like a clearfix, holds the margin of its side.
        && styles.generated(id).is_none_or(|pseudos| {
            let pseudo = &pseudos[side / 2];
            crate::render::inline::drawn(pseudo, style).is_none()
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
            || (side == 3
                || style.get(Prop::HEIGHT).is_none_or(css::value::auto_size)
                    && (style.get(Prop::MAX_HEIGHT)).is_none_or(|value| {
                        // Like Chrome, only a `calc-size()` that can cut its content holds the last margin.
                        css::value::calc_size(value).is_none_or(|(basis, size)| {
                            css::value::auto_size(basis)
                                && [0f32, 1e6].into_iter().all(|at| {
                                    let calc = css::value::with_size(size, &format!("({at}px)"));
                                    (styles.lengths[id].resolve(&format!("calc({calc})"), width))
                                        .is_some_and(|got| got >= at)
                                })
                        })
                    }))
                && style
                    .get(Prop::MIN_HEIGHT)
                    .is_none_or(|value| value == "auto" || edge(Prop::MIN_HEIGHT) == 0.))
}

// Whether the block around the line of a box has a definite height, which its percent heights resolve against.
pub(super) fn definite_line(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    let block =
        std::iter::successors(doc.nodes[id].parent, |&node| doc.nodes[node].parent).find(|&node| {
            styles.values[node]
                .get(Prop::DISPLAY)
                .is_some_and(|value| !matches!(value, "inline" | "contents"))
        });
    definite_height(doc, styles, block)
}

pub(super) fn definite_height(doc: &Document, styles: &ComputedStyles, id: Option<usize>) -> bool {
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
pub(super) fn shrinks_around(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
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

pub(super) fn percent_calc(units: &css::value::units::Lengths, value: &str) -> Option<(f32, f32)> {
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
pub(super) fn percent_bound(
    units: &css::value::units::Lengths,
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
        units: &'a css::value::units::Lengths,
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
pub(super) fn adjoining(
    build: &Build<'_>,
    id: usize,
    width: f32,
    around: Option<[f32; 2]>,
) -> Sets {
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
                if crate::render::inline::makes_no_line(build.doc, build.styles, child) {
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
        && style.get(Prop::HEIGHT).is_none_or(|value| {
            css::value::auto_size(value) || units.resolve(value, width) == Some(0.)
        })
        && children.iter().all(|&child| {
            if block(build, child) {
                adjoining(build, child, width, around).2
            } else {
                crate::render::inline::makes_no_line(build.doc, build.styles, child)
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

pub(crate) fn overscroll(style: &css::properties::Style) -> renderer::Point<renderer::Overscroll> {
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

pub(super) fn clear_bits(style: &css::properties::Style) -> u8 {
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

pub(super) fn float_side(style: &css::properties::Style) -> Option<bool> {
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    match style.get(Prop::FLOAT) {
        Some("left") => Some(false),
        Some("right") => Some(true),
        Some("inline-start") => Some(rtl),
        Some("inline-end") => Some(!rtl),
        _ => None,
    }
}
