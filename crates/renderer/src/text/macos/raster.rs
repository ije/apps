use super::*;

/// Skia's A8 mask gamma tables (`SkTMaskGamma<3, 3, 3>`) for Chromium on macOS: sRGB, no contrast. Each of the
/// 8 luminance levels maps linear coverage so that blending in sRGB values looks like blending in linear light.
pub(super) fn mask_gamma() -> &'static [[u8; 256]; 8] {
    static TABLES: OnceLock<[[u8; 256]; 8]> = OnceLock::new();
    TABLES.get_or_init(|| {
        let to_linear = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let from_linear = |v: f32| {
            if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1. / 2.4) - 0.055
            }
        };
        std::array::from_fn(|level| {
            let lum = (level as u32) << 5 | (level as u32) << 2 | (level as u32) >> 1;
            let src = lum as f32 / 255.;
            let dst = 1. - src;
            let (src_linear, dst_linear) = (to_linear(src), to_linear(dst));
            std::array::from_fn(|i| {
                let a = i as f32 / 255.;
                if (src - dst).abs() < 1. / 256. {
                    return i as u8;
                }
                let out = from_linear(src_linear * a + (1. - a) * dst_linear);
                (255. * (out - dst) / (src - dst)).round().clamp(0., 255.) as u8
            })
        })
    })
}

/// Skia's skew for a synthetic slant.
pub(super) const SKEW: f32 = 0.25;

/// The width of the line that Skia strokes around outlines for a synthetic bold, in CSS pixels: 1/24 of the size
/// up to 9px, 1/32 from 36px. Like Chromium, the size is in device pixels, so the line is thinner at 2x.
pub(super) fn embolden(size: f32, scale: f32) -> f32 {
    let device = size * scale;
    let t = ((device - 9.) / 27.).clamp(0., 1.);
    size * (1. / 24. + t * (1. / 32. - 1. / 24.))
}

impl TextSystem {
    pub fn raster_bounds(&self, glyph: &GlyphRaster) -> Result<Rect<i32>> {
        let book = self.0.borrow();
        let face = book.face(glyph.font);
        let [x0, y0, x1, y1] = face.drawn(glyph.glyph, glyph.size.0);
        // To y down, measured from the top.
        let (width, height) = (x1 - x0, y1 - y0);
        let (left, top) = (x0, -y0 - height);
        let s = glyph.scale;
        let min = point((left * s).floor() as i32, (top * s).floor() as i32);
        let max = point(
            ((left + width) * s).ceil() as i32,
            ((top + height) * s).ceil() as i32,
        );
        // A miter join reaches twice the stroke width past the outline at most. `drawn` holds half the synthetic
        // bold, and sharp corners can reach half more.
        let bold = if face.bold {
            embolden(glyph.size.0, s) / 2.
        } else {
            0.
        };
        let reach = ((glyph.stroke * 2. + bold) * s).ceil() as i32;
        Ok(Rect::new(min, size(max.x - min.x, max.y - min.y)).outset(1 + reach))
    }

    pub fn rasterize(
        &self,
        glyph: &GlyphRaster,
        bounds: Rect<i32>,
    ) -> Result<(Size<i32>, Vec<u8>)> {
        if bounds.size.width == 0 || bounds.size.height == 0 {
            bail!("glyph bounds are empty");
        }
        let mut canvas = bounds.size;
        if glyph.subpixel > 0 {
            canvas.width += 1;
        }
        let (width, height) = (canvas.width as usize, canvas.height as usize);
        // Glyphs draw black on opaque white and the mask inverts after: an alpha-only canvas makes them about 17%
        // heavier. Like Chromium, font smoothing dilates glyphs of every color the same.
        let opaque = !glyph.emoji;
        // Premultiplied RGBA for emoji, BGRX for masks.
        let info = if opaque { 6 | 2 << 12 } else { 1 };
        let mut bytes = vec![0u8; width * height * 4];
        let space = CGColorSpace::new_device_rgb();
        // SAFETY: the buffer matches the size, row length and pixel format, and outlives the context.
        let context = unsafe {
            CGBitmapContextCreate(
                bytes.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                space.as_deref(),
                info,
            )
        }
        .context("Could not make a glyph canvas")?;
        let cx = Some(&*context);
        let (bold, oblique) = {
            let book = self.0.borrow();
            let face = book.face(glyph.font);
            (face.bold, face.oblique)
        };
        // Like Skia, which draws emboldened and stroked glyphs from their outlines, without the font smoothing of
        // CoreText.
        let smooth = glyph.dilation > 0 && !bold && glyph.stroke == 0.;
        if opaque {
            CGContext::set_gray_fill_color(cx, 1., 1.);
            CGContext::fill_rect(
                cx,
                CGRect::new(CGPoint::ZERO, CGSize::new(width as _, height as _)),
            );
            CGContext::set_allows_font_smoothing(cx, smooth);
            CGContext::set_should_smooth_fonts(cx, smooth);
        }
        CGContext::translate_ctm(
            cx,
            -bounds.origin.x as CGFloat,
            (bounds.origin.y + bounds.size.height) as CGFloat,
        );
        // CoreText enlarges emoji below 24 points, so small emoji draw at twice the size or more.
        let oversize = if glyph.emoji { glyph.scale.max(2.) } else { 1. };
        let scale = (glyph.scale / oversize) as CGFloat;
        CGContext::scale_ctm(cx, scale, scale);
        if oblique {
            let skew = CGAffineTransform {
                a: 1.,
                b: 0.,
                c: SKEW as CGFloat,
                d: 1.,
                tx: 0.,
                ty: 0.,
            };
            CGContext::concat_ctm(cx, skew);
        }
        let bold = if bold {
            embolden(glyph.size.0, glyph.scale)
        } else {
            0.
        };
        // Like Skia, whose stroked text has miter joins up to 4 times the width.
        CGContext::set_line_width(cx, (glyph.stroke + bold) as CGFloat);
        CGContext::set_miter_limit(cx, 4.);
        CGContext::set_gray_stroke_color(cx, 0., 1.);
        if glyph.stroke > 0. {
            CGContext::set_text_drawing_mode(cx, CGTextDrawingMode::Stroke);
        } else {
            CGContext::set_text_drawing_mode(cx, CGTextDrawingMode::Fill);
        }
        CGContext::set_allows_antialiasing(cx, true);
        CGContext::set_should_antialias(cx, true);
        CGContext::set_allows_font_subpixel_positioning(cx, true);
        CGContext::set_should_subpixel_position_fonts(cx, true);
        CGContext::set_allows_font_subpixel_quantization(cx, false);
        CGContext::set_should_subpixel_quantize_fonts(cx, false);
        CGContext::set_gray_fill_color(cx, 0., 1.);
        let shift = f32::from(glyph.subpixel) / f32::from(SUBPIXEL_STEPS);
        let font = self
            .0
            .borrow_mut()
            .sized(glyph.font, glyph.size.0 * oversize);
        let (mut id, mut at) = (
            glyph.glyph,
            CGPoint::new((shift / glyph.scale) as CGFloat, 0.),
        );
        // Like Skia, a synthetic bold fills the outline grown by half the width: a fill and a stroke of the path.
        let path = if glyph.stroke == 0. && bold > 0. {
            let moved = CGAffineTransform {
                a: 1.,
                b: 0.,
                c: 0.,
                d: 1.,
                tx: at.x,
                ty: 0.,
            };
            // SAFETY: a path of one glyph of a live font.
            unsafe { font.path_for_glyph(id, &moved) }
        } else {
            None
        };
        if let Some(path) = &path {
            CGContext::add_path(cx, Some(path));
            CGContext::draw_path(cx, CGPathDrawingMode::FillStroke);
        } else {
            // SAFETY: one glyph at one position, drawn into a live context.
            unsafe {
                font.draw_glyphs(NonNull::from(&mut id), NonNull::from(&mut at), 1, &context)
            };
        }
        if opaque {
            // Like Skia: smoothed CoreGraphics values are squared back to linear coverage, and all masks go
            // through the mask gamma of their text luminance.
            let gamma =
                (glyph.dilation > 0).then(|| &mask_gamma()[usize::from(glyph.dilation - 1)]);
            bytes = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| {
                    let value = u32::from(pixel[1]);
                    let value = if smooth {
                        (value * value + 128) / 255
                    } else {
                        value
                    };
                    255 - value as u8
                })
                .map(|coverage| gamma.map_or(coverage, |table| table[usize::from(coverage)]))
                .collect();
        } else if glyph.emoji {
            for pixel in bytes.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
                let alpha = f32::from(pixel[3]) / 255.;
                if alpha > 0. {
                    for channel in &mut pixel[..3] {
                        *channel = (f32::from(*channel) / alpha).min(255.) as u8;
                    }
                }
            }
        }
        Ok((canvas, bytes))
    }
    /// 0 without font smoothing, else 1 + the luminance level of `color` for `mask_gamma`.
    pub fn dilation(&self, color: Color) -> u8 {
        static SMOOTHING: OnceLock<bool> = OnceLock::new();
        let smoothing = *SMOOTHING.get_or_init(|| {
            use objc2_core_foundation::{
                CFPreferencesCopyAppValue, kCFPreferencesCurrentApplication,
            };
            let key = CFString::from_static_str("AppleFontSmoothing");
            // SAFETY: the application id is a CoreFoundation constant.
            let value =
                CFPreferencesCopyAppValue(&key, unsafe { kCFPreferencesCurrentApplication });
            value
                .and_then(|value| value.downcast::<CFNumber>().ok())
                .is_none_or(|value| value.as_i64() != Some(0))
        });
        if !smoothing {
            return 0;
        }
        // Like Skia's mask gamma: 3 bits per channel, halved for CoreGraphics smoothing and cut to 3 bits again,
        // then 3 bits of luminance.
        let canonical = |bits: u32| bits << 5 | bits << 2 | bits >> 1;
        let [r, g, b] = [color.r, color.g, color.b].map(|value| {
            let bits = (value.clamp(0., 1.) * 255.).round() as u32 >> 5;
            canonical((canonical(bits) / 2) >> 5)
        });
        let level = (r * 54 + g * 183 + b * 19) >> 8 >> 5;
        1 + level as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_take_skias_mask_gamma_of_their_color() {
        let tables = mask_gamma();
        // Dark text: edge coverage lightens, as sRGB blending of linear light does. Light text stays near linear.
        assert_eq!((tables[0][0], tables[0][255]), (0, 255));
        assert!((66..=70).contains(&tables[0][128]), "{}", tables[0][128]);
        assert!((110..=128).contains(&tables[3][128]), "{}", tables[3][128]);
        // Like Chromium, the synthetic bold line is thinner at 2x: its fraction of the size follows device pixels.
        assert!(embolden(16., 2.) < embolden(16., 1.));
        assert_eq!(embolden(48., 2.), 48. / 32.);
        let text = TextSystem::default();
        let font = |weight| {
            text.font(&Font {
                weight,
                ..Font::new("Helvetica")
            })
            .unwrap()
        };
        let regular = font(400.);
        if text.dilation(Color::WHITE) == 0 {
            return;
        }
        // Halved for CoreGraphics smoothing, white is luminance level 3.
        assert_eq!(text.dilation(Color::BLACK), 1);
        assert_eq!(text.dilation(Color::WHITE), 4);
        let glyph = text.glyph(regular, 'e').unwrap();
        let ink = |dilation, stroke| {
            let raster = GlyphRaster {
                font: regular,
                glyph,
                size: px(13.),
                scale: 2.,
                subpixel: 0,
                emoji: false,
                dilation,
                stroke,
            };
            let (_, pixels) = text
                .rasterize(&raster, text.raster_bounds(&raster).unwrap())
                .unwrap();
            pixels.iter().map(|&value| u32::from(value)).sum::<u32>()
        };
        // Like Chrome, light text on a dark background draws heavier than dark text, and so do stroked glyphs.
        assert!(ink(4, 0.) > ink(1, 0.) * 21 / 20);
        assert!(ink(1, 1.) < ink(0, 1.));
    }
}
