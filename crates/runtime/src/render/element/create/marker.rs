use super::super::Build;
use super::super::query::text_indent;
use super::cx::{Cx, Frame};
use crate::css;
use crate::css::properties::Prop;
use crate::style::{self};
use renderer::{Color, px, relative};
use std::rc::Rc;

/// Builds the marker of a list item.
#[inline(never)]
pub(super) fn list_marker(cx: &Cx<'_>, fr: &Frame, mut el: renderer::Node) -> renderer::Node {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        parsed,
        display,
        containing,
        ..
    } = *cx;
    let Build {
        doc,
        styles,
        text,
        pixel_ratio,
        siblings,
        view,
        ..
    } = *reader;
    let Frame {
        padding, borders, ..
    } = *fr;
    let marker_style = styles
        .text_pseudos
        .get(&id)
        .and_then(|pseudos| pseudos[0].as_ref());
    let content = marker_style
        .and_then(|marker| marker.get(Prop::CONTENT))
        .filter(|value| *value != "normal");
    if display == "list-item"
        && content.is_none_or(|value| value != "none")
        && let Some(kind) = style
            .get(Prop::LIST_STYLE_TYPE)
            .filter(|kind| *kind != "none" || content.is_some())
    {
        let list = node.parent.map(|parent| &doc.nodes[parent]);
        let items = node.parent.map(|parent| {
            siblings
                .borrow_mut()
                .entry(parent)
                .or_insert_with(|| {
                    let items = doc.nodes[parent].children.iter().copied().filter(|&child| {
                        styles.values[child]
                            .get(Prop::DISPLAY)
                            .is_some_and(|value| value == "list-item")
                    });
                    Rc::new(
                        items
                            .enumerate()
                            .map(|(index, item)| (item, index))
                            .collect(),
                    )
                })
                .clone()
        });
        let position = items
            .as_ref()
            .and_then(|items| items.get(&id).copied())
            .unwrap_or(0) as i64;
        let count = items.as_ref().map_or(1, |items| items.len()) as i64;
        let start = list
            .and_then(|list| list.attrs.get("start"))
            .and_then(|value| value.parse::<i64>().ok());
        let number = node
            .attrs
            .get("value")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(
                if list.is_some_and(|list| list.attrs.contains_key("reversed")) {
                    start.unwrap_or(count) - position
                } else {
                    start.unwrap_or(1) + position
                },
            );
        let number = styles.list_items.get(&id).copied().unwrap_or(number);
        let marker = match kind {
            "disc" | "circle" | "square" => css::value::counter::counter_text(kind, number),
            "disclosure-closed" => "▸".to_owned(),
            "disclosure-open" => "▾".to_owned(),
            other if other.starts_with(['"', '\'']) => other.trim_matches(['"', '\'']).to_owned(),
            "cjk-decimal"
            | "cjk-ideographic"
            | "trad-chinese-informal"
            | "simp-chinese-informal"
            | "hiragana"
            | "katakana" => format!("{}、", css::value::counter::counter_text(kind, number)),
            _ => format!("{}.", css::value::counter::counter_text(kind, number)),
        };
        let marker = content.map_or(marker, |content| {
            let mut text = String::new();
            for part in css::value::tokens::components(content, ' ') {
                if part.starts_with("counter(") || part.starts_with("counters(") {
                    text.push_str(&number.to_string());
                    continue;
                }
                let mut input = cssparser::Parser::new(part);
                if let Ok(string) = input.expect_string() {
                    text.push_str(string);
                }
            }
            text
        });
        let inside = style
            .get(Prop::LIST_STYLE_POSITION)
            .is_some_and(|value| value == "inside");
        let (indent, percent) = text_indent(style, &units);
        let indent = indent + percent * containing[0];
        let face = text.face(&style::font(marker_style.unwrap_or(style)));
        let size = marker_style
            .and_then(|marker| marker.get(Prop::FONT_SIZE))
            .and_then(|value| units.resolve(value, units.font.em))
            .unwrap_or(units.font.em);
        let metrics = text
            .line_metrics(face, px(size * pixel_ratio))
            .map(f32::from);
        let line = match marker_style.map(style::line_height) {
            Some(style::LineHeight::Fixed(line)) => line,
            Some(style::LineHeight::Scale(scale)) => scale * size,
            Some(style::LineHeight::Normal) => metrics.iter().sum::<f32>() / pixel_ratio,
            None => units.font.lh,
        };
        let mut top = padding[0];
        if !inside && marker_style.is_some() {
            let own = text.face(&parsed.face);
            let rise = f32::from(text.baseline(face, px(size), px(line), pixel_ratio))
                - f32::from(text.baseline(own, px(units.font.em), px(units.font.lh), pixel_ratio));
            if rise > 0. {
                el = el.pt(px(padding[0] + rise));
            } else {
                top -= rise;
            }
        }
        let mut marker_box = if content.is_none() && kind.starts_with("disclosure-") {
            let [ascent, descent, _] = metrics;
            let side = (ascent * 2. / 3.).floor();
            let leading = ((line * pixel_ratio - ascent - descent) / 2.).floor();
            let space = text
                .advance(face, px(size), ' ')
                .map_or(size / 4., f32::from);
            let look = crate::render::paint::control::Look {
                control: crate::render::paint::control::Control::Disclosure,
                checked: kind == "disclosure-open",
                ink: marker_style
                    .unwrap_or(style)
                    .get(Prop::COLOR)
                    .and_then(css::value::color::color)
                    .map_or(0, |(ink, _)| ink),
                ..Default::default()
            };
            let triangle = renderer::node()
                .absolute()
                .left_0()
                .top(px((leading + ascent - side) / pixel_ratio))
                .size(px(side / pixel_ratio))
                .child(crate::render::paint::control::layer(id, look, view.clone()));
            let above = (leading + ascent) / pixel_ratio;
            let marker = renderer::node()
                .flex_shrink_0()
                .w(px(side / pixel_ratio + space))
                .h(px(above))
                .mb(px(line - above))
                .baseline(renderer::Baseline::Bottom)
                .child(triangle);
            if inside {
                marker
            } else {
                marker.absolute().top(px(top)).right(relative(1.))
            }
        } else if inside && !(content.is_none() && matches!(kind, "disc" | "circle" | "square")) {
            // The marker starts the first line, so the text of that line is indented by its width.
            let marker = if marker.ends_with(' ') {
                marker
            } else {
                marker + " "
            };
            let run = style::text_run(marker_style.unwrap_or(style), &units, marker.len());
            let width = f32::from(text.shape_line(&marker, px(size), &run).line.width);
            el.text_style()
                .set_text_indent(px(indent + width))
                .set_text_indent_percent(0.);
            renderer::node()
                .absolute()
                .top(px(top))
                .left(px(padding[3] + indent))
                .whitespace_nowrap()
                .child(marker)
        } else if content.is_none() && matches!(kind, "disc" | "circle" | "square") {
            let color = marker_style
                .unwrap_or(style)
                .get(Prop::COLOR)
                .and_then(css::value::color::paint)
                .unwrap_or(Color::hex(0x000000ff));
            let [ascent, descent, _] = metrics;
            let offset = (ascent * 2. / 3.).floor();
            let leading = ((line * pixel_ratio - ascent - descent) / 2.).floor();
            let side = ((offset + 1.) / 2.).floor() / pixel_ratio;
            // Like Chrome, an inside symbol takes its size, 1px and 1em before the text.
            if inside {
                el.text_style()
                    .set_text_indent(px(indent + side + 1. + size))
                    .set_text_indent_percent(0.);
            }
            let dot = renderer::node()
                .absolute()
                .left(px(if inside {
                    padding[3] + indent
                } else {
                    padding[3] - (offset + 7.) / pixel_ratio
                }))
                .top(px(top
                    + (leading + (3. * (ascent - offset) / 2.).floor())
                        / pixel_ratio))
                .size(px(side));
            match kind {
                "disc" => dot.rounded_full().bg(color),
                "circle" => dot
                    .rounded_full()
                    .border(px(1. / pixel_ratio))
                    .border_color(color),
                _ => dot.bg(color),
            }
        } else {
            renderer::node()
                .absolute()
                .top(px(top))
                .right(relative(1.))
                // Like Chrome, a counter marker ends with a space, and strings and `、` end as they are.
                .pr(px(
                    if content.is_some()
                        || marker.ends_with(['、', ' '])
                        || kind.starts_with(['"', '\''])
                    {
                        0.
                    } else {
                        text.advance(face, px(size), ' ')
                            .map_or(size / 4., f32::from)
                    },
                ))
                .whitespace_nowrap()
                .child(marker)
        };
        if let Some(marker) = marker_style {
            let face = style::font(marker);
            marker_box = marker_box
                .font_family(face.family)
                .font_weight(face.weight)
                .text_size(px(size))
                .line_height(px(line));
            marker_box
                .text_style()
                .set_font_style(face.style)
                .set_font_synthesis(face.synthesis);
            if let Some(ink) = marker.get(Prop::COLOR).and_then(css::value::color::color) {
                marker_box = marker_box.text_color(css::value::color::rgba(ink));
            }
        }
        // Like Chrome's `::marker`, numbers take tabular widths.
        marker_box
            .text_style()
            .set_font_features(Rc::from([("tnum".to_owned(), 1)]));
        // Like Chrome, an empty item keeps the line of its marker.
        if !inside
            && node.children.iter().all(|&child| {
                doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty()
            })
            && style.get(Prop::HEIGHT).is_none_or(|v| v == "auto")
            && style.get(Prop::MIN_HEIGHT).is_none_or(|v| v == "auto")
        {
            let edges = padding[0] + padding[2] + borders[0] + borders[2];
            el = el.min_h(px(line.max(units.font.lh) + edges));
        }
        el = el.child(marker_box);
    }
    el
}
