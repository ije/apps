use super::{
    ViewData,
    background::css_background,
    cull,
    paint::{DomBox, PaintLayers, Stacked, Turned, paint_layers},
    scrollbar,
    text::{InlineText, TextHighlight, paint_text},
};
use crate::style::Prop;
use crate::{
    desktop::Desktop,
    dom::Document,
    style::{self, ComputedStyles},
};
use renderer::{
    AnyElement, Bounds, MouseButton, SharedString, div, img, point, prelude::*, px, relative, rgba,
    size,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

pub(super) struct Build<'a> {
    pub(super) doc: &'a Document,
    pub(super) styles: &'a ComputedStyles,
    pub(super) text: &'a renderer::TextSystem,
    pub(super) pixel_ratio: f32,
    pub(super) view: &'a Rc<ViewData>,
    pub(super) layers: PaintLayers,
    /// The boxes from the root to the parent.
    pub(super) ancestors: Rc<[usize]>,
    /// Where the boxes after the stacking context root start in `ancestors`.
    pub(super) stacking: usize,
    pub(super) flex_parent: bool,
    pub(super) measured: &'a RefCell<super::grid::Measurements>,
    pub(super) inline_text: &'a RefCell<HashMap<usize, Option<Rc<style::InlineText>>>>,
    /// The parent's `perspective` distance and `perspective-origin`.
    pub(super) perspective: Option<(f32, String)>,
    /// The in-flow children (`false`) and list items (`true`) of a parent, with their indexes.
    pub(super) siblings: &'a RefCell<HashMap<(usize, bool), Rc<Siblings>>>,
    pub(super) ids: &'a Rc<super::effects::Ids>,
    pub(super) info: &'a [cull::Info],
    /// The visible range for culling children, or `None` where nothing is culled.
    pub(super) zone: Option<Rc<cull::Zone>>,
}

type Siblings = (Vec<usize>, rustc_hash::FxHashMap<usize, usize>);

pub(super) fn element(
    build: &Build<'_>,
    id: usize,
    containing: [f32; 2],
    container: [f32; 2],
    slot: Option<([f32; 4], super::grid::Grid)>,
) -> AnyElement {
    let Build {
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        ref layers,
        ref ancestors,
        stacking: stack,
        flex_parent,
        measured,
        inline_text,
        perspective: ref parent_perspective,
        siblings,
        ids,
        info,
        ref zone,
    } = *build;
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    let display =
        style
            .get(Prop::DISPLAY)
            .unwrap_or(if node.tag.starts_with('#') && node.tag != "#text" {
                "block"
            } else {
                "inline"
            });
    let order = style
        .get(Prop::Z_INDEX)
        .and_then(|value| value.parse::<i32>().ok());
    let positioned = style
        .get(Prop::POSITION)
        .is_some_and(|value| value != "static");
    let stacking = id == 0
        || style
            .get(Prop::ISOLATION)
            .is_some_and(|value| value == "isolate")
        || style
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "fixed" | "sticky"))
        || positioned && order.is_some()
        || style
            .get(Prop::TRANSFORM)
            .is_some_and(|value| value != "none")
        || [Prop::FILTER, Prop::CLIP_PATH]
            .into_iter()
            .any(|key| style.get(key).is_some_and(|value| value != "none"))
        || style
            .get(Prop::OPACITY)
            .and_then(|value| value.parse::<f32>().ok())
            .is_some_and(|value| value < 1.);
    let chain: Rc<[usize]> = ancestors.iter().copied().chain([id]).collect();
    let child_build = Build {
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        measured,
        inline_text,
        siblings,
        ids,
        info,
        zone: zone
            .as_ref()
            .filter(|_| info[id].flags & (cull::CHANGED | cull::MOVED) == 0)
            .and_then(|zone| {
                if style
                    .get(Prop::OVERFLOW_Y)
                    .is_none_or(|value| value == "visible")
                {
                    return Some(zone.clone());
                }
                let rect = *view.host.borrow().dom.get(&view.id)?.bounds.get(&id)?;
                Some(Rc::new(cull::Zone {
                    top: zone.top.max(rect[1]),
                    bottom: zone.bottom.min(rect[1] + rect[3]),
                    clips: zone.clips.iter().copied().chain([id]).collect(),
                }))
            }),
        layers: if stacking {
            Rc::default()
        } else {
            layers.clone()
        },
        stacking: if stacking { chain.len() } else { stack },
        ancestors: chain.clone(),
        flex_parent: matches!(display, "flex" | "inline-flex"),
        perspective: style
            .get(Prop::PERSPECTIVE)
            .filter(|value| *value != "none")
            .and_then(|value| styles.lengths[id].resolve(value, 0.))
            .map(|distance| {
                (
                    distance,
                    style
                        .get(Prop::PERSPECTIVE_ORIGIN)
                        .map(str::to_owned)
                        .unwrap_or_else(|| "50% 50%".into()),
                )
            }),
    };
    let build = &child_build;
    let mut units = styles.lengths[id];
    units.container = Some(container);
    // WARNING: element building and layout recurse per level, so deep pages must not overflow the stack.
    if chain.len() > 256 || style.get(Prop::DISPLAY).is_some_and(|s| s == "none") {
        return div().hidden().into_any_element();
    }
    let opacity = style
        .get(Prop::OPACITY)
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
        Prop::FILTER,
        Prop::CLIP_PATH,
        Prop::MASK_IMAGE,
        Prop::WEBKIT_MASK_IMAGE,
        Prop::BACKDROP_FILTER,
        Prop::WEBKIT_BACKDROP_FILTER,
    ]
    .into_iter()
    .any(|name| style.get(name).is_some_and(|value| value != "none"))
        || style
            .get(Prop::MIX_BLEND_MODE)
            .is_some_and(|value| value != "normal")
        || style
            .get(Prop::ISOLATION)
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
    el.style().flex_shrink = if flex_parent { 1. } else { 0. };
    {
        let text = el.text_style();
        text.letter_spacing = Some(style::spacing(style, &units, Prop::LETTER_SPACING));
        text.word_spacing = Some(style::spacing(style, &units, Prop::WORD_SPACING));
        text.font_smoothing = Some(
            ![Prop::WEBKIT_FONT_SMOOTHING, Prop::MOZ_OSX_FONT_SMOOTHING]
                .into_iter()
                .any(|name| {
                    style
                        .get(name)
                        .is_some_and(|v| v == "antialiased" || v == "grayscale")
                }),
        );
        text.text_indent = Some(px(style
            .get(Prop::TEXT_INDENT)
            .and_then(|value| units.resolve(value, containing[0]))
            .unwrap_or(0.)));
    }
    if face.style != renderer::FontStyle::Normal {
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
            .get(Prop::DISPLAY)
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
                .get(Prop::POSITION)
                .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
    };
    let inline_level = |child: usize| {
        doc.nodes[child].tag.starts_with('#')
            || !floated(child)
                && styles.values[child]
                    .get(Prop::DISPLAY)
                    .is_none_or(|value| value.starts_with("inline"))
            || styles.values[child]
                .get(Prop::POSITION)
                .is_some_and(|value| matches!(value, "absolute" | "fixed"))
    };
    let all_inline = flow && !flat.is_empty() && flat.iter().all(|&child| inline_level(child));
    let line_justify = match style.get(Prop::TEXT_ALIGN) {
        Some("right" | "end") => Some(renderer::JustifyContent::End),
        Some("center" | "-webkit-center") => Some(renderer::JustifyContent::Center),
        _ => None,
    };
    let line_align = Some(
        if flat.iter().any(|&child| {
            styles.values[child]
                .get(Prop::VERTICAL_ALIGN)
                .is_some_and(|value| value == "middle")
        }) {
            renderer::AlignItems::Center
        } else {
            renderer::AlignItems::Baseline
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
        if let Some(image) = view.images.borrow().get(&id) {
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
                let size = image.size();
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
            let auto = |prop| style.get(prop).is_none_or(|value| value == "auto");
            if natural[1] > 0. {
                el.style().aspect_ratio = Some(natural[0] / natural[1]);
            }
            match (
                auto(Prop::WIDTH),
                auto(Prop::HEIGHT),
                attr("width"),
                attr("height"),
            ) {
                (true, _, Some(width), _) => el = el.w(px(width)),
                (true, true, None, None) => el = el.w(px(natural[0])),
                _ => {}
            }
            if auto(Prop::HEIGHT)
                && let Some(height) = attr("height")
            {
                el = el.h(px(height));
            }
            let fit = match style.get(Prop::OBJECT_FIT) {
                Some("contain") => renderer::ObjectFit::Contain,
                Some("cover") => renderer::ObjectFit::Cover,
                Some("none") => renderer::ObjectFit::None,
                Some("scale-down") => renderer::ObjectFit::ScaleDown,
                _ => renderer::ObjectFit::Fill,
            };
            if let Some(image) = image {
                if matches!(fit, renderer::ObjectFit::Cover | renderer::ObjectFit::None) {
                    cropped = Some((image, fit));
                } else {
                    image_child = Some(img(image).size_full().object_fit(fit));
                }
            } else if vector.is_some() {
                el = el.child(svg_layer(id, false, view.clone()));
            }
        } else {
            // WARNING: images that are loading or failed to decode keep only their attribute size.
            for (name, prop, horizontal) in [
                ("width", Prop::WIDTH, true),
                ("height", Prop::HEIGHT, false),
            ] {
                if style.get(prop).is_none_or(|value| value == "auto")
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
        }
    }
    if node.tag == "svg" {
        let width = style
            .get(Prop::WIDTH)
            .and_then(|v| units.resolve(v, containing[0]))
            .or_else(|| node.attrs.get("width").and_then(|v| v.parse().ok()))
            .unwrap_or(24.)
            .clamp(1., 4096.);
        let height = style
            .get(Prop::HEIGHT)
            .and_then(|v| units.resolve(v, containing[1]))
            .or_else(|| node.attrs.get("height").and_then(|v| v.parse().ok()))
            .unwrap_or(24.)
            .clamp(1., 4096.);
        let color = style.get(Prop::COLOR).unwrap_or("");
        let cached = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|state| state.svg_sources.get(&id))
            .is_some_and(|(size, old, _)| *size == [width, height] && old == color);
        if !cached {
            let mut source = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\""
            );
            for (key, value) in &node.attrs {
                if !matches!(
                    key.as_str(),
                    "width" | "height" | "xmlns" | "xmlns:xlink" | "style"
                ) && Prop::find(key).is_none_or(|prop| !style.contains(prop))
                {
                    source.push_str(&format!(" {key}=\"{}\"", crate::dom::escape(value)));
                }
            }
            source.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
            for prop in [
                Prop::FILL,
                Prop::FILL_OPACITY,
                Prop::FILL_RULE,
                Prop::STROKE,
                Prop::STROKE_WIDTH,
                Prop::STROKE_OPACITY,
                Prop::STROKE_LINECAP,
                Prop::STROKE_LINEJOIN,
                Prop::COLOR,
            ] {
                if let Some(value) = style.get(prop) {
                    source.push_str(&format!(
                        " {}=\"{}\"",
                        prop.name(),
                        crate::dom::escape(value)
                    ));
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
                    && let Some(target) =
                        super::effects::index(ids, doc)
                            .get(reference)
                            .and_then(|targets| {
                                targets.iter().copied().find(|&target| {
                                    let mut root = target;
                                    while let Some(parent) = doc.nodes[root].parent {
                                        if parent == id {
                                            return false;
                                        }
                                        root = parent;
                                    }
                                    root == scope
                                })
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
            // CSS keywords ignore case, and resvg does not parse every CSS color, so `currentcolor`
            // becomes the resolved color in hex.
            let current = style::color(style.get(Prop::COLOR).unwrap_or(""))
                .map_or("black".to_owned(), |(rgb, alpha)| {
                    format!("#{rgb:06x}{alpha:02x}")
                });
            let lower = source.to_ascii_lowercase();
            let mut end = 0;
            let mut replaced = String::with_capacity(source.len());
            for (start, _) in lower.match_indices("currentcolor") {
                replaced.push_str(&source[end..start]);
                replaced.push_str(&current);
                end = start + "currentcolor".len();
            }
            replaced.push_str(&source[end..]);
            source = replaced;
            let mut host = view.host.borrow_mut();
            let state = host.dom.entry(view.id).or_default();
            // WARNING: paint only checks the raster size of inline SVGs, so a changed source drops the raster here.
            if state.svgs.get(&id).is_some_and(|(old, _)| *old != source)
                && let Some((_, image)) = state.svgs.remove(&id)
            {
                state.retired.push(image);
            }
            state
                .svg_sources
                .insert(id, ([width, height], color.to_owned(), source));
        }
        el = el
            .w(px(width))
            .h(px(height))
            .child(svg_layer(id, true, view.clone()));
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
        let text_color = style
            .get(Prop::COLOR)
            .and_then(style::color)
            .map(|(c, a)| rgba((c << 8) | a as u32))
            .unwrap_or(renderer::black());
        let caret_color = style
            .get(Prop::CARET_COLOR)
            .filter(|value| !matches!(*value, "auto" | "currentColor"))
            .and_then(style::color)
            .map(|(color, alpha)| rgba((color << 8) | alpha as u32))
            .unwrap_or(text_color);
        // WebKit draws placeholders in darkgray unless `::placeholder` sets a color.
        let placeholder_color = styles
            .text_pseudos
            .get(&id)
            .and_then(|pseudos| style::color(pseudos[3].as_ref()?.get(Prop::COLOR)?))
            .map(|(color, alpha)| rgba((color << 8) | alpha as u32))
            .unwrap_or(rgba(0xa9a9a9ff));
        let single_line = node.tag != "textarea";
        // The text is drawn in the content box.
        let padding = units.edges(style, style::PADDING, containing[0]);
        let entity = view.clone();
        el = el.cursor(renderer::CursorStyle::IBeam).child(
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| {
                    let view = &*entity;
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
                        let run = renderer::TextRun {
                            len: text.len(),
                            font: face.clone(),
                            color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                            letter_spacing: renderer::px(0.),
                            word_spacing: renderer::px(0.),
                            font_size: None,
                        };
                        let line = window.text_system().shape_line(
                            text.to_owned().into(),
                            px(font_size),
                            std::slice::from_ref(&run),
                            None,
                        );
                        let length = text.encode_utf16().count();
                        // A single-line input centers its line, like browsers.
                        let middle = if single_line {
                            ((bounds.size.height - px(line_height)) / 2.).max(px(0.))
                        } else {
                            px(0.)
                        };
                        let top = bounds.top() + middle + px(row as f32 * line_height);
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
                                window.paint_quad(renderer::fill(
                                    Bounds::new(
                                        point(bounds.left() + x, top),
                                        size(px(1.), px(line_height)),
                                    ),
                                    caret_color,
                                ));
                            } else if end_x > x {
                                highlights.push(TextHighlight {
                                    range: left..right,
                                    color: selection_colors.0.map(rgba),
                                    background: rgba(selection_colors.1),
                                });
                            }
                        }
                        if highlights.is_empty() {
                            line.paint(
                                point(bounds.left(), top),
                                px(line_height),
                                renderer::TextAlign::Left,
                                None,
                                window,
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
            .top(px(padding[0]))
            .right(px(padding[1]))
            .bottom(px(padding[2]))
            .left(px(padding[3])),
        );
    }
    if let Some((ink, alpha)) = style.get(Prop::COLOR).and_then(style::color) {
        el = el.text_color(rgba((ink << 8) | alpha as u32));
    }
    if let Some(color) = style
        .get(Prop::BACKGROUND_COLOR)
        .and_then(style::color)
        .filter(|(_, alpha)| *alpha > 0)
    {
        let color = rgba((color.0 << 8) | color.1 as u32);
        let blended = style
            .get(Prop::BACKGROUND_BLEND_MODE)
            .is_some_and(|value| value.split(',').any(|mode| mode.trim() != "normal"))
            && style
                .get(Prop::BACKGROUND_IMAGE)
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
                .get(Prop::POSITION)
                .is_some_and(|value| value == "absolute")
        {
            let font = text.resolve_font(&style::font(style));
            let font_size = px(units.font.em);
            let height = text.ascent(font, font_size) - text.descent(font, font_size);
            let line_height = px(units.font.lh);
            let radius = px(style
                .get(Prop::BORDER_RADIUS)
                .and_then(|value| units.resolve(value, containing[0]))
                .unwrap_or(0.));
            el = el.child(
                renderer::canvas(
                    |_, _| (),
                    move |bounds, _, window| {
                        let step = line_height.max(px(1.));
                        let mask = window.content_mask().bounds;
                        let mut top = bounds.top() + (line_height - height) / 2.;
                        top += step * ((mask.top() - top - height) / step).floor().max(0.);
                        while top < bounds.bottom().min(mask.bottom()) {
                            window.paint_quad(
                                renderer::fill(
                                    Bounds::new(
                                        point(bounds.left(), top),
                                        size(bounds.size.width, height),
                                    ),
                                    color,
                                )
                                .corner_radii(radius),
                            );
                            top += step;
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
    // CSS: `visible` and `clip` compute to `auto` and `hidden` when the other axis scrolls or hides. The root
    // and the body keep theirs, as the page scrolls for them.
    let overflow =
        [Prop::OVERFLOW_X, Prop::OVERFLOW_Y].map(|prop| style.get(prop).unwrap_or("visible"));
    let page = matches!(node.tag.as_str(), "html" | "body");
    let boxed = !page
        && overflow
            .iter()
            .any(|value| !matches!(*value, "visible" | "clip"));
    let overflow = overflow.map(|value| match value {
        "auto" | "scroll" => renderer::Overflow::Scroll,
        "hidden" => renderer::Overflow::Hidden,
        "clip" if boxed => renderer::Overflow::Hidden,
        "clip" => renderer::Overflow::Clip,
        _ if boxed => renderer::Overflow::Scroll,
        _ => renderer::Overflow::Visible,
    });
    if overflow != [renderer::Overflow::Visible; 2] {
        el.style().overflow.x = overflow[0];
        el.style().overflow.y = overflow[1];
    }
    let scrolls = overflow.map(|axis| axis == renderer::Overflow::Scroll);
    let look = if scrolls.contains(&true) {
        scrollbar::Look::new(
            style,
            styles.scrollbars.get(&id).map(|parts| &**parts),
            &units,
            chain.iter().rev().find_map(|&id| {
                let (ink, alpha) = style::color(styles.values[id].get(Prop::BACKGROUND_COLOR)?)?;
                (alpha >= 128).then_some(ink)
            }),
        )
        .map(|mut look| {
            if page {
                look.forced = [false; 2];
            }
            look
        })
    } else {
        None
    };
    // Custom bars take their space when last frame's box overflowed.
    let reserved = look.as_ref().map_or([0.; 2], |look| {
        let max = view
            .host
            .borrow()
            .dom
            .get(&view.id)
            .and_then(|dom| dom.scroll.get(&id))
            .map(renderer::ScrollHandle::max_offset)
            .unwrap_or_default();
        look.space(look.shown(scrolls, max))
    });
    let mut padding = units
        .edges(style, style::PADDING, containing[0])
        .map(|value| value.max(0.));
    padding[1] += reserved[1];
    padding[2] += reserved[0];
    el = el
        .pt(px(padding[0]))
        .pr(px(padding[1]))
        .pb(px(padding[2]))
        .pl(px(padding[3]));
    let margin = margins(
        build,
        id,
        &units,
        containing[0],
        flex_parent,
        slot.is_some(),
    );
    el = el
        .mt(px(margin[0]))
        .mr(px(margin[1]))
        .mb(px(margin[2]))
        .ml(px(margin[3]));
    for (side, prop) in style::MARGIN[1..].iter().enumerate() {
        if style.get(*prop).is_some_and(|s| s == "auto") {
            el = match side {
                0 => el.mt_auto(),
                1 => el.mr_auto(),
                2 => el.mb_auto(),
                _ => el.ml_auto(),
            };
        }
    }
    let ink = style
        .get(Prop::COLOR)
        .and_then(style::color)
        .map_or(rgba(0x000000ff), |(ink, alpha)| {
            rgba(ink << 8 | alpha as u32)
        });
    let sides = style::BORDERS.map(|[width, kind, color]| (width, kind, color));
    let borders = sides.map(|(width, kind, _)| {
        if style
            .get(kind)
            .is_none_or(|kind| matches!(kind, "none" | "hidden"))
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
            .and_then(style::color)
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
    let border_kinds = sides.map(|(_, kind, _)| style.get(kind).unwrap_or("none"));
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
            el = el.border_color(renderer::transparent_black());
        }
        if visible
            .iter()
            .all(|&side| border_kinds[side] == border_kinds[first])
        {
            el.style().border_style = border_style(border_kinds[first]);
        } else if visible
            .iter()
            .any(|&side| matches!(border_kinds[side], "dashed" | "dotted"))
        {
            el.style().border_style = renderer::BorderStyle::Dashed;
        }
    }
    let extra = if style
        .get(Prop::BOX_SIZING)
        .is_some_and(|value| value == "border-box")
    {
        [0.; 2]
    } else {
        [
            padding[1] + padding[3] + borders[1] + borders[3] - reserved[1],
            padding[0] + padding[2] + borders[0] + borders[2] - reserved[0],
        ]
    };
    if editable {
        // Fields size their content like WebKit, then add padding and borders whatever `box-sizing` is.
        let chrome = [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ];
        let textarea = node.tag == "textarea";
        let count = |name, default| {
            node.attrs
                .get(name)
                .and_then(|value| value.trim().parse::<u32>().ok())
                .filter(|&value| value > 0)
                .unwrap_or(default) as f32
        };
        if style.get(Prop::WIDTH).is_none_or(|v| v == "auto") {
            let face = style::font(style);
            let em = px(units.font.em);
            let width = if style
                .get(Prop::FIELD_SIZING)
                .is_some_and(|v| v == "content")
            {
                // The box fits its text, or the placeholder while empty, plus the caret.
                let shown = node
                    .attrs
                    .get("value")
                    .filter(|value| !value.is_empty())
                    .or_else(|| node.attrs.get("placeholder"))
                    .cloned()
                    .unwrap_or_default();
                shown
                    .split('\n')
                    .map(|line| {
                        let run = renderer::TextRun {
                            len: line.len(),
                            font: face.clone(),
                            color: renderer::black(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                            letter_spacing: px(0.),
                            word_spacing: px(0.),
                            font_size: None,
                        };
                        f32::from(
                            text.shape_line(line.to_owned().into(), em, &[run], None)
                                .width,
                        )
                    })
                    .fold(0., f32::max)
                    + 1.
            } else {
                // `size` or `cols` characters. System fonts and fonts with a bad OS/2 table use the width
                // of `0`; other fonts use the OS/2 average, and inputs add the widest glyph's extra width.
                let family = style
                    .get(Prop::FONT_FAMILY)
                    .and_then(|value| value.split(',').next())
                    .map(|value| value.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
                    .unwrap_or_default();
                let id = text.resolve_font(&face);
                let zero = text
                    .ch_advance(id, em)
                    .map_or(units.font.em / 2., f32::from);
                let average = f32::from(text.average_width(id, em));
                let (char_width, extra) = if family.starts_with("ui-")
                    || "|system-ui|-apple-system|blinkmacsystemfont|american typewriter|arial hebrew|\
                        chalkboard|cochin|corsiva hebrew|courier|euphemia ucas|geneva|gill sans|hei|\
                        helvetica|hoefler text|inaimathi|kai|lucida grande|marker felt|monaco|mshtakan|\
                        new peninim mt|osaka|raanana|stheiti|symbol|times|apple braille|apple ligothic|\
                        apple lisung|apple symbols|applegothic|applemyungjo|#gungseo|#headlinea|\
                        #pcmyungjo|#pilgi|"
                        .contains(&format!("|{family}|"))
                {
                    (zero, 0.)
                } else {
                    let average = if average > 0. { average } else { zero }.round();
                    let widest = f32::from(text.bounding_box(id, em).size.width).round();
                    (average, if textarea { 0. } else { widest - average })
                };
                if textarea {
                    (char_width * count("cols", 20)).ceil()
                } else {
                    // The inner text box has 1px padding on each side.
                    (char_width * count("size", 20) * 64.).ceil() / 64. + extra + 2.
                }
            };
            el = el.w(px(width + chrome[0]));
        }
        if style.get(Prop::HEIGHT).is_none_or(|value| value == "auto") {
            let rows = if textarea { count("rows", 2) } else { 1. };
            el = el.min_h(px(rows * units.font.lh + chrome[1]));
        }
    }
    let flexbox = matches!(display, "flex" | "inline-flex");
    let position = style.get(Prop::POSITION).unwrap_or("static");
    let mut radius = [0.; 4];
    let mut inner_shadows = Vec::new();
    let mut intrinsic_width = None;
    for (prop, value) in style.iter() {
        let value: &str = value;
        let vertical = matches!(
            prop,
            Prop::HEIGHT | Prop::MIN_HEIGHT | Prop::MAX_HEIGHT | Prop::TOP | Prop::BOTTOM
        );
        let base = containing[usize::from(vertical)];
        let length = || units.resolve(value, base);
        match prop {
            Prop::WIDTH
            | Prop::HEIGHT
            | Prop::MIN_WIDTH
            | Prop::MAX_WIDTH
            | Prop::MIN_HEIGHT
            | Prop::MAX_HEIGHT => {
                let axis = usize::from(vertical);
                let length: renderer::Length = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    if extra[axis] > 0. && base > 0. {
                        px(percent.max(0.) / 100. * base + extra[axis]).into()
                    } else {
                        relative(percent.max(0.) / 100.)
                    }
                } else if let Some(length) = length() {
                    px(length.max(0.) + extra[axis]).into()
                } else if matches!(prop, Prop::WIDTH | Prop::HEIGHT) && value == "auto" {
                    renderer::Length::Auto
                } else if matches!(value, "stretch" | "-webkit-fill-available") {
                    relative(1.)
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
                el = match prop {
                    Prop::WIDTH => el.w(length),
                    Prop::HEIGHT => el.h(length),
                    Prop::MIN_WIDTH => el.min_w(length),
                    Prop::MAX_WIDTH => el.max_w(length),
                    Prop::MIN_HEIGHT => el.min_h(length),
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
            Prop::BORDER_TOP_LEFT_RADIUS
            | Prop::BORDER_TOP_RIGHT_RADIUS
            | Prop::BORDER_BOTTOM_RIGHT_RADIUS
            | Prop::BORDER_BOTTOM_LEFT_RADIUS => {
                let own = style
                    .get(Prop::WIDTH)
                    .and_then(|width| units.resolve(width, containing[0]))
                    .unwrap_or(containing[0]);
                let Some(length) = units.resolve(value, own) else {
                    continue;
                };
                let length = length.max(0.);
                let corner = match prop {
                    Prop::BORDER_TOP_LEFT_RADIUS => 0,
                    Prop::BORDER_TOP_RIGHT_RADIUS => 1,
                    Prop::BORDER_BOTTOM_RIGHT_RADIUS => 2,
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
            Prop::BOX_SHADOW => {
                el.style().box_shadow = units.shadows(value, false);
                inner_shadows = units.shadows(value, true);
            }
            Prop::ASPECT_RATIO => {
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
            Prop::DISPLAY if matches!(value, "grid" | "inline-grid") => el = el.grid(),
            Prop::DISPLAY if matches!(value, "flex" | "inline-flex") => el = el.flex().flex_row(),
            Prop::FLEX_DIRECTION if flexbox => {
                el.style().flex_direction = match value {
                    "row-reverse" => renderer::FlexDirection::RowReverse,
                    "column" => renderer::FlexDirection::Column,
                    "column-reverse" => renderer::FlexDirection::ColumnReverse,
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
            Prop::FLEX_GROW | Prop::FLEX_SHRINK => {
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
            Prop::FLEX_BASIS => {
                el.style().flex_basis = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    relative(percent.max(0.) / 100.)
                } else if let Some(length) = length() {
                    // The basis runs along the parent's main axis.
                    let column = node.parent.is_some_and(|parent| {
                        styles.values[parent]
                            .get(Prop::FLEX_DIRECTION)
                            .is_some_and(|v| v.starts_with("column"))
                    });
                    px(length.max(0.) + extra[usize::from(column)]).into()
                } else {
                    renderer::Length::Auto
                };
            }
            Prop::ALIGN_ITEMS | Prop::ALIGN_SELF if flex_parent || flexbox => {
                let align = match value {
                    "center" => renderer::AlignItems::Center,
                    "flex-start" | "start" | "self-start" => renderer::AlignItems::FlexStart,
                    "flex-end" | "end" | "self-end" => renderer::AlignItems::FlexEnd,
                    "baseline" | "first baseline" | "last baseline" => {
                        renderer::AlignItems::Baseline
                    }
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
                let align = match value {
                    "center" => renderer::AlignContent::Center,
                    "flex-end" | "end" | "right" => renderer::AlignContent::FlexEnd,
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
                el = match value {
                    "center" => el.text_center(),
                    "right" | "end" => el.text_right(),
                    _ => el.text_left(),
                }
            }
            Prop::POSITION if matches!(value, "absolute" | "fixed") => el = el.absolute(),
            Prop::LEFT | Prop::RIGHT | Prop::TOP | Prop::BOTTOM
                if matches!(position, "relative" | "absolute") =>
            {
                let length: renderer::Length = if let Some(percent) = value
                    .strip_suffix('%')
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    relative(percent / 100.)
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
            Prop::TEXT_WRAP => {
                el.text_style().text_wrap = Some(if value.contains("balance") {
                    renderer::TextWrap::Balance
                } else if value.contains("pretty") {
                    renderer::TextWrap::Pretty
                } else {
                    renderer::TextWrap::Wrap
                })
            }
            Prop::CURSOR => {
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
        let align = style.get(Prop::VERTICAL_ALIGN).unwrap_or("baseline");
        el.style().align_self = match align {
            "middle" => Some(renderer::AlignItems::Center),
            "top" | "text-top" => Some(renderer::AlignItems::FlexStart),
            "bottom" => Some(renderer::AlignItems::FlexEnd),
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
        // WARNING: line boxes align box bottoms, so boxes are lifted until their baseline meets the parent's.
        let lift = if el.style().align_self.is_none() && align != "text-bottom" {
            let parent = below(&styles.values[parent], line);
            if replaced {
                parent
            } else if field {
                parent - padding[2] - borders[2] - below(style, units.font)
            } else if display == "inline" {
                parent - below(style, units.font)
            } else {
                0.
            }
        } else {
            0.
        };
        // Relative offsets apply after line layout, so they add to the shift. Percentages keep their own inset.
        let inset = |prop| style.get(prop).filter(|value| *value != "auto");
        let offset = match (inset(Prop::TOP), inset(Prop::BOTTOM)) {
            _ if position != "relative" => Some(0.),
            (Some(top), _) => units.resolve(top, f32::NAN),
            (None, Some(bottom)) => units.resolve(bottom, f32::NAN).map(|bottom| -bottom),
            (None, None) => Some(0.),
        };
        let raise = match align {
            _ if offset.is_none() => 0.,
            "super" => line.em / 3. + lift,
            "sub" => -line.em / 5. + lift,
            "baseline" => lift,
            value => units.resolve(value, units.font.lh).unwrap_or(0.) + lift,
        };
        // WARNING: the margin reserves the shift so the line box grows instead of overlapping its neighbors.
        if raise > 0. {
            el = el.mt(px(margin[0] + raise));
        } else if raise < 0. {
            el = el.mb(px(margin[2] - raise));
        }
        if let Some(offset) = offset
            && (raise != 0. || offset != 0.)
        {
            el = el.top(px(offset - raise));
        }
    }
    if style
        .get(Prop::BACKGROUND_IMAGE)
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
            view.clone(),
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
            let view = view.clone();
            // WARNING: the renderer rounds the fitted image bounds, so overflowing fits paint a cached crop of the visible part.
            el = el.child(
                renderer::canvas(
                    |_, _| (),
                    move |bounds, _, window| {
                        let natural = image.size();
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
                            let view = &*view;
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
                                _ => Some(image.bytes()).map(|bytes| {
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
                                    let shown = Arc::new(renderer::RenderImage::new(
                                        pixels.width(),
                                        pixels.height(),
                                        pixels.into_raw(),
                                    ));
                                    if let Some((_, old)) =
                                        state.image_crops.insert(id, (key, shown.clone()))
                                    {
                                        window.drop_image(old);
                                    }
                                    shown
                                }),
                            }
                        };
                        if let Some(shown) = shown {
                            let corners = renderer::Corners {
                                top_left: px(corners[0]),
                                top_right: px(corners[1]),
                                bottom_right: px(corners[2]),
                                bottom_left: px(corners[3]),
                            }
                            .clamp_radii_for_quad_size(visible.size);
                            window.paint_image(visible, visible, corners, shown, false);
                        }
                    },
                )
                .size_full(),
            );
        }
    }
    if let Some(lines) = style
        .get(Prop::WEBKIT_LINE_CLAMP)
        .or_else(|| style.get(Prop::LINE_CLAMP))
        .and_then(|value| value.parse::<usize>().ok())
        .or_else(|| {
            (style
                .get(Prop::TEXT_OVERFLOW)
                .is_some_and(|value| value == "ellipsis")
                && style
                    .get(Prop::WHITE_SPACE)
                    .is_some_and(|value| matches!(value, "nowrap" | "pre"))
                && el.style().overflow.x != renderer::Overflow::Visible)
                .then_some(1)
        })
    {
        let text = el.text_style();
        text.white_space = Some(renderer::WhiteSpace::Normal);
        text.line_clamp = Some(lines.max(1));
        text.text_overflow = Some("…".into());
    }
    if position == "fixed" {
        let viewport = units.viewport;
        let inset = [Prop::LEFT, Prop::RIGHT, Prop::TOP, Prop::BOTTOM].map(|prop| {
            style.get(prop).and_then(|v| {
                units.resolve(
                    v,
                    viewport[usize::from(matches!(prop, Prop::TOP | Prop::BOTTOM))],
                )
            })
        });
        if style.get(Prop::WIDTH).is_none_or(|v| v == "auto")
            && let [Some(left), Some(right), ..] = inset
        {
            el = el.w(px((viewport[0] - left - right).max(0.)));
        }
        if style.get(Prop::HEIGHT).is_none_or(|v| v == "auto")
            && let [_, _, Some(top), Some(bottom)] = inset
        {
            el = el.h(px((viewport[1] - top - bottom).max(0.)));
        }
    }
    if let Some(width) = units
        .resolve(
            style.get(Prop::OUTLINE_WIDTH).unwrap_or("3px"),
            containing[0],
        )
        .filter(|width| {
            *width > 0.
                && style
                    .get(Prop::OUTLINE_STYLE)
                    .is_some_and(|kind| !matches!(kind, "none" | "hidden"))
        })
    {
        let grow = width
            + style
                .get(Prop::OUTLINE_OFFSET)
                .and_then(|v| units.resolve(v, containing[0]))
                .unwrap_or(0.);
        let color = style
            .get(Prop::OUTLINE_COLOR)
            .and_then(style::color)
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
        outline.style().border_style =
            border_style(style.get(Prop::OUTLINE_STYLE).unwrap_or("solid"));
        el = el.child(outline);
    }
    if shaded
        || visible
            .iter()
            .any(|&side| border_colors[side] != border_colors[visible[0]])
    {
        // Each side is painted alone; 3D styles shade it, and groove and ridge split it in two halves.
        let mut bands = vec![([0.; 4], borders, [renderer::transparent_black(); 4])];
        let split = (0..4).any(|side| matches!(border_kinds[side], "groove" | "ridge"));
        if split {
            bands.push(([0.; 4], [0.; 4], [renderer::transparent_black(); 4]));
        }
        for side in 0..4 {
            let color = border_colors[side];
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
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| {
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
                        let corners = renderer::Corners {
                            top_left: shrink(0, 0, 3),
                            top_right: shrink(1, 0, 1),
                            bottom_right: shrink(2, 2, 1),
                            bottom_left: shrink(3, 2, 3),
                        }
                        .clamp_radii_for_quad_size(bounds.size);
                        for side in 0..4 {
                            if widths[side] <= 0. {
                                continue;
                            }
                            let mut edges = [px(0.); 4];
                            edges[side] = px(widths[side]);
                            window.paint_quad(renderer::quad(
                                bounds,
                                corners,
                                renderer::transparent_black(),
                                renderer::Edges {
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
            .get(Prop::ACCENT_COLOR)
            .and_then(style::color)
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
    // WARNING: `overflow: hidden` boxes track a scroll handle only once a script scrolls them.
    let handle = {
        let mut host = view.host.borrow_mut();
        let scroll = &mut host.dom.entry(view.id).or_default().scroll;
        if scrolls.contains(&true) {
            Some(scroll.entry(id).or_default().clone())
        } else if overflow.contains(&renderer::Overflow::Hidden) {
            scroll.get(&id).cloned()
        } else {
            None
        }
    };
    let mut bars = None;
    if let Some(handle) = handle {
        el = el.track_scroll(&handle);
        el.style().restrict_scroll_to_axis = true;
        bars = Some(scrollbar::Scrollbar {
            id,
            handle,
            look,
            axes: scrolls,
            reserved,
            borders,
            origin: [
                padding[3] - borders[1] - borders[3],
                padding[0] - borders[0] - borders[2],
            ],
        });
    }
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
                .entry((parent, true))
                .or_insert_with(|| {
                    let items: Vec<usize> = doc.nodes[parent]
                        .children
                        .iter()
                        .copied()
                        .filter(|&child| {
                            styles.values[child]
                                .get(Prop::DISPLAY)
                                .is_some_and(|value| value == "list-item")
                        })
                        .collect();
                    let index = items.iter().enumerate().map(|(index, &item)| (item, index));
                    let index = index.collect();
                    Rc::new((items, index))
                })
                .clone()
        });
        let position = items
            .as_ref()
            .and_then(|items| items.1.get(&id).copied())
            .unwrap_or(0) as i64;
        let count = items.as_ref().map_or(1, |items| items.0.len()) as i64;
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
        let marker = match kind {
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
            .get(Prop::LIST_STYLE_POSITION)
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
            if let Some((ink, alpha)) = marker.get(Prop::COLOR).and_then(style::color) {
                marker_box = marker_box.text_color(rgba(ink << 8 | alpha as u32));
            }
            if let Some(size) = marker
                .get(Prop::FONT_SIZE)
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
                .get(Prop::FONT_SIZE)
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
            if let Some((ink, alpha)) = pseudo.get(Prop::COLOR).and_then(style::color) {
                decoration = decoration.text_color(rgba(ink << 8 | alpha as u32));
            }
            let absolute = pseudo
                .get(Prop::POSITION)
                .is_some_and(|v| matches!(v, "absolute" | "fixed"));
            if absolute {
                decoration = decoration.absolute();
            } else if pseudo
                .get(Prop::DISPLAY)
                .is_some_and(|value| !value.starts_with("inline"))
                && (all_inline || display == "inline")
            {
                decoration = decoration.w_full();
            }
            let pseudo_padding = pseudo_units
                .edges(pseudo, style::PADDING, containing[0])
                .map(|v| v.max(0.));
            let pseudo_margin = pseudo_units.edges(pseudo, style::MARGIN, containing[0]);
            let pseudo_borders = sides.map(|(width, kind, _)| {
                if pseudo
                    .get(kind)
                    .is_none_or(|kind| matches!(kind, "none" | "hidden"))
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
                        .and_then(style::color)
                        .map_or(ink, |(ink, alpha)| rgba(ink << 8 | alpha as u32)),
                );
            }
            let mut pseudo_radius = [0.; 4];
            let mut pseudo_shadows = Vec::new();
            for (prop, value) in pseudo.iter() {
                let value: &str = value;
                let vertical = matches!(
                    prop,
                    Prop::HEIGHT | Prop::MIN_HEIGHT | Prop::MAX_HEIGHT | Prop::TOP | Prop::BOTTOM
                );
                let base = containing[usize::from(vertical)];
                let length = || -> Option<renderer::Length> {
                    match value
                        .strip_suffix('%')
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                    {
                        Some(percent) => Some(relative(percent / 100.)),
                        None => pseudo_units
                            .resolve(value, base)
                            .map(|value| px(value).into()),
                    }
                };
                match prop {
                    Prop::WIDTH
                    | Prop::HEIGHT
                    | Prop::MIN_WIDTH
                    | Prop::MAX_WIDTH
                    | Prop::MIN_HEIGHT
                    | Prop::MAX_HEIGHT
                    | Prop::LEFT
                    | Prop::TOP
                    | Prop::RIGHT
                    | Prop::BOTTOM => {
                        let Some(length) = length() else { continue };
                        decoration = match prop {
                            Prop::WIDTH => decoration.w(length),
                            Prop::HEIGHT => decoration.h(length),
                            Prop::MIN_WIDTH => decoration.min_w(length),
                            Prop::MAX_WIDTH => decoration.max_w(length),
                            Prop::MIN_HEIGHT => decoration.min_h(length),
                            Prop::MAX_HEIGHT => decoration.max_h(length),
                            Prop::LEFT if absolute => decoration.left(length),
                            Prop::TOP if absolute => decoration.top(length),
                            Prop::RIGHT if absolute => decoration.right(length),
                            Prop::BOTTOM if absolute => decoration.bottom(length),
                            _ => decoration,
                        };
                    }
                    Prop::BORDER_TOP_LEFT_RADIUS
                    | Prop::BORDER_TOP_RIGHT_RADIUS
                    | Prop::BORDER_BOTTOM_RIGHT_RADIUS
                    | Prop::BORDER_BOTTOM_LEFT_RADIUS => {
                        let Some(length) = pseudo_units
                            .resolve(value, containing[0])
                            .map(|v| px(v.max(0.)))
                        else {
                            continue;
                        };
                        let corner = match prop {
                            Prop::BORDER_TOP_LEFT_RADIUS => 0,
                            Prop::BORDER_TOP_RIGHT_RADIUS => 1,
                            Prop::BORDER_BOTTOM_RIGHT_RADIUS => 2,
                            _ => 3,
                        };
                        pseudo_radius[corner] = f32::from(length);
                        decoration = match prop {
                            Prop::BORDER_TOP_LEFT_RADIUS => decoration.rounded_tl(length),
                            Prop::BORDER_TOP_RIGHT_RADIUS => decoration.rounded_tr(length),
                            Prop::BORDER_BOTTOM_RIGHT_RADIUS => decoration.rounded_br(length),
                            _ => decoration.rounded_bl(length),
                        };
                    }
                    Prop::BACKGROUND_COLOR => {
                        if let Some((color, alpha)) = style::color(value) {
                            decoration = decoration.bg(rgba(color << 8 | alpha as u32));
                        }
                    }
                    Prop::OPACITY => {
                        if let Ok(opacity) = value.parse::<f32>() {
                            decoration = decoration.opacity(opacity.clamp(0., 1.));
                        }
                    }
                    Prop::BOX_SHADOW => {
                        decoration.style().box_shadow = pseudo_units.shadows(value, false);
                        pseudo_shadows = pseudo_units.shadows(value, true);
                    }
                    _ => {}
                }
            }
            if pseudo
                .get(Prop::BACKGROUND_IMAGE)
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
                    view.clone(),
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
                .get(Prop::Z_INDEX)
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            let decoration = if order != 0
                && pseudo
                    .get(Prop::POSITION)
                    .is_some_and(|value| value != "static")
            {
                Stacked {
                    inner: Some(decoration.into_any_element()),
                    layers: build.layers.clone(),
                    order,
                    path: build.ancestors[build.stacking..].into(),
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
        .get(Prop::WHITE_SPACE)
        .is_some_and(|v| matches!(v, "pre" | "pre-wrap" | "break-spaces"))
        || node.tag == "pre";
    if node.tag == "#text"
        && style
            .get(Prop::WHITE_SPACE)
            .is_none_or(|value| !matches!(value, "pre" | "nowrap"))
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
    let drag = style
        .get(Prop::APP_REGION)
        .is_some_and(|value| value == "drag");
    if (hit_test || node.tag == "#text")
        && !drag
        && styles.user_select[id].0 != style::UserSelect::None
        && style.get(Prop::CURSOR).is_none_or(|value| value == "auto")
    {
        el = el.cursor(renderer::CursorStyle::IBeam);
    }
    if let Some(data) = inline {
        if style
            .get(Prop::CONTAINER_TYPE)
            .is_some_and(|v| matches!(v, "inline-size" | "size"))
        {
            let size = [0, 1].map(|axis| {
                (style
                    .get([Prop::WIDTH, Prop::HEIGHT][axis])
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
        let inner = renderer::StyledText::new(data.text.to_string()).with_runs(data.runs.clone());
        el = el.child(
            div()
                .min_w(px(0.))
                .max_w(relative(1.))
                .flex_shrink(1.)
                .child(InlineText {
                    inner,
                    data,
                    view: view.clone(),
                    runs: None,
                    measured: None,
                    justify: style
                        .get(Prop::TEXT_ALIGN)
                        .is_some_and(|value| value == "justify")
                        .then(|| {
                            style
                                .get(Prop::TEXT_ALIGN_LAST)
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
        for (axis, (prop, min, max)) in [
            (Prop::WIDTH, Prop::MIN_WIDTH, Prop::MAX_WIDTH),
            (Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(value) = style
                .get(prop)
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
            .get(Prop::CONTAINER_TYPE)
            .is_some_and(|v| matches!(v, "inline-size" | "size"))
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
        // A flexed grid gets its height from the flex layout, which runs after this. Its
        // tracks use the height from the last layout, and prepaint renders again on change.
        let flexed = style
            .get(Prop::DISPLAY)
            .is_some_and(|v| v.ends_with("grid"))
            && slot.is_none()
            && style.get(Prop::HEIGHT).is_none_or(|v| v == "auto")
            && node.parent.is_some_and(|parent| {
                let parent = &styles.values[parent];
                parent
                    .get(Prop::DISPLAY)
                    .is_some_and(|v| v.ends_with("flex"))
                    && parent
                        .get(Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v.starts_with("column"))
            })
            && (style
                .get(Prop::FLEX_GROW)
                .and_then(|v| v.parse::<f32>().ok())
                .is_some_and(|v| v > 0.)
                || style
                    .get(Prop::FLEX_BASIS)
                    .is_some_and(|v| v != "auto" && v != "content"));
        // An auto width grid in a box sized by its content, like a row flex item, gets its
        // width from the flex layout. Its tracks use the width from the last layout.
        let shrunk = style.get(Prop::DISPLAY).is_some_and(|v| v == "grid")
            && slot.is_none()
            && style.get(Prop::WIDTH).is_none_or(|v| v == "auto")
            && {
                let mut child = id;
                loop {
                    let Some(parent) = doc.nodes[child].parent else {
                        break false;
                    };
                    let parent = &styles.values[parent];
                    let display = parent.get(Prop::DISPLAY).unwrap_or("block");
                    if display.ends_with("flex") {
                        break parent
                            .get(Prop::FLEX_DIRECTION)
                            .is_none_or(|v| v.starts_with("row"));
                    }
                    if display != "block" || parent.get(Prop::WIDTH).is_some_and(|v| v != "auto") {
                        break false;
                    }
                    child = doc.nodes[child].parent.unwrap_or(child);
                }
            };
        let mut widest = None;
        let grid = if let Some(display) = style
            .get(Prop::DISPLAY)
            .filter(|v| matches!(*v, "grid" | "inline-grid"))
        {
            if display == "inline-grid"
                && slot.is_none()
                && style.get(Prop::WIDTH).is_none_or(|v| v == "auto")
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
            let horizontal = padding[1] + padding[3] + borders[1] + borders[3];
            let vertical = padding[0] + padding[2] + borders[0] + borders[2];
            let mut tracked = [None; 2];
            if shrunk {
                let [_, max, ..] = super::grid::intrinsic(
                    doc,
                    id,
                    styles,
                    None,
                    None,
                    text,
                    &mut measured.borrow_mut(),
                );
                let max = (max - horizontal - margin[1] - margin[3]).max(0.);
                widest = Some(max);
                let last = view.host.borrow().dom.get(&view.id).and_then(|state| {
                    state
                        .bounds
                        .get(&id)
                        .map(|bounds| (bounds[2] - horizontal).max(0.))
                });
                // Laid-out bounds snap to device pixels, so a fitting grid may look narrower.
                content[0] = last.map_or(content[0].min(max), |last| {
                    if last + 0.5 >= max {
                        last.max(max)
                    } else {
                        last
                    }
                });
                tracked[0] = Some((last.unwrap_or(f32::NAN), horizontal));
            }
            let height = if slot.is_some() || style.get(Prop::HEIGHT).is_some_and(|v| v != "auto") {
                Some(content[1])
            } else if flexed {
                let height = view.host.borrow().dom.get(&view.id).and_then(|state| {
                    state
                        .bounds
                        .get(&id)
                        .map(|bounds| (bounds[3] - vertical).max(0.))
                });
                tracked[1] = Some((height.unwrap_or(f32::NAN), vertical));
                height
            } else {
                None
            };
            if tracked.iter().any(Option::is_some) {
                let mut host = view.host.borrow_mut();
                host.dom
                    .entry(view.id)
                    .or_default()
                    .flexed_grids
                    .insert(id, tracked);
            }
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
            el = el.flex().flex_col();
            // A shrunk grid stretches; a spacer gives it its max-content width.
            el = match widest {
                Some(widest) => el.child(div().w(px(widest)).h(px(0.)).flex_shrink_0()),
                None => el.w(px(content[0]
                    + padding[1]
                    + padding[3]
                    + borders[1]
                    + borders[3])),
            };
            if style.get(Prop::HEIGHT).is_none_or(|v| v == "auto") && !flexed {
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
                    .get(Prop::Z_INDEX)
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            });
        } else if flexbox {
            children.sort_by_key(|child| {
                styles.values[*child]
                    .get(Prop::ORDER)
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
            });
        }
        // Whitespace between inline content, text included, is one space.
        let inline_element = |child: Option<&usize>| {
            child.is_some_and(|&child| {
                let node = &doc.nodes[child];
                node.tag == "#text" && !node.text.trim().is_empty()
                    || !node.tag.starts_with('#') && node.tag != "br" && inline_level(child)
            })
        };
        let multicol = columns(style, &units, content[0]);
        let mut line: Option<renderer::Div> = None;
        let mut floating = Vec::new();
        // Floats and the content after them, until a `clear`, share a row: left floats, the
        // content, then right floats. Text does not flow under a float that ends early.
        let mut group: Option<(Vec<AnyElement>, Vec<AnyElement>, Vec<AnyElement>)> = None;
        // With multiple columns, block content is collected and split into columns at the end.
        let columns = multicol.filter(|_| flow && grid.is_none() && !all_inline);
        let mut collected: Vec<(Option<usize>, AnyElement)> = Vec::new();
        let shown = |index: usize| {
            let child = &doc.nodes[children[index]];
            preserve
                || !(child.tag == "#text" && child.text.trim().is_empty())
                || flow
                    && grid.is_none()
                    && inline_element(index.checked_sub(1).and_then(|index| children.get(index)))
                    && inline_element(children.get(index + 1))
        };
        // WARNING: wrapped lines are culled whole, and only when the container's height is its content's, so `align-content` has no space to share.
        let wrap = display == "flex"
            && style.get(Prop::FLEX_DIRECTION).is_none_or(|v| v == "row")
            && style.get(Prop::FLEX_WRAP).is_some_and(|v| v == "wrap")
            && style
                .get(Prop::ALIGN_CONTENT)
                .is_none_or(|v| matches!(v, "normal" | "stretch" | "flex-start" | "start"))
            && style
                .get(Prop::ALIGN_ITEMS)
                .is_none_or(|v| !v.contains("baseline"))
            && [Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT]
                .into_iter()
                .all(|name| {
                    style
                        .get(name)
                        .is_none_or(|v| matches!(v, "auto" | "none" | "0" | "0px"))
                })
            && !flex_parent
            && slot.is_none()
            && info[id].flags & cull::RELISTED == 0;
        // Offscreen children that did not change since their last layout become spacers.
        let stack = build.zone.clone().filter(|_| {
            grid.is_none()
                && columns.is_none()
                && !all_inline
                && display != "inline"
                && if flexbox {
                    wrap || style
                        .get(Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v == "column")
                        && style.get(Prop::FLEX_WRAP).is_none_or(|v| v == "nowrap")
                        && style.get(Prop::JUSTIFY_CONTENT).is_none_or(|v| {
                            matches!(v, "normal" | "flex-start" | "start" | "stretch")
                        })
                } else {
                    flow && !children.iter().any(|&child| floated(child))
                }
        });
        let mut plan = None;
        if let Some(zone) = stack {
            let host = view.host.borrow();
            if let Some(dom) = host.dom.get(&view.id)
                && let Some(&[left, ..]) = dom.bounds.get(&id)
                && let Some(&width) = dom.cull.widths.get(&id)
            {
                let gap = if flexbox {
                    style
                        .get(Prop::ROW_GAP)
                        .and_then(|v| units.resolve(v, containing[0]))
                        .unwrap_or(0.)
                        .max(0.)
                } else {
                    0.
                };
                let (slots, mut runs) = if wrap {
                    cull::lines(
                        &children,
                        |index| shown(index) && info[children[index]].flags & cull::OUT == 0,
                        |child| {
                            let placed = *dom.cull.placed.get(child)?;
                            let bounds = dom.bounds.get(&child)?;
                            let [top, _, bottom, left] = placed.2;
                            let member = cull::member(
                                info[child],
                                placed,
                                Some(bounds),
                                &zone,
                                units.viewport[1],
                                false,
                            );
                            Some((
                                member.is_some(),
                                [
                                    bounds[0] - left,
                                    bounds[1] - top,
                                    bounds[0] - left + placed.1[0],
                                    bounds[1] + bounds[3] + bottom,
                                ],
                            ))
                        },
                        gap,
                    )
                } else {
                    cull::plan(
                        &children,
                        shown,
                        |child| {
                            cull::member(
                                info[child],
                                dom.cull.placed.get(child).copied().unwrap_or_default(),
                                dom.bounds.get(&child),
                                &zone,
                                units.viewport[1],
                                flexbox,
                            )
                        },
                        gap,
                    )
                };
                if wrap {
                    let mut watch = Vec::new();
                    for (index, &child) in children.iter().enumerate() {
                        match slots[index] {
                            0 => {
                                let placed =
                                    dom.cull.placed.get(child).copied().unwrap_or_default();
                                if placed.0 != info[child].revision {
                                    watch.push((child, placed.1[0]));
                                }
                            }
                            slot => {
                                if let Some(run) = runs[slot - 1]
                                    .as_mut()
                                    .filter(|run| run.members[0] == child)
                                {
                                    run.width = width;
                                    run.watch = std::mem::take(&mut watch);
                                }
                            }
                        }
                    }
                }
                // WARNING: the first and last box of a run collapse their margins with siblings outside it, which may have changed.
                if !flexbox {
                    let collapsed = |child: usize, side: usize| {
                        let mut units = styles.lengths[child];
                        units.container = Some(container);
                        margins(build, child, &units, content[0], false, false)[side]
                    };
                    for run in runs.iter_mut().flatten() {
                        run.margins = [
                            collapsed(run.members[0], 0),
                            collapsed(run.members[run.members.len() - 1], 2),
                        ];
                    }
                }
                plan = (!runs.is_empty()).then_some((slots, runs, [left, width], zone));
            }
        }
        // Offscreen grid items that did not change and keep their cell size are not built.
        let mut culled = Vec::new();
        if let (Some(grid), Some(zone)) = (&grid, &build.zone) {
            if let Some(dom) = view.host.borrow().dom.get(&view.id) {
                for &(child, _, [x, y, width, height]) in &grid.cells {
                    let placed = dom.cull.placed.get(child).copied().unwrap_or_default();
                    let [top, right, bottom, left] = placed.2;
                    // WARNING: the cell sizes come before layout snaps boxes to pixels, so they compare with the unsnapped cell sizes of the last layout.
                    if placed.1 == [width, height]
                        && cull::member(
                            info[child],
                            placed,
                            dom.bounds.get(&child),
                            zone,
                            units.viewport[1],
                            false,
                        )
                        .is_some()
                    {
                        // WARNING: layout rounds each inset and margin to device pixels, half toward zero, so the offsets from the anchor do too.
                        let snap = |value: f32| {
                            let value = value * pixel_ratio;
                            (value.abs() - 0.5).ceil().copysign(value) / pixel_ratio
                        };
                        culled.push((
                            child,
                            [
                                snap(x + padding[3]) + snap(left),
                                snap(y + padding[0]) + snap(top),
                                width - left - right,
                                height - top - bottom,
                            ],
                        ));
                    }
                }
            }
            if !culled.is_empty() {
                el = el.child(cull::anchor(culled.clone(), zone.clone(), view.clone()));
            }
        }
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
            if shown(index) {
                if let Some((slots, runs, then, zone)) = plan.as_mut()
                    && let Some(&slot) = slots.get(index).filter(|&&slot| slot > 0)
                {
                    if let Some(run) = runs[slot - 1].take() {
                        if let Some(open) = line.take() {
                            place!(open, None);
                        }
                        place!(
                            cull::spacer(run, id, *then, zone.clone(), view.clone()),
                            None
                        );
                    }
                    continue;
                }
                if culled.binary_search_by_key(&child, |&(id, _)| id).is_ok() {
                    continue;
                }
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
                let child_element = element(build, child, content, container, placement);
                let out_of_flow = styles.values[child]
                    .get(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"));
                let child_style = &styles.values[child];
                if grid.is_none() && !out_of_flow {
                    let clear = child_style.get(Prop::CLEAR).unwrap_or("none");
                    if group.is_some() && clear != "none" {
                        close_group!();
                    }
                    if floated(child) {
                        if let Some(open) = line.take() {
                            place!(open, None);
                        }
                        let open = group.get_or_insert_with(Default::default);
                        // A float without a width shrinks to fit, but no wider than the row.
                        let float = if child_style
                            .get(Prop::WIDTH)
                            .is_none_or(|value| value == "auto")
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
                        .get(Prop::POSITION)
                        .is_some_and(|value| value == "absolute")
                    && child_style
                        .get(Prop::DISPLAY)
                        .is_none_or(|value| value != "none")
                    && [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]]
                        .iter()
                        .any(|axis| {
                            axis.iter().all(|prop| {
                                child_style.get(*prop).is_none_or(|value| value == "auto")
                            })
                        })
                {
                    // WARNING: the placeholder must come before the box, which reads its bounds in prepaint.
                    let block = child_style
                        .get(Prop::DISPLAY)
                        .is_some_and(|value| !value.starts_with("inline"));
                    let entity = view.clone();
                    let mut placeholder = renderer::canvas(
                        move |bounds, _| {
                            let view = &*entity;
                            view.host
                                .borrow_mut()
                                .dom
                                .entry(view.id)
                                .or_default()
                                .anchors
                                .insert(child, bounds);
                        },
                        |_, _, _| {},
                    )
                    .w(px(0.))
                    .flex_shrink_0()
                    .h(px(
                        if block
                            && (line.is_some()
                                || all_inline
                                    && flat
                                        .iter()
                                        .any(|&other| other != child && in_flow(doc, info, other)))
                        {
                            units.font.lh
                        } else {
                            0.
                        },
                    ));
                    placeholder.style().align_self = Some(renderer::AlignSelf::FlexStart);
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
        let align = match style.get(Prop::VERTICAL_ALIGN) {
            Some("middle") => Some(renderer::AlignContent::Center),
            Some("bottom") => Some(renderer::AlignContent::FlexEnd),
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
    // WARNING: Windows moves the window in the native hit test (`window_control_area`), macOS and Linux on the first drag.
    if drag {
        el = el
            .window_control_area(renderer::WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, {
                let view = view.clone();
                move |event: &renderer::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    if event.click_count != 2 {
                        view.moving.set(true);
                    } else if cfg!(target_os = "macos") {
                        window.titlebar_double_click();
                    } else {
                        window.zoom_window();
                    }
                }
            });
    } else if style
        .get(Prop::POINTER_EVENTS)
        .is_none_or(|value| value != "none")
    {
        let view = view.clone();
        el = el.on_mouse_down(MouseButton::Left, move |event: &renderer::MouseDownEvent, _, cx| {
            cx.stop_propagation();
            Desktop::dispatch(view.runtime.clone(), view.id, "pointerdown", if hit_test { &[] } else { std::slice::from_ref(&id) }, serde_json::json!({"pointerId":1,"pointerType":"mouse","isPrimary":true,"clientX":f32::from(event.position.x),"clientY":f32::from(event.position.y),"button":0,"buttons":1,"detail":event.click_count,"ctrlKey":event.modifiers.control,"metaKey":event.modifiers.platform,"altKey":event.modifiers.alt,"shiftKey":event.modifiers.shift}), cx);
        });
    }
    let mut parent = node.parent;
    while let Some(ancestor) = parent
        && styles.values[ancestor]
            .get(Prop::DISPLAY)
            .is_some_and(|value| value == "contents")
    {
        parent = doc.nodes[ancestor].parent;
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
                    style::PADDING,
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
    el = el.children(bars);
    let el = if drag
        || style
            .get(Prop::POINTER_EVENTS)
            .is_some_and(|value| value == "none")
    {
        el
    } else {
        let view = view.clone();
        el.on_click(move |event: &renderer::ClickEvent, window, cx| {
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
                                if hit_test { String::from("[]") } else { format!("[{id}]") },
                                serde_json::json!({"clientX": f32::from(event.position().x), "clientY": f32::from(event.position().y), "detail":event.click_count(), "ctrlKey":event.modifiers().control,"metaKey":event.modifiers().platform,"altKey":event.modifiers().alt,"shiftKey":event.modifiers().shift})
                            ),
                            "apps:click",
                        )
                    })
            };
            if let Some(Err(error)) = result {
                eprintln!("apps: {error}");
            }
            window.refresh();
            cx.defer(move |cx| Desktop::drive(runtime, cx));
        })
    };
    let sticky = (style.get(Prop::POSITION).is_some_and(|v| v == "sticky"))
        .then(|| {
            let host = view.host.borrow();
            ancestors.iter().rev().copied().find_map(|parent| {
                let scroll = host.dom.get(&view.id)?.scroll.get(&parent)?.clone();
                let style = &styles.values[parent];
                let border = [
                    Prop::BORDER_TOP_WIDTH,
                    Prop::BORDER_RIGHT_WIDTH,
                    Prop::BORDER_BOTTOM_WIDTH,
                    Prop::BORDER_LEFT_WIDTH,
                ]
                .map(|prop| {
                    style
                        .get(prop)
                        .or_else(|| style.get(Prop::BORDER_WIDTH))
                        .and_then(|v| styles.lengths[parent].resolve(v, containing[0]))
                        .unwrap_or(0.)
                        .max(0.)
                });
                Some((scroll, border, node.parent))
            })
        })
        .flatten();
    // Vertical writing modes lay the box out horizontally at its inline size, then turn it.
    let vertical = style
        .get(Prop::WRITING_MODE)
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
                .get(Prop::HEIGHT)
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
        revision: info[id].revision,
        sides: padding[1] + padding[3] + borders[1] + borders[3],
        margin,
        cell: slot.as_ref().map(|(bounds, _)| [bounds[2], bounds[3]]),
        id,
        ids: ids.clone(),
        view: view.clone(),
        sticky,
        fixed: position == "fixed",
        inset: [Prop::TOP, Prop::RIGHT, Prop::BOTTOM, Prop::LEFT].map(|prop| {
            let axis = usize::from(matches!(prop, Prop::TOP | Prop::BOTTOM));
            style.get(prop).and_then(|v| {
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
            path: ancestors[stack..].into(),
        }
        .into_any_element()
    } else {
        element
    }
}

/// Whether a child takes part in its parent's flow: not a comment, blank text, hidden or out of flow.
fn in_flow(doc: &Document, info: &[cull::Info], id: usize) -> bool {
    let node = &doc.nodes[id];
    node.tag != "#comment"
        && !(node.tag == "#text" && node.text.trim().is_empty())
        && info[id].flags & cull::OUT == 0
}

/// The margins of a box. Vertical margins collapse with block siblings, the first and the
/// last child, and the parent, like block layout.
// WARNING: culled spacers take the margins of their first and last box from here, so these must stay the margins the box is laid out with.
pub(super) fn margins(
    build: &Build<'_>,
    id: usize,
    units: &style::Lengths,
    width: f32,
    flex_parent: bool,
    slot: bool,
) -> [f32; 4] {
    let (doc, styles, node, style) = (
        build.doc,
        build.styles,
        &build.doc.nodes[id],
        &build.styles.values[id],
    );
    let mut margin = units.edges(style, style::MARGIN, width);
    let edge = |id: usize, prop: Prop| {
        styles.values[id]
            .get(prop)
            .and_then(|value| styles.lengths[id].resolve(value, width))
            .unwrap_or(0.)
    };
    let in_flow = |id: usize| in_flow(doc, build.info, id);
    let block = |id: usize| {
        !doc.nodes[id].tag.starts_with('#')
            && styles.values[id]
                .get(Prop::DISPLAY)
                .is_some_and(|value| !value.starts_with("inline") && value != "contents")
    };
    // WARNING: margins only collapse through plain block boxes; flex/grid items and scroll containers keep their own.
    let collapses = |id: usize, side: usize| {
        let style = &styles.values[id];
        let [padding, border, kind] = if side == 0 {
            [
                Prop::PADDING_TOP,
                Prop::BORDER_TOP_WIDTH,
                Prop::BORDER_TOP_STYLE,
            ]
        } else {
            [
                Prop::PADDING_BOTTOM,
                Prop::BORDER_BOTTOM_WIDTH,
                Prop::BORDER_BOTTOM_STYLE,
            ]
        };
        id != 0
            && doc.nodes[id].tag != "html"
            && doc.nodes[id].parent.is_none_or(|parent| {
                !styles.values[parent]
                    .get(Prop::DISPLAY)
                    .is_some_and(|value| {
                        matches!(value, "flex" | "inline-flex" | "grid" | "inline-grid")
                    })
            })
            && style
                .get(Prop::DISPLAY)
                .is_some_and(|value| matches!(value, "block" | "list-item"))
            && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                .into_iter()
                .all(|key| style.get(key).is_none_or(|value| value == "visible"))
            && style
                .get(Prop::POSITION)
                .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
            && edge(id, padding) == 0.
            && (edge(id, border) == 0.
                || style
                    .get(kind)
                    .is_none_or(|value| matches!(value, "none" | "hidden")))
            && (side == 0 || style.get(Prop::HEIGHT).is_none_or(|value| value == "auto"))
    };
    if block(id)
        && in_flow(id)
        && !slot
        && !flex_parent
        && let Some(parent) = node.parent
    {
        let siblings = build
            .siblings
            .borrow_mut()
            .entry((parent, false))
            .or_insert_with(|| {
                let list: Vec<usize> = doc.nodes[parent]
                    .children
                    .iter()
                    .copied()
                    .filter(|&child| in_flow(child))
                    .collect();
                let index = list
                    .iter()
                    .enumerate()
                    .map(|(index, &child)| (child, index));
                let index = index.collect();
                Rc::new((list, index))
            })
            .clone();
        let index = siblings.1.get(&id).copied();
        let siblings = &siblings.0;
        if let Some(previous) = index
            .and_then(|index| index.checked_sub(1))
            .map(|index| siblings[index])
            .filter(|&previous| block(previous))
        {
            let (a, b) = (margin[0], edge(previous, Prop::MARGIN_BOTTOM));
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
            && edge(parent, Prop::MARGIN_TOP) >= 0.
            && collapses(parent, 0)
        {
            margin[0] = 0.;
        }
        if index.is_some_and(|index| index + 1 == siblings.len())
            && margin[2] >= 0.
            && edge(parent, Prop::MARGIN_BOTTOM) >= 0.
            && collapses(parent, 2)
        {
            margin[2] = 0.;
        }
    }
    if let Some(first) = node
        .children
        .iter()
        .copied()
        .find(|&child| in_flow(child))
        .filter(|&first| block(first) && margin[0] >= 0. && collapses(id, 0))
    {
        margin[0] = margin[0].max(edge(first, Prop::MARGIN_TOP));
    }
    if let Some(last) = node
        .children
        .iter()
        .copied()
        .rfind(|&child| in_flow(child))
        .filter(|&last| block(last) && margin[2] >= 0. && collapses(id, 2))
    {
        margin[2] = margin[2].max(edge(last, Prop::MARGIN_BOTTOM));
    }
    margin
}

/// `Some(true)` for a box floated right, `Some(false)` for left.
fn float_side(style: &style::Style) -> Option<bool> {
    match style.get(Prop::FLOAT) {
        Some("left" | "inline-start") => Some(false),
        Some("right" | "inline-end") => Some(true),
        _ => None,
    }
}

/// The width, color and style of a column rule.
type ColumnRule = (f32, renderer::Rgba, renderer::BorderStyle);

/// The column count, gap and rule of a multi-column box with the given content width.
fn columns(
    style: &style::Style,
    units: &style::Lengths,
    width: f32,
) -> Option<(usize, f32, Option<ColumnRule>)> {
    let mut count = style
        .get(Prop::COLUMN_COUNT)
        .and_then(|value| value.parse::<usize>().ok());
    let mut column_width = style
        .get(Prop::COLUMN_WIDTH)
        .and_then(|value| units.resolve(value, width));
    for part in style::components(style.get(Prop::COLUMNS).unwrap_or(""), ' ') {
        if let Ok(value) = part.parse::<usize>() {
            count = count.or(Some(value));
        } else if let Some(value) = units.resolve(part, width) {
            column_width = column_width.or(Some(value));
        }
    }
    let gap = style
        .get(Prop::COLUMN_GAP)
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
        style.get(Prop::COLUMN_RULE_WIDTH),
        style.get(Prop::COLUMN_RULE_STYLE),
        style.get(Prop::COLUMN_RULE_COLOR),
    );
    for part in style::components(style.get(Prop::COLUMN_RULE).unwrap_or(""), ' ') {
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
                .or_else(|| style.get(Prop::COLOR))
                .and_then(style::color)
                .map_or(renderer::black(), |(ink, alpha)| {
                    rgba(ink << 8 | alpha as u32)
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
    color: renderer::Rgba,
    kind: renderer::BorderStyle,
) -> AnyElement {
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            let column = (f32::from(bounds.size.width) - gap * (count - 1) as f32) / count as f32;
            for index in 1..count {
                let x = bounds.left() + px(index as f32 * (column + gap) - gap / 2. - width / 2.);
                window.paint_quad(renderer::quad(
                    Bounds::new(point(x, bounds.top()), size(px(width), bounds.size.height)),
                    renderer::Corners::default(),
                    renderer::transparent_black(),
                    renderer::Edges {
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

fn border_style(kind: &str) -> renderer::BorderStyle {
    match kind {
        "dashed" => renderer::BorderStyle::Dashed,
        "dotted" => renderer::BorderStyle::Dotted,
        "double" => renderer::BorderStyle::Double,
        _ => renderer::BorderStyle::Solid,
    }
}

/// The darker or lighter shade of a 3D border, like WebKit.
fn shade_border(color: renderer::Rgba, dark: bool) -> renderer::Rgba {
    let rgba = color;
    let value = rgba.r.max(rgba.g).max(rgba.b);
    let factor = if value <= 0. {
        return if dark {
            color
        } else {
            renderer::Rgba {
                r: 0.33,
                g: 0.33,
                b: 0.33,
                a: rgba.a,
            }
        };
    } else if dark {
        ((value - 0.33) / value).max(0.)
    } else {
        (value + 0.33).min(1.) / value
    };
    renderer::Rgba {
        r: (rgba.r * factor).min(1.),
        g: (rgba.g * factor).min(1.),
        b: (rgba.b * factor).min(1.),
        a: rgba.a,
    }
}

fn inset_shadows(
    shadows: Vec<renderer::BoxShadow>,
    radius: [f32; 4],
    borders: [f32; 4],
) -> AnyElement {
    let inner =
        |corner: usize, a: usize, b: usize| (radius[corner] - borders[a].max(borders[b])).max(0.);
    let radii = super::effects::corners([
        inner(0, 0, 3),
        inner(1, 0, 1),
        inner(2, 2, 1),
        inner(3, 2, 3),
    ]);
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| window.paint_inset_shadows(bounds, radii, &shadows),
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// Paints the inline `<svg>` or the SVG `<img>` source of a node, rasterized at the box size.
fn svg_layer(id: usize, inline: bool, view: Rc<super::ViewData>) -> AnyElement {
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            let [width, height] = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            if width <= 0. || height <= 0. {
                return;
            }
            let scale = window.scale_factor();
            let raster =
                [width, height].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
            let view = &*view;
            let mut host = view.host.borrow_mut();
            let state = host.dom.entry(view.id).or_default();
            let source = if inline {
                state
                    .svg_sources
                    .get(&id)
                    .map(|(_, _, source)| source.as_str())
            } else {
                state.svg_images.get(&id).map(|(source, _)| &**source)
            };
            let Some(source) = source else { return };
            if state.svgs.get(&id).is_none_or(|(old, image)| {
                !inline && old != source
                    || u32::from(image.size().width) != raster[0]
                    || u32::from(image.size().height) != raster[1]
            }) {
                match super::canvas::svg(source, raster[0], raster[1], None) {
                    Ok(image) => {
                        if let Some((_, old)) = state.svgs.insert(id, (source.to_owned(), image)) {
                            window.drop_image(old);
                        }
                    }
                    Err(error) => eprintln!("apps: SVG: {error}"),
                }
            }
            let image = state.svgs.get(&id).map(|(_, image)| image.clone());
            drop(host);
            if let Some(image) = image {
                window.paint_image(bounds, bounds, renderer::Corners::default(), image, false);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}
