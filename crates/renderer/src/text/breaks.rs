use super::{Hyphen, Line, TextWrap, WordBreak};
use crate::{Pixels, px};
use std::ops::Range;

/// `text-indent`: the room before the first row of the text, and before the other rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Hash)]
pub struct Indent {
  pub first: Pixels,
  pub rest: Pixels,
  /// The row after each line break is like the first one.
  pub each: bool,
}

impl Indent {
  pub(super) fn line(self, line: usize) -> Self {
    let [first, rest] = self.of(line);
    Self {
      first,
      rest,
      ..self
    }
  }

  /// The room before the first row of a line, and before its other rows.
  pub fn of(self, line: usize) -> [Pixels; 2] {
    match line == 0 || self.each {
      true => [self.first, self.rest],
      false => [self.rest; 2],
    }
  }
}

#[derive(Clone, Copy, Default, PartialEq, Hash)]
pub struct Wrapping {
  /// No breaks without a width.
  pub width: Option<Pixels>,
  pub max_lines: Option<usize>,
  pub indent: Indent,
  pub wrap: TextWrap,
  pub word_break: WordBreak,
}

#[derive(Clone, Copy)]
struct Chance {
  glyph: u32,
  x: f32,
  words_end: f32,
}

/// Floats beside a paragraph: `[top, bottom, left, right]` of each float in its content box, the line
/// height, the top of the next row, and `[left, right, top]` of each row so far.
pub struct Banded<'a> {
  pub floats: &'a [[f32; 4]],
  pub line: f32,
  pub y: f32,
  pub rows: Vec<[f32; 3]>,
}

impl Banded<'_> {
  /// Moves the next row below the floats on the left, bit 1, or the right, bit 2.
  pub(crate) fn clear(&mut self, sides: u8) {
    for &[_, bottom, left, right] in self.floats {
      if sides & 1 != 0 && left > 0. || sides & 2 != 0 && right > 0. {
        self.y = self.y.max(bottom);
      }
    }
  }

  /// The insets of a row at `y`, and the first float bottom below it.
  pub(crate) fn edges(&self, y: f32) -> ([f32; 2], f32) {
    self
      .floats
      .iter()
      .filter(|float| float[0] < y + self.line && float[1] > y)
      .fold(([0.; 2], f32::INFINITY), |(edges, low), float| {
        (
          [edges[0].max(float[2]), edges[1].max(float[3])],
          low.min(float[1]),
        )
      })
  }
}

/// Returns the glyph index that starts each row after the first.
pub(crate) fn breaks(
  line: &Line,
  text: &str,
  wrapping: &Wrapping,
  nowrap: &[Range<usize>],
) -> Vec<u32> {
  breaks_beside(line, text, wrapping, nowrap, &[], None)
}

impl Banded<'_> {
  /// The insets of the next `count` rows that don't wrap, like a `nowrap` line.
  pub(crate) fn rows(&mut self, count: usize) -> std::rc::Rc<[[f32; 3]]> {
    (0..count)
      .map(|_| {
        let [left, right] = self.edges(self.y).0;
        self.y += self.line;
        [left, right, self.y - self.line]
      })
      .collect()
  }
}

/// Like `breaks`, with rows that get shorter beside floats and move below them when no word fits.
pub(crate) fn breaks_beside(
  line: &Line,
  text: &str,
  wrapping: &Wrapping,
  nowrap: &[Range<usize>],
  hyphens: &[Hyphen],
  mut banded: Option<&mut Banded<'_>>,
) -> Vec<u32> {
  let width = wrapping.width.unwrap_or_default();
  let held = |index: usize| nowrap.iter().any(|range| range.contains(&index));
  // Like Chromium, preserved tabs turn balance and pretty off.
  let even = wrapping.wrap != TextWrap::Wrap
    && wrapping.max_lines.is_none()
    && banded.is_none()
    && !text.contains('\t');
  let mut edges = [0.; 2];
  if let Some(banded) = banded.as_deref_mut() {
    edges = banded.edges(banded.y).0;
    banded.rows.push([edges[0], edges[1], banded.y]);
  }
  let glyphs = &line.glyphs;
  let mut rows = Vec::new();
  let mut chances = Vec::new();
  if even {
    chances.push(Chance {
      glyph: 0,
      x: 0.,
      words_end: 0.,
    });
  }
  let mut chance: Option<(u32, Pixels)> = None;
  // The chance is in the last word, which starts the row.
  let mut alone = false;
  let (mut row_glyph, mut row_x) = (0, px(0.));
  let (mut started, mut words_end, mut previous_index) = (false, 0., 0);
  let (mut previous, mut before_previous) = ('\0', '\0');
  let whole = matches!(wrapping.word_break, WordBreak::Normal | WordBreak::KeepAll);
  let mut room = width - wrapping.indent.first - px(edges[0] + edges[1]);
  let mut marks = hyphens.iter().peekable();
  for (at, glyph) in glyphs.iter().enumerate() {
    let index = glyph.index as usize;
    let Some(ch) = text[index..].chars().next() else {
      continue;
    };
    if ch == '\n' {
      continue;
    }
    let joined = !nowrap.is_empty() && held(index) && held(previous_index);
    previous_index = index;
    let at = at as u32;
    let class = line_class(ch);
    let mut mark = None;
    if !hyphens.is_empty() {
      while (marks.next_if(|hyphen| (hyphen.at as usize) < index)).is_some() {}
      mark = marks.next_if(|hyphen| hyphen.at as usize == index);
    }
    if let Some(hyphen) = mark
      && !joined
      && (chance.is_none() || !hyphen.last || alone)
      && (chance.is_none() && whole || glyph.x + hyphen.line.width - row_x <= room + px(0.01))
    {
      chance = Some((at, glyph.x));
      alone = hyphen.last;
      if even {
        chances.push(Chance {
          glyph: at,
          x: glyph.x.0,
          words_end: words_end + hyphen.line.width.0,
        });
      }
    } else if started
            && ch != ' '
            && !joined
            // A zero width joiner holds the characters on both sides together.
            && !text[..index].ends_with('\u{200d}')
            && (previous == ' '
                || wrapping.word_break == WordBreak::BreakAll
                || class != CM
                    && can_break(
                        before_previous,
                        previous,
                        ch,
                        class,
                        &text[index + ch.len_utf8()..],
                    )
                    && !(wrapping.word_break == WordBreak::KeepAll
                        && LETTERS & 1 << line_class(previous) != 0
                        && LETTERS & 1 << class != 0))
    {
      chance = Some((at, glyph.x));
      alone = false;
      if even {
        chances.push(Chance {
          glyph: at,
          x: glyph.x.0,
          words_end,
        });
      }
    }
    started |= ch != ' ';
    let next_x = glyphs
      .get(at as usize + 1)
      .map_or(line.width, |next| next.x);
    if ch != ' ' {
      words_end = next_x.0;
    }
    // Like Chrome, a row where the first word doesn't fit beside the floats moves below the first of them.
    while ch != ' '
      && chance.is_none()
      && edges != [0.; 2]
      && next_x - row_x > room + px(0.01)
      && let Some(banded) = banded.as_deref_mut()
    {
      let y = banded.edges(banded.y).1;
      banded.y = y;
      room += px(edges[0] + edges[1]);
      edges = banded.edges(y).0;
      room -= px(edges[0] + edges[1]);
      *banded.rows.last_mut().unwrap() = [edges[0], edges[1], y];
    }
    if ch != ' '
      && next_x - row_x > room + px(0.01)
      && at > row_glyph
      && (chance.is_some() || !whole && !joined)
    {
      if wrapping
        .max_lines
        .is_some_and(|max| rows.len() >= max.saturating_sub(1))
      {
        break;
      }
      (row_glyph, row_x) = chance.take().unwrap_or((at, glyph.x));
      rows.push(row_glyph);
      if let Some(banded) = banded.as_deref_mut() {
        banded.y += banded.line;
        edges = banded.edges(banded.y).0;
        banded.rows.push([edges[0], edges[1], banded.y]);
      }
      room = width - wrapping.indent.rest - px(edges[0] + edges[1]);
    }
    if class != CM {
      (before_previous, previous) = (previous, ch);
    }
  }
  if let Some(banded) = banded {
    banded.y += banded.line;
  }
  if !even || rows.is_empty() {
    return rows;
  }
  chances.push(Chance {
    glyph: row_glyph,
    x: line.width.0,
    words_end,
  });
  even_out(rows, &chances, width.0, line.font_size.0, wrapping)
}

fn even_out(
  rows: Vec<u32>,
  chances: &[Chance],
  max: f32,
  font_size: f32,
  wrapping: &Wrapping,
) -> Vec<u32> {
  let (wrap, indent) = (wrapping.wrap, wrapping.indent);
  let last = chances.len() - 1;
  let span = |from: usize, to: usize| {
    let indent = if from == 0 { indent.first } else { indent.rest };
    chances[to].words_end - chances[from].x + indent.0
  };
  let mut greedy = vec![0];
  for row in &rows {
    match chances.iter().position(|chance| chance.glyph == *row) {
      Some(at) => greedy.push(at),
      None => return rows,
    }
  }
  greedy.push(last);
  let count = greedy.len() - 1;
  let ideal = if wrap == TextWrap::Balance {
    greedy
      .windows(2)
      .map(|pair| span(pair[0], pair[1]))
      .sum::<f32>()
      / count as f32
  } else {
    0.
  };
  let pretty = wrap == TextWrap::Pretty;
  // Like Chromium's ScoreLineBreaker: pretty acts only when no word overflows and the greedy last row
  // is one word shorter than a third of the row. It moves only the breaks of the last 4 rows, and
  // needs 3 break chances there.
  let kept = if pretty { count.saturating_sub(4) } else { 0 };
  let start = greedy[kept];
  if pretty
    && (count < 2
      || greedy[count - 1] + 1 != last
      || span(greedy[count - 1], last) >= max / 3.
      || last - start < 4
      || (1..=last).any(|at| span(at - 1, at) > max))
  {
    return rows;
  }
  // Balance keeps the row count of the greedy result: a layer per row count. Pretty needs only "reached".
  let exact = wrap == TextWrap::Balance && count <= 12;
  let layers = if exact { count + 1 } else { 2 };
  let cell = |at: usize, layer: usize| at * layers + layer;
  let mut cost = vec![f32::INFINITY; chances.len() * layers];
  let mut came = vec![0; chances.len() * layers];
  cost[cell(start, 0)] = 0.;
  let mut first = start;
  // Chromium's penalties: a cost per row, and an orphan cost for a last row of one word.
  let line_penalty = 4. * max * font_size;
  for to in start + 1..=last {
    while first < to && span(first, to) > max {
      first += 1;
    }
    for from in first..to {
      let width = span(from, to);
      let row = match (pretty, to == last) {
        (true, true) if from + 1 == to => 50000. + line_penalty,
        (true, true) => line_penalty,
        (true, false) => (max - width).powi(2) + line_penalty,
        _ => 100. * ((width - ideal) / 15.).abs().powi(3),
      };
      for layer in 0..if exact { layers - 1 } else { layers } {
        let next = if exact { layer + 1 } else { 1 };
        let total = cost[cell(from, layer)] + row;
        let current = cost[cell(to, next)];
        // On a tie, pretty keeps the earlier rows long, as Chromium does.
        if total < current
          || pretty && total.is_finite() && total == current
          || exact && total.is_finite() && total - current <= current * 1e-6
        {
          cost[cell(to, next)] = total;
          came[cell(to, next)] = from;
        }
      }
    }
  }
  let mut layer = layers - 1;
  if !cost[cell(last, layer)].is_finite() {
    return rows;
  }
  let mut picked = Vec::new();
  let mut at = came[cell(last, layer)];
  while at > start {
    picked.push(chances[at].glyph);
    layer = if exact { layer - 1 } else { 1 };
    at = came[cell(at, layer)];
  }
  // Like Chromium, pretty falls back to the greedy rows when it would change their count.
  if pretty && picked.len() != count - 1 - kept {
    return rows;
  }
  picked.extend(rows[..kept].iter().rev());
  picked.reverse();
  picked
}

// UAX #14 line break classes, merged where Chrome breaks the same. OPW is an East Asian wide OP.
// SA (Thai, Lao, Myanmar, Khmer) needs a dictionary, so it breaks between its letters as a fallback.
const AL: u8 = 0;
const NU: u8 = 1;
const OP: u8 = 2;
const OPW: u8 = 3;
const CL: u8 = 4;
const CP: u8 = 5;
const QU: u8 = 6;
const GL: u8 = 7;
const NS: u8 = 8;
const EX: u8 = 9;
const SY: u8 = 10;
const IS: u8 = 11;
const PR: u8 = 12;
const PO: u8 = 13;
const ID: u8 = 14;
const IN: u8 = 15;
const HY: u8 = 16;
const BA: u8 = 17;
const B2: u8 = 18;
const ZW: u8 = 19;
const CM: u8 = 20;
const CB: u8 = 21;
const SA: u8 = 22;

const fn set(classes: &[u8]) -> u32 {
  let (mut set, mut at) = (0, 0);
  while at < classes.len() {
    set |= 1 << classes[at];
    at += 1;
  }
  set
}

const LETTERS: u32 = set(&[AL, NU, ID]);
/// Classes with no break before them, and with no break after them.
const NO_BREAK_BEFORE: u32 = set(&[CL, CP, EX, IS, SY, QU, GL, NS, IN, HY, BA, CM, ZW]);
const NO_BREAK_AFTER: u32 = set(&[OP, OPW, QU, GL]);

fn line_class(c: char) -> u8 {
  match c {
    'a'..='z' | 'A'..='Z' => AL,
    '0'..='9' => NU,
    '(' | '[' | '{' => OP,
    ')' | ']' => CP,
    '}' => CL,
    '-' => HY,
    '!' | '?' => EX,
    '.' | ',' | ':' | ';' => IS,
    '/' => SY,
    '$' | '+' | '\\' => PR,
    '%' => PO,
    '"' | '\'' => QU,
    '|' => BA,
    '\0'..='\u{7f}' => AL,
    '\u{a0}' | '\u{ad}' | '\u{2007}' | '\u{2011}' | '\u{202f}' | '\u{2060}' | '\u{feff}' => GL,
    '\u{200b}' => ZW,
    '\u{fffc}' => CB,
    '\u{300}'..='\u{36f}'
    | '\u{e31}'
    | '\u{e34}'..='\u{e3a}'
    | '\u{e47}'..='\u{e4e}'
    | '\u{1ab0}'..='\u{1aff}'
    | '\u{1dc0}'..='\u{1dff}'
    | '\u{200c}'
    | '\u{200d}'
    | '\u{20d0}'..='\u{20ff}'
    | '\u{fe00}'..='\u{fe0f}'
    | '\u{fe20}'..='\u{fe2f}'
    | '\u{1f3fb}'..='\u{1f3ff}'
    | '\u{e0020}'..='\u{e01ef}' => CM,
    '\u{a1}' | '\u{bf}' | '\u{201a}' | '\u{201e}' | '\u{2045}' | '\u{207d}' | '\u{208d}' => OP,
    '\u{2046}' | '\u{207e}' | '\u{208e}' => CL,
    '\u{ab}'
    | '\u{bb}'
    | '\u{2018}'
    | '\u{2019}'
    | '\u{201b}'..='\u{201d}'
    | '\u{201f}'
    | '\u{2039}'
    | '\u{203a}' => QU,
    '\u{2014}' | '\u{2e3a}' | '\u{2e3b}' => B2,
    '\u{5be}'
    | '\u{2000}'..='\u{2006}'
    | '\u{2008}'..='\u{200a}'
    | '\u{2010}'
    | '\u{2012}'
    | '\u{2013}'
    | '\u{2027}'
    | '\u{205f}'
    | '\u{3000}' => BA,
    '\u{2024}'..='\u{2026}' | '\u{22ef}' | '\u{fe19}' => IN,
    '\u{61b}' | '\u{61f}' | '\u{6d4}' | '\u{fe56}' | '\u{fe57}' | '\u{ff01}' | '\u{ff1f}' => EX,
    '\u{37e}' | '\u{60c}' | '\u{60d}' | '\u{2044}' | '\u{fe10}' | '\u{fe13}' | '\u{fe14}' => IS,
    '\u{a3}'..='\u{a5}'
    | '\u{b1}'
    | '\u{2116}'
    | '\u{2212}'
    | '\u{2213}'
    | '\u{20a0}'..='\u{20a6}'
    | '\u{20a8}'..='\u{20cf}' => PR,
    '\u{a2}'
    | '\u{b0}'
    | '\u{2030}'..='\u{2037}'
    | '\u{20a7}'
    | '\u{2103}'
    | '\u{2109}'
    | '\u{ff05}'
    | '\u{ffe0}' => PO,
    '\u{3001}' | '\u{3002}' | '\u{ff09}' | '\u{ff0c}' | '\u{ff0e}' | '\u{ff3d}' | '\u{ff5d}'
    | '\u{ff60}' | '\u{ff61}' | '\u{ff63}' | '\u{ff64}' | '\u{301e}' | '\u{301f}' => CL,
    '\u{ff08}' | '\u{ff3b}' | '\u{ff5b}' | '\u{ff5f}' | '\u{ff62}' | '\u{301d}' => OPW,
    '\u{3008}'..='\u{3011}' | '\u{3014}'..='\u{301b}' => {
      if c as u32 & 1 == 0 {
        OPW
      } else {
        CL
      }
    }
    '\u{3005}'
    | '\u{301c}'
    | '\u{303b}'
    | '\u{309b}'..='\u{309e}'
    | '\u{30a0}'
    | '\u{30fb}'..='\u{30fe}'
    | '\u{31f0}'..='\u{31ff}'
    | '\u{203c}'
    | '\u{203d}'
    | '\u{2047}'..='\u{2049}'
    | '\u{ff1a}'
    | '\u{ff1b}'
    | '\u{ff65}'
    | '\u{ff67}'..='\u{ff70}' => NS,
    // Small kana.
    '\u{3041}' | '\u{3043}' | '\u{3045}' | '\u{3047}' | '\u{3049}' | '\u{3063}' | '\u{3083}'
    | '\u{3085}' | '\u{3087}' | '\u{308e}' | '\u{3095}' | '\u{3096}' | '\u{30a1}' | '\u{30a3}'
    | '\u{30a5}' | '\u{30a7}' | '\u{30a9}' | '\u{30c3}' | '\u{30e3}' | '\u{30e5}' | '\u{30e7}'
    | '\u{30ee}' | '\u{30f5}' | '\u{30f6}' => NS,
    '\u{660}'..='\u{669}' | '\u{6f0}'..='\u{6f9}' => NU,
    '\u{e00}'..='\u{eff}' | '\u{1000}'..='\u{109f}' | '\u{1780}'..='\u{17ff}' => SA,
    '\u{1100}'..='\u{115f}'
    | '\u{2e80}'..='\u{2fff}'
    | '\u{3003}'..='\u{33ff}'
    | '\u{3400}'..='\u{4dbf}'
    | '\u{4e00}'..='\u{9fff}'
    | '\u{a000}'..='\u{a4cf}'
    | '\u{ac00}'..='\u{d7af}'
    | '\u{f900}'..='\u{faff}'
    | '\u{fe30}'..='\u{fe4f}'
    | '\u{ff00}'..='\u{ffef}'
    | '\u{1f000}'..='\u{1faff}'
    | '\u{20000}'..='\u{3fffd}' => ID,
    _ => AL,
  }
}

/// East Asian wide and fullwidth characters, roughly.
fn wide(c: char) -> bool {
  matches!(c,
        '\u{1100}'..='\u{115f}' | '\u{2e80}'..='\u{a4cf}' | '\u{ac00}'..='\u{d7a3}' | '\u{f900}'..='\u{faff}'
        | '\u{fe30}'..='\u{fe4f}' | '\u{ff00}'..='\u{ff60}' | '\u{ffe0}'..='\u{ffe6}' | '\u{1f300}'..='\u{1faff}'
        | '\u{20000}'..='\u{3fffd}')
}

/// Whether a line can break between `previous` and `ch` (of `class`), with no spaces between them.
fn can_break(before_previous: char, previous: char, ch: char, class: u8, after: &str) -> bool {
  // Like Chrome, pairs of ASCII characters break only after `-` and `?`, and before an opening bracket after some symbols.
  if previous.is_ascii() && ch.is_ascii() {
    let ch = ch as u8;
    return match previous {
      // A `-` before a digit is a minus sign unless a letter or a digit comes before it.
      '-' if ch.is_ascii_digit() => before_previous.is_ascii_alphanumeric(),
      '-' => !b"!$),./:;?]}".contains(&ch),
      '?' => !b"!\"'),./:;?]}".contains(&ch),
      _ => b"([{<".contains(&ch) && b"!\"#%&)*+,-.:;=>?\\]|}~".contains(&(previous as u8)),
    };
  }
  let before = line_class(previous);
  if matches!(before, ZW | CB) || class == CB {
    return true;
  }
  const NOT_QU: u32 = !(1 << QU);
  if NO_BREAK_BEFORE & NOT_QU & 1 << class != 0 || NO_BREAK_AFTER & NOT_QU & 1 << before != 0 {
    return class == GL && matches!(before, HY | BA);
  }
  // Like Chrome (UAX #14 LB19a), an opening quote can start a line and a closing quote can end one only
  // between East Asian characters.
  if class == QU || before == QU {
    let opening = matches!(
      ch,
      '\u{2018}' | '\u{201c}' | '\u{ab}' | '\u{2039}' | '\u{201b}' | '\u{201f}'
    );
    let closing = matches!(previous, '\u{2019}' | '\u{201d}' | '\u{bb}' | '\u{203a}');
    return opening && wide(previous) && after.chars().next().is_some_and(wide)
      || closing && wide(ch) && wide(before_previous);
  }
  let kept = match class {
    AL => const { set(&[AL, NU, PR, PO, IS, CP, SA]) },
    SA => const { set(&[AL, NU, PR, PO, IS, CP]) },
    NU => const { set(&[AL, NU, PR, PO, IS, SY, HY, CP, SA]) },
    OP => const { set(&[AL, NU, SA]) },
    PR => const { set(&[AL, NU, SA]) },
    PO => const { set(&[AL, NU, ID, SA]) },
    ID => const { set(&[PR]) },
    B2 => const { set(&[B2]) },
    _ => 0,
  };
  kept & 1 << before == 0
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::text::Glyph;

  /// A line of 10 px wide glyphs, one per character.
  fn line(text: &str) -> Line {
    let mut line = Line {
      glyphs: text
        .char_indices()
        .enumerate()
        .map(|(at, (index, _))| Glyph {
          x: px(at as f32 * 10.),
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
      ..Default::default()
    };
    line.sort_check();
    line
  }

  fn wrapping(width: f32, wrap: TextWrap) -> Wrapping {
    Wrapping {
      width: Some(px(width)),
      wrap,
      ..Default::default()
    }
  }

  #[test]
  fn rows_beside_floats_shorten_and_move_below_them_like_chrome() {
    // 10px glyphs and 10px rows. A left float of 30px is 15px tall, a right one of 20px 35px tall.
    let floats = [[0., 15., 30., 0.], [0., 35., 0., 20.]];
    let mut banded = Banded {
      floats: &floats,
      line: 10.,
      y: 0.,
      rows: Vec::new(),
    };
    let text = "ab cd ef gh ij";
    let rows = breaks_beside(
      &line(text),
      text,
      &wrapping(100., TextWrap::Wrap),
      &[],
      &[],
      Some(&mut banded),
    );
    // Rows of 50px, 50px, 80px, then 100px.
    assert_eq!(rows, [6, 12]);
    assert_eq!(
      banded.rows,
      [[30., 20., 0.], [30., 20., 10.], [0., 20., 20.]]
    );
    assert_eq!(banded.y, 30.);
    banded.clear(1);
    assert_eq!(banded.y, 30.);
    banded.clear(2);
    assert_eq!(banded.y, 35.);
    // A word wider than the room beside the floats moves below the first float that ends.
    let mut banded = Banded {
      floats: &floats,
      line: 10.,
      y: 0.,
      rows: Vec::new(),
    };
    let text = "abcdefgh";
    let rows = breaks_beside(
      &line(text),
      text,
      &wrapping(100., TextWrap::Wrap),
      &[],
      &[],
      Some(&mut banded),
    );
    assert!(rows.is_empty());
    assert_eq!(banded.rows, [[0., 20., 15.]]);
    assert_eq!(banded.y, 25.);
  }

  #[test]
  fn objects_break_on_both_sides_and_nowrap_runs_hold_together() {
    let find = |text: &str, nowrap: &[Range<usize>]| {
      breaks(&line(text), text, &wrapping(0., TextWrap::Wrap), nowrap)
    };
    assert_eq!(find("ab\u{fffc}cd", &[]), [2, 3]);
    assert_eq!(find("ab cd ef", &[]), [3, 6]);
    assert_eq!(find("ab cd ef", std::slice::from_ref(&(0..5))), [6]);
  }

  #[test]
  fn hyphens_break_after_inside_words() {
    let text = "ab-cd x-12 -ef";
    assert_eq!(
      breaks(&line(text), text, &wrapping(0., TextWrap::Wrap), &[]),
      [3, 6, 8, 11, 12]
    );
  }

  #[test]
  fn breaks_follow_line_break_classes_like_chrome() {
    // Each text with the breaks that Chrome finds in a box of zero width.
    for (text, starts) in [
      ("pr-312", &[3][..]),
      ("a--2", &[2]),
      (".-2", &[]),
      ("x(-1)", &[]),
      ("é-1", &[]),
      ("a?1", &[2]),
      ("a/b+c<d>e&f|g*h", &[]),
      ("kj@nasa.gov>,", &[]),
      ("1,000.5 $12.50 50%", &[8, 15]),
      ("a中b", &[1, 2]),
      ("中文。中", &[1, 3]),
      ("「中」a", &[3]),
      ("a—b a…b a–1", &[1, 2, 4, 6, 8, 10]),
      ("“a” «b» ‘c’", &[4, 8]),
      ("a\u{200b}b", &[2]),
      ("€(a", &[1]),
      ("中“文”中 中«文»中", &[1, 4, 6, 7, 10]),
      ("中\"文\"中 中“a”中 「“文”」", &[6, 12]),
      ("a\u{200d}中 中\u{200d}中", &[4]),
    ] {
      assert_eq!(
        breaks(&line(text), text, &wrapping(0., TextWrap::Wrap), &[]),
        starts,
        "{text}"
      );
    }
  }

  #[test]
  fn opening_brackets_stay_with_the_word_before_them_like_chrome() {
    let find = |text: &str| breaks(&line(text), text, &wrapping(0., TextWrap::Wrap), &[]);
    assert_eq!(find("f(el).x a[0] (b)"), [8, 13]);
    assert_eq!(find("$(x) a([{b}])"), [5]);
    assert_eq!(find("a)(b x.(y) ?(a)"), [2, 5, 7, 11, 12]);
  }

  #[test]
  fn keep_all_holds_ideographs_together_like_chrome() {
    let text = "ab 这是一个 中文";
    let mut wrapping = wrapping(0., TextWrap::Wrap);
    assert_eq!(
      breaks(&line(text), text, &wrapping, &[]),
      [3, 4, 5, 6, 8, 9]
    );
    wrapping.word_break = WordBreak::KeepAll;
    assert_eq!(breaks(&line(text), text, &wrapping, &[]), [3, 8]);
  }

  #[test]
  fn balance_and_pretty_even_out_lines() {
    let text = ["aaaa"; 9].join(" ");
    let line = line(&text);
    for (wrap, starts) in [
      (TextWrap::Wrap, [20, 40]),
      (TextWrap::Balance, [15, 30]),
      // Like Chromium: the greedy last row holds one word, so the last rows take a word from the rows above.
      (TextWrap::Pretty, [20, 35]),
    ] {
      assert_eq!(
        breaks(&line, &text, &wrapping(200., wrap), &[]),
        starts,
        "{wrap:?}"
      );
    }
  }

  #[test]
  fn pretty_acts_only_where_chromium_does() {
    let at = |text: &str, width: f32| {
      let pretty = breaks(&line(text), text, &wrapping(width, TextWrap::Pretty), &[]);
      (
        breaks(&line(text), text, &wrapping(width, TextWrap::Wrap), &[]),
        pretty,
      )
    };
    // The last row of 70px is not shorter than a third of 200px, but is of 220px.
    let text = format!("{} aaaaaaa", ["aaaa"; 8].join(" "));
    assert_eq!(at(&text, 200.), (vec![20, 40], vec![20, 40]));
    assert_eq!(at(&text, 220.), (vec![20, 40], vec![20, 35]));
    // Preserved tabs turn pretty off.
    let tabbed = text.replacen(' ', "\t", 1);
    let (wrap, pretty) = at(&tabbed, 220.);
    assert_eq!(pretty, wrap);
    // A one-word last row costs less than the 250px gap that moving a word down leaves.
    let text = format!("{} {} {} a", "a".repeat(39), "a".repeat(15), "a".repeat(24));
    assert_eq!(at(&text, 400.), (vec![40, 81], vec![40, 81]));
    // Fewer than 3 break chances in the last rows.
    assert_eq!(at("aaaaaaaa aaaaaaaaaa b", 200.), (vec![20], vec![20]));
  }

  #[test]
  fn word_breaks_split_words_and_clamps_stop_early() {
    let text = "abcdefgh";
    let line = line(text);
    let mut wrapping = wrapping(30., TextWrap::Wrap);
    assert!(breaks(&line, text, &wrapping, &[]).is_empty());
    wrapping.word_break = WordBreak::BreakAll;
    assert_eq!(breaks(&line, text, &wrapping, &[]), [3, 6]);
    wrapping.max_lines = Some(2);
    assert_eq!(breaks(&line, text, &wrapping, &[]), [3]);
    wrapping.indent.first = px(10.);
    wrapping.max_lines = None;
    assert_eq!(breaks(&line, text, &wrapping, &[]), [2, 5]);
  }
}
