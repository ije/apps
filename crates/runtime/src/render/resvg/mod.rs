// Copyright 2017 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A copy of resvg 0.46.0, adapted to usvg 0.49.0, whose raster images decode through
// `render::image`, so it must stay in step with the usvg version.

#![allow(clippy::too_many_arguments)]

mod filter;
mod pixmap;

pub use filter::blur;
use std::sync::Arc;

pub fn render(
    tree: &usvg::Tree,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) {
    let [width, height] = [pixmap.width(), pixmap.height()];
    let max_bbox = tiny_skia::IntRect::from_xywh(
        -(width as i32) * 2,
        -(height as i32) * 2,
        width * 5,
        height * 5,
    )
    .unwrap();
    render_nodes(tree.root(), max_bbox, transform, pixmap);
}

pub fn render_nodes(
    parent: &usvg::Group,
    max_bbox: tiny_skia::IntRect,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) {
    for node in parent.children() {
        match node {
            usvg::Node::Group(group) => {
                render_group(group, max_bbox, transform, pixmap);
            }
            usvg::Node::Path(path) => {
                render_path(path, max_bbox, transform, pixmap);
            }
            usvg::Node::Image(image) => {
                render_image(image, transform, pixmap);
            }
            usvg::Node::Text(text) => {
                render_group(text.flattened(), max_bbox, transform, pixmap);
            }
        }
    }
}

fn render_group(
    group: &usvg::Group,
    max_bbox: tiny_skia::IntRect,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) -> Option<()> {
    let transform = transform.pre_concat(group.transform());

    if !group.should_isolate() {
        render_nodes(group, max_bbox, transform, pixmap);
        return Some(());
    }

    let bbox = group.layer_bounding_box().transform(transform)?;

    let ibbox = if group.filters().is_empty() {
        // Limit unfiltered layers to parent bounds. Per-pixel effects need no extra area.
        let parent = tiny_skia::IntRect::from_xywh(0, 0, pixmap.width(), pixmap.height())?;
        tiny_skia::IntRect::from_xywh(
            (bbox.x().floor() as i32).checked_sub(2)?,
            (bbox.y().floor() as i32).checked_sub(2)?,
            (bbox.width().ceil() as u32).checked_add(4)?,
            (bbox.height().ceil() as u32).checked_add(4)?,
        )?
        .intersect(&parent)?
    } else {
        // Don't expand the box of a group with filters by 2px, since it already acts as the clipping region.
        bbox.to_int_rect().intersect(&max_bbox)?
    };

    let transform = tiny_skia::Transform::from_translate(-ibbox.x() as f32, -ibbox.y() as f32)
        .pre_concat(transform);

    let Some(mut sub_pixmap) = tiny_skia::Pixmap::new(ibbox.width(), ibbox.height()) else {
        log::warn!("Failed to allocate a group layer for: {:?}.", ibbox);
        return None;
    };

    render_nodes(group, max_bbox, transform, &mut sub_pixmap.as_mut());

    for filter in group.filters() {
        filter::apply(filter, transform, &mut sub_pixmap, render_nodes);
    }

    if let Some(clip_path) = group.clip_path() {
        self::clip(clip_path, transform, &mut sub_pixmap);
    }

    if let Some(mask) = group.mask() {
        self::mask(mask, max_bbox, transform, &mut sub_pixmap);
    }

    let blend_mode = pixmap::convert_blend_mode(group.blend_mode());
    if blend_mode == tiny_skia::BlendMode::SourceOver {
        pixmap::draw_over(
            pixmap,
            sub_pixmap.as_ref(),
            ibbox.x(),
            ibbox.y(),
            group.opacity().get(),
        );
    } else {
        let paint = tiny_skia::PixmapPaint {
            opacity: group.opacity().get(),
            blend_mode,
            quality: tiny_skia::FilterQuality::Nearest,
        };
        pixmap.draw_pixmap(
            ibbox.x(),
            ibbox.y(),
            sub_pixmap.as_ref(),
            &paint,
            tiny_skia::Transform::identity(),
            None,
        );
    }

    Some(())
}

type Decoded = Vec<(Arc<Vec<u8>>, Arc<renderer::RenderImage>)>;

thread_local! {
    static DECODED: std::cell::RefCell<Decoded> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The data of an image that draws `bitmap` inside `decoded()`. usvg reads only the size of its PNG header.
pub fn stub(bitmap: &renderer::RenderImage) -> Arc<Vec<u8>> {
    let size = bitmap.size();
    let mut data = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    data.extend((size.width as u32).to_be_bytes());
    data.extend((size.height as u32).to_be_bytes());
    Arc::new(data)
}

/// Runs `run` with the bitmaps of `stub()` data, so they draw without a decode.
pub fn decoded<R>(bitmaps: Decoded, run: impl FnOnce() -> R) -> R {
    DECODED.set(bitmaps);
    let result = run();
    DECODED.take();
    result
}

fn render_image(
    image: &usvg::Image,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) {
    if !image.is_visible() {
        return;
    }

    match image.kind() {
        usvg::ImageKind::SVG(tree) => {
            render_vector(tree, transform, pixmap);
        }
        usvg::ImageKind::JPEG(data)
        | usvg::ImageKind::PNG(data)
        | usvg::ImageKind::GIF(data)
        | usvg::ImageKind::WEBP(data) => {
            let bitmap = DECODED.with_borrow(|bitmaps| {
                bitmaps
                    .iter()
                    .find(|(stub, _)| Arc::ptr_eq(stub, data))
                    .map(|(_, bitmap)| bitmap.clone())
            });
            let pixels = match bitmap {
                Some(bitmap) => {
                    let size = bitmap.size();
                    let [width, height] = [size.width, size.height].map(|side| side as usize);
                    Some((bitmap.bytes().to_vec(), width, height))
                }
                None => crate::render::image::pixels(data)
                    .map_err(|error| log::warn!("Failed to decode an image: {error}."))
                    .ok(),
            };
            if let Some(pixels) = pixels {
                render_raster(pixels, transform, image.rendering_mode(), pixmap);
            }
        }
    }
}

fn render_vector(
    tree: &usvg::Tree,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) {
    let mut sub_pixmap = tiny_skia::Pixmap::new(pixmap.width(), pixmap.height()).unwrap();
    render(tree, transform, &mut sub_pixmap.as_mut());
    pixmap::draw_over(pixmap, sub_pixmap.as_ref(), 0, 0, 1.0);
}

fn render_raster(
    (mut pixels, width, height): (Vec<u8>, usize, usize),
    transform: tiny_skia::Transform,
    rendering_mode: usvg::ImageRendering,
    pixmap: &mut tiny_skia::PixmapMut,
) -> Option<()> {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let [b, g, r, a] = *pixel;
        let color = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
        *pixel = [color.red(), color.green(), color.blue(), a];
    }
    let raster = tiny_skia::Pixmap::from_vec(
        pixels,
        tiny_skia::IntSize::from_wh(width as u32, height as u32)?,
    )?;

    let rect = tiny_skia::Size::from_wh(raster.width() as f32, raster.height() as f32)?
        .to_rect(0.0, 0.0)?;

    let quality = match rendering_mode {
        usvg::ImageRendering::OptimizeQuality | usvg::ImageRendering::HighQuality => {
            tiny_skia::FilterQuality::Bicubic
        }
        usvg::ImageRendering::OptimizeSpeed
        | usvg::ImageRendering::CrispEdges
        | usvg::ImageRendering::Pixelated => tiny_skia::FilterQuality::Nearest,
        usvg::ImageRendering::Smooth => tiny_skia::FilterQuality::Bilinear,
    };

    let pattern = tiny_skia::Pattern::new(
        raster.as_ref(),
        tiny_skia::SpreadMode::Pad,
        quality,
        1.0,
        tiny_skia::Transform::default(),
    );
    let paint = tiny_skia::Paint {
        shader: pattern,
        ..Default::default()
    };

    pixmap.fill_rect(rect, &paint, transform, None);

    Some(())
}

fn clip(clip: &usvg::ClipPath, transform: tiny_skia::Transform, pixmap: &mut tiny_skia::Pixmap) {
    // A single path can fill a mask directly. Keep pixmaps for multiple paths: overlap rounding differs.
    let mut ts = transform.pre_concat(clip.transform());
    let mut group = clip.root();
    let path = loop {
        match group.children() {
            [usvg::Node::Path(path)] => break Some(path),
            [usvg::Node::Group(child)] if child.clip_path().is_none() => {
                ts = ts.pre_concat(child.transform());
                group = child;
            }
            [usvg::Node::Text(text)] => group = text.flattened(),
            _ => break None,
        }
    };
    if let Some(path) = path {
        let Some(mut mask) = tiny_skia::Mask::new(pixmap.width(), pixmap.height()) else {
            return;
        };
        let bounds = path.data().bounds();
        if let Some(fill) = path.fill()
            && path.is_visible()
            && bounds.width() != 0.0
            && bounds.height() != 0.0
        {
            let rule = match fill.rule() {
                usvg::FillRule::NonZero => tiny_skia::FillRule::Winding,
                usvg::FillRule::EvenOdd => tiny_skia::FillRule::EvenOdd,
            };
            let anti_alias = path.rendering_mode().use_shape_antialiasing();
            mask.fill_path(path.data(), rule, anti_alias, ts);
        }
        if let Some(clip) = clip.clip_path() {
            self::clip(clip, transform, pixmap);
        }
        pixmap::apply_mask(pixmap, &mask);
        return;
    }

    let mut clip_pixmap = tiny_skia::Pixmap::new(pixmap.width(), pixmap.height()).unwrap();
    clip_pixmap.fill(tiny_skia::Color::BLACK);

    clip_children(
        clip.root(),
        tiny_skia::BlendMode::Clear,
        transform.pre_concat(clip.transform()),
        &mut clip_pixmap.as_mut(),
    );

    if let Some(clip) = clip.clip_path() {
        self::clip(clip, transform, pixmap);
    }

    let mut mask = tiny_skia::Mask::from_pixmap(clip_pixmap.as_ref(), tiny_skia::MaskType::Alpha);
    mask.invert();
    pixmap::apply_mask(pixmap, &mask);
}

fn clip_children(
    parent: &usvg::Group,
    mode: tiny_skia::BlendMode,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) {
    for child in parent.children() {
        match child {
            usvg::Node::Path(path) => {
                if !path.is_visible() {
                    continue;
                }

                let max_bbox = tiny_skia::IntRect::from_xywh(0, 0, 1, 1).unwrap();
                fill_path(path, mode, max_bbox, transform, pixmap);
            }
            usvg::Node::Text(text) => {
                clip_children(text.flattened(), mode, transform, pixmap);
            }
            usvg::Node::Group(group) => {
                let transform = transform.pre_concat(group.transform());

                if let Some(clip) = group.clip_path() {
                    clip_group(group, clip, transform, pixmap);
                } else {
                    clip_children(group, mode, transform, pixmap);
                }
            }
            _ => {}
        }
    }
}

fn clip_group(
    children: &usvg::Group,
    clip: &usvg::ClipPath,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) {
    let mut clip_pixmap = tiny_skia::Pixmap::new(pixmap.width(), pixmap.height()).unwrap();

    clip_children(
        children,
        tiny_skia::BlendMode::SourceOver,
        transform,
        &mut clip_pixmap.as_mut(),
    );
    self::clip(clip, transform, &mut clip_pixmap);

    let paint = tiny_skia::PixmapPaint {
        blend_mode: tiny_skia::BlendMode::Xor,
        ..Default::default()
    };
    pixmap.draw_pixmap(
        0,
        0,
        clip_pixmap.as_ref(),
        &paint,
        tiny_skia::Transform::identity(),
        None,
    );
}

fn mask(
    mask: &usvg::Mask,
    max_bbox: tiny_skia::IntRect,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::Pixmap,
) {
    if mask.root().children().is_empty() {
        pixmap.fill(tiny_skia::Color::TRANSPARENT);
        return;
    }

    let mut mask_pixmap = tiny_skia::Pixmap::new(pixmap.width(), pixmap.height()).unwrap();

    {
        let mut alpha_mask = tiny_skia::Mask::new(pixmap.width(), pixmap.height()).unwrap();
        alpha_mask.fill_path(
            &tiny_skia::PathBuilder::from_rect(mask.rect().to_rect()),
            tiny_skia::FillRule::Winding,
            true,
            transform,
        );

        render_nodes(mask.root(), max_bbox, transform, &mut mask_pixmap.as_mut());

        pixmap::apply_mask(&mut mask_pixmap, &alpha_mask);
    }

    if let Some(mask) = mask.mask() {
        self::mask(mask, max_bbox, transform, pixmap);
    }

    let mask_type = match mask.kind() {
        usvg::MaskType::Luminance => tiny_skia::MaskType::Luminance,
        usvg::MaskType::Alpha => tiny_skia::MaskType::Alpha,
    };

    let mask = tiny_skia::Mask::from_pixmap(mask_pixmap.as_ref(), mask_type);
    pixmap::apply_mask(pixmap, &mask);
}

fn render_path(
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

fn fill_path(
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
    render_nodes(pattern.root(), max_bbox, transform, &mut pixmap.as_mut());

    let mut ts = tiny_skia::Transform::default();
    ts = ts.pre_concat(pattern.transform());
    ts = ts.pre_translate(rect.x(), rect.y());
    ts = ts.pre_scale(1.0 / sx, 1.0 / sy);

    Some((pixmap, ts))
}
