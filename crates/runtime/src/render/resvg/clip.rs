// Copyright 2019 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::render::Context;

pub fn apply(
    clip: &usvg::ClipPath,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::Pixmap,
) {
    // WARNING: a clip of one path is that path's coverage, which is what clearing it from an opaque pixmap and
    // inverting the alpha gave, so it fills a mask directly. Clips of more paths keep the pixmap, because its
    // rounding where they overlap differs from a mask's.
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
            apply(clip, transform, pixmap);
        }
        super::render::apply_mask(pixmap, &mask);
        return;
    }

    let mut clip_pixmap = tiny_skia::Pixmap::new(pixmap.width(), pixmap.height()).unwrap();
    clip_pixmap.fill(tiny_skia::Color::BLACK);

    draw_children(
        clip.root(),
        tiny_skia::BlendMode::Clear,
        transform.pre_concat(clip.transform()),
        &mut clip_pixmap.as_mut(),
    );

    if let Some(clip) = clip.clip_path() {
        apply(clip, transform, pixmap);
    }

    let mut mask = tiny_skia::Mask::from_pixmap(clip_pixmap.as_ref(), tiny_skia::MaskType::Alpha);
    mask.invert();
    super::render::apply_mask(pixmap, &mask);
}

fn draw_children(
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

                let ctx = Context {
                    max_bbox: tiny_skia::IntRect::from_xywh(0, 0, 1, 1).unwrap(),
                };

                super::path::fill_path(path, mode, &ctx, transform, pixmap);
            }
            usvg::Node::Text(text) => {
                draw_children(text.flattened(), mode, transform, pixmap);
            }
            usvg::Node::Group(group) => {
                let transform = transform.pre_concat(group.transform());

                if let Some(clip) = group.clip_path() {
                    clip_group(group, clip, transform, pixmap);
                } else {
                    draw_children(group, mode, transform, pixmap);
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
) -> Option<()> {
    let mut clip_pixmap = tiny_skia::Pixmap::new(pixmap.width(), pixmap.height()).unwrap();

    draw_children(
        children,
        tiny_skia::BlendMode::SourceOver,
        transform,
        &mut clip_pixmap.as_mut(),
    );
    apply(clip, transform, &mut clip_pixmap);

    let mut paint = tiny_skia::PixmapPaint::default();
    paint.blend_mode = tiny_skia::BlendMode::Xor;
    pixmap.draw_pixmap(
        0,
        0,
        clip_pixmap.as_ref(),
        &paint,
        tiny_skia::Transform::identity(),
        None,
    );

    Some(())
}
