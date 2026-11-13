use super::{FontId, Fonts, GlyphId};
use crate::{Color, Pixels, Point, Rect, Size};
use anyhow::Result;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphRaster {
    pub font: FontId,
    pub glyph: GlyphId,
    pub size: Pixels,
    pub scale: f32,
    pub subpixel: u8,
    pub emoji: bool,
    pub dilation: u8,
}

impl Eq for GlyphRaster {}

impl Hash for GlyphRaster {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let bits = [self.size.0, self.scale].map(f32::to_bits);
        (
            self.font,
            self.glyph,
            bits,
            [self.subpixel, self.dilation],
            self.emoji,
        )
            .hash(state);
    }
}

impl Fonts {
    /// Offset from the pen in device pixels, size, and alpha bytes, or RGBA for emoji.
    pub fn glyph_pixels(
        &self,
        font: FontId,
        glyph: GlyphId,
        size: Pixels,
        scale: f32,
        subpixel: u8,
        emoji: bool,
    ) -> Option<(Point<i32>, Size<i32>, Vec<u8>)> {
        let raster = GlyphRaster {
            font,
            glyph,
            size,
            scale,
            subpixel,
            emoji,
            dilation: 0,
        };
        let (shown, size, bytes) = self.raster(&raster, i32::MAX).ok()??;
        Some((shown.origin, size, bytes))
    }

    /// A glyph larger than `max` rasters at a smaller scale, and `shown` is the place it stretches back to.
    pub(crate) fn raster(
        &self,
        raster: &GlyphRaster,
        max: i32,
    ) -> Result<Option<crate::atlas::Raster<Vec<u8>>>> {
        let full = self.platform.raster_bounds(raster)?;
        let stretch = full.size.width.max(full.size.height) as f32 / max as f32;
        let (raster, bounds) = if stretch > 1. {
            let small = GlyphRaster {
                scale: raster.scale / stretch,
                subpixel: 0,
                ..*raster
            };
            (small, self.platform.raster_bounds(&small)?)
        } else {
            (*raster, full)
        };
        if bounds.is_empty() {
            return Ok(None);
        }
        let (size, bytes) = self.platform.rasterize(&raster, bounds)?;
        let shown = if stretch > 1. {
            let grow = |value: i32| (value as f32 * stretch).round() as i32;
            Rect::new(bounds.origin.map(grow), size.map(grow))
        } else {
            Rect::new(bounds.origin, size)
        };
        Ok(Some((shown, size, bytes)))
    }

    pub(crate) fn dilation(&self, color: Color) -> u8 {
        self.platform.dilation(color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Font, TextSystem, px};

    #[cfg(target_os = "macos")]
    #[test]
    fn smoothed_dark_glyphs_cover_the_rows_of_chrome() {
        let text = TextSystem::new();
        let font = text.face(&Font::new("Helvetica"));
        let glyph = text.platform.glyph(font, 'H').unwrap();
        // The top edge of an 'H' at 32 px and 2x, in the darkest pixel of each row.
        let edge = |dilation| {
            let raster = GlyphRaster {
                font,
                glyph,
                size: px(32.),
                scale: 2.,
                subpixel: 0,
                emoji: false,
                dilation,
            };
            let (_, size, bytes) = text.raster(&raster, 512).unwrap().unwrap();
            bytes
                .chunks(size.width as usize)
                .map(|row| *row.iter().max().unwrap())
                .find(|&alpha| alpha > 0)
                .unwrap()
        };
        // Chromium has a partial row of 120 above the cap height, and a glyph without smoothing starts lower.
        assert!((i32::from(edge(1)) - 120).abs() < 16, "{}", edge(1));
        assert!(edge(0) > 200, "{}", edge(0));
    }

    #[test]
    fn huge_glyphs_raster_small_and_stretch_to_their_size() {
        let text = TextSystem::new();
        let font = text.face(&Font::new("Helvetica"));
        let glyph = text.platform.glyph(font, 'W').unwrap();
        let raster = GlyphRaster {
            font,
            glyph,
            size: px(10000.),
            scale: 2.,
            subpixel: 2,
            emoji: false,
            dilation: 0,
        };
        let full = text.platform.raster_bounds(&raster).unwrap();
        assert!(full.size.width > 16384);
        let max = crate::atlas::MAX_GLYPH_SIDE;
        let (shown, size, bytes) = text.raster(&raster, max).unwrap().unwrap();
        assert!(size.width.max(size.height) <= max + 4);
        assert_eq!(bytes.len(), (size.width * size.height) as usize);
        assert!(bytes.iter().any(|alpha| *alpha > 0));
        let stretch = full.size.width.max(full.size.height) / max + 1;
        for (shown, full) in [
            (shown.origin.x, full.origin.x),
            (shown.origin.y, full.origin.y),
            (shown.size.width, full.size.width),
            (shown.size.height, full.size.height),
        ] {
            assert!((shown - full).abs() <= 3 * stretch, "{shown:?} vs {full:?}");
        }
        let small = GlyphRaster {
            size: px(16.),
            ..raster
        };
        let (shown, size, _) = text.raster(&small, max).unwrap().unwrap();
        assert_eq!(shown.size, size);
        assert_eq!(
            shown.origin,
            text.platform.raster_bounds(&small).unwrap().origin
        );
    }
}
