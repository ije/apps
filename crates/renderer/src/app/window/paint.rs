use super::*;
use crate::gpu::{BOX, GLYPH, IMAGE, Instance, LINE, SHADOW};

// A border around a box. `sides` limits it to some sides, bit 0 top, 1 right, 2 bottom, 3 left; 0 draws
// all of them.
#[derive(Clone, Copy, Debug, Default)]
pub struct Border {
    pub widths: Edges<Pixels>,
    pub color: Color,
    pub style: BorderStyle,
    pub sides: u8,
}

impl crate::text::TextCanvas for Window {
    fn text_system(&self) -> &TextSystem {
        &self.text_system
    }

    fn content_mask(&self) -> Clip<Pixels> {
        Window::content_mask(self)
    }

    fn cull(&mut self) {
        Window::cull(self);
    }

    fn draw_rect(&mut self, bounds: Rect<Pixels>, color: Color) {
        Window::draw_rect(self, bounds, color);
    }

    fn draw_line(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        thickness: Pixels,
        color: Color,
        style: DecorationStyle,
    ) {
        Window::draw_line(self, origin, width, thickness, color, style);
    }

    fn draw_glyph(
        &mut self,
        origin: Point<Pixels>,
        font: FontId,
        glyph: GlyphId,
        size: Pixels,
        color: Color,
        emoji: bool,
        stroke: Pixels,
    ) {
        Window::draw_glyph(self, origin, font, glyph, size, color, emoji, stroke);
    }
}

/// How far a wavy line of `thickness` device pixels reaches from its middle.
pub fn wave_reach(thickness: f32) -> f32 {
    0.2887 * (0.5 + (3. * thickness + 0.5).round()) + thickness * 0.5
}

impl Window {
    pub(super) fn snap_bounds(&self, bounds: Rect<Pixels>) -> Rect<f32> {
        let scale = self.scale_factor;
        let left = snap(bounds.left().0 * scale);
        let top = snap(bounds.top().0 * scale);
        let right = snap(bounds.right().0 * scale).max(left);
        let bottom = snap(bounds.bottom().0 * scale).max(top);
        Rect::between(point(left, top), point(right, bottom))
    }

    fn snap_stroke(&self, value: Pixels) -> f32 {
        snap_width(value.0, self.scale_factor)
    }

    fn cover_bounds(&self, bounds: Rect<Pixels>) -> Rect<f32> {
        let scale = self.scale_factor;
        let left = (bounds.left().0 * scale).floor();
        let top = (bounds.top().0 * scale).floor();
        let right = (bounds.right().0 * scale).ceil().max(left);
        let bottom = (bounds.bottom().0 * scale).ceil().max(top);
        Rect::between(point(left, top), point(right, bottom))
    }

    pub(super) fn snapped_content_mask(&self) -> Clip<f32> {
        match self.content_masks.last() {
            Some((_, snapped)) => *snapped,
            None => self.snap_mask(self.content_mask()),
        }
    }

    // The content of a scroll container, which `offset` moved, paints between these two calls.
    // The content of a scroll container paints in an interest area around its viewport, which stays put in the
    // content until the viewport nears its edge, so copies of the content can move with a scroll. The scene
    // clips the content to the viewport after paint. Prepaint uses the same masks.
    // Whether a scroll container here paints its content in an interest area.
    pub(crate) fn opens_interest(&self) -> bool {
        self.content_mask().radii == Default::default()
    }

    // Runs `f` in the interest area of a scroll at `at`: a point of the box in the window, so the area moves with
    // the box when an outer scroll moves it, and the scroll offset.
    pub(crate) fn with_interest<R>(
        &mut self,
        at: Option<(Point<Pixels>, Point<Pixels>)>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let Some((origin, offset)) = at else {
            return f(self);
        };
        self.begin_interest(origin, offset);
        let result = f(self);
        self.end_interest();
        result
    }

    fn begin_interest(&mut self, origin: Point<Pixels>, offset: Point<Pixels>) -> bool {
        let mask = self.content_mask();
        let open = self.opens_interest();
        self.viewports.push(open.then_some(mask.bounds));
        if open {
            // `offset` moves the content, so a place in the content is a place in the window less `at`.
            let axis = |start: Pixels, length: Pixels, at: Pixels| {
                let step = (length.0 / 2.).max(256.).round();
                let base = ((start - at).0 / step).floor() * step;
                (px(base - step) + at, px(length.0 + step * 3.))
            };
            let at = origin + offset;
            let (x, width) = axis(mask.bounds.origin.x, mask.bounds.size.width, at.x);
            let (y, height) = axis(mask.bounds.origin.y, mask.bounds.size.height, at.y);
            self.push_content_mask(Clip::from(Rect::new(point(x, y), size(width, height))));
        }
        open
    }

    fn end_interest(&mut self) {
        if self.viewports.pop().flatten().is_some() {
            self.pop_content_mask();
        }
    }

    // The content of a scroll container paints in `f`: its id, a point of its box, and the offset that moved it.
    pub(crate) fn with_scroll<R>(
        &mut self,
        scroll: Option<(u64, Point<Pixels>, Point<Pixels>)>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let Some((id, origin, offset)) = scroll else {
            return f(self);
        };
        let viewport = self.snap_mask(self.content_mask());
        let scale = self.scale_factor;
        let open = self.begin_interest(origin, offset);
        self.scene.begin_scroll(
            id,
            viewport,
            Point::new(-offset.x.0 * scale, -offset.y.0 * scale),
            open,
        );
        let result = f(self);
        self.end_interest();
        self.scene.end_scroll();
        result
    }

    pub(super) fn snap_mask(&self, mask: Clip<Pixels>) -> Clip<f32> {
        Clip {
            bounds: self.cover_bounds(mask.bounds),
            rounded: mask.rounded.scale(self.scale_factor),
            radii: mask.radii.scale(self.scale_factor),
        }
    }

    pub fn draw_rect(&mut self, bounds: Rect<Pixels>, color: Color) {
        self.draw_box(bounds, Radii::default(), color, Border::default());
    }

    pub fn draw_box(
        &mut self,
        bounds: Rect<Pixels>,
        radii: impl Into<Radii<Pixels>>,
        fill: Color,
        border: Border,
    ) {
        if self.opacity <= 0. {
            return;
        }
        let rect = self.snap_bounds(bounds);
        let clip = self.snapped_content_mask();
        let widths = border.widths.map(|width| self.snap_stroke(width));
        let mut instance = Instance::new(BOX, rect, clip);
        let radii = radii.into();
        instance.radii = radii.scale(self.scale_factor).to_array();
        [instance.shape[0], instance.shape[1]] = radii.shape_array();
        instance.color = fill.fade(self.opacity).to_array();
        instance.border_color = border.color.fade(self.opacity).to_array();
        instance.aux = [widths.top, widths.right, widths.bottom, widths.left];
        // Like in Chromium, dashes of square borders thinner than 3 CSS pixels are longer. On rounded borders, dashes
        // thinner than 3 device pixels are longer, and dots up to 3 device pixels wide are square.
        let sides = [
            border.widths.top,
            border.widths.right,
            border.widths.bottom,
            border.widths.left,
        ];
        let square = instance.radii == [0.; 8];
        let below = |limit: f32, scale: f32| sides.iter().all(|width| width.0 * scale < limit);
        let thin = match border.style {
            BorderStyle::Dashed if square => below(3., 1.),
            BorderStyle::Dashed => below(3., self.scale_factor),
            BorderStyle::Dotted => !square && below(3.01, self.scale_factor),
            _ => false,
        };
        instance.info[1] =
            (border.style as u32 | u32::from(thin) << 4 | u32::from(border.sides) << 8) as f32;
        if instance.color[3] > 0. {
            self.scene.push(instance);
            return;
        }
        if instance.border_color[3] <= 0. || instance.aux == [0.; 4] {
            return;
        }
        // A border alone draws as four strips around the largest box it leaves empty, so the empty pixels
        // run no shader. Where the radii reach further in than the widths, the box keeps clear of them.
        let r = &instance.radii;
        // A concave corner draws its border up to a width past its radius.
        let past = if radii.shape.is_zero() {
            0.
        } else {
            instance.aux.into_iter().fold(0., f32::max)
        };
        let reach = [
            r[4].max(r[5]),
            r[1].max(r[2]),
            r[6].max(r[7]),
            r[0].max(r[3]),
        ]
        .map(|reach| reach + past);
        // One side alone draws inside its own band.
        if border.sides.is_power_of_two() {
            let side = border.sides.trailing_zeros() as usize;
            if instance.aux[side] <= 0. {
                return;
            }
            let depth = instance.aux[side].max(reach[side]) + 1.;
            let band = match side {
                0 => Rect::between(rect.origin, point(rect.right(), rect.top() + depth)),
                1 => Rect::between(point(rect.right() - depth, rect.top()), rect.bottom_right()),
                2 => Rect::between(
                    point(rect.left(), rect.bottom() - depth),
                    rect.bottom_right(),
                ),
                _ => Rect::between(rect.origin, point(rect.left() + depth, rect.bottom())),
            };
            let part = clip.bounds.intersect(band);
            // A band that the mask cut draws differently in another mask, so a copy of it can't move.
            if part != band {
                self.scene.cull();
            }
            if !part.is_empty() {
                instance.clip = part.to_array();
                self.scene.push(instance);
            }
            return;
        }
        // A side with no border and no corner needs no strip, so the hole reaches the edge there.
        let pad = |edge: f32| if edge > 0. { edge + 1. } else { 0. };
        let inset = |top: f32, right: f32, bottom: f32, left: f32| {
            Rect::between(
                point(rect.left() + pad(left), rect.top() + pad(top)),
                point(rect.right() - pad(right), rect.bottom() - pad(bottom)),
            )
        };
        let rows = inset(
            widths.top.max(reach[0]),
            widths.right,
            widths.bottom.max(reach[2]),
            widths.left,
        );
        let columns = inset(
            widths.top,
            widths.right.max(reach[1]),
            widths.bottom,
            widths.left.max(reach[3]),
        );
        let area = |rect: Rect<f32>| rect.size.width.max(0.) * rect.size.height.max(0.);
        let hole = if area(rows) >= area(columns) {
            rows
        } else {
            columns
        };
        if hole.is_empty() {
            self.scene.push(instance);
            return;
        }
        for strip in [
            Rect::between(rect.origin, point(rect.right(), hole.top())),
            Rect::between(point(rect.left(), hole.bottom()), rect.bottom_right()),
            Rect::between(point(rect.left(), hole.top()), hole.bottom_left()),
            Rect::between(hole.top_right(), point(rect.right(), hole.bottom())),
        ] {
            let part = clip.bounds.intersect(strip);
            if part != strip {
                self.scene.cull();
            }
            if !part.is_empty() {
                instance.clip = part.to_array();
                self.scene.push(instance);
            }
        }
    }

    pub fn draw_shadows(
        &mut self,
        bounds: Rect<Pixels>,
        radii: Radii<Pixels>,
        shadows: &[BoxShadow],
        inset: bool,
    ) {
        if self.opacity <= 0. {
            return;
        }
        let scale = self.scale_factor;
        let clip = self.snapped_content_mask();
        let caster = self.cover_bounds(bounds).to_array();
        let caster_radii = radii.scale(scale).to_array();
        let [top, bottom] = radii.shape_array();
        for shadow in shadows.iter().filter(|shadow| shadow.inset == inset) {
            let spread = shadow.spread_radius;
            let (shape, shape_radii) = if inset {
                (
                    (bounds + shadow.offset).outset(-spread),
                    radii.inset(spread, spread, spread, spread),
                )
            } else {
                // Like Chromium, a radius smaller than the spread grows less, so small curves stay sharp.
                let spread = spread.0;
                let grow = |radius: Pixels| {
                    let radius = radius.0;
                    Pixels(if radius <= 0. || spread == 0. {
                        radius
                    } else if spread < 0. || radius >= spread {
                        (radius + spread).max(0.)
                    } else {
                        radius + spread * (1. + (radius / spread - 1.).powi(3))
                    })
                };
                let grown = Radii {
                    x: radii.x.map(grow),
                    y: radii.y.map(grow),
                    shape: radii.shape,
                };
                let grown = radii.moved_out(shadow.spread_radius, grown);
                ((bounds + shadow.offset).outset(shadow.spread_radius), grown)
            };
            let quad = self.cover_bounds(shape);
            let mut instance = Instance::new(SHADOW, quad, clip);
            instance.radii = shape_radii.scale(scale).to_array();
            instance.color = shadow.color.fade(self.opacity).to_array();
            instance.aux = caster;
            instance.aux_radii = caster_radii;
            [instance.shape[0], instance.shape[1]] = [top, bottom];
            instance.info[1] = f32::from(u8::from(inset));
            instance.info[2] = shadow.blur_radius.scale(scale);
            // The shader draws nothing under the box of an outer shadow, or inside the shape of an inset
            // one, so the quad leaves that hole out.
            let blur = instance.info[2] * 3.;
            let rect =
                |[x, y, width, height]: [f32; 4]| Rect::new(point(x, y), size(width, height));
            let area = match inset {
                true => rect(caster),
                false => rect([
                    quad.left() - blur,
                    quad.top() - blur,
                    quad.size.width + blur * 2.,
                    quad.size.height + blur * 2.,
                ]),
            };
            let (solid, radii, margin) = match inset {
                true => (quad, instance.radii, blur + 1.),
                false => (rect(caster), caster_radii, 1.),
            };
            let [x, y] = [&radii[..4], &radii[4..]]
                .map(|radii| radii.iter().copied().fold(0., f32::max) + margin);
            let hole = rect([
                solid.left() + x,
                solid.top() + y,
                solid.size.width - x * 2.,
                solid.size.height - y * 2.,
            ])
            .intersect(area);
            // A strip that the mask cut would draw differently in another mask, so such a shadow stays whole.
            let cut = clip.bounds.intersect(area) != area;
            if cut || hole.size.width < 64. || hole.size.height < 64. {
                self.scene.push(instance);
                continue;
            }
            for strip in [
                Rect::between(area.origin, point(area.right(), hole.top())),
                Rect::between(point(area.left(), hole.bottom()), area.bottom_right()),
                Rect::between(point(area.left(), hole.top()), hole.bottom_left()),
                Rect::between(hole.top_right(), point(area.right(), hole.bottom())),
            ] {
                if !strip.is_empty() {
                    instance.clip = strip.to_array();
                    self.scene.push(instance);
                }
            }
        }
    }

    pub fn with_effect<R>(&mut self, effect: PaintEffect, f: impl FnOnce(&mut Self) -> R) -> R {
        // At opacity 0 nothing inside draws, but `f` still registers hitboxes and listeners.
        if self.opacity <= 0. {
            return f(self);
        }
        let scale = self.scale_factor;
        let content_mask = self.content_mask().scale(scale);
        let mask = effect.mask.and_then(|(image, bounds)| {
            let tile = self.image_tile(&image)?;
            Some((tile, bounds.scale(scale)))
        });
        let state = EffectState {
            transform: effect.transform.map(|matrix| device_matrix(matrix, scale)),
            opacity: effect.opacity.clamp(0., 1.),
            color_matrix: effect.color_matrix,
            blur: effect.blur.0 * scale,
            shadow: effect.shadow.map(|(offset, blur, color)| {
                (
                    point(offset.x.0 * scale, offset.y.0 * scale),
                    blur.0 * scale,
                    color,
                )
            }),
            blend_mode: effect.blend_mode,
            clip: effect
                .clip
                .map(|(bounds, radii)| (bounds.scale(scale), radii.fit(bounds.size).scale(scale))),
            mask,
            backdrop: effect.backdrop.map(
                |Backdrop {
                     bounds,
                     corner_radii,
                     blur,
                     color_matrix,
                 }| BackdropState {
                    bounds: bounds.scale(scale),
                    corner_radii: corner_radii.fit(bounds.size).scale(scale),
                    blur: blur.0 * scale,
                    color_matrix,
                },
            ),
            glass: effect.glass.map(|glass| crate::gpu::scene::GlassState {
                key: glass.key,
                bounds: glass.bounds.scale(scale),
                corner_radius: glass.corner_radius.0 * scale,
                tint: glass.tint,
                clear: glass.clear,
                interactive: glass.interactive,
            }),
            content_mask,
            text_mask: effect
                .text_mask
                .then(|| self.text_clip.map(|(layer, _)| layer))
                .flatten(),
        };
        let outer = self.space.take();
        let Space {
            joins,
            preserves,
            hidden,
        } = effect.space;
        let mut state = state;
        let mut text_clip = None;
        let root = match outer.as_deref().filter(|_| joins) {
            // The plane around it ends, and a new one continues it after this plane.
            Some(space) => {
                state.content_mask = space.plane.content_mask;
                self.scene.end_plane(space.hidden);
                text_clip = self.text_clip.take();
                false
            }
            None => preserves,
        };
        let start = self.scene.nested_len();
        if root {
            self.planes.push(Vec::new());
        }
        if root || joins && outer.is_some() {
            self.begin_plane(effect.plane.scale(scale));
        }
        if let Some(matrix) = effect.transform.filter(|_| preserves) {
            self.space = Some(Box::new(PlaneSpace {
                matrix,
                plane: state.clone(),
                bounds: effect.plane.scale(scale),
                depth: self.scene.depth() + 1,
                hidden,
            }));
        }
        self.scene.push_effect(state, effect.key);
        // Ancestor clips apply after the effect, in window space, so the layer starts unclipped.
        self.push_content_mask(unbounded_mask());
        let groups = mem::take(&mut self.group_clips);
        let outer_scale = self.transform_scale;
        if let Some(m) = effect.transform {
            let axis = |x: f32, y: f32| (x * x + y * y).sqrt();
            let scale = axis(m[0], m[1]).max(axis(m[4], m[5]));
            if scale.is_finite() {
                self.transform_scale *= scale;
            }
        }
        let result = f(self);
        self.transform_scale = outer_scale;
        self.group_clips = groups;
        self.content_masks.pop();
        self.scene.end_plane(hidden);
        self.space = outer;
        if root {
            self.sort_planes(start);
        } else if let Some(space) = self.space.as_deref().filter(|_| joins) {
            let (plane, bounds) = (space.plane.clone(), space.bounds);
            self.begin_plane(bounds);
            self.scene.push_effect(plane, None);
            self.text_clip = text_clip;
        }
        result
    }

    /// The matrix of the 3D rendering context that a layer here can join.
    pub fn space(&self) -> Option<&[f32; 16]> {
        (self.space.as_deref())
            .filter(|space| space.depth == self.scene.depth())
            .map(|space| &space.matrix)
    }

    fn begin_plane(&mut self, bounds: Rect<f32>) {
        let plane = (self.scene.next_layer(), bounds, self.pending.lens());
        if let Some(planes) = self.planes.last_mut() {
            planes.push(plane);
        }
    }

    // Draws the planes of a context from back to front, and gives them the pointer in the reverse order.
    fn sort_planes(&mut self, start: usize) {
        let Some(planes) = self.planes.pop() else {
            return;
        };
        let last = self.pending.lens();
        let ends = planes.iter().skip(1).map(|plane| plane.2).chain([last]);
        let parts: Vec<_> = (planes.iter().zip(ends))
            .map(|(plane, end)| [1, 3].map(|at| [plane.2[at], end[at]]))
            .collect();
        // A plane that draws nothing takes its place for the pointer.
        let listed = (planes.iter().zip(&parts))
            .filter(|(_, [_, marks])| marks[0] < marks[1])
            .map(|(&(layer, bounds, _), _)| (layer, bounds));
        let order = self.scene.sort_planes(start, listed);
        let mut parts: Vec<_> = (planes.iter().zip(parts))
            .map(|(plane, part)| (order.iter().rposition(|drawn| *drawn == plane.0), part))
            .collect();
        if !parts.is_sorted_by_key(|part| part.0) {
            parts.sort_by_key(|part| part.0);
            let order = parts.into_iter().map(|part| part.1).collect();
            self.pending.sorted.push(order);
        }
    }

    /// Glyphs and text lines that `f` draws outside nested effects also draw into a mask, which an effect
    /// with `text_mask` in `f` shows its content through, as `background-clip: text`.
    pub fn with_text_clip<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let layer = self.scene.mask_layer();
        let outer = self.text_clip.replace((layer, self.scene.depth()));
        let result = f(self);
        self.text_clip = outer;
        result
    }

    /// Text that `f` draws, like a text shadow, takes no part in the mask of `with_text_clip`.
    pub fn without_text_clip<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let outer = self.text_clip.take();
        let result = f(self);
        self.text_clip = outer;
        result
    }

    fn push_text(&mut self, instance: Instance) {
        if let Some((layer, depth)) = self.text_clip
            && depth == self.scene.depth()
        {
            let mut mask = instance;
            match instance.kind() {
                IMAGE => mask.info[2] = 1.,
                GLYPH => (mask.color, mask.info[2]) = ([0., 0., 0., 1.], 1.),
                _ => mask.color = [0., 0., 0., 1.],
            }
            self.scene.push_into(layer, mask);
        }
        if instance.kind() == IMAGE || instance.color[3] > 0. {
            self.scene.push(instance);
        }
    }

    /// Whether transparent text still draws, into the mask of `with_text_clip`.
    pub fn clips_text(&self) -> bool {
        self.text_clip.is_some()
    }

    // `origin` is the top left of the line.
    pub fn draw_line(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        thickness: Pixels,
        color: Color,
        style: DecorationStyle,
    ) {
        self.draw_line_between(origin, width, thickness, color, style, &[]);
    }

    /// Like `draw_line`, without the parts in `gaps`, which are in order.
    pub fn draw_line_between(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        thickness: Pixels,
        color: Color,
        style: DecorationStyle,
        gaps: &[[Pixels; 2]],
    ) {
        if self.opacity <= 0. {
            return;
        }
        let scale = self.scale_factor;
        let wavy = style == DecorationStyle::Wavy;
        let thickness = if wavy {
            (thickness.0 * scale).max(1.)
        } else {
            self.snap_stroke(thickness)
        };
        let clip = self.snapped_content_mask();
        let mut rect = Rect::new(
            origin.map(|value| snap(value.0 * scale)),
            size(self.snap_stroke(width), thickness),
        );
        if wavy {
            let reach = wave_reach(thickness).ceil() + 1.;
            rect.origin.y += thickness + 1. - reach;
            rect.size.height = reach * 2.;
        }
        let mut line = Instance::new(LINE, rect, clip);
        line.color = color.fade(self.opacity).to_array();
        line.info[2] = thickness;
        // Like Chrome, dashes fit the line with one at each end, and dots wider than 3 pixels are round.
        let dots = style == DecorationStyle::Dotted;
        let ratios = match style {
            DecorationStyle::Dotted => Some([1., 1.]),
            DecorationStyle::Dashed if thickness >= 3. => Some([2., 1.]),
            DecorationStyle::Dashed => Some([3., 2.]),
            _ => None,
        };
        line.info[1] = f32::from(u8::from(wavy));
        if let Some([dash, gap]) = ratios.map(|ratios| ratios.map(|ratio| ratio * thickness))
            && rect.size.width > dash * 2.
        {
            let length = rect.size.width;
            let count = ((length + gap) / (dash + gap)).floor();
            let fit = |count: f32| (length - count * dash) / (count - 1.);
            let (wide, narrow) = (fit(count), fit(count + 1.));
            let gap = if narrow <= 0. || (wide - gap).abs() < (narrow - gap).abs() {
                wide
            } else {
                narrow
            };
            line.info[1] = if dots && thickness > 3. { 4. } else { 2. };
            (line.aux[0], line.aux[1]) = (dash, dash + gap);
        }
        for _ in 0..1 + usize::from(style == DecorationStyle::Double) {
            let full = crate::gpu::rect(line.rect);
            let shown = full.intersect(clip.bounds);
            if shown != full {
                self.scene.cull();
            }
            if gaps.is_empty() && !shown.is_empty() {
                self.push_text(line);
            } else if !shown.is_empty() {
                let mut left = shown.left();
                for at in 0..=gaps.len() {
                    let [end, next] = (gaps.get(at))
                        .map_or([shown.right(); 2], |gap| gap.map(|x| snap(x.0 * scale)));
                    let end = end.min(shown.right());
                    if left < end {
                        let mut piece = line;
                        piece.clip = [left, shown.top(), end - left, shown.size.height];
                        self.push_text(piece);
                    }
                    left = left.max(next);
                }
            }
            line.rect[1] += thickness + 1.;
        }
    }

    // An emoji draws in its own colors. A `stroke` above 0 draws a line of that width along the outline instead.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_glyph(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Color,
        emoji: bool,
        stroke: Pixels,
    ) {
        if self.opacity <= 0. {
            return;
        }
        let scale = self.scale_factor;
        let origin = origin.scale(scale);
        let variants = SUBPIXEL_STEPS as f32;
        let (at, subpixel) = if emoji {
            (origin.map(snap), 0)
        } else {
            // Like Skia, halves round up, so a baseline at y.5 draws on the same row as in Chromium.
            let x = (origin.x * variants + 0.5).floor() / variants;
            (
                point(x.floor(), (origin.y + 0.5).floor()),
                ((x - x.floor()) * variants) as u8,
            )
        };
        let params = GlyphRaster {
            font: font_id,
            glyph: glyph_id,
            size: font_size,
            scale,
            subpixel,
            emoji,
            dilation: if self.font_smoothing && !emoji {
                self.text_system.dilation(color)
            } else {
                0
            },
            stroke: stroke.0,
        };
        let text = &self.text_system;
        let Some((tile, shown)) =
            self.atlas
                .get_or_insert_with(&TileKey::Glyph(params), &mut || {
                    Ok(text
                        .raster(&params, crate::text::MAX_GLYPH_SIDE)?
                        .map(|(shown, size, bytes)| (shown, size, Cow::Owned(bytes))))
                })
        else {
            return;
        };
        let rect = Rect {
            origin: at + shown.origin.map(|value| value as f32),
            size: shown.size.map(|value| value as f32),
        };
        let clip = self.snapped_content_mask();
        let instance = if emoji {
            self.sprite(IMAGE, rect, clip, tile)
        } else {
            let mut instance = self.sprite(GLYPH, rect, clip, tile);
            instance.color = color.fade(self.opacity).to_array();
            instance
        };
        self.push_text(instance);
    }

    fn sprite(
        &self,
        kind: u32,
        rect: Rect<f32>,
        clip: Clip<f32>,
        tile: crate::gpu::atlas::Tile,
    ) -> Instance {
        let mut instance = Instance::new(kind, rect, clip);
        instance.aux = tile.rect.map(|value| value as f32).to_array();
        instance.info[2] = self.opacity;
        instance.info[3] = tile.page as f32;
        instance
    }

    fn image_tile(&self, image: &RenderImage) -> Option<crate::gpu::atlas::Tile> {
        #[cfg(target_os = "linux")]
        self.atlas.insert_surface(image);
        self.atlas
            .get_or_insert_with(&TileKey::Image(image.id), &mut || {
                let size = image.size();
                Ok(Some((
                    Rect::new(Point::default(), size),
                    size,
                    Cow::Borrowed(image.bytes()),
                )))
            })
            .map(|(tile, _)| tile)
    }

    pub fn draw_image(
        &mut self,
        bounds: Rect<Pixels>,
        image_bounds: Rect<Pixels>,
        radii: impl Into<Radii<Pixels>>,
        image: Arc<RenderImage>,
        grayscale: bool,
        pixelated: bool,
    ) {
        if self.opacity <= 0. {
            return;
        }
        let visible = bounds.intersect(image_bounds);
        if visible.is_empty() || image_bounds.is_empty() {
            return;
        }
        let Some(mut tile) = self.image_tile(&image) else {
            return;
        };
        if visible != image_bounds {
            let full = tile.rect;
            let texels = |offset: Pixels, whole: Pixels, length: i32| {
                (offset / whole * length as f32).round() as i32
            };
            let (origin, whole) = (image_bounds.origin, image_bounds.size);
            let left = (full.origin.x
                + texels(visible.origin.x - origin.x, whole.width, full.size.width))
            .clamp(full.origin.x, full.right());
            let top = (full.origin.y
                + texels(visible.origin.y - origin.y, whole.height, full.size.height))
            .clamp(full.origin.y, full.bottom());
            let width = texels(visible.size.width, whole.width, full.size.width)
                .min(full.right() - left)
                .max(0);
            let height = texels(visible.size.height, whole.height, full.size.height)
                .min(full.bottom() - top)
                .max(0);
            tile.rect = Rect::new(point(left, top), size(width, height));
        }
        let mut instance = self.sprite(
            IMAGE,
            self.snap_bounds(visible),
            self.snapped_content_mask(),
            tile,
        );
        let radii = radii.into().fit(visible.size);
        instance.radii = radii.scale(self.scale_factor).to_array();
        [instance.shape[0], instance.shape[1]] = radii.shape_array();
        instance.info[1] = f32::from(
            u8::from(grayscale)
                | u8::from(image.is_premultiplied()) << 1
                | u8::from(pixelated) << 2
                | u8::from(image.is_display_p3()) << 3,
        );
        self.scene.push(instance);
    }

    pub fn release_image(&mut self, image: Arc<RenderImage>) {
        self.atlas.remove(&TileKey::Image(image.id));
    }
}
