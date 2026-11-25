use super::super::Build;
use super::super::kept::{Query, Read, Shared, hashed};
use super::super::margin::{clear_bits, float_side};
use super::super::query::{boxless, collapses, insets, padding_start, scroller};
use super::cx::{Cx, Frame};
use crate::css::properties::Prop;
use crate::render::{
    layout::cull,
    paint::{DomBox, Turned, scrollbar},
};
use crate::style::{self};
use renderer::px;
use std::rc::Rc;

/// Finishes the box: window drags, the anchor and sticky offsets of positioned boxes, its hooks and the stacking
/// order, and stores its style for equal boxes.
#[allow(clippy::too_many_arguments)]
pub(super) fn finish(
    cx: &Cx<'_>,
    fr: &Frame,
    mut el: renderer::Node,
    drag: bool,
    replaced: bool,
    bars: Option<scrollbar::Scrollbar>,
    mut cut_parts: Vec<(usize, usize, crate::render::paint::Piece)>,
    plans: u8,
    text_clip: bool,
    pseudo_origin: Option<Rc<std::cell::Cell<renderer::Point<renderer::Pixels>>>>,
) -> renderer::Child {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        parsed,
        display,
        position,
        containing,
        slot,
        depth,
        parent,
        multicol,
        cuts,
        ref hit,
        stacking,
        effect_layer,
        composited,
        drawable,
        layered,
        mark,
        order,
        positioned,
        parent_box,
        positioner,
        share,
        ..
    } = *cx;
    let Build {
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        measured,
        ids,
        info,
        reads,
        top_layer,
        flex_parent,
        ref ancestors,
        perspective: ref parent_perspective,
        ..
    } = *reader;
    let Frame {
        padding,
        borders,
        margin,
        extra,
        ..
    } = *fr;
    let made = parsed.made;
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
                let dragged = view.store.try_borrow().is_ok_and(|store| {
                    let viewport = *store.viewports.get(&view.id).unwrap_or(&[800., 600.]);
                    let Some(page) = store.pages.get(&view.id) else {
                        return true;
                    };
                    let state = &page.state;
                    let Some(computed) = &state.styles.computed else {
                        return true;
                    };
                    let at = [position.x, position.y].map(f32::from);
                    crate::render::hit::hit_test(
                        &page.doc, state, computed, viewport, at, true, false,
                    )
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
            let store = view.store.borrow();
            let scrolls = &store.page_state(view.id)?.geometry.scroll;
            let parent = scroller(doc, styles, ancestors.iter().rev().copied(), 0);
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
            Some(Box::new(crate::render::paint::Sticky {
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
        && super::super::query::rtl(doc, styles, positioner.unwrap_or(0));
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
            let [_, widest, ..] = crate::render::layout::grid::intrinsic::intrinsic(
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
            let block = crate::render::layout::grid::intrinsic::intrinsic(
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
            Some(Rc::new(crate::render::paint::Parts::Rows(rows.collect())))
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
                Rc::new(crate::render::paint::Parts::Cuts {
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
        element = element.hooks(crate::render::paint::ElementOrigin(origin));
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
                zone: reader.zone.clone(),
                culls: &culls,
                checking: true,
                extra: Default::default(),
                plan: Default::default(),
                ..*reader
            };
            let fresh = super::create(&shadow, id, containing, cx.container, None, None);
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
