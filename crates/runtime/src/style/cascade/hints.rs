use crate::css::properties::{BORDERS, PADDING, Prop};
use crate::dom::Document;
use crate::style::cascade::table::span;
use std::rc::Rc;

type Hints = Vec<(Prop, Rc<str>)>;

#[inline(never)]
fn add(hints: &mut Hints, props: &[Prop], value: &str) {
  let value: Rc<str> = value.into();
  hints.extend(props.iter().map(|&prop| (prop, value.clone())));
}

const ALL: [usize; 4] = [0, 1, 2, 3];

// An HTML dimension: a number with an optional `%`. What follows it is ignored.
fn length(value: &str) -> Option<(f32, bool)> {
  let value = value.trim_start();
  let value = value.strip_prefix('+').unwrap_or(value);
  let mut dot = false;
  let end = value
    .find(|c: char| !(c.is_ascii_digit() || c == '.' && !std::mem::replace(&mut dot, true)))
    .unwrap_or(value.len());
  Some((
    value[..end].trim_end_matches('.').parse().ok()?,
    value[end..].starts_with('%'),
  ))
}

fn size(value: &str) -> Option<String> {
  let (number, percent) = length(value)?;
  Some(format!("{number}{}", if percent { "%" } else { "px" }))
}

fn integer(value: &str) -> Option<i64> {
  let value = value.trim_start();
  let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
  let end = digits
    .find(|c: char| !c.is_ascii_digit())
    .unwrap_or(digits.len());
  value[..value.len() - digits.len() + end].parse().ok()
}

// The rules of HTML for a legacy color value.
fn color(value: &str) -> Option<String> {
  let value = value.trim();
  if value.is_empty() || value.eq_ignore_ascii_case("transparent") {
    return None;
  }
  let hex = value.strip_prefix('#').unwrap_or(value);
  let digits = hex.bytes().all(|c| c.is_ascii_hexdigit());
  if digits && hex.len() == 6 || digits && hex.len() == 3 && value.starts_with('#') {
    return Some(format!("#{hex}"));
  }
  if !value.starts_with('#')
    && value.bytes().all(|c| c.is_ascii_alphabetic())
    && crate::css::value::color::color(value).is_some()
  {
    return Some(value.to_owned());
  }
  let mut digits = Vec::new();
  for c in hex.chars().take(128) {
    let c = match c {
      c if c.is_ascii_hexdigit() => c as u8,
      c if c as u32 > 0xffff => {
        digits.push(b'0');
        b'0'
      }
      _ => b'0',
    };
    digits.push(c);
  }
  while digits.is_empty() || !digits.len().is_multiple_of(3) {
    digits.push(b'0');
  }
  let part = digits.len() / 3;
  let mut from = part.saturating_sub(8);
  while part - from > 2 && (0..3).all(|at| digits[at * part + from] == b'0') {
    from += 1;
  }
  let mut rgb = "#".to_owned();
  for at in 0..3 {
    let end = part.min(from + 2);
    if end - from < 2 {
      rgb.push('0');
    }
    rgb.extend((from..end).map(|index| digits[at * part + index] as char));
  }
  Some(rgb)
}

// `align` as Chrome reads it for blocks and table parts: these values also align the blocks inside.
fn align(value: &str) -> Option<&'static str> {
  Some(match value {
    "center" | "middle" => "-webkit-center",
    "absmiddle" => "center",
    "left" => "-webkit-left",
    "right" => "-webkit-right",
    "justify" => "justify",
    "start" => "start",
    "end" => "end",
    _ => return None,
  })
}

fn borders(hints: &mut Hints, sides: &[usize], width: &str, kind: &str, color: &str) {
  for &side in sides {
    for (prop, value) in BORDERS[side].into_iter().zip([width, kind, color]) {
      if !value.is_empty() {
        add(hints, &[prop], value);
      }
    }
  }
}

// Elements with presentational attributes, as bits.
const TABLE: u32 = 1;
const CELL: u32 = 1 << 1;
const ROW: u32 = 1 << 2;
const GROUP: u32 = 1 << 3;
const COL: u32 = 1 << 4;
const COLGROUP: u32 = 1 << 5;
const CAPTION: u32 = 1 << 6;
const HR: u32 = 1 << 7;
const OL: u32 = 1 << 8;
const UL: u32 = 1 << 9;
const LI: u32 = 1 << 10;
const FONT: u32 = 1 << 11;
const BODY: u32 = 1 << 12;
const HEADING: u32 = 1 << 13;
const BLOCK: u32 = 1 << 14;
const IMAGE: u32 = 1 << 15;
const FRAMED: u32 = 1 << 16;
const IFRAME: u32 = 1 << 17;
const BR: u32 = 1 << 18;
const COLS: u32 = COL | COLGROUP;
const PART: u32 = CELL | ROW | GROUP | COLS;

// The presentational hints of the attributes of an HTML element, as in Chrome.
pub(crate) fn html(doc: &Document, id: usize, hints: &mut Hints) {
  let node = &doc.nodes[id];
  let tag = match node.tag.as_str() {
    "table" => TABLE,
    "td" | "th" => CELL,
    "tr" => ROW,
    "thead" | "tbody" | "tfoot" => GROUP,
    "col" => COL,
    "colgroup" => COLGROUP,
    "caption" => CAPTION,
    "hr" => HR,
    "ol" => OL,
    "ul" => UL,
    "li" => LI,
    "font" => FONT,
    "body" => BODY,
    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => HEADING,
    "div" | "p" => BLOCK,
    "img" | "object" => IMAGE | FRAMED,
    "embed" => IMAGE,
    "input" if node.input_type().eq_ignore_ascii_case("image") => IMAGE,
    "iframe" => IFRAME,
    "br" => BR,
    // The styles that a view transition gives its pseudo-elements, which author rules override.
    tag if tag.starts_with("::") => {
      for (name, value) in node.attrs.iter() {
        let prop = if name.starts_with("--") {
          Some(Prop::new(name))
        } else {
          Prop::find(name)
        };
        hints.extend(prop.map(|prop| (prop, value.as_str().into())));
      }
      return;
    }
    _ => return,
  };
  let of = |tags: u32| tag & tags != 0;
  let is = |value: Option<&String>, keyword: &str| {
    value.is_some_and(|value| value.eq_ignore_ascii_case(keyword))
  };
  if of(PART) {
    let Some(table) = std::iter::successors(node.parent, |&at| doc.nodes[at].parent)
      .take(3)
      .find(|&at| doc.nodes[at].tag == "table")
    else {
      return attributes(node, tag, hints);
    };
    let table = &doc.nodes[table].attrs;
    let rules = table.get("rules");
    if of(CELL) {
      if let Some(padding) = table.get("cellpadding").filter(|value| !value.is_empty()) {
        let padding = format!("{}px", integer(padding).unwrap_or(0).max(0));
        add(hints, &PADDING, &padding);
      }
      let border = (table.get("border")).is_some_and(|value| integer(value) != Some(0));
      let (sides, kind): (&[usize], _) = if is(rules, "all") {
        (&ALL, "solid")
      } else if is(rules, "cols") {
        (&[1, 3], "solid")
      } else if is(rules, "rows") {
        (&[0, 2], "solid")
      } else if !border || is(rules, "none") || is(rules, "groups") {
        (&[], "")
      } else if table.get("bordercolor").is_some_and(|v| !v.is_empty()) {
        (&ALL, "solid")
      } else {
        (&ALL, "inset")
      };
      borders(hints, sides, "1px", kind, "inherit");
    } else if of(GROUP | COLGROUP) && is(rules, "groups") {
      let sides = if of(GROUP) { [0, 2] } else { [1, 3] };
      borders(hints, &sides, "1px", "solid", "");
    }
  }
  if of(TABLE) {
    let get = |name| node.attrs.get(name);
    let border = get("border").map(|value| integer(value).filter(|&v| v >= 0).unwrap_or(1));
    let ink = (get("bordercolor").and_then(|value| color(value))).unwrap_or_default();
    let rules = get("rules");
    let ruled = (["none", "groups", "rows", "cols", "all"].iter()).any(|rule| is(rules, rule));
    // The sides as bits, from the top.
    let frames = [
      ("above", 1),
      ("below", 4),
      ("hsides", 5),
      ("vsides", 10),
      ("lhs", 8),
      ("rhs", 2),
      ("box", 15),
      ("border", 15),
      ("void", 0),
    ];
    let frame = frames.iter().find(|(name, _)| is(get("frame"), name));
    if ruled {
      add(hints, &[Prop::BORDER_COLLAPSE], "collapse");
    }
    let given = border.map(|width| format!("{width}px"));
    for side in ALL {
      let (width, kind) = match (frame, border) {
        (Some((_, sides)), _) if sides >> side & 1 != 0 => ("1px", "solid"),
        (Some(_), _) => ("1px", "hidden"),
        (None, Some(1..)) if ink.is_empty() => ("", "outset"),
        (None, Some(1..)) => ("", "solid"),
        (None, _) => ("", if ruled { "hidden" } else { "" }),
      };
      let width = given.as_deref().unwrap_or(width);
      borders(hints, &[side], width, kind, &ink);
    }
  }
  attributes(node, tag, hints);
}

fn attributes(node: &crate::dom::Node, tag: u32, hints: &mut Hints) {
  const MARGINS: [Prop; 2] = [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT];
  let of = |tags: u32| tag & tags != 0;
  for (name, value) in node.attrs.iter() {
    let low = match name.as_str() {
      "type" | "align" | "valign" | "clear" => value.to_ascii_lowercase(),
      _ => String::new(),
    };
    let low = low.as_str();
    let one_of = |keywords: &[&'static str]| -> &'static str {
      let found = keywords.iter().find(|&&keyword| keyword == low);
      found.copied().unwrap_or_default()
    };
    let mut text = String::new();
    // The properties of a dimension or color that `text` holds.
    let mut props: &[Prop] = &[];
    let mut dimension = |list: &'static [Prop], positive: bool| {
      if !positive || length(value).is_some_and(|(value, _)| value > 0.) {
        (props, text) = (list, size(value).unwrap_or_default());
      }
    };
    let mut found: (Prop, &str) = (Prop::WIDTH, "");
    match name.as_str() {
      "width" if of(TABLE | CELL) => dimension(&[Prop::WIDTH], true),
      "width" if of(HR) || of(COLS) && !value.trim_end().ends_with('*') => {
        dimension(&[Prop::WIDTH], false)
      }
      "height" if of(CELL) => dimension(&[Prop::HEIGHT], true),
      "height" if of(TABLE | ROW | GROUP) => dimension(&[Prop::HEIGHT], false),
      "cellspacing" if of(TABLE) && !value.contains('%') => {
        dimension(&[Prop::BORDER_SPACING], false)
      }
      "hspace" if of(IMAGE) => dimension(&MARGINS, false),
      "vspace" if of(IMAGE) => dimension(&[Prop::MARGIN_TOP, Prop::MARGIN_BOTTOM], false),
      "bgcolor" if of(PART | TABLE | BODY) => {
        (props, text) = (&[Prop::BACKGROUND_COLOR], color(value).unwrap_or_default());
      }
      "bordercolor" if of(PART) => {
        let ink = color(value).unwrap_or_default();
        borders(hints, &ALL, "", "", &ink);
      }
      "text" if of(BODY) => (props, text) = (&[Prop::COLOR], color(value).unwrap_or_default()),
      "color" if of(FONT) => {
        (props, text) = (&[Prop::COLOR], color(value).unwrap_or_default());
      }
      "color" | "noshade" if of(HR) => {
        let ink = match name.as_str() {
          "color" => color(value).unwrap_or_default(),
          _ if node.attrs.contains_key("color") => String::new(),
          _ => "#808080".into(),
        };
        if !ink.is_empty() {
          borders(hints, &ALL, "", "solid", &ink);
        }
        (props, text) = (&[Prop::BACKGROUND_COLOR], ink);
      }
      "type" if of(OL | UL | LI) => {
        let kind = match value.as_str() {
          "a" => "lower-alpha",
          "A" => "upper-alpha",
          "i" => "lower-roman",
          "I" => "upper-roman",
          "1" => "decimal",
          _ => "",
        };
        found = (Prop::LIST_STYLE_TYPE, if of(UL) { "" } else { kind });
        if found.1.is_empty() && !of(OL) {
          found.1 = one_of(&["disc", "circle", "square", "none", "decimal"]);
        }
      }
      "face" if of(FONT) => found = (Prop::FONT_FAMILY, value.trim()),
      "size" if of(FONT) => {
        if let Some(size) = integer(value) {
          let relative = value.trim_start().starts_with(['+', '-']);
          let size = size.saturating_add(if relative { 3 } else { 0 });
          let names = [
            "x-small",
            "small",
            "medium",
            "large",
            "x-large",
            "xx-large",
            "xxx-large",
          ];
          found = (Prop::FONT_SIZE, names[size.clamp(1, 7) as usize - 1]);
        }
      }
      "size" if of(HR) => match integer(value).unwrap_or(0) {
        size if size > 1 => (props, text) = (&[Prop::HEIGHT], format!("{}px", size - 2)),
        _ => found = (Prop::BORDER_BOTTOM_WIDTH, "0"),
      },
      "nowrap" if of(CELL) => found = (Prop::WHITE_SPACE, "nowrap"),
      "colspan" if of(CELL) => {
        text = format!("span {}", span(node, name));
        props = &[Prop::GRID_COLUMN_START];
      }
      "rowspan" if of(CELL) => {
        text = format!("span {}", span(node, name));
        props = &[Prop::GRID_ROW_START];
      }
      "align" if of(TABLE) && low == "center" => add(hints, &MARGINS, "auto"),
      "align" if of(TABLE) => found = (Prop::FLOAT, one_of(&["left", "right"])),
      "align" if of(CAPTION) => found = (Prop::CAPTION_SIDE, one_of(&["top", "bottom"])),
      "align" if of(HEADING) => {
        let sides = ["left", "right", "center", "justify", "start", "end"];
        let side = if low == "middle" {
          "center"
        } else {
          one_of(&sides)
        };
        found = (Prop::TEXT_ALIGN, side);
      }
      "align" if of(PART | BLOCK) => {
        found = (Prop::TEXT_ALIGN, align(low.trim()).unwrap_or_default());
      }
      "align" if of(HR) => {
        add(
          hints,
          &MARGINS[..1],
          if low == "left" { "0" } else { "auto" },
        );
        found = (MARGINS[1], if low == "right" { "0" } else { "auto" });
      }
      "align" if of(IMAGE | IFRAME) => {
        let side = match low {
          "left" | "right" => {
            add(hints, &[Prop::FLOAT], low);
            "top"
          }
          "absmiddle" => "middle",
          "center" | "middle" => "-webkit-baseline-middle",
          "absbottom" => "bottom",
          "bottom" => "baseline",
          "texttop" => "text-top",
          _ => one_of(&["top"]),
        };
        found = (Prop::VERTICAL_ALIGN, side);
      }
      "valign" if of(PART) => {
        let sides = ["top", "middle", "bottom", "baseline"];
        found = (Prop::VERTICAL_ALIGN, one_of(&sides));
      }
      "border" if of(FRAMED) => {
        let width = format!("{}px", integer(value).unwrap_or(0).max(0));
        borders(hints, &ALL, &width, "solid", "");
      }
      "clear" if of(BR) => {
        let side = if low == "all" {
          "both"
        } else {
          one_of(&["both", "left", "right"])
        };
        found = (Prop::CLEAR, side);
      }
      _ => {}
    }
    if !text.is_empty() {
      add(hints, props, &text);
    } else if !found.1.is_empty() {
      add(hints, &[found.0], found.1);
    }
  }
}
