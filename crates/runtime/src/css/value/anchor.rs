use crate::css::properties::{Prop, PropSet, Style};
use crate::css::value::grammar::syntax_matches;
use crate::css::value::tokens::components;

/// An `anchor()` or `anchor-size()` function. `side` is in lowercase, and empty for `anchor-size()` without a size.
pub(crate) struct Function<'a> {
  pub(crate) size: bool,
  pub(crate) name: Option<&'a str>,
  pub(crate) side: String,
  pub(crate) fallback: Option<&'a str>,
}

fn dashed(value: &str) -> bool {
  value.starts_with("--") && syntax_matches("<custom-ident>", value)
}

impl<'a> Function<'a> {
  fn parse(size: bool, args: &'a str) -> Option<Self> {
    let head = components(args, ',').first().copied().unwrap_or("");
    // Like Chrome, a fallback can't be empty.
    let fallback = match args[head.len()..]
      .trim_start()
      .strip_prefix(',')
      .map(str::trim)
    {
      Some("") => return None,
      fallback => fallback,
    };
    let parts = components(head.trim(), ' ');
    let (name, side) = match parts[..] {
      [] | [""] => (None, ""),
      [name] if dashed(name) => (Some(name), ""),
      [side] => (None, side),
      [name, side] if dashed(name) => (Some(name), side),
      [side, name] if dashed(name) => (Some(name), side),
      _ => return None,
    };
    let side = side.to_ascii_lowercase();
    let valid = match size {
      true => matches!(
        &*side,
        "" | "width" | "height" | "block" | "inline" | "self-block" | "self-inline"
      ),
      false => {
        matches!(
          &*side,
          "inside"
            | "outside"
            | "top"
            | "left"
            | "right"
            | "bottom"
            | "start"
            | "end"
            | "self-start"
            | "self-end"
            | "center"
        ) || syntax_matches("<percentage>", &side)
      }
    };
    valid.then_some(Self {
      size,
      name,
      side,
      fallback,
    })
  }

  pub(crate) fn text(&self) -> String {
    let mut words = Vec::from_iter(self.name);
    words.extend((!self.side.is_empty()).then_some(&*self.side));
    format!(
      "{}({}{})",
      if self.size { "anchor-size" } else { "anchor" },
      words.join(" "),
      self
        .fallback
        .map_or(String::new(), |fallback| format!(", {fallback}"))
    )
  }
}

pub(crate) fn has_function(value: &str) -> bool {
  starts(value).next().is_some()
}

const PLACES: PropSet = PropSet::new(&[
  Prop::TOP,
  Prop::RIGHT,
  Prop::BOTTOM,
  Prop::LEFT,
  Prop::WIDTH,
  Prop::HEIGHT,
  Prop::MIN_WIDTH,
  Prop::MIN_HEIGHT,
  Prop::MAX_WIDTH,
  Prop::MAX_HEIGHT,
  Prop::MARGIN_TOP,
  Prop::MARGIN_RIGHT,
  Prop::MARGIN_BOTTOM,
  Prop::MARGIN_LEFT,
]);

/// Whether anchors or position options place a box with this style.
pub(crate) fn positions(style: &Style) -> bool {
  let set = |prop: Prop| {
    style
      .get(prop)
      .filter(|value| !matches!(*value, "none" | "normal"))
  };
  let placed = [
    Prop::POSITION_AREA,
    Prop::POSITION_ANCHOR,
    Prop::POSITION_TRY_FALLBACKS,
  ];
  set(Prop::POSITION).is_some_and(|value| matches!(value, "absolute" | "fixed"))
    && (placed.into_iter().any(|prop| set(prop).is_some())
      || [Prop::ALIGN_SELF, Prop::JUSTIFY_SELF]
        .into_iter()
        .any(|prop| set(prop).is_some_and(|value| value.ends_with("anchor-center"))))
    || style.values_in(&PLACES).any(has_function)
}

// Where the anchor functions of `value` start, and whether they are `anchor-size()`.
fn starts(value: &str) -> impl Iterator<Item = (usize, bool)> + '_ {
  let bytes = value.as_bytes();
  (0..bytes.len()).filter_map(move |at| {
    let is = |name: &str| {
      bytes
        .get(at..at + name.len())
        .is_some_and(|text| text.eq_ignore_ascii_case(name.as_bytes()))
    };
    if !matches!(bytes[at], b'a' | b'A')
      || at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || matches!(bytes[at - 1], b'-' | b'_'))
    {
      return None;
    }
    match is("anchor(") {
      true => Some((at, false)),
      false => is("anchor-size(").then_some((at, true)),
    }
  })
}

/// Puts the text that `with` gives in the place of each anchor function of `value`, or `None` when a function is
/// invalid or has none.
pub(crate) fn replace(
  value: &str,
  with: &mut dyn FnMut(&Function) -> Option<String>,
) -> Option<String> {
  let mut out = String::new();
  let mut rest = value;
  while let Some((at, size)) = starts(rest).next() {
    let open = at + if size { "anchor-size(" } else { "anchor(" }.len();
    let mut depth = 1;
    // The end of a value closes its functions.
    let close = rest.as_bytes()[open..]
      .iter()
      .position(|&byte| {
        depth += i32::from(byte == b'(') - i32::from(byte == b')');
        depth == 0
      })
      .map_or(rest.len(), |length| open + length);
    out.push_str(&rest[..at]);
    out.push_str(&with(&Function::parse(size, &rest[open..close])?)?);
    rest = rest.get(close + 1..).unwrap_or_default();
  }
  out.push_str(rest);
  Some(out)
}

/// The text of `value` with its anchor functions as Chrome writes them, or `None` for an invalid one.
pub(crate) fn canonical(name: &str, value: &str) -> Option<String> {
  let inset = matches!(name, "top" | "right" | "bottom" | "left") || name.starts_with("inset");
  let sized =
    Prop::find(name).is_some_and(|prop| DESCRIPTORS.contains(prop)) && !name.ends_with("self");
  if !sized || value.contains("fit-content(") {
    return None;
  }
  replace(value, &mut |function| {
    if !function.size && !inset {
      return None;
    }
    let fallback = match function.fallback {
      Some(fallback) => {
        let fallback = canonical(name, fallback)?;
        let plain = replace(&fallback, &mut |_| Some("1px".into()))?;
        if !syntax_matches("<length-percentage>", &plain) {
          return None;
        }
        Some(fallback)
      }
      None => None,
    };
    Some(
      Function {
        fallback: fallback.as_deref(),
        side: function.side.clone(),
        ..*function
      }
      .text(),
    )
  })
}

/// The tracks of one axis of a `position-area`, as the grid lines at its start and end: 0 and 3 are the edges of the
/// containing block, 1 and 2 those of the anchor.
pub(crate) type Tracks = [u8; 2];

#[derive(Clone, Copy, PartialEq)]
enum Group {
  X,
  Y,
  Block,
  Inline,
  OwnBlock,
  OwnInline,
  Any,
  Own,
  Both,
}

// The keywords without `span-`, in pairs of a start and an end. A flip of x and y swaps neighbor pairs.
const WORDS: [(&str, Group); 24] = [
  ("left", Group::X),
  ("right", Group::X),
  ("top", Group::Y),
  ("bottom", Group::Y),
  ("x-start", Group::X),
  ("x-end", Group::X),
  ("y-start", Group::Y),
  ("y-end", Group::Y),
  ("self-x-start", Group::X),
  ("self-x-end", Group::X),
  ("self-y-start", Group::Y),
  ("self-y-end", Group::Y),
  ("block-start", Group::Block),
  ("block-end", Group::Block),
  ("inline-start", Group::Inline),
  ("inline-end", Group::Inline),
  ("self-block-start", Group::OwnBlock),
  ("self-block-end", Group::OwnBlock),
  ("self-inline-start", Group::OwnInline),
  ("self-inline-end", Group::OwnInline),
  ("start", Group::Any),
  ("end", Group::Any),
  ("self-start", Group::Own),
  ("self-end", Group::Own),
];

// The place of a keyword in `WORDS`.
fn word_index(word: &str) -> Option<usize> {
  let name = word.strip_prefix("span-").unwrap_or(word);
  WORDS.iter().position(|(found, _)| *found == name)
}

// The group of a `position-area` keyword, its tracks, and whether the direction of its box or its containing block
// (or, for a `self` keyword, its own) turns them.
fn keyword(word: &str) -> Option<(Group, Tracks, bool)> {
  Some(match word {
    "span-all" => (Group::Both, [0, 3], false),
    "center" => (Group::Both, [1, 2], false),
    word => {
      let at = word_index(word)?;
      let tracks = match (at % 2, word.starts_with("span-")) {
        (0, false) => [0, 1],
        (0, true) => [0, 2],
        (_, false) => [2, 3],
        (_, true) => [1, 3],
      };
      (WORDS[at].1, tracks, at > 3)
    }
  })
}

// The keywords of a `position-area` in the order that Chrome writes them, x or block first.
fn area_words(value: &str) -> Option<String> {
  let lower = value.to_ascii_lowercase();
  let mut words = lower.split_ascii_whitespace();
  let mut one = words.next()?;
  let group = |word: &str| Some(keyword(word)?.0);
  let first = group(one)?;
  let Some(mut two) = words.next() else {
    return Some(one.to_owned());
  };
  use Group::*;
  let swap = match (first, group(two)?) {
    _ if words.next().is_some() => return None,
    (X, Y) | (Block, Inline) | (OwnBlock, OwnInline) | (Any, Any) | (Own, Own) => false,
    (Y, X) | (Inline, Block) | (OwnInline, OwnBlock) => true,
    (Both, X | Block | OwnBlock) | (Y | Inline | OwnInline, Both) => true,
    (Both, _) | (_, Both) => false,
    _ => return None,
  };
  if swap {
    (one, two) = (two, one);
  }
  let lone = |word: &str| group(word).is_some_and(|found| !matches!(found, Any | Own | Both));
  Some(if one == two || two == "span-all" && lone(one) {
    one.to_owned()
  } else if one == "span-all" && lone(two) {
    two.to_owned()
  } else {
    format!("{one} {two}")
  })
}

/// The tracks of a `position-area` on the x and y axes. `rtl` tells whether the containing block and the box itself
/// are right-to-left.
pub(crate) fn area(value: &str, rtl: [bool; 2]) -> Option<[Tracks; 2]> {
  let text = area_words(value)?;
  let words: Vec<&str> = text.split(' ').collect();
  let found: Vec<_> = words.iter().filter_map(|word| keyword(word)).collect();
  let turn = |tracks: Tracks| [3 - tracks[1], 3 - tracks[0]];
  let own = |word: &str| word.starts_with("self-") || word.starts_with("span-self-");
  let x = |index: usize| {
    let (_, tracks, logical) = found[index];
    match logical && rtl[usize::from(own(words[index]))] {
      true => turn(tracks),
      false => tracks,
    }
  };
  let y = |index: usize| found[index].1;
  use Group::*;
  Some(match found.len() {
    1 => match found[0].0 {
      X | Inline | OwnInline => [x(0), [0, 3]],
      Y | Block | OwnBlock => [[0, 3], y(0)],
      Any | Own | Both => [x(0), y(0)],
    },
    _ => match (found[0].0, found[1].0) {
      (X, _) | (_, Y) => [x(0), y(1)],
      _ => [x(1), y(0)],
    },
  })
}

const FLIPS: [&str; 5] = [
  "flip-block",
  "flip-inline",
  "flip-start",
  "flip-x",
  "flip-y",
];

// The options as Chrome writes them: an area, or a name and then flips.
fn fallbacks(value: &str) -> Option<String> {
  let mut out = String::new();
  for item in components(value, ',') {
    let item = item.trim();
    if !out.is_empty() {
      out.push_str(", ");
    }
    if let Some(area) = area_words(item) {
      out.push_str(&area);
      continue;
    }
    let (mut name, mut flips) = ("", String::new());
    for word in item.split_ascii_whitespace() {
      let lower = word.to_ascii_lowercase();
      if FLIPS.contains(&&*lower) && !flips.split(' ').any(|flip| flip == lower) {
        flips.push(' ');
        flips.push_str(&lower);
      } else if name.is_empty() && dashed(word) {
        name = word;
      } else {
        return None;
      }
    }
    out.push_str(name);
    out.push_str(if name.is_empty() {
      flips.trim_start()
    } else {
      &flips
    });
    if name.is_empty() && flips.is_empty() {
      return None;
    }
  }
  (!out.is_empty()).then_some(out)
}

const ORDERS: [&str; 5] = [
  "normal",
  "most-width",
  "most-height",
  "most-block-size",
  "most-inline-size",
];

/// Splits `position-try` into its order and its fallbacks.
pub(crate) fn position_try(value: &str) -> Option<(String, String)> {
  let (first, rest) = value.split_once([' ', '\t', '\n']).unwrap_or((value, ""));
  let (order, rest) = match ORDERS
    .iter()
    .find(|order| first.eq_ignore_ascii_case(order))
  {
    Some(order) => (*order, rest.trim()),
    None => ("normal", value),
  };
  Some((order.into(), specified("position-try-fallbacks", rest)?))
}

/// Chrome's text of a value of a property of anchor positioning, or `None` when it is invalid.
pub(crate) fn specified(name: &str, value: &str) -> Option<String> {
  let value = value.trim();
  let lower = value.to_ascii_lowercase();
  let names = || {
    let names = components(value, ',');
    let names: Vec<_> = names.into_iter().map(str::trim).collect();
    names
      .iter()
      .all(|name| dashed(name))
      .then(|| names.join(", "))
  };
  match name {
    "anchor-name" if lower == "none" => Some(lower),
    "anchor-scope" if matches!(&*lower, "none" | "all") => Some(lower),
    "anchor-name" | "anchor-scope" => names(),
    "position-anchor" if matches!(&*lower, "normal" | "none" | "auto") => Some(lower),
    "position-anchor" => dashed(value).then(|| value.into()),
    "position-area" if lower == "none" => Some(lower),
    "position-area" => area_words(value),
    "position-try-order" => ORDERS.contains(&&*lower).then_some(lower),
    "position-try-fallbacks" if lower == "none" => Some(lower),
    "position-try-fallbacks" => fallbacks(value),
    "position-try" => {
      let (order, fallbacks) = position_try(value)?;
      Some(match &*order {
        "normal" => fallbacks,
        _ => format!("{order} {fallbacks}"),
      })
    }
    "position-visibility" if lower == "always" => Some(lower),
    "position-visibility" => {
      let mut out = String::new();
      let mut count = 0;
      for rule in ["anchor-valid", "anchor-visible", "no-overflow"] {
        for word in lower.split_ascii_whitespace() {
          if word == rule || word == "anchors-visible" && rule == "anchor-visible" {
            out.push_str(if out.is_empty() { "" } else { " " });
            out.push_str(rule);
            count += 1;
          }
        }
      }
      let words = lower.split_ascii_whitespace();
      let once = out
        .split(' ')
        .zip(out.split(' ').skip(1))
        .all(|(a, b)| a != b);
      (count > 0 && count == words.count() && once).then_some(out)
    }
    _ => None,
  }
}

pub(crate) fn computed_area(value: &str) -> String {
  match value.split_once(' ') {
    Some(words) => {
      let plain = |word: &str| word.replace("block-", "").replace("inline-", "");
      let words = [plain(words.0), plain(words.1)];
      match words[0] == words[1] {
        true => words[0].clone(),
        false => words.join(" "),
      }
    }
    None => value.into(),
  }
}

/// The properties of an `@position-try` rule.
pub(crate) const DESCRIPTORS: PropSet = PropSet::new(&[
  Prop::TOP,
  Prop::RIGHT,
  Prop::BOTTOM,
  Prop::LEFT,
  Prop::POSITION_ANCHOR,
  Prop::POSITION_AREA,
  Prop::ALIGN_SELF,
  Prop::JUSTIFY_SELF,
  Prop::PLACE_SELF,
  Prop::BLOCK_SIZE,
  Prop::HEIGHT,
  Prop::INLINE_SIZE,
  Prop::INSET,
  Prop::INSET_BLOCK,
  Prop::INSET_BLOCK_END,
  Prop::INSET_BLOCK_START,
  Prop::INSET_INLINE,
  Prop::INSET_INLINE_END,
  Prop::INSET_INLINE_START,
  Prop::MARGIN,
  Prop::MARGIN_BLOCK,
  Prop::MARGIN_BLOCK_END,
  Prop::MARGIN_BLOCK_START,
  Prop::MARGIN_BOTTOM,
  Prop::MARGIN_INLINE,
  Prop::MARGIN_INLINE_END,
  Prop::MARGIN_INLINE_START,
  Prop::MARGIN_LEFT,
  Prop::MARGIN_RIGHT,
  Prop::MARGIN_TOP,
  Prop::MAX_BLOCK_SIZE,
  Prop::MAX_HEIGHT,
  Prop::MAX_INLINE_SIZE,
  Prop::MAX_WIDTH,
  Prop::MIN_BLOCK_SIZE,
  Prop::MIN_HEIGHT,
  Prop::MIN_INLINE_SIZE,
  Prop::MIN_WIDTH,
  Prop::WIDTH,
]);

// The flips of sides: of left and right, of top and bottom, and of x and y.
type Flips = [bool; 3];

// A keyword of `WORDS` after a flip. `first` tells a word for both axes that it is for the block axis.
fn flipped(word: &str, flips: Flips, first: bool) -> String {
  let Some(mut at) = word_index(word) else {
    return word.into();
  };
  let axis = match WORDS[at].1 {
    Group::X | Group::Inline | Group::OwnInline => 0,
    Group::Y | Group::Block | Group::OwnBlock => 1,
    _ => usize::from(first),
  };
  if flips[2] && at < 20 {
    at ^= 2;
  }
  if flips[axis] {
    at ^= 1;
  }
  let span = if word.starts_with("span-") {
    "span-"
  } else {
    ""
  };
  format!("{span}{}", WORDS[at].0)
}

// What a flip turns: the insets and the margins by side, the sizes with those of y after those of x, and the
// alignments of both axes.
const TURNED: [Prop; 17] = [
  Prop::TOP,
  Prop::RIGHT,
  Prop::BOTTOM,
  Prop::LEFT,
  Prop::MARGIN_TOP,
  Prop::MARGIN_RIGHT,
  Prop::MARGIN_BOTTOM,
  Prop::MARGIN_LEFT,
  Prop::WIDTH,
  Prop::MIN_WIDTH,
  Prop::MAX_WIDTH,
  Prop::HEIGHT,
  Prop::MIN_HEIGHT,
  Prop::MAX_HEIGHT,
  Prop::JUSTIFY_SELF,
  Prop::ALIGN_SELF,
  Prop::POSITION_AREA,
];

// The functions of a value, turned with the side it is for.
fn turn_functions(value: &str, side: usize, flips: Flips, mirrored: bool) -> String {
  let own = flips[usize::from(side.is_multiple_of(2))];
  let turned = replace(value, &mut |function| {
    let side = match (&*function.side, function.size) {
      (side, true) if !flips[2] => side.to_owned(),
      ("width", true) => "height".into(),
      ("height", true) => "width".into(),
      (side, true) if side.contains("block") => side.replace("block", "inline"),
      (side, true) => side.replace("inline", "block"),
      (side, _) if own && side.ends_with("start") => side.replace("start", "end"),
      (side, _) if own && side.ends_with("end") => side.replace("end", "start"),
      (side, _) if own && side.contains('%') => match side.strip_suffix('%') {
        Some(number) if !side.contains('(') => {
          format!("{}%", 100. - number.parse::<f64>().ok()?)
        }
        _ => format!("calc(100% - {side})"),
      },
      (side @ ("left" | "right" | "top" | "bottom"), _) => turn_side(side, flips, mirrored),
      (side, _) => side.to_owned(),
    };
    Some(Function { side, ..*function }.text())
  });
  turned.unwrap_or_else(|| value.into())
}

// In a right-to-left box the start of x is at the right, so the corners that `flip-start` swaps are the other two.
fn turn_side(side: &str, flips: Flips, mirrored: bool) -> String {
  let side = flipped(side, flips, false);
  match mirrored {
    true => flipped(&side, [true, true, false], false),
    false => side,
  }
}

/// Turns the positions of a style by flips, as `position-try-fallbacks` does.
pub(crate) fn flip(flips: &[&str], rtl: bool, style: &mut Style) {
  const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];
  let mut values: [Option<String>; 17] = TURNED.map(|prop| style.get(prop).map(str::to_owned));
  for flip in flips {
    let turns: Flips = match *flip {
      "flip-inline" | "flip-x" => [true, false, false],
      "flip-block" | "flip-y" => [false, true, false],
      _ => [false, false, true],
    };
    let mirrored = turns[2] && rtl;
    let old = std::mem::take(&mut values);
    for (at, value) in old.into_iter().enumerate() {
      let value = value.map(|value| match at {
        16 => flip_area(&value, turns),
        14 | 15 if !turns[2] && turns[at - 14] => value
          .split(' ')
          .map(|word| match word {
            "left" => "right".into(),
            "right" => "left".into(),
            word if word.ends_with("start") => word.replace("start", "end"),
            word if word.ends_with("end") => word.replace("end", "start"),
            word => word.into(),
          })
          .collect::<Vec<_>>()
          .join(" "),
        14 | 15 => value,
        _ => turn_functions(&value, at % 4, turns, mirrored),
      });
      let to = match at {
        0..8 => {
          let side = turn_side(SIDES[at % 4], turns, mirrored);
          at / 4 * 4 + SIDES.iter().position(|name| *name == side).unwrap_or(0)
        }
        8..14 if turns[2] => 8 + (at - 8 + 3) % 6,
        14 | 15 if turns[2] => 29 - at,
        at => at,
      };
      values[to] = value;
    }
  }
  for (prop, value) in TURNED.into_iter().zip(values) {
    match value {
      Some(value) if style.get(prop) == Some(&*value) => {}
      Some(value) => style.insert(prop, value),
      None => drop(style.remove(prop)),
    }
  }
}

fn flip_area(value: &str, turns: Flips) -> String {
  let (one, two) = value.split_once(' ').unwrap_or((value, ""));
  let both = |word: &str| {
    keyword(word).is_some_and(|found| matches!(found.0, Group::Any | Group::Own | Group::Both))
  };
  // `start` alone is `start start`, where the first one is for the block axis.
  let two = match word_index(one) {
    Some(20..) if two.is_empty() => one,
    _ => two,
  };
  let mut words = [flipped(one, turns, true), flipped(two, turns, false)];
  if turns[2] && both(one) && both(two) {
    words.swap(0, 1);
  }
  let text = words.join(" ").trim_end().to_owned();
  area_words(&text).unwrap_or(text)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn values_print_like_chrome() {
    for (name, value, expected) in [
      ("anchor-name", "--A,--b", Some("--A, --b")),
      ("anchor-name", "--a --b", None),
      ("anchor-scope", "ALL", Some("all")),
      ("position-anchor", "a", None),
      ("position-area", "top left", Some("left top")),
      ("position-area", "center center", Some("center")),
      ("position-area", "span-all top", Some("top")),
      ("position-area", "start span-all", Some("start span-all")),
      (
        "position-area",
        "inline-end block-start",
        Some("block-start inline-end"),
      ),
      ("position-area", "top inline-start", None),
      ("position-area", "left right", None),
      ("position-area", "x-self-start", None),
      (
        "position-try-fallbacks",
        "flip-block --x, top left",
        Some("--x flip-block, left top"),
      ),
      ("position-try-fallbacks", "flip-block flip-block", None),
      ("position-try-fallbacks", "none, --x", None),
      (
        "position-try",
        "most-width flip-block",
        Some("most-width flip-block"),
      ),
      ("position-try", "normal none", Some("none")),
      ("position-try", "most-width", None),
      (
        "position-visibility",
        "anchors-visible",
        Some("anchor-visible"),
      ),
      ("position-visibility", "always no-overflow", None),
    ] {
      assert_eq!(
        specified(name, value).as_deref(),
        expected,
        "{name}: {value}"
      );
    }
    for (name, value, expected) in [
      ("left", "anchor(right --a)", Some("anchor(--a right)")),
      ("left", "ANCHOR(LEFT, 5px)", Some("anchor(left, 5px)")),
      (
        "left",
        "calc(anchor(50%) + 1px)",
        Some("calc(anchor(50%) + 1px)"),
      ),
      ("left", "anchor(left, 3px", Some("anchor(left, 3px)")),
      ("left", "anchor(left, )", None),
      ("left", "anchor(,3px)", None),
      ("left", "anchor(5px)", None),
      ("left", "anchor(left, auto)", None),
      ("width", "anchor(left)", None),
      ("width", "anchor-size()", Some("anchor-size()")),
      ("width", "fit-content(anchor-size(width))", None),
      ("padding-left", "anchor-size(width)", None),
    ] {
      assert_eq!(
        canonical(name, value).as_deref(),
        expected,
        "{name}: {value}"
      );
    }
  }

  #[test]
  fn areas_follow_the_direction() {
    for (value, rtl, tracks) in [
      ("top", [false; 2], [[0, 3], [0, 1]]),
      ("bottom span-left", [false; 2], [[0, 2], [2, 3]]),
      ("start end", [false; 2], [[2, 3], [0, 1]]),
      ("start end", [true, false], [[0, 1], [0, 1]]),
      ("self-start", [true, false], [[0, 1], [0, 1]]),
      ("self-start", [false, true], [[2, 3], [0, 1]]),
      ("inline-start", [true, true], [[2, 3], [0, 3]]),
    ] {
      assert_eq!(area(value, rtl), Some(tracks), "{value}");
    }
  }

  #[test]
  fn flips_turn_sides_functions_and_areas() {
    let style: Style = [
      (Prop::BOTTOM, "calc(anchor(--a top, 3px) + 1px)"),
      (Prop::LEFT, "anchor(25%)"),
      (Prop::MARGIN_BOTTOM, "4px"),
      (Prop::WIDTH, "anchor-size(width)"),
      (Prop::ALIGN_SELF, "safe start"),
      (Prop::POSITION_AREA, "left top"),
    ]
    .map(|(prop, value)| (prop, value.into()))
    .into_iter()
    .collect();
    let turned = |flips: &[&str], prop: Prop| {
      let mut turned = style.clone();
      flip(flips, false, &mut turned);
      turned.get(prop).map(str::to_owned)
    };
    let block = ["flip-block"];
    assert_eq!(
      turned(&block, Prop::TOP).as_deref(),
      Some("calc(anchor(--a bottom, 3px) + 1px)")
    );
    assert_eq!(turned(&block, Prop::BOTTOM), None);
    assert_eq!(turned(&block, Prop::MARGIN_TOP).as_deref(), Some("4px"));
    assert_eq!(
      turned(&block, Prop::ALIGN_SELF).as_deref(),
      Some("safe end")
    );
    assert_eq!(
      turned(&block, Prop::POSITION_AREA).as_deref(),
      Some("left bottom")
    );
    assert_eq!(
      turned(&["flip-inline"], Prop::RIGHT).as_deref(),
      Some("anchor(75%)")
    );
    let start = ["flip-start"];
    assert_eq!(
      turned(&start, Prop::HEIGHT).as_deref(),
      Some("anchor-size(height)")
    );
    assert_eq!(
      turned(&start, Prop::RIGHT)
        .as_deref()
        .map(|value| value.contains("anchor(--a left")),
      Some(true)
    );
    assert_eq!(flip_area("start", [false, true, false]), "end start");
    assert_eq!(
      flip_area("span-self-x-start top", [false, false, true]),
      "left span-self-y-start"
    );
    assert_eq!(
      flip_area("block-start span-inline-end", [false, false, true]),
      "span-block-end inline-start"
    );
    assert_eq!(flip_area("none", [true, false, false]), "none");
    let other: Style = [(Prop::TOP, "anchor(start)".into())].into_iter().collect();
    let mut turned = other.clone();
    flip(&["flip-inline"], false, &mut turned);
    assert_eq!(turned.get(Prop::TOP), Some("anchor(start)"));
  }

  #[test]
  fn anchor_functions_take_any_case() {
    assert_eq!(
      crate::css::value::grammar::supports("left", "ANCHOR(--a right)"),
      Some(true)
    );
    assert!(has_function("Anchor-Size(width)"));
  }
}
