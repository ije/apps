use super::super::Build;
use super::super::kept::{Query, hashed};
use super::super::query::LOOK;
use super::cx::{Cx, Frame};
use crate::css;
use crate::css::properties::Prop;
use renderer::{Color, Rect, point, px, size};

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
) -> (renderer::Node, Option<crate::render::paint::control::Look>) {
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
    let native = crate::render::paint::control::kind(&node.tag, input, list.is_some(), style).map(
        |(control, boxed, plain)| {
            let number = |name: &str| {
                node.attrs
                    .get(name)
                    .and_then(|value| value.trim().parse::<f32>().ok())
                    .filter(|value| value.is_finite())
            };
            let [min, max] = match control {
                crate::render::paint::control::Control::Range => {
                    [number("min").unwrap_or(0.), number("max").unwrap_or(100.)]
                }
                crate::render::paint::control::Control::Meter => {
                    [number("min").unwrap_or(0.), number("max").unwrap_or(1.)]
                }
                _ => [0., number("max").filter(|&max| max > 0.).unwrap_or(1.)],
            };
            let max = max.max(min);
            let value = match control {
                crate::render::paint::control::Control::Range => {
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
                    if control == crate::render::paint::control::Control::Progress {
                        -1.
                    } else {
                        0.
                    },
                ),
            };
            let mut dark = cx.dark();
            let accent = match control {
                crate::render::paint::control::Control::Meter => {
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
                crate::render::paint::control::Control::Color => node
                    .current_value()
                    .and_then(css::value::color::color)
                    .map_or(0, |(color, _)| color),
                _ if node.attrs.contains_key("disabled") => 0x767676,
                _ => match style
                    .get(Prop::ACCENT_COLOR)
                    .and_then(css::value::color::color)
                {
                    Some((color, _)) => {
                        dark = crate::render::paint::control::accent_dark(
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
            crate::render::paint::control::Look {
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
                    || max <= min && control != crate::render::paint::control::Control::Progress
                {
                    if control == crate::render::paint::control::Control::Progress {
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
            let (edge, fill) = crate::render::paint::control::box_colors(look);
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
        if look.control == crate::render::paint::control::Control::Menu {
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

/// Draws the native look of a control or the resizer of a textarea, above the box.
#[inline(never)]
pub(super) fn control_layer(
    cx: &Cx<'_>,
    fr: &Frame,
    mut el: renderer::Node,
    native: Option<crate::render::paint::control::Look>,
) -> renderer::Node {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        ..
    } = *cx;
    let Build { styles, view, .. } = *reader;
    let padding = fr.padding;
    let resizer = (node.tag == "textarea"
        && style.get(Prop::RESIZE).is_none_or(|value| value != "none"))
    .then_some(crate::render::paint::control::Look {
        control: crate::render::paint::control::Control::Resizer,
        ..Default::default()
    });
    if let Some(look) = native
        .filter(|look| {
            !matches!(
                look.control,
                crate::render::paint::control::Control::Button
                    | crate::render::paint::control::Control::Field
            )
        })
        .or(resizer)
    {
        el = el.child(if look.plain {
            let parts = styles.extras(id).scrollbars.as_ref();
            let range = look.control == crate::render::paint::control::Control::Range;
            let ink = css::value::color::rgba(
                style
                    .get(Prop::COLOR)
                    .and_then(css::value::color::color)
                    .unwrap_or((0, 255)),
            );
            // Size, margin-top, border width, radius, fill and border color of the track and thumb, or of the
            // bar and value of a progress bar, with the defaults of Chrome. Both have `box-sizing: border-box`.
            let [track, thumb] = [1, 2].map(|slot| {
                let part = parts.and_then(|parts| parts[slot].as_ref());
                let get = |prop| part.and_then(|part| part.get(prop));
                let length = |prop| get(prop).and_then(|value| units.resolve(value, f32::NAN));
                let color = |prop| get(prop).and_then(css::value::color::paint);
                let native = range
                    && slot == 2
                    && [Prop::APPEARANCE, Prop::WEBKIT_APPEARANCE]
                        .into_iter()
                        .all(|prop| get(prop).is_none_or(|value| value != "none"));
                let border = get(Prop::BORDER_TOP_STYLE)
                    .is_some_and(|value| !matches!(value, "none" | "hidden"))
                    .then(|| length(Prop::BORDER_TOP_WIDTH))
                    .flatten()
                    .unwrap_or(0.);
                let fill = match (range, slot) {
                    _ if native => Some(css::value::color::rgba((look.accent, 255))),
                    (false, 1) => Some(renderer::Color::hex(0x808080ff)),
                    (false, _) => Some(renderer::Color::hex(0x008000ff)),
                    _ => None,
                };
                (
                    [Prop::WIDTH, Prop::HEIGHT].map(|prop| length(prop).or(native.then_some(16.))),
                    length(Prop::MARGIN_TOP).unwrap_or(0.),
                    border,
                    if native {
                        8.
                    } else {
                        get(Prop::BORDER_TOP_LEFT_RADIUS).map_or(0., |value| {
                            // Only round ends matter here, and the box clamps large radii.
                            if value.contains('%') {
                                1e4
                            } else {
                                units.resolve(value, f32::NAN).unwrap_or(0.)
                            }
                        })
                    },
                    color(Prop::BACKGROUND_COLOR).or(fill),
                    color(Prop::BORDER_TOP_COLOR).unwrap_or(ink),
                )
            });
            let (value, inset) = (look.value, padding);
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| {
                    let content = Rect::between(
                        bounds.origin + point(px(inset[3]), px(inset[0])),
                        bounds.bottom_right() - point(px(inset[1]), px(inset[2])),
                    );
                    let [width, height] = [content.size.width, content.size.height].map(f32::from);
                    let thumb_size = thumb.0.map(|side| side.unwrap_or(0.));
                    let track_height = match track.0[1] {
                        Some(side) if range => side,
                        _ if range => thumb_size[1] + 2. * track.2,
                        _ => height,
                    };
                    let track_top = (height - track_height) / 2.;
                    let inner = width - 2. * track.2;
                    let rects = [
                        (track, [0., track_top, width, track_height]),
                        if range {
                            (
                                thumb,
                                [
                                    track.2 + (inner - thumb_size[0]) * value,
                                    track_top + track.2 + thumb.1,
                                    thumb_size[0],
                                    thumb_size[1],
                                ],
                            )
                        } else {
                            (
                                thumb,
                                [
                                    track.2,
                                    track.2,
                                    inner * value.max(0.),
                                    height - 2. * track.2,
                                ],
                            )
                        },
                    ];
                    for ((_, _, border, radius, fill, edge), [x, y, w, h]) in rects {
                        if w <= 0. || h <= 0. {
                            continue;
                        }
                        let rect =
                            Rect::new(content.origin + point(px(x), px(y)), size(px(w), px(h)));
                        window.draw_box(
                            rect,
                            renderer::Radii::from(px(radius)).fit(rect.size),
                            fill.unwrap_or(renderer::Color::TRANSPARENT),
                            renderer::Border {
                                widths: px(border).into(),
                                color: edge,
                                style: renderer::BorderStyle::Solid,
                                sides: 0,
                            },
                        );
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into()
        } else {
            crate::render::paint::control::layer(id, look, view.clone())
        });
    }
    el
}
