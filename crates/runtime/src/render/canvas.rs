use anyhow::{Context, Result, ensure};
use resvg::{tiny_skia as skia, usvg};
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

// WARNING: SVG rasters have at most this many pixels per side. Larger ones scale down to fit and draw scaled up.
pub const RASTER_MAX: u32 = 4096;

#[derive(Clone)]
struct State {
    // WARNING: `[a, b, c, d, e, f]` in f64, so a far translation and a far point cancel exactly.
    transform: [f64; 6],
    fill: skia::Shader<'static>,
    stroke: skia::Shader<'static>,
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
}

type GlyphKey = (usize, u32, u32, u32, u8, bool);

type Glyph = (
    renderer::Point<renderer::DevicePixels>,
    renderer::Size<renderer::DevicePixels>,
    Vec<u8>,
);

pub fn line(
    system: &renderer::TextSystem,
    aliases: &std::collections::HashMap<String, String>,
    font: &str,
    text: &str,
    [letter, word]: [f32; 2],
) -> (Rc<renderer::LineLayout>, [f32; 4]) {
    let text = text.replace(['\n', '\r', '\t'], " ");
    let (size, family) = font.split_once("px").unwrap_or(("10", "sans-serif"));
    let mut face = crate::style::font(&crate::style::Style::from_iter([
        (crate::style::Prop::FONT_FAMILY, family.trim().into()),
        (
            crate::style::Prop::FONT_WEIGHT,
            size.split_whitespace()
                .rev()
                .skip(1)
                .find(|part| *part == "bold" || part.parse::<u16>().is_ok())
                .unwrap_or("400")
                .into(),
        ),
        (
            crate::style::Prop::FONT_STYLE,
            if size.contains("italic") {
                "italic"
            } else {
                "normal"
            }
            .into(),
        ),
    ]));
    if let Some(family) = aliases.get(face.family.as_ref()) {
        face.family = family.clone().into();
    }
    let run = renderer::TextRun {
        len: text.len(),
        font: face,
        color: renderer::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
        letter_spacing: renderer::px(letter),
        word_spacing: renderer::px(word),
        font_size: None,
        inset: Default::default(),
        nowrap: false,
    };
    let font_size = size
        .split_whitespace()
        .last()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|size| size.is_finite() && *size > 0.)
        .unwrap_or(10.);
    let id = system.resolve_font(&run.font);
    let [ascent, descent, _] = system.line_metrics(id, renderer::px(font_size));
    let em = f32::from(system.em_ascent(id, renderer::px(font_size)));
    (
        system.layout_line(&text, renderer::px(font_size), &[run], None),
        [f32::from(ascent), f32::from(descent), em, font_size - em],
    )
}

pub fn ink(system: &renderer::TextSystem, line: &renderer::LineLayout) -> [f32; 4] {
    let mut ink: Option<[f32; 4]> = None;
    for run in &line.runs {
        for glyph in &run.glyphs {
            let Some([left, right, top, bottom]) =
                system.glyph_bounds(run.font_id, glyph.id, run.font_size)
            else {
                continue;
            };
            let x = f32::from(glyph.position.x);
            let [l, r, t, b] = ink.get_or_insert([x + left, x + right, top, bottom]);
            *l = l.min(x + left);
            *r = r.max(x + right);
            *t = t.max(top);
            *b = b.min(bottom);
        }
    }
    ink.map_or([0.; 4], |[left, right, top, bottom]| {
        [left, right, top, -bottom]
    })
}

fn gradient(spec: &str) -> Option<skia::Shader<'static>> {
    let (shape, stops): (Vec<f32>, Vec<(f32, String)>) = serde_json::from_str(spec).ok()?;
    // WARNING: tiny-skia has no start radius, so the stops move out by it. Inside it the first color fills.
    let start = if shape.len() == 6 {
        shape[2] / shape[5]
    } else {
        0.
    };
    let stops = stops
        .iter()
        .filter_map(|(offset, color)| {
            let (rgb, alpha) = crate::style::color(color)?;
            let [_, r, g, b] = rgb.to_be_bytes();
            Some(skia::GradientStop::new(
                start + offset * (1. - start),
                skia::Color::from_rgba8(r, g, b, alpha),
            ))
        })
        .collect();
    let identity = skia::Transform::identity();
    match shape[..] {
        [x0, y0, x1, y1] => skia::LinearGradient::new(
            (x0, y0).into(),
            (x1, y1).into(),
            stops,
            skia::SpreadMode::Pad,
            identity,
        ),
        [x0, y0, _, x1, y1, r1] => skia::RadialGradient::new(
            (x0, y0).into(),
            (x1, y1).into(),
            r1,
            stops,
            skia::SpreadMode::Pad,
            identity,
        ),
        _ => None,
    }
}

// WARNING: device points of canvas points, or `None` past the range where tiny-skia drops the whole path, so
// such a call is skipped like one with non-finite arguments.
fn device<const N: usize>(
    [a, b, c, d, e, f]: [f64; 6],
    points: [[f64; 2]; N],
) -> Option<[(f32, f32); N]> {
    let mut out = [(0., 0.); N];
    for (out, [x, y]) in out.iter_mut().zip(points) {
        let [x, y] = [a * x + c * y + e, b * x + d * y + f];
        if !(x.abs() <= 1e37 && y.abs() <= 1e37) {
            return None;
        }
        *out = (x as f32, y as f32);
    }
    Some(out)
}

fn matrix([a, b, c, d, e, f]: [f64; 6]) -> skia::Transform {
    skia::Transform::from_row(a as f32, b as f32, c as f32, d as f32, e as f32, f as f32)
}

fn concat([a, b, c, d, e, f]: [f64; 6], [ma, mb, mc, md, me, mf]: [f64; 6]) -> [f64; 6] {
    [
        a * ma + c * mb,
        b * ma + d * mb,
        a * mc + c * md,
        b * mc + d * md,
        a * me + c * mf + e,
        b * me + d * mf + f,
    ]
}

// WARNING: strokes are built in the space of the linear part, so their width is in canvas units and far
// translations never meet f32 points. The paint shader maps canvas points into that space.
fn stroke_space([a, b, c, d, e, f]: [f64; 6]) -> Option<(skia::Transform, skia::Transform)> {
    let det = a * d - b * c;
    let shift = [(d * e - c * f) / det, (a * f - b * e) / det];
    let linear = matrix([a, b, c, d, 0., 0.]);
    (det != 0. && shift.iter().all(|v| v.is_finite()) && linear.is_finite()).then(|| {
        (
            linear,
            skia::Transform::from_translate(shift[0] as f32, shift[1] as f32),
        )
    })
}

fn quad(t: [f64; 6], [x, y, w, h]: [f64; 4]) -> Option<skia::Path> {
    let [p0, p1, p2, p3] = device(t, [[x, y], [x + w, y], [x + w, y + h], [x, y + h]])?;
    let mut path = skia::PathBuilder::new();
    path.move_to(p0.0, p0.1);
    path.line_to(p1.0, p1.1);
    path.line_to(p2.0, p2.1);
    path.line_to(p3.0, p3.1);
    path.close();
    path.finish()
}

fn fill_area(
    pixels: &mut skia::Pixmap,
    t: [f64; 6],
    area: [f64; 4],
    paint: &skia::Paint,
    mask: Option<&skia::Mask>,
) {
    let identity = skia::Transform::identity();
    if t[1] == 0. && t[2] == 0. {
        let [x, y, w, h] = area;
        if let Some([(left, top), (right, bottom)]) = device(t, [[x, y], [x + w, y + h]])
            && let Some(rect) = skia::Rect::from_ltrb(
                left.min(right),
                top.min(bottom),
                left.max(right),
                top.max(bottom),
            )
        {
            pixels.fill_rect(rect, paint, identity, mask);
        }
    } else if let Some(path) = quad(t, area) {
        pixels.fill_path(&path, paint, skia::FillRule::Winding, identity, mask);
    }
}

fn arc(
    path: &mut skia::PathBuilder,
    t: [f64; 6],
    [cx, cy, rx, ry, rotation, start, end, counter]: [f64; 8],
) {
    use std::f64::consts::{FRAC_PI_2, TAU};
    let sweep = end - start;
    let sweep = if counter == 0. && sweep >= TAU {
        TAU
    } else if counter != 0. && sweep <= -TAU {
        -TAU
    } else if counter != 0. {
        sweep.rem_euclid(TAU) - TAU * f64::from(sweep.rem_euclid(TAU) > 0.)
    } else {
        sweep.rem_euclid(TAU)
    };
    let (sin, cos) = rotation.sin_cos();
    let at = |x: f64, y: f64| {
        let (x, y) = (x * rx, y * ry);
        [cx + x * cos - y * sin, cy + x * sin + y * cos]
    };
    let parts = (sweep.abs() / FRAC_PI_2).ceil().clamp(1., 4.) as usize;
    let step = sweep / parts as f64;
    let k = 4. / 3. * (step / 4.).tan();
    let mut points = [[0.; 2]; 13];
    let (mut s0, mut c0) = start.sin_cos();
    points[0] = at(c0, s0);
    for part in 1..=parts {
        let (s1, c1) = (start + step * part as f64).sin_cos();
        points[part * 3 - 2] = at(c0 - k * s0, s0 + k * c0);
        points[part * 3 - 1] = at(c1 + k * s1, s1 - k * c1);
        points[part * 3] = at(c1, s1);
        (s0, c0) = (s1, c1);
    }
    let Some(points) = device(t, points) else {
        return;
    };
    let (x, y) = points[0];
    if path.is_empty() {
        path.move_to(x, y);
    } else {
        path.line_to(x, y);
    }
    for [c1, c2, end] in points[1..=parts * 3].as_chunks::<3>().0 {
        path.cubic_to(c1.0, c1.1, c2.0, c2.1, end.0, end.1);
    }
}

pub fn fonts(system: bool) -> MutexGuard<'static, (Arc<usvg::fontdb::Database>, bool)> {
    static FONTS: LazyLock<Mutex<(Arc<usvg::fontdb::Database>, bool)>> =
        LazyLock::new(Mutex::default);
    let mut fonts = FONTS.lock().unwrap();
    if system && !fonts.1 {
        Arc::make_mut(&mut fonts.0).load_system_fonts();
        fonts.1 = true;
    }
    fonts
}

pub(crate) fn render_image(pixels: &skia::Pixmap) -> Arc<renderer::RenderImage> {
    let mut bytes = vec![0; pixels.data().len()];
    for (out, pixel) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(pixels.pixels()) {
        if pixel.alpha() != 0 {
            let pixel = pixel.demultiply();
            *out = [pixel.blue(), pixel.green(), pixel.red(), pixel.alpha()];
        }
    }
    Arc::new(renderer::RenderImage::new(
        pixels.width(),
        pixels.height(),
        bytes,
    ))
}

pub(crate) fn pixmap(image: &renderer::RenderImage) -> Option<skia::Pixmap> {
    let size = image.size();
    let mut pixels = skia::Pixmap::new(size.width.into(), size.height.into())?;
    for (out, pixel) in pixels
        .pixels_mut()
        .iter_mut()
        .zip(image.bytes().as_chunks::<4>().0)
    {
        *out = skia::ColorU8::from_rgba(pixel[2], pixel[1], pixel[0], pixel[3]).premultiply();
    }
    Some(pixels)
}

// WARNING: the pixels of a `drawImage()` source: a canvas is borrowed, other sources come from a cache.
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
        let image = Arc::new(renderer::RenderImage::new(
            width,
            height,
            vec![0; (width * height * 4) as usize],
        ));
        Ok(Self {
            pixels,
            image,
            present: true,
            state: State {
                transform: [1., 0., 0., 1., 0., 0.],
                fill: skia::Shader::SolidColor(skia::Color::BLACK),
                stroke: skia::Shader::SolidColor(skia::Color::BLACK),
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
            },
            stack: Vec::new(),
            path: skia::PathBuilder::new(),
            glyphs: Default::default(),
            glyph_bytes: 0,
        })
    }

    // WARNING: runs the `CanvasRenderingContext2D` operations from `dom.js`: a code and its arguments. Strings are
    // UTF-16 lengths of consecutive parts of `strings`, and `putImageData` bytes are in `data`. `image`
    // returns the pixels of another canvas or image node, with an SVG drawn at the given size.
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
    ) -> Result<()> {
        // WARNING: the codes and argument counts must match `CanvasRenderingContext2D` in `dom.js`.
        const ARGS: [usize; 41] = [
            0, 0, 6, 6, 1, 1, 1, 1, 1, 1, 1, 0, 0, 2, 2, 4, 6, 4, 1, 0, 1, 4, 4, 4, 4, 11, 5, 4, 6,
            8, 5, 12, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        ];
        let paint = |source: &skia::Shader<'static>,
                     alpha: f32,
                     blend_mode: skia::BlendMode,
                     transform: skia::Transform| {
            let mut shader = source.clone();
            shader.apply_opacity(alpha);
            shader.transform(transform);
            skia::Paint {
                shader,
                blend_mode,
                ..Default::default()
            }
        };
        let stroke = |state: &State| skia::Stroke {
            width: state.width,
            miter_limit: state.miter,
            line_cap: state.cap,
            line_join: state.join,
            dash: skia::StrokeDash::new(state.dash.clone(), state.dash_offset),
        };
        // WARNING: these modes also change pixels outside the shape, so the shape draws into a layer that covers the canvas.
        let mut covered: Option<skia::Pixmap> = None;
        let finish = |pixels: &mut skia::Pixmap, saved: skia::Pixmap, state: &State| {
            let layer = std::mem::replace(pixels, saved);
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
        };
        let mut string = |units: f64| {
            let (part, rest) = strings.split_at(crate::dom::utf16_index(strings, units as usize));
            strings = rest;
            part
        };
        let identity = skia::Transform::identity();
        let mut sources = rustc_hash::FxHashMap::default();
        let [width, height] = [self.pixels.width(), self.pixels.height()];
        let mut rest = ops;
        while let [code, tail @ ..] = rest {
            if let Some(saved) = covered.take() {
                finish(&mut self.pixels, saved, &self.state);
            }
            let count = *ARGS
                .get(*code as usize)
                .context("Invalid canvas operation")?;
            ensure!(tail.len() >= count, "Invalid canvas operation");
            let (a, tail) = tail.split_at(count);
            rest = tail;
            let blend = if matches!(*code as u8, 18 | 19 | 21 | 22 | 24 | 25 | 27)
                && matches!(
                    self.state.blend,
                    skia::BlendMode::SourceIn
                        | skia::BlendMode::SourceOut
                        | skia::BlendMode::DestinationIn
                        | skia::BlendMode::DestinationAtop
                        | skia::BlendMode::Source
                ) {
                covered = Some(std::mem::replace(
                    &mut self.pixels,
                    skia::Pixmap::new(width, height).context("Cannot allocate canvas layer")?,
                ));
                skia::BlendMode::SourceOver
            } else {
                self.state.blend
            };
            let state = &mut self.state;
            let t = state.transform;
            match *code as u8 {
                0 => self.stack.push(state.clone()),
                1 => {
                    if let Some(saved) = self.stack.pop() {
                        *state = saved;
                    }
                }
                2 | 3 if a.iter().all(|v| v.is_finite()) => {
                    let m = [a[0], a[1], a[2], a[3], a[4], a[5]];
                    state.transform = if *code == 2. { concat(t, m) } else { m };
                }
                2 | 3 => {}
                4 | 5 => {
                    let value = string(a[0]);
                    let source = if let Some(spec) = value.strip_prefix("gradient:") {
                        Some(
                            gradient(spec)
                                .unwrap_or(skia::Shader::SolidColor(skia::Color::TRANSPARENT)),
                        )
                    } else {
                        crate::style::color(value).map(|(rgb, alpha)| {
                            let [_, r, g, b] = rgb.to_be_bytes();
                            skia::Shader::SolidColor(skia::Color::from_rgba8(r, g, b, alpha))
                        })
                    };
                    if let Some(source) = source {
                        if *code == 4. {
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
                12 => self.path.close(),
                // WARNING: as the standard says, a call with non-finite arguments is ignored, and so is one
                // whose points land too far for tiny-skia.
                13 | 14 => {
                    let Some([(x, y)]) = device(t, [[a[0], a[1]]]) else {
                        continue;
                    };
                    if *code == 13. || self.path.is_empty() {
                        self.path.move_to(x, y);
                    } else {
                        self.path.line_to(x, y);
                    }
                }
                15 => {
                    let Some([(x1, y1), (x, y)]) = device(t, [[a[0], a[1]], [a[2], a[3]]]) else {
                        continue;
                    };
                    if self.path.is_empty() {
                        self.path.move_to(x1, y1);
                    }
                    self.path.quad_to(x1, y1, x, y);
                }
                16 => {
                    let Some([(x1, y1), (x2, y2), (x, y)]) =
                        device(t, [[a[0], a[1]], [a[2], a[3]], [a[4], a[5]]])
                    else {
                        continue;
                    };
                    if self.path.is_empty() {
                        self.path.move_to(x1, y1);
                    }
                    self.path.cubic_to(x1, y1, x2, y2, x, y);
                }
                17 => {
                    let [x, y, w, h] = [a[0], a[1], a[2], a[3]];
                    let Some(points) = device(t, [[x, y], [x + w, y], [x + w, y + h], [x, y + h]])
                    else {
                        continue;
                    };
                    self.path.move_to(points[0].0, points[0].1);
                    for (x, y) in &points[1..] {
                        self.path.line_to(*x, *y);
                    }
                    self.path.close();
                }
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
                    match self.path.clone().finish() {
                        Some(path) => mask.intersect_path(&path, rule, true, identity),
                        None => mask.data_mut().fill(0),
                    }
                    state.clip = Some(Rc::new(mask));
                }
                18 | 21 | 23 => {
                    let mask = state.clip.as_deref();
                    let paint = if *code == 23. {
                        skia::Paint {
                            blend_mode: skia::BlendMode::Clear,
                            ..Default::default()
                        }
                    } else {
                        paint(&state.fill, state.alpha, blend, matrix(t))
                    };
                    if *code != 18. {
                        fill_area(&mut self.pixels, t, [a[0], a[1], a[2], a[3]], &paint, mask);
                    } else if let Some(path) = self.path.clone().finish() {
                        let rule = if a[0] == 1. {
                            skia::FillRule::EvenOdd
                        } else {
                            skia::FillRule::Winding
                        };
                        self.pixels.fill_path(&path, &paint, rule, identity, mask);
                    }
                }
                19 | 22 => {
                    let path = if *code == 19. {
                        self.path.clone().finish()
                    } else {
                        quad(t, [a[0], a[1], a[2], a[3]])
                    };
                    if let Some((linear, shift)) = stroke_space(t)
                        && let Some(path) = path.and_then(|path| path.transform(linear.invert()?))
                    {
                        self.pixels.stroke_path(
                            &path,
                            &paint(&state.stroke, state.alpha, blend, shift),
                            &stroke(state),
                            linear,
                            state.clip.as_deref(),
                        );
                    }
                }
                24 | 27 => {
                    let value = string(a[0]);
                    let Some((system, aliases)) =
                        fonts.filter(|_| a[1].is_finite() && a[2].is_finite())
                    else {
                        continue;
                    };
                    let (line, [ascent, descent, em_ascent, em_descent]) =
                        line(system, aliases, &state.font, value, state.spacing);
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
                        continue;
                    }
                    // WARNING: glyphs rasterize at most 1024 pixels per em and larger text scales the layer up, so a huge font or transform cannot allocate huge glyphs.
                    let mut raster = scale.min(1024. / f32::from(line.font_size));
                    let upright =
                        raster == scale && t.kx == 0. && t.ky == 0. && t.sx > 0. && t.sx == t.sy;
                    let canvas = skia::Rect::from_xywh(0., 0., width as f32, height as f32);
                    // WARNING: a stretched or skewed transform maps the canvas to a huge part of the layer, so the
                    // layer gets coarser until the visible ink fits in twice the canvas sides and 2^24 pixels.
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
                    // WARNING: glyphs far outside the canvas are skipped and the layer is cut to the canvas, so a long or huge text cannot make a huge layer.
                    let Some(visible) = to_canvas.invert().and_then(|t| canvas?.transform(t))
                    else {
                        continue;
                    };
                    // WARNING: pages can draw any glyph at any scale, so the cache starts over past 4 MB.
                    if self.glyph_bytes > 4 << 20 {
                        self.glyphs.clear();
                        self.glyph_bytes = 0;
                    }
                    let mut placed = Vec::new();
                    let [mut left, mut top, mut right, mut bottom] =
                        [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
                    for run in &line.runs {
                        for glyph in &run.glyphs {
                            let x = f32::from(glyph.position.x) * raster + fx;
                            let variants = f32::from(renderer::SUBPIXEL_VARIANTS_X);
                            let x = if glyph.is_emoji {
                                x.round()
                            } else {
                                (x * variants).round() / variants
                            };
                            let reach = f32::from(run.font_size) * raster * 2.;
                            if x + reach < visible.left()
                                || x - reach > visible.right()
                                || fy + reach < visible.top()
                                || fy - reach > visible.bottom()
                            {
                                continue;
                            }
                            let key = (
                                run.font_id.0,
                                glyph.id.0,
                                f32::from(run.font_size).to_bits(),
                                raster.to_bits(),
                                ((x - x.floor()) * variants) as u8,
                                glyph.is_emoji,
                            );
                            if !self.glyphs.contains_key(&key) {
                                let pixels = system.glyph_pixels(
                                    run.font_id,
                                    glyph.id,
                                    run.font_size,
                                    raster,
                                    key.4,
                                    key.5,
                                );
                                self.glyph_bytes +=
                                    pixels.as_ref().map_or(0, |pixels| pixels.2.len()) + 64;
                                self.glyphs.insert(key, pixels);
                            }
                            if let Some((origin, size, _)) = &self.glyphs[&key] {
                                let x = x.floor() as i64 + i64::from(origin.x.0);
                                let y = (fy + f32::from(glyph.position.y) * raster).round() as i64
                                    + i64::from(origin.y.0);
                                [left, top] = [left.min(x), top.min(y)];
                                [right, bottom] = [
                                    right.max(x + i64::from(size.width.0)),
                                    bottom.max(y + i64::from(size.height.0)),
                                ];
                                placed.push((key, x, y));
                            }
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
                        // WARNING: a text that cannot draw keeps the canvas, so a covering mode gets it back.
                        if let Some(saved) = covered.take() {
                            self.pixels = saved;
                        }
                        continue;
                    };
                    let stride = layer.width() as usize;
                    let out = layer.pixels_mut();
                    let source = if *code == 24. {
                        &state.fill
                    } else {
                        &state.stroke
                    };
                    let fill = match source {
                        skia::Shader::SolidColor(color) => color.to_color_u8(),
                        _ => skia::ColorU8::from_rgba(255, 255, 255, 255),
                    };
                    for (key, x, y) in placed {
                        let Some((_, size, bytes)) = &self.glyphs[&key] else {
                            continue;
                        };
                        let columns = size.width.0 as usize;
                        let depth = bytes.len() / (columns * size.height.0 as usize).max(1);
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
                            .filter(|_| !matches!(source, skia::Shader::SolidColor(_)))
                            .zip(to_canvas.invert())
                    {
                        let (rect, to_layer) = to_layer;
                        layer.fill_rect(
                            rect,
                            &paint(
                                source,
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
                        state.clip.as_deref(),
                    );
                }
                25 if a.iter().all(|v| v.is_finite()) && a[5] != 0. && a[6] != 0. => {
                    let [iw, ih, sx, sy, sw, sh, dx, dy, dw, dh] =
                        [a[1], a[2], a[3], a[4], a[5], a[6], a[7], a[8], a[9], a[10]];
                    let (kx, ky) = (dw / sw, dh / sh);
                    // WARNING: an SVG draws at the size it covers on the canvas, so `RASTER_MAX` caps its memory.
                    let size = [iw * kx * t[0].hypot(t[1]), ih * ky * t[2].hypot(t[3])]
                        .map(|side| side.abs().ceil().min(f64::from(RASTER_MAX)) as u32);
                    // WARNING: a negative node draws the canvas onto itself, so it is copied for each call.
                    let own;
                    let source = if a[0] < 0. {
                        own = covered.as_ref().unwrap_or(&self.pixels).clone();
                        &own
                    } else {
                        let key = (a[0] as usize, size);
                        let Some(fetched) = sources.entry(key).or_insert_with(|| image(key)) else {
                            continue;
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
                25 => {}
                28 | 29 => {
                    let args = if *code == 28. {
                        [a[0], a[1], a[2], a[2], 0., a[3], a[4], a[5]]
                    } else {
                        [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]]
                    };
                    if args.iter().all(|v| v.is_finite()) {
                        arc(&mut self.path, t, args);
                    }
                }
                30 if a.iter().all(|v| v.is_finite()) => {
                    let [x1, y1, x2, y2, r] = [a[0], a[1], a[2], a[3], a[4]];
                    let Some([(cx, cy)]) = device(t, [[x1, y1]]) else {
                        continue;
                    };
                    let Some(last) = self.path.last_point().filter(|_| !self.path.is_empty())
                    else {
                        self.path.move_to(cx, cy);
                        continue;
                    };
                    let [ta, tb, tc, td, te, tf] = t;
                    let det = ta * td - tb * tc;
                    let [lx, ly] = [f64::from(last.x) - te, f64::from(last.y) - tf];
                    let from = [(td * lx - tc * ly) / det, (ta * ly - tb * lx) / det];
                    let from = if from.iter().all(|v| v.is_finite()) {
                        from
                    } else {
                        [f64::from(last.x), f64::from(last.y)]
                    };
                    let (dx0, dy0, dx1, dy1) = (x1 - from[0], y1 - from[1], x2 - x1, y2 - y1);
                    let (len0, len1) = (dx0.hypot(dy0), dx1.hypot(dy1));
                    let cross = dx0 * dy1 - dy0 * dx1;
                    if len0 == 0. || len1 == 0. || r == 0. || cross.abs() < 1e-6 {
                        self.path.line_to(cx, cy);
                        continue;
                    }
                    let (ux0, uy0, ux1, uy1) = (dx0 / len0, dy0 / len0, dx1 / len1, dy1 / len1);
                    let turn = (ux0 * uy1 - uy0 * ux1).atan2(ux0 * ux1 + uy0 * uy1).abs();
                    let (reach, k) = (r * (turn / 2.).tan(), r * 4. / 3. * (turn / 4.).tan());
                    let (sx, sy) = (x1 - ux0 * reach, y1 - uy0 * reach);
                    let (ex, ey) = (x1 + ux1 * reach, y1 + uy1 * reach);
                    let Some([start, c1, c2, end]) = device(
                        t,
                        [
                            [sx, sy],
                            [sx + ux0 * k, sy + uy0 * k],
                            [ex - ux1 * k, ey - uy1 * k],
                            [ex, ey],
                        ],
                    ) else {
                        continue;
                    };
                    self.path.line_to(start.0, start.1);
                    self.path.cubic_to(c1.0, c1.1, c2.0, c2.1, end.0, end.1);
                }
                30 => {}
                31 if a.iter().all(|v| v.is_finite()) => {
                    let [x, y, w, h] = [a[0], a[1], a[2], a[3]];
                    let (right, bottom) = (x + w, y + h);
                    let (sx, sy) = (w.signum(), h.signum());
                    let r: [f64; 8] =
                        std::array::from_fn(|i| a[4 + i] * if i % 2 == 0 { sx } else { sy });
                    const CORNER_CURVE: f64 = 0.447_715_25;
                    #[rustfmt::skip]
                    let Some(points) = device(t, [
                        [x + r[0], y],
                        [right - r[2], y], [right - r[2] * CORNER_CURVE, y], [right, y + r[3] * CORNER_CURVE], [right, y + r[3]],
                        [right, bottom - r[5]], [right, bottom - r[5] * CORNER_CURVE], [right - r[4] * CORNER_CURVE, bottom], [right - r[4], bottom],
                        [x + r[6], bottom], [x + r[6] * CORNER_CURVE, bottom], [x, bottom - r[7] * CORNER_CURVE], [x, bottom - r[7]],
                        [x, y + r[1]], [x, y + r[1] * CORNER_CURVE], [x + r[0] * CORNER_CURVE, y], [x + r[0], y],
                        [x, y],
                    ]) else {
                        continue;
                    };
                    self.path.move_to(points[0].0, points[0].1);
                    for [line, c1, c2, end] in points[1..17].as_chunks::<4>().0 {
                        self.path.line_to(line.0, line.1);
                        self.path.cubic_to(c1.0, c1.1, c2.0, c2.1, end.0, end.1);
                    }
                    self.path.close();
                    self.path.move_to(points[17].0, points[17].1);
                }
                31 => {}
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
                39 => state.spacing[1] = a[0] as f32,
                40 => state.smoothing = a[0] != 0.,
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
        if self.present {
            self.image = render_image(&self.pixels);
        }
        Ok(())
    }

    pub fn hit(&self, x: f32, y: f32, even_odd: bool, stroke: bool) -> bool {
        let Some(path) = self.path.clone().finish() else {
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

pub fn svg(
    source: &str,
    width: u32,
    height: u32,
    transform: Option<([f32; 2], [f32; 6])>,
) -> Result<Arc<renderer::RenderImage>> {
    render_svg(&svg_tree(source)?, width, height, transform)
}

pub fn svg_tree(source: &str) -> Result<usvg::Tree> {
    Ok(usvg::Tree::from_str(
        source,
        &usvg::Options {
            fontdb: fonts(source.contains("<text")).0.clone(),
            ..Default::default()
        },
    )?)
}

pub fn render_svg(
    tree: &usvg::Tree,
    width: u32,
    height: u32,
    transform: Option<([f32; 2], [f32; 6])>,
) -> Result<Arc<renderer::RenderImage>> {
    Ok(render_image(&svg_pixels(tree, width, height, transform)?))
}

// WARNING: every SVG raster goes through here, so `RASTER_MAX` bounds them all. A larger size draws scaled
// down into a raster that fits, and `transform` stays in the pixels of the asked size.
pub fn svg_pixels(
    tree: &usvg::Tree,
    width: u32,
    height: u32,
    transform: Option<([f32; 2], [f32; 6])>,
) -> Result<skia::Pixmap> {
    let [width, height] = [width.max(1), height.max(1)];
    let [w, h] = [width.min(RASTER_MAX), height.min(RASTER_MAX)];
    let mut pixels = skia::Pixmap::new(w, h).context("Cannot allocate SVG image")?;
    let size = tree.size();
    let ([fit_width, fit_height], [a, b, c, d, e, f]) =
        transform.unwrap_or(([width as f32, height as f32], [1., 0., 0., 1., 0., 0.]));
    resvg::render(
        tree,
        skia::Transform::from_scale(w as f32 / width as f32, h as f32 / height as f32)
            .pre_concat(skia::Transform::from_row(a, b, c, d, e, f))
            .pre_scale(fit_width / size.width(), fit_height / size.height()),
        &mut pixels.as_mut(),
    );
    Ok(pixels)
}

pub type SvgGeometry = ([f32; 2], [f32; 4], Vec<(String, [f32; 4], [f32; 4])>);

pub fn svg_boxes(tree: &usvg::Tree) -> SvgGeometry {
    let rect = |rect: usvg::Rect| [rect.x(), rect.y(), rect.width(), rect.height()];
    let mut boxes = Vec::new();
    // WARNING: usvg applies the transform of a `<use>` twice in `abs_transform`, so canvas boxes come from the
    // relative transforms.
    let root = tree.root().abs_transform();
    let mut pending: Vec<_> = tree
        .root()
        .children()
        .iter()
        .map(|node| (node, root))
        .collect();
    while let Some((node, transform)) = pending.pop() {
        let transform = match node {
            usvg::Node::Group(group) => transform.pre_concat(group.transform()),
            _ => transform,
        };
        if !node.id().is_empty() {
            boxes.push((
                node.id().to_owned(),
                rect(
                    node.bounding_box()
                        .transform(transform)
                        .unwrap_or(node.abs_bounding_box()),
                ),
                rect(node.bounding_box()),
            ));
        }
        if let usvg::Node::Group(group) = node {
            pending.extend(group.children().iter().map(|child| (child, transform)));
        }
    }
    let content = match tree.root().children() {
        [usvg::Node::Group(group)] if group.id().is_empty() => group.bounding_box(),
        _ => tree.root().bounding_box(),
    };
    (
        [tree.size().width(), tree.size().height()],
        rect(content),
        boxes,
    )
}

pub fn svg_shape(source: &str) -> Option<([f32; 4], f32)> {
    type Shapes = std::collections::HashMap<String, Option<([f32; 4], f32)>>;
    thread_local! {
        static SHAPES: std::cell::RefCell<Shapes> = Default::default();
    }
    if let Some(shape) = SHAPES.with_borrow(|shapes| shapes.get(source).copied()) {
        return shape;
    }
    let shape = svg_tree(source).ok().and_then(|tree| {
        let node = tree.node_by_id("apps-shape")?;
        let rect = node.bounding_box();
        let length = match node {
            usvg::Node::Path(path) => {
                let (mut length, mut start, mut last) =
                    (0., skia::Point::zero(), skia::Point::zero());
                let mut add = |from: skia::Point, to: skia::Point| {
                    length += ((to.x - from.x).powi(2) + (to.y - from.y).powi(2)).sqrt()
                };
                for segment in path.data().segments() {
                    match segment {
                        skia::PathSegment::MoveTo(point) => (start, last) = (point, point),
                        skia::PathSegment::LineTo(point) => {
                            add(last, point);
                            last = point;
                        }
                        skia::PathSegment::QuadTo(control, point) => {
                            let mut from = last;
                            for step in 1..=16 {
                                let t = step as f32 / 16.;
                                let u = 1. - t;
                                let to = skia::Point::from_xy(
                                    u * u * last.x + 2. * u * t * control.x + t * t * point.x,
                                    u * u * last.y + 2. * u * t * control.y + t * t * point.y,
                                );
                                add(from, to);
                                from = to;
                            }
                            last = point;
                        }
                        skia::PathSegment::CubicTo(first, second, point) => {
                            let mut from = last;
                            for step in 1..=16 {
                                let t = step as f32 / 16.;
                                let u = 1. - t;
                                let [a, b, c, d] =
                                    [u * u * u, 3. * u * u * t, 3. * u * t * t, t * t * t];
                                let to = skia::Point::from_xy(
                                    a * last.x + b * first.x + c * second.x + d * point.x,
                                    a * last.y + b * first.y + c * second.y + d * point.y,
                                );
                                add(from, to);
                                from = to;
                            }
                            last = point;
                        }
                        skia::PathSegment::Close => {
                            add(last, start);
                            last = start;
                        }
                    }
                }
                length
            }
            _ => 0.,
        };
        Some(([rect.x(), rect.y(), rect.width(), rect.height()], length))
    });
    SHAPES.with_borrow_mut(|shapes| {
        if shapes.len() >= 256 {
            shapes.clear();
        }
        shapes.insert(source.to_owned(), shape);
    });
    shape
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(canvas.hit(20., 5.5, false, true));
        assert!(!canvas.hit(20., 0.5, false, true));
        draw(&mut canvas, &[11., 17., 2., 2., 6., 6.], "");
        assert!(canvas.hit(4., 4., false, false));
        assert!(!canvas.hit(20., 4., false, false));
        draw(&mut canvas, &[2., 2., 0., 0., 2., 3., 4.], "");
        assert_eq!(canvas.transform(), [2., 0., 0., 2., 3., 4.]);
    }

    #[test]
    fn svg_boxes_map_elements_to_canvas_and_own_coordinates() {
        let (size, content, boxes) = svg_boxes(&svg_tree(r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 100 50"><rect id="r" x="10" y="5" width="30" height="20"/><g transform="translate(5 30)"><rect id="apps-7" width="10" height="10"/></g></svg>"##).unwrap());
        assert_eq!(size, [200., 100.]);
        assert_eq!(content, [5., 5., 35., 35.]);
        let find = |name: &str| {
            boxes
                .iter()
                .find(|(id, ..)| id == name)
                .map(|(_, canvas, object)| (*canvas, *object))
        };
        assert_eq!(find("r"), Some(([20., 10., 60., 40.], [10., 5., 30., 20.])));
        assert_eq!(
            find("apps-7"),
            Some(([10., 60., 20., 20.], [0., 0., 10., 10.]))
        );
        let (_, _, boxes) = svg_boxes(&svg_tree(r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><rect id="s" width="10" height="10"/></defs><use id="u" href="#s" x="20" y="10" transform="scale(2)"/></svg>"##).unwrap());
        assert_eq!(
            boxes
                .iter()
                .find(|(id, ..)| id == "u")
                .map(|(_, canvas, _)| *canvas),
            Some([40., 20., 20., 20.])
        );
    }

    #[test]
    fn images_keep_straight_alpha_at_display_resolution() {
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
        assert_eq!(&canvas.image.bytes()[..4], &[0, 0, 255, 128]);
        assert_eq!(canvas.pixels.pixels()[0].red(), 128);
        let image = svg(r##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><path d="M0 0h2v2H0Z" fill="#ff0000" opacity="0.5"/></svg>"##, 4, 4, None).unwrap();
        assert_eq!(u32::from(image.size().width), 4);
        assert_eq!(image.bytes(), [0, 0, 255, 128].repeat(16));
        let rotated = svg(r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><path d="M0 0h2v2H0Z" fill="#ff0000"/><path d="M2 0h2v2H2Z" fill="#0000ff"/></svg>"##, 1, 2, Some(([2., 1.], [0., 1., -1., 0., 1., 0.]))).unwrap();
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
