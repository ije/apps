use crate::style;
use render::{AnyElement, Bounds, Pixels, Point, Window, point, prelude::*, px};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

/// A box turned a quarter turn for vertical writing modes.
pub(super) struct Turned {
    pub(super) child: AnyElement,
    pub(super) clockwise: bool,
    pub(super) block: f32,
    pub(super) inline: f32,
}

impl Turned {
    fn matrix(&self, origin: Point<Pixels>) -> super::effects::Matrix {
        let [x, y] = [f32::from(origin.x), f32::from(origin.y)];
        // Clockwise maps (u, v) to (block - v, u); counterclockwise to (v, inline - u).
        let mut m = super::effects::IDENTITY;
        if self.clockwise {
            [m[0], m[1], m[4], m[5]] = [0., 1., -1., 0.];
            [m[12], m[13]] = [x + self.block + y, y - x];
        } else {
            [m[0], m[1], m[4], m[5]] = [0., -1., 1., 0.];
            [m[12], m[13]] = [x - y, y + self.inline + x];
        }
        m
    }
}

impl IntoElement for Turned {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl render::Element for Turned {
    type RequestLayoutState = ();
    type PrepaintState = super::effects::Matrix;
    fn request_layout(&mut self, window: &mut Window) -> (render::LayoutId, ()) {
        let child = self.child.request_layout(window);
        let mut style = render::Style {
            position: render::Position::Absolute,
            ..Default::default()
        };
        style.size.width = px(self.block).into();
        style.size.height = px(self.inline).into();
        (window.request_layout(&style, [child]), ())
    }
    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
    ) -> super::effects::Matrix {
        let matrix = self.matrix(bounds.origin);
        window.with_hit_transform(matrix, |window| self.child.prepaint(window));
        matrix
    }
    fn paint(
        &mut self,
        _: Bounds<Pixels>,
        _: &mut (),
        matrix: &mut super::effects::Matrix,
        window: &mut Window,
    ) {
        let effect = render::PaintEffect {
            transform: Some(*matrix),
            ..Default::default()
        };
        window.paint_effect(effect, |window| self.child.paint(window));
    }
}

/// Paints a child through an effect layer.
pub(super) struct Effected {
    pub(super) effect: render::PaintEffect,
    pub(super) child: AnyElement,
}

impl IntoElement for Effected {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl render::Element for Effected {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn request_layout(&mut self, window: &mut Window) -> (render::LayoutId, ()) {
        (self.child.request_layout(window), ())
    }
    fn prepaint(&mut self, _: Bounds<Pixels>, _: &mut (), window: &mut Window) {
        self.child.prepaint(window);
    }
    fn paint(&mut self, _: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window) {
        window.paint_effect(self.effect.clone(), |window| self.child.paint(window));
    }
}

pub(super) struct DomBox {
    pub(super) inner: Option<render::Div>,
    pub(super) anchor: Option<([bool; 2], bool, [f32; 2])>,
    pub(super) id: usize,
    pub(super) ids: Rc<super::effects::Ids>,
    pub(super) view: Rc<super::ViewData>,
    /// The scroll container, its borders, and the containing block of a sticky box.
    pub(super) sticky: Option<(render::ScrollHandle, [f32; 4], Option<usize>)>,
    pub(super) fixed: bool,
    pub(super) inset: [Option<f32>; 4],
    pub(super) style: Rc<style::Style>,
    /// The group opacity, when the box paints through an effect layer.
    pub(super) effects: Option<f32>,
    /// The parent's perspective, and the parent.
    pub(super) perspective: Option<((f32, String), usize)>,
    pub(super) units: style::Lengths,
    /// The subtree revision to record with the bounds.
    pub(super) revision: u64,
    /// The horizontal padding and borders.
    pub(super) sides: f32,
    /// The margins after collapsing.
    pub(super) margin: [f32; 4],
    /// The size of the grid cell the box fills, before layout snaps it to pixels.
    pub(super) cell: Option<[f32; 2]>,
}

impl IntoElement for DomBox {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl render::Element for DomBox {
    type RequestLayoutState = AnyElement;
    type PrepaintState = (Bounds<Pixels>, Option<super::effects::Matrix>);
    fn request_layout(&mut self, window: &mut Window) -> (render::LayoutId, AnyElement) {
        let mut inner = self.inner.take().unwrap().into_any_element();
        (inner.request_layout(window), inner)
    }
    fn prepaint(
        &mut self,
        mut bounds: Bounds<Pixels>,
        inner: &mut AnyElement,
        window: &mut Window,
    ) -> Self::PrepaintState {
        let mut offset = point(px(0.), px(0.));
        if let Some((axes, block, margin)) = self.anchor
            && let Some(anchor) = {
                let view = &*self.view;
                let host = view.host.borrow();
                host.dom
                    .get(&view.id)
                    .and_then(|dom| dom.anchors.get(&self.id).copied())
            }
        {
            if axes[0] && !block {
                offset.x = anchor.left() + px(margin[0]) - bounds.left();
            }
            if axes[1] {
                offset.y = if block { anchor.bottom() } else { anchor.top() } + px(margin[1])
                    - bounds.top();
            }
            bounds.origin += offset;
        }
        if let Some((scroll, border, parent)) = &self.sticky {
            let mut viewport = scroll.bounds();
            viewport.origin += point(px(border[3]), px(border[0]));
            viewport.size.width -= px(border[1] + border[3]);
            viewport.size.height -= px(border[0] + border[2]);
            if let Some(left) = self.inset[3] {
                offset.x = (viewport.left() + px(left) - bounds.left()).max(px(0.));
            }
            if let Some(right) = self.inset[1] {
                offset.x = offset.x.min(viewport.right() - px(right) - bounds.right());
            }
            if let Some(top) = self.inset[0] {
                offset.y = (viewport.top() + px(top) - bounds.top()).max(px(0.));
            }
            if let Some(bottom) = self.inset[2] {
                offset.y = offset
                    .y
                    .min(viewport.bottom() - px(bottom) - bounds.bottom());
            }
            // A sticky box never leaves its containing block, which was laid out first.
            if let Some([x, y, width, height]) = parent.and_then(|parent| {
                let view = &*self.view;
                let host = view.host.borrow();
                host.dom.get(&view.id)?.bounds.get(&parent).copied()
            }) {
                offset.x = offset
                    .x
                    .min(px(x + width) - bounds.right())
                    .max(px(x) - bounds.left());
                offset.y = offset
                    .y
                    .min(px(y + height) - bounds.bottom())
                    .max(px(y) - bounds.top());
            }
            bounds.origin += offset;
        }
        if self.fixed {
            let viewport = window.viewport_size();
            if let Some(left) = self.inset[3] {
                offset.x = px(left) - bounds.left();
            } else if let Some(right) = self.inset[1] {
                offset.x = viewport.width - px(right) - bounds.right();
            }
            if let Some(top) = self.inset[0] {
                offset.y = px(top) - bounds.top();
            } else if let Some(bottom) = self.inset[2] {
                offset.y = viewport.height - px(bottom) - bounds.bottom();
            }
            bounds.origin += offset;
        }
        let perspective = self
            .perspective
            .as_ref()
            .and_then(|((distance, origin), parent)| {
                let view = &*self.view;
                let host = view.host.borrow();
                let [x, y, width, height] = *host.dom.get(&view.id)?.bounds.get(parent)?;
                let [ox, oy, _] = super::effects::origin(origin, &self.units, [width, height]);
                Some((*distance, [x + ox, y + oy]))
            });
        let mut matrix = super::effects::transform(&self.style, &self.units, bounds, perspective);
        // Plain translations move the box instead of painting it through a layer.
        if let Some([x, y]) = matrix.as_ref().and_then(super::effects::translation) {
            let translation = point(px(x), px(y));
            offset += translation;
            bounds.origin += translation;
            matrix = None;
        }
        let mut prepaint = |window: &mut Window| {
            let window_bounds = window.transformed_bounds(bounds);
            let view = &*self.view;
            {
                let mut host = view.host.borrow_mut();
                let dom = host.dom.entry(view.id).or_default();
                if let Some(axes) = dom.flexed_grids.get(&self.id)
                    && axes
                        .iter()
                        .zip([window_bounds.size.width, window_bounds.size.height])
                        .any(|(axis, size)| {
                            axis.is_some_and(|(used, extent)| {
                                !((f32::from(size) - extent - used).abs() <= 0.5)
                            })
                        })
                {
                    window.refresh();
                }
                dom.bounds.insert(
                    self.id,
                    [
                        window_bounds.origin.x.into(),
                        window_bounds.origin.y.into(),
                        window_bounds.size.width.into(),
                        window_bounds.size.height.into(),
                    ],
                );
                let cull = &mut dom.cull;
                let width = f32::from(bounds.size.width);
                if cull.placed.len() <= self.id {
                    cull.placed.resize(self.id + 1, Default::default());
                }
                let [top, right, bottom, left] = self.margin;
                cull.placed[self.id] = (
                    self.revision,
                    self.cell.unwrap_or([
                        (width + right + left).max(0.),
                        f32::from(bounds.size.height) + top + bottom,
                    ]),
                    self.margin,
                );
                cull.widths.insert(self.id, width - self.sides);
            }
            if let Some((browser, _)) = view.browsers.borrow().get(&self.id) {
                let _ = browser.set_bounds([
                    f32::from(window_bounds.origin.x) as f64,
                    f32::from(window_bounds.origin.y) as f64,
                    f32::from(window_bounds.size.width) as f64,
                    f32::from(window_bounds.size.height) as f64,
                ]);
            }
            window.with_element_offset(offset, |window| {
                inner.prepaint(window);
            });
        };
        match matrix {
            Some(matrix) => window.with_hit_transform(matrix, &mut prepaint),
            None => prepaint(window),
        }
        (bounds, matrix)
    }
    fn paint(
        &mut self,
        _: Bounds<Pixels>,
        inner: &mut AnyElement,
        (bounds, matrix): &mut Self::PrepaintState,
        window: &mut Window,
    ) {
        if matrix.is_none() && self.effects.is_none() {
            return inner.paint(window);
        }
        let bounds = *bounds;
        if let Some(matrix) = matrix
            && self
                .style
                .get("backface-visibility")
                .is_some_and(|value| value == "hidden")
            && super::effects::backface(matrix, bounds)
        {
            return;
        }
        let (clip, mut layers) = self.effects(bounds, *matrix, window);
        let mut paint = |window: &mut Window| match &clip {
            Some(clip) => window.with_content_mask(Some(*clip), |window| inner.paint(window)),
            None => inner.paint(window),
        };
        nest_effects(&mut layers, window, &mut paint);
    }
}

impl DomBox {
    /// The rounded `clip-path` and the effect layers of the box, innermost first.
    fn effects(
        &self,
        bounds: Bounds<Pixels>,
        transform: Option<super::effects::Matrix>,
        window: &mut Window,
    ) -> (
        Option<render::ContentMask<Pixels>>,
        Vec<render::PaintEffect>,
    ) {
        use super::effects::{self, Clip};
        let style = &self.style;
        let units = self.units;
        let value = |name: &str| {
            style
                .get(name)
                .or_else(|| style.get(&format!("-webkit-{name}")))
                .filter(|value| !matches!(value.as_str(), "none" | "normal" | "auto"))
        };
        let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
        let view = &*self.view;
        let mut host = view.host.borrow_mut();
        let empty = crate::dom::Document::default();
        let host = &mut *host;
        let doc = host.pages.get(&view.id).map_or(&empty, |page| &page.doc);
        let mut layers = value("filter")
            .map(|value| effects::filters(value, &units, doc, &self.ids))
            .unwrap_or_default();
        let mut clip = None;
        let mut shapes = None;
        match value("clip-path")
            .and_then(|value| effects::clip(value, &units, size, doc, &self.ids))
        {
            Some(Clip::Rounded(rect, radii)) => {
                clip = Some(render::ContentMask::rounded(
                    Bounds::new(
                        bounds.origin + point(px(rect[0]), px(rect[1])),
                        render::size(px(rect[2]), px(rect[3])),
                    ),
                    effects::corners(radii),
                ))
            }
            Some(Clip::Shape(shape)) => shapes = Some(shape),
            None => {}
        }
        let base = host.dom.get(&view.id).and_then(|state| state.base.clone());
        let mask_layers = mask_svg(style, units, size, base.as_ref(), &host.assets);
        let mask = (shapes.is_some() || mask_layers.is_some()).then(|| {
            let [width, height] = size;
            let source = match (shapes, mask_layers) {
                (Some(shape), Some(layers)) => format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\"><defs><clipPath id=\"h2o-clip\">{shape}</clipPath></defs><g clip-path=\"url(#h2o-clip)\">{layers}</g></svg>"
                ),
                (Some(shape), None) => format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">{shape}</svg>"
                ),
                (None, layers) => format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">{}</svg>",
                    layers.unwrap_or_default()
                ),
            };
            let scale = window.scale_factor();
            let raster = [width, height].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
            let masks = &mut host.dom.entry(view.id).or_default().masks;
            if masks.get(&self.id).is_none_or(|(old, image)| {
                *old != source
                    || u32::from(image.size().width) != raster[0]
                    || u32::from(image.size().height) != raster[1]
            }) {
                match super::canvas::svg(&source, raster[0], raster[1], None) {
                    Ok(image) => {
                        if let Some((_, old)) = masks.insert(self.id, (source, image)) {
                            window.drop_image(old);
                        }
                    }
                    Err(error) => eprintln!("apps: mask: {error}"),
                }
            }
            masks.get(&self.id).map(|(_, image)| (image.clone(), bounds))
        });
        let radii = || {
            effects::corners(
                [
                    "border-top-left-radius",
                    "border-top-right-radius",
                    "border-bottom-right-radius",
                    "border-bottom-left-radius",
                ]
                .map(|name| {
                    style
                        .get(name)
                        .and_then(|value| units.resolve(value, size[0]))
                        .unwrap_or(0.)
                        .max(0.)
                }),
            )
        };
        let backdrop = value("backdrop-filter")
            .and_then(|value| effects::backdrop(value, &units))
            .map(|(blur, color_matrix)| render::Backdrop {
                bounds,
                corner_radii: radii(),
                blur,
                color_matrix,
            });
        let outer = render::PaintEffect {
            transform,
            opacity: self.effects.unwrap_or(1.),
            blend_mode: value("mix-blend-mode").map_or(render::BlendMode::Normal, |value| {
                effects::blend_mode(value)
            }),
            mask: mask.flatten(),
            backdrop,
            ..Default::default()
        };
        let isolate = style
            .get("isolation")
            .is_some_and(|value| value == "isolate");
        let needed = outer.transform.is_some()
            || outer.opacity < 1.
            || outer.blend_mode != render::BlendMode::Normal
            || outer.mask.is_some()
            || outer.backdrop.is_some()
            || isolate;
        if needed {
            // One filter function and no clip can share the outer layer.
            if layers.len() == 1 && clip.is_none() {
                let filter = layers.pop().unwrap();
                layers.push(render::PaintEffect {
                    color_matrix: filter.color_matrix,
                    blur: filter.blur,
                    shadow: filter.shadow,
                    ..outer
                });
            } else {
                layers.push(outer);
            }
        }
        (clip, layers)
    }
}

/// Paints through the effect layers, the last one outermost.
fn nest_effects(
    layers: &mut Vec<render::PaintEffect>,
    window: &mut Window,
    paint: &mut dyn FnMut(&mut Window),
) {
    match layers.pop() {
        Some(effect) => window.paint_effect(effect, |window| nest_effects(layers, window, paint)),
        None => paint(window),
    }
}

/// The layers of `mask-image` as SVG content covering a box, the first layer on top.
fn mask_svg(
    style: &style::Style,
    units: style::Lengths,
    [width, height]: [f32; 2],
    base: Option<&url::Url>,
    assets: &HashMap<String, std::path::PathBuf>,
) -> Option<String> {
    let get = |name: &str| {
        style
            .get(name)
            .or_else(|| style.get(&format!("-webkit-{name}")))
            .map_or("", String::as_str)
    };
    let images = get("mask-image");
    if images.is_empty() || images == "none" {
        return None;
    }
    let list = |name: &str| {
        style::components(get(name), ',')
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (sizes, positions, repeats) = (
        list("mask-size"),
        list("mask-position"),
        list("mask-repeat"),
    );
    let pick = |values: &[String], index: usize| {
        values
            .get(index % values.len().max(1))
            .cloned()
            .unwrap_or_default()
    };
    let mut content = String::new();
    for (index, image) in style::components(images, ',').into_iter().enumerate().rev() {
        let image = image.trim();
        if let Some(source) = image.strip_prefix("url(").and_then(|v| v.strip_suffix(')')) {
            let Some(path) = base
                .and_then(|base| base.join(source.trim().trim_matches(['"', '\''])).ok())
                .and_then(|url| match url.scheme() {
                    "file" => url.to_file_path().ok(),
                    "apps" => assets.get(url.path().trim_start_matches('/')).cloned(),
                    _ => None,
                })
            else {
                continue;
            };
            let aspect = match pick(&sizes, index).as_str() {
                "cover" => "xMidYMid slice",
                "contain" => "xMidYMid meet",
                _ => "none",
            };
            content.push_str(&format!(
                "<image href=\"{}\" width=\"{width}\" height=\"{height}\" preserveAspectRatio=\"{aspect}\"/>",
                crate::dom::escape(&path.to_string_lossy())
            ));
            continue;
        }
        let Some((kind, body)) = image
            .split_once('(')
            .filter(|(kind, _)| kind.ends_with("-gradient"))
            .and_then(|(kind, body)| Some((kind, body.strip_suffix(')')?)))
        else {
            continue;
        };
        let size = pick(&sizes, index);
        let position = pick(&positions, index);
        let repeat = pick(&repeats, index);
        if let Some(svg) = super::effects::gradient_svg(
            kind,
            body,
            if size.is_empty() { "auto" } else { &size },
            if position.is_empty() {
                "0% 0%"
            } else {
                &position
            },
            if repeat.is_empty() { "repeat" } else { &repeat },
            units,
            width,
            height,
        ) && let Some(start) = svg.find('>')
            && let Some(end) = svg.rfind("</svg>")
        {
            // Each layer is its own SVG document, so its `id`s must not collide.
            content.push_str(
                &svg[start + 1..end]
                    .replace("id=\"", &format!("id=\"m{index}"))
                    .replace("url(#", &format!("url(#m{index}")),
            );
        }
    }
    (!content.is_empty()).then_some(content)
}

pub(super) type PaintLayers = Rc<
    RefCell<
        Vec<(
            i32,
            Rc<RefCell<AnyElement>>,
            render::ContentMask<Pixels>,
            Rc<[usize]>,
        )>,
    >,
>;

pub(super) struct Stacked {
    pub(super) inner: Option<AnyElement>,
    pub(super) layers: PaintLayers,
    pub(super) order: i32,
    pub(super) path: Rc<[usize]>,
}

impl IntoElement for Stacked {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl render::Element for Stacked {
    type RequestLayoutState = Rc<RefCell<AnyElement>>;
    type PrepaintState = ();
    fn request_layout(
        &mut self,
        window: &mut Window,
    ) -> (render::LayoutId, Self::RequestLayoutState) {
        let mut inner = self.inner.take().unwrap();
        let id = inner.request_layout(window);
        (id, Rc::new(RefCell::new(inner)))
    }
    fn prepaint(
        &mut self,
        _: Bounds<Pixels>,
        inner: &mut Self::RequestLayoutState,
        window: &mut Window,
    ) {
        inner.borrow_mut().prepaint(window);
        self.layers.borrow_mut().push((
            self.order,
            inner.clone(),
            window.content_mask(),
            self.path.clone(),
        ));
    }
    fn paint(
        &mut self,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut (),
        _: &mut Window,
    ) {
    }
}

pub(super) fn paint_layers(layers: PaintLayers, negative: bool) -> impl IntoElement {
    render::canvas(
        |_, _| (),
        move |_, _, window| {
            let mut layers = layers.borrow_mut();
            layers.sort_by_key(|(order, ..)| *order);
            for (_, inner, mask, _) in layers.iter().filter(|(order, ..)| (*order < 0) == negative)
            {
                window.with_content_mask(Some(*mask), |window| inner.borrow_mut().paint(window));
            }
        },
    )
    .absolute()
    .w(px(0.))
    .h(px(0.))
}
