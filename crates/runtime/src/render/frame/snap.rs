use crate::{
    css::properties::{Prop, Style},
    dom::Document,
    render::{PageState, paint::scrollbar},
};
use renderer::{ScrollSnaps, SnapArea, px};

// What the snap areas of a scroll container come from: document, styles, its box, its scroll range, and the room of
// its scroll bars.
pub(crate) type Key = (u64, u64, [u32; 4], [u32; 4], [u32; 4]);

// Gives the scroll containers the snap areas of this layout. `space` is the room of the scroll bars of the page at the
// bottom and at the right. True when a box moved to an area.
pub(super) fn update(doc: &Document, dom: &mut PageState, space: [f32; 2]) -> bool {
    let Some(styles) = dom.styles.computed.clone() else {
        return false;
    };
    let html = doc
        .nodes
        .first()
        .and_then(|root| (root.children.iter().copied()).find(|&id| doc.nodes[id].tag == "html"));
    let geometry = &mut dom.geometry;
    let mut moved = Vec::new();
    for (&id, handle) in &geometry.scroll {
        let Some(node) = (if id == 0 { html } else { Some(id) }) else {
            continue;
        };
        let Some(style) = styles.values.get(node) else {
            continue;
        };
        // The viewport takes the snap type of the root.
        let kind = (id == 0 || html != Some(id))
            .then(|| style.get(Prop::SCROLL_SNAP_TYPE))
            .flatten()
            .filter(|kind| *kind != "none");
        let Some(kind) = kind else {
            if handle.snaps() {
                handle.set_snaps(None);
                geometry.snaps.remove(&id);
            }
            continue;
        };
        let rect = match id {
            0 => {
                let size = handle.bounds().size;
                [0., 0., size.width.0, size.height.0]
            }
            _ => geometry.mirror.get(&id).copied().unwrap_or_default(),
        };
        if id != 0 && !(geometry.mirror.shown(id) && rect != [0.; 4]) {
            handle.forget_snaps();
            geometry.snaps.remove(&id);
            continue;
        }
        let units = &styles.lengths[node];
        let bars = match id {
            0 => [0., space[1], space[0], 0.],
            _ => scrollbar::room(style, styles.extras(node), units, handle.extent()),
        };
        let range = handle.range();
        let key: Key = (
            doc.nodes.latest(),
            geometry.layout_epoch,
            rect.map(f32::to_bits),
            [range[0].x, range[0].y, range[1].x, range[1].y].map(|value| value.0.to_bits()),
            bars.map(f32::to_bits),
        );
        if geometry.snaps.get(&id) == Some(&key) {
            continue;
        }
        geometry.snaps.insert(id, key);
        let borders = match id {
            0 => [0.; 4],
            _ => units.borders(style, rect[2]),
        };
        let [top, right, bottom, left] = [0, 1, 2, 3].map(|side| borders[side] + bars[side]);
        let client = [rect[2] - left - right, rect[3] - top - bottom];
        let padding = |prop: Prop, base: f32| {
            (style.get(prop))
                .and_then(|value| units.resolve(value, base))
                .unwrap_or(0.)
        };
        let start = [
            rect[0] + left + padding(Prop::SCROLL_PADDING_LEFT, client[0]),
            rect[1] + top + padding(Prop::SCROLL_PADDING_TOP, client[1]),
        ];
        let end = [
            rect[0] + left + client[0] - padding(Prop::SCROLL_PADDING_RIGHT, client[0]),
            rect[1] + top + client[1] - padding(Prop::SCROLL_PADDING_BOTTOM, client[1]),
        ];
        let must = kind.contains("mandatory");
        let axis = kind.split_ascii_whitespace().next().unwrap_or_default();
        let strict = [
            matches!(axis, "x" | "inline" | "both").then_some(must),
            matches!(axis, "y" | "block" | "both").then_some(must),
        ];
        let rtl = style.get(Prop::DIRECTION) == Some("rtl");
        let shown = geometry.mirror_offset(id);
        let at = [-shown.x.0, -shown.y.0];
        let scrolls = |style: &Style| {
            [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                .into_iter()
                .any(|prop| {
                    (style.get(prop)).is_some_and(|value| !matches!(value, "visible" | "clip"))
                })
        };
        let mut areas = Vec::new();
        let mut stack: Vec<usize> = styles
            .flat_children(doc, node)
            .iter()
            .rev()
            .copied()
            .collect();
        while let Some(child) = stack.pop() {
            let Some(style) = styles.values.get(child) else {
                continue;
            };
            let align = style
                .get(Prop::SCROLL_SNAP_ALIGN)
                .filter(|align| *align != "none");
            if let Some(align) = align
                && let Some(&area) = geometry.mirror.get(&child)
                && area != [0.; 4]
                && geometry.mirror.shown(child)
            {
                let units = &styles.lengths[child];
                let margin = |prop: Prop| {
                    (style.get(prop))
                        .and_then(|value| units.resolve(value, 0.))
                        .unwrap_or(0.)
                };
                let low = [
                    area[0] - margin(Prop::SCROLL_MARGIN_LEFT),
                    area[1] - margin(Prop::SCROLL_MARGIN_TOP),
                ];
                let high = [
                    area[0] + area[2] + margin(Prop::SCROLL_MARGIN_RIGHT),
                    area[1] + area[3] + margin(Prop::SCROLL_MARGIN_BOTTOM),
                ];
                let mut aligns = align.split_ascii_whitespace();
                let block = aligns.next().unwrap_or("none");
                let inline = match aligns.next().unwrap_or(block) {
                    "start" if rtl => "end",
                    "end" if rtl => "start",
                    inline => inline,
                };
                let place = |axis: usize, align: &str| {
                    strict[axis]?;
                    let first = at[axis] + low[axis] - start[axis];
                    let last = at[axis] + high[axis] - end[axis];
                    // The snapport can be anywhere in an area that is larger.
                    Some(match align {
                        "none" => return None,
                        _ if last > first => [first, last],
                        "start" => [first; 2],
                        "end" => [last; 2],
                        _ => [(first + last) / 2.; 2],
                    })
                };
                areas.push(SnapArea {
                    id: child as u64,
                    at: [place(0, inline), place(1, block)],
                    seen: [0, 1].map(|axis| {
                        [
                            at[axis] + low[axis] - end[axis],
                            at[axis] + high[axis] - start[axis],
                        ]
                    }),
                    stop: style.get(Prop::SCROLL_SNAP_STOP) == Some("always"),
                });
            }
            // The areas in an inner scroll container belong to it.
            if !scrolls(style) {
                stack.extend(styles.flat_children(doc, child).iter().rev());
            }
        }
        let ratio = dom.pixel_ratio.max(1.);
        // Offsets are whole device pixels.
        for area in &mut areas {
            for value in area.at.iter_mut().flatten().flatten() {
                *value = (*value * ratio).round() / ratio;
            }
        }
        let before = handle.offset();
        if handle.set_snaps(Some(ScrollSnaps {
            strict,
            port: [end[0] - start[0], end[1] - start[1]],
            areas,
        })) && (handle.offset() - before) != renderer::point(px(0.), px(0.))
        {
            moved.push(id);
        }
    }
    if moved.is_empty() {
        return false;
    }
    geometry.mirror.set_moved();
    true
}
