// Copyright 2018 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::sync::Arc;

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
    super::render(tree, transform, &mut sub_pixmap.as_mut());
    super::render::draw_over(pixmap, sub_pixmap.as_ref(), 0, 0, 1.0);
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
