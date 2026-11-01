use super::{
    SharedString, TextAlign, TextRun, TextWrap, WhiteSpace, WordBreak, WrapBoundary, WrappedLine,
    WrappedLineLayout,
};
use crate::{
    AvailableSpace, Bounds, Element, IntoElement, LayoutId, Pixels, Point, Size, Style, TextStyle,
    Window, px,
};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    rc::Rc,
};

/// Text in the inherited text style, or with its own runs.
pub struct StyledText {
    text: SharedString,
    runs: Option<Vec<TextRun>>,
    layout: TextLayout,
}

impl StyledText {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            runs: None,
            layout: TextLayout::default(),
        }
    }

    /// Runs that cover the whole text.
    pub fn with_runs(mut self, runs: Vec<TextRun>) -> Self {
        debug_assert_eq!(
            runs.iter().map(|run| run.len).sum::<usize>(),
            self.text.len(),
            "invalid text runs"
        );
        self.runs = Some(runs);
        self
    }

    pub fn layout(&self) -> &TextLayout {
        &self.layout
    }
}

impl Element for StyledText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn request_layout(&mut self, window: &mut Window) -> (LayoutId, ()) {
        (
            self.layout
                .layout(self.text.clone(), self.runs.take(), window),
            (),
        )
    }

    fn prepaint(&mut self, bounds: Bounds<Pixels>, _: &mut (), _: &mut Window) {
        if let Some(inner) = self.layout.0.borrow_mut().as_mut() {
            inner.bounds = Some(bounds);
        }
    }

    fn paint(&mut self, _: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window) {
        self.layout.paint(window);
    }
}

impl IntoElement for StyledText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl IntoElement for SharedString {
    type Element = StyledText;
    fn into_element(self) -> StyledText {
        StyledText::new(self)
    }
}

impl IntoElement for String {
    type Element = StyledText;
    fn into_element(self) -> StyledText {
        StyledText::new(self)
    }
}

impl IntoElement for &'static str {
    type Element = StyledText;
    fn into_element(self) -> StyledText {
        StyledText::new(self)
    }
}

/// The shaped lines of a text element, shared with whoever needs positions in it.
#[derive(Default, Clone)]
pub struct TextLayout(Rc<RefCell<Option<TextLayoutInner>>>);

struct TextLayoutInner {
    len: usize,
    lines: Vec<WrappedLine>,
    line_height: Pixels,
    wrap_width: Option<Pixels>,
    truncated: bool,
    size: Size<Pixels>,
    /// The first and last line's baselines.
    baselines: [Pixels; 2],
    bounds: Option<Bounds<Pixels>>,
}

/// What a text layout depends on, with the layout it holds. Equal inputs keep the layout.
struct Measured {
    text: SharedString,
    runs: Vec<TextRun>,
    style: TextStyle,
    scale: f32,
    layout: TextLayout,
    /// The min-content and max-content sizes and baselines of wrapping text.
    probes: Cell<[Option<Probe>; 2]>,
}

type Probe = (Size<Pixels>, [Pixels; 2]);

impl PartialEq for Measured {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.runs == other.runs
            && self.style == other.style
            && self.scale == other.scale
    }
}

impl TextLayout {
    fn layout(
        &mut self,
        text: SharedString,
        runs: Option<Vec<TextRun>>,
        window: &mut Window,
    ) -> LayoutId {
        let style = window.text_style();
        let runs = runs.unwrap_or_else(|| vec![style.to_run(text.len())]);
        let measured = Measured {
            text,
            runs,
            style,
            scale: window.scale_factor(),
            layout: self.clone(),
            probes: Cell::default(),
        };
        let (id, measured) = window.request_measured_layout(
            &Style::default(),
            measured,
            |measured, known, available, window| {
                let Measured {
                    text,
                    runs,
                    style,
                    layout,
                    probes,
                    ..
                } = measured;
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
                    && truncate.is_none()
                    // A truncated layout answers an unconstrained probe with the truncated size,
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
                let len = text.len();
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
                            .flat_map(|(run_ix, run)| {
                                run.glyphs.iter().enumerate().map(move |(glyph_ix, glyph)| {
                                    (WrapBoundary { run_ix, glyph_ix }, glyph)
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
                        len,
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
            },
        );
        *self = measured.layout.clone();
        id
    }

    pub fn paint(&self, window: &mut Window) {
        let inner = self.0.borrow();
        let Some(inner) = inner.as_ref() else {
            return;
        };
        let Some(bounds) = inner.bounds else {
            return;
        };
        let align = window
            .text_styles
            .last()
            .map_or(TextAlign::Left, |style| style.text_align);
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

    pub fn line_layout_for_index(&self, index: usize) -> Option<Rc<WrappedLineLayout>> {
        let inner = self.0.borrow();
        let mut start = 0;
        for line in &inner.as_ref()?.lines {
            let end = start + line.len();
            if index < start {
                break;
            } else if index > end {
                start = end + 1;
            } else {
                return Some(line.layout.clone());
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

    pub fn line_height(&self) -> Pixels {
        self.0
            .borrow()
            .as_ref()
            .map_or(px(0.), |inner| inner.line_height)
    }

    pub fn len(&self) -> usize {
        self.0.borrow().as_ref().map_or(0, |inner| inner.len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Cuts the text that does not fit `width`, or `line_clamp` rows of `wrap_width`, and ends it
/// with `affix`.
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
    // The cut index and the end of the row it is in.
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
                    .map(|boundary| layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index)
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
