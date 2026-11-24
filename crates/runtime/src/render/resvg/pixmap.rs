// Copyright 2018 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Match tiny-skia's highp f32 SourceOver arithmetic without FMA. Skip transparent pixels and copy opaque
// ones. Negative offsets must not repeat edge pixels.
pub fn draw_over(
    dst: &mut tiny_skia::PixmapMut,
    src: tiny_skia::PixmapRef,
    x: i32,
    y: i32,
    opacity: f32,
) {
    let (dw, sw) = (dst.width() as i32, src.width() as i32);
    let [left, top] = [x.max(0), y.max(0)];
    let right = (x + sw).min(dw);
    let bottom = (y + src.height() as i32).min(dst.height() as i32);
    if left >= right || top >= bottom {
        return;
    }
    let len = (right - left) as usize * 4;
    let k = 1.0 / 255.0;
    let (src, dst) = (src.data(), dst.data_mut());
    for row in top..bottom {
        let from = &src[((row - y) * sw + left - x) as usize * 4..][..len];
        let to = &mut dst[(row * dw + left) as usize * 4..][..len];
        for (d, s) in to
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(from.as_chunks::<4>().0)
        {
            if *s == [0; 4] {
                continue;
            }
            if s[3] == 255 && opacity == 1.0 {
                *d = *s;
                continue;
            }
            let inv = 1.0 - s[3] as f32 * k * opacity;
            for (d, s) in d.iter_mut().zip(s) {
                let v = *d as f32 * k * inv + *s as f32 * k * opacity;
                *d = (v.clamp(0.0, 1.0) * 255.0).round_ties_even() as u8;
            }
        }
    }
}

// Match draw_pixmap onto transparent pixels, except for the negative-offset correction in draw_over.
pub fn copy_pixmap(dst: &mut tiny_skia::Pixmap, src: &tiny_skia::Pixmap, x: i32, y: i32) {
    let (dw, sw) = (dst.width() as i32, src.width() as i32);
    let [left, top] = [x.max(0), y.max(0)];
    let right = (x + sw).min(dw);
    let bottom = (y + src.height() as i32).min(dst.height() as i32);
    if left >= right || top >= bottom {
        return;
    }
    let len = (right - left) as usize * 4;
    let (src, dst) = (src.data(), dst.data_mut());
    for row in top..bottom {
        dst[(row * dw + left) as usize * 4..][..len]
            .copy_from_slice(&src[((row - y) * sw + left - x) as usize * 4..][..len]);
    }
}

// Match Pixmap::apply_mask: (d * m + 255) >> 8. Mask 255 preserves pixels. Mask 0 clears them.
pub fn apply_mask(pixmap: &mut tiny_skia::Pixmap, mask: &tiny_skia::Mask) {
    for (pixel, &m) in pixmap
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(mask.data())
    {
        match m {
            255 => {}
            0 => *pixel = [0; 4],
            m => {
                for c in pixel {
                    *c = ((*c as u16 * m as u16 + 255) >> 8) as u8;
                }
            }
        }
    }
}

pub fn convert_blend_mode(mode: usvg::BlendMode) -> tiny_skia::BlendMode {
    match mode {
        usvg::BlendMode::Normal => tiny_skia::BlendMode::SourceOver,
        usvg::BlendMode::Multiply => tiny_skia::BlendMode::Multiply,
        usvg::BlendMode::Screen => tiny_skia::BlendMode::Screen,
        usvg::BlendMode::Overlay => tiny_skia::BlendMode::Overlay,
        usvg::BlendMode::Darken => tiny_skia::BlendMode::Darken,
        usvg::BlendMode::Lighten => tiny_skia::BlendMode::Lighten,
        usvg::BlendMode::ColorDodge => tiny_skia::BlendMode::ColorDodge,
        usvg::BlendMode::ColorBurn => tiny_skia::BlendMode::ColorBurn,
        usvg::BlendMode::HardLight => tiny_skia::BlendMode::HardLight,
        usvg::BlendMode::SoftLight => tiny_skia::BlendMode::SoftLight,
        usvg::BlendMode::Difference => tiny_skia::BlendMode::Difference,
        usvg::BlendMode::Exclusion => tiny_skia::BlendMode::Exclusion,
        usvg::BlendMode::Hue => tiny_skia::BlendMode::Hue,
        usvg::BlendMode::Saturation => tiny_skia::BlendMode::Saturation,
        usvg::BlendMode::Color => tiny_skia::BlendMode::Color,
        usvg::BlendMode::Luminosity => tiny_skia::BlendMode::Luminosity,
        usvg::BlendMode::PlusLighter => tiny_skia::BlendMode::Plus,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pixel_helpers_match_tiny_skia() {
        let mut seed = 0x9e37_79b9u32;
        let mut next = |n: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed % n
        };
        for _ in 0..2000 {
            let mut images = [0, 0].map(|_| {
                let mut pixmap = tiny_skia::Pixmap::new(next(9) + 1, next(9) + 1).unwrap();
                for pixel in pixmap.data_mut().as_chunks_mut::<4>().0 {
                    let alpha = [0, 255, next(256)][next(3) as usize];
                    *pixel = [0, 0, 0, 0].map(|_| (next(256) % (alpha + 1)) as u8);
                    pixel[3] = alpha as u8;
                }
                pixmap
            });
            let [x, y] = [next(6) as i32, next(6) as i32];
            let opacity = [1.0, 0.5, 0.3, 0.77][next(4) as usize];
            let [src, dst] = &mut images;
            let mut expected = dst.clone();
            let paint = tiny_skia::PixmapPaint {
                opacity,
                ..Default::default()
            };
            let identity = tiny_skia::Transform::identity();
            expected.draw_pixmap(x, y, src.as_ref(), &paint, identity, None);
            super::draw_over(&mut dst.as_mut(), src.as_ref(), x, y, opacity);
            assert_eq!(dst.data(), expected.data());

            let mut mask = tiny_skia::Mask::new(dst.width(), dst.height()).unwrap();
            for value in mask.data_mut() {
                *value = [0, 255, next(256) as u8][next(3) as usize];
            }
            let mut expected = dst.clone();
            expected.apply_mask(&mask);
            super::apply_mask(dst, &mask);
            assert_eq!(dst.data(), expected.data());
        }
    }
}
