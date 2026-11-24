use super::Build;
use super::cx::Cx;
use super::kept::{Query, hashed};
use super::query::LOOK;
use crate::css;
use crate::css::properties::Prop;
use renderer::{Color, px};

/// Reads the look of a native form control, and gives its box the padding and borders of that look.
#[allow(clippy::too_many_arguments)]
pub(super) fn native_control(
    cx: &Cx<'_>,
    mut el: renderer::Node,
    input: Option<&str>,
    appkit: Option<renderer::ControlKind>,
    padding: &mut [f32; 4],
    borders: &mut [f32; 4],
    border_kinds: &mut [&str; 4],
    border_colors: &mut [Color; 4],
) -> (renderer::Node, Option<super::super::paint::control::Look>) {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        parsed,
        ..
    } = *cx;
    let Build {
        doc,
        text,
        pixel_ratio,
        ..
    } = *reader;
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
    let native = super::super::paint::control::kind(&node.tag, input, list.is_some(), style).map(
        |(control, boxed, plain)| {
            let number = |name: &str| {
                node.attrs
                    .get(name)
                    .and_then(|value| value.trim().parse::<f32>().ok())
                    .filter(|value| value.is_finite())
            };
            let [min, max] = match control {
                super::super::paint::control::Control::Range => {
                    [number("min").unwrap_or(0.), number("max").unwrap_or(100.)]
                }
                super::super::paint::control::Control::Meter => {
                    [number("min").unwrap_or(0.), number("max").unwrap_or(1.)]
                }
                _ => [0., number("max").filter(|&max| max > 0.).unwrap_or(1.)],
            };
            let max = max.max(min);
            let value = match control {
                super::super::paint::control::Control::Range => {
                    let value = node
                        .current_value()
                        .and_then(|value| value.trim().parse::<f32>().ok())
                        .map_or((min + max) / 2., |value| value.clamp(min, max));
                    // Like the value property, the thumb snaps to a step from `min`.
                    match node.attrs.get("step").map(|step| step.trim()) {
                        Some(step) if step.eq_ignore_ascii_case("any") => value,
                        step => {
                            let step = step
                                .and_then(|step| step.parse::<f32>().ok())
                                .filter(|step| *step > 0.)
                                .unwrap_or(1.);
                            let steps = ((value - min) / step).round();
                            min + if min + steps * step > max {
                                steps - 1.
                            } else {
                                steps
                            } * step
                        }
                    }
                }
                _ => number("value").unwrap_or(
                    if control == super::super::paint::control::Control::Progress {
                        -1.
                    } else {
                        0.
                    },
                ),
            };
            let mut dark = cx.dark();
            let accent = match control {
                super::super::paint::control::Control::Meter => {
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
                super::super::paint::control::Control::Color => node
                    .current_value()
                    .and_then(css::value::color::color)
                    .map_or(0, |(color, _)| color),
                _ if node.attrs.contains_key("disabled") => 0x767676,
                _ => match style
                    .get(Prop::ACCENT_COLOR)
                    .and_then(css::value::color::color)
                {
                    Some((color, _)) => {
                        dark = super::super::paint::control::accent_dark(
                            control,
                            node.checked(),
                            color,
                            dark,
                        );
                        color
                    }
                    None if dark => 0x99c8ff,
                    None => 0x0075ff,
                },
            };
            super::super::paint::control::Look {
                control,
                boxed,
                plain,
                checked: node.checked(),
                disabled: node.attrs.contains_key("disabled"),
                hover: node.state & crate::css::selector::HOVER != 0,
                active: node.state & crate::css::selector::ACTIVE != 0,
                dark,
                accent,
                ink: style
                    .get(Prop::COLOR)
                    .and_then(css::value::color::color)
                    .map_or(0, |(color, _)| color),
                value: if value < min
                    || max <= min && control != super::super::paint::control::Control::Progress
                {
                    if control == super::super::paint::control::Control::Progress {
                        -1.
                    } else {
                        0.
                    }
                } else {
                    ((value - min) / (max - min).max(f32::EPSILON)).clamp(0., 1.)
                },
            }
        },
    );
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
            let (edge, fill) = super::super::paint::control::box_colors(look);
            for side in 0..4 {
                padding[side] += (borders[side] - 1.).max(0.);
                borders[side] = borders[side].min(1.);
                border_kinds[side] = "solid";
                if let Some(edge) = edge {
                    border_colors[side] = Color::hex(edge << 8 | 0xff);
                }
            }
            if let Some(fill) = fill {
                el = el.bg(Color::hex(fill << 8 | 0xff));
            }
        }
        if let Some(rows) = list
            && style.get(Prop::HEIGHT).is_none_or(|value| value == "auto")
        {
            // Options have `line-height: normal` and `min-height: 1.2em`.
            let normal = text
                .line_metrics(text.face(&parsed.face), px(units.font.em * pixel_ratio))
                .map(f32::from)
                .iter()
                .sum::<f32>()
                / pixel_ratio;
            let row = (normal.max(1.2 * units.font.em) * 64.).round() / 64. + 1.;
            el = el.h(px(rows as f32 * row
                + padding[0]
                + padding[2]
                + borders[0]
                + borders[2]));
        }
        if look.control == super::super::paint::control::Control::Menu {
            padding[1] += 16.;
            padding[3] += 4.;
            if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
                let (font, size) = (parsed.face.clone(), px(units.font.em));
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
                                color: renderer::Color::BLACK,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                                overline: None,
                                letter_spacing: px(0.),
                                word_spacing: px(0.),
                                font_size: None,
                                inset: Default::default(),
                                nowrap: false,
                                tab_size: 8.,
                                stroke: None,
                            };
                            widest = widest
                                .max(f32::from(text.shape_line(&label, size, &run).line.width));
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
                let face = text.face(&parsed.face);
                let [ascent, descent, _] = text
                    .line_metrics(face, px(units.font.em * pixel_ratio))
                    .map(f32::from);
                let height = px((ascent + descent) / pixel_ratio
                    + 2.
                    + padding[0]
                    + padding[2]
                    + borders[0]
                    + borders[2]);
                el = if cx.stretched(1) {
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
    (el, native)
}
