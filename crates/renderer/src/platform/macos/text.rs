use super::open_type;
use crate::{
    Color, Pixels, Rect, Size, point, px, size,
    text::{
        FaceMetrics, Features, Font, FontId, FontStyle, Glyph, GlyphId, GlyphRaster, Line,
        SUBPIXEL_STEPS, SharedString, Span, best,
    },
};
use anyhow::{Context as _, Result, bail};
use objc2_core_foundation::{
    CFArray, CFData, CFDictionary, CFIndex, CFMutableAttributedString, CFNumber, CFRange,
    CFRetained, CFString, CFType, CGAffineTransform, CGFloat, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGPathDrawingMode, CGTextDrawingMode,
};
use objc2_core_text::{
    CTFont, CTFontCollection, CTFontDescriptor, CTFontManagerCopyAvailableFontFamilyNames,
    CTFontManagerCreateFontDescriptorsFromData, CTFontOrientation, CTFontSymbolicTraits, CTLine,
    CTRun, kCTFontAttributeName, kCTFontCascadeListAttribute, kCTFontFamilyNameAttribute,
    kCTFontSlantTrait, kCTFontSymbolicTrait, kCTFontWeightTrait, kCTFontWidthTrait,
};
use rustc_hash::FxHashMap;
use std::{
    cell::{OnceCell, RefCell},
    hash::Hash,
    mem,
    ops::Range,
    ptr::{self, NonNull},
    rc::Rc,
    sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError},
};

#[derive(Clone)]
struct Shared(CFRetained<CTFont>);

// SAFETY: CoreText fonts are immutable, and Apple documents them as safe to use from any thread at once.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

/// Each value is made outside the map lock, so a slow load does not block other lookups.
struct Process<K, V>(LazyLock<Mutex<FxHashMap<K, Arc<OnceLock<V>>>>>);

impl<K: Hash + Eq, V: Clone> Process<K, V> {
    const fn new() -> Self {
        Self(LazyLock::new(Default::default))
    }

    fn get(&self, key: K, make: impl FnOnce() -> V) -> V {
        let slot = (self.0.lock().unwrap_or_else(PoisonError::into_inner))
            .entry(key)
            .or_default()
            .clone();
        slot.get_or_init(make).clone()
    }
}

type Faces = Option<Vec<(Shared, String)>>;

static SYSTEM_FACES: Process<(String, Vec<(String, u32)>), Faces> = Process::new();
/// Keyed by font pointer. The faces stay alive in `SYSTEM_FACES`.
static AXES: Process<usize, Option<f32>> = Process::new();
/// Axis tags and value bits.
type Coords = Vec<(u32, u32)>;
/// Tags with minimum, default and maximum.
type Axes = Rc<[(u32, [f32; 3])]>;

static INSTANCES: Process<(usize, Coords), (Shared, String)> = Process::new();

static FAMILIES: LazyLock<Vec<String>> = LazyLock::new(|| {
    // SAFETY: CoreText returns an array of strings.
    let names = unsafe { CTFontManagerCopyAvailableFontFamilyNames() };
    let names = unsafe { names.cast_unchecked::<CFString>() };
    // macOS leaves these out of the list, though it has them and browsers find them.
    names
        .iter()
        .map(|name| name.to_string())
        .chain(["Times".into(), "Courier".into()])
        .collect()
});

pub(crate) struct TextSystem(RefCell<Book>);

struct Face {
    font: CFRetained<CTFont>,
    local: bool,
    emoji: bool,
    mono: bool,
    /// Like Chrome, a face without the asked bold or slant draws its glyphs emboldened or skewed.
    bold: bool,
    oblique: bool,
    units: OnceCell<Units>,
    matching: OnceCell<(f32, FontStyle, f32)>,
    axes: OnceCell<Axes>,
}

/// In font units, computed as font-kit did, so layout keeps its numbers.
#[derive(Clone, Copy)]
struct Units {
    per_em: u32,
    /// Font units per point at the face's size.
    per_point: f32,
    ascent: f32,
    descent: f32,
    gap: f32,
    cap: f32,
    x: f32,
    /// `[min x, min y, max x, max y]`.
    bounds: [f32; 4],
}

impl Face {
    fn new(font: CFRetained<CTFont>, name: &str, local: bool) -> Self {
        // SAFETY: plain CoreText queries.
        let traits = unsafe { font.symbolic_traits() };
        Self {
            emoji: name == "AppleColorEmoji" || name == ".AppleColorEmojiUI",
            mono: traits.contains(CTFontSymbolicTraits::TraitMonoSpace),
            bold: false,
            oblique: false,
            font,
            local,
            units: OnceCell::new(),
            matching: OnceCell::new(),
            axes: OnceCell::new(),
        }
    }

    fn axes(&self) -> Axes {
        self.axes
            .get_or_init(|| open_type::axes(&self.font).into())
            .clone()
    }

    fn units(&self) -> Units {
        *self.units.get_or_init(|| {
            let font = &self.font;
            // SAFETY: plain CoreText queries.
            unsafe {
                let per_em = font.units_per_em();
                let per_point = f64::from(per_em) / font.size();
                let scaled = |value: CGFloat| (value * per_point) as f32;
                let rect = font.bounding_box();
                let (x, y) = (rect.origin.x as f32, rect.origin.y as f32);
                let bounds = [
                    x,
                    y,
                    x + rect.size.width as f32,
                    y + rect.size.height as f32,
                ]
                .map(|v| v * per_point as f32);
                Units {
                    per_em,
                    per_point: per_point as f32,
                    ascent: scaled(font.ascent()),
                    descent: scaled(-font.descent()),
                    gap: scaled(font.leading()),
                    cap: scaled(font.cap_height()),
                    x: scaled(font.x_height()),
                    bounds,
                }
            }
        })
    }

    /// `[min x, min y, max x, max y]` in font units.
    fn outline(&self, glyph: GlyphId) -> [f32; 4] {
        let mut id = glyph;
        // SAFETY: one glyph in, the overall rect out.
        let rect = unsafe {
            self.font.bounding_rects_for_glyphs(
                CTFontOrientation::Default,
                NonNull::from(&mut id),
                ptr::null_mut(),
                1,
            )
        };
        let (x, y) = (rect.origin.x as f32, rect.origin.y as f32);
        let per_point = self.units().per_point;
        [
            x,
            y,
            x + rect.size.width as f32,
            y + rect.size.height as f32,
        ]
        .map(|value| value * per_point)
    }

    /// `[min x, min y, max x, max y]` of the drawn glyph at `size`, with the synthetic bold and slant.
    fn drawn(&self, glyph: GlyphId, size: f32) -> [f32; 4] {
        let k = size / self.units().per_em as f32;
        let [x0, y0, x1, y1] = self.outline(glyph).map(|value| value * k);
        let (x0, x1) = if self.oblique {
            (x0 + SKEW * y0.min(0.), x1 + SKEW * y1.max(0.))
        } else {
            (x0, x1)
        };
        let out = if self.bold {
            embolden(size, 1.) / 2.
        } else {
            0.
        };
        [x0 - out, y0 - out, x1 + out, y1 + out]
    }

    fn matching(&self) -> (f32, FontStyle, f32) {
        *self.matching.get_or_init(|| {
            // SAFETY: plain CoreText queries.
            let (traits, symbolic) = unsafe { (self.font.traits(), self.font.symbolic_traits()) };
            let traits = unsafe { traits.cast_unchecked::<CFString, CFType>() };
            let number = |key: &CFString| traits.get(key)?.downcast::<CFNumber>().ok();
            // SAFETY: the keys are CoreText constants.
            let [weight, width, slant] =
                unsafe { [kCTFontWeightTrait, kCTFontWidthTrait, kCTFontSlantTrait] }
                    .map(|key| number(key).and_then(|value| value.as_f64()).unwrap_or(0.));
            let style = if symbolic.contains(CTFontSymbolicTraits::TraitItalic) {
                FontStyle::Italic
            } else if slant > 0. {
                FontStyle::Oblique
            } else {
                FontStyle::Normal
            };
            let class = self
                .local
                .then(|| open_type::weight_class(&self.font))
                .flatten();
            // SAFETY: the key is a CoreText constant.
            let trait_weight =
                number(unsafe { kCTFontWeightTrait }).and_then(|value| value.as_f32());
            let weight = class
                .or_else(|| trait_weight.map(css_weight))
                .unwrap_or_else(|| find_index(weight as f32, &KIT_WEIGHTS) * 100. + 100.);
            (weight, style, css_stretch(width as f32))
        })
    }
}

/// Skia's A8 mask gamma tables (`SkTMaskGamma<3, 3, 3>`) for Chromium on macOS: sRGB, no contrast. Each of the
/// 8 luminance levels maps linear coverage so that blending in sRGB values looks like blending in linear light.
fn mask_gamma() -> &'static [[u8; 256]; 8] {
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
const SKEW: f32 = 0.25;

/// The width of the line that Skia strokes around outlines for a synthetic bold, in CSS pixels: 1/24 of the size
/// up to 9px, 1/32 from 36px. Like Chromium, the size is in device pixels, so the line is thinner at 2x.
fn embolden(size: f32, scale: f32) -> f32 {
    let device = size * scale;
    let t = ((device - 9.) / 27.).clamp(0., 1.);
    size * (1. / 24. + t * (1. / 32. - 1. / 24.))
}

/// CoreText weights of the CSS weights 100 to 900 for Apple's faces. A plain linear scale maps Medium to 530,
/// so CSS 500 would pick Regular.
const APPLE_WEIGHTS: [f32; 9] = [-0.8, -0.6, -0.4, 0., 0.23, 0.3, 0.4, 0.56, 0.62];
/// The scale font-kit used when a face has no numeric weight trait.
const KIT_WEIGHTS: [f32; 9] = [-0.7, -0.5, -0.23, 0., 0.2, 0.3, 0.4, 0.6, 0.8];
/// CSS stretch at CoreText widths -1 to 1 in quarters.
const STRETCHES: [f32; 9] = [0.5, 0.625, 0.75, 0.875, 1., 1.125, 1.25, 1.5, 2.];

fn css_weight(weight: f32) -> f32 {
    let below = APPLE_WEIGHTS
        .iter()
        .rposition(|&knot| weight >= knot)
        .unwrap_or(0);
    let fraction = match APPLE_WEIGHTS.get(below + 1) {
        Some(next) if weight >= APPLE_WEIGHTS[0] => {
            (weight - APPLE_WEIGHTS[below]) / (next - APPLE_WEIGHTS[below])
        }
        _ => 0.,
    };
    100. * (below as f32 + 1. + fraction)
}

fn css_stretch(width: f32) -> f32 {
    let at = (width + 1.) * 4.;
    let (low, high) = (
        STRETCHES[at.floor() as usize],
        STRETCHES[at.ceil() as usize],
    );
    low + (high - low) * at.fract()
}

fn find_index(value: f32, knots: &[f32]) -> f32 {
    let above = match knots
        .binary_search_by(|knot| knot.partial_cmp(&value).unwrap_or(std::cmp::Ordering::Less))
    {
        Ok(at) => return at as f32,
        Err(above) => above,
    };
    if above == 0 || above >= knots.len() {
        return above as f32;
    }
    let (low, high) = (knots[above - 1], knots[above]);
    (above - 1) as f32 + (value - low) / (high - low)
}

struct Added {
    family: String,
    descriptor: CFRetained<CTFontDescriptor>,
}

#[derive(Default)]
struct Book {
    faces: Vec<Face>,
    /// PostScript names, plus axis values for instances.
    names: FxHashMap<String, FontId>,
    families: FxHashMap<(SharedString, Features), Rc<[FontId]>>,
    /// Sorted by family, stable.
    added: Vec<Added>,
    // Each sized copy rebuilds CoreText caches at a cost, so the copies stay.
    sized: FxHashMap<(FontId, u32), CFRetained<CTFont>>,
    /// Base face and axis values, to the instance face.
    instances: FxHashMap<(FontId, Coords), FontId>,
    // Keyed by pointer; the kept font keeps the address from being reused.
    natives: FxHashMap<usize, (CFRetained<CTFont>, FontId)>,
    words: Words,
    scratch: Scratch,
}

#[derive(Default)]
struct Scratch {
    ids: Vec<u16>,
    points: Vec<CGPoint>,
    indices: Vec<CFIndex>,
}

impl Default for TextSystem {
    fn default() -> Self {
        // Listing the font families takes about 20 ms, so it starts here, off the first frame's path.
        static LISTING: std::sync::Once = std::sync::Once::new();
        LISTING.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("font families".into())
                .spawn(|| LazyLock::force(&FAMILIES).len());
        });
        Self(RefCell::default())
    }
}

impl TextSystem {
    /// Adds a face to `family`, as CSS groups faces by the family of their `@font-face`. Returns the family.
    pub fn add_font(&self, bytes: Vec<u8>, family: &str) -> Result<String> {
        // SAFETY: CoreText parses the data and returns an array of descriptors.
        let descriptor = unsafe {
            let all = CTFontManagerCreateFontDescriptorsFromData(&CFData::from_bytes(&bytes));
            all.cast_unchecked::<CTFontDescriptor>().get(0)
        }
        .context("Unsupported or invalid font data")?;
        let book = &mut *self.0.borrow_mut();
        book.added.push(Added {
            family: family.to_owned(),
            descriptor,
        });
        book.added.sort_by(|a, b| a.family.cmp(&b.family));
        book.families.clear();
        Ok(family.to_owned())
    }

    pub fn families(&self) -> Vec<String> {
        let mut names = FAMILIES.clone();
        names.extend(
            self.0
                .borrow()
                .added
                .iter()
                .map(|added| added.family.clone()),
        );
        names
    }

    pub fn font(&self, font: &Font) -> Option<FontId> {
        self.0.borrow_mut().font(font)
    }

    pub fn metrics(&self, font: FontId) -> FaceMetrics {
        let book = self.0.borrow();
        let face = book.face(font);
        let units = face.units();
        let per_em = units.per_em as f32;
        let [x0, y0, x1, y1] = units.bounds;
        let typo = open_type::typo_metrics(&face.font);
        // SAFETY: a plain CoreText query.
        let family = unsafe { face.font.family_name() }.to_string();
        FaceMetrics {
            ascent: units.ascent / per_em,
            descent: units.descent / per_em,
            line_gap: units.gap / per_em,
            cap_height: units.cap / per_em,
            x_height: units.x / per_em,
            bounds: Rect::new(point(x0, y0), size(x1 - x0, y1 - y0)) / per_em,
            em_ascent: match typo {
                Some([ascent, descent]) => ascent / (ascent - descent),
                None => units.ascent / (units.ascent - units.descent),
            },
            tall: matches!(family.as_str(), "Times" | "Helvetica" | "Courier"),
        }
    }

    pub fn glyph(&self, font: FontId, ch: char) -> Option<GlyphId> {
        let (mut units, mut glyphs) = ([0u16; 2], [0u16; 2]);
        ch.encode_utf16(&mut units);
        // SAFETY: both buffers hold two values.
        unsafe {
            self.0.borrow().face(font).font.glyphs_for_characters(
                NonNull::from(&mut units).cast(),
                NonNull::from(&mut glyphs).cast(),
                2,
            )
        };
        Some(glyphs[0]).filter(|&glyph| glyph != 0)
    }

    pub fn advance(&self, font: FontId, glyph: GlyphId) -> f32 {
        let book = self.0.borrow();
        let face = book.face(font);
        let (mut id, mut advance) = (glyph, CGSize::default());
        // SAFETY: one glyph in, one advance out.
        unsafe {
            face.font.advances_for_glyphs(
                CTFontOrientation::Default,
                NonNull::from(&mut id),
                &mut advance,
                1,
            )
        };
        let units = face.units();
        advance.width as f32 * units.per_point / units.per_em as f32
    }

    pub fn ink(&self, font: FontId, glyph: GlyphId, size: f32) -> Option<[f32; 4]> {
        let book = self.0.borrow();
        let [x0, y0, x1, y1] = book.face(font).drawn(glyph, size);
        Some([x0, x1, y1, y0])
    }

    pub fn shape(&self, text: &str, font_size: Pixels, spans: &[Span]) -> Line {
        self.0.borrow_mut().shape(text, font_size, spans)
    }

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

    pub fn warm(lists: Vec<Vec<String>>) {
        let load = move || {
            let mut names = FAMILIES.clone();
            names.sort_unstable();
            names.dedup();
            let names: FxHashMap<String, String> = names
                .into_iter()
                .map(|name| (name.to_lowercase(), name))
                .collect();
            let text = Self::default();
            for list in lists {
                if let Some(name) = list
                    .iter()
                    .find_map(|family| names.get(&family.to_lowercase()))
                {
                    text.font(&Font::new(name.as_str()));
                }
            }
        };
        let _ = std::thread::Builder::new()
            .name("apps-fonts".into())
            .spawn(load);
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

impl Book {
    fn face(&self, font: FontId) -> &Face {
        &self.faces[font as usize]
    }

    fn register(&mut self, font: CFRetained<CTFont>, name: String, local: bool) -> FontId {
        let id = self.faces.len() as FontId;
        self.faces.push(Face::new(font, &name, local));
        self.names.insert(name, id);
        id
    }

    fn font(&mut self, font: &Font) -> Option<FontId> {
        if let Some((first, rest)) = font.family.split_once('\n') {
            let id = self.font(&Font {
                family: first.into(),
                ..font.clone()
            })?;
            return Some(self.cascade(id, rest, font));
        }
        let id = self.matched(font)?;
        let face = self.face(id);
        let weight = face.matching().0;
        let bold = font.synthesis[0]
            && font.weight >= 600.
            && weight < 600.
            && !face.axes().iter().any(|&(tag, _)| tag == open_type::WGHT);
        // Like Chrome, faces that only lean, without the italic trait, slant more.
        // SAFETY: plain CoreText query.
        let italic =
            unsafe { face.font.symbolic_traits() }.contains(CTFontSymbolicTraits::TraitItalic);
        let oblique = font.synthesis[1] && font.style != FontStyle::Normal && !italic;
        if face.emoji || !bold && !oblique {
            return Some(id);
        }
        let name = format!("{id}#synthetic{}{}", u8::from(bold), u8::from(oblique));
        if let Some(&known) = self.names.get(&name) {
            return Some(known);
        }
        let (copy, local) = (face.font.clone(), face.local);
        let synthetic = self.register(copy, name, local);
        let face = &mut self.faces[synthetic as usize];
        (face.bold, face.oblique) = (bold, oblique);
        Some(synthetic)
    }

    fn matched(&mut self, font: &Font) -> Option<FontId> {
        let key = (font.family.clone(), font.features.clone());
        let candidates = match self.families.get(&key) {
            Some(found) => found.clone(),
            None => {
                let found: Rc<[FontId]> = self.load(&font.family, &font.features)?.into();
                self.families.insert(key, found.clone());
                found
            }
        };
        let matching: Vec<_> = candidates
            .iter()
            .map(|&id| self.face(id).matching())
            .collect();
        let id = candidates[best(&matching, font.weight, font.style)?];
        let face = self.face(id);
        let axes = face.axes();
        if axes.is_empty() {
            return Some(id);
        }
        let default = if face.local {
            open_type::default_weight(&face.font)
        } else {
            let pointer = CFRetained::as_ptr(&face.font).as_ptr() as usize;
            AXES.get(pointer, || open_type::default_weight(&face.font))
        };
        let current = open_type::variation(&face.font);
        let mut wanted = Vec::new();
        let mut same = true;
        for &(tag, [min, initial, max]) in axes.iter() {
            let set = font
                .variations
                .iter()
                .find(|(name, _)| u32::from_be_bytes(*name) == tag)
                .map(|&(_, value)| value);
            let value = set.or(match tag {
                open_type::WGHT if default.is_some() => Some(font.weight),
                // Like Chromium, web fonts follow the font size. CoreText sizes system fonts itself.
                open_type::OPSZ if face.local => font.optical_size,
                _ => None,
            });
            if let Some(value) = value {
                // Each value makes a font, so a transition steps in values that look the same.
                let value = match tag {
                    open_type::WGHT | open_type::WDTH => value.round(),
                    _ if max > min => {
                        min + ((value - min) * 256. / (max - min)).round() * (max - min) / 256.
                    }
                    _ => value,
                }
                .clamp(min, max);
                let now = current
                    .iter()
                    .find(|(name, _)| *name == tag)
                    .map_or(initial, |&(_, value)| value);
                same &= value == now;
                wanted.push((tag, value));
            }
        }
        if wanted.is_empty() || same && !face.local {
            return Some(id);
        }
        Some(self.instance(id, wanted))
    }

    /// A copy of `id` that falls back to the faces of `families`, one per line, before the system fonts.
    fn cascade(&mut self, id: FontId, families: &str, font: &Font) -> FontId {
        let name = format!("{id}#cascade\n{families}");
        if let Some(&known) = self.names.get(&name) {
            return known;
        }
        let descriptors: Vec<_> = families
            .split('\n')
            .filter_map(|family| {
                let id = self.font(&Font {
                    family: family.into(),
                    ..font.clone()
                })?;
                // SAFETY: plain CoreText query.
                Some(unsafe { self.face(id).font.font_descriptor() })
            })
            .collect();
        if descriptors.is_empty() {
            return id;
        }
        let list = CFArray::from_retained_objects(&descriptors);
        // SAFETY: a descriptor with a cascade list of descriptors, and a copy of a live font with it.
        let copy = unsafe {
            let attributes = CFDictionary::<CFString, CFType>::from_slices(
                &[kCTFontCascadeListAttribute],
                &[list.as_ref()],
            );
            let descriptor = CTFontDescriptor::with_attributes(attributes.as_opaque());
            self.face(id)
                .font
                .copy_with_attributes(0., ptr::null(), Some(&descriptor))
        };
        let (local, bold, oblique) = {
            let face = self.face(id);
            (face.local, face.bold, face.oblique)
        };
        let cascade = self.register(copy, name, local);
        let face = &mut self.faces[cascade as usize];
        (face.bold, face.oblique) = (bold, oblique);
        cascade
    }

    fn load(&mut self, family: &str, features: &[(String, u32)]) -> Option<Vec<FontId>> {
        let family = if family == ".SystemUIFont" {
            ".AppleSystemUIFont"
        } else {
            family
        };
        let start = self
            .added
            .partition_point(|added| added.family.as_str() < family);
        let added: Vec<_> = self.added[start..]
            .iter()
            .take_while(|added| added.family == family)
            .map(|added| {
                // SAFETY: the descriptor came from CoreText.
                unsafe { CTFont::with_font_descriptor(&added.descriptor, 16., ptr::null()) }
            })
            .collect();
        let (faces, local) = if added.is_empty() {
            let faces = SYSTEM_FACES.get((family.to_owned(), features.to_vec()), || {
                Some(
                    usable(installed(family)?.into_iter(), features)
                        .map(|(font, name)| (Shared(font), name))
                        .collect(),
                )
            })?;
            (
                faces
                    .into_iter()
                    .map(|(font, name)| (font.0, name))
                    .collect::<Vec<_>>(),
                false,
            )
        } else {
            (usable(added.into_iter(), features).collect(), true)
        };
        Some(
            faces
                .into_iter()
                .map(|(font, name)| self.register(font, name, local))
                .collect(),
        )
    }

    fn instance(&mut self, id: FontId, axes: Vec<(u32, f32)>) -> FontId {
        let key: Coords = axes
            .iter()
            .map(|&(tag, value)| (tag, value.to_bits()))
            .collect();
        if let Some(&known) = self.instances.get(&(id, key.clone())) {
            return known;
        }
        let face = self.face(id);
        let local = face.local;
        let pointer = CFRetained::as_ptr(&face.font).as_ptr() as usize;
        let make = || {
            let font = open_type::with_variations(&face.font, &axes);
            // The base pointer keeps instances of faces with other features apart.
            let mut name = format!("{}#{pointer:x}", unsafe { font.post_script_name() });
            for (tag, value) in &axes {
                name += &format!("#{tag:x}={value}");
            }
            (Shared(font), name)
        };
        let (font, name) = if local {
            make()
        } else {
            let mut all = INSTANCES.0.lock().unwrap_or_else(PoisonError::into_inner);
            if all.len() >= 4096 {
                all.clear();
            }
            drop(all);
            INSTANCES.get((pointer, key.clone()), make)
        };
        let instance = match self.names.get(&name) {
            Some(&known) => known,
            None => self.register(font.0, name, local),
        };
        self.instances.insert((id, key), instance);
        instance
    }

    fn native(&mut self, font: &CTFont) -> FontId {
        // SAFETY: plain CoreText queries.
        let mut name = unsafe { font.post_script_name() }.to_string();
        for (tag, value) in open_type::variation(font) {
            name += &format!("#native{tag:x}={value}");
        }
        if let Some(&id) = self.names.get(&name) {
            return id;
        }
        // SAFETY: the graphics font comes from a live CoreText font.
        let face = unsafe {
            CTFont::with_graphics_font(&font.graphics_font(ptr::null_mut()), 16., ptr::null(), None)
        };
        self.register(face, name, false)
    }

    fn sized(&mut self, font: FontId, size: f32) -> CFRetained<CTFont> {
        if self.sized.len() >= 512 {
            self.sized.clear();
        }
        let faces = &self.faces;
        self.sized
            .entry((font, size.to_bits()))
            // SAFETY: a copy at another size. CoreText keeps the size of the font for a size of 0, so text of size 0
            // draws at a size too small to show.
            .or_insert_with(|| unsafe {
                faces[font as usize].font.copy_with_attributes(
                    size.max(1e-4) as CGFloat,
                    ptr::null(),
                    None,
                )
            })
            .clone()
    }

    fn shape(&mut self, text: &str, font_size: Pixels, spans: &[Span]) -> Line {
        if spans.is_empty() {
            return self.shape_whole(text, font_size, spans, None);
        }
        if text.is_ascii()
            && !text.contains(['\t', '\n', '\r'])
            && spans.iter().all(|span| self.face(span.font).mono)
        {
            return self.shape_pieces(text, font_size, spans);
        }
        let plain = |ch: char| {
            matches!(ch, ' '..='~' | '\u{a0}'..='\u{58f}' | '\u{1e00}'..='\u{1fff}' | '\u{2010}'..='\u{2027}'
                | '\u{2030}'..='\u{205e}' | '\u{20a0}'..='\u{20cf}')
        };
        if text.chars().all(plain) {
            let mut parts = mem::take(&mut self.words.parts);
            parts.clear();
            let line = self.shape_words(text, font_size, spans, &mut parts);
            self.words.parts = parts;
            if let Some(line) = line {
                debug_assert!(
                    same(&self.shape_whole(text, font_size, spans, None), &line),
                    "words of {text:?} shape unlike the whole line"
                );
                return line;
            }
        }
        self.shape_whole(text, font_size, spans, None)
    }

    fn shape_pieces(&mut self, text: &str, font_size: Pixels, spans: &[Span]) -> Line {
        self.words.trim(true);
        let mut glyphs = Vec::with_capacity(text.len());
        let mut x = 0f32;
        let mut start = 0;
        for span in spans {
            let key = (span.font, span.size.0.to_bits());
            for (offset, piece) in pieces(&text[start..start + span.len]) {
                let found = self
                    .words
                    .pieces
                    .get(&key)
                    .and_then(|cached| cached.get(piece))
                    .cloned();
                let shaped = match found {
                    Some(shaped) => shaped,
                    None => {
                        let alone = Span {
                            len: piece.len(),
                            ..*span
                        };
                        let shaped = Rc::new(
                            match self.digits(span.font, span.size, piece, 0..piece.len()) {
                                Some(word) => {
                                    let line = Line {
                                        glyphs: word
                                            .glyphs
                                            .iter()
                                            .map(|piece| piece.glyph(span.size, 0., 0))
                                            .collect(),
                                        width: px(word.width as f32),
                                        len: piece.len(),
                                        font_size: span.size,
                                        ..Default::default()
                                    };
                                    debug_assert!(
                                        same(
                                            &line,
                                            &self.shape_whole(piece, span.size, &[alone], None)
                                        ),
                                        "digits of {piece:?} shape unlike the whole piece"
                                    );
                                    line
                                }
                                None => self.shape_whole(piece, span.size, &[alone], None),
                            },
                        );
                        self.words.count += 1;
                        self.words
                            .pieces
                            .entry(key)
                            .or_default()
                            .insert(piece.into(), shaped.clone());
                        shaped
                    }
                };
                let at = (start + offset) as u32;
                glyphs.extend(shaped.glyphs.iter().map(|glyph| Glyph {
                    x: glyph.x + px(x),
                    index: glyph.index + at,
                    ..*glyph
                }));
                x += shaped.width.0;
            }
            start += span.len;
        }
        Line {
            glyphs,
            width: px(x),
            len: text.len(),
            font_size,
            ..Default::default()
        }
    }

    fn shape_words(
        &mut self,
        text: &str,
        font_size: Pixels,
        spans: &[Span],
        parts: &mut Vec<Part>,
    ) -> Option<Line> {
        self.words.trim(false);
        let (mut start, mut nudge, mut missing) = (0, false, 0);
        for (index, span) in spans.iter().enumerate() {
            // Every other span shapes a hair larger, as in `shape_whole`, or CoreText joins the spans and forms
            // ligatures across them.
            let size = if nudge {
                px(span.size.0.next_up())
            } else {
                span.size
            };
            let key = (span.font, size.0.to_bits());
            let source = &text[start..start + span.len];
            for (offset, piece) in pieces(source) {
                let range = start + offset..start + offset + piece.len();
                let around = u8::from(offset > 0) | u8::from(range.end < start + source.len()) << 1;
                if piece.starts_with(' ')
                    && around & 2 != 0
                    && let Some(next) = text[range.end..].chars().next()
                    && !self.spaced(span.font, size, next)
                {
                    return None;
                }
                let word = (self.words.faces.get(&key)).and_then(|face| {
                    face.words.get(&text[range.clone()])?[usize::from(around)].clone()
                });
                missing += usize::from(word.is_none());
                parts.push(Part {
                    span: index,
                    size,
                    range,
                    around,
                    word,
                });
            }
            nudge ^= span.len > 0;
            start += span.len;
        }
        if missing > 2 {
            let mut pens = Pens::default();
            let whole = self.shape_whole(text, font_size, spans, Some(&mut pens));
            self.harvest(text, 0, &whole, &pens, spans, parts);
            return Some(whole);
        }
        for at in 0..parts.len() {
            if parts[at].word.is_some() {
                continue;
            }
            let Part {
                span,
                size,
                ref range,
                around,
                ..
            } = parts[at];
            let before = (around & 1 != 0)
                .then(|| text[..range.start].chars().next_back())
                .flatten();
            let after = (around & 2 != 0)
                .then(|| text[range.end..].chars().next())
                .flatten();
            let context = range.start - before.map_or(0, char::len_utf8)
                ..range.end + after.map_or(0, char::len_utf8);
            let inner = range.start - context.start..range.end - context.start;
            if let Some(word) = self.digits(spans[span].font, size, &text[context.clone()], inner) {
                parts[at].word = Some(Rc::new(word));
                continue;
            }
            let alone = Span {
                len: context.len(),
                size,
                ..spans[span]
            };
            let mut pens = Pens::default();
            let shaped = self.shape_whole(&text[context.clone()], size, &[alone], Some(&mut pens));
            self.harvest(
                text,
                context.start,
                &shaped,
                &pens,
                spans,
                &mut parts[at..at + 1],
            );
            parts[at].word.as_ref()?;
        }
        let count = parts
            .iter()
            .map(|part| part.word.as_ref().map_or(0, |word| word.glyphs.len()))
            .sum();
        let mut glyphs = Vec::with_capacity(count);
        let mut x = 0f64;
        let mut last: Option<(usize, Pixels)> = None;
        for part in parts.iter() {
            let Some(word) = &part.word else {
                continue;
            };
            let size = match last {
                Some((span, size)) if span == part.span => size,
                _ => nearest(spans, part.size.0, font_size),
            };
            last = Some((part.span, size));
            glyphs.extend(
                word.glyphs
                    .iter()
                    .map(|piece| piece.glyph(size, x, part.range.start)),
            );
            x += word.width;
        }
        Some(Line {
            glyphs,
            width: px(x as f32),
            len: text.len(),
            font_size,
            ..Default::default()
        })
    }

    fn harvest(
        &mut self,
        text: &str,
        at: usize,
        line: &Line,
        pens: &Pens,
        spans: &[Span],
        parts: &mut [Part],
    ) {
        let glyphs = &line.glyphs;
        if glyphs.iter().any(|glyph| glyph.emoji) {
            return;
        }
        for part in parts {
            let first =
                glyphs.partition_point(|glyph| glyph.index as usize + at < part.range.start);
            let count = glyphs[first..]
                .partition_point(|glyph| (glyph.index as usize + at) < part.range.end);
            let own = &glyphs[first..first + count];
            if part.word.is_some()
                || own
                    .first()
                    .is_none_or(|head| own.iter().any(|glyph| glyph.font != head.font))
            {
                continue;
            }
            // CoreText adjusts glyphs at the edges of its runs, so a word without a space beside it takes its
            // edge from a run boundary.
            let edge = |index: usize| match index - at {
                end if end == line.len => Some(pens.xs[glyphs.len()]),
                index => pens
                    .starts
                    .iter()
                    .find(|start| start.0 == index)
                    .map(|start| start.1),
            };
            let left = if part.around & 1 == 0 {
                edge(part.range.start)
            } else {
                Some(pens.xs[first])
            };
            let right = if part.around & 2 == 0 {
                edge(part.range.end)
            } else {
                Some(pens.xs[first + count])
            };
            let (Some(left), Some(right)) = (left, right) else {
                continue;
            };
            let word = Rc::new(Word {
                glyphs: own
                    .iter()
                    .zip(&pens.xs[first..])
                    .map(|(glyph, x)| Piece {
                        x: x - left,
                        font: glyph.font,
                        id: glyph.id,
                        y: glyph.y.0,
                        offset: (glyph.index as usize + at - part.range.start) as u32,
                    })
                    .collect(),
                width: right - left,
            });
            self.words.count += 1;
            let face = self
                .words
                .faces
                .entry((spans[part.span].font, part.size.0.to_bits()))
                .or_default();
            face.words
                .entry(text[part.range.clone()].into())
                .or_default()[usize::from(part.around)] = Some(word.clone());
            part.word = Some(word);
        }
    }

    fn spaced(&mut self, font: FontId, size: Pixels, next: char) -> bool {
        let key = (font, size.0.to_bits());
        if let Some(&spaced) = self
            .words
            .faces
            .get(&key)
            .and_then(|face| face.spaced.get(&next))
        {
            return spaced;
        }
        let span = Span {
            len: 1,
            font,
            size,
            letter_spacing: px(0.),
            word_spacing: px(0.),
            inset: [px(0.); 2],
            nowrap: false,
        };
        let space = self.shape_whole(" ", size, &[span], None).width;
        let pair = format!(" {next}");
        let both = self.shape_whole(
            &pair,
            size,
            &[Span {
                len: pair.len(),
                ..span
            }],
            None,
        );
        let spaced = (both.glyphs.iter().find(|glyph| glyph.index > 0))
            .is_some_and(|glyph| (glyph.x - space).abs() < px(1e-4));
        self.words
            .faces
            .entry(key)
            .or_default()
            .spaced
            .insert(next, spaced);
        spaced
    }

    /// `word` is the part of `text` to return; the rest is context.
    fn digits(
        &mut self,
        font: FontId,
        size: Pixels,
        text: &str,
        word: Range<usize>,
    ) -> Option<Word> {
        const CHARS: &[u8; 11] = b"0123456789 ";
        let bytes = text.as_bytes();
        if !bytes.iter().all(|byte| CHARS.contains(byte)) {
            return None;
        }
        let slot = |byte: u8| {
            if byte == b' ' {
                10
            } else {
                usize::from(byte - b'0')
            }
        };
        let key = (font, size.0.to_bits());
        let face = self.words.faces.entry(key).or_default();
        face.misses = face.misses.saturating_add(1);
        let misses = face.misses;
        let mut table = match face.digits.take() {
            Some(table) => table,
            None if misses == 32 => self.digit_table(font, size, CHARS)?,
            None => return None,
        };
        let stitched = 'stitch: {
            if bytes.windows(3).any(|three| {
                !table.triples[slot(three[0]) * 121 + slot(three[1]) * 11 + slot(three[2])]
            }) {
                break 'stitch None;
            }
            let n = bytes.len();
            for (a, b) in [(0, 1), (n.wrapping_sub(2), n.wrapping_sub(1))] {
                let Some((&a, &b)) = bytes.get(a).zip(bytes.get(b)) else {
                    break;
                };
                let (a, b) = (slot(a), slot(b));
                if table.edges[a][b] == 0 {
                    let (glyphs, xs) = self.shape_digits(font, size, CHARS, &[a, b]);
                    let advance = table.advances[a][b];
                    let kept = matches!(glyphs[..], [(f, x, 0, true), (g, y, 1, true)]
                        if f == table.font && g == table.font && x == table.glyphs[a] && y == table.glyphs[b])
                        && xs[0] == 0.
                        && (xs[1] - advance).abs() < 1e-9
                        && (xs[2] - advance - table.ends[b]).abs() < 1e-9;
                    table.edges[a][b] = if kept { 1 } else { 2 };
                }
                if table.edges[a][b] != 1 {
                    break 'stitch None;
                }
            }
            let (mut x, mut left) = (0., 0.);
            let mut glyphs = Vec::with_capacity(word.len());
            for (at, &byte) in bytes.iter().enumerate().take(word.end) {
                if at == word.start {
                    left = x;
                }
                if at >= word.start {
                    glyphs.push(Piece {
                        x: x - left,
                        font: table.font,
                        id: table.glyphs[slot(byte)],
                        y: 0.,
                        offset: (at - word.start) as u32,
                    });
                }
                x += match bytes.get(at + 1) {
                    Some(&next) => table.advances[slot(byte)][slot(next)],
                    None => table.ends[slot(byte)],
                };
            }
            Some(Word {
                glyphs: glyphs.into(),
                width: x - left,
            })
        };
        self.words.faces.get_mut(&key)?.digits = Some(table);
        stitched
    }

    fn digit_table(&mut self, font: FontId, size: Pixels, chars: &[u8; 11]) -> Option<Box<Digits>> {
        let mut table = Box::new(Digits {
            font: FontId::MAX,
            glyphs: [0; 11],
            ends: [0.; 11],
            advances: [[f64::NAN; 11]; 11],
            triples: vec![false; 11 * 11 * 11].into(),
            edges: [[0; 11]; 11],
        });
        for at in 0..11 {
            let (glyphs, xs) = self.shape_digits(font, size, chars, &[at]);
            let [(face, id, 0, true)] = glyphs[..] else {
                return None;
            };
            if xs[0] != 0. || at > 0 && face != table.font {
                return None;
            }
            (table.font, table.glyphs[at], table.ends[at]) = (face, id, xs[1]);
        }
        // The concatenated Lyndon words of lengths 1 and 3 in order: the least de Bruijn sequence.
        let mut order = Vec::with_capacity(11 * 11 * 11 + 2);
        let mut lyndon = vec![0];
        loop {
            if 3 % lyndon.len() == 0 {
                order.extend_from_slice(&lyndon);
            }
            let period = lyndon.len();
            while lyndon.len() < 3 {
                lyndon.push(lyndon[lyndon.len() - period]);
            }
            while lyndon.last() == Some(&10) {
                lyndon.pop();
            }
            match lyndon.last_mut() {
                Some(last) => *last += 1,
                None => break,
            }
        }
        order.extend([order[0], order[1]]);
        let (glyphs, xs) = self.shape_digits(font, size, chars, &order);
        if glyphs.len() != order.len() {
            return None;
        }
        let mut moved = vec![false; order.len()];
        for (at, &(face, id, index, plain)) in glyphs.iter().enumerate() {
            moved[at] |=
                !plain || face != table.font || id != table.glyphs[order[at]] || index != at;
            if let Some(&next) = order.get(at + 1) {
                let step = xs[at + 1] - xs[at];
                let advance = &mut table.advances[order[at]][next];
                if advance.is_nan() {
                    *advance = step;
                } else if (*advance - step).abs() > 1e-9 {
                    moved[at] = true;
                    moved[at + 1] = true;
                }
            }
        }
        for (at, three) in order.windows(3).enumerate() {
            table.triples[three[0] * 121 + three[1] * 11 + three[2]] =
                !moved[at..at + 3].contains(&true);
        }
        Some(table)
    }

    fn shape_digits(
        &mut self,
        font: FontId,
        size: Pixels,
        chars: &[u8; 11],
        slots: &[usize],
    ) -> (Vec<Plain>, Vec<f64>) {
        let text: String = slots.iter().map(|&slot| char::from(chars[slot])).collect();
        let span = Span {
            len: text.len(),
            font,
            size,
            letter_spacing: px(0.),
            word_spacing: px(0.),
            inset: [px(0.); 2],
            nowrap: false,
        };
        let mut pens = Pens::default();
        let line = self.shape_whole(&text, size, &[span], Some(&mut pens));
        let glyphs = (line.glyphs.iter())
            .map(|glyph| {
                (
                    glyph.font,
                    glyph.id,
                    glyph.index as usize,
                    glyph.y == px(0.) && !glyph.emoji,
                )
            })
            .collect();
        (glyphs, pens.xs)
    }

    fn shape_whole(
        &mut self,
        text: &str,
        font_size: Pixels,
        spans: &[Span],
        mut pens: Option<&mut Pens>,
    ) -> Line {
        let string =
            CFMutableAttributedString::new(None, 0).expect("CFAttributedStringCreateMutable");
        let (mut rest, mut nudge) = (text, false);
        // Instances share a PostScript name, so runs map back to their span font by pointer.
        let mut own = Vec::with_capacity(spans.len());
        for span in spans {
            let piece;
            (piece, rest) = rest.split_at(span.len);
            let start = string.length();
            // SAFETY: the string is live and the range is its end.
            // a CFString drops some code points, like a leading byte order mark, so measure the length after the append.
            unsafe {
                CFMutableAttributedString::replace_string(
                    Some(&string),
                    CFRange::new(start, 0),
                    Some(&CFString::from_str(piece)),
                )
            };
            let range = CFRange::new(start, string.length() - start);
            // Alternate spans shape a hair larger so CoreText keeps them apart and forms no ligatures across
            // them. The first stays exact for the tracking of SF.
            let size = if nudge {
                span.size.0.next_up()
            } else {
                span.size.0
            };
            let font = self.sized(span.font, size);
            own.push((CFRetained::as_ptr(&font).as_ptr() as usize, span.font));
            // SAFETY: the range is inside the string, and the value is a font.
            unsafe {
                CFMutableAttributedString::set_attribute(
                    Some(&string),
                    range,
                    Some(kCTFontAttributeName),
                    Some(&font),
                )
            };
            if range.length > 0 {
                nudge = !nudge;
            }
        }
        // SAFETY: CoreText lays out a live attributed string, and its runs are CTRuns with font attributes.
        let line = unsafe { CTLine::with_attributed_string(&string) };
        let runs = unsafe { line.glyph_runs() };
        let runs = unsafe { runs.cast_unchecked::<CTRun>() };
        let mut glyphs = Vec::with_capacity(unsafe { line.glyph_count() } as usize);
        let mut scratch = mem::take(&mut self.scratch);
        let mut cursor = Utf16Cursor {
            text,
            utf8: 0,
            utf16: 0,
        };
        let (mut shift, mut emoji_end, mut pen) = (0f32, None::<f32>, 0f64);
        for run in runs.iter() {
            let font = unsafe {
                let attributes = run.attributes();
                let attributes = attributes.cast_unchecked::<CFString, CFType>();
                CFRetained::cast_unchecked::<CTFont>(
                    attributes.get(kCTFontAttributeName).expect("run font"),
                )
            };
            let run_size = nearest(spans, unsafe { font.size() } as f32, font_size);
            if self.natives.len() >= 512 {
                self.natives.clear();
            }
            let key = CFRetained::as_ptr(&font).as_ptr() as usize;
            let id = match own.iter().find(|(pointer, _)| *pointer == key) {
                Some(&(_, id)) => id,
                None => match self.natives.get(&key) {
                    Some(&(_, id)) => id,
                    None => {
                        let id = self.native(&font);
                        self.natives.insert(key, (font.clone(), id));
                        id
                    }
                },
            };
            let emoji = self.face(id).emoji;
            // The emoji font that system fonts fall back to has wider advances than Chromium's.
            let wide = emoji.then(|| unsafe {
                CTFont::with_name(
                    &CFString::from_static_str("AppleColorEmoji"),
                    f64::from(run_size.0) * 2.,
                    ptr::null(),
                )
            });
            let count = unsafe { run.glyph_count() } as usize;
            let all = CFRange::new(0, 0);
            scratch.ids.resize(count, 0);
            scratch.points.resize(count, CGPoint::ZERO);
            scratch.indices.resize(count, 0);
            // SAFETY: each buffer holds `count` values.
            unsafe {
                if count > 0 {
                    run.glyphs(all, NonNull::new_unchecked(scratch.ids.as_mut_ptr()));
                    run.positions(all, NonNull::new_unchecked(scratch.points.as_mut_ptr()));
                    run.string_indices(all, NonNull::new_unchecked(scratch.indices.as_mut_ptr()));
                }
            }
            let mut first = true;
            for k in 0..count {
                cursor.seek(scratch.indices[k] as usize);
                let at = scratch.points[k];
                if let Some(end) = emoji_end.take() {
                    shift = end - at.x as f32;
                }
                let x = at.x as f32 + shift;
                if let Some(pens) = pens.as_deref_mut() {
                    if mem::take(&mut first) {
                        pens.starts.push((cursor.utf8, pen));
                    }
                    pens.xs.push(at.x + f64::from(shift));
                }
                let mut id_raw = scratch.ids[k];
                glyphs.push(Glyph {
                    x: px(x),
                    y: px(at.y as f32),
                    size: run_size,
                    font: id,
                    index: cursor.utf8 as u32,
                    id: id_raw,
                    emoji,
                });
                if let Some(wide) = &wide {
                    // SAFETY: one glyph; the advances out pointer may be null.
                    let mut drawn = unsafe {
                        wide.advances_for_glyphs(
                            CTFontOrientation::Default,
                            NonNull::from(&mut id_raw),
                            ptr::null_mut(),
                            1,
                        )
                    } as f32;
                    // Where CoreText enlarges emoji, Chromium's advance is one device pixel less.
                    if drawn > run_size.0 * 2. {
                        drawn -= 1.;
                    }
                    emoji_end = Some(x + drawn / 2.);
                }
            }
            if pens.is_some() {
                // SAFETY: the null pointers ask for the width only.
                pen += unsafe {
                    run.typographic_bounds(all, ptr::null_mut(), ptr::null_mut(), ptr::null_mut())
                };
            }
        }
        self.scratch = scratch;
        // SAFETY: the null pointers ask for the width only.
        let width =
            unsafe { line.typographic_bounds(ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) };
        if let Some(pens) = pens {
            pens.xs.push(width + f64::from(shift));
        }
        Line {
            glyphs,
            width: px(emoji_end.unwrap_or(width as f32 + shift)),
            len: text.len(),
            font_size,
            ..Default::default()
        }
    }
}

fn installed(family: &str) -> Option<Vec<CFRetained<CTFont>>> {
    let name = CFString::from_str(family);
    // SAFETY: a descriptor and collection query with valid attributes.
    unsafe {
        let attributes =
            CFDictionary::<CFString, CFType>::from_slices(&[kCTFontFamilyNameAttribute], &[&name]);
        let query = CTFontDescriptor::with_attributes(attributes.as_opaque());
        let collection = CTFontCollection::with_font_descriptors(
            Some(CFArray::from_objects(&[&*query]).as_opaque()),
            None,
        );
        let found = collection.matching_font_descriptors()?;
        let found = found.cast_unchecked::<CTFontDescriptor>();
        Some(
            found
                .iter()
                .map(|descriptor| CTFont::with_font_descriptor(&descriptor, 16., ptr::null()))
                .collect(),
        )
        .filter(|faces: &Vec<_>| !faces.is_empty())
    }
}

fn usable<'a>(
    faces: impl Iterator<Item = CFRetained<CTFont>> + 'a,
    features: &'a [(String, u32)],
) -> impl Iterator<Item = (CFRetained<CTFont>, String)> + 'a {
    let mut seen = Vec::<String>::new();
    faces.filter_map(move |face| {
        let face = open_type::with_features(&face, features);
        // SAFETY: plain CoreText queries.
        let traits = unsafe { face.traits() };
        let traits = unsafe { traits.cast_unchecked::<CFString, CFType>() };
        // SAFETY: the keys are CoreText constants.
        let keys = unsafe {
            [
                kCTFontSymbolicTrait,
                kCTFontWidthTrait,
                kCTFontWeightTrait,
                kCTFontSlantTrait,
            ]
        };
        if !keys.iter().all(|key| {
            traits
                .get(key)
                .is_some_and(|value| value.downcast::<CFNumber>().is_ok())
        }) {
            return None;
        }
        let name = unsafe { face.post_script_name() }.to_string();
        // Duplicates drop only within one family load, so a second load never ends up without faces.
        if seen.contains(&name) {
            return None;
        }
        seen.push(name.clone());
        Some((face, name))
    })
}

fn pieces(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let bytes = text.as_bytes();
    let mut start = 0;
    std::iter::from_fn(move || {
        let first = *bytes.get(start)?;
        let len = bytes[start..]
            .iter()
            .position(|&byte| (byte == b' ') != (first == b' '))
            .unwrap_or(bytes.len() - start);
        let piece = (start, &text[start..start + len]);
        start += len;
        Some(piece)
    })
}

fn nearest(spans: &[Span], shaped: f32, fallback: Pixels) -> Pixels {
    spans
        .iter()
        .map(|span| span.size)
        .min_by(|a, b| (a.0 - shaped).abs().total_cmp(&(b.0 - shaped).abs()))
        .unwrap_or(fallback)
}

/// Maps UTF-16 offsets from CoreText to UTF-8 offsets, walking either way.
struct Utf16Cursor<'a> {
    text: &'a str,
    utf8: usize,
    utf16: usize,
}

impl Utf16Cursor<'_> {
    fn seek(&mut self, utf16: usize) {
        while self.utf16 > utf16
            && let Some(ch) = self.text[..self.utf8].chars().next_back()
        {
            self.utf8 -= ch.len_utf8();
            self.utf16 -= ch.len_utf16();
        }
        for ch in self.text[self.utf8..].chars() {
            if self.utf16 >= utf16 {
                return;
            }
            self.utf8 += ch.len_utf8();
            self.utf16 += ch.len_utf16();
        }
    }
}

/// A shaped glyph of digits: face, glyph, source index, and whether it sits on the baseline and is no emoji.
type Plain = (FontId, GlyphId, usize, bool);

#[derive(Default)]
struct Pens {
    xs: Vec<f64>,
    starts: Vec<(usize, f64)>,
}

#[derive(Default)]
struct Words {
    faces: FxHashMap<(FontId, u32), FaceWords>,
    pieces: FxHashMap<(FontId, u32), FxHashMap<Box<str>, Rc<Line>>>,
    count: usize,
    parts: Vec<Part>,
}

impl Words {
    fn trim(&mut self, pieces: bool) {
        if self.count > 16384 {
            self.faces.clear();
            if pieces {
                self.pieces.clear();
            }
            self.count = 0;
        }
    }
}

#[derive(Default)]
struct FaceWords {
    /// By text, then by whether text comes before (bit 0) and after (bit 1) it in its span.
    words: FxHashMap<Box<str>, [Option<Rc<Word>>; 4]>,
    spaced: FxHashMap<char, bool>,
    misses: u32,
    digits: Option<Box<Digits>>,
}

struct Digits {
    font: FontId,
    glyphs: [GlyphId; 11],
    ends: [f64; 11],
    advances: [[f64; 11]; 11],
    triples: Box<[bool]>,
    /// Pairs at word edges: 0 unknown, 1 stitch, 2 shape.
    edges: [[u8; 11]; 11],
}

struct Word {
    glyphs: Box<[Piece]>,
    width: f64,
}

#[derive(Clone, Copy)]
struct Piece {
    x: f64,
    font: FontId,
    id: GlyphId,
    y: f32,
    offset: u32,
}

impl Piece {
    fn glyph(&self, size: Pixels, pen: f64, start: usize) -> Glyph {
        Glyph {
            x: px((pen + self.x) as f32),
            y: px(self.y),
            size,
            font: self.font,
            index: start as u32 + self.offset,
            id: self.id,
            emoji: false,
        }
    }
}

struct Part {
    span: usize,
    size: Pixels,
    range: Range<usize>,
    around: u8,
    word: Option<Rc<Word>>,
}

fn same(a: &Line, b: &Line) -> bool {
    (a.width - b.width).abs() < px(1e-3)
        && a.glyphs.len() == b.glyphs.len()
        && a.glyphs.iter().zip(&b.glyphs).all(|(a, b)| {
            (a.font, a.size, a.id, a.index, a.emoji, a.y)
                == (b.font, b.size, b.id, b.index, b.emoji, b.y)
                && (a.x - b.x).abs() < px(1e-3)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &TextSystem, id: FontId) -> String {
        let book = text.0.borrow();
        book.names
            .iter()
            .find(|(_, kept)| **kept == id)
            .unwrap()
            .0
            .clone()
    }

    #[test]
    fn system_fonts_load_once_per_process() {
        let names = |text: &TextSystem, weight: f32| {
            let id = text
                .font(&Font {
                    weight,
                    ..Font::new(".SystemUIFont")
                })
                .unwrap();
            name(text, id)
        };
        let [first, second] = [TextSystem::default(), TextSystem::default()];
        let bold = names(&first, 600.);
        assert!(bold.ends_with("#77676874=600"), "{bold}");
        assert_eq!(
            [names(&second, 400.), names(&second, 600.)],
            [names(&first, 400.), bold]
        );
        let missing = Font::new("No Such Family h2o");
        assert!(first.font(&missing).is_none() && second.font(&missing).is_none());
        let faces = SYSTEM_FACES.0.lock().unwrap();
        assert!(
            faces
                .iter()
                .any(|(key, faces)| key.0 == "No Such Family h2o"
                    && faces.get().is_some_and(Option::is_none))
        );
        let system = faces
            .iter()
            .find(|(key, _)| key.0 == ".AppleSystemUIFont")
            .unwrap();
        let face = &system.1.get().unwrap().as_ref().unwrap()[0].0.0;
        let shared = second
            .0
            .borrow()
            .faces
            .iter()
            .any(|kept| CFRetained::as_ptr(&kept.font) == CFRetained::as_ptr(face));
        assert!(shared, "the second text system uses the process's faces");
    }

    #[test]
    fn later_families_fill_in_missing_glyphs_before_the_system_fonts() {
        let text = TextSystem::default();
        let fonts = |family: &str| {
            let id = text.font(&Font::new(family)).unwrap();
            let line = text.shape(
                "aא",
                px(16.),
                &[Span {
                    len: "aא".len(),
                    font: id,
                    size: px(16.),
                    letter_spacing: px(0.),
                    word_spacing: px(0.),
                    inset: [px(0.); 2],
                    nowrap: false,
                }],
            );
            line.glyphs
                .iter()
                .map(|glyph| name(&text, glyph.font))
                .collect::<Vec<_>>()
        };
        let alone = fonts("Helvetica");
        let listed = fonts("Helvetica\nTimes New Roman");
        assert!(
            alone[0].starts_with("Helvetica") && !alone[1].starts_with("TimesNewRoman"),
            "{alone:?}"
        );
        assert!(
            listed[0].contains("#cascade") && listed[1].starts_with("TimesNewRoman"),
            "{listed:?}"
        );
    }

    #[test]
    fn faces_without_bold_or_italic_get_synthetic_ones_like_chrome() {
        let text = TextSystem::default();
        let face = |weight, style, synthesis| {
            let id = text
                .font(&Font {
                    weight,
                    style,
                    synthesis,
                    ..Font::new("Herculanum")
                })
                .unwrap();
            let book = text.0.borrow();
            (id, book.face(id).bold, book.face(id).oblique)
        };
        let on = [true; 2];
        let (regular, ..) = face(400., FontStyle::Normal, on);
        assert_eq!(face(500., FontStyle::Normal, on).0, regular);
        let (bold, true, false) = face(600., FontStyle::Normal, on) else {
            panic!("600 is bold")
        };
        let (slanted, false, true) = face(400., FontStyle::Italic, on) else {
            panic!("italic slants")
        };
        assert!(matches!(
            face(700., FontStyle::Oblique, on),
            (_, true, true)
        ));
        // `font-synthesis` turns each off.
        assert_eq!(face(700., FontStyle::Italic, [false; 2]).0, regular);
        assert!(matches!(
            face(700., FontStyle::Italic, [false, true]),
            (_, false, true)
        ));
        // Helvetica has its own bold and italic faces.
        let helvetica = text.font(&Font {
            weight: 700.,
            style: FontStyle::Oblique,
            ..Font::new("Helvetica")
        });
        let book = text.0.borrow();
        let face = book.face(helvetica.unwrap());
        assert!(!face.bold && !face.oblique);
        drop(book);
        let glyph = text.glyph(regular, 'H').unwrap();
        let raster = |font, size, scale| GlyphRaster {
            font,
            glyph,
            size: px(size),
            scale,
            subpixel: 0,
            emoji: false,
            dilation: 0,
            stroke: 0.,
        };
        let bounds = |font| text.raster_bounds(&raster(font, 48., 2.)).unwrap();
        let [plain, bold_box, slanted_box] = [regular, bold, slanted].map(bounds);
        assert!(bold_box.size.width > plain.size.width && bold_box.size.height > plain.size.height);
        assert!(
            slanted_box.size.width > plain.size.width
                && slanted_box.size.height == plain.size.height
        );
        // Like Chrome, the advances stay those of the face.
        assert_eq!(text.advance(bold, glyph), text.advance(regular, glyph));
        // Small text at 1x gets bolder too.
        for (size, scale) in [(48., 2.), (12., 1.)] {
            let ink = |font| {
                let glyph = raster(font, size, scale);
                let (_, pixels) = text
                    .rasterize(&glyph, text.raster_bounds(&glyph).unwrap())
                    .unwrap();
                pixels.iter().map(|&value| u32::from(value)).sum::<u32>()
            };
            assert!(ink(bold) > ink(regular) * 11 / 10, "{size}px at {scale}x");
        }
    }

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

    #[test]
    fn fonts_from_memory_stay_with_their_text_system() {
        let bytes = std::fs::read("/System/Library/Fonts/Supplemental/Chalkduster.ttf").unwrap();
        let text = TextSystem::default();
        text.add_font(bytes, "Chalkduster").unwrap();
        assert!(text.families().iter().any(|family| family == "Chalkduster"));
        let id = text.font(&Font::new("Chalkduster")).unwrap();
        assert!(text.0.borrow().face(id).local);
        assert!(
            !SYSTEM_FACES
                .0
                .lock()
                .unwrap()
                .keys()
                .any(|key| key.0 == "Chalkduster")
        );
    }

    #[test]
    fn variable_weights_reuse_their_instances() {
        let bytes = std::fs::read("/System/Library/Fonts/SFNS.ttf").unwrap();
        let text = TextSystem::default();
        text.add_font(bytes, "Variable").unwrap();
        let font = |weight| {
            text.font(&Font {
                weight,
                ..Font::new("Variable")
            })
            .unwrap()
        };
        let bold = font(600.);
        assert_eq!(font(600.2), bold);
        let faces = text.0.borrow().faces.len();
        // A transition steps through many values, each time it runs.
        for _ in 0..3 {
            for step in 0..=300 {
                font(400. + step as f32 * 0.37);
            }
        }
        assert!(text.0.borrow().faces.len() - faces <= 112);
        assert_eq!(font(600.), bold);
    }

    #[test]
    fn css_matching_prefers_the_spec_order() {
        let faces = [
            (400., FontStyle::Normal, 1.),
            (700., FontStyle::Normal, 1.),
            (300., FontStyle::Italic, 1.),
        ];
        assert_eq!(best(&faces, 400., FontStyle::Normal), Some(0));
        assert_eq!(best(&faces, 600., FontStyle::Normal), Some(1));
        assert_eq!(best(&faces, 500., FontStyle::Normal), Some(0));
        assert_eq!(best(&faces, 900., FontStyle::Italic), Some(2));
        let narrow = [
            (400., FontStyle::Normal, 0.75),
            (400., FontStyle::Normal, 1.25),
        ];
        assert_eq!(best(&narrow, 400., FontStyle::Normal), Some(0));
        assert_eq!(
            (css_weight(0.), css_weight(0.23), css_weight(-1.)),
            (400., 500., 100.)
        );
        assert_eq!((css_stretch(0.), css_stretch(-1.)), (1., 0.5));
        assert_eq!(find_index(0.1, &KIT_WEIGHTS), 3.5);
    }

    #[test]
    fn word_and_digit_caches_shape_like_whole_lines() {
        let text = TextSystem::default();
        let id = text.font(&Font::new("Helvetica")).unwrap();
        let span = |len| Span {
            len,
            font: id,
            size: px(16.),
            letter_spacing: px(0.),
            word_spacing: px(0.),
            inset: [px(0.); 2],
            nowrap: false,
        };
        for sample in [
            "Hello world, again",
            "Type 1234 and 5678 then 90",
            "AV Wa To. fi fl",
        ] {
            let line = text.shape(sample, px(16.), &[span(sample.len())]);
            for _ in 0..40 {
                let again = text.shape(sample, px(16.), &[span(sample.len())]);
                assert!(same(&line, &again), "{sample}");
            }
            let whole =
                text.0
                    .borrow_mut()
                    .shape_whole(sample, px(16.), &[span(sample.len())], None);
            assert!(same(&line, &whole), "{sample}");
        }
        assert!(text.0.borrow().words.count > 0);
    }
}
