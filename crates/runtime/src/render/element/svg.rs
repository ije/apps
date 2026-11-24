use super::super::layout::size::dimension;
use super::Build;
use super::cx::Cx;
use super::kept::{Query, hashed};
use super::query::{href, uses};
use crate::css;
use crate::css::properties::Prop;
use crate::{
    dom::Document,
    style::{self, ComputedStyles},
};
use renderer::{Child, Rect, point, px, size};
use std::{rc::Rc, sync::Arc};

pub(super) const SVG_SOURCE: [Prop; 13] = [
    Prop::TRANSLATE,
    Prop::ROTATE,
    Prop::SCALE,
    Prop::TRANSFORM_BOX,
    Prop::CX,
    Prop::CY,
    Prop::R,
    Prop::RX,
    Prop::RY,
    Prop::X,
    Prop::Y,
    Prop::WIDTH,
    Prop::HEIGHT,
];

pub(super) struct SvgSource<'a> {
    pub(super) doc: &'a Document,
    pub(super) styles: &'a ComputedStyles,
    pub(super) root: usize,
    pub(super) scale: f32,
    pub(super) view: [f32; 2],
    pub(super) header: String,
    pub(super) ids: &'a super::super::effects::Ids,
}

impl SvgSource<'_> {
    pub(super) fn attrs(
        &self,
        element: usize,
        paint: bool,
        transform: bool,
    ) -> Vec<(&'static str, String)> {
        let (doc, root) = (self.doc, self.root);
        let own = &self.styles.values[element];
        let units = &self.styles.lengths[element];
        let tag = doc.nodes[element].tag.as_str();
        let mut attrs = Vec::new();
        if element != root {
            for prop in &SVG_SOURCE[4..] {
                let applies = match *prop {
                    Prop::CX | Prop::CY => matches!(tag, "circle" | "ellipse"),
                    Prop::R => tag == "circle",
                    Prop::RX | Prop::RY => matches!(tag, "ellipse" | "rect"),
                    _ => matches!(tag, "rect" | "image" | "foreignObject"),
                };
                if let Some(value) = own.get(*prop).filter(|value| applies && *value != "auto") {
                    let value = if value.ends_with('%') || value.parse::<f32>().is_ok() {
                        value.to_owned()
                    } else {
                        units
                            .resolve(value, f32::NAN)
                            .map_or_else(|| value.to_owned(), |length| length.to_string())
                    };
                    attrs.push((prop.name(), value));
                }
            }
        }
        // resvg doesn't take CSS transforms, `transform-box` or `transform-origin`, so they become a matrix.
        if transform
            && element != root
            && super::super::effects::TRANSFORM_PROPS
                .iter()
                .any(|&prop| own.get(prop).is_some())
        {
            let fill = own
                .get(Prop::TRANSFORM_BOX)
                .is_some_and(|value| value != "view-box");
            let [x, y, w, h] = fill
                .then(|| self.shape(element))
                .flatten()
                .map_or([0., 0., self.view[0], self.view[1]], |(rect, _)| rect);
            let bounds = Rect::new(point(px(x), px(y)), size(px(w), px(h)));
            attrs.push((
                "transform",
                crate::css::value::transform::transform(own, units, bounds, None).map_or(
                    String::new(),
                    |m| {
                        format!(
                            "matrix({} {} {} {} {} {})",
                            m[0], m[1], m[4], m[5], m[12], m[13]
                        )
                    },
                ),
            ));
        }
        if !paint {
            return attrs;
        }
        let parent = doc.nodes[element]
            .parent
            .map(|parent| &self.styles.values[parent]);
        // resvg ignores `vector-effect: non-scaling-stroke` and `pathLength`, so stroke widths and dashes are
        // scaled here.
        let width = if element != root
            && self.scale != 1.
            && own.get(Prop::VECTOR_EFFECT) == Some("non-scaling-stroke")
        {
            1. / self.scale
        } else {
            1.
        };
        let dashed = own
            .get(Prop::STROKE_DASHARRAY)
            .is_some_and(|value| value != "none")
            || own
                .get(Prop::STROKE_DASHOFFSET)
                .is_some_and(|value| value.trim_end_matches("px").parse::<f32>() != Ok(0.));
        let dashes = width
            * doc.nodes[element]
                .attrs
                .get("pathLength")
                .and_then(|value| value.trim().parse::<f32>().ok())
                .filter(|length| *length > 0. && dashed && element != root)
                .and_then(|length| Some(self.shape(element)?.1 / length))
                .filter(|ratio| *ratio > 0.)
                .unwrap_or(1.);
        let mut result = String::new();
        for prop in style::SVG {
            // A style holds the lines of a text decoration, and SVG reads the short form. The lines include the
            // ones of ancestors, which don't reach into an image, so only a change from the parent counts.
            let value = own.get(prop).or_else(|| {
                let outer = parent.and_then(|parent| parent.get(Prop::TEXT_DECORATION_LINE));
                own.get(Prop::TEXT_DECORATION_LINE)
                    .filter(|line| prop == Prop::TEXT_DECORATION && outer != Some(*line))
            });
            let Some(value) = value else { continue };
            let factor = match prop {
                Prop::TRANSFORM | Prop::TRANSFORM_ORIGIN => continue,
                Prop::STROKE_WIDTH => width,
                Prop::STROKE_DASHARRAY | Prop::STROKE_DASHOFFSET => dashes,
                _ => 1.,
            };
            if if element == root {
                !prop.inherited()
            } else {
                prop.inherited()
                    && factor == 1.
                    && !doc.nodes[element].attrs.contains_key(prop.name())
                    && parent.and_then(|parent| parent.get(prop)) == Some(value)
            } {
                continue;
            }
            let value = match prop {
                Prop::DISPLAY if value != "none" => continue,
                Prop::CLIP_PATH | Prop::MASK | Prop::FILTER if !value.starts_with("url(") => {
                    continue;
                }
                Prop::VECTOR_EFFECT => continue,
                Prop::FONT_SIZE => format!("{}px", units.font.em),
                // Resolve currentColor from the <use> element's inherited color, not the referenced
                // definition.
                Prop::FILL
                | Prop::STROKE
                | Prop::STOP_COLOR
                | Prop::FLOOD_COLOR
                | Prop::LIGHTING_COLOR
                    if value.eq_ignore_ascii_case("currentcolor") =>
                {
                    "currentColor".to_owned()
                }
                Prop::FILL
                | Prop::STROKE
                | Prop::COLOR
                | Prop::STOP_COLOR
                | Prop::FLOOD_COLOR
                | Prop::LIGHTING_COLOR => css::value::color::color(value)
                    .map_or(value.to_owned(), |(rgb, alpha)| {
                        format!("#{rgb:06x}{alpha:02x}")
                    }),
                _ if factor != 1. => value
                    .split(|c: char| c == ',' || c.is_ascii_whitespace())
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        part.strip_suffix("px")
                            .unwrap_or(part)
                            .parse::<f32>()
                            .map_or(part.to_owned(), |length| (length * factor).to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => value.to_owned(),
            };
            result.push_str(prop.name());
            result.push(':');
            result.push_str(&value);
            result.push(';');
        }
        if !result.is_empty() {
            attrs.push(("style", result));
        }
        attrs
    }

    pub(super) fn shape(&self, element: usize) -> Option<([f32; 4], f32)> {
        let attrs = |node| {
            let mut attrs = self.attrs(node, false, node != element);
            // resvg drops shapes without paint.
            attrs.push(("fill", "#000".into()));
            if node == element {
                attrs.extend([("id", "apps-shape".into()), ("transform", String::new())]);
                if self.doc.nodes[node].tag == "use" {
                    attrs.extend([("x", String::new()), ("y", String::new())]);
                }
            }
            attrs
        };
        let (mut targets, mut pending) = (Vec::new(), vec![element]);
        while let Some(node) = pending.pop() {
            let node = &self.doc.nodes[node];
            if node.tag == "use"
                && let Some(target) = href(node).and_then(|name| {
                    super::super::effects::index(self.ids, self.doc)
                        .get(name)?
                        .first()
                        .copied()
                })
                && !targets.contains(&target)
            {
                targets.push(target);
                pending.push(target);
            }
            pending.extend(&node.children);
        }
        let mut source = format!("{}><defs>", self.header);
        for target in targets {
            source.push_str(&self.doc.serialize_svg(target, false, &attrs));
        }
        source.push_str("</defs>");
        source.push_str(&self.doc.serialize_svg(element, false, &attrs));
        source.push_str("</svg>");
        super::super::raster::svg::svg_shape(&source)
    }
}

pub(crate) type Svg = (Vec<Rc<css::properties::Style>>, ([f32; 2], u64));

/// `padding` places the drawing in the content box, as the layer is absolute in the padding box.
pub(super) fn svg_layer(
    id: usize,
    inline: bool,
    spill: bool,
    view: Rc<super::super::ViewState>,
    padding: [f32; 4],
) -> Child {
    let geometry = view.clone();
    renderer::canvas(
        move |bounds, window| {
            let [width, height] = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            if !inline || width <= 0. || height <= 0. {
                return;
            }
            let mut host = geometry.host.borrow_mut();
            let state = host.state(geometry.id);
            if let Some(source) = state.media.svg_sources.get(&id)
                && let Some(entry) = state.rasters.svg_boxes.get_mut(&id)
            {
                if entry.0 != *source {
                    // Drop stale svg_trees entries here: repeated draws can skip paint and leave old sources
                    // cached.
                    state.rasters.svg_trees.remove(&id);
                    let (size, content, boxes, painted) = match state.rasters.svg_cache.get(source)
                    {
                        Some((geometry, _)) => geometry.clone(),
                        None => {
                            let tree = super::super::raster::svg::svg_tree(source, false).ok();
                            let geometry = tree
                                .as_ref()
                                .map(super::super::raster::svg::svg_boxes)
                                .unwrap_or_default();
                            if let Some(tree) = tree {
                                state.rasters.svg_trees.insert(id, tree);
                                // Bound the cache here: this is the only place that adds sources.
                                if state.rasters.svg_cache.len() >= 256 {
                                    for image in state
                                        .rasters
                                        .svg_cache
                                        .drain()
                                        .flat_map(|(_, entry)| entry.1)
                                    {
                                        if Arc::strong_count(&image) == 1 {
                                            window.release_image(image);
                                        }
                                    }
                                }
                                state
                                    .rasters
                                    .svg_cache
                                    .insert(source.clone(), (geometry.clone(), Vec::new()));
                            }
                            geometry
                        }
                    };
                    let names = &entry.1;
                    entry.2 = size;
                    entry.4 = content;
                    entry.5 = painted;
                    entry.3 = boxes
                        .into_iter()
                        .filter_map(|(name, canvas, object)| {
                            let node = match name.strip_prefix("apps-") {
                                Some(mark) => names.get(mark.parse::<usize>().ok()?)?.1,
                                None => names.iter().find(|(other, _)| *other == name)?.1,
                            };
                            Some((node, canvas, object))
                        })
                        .collect();
                    entry.0 = source.clone();
                }
                let [scale_x, scale_y] =
                    [0, 1].map(|axis| [width, height][axis] / entry.2[axis].max(f32::EPSILON));
                let [x, y, w, h] = entry.5;
                let reach = (-x * scale_x)
                    .max(-y * scale_y)
                    .max((x + w) * scale_x - width)
                    .max((y + h) * scale_y - height);
                // Like the cull margin of a box shadow.
                let far = spill && reach >= f32::from(window.viewport_size().height);
                if far != state.layout.cull.spills.contains(&id) {
                    match far {
                        true => state.layout.cull.spills.insert(id),
                        false => state.layout.cull.spills.remove(&id),
                    };
                    state.layout.cull.swapped.push(id);
                    state.layout.cull.respilled.push(id);
                    window.redraw();
                }
                for &(node, [x, y, w, h], _) in &entry.3 {
                    let rect = window.transformed_bounds(Rect::new(
                        point(
                            bounds.left() + px(x * scale_x),
                            bounds.top() + px(y * scale_y),
                        ),
                        size(px(w * scale_x), px(h * scale_y)),
                    ));
                    state.geometry.mirror.insert(
                        node,
                        [
                            rect.origin.x.into(),
                            rect.origin.y.into(),
                            rect.size.width.into(),
                            rect.size.height.into(),
                        ],
                    );
                }
            }
        },
        move |bounds, _, window| {
            let [width, height] = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            if width <= 0. || height <= 0. {
                return;
            }
            let scale = window.scale_factor();
            let view = &*view;
            let mut host = view.host.borrow_mut();
            let state = host.state(view.id);
            // The painted area in the box, in CSS pixels on device pixel edges. A long spill paints only near
            // the content mask, in steps, so it keeps the device scale.
            let mask = window.content_mask().bounds;
            let mut cut = false;
            let area = match state.rasters.svg_boxes.get(&id).filter(|_| spill) {
                Some((_, _, [sw, sh], _, _, [x, y, w, h], _)) => {
                    let [kx, ky] = [width / sw.max(f32::EPSILON), height / sh.max(f32::EPSILON)];
                    let mut edges = [
                        (x * kx).min(0.),
                        (y * ky).min(0.),
                        ((x + w) * kx).max(width),
                        ((y + h) * ky).max(height),
                    ];
                    if mask.size.width < px(1e5) {
                        const STEP: f32 = 256.;
                        let [left, top] = [mask.left() - bounds.left(), mask.top() - bounds.top()]
                            .map(|at| ((f32::from(at) - STEP) / STEP).floor() * STEP);
                        let [right, bottom] =
                            [mask.right() - bounds.left(), mask.bottom() - bounds.top()]
                                .map(|at| ((f32::from(at) + STEP) / STEP).ceil() * STEP);
                        let near = [
                            edges[0].max(left),
                            edges[1].max(top),
                            edges[2].min(right),
                            edges[3].min(bottom),
                        ];
                        cut = near != edges;
                        edges = near;
                    }
                    let [left, top] = [edges[0], edges[1]].map(|at| (at * scale).floor() / scale);
                    let [right, bottom] =
                        [edges[2], edges[3]].map(|at| (at * scale).ceil() / scale);
                    [left, top, right - left, bottom - top]
                }
                _ => [0., 0., width, height],
            };
            if cut {
                window.paint_volatile();
            }
            if area[2] <= 0. || area[3] <= 0. {
                return;
            }
            let moved = area != [0., 0., width, height];
            let raster =
                [area[2], area[3]].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
            let source = if inline {
                state.media.svg_sources.get(&id).cloned()
            } else {
                state
                    .media
                    .svg_images
                    .get(&id)
                    .map(|(source, _)| source.clone())
            };
            let Some(source) = source else { return };
            if state.rasters.svgs.get(&id).is_none_or(|(old, image)| {
                !inline && *old != source
                    || (image.size().width as u32) != raster[0]
                    || (image.size().height as u32) != raster[1]
            }) || moved
                && state
                    .rasters
                    .svg_boxes
                    .get(&id)
                    .is_some_and(|entry| entry.6 != area)
            {
                if let Some(entry) = state.rasters.svg_boxes.get_mut(&id) {
                    entry.6 = area;
                }
                let shared = state
                    .rasters
                    .svg_cache
                    .get(&source)
                    .filter(|_| inline && !moved)
                    .and_then(|(_, images)| {
                        images.iter().find(|image| {
                            [image.size().width as u32, image.size().height as u32] == raster
                        })
                    })
                    .cloned();
                let image = match (
                    &shared,
                    state.rasters.svg_trees.remove(&id).filter(|_| inline),
                ) {
                    (Some(image), _) => Ok(image.clone()),
                    (_, Some(tree)) if !moved => {
                        super::super::raster::svg::render_svg(&tree, raster[0], raster[1], None)
                    }
                    (_, tree) if moved => tree
                        .map_or_else(|| super::super::raster::svg::svg_tree(&source, false), Ok)
                        .and_then(|tree| {
                            let [kx, ky] = [raster[0] as f32 / area[2], raster[1] as f32 / area[3]];
                            super::super::raster::svg::render_svg(
                                &tree,
                                raster[0],
                                raster[1],
                                Some((
                                    [width * kx, height * ky],
                                    [1., 0., 0., 1., -area[0] * kx, -area[1] * ky],
                                )),
                            )
                        }),
                    _ => super::super::raster::svg::svg(&source, raster[0], raster[1]),
                };
                match image {
                    Ok(image) => {
                        if inline
                            && shared.is_none()
                            && raster[0] <= 96
                            && raster[1] <= 96
                            && !moved
                            && let Some((_, images)) = state.rasters.svg_cache.get_mut(&source)
                        {
                            // Keep the rasters of the last few sizes, so a resized icon doesn't grow the list.
                            if images.len() >= 4 {
                                let old = images.remove(0);
                                if Arc::strong_count(&old) == 1 {
                                    window.release_image(old);
                                }
                            }
                            images.push(image.clone());
                        }
                        if let Some((_, old)) = state.rasters.svgs.insert(id, (source, image))
                            && Arc::strong_count(&old) == 1
                        {
                            window.release_image(old);
                        }
                    }
                    Err(error) => eprintln!("apps: SVG: {error}"),
                }
            }
            let image = state.rasters.svgs.get(&id).map(|(_, image)| image.clone());
            drop(host);
            if let Some(image) = image {
                let target = Rect::new(
                    point(bounds.left() + px(area[0]), bounds.top() + px(area[1])),
                    size(px(area[2]), px(area[3])),
                );
                window.draw_image(
                    target,
                    target,
                    renderer::Corners::default(),
                    image,
                    false,
                    false,
                );
            }
        },
    )
    .absolute()
    .top(px(padding[0]))
    .right(px(padding[1]))
    .bottom(px(padding[2]))
    .left(px(padding[3]))
    .into()
}

/// Writes the SVG source of an inline `<svg>` for its raster, and sizes its box.
#[inline(never)]
pub(super) fn svg_box(
    cx: &Cx<'_>,
    mut el: renderer::Node,
    content_layers: &mut Vec<Child>,
) -> renderer::Node {
    let Cx {
        reader,
        id,
        node,
        style,
        units,
        position,
        containing,
        ..
    } = *cx;
    let Build {
        doc,
        styles,
        ids,
        view,
        flex_parent,
        ..
    } = *reader;
    // Padding and borders, which the drawing doesn't fill.
    let frame = {
        let padding = units.edges(style, css::properties::PADDING, containing[0]);
        let borders = css::properties::BORDERS.map(|[width, kind, _]| {
            style
                .get(kind)
                .filter(|kind| !matches!(*kind, "none" | "hidden"))
                .and_then(|_| units.resolve(style.get(width).unwrap_or("3px"), containing[0]))
                .unwrap_or(0.)
                .max(0.)
        });
        [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ]
    };
    let border_box = style.get(Prop::BOX_SIZING) == Some("border-box");
    let [width, height] = [(Prop::WIDTH, "width"), (Prop::HEIGHT, "height")]
        .into_iter()
        .enumerate()
        .map(|(axis, (prop, attr))| {
            match style.get(prop) {
                Some(value) => units.resolve(value, containing[axis]),
                None => node.attrs.get(attr).and_then(|v| dimension(v, false)),
            }
            .map(|size| if border_box { size - frame[axis] } else { size })
        })
        .collect::<Vec<_>>()[..]
    else {
        unreachable!()
    };
    let view_box = node
        .attrs
        .get("viewBox")
        .map(|value| {
            value
                .split(|c: char| c == ',' || c.is_ascii_whitespace())
                .filter_map(|part| part.parse::<f32>().ok())
                .collect::<Vec<_>>()
        })
        .filter(|parts| parts.len() == 4 && parts[2] > 0. && parts[3] > 0.);
    let ratio = view_box.as_ref().map(|parts| parts[2] / parts[3]);
    // Like browsers: an SVG with a ratio and no size fills the available width, and one with neither is
    // 300 by 150.
    let available = || {
        let margins = units.edges(style, css::properties::MARGIN, containing[0]);
        let insets = if matches!(position, "absolute" | "fixed") {
            [Prop::LEFT, Prop::RIGHT].map(|prop| {
                style
                    .get(prop)
                    .and_then(|value| units.resolve(value, containing[0]))
                    .unwrap_or(0.)
            })
        } else {
            [0.; 2]
        };
        containing[0] - margins[1] - margins[3] - insets[0] - insets[1]
    };
    let [width, height] = match (width, height, ratio) {
        (None, Some(height), Some(ratio)) => [height * ratio, height],
        (Some(width), None, Some(ratio)) => [width, width / ratio],
        // A stretched flex item in a row of definite height takes that height.
        (None, None, Some(ratio))
            if flex_parent
                && reader
                    .parent(id, Prop::FLEX_DIRECTION)
                    .is_none_or(|value| value.starts_with("row"))
                && reader
                    .parent(id, Prop::HEIGHT)
                    .is_some_and(|value| value.ends_with("px"))
                && style
                    .get(Prop::ALIGN_SELF)
                    .filter(|value| *value != "auto")
                    .or_else(|| reader.parent(id, Prop::ALIGN_ITEMS))
                    .is_none_or(|value| matches!(value, "normal" | "stretch"))
                && containing[1] > 0. =>
        {
            let margins = units.edges(style, css::properties::MARGIN, containing[0]);
            let height = containing[1] - margins[0] - margins[2] - frame[1];
            [height * ratio, height]
        }
        (None, None, Some(ratio)) => {
            let width = available() - frame[0];
            [width, width / ratio]
        }
        _ => [width.unwrap_or(300.), height.unwrap_or(150.)],
    }
    .map(|value| value.clamp(1., 4096.));
    let [inner_width, inner_height] = [width, height];
    let [width, height] = [width + frame[0], height + frame[1]];
    let mut scope = id;
    while let Some(parent) = doc.nodes[scope].parent {
        scope = parent;
    }
    let mut pending = vec![id];
    let mut visited = rustc_hash::FxHashSet::default();
    let mut linked = false;
    let mut targets = Vec::new();
    let mut painted = Vec::new();
    let mut hasher = rustc_hash::FxHasher::default();
    while let Some(child) = pending.pop() {
        if !visited.insert(child) {
            continue;
        }
        std::hash::Hash::hash(&(child, doc.nodes.revision(child)), &mut hasher);
        painted.push(styles.values[child].clone());
        let child = &doc.nodes[child];
        pending.extend(&child.children);
        linked |= child.tag == "use";
        if child.tag == "use"
            && let Some(reference) = href(child)
            && let Some(target) = super::super::effects::index(ids, doc)
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
            targets.push(target);
            pending.push(target);
        }
    }
    // `uses` finds a link only through a `use` element of the subtree just walked.
    let used = if linked {
        uses(doc, styles, ids, id)
    } else {
        None
    };
    if used.is_some() {
        reader.log(Query::Uses(id as u32), 1, hashed(used));
    }
    let key = (
        [inner_width, inner_height],
        std::hash::Hasher::finish(&hasher),
    );
    let props = || style::SVG.iter().chain(&SVG_SOURCE);
    let kept = view.svgs.borrow().get(&id).is_some_and(|(old, at)| {
        *at == key
            && old.len() == painted.len()
            && old
                .iter()
                .zip(&painted)
                .enumerate()
                .all(|(index, (old, new))| {
                    Rc::ptr_eq(old, new)
                        || props().all(|&prop| {
                            index == 0 && !prop.inherited() || old.get(prop) == new.get(prop)
                        })
                })
    }) && view
        .host
        .borrow()
        .page_state(view.id)
        .is_some_and(|state| state.media.svg_sources.contains_key(&id));
    if !kept {
        let mut header = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{inner_width}\" height=\"{inner_height}\""
        );
        for (key, value) in &node.attrs {
            if !matches!(
                key.as_str(),
                "width" | "height" | "xmlns" | "xmlns:xlink" | "style"
            ) && style::presentation(key).is_none()
            {
                header.push_str(&format!(" {key}=\"{}\"", crate::dom::escape(value)));
            }
        }
        header.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
        let svg = SvgSource {
            doc,
            styles,
            root: id,
            scale: view_box.as_ref().map_or(1., |parts| {
                (inner_width / parts[2]).min(inner_height / parts[3])
            }),
            view: view_box
                .as_ref()
                .map_or([inner_width, inner_height], |parts| [parts[2], parts[3]]),
            header,
            ids,
        };
        let attrs = |element| svg.attrs(element, true, true);
        let mut source = svg.header.clone();
        for (name, value) in attrs(id) {
            source.push_str(&format!(" {name}=\"{}\"", crate::dom::escape(&value)));
        }
        source.push('>');
        source.push_str("<defs>");
        for &target in &targets {
            source.push_str(&doc.serialize_svg(target, false, &attrs));
        }
        source.push_str("</defs>");
        for child in &node.children {
            source.push_str(&doc.serialize_svg(*child, true, &attrs));
        }
        source.push_str("</svg>");
        let mut names = Vec::new();
        let mut counted = String::with_capacity(source.len());
        let mut rest = &*source;
        const MARK: &str = " id=\"apps-";
        while let Some(start) = rest.find(MARK) {
            let (before, after) = rest.split_at(start + MARK.len());
            counted.push_str(before);
            rest = after;
            if let Some(end) = after.find('"')
                && let Ok(marked) = after[..end].parse()
            {
                counted.push_str(&names.len().to_string());
                names.push((String::new(), marked));
                rest = &after[end..];
            }
        }
        counted.push_str(rest);
        let source = counted;
        let mut pending = node.children.clone();
        while let Some(child) = pending.pop() {
            if let Some(name) = doc.nodes[child].attrs.get("id") {
                names.push((name.clone(), child));
            }
            pending.extend(&doc.nodes[child].children);
        }
        // CSS keywords ignore case, but resvg only reads `currentColor`.
        let lower = source.to_ascii_lowercase();
        let mut end = 0;
        let mut replaced = String::with_capacity(source.len());
        for (start, _) in lower.match_indices("currentcolor") {
            replaced.push_str(&source[end..start]);
            replaced.push_str("currentColor");
            end = start + "currentcolor".len();
        }
        replaced.push_str(&source[end..]);
        let source: Rc<str> = replaced.into();
        view.svgs.borrow_mut().insert(id, (painted, key));
        let mut host = view.host.borrow_mut();
        let state = host.state(view.id);
        // Discard the raster on source changes: paint checks only inline SVG dimensions.
        if state
            .rasters
            .svgs
            .get(&id)
            .is_some_and(|(old, _)| *old != source)
            && let Some((_, image)) = state.rasters.svgs.remove(&id)
            && Arc::strong_count(&image) == 1
        {
            state.media.retired.push(image);
        }
        state.rasters.svg_boxes.insert(
            id,
            (
                "".into(),
                names,
                [0.; 2],
                Vec::new(),
                [0.; 4],
                [0.; 4],
                [0.; 4],
            ),
        );
        state.media.svg_sources.insert(id, source);
    }
    el = el.w(px(width)).h(px(height));
    // As in Chrome, an `<svg>` with `overflow: visible` paints outside its box.
    let spill = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
        .into_iter()
        .all(|prop| style.get(prop).is_none_or(|value| value == "visible"));
    content_layers.push(svg_layer(
        id,
        true,
        spill,
        view.clone(),
        units.edges(style, css::properties::PADDING, containing[0]),
    ));
    el
}
