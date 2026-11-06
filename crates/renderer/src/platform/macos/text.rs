use crate::{
    Bounds, DevicePixels, Pixels, Rgba, Size, point, px, size,
    text::{
        Font, FontFallbacks, FontFeatures, FontId, FontMetrics, FontRun, FontStyle, FontWeight,
        GlyphId, LineLayout, RenderGlyphParams, SUBPIXEL_VARIANTS_X, ShapedGlyph, ShapedRun,
        SharedString,
    },
};
use anyhow::Result;
use anyhow::anyhow;
use core_foundation::{
    array::{CFArray, CFArrayRef},
    attributed_string::CFMutableAttributedString,
    base::{CFRange, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_graphics::{
    base::{CGFloat, CGGlyph, kCGImageAlphaPremultipliedLast},
    color_space::CGColorSpace,
    context::{CGContext, CGTextDrawingMode},
    geometry::CGPoint,
};
use core_text::{
    font::CTFont,
    font_descriptor::{
        kCTFontSlantTrait, kCTFontSymbolicTrait, kCTFontWeightTrait, kCTFontWidthTrait,
    },
    line::CTLine,
    string_attributes::kCTFontAttributeName,
};
use font_kit::{
    font::Font as FontKitFont,
    handle::Handle,
    hinting::HintingOptions,
    metrics::Metrics,
    properties::{Style as FontkitStyle, Weight as FontkitWeight},
    source::SystemSource,
    sources::mem::MemSource,
};
use pathfinder_geometry::{
    rect::{RectF, RectI},
    transform2d::Transform2F,
    vector::Vector2F,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use smallvec::SmallVec;
use std::cell::RefCell;
use std::{borrow::Cow, char, convert::TryFrom, sync::Arc};

use core_foundation::base::CFType;
use std::sync::OnceLock;

use super::open_type::apply_features_and_fallbacks;

/// The `wght` variation axis tag.
const WGHT: i64 = 0x77676874;

unsafe extern "C" {
    static kCTFontVariationAttribute: core_foundation::string::CFStringRef;
    static kCTFontVariationAxisIdentifierKey: core_foundation::string::CFStringRef;
    static kCTFontVariationAxisDefaultValueKey: core_foundation::string::CFStringRef;
    fn CTFontCopyVariation(
        font: core_text::font::CTFontRef,
    ) -> core_foundation::dictionary::CFDictionaryRef;
    fn CTFontCreateCopyWithAttributes(
        font: core_text::font::CTFontRef,
        size: CGFloat,
        matrix: *const core_graphics::geometry::CGAffineTransform,
        attributes: core_text::font_descriptor::CTFontDescriptorRef,
    ) -> core_text::font::CTFontRef;
}

/// The `wght` axis value of a variable font instance.
fn variation_weight(font: &CTFont) -> Option<f32> {
    let variations = unsafe { CTFontCopyVariation(font.as_concrete_TypeRef()) };
    if variations.is_null() {
        return None;
    }
    let variations: CFDictionary<CFNumber, CFNumber> =
        unsafe { CFDictionary::wrap_under_create_rule(variations) };
    variations
        .find(CFNumber::from(WGHT))
        .and_then(|weight| weight.to_f64())
        .map(|weight| weight as f32)
}

#[allow(non_upper_case_globals)]
const kCGImageAlphaOnly: u32 = 7;

/// Fonts through font-kit, shaping through CoreText and glyphs through CoreGraphics.
pub(crate) struct TextSystem(RefCell<AppleTextSystemState>);

#[derive(Clone, PartialEq, Eq, Hash)]
struct FontKey {
    font_family: SharedString,
    font_features: FontFeatures,
    font_fallbacks: Option<FontFallbacks>,
}

struct AppleTextSystemState {
    memory_source: MemSource,
    system_source: SystemSource,
    fonts: Vec<FontKitFont>,
    font_selections: HashMap<Font, FontId>,
    font_ids_by_postscript_name: HashMap<String, FontId>,
    font_ids_by_font_key: HashMap<FontKey, SmallVec<[FontId; 4]>>,
    postscript_names_by_font_id: HashMap<FontId, String>,
    /// Faces at a point size, by font and size bits.
    sized: HashMap<(FontId, u32), CTFont>,
    /// The font of each shaped run's face, kept alive so its address stays unique.
    natives: HashMap<usize, (CTFont, FontId)>,
}

/// The installed font families, listed once for all windows.
static FAMILIES: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
    unsafe extern "C" {
        fn CTFontManagerCopyAvailableFontFamilyNames() -> CFArrayRef;
    }
    // SAFETY: the function follows the Create Rule and returns an array of strings.
    let families: CFArray<CFString> =
        unsafe { CFArray::wrap_under_create_rule(CTFontManagerCopyAvailableFontFamilyNames()) };
    families.iter().map(|name| name.to_string()).collect()
});

impl TextSystem {
    pub fn new() -> Self {
        // WARNING: listing the font families takes about 20 ms, so it starts here, off the first frame's path.
        static LISTING: std::sync::Once = std::sync::Once::new();
        LISTING.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("font families".into())
                .spawn(|| std::sync::LazyLock::force(&FAMILIES).len());
        });
        Self(RefCell::new(AppleTextSystemState {
            memory_source: MemSource::empty(),
            system_source: SystemSource::new(),
            fonts: Vec::new(),
            font_selections: HashMap::default(),
            font_ids_by_postscript_name: HashMap::default(),
            font_ids_by_font_key: HashMap::default(),
            postscript_names_by_font_id: HashMap::default(),
            sized: HashMap::default(),
            natives: HashMap::default(),
        }))
    }

    pub fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.0.borrow_mut().add_fonts(fonts)
    }

    pub fn all_font_names(&self) -> Vec<String> {
        let mut names = FAMILIES.clone();
        if let Ok(fonts_in_memory) = self.0.borrow().memory_source.all_families() {
            names.extend(fonts_in_memory);
        }
        names
    }

    pub fn font_id(&self, font: &Font) -> Result<FontId> {
        let mut state = self.0.borrow_mut();
        if let Some(font_id) = state.font_selections.get(font) {
            return Ok(*font_id);
        }
        let font_key = FontKey {
            font_family: font.family.clone(),
            font_features: font.features.clone(),
            font_fallbacks: font.fallbacks.clone(),
        };
        let candidates = match state.font_ids_by_font_key.get(&font_key) {
            Some(font_ids) => font_ids.clone(),
            None => {
                let font_ids =
                    state.load_family(&font.family, &font.features, font.fallbacks.as_ref())?;
                state
                    .font_ids_by_font_key
                    .insert(font_key, font_ids.clone());
                font_ids
            }
        };
        // font-kit maps Medium (0.23) to weight 530, so 500 would pick Regular. Map Apple's weight
        // constants to CSS weights 100 to 900 instead, as browsers do.
        let properties = candidates
            .iter()
            .map(|font_id| {
                let font = &state.fonts[font_id.0];
                let mut properties = font.properties();
                if let Some(weight) = font
                    .native_font()
                    .all_traits()
                    .get(unsafe { kCTFontWeightTrait })
                    .downcast::<CFNumber>()
                    .and_then(|weight| weight.to_f32())
                {
                    let knots = [-0.8, -0.6, -0.4, 0., 0.23, 0.3, 0.4, 0.56, 0.62];
                    let index = knots.iter().rposition(|&knot| weight >= knot).unwrap_or(0);
                    let next = knots.get(index + 1).filter(|_| weight >= knots[0]);
                    let fraction =
                        next.map_or(0., |next| (weight - knots[index]) / (next - knots[index]));
                    properties.weight =
                        font_kit::properties::Weight(100. * (index as f32 + 1. + fraction));
                }
                properties
            })
            .collect::<SmallVec<[_; 4]>>();
        let index = font_kit::matching::find_best_match(
            &properties,
            &font_kit::properties::Properties {
                style: fontkit_style(font.style),
                weight: fontkit_weight(font.weight),
                stretch: Default::default(),
            },
        )?;
        let mut font_id = candidates[index];
        // Like browsers, a variable font's weight axis takes the exact CSS weight, which differs from
        // its named faces for weights like 500 and 600. A face at its default instance has no `wght`
        // in its variation, so the axis default stands in.
        // WARNING: old Apple fonts like Skia use a 0.5 to 3.2 `wght` scale, so they keep their faces.
        let native = state.fonts[font_id.0].native_font();
        let default = native
            .get_variation_axes()
            .and_then(|axes| {
                axes.iter().find_map(|axis| {
                    let number = |key| axis.find(key)?.downcast::<CFNumber>();
                    (number(unsafe { kCTFontVariationAxisIdentifierKey })?.to_i64()? == WGHT)
                        .then(|| number(unsafe { kCTFontVariationAxisDefaultValueKey })?.to_f32())?
                })
            })
            .filter(|weight| *weight >= 100.);
        if default.is_some() && variation_weight(&native).or(default) != Some(font.weight.0) {
            font_id = state.weighted(font_id, font.weight.0);
        }
        state.font_selections.insert(font.clone(), font_id);
        Ok(font_id)
    }

    pub fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        let state = self.0.borrow();
        let font = &state.fonts[font_id.0];
        FontMetrics {
            tall_ascent: matches!(
                font.family_name().as_str(),
                "Times" | "Helvetica" | "Courier"
            ),
            ..font_kit_metrics_to_metrics(font.metrics())
        }
    }

    pub fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        self.0.borrow().advance(font_id, glyph_id)
    }

    pub fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.0.borrow().glyph_for_char(font_id, ch)
    }

    pub fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        self.0.borrow().raster_bounds(params)
    }

    pub fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        self.0.borrow_mut().rasterize_glyph(params, raster_bounds)
    }

    pub fn layout_line(&self, text: &str, font_size: Pixels, font_runs: &[FontRun]) -> LineLayout {
        self.0.borrow_mut().layout_line(text, font_size, font_runs)
    }

    /// With font smoothing on, CoreGraphics thickens strokes by an amount that depends on the
    /// text color's luminance. This picks the same level.
    pub fn glyph_dilation_for_color(&self, color: Rgba) -> u8 {
        if !font_smoothing_allowed_by_user() {
            return 0;
        }
        let luminance = 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
        (((4.0 * luminance) + 0.5).floor() as i32).clamp(0, 4) as u8
    }
}

fn font_smoothing_allowed_by_user() -> bool {
    static ALLOWED: OnceLock<bool> = OnceLock::new();
    *ALLOWED.get_or_init(|| {
        use core_foundation_sys::preferences::{
            CFPreferencesCopyAppValue, kCFPreferencesCurrentApplication,
        };

        let key = CFString::new("AppleFontSmoothing");
        let value_ref = unsafe {
            CFPreferencesCopyAppValue(key.as_concrete_TypeRef(), kCFPreferencesCurrentApplication)
        };
        if value_ref.is_null() {
            return true;
        }
        let value = unsafe { CFType::wrap_under_create_rule(value_ref) };
        let Some(number) = value.downcast_into::<CFNumber>() else {
            return true;
        };
        // Only an explicit value of `0` means that font smoothing is disabled.
        number.to_i64() != Some(0)
    })
}

impl AppleTextSystemState {
    fn add_fonts(&mut self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        let fonts = fonts
            .into_iter()
            .map(|bytes| match bytes {
                Cow::Borrowed(embedded_font) => {
                    let data_provider = unsafe {
                        core_graphics::data_provider::CGDataProvider::from_slice(embedded_font)
                    };
                    let font = core_graphics::font::CGFont::from_data_provider(data_provider)
                        .map_err(|()| anyhow!("Could not load an embedded font."))?;
                    let font = font_kit::loaders::core_text::Font::from_core_graphics_font(font);
                    Ok(Handle::from_native(&font))
                }
                Cow::Owned(bytes) => Ok(Handle::from_memory(Arc::new(bytes), 0)),
            })
            .collect::<Result<Vec<_>>>()?;
        self.memory_source.add_fonts(fonts.into_iter())?;
        // Families resolved before, maybe to system fonts, must be resolved again.
        self.font_selections.clear();
        self.font_ids_by_font_key.clear();
        Ok(())
    }

    fn load_family(
        &mut self,
        name: &str,
        features: &FontFeatures,
        fallbacks: Option<&FontFallbacks>,
    ) -> Result<SmallVec<[FontId; 4]>> {
        let name = if name == ".SystemUIFont" {
            ".AppleSystemUIFont"
        } else {
            name
        };

        let mut font_ids = SmallVec::new();
        let mut postscript_names_seen = HashSet::default();
        let family = self
            .memory_source
            .select_family_by_name(name)
            .or_else(|_| self.system_source.select_family_by_name(name))?;
        for font in family.fonts() {
            let mut font = font.load()?;

            apply_features_and_fallbacks(&mut font, features, fallbacks)?;

            // WARNING: `font.properties()` panics on fonts whose traits are not CFNumbers.
            let traits = font.native_font().all_traits();
            if unsafe {
                !(traits
                    .get(kCTFontSymbolicTrait)
                    .downcast::<CFNumber>()
                    .is_some()
                    && traits
                        .get(kCTFontWidthTrait)
                        .downcast::<CFNumber>()
                        .is_some()
                    && traits
                        .get(kCTFontWeightTrait)
                        .downcast::<CFNumber>()
                        .is_some()
                    && traits
                        .get(kCTFontSlantTrait)
                        .downcast::<CFNumber>()
                        .is_some())
            } {
                continue;
            }

            let Some(postscript_name) = font.postscript_name() else {
                continue;
            };
            // Dedup only within this call: a family can load again under another `FontKey`, and
            // a check against `font_ids_by_postscript_name` would leave that call empty.
            if !postscript_names_seen.insert(postscript_name.clone()) {
                continue;
            }
            let font_id = FontId(self.fonts.len());
            font_ids.push(font_id);
            self.font_ids_by_postscript_name
                .insert(postscript_name.clone(), font_id);
            self.postscript_names_by_font_id
                .insert(font_id, postscript_name);
            self.fonts.push(font);
        }
        Ok(font_ids)
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        Ok(size_from_vector2f(
            self.fonts[font_id.0].advance(glyph_id.0)?,
        ))
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.fonts[font_id.0].glyph_for_char(ch).map(GlyphId)
    }

    /// The face at `size`, created once.
    // WARNING: every new copy of a variable face, like the system font, builds its CoreText caches
    // again, which costs most of the time of shaping a line.
    fn sized(&mut self, font_id: FontId, size: f32) -> CTFont {
        if self.sized.len() >= 512 {
            self.sized.clear();
        }
        let fonts = &self.fonts;
        self.sized
            .entry((font_id, size.to_bits()))
            .or_insert_with(|| {
                fonts[font_id.0]
                    .native_font()
                    .clone_with_font_size(size as CGFloat)
            })
            .clone()
    }

    /// The face with its `wght` axis at `weight`, created once.
    fn weighted(&mut self, font_id: FontId, weight: f32) -> FontId {
        let base = self.fonts[font_id.0].native_font();
        let variations = CFDictionary::from_CFType_pairs(&[(
            CFNumber::from(WGHT).as_CFType(),
            CFNumber::from(f64::from(weight)).as_CFType(),
        )]);
        let attributes = CFDictionary::from_CFType_pairs(&[(
            unsafe { CFString::wrap_under_get_rule(kCTFontVariationAttribute) },
            variations.as_CFType(),
        )]);
        let descriptor = core_text::font_descriptor::new_from_attributes(&attributes);
        let font = unsafe {
            CTFont::wrap_under_create_rule(CTFontCreateCopyWithAttributes(
                base.as_concrete_TypeRef(),
                base.pt_size(),
                std::ptr::null(),
                descriptor.as_concrete_TypeRef(),
            ))
        };
        let Some(name) = variation_weight(&font)
            .map(|weight| format!("{}#wght{weight}", font.postscript_name()))
        else {
            return font_id;
        };
        if let Some(&id) = self.font_ids_by_postscript_name.get(&name) {
            return id;
        }
        let id = FontId(self.fonts.len());
        self.font_ids_by_postscript_name.insert(name.clone(), id);
        self.postscript_names_by_font_id.insert(id, name);
        self.fonts
            .push(unsafe { font_kit::font::Font::from_native_font(&font) });
        id
    }

    fn id_for_native_font(&mut self, requested_font: CTFont) -> FontId {
        let postscript_name = requested_font.postscript_name();
        if let Some(font_id) = variation_weight(&requested_font)
            .and_then(|weight| {
                self.font_ids_by_postscript_name
                    .get(&format!("{postscript_name}#wght{weight}"))
            })
            .or_else(|| self.font_ids_by_postscript_name.get(&postscript_name))
        {
            *font_id
        } else {
            let font_id = FontId(self.fonts.len());
            self.font_ids_by_postscript_name
                .insert(postscript_name.clone(), font_id);
            self.postscript_names_by_font_id
                .insert(font_id, postscript_name);
            self.fonts
                .push(font_kit::font::Font::from_core_graphics_font(
                    requested_font.copy_to_CGFont(),
                ));
            font_id
        }
    }

    fn is_emoji(&self, font_id: FontId) -> bool {
        self.postscript_names_by_font_id
            .get(&font_id)
            .is_some_and(|postscript_name| {
                postscript_name == "AppleColorEmoji" || postscript_name == ".AppleColorEmojiUI"
            })
    }

    fn raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        let font = &self.fonts[params.font_id.0];
        let scale = Transform2F::from_scale(params.scale_factor);
        let bounds: Bounds<DevicePixels> = bounds_from_rect_i(font.raster_bounds(
            params.glyph_id.0,
            params.font_size.into(),
            scale,
            HintingOptions::None,
            font_kit::canvas::RasterizationOptions::GrayscaleAa,
        )?);

        // 1 pixel of room on each side for CoreGraphics anti-aliasing.
        Ok(bounds.dilate(DevicePixels(1)))
    }

    fn rasterize_glyph(
        &mut self,
        params: &RenderGlyphParams,
        glyph_bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        if glyph_bounds.size.width.0 == 0 || glyph_bounds.size.height.0 == 0 {
            anyhow::bail!("glyph bounds are empty");
        } else {
            // A subpixel offset needs 1 more pixel for anti-aliasing.
            let mut bitmap_size = glyph_bounds.size;
            if params.subpixel_variant.x > 0 {
                bitmap_size.width += DevicePixels(1);
            }
            if params.subpixel_variant.y > 0 {
                bitmap_size.height += DevicePixels(1);
            }
            let bitmap_size = bitmap_size;

            let mut bytes;

            let cx = if params.is_emoji {
                bytes = vec![0; bitmap_size.width.0 as usize * 4 * bitmap_size.height.0 as usize];
                CGContext::create_bitmap_context(
                    Some(bytes.as_mut_ptr() as *mut _),
                    bitmap_size.width.0 as usize,
                    bitmap_size.height.0 as usize,
                    8,
                    bitmap_size.width.0 as usize * 4,
                    &CGColorSpace::create_device_rgb(),
                    kCGImageAlphaPremultipliedLast,
                )
            } else {
                bytes = vec![0; bitmap_size.width.0 as usize * bitmap_size.height.0 as usize];
                CGContext::create_bitmap_context(
                    Some(bytes.as_mut_ptr() as *mut _),
                    bitmap_size.width.0 as usize,
                    bitmap_size.height.0 as usize,
                    8,
                    bitmap_size.width.0 as usize,
                    &CGColorSpace::create_device_gray(),
                    kCGImageAlphaOnly,
                )
            };

            // Bottom-left origin and scale, to match font-kit's `raster_bounds`.
            cx.translate(
                -glyph_bounds.origin.x.0 as CGFloat,
                (glyph_bounds.origin.y.0 + glyph_bounds.size.height.0) as CGFloat,
            );
            cx.scale(
                params.scale_factor as CGFloat,
                params.scale_factor as CGFloat,
            );

            let subpixel_shift = params
                .subpixel_variant
                .map(|v| v as f32 / SUBPIXEL_VARIANTS_X as f32);
            cx.set_text_drawing_mode(CGTextDrawingMode::CGTextFill);
            cx.set_allows_antialiasing(true);
            cx.set_should_antialias(true);
            cx.set_allows_font_subpixel_positioning(true);
            cx.set_should_subpixel_position_fonts(true);
            cx.set_allows_font_subpixel_quantization(false);
            cx.set_should_subpixel_quantize_fonts(false);

            if params.dilation > 0 {
                let luminance = params.dilation as f64 * 0.25;
                cx.set_should_smooth_fonts(true);
                cx.set_gray_fill_color(luminance, 1.0);
            } else {
                cx.set_gray_fill_color(0.0, 1.0);
            }
            self.sized(params.font_id, f32::from(params.font_size))
                .draw_glyphs(
                    &[params.glyph_id.0 as CGGlyph],
                    &[CGPoint::new(
                        (subpixel_shift.x / params.scale_factor) as CGFloat,
                        (subpixel_shift.y / params.scale_factor) as CGFloat,
                    )],
                    cx,
                );

            if params.is_emoji {
                // Convert from RGBA with premultiplied alpha to BGRA with straight alpha.
                for pixel in bytes.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                    let a = pixel[3] as f32 / 255.;
                    if a > 0. {
                        for channel in &mut pixel[..3] {
                            *channel = (*channel as f32 / a).min(255.) as u8;
                        }
                    }
                }
            }

            Ok((bitmap_size, bytes))
        }
    }

    fn layout_line(&mut self, text: &str, font_size: Pixels, font_runs: &[FontRun]) -> LineLayout {
        let mut string = CFMutableAttributedString::new();
        let mut max_ascent = 0.0f32;
        let mut max_descent = 0.0f32;

        {
            let mut text = text;
            // CoreText merges adjacent runs that end up in equal fonts, including runs of
            // different fonts that fall back to the same one, so every other run that inserts
            // text is set a hair larger to keep ligatures from forming across them. The first
            // run keeps its exact size: SF tracks by its exact point size, and a nudged size
            // draws wider or narrower than AppKit sets the same text.
            let mut break_ligature = false;
            for run in font_runs {
                let text_run;
                (text_run, text) = text.split_at(run.len);

                let utf16_start = string.char_len(); // insert at end of string
                // WARNING: `replace_str` can drop some code points, like a BOM at the start.
                string.replace_str(&CFString::new(text_run), CFRange::init(utf16_start, 0));
                let utf16_end = string.char_len();

                let length = utf16_end - utf16_start;
                let cf_range = CFRange::init(utf16_start, length);
                let font_metrics = self.fonts[run.font_id.0].metrics();
                let font_scale = f32::from(run.font_size) / font_metrics.units_per_em as f32;
                max_ascent = max_ascent.max(font_metrics.ascent * font_scale);
                max_descent = max_descent.max(-font_metrics.descent * font_scale);

                let font_size = if break_ligature {
                    px(f32::from(run.font_size).next_up())
                } else {
                    run.font_size
                };
                let font = self.sized(run.font_id, font_size.into());
                unsafe {
                    string.set_attribute(cf_range, kCTFontAttributeName, &font);
                }
                if length > 0 {
                    break_ligature = !break_ligature;
                }
            }
        }
        let line = CTLine::new_with_attributed_string(string.as_concrete_TypeRef());
        let glyph_runs = line.glyph_runs();
        let mut runs = <Vec<ShapedRun>>::with_capacity(glyph_runs.len() as usize);
        let mut ix_converter = StringIndexConverter::new(text);
        for run in glyph_runs.into_iter() {
            let attributes = run.attributes().unwrap();
            let font = unsafe {
                attributes
                    .get(kCTFontAttributeName)
                    .downcast::<CTFont>()
                    .unwrap()
            };
            // Undo the nudge that keeps ligatures from forming across runs.
            let shaped_size = font.pt_size() as f32;
            let run_size = font_runs
                .iter()
                .map(|run| run.font_size)
                .min_by(|a, b| {
                    (f32::from(*a) - shaped_size)
                        .abs()
                        .total_cmp(&(f32::from(*b) - shaped_size).abs())
                })
                .unwrap_or(font_size);
            if self.natives.len() >= 512 {
                self.natives.clear();
            }
            let key = font.as_concrete_TypeRef() as usize;
            let font_id = match self.natives.get(&key) {
                Some((_, font_id)) => *font_id,
                None => {
                    let font_id = self.id_for_native_font(font.clone());
                    self.natives.insert(key, (font, font_id));
                    font_id
                }
            };
            let is_emoji = self.is_emoji(font_id);

            let glyphs = match runs.last_mut() {
                Some(run) if run.font_id == font_id && run.font_size == run_size => &mut run.glyphs,
                _ => {
                    runs.push(ShapedRun {
                        font_id,
                        font_size: run_size,
                        glyphs: Vec::with_capacity(run.glyph_count().try_into().unwrap_or(0)),
                    });
                    &mut runs.last_mut().unwrap().glyphs
                }
            };
            for ((&glyph_id, position), &glyph_utf16_ix) in run
                .glyphs()
                .iter()
                .zip(run.positions().iter())
                .zip(run.string_indices().iter())
            {
                ix_converter.seek_to_utf16_ix(usize::try_from(glyph_utf16_ix).unwrap());
                glyphs.push(ShapedGlyph {
                    id: GlyphId(glyph_id as u32),
                    position: point(position.x as f32, position.y as f32).map(px),
                    index: ix_converter.utf8_ix,
                    is_emoji,
                });
            }
        }
        let typographic_bounds = line.get_typographic_bounds();
        LineLayout {
            runs,
            font_size,
            width: typographic_bounds.width.into(),
            ascent: max_ascent.into(),
            descent: max_descent.into(),
            len: text.len(),
        }
    }
}

#[derive(Debug, Clone)]
struct StringIndexConverter<'a> {
    text: &'a str,
    utf8_ix: usize,
    utf16_ix: usize,
}

impl<'a> StringIndexConverter<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            utf8_ix: 0,
            utf16_ix: 0,
        }
    }

    /// Moves forward or back to the first character at or after `utf16_target`.
    fn seek_to_utf16_ix(&mut self, utf16_target: usize) {
        while self.utf16_ix > utf16_target
            && let Some(c) = self.text[..self.utf8_ix].chars().next_back()
        {
            self.utf8_ix -= c.len_utf8();
            self.utf16_ix -= c.len_utf16();
        }
        for (ix, c) in self.text[self.utf8_ix..].char_indices() {
            if self.utf16_ix >= utf16_target {
                self.utf8_ix += ix;
                return;
            }
            self.utf16_ix += c.len_utf16();
        }
        self.utf8_ix = self.text.len();
    }
}

fn font_kit_metrics_to_metrics(metrics: Metrics) -> FontMetrics {
    FontMetrics {
        units_per_em: metrics.units_per_em,
        ascent: metrics.ascent,
        descent: metrics.descent,
        line_gap: metrics.line_gap,
        cap_height: metrics.cap_height,
        x_height: metrics.x_height,
        tall_ascent: false,
        bounding_box: bounds_from_rect(metrics.bounding_box),
    }
}

fn bounds_from_rect(rect: RectF) -> Bounds<f32> {
    Bounds {
        origin: point(rect.origin_x(), rect.origin_y()),
        size: size(rect.width(), rect.height()),
    }
}

fn bounds_from_rect_i(rect: RectI) -> Bounds<DevicePixels> {
    Bounds {
        origin: point(DevicePixels(rect.origin_x()), DevicePixels(rect.origin_y())),
        size: size(DevicePixels(rect.width()), DevicePixels(rect.height())),
    }
}

fn size_from_vector2f(vec: Vector2F) -> Size<f32> {
    size(vec.x(), vec.y())
}

fn fontkit_weight(value: FontWeight) -> FontkitWeight {
    FontkitWeight(value.0)
}

fn fontkit_style(style: FontStyle) -> FontkitStyle {
    match style {
        FontStyle::Normal => FontkitStyle::Normal,
        FontStyle::Italic => FontkitStyle::Italic,
        FontStyle::Oblique => FontkitStyle::Oblique,
    }
}
