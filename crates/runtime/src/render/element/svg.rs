use super::query::href;
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
        let units = &self.styles.lengths[element].unzoomed();
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
            let mut store = geometry.store.borrow_mut();
            let state = store.state(geometry.id);
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
                            let tree = super::super::raster::svg::svg_tree(source).ok();
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
                    state.geometry.shapes.remove(&node);
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
            let mut store = view.store.borrow_mut();
            let state = store.state(view.id);
            // The painted area in the box, in CSS pixels on device pixel edges. A long spill or a large box
            // paints only near the content mask, in steps, so it keeps the device scale.
            let mask = window.content_mask().bounds;
            let mut cut = false;
            let area = match state.rasters.svg_boxes.get(&id) {
                Some((_, _, [sw, sh], _, _, [x, y, w, h], _)) => {
                    let mut edges = [0., 0., width, height];
                    if spill {
                        let [kx, ky] =
                            [width / sw.max(f32::EPSILON), height / sh.max(f32::EPSILON)];
                        edges = [
                            (x * kx).min(0.),
                            (y * ky).min(0.),
                            ((x + w) * kx).max(width),
                            ((y + h) * ky).max(height),
                        ];
                    }
                    // Above 4 MB of pixels.
                    let large = (edges[2] - edges[0]) * (edges[3] - edges[1]) * scale * scale
                        > 1024. * 1024.;
                    if (spill || large) && mask.size.width < px(1e5) {
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
                    if !spill && !cut {
                        [0., 0., width, height]
                    } else {
                        let [left, top] =
                            [edges[0], edges[1]].map(|at| (at * scale).floor() / scale);
                        let [right, bottom] =
                            [edges[2], edges[3]].map(|at| (at * scale).ceil() / scale);
                        [left, top, right - left, bottom - top]
                    }
                }
                _ => [0., 0., width, height],
            };
            if cut {
                window.paint_volatile();
            }
            if area[2] <= 0. || area[3] <= 0. {
                return;
            }
            let target = Rect::new(
                point(bounds.left() + px(area[0]), bounds.top() + px(area[1])),
                size(px(area[2]), px(area[3])),
            );
            // A box outside the content mask, or far outside the window, keeps no raster. The layout mirror places
            // boxes in layers too, which paint without the clips around them. A far box paints again in each
            // frame, so it rasters when it comes near.
            let far = far(&state.geometry.mirror, id, window.viewport_size(), 1.);
            if far || target.intersect(mask).is_empty() {
                match far {
                    true => window.paint_volatile(),
                    false => window.cull(),
                }
                if let Some((_, old)) = state.rasters.svgs.remove(&id)
                    && Arc::strong_count(&old) == 1
                {
                    window.release_image(old);
                }
                return;
            }
            let moved = area != [0., 0., width, height];
            // A transform that shows the box smaller gets fewer pixels. A raster with up to twice the pixels it
            // shows stays, so an animated transform doesn't draw it again in each frame.
            let shown = window.transform_scale().clamp(1. / 8., 1.);
            let kept = state
                .rasters
                .svgs
                .get(&id)
                .map(|(_, image)| image.size().width as f32 / (area[2] * scale).max(1.));
            let ratio = match kept {
                Some(kept) if kept >= shown && kept <= shown * 2. && kept < 1. => kept,
                _ => (shown * 8.).ceil() / 8.,
            };
            let raster = [area[2], area[3]]
                .map(|extent| (extent * scale * ratio).ceil().clamp(1., 4096.) as u32);
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
                        .map_or_else(|| super::super::raster::svg::svg_tree(&source), Ok)
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
            drop(store);
            if let Some(image) = image {
                window.draw_image(target, target, renderer::Corners::default(), image, false);
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

// Whether the box of `id` in the layout mirror is more than `margin` window sizes outside the window.
pub(crate) fn far(
    mirror: &crate::dom::LayoutMirror,
    id: usize,
    window: renderer::Size<renderer::Pixels>,
    margin: f32,
) -> bool {
    let [width, height] = [f32::from(window.width), f32::from(window.height)];
    mirror.get(&id).is_some_and(|&[x, y, w, h]| {
        x + w < -width * margin
            || y + h < -height * margin
            || x > width * (1. + margin)
            || y > height * (1. + margin)
    })
}
