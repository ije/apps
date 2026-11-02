use crate::style;
use crate::style::Prop;
use anyhow::Result;
use renderer::{Bounds, Pixels, Point, SharedString, Window, point, prelude::*, px, rgba, size};
use std::{ops::Range, rc::Rc};

pub(super) struct TextHighlight {
    pub(super) range: Range<usize>,
    pub(super) color: Option<renderer::Rgba>,
    pub(super) background: renderer::Rgba,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_text(
    line: &renderer::LineLayout,
    wraps: &[renderer::WrapBoundary],
    origin: Point<Pixels>,
    line_height: Pixels,
    runs: &[renderer::TextRun],
    offset: usize,
    highlights: &[TextHighlight],
    rows: &[crate::dom::Row],
    text: &str,
    ink: Option<renderer::Rgba>,
    strut: renderer::FontId,
    window: &mut Window,
) {
    let smoothing = window.text_style().font_smoothing;
    let shift = |row: usize| rows.get(row).map_or(px(0.), |row| row.shift);
    // Justified rows widen each space by the row gap.
    let justify = |row: usize, index: usize| {
        rows.get(row)
            .filter(|row| row.gap > px(0.))
            .map_or(px(0.), |row| {
                row.gap * spaces(text.get(row.start.min(index)..index).unwrap_or(""))
            })
    };
    window.paint_layer(
        Bounds::new(
            origin,
            size(
                rows.iter().fold(line.width, |max, row| {
                    max.max(line.width + row.shift).max(row.right)
                }),
                line_height * (wraps.len() + 1),
            ),
        ),
        |window| {
            // The strut sets the baseline, so fallback fonts (emoji) and larger first-letter or
            // first-line runs don't move it. Taller runs move their row down with `drop`.
            let baseline = window.text_system().baseline(
                strut,
                line.font_size,
                line_height,
                window.scale_factor(),
            );
            let drop = |row: usize| rows.get(row).map_or(px(0.), |row| row.drop);
            let top = |row: usize| origin.y + line_height * row + drop(row);
            let mask = window.content_mask().bounds;
            let tall = line.runs.iter().fold(line_height, |tall, run| {
                tall.max(
                    window
                        .text_system()
                        .bounding_box(run.font_id, run.font_size)
                        .size
                        .height,
                )
            });
            let shown = |row: usize| top(row) < mask.bottom() && top(row) + tall > mask.top();
            let mut start = 0;
            let mut left = px(0.);
            let mut first_run = (0, 0);
            for (row, boundary) in wraps.iter().map(Some).chain([None]).enumerate() {
                let (end, right) = boundary.map_or((line.len, line.width), |boundary| {
                    let glyph = &line.runs[boundary.run_ix].glyphs[boundary.glyph_ix];
                    (glyph.index, glyph.position.x)
                });
                if !shown(row) {
                    start = end;
                    left = right;
                    continue;
                }
                let row_left = origin.x + shift(row) - left;
                let top = top(row);
                while first_run.0 < runs.len()
                    && first_run.1 + runs[first_run.0].len <= offset + start
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
                    if a >= b
                        || run.underline.is_none()
                            && run.strikethrough.is_none()
                            && run.background_color.is_none()
                    {
                        continue;
                    }
                    let x = line.x_for_index(a - offset) + justify(row, a);
                    let end_x = if b == offset + end {
                        right + justify(row, b)
                    } else {
                        line.x_for_index(b - offset) + justify(row, b)
                    };
                    if let Some(background) = run.background_color.filter(|_| ink.is_none()) {
                        window.paint_quad(renderer::fill(
                            Bounds::new(
                                point(row_left + x, top + baseline - line.ascent),
                                size(end_x - x, line.ascent + line.descent),
                            ),
                            background,
                        ));
                    }
                    if let Some(underline) = &run.underline {
                        window.paint_underline(
                            point(
                                row_left + x,
                                top + baseline + underline.offset.unwrap_or(line.descent * 0.618),
                            ),
                            end_x - x,
                            &renderer::UnderlineStyle {
                                color: ink.or(underline.color),
                                ..*underline
                            },
                        );
                    }
                    if let Some(strikethrough) = &run.strikethrough {
                        window.paint_strikethrough(
                            point(row_left + x, top + (line.ascent * 0.5 + baseline) * 0.5),
                            end_x - x,
                            &renderer::StrikethroughStyle {
                                color: ink.or(strikethrough.color),
                                ..*strikethrough
                            },
                        );
                    }
                }
                for highlight in highlights {
                    let a = highlight.range.start.max(offset + start);
                    let b = highlight.range.end.min(offset + end);
                    if a >= b {
                        continue;
                    }
                    let x = line.x_for_index(a - offset) + justify(row, a);
                    let end_x = if b == offset + end {
                        right + justify(row, b)
                    } else {
                        line.x_for_index(b - offset) + justify(row, b)
                    };
                    if end_x > x && highlight.background.a > 0. {
                        window.paint_quad(renderer::fill(
                            Bounds::new(point(row_left + x, top), size(end_x - x, line_height)),
                            highlight.background,
                        ));
                    }
                }
                start = end;
                left = right;
            }
            // WARNING: rows only grow downward, so painting starts at the first row that reaches the content mask.
            let Some(mut wrap) = (0..=wraps.len()).find(|&row| top(row) + tall > mask.top()) else {
                return;
            };
            let (mut left, from) = match wrap.checked_sub(1).map(|row| wraps[row]) {
                Some(at) => (
                    line.runs[at.run_ix].glyphs[at.glyph_ix].position.x,
                    (at.run_ix, at.glyph_ix),
                ),
                None => (px(0.), (0, 0)),
            };
            let mut run_index = 0;
            let mut run_end = runs.first().map_or(0, |run| run.len);
            let mut spaced = (usize::MAX, 0, 0.);
            'rows: for (run_ix, run) in line.runs.iter().enumerate().skip(from.0) {
                let glyph_size = window
                    .text_system()
                    .bounding_box(run.font_id, run.font_size)
                    .size;
                let skip = if run_ix == from.0 { from.1 } else { 0 };
                for (glyph_ix, glyph) in run.glyphs.iter().enumerate().skip(skip) {
                    if wraps.get(wrap) == Some(&renderer::WrapBoundary { run_ix, glyph_ix }) {
                        wrap += 1;
                        left = glyph.position.x;
                    }
                    if top(wrap) >= mask.bottom() {
                        break 'rows;
                    }
                    let index = offset + glyph.index;
                    while index >= run_end && run_index + 1 < runs.len() {
                        run_index += 1;
                        run_end += runs[run_index].len;
                    }
                    let color = ink.unwrap_or_else(|| {
                        highlights
                            .iter()
                            .find(|highlight| highlight.range.contains(&index))
                            .and_then(|highlight| highlight.color)
                            .unwrap_or(runs[run_index].color)
                    });
                    if color.a == 0. {
                        continue;
                    }
                    let gap = rows
                        .get(wrap)
                        .filter(|row| row.gap > px(0.))
                        .map_or(px(0.), |row| {
                            if spaced.0 != wrap || index < spaced.1 {
                                spaced = (wrap, row.start.min(index), 0.);
                            }
                            spaced.2 += spaces(text.get(spaced.1..index).unwrap_or(""));
                            spaced.1 = index;
                            row.gap * spaced.2
                        });
                    let position = point(
                        origin.x + shift(wrap) + glyph.position.x - left + gap,
                        origin.y + line_height * wrap + baseline + drop(wrap),
                    );
                    if !Bounds::new(point(position.x, position.y - baseline), glyph_size)
                        .intersects(&window.content_mask().bounds)
                    {
                        continue;
                    }
                    if glyph.is_emoji {
                        window.paint_emoji(position, run.font_id, glyph.id, run.font_size);
                    } else {
                        window.paint_glyph(
                            position,
                            run.font_id,
                            glyph.id,
                            run.font_size,
                            color,
                            smoothing,
                        );
                    }
                }
            }
        },
    );
}

pub(super) fn spaces(text: &str) -> f32 {
    text.bytes().filter(|byte| *byte == b' ').count() as f32
}

/// How much space each row needs above and below the paragraph's strut, like Chrome's line boxes: every
/// run's font sits centered in its own line height, in device pixels. Like browsers, fallback fonts (emoji)
/// count only with `line-height: normal`; other heights center the run's own font.
fn row_extents<'a>(
    lines: impl IntoIterator<Item = &'a renderer::WrappedLineLayout>,
    runs: &[renderer::TextRun],
    heights: &[(usize, style::LineHeight)],
    strut: (renderer::FontId, Pixels, Pixels),
    system: &renderer::TextSystem,
    scale: f32,
) -> Vec<[Pixels; 2]> {
    let inline = |font, size: Pixels, line: Option<f32>| {
        let [ascent, descent, gap] = system.line_metrics(font, size * scale).map(f32::from);
        let line = line.unwrap_or(ascent + descent + gap);
        let above = ascent + ((line - ascent - descent) / 2.).floor();
        [above, line - above]
    };
    let root = inline(strut.0, strut.1, Some(f32::from(strut.2) * scale));
    let run_ends: Vec<usize> = runs
        .iter()
        .scan(0, |end, run| {
            *end += run.len;
            Some(*end)
        })
        .collect();
    let mut rows = Vec::new();
    let mut offset = 0;
    for line in lines {
        let layout = &line.unwrapped_layout;
        let ends: Vec<usize> = line
            .wrap_boundaries
            .iter()
            .map(|boundary| layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index)
            .collect();
        let first = rows.len();
        rows.resize(first + ends.len() + 1, root);
        for run in &layout.runs {
            let mut last = None;
            for glyph in &run.glyphs {
                let row = first + ends.partition_point(|&end| end <= glyph.index);
                let at = offset + glyph.index;
                let kind = heights
                    .get(heights.partition_point(|(end, _)| *end <= at))
                    .map_or(style::LineHeight::Normal, |(_, kind)| *kind);
                let source = (kind != style::LineHeight::Normal)
                    .then(|| run_ends.partition_point(|&end| end <= at));
                if last == Some((row, kind, source)) {
                    continue;
                }
                last = Some((row, kind, source));
                let size = f32::from(run.font_size);
                let [above, below] = inline(
                    source
                        .and_then(|index| runs.get(index))
                        .map_or(run.font_id, |styled| system.resolve_font(&styled.font)),
                    run.font_size,
                    match kind {
                        style::LineHeight::Normal => None,
                        style::LineHeight::Scale(factor) => {
                            Some((factor * size * scale * 64.).floor() / 64.)
                        }
                        style::LineHeight::Fixed(line) => Some((line * scale * 64.).round() / 64.),
                    },
                );
                rows[row] = [rows[row][0].max(above), rows[row][1].max(below)];
            }
        }
        offset += line.len() + 1;
    }
    rows.into_iter()
        .map(|[above, below]| {
            [above - root[0], below - root[1]].map(|extra| px(extra.max(0.) / scale))
        })
        .collect()
}

pub(crate) fn text_position(
    layout: &renderer::TextLayout,
    text: &str,
    rows: &[crate::dom::Row],
    index: usize,
) -> Option<Point<Pixels>> {
    let mut position = layout.position_for_index(index)?;
    let row = ((position.y - layout.bounds().top()) / layout.line_height()).round();
    if let Some(row) = rows.get(row as usize) {
        position.x +=
            row.shift + row.gap * spaces(text.get(row.start.min(index)..index).unwrap_or(""));
        position.y += row.drop;
    }
    Some(position)
}

/// Where an inline element's text starts and ends, and the top and height of its content area in a row:
/// its font's ascent and descent around the paragraph's baseline, like Chrome. A range that starts at a
/// wrap starts on the next row.
fn inline_box(
    layout: &renderer::TextLayout,
    text: &str,
    rows: &[crate::dom::Row],
    range: &std::ops::Range<usize>,
    style: Option<(&style::Style, style::Lengths)>,
    window: &Window,
) -> Option<(Point<Pixels>, Point<Pixels>, Pixels, Pixels)> {
    let (mut a, b) = text_position(layout, text, rows, range.start)
        .zip(text_position(layout, text, rows, range.end))?;
    if let Some(next) = text
        .get(range.clone())
        .and_then(|text| text.chars().next())
        .and_then(|first| text_position(layout, text, rows, range.start + first.len_utf8()))
        .filter(|next| next.y > a.y)
    {
        let row = ((next.y - layout.bounds().top()) / layout.line_height()).round();
        a = point(
            layout.bounds().left() + rows.get(row as usize).map_or(px(0.), |row| row.shift),
            next.y,
        );
    }
    let line_height = layout.line_height();
    let Some((style, units)) = style else {
        return Some((a, b, px(0.), line_height));
    };
    let system = window.text_system();
    let scale = window.scale_factor();
    let root = window.text_style();
    let base = system.baseline(
        system.resolve_font(&root.font()),
        root.font_size,
        line_height,
        scale,
    );
    let [ascent, descent, _] = system
        .line_metrics(
            system.resolve_font(&style::font(style)),
            px(units.font.em * scale),
        )
        .map(|value| value / scale);
    Some((a, b, base - ascent, ascent + descent))
}

pub(crate) fn text_index(
    layout: &renderer::TextLayout,
    text: &str,
    rows: &[crate::dom::Row],
    mut position: Point<Pixels>,
) -> Result<usize, usize> {
    let (top, line_height) = (layout.bounds().top(), layout.line_height());
    // Rows move down by their drop, so the row is the last one that starts above the point.
    let row = rows
        .iter()
        .enumerate()
        .rposition(|(index, row)| {
            top + line_height * index as f32 + row.drop - row.above <= position.y
        })
        .unwrap_or(0);
    let Some(placed) = rows.get(row) else {
        return layout.index_for_position(position);
    };
    position.y = (position.y - placed.drop)
        .max(top + line_height * row as f32)
        .min(top + line_height * (row as f32 + 1.) - px(0.01));
    if placed.gap > px(0.) {
        // Justified rows: pick the closest character boundary of the row.
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
    layout.index_for_position(position)
}

fn first_line(
    data: &style::InlineText,
    width: Option<Pixels>,
    font_size: Pixels,
    text_system: &renderer::TextSystem,
) -> (SharedString, Vec<renderer::TextRun>) {
    let mut text = data.text.to_string();
    let Some((base, pseudo, mode)) = &data.first_line else {
        return (text.into(), data.runs.clone());
    };
    let len = text.find('\n').unwrap_or(text.len());
    let mut first = text[..len].to_owned();
    if let Some(mode) = mode {
        style::text_transform(&mut first, 0..len, mode);
    }
    let end = text_system
        .shape_text(
            first.into(),
            font_size,
            &style::restyle(&data.runs, 0..len, base, pseudo),
            width,
            Some(2),
            renderer::WordBreak::Normal,
        )
        .ok()
        .and_then(|lines| {
            let line = lines.first()?;
            let boundary = line.wrap_boundaries.first()?;
            Some(line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index)
        })
        .unwrap_or(len);
    if let Some(mode) = mode {
        style::text_transform(&mut text, 0..end, mode);
    }
    (
        text.into(),
        style::restyle(&data.runs, 0..end, base, pseudo),
    )
}

pub(super) struct InlineText {
    pub(super) inner: renderer::StyledText,
    pub(super) data: Rc<style::InlineText>,
    pub(super) view: Rc<super::PageState>,
    pub(super) runs: Option<Vec<renderer::TextRun>>,
    /// For justified text, whether the last row is justified too.
    pub(super) justify: Option<bool>,
    /// Set when the text is measured here instead of by `inner`: the last measured wrap
    /// width and text width.
    pub(super) measured: Option<Rc<std::cell::Cell<[Pixels; 2]>>>,
}

/// What a measured paragraph depends on, with its last measurement. Equal inputs keep it.
struct Measured {
    data: Rc<style::InlineText>,
    style: renderer::TextStyle,
    /// The last measured wrap width and text width.
    measured: Rc<std::cell::Cell<[Pixels; 2]>>,
    cached: std::cell::Cell<Option<(Option<Pixels>, renderer::Size<Pixels>)>>,
    /// The first and last baselines of the cached layout.
    baselines: std::cell::Cell<[Pixels; 2]>,
}

impl PartialEq for Measured {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.data, &other.data) && self.style == other.style
    }
}

impl IntoElement for InlineText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl renderer::Element for InlineText {
    type RequestLayoutState = renderer::LayoutId;
    type PrepaintState = (
        Vec<(renderer::Hitbox, renderer::CursorStyle)>,
        crate::dom::Rows,
    );
    fn request_layout(&mut self, window: &mut Window) -> (renderer::LayoutId, renderer::LayoutId) {
        let sized = self.data.runs.iter().any(|run| run.font_size.is_some())
            || self
                .data
                .first_line
                .as_ref()
                .is_some_and(|(_, pseudo, _)| pseudo.font_size.is_some());
        if self.data.first_line.is_none() && !sized {
            self.measured = None;
            let (id, ()) = self.inner.request_layout(window);
            return (id, id);
        }
        let (layout, kept) = window.request_measured_layout(
            &renderer::Style::default(),
            Measured {
                data: self.data.clone(),
                style: window.text_style(),
                measured: Rc::default(),
                cached: std::cell::Cell::new(None),
                baselines: std::cell::Cell::new([px(0.); 2]),
            },
            |kept, known, available, window| {
                let Measured {
                    data,
                    style,
                    measured,
                    cached,
                    baselines: kept_baselines,
                } = kept;
                let (font_size, line_height) = (style.font_size, style.line_height);
                let width = known
                    .width
                    .or(match available.width {
                        renderer::AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    })
                    .filter(|_| style.white_space == renderer::WhiteSpace::Normal);
                if let Some((_, size)) = cached
                    .get()
                    .filter(|(cached, _): &(_, renderer::Size<Pixels>)| *cached == width)
                {
                    measured.set([width.unwrap_or_default(), size.width]);
                    return (size, Some(kept_baselines.get()));
                }
                let system = window.text_system().clone();
                let (text, runs) = first_line(data, width, font_size, &system);
                let mut size = renderer::Size::<Pixels>::default();
                let lines = system
                    .shape_text_indented(
                        text,
                        font_size,
                        &runs,
                        width,
                        style.line_clamp,
                        style.text_indent,
                        style.text_wrap,
                        style.word_break,
                    )
                    .unwrap_or_default();
                for line in &lines {
                    let line = line.size(line_height);
                    size.height += line.height;
                    size.width = size.width.max(line.width);
                }
                let strut = (system.resolve_font(&style.font()), font_size, line_height);
                let scale = window.scale_factor();
                let extents = row_extents(
                    lines.iter().map(|line| &**line),
                    &runs,
                    &data.line_heights,
                    strut,
                    &system,
                    scale,
                );
                for [above, below] in &extents {
                    size.height += *above + *below;
                }
                let base = system.baseline(strut.0, font_size, line_height, scale);
                let baselines = [
                    extents.first().map_or(px(0.), |row| row[0]) + base,
                    size.height - extents.last().map_or(px(0.), |row| row[1]) - line_height + base,
                ];
                cached.set(Some((width, size)));
                kept_baselines.set(baselines);
                measured.set([width.unwrap_or_default(), size.width]);
                (size, Some(baselines))
            },
        );
        self.measured = Some(kept.measured.clone());
        (layout, layout)
    }
    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        id: &mut renderer::LayoutId,
        window: &mut Window,
    ) -> Self::PrepaintState {
        let style = window.text_style();
        if let Some(measured) = &self.measured {
            // Bounds snap to device pixels and can be a little narrower than the measured
            // wrap or text width, which must not wrap or truncate again.
            let wrap = measured
                .get()
                .into_iter()
                .filter(|width| *width >= bounds.size.width && *width - bounds.size.width < px(1.))
                .fold(bounds.size.width, Pixels::max);
            let width = (style.white_space == renderer::WhiteSpace::Normal).then_some(wrap);
            let font_size = style.font_size;
            let system = window.text_system().clone();
            let (text, runs) = first_line(&self.data, width, font_size, &system);
            self.inner = renderer::StyledText::new(text).with_runs(runs.clone());
            let (layout, _) = self.inner.request_layout(window);
            window.compute_layout(
                layout,
                size(
                    renderer::AvailableSpace::Definite(wrap),
                    renderer::AvailableSpace::MinContent,
                ),
            );
            self.runs = Some(runs);
        }
        self.inner.prepaint(bounds, &mut (), window);
        let layout = self.inner.layout();
        let mut rows = Vec::new();
        let indent = style.text_indent;
        let extents = row_extents(
            layout.line_layouts().iter().map(|line| &**line),
            self.runs.as_deref().unwrap_or(&self.data.runs),
            &self.data.line_heights,
            (
                window.text_system().resolve_font(&style.font()),
                style.font_size,
                layout.line_height(),
            ),
            window.text_system(),
            window.scale_factor(),
        );
        let mut drop = px(0.);
        if style.text_align != renderer::TextAlign::Left
            || !self.data.backgrounds.is_empty()
            || indent != px(0.)
            || self.justify.is_some()
            || extents.iter().flatten().any(|extra| *extra > px(0.))
        {
            let mut offset = 0;
            for line in layout.line_layouts() {
                let glyphs = &line.unwrapped_layout.runs;
                let mut left = px(0.);
                let mut start = offset;
                for boundary in line.wrap_boundaries.iter().map(Some).chain([None]) {
                    let (right, end) = boundary.map_or(
                        (line.unwrapped_layout.width, line.unwrapped_layout.width),
                        |boundary| {
                            let x = glyphs[boundary.run_ix].glyphs[boundary.glyph_ix].position.x;
                            let previous = boundary
                                .glyph_ix
                                .checked_sub(1)
                                .map(|glyph| &glyphs[boundary.run_ix].glyphs[glyph])
                                .or_else(|| {
                                    let run = boundary.run_ix.checked_sub(1)?;
                                    glyphs[run].glyphs.last()
                                });
                            let hanging = previous.filter(|glyph| {
                                self.data.text.as_bytes().get(offset + glyph.index) == Some(&b' ')
                            });
                            (x, hanging.map_or(x, |glyph| glyph.position.x))
                        },
                    );
                    // The text indent only applies to the first row of the text.
                    let indent = if rows.is_empty() { indent } else { px(0.) };
                    let free = (bounds.size.width - indent - (end - left)).max(px(0.));
                    let next = boundary.map_or(offset + line.len(), |boundary| {
                        offset + glyphs[boundary.run_ix].glyphs[boundary.glyph_ix].index
                    });
                    let gaps = spaces(self.data.text.get(start..next).unwrap_or("").trim_end());
                    let justified = match self.justify {
                        Some(last) => (boundary.is_some() || last) && gaps > 0.,
                        None => false,
                    };
                    let (shift, gap) = if justified {
                        (indent, free / gaps)
                    } else {
                        let shift = match style.text_align {
                            renderer::TextAlign::Left => px(0.),
                            renderer::TextAlign::Center => free / 2.,
                            renderer::TextAlign::Right => free,
                        };
                        (indent + shift, px(0.))
                    };
                    let [above, below] = extents.get(rows.len()).copied().unwrap_or_default();
                    rows.push(crate::dom::Row {
                        shift,
                        right: shift + end - left + gap * gaps,
                        gap,
                        start,
                        drop: drop + above,
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
        let rows: crate::dom::Rows = rows.into();
        let hitboxes = self
            .data
            .cursors
            .iter()
            .filter_map(|&(index, cursor)| {
                let range = &self.data.ranges.get(index)?.1;
                let (a, b) = text_position(layout, &self.data.text, &rows, range.start)
                    .zip(text_position(layout, &self.data.text, &rows, range.end))?;
                let rect = if a.y == b.y {
                    Bounds::new(a, size(b.x - a.x, layout.line_height()))
                } else {
                    Bounds::new(
                        point(bounds.left(), a.y),
                        size(bounds.size.width, b.y - a.y + layout.line_height()),
                    )
                };
                Some((
                    window.insert_hitbox(rect, renderer::HitboxBehavior::Normal),
                    cursor,
                ))
            })
            .collect();
        // Inline boxes are recorded in layout units like Chrome, not at the snapped bounds.
        let exact = window.exact_layout_bounds(*id);
        let shift = exact.origin - bounds.origin;
        let view = &*self.view;
        let mut host = view.host.borrow_mut();
        let dom = host.dom.entry(view.id).or_default();
        let layout = self.inner.layout();
        let computed = dom.computed.clone();
        let mut spans: Option<Vec<(usize, Pixels)>> = None;
        for (id, range) in &self.data.ranges {
            if let Some((a, b, top, height)) = inline_box(
                layout,
                &self.data.text,
                &rows,
                range,
                computed
                    .as_deref()
                    .map(|styles| (&*styles.values[*id], styles.lengths[*id])),
                window,
            ) {
                let (left, width) = if a.y == b.y {
                    (a.x, b.x - a.x)
                } else {
                    // A box on several rows is the union of its pieces, which can overflow the text like Chrome's.
                    let spans = spans.get_or_insert_with(|| {
                        let mut spans = Vec::new();
                        let mut offset = 0;
                        for line in layout.line_layouts() {
                            let glyphs = &line.unwrapped_layout.runs;
                            let (mut left, mut start) = (px(0.), offset);
                            for boundary in line.wrap_boundaries.iter().map(Some).chain([None]) {
                                let (x, end, next) = boundary.map_or(
                                    (
                                        line.unwrapped_layout.width,
                                        line.unwrapped_layout.width,
                                        offset + line.len(),
                                    ),
                                    |boundary| {
                                        let glyph =
                                            &glyphs[boundary.run_ix].glyphs[boundary.glyph_ix];
                                        // A space before the break hangs outside the row.
                                        let hanging = glyphs[..=boundary.run_ix]
                                            .iter()
                                            .rev()
                                            .flat_map(|run| run.glyphs.iter().rev())
                                            .find(|other| other.index < glyph.index)
                                            .filter(|other| {
                                                self.data.text.as_bytes().get(offset + other.index)
                                                    == Some(&b' ')
                                            });
                                        (
                                            glyph.position.x,
                                            hanging
                                                .map_or(glyph.position.x, |other| other.position.x),
                                            offset + glyph.index,
                                        )
                                    },
                                );
                                spans.push((start, end - left));
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
                dom.bounds.insert(
                    *id,
                    [
                        (left + shift.x).into(),
                        (a.y + top + shift.y).into(),
                        width.into(),
                        (b.y - a.y + height).into(),
                    ],
                );
                dom.text_layouts.insert(
                    *id,
                    (
                        layout.clone(),
                        self.data.text.clone(),
                        range.clone(),
                        rows.clone(),
                    ),
                );
            }
        }
        (hitboxes, rows)
    }
    fn paint(
        &mut self,
        _: Bounds<Pixels>,
        _: &mut renderer::LayoutId,
        (hitboxes, rows): &mut Self::PrepaintState,
        window: &mut Window,
    ) {
        for (hitbox, cursor) in hitboxes.iter() {
            window.set_cursor_style(*cursor, hitbox);
        }
        let mut highlights = Vec::new();
        {
            let view = &*self.view;
            let host = view.host.borrow();
            let dom = &host.dom[&view.id];
            let layout = self.inner.layout();
            if let Some(styles) = &dom.computed {
                for &(index, ink) in &self.data.backgrounds {
                    let (id, range) = &self.data.ranges[index];
                    let style = &styles.values[*id];
                    let units = styles.lengths[*id];
                    if let Some((a, b, offset, height)) = inline_box(
                        layout,
                        &self.data.text,
                        rows,
                        range,
                        Some((style, units)),
                        window,
                    ) {
                        let radii = [
                            Prop::BORDER_TOP_LEFT_RADIUS,
                            Prop::BORDER_TOP_RIGHT_RADIUS,
                            Prop::BORDER_BOTTOM_RIGHT_RADIUS,
                            Prop::BORDER_BOTTOM_LEFT_RADIUS,
                        ]
                        .map(|corner| {
                            px(style
                                .get(corner)
                                .or_else(|| style.get(Prop::BORDER_RADIUS))
                                .and_then(|v| units.resolve(v, (b.x - a.x).into()))
                                .unwrap_or(0.))
                        });
                        let (top, line_height) = (layout.bounds().top(), layout.line_height());
                        let [first, last] = [a.y, b.y].map(|y| {
                            (((y - top) / line_height).round() as usize)
                                .min(rows.len().saturating_sub(1))
                        });
                        let clone = style
                            .get(Prop::BOX_DECORATION_BREAK)
                            .is_some_and(|value| value == "clone");
                        for row in first..=last {
                            let (start, end) = (row == first, row == last);
                            let extent = rows
                                .get(row)
                                .map_or((px(0.), layout.bounds().size.width), |row| {
                                    (row.shift, row.right)
                                });
                            let x = if start {
                                a.x
                            } else {
                                layout.bounds().left() + extent.0
                            };
                            let right = if end {
                                b.x
                            } else {
                                layout.bounds().left() + extent.1
                            };
                            let [left_side, right_side] = [start || clone, end || clone];
                            window.paint_quad(
                                renderer::fill(
                                    Bounds::new(
                                        point(x, top + line_height * row + offset),
                                        size(right - x, height),
                                    ),
                                    rgba(ink),
                                )
                                .corner_radii(renderer::Corners {
                                    top_left: if left_side { radii[0] } else { px(0.) },
                                    top_right: if right_side { radii[1] } else { px(0.) },
                                    bottom_right: if right_side { radii[2] } else { px(0.) },
                                    bottom_left: if left_side { radii[3] } else { px(0.) },
                                }),
                            );
                        }
                    }
                }
            }
            for (id, range) in &self.data.ranges {
                let Some([start, end]) = dom.selection.get(id) else {
                    continue;
                };
                let Some(styles) = &dom.computed else {
                    continue;
                };
                if styles.user_select[*id].0 == style::UserSelect::None {
                    continue;
                }
                let caret = dom.selection_caret == Some(*id);
                let text = &self.data.text[range.clone()];
                let start = range.start + crate::dom::utf16_index(text, *start);
                let end = range.start + crate::dom::utf16_index(text, *end);
                if caret {
                    // `auto` and `currentcolor` use the text color.
                    let ink = [Prop::CARET_COLOR, Prop::COLOR]
                        .into_iter()
                        .find_map(|prop| style::color(styles.values[*id].get(prop)?))
                        .map_or(0x000000ff, |(color, alpha)| color << 8 | alpha as u32);
                    if let Some(position) = text_position(layout, &self.data.text, rows, start) {
                        super::input::paint_caret(
                            view,
                            [*id, start, range.len()],
                            Bounds::new(position, size(px(1.), layout.line_height())),
                            rgba(ink),
                            window,
                        );
                    }
                } else if start != end {
                    let (color, background) =
                        style::selection_colors(&styles.values[*id], styles.selections.get(id));
                    highlights.push(TextHighlight {
                        range: start.min(end)..start.max(end),
                        color: color.map(rgba),
                        background: rgba(background),
                    });
                }
            }
        }
        let layout = self.inner.layout();
        let runs = self.runs.as_deref().unwrap_or(&self.data.runs);
        let strut = window
            .text_system()
            .resolve_font(&window.text_style().font());
        let passes = self
            .data
            .shadows
            .iter()
            .map(|&(x, y, blur, color)| (point(px(x), px(y)), Some(rgba(color)), blur))
            .chain([(point(px(0.), px(0.)), None, 0.)]);
        for (shift, ink, blur) in passes {
            let paint = |window: &mut Window| {
                let mut offset = 0;
                let mut row = 0;
                let mut origin = layout.bounds().origin + shift;
                let mask = window.content_mask().bounds;
                for line in layout.line_layouts() {
                    let count = line.wrap_boundaries.len() + 1;
                    let height = line.size(layout.line_height()).height;
                    if origin.y > mask.bottom() {
                        break;
                    }
                    if origin.y + height + layout.line_height() > mask.top() {
                        paint_text(
                            &line.unwrapped_layout,
                            &line.wrap_boundaries,
                            origin,
                            layout.line_height(),
                            runs,
                            offset,
                            if ink.is_some() { &[] } else { &highlights },
                            rows.get(row..row + count).unwrap_or_default(),
                            &self.data.text,
                            ink,
                            strut,
                            window,
                        );
                    }
                    row += count;
                    offset += line.len() + 1;
                    origin.y += height;
                    if offset > self.data.text.len() {
                        break;
                    }
                }
            };
            if blur > 0. {
                // The CSS blur radius is twice the gaussian deviation.
                let effect = renderer::PaintEffect {
                    blur: px(blur / 2.),
                    ..Default::default()
                };
                window.paint_effect(effect, paint);
            } else {
                paint(window);
            }
        }
    }
}
