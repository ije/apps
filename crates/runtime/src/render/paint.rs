use crate::style;
use crate::style::Prop;
use renderer::{Bounds, Child, Hooks, NodeCx, Pixels, Point, Window, point, px};
use std::{cell::Cell, collections::HashMap, rc::Rc};

pub(super) struct Turned {
    clockwise: bool,
    block: f32,
    inline: f32,
    matrix: Cell<super::effects::Matrix>,
}

impl Turned {
    pub(super) fn node(child: Child, clockwise: bool, block: f32, inline: f32) -> renderer::Node {
        let mut style = renderer::Style {
            position: renderer::Position::Absolute,
            ..Default::default()
        };
        style.size.width = px(block).into();
        style.size.height = px(inline).into();
        let mut node = renderer::node().child(child).hooks(Turned {
            clockwise,
            block,
            inline,
            matrix: Cell::new(super::effects::IDENTITY),
        });
        *node.style() = style;
        node
    }

    fn matrix(&self, origin: Point<Pixels>) -> super::effects::Matrix {
        let [x, y] = [f32::from(origin.x), f32::from(origin.y)];
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

impl Hooks for Turned {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let matrix = self.matrix(cx.bounds.origin);
        self.matrix.set(matrix);
        cx.window.with_hit_transform(matrix, inner);
    }
    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let effect = renderer::PaintEffect {
            transform: Some(self.matrix.get()),
            ..Default::default()
        };
        cx.window.paint_effect(effect, inner);
    }
}

pub(super) struct Transformed {
    pub(super) style: Rc<style::Style>,
    pub(super) units: style::Lengths,
    pub(super) matrix: Cell<Option<super::effects::Matrix>>,
}

impl Hooks for Transformed {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let matrix = super::effects::transform(&self.style, &self.units, cx.bounds, None);
        self.matrix.set(matrix);
        match matrix {
            Some(matrix) => cx.window.with_hit_transform(matrix, inner),
            None => inner(cx.window),
        }
    }
    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        match self.matrix.get() {
            Some(matrix) => {
                let effect = renderer::PaintEffect {
                    transform: Some(matrix),
                    ..Default::default()
                };
                cx.window.paint_effect(effect, inner);
            }
            None => inner(cx.window),
        }
    }
}

pub(super) struct Effected {
    pub(super) effect: renderer::PaintEffect,
    pub(super) backdrop: Option<renderer::Radii<renderer::Length>>,
}

impl Hooks for Effected {
    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let mut effect = self.effect.clone();
        if let Some(backdrop) = effect.backdrop.as_mut()
            && let Some(radii) = &self.backdrop
        {
            backdrop.bounds = cx.bounds;
            backdrop.corner_radii = radii.resolve(cx.bounds.size);
        }
        cx.window.paint_effect(effect, inner);
    }
}

pub(super) struct Sticky {
    pub(super) scroll: renderer::ScrollHandle,
    pub(super) border: [f32; 4],
    pub(super) block: Option<(usize, [f32; 4])>,
}

pub(super) struct DomBox {
    pub(super) anchor: Option<([bool; 2], bool, [f32; 2])>,
    pub(super) id: usize,
    pub(super) ids: Rc<super::effects::Ids>,
    pub(super) view: Rc<super::PageState>,
    pub(super) sticky: Option<Sticky>,
    pub(super) fixed: bool,
    pub(super) block: Option<[f32; 4]>,
    pub(super) container: Option<usize>,
    pub(super) sized: Option<[f32; 2]>,
    pub(super) inset: [Option<f32>; 4],
    pub(super) style: Rc<style::Style>,
    pub(super) effects: Option<f32>,
    pub(super) composited: bool,
    pub(super) transformed: bool,
    pub(super) perspective: Option<((f32, String), usize)>,
    pub(super) units: style::Lengths,
    pub(super) revision: u64,
    pub(super) sides: [f32; 2],
    pub(super) margin: [f32; 4],
    pub(super) cell: Option<[f32; 2]>,
    pub(super) grid: Option<(usize, bool)>,
    pub(super) parts: Rc<[(usize, Vec<usize>)]>,
    pub(super) shared: Option<[f32; 4]>,
    pub(super) leading: Option<[f32; 3]>,
    pub(super) boxless: bool,
    pub(super) text: Option<([usize; 2], [f32; 2])>,
    pub(super) placed: Cell<(Bounds<Pixels>, Option<super::effects::Matrix>)>,
}

impl Hooks for DomBox {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let (exact, baselines) = (cx.exact, cx.baselines);
        let window = &mut *cx.window;
        let mut bounds = exact;
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
        if let Some(Sticky {
            scroll,
            border,
            block,
        }) = &self.sticky
        {
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
            if let Some([x, y, width, height]) =
                block.and_then(|(parent, [top, right, bottom, left])| {
                    let view = &*self.view;
                    let host = view.host.borrow();
                    let [x, y, width, height] =
                        host.dom.get(&view.id)?.bounds.get(&parent).copied()?;
                    Some([
                        x + left,
                        y + top,
                        width - left - right,
                        height - top - bottom,
                    ])
                })
            {
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
                host.dom.get(&view.id).and_then(|dom| match container {
                    0 => dom.scroll.get(&0).map(|scroll| {
                        let [width, height] = self.units.viewport;
                        Bounds::new(
                            scroll.bounds().origin + scroll.offset() + scroll.stretch(),
                            renderer::size(px(width), px(height)),
                        )
                    }),
                    _ => dom.blocks.get(&container).copied(),
                })
            }
        {
            // WARNING: the box was sized in last frame's block, so a block that changed size draws again.
            if self.sized.is_some_and(|[width, height]| {
                (f32::from(block.size.width) - width).abs() > 0.5
                    || (f32::from(block.size.height) - height).abs() > 0.5
            }) {
                window.redraw();
            }
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
            for axis in 0..2 {
                let [start, end] = if axis == 0 { [3, 1] } else { [0, 2] };
                let auto = |side: usize| self.style.get(style::MARGIN[side + 1]) == Some("auto");
                let margin = |side: usize| {
                    if auto(side) {
                        px(0.)
                    } else {
                        px(self.margin[side])
                    }
                };
                let [extent, origin, size] = if axis == 0 {
                    [viewport.width, bounds.left(), bounds.size.width]
                } else {
                    [viewport.height, bounds.top(), bounds.size.height]
                };
                let at = match (self.inset[start], self.inset[end]) {
                    (Some(first), Some(last)) if auto(start) || auto(end) => {
                        let free = extent - px(first + last) - size - margin(start) - margin(end);
                        px(first)
                            + match (auto(start), auto(end)) {
                                (true, true) if axis == 0 => (free / 2.).max(px(0.)),
                                (true, true) => free / 2.,
                                (true, false) => free,
                                _ => margin(start),
                            }
                    }
                    (Some(first), _) => px(first) + margin(start),
                    (None, Some(last)) => extent - px(last) - margin(end) - size,
                    (None, None) => continue,
                };
                if axis == 0 {
                    offset.x = at - origin;
                } else {
                    offset.y = at - origin;
                }
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
        if self.transformed {
            let matrix = *matrix.get_or_insert(super::effects::IDENTITY);
            self.view
                .composited_transforms
                .borrow_mut()
                .insert(self.id, (bounds, perspective, matrix));
        }
        if !self.transformed
            && let Some([x, y]) = matrix.as_ref().and_then(super::effects::translation)
        {
            let translation = point(px(x), px(y));
            offset += translation;
            bounds.origin += translation;
            matrix = None;
            // WARNING: boxes positioned in this block include the translation through the offset, so the block must too.
            let view = &*self.view;
            if self.block.is_some()
                && let Some(block) = view
                    .host
                    .borrow_mut()
                    .dom
                    .get_mut(&view.id)
                    .and_then(|dom| dom.blocks.get_mut(&self.id))
            {
                block.origin += translation;
            }
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
                let inner = [
                    width - self.sides[0],
                    f32::from(bounds.size.height) - self.sides[1],
                ];
                cull.widths.insert(self.id, inner);
                // WARNING: a container's size starts from an estimate, so queries read the size that layout gave it.
                if dom
                    .containers
                    .get(&self.id)
                    .is_some_and(|size| (size[0] - inner[0]).abs() > 0.5)
                {
                    dom.containers.insert(self.id, inner);
                    window.redraw();
                }
                if let Some((grid, aligned)) = self.grid {
                    let baselines = baselines.map(|value| value.map(f32::from));
                    // WARNING: the grid's own baseline comes from its first item in the same layout, so only aligned
                    // items, which the grid places from their last baselines, need another pass.
                    if dom.baselines.insert(self.id, baselines) != Some(baselines) {
                        dom.grids.remove(&grid);
                        if aligned {
                            window.redraw();
                        }
                    }
                }
            }
            if let Some((browser, _)) = view.browsers.borrow().get(&self.id)
                && let Some((set, ..)) = view.frames.borrow_mut().get_mut(&self.id)
            {
                // WARNING: native views are placed in window points, which the page zoom scales.
                let zoom = window.zoom();
                let bounds = [
                    window_bounds.origin.x,
                    window_bounds.origin.y,
                    window_bounds.size.width,
                    window_bounds.size.height,
                ]
                .map(|value| (f32::from(value) * zoom) as f64);
                if *set != bounds {
                    *set = bounds;
                    let _ = browser.set_bounds(bounds);
                }
            }
            window.with_element_offset(offset, |window| inner(window));
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
            Some(matrix) => window.with_keyed_hit_transform(
                matrix,
                self.transformed.then_some(self.id as u64),
                &mut prepaint,
            ),
            None => prepaint(window),
        }
        self.placed.set((bounds, matrix));
    }
    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let window = &mut *cx.window;
        let (bounds, matrix) = self.placed.get();
        {
            let mut host = self.view.host.borrow_mut();
            let dom = host.dom.entry(self.view.id).or_default();
            if dom.painted.len() <= self.id {
                dom.painted.resize(self.id + 1, 0);
            }
            dom.paint_count += 1;
            dom.painted[self.id] = dom.paint_count;
        }
        if matrix.is_none() && self.effects.is_none() {
            return inner(window);
        }
        if let Some(matrix) = &matrix
            && self
                .style
                .get(Prop::BACKFACE_VISIBILITY)
                .is_some_and(|value| value == "hidden")
            && super::effects::backface(matrix, bounds)
        {
            return;
        }
        let (clip, mut layers) = self.effects(bounds, matrix, window);
        let mut paint = |window: &mut Window| match &clip {
            Some(clip) => window.with_content_mask(Some(*clip), |window| inner(window)),
            None => inner(window),
        };
        nest_effects(&mut layers, window, &mut paint);
    }
}

impl DomBox {
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
        let images = [Prop::MASK_IMAGE, Prop::WEBKIT_MASK_IMAGE]
            .into_iter()
            .find_map(|prop| style.get(prop))
            .filter(|value| !value.is_empty() && *value != "none");
        let mask = (shapes.is_some() || images.is_some()).then(|| {
            // WARNING: building a mask's SVG costs more than drawing the box, so the bitmap is kept by its
            // exact inputs: the mask properties and clip shape, then the box size and units as hex bits.
            let mut inputs = String::new();
            for part in [
                Prop::MASK_IMAGE,
                Prop::WEBKIT_MASK_IMAGE,
                Prop::MASK_SIZE,
                Prop::WEBKIT_MASK_SIZE,
                Prop::MASK_POSITION,
                Prop::WEBKIT_MASK_POSITION,
                Prop::MASK_REPEAT,
                Prop::WEBKIT_MASK_REPEAT,
            ]
            .map(|prop| style.get(prop).unwrap_or(""))
            .into_iter()
            .chain(shapes.as_deref())
            {
                inputs.push_str(part);
                inputs.push('\0');
            }
            super::effects::push_bits(&mut inputs, size, &units);
            let [width, height] = size;
            let scale = window.scale_factor();
            let raster = [width, height].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
            let masks = &mut host.dom.entry(view.id).or_default().masks;
            if masks.get(&self.id).is_none_or(|(old, image)| {
                *old != inputs
                    || image.as_ref().is_some_and(|image| {
                        u32::from(image.size().width) != raster[0]
                            || u32::from(image.size().height) != raster[1]
                    })
            }) {
                // WARNING: scroll areas keep gradient masks that are usually fully opaque, and a resize changes them in every frame, so these skip the SVG.
                if shapes.is_none()
                    && let Some([across, down]) =
                        super::effects::axis_mask(style, &units, size, raster)
                {
                    let opaque = |alpha: &f32| *alpha >= 0.999;
                    let image = (!across.iter().all(opaque) || !down.iter().all(opaque)).then(|| {
                        let mut bytes = vec![0; across.len() * down.len() * 4];
                        for (row, y) in bytes.chunks_exact_mut(across.len() * 4).zip(&down) {
                            for (pixel, x) in row.as_chunks_mut::<4>().0.iter_mut().zip(&across) {
                                pixel[3] = (x * y * 255. + 0.5) as u8;
                            }
                        }
                        std::sync::Arc::new(renderer::RenderImage::new(raster[0], raster[1], bytes))
                    });
                    if let Some((_, Some(old))) = masks.insert(self.id, (inputs, image)) {
                        window.drop_image(old);
                    }
                    return masks
                        .get(&self.id)
                        .and_then(|(_, image)| image.clone().map(|image| (image, bounds)));
                }
                let source = match (shapes, mask_svg(style, units, size, base.as_ref(), &host.assets)) {
                    (Some(shape), Some(layers)) => format!(
                        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\"><defs><clipPath id=\"apps-clip\">{shape}</clipPath></defs><g clip-path=\"url(#apps-clip)\">{layers}</g></svg>"
                    ),
                    (Some(shape), None) => format!(
                        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">{shape}</svg>"
                    ),
                    (None, Some(layers)) => format!(
                        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">{layers}</svg>"
                    ),
                    (None, None) => return None,
                };
                match super::canvas::svg(&source, raster[0], raster[1], None) {
                    Ok(image) => {
                        if let Some((_, Some(old))) = masks.insert(self.id, (inputs, Some(image))) {
                            window.drop_image(old);
                        }
                    }
                    Err(error) => eprintln!("apps: mask: {error}"),
                }
            }
            masks
                .get(&self.id)
                .and_then(|(_, image)| image.clone().map(|image| (image, bounds)))
        });
        let shown = self.effects.unwrap_or(1.) > 0.
            && style
                .get(Prop::VISIBILITY)
                .is_none_or(|value| value == "visible");
        let backdrop = value(&[Prop::BACKDROP_FILTER, Prop::WEBKIT_BACKDROP_FILTER])
            .filter(|_| shown)
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
            key: self.composited.then_some(self.id as u64),
            ..Default::default()
        };
        // WARNING: isolation only contains `mix-blend-mode`. It is no backdrop root, so without blending a group
        // would hide what is behind from a `backdrop-filter` inside.
        let isolate = style
            .get(Prop::ISOLATION)
            .is_some_and(|value| value == "isolate")
            && host
                .dom
                .get(&view.id)
                .and_then(|dom| dom.computed.as_ref())
                .is_none_or(|styles| styles.blended());
        let needed = outer.transform.is_some()
            || outer.opacity < 1.
            || outer.blend_mode != renderer::BlendMode::Normal
            || outer.mask.is_some()
            || outer.backdrop.is_some()
            || isolate
            || self.composited;
        if needed {
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
            let source = source.trim().trim_matches(['"', '\'']);
            let Some(path) = super::image::data_file(source).or_else(|| {
                base.and_then(|base| base.join(source).ok())
                    .and_then(|url| match url.scheme() {
                        "file" => url.to_file_path().ok(),
                        "apps" => assets.get(url.path().trim_start_matches('/')).cloned(),
                        _ => None,
                    })
            }) else {
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
            // WARNING: each layer is its own SVG document, so its `id`s must not collide.
            content.push_str(
                &svg[start + 1..end]
                    .replace("id=\"", &format!("id=\"m{index}"))
                    .replace("url(#", &format!("url(#m{index}")),
            );
        }
    }
    (!content.is_empty()).then_some(content)
}
