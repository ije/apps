use super::PageState;
use crate::css;
use crate::css::properties::Prop;
use crate::dom::Document;
use crate::dom::{LEAVING, TOP_LAYER};
use crate::style;
use serde_json::{Value, json};

pub(crate) fn hit_test(
    doc: &Document,
    state: &PageState,
    computed: &style::ComputedStyles,
    viewport: [f32; 2],
    [x, y]: [f32; 2],
    first: bool,
    caret: bool,
) -> Value {
    let mut nearest = Vec::new();
    // Like Chrome, a box at the height of the point comes first, then the boxes below it, then the boxes above it.
    // Then the vertical and the horizontal distance of the point to the box.
    let gap = |[left, top, width, height]: [f32; 4]| {
        let rank = if y < top {
            1.
        } else if y > top + height {
            2.
        } else {
            0.
        };
        [
            rank,
            (top - y).max(y - top - height).max(0.),
            (left - x).max(x - left - width).max(0.),
        ]
    };
    let order = |a: &[f32; 3], b: &[f32; 3]| {
        (a[0].total_cmp(&b[0]))
            .then(a[1].total_cmp(&b[1]))
            .then(a[2].total_cmp(&b[2]))
    };
    let screen = [0., 0., viewport[0], viewport[1]];
    let up = |id: usize| {
        doc.nodes[id]
            .parent
            .or_else(|| state.tree.hosts.get(&id).copied())
    };
    let top: Vec<usize> = state
        .interaction
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
    // The last field is 1 in the drawable content of a canvas, which takes no pointer, and 2 in an element
    // that the canvas drew.
    // The rounded corners of the nearest clipping box, as in the renderer.
    type Round = Option<(renderer::Rect<renderer::Pixels>, renderer::Radii)>;
    let mut pending = vec![(
        0,
        screen,
        false,
        0,
        0u8,
        None::<(renderer::Rect<renderer::Pixels>, renderer::Radii)>,
    )];
    pending.extend(top.iter().map(|&id| (id, screen, false, 0, 0, None)));
    let mut hits = Vec::new();
    // The ancestors of the current node with the clips inside them and, once read, the containing blocks they
    // make: 1 when read, 2 for absolute and 4 for fixed boxes.
    let mut path: Vec<(usize, [f32; 4], u8, Round)> = Vec::new();
    let inside = |clip: [f32; 4]| x >= clip[0] && x < clip[2] && y >= clip[1] && y < clip[3];
    while let Some((id, mut clip, hit, depth, mut drawn, mut round)) = pending.pop() {
        if hit {
            hits.push(id);
            continue;
        }
        // The last frame marked each subtree with an out-of-flow box uncullable.
        if !inside(clip)
            && state
                .layout
                .cull
                .info
                .get(id)
                .is_some_and(|info| info.flags & crate::render::layout::cull::UNCULLABLE == 0)
        {
            continue;
        }
        let node = &doc.nodes[id];
        let Some(style) = computed.values.get(id) else {
            continue;
        };
        if state.styles.inert(id)
            || style
                .get(Prop::DISPLAY)
                .is_some_and(|value| value == "none")
        {
            continue;
        }
        path.truncate(depth);
        if drawn != 0 {
            drawn = match state.geometry.drawn.get(&id) {
                Some(place) => {
                    // The clip is in the content box of the canvas.
                    let canvas =
                        (path.iter().rev()).find(|(id, ..)| doc.nodes[*id].tag == "canvas");
                    if let (Some([x, y, width, height]), Some(&(canvas, ..))) = (place.clip, canvas)
                        && let Some(&[left, top, wide, _]) = state.geometry.mirror.get(&canvas)
                    {
                        let (style, units) = (&computed.values[canvas], &computed.lengths[canvas]);
                        let (border, padding) = (
                            units.borders(style, wide),
                            units.edges(style, css::properties::PADDING, wide),
                        );
                        let [x, y] = [
                            x + left + border[3] + padding[3],
                            y + top + border[0] + padding[0],
                        ];
                        clip = [
                            clip[0].max(x),
                            clip[1].max(y),
                            clip[2].min(x + width),
                            clip[3].min(y + height),
                        ];
                    }
                    2
                }
                None if node.attrs.contains_key("drawable") => 1,
                None => drawn,
            };
        }
        // An out-of-flow box gets only the clip of its containing block, as the renderer draws it. That clip
        // holds the clip of each box between, so a box without a clip keeps it.
        if clip != screen
            && let Some(escape) = style
                .get(Prop::POSITION)
                .filter(|&value| matches!(value, "absolute" | "fixed"))
        {
            let bit = if escape == "fixed" { 4 } else { 2 };
            (clip, round) = path
                .iter_mut()
                .rev()
                .find_map(|(id, clip, made, round)| {
                    if *made == 0 {
                        let block = crate::render::effects::establishes(&computed.values[*id]);
                        *made = 1 | (u8::from(block.absolute) * 2) | (u8::from(block.fixed) * 4);
                    }
                    (*made & bit != 0).then_some((*clip, *round))
                })
                .unwrap_or((screen, None));
        }
        let canvas = style::drawable_canvas(node, style);
        if inside(clip)
            && let Some(&[left, top, width, height]) = state.geometry.mirror.get(&id)
        {
            let size = renderer::size(renderer::px(width), renderer::px(height));
            let rect =
                renderer::Rect::new(renderer::point(renderer::px(left), renderer::px(top)), size);
            let at = renderer::point(renderer::px(x), renderer::px(y));
            let corners = match node.tag.starts_with('#') {
                true => renderer::Radii::default(),
                false => crate::render::effects::clip::radii(style, &computed.lengths[id], || {
                    [width, height]
                })
                .resolve(size),
            };
            let selectable = !style
                .get(Prop::POINTER_EVENTS)
                .is_some_and(|value| value == "none")
                && !style
                    .get(Prop::VISIBILITY)
                    .is_some_and(|value| matches!(value, "hidden" | "collapse"));
            if caret && node.tag == "#text" && selectable && height > 0. {
                nearest.push((id, gap([left, top, width, height])));
            }
            let within = |[left, top, width, height]: [f32; 4]| {
                width > 0.
                    && height > 0.
                    && x >= left
                    && x < left + width
                    && y >= top
                    && y < top + height
            };
            if (caret || !node.tag.starts_with('#'))
                && match state.geometry.fragments.is_empty() {
                    true => within([left, top, width, height]),
                    false => state
                        .geometry.fragments
                        .get(&id)
                        .map_or(within([left, top, width, height]), |parts| {
                            parts.iter().copied().any(within)
                        }),
                }
                // Like Chrome, a point outside the rounded corners misses the box and what it clips.
                && (node.tag.starts_with('#') || corners.contains(rect, at))
                && round.is_none_or(|(rect, corners)| corners.contains(rect, at))
                && selectable
                && drawn != 1
            {
                pending.push((id, clip, true, depth, drawn, None));
            }
            // A canvas shows its drawable content only inside its box.
            if canvas {
                drawn = 1;
            }
            for (axis, prop) in [(0, Prop::OVERFLOW_X), (1, Prop::OVERFLOW_Y)] {
                if canvas
                    || drawn != 1
                        && style.get(prop).is_some_and(|value| {
                            matches!(value, "hidden" | "clip" | "auto" | "scroll")
                        })
                {
                    clip[axis] = clip[axis].max([left, top][axis]);
                    clip[axis + 2] = clip[axis + 2].min([left + width, top + height][axis]);
                    if !corners.is_zero() {
                        round = Some((rect, corners));
                    }
                }
            }
        }
        // The fallback content of a canvas has no boxes, and skipped contents take no pointer.
        if node.tag == "canvas" && !canvas
            || crate::render::skipping(doc, state, computed, id, false)
        {
            continue;
        }
        path.push((id, clip, 0, round));
        pending.extend(
            computed
                .flat_children(doc, id)
                .iter()
                .filter(|child| !top.contains(child))
                .map(|&child| (child, clip, false, depth + 1, drawn, round)),
        );
    }
    hits.sort_by_cached_key(|&id| {
        let painted = |id: usize| {
            std::iter::successors(Some(id), |&id| up(id))
                .find_map(|id| {
                    state
                        .layout
                        .painted
                        .get(id)
                        .copied()
                        .filter(|&order| order > 0)
                })
                .unwrap_or(0)
        };
        // Elements that a canvas drew are above the canvas, the last one drawn first.
        let drawn = std::iter::successors(Some(id), |&id| doc.nodes[id].parent)
            .filter(|_| !state.geometry.drawn.is_empty())
            .find_map(|at| Some((at, state.geometry.drawn.get(&at)?.order)));
        std::cmp::Reverse(match drawn {
            Some((element, order)) => {
                let canvas =
                    std::iter::successors(doc.nodes[element].parent, |&id| doc.nodes[id].parent)
                        .find(|&id| doc.nodes[id].tag == "canvas");
                (canvas.map_or(0, painted), order, painted(id))
            }
            None => (painted(id), 0, 0),
        })
    });
    for &id in top.iter().rev() {
        if hits.first().is_some_and(|&hit| {
            std::iter::successors(Some(hit), |&hit| up(hit)).any(|hit| hit == id)
        }) {
            break;
        }
        if !state.styles.inert(id)
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
        let editable = |id: usize| {
            std::iter::successors(Some(id), |&id| doc.nodes[id].parent)
                .find_map(|id| doc.nodes[id].content_editable())
                .unwrap_or(false)
        };
        let editing = target.is_some_and(editable);
        // Like Chrome: the line nearest to the point, then the nearest text on it. An editable target takes
        // only its editable text.
        let text = nearest
            .into_iter()
            .filter(|&(id, _)| {
                std::iter::successors(doc.nodes[id].parent, |&id| doc.nodes[id].parent)
                    .any(|id| Some(id) == target)
                    && (!editing || editable(id))
            })
            .map(|(id, key)| {
                // A text box that spans the point's height can hold more lines: measure each.
                let lines = (state.layout.text_layouts.get(&id))
                    .filter(|_| key[0] == 0.)
                    .map(|(layout, text, range, rows, font)| {
                        crate::render::paint::text::text_rects(
                            layout,
                            text,
                            rows,
                            [range.start, range.end],
                            *font,
                        )
                    });
                (
                    id,
                    lines
                        .into_iter()
                        .flatten()
                        .map(gap)
                        .min_by(order)
                        .unwrap_or(key),
                )
            })
            .min_by(|a, b| order(&a.1, &b.1))
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
