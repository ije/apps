use super::{FontId, FontRun, GlyphId, SharedString, TextWrap, WordBreak};
use crate::{Pixels, Point, Size, platform, point, px};
use rustc_hash::FxHashMap;
use std::{
    borrow::Borrow,
    cell::{Cell, RefCell},
    hash::{Hash, Hasher},
    mem,
    rc::Rc,
};

#[derive(Default, Debug)]
pub struct LineLayout {
    pub font_size: Pixels,
    pub width: Pixels,
    pub ascent: Pixels,
    pub descent: Pixels,
    pub runs: Vec<ShapedRun>,
    pub len: usize,
}

#[derive(Debug, Clone)]
pub struct ShapedRun {
    pub font_id: FontId,
    pub font_size: Pixels,
    pub glyphs: Vec<ShapedGlyph>,
}

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub id: GlyphId,

    pub position: Point<Pixels>,

    pub index: usize,

    pub is_emoji: bool,
}

impl LineLayout {
    pub fn index_for_x(&self, x: Pixels) -> Option<usize> {
        if x >= self.width {
            None
        } else {
            for run in self.runs.iter().rev() {
                for glyph in run.glyphs.iter().rev() {
                    if glyph.position.x <= x {
                        return Some(glyph.index);
                    }
                }
            }
            Some(0)
        }
    }

    pub fn closest_index_for_x(&self, x: Pixels) -> usize {
        let mut prev_index = 0;
        let mut prev_x = px(0.);

        for run in self.runs.iter() {
            for glyph in run.glyphs.iter() {
                if glyph.position.x >= x {
                    if glyph.position.x - x < x - prev_x {
                        return glyph.index;
                    } else {
                        return prev_index;
                    }
                }
                prev_index = glyph.index;
                prev_x = glyph.position.x;
            }
        }

        if self.len == 1 {
            if x > self.width / 2. {
                return 1;
            } else {
                return 0;
            }
        }

        self.len
    }

    pub fn x_for_index(&self, index: usize) -> Pixels {
        for run in &self.runs {
            for glyph in &run.glyphs {
                if glyph.index >= index {
                    return glyph.position.x;
                }
            }
        }
        self.width
    }

    #[allow(clippy::too_many_arguments)]
    fn compute_wrap_boundaries(
        &self,
        text: &str,
        wrap_width: Pixels,
        max_lines: Option<usize>,
        indent: Pixels,
        wrap: TextWrap,
        word_break: WordBreak,
        nowrap: &[std::ops::Range<usize>],
    ) -> Vec<WrapBoundary> {
        let inside = |index: usize| nowrap.iter().any(|range| range.contains(&index));
        let mut prev_index = 0;
        let mut boundaries = Vec::new();
        let constrain = wrap != TextWrap::Wrap && max_lines.is_none();
        let mut breaks = Vec::new();
        let mut words_end = 0.;
        let mut first_non_whitespace_index = None;
        let mut last_candidate_index = None;
        let mut last_candidate_x = px(0.);
        let mut last_boundary = WrapBoundary {
            run_index: 0,
            glyph_index: 0,
        };
        let mut last_boundary_x = px(0.);
        let (mut prev_ch, mut hyphen_after) = ('\0', false);
        let mut glyphs = self
            .runs
            .iter()
            .enumerate()
            .flat_map(move |(run_index, run)| {
                run.glyphs
                    .iter()
                    .enumerate()
                    .map(move |(glyph_index, glyph)| {
                        let character = text[glyph.index..].chars().next().unwrap();
                        (
                            WrapBoundary {
                                run_index,
                                glyph_index,
                            },
                            character,
                            glyph.position.x,
                            glyph.index,
                        )
                    })
            })
            .peekable();

        while let Some((boundary, ch, x, index)) = glyphs.next() {
            if ch == '\n' {
                continue;
            }
            let joined = !nowrap.is_empty() && inside(index) && inside(prev_index);
            prev_index = index;

            if first_non_whitespace_index.is_some()
                && ch != ' '
                && !joined
                && (prev_ch == ' '
                    || prev_ch == '\u{fffc}'
                    || !is_word_char(ch)
                    || word_break == WordBreak::BreakAll
                    || hyphen_after && ch.is_alphabetic())
            {
                last_candidate_index = Some(boundary);
                last_candidate_x = x;
                if constrain {
                    breaks.push((boundary, x.0, words_end));
                }
            }

            if ch != ' ' && first_non_whitespace_index.is_none() {
                first_non_whitespace_index = Some(boundary);
            }

            let next_x = glyphs.peek().map_or(self.width, |(_, _, x, _)| *x);
            if ch != ' ' {
                words_end = next_x.0;
            }
            let width = next_x - last_boundary_x;
            let available = if boundaries.is_empty() {
                wrap_width - indent
            } else {
                wrap_width
            };

            if ch != ' '
                && width > available + px(0.01)
                && boundary > last_boundary
                && (last_candidate_index.is_some() || word_break != WordBreak::Normal && !joined)
            {
                if let Some(max_lines) = max_lines
                    && boundaries.len() >= max_lines.saturating_sub(1)
                {
                    break;
                }

                if let Some(last_candidate_index) = last_candidate_index.take() {
                    last_boundary = last_candidate_index;
                    last_boundary_x = last_candidate_x;
                } else {
                    last_boundary = boundary;
                    last_boundary_x = x;
                }
                boundaries.push(last_boundary);
            }
            hyphen_after = ch == '-' && prev_ch.is_alphanumeric();
            prev_ch = ch;
        }
        if !constrain || boundaries.is_empty() {
            return boundaries;
        }

        breaks.insert(
            0,
            (
                WrapBoundary {
                    run_index: 0,
                    glyph_index: 0,
                },
                0.,
                0.,
            ),
        );
        breaks.push((last_boundary, self.width.0, words_end));
        let n = breaks.len();
        let width = |from: usize, to: usize| {
            breaks[to].2 - breaks[from].1 + if from == 0 { indent.0 } else { 0. }
        };
        let raggedness = |width: f32, ideal: f32| 100. * ((width - ideal) / 15.).abs().powi(3);
        let max = wrap_width.0;
        let mut lines = vec![0];
        for boundary in &boundaries {
            let Some(index) = breaks
                .iter()
                .position(|(candidate, ..)| candidate == boundary)
            else {
                return boundaries;
            };
            lines.push(index);
        }
        lines.push(n - 1);
        let count = lines.len() - 1;
        let exact = wrap == TextWrap::Balance && count <= 12;
        let ideal = match wrap {
            TextWrap::Balance => {
                lines
                    .windows(2)
                    .map(|line| width(line[0], line[1]))
                    .sum::<f32>()
                    / count as f32
            }
            _ => max - 45.,
        };
        if wrap == TextWrap::Pretty && (1..n).any(|i| width(i - 1, i) > ideal) {
            return boundaries;
        }
        let layers = if exact { count + 1 } else { 2 };
        let mut cost = vec![f32::INFINITY; n * layers];
        let mut previous = vec![0; n * layers];
        cost[0] = 0.;
        let mut start = 0;
        let mut last_valid = 0;
        for i in 1..n {
            while start < i && width(start, i) > max {
                start += 1;
            }
            if wrap == TextWrap::Pretty && start > last_valid {
                let end = (last_valid + 2..n)
                    .take_while(|&end| width(last_valid, end) <= max)
                    .last()
                    .unwrap_or(last_valid + 1);
                cost[end * layers + 1] =
                    cost[last_valid * layers + 1].min(cost[last_valid * layers]);
                previous[end * layers + 1] = last_valid;
                last_valid = end;
            }
            for from in start..i {
                let line_width = width(from, i);
                let line =
                    if wrap == TextWrap::Pretty && i + 5 < n && (line_width - ideal).abs() > 45. {
                        f32::INFINITY
                    } else {
                        raggedness(line_width, ideal)
                    };
                for layer in 0..if exact { layers - 1 } else { layers } {
                    let to = if exact { layer + 1 } else { 1 };
                    let total = cost[from * layers + layer] + line;
                    let current = cost[i * layers + to];
                    if total < current
                        || exact && total.is_finite() && total - current <= current * 1e-6
                    {
                        cost[i * layers + to] = total;
                        previous[i * layers + to] = from;
                        last_valid = i;
                    }
                }
            }
        }
        let mut layer = layers - 1;
        if !cost[(n - 1) * layers + layer].is_finite() {
            return boundaries;
        }
        let mut constrained = Vec::new();
        let mut i = previous[(n - 1) * layers + layer];
        while i > 0 {
            constrained.push(breaks[i].0);
            layer = if exact { layer - 1 } else { 1 };
            i = previous[i * layers + layer];
        }
        constrained.reverse();
        constrained
    }
}

#[derive(Default, Debug)]
pub struct WrappedLineLayout {
    pub unwrapped_layout: Rc<LineLayout>,

    pub wrap_boundaries: Vec<WrapBoundary>,

    pub wrap_width: Option<Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WrapBoundary {
    pub run_index: usize,
    pub glyph_index: usize,
}

impl WrappedLineLayout {
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.unwrapped_layout.len
    }

    pub fn width(&self) -> Pixels {
        self.wrap_width
            .unwrap_or(Pixels::MAX)
            .min(self.unwrapped_layout.width)
    }

    pub fn size(&self, line_height: Pixels) -> Size<Pixels> {
        Size {
            width: self.width(),
            height: line_height * (self.wrap_boundaries.len() + 1),
        }
    }

    pub fn index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
    ) -> Result<usize, usize> {
        self.find_index_for_position(position, line_height, false)
    }

    pub fn closest_index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
    ) -> Result<usize, usize> {
        self.find_index_for_position(position, line_height, true)
    }

    fn find_index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
        closest: bool,
    ) -> Result<usize, usize> {
        let wrapped_line_index = (position.y / line_height) as usize;

        let (wrapped_line_start_index, wrapped_line_start_x) = if wrapped_line_index > 0 {
            let Some(line_start_boundary) = self.wrap_boundaries.get(wrapped_line_index - 1) else {
                return Err(0);
            };
            let run = &self.unwrapped_layout.runs[line_start_boundary.run_index];
            let glyph = &run.glyphs[line_start_boundary.glyph_index];
            (glyph.index, glyph.position.x)
        } else {
            (0, Pixels::ZERO)
        };

        let (wrapped_line_end_index, wrapped_line_end_x) =
            if wrapped_line_index < self.wrap_boundaries.len() {
                let next_wrap_boundary_index = wrapped_line_index;
                let next_wrap_boundary = self.wrap_boundaries[next_wrap_boundary_index];
                let run = &self.unwrapped_layout.runs[next_wrap_boundary.run_index];
                let glyph = &run.glyphs[next_wrap_boundary.glyph_index];
                (glyph.index, glyph.position.x)
            } else {
                (self.unwrapped_layout.len, self.unwrapped_layout.width)
            };

        let mut position_in_unwrapped_line = position;
        position_in_unwrapped_line.x += wrapped_line_start_x;
        if position_in_unwrapped_line.x < wrapped_line_start_x {
            Err(wrapped_line_start_index)
        } else if position_in_unwrapped_line.x >= wrapped_line_end_x {
            Err(wrapped_line_end_index)
        } else {
            if closest {
                Ok(self
                    .unwrapped_layout
                    .closest_index_for_x(position_in_unwrapped_line.x))
            } else {
                // WARNING: the shaper can place a trailing zero-width wrap boundary glyph slightly past
                // the line's width, so the row can extend past where `index_for_x` has glyphs.
                self.unwrapped_layout
                    .index_for_x(position_in_unwrapped_line.x)
                    .ok_or(wrapped_line_end_index)
            }
        }
    }

    pub fn position_for_index(&self, index: usize, line_height: Pixels) -> Option<Point<Pixels>> {
        let mut line_start_index = 0;
        let line_end_indices = self
            .wrap_boundaries
            .iter()
            .map(|wrap_boundary| {
                let run = &self.unwrapped_layout.runs[wrap_boundary.run_index];
                let glyph = &run.glyphs[wrap_boundary.glyph_index];
                glyph.index
            })
            .chain([self.len()])
            .enumerate();
        for (line_index, line_end_index) in line_end_indices {
            let line_y = line_index as f32 * line_height;
            if index < line_start_index {
                break;
            } else if index > line_end_index {
                line_start_index = line_end_index;
                continue;
            } else {
                let line_start_x = if line_index == 0 {
                    px(0.)
                } else {
                    self.unwrapped_layout.x_for_index(line_start_index)
                };
                let x = self.unwrapped_layout.x_for_index(index) - line_start_x;
                return Some(point(x, line_y));
            }
        }

        None
    }
}

#[derive(Default)]
pub(crate) struct LineLayoutCache {
    state: RefCell<Lines>,
}

const MAX_AGE: u32 = 60;
const MAX_BYTES: usize = 4 << 20;

type Entries<T> = FxHashMap<Rc<CacheKey>, (Rc<T>, Cell<u32>, usize)>;

#[derive(Default)]
struct Lines {
    frame: u32,
    bytes: usize,
    lines: Entries<LineLayout>,
    wrapped: Entries<WrappedLineLayout>,
}

fn size_of_line(text: &str, layout: &LineLayout) -> usize {
    text.len()
        + layout
            .runs
            .iter()
            .map(|run| run.glyphs.len())
            .sum::<usize>()
            * mem::size_of::<ShapedGlyph>()
}

fn evict<T>(entries: &mut Entries<T>, bytes: &mut usize, keep: impl Fn(u32) -> bool) {
    entries.retain(|_, (_, used, size)| {
        keep(used.get()) || {
            *bytes -= *size;
            false
        }
    });
}

impl LineLayoutCache {
    pub fn clear(&self) {
        *self.state.borrow_mut() = Lines::default();
    }

    pub fn finish_frame(&self) {
        let Lines {
            frame,
            bytes,
            lines,
            wrapped,
        } = &mut *self.state.borrow_mut();
        let (current, age) = (*frame, if *bytes > MAX_BYTES { 1 } else { MAX_AGE });
        let keep = move |used: u32| current.wrapping_sub(used) < age;
        evict(lines, bytes, keep);
        evict(wrapped, bytes, keep);
        *frame = current.wrapping_add(1);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn layout_wrapped_line(
        &self,
        platform: &platform::TextSystem,
        text: &str,
        font_size: Pixels,
        runs: &[FontRun],
        wrap_width: Option<Pixels>,
        max_lines: Option<usize>,
        indent: Pixels,
        wrap: TextWrap,
        word_break: WordBreak,
    ) -> (SharedString, Rc<WrappedLineLayout>) {
        let key = &CacheKeyRef {
            text,
            font_size,
            runs,
            wrap_width,
            force_width: None,
            max_lines,
            indent,
            wrap,
            word_break,
        } as &dyn AsCacheKeyRef;
        {
            let state = self.state.borrow();
            if let Some((key, (layout, used, _))) = state.wrapped.get_key_value(key) {
                used.set(state.frame);
                return (key.text.clone(), layout.clone());
            }
        }
        let unwrapped_layout = self.layout_line(platform, text, font_size, runs, None);
        let wrap_boundaries = match wrap_width {
            Some(width) => {
                let mut end = 0;
                let nowrap: Vec<_> = runs
                    .iter()
                    .filter_map(|run| {
                        let start = end;
                        end += run.len;
                        run.nowrap.then_some(start..end)
                    })
                    .collect();
                unwrapped_layout.compute_wrap_boundaries(
                    text, width, max_lines, indent, wrap, word_break, &nowrap,
                )
            }
            None => Vec::new(),
        };
        let size = size_of_line(text, &unwrapped_layout)
            + wrap_boundaries.len() * mem::size_of::<WrapBoundary>();
        let layout = Rc::new(WrappedLineLayout {
            unwrapped_layout,
            wrap_boundaries,
            wrap_width,
        });
        let text: SharedString = text.into();
        let key = Rc::new(CacheKey {
            text: text.clone(),
            font_size,
            runs: runs.into(),
            wrap_width,
            force_width: None,
            max_lines,
            indent,
            wrap,
            word_break,
        });
        let state = &mut *self.state.borrow_mut();
        state.bytes += size;
        state
            .wrapped
            .insert(key, (layout.clone(), Cell::new(state.frame), size));
        (text, layout)
    }

    pub fn layout_line(
        &self,
        platform: &platform::TextSystem,
        text: &str,
        font_size: Pixels,
        runs: &[FontRun],
        force_width: Option<Pixels>,
    ) -> Rc<LineLayout> {
        let key = &CacheKeyRef {
            text,
            font_size,
            runs,
            wrap_width: None,
            force_width,
            max_lines: None,
            indent: px(0.),
            wrap: TextWrap::Wrap,
            word_break: WordBreak::Normal,
        } as &dyn AsCacheKeyRef;
        {
            let state = self.state.borrow();
            if let Some((layout, used, _)) = state.lines.get(key) {
                used.set(state.frame);
                return layout.clone();
            }
        }
        let mut layout = platform.layout_line(text, font_size, runs);
        if runs.iter().any(|run| {
            run.letter_spacing != px(0.) || run.word_spacing != px(0.) || run.inset != [px(0.); 2]
        }) {
            apply_spacing(&mut layout, text, runs);
        }
        if let Some(force_width) = force_width {
            apply_force_width_to_layout(&mut layout, force_width);
        }
        let size = size_of_line(text, &layout);
        let key = Rc::new(CacheKey {
            text: text.into(),
            font_size,
            runs: runs.into(),
            wrap_width: None,
            force_width,
            max_lines: None,
            indent: px(0.),
            wrap: TextWrap::Wrap,
            word_break: WordBreak::Normal,
        });
        let layout = Rc::new(layout);
        let state = &mut *self.state.borrow_mut();
        state.bytes += size;
        state
            .lines
            .insert(key, (layout.clone(), Cell::new(state.frame), size));
        layout
    }
}

// WARNING: combining marks, like Thai vowel signs or Arabic diacritics, are shaped at the x of their base
// character, so they must not take a cell. A glyph less than half a cell past the last base is one.
fn apply_force_width_to_layout(layout: &mut LineLayout, force_width: Pixels) {
    let mut glyph_pos: usize = 0;
    let mut last_base_shaped_x = px(f32::NEG_INFINITY);
    let mut last_base_actual_x = px(0.);

    for run in layout.runs.iter_mut() {
        for glyph in run.glyphs.iter_mut() {
            let shaped_x = glyph.position.x;

            if shaped_x > last_base_shaped_x + force_width * 0.5 {
                let forced_x = glyph_pos * force_width;
                if (shaped_x - forced_x).abs() > px(1.) {
                    glyph.position.x = forced_x;
                }
                last_base_shaped_x = shaped_x;
                last_base_actual_x = glyph.position.x;
                glyph_pos += 1;
            } else {
                glyph.position.x = last_base_actual_x + (shaped_x - last_base_shaped_x);
            }
        }
    }
}

fn apply_spacing(layout: &mut LineLayout, text: &str, runs: &[FontRun]) {
    let mut ends = Vec::with_capacity(runs.len());
    let mut end = 0;
    for run in runs {
        end += run.len;
        ends.push(end);
    }
    let spacing = |index: usize| {
        let run = &runs[ends
            .partition_point(|&end| end <= index)
            .min(runs.len() - 1)];
        let space = matches!(text[index..].chars().next(), Some(' ' | '\u{a0}'));
        run.letter_spacing + if space { run.word_spacing } else { px(0.) }
    };
    let run_of = |index: usize| {
        ends.partition_point(|&end| end <= index)
            .min(runs.len() - 1)
    };
    let mut shift = px(0.);
    let mut previous: Option<usize> = None;
    for run in &mut layout.runs {
        for glyph in &mut run.glyphs {
            match previous.filter(|&previous| previous != glyph.index) {
                Some(previous) => {
                    shift += spacing(previous);
                    let (from, to) = (run_of(previous), run_of(glyph.index));
                    if from != to {
                        shift += runs[from].inset[1];
                        shift += runs[from + 1..=to]
                            .iter()
                            .map(|run| run.inset[0])
                            .fold(px(0.), |sum, inset| sum + inset);
                    }
                }
                None if previous.is_none() => shift += runs[run_of(glyph.index)].inset[0],
                None => {}
            }
            previous = Some(glyph.index);
            glyph.position.x += shift;
        }
    }
    if let Some(previous) = previous {
        shift += runs[run_of(previous)].inset[1];
        shift += spacing(previous);
    }
    layout.width += shift;
}

trait AsCacheKeyRef {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_>;
}

#[derive(Clone, Debug, Eq)]
struct CacheKey {
    text: SharedString,
    font_size: Pixels,
    runs: Box<[FontRun]>,
    wrap_width: Option<Pixels>,
    force_width: Option<Pixels>,
    max_lines: Option<usize>,
    indent: Pixels,
    wrap: TextWrap,
    word_break: WordBreak,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
struct CacheKeyRef<'a> {
    text: &'a str,
    font_size: Pixels,
    runs: &'a [FontRun],
    wrap_width: Option<Pixels>,
    force_width: Option<Pixels>,
    max_lines: Option<usize>,
    indent: Pixels,
    wrap: TextWrap,
    word_break: WordBreak,
}

impl PartialEq for dyn AsCacheKeyRef + '_ {
    fn eq(&self, other: &dyn AsCacheKeyRef) -> bool {
        self.as_cache_key_ref() == other.as_cache_key_ref()
    }
}

impl Eq for dyn AsCacheKeyRef + '_ {}

impl Hash for dyn AsCacheKeyRef + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_cache_key_ref().hash(state)
    }
}

impl AsCacheKeyRef for CacheKey {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_> {
        CacheKeyRef {
            text: &self.text,
            font_size: self.font_size,
            runs: &self.runs,
            wrap_width: self.wrap_width,
            force_width: self.force_width,
            max_lines: self.max_lines,
            indent: self.indent,
            wrap: self.wrap,
            word_break: self.word_break,
        }
    }
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.as_cache_key_ref().eq(&other.as_cache_key_ref())
    }
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_cache_key_ref().hash(state);
    }
}

impl<'a> Borrow<dyn AsCacheKeyRef + 'a> for Rc<CacheKey> {
    fn borrow(&self) -> &(dyn AsCacheKeyRef + 'a) {
        self.as_ref() as &dyn AsCacheKeyRef
    }
}

impl AsCacheKeyRef for CacheKeyRef<'_> {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_> {
        *self
    }
}

pub(crate) fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '\u{00C0}'..='\u{024F}' | '\u{0400}'..='\u{04FF}')
        || matches!(c, '\u{1E00}'..='\u{1EFF}' | '\u{0300}'..='\u{036F}' | '\u{0980}'..='\u{09FF}')
        || matches!(
            c,
            '-' | '_'
                | '.'
                | '\''
                | '’'
                | '‘'
                | '$'
                | '%'
                | '@'
                | '#'
                | '^'
                | '~'
                | ','
                | '='
                | ':'
                | ';'
        )
        || matches!(c, '!' | ')' | ']' | '}' | '"' | '”' | '»' | '…' | '⋯')
        || matches!(c, '\u{202F}' | '\u{00A0}' | '\u{2011}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{point, px};

    fn layout(text: &str) -> LineLayout {
        LineLayout {
            font_size: px(10.),
            width: px(text.len() as f32 * 10.),
            runs: vec![ShapedRun {
                font_id: FontId(0),
                font_size: px(10.),
                glyphs: (0..text.len())
                    .map(|index| ShapedGlyph {
                        id: GlyphId(0),
                        position: point(px(index as f32 * 10.), px(0.)),
                        index,
                        is_emoji: false,
                    })
                    .collect(),
            }],
            len: text.len(),
            ..Default::default()
        }
    }

    #[test]
    fn objects_break_on_both_sides_and_nowrap_runs_hold_together() {
        let breaks = |text: &str, nowrap: &[std::ops::Range<usize>]| -> Vec<usize> {
            let mut shaped = layout(text);
            for glyph in shaped.runs.iter_mut().flat_map(|run| &mut run.glyphs) {
                glyph.index = text
                    .char_indices()
                    .nth(glyph.position.x.0 as usize / 10)
                    .map_or(text.len(), |(byte, _)| byte);
            }
            shaped.runs[0].glyphs.truncate(text.chars().count());
            shaped
                .compute_wrap_boundaries(
                    text,
                    px(0.),
                    None,
                    px(0.),
                    TextWrap::Wrap,
                    WordBreak::Normal,
                    nowrap,
                )
                .iter()
                .map(|boundary| boundary.glyph_index)
                .collect()
        };
        assert_eq!(breaks("ab\u{fffc}cd", &[]), [2, 3]);
        assert_eq!(breaks("ab cd ef", &[]), [3, 6]);
        assert_eq!(breaks("ab cd ef", std::slice::from_ref(&(0..5))), [6]);
    }

    #[test]
    fn hyphens_break_after_inside_words() {
        let text = "ab-cd x-12 -ef";
        let boundaries = layout(text).compute_wrap_boundaries(
            text,
            px(0.),
            None,
            px(0.),
            TextWrap::Wrap,
            WordBreak::Normal,
            &[],
        );
        let found: Vec<_> = boundaries
            .iter()
            .map(|boundary| boundary.glyph_index)
            .collect();
        assert_eq!(found, [3, 6, 11]);
    }

    #[test]
    fn balance_and_pretty_even_out_lines() {
        let text = ["aaaa"; 9].join(" ");
        let layout = layout(&text);
        for (wrap, starts) in [
            (TextWrap::Wrap, vec![20, 40]),
            (TextWrap::Balance, vec![15, 30]),
            (TextWrap::Pretty, vec![15, 30]),
        ] {
            let boundaries = layout.compute_wrap_boundaries(
                &text,
                px(200.),
                None,
                px(0.),
                wrap,
                WordBreak::Normal,
                &[],
            );
            let found: Vec<_> = boundaries
                .iter()
                .map(|boundary| boundary.glyph_index)
                .collect();
            assert_eq!(found, starts, "{wrap:?}");
        }
    }
}
