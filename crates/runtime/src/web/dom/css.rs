use super::display;
use crate::css;
use crate::css::properties::Prop;
use crate::dom::Document;
use crate::render::PageState;
use crate::style;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::rc::Rc;

// A path with a bad item is empty, so the edit fails with IndexSizeError instead of reaching another rule.
fn path(kind: &str, value: &Value) -> (Vec<usize>, Option<(usize, bool)>) {
    let path: Vec<usize> = value
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .map(|item| Some(item.as_u64()? as usize))
                .collect()
        })
        .unwrap_or_default();
    let top = (matches!(kind, "insert" | "delete") && path.len() == 1)
        .then(|| (path[0], kind == "insert"));
    (path, top)
}

fn edit<'a>(kind: &str, text: &'a str) -> css::sheet::Edit<'a> {
    match kind {
        "insert" => css::sheet::Edit::Insert(text),
        "style" => css::sheet::Edit::Declarations(text),
        "selector" => css::sheet::Edit::Selector(text),
        "keyframe" => css::sheet::Edit::Keyframe(text),
        "replace" => css::sheet::Edit::Replace(text),
        _ => css::sheet::Edit::Delete,
    }
}

pub(super) fn css_control(
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let time = args[2].as_f64().unwrap_or(0.);
    if !time.is_finite() {
        return Ok(json!(false));
    }
    let styles = state.restyle(doc, viewport);
    state.dirty = true;
    Ok(json!(state.styles.motion.css_control(
        args[0].as_u64().unwrap_or(0) as u32,
        args[1].as_str().unwrap_or_default(),
        time,
        &styles,
    )))
}

pub(super) fn easing(args: &Value) -> Result<Value> {
    let progress = args[1].as_f64().unwrap_or(0.);
    Ok(json!(
        css::value::easing::Easing::parse(args[0].as_str().unwrap_or("linear"))
            .map_or(progress, |easing| easing.at(progress, args[2] == true))
    ))
}

pub(super) fn animation(args: &Value, doc: &mut Document, state: &mut PageState) -> Result<Value> {
    let id = args[0].as_u64().context("Expected an animation")? as u32;
    state.dirty = true;
    let old = state.styles.motion.unscript(id);
    if let Some((node, _)) = &old {
        state.styles.animating.remove(node);
        state.styles.animating_pseudos.remove(node);
    }
    let spec = &args[2];
    let Some(node) = args[1]
        .as_u64()
        .map(|node| node as usize)
        .filter(|&node| spec.is_object() && node < doc.nodes.len())
    else {
        return Ok(Value::Null);
    };
    state.styles.animating.remove(&node);
    state.styles.animating_pseudos.remove(&node);
    let frames: Vec<(f64, &str, &str)> = spec["frames"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|frame| {
            Some((
                frame[0].as_f64()?,
                frame[1].as_str()?,
                frame[2].as_str().unwrap_or("linear"),
            ))
        })
        .collect();
    let number = |index: usize| spec["timing"][index].as_f64().unwrap_or(0.);
    let iterations = number(3);
    let animation = style::animation::timing::Scripted::new(
        id,
        &frames,
        [
            number(0) / 1000.,
            number(1) / 1000.,
            number(2) / 1000.,
            if iterations < 0. {
                f64::INFINITY
            } else {
                iterations
            },
            number(4),
        ],
        [
            spec["direction"].as_str().unwrap_or("normal"),
            spec["fill"].as_str().unwrap_or("none"),
            spec["easing"].as_str().unwrap_or("linear"),
            spec["composite"].as_str().unwrap_or("replace"),
        ],
        match spec["pseudo"].as_str() {
            Some("::before") => 1,
            Some("::after") => 2,
            _ => 0,
        },
    );
    state.styles.motion.script(
        node,
        animation,
        spec["time"].as_f64().unwrap_or(0.) / 1000.,
        spec["rate"].as_f64(),
        old.map(|(_, old)| old)
            .filter(|_| !spec["frames"].is_array()),
    );
    Ok(Value::Null)
}

pub(super) fn media_matches(
    args: &Value,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let query = args[0].as_str().context("Expected a media query")?;
    let source = format!("@media {query} {{}}");
    let theme = state.theme();
    let sheet = state
        .styles
        .sheets
        .entry(source.as_str().into())
        .or_insert_with(|| Rc::new(css::sheet::Stylesheet::parse(&source)));
    Ok(sheet.matches_media(viewport[0], viewport[1], theme).into())
}

pub(super) fn properties() -> Result<Value> {
    Ok(json!(css::properties::Prop::names()))
}

pub(super) fn aliases() -> Result<Value> {
    let aliases = css::properties::Prop::names()
        .iter()
        .map(|name| format!("-webkit-{name}"));
    Ok(json!(
        aliases
            .filter(|alias| css::properties::Prop::unprefixed(alias).is_some())
            .collect::<Vec<_>>()
    ))
}

pub(super) fn css_path(args: &Value) -> Result<Value> {
    Ok(json!(crate::render::raster::canvas::path::svg_path(
        args[0].as_str().unwrap_or_default()
    )))
}

pub(super) fn specified(args: &Value) -> Result<Value> {
    Ok(json!(css::value::specified::specified(
        args[0].as_str().unwrap_or_default(),
        args[1].as_str().unwrap_or_default()
    )))
}

pub(super) fn declare(args: &Value) -> Result<Value> {
    Ok(json!(
        css::sheet::declare(args[0].as_str().unwrap_or_default()).concat()
    ))
}

pub(super) fn constructed(op: &str, args: &Value, state: &mut PageState) -> Result<Value> {
    let text = args[2].as_str().unwrap_or_default();
    let result = match op {
        "dom.supports" => {
            let name = args[0].as_str().context("Expected CSS text")?;
            return Ok(json!(match args[1].as_str() {
                Some(value) =>
                    css::value::grammar::supports(&name.to_ascii_lowercase(), value) == Some(true),
                None => css::sheet::parser::supports_condition(name),
            }));
        }
        "dom.registerProperty" => {
            let name = args[0].as_str().context("Expected a name")?;
            let syntax = args[1].as_str().unwrap_or("*");
            let initial = args[3].as_str();
            let rule = format!(
                "@property {name}{{syntax:{};inherits:{};{}}}",
                json!(syntax),
                args[2].as_bool().unwrap_or(false),
                initial.map_or(String::new(), |value| format!("initial-value:{value}"))
            );
            let registered = css::sheet::Stylesheet::parse(&rule).property();
            // `Prop::new` leaks the names it interns, so only a valid new registration interns one.
            match registered {
                _ if !name.starts_with("--") => Err("SyntaxError"),
                _ if css::properties::Prop::find(name)
                    .is_some_and(|prop| state.styles.cache.scripted.contains_key(&prop)) =>
                {
                    Err("InvalidModificationError")
                }
                Some(value) => {
                    state
                        .styles
                        .cache
                        .scripted
                        .insert(css::properties::Prop::new(name), value);
                    Ok(Value::Null)
                }
                None => Err("SyntaxError"),
            }
        }
        "dom.adopt" => {
            let root = args[0].as_u64().context("Expected a root")? as usize;
            let ids = args[1]
                .as_array()
                .context("Expected sheets")?
                .iter()
                .filter_map(|id| Some(id.as_u64()? as u32))
                .collect::<Vec<_>>();
            if ids.is_empty() {
                state.styles.cache.adopted.remove(&root);
            } else {
                state.styles.cache.adopted.insert(root, ids);
            }
            Ok(Value::Null)
        }
        _ => {
            let id = args[0].as_u64().context("Expected a sheet")? as u32;
            match args[1].as_str().unwrap_or_default() {
                "parse" => return Ok(css::sheet::Stylesheet::parse(text).cssom()),
                kind @ ("replace" | "load") => {
                    let sheet = css::sheet::Stylesheet::parse(text);
                    let tree = if kind == "load" {
                        Value::Null
                    } else {
                        sheet.cssom()
                    };
                    state.styles.cache.constructed.insert(id, Rc::new(sheet));
                    Ok(tree)
                }
                "drop" => {
                    state.styles.cache.constructed.remove(&id);
                    Ok(Value::Null)
                }
                kind => {
                    let (path, top) = path(kind, &args[3]);
                    state
                        .styles
                        .cache
                        .edit_constructed(id, top, |sheet| sheet.edit(&path, edit(kind, text)))
                }
            }
        }
    };
    state.styles.invalidate();
    state.dirty = true;
    Ok(result.unwrap_or_else(|error| json!({ "error": error })))
}

pub(super) fn css_animations(
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let styles = state.restyle(doc, viewport);
    let mut nodes = Vec::new();
    let mut pending = vec![id];
    while let Some(node) = pending.pop() {
        nodes.push(node);
        if args[1].as_bool() == Some(true) {
            pending.extend(doc.nodes[node].children.iter().rev());
            pending.extend(state.tree.shadows.get(&node));
        }
    }
    // Without a layout, answer only when the frame would start nothing here.
    if args[3].as_bool() == Some(true) && !state.styles.motion.settled(&nodes, &styles) {
        return Ok(Value::Null);
    }
    Ok(json!(state.styles.motion.css(
        &nodes,
        args[2].as_bool() == Some(true),
        &styles
    )))
}

pub(super) fn animated(
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let styles = state.restyle(doc, viewport);
    Ok(state
        .styles
        .motion
        .scripted(id, 0, &styles, args[1].as_u64().unwrap_or(0) as u32)
        .map_or(Value::Null, |values| {
            values
                .iter()
                .map(|(key, value)| (key.name().to_owned(), json!(&**value)))
                .collect::<serde_json::Map<_, _>>()
                .into()
        }))
}

pub(super) fn computed(
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let styles = state.restyle(doc, viewport);
    let pseudo = args[2].as_str().unwrap_or_default().trim_start_matches(':');
    if let Some(style) = styles.values.get(id) {
        state.styles.motion.read(id, style);
    }
    let slot = match pseudo {
        "" => Some(0),
        "before" => Some(1),
        "after" => Some(2),
        _ => None,
    };
    let scripted = slot.and_then(|slot| state.styles.motion.scripted(id, slot, &styles, u32::MAX));
    // The compositor can play animations between frames, so they are sampled now. As in Chrome, values that
    // move boxes stay those of the last frame, which match its layout.
    let current = state.styles.animating.get(&id).and_then(|last| {
        let now = std::time::Instant::now();
        let current = state
            .styles
            .motion
            .timeline(id, 0, &styles, &[now], &[])
            .pop()?;
        let paints = current.iter().all(|(prop, value)| {
            crate::render::composited(prop) || last.get(prop) == Some(&**value)
        });
        paints.then_some(current)
    });
    let mut sampled = None;
    let backdrop = (pseudo == "backdrop")
        .then(|| state.styles.motion.backdrop(id, styles.backdrops.get(&id)?))
        .flatten();
    let style = match pseudo {
        "" => Some(current.as_ref().unwrap_or_else(|| {
            &**state
                .styles
                .animating
                .get(&id)
                .or_else(|| styles.authored.get(&id))
                .unwrap_or(&styles.values[id])
        })),
        "before" | "after" => state
            .styles
            .animating_pseudos
            .get(&id)
            .or_else(|| styles.pseudos.get(&id))
            .map(|values| &values[usize::from(pseudo == "after")]),
        "backdrop" => backdrop
            .as_ref()
            .or_else(|| styles.backdrops.get(&id).map(|style| &**style)),
        "selection" => styles.selections.get(&id).map(|style| &**style),
        // The button of a file input shares the slot of the scrollbar track.
        "file-selector-button" | "-webkit-file-upload-button"
            if doc.nodes[id].tag == "input"
                && doc.nodes[id].input_type().eq_ignore_ascii_case("file") =>
        {
            styles
                .scrollbars
                .get(&id)
                .and_then(|styles| styles[1].as_ref())
        }
        "marker" | "first-letter" | "first-line" | "placeholder" => {
            let index = ["marker", "first-letter", "first-line"]
                .iter()
                .position(|name| *name == pseudo)
                .unwrap_or(3);
            styles
                .text_pseudos
                .get(&id)
                .and_then(|styles| styles[index].as_ref())
        }
        _ => None,
    };
    let style = match (style, scripted) {
        (Some(style), Some(values)) => {
            let mut style = style.clone();
            style.extend(values.iter().map(|(key, value)| (key, value.clone())));
            Some(&*sampled.insert(style))
        }
        (style, _) => style,
    };
    let mut full = None;
    let style = match style {
        Some(style)
            if style.vars().is_empty()
                && !styles.vars[id].is_empty()
                && args[1].as_str().is_none_or(|name| name.starts_with("--")) =>
        {
            let mut copy = style.clone();
            copy.set_vars(styles.vars[id].clone());
            Some(&*full.insert(copy))
        }
        style => style,
    };
    let Some(style) = style.filter(|style| !style.is_empty()) else {
        return Ok(json!(if args[1].is_string() {
            json!(
                if args[1] == "content" && matches!(pseudo, "before" | "after") {
                    "none"
                } else {
                    ""
                }
            )
        } else {
            json!([])
        }));
    };
    let mut units = styles.lengths[id];
    // Container units: the width of the nearest container, the height of the nearest size container.
    let mut sizes = [None, None];
    let queried =
        !state.layout.containers().is_empty() && style.own().any(|(_, value)| value.contains("cq"));
    for node in std::iter::successors(doc.nodes[id].parent, |&node| doc.nodes[node].parent)
        .take_while(|_| queried)
    {
        let kind = styles.values[node].get(Prop::CONTAINER_TYPE);
        if let (Some(kind @ ("size" | "inline-size")), Some(size)) =
            (kind, state.layout.containers().get(&node))
        {
            sizes[0] = sizes[0].or(Some(size[0]));
            if kind == "size" {
                sizes[1] = Some(size[1]);
                break;
            }
        }
    }
    if sizes[0].is_some() {
        units.container = Some([0, 1].map(|axis| sizes[axis].unwrap_or(viewport[axis])));
    }
    let shown = |node: usize| {
        state.geometry.mirror.get(&node).is_some() && state.geometry.mirror.shown(node)
    };
    // Padding, plus borders when `borders`, of each side, with percentages of `base`.
    let edges =
        |style: &css::properties::Style, units: &css::value::units::Lengths, base, borders| {
            let mut edges = units.edges(style, css::properties::PADDING, base);
            for (edge, [width, kind, _]) in edges.iter_mut().zip(css::properties::BORDERS) {
                if borders
                    && style
                        .get(kind)
                        .is_some_and(|kind| !matches!(kind, "none" | "hidden"))
                {
                    *edge += style
                        .get(width)
                        .map_or(Some(3.), |width| units.resolve(width, base))
                        .unwrap_or(0.)
                        .max(0.);
                }
            }
            edges
        };
    let position = style.get(Prop::POSITION).unwrap_or("static");
    // The containing block: the parent's content box, or for positioned boxes the padding box of the
    // nearest positioned or transformed ancestor. `None` is the viewport.
    // `::before` and `::after` are children of their element.
    let generated = matches!(pseudo, "before" | "after");
    let start = if generated {
        Some(id)
    } else {
        doc.nodes[id].parent
    };
    let block = match position {
        "fixed" => None,
        "absolute" => std::iter::successors(start, |&node| doc.nodes[node].parent).find(|&node| {
            styles.values[node]
                .get(Prop::POSITION)
                .is_some_and(|value| value != "static")
                || styles.values[node]
                    .get(Prop::TRANSFORM)
                    .is_some_and(|value| value != "none")
        }),
        _ => start,
    }
    .and_then(|node| {
        let rect = *state.geometry.mirror.get(&node)?;
        let (style, units) = (&styles.values[node], &styles.lengths[node]);
        let outer = edges(style, units, rect[2], true);
        let inner = state
            .layout
            .content_size(node)
            .unwrap_or([rect[2] - outer[1] - outer[3], rect[3] - outer[0] - outer[2]]);
        let padding = if position == "absolute" {
            edges(style, units, inner[0], false)
        } else {
            [0.; 4]
        };
        Some([
            rect[0] + outer[3] - padding[3],
            rect[1] + outer[0] - padding[0],
            inner[0] + padding[1] + padding[3],
            inner[1] + padding[0] + padding[2],
        ])
    })
    .unwrap_or([0., 0., viewport[0], viewport[1]]);
    // A grid item takes percentages from its grid area.
    let area = (!generated
        && !matches!(position, "absolute" | "fixed")
        && !style::table_part(doc, &styles, id))
    .then(|| {
        // The box parent, past `display: contents` boxes.
        std::iter::successors(
            state.tree.flat_parent(doc, &styles.children, id),
            |&parent| state.tree.flat_parent(doc, &styles.children, parent),
        )
        .find(|&parent| styles.values[parent].get(Prop::DISPLAY) != Some("contents"))
    })
    .flatten()
    .and_then(|grid| state.layout.grid_area(grid, id));
    let block = area.map_or(block, |[width, height]| [block[0], block[1], width, height]);
    let container = || [block[2], block[3]];
    let inline = !doc.nodes[id].tag.starts_with('#')
        && style
            .get(Prop::DISPLAY)
            .is_none_or(|display| display == "inline")
        && !matches!(
            doc.nodes[id].tag.as_str(),
            "img"
                | "svg"
                | "canvas"
                | "video"
                | "audio"
                | "iframe"
                | "embed"
                | "object"
                | "input"
                | "select"
                | "textarea"
                | "button"
        );
    // The untransformed border box. Inline boxes and boxes that are not rendered have none, so their
    // percentages stay as they are.
    // The box of a rendered `::before` or `::after`: its place, size and padding plus border.
    let pseudo_box = (generated
        && !inline
        && shown(id)
        && !matches!(style.get(Prop::DISPLAY), Some("none" | "contents"))
        && style
            .get(Prop::CONTENT)
            .is_some_and(|content| !matches!(content, "none" | "normal")))
    .then(|| state.layout.pseudo_box(id, pseudo == "after"))
    .flatten();
    let size = (pseudo.is_empty() && !inline && shown(id))
        .then(|| {
            let inner = state.layout.content_size(id)?;
            let sides = edges(style, &units, block[2], true);
            Some([
                inner[0] + sides[1] + sides[3],
                inner[1] + sides[0] + sides[2],
            ])
        })
        .flatten()
        .or_else(|| {
            (pseudo.is_empty() && !inline && shown(id))
                .then(|| {
                    state
                        .geometry
                        .mirror
                        .get(&id)
                        .map(|rect| [rect[2], rect[3]])
                })
                .flatten()
        })
        .or_else(|| pseudo_box.map(|rect| [rect[2], rect[3]]));
    let registry = state.styles.cache.registry();
    let parent = start.map(|parent| &*styles.values[parent]);
    let flow = start.is_some_and(|parent| style::table_box(style::role(&styles, parent)))
        || parent.is_none_or(|parent| {
            parent
                .get(Prop::DISPLAY)
                .is_none_or(|display| !display.ends_with("flex") && !display.ends_with("grid"))
        });
    let value = |name: &str| -> String {
        let name = css::value::resolved::physical_name(style, name);
        let get =
            |name| css::value::resolved::resolved(style, &units, name, size, &container, &registry);
        let value = get(&name);
        match (&*name, value.as_str()) {
            ("display", display_value) if pseudo.is_empty() => {
                display(&styles, id, display_value).to_owned()
            }
            ("gap" | "row-gap" | "column-gap", _) if style::table_box(style::role(&styles, id)) => {
                "normal".to_owned()
            }
            // Like Chrome, an element with no box, like `display: contents` or an element inside a
            // `display: none` box of the flat tree, has no transform.
            ("transform", transform)
                if transform != "none"
                    && size.is_none()
                    && (style.get(Prop::DISPLAY) == Some("contents")
                        || std::iter::successors(
                            state.tree.flat_parent(doc, &styles.children, id),
                            |&node| state.tree.flat_parent(doc, &styles.children, node),
                        )
                        .any(|node| styles.values[node].get(Prop::DISPLAY) == Some("none"))) =>
            {
                "none".to_owned()
            }
            // As in Chrome, an SVG shape with no size has none, and a percentage is of its viewport: the
            // `viewBox` or the box of its `<svg>`.
            ("width" | "height", given)
                if matches!(doc.nodes[id].tag.as_str(), "rect" | "image")
                    && (given == "auto" || given.contains('%')) =>
            {
                let axis = usize::from(&*name == "height");
                let svg =
                    std::iter::successors(doc.nodes[id].parent, |&node| doc.nodes[node].parent)
                        .find(|&node| doc.nodes[node].tag == "svg");
                let viewport = svg.and_then(|svg| {
                    let view_box: Vec<f32> = doc.nodes[svg]
                        .attrs
                        .get("viewBox")?
                        .split([' ', ','])
                        .filter_map(|part| part.parse().ok())
                        .collect();
                    view_box.get(2 + axis).copied()
                });
                let viewport = viewport.or_else(|| {
                    svg.and_then(|svg| state.geometry.mirror.get(&svg))
                        .map(|rect| rect[2 + axis])
                });
                match viewport.and_then(|base| units.resolve(given, base)) {
                    Some(length) => {
                        format!("{}px", css::value::tokens::number(f64::from(length)))
                    }
                    None => "0px".to_owned(),
                }
            }
            ("min-width" | "min-height", "auto") if flow => "0px".to_owned(),
            ("top" | "right" | "bottom" | "left", "auto")
                if position != "static" && position != "sticky" =>
            {
                let (opposite, axis) = match &*name {
                    "top" => ("bottom", 1),
                    "bottom" => ("top", 1),
                    "left" => ("right", 0),
                    _ => ("left", 0),
                };
                if position == "relative" {
                    let other = get(opposite);
                    return match other
                        .strip_suffix("px")
                        .and_then(|px| px.parse::<f32>().ok())
                    {
                        Some(px) if px != 0. => {
                            format!("{}px", css::value::tokens::number(f64::from(-px)))
                        }
                        _ => "0px".to_owned(),
                    };
                }
                let corner = if generated {
                    // The layout place of the pseudo-element is from the border box of its element.
                    let (Some([x, y, ..]), Some(&[left, top, ..])) =
                        (pseudo_box, state.geometry.mirror.get(&id))
                    else {
                        return value;
                    };
                    [left + x, top + y]
                } else {
                    let (Some(&[x, y, width, height]), Some(size)) =
                        (state.geometry.mirror.get(&id), size)
                    else {
                        return value;
                    };
                    // The bounds hold the transformed box, whose center is the transformed center of the box.
                    let bounds = renderer::Rect::new(
                        renderer::point(renderer::px(0.), renderer::px(0.)),
                        renderer::size(renderer::px(size[0]), renderer::px(size[1])),
                    );
                    let center = match crate::css::value::transform::transform(
                        style, &units, bounds, None,
                    ) {
                        Some(m) if m[3] == 0. && m[7] == 0. && m[15] == 1. => {
                            let [cx, cy] = [size[0] / 2., size[1] / 2.];
                            [m[0] * cx + m[4] * cy + m[12], m[1] * cx + m[5] * cy + m[13]]
                        }
                        _ => [size[0] / 2., size[1] / 2.],
                    };
                    [x + width / 2. - center[0], y + height / 2. - center[1]]
                };
                let Some(size) = size else {
                    return value;
                };
                let margin = units.edges(style, css::properties::MARGIN, block[2]);
                let start = corner[axis] - margin[3 - 3 * axis] - block[axis];
                let used = if name == "top" || name == "left" {
                    start
                } else {
                    block[2 + axis] - start - size[axis] - margin[3 - 3 * axis] - margin[1 + axis]
                };
                format!(
                    "{}px",
                    css::value::tokens::number(f64::from((used * 64.).round() / 64.))
                )
            }
            ("margin-left" | "margin-right", "auto") if size.is_some() => {
                if !flow || position == "absolute" || position == "fixed" {
                    return "0px".to_owned();
                }
                let left = name == "margin-left";
                let other = get(if left { "margin-right" } else { "margin-left" });
                let other = other
                    .strip_suffix("px")
                    .and_then(|px| px.parse::<f32>().ok());
                let free = block[2] - size.unwrap_or_default()[0] - other.unwrap_or(0.);
                // CSS 2 §10.3.3: auto margins of a box wider than its containing block are 0, and the end
                // margin takes the rest.
                let rtl = parent.and_then(|parent| parent.get(Prop::DIRECTION)) == Some("rtl");
                let used = match other {
                    _ if free < 0. => {
                        if left == rtl {
                            free
                        } else {
                            0.
                        }
                    }
                    Some(_) => free,
                    None => free / 2.,
                };
                format!(
                    "{}px",
                    css::value::tokens::number(f64::from((used * 64.).round() / 64.))
                )
            }
            ("margin-top" | "margin-bottom", "auto") if size.is_some() => "0px".to_owned(),
            // Layout gives a pseudo-element box its padding, so its sizes are not resolved again.
            (
                "width" | "height" | "padding-top" | "padding-right" | "padding-bottom"
                | "padding-left",
                _,
            ) if let Some(rect) = pseudo_box => {
                let borders = [
                    Prop::BORDER_TOP_WIDTH,
                    Prop::BORDER_RIGHT_WIDTH,
                    Prop::BORDER_BOTTOM_WIDTH,
                    Prop::BORDER_LEFT_WIDTH,
                ]
                .map(|prop| {
                    get(prop.name())
                        .strip_suffix("px")
                        .and_then(|px| px.parse::<f32>().ok())
                        .unwrap_or(0.)
                });
                let used = match &*name {
                    "width" | "height" => {
                        let axis = usize::from(&*name == "height");
                        match style.get(Prop::BOX_SIZING) == Some("border-box") {
                            true => rect[2 + axis],
                            false => rect[2 + axis] - rect[5 - axis] - rect[7 - axis],
                        }
                    }
                    side => {
                        let side = [
                            "padding-top",
                            "padding-right",
                            "padding-bottom",
                            "padding-left",
                        ]
                        .iter()
                        .position(|name| *name == side)
                        .unwrap_or(0);
                        rect[4 + side] - borders[side]
                    }
                };
                format!(
                    "{}px",
                    css::value::tokens::number(f64::from((used.max(0.) * 64.).round() / 64.))
                )
            }
            // The image layers set the number of layers of the other lists.
            (
                "background-position"
                | "background-size"
                | "background-repeat"
                | "background-attachment"
                | "background-origin"
                | "background-clip"
                | "background-blend-mode"
                | "mask-position"
                | "mask-size"
                | "mask-repeat"
                | "mask-origin"
                | "mask-clip"
                | "mask-composite"
                | "mask-mode",
                _,
            ) => {
                let image = if name.starts_with("mask") {
                    "mask-image"
                } else {
                    "background-image"
                };
                let count = css::value::tokens::components(&get(image), ',').len();
                let layers = css::value::tokens::components(&value, ',');
                if layers.len() == count {
                    return value;
                }
                (0..count)
                    .map(|index| layers[index % layers.len()].trim())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
            // Like Chrome, a rendered grid gives its used tracks, with the line names of explicit ones.
            ("grid-template-columns" | "grid-template-rows", _)
                if size.is_some()
                    && value != "subgrid"
                    && style
                        .get(Prop::DISPLAY)
                        .is_some_and(|display| display.ends_with("grid")) =>
            {
                let Some(tracks) = state.layout.tracks(id) else {
                    return value;
                };
                let used = &tracks[usize::from(name == "grid-template-rows")];
                let mut used = used.iter().map(|&track| {
                    format!(
                        "{}px",
                        css::value::tokens::number(f64::from((track * 64.).round() / 64.))
                    )
                });
                // `repeat()` with a count gives its tracks and names that many times.
                let mut parts = Vec::new();
                for part in css::value::tokens::components(&value, ' ') {
                    let repeat = part
                        .strip_prefix("repeat(")
                        .and_then(|inner| inner.strip_suffix(')')?.split_once(','))
                        .and_then(|(count, body)| {
                            Some((count.trim().parse::<usize>().ok()?, body.trim()))
                        });
                    match repeat {
                        Some((count, body)) => {
                            for _ in 0..count.min(1000) {
                                parts.extend(css::value::tokens::components(body, ' '));
                            }
                        }
                        None => parts.push(part),
                    }
                }
                let mut out: Vec<String> = Vec::new();
                if value != "none" && !parts.iter().any(|part| part.starts_with("repeat(")) {
                    for part in parts {
                        match (part.strip_prefix('['), out.last_mut()) {
                            // Names of one line join, like `[b a]`.
                            (Some(names), Some(last)) if last.ends_with(']') => {
                                last.pop();
                                last.push(' ');
                                last.push_str(names);
                            }
                            (Some(_), _) => out.push(part.to_owned()),
                            (None, _) => out.extend(used.next()),
                        }
                    }
                }
                out.extend(used);
                match out.is_empty() {
                    true => value,
                    false => out.join(" "),
                }
            }
            ("content", "normal") if matches!(pseudo, "before" | "after") => "none".to_owned(),
            // Chrome gives the strings and `attr()` values of generated content as one string.
            ("content", _) if !pseudo.is_empty() && value != "none" && value != "normal" => {
                // Counters keep their declared form, which `counting` holds.
                let value = match styles.counting.get(&id) {
                    Some(sides) if matches!(pseudo, "before" | "after") => sides
                        [usize::from(pseudo == "after")]
                    .as_deref()
                    .map_or(value, str::to_owned),
                    _ => value,
                };
                let (mut parts, mut text) = (Vec::new(), None::<String>);
                let quote = |text: String| {
                    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
                };
                for part in css::value::tokens::components(&value, ' ') {
                    let mut input = cssparser::Parser::new(part);
                    let mut strings = None::<String>;
                    while let Ok(string) = input.expect_string() {
                        strings.get_or_insert_default().push_str(string);
                    }
                    let string = strings.filter(|_| input.is_exhausted()).or_else(|| {
                        let name = part.strip_prefix("attr(")?.strip_suffix(')')?.trim();
                        Some(doc.nodes[id].attrs.get(name).cloned().unwrap_or_default())
                    });
                    match string {
                        Some(string) => text.get_or_insert_default().push_str(&string),
                        None => {
                            parts.extend(text.take().map(quote));
                            parts.push(part.to_owned());
                        }
                    }
                }
                parts.extend(text.map(quote));
                parts.join(" ")
            }
            _ => value,
        }
    };
    Ok(match args[1].as_str() {
        Some(name @ ("inset" | "margin")) => {
            let sides = if name == "inset" {
                ["top", "right", "bottom", "left"]
            } else {
                ["margin-top", "margin-right", "margin-bottom", "margin-left"]
            }
            .map(value);
            json!(css::value::specified::shortest(
                sides.each_ref().map(String::as_str)
            ))
        }
        Some(name) => json!(value(name)),
        None => json!(
            css::value::grammar::computed_names()
                .chain(style.vars().iter().map(|(prop, _)| prop.name()))
                .collect::<Vec<_>>()
        ),
    })
}

pub(super) fn css(id: usize, doc: &mut Document, state: &mut PageState) -> Result<Value> {
    if doc.nodes[id].tag == "link" && doc.nodes[id].stylesheet.is_none() {
        return Ok(Value::Null);
    }
    let sheet = state.styles.cache.sheet(doc, id, &mut state.styles.sheets);
    Ok(sheet.map_or_else(|| json!([]), |sheet| sheet.cssom()))
}

pub(super) fn sheet(
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
) -> Result<Value> {
    let kind = args[1].as_str().unwrap_or_default();
    let text = args[2].as_str().unwrap_or_default();
    let (path, top) = path(kind, &args[3]);
    let result =
        state
            .styles
            .cache
            .edit_sheet(doc, id, &mut state.styles.sheets, top, |sheet, disabled| {
                if kind == "disabled" {
                    *disabled = args[2].as_bool().unwrap_or(false);
                    return Ok(Value::Null);
                }
                sheet.edit(&path, edit(kind, text))
            });
    state.styles.invalidate();
    state.dirty = true;
    Ok(result.unwrap_or_else(|error| json!({ "error": error })))
}

pub(super) fn font(args: &Value, state: &mut PageState) -> Result<Value> {
    let family = args[1].as_str().context("Expected font family")?;
    Ok(match args[2].as_str() {
        Some(name) => json!(state.local_font(family, name)),
        None => json!(state.add_font(family, crate::web::bytes(&args[2])?)?),
    })
}

pub(super) fn font_sources(args: &Value) -> Result<Value> {
    Ok(json!(crate::render::fonts::sources(
        args[1].as_str().context("Expected font sources")?
    )))
}

pub(super) fn font_check(args: &Value, state: &mut PageState) -> Result<Value> {
    let family = args[1].as_str().context("Expected font family")?;
    let family = state
        .fonts
        .aliases
        .get(family)
        .map(String::as_str)
        .unwrap_or(family);
    Ok(json!(
        matches!(family, "serif" | "sans-serif" | "monospace" | "system-ui")
            || crate::render::raster::svg::fonts(true)
                .0
                .faces()
                .any(|face| face
                    .families
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case(family)))
    ))
}
