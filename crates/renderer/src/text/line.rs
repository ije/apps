use super::{FontId, GlyphId, TextRun};
use crate::{Color, Decoration, Pixels, Size, px};
use std::rc::Rc;

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
    /// Glyph `x` and `index` never decrease, so hit tests can bisect.
    pub(crate) sorted: bool,
}

impl Line {
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
        let at = if self.sorted {
            self.glyphs.partition_point(|glyph| glyph.x < x)
        } else {
            (self.glyphs.iter().position(|glyph| glyph.x >= x)).unwrap_or(self.glyphs.len())
        };
        // Past every glyph, the choice is between the rightmost glyph and the line end.
        let before = if self.sorted || at < self.glyphs.len() {
            at.checked_sub(1).map(|before| self.glyphs[before])
        } else {
            self.glyphs.iter().copied().max_by_key(|glyph| glyph.x)
        };
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

    pub fn x_at(&self, index: usize) -> Pixels {
        let at = if self.sorted {
            Some(
                self.glyphs
                    .partition_point(|glyph| (glyph.index as usize) < index),
            )
            .filter(|&at| at < self.glyphs.len())
        } else {
            self.glyphs
                .iter()
                .position(|glyph| glyph.index as usize >= index)
        };
        at.map_or(self.width, |at| self.glyphs[at].x)
    }
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

    pub fn extent(&self, line_height: Pixels) -> Size {
        let width = self.wrap.unwrap_or(Pixels::MAX).min(self.line.width);
        Size::new(width, line_height * self.rows())
    }

    pub fn first_row_end(&self) -> Pixels {
        let end = self
            .row_start(1)
            .map_or(self.line.len, |glyph| glyph.index as usize);
        self.line.x_at(self.text[..end].trim_end().len())
    }

    /// Outside the row, `Err` with the index at the row edge, and 0 for rows past the last.
    pub fn hit(&self, row: usize, x: Pixels, nearest: bool) -> Result<usize, usize> {
        let (start, left) = match row {
            0 => (0, px(0.)),
            _ => match self.row_start(row) {
                Some(glyph) => (glyph.index as usize, glyph.x),
                None => return Err(0),
            },
        };
        let (end, right) = match self.row_start(row + 1) {
            Some(glyph) => (glyph.index as usize, glyph.x),
            None => (self.line.len, self.line.width),
        };
        let x = x + left;
        if x < left {
            Err(start)
        } else if x >= right {
            Err(end)
        } else if nearest {
            Ok(self.line.nearest_index(x))
        } else {
            // A trailing zero-width glyph can reach past the line width, where no glyph starts.
            self.line.index_at(x).ok_or(end)
        }
    }

    pub fn caret(&self, index: usize) -> Option<(usize, Pixels)> {
        let mut start = 0;
        for row in 0..self.rows() {
            let end = self
                .row_start(row + 1)
                .map_or(self.line.len, |glyph| glyph.index as usize);
            if index < start {
                return None;
            }
            if index <= end {
                let left = if row == 0 {
                    px(0.)
                } else {
                    self.line.x_at(start)
                };
                return Some((row, self.line.x_at(index) - left));
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
