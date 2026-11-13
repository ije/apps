mod breaks;
mod cache;
mod draw;
mod line;
mod paragraph;
mod raster;
mod style;

pub use breaks::Wrapping;
pub use cache::Span;
pub use draw::Decoration;
pub use line::*;
pub use paragraph::*;
pub use raster::*;
pub use style::*;

use crate::{Pixels, Rect, platform, px};
use anyhow::Result;
use cache::Cache;
use rustc_hash::FxHashMap;
use std::{
    cell::{Cell, RefCell},
    hash::{Hash, Hasher},
    rc::Rc,
};

pub type SharedString = Rc<str>;

pub const SUBPIXEL_STEPS: u8 = 4;

/// Dense from 0, never reused.
pub type FontId = u32;

pub type GlyphId = u16;

/// CSS weight, 1 to 1000.
pub type FontWeight = f32;

pub type Features = Rc<[(String, u32)]>;

#[derive(Clone, PartialEq)]
pub struct Font {
    pub family: Rc<str>,
    pub features: Features,
    pub weight: FontWeight,
    pub style: FontStyle,
}

thread_local! {
    // One shared name, so fonts of the default family compare by pointer in `face`.
    pub(crate) static SYSTEM_UI_FONT: Rc<str> = ".SystemUIFont".into();
}

impl Font {
    pub fn new(family: impl Into<Rc<str>>) -> Self {
        Self {
            family: family.into(),
            features: Features::default(),
            weight: 400.,
            style: FontStyle::Normal,
        }
    }
}

impl Default for Font {
    fn default() -> Self {
        Self::new(SYSTEM_UI_FONT.with(Rc::clone))
    }
}

// Fonts key maps, and a weight is a float.
impl Eq for Font {}

impl Hash for Font {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (
            &self.family,
            &self.features,
            self.weight.to_bits(),
            self.style,
        )
            .hash(state);
    }
}

/// In em units. `descent` is negative.
#[derive(Clone, Copy)]
pub struct FaceMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub cap_height: f32,
    pub x_height: f32,
    pub bounds: Rect<f32>,
    pub em_ascent: f32,
    pub tall: bool,
}

const STAND_INS: [&str; 6] = [
    "Helvetica",
    "Segoe UI",
    "Noto Sans",
    "DejaVu Sans",
    "Ubuntu",
    "Arial",
];

#[derive(Default)]
pub struct Fonts {
    // Backends are called from one thread at a time.
    platform: platform::TextSystem,
    resolved: RefCell<FxHashMap<Font, Option<FontId>>>,
    // Hashing a whole `Font` costs more than a shaping cache lookup, so recent fonts match by pointer first.
    recent: RefCell<Vec<(Font, FontId)>>,
    metrics: RefCell<Vec<Option<FaceMetrics>>>,
    generation: Cell<usize>,
}

impl Fonts {
    pub fn families(&self) -> Vec<String> {
        let mut names = self.platform.families();
        names.sort_unstable();
        names.dedup();
        names
    }

    pub fn face(&self, font: &Font) -> FontId {
        let same = |kept: &Font| {
            Rc::ptr_eq(&kept.family, &font.family)
                && Rc::ptr_eq(&kept.features, &font.features)
                && kept.weight == font.weight
                && kept.style == font.style
        };
        if let Some(&(_, id)) = self.recent.borrow().iter().find(|(kept, _)| same(kept)) {
            return id;
        }
        let known = self.resolved.borrow().get(font).copied();
        if let Some(Some(id)) = known {
            let mut recent = self.recent.borrow_mut();
            if recent.len() == 8 {
                recent.remove(0);
            }
            recent.push((font.clone(), id));
            return id;
        }
        let find = |wanted: &Font| {
            let known = self.resolved.borrow().get(wanted).copied();
            known.unwrap_or_else(|| {
                let id = self.platform.font(wanted);
                self.resolved.borrow_mut().insert(wanted.clone(), id);
                id
            })
        };
        let id = find(font)
            .or_else(|| {
                STAND_INS.iter().find_map(|family| {
                    find(&Font {
                        family: (*family).into(),
                        ..font.clone()
                    })
                })
            })
            .unwrap_or(0);
        self.resolved.borrow_mut().insert(font.clone(), Some(id));
        id
    }

    /// In em units: multiply by the font size.
    pub fn metrics(&self, font: FontId) -> FaceMetrics {
        let at = font as usize;
        if let Some(Some(metrics)) = self.metrics.borrow().get(at) {
            return *metrics;
        }
        let metrics = self.platform.metrics(font);
        let mut all = self.metrics.borrow_mut();
        if all.len() <= at {
            all.resize(at + 1, None);
        }
        all[at] = Some(metrics);
        metrics
    }

    pub fn glyph_box(&self, font: FontId, size: Pixels) -> Rect<Pixels> {
        (self.metrics(font).bounds * size.0).map(px)
    }

    pub fn advance(&self, font: FontId, size: Pixels, ch: char) -> Option<Pixels> {
        let glyph = self.platform.glyph(font, ch)?;
        Some(px(self.platform.advance(font, glyph) * size.0))
    }

    /// Rounded, as browsers build line boxes.
    pub fn line_metrics(&self, font: FontId, size: Pixels) -> [Pixels; 3] {
        let metrics = self.metrics(font);
        let [ascent, descent, gap] = [metrics.ascent, -metrics.descent, metrics.line_gap]
            .map(|value| (value * size.0).round());
        // Like WebKit, the classic Mac faces get 15% more ascent.
        let extra = if metrics.tall {
            ((ascent + descent) * 0.15).round()
        } else {
            0.
        };
        [px(ascent + extra), px(descent), px(gap)]
    }

    pub fn baseline(&self, font: FontId, size: Pixels, line_height: Pixels, scale: f32) -> Pixels {
        let [ascent, descent, _] = self.line_metrics(font, size * scale);
        (((line_height * scale - ascent - descent) / 2.).floor() + ascent) / scale
    }

    /// `[left, right, top, bottom]`, y up.
    pub fn glyph_bounds(&self, font: FontId, glyph: GlyphId, size: Pixels) -> Option<[f32; 4]> {
        self.platform
            .ink(font, glyph, size.0)
            .filter(|[left, right, top, bottom]| right > left && top > bottom)
    }
}

#[cfg(not(windows))]
fn nearest_value(
    values: impl Iterator<Item = f32> + Clone,
    wanted: f32,
    down: bool,
) -> Option<f32> {
    let below = values
        .clone()
        .filter(|&value| value <= wanted)
        .reduce(f32::max);
    let above = values.filter(|&value| value > wanted).reduce(f32::min);
    if down {
        below.or(above)
    } else {
        above.or(below)
    }
}

#[cfg(not(windows))]
pub(crate) fn best(
    faces: &[(f32, FontStyle, f32)],
    weight: f32,
    style: FontStyle,
) -> Option<usize> {
    let mut set: Vec<usize> = (0..faces.len()).collect();
    let stretch = nearest_value(set.iter().map(|&at| faces[at].2), 1., true)?;
    set.retain(|&at| faces[at].2 == stretch);
    let order = match style {
        FontStyle::Italic => [FontStyle::Italic, FontStyle::Oblique, FontStyle::Normal],
        FontStyle::Oblique => [FontStyle::Oblique, FontStyle::Italic, FontStyle::Normal],
        FontStyle::Normal => [FontStyle::Normal, FontStyle::Oblique, FontStyle::Italic],
    };
    let style = order
        .into_iter()
        .find(|style| set.iter().any(|&at| faces[at].1 == *style))?;
    set.retain(|&at| faces[at].1 == style);
    let weights = set.iter().map(|&at| faces[at].0);
    let has = |value: f32| weights.clone().any(|weight| weight == value);
    let chosen = if has(weight) {
        weight
    } else if (400. ..450.).contains(&weight) && has(500.) {
        500.
    } else if (450. ..=500.).contains(&weight) && has(400.) {
        400.
    } else {
        nearest_value(weights.clone(), weight, weight <= 500.)?
    };
    set.into_iter().find(|&at| faces[at].0 == chosen)
}

pub fn warm(lists: Vec<Vec<String>>) {
    if !lists.is_empty() {
        platform::TextSystem::warm(lists);
    }
}

#[derive(Default)]
pub struct TextSystem {
    fonts: Rc<Fonts>,
    cache: Cache,
    generation: Cell<usize>,
    spans: Cell<Vec<Span>>,
}

impl std::ops::Deref for TextSystem {
    type Target = Fonts;
    fn deref(&self) -> &Fonts {
        &self.fonts
    }
}

impl TextSystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn for_window(&self) -> Self {
        Self {
            generation: Cell::new(self.fonts.generation.get()),
            fonts: self.fonts.clone(),
            cache: Cache::default(),
            spans: Cell::default(),
        }
    }

    pub(crate) fn generation(&self) -> usize {
        self.fonts.generation.get()
    }

    pub fn add_font(&self, bytes: Vec<u8>) -> Result<()> {
        self.platform.add_font(bytes)?;
        self.resolved.borrow_mut().clear();
        self.recent.borrow_mut().clear();
        self.fonts.generation.set(self.fonts.generation.get() + 1);
        Ok(())
    }

    fn cache(&self) -> &Cache {
        let now = self.fonts.generation.get();
        if self.generation.replace(now) != now {
            self.cache.clear();
        }
        &self.cache
    }

    pub(crate) fn end_frame(&self) {
        self.cache.end_frame();
    }

    /// Runs split where their paint changes, so ligatures do not form across a color change.
    fn spans<'a>(
        &self,
        runs: impl Iterator<Item = (usize, &'a TextRun)>,
        font_size: Pixels,
    ) -> Vec<Span> {
        let mut spans = self.spans.take();
        spans.clear();
        let mut last: Option<(&TextRun, FontId)> = None;
        for (len, run) in runs {
            let repaint = last.is_none_or(|(last, _)| {
                last.color != run.color
                    || last.underline != run.underline
                    || last.strikethrough != run.strikethrough
                    || last.overline != run.overline
                    || last.background_color != run.background_color
            });
            let font = match last {
                Some((last, id)) if last.font == run.font => id,
                _ => self.face(&run.font),
            };
            last = Some((run, font));
            let size = run.font_size.unwrap_or(font_size);
            if let Some(span) = spans.last_mut()
                && !repaint
                && span.font == font
                && span.size == size
                && span.letter_spacing == run.letter_spacing
                && span.word_spacing == run.word_spacing
                && span.nowrap == run.nowrap
                && span.inset[1] == Pixels::ZERO
                && run.inset[0] == Pixels::ZERO
            {
                span.len += len;
                span.inset[1] = run.inset[1];
                continue;
            }
            spans.push(Span {
                len,
                font,
                size,
                letter_spacing: run.letter_spacing,
                word_spacing: run.word_spacing,
                inset: run.inset,
                nowrap: run.nowrap,
            });
        }
        spans
    }

    pub fn shape_line(&self, text: &str, font_size: Pixels, run: &TextRun) -> TextLine {
        debug_assert!(
            !text.contains('\n'),
            "shape_line takes text without newlines"
        );
        let spans = self.spans(std::iter::once((run.len, run)), font_size);
        let (text, line, _) = self.cache().get(&self.fonts, text, font_size, &spans, None);
        self.spans.set(spans);
        TextLine {
            line,
            breaks: Rc::default(),
            wrap: None,
            text,
            looks: looks(std::iter::once((run.len, run))),
        }
    }

    /// Like Chromium, a tab reaches the next multiple of `tab-size` spaces from the line start, or the one after
    /// when that is closer than half a space.
    fn tab_stops(
        &self,
        line: Rc<Line>,
        text: &str,
        pieces: &[(usize, &TextRun)],
        font_size: Pixels,
    ) -> Rc<Line> {
        let mut tabbed = (*line).clone();
        let mut shift = px(0.);
        for at in 0..tabbed.glyphs.len() {
            tabbed.glyphs[at].x += shift;
            let glyph = tabbed.glyphs[at];
            if text.as_bytes().get(glyph.index as usize) != Some(&b'\t') {
                continue;
            }
            let Some(run) = pieces
                .iter()
                .scan(0, |end, &(len, run)| {
                    *end += len;
                    Some((*end, run))
                })
                .find(|(end, _)| *end > glyph.index as usize)
                .map(|(_, run)| run)
            else {
                continue;
            };
            let size = run.font_size.unwrap_or(font_size);
            let space = self
                .advance(self.face(&run.font), size, ' ')
                .unwrap_or(size * 0.25)
                + run.letter_spacing
                + run.word_spacing;
            let stop = if run.tab_size < 0. {
                px(-run.tab_size)
            } else {
                space * run.tab_size
            };
            let end = tabbed.glyphs.get(at + 1).map_or(line.width, |next| next.x) + shift;
            if stop <= px(0.) {
                continue;
            }
            let mut next = stop * ((glyph.x / stop).floor() + 1.);
            if next - glyph.x < space * 0.5 {
                next += stop;
            }
            shift += next - end;
        }
        tabbed.width += shift;
        Rc::new(tabbed)
    }

    pub fn shape_text(
        &self,
        text: &str,
        font_size: Pixels,
        runs: &[TextRun],
        wrapping: &Wrapping,
    ) -> Vec<TextLine> {
        let mut cursor = RunCursor {
            runs,
            at: 0,
            used: 0,
        };
        let mut pieces = Vec::new();
        let (mut rows, mut lines) = (0, Vec::new());
        for (index, source) in text.split('\n').enumerate() {
            if wrapping.width.is_some()
                && rows > 0
                && wrapping.max_lines.is_some_and(|max| rows >= max)
            {
                break;
            }
            pieces.clear();
            cursor.take(source.len(), |len, run| pieces.push((len, run)));
            let spans = self.spans(pieces.iter().copied(), font_size);
            let wrap = wrapping.width.map(|width| Wrapping {
                width: Some(width),
                max_lines: wrapping.max_lines.map(|max| max.saturating_sub(rows)),
                indent: if index == 0 { wrapping.indent } else { px(0.) },
                ..*wrapping
            });
            let (text, line, breaks) =
                self.cache()
                    .get(&self.fonts, source, font_size, &spans, wrap.as_ref());
            self.spans.set(spans);
            let line = if source.contains('\t') {
                self.tab_stops(line, &text, &pieces, font_size)
            } else {
                line
            };
            rows += breaks.len() + 1;
            // The newline takes one byte of the runs.
            cursor.take(1, |_, _| {});
            lines.push(TextLine {
                line,
                breaks,
                wrap: wrapping.width,
                text,
                looks: looks(pieces.iter().copied()),
            });
        }
        lines
    }
}

struct RunCursor<'a> {
    runs: &'a [TextRun],
    at: usize,
    used: usize,
}

impl<'a> RunCursor<'a> {
    fn take(&mut self, mut len: usize, mut piece: impl FnMut(usize, &'a TextRun)) {
        while len > 0
            && let Some(run) = self.runs.get(self.at)
        {
            let part = len.min(run.len - self.used);
            if part > 0 {
                piece(part, run);
            }
            (self.used, len) = (self.used + part, len - part);
            if self.used == run.len {
                (self.at, self.used) = (self.at + 1, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_scale_and_stand_ins_cover_missing_families() {
        let text = TextSystem::new();
        let helvetica = text.face(&Font::new("Helvetica"));
        let missing = text.face(&Font::new("No Such Family h2o"));
        assert!(missing == helvetica);
        let [ascent, descent, _] = text.line_metrics(helvetica, px(16.));
        assert!(ascent > descent && descent > px(0.));
        let metrics = text.metrics(helvetica);
        assert!(
            metrics.x_height > 0.4 && metrics.cap_height > metrics.x_height && metrics.descent < 0.
        );
        assert!(
            text.advance(helvetica, px(16.), '0')
                .is_some_and(|width| width > px(5.))
        );
    }

    #[test]
    fn faces_follow_requests_and_recent_requests_skip_hashing() {
        let text = TextSystem::new();
        let regular = Font::new("Helvetica");
        let bold = Font {
            weight: 700.,
            ..regular.clone()
        };
        let (plain, heavy) = (text.face(&regular), text.face(&bold));
        assert!(plain != heavy && text.face(&regular) == plain && text.face(&bold) == heavy);
        let recent = text.recent.borrow();
        assert!(
            recent
                .iter()
                .any(|(kept, id)| Rc::ptr_eq(&kept.family, &regular.family) && *id == plain)
        );
        // The baseline snaps to device pixels at the scale.
        let baseline = text.baseline(plain, px(16.), px(20.), 2.);
        assert!(baseline > px(10.) && baseline < px(20.) && (baseline.0 * 2.).fract() == 0.);
    }

    #[test]
    fn classic_mac_faces_get_taller_line_boxes() {
        let text = TextSystem::new();
        for (family, tall) in [
            ("Helvetica", true),
            ("Times", true),
            ("Menlo", false),
            ("Georgia", false),
        ] {
            let id = text.face(&Font::new(family));
            let metrics = text.metrics(id);
            let [ascent, descent, _] = text.line_metrics(id, px(20.));
            let plain = (metrics.ascent * 20.).round();
            let extra = if tall {
                ((plain + descent.0) * 0.15).round()
            } else {
                0.
            };
            assert!(
                metrics.tall == tall && ascent.0 == plain + extra,
                "{family}"
            );
        }
    }
}
