use super::super::layout::size::{aspect_ratio, layout_ceil};
use super::Build;
use super::cx::{Cx, Frame, percentage, viewport};
use super::margin::{percent_bound, percent_calc};
use super::query::only_auto_ratio;
use crate::css;
use crate::css::properties::Prop;
use crate::style::{self};
use renderer::{px, relative};

/// Applies the box properties of the computed style to the box: sizes, insets, flex and grid items, effects.
pub(super) fn props(
    cx: &Cx<'_>,
    fr: &mut Frame,
    mut el: renderer::Node,
    inner_shadows: &mut Vec<renderer::BoxShadow>,
    line_base: Option<bool>,
    in_details: bool,
) -> renderer::Node {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        position,
        containing,
        slot,
        parent,
        rtl,
        ref hit,
        flexbox,
        editable,
        sizeless,
        block_size,
        opacity,
        effect_layer,
        ..
    } = *cx;
    let Build {
        doc,
        styles,
        text,
        pixel_ratio,
        measured,
        flex_parent,
        ..
    } = *reader;
    let Frame {
        margin,
        extra,
        inset_x,
        natural_ratio,
        ..
    } = *fr;
    let viewport = || viewport(reader, &units);
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
                    let [min, max, ..] = super::super::layout::grid::intrinsic::intrinsic(
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
                    fr.intrinsic_width =
                        Some(max.min(min.max(containing[0] - margin[1] - margin[3])));
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
                    let [min, max, ..] = super::super::layout::grid::intrinsic::intrinsic(
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
                    fr.intrinsic_width = Some(size);
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
                el.style().box_shadow = css::value::box_shadows(&units, value, false);
                *inner_shadows = css::value::box_shadows(&units, value, true);
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
    el
}
