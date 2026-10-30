mod element;
mod layout;
mod line;

pub use element::*;
pub use layout::*;
pub use line::*;

use crate::{Bounds, DevicePixels, Pixels, Point, Rgba, Size, platform, px};
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

/// Glyphs are rasterized at this many horizontal offsets within a pixel.
pub const SUBPIXEL_VARIANTS_X: u8 = 4;

/// The degree of blackness of a font, from 100 to 900, with 400 as normal.
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
    pub const THIN: Self = Self(100.);
    pub const LIGHT: Self = Self(300.);
    pub const NORMAL: Self = Self(400.);
    pub const MEDIUM: Self = Self(500.);
    pub const SEMIBOLD: Self = Self(600.);
    pub const BOLD: Self = Self(700.);
    pub const BLACK: Self = Self(900.);
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash, Default)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}

/// OpenType features, as tags and values.
#[derive(Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct FontFeatures(pub Rc<[(String, u32)]>);

impl FontFeatures {
    pub fn tag_value_list(&self) -> &[(String, u32)] {
        &self.0
    }
    pub fn is_calt_enabled(&self) -> Option<bool> {
        self.0
            .iter()
            .find(|(tag, _)| tag == "calt")
            .map(|(_, value)| *value == 1)
    }
}

/// Families to try, in order, when a glyph is missing from the font.
#[derive(Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct FontFallbacks(pub Rc<[String]>);

impl FontFallbacks {
    pub fn fallback_list(&self) -> &[String] {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Font {
    /// The family name. `.SystemUIFont` is the platform's interface font.
    pub family: SharedString,
    pub features: FontFeatures,
    pub fallbacks: Option<FontFallbacks>,
    pub weight: FontWeight,
    pub style: FontStyle,
}

impl Default for Font {
    fn default() -> Self {
        font(".SystemUIFont")
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

/// The measurements of a typeface, in font units.
#[derive(Clone, Copy, Debug)]
pub struct FontMetrics {
    pub units_per_em: u32,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub underline_position: f32,
    pub underline_thickness: f32,
    pub cap_height: f32,
    pub x_height: f32,
    pub bounding_box: Bounds<f32>,
    /// `xAvgCharWidth` from the OpenType `OS/2` table; zero when missing.
    pub average_width: f32,
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
    /// Two lines, one thickness apart.
    Double,
    /// Square dots, one thickness long and apart.
    Dotted,
    /// Dashes and gaps, two thicknesses long.
    Dashed,
    Wavy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UnderlineStyle {
    pub thickness: Pixels,
    pub color: Option<Rgba>,
    pub style: DecorationStyle,
    /// The distance from the baseline, like CSS `text-underline-offset`. `None` places it in the descent.
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
pub enum TextWrap {
    /// Each line takes as many words as fit.
    #[default]
    Wrap,
    /// Lines get about the same width.
    Balance,
    /// Lines avoid a ragged edge and a short last line.
    Pretty,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WhiteSpace {
    #[default]
    Normal,
    Nowrap,
}

/// A styled run of text.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TextRun {
    /// The length in UTF-8 bytes.
    pub len: usize,
    pub font: Font,
    pub color: Rgba,
    pub background_color: Option<Rgba>,
    pub underline: Option<UnderlineStyle>,
    pub strikethrough: Option<StrikethroughStyle>,
    /// Extra space after each character, like CSS `letter-spacing`.
    pub letter_spacing: Pixels,
    /// Extra space after each space, like CSS `word-spacing`.
    pub word_spacing: Pixels,
    /// A font size for this run instead of the line's.
    pub font_size: Option<Pixels>,
}

/// A run of text in one font, as the shaper sees it.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct FontRun {
    pub len: usize,
    pub font_id: FontId,
    pub letter_spacing: Pixels,
    pub word_spacing: Pixels,
    pub font_size: Pixels,
}

/// A glyph raster: the cache key of its atlas tile.
#[derive(Clone, Debug, PartialEq)]
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

/// The fonts of the app, shared by the text systems of its windows.
pub struct Fonts {
    pub(crate) platform: platform::TextSystem,
    fonts: RefCell<FxHashMap<Font, Option<FontId>>>,
    metrics: RefCell<FxHashMap<FontId, FontMetrics>>,
    raster_bounds: RefCell<FxHashMap<RenderGlyphParams, Bounds<DevicePixels>>>,
    /// Advances when fonts are added, so cached layouts made with the old fonts go away.
    generation: Cell<usize>,
}

/// Fonts, metrics and shaped lines, used on the main thread only. Each window has its own
/// cache of shaped lines over the app's fonts.
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

impl TextSystem {
    pub fn new() -> Self {
        Self {
            shared: Rc::new(Fonts {
                platform: platform::TextSystem::new(),
                fonts: RefCell::default(),
                metrics: RefCell::default(),
                raster_bounds: RefCell::default(),
                generation: Cell::new(0),
            }),
            font_runs: Cell::default(),
            lines: LineLayoutCache::default(),
            generation: Cell::new(0),
        }
    }

    /// A text system with its own line cache over the same fonts.
    pub(crate) fn for_window(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            font_runs: Cell::default(),
            lines: LineLayoutCache::default(),
            generation: Cell::new(self.shared.generation.get()),
        }
    }

    /// Drops the lines shaped with fonts that changed since.
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

    /// Adds font files. Cached layouts and font choices are dropped.
    pub fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.platform.add_fonts(fonts)?;
        self.fonts.borrow_mut().clear();
        self.shared.generation.set(self.shared.generation.get() + 1);
        Ok(())
    }

    /// The font, or the first fallback family that loads.
    pub fn resolve_font(&self, font: &Font) -> FontId {
        if let Some(Some(id)) = self.fonts.borrow().get(font) {
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
        // WARNING: a system without any of the fallback fonts has no text to show.
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

    pub fn typographic_bounds(
        &self,
        font_id: FontId,
        font_size: Pixels,
        character: char,
    ) -> Result<Bounds<Pixels>> {
        let glyph = self
            .platform
            .glyph_for_char(font_id, character)
            .with_context(|| format!("glyph not found for character '{character}'"))?;
        let bounds = self.platform.typographic_bounds(font_id, glyph)?;
        Ok(self.read_metrics(font_id, |metrics| {
            (bounds / metrics.units_per_em as f32 * font_size.0).map(px)
        }))
    }

    pub fn advance(&self, font_id: FontId, font_size: Pixels, ch: char) -> Result<Size<Pixels>> {
        let glyph = self
            .platform
            .glyph_for_char(font_id, ch)
            .with_context(|| format!("glyph not found for character '{ch}'"))?;
        let advance = self.platform.advance(font_id, glyph)? / self.units_per_em(font_id) as f32;
        Ok(advance.map(|value| px(value * font_size.0)))
    }

    /// The advance of `0`, the CSS `ch` unit.
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

    /// The OS/2 average character width; zero when the font has none.
    pub fn average_width(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| {
            metrics.scale(metrics.average_width, font_size)
        })
    }

    pub fn ascent(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| metrics.scale(metrics.ascent, font_size))
    }

    /// Below the baseline, so usually negative.
    pub fn descent(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| metrics.scale(metrics.descent, font_size))
    }

    pub(crate) fn raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        if let Some(bounds) = self.raster_bounds.borrow().get(params) {
            return Ok(*bounds);
        }
        let bounds = self.platform.glyph_raster_bounds(params)?;
        self.raster_bounds
            .borrow_mut()
            .insert(params.clone(), bounds);
        Ok(bounds)
    }

    pub(crate) fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        let bounds = self.raster_bounds(params)?;
        self.platform.rasterize_glyph(params, bounds)
    }

    /// Drops the layouts that were not used since the last frame.
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
        let mut last: Option<&TextRun> = None;
        for (len, run) in runs {
            // Background colors do not change glyphs, so they never split a font run.
            let decoration_changed = !last.is_some_and(|last| {
                last.color == run.color
                    && last.underline == run.underline
                    && last.strikethrough == run.strikethrough
                    && (glyphs_only || last.background_color == run.background_color)
            });
            last = Some(run);
            let font_id = self.resolve_font(&run.font);
            let size = run.font_size.unwrap_or(font_size);
            match font_runs.last_mut() {
                Some(font_run)
                    if font_run.font_id == font_id
                        && font_run.letter_spacing == run.letter_spacing
                        && font_run.word_spacing == run.word_spacing
                        && font_run.font_size == size
                        && !decoration_changed =>
                {
                    font_run.len += len
                }
                _ => font_runs.push(FontRun {
                    len,
                    font_id,
                    letter_spacing: run.letter_spacing,
                    word_spacing: run.word_spacing,
                    font_size: size,
                }),
            }
        }
        font_runs
    }

    /// Lays out one line, without newlines.
    pub fn layout_line(
        &self,
        text: &str,
        font_size: Pixels,
        runs: &[TextRun],
        force_width: Option<Pixels>,
    ) -> Rc<LineLayout> {
        let font_runs = self.font_runs(runs.iter().map(|run| (run.len, run)), font_size, true);
        let layout =
            self.lines()
                .layout_line(&self.platform, text, font_size, &font_runs, force_width);
        self.font_runs.set(font_runs);
        layout
    }

    /// Shapes one line, without newlines, for painting.
    pub fn shape_line(
        &self,
        text: SharedString,
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
        let layout = self.layout_line(&text, font_size, runs, force_width);
        ShapedLine {
            layout,
            text,
            decoration_runs,
        }
    }

    /// Shapes text with newlines, wrapping each line at `wrap_width`.
    pub fn shape_text(
        &self,
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
        wrap_width: Option<Pixels>,
        line_clamp: Option<usize>,
    ) -> Result<Vec<WrappedLine>> {
        self.shape_text_indented(
            text,
            font_size,
            runs,
            wrap_width,
            line_clamp,
            px(0.),
            TextWrap::Wrap,
        )
    }

    /// Like [`Self::shape_text`], but the first line is shortened by `indent`, like CSS
    /// `text-indent`, and lines break with `wrap`.
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
    ) -> Result<Vec<WrappedLine>> {
        let mut runs = runs.iter().filter(|run| run.len > 0).cloned().peekable();
        let mut lines = Vec::new();
        let mut wrapped = 0;
        let mut start = 0;
        let count = text.split('\n').count();
        for (index, line_text) in text.split('\n').enumerate() {
            let end = start + line_text.len();
            let mut pieces = Vec::new();
            let mut decoration_runs = Vec::<DecorationRun>::new();
            let mut run_start = start;
            while run_start < end {
                let Some(run) = runs.peek_mut() else {
                    break;
                };
                let len = (end - run_start).min(run.len);
                match decoration_runs.last_mut() {
                    Some(last) if last.same(run) => last.len += len as u32,
                    _ => decoration_runs.push(DecorationRun::new(run, len)),
                }
                pieces.push((len, run.clone()));
                run.len -= len;
                if run.len == 0 {
                    runs.next();
                }
                run_start += len;
            }
            let font_runs = self.font_runs(
                pieces.iter().map(|(len, run)| (*len, run)),
                font_size,
                false,
            );
            let line_text: SharedString = if count == 1 {
                text.clone()
            } else {
                line_text.into()
            };
            let layout = self.lines().layout_wrapped_line(
                &self.platform,
                &line_text,
                font_size,
                &font_runs,
                wrap_width,
                line_clamp.map(|max| max.saturating_sub(wrapped)),
                if index == 0 { indent } else { px(0.) },
                wrap,
            );
            self.font_runs.set(font_runs);
            wrapped += layout.wrap_boundaries.len();
            lines.push(WrappedLine {
                layout,
                decoration_runs,
                text: line_text,
            });
            // Skips the newline.
            if let Some(run) = runs.peek_mut() {
                run.len -= 1;
                if run.len == 0 {
                    runs.next();
                }
            }
            start = end + 1;
        }
        Ok(lines)
    }
}

/// A run of one text color and decoration.
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
