use crate::style;
use crate::style::Prop;
use renderer::{AnyElement, Bounds, Pixels, Point, Window, point, prelude::*, px};
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

impl renderer::Element for Turned {
    type RequestLayoutState = ();
    type PrepaintState = super::effects::Matrix;
    fn request_layout(&mut self, window: &mut Window) -> (renderer::LayoutId, ()) {
        let child = self.child.request_layout(window);
        let mut style = renderer::Style {
            position: renderer::Position::Absolute,
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
        let effect = renderer::PaintEffect {
            transform: Some(*matrix),
            ..Default::default()
        };
        window.paint_effect(effect, |window| self.child.paint(window));
    }
}

/// Paints a child through an effect layer.
pub(super) struct Effected {
    pub(super) effect: renderer::PaintEffect,
    pub(super) child: AnyElement,
}

impl IntoElement for Effected {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl renderer::Element for Effected {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn request_layout(&mut self, window: &mut Window) -> (renderer::LayoutId, ()) {
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
    pub(super) inner: Option<renderer::Div>,
    pub(super) anchor: Option<([bool; 2], bool, [f32; 2])>,
    pub(super) id: usize,
    pub(super) ids: Rc<super::effects::Ids>,
    pub(super) view: Rc<super::ViewData>,
    /// The scroll container, its borders, and the containing block of a sticky box.
    pub(super) sticky: Option<(renderer::ScrollHandle, [f32; 4], Option<usize>)>,
    pub(super) fixed: bool,
    pub(super) block: Option<[f32; 4]>,
    pub(super) container: Option<usize>,
    pub(super) inset: [Option<f32>; 4],
    pub(super) style: Rc<style::Style>,
    /// The group opacity, when the box paints through an effect layer.
    pub(super) effects: Option<f32>,
    /// The parent's perspective, and the parent.
    pub(super) perspective: Option<((f32, String), usize)>,
    pub(super) units: style::Lengths,
    /// The subtree revision to record with the bounds.
    pub(super) revision: u64,
    /// The horizontal and vertical padding and borders.
    pub(super) sides: [f32; 2],
    /// The margins after collapsing.
    pub(super) margin: [f32; 4],
    /// The size of the grid cell the box fills, before layout snaps it to pixels.
    pub(super) cell: Option<[f32; 2]>,
    /// The grid that aligns the box's baseline. Prepaint records the box's baselines and draws the frame again
    /// when they change.
    pub(super) grid: Option<usize>,
    /// For a table, its rows with their cells, then its row groups with their rows. They have no box of their
    /// own, so their bounds are the union of their members'.
    pub(super) parts: Rc<[(usize, Vec<usize>)]>,
    /// For a cell with collapsed borders, the half borders it shares with its neighbors, outside its box.
    pub(super) shared: Option<[f32; 4]>,
    /// For an inline box, its height on one line and the leading above and below its font's content area,
    /// which browsers leave out of its rectangle.
    pub(super) leading: Option<[f32; 3]>,
    /// The box shows in place of another, like the chosen option of a closed select, and reports no box.
    pub(super) boxless: bool,
    /// For an inline box, its first and last text and its top and bottom padding and border. Its rectangle
    /// spans its text's, whatever else its line holds.
    pub(super) text: Option<([usize; 2], [f32; 2])>,
}

impl IntoElement for DomBox {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl renderer::Element for DomBox {
    type RequestLayoutState = (renderer::LayoutId, AnyElement);
    type PrepaintState = (Bounds<Pixels>, Option<super::effects::Matrix>);
    fn request_layout(
        &mut self,
        window: &mut Window,
    ) -> (renderer::LayoutId, Self::RequestLayoutState) {
        let mut inner = self.inner.take().unwrap().into_any_element();
        let id = inner.request_layout(window);
        (id, (id, inner))
    }
    fn prepaint(
        &mut self,
        _: Bounds<Pixels>,
        (id, inner): &mut Self::RequestLayoutState,
        window: &mut Window,
    ) -> Self::PrepaintState {
        // Positions keep layout units like Chrome; the inner element paints at snapped bounds.
        let mut bounds = window.exact_layout_bounds(*id);
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
        if let Some(container) = self.container
            && let Some(block) = {
                let view = &*self.view;
                let host = view.host.borrow();
                host.dom
                    .get(&view.id)
                    .and_then(|dom| dom.blocks.get(&container).copied())
            }
        {
            let [top, right, bottom, left] = self.inset;
            let [margin_top, margin_right, margin_bottom, margin_left] = self.margin.map(px);
            let mut shift = point(px(0.), px(0.));
            if let Some(left) = left {
                shift.x = block.left() + px(left) + margin_left - bounds.left();
            } else if let Some(right) = right {
                shift.x = block.right() - px(right) - margin_right - bounds.right();
            }
            if let Some(top) = top {
                shift.y = block.top() + px(top) + margin_top - bounds.top();
            } else if let Some(bottom) = bottom {
                shift.y = block.bottom() - px(bottom) - margin_bottom - bounds.bottom();
            }
            offset += shift;
            bounds.origin += shift;
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
        if let Some([top, right, bottom, left]) = self.block {
            let view = &*self.view;
            let mut host = view.host.borrow_mut();
            let padding = Bounds::new(
                bounds.origin + point(px(left), px(top)),
                renderer::size(
                    bounds.size.width - px(left + right),
                    bounds.size.height - px(top + bottom),
                ),
            );
            host.dom
                .entry(view.id)
                .or_default()
                .blocks
                .insert(self.id, padding);
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
                // WARNING: grid tracks are sized before transforms, so they compare with the untransformed box.
                if let Some(axes) = dom.flexed_grids.get(&self.id)
                    && axes
                        .iter()
                        .zip([bounds.size.width, bounds.size.height])
                        .any(|(axis, size)| {
                            axis.is_some_and(|(used, extent)| {
                                let gap = (f32::from(size) - extent - used).abs();
                                gap.is_nan() || gap > 0.5
                            })
                        })
                {
                    window.redraw();
                }
                let [mut top, right, mut bottom, left] = self.shared.unwrap_or_default();
                if let Some([line, above, below]) = self.leading
                    && (f32::from(window_bounds.size.height) - line).abs() < 0.5
                {
                    (top, bottom) = (above, below);
                }
                dom.bounds.insert(
                    self.id,
                    if self.boxless {
                        [0.; 4]
                    } else {
                        [
                            f32::from(window_bounds.origin.x) + left,
                            f32::from(window_bounds.origin.y) + top,
                            f32::from(window_bounds.size.width) - left - right,
                            f32::from(window_bounds.size.height) - top - bottom,
                        ]
                    },
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
                cull.widths.insert(
                    self.id,
                    [
                        width - self.sides[0],
                        f32::from(bounds.size.height) - self.sides[1],
                    ],
                );
                if let Some(grid) = self.grid {
                    let baselines = window
                        .layout_baselines(*id)
                        .map(|value| value.map(f32::from));
                    if dom.baselines.insert(self.id, baselines) != Some(baselines) {
                        dom.grids.remove(&grid);
                        window.redraw();
                    }
                }
            }
            if let Some((browser, _)) = view.browsers.borrow().get(&self.id)
                && let Some((set, _)) = view.frames.borrow_mut().get_mut(&self.id)
            {
                let bounds = [
                    window_bounds.origin.x,
                    window_bounds.origin.y,
                    window_bounds.size.width,
                    window_bounds.size.height,
                ]
                .map(|value| f32::from(value) as f64);
                if *set != bounds {
                    *set = bounds;
                    let _ = browser.set_bounds(bounds);
                }
            }
            window.with_element_offset(offset, |window| {
                inner.prepaint(window);
            });
            if let Some(([first, last], [above, below])) = self.text {
                let mut host = view.host.borrow_mut();
                let bounds = &mut host.dom.entry(view.id).or_default().bounds;
                if let (Some(&[x, _, width, _]), Some(&[_, top, ..]), Some(&[_, y, _, height])) =
                    (bounds.get(&self.id), bounds.get(&first), bounds.get(&last))
                    && width > 0.
                {
                    bounds.insert(
                        self.id,
                        [x, top - above, width, y + height - top + above + below],
                    );
                }
            }
            if !self.parts.is_empty() {
                let mut host = view.host.borrow_mut();
                let bounds = &mut host.dom.entry(view.id).or_default().bounds;
                for (part, members) in self.parts.iter() {
                    let union = members
                        .iter()
                        .filter_map(|member| bounds.get(member).copied())
                        .reduce(|[x, y, w, h], [a, b, c, d]| {
                            let [left, top] = [x.min(a), y.min(b)];
                            [
                                left,
                                top,
                                (x + w).max(a + c) - left,
                                (y + h).max(b + d) - top,
                            ]
                        });
                    if let Some(union) = union {
                        bounds.insert(*part, union);
                    }
                }
            }
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
        (_, inner): &mut Self::RequestLayoutState,
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
                .get(Prop::BACKFACE_VISIBILITY)
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
        Option<renderer::ContentMask<Pixels>>,
        Vec<renderer::PaintEffect>,
    ) {
        use super::effects::{self, Clip};
        let style = &self.style;
        let units = self.units;
        let value = |props: &[Prop]| {
            props
                .iter()
                .find_map(|&prop| style.get(prop))
                .filter(|value| !matches!(*value, "none" | "normal" | "auto"))
        };
        let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
        let view = &*self.view;
        let mut host = view.host.borrow_mut();
        let empty = crate::dom::Document::default();
        let host = &mut *host;
        let doc = host.pages.get(&view.id).map_or(&empty, |page| &page.doc);
        let mut layers = value(&[Prop::FILTER, Prop::WEBKIT_FILTER])
            .map(|value| effects::filters(value, &units, doc, &self.ids))
            .unwrap_or_default();
        let mut clip = None;
        let mut shapes = None;
        match value(&[Prop::CLIP_PATH, Prop::WEBKIT_CLIP_PATH])
            .and_then(|value| effects::clip(value, &units, size, doc, &self.ids))
        {
            Some(Clip::Rounded(rect, radii)) => {
                clip = Some(renderer::ContentMask::rounded(
                    Bounds::new(
                        bounds.origin + point(px(rect[0]), px(rect[1])),
                        renderer::size(px(rect[2]), px(rect[3])),
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
        let backdrop = value(&[Prop::BACKDROP_FILTER, Prop::WEBKIT_BACKDROP_FILTER])
            .and_then(|value| effects::backdrop(value, &units))
            .map(|(blur, color_matrix)| renderer::Backdrop {
                bounds,
                corner_radii: effects::radii(style, &units, || size).resolve(bounds.size),
                blur,
                color_matrix,
            });
        let outer = renderer::PaintEffect {
            transform,
            opacity: self.effects.unwrap_or(1.),
            blend_mode: value(&[Prop::MIX_BLEND_MODE])
                .map_or(renderer::BlendMode::Normal, |value| {
                    effects::blend_mode(value)
                }),
            mask: mask.flatten(),
            backdrop,
            ..Default::default()
        };
        let isolate = style
            .get(Prop::ISOLATION)
            .is_some_and(|value| value == "isolate");
        let needed = outer.transform.is_some()
            || outer.opacity < 1.
            || outer.blend_mode != renderer::BlendMode::Normal
            || outer.mask.is_some()
            || outer.backdrop.is_some()
            || isolate;
        if needed {
            // One filter function and no clip can share the outer layer.
            if layers.len() == 1 && clip.is_none() {
                let filter = layers.pop().unwrap();
                layers.push(renderer::PaintEffect {
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
    layers: &mut Vec<renderer::PaintEffect>,
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
    let get =
        |[prop, webkit]: [Prop; 2]| style.get(prop).or_else(|| style.get(webkit)).unwrap_or("");
    let images = get([Prop::MASK_IMAGE, Prop::WEBKIT_MASK_IMAGE]);
    if images.is_empty() || images == "none" {
        return None;
    }
    let list = |props| {
        style::components(get(props), ',')
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (sizes, positions, repeats) = (
        list([Prop::MASK_SIZE, Prop::WEBKIT_MASK_SIZE]),
        list([Prop::MASK_POSITION, Prop::WEBKIT_MASK_POSITION]),
        list([Prop::MASK_REPEAT, Prop::WEBKIT_MASK_REPEAT]),
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
            renderer::ContentMask<Pixels>,
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

impl renderer::Element for Stacked {
    type RequestLayoutState = Rc<RefCell<AnyElement>>;
    type PrepaintState = ();
    fn request_layout(
        &mut self,
        window: &mut Window,
    ) -> (renderer::LayoutId, Self::RequestLayoutState) {
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
    renderer::canvas(
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
