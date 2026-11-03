//! DirectWrite fonts, shaping and glyph rasterization.

use crate::{
    Bounds, DevicePixels, Pixels, Point, Rgba, Size, point, px, size,
    text::{
        Font, FontFeatures, FontId, FontMetrics, FontRun, FontStyle, FontWeight, GlyphId,
        LineLayout, RenderGlyphParams, SUBPIXEL_VARIANTS_X, ShapedGlyph, ShapedRun,
    },
};
use anyhow::{Context as _, Result, bail};
use rustc_hash::FxHashMap;
use std::{borrow::Cow, cell::RefCell, ffi::c_void, mem::ManuallyDrop};
use windows::{
    Win32::{
        Foundation::*,
        Globalization::GetUserDefaultLocaleName,
        Graphics::{DirectWrite::*, Gdi::LOGFONTW},
        System::SystemServices::LOCALE_NAME_MAX_LENGTH,
        UI::WindowsAndMessaging::*,
    },
    core::{BOOL, HSTRING, IUnknown, Interface, Ref, implement, w},
};

struct FontInfo {
    family: HSTRING,
    face: IDWriteFontFace3,
    features: IDWriteTypography,
    fallbacks: Option<IDWriteFontFallback>,
    collection: IDWriteFontCollection1,
}

struct State {
    factory: IDWriteFactory5,
    loader: IDWriteInMemoryFontFileLoader,
    builder: IDWriteFontSetBuilder1,
    renderer: IDWriteTextRenderer,
    locale: HSTRING,
    ui_font: String,
    system: IDWriteFontCollection1,
    custom: IDWriteFontCollection1,
    fonts: Vec<FontInfo>,
    by_font: FxHashMap<Font, FontId>,
    /// Fonts by the address of their face, for the faces DirectWrite falls back to.
    by_face: FxHashMap<usize, FontId>,
    utf16: Vec<u16>,
}

pub(crate) struct TextSystem(RefCell<State>);

impl Drop for TextSystem {
    fn drop(&mut self) {
        let state = self.0.borrow();
        // SAFETY: the loader was registered with this factory.
        let _ = unsafe { state.factory.UnregisterFontFileLoader(&state.loader) };
    }
}

impl TextSystem {
    pub fn new() -> Self {
        Self::create().expect("DirectWrite must start")
    }

    fn create() -> Result<Self> {
        // SAFETY: plain DirectWrite setup; the in-memory loader needs Windows 10 1703.
        unsafe {
            let factory: IDWriteFactory5 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let loader = factory.CreateInMemoryFontFileLoader()?;
            factory.RegisterFontFileLoader(&loader)?;
            let builder = factory.CreateFontSetBuilder()?;
            let mut locale = [0u16; LOCALE_NAME_MAX_LENGTH as usize];
            let len = GetUserDefaultLocaleName(&mut locale).max(1) as usize - 1;
            let locale = HSTRING::from_wide(&locale[..len]);
            let mut system = None;
            factory.GetSystemFontCollection(false, &mut system, true)?;
            let custom = factory.CreateFontCollectionFromFontSet(&builder.CreateFontSet()?)?;
            let mut font = LOGFONTW::default();
            let ui_font = match SystemParametersInfoW(
                SPI_GETICONTITLELOGFONT,
                size_of::<LOGFONTW>() as u32,
                Some((&raw mut font).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            ) {
                Ok(()) => String::from_utf16_lossy(&font.lfFaceName)
                    .trim_end_matches('\0')
                    .to_owned(),
                Err(_) => "Segoe UI".to_owned(),
            };
            Ok(Self(RefCell::new(State {
                renderer: Renderer.into(),
                factory,
                loader,
                builder,
                locale,
                ui_font,
                system: system.context("missing system fonts")?,
                custom,
                fonts: Vec::new(),
                by_font: FxHashMap::default(),
                by_face: FxHashMap::default(),
                utf16: Vec::new(),
            })))
        }
    }

    pub fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        let mut state = self.0.borrow_mut();
        // SAFETY: DirectWrite copies the font data, since there is no owner.
        unsafe {
            for font in &fonts {
                let file = state.loader.CreateInMemoryFontFileReference(
                    &state.factory,
                    font.as_ptr().cast(),
                    font.len() as u32,
                    None,
                )?;
                state.builder.AddFontFile(&file)?;
            }
            state.custom = state
                .factory
                .CreateFontCollectionFromFontSet(&state.builder.CreateFontSet()?)?;
        }
        // Fonts selected before, maybe from system fonts, are selected again.
        state.by_font.clear();
        Ok(())
    }

    pub fn all_font_names(&self) -> Vec<String> {
        let state = self.0.borrow();
        let mut names = Vec::new();
        for collection in [&state.system, &state.custom] {
            // SAFETY: plain collection queries.
            unsafe {
                for index in 0..collection.GetFontFamilyCount() {
                    if let Ok(family) = collection.GetFontFamily(index)
                        && let Ok(name) = family
                            .GetFamilyNames()
                            .and_then(|names| localized(&names, &state.locale))
                    {
                        names.push(name);
                    }
                }
            }
        }
        names
    }

    pub fn font_id(&self, font: &Font) -> Result<FontId> {
        let mut state = self.0.borrow_mut();
        if let Some(id) = state.by_font.get(font) {
            return Ok(*id);
        }
        state
            .select(font)
            .with_context(|| format!("selecting font {font:?}"))
    }

    pub fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        let state = self.0.borrow();
        let face = &state.fonts[font_id.0].face;
        let mut metrics = DWRITE_FONT_METRICS1::default();
        // SAFETY: plain metrics query into a local struct.
        unsafe { face.GetMetrics(&mut metrics) };
        FontMetrics {
            units_per_em: metrics.Base.designUnitsPerEm as u32,
            ascent: metrics.Base.ascent as f32,
            descent: -(metrics.Base.descent as f32),
            line_gap: metrics.Base.lineGap as f32,
            cap_height: metrics.Base.capHeight as f32,
            x_height: metrics.Base.xHeight as f32,
            bounding_box: Bounds::new(
                point(metrics.glyphBoxLeft as f32, metrics.glyphBoxBottom as f32),
                size(
                    (metrics.glyphBoxRight - metrics.glyphBoxLeft) as f32,
                    (metrics.glyphBoxTop - metrics.glyphBoxBottom) as f32,
                ),
            ),
            tall_ascent: false,
        }
    }

    pub fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        let metrics = self.0.borrow().glyph_metrics(font_id, glyph_id)?;
        Ok(size(metrics.advanceWidth as f32, 0.))
    }

    pub fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        let state = self.0.borrow();
        let mut glyph = 0u16;
        // SAFETY: one code point in, one glyph out.
        unsafe {
            state.fonts[font_id.0]
                .face
                .GetGlyphIndices(&(ch as u32), 1, &mut glyph)
        }
        .ok()?;
        Some(GlyphId(glyph as u32))
    }

    pub fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        let state = self.0.borrow();
        let analysis = state.analysis(params)?;
        // SAFETY: plain analysis query.
        let rect = unsafe { analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)? };
        Ok(if rect.right <= rect.left || rect.bottom <= rect.top {
            Bounds::default()
        } else {
            Bounds::new(
                point(DevicePixels(rect.left), DevicePixels(rect.top)),
                size(
                    DevicePixels(rect.right - rect.left),
                    DevicePixels(rect.bottom - rect.top),
                ),
            )
        })
    }

    /// One alpha byte per pixel, or straight-alpha BGRA pixels for emoji.
    pub fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        if bounds.size.width.0 <= 0 || bounds.size.height.0 <= 0 {
            bail!("glyph bounds are empty");
        }
        let state = self.0.borrow();
        let rect = RECT {
            left: bounds.origin.x.0,
            top: bounds.origin.y.0,
            right: bounds.origin.x.0 + bounds.size.width.0,
            bottom: bounds.origin.y.0 + bounds.size.height.0,
        };
        let pixels = bounds.size.width.0 as usize * bounds.size.height.0 as usize;
        if params.is_emoji
            && let Ok(color) = state.rasterize_color(params, bounds)
        {
            return Ok((bounds.size, color));
        }
        let mut alpha = vec![0u8; pixels];
        // SAFETY: the buffer holds one byte per pixel of the rectangle.
        unsafe {
            state.analysis(params)?.CreateAlphaTexture(
                DWRITE_TEXTURE_ALIASED_1x1,
                &rect,
                &mut alpha,
            )?
        };
        if params.is_emoji {
            alpha = alpha
                .into_iter()
                .flat_map(|alpha| [0, 0, 0, alpha])
                .collect();
        }
        Ok((bounds.size, alpha))
    }

    pub fn layout_line(&self, text: &str, font_size: Pixels, font_runs: &[FontRun]) -> LineLayout {
        let mut state = self.0.borrow_mut();
        let result = if text.contains(['\u{2028}', '\u{2029}']) {
            // DirectWrite breaks lines at these even without wrapping, so each part is laid out alone.
            let mut layout = LineLayout {
                font_size,
                len: text.len(),
                ..Default::default()
            };
            let (mut runs, mut run_offset, mut offset) = (font_runs, 0, 0);
            let mut parts = Vec::new();
            (|| -> Result<LineLayout> {
                for part in text.split_inclusive(['\u{2028}', '\u{2029}']) {
                    parts.clear();
                    let mut remaining = part.len();
                    while remaining > 0
                        && let Some(run) = runs.first()
                    {
                        let len = remaining.min(run.len - run_offset);
                        if len > 0 {
                            parts.push(FontRun { len, ..*run });
                        }
                        remaining -= len;
                        run_offset += len;
                        if run_offset == run.len {
                            runs = &runs[1..];
                            run_offset = 0;
                        }
                    }
                    let part_layout = state.layout(part, font_size, &parts)?;
                    for mut run in part_layout.runs {
                        for glyph in &mut run.glyphs {
                            glyph.index += offset;
                            glyph.position.x += layout.width;
                        }
                        match layout.runs.last_mut() {
                            Some(last)
                                if last.font_id == run.font_id
                                    && last.font_size == run.font_size =>
                            {
                                last.glyphs.append(&mut run.glyphs)
                            }
                            _ => layout.runs.push(run),
                        }
                    }
                    layout.width += part_layout.width;
                    layout.ascent = layout.ascent.max(part_layout.ascent);
                    layout.descent = layout.descent.max(part_layout.descent);
                    offset += part.len();
                }
                Ok(std::mem::take(&mut layout))
            })()
        } else {
            state.layout(text, font_size, font_runs)
        };
        result.unwrap_or_else(|error| {
            eprintln!("apps: text layout: {error:#}");
            LineLayout {
                font_size,
                ..Default::default()
            }
        })
    }

    /// DirectWrite needs no dilation.
    pub fn glyph_dilation_for_color(&self, _color: Rgba) -> u8 {
        0
    }
}

impl State {
    fn select(&mut self, font: &Font) -> Option<FontId> {
        let id = self.find(font).or_else(|| {
            // Fonts installed since launch.
            let mut system = None;
            // SAFETY: plain factory query.
            if unsafe {
                self.factory
                    .GetSystemFontCollection(false, &mut system, true)
            }
            .is_ok()
                && let Some(system) = system
            {
                self.system = system;
            }
            self.find(font)
        })?;
        self.by_font.insert(font.clone(), id);
        Some(id)
    }

    fn find(&mut self, font: &Font) -> Option<FontId> {
        let family = if &*font.family == ".SystemUIFont" {
            self.ui_font.as_str()
        } else {
            &font.family
        };
        let family = HSTRING::from(family);
        for collection in [self.custom.clone(), self.system.clone()] {
            // SAFETY: plain DirectWrite queries.
            unsafe {
                let Ok(set) = collection.GetFontSet() else {
                    continue;
                };
                let Ok(matches) = set.GetMatchingFonts(
                    &family,
                    DWRITE_FONT_WEIGHT(font.weight.0 as i32),
                    DWRITE_FONT_STRETCH_NORMAL,
                    match font.style {
                        FontStyle::Normal => DWRITE_FONT_STYLE_NORMAL,
                        FontStyle::Italic => DWRITE_FONT_STYLE_ITALIC,
                        FontStyle::Oblique => DWRITE_FONT_STYLE_OBLIQUE,
                    },
                ) else {
                    continue;
                };
                for index in 0..matches.GetFontCount() {
                    let Ok(face) = matches
                        .GetFontFaceReference(index)
                        .and_then(|face| face.CreateFontFace())
                    else {
                        continue;
                    };
                    let Ok(features) = self.typography(&font.features) else {
                        continue;
                    };
                    let fallbacks = font.fallbacks.as_ref().and_then(|fallbacks| {
                        self.fallbacks(fallbacks.fallback_list()).ok().flatten()
                    });
                    let id = FontId(self.fonts.len());
                    self.by_face.insert(face.as_raw() as usize, id);
                    self.fonts.push(FontInfo {
                        family: family.clone(),
                        face,
                        features,
                        fallbacks,
                        collection,
                    });
                    return Some(id);
                }
            }
        }
        None
    }

    /// The OpenType features, with ligatures and contextual alternates on unless turned off.
    fn typography(&self, features: &FontFeatures) -> Result<IDWriteTypography> {
        let tag = |name: &str| {
            DWRITE_FONT_FEATURE_TAG(u32::from_le_bytes(
                name.as_bytes().try_into().unwrap_or(*b"    "),
            ))
        };
        // SAFETY: plain DirectWrite calls.
        unsafe {
            let typography = self.factory.CreateTypography()?;
            for default in ["liga", "clig", "calt"] {
                let on = features
                    .tag_value_list()
                    .iter()
                    .find(|(name, _)| name == default)
                    .is_none_or(|(_, value)| *value != 0);
                typography.AddFontFeature(DWRITE_FONT_FEATURE {
                    nameTag: tag(default),
                    parameter: on as u32,
                })?;
            }
            for (name, value) in features.tag_value_list() {
                if !matches!(name.as_str(), "liga" | "clig" | "calt") {
                    typography.AddFontFeature(DWRITE_FONT_FEATURE {
                        nameTag: tag(name),
                        parameter: *value,
                    })?;
                }
            }
            Ok(typography)
        }
    }

    /// Falls back to `families` before the system fallbacks.
    fn fallbacks(&self, families: &[String]) -> Result<Option<IDWriteFontFallback>> {
        if families.is_empty() {
            return Ok(None);
        }
        // SAFETY: plain DirectWrite calls; the names outlive `AddMapping`.
        unsafe {
            let builder = self.factory.CreateFontFallbackBuilder()?;
            let set = self.system.GetFontSet()?;
            let mut ranges = Vec::new();
            for family in families {
                let family = HSTRING::from(family.as_str());
                let Ok(face) = set
                    .GetMatchingFonts(
                        &family,
                        DWRITE_FONT_WEIGHT_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        DWRITE_FONT_STYLE_NORMAL,
                    )
                    .and_then(|fonts| fonts.GetFontFaceReference(0))
                    .and_then(|face| face.CreateFontFace())
                else {
                    continue;
                };
                let mut count = 0;
                let _ = face.GetUnicodeRanges(None, &mut count);
                if count == 0 {
                    continue;
                }
                ranges.resize(count as usize, DWRITE_UNICODE_RANGE::default());
                if face.GetUnicodeRanges(Some(&mut ranges), &mut count).is_ok() {
                    builder.AddMapping(&ranges, &[family.as_ptr()], None, None, None, 1.0)?;
                }
            }
            builder.AddMappings(&self.factory.GetSystemFontFallback()?)?;
            Ok(Some(builder.CreateFontFallback()?))
        }
    }

    fn glyph_metrics(&self, font_id: FontId, glyph_id: GlyphId) -> Result<DWRITE_GLYPH_METRICS> {
        let mut metrics = DWRITE_GLYPH_METRICS::default();
        // SAFETY: one glyph in, one metric out.
        unsafe {
            self.fonts[font_id.0].face.GetDesignGlyphMetrics(
                &(glyph_id.0 as u16),
                1,
                &mut metrics,
                false,
            )?
        };
        Ok(metrics)
    }

    fn layout(
        &mut self,
        text: &str,
        font_size: Pixels,
        font_runs: &[FontRun],
    ) -> Result<LineLayout> {
        let Some(first) = font_runs.first() else {
            return Ok(LineLayout {
                font_size,
                ..Default::default()
            });
        };
        self.utf16.clear();
        self.utf16.extend(text.encode_utf16());
        // SAFETY: plain DirectWrite calls. `Draw` calls the renderer back with the context.
        unsafe {
            let font = &self.fonts[first.font_id.0];
            let format: IDWriteTextFormat1 = self
                .factory
                .CreateTextFormat(
                    &font.family,
                    &font.collection,
                    font.face.GetWeight(),
                    font.face.GetStyle(),
                    DWRITE_FONT_STRETCH_NORMAL,
                    first.font_size.0,
                    &self.locale,
                )?
                .cast()?;
            if let Some(fallbacks) = &font.fallbacks {
                format.SetFontFallback(fallbacks)?;
            }
            let layout = self.factory.CreateTextLayout(
                &self.utf16,
                &format,
                f32::INFINITY,
                f32::INFINITY,
            )?;
            let (mut utf8, mut utf16) = (0, 0u32);
            for (index, run) in font_runs.iter().enumerate() {
                let font = &self.fonts[run.font_id.0];
                let len = text[utf8..utf8 + run.len].encode_utf16().count() as u32;
                let range = DWRITE_TEXT_RANGE {
                    startPosition: utf16,
                    length: len,
                };
                utf8 += run.len;
                utf16 += len;
                if index > 0 {
                    layout.SetFontCollection(&font.collection, range)?;
                    layout.SetFontFamilyName(&font.family, range)?;
                    // WARNING: alternating sizes by one ulp keeps ligatures from forming across runs.
                    layout.SetFontSize(
                        if index % 2 == 1 {
                            run.font_size.0.next_up()
                        } else {
                            run.font_size.0
                        },
                        range,
                    )?;
                    layout.SetFontStyle(font.face.GetStyle(), range)?;
                    layout.SetFontWeight(font.face.GetWeight(), range)?;
                }
                layout.SetTypography(&font.features, range)?;
            }
            let mut lines = [DWRITE_LINE_METRICS::default(); 4];
            let mut count = 0;
            layout.GetLineMetrics(Some(&mut lines), &mut count)?;
            let renderer = self.renderer.clone();
            let mut context = Context {
                state: self,
                text,
                utf8: 0,
                utf16: 0,
                runs: Vec::new(),
                width: 0.,
            };
            layout.Draw(Some((&raw mut context).cast()), &renderer, 0., 0.)?;
            let (width, mut runs) = (context.width, context.runs);
            // Undoes the nudge.
            for run in &mut runs {
                let shaped = run.font_size.0;
                run.font_size = font_runs
                    .iter()
                    .map(|font_run| font_run.font_size)
                    .min_by(|a, b| (a.0 - shaped).abs().total_cmp(&(b.0 - shaped).abs()))
                    .unwrap_or(font_size);
            }
            Ok(LineLayout {
                font_size,
                width: px(width),
                ascent: px(lines[0].baseline),
                descent: px(lines[0].height - lines[0].baseline),
                runs,
                len: text.len(),
            })
        }
    }

    fn glyph_run<'a>(
        &self,
        params: &RenderGlyphParams,
        glyph: &'a u16,
        advance: &'a f32,
        offset: &'a DWRITE_GLYPH_OFFSET,
    ) -> DWRITE_GLYPH_RUN {
        DWRITE_GLYPH_RUN {
            // SAFETY: the run borrows the face without a reference count.
            fontFace: ManuallyDrop::new(Some(unsafe {
                std::ptr::read(&***self.fonts[params.font_id.0].face)
            })),
            fontEmSize: params.font_size.0,
            glyphCount: 1,
            glyphIndices: glyph,
            glyphAdvances: advance,
            glyphOffsets: offset,
            isSideways: false.into(),
            bidiLevel: 0,
        }
    }

    fn analysis(&self, params: &RenderGlyphParams) -> Result<IDWriteGlyphRunAnalysis> {
        let (glyph, advance, offset) =
            (params.glyph_id.0 as u16, 0., DWRITE_GLYPH_OFFSET::default());
        let run = self.glyph_run(params, &glyph, &advance, &offset);
        let transform = transform(params);
        let x = params.subpixel_variant.x as f32 / SUBPIXEL_VARIANTS_X as f32 / params.scale_factor;
        // SAFETY: the run and transform outlive the calls.
        unsafe {
            let (mut mode, mut grid) = (
                DWRITE_RENDERING_MODE1::default(),
                DWRITE_GRID_FIT_MODE::default(),
            );
            // The scale is in the transform, so the DPI is the default.
            self.fonts[params.font_id.0]
                .face
                .GetRecommendedRenderingMode(
                    params.font_size.0,
                    96.,
                    96.,
                    Some(&transform),
                    false,
                    DWRITE_OUTLINE_THRESHOLD_ANTIALIASED,
                    DWRITE_MEASURING_MODE_NATURAL,
                    None,
                    &mut mode,
                    &mut grid,
                )?;
            if mode == DWRITE_RENDERING_MODE1_OUTLINE {
                mode = DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC;
            }
            Ok(self.factory.CreateGlyphRunAnalysis(
                &run,
                Some(&transform),
                mode,
                DWRITE_MEASURING_MODE_NATURAL,
                grid,
                DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                x,
                0.,
            )?)
        }
    }

    /// Paints the color layers of a COLR glyph over each other, into straight-alpha BGRA.
    fn rasterize_color(
        &self,
        params: &RenderGlyphParams,
        bounds: Bounds<DevicePixels>,
    ) -> Result<Vec<u8>> {
        let (glyph, advance, offset) =
            (params.glyph_id.0 as u16, 0., DWRITE_GLYPH_OFFSET::default());
        let run = self.glyph_run(params, &glyph, &advance, &offset);
        let transform = transform(params);
        let x = params.subpixel_variant.x as f32 / SUBPIXEL_VARIANTS_X as f32 / params.scale_factor;
        let (width, height) = (bounds.size.width.0 as usize, bounds.size.height.0 as usize);
        let mut pixels = vec![[0f32; 4]; width * height];
        let mut alpha = Vec::new();
        // SAFETY: the layers stay valid until the enumerator moves; the buffers fit their rectangles.
        unsafe {
            let layers = IDWriteFactory2::TranslateColorGlyphRun(
                &self.factory,
                x,
                0.,
                &run,
                None,
                DWRITE_MEASURING_MODE_NATURAL,
                Some(&transform),
                0,
            )?;
            while layers.MoveNext()?.as_bool() {
                let layer = &*layers.GetCurrentRun()?;
                let analysis = self.factory.CreateGlyphRunAnalysis(
                    &layer.glyphRun,
                    Some(&transform),
                    DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
                    DWRITE_MEASURING_MODE_NATURAL,
                    DWRITE_GRID_FIT_MODE_DEFAULT,
                    DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                    layer.baselineOriginX,
                    layer.baselineOriginY,
                )?;
                let rect = analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)?;
                if rect.right <= rect.left || rect.bottom <= rect.top {
                    continue;
                }
                let layer_width = (rect.right - rect.left) as usize;
                alpha.clear();
                alpha.resize(layer_width * (rect.bottom - rect.top) as usize, 0);
                analysis.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &rect, &mut alpha)?;
                // Layers in the text color are drawn black, as emoji tiles have no text color.
                let color = if layer.paletteIndex == 0xFFFF {
                    DWRITE_COLOR_F {
                        a: 1.,
                        ..Default::default()
                    }
                } else {
                    layer.runColor
                };
                for (row, line) in alpha.chunks_exact(layer_width).enumerate() {
                    let y = rect.top + row as i32 - bounds.origin.y.0;
                    if !(0..height as i32).contains(&y) {
                        continue;
                    }
                    for (column, &coverage) in line.iter().enumerate() {
                        let x = rect.left + column as i32 - bounds.origin.x.0;
                        if coverage == 0 || !(0..width as i32).contains(&x) {
                            continue;
                        }
                        let a = coverage as f32 / 255. * color.a;
                        let pixel = &mut pixels[y as usize * width + x as usize];
                        for (channel, value) in
                            [color.b, color.g, color.r, 1.].into_iter().enumerate()
                        {
                            pixel[channel] = value * a + pixel[channel] * (1. - a);
                        }
                    }
                }
            }
        }
        Ok(pixels
            .into_iter()
            .flat_map(|[b, g, r, a]| {
                let straight = |value: f32| {
                    if a > 0. {
                        (value / a * 255.).round().clamp(0., 255.) as u8
                    } else {
                        0
                    }
                };
                [
                    straight(b),
                    straight(g),
                    straight(r),
                    (a * 255.).round() as u8,
                ]
            })
            .collect())
    }
}

fn transform(params: &RenderGlyphParams) -> DWRITE_MATRIX {
    DWRITE_MATRIX {
        m11: params.scale_factor,
        m22: params.scale_factor,
        ..Default::default()
    }
}

fn localized(names: &IDWriteLocalizedStrings, locale: &HSTRING) -> windows::core::Result<String> {
    // SAFETY: the buffer holds the name and its NUL.
    unsafe {
        let (mut index, mut exists) = (0, BOOL(0));
        names.FindLocaleName(locale, &mut index, &mut exists)?;
        if !exists.as_bool() {
            names.FindLocaleName(w!("en-US"), &mut index, &mut exists)?;
            if !exists.as_bool() {
                index = 0;
            }
        }
        let mut name = vec![0u16; names.GetStringLength(index)? as usize + 1];
        names.GetString(index, &mut name)?;
        Ok(String::from_utf16_lossy(&name[..name.len() - 1]))
    }
}

/// The layout being drawn, passed to the renderer callbacks.
struct Context<'a> {
    state: &'a mut State,
    text: &'a str,
    utf8: usize,
    utf16: usize,
    runs: Vec<ShapedRun>,
    width: f32,
}

impl Context<'_> {
    /// The UTF-8 index of a UTF-16 index, at or after the last one.
    fn utf8_index(&mut self, utf16: usize) -> usize {
        for char in self.text[self.utf8..].chars() {
            if self.utf16 >= utf16 {
                break;
            }
            self.utf16 += char.len_utf16();
            self.utf8 += char.len_utf8();
        }
        self.utf8
    }

    /// The font of a face DirectWrite picked, which can be a fallback face.
    fn font_id(&mut self, face: &IDWriteFontFace3) -> Option<FontId> {
        if let Some(id) = self.state.by_face.get(&(face.as_raw() as usize)) {
            return Some(*id);
        }
        // SAFETY: plain face queries.
        let font = unsafe {
            Font {
                family: localized(&face.GetFamilyNames().ok()?, &self.state.locale)
                    .ok()?
                    .into(),
                features: FontFeatures::default(),
                fallbacks: None,
                weight: FontWeight(face.GetWeight().0 as f32),
                style: match face.GetStyle() {
                    DWRITE_FONT_STYLE_ITALIC => FontStyle::Italic,
                    DWRITE_FONT_STYLE_OBLIQUE => FontStyle::Oblique,
                    _ => FontStyle::Normal,
                },
            }
        };
        let id = match self.state.by_font.get(&font) {
            Some(id) => *id,
            None => self.state.select(&font)?,
        };
        self.state.by_face.insert(face.as_raw() as usize, id);
        Some(id)
    }
}

#[implement(IDWriteTextRenderer)]
struct Renderer;

#[allow(non_snake_case)]
impl IDWritePixelSnapping_Impl for Renderer_Impl {
    fn IsPixelSnappingDisabled(&self, _: *const c_void) -> windows::core::Result<BOOL> {
        Ok(BOOL(0))
    }

    fn GetCurrentTransform(
        &self,
        _: *const c_void,
        transform: *mut DWRITE_MATRIX,
    ) -> windows::core::Result<()> {
        // SAFETY: DirectWrite passes a matrix to fill.
        unsafe {
            *transform = DWRITE_MATRIX {
                m11: 1.,
                m22: 1.,
                ..Default::default()
            };
        }
        Ok(())
    }

    fn GetPixelsPerDip(&self, _: *const c_void) -> windows::core::Result<f32> {
        Ok(1.)
    }
}

#[allow(non_snake_case)]
impl IDWriteTextRenderer_Impl for Renderer_Impl {
    fn DrawGlyphRun(
        &self,
        context: *const c_void,
        _: f32,
        _: f32,
        _: DWRITE_MEASURING_MODE,
        run: *const DWRITE_GLYPH_RUN,
        description: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _: Ref<IUnknown>,
    ) -> windows::core::Result<()> {
        // SAFETY: `State::layout` passes its context, and DirectWrite passes arrays of the run's
        // glyph count and the description's string length.
        unsafe {
            let (run, description) = (&*run, &*description);
            let count = run.glyphCount as usize;
            if count == 0
                || run.glyphIndices.is_null()
                || run.glyphAdvances.is_null()
                || description.clusterMap.is_null()
            {
                return Ok(());
            }
            let context = &mut *(context as *mut Context);
            let face: IDWriteFontFace3 = run
                .fontFace
                .as_ref()
                .ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?
                .cast()?;
            let font_id = context
                .font_id(&face)
                .ok_or_else(|| windows::core::Error::from(DWRITE_E_NOFONT))?;
            let color_font = face.IsColorFont().as_bool();
            let factory = context.state.factory.clone();
            let ids = std::slice::from_raw_parts(run.glyphIndices, count);
            let advances = std::slice::from_raw_parts(run.glyphAdvances, count);
            let offsets = (!run.glyphOffsets.is_null())
                .then(|| std::slice::from_raw_parts(run.glyphOffsets, count));
            let clusters = std::slice::from_raw_parts(
                description.clusterMap,
                description.stringLength as usize,
            );
            let mut glyphs = Vec::with_capacity(count);
            let mut utf16 = description.textPosition as usize;
            let mut start = 0;
            // Each cluster is the text whose cluster map entries share a first glyph.
            while start < clusters.len() {
                let first = clusters[start] as usize;
                let end = clusters[start..]
                    .iter()
                    .position(|&glyph| glyph as usize != first)
                    .map_or(clusters.len(), |len| start + len);
                let last = clusters.get(end).map_or(count, |&glyph| glyph as usize);
                let index = context.utf8_index(utf16);
                utf16 += end - start;
                for glyph in first..last.min(count) {
                    let id = GlyphId(ids[glyph] as u32);
                    let offset =
                        offsets.map_or(DWRITE_GLYPH_OFFSET::default(), |offsets| offsets[glyph]);
                    glyphs.push(ShapedGlyph {
                        id,
                        position: Point::new(
                            px(context.width + offset.advanceOffset),
                            px(-offset.ascenderOffset),
                        ),
                        index,
                        is_emoji: color_font && is_color_glyph(&factory, &face, id),
                    });
                    context.width += advances[glyph];
                }
                start = end;
            }
            context.runs.push(ShapedRun {
                font_id,
                font_size: px(run.fontEmSize),
                glyphs,
            });
            Ok(())
        }
    }

    fn DrawUnderline(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_UNDERLINE,
        _: Ref<IUnknown>,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn DrawStrikethrough(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_STRIKETHROUGH,
        _: Ref<IUnknown>,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn DrawInlineObject(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: Ref<IDWriteInlineObject>,
        _: BOOL,
        _: BOOL,
        _: Ref<IUnknown>,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}

/// Whether a glyph of a color font has color layers.
fn is_color_glyph(factory: &IDWriteFactory5, face: &IDWriteFontFace3, glyph: GlyphId) -> bool {
    let (index, advance, offset) = (glyph.0 as u16, 0., DWRITE_GLYPH_OFFSET::default());
    let run = DWRITE_GLYPH_RUN {
        // SAFETY: the run borrows the face without a reference count.
        fontFace: ManuallyDrop::new(Some(unsafe { std::ptr::read(&****face) })),
        fontEmSize: 14.,
        glyphCount: 1,
        glyphIndices: &index,
        glyphAdvances: &advance,
        glyphOffsets: &offset,
        isSideways: false.into(),
        bidiLevel: 0,
    };
    // SAFETY: the run outlives the call.
    unsafe {
        IDWriteFactory2::TranslateColorGlyphRun(
            factory,
            0.,
            0.,
            &run,
            None,
            DWRITE_MEASURING_MODE_NATURAL,
            None,
            0,
        )
    }
    .is_ok()
}
