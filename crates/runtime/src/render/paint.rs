use super::background;
use crate::style;
use crate::style::Prop;
use renderer::{Child, Hooks, NodeCx, Pixels, Point, Rect, Size, Window, point, px};
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
    fn cached(&self) -> bool {
        true
    }

    // Inside a box that a scroll moved, the renderer moves its hit transform.
    fn shifts(&self) -> bool {
        true
    }

    // The matrix follows the bounds, so a kept box places its hit transform again.
    fn volatile(&self) -> bool {
        false
    }

    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.prepaint(cx, inner)
    }

    fn skippable(&self) -> bool {
        true
    }

    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let matrix = self.matrix(cx.bounds.origin);
        self.matrix.set(matrix);
        cx.window.with_keyed_hit_transform(matrix, None, inner);
    }
    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let effect = renderer::PaintEffect {
            transform: Some(self.matrix.get()),
            ..Default::default()
        };
        cx.window.with_effect(effect, inner);
    }
}

pub(super) struct Transformed {
    pub(super) style: Rc<style::Style>,
    pub(super) units: style::Lengths,
    pub(super) matrix: Cell<Option<super::effects::Matrix>>,
    pub(super) local: super::effects::Local,
}

impl Hooks for Transformed {
    fn cached(&self) -> bool {
        true
    }

    // Inside a box that a scroll moved, the renderer moves its hit transform.
    fn shifts(&self) -> bool {
        true
    }

    // The matrix follows the bounds, so a kept box places its hit transform again.
    fn volatile(&self) -> bool {
        false
    }

    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.prepaint(cx, inner)
    }

    fn skippable(&self) -> bool {
        true
    }

    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let matrix = super::effects::transform_at(&self.local, &self.style, &self.units, cx.bounds);
        self.matrix.set(matrix);
        match matrix {
            Some(matrix) => cx.window.with_keyed_hit_transform(matrix, None, inner),
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
                cx.window.with_effect(effect, inner);
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
    fn volatile(&self) -> bool {
        false
    }

    fn skippable(&self) -> bool {
        true
    }

    fn skip(&self, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shift(&self, _: Point<Pixels>, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shifts(&self) -> bool {
        true
    }

    // Frames patch the layers of composited animations.
    fn cached(&self) -> bool {
        self.effect.key.is_none()
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let mut effect = self.effect.clone();
        if let Some(backdrop) = effect.backdrop.as_mut()
            && let Some(radii) = &self.backdrop
        {
            backdrop.bounds = cx.bounds;
            backdrop.corner_radii = radii.resolve(cx.bounds.size);
        }
        cx.window.with_effect(effect, inner);
    }
}

// The mask layers of a pseudo-element.
pub(super) struct Masked {
    pub(super) view: Rc<super::PageState>,
    pub(super) key: (usize, u8),
    pub(super) style: Rc<style::Style>,
    pub(super) units: style::Lengths,
}

impl Hooks for Masked {
    fn volatile(&self) -> bool {
        false
    }

    // Nothing draws outside the mask image.
    fn bounded(&self) -> bool {
        true
    }

    fn skippable(&self) -> bool {
        true
    }

    fn skip(&self, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shift(&self, _: Point<Pixels>, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shifts(&self) -> bool {
        true
    }

    fn cached(&self) -> bool {
        true
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let mask = {
            let mut host = self.view.host.borrow_mut();
            let host = &mut *host;
            mask(
                &self.view,
                host.dom.entry(self.view.id).or_default(),
                &host.assets,
                self.key,
                &self.style,
                self.units,
                None,
                cx.bounds,
                cx.window,
            )
        };
        let effect = renderer::PaintEffect {
            mask,
            ..Default::default()
        };
        cx.window.with_effect(effect, inner);
    }
}

// The children of a canvas with drawable content. The window doesn't show them, and they take no pointer
// until the page draws them.
pub(super) struct Undrawn(pub(super) Rc<super::PageState>);

impl Hooks for Undrawn {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        // The drawable boxes inside find the content box of their canvas here.
        let outer = self.0.drawing.replace(cx.bounds.origin);
        cx.window.with_hit_base(cx.bounds, |window| {
            window.with_hits_at(None, |window| inner(window))
        });
        self.0.drawing.set(outer);
    }
    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        cx.window.with_snapshot(None, cx.bounds, inner);
    }
}

#[derive(Clone, Copy)]
pub(super) enum Source {
    Element,
    Presented,
    Drawn,
}

pub(super) struct Picture {
    pub(super) id: usize,
    pub(super) view: Rc<super::PageState>,
    pub(super) source: Source,
    pub(super) fit: renderer::ObjectFit,
    pub(super) cropped: Option<renderer::Radii<Pixels>>,
    /// `object-position` when it isn't centered.
    pub(super) position: Option<(Rc<str>, style::Lengths)>,
    pub(super) pixelated: bool,
}

impl Hooks for Picture {
    fn volatile(&self) -> bool {
        false
    }

    fn skippable(&self) -> bool {
        true
    }

    fn skip(&self, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shift(&self, _: Point<Pixels>, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shifts(&self) -> bool {
        true
    }

    // Canvases and video frames change without a new box. A new image repaints the window.
    fn cached(&self) -> bool {
        matches!(self.source, Source::Element)
    }

    fn bounded(&self) -> bool {
        true
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let view = &*self.view;
        let image = view.host.try_borrow().ok().and_then(|host| {
            let dom = host.dom.get(&view.id);
            match self.source {
                Source::Element => dom?.images.get(&self.id).cloned(),
                Source::Presented => view
                    .images
                    .borrow()
                    .get(&self.id)
                    .or_else(|| dom?.images.get(&self.id))
                    .cloned(),
                Source::Drawn => dom?
                    .canvases
                    .get(&self.id)
                    .map(|canvas| canvas.image.clone()),
            }
        });
        if let Some(image) = image
            && image.size().width > 0
            && image.size().height > 0
        {
            let bounds = cx.bounds;
            let mut fitted = self.fit.place(bounds, image.size());
            if let Some((position, units)) = &self.position {
                let free = bounds.size - fitted.size;
                let [x, y] = super::effects::place(
                    position,
                    [f32::from(free.width), f32::from(free.height)],
                    units,
                );
                fitted.origin = bounds.origin + point(px(x), px(y));
            }
            match self.cropped {
                Some(corners) => {
                    cx.window
                        .draw_image(bounds, fitted, corners, image, false, self.pixelated)
                }
                None => {
                    let radii = cx.style.corner_radii.resolve(bounds.size);
                    cx.window
                        .draw_image(fitted, fitted, radii, image, false, self.pixelated);
                }
            }
        }
        inner(cx.window);
    }
}

pub(super) struct Sticky {
    pub(super) scroll: renderer::ScrollRef,
    pub(super) border: [f32; 4],
    pub(super) block: Option<(usize, [f32; 4])>,
}

pub(super) struct DomBox {
    pub(super) anchor: Option<([bool; 2], bool, [f32; 2])>,
    pub(super) id: usize,
    pub(super) plans: u8,
    pub(super) ids: Rc<super::effects::Ids>,
    pub(super) view: Rc<super::PageState>,
    pub(super) sticky: Option<Box<Sticky>>,
    pub(super) fixed: bool,
    pub(super) block: Option<[f32; 4]>,
    pub(super) container: Option<usize>,
    pub(super) sized: Option<[f32; 2]>,
    pub(super) inset: [Option<f32>; 4],
    pub(super) style: Rc<style::Style>,
    pub(super) effects: Option<f32>,
    pub(super) composited: bool,
    pub(super) transformed: bool,
    pub(super) perspective: Option<Box<((f32, String), usize)>>,
    pub(super) units: style::Lengths,
    pub(super) revision: u64,
    pub(super) sides: [f32; 2],
    pub(super) margin: [f32; 4],
    pub(super) cell: Option<usize>,
    pub(super) grid: Option<(usize, bool)>,
    pub(super) parts: Rc<[(usize, bool, Vec<usize>)]>,
    pub(super) shared: Option<[f32; 4]>,
    pub(super) leading: Option<[f32; 3]>,
    pub(super) boxless: bool,
    pub(super) text: Option<([u32; 2], [f32; 2])>,
    pub(super) placed: Cell<(Rect<Pixels>, Option<super::effects::Matrix>)>,
    // The window bounds and cull flags of the last prepaint, which a skip gives to `merge`.
    pub(super) reached_at: Cell<(Rect<Pixels>, u16)>,
    pub(super) offset: Cell<Point<Pixels>>,
    pub(super) frame: bool,
    pub(super) text_clip: bool,
    // A drawable element of a canvas, which paints into a snapshot for `drawElementImage()`.
    pub(super) drawn: bool,
    // The transform at the origin for a size, which a moved box places again.
    pub(super) local: super::effects::Local,
}

// With its counts, a box's `Rc` must stay within the small blocks of the runtime's allocator.
const _: () = assert!(size_of::<DomBox>() + 16 <= 512);

impl DomBox {
    fn block(&self, bounds: Rect<Pixels>, translation: Point<Pixels>) {
        if let Some([top, right, bottom, left]) = self.block {
            let view = &*self.view;
            let mut host = view.host.borrow_mut();
            let mut padding = Rect::new(
                bounds.origin + point(px(left), px(top)),
                renderer::size(
                    bounds.size.width - px(left + right),
                    bounds.size.height - px(top + bottom),
                ),
            );
            padding.origin += translation;
            host.dom
                .entry(view.id)
                .or_default()
                .blocks
                .insert(self.id, padding);
        }
    }

    fn mirror(
        &self,
        dom: &mut crate::dom::State,
        window_bounds: Rect<Pixels>,
        size: Size<Pixels>,
        parts: &[Rect<Pixels>],
    ) {
        let mirror = &mut dom.bounds;
        let [mut top, right, mut bottom, left] = self.shared.unwrap_or_default();
        if let Some([line, above, below]) = self.leading
            && (f32::from(window_bounds.size.height) - line).abs() < 0.5
        {
            (top, bottom) = (above, below);
        }
        mirror.insert(
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
        let scaled = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() > 0.01;
        mirror.transformed(
            self.id,
            scaled(window_bounds.size.width, size.width)
                || scaled(window_bounds.size.height, size.height)
                || !self.drawn
                    && [Prop::TRANSLATE, Prop::ROTATE, Prop::SCALE, Prop::TRANSFORM]
                        .into_iter()
                        .any(|prop| self.style.get(prop).is_some_and(|value| value != "none")),
        );
        // Like Chrome, a box that columns split reports the union of its parts.
        if let Some(union) = parts.iter().copied().reduce(|all, part| all.union(part)) {
            let rect = |part: Rect<Pixels>| {
                [
                    part.origin.x,
                    part.origin.y,
                    part.size.width,
                    part.size.height,
                ]
                .map(f32::from)
            };
            mirror.insert(self.id, rect(union));
            dom.fragments
                .insert(self.id, parts.iter().copied().map(rect).collect());
        } else if !dom.fragments.is_empty() {
            dom.fragments.remove(&self.id);
        }
    }

    fn unions(&self) {
        let view = &*self.view;
        if let Some(([first, last], [above, below])) = self.text {
            let mut host = view.host.borrow_mut();
            let bounds = &mut host.dom.entry(view.id).or_default().bounds;
            if let (Some(&[x, _, width, _]), Some(&[_, top, ..]), Some(&[_, y, _, height])) = (
                bounds.get(&self.id),
                bounds.get(&(first as usize)),
                bounds.get(&(last as usize)),
            ) && width > 0.
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
            // Rows and row groups span all columns of the table. A row ends where its first cell that
            // spans no rows below ends.
            let mut across = [f32::INFINITY, f32::NEG_INFINITY];
            for (part, row, members) in self.parts.iter() {
                let union = members
                    .iter()
                    .filter_map(|member| bounds.get(member).copied())
                    .reduce(|[x, y, w, h], [a, b, c, d]| {
                        let [left, top] = [x.min(a), y.min(b)];
                        let bottom = if *row {
                            (y + h).min(b + d)
                        } else {
                            (y + h).max(b + d)
                        };
                        [left, top, (x + w).max(a + c) - left, bottom - top]
                    });
                if let Some(union) = union {
                    across = [across[0].min(union[0]), across[1].max(union[0] + union[2])];
                    bounds.insert(*part, union);
                }
            }
            for (part, ..) in self.parts.iter() {
                if let Some(rect) = bounds.get_mut(part) {
                    [rect[0], rect[2]] = [across[0], across[1] - across[0]];
                }
            }
        }
    }

    fn reached(&self, window_bounds: Rect<Pixels>, flags: u16) {
        self.reached_at.set((window_bounds, flags));
        let view = &*self.view;
        let none = [f32::INFINITY, f32::NEG_INFINITY];
        let found = view
            .reaching
            .borrow_mut()
            .pop()
            .map_or(none, |(reach, _)| reach);
        let reach = if self.plans == u8::MAX { none } else { found };
        let mut reached = view.reach.borrow_mut();
        if reached.len() <= self.id {
            reached.resize(self.id + 1, [f32::NAN; 2]);
        }
        reached[self.id] = reach;
        drop(reached);
        self.merge(reach, window_bounds, flags);
    }

    // Adds the reach of the box to the box around it.
    fn merge(&self, reach: [f32; 2], window_bounds: Rect<Pixels>, flags: u16) {
        let mut reaching = self.view.reaching.borrow_mut();
        if let Some(([bottom, top], plans)) = reaching.last_mut() {
            *bottom = bottom.min(reach[0]);
            *top = top.max(reach[1]);
            if matches!(*plans, 1 | 2)
                && flags & (super::cull::PLAIN_BLOCK | super::cull::UNCULLABLE)
                    == super::cull::PLAIN_BLOCK
                && (*plans == 1 || flags & super::cull::DEFINITE_SIZE == 0)
            {
                *bottom = bottom.min(f32::from(window_bounds.bottom()));
                *top = top.max(f32::from(window_bounds.top()));
            }
        }
    }
}

impl Hooks for DomBox {
    // Of the effect layers, only filters draw outside the boxes inside.
    fn bounded(&self) -> bool {
        self.placed.get().1.is_none()
            && !self.drawn
            && (self.effects.is_none()
                || [Prop::FILTER, Prop::WEBKIT_FILTER]
                    .into_iter()
                    .all(|prop| self.style.get(prop).is_none_or(|value| value == "none")))
    }

    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let parts: Vec<_> = cx.fragments().collect();
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
        if let Some(sticky) = &self.sticky {
            let Sticky {
                scroll,
                border,
                block,
            } = &**sticky;
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
                        Rect::new(
                            scroll.bounds().origin + scroll.offset() + scroll.stretch(),
                            renderer::size(px(width), px(height)),
                        )
                    }),
                    _ => dom.blocks.get(&container).copied(),
                })
            }
        {
            // Redraw only once per mismatch to avoid endless frames when layout repeats the same size.
            if let Some([width, height]) = self.sized
                && ((f32::from(block.size.width) - width).abs() > 0.5
                    || (f32::from(block.size.height) - height).abs() > 0.5)
            {
                let key = (
                    self.id,
                    [
                        width,
                        height,
                        block.size.width.into(),
                        block.size.height.into(),
                    ]
                    .map(f32::to_bits),
                );
                let [current, seen] = &mut *self.view.mismatched.borrow_mut();
                if current.insert(key) && !seen.contains(&key) {
                    window.redraw();
                }
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
                let auto = |side: usize| self.style.get(style::MARGIN[side]) == Some("auto");
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
        self.block(bounds, point(px(0.), px(0.)));
        let perspective = self
            .perspective
            .as_deref()
            .and_then(|((distance, origin), parent)| {
                let view = &*self.view;
                let host = view.host.borrow();
                let [x, y, width, height] = *host.dom.get(&view.id)?.bounds.get(parent)?;
                let [ox, oy, _] = super::effects::origin(origin, &self.units, [width, height]);
                Some((*distance, [x + ox, y + oy]))
            });
        // A canvas draws a drawable element as it is before its transform.
        let mut matrix = match perspective {
            Some(_) => super::effects::transform(&self.style, &self.units, bounds, perspective),
            None => super::effects::transform_at(&self.local, &self.style, &self.units, bounds),
        }
        .filter(|_| !self.drawn);
        if self.transformed && !self.drawn {
            matrix.get_or_insert(super::effects::IDENTITY);
            self.view
                .composited_transforms
                .borrow_mut()
                .insert(self.id, (bounds, perspective));
        }
        if !self.transformed
            && let Some([x, y]) = matrix.as_ref().and_then(super::effects::translation)
        {
            let translation = point(px(x), px(y));
            offset += translation;
            bounds.origin += translation;
            matrix = None;
            // Boxes positioned in this block include the translation through the offset, so the block must too.
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
            // The parts move with the box's offsets and transform.
            let parts: Vec<_> = parts
                .iter()
                .map(|part| window.transformed_bounds(*part + (bounds.origin - exact.origin)))
                .collect();
            let view = &*self.view;
            let flags = {
                let mut host = view.host.borrow_mut();
                let dom = host.dom.entry(view.id).or_default();
                // Grid tracks are sized before transforms, so they compare with the untransformed box.
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
                self.mirror(dom, window_bounds, bounds.size, &parts);
                let width = f32::from(bounds.size.width);
                let [top, right, bottom, left] = self.margin;
                let cell = self
                    .cell
                    .and_then(|grid| {
                        let cells = &dom.grids.get(&grid)?.3.cells;
                        let index = cells.binary_search_by_key(&self.id, |cell| cell.0).ok()?;
                        let [_, _, width, height] = cells[index].2;
                        Some([width, height])
                    })
                    .unwrap_or([
                        (width + right + left).max(0.),
                        f32::from(bounds.size.height) + top + bottom,
                    ]);
                let cull = &mut dom.cull;
                if cull.placed.len() <= self.id {
                    cull.placed.resize(self.id + 1, Default::default());
                }
                cull.placed[self.id] = (self.revision, cell, self.margin);
                let inner = [
                    width - self.sides[0],
                    f32::from(bounds.size.height) - self.sides[1],
                ];
                cull.widths.insert(self.id, inner);
                let flags = cull.info.get(self.id).map_or(0, |info| info.flags);
                view.reaching
                    .borrow_mut()
                    .push(([f32::INFINITY, f32::NEG_INFINITY], self.plans));
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
                    if dom.baselines.insert(self.id, baselines) != Some(baselines) {
                        dom.grids.remove(&grid);
                        if aligned {
                            window.redraw();
                        }
                    }
                }
                flags
            };
            let moved = view
                .frames
                .borrow_mut()
                .get_mut(&self.id)
                .is_some_and(|frame| {
                    let zoom = window.zoom();
                    let bounds = [
                        window_bounds.origin.x,
                        window_bounds.origin.y,
                        window_bounds.size.width,
                        window_bounds.size.height,
                    ]
                    .map(|value| (f32::from(value) * zoom) as f64);
                    frame.bounds != bounds && {
                        frame.bounds = bounds;
                        true
                    }
                });
            // The web view can run its event loop while it moves, so only a shared borrow is held.
            if moved && let Some(frame) = view.frames.borrow().get(&self.id) {
                let _ = frame.browser.set_bounds(frame.bounds);
            }
            window.scope(renderer::Scope::Offset(offset), |window| inner(window));
            self.reached(window_bounds, flags);
            self.unions();
        };
        match matrix {
            _ if self.drawn => {
                // The box takes the pointer where its canvas drew it.
                let view = &*self.view;
                let content = view.drawing.get();
                let drawn = view.host.borrow().dom.get(&view.id).and_then(|dom| {
                    let to_canvas = dom.drawn.get(&self.id)?.matrix;
                    let [x, y] = [bounds.origin.x, bounds.origin.y].map(f32::from);
                    let from_box = super::effects::translate(-x, -y, 0.);
                    let to_window =
                        super::effects::translate(content.x.into(), content.y.into(), 0.);
                    let placed = super::effects::multiply(&to_canvas, &from_box);
                    Some(super::effects::multiply(&to_window, &placed))
                });
                window.with_hits_at(drawn, &mut prepaint)
            }
            Some(matrix) => window.with_keyed_hit_transform(
                matrix,
                self.transformed.then_some(self.id as u64),
                &mut prepaint,
            ),
            None => prepaint(window),
        }
        self.placed.set((bounds, matrix));
        self.offset.set(offset);
    }
    // A drawn box is a snapshot that the page reads, a composited box has layers that frames patch, and filters
    // or clips can read elements elsewhere in the page.
    fn cached(&self) -> bool {
        !self.drawn
            && !self.composited
            && !self.frame
            && [
                Prop::FILTER,
                Prop::WEBKIT_FILTER,
                Prop::CLIP_PATH,
                Prop::WEBKIT_CLIP_PATH,
            ]
            .into_iter()
            .all(|prop| {
                self.style
                    .get(prop)
                    .is_none_or(|value| !value.contains("url("))
            })
    }

    fn volatile(&self) -> bool {
        self.drawn
            || self.anchor.is_some()
            || self.sticky.is_some()
            || self.fixed
            || self.container.is_some()
            || self.sized.is_some()
            || self.perspective.is_some()
            || self.transformed
            || self.frame
    }

    fn skippable(&self) -> bool {
        true
    }

    fn shifts(&self) -> bool {
        true
    }

    // Moves the layout mirror, containing blocks, fragments and cull reach of the nodes below the box.
    fn shift(&self, by: Point<Pixels>, exact: Rect<Pixels>, _: &mut Window) -> bool {
        let (placed, matrix) = self.placed.get();
        let bounds = Rect::new(exact.origin + self.offset.get(), exact.size);
        let near = |a: Pixels, b: Pixels| (a - b).0.abs() < 0.01;
        if !near(placed.origin.x + by.x, bounds.origin.x)
            || !near(placed.origin.y + by.y, bounds.origin.y)
            || placed.size != bounds.size
        {
            return false;
        }
        let view = &*self.view;
        let mut host = view.host.borrow_mut();
        let dom = host.dom.entry(view.id).or_default();
        let walk = &dom.style_walk;
        let Some(start) = walk.place(self.id) else {
            return false;
        };
        let range = start as usize..start as usize + walk.span(self.id);
        let inside = |id: usize| {
            walk.place(id)
                .is_some_and(|at| range.contains(&(at as usize)))
        };
        // Top layer boxes paint at the root, outside this box.
        if dom.top_layer.iter().any(|&node| inside(node)) {
            return false;
        }
        let [x, y] = [by.x, by.y].map(f32::from);
        let nodes = walk.nodes.clone();
        let mut reach = view.reach.borrow_mut();
        for &(id, _) in &nodes[range] {
            let id = id as usize;
            if let Some(parts) = dom.fragments.get_mut(&id) {
                parts
                    .iter_mut()
                    .for_each(|part| (part[0], part[1]) = (part[0] + x, part[1] + y));
            }
            if let Some(block) = dom.blocks.get_mut(&id) {
                block.origin += by;
            }
            // A node without a box keeps an empty rectangle at the origin.
            if let Some(rect) = dom.bounds.get_mut(&id)
                && *rect != [0.; 4]
            {
                (rect[0], rect[1]) = (rect[0] + x, rect[1] + y);
            }
            if let Some(reach) = reach.get_mut(id)
                && reach[0].is_finite()
            {
                (reach[0], reach[1]) = (reach[0] + y, reach[1] + y);
            }
        }
        let reach = reach
            .get(self.id)
            .copied()
            .unwrap_or([f32::INFINITY, f32::NEG_INFINITY]);
        drop((host, nodes));
        // A transform turns about the new place.
        let matrix = matrix.and_then(|_| {
            super::effects::transform_at(&self.local, &self.style, &self.units, bounds)
        });
        self.placed.set((bounds, matrix));
        let (window_bounds, flags) = self.reached_at.get();
        let window_bounds = Rect::new(window_bounds.origin + by, window_bounds.size);
        self.reached_at.set((window_bounds, flags));
        self.merge(reach, window_bounds, flags);
        true
    }

    fn skip(&self, exact: Rect<Pixels>, _: &mut Window) -> bool {
        if Rect::new(exact.origin + self.offset.get(), exact.size) != self.placed.get().0 {
            return false;
        }
        let none = [f32::INFINITY, f32::NEG_INFINITY];
        let reach = (self.view.reach.borrow().get(self.id).copied()).unwrap_or(none);
        let (window_bounds, flags) = self.reached_at.get();
        self.merge(reach, window_bounds, flags);
        true
    }

    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let (placed, mut matrix) = self.placed.get();
        let offset = self.offset.get();
        let bounds = Rect::new(cx.exact.origin + offset, cx.exact.size);
        let moved = bounds != placed;
        let parts: Vec<_> = match moved {
            true => cx.fragments().collect(),
            false => Vec::new(),
        };
        if moved {
            // A kept box has its size, so its transform only turns about the new place.
            if matrix.is_some() {
                matrix =
                    super::effects::transform_at(&self.local, &self.style, &self.units, bounds);
            }
            self.block(cx.exact, offset);
            self.placed.set((bounds, matrix));
        }
        let mut kept = |window: &mut Window| {
            let window_bounds = window.transformed_bounds(bounds);
            let parts: Vec<_> = parts
                .iter()
                .map(|part| window.transformed_bounds(*part + offset))
                .collect();
            let view = &*self.view;
            let flags = {
                let mut host = view.host.borrow_mut();
                let dom = host.dom.entry(view.id).or_default();
                if moved {
                    self.mirror(dom, window_bounds, bounds.size, &parts);
                }
                dom.cull.info.get(self.id).map_or(0, |info| info.flags)
            };
            view.reaching
                .borrow_mut()
                .push(([f32::INFINITY, f32::NEG_INFINITY], self.plans));
            window.scope(renderer::Scope::Offset(offset), |window| inner(window));
            self.reached(window_bounds, flags);
            if moved {
                self.unions();
            }
        };
        match matrix {
            Some(matrix) => cx.window.with_keyed_hit_transform(matrix, None, &mut kept),
            None => kept(cx.window),
        }
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        if !self.drawn {
            return self.draw(cx.window, inner);
        }
        // A new page or a new node with this number has no snapshot of the box yet.
        let view = &*self.view;
        let host = view.host.borrow();
        let fresh =
            (host.dom.get(&view.id)).is_none_or(|dom| !dom.snapshots.contains_key(&self.id));
        drop(host);
        let key = Some((self.id as u64, fresh));
        (cx.window).with_snapshot(key, self.placed.get().0, |window| self.draw(window, inner));
    }
}

impl DomBox {
    fn draw(&self, window: &mut Window, inner: &mut dyn FnMut(&mut Window)) {
        let (bounds, matrix) = self.placed.get();
        super::native::paint(&self.view, self.id, bounds, &self.style, window);
        window.mark(self.view.id << 32 | self.id as u64);
        let inner = &mut |window: &mut Window| match self.text_clip {
            true => window.with_text_clip(|window| inner(window)),
            false => inner(window),
        };
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
            Some(clip) => window.scope(renderer::Scope::Clip(Some(*clip)), |window| inner(window)),
            None => inner(window),
        };
        nest_effects(&mut layers, window, &mut paint);
    }

    fn effects(
        &self,
        bounds: Rect<Pixels>,
        transform: Option<super::effects::Matrix>,
        window: &mut Window,
    ) -> (Option<renderer::Clip<Pixels>>, Vec<renderer::PaintEffect>) {
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
                clip = Some(renderer::Clip::rounded(
                    Rect::new(
                        bounds.origin + point(px(rect[0]), px(rect[1])),
                        renderer::size(px(rect[2]), px(rect[3])),
                    ),
                    effects::corners(radii),
                ))
            }
            Some(Clip::Shape(shape)) => shapes = Some(shape),
            None => {}
        }
        let mask = mask(
            view,
            host.dom.entry(view.id).or_default(),
            &host.assets,
            (self.id, 0),
            style,
            units,
            shapes.as_deref(),
            bounds,
            window,
        );
        let visible = style
            .get(Prop::VISIBILITY)
            .is_none_or(|value| value == "visible");
        let shown = self.effects.unwrap_or(1.) > 0. && visible;
        // A composited box keeps its backdrop also at no opacity or blur, so frames can patch them.
        let backdrop = value(&[Prop::BACKDROP_FILTER, Prop::WEBKIT_BACKDROP_FILTER])
            .filter(|_| shown || self.composited && visible)
            .and_then(|value| {
                effects::backdrop(value, &units).or_else(|| {
                    effects::lone_blur(Some(value))
                        .filter(|_| self.composited)
                        .map(|_| (px(0.), None))
                })
            })
            .map(|(blur, color_matrix)| renderer::Backdrop {
                bounds,
                corner_radii: effects::radii(style, &units, || size).resolve(bounds.size),
                blur,
                color_matrix,
            });
        let glass = style
            .get(Prop::MACOS_GLASS)
            .filter(|kind| {
                cfg!(target_os = "macos")
                    && shown
                    && layers.is_empty()
                    && clip.is_none()
                    && matches!(*kind, "regular" | "clear")
            })
            .and_then(|kind| {
                let radii = effects::radii(style, &units, || size).resolve(bounds.size);
                let radius = radii.x.top_left;
                if radii != renderer::Radii::from(radius) {
                    return None;
                }
                Some(renderer::Glass {
                    key: self.id as u64,
                    bounds,
                    corner_radius: radius,
                    tint: style
                        .get(Prop::MACOS_GLASS_TINT_COLOR)
                        .and_then(style::color)
                        .filter(|(_, alpha)| *alpha > 0)
                        .map(crate::style::rgba),
                    clear: kind == "clear",
                    interactive: style.get(Prop::MACOS_GLASS_INTERACTIVE) == Some("true")
                        && style.get(Prop::POINTER_EVENTS) != Some("none")
                        && doc
                            .nodes
                            .get(self.id)
                            .is_some_and(|node| !node.attrs.contains_key("disabled")),
                })
            });
        let outer = renderer::PaintEffect {
            transform,
            opacity: self.effects.unwrap_or(1.),
            blend_mode: value(&[Prop::MIX_BLEND_MODE])
                .map_or(renderer::BlendMode::Normal, |value| {
                    effects::blend_mode(value)
                }),
            mask,
            backdrop,
            glass,
            key: self.composited.then_some(self.id as u64),
            ..Default::default()
        };
        // Isolation affects mix-blend-mode only. An extra group without blending incorrectly hides content
        // from descendant backdrop filters.
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
            || outer.glass.is_some()
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

/// The mask image of a box or a pseudo-element `key` from its mask layers and its clip path shapes.
#[allow(clippy::too_many_arguments)]
pub(super) fn mask(
    view: &super::PageState,
    dom: &mut crate::dom::State,
    assets: &HashMap<String, std::path::PathBuf>,
    key: (usize, u8),
    style: &style::Style,
    units: style::Lengths,
    shapes: Option<&str>,
    bounds: Rect<Pixels>,
    window: &mut Window,
) -> Option<(std::sync::Arc<renderer::RenderImage>, Rect<Pixels>)> {
    let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
    let base = dom.base.clone();
    let images = [Prop::MASK_IMAGE, Prop::WEBKIT_MASK_IMAGE]
        .into_iter()
        .find_map(|prop| style.get(prop))
        .filter(|value| !value.is_empty() && *value != "none");
    (shapes.is_some() || images.is_some()).then(|| {
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
            Prop::MASK_COMPOSITE,
        ]
        .map(|prop| style.get(prop).unwrap_or(""))
        .into_iter()
        .chain(shapes)
        {
            inputs.push_str(part);
            inputs.push('\0');
        }
        super::effects::push_bits(&mut inputs, size, &units);
        let [width, height] = size;
        let scale = window.scale_factor();
        let raster = [width, height].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
        let masks = &mut dom.masks;
        if masks.get(&key).is_none_or(|(old, image)| {
            *old != inputs
                || image.as_ref().is_some_and(|image| {
                    (image.size().width as u32) != raster[0]
                        || (image.size().height as u32) != raster[1]
                })
        }) {
            let image = if shapes.is_none()
                && let Some(bytes) = super::effects::axis_mask(style, &units, size, raster)
            {
                bytes.map(|bytes| {
                    std::sync::Arc::new(renderer::RenderImage::new(raster[0], raster[1], bytes))
                })
            } else {
                let mut files = Vec::new();
                let layers = mask_svg(style, units, size, base.as_ref(), assets, &mut files);
                // Hold the decoded files: `retire` drops the bitmaps that nothing holds.
                let mut sources = view.mask_sources.borrow_mut();
                let held = sources.entry(key).or_default();
                held.retain(|(file, _)| files.iter().any(|path| path.to_string_lossy() == *file));
                let mut kinds = Vec::with_capacity(files.len());
                let mut decoded = Vec::new();
                for path in &files {
                    let file = path.to_string_lossy();
                    let source = match held.iter().position(|(held, _)| *held == file) {
                        Some(index) => Some(&held[index].1),
                        None => background::load(view, &mut dom.bitmaps, key.0, &file, path, window)
                            .map(|source| &held.push_mut((file.into_owned(), source)).1),
                    };
                    // A file that still decodes is a transparent layer until the window redraws.
                    kinds.push(source.map(|(bitmap, tree)| match tree {
                        Some((tree, _)) => usvg::ImageKind::SVG(tree.clone()),
                        None => {
                            let stub = super::resvg::stub(bitmap);
                            decoded.push((stub.clone(), bitmap.clone()));
                            usvg::ImageKind::PNG(stub)
                        }
                    }));
                }
                if kinds.iter().any(Option::is_none) {
                    inputs.push_str("\0decoding");
                }
                let source = match (shapes, layers) {
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
                let mut options = super::canvas::svg_options(false, false);
                options.image_href_resolver.resolve_string = Box::new(move |href, _| {
                    kinds.get(href.strip_prefix("apps-mask:")?.parse::<usize>().ok()?)?.clone()
                });
                match usvg::Tree::from_str(&source, &options)
                    .map_err(anyhow::Error::from)
                    .and_then(|tree| {
                        super::resvg::decoded(decoded, || {
                            super::canvas::render_svg(&tree, raster[0], raster[1], None)
                        })
                    }) {
                    Ok(image) => Some(image),
                    // Keep the failure too, or every paint builds and reports the same mask again.
                    Err(error) => {
                        eprintln!("apps: mask: {error}");
                        None
                    }
                }
            };
            if let Some((_, Some(old))) = masks.insert(key, (inputs, image)) {
                window.release_image(old);
            }
        }
        masks
            .get(&key)
            .and_then(|(_, image)| image.clone().map(|image| (image, bounds)))
    })
    .flatten()
}

fn nest_effects(
    layers: &mut Vec<renderer::PaintEffect>,
    window: &mut Window,
    paint: &mut dyn FnMut(&mut Window),
) {
    match layers.pop() {
        Some(effect) => window.with_effect(effect, |window| nest_effects(layers, window, paint)),
        None => paint(window),
    }
}

pub(super) fn mask_svg<'a>(
    style: &'a style::Style,
    units: style::Lengths,
    [width, height]: [f32; 2],
    base: Option<&url::Url>,
    assets: &HashMap<String, std::path::PathBuf>,
    // The files of the `url()` layers, which the SVG names as `apps-mask:<index>`.
    files: &mut Vec<std::path::PathBuf>,
) -> Option<String> {
    let get =
        |[prop, webkit]: [Prop; 2]| style.get(prop).or_else(|| style.get(webkit)).unwrap_or("");
    let images = get([Prop::MASK_IMAGE, Prop::WEBKIT_MASK_IMAGE]);
    if images.is_empty() || images == "none" {
        return None;
    }
    let list = |props| style::components(get(props), ',');
    let (sizes, positions, repeats) = (
        list([Prop::MASK_SIZE, Prop::WEBKIT_MASK_SIZE]),
        list([Prop::MASK_POSITION, Prop::WEBKIT_MASK_POSITION]),
        list([Prop::MASK_REPEAT, Prop::WEBKIT_MASK_REPEAT]),
    );
    let pick = |values: &[&'a str], index: usize| {
        values
            .get(index % values.len().max(1))
            .copied()
            .unwrap_or("")
    };
    let composites = style::components(style.get(Prop::MASK_COMPOSITE).unwrap_or(""), ',');
    // Layers go from the bottom up, and each one joins the layers below it by its `mask-composite`.
    let (mut defs, mut below) = (String::new(), None::<String>);
    let area = format!(
        "maskUnits=\"userSpaceOnUse\" x=\"0\" y=\"0\" width=\"{width}\" height=\"{height}\""
    );
    for (index, image) in style::components(images, ',').into_iter().enumerate().rev() {
        let image = image.trim();
        let [size, position, repeat] = [
            (pick(&sizes, index), "auto"),
            (pick(&positions, index), "0% 0%"),
            (pick(&repeats, index), "repeat"),
        ]
        .map(|(value, initial)| if value.is_empty() { initial } else { value });
        let layer = if let Some(source) =
            image.strip_prefix("url(").and_then(|v| v.strip_suffix(')'))
        {
            let Some(path) = super::image::url_file(source, base, assets) else {
                continue;
            };
            let aspect = match size {
                "cover" => "xMidYMid slice",
                "contain" => "xMidYMid meet",
                _ => "none",
            };
            let [[tw, th], at] = super::effects::tile(size, position, &units, width, height, None);
            files.push(path);
            super::effects::tiled(
                repeat,
                [tw, th],
                at,
                &format!(
                    "<image href=\"apps-mask:{}\" width=\"{tw}\" height=\"{th}\" preserveAspectRatio=\"{aspect}\"/>",
                    files.len() - 1
                ),
            )
        } else {
            let Some((kind, body)) = image
                .split_once('(')
                .filter(|(kind, _)| kind.ends_with("-gradient"))
                .and_then(|(kind, body)| Some((kind, body.strip_suffix(')')?)))
            else {
                continue;
            };
            let Some((svg, _)) = super::effects::gradient_svg(
                kind,
                body,
                size,
                position,
                repeat,
                units,
                width,
                height,
                [0., 0., width, height],
            ) else {
                continue;
            };
            let (Some(start), Some(end)) = (svg.find('>'), svg.rfind("</svg>")) else {
                continue;
            };
            svg[start + 1..end].to_owned()
        };
        // Each layer is its own SVG document, so its `id`s must not collide.
        let layer = layer
            .replace("id=\"", &format!("id=\"m{index}"))
            .replace("url(#", &format!("url(#m{index}"));
        let Some(under) = below.take() else {
            below = Some(layer);
            continue;
        };
        // `source-*` and `xor` are the `-webkit-mask-composite` names.
        let joined = match pick(&composites, index).trim() {
            "intersect" | "source-in" => {
                defs.push_str(&format!(
                    "<mask id=\"apps-in{index}\" mask-type=\"alpha\" {area}>{under}</mask>"
                ));
                format!("<g mask=\"url(#apps-in{index})\">{layer}</g>")
            }
            op @ ("subtract" | "source-out" | "exclude" | "xor") => {
                // An inverted alpha mask: white, with the shape drawn black over it.
                let out = |id: &str, shape: &str| {
                    format!(
                        "<mask id=\"{id}\" {area}><rect width=\"{width}\" height=\"{height}\" fill=\"#fff\"/><g filter=\"url(#apps-black)\">{shape}</g></mask>"
                    )
                };
                defs.push_str(&out(&format!("apps-out{index}"), &under));
                let mut joined = format!("<g mask=\"url(#apps-out{index})\">{layer}</g>");
                if matches!(op, "exclude" | "xor") {
                    defs.push_str(&out(&format!("apps-back{index}"), &layer));
                    joined.push_str(&format!("<g mask=\"url(#apps-back{index})\">{under}</g>"));
                }
                joined
            }
            _ => under + &layer,
        };
        below = Some(joined);
    }
    let content = below?;
    if defs.contains("apps-black") {
        defs.push_str(&format!("<filter id=\"apps-black\" filterUnits=\"userSpaceOnUse\" x=\"0\" y=\"0\" width=\"{width}\" height=\"{height}\"><feColorMatrix values=\"0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0\"/></filter>"));
    }
    Some(if defs.is_empty() {
        content
    } else {
        format!("<defs>{defs}</defs>{content}")
    })
}
