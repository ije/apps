use anyhow::{Context, Result, ensure};
use std::rc::Rc;
use std::sync::Arc;
use tiny_skia as skia;

pub mod path;
pub mod text;

use super::{RASTER_MAX, render_image};
use path::{ARGS, concat, fill_area, matrix, quad, replay, stroke_space, trace};
use text::{ink, line};

#[derive(Clone)]
struct State {
    // F64, so a far translation and a far point cancel exactly.
    transform: [f64; 6],
    fill: Brush,
    stroke: Brush,
    alpha: f32,
    width: f32,
    cap: skia::LineCap,
    join: skia::LineJoin,
    miter: f32,
    dash: Vec<f32>,
    dash_offset: f32,
    blend: skia::BlendMode,
    smoothing: bool,
    spacing: [f32; 2],
    font: Rc<str>,
    align: f32,
    baseline: u8,
    clip: Option<Rc<skia::Mask>>,
    shadow: (skia::Color, f32, [f32; 2]),
    /// The standard deviation of `filter: blur()`, in canvas pixels.
    blur: f32,
}

impl State {
    fn shadowed(&self) -> bool {
        let (color, blur, offset) = self.shadow;
        color.alpha() > 0. && (blur > 0. || offset != [0.; 2])
    }
}

#[derive(Clone)]
enum Brush {
    Shader(skia::Shader<'static>),
    Pattern(Rc<skia::Pixmap>, [bool; 2], skia::Transform),
}

fn paint(
    source: &Brush,
    smooth: bool,
    alpha: f32,
    blend_mode: skia::BlendMode,
    transform: skia::Transform,
) -> skia::Paint<'_> {
    let mut shader = match source {
        Brush::Shader(shader) => shader.clone(),
        Brush::Pattern(pixels, _, m) => skia::Pattern::new(
            pixels.as_ref().as_ref(),
            skia::SpreadMode::Repeat,
            if smooth {
                skia::FilterQuality::Bilinear
            } else {
                skia::FilterQuality::Nearest
            },
            1.,
            *m,
        ),
    };
    shader.apply_opacity(alpha);
    shader.transform(transform);
    skia::Paint {
        shader,
        blend_mode,
        ..Default::default()
    }
}

fn strip<'a>(
    source: &Brush,
    clip: Option<&'a skia::Mask>,
    t: [f64; 6],
    [width, height]: [u32; 2],
) -> Option<std::borrow::Cow<'a, skia::Mask>> {
    let Brush::Pattern(pixels, [x @ false, y] | [x, y @ false], m) = source else {
        return clip.map(std::borrow::Cow::Borrowed);
    };
    let mut mask = match clip {
        Some(clip) => clip.clone(),
        None => {
            let mut mask = skia::Mask::new(width, height)?;
            mask.data_mut().fill(255);
            mask
        }
    };
    let to_device = matrix(t).pre_concat(*m);
    let seen = skia::Rect::from_xywh(0., 0., width as f32, height as f32)?
        .transform(to_device.invert()?)?;
    match skia::Rect::from_ltrb(
        if *x { seen.left() } else { 0. },
        if *y { seen.top() } else { 0. },
        if *x {
            seen.right()
        } else {
            pixels.width() as f32
        },
        if *y {
            seen.bottom()
        } else {
            pixels.height() as f32
        },
    ) {
        Some(rect) => mask.intersect_path(
            &skia::PathBuilder::from_rect(rect),
            skia::FillRule::Winding,
            true,
            to_device,
        ),
        None => mask.data_mut().fill(0),
    }
    Some(std::borrow::Cow::Owned(mask))
}

fn shadow(
    layer: &skia::Pixmap,
    (color, blur, [dx, dy]): (skia::Color, f32, [f32; 2]),
) -> Option<(skia::Pixmap, i32, i32)> {
    let width = layer.width() as usize;
    let [mut left, mut top, mut right, mut bottom] = [usize::MAX, usize::MAX, 0, 0];
    for (y, row) in layer.pixels().chunks_exact(width).enumerate() {
        if let Some(first) = row.iter().position(|p| p.alpha() > 0) {
            let last = row.iter().rposition(|p| p.alpha() > 0).unwrap_or(first);
            [left, right, top, bottom] = [left.min(first), right.max(last + 1), top.min(y), y + 1];
        }
    }
    if right <= left {
        return None;
    }
    // Larger blurs cost memory past the canvas and are close to invisible.
    let blur = blur.min(256.);
    let pad = (blur * 1.5).ceil() as usize;
    let mut shadow = skia::Pixmap::new(
        (right - left + 2 * pad) as u32,
        (bottom - top + 2 * pad) as u32,
    )?;
    let stride = shadow.width() as usize;
    let tint: Vec<_> = (0..=255u8)
        .map(|alpha| {
            let mut color = color;
            color.apply_opacity(f32::from(alpha) / 255.);
            color.premultiply().to_color_u8()
        })
        .collect();
    let out = shadow.pixels_mut();
    for y in top..bottom {
        for x in left..right {
            out[(y - top + pad) * stride + x - left + pad] =
                tint[layer.pixels()[y * width + x].alpha() as usize];
        }
    }
    super::resvg::blur(&mut shadow, blur / 2.);
    Some((
        shadow,
        (left as f32 - pad as f32 + dx).round() as i32,
        (top as f32 - pad as f32 + dy).round() as i32,
    ))
}

pub struct Canvas {
    pub pixels: skia::Pixmap,
    pub image: Arc<renderer::RenderImage>,
    pub present: bool,
    state: State,
    stack: Vec<State>,
    path: skia::PathBuilder,
    glyphs: rustc_hash::FxHashMap<GlyphKey, Option<Glyph>>,
    glyph_bytes: usize,
    paths: rustc_hash::FxHashMap<u64, Path2D>,
}

type GlyphKey = (u32, u16, [u32; 3], u8, bool);

type Glyph = (renderer::Point<i32>, renderer::Size<i32>, Vec<u8>);

type Path2D = (Vec<f64>, [f64; 6], Option<skia::Path>);

fn gradient(spec: &str) -> Option<skia::Shader<'static>> {
    let (shape, stops): (Vec<f32>, Vec<(f32, String)>) = serde_json::from_str(spec).ok()?;
    let stops = stops
        .iter()
        .filter_map(|(offset, color)| {
            let (rgb, alpha) = crate::css::value::color::color(color)?;
            let [_, r, g, b] = rgb.to_be_bytes();
            Some(skia::GradientStop::new(
                *offset,
                skia::Color::from_rgba8(r, g, b, alpha),
            ))
        })
        .collect();
    let identity = skia::Transform::identity();
    // tiny-skia paints a solid color for these, where the spec paints nothing.
    match shape[..] {
        [x0, y0, x1, y1] if x0 == x1 && y0 == y1 => None,
        [x0, y0, r0, x1, y1, r1] if x0 == x1 && y0 == y1 && r0 == r1 => None,
        [x0, y0, x1, y1] => skia::LinearGradient::new(
            (x0, y0).into(),
            (x1, y1).into(),
            stops,
            skia::SpreadMode::Pad,
            identity,
        ),
        [angle, x, y] => skia::SweepGradient::new(
            (0., 0.).into(),
            0.,
            360.,
            stops,
            skia::SpreadMode::Pad,
            skia::Transform::from_translate(x, y).pre_rotate(angle.to_degrees()),
        ),
        [x0, y0, r0, x1, y1, r1] => skia::RadialGradient::new(
            (x0, y0).into(),
            r0,
            (x1, y1).into(),
            r1,
            stops,
            skia::SpreadMode::Pad,
            identity,
        ),
        _ => None,
    }
}

pub enum Source<'a> {
    Pixels(&'a skia::Pixmap),
    Shared(Rc<skia::Pixmap>),
}

impl std::ops::Deref for Source<'_> {
    type Target = skia::Pixmap;

    fn deref(&self) -> &skia::Pixmap {
        match self {
            Source::Pixels(pixels) => pixels,
            Source::Shared(pixels) => pixels,
        }
    }
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        ensure!(
            width > 0
                && height > 0
                && width <= 16384
                && height <= 16384
                && u64::from(width) * u64::from(height) <= 16_777_216,
            "Canvas dimensions exceed the pixel limit"
        );
        let pixels = skia::Pixmap::new(width, height).context("Cannot allocate canvas")?;
        // The first draw makes the image to show. A canvas that is never shown never needs one.
        let image = Arc::new(renderer::RenderImage::new(0, 0, Vec::new()));
        Ok(Self {
            pixels,
            image,
            present: true,
            state: State {
                transform: [1., 0., 0., 1., 0., 0.],
                fill: Brush::Shader(skia::Shader::SolidColor(skia::Color::BLACK)),
                stroke: Brush::Shader(skia::Shader::SolidColor(skia::Color::BLACK)),
                alpha: 1.,
                width: 1.,
                cap: skia::LineCap::Butt,
                join: skia::LineJoin::Miter,
                miter: 10.,
                dash: Vec::new(),
                dash_offset: 0.,
                blend: skia::BlendMode::SourceOver,
                smoothing: true,
                spacing: [0.; 2],
                font: "10px sans-serif".into(),
                align: 0.,
                baseline: 0,
                clip: None,
                shadow: (skia::Color::TRANSPARENT, 0., [0.; 2]),
                blur: 0.,
            },
            stack: Vec::new(),
            path: skia::PathBuilder::new(),
            glyphs: Default::default(),
            glyph_bytes: 0,
            paths: Default::default(),
        })
    }

    pub fn draw<'a>(
        &mut self,
        ops: &[f64],
        mut strings: &str,
        data: &[&[u8]],
        image: &dyn Fn((usize, [u32; 2])) -> Option<Source<'a>>,
        fonts: Option<(
            &renderer::TextSystem,
            &std::collections::HashMap<String, String>,
        )>,
    ) -> Result<Option<Arc<renderer::RenderImage>>> {
        // Shadows and modes that change pixels outside the shape draw the shape into a layer that covers the
        // canvas. A new canvas-sized buffer costs page faults, so one batch reuses its layer through `spare`.
        let (mut covered, mut spare): (Option<skia::Pixmap>, Option<skia::Pixmap>) = (None, None);
        let finish = |pixels: &mut skia::Pixmap, saved: skia::Pixmap, state: &State| {
            let mut layer = std::mem::replace(pixels, saved);
            if state.blur > 0. {
                super::resvg::blur(&mut layer, state.blur.min(256.));
            }
            if let Some((shadow, x, y)) = shadow(&layer, state.shadow).filter(|_| state.shadowed())
            {
                pixels.draw_pixmap(
                    x,
                    y,
                    shadow.as_ref(),
                    &skia::PixmapPaint {
                        blend_mode: state.blend,
                        ..Default::default()
                    },
                    skia::Transform::identity(),
                    state.clip.as_deref(),
                );
            }
            pixels.draw_pixmap(
                0,
                0,
                layer.as_ref(),
                &skia::PixmapPaint {
                    blend_mode: state.blend,
                    ..Default::default()
                },
                skia::Transform::identity(),
                state.clip.as_deref(),
            );
            layer
        };
        let mut string = |units: f64| {
            let (part, rest) =
                strings.split_at(crate::dom::text::utf16_index(strings, units as usize));
            strings = rest;
            part
        };
        // A call that draws nothing must not clear the canvas, so a covering mode puts the canvas back.
        macro_rules! skip {
            ($pixels:expr) => {{
                if let Some(saved) = covered.take() {
                    spare = Some(std::mem::replace(&mut $pixels, saved));
                }
                continue;
            }};
        }
        let identity = skia::Transform::identity();
        let mut sources = rustc_hash::FxHashMap::default();
        let [width, height] = [self.pixels.width(), self.pixels.height()];
        let mut rest = ops;
        while let [code, tail @ ..] = rest {
            if let Some(saved) = covered.take() {
                spare = Some(finish(&mut self.pixels, saved, &self.state));
            }
            let count = *ARGS
                .get(*code as usize)
                .context("Invalid canvas operation")?;
            ensure!(tail.len() >= count, "Invalid canvas operation");
            let (a, tail) = tail.split_at(count);
            rest = tail;
            let (code, a, shape) = if *code == 42. {
                let t = self.state.transform;
                let shape = self
                    .paths
                    .get_mut(&(a[0] as u64))
                    .and_then(|(ops, at, built)| {
                        if *at != t {
                            (*at, *built) = (t, replay(ops, t));
                        }
                        built.clone()
                    });
                (a[1], &a[2..], Some(shape))
            } else {
                (*code, a, None)
            };
            let blend = if matches!(code as u8, 18 | 19 | 21 | 22 | 24 | 25 | 27)
                && (self.state.shadowed()
                    || self.state.blur > 0.
                    || matches!(
                        self.state.blend,
                        skia::BlendMode::SourceIn
                            | skia::BlendMode::SourceOut
                            | skia::BlendMode::DestinationIn
                            | skia::BlendMode::DestinationAtop
                            | skia::BlendMode::Source
                    )) {
                let layer = match spare.take() {
                    Some(mut layer) => {
                        layer.fill(skia::Color::TRANSPARENT);
                        layer
                    }
                    None => {
                        skia::Pixmap::new(width, height).context("Cannot allocate canvas layer")?
                    }
                };
                covered = Some(std::mem::replace(&mut self.pixels, layer));
                skia::BlendMode::SourceOver
            } else {
                self.state.blend
            };
            let state = &mut self.state;
            let t = state.transform;
            match code as u8 {
                0 => self.stack.push(state.clone()),
                1 => {
                    if let Some(saved) = self.stack.pop() {
                        *state = saved;
                    }
                }
                2 | 3 if a.iter().all(|v| v.is_finite()) => {
                    let m = [a[0], a[1], a[2], a[3], a[4], a[5]];
                    state.transform = if code == 2. { concat(t, m) } else { m };
                }
                2 | 3 => {}
                4 | 5 => {
                    let value = string(a[0]);
                    let source = if let Some(spec) = value.strip_prefix("gradient:") {
                        Some(Brush::Shader(gradient(spec).unwrap_or(
                            skia::Shader::SolidColor(skia::Color::TRANSPARENT),
                        )))
                    } else if let Some(spec) = value.strip_prefix("pattern:") {
                        let [id, w, h, x, y, m @ ..]: [f64; 11] =
                            serde_json::from_str(spec).context("Invalid canvas pattern")?;
                        let pixels = if id < 0. {
                            Some(Rc::new(self.pixels.clone()))
                        } else {
                            let key = (id as usize, [w as u32, h as u32]);
                            match sources.entry(key).or_insert_with(|| image(key)) {
                                Some(Source::Shared(pixels)) => Some(pixels.clone()),
                                Some(Source::Pixels(pixels)) => Some(Rc::new((*pixels).clone())),
                                None => None,
                            }
                        };
                        pixels.map(|pixels| {
                            let m = matrix(m).pre_scale(
                                (w / f64::from(pixels.width())) as f32,
                                (h / f64::from(pixels.height())) as f32,
                            );
                            Brush::Pattern(pixels, [x != 0., y != 0.], m)
                        })
                    } else {
                        crate::css::value::color::color(value).map(|(rgb, alpha)| {
                            let [_, r, g, b] = rgb.to_be_bytes();
                            Brush::Shader(skia::Shader::SolidColor(skia::Color::from_rgba8(
                                r, g, b, alpha,
                            )))
                        })
                    };
                    if let Some(source) = source {
                        if code == 4. {
                            state.fill = source;
                        } else {
                            state.stroke = source;
                        }
                    }
                }
                6 => state.font = string(a[0]).into(),
                7 => {
                    state.align = match string(a[0]) {
                        "center" => 0.5,
                        "end" | "right" => 1.,
                        _ => 0.,
                    }
                }
                8 => {
                    state.baseline = match string(a[0]) {
                        "top" => 1,
                        "middle" => 2,
                        "bottom" => 3,
                        "hanging" => 4,
                        "ideographic" => 5,
                        _ => 0,
                    }
                }
                9 => state.alpha = a[0] as f32,
                10 => state.width = a[0] as f32,
                11 => self.path.clear(),
                12..=17 | 28..=31 => trace(&mut self.path, t, code as u8, a),
                20 => {
                    let mut mask = match state.clip.take() {
                        Some(mask) => Rc::unwrap_or_clone(mask),
                        None => {
                            let mut mask = skia::Mask::new(width, height)
                                .context("Cannot allocate clip mask")?;
                            mask.data_mut().fill(255);
                            mask
                        }
                    };
                    let rule = if a[0] == 1. {
                        skia::FillRule::EvenOdd
                    } else {
                        skia::FillRule::Winding
                    };
                    match shape.unwrap_or_else(|| self.path.clone().finish()) {
                        Some(path) => mask.intersect_path(&path, rule, true, identity),
                        None => mask.data_mut().fill(0),
                    }
                    state.clip = Some(Rc::new(mask));
                }
                18 | 21 | 23 => {
                    let mask = if code == 23. {
                        state.clip.as_deref().map(std::borrow::Cow::Borrowed)
                    } else {
                        strip(&state.fill, state.clip.as_deref(), t, [width, height])
                    };
                    let mask = mask.as_deref();
                    // tiny-skia's `Clear` ignores the clip mask.
                    let paint = if code == 23. {
                        skia::Paint {
                            blend_mode: skia::BlendMode::DestinationOut,
                            ..Default::default()
                        }
                    } else {
                        paint(&state.fill, state.smoothing, state.alpha, blend, matrix(t))
                    };
                    if code != 18. {
                        fill_area(&mut self.pixels, t, [a[0], a[1], a[2], a[3]], &paint, mask);
                    } else {
                        let Some(path) = shape.unwrap_or_else(|| self.path.clone().finish()) else {
                            skip!(self.pixels)
                        };
                        let rule = if a[0] == 1. {
                            skia::FillRule::EvenOdd
                        } else {
                            skia::FillRule::Winding
                        };
                        self.pixels.fill_path(&path, &paint, rule, identity, mask);
                    }
                }
                19 | 22 => {
                    let path = if code == 19. {
                        shape.unwrap_or_else(|| self.path.clone().finish())
                    } else {
                        quad(t, [a[0], a[1], a[2], a[3]])
                    };
                    let Some((linear, shift)) = stroke_space(t) else {
                        skip!(self.pixels)
                    };
                    let Some(path) = path.and_then(|path| path.transform(linear.invert()?)) else {
                        skip!(self.pixels)
                    };
                    self.pixels.stroke_path(
                        &path,
                        &paint(&state.stroke, state.smoothing, state.alpha, blend, shift),
                        &skia::Stroke {
                            width: state.width,
                            miter_limit: state.miter,
                            line_cap: state.cap,
                            line_join: state.join,
                            dash: skia::StrokeDash::new(state.dash.clone(), state.dash_offset),
                        },
                        linear,
                        strip(&state.stroke, state.clip.as_deref(), t, [width, height]).as_deref(),
                    );
                }
                24 | 27 => {
                    let value = string(a[0]);
                    let Some((system, aliases)) =
                        fonts.filter(|_| a[1].is_finite() && a[2].is_finite())
                    else {
                        skip!(self.pixels)
                    };
                    let (line, [ascent, descent, em_ascent, em_descent]) =
                        line(system, aliases, &state.font, value, state.spacing);
                    // As in Chrome, empty text draws nothing, while text outside the canvas still composites.
                    if line.glyphs.is_empty() {
                        skip!(self.pixels)
                    }
                    let advance = f32::from(line.width);
                    let squeeze = if f64::from(advance) > a[3] + 1e-3 {
                        (a[3] / f64::from(advance)) as f32
                    } else {
                        1.
                    };
                    let t = matrix(concat(t, [1., 0., 0., 1., a[1], a[2]]))
                        .pre_scale(squeeze, 1.)
                        .pre_translate(
                            -advance * state.align,
                            [
                                0.,
                                em_ascent,
                                (em_ascent - em_descent) / 2.,
                                -em_descent,
                                ascent * 0.8,
                                -descent,
                            ][state.baseline as usize],
                        );
                    let scale = (t.sx * t.sy - t.kx * t.ky).abs().sqrt();
                    if !t.is_finite() || !scale.is_finite() || scale <= 1e-3 {
                        skip!(self.pixels)
                    }
                    // Cap glyph rasterization at 1024 pixels per em, then scale, to bound allocations.
                    let mut raster = scale.min(1024. / f32::from(line.font_size));
                    let upright =
                        raster == scale && t.kx == 0. && t.ky == 0. && t.sx > 0. && t.sx == t.sy;
                    let canvas = skia::Rect::from_xywh(0., 0., width as f32, height as f32);
                    // Reduce layer resolution until visible ink fits within twice the canvas dimensions and
                    // 2^24 pixels.
                    if !upright
                        && let Some(visible) = t
                            .pre_scale(1. / raster, 1. / raster)
                            .invert()
                            .and_then(|t| canvas?.transform(t))
                    {
                        let [left, right, ascent, descent] = ink(system, &line);
                        let w =
                            right.min(visible.right() / raster) - left.max(visible.left() / raster);
                        let h = descent.min(visible.bottom() / raster)
                            - (-ascent).max(visible.top() / raster);
                        let side = 2. * (width + height) as f32;
                        if w > 0. {
                            raster = raster.min(side / w);
                        }
                        if h > 0. {
                            raster = raster.min(side / h);
                        }
                        if w > 0. && h > 0. {
                            raster = raster.min((16_777_216. / (w * h)).sqrt());
                        }
                    }
                    let (fx, fy, to_canvas) = if upright {
                        let [x, y] = [t.tx.floor(), t.ty.floor()];
                        (t.tx - x, t.ty - y, skia::Transform::from_translate(x, y))
                    } else {
                        (0., 0., t.pre_scale(1. / raster, 1. / raster))
                    };
                    let Some(visible) = to_canvas.invert().and_then(|t| canvas?.transform(t))
                    else {
                        skip!(self.pixels)
                    };
                    if self.glyph_bytes > 4 << 20 {
                        self.glyphs.clear();
                        self.glyph_bytes = 0;
                    }
                    let mut placed = Vec::new();
                    let [mut left, mut top, mut right, mut bottom] =
                        [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
                    let variants = f32::from(renderer::SUBPIXEL_STEPS);
                    for glyph in &line.glyphs {
                        let x = f32::from(glyph.x) * raster + fx;
                        let x = if glyph.emoji {
                            x.round()
                        } else {
                            (x * variants).round() / variants
                        };
                        // DirectWrite can't stroke glyphs, so text fills with the stroke style there.
                        let stroke = if code == 24. || glyph.emoji || cfg!(windows) {
                            0.
                        } else {
                            state.width
                        };
                        let reach = (f32::from(glyph.size) + stroke) * raster * 2.;
                        if x + reach < visible.left()
                            || x - reach > visible.right()
                            || fy + reach < visible.top()
                            || fy - reach > visible.bottom()
                        {
                            continue;
                        }
                        let key = (
                            glyph.font,
                            glyph.id,
                            [f32::from(glyph.size), raster, stroke].map(f32::to_bits),
                            ((x - x.floor()) * variants) as u8,
                            glyph.emoji,
                        );
                        if !self.glyphs.contains_key(&key) {
                            let pixels = system.glyph_pixels(
                                glyph.font, glyph.id, glyph.size, raster, key.3, key.4, stroke,
                            );
                            self.glyph_bytes +=
                                pixels.as_ref().map_or(0, |pixels| pixels.2.len()) + 64;
                            self.glyphs.insert(key, pixels);
                        }
                        if let Some((origin, size, _)) = &self.glyphs[&key] {
                            let x = x.floor() as i64 + i64::from(origin.x);
                            let y = (fy + f32::from(glyph.y) * raster).round() as i64
                                + i64::from(origin.y);
                            [left, top] = [left.min(x), top.min(y)];
                            [right, bottom] = [
                                right.max(x + i64::from(size.width)),
                                bottom.max(y + i64::from(size.height)),
                            ];
                            placed.push((key, x, y));
                        }
                    }
                    let [left, top, right, bottom] = [
                        left.max((visible.left().floor() as i64).saturating_sub(1)),
                        top.max((visible.top().floor() as i64).saturating_sub(1)),
                        right.min((visible.right().ceil() as i64).saturating_add(1)),
                        bottom.min((visible.bottom().ceil() as i64).saturating_add(1)),
                    ];
                    if right <= left || bottom <= top {
                        continue;
                    }
                    let Some(mut layer) = ((right - left).saturating_mul(bottom - top) <= 1 << 25)
                        .then(|| skia::Pixmap::new((right - left) as u32, (bottom - top) as u32))
                        .flatten()
                    else {
                        skip!(self.pixels)
                    };
                    let stride = layer.width() as usize;
                    let out = layer.pixels_mut();
                    let source = if code == 24. {
                        &state.fill
                    } else {
                        &state.stroke
                    };
                    let fill = match source {
                        Brush::Shader(skia::Shader::SolidColor(color)) => color.to_color_u8(),
                        _ => skia::ColorU8::from_rgba(255, 255, 255, 255),
                    };
                    for (key, x, y) in placed {
                        let Some((_, size, bytes)) = &self.glyphs[&key] else {
                            continue;
                        };
                        let columns = size.width as usize;
                        let depth = bytes.len() / (columns * size.height as usize).max(1);
                        if columns == 0 || !matches!(depth, 1 | 4) {
                            continue;
                        }
                        let [from, to] = [(left - x).max(0), (right - x).min(columns as i64)];
                        if from >= to {
                            continue;
                        }
                        let [from, to] = [from as usize, to as usize];
                        for (row, line) in bytes.chunks_exact(columns * depth).enumerate() {
                            let row = y + row as i64;
                            if row < top || row >= bottom {
                                continue;
                            }
                            let at =
                                (row - top) as usize * stride + (x + from as i64 - left) as usize;
                            for (pixel, source) in out[at..at + to - from]
                                .iter_mut()
                                .zip(line[from * depth..to * depth].chunks_exact(depth))
                            {
                                let source = if depth == 4 {
                                    skia::ColorU8::from_rgba(
                                        source[2], source[1], source[0], source[3],
                                    )
                                } else {
                                    skia::ColorU8::from_rgba(
                                        fill.red(),
                                        fill.green(),
                                        fill.blue(),
                                        ((u16::from(fill.alpha()) * u16::from(source[0]) + 127)
                                            / 255) as u8,
                                    )
                                }
                                .premultiply();
                                let rest = 255 - u16::from(source.alpha());
                                let over = |s: u8, d: u8| {
                                    (u16::from(s) + (u16::from(d) * rest + 127) / 255) as u8
                                };
                                *pixel = skia::PremultipliedColorU8::from_rgba(
                                    over(source.red(), pixel.red()),
                                    over(source.green(), pixel.green()),
                                    over(source.blue(), pixel.blue()),
                                    over(source.alpha(), pixel.alpha()),
                                )
                                .unwrap_or(source);
                            }
                        }
                    }
                    let to_canvas = to_canvas.pre_translate(left as f32, top as f32);
                    if let Some(to_layer) =
                        skia::Rect::from_xywh(0., 0., layer.width() as f32, layer.height() as f32)
                            .filter(|_| {
                                !matches!(source, Brush::Shader(skia::Shader::SolidColor(_)))
                            })
                            .zip(to_canvas.invert())
                    {
                        let (rect, to_layer) = to_layer;
                        layer.fill_rect(
                            rect,
                            &paint(
                                source,
                                state.smoothing,
                                1.,
                                skia::BlendMode::SourceIn,
                                matrix(state.transform).post_concat(to_layer),
                            ),
                            identity,
                            None,
                        );
                    }
                    let paint = skia::PixmapPaint {
                        opacity: state.alpha,
                        blend_mode: blend,
                        quality: if upright {
                            skia::FilterQuality::Nearest
                        } else {
                            skia::FilterQuality::Bilinear
                        },
                    };
                    self.pixels.draw_pixmap(
                        0,
                        0,
                        layer.as_ref(),
                        &paint,
                        to_canvas,
                        strip(
                            source,
                            state.clip.as_deref(),
                            state.transform,
                            [width, height],
                        )
                        .as_deref(),
                    );
                }
                25 if a.iter().all(|v| v.is_finite()) && a[5] != 0. && a[6] != 0. => {
                    let [iw, ih, sx, sy, sw, sh, dx, dy, dw, dh] =
                        [a[1], a[2], a[3], a[4], a[5], a[6], a[7], a[8], a[9], a[10]];
                    let (kx, ky) = (dw / sw, dh / sh);
                    let size = [iw * kx * t[0].hypot(t[1]), ih * ky * t[2].hypot(t[3])]
                        .map(|side| side.abs().ceil().min(f64::from(RASTER_MAX)) as u32);
                    // A negative node draws the canvas onto itself, so it is copied for each call.
                    let own;
                    let source = if a[0] < 0. {
                        own = covered.as_ref().unwrap_or(&self.pixels).clone();
                        &own
                    } else {
                        let key = (a[0] as usize, size);
                        let Some(fetched) = sources.entry(key).or_insert_with(|| image(key)) else {
                            skip!(self.pixels)
                        };
                        &**fetched
                    };
                    let shader = matrix(concat(
                        t,
                        [
                            kx * iw / f64::from(source.width()),
                            0.,
                            0.,
                            ky * ih / f64::from(source.height()),
                            dx - sx * kx,
                            dy - sy * ky,
                        ],
                    ));
                    let paint = skia::Paint {
                        shader: skia::Pattern::new(
                            source.as_ref(),
                            skia::SpreadMode::Pad,
                            if state.smoothing {
                                skia::FilterQuality::Bicubic
                            } else {
                                skia::FilterQuality::Nearest
                            },
                            state.alpha,
                            shader,
                        ),
                        blend_mode: blend,
                        ..Default::default()
                    };
                    fill_area(
                        &mut self.pixels,
                        t,
                        [dx, dy, dw, dh],
                        &paint,
                        state.clip.as_deref(),
                    );
                }
                25 => skip!(self.pixels),
                32 => {
                    state.cap = match string(a[0]) {
                        "round" => skia::LineCap::Round,
                        "square" => skia::LineCap::Square,
                        _ => skia::LineCap::Butt,
                    }
                }
                33 => {
                    state.join = match string(a[0]) {
                        "round" => skia::LineJoin::Round,
                        "bevel" => skia::LineJoin::Bevel,
                        _ => skia::LineJoin::Miter,
                    }
                }
                34 => state.miter = a[0] as f32,
                35 => {
                    state.dash = string(a[0])
                        .split(',')
                        .filter_map(|v| v.parse().ok())
                        .collect()
                }
                36 => state.dash_offset = a[0] as f32,
                37 => {
                    use skia::BlendMode::*;
                    state.blend = match string(a[0]) {
                        "source-in" => SourceIn,
                        "source-out" => SourceOut,
                        "source-atop" => SourceAtop,
                        "destination-over" => DestinationOver,
                        "destination-in" => DestinationIn,
                        "destination-out" => DestinationOut,
                        "destination-atop" => DestinationAtop,
                        "lighter" => Plus,
                        "copy" => Source,
                        "xor" => Xor,
                        "multiply" => Multiply,
                        "screen" => Screen,
                        "overlay" => Overlay,
                        "darken" => Darken,
                        "lighten" => Lighten,
                        "color-dodge" => ColorDodge,
                        "color-burn" => ColorBurn,
                        "hard-light" => HardLight,
                        "soft-light" => SoftLight,
                        "difference" => Difference,
                        "exclusion" => Exclusion,
                        "hue" => Hue,
                        "saturation" => Saturation,
                        "color" => Color,
                        "luminosity" => Luminosity,
                        _ => SourceOver,
                    }
                }
                38 => state.spacing[0] = a[0] as f32,
                44 => state.blur = (a[0] as f32).max(0.),
                39 => state.spacing[1] = a[0] as f32,
                40 => state.smoothing = a[0] != 0.,
                43 => {
                    if let Some((rgb, alpha)) = crate::css::value::color::color(string(a[0])) {
                        let [_, r, g, b] = rgb.to_be_bytes();
                        state.shadow.0 = skia::Color::from_rgba8(r, g, b, alpha);
                    }
                    state.shadow.1 = a[1] as f32;
                    state.shadow.2 = [a[2] as f32, a[3] as f32];
                }
                41 => {
                    if a[2] != 0. {
                        self.paths.clear();
                    }
                    let ops = data.get(a[1] as usize).context("Expected path data")?;
                    let ops = ops
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .map(|b| f64::from_ne_bytes(*b));
                    self.paths
                        .insert(a[0] as u64, (ops.collect(), [f64::NAN; 6], None));
                }
                _ => {
                    let bytes = data.get(a[0] as usize).context("Expected image data")?;
                    let (x, y) = (i64::from(a[1] as i32), i64::from(a[2] as i32));
                    let (w, h) = (a[3] as usize, a[4] as usize);
                    ensure!(
                        w > 0
                            && w.checked_mul(h).and_then(|n| n.checked_mul(4)) == Some(bytes.len()),
                        "Invalid image data length"
                    );
                    let pixels = self.pixels.pixels_mut();
                    for (row, line) in bytes.chunks_exact(w * 4).enumerate() {
                        let dy = y + row as i64;
                        if !(0..i64::from(height)).contains(&dy) {
                            continue;
                        }
                        for (col, p) in line.as_chunks::<4>().0.iter().enumerate() {
                            let dx = x + col as i64;
                            if (0..i64::from(width)).contains(&dx) {
                                pixels[(dy * i64::from(width) + dx) as usize] =
                                    skia::ColorU8::from_rgba(p[0], p[1], p[2], p[3]).premultiply();
                            }
                        }
                    }
                }
            }
        }
        if let Some(saved) = covered.take() {
            finish(&mut self.pixels, saved, &self.state);
        }
        drop(spare);
        // The page holds the last image only while it paints, so the new one mostly gets its buffer. The caller
        // releases the tile of the last image.
        Ok(self.present.then(|| {
            let spare = Arc::get_mut(&mut self.image).map(renderer::RenderImage::take_bytes);
            let image = render_image(&self.pixels, spare.unwrap_or_default());
            std::mem::replace(&mut self.image, image)
        }))
    }

    pub fn hit(&self, x: f32, y: f32, even_odd: bool, stroke: bool, path: Option<u64>) -> bool {
        let path = match path {
            Some(id) => self
                .paths
                .get(&id)
                .and_then(|(ops, ..)| replay(ops, self.state.transform)),
            None => self.path.clone().finish(),
        };
        let Some(path) = path else {
            return false;
        };
        let state = &self.state;
        let path = if stroke {
            let Some(path) = stroke_space(state.transform).and_then(|(linear, _)| {
                path.transform(linear.invert()?)?
                    .stroke(
                        &skia::Stroke {
                            width: state.width,
                            miter_limit: state.miter,
                            line_cap: state.cap,
                            line_join: state.join,
                            dash: None,
                        },
                        1.,
                    )?
                    .transform(linear)
            }) else {
                return false;
            };
            path
        } else {
            path
        };
        let Some(mut mask) = skia::Mask::new(1, 1) else {
            return false;
        };
        mask.fill_path(
            &path,
            if even_odd {
                skia::FillRule::EvenOdd
            } else {
                skia::FillRule::Winding
            },
            false,
            skia::Transform::from_translate(-x.floor(), -y.floor()),
        );
        mask.data()[0] != 0
    }

    pub fn transform(&self) -> [f64; 6] {
        self.state.transform
    }
}

#[cfg(test)]
mod tests {
    use super::super::pixmap;
    use super::super::svg::{render_svg, svg, svg_pixels, svg_tree};
    use super::*;
    use path::svg_path;
    use text::font;

    fn draw(canvas: &mut Canvas, ops: &[f64], strings: &str) {
        canvas.draw(ops, strings, &[], &|_| None, None).unwrap();
    }

    fn pixel(canvas: &Canvas, x: u32, y: u32) -> [u8; 4] {
        let pixel = canvas.pixels.pixel(x, y).unwrap().demultiply();
        [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
    }

    #[test]
    fn arcs_gradients_and_rounded_rects_draw_like_paths() {
        let mut canvas = Canvas::new(100, 40).unwrap();
        draw(
            &mut canvas,
            &[
                11., 28., 20., 20., 15., 0., 6.3, 0., 18., 0., 11., 31., 50., 0., 40., 40., 20.,
                20., 0., 0., 0., 0., 0., 0., 18., 0.,
            ],
            "",
        );
        assert_eq!(pixel(&canvas, 20, 20), [0, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 1, 1)[3], 0);
        assert_eq!(pixel(&canvas, 51, 1)[3], 0);
        assert_eq!(pixel(&canvas, 85, 30), [0, 0, 0, 255]);

        let spec = r##"gradient:[[0,0,100,0],[[0,"#ff0000"],[1,"#0000ff"]]]"##;
        let mut canvas = Canvas::new(100, 10).unwrap();
        draw(
            &mut canvas,
            &[
                4.,
                spec.encode_utf16().count() as f64,
                21.,
                0.,
                0.,
                100.,
                10.,
            ],
            spec,
        );
        let [left, right] = [pixel(&canvas, 2, 5), pixel(&canvas, 97, 5)];
        assert!(left[0] > 240 && left[2] < 15 && right[2] > 240 && right[0] < 15);
    }

    #[test]
    fn whole_turn_arcs_against_their_direction_draw_a_circle() {
        use std::f64::consts::TAU;
        for [start, end, counter] in [[0., TAU, 1.], [TAU, 0., 0.], [0., 3. * TAU, 1.]] {
            let mut canvas = Canvas::new(40, 40).unwrap();
            draw(
                &mut canvas,
                &[11., 28., 20., 20., 15., start, end, counter, 18., 0.],
                "",
            );
            assert_eq!(pixel(&canvas, 20, 20), [0, 0, 0, 255], "{start} {end}");
        }
        // A sweep against the direction goes the other way around: here three quarters of the circle.
        let mut canvas = Canvas::new(40, 40).unwrap();
        draw(
            &mut canvas,
            &[11., 28., 20., 20., 15., 0., TAU / 4., 1., 18., 0.],
            "",
        );
        assert_eq!(pixel(&canvas, 10, 10)[3], 255);
        assert_eq!(pixel(&canvas, 30, 30)[3], 0);
    }

    #[test]
    fn gradients_with_equal_points_or_circles_paint_nothing() {
        for shape in [vec![10, 5, 10, 5], vec![20, 20, 10, 20, 20, 10]] {
            let spec = format!(
                "gradient:{}",
                serde_json::json!([shape, [[0, "#ff0000"], [1, "#0000ff"]]])
            );
            let mut canvas = Canvas::new(40, 40).unwrap();
            draw(
                &mut canvas,
                &[4., spec.len() as f64, 21., 0., 0., 40., 40.],
                &spec,
            );
            assert_eq!(pixel(&canvas, 20, 20)[3], 0, "{shape:?}");
        }
    }

    #[test]
    fn canvas_fonts_resolve_like_chrome_and_reject_bad_values() {
        for (value, font_value) in [
            ("12pt Arial", Some("16px Arial")),
            ("1.5em serif", Some("15px serif")),
            ("150% serif", Some("15px serif")),
            ("large sans-serif", Some("18px sans-serif")),
            ("bold 16px/20px Arial", Some("bold 16px Arial")),
            (
                "italic 600 10px a, 'Times New Roman'",
                Some("italic 600 10px a, \"Times New Roman\""),
            ),
            ("16px", None),
            ("bogus", None),
            ("foo Arial", None),
            ("-1px Arial", None),
            ("inherit", None),
        ] {
            assert_eq!(font(value).as_deref(), font_value, "{value}");
        }
    }

    #[test]
    fn calls_that_draw_nothing_keep_the_canvas_in_covering_modes() {
        let system = renderer::TextSystem::new();
        let aliases = std::collections::HashMap::new();
        for tail in [
            &[11., 19.][..],
            &[11., 18., 0.],
            &[24., 0., 5., 5., f64::INFINITY],
        ] {
            let mut canvas = Canvas::new(10, 10).unwrap();
            let mut ops = vec![21., 0., 0., 10., 10., 37., 4.];
            ops.extend(tail);
            canvas
                .draw(&ops, "copy", &[], &|_| None, Some((&system, &aliases)))
                .unwrap();
            assert_eq!(pixel(&canvas, 5, 5)[3], 255, "{tail:?}");
        }
    }

    #[test]
    fn radial_gradients_keep_both_circle_radii() {
        for (shape, red, blue) in [
            ([20, 20, 10, 20, 20, 20], (20, 20), (39, 20)),
            ([20, 20, 20, 20, 20, 10], (39, 20), (20, 20)),
            ([10, 20, 10, 20, 20, 20], (20, 20), (39, 20)),
        ] {
            let spec = format!(
                "gradient:{}",
                serde_json::json!([shape, [[0, "#ff0000"], [1, "#0000ff"]]])
            );
            let mut canvas = Canvas::new(40, 40).unwrap();
            draw(
                &mut canvas,
                &[4., spec.len() as f64, 21., 0., 0., 40., 40.],
                &spec,
            );
            let red = pixel(&canvas, red.0, red.1);
            let blue = pixel(&canvas, blue.0, blue.1);
            assert!(red[0] > 235 && red[2] < 20, "{shape:?}: {red:?}");
            assert!(blue[2] > 235 && blue[0] < 20, "{shape:?}: {blue:?}");
            if shape[0] == 10 {
                let [r, _, b, _] = pixel(&canvas, 30, 20);
                assert!(r.abs_diff(121) <= 2 && b.abs_diff(134) <= 2, "{r} {b}");
            }
        }
    }

    #[test]
    fn svg_paths_keep_lines_and_stop_at_errors() {
        assert_eq!(svg_path("M0 0L10 0"), [13., 0., 0., 14., 10., 0.]);
        assert_eq!(svg_path("M1 1h2 x"), [13., 1., 1., 14., 3., 1.]);
        assert_eq!(
            svg_path("M0 0Q1 1 2 0Z\"&<"),
            [13., 0., 0., 15., 1., 1., 2., 0., 12.]
        );
        assert!(svg_path("M5 5").is_empty());
    }

    #[test]
    fn composite_modes_that_cover_the_canvas_change_pixels_outside_the_shape() {
        let mut canvas = Canvas::new(20, 10).unwrap();
        let mode = "destination-in";
        draw(
            &mut canvas,
            &[
                21.,
                0.,
                0.,
                20.,
                10.,
                37.,
                mode.len() as f64,
                21.,
                0.,
                0.,
                10.,
                10.,
            ],
            mode,
        );
        assert_eq!(pixel(&canvas, 5, 5)[3], 255);
        assert_eq!(pixel(&canvas, 15, 5)[3], 0);
    }

    #[test]
    fn text_squeezes_to_its_max_width() {
        let system = renderer::TextSystem::new();
        let aliases = std::collections::HashMap::new();
        let strings = "20px sans-serifWWWWWWWW";
        let ink = |max: f64| {
            let mut canvas = Canvas::new(400, 30).unwrap();
            canvas
                .draw(
                    &[6., 15., 24., 8., 0., 24., max],
                    strings,
                    &[],
                    &|_| None,
                    Some((&system, &aliases)),
                )
                .unwrap();
            (0..400)
                .rev()
                .find(|x| (0..30).any(|y| canvas.pixels.pixel(*x, y).unwrap().alpha() > 32))
        };
        let full = ink(f64::INFINITY).unwrap();
        let squeezed = ink(50.).unwrap();
        assert!(full > 100, "{full}");
        assert!((45..=52).contains(&squeezed), "{squeezed}");
    }

    #[test]
    fn dashes_leave_gaps_and_hits_follow_the_path() {
        let mut canvas = Canvas::new(40, 10).unwrap();
        let dash = "10,10";
        draw(
            &mut canvas,
            &[
                35.,
                dash.len() as f64,
                10.,
                4.,
                13.,
                0.,
                5.,
                14.,
                40.,
                5.,
                19.,
            ],
            dash,
        );
        assert_eq!(pixel(&canvas, 5, 5)[3], 255);
        assert_eq!(pixel(&canvas, 15, 5)[3], 0);
        assert!(canvas.hit(20., 5.5, false, true, None));
        assert!(!canvas.hit(20., 0.5, false, true, None));
        draw(&mut canvas, &[11., 17., 2., 2., 6., 6.], "");
        assert!(canvas.hit(4., 4., false, false, None));
        assert!(!canvas.hit(20., 4., false, false, None));
        draw(&mut canvas, &[2., 2., 0., 0., 2., 3., 4.], "");
        assert_eq!(canvas.transform(), [2., 0., 0., 2., 3., 4.]);
    }

    #[test]
    fn images_keep_premultiplied_alpha_at_display_resolution() {
        let mut canvas = Canvas::new(2, 2).unwrap();
        canvas
            .draw(
                &[4., 7., 9., 0.5, 21., 0., 0., 2., 2.],
                "#ff0000",
                &[],
                &|_| None,
                None,
            )
            .unwrap();
        assert!(canvas.image.is_premultiplied());
        assert_eq!(&canvas.image.bytes()[..4], &[0, 0, 128, 128]);
        assert_eq!(canvas.pixels.pixels()[0].red(), 128);
        assert_eq!(pixmap(&canvas.image).unwrap().data()[..4], [128, 0, 0, 128]);
        let (id, buffer) = (canvas.image.id, canvas.image.bytes().as_ptr());
        let old = canvas.draw(&[23., 0., 0., 1., 1.], "", &[], &|_| None, None);
        let old = old.unwrap().unwrap();
        assert_eq!((old.id, old.bytes().len()), (id, 0));
        assert_eq!(canvas.image.bytes().as_ptr(), buffer);
        assert_eq!(canvas.image.bytes()[..8], [0, 0, 0, 0, 0, 0, 128, 128]);
        let held = canvas.image.clone();
        canvas
            .draw(&[23., 0., 0., 2., 2.], "", &[], &|_| None, None)
            .unwrap();
        assert_eq!(held.bytes()[4..8], [0, 0, 128, 128]);
        assert_eq!(canvas.image.bytes(), [0; 16]);
        let image = svg(r##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><path d="M0 0h2v2H0Z" fill="#ff0000" opacity="0.5"/></svg>"##, 4, 4).unwrap();
        assert_eq!(image.size().width as u32, 4);
        assert_eq!(image.bytes(), [0, 0, 128, 128].repeat(16));
        let tree = svg_tree(r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><path d="M0 0h2v2H0Z" fill="#ff0000"/><path d="M2 0h2v2H2Z" fill="#0000ff"/></svg>"##).unwrap();
        let rotated = render_svg(&tree, 1, 2, Some(([2., 1.], [0., 1., -1., 0., 1., 0.]))).unwrap();
        assert_eq!(
            rotated.bytes(),
            [[0, 0, 255, 255], [255, 0, 0, 255]].concat()
        );
    }

    #[test]
    fn paths_and_clips_use_the_transform_when_they_are_added() {
        let mut canvas = Canvas::new(4, 4).unwrap();
        #[rustfmt::skip]
        let ops = [
            4., 4., 0., 2., 1., 0., 0., 1., 2., 0., 17., 0., 0., 2., 4., 20., 0.,
            21., -2., 0., 4., 4., 1., 4., 4., 21., 0., 0., 1., 1.,
        ];
        canvas.draw(&ops, "#f00blue", &[], &|_| None, None).unwrap();
        let red = |x: usize| canvas.pixels.pixels()[x].red();
        assert_eq!(canvas.pixels.pixels()[0].blue(), 255);
        assert_eq!([red(1), red(2), red(3)], [0, 255, 255]);
        assert_eq!(canvas.pixels.pixels()[1].alpha(), 0);
    }

    #[test]
    fn text_draws_with_the_page_fonts_and_skips_offscreen_glyphs() {
        let system = renderer::TextSystem::new();
        let aliases = Default::default();
        let mut canvas = Canvas::new(200, 24).unwrap();
        let text = "Hi".repeat(5000);
        #[rustfmt::skip]
        let inf = f64::INFINITY;
        let ops = [
            6., 15., 7., 6., 24., 2., 150., 18., inf, 24., 10000., -1e6, 18., inf,
        ];
        let strings = format!("20px sans-serifcenterHi{text}");
        canvas
            .draw(&ops, &strings, &[], &|_| None, Some((&system, &aliases)))
            .unwrap();
        let inked = |range: std::ops::Range<usize>| {
            canvas
                .pixels
                .pixels()
                .chunks(200)
                .flat_map(|row| &row[range.clone()])
                .any(|p| p.alpha() > 0)
        };
        assert!(inked(140..160) && !inked(0..130) && !inked(170..200));
        assert_eq!(canvas.glyphs.len(), 2);
    }

    #[test]
    fn huge_text_rasterizes_small_glyphs() {
        let system = renderer::TextSystem::new();
        let aliases = Default::default();
        let mut canvas = Canvas::new(64, 64).unwrap();
        #[rustfmt::skip]
        let ops = [
            6., 11., 24., 1., 0., 100., f64::INFINITY,
            6., 11., 24., 1., 0., 10., f64::INFINITY,
            6., 10., 2., 1e5, 0., 0., 1e5, 0., 0., 24., 1., 0., 0., f64::INFINITY,
        ];
        let strings = "1e7px serifWNaNpx serifW10px serifW";
        canvas
            .draw(&ops, strings, &[], &|_| None, Some((&system, &aliases)))
            .unwrap();
        assert!(canvas.glyph_bytes < 3 * (1100 * 1100 * 4 + 64));
    }

    #[test]
    fn stretched_text_keeps_a_small_layer() {
        let system = renderer::TextSystem::new();
        let aliases = Default::default();
        let mut canvas = Canvas::new(300, 150).unwrap();
        let strings = format!("100px serif{}{}", "W".repeat(1000), "W".repeat(100_000));
        #[rustfmt::skip]
        let ops = [
            6., 11., 3., 0.001, 0., 0., 104857.6, 0., 0., 24., 1000., 0., 0., f64::INFINITY,
            3., 0.001, 50., -50., 104857.6, 150., 75., 24., 100_000., 0., 0., f64::INFINITY,
        ];
        canvas
            .draw(&ops, &strings, &[], &|_| None, Some((&system, &aliases)))
            .unwrap();
        assert!(canvas.glyph_bytes < 64 << 10, "{}", canvas.glyph_bytes);

        let mut canvas = Canvas::new(60, 100).unwrap();
        let ops = [
            6.,
            15.,
            3.,
            0.,
            1.,
            -1.,
            0.,
            20.,
            10.,
            24.,
            3.,
            0.,
            0.,
            f64::INFINITY,
        ];
        canvas
            .draw(
                &ops,
                "20px sans-serifWWW",
                &[],
                &|_| None,
                Some((&system, &aliases)),
            )
            .unwrap();
        let inked = |xs: std::ops::Range<u32>, ys: std::ops::Range<u32>| {
            xs.flat_map(|x| ys.clone().map(move |y| (x, y)))
                .any(|(x, y)| canvas.pixels.pixel(x, y).unwrap().alpha() > 0)
        };
        assert!(inked(20..36, 10..66) && !inked(0..18, 0..100) && !inked(0..60, 72..100));
    }

    #[test]
    fn far_coordinates_stay_exact_and_bad_points_skip_one_call() {
        let mut canvas = Canvas::new(8, 8).unwrap();
        #[rustfmt::skip]
        draw(&mut canvas, &[
            2., 1., 0., 0., 1., -1e8, 0., 21., 1e8 + 3., 0., 1., 1., 3., 1., 0., 0., 1., 0., 0.,
            11., 13., 0., 2., 14., f64::NAN, 3., 14., 1e300, 3., 14., 4., 2., 14., 4., 6., 14., 0., 6., 18., 0.,
        ], "");
        assert_eq!([2, 3, 4].map(|x| pixel(&canvas, x, 0)[3]), [0, 255, 0]);
        assert_eq!([pixel(&canvas, 2, 4)[3], pixel(&canvas, 6, 4)[3]], [255, 0]);
        let mut canvas = Canvas::new(8, 8).unwrap();
        #[rustfmt::skip]
        draw(&mut canvas, &[
            17., 0., 0., 4., 8., 14., f64::INFINITY, 0., 20., 0., 21., 0., 0., 8., 8.,
        ], "");
        assert_eq!([pixel(&canvas, 2, 1)[3], pixel(&canvas, 6, 1)[3]], [255, 0]);
    }

    #[test]
    fn svg_sources_draw_capped_at_their_canvas_size() {
        let tree = svg_tree(r##"<svg xmlns="http://www.w3.org/2000/svg" width="30000" height="10"><path d="M0 0h15000v10H0Z" fill="#ff0000"/><path d="M15000 0h15000v10H15000Z" fill="#0000ff"/></svg>"##).unwrap();
        let pixels = svg_pixels(&tree, 30000, 10, None).unwrap();
        assert_eq!([pixels.width(), pixels.height()], [RASTER_MAX, 10]);
        let [red, blue] = [
            pixels.pixel(1000, 5).unwrap(),
            pixels.pixel(3000, 5).unwrap(),
        ];
        assert_eq!(
            [red.red(), red.blue(), blue.red(), blue.blue()],
            [255, 0, 0, 255]
        );

        let mut red = skia::Pixmap::new(2, 2).unwrap();
        red.fill(skia::Color::from_rgba8(255, 0, 0, 255));
        let asked = std::cell::Cell::new(None);
        let mut canvas = Canvas::new(10, 10).unwrap();
        #[rustfmt::skip]
        let ops = [
            2., 2., 0., 0., 2., 0., 0.,
            25., 5., 30000., 30000., 0., 0., 30000., 30000., 0., 0., 5., 5.,
        ];
        let source = |(id, size)| {
            asked.set(Some(size));
            (id == 5).then_some(Source::Pixels(&red))
        };
        canvas.draw(&ops, "", &[], &source, None).unwrap();
        assert_eq!(asked.get(), Some([10, 10]));
        assert_eq!(
            [pixel(&canvas, 0, 0), pixel(&canvas, 9, 9)],
            [[255, 0, 0, 255]; 2]
        );
    }
}
