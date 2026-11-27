use super::{
  RunCursor, SharedString, TextAlign, TextCanvas, TextLine, TextRun, TextStyle, TextSystem,
  TextWrap, WhiteSpace, WordBreak, Wrapping,
  breaks::{Banded, Indent, breaks_beside},
};
use crate::{AvailableSpace, Pixels, Point, Rect, Size, px};
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

#[derive(Clone, Copy, PartialEq)]
enum Shaped {
  Free,
  MinContent,
  Width(Pixels),
}

struct ParagraphState {
  shaped: Shaped,
  lines: Rc<[TextLine]>,
  line_height: Pixels,
  top: Pixels,
  wrap_width: Option<Pixels>,
  floats: Option<Rc<[[f32; 4]]>>,
  // Where the ellipsis starts.
  cut: Option<usize>,
  size: Size<Pixels>,
  baselines: [Pixels; 2],
  bounds: Option<Rect<Pixels>>,
  // The widest line or unbreakable piece, which can overflow the box. Found when a scroll container asks.
  overflow: Cell<Option<Pixels>>,
  // How far the ink of a glyph reaches outside its row. Found at the first paint.
  reach: Cell<Option<Pixels>>,
}

pub(crate) struct TextState {
  text: SharedString,
  runs: RefCell<Rc<[TextRun]>>,
  style: RefCell<TextStyle>,
  scale: f32,
  pub(crate) layout: Paragraph,
  probes: Cell<[Option<Probe>; 2]>,
  repaint: Cell<bool>,
  base: RefCell<Option<Base>>,
  // The floats of the last layout, which `fit` keeps.
  floats: RefCell<Option<Rc<[[f32; 4]]>>>,
}

type Probe = (Size<Pixels>, [Pixels; 2]);

type Base = (Rc<[TextLine]>, [Pixels; 2]);

impl TextState {
  pub(crate) fn new(
    text: SharedString,
    runs: Option<Rc<[TextRun]>>,
    style: &TextStyle,
    scale: f32,
  ) -> Self {
    Self {
      runs: RefCell::new(runs.unwrap_or_else(|| Rc::new([style.run(text.len())]))),
      text,
      style: RefCell::new(style.clone()),
      scale,
      layout: Paragraph::default(),
      probes: Cell::default(),
      repaint: Cell::new(false),
      base: RefCell::default(),
      floats: RefCell::default(),
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
    let same = Rc::ptr_eq(&old, &runs)
            || old.len() == runs.len()
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
                        && same!(a, b; len, font, letter_spacing, word_spacing, font_size, inset, nowrap, preserve, clear, tab_size, hyphenation, rtl, plaintext, embedding)
                });
    if !same {
      return false;
    }
    if !Rc::ptr_eq(&old, &runs) && *old != *runs || *self.style.borrow() != *new.style.borrow() {
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
    system: &TextSystem,
    scale: f32,
  ) -> (Size<Pixels>, Option<[Pixels; 2]>) {
    let floats = system.floats();
    self.measure_beside(known, available, system, scale, floats)
  }

  fn measure_beside(
    &self,
    known: Size<Option<Pixels>>,
    available: Size<AvailableSpace>,
    system: &TextSystem,
    scale: f32,
    floats: Option<Rc<[[f32; 4]]>>,
  ) -> (Size<Pixels>, Option<[Pixels; 2]>) {
    let Self {
      text,
      layout,
      probes,
      ..
    } = self;
    let (runs, style) = (self.runs.borrow(), self.style.borrow());
    let (runs, style): (&[TextRun], _) = (&runs, &*style);
    // Like Chrome's layout units, which count device pixels, a line is at most 2^25 device pixels tall.
    let line_height = style.line_height.min(px(33_554_431. / scale.max(1.)));
    let definite = |space: AvailableSpace| match space {
      AvailableSpace::Definite(width) => Some(width),
      _ => None,
    };
    let width = known.width.or(definite(available.width));
    let wraps = style.white_space == WhiteSpace::Normal;
    let wrap_width = width.filter(|_| wraps);
    let indent = style.indent(width.unwrap_or_default());
    let truncate = style.text_overflow.clone().zip(width);
    let probe = ((wraps || style.float > px(0.)) && width.is_none())
      .then(|| (available.width != AvailableSpace::MinContent) as usize);
    let floats = floats.filter(|_| width.is_some() && style.float == px(0.) && probe.is_none());
    if width.is_some() {
      *self.floats.borrow_mut() = floats.clone();
    }
    if let Some(probe) = probe {
      if let Some((size, baselines)) = probes.get()[probe] {
        return (size, Some(baselines));
      }
    } else if let Some(inner) = layout.0.borrow().as_ref()
            && wrap_width == inner.wrap_width
            && (wraps || style.text_indent_percent == 0.)
            && inner.floats == floats
            && inner.shaped != Shaped::MinContent
            && truncate.is_none()
            // A truncated layout answers an unconstrained probe with the truncated size,
            // so it is never reused.
            && inner.cut.is_none()
    {
      return (inner.size, Some(inner.baselines));
    }
    let min_content = probe == Some(0);
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
        let end = (line.line.glyphs.iter()).fold(line.line.width, |end, glyph| end.max(glyph.x));
        reach[(index > 0) as usize] = reach[(index > 0) as usize].max(end);
      }
      (lines, reach)
    };
    // Skip truncate_text for a line that fits, or it shapes twice.
    let banded = |floats| Banded {
      floats,
      line: line_height.0,
      y: 0.,
      rows: Vec::new(),
    };
    // Rows that don't wrap only take the insets of the floats beside them, and truncation takes the room
    // of the first row.
    let unwrapped = wrap_width.is_none() || truncate.is_some();
    let mut later = floats.as_deref().filter(|_| unwrapped).map(banded);
    let lean = later.as_ref().map_or(px(0.), |later| {
      let [left, right] = later.edges(0.).0;
      px(left + right)
    });
    let mut banded = floats.as_deref().filter(|_| !unwrapped).map(banded);
    let fits = truncate.as_ref().is_some_and(|(_, width)| {
      float == px(0.)
        && wrap_width.is_none()
        && style.line_clamp.is_none()
        && style.text_indent == px(0.)
        && style.text_indent_percent == 0.
        && {
          let mut kept = self.base.borrow_mut();
          let (lines, _) = kept.get_or_insert_with(base);
          lines.len() == 1 && lines[0].line.width <= *width - lean
        }
    });
    let mut cut = None;
    let (lines, top, truncated) = if (truncate.is_none() || fits) && float == px(0.) {
      let mut kept = self.base.borrow_mut();
      let (lines, [first, rest]) = kept.get_or_insert_with(base);
      let wrap = match min_content {
        true => Some((px(0.), None, TextWrap::Wrap)),
        false => wrap_width.map(|width| (width, style.line_clamp, style.text_wrap)),
      };
      let lines = match wrap {
        Some((width, ..))
          if banded.is_none()
            && *first <= width.min(width - indent.first)
            && *rest <= width.min(width - indent.of(1)[0]) =>
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
              let mut pieces = Vec::new();
              cursor.take(line.len(), |len, run| {
                if line.extra.is_some() {
                  pieces.push((len, run));
                }
                if run.nowrap {
                  ranges.push(at..at + len);
                }
                at += len;
              });
              let mut clear = 0;
              cursor.take(1, |_, run| clear = run.clear);
              let wrapping = Wrapping {
                width: Some(width),
                max_lines: clamp.map(|max: usize| max.saturating_sub(wrapped)),
                indent: indent.line(index),
                wrap,
                word_break,
              };
              let boundaries = breaks_beside(
                line.logical(),
                &line.text,
                &wrapping,
                &ranges,
                line.hyphens().unwrap_or_default(),
                banded.as_mut(),
              );
              wrapped += boundaries.len() + 1;
              let mut line = TextLine {
                breaks: boundaries.into(),
                wrap: Some(width),
                shifts: banded.as_mut().map_or_else(Rc::default, |banded| {
                  if clear > 0 {
                    banded.clear(clear);
                  }
                  std::mem::take(&mut banded.rows).into()
                }),
                ..line.clone()
              };
              if let Some(extra) = &line.extra {
                line.arrange(extra.rtl, &pieces);
              }
              line
            })
            .collect()
        }
      };
      (lines, px(0.), false)
    } else {
      let (text, runs, truncated) = match truncate {
        Some((affix, width)) => {
          let (text, runs, truncated) =
            truncate_text(system, style, text, runs, &affix, width - lean, wrap_width);
          cut = truncated.then(|| text.len() - affix.len());
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
        indent,
        wrap: if min_content {
          TextWrap::Wrap
        } else {
          style.text_wrap
        },
        word_break,
      };
      let shape = |runs: &[TextRun]| system.shape_text(&text, style.font_size, runs, &wrapping);
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
    let lines: Rc<[TextLine]> = match later.as_mut() {
      Some(later) => lines
        .iter()
        .map(|line| TextLine {
          shifts: later.rows(line.rows()),
          ..line.clone()
        })
        .collect(),
      None => lines,
    };
    let mut size = Size::<Pixels>::default();
    for (index, line) in lines.iter().enumerate() {
      let mut line_size = line.extent(line_height);
      if line.breaks.is_empty() {
        line_size.width = (line_size.width + indent.of(index)[0]).max(px(0.));
      }
      size.height += line_size.height;
      size.width = size.width.max(line_size.width);
    }
    if min_content {
      size.width = widest_piece(&lines, float - top, indent);
    } else if wrap_width.is_some() && !truncated {
      // Like CSS, a box that fits its content in the available width is never narrower than its widest word.
      size.width = size.width.max(widest_piece(&lines, float - top, indent));
    }
    if let Some(banded) = &banded {
      size.height = px(banded.y);
    }
    size.width = size.width.max(float);
    size.height += top;
    let baseline = system.baseline(
      system.face(&style.font()),
      style.font_size,
      line_height,
      scale,
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
        shaped: match width {
          _ if min_content => Shaped::MinContent,
          Some(width) => Shaped::Width(width),
          None => Shaped::Free,
        },
        lines,
        line_height,
        top,
        wrap_width,
        floats,
        cut,
        size,
        baselines,
        bounds: None,
        overflow: Cell::new(None),
        reach: Cell::new(None),
      });
    }
    (size, Some(baselines))
  }
}

pub fn widest_piece(lines: &[TextLine], float: Pixels, indent: Indent) -> Pixels {
  let mut width = px(0.);
  for (index, line) in lines.iter().enumerate() {
    let first = indent.of(index)[0];
    let mut start = if index == 0 { float } else { px(0.) } - first;
    let glyphs = &line.line.glyphs;
    let mut breaks = line.breaks.iter().peekable();
    for (at, glyph) in glyphs.iter().enumerate() {
      if breaks.next_if(|&&next| next as usize == at).is_some() {
        if let Some(hyphen) = line.hyphen_before(glyph.index as usize) {
          width = width.max(glyph.x + hyphen.line.width - start);
        }
        start = glyph.x;
      }
      let end = glyphs.get(at + 1).map_or(line.line.width, |next| next.x);
      if line.text.as_bytes().get(glyph.index as usize) != Some(&b' ') {
        width = width.max(end - start);
      }
    }
  }
  width
}

impl TextState {
  pub(crate) fn unplaced(&self) -> bool {
    self.repaint.get()
      || (self.layout.0.borrow().as_ref()).is_some_and(|inner| inner.bounds.is_none())
  }

  /// taffy can answer the final width from its cache across frames, while the text was last shaped for another width.
  pub(crate) fn fit(&self, width: Pixels, system: &TextSystem, scale: f32) {
    let style = self.style.borrow();
    let wraps = style.white_space == WhiteSpace::Normal || style.text_overflow.is_some();
    drop(style);
    let stale = self.repaint.take()
      || wraps
        && self
          .layout
          .0
          .borrow()
          .as_ref()
          .is_some_and(|inner| match inner.shaped {
            Shaped::Width(shaped) => (shaped - width).abs() > px(0.01),
            Shaped::MinContent => width.abs() > px(0.01),
            Shaped::Free => inner.size.width > width + px(0.01),
          });
    if stale {
      let floats = self.floats.borrow().clone();
      self.measure_beside(
        Size::new(Some(width), None),
        Size::new(AvailableSpace::Definite(width), AvailableSpace::MinContent),
        system,
        scale,
        floats,
      );
    }
  }
}

impl Paragraph {
  pub(crate) fn overflow(&self) -> Option<Pixels> {
    let inner = self.0.borrow();
    let state = inner.as_ref()?;
    let width = state.overflow.get().unwrap_or_else(|| {
      state
        .size
        .width
        .max(widest_piece(&state.lines, px(0.), Indent::default()))
    });
    state.overflow.set(Some(width));
    Some(width)
  }

  pub fn paint(&self, align: TextAlign, canvas: &mut impl TextCanvas) {
    let inner = self.0.borrow();
    let Some(inner) = inner.as_ref() else {
      return;
    };
    let Some(bounds) = inner.bounds else {
      return;
    };
    let mut origin = bounds.origin + Point::new(px(0.), inner.top);
    let width = Some(bounds.size.width);
    let line_height = inner.line_height;
    let reach = inner.reach.get().unwrap_or_else(|| {
      let reach = (inner.lines.iter())
        .map(|line| line.reach(line_height, canvas.text_system()))
        .fold(px(0.), Pixels::max);
      inner.reach.set(Some(reach));
      reach
    });
    let mask = canvas.content_mask().bounds;
    let (top, bottom) = (mask.top() - reach, mask.bottom() + reach);
    for line in inner.lines.iter() {
      let height = line.extent(line_height).height;
      let rows = line.rows();
      let first = (((top - origin.y) / line_height).floor().max(0.) as usize).min(rows);
      let last = (((bottom - origin.y) / line_height).ceil().max(0.) as usize).min(rows);
      if first >= last {
        if !line.line.glyphs.is_empty() {
          // A copy of this paint can't move: it lacks the line.
          canvas.cull();
        }
      } else {
        line.paint_background(origin, line_height, align, width, canvas);
        line.paint_rows(origin, line_height, align, width, first..last, canvas);
      }
      origin.y += height;
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
        // The ellipsis is not in the text.
        let place = |index| inner.cut.unwrap_or(usize::MAX).min(start + index);
        return match line.hit((at.y / inner.line_height) as usize, at.x, nearest) {
          Ok(index) => Ok(place(index)),
          Err(index) => Err(place(index)),
        };
      }
    }
    Err(inner.cut.unwrap_or(usize::MAX).min(start.saturating_sub(1)))
  }

  /// The spaces left of `x` in a row of the paragraph.
  pub fn spaces_before(&self, mut row: usize, x: Pixels) -> usize {
    let inner = self.0.borrow();
    for line in inner.iter().flat_map(|inner| inner.lines.iter()) {
      if row < line.rows() {
        return line.spaces_before(row, x);
      }
      row -= line.rows();
    }
    0
  }

  pub fn point_at(&self, index: usize) -> Option<Point<Pixels>> {
    let inner = self.0.borrow();
    let inner = inner.as_ref()?;
    // Like the text after it, the text under the ellipsis has no place.
    if inner.cut.is_some_and(|cut| index > cut) {
      return None;
    }
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

  /// Some glyphs are not in the order of the text, or a right-to-left row can have spaces at its left, or a line places edges of inline boxes.
  pub fn reordered(&self) -> bool {
    (self.0.borrow().as_ref()).is_some_and(|inner| {
      (inner.lines.iter()).any(|line| {
        line.rtl() || line.lead(0).is_some() || line.extra.is_some() && !line.line.sorted()
      })
    })
  }

  pub fn lines(&self) -> Rc<[TextLine]> {
    self.0.borrow().as_ref().map_or_else(
      || Rc::new([]) as Rc<[TextLine]>,
      |inner| inner.lines.clone(),
    )
  }

  pub fn bounds(&self) -> Rect<Pixels> {
    self
      .0
      .borrow()
      .as_ref()
      .and_then(|inner| inner.bounds)
      .unwrap_or_default()
  }

  pub fn shape(
    text: SharedString,
    runs: Rc<[TextRun]>,
    style: &TextStyle,
    width: Pixels,
    bounds: Rect<Pixels>,
    system: &TextSystem,
    scale: f32,
  ) -> Self {
    let state = TextState::new(text, Some(runs), style, scale);
    state.measure(
      Size::default(),
      Size::new(AvailableSpace::Definite(width), AvailableSpace::MinContent),
      system,
      scale,
    );
    state.layout.place(bounds);
    state.layout
  }

  pub fn shift(&self, by: Point<Pixels>) {
    if let Some(bounds) = self
      .0
      .borrow_mut()
      .as_mut()
      .and_then(|inner| inner.bounds.as_mut())
    {
      bounds.origin += by;
    }
  }

  pub(crate) fn place(&self, bounds: Rect<Pixels>) {
    if let Some(inner) = self.0.borrow_mut().as_mut() {
      inner.bounds = Some(bounds);
    }
  }

  pub fn line_height(&self) -> Pixels {
    self
      .0
      .borrow()
      .as_ref()
      .map_or(px(0.), |inner| inner.line_height)
  }
}

pub(crate) fn same_shaping(a: &TextStyle, b: &TextStyle) -> bool {
  same!(a, b; font_family, font_features, font_size, line_height, font_weight, font_style, font_synthesis, font_size_adjust,
        white_space, text_overflow, line_clamp, letter_spacing, word_spacing, text_indent, text_indent_percent, text_indent_rows, float, text_wrap, word_break)
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
        let glyphs = line.line.glyphs.len();
        let ends = line.breaks.iter().map(|&at| at as usize).chain([glyphs]);
        let mut row_start = 0;
        for end in ends {
          rows += 1;
          if rows == max && (end < glyphs || line.text.len() + start < text.len()) {
            let layout = line.logical().clone();
            found = Some((start, layout, row_start..end, wrap));
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
      .map(|line| {
        let layout = line.logical().clone();
        (0, layout, 0..line.line.glyphs.len(), width)
      }),
  };
  let Some((offset, layout, row, room)) = cut else {
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
  let glyphs = &layout.glyphs;
  let left = glyphs.get(row.start).map_or(px(0.), |glyph| glyph.x);
  let room = left + (room - affix_width).max(px(0.));
  let mut used = left;
  let row_end = glyphs
    .get(row.end)
    .map_or(layout.len, |glyph| glyph.index as usize);
  let index = row
    .into_iter()
    .find_map(|at| {
      used += glyphs.get(at + 1).map_or(layout.width, |next| next.x) - glyphs[at].x;
      (used > room).then_some(glyphs[at].index as usize)
    })
    .unwrap_or(row_end);
  // Like Chrome, a run of the other direction is whole or not there.
  let level = |at: usize| layout.levels.get(at).map_or(0, |level| level % 2);
  let base = u8::from(runs.first().is_some_and(|run| run.rtl));
  let at = glyphs.partition_point(|glyph| (glyph.index as usize) < index);
  let index = match at < glyphs.len() && at > 0 && level(at) != base && level(at - 1) != base {
    true => (0..at)
      .rfind(|&at| level(at) == base)
      .map_or(index, |at| glyphs[at + 1].index as usize),
    false => index,
  };
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
    new_runs.push(run.part(len, run.inset));
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
  fn line_clamp_cuts_right_to_left_text() {
    let system = crate::TextSystem::new();
    let style = TextStyle {
      line_clamp: Some(2),
      ..TextStyle::default()
    };
    for text in [
      "שלום עולם זהו טקסט ארוך בעברית שעובר כמה שורות",
      "hello world this is a long text that wraps on rows",
    ] {
      let text: SharedString = text.into();
      let runs = [style.run(text.len())];
      let (kept, _, truncated) =
        truncate_text(&system, &style, &text, &runs, "…", px(60.), Some(px(60.)));
      let head = kept.strip_suffix('…').unwrap();
      assert!(
        truncated && !head.is_empty() && text.starts_with(head),
        "{kept}"
      );
      assert!(head.len() < text.len() / 2, "{kept}");
    }
  }

  #[test]
  fn paint_changes_keep_the_measure() {
    let style = TextStyle::default();
    let run = |len: usize, color: u32| TextRun {
      color: Color::hex(color),
      ..style.run(len)
    };
    let state = |runs: Vec<TextRun>| TextState::new("ab cd".into(), Some(runs.into()), &style, 1.);
    let old = state(vec![run(2, 0xff0000ff), run(3, 0x0000ffff)]);
    *old.layout.0.borrow_mut() = Some(ParagraphState {
      shaped: Shaped::Free,
      lines: Rc::new([]),
      line_height: px(0.),
      top: px(0.),
      wrap_width: None,
      floats: None,
      cut: None,
      size: Size::default(),
      baselines: [px(0.); 2],
      bounds: None,
      overflow: Cell::new(None),
      reach: Cell::new(None),
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

  #[test]
  fn layout_changes_drop_the_measure() {
    let style = TextStyle::default();
    let state = |run: TextRun| TextState::new("a\tb".into(), Some(vec![run].into()), &style, 1.);
    let old = state(style.run(3));
    for run in [
      TextRun {
        tab_size: 2.,
        ..style.run(3)
      },
      TextRun {
        preserve: !style.run(3).preserve,
        ..style.run(3)
      },
    ] {
      assert!(!old.keep(&state(run)));
    }
  }
}
