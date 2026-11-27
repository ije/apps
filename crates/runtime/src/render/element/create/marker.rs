use super::super::Build;
use super::super::margin::marker_line;
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
        rtl,
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
        .extras(id)
        .text
        .as_ref()
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
            "disc" | "circle" | "square" | "disclosure-closed" | "disclosure-open" => {
                css::value::counter::counter_text(kind, number)
            }
            other if other.starts_with(['"', '\'']) => other.trim_matches(['"', '\'']).to_owned(),
            "cjk-decimal"
            | "cjk-ideographic"
            | "trad-chinese-informal"
            | "simp-chinese-informal"
            | "hiragana"
            | "katakana" => format!("{}、", css::value::counter::counter_text(kind, number)),
            _ => {
                let number = css::value::counter::counter_text(kind, number);
                match rtl {
                    true => format!(".{number}"),
                    false => number + ".",
                }
            }
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
        let line = match marker_style.map(|marker| style::line_height(marker, units.font.zoom)) {
            Some(style::LineHeight::Fixed(line)) => line,
            Some(style::LineHeight::Scale(scale)) => scale * size,
            Some(style::LineHeight::Normal) => metrics.iter().sum::<f32>() / pixel_ratio,
            None => units.font.lh,
        };
        let mut top = padding[0];
        // Like Chrome, an inside marker before a block is in a line of its own.
        let alone = marker_line(reader, id);
        // The marker starts the first line, so the content of that line starts after its width.
        let start = |mut el: renderer::Node, width: f32| {
            if alone {
                return el;
            }
            if cx.all_inline && cx.inline_text().is_none() {
                return el.child(renderer::node().w(px(width)).flex_shrink_0());
            }
            el.text_style()
                .set_text_indent(px(indent + width))
                .set_text_indent_percent(0.);
            el
        };
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
                zoom: units.font.zoom,
                control: crate::render::paint::control::Control::Disclosure,
                checked: kind == "disclosure-open",
                ink: marker_style
                    .unwrap_or(style)
                    .get(Prop::COLOR)
                    .and_then(css::value::color::color)
                    .map_or(0, |(ink, _)| ink),
                ..Default::default()
            };
            let triangle = renderer::node().absolute();
            let triangle = match rtl {
                true => triangle.right(px(0.)),
                false => triangle.left_0(),
            }
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
                let marker = marker.absolute().top(px(top));
                match rtl {
                    true => marker.left(relative(1.)),
                    false => marker.right(relative(1.)),
                }
            }
        } else if inside && !(content.is_none() && matches!(kind, "disc" | "circle" | "square")) {
            let marker = match rtl {
                _ if marker.ends_with(' ') => marker,
                true => format!(" {marker}"),
                false => marker + " ",
            };
            let run = style::text_run(marker_style.unwrap_or(style), &units, marker.len());
            let width = f32::from(text.shape_line(&marker, px(size), &run).line.width);
            el = start(el, width);
            let marker = renderer::node()
                .absolute()
                .top(px(top))
                .whitespace_nowrap()
                .child(marker);
            match rtl {
                true => marker.right(px(padding[1] + indent)),
                false => marker.left(px(padding[3] + indent)),
            }
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
            // Like Chrome, an inside symbol takes its size, a device pixel and 1em before the text.
            if inside {
                el = start(el, side + 1. / pixel_ratio + size);
            }
            let start = if inside {
                indent
            } else {
                -(offset + 7.) / pixel_ratio
            };
            let dot = renderer::node().absolute();
            let dot = match rtl {
                true => dot.right(px(padding[1] + start)),
                false => dot.left(px(padding[3] + start)),
            }
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
            // Like Chrome, a counter marker ends with a space, and strings and `、` end as they are.
            let space = if content.is_some()
                || marker.ends_with(['、', ' '])
                || kind.starts_with(['"', '\''])
            {
                0.
            } else {
                text.advance(face, px(size), ' ')
                    .map_or(size / 4., f32::from)
            };
            let marker = renderer::node()
                .absolute()
                .top(px(top))
                .whitespace_nowrap()
                .child(marker);
            match rtl {
                true => marker.left(relative(1.)).pl(px(space)),
                false => marker.right(relative(1.)).pr(px(space)),
            }
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
        if alone {
            return el.child(
                renderer::node()
                    .w_full()
                    .h(px(line))
                    .flex_shrink_0()
                    .child(marker_box),
            );
        }
        // Like Chrome, an empty item keeps the line of its marker.
        if node.children.iter().all(|&child| {
            doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty()
        }) && style.get(Prop::HEIGHT).is_none_or(|v| v == "auto")
            && style.get(Prop::MIN_HEIGHT).is_none_or(|v| v == "auto")
        {
            let edges = padding[0] + padding[2] + borders[0] + borders[2];
            el = el.min_h(px(line.max(units.font.lh) + edges));
        }
        el = el.child(marker_box);
    }
    el
}
