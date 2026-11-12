// Copyright 2017 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A copy of resvg 0.46.0, adapted to usvg 0.49.0, whose raster images decode through
// `render::image`, so it must stay in step with the usvg version.

#![allow(clippy::too_many_arguments)]

mod clip;
mod filter;
mod image;
mod mask;
mod path;
mod render;

pub use filter::blur;

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
    render::render_nodes(tree.root(), max_bbox, transform, pixmap);
}
