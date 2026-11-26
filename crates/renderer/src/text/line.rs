use super::{Decoration, FontId, GlyphId, Hyphen, TextRun, bidi::BoxPart, hyphen::find};
use crate::{Color, Pixels, Size, px};
use std::{ops::Range, rc::Rc};

/// `x` and `y` are relative to the start of its line and the baseline.
#[derive(Clone, Copy)]
pub struct Glyph {
    pub x: Pixels,
    pub y: Pixels,
    pub size: Pixels,
    pub font: FontId,
    /// Byte offset of the glyph's cluster in the line text.
    pub index: u32,
    pub id: GlyphId,
    pub emoji: bool,
}

/// Glyphs are in visual order.
#[derive(Clone, Default)]
pub struct Line {
    pub glyphs: Vec<Glyph>,
    pub width: Pixels,
    pub ascent: Pixels,
    pub descent: Pixels,
    pub len: usize,
    pub font_size: Pixels,
    /// The bidi level of each glyph, in text of several directions.
    pub levels: Vec<u8>,
    /// Where each glyph ends, when edges of inline boxes come before the next.
    pub(crate) rights: Vec<Pixels>,
    /// Glyph `x` and `index` never decrease, so hit tests can bisect.
    pub(crate) sorted: bool,
}

impl Line {
    /// No text of the line is right-to-left.
    pub fn sorted(&self) -> bool {
        self.sorted
    }

    pub(crate) fn sort_check(&mut self) {
        self.sorted = self
            .glyphs
            .windows(2)
            .all(|pair| pair[0].x <= pair[1].x && pair[0].index <= pair[1].index);
    }

    pub fn index_at(&self, x: Pixels) -> Option<usize> {
        if x >= self.width {
            return None;
        }
        let found = if self.sorted {
            self.glyphs
                .partition_point(|glyph| glyph.x <= x)
                .checked_sub(1)
        } else {
            self.glyphs.iter().rposition(|glyph| glyph.x <= x)
        };
        Some(found.map_or(0, |at| self.glyphs[at].index as usize))
    }

    pub fn nearest_index(&self, x: Pixels) -> usize {
        self.nearest_in(x, 0..self.glyphs.len())
    }

    fn nearest_in(&self, x: Pixels, glyphs: Range<usize>) -> usize {
        if !self.sorted {
            // Each glyph has a caret stop at both sides: before its text on its leading side, after it on the other.
            let mut best = (Pixels(f32::INFINITY), 0);
            for at in glyphs {
                let [left, right] = self.edges(at);
                for (edge, index) in [(self.glyphs[at].x, left), (self.right_of(at), right)] {
                    let distance = (edge - x).abs();
                    if distance < best.0 {
                        best = (distance, index);
                    }
                }
            }
            return if self.glyphs.is_empty() { 0 } else { best.1 };
        }
        let at = self.glyphs.partition_point(|glyph| glyph.x < x);
        let before = at.checked_sub(1).map(|before| self.glyphs[before]);
        let (before, before_x) =
            before.map_or((0, px(0.)), |glyph| (glyph.index as usize, glyph.x));
        let (after, after_x) = (self.glyphs.get(at)).map_or((self.len, self.width), |glyph| {
            (glyph.index as usize, glyph.x)
        });
        if after_x - x < x - before_x {
            after
        } else {
            before
        }
    }

    /// Whether the glyph at visual place `at` is in a right-to-left run: a neighbor's text goes the other way.
    fn reversed(&self, at: usize) -> bool {
        if let Some(level) = self.levels.get(at) {
            return level % 2 == 1;
        }
        let index = self.glyphs[at].index;
        self.glyphs
            .get(at + 1)
            .is_some_and(|next| next.index < index)
            || at
                .checked_sub(1)
                .is_some_and(|before| self.glyphs[before].index > index)
    }

    fn right_of(&self, at: usize) -> Pixels {
        match self.rights.get(at) {
            Some(&right) => right,
            None => self.glyphs.get(at + 1).map_or(self.width, |next| next.x),
        }
    }

    /// The text offsets at the left and right edges of the glyph at visual place `at`.
    fn edges(&self, at: usize) -> [usize; 2] {
        let index = self.glyphs[at].index;
        let end = (self.glyphs.iter())
            .map(|glyph| glyph.index)
            .filter(|&other| other > index)
            .min()
            .map_or(self.len, |end| end as usize);
        match self.reversed(at) {
            true => [end, index as usize],
            false => [index as usize, end],
        }
    }

    pub fn x_at(&self, index: usize) -> Pixels {
        self.x_in(index, 0..self.glyphs.len())
    }

    fn x_in(&self, index: usize, glyphs: Range<usize>) -> Pixels {
        if !self.levels.is_empty() {
            // Like Chrome, the caret goes into the first run from the left that has its place, also at an end of the run.
            let mut start = glyphs.start;
            while start < glyphs.end {
                let level = self.levels[start];
                let len = (glyphs.start.max(start)..glyphs.end)
                    .take_while(|&at| self.levels[at] == level)
                    .count();
                let run = start..start + len;
                start = run.end;
                let indexes = || run.clone().map(|at| self.glyphs[at].index as usize);
                let (low, high) = (indexes().min().unwrap_or(0), indexes().max().unwrap_or(0));
                let end = (self.glyphs.iter())
                    .map(|glyph| glyph.index as usize)
                    .filter(|&other| other > high)
                    .min()
                    .unwrap_or(self.len);
                if !(low..=end).contains(&index) {
                    continue;
                }
                let odd = level % 2 == 1;
                let next = (run.clone())
                    .filter(|&at| self.glyphs[at].index as usize >= index)
                    .min_by_key(|&at| (self.glyphs[at].index, at));
                return match (next, odd) {
                    (Some(at), false) => self.glyphs[at].x,
                    (Some(at), true) => self.right_of(at),
                    (None, false) => self.right_of(run.end - 1),
                    (None, true) => self.glyphs[run.start].x,
                };
            }
        }
        if !self.sorted {
            // The caret goes before the glyph that the text at `index` starts, on its leading side, or after the
            // last glyph of the text on its trailing side.
            let next = (glyphs.clone())
                .filter(|&at| self.glyphs[at].index as usize >= index)
                .min_by_key(|&at| (self.glyphs[at].index, at));
            return match next {
                Some(at) if self.reversed(at) => self.right_of(at),
                Some(at) => self.glyphs[at].x,
                None => {
                    glyphs
                        .max_by_key(|&at| (self.glyphs[at].index, at))
                        .map_or(self.width, |at| match self.reversed(at) {
                            true => self.glyphs[at].x,
                            false => self.right_of(at),
                        })
                }
            };
        }
        let at = self
            .glyphs
            .partition_point(|glyph| (glyph.index as usize) < index);
        self.glyphs.get(at).map_or(self.width, |glyph| glyph.x)
    }
}

/// Text of one direction in a row.
pub struct Part {
    pub left: Pixels,
    pub right: Pixels,
    /// The text offsets of its first and last characters.
    pub first: usize,
    pub last: usize,
    pub odd: bool,
}

pub struct Look {
    pub len: usize,
    pub color: Color,
    pub background: Option<Color>,
    pub underline: Option<Decoration>,
    pub strikethrough: Option<Decoration>,
}

pub(crate) fn looks<'a>(runs: impl Iterator<Item = (usize, &'a TextRun)>) -> Rc<[Look]> {
    let mut list: Vec<Look> = Vec::new();
    for (len, run) in runs {
        let &TextRun {
            color,
            background_color: background,
            underline,
            strikethrough,
            ..
        } = run;
        let look = Look {
            len,
            color,
            background,
            underline,
            strikethrough,
        };
        match list.last_mut() {
            Some(last)
                if (
                    last.color,
                    last.background,
                    last.underline,
                    last.strikethrough,
                ) == (color, background, underline, strikethrough) =>
            {
                last.len += len
            }
            _ => list.push(look),
        }
    }
    list.into()
}

#[derive(Clone, Default)]
pub struct TextLine {
    pub line: Rc<Line>,
    /// Indexes into `line.glyphs` of the first glyph of each row after the first.
    pub breaks: Rc<[u32]>,
    pub wrap: Option<Pixels>,
    pub text: Rc<str>,
    pub looks: Rc<[Look]>,
    /// Beside floats, `[left, right, top]` of each row: its insets and its top in the paragraph. Else empty.
    pub shifts: Rc<[[f32; 3]]>,
    pub(crate) extra: Option<Rc<Extra>>,
}

/// What few lines have.
#[derive(Clone, Default)]
pub(crate) struct Extra {
    /// In order.
    pub hyphens: Option<Rc<[Hyphen]>>,
    /// In text of several directions, the line with its glyphs in the order of the text, and where the rows after the
    /// first start in the text.
    pub logical: Option<Rc<Line>>,
    pub starts: Vec<u32>,
    pub rtl: bool,
    /// The spaces at the end of a row stay.
    pub preserve: bool,
    /// Of inline boxes, sorted by box, row and place.
    pub parts: Vec<BoxPart>,
    /// The room of edges before the first glyph of each row.
    pub leads: Vec<Pixels>,
}

impl TextLine {
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.line.len
    }

    pub fn rows(&self) -> usize {
        self.breaks.len() + 1
    }

    /// `None` for the first row and past the last.
    fn row_start(&self, row: usize) -> Option<&super::Glyph> {
        let at = *self.breaks.get(row.checked_sub(1)?)?;
        Some(&self.line.glyphs[at as usize])
    }

    pub fn hyphenates(&self) -> bool {
        self.hyphens().is_some()
    }

    pub(crate) fn hyphens(&self) -> Option<&[Hyphen]> {
        self.extra.as_ref()?.hyphens.as_deref()
    }

    /// The line with its glyphs in the order of the text.
    pub(crate) fn logical(&self) -> &Rc<Line> {
        let logical = self.extra.as_ref().and_then(|extra| extra.logical.as_ref());
        logical.unwrap_or(&self.line)
    }

    /// The hyphen that a row ends with.
    pub fn hyphen(&self, row: usize) -> Option<&Hyphen> {
        self.hyphens()?;
        self.row_start(row + 1)?;
        self.hyphen_before(self.row_index(row + 1))
    }

    pub fn hyphen_before(&self, index: usize) -> Option<&Hyphen> {
        find(self.hyphens()?, index)
    }

    /// Where a row starts in the text, or the end of the text past the last row.
    pub fn row_index(&self, row: usize) -> usize {
        let Some(after) = row.checked_sub(1) else {
            return 0;
        };
        match self.extra.as_ref().filter(|extra| extra.logical.is_some()) {
            Some(extra) => (extra.starts.get(after)).map_or(self.line.len, |&start| start as usize),
            None => self
                .row_start(row)
                .map_or(self.line.len, |glyph| glyph.index as usize),
        }
    }

    pub fn row_glyphs(&self, row: usize) -> Range<usize> {
        let end =
            |row: usize| (self.breaks.get(row)).map_or(self.line.glyphs.len(), |&at| at as usize);
        row.checked_sub(1).map_or(0, end)..end(row)
    }

    pub fn extent(&self, line_height: Pixels) -> Size {
        let width = self.wrap.unwrap_or(Pixels::MAX).min(self.line.width);
        Size::new(width, line_height * self.rows())
    }

    pub fn first_row_end(&self) -> Pixels {
        let end = self.row_index(1);
        if !self.line.sorted {
            return (self.row_start(1)).map_or(self.line.width, |glyph| glyph.x);
        }
        self.line.x_at(self.text[..end].trim_end().len())
    }

    /// The glyphs of the spaces that hang at the end of a row of text of several directions.
    fn hung(&self, row: usize) -> Range<usize> {
        let glyphs = self.row_glyphs(row);
        let start = glyphs.clone().rev().take_while(|&at| self.is_space(at));
        glyphs.end - start.count()..glyphs.end
    }

    fn is_space(&self, at: usize) -> bool {
        (self.text.as_bytes()).get(self.line.glyphs[at].index as usize) == Some(&b' ')
    }

    /// The text is in a right-to-left paragraph, and not all of it is right-to-left itself.
    pub fn rtl(&self) -> bool {
        self.extra.as_ref().is_some_and(|extra| extra.rtl)
    }

    pub fn preserves(&self) -> bool {
        self.extra.as_ref().is_some_and(|extra| extra.preserve)
    }

    /// In text of several directions, the place between the characters around `index`, from the left of the row, and
    /// whether it is in right-to-left text. Edges of inline boxes take room before and after it.
    pub fn mark(
        &self,
        index: usize,
        row: usize,
        gap: Pixels,
        [lead, trail]: [Pixels; 2],
    ) -> Option<(Pixels, bool)> {
        let logical = self.extra.as_ref()?.logical.as_ref()?;
        let next = (logical.glyphs).partition_point(|glyph| (glyph.index as usize) < index);
        let base = u8::from(self.rtl());
        let level = |at: Option<usize>| {
            let level = at.and_then(|at| logical.levels.get(at));
            level.map_or(base, |level| level & 15)
        };
        let sides = [next.checked_sub(1), Some(next)];
        let levels = sides.map(level);
        let own = levels[0].min(levels[1]);
        let odd = own % 2 == 1;
        // The glyphs before and after, in this row.
        let glyphs = self.row_glyphs(row);
        let shown = sides.map(|side| {
            let index = logical.glyphs.get(side?)?.index;
            glyphs
                .clone()
                .find(|&at| self.line.glyphs[at].index == index)
        });
        let edge = |at: usize, right: bool| match right {
            true => self.line.right_of(at),
            false => self.line.glyphs[at].x,
        };
        let (x, at) = match (shown, levels) {
            ([_, Some(after)], [_, level]) if level == own => {
                let trail = if odd { trail } else { -trail };
                (edge(after, odd) + trail, after)
            }
            ([Some(before), _], [level, _]) if level == own => {
                let lead = if odd { -lead } else { lead };
                (edge(before, !odd) + lead, before)
            }
            ([before, after], _) => {
                // Text of the other direction, as a whole.
                let end = after.is_none();
                let inside = after.or(before)?;
                let same = |at: &usize| self.line.levels[*at] > own;
                let at = match odd == end {
                    true => (glyphs.start..=inside).rev().take_while(same).last(),
                    false => (inside..glyphs.end).take_while(same).last(),
                };
                let at = at.unwrap_or(inside);
                (edge(at, odd != end), at)
            }
        };
        let spaces = (glyphs.start..at).filter(|&at| self.is_space(at)).count();
        Some((x - self.row_left(row) + gap * spaces as f32, odd))
    }

    /// The edges of inline boxes before the first glyph of a row, when the line places them.
    pub fn lead(&self, row: usize) -> Option<Pixels> {
        self.extra.as_ref()?.leads.get(row).copied()
    }

    /// The parts of an inline box: the row, the left and the right edge from the left of the row, where each space is
    /// as much wider as `gap` says, and whether the part has the sides of the box.
    pub fn box_parts(
        &self,
        id: usize,
        gap: impl Fn(usize) -> Pixels,
    ) -> impl Iterator<Item = (usize, [Pixels; 2], [bool; 2])> {
        let parts = self.extra.as_ref().map_or(&[][..], |extra| &extra.parts);
        let parts = &parts[parts.partition_point(|part| part.id < id)..];
        (parts.iter().take_while(move |part| part.id == id)).map(move |part| {
            let spaces = |end: usize| {
                let glyphs = self.row_glyphs(part.row).start..end;
                gap(part.row) * glyphs.filter(|&at| self.is_space(at)).count() as f32
            };
            let left = self.row_left(part.row);
            let edges = [
                self.line.glyphs[part.glyphs.start].x + spaces(part.glyphs.start) - part.pads[0],
                self.line.right_of(part.glyphs.end - 1) + spaces(part.glyphs.end) + part.pads[1],
            ];
            (part.row, edges.map(|x| x - left), part.sides)
        })
    }

    fn row_left(&self, row: usize) -> Pixels {
        match row {
            0 => px(0.),
            _ => self.row_start(row).map_or(px(0.), |glyph| glyph.x),
        }
    }

    /// The parts of the text in `range` in a row, with their places from the left of the row, where each space is
    /// `gap` wider. The spaces that hang at the end of the row are a part with `hang`.
    pub fn parts(&self, row: usize, range: Range<usize>, gap: Pixels, hang: bool) -> Vec<Part> {
        let (glyphs, hung) = (self.row_glyphs(row), self.hung(row));
        let mut left = self.row_left(row);
        let mut parts: Vec<Part> = Vec::new();
        let (mut open, mut last) = (false, None);
        for at in glyphs.start..hung.start {
            let index = self.line.glyphs[at].index as usize;
            let odd = self.line.levels.get(at).is_some_and(|level| level % 2 == 1);
            let inside = range.contains(&index);
            let start = self.line.glyphs[at].x - left;
            if self.is_space(at) {
                left -= gap;
            }
            let end = self.line.right_of(at) - left;
            let level = self.line.levels.get(at).copied();
            match parts.last_mut().filter(|_| inside && open && level == last) {
                Some(part) => {
                    part.right = end;
                    part.first = part.first.min(index);
                    part.last = part.last.max(index);
                }
                None if inside => parts.push(Part {
                    left: start,
                    right: end,
                    first: index,
                    last: index,
                    odd,
                }),
                None => {}
            }
            (open, last) = (inside, level);
        }
        if hang && !hung.is_empty() {
            let right = self.line.right_of(hung.end - 1) - left;
            let mut pen = if self.rtl() {
                px(0.)
            } else {
                self.line.glyphs[hung.start].x - left
            };
            let mut ends: Option<Part> = None;
            for at in hung {
                let index = self.line.glyphs[at].index as usize;
                let width = self.line.right_of(at) - self.line.glyphs[at].x;
                let [low, high] = match self.rtl() {
                    true => [pen - width, pen],
                    false => [pen, pen + width],
                };
                pen = if self.rtl() { low } else { high };
                if range.contains(&index) {
                    let part = ends.get_or_insert(Part {
                        left: low,
                        right: high,
                        first: index,
                        last: index,
                        odd: self.rtl(),
                    });
                    (part.left, part.right) = (
                        part.left.min(low),
                        part.right.max(high).min(right.max(high)),
                    );
                    part.last = index;
                }
            }
            parts.extend(ends);
        }
        parts
    }

    /// The spaces of a row to the left of `x`, from the left of the row.
    pub fn spaces_before(&self, row: usize, x: Pixels) -> usize {
        let left = self.row_left(row);
        (self.row_glyphs(row).start..self.hung(row).start)
            .filter(|&at| self.is_space(at) && self.line.glyphs[at].x - left < x - px(0.01))
            .count()
    }

    /// Outside the row, `Err` with the index at the row edge, and 0 for rows past the last.
    pub fn hit(&self, row: usize, x: Pixels, nearest: bool) -> Result<usize, usize> {
        if row >= self.rows() {
            return Err(0);
        }
        let start = self.row_index(row);
        let left = match row {
            0 => px(0.),
            _ => self.row_start(row).map_or(px(0.), |glyph| glyph.x),
        };
        let (end, right, kept) = match self.row_start(row + 1) {
            // Like Chrome, the caret goes before the spaces that hang at the end of a wrapped row.
            Some(glyph) => {
                let end = self.row_index(row + 1);
                let kept = self
                    .text
                    .get(start..end)
                    .map_or(end, |text| start + text.trim_end_matches(' ').len());
                (end, glyph.x, kept)
            }
            None => (self.line.len, self.line.width, self.line.len),
        };
        let x = x + left;
        if !self.line.sorted {
            let inside = self.line.nearest_in(x, self.row_glyphs(row)).min(kept);
            return match x >= left && x < right {
                true if !nearest => self.line.index_at(x).ok_or(end),
                true => Ok(inside),
                false => Err(inside),
            };
        }
        if x < left {
            Err(start)
        } else if x >= right {
            Err(kept)
        } else if nearest {
            Ok(self.line.nearest_index(x).min(kept))
        } else {
            // A trailing zero-width glyph can reach past the line width, where no glyph starts.
            self.line.index_at(x).ok_or(end)
        }
    }

    pub fn caret(&self, index: usize) -> Option<(usize, Pixels)> {
        let mut start = 0;
        for row in 0..self.rows() {
            let end = self.row_index(row + 1);
            if index < start {
                return None;
            }
            if index <= end {
                let left = match row {
                    0 => px(0.),
                    _ => self.row_start(row)?.x,
                };
                if self.line.sorted && self.line.levels.is_empty() {
                    return Some((row, self.line.x_at(index) - left));
                }
                let hung = self.hung(row);
                // The spaces of a right-to-left row hang at its left.
                if self.rtl()
                    && let Some(first) = hung.clone().next()
                    && index >= self.line.glyphs[first].index as usize
                {
                    let before =
                        hung.take_while(|&at| (self.line.glyphs[at].index as usize) < index);
                    let width = before.fold(px(0.), |width, at| {
                        width + self.line.right_of(at) - self.line.glyphs[at].x
                    });
                    return Some((row, -width));
                }
                return Some((row, self.line.x_in(index, self.row_glyphs(row)) - left));
            }
            start = end;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn line(text: &str) -> Line {
        let mut line = Line {
            glyphs: (0..text.len())
                .map(|index| Glyph {
                    x: px(index as f32 * 10.),
                    y: px(0.),
                    size: px(10.),
                    font: 0,
                    index: index as u32,
                    id: 0,
                    emoji: false,
                })
                .collect(),
            width: px(text.len() as f32 * 10.),
            len: text.len(),
            font_size: px(10.),
            ..Default::default()
        };
        line.sort_check();
        line
    }

    #[test]
    fn right_to_left_glyphs_put_carets_on_their_leading_side() {
        // "ab" then three right-to-left letters, drawn reversed: a b 4 3 2, each 10 wide.
        let mut line = line("abcde");
        for (glyph, index) in line.glyphs.iter_mut().zip([0, 1, 4, 3, 2]) {
            glyph.index = index;
        }
        line.sort_check();
        assert!(!line.sorted);
        let carets: Vec<f32> = (0..=5).map(|index| line.x_at(index).0).collect();
        assert_eq!(carets, [0., 10., 50., 40., 30., 20.]);
        // At 20 the right side of b and the left side of the run meet: the first stop wins.
        let hits: Vec<usize> = [0., 10., 20., 30., 40., 50.]
            .into_iter()
            .map(|x| line.nearest_index(px(x)))
            .collect();
        assert_eq!(hits, [0, 1, 2, 4, 3, 2]);
    }

    #[test]
    fn hit_tests_agree_on_sorted_and_unsorted_lines() {
        let sorted = line("abcdef");
        let mut scan = sorted.clone();
        scan.sorted = false;
        for x in [-5., 0., 4., 5., 6., 14., 15., 16., 49., 55., 59.9, 60., 70.] {
            let x = px(x);
            assert_eq!(sorted.index_at(x), scan.index_at(x), "{x:?}");
            assert_eq!(sorted.nearest_index(x), scan.nearest_index(x), "{x:?}");
        }
        for index in 0..=7 {
            assert_eq!(sorted.x_at(index), scan.x_at(index));
        }
        assert_eq!(sorted.index_at(px(25.)), Some(2));
        assert_eq!(sorted.index_at(px(60.)), None);
        assert_eq!(sorted.nearest_index(px(14.)), 1);
        assert_eq!(sorted.nearest_index(px(16.)), 2);
        assert_eq!(sorted.nearest_index(px(99.)), 6);
        assert_eq!(sorted.nearest_index(px(54.)), 5);
        assert_eq!(sorted.nearest_index(px(56.)), 6);
        assert_eq!(sorted.x_at(9), px(60.));
    }

    #[test]
    fn rows_map_positions_and_indexes() {
        let text = TextLine {
            line: Rc::new(line("abcdef")),
            breaks: [3].into(),
            wrap: Some(px(30.)),
            text: "abcdef".into(),
            looks: Rc::default(),
            shifts: Rc::default(),
            extra: None,
        };
        let height = px(20.);
        assert_eq!(text.extent(height), Size::new(px(30.), px(40.)));
        assert_eq!(text.hit(1, px(12.), false), Ok(4));
        assert_eq!(text.hit(0, px(31.), false), Err(3));
        assert_eq!(text.hit(2, px(1.), false), Err(0));
        assert_eq!(text.hit(1, px(16.), true), Ok(5));
        assert_eq!(text.caret(4), Some((1, px(10.))));
        assert_eq!(text.caret(3), Some((0, px(30.))));
        assert_eq!(text.caret(7), None);
        assert_eq!(text.first_row_end(), px(30.));
    }
}
