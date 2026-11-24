use super::super::Build;
use super::super::border::{border_canvas, inset_shadows};
use super::super::kept::{Query, hashed};
use super::super::margin::{clear_bits, float_side, percent_bound, percent_calc};
use super::super::query::{inset_axes, only_auto_ratio, positioner, static_mark};
use super::cx::{Carry, Cx, Frame, percentage, viewport};
use crate::css;
use crate::css::properties::{Prop, Style};
use crate::render::layout::size::{aspect_ratio, pseudo_lengths};
use crate::render::{layout::cull, paint::background::css_background};
use crate::style::{self};
use renderer::{Color, Rect, SharedString, px, relative};
use std::ops::ControlFlow;
use std::rc::Rc;

/// Builds `::before` and `::after`, and puts each where it goes: in the box, its lines, its text or its items.
/// `Break` holds the result of `create` when the box is the pseudo-element itself.
pub(super) fn pseudo_elements(
    cx: &Cx<'_>,
    fr: &Frame,
    mut el: renderer::Node,
    carry: &mut Carry,
    pseudos: &[Style; 2],
    own: Option<[f32; 2]>,
) -> ControlFlow<renderer::Child, renderer::Node> {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        parsed,
        display,
        containing,
        pseudo,
        flow,
        all_inline,
        rtl,
        flexbox,
        editable,
        ..
    } = *cx;
    let Build {
        ref ancestors,
        doc,
        ids,
        info,
        pixel_ratio,
        styles,
        text,
        view,
        ..
    } = *reader;
    let Carry {
        after,
        lines,
        static_anchor,
        items,
        pseudo_atoms,
        pseudo_origin,
        ..
    } = carry;
    let padding = fr.padding;
    let content_box = || fr.content_box(cx);
    let viewport = || viewport(reader, &units);
    let made = parsed.made;
    let ink = parsed.ink.unwrap_or(Color::BLACK);
    let sides = css::properties::BORDERS.map(|[width, kind, color]| (width, kind, color));
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
        let held = cx.inline_text().or_else(|| cx.edge_text(index));
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
                                renderer::AlignItems::FlexEnd => renderer::AlignContent::FlexEnd,
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
        if let Some(ink) = pseudo.get(Prop::COLOR).and_then(css::value::color::color) {
            decoration = decoration.text_color(css::value::color::rgba(ink));
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
            .edges(pseudo, css::properties::PADDING, base)
            .map(|v| v.max(0.));
        let pseudo_margin = pseudo_units.edges(pseudo, css::properties::MARGIN, base);
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
        for (side, prop) in css::properties::MARGIN.iter().enumerate() {
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
        let pseudo_radius =
            crate::render::effects::clip::radii(pseudo, &pseudo_units, || containing);
        decoration.style().corner_radii = pseudo_radius;
        let colors = sides.map(|(_, _, color)| {
            pseudo
                .get(color)
                .and_then(css::value::color::paint)
                .unwrap_or(ink)
        });
        let border_image = crate::render::paint::background::border_image(
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
                let kinds = sides.map(|(_, kind, _)| {
                    crate::render::effects::border_style(pseudo.get(kind).unwrap_or("solid"))
                });
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
                    |ancestor| crate::render::effects::establishes(&styles.values[ancestor]),
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
                    let size = reader.block_size(view.host.borrow().page_state(view.id), block);
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
                        Some(percent) => {
                            renderer::Length::Calc(percent.max(0.) / 100., px(pseudo_extra[axis]))
                        }
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
                        "flex-start" | "start" | "self-start" => renderer::AlignItems::FlexStart,
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
                    if let Some(color) = css::value::color::paint(value) {
                        decoration = decoration.bg(color);
                    }
                }
                Prop::OPACITY if !keyed => {
                    decoration = decoration.opacity(crate::render::effects::opacity(pseudo));
                }
                Prop::BOX_SHADOW => {
                    decoration.style().box_shadow =
                        css::value::box_shadows(&pseudo_units, value, false);
                    pseudo_shadows = css::value::box_shadows(&pseudo_units, value, true);
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
                .page_state(view.id)
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
                &view.host.borrow().app.assets,
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
        if !absolute && own.starts_with("inline") && (all_inline || display == "inline" || atom) {
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
                        Some(height) if align == "text-top" => above - pseudo_margin[0] - height,
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
            for effect in crate::render::effects::filter::filters(value, &pseudo_units, doc, ids) {
                decoration = decoration.hooks(crate::render::paint::Effected {
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
            decoration = decoration.hooks(crate::render::paint::Masked {
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
            decoration = decoration.hooks(crate::render::paint::Effected {
                effect: renderer::PaintEffect {
                    blend_mode: crate::render::effects::filter::blend_mode(value),
                    ..Default::default()
                },
                backdrop: None,
            });
        }
        // An opacity animation keeps one keyed layer, so its frames only patch the layer's opacity.
        if keyed {
            decoration = decoration.hooks(crate::render::paint::Effected {
                effect: renderer::PaintEffect {
                    opacity: crate::render::effects::opacity(pseudo),
                    key: Some(crate::render::effects::pseudo_key(id, index as u8 + 1)),
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
            .and_then(|value| crate::render::effects::filter::backdrop(value, &pseudo_units))
        {
            Some((blur, color_matrix)) => decoration.hooks(crate::render::paint::Effected {
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
        let decoration = if crate::render::effects::TRANSFORM_PROPS
            .iter()
            .any(|&prop| pseudo.get(prop).is_some_and(|value| value != "none"))
        {
            decoration.hooks(crate::render::paint::Transformed {
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
                contained = Some(crate::render::paint::PseudoBlock {
                    view: view.clone(),
                    anchor: None,
                    place: Some(crate::render::paint::Contained {
                        key: crate::render::effects::pseudo_key(id, index as u8 + 1) as usize,
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
        if index == 1 && crate::render::inline::static_after(styles, id) {
            let key = crate::render::inline::atom_key(id, 2);
            if flow
                && !flexbox
                && !matches!(display, "grid" | "inline-grid")
                && !node.children.is_empty()
                && cx.inline_text().is_none()
            {
                let mut placeholder = static_mark(view, key).flex_shrink_0();
                placeholder.style().align_self = Some(renderer::AlignItems::FlexStart);
                *static_anchor = Some(placeholder);
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
                    contained = Some(crate::render::paint::PseudoBlock {
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
            decoration.hooks(crate::render::paint::PseudoBox {
                view: view.clone(),
                element: id,
                origin: pseudo_origin.get_or_insert_default().clone(),
                shift: contained.as_ref().map(|contained| contained.offset.clone()),
                mirrored: lifted,
                key: crate::render::inline::atom_key(id, index as u8 + 1),
            })
        };
        // Outside `PseudoBox`, which reads its move.
        let decoration = match contained {
            Some(contained) => decoration.hooks(contained),
            None => decoration,
        };
        if lifted {
            return ControlFlow::Break(decoration.into());
        }
        // The part of a spliced box gives the box its bounds.
        if pseudo_only {
            return ControlFlow::Break(
                decoration
                    .hooks(crate::render::paint::PseudoPart {
                        view: view.clone(),
                        node: id,
                        key: crate::render::inline::atom_key(id, index as u8 + 1),
                    })
                    .into(),
            );
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
            && cx.inline_text().is_none()
        {
            lines[index] = Some(decoration);
        } else if index == 0 && flex_order <= 0 || index == 1 && flex_order < 0 {
            if std::mem::take(&mut carry.strut_due) {
                el = el.child(cx.strut());
            }
            el = el.child(decoration);
        } else {
            after.push((flex_order, decoration));
        }
    }
    ControlFlow::Continue(el)
}
