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

/// A laid out and styled line of text
#[derive(Default, Debug)]
pub struct LineLayout {
    /// The font size for this line
    pub font_size: Pixels,
    /// The width of the line
    pub width: Pixels,
    /// The ascent of the line
    pub ascent: Pixels,
    /// The descent of the line
    pub descent: Pixels,
    /// The shaped runs that make up this line
    pub runs: Vec<ShapedRun>,
    /// The length of the line in utf-8 bytes
    pub len: usize,
}

/// A run of text that has been shaped .
#[derive(Debug, Clone)]
pub struct ShapedRun {
    /// The font id for this run
    pub font_id: FontId,
    /// The font size of this run
    pub font_size: Pixels,
    /// The glyphs that make up this run
    pub glyphs: Vec<ShapedGlyph>,
}

/// A single glyph, ready to paint.
#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    /// The ID for this glyph, as determined by the text system.
    pub id: GlyphId,

    /// The position of this glyph in its containing line.
    pub position: Point<Pixels>,

    /// The index of this glyph in the original text.
    pub index: usize,

    /// Whether this glyph is an emoji
    pub is_emoji: bool,
}

impl LineLayout {
    /// The index for the character at the given x coordinate
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

    /// closest_index_for_x returns the character boundary closest to the given x coordinate
    /// (e.g. to handle aligning up/down arrow keys)
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

    /// The x position of the character at the given index
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

    /// The corresponding Font at the given index
    pub fn font_id_for_index(&self, index: usize) -> Option<FontId> {
        for run in &self.runs {
            for glyph in &run.glyphs {
                if glyph.index >= index {
                    return Some(run.font_id);
                }
            }
        }
        None
    }

    fn compute_wrap_boundaries(
        &self,
        text: &str,
        wrap_width: Pixels,
        max_lines: Option<usize>,
        indent: Pixels,
        wrap: TextWrap,
        word_break: WordBreak,
    ) -> Vec<WrapBoundary> {
        let mut boundaries = Vec::new();
        // Break opportunities for `wrap`: the boundary, its x, and the x where the words before it end.
        let constrain = wrap != TextWrap::Wrap && max_lines.is_none();
        let mut breaks = Vec::new();
        let mut words_end = 0.;
        let mut first_non_whitespace_ix = None;
        let mut last_candidate_ix = None;
        let mut last_candidate_x = px(0.);
        let mut last_boundary = WrapBoundary {
            run_ix: 0,
            glyph_ix: 0,
        };
        let mut last_boundary_x = px(0.);
        let mut prev_ch = '\0';
        let mut glyphs = self
            .runs
            .iter()
            .enumerate()
            .flat_map(move |(run_ix, run)| {
                run.glyphs.iter().enumerate().map(move |(glyph_ix, glyph)| {
                    let character = text[glyph.index..].chars().next().unwrap();
                    (
                        WrapBoundary { run_ix, glyph_ix },
                        character,
                        glyph.position.x,
                    )
                })
            })
            .peekable();

        while let Some((boundary, ch, x)) = glyphs.next() {
            if ch == '\n' {
                continue;
            }

            // Here is very similar to `LineWrapper::wrap_line` to determine text wrapping,
            // but there are some differences, so we have to duplicate the code here.
            if first_non_whitespace_ix.is_some()
                && ch != ' '
                && (prev_ch == ' ' || !is_word_char(ch) || word_break == WordBreak::BreakAll)
            {
                last_candidate_ix = Some(boundary);
                last_candidate_x = x;
                if constrain {
                    breaks.push((boundary, x.0, words_end));
                }
            }

            if ch != ' ' && first_non_whitespace_ix.is_none() {
                first_non_whitespace_ix = Some(boundary);
            }

            let next_x = glyphs.peek().map_or(self.width, |(_, _, x)| *x);
            if ch != ' ' {
                words_end = next_x.0;
            }
            let width = next_x - last_boundary_x;
            // The first line is shortened by the text indent.
            let available = if boundaries.is_empty() {
                wrap_width - indent
            } else {
                wrap_width
            };

            // Spaces at the end of a line hang, and float error must not wrap a line.
            if ch != ' '
                && width > available + px(0.01)
                && boundary > last_boundary
                && (last_candidate_ix.is_some() || word_break != WordBreak::Normal)
            {
                // When used line_clamp, we should limit the number of lines.
                if let Some(max_lines) = max_lines
                    && boundaries.len() >= max_lines.saturating_sub(1)
                {
                    break;
                }

                if let Some(last_candidate_ix) = last_candidate_ix.take() {
                    last_boundary = last_candidate_ix;
                    last_boundary_x = last_candidate_x;
                } else {
                    last_boundary = boundary;
                    last_boundary_x = x;
                }
                boundaries.push(last_boundary);
            }
            prev_ch = ch;
        }
        if !constrain || boundaries.is_empty() {
            return boundaries;
        }

        // Like WebKit's `InlineContentConstrainer`: choose the breaks with the least summed
        // raggedness `100 * |(width - ideal) / 15|^3`, where widths skip trailing spaces.
        breaks.insert(
            0,
            (
                WrapBoundary {
                    run_ix: 0,
                    glyph_ix: 0,
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
        // Balance keeps the line count up to 12 lines, like WebKit.
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
            // WebKit: when no line can start at a reached break, the next line is filled greedily.
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
                // Pretty lines must stay within 45px of the ideal width, except near the end.
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

/// A line of text that has been wrapped to fit a given width
#[derive(Default, Debug)]
pub struct WrappedLineLayout {
    /// The line layout, pre-wrapping.
    pub unwrapped_layout: Rc<LineLayout>,

    /// The boundaries at which the line was wrapped
    pub wrap_boundaries: Vec<WrapBoundary>,

    /// The width of the line, if it was wrapped
    pub wrap_width: Option<Pixels>,
}

/// A boundary at which a line was wrapped
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WrapBoundary {
    /// The index in the run just before the line was wrapped
    pub run_ix: usize,
    /// The index of the glyph just before the line was wrapped
    pub glyph_ix: usize,
}

impl WrappedLineLayout {
    /// The length of the underlying text, in utf8 bytes.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.unwrapped_layout.len
    }

    /// The width of this line, in pixels, whether or not it was wrapped.
    pub fn width(&self) -> Pixels {
        self.wrap_width
            .unwrap_or(Pixels::MAX)
            .min(self.unwrapped_layout.width)
    }

    /// The size of the whole wrapped text, for the given line_height.
    /// can span multiple lines if there are multiple wrap boundaries.
    pub fn size(&self, line_height: Pixels) -> Size<Pixels> {
        Size {
            width: self.width(),
            height: line_height * (self.wrap_boundaries.len() + 1),
        }
    }

    /// The ascent of a line in this layout
    pub fn ascent(&self) -> Pixels {
        self.unwrapped_layout.ascent
    }

    /// The descent of a line in this layout
    pub fn descent(&self) -> Pixels {
        self.unwrapped_layout.descent
    }

    /// The wrap boundaries in this layout
    pub fn wrap_boundaries(&self) -> &[WrapBoundary] {
        &self.wrap_boundaries
    }

    /// The font size of this layout
    pub fn font_size(&self) -> Pixels {
        self.unwrapped_layout.font_size
    }

    /// The runs in this layout, sans wrapping
    pub fn runs(&self) -> &[ShapedRun] {
        &self.unwrapped_layout.runs
    }

    /// The index corresponding to a given position in this layout for the given line height.
    ///
    /// See also [`Self::closest_index_for_position`].
    pub fn index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
    ) -> Result<usize, usize> {
        self._index_for_position(position, line_height, false)
    }

    /// The closest index to a given position in this layout for the given line height.
    ///
    /// Closest means the character boundary closest to the given position.
    ///
    /// See also [`LineLayout::closest_index_for_x`].
    pub fn closest_index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
    ) -> Result<usize, usize> {
        self._index_for_position(position, line_height, true)
    }

    fn _index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
        closest: bool,
    ) -> Result<usize, usize> {
        let wrapped_line_ix = (position.y / line_height) as usize;

        let (wrapped_line_start_index, wrapped_line_start_x) = if wrapped_line_ix > 0 {
            let Some(line_start_boundary) = self.wrap_boundaries.get(wrapped_line_ix - 1) else {
                return Err(0);
            };
            let run = &self.unwrapped_layout.runs[line_start_boundary.run_ix];
            let glyph = &run.glyphs[line_start_boundary.glyph_ix];
            (glyph.index, glyph.position.x)
        } else {
            (0, Pixels::ZERO)
        };

        let (wrapped_line_end_index, wrapped_line_end_x) =
            if wrapped_line_ix < self.wrap_boundaries.len() {
                let next_wrap_boundary_ix = wrapped_line_ix;
                let next_wrap_boundary = self.wrap_boundaries[next_wrap_boundary_ix];
                let run = &self.unwrapped_layout.runs[next_wrap_boundary.run_ix];
                let glyph = &run.glyphs[next_wrap_boundary.glyph_ix];
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
                // The shaper can place a trailing zero-width wrap boundary glyph slightly past
                // the line's width, so the row can extend past where `index_for_x` has glyphs.
                self.unwrapped_layout
                    .index_for_x(position_in_unwrapped_line.x)
                    .ok_or(wrapped_line_end_index)
            }
        }
    }

    /// Returns the pixel position for the given byte index.
    pub fn position_for_index(&self, index: usize, line_height: Pixels) -> Option<Point<Pixels>> {
        let mut line_start_ix = 0;
        let line_end_indices = self
            .wrap_boundaries
            .iter()
            .map(|wrap_boundary| {
                let run = &self.unwrapped_layout.runs[wrap_boundary.run_ix];
                let glyph = &run.glyphs[wrap_boundary.glyph_ix];
                glyph.index
            })
            .chain([self.len()])
            .enumerate();
        for (ix, line_end_ix) in line_end_indices {
            let line_y = ix as f32 * line_height;
            if index < line_start_ix {
                break;
            } else if index > line_end_ix {
                line_start_ix = line_end_ix;
                continue;
            } else {
                let line_start_x = self.unwrapped_layout.x_for_index(line_start_ix);
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

/// Lines unused for this many frames are dropped.
const MAX_AGE: u32 = 60;
/// Past this many bytes, only the lines of the last frame are kept.
const MAX_BYTES: usize = 4 << 20;

/// Layouts with the frame they were last used in and their size in bytes.
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

    /// The wrapped layout and the text of the line.
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
            Some(width) => unwrapped_layout
                .compute_wrap_boundaries(text, width, max_lines, indent, wrap, word_break),
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
        if runs
            .iter()
            .any(|run| run.letter_spacing != px(0.) || run.word_spacing != px(0.))
        {
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

// Combining marks (e.g. Thai vowel signs, Arabic diacritics) are shaped by
// HarfBuzz at the same x position as their base character. The force-width
// loop must not advance the cell counter for these zero-advance glyphs,
// otherwise they get displaced into the next cell. We detect them by checking
// whether shaped x has advanced by at least half a cell beyond the last base.
fn apply_force_width_to_layout(layout: &mut LineLayout, force_width: Pixels) {
    let mut glyph_pos: usize = 0;
    // NEG_INFINITY ensures the first glyph is always classified as a base.
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

/// Adds the letter and word spacing of the runs to the glyph positions and the width.
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
    let mut shift = px(0.);
    let mut previous = None;
    for run in &mut layout.runs {
        for glyph in &mut run.glyphs {
            if let Some(previous) = previous.filter(|&previous| previous != glyph.index) {
                shift += spacing(previous);
            }
            previous = Some(glyph.index);
            glyph.position.x += shift;
        }
    }
    if let Some(previous) = previous {
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

/// Whether a character joins the word before it, so lines don't break before it.
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        // Latin-1 Supplement, Latin Extended-A and B, Cyrillic.
        || matches!(c, '\u{00C0}'..='\u{024F}' | '\u{0400}'..='\u{04FF}')
        // Latin Extended Additional, combining diacritical marks, Bengali.
        || matches!(c, '\u{1E00}'..='\u{1EFF}' | '\u{0300}'..='\u{036F}' | '\u{0980}'..='\u{09FF}')
        // Punctuation that stays with the word: `a-b`, `I'm`, `100%`, `3.14`, trailing `,.:;`.
        || matches!(c, '-' | '_' | '.' | '\'' | '’' | '‘' | '$' | '%' | '@' | '#' | '^' | '~' | ',' | '=' | ':' | ';')
        // Closing punctuation never starts a line (UAX #14 LB13).
        || matches!(c, '!' | ')' | ']' | '}' | '"' | '”' | '»' | '…' | '⋯')
        // Non-breaking glue.
        || matches!(c, '\u{202F}' | '\u{00A0}' | '\u{2011}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{point, px};

    #[test]
    fn balance_and_pretty_even_out_lines() {
        // Nine 40px words with 10px spaces, in a 200px line.
        let text = ["aaaa"; 9].join(" ");
        let layout = LineLayout {
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
        };
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
            );
            let found: Vec<_> = boundaries
                .iter()
                .map(|boundary| boundary.glyph_ix)
                .collect();
            assert_eq!(found, starts, "{wrap:?}");
        }
    }
}
