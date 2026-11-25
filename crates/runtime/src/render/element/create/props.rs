use super::super::Build;
use super::super::kept::{Query, hashed};
use super::super::margin::{percent_bound, percent_calc, shrinks_around};
use super::super::query::{inset_axes, only_auto_ratio, text_indent};
use super::cx::{Cx, Frame, percentage, viewport};
use crate::css;
use crate::css::properties::Prop;
use crate::render::layout::size::{aspect_ratio, dimension, layout_ceil};
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
    let viewport = || viewport(reader);
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
            true => Some(viewport()),
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
                    let [min, max, ..] = crate::render::layout::grid::intrinsic::intrinsic(
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
                    let [min, max, ..] = crate::render::layout::grid::intrinsic::intrinsic(
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
                if matches!(position, "relative" | "absolute")
                    || position == "fixed" && cx.positioner.is_some() =>
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

/// Sizes a box with a natural size, like an image, from that size, its ratio and its constraints.
#[inline(never)]
pub(super) fn natural_box(
    cx: &Cx<'_>,
    fr: &Frame,
    mut el: renderer::Node,
    natural: [f32; 2],
    line_base: Option<bool>,
) -> renderer::Node {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        position,
        containing,
        broken,
        ..
    } = *cx;
    let Build {
        doc,
        styles,
        flex_parent,
        ..
    } = *reader;
    let Frame {
        padding,
        borders,
        margin,
        extra,
        natural_ratio,
        ..
    } = *fr;
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
                value != "auto" && !(axis == 1 && line_base == Some(false) && value.contains('%'))
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
    let stretch = [cx.stretched(0), cx.stretched(1)];
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
            if let Some(min) = fr.floor(cx, 0, width + chrome[0]) {
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
        Some(ratio) if matches!(position, "absolute" | "fixed") => width.map(|width| width / ratio),
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
            if let Some(min) = fr.floor(cx, 1, height) {
                el = el.min_h(px(min));
            }
        }
    }
    el
}

/// Makes the box of an element whose style no equal box built before: its display, position, breaks and text.
pub(super) fn new_box(cx: &Cx<'_>) -> renderer::Node {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        parsed,
        display,
        position,
        slot,
        ..
    } = *cx;
    let Build {
        flex_parent,
        columned,
        ..
    } = *reader;
    let (face, line_height, made) = (&parsed.face, parsed.line_height, parsed.made);
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
    if el.style().escape != 0 {
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
