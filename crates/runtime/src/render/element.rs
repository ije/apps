use super::{
    WebView,
    background::css_background,
    paint::{DomBox, PaintLayers, Stacked, Turned, paint_layers},
    text::{InlineText, TextHighlight, paint_text},
};
use crate::{
    desktop::Desktop,
    dom::Document,
    style::{self, ComputedStyles},
};
use gpui::{
    AnyElement, Bounds, Context, MouseButton, SharedString, div, img, point, prelude::*, px,
    relative, rgba, size,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

pub(super) struct Build<'a> {
    pub(super) doc: &'a Document,
    pub(super) styles: &'a ComputedStyles,
    pub(super) text: &'a gpui::WindowTextSystem,
    pub(super) pixel_ratio: f32,
    pub(super) view: &'a WebView,
    pub(super) layers: PaintLayers,
    pub(super) path: Rc<[usize]>,
    pub(super) flex_parent: bool,
    pub(super) measured: &'a RefCell<super::grid::Measurements>,
    pub(super) inline_text: &'a RefCell<HashMap<usize, Option<Rc<style::InlineText>>>>,
    /// The parent's `perspective` distance and `perspective-origin`.
    pub(super) perspective: Option<(f32, String)>,
}

pub(super) fn element(
    build: &Build<'_>,
    id: usize,
    containing: [f32; 2],
    container: [f32; 2],
    slot: Option<([f32; 4], super::grid::Grid)>,
    cx: &mut Context<WebView>,
) -> AnyElement {
    let Build {
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        ref layers,
        ref path,
        flex_parent,
        measured,
        inline_text,
        perspective: ref parent_perspective,
    } = *build;
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    let display = style.get("display").map_or(
        if node.tag.starts_with('#') && node.tag != "#text" {
            "block"
        } else {
            "inline"
        },
        String::as_str,
    );
    let order = style
        .get("z-index")
        .and_then(|value| value.parse::<i32>().ok());
    let positioned = style.get("position").is_some_and(|value| value != "static");
    let stacking = id == 0
        || style
            .get("isolation")
            .is_some_and(|value| value == "isolate")
        || style
            .get("position")
            .is_some_and(|value| matches!(value.as_str(), "fixed" | "sticky"))
        || positioned && order.is_some()
        || style.get("transform").is_some_and(|value| value != "none")
        || ["filter", "clip-path"]
            .iter()
            .any(|key| style.get(*key).is_some_and(|value| value != "none"))
        || style
            .get("opacity")
            .and_then(|value| value.parse::<f32>().ok())
            .is_some_and(|value| value < 1.);
    let child_build = Build {
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        measured,
        inline_text,
        layers: if stacking {
            Rc::default()
        } else {
            layers.clone()
        },
        path: if stacking {
            Rc::default()
        } else {
            path.iter().copied().chain([id]).collect()
        },
        flex_parent: matches!(display, "flex" | "inline-flex"),
        perspective: style
            .get("perspective")
            .filter(|value| *value != "none")
            .and_then(|value| styles.lengths[id].resolve(value, 0.))
            .map(|distance| {
                (
                    distance,
                    style
                        .get("perspective-origin")
                        .cloned()
                        .unwrap_or_else(|| "50% 50%".into()),
                )
            }),
    };
    let build = &child_build;
    let mut units = styles.lengths[id];
    units.container = Some(container);
    let mut ancestors = vec![id];
    let mut parent = node.parent;
    while let Some(id) = parent {
        ancestors.push(id);
        parent = doc.nodes[id].parent;
    }
    // WARNING: element building and gpui layout recurse per level, so deep pages must not overflow the stack.
    if ancestors.len() > 256 || style.get("display").is_some_and(|s| s == "none") {
        return div().hidden().into_any_element();
    }
    let opacity = style
        .get("opacity")
        .and_then(|value| {
            value
                .strip_suffix('%')
                .map_or(value.parse::<f32>().ok(), |v| {
                    v.parse::<f32>().ok().map(|v| v / 100.)
                })
        })
        .filter(|v| v.is_finite())
        .unwrap_or(1.)
        .clamp(0., 1.);
    // Group opacity and these properties paint the box through an offscreen layer.
    let effect_layer = ([
        "filter",
        "clip-path",
        "mask-image",
        "-webkit-mask-image",
        "backdrop-filter",
        "-webkit-backdrop-filter",
    ]
    .iter()
    .any(|name| style.get(*name).is_some_and(|value| value != "none"))
        || style
            .get("mix-blend-mode")
            .is_some_and(|value| value != "normal")
        || style
            .get("isolation")
            .is_some_and(|value| value == "isolate")
        || opacity < 1. && !node.children.is_empty())
    .then_some(opacity);
    let selection_colors = style::selection_colors(style, styles.selections.get(&id));
    let face = style::font(style);
    let mut el = div()
        .id(id)
        .flex()
        .flex_col()
        .text_size(px(units.font.em))
        .font_family(face.family)
        .font_weight(face.weight)
        .line_height(px(units.font.lh));
    el.style().flex_shrink = Some(if flex_parent { 1. } else { 0. });
    {
        let text = el.text_style();
        text.letter_spacing = Some(style::spacing(style, &units, "letter-spacing"));
        text.word_spacing = Some(style::spacing(style, &units, "word-spacing"));
        text.text_indent = Some(px(style
            .get("text-indent")
            .and_then(|value| units.resolve(value, containing[0]))
            .unwrap_or(0.)));
    }
    if face.style != gpui::FontStyle::Normal {
        el.text_style().font_style = Some(face.style);
    }
    if node.tag == "br" {
        return div().w_full().h(px(0.)).into_any_element();
    }
    if stacking {
        el = el.child(paint_layers(build.layers.clone(), true));
    }
    let flow = !matches!(display, "flex" | "inline-flex" | "grid" | "inline-grid");
    let mut flat = Vec::new();
    let mut pending: Vec<usize> = node.children.iter().rev().copied().collect();
    while let Some(child) = pending.pop() {
        if styles.values[child]
            .get("display")
            .is_some_and(|value| value == "contents")
        {
            pending.extend(doc.nodes[child].children.iter().rev());
        } else if doc.nodes[child].tag != "#comment" {
            flat.push(child);
        }
    }
    let floated = |child: usize| {
        flow && float_side(&styles.values[child]).is_some()
            && styles.values[child]
                .get("position")
                .is_none_or(|value| !matches!(value.as_str(), "absolute" | "fixed"))
    };
    let inline_level = |child: usize| {
        doc.nodes[child].tag.starts_with('#')
            || !floated(child)
                && styles.values[child]
                    .get("display")
                    .is_none_or(|value| value.starts_with("inline"))
            || styles.values[child]
                .get("position")
                .is_some_and(|value| matches!(value.as_str(), "absolute" | "fixed"))
    };
    let all_inline = flow && !flat.is_empty() && flat.iter().all(|&child| inline_level(child));
    let line_justify = match style.get("text-align").map(String::as_str) {
        Some("right" | "end") => Some(gpui::JustifyContent::End),
        Some("center" | "-webkit-center") => Some(gpui::JustifyContent::Center),
        _ => None,
    };
    let line_align = Some(
        if flat.iter().any(|&child| {
            styles.values[child]
                .get("vertical-align")
                .is_some_and(|value| value == "middle")
        }) {
            gpui::AlignItems::Center
        } else {
            gpui::AlignItems::Baseline
        },
    );
    if all_inline || display == "inline" {
        el = el.flex_row().flex_wrap();
        el.style().justify_content = line_justify;
        el.style().align_items = line_align;
    }
    if node.tag == "canvas" {
        el = el
            .w(px(node
                .attrs
                .get("width")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300.)))
            .h(px(node
                .attrs
                .get("height")
                .and_then(|v| v.parse().ok())
                .unwrap_or(150.)));
        if let Some(image) = view.images.get(&id) {
            el = el.child(img(image.clone()).size_full());
        }
        if let Some(image) = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|state| state.canvases.get(&id))
            .map(|canvas| canvas.image.clone())
        {
            el = el.child(img(image).absolute().size_full());
        }
    }
    let mut image_child = None;
    let mut cropped = None;
    if node.tag == "img" {
        let (image, vector) = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .map_or((None, None), |state| {
                (
                    state.images.get(&id).cloned(),
                    state.svg_images.get(&id).cloned(),
                )
            });
        if let Some(natural) = image
            .as_ref()
            .map(|image| {
                let size = image.size(0);
                [u32::from(size.width) as f32, u32::from(size.height) as f32]
            })
            .or(vector.as_ref().map(|(_, natural)| *natural))
        {
            let attr = |name: &str| {
                node.attrs
                    .get(name)
                    .and_then(|value| value.parse::<f32>().ok())
                    .filter(|value| value.is_finite() && *value >= 0.)
            };
            let auto = |name: &str| style.get(name).is_none_or(|value| value == "auto");
            if natural[1] > 0. {
                el.style().aspect_ratio = Some(natural[0] / natural[1]);
            }
            match (auto("width"), auto("height"), attr("width"), attr("height")) {
                (true, _, Some(width), _) => el = el.w(px(width)),
                (true, true, None, None) => el = el.w(px(natural[0])),
                _ => {}
            }
            if auto("height")
                && let Some(height) = attr("height")
            {
                el = el.h(px(height));
            }
            let fit = match style.get("object-fit").map(String::as_str) {
                Some("contain") => gpui::ObjectFit::Contain,
                Some("cover") => gpui::ObjectFit::Cover,
                Some("none") => gpui::ObjectFit::None,
                Some("scale-down") => gpui::ObjectFit::ScaleDown,
                _ => gpui::ObjectFit::Fill,
            };
            if let Some(image) = image {
                if matches!(fit, gpui::ObjectFit::Cover | gpui::ObjectFit::None) {
                    cropped = Some((image, fit));
                } else {
                    image_child = Some(img(image).size_full().object_fit(fit));
                }
            } else if let Some((source, _)) = vector {
                el = el.child(svg_layer(source.to_string(), id, cx.entity()));
            }
        } else if let Some(path) = node.attrs.get("src").and_then(|source| {
            let base = view
                .host
                .borrow()
                .dom
                .get(&view.id)
                .and_then(|state| state.base.clone())?;
            base.join(source)
                .ok()
                .filter(|url| url.scheme() == "file")?
                .to_file_path()
                .ok()
        }) {
            let auto = |name: &str| style.get(name).is_none_or(|value| value == "auto");
            for (name, horizontal) in [("width", true), ("height", false)] {
                if auto(name)
                    && let Some(length) = node
                        .attrs
                        .get(name)
                        .and_then(|value| value.parse::<f32>().ok())
                        .filter(|value| value.is_finite() && *value >= 0.)
                {
                    el = if horizontal {
                        el.w(px(length))
                    } else {
                        el.h(px(length))
                    };
                }
            }
            let sized = [
                !auto("width") || node.attrs.contains_key("width"),
                !auto("height") || node.attrs.contains_key("height"),
            ];
            let image = img(path).object_fit(match style.get("object-fit").map(String::as_str) {
                Some("contain") => gpui::ObjectFit::Contain,
                Some("cover") => gpui::ObjectFit::Cover,
                Some("none") => gpui::ObjectFit::None,
                Some("scale-down") => gpui::ObjectFit::ScaleDown,
                _ => gpui::ObjectFit::Fill,
            });
            el = el.overflow_hidden();
            image_child = Some(match sized {
                [true, true] => image.size_full(),
                [true, false] => image.w_full(),
                [false, true] => image.h_full(),
                _ => image,
            });
        }
    }
    if node.tag == "svg" {
        let width = style
            .get("width")
            .and_then(|v| units.resolve(v, containing[0]))
            .or_else(|| node.attrs.get("width").and_then(|v| v.parse().ok()))
            .unwrap_or(24.)
            .clamp(1., 4096.);
        let height = style
            .get("height")
            .and_then(|v| units.resolve(v, containing[1]))
            .or_else(|| node.attrs.get("height").and_then(|v| v.parse().ok()))
            .unwrap_or(24.)
            .clamp(1., 4096.);
        let cached = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|state| state.svg_sources.get(&id))
            .filter(|(size, color, _)| {
                *size == [width, height]
                    && color == &style.get("color").cloned().unwrap_or_default()
            })
            .map(|(_, _, source)| source.clone());
        let source = if let Some(source) = cached {
            source
        } else {
            let mut source = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\""
            );
            for (key, value) in &node.attrs {
                if !matches!(
                    key.as_str(),
                    "width" | "height" | "xmlns" | "xmlns:xlink" | "style"
                ) && !style.contains_key(key)
                {
                    source.push_str(&format!(" {key}=\"{}\"", crate::dom::escape(value)));
                }
            }
            source.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
            for key in [
                "fill",
                "fill-opacity",
                "fill-rule",
                "stroke",
                "stroke-width",
                "stroke-opacity",
                "stroke-linecap",
                "stroke-linejoin",
                "color",
            ] {
                if let Some(value) = style.get(key) {
                    source.push_str(&format!(" {key}=\"{}\"", crate::dom::escape(value)));
                }
            }
            source.push('>');
            let mut scope = id;
            while let Some(parent) = doc.nodes[scope].parent {
                scope = parent;
            }
            let mut pending = vec![id];
            let mut visited = std::collections::HashSet::new();
            source.push_str("<defs>");
            while let Some(child) = pending.pop() {
                if !visited.insert(child) {
                    continue;
                }
                let child = &doc.nodes[child];
                pending.extend(&child.children);
                if child.tag == "use"
                    && let Some(reference) = child
                        .attrs
                        .get("href")
                        .or_else(|| child.attrs.get("xlink:href"))
                        .and_then(|value| value.strip_prefix('#'))
                    && let Some((target, _)) =
                        doc.nodes.iter().enumerate().find(|(target, node)| {
                            if !node.attrs.get("id").is_some_and(|value| value == reference) {
                                return false;
                            }
                            let mut root = *target;
                            while let Some(parent) = doc.nodes[root].parent {
                                if parent == id {
                                    return false;
                                }
                                root = parent;
                            }
                            root == scope
                        })
                    && !visited.contains(&target)
                {
                    source.push_str(&doc.serialize(target));
                    pending.push(target);
                }
            }
            source.push_str("</defs>");
            for child in &node.children {
                source.push_str(&doc.serialize(*child));
            }
            source.push_str("</svg>");
            source = source.replace(
                "currentColor",
                style.get("color").map(String::as_str).unwrap_or("black"),
            );
            view.host
                .borrow_mut()
                .dom
                .entry(view.id)
                .or_default()
                .svg_sources
                .insert(
                    id,
                    (
                        [width, height],
                        style.get("color").cloned().unwrap_or_default(),
                        source.clone(),
                    ),
                );
            source
        };
        el = el
            .w(px(width))
            .h(px(height))
            .child(svg_layer(source, id, cx.entity()));
    }
    if node.tag == "iframe" {
        el = el.min_w(px(300.)).min_h(px(150.));
    }
    let input =
        (node.tag == "input").then(|| node.attrs.get("type").map_or("text", String::as_str));
    let check = input.is_some_and(|kind| {
        kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio")
    });
    let editable = node.tag == "textarea"
        || input.is_some_and(|kind| {
            ![
                "checkbox", "radio", "button", "submit", "reset", "image", "file", "color",
                "range", "hidden",
            ]
            .iter()
            .any(|other| kind.eq_ignore_ascii_case(other))
        });
    if input.is_some_and(|kind| {
        ["button", "submit", "reset"]
            .iter()
            .any(|other| kind.eq_ignore_ascii_case(other))
    }) {
        el = el.items_center().justify_center().child(SharedString::from(
            node.attrs.get("value").cloned().unwrap_or_else(|| {
                if input == Some("reset") {
                    "Reset".into()
                } else if input == Some("submit") {
                    "Submit".into()
                } else {
                    String::new()
                }
            }),
        ));
    }
    if editable {
        if !style.contains_key("width") {
            el = el.w(px(180.));
        }

        let text = node.attrs.get("value").cloned().unwrap_or_else(|| {
            if node.tag == "textarea" {
                doc.text(id)
            } else {
                String::new()
            }
        });
        let placeholder = node.attrs.get("placeholder").cloned().unwrap_or_default();
        let face = style::font(style);
        let font_size = units.font.em;
        let line_height = units.font.lh;
        let text_color = style::color(&style["color"])
            .map(|(c, a)| rgba((c << 8) | a as u32))
            .unwrap_or(gpui::black().into());
        let caret_color = style
            .get("caret-color")
            .filter(|value| !matches!(value.as_str(), "auto" | "currentColor"))
            .and_then(|value| style::color(value))
            .map(|(color, alpha)| rgba((color << 8) | alpha as u32))
            .unwrap_or(text_color);
        let placeholder_color = rgba(0x888888ff);
        let entity = cx.entity();
        el = el.cursor(gpui::CursorStyle::IBeam).child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    let view = entity.read(cx);
                    let host = view.host.borrow();
                    let state = host.dom.get(&view.id);
                    let selected = state.and_then(|s| s.selection.get(&id).copied());
                    let focused = state.and_then(|s| s.focus) == Some(id);
                    drop(host);
                    let shown = if text.is_empty() {
                        placeholder.clone()
                    } else {
                        text.clone()
                    };
                    let color = if text.is_empty() {
                        placeholder_color
                    } else {
                        text_color
                    };
                    let mut offset = 0;
                    for (row, text) in shown.split('\n').enumerate() {
                        let run = gpui::TextRun {
                            len: text.len(),
                            font: face.clone(),
                            color: color.into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                            letter_spacing: gpui::px(0.),
                            word_spacing: gpui::px(0.),
                            font_size: None,
                        };
                        let line = window.text_system().shape_line(
                            text.to_owned().into(),
                            px(font_size),
                            std::slice::from_ref(&run),
                            None,
                        );
                        let length = text.encode_utf16().count();
                        let top = bounds.top() + px(row as f32 * line_height);
                        let mut highlights = Vec::new();
                        if focused
                            && let Some([start, end]) = selected
                            && start.min(end) <= offset + length
                            && start.max(end) >= offset
                        {
                            let left = crate::dom::utf16_index(
                                text,
                                start.min(end).saturating_sub(offset),
                            );
                            let right = crate::dom::utf16_index(
                                text,
                                start.max(end).saturating_sub(offset),
                            );
                            let x = line.x_for_index(left);
                            let end_x = line.x_for_index(right);
                            if start == end {
                                window.paint_quad(gpui::fill(
                                    Bounds::new(
                                        point(bounds.left() + x, top),
                                        size(px(1.), px(line_height)),
                                    ),
                                    caret_color,
                                ));
                            } else if end_x > x {
                                highlights.push(TextHighlight {
                                    range: left..right,
                                    color: selection_colors.0.map(|color| rgba(color).into()),
                                    background: rgba(selection_colors.1).into(),
                                });
                            }
                        }
                        if highlights.is_empty() {
                            let _ = line.paint(
                                point(bounds.left(), top),
                                px(line_height),
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        } else {
                            paint_text(
                                &line,
                                &[],
                                point(bounds.left(), top),
                                px(line_height),
                                std::slice::from_ref(&run),
                                0,
                                &highlights,
                                &[],
                                text,
                                None,
                                window,
                            );
                        }
                        offset += length + 1;
                    }
                },
            )
            .absolute()
            .size_full(),
        );
    }
    if let Some((ink, alpha)) = style::color(&style["color"]) {
        el = el.text_color(rgba((ink << 8) | alpha as u32));
    }
    if let Some(color) = style
        .get("background-color")
        .and_then(|s| style::color(s))
        .filter(|(_, alpha)| *alpha > 0)
    {
        let color = rgba((color.0 << 8) | color.1 as u32);
        let blended = style
            .get("background-blend-mode")
            .is_some_and(|value| value.split(',').any(|mode| mode.trim() != "normal"))
            && style
                .get("background-image")
                .is_some_and(|value| value != "none");
        if blended {
            // The color is painted inside the blended background group.
        } else if display == "inline"
            && !node.tag.starts_with('#')
            && !matches!(
                node.tag.as_str(),
                "img" | "svg" | "canvas" | "input" | "button" | "select" | "textarea" | "iframe"
            )
            && !style
                .get("position")
                .is_some_and(|value| value == "absolute")
        {
            let text = cx.text_system();
            let font = text.resolve_font(&style::font(style));
            let font_size = px(units.font.em);
            let height = text.ascent(font, font_size) - text.descent(font, font_size);
            let line_height = px(units.font.lh);
            let radius = px(style
                .get("border-radius")
                .and_then(|value| units.resolve(value, containing[0]))
                .unwrap_or(0.));
            el = el.child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let mut top = bounds.top() + (line_height - height) / 2.;
                        while top < bounds.bottom() {
                            window.paint_quad(
                                gpui::fill(
                                    Bounds::new(
                                        point(bounds.left(), top),
                                        size(bounds.size.width, height),
                                    ),
                                    color,
                                )
                                .corner_radii(radius),
                            );
                            top += line_height.max(px(1.));
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        } else {
            el = el.bg(color);
        }
    }
    let padding = units
        .edges(style, "padding", containing[0])
        .map(|value| value.max(0.));
    el = el
        .pt(px(padding[0]))
        .pr(px(padding[1]))
        .pb(px(padding[2]))
        .pl(px(padding[3]));
    let mut margin = units.edges(style, "margin", containing[0]);
    let edge = |id: usize, name: &str| {
        styles.values[id]
            .get(name)
            .and_then(|value| styles.lengths[id].resolve(value, containing[0]))
            .unwrap_or(0.)
    };
    let in_flow = |id: usize| {
        let (node, style) = (&doc.nodes[id], &styles.values[id]);
        node.tag != "#comment"
            && !(node.tag == "#text" && node.text.trim().is_empty())
            && style.get("display").is_none_or(|value| value != "none")
            && style
                .get("position")
                .is_none_or(|value| !matches!(value.as_str(), "absolute" | "fixed"))
    };
    let block = |id: usize| {
        !doc.nodes[id].tag.starts_with('#')
            && styles.values[id]
                .get("display")
                .is_some_and(|value| !value.starts_with("inline") && value != "contents")
    };
    // WARNING: margins only collapse through plain block boxes; flex/grid items and scroll containers keep their own.
    let collapses = |id: usize, side: usize| {
        let style = &styles.values[id];
        let [padding, border, kind] = if side == 0 {
            ["padding-top", "border-top-width", "border-top-style"]
        } else {
            [
                "padding-bottom",
                "border-bottom-width",
                "border-bottom-style",
            ]
        };
        id != 0
            && doc.nodes[id].tag != "html"
            && doc.nodes[id].parent.is_none_or(|parent| {
                !styles.values[parent].get("display").is_some_and(|value| {
                    matches!(
                        value.as_str(),
                        "flex" | "inline-flex" | "grid" | "inline-grid"
                    )
                })
            })
            && style
                .get("display")
                .is_some_and(|value| matches!(value.as_str(), "block" | "list-item"))
            && ["overflow-x", "overflow-y"]
                .iter()
                .all(|key| style.get(*key).is_none_or(|value| value == "visible"))
            && style
                .get("position")
                .is_none_or(|value| !matches!(value.as_str(), "absolute" | "fixed"))
            && edge(id, padding) == 0.
            && (edge(id, border) == 0.
                || style
                    .get(kind)
                    .is_none_or(|value| matches!(value.as_str(), "none" | "hidden")))
            && (side == 0 || style.get("height").is_none_or(|value| value == "auto"))
    };
    if block(id)
        && in_flow(id)
        && slot.is_none()
        && !flex_parent
        && let Some(parent) = node.parent
    {
        let siblings: Vec<usize> = doc.nodes[parent]
            .children
            .iter()
            .copied()
            .filter(|&child| in_flow(child))
            .collect();
        let index = siblings.iter().position(|&child| child == id);
        if let Some(previous) = index
            .and_then(|index| index.checked_sub(1))
            .map(|index| siblings[index])
            .filter(|&previous| block(previous))
        {
            let (a, b) = (margin[0], edge(previous, "margin-bottom"));
            margin[0] = if a >= 0. && b >= 0. {
                a.max(b)
            } else if a < 0. && b < 0. {
                a.min(b)
            } else {
                a + b
            } - b;
        }
        if index == Some(0)
            && margin[0] >= 0.
            && edge(parent, "margin-top") >= 0.
            && collapses(parent, 0)
        {
            margin[0] = 0.;
        }
        if index.is_some_and(|index| index + 1 == siblings.len())
            && margin[2] >= 0.
            && edge(parent, "margin-bottom") >= 0.
            && collapses(parent, 2)
        {
            margin[2] = 0.;
        }
    }
    let children: Vec<usize> = node
        .children
        .iter()
        .copied()
        .filter(|&child| in_flow(child))
        .collect();
    if let Some(&first) = children
        .first()
        .filter(|&&first| block(first) && margin[0] >= 0. && collapses(id, 0))
    {
        margin[0] = margin[0].max(edge(first, "margin-top"));
    }
    if let Some(&last) = children
        .last()
        .filter(|&&last| block(last) && margin[2] >= 0. && collapses(id, 2))
    {
        margin[2] = margin[2].max(edge(last, "margin-bottom"));
    }
    el = el
        .mt(px(margin[0]))
        .mr(px(margin[1]))
        .mb(px(margin[2]))
        .ml(px(margin[3]));
    for side in ["top", "right", "bottom", "left"] {
        if style
            .get(&format!("margin-{side}"))
            .is_some_and(|s| s == "auto")
        {
            el = match side {
                "top" => el.mt_auto(),
                "right" => el.mr_auto(),
                "bottom" => el.mb_auto(),
                _ => el.ml_auto(),
            };
        }
    }
    let ink = style::color(&style["color"]).map_or(rgba(0x000000ff), |(ink, alpha)| {
        rgba(ink << 8 | alpha as u32)
    });
    let sides = [
        ("border-top-width", "border-top-style", "border-top-color"),
        (
            "border-right-width",
            "border-right-style",
            "border-right-color",
        ),
        (
            "border-bottom-width",
            "border-bottom-style",
            "border-bottom-color",
        ),
        (
            "border-left-width",
            "border-left-style",
            "border-left-color",
        ),
    ];
    let borders = sides.map(|(width, kind, _)| {
        if style
            .get(kind)
            .is_none_or(|kind| matches!(kind.as_str(), "none" | "hidden"))
        {
            return 0.;
        }
        style
            .get(width)
            .map_or(Some(3.), |width| units.resolve(width, containing[0]))
            .unwrap_or(0.)
            .max(0.)
    });
    let border_colors = sides.map(|(_, _, name)| {
        style
            .get(name)
            .and_then(|value| style::color(value))
            .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32))
    });
    el = el
        .border_t(px(borders[0]))
        .border_r(px(borders[1]))
        .border_b(px(borders[2]))
        .border_l(px(borders[3]));
    let visible = (0..4)
        .filter(|&side| borders[side] > 0.)
        .collect::<Vec<_>>();
    let border_kinds = sides.map(|(_, kind, _)| style.get(kind).map_or("none", String::as_str));
    let shaded = visible
        .iter()
        .any(|&side| matches!(border_kinds[side], "groove" | "ridge" | "inset" | "outset"));
    if let Some(&first) = visible.first() {
        if !shaded
            && visible
                .iter()
                .all(|&side| border_colors[side] == border_colors[first])
        {
            el = el.border_color(border_colors[first]);
        } else {
            el = el.border_color(gpui::transparent_black());
        }
        if visible
            .iter()
            .all(|&side| border_kinds[side] == border_kinds[first])
        {
            el.style().border_style = Some(border_style(border_kinds[first]));
        } else if visible
            .iter()
            .any(|&side| matches!(border_kinds[side], "dashed" | "dotted"))
        {
            el.style().border_style = Some(gpui::BorderStyle::Dashed);
        }
    }
    let extra = if style
        .get("box-sizing")
        .is_some_and(|value| value == "border-box")
    {
        [0.; 2]
    } else {
        [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ]
    };
    if editable && style.get("height").is_none_or(|value| value == "auto") {
        el = el.min_h(px(if node.tag == "textarea" {
            2. * units.font.lh
        } else {
            units.font.lh
        } + extra[1]));
    }
    let flexbox = matches!(display, "flex" | "inline-flex");
    let position = style.get("position").map_or("static", String::as_str);
    let mut radius = [0.; 4];
    let mut inner_shadows = Vec::new();
    let mut intrinsic_width = None;
    for (name, value) in
        style.range::<str, _>((std::ops::Bound::Included("a"), std::ops::Bound::Unbounded))
    {
        let value = value.as_str();
        let vertical = name.ends_with("height") || matches!(name.as_str(), "top" | "bottom");
        let base = containing[usize::from(vertical)];
        let length = || units.resolve(value, base);
        match name.as_str() {
            "width" | "height" | "min-width" | "max-width" | "min-height" | "max-height" => {
                let axis = usize::from(vertical);
                let length: gpui::Length = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    if extra[axis] > 0. && base > 0. {
                        px(percent.max(0.) / 100. * base + extra[axis]).into()
                    } else {
                        relative(percent.max(0.) / 100.).into()
                    }
                } else if let Some(length) = length() {
                    px(length.max(0.) + extra[axis]).into()
                } else if matches!(name.as_str(), "width" | "height") && value == "auto" {
                    gpui::Length::Auto
                } else if matches!(value, "stretch" | "-webkit-fill-available") {
                    relative(1.).into()
                } else if matches!(value, "fit-content" | "max-content" | "min-content")
                    && axis == 0
                {
                    let [min, max, ..] = super::grid::intrinsic(
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
                        _ => max.min(min.max(containing[0])),
                    };
                    intrinsic_width = Some((size - margin[1] - margin[3]).max(0.));
                    px((size - margin[1] - margin[3]).max(0.)).into()
                } else {
                    continue;
                };
                el = match name.as_str() {
                    "width" => el.w(length),
                    "height" => el.h(length),
                    "min-width" => el.min_w(length),
                    "max-width" => el.max_w(length),
                    "min-height" => el.min_h(length),
                    _ => el.max_h(length),
                };
            }
            "row-gap" | "column-gap" if flexbox => {
                let Some(length) = length() else { continue };
                el = if name == "row-gap" {
                    el.gap_y(px(length.max(0.)))
                } else {
                    el.gap_x(px(length.max(0.)))
                };
            }
            "border-top-left-radius"
            | "border-top-right-radius"
            | "border-bottom-right-radius"
            | "border-bottom-left-radius" => {
                let own = style
                    .get("width")
                    .and_then(|width| units.resolve(width, containing[0]))
                    .unwrap_or(containing[0]);
                let Some(length) = units.resolve(value, own) else {
                    continue;
                };
                let length = length.max(0.);
                let corner = match name.as_str() {
                    "border-top-left-radius" => 0,
                    "border-top-right-radius" => 1,
                    "border-bottom-right-radius" => 2,
                    _ => 3,
                };
                radius[corner] = length;
                el = match corner {
                    0 => el.rounded_tl(px(length)),
                    1 => el.rounded_tr(px(length)),
                    2 => el.rounded_br(px(length)),
                    _ => el.rounded_bl(px(length)),
                };
            }
            "box-shadow" => {
                el.style().box_shadow = Some(units.shadows(value, false));
                inner_shadows = units.shadows(value, true);
            }
            "aspect-ratio" => {
                let value = value.replace('/', " / ");
                let parts = style::components(&value, ' ');
                let numbers: Vec<f32> = parts.iter().filter_map(|part| part.parse().ok()).collect();
                let ratio = match numbers.as_slice() {
                    [a, b, ..] if parts.contains(&"/") => a / b,
                    [a, ..] => *a,
                    _ => continue,
                };
                if ratio.is_finite() && ratio > 0. {
                    el.style().aspect_ratio = Some(ratio);
                }
            }
            "display" if matches!(value, "grid" | "inline-grid") => el = el.grid(),
            "display" if matches!(value, "flex" | "inline-flex") => el = el.flex().flex_row(),
            "flex-direction" if flexbox => {
                el.style().flex_direction = Some(match value {
                    "row-reverse" => gpui::FlexDirection::RowReverse,
                    "column" => gpui::FlexDirection::Column,
                    "column-reverse" => gpui::FlexDirection::ColumnReverse,
                    _ => gpui::FlexDirection::Row,
                })
            }
            "flex-wrap" if flexbox => {
                el.style().flex_wrap = Some(match value {
                    "wrap" => gpui::FlexWrap::Wrap,
                    "wrap-reverse" => gpui::FlexWrap::WrapReverse,
                    _ => gpui::FlexWrap::NoWrap,
                })
            }
            "flex-grow" | "flex-shrink" => {
                if let Some(factor) = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.)
                {
                    if name == "flex-grow" {
                        el.style().flex_grow = Some(factor);
                    } else {
                        el.style().flex_shrink = Some(factor);
                    }
                }
            }
            "flex-basis" => {
                el.style().flex_basis = Some(
                    if let Some(percent) = value
                        .strip_suffix('%')
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                    {
                        relative(percent.max(0.) / 100.).into()
                    } else if let Some(length) = length() {
                        px(length.max(0.) + extra[0]).into()
                    } else {
                        gpui::Length::Auto
                    },
                )
            }
            "align-items" | "align-self" if flex_parent || flexbox => {
                let align = match value {
                    "center" => gpui::AlignItems::Center,
                    "flex-start" | "start" | "self-start" => gpui::AlignItems::FlexStart,
                    "flex-end" | "end" | "self-end" => gpui::AlignItems::FlexEnd,
                    "baseline" | "first baseline" | "last baseline" => gpui::AlignItems::Baseline,
                    "stretch" | "normal" => gpui::AlignItems::Stretch,
                    _ => continue,
                };
                if name == "align-self" {
                    el.style().align_self = Some(align);
                } else if flexbox {
                    el.style().align_items = Some(align);
                }
            }
            "align-content" | "justify-content" if flexbox => {
                let align = match value {
                    "center" => gpui::AlignContent::Center,
                    "flex-end" | "end" | "right" => gpui::AlignContent::FlexEnd,
                    "space-between" => gpui::AlignContent::SpaceBetween,
                    "space-around" => gpui::AlignContent::SpaceAround,
                    "space-evenly" => gpui::AlignContent::SpaceEvenly,
                    "stretch" => gpui::AlignContent::Stretch,
                    _ => gpui::AlignContent::FlexStart,
                };
                if name == "justify-content" {
                    el.style().justify_content = Some(align);
                } else {
                    el.style().align_content = Some(align);
                }
            }
            "text-align" => {
                el = match value {
                    "center" => el.text_center(),
                    "right" | "end" => el.text_right(),
                    _ => el.text_left(),
                }
            }
            "position" if matches!(value, "absolute" | "fixed") => el = el.absolute(),
            "left" | "right" | "top" | "bottom" if matches!(position, "relative" | "absolute") => {
                let length: gpui::Length = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    relative(percent / 100.).into()
                } else if let Some(length) = length() {
                    px(length).into()
                } else {
                    continue;
                };
                el = match name.as_str() {
                    "left" => el.left(length),
                    "right" => el.right(length),
                    "top" => el.top(length),
                    _ => el.bottom(length),
                };
            }
            "visibility" => {
                el.style().visibility = Some(if value == "hidden" || value == "collapse" {
                    gpui::Visibility::Hidden
                } else {
                    gpui::Visibility::Visible
                })
            }
            "opacity" if effect_layer.is_none() => el = el.opacity(opacity),
            "white-space" if matches!(value, "nowrap" | "pre") => {
                el = el.whitespace_nowrap().flex_nowrap()
            }
            "overflow-x" | "overflow-y" => {
                let overflow = match value {
                    "auto" | "scroll" => gpui::Overflow::Scroll,
                    "hidden" => gpui::Overflow::Hidden,
                    "clip" => gpui::Overflow::Clip,
                    _ => gpui::Overflow::Visible,
                };
                if name == "overflow-x" {
                    el.style().overflow.x = Some(overflow);
                } else {
                    el.style().overflow.y = Some(overflow);
                }
            }
            "cursor" => {
                if let Some(cursor) = style::cursor(value) {
                    el = el.cursor(cursor);
                }
            }
            _ => {}
        }
    }
    if !flex_parent
        && slot.is_none()
        && !matches!(position, "absolute" | "fixed")
        && display.starts_with("inline")
        && let Some(parent) = node.parent
    {
        let align = style
            .get("vertical-align")
            .map_or("baseline", String::as_str);
        el.style().align_self = match align {
            "middle" => Some(gpui::AlignItems::Center),
            "top" | "text-top" => Some(gpui::AlignItems::FlexStart),
            "bottom" => Some(gpui::AlignItems::FlexEnd),
            _ => None,
        };
        let below = |style: &style::Style, font: style::FontUnits| {
            let face = text.resolve_font(&style::font(style));
            let ascent = f32::from(text.ascent(face, px(font.em)));
            let descent = -f32::from(text.descent(face, px(font.em)));
            (font.lh - ascent - descent) / 2. + descent
        };
        let line = styles.lengths[parent].font;
        let replaced = matches!(
            node.tag.as_str(),
            "img"
                | "svg"
                | "canvas"
                | "video"
                | "audio"
                | "iframe"
                | "embed"
                | "object"
                | "progress"
                | "meter"
        ) || check
            || display != "inline"
                && !node.tag.starts_with('#')
                && node.children.iter().all(|&child| {
                    doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim().is_empty()
                });
        let field = editable && node.tag == "input";
        // WARNING: line boxes align text bottoms, so boxes without text are lifted by the parent's descent and half-leading.
        let lift =
            if (replaced || field) && el.style().align_self.is_none() && align != "text-bottom" {
                below(&styles.values[parent], line)
                    - if field {
                        padding[2] + borders[2] + below(style, units.font)
                    } else {
                        0.
                    }
            } else {
                0.
            };
        let raise = match align {
            _ if style.contains_key("top") || style.contains_key("bottom") => 0.,
            "super" => line.em / 3. + lift,
            "sub" => -line.em / 5. + lift,
            "baseline" => lift,
            value => units.resolve(value, units.font.lh).unwrap_or(0.) + lift,
        };
        // WARNING: the margin reserves the shift so the line box grows instead of overlapping its neighbors.
        if raise > 0. {
            el = el.mt(px(margin[0] + raise)).top(px(-raise));
        } else if raise < 0. {
            el = el.mb(px(margin[2] - raise)).top(px(-raise));
        }
    }
    if style
        .get("background-image")
        .is_some_and(|value| value != "none")
    {
        let base = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|state| state.base.clone());
        el = el.children(css_background(
            style,
            units,
            id,
            0,
            radius,
            cx.entity(),
            base.as_ref(),
            &view.host.borrow().assets,
        ));
    }
    if !inner_shadows.is_empty() {
        el = el.child(inset_shadows(inner_shadows, radius, borders));
    }
    if image_child.is_some() || cropped.is_some() {
        let inner = |corner: usize, a: usize, b: usize| {
            (radius[corner] - (borders[a] + padding[a]).max(borders[b] + padding[b])).max(0.)
        };
        let corners = [
            inner(0, 0, 3),
            inner(1, 0, 1),
            inner(2, 2, 1),
            inner(3, 2, 3),
        ];
        if let Some(image) = image_child {
            el = el.child(
                image
                    .rounded_tl(px(corners[0]))
                    .rounded_tr(px(corners[1]))
                    .rounded_br(px(corners[2]))
                    .rounded_bl(px(corners[3])),
            );
        } else if let Some((image, fit)) = cropped {
            let view = cx.entity();
            // WARNING: gpui rounds the fitted image bounds, so overflowing fits paint a cached crop of the visible part.
            el = el.child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let natural = image.size(0);
                        let [width, height] = [u32::from(natural.width), u32::from(natural.height)];
                        let fitted = fit.get_bounds(bounds, natural);
                        let visible = fitted.intersect(&bounds);
                        if width == 0
                            || height == 0
                            || visible.size.width <= px(0.)
                            || visible.size.height <= px(0.)
                        {
                            return;
                        }
                        let scale = [
                            width as f32 / f32::from(fitted.size.width),
                            height as f32 / f32::from(fitted.size.height),
                        ];
                        let x = ((f32::from(visible.left() - fitted.left()) * scale[0]).round()
                            as u32)
                            .min(width - 1);
                        let y = ((f32::from(visible.top() - fitted.top()) * scale[1]).round()
                            as u32)
                            .min(height - 1);
                        let crop = [
                            x,
                            y,
                            ((f32::from(visible.size.width) * scale[0]).round() as u32)
                                .clamp(1, width - x),
                            ((f32::from(visible.size.height) * scale[1]).round() as u32)
                                .clamp(1, height - y),
                        ];
                        let shown = if crop == [0, 0, width, height] {
                            Some(image.clone())
                        } else {
                            let view = view.read(cx);
                            let mut host = view.host.borrow_mut();
                            let state = host.dom.entry(view.id).or_default();
                            let key = {
                                use std::hash::{Hash, Hasher};
                                let mut hasher = std::hash::DefaultHasher::new();
                                (image.id, crop).hash(&mut hasher);
                                hasher.finish()
                            };
                            match state.image_crops.get(&id) {
                                Some((cached, shown)) if *cached == key => Some(shown.clone()),
                                _ => image.as_bytes(0).map(|bytes| {
                                    let pixels = image::RgbaImage::from_fn(
                                        crop[2],
                                        crop[3],
                                        |column, row| {
                                            let at = ((crop[1] + row) * width + crop[0] + column)
                                                as usize
                                                * 4;
                                            image::Rgba([
                                                bytes[at],
                                                bytes[at + 1],
                                                bytes[at + 2],
                                                bytes[at + 3],
                                            ])
                                        },
                                    );
                                    let shown =
                                        Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
                                            pixels,
                                        )]));
                                    if let Some((_, old)) =
                                        state.image_crops.insert(id, (key, shown.clone()))
                                    {
                                        let _ = window.drop_image(old);
                                    }
                                    shown
                                }),
                            }
                        };
                        if let Some(shown) = shown {
                            let corners = gpui::Corners {
                                top_left: px(corners[0]),
                                top_right: px(corners[1]),
                                bottom_right: px(corners[2]),
                                bottom_left: px(corners[3]),
                            }
                            .clamp_radii_for_quad_size(visible.size);
                            let _ = window.paint_image(visible, visible, corners, shown, 0, false);
                        }
                    },
                )
                .size_full(),
            );
        }
    }
    if let Some(lines) = style
        .get("-webkit-line-clamp")
        .or_else(|| style.get("line-clamp"))
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| {
            (style
                .get("text-overflow")
                .is_some_and(|value| value == "ellipsis")
                && style
                    .get("white-space")
                    .is_some_and(|value| matches!(value.as_str(), "nowrap" | "pre"))
                && el
                    .style()
                    .overflow
                    .x
                    .is_some_and(|overflow| overflow != gpui::Overflow::Visible))
            .then_some(1)
        })
    {
        let text = el.text_style();
        text.white_space = Some(gpui::WhiteSpace::Normal);
        text.line_clamp = Some(lines.max(1));
        text.text_overflow = Some(gpui::TextOverflow::Truncate("…".into()));
    }
    if position == "fixed" {
        let viewport = units.viewport;
        let inset = ["left", "right", "top", "bottom"].map(|name| {
            style.get(name).and_then(|v| {
                units.resolve(v, viewport[usize::from(matches!(name, "top" | "bottom"))])
            })
        });
        if style.get("width").is_none_or(|v| v == "auto")
            && let [Some(left), Some(right), ..] = inset
        {
            el = el.w(px((viewport[0] - left - right).max(0.)));
        }
        if style.get("height").is_none_or(|v| v == "auto")
            && let [_, _, Some(top), Some(bottom)] = inset
        {
            el = el.h(px((viewport[1] - top - bottom).max(0.)));
        }
    }
    if let Some(width) = units
        .resolve(
            style.get("outline-width").map_or("3px", String::as_str),
            containing[0],
        )
        .filter(|width| {
            *width > 0.
                && style
                    .get("outline-style")
                    .is_some_and(|kind| !matches!(kind.as_str(), "none" | "hidden"))
        })
    {
        let grow = width
            + style
                .get("outline-offset")
                .and_then(|v| units.resolve(v, containing[0]))
                .unwrap_or(0.);
        let color = style
            .get("outline-color")
            .and_then(|v| style::color(v))
            .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32));
        let mut outline = div()
            .absolute()
            .top(px(-grow - borders[0]))
            .right(px(-grow - borders[1]))
            .bottom(px(-grow - borders[2]))
            .left(px(-grow - borders[3]))
            .border(px(width))
            .border_color(color)
            .rounded_tl(px(radius[0] + grow))
            .rounded_tr(px(radius[1] + grow))
            .rounded_br(px(radius[2] + grow))
            .rounded_bl(px(radius[3] + grow));
        outline.style().border_style = Some(border_style(
            style.get("outline-style").map_or("solid", String::as_str),
        ));
        el = el.child(outline);
    }
    if shaded
        || visible
            .iter()
            .any(|&side| border_colors[side] != border_colors[visible[0]])
    {
        // Each side is painted alone; 3D styles shade it, and groove and ridge split it in two halves.
        let mut bands = vec![([0.; 4], borders, [gpui::transparent_black(); 4])];
        let split = (0..4).any(|side| matches!(border_kinds[side], "groove" | "ridge"));
        if split {
            bands.push(([0.; 4], [0.; 4], [gpui::transparent_black(); 4]));
        }
        for side in 0..4 {
            let color = gpui::Hsla::from(border_colors[side]);
            let raised = side == 0 || side == 3;
            let (outer, inner) = match border_kinds[side] {
                "inset" => (shade_border(color, raised), None),
                "outset" => (shade_border(color, !raised), None),
                "groove" => (
                    shade_border(color, raised),
                    Some(shade_border(color, !raised)),
                ),
                "ridge" => (
                    shade_border(color, !raised),
                    Some(shade_border(color, raised)),
                ),
                _ => (color, None),
            };
            bands[0].2[side] = outer;
            if let Some(inner) = inner {
                let half = borders[side] / 2.;
                bands[0].1[side] = half;
                bands[1].0[side] = half;
                bands[1].1[side] = half;
                bands[1].2[side] = inner;
            }
        }
        if split {
            bands[1].0 = std::array::from_fn(|side| {
                if bands[1].1[side] > 0. {
                    bands[1].0[side]
                } else {
                    borders[side]
                }
            });
        }
        let kinds = border_kinds.map(border_style);
        el = el.child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let outer = Bounds::new(
                        point(
                            bounds.left() - px(borders[3]),
                            bounds.top() - px(borders[0]),
                        ),
                        size(
                            bounds.size.width + px(borders[1] + borders[3]),
                            bounds.size.height + px(borders[0] + borders[2]),
                        ),
                    );
                    for (inset, widths, colors) in &bands {
                        let bounds = Bounds::from_corners(
                            outer.origin + point(px(inset[3]), px(inset[0])),
                            outer.bottom_right() - point(px(inset[1]), px(inset[2])),
                        );
                        let shrink = |corner: usize, a: usize, b: usize| {
                            px((radius[corner] - inset[a].max(inset[b])).max(0.))
                        };
                        let corners = gpui::Corners {
                            top_left: shrink(0, 0, 3),
                            top_right: shrink(1, 0, 1),
                            bottom_right: shrink(2, 2, 1),
                            bottom_left: shrink(3, 2, 3),
                        };
                        for side in 0..4 {
                            if widths[side] <= 0. {
                                continue;
                            }
                            let mut edges = [px(0.); 4];
                            edges[side] = px(widths[side]);
                            window.paint_quad(gpui::quad(
                                bounds,
                                corners,
                                gpui::transparent_black(),
                                gpui::Edges {
                                    top: edges[0],
                                    right: edges[1],
                                    bottom: edges[2],
                                    left: edges[3],
                                },
                                colors[side],
                                kinds[side],
                            ));
                        }
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
    }
    if check && node.attrs.contains_key("checked") {
        let accent = style
            .get("accent-color")
            .and_then(|value| style::color(value))
            .map_or(rgba(0x0a84ffff), |(ink, alpha)| {
                rgba(ink << 8 | alpha as u32)
            });
        let mark = rgba(0xffffffff);
        el = el
            .bg(accent)
            .border_color(accent)
            .items_center()
            .justify_center()
            .child(
                if input.is_some_and(|kind| kind.eq_ignore_ascii_case("radio")) {
                    div().size(px(units.font.em * 0.35)).rounded_full().bg(mark)
                } else {
                    div()
                        .text_color(mark)
                        .text_size(px(units.font.em * 0.75))
                        .line_height(px(units.font.em * 0.75))
                        .child("✓")
                },
            );
    }
    if el.style().overflow.x == Some(gpui::Overflow::Scroll)
        || el.style().overflow.y == Some(gpui::Overflow::Scroll)
    {
        let scroll = view
            .host
            .borrow_mut()
            .dom
            .entry(view.id)
            .or_default()
            .scroll
            .entry(id)
            .or_default()
            .clone();
        el = el.track_scroll(&scroll);
        el.style().restrict_scroll_to_axis = Some(true);
        el = el.child(
            div()
                .absolute()
                .left(px(padding[3] - borders[1] - borders[3]))
                .top(px(padding[0] - borders[0] - borders[2]))
                .size(px(0.)),
        );
    }
    let marker_style = styles
        .text_pseudos
        .get(&id)
        .and_then(|pseudos| pseudos[0].as_ref());
    let content = marker_style
        .and_then(|marker| marker.get("content"))
        .filter(|value| *value != "normal");
    if display == "list-item"
        && content.is_none_or(|value| value != "none")
        && let Some(kind) = style
            .get("list-style-type")
            .filter(|kind| kind.as_str() != "none" || content.is_some())
    {
        let list = node.parent.map(|parent| &doc.nodes[parent]);
        let items: Vec<usize> = list.map_or(vec![id], |list| {
            list.children
                .iter()
                .copied()
                .filter(|&child| {
                    styles.values[child]
                        .get("display")
                        .is_some_and(|value| value == "list-item")
                })
                .collect()
        });
        let position = items.iter().position(|&item| item == id).unwrap_or(0) as i64;
        let start = list
            .and_then(|list| list.attrs.get("start"))
            .and_then(|value| value.parse::<i64>().ok());
        let number = node
            .attrs
            .get("value")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(
                if list.is_some_and(|list| list.attrs.contains_key("reversed")) {
                    start.unwrap_or(items.len() as i64) - position
                } else {
                    start.unwrap_or(1) + position
                },
            );
        let marker = match kind.as_str() {
            "disc" => "•".to_owned(),
            "circle" => "◦".to_owned(),
            "square" => "▪".to_owned(),
            "disclosure-closed" => "▸".to_owned(),
            "disclosure-open" => "▾".to_owned(),
            "decimal" => format!("{number}."),
            "decimal-leading-zero" => format!("{number:02}."),
            "lower-alpha" | "lower-latin" | "upper-alpha" | "upper-latin" if number > 0 => {
                let mut letters = Vec::new();
                let mut rest = number;
                while rest > 0 {
                    rest -= 1;
                    letters.push((b'a' + (rest % 26) as u8) as char);
                    rest /= 26;
                }
                let letters: String = letters.into_iter().rev().collect();
                format!(
                    "{}.",
                    if kind.starts_with("upper") {
                        letters.to_ascii_uppercase()
                    } else {
                        letters
                    }
                )
            }
            "lower-roman" | "upper-roman" if (1..4000).contains(&number) => {
                let mut rest = number;
                let mut roman = String::new();
                for (value, digits) in [
                    (1000, "m"),
                    (900, "cm"),
                    (500, "d"),
                    (400, "cd"),
                    (100, "c"),
                    (90, "xc"),
                    (50, "l"),
                    (40, "xl"),
                    (10, "x"),
                    (9, "ix"),
                    (5, "v"),
                    (4, "iv"),
                    (1, "i"),
                ] {
                    while rest >= value {
                        roman.push_str(digits);
                        rest -= value;
                    }
                }
                format!(
                    "{}.",
                    if kind == "upper-roman" {
                        roman.to_ascii_uppercase()
                    } else {
                        roman
                    }
                )
            }
            other if other.starts_with(['"', '\'']) => other.trim_matches(['"', '\'']).to_owned(),
            _ => format!("{number}."),
        };
        let marker = content.map_or(marker, |content| {
            let mut text = String::new();
            for part in style::components(content, ' ') {
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
        let mut marker_box = if style
            .get("list-style-position")
            .is_some_and(|value| value == "inside")
        {
            div()
                .flex_shrink_0()
                .mr(px(units.font.em * 0.3))
                .child(marker)
        } else {
            div()
                .absolute()
                .top(px(padding[0]))
                .right(relative(1.))
                .pr(px(units.font.em * 0.5))
                .whitespace_nowrap()
                .child(marker)
        };
        if let Some(marker) = marker_style {
            let face = style::font(marker);
            marker_box = marker_box.font_family(face.family).font_weight(face.weight);
            marker_box.text_style().font_style = Some(face.style);
            if let Some((ink, alpha)) = marker.get("color").and_then(|value| style::color(value)) {
                marker_box = marker_box.text_color(rgba(ink << 8 | alpha as u32));
            }
            if let Some(size) = marker
                .get("font-size")
                .and_then(|value| units.resolve(value, units.font.em))
            {
                marker_box = marker_box.text_size(px(size));
            }
        }
        el = el.child(marker_box);
    }
    let mut after = None;
    if let Some(pseudos) = styles.pseudos.get(&id) {
        for (index, pseudo) in pseudos.iter().enumerate() {
            let Some(content) = pseudo
                .get("content")
                .filter(|v| !matches!(v.as_str(), "none" | "normal"))
            else {
                continue;
            };
            if pseudo.get("display").is_some_and(|value| value == "none") {
                continue;
            }
            let mut text = String::new();
            for part in style::components(content, ' ') {
                if let Some(name) = part.strip_prefix("attr(").and_then(|v| v.strip_suffix(')')) {
                    text.push_str(node.attrs.get(name.trim()).map_or("", String::as_str));
                } else if part == "open-quote" {
                    text.push('\u{201c}');
                } else if part == "close-quote" {
                    text.push('\u{201d}');
                } else if let Some(string) = part
                    .strip_prefix(['"', '\''])
                    .and_then(|v| v.strip_suffix(['"', '\'']))
                {
                    let mut chars = string.chars().peekable();
                    while let Some(c) = chars.next() {
                        if c != '\\' {
                            text.push(c);
                            continue;
                        }
                        let mut code = String::new();
                        while code.len() < 6
                            && let Some(digit) = chars.next_if(char::is_ascii_hexdigit)
                        {
                            code.push(digit);
                        }
                        if code.is_empty() {
                            text.extend(chars.next());
                            continue;
                        }
                        chars.next_if_eq(&' ');
                        text.push(
                            u32::from_str_radix(&code, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .unwrap_or('\u{fffd}'),
                        );
                    }
                }
            }
            let mut pseudo_units = units;
            if let Some(size) = pseudo
                .get("font-size")
                .and_then(|v| units.resolve(v, units.font.em))
            {
                pseudo_units.font = style::FontUnits::new(size);
            }
            let face = style::font(pseudo);
            let mut decoration = div()
                .flex()
                .flex_row()
                .flex_shrink_0()
                .text_size(px(pseudo_units.font.em))
                .font_family(face.family)
                .font_weight(face.weight);
            if let Some((ink, alpha)) = pseudo.get("color").and_then(|v| style::color(v)) {
                decoration = decoration.text_color(rgba(ink << 8 | alpha as u32));
            }
            let absolute = pseudo
                .get("position")
                .is_some_and(|v| matches!(v.as_str(), "absolute" | "fixed"));
            if absolute {
                decoration = decoration.absolute();
            } else if pseudo
                .get("display")
                .is_some_and(|value| !value.starts_with("inline"))
                && (all_inline || display == "inline")
            {
                decoration = decoration.w_full();
            }
            let pseudo_padding = pseudo_units
                .edges(pseudo, "padding", containing[0])
                .map(|v| v.max(0.));
            let pseudo_margin = pseudo_units.edges(pseudo, "margin", containing[0]);
            let pseudo_borders = sides.map(|(width, kind, _)| {
                if pseudo
                    .get(kind)
                    .is_none_or(|kind| matches!(kind.as_str(), "none" | "hidden"))
                {
                    return 0.;
                }
                pseudo
                    .get(width)
                    .map_or(Some(3.), |width| pseudo_units.resolve(width, containing[0]))
                    .unwrap_or(0.)
                    .max(0.)
            });
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
            if let Some(side) = (0..4).find(|&side| pseudo_borders[side] > 0.) {
                decoration = decoration.border_color(
                    pseudo
                        .get(sides[side].2)
                        .and_then(|v| style::color(v))
                        .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32)),
                );
            }
            let mut pseudo_radius = [0.; 4];
            let mut pseudo_shadows = Vec::new();
            for (name, value) in pseudo {
                let vertical =
                    name.ends_with("height") || matches!(name.as_str(), "top" | "bottom");
                let base = containing[usize::from(vertical)];
                let length = || -> Option<gpui::Length> {
                    match value
                        .strip_suffix('%')
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                    {
                        Some(percent) => Some(relative(percent / 100.).into()),
                        None => pseudo_units
                            .resolve(value, base)
                            .map(|value| px(value).into()),
                    }
                };
                match name.as_str() {
                    "width" | "height" | "min-width" | "max-width" | "min-height"
                    | "max-height" | "left" | "top" | "right" | "bottom" => {
                        let Some(length) = length() else { continue };
                        decoration = match name.as_str() {
                            "width" => decoration.w(length),
                            "height" => decoration.h(length),
                            "min-width" => decoration.min_w(length),
                            "max-width" => decoration.max_w(length),
                            "min-height" => decoration.min_h(length),
                            "max-height" => decoration.max_h(length),
                            "left" if absolute => decoration.left(length),
                            "top" if absolute => decoration.top(length),
                            "right" if absolute => decoration.right(length),
                            "bottom" if absolute => decoration.bottom(length),
                            _ => decoration,
                        };
                    }
                    "border-top-left-radius"
                    | "border-top-right-radius"
                    | "border-bottom-right-radius"
                    | "border-bottom-left-radius" => {
                        let Some(length) = pseudo_units
                            .resolve(value, containing[0])
                            .map(|v| px(v.max(0.)))
                        else {
                            continue;
                        };
                        let corner = match name.as_str() {
                            "border-top-left-radius" => 0,
                            "border-top-right-radius" => 1,
                            "border-bottom-right-radius" => 2,
                            _ => 3,
                        };
                        pseudo_radius[corner] = f32::from(length);
                        decoration = match name.as_str() {
                            "border-top-left-radius" => decoration.rounded_tl(length),
                            "border-top-right-radius" => decoration.rounded_tr(length),
                            "border-bottom-right-radius" => decoration.rounded_br(length),
                            _ => decoration.rounded_bl(length),
                        };
                    }
                    "background-color" => {
                        if let Some((color, alpha)) = style::color(value) {
                            decoration = decoration.bg(rgba(color << 8 | alpha as u32));
                        }
                    }
                    "opacity" => {
                        if let Ok(opacity) = value.parse::<f32>() {
                            decoration = decoration.opacity(opacity.clamp(0., 1.));
                        }
                    }
                    "box-shadow" => {
                        decoration.style().box_shadow = Some(pseudo_units.shadows(value, false));
                        pseudo_shadows = pseudo_units.shadows(value, true);
                    }
                    _ => {}
                }
            }
            if pseudo
                .get("background-image")
                .is_some_and(|value| value != "none")
            {
                let base = view
                    .host
                    .borrow()
                    .dom
                    .get(&view.id)
                    .and_then(|state| state.base.clone());
                decoration = decoration.children(css_background(
                    pseudo,
                    units,
                    id,
                    index + 1,
                    [0.; 4],
                    cx.entity(),
                    base.as_ref(),
                    &view.host.borrow().assets,
                ));
            }
            if !pseudo_shadows.is_empty() {
                decoration =
                    decoration.child(inset_shadows(pseudo_shadows, pseudo_radius, pseudo_borders));
            }
            if !text.is_empty() {
                decoration = decoration.child(SharedString::from(text));
            }
            let order = pseudo
                .get("z-index")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            let decoration = if order != 0
                && pseudo
                    .get("position")
                    .is_some_and(|value| value != "static")
            {
                Stacked {
                    inner: Some(decoration.into_any_element()),
                    layers: build.layers.clone(),
                    order,
                    path: build.path.clone(),
                }
                .into_any_element()
            } else {
                decoration.into_any_element()
            };
            if index == 0 {
                el = el.child(decoration);
            } else {
                after = Some(decoration);
            }
        }
    }
    let preserve = style
        .get("white-space")
        .is_some_and(|v| matches!(v.as_str(), "pre" | "pre-wrap" | "break-spaces"))
        || node.tag == "pre";
    if node.tag == "#text"
        && style
            .get("white-space")
            .is_none_or(|value| !matches!(value.as_str(), "pre" | "nowrap"))
    {
        el = el.min_w(px(0.)).max_w(relative(1.)).flex_shrink(1.);
    }
    if preserve && node.text == "\n" {
        el = el.min_h(px(units.font.lh));
    }
    let inline = if editable {
        None
    } else {
        inline_text
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| style::inline_text(doc, styles, id).map(Rc::new))
            .clone()
    };
    let hit_test = inline.is_some();
    if (hit_test || node.tag == "#text")
        && styles.user_select[id].0 != style::UserSelect::None
        && style.get("cursor").is_none_or(|value| value == "auto")
    {
        el = el.cursor(gpui::CursorStyle::IBeam);
    }
    if let Some(data) = inline {
        if style
            .get("container-type")
            .is_some_and(|v| matches!(v.as_str(), "inline-size" | "size"))
        {
            let size = [0, 1].map(|axis| {
                (style
                    .get(["width", "height"][axis])
                    .and_then(|v| units.resolve(v, containing[axis]))
                    .map_or(
                        containing[axis] - [margin[1] + margin[3], 0.][axis],
                        |value| value + extra[axis],
                    )
                    - padding[1 - axis]
                    - padding[3 - axis]
                    - borders[1 - axis]
                    - borders[3 - axis])
                    .max(0.)
            });
            view.host
                .borrow_mut()
                .dom
                .entry(view.id)
                .or_default()
                .containers
                .insert(id, size);
        }
        let inner = gpui::StyledText::new(data.text.to_string()).with_runs(data.runs.clone());
        el = el.child(
            div()
                .min_w(px(0.))
                .max_w(relative(1.))
                .flex_shrink(1.)
                .child(InlineText {
                    inner,
                    data,
                    view: cx.entity(),
                    runs: None,
                    measured: false,
                    justify: style
                        .get("text-align")
                        .is_some_and(|value| value == "justify")
                        .then(|| {
                            style
                                .get("text-align-last")
                                .is_some_and(|value| value == "justify")
                        }),
                }),
        );
    } else if !node.children.is_empty()
        && node.tag != "canvas"
        && node.tag != "iframe"
        && node.tag != "svg"
        && !editable
    {
        let mut content = containing;
        for (axis, (name, min, max)) in [
            ("width", "min-width", "max-width"),
            ("height", "min-height", "max-height"),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(value) = style
                .get(name)
                .and_then(|v| units.resolve(v, containing[axis]))
                .map(|value| value + extra[axis])
                .or(intrinsic_width.filter(|_| axis == 0))
            {
                content[axis] = value;
            } else if axis == 0 {
                content[axis] -= margin[1] + margin[3];
            }
            if let Some(value) = style
                .get(max)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].min(value + extra[axis]);
            }
            if let Some(value) = style
                .get(min)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].max(value + extra[axis]);
            }
            content[axis] = (content[axis]
                - padding[1 - axis]
                - padding[3 - axis]
                - borders[1 - axis]
                - borders[3 - axis])
                .max(0.);
        }
        if let Some((bounds, _)) = &slot {
            content[0] = (bounds[2]
                - padding[1]
                - padding[3]
                - margin[1]
                - margin[3]
                - borders[1]
                - borders[3])
                .max(0.);
            content[1] = (bounds[3]
                - padding[0]
                - padding[2]
                - margin[0]
                - margin[2]
                - borders[0]
                - borders[2])
                .max(0.);
        }
        let container = if style
            .get("container-type")
            .is_some_and(|v| matches!(v.as_str(), "inline-size" | "size"))
        {
            view.host
                .borrow_mut()
                .dom
                .entry(view.id)
                .or_default()
                .containers
                .insert(id, content);
            content
        } else {
            container
        };
        let grid = if let Some(display) = style
            .get("display")
            .filter(|v| matches!(v.as_str(), "grid" | "inline-grid"))
        {
            if display == "inline-grid"
                && slot.is_none()
                && style.get("width").is_none_or(|v| v == "auto")
            {
                let extent =
                    padding[1] + padding[3] + borders[1] + borders[3] + margin[1] + margin[3];
                let [min, max, ..] = super::grid::intrinsic(
                    doc,
                    id,
                    styles,
                    None,
                    None,
                    text,
                    &mut measured.borrow_mut(),
                );
                content[0] = (max.min(content[0] + extent).max(min) - extent).max(0.);
            }
            let height = (slot.is_some() || style.get("height").is_some_and(|v| v != "auto"))
                .then_some(content[1]);
            let cached = view
                .host
                .borrow()
                .dom
                .get(&view.id)
                .and_then(|state| state.grids.get(&id))
                .filter(|(width, cached, inherited, _)| {
                    *width == content[0]
                        && *cached == height
                        && inherited.as_ref() == slot.as_ref().map(|(_, grid)| grid)
                })
                .map(|(_, _, _, grid)| grid.clone());
            Some(cached.unwrap_or_else(|| {
                let grid = super::grid::layout(
                    doc,
                    id,
                    styles,
                    Some(content[0]),
                    height,
                    slot.as_ref().map(|(_, grid)| grid),
                    text,
                    &mut measured.borrow_mut(),
                );
                view.host
                    .borrow_mut()
                    .dom
                    .entry(view.id)
                    .or_default()
                    .grids
                    .insert(
                        id,
                        (
                            content[0],
                            height,
                            slot.as_ref().map(|(_, grid)| grid.clone()),
                            grid.clone(),
                        ),
                    );
                grid
            }))
        } else {
            None
        };
        if let Some(grid) = &grid {
            el = el.flex().flex_col().w(px(content[0]
                + padding[1]
                + padding[3]
                + borders[1]
                + borders[3]));
            if style.get("height").is_none_or(|v| v == "auto") {
                el = el.h(px(grid.rows.iter().sum::<f32>()
                    + grid.rows.len().saturating_sub(1) as f32 * grid.gap[1]
                    + padding[0]
                    + padding[2]
                    + borders[0]
                    + borders[2]));
            }
        }
        let mut children = if grid.is_some() {
            node.children.clone()
        } else {
            flat.clone()
        };
        if grid.is_some() {
            children.sort_by_key(|child| {
                styles.values[*child]
                    .get("z-index")
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            });
        } else if flexbox {
            children.sort_by_key(|child| {
                styles.values[*child]
                    .get("order")
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            });
        }
        let inline_element = |child: Option<&usize>| {
            child.is_some_and(|&child| {
                !doc.nodes[child].tag.starts_with('#')
                    && doc.nodes[child].tag != "br"
                    && inline_level(child)
            })
        };
        let multicol = columns(style, &units, content[0]);
        let mut line: Option<gpui::Div> = None;
        let mut floating = Vec::new();
        // Floats and the content after them, until a `clear`, share a row: left floats, the
        // content, then right floats. Text does not flow under a float that ends early.
        let mut group: Option<(Vec<AnyElement>, Vec<AnyElement>, Vec<AnyElement>)> = None;
        // With multiple columns, block content is collected and split into columns at the end.
        let columns = multicol.filter(|_| flow && grid.is_none() && !all_inline);
        let mut collected: Vec<(Option<usize>, AnyElement)> = Vec::new();
        macro_rules! place {
            ($element:expr, $child:expr) => {{
                let element = $element.into_any_element();
                match group.as_mut() {
                    Some((_, _, content)) => content.push(element),
                    None if columns.is_some() => collected.push(($child, element)),
                    None => el = el.child(element),
                }
            }};
        }
        macro_rules! close_group {
            () => {{
                if let Some(open) = line.take() {
                    place!(open, None);
                }
                if let Some((lefts, rights, content)) = group.take() {
                    let row = div()
                        .flex()
                        .flex_row()
                        .items_start()
                        .w_full()
                        .children(lefts)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.))
                                .children(content),
                        )
                        .children(rights.into_iter().rev());
                    if columns.is_some() {
                        collected.push((None, row.into_any_element()));
                    } else {
                        el = el.child(row);
                    }
                }
            }};
        }
        for (index, &child) in children.iter().enumerate() {
            if preserve
                || !(doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim().is_empty())
                || flow
                    && grid.is_none()
                    && inline_element(index.checked_sub(1).and_then(|index| children.get(index)))
                    && inline_element(children.get(index + 1))
            {
                let placement = grid.as_ref().and_then(|grid| {
                    let index = grid
                        .cells
                        .binary_search_by_key(&child, |(id, ..)| *id)
                        .ok()?;
                    let (_, [column, row, columns, rows], mut bounds) = *grid.cells.get(index)?;
                    bounds[0] += padding[3];
                    bounds[1] += padding[0];
                    Some((
                        bounds,
                        super::grid::Grid {
                            columns: grid.columns.get(column..column + columns)?.to_vec(),
                            rows: grid.rows.get(row..row + rows)?.to_vec(),
                            gap: grid.gap,
                            cells: Vec::new(),
                        },
                    ))
                });
                let child_element = element(build, child, content, container, placement, cx);
                let out_of_flow = styles.values[child]
                    .get("position")
                    .is_some_and(|value| matches!(value.as_str(), "absolute" | "fixed"));
                let child_style = &styles.values[child];
                if grid.is_none() && !out_of_flow {
                    let clear = child_style.get("clear").map_or("none", String::as_str);
                    if group.is_some() && clear != "none" {
                        close_group!();
                    }
                    if floated(child) {
                        if let Some(open) = line.take() {
                            place!(open, None);
                        }
                        let open = group.get_or_insert_with(Default::default);
                        // A float without a width shrinks to fit, but no wider than the row.
                        let float = if child_style.get("width").is_none_or(|value| value == "auto")
                        {
                            div()
                                .flex()
                                .flex_col()
                                .flex_shrink(1.)
                                .min_w(px(0.))
                                .child(child_element)
                                .into_any_element()
                        } else {
                            child_element
                        };
                        if float_side(child_style) == Some(true) {
                            open.1.push(float);
                        } else {
                            open.0.push(float);
                        }
                        continue;
                    }
                }
                if flow
                    && grid.is_none()
                    && child_style
                        .get("position")
                        .is_some_and(|value| value == "absolute")
                    && child_style
                        .get("display")
                        .is_none_or(|value| value != "none")
                    && [["left", "right"], ["top", "bottom"]].iter().any(|axis| {
                        axis.iter()
                            .all(|name| child_style.get(*name).is_none_or(|value| value == "auto"))
                    })
                {
                    // WARNING: the placeholder must come before the box, which reads its bounds in prepaint.
                    let block = child_style
                        .get("display")
                        .is_some_and(|value| !value.starts_with("inline"));
                    let entity = cx.entity();
                    let mut placeholder = gpui::canvas(
                        move |bounds, _, cx| {
                            let view = entity.read(cx);
                            view.host
                                .borrow_mut()
                                .dom
                                .entry(view.id)
                                .or_default()
                                .anchors
                                .insert(child, bounds);
                        },
                        |_, _, _, _| {},
                    )
                    .w(px(0.))
                    .flex_shrink_0()
                    .h(px(
                        if block
                            && (line.is_some()
                                || all_inline
                                    && flat.iter().any(|&other| other != child && in_flow(other)))
                        {
                            units.font.lh
                        } else {
                            0.
                        },
                    ));
                    placeholder.style().align_self = Some(gpui::AlignSelf::FlexStart);
                    match line.take() {
                        Some(open) => line = Some(open.child(placeholder)),
                        None if all_inline || block => place!(placeholder, None),
                        None => {
                            let mut next = div().flex().flex_row().flex_wrap();
                            next.style().justify_content = line_justify;
                            line = Some(next.child(placeholder));
                        }
                    }
                }
                if flow && !all_inline && grid.is_none() && inline_level(child) && !out_of_flow {
                    let mut next = line
                        .take()
                        .unwrap_or_else(|| div().flex().flex_row().flex_wrap());
                    next.style().justify_content = line_justify;
                    next.style().align_items = line_align;
                    line = Some(next.child(child_element));
                } else if out_of_flow {
                    floating.push(child_element);
                } else {
                    if let Some(line) = line.take() {
                        place!(line, None);
                    }
                    place!(child_element, Some(child));
                }
            }
        }
        close_group!();
        if let Some((count, gap, rule)) = columns {
            // Balance the columns by the measured height of each block.
            let width = ((content[0] - gap * (count - 1) as f32) / count as f32).max(0.);
            let heights: Vec<f32> = collected
                .iter()
                .map(|(child, _)| {
                    child.map_or(units.font.lh, |child| {
                        super::grid::intrinsic(
                            doc,
                            child,
                            styles,
                            Some(width),
                            None,
                            text,
                            &mut measured.borrow_mut(),
                        )[3]
                    })
                })
                .collect();
            let target = heights.iter().sum::<f32>() / count as f32;
            let mut columns: Vec<Vec<AnyElement>> = (0..count).map(|_| Vec::new()).collect();
            let (mut column, mut filled) = (0, 0.);
            for ((_, item), height) in collected.into_iter().zip(heights) {
                if filled > 0. && filled + height / 2. > target && column + 1 < count {
                    column += 1;
                    filled = 0.;
                }
                filled += height;
                columns[column].push(item);
            }
            let mut row = div()
                .flex()
                .flex_row()
                .items_start()
                .w_full()
                .gap_x(px(gap));
            for items in columns {
                row = row.child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.))
                        .children(items),
                );
            }
            if let Some((rule_width, color, kind)) = rule {
                row = row.child(column_rules(count, gap, rule_width, color, kind));
            }
            el = el.child(row);
        }
        el = el.children(floating);
    }
    if let Some(after) = after {
        el = el.child(after);
    }
    if slot.is_some() && flow && matches!(node.tag.as_str(), "td" | "th") {
        let align = match style.get("vertical-align").map(String::as_str) {
            Some("middle") => Some(gpui::AlignContent::Center),
            Some("bottom") => Some(gpui::AlignContent::FlexEnd),
            _ => None,
        };
        if all_inline {
            el.style().align_content = align;
        } else {
            el.style().justify_content = align;
        }
    }
    if let Some((bounds, _)) = &slot {
        el = el
            .absolute()
            .left(px(bounds[0]))
            .top(px(bounds[1]))
            .mt(px(margin[0]))
            .mr(px(margin[1]))
            .mb(px(margin[2]))
            .ml(px(margin[3]))
            .w(px((bounds[2] - margin[1] - margin[3]).max(0.)))
            .h(px((bounds[3] - margin[0] - margin[2]).max(0.)));
    }
    if style
        .get("pointer-events")
        .is_none_or(|value| value != "none")
    {
        let path = ancestors.clone();
        el = el.on_mouse_down(MouseButton::Left, cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
            window.focus(&view.focus, cx);
            cx.stop_propagation();
            Desktop::dispatch(view.runtime.clone(), view.id, "pointerdown", if hit_test { &[] } else { &path }, serde_json::json!({"pointerId":1,"pointerType":"mouse","isPrimary":true,"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y),"button":0,"buttons":1,"detail":event.click_count,"ctrlKey":event.modifiers.control,"metaKey":event.modifiers.platform,"altKey":event.modifiers.alt,"shiftKey":event.modifiers.shift}), cx);
        }));
    }
    let mut parent = node.parent;
    while let Some(ancestor) = parent
        && styles.values[ancestor]
            .get("display")
            .is_some_and(|value| value == "contents")
    {
        parent = doc.nodes[ancestor].parent;
    }
    let parent_display = parent
        .and_then(|parent| styles.values[parent].get("display"))
        .map_or("block", String::as_str);
    let axes = [["left", "right"], ["top", "bottom"]].map(|axis| {
        axis.iter()
            .all(|name| style.get(*name).is_none_or(|v| v == "auto"))
    });
    let mut anchor = None;
    if position == "absolute" && slot.is_none() && axes.contains(&true) {
        if matches!(parent_display, "grid" | "inline-grid") {
            if axes[0] {
                el = el.left(px(0.));
            }
            if axes[1] {
                el = el.top(px(0.));
            }
        } else if !matches!(parent_display, "flex" | "inline-flex") {
            let block = !display.starts_with("inline");
            if block
                && axes[0]
                && let Some(parent) = parent
            {
                el = el.left(px(styles.lengths[parent].edges(
                    &styles.values[parent],
                    "padding",
                    containing[0],
                )[3]
                .max(0.)));
            }
            anchor = Some((axes, block, [margin[3], margin[0]]));
        }
    }
    if stacking {
        el = el.child(paint_layers(build.layers.clone(), false));
    }
    let scroll_ancestors = ancestors.clone();
    let el = if style
        .get("pointer-events")
        .is_some_and(|value| value == "none")
    {
        el
    } else {
        el.on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
            cx.stop_propagation();
            let runtime = view.runtime.clone();
            let result = {
                let app = runtime.borrow();
                app.views
                    .get(&view.id)
                    .and_then(|v| v.renderer.as_ref())
                    .map(|engine| {
                        engine.eval(
                            &format!(
                                "__appsDispatch('click',{},{})",
                                serde_json::to_string(if hit_test { &[][..] } else { &ancestors[..] }).unwrap(),
                                serde_json::json!({"clientX": f32::from(event.position().x), "clientY": f32::from(event.position().y), "detail":event.click_count(), "ctrlKey":event.modifiers().control,"metaKey":event.modifiers().platform,"altKey":event.modifiers().alt,"shiftKey":event.modifiers().shift})
                            ),
                            "apps:click",
                        )
                    })
            };
            if let Some(Err(error)) = result {
                eprintln!("apps: {error}");
            }
            cx.notify();
            cx.defer(move |cx| Desktop::drive(runtime, cx));
        }))
    };
    let sticky = (style.get("position").is_some_and(|v| v == "sticky"))
        .then(|| {
            let host = view.host.borrow();
            scroll_ancestors.into_iter().skip(1).find_map(|parent| {
                let scroll = host.dom.get(&view.id)?.scroll.get(&parent)?.clone();
                let style = &styles.values[parent];
                let border = [
                    "border-top-width",
                    "border-right-width",
                    "border-bottom-width",
                    "border-left-width",
                ]
                .map(|name| {
                    style
                        .get(name)
                        .or_else(|| style.get("border-width"))
                        .and_then(|v| styles.lengths[parent].resolve(v, containing[0]))
                        .unwrap_or(0.)
                        .max(0.)
                });
                Some((scroll, border))
            })
        })
        .flatten();
    // Vertical writing modes lay the box out horizontally at its inline size, then turn it.
    let vertical = style
        .get("writing-mode")
        .map(String::as_str)
        .filter(|mode| mode.starts_with("vertical") || mode.starts_with("sideways"))
        .filter(|_| id != 0 && !node.tag.starts_with('#') && slot.is_none());
    let el = match vertical {
        Some(mode) => {
            let [_, widest, ..] = super::grid::intrinsic(
                doc,
                id,
                styles,
                None,
                None,
                text,
                &mut measured.borrow_mut(),
            );
            let inline = style
                .get("height")
                .and_then(|value| units.resolve(value, containing[1]))
                .map(|height| height + extra[1])
                .unwrap_or_else(|| widest.min(units.viewport[1]))
                .max(1.);
            let block = super::grid::intrinsic(
                doc,
                id,
                styles,
                Some(inline),
                None,
                text,
                &mut measured.borrow_mut(),
            )[3];
            div()
                .id(("vertical", id))
                .relative()
                .flex_shrink_0()
                .w(px(block))
                .h(px(inline))
                .child(Turned {
                    child: el
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(px(inline))
                        .into_any_element(),
                    clockwise: mode != "sideways-lr",
                    block,
                    inline,
                })
        }
        None => el,
    };
    let element = DomBox {
        inner: Some(el),
        anchor,
        style: style.clone(),
        effects: effect_layer,
        perspective: parent_perspective.clone().zip(parent),
        units,
        id,
        view: cx.entity(),
        sticky,
        fixed: position == "fixed",
        inset: ["top", "right", "bottom", "left"].map(|name| {
            let axis = usize::from(matches!(name, "top" | "bottom"));
            style.get(name).and_then(|v| {
                units.resolve(
                    v,
                    if position == "fixed" {
                        units.viewport[axis]
                    } else {
                        containing[axis]
                    },
                )
            })
        }),
    }
    .into_any_element();
    if positioned && let Some(order) = order.filter(|order| *order != 0) {
        Stacked {
            inner: Some(element),
            layers: layers.clone(),
            order,
            path: path.clone(),
        }
        .into_any_element()
    } else {
        element
    }
}

/// `Some(true)` for a box floated right, `Some(false)` for left.
fn float_side(style: &style::Style) -> Option<bool> {
    match style.get("float").map(String::as_str) {
        Some("left" | "inline-start") => Some(false),
        Some("right" | "inline-end") => Some(true),
        _ => None,
    }
}

/// The width, color and style of a column rule.
type ColumnRule = (f32, gpui::Hsla, gpui::BorderStyle);

/// The column count, gap and rule of a multi-column box with the given content width.
fn columns(
    style: &style::Style,
    units: &style::Lengths,
    width: f32,
) -> Option<(usize, f32, Option<ColumnRule>)> {
    let mut count = style
        .get("column-count")
        .and_then(|value| value.parse::<usize>().ok());
    let mut column_width = style
        .get("column-width")
        .and_then(|value| units.resolve(value, width));
    for part in style::components(style.get("columns").map_or("", String::as_str), ' ') {
        if let Ok(value) = part.parse::<usize>() {
            count = count.or(Some(value));
        } else if let Some(value) = units.resolve(part, width) {
            column_width = column_width.or(Some(value));
        }
    }
    let gap = style
        .get("column-gap")
        .filter(|value| *value != "normal")
        .and_then(|value| units.resolve(value, width))
        .unwrap_or(units.font.em)
        .max(0.);
    let fit = column_width
        .filter(|value| *value > 0.)
        .map(|value| (((width + gap) / (value + gap)).floor() as usize).max(1));
    let count = match (count, fit) {
        (Some(count), Some(fit)) => count.min(fit),
        (count, fit) => count.or(fit)?,
    }
    .clamp(1, 64);
    if count < 2 {
        return None;
    }
    // The `column-rule` shorthand is split here when the cascade kept it whole.
    let (mut rule_width, mut rule_style, mut rule_color) = (
        style.get("column-rule-width").map(String::as_str),
        style.get("column-rule-style").map(String::as_str),
        style.get("column-rule-color").map(String::as_str),
    );
    for part in style::components(style.get("column-rule").map_or("", String::as_str), ' ') {
        if matches!(
            part,
            "none"
                | "hidden"
                | "dotted"
                | "dashed"
                | "solid"
                | "double"
                | "groove"
                | "ridge"
                | "inset"
                | "outset"
        ) {
            rule_style = rule_style.or(Some(part));
        } else if style::color(part).is_some() {
            rule_color = rule_color.or(Some(part));
        } else {
            rule_width = rule_width.or(Some(part));
        }
    }
    let rule = rule_style
        .filter(|kind| !matches!(*kind, "none" | "hidden"))
        .map(|kind| {
            let width = rule_width
                .map_or(Some(3.), |value| units.resolve(value, width))
                .unwrap_or(0.);
            let color = rule_color
                .or_else(|| style.get("color").map(String::as_str))
                .and_then(style::color)
                .map_or(gpui::black(), |(ink, alpha)| {
                    rgba(ink << 8 | alpha as u32).into()
                });
            (width, color, border_style(kind))
        })
        .filter(|(width, ..)| *width > 0.);
    Some((count, gap, rule))
}

/// Vertical lines centered in the gaps between columns.
fn column_rules(
    count: usize,
    gap: f32,
    width: f32,
    color: gpui::Hsla,
    kind: gpui::BorderStyle,
) -> AnyElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let column = (f32::from(bounds.size.width) - gap * (count - 1) as f32) / count as f32;
            for index in 1..count {
                let x = bounds.left() + px(index as f32 * (column + gap) - gap / 2. - width / 2.);
                window.paint_quad(gpui::quad(
                    Bounds::new(point(x, bounds.top()), size(px(width), bounds.size.height)),
                    gpui::Corners::default(),
                    gpui::transparent_black(),
                    gpui::Edges {
                        left: px(width),
                        ..Default::default()
                    },
                    color,
                    kind,
                ));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

fn border_style(kind: &str) -> gpui::BorderStyle {
    match kind {
        "dashed" => gpui::BorderStyle::Dashed,
        "dotted" => gpui::BorderStyle::Dotted,
        "double" => gpui::BorderStyle::Double,
        _ => gpui::BorderStyle::Solid,
    }
}

/// The darker or lighter shade of a 3D border, like WebKit.
fn shade_border(color: gpui::Hsla, dark: bool) -> gpui::Hsla {
    let rgba = color.to_rgb();
    let value = rgba.r.max(rgba.g).max(rgba.b);
    let factor = if value <= 0. {
        return if dark {
            color
        } else {
            gpui::Hsla::from(gpui::Rgba {
                r: 0.33,
                g: 0.33,
                b: 0.33,
                a: rgba.a,
            })
        };
    } else if dark {
        ((value - 0.33) / value).max(0.)
    } else {
        (value + 0.33).min(1.) / value
    };
    gpui::Rgba {
        r: (rgba.r * factor).min(1.),
        g: (rgba.g * factor).min(1.),
        b: (rgba.b * factor).min(1.),
        a: rgba.a,
    }
    .into()
}

fn inset_shadows(shadows: Vec<gpui::BoxShadow>, radius: [f32; 4], borders: [f32; 4]) -> AnyElement {
    let inner =
        |corner: usize, a: usize, b: usize| (radius[corner] - borders[a].max(borders[b])).max(0.);
    let radii = super::effects::corners([
        inner(0, 0, 3),
        inner(1, 0, 1),
        inner(2, 2, 1),
        inner(3, 2, 3),
    ]);
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| window.paint_inset_shadows(bounds, radii, &shadows),
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

fn svg_layer(source: String, id: usize, view: gpui::Entity<WebView>) -> AnyElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let [width, height] = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            if width <= 0. || height <= 0. {
                return;
            }
            let scale = window.scale_factor();
            let raster =
                [width, height].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
            let view = view.read(cx);
            let mut host = view.host.borrow_mut();
            let state = host.dom.entry(view.id).or_default();
            if state.svgs.get(&id).is_none_or(|(old, image)| {
                *old != source
                    || u32::from(image.size(0).width) != raster[0]
                    || u32::from(image.size(0).height) != raster[1]
            }) {
                match super::canvas::svg(&source, raster[0], raster[1], None) {
                    Ok(image) => {
                        if let Some((_, old)) = state.svgs.insert(id, (source.clone(), image)) {
                            let _ = window.drop_image(old);
                        }
                    }
                    Err(error) => eprintln!("apps: SVG: {error}"),
                }
            }
            let image = state.svgs.get(&id).map(|(_, image)| image.clone());
            drop(host);
            if let Some(image) = image {
                let _ =
                    window.paint_image(bounds, bounds, gpui::Corners::default(), image, 0, false);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}
