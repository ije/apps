use super::{
    RunCursor, SharedString, TextAlign, TextLine, TextRun, TextStyle, TextWrap, WhiteSpace,
    WordBreak, Wrapping, breaks::breaks,
};
use crate::{AvailableSpace, Pixels, Point, Rect, Size, Window, px};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    rc::Rc,
};

macro_rules! same {
    ($a:expr, $b:expr; $($field:ident),*) => { $($a.$field == $b.$field)&&* };
}

#[derive(Default, Clone)]
pub struct Paragraph(Rc<RefCell<Option<ParagraphState>>>);

struct ParagraphState {
    shaped_width: Option<Pixels>,
    lines: Rc<[TextLine]>,
    line_height: Pixels,
    top: Pixels,
    wrap_width: Option<Pixels>,
    truncated: bool,
    size: Size<Pixels>,
    baselines: [Pixels; 2],
    bounds: Option<Rect<Pixels>>,
    // The widest line or unbreakable piece, which can overflow the box. Found when a scroll container asks.
    overflow: Cell<Option<Pixels>>,
}

pub(crate) struct TextState {
    text: SharedString,
    runs: RefCell<Vec<TextRun>>,
    style: RefCell<TextStyle>,
    scale: f32,
    pub(crate) layout: Paragraph,
    probes: Cell<[Option<Probe>; 2]>,
    repaint: Cell<bool>,
    base: RefCell<Option<Base>>,
}

type Probe = (Size<Pixels>, [Pixels; 2]);

type Base = (Rc<[TextLine]>, [Pixels; 2]);

impl TextState {
    pub(crate) fn new(
        text: SharedString,
        runs: Option<Vec<TextRun>>,
        style: &TextStyle,
        scale: f32,
    ) -> Self {
        Self {
            runs: RefCell::new(runs.unwrap_or_else(|| vec![style.run(text.len())])),
            text,
            style: RefCell::new(style.clone()),
            scale,
            layout: Paragraph::default(),
            probes: Cell::default(),
            repaint: Cell::new(false),
            base: RefCell::default(),
        }
    }

    pub(crate) fn restyled(&self, own: bool, style: &TextStyle, scale: f32) -> Self {
        Self::new(
            self.text.clone(),
            own.then(|| self.runs.borrow().clone()),
            style,
            scale,
        )
    }

    pub(crate) fn keep(&self, new: &TextState) -> bool {
        if self.text != new.text
            || self.scale != new.scale
            || !same_shaping(&self.style.borrow(), &new.style.borrow())
        {
            return false;
        }
        let (old, runs) = (self.runs.borrow(), new.runs.borrow());
        let mut last: Option<(&TextRun, &TextRun)> = None;
        // Runs split where their decoration changes, which breaks ligatures, so those places must stay.
        let same = old.len() == runs.len()
            && old
                .iter()
                .zip(runs.iter())
                .filter(|(a, b)| a.len > 0 || b.len > 0)
                .all(|(a, b)| {
                    let joined = last.is_none_or(|(x, y)| {
                        same!(x, a; color, background_color, underline, strikethrough, overline)
                            == same!(y, b; color, background_color, underline, strikethrough, overline)
                    });
                    last = Some((a, b));
                    joined
                        && same!(a, b; len, font, letter_spacing, word_spacing, font_size, inset, nowrap)
                });
        if !same {
            return false;
        }
        if *old != *runs || *self.style.borrow() != *new.style.borrow() {
            drop((old, runs));
            self.runs.swap(&new.runs);
            self.style.swap(&new.style);
            self.base.take();
            if self.layout.0.borrow_mut().take().is_some() {
                self.repaint.set(true);
            }
        }
        true
    }

    pub(crate) fn measure(
        &self,
        known: Size<Option<Pixels>>,
        available: Size<AvailableSpace>,
        window: &mut Window,
    ) -> (Size<Pixels>, Option<[Pixels; 2]>) {
        let Self {
            text,
            layout,
            probes,
            ..
        } = self;
        let (runs, style) = (self.runs.borrow(), self.style.borrow());
        let (runs, style) = (&*runs, &*style);
        let line_height = style.line_height;
        let definite = |space: AvailableSpace| match space {
            AvailableSpace::Definite(width) => Some(width),
            _ => None,
        };
        let width = known.width.or(definite(available.width));
        let wraps = style.white_space == WhiteSpace::Normal;
        let wrap_width = width.filter(|_| wraps);
        let truncate = style.text_overflow.clone().zip(width);
        let probe = ((wraps || style.float > px(0.)) && width.is_none())
            .then(|| (available.width != AvailableSpace::MinContent) as usize);
        if let Some(probe) = probe {
            if let Some((size, baselines)) = probes.get()[probe] {
                return (size, Some(baselines));
            }
        } else if let Some(inner) = layout.0.borrow().as_ref()
            && wrap_width == inner.wrap_width
            && inner.shaped_width != Some(px(0.))
            && truncate.is_none()
            // A truncated layout answers an unconstrained probe with the truncated size,
            // so it is never reused.
            && !inner.truncated
        {
            return (inner.size, Some(inner.baselines));
        }
        let min_content = probe == Some(0);
        let system = window.text_system().clone();
        // Like CSS, `BreakWord` does not break inside words for the min-content size.
        let word_break = match style.word_break {
            WordBreak::BreakWord if min_content => WordBreak::Normal,
            word_break => word_break,
        };
        let float = style.float;
        let base = || {
            let lines: Rc<[TextLine]> = system
                .shape_text(text, style.font_size, runs, &Wrapping::default())
                .into();
            let mut reach = [px(0.); 2];
            for (index, line) in lines.iter().enumerate() {
                let end =
                    (line.line.glyphs.iter()).fold(line.line.width, |end, glyph| end.max(glyph.x));
                reach[(index > 0) as usize] = reach[(index > 0) as usize].max(end);
            }
            (lines, reach)
        };
        // Skip truncate_text for a line that fits, or it shapes twice.
        let fits = truncate.as_ref().is_some_and(|(_, width)| {
            float == px(0.)
                && wrap_width.is_none()
                && style.line_clamp.is_none()
                && style.text_indent == px(0.)
                && {
                    let mut kept = self.base.borrow_mut();
                    let (lines, _) = kept.get_or_insert_with(base);
                    lines.len() == 1 && lines[0].line.width <= *width
                }
        });
        let (lines, top, truncated) = if (truncate.is_none() || fits) && float == px(0.) {
            let mut kept = self.base.borrow_mut();
            let (lines, [first, rest]) = kept.get_or_insert_with(base);
            let wrap = match min_content {
                true => Some((px(0.), None, TextWrap::Wrap)),
                false => wrap_width.map(|width| (width, style.line_clamp, style.text_wrap)),
            };
            let lines = match wrap {
                Some((width, ..))
                    if *first <= width.min(width - style.text_indent) && *rest <= width =>
                {
                    lines.clone()
                }
                None => lines.clone(),
                Some((width, clamp, wrap)) => {
                    let mut cursor = RunCursor {
                        runs,
                        at: 0,
                        used: 0,
                    };
                    let mut wrapped = 0;
                    lines
                        .iter()
                        .enumerate()
                        .map(|(index, line)| {
                            let (mut ranges, mut at) = (Vec::new(), 0);
                            cursor.take(line.len(), |len, run| {
                                if run.nowrap {
                                    ranges.push(at..at + len);
                                }
                                at += len;
                            });
                            cursor.take(1, |_, _| {});
                            let wrapping = Wrapping {
                                width: Some(width),
                                max_lines: clamp.map(|max: usize| max.saturating_sub(wrapped)),
                                indent: if index == 0 {
                                    style.text_indent
                                } else {
                                    px(0.)
                                },
                                wrap,
                                word_break,
                            };
                            let boundaries = breaks(&line.line, &line.text, &wrapping, &ranges);
                            wrapped += boundaries.len() + 1;
                            TextLine {
                                breaks: boundaries.into(),
                                wrap: Some(width),
                                ..line.clone()
                            }
                        })
                        .collect()
                }
            };
            (lines, px(0.), false)
        } else {
            let (text, runs, truncated) = match truncate {
                Some((affix, width)) => {
                    let (text, runs, truncated) =
                        truncate_text(&system, style, text, runs, &affix, width, wrap_width);
                    (text, Cow::Owned(runs), truncated)
                }
                None => (text.clone(), Cow::Borrowed(runs), false),
            };
            let wrapping = Wrapping {
                width: if min_content && wraps {
                    Some(px(0.))
                } else {
                    wrap_width
                },
                max_lines: style.line_clamp.filter(|_| !min_content),
                indent: style.text_indent,
                wrap: if min_content {
                    TextWrap::Wrap
                } else {
                    style.text_wrap
                },
                word_break,
            };
            let shape =
                |runs: &[TextRun]| system.shape_text(&text, style.font_size, runs, &wrapping);
            let beside = (float > px(0.) && !(min_content && wraps))
                .then(|| {
                    let mut runs = runs.to_vec();
                    runs.first_mut()?.inset[0] += float;
                    Some(shape(&runs))
                })
                .flatten()
                .filter(|lines| {
                    wrap_width
                        .zip(lines.first())
                        .is_none_or(|(width, line)| line.first_row_end() <= width + px(0.01))
                });
            let top = if beside.is_none() { float } else { px(0.) };
            (
                beside.unwrap_or_else(|| shape(&runs)).into(),
                top,
                truncated,
            )
        };
        let mut size = Size::<Pixels>::default();
        for (index, line) in lines.iter().enumerate() {
            let mut line_size = line.extent(line_height);
            if index == 0 && line.breaks.is_empty() {
                line_size.width = (line_size.width + style.text_indent).max(px(0.));
            }
            size.height += line_size.height;
            size.width = size.width.max(line_size.width);
        }
        if min_content {
            size.width = widest_piece(&lines, float - top - style.text_indent);
        }
        size.width = size.width.max(float);
        size.height += top;
        let baseline = system.baseline(
            system.face(&style.font()),
            style.font_size,
            line_height,
            window.scale_factor(),
        );
        let baselines = [top + baseline, baseline + size.height - line_height];
        if let Some(probe) = probe {
            let mut kept = probes.get();
            kept[probe] = Some((size, baselines));
            probes.set(kept);
        }
        // taffy can take the final size from its cache, so a probe keeps the painted layout.
        if probe.is_none() || layout.0.borrow().is_none() {
            self.repaint.set(false);
            *layout.0.borrow_mut() = Some(ParagraphState {
                shaped_width: if min_content { Some(px(0.)) } else { width },
                lines,
                line_height,
                top,
                wrap_width,
                truncated,
                size,
                baselines,
                bounds: None,
                overflow: Cell::new(None),
            });
        }
        (size, Some(baselines))
    }
}

pub fn widest_piece(lines: &[TextLine], mut start: Pixels) -> Pixels {
    let mut width = px(0.);
    for line in lines {
        let glyphs = &line.line.glyphs;
        let mut breaks = line.breaks.iter().peekable();
        for (at, glyph) in glyphs.iter().enumerate() {
            if breaks.next_if(|&&next| next as usize == at).is_some() {
                start = glyph.x;
            }
            let end = glyphs.get(at + 1).map_or(line.line.width, |next| next.x);
            if line.text.as_bytes().get(glyph.index as usize) != Some(&b' ') {
                width = width.max(end - start);
            }
        }
        start = px(0.);
    }
    width
}

impl TextState {
    /// taffy can answer the final width from its cache across frames, while the text was last shaped for another width.
    pub(crate) fn fit(&self, width: Pixels, window: &mut Window) {
        let style = self.style.borrow();
        let wraps = style.white_space == WhiteSpace::Normal || style.text_overflow.is_some();
        drop(style);
        let stale =
            self.repaint.take()
                || wraps
                    && self.layout.0.borrow().as_ref().is_some_and(|inner| {
                        match inner.shaped_width {
                            Some(shaped) => (shaped - width).abs() > px(0.01),
                            None => inner.size.width > width + px(0.01),
                        }
                    });
        if stale {
            self.measure(
                Size::new(Some(width), None),
                Size::new(AvailableSpace::Definite(width), AvailableSpace::MinContent),
                window,
            );
        }
    }
}

impl Paragraph {
    pub(crate) fn overflow(&self) -> Option<Pixels> {
        let inner = self.0.borrow();
        let state = inner.as_ref()?;
        let width = state
            .overflow
            .get()
            .unwrap_or_else(|| state.size.width.max(widest_piece(&state.lines, px(0.))));
        state.overflow.set(Some(width));
        Some(width)
    }

    pub fn paint(&self, align: TextAlign, window: &mut Window) {
        let inner = self.0.borrow();
        let Some(inner) = inner.as_ref() else {
            return;
        };
        let Some(bounds) = inner.bounds else {
            return;
        };
        let mut origin = bounds.origin + Point::new(px(0.), inner.top);
        let width = Some(bounds.size.width);
        for line in inner.lines.iter() {
            line.paint_background(origin, inner.line_height, align, width, window);
            line.paint(origin, inner.line_height, align, width, window);
            origin.y += line.extent(inner.line_height).height;
        }
    }

    /// With `nearest`, the index of the nearest character edge, for a caret. Else the character under the point.
    pub fn index_at(&self, position: Point<Pixels>, nearest: bool) -> Result<usize, usize> {
        let inner = self.0.borrow();
        let Some((inner, bounds)) = inner
            .as_ref()
            .and_then(|inner| Some((inner, inner.bounds?)))
        else {
            return Err(0);
        };
        if position.y < bounds.top() {
            return Err(0);
        }
        let mut origin = bounds.origin + Point::new(px(0.), inner.top);
        let mut start = 0;
        for line in inner.lines.iter() {
            let bottom = origin.y + line.extent(inner.line_height).height;
            if position.y > bottom {
                origin.y = bottom;
                start += line.len() + 1;
            } else {
                let at = position - origin;
                return match line.hit((at.y / inner.line_height) as usize, at.x, nearest) {
                    Ok(index) => Ok(start + index),
                    Err(index) => Err(start + index),
                };
            }
        }
        Err(start.saturating_sub(1))
    }

    pub fn point_at(&self, index: usize) -> Option<Point<Pixels>> {
        let inner = self.0.borrow();
        let inner = inner.as_ref()?;
        let mut origin = inner.bounds?.origin + Point::new(px(0.), inner.top);
        let mut start = 0;
        for line in inner.lines.iter() {
            let end = start + line.len();
            if index < start {
                break;
            } else if index > end {
                origin.y += line.extent(inner.line_height).height;
                start = end + 1;
            } else {
                let (row, x) = line.caret(index - start)?;
                return Some(origin + Point::new(x, inner.line_height * row));
            }
        }
        None
    }

    pub fn lines(&self) -> Rc<[TextLine]> {
        self.0.borrow().as_ref().map_or_else(
            || Rc::new([]) as Rc<[TextLine]>,
            |inner| inner.lines.clone(),
        )
    }

    pub fn bounds(&self) -> Rect<Pixels> {
        self.0
            .borrow()
            .as_ref()
            .and_then(|inner| inner.bounds)
            .unwrap_or_default()
    }

    pub fn shape(
        text: SharedString,
        runs: Vec<TextRun>,
        style: &TextStyle,
        width: Pixels,
        bounds: Rect<Pixels>,
        window: &mut Window,
    ) -> Self {
        let state = TextState::new(text, Some(runs), style, window.scale_factor());
        state.measure(
            Size::default(),
            Size::new(AvailableSpace::Definite(width), AvailableSpace::MinContent),
            window,
        );
        state.layout.place(bounds);
        state.layout
    }

    pub(crate) fn place(&self, bounds: Rect<Pixels>) {
        if let Some(inner) = self.0.borrow_mut().as_mut() {
            inner.bounds = Some(bounds);
        }
    }

    pub fn line_height(&self) -> Pixels {
        self.0
            .borrow()
            .as_ref()
            .map_or(px(0.), |inner| inner.line_height)
    }
}

pub(crate) fn same_shaping(a: &TextStyle, b: &TextStyle) -> bool {
    same!(a, b; font_family, font_features, font_size, line_height, font_weight, font_style,
        white_space, text_overflow, line_clamp, letter_spacing, word_spacing, text_indent, float, text_wrap, word_break)
}

fn truncate_text(
    system: &super::TextSystem,
    style: &TextStyle,
    text: &SharedString,
    runs: &[TextRun],
    affix: &str,
    width: Pixels,
    wrap_width: Option<Pixels>,
) -> (SharedString, Vec<TextRun>, bool) {
    let unchanged = || (text.clone(), runs.to_vec(), false);
    let font_size = style.font_size;
    let clamp = style.line_clamp.zip(wrap_width);
    let lines = system.shape_text(
        text,
        font_size,
        runs,
        &Wrapping {
            width: clamp.map(|(_, width)| width),
            word_break: style.word_break,
            ..Wrapping::default()
        },
    );
    let cut = match clamp {
        Some((max, wrap)) => {
            let mut rows = 0;
            let mut start = 0;
            let mut found = None;
            for line in &lines {
                let layout = &line.line;
                let ends = (line.breaks.iter())
                    .map(|&at| layout.glyphs[at as usize].index as usize)
                    .chain([layout.len]);
                let mut row_start = 0;
                for end in ends {
                    rows += 1;
                    if rows == max && (end < layout.len || line.text.len() + start < text.len()) {
                        found = Some((start, layout.clone(), row_start, end, wrap));
                        break;
                    }
                    row_start = end;
                }
                if found.is_some() {
                    break;
                }
                start += line.len() + 1;
            }
            found
        }
        None => lines
            .first()
            .filter(|line| lines.len() > 1 || line.line.width > width)
            .map(|line| (0, line.line.clone(), 0, line.len(), width)),
    };
    let Some((offset, layout, row_start, row_end, room)) = cut else {
        return unchanged();
    };
    let affix_run = runs
        .last()
        .cloned()
        .map(|run| TextRun {
            len: affix.len(),
            inset: Default::default(),
            nowrap: false,
            ..run
        })
        .unwrap_or_else(|| style.run(affix.len()));
    let affix_width = system.shape_line(affix, font_size, &affix_run).line.width;
    let left = layout.x_at(row_start);
    let index = layout
        .index_at(left + (room - affix_width).max(px(0.)))
        .unwrap_or(row_end)
        .clamp(row_start, row_end);
    let end = offset + index;
    let mut kept = String::with_capacity(end + affix.len());
    kept.push_str(text[..end].trim_end());
    let end = kept.len();
    kept.push_str(affix);
    let mut new_runs = Vec::new();
    let mut remaining = end;
    for run in runs {
        if remaining == 0 {
            break;
        }
        let len = run.len.min(remaining);
        new_runs.push(TextRun { len, ..run.clone() });
        remaining -= len;
    }
    match new_runs.last_mut() {
        Some(last) => last.len += affix.len(),
        None => new_runs.push(style.run(affix.len())),
    }
    (kept.into(), new_runs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    #[test]
    fn paint_changes_keep_the_measure() {
        let style = TextStyle::default();
        let run = |len: usize, color: u32| TextRun {
            color: Color::hex(color),
            ..style.run(len)
        };
        let state = |runs: Vec<TextRun>| TextState::new("ab cd".into(), Some(runs), &style, 1.);
        let old = state(vec![run(2, 0xff0000ff), run(3, 0x0000ffff)]);
        *old.layout.0.borrow_mut() = Some(ParagraphState {
            shaped_width: None,
            lines: Rc::new([]),
            line_height: px(0.),
            top: px(0.),
            wrap_width: None,
            truncated: false,
            size: Size::default(),
            baselines: [px(0.); 2],
            bounds: None,
            overflow: Cell::new(None),
        });
        assert!(old.keep(&state(vec![run(2, 0xff0000ff), run(3, 0x0000ffff)])));
        assert!(old.layout.0.borrow().is_some() && !old.repaint.get());
        assert!(old.keep(&state(vec![run(2, 0x00ff00ff), run(3, 0x0000ffff)])));
        assert_eq!(old.runs.borrow()[0].color, Color::hex(0x00ff00ff));
        assert!(old.layout.0.borrow().is_none() && old.repaint.get());
        assert!(!old.keep(&state(vec![run(2, 0x0000ffff), run(3, 0x0000ffff)])));
        assert!(!old.keep(&state(vec![run(3, 0x00ff00ff), run(2, 0x0000ffff)])));
        let wide = TextStyle {
            letter_spacing: px(1.),
            color: Color::hex(0xff0000ff),
            ..style.clone()
        };
        assert!(same_shaping(
            &style,
            &TextStyle {
                color: Color::hex(0xff0000ff),
                ..style.clone()
            }
        ));
        assert!(!same_shaping(&style, &wide));
    }
}
