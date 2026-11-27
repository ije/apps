use crate::css;
use crate::css::properties::Prop;
use crate::dom::Document;
use crate::render::PageState;
use crate::style;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub(crate) fn layout_flags(doc: &mut Document, state: &mut PageState, viewport: [f32; 2]) {
    let computed = state.restyle(doc, viewport);
    state
        .geometry
        .style_flags(computed.values.iter(), state.styles.epoch, |style| {
            let clip = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                .into_iter()
                .any(|prop| {
                    style
                        .get(prop)
                        .is_some_and(|value| matches!(value, "hidden" | "clip" | "auto" | "scroll"))
                });
            let position = style.get(Prop::POSITION).unwrap_or("static");
            let positioned = crate::render::effects::establishes(style).absolute;
            [clip, positioned, position == "fixed"]
        });
}

pub(super) fn scroll_events(state: &mut PageState) -> Result<Value> {
    let mut events = json!(state.geometry.scroll_events());
    let skips = state.layout.take_skip_events();
    if !skips.is_empty()
        && let Some(list) = events.as_array_mut()
    {
        list.push(json!({ "skips": skips }));
    }
    Ok(events)
}

pub(super) fn hit_test(
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    ensure!(!doc.nodes.is_empty(), "Document is unavailable");
    let x = args[0].as_f64().context("Expected x")? as f32;
    let y = args[1].as_f64().context("Expected y")? as f32;
    let computed = state.restyle(doc, viewport);
    use crate::render::hit::Hit;
    let mode = match (args[2].as_bool(), args[3].as_bool()) {
        (_, Some(true)) => Hit::Caret,
        (Some(true), _) => Hit::First,
        _ => Hit::All,
    };
    let hits = match crate::render::transition::covered(doc, state).filter(|_| mode != Hit::Caret) {
        Some(root) => vec![root],
        None => crate::render::hit::hit_test(doc, state, &computed, viewport, [x, y], mode),
    };
    Ok(match mode {
        Hit::All => json!(hits),
        _ => json!(hits.first()),
    })
}

// A box that doesn't snap, with no scroll offset, and none to come, is at 0 after any layout.
pub(super) fn scrolled(id: usize, state: &mut PageState) -> Result<Value> {
    Ok(json!(state.geometry.scroll.get(&id).is_some_and(
        |handle| {
            handle.snaps()
                || handle.offset() != renderer::Point::default()
                || handle.target() != renderer::Point::default()
        }
    )))
}

pub(super) fn scroll(
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let page = doc.nodes[id].parent == Some(0) && doc.nodes[id].tag == "html";
    if args[1].is_null() && args[2].is_null() {
        let handle = state.geometry.scroll.get(&if page { 0 } else { id });
        if !handle.is_some_and(renderer::ScrollRef::snaps) {
            return Ok(Value::Null);
        }
    }
    // As in Chrome, a box that can't scroll keeps its offset at 0.
    if !page && !state.geometry.scroll.contains_key(&id) {
        let computed = state.restyle(doc, viewport);
        let style = computed.values.get(id);
        let scrolls = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
            .into_iter()
            .any(|prop| {
                style
                    .and_then(|style| style.get(prop))
                    .is_some_and(|value| !matches!(value, "visible" | "clip"))
            });
        let boxed = style
            .and_then(|style| style.get(Prop::DISPLAY))
            .is_some_and(|value| !matches!(value, "inline" | "contents" | "none"));
        if doc.nodes[id].tag.starts_with('#')
            || !scrolls
            || !boxed
            || crate::render::viewport_overflow(doc, &computed, id)
        {
            return Ok(json!([0., 0.]));
        }
        if crate::render::layout::size::turned(doc, &computed, id).is_some() {
            state.geometry.turned.insert(id);
        }
    }
    // A box that lays out turned clockwise scrolls left on the axis of its top.
    let turned = !page && state.geometry.turned.contains(&id);
    let [left, top] = [1, 2].map(|axis| args[axis].as_f64());
    let [left, top] = match turned {
        true => [top, left.map(|left| -left)],
        false => [left, top],
    };
    let handle = state
        .geometry
        .scroll
        .entry(if page { 0 } else { id })
        .or_default()
        .clone();
    let previous = handle.offset();
    let ratio = state.pixel_ratio.max(1.);
    let axis = |value: Option<f64>, current: renderer::Pixels| {
        renderer::px(value.map_or(f32::from(current), |v| -(v as f32 * ratio).round() / ratio))
    };
    let target = renderer::point(axis(left, previous.x), axis(top, previous.y));
    let snap = match args[4].as_str() {
        Some("by") => renderer::Snap::From(previous),
        Some(_) => renderer::Snap::To([true; 2]),
        None => renderer::Snap::To([left.is_some(), top.is_some()]),
    };
    // A script can turn snapping off and scroll before the next layout.
    let snaps = handle.snaps()
        && (state.restyle(doc, viewport).values[id].get(Prop::SCROLL_SNAP_TYPE))
            .is_some_and(|kind| kind != "none");
    let target = match snaps {
        true => handle.snap(target, snap),
        false => target,
    };
    let target = crate::dom::geometry::scroll_offset(&handle, target);
    if target != handle.target() {
        let smooth = handle.extent() != renderer::Point::default()
            && match args[3].as_str() {
                Some("smooth") => true,
                Some("instant") => false,
                _ => {
                    state.restyle(doc, viewport).values[id].get(Prop::SCROLL_BEHAVIOR)
                        == Some("smooth")
                }
            };
        if smooth {
            handle.glide_to(target);
        } else {
            handle.jump(target);
        }
    }
    state.dirty |= target != previous;
    state.geometry.sync_scroll();
    let [left, top] = [-target.x, -target.y].map(f32::from);
    Ok(json!(if turned { [-top, left] } else { [left, top] }))
}

pub(super) fn fragments(id: usize, state: &mut PageState) -> Result<Value> {
    Ok(state
        .geometry
        .fragments
        .get(&id)
        .map_or(Value::Null, |parts| json!(parts)))
}

// The far edge of the scrollable overflow of a box that doesn't scroll, from `start`, or from the left of `right`.
// As in Chrome, it holds the boxes in it and the boxes in those that don't clip, but not the out-of-flow boxes whose
// containing block is outside it.
pub(super) fn overflow(
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let axis = usize::from(args[1].as_u64() == Some(1));
    let start = args[2].as_f64().context("Expected a start")? as f32;
    let right = args[3].as_f64().map(|right| right as f32);
    let styles = state.restyle(doc, viewport);
    let mirror = &state.geometry.mirror;
    let flat = !styles.children.is_empty();
    let kids = |id: usize| {
        (flat.then(|| styles.children.get(&id)).flatten()).unwrap_or(&doc.nodes[id].children)
    };
    let own = crate::render::effects::establishes(&styles.values[id]);
    let mut pending: Vec<_> = (kids(id).iter())
        .map(|&kid| (kid, own.absolute, own.fixed))
        .collect();
    // What the walk reads of a style: 1 absolute, 2 fixed, 4 hidden, 8 clips, 16 and 32 the containing blocks it makes,
    // and 64 inline, whose lines hold its text.
    let mut kinds = rustc_hash::FxHashMap::default();
    let mut end = 0f32;
    while let Some((kid, absolute, fixed)) = pending.pop() {
        let Some(style) = styles.values.get(kid) else {
            continue;
        };
        let text = doc.nodes[kid].tag.starts_with('#');
        let kind = match text {
            true => 0,
            false => *kinds.entry(std::rc::Rc::as_ptr(style)).or_insert_with(|| {
                let made = crate::render::effects::establishes(style);
                let clips = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                    .into_iter()
                    .any(|prop| style.get(prop).is_some_and(|value| value != "visible"))
                    || style.get(Prop::CONTAIN).is_some_and(|value| {
                        ["paint", "strict", "content"]
                            .iter()
                            .any(|kind| value.contains(kind))
                    });
                let display = style.get(Prop::DISPLAY);
                let position = match style.get(Prop::POSITION) {
                    Some("absolute") => 1,
                    Some("fixed") => 2,
                    _ => u8::from(display == Some("none")) * 4,
                };
                [
                    clips,
                    made.absolute,
                    made.fixed,
                    display.is_none_or(|v| v == "inline"),
                ]
                .into_iter()
                .enumerate()
                .fold(position, |kind, (bit, on)| kind | u8::from(on) << (bit + 3))
            }),
        };
        if kind & 4 != 0 || kind & 1 != 0 && !absolute || kind & 2 != 0 && !fixed {
            continue;
        }
        if let Some(&[x, y, width, height]) = mirror.get(&kid).filter(|_| mirror.shown(kid))
            && (width != 0. || height != 0.)
        {
            end = end.max(
                right.map_or([x, y][axis] + [width, height][axis] - start, |right| {
                    right - x
                }),
            );
            if kind & 8 != 0 {
                continue;
            }
        }
        if !text {
            let (absolute, fixed) = (absolute || kind & 16 != 0, fixed || kind & 32 != 0);
            let inline = kind & 64 != 0 && mirror.get(&kid).is_some();
            pending.extend(
                (kids(kid).iter())
                    .filter(|&&next| !inline || !doc.nodes[next].tag.starts_with('#'))
                    .map(|&next| (next, absolute, fixed)),
            );
        }
    }
    Ok(json!(end))
}

pub(super) fn client(
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let rect = state.geometry.mirror.get(&id).copied().unwrap_or_default();
    // Reuse the last frame's styles for resize observers to avoid a style pass per callback.
    let last = (args[1].as_bool() == Some(true))
        .then(|| {
            state
                .styles
                .computed
                .clone()
                .or_else(|| state.styles.previous.clone())
        })
        .flatten()
        .filter(|last| id < last.values.len() && last.lengths.get(id).is_some());
    let computed = match last {
        Some(last) => last,
        None => state.restyle(doc, viewport),
    };
    let (style, units) = (&computed.values[id], &state.units(&computed, id));
    let tag = doc.nodes[id].tag.as_str();
    if (style::ROW..=style::FOOTER).contains(&style::role(&computed, id)) {
        return Ok(json!([
            0.,
            0.,
            rect[2],
            rect[3],
            0.,
            0.,
            0.,
            0.,
            rect[2],
            rect[3],
            0.,
            0.,
            0.,
            units.font.zoom
        ]));
    }
    if tag.starts_with('#')
        || style
            .get(Prop::DISPLAY)
            .is_none_or(|value| matches!(value, "inline" | "contents"))
    {
        // The last value says that the box is inline, so it has no client box.
        return Ok(json!([
            0.,
            0.,
            0.,
            0.,
            0.,
            0.,
            0.,
            0.,
            rect[2],
            rect[3],
            0.,
            0.,
            1.,
            units.font.zoom
        ]));
    }
    let shared = style::role(&computed, id) == style::CELL
        && std::iter::successors(doc.nodes[id].parent, |&parent| doc.nodes[parent].parent)
            .find(|&parent| style::table_box(style::role(&computed, parent)))
            .is_some_and(|table| {
                computed
                    .values
                    .get(table)
                    .and_then(|style| style.get(Prop::BORDER_COLLAPSE))
                    == Some("collapse")
            });
    let base = doc.nodes[id]
        .parent
        .and_then(|parent| state.geometry.mirror.get(&parent))
        .map_or(viewport[0], |rect| rect[2]);
    // The bounds are those of the last frame, which laid out the animated padding and borders.
    let now = state.styles.animating.get(&id).unwrap_or(style);
    let borders = units
        .borders(now, base)
        .map(|width| width / if shared { 2. } else { 1. });
    let padding = units
        .edges(now, css::properties::PADDING, base)
        .map(|value| value.max(0.));
    let bars = (state.geometry.scroll.get(&id)).map_or([0.; 4], |handle| {
        crate::render::paint::scrollbar::room(style, computed.extras(id), units, handle, true)
    });
    // Like offsetWidth, client sizes ignore transforms, so a transformed box takes its layout size,
    // and the shift from its bounds to its own untransformed corner.
    let transformed = state.geometry.mirror.is_transformed(id);
    let size = state
        .layout
        .content_size(id)
        .filter(|_| transformed)
        .map_or([rect[2], rect[3]], |inner| {
            [
                inner[0] + padding[1] + padding[3] + borders[1] + borders[3],
                inner[1] + padding[0] + padding[2] + borders[0] + borders[2],
            ]
        });
    let local = renderer::Rect::new(
        renderer::point(renderer::px(0.), renderer::px(0.)),
        renderer::size(renderer::px(size[0]), renderer::px(size[1])),
    );
    // The bounds are those of the last frame, so its animated transform moved them.
    let shift = match transformed
        .then(|| crate::css::value::transform::transform(now, units, local, None))
        .flatten()
    {
        Some(m) if m[3] == 0. && m[7] == 0. && m[15] == 1. => {
            let [x, y] = [size[0] / 2., size[1] / 2.];
            [
                rect[2] / 2. - m[0] * x - m[4] * y - m[12],
                rect[3] / 2. - m[1] * x - m[5] * y - m[13],
            ]
        }
        _ => [0., 0.],
    };
    Ok(json!([
        borders[3],
        borders[0],
        (size[0] - borders[1] - borders[3] - bars[1] - bars[3]).max(0.),
        (size[1] - borders[0] - borders[2] - bars[2]).max(0.),
        padding[0],
        padding[1],
        padding[2],
        padding[3],
        size[0],
        size[1],
        shift[0],
        shift[1],
        0.,
        units.font.zoom
    ]))
}

// The boxes around the node that skip their contents: 2 if one has `content-visibility: hidden`, 1 if
// all have `auto`, 0 if none does, and -1 if the node or a box around it has `display: none`. With `true`,
// they show their contents until the next frame, so a layout gives the geometry of the node.
pub(super) fn skipped(
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let computed = state.restyle(doc, viewport);
    let (mut kind, mut roots) = (0, Vec::new());
    // An `auto` box that shows its contents from the next frame on still skips them.
    let hidden = state.layout.culled(id);
    let mut node = Some(id);
    while let Some(at) = node {
        let style = &computed.values[at];
        if style.get(Prop::DISPLAY) == Some("none") {
            return Ok(json!(-1));
        }
        if at != id
            && (crate::render::layout::cull::skipping(doc, &state.layout, &computed, at, false)
                || hidden
                    && style.get(Prop::CONTENT_VISIBILITY) == Some("auto")
                    && state.layout.auto_box(at))
        {
            roots.push(at);
            kind = kind.max(if style.get(Prop::CONTENT_VISIBILITY) == Some("hidden") {
                2
            } else {
                1
            });
        }
        node = state.tree.flat_parent(doc, &computed.children, at);
    }
    if args[1].as_bool() == Some(true) {
        for root in roots {
            state.layout.force_layout(root);
        }
    }
    Ok(json!(kind))
}
