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
        instance.radii = radii.into().scale(self.scale_factor).to_array();
        instance.color = fill.fade(self.opacity).to_array();
        instance.border_color = border.color.fade(self.opacity).to_array();
        instance.aux = [widths.top, widths.right, widths.bottom, widths.left];
        instance.info[1] = (border.style as u32 | u32::from(border.sides) << 8) as f32;
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
        let reach = [
            r[4].max(r[5]),
            r[1].max(r[2]),
            r[6].max(r[7]),
            r[0].max(r[3]),
        ];
        // One side alone draws inside its own band.
        if false {
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
            if !part.is_empty() {
                instance.clip = part.to_array();
                self.scene.push(instance);
            }
            return;
        }
        // A side with no border and no corner needs no strip, so the hole reaches the edge there.
        let pad = |edge: f32| edge + 1.;
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
                (
                    (bounds + shadow.offset).outset(shadow.spread_radius),
                    Radii {
                        x: radii.x.map(grow),
                        y: radii.y.map(grow),
                    },
                )
            };
            let mut instance = Instance::new(SHADOW, self.cover_bounds(shape), clip);
            instance.radii = shape_radii.scale(scale).to_array();
            instance.color = shadow.color.fade(self.opacity).to_array();
            instance.aux = caster;
            instance.aux_radii = caster_radii;
            instance.info[1] = f32::from(u8::from(inset));
            instance.info[2] = shadow.blur_radius.scale(scale);
            self.scene.push(instance);
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
            glass: effect.glass.map(|glass| crate::scene::GlassState {
                key: glass.key,
                bounds: glass.bounds.scale(scale),
                corner_radius: glass.corner_radius.0 * scale,
                tint: glass.tint,
                clear: glass.clear,
                interactive: glass.interactive,
            }),
            content_mask,
        };
        self.scene.push_effect(state, effect.key);
        // Ancestor clips apply after the effect, in window space, so the layer starts unclipped.
        self.push_content_mask(unbounded_mask());
        let result = f(self);
        self.content_masks.pop();
        self.scene.pop_effect();
        result
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
        if self.opacity <= 0. {
            return;
        }
        let thickness = self.snap_stroke(thickness);
        let clip = self.snapped_content_mask();
        let rows = match style {
            DecorationStyle::Wavy => 3.,
            _ => 1.,
        };
        let mut instance = Instance::new(
            LINE,
            Rect::new(
                origin.map(|value| snap(value.0 * self.scale_factor)),
                size(self.snap_stroke(width), thickness * rows),
            ),
            clip,
        );
        instance.color = color.fade(self.opacity).to_array();
        instance.info[1] = f32::from(u8::from(style == DecorationStyle::Wavy));
        instance.info[2] = thickness;
        let dash = match style {
            DecorationStyle::Dotted => thickness,
            DecorationStyle::Dashed => thickness * 2.,
            _ => 0.,
        };
        for row in [0., 2.].iter().take(if style == DecorationStyle::Double {
            2
        } else {
            1
        }) {
            let mut line = instance;
            line.rect[1] += thickness * row;
            let shown = crate::gpu::rect(line.rect).intersect(clip.bounds);
            if shown.is_empty() {
                continue;
            }
            if dash <= 0. {
                self.scene.push(line);
                continue;
            }
            let period = dash * 2.;
            let skipped = 0.;
            let mut start = line.rect[0] + skipped * period;
            while start < shown.right() {
                let left = start.max(shown.left());
                let right = (start + dash).min(shown.right());
                if left < right {
                    line.clip = [left, shown.top(), right - left, shown.size.height];
                    self.scene.push(line);
                }
                start += period;
            }
        }
    }

    // An emoji draws in its own colors.
    pub fn draw_glyph(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Color,
        emoji: bool,
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
        };
        let text = &self.text_system;
        let Some((tile, shown)) =
            self.platform
                .atlas()
                .get_or_insert_with(&TileKey::Glyph(params), &mut || {
                    Ok(text
                        .raster(&params, crate::atlas::MAX_GLYPH_SIDE)?
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
        self.scene.push(instance);
    }

    fn sprite(
        &self,
        kind: u32,
        rect: Rect<f32>,
        clip: Clip<f32>,
        tile: crate::atlas::Tile,
    ) -> Instance {
        let mut instance = Instance::new(kind, rect, clip);
        instance.aux = tile.rect.map(|value| value as f32).to_array();
        instance.info[2] = self.opacity;
        instance.info[3] = tile.page as f32;
        instance
    }

    fn image_tile(&self, image: &RenderImage) -> Option<crate::atlas::Tile> {
        self.platform
            .atlas()
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

    pub fn draw_symbol(&mut self, name: &str, bounds: Rect<Pixels>, color: Color) {
        if self.opacity <= 0. {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            if bounds.is_empty() || color.a <= 0. {
                return;
            }
            let rect = self.snap_bounds(bounds);
            let [width, height] = [rect.size.width, rect.size.height];
            if !width.is_finite() || !height.is_finite() || width <= 0. || height <= 0. {
                return;
            }
            let scale = (1024. / width.max(height)).min(1.);
            let pixels = [
                (width * scale).ceil() as u16,
                (height * scale).ceil() as u16,
            ];
            let image = self.symbols.get(name, pixels, self.frames_drawn);
            for id in self.symbols.retired.drain(..) {
                self.platform.atlas().remove(&TileKey::Symbol(id));
            }
            let Some(image) = image else { return };
            let tile =
                self.platform
                    .atlas()
                    .get_or_insert_with(&TileKey::Symbol(image.id), &mut || {
                        let size = size(i32::from(image.size[0]), i32::from(image.size[1]));
                        Ok(Some((
                            Rect::new(Point::default(), size),
                            size,
                            Cow::Borrowed(&image.bytes),
                        )))
                    });
            if let Some((tile, _)) = tile {
                let mut instance = self.sprite(GLYPH, rect, self.snapped_content_mask(), tile);
                instance.color = color.fade(self.opacity).to_array();
                self.scene.push(instance);
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (name, bounds, color);
    }

    pub fn draw_image(
        &mut self,
        bounds: Rect<Pixels>,
        image_bounds: Rect<Pixels>,
        radii: impl Into<Radii<Pixels>>,
        image: Arc<RenderImage>,
        grayscale: bool,
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
        instance.radii = radii
            .into()
            .fit(visible.size)
            .scale(self.scale_factor)
            .to_array();
        instance.info[1] = f32::from(u8::from(grayscale) | u8::from(image.is_premultiplied()) << 1);
        self.scene.push(instance);
    }

    pub fn release_image(&mut self, image: Arc<RenderImage>) {
        self.platform.atlas().remove(&TileKey::Image(image.id));
    }
}
