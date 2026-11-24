pub mod canvas;
pub mod image;
pub mod resvg;
pub mod svg;

use std::sync::Arc;
use tiny_skia as skia;

pub const RASTER_MAX: u32 = 4096;
// A new buffer costs page faults, about 0.5 ms at 1080p, so `spare` is used when it has the right size.
pub(crate) fn render_image(pixels: &skia::Pixmap, spare: Vec<u8>) -> Arc<renderer::RenderImage> {
    let mut bytes = match spare.len() == pixels.data().len() {
        true => spare,
        false => vec![0; pixels.data().len()],
    };
    for (out, &[r, g, b, a]) in bytes
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(pixels.data().as_chunks::<4>().0)
    {
        *out = [b, g, r, a];
    }
    Arc::new(renderer::RenderImage::new_premultiplied(
        pixels.width(),
        pixels.height(),
        bytes,
    ))
}

pub(crate) fn pixmap(image: &renderer::RenderImage) -> Option<skia::Pixmap> {
    let size = image.size();
    let mut pixels = skia::Pixmap::new(size.width as u32, size.height as u32)?;
    let source = image.bytes().as_chunks::<4>().0;
    if image.is_premultiplied() {
        for (out, &[b, g, r, a]) in pixels
            .data_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(source)
        {
            *out = [r, g, b, a];
        }
    } else {
        for (out, pixel) in pixels.pixels_mut().iter_mut().zip(source) {
            *out = skia::ColorU8::from_rgba(pixel[2], pixel[1], pixel[0], pixel[3]).premultiply();
        }
    }
    Some(pixels)
}
