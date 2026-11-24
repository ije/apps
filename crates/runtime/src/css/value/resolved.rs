use crate::css::value::grammar::grammar;
use crate::css::value::specified::math_sum;
use crate::css::{
    properties::BORDERS, properties::PADDING, properties::Prop, properties::Registry,
    properties::Style, value::area_cells, value::corner_shape, value::grammar::INITIAL,
    value::grammar::SHORTHANDS, value::shorthand::BORDER_IMAGE, value::shorthand::expand,
    value::specified::specified, value::tokens::components, value::tokens::number,
    value::tokens::write_printed, value::units::Kind, value::units::Lengths,
};
use crate::css::{
    properties::LOGICAL, value::color::serialize as serialize_color, value::specified::shortest,
};
use std::borrow::Cow;
pub(crate) fn resolved(
    style: &Style,
    units: &Lengths,
    name: &str,
    size: Option<[f32; 2]>,
    container: &dyn Fn() -> [f32; 2],
    registry: &Registry,
) -> String {
    let cell = std::cell::OnceCell::new();
    let container = &|| *cell.get_or_init(container);
    let name = &*physical_name(style, name);
    if name.starts_with("--") {
        return Prop::find(name)
            .and_then(|prop| {
                style
                    .value(prop)
                    .or_else(|| registry.get(&prop)?.2.as_ref())
            })
            .map_or_else(String::new, |value| value.to_string());
    }
    let get = |name: &str| resolved(style, units, name, size, container, registry);
    // The item count comes from the longhand that defines it, because not every shorthand is stored.
    let list = |shorthand: &str, key: &str, defining: &str| {
        let parts = |key: &str| -> Vec<String> {
            components(&get(&format!("{shorthand}-{key}")), ',')
                .into_iter()
                .map(|part| part.trim().to_owned())
                .collect()
        };
        let count = parts(defining).len().max(1);
        let parts = parts(key);
        (0..count)
            .map(|index| {
                parts
                    .get(index % parts.len().max(1))
                    .cloned()
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
    };
    match name {
        "font" => {
            let mut parts = Vec::new();
            for (longhand, initial) in [
                ("font-style", "normal"),
                ("font-variant", "normal"),
                ("font-weight", "400"),
                ("font-stretch", "100%"),
            ] {
                let mut value = get(longhand);
                // The shorthand takes only the keywords of `font-stretch`.
                if longhand == "font-stretch" && value != initial {
                    let keywords = [
                        "50%", "62.5%", "75%", "87.5%", "100%", "112.5%", "125%", "150%", "200%",
                    ];
                    let Some(index) = keywords.iter().position(|keyword| *keyword == value) else {
                        return String::new();
                    };
                    value = [
                        "ultra-condensed",
                        "extra-condensed",
                        "condensed",
                        "semi-condensed",
                        "normal",
                        "semi-expanded",
                        "expanded",
                        "extra-expanded",
                        "ultra-expanded",
                    ][index]
                        .into();
                }
                if value != initial && value != "normal" {
                    parts.push(value);
                }
            }
            let line = get("line-height");
            parts.push(if line == "normal" {
                get("font-size")
            } else {
                format!("{} / {line}", get("font-size"))
            });
            parts.push(get("font-family"));
            return parts.join(" ");
        }
        "background" => {
            let images = list("background", "image", "image");
            let layers: Vec<String> = (0..images.len())
                .map(|index| {
                    let pick = |key: &str| list("background", key, "image").swap_remove(index);
                    let mut layer = format!(
                        "{} {} {} {} / {} {} {}",
                        images[index],
                        pick("repeat"),
                        pick("attachment"),
                        pick("position"),
                        pick("size"),
                        pick("origin"),
                        pick("clip")
                    );
                    if index + 1 == images.len() {
                        layer = format!("{} {layer}", get("background-color"));
                    }
                    layer
                })
                .collect();
            return layers.join(", ");
        }
        "transition" | "animation" => {
            let defining = if name == "transition" {
                "property"
            } else {
                "name"
            };
            let keys: &[(&str, &str)] = if name == "transition" {
                &[
                    ("property", "all"),
                    ("duration", "0s"),
                    ("timing-function", "ease"),
                    ("delay", "0s"),
                    ("behavior", "normal"),
                ]
            } else {
                &[
                    ("duration", "0s"),
                    ("timing-function", "ease"),
                    ("delay", "0s"),
                    ("iteration-count", "1"),
                    ("direction", "normal"),
                    ("fill-mode", "none"),
                    ("play-state", "running"),
                    ("name", "none"),
                ]
            };
            let columns: Vec<Vec<String>> = keys
                .iter()
                .map(|(key, _)| list(name, key, defining))
                .collect();
            let count = columns.iter().map(Vec::len).max().unwrap_or(1);
            let delay = keys
                .iter()
                .position(|(key, _)| *key == "delay")
                .unwrap_or(0);
            return (0..count)
                .map(|index| {
                    let at = |column: &Vec<String>| column.get(index).cloned().unwrap_or_default();
                    // Like Chrome, a delay needs its duration, and an item of only initial values keeps its name.
                    let kept = keys
                        .iter()
                        .zip(&columns)
                        .filter_map(|((key, initial), values)| {
                            let value = at(values);
                            (value != *initial || *key == "duration" && at(&columns[delay]) != "0s")
                                .then_some(value)
                        })
                        .collect::<Vec<_>>();
                    if kept.is_empty() {
                        at(&columns[if name == "transition" {
                            0
                        } else {
                            keys.len() - 1
                        }])
                    } else {
                        kept.join(" ")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
        }
        "border-radius" => {
            let corners: Vec<Vec<String>> = [
                "border-top-left-radius",
                "border-top-right-radius",
                "border-bottom-right-radius",
                "border-bottom-left-radius",
            ]
            .iter()
            .map(|corner| {
                components(&get(corner), ' ')
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            })
            .collect();
            let axis = |index: usize| {
                shortest(std::array::from_fn(|corner| {
                    let corner = &corners[corner];
                    corner
                        .get(index)
                        .or(corner.first())
                        .map_or("0px", String::as_str)
                }))
            };
            let (horizontal, vertical) = (axis(0), axis(1));
            return if horizontal == vertical {
                horizontal
            } else {
                format!("{horizontal} / {vertical}")
            };
        }
        "outline" => {
            return ["outline-color", "outline-style", "outline-width"]
                .map(get)
                .join(" ");
        }
        "text-decoration" => {
            let mut parts = vec![get("text-decoration-line")];
            for (longhand, initial) in [
                ("text-decoration-thickness", "auto"),
                ("text-decoration-style", "solid"),
            ] {
                let value = get(longhand);
                if value != initial {
                    parts.push(value);
                }
            }
            // The cascade can store `currentcolor` as the color it stands for, so equal colors count as it.
            let color = get("text-decoration-color");
            if color != get("color") {
                parts.push(color);
            }
            return parts.join(" ");
        }
        "border-image" => {
            let [source, slice, width, outset, repeat] = BORDER_IMAGE.map(get);
            return if source == "none" {
                source
            } else {
                format!("{source} {slice} / {width} / {outset} {repeat}")
            };
        }
        "border-block-start" | "border-block-end" | "border-inline-start" | "border-inline-end" => {
            let width = format!("{name}-width");
            return get(physical_name(style, &width).trim_end_matches("-width"));
        }
        "container" => {
            let (name, kind) = (get("container-name"), get("container-type"));
            return if kind == "normal" {
                name
            } else {
                format!("{name} / {kind}")
            };
        }
        "border-block" | "border-inline" => {
            let [start, end] = if name == "border-block" {
                ["border-top", "border-bottom"]
            } else {
                ["border-left", "border-right"]
            }
            .map(get);
            return if start == end { start } else { String::new() };
        }
        "margin-block" | "margin-inline" | "padding-block" | "padding-inline" | "inset-block"
        | "inset-inline" => {
            let mut sides = Vec::new();
            expand(name, "0 1", &mut sides);
            let [start, end] = [0, sides.len().saturating_sub(1)].map(|index| {
                sides
                    .get(index)
                    .map_or_else(String::new, |(side, _)| get(side))
            });
            return if start == end {
                start
            } else {
                format!("{start} {end}")
            };
        }
        "border" => {
            let sides = ["border-top", "border-right", "border-bottom", "border-left"].map(get);
            return if sides.iter().all(|side| *side == sides[0]) {
                sides[0].clone()
            } else {
                String::new()
            };
        }
        "flex-flow" => return format!("{} {}", get("flex-direction"), get("flex-wrap")),
        "columns" => {
            let (width, count) = (get("column-width"), get("column-count"));
            return match (width.as_str(), count.as_str()) {
                ("auto", "auto") => "auto".into(),
                ("auto", _) => count,
                (_, "auto") => width,
                _ => format!("{width} {count}"),
            };
        }
        "grid-row" | "grid-column" => {
            let start = get(&format!("{name}-start"));
            let end = get(&format!("{name}-end"));
            return if end == grid_default(&start) {
                start
            } else {
                format!("{start} / {end}")
            };
        }
        "grid-area" => {
            let mut parts: Vec<String> = [
                "grid-row-start",
                "grid-column-start",
                "grid-row-end",
                "grid-column-end",
            ]
            .iter()
            .map(|longhand| get(longhand))
            .collect();
            // Like Chrome, a line that its start line implies is left out.
            while parts.len() > 1
                && parts[parts.len() - 1] == grid_default(&parts[usize::from(parts.len() == 4)])
            {
                parts.pop();
            }
            return parts.join(" / ");
        }
        "grid-template" => {
            let (rows, columns, areas) = (
                get("grid-template-rows"),
                get("grid-template-columns"),
                get("grid-template-areas"),
            );
            if areas == "none" {
                return format!("{rows} / {columns}");
            }
            let names: Vec<&str> = areas
                .split('"')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .collect();
            let sizes = components(&rows, ' ');
            let lines: Vec<String> = names
                .iter()
                .enumerate()
                .map(|(index, row)| match sizes.get(index) {
                    Some(size) if *size != "auto" => format!("\"{row}\" {size}"),
                    _ => format!("\"{row}\""),
                })
                .collect();
            return format!("{} / {columns}", lines.join(" "));
        }
        "grid" => {
            return [
                "grid-template-rows",
                "grid-template-columns",
                "grid-template-areas",
                "grid-auto-flow",
                "grid-auto-rows",
                "grid-auto-columns",
            ]
            .map(get)
            .join(" / ");
        }
        _ => {}
    }
    if let Some((_, longhands)) = SHORTHANDS.iter().find(|(shorthand, _)| *shorthand == name) {
        let values: Vec<_> = longhands
            .iter()
            .map(|longhand| resolved(style, units, longhand, size, container, registry))
            .collect();
        return match values.as_slice() {
            [top, right, bottom, left] => shortest([top, right, bottom, left].map(String::as_str)),
            [a, b] if a == b => a.clone(),
            _ => values.join(" "),
        };
    }
    let Some(prop) = Prop::find(name) else {
        return String::new();
    };
    let initial = INITIAL
        .binary_search_by(|(property, _)| (*property).cmp(name))
        .ok()
        .map(|index| INITIAL[index].1);
    let shorthand = |shorthand: Prop, key: &str| {
        let list = components(style.get(shorthand)?, ',');
        let parts: Vec<_> = list
            .iter()
            .map(|item| {
                let mut times = 0;
                let parts = components(item, ' ');
                let found = parts.into_iter().find(|part| {
                    let time = part.ends_with('s')
                        && part.trim_end_matches(['m', 's']).parse::<f32>().is_ok()
                        || part.ends_with(')')
                            && units
                                .math(part, f32::NAN, &[])
                                .is_some_and(|(_, kind)| kind == Kind::Time);
                    times += usize::from(time);
                    match key {
                        "duration" => time && times == 1,
                        "delay" => time && times == 2,
                        "iteration-count" => *part == "infinite" || part.parse::<f32>().is_ok(),
                        "direction" => matches!(
                            *part,
                            "normal" | "reverse" | "alternate" | "alternate-reverse"
                        ),
                        "fill-mode" => matches!(*part, "forwards" | "backwards" | "both"),
                        "play-state" => matches!(*part, "paused" | "running"),
                        "behavior" => matches!(*part, "allow-discrete"),
                        "timing-function" => {
                            matches!(
                                *part,
                                "linear"
                                    | "ease"
                                    | "ease-in"
                                    | "ease-out"
                                    | "ease-in-out"
                                    | "step-start"
                                    | "step-end"
                            ) || part.starts_with("cubic-bezier(")
                                || part.starts_with("steps(")
                                || part.starts_with("linear(")
                        }
                        _ => {
                            !time
                                && part.parse::<f32>().is_err()
                                && !matches!(
                                    *part,
                                    "infinite"
                                        | "normal"
                                        | "reverse"
                                        | "alternate"
                                        | "alternate-reverse"
                                        | "forwards"
                                        | "backwards"
                                        | "both"
                                        | "none"
                                        | "paused"
                                        | "running"
                                        | "linear"
                                        | "ease"
                                        | "ease-in"
                                        | "ease-out"
                                        | "ease-in-out"
                                        | "step-start"
                                        | "step-end"
                                        | "allow-discrete"
                                )
                                && !part.contains('(')
                        }
                    }
                });
                found.map_or_else(
                    || initial.unwrap_or_default().to_owned(),
                    |part| part.to_owned(),
                )
            })
            .collect();
        Some(parts.join(", "))
    };
    let value = match style.get(prop) {
        Some(value) => value.to_owned(),
        None => match name.split_once('-') {
            Some(("animation", key)) => shorthand(Prop::ANIMATION, key),
            Some(("transition", "property")) if style.get(Prop::TRANSITION) == Some("none") => {
                Some("none".to_owned())
            }
            Some(("transition", key)) => shorthand(Prop::TRANSITION, key),
            _ => None,
        }
        .unwrap_or_else(|| initial.unwrap_or_default().to_owned()),
    };
    // Empty `var()` references leave their spaces, which Chrome's tokens don't keep.
    let value = if (value.contains("  ") || value.starts_with(' ') || value.ends_with(' '))
        && !value.contains(['"', '\''])
    {
        value.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        value
    };
    let number = |value: f32| number(f64::from(value));
    let printed = |value: f64| {
        let mut out = String::new();
        write_printed(value, &mut out);
        out
    };
    // Chrome's layout units are 1/64 px, and a float becomes one by truncation. Our layout can be off by a little, so a size within 0.01px of a whole pixel is that pixel.
    let layout_unit = |value: f32| match value.round() {
        whole if (value - whole).abs() < 0.01 => whole,
        _ => (value * 64.).trunc() / 64.,
    };
    let px = |value: f32| {
        let mut out = String::with_capacity(12);
        write_printed(f64::from(value), &mut out);
        out.push_str("px");
        out
    };

    let vertical = matches!(
        name,
        "top" | "bottom" | "height" | "min-height" | "max-height"
    );
    // A sum of a percentage and a length as Chrome's text. Like Chrome, a percentage alone is clamped for a
    // property that takes no negative values.
    let percents = |part: &str| match percent_sum(part, units) {
        Some((percent, 0.))
            if percent < 0. && grammar(name).is_some_and(|g| g.contains("<non-negative-")) =>
        {
            "0%".to_owned()
        }
        Some(sum) => percent_text(sum),
        None => match part
            .get(..5)
            .is_some_and(|start| start.eq_ignore_ascii_case("calc("))
        {
            true => math_sum(&part[5..part.len() - 1], Some(units))
                .map_or_else(|| part.to_owned(), |sum| format!("calc({sum})")),
            false => part.to_owned(),
        },
    };
    let length = |value: &str| {
        let base = match value.contains('%') {
            // A box that is not rendered has no used values, so its percentages stay.
            true if size.is_none() => return percents(value),
            true => container()[usize::from(vertical)],
            false => 0.,
        };
        units
            .resolve(value, base)
            .map_or_else(|| value.to_owned(), px)
    };
    let current = style.get(Prop::COLOR).unwrap_or("canvastext");
    let paint = |value: &str| {
        let value = if value.eq_ignore_ascii_case("currentcolor") {
            current
        } else {
            value
        };
        match serialize_color(value) {
            Some(text) => text,
            None => value.to_owned(),
        }
    };
    let none = |prop: Prop| {
        style
            .get(prop)
            .is_none_or(|value| matches!(value, "none" | "hidden"))
    };
    match name {
        // Like Chrome, keywords compute to their superellipse.
        _ if name.starts_with("corner-") => match corner_shape(&value) {
            Some(k) if k.is_infinite() => {
                format!("superellipse({}infinity)", if k < 0. { "-" } else { "" })
            }
            Some(k) => format!("superellipse({})", crate::css::value::tokens::number(k)),
            None => value,
        },
        "border-top-width" if none(Prop::BORDER_TOP_STYLE) => "0px".into(),
        "border-right-width" if none(Prop::BORDER_RIGHT_STYLE) => "0px".into(),
        "border-bottom-width" if none(Prop::BORDER_BOTTOM_STYLE) => "0px".into(),
        "border-left-width" if none(Prop::BORDER_LEFT_STYLE) => "0px".into(),
        "column-rule-width" if none(Prop::COLUMN_RULE_STYLE) => "0px".into(),
        "width" | "height"
            if size.is_some()
                && !style
                    .get(Prop::DISPLAY)
                    .is_some_and(|display| display == "inline") =>
        {
            let [width, height] = size.unwrap_or_default();
            let border_box = style.get(Prop::BOX_SIZING) == Some("border-box");
            let [padding, border] = [
                PADDING,
                [
                    Prop::BORDER_TOP_WIDTH,
                    Prop::BORDER_RIGHT_WIDTH,
                    Prop::BORDER_BOTTOM_WIDTH,
                    Prop::BORDER_LEFT_WIDTH,
                ],
            ]
            .map(|props| units.edges(style, props, container()[0]));
            let border = std::array::from_fn::<f32, 4, _>(|side| {
                if none(BORDERS[side][1]) {
                    0.
                } else {
                    border[side]
                }
            });
            px(layout_unit(
                if border_box {
                    if name == "width" { width } else { height }
                } else if name == "width" {
                    width - padding[1] - padding[3] - border[1] - border[3]
                } else {
                    height - padding[0] - padding[2] - border[0] - border[2]
                }
                .max(0.),
            ))
        }
        "line-height" if value != "normal" => match units.parse(&value, units.font.em) {
            Some((number, false)) => px(value.parse().unwrap_or(number) * units.font.em),
            Some((length, true)) => px(length),
            None => value,
        },
        "grid-template-areas" if value != "none" => value
            .split(['"', '\''])
            .map(str::trim)
            .filter(|row| !row.is_empty())
            .map(|row| format!("\"{}\"", area_cells(row).join(" ")))
            .collect::<Vec<_>>()
            .join(" "),
        "font-family" => components(&value, ',')
            .into_iter()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(", "),
        _ if name.ends_with("duration") || name.ends_with("delay") => components(&value, ',')
            .into_iter()
            .map(|time| {
                let time = time.trim();
                if time.ends_with(')')
                    && let Some((seconds, Kind::Time)) = units.math(time, f32::NAN, &[])
                {
                    return format!("{}s", printed(seconds));
                }
                match time
                    .strip_suffix("ms")
                    .map(|ms| (ms, 1000.))
                    .or_else(|| time.strip_suffix('s').map(|s| (s, 1.)))
                {
                    Some((number, scale)) => number.parse::<f32>().map_or_else(
                        |_| time.to_owned(),
                        |number| format!("{}s", printed(f64::from(number / scale))),
                    ),
                    None => time.to_owned(),
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
        "opacity" | "fill-opacity" | "stroke-opacity" | "flood-opacity" => {
            match value.strip_suffix('%') {
                Some(percent) => percent.parse::<f64>().map_or(value.clone(), |percent| {
                    printed((percent / 100.).clamp(0., 1.))
                }),
                None => value.parse::<f64>().map_or_else(
                    |_| {
                        units
                            .math(&value, 1., &[])
                            .map_or(value.clone(), |(alpha, _)| printed(alpha.clamp(0., 1.)))
                    },
                    |value| printed(value.clamp(0., 1.)),
                ),
            }
        }
        // Like Chrome, a box with `display: none` has no transform.
        "transform" if style.get(Prop::DISPLAY) == Some("none") => "none".into(),
        // Like Chrome, SVG lengths compute to `px` and a dash list is separated by commas.
        "stroke-width" | "stroke-dashoffset" | "stroke-dasharray" | "r" | "cx" | "cy" | "rx"
        | "ry" | "x" | "y"
            if !matches!(value.as_str(), "none" | "auto") =>
        {
            if value == "0px" {
                return value;
            }
            // Parts are separated by commas or spaces, so a dash list needs no allocation per part.
            let mut out = String::with_capacity(value.len() + 8);
            for part in value.split([',', ' ']).filter(|part| !part.is_empty()) {
                if !out.is_empty() {
                    out.push_str(", ");
                }
                if part.contains(['%', '(']) {
                    return components(&value.replace(',', " "), ' ')
                        .into_iter()
                        .map(|part| match part.contains('%') {
                            true => percents(part),
                            false => units
                                .parse(part, f32::NAN)
                                .map_or_else(|| part.to_owned(), |(length, _)| px(length)),
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                }
                match units.parse(part, f32::NAN) {
                    Some((length, _)) => {
                        write_printed(f64::from(length), &mut out);
                        out.push_str("px");
                    }
                    None => out.push_str(part),
                }
            }
            out
        }
        "transform" if value != "none" => {
            let [width, height] = size.unwrap_or_default();
            let mut flat = style.clone();
            flat.insert(Prop::TRANSFORM_ORIGIN, "0 0");
            let bounds = renderer::Rect::new(
                renderer::point(renderer::px(0.), renderer::px(0.)),
                renderer::size(renderer::px(width), renderer::px(height)),
            );
            match crate::css::value::transform::transform(&flat, units, bounds, None) {
                Some(m)
                    if [m[2], m[3], m[6], m[7], m[8], m[9], m[11], m[14]] == [0.; 8]
                        && m[10] == 1.
                        && m[15] == 1. =>
                {
                    format!(
                        "matrix({}, {}, {}, {}, {}, {})",
                        number(m[0]),
                        number(m[1]),
                        number(m[4]),
                        number(m[5]),
                        printed(f64::from(m[12])),
                        printed(f64::from(m[13]))
                    )
                }
                Some(m) => format!("matrix3d({})", m.map(number).join(", ")),
                None if value.contains('(') => "matrix(1, 0, 0, 1, 0, 0)".into(),
                None => value,
            }
        }
        "box-shadow" | "text-shadow" if value != "none" => components(&value, ',')
            .into_iter()
            .map(|shadow| {
                let (mut ink, mut lengths, mut inset) = (current.to_owned(), Vec::new(), false);
                for part in components(shadow, ' ') {
                    if part == "inset" {
                        inset = true;
                    } else if let Some(size) = units.resolve(part, f32::NAN) {
                        lengths.push(px(size));
                    } else {
                        ink = part.to_owned();
                    }
                }
                lengths.resize(if name == "box-shadow" { 4 } else { 3 }, "0px".into());
                let mut text = format!("{} {}", paint(&ink), lengths.join(" "));
                if inset {
                    text.push_str(" inset");
                }
                text
            })
            .collect::<Vec<_>>()
            .join(", "),
        "flex-basis" if value.ends_with('%') => value,
        // Like Chrome, a size computes to pixels, and one length gets a second `auto`.
        "background-size" | "mask-size" => components(&value, ',')
            .into_iter()
            .map(|layer| {
                let mut parts: Vec<String> = components(layer.trim(), ' ')
                    .into_iter()
                    .map(|part| match part.contains('%') {
                        true => percents(part),
                        false => units
                            .resolve(part, f32::NAN)
                            .map_or_else(|| part.to_owned(), px),
                    })
                    .collect();
                if let [size] = &parts[..]
                    && !matches!(size.as_str(), "auto" | "cover" | "contain")
                {
                    parts.push("auto".to_owned());
                }
                parts.join(" ")
            })
            .collect::<Vec<_>>()
            .join(", "),
        "overflow-x" | "overflow-y" if matches!(value.as_str(), "visible" | "clip") => {
            let other = if name == "overflow-x" {
                Prop::OVERFLOW_Y
            } else {
                Prop::OVERFLOW_X
            };
            match style.get(other) {
                Some(other) if !matches!(other, "visible" | "clip") => {
                    if value == "visible" { "auto" } else { "hidden" }.to_owned()
                }
                _ => value,
            }
        }
        "background-position" | "object-position" | "mask-position" => components(&value, ',')
            .into_iter()
            .map(|layer| {
                let mut parts: Vec<&str> = components(layer, ' ');
                let edge =
                    |part: &str| matches!(part, "left" | "right" | "top" | "bottom" | "center");
                // Edges with offsets, like `right 10px bottom`.
                if parts.len() > 2 {
                    let mut axes: [Option<String>; 2] = [None, None];
                    let mut rest = &parts[..];
                    while let [side, tail @ ..] = rest {
                        let offset = tail.first().filter(|part| !edge(part));
                        rest = &tail[usize::from(offset.is_some())..];
                        if *side == "center" {
                            continue;
                        }
                        let axis = usize::from(matches!(*side, "top" | "bottom"));
                        let far = matches!(*side, "right" | "bottom");
                        let value = match offset {
                            None => if far { "100%" } else { "0%" }.to_owned(),
                            Some(offset) => {
                                match offset.strip_suffix('%').and_then(|n| n.parse::<f32>().ok()) {
                                    Some(percent) if far => format!("{}%", number(100. - percent)),
                                    _ if let Some((percent, pixels)) =
                                        percent_sum(offset, units) =>
                                    {
                                        percent_text(if far {
                                            (100. - percent, -pixels)
                                        } else {
                                            (percent, pixels)
                                        })
                                    }
                                    _ => {
                                        let offset = units
                                            .resolve(offset, f32::NAN)
                                            .map_or_else(|| (*offset).to_owned(), px);
                                        if far {
                                            format!("calc(100% - {offset})")
                                        } else {
                                            offset
                                        }
                                    }
                                }
                            }
                        };
                        axes[axis] = Some(value);
                    }
                    return axes
                        .map(|value| value.unwrap_or_else(|| "50%".into()))
                        .join(" ");
                }
                if parts.len() == 1 {
                    parts.insert(usize::from(!matches!(parts[0], "top" | "bottom")), "center");
                }
                if parts.len() == 2
                    && (matches!(parts[0], "top" | "bottom")
                        || matches!(parts[1], "left" | "right"))
                {
                    parts.swap(0, 1);
                }
                parts
                    .into_iter()
                    .map(|part| match part {
                        "left" | "top" => "0%".to_owned(),
                        "center" => "50%".to_owned(),
                        "right" | "bottom" => "100%".to_owned(),
                        part if part.contains('%') => percents(part),
                        part => units
                            .resolve(part, f32::NAN)
                            .map_or_else(|| part.to_owned(), px),
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join(", "),
        "clip-path" | "shape-outside" if value.starts_with("inset(") && value.ends_with(')') => {
            let body = &value[6..value.len() - 1];
            let (sides, round) = body
                .split_once(" round ")
                .map_or((body, None), |(sides, round)| (sides, Some(round.trim())));
            let sides = match components(sides, ' ')[..] {
                [a] => [a; 4],
                [a, b] => [a, b, a, b],
                [a, b, c] => [a, b, c, b],
                [a, b, c, d] => [a, b, c, d],
                _ => return value,
            };
            // Like Chrome, each side and radius is a computed length-percentage.
            let computed = |part: &str| match part.contains('%') {
                true => percents(part),
                false => units
                    .resolve(part, f32::NAN)
                    .map_or_else(|| part.to_owned(), px),
            };
            let sides = sides.map(computed);
            let round = round.map(|round| {
                components(round, ' ')
                    .into_iter()
                    .map(|part| {
                        if part == "/" {
                            part.to_owned()
                        } else {
                            computed(part)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            });
            let mut result = format!("inset({}", shortest(sides.each_ref().map(String::as_str)));
            if let Some(round) = round {
                result.push_str(" round ");
                result.push_str(&round);
            }
            result.push(')');
            result
        }
        "transform-origin" | "perspective-origin" => {
            let [width, height] = size.unwrap_or_default();
            let mut parts = components(&value, ' ');
            if parts.len() == 1 {
                parts.insert(usize::from(!matches!(parts[0], "top" | "bottom")), "center");
            }
            if parts.len() > 1
                && (matches!(parts[0], "top" | "bottom") || matches!(parts[1], "left" | "right"))
            {
                parts.swap(0, 1);
            }
            parts
                .into_iter()
                .enumerate()
                .map(|(index, part)| {
                    let part = match part {
                        "left" | "top" => "0%",
                        "center" => "50%",
                        "right" | "bottom" => "100%",
                        part => part,
                    };
                    if size.is_none() && part.contains('%') {
                        return part.to_owned();
                    }
                    units
                        .resolve(part, if index == 0 { width } else { height })
                        .map_or_else(|| part.to_owned(), |value| px((value * 64.).round() / 64.))
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        "border-image-slice"
        | "border-image-width"
        | "border-image-outset"
        | "border-image-repeat" => components(&specified(name, &value), ' ')
            .into_iter()
            .map(|part| {
                if part.starts_with(|c: char| c.is_ascii_digit() || c == '.')
                    && part.ends_with(|c: char| c.is_ascii_alphabetic())
                {
                    length(part)
                } else if part.starts_with("calc(") {
                    percents(part)
                } else {
                    part.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        "caret-color" if value == "auto" => paint(current),
        // Like Chrome, each counter gets its number.
        "counter-reset" | "counter-increment" | "counter-set"
            if value != "none" && !value.contains('(') =>
        {
            let default = if name == "counter-increment" {
                "1"
            } else {
                "0"
            };
            let parts = components(&value, ' ');
            let mut out = Vec::new();
            for (index, part) in parts.iter().enumerate() {
                out.push(*part);
                let next = parts.get(index + 1);
                if part.parse::<i32>().is_err()
                    && next.is_none_or(|next| next.parse::<i32>().is_err())
                {
                    out.push(default);
                }
            }
            out.join(" ")
        }
        "grid-template-columns" | "grid-template-rows" | "grid-auto-columns" | "grid-auto-rows" => {
            let mut out = String::with_capacity(value.len());
            tracks(&value, units, &mut out);
            out
        }
        "zoom" if value == "normal" => "1".to_owned(),
        "rotate" | "translate" | "scale" | "zoom" if value != "none" => {
            let mut parts: Vec<String> = components(&value, ' ')
                .into_iter()
                .map(|part| match units.math(part, f32::NAN, &[]) {
                    _ if name == "translate" && part.contains('%') => percents(part),
                    _ if part.ends_with('%') => part[..part.len() - 1]
                        .parse::<f32>()
                        .map_or_else(|_| part.to_owned(), |percent| number(percent / 100.)),
                    Some((amount, Kind::Angle)) => format!("{}deg", number(amount as f32)),
                    Some((amount, Kind::Length)) => px(amount as f32),
                    Some((amount, _)) if name == "translate" => px(amount as f32),
                    Some((amount, _)) => number(amount as f32),
                    None => part.to_owned(),
                })
                .collect();
            // Like Chrome, a translation leaves out its trailing zero offsets, a scale its trailing ones, and a
            // rotation its axis when the axis is x, y or z.
            match name {
                "translate" => {
                    while parts.len() > 1 && parts.last().is_some_and(|part| part == "0px") {
                        parts.pop();
                    }
                }
                "scale" => {
                    if parts.len() == 3 && parts[2] == "1" {
                        parts.pop();
                    }
                    if parts.len() == 2 && parts[0] == parts[1] {
                        parts.pop();
                    }
                }
                "rotate" if parts.len() == 2 && parts[0] == "z" => {
                    parts.remove(0);
                }
                _ if parts.len() == 4 => {
                    // An axis along x, y or z is its name, and a negative one turns the other way.
                    let axis: Vec<f32> = parts[..3]
                        .iter()
                        .filter_map(|part| part.parse().ok())
                        .collect();
                    let along = axis.iter().filter(|value| **value != 0.).count() == 1;
                    if let (true, Some(at), Some(angle)) = (
                        along && axis.len() == 3,
                        axis.iter().position(|value| *value != 0.),
                        parts[3]
                            .strip_suffix("deg")
                            .and_then(|angle| angle.parse::<f32>().ok()),
                    ) {
                        let angle = format!("{}deg", number(angle * axis[at].signum()));
                        parts = vec![format!("{}{angle}", ["x ", "y ", ""][at])];
                    }
                }
                _ => {}
            }
            parts.join(" ")
        }
        "aspect-ratio" => {
            let (auto, ratio) = match value
                .strip_prefix("auto")
                .or_else(|| value.strip_suffix("auto"))
            {
                Some(rest) if !rest.trim().is_empty() => ("auto ", rest.trim()),
                _ => ("", value.as_str()),
            };
            match ratio.split_once('/') {
                _ if ratio == "auto" => value,
                Some((width, height)) => format!("{auto}{} / {}", width.trim(), height.trim()),
                None => format!("{auto}{ratio} / 1"),
            }
        }
        // Like Chrome, a zero letter spacing computes to `normal` and `normal` word spacing to `0px`.
        "word-spacing" if value == "normal" => "0px".to_owned(),
        "letter-spacing" if !value.contains('%') && units.resolve(&value, 0.) == Some(0.) => {
            "normal".to_owned()
        }
        "grid-auto-flow" if value.contains("dense") => if value.contains("column") {
            "column dense"
        } else {
            "dense"
        }
        .to_owned(),
        "text-emphasis" => {
            let (mut fill, mut shape, mut color) = (None, None, "currentcolor");
            for part in components(&value, ' ') {
                match part {
                    "filled" | "open" => fill = Some(part),
                    "none" | "dot" | "circle" | "double-circle" | "triangle" | "sesame" => {
                        shape = Some(part)
                    }
                    _ if part.starts_with(['"', '\'']) => shape = Some(part),
                    _ => color = part,
                }
            }
            let style = match (fill, shape) {
                (None, None) => "none".to_owned(),
                (Some("open"), shape) => format!("open {}", shape.unwrap_or("dot")),
                (_, shape) => shape.unwrap_or("dot").to_owned(),
            };
            format!("{style} {}", paint(color))
        }
        "font-stretch" => match value.as_str() {
            "ultra-condensed" => "50%",
            "extra-condensed" => "62.5%",
            "condensed" => "75%",
            "semi-condensed" => "87.5%",
            "normal" => "100%",
            "semi-expanded" => "112.5%",
            "expanded" => "125%",
            "extra-expanded" => "150%",
            "ultra-expanded" => "200%",
            _ => &value,
        }
        .to_owned(),
        _ if name.ends_with("color")
            || name.starts_with("-terminal-")
            || matches!(name, "fill" | "stroke") =>
        {
            if value == "auto" {
                value
            } else {
                paint(&value)
            }
        }
        _ if name.starts_with("margin-")
            || name.starts_with("padding-")
            || name.starts_with("scroll-margin-")
            || name.starts_with("scroll-padding-")
            || name.ends_with("-width") && name.starts_with("border-")
            || name.ends_with("-radius")
            || matches!(
                name,
                "top"
                    | "right"
                    | "bottom"
                    | "left"
                    | "width"
                    | "height"
                    | "min-width"
                    | "min-height"
                    | "max-width"
                    | "max-height"
                    | "font-size"
                    | "letter-spacing"
                    | "word-spacing"
                    | "text-indent"
                    | "outline-offset"
                    | "outline-width"
                    | "row-gap"
                    | "column-gap"
                    | "border-spacing"
                    | "perspective"
                    | "flex-basis"
                    | "text-underline-offset"
                    | "text-decoration-thickness"
                    | "column-width"
                    | "column-rule-width"
                    | "-webkit-text-stroke-width"
            ) =>
        {
            // Like Chrome, the offsets of a static box are not used values.
            let used = name.starts_with("margin-")
                || name.starts_with("padding-")
                || matches!(name, "width" | "height")
                || matches!(name, "top" | "right" | "bottom" | "left")
                    && style
                        .get(Prop::POSITION)
                        .is_some_and(|position| position != "static");
            let mut parts = components(&value, ' ')
                .into_iter()
                .map(|part| {
                    if !used && part.contains('%') {
                        percents(part)
                    } else if used
                        && size.is_some()
                        && (part.contains('%') || matches!(name, "width" | "height"))
                    {
                        let text = length(part);
                        // Layout boxes have sizes in 1/64 px, like Chrome's, so their used values truncate to it.
                        // Fixed margins, paddings and offsets stay as they are.
                        match text
                            .strip_suffix("px")
                            .and_then(|number| number.parse::<f32>().ok())
                        {
                            Some(number) => px(layout_unit(number)),
                            None => text,
                        }
                    } else {
                        length(part)
                    }
                })
                .collect::<Vec<_>>();
            if name == "border-spacing" && parts.len() == 2 && parts[0] == parts[1] {
                parts.pop();
            }
            parts.join(" ")
        }
        // Like Chrome, a font weight keeps quarters.
        "font-weight" if let Ok(weight) = value.parse::<f64>() => {
            printed((weight * 4.).floor() / 4.)
        }
        // Computed numbers print like Chrome's, like `3` for `03` and `1e+07` for `10000000`.
        _ if grammar(name)
            .is_some_and(|grammar| grammar.contains("integer>") || grammar.contains("number>")) =>
        {
            let integer = grammar(name).is_some_and(|grammar| grammar.contains("integer>"));
            components(&value, ',')
                .into_iter()
                .map(|part| {
                    let part = part.trim();
                    match part.parse::<f64>() {
                        _ if !part.starts_with(|c: char| {
                            c.is_ascii_digit() || matches!(c, '+' | '-' | '.')
                        }) =>
                        {
                            part.to_owned()
                        }
                        Ok(parsed) if integer && parsed.fract() == 0. => {
                            (parsed.clamp(i32::MIN.into(), i32::MAX.into()) as i32).to_string()
                        }
                        Ok(parsed) if !integer => printed(parsed),
                        _ => part.to_owned(),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
        _ => value,
    }
}

// A sum of percentages and lengths, like `calc(10% + 1em)`, as its percentage and its pixels.
pub(crate) fn percent_sum(value: &str, units: &Lengths) -> Option<(f32, f32)> {
    // Such a sum is linear in its base, so it is its value at base 0 and the change of one base of 100.
    // Like Chrome, other functions with percentages in it, like `min(10%, 5px)`, keep their text.
    let bytes = value.as_bytes();
    if !bytes
        .get(..5)
        .is_some_and(|start| start.eq_ignore_ascii_case(b"calc("))
        || value.match_indices('(').skip(1).any(|(at, _)| {
            let mut depth = 0;
            bytes[at - 1].is_ascii_alphabetic()
                && !value[..at].to_ascii_lowercase().ends_with("calc")
                && bytes[at..]
                    .iter()
                    .take_while(|&&byte| {
                        depth += i32::from(byte == b'(') - i32::from(byte == b')');
                        depth > 0
                    })
                    .any(|&byte| byte == b'%')
        })
    {
        return None;
    }
    let (pixels, _) = units.math(value, 0., &[])?;
    let (sum, kind) = units.math(value, 100., &[])?;
    (kind == Kind::Length).then_some(((sum - pixels) as f32, pixels as f32))
}

// Chrome's text of a percentage plus pixels.
pub(crate) fn percent_text((percent, pixels): (f32, f32)) -> String {
    let mut out = String::new();
    if pixels != 0. {
        out.push_str("calc(");
    }
    write_printed(f64::from(percent), &mut out);
    out.push('%');
    if pixels != 0. {
        out.push_str(if pixels < 0. { " - " } else { " + " });
        write_printed(f64::from(pixels.abs()), &mut out);
        out.push_str("px)");
    }
    out
}

// The grid line that a missing line of a shorthand takes from `start`: the same name, or `auto`.
fn grid_default(start: &str) -> &str {
    let number = start.trim_start_matches(['-', '+']);
    if start.contains(' ') || number.starts_with(|c: char| c.is_ascii_digit()) {
        "auto"
    } else {
        start
    }
}

// A track list with its lengths in pixels, like Chrome's computed tracks.
fn tracks(value: &str, units: &Lengths, out: &mut String) {
    for (index, part) in components(value, ' ').into_iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        match part.split_once('(') {
            Some((function, args)) if function != "calc" && part.ends_with(')') => {
                out.push_str(function);
                out.push('(');
                for (index, arg) in components(&args[..args.len() - 1], ',')
                    .into_iter()
                    .enumerate()
                {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    tracks(arg.trim(), units, out);
                }
                out.push(')');
            }
            _ => match units.math(part, f32::NAN, &[]) {
                Some((pixels, Kind::Length)) => {
                    out.push_str(&number(pixels));
                    out.push_str("px");
                }
                Some((0., Kind::Number)) => out.push_str("0px"),
                _ => out.push_str(part),
            },
        }
    }
}

// The physical property that a logical property name maps to, as the cascade maps it.
pub(crate) fn physical_name<'a>(style: &Style, name: &'a str) -> Cow<'a, str> {
    if let Some((_, ltr, rtl)) = LOGICAL.iter().find(|(logical, ..)| logical.name() == name) {
        return Cow::Borrowed(
            if style.get(Prop::DIRECTION) == Some("rtl") {
                rtl
            } else {
                ltr
            }
            .name(),
        );
    }
    let mut out = Vec::new();
    if (name.contains("block") || name.contains("inline"))
        && expand(name, "0", &mut out)
        && let [(physical, _)] = &out[..]
        && physical != name
    {
        return Cow::Owned(physical_name(style, physical).into_owned());
    }
    Cow::Borrowed(name)
}
