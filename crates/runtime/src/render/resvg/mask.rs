// Copyright 2019 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::render::Context;

pub fn apply(
    mask: &usvg::Mask,
    ctx: &Context,
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

        super::render::render_nodes(mask.root(), ctx, transform, &mut mask_pixmap.as_mut());

        super::render::apply_mask(&mut mask_pixmap, &alpha_mask);
    }

    if let Some(mask) = mask.mask() {
        self::apply(mask, ctx, transform, pixmap);
    }

    let mask_type = match mask.kind() {
        usvg::MaskType::Luminance => tiny_skia::MaskType::Luminance,
        usvg::MaskType::Alpha => tiny_skia::MaskType::Alpha,
    };

    let mask = tiny_skia::Mask::from_pixmap(mask_pixmap.as_ref(), mask_type);
    super::render::apply_mask(pixmap, &mask);
}
