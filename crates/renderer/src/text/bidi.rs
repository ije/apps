use super::{Extra, Fonts, Line, Span, TextLine, TextRun};
use crate::{Pixels, px};
use std::{ops::Range, rc::Rc};
use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};

/// The inline boxes around a run, outermost first.
#[derive(Clone, Default, PartialEq)]
pub struct Embedding {
  /// Those with a `unicode-bidi`: a number for each, and the character that opens what it does.
  pub marks: Vec<(usize, char)>,
  /// Those with edges, in text of several directions.
  pub boxes: Vec<InlineEdges>,
}

/// The edges of an inline box, at its left and its right.
#[derive(Clone, Copy, PartialEq)]
pub struct InlineEdges {
  pub id: usize,
  pub rtl: bool,
  /// The run has the start and the end of the box.
  pub ends: [bool; 2],
  pub margins: [Pixels; 2],
  pub edges: [Pixels; 2],
}

impl InlineEdges {
  fn start(&self) -> Pixels {
    let side = usize::from(self.rtl);
    self.margins[side] + self.edges[side]
  }

  fn end(&self) -> Pixels {
    let side = usize::from(!self.rtl);
    self.margins[side] + self.edges[side]
  }
}

/// Glyphs of an inline box that are together in a row.
#[derive(Clone)]
pub(crate) struct BoxPart {
  pub id: usize,
  pub row: usize,
  pub glyphs: Range<usize>,
  /// From the border edges to the glyphs.
  pub pads: [Pixels; 2],
  /// The part has the left and the right edge of the box.
  pub sides: [bool; 2],
}

/// A box in a line, with its pieces.
struct LineBox {
  edges: InlineEdges,
  pieces: Range<usize>,
}

/// Whether a text has right-to-left letters or characters that order text.
pub fn reorders(text: &str) -> bool {
  text.bytes().any(|byte| byte >= 0xd6)
    && text.chars().any(|ch| match ch {
      '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => true,
      '\u{590}'..='\u{8ff}'
      | '\u{fb1d}'..='\u{feff}'
      | '\u{10800}'..='\u{10fff}'
      | '\u{1e800}'..='\u{1efff}' => {
        matches!(bidi_class(ch), BidiClass::R | BidiClass::AL | BidiClass::AN)
      }
      _ => false,
    })
}

/// Whether the first letter of a text is right-to-left.
pub fn right_to_left(text: &str) -> Option<bool> {
  text.chars().find_map(|ch| match ch {
    'a'..='z' | 'A'..='Z' => Some(false),
    '\0'..='\x7f' => None,
    ch => match bidi_class(ch) {
      BidiClass::L => Some(false),
      BidiClass::R | BidiClass::AL => Some(true),
      _ => None,
    },
  })
}

/// Whether the paragraph of a line is right-to-left.
pub(super) fn base(text: &str, pieces: &[(usize, &TextRun)]) -> bool {
  pieces.first().is_some_and(|(_, run)| match run.plaintext {
    true => right_to_left(text).unwrap_or(run.rtl),
    false => run.rtl,
  })
}

// The level is in the low bits. In the high bits of the first byte after an isolate starts or ends, the level of the
// text around the isolate and 1.
fn depth(packed: u8) -> u8 {
  packed & 15
}

fn pack(level: Level) -> u8 {
  let level = level.number();
  if level > 13 { 12 + level % 2 } else { level }
}

/// The bidi level of each byte, or nothing for text in one left-to-right run.
pub(super) fn levels(text: &str, pieces: &[(usize, &TextRun)]) -> Option<Box<[u8]>> {
  pieces.first()?;
  let embedded = pieces.iter().any(|(_, run)| run.embedding.is_some());
  let rtl = base(text, pieces);
  if !rtl && !embedded && !reorders(text) {
    return None;
  }
  let base = Some(match rtl {
    true => Level::rtl(),
    false => Level::ltr(),
  });
  if !embedded {
    let info = BidiInfo::new(text, base);
    return Some(info.levels.iter().map(|&level| pack(level)).collect());
  }
  // The text with the characters that open and close the embeddings.
  let mut marked = String::with_capacity(text.len() + 16);
  let mut places = Vec::with_capacity(text.len());
  let mut open: Vec<(usize, char)> = Vec::new();
  let mut start = 0;
  let close = |marked: &mut String, open: &mut Vec<(usize, char)>, keep: usize| {
    for (_, mark) in open.drain(keep..).rev() {
      marked.push(match mark {
        '\u{2066}'..='\u{2068}' => '\u{2069}',
        _ => '\u{202c}',
      });
    }
  };
  for (len, run) in pieces {
    let wanted = run.embedding.as_ref().map_or(&[][..], |own| &own.marks);
    let kept = (open.iter().zip(wanted))
      .take_while(|(a, b)| a == b)
      .count();
    close(&mut marked, &mut open, kept);
    for &entry in &wanted[kept..] {
      marked.push(entry.1);
      open.push(entry);
    }
    places.extend((0..*len).map(|at| marked.len() + at));
    marked.push_str(&text[start..start + len]);
    start += len;
  }
  close(&mut marked, &mut open, 0);
  let info = BidiInfo::new(&marked, base);
  let mut last = None;
  let packed = places.into_iter().map(|at| {
    let own = pack(info.levels[at]);
    let marks = last.map_or(at..at, |last: usize| last + 1..at);
    last = Some(at);
    let around = marks
      .filter(|&mark| {
        matches!(
          marked.get(mark..mark + 3),
          Some("\u{2066}" | "\u{2067}" | "\u{2068}" | "\u{2069}")
        )
      })
      .map(|mark| pack(info.levels[mark]) + 1)
      .min();
    own | around.unwrap_or(0) << 4
  });
  Some(packed.collect())
}

/// Shapes text of several directions. The glyphs are in the order of the text, with their levels.
pub(super) fn shape(
  fonts: &Fonts,
  text: &str,
  font_size: Pixels,
  spans: &[Span],
  levels: &[u8],
) -> Line {
  let mut line = Line {
    len: text.len(),
    font_size,
    ..Default::default()
  };
  let mut start = 0;
  while start < text.len() {
    let packed = levels[start];
    let level = depth(packed);
    let mut end = start + 1;
    while end < text.len() && levels[end] == level {
      end += 1;
    }
    let run = &text[start..end];
    let mut own = Vec::new();
    let mut at = 0;
    for span in spans {
      let [from, to] = [at, at + span.len].map(|index| index.clamp(start, end));
      at += span.len;
      if from < to {
        own.push(Span {
          len: to - from,
          inset: [px(0.); 2],
          ..*span
        });
      }
    }
    let odd = level % 2 == 1;
    let wrapped = odd || reorders(run);
    const MARK: usize = 3;
    let shaped = if wrapped {
      let marked = format!("{}{run}\u{202c}", if odd { '\u{202e}' } else { '\u{202d}' });
      if let Some(first) = own.first_mut() {
        first.len += MARK;
      }
      if let Some(last) = own.last_mut() {
        last.len += MARK;
      }
      fonts.shape(&marked, font_size, &own)
    } else {
      fonts.shape(run, font_size, &own)
    };
    let shift = if wrapped { MARK } else { 0 };
    let inside =
      |glyph: &super::Glyph| (shift..shift + run.len()).contains(&(glyph.index as usize));
    // The advance of each glyph, in visual order.
    let mut glyphs = Vec::with_capacity(shaped.glyphs.len());
    for (at, glyph) in shaped.glyphs.iter().enumerate() {
      if inside(glyph) {
        let next = (shaped.glyphs.get(at + 1)).map_or(shaped.width, |next| next.x);
        let mut glyph = *glyph;
        glyph.index = glyph.index - shift as u32 + start as u32;
        glyphs.push((glyph, next - glyph.x));
      }
    }
    if odd {
      glyphs.reverse();
    }
    // Few glyphs are out of order, like spaces at the end.
    for at in 1..glyphs.len() {
      let mut to = at;
      while to > 0 && glyphs[to - 1].0.index > glyphs[to].0.index {
        glyphs.swap(to - 1, to);
        to -= 1;
      }
    }
    for (at, (mut glyph, advance)) in glyphs.into_iter().enumerate() {
      glyph.x = line.width;
      line.width += advance;
      line.glyphs.push(glyph);
      line.levels.push(if at == 0 { packed } else { level });
    }
    start = end;
  }
  line
}

/// The room that the insets of runs take in a line, which is in the advances of the glyphs before them.
struct Insets {
  boxes: Vec<LineBox>,
  /// The piece of each glyph.
  pieces: Vec<usize>,
  /// The boxes of each piece, outermost first.
  inside: Vec<Vec<usize>>,
  /// Before each glyph, and after the last: all the room, and the room that no box takes.
  before: Vec<[Pixels; 2]>,
}

impl Insets {
  #[cold]
  fn new(text: &str, line: &Line, pieces: &[(usize, &TextRun)]) -> Self {
    let mut boxes: Vec<LineBox> = Vec::new();
    let mut inside: Vec<Vec<usize>> = Vec::with_capacity(pieces.len());
    let mut ends = Vec::with_capacity(pieces.len());
    for (at, (len, run)) in pieces.iter().enumerate() {
      ends.push(ends.last().copied().unwrap_or(0) + len);
      let wanted = run.embedding.as_ref().map_or(&[][..], |own| &own.boxes);
      let own = wanted.iter().enumerate().map(|(depth, edges)| {
        let last = inside.last().and_then(|last| last.get(depth).copied());
        let known = last.filter(|&last| boxes[last].edges.id == edges.id);
        let known = known.unwrap_or_else(|| {
          boxes.push(LineBox {
            edges: *edges,
            pieces: at..at,
          });
          boxes.len() - 1
        });
        boxes[known].pieces.end = at + 1;
        boxes[known].edges.ends[1] = edges.ends[1];
        known
      });
      let own = own.collect();
      inside.push(own);
    }
    let piece = |glyph: &super::Glyph| {
      ends
        .partition_point(|&end| end <= glyph.index as usize)
        .min(pieces.len() - 1)
    };
    let places: Vec<usize> = line.glyphs.iter().map(piece).collect();
    let before = (0..=places.len()).map(|at| {
      let last = at.checked_sub(1).map(|last| places[last]);
      let next = places.get(at).copied();
      if last == next {
        return [px(0.); 2];
      }
      let run = |piece: usize| pieces[piece].1;
      let mut all = px(0.);
      let mut closed = px(0.);
      if let Some(last) = last {
        for &own in &inside[last] {
          let LineBox { edges, pieces } = &boxes[own];
          if edges.ends[1] && next.is_none_or(|next| !pieces.contains(&next)) {
            closed += edges.end();
          }
        }
        // The inset of an object has its width too.
        let object = text[line.glyphs[at - 1].index as usize..].starts_with('\u{fffc}');
        all += if object { closed } else { run(last).inset[1] };
      }
      let mut opened = px(0.);
      if let Some(next) = next {
        for &own in &inside[next] {
          let LineBox { edges, pieces } = &boxes[own];
          if edges.ends[0] && last.is_none_or(|last| !pieces.contains(&last)) {
            opened += edges.start();
          }
        }
        for piece in last.map_or(next, |last| last + 1)..=next {
          all += run(piece).inset[0];
        }
      }
      [all, all - opened - closed]
    });
    let before = before.collect();
    Self {
      boxes,
      pieces: places,
      inside,
      before,
    }
  }

  /// Adds the parts of the boxes in a row, whose glyphs are `visual` in text order. Gives the room of the edges at
  /// the left and the right of each glyph.
  #[cold]
  fn row(
    &self,
    row: usize,
    glyphs: Range<usize>,
    visual: &[usize],
    parts: &mut [Vec<BoxPart>],
  ) -> Vec<[Pixels; 2]> {
    let mut pads = vec![[px(0.); 2]; visual.len()];
    // The places of each box. Inner boxes are after the boxes around them.
    let inside = |at: usize| &self.inside[self.pieces[at]];
    let first = (visual.iter().filter_map(|&at| inside(at).first()).min()).map_or(0, |at| *at);
    let mut places: Vec<Vec<usize>> = Vec::new();
    for (place, &at) in visual.iter().enumerate() {
      for &own in inside(at) {
        if places.len() <= own - first {
          places.resize(own - first + 1, Vec::new());
        }
        places[own - first].push(place);
      }
    }
    for (at, group) in places.iter().enumerate().rev() {
      let [Some(&low), Some(&high)] = [group.first(), group.last()] else {
        continue;
      };
      let LineBox { edges, pieces } = &self.boxes[first + at];
      let start = self.pieces.partition_point(|&own| own < pieces.start);
      let end = self.pieces.partition_point(|&own| own < pieces.end);
      let mut ends = [
        edges.ends[0] && glyphs.contains(&start),
        edges.ends[1] && glyphs.contains(&end.wrapping_sub(1)),
      ];
      if edges.rtl {
        ends.reverse();
      }
      for run in group.chunk_by(|a, b| a + 1 == *b) {
        let outer = [run[0], run[run.len() - 1]];
        let sides = [ends[0] && outer[0] == low, ends[1] && outer[1] == high];
        let mut own = [px(0.); 2];
        for side in 0..2 {
          let pad = &mut pads[outer[side]][side];
          own[side] = *pad;
          if sides[side] {
            own[side] += edges.edges[side];
            *pad = own[side] + edges.margins[side];
          }
        }
        parts[first + at].push(BoxPart {
          id: edges.id,
          row,
          glyphs: glyphs.start + outer[0]..glyphs.start + outer[1] + 1,
          pads: own,
          sides,
        });
      }
    }
    pads
  }
}

impl TextLine {
  /// Places the glyphs of a line of several directions, whose rows break at `breaks`, in visual order.
  pub(super) fn arrange(&mut self, rtl: bool, pieces: &[(usize, &TextRun)]) {
    let logical = self.logical().clone();
    if logical.levels.is_empty() || logical.glyphs.is_empty() {
      return;
    }
    let boxed = |run: &TextRun| {
      run
        .embedding
        .as_ref()
        .is_some_and(|own| !own.boxes.is_empty())
    };
    let insets =
      (pieces.iter().any(|(_, run)| boxed(run))).then(|| Insets::new(&self.text, &logical, pieces));
    let base = u8::from(rtl);
    let mut line = Line {
      glyphs: Vec::with_capacity(logical.glyphs.len()),
      levels: Vec::with_capacity(logical.glyphs.len()),
      rights: Vec::new(),
      ..*logical
    };
    let mut pen = match insets {
      Some(_) => px(0.),
      None => logical.glyphs[0].x,
    };
    let mut starts = Vec::with_capacity(self.breaks.len());
    let boxes = insets.as_ref().map_or(0, |insets| insets.boxes.len());
    let mut parts = vec![Vec::new(); boxes];
    let mut leads = Vec::new();
    let mut from = 0;
    let ends = self.breaks.iter().map(|&at| at as usize);
    for (row, end) in ends.chain([logical.glyphs.len()]).enumerate() {
      let space =
        |at: &usize| self.text.as_bytes().get(logical.glyphs[*at].index as usize) == Some(&b' ');
      // Spaces at the end of a wrapped row hang after its text.
      let hung = match end < logical.glyphs.len() || !rtl {
        true => (from..end).rev().take_while(space).count(),
        false => 0,
      };
      // With a place between isolates, which keeps them apart.
      let mut items = Vec::with_capacity(end - from);
      let mut levels = Vec::with_capacity(end - from);
      for (at, &packed) in logical
        .levels
        .iter()
        .enumerate()
        .take(end - hung)
        .skip(from)
      {
        for (item, level) in [
          (None, (packed >> 4).checked_sub(1)),
          (Some(at), Some(depth(packed))),
        ] {
          if let Some(level) = level.and_then(|level| Level::new(level).ok()) {
            items.push(item);
            levels.push(level);
          }
        }
      }
      let mut visual = BidiInfo::reorder_visual(&levels);
      visual.retain_mut(|item| items[*item].map(|at| *item = at).is_some());
      let pads = insets
        .as_ref()
        .map(|insets| insets.row(row, from..end, &visual, &mut parts));
      let shown = visual.len();
      visual.extend(end - hung..end);
      for (place, &at) in visual.iter().enumerate() {
        let glyph = logical.glyphs[at];
        let next = (logical.glyphs.get(at + 1)).map_or(logical.width, |next| next.x);
        let level = match place < shown {
          true => depth(logical.levels[at]),
          false => base,
        };
        let mut advance = next - glyph.x;
        let mut room = [px(0.); 2];
        if let Some((insets, pads)) = insets.as_ref().zip(pads.as_ref()) {
          advance -= insets.before[at + 1][0];
          room = pads.get(place).copied().unwrap_or_default();
          // The edge before a right-to-left glyph is at its right.
          room[usize::from(level % 2)] += insets.before[at][1];
          if at + 1 == logical.glyphs.len() {
            room[usize::from(level % 2 == 0)] += insets.before[at + 1][1];
          }
          if place == 0 {
            leads.push(room[0]);
          }
        }
        pen += room[0];
        line.glyphs.push(super::Glyph { x: pen, ..glyph });
        line.levels.push(level);
        pen += advance;
        if insets.is_some() {
          line.rights.push(pen);
        }
        pen += room[1];
      }
      if let Some(next) = logical.glyphs.get(end) {
        starts.push(next.index);
      }
      from = end;
    }
    if insets.is_some() {
      line.width = pen;
    }
    line.sort_check();
    self.extra = Some(Rc::new(Extra {
      hyphens: self.extra.as_ref().and_then(|extra| extra.hyphens.clone()),
      logical: Some(logical),
      starts,
      rtl,
      preserve: pieces.first().is_some_and(|(_, run)| run.preserve),
      parts: parts.into_iter().flatten().collect(),
      leads,
    }));
    self.line = Rc::new(line);
  }
}

#[cfg(test)]
mod tests {
  use crate::{Font, TextRun, TextSystem, Wrapping, px};
  use std::rc::Rc;

  fn run(len: usize, rtl: bool) -> TextRun {
    TextRun {
      len,
      rtl,
      font: Font::new("Helvetica"),
      ..TextRun::default()
    }
  }

  fn marks(id: usize, mark: char) -> Option<Rc<crate::Embedding>> {
    Some(Rc::new(crate::Embedding {
      marks: vec![(id, mark)],
      boxes: Vec::new(),
    }))
  }

  fn visual(text: &str, runs: &[TextRun], width: Option<f32>) -> Vec<String> {
    let system = TextSystem::new();
    let wrapping = Wrapping {
      width: width.map(px),
      ..Wrapping::default()
    };
    let lines = system.shape_text(text, px(16.), runs, &wrapping);
    let line = &lines[0];
    (0..line.rows())
      .map(|row| {
        line
          .row_glyphs(row)
          .map(|at| {
            let index = line.line.glyphs[at].index as usize;
            line.text[index..].chars().next().unwrap()
          })
          .collect()
      })
      .collect()
  }

  #[test]
  fn rows_break_in_text_order_and_show_in_visual_order() {
    assert_eq!(visual("abc", &[run(3, false)], None), ["abc"]);
    assert_eq!(visual("ab אבג cd", &[run(12, false)], None), ["ab גבא cd"]);
    assert_eq!(visual("hi!", &[run(3, true)], None), ["!hi"]);
    // The first row holds the start of the text, with its trailing space after it.
    let hebrew = "אבג דהו";
    assert_eq!(
      visual(hebrew, &[run(hebrew.len(), true)], Some(40.)),
      ["גבא ", "והד"]
    );
    // An override reverses Latin text, and an isolate keeps its own order.
    let reversed = TextRun {
      embedding: marks(1, '\u{202e}'),
      ..run(3, false)
    };
    assert_eq!(
      visual("xabcy", &[run(1, false), reversed, run(1, false)], None),
      ["xcbay"]
    );
    let isolate = |id| TextRun {
      embedding: marks(id, '\u{2067}'),
      ..run(2, false)
    };
    assert_eq!(
      visual(
        "אבגד",
        &[isolate(1), isolate(1)].map(|run| TextRun { len: 4, ..run }),
        None
      ),
      ["דגבא"]
    );
    let parts = [isolate(1), isolate(2)].map(|run| TextRun { len: 4, ..run });
    assert_eq!(visual("אבגד", &parts, None), ["באדג"]);
  }

  #[test]
  fn plaintext_lines_have_their_own_direction() {
    let run = |len| TextRun {
      plaintext: true,
      ..run(len, false)
    };
    assert_eq!(visual("ab!", &[run(3)], None), ["ab!"]);
    assert_eq!(visual("1 אב!", &[run(7)], None), ["!בא 1"]);
    assert_eq!(crate::right_to_left("12, שa"), Some(true));
    assert_eq!(crate::right_to_left("12"), None);
  }

  #[test]
  fn ellipses_map_to_the_text_and_keep_whole_runs() {
    let system = TextSystem::new();
    let text = "abc שלום עולם 123 def العربية (hi)";
    let style = crate::TextStyle {
      font_family: "Helvetica".into(),
      white_space: crate::WhiteSpace::Nowrap,
      text_overflow: Some("…".into()),
      ..Default::default()
    };
    let bounds = crate::Rect::new(crate::Point::default(), crate::Size::new(px(150.), px(20.)));
    for rtl in [false, true] {
      let layout = crate::Paragraph::shape(
        text.into(),
        Rc::new([run(text.len(), rtl)]),
        &style,
        px(150.),
        bounds,
        &system,
        1.,
      );
      for x in 0..160 {
        let at = crate::Point::new(px(x as f32), px(10.));
        let (Ok(index) | Err(index)) = layout.index_at(at, true);
        assert!(text.is_char_boundary(index), "{index} at {x}");
      }
      assert!(layout.point_at(text.len()).is_none());
      let latin = crate::Paragraph::shape(
        "abc def".into(),
        Rc::new([run(7, rtl)]),
        &crate::TextStyle::default(),
        px(30.),
        bounds,
        &system,
        1.,
      );
      // Such rows have their spaces at the left, so ranges use their parts.
      assert_eq!(latin.reordered(), rtl);
    }
  }

  #[test]
  fn spaces_of_right_to_left_rows_hang_at_the_left() {
    let system = TextSystem::new();
    let text = "אבג   דהו";
    let wrapping = Wrapping {
      width: Some(px(40.)),
      ..Wrapping::default()
    };
    let lines = system.shape_text(text, px(16.), &[run(text.len(), true)], &wrapping);
    let line = &lines[0];
    assert_eq!(line.rows(), 2);
    assert!(line.parts(0, 6..9, px(0.), false).is_empty());
    let parts = line.parts(0, 6..9, px(0.), true);
    assert!(parts.len() == 1 && parts[0].right == px(0.) && parts[0].left < px(-5.));
    assert!(line.caret(7).unwrap().1 < px(0.));
  }

  #[test]
  fn inline_boxes_have_their_edges_at_their_outer_parts() {
    let system = TextSystem::new();
    let text = "ab גדה xy";
    // A box around `גדה xy`, with edges of 4 and 6 pixels and margins of 1 and 2.
    let boxed = |rtl: bool, width| {
      let edges = crate::InlineEdges {
        id: 7,
        rtl,
        ends: [true; 2],
        margins: [px(1.), px(2.)],
        edges: [px(4.), px(6.)],
      };
      let sides = if rtl { [8., 5.] } else { [5., 8.] };
      let runs = [
        run(3, false),
        TextRun {
          inset: sides.map(px),
          embedding: Some(Rc::new(crate::Embedding {
            marks: Vec::new(),
            boxes: vec![edges],
          })),
          ..run(text.len() - 3, false)
        },
      ];
      let wrapping = Wrapping {
        width,
        ..Wrapping::default()
      };
      system.shape_text(text, px(16.), &runs, &wrapping).remove(0)
    };
    let plain = system.shape_text(
      text,
      px(16.),
      &[run(text.len(), false)],
      &Wrapping::default(),
    );
    for rtl in [false, true] {
      let line = boxed(rtl, None);
      assert!((line.line.width - plain[0].line.width - px(13.)).abs() < px(0.01));
      let parts: Vec<_> = line.box_parts(7, |_| px(0.)).collect();
      let [(0, [left, right], [true, true])] = parts[..] else {
        panic!("{parts:?}");
      };
      assert!((left - plain[0].caret(3).unwrap().1 - px(1.)).abs() < px(0.01));
      assert!((line.line.width - right - px(2.)).abs() < px(0.01));
      // The box starts in the first row and ends in the second.
      let line = boxed(rtl, Some(px(70.)));
      let sides: Vec<_> = line
        .box_parts(7, |_| px(0.))
        .map(|part| (part.0, part.2))
        .collect();
      assert_eq!(sides, [(0, [!rtl, rtl]), (1, [rtl, !rtl])]);
    }
  }

  #[test]
  fn spaces_at_the_end_of_right_to_left_text_are_at_its_left() {
    assert_eq!(visual("ab  ", &[run(4, true)], None), ["  ab"]);
  }

  #[test]
  fn carets_stay_in_their_row() {
    let system = TextSystem::new();
    let text = "אבג דהו";
    let wrapping = Wrapping {
      width: Some(px(40.)),
      ..Wrapping::default()
    };
    let lines = system.shape_text(text, px(16.), &[run(text.len(), true)], &wrapping);
    let line = &lines[0];
    let x = |index| line.caret(index).unwrap();
    // The start of a row is at its right end.
    let (row, right) = x(0);
    assert!(row == 0 && right > x(2).1 && x(2).1 > x(4).1);
    let (row, first) = x(9);
    assert!(row == 1 && first < px(40.));
    assert_eq!(line.hit(1, px(0.5), true), Ok(text.len()));
    assert_eq!(line.hit(0, right - px(0.5), true), Ok(0));
  }
}
