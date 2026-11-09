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
    geometry::{CGPoint, CGRect, CGSize},
};
use core_text::{
    font::CTFont,
    font_descriptor::{
        kCTFontOrientationDefault, kCTFontSlantTrait, kCTFontSymbolicTrait, kCTFontWeightTrait,
        kCTFontWidthTrait,
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
use std::{borrow::Cow, char, convert::TryFrom, mem, rc::Rc, sync::Arc};

use core_foundation::base::CFType;
use std::sync::OnceLock;

use super::open_type::apply_features_and_fallbacks;

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
#[allow(non_upper_case_globals)]
const kCGImageAlphaNoneSkipFirst: u32 = 6;
#[allow(non_upper_case_globals)]
const kCGBitmapByteOrder32Little: u32 = 2 << 12;

pub(crate) struct TextSystem(RefCell<State>);

#[derive(Clone, PartialEq, Eq, Hash)]
struct FontKey {
    font_family: SharedString,
    font_features: FontFeatures,
    font_fallbacks: Option<FontFallbacks>,
}

struct State {
    memory_source: MemSource,
    system_source: SystemSource,
    fonts: Vec<FontKitFont>,
    font_selections: HashMap<Font, FontId>,
    font_ids_by_postscript_name: HashMap<String, FontId>,
    font_ids_by_font_key: HashMap<FontKey, SmallVec<[FontId; 4]>>,
    postscript_names_by_font_id: HashMap<FontId, String>,
    // WARNING: the fonts from memory, which the process caches leave out.
    local: HashSet<FontId>,
    sized: HashMap<(FontId, u32), CTFont>,
    // WARNING: the font of each shaped run's face, kept alive so its address stays unique.
    natives: HashMap<usize, (CTFont, FontId)>,
    // WARNING: code repeats its words and indents across lines, so lines in a monospace font shape by runs of
    // spaces and non-spaces, each once per font and size. Monospace fonts don't kern, so the line is the same.
    segments: HashMap<(FontId, u32), HashMap<Box<str>, Rc<LineLayout>>>,
    monospace: HashMap<FontId, bool>,
    // WARNING: other lines of simple scripts shape by words and runs of spaces too, each once per font, size and
    // whether a space comes before and after it, shaped next to those spaces. That holds while no character after a
    // space kerns with it, which `Face::spaced` learns per character. Words keep each x in f64, so a line of them
    // rounds like the whole line.
    faces: HashMap<(FontId, u32), Face>,
    words: usize,
    parts: Vec<Part>,
    metrics: HashMap<FontId, Metrics>,
}

#[derive(Default)]
struct Face {
    words: HashMap<Box<str>, [Option<Rc<Word>>; 4]>,
    spaced: HashMap<char, bool>,
    misses: u32,
    numbers: Option<Box<Numbers>>,
}

// WARNING: after some missing words of digits, a font and size learn each digit's glyph and its advance before every
// digit or space, from one line that holds each three of them in a row once. A digit word then shapes without
// CoreText while the triples in it, and the pairs at its ends, which shape alone once, match those advances.
struct Numbers {
    font: FontId,
    glyphs: [GlyphId; 11],
    ends: [f64; 11],
    advances: [[f64; 11]; 11],
    triples: Box<[bool]>,
    edges: [[u8; 11]; 11],
}

// WARNING: a word or a run of spaces: its run, shaped size, range in the line, which sides have a space, and shape.
type Part = (
    usize,
    Pixels,
    std::ops::Range<usize>,
    usize,
    Option<Rc<Word>>,
);

// WARNING: each glyph's x in f64, as CoreText places it, then the width, and where each CoreText run starts in the
// text and on the line.
type Exact = (Vec<f64>, Vec<(usize, f64)>);

// WARNING: each glyph's x, font, id, y and byte offset in the word, then the word's width.
type Glyphs = Box<[(f64, FontId, GlyphId, f32, usize)]>;

struct Word(Glyphs, f64);

static FAMILIES: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
    unsafe extern "C" {
        fn CTFontManagerCopyAvailableFontFamilyNames() -> CFArrayRef;
    }
    // SAFETY: the function follows the Create Rule and returns an array of strings.
    let families: CFArray<CFString> =
        unsafe { CFArray::wrap_under_create_rule(CTFontManagerCopyAvailableFontFamilyNames()) };
    // WARNING: macOS leaves these out of the list, though it has them and browsers find them.
    families
        .iter()
        .map(|name| name.to_string())
        .chain(["Times".into(), "Courier".into()])
        .collect()
});

// WARNING: finding a system family's faces, a weight of a variable face, or its axes (which CoreText copies with
// their localized names) takes milliseconds, the same in every window, so system faces by family, features and
// fallbacks, their weights and default weights, and the names that have none, are kept for the process. Their
// fonts stay alive here, so a face's address names it. Fonts from memory belong to their text system.
// WARNING: each entry is found once, outside the map's lock, so threads that look up other entries meanwhile don't wait.
type Shared<K, V> = std::sync::LazyLock<std::sync::Mutex<HashMap<K, Arc<OnceLock<V>>>>>;
type Faces = Option<Vec<(FontKitFont, String)>>;
type SystemKey = (String, Vec<(String, u32)>, Option<Vec<String>>);
static SYSTEM_FAMILIES: Shared<SystemKey, Faces> = std::sync::LazyLock::new(Default::default);
static AXES: Shared<usize, Option<f32>> = std::sync::LazyLock::new(Default::default);
static WEIGHTS: Shared<(usize, u32), Option<(FontKitFont, String)>> =
    std::sync::LazyLock::new(Default::default);

fn shared<K: std::hash::Hash + Eq, V: Clone>(
    map: &Shared<K, V>,
    key: K,
    find: impl FnOnce() -> V,
) -> V {
    let entry = map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(key)
        .or_default()
        .clone();
    entry.get_or_init(find).clone()
}

impl TextSystem {
    pub fn new() -> Self {
        // WARNING: listing the font families takes about 20 ms, so it starts here, off the first frame's path.
        static LISTING: std::sync::Once = std::sync::Once::new();
        LISTING.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("font families".into())
                .spawn(|| std::sync::LazyLock::force(&FAMILIES).len());
        });
        Self(RefCell::new(State {
            memory_source: MemSource::empty(),
            system_source: SystemSource::new(),
            fonts: Vec::new(),
            font_selections: HashMap::default(),
            font_ids_by_postscript_name: HashMap::default(),
            font_ids_by_font_key: HashMap::default(),
            postscript_names_by_font_id: HashMap::default(),
            local: HashSet::default(),
            sized: HashMap::default(),
            natives: HashMap::default(),
            segments: HashMap::default(),
            monospace: HashMap::default(),
            faces: HashMap::default(),
            words: 0,
            parts: Vec::new(),
            metrics: HashMap::default(),
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
        // WARNING: font-kit maps Medium (0.23) to weight 530, so 500 would pick Regular. Map Apple's weight
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
                style: font_kit_style(font.style),
                weight: font_kit_weight(font.weight),
                stretch: Default::default(),
            },
        )?;
        let mut font_id = candidates[index];
        // Like browsers, a variable font's weight axis takes the exact CSS weight, which differs from
        // its named faces for weights like 500 and 600. A face at its default instance has no `wght`
        // in its variation, so the axis default stands in.
        // WARNING: old Apple fonts like Skia use a 0.5 to 3.2 `wght` scale, so they keep their faces.
        let native = state.fonts[font_id.0].native_font();
        let axis = || {
            native
                .get_variation_axes()
                .and_then(|axes| {
                    axes.iter().find_map(|axis| {
                        let number = |key| axis.find(key)?.downcast::<CFNumber>();
                        (number(unsafe { kCTFontVariationAxisIdentifierKey })?.to_i64()? == WGHT)
                            .then(|| {
                                number(unsafe { kCTFontVariationAxisDefaultValueKey })?.to_f32()
                            })?
                    })
                })
                .filter(|weight| *weight >= 100.)
        };
        let default = match state.local.contains(&font_id) {
            true => axis(),
            false => shared(&AXES, native.as_concrete_TypeRef() as usize, axis),
        };
        if default.is_some() && variation_weight(&native).or(default) != Some(font.weight.0) {
            font_id = state.weighted(font_id, font.weight.0);
        }
        state.font_selections.insert(font.clone(), font_id);
        Ok(font_id)
    }

    pub fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        let state = self.0.borrow();
        let font = &state.fonts[font_id.0];
        let metrics = font_kit_metrics(font.metrics());
        let typo = font
            .load_font_table(u32::from_be_bytes(*b"OS/2"))
            .filter(|table| table.len() >= 72)
            .map(|table| {
                [68, 70].map(|at| f32::from(i16::from_be_bytes([table[at], table[at + 1]])))
            })
            .filter(|[ascent, descent]| *ascent > 0. && *descent < 0.);
        FontMetrics {
            tall_ascent: matches!(
                font.family_name().as_str(),
                "Times" | "Helvetica" | "Courier"
            ),
            em_ascent: typo.map_or(metrics.em_ascent, |[ascent, descent]| {
                ascent / (ascent - descent)
            }),
            ..metrics
        }
    }

    pub fn glyph_bounds(
        &self,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: f32,
    ) -> Option<[f32; 4]> {
        let state = self.0.borrow();
        let font = &state.fonts[font_id.0];
        let bounds = font.typographic_bounds(glyph_id.0).ok()?;
        let scale = font_size / font.metrics().units_per_em as f32;
        Some([
            bounds.min_x() * scale,
            bounds.max_x() * scale,
            bounds.max_y() * scale,
            bounds.min_y() * scale,
        ])
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
        number.to_i64() != Some(0)
    })
}

impl State {
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
        // WARNING: families resolved before, maybe to system fonts, must be resolved again.
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
        let load = |family: font_kit::family_handle::FamilyHandle| {
            let mut faces = Vec::new();
            let mut postscript_names_seen = HashSet::default();
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
                // WARNING: dedup only within this call: a family can load again under another `FontKey`, and
                // a check against `font_ids_by_postscript_name` would leave that call empty.
                if postscript_names_seen.insert(postscript_name.clone()) {
                    faces.push((font, postscript_name));
                }
            }
            anyhow::Ok(faces)
        };
        let (faces, local) = match self.memory_source.select_family_by_name(name) {
            Ok(family) => (load(family)?, true),
            Err(_) => {
                let key = (
                    name.to_owned(),
                    features.0.to_vec(),
                    fallbacks.map(|fallbacks| fallbacks.fallback_list().to_vec()),
                );
                let faces = shared(&SYSTEM_FAMILIES, key, || {
                    load(self.system_source.select_family_by_name(name).ok()?).ok()
                });
                (
                    faces.ok_or_else(|| anyhow!("no font family {name}"))?,
                    false,
                )
            }
        };
        for (font, postscript_name) in faces {
            let font_id = FontId(self.fonts.len());
            if local {
                self.local.insert(font_id);
            }
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

    fn weighted(&mut self, font_id: FontId, weight: f32) -> FontId {
        let base = self.fonts[font_id.0].native_font();
        let make = || {
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
            let name = variation_weight(&font)
                .map(|weight| format!("{}#wght{weight}", font.postscript_name()))?;
            Some((
                unsafe { font_kit::font::Font::from_native_font(&font) },
                name,
            ))
        };
        let weighted = match self.local.contains(&font_id) {
            true => make(),
            false => shared(
                &WEIGHTS,
                (base.as_concrete_TypeRef() as usize, weight.to_bits()),
                make,
            ),
        };
        let Some((font, name)) = weighted else {
            return font_id;
        };
        if let Some(&id) = self.font_ids_by_postscript_name.get(&name) {
            return id;
        }
        let id = FontId(self.fonts.len());
        if self.local.contains(&font_id) {
            self.local.insert(id);
        }
        self.font_ids_by_postscript_name.insert(name.clone(), id);
        self.postscript_names_by_font_id.insert(id, name);
        self.fonts.push(font);
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
        let bounds: Bounds<DevicePixels> = bounds_from_rect_i32(font.raster_bounds(
            params.glyph_id.0,
            params.font_size.into(),
            scale,
            HintingOptions::None,
            font_kit::canvas::RasterizationOptions::GrayscaleAa,
        )?);

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
            let mut bitmap_size = glyph_bounds.size;
            if params.subpixel_variant.x > 0 {
                bitmap_size.width += DevicePixels(1);
            }
            if params.subpixel_variant.y > 0 {
                bitmap_size.height += DevicePixels(1);
            }
            let bitmap_size = bitmap_size;

            let mut bytes;

            // WARNING: an alpha-only context makes glyphs about 17% heavier than an opaque one. Like Skia, antialiased
            // text draws black on white in an opaque context, whose inverse is the mask.
            let opaque_mask = !params.is_emoji && params.dilation == 0;
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
            } else if opaque_mask {
                bytes = vec![0; bitmap_size.width.0 as usize * 4 * bitmap_size.height.0 as usize];
                CGContext::create_bitmap_context(
                    Some(bytes.as_mut_ptr() as *mut _),
                    bitmap_size.width.0 as usize,
                    bitmap_size.height.0 as usize,
                    8,
                    bitmap_size.width.0 as usize * 4,
                    &CGColorSpace::create_device_rgb(),
                    kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little,
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

            if opaque_mask {
                cx.set_gray_fill_color(1.0, 1.0);
                cx.fill_rect(CGRect::new(
                    &CGPoint::new(0., 0.),
                    &CGSize::new(
                        bitmap_size.width.0 as CGFloat,
                        bitmap_size.height.0 as CGFloat,
                    ),
                ));
                cx.set_allows_font_smoothing(false);
                cx.set_should_smooth_fonts(false);
            }

            cx.translate(
                -glyph_bounds.origin.x.0 as CGFloat,
                (glyph_bounds.origin.y.0 + glyph_bounds.size.height.0) as CGFloat,
            );
            // WARNING: CoreText draws emoji larger under about 24 points, while Chromium draws them at device size.
            // So they draw at twice the size or more, and they stay in proportion to the text at any scale.
            let oversize = if params.is_emoji {
                params.scale_factor.max(2.)
            } else {
                1.
            };
            let drawn = (params.scale_factor / oversize) as CGFloat;
            cx.scale(drawn, drawn);

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
            self.sized(params.font_id, f32::from(params.font_size) * oversize)
                .draw_glyphs(
                    &[params.glyph_id.0 as CGGlyph],
                    &[CGPoint::new(
                        (subpixel_shift.x / params.scale_factor) as CGFloat,
                        (subpixel_shift.y / params.scale_factor) as CGFloat,
                    )],
                    cx,
                );

            if opaque_mask {
                bytes = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| 255 - pixel[1])
                    .collect();
            }
            if params.is_emoji {
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
        if text.is_ascii() && !text.contains(['\t', '\n', '\r']) && !font_runs.is_empty() {
            let fonts = &self.fonts;
            let monospace = font_runs.iter().all(|run| {
                *self
                    .monospace
                    .entry(run.font_id)
                    .or_insert_with(|| fonts[run.font_id.0].is_monospace())
            });
            if monospace {
                return self.layout_segments(text, font_size, font_runs);
            }
        }
        let simple = |ch: char| {
            matches!(ch, ' '..='~' | '\u{a0}'..='\u{58f}' | '\u{1e00}'..='\u{1fff}' | '\u{2010}'..='\u{2027}'
                | '\u{2030}'..='\u{205e}' | '\u{20a0}'..='\u{20cf}')
        };
        if !font_runs.is_empty()
            && text.chars().all(simple)
            && let Some(layout) = self.layout_words(text, font_size, font_runs)
        {
            #[cfg(debug_assertions)]
            assert!(
                same_lines(
                    &self.layout_whole(text, font_size, font_runs, None),
                    &layout
                ),
                "words of {text:?} shape unlike the whole line"
            );
            return layout;
        }
        self.layout_whole(text, font_size, font_runs, None)
    }

    fn layout_words(
        &mut self,
        text: &str,
        font_size: Pixels,
        font_runs: &[FontRun],
    ) -> Option<LineLayout> {
        if self.words > 16384 {
            self.faces.clear();
            self.words = 0;
        }
        let mut parts = mem::take(&mut self.parts);
        parts.clear();
        let (mut start, mut nudged, mut missing) = (0, false, 0);
        for (index, run) in font_runs.iter().enumerate() {
            // WARNING: like `layout_whole`, every other run shapes a hair larger.
            let size = if nudged {
                px(f32::from(run.font_size).next_up())
            } else {
                run.font_size
            };
            let part = &text[start..start + run.len];
            let key = (run.font_id, f32::from(size).to_bits());
            let mut offset = 0;
            while offset < part.len() {
                let space = part.as_bytes()[offset] == b' ';
                let end = part[offset..]
                    .bytes()
                    .position(|byte| (byte == b' ') != space)
                    .map_or(part.len(), |length| offset + length);
                let around = (offset > 0) as usize | ((end < part.len()) as usize) << 1;
                parts.push((index, size, start + offset..start + end, around, None));
                offset = end;
            }
            let first = parts.len()
                - parts
                    .iter()
                    .rev()
                    .take_while(|part| part.0 == index)
                    .count();
            let mut face = self.faces.get(&key);
            for at in first..parts.len() {
                let (range, around) = (parts[at].2.clone(), parts[at].3);
                if text.as_bytes()[range.start] == b' '
                    && let Some(next) = text[range.end..].chars().next().filter(|_| around & 2 != 0)
                {
                    let spaced = match face.and_then(|face| face.spaced.get(&next)) {
                        Some(&spaced) => spaced,
                        None => {
                            let spaced = self.spaced(run.font_id, size, next);
                            face = self.faces.get(&key);
                            spaced
                        }
                    };
                    if !spaced {
                        self.parts = parts;
                        return None;
                    }
                }
                parts[at].4 = face.and_then(|face| face.words.get(&text[range])?[around].clone());
                missing += parts[at].4.is_none() as usize;
            }
            nudged ^= run.len > 0;
            start += run.len;
        }
        if missing > 2 {
            let mut exact = Exact::default();
            let whole = self.layout_whole(text, font_size, font_runs, Some(&mut exact));
            self.harvest(text, 0, &whole, &exact, font_runs, &mut parts);
            self.parts = parts;
            return Some(whole);
        }
        for at in 0..parts.len() {
            let (index, size, range, around, None) = parts[at].clone() else {
                continue;
            };
            let before = text[..range.start]
                .chars()
                .next_back()
                .filter(|_| around & 1 != 0);
            let after = text[range.end..].chars().next().filter(|_| around & 2 != 0);
            let context = range.start - before.map_or(0, char::len_utf8)
                ..range.end + after.map_or(0, char::len_utf8);
            let word = range.start - context.start..range.end - context.start;
            if let Some(word) =
                self.numbers(font_runs[index].font_id, size, &text[context.clone()], word)
            {
                parts[at].4 = Some(Rc::new(word));
                continue;
            }
            let single = FontRun {
                len: context.len(),
                font_size: size,
                ..font_runs[index]
            };
            let mut exact = Exact::default();
            let shaped =
                self.layout_whole(&text[context.clone()], size, &[single], Some(&mut exact));
            self.harvest(
                text,
                context.start,
                &shaped,
                &exact,
                font_runs,
                &mut parts[at..at + 1],
            );
            if parts[at].4.is_none() {
                self.parts = parts;
                return None;
            }
        }
        let mut runs: Vec<ShapedRun> = Vec::with_capacity(1);
        let mut count: usize = parts
            .iter()
            .filter_map(|part| Some(part.4.as_ref()?.0.len()))
            .sum();
        let (mut x, mut last) = (0f64, (usize::MAX, font_size));
        for (index, size, range, _, word) in &parts {
            let Some(Word(glyphs, width)) = word.as_deref() else {
                continue;
            };
            // WARNING: like `layout_whole`, a run takes the size of the line's run closest to its shaped size.
            if *index != last.0 {
                let shaped = f32::from(*size);
                let run_size = font_runs
                    .iter()
                    .map(|run| run.font_size)
                    .min_by(|a, b| {
                        (f32::from(*a) - shaped)
                            .abs()
                            .total_cmp(&(f32::from(*b) - shaped).abs())
                    })
                    .unwrap_or(font_size);
                last = (*index, run_size);
            }
            for &(at, font_id, id, y, offset) in glyphs {
                let glyph = ShapedGlyph {
                    id,
                    position: point(px((x + at) as f32), px(y)),
                    index: range.start + offset,
                    is_emoji: false,
                };
                match runs.last_mut() {
                    Some(run) if run.font_id == font_id && run.font_size == last.1 => {
                        run.glyphs.push(glyph)
                    }
                    _ => {
                        let mut glyphs = Vec::with_capacity(count);
                        glyphs.push(glyph);
                        runs.push(ShapedRun {
                            font_id,
                            font_size: last.1,
                            glyphs,
                        })
                    }
                }
                count -= 1;
            }
            x += width;
        }
        self.parts = parts;
        let (mut ascent, mut descent) = (0f32, 0f32);
        for run in font_runs {
            let fonts = &self.fonts;
            let metrics = self
                .metrics
                .entry(run.font_id)
                .or_insert_with(|| fonts[run.font_id.0].metrics());
            let scale = f32::from(run.font_size) / metrics.units_per_em as f32;
            ascent = ascent.max(metrics.ascent * scale);
            descent = descent.max(-metrics.descent * scale);
        }
        Some(LineLayout {
            runs,
            font_size,
            width: px(x as f32),
            ascent: px(ascent),
            descent: px(descent),
            len: text.len(),
        })
    }

    // WARNING: keeps the parts that `layout`, shaped from `text` at `at`, holds whole and in one font.
    fn harvest(
        &mut self,
        text: &str,
        at: usize,
        layout: &LineLayout,
        (xs, starts): &Exact,
        font_runs: &[FontRun],
        parts: &mut [Part],
    ) {
        let glyphs: Vec<_> = layout
            .runs
            .iter()
            .flat_map(|run| run.glyphs.iter().map(move |glyph| (run.font_id, glyph)))
            .collect();
        if glyphs.iter().any(|(_, glyph)| glyph.is_emoji) {
            return;
        }
        for (index, size, range, around, word) in parts {
            let first = glyphs.partition_point(|(_, glyph)| glyph.index + at < range.start);
            let count = glyphs[first..].partition_point(|(_, glyph)| glyph.index + at < range.end);
            let own = &glyphs[first..first + count];
            if word.is_some()
                || own
                    .first()
                    .is_none_or(|(font, _)| own.iter().any(|(other, _)| other != font))
            {
                continue;
            }
            // WARNING: CoreText moves some glyphs, like an opening quote, at the start of a run, so a part without a
            // space before it starts where its run starts, and one without a space after it ends where its run ends.
            let edge = |index: usize| match index - at {
                end if end == layout.len => Some(xs[glyphs.len()]),
                index => starts
                    .iter()
                    .find(|start| start.0 == index)
                    .map(|start| start.1),
            };
            let (Some(left), Some(right)) = (
                if *around & 1 == 0 {
                    edge(range.start)
                } else {
                    Some(xs[first])
                },
                if *around & 2 == 0 {
                    edge(range.end)
                } else {
                    Some(xs[first + count])
                },
            ) else {
                continue;
            };
            let shaped = Rc::new(Word(
                own.iter()
                    .zip(&xs[first..])
                    .map(|((font, glyph), x)| {
                        (
                            x - left,
                            *font,
                            glyph.id,
                            f32::from(glyph.position.y),
                            glyph.index + at - range.start,
                        )
                    })
                    .collect(),
                right - left,
            ));
            self.words += 1;
            self.faces
                .entry((font_runs[*index].font_id, f32::from(*size).to_bits()))
                .or_default()
                .words
                .entry(text[range.clone()].into())
                .or_default()[*around] = Some(shaped.clone());
            *word = Some(shaped);
        }
    }

    fn numbers(
        &mut self,
        font_id: FontId,
        size: Pixels,
        text: &str,
        word: std::ops::Range<usize>,
    ) -> Option<Word> {
        const CHARS: &[u8; 11] = b"0123456789 ";
        let bytes = text.as_bytes();
        if !bytes.iter().all(|byte| CHARS.contains(byte)) {
            return None;
        }
        let digit = |byte: u8| {
            if byte == b' ' {
                10
            } else {
                (byte - b'0') as usize
            }
        };
        let key = (font_id, f32::from(size).to_bits());
        let face = self.faces.entry(key).or_default();
        face.misses = face.misses.saturating_add(1);
        let (misses, numbers) = (face.misses, face.numbers.take());
        let shape = |state: &mut State, chars: &[usize]| {
            let text: String = chars.iter().map(|&at| CHARS[at] as char).collect();
            let run = FontRun {
                len: text.len(),
                font_id,
                font_size: size,
                letter_spacing: px(0.),
                word_spacing: px(0.),
                inset: [px(0.); 2],
                nowrap: false,
            };
            let mut exact = Exact::default();
            let layout = state.layout_whole(&text, size, &[run], Some(&mut exact));
            let glyphs: Vec<_> = layout
                .runs
                .iter()
                .flat_map(|run| {
                    run.glyphs.iter().map(move |glyph| {
                        let plain = glyph.position.y == px(0.) && !glyph.is_emoji;
                        (run.font_id, glyph.id, glyph.index, plain)
                    })
                })
                .collect();
            (glyphs, exact.0)
        };
        let mut numbers = match numbers {
            Some(numbers) => numbers,
            None if misses == 32 => {
                let mut numbers = Box::new(Numbers {
                    font: FontId(usize::MAX),
                    glyphs: [GlyphId(0); 11],
                    ends: [0.; 11],
                    advances: [[f64::NAN; 11]; 11],
                    triples: vec![false; 1331].into(),
                    edges: [[0; 11]; 11],
                });
                for at in 0..11 {
                    let (glyphs, xs) = shape(self, &[at]);
                    let [(font, id, 0, true)] = glyphs[..] else {
                        return None;
                    };
                    if xs[0] != 0. || at > 0 && font != numbers.font {
                        return None;
                    }
                    (numbers.font, numbers.glyphs[at], numbers.ends[at]) = (font, id, xs[1]);
                }
                // WARNING: a de Bruijn sequence holds each three of the characters in a row once.
                let (mut lyndon, mut order) = (vec![-1isize], Vec::new());
                while let Some(last) = lyndon.last_mut() {
                    *last += 1;
                    let length = lyndon.len();
                    if 3 % length == 0 {
                        order.extend(lyndon.iter().map(|&at| at as usize));
                    }
                    while lyndon.len() < 3 {
                        lyndon.push(lyndon[lyndon.len() - length]);
                    }
                    while lyndon.last() == Some(&10) {
                        lyndon.pop();
                    }
                }
                order.extend([order[0], order[1]]);
                let (glyphs, xs) = shape(self, &order);
                if glyphs.len() != order.len() {
                    return None;
                }
                let mut moved = vec![false; order.len()];
                for (at, &(font, id, index, plain)) in glyphs.iter().enumerate() {
                    moved[at] |= !plain
                        || font != numbers.font
                        || id != numbers.glyphs[order[at]]
                        || index != at;
                    if let Some(&next) = order.get(at + 1) {
                        let advance = &mut numbers.advances[order[at]][next];
                        if advance.is_nan() {
                            *advance = xs[at + 1] - xs[at];
                        } else if (*advance - (xs[at + 1] - xs[at])).abs() > 1e-9 {
                            moved[at] = true;
                            moved[at + 1] = true;
                        }
                    }
                }
                for at in 0..order.len() - 2 {
                    numbers.triples[order[at] * 121 + order[at + 1] * 11 + order[at + 2]] =
                        !moved[at..at + 3].contains(&true);
                }
                numbers
            }
            None => return None,
        };
        let n = bytes.len();
        let word = 'stitch: {
            if bytes.windows(3).any(|three| {
                !numbers.triples[digit(three[0]) * 121 + digit(three[1]) * 11 + digit(three[2])]
            }) {
                break 'stitch None;
            }
            for (a, b) in [(0, 1), (n.wrapping_sub(2), n.wrapping_sub(1))] {
                let Some((a, b)) = bytes.get(a).zip(bytes.get(b)) else {
                    break;
                };
                let (a, b) = (digit(*a), digit(*b));
                if numbers.edges[a][b] == 0 {
                    let (glyphs, xs) = shape(self, &[a, b]);
                    let advance = numbers.advances[a][b];
                    let same = matches!(glyphs[..], [(first, x, 0, true), (second, y, 1, true)]
                        if first == numbers.font && second == numbers.font
                            && x == numbers.glyphs[a] && y == numbers.glyphs[b])
                        && xs[0] == 0.
                        && (xs[1] - advance).abs() < 1e-9
                        && (xs[2] - advance - numbers.ends[b]).abs() < 1e-9;
                    numbers.edges[a][b] = if same { 1 } else { 2 };
                }
                if numbers.edges[a][b] != 1 {
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
                    glyphs.push((
                        x - left,
                        numbers.font,
                        numbers.glyphs[digit(byte)],
                        0.,
                        at - word.start,
                    ));
                }
                x += match bytes.get(at + 1) {
                    Some(&next) => numbers.advances[digit(byte)][digit(next)],
                    None => numbers.ends[digit(byte)],
                };
            }
            Some(Word(glyphs.into(), x - left))
        };
        self.faces.get_mut(&key)?.numbers = Some(numbers);
        word
    }

    fn spaced(&mut self, font_id: FontId, size: Pixels, ch: char) -> bool {
        let key = (font_id, f32::from(size).to_bits());
        if let Some(&spaced) = self.faces.get(&key).and_then(|face| face.spaced.get(&ch)) {
            return spaced;
        }
        let pair = format!(" {ch}");
        let run = FontRun {
            len: 1,
            font_id,
            font_size: size,
            letter_spacing: px(0.),
            word_spacing: px(0.),
            inset: [px(0.); 2],
            nowrap: false,
        };
        let space = self.layout_whole(" ", size, &[run], None).width;
        let both = self.layout_whole(
            &pair,
            size,
            &[FontRun {
                len: pair.len(),
                ..run
            }],
            None,
        );
        let spaced = both
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .find(|glyph| glyph.index > 0)
            .is_some_and(|glyph| (glyph.position.x - space).abs() < px(1e-4));
        self.faces.entry(key).or_default().spaced.insert(ch, spaced);
        spaced
    }

    fn layout_segments(
        &mut self,
        text: &str,
        font_size: Pixels,
        font_runs: &[FontRun],
    ) -> LineLayout {
        if self.words > 16384 {
            self.faces.clear();
            self.segments.clear();
            self.words = 0;
        }
        let mut runs: Vec<ShapedRun> = Vec::new();
        let (mut x, mut ascent, mut descent) = (0f32, px(0.), px(0.));
        let mut start = 0;
        for run in font_runs {
            let part = &text[start..start + run.len];
            let key = (run.font_id, f32::from(run.font_size).to_bits());
            let mut offset = 0;
            while offset < part.len() {
                let space = part.as_bytes()[offset] == b' ';
                let end = part[offset..]
                    .bytes()
                    .position(|byte| (byte == b' ') != space)
                    .map_or(part.len(), |length| offset + length);
                let segment = &part[offset..end];
                let found = self
                    .segments
                    .get(&key)
                    .and_then(|map| map.get(segment))
                    .cloned();
                let shaped = match found {
                    Some(shaped) => shaped,
                    None => {
                        let single = FontRun {
                            len: segment.len(),
                            ..*run
                        };
                        let numbers =
                            self.numbers(run.font_id, run.font_size, segment, 0..segment.len());
                        let shaped = Rc::new(match numbers {
                            Some(Word(glyphs, width)) => {
                                let fonts = &self.fonts;
                                let metrics = self
                                    .metrics
                                    .entry(run.font_id)
                                    .or_insert_with(|| fonts[run.font_id.0].metrics());
                                let scale = f32::from(run.font_size) / metrics.units_per_em as f32;
                                let layout = LineLayout {
                                    runs: vec![ShapedRun {
                                        font_id: glyphs[0].1,
                                        font_size: run.font_size,
                                        glyphs: glyphs
                                            .iter()
                                            .map(|&(x, _, id, y, index)| ShapedGlyph {
                                                id,
                                                position: point(px(x as f32), px(y)),
                                                index,
                                                is_emoji: false,
                                            })
                                            .collect(),
                                    }],
                                    font_size: run.font_size,
                                    width: px(width as f32),
                                    ascent: px(0f32.max(metrics.ascent * scale)),
                                    descent: px(0f32.max(-metrics.descent * scale)),
                                    len: segment.len(),
                                };
                                #[cfg(debug_assertions)]
                                assert!(
                                    same_lines(
                                        &layout,
                                        &self.layout_whole(segment, run.font_size, &[single], None)
                                    ),
                                    "digits of {segment:?} shape unlike the whole segment"
                                );
                                layout
                            }
                            None => self.layout_whole(segment, run.font_size, &[single], None),
                        });
                        self.words += 1;
                        self.segments
                            .entry(key)
                            .or_default()
                            .insert(segment.into(), shaped.clone());
                        shaped
                    }
                };
                for shaped_run in &shaped.runs {
                    let glyphs = match runs.last_mut() {
                        Some(last)
                            if last.font_id == shaped_run.font_id
                                && last.font_size == shaped_run.font_size =>
                        {
                            &mut last.glyphs
                        }
                        _ => {
                            runs.push(ShapedRun {
                                font_id: shaped_run.font_id,
                                font_size: shaped_run.font_size,
                                glyphs: Vec::with_capacity(text.len() - start - offset),
                            });
                            &mut runs.last_mut().unwrap().glyphs
                        }
                    };
                    glyphs.extend(shaped_run.glyphs.iter().map(|glyph| ShapedGlyph {
                        position: point(glyph.position.x + px(x), glyph.position.y),
                        index: glyph.index + start + offset,
                        ..glyph.clone()
                    }));
                }
                x += f32::from(shaped.width);
                ascent = ascent.max(shaped.ascent);
                descent = descent.max(shaped.descent);
                offset = end;
            }
            start += run.len;
        }
        LineLayout {
            runs,
            font_size,
            width: px(x),
            ascent,
            descent,
            len: text.len(),
        }
    }

    fn layout_whole(
        &mut self,
        text: &str,
        font_size: Pixels,
        font_runs: &[FontRun],
        mut exact: Option<&mut Exact>,
    ) -> LineLayout {
        let mut string = CFMutableAttributedString::new();
        let mut max_ascent = 0.0f32;
        let mut max_descent = 0.0f32;

        {
            let mut text = text;
            // WARNING: CoreText merges adjacent runs that end up in equal fonts, including runs of
            // different fonts that fall back to the same one, so every other run that inserts
            // text is set a hair larger to keep ligatures from forming across them. The first
            // run keeps its exact size: SF tracks by its exact point size, and a nudged size
            // draws wider or narrower than AppKit sets the same text.
            let mut break_ligature = false;
            for run in font_runs {
                let text_run;
                (text_run, text) = text.split_at(run.len);

                let utf16_start = string.char_len();
                // WARNING: `replace_str` can drop some code points, like a BOM at the start.
                string.replace_str(&CFString::new(text_run), CFRange::init(utf16_start, 0));
                let utf16_end = string.char_len();

                let length = utf16_end - utf16_start;
                let cf_range = CFRange::init(utf16_start, length);
                let fonts = &self.fonts;
                let font_metrics = *self
                    .metrics
                    .entry(run.font_id)
                    .or_insert_with(|| fonts[run.font_id.0].metrics());
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
        let mut index_converter = StringIndexConverter::new(text);
        let mut shift = 0f32;
        let mut emoji_end = None;
        let mut pen = 0f64;
        for run in glyph_runs.into_iter() {
            let mut first = exact.is_some();
            let attributes = run.attributes().unwrap();
            let font = unsafe {
                attributes
                    .get(kCTFontAttributeName)
                    .downcast::<CTFont>()
                    .unwrap()
            };
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
                    self.natives.insert(key, (font.clone(), font_id));
                    font_id
                }
            };
            let is_emoji = self.is_emoji(font_id);
            // Emoji take the advance of the size they draw at, see `rasterize_glyph`.
            // WARNING: the emoji font that system fonts fall back to has wider advances than Chromium's.
            let doubled = is_emoji
                .then(|| {
                    core_text::font::new_from_name(
                        "AppleColorEmoji",
                        f64::from(f32::from(run_size)) * 2.,
                    )
                    .ok()
                })
                .flatten();

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
            for ((&glyph_id, position), &glyph_utf16_index) in run
                .glyphs()
                .iter()
                .zip(run.positions().iter())
                .zip(run.string_indices().iter())
            {
                index_converter.seek_to_utf16_index(usize::try_from(glyph_utf16_index).unwrap());
                if let Some(end) = emoji_end.take() {
                    shift = end - position.x as f32;
                }
                let x = position.x as f32 + shift;
                if let Some((xs, starts)) = &mut exact {
                    if mem::take(&mut first) {
                        starts.push((index_converter.utf8_index, pen));
                    }
                    xs.push(position.x + f64::from(shift));
                }
                glyphs.push(ShapedGlyph {
                    id: GlyphId(glyph_id as u32),
                    position: point(x, position.y as f32).map(px),
                    index: index_converter.utf8_index,
                    is_emoji,
                });
                if let Some(doubled) = &doubled {
                    // SAFETY: the pointer is to one glyph, and the advances can be null.
                    let drawn = unsafe {
                        doubled.get_advances_for_glyphs(
                            kCTFontOrientationDefault,
                            &glyph_id,
                            std::ptr::null_mut(),
                            1,
                        )
                    } as f32;
                    // WARNING: where CoreText enlarges emoji, Chromium's advance is one device pixel less.
                    let drawn = if drawn > f32::from(run_size) * 2. {
                        drawn - 1.
                    } else {
                        drawn
                    };
                    emoji_end = Some(x + drawn / 2.);
                }
            }
            if exact.is_some() {
                pen += run.get_typographic_bounds().width;
            }
        }
        let typographic_bounds = line.get_typographic_bounds();
        if let Some((xs, _)) = exact {
            xs.push(typographic_bounds.width + f64::from(shift));
        }
        LineLayout {
            runs,
            font_size,
            width: px(emoji_end.unwrap_or(typographic_bounds.width as f32 + shift)),
            ascent: max_ascent.into(),
            descent: max_descent.into(),
            len: text.len(),
        }
    }
}

#[cfg(debug_assertions)]
fn same_lines(a: &LineLayout, b: &LineLayout) -> bool {
    let glyphs = |layout: &LineLayout| {
        layout
            .runs
            .iter()
            .flat_map(|run| {
                run.glyphs
                    .iter()
                    .map(move |glyph| (run.font_id, run.font_size, glyph.clone()))
            })
            .collect::<Vec<_>>()
    };
    let (a_glyphs, b_glyphs) = (glyphs(a), glyphs(b));
    a.runs.len() == b.runs.len()
        && (a.width - b.width).abs() < px(1e-3)
        && (a.ascent, a.descent) == (b.ascent, b.descent)
        && a_glyphs.len() == b_glyphs.len()
        && a_glyphs
            .iter()
            .zip(&b_glyphs)
            .all(|((a_font, a_size, a), (b_font, b_size, b))| {
                (a_font, a_size, a.id, a.index, a.is_emoji, a.position.y)
                    == (b_font, b_size, b.id, b.index, b.is_emoji, b.position.y)
                    && (a.position.x - b.position.x).abs() < px(1e-3)
            })
}

#[derive(Debug, Clone)]
struct StringIndexConverter<'a> {
    text: &'a str,
    utf8_index: usize,
    utf16_index: usize,
}

impl<'a> StringIndexConverter<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            utf8_index: 0,
            utf16_index: 0,
        }
    }

    fn seek_to_utf16_index(&mut self, utf16_target: usize) {
        while self.utf16_index > utf16_target
            && let Some(c) = self.text[..self.utf8_index].chars().next_back()
        {
            self.utf8_index -= c.len_utf8();
            self.utf16_index -= c.len_utf16();
        }
        for (offset, c) in self.text[self.utf8_index..].char_indices() {
            if self.utf16_index >= utf16_target {
                self.utf8_index += offset;
                return;
            }
            self.utf16_index += c.len_utf16();
        }
        self.utf8_index = self.text.len();
    }
}

fn font_kit_metrics(metrics: Metrics) -> FontMetrics {
    FontMetrics {
        units_per_em: metrics.units_per_em,
        ascent: metrics.ascent,
        descent: metrics.descent,
        line_gap: metrics.line_gap,
        cap_height: metrics.cap_height,
        x_height: metrics.x_height,
        tall_ascent: false,
        em_ascent: metrics.ascent / (metrics.ascent - metrics.descent),
        bounding_box: bounds_from_rect(metrics.bounding_box),
    }
}

fn bounds_from_rect(rect: RectF) -> Bounds<f32> {
    Bounds {
        origin: point(rect.origin_x(), rect.origin_y()),
        size: size(rect.width(), rect.height()),
    }
}

fn bounds_from_rect_i32(rect: RectI) -> Bounds<DevicePixels> {
    Bounds {
        origin: point(DevicePixels(rect.origin_x()), DevicePixels(rect.origin_y())),
        size: size(DevicePixels(rect.width()), DevicePixels(rect.height())),
    }
}

fn size_from_vector2f(vec: Vector2F) -> Size<f32> {
    size(vec.x(), vec.y())
}

fn font_kit_weight(value: FontWeight) -> FontkitWeight {
    FontkitWeight(value.0)
}

fn font_kit_style(style: FontStyle) -> FontkitStyle {
    match style {
        FontStyle::Normal => FontkitStyle::Normal,
        FontStyle::Italic => FontkitStyle::Italic,
        FontStyle::Oblique => FontkitStyle::Oblique,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_fonts_load_once_per_process() {
        let names = |text: &TextSystem, weight: f32| {
            let font = Font {
                weight: crate::FontWeight(weight),
                ..crate::text::font(".SystemUIFont")
            };
            let id = text.font_id(&font).unwrap();
            text.0.borrow().postscript_names_by_font_id[&id].clone()
        };
        let [first, second] = [TextSystem::new(), TextSystem::new()];
        let bold = names(&first, 600.);
        assert!(bold.ends_with("#wght600"), "{bold}");
        assert_eq!(
            [names(&second, 400.), names(&second, 600.)],
            [names(&first, 400.), bold]
        );
        let missing = crate::text::font("No Such Family h2o");
        assert!(first.font_id(&missing).is_err() && second.font_id(&missing).is_err());
        let families = SYSTEM_FAMILIES.lock().unwrap();
        assert!(families.iter().any(|(key, faces)| {
            key.0 == "No Such Family h2o" && faces.get().is_some_and(Option::is_none)
        }));
        let system = families
            .iter()
            .find(|(key, _)| key.0 == ".AppleSystemUIFont")
            .unwrap();
        let face = &system.1.get().unwrap().as_ref().unwrap()[0].0;
        let shared = second.0.borrow().fonts.iter().any(|font| {
            font.native_font().as_concrete_TypeRef() == face.native_font().as_concrete_TypeRef()
        });
        assert!(shared, "the second text system uses the process's faces");
    }

    #[test]
    fn fonts_from_memory_stay_with_their_text_system() {
        let bytes = std::fs::read("/System/Library/Fonts/Supplemental/Chalkduster.ttf").unwrap();
        let text = TextSystem::new();
        text.add_fonts(vec![Cow::Owned(bytes)]).unwrap();
        let id = text.font_id(&crate::text::font("Chalkduster")).unwrap();
        assert!(text.0.borrow().local.contains(&id));
        assert!(
            !SYSTEM_FAMILIES
                .lock()
                .unwrap()
                .keys()
                .any(|key| key.0 == "Chalkduster")
        );
    }
}
