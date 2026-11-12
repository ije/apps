// Copyright 2018 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

pub fn render(
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
            render_raster(data, transform, image.rendering_mode(), pixmap);
        }
    }
}

fn render_vector(
    tree: &usvg::Tree,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) {
    let mut sub_pixmap = tiny_skia::Pixmap::new(pixmap.width(), pixmap.height()).unwrap();
    super::render(tree, transform, &mut sub_pixmap.as_mut());
    super::render::draw_over(pixmap, sub_pixmap.as_ref(), 0, 0, 1.0);
}

fn render_raster(
    data: &[u8],
    transform: tiny_skia::Transform,
    rendering_mode: usvg::ImageRendering,
    pixmap: &mut tiny_skia::PixmapMut,
) -> Option<()> {
    let (mut pixels, width, height) = crate::render::image::pixels(data)
        .map_err(|error| log::warn!("Failed to decode an image: {error}."))
        .ok()?;
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
