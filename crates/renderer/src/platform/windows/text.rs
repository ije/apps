use crate::text::{FaceMetrics, Features, GlyphRaster, SUBPIXEL_STEPS, Span};
use crate::{Color, Font, FontId, Glyph, GlyphId, Line, Pixels, Rect, Size, point, px, size};
use anyhow::{Context as _, Result, bail, ensure};
use rustc_hash::FxHashMap;
use std::{cell::RefCell, ffi::c_void, mem::ManuallyDrop, ptr, slice};
use windows::Win32::Foundation::RECT;
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::LOGFONTW;
use windows::Win32::System::SystemServices::LOCALE_NAME_MAX_LENGTH;
use windows::Win32::UI::WindowsAndMessaging::SPI_GETICONTITLELOGFONT;
use windows::core::{BOOL, HSTRING, IUnknown, Interface, Ref, implement};

type Done = windows::core::Result<()>;

// Separators that DirectWrite breaks lines at, even in a layout without a width.
const BREAKS: [char; 6] = ['\r', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}'];

// Layouts find it by family in its collection, with its weight, style and stretch.
struct Face {
    face: IDWriteFontFace3,
    family: HSTRING,
    collection: IDWriteFontCollection1,
    weight: DWRITE_FONT_WEIGHT,
    style: DWRITE_FONT_STYLE,
    stretch: DWRITE_FONT_STRETCH,
    features: Option<(Features, IDWriteTypography)>,
    per_em: f32,
    color: bool,
}

struct Library {
    dwrite: IDWriteFactory5,
    loader: IDWriteInMemoryFontFileLoader,
    added_faces: IDWriteFontSetBuilder1,
    system: IDWriteFontCollection1,
    added: IDWriteFontCollection1,
    locale: HSTRING,
    ui_family: String,
    faces: Vec<Face>,
    // DirectWrite hands out one object per face, so its address finds the face again.
    by_address: FxHashMap<usize, FontId>,
    collector: IDWriteTextRenderer,
}

pub(crate) struct TextSystem(RefCell<Library>);

impl Default for TextSystem {
    fn default() -> Self {
        Self(RefCell::new(
            Library::open().expect("DirectWrite must start"),
        ))
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: the loader was registered with this factory.
        let _ = unsafe { self.dwrite.UnregisterFontFileLoader(&self.loader) };
    }
}

fn localized(names: &IDWriteLocalizedStrings, locale: &HSTRING) -> Option<String> {
    let english = HSTRING::from("en-us");
    // SAFETY: plain queries, and the buffer holds the name and its terminator.
    unsafe {
        let index = [locale, &english]
            .into_iter()
            .find_map(|wanted| {
                let (mut index, mut found) = (0, BOOL(0));
                names.FindLocaleName(wanted, &mut index, &mut found).ok()?;
                found.as_bool().then_some(index)
            })
            .unwrap_or(0);
        let mut name = vec![0u16; names.GetStringLength(index).ok()? as usize + 1];
        names.GetString(index, &mut name).ok()?;
        name.pop();
        Some(String::from_utf16_lossy(&name))
    }
}

// `size` is in device independent pixels.
fn one_glyph<R>(
    face: &IDWriteFontFace3,
    glyph: GlyphId,
    size: f32,
    f: impl FnOnce(&DWRITE_GLYPH_RUN) -> Result<R>,
) -> Result<R> {
    let (advance, offset) = (0f32, DWRITE_GLYPH_OFFSET::default());
    let mut run = DWRITE_GLYPH_RUN {
        fontFace: ManuallyDrop::new(Some(face.cast()?)),
        fontEmSize: size,
        glyphCount: 1,
        glyphIndices: &glyph,
        glyphAdvances: &advance,
        glyphOffsets: &offset,
        isSideways: false.into(),
        bidiLevel: 0,
    };
    let result = f(&run);
    // SAFETY: the run holds one reference to the face, which goes once here.
    unsafe { ManuallyDrop::drop(&mut run.fontFace) };
    result
}

fn is_color_glyph(factory: &IDWriteFactory5, face: &IDWriteFontFace3, glyph: GlyphId) -> bool {
    one_glyph(face, glyph, 16., |run| {
        // SAFETY: the run outlives the call. A glyph without color layers fails with DWRITE_E_NOCOLOR.
        unsafe {
            IDWriteFactory2::TranslateColorGlyphRun(
                factory,
                0.,
                0.,
                run,
                None,
                DWRITE_MEASURING_MODE_NATURAL,
                None,
                0,
            )
        }?;
        Ok(())
    })
    .is_ok()
}

fn typo_metrics(face: &IDWriteFontFace3) -> Option<[f32; 2]> {
    let tag = u32::from_le_bytes(*b"OS/2");
    let (mut data, mut len, mut context, mut exists) =
        (ptr::null_mut(), 0u32, ptr::null_mut(), BOOL(0));
    // SAFETY: the table stays readable until it is released, and only its first 72 bytes are read.
    unsafe {
        face.TryGetFontTable(tag, &mut data, &mut len, &mut context, &mut exists)
            .ok()?;
        if !exists.as_bool() {
            return None;
        }
        let read = (len >= 72 && !data.is_null()).then(|| {
            let table = slice::from_raw_parts(data.cast::<u8>(), 72);
            [68, 70].map(|at| f32::from(i16::from_be_bytes([table[at], table[at + 1]])))
        });
        face.ReleaseFontTable(context);
        read.filter(|[ascent, descent]| *ascent > 0. && *descent < 0.)
    }
}

fn texture_rect(bounds: Rect<i32>) -> RECT {
    RECT {
        left: bounds.origin.x,
        top: bounds.origin.y,
        right: bounds.origin.x + bounds.size.width,
        bottom: bounds.origin.y + bounds.size.height,
    }
}

impl Library {
    fn open() -> Result<Self> {
        // SAFETY: DirectWrite setup into locals. The in-memory loader needs Windows 10 1703.
        let (dwrite, loader, added_faces, added, system) = unsafe {
            let dwrite: IDWriteFactory5 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let loader = dwrite.CreateInMemoryFontFileLoader()?;
            dwrite.RegisterFontFileLoader(&loader)?;
            let added_faces = dwrite.CreateFontSetBuilder()?;
            let added = dwrite.CreateFontCollectionFromFontSet(&added_faces.CreateFontSet()?)?;
            let mut system = None;
            dwrite.GetSystemFontCollection(false, &mut system, false)?;
            (dwrite, loader, added_faces, added, system)
        };
        let mut locale = vec![0u16; LOCALE_NAME_MAX_LENGTH as usize];
        // SAFETY: fills the local buffer and returns its length with the terminator.
        let written = unsafe { GetUserDefaultLocaleName(&mut locale) };
        locale.truncate(written.max(1) as usize - 1);
        // The user interface font is the one of icon titles.
        let mut icons = LOGFONTW::default();
        let ui_family = super::setting(SPI_GETICONTITLELOGFONT, &mut icons)
            .then(|| {
                let name = icons.lfFaceName.split(|&unit| unit == 0).next()?;
                Some(String::from_utf16_lossy(name))
            })
            .flatten()
            .filter(|family| !family.is_empty())
            .unwrap_or_else(|| "Segoe UI".to_owned());
        Ok(Self {
            system: system.context("no system fonts")?,
            dwrite,
            loader,
            added_faces,
            added,
            locale: HSTRING::from_wide(&locale),
            ui_family,
            faces: Vec::new(),
            by_address: FxHashMap::default(),
            collector: Collector.into(),
        })
    }

    fn face(&self, id: FontId) -> &Face {
        &self.faces[id as usize]
    }

    fn typography(&self, features: &[(String, u32)]) -> Result<IDWriteTypography> {
        let tag = |name: &str| {
            DWRITE_FONT_FEATURE_TAG(u32::from_le_bytes(
                name.as_bytes().try_into().unwrap_or(*b"    "),
            ))
        };
        // SAFETY: plain DirectWrite calls.
        unsafe {
            let typography = self.dwrite.CreateTypography()?;
            // Ligatures and contextual forms stay on unless a setting turns them off.
            for (name, value) in ["liga", "clig", "calt"]
                .into_iter()
                .filter(|default| features.iter().all(|(name, _)| name != default))
                .map(|default| (default, 1))
                .chain(features.iter().map(|(name, value)| (name.as_str(), *value)))
            {
                typography.AddFontFeature(DWRITE_FONT_FEATURE {
                    nameTag: tag(name),
                    parameter: value,
                })?;
            }
            Ok(typography)
        }
    }

    fn register(&mut self, face: Face) -> FontId {
        let id = self.faces.len() as FontId;
        self.by_address.insert(face.face.as_raw() as usize, id);
        self.faces.push(face);
        id
    }

    // Describes a face for layouts, which find it again by family in `collection` with `matching`: the weight,
    // style and stretch that found it. With the face's own values a simulated bold face would not be found.
    fn describe(
        &self,
        face: IDWriteFontFace3,
        family: HSTRING,
        collection: IDWriteFontCollection1,
        matching: Option<(DWRITE_FONT_WEIGHT, DWRITE_FONT_STYLE, DWRITE_FONT_STRETCH)>,
        features: Option<(Features, IDWriteTypography)>,
    ) -> Face {
        let mut metrics = DWRITE_FONT_METRICS1::default();
        // SAFETY: plain face queries.
        unsafe {
            face.GetMetrics(&mut metrics);
            let (weight, style, stretch) =
                matching.unwrap_or_else(|| (face.GetWeight(), face.GetStyle(), face.GetStretch()));
            Face {
                family,
                collection,
                weight,
                style,
                stretch,
                features,
                per_em: f32::from(metrics.Base.designUnitsPerEm.max(1)),
                color: face.IsColorFont().as_bool(),
                face,
            }
        }
    }

    fn font(&mut self, font: &Font) -> Option<FontId> {
        let family = HSTRING::from(match &*font.family {
            ".SystemUIFont" => self.ui_family.as_str(),
            family => family,
        });
        let weight = DWRITE_FONT_WEIGHT((font.weight.round() as i32).clamp(1, 999));
        // In the order of `FontStyle`.
        let styles = [
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STYLE_ITALIC,
            DWRITE_FONT_STYLE_OBLIQUE,
        ];
        let style = styles[font.style as usize];
        // Added fonts come before installed ones. A missing family reads the installed fonts again once, in
        // case it was installed since.
        for fresh in [false, true] {
            if fresh {
                let mut system = None;
                // SAFETY: a plain factory query into a local.
                unsafe {
                    self.dwrite
                        .GetSystemFontCollection(false, &mut system, true)
                }
                .ok()?;
                self.system = system?;
            }
            for collection in [self.added.clone(), self.system.clone()] {
                let (mut index, mut found) = (0, BOOL(0));
                // SAFETY: plain collection queries.
                let face = unsafe {
                    collection
                        .FindFamilyName(&family, &mut index, &mut found)
                        .ok()
                        .filter(|()| found.as_bool())
                        .and_then(|()| collection.GetFontFamily(index).ok())
                        .and_then(|members| {
                            members
                                .GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, style)
                                .ok()
                        })
                        .and_then(|matched| matched.CreateFontFace().ok())
                        .and_then(|face| face.cast::<IDWriteFontFace3>().ok())
                };
                let Some(face) = face else {
                    continue;
                };
                let same = |known: &Face| {
                    known.face.as_raw() == face.as_raw()
                        && known.features.as_ref().map(|(features, _)| features)
                            == (!font.features.is_empty()).then_some(&font.features)
                };
                if let Some(id) = self.faces.iter().position(same) {
                    return Some(id as FontId);
                }
                let features = match font.features.is_empty() {
                    true => None,
                    false => Some((font.features.clone(), self.typography(&font.features).ok()?)),
                };
                let matching = Some((weight, style, DWRITE_FONT_STRETCH_NORMAL));
                let face = self.describe(face, family, collection, matching, features);
                return Some(self.register(face));
            }
        }
        None
    }

    fn found(&mut self, face: &IDWriteFontFace, span: FontId) -> Option<FontId> {
        let own = &self.faces.get(span as usize)?.face;
        if own.as_raw() == face.as_raw() {
            return Some(span);
        }
        if let Some(&id) = self.by_address.get(&(face.as_raw() as usize)) {
            return Some(id);
        }
        // SAFETY: plain face comparisons and queries.
        unsafe {
            let face: IDWriteFontFace3 = face.cast().ok()?;
            // A layout can make its own object for the span's face.
            if let (Ok(mine), Ok(theirs)) =
                (own.GetFontFaceReference(), face.GetFontFaceReference())
                && mine.Equals(&theirs).as_bool()
            {
                return Some(span);
            }
            let family = localized(&face.GetFamilyNames().ok()?, &self.locale)?;
            let fallback =
                self.describe(face, HSTRING::from(family), self.system.clone(), None, None);
            Some(self.register(fallback))
        }
    }

    fn metrics(&self, id: FontId) -> FaceMetrics {
        let face = self.face(id);
        let mut metrics = DWRITE_FONT_METRICS1::default();
        // SAFETY: a plain query into a local.
        unsafe { face.face.GetMetrics(&mut metrics) };
        let base = &metrics.Base;
        let em = face.per_em;
        let (ascent, descent) = (f32::from(base.ascent), -f32::from(base.descent));
        let [typo_ascent, typo_descent] = typo_metrics(&face.face).unwrap_or([ascent, descent]);
        let [left, top, right, bottom] = [
            metrics.glyphBoxLeft,
            metrics.glyphBoxTop,
            metrics.glyphBoxRight,
            metrics.glyphBoxBottom,
        ]
        .map(f32::from);
        FaceMetrics {
            ascent: ascent / em,
            descent: descent / em,
            line_gap: f32::from(base.lineGap) / em,
            cap_height: f32::from(base.capHeight) / em,
            x_height: f32::from(base.xHeight) / em,
            bounds: Rect::new(point(left, bottom), size(right - left, top - bottom)) / em,
            em_ascent: typo_ascent / (typo_ascent - typo_descent),
            tall: false,
        }
    }

    fn design(&self, id: FontId, glyph: GlyphId) -> DWRITE_GLYPH_METRICS {
        let mut metrics = DWRITE_GLYPH_METRICS::default();
        // SAFETY: one glyph in, one metric out.
        let _ = unsafe {
            self.face(id)
                .face
                .GetDesignGlyphMetrics(&glyph, 1, &mut metrics, false)
        };
        metrics
    }

    fn lay_out(&mut self, text: &str, spans: &[Span]) -> Result<Vec<(f32, f32, Vec<Glyph>)>> {
        // The byte offset of each UTF-16 unit. Breaks become zero width spaces of the same length.
        let mut units = Vec::with_capacity(text.len());
        let mut bytes = Vec::with_capacity(text.len() + 1);
        for (at, c) in text.char_indices() {
            let c = if BREAKS.contains(&c) { '\u{200b}' } else { c };
            let mut pair = [0u16; 2];
            for unit in c.encode_utf16(&mut pair) {
                units.push(*unit);
                bytes.push(at as u32);
            }
        }
        bytes.push(text.len() as u32);
        let ends: Vec<usize> = spans
            .iter()
            .scan(0, |end, span| {
                *end += span.len;
                Some(*end)
            })
            .collect();
        let first = self.face(spans[0].font);
        // SAFETY: plain DirectWrite calls. `Draw` calls the collector back with `runs` while this waits.
        unsafe {
            let format = self.dwrite.CreateTextFormat(
                &first.family,
                &first.collection,
                first.weight,
                first.style,
                first.stretch,
                spans[0].size.0,
                &self.locale,
            )?;
            let layout =
                self.dwrite
                    .CreateTextLayout(&units, &format, f32::INFINITY, f32::INFINITY)?;
            let mut start = 0;
            for (index, (span, &end)) in spans.iter().zip(&ends).enumerate() {
                let end = bytes.partition_point(|&at| (at as usize) < end);
                let range = DWRITE_TEXT_RANGE {
                    startPosition: start as u32,
                    length: (end - start) as u32,
                };
                start = end;
                let face = self.face(span.font);
                layout.SetFontCollection(&face.collection, range)?;
                layout.SetFontFamilyName(&face.family, range)?;
                layout.SetFontWeight(face.weight, range)?;
                layout.SetFontStyle(face.style, range)?;
                layout.SetFontStretch(face.stretch, range)?;
                // Neighbors differ in size by one step, so ligatures don't form across them.
                let size = if index % 2 == 1 {
                    span.size.0.next_up()
                } else {
                    span.size.0
                };
                layout.SetFontSize(size, range)?;
                if let Some((_, typography)) = &face.features {
                    layout.SetTypography(typography, range)?;
                }
            }
            let collector = self.collector.clone();
            let mut runs = Runs {
                library: self,
                bytes: &bytes,
                spans,
                ends: &ends,
                runs: Vec::new(),
            };
            layout.Draw(Some((&raw mut runs).cast()), &collector, 0., 0.)?;
            Ok(runs.runs)
        }
    }

    fn analysis(
        &self,
        run: &DWRITE_GLYPH_RUN,
        scale: f32,
        (mode, grid): (DWRITE_RENDERING_MODE1, DWRITE_GRID_FIT_MODE),
        [x, y]: [f32; 2],
    ) -> Result<IDWriteGlyphRunAnalysis> {
        let transform = DWRITE_MATRIX {
            m11: scale,
            m22: scale,
            ..Default::default()
        };
        // SAFETY: the run and the transform outlive the call.
        Ok(unsafe {
            self.dwrite.CreateGlyphRunAnalysis(
                run,
                Some(&transform),
                mode,
                DWRITE_MEASURING_MODE_NATURAL,
                grid,
                DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                x,
                y,
            )?
        })
    }

    fn glyph_analysis(&self, raster: &GlyphRaster, shift: f32) -> Result<IDWriteGlyphRunAnalysis> {
        let face = &self.face(raster.font).face;
        let transform = DWRITE_MATRIX {
            m11: raster.scale,
            m22: raster.scale,
            ..Default::default()
        };
        let mut way: (DWRITE_RENDERING_MODE1, DWRITE_GRID_FIT_MODE) = Default::default();
        // SAFETY: the transform outlives the call, which fills locals.
        unsafe {
            face.GetRecommendedRenderingMode(
                raster.size.0,
                96.,
                96.,
                Some(&transform),
                false,
                DWRITE_OUTLINE_THRESHOLD_ANTIALIASED,
                DWRITE_MEASURING_MODE_NATURAL,
                None::<&IDWriteRenderingParams>,
                &mut way.0,
                &mut way.1,
            )?;
        }
        // Outlines are for huge text, which analysis can't make.
        if way.0 == DWRITE_RENDERING_MODE1_OUTLINE {
            way.0 = DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC;
        }
        one_glyph(face, raster.glyph, raster.size.0, |run| {
            self.analysis(run, raster.scale, way, [shift, 0.])
        })
    }

    // Returns straight BGRA.
    fn color_glyph(&self, raster: &GlyphRaster, area: RECT) -> Result<Vec<u8>> {
        let face = &self.face(raster.font).face;
        let transform = DWRITE_MATRIX {
            m11: raster.scale,
            m22: raster.scale,
            ..Default::default()
        };
        let [_, _, width, height] = extent(area).map(|side| side as usize);
        // Premultiplied BGRA while layers add up.
        let mut paint = vec![[0f32; 4]; width * height];
        one_glyph(face, raster.glyph, raster.size.0, |run| {
            // SAFETY: the run and the transform outlive the call, and each layer stays valid until the
            // enumerator moves on.
            unsafe {
                let layers = IDWriteFactory2::TranslateColorGlyphRun(
                    &self.dwrite,
                    0.,
                    0.,
                    run,
                    None,
                    DWRITE_MEASURING_MODE_NATURAL,
                    Some(&transform),
                    0,
                )?;
                while layers.MoveNext()?.as_bool() {
                    let layer = &*layers.GetCurrentRun()?;
                    let way = (
                        DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
                        DWRITE_GRID_FIT_MODE_DEFAULT,
                    );
                    let origin = [layer.baselineOriginX, layer.baselineOriginY];
                    let analysis = self.analysis(&layer.glyphRun, raster.scale, way, origin)?;
                    let (bounds, coverage) = alpha(&analysis, None)?;
                    // Palette entry 0xFFFF is the text color, which emoji take as black.
                    let ink = match layer.paletteIndex {
                        0xFFFF => [0., 0., 0., 1.],
                        _ => {
                            let c = layer.runColor;
                            [c.b, c.g, c.r, c.a]
                        }
                    };
                    let [left, top, span, _] = extent(bounds);
                    for (at, &covered) in coverage.iter().enumerate() {
                        let x = left + (at % span as usize) as i32 - area.left;
                        let y = top + (at / span as usize) as i32 - area.top;
                        if covered == 0
                            || !(0..width as i32).contains(&x)
                            || !(0..height as i32).contains(&y)
                        {
                            continue;
                        }
                        let a = f32::from(covered) / 255. * ink[3];
                        let pixel = &mut paint[y as usize * width + x as usize];
                        for (value, ink) in pixel.iter_mut().zip([ink[0], ink[1], ink[2], 1.]) {
                            *value = ink * a + *value * (1. - a);
                        }
                    }
                }
            }
            Ok(())
        })?;
        let mut bgra = vec![0u8; paint.len() * 4];
        for (out, [b, g, r, a]) in bgra.as_chunks_mut::<4>().0.iter_mut().zip(paint) {
            let straight = |value: f32| match a > 0. {
                true => (value / a * 255.).round().clamp(0., 255.) as u8,
                false => 0,
            };
            *out = [
                straight(b),
                straight(g),
                straight(r),
                (a * 255.).round() as u8,
            ];
        }
        Ok(bgra)
    }
}

// Grayscale analyses give their coverage in the aliased texture type, one byte per pixel.
const COVERAGE: DWRITE_TEXTURE_TYPE = DWRITE_TEXTURE_ALIASED_1x1;

fn extent(rect: RECT) -> [i32; 4] {
    [
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    ]
}

// Empty bounds give no bytes.
fn alpha(analysis: &IDWriteGlyphRunAnalysis, area: Option<RECT>) -> Result<(RECT, Vec<u8>)> {
    // SAFETY: plain analysis calls, and the buffer has one byte for each pixel of the area.
    unsafe {
        let area = match area {
            Some(area) => area,
            None => analysis.GetAlphaTextureBounds(COVERAGE)?,
        };
        let [_, _, width, height] = extent(area);
        let mut bytes = vec![0u8; (width.max(0) * height.max(0)) as usize];
        if !bytes.is_empty() {
            analysis.CreateAlphaTexture(COVERAGE, &area, &mut bytes)?;
        }
        Ok((area, bytes))
    }
}

struct Runs<'a> {
    library: &'a mut Library,
    bytes: &'a [u32],
    spans: &'a [Span],
    ends: &'a [usize],
    runs: Vec<(f32, f32, Vec<Glyph>)>,
}

impl Runs<'_> {
    fn span_at(&self, byte: u32) -> &Span {
        let index = self.ends.partition_point(|&end| end <= byte as usize);
        &self.spans[index.min(self.spans.len() - 1)]
    }

    // Glyph arrays are in logical order. A right-to-left run draws leftward from its origin.
    fn add(&mut self, origin: f32, run: &DWRITE_GLYPH_RUN, about: &DWRITE_GLYPH_RUN_DESCRIPTION) {
        let count = run.glyphCount as usize;
        if count == 0 || run.glyphIndices.is_null() || run.glyphAdvances.is_null() {
            return;
        }
        // SAFETY: DirectWrite passes arrays of the run's glyph count and the description's length.
        let (ids, advances, offsets, clusters) = unsafe {
            (
                slice::from_raw_parts(run.glyphIndices, count),
                slice::from_raw_parts(run.glyphAdvances, count),
                (!run.glyphOffsets.is_null())
                    .then(|| slice::from_raw_parts(run.glyphOffsets, count)),
                match about.clusterMap.is_null() {
                    true => &[][..],
                    false => slice::from_raw_parts(about.clusterMap, about.stringLength as usize),
                },
            )
        };
        let start = about.textPosition as usize;
        let Some(&first) = self.bytes.get(start) else {
            return;
        };
        let mut sources = vec![first; count];
        let mut unit = 0;
        while unit < clusters.len() {
            let glyph = usize::from(clusters[unit]);
            let next = clusters[unit..]
                .iter()
                .position(|&other| usize::from(other) != glyph)
                .map_or(clusters.len(), |len| unit + len);
            let end = clusters
                .get(next)
                .map_or(count, |&other| usize::from(other));
            if let (Some(&byte), Some(glyphs)) = (
                self.bytes.get(start + unit),
                sources.get_mut(glyph..end.min(count)),
            ) {
                glyphs.fill(byte);
            }
            unit = next;
        }
        let Some(face) = run.fontFace.as_ref() else {
            return;
        };
        let span = self.span_at(first).font;
        let Some(font) = self.library.found(face, span) else {
            return;
        };
        let color = self.library.face(font).color.then(|| {
            let face = self.library.face(font).face.clone();
            (self.library.dwrite.clone(), face)
        });
        let leftward = run.bidiLevel & 1 == 1;
        let total: f32 = advances.iter().sum();
        let left = if leftward { origin - total } else { origin };
        let mut pen = left;
        let mut glyphs = Vec::with_capacity(count);
        for step in 0..count {
            let at = if leftward { count - 1 - step } else { step };
            let offset = offsets.map_or_else(DWRITE_GLYPH_OFFSET::default, |offsets| offsets[at]);
            let along = if leftward {
                -offset.advanceOffset
            } else {
                offset.advanceOffset
            };
            let source = sources[at];
            glyphs.push(Glyph {
                x: px(pen + along),
                y: px(-offset.ascenderOffset),
                size: self.span_at(source).size,
                font,
                index: source,
                id: ids[at],
                emoji: color
                    .as_ref()
                    .is_some_and(|(factory, face)| is_color_glyph(factory, face, ids[at])),
            });
            pen += advances[at];
        }
        self.runs.push((left, left + total, glyphs));
    }
}

#[implement(IDWriteTextRenderer)]
struct Collector;

#[allow(non_snake_case)]
impl IDWritePixelSnapping_Impl for Collector_Impl {
    // Glyphs keep their fractional positions; painting snaps them.
    fn IsPixelSnappingDisabled(&self, _: *const c_void) -> windows::core::Result<BOOL> {
        Ok(true.into())
    }

    fn GetCurrentTransform(&self, _: *const c_void, transform: *mut DWRITE_MATRIX) -> Done {
        let identity = DWRITE_MATRIX {
            m11: 1.,
            m22: 1.,
            ..Default::default()
        };
        // SAFETY: DirectWrite passes a matrix to fill.
        unsafe { transform.write(identity) };
        Ok(())
    }

    fn GetPixelsPerDip(&self, _: *const c_void) -> windows::core::Result<f32> {
        Ok(1.)
    }
}

#[allow(non_snake_case)]
impl IDWriteTextRenderer_Impl for Collector_Impl {
    fn DrawGlyphRun(
        &self,
        context: *const c_void,
        x: f32,
        _: f32,
        _: DWRITE_MEASURING_MODE,
        run: *const DWRITE_GLYPH_RUN,
        about: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _: Ref<IUnknown>,
    ) -> Done {
        // SAFETY: `lay_out` passes its `Runs` as the context, and DirectWrite passes a run and its description.
        unsafe {
            if let (Some(runs), Some(run), Some(about)) = (
                context.cast_mut().cast::<Runs>().as_mut(),
                run.as_ref(),
                about.as_ref(),
            ) {
                runs.add(x, run, about);
            }
        }
        Ok(())
    }

    fn DrawUnderline(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_UNDERLINE,
        _: Ref<IUnknown>,
    ) -> Done {
        Ok(())
    }

    fn DrawStrikethrough(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_STRIKETHROUGH,
        _: Ref<IUnknown>,
    ) -> Done {
        Ok(())
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
    ) -> Done {
        Ok(())
    }
}

impl TextSystem {
    pub fn add_font(&self, bytes: Vec<u8>) -> Result<()> {
        let library = &mut *self.0.borrow_mut();
        // SAFETY: DirectWrite copies the data, since the reference has no owner object.
        unsafe {
            let file = library.loader.CreateInMemoryFontFileReference(
                &library.dwrite,
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                None::<&IUnknown>,
            )?;
            let (mut supported, mut kind, mut faces) = (BOOL(0), DWRITE_FONT_FILE_TYPE(0), 0);
            file.Analyze(&mut supported, &mut kind, None, &mut faces)?;
            ensure!(
                supported.as_bool() && faces > 0,
                "Unsupported or invalid font data"
            );
            let first =
                library
                    .dwrite
                    .CreateFontFaceReference(&file, 0, DWRITE_FONT_SIMULATIONS_NONE)?;
            library.added_faces.AddFontFaceReference2(&first)?;
            library.added = library
                .dwrite
                .CreateFontCollectionFromFontSet(&library.added_faces.CreateFontSet()?)?;
        }
        Ok(())
    }

    pub fn families(&self) -> Vec<String> {
        let library = self.0.borrow();
        let mut names = Vec::new();
        for collection in [&library.system, &library.added] {
            // SAFETY: plain collection queries.
            unsafe {
                for index in 0..collection.GetFontFamilyCount() {
                    names.extend(
                        collection
                            .GetFontFamily(index)
                            .and_then(|family| family.GetFamilyNames())
                            .ok()
                            .and_then(|list| localized(&list, &library.locale)),
                    );
                }
            }
        }
        names
    }

    pub fn font(&self, font: &Font) -> Option<FontId> {
        self.0.borrow_mut().font(font)
    }

    pub fn metrics(&self, font: FontId) -> FaceMetrics {
        self.0.borrow().metrics(font)
    }

    pub fn glyph(&self, font: FontId, ch: char) -> Option<GlyphId> {
        let mut glyph = 0;
        // SAFETY: one code point in, one glyph out.
        unsafe {
            self.0
                .borrow()
                .face(font)
                .face
                .GetGlyphIndices(&u32::from(ch), 1, &mut glyph)
        }
        .ok()?;
        (glyph != 0).then_some(glyph)
    }

    pub fn advance(&self, font: FontId, glyph: GlyphId) -> f32 {
        let library = self.0.borrow();
        library.design(font, glyph).advanceWidth as f32 / library.face(font).per_em
    }

    pub fn ink(&self, font: FontId, glyph: GlyphId, size: f32) -> Option<[f32; 4]> {
        let library = self.0.borrow();
        let m = library.design(font, glyph);
        let scale = size / library.face(font).per_em;
        let bottom = m.verticalOriginY - m.advanceHeight as i32 + m.bottomSideBearing;
        Some(
            [
                m.leftSideBearing,
                m.advanceWidth as i32 - m.rightSideBearing,
                m.verticalOriginY - m.topSideBearing,
                bottom,
            ]
            .map(|value| value as f32 * scale),
        )
    }

    pub fn shape(&self, text: &str, font_size: Pixels, spans: &[Span]) -> Line {
        let library = &mut *self.0.borrow_mut();
        let mut line = Line {
            font_size,
            len: text.len(),
            ..Default::default()
        };
        for span in spans {
            let metrics = library.metrics(span.font);
            line.ascent = line.ascent.max(span.size * metrics.ascent);
            line.descent = line.descent.max(span.size * -metrics.descent);
        }
        if text.is_empty() || spans.is_empty() {
            return line;
        }
        let mut runs = match library.lay_out(text, spans) {
            Ok(runs) => runs,
            Err(error) => {
                eprintln!("apps: text layout: {error:#}");
                return line;
            }
        };
        runs.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, right, glyphs) in runs {
            line.width = line.width.max(px(right));
            line.glyphs.extend(glyphs);
        }
        line
    }

    pub fn raster_bounds(&self, glyph: &GlyphRaster) -> Result<Rect<i32>> {
        let analysis = self.0.borrow().glyph_analysis(glyph, 0.)?;
        // SAFETY: a plain analysis query.
        let [x, y, width, height] = extent(unsafe { analysis.GetAlphaTextureBounds(COVERAGE)? });
        if width <= 0 || height <= 0 {
            return Ok(Rect::default());
        }
        Ok(Rect::new(point(x, y), size(width, height)).outset(1))
    }

    pub fn rasterize(
        &self,
        glyph: &GlyphRaster,
        bounds: Rect<i32>,
    ) -> Result<(Size<i32>, Vec<u8>)> {
        if bounds.size.width <= 0 || bounds.size.height <= 0 {
            bail!("glyph bounds are empty");
        }
        let mut canvas = bounds.size;
        if glyph.subpixel > 0 {
            canvas.width += 1;
        }
        let area = texture_rect(Rect::new(bounds.origin, canvas));
        let library = self.0.borrow();
        if glyph.emoji
            && let Ok(pixels) = library.color_glyph(glyph, area)
        {
            return Ok((canvas, pixels));
        }
        // The shift is in device independent pixels, which the transform scales to device pixels.
        let shift = f32::from(glyph.subpixel) / f32::from(SUBPIXEL_STEPS) / glyph.scale;
        let (_, coverage) = alpha(&library.glyph_analysis(glyph, shift)?, Some(area))?;
        if !glyph.emoji {
            return Ok((canvas, coverage));
        }
        // An emoji without color layers is black with the coverage as alpha.
        let mut bgra = vec![0u8; coverage.len() * 4];
        for (pixel, covered) in bgra.as_chunks_mut::<4>().0.iter_mut().zip(coverage) {
            pixel[3] = covered;
        }
        Ok((canvas, bgra))
    }

    // Windows has no setting for heavier light text.
    pub fn dilation(&self, _: Color) -> u8 {
        0
    }

    // DirectWrite shares its font cache across the process, so there is nothing to load ahead.
    pub fn warm(_: Vec<Vec<String>>) {}
}
