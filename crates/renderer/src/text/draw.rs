use super::{DecorationStyle, FontId, Line, Look, TextAlign, TextLine};
use crate::{Color, Pixels, Point, Rect, Scalar, Size, Window, point, px};

/// Strikethroughs have no offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decoration {
    pub thickness: Pixels,
    /// The text color when `None`.
    pub color: Option<Color>,
    pub style: DecorationStyle,
    /// From the baseline, instead of the default place under it.
    pub offset: Option<Pixels>,
}

struct Rows<'a> {
    line: &'a Line,
    breaks: &'a [u32],
    origin: Point<Pixels>,
    align: TextAlign,
    align_width: Pixels,
}

impl Rows<'_> {
    fn left(&self, start: Pixels, row: usize) -> Pixels {
        let end =
            (self.breaks.get(row)).map_or(self.line.width, |&at| self.line.glyphs[at as usize].x);
        let width = end - start;
        let x = self.origin.x;
        match self.align {
            TextAlign::Left => x,
            TextAlign::Center => (x * 2. + self.align_width - width) / 2.,
            TextAlign::Right => x + self.align_width - width,
        }
    }
}

struct Cursor<'a> {
    looks: std::slice::Iter<'a, Look>,
    end: usize,
    len: usize,
}

impl<'a> Cursor<'a> {
    /// `None` while `index` stays in the current look. Past it, the look that holds `index` with
    /// its start, or `None` after the last one.
    fn step(&mut self, index: usize) -> Option<Option<(usize, &'a Look)>> {
        if index < self.end {
            return None;
        }
        let mut next = self.looks.next();
        while let Some(look) = next
            && index >= self.end + look.len
        {
            self.end += look.len;
            next = self.looks.next();
        }
        Some(match next {
            Some(look) => {
                let start = self.end;
                self.end += look.len;
                Some((start, look))
            }
            None => {
                self.end = self.len;
                None
            }
        })
    }
}

#[derive(Default)]
struct Ink {
    key: Option<(FontId, Pixels)>,
    size: Size<Pixels>,
    // From the baseline to the top left of the font box.
    lift: Point<Pixels>,
}

impl Ink {
    fn update(&mut self, glyph: &super::Glyph, window: &Window) {
        let key = (glyph.font, glyph.size);
        if self.key != Some(key) {
            self.key = Some(key);
            let bounds = window.text_system().glyph_box(key.0, key.1);
            (self.size, self.lift) = (bounds.size, point(bounds.left(), -bounds.bottom()));
        }
    }
}

impl TextLine {
    fn placed(&self, origin: Point<Pixels>, align: TextAlign, width: Option<Pixels>) -> Rows<'_> {
        Rows {
            line: &self.line,
            breaks: &self.breaks,
            origin,
            align,
            align_width: width.or(self.wrap).unwrap_or(self.line.width),
        }
    }

    pub fn paint(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        width: Option<Pixels>,
        window: &mut Window,
    ) {
        let rows = self.placed(origin, align, width);
        let line = &*self.line;
        let decorated = self
            .looks
            .iter()
            .any(|look| look.underline.is_some() || look.strikethrough.is_some());
        let padding = (line_height - line.ascent - line.descent) / 2.;
        let baseline = padding + line.ascent;
        let underline_y = padding + line.ascent + line.descent * 0.618;
        let strike_y = (line.ascent * 0.5 + baseline) * 0.5;
        // Decorations draw before the glyphs, so a line under the text never covers it.
        for glyphs in [false, true] {
            if !glyphs && !decorated {
                continue;
            }
            let draw =
                |window: &mut Window, start: Point<Pixels>, width: Pixels, style: &Decoration| {
                    if !glyphs {
                        let color = style.color.unwrap_or_default();
                        window.draw_line(start, width, style.thickness, color, style.style);
                    }
                };
            let mut cursor = Cursor {
                looks: self.looks.iter(),
                end: 0,
                len: line.len,
            };
            let mut ink = Ink::default();
            let mut kept: [Option<(Point<Pixels>, Decoration)>; 2] = [None; 2];
            let mut color = Color::BLACK;
            let (mut row, mut last_x) = (0, px(0.));
            let mut pen = point(rows.left(px(0.), 0), origin.y);
            let mut first_x = origin.x;
            for (at, glyph) in line.glyphs.iter().enumerate() {
                ink.update(glyph, window);
                let half = ink.size.width.half();
                pen.x += glyph.x - last_x;
                if at == 0 {
                    first_x = pen.x;
                }
                if rows.breaks.get(row) == Some(&(at as u32)) {
                    for (start, style) in kept.iter_mut().flatten() {
                        if pen.x == start.x {
                            start.x -= half;
                        }
                        draw(window, *start, pen.x - start.x, style);
                        *start = point(origin.x, start.y + line_height);
                    }
                    if glyph.index as usize >= cursor.end {
                        kept = [None; 2];
                    }
                    row += 1;
                    pen = point(rows.left(glyph.x, row), pen.y + line_height);
                }
                last_x = glyph.x;
                let mut ended = [None; 2];
                match cursor.step(glyph.index as usize) {
                    None => {}
                    Some(Some((_, look))) => {
                        for (at, wanted) in
                            [look.underline, look.strikethrough].into_iter().enumerate()
                        {
                            // The kept style has its color resolved, so a run without its own color restarts the line.
                            if kept[at].is_some_and(|(_, style)| wanted != Some(style)) {
                                ended[at] = kept[at].take();
                            }
                            if let Some(style) = wanted {
                                let y = match at {
                                    0 => {
                                        style.offset.map_or(underline_y, |offset| baseline + offset)
                                    }
                                    _ => strike_y,
                                };
                                kept[at].get_or_insert((
                                    point(pen.x, pen.y + y),
                                    Decoration {
                                        color: Some(style.color.unwrap_or(look.color)),
                                        ..style
                                    },
                                ));
                            }
                        }
                        color = look.color;
                    }
                    Some(None) => ended = std::mem::take(&mut kept),
                }
                for (mut start, style) in ended.into_iter().flatten() {
                    if start.x == pen.x {
                        start.x -= half;
                    }
                    draw(window, start, pen.x - start.x, &style);
                }
                let at = pen + point(px(0.), baseline) + point(px(0.), glyph.y);
                if glyphs
                    && Rect::new(at + ink.lift, ink.size).intersects(window.content_mask().bounds)
                {
                    window.draw_glyph(at, glyph.font, glyph.id, glyph.size, color, glyph.emoji);
                }
            }
            let half = ink.size.width.half();
            let mut end = first_x + line.width;
            if let Some(&at) = rows.breaks.last() {
                end -= line.glyphs[at as usize].x;
            }
            for (mut start, style) in kept.into_iter().flatten() {
                if end == start.x {
                    start.x -= half;
                }
                draw(window, start, end - start.x, &style);
            }
        }
    }

    /// Paints under `paint`, so call it first.
    pub fn paint_background(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        width: Option<Pixels>,
        window: &mut Window,
    ) {
        if self.looks.iter().all(|look| look.background.is_none()) {
            return;
        }
        let rows = self.placed(origin, align, width);
        let line = &*self.line;
        let quad = |window: &mut Window, origin: Point<Pixels>, width: Pixels, color: Color| {
            window.draw_rect(Rect::new(origin, Size::new(width, line_height)), color)
        };
        let mut cursor = Cursor {
            looks: self.looks.iter(),
            end: 0,
            len: line.len,
        };
        let mut ink = Ink::default();
        let mut background: Option<(Point<Pixels>, Color)> = None;
        let (mut row, mut last_x) = (0, px(0.));
        let mut pen = point(rows.left(px(0.), 0), origin.y);
        for (at, glyph) in line.glyphs.iter().enumerate() {
            ink.update(glyph, window);
            let half = ink.size.width.half();
            pen.x += glyph.x - last_x;
            if rows.breaks.get(row) == Some(&(at as u32)) {
                if let Some((start, color)) = &mut background {
                    if pen.x == start.x {
                        start.x -= half;
                    }
                    quad(window, *start, pen.x - start.x, *color);
                    if (glyph.index as usize) < cursor.end {
                        *start = point(origin.x, start.y + line_height);
                    } else {
                        background = None;
                    }
                }
                row += 1;
                pen = point(rows.left(glyph.x, row), pen.y + line_height);
            }
            last_x = glyph.x;
            let ended = match cursor.step(glyph.index as usize) {
                None => None,
                Some(Some((_, look))) => {
                    let ended = background
                        .filter(|(_, color)| look.background != Some(*color))
                        .and_then(|_| background.take());
                    if let Some(color) = look.background {
                        background.get_or_insert((pen, color));
                    }
                    ended
                }
                Some(None) => background.take(),
            };
            // The width is taken before an empty background moves left, unlike at row ends.
            if let Some((mut start, color)) = ended {
                let width = pen.x - start.x;
                if start.x == pen.x {
                    start.x -= half;
                }
                quad(window, start, width, color);
            }
        }
        let mut end = origin.x + line.width;
        if let Some(&at) = rows.breaks.last() {
            end -= line.glyphs[at as usize].x;
        }
        if let Some((mut start, color)) = background {
            if end == start.x {
                start.x -= ink.size.width.half();
            }
            quad(window, start, end - start.x, color);
        }
    }
}
