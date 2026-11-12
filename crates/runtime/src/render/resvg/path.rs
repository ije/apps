// Copyright 2019 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

pub fn render(
    path: &usvg::Path,
    max_bbox: tiny_skia::IntRect,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) {
    if !path.is_visible() {
        return;
    }

    let blend_mode = tiny_skia::BlendMode::SourceOver;
    if path.paint_order() == usvg::PaintOrder::FillAndStroke {
        fill_path(path, blend_mode, max_bbox, transform, pixmap);
        stroke_path(path, max_bbox, transform, pixmap);
    } else {
        stroke_path(path, max_bbox, transform, pixmap);
        fill_path(path, blend_mode, max_bbox, transform, pixmap);
    }
}

pub fn fill_path(
    path: &usvg::Path,
    blend_mode: tiny_skia::BlendMode,
    max_bbox: tiny_skia::IntRect,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) -> Option<()> {
    let fill = path.fill()?;

    if path.data().bounds().width() == 0.0 || path.data().bounds().height() == 0.0 {
        return None;
    }

    let rule = match fill.rule() {
        usvg::FillRule::NonZero => tiny_skia::FillRule::Winding,
        usvg::FillRule::EvenOdd => tiny_skia::FillRule::EvenOdd,
    };

    let mut pattern = None;
    let mut paint = convert_paint(
        fill.paint(),
        fill.opacity(),
        max_bbox,
        transform,
        &mut pattern,
    )?;
    paint.anti_alias = path.rendering_mode().use_shape_antialiasing();
    paint.blend_mode = blend_mode;

    pixmap.fill_path(path.data(), &paint, rule, transform, None);
    Some(())
}

fn stroke_path(
    path: &usvg::Path,
    max_bbox: tiny_skia::IntRect,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) -> Option<()> {
    let stroke = path.stroke()?;
    let mut pattern = None;
    let mut paint = convert_paint(
        stroke.paint(),
        stroke.opacity(),
        max_bbox,
        transform,
        &mut pattern,
    )?;
    paint.anti_alias = path.rendering_mode().use_shape_antialiasing();

    pixmap.stroke_path(path.data(), &paint, &stroke.to_tiny_skia(), transform, None);

    Some(())
}

fn convert_paint<'a>(
    paint: &'a usvg::Paint,
    opacity: usvg::Opacity,
    max_bbox: tiny_skia::IntRect,
    transform: tiny_skia::Transform,
    pattern_pixmap: &'a mut Option<tiny_skia::Pixmap>,
) -> Option<tiny_skia::Paint<'a>> {
    let mut result = tiny_skia::Paint::default();
    match paint {
        usvg::Paint::Color(c) => {
            result.set_color_rgba8(c.red, c.green, c.blue, opacity.to_u8());
        }
        usvg::Paint::LinearGradient(lg) => {
            result.shader = convert_linear_gradient(lg, opacity)?;
        }
        usvg::Paint::RadialGradient(rg) => {
            result.shader = convert_radial_gradient(rg, opacity)?;
        }
        usvg::Paint::Pattern(pattern) => {
            let (patt_pix, patt_ts) = render_pattern_pixmap(pattern, max_bbox, transform)?;
            result.shader = tiny_skia::Pattern::new(
                pattern_pixmap.insert(patt_pix).as_ref(),
                tiny_skia::SpreadMode::Repeat,
                tiny_skia::FilterQuality::Bicubic,
                opacity.get(),
                patt_ts,
            );
        }
    }
    Some(result)
}

fn convert_linear_gradient(
    gradient: &usvg::LinearGradient,
    opacity: usvg::Opacity,
) -> Option<tiny_skia::Shader<'_>> {
    let (mode, points) = convert_base_gradient(gradient, opacity);

    let shader = tiny_skia::LinearGradient::new(
        (gradient.x1(), gradient.y1()).into(),
        (gradient.x2(), gradient.y2()).into(),
        points,
        mode,
        gradient.transform(),
    )?;

    Some(shader)
}

fn convert_radial_gradient(
    gradient: &usvg::RadialGradient,
    opacity: usvg::Opacity,
) -> Option<tiny_skia::Shader<'_>> {
    let (mode, points) = convert_base_gradient(gradient, opacity);

    let shader = tiny_skia::RadialGradient::new(
        (gradient.fx(), gradient.fy()).into(),
        gradient.fr().get(),
        (gradient.cx(), gradient.cy()).into(),
        gradient.r().get(),
        points,
        mode,
        gradient.transform(),
    )?;

    Some(shader)
}

fn convert_base_gradient(
    gradient: &usvg::BaseGradient,
    opacity: usvg::Opacity,
) -> (tiny_skia::SpreadMode, Vec<tiny_skia::GradientStop>) {
    let mode = match gradient.spread_method() {
        usvg::SpreadMethod::Pad => tiny_skia::SpreadMode::Pad,
        usvg::SpreadMethod::Reflect => tiny_skia::SpreadMode::Reflect,
        usvg::SpreadMethod::Repeat => tiny_skia::SpreadMode::Repeat,
    };

    let mut points = Vec::with_capacity(gradient.stops().len());
    for stop in gradient.stops() {
        let alpha = stop.opacity() * opacity;
        let color = tiny_skia::Color::from_rgba8(
            stop.color().red,
            stop.color().green,
            stop.color().blue,
            alpha.to_u8(),
        );
        points.push(tiny_skia::GradientStop::new(stop.offset().get(), color));
    }

    (mode, points)
}

fn render_pattern_pixmap(
    pattern: &usvg::Pattern,
    max_bbox: tiny_skia::IntRect,
    transform: tiny_skia::Transform,
) -> Option<(tiny_skia::Pixmap, tiny_skia::Transform)> {
    let (sx, sy) = {
        let ts2 = transform.pre_concat(pattern.transform());
        ts2.get_scale()
    };

    let rect = pattern.rect();
    let img_size = tiny_skia::IntSize::from_wh(
        (rect.width() * sx).round() as u32,
        (rect.height() * sy).round() as u32,
    )?;
    let mut pixmap = tiny_skia::Pixmap::new(img_size.width(), img_size.height())?;

    let transform = tiny_skia::Transform::from_scale(sx, sy);
    super::render::render_nodes(pattern.root(), max_bbox, transform, &mut pixmap.as_mut());

    let mut ts = tiny_skia::Transform::default();
    ts = ts.pre_concat(pattern.transform());
    ts = ts.pre_translate(rect.x(), rect.y());
    ts = ts.pre_scale(1.0 / sx, 1.0 / sy);

    Some((pixmap, ts))
}
