use super::{
    SharedString, TextAlign, TextRun, TextWrap, WhiteSpace, WordBreak, WrapBoundary, WrappedLine,
    WrappedLineLayout,
};
use crate::{AvailableSpace, Bounds, Pixels, Point, Size, TextStyle, Window, px};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Default, Clone)]
pub struct TextLayout(Rc<RefCell<Option<TextLayoutInner>>>);

struct TextLayoutInner {
    shaped_width: Option<Pixels>,
    lines: Vec<WrappedLine>,
    line_height: Pixels,
    wrap_width: Option<Pixels>,
    truncated: bool,
    size: Size<Pixels>,
    baselines: [Pixels; 2],
    bounds: Option<Bounds<Pixels>>,
}

pub(crate) struct TextState {
    text: SharedString,
    runs: Vec<TextRun>,
    style: TextStyle,
    scale: f32,
    pub(crate) layout: TextLayout,
    probes: Cell<[Option<Probe>; 2]>,
}

type Probe = (Size<Pixels>, [Pixels; 2]);

impl PartialEq for TextState {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.runs == other.runs
            && self.style == other.style
            && self.scale == other.scale
    }
}

impl TextState {
    pub(crate) fn new(
        text: SharedString,
        runs: Option<Vec<TextRun>>,
        style: &TextStyle,
        scale: f32,
    ) -> Self {
        Self {
            runs: runs.unwrap_or_else(|| vec![style.to_run(text.len())]),
            text,
            style: style.clone(),
            scale,
            layout: TextLayout::default(),
            probes: Cell::default(),
        }
    }

    pub(crate) fn restyled(&self, own: bool, style: &TextStyle, scale: f32) -> Self {
        Self::new(
            self.text.clone(),
            own.then(|| self.runs.clone()),
            style,
            scale,
        )
    }

    pub(crate) fn measure(
        &self,
        known: Size<Option<Pixels>>,
        available: Size<AvailableSpace>,
        window: &mut Window,
    ) -> (Size<Pixels>, Option<[Pixels; 2]>) {
        let Self {
            text,
            runs,
            style,
            layout,
            probes,
            ..
        } = self;
        let line_height = style.line_height;
        let definite = |space: AvailableSpace| match space {
            AvailableSpace::Definite(width) => Some(width),
            _ => None,
        };
        let width = known.width.or(definite(available.width));
        let wraps = style.white_space == WhiteSpace::Normal;
        let wrap_width = width.filter(|_| wraps);
        let truncate = style.text_overflow.clone().zip(width);
        let probe = (wraps && width.is_none())
            .then(|| (available.width != AvailableSpace::MinContent) as usize);
        if let Some(probe) = probe {
            if let Some((size, baselines)) = probes.get()[probe] {
                return (size, Some(baselines));
            }
        } else if let Some(inner) = layout.0.borrow().as_ref()
            && wrap_width == inner.wrap_width
            && inner.shaped_width != Some(px(0.))
            && truncate.is_none()
            // WARNING: A truncated layout answers an unconstrained probe with the truncated size,
            // so it is never reused.
            && !inner.truncated
        {
            return (inner.size, Some(inner.baselines));
        }
        let min_content = probe == Some(0);
        let system = window.text_system().clone();
        let (text, runs, truncated) = match truncate {
            Some((affix, width)) => {
                let (text, runs, truncated) =
                    truncate_text(&system, style, text, runs, &affix, width, wrap_width);
                (text, Cow::Owned(runs), truncated)
            }
            None => (text.clone(), Cow::Borrowed(runs), false),
        };
        // WARNING: like CSS, `BreakWord` does not break inside words for the min-content size.
        let lines = system
            .shape_text_indented(
                text,
                style.font_size,
                &runs,
                if min_content {
                    Some(px(0.))
                } else {
                    wrap_width
                },
                style.line_clamp.filter(|_| !min_content),
                style.text_indent,
                if min_content {
                    TextWrap::Wrap
                } else {
                    style.text_wrap
                },
                match style.word_break {
                    WordBreak::BreakWord if min_content => WordBreak::Normal,
                    word_break => word_break,
                },
            )
            .unwrap_or_default();
        let mut size = Size::<Pixels>::default();
        for line in &lines {
            let line = line.size(line_height);
            size.height += line.height;
            size.width = size.width.max(line.width);
        }
        if min_content {
            size.width = px(0.);
            let mut start = px(0.) - style.text_indent;
            for line in &lines {
                let unwrapped = &line.unwrapped_layout;
                let mut breaks = line.wrap_boundaries.iter().peekable();
                let mut glyphs = unwrapped
                    .runs
                    .iter()
                    .enumerate()
                    .flat_map(|(run_index, run)| {
                        run.glyphs
                            .iter()
                            .enumerate()
                            .map(move |(glyph_index, glyph)| {
                                (
                                    WrapBoundary {
                                        run_index,
                                        glyph_index,
                                    },
                                    glyph,
                                )
                            })
                    })
                    .peekable();
                while let Some((boundary, glyph)) = glyphs.next() {
                    if breaks.next_if(|next| **next == boundary).is_some() {
                        start = glyph.position.x;
                    }
                    let end = glyphs
                        .peek()
                        .map_or(unwrapped.width, |(_, next)| next.position.x);
                    if line.text.as_bytes().get(glyph.index) != Some(&b' ') {
                        size.width = size.width.max(end - start);
                    }
                }
                start = px(0.);
            }
        }
        let baseline = system.baseline(
            system.resolve_font(&style.font()),
            style.font_size,
            line_height,
            window.scale_factor(),
        );
        let baselines = [baseline, baseline + size.height - line_height];
        if let Some(probe) = probe {
            let mut kept = probes.get();
            kept[probe] = Some((size, baselines));
            probes.set(kept);
        }
        // WARNING: taffy can take the final size from its cache, so a probe keeps the painted layout.
        if probe.is_none() || layout.0.borrow().is_none() {
            *layout.0.borrow_mut() = Some(TextLayoutInner {
                shaped_width: if min_content { Some(px(0.)) } else { width },
                lines,
                line_height,
                wrap_width,
                truncated,
                size,
                baselines,
                bounds: None,
            });
        }
        (size, Some(baselines))
    }
}

impl TextState {
    /// WARNING: taffy can answer the final width from its cache across frames, while the text was last shaped for another width.
    pub(crate) fn fit(&self, width: Pixels, window: &mut Window) {
        if self.style.white_space != WhiteSpace::Normal && self.style.text_overflow.is_none() {
            return;
        }
        let stale = self
            .layout
            .0
            .borrow()
            .as_ref()
            .is_some_and(|inner| match inner.shaped_width {
                Some(shaped) => (shaped - width).abs() > px(0.01),
                None => inner.size.width > width + px(0.01),
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

impl TextLayout {
    pub fn paint(&self, align: TextAlign, window: &mut Window) {
        let inner = self.0.borrow();
        let Some(inner) = inner.as_ref() else {
            return;
        };
        let Some(bounds) = inner.bounds else {
            return;
        };
        let mut origin = bounds.origin;
        for line in &inner.lines {
            line.paint_background(origin, inner.line_height, align, Some(bounds), window);
            line.paint(origin, inner.line_height, align, Some(bounds), window);
            origin.y += line.size(inner.line_height).height;
        }
    }

    pub fn index_for_position(&self, position: Point<Pixels>) -> Result<usize, usize> {
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
        let mut origin = bounds.origin;
        let mut start = 0;
        for line in &inner.lines {
            let bottom = origin.y + line.size(inner.line_height).height;
            if position.y > bottom {
                origin.y = bottom;
                start += line.len() + 1;
            } else {
                return match line.index_for_position(position - origin, inner.line_height) {
                    Ok(index) => Ok(start + index),
                    Err(index) => Err(start + index),
                };
            }
        }
        Err(start.saturating_sub(1))
    }

    pub fn position_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        let inner = self.0.borrow();
        let inner = inner.as_ref()?;
        let mut origin = inner.bounds?.origin;
        let mut start = 0;
        for line in &inner.lines {
            let end = start + line.len();
            if index < start {
                break;
            } else if index > end {
                origin.y += line.size(inner.line_height).height;
                start = end + 1;
            } else {
                return Some(origin + line.position_for_index(index - start, inner.line_height)?);
            }
        }
        None
    }

    pub fn line_layouts(&self) -> Vec<Rc<WrappedLineLayout>> {
        self.0
            .borrow()
            .as_ref()
            .map(|inner| inner.lines.iter().map(|line| line.layout.clone()).collect())
            .unwrap_or_default()
    }

    pub fn bounds(&self) -> Bounds<Pixels> {
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
        bounds: Bounds<Pixels>,
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

    pub(crate) fn place(&self, bounds: Bounds<Pixels>) {
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
    let Ok(lines) = system.shape_text(
        text.clone(),
        font_size,
        runs,
        clamp.map(|(_, width)| width),
        None,
        style.word_break,
    ) else {
        return unchanged();
    };
    let cut = match clamp {
        Some((max, wrap)) => {
            let mut rows = 0;
            let mut start = 0;
            let mut found = None;
            for line in &lines {
                let layout = &line.layout.unwrapped_layout;
                let ends = line
                    .wrap_boundaries
                    .iter()
                    .map(|boundary| {
                        layout.runs[boundary.run_index].glyphs[boundary.glyph_index].index
                    })
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
            .filter(|line| lines.len() > 1 || line.layout.unwrapped_layout.width > width)
            .map(|line| {
                (
                    0,
                    line.layout.unwrapped_layout.clone(),
                    0,
                    line.len(),
                    width,
                )
            }),
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
        .unwrap_or_else(|| style.to_run(affix.len()));
    let affix_width = system
        .layout_line(affix, font_size, &[affix_run], None)
        .width;
    let left = layout.x_for_index(row_start);
    let index = layout
        .index_for_x(left + (room - affix_width).max(px(0.)))
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
        None => new_runs.push(style.to_run(affix.len())),
    }
    (kept.into(), new_runs, true)
}
