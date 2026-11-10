use crate::style;
use crate::style::Prop;
use anyhow::Result;
use renderer::{AlignItems, Bounds, Pixels, Point, SharedString, Window, point, px, rgba, size};
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
    let smoothing = window.font_smoothing();
    let shift = |row: usize| rows.get(row).map_or(px(0.), |row| row.shift);
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
                    let glyph = &line.runs[boundary.run_index].glyphs[boundary.glyph_index];
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
            let Some(mut wrap) = (0..=wraps.len()).find(|&row| top(row) + tall > mask.top()) else {
                return;
            };
            let (mut left, from) = match wrap.checked_sub(1).map(|row| wraps[row]) {
                Some(at) => (
                    line.runs[at.run_index].glyphs[at.glyph_index].position.x,
                    (at.run_index, at.glyph_index),
                ),
                None => (px(0.), (0, 0)),
            };
            let mut text_run = 0;
            let mut run_end = runs.first().map_or(0, |run| run.len);
            let mut spaced = (usize::MAX, 0, 0.);
            'rows: for (run_index, run) in line.runs.iter().enumerate().skip(from.0) {
                let glyph_size = window
                    .text_system()
                    .bounding_box(run.font_id, run.font_size)
                    .size;
                let skip = if run_index == from.0 { from.1 } else { 0 };
                for (glyph_index, glyph) in run.glyphs.iter().enumerate().skip(skip) {
                    if wraps.get(wrap)
                        == Some(&renderer::WrapBoundary {
                            run_index,
                            glyph_index,
                        })
                    {
                        wrap += 1;
                        left = glyph.position.x;
                    }
                    if top(wrap) >= mask.bottom() {
                        break 'rows;
                    }
                    let index = offset + glyph.index;
                    while index >= run_end && text_run + 1 < runs.len() {
                        text_run += 1;
                        run_end += runs[text_run].len;
                    }
                    let color = ink.unwrap_or_else(|| {
                        highlights
                            .iter()
                            .find(|highlight| highlight.range.contains(&index))
                            .and_then(|highlight| highlight.color)
                            .unwrap_or(runs[text_run].color)
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

fn row_extents<'a>(
    lines: impl IntoIterator<Item = &'a renderer::WrappedLineLayout>,
    runs: &[renderer::TextRun],
    heights: &[(usize, style::LineHeight)],
    atoms: &[(usize, [f32; 3], Option<AlignItems>)],
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
    let mut edges = Vec::new();
    let mut offset = 0;
    for line in lines {
        let layout = &line.unwrapped_layout;
        let ends: Vec<usize> = line
            .wrap_boundaries
            .iter()
            .map(|boundary| layout.runs[boundary.run_index].glyphs[boundary.glyph_index].index)
            .collect();
        let first = rows.len();
        rows.resize(first + ends.len() + 1, root);
        for run in &layout.runs {
            let mut last = None;
            for glyph in &run.glyphs {
                let row = first + ends.partition_point(|&end| end <= glyph.index);
                let at = offset + glyph.index;
                if let Some((_, [_, height, baseline], align)) =
                    atoms.iter().find(|(place, ..)| *place == at)
                {
                    if let Some(align @ (AlignItems::FlexStart | AlignItems::FlexEnd)) = align {
                        edges.push((row, height * scale, *align == AlignItems::FlexEnd));
                        continue;
                    }
                    // Do not shift the row for fractional-pixel differences between a full-line baseline box
                    // and the strut.
                    let [above, below] = [baseline * scale, (height - baseline) * scale];
                    rows[row] = [
                        rows[row][0].max(if above - root[0] < 1. { 0. } else { above }),
                        rows[row][1].max(if below - root[1] < 1. { 0. } else { below }),
                    ];
                    continue;
                }
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
    edges.sort_by(|a: &(usize, f32, bool), b| b.1.total_cmp(&a.1));
    for (row, height, bottom) in edges {
        let [above, below] = &mut rows[row];
        if bottom {
            *above = above.max(height - *below);
        } else {
            *below = below.max(height - *above);
        }
    }
    rows.into_iter()
        .map(|[above, below]| {
            [above - root[0], below - root[1]].map(|extra| px(extra.max(0.) / scale))
        })
        .collect()
}

fn computed_clone(view: &super::PageState, id: usize) -> bool {
    view.host.borrow().dom.get(&view.id).is_some_and(|dom| {
        dom.computed.as_ref().is_some_and(|styles| {
            styles.values[id].get(Prop::BOX_DECORATION_BREAK) == Some("clone")
        })
    })
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

fn text_start(
    layout: &renderer::TextLayout,
    text: &str,
    rows: &[crate::dom::Row],
    index: usize,
    end: usize,
) -> Option<Point<Pixels>> {
    let at = text_position(layout, text, rows, index)?;
    let whole = text;
    let next = text
        .get(index..end)
        .and_then(|text| text.chars().next())
        .and_then(|first| text_position(layout, whole, rows, index + first.len_utf8()))
        .filter(|next| next.y > at.y);
    Some(next.map_or(at, |next| {
        let row = ((next.y - layout.bounds().top()) / layout.line_height()).round();
        point(
            layout.bounds().left() + rows.get(row as usize).map_or(px(0.), |row| row.shift),
            next.y,
        )
    }))
}

pub(crate) fn text_rects(
    layout: &renderer::TextLayout,
    text: &str,
    rows: &[crate::dom::Row],
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
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in layout.line_layouts() {
        starts.push(offset);
        for boundary in &line.wrap_boundaries {
            starts.push(
                offset
                    + line.unwrapped_layout.runs[boundary.run_index].glyphs[boundary.glyph_index]
                        .index,
            );
        }
        offset += line.len() + 1;
    }
    let mut rects = Vec::new();
    for (index, &row) in starts.iter().enumerate() {
        let next = starts.get(index + 1).copied().unwrap_or(text.len());
        let (from, to) = (start.max(row), end.min(next));
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

fn inline_box(
    layout: &renderer::TextLayout,
    text: &str,
    rows: &[crate::dom::Row],
    range: &std::ops::Range<usize>,
    style: Option<(&style::Style, style::Lengths)>,
    root: &renderer::TextStyle,
    window: &Window,
) -> Option<(Point<Pixels>, Point<Pixels>, Pixels, Pixels)> {
    let a = text_start(layout, text, rows, range.start, range.end)?;
    let b = text_position(layout, text, rows, range.end)?;
    let line_height = layout.line_height();
    let Some((style, units)) = style else {
        return Some((a, b, px(0.), line_height));
    };
    let system = window.text_system();
    let scale = window.scale_factor();
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
    let Some((base, pseudo, mode)) = data.first_line.as_deref() else {
        return (text.into(), data.runs.to_vec());
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
            Some(line.unwrapped_layout.runs[boundary.run_index].glyphs[boundary.glyph_index].index)
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
    pub(super) data: Rc<style::InlineText>,
    pub(super) view: Rc<super::PageState>,
    pub(super) atoms: Rc<[AtomSlot]>,
    pub(super) justify: Option<bool>,
    pub(super) shaped: std::cell::RefCell<Option<(renderer::TextLayout, Vec<renderer::TextRun>)>>,
    pub(super) prepainted: std::cell::RefCell<Prepainted>,
}

#[derive(Default)]
pub(super) struct Prepainted {
    hitboxes: Vec<(renderer::Hitbox, renderer::CursorStyle)>,
    rows: crate::dom::Rows,
    layouts: Vec<(usize, TextLayoutEntry)>,
    rects: Vec<(usize, [f32; 4])>,
    origin: Point<Pixels>,
}

type TextLayoutEntry = (
    renderer::TextLayout,
    Rc<str>,
    std::ops::Range<usize>,
    crate::dom::Rows,
    [Pixels; 2],
);

pub(super) struct AtomSlot {
    pub(super) size: [f32; 3],
    pub(super) align: Option<AlignItems>,
    pub(super) at: std::cell::Cell<Point<Pixels>>,
}

pub(super) struct AtomBox {
    pub(super) slots: Rc<[AtomSlot]>,
    pub(super) index: usize,
    pub(super) node: usize,
    pub(super) view: Rc<super::PageState>,
}

impl renderer::Hooks for AtomBox {
    fn prepaint(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let slot = &self.slots[self.index];
        let height = f32::from(cx.exact.size.height);
        let size = [
            f32::from(cx.exact.size.width),
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
            view.host
                .borrow_mut()
                .dom
                .entry(view.id)
                .or_default()
                .atoms
                .insert(self.node, size);
            cx.window.redraw();
        }
        let offset = slot.at.get() - cx.bounds.origin;
        cx.window
            .with_element_offset(offset, |window| inner(window));
    }

    fn paint(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
    }
}

fn with_atoms(
    data: &style::InlineText,
    sizes: impl Fn(usize) -> f32,
    runs: &mut [renderer::TextRun],
) {
    for (index, atom) in data.atoms.iter().enumerate() {
        if let Some(run) = runs.get_mut(atom.run).filter(|_| data.first_line.is_none()) {
            run.inset[1] = px(sizes(index));
        }
    }
}

struct Measured {
    data: Rc<style::InlineText>,
    atoms: Box<[([f32; 3], Option<AlignItems>)]>,
    styled: std::cell::RefCell<Option<renderer::TextStyle>>,
    measured: Rc<std::cell::Cell<[Pixels; 2]>>,
    cached: std::cell::Cell<Option<(Option<Pixels>, renderer::Size<Pixels>)>>,
    baselines: std::cell::Cell<[Pixels; 2]>,
}

impl PartialEq for Measured {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.data, &other.data) && self.atoms == other.atoms
    }
}

impl From<InlineText> for renderer::Node {
    fn from(text: InlineText) -> Self {
        let data = &text.data;
        let sized = data.runs.iter().any(|run| run.font_size.is_some())
            || data
                .first_line
                .as_deref()
                .is_some_and(|(_, pseudo, _)| pseudo.font_size.is_some());
        if data.first_line.is_none() && !sized {
            return renderer::text(data.text.clone(), Some(data.runs.to_vec())).hooks(text);
        }
        renderer::node()
            .measure(
                Measured {
                    data: data.clone(),
                    atoms: text
                        .atoms
                        .iter()
                        .map(|slot| (slot.size, slot.align))
                        .collect(),
                    styled: Default::default(),
                    measured: Rc::default(),
                    cached: std::cell::Cell::new(None),
                    baselines: std::cell::Cell::new([px(0.); 2]),
                },
                |kept, style, known, available, window| {
                    let Measured {
                        data,
                        atoms,
                        styled,
                        measured,
                        cached,
                        baselines: kept_baselines,
                    } = kept;
                    if styled.borrow().as_ref() != Some(style) {
                        cached.set(None);
                        *styled.borrow_mut() = Some(style.clone());
                    }
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
                    let (text, mut runs) = first_line(data, width, font_size, &system);
                    with_atoms(data, |index| atoms[index].0[0], &mut runs);
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
                    for (index, line) in lines.iter().enumerate() {
                        let mut line_size = line.size(line_height);
                        if index == 0 && line.wrap_boundaries.is_empty() {
                            line_size.width = (line_size.width + style.text_indent).max(px(0.));
                        }
                        size.height += line_size.height;
                        size.width = size.width.max(line_size.width);
                    }
                    let strut = (system.resolve_font(&style.font()), font_size, line_height);
                    let scale = window.scale_factor();
                    let extents = row_extents(
                        lines.iter().map(|line| &**line),
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
                    cached.set(Some((width, size)));
                    kept_baselines.set(baselines);
                    measured.set([width.unwrap_or_default(), size.width]);
                    (size, Some(baselines))
                },
            )
            .hooks(text)
    }
}

impl InlineText {
    fn cursors<'a>(
        &'a self,
        layout: &'a renderer::TextLayout,
        rows: &'a crate::dom::Rows,
        bounds: Bounds<Pixels>,
    ) -> impl Iterator<Item = (Bounds<Pixels>, renderer::CursorStyle)> + 'a {
        self.data
            .cursors
            .iter()
            .filter_map(move |&(index, cursor)| {
                let range = &self.data.ranges.get(index)?.1;
                let (a, b) = text_position(layout, &self.data.text, rows, range.start)
                    .zip(text_position(layout, &self.data.text, rows, range.end))?;
                let rect = if a.y == b.y {
                    Bounds::new(a, size(b.x - a.x, layout.line_height()))
                } else {
                    Bounds::new(
                        point(bounds.left(), a.y),
                        size(bounds.size.width, b.y - a.y + layout.line_height()),
                    )
                };
                Some((rect, cursor))
            })
    }
}

impl renderer::Hooks for InlineText {
    fn bounded(&self) -> bool {
        true
    }

    fn prepaint(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        let [top, right, bottom, left] = cx.inset;
        let inside = |bounds: Bounds<Pixels>| {
            Bounds::new(
                bounds.origin + point(left, top),
                size(
                    bounds.size.width - left - right,
                    bounds.size.height - top - bottom,
                ),
            )
        };
        let (bounds, exact, style) = (inside(cx.bounds), inside(cx.exact), cx.text.clone());
        let measured = cx.state::<Measured>().map(|kept| kept.measured.clone());
        let node_layout = cx.text_layout().cloned();
        let window = &mut *cx.window;
        if let Some(measured) = &measured {
            // Snapped bounds can be narrower than measured text. Do not wrap or truncate again.
            let wrap = measured
                .get()
                .into_iter()
                .filter(|width| *width >= bounds.size.width && *width - bounds.size.width < px(1.))
                .fold(bounds.size.width, Pixels::max);
            let width = (style.white_space == renderer::WhiteSpace::Normal).then_some(wrap);
            let font_size = style.font_size;
            let system = window.text_system().clone();
            let (text, mut runs) = first_line(&self.data, width, font_size, &system);
            with_atoms(&self.data, |index| self.atoms[index].size[0], &mut runs);
            // Like Chromium, text starts at its exact place, and only each row's baseline snaps to a device pixel.
            let placed = renderer::Bounds::new(exact.origin, bounds.size);
            let layout =
                renderer::TextLayout::shape(text, runs.clone(), &style, wrap, placed, window);
            *self.shaped.borrow_mut() = Some((layout, runs));
        }
        let shaped = self.shaped.borrow();
        let (layout, runs) = match (&*shaped, &node_layout) {
            (Some((layout, runs)), _) => (layout, &runs[..]),
            (None, Some(layout)) => (layout, &self.data.runs[..]),
            (None, None) => return,
        };
        let mut rows = Vec::new();
        let indent = style.text_indent;
        let extents = row_extents(
            layout.line_layouts().iter().map(|line| &**line),
            runs,
            &self.data.line_heights,
            &self
                .data
                .atoms
                .iter()
                .zip(self.atoms.iter())
                .map(|(atom, slot)| (atom.at, slot.size, slot.align))
                .collect::<Vec<_>>(),
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
            || !self.data.boxes.is_empty()
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
                            let x = glyphs[boundary.run_index].glyphs[boundary.glyph_index]
                                .position
                                .x;
                            let previous = boundary
                                .glyph_index
                                .checked_sub(1)
                                .map(|glyph| &glyphs[boundary.run_index].glyphs[glyph])
                                .or_else(|| {
                                    let run = boundary.run_index.checked_sub(1)?;
                                    glyphs[run].glyphs.last()
                                });
                            let hanging = previous.filter(|glyph| {
                                self.data.text.as_bytes().get(offset + glyph.index) == Some(&b' ')
                            });
                            (x, hanging.map_or(x, |glyph| glyph.position.x))
                        },
                    );
                    let indent = if rows.is_empty() { indent } else { px(0.) };
                    let free = (bounds.size.width - indent - (end - left)).max(px(0.));
                    let next = boundary.map_or(offset + line.len(), |boundary| {
                        offset + glyphs[boundary.run_index].glyphs[boundary.glyph_index].index
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
        for inline in &self.data.boxes {
            let (id, range) = &self.data.ranges[inline.index];
            if inline.lead == 0. || range.start == range.end {
                continue;
            }
            let clone = computed_clone(&self.view, *id);
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
        let rows: crate::dom::Rows = rows.into();
        for (atom, slot) in self.data.atoms.iter().zip(self.atoms.iter()) {
            let strut = window.text_system().resolve_font(&style.font());
            let base = window.text_system().baseline(
                strut,
                style.font_size,
                layout.line_height(),
                window.scale_factor(),
            );
            if let Some((a, ..)) = inline_box(
                layout,
                &self.data.text,
                &rows,
                &(atom.at..atom.at + '\u{fffc}'.len_utf8()),
                None,
                &style,
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
        let hitboxes = self
            .cursors(layout, &rows, bounds)
            .map(|(rect, cursor)| {
                (
                    window.insert_hitbox(rect, renderer::HitboxBehavior::Normal),
                    cursor,
                )
            })
            .collect();
        let shift = exact.origin - bounds.origin;
        let view = &*self.view;
        let mut host = view.host.borrow_mut();
        let dom = host.dom.entry(view.id).or_default();
        let computed = dom.computed.clone();
        let mut spans: Option<Vec<(usize, Pixels)>> = None;
        let (mut layouts, mut rects) = {
            let mut prepainted = self.prepainted.borrow_mut();
            (
                std::mem::take(&mut prepainted.layouts),
                std::mem::take(&mut prepainted.rects),
            )
        };
        layouts.clear();
        rects.clear();
        for (at, (id, range)) in self.data.ranges.iter().enumerate() {
            let boxed = self.data.boxes.iter().find(|inline| inline.index == at);
            if let Some((mut a, mut b, mut top, mut height)) = inline_box(
                layout,
                &self.data.text,
                &rows,
                range,
                computed
                    .as_deref()
                    .map(|styles| (&*styles.values[*id], styles.lengths[*id])),
                &style,
                window,
            ) {
                let font = [top, height];
                if let Some(inline) = boxed {
                    a.x -= px(inline.lead);
                    b.x = if inline.trail.is_nan() {
                        a.x + px(inline.edges[1] + inline.edges[3])
                    } else {
                        b.x - px(inline.trail)
                    };
                    top -= px(inline.edges[0]);
                    height += px(inline.edges[0] + inline.edges[2]);
                }
                let (left, width) = if a.y == b.y {
                    (a.x, b.x - a.x)
                } else {
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
                                        let glyph = &glyphs[boundary.run_index].glyphs
                                            [boundary.glyph_index];
                                        let hanging = glyphs[..=boundary.run_index]
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
                let rect = [
                    (left + shift.x).into(),
                    (a.y + top + shift.y).into(),
                    width.into(),
                    (b.y - a.y + height).into(),
                ];
                dom.bounds.insert(*id, rect);
                rects.push((*id, rect));
                let entry = (
                    layout.clone(),
                    self.data.text.clone(),
                    range.clone(),
                    rows.clone(),
                    font,
                );
                dom.text_layouts.insert(*id, entry.clone());
                layouts.push((*id, entry));
            }
        }
        std::mem::drop(host);
        std::mem::drop(shaped);
        *self.prepainted.borrow_mut() = Prepainted {
            hitboxes,
            rows,
            layouts,
            rects,
            origin: exact.origin,
        };
    }

    fn volatile(&self) -> bool {
        !self.atoms.is_empty()
            || self.data.first_line.is_some()
            || self.data.runs.iter().any(|run| run.font_size.is_some())
    }

    fn kept(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        let [top, right, bottom, left] = cx.inset;
        let bounds = Bounds::new(
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
        } = &mut *prepainted;
        let moved = origin != *at;
        let window = &mut *cx.window;
        let places: Vec<_> = match &layout {
            Some(layout) if moved => self
                .cursors(layout, rows, bounds)
                .map(|(rect, _)| rect)
                .collect(),
            _ => hitboxes.iter().map(|(hitbox, _)| hitbox.bounds).collect(),
        };
        for ((hitbox, _), place) in hitboxes.iter_mut().zip(places) {
            *hitbox = window.place_hitbox(hitbox, place);
        }
        let view = &*self.view;
        let mut host = view.host.borrow_mut();
        let dom = host.dom.entry(view.id).or_default();
        if moved {
            let [x, y] = [origin.x - at.x, origin.y - at.y].map(f32::from);
            *at = origin;
            for (id, rect) in rects.iter_mut() {
                rect[0] += x;
                rect[1] += y;
                dom.bounds.insert(*id, *rect);
            }
        }
        for (id, entry) in layouts.iter() {
            dom.text_layouts.insert(*id, entry.clone());
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
        let Prepainted { hitboxes, rows, .. } = &*prepainted;
        for (hitbox, cursor) in hitboxes.iter() {
            window.set_cursor_style(*cursor, hitbox);
        }
        let mut highlights = Vec::new();
        {
            let view = &*self.view;
            let host = view.host.borrow();
            let dom = &host.dom[&view.id];
            if let Some(styles) = &dom.computed {
                for inline in &self.data.boxes {
                    if inline.fill == 0 && (inline.border == 0 || inline.borders == [0.; 4]) {
                        continue;
                    }
                    let (id, range) = &self.data.ranges[inline.index];
                    let style = &styles.values[*id];
                    let units = styles.lengths[*id];
                    if let Some((a, b, offset, height)) = inline_box(
                        layout,
                        &self.data.text,
                        rows,
                        range,
                        Some((style, units)),
                        &text_style,
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
                        let drop = |row: usize| rows.get(row).map_or(px(0.), |row| row.drop);
                        let row_at = |y: Pixels| {
                            (0..rows.len().max(1))
                                .rev()
                                .find(|&row| {
                                    top + line_height * row as f32 + drop(row) <= y + px(0.5)
                                })
                                .unwrap_or(0)
                        };
                        let [first, last] = [a.y, b.y].map(row_at);
                        let clone = style
                            .get(Prop::BOX_DECORATION_BREAK)
                            .is_some_and(|value| value == "clone");
                        let [width_above, width_right, width_below, width_left] =
                            inline.borders.map(px);
                        let [above, right_edge, below, left_edge] = inline.edges.map(px);
                        for row in first..=last {
                            let (start, end) = (row == first, row == last);
                            let extent = rows
                                .get(row)
                                .map_or((px(0.), layout.bounds().size.width), |row| {
                                    (row.shift, row.right)
                                });
                            let [left_side, right_side] = [start || clone, end || clone];
                            let x = if start {
                                a.x - px(inline.lead)
                            } else {
                                layout.bounds().left() + extent.0
                                    - if clone { left_edge } else { px(0.) }
                            };
                            let right = if end {
                                if inline.trail.is_nan() {
                                    x + left_edge + right_edge
                                } else {
                                    b.x - px(inline.trail)
                                }
                            } else {
                                layout.bounds().left()
                                    + extent.1
                                    + if clone { right_edge } else { px(0.) }
                            };
                            window.paint_quad(renderer::quad(
                                Bounds::new(
                                    point(
                                        x,
                                        top + line_height * row as f32 + drop(row) + offset - above,
                                    ),
                                    size(right - x, height + above + below),
                                ),
                                renderer::Corners {
                                    top_left: if left_side { radii[0] } else { px(0.) },
                                    top_right: if right_side { radii[1] } else { px(0.) },
                                    bottom_right: if right_side { radii[2] } else { px(0.) },
                                    bottom_left: if left_side { radii[3] } else { px(0.) },
                                },
                                rgba(inline.fill),
                                renderer::Edges {
                                    top: width_above,
                                    right: if right_side { width_right } else { px(0.) },
                                    bottom: width_below,
                                    left: if left_side { width_left } else { px(0.) },
                                },
                                rgba(inline.border),
                                renderer::BorderStyle::Solid,
                            ));
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
                if styles.user_select[*id].0 == style::UserSelect::None || dom.inert(*id) {
                    continue;
                }
                let caret = dom.selection_caret == Some(*id);
                let text = &self.data.text[range.clone()];
                let source = host
                    .pages
                    .get(&view.id)
                    .map_or("", |page| page.doc.nodes[*id].text.as_str());
                let start = range.start + crate::dom::shown_index(source, text, *start);
                let end = range.start + crate::dom::shown_index(source, text, *end);
                if caret {
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
        let strut = window.text_system().resolve_font(&text_style.font());
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
                let effect = renderer::PaintEffect {
                    blur: px(blur / 2.),
                    ..Default::default()
                };
                window.paint_effect(effect, paint);
            } else {
                paint(window);
            }
        }
        true
    }
}
