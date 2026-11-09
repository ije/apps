mod layout;
mod line;
mod paragraph;

pub use layout::*;
pub use line::*;
pub use paragraph::*;

use crate::{Bounds, Pixels, Point, Rgba, Size, platform, px};
use anyhow::{Context as _, Result};
use rustc_hash::FxHashMap;
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    hash::{Hash, Hasher},
    rc::Rc,
};

pub type SharedString = Rc<str>;

#[derive(Hash, PartialEq, Eq, Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct FontId(pub usize);

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[repr(C)]
pub struct GlyphId(pub u32);

pub const SUBPIXEL_VARIANTS_X: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct FontWeight(pub f32);

impl Default for FontWeight {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl Hash for FontWeight {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl Eq for FontWeight {}

impl FontWeight {
    pub const NORMAL: Self = Self(400.);
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash, Default)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct FontFeatures(pub Rc<[(String, u32)]>);

impl FontFeatures {
    pub fn tag_value_list(&self) -> &[(String, u32)] {
        &self.0
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct FontFallbacks(pub Rc<[String]>);

impl FontFallbacks {
    pub fn fallback_list(&self) -> &[String] {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Font {
    pub family: SharedString,
    pub features: FontFeatures,
    pub fallbacks: Option<FontFallbacks>,
    pub weight: FontWeight,
    pub style: FontStyle,
}

thread_local! {
    pub(crate) static SYSTEM_UI_FONT: SharedString = ".SystemUIFont".into();
}

impl Default for Font {
    fn default() -> Self {
        font(SYSTEM_UI_FONT.with(Rc::clone))
    }
}

pub fn font(family: impl Into<SharedString>) -> Font {
    Font {
        family: family.into(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: FontWeight::default(),
        style: FontStyle::default(),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FontMetrics {
    pub units_per_em: u32,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub cap_height: f32,
    pub x_height: f32,
    pub bounding_box: Bounds<f32>,
    pub tall_ascent: bool,
    pub em_ascent: f32,
}

impl FontMetrics {
    fn scale(&self, value: f32, font_size: Pixels) -> Pixels {
        Pixels(value / self.units_per_em as f32 * font_size.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DecorationStyle {
    #[default]
    Solid,
    Double,
    Dotted,
    Dashed,
    Wavy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UnderlineStyle {
    pub thickness: Pixels,
    pub color: Option<Rgba>,
    pub style: DecorationStyle,
    pub offset: Option<Pixels>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StrikethroughStyle {
    pub thickness: Pixels,
    pub color: Option<Rgba>,
    pub style: DecorationStyle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WordBreak {
    #[default]
    Normal,
    BreakWord,
    Anywhere,
    BreakAll,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextWrap {
    #[default]
    Wrap,
    Balance,
    Pretty,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WhiteSpace {
    #[default]
    Normal,
    Nowrap,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct TextRun {
    pub len: usize,
    pub font: Font,
    pub color: Rgba,
    pub background_color: Option<Rgba>,
    pub underline: Option<UnderlineStyle>,
    pub strikethrough: Option<StrikethroughStyle>,
    pub letter_spacing: Pixels,
    pub word_spacing: Pixels,
    pub font_size: Option<Pixels>,
    pub inset: [Pixels; 2],
    pub nowrap: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct FontRun {
    pub len: usize,
    pub font_id: FontId,
    pub letter_spacing: Pixels,
    pub word_spacing: Pixels,
    pub font_size: Pixels,
    pub inset: [Pixels; 2],
    pub nowrap: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RenderGlyphParams {
    pub font_id: FontId,
    pub glyph_id: GlyphId,
    pub font_size: Pixels,
    pub subpixel_variant: Point<u8>,
    pub scale_factor: f32,
    pub is_emoji: bool,
    pub dilation: u8,
}

impl Eq for RenderGlyphParams {}

impl Hash for RenderGlyphParams {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.font_id.0.hash(state);
        self.glyph_id.0.hash(state);
        self.font_size.0.to_bits().hash(state);
        self.subpixel_variant.hash(state);
        self.scale_factor.to_bits().hash(state);
        self.is_emoji.hash(state);
        self.dilation.hash(state);
    }
}

pub struct Fonts {
    pub(crate) platform: platform::TextSystem,
    fonts: RefCell<FxHashMap<Font, Option<FontId>>>,
    // WARNING: found by the pointers of their family and features, since hashing a whole font for every run costs more
    // than its shaping cache lookups.
    recent: RefCell<Vec<(Font, FontId)>>,
    metrics: RefCell<FxHashMap<FontId, FontMetrics>>,
    generation: Cell<usize>,
}

pub struct TextSystem {
    shared: Rc<Fonts>,
    font_runs: Cell<Vec<FontRun>>,
    lines: LineLayoutCache,
    generation: Cell<usize>,
}

const FALLBACKS: [&str; 6] = [
    "Helvetica",
    "Segoe UI",
    "Noto Sans",
    "DejaVu Sans",
    "Ubuntu",
    "Arial",
];

impl std::ops::Deref for TextSystem {
    type Target = Fonts;
    fn deref(&self) -> &Fonts {
        &self.shared
    }
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new()
    }
}

pub fn warm(lists: Vec<Vec<String>>) {
    #[cfg(target_os = "macos")]
    if !lists.is_empty() {
        let _ = std::thread::Builder::new()
            .name("apps-fonts".into())
            .spawn(move || {
                let text = TextSystem::new();
                let names: FxHashMap<String, String> = text
                    .all_font_names()
                    .into_iter()
                    .map(|name| (name.to_lowercase(), name))
                    .collect();
                for list in lists {
                    if let Some(name) = list
                        .iter()
                        .find_map(|family| names.get(&family.to_lowercase()))
                    {
                        text.resolve_font(&font(name.clone()));
                    }
                }
            });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = lists;
}

impl TextSystem {
    pub fn new() -> Self {
        Self {
            shared: Rc::new(Fonts {
                platform: platform::TextSystem::new(),
                fonts: RefCell::default(),
                recent: RefCell::default(),
                metrics: RefCell::default(),
                generation: Cell::new(0),
            }),
            font_runs: Cell::default(),
            lines: LineLayoutCache::default(),
            generation: Cell::new(0),
        }
    }

    pub(crate) fn for_window(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            font_runs: Cell::default(),
            lines: LineLayoutCache::default(),
            generation: Cell::new(self.shared.generation.get()),
        }
    }

    fn lines(&self) -> &LineLayoutCache {
        let generation = self.shared.generation.get();
        if self.generation.replace(generation) != generation {
            self.lines.clear();
        }
        &self.lines
    }

    pub fn all_font_names(&self) -> Vec<String> {
        let mut names = self.platform.all_font_names();
        names.sort_unstable();
        names.dedup();
        names
    }

    pub(crate) fn generation(&self) -> usize {
        self.shared.generation.get()
    }

    pub fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.platform.add_fonts(fonts)?;
        self.fonts.borrow_mut().clear();
        self.recent.borrow_mut().clear();
        self.shared.generation.set(self.shared.generation.get() + 1);
        Ok(())
    }

    pub fn resolve_font(&self, font: &Font) -> FontId {
        let same = |kept: &Font| {
            Rc::ptr_eq(&kept.family, &font.family)
                && Rc::ptr_eq(&kept.features.0, &font.features.0)
                && kept.fallbacks.as_ref().map(|list| Rc::as_ptr(&list.0))
                    == font.fallbacks.as_ref().map(|list| Rc::as_ptr(&list.0))
                && (kept.weight, kept.style) == (font.weight, font.style)
        };
        if let Some(&(_, id)) = self.recent.borrow().iter().find(|(kept, _)| same(kept)) {
            return id;
        }
        if let Some(Some(id)) = self.fonts.borrow().get(font) {
            let mut recent = self.recent.borrow_mut();
            if recent.len() == 8 {
                recent.remove(0);
            }
            recent.push((font.clone(), *id));
            return *id;
        }
        let lookup = |candidate: &Font| {
            if let Some(id) = self.fonts.borrow().get(candidate) {
                return *id;
            }
            let id = self.platform.font_id(candidate).ok();
            self.fonts.borrow_mut().insert(candidate.clone(), id);
            id
        };
        let id = lookup(font).or_else(|| {
            FALLBACKS.iter().find_map(|family| {
                lookup(&Font {
                    family: (*family).into(),
                    ..font.clone()
                })
            })
        });
        let id = id.unwrap_or_default();
        self.fonts.borrow_mut().insert(font.clone(), Some(id));
        id
    }

    fn read_metrics<T>(&self, font_id: FontId, read: impl FnOnce(&FontMetrics) -> T) -> T {
        if let Some(metrics) = self.metrics.borrow().get(&font_id) {
            return read(metrics);
        }
        let metrics = self.platform.font_metrics(font_id);
        self.metrics.borrow_mut().insert(font_id, metrics);
        read(&metrics)
    }

    pub fn bounding_box(&self, font_id: FontId, font_size: Pixels) -> Bounds<Pixels> {
        self.read_metrics(font_id, |metrics| {
            (metrics.bounding_box / metrics.units_per_em as f32 * font_size.0).map(px)
        })
    }

    pub fn advance(&self, font_id: FontId, font_size: Pixels, ch: char) -> Result<Size<Pixels>> {
        let glyph = self
            .platform
            .glyph_for_char(font_id, ch)
            .with_context(|| format!("glyph not found for character '{ch}'"))?;
        let advance = self.platform.advance(font_id, glyph)? / self.units_per_em(font_id) as f32;
        Ok(advance.map(|value| px(value * font_size.0)))
    }

    pub fn ch_advance(&self, font_id: FontId, font_size: Pixels) -> Result<Pixels> {
        Ok(self.advance(font_id, font_size, '0')?.width)
    }

    pub fn units_per_em(&self, font_id: FontId) -> u32 {
        self.read_metrics(font_id, |metrics| metrics.units_per_em)
    }

    pub fn cap_height(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| {
            metrics.scale(metrics.cap_height, font_size)
        })
    }

    pub fn x_height(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| {
            metrics.scale(metrics.x_height, font_size)
        })
    }

    pub fn ascent(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| metrics.scale(metrics.ascent, font_size))
    }

    pub fn em_ascent(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| px(metrics.em_ascent * font_size.0))
    }

    pub fn line_metrics(&self, font_id: FontId, font_size: Pixels) -> [Pixels; 3] {
        self.read_metrics(font_id, |metrics| {
            let scale = |value: f32| (value / metrics.units_per_em as f32 * font_size.0).round();
            let (ascent, descent) = (scale(metrics.ascent), scale(-metrics.descent));
            let tall = if metrics.tall_ascent {
                ((ascent + descent) * 0.15).round()
            } else {
                0.
            };
            [px(ascent + tall), px(descent), px(scale(metrics.line_gap))]
        })
    }

    pub fn baseline(
        &self,
        font_id: FontId,
        font_size: Pixels,
        line_height: Pixels,
        scale: f32,
    ) -> Pixels {
        let [ascent, descent, _] = self.line_metrics(font_id, font_size * scale);
        (((line_height * scale - ascent - descent) / 2.).floor() + ascent) / scale
    }

    pub fn descent(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| metrics.scale(metrics.descent, font_size))
    }

    pub(crate) fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        max: i32,
    ) -> Result<Option<crate::atlas::Raster<Vec<u8>>>> {
        let mut params = *params;
        let mut bounds = self.platform.glyph_raster_bounds(&params)?;
        let side = bounds.size.width.0.max(bounds.size.height.0);
        let stretch = side as f32 / max as f32;
        if stretch > 1. {
            params.scale_factor /= stretch;
            params.subpixel_variant = crate::point(0, 0);
            bounds = self.platform.glyph_raster_bounds(&params)?;
        }
        if bounds.size.width.0 == 0 || bounds.size.height.0 == 0 {
            return Ok(None);
        }
        let (size, bytes) = self.platform.rasterize_glyph(&params, bounds)?;
        let shown = if stretch > 1. {
            let scale = |value: crate::DevicePixels| {
                crate::DevicePixels((value.0 as f32 * stretch).round() as i32)
            };
            Bounds::new(bounds.origin.map(scale), size.map(scale))
        } else {
            Bounds::new(bounds.origin, size)
        };
        Ok(Some((shown, size, bytes)))
    }

    pub fn glyph_bounds(
        &self,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
    ) -> Option<[f32; 4]> {
        #[cfg(target_os = "macos")]
        let bounds = self.platform.glyph_bounds(font_id, glyph_id, font_size.0);
        // WARNING: other platforms measure raster bounds at this scale, so the bounds are off by up to its inverse.
        #[cfg(not(target_os = "macos"))]
        let bounds = {
            const SCALE: f32 = 8.;
            let bounds = self
                .platform
                .glyph_raster_bounds(&RenderGlyphParams {
                    font_id,
                    glyph_id,
                    font_size,
                    subpixel_variant: crate::point(0, 0),
                    scale_factor: SCALE,
                    is_emoji: false,
                    dilation: 0,
                })
                .ok()?;
            let [x, y, w, h] = [
                bounds.origin.x.0,
                bounds.origin.y.0,
                bounds.size.width.0,
                bounds.size.height.0,
            ]
            .map(|v| v as f32 / SCALE);
            Some([x, x + w, -y, -(y + h)])
        };
        bounds.filter(|[left, right, top, bottom]| right > left && top > bottom)
    }

    pub fn glyph_pixels(
        &self,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        scale_factor: f32,
        subpixel: u8,
        is_emoji: bool,
    ) -> Option<(
        Point<crate::DevicePixels>,
        Size<crate::DevicePixels>,
        Vec<u8>,
    )> {
        self.rasterize_glyph(
            &RenderGlyphParams {
                font_id,
                glyph_id,
                font_size,
                subpixel_variant: crate::point(subpixel, 0),
                scale_factor,
                is_emoji,
                dilation: 0,
            },
            i32::MAX,
        )
        .ok()
        .flatten()
        .map(|(bounds, size, bytes)| (bounds.origin, size, bytes))
    }

    pub(crate) fn finish_frame(&self) {
        self.lines.finish_frame();
    }

    fn font_runs<'a>(
        &self,
        runs: impl Iterator<Item = (usize, &'a TextRun)>,
        font_size: Pixels,
        glyphs_only: bool,
    ) -> Vec<FontRun> {
        let mut font_runs = self.font_runs.take();
        font_runs.clear();
        let mut last: Option<(&TextRun, FontId)> = None;
        for (len, run) in runs {
            let decoration_changed = !last.is_some_and(|(last, _)| {
                last.color == run.color
                    && last.underline == run.underline
                    && last.strikethrough == run.strikethrough
                    && (glyphs_only || last.background_color == run.background_color)
            });
            let font_id = match last {
                Some((last, id)) if last.font == run.font => id,
                _ => self.resolve_font(&run.font),
            };
            last = Some((run, font_id));
            let size = run.font_size.unwrap_or(font_size);
            match font_runs.last_mut() {
                Some(font_run)
                    if font_run.font_id == font_id
                        && font_run.letter_spacing == run.letter_spacing
                        && font_run.word_spacing == run.word_spacing
                        && font_run.font_size == size
                        && font_run.inset[1] == Pixels::ZERO
                        && run.inset[0] == Pixels::ZERO
                        && font_run.nowrap == run.nowrap
                        && !decoration_changed =>
                {
                    font_run.len += len;
                    font_run.inset[1] = run.inset[1];
                }
                _ => font_runs.push(FontRun {
                    len,
                    font_id,
                    letter_spacing: run.letter_spacing,
                    word_spacing: run.word_spacing,
                    font_size: size,
                    inset: run.inset,
                    nowrap: run.nowrap,
                }),
            }
        }
        font_runs
    }

    pub fn layout_line(
        &self,
        text: &str,
        font_size: Pixels,
        runs: &[TextRun],
        force_width: Option<Pixels>,
    ) -> Rc<LineLayout> {
        let font_runs = self.font_runs(runs.iter().map(|run| (run.len, run)), font_size, true);
        let (_, layout) =
            self.lines()
                .layout_line(&self.platform, text, font_size, &font_runs, force_width);
        self.font_runs.set(font_runs);
        layout
    }

    pub fn shape_line(
        &self,
        text: &str,
        font_size: Pixels,
        runs: &[TextRun],
        force_width: Option<Pixels>,
    ) -> ShapedLine {
        debug_assert!(
            !text.contains('\n'),
            "text argument should not contain newlines"
        );
        let mut decoration_runs = Vec::<DecorationRun>::new();
        for run in runs {
            match decoration_runs.last_mut() {
                Some(last) if last.same(run) => last.len += run.len as u32,
                _ => decoration_runs.push(DecorationRun::new(run, run.len)),
            }
        }
        let layout = self.layout_line(text, font_size, runs, force_width);
        ShapedLine {
            layout,
            decoration_runs,
        }
    }

    pub fn shape_text(
        &self,
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
        wrap_width: Option<Pixels>,
        line_clamp: Option<usize>,
        word_break: WordBreak,
    ) -> Result<Vec<WrappedLine>> {
        self.shape_text_indented(
            text,
            font_size,
            runs,
            wrap_width,
            line_clamp,
            px(0.),
            TextWrap::Wrap,
            word_break,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn shape_text_indented(
        &self,
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
        wrap_width: Option<Pixels>,
        line_clamp: Option<usize>,
        indent: Pixels,
        wrap: TextWrap,
        word_break: WordBreak,
    ) -> Result<Vec<WrappedLine>> {
        let mut runs = runs.iter().filter(|run| run.len > 0).peekable();
        let mut used = 0;
        let mut pieces = Vec::new();
        let mut lines = Vec::new();
        let mut wrapped = 0;
        for (index, line_text) in text.split('\n').enumerate() {
            pieces.clear();
            let mut decoration_runs = Vec::<DecorationRun>::new();
            let mut left = line_text.len();
            while left > 0
                && let Some(&run) = runs.peek()
            {
                let len = left.min(run.len - used);
                match decoration_runs.last_mut() {
                    Some(last) if last.same(run) => last.len += len as u32,
                    _ => decoration_runs.push(DecorationRun::new(run, len)),
                }
                pieces.push((len, run));
                used += len;
                left -= len;
                if used == run.len {
                    runs.next();
                    used = 0;
                }
            }
            let font_runs = self.font_runs(pieces.iter().copied(), font_size, false);
            let (text, layout) = self.lines().layout_wrapped_line(
                &self.platform,
                line_text,
                font_size,
                &font_runs,
                wrap_width,
                line_clamp.map(|max| max.saturating_sub(wrapped)),
                if index == 0 { indent } else { px(0.) },
                wrap,
                word_break,
            );
            self.font_runs.set(font_runs);
            wrapped += layout.wrap_boundaries.len();
            lines.push(WrappedLine {
                layout,
                decoration_runs,
                text,
            });
            if let Some(&run) = runs.peek() {
                used += 1;
                if used == run.len {
                    runs.next();
                    used = 0;
                }
            }
        }
        Ok(lines)
    }
}

#[derive(Debug, Clone)]
pub struct DecorationRun {
    pub len: u32,
    pub color: Rgba,
    pub background_color: Option<Rgba>,
    pub underline: Option<UnderlineStyle>,
    pub strikethrough: Option<StrikethroughStyle>,
}

impl DecorationRun {
    fn new(run: &TextRun, len: usize) -> Self {
        Self {
            len: len as u32,
            color: run.color,
            background_color: run.background_color,
            underline: run.underline,
            strikethrough: run.strikethrough,
        }
    }
    fn same(&self, run: &TextRun) -> bool {
        self.color == run.color
            && self.underline == run.underline
            && self.strikethrough == run.strikethrough
            && self.background_color == run.background_color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_glyphs_raster_small_and_stretch_to_their_size() {
        let text = TextSystem::new();
        let font_id = text.resolve_font(&font("Helvetica"));
        let glyph_id = text.platform.glyph_for_char(font_id, 'W').unwrap();
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size: px(10000.),
            subpixel_variant: crate::point(2, 0),
            scale_factor: 2.,
            is_emoji: false,
            dilation: 0,
        };
        let full = text.platform.glyph_raster_bounds(&params).unwrap();
        assert!(full.size.width.0 > 16384);
        let max = crate::atlas::MAX_GLYPH_SIDE;
        let (shown, size, bytes) = text.rasterize_glyph(&params, max).unwrap().unwrap();
        assert!(size.width.0.max(size.height.0) <= max + 4);
        assert_eq!(bytes.len(), (size.width.0 * size.height.0) as usize);
        assert!(bytes.iter().any(|alpha| *alpha > 0));
        let stretch = full.size.width.0.max(full.size.height.0) / max + 1;
        for (shown, full) in [
            (shown.origin.x, full.origin.x),
            (shown.origin.y, full.origin.y),
            (shown.size.width, full.size.width),
            (shown.size.height, full.size.height),
        ] {
            assert!(
                (shown.0 - full.0).abs() <= 3 * stretch,
                "{shown:?} vs {full:?}"
            );
        }
        let small = RenderGlyphParams {
            font_size: px(16.),
            ..params
        };
        let (shown, size, _) = text.rasterize_glyph(&small, max).unwrap().unwrap();
        assert_eq!(shown.size, size);
        assert_eq!(
            shown.origin,
            text.platform.glyph_raster_bounds(&small).unwrap().origin
        );
    }
}
