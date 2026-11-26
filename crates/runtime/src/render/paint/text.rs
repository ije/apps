use super::super::ViewState;
use crate::css;
use crate::css::properties::Prop;
use crate::style;
use anyhow::Result;
use renderer::{AlignItems, Color, Pixels, Point, Rect, SharedString, Window, point, px, size};
use std::{
    ops::Range,
    rc::Rc,
    time::{Duration, Instant},
};

pub(crate) type Blink = ([usize; 3], Instant, Option<(u32, renderer::Timer)>);

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum CaretShape {
    Bar,
    Block,
    Underscore,
}

#[derive(Clone, Copy)]
pub(crate) struct CaretLook {
    color: Color,
    pub(crate) shape: CaretShape,
    blinks: bool,
    rtl: bool,
}

impl CaretLook {
    pub(crate) fn new(style: &css::properties::Style) -> Self {
        let own = style
            .get(Prop::CARET_COLOR)
            .and_then(css::value::color::paint);
        let shape = match style.get(Prop::CARET_SHAPE) {
            Some("block") => CaretShape::Block,
            Some("underscore") => CaretShape::Underscore,
            _ => CaretShape::Bar,
        };
        let mut color = own
            .or_else(|| css::value::color::paint(style.get(Prop::COLOR)?))
            .unwrap_or(Color::BLACK);
        // Like Chrome, the text stays readable on a block in its own color.
        if own.is_none() && shape == CaretShape::Block {
            color.a *= 0.5;
        }
        Self {
            color,
            shape,
            blinks: style.get(Prop::CARET_ANIMATION) != Some("manual"),
            rtl: style.get(Prop::DIRECTION) == Some("rtl"),
        }
    }

    // From the caret to the next character that the row draws.
    pub(crate) fn advance(
        &self,
        layout: &renderer::Paragraph,
        text: &str,
        rows: &[crate::render::layout::Row],
        index: usize,
        at: Point<Pixels>,
    ) -> Option<Pixels> {
        if self.shape == CaretShape::Bar {
            return None;
        }
        text.get(index..)?
            .char_indices()
            .take_while(|&(_, ch)| ch != '\n')
            .map_while(|(offset, ch)| {
                text_position(layout, text, rows, index + offset + ch.len_utf8())
            })
            .find(|next| *next != at)
            .filter(|next| next.y == at.y)
            .map(|next| next.x - at.x)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_caret(
    view: &ViewState,
    key: [usize; 3],
    at: Point<Pixels>,
    line_height: Pixels,
    advance: Option<Pixels>,
    (face, font_size, baseline): (renderer::FontId, Pixels, Pixels),
    look: CaretLook,
    window: &mut Window,
) {
    const BLINK: Duration = renderer::CARET_BLINK;
    let color = look.color;
    if color.a == 0. || !window.is_active() {
        return;
    }
    let bounds = if look.shape == CaretShape::Bar {
        Rect::new(at, size(px(1.), line_height))
    } else {
        // The content area of the font, as in Chrome.
        let (system, scale) = (window.text_system(), window.scale_factor());
        let [ascent, descent, _] = system
            .line_metrics(face, font_size * scale)
            .map(|value| value / scale);
        let top = at.y + baseline - ascent;
        let width = advance.unwrap_or_else(|| {
            let zero = system
                .advance(face, font_size, '0')
                .unwrap_or(font_size / 2.);
            if look.rtl { -zero } else { zero }
        });
        let height = if look.shape == CaretShape::Block {
            ascent + descent
        } else {
            px(1.)
        };
        Rect::new(
            point(at.x + width.min(px(0.)), top + ascent + descent - height),
            size(width.abs(), height),
        )
    };
    if !look.blinks {
        *view.caret.borrow_mut() = None;
        window.draw_rect(bounds, color);
        return;
    }
    let now = Instant::now();
    let mut caret = view.caret.borrow_mut();
    if caret.as_ref().is_some_and(|(old, ..)| *old != key) {
        *caret = None;
    }
    let (_, since, wake) = caret.get_or_insert((key, now, None));
    // A timer can fire just before its time, so the phase rounds up within 8 ms.
    let phase = ((now - *since + Duration::from_millis(8)).as_millis() / BLINK.as_millis()) as u32;
    // The first phase draws, so typing and scrolling move the caret in the same frame as the text.
    // The overlay is above the text, and a block is behind it.
    if phase > 0
        && look.shape != CaretShape::Block
        && window.overlay_caret(bounds, color, *since, phase.is_multiple_of(2))
    {
        *wake = None;
        return;
    }
    if wake.as_ref().is_none_or(|(old, _)| *old != phase) {
        let due = *since + BLINK * (phase + 1);
        *wake = Some((phase, window.timer(due - now, |window, _| window.refresh())));
    }
    if phase.is_multiple_of(2) {
        window.draw_rect(bounds, color);
    }
}

pub(crate) struct TextHighlight {
    pub(crate) range: Range<usize>,
    pub(crate) color: Option<renderer::Color>,
    pub(crate) background: renderer::Color,
    /// Like Chrome, a `::selection` style draws the text without its stroke.
    pub(crate) stroke: bool,
    /// The background of a custom highlight is as tall as the font, not the line.
    pub(crate) tight: bool,
    /// The underline, line-through and overline.
    pub(crate) lines: [Option<renderer::Decoration>; 3],
}

// A background or border of an inline box in a group of the text: the group, rect, corners, fill and border.
type Grouped = (
    usize,
    Rect<Pixels>,
    renderer::Corners<Pixels>,
    Color,
    renderer::Border,
);

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_text(
    text_line: &renderer::TextLine,
    origin: Point<Pixels>,
    line_height: Pixels,
    runs: &[renderer::TextRun],
    offset: usize,
    highlights: &[TextHighlight],
    rows: &[crate::render::layout::Row],
    text: &str,
    ink: Option<renderer::Color>,
    strut: renderer::FontId,
    window: &mut Window,
) {
    let (line, wraps) = (&text_line.line, &text_line.breaks[..]);
    let shift = |row: usize| rows.get(row).map_or(px(0.), |row| row.shift);
    let justify = |row: usize, index: usize| {
        rows.get(row)
            .filter(|row| row.gap > px(0.))
            .map_or(px(0.), |row| {
                row.gap * spaces(text.get(row.start.min(index)..index).unwrap_or(""))
            })
    };
    let baseline =
        window
            .text_system()
            .baseline(strut, line.font_size, line_height, window.scale_factor());
    let drop = |row: usize| rows.get(row).map_or(px(0.), |row| row.drop);
    let top = |row: usize| origin.y + line_height * row + drop(row);
    let mask = window.content_mask().bounds;
    let mut face = None;
    let tall = line.glyphs.iter().fold(line_height, |tall, glyph| {
        if face.replace((glyph.font, glyph.size)) == Some((glyph.font, glyph.size)) {
            return tall;
        }
        tall.max(
            window
                .text_system()
                .glyph_box(glyph.font, glyph.size)
                .size
                .height,
        )
    });
    let shown = |row: usize| top(row) < mask.bottom() && top(row) + tall > mask.top();
    // The parts of the highlights in this line.
    let highlights: Vec<_> = highlights
        .iter()
        .filter(|part| part.range.start < offset + line.len && part.range.end > offset)
        .collect();
    let lined = |highlight: &&TextHighlight| highlight.lines.iter().any(Option::is_some);
    let decorated = highlights.iter().any(lined)
        || runs.iter().any(|run| {
            run.underline.is_some() || run.strikethrough.is_some() || run.overline.is_some()
        });
    let over = |index: usize| {
        let mut over = highlights.iter().rev();
        over.find(|part| part.range.contains(&index) && (part.color.is_some() || !part.stroke))
    };
    let paint_glyphs = |window: &mut Window| {
        let Some(mut wrap) = (0..=wraps.len()).find(|&row| top(row) + tall > mask.top()) else {
            return;
        };
        let (mut left, from) = match wrap.checked_sub(1).map(|row| wraps[row] as usize) {
            Some(at) => (line.glyphs[at].x, at),
            None => (px(0.), 0),
        };
        let mut text_run = 0;
        let mut run_end = runs.first().map_or(0, |run| run.len);
        let mut spaced = (usize::MAX, 0, 0.);
        let (mut face, mut glyph_box) = (None, Rect::default());
        let (mut cluster, mut marks) = (usize::MAX, (usize::MAX, None));
        // Like Chrome, marks keep their color in selected text, unless the selection holds the whole line.
        let whole = highlights
            .iter()
            .find(|part| {
                part.range.start <= offset && part.range.end >= offset + line.len && !part.tight
            })
            .and_then(|part| part.color);
        for (at, glyph) in line.glyphs.iter().enumerate().skip(from) {
            if face.replace((glyph.font, glyph.size)) != Some((glyph.font, glyph.size)) {
                glyph_box = window.text_system().glyph_box(glyph.font, glyph.size);
            }
            if wraps.get(wrap) == Some(&(at as u32)) {
                wrap += 1;
                left = glyph.x;
            }
            if top(wrap) >= mask.bottom() {
                break;
            }
            let index = offset + glyph.index as usize;
            // Right-to-left text goes back.
            if index < run_end - runs[text_run].len {
                (text_run, run_end) = (0, runs[0].len);
            }
            while index >= run_end && text_run + 1 < runs.len() {
                text_run += 1;
                run_end += runs[text_run].len;
            }
            let highlight = over(index);
            let color = ink.unwrap_or_else(|| {
                (highlight.and_then(|highlight| highlight.color)).unwrap_or(runs[text_run].color)
            });
            let stroke = runs[text_run]
                .stroke
                .filter(|_| !glyph.emoji && highlight.is_none_or(|highlight| highlight.stroke));
            let fill = color.a > 0. || window.clips_text();
            if !fill && stroke.is_none_or(|(_, color)| color.a == 0.) {
                continue;
            }
            let gap = rows
                .get(wrap)
                .filter(|row| row.gap > px(0.))
                .map_or(px(0.), |row| {
                    if !line.sorted() {
                        if spaced.0 != wrap {
                            spaced = (wrap, 0, 0.);
                        }
                        let gap = row.gap * spaced.2;
                        spaced.2 += spaces(text.get(index..index + 1).unwrap_or(""));
                        return gap;
                    }
                    if spaced.0 != wrap || index < spaced.1 {
                        spaced = (wrap, row.start.min(index), 0.);
                    }
                    spaced.2 += spaces(text.get(spaced.1..index).unwrap_or(""));
                    spaced.1 = index;
                    row.gap * spaced.2
                });
            let position = point(
                origin.x + shift(wrap) + glyph.x - left + gap,
                origin.y + line_height * wrap + baseline + drop(wrap),
            );
            if let Some(mark) = &runs[text_run].emphasis
                && std::mem::replace(&mut cluster, index) != index
                // The text of the line, which has the ellipsis of truncated text.
                && let shown = &*text_line.text
                && let at = glyph.index as usize
                && shown.get(at..).and_then(|text| text.chars().next()).is_some_and(|c| {
                    // A character joined to the one before it is in its grapheme cluster.
                    let mut before = shown[..at].chars().rev();
                    let flag = |c: Option<char>| c.is_some_and(|c| ('\u{1f1e6}'..='\u{1f1ff}').contains(&c));
                    let joined = match before.next() {
                        Some('\u{200d}') => true,
                        last if flag(Some(c)) && flag(last) => !flag(before.next()),
                        _ => false,
                    };
                    !joined && emphasized(c)
                })
            {
                if marks.0 != text_run {
                    marks = (
                        text_run,
                        emphasis_glyph(&runs[text_run], mark, line.font_size, window),
                    );
                }
                if let Some((mark_glyph, center, rise)) = marks.1 {
                    // Like Chrome, the middle of the cluster's own advance, without letter spacing.
                    let last = (at + 1..line.glyphs.len())
                        .take_while(|&next| line.glyphs[next].index == glyph.index)
                        .last()
                        .unwrap_or(at);
                    let mut advance = line.right_of(last) - glyph.x - runs[text_run].letter_spacing;
                    // The padding of inline boxes that end the run or start the next one.
                    let end = line
                        .glyphs
                        .get(last + 1)
                        .map_or(line.len, |next| next.index as usize);
                    if offset + end == run_end {
                        advance -= runs[text_run].inset[1]
                            + runs.get(text_run + 1).map_or(px(0.), |next| next.inset[0]);
                    }
                    let middle = advance / 2.;
                    window.draw_glyph(
                        point(position.x + middle - center, position.y + rise),
                        mark_glyph.font,
                        mark_glyph.id,
                        mark_glyph.size,
                        ink.or(whole).unwrap_or(mark.color),
                        mark_glyph.emoji,
                        px(0.),
                    );
                }
            }
            // The font box has y up from the baseline. A miter of the stroke reaches twice its width past the outline.
            let reach = stroke.map_or(px(0.), |(width, _)| width * 2.);
            if !Rect::new(
                point(
                    position.x + glyph_box.left() - reach,
                    position.y - glyph_box.bottom() - reach,
                ),
                size(
                    glyph_box.size.width + reach * 2.,
                    glyph_box.size.height + reach * 2.,
                ),
            )
            .intersects(window.content_mask().bounds)
            {
                window.cull();
                continue;
            }
            if fill {
                let id = glyph.id;
                window.draw_glyph(
                    position,
                    glyph.font,
                    id,
                    glyph.size,
                    color,
                    glyph.emoji,
                    px(0.),
                );
            }
            // Like Chrome, the stroke draws above the fill.
            if let Some((width, stroke)) = stroke {
                let color = ink.unwrap_or(stroke);
                window.draw_glyph(
                    position, glyph.font, glyph.id, glyph.size, color, false, width,
                );
            }
        }
        if !text_line.hyphenates() {
            return;
        }
        let mut left = px(0.);
        for (row, &at) in wraps.iter().enumerate() {
            let next = &line.glyphs[at as usize];
            if let Some(hyphen) = text_line.hyphen(row).filter(|_| shown(row)) {
                let end = offset + next.index as usize;
                let mut run_end = 0;
                let run = runs.iter().find(|run| {
                    run_end += run.len;
                    end <= run_end
                });
                let highlight = over(end - 1).filter(|part| !part.tight);
                let color = (ink.or(highlight.and_then(|highlight| highlight.color)))
                    .or(run.map(|run| run.color))
                    .unwrap_or_default();
                // At the end of the row, the left one in right-to-left text.
                let x = origin.x
                    + shift(row)
                    + match text_line.rtl() {
                        true => -hyphen.line.width,
                        false => next.x - left + justify(row, end),
                    };
                for glyph in &hyphen.line.glyphs {
                    let position = point(x + glyph.x, top(row) + baseline);
                    window.draw_glyph(
                        position,
                        glyph.font,
                        glyph.id,
                        glyph.size,
                        color,
                        glyph.emoji,
                        px(0.),
                    );
                }
            }
            left = next.x;
        }
    };
    // Backgrounds and highlights of all rows draw first, then the lines below the glyphs. Lines through the glyphs
    // draw last.
    let row_parts = |pass: usize, window: &mut Window| {
        if pass > 0 && !decorated {
            return;
        }
        let mut start = 0;
        let mut left = px(0.);
        let mut first_run = (0, 0);
        let mut first_glyph = 0;
        let mut gaps: Vec<[Pixels; 2]> = Vec::new();
        for (row, boundary) in wraps.iter().map(Some).chain([None]).enumerate() {
            let end = text_line.row_index(row + 1);
            let right = boundary.map_or(line.width, |&at| line.glyphs[at as usize].x);
            let hyphen = text_line
                .hyphen(row)
                .map_or(px(0.), |hyphen| hyphen.line.width);
            let glyphs = first_glyph..boundary.map_or(line.glyphs.len(), |&at| at as usize);
            first_glyph = glyphs.end;
            if !shown(row) {
                window.cull();
                start = end;
                left = right;
                continue;
            }
            let row_left = origin.x + shift(row) - left;
            let top = top(row);
            let preserved = runs.first().is_some_and(|run| run.preserve);
            let rtl = text_line.rtl();
            // The ends of text in visual order. Right-to-left text can have more than one part.
            let parts =
                |a: usize, b: usize, hyphen: Pixels, each: &mut dyn FnMut(Pixels, Pixels)| {
                    if line.sorted() {
                        let end_x = if b == offset + end {
                            right + hyphen + justify(row, b)
                        } else {
                            line.x_at(b - offset) + justify(row, b)
                        };
                        return each(line.x_at(a - offset) + justify(row, a), end_x);
                    }
                    let gap = rows.get(row).map_or(px(0.), |row| row.gap);
                    for part in text_line.parts(row, a - offset..b - offset, gap, preserved) {
                        each(left + part.left, left + part.right);
                    }
                    if rtl && hyphen > px(0.) && b == offset + end {
                        each(left - hyphen, left);
                    }
                };
            while first_run.0 < runs.len() && first_run.1 + runs[first_run.0].len <= offset + start
            {
                first_run.1 += runs[first_run.0].len;
                first_run.0 += 1;
            }
            let mut run_start = first_run.1;
            for run in &runs[first_run.0..] {
                if run_start >= offset + end {
                    break;
                }
                let (a, b) = (
                    run_start.max(offset + start),
                    (run_start + run.len).min(offset + end),
                );
                run_start += run.len;
                let background = run.background_color.filter(|_| ink.is_none());
                if a >= b {
                    continue;
                }
                if pass == 0 {
                    if let Some(background) = background {
                        parts(a, b, hyphen, &mut |x, end_x| {
                            window.draw_rect(
                                Rect::new(
                                    point(row_left + x, top + baseline - line.ascent),
                                    size(end_x - x, line.ascent + line.descent),
                                ),
                                background,
                            )
                        });
                    }
                    // Like Chrome, a custom highlight is as tall as the font, and leaves out the hyphen and the spaces that hang at
                    // the end of the row.
                    for part in highlights.iter().filter(|part| part.tight) {
                        let (a, mut b) = (part.range.start.max(a), part.range.end.min(b));
                        if a < b
                            && b == offset + end
                            && boundary.is_some()
                            && let Some(shown) = text.get(a..b)
                        {
                            b = a + shown.trim_end_matches(' ').len();
                        }
                        if a >= b || part.background.a == 0. {
                            continue;
                        }
                        let system = window.text_system();
                        let size = run.font_size.unwrap_or(line.font_size);
                        let [ascent, descent, _] =
                            system.line_metrics(system.face(&run.font), size);
                        parts(a, b, px(0.), &mut |x, end_x| {
                            window.draw_rect(
                                Rect::new(
                                    point(row_left + x, top + baseline - ascent),
                                    renderer::size(end_x - x, ascent + descent),
                                ),
                                part.background,
                            )
                        });
                    }
                    continue;
                }
                // Lines under and over the text are below the glyphs.
                let passed = |[under, through, over]: [Option<renderer::Decoration>; 3]| match pass
                {
                    1 => [under, None, over],
                    _ => [None, through, None],
                };
                // Like Chrome, a collapsible space where the row wraps has no line.
                let mut shown = b;
                if !run.preserve && boundary.is_some() && b == offset + end {
                    shown = a + text.get(a..b).unwrap_or("").trim_end_matches(' ').len();
                }
                let own = [run.underline, run.strikethrough, run.overline];
                let marked = highlights.iter().filter(|part| lined(part)).map(|part| {
                    let (start, end) = (part.range.start.max(a), part.range.end.min(b));
                    (start, end.min(shown), px(0.), part.lines)
                });
                for (a, b, hyphen, lines) in std::iter::once((a, shown, hyphen, own)).chain(marked)
                {
                    let lines = passed(lines);
                    if a >= b || lines.iter().all(Option::is_none) {
                        continue;
                    }
                    // Like Chromium: in device pixels, from the rounded ascent of the run's font, lines as thick as a
                    // whole number of pixels, an underline below the baseline by half its thickness, a line-through a
                    // third of the ascent above it, and an overline on top of the ascent.
                    let scale = window.scale_factor();
                    let system = window.text_system();
                    let size = run.font_size.unwrap_or(line.font_size) * scale;
                    let ascent = f32::from(system.line_metrics(system.face(&run.font), size)[0]);
                    let base = ((top + baseline).0 * scale).round();
                    for (kind, decoration) in lines.into_iter().enumerate() {
                        let Some(decoration) = decoration else {
                            continue;
                        };
                        let thick = (decoration.thickness.0 * scale).max(1.);
                        let double = decoration.style == renderer::DecorationStyle::Double;
                        let wavy = decoration.style == renderer::DecorationStyle::Wavy;
                        let mut y = match kind {
                            0 if decoration.under => {
                                let system = window.text_system();
                                let em = system.metrics(system.face(&run.font)).em_ascent;
                                let offset =
                                    decoration.offset.map_or(0., |offset| offset.0 * scale);
                                base + (size.0 * (1. - em) + offset).floor() + 1.
                            }
                            0 => {
                                base + (decoration.offset).map_or((thick / 2.).ceil(), |offset| {
                                    (offset.0 * scale).round()
                                })
                            }
                            1 => (base - ascent / 3. - thick / 2.).round(),
                            _ => base - ascent - thick.floor(),
                        };
                        y -= match kind {
                            2 if double => thick.floor() + 1.,
                            1 | 2 if wavy => (thick + 1.) * kind as f32,
                            _ => 0.,
                        };
                        let mut ends = Vec::new();
                        parts(a, b, hyphen, &mut |x, end_x| ends.push([x, end_x]));
                        gaps.clear();
                        if decoration.skip_ink {
                            // Like Chrome, ink that reaches less than half a device pixel into the line makes no gap.
                            let [above, below] = if wavy {
                                let reach = renderer::wave_reach(thick);
                                [thick + 1. - reach, thick + 1. + reach]
                            } else if double {
                                [0., thick * 2. + 1.]
                            } else {
                                [0., thick]
                            };
                            let band = [y - base + above + 0.5, y - base + below - 0.5];
                            let around = px(thick.min(13.) / scale);
                            let system = window.text_system();
                            let (mut counted, mut gap) = (a, justify(row, a));
                            for glyph in &line.glyphs[glyphs.clone()] {
                                let index = offset + glyph.index as usize;
                                if !(a..b).contains(&index) || glyph.emoji {
                                    continue;
                                }
                                if index != counted {
                                    gap = justify(row, index);
                                    counted = index;
                                }
                                if let Some(ends) = system.intercept(
                                    glyph.font,
                                    glyph.id,
                                    glyph.size * scale,
                                    band.map(|y| y - glyph.y.0 * scale),
                                ) && !text
                                    .get(index..)
                                    .is_some_and(|text| text.starts_with(covered))
                                {
                                    let at = row_left + glyph.x + gap;
                                    let [left, right] = ends.map(|x| at + px(x / scale));
                                    gaps.push([left - around, right + around]);
                                }
                            }
                        }
                        for [x, end_x] in ends {
                            window.draw_line_between(
                                point(row_left + x, px(y / scale)),
                                end_x - x,
                                px(if wavy { thick } else { thick.floor() } / scale),
                                ink.or(decoration.color).unwrap_or_default(),
                                decoration.style,
                                &gaps,
                            );
                        }
                    }
                }
            }
            for highlight in highlights.iter().filter(|part| pass == 0 && !part.tight) {
                let a = highlight.range.start.max(offset + start);
                let b = highlight.range.end.min(offset + end);
                if a >= b {
                    continue;
                }
                parts(a, b, hyphen, &mut |x, end_x| {
                    if end_x > x && highlight.background.a > 0. {
                        window.draw_rect(
                            Rect::new(point(row_left + x, top), size(end_x - x, line_height)),
                            highlight.background,
                        );
                    }
                });
            }
            start = end;
            left = right;
        }
    };
    row_parts(0, window);
    row_parts(1, window);
    paint_glyphs(window);
    row_parts(2, window);
}

// The glyph of an emphasis mark at half the size of the text, the middle of its ink, and its baseline from the
// baseline of the text. Like Chrome, it is the first glyph of the mark text, beyond the ascent or descent of the text.
fn emphasis_glyph(
    run: &renderer::TextRun,
    mark: &renderer::Emphasis,
    font_size: Pixels,
    window: &Window,
) -> Option<(renderer::Glyph, Pixels, Pixels)> {
    let system = window.text_system();
    let size = run.font_size.unwrap_or(font_size);
    let shape = renderer::TextRun {
        len: mark.mark.len(),
        font: run.font.clone(),
        ..Default::default()
    };
    let mut glyph = *system
        .shape_line(&mark.mark, size, &shape)
        .line
        .glyphs
        .first()?;
    glyph.size = size / 2.;
    let scale = window.scale_factor();
    let [ascent, descent, _] = system.line_metrics(system.face(&run.font), size * scale);
    let em = system.metrics(glyph.font).em_ascent * f32::from(glyph.size) * scale;
    let rise = match mark.under {
        true => (descent.0 + em).ceil(),
        false => (-ascent.0 - (f32::from(glyph.size) * scale - em)).floor(),
    };
    let center = system
        .glyph_bounds(glyph.font, glyph.id, glyph.size)
        .map_or(0., |[left, right, ..]| (left + right) / 2.);
    Some((glyph, px(center), px(rise / scale)))
}

// Like Chrome, emphasis marks skip spaces, controls, combining marks and punctuation, except the signs that the CSS
// standard keeps.
fn emphasized(c: char) -> bool {
    matches!(
        c,
        '#' | '%' | '&' | '@' | '§' | '¶' | '‰' | '‱' | '※' | '⁂' | '＃' | '％' | '＆' | '＠'
    ) || !(c.is_whitespace()
        || c.is_control()
        || c.is_ascii_punctuation()
            && !matches!(c, '$' | '+' | '<' | '=' | '>' | '^' | '`' | '|' | '~')
        || matches!(c, '\u{a1}' | '\u{ab}' | '\u{ad}' | '\u{b7}' | '\u{bb}' | '\u{bf}'
            | '\u{300}'..='\u{36f}' | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}' | '\u{20d0}'..='\u{20ff}'
            | '\u{fe00}'..='\u{fe0f}' | '\u{fe20}'..='\u{fe2f}' | '\u{1f3fb}'..='\u{1f3ff}' | '\u{e0000}'..='\u{e007f}'
            | '\u{200b}'..='\u{2027}' | '\u{202a}'..='\u{202e}' | '\u{2030}'..='\u{205e}' | '\u{2060}'..='\u{2064}'
            | '\u{3001}'..='\u{3003}' | '\u{3008}'..='\u{3011}' | '\u{3014}'..='\u{301f}' | '\u{30fb}'
            | '\u{fe10}'..='\u{fe19}' | '\u{fe30}'..='\u{fe4f}' | '\u{feff}' | '\u{fffc}'
            | '\u{ff01}'..='\u{ff03}' | '\u{ff05}'..='\u{ff0a}' | '\u{ff0c}'..='\u{ff0f}' | '\u{ff1a}' | '\u{ff1b}'
            | '\u{ff1f}' | '\u{ff20}' | '\u{ff3b}'..='\u{ff3d}' | '\u{ff3f}' | '\u{ff5b}' | '\u{ff5d}'
            | '\u{ff5f}'..='\u{ff65}'))
}

// Like Chrome, lines don't skip the ink of these characters.
fn covered(ch: char) -> bool {
    matches!(ch, '_' | '\u{2e80}'..='\u{a4cf}' | '\u{f900}'..='\u{faff}' | '\u{fe30}'..='\u{fe4f}' | '\u{ff00}'..='\u{ffef}'
        | '\u{20000}'..='\u{3ffff}')
}

#[inline(never)]
pub(crate) fn spaces(text: &str) -> f32 {
    text.bytes().filter(|byte| *byte == b' ').count() as f32
}

pub(crate) fn text_position(
    layout: &renderer::Paragraph,
    text: &str,
    rows: &[crate::render::layout::Row],
    index: usize,
) -> Option<Point<Pixels>> {
    let mut position = layout.point_at(index)?;
    let row = ((position.y - layout.bounds().top()) / layout.line_height()).round();
    if let Some(row) = rows.get(row as usize) {
        let spaces = match row.gap > px(0.) && layout.reordered() {
            true => {
                let x = position.x - layout.bounds().left();
                let at = ((position.y - layout.bounds().top()) / layout.line_height()).round();
                layout.spaces_before(at as usize, x) as f32
            }
            false => spaces(text.get(row.start.min(index)..index).unwrap_or("")),
        };
        position.x += row.shift + row.gap * spaces;
        position.y += row.drop;
    }
    Some(position)
}

fn text_start(
    layout: &renderer::Paragraph,
    text: &str,
    rows: &[crate::render::layout::Row],
    index: usize,
    end: usize,
) -> Option<Point<Pixels>> {
    let at = text_position(layout, text, rows, index)?;
    // Compare the rows of the layout: rows that moved to other columns are not below in the window.
    let next = text
        .get(index..end)
        .and_then(|part| part.chars().next())
        .and_then(|first| layout.point_at(index + first.len_utf8()))
        .filter(|next| Some(next.y) > layout.point_at(index).map(|at| at.y));
    Some(next.map_or(at, |next| {
        let row = ((next.y - layout.bounds().top()) / layout.line_height()).round();
        let row = rows.get(row as usize);
        point(
            layout.bounds().left() + row.map_or(px(0.), |row| row.shift),
            next.y + row.map_or(px(0.), |row| row.drop),
        )
    }))
}

pub(crate) fn text_rects(
    layout: &renderer::Paragraph,
    text: &str,
    rows: &[crate::render::layout::Row],
    [start, end]: [usize; 2],
    font: [Pixels; 2],
) -> Vec<[f32; 4]> {
    let rect = |a: Point<Pixels>, b: Point<Pixels>| {
        [
            f32::from(a.x),
            f32::from(a.y + font[0]),
            f32::from(b.x - a.x),
            f32::from(font[1]),
        ]
    };
    if start == end {
        return text_position(layout, text, rows, start)
            .map(|at| vec![rect(at, at)])
            .unwrap_or_default();
    }
    if let Some(parts) = visual_parts(layout, rows, &(start..end), true) {
        return parts
            .into_iter()
            .map(|(_, top, part)| rect(point(part.left, top), point(part.right, top)))
            .collect();
    }
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in layout.lines().iter() {
        starts.push(offset);
        starts.extend((1..line.rows()).map(|row| offset + line.row_index(row)));
        offset += line.len() + 1;
    }
    let mut rects = Vec::new();
    for (index, &row) in starts.iter().enumerate() {
        let next = starts.get(index + 1).copied().unwrap_or(text.len());
        let (from, mut to) = (start.max(row), end.min(next));
        // As in browsers, the spaces at a soft wrap hang, so a row's rectangle ends before them.
        if to == next
            && index + 1 < starts.len()
            && let Some(row) = text.get(from..to)
        {
            to = from + row.trim_end_matches(' ').len();
        }
        if from >= to {
            continue;
        }
        if let Some((a, b)) =
            text_start(layout, text, rows, from, to).zip(text_position(layout, text, rows, to))
        {
            rects.push(rect(a, b));
        }
    }
    rects
}

// The parts of `range` in text of several directions, with their rows and their places in the window.
fn visual_parts(
    layout: &renderer::Paragraph,
    rows: &[crate::render::layout::Row],
    range: &std::ops::Range<usize>,
    hang: bool,
) -> Option<Vec<(usize, Pixels, renderer::Part)>> {
    if !layout.reordered() {
        return None;
    }
    let lines = layout.lines();
    let bounds = layout.bounds();
    let (mut parts, mut offset, mut row) = (Vec::new(), 0, 0);
    for line in lines.iter() {
        for at in 0..line.rows() {
            let own = range.start.max(offset + line.row_index(at))
                ..range.end.min(offset + line.row_index(at + 1));
            if own.start < own.end {
                let placed = rows.get(row);
                let [shift, gap, drop] = placed.map_or([px(0.); 3], |placed| {
                    [placed.shift, placed.gap, placed.drop]
                });
                let top = bounds.top() + layout.line_height() * row as f32 + drop;
                let left = bounds.left() + shift;
                for part in line.parts(
                    at,
                    own.start - offset..own.end - offset,
                    gap,
                    hang && line.preserves(),
                ) {
                    let part = renderer::Part {
                        left: left + part.left,
                        right: left + part.right,
                        first: offset + part.first,
                        last: offset + part.last,
                        ..part
                    };
                    parts.push((row, top, part));
                }
            }
            row += 1;
        }
        offset += line.len() + 1;
    }
    Some(parts)
}

/// The parts of an inline box whose line places its edges: the top of the row, the left and the right edge in the
/// window, and whether the part has the sides of the box.
fn box_parts(
    layout: &renderer::Paragraph,
    rows: &[crate::render::layout::Row],
    id: usize,
) -> Vec<(Pixels, [Pixels; 2], [bool; 2])> {
    let (mut parts, mut first) = (Vec::new(), 0);
    let bounds = layout.bounds();
    for line in layout.lines().iter() {
        let gap = |row: usize| rows.get(first + row).map_or(px(0.), |row| row.gap);
        for (row, edges, sides) in line.box_parts(id, gap) {
            let row = first + row;
            let [shift, drop] = rows
                .get(row)
                .map_or([px(0.); 2], |row| [row.shift, row.drop]);
            let top = bounds.top() + layout.line_height() * row as f32 + drop;
            parts.push((top, edges.map(|x| bounds.left() + shift + x), sides));
        }
        first += line.rows();
    }
    parts
}

fn inline_box(
    layout: &renderer::Paragraph,
    text: &str,
    rows: &[crate::render::layout::Row],
    range: &std::ops::Range<usize>,
    style: Option<(&css::properties::Style, css::value::units::Lengths)>,
    base: Pixels,
    window: &Window,
) -> Option<(Point<Pixels>, Point<Pixels>, Pixels, Pixels)> {
    let visual = visual_parts(layout, rows, range, false).filter(|parts| !parts.is_empty());
    let (a, b) = match visual {
        Some(parts) => {
            let (first, last) = (&parts[0], &parts[parts.len() - 1]);
            let ends = |row: usize| {
                let own = parts.iter().filter(move |part| part.0 == row);
                own.fold(
                    [px(f32::INFINITY), px(f32::NEG_INFINITY)],
                    |ends, (.., part)| [ends[0].min(part.left), ends[1].max(part.right)],
                )
            };
            (
                point(ends(first.0)[0], first.1),
                point(ends(last.0)[1], last.1),
            )
        }
        None => (
            text_start(layout, text, rows, range.start, range.end)?,
            text_position(layout, text, rows, range.end)?,
        ),
    };
    let Some((style, units)) = style else {
        return Some((a, b, px(0.), layout.line_height()));
    };
    let system = window.text_system();
    let scale = window.scale_factor();
    let [ascent, descent, _] = system
        .line_metrics(system.face(&style::font(style)), px(units.font.em * scale))
        .map(|value| value / scale);
    Some((a, b, base - ascent, ascent + descent))
}

pub(crate) fn text_index(
    layout: &renderer::Paragraph,
    text: &str,
    rows: &[crate::render::layout::Row],
    mut position: Point<Pixels>,
    nearest: bool,
) -> Result<usize, usize> {
    let (top, line_height) = (layout.bounds().top(), layout.line_height());
    let split = rows.windows(2).any(|pair| pair[1].drop < pair[0].drop);
    // Above the text is its start and below it its end, as for text without rows.
    let bounds = layout.bounds();
    if !split && (position.y < bounds.top() || position.y >= bounds.bottom()) {
        return layout.index_at(position, nearest);
    }
    let row = if split {
        // Rows in columns: the nearest row box to the point.
        let left = layout.bounds().left();
        let distance = |(index, row): (usize, &crate::render::layout::Row)| {
            let y = top + line_height * index as f32 + row.drop;
            let dx = (left + row.shift - position.x)
                .max(position.x - left - row.right)
                .max(px(0.));
            let dy = (y - position.y)
                .max(position.y - y - line_height)
                .max(px(0.));
            f32::from(dx * dx.0 + dy * dy.0)
        };
        (0..rows.len())
            .min_by(|&a, &b| distance((a, &rows[a])).total_cmp(&distance((b, &rows[b]))))
            .unwrap_or(0)
    } else {
        rows.iter()
            .enumerate()
            .rposition(|(index, row)| {
                top + line_height * index as f32 + row.drop - row.above <= position.y
            })
            .unwrap_or(0)
    };
    let Some(placed) = rows.get(row) else {
        return layout.index_at(position, nearest);
    };
    position.y = (position.y - placed.drop)
        .max(top + line_height * row as f32)
        .min(top + line_height * (row as f32 + 1.) - px(0.01));
    if placed.gap > px(0.) {
        let end = rows.get(row + 1).map_or(text.len(), |next| next.start);
        return (placed.start..=end)
            .filter(|&index| text.is_char_boundary(index))
            .filter_map(|index| Some((index, text_position(layout, text, rows, index)?.x)))
            .min_by(|a, b| {
                f32::from((a.1 - position.x).abs()).total_cmp(&f32::from((b.1 - position.x).abs()))
            })
            .map(|(index, _)| index)
            .ok_or(placed.start);
    }
    position.x -= placed.shift;
    layout.index_at(position, nearest)
}

fn first_line(
    data: &crate::render::inline::InlineText,
    width: Option<Pixels>,
    font_size: Pixels,
    text_system: &renderer::TextSystem,
) -> (SharedString, Rc<[renderer::TextRun]>) {
    let Some((base, pseudo, mode)) = data.first_line.as_deref() else {
        return (data.text.clone(), data.runs.clone());
    };
    let mut text = data.text.to_string();
    let len = text.find('\n').unwrap_or(text.len());
    let mut first = text[..len].to_owned();
    if let Some(mode) = mode {
        style::text_transform(&mut first, 0..len, mode);
    }
    let lines = text_system.shape_text(
        &first,
        font_size,
        &style::restyle(&data.runs, 0..len, base, pseudo),
        &renderer::Wrapping {
            width,
            max_lines: Some(2),
            ..Default::default()
        },
    );
    let end = lines.first().map_or(len, |line| line.row_index(1));
    if let Some(mode) = mode {
        style::text_transform(&mut text, 0..end, mode);
    }
    (
        text.into(),
        style::restyle(&data.runs, 0..end, base, pseudo).into(),
    )
}

pub(crate) struct InlineText {
    pub(crate) data: Rc<crate::render::inline::InlineText>,
    /// Rows can grow around runs or fallback glyphs, see `crate::render::inline::InlineText::mixed`.
    pub(crate) mixed: bool,
    pub(crate) view: Rc<super::super::ViewState>,
    pub(crate) atoms: Rc<[AtomSlot]>,
    // The static places in the text.
    pub(crate) marks: Rc<[std::cell::Cell<MarkPlace>]>,
    // Each line starts at the side of its own direction.
    pub(crate) plaintext: bool,
    // Whether rows stretch, and rows that end a line of the text.
    pub(crate) justify: [bool; 2],
    // The alignment of the rows that end a line, from `text-align-last`.
    pub(crate) last: Option<renderer::TextAlign>,
    /// Right-to-left text: indents and rows that overflow start at the right.
    pub(crate) rtl: bool,
    /// The lines go from the bottom, like in `vertical-lr`.
    pub(crate) reversed: bool,
    /// The margins before the block, where emphasis marks over the first row can reach.
    pub(crate) room: f32,
    pub(crate) shaped: std::cell::RefCell<Option<(renderer::Paragraph, Rc<[renderer::TextRun]>)>>,
    pub(crate) prepainted: std::cell::RefCell<Prepainted>,
}

#[derive(Default)]
pub(crate) struct Prepainted {
    hitboxes: Vec<(renderer::Region, renderer::Cursor)>,
    rows: crate::render::layout::Rows,
    layouts: Vec<(usize, crate::render::layout::TextLayout)>,
    rects: Vec<(usize, [f32; 4])>,
    origin: Point<Pixels>,
    // Where the text that prepaint shaped is placed. The renderer moves only the text of its own nodes.
    shaped_at: Point<Pixels>,
    // Some rows moved to later columns, so rows don't go down in order.
    split: bool,
}

pub(crate) struct AtomSlot {
    pub(crate) size: [f32; 3],
    pub(crate) align: Option<AlignItems>,
    pub(crate) at: std::cell::Cell<Point<Pixels>>,
}

// Where the text has a mark, and the direction there in mixed text.
pub(crate) type MarkPlace = (Point<Pixels>, Option<bool>);

// Moves the mark of a static place to where the text has it. A box that was a block is at the start of the line.
pub(crate) struct MarkBox {
    pub(crate) marks: Rc<[std::cell::Cell<MarkPlace>]>,
    pub(crate) index: usize,
    pub(crate) key: usize,
    pub(crate) block: bool,
}

impl renderer::Hooks for MarkBox {
    fn cached(&self) -> bool {
        true
    }

    fn prepaint(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let (at, mut rtl) = self.marks[self.index].get();
        let mut by = at - cx.exact.origin;
        if self.block {
            (by.x, rtl) = (px(0.), None);
        }
        cx.window.place_mark(self.key as u64, by, rtl);
        inner(cx.window);
    }
}

pub(crate) struct AtomBox {
    pub(crate) slots: Rc<[AtomSlot]>,
    pub(crate) index: usize,
    pub(crate) node: usize,
    pub(crate) view: Rc<super::super::ViewState>,
    // The width that the atom leaves free in a box as wide as the block, see `AtomRest`.
    pub(crate) rest: Option<Rc<std::cell::Cell<Pixels>>>,
}

// Fills the box of an atom with percent sizes. That box is as wide as the block, so the percentages
// resolve against the block, and the atom is as wide as the box without this filler.
pub(crate) struct AtomRest(pub(crate) Rc<std::cell::Cell<Pixels>>);

impl renderer::Hooks for AtomRest {
    fn cached(&self) -> bool {
        true
    }

    fn prepaint(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.0.set(cx.exact.size.width);
        inner(cx.window);
    }
}

impl renderer::Hooks for AtomBox {
    fn cached(&self) -> bool {
        true
    }

    fn prepaint(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let slot = &self.slots[self.index];
        let offset = slot.at.get() - cx.bounds.origin;
        // The filler reports its width in its own prepaint, so the children go first.
        cx.window
            .scope(renderer::Scope::Offset(offset), |window| inner(window));
        let height = f32::from(cx.exact.size.height);
        let rest = self.rest.as_ref().map_or(px(0.), |rest| rest.get());
        let size = [
            f32::from(cx.exact.size.width - rest),
            height,
            cx.baselines[0].map_or(height, f32::from),
        ];
        if slot
            .size
            .iter()
            .zip(&size)
            .any(|(old, new)| (old - new).abs() > 0.01)
        {
            let view = &*self.view;
            view.store
                .borrow_mut()
                .state(view.id)
                .layout
                .atoms
                .insert(self.node, size);
            cx.window.redraw();
        }
    }
}

fn with_atoms(
    data: &crate::render::inline::InlineText,
    sizes: impl Fn(usize) -> f32,
    runs: &mut Rc<[renderer::TextRun]>,
) {
    if data.first_line.is_some() || data.atoms.is_empty() {
        return;
    }
    let mut sized = runs.to_vec();
    for (index, atom) in data.atoms.iter().enumerate() {
        if let Some(run) = sized.get_mut(atom.run) {
            run.inset[1] += px(sizes(index));
        }
    }
    *runs = sized.into();
}

type MeasuredSize = ((bool, Option<Pixels>), renderer::Size<Pixels>);

struct Measured {
    data: Rc<crate::render::inline::InlineText>,
    margins: f32,
    atoms: Box<[([f32; 3], Option<AlignItems>)]>,
    styled: std::cell::RefCell<Option<renderer::TextStyle>>,
    measured: std::cell::Cell<[Pixels; 2]>,
    cached: std::cell::Cell<Option<MeasuredSize>>,
    baselines: std::cell::Cell<[Pixels; 2]>,
    // The floats beside the lines in the last layout.
    floats: std::cell::RefCell<Option<Rc<[[f32; 4]]>>>,
}

impl PartialEq for Measured {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.data, &other.data)
            && self.atoms == other.atoms
            && self.margins == other.margins
    }
}

impl From<InlineText> for renderer::Node {
    fn from(text: InlineText) -> Self {
        let data = &text.data;
        if data.first_line.is_none() && !text.mixed {
            return renderer::text(data.text.clone(), Some(data.runs.clone())).hooks(text);
        }
        renderer::node()
            .measure(
                Measured {
                    data: data.clone(),
                    margins: text.room,
                    atoms: text
                        .atoms
                        .iter()
                        .map(|slot| (slot.size, slot.align))
                        .collect(),
                    styled: Default::default(),
                    measured: Default::default(),
                    cached: std::cell::Cell::new(None),
                    baselines: std::cell::Cell::new([px(0.); 2]),
                    floats: Default::default(),
                },
                |kept, style, known, available, window| {
                    let Measured {
                        data,
                        margins,
                        atoms,
                        styled,
                        measured,
                        cached,
                        baselines: kept_baselines,
                        floats: kept_floats,
                    } = kept;
                    if styled.borrow().as_ref() != Some(style) {
                        cached.set(None);
                        *styled.borrow_mut() = Some(style.clone());
                    }
                    // Like the renderer's paragraphs, a line is at most 2^25 device pixels tall.
                    let font_size = style.font_size;
                    let line_height = style
                        .line_height
                        .min(px(33_554_431. / window.scale_factor().max(1.)));
                    let wraps = style.white_space == renderer::WhiteSpace::Normal;
                    let min_content = wraps
                        && known.width.is_none()
                        && available.width == renderer::AvailableSpace::MinContent;
                    let room = known.width.or(match available.width {
                        renderer::AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    });
                    let width = room.filter(|_| wraps);
                    // A percentage of `text-indent` is of the width of the paragraph.
                    let indent = style.indent(room.unwrap_or_default());
                    let key = (
                        min_content,
                        room.filter(|_| style.text_indent_percent != 0.).or(width),
                    );
                    let floats =
                        (window.text_system().floats()).filter(|_| room.is_some() && !min_content);
                    // Only a layout with a width keeps its floats for prepaint. The cache holds results with them.
                    if room.is_some() && *kept_floats.borrow() != floats {
                        cached.set(None);
                        *kept_floats.borrow_mut() = floats.clone();
                    }
                    let kept = *kept_floats.borrow() == floats;
                    if let Some((_, size)) = cached
                        .get()
                        .filter(|(cached, _): &(_, renderer::Size<Pixels>)| kept && *cached == key)
                    {
                        measured.set([width.unwrap_or_default(), size.width]);
                        return (size, Some(kept_baselines.get()));
                    }
                    let system = window.text_system().clone();
                    let wrap = if min_content { Some(px(0.)) } else { width };
                    let (text, mut runs) = first_line(data, wrap, font_size, &system);
                    with_atoms(data, |index| atoms[index].0[0], &mut runs);
                    let mut size = renderer::Size::<Pixels>::default();
                    let lines = system.shape_beside(
                        &text,
                        font_size,
                        &runs,
                        &renderer::Wrapping {
                            width: wrap,
                            max_lines: style.line_clamp.filter(|_| !min_content),
                            indent,
                            wrap: if min_content {
                                renderer::TextWrap::Wrap
                            } else {
                                style.text_wrap
                            },
                            word_break: match style.word_break {
                                renderer::WordBreak::BreakWord if min_content => {
                                    renderer::WordBreak::Normal
                                }
                                word_break => word_break,
                            },
                        },
                        floats
                            .as_deref()
                            .map(|floats| (floats, f32::from(line_height))),
                    );
                    for (index, line) in lines.iter().enumerate() {
                        let mut line_size = line.extent(line_height);
                        if line.breaks.is_empty() {
                            line_size.width = (line_size.width + indent.of(index)[0]).max(px(0.));
                        }
                        size.height += line_size.height;
                        size.width = size.width.max(line_size.width);
                    }
                    if min_content {
                        size.width = renderer::widest_piece(&lines, px(0.), indent);
                    }
                    // Rows that moved below floats make the text taller.
                    if let Some(&[.., top]) = lines.last().and_then(|line| line.shifts.last()) {
                        size.height = px(top) + line_height;
                    }
                    let strut = (system.face(&style.font()), font_size, line_height);
                    let scale = window.scale_factor();
                    let extents = super::super::layout::line::row_extents(
                        lines.iter(),
                        &runs,
                        &data.line_heights,
                        &data
                            .atoms
                            .iter()
                            .zip(atoms.iter())
                            .map(|(atom, &(size, align))| (atom.at, size, align))
                            .collect::<Vec<_>>(),
                        strut,
                        &system,
                        scale,
                        *margins,
                    );
                    for [above, below] in &extents {
                        size.height += *above + *below;
                    }
                    let base = system.baseline(strut.0, font_size, line_height, scale);
                    let baselines = [
                        extents.first().map_or(px(0.), |row| row[0]) + base,
                        size.height - extents.last().map_or(px(0.), |row| row[1]) - line_height
                            + base,
                    ];
                    if kept {
                        cached.set(Some((key, size)));
                        kept_baselines.set(baselines);
                    }
                    measured.set([width.unwrap_or_default(), size.width]);
                    (size, Some(baselines))
                },
            )
            .hooks(text)
    }
}

// The mirror of a node of the text. A spliced inline box has a part in each line of text, which its block joins.
fn place(
    dom: &mut crate::render::PageState,
    data: &crate::render::inline::InlineText,
    id: usize,
    rect: [f32; 4],
) {
    if !dom.geometry.shapes.is_empty() {
        dom.geometry.shapes.remove(&id);
    }
    if !data.cuts.contains(&id) {
        dom.geometry.mirror.insert(id, rect);
        return;
    }
    dom.geometry
        .pieces
        .entry(id)
        .or_default()
        .push((data.key, rect));
}

// Drops the parts that the text gave its spliced boxes before it gives them again.
fn unplace(dom: &mut crate::render::PageState, data: &crate::render::inline::InlineText) {
    for cut in &data.cuts {
        if let Some(pieces) = dom.geometry.pieces.get_mut(cut) {
            pieces.retain(|(key, _)| *key != data.key);
        }
    }
}

impl InlineText {
    // Places the marks of static places where Chrome has them. A box that was inline is after the character before
    // it, and a space that hangs at the end of its row is not between them. A box that was a block is at the start
    // of the row, below it when the row has content before the mark.
    fn place_marks(
        &self,
        layout: &renderer::Paragraph,
        rows: &[crate::render::layout::Row],
        extents: &[[Pixels; 2]],
        style: &renderer::TextStyle,
        origin: Point<Pixels>,
    ) {
        let (text, height, bounds) = (&*self.data.text, layout.line_height(), layout.bounds());
        // The start of each row, and whether the row before it is wider than the text with its last space.
        let mut starts = Vec::new();
        let mut offset = 0;
        for line in layout.lines().iter() {
            starts.push((offset, false));
            let mut left = px(0.);
            for (row, &at) in line.breaks.iter().enumerate() {
                let glyph = &line.line.glyphs[at as usize];
                let wide = glyph.x - left > bounds.size.width + px(0.01);
                starts.push((offset + line.row_index(row + 1), wide));
                left = glyph.x;
            }
            offset += line.len() + 1;
        }
        for (mark, place) in self.data.marks.iter().zip(&*self.marks) {
            let at = mark.at;
            let mut row = starts
                .partition_point(|&(start, _)| start < at)
                .saturating_sub(1);
            let mut x = None;
            if at > text.len() {
                row = starts.len();
            } else if let Some(&(_, wide)) = starts.get(row + 1).filter(|next| next.0 == at) {
                let before = text.get(..at).unwrap_or_default();
                let visible = before.trim_end_matches(' ').len();
                // A space that doesn't fit takes the mark to the next row.
                if before.ends_with('\n') || wide && visible < at {
                    row += 1;
                } else {
                    x = text_position(layout, text, rows, visible).map(|at| at.x);
                }
            }
            let start = starts.get(row).map_or(at, |start| start.0);
            let placed = rows.get(row).copied().unwrap_or_default();
            let [above, below] = extents.get(row).copied().unwrap_or_default();
            let mut y = bounds.top() + height * row as f32;
            y += match rows.get(row).or(rows.last()) {
                Some(last) if row < starts.len() => last.drop - last.above,
                Some(last) => last.drop + extents.last().map_or(px(0.), |extent| extent[1]),
                None => px(0.),
            };
            let visible = match starts.get(row + 1) {
                Some(next) if next.0 == at => text[..at].trim_end_matches(' ').len(),
                _ => at,
            };
            let mut first = [0; 2];
            let mixed = layout.lines().iter().find_map(|line| {
                let [offset, rows] = first;
                first = [offset + line.len() + 1, rows + line.rows()];
                let inside = row < first[1] && at <= offset + line.len();
                inside.then(|| {
                    line.mark(
                        visible - offset,
                        row - rows,
                        placed.gap,
                        [mark.lead, mark.trail].map(px),
                    )
                })
            });
            let mixed = mixed.flatten().filter(|_| !mark.block);
            let x = if let Some((x, _)) = mixed {
                bounds.left() + placed.shift + x
            } else if mark.block {
                if start < at.min(text.len()) || mark.opened {
                    y += above + height + below;
                }
                px(0.)
            } else if row == starts.len() || start == at && text.is_empty() {
                match style.text_align {
                    renderer::TextAlign::Left => bounds.left(),
                    renderer::TextAlign::Center => bounds.center().x,
                    renderer::TextAlign::Right => bounds.right(),
                }
            } else if self.rtl
                && (text.get(at..starts.get(row + 1).map_or(text.len(), |next| next.0)))
                    .is_none_or(|rest| rest.trim().is_empty())
            {
                bounds.left() + placed.shift
            } else if let Some(x) = x {
                x
            } else if start == at && row > 0 {
                bounds.left() + placed.shift
            } else {
                text_position(layout, text, rows, at)
                    .map_or(bounds.left(), |at| at.x - px(mark.trail))
            };
            let rtl = mixed.map(|(_, rtl)| rtl);
            place.set((point(x, y) - bounds.origin + origin, rtl));
        }
    }

    fn cursors<'a>(
        &'a self,
        layout: &'a renderer::Paragraph,
        rows: &'a crate::render::layout::Rows,
        bounds: Rect<Pixels>,
    ) -> impl Iterator<Item = (Rect<Pixels>, renderer::Cursor)> + 'a {
        self.data
            .cursors
            .iter()
            .filter_map(move |&(index, cursor)| {
                let range = &self.data.ranges.get(index)?.1;
                let (a, b) = text_position(layout, &self.data.text, rows, range.start)
                    .zip(text_position(layout, &self.data.text, rows, range.end))?;
                let rect = if a.y == b.y {
                    Rect::new(a, size(b.x - a.x, layout.line_height()))
                } else {
                    Rect::new(
                        point(bounds.left(), a.y),
                        size(bounds.size.width, b.y - a.y + layout.line_height()),
                    )
                };
                Some((rect, cursor))
            })
    }
}

impl renderer::Hooks for InlineText {
    fn overflows_left(&self) -> bool {
        self.rtl
    }

    fn bounded(&self) -> bool {
        true
    }

    fn cached(&self) -> bool {
        true
    }

    fn skippable(&self) -> bool {
        true
    }

    fn skips_inside(&self) -> bool {
        true
    }

    fn skip_inside(&self, exact: Rect<Pixels>, window: &mut Window) {
        self.skip(exact, window);
    }

    fn shift_inside(&self, by: Point<Pixels>, exact: Rect<Pixels>, window: &mut Window) {
        self.shift(by, exact, window);
    }

    // The layouts go back into the mirror, which each frame clears.
    fn shifts(&self) -> bool {
        true
    }

    // The renderer moved the text layouts.
    fn shift(&self, by: Point<Pixels>, exact: Rect<Pixels>, window: &mut Window) -> bool {
        if let Some((layout, _)) = &*self.shaped.borrow() {
            layout.shift(by);
            self.prepainted.borrow_mut().shaped_at += by;
        }
        self.skip(exact, window)
    }

    fn skip(&self, _: Rect<Pixels>, _: &mut Window) -> bool {
        let view = &*self.view;
        let mut store = view.store.borrow_mut();
        let dom = store.state(view.id);
        for (id, entry) in self.prepainted.borrow().layouts.iter() {
            dom.layout.text_layouts.insert(*id, entry.clone());
        }
        true
    }

    fn prepaint(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        let moves: Vec<_> = cx.row_moves().collect();
        let [top, right, bottom, left] = cx.inset;
        let inside = |bounds: Rect<Pixels>| {
            Rect::new(
                bounds.origin + point(left, top),
                size(
                    bounds.size.width - left - right,
                    bounds.size.height - top - bottom,
                ),
            )
        };
        let (bounds, exact, style) = (inside(cx.bounds), inside(cx.exact), cx.text.clone());
        let measured = cx.state::<Measured>().map(|kept| kept.measured.get());
        let cx_floats = cx
            .state::<Measured>()
            .and_then(|kept| kept.floats.borrow().clone());
        let node_layout = cx.text_layout().cloned();
        let window = &mut *cx.window;
        if let Some(measured) = measured {
            // Snapped bounds can be narrower than measured text. Do not wrap or truncate again.
            let wrap = measured
                .into_iter()
                .filter(|width| *width >= bounds.size.width && *width - bounds.size.width < px(1.))
                .fold(bounds.size.width, Pixels::max);
            let width = (style.white_space == renderer::WhiteSpace::Normal).then_some(wrap);
            let font_size = style.font_size;
            let system = window.text_system().clone();
            let (text, mut runs) = first_line(&self.data, width, font_size, &system);
            with_atoms(&self.data, |index| self.atoms[index].size[0], &mut runs);
            // Like Chromium, text starts at its exact place, and only each row's baseline snaps to a device pixel.
            let placed = renderer::Rect::new(exact.origin, bounds.size);
            let floats = cx_floats;
            let system = window.text_system().clone();
            let scale = window.scale_factor();
            let layout = system.beside(floats, || {
                renderer::Paragraph::shape(text, runs.clone(), &style, wrap, placed, &system, scale)
            });
            *self.shaped.borrow_mut() = Some((layout, runs));
        }
        let shaped = self.shaped.borrow();
        let (layout, runs) = match (&*shaped, &node_layout) {
            (Some((layout, runs)), _) => (layout, &runs[..]),
            (None, Some(layout)) => (layout, &self.data.runs[..]),
            (None, None) => return,
        };
        let mut rows = Vec::new();
        // Like Chrome, a percentage is of the content width of the block, which its paint hook records first.
        let base = (style.text_indent_percent != 0.)
            .then(|| {
                let store = self.view.store.borrow();
                let (page, dom) = (
                    store.pages.get(&self.view.id)?,
                    store.page_state(self.view.id)?,
                );
                let &(node, _) = self.data.ranges.first()?;
                let (up, inner) = std::iter::successors(page.doc.nodes.get(node)?.parent, |&up| {
                    page.doc.nodes[up].parent
                })
                .find_map(|up| Some((up, dom.layout.cull.widths.get(&up)?)))?;
                // The text of a flex or grid container is an anonymous item that fits its content.
                let display = dom
                    .styles
                    .last()?
                    .values
                    .get(up)?
                    .get(crate::css::properties::Prop::DISPLAY);
                (!display
                    .is_some_and(|display| display.ends_with("flex") || display.ends_with("grid")))
                .then(|| px(inner[0]))
            })
            .flatten()
            .unwrap_or(bounds.size.width);
        let indent = style.indent(base);
        let strut = window.text_system().face(&style.font());
        let extents = super::super::layout::line::row_extents(
            layout.lines().iter(),
            runs,
            &self.data.line_heights,
            &self
                .data
                .atoms
                .iter()
                .zip(self.atoms.iter())
                .map(|(atom, slot)| (atom.at, slot.size, slot.align))
                .collect::<Vec<_>>(),
            (strut, style.font_size, layout.line_height()),
            window.text_system(),
            window.scale_factor(),
            self.room,
        );
        let view = &*self.view;
        let mut lead_styles = None;
        let mut drop = px(0.);
        let banded = layout.lines().iter().any(|line| !line.shifts.is_empty());
        if style.text_align != renderer::TextAlign::Left
            || self.rtl
            || banded
            || !self.data.boxes.is_empty()
            || !self.data.outlines.is_empty()
            || indent != renderer::Indent::default()
            || self.justify != [false; 2]
            || self.last.is_some()
            || self.plaintext
            || self.reversed
            || extents.iter().flatten().any(|extra| *extra > px(0.))
            || !moves.is_empty()
        {
            let mut offset = 0;
            for (index, line) in layout.lines().iter().enumerate() {
                let glyphs = &line.line.glyphs;
                let (rtl, side) = match (self.plaintext, line.rtl()) {
                    (false, _) => (self.rtl, style.text_align),
                    (true, false) => (false, renderer::TextAlign::Left),
                    (true, true) => (true, renderer::TextAlign::Right),
                };
                let mut left = px(0.);
                let mut start = offset;
                for (line_rows, boundary) in line.breaks.iter().map(Some).chain([None]).enumerate()
                {
                    let lead = line.lead(line_rows).filter(|_| line_rows > 0);
                    let (right, end) = boundary.map_or((line.line.width, line.line.width), |&at| {
                        let x =
                            glyphs[at as usize].x - line.lead(line_rows + 1).unwrap_or_default();
                        let hanging = glyphs[..at as usize].iter().rev().take_while(|glyph| {
                            glyph.x >= left
                                && self.data.text.as_bytes().get(offset + glyph.index as usize)
                                    == Some(&b' ')
                        });
                        (x, hanging.last().map_or(x, |glyph| glyph.x))
                    });
                    let hyphen = line.hyphen(line_rows);
                    let hyphen = hyphen.map_or(px(0.), |hyphen| hyphen.line.width);
                    let end = end + hyphen;
                    let indent = indent.of(index)[usize::from(line_rows > 0)];
                    // Beside floats, a row starts after the left ones, and moves down below the ones it doesn't fit.
                    let [inset, outset, lowered] = line
                        .shifts
                        .get(line_rows)
                        .map_or([0.; 3], |&[left, right, top]| {
                            [
                                left,
                                right,
                                top - f32::from(layout.line_height()) * rows.len() as f32,
                            ]
                        })
                        .map(px);
                    let free = bounds.size.width - inset - outset - indent - (end - left);
                    let next = offset + line.row_index(line_rows + 1);
                    let gaps = spaces(self.data.text.get(start..next).unwrap_or("").trim_end());
                    let ends = boundary.is_none();
                    let justified = self.justify[usize::from(ends)] && gaps > 0.;
                    // The indent and a row that overflows are at the start edge, the right one in right-to-left text.
                    let first = inset + if rtl { px(0.) } else { indent };
                    let (shift, gap) = if free < px(0.) {
                        (first + if rtl { free } else { px(0.) }, px(0.))
                    } else if justified {
                        (first, free / gaps)
                    } else {
                        let shift = match self.last.filter(|_| ends).unwrap_or(side) {
                            renderer::TextAlign::Left => px(0.),
                            renderer::TextAlign::Center => free / 2.,
                            renderer::TextAlign::Right => free,
                        };
                        (first + shift, px(0.))
                    };
                    let [above, below] = extents.get(rows.len()).copied().unwrap_or_default();
                    rows.push(crate::render::layout::Row {
                        // The hyphen of a right-to-left row is at its left.
                        shift: shift + lead.unwrap_or_default() + if rtl { hyphen } else { px(0.) },
                        right: shift + end - left + gap * gaps,
                        gap,
                        start,
                        drop: drop + above + lowered,
                        above,
                    });
                    drop += above + below;
                    left = right;
                    start = next;
                }
                offset += line.len() + 1;
                if offset > self.data.text.len() {
                    break;
                }
            }
        }
        let edged = layout.lines().iter().any(|line| line.lead(0).is_some());
        for inline in &self.data.boxes {
            let (id, range) = &self.data.ranges[inline.index];
            if inline.lead == 0. || range.start == range.end || edged {
                continue;
            }
            let clone = lead_styles
                .get_or_insert_with(|| {
                    view.store
                        .borrow()
                        .page_state(view.id)
                        .and_then(|dom| dom.styles.last().cloned())
                })
                .as_ref()
                .is_some_and(|styles| {
                    styles.values[*id].get(Prop::BOX_DECORATION_BREAK) == Some("clone")
                });
            for row in rows.iter_mut() {
                let wrapped =
                    row.start > 0 && self.data.text.as_bytes().get(row.start - 1) != Some(&b'\n');
                let inside = row.start > range.start && row.start < range.end && clone;
                if wrapped && (row.start == range.start || inside) {
                    row.shift += px(inline.lead);
                    row.right += px(inline.lead);
                }
            }
        }
        // Rows in later columns move there.
        for (at, &(first, by)) in moves.iter().enumerate() {
            let end = moves.get(at + 1).map_or(rows.len(), |next| next.0);
            for row in rows.get_mut(first..end).into_iter().flatten() {
                row.shift += by.x;
                row.right += by.x;
                row.drop += by.y;
            }
        }
        // The first line is at the bottom.
        if self.reversed {
            let line = layout.line_height();
            for (index, row) in rows.iter_mut().enumerate() {
                let below = extents.get(index).map_or(px(0.), |row| row[1]);
                row.drop = bounds.size.height - line * (2 * index + 1) as f32 - row.drop
                    + row.above
                    - below;
            }
        }
        let rows: crate::render::layout::Rows = rows.into();
        let base = window.text_system().baseline(
            strut,
            style.font_size,
            layout.line_height(),
            window.scale_factor(),
        );
        for (atom, slot) in self.data.atoms.iter().zip(self.atoms.iter()) {
            if let Some((a, ..)) = inline_box(
                layout,
                &self.data.text,
                &rows,
                &(atom.at..atom.at + '\u{fffc}'.len_utf8()),
                None,
                base,
                window,
            ) {
                let row = rows
                    .iter()
                    .rposition(|row| row.start <= atom.at)
                    .unwrap_or_else(|| {
                        ((a.y - layout.bounds().top()) / layout.line_height()).round() as usize
                    });
                let [above, below] = extents.get(row).copied().unwrap_or_default();
                let y = match slot.align {
                    Some(AlignItems::FlexStart) => a.y - above,
                    Some(AlignItems::FlexEnd) => {
                        a.y + layout.line_height() + below - px(slot.size[1])
                    }
                    _ => a.y + base - px(slot.size[2]),
                };
                slot.at.set(point(a.x, y));
            }
        }
        if !self.data.marks.is_empty() {
            self.place_marks(layout, &rows, &extents, &style, exact.origin);
        }
        let hitboxes = self
            .cursors(layout, &rows, bounds)
            .map(|(rect, cursor)| (window.add_region(rect, renderer::Hit::Pass), cursor))
            .collect();
        let shift = exact.origin - bounds.origin;
        let mut store = view.store.borrow_mut();
        let dom = store.state(view.id);
        let computed = dom.styles.last().cloned();
        let mut spans: Option<Vec<(usize, Pixels)>> = None;
        unplace(dom, &self.data);
        let (mut layouts, mut rects) = {
            let mut prepainted = self.prepainted.borrow_mut();
            (
                std::mem::take(&mut prepainted.layouts),
                std::mem::take(&mut prepainted.rects),
            )
        };
        layouts.clear();
        rects.clear();
        let mut boxes = self.data.boxes.iter().peekable();
        for (at, (id, range)) in self.data.ranges.iter().enumerate() {
            let boxed = boxes.next_if(|inline| inline.index == at);
            if let Some((mut a, mut b, mut top, mut height)) = inline_box(
                layout,
                &self.data.text,
                &rows,
                range,
                computed
                    .as_deref()
                    .map(|styles| (&*styles.values[*id], styles.lengths[*id])),
                base,
                window,
            ) {
                let font = [top, height];
                let mut whole: Option<[Pixels; 2]> = None;
                if let Some(inline) = boxed {
                    for (_, [left, right], _) in box_parts(layout, &rows, at) {
                        let [low, high] = whole.unwrap_or([left, right]);
                        whole = Some([low.min(left), high.max(right)]);
                    }
                    (a.x, b.x) = match whole {
                        Some([left, right]) => (left, right),
                        None if inline.trail.is_nan() => {
                            let left = a.x - px(inline.lead);
                            (left, left + px(inline.edges[1] + inline.edges[3]))
                        }
                        None => (a.x - px(inline.lead), b.x - px(inline.trail)),
                    };
                    top -= px(inline.edges[0]);
                    height += px(inline.edges[0] + inline.edges[2]);
                }
                // A positioned inline box contains the out-of-flow boxes in it: the padding box from the start of its
                // first part to the end of its last, as in Chrome.
                if let Some(styles) = computed.as_deref()
                    && super::super::effects::establishes(&styles.values[*id]).absolute
                {
                    let borders = boxed.map_or([0.; 4], |inline| inline.borders);
                    let start = point(a.x + px(borders[3]), a.y + top + px(borders[0]));
                    let end = point(b.x - px(borders[1]), b.y + top + height - px(borders[2]));
                    dom.layout.blocks.insert(
                        *id,
                        Rect::new(
                            start + shift,
                            renderer::size(
                                (end.x - start.x).max(px(0.)),
                                (end.y - start.y).max(px(0.)),
                            ),
                        ),
                    );
                }
                let (left, width) = if a.y == b.y || whole.is_some() {
                    (a.x, b.x - a.x)
                } else {
                    let spans = spans.get_or_insert_with(|| {
                        let mut spans = Vec::new();
                        let mut offset = 0;
                        for line in layout.lines().iter() {
                            let glyphs = &line.line.glyphs;
                            let (mut left, mut start) = (px(0.), offset);
                            for (row, boundary) in
                                line.breaks.iter().map(Some).chain([None]).enumerate()
                            {
                                let hyphen =
                                    line.hyphen(row).map_or(px(0.), |hyphen| hyphen.line.width);
                                let (x, end, next) = boundary.map_or(
                                    (line.line.width, line.line.width, offset + line.len()),
                                    |&at| {
                                        let glyph = &glyphs[at as usize];
                                        // Search back from the end of the glyph's face and size.
                                        let face = (glyph.font, glyph.size);
                                        let group = glyphs[at as usize..]
                                            .iter()
                                            .position(|other| (other.font, other.size) != face)
                                            .map_or(glyphs.len(), |len| at as usize + len);
                                        let hanging = glyphs[..group]
                                            .iter()
                                            .rev()
                                            .find(|other| other.index < glyph.index)
                                            .filter(|other| {
                                                self.data
                                                    .text
                                                    .as_bytes()
                                                    .get(offset + other.index as usize)
                                                    == Some(&b' ')
                                            });
                                        (
                                            glyph.x,
                                            hanging.map_or(glyph.x, |other| other.x),
                                            offset + glyph.index as usize,
                                        )
                                    },
                                );
                                spans.push((start, end + hyphen - left));
                                (left, start) = (x, next);
                            }
                            offset += line.len() + 1;
                        }
                        spans
                    });
                    let first = spans
                        .partition_point(|&(start, _)| start <= range.start)
                        .saturating_sub(1);
                    let last = spans
                        .partition_point(|&(start, _)| start < range.end)
                        .saturating_sub(1)
                        .max(first);
                    let row_shift = |row: usize| rows.get(row).map_or(px(0.), |row| row.shift);
                    let origin = layout.bounds().left();
                    let (mut low, mut high) = (a.x - origin, b.x - origin);
                    for (row, &(_, width)) in spans.iter().enumerate().take(last + 1).skip(first) {
                        if row > first {
                            low = low.min(row_shift(row));
                        }
                        if row < last {
                            high = high.max(row_shift(row) + width);
                        }
                    }
                    (origin + low, high - low)
                };
                let mut rect = [
                    (left + shift.x).into(),
                    (a.y + top + shift.y).into(),
                    width.into(),
                    (b.y - a.y + height).into(),
                ];
                // Text in more than one column takes the union of its rows, and pointer events in each.
                if !moves.is_empty() && a.y != b.y {
                    let parts: Box<[[f32; 4]]> = text_rects(
                        layout,
                        &self.data.text,
                        &rows,
                        [range.start, range.end],
                        font,
                    )
                    .into_iter()
                    .map(|[x, y, width, height]| {
                        [
                            x + f32::from(shift.x),
                            y + f32::from(shift.y),
                            width,
                            height,
                        ]
                    })
                    .collect();
                    if let Some(union) =
                        parts.iter().copied().reduce(|[x, y, w, h], [a, b, c, d]| {
                            let [left, top] = [x.min(a), y.min(b)];
                            [
                                left,
                                top,
                                (x + w).max(a + c) - left,
                                (y + h).max(b + d) - top,
                            ]
                        })
                    {
                        rect = union;
                    }
                    dom.geometry.fragments.insert(*id, parts);
                } else if !dom.geometry.fragments.is_empty() && !self.data.cuts.contains(id) {
                    dom.geometry.fragments.remove(id);
                }
                // Like Chrome, a part that starts with a line break also has an empty part on the line before.
                if self.data.cuts.contains(id) && self.data.text[range.clone()].starts_with('\n') {
                    let line = f32::from(layout.line_height());
                    let left = f32::from(layout.bounds().left() + shift.x);
                    let empty = [left, rect[1] - line, 0., rect[3]];
                    place(dom, &self.data, *id, empty);
                    rects.push((*id, empty));
                }
                place(dom, &self.data, *id, rect);
                rects.push((*id, rect));
                let entry = (
                    layout.clone(),
                    self.data.text.clone(),
                    range.clone(),
                    rows.clone(),
                    font,
                );
                dom.layout.text_layouts.insert(*id, entry.clone());
                layouts.push((*id, entry));
            }
        }
        std::mem::drop(store);
        std::mem::drop(shaped);
        *self.prepainted.borrow_mut() = Prepainted {
            hitboxes,
            rows,
            layouts,
            rects,
            origin: exact.origin,
            shaped_at: exact.origin,
            split: !moves.is_empty(),
        };
    }

    fn volatile(&self) -> bool {
        !self.atoms.is_empty()
            || !self.marks.is_empty()
            || self.data.first_line.is_some()
            || self.data.runs.iter().any(|run| run.font_size.is_some())
    }

    fn kept(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        let [top, right, bottom, left] = cx.inset;
        let bounds = Rect::new(
            cx.bounds.origin + point(left, top),
            size(
                cx.bounds.size.width - left - right,
                cx.bounds.size.height - top - bottom,
            ),
        );
        let origin = cx.exact.origin + point(left, top);
        let layout = cx.text_layout().cloned();
        let mut prepainted = self.prepainted.borrow_mut();
        let Prepainted {
            hitboxes,
            rows,
            layouts,
            rects,
            origin: at,
            shaped_at,
            ..
        } = &mut *prepainted;
        let moved = origin != *at;
        if let Some((shaped, _)) = &*self.shaped.borrow()
            && *shaped_at != origin
        {
            shaped.shift(origin - *shaped_at);
            *shaped_at = origin;
        }
        let window = &mut *cx.window;
        let places: Vec<_> = match &layout {
            Some(layout) if moved => self
                .cursors(layout, rows, bounds)
                .map(|(rect, _)| rect)
                .collect(),
            _ => hitboxes.iter().map(|(hitbox, _)| hitbox.bounds).collect(),
        };
        for ((hitbox, _), place) in hitboxes.iter_mut().zip(places) {
            *hitbox = window.move_region(hitbox, place);
        }
        let view = &*self.view;
        let mut store = view.store.borrow_mut();
        let dom = store.state(view.id);
        if moved {
            let [x, y] = [origin.x - at.x, origin.y - at.y].map(f32::from);
            *at = origin;
            unplace(dom, &self.data);
            for (id, rect) in rects.iter_mut() {
                rect[0] += x;
                rect[1] += y;
                place(dom, &self.data, *id, *rect);
                for part in dom.geometry.fragments.get_mut(id).into_iter().flatten() {
                    part[0] += x;
                    part[1] += y;
                }
            }
        }
        for (id, entry) in layouts.iter() {
            dom.layout.text_layouts.insert(*id, entry.clone());
        }
    }

    fn paint_content(&self, cx: &mut renderer::NodeCx<'_>) -> bool {
        let node_layout = cx.text_layout().cloned();
        let text_style = cx.text.clone();
        let window = &mut *cx.window;
        let shaped = self.shaped.borrow();
        let (layout, runs) = match (&*shaped, &node_layout) {
            (Some((layout, runs)), _) => (layout, &runs[..]),
            (None, Some(layout)) => (layout, &self.data.runs[..]),
            (None, None) => return true,
        };
        let prepainted = self.prepainted.borrow();
        let Prepainted {
            hitboxes,
            rows,
            split,
            ..
        } = &*prepainted;
        for (hitbox, cursor) in hitboxes.iter() {
            window.set_cursor(hitbox, *cursor);
        }
        let strut = window.text_system().face(&text_style.font());
        let mut highlights = Vec::new();
        let mut outlines = Vec::new();
        // Inline boxes with opacity whose text, background and borders draw as a group, in a layer after the rest of
        // the text, like the stacking context of such a box in Chrome. The outermost ones, with their text ranges.
        let mut groups: Vec<(f32, std::ops::Range<usize>)> = Vec::new();
        for inline in self.data.boxes.iter().filter(|inline| inline.group < 1.) {
            let range = self.data.ranges[inline.index].1.clone();
            if !groups
                .iter()
                .any(|(_, outer)| outer.start <= range.start && range.end <= outer.end)
            {
                groups.push((inline.group, range));
            }
        }
        let group_of = |range: &std::ops::Range<usize>| {
            groups
                .iter()
                .position(|(_, outer)| outer.start <= range.start && range.end <= outer.end)
        };
        // The backgrounds and borders of the boxes in each group.
        let mut grouped: Vec<Grouped> = Vec::new();
        {
            let view = &*self.view;
            let store = view.store.borrow();
            let dom = &store.pages[&view.id].state;
            if let Some(styles) = dom.styles.last() {
                let base = window.text_system().baseline(
                    strut,
                    text_style.font_size,
                    layout.line_height(),
                    window.scale_factor(),
                );
                // The border box of an inline box on each of its rows, with its left and right sides.
                let fragments = |index: usize,
                                 inline: Option<&crate::render::inline::InlineBox>,
                                 window: &Window| {
                    let (id, range) = &self.data.ranges[index];
                    let (style, units) = (&styles.values[*id], styles.lengths[*id]);
                    let (a, b, offset, height) = inline_box(
                        layout,
                        &self.data.text,
                        rows,
                        range,
                        Some((style, units)),
                        base,
                        window,
                    )?;
                    let (top, line_height) = (layout.bounds().top(), layout.line_height());
                    let drop = |row: usize| rows.get(row).map_or(px(0.), |row| row.drop);
                    let row_at = |y: Pixels| {
                        (0..rows.len().max(1))
                            .rev()
                            .find(|&row| top + line_height * row as f32 + drop(row) <= y + px(0.5))
                            .unwrap_or(0)
                    };
                    let [first, last] = [a.y, b.y].map(row_at);
                    let clone = style
                        .get(Prop::BOX_DECORATION_BREAK)
                        .is_some_and(|value| value == "clone");
                    let [above, right_edge, below, left_edge] =
                        inline.map_or([0.; 4], |inline| inline.edges).map(px);
                    let (lead, trail) =
                        inline.map_or((0., 0.), |inline| (inline.lead, inline.trail));
                    let placed = match inline {
                        Some(inline) => box_parts(layout, rows, inline.index),
                        None => Vec::new(),
                    };
                    let visual = match placed.is_empty() {
                        true => visual_parts(layout, rows, range, false),
                        false => None,
                    };
                    let count = match placed.len() {
                        0 => visual.as_ref().map_or(last + 1 - first, Vec::len),
                        count => count,
                    };
                    let [begins, ends] =
                        visual.iter().flatten().fold([usize::MAX, 0], |ends, part| {
                            [ends[0].min(part.2.first), ends[1].max(part.2.last)]
                        });
                    let parts = (0..count).map(move |at| {
                        if let Some(&(y, [left, right], sides)) = placed.get(at) {
                            let rect = Rect::new(
                                point(left, y + offset - above),
                                size(right - left, height + above + below),
                            );
                            return (rect, sides.map(|side| side || clone));
                        }
                        // The start of right-to-left text is at its right.
                        if let Some((_, y, part)) = visual.as_ref().map(|parts| &parts[at]) {
                            let [start, end] =
                                [part.first == begins || clone, part.last == ends || clone];
                            let (left, right, sides) = match part.odd {
                                true => (
                                    part.left - if end { left_edge } else { px(0.) },
                                    part.right,
                                    [end, start],
                                ),
                                false => (
                                    part.left - if start { px(lead) } else { px(0.) },
                                    part.right
                                        - if end && !trail.is_nan() {
                                            px(trail)
                                        } else {
                                            px(0.)
                                        },
                                    [start, end],
                                ),
                            };
                            let rect = Rect::new(
                                point(left, *y + offset - above),
                                size(right - left, height + above + below),
                            );
                            return (rect, sides);
                        }
                        let row = first + at;
                        let (start, end) = (row == first, row == last);
                        let extent = rows
                            .get(row)
                            .map_or((px(0.), layout.bounds().size.width), |row| {
                                (row.shift, row.right)
                            });
                        let x = if start {
                            a.x - px(lead)
                        } else {
                            layout.bounds().left() + extent.0
                                - if clone { left_edge } else { px(0.) }
                        };
                        let right = if !end {
                            layout.bounds().left()
                                + extent.1
                                + if clone { right_edge } else { px(0.) }
                        } else if trail.is_nan() {
                            x + left_edge + right_edge
                        } else {
                            b.x - px(trail)
                        };
                        let rect = Rect::new(
                            point(
                                x,
                                top + line_height * row as f32 + drop(row) + offset - above,
                            ),
                            size(right - x, height + above + below),
                        );
                        (rect, [start || clone, end || clone])
                    });
                    Some((style, units, b.x - a.x, parts))
                };
                let radii = |style: &css::properties::Style,
                             units: css::value::units::Lengths,
                             width: Pixels| {
                    [
                        Prop::BORDER_TOP_LEFT_RADIUS,
                        Prop::BORDER_TOP_RIGHT_RADIUS,
                        Prop::BORDER_BOTTOM_RIGHT_RADIUS,
                        Prop::BORDER_BOTTOM_LEFT_RADIUS,
                    ]
                    .map(|corner| {
                        px(style
                            .get(corner)
                            .and_then(|v| units.resolve(v, width.into()))
                            .unwrap_or(0.))
                    })
                };
                for inline in &self.data.boxes {
                    let shadowed = self.data.cuts.contains(&self.data.ranges[inline.index].0);
                    if inline.fill.is_transparent()
                        && (inline.border.is_transparent() || inline.borders == [0.; 4])
                        && !shadowed
                    {
                        continue;
                    }
                    let Some((style, units, width, parts)) =
                        fragments(inline.index, Some(inline), window)
                    else {
                        continue;
                    };
                    let parts: Vec<_> = parts
                        .map(|(rect, [left, right])| {
                            (rect, [left && !inline.open[0], right && !inline.open[1]])
                        })
                        .collect();
                    let radii = radii(style, units, width);
                    // A spliced box has no box of its own, so each part draws its shadow.
                    let mut shadows = match shadowed {
                        true => css::value::box_shadows(
                            &units,
                            style.get(Prop::BOX_SHADOW).unwrap_or("none"),
                            false,
                        ),
                        false => Vec::new(),
                    };
                    for shadow in &mut shadows {
                        shadow.color = shadow.color.fade(inline.fade);
                    }
                    // A part has no shadow at its open sides: it goes on past them, cut at the side.
                    for (rect, [left_side, right_side]) in
                        parts.iter().filter(|_| !shadows.is_empty())
                    {
                        let corner =
                            |round: bool, radius: Pixels| if round { radius } else { px(0.) };
                        let corners = renderer::Corners {
                            top_left: corner(*left_side, radii[0]),
                            top_right: corner(*right_side, radii[1]),
                            bottom_right: corner(*right_side, radii[2]),
                            bottom_left: corner(*left_side, radii[3]),
                        };
                        let far = px(1e4);
                        let [left, right] = [*left_side, *right_side]
                            .map(|closed| if closed { far } else { px(0.) });
                        let clip = Rect::new(
                            point(rect.left() - left, rect.top() - far),
                            size(rect.size.width + left + right, rect.size.height + far + far),
                        );
                        let [left, right] = [*left_side, *right_side]
                            .map(|closed| if closed { px(0.) } else { far });
                        let wide = Rect::new(
                            point(rect.left() - left, rect.top()),
                            size(rect.size.width + left + right, rect.size.height),
                        );
                        window.scope(
                            renderer::Scope::Clip(Some(renderer::Clip::rounded(
                                clip,
                                renderer::Corners::default(),
                            ))),
                            |window| window.draw_shadows(wide, corners.into(), &shadows, false),
                        );
                    }
                    let [width_above, width_right, width_below, width_left] =
                        inline.borders.map(px);
                    let group = group_of(&self.data.ranges[inline.index].1);
                    for (rect, [left_side, right_side]) in parts {
                        let corners = renderer::Corners {
                            top_left: if left_side { radii[0] } else { px(0.) },
                            top_right: if right_side { radii[1] } else { px(0.) },
                            bottom_right: if right_side { radii[2] } else { px(0.) },
                            bottom_left: if left_side { radii[3] } else { px(0.) },
                        };
                        let border = renderer::Border {
                            widths: renderer::Edges {
                                top: width_above,
                                right: if right_side { width_right } else { px(0.) },
                                bottom: width_below,
                                left: if left_side { width_left } else { px(0.) },
                            },
                            color: inline.border,
                            style: renderer::BorderStyle::Solid,
                            sides: 0,
                        };
                        match group {
                            Some(group) => {
                                grouped.push((group, rect, corners, inline.fill, border))
                            }
                            None => window.draw_box(rect, corners, inline.fill, border),
                        }
                    }
                }
                // Like Chrome, each row of an outlined inline box draws its own outline, above the text.
                for &index in &self.data.outlines {
                    let inline = (self.data.boxes)
                        .binary_search_by_key(&index, |inline| inline.index)
                        .ok()
                        .map(|at| &self.data.boxes[at]);
                    let Some((style, units, width, parts)) = fragments(index, inline, window)
                    else {
                        continue;
                    };
                    let Some(kind) = style
                        .get(Prop::OUTLINE_STYLE)
                        .filter(|kind| !matches!(*kind, "none" | "hidden"))
                    else {
                        continue;
                    };
                    let color = [Prop::OUTLINE_COLOR, Prop::COLOR]
                        .into_iter()
                        .find_map(|prop| style.get(prop).and_then(css::value::color::paint))
                        .unwrap_or(Color::BLACK);
                    let offset = style
                        .get(Prop::OUTLINE_OFFSET)
                        .and_then(|v| units.resolve(v, 0.))
                        .unwrap_or(0.);
                    let radii = radii(style, units, width);
                    // A focus ring is 2px of the outline color over a 1px ring of white, or of #101010 in a dark
                    // scheme, and has a radius of 3px without a border radius.
                    // Rings as (grow, width, color, style), and how radii grow: (by, then, without a radius).
                    let (rings, (by, then, square)) = if kind == "auto" {
                        let at = offset.trunc()
                            - if inline
                                .is_some_and(|inline| inline.borders.iter().all(|w| *w >= 1.))
                            {
                                1.
                            } else {
                                0.
                            };
                        let under =
                            Color::hex(if style::dark(style, crate::native::theme_state()) {
                                0x101010ff
                            } else {
                                0xffffffff
                            });
                        let rings =
                            vec![(at + 2., 2., under, "solid"), (at + 1., 2., color, "solid")];
                        (rings, (at, 1., 4.))
                    } else {
                        let Some(line) = units
                            .resolve(style.get(Prop::OUTLINE_WIDTH).unwrap_or("3px"), 0.)
                            .filter(|line| *line > 0.)
                        else {
                            continue;
                        };
                        (
                            vec![(offset + line, line, color, kind)],
                            (offset + line, 0., 0.),
                        )
                    };
                    for (rect, [left_side, right_side]) in parts {
                        for &(grow, line, color, kind) in &rings {
                            let rect = Rect::new(
                                rect.origin - point(px(grow), px(grow)),
                                rect.size + size(px(grow * 2.), px(grow * 2.)),
                            );
                            if rect.size.width <= px(0.) || rect.size.height <= px(0.) {
                                continue;
                            }
                            let corner = |side: bool, radius: Pixels| match side && radius > px(0.)
                            {
                                true => (radius + px(by)).max(px(0.)) + px(then),
                                false => px(square),
                            };
                            outlines.push((
                                rect,
                                renderer::Corners {
                                    top_left: corner(left_side, radii[0]),
                                    top_right: corner(right_side, radii[1]),
                                    bottom_right: corner(right_side, radii[2]),
                                    bottom_left: corner(left_side, radii[3]),
                                },
                                renderer::Border {
                                    widths: renderer::Edges::all(px(line)),
                                    color,
                                    style: super::super::effects::border_style(kind),
                                    sides: 0,
                                },
                            ));
                        }
                    }
                }
                let doc = &store.pages[&view.id].doc;
                // Like Chrome, a highlight leaves out the ellipsis of truncated text.
                let mut shown = 0;
                if !dom.interaction.highlights.is_empty() {
                    for line in layout.lines().iter() {
                        let rest = self.data.text.get(shown..).unwrap_or("");
                        let same = line
                            .text
                            .bytes()
                            .zip(rest.bytes())
                            .take_while(|(a, b)| a == b);
                        let same = same.count();
                        shown += same;
                        if same < line.text.len() {
                            break;
                        }
                        shown += 1;
                    }
                }
                for (name, highlight) in &dom.interaction.highlights {
                    let Some(highlight) = dom.interaction.highlighted.get(highlight) else {
                        continue;
                    };
                    for (id, range) in &self.data.ranges {
                        let Some(parts) = highlight.nodes.get(id) else {
                            continue;
                        };
                        // Highlights change without a new box, like the selection.
                        window.paint_volatile();
                        let element = doc.nodes[*id].parent.unwrap_or(*id);
                        let kept = &mut dom.styles.highlights.borrow_mut();
                        let cache = &dom.styles.cache;
                        let Some(style) = cache.highlight(styles, kept, element, name) else {
                            continue;
                        };
                        let own = &styles.values[*id];
                        let mut look = (**own).clone();
                        look.remove(Prop::TEXT_DECORATION_LINE);
                        look.extend(style.own().map(|(prop, value)| (prop, value.clone())));
                        if !style.contains(Prop::WEBKIT_TEXT_FILL_COLOR)
                            && let Some(value) = style.value(Prop::COLOR)
                        {
                            look.insert(Prop::WEBKIT_TEXT_FILL_COLOR, value.clone());
                        }
                        for prop in [Prop::COLOR, Prop::WEBKIT_TEXT_FILL_COLOR] {
                            if look
                                .get(prop)
                                .is_some_and(|value| value.eq_ignore_ascii_case("currentcolor"))
                                && let Some(value) = own.value(Prop::COLOR)
                            {
                                look.insert(prop, value.clone());
                            }
                        }
                        let run = style::text_run(&look, &styles.lengths[*id], 0);
                        let colored = style.get(Prop::COLOR).is_some()
                            || style.get(Prop::WEBKIT_TEXT_FILL_COLOR).is_some();
                        let background = match style.get(Prop::BACKGROUND_COLOR) {
                            Some(value) if value.eq_ignore_ascii_case("currentcolor") => run.color,
                            value => value
                                .and_then(css::value::color::paint)
                                .unwrap_or(Color::TRANSPARENT),
                        };
                        let text = &self.data.text[range.clone()];
                        let source = doc.nodes[*id].text.as_str();
                        for (_, part) in parts {
                            let [start, end] = part.map(|at| {
                                range.start + crate::dom::text::shown_index(source, text, at)
                            });
                            let end = end.min(shown);
                            if start < end {
                                highlights.push(TextHighlight {
                                    range: start..end,
                                    color: colored.then_some(run.color),
                                    background,
                                    stroke: true,
                                    tight: true,
                                    lines: [run.underline, run.strikethrough, run.overline],
                                });
                            }
                        }
                    }
                }
                for (id, range) in &self.data.ranges {
                    let Some([start, end]) = dom.interaction.selection.get(id) else {
                        continue;
                    };
                    // A selection changes without a new box, so this text draws again in each frame.
                    window.paint_volatile();
                    if styles.user_select[*id].0 == style::UserSelect::None || dom.styles.inert(*id)
                    {
                        continue;
                    }
                    let caret = dom.interaction.selection_caret == Some(*id);
                    let text = &self.data.text[range.clone()];
                    let source = store
                        .pages
                        .get(&view.id)
                        .map_or("", |page| page.doc.nodes[*id].text.as_str());
                    let start = range.start + crate::dom::text::shown_index(source, text, *start);
                    let end = range.start + crate::dom::text::shown_index(source, text, *end);
                    if caret {
                        let look = CaretLook::new(&styles.values[*id]);
                        if let Some(position) = text_position(layout, &self.data.text, rows, start)
                        {
                            paint_caret(
                                view,
                                [*id, start, range.len()],
                                position,
                                layout.line_height(),
                                look.advance(layout, &self.data.text, rows, start, position),
                                (
                                    window.text_system().face(&style::font(&styles.values[*id])),
                                    px(styles.lengths[*id].font.em),
                                    base,
                                ),
                                look,
                                window,
                            );
                        }
                    } else if start != end {
                        let (color, background) = style::selection_colors(
                            &styles.values[*id],
                            styles.extras(*id).selection.as_ref(),
                        );
                        highlights.push(TextHighlight {
                            range: start.min(end)..start.max(end),
                            color: color.map(Color::hex),
                            background: Color::hex(background),
                            stroke: styles.extras(*id).selection.is_none(),
                            tight: false,
                            lines: Default::default(),
                        });
                    }
                }
            }
        }
        // Runs whose text is in a group, or out of it, draw clear.
        let clear = |runs: &[renderer::TextRun], keep: &dyn Fn(usize) -> bool| {
            let mut at = 0;
            runs.iter()
                .map(|run| {
                    let mut run = run.clone();
                    if !keep(at) {
                        run.color = Color::TRANSPARENT;
                        run.background_color = None;
                        run.stroke = None;
                        run.emphasis = None;
                        for line in [
                            &mut run.underline,
                            &mut run.strikethrough,
                            &mut run.overline,
                        ] {
                            if let Some(line) = line.as_mut() {
                                line.color = Some(Color::TRANSPARENT);
                            }
                        }
                    }
                    at += run.len;
                    run
                })
                .collect::<Vec<_>>()
        };
        let inside = |at: usize| {
            groups
                .iter()
                .any(|(_, range)| range.start <= at && at < range.end)
        };
        let main = (!groups.is_empty()).then(|| clear(runs, &|at| !inside(at)));
        let main = main.as_deref().unwrap_or(runs);
        let lines = |window: &mut Window,
                     runs: &[renderer::TextRun],
                     shift: Point<Pixels>,
                     ink: Option<Color>,
                     highlights: &[TextHighlight]| {
            let mut offset = 0;
            let mut row = 0;
            let mut origin = layout.bounds().origin + shift;
            let mask = window.content_mask().bounds;
            for line in layout.lines().iter() {
                let count = line.rows();
                let height = line.extent(layout.line_height()).height;
                if origin.y > mask.bottom() && !split {
                    window.cull();
                    break;
                }
                // Taller rows above push this line down: the next row's drop says how far.
                let dropped = match rows.get(row + count) {
                    Some(next) => next.drop - next.above,
                    None if rows.is_empty() => px(0.),
                    None => px(f32::INFINITY),
                };
                if *split || origin.y + height + dropped + layout.line_height() > mask.top() {
                    paint_text(
                        line,
                        origin,
                        layout.line_height(),
                        runs,
                        offset,
                        highlights,
                        rows.get(row..row + count).unwrap_or_default(),
                        &self.data.text,
                        ink,
                        strut,
                        window,
                    );
                } else {
                    window.cull();
                }
                row += count;
                offset += line.len() + 1;
                origin.y += height;
                if offset > self.data.text.len() {
                    break;
                }
            }
        };
        let passes = self
            .data
            .shadows
            .iter()
            .map(|&(x, y, blur, color)| (point(px(x), px(y)), Some(color), blur))
            .chain([(point(px(0.), px(0.)), None, 0.)]);
        for (shift, ink, blur) in passes {
            let highlights = if ink.is_some() {
                &[][..]
            } else {
                &highlights[..]
            };
            let paint = |window: &mut Window| lines(window, main, shift, ink, highlights);
            if blur > 0. {
                let effect = renderer::PaintEffect {
                    blur: px(blur / 2.),
                    ..Default::default()
                };
                window.with_effect(effect, paint);
            } else if ink.is_some() {
                window.without_text_clip(paint);
            } else {
                paint(window);
            }
        }
        for (group, (opacity, range)) in groups.iter().enumerate() {
            let runs = clear(runs, &|at| range.start <= at && at < range.end);
            let effect = renderer::PaintEffect {
                opacity: *opacity,
                ..Default::default()
            };
            window.with_effect(effect, |window| {
                for (_, rect, corners, fill, border) in
                    grouped.iter().filter(|draw| draw.0 == group)
                {
                    window.draw_box(*rect, *corners, *fill, *border);
                }
                lines(window, &runs, point(px(0.), px(0.)), None, &highlights);
            });
        }
        for (rect, corners, border) in outlines {
            window.draw_box(rect, corners, Color::TRANSPARENT, border);
        }
        true
    }
}
