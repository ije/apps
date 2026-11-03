use anyhow::{Context, Result, ensure};
use resvg::{tiny_skia as skia, usvg};
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

#[derive(Clone)]
struct State {
    transform: skia::Transform,
    fill: skia::Color,
    stroke: skia::Color,
    alpha: f32,
    width: f32,
    font: Rc<str>,
    /// The part of the text width that is left of the anchor: 0, 0.5 or 1.
    align: f32,
    /// 0 alphabetic, 1 top, 2 middle, 3 bottom.
    baseline: u8,
    clip: Option<Rc<skia::Mask>>,
}

/// A 2D context: its pixels, the drawing state, and the current path in canvas pixels.
pub struct Canvas {
    pub pixels: skia::Pixmap,
    pub image: Arc<renderer::RenderImage>,
    state: State,
    stack: Vec<State>,
    path: skia::PathBuilder,
    glyphs: rustc_hash::FxHashMap<GlyphKey, Option<Glyph>>,
    glyph_bytes: usize,
}

/// Font, glyph, size bits, scale bits, subpixel variant and emoji.
type GlyphKey = (usize, u32, u32, u32, u8, bool);

type Glyph = (
    renderer::Point<renderer::DevicePixels>,
    renderer::Size<renderer::DevicePixels>,
    Vec<u8>,
);

/// Lays out canvas text in a `font` value like `bold 14px Inter`. Tabs and newlines become spaces.
pub fn line(
    system: &renderer::TextSystem,
    aliases: &std::collections::HashMap<String, String>,
    font: &str,
    text: &str,
) -> Rc<renderer::LineLayout> {
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
        letter_spacing: renderer::px(0.),
        word_spacing: renderer::px(0.),
        font_size: None,
    };
    let font_size = size
        .split_whitespace()
        .last()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|size| size.is_finite() && *size > 0.)
        .unwrap_or(10.);
    system.layout_line(&text, renderer::px(font_size), &[run], None)
}

/// Web fonts, and the system fonts once `system` asks for them, since loading those takes tens of milliseconds.
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

fn render_image(pixels: &skia::Pixmap) -> Arc<renderer::RenderImage> {
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
            state: State {
                transform: skia::Transform::identity(),
                fill: skia::Color::BLACK,
                stroke: skia::Color::BLACK,
                alpha: 1.,
                width: 1.,
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

    /// Runs the operations of `CanvasRenderingContext2D` in `dom.js`: a code and its arguments, with strings as UTF-16 lengths of consecutive parts of `strings` and `putImageData` bytes in `data`. `image` returns the pixels of another canvas or image node. Text needs the window's text system and font aliases.
    pub fn draw(
        &mut self,
        ops: &[f32],
        mut strings: &str,
        data: &[&[u8]],
        image: &dyn Fn(usize) -> Option<skia::Pixmap>,
        fonts: Option<(
            &renderer::TextSystem,
            &std::collections::HashMap<String, String>,
        )>,
    ) -> Result<()> {
        // WARNING: the codes and argument counts must match `CanvasRenderingContext2D` in `dom.js`.
        const ARGS: [usize; 27] = [
            0, 0, 6, 6, 1, 1, 1, 1, 1, 1, 1, 0, 0, 2, 2, 4, 6, 4, 1, 0, 1, 4, 4, 4, 3, 11, 5,
        ];
        let paint = |mut color: skia::Color, alpha: f32| {
            color.apply_opacity(alpha);
            let mut paint = skia::Paint::default();
            paint.set_color(color);
            paint
        };
        let mut string = |units: f32| {
            let (part, rest) = strings.split_at(crate::dom::utf16_index(strings, units as usize));
            strings = rest;
            part
        };
        let mut sources = rustc_hash::FxHashMap::default();
        let [width, height] = [self.pixels.width(), self.pixels.height()];
        let mut rest = ops;
        while let [code, tail @ ..] = rest {
            let count = *ARGS
                .get(*code as usize)
                .context("Invalid canvas operation")?;
            ensure!(tail.len() >= count, "Invalid canvas operation");
            let (a, tail) = tail.split_at(count);
            rest = tail;
            let state = &mut self.state;
            let point = |x: f32, y: f32| {
                let t = state.transform;
                (t.sx * x + t.kx * y + t.tx, t.ky * x + t.sy * y + t.ty)
            };
            match *code as u8 {
                0 => self.stack.push(state.clone()),
                1 => {
                    if let Some(saved) = self.stack.pop() {
                        *state = saved;
                    }
                }
                2 | 3 if a.iter().all(|v| v.is_finite()) => {
                    let matrix = skia::Transform::from_row(a[0], a[1], a[2], a[3], a[4], a[5]);
                    state.transform = if *code == 2. {
                        state.transform.pre_concat(matrix)
                    } else {
                        matrix
                    };
                }
                2 | 3 => {}
                4 | 5 => {
                    if let Some((rgb, alpha)) = crate::style::color(string(a[0])) {
                        let [_, r, g, b] = rgb.to_be_bytes();
                        let color = skia::Color::from_rgba8(r, g, b, alpha);
                        if *code == 4. {
                            state.fill = color;
                        } else {
                            state.stroke = color;
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
                        "top" | "hanging" => 1,
                        "middle" => 2,
                        "bottom" | "ideographic" => 3,
                        _ => 0,
                    }
                }
                9 => state.alpha = a[0],
                10 => state.width = a[0],
                11 => self.path.clear(),
                12 => self.path.close(),
                13 => {
                    let (x, y) = point(a[0], a[1]);
                    self.path.move_to(x, y);
                }
                14 => {
                    let (x, y) = point(a[0], a[1]);
                    self.path.line_to(x, y);
                }
                15 => {
                    let ((x1, y1), (x, y)) = (point(a[0], a[1]), point(a[2], a[3]));
                    self.path.quad_to(x1, y1, x, y);
                }
                16 => {
                    let ((x1, y1), (x2, y2), (x, y)) =
                        (point(a[0], a[1]), point(a[2], a[3]), point(a[4], a[5]));
                    self.path.cubic_to(x1, y1, x2, y2, x, y);
                }
                17 => {
                    for (i, (x, y)) in [(0., 0.), (a[2], 0.), (a[2], a[3]), (0., a[3])]
                        .into_iter()
                        .enumerate()
                    {
                        let (x, y) = point(a[0] + x, a[1] + y);
                        if i == 0 {
                            self.path.move_to(x, y);
                        } else {
                            self.path.line_to(x, y);
                        }
                    }
                    self.path.close();
                }
                18..=20 => {
                    let rule = if a.first() == Some(&1.) {
                        skia::FillRule::EvenOdd
                    } else {
                        skia::FillRule::Winding
                    };
                    let path = self.path.clone().finish();
                    if *code == 20. {
                        let mut mask = match state.clip.take() {
                            Some(mask) => Rc::unwrap_or_clone(mask),
                            None => {
                                let mut mask = skia::Mask::new(width, height)
                                    .context("Cannot allocate clip mask")?;
                                mask.data_mut().fill(255);
                                mask
                            }
                        };
                        match path {
                            Some(path) => {
                                mask.intersect_path(&path, rule, true, skia::Transform::identity())
                            }
                            None => mask.data_mut().fill(0),
                        }
                        state.clip = Some(Rc::new(mask));
                    } else if let Some(path) = path {
                        let mask = state.clip.as_deref();
                        if *code == 18. {
                            self.pixels.fill_path(
                                &path,
                                &paint(state.fill, state.alpha),
                                rule,
                                skia::Transform::identity(),
                                mask,
                            );
                        } else if let Some(path) =
                            state.transform.invert().and_then(|t| path.transform(t))
                        {
                            // The path is in canvas pixels, while the line width follows the transform.
                            self.pixels.stroke_path(
                                &path,
                                &paint(state.stroke, state.alpha),
                                &skia::Stroke {
                                    width: state.width,
                                    miter_limit: 10.,
                                    ..Default::default()
                                },
                                state.transform,
                                mask,
                            );
                        }
                    }
                }
                21..=23 => {
                    let [x, y, w, h] = [a[0], a[1], a[2], a[3]];
                    let Some(rect) = skia::Rect::from_ltrb(
                        x.min(x + w),
                        y.min(y + h),
                        x.max(x + w),
                        y.max(y + h),
                    ) else {
                        continue;
                    };
                    let mask = state.clip.as_deref();
                    match *code as u8 {
                        21 => self.pixels.fill_rect(
                            rect,
                            &paint(state.fill, state.alpha),
                            state.transform,
                            mask,
                        ),
                        22 => self.pixels.stroke_path(
                            &skia::PathBuilder::from_rect(rect),
                            &paint(state.stroke, state.alpha),
                            &skia::Stroke {
                                width: state.width,
                                miter_limit: 10.,
                                ..Default::default()
                            },
                            state.transform,
                            mask,
                        ),
                        _ => self.pixels.fill_rect(
                            rect,
                            &skia::Paint {
                                blend_mode: skia::BlendMode::Clear,
                                ..Default::default()
                            },
                            state.transform,
                            mask,
                        ),
                    }
                }
                24 => {
                    let value = string(a[0]);
                    let Some((system, aliases)) = fonts else {
                        continue;
                    };
                    let line = line(system, aliases, &state.font, value);
                    let [advance, ascent, descent] =
                        [line.width, line.ascent, line.descent].map(f32::from);
                    let t = state.transform.pre_translate(
                        a[1] - advance * state.align,
                        a[2] + [0., ascent, (ascent - descent) / 2., -descent]
                            [state.baseline as usize],
                    );
                    let scale = (t.sx * t.sy - t.kx * t.ky).abs().sqrt();
                    if !scale.is_finite() || scale <= 1e-3 {
                        continue;
                    }
                    // WARNING: glyphs rasterize at most 1024 pixels per em and larger text scales the layer up, so a huge font or transform cannot allocate huge glyphs.
                    let raster = scale.min(1024. / f32::from(line.font_size));
                    // Glyphs rasterize at the device scale into a layer, which an upright transform places on whole pixels, so text stays as sharp as page text.
                    let upright =
                        raster == scale && t.kx == 0. && t.ky == 0. && t.sx > 0. && t.sx == t.sy;
                    let (fx, fy, to_canvas) = if upright {
                        let [x, y] = [t.tx.floor(), t.ty.floor()];
                        (t.tx - x, t.ty - y, skia::Transform::from_translate(x, y))
                    } else {
                        (0., 0., t.pre_scale(1. / raster, 1. / raster))
                    };
                    // WARNING: glyphs far outside the canvas are skipped, so a long or huge text cannot make a huge layer.
                    let Some(visible) = to_canvas.invert().and_then(|t| {
                        skia::Rect::from_xywh(0., 0., width as f32, height as f32)?.transform(t)
                    }) else {
                        continue;
                    };
                    // WARNING: pages can draw any glyph at any scale, so the cache starts over past 4 MB.
                    if self.glyph_bytes > 4 << 20 {
                        self.glyphs.clear();
                        self.glyph_bytes = 0;
                    }
                    let mut placed = Vec::new();
                    let [mut left, mut top, mut right, mut bottom] =
                        [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
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
                                let x = x.floor() as i32 + origin.x.0;
                                let y = (fy + f32::from(glyph.position.y) * raster).round() as i32
                                    + origin.y.0;
                                [left, top] = [left.min(x), top.min(y)];
                                [right, bottom] =
                                    [right.max(x + size.width.0), bottom.max(y + size.height.0)];
                                placed.push((key, x, y));
                            }
                        }
                    }
                    if right <= left || bottom <= top {
                        continue;
                    }
                    let mut layer = skia::Pixmap::new((right - left) as u32, (bottom - top) as u32)
                        .context("Cannot allocate text layer")?;
                    let stride = layer.width() as usize;
                    let out = layer.pixels_mut();
                    let fill = state.fill.to_color_u8();
                    for (key, x, y) in placed {
                        let Some((_, size, bytes)) = &self.glyphs[&key] else {
                            continue;
                        };
                        let columns = size.width.0 as usize;
                        let depth = bytes.len() / (columns * size.height.0 as usize).max(1);
                        if columns == 0 || !matches!(depth, 1 | 4) {
                            continue;
                        }
                        for (row, line) in bytes.chunks_exact(columns * depth).enumerate() {
                            let at =
                                (y - top) as usize * stride + row * stride + (x - left) as usize;
                            for (pixel, source) in out[at..at + columns]
                                .iter_mut()
                                .zip(line.chunks_exact(depth))
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
                    let paint = skia::PixmapPaint {
                        opacity: state.alpha,
                        quality: if upright {
                            skia::FilterQuality::Nearest
                        } else {
                            skia::FilterQuality::Bilinear
                        },
                        ..Default::default()
                    };
                    self.pixels.draw_pixmap(
                        left,
                        top,
                        layer.as_ref(),
                        &paint,
                        to_canvas,
                        state.clip.as_deref(),
                    );
                }
                25 => {
                    // WARNING: a negative node draws the canvas onto itself, so it is copied for each call.
                    let own;
                    let source = if a[0] < 0. {
                        own = self.pixels.clone();
                        Some(&own)
                    } else {
                        sources
                            .entry(a[0] as usize)
                            .or_insert_with(|| image(a[0] as usize))
                            .as_ref()
                    };
                    let Some(source) = source else {
                        continue;
                    };
                    let [iw, ih, sx, sy, sw, sh, dx, dy, dw, dh] =
                        [a[1], a[2], a[3], a[4], a[5], a[6], a[7], a[8], a[9], a[10]];
                    let Some(rect) =
                        skia::Rect::from_xywh(dx, dy, dw, dh).filter(|_| sw != 0. && sh != 0.)
                    else {
                        continue;
                    };
                    // Source coordinates use the natural size, which can differ from the decoded pixels.
                    let (x, y) = (dw / sw, dh / sh);
                    let paint = skia::Paint {
                        shader: skia::Pattern::new(
                            source.as_ref(),
                            skia::SpreadMode::Pad,
                            skia::FilterQuality::Bicubic,
                            state.alpha,
                            skia::Transform::from_row(
                                x * iw / source.width() as f32,
                                0.,
                                0.,
                                y * ih / source.height() as f32,
                                dx - sx * x,
                                dy - sy * y,
                            ),
                        ),
                        ..Default::default()
                    };
                    self.pixels
                        .fill_rect(rect, &paint, state.transform, state.clip.as_deref());
                }
                _ => {
                    let bytes = data.get(a[0] as usize).context("Expected image data")?;
                    let (x, y, w, h) = (a[1] as i64, a[2] as i64, a[3] as usize, a[4] as usize);
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
        self.image = render_image(&self.pixels);
        Ok(())
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
    let mut pixels = skia::Pixmap::new(width, height).context("Cannot allocate SVG image")?;
    let size = tree.size();
    let ([fit_width, fit_height], [a, b, c, d, e, f]) =
        transform.unwrap_or(([width as f32, height as f32], [1., 0., 0., 1., 0., 0.]));
    resvg::render(
        tree,
        skia::Transform::from_row(a, b, c, d, e, f)
            .pre_scale(fit_width / size.width(), fit_height / size.height()),
        &mut pixels.as_mut(),
    );
    Ok(render_image(&pixels))
}

/// The SVG's size, the box of its content in its own coordinates, and for each element with an id, its box on
/// the canvas and its object box in its own coordinates, as `[x, y, width, height]`.
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
    // A `viewBox` puts the content in one unnamed group, whose box is in the SVG's own coordinates.
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

/// The object box `[x, y, width, height]` and the path length of the element with the id `h2o-shape` in an SVG
/// source, in its own coordinates. Results are cached by source.
pub fn svg_shape(source: &str) -> Option<([f32; 4], f32)> {
    type Shapes = std::collections::HashMap<String, Option<([f32; 4], f32)>>;
    thread_local! {
        static SHAPES: std::cell::RefCell<Shapes> = Default::default();
    }
    if let Some(shape) = SHAPES.with_borrow(|shapes| shapes.get(source).copied()) {
        return shape;
    }
    let shape = svg_tree(source).ok().and_then(|tree| {
        let node = tree.node_by_id("h2o-shape")?;
        let rect = node.bounding_box();
        let length = match node {
            usvg::Node::Path(path) => {
                // Curves are measured as 16 chords each, close to the exact length for SVG shapes.
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

    #[test]
    fn svg_boxes_map_elements_to_canvas_and_own_coordinates() {
        let (size, content, boxes) = svg_boxes(&svg_tree(r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 100 50"><rect id="r" x="10" y="5" width="30" height="20"/><g transform="translate(5 30)"><rect id="h2o-7" width="10" height="10"/></g></svg>"##).unwrap());
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
            find("h2o-7"),
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
        let ops = [6., 15., 7., 6., 24., 2., 150., 18., 24., 10000., -1e6, 18.];
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
            6., 11., 24., 1., 0., 100.,
            6., 11., 24., 1., 0., 10.,
            6., 10., 2., 1e5, 0., 0., 1e5, 0., 0., 24., 1., 0., 0.,
        ];
        let strings = "1e7px serifWNaNpx serifW10px serifW";
        canvas
            .draw(&ops, strings, &[], &|_| None, Some((&system, &aliases)))
            .unwrap();
        assert!(canvas.glyph_bytes < 3 * (1100 * 1100 * 4 + 64));
    }
}
