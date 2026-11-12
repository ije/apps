// Copyright 2017 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A copy of resvg 0.46.0, adapted to usvg 0.49.0, whose raster images decode through
// `render::image`, so it must stay in step with the usvg version.

#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::identity_op)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::upper_case_acronyms)]
#![allow(clippy::wrong_self_convention)]

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
    let target_size = tiny_skia::IntSize::from_wh(pixmap.width(), pixmap.height()).unwrap();
    let max_bbox = tiny_skia::IntRect::from_xywh(
        -(target_size.width() as i32) * 2,
        -(target_size.height() as i32) * 2,
        target_size.width() * 5,
        target_size.height() * 5,
    )
    .unwrap();

    let ctx = render::Context { max_bbox };
    render::render_nodes(tree.root(), &ctx, transform, pixmap);
}
