//! The basic shapes of `clip-path`, `offset-path` and `shape-outside`.
use crate::css::value::{
  resolved::percent_sum, resolved::percent_text, specified::shortest, tokens::components,
  tokens::number, tokens::write_printed, units::Kind, units::Lengths,
};
use std::f32::consts::{FRAC_PI_2, TAU};

pub(crate) const BOXES: [&str; 7] = [
  "border-box",
  "padding-box",
  "content-box",
  "margin-box",
  "fill-box",
  "stroke-box",
  "view-box",
];

/// A value of `clip-path` or `offset-path` with a shape, a box or both.
#[cfg_attr(test, derive(Debug, PartialEq))]
pub(crate) struct Shaped<'a> {
  pub(crate) shape: Option<Shape<'a>>,
  pub(crate) reference: Option<&'static str>,
}

#[cfg_attr(test, derive(Debug, PartialEq))]
pub(crate) enum Shape<'a> {
  /// The function as written, its arguments and its radii.
  Inset(&'a str, Vec<&'a str>, Vec<&'a str>, Vec<&'a str>),
  Circle(Option<&'a str>, Option<Position<'a>>),
  Ellipse(Option<[&'a str; 2]>, Option<Position<'a>>),
  Polygon(bool, Option<&'a str>, Vec<[&'a str; 2]>),
  Path(bool, Vec<Segment>),
  Commands(bool, Position<'a>, Vec<Command<'a>>),
  Ray(Ray<'a>),
}

#[cfg_attr(test, derive(Debug, PartialEq))]
pub(crate) struct Ray<'a> {
  pub(crate) angle: &'a str,
  pub(crate) size: &'a str,
  pub(crate) contain: bool,
  pub(crate) at: Option<Position<'a>>,
}

/// A `<position>` in the order of its axes: one or two parts for each.
#[cfg_attr(test, derive(Debug, PartialEq))]
pub(crate) struct Position<'a>(pub(crate) [Vec<&'a str>; 2]);

#[cfg_attr(test, derive(Debug, PartialEq))]
pub(crate) struct Segment(pub(crate) u8, pub(crate) Vec<f64>);

#[cfg_attr(test, derive(Debug, PartialEq))]
pub(crate) struct Command<'a> {
  /// `m`, `l`, `h`, `v`, `c`, `s`, `a` or `z`.
  kind: u8,
  to: bool,
  end: Vec<Position<'a>>,
  coordinate: &'a str,
  controls: Vec<(Position<'a>, &'a str)>,
  radii: Vec<&'a str>,
  clockwise: bool,
  large: bool,
  rotate: Option<&'a str>,
}

/// A shape in px, where arcs are cubic curves.
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub(crate) enum Op {
  Move([f32; 2]),
  Line([f32; 2]),
  Quad([f32; 2], [f32; 2]),
  Cubic([f32; 2], [f32; 2], [f32; 2]),
  Close,
}

fn is(value: &str, keyword: &str) -> bool {
  value.eq_ignore_ascii_case(keyword)
}

// Math in px needs a unit on its zeros.
fn unit(value: &str) -> &str {
  match value.parse::<f32>() {
    Ok(0.) => "0px",
    _ => value,
  }
}

fn length(value: &str, positive: bool) -> bool {
  // Most parts are plain, and paint parses shapes too.
  let plain = value
    .strip_suffix("px")
    .or_else(|| value.strip_suffix('%'))
    .unwrap_or("");
  !(positive && value.starts_with('-'))
    && (plain.starts_with(|c: char| c.is_ascii_digit())
      && plain.bytes().all(|b| b.is_ascii_digit() || b == b'.')
      && plain.parse::<f32>().is_ok()
      && !plain.ends_with('.')
      || super::grammar::syntax_matches("<length-percentage>", value))
}

fn angle(value: &str) -> bool {
  matches!(
    Lengths::new(16., 1., 1.).math(value, f32::NAN, &[]),
    Some((_, Kind::Angle))
  )
}

impl<'a> Position<'a> {
  pub(crate) fn parse(parts: &[&'a str]) -> Option<Self> {
    let side = |part: &str, axis: usize| {
      [["left", "right"], ["top", "bottom"]][axis]
        .iter()
        .any(|name| is(part, name))
    };
    let (mut x, mut y) = match *parts {
      [one] if side(one, 1) => (vec!["center"], vec![one]),
      [one] => (vec![one], vec!["center"]),
      [a, b] => (vec![a], vec![b]),
      [a, b, c, d] => (vec![a, b], vec![c, d]),
      _ => return None,
    };
    if side(x[0], 1) || side(y[0], 0) {
      std::mem::swap(&mut x, &mut y);
    }
    let valid = |axis: &[&str], at: usize| match axis {
      [one] => side(one, at) || is(one, "center") || length(one, false),
      [edge, offset] => side(edge, at) && length(offset, false),
      _ => false,
    };
    // A length can't come before a vertical keyword, or after a horizontal one.
    let ordered = parts.len() != 2
      || x[0] == parts[0]
      || [parts[0], parts[1]]
        .iter()
        .all(|part| side(part, 0) || side(part, 1) || is(part, "center"));
    (ordered && valid(&x, 0) && valid(&y, 1)).then_some(Self([x, y]))
  }

  /// The coordinates as length-percentages.
  pub(crate) fn offsets(&self) -> [String; 2] {
    self.0.each_ref().map(|axis| {
      let far = is(axis[0], "right") || is(axis[0], "bottom");
      match axis[..] {
        [_, offset] if far => format!("calc(100% - {})", unit(offset)),
        [_, offset] => unit(offset).into(),
        _ => coordinate(axis[0]),
      }
    })
  }

  fn written(&self) -> String {
    self
      .0
      .iter()
      .flatten()
      .map(|part| specified(part))
      .collect::<Vec<_>>()
      .join(" ")
  }
}

fn coordinate(value: &str) -> String {
  match &*value.to_ascii_lowercase() {
    "left" | "top" | "x-start" | "y-start" => "0%".into(),
    "center" => "50%".into(),
    "right" | "bottom" | "x-end" | "y-end" => "100%".into(),
    _ => unit(value).into(),
  }
}

// A part as Chrome writes it in a declaration.
fn specified(value: &str) -> String {
  let mut out = String::new();
  super::specified::canonical(unit(value), true, false, &mut out);
  out
}

/// The computed text of a length-percentage.
pub(crate) fn amount(value: &str, units: &Lengths) -> String {
  let value = unit(value);
  if let Some(percent) = value.strip_suffix('%').and_then(|n| n.parse::<f64>().ok()) {
    let mut out = String::new();
    write_printed(percent, &mut out);
    return out + "%";
  }
  if value.contains('%') {
    return percent_sum(value, units).map_or_else(|| specified(value), percent_text);
  }
  units.resolve(value, f32::NAN).map_or_else(
    || value.to_owned(),
    |length| {
      let mut out = String::new();
      write_printed(f64::from(length), &mut out);
      out + "px"
    },
  )
}

// The four values of one to four sides or corners.
fn four<'a>(values: &[&'a str]) -> Option<[&'a str; 4]> {
  Some(match *values {
    [a] => [a; 4],
    [a, b] => [a, b, a, b],
    [a, b, c] => [a, b, c, b],
    [a, b, c, d] => [a, b, c, d],
    _ => return None,
  })
}

// The parts with each `/` as a part, as in `20px/10px`.
fn slashes<'a>(parts: &[&'a str]) -> Vec<&'a str> {
  let mut out = Vec::with_capacity(parts.len());
  for part in parts {
    let (mut depth, mut start) = (0, 0);
    for (at, byte) in part.bytes().enumerate() {
      depth += i32::from(byte == b'(') - i32::from(byte == b')');
      if byte == b'/' && depth == 0 {
        out.extend(
          [&part[start..at], "/"]
            .iter()
            .filter(|part| !part.is_empty()),
        );
        start = at + 1;
      }
    }
    out.extend(Some(&part[start..]).filter(|part| !part.is_empty()));
  }
  out
}

fn rule(parts: &mut Vec<&str>) -> Option<bool> {
  let even = is(parts.first()?, "evenodd");
  (even || is(parts[0], "nonzero")).then(|| {
    parts.remove(0);
    even
  })
}

#[inline(never)]
fn call(name: &str, parts: &[String]) -> String {
  let mut out = format!("{name}(");
  for part in parts.iter().filter(|part| !part.is_empty()) {
    if !out.ends_with('(') {
      out.push(' ');
    }
    out.push_str(part);
  }
  out + ")"
}

impl<'a> Shaped<'a> {
  /// None when Chrome takes the value as invalid, or it is a keyword or a `url()`.
  pub(crate) fn parse(value: &'a str) -> Option<Self> {
    let parts = components(value, ' ');
    let mut reference = None;
    let mut shape = None;
    for part in &parts {
      match BOXES.iter().find(|name| is(part, name)) {
        Some(name) if reference.is_none() => reference = Some(*name),
        None if shape.is_none() => shape = Some(Shape::parse(part)?),
        _ => return None,
      }
    }
    (!parts.is_empty()).then_some(Self { shape, reference })
  }

  /// Chrome's text: computed with `units`, or as in a declaration.
  pub(crate) fn text(&self, units: Option<&Lengths>) -> String {
    let shape = self.shape.as_ref().map(|shape| shape.text(units));
    match (shape, self.reference) {
      (Some(shape), None | Some("border-box")) => shape,
      (Some(shape), Some(reference)) => format!("{shape} {reference}"),
      (None, reference) => reference.unwrap_or("none").into(),
    }
  }
}

impl<'a> Shape<'a> {
  pub(crate) fn parse(value: &'a str) -> Option<Self> {
    let (name, args) = value.strip_suffix(')')?.split_once('(')?;
    let name = &*name.to_ascii_lowercase();
    if args.trim_end().ends_with(',') {
      return None;
    }
    let list = components(args, ',');
    let parts = slashes(&components(args, ' '));
    // The parts before and after a keyword.
    let split = |keyword: &str| match parts.iter().position(|part| is(part, keyword)) {
      Some(at) => (&parts[..at], Some(&parts[at + 1..])),
      None => (&parts[..], None),
    };
    match name {
      "inset" | "rect" | "xywh" => {
        let (sides, round) = split("round");
        let auto = |side: &&str| name == "rect" && is(side, "auto");
        if !(1..=4).contains(&sides.len())
          || name != "inset" && sides.len() != 4
          || !sides
            .iter()
            .enumerate()
            .all(|(index, side)| auto(side) || length(side, name == "xywh" && index > 1))
        {
          return None;
        }
        let halves: Vec<&[&str]> = round
          .map(|round| round.split(|part| *part == "/").collect())
          .unwrap_or_default();
        if halves.len() > 2
          || !halves
            .iter()
            .all(|half| (1..=4).contains(&half.len()) && half.iter().all(|r| length(r, true)))
        {
          return None;
        }
        let radii = |index: usize| halves.get(index).map_or(Vec::new(), |h| h.to_vec());
        Some(Self::Inset(
          &value[..name.len()],
          sides.to_vec(),
          radii(0),
          radii(1),
        ))
      }
      "circle" | "ellipse" => {
        let (radii, at) = split("at");
        let at = match at {
          Some(at) => Some(Position::parse(at)?),
          None => None,
        };
        let side = |r: &str| is(r, "closest-side") || is(r, "farthest-side");
        if !radii.iter().all(|r| side(r) || length(r, true)) {
          return None;
        }
        match (name, radii) {
          ("circle", []) => Some(Self::Circle(None, at)),
          ("circle", [r]) if is(r, "closest-side") => Some(Self::Circle(None, at)),
          ("circle", [r]) => Some(Self::Circle(Some(r), at)),
          ("ellipse", []) => Some(Self::Ellipse(None, at)),
          ("ellipse", [x, y]) if is(x, "closest-side") && is(y, "closest-side") => {
            Some(Self::Ellipse(None, at))
          }
          ("ellipse", [x, y]) => Some(Self::Ellipse(Some([x, y]), at)),
          _ => None,
        }
      }
      "polygon" => {
        let mut first = components(list.first()?, ' ');
        let even = rule(&mut first);
        let round = match first.first() {
          Some(word) if is(word, "round") => Some(*first.get(1).filter(|r| {
            first.len() == 2 && !r.contains('%') && !r.starts_with('-') && length(r, true)
          })?),
          _ => None,
        };
        let head = even.is_some() || round.is_some();
        if head && round.is_none() && !first.is_empty() {
          return None;
        }
        let points = list[usize::from(head)..]
          .iter()
          .map(|point| {
            <[&str; 2]>::try_from(components(point, ' '))
              .ok()
              .filter(|point| point.iter().all(|part| length(part, false)))
          })
          .collect::<Option<Vec<_>>>()?;
        (!points.is_empty()).then_some(Self::Polygon(even.unwrap_or(false), round, points))
      }
      "path" => {
        let (even, data) = match list[..] {
          [data] => (false, data),
          [rule, data] => (is(rule, "evenodd"), data),
          _ => return None,
        };
        if list.len() == 2 && !even && !is(list[0], "nonzero") {
          return None;
        }
        let quote = data.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let data = data.strip_prefix(quote)?.strip_suffix(quote)?;
        Some(Self::Path(even, segments(data)?))
      }
      "shape" => {
        let mut first = components(list.first()?, ' ');
        let even = rule(&mut first).unwrap_or(false);
        let from = match first.split_first() {
          Some((word, from)) if is(word, "from") => Position::parse(from)?,
          _ => return None,
        };
        let commands = list[1..]
          .iter()
          .map(|command| Command::parse(command))
          .collect::<Option<Vec<_>>>()?;
        (!commands.is_empty()).then_some(Self::Commands(even, from, commands))
      }
      "ray" => {
        let (parts, at) = split("at");
        let at = match at {
          Some(at) => Some(Position::parse(at)?),
          None => None,
        };
        let mut sized = false;
        let mut ray = Ray {
          angle: "",
          size: "closest-side",
          contain: false,
          at,
        };
        for part in parts {
          if is(part, "contain") && !ray.contain {
            ray.contain = true;
          } else if let Some(size) = SIZES.iter().find(|size| is(part, size)) {
            if std::mem::replace(&mut sized, true) {
              return None;
            }
            ray.size = size;
          } else if ray.angle.is_empty() && angle(part) {
            ray.angle = part;
          } else {
            return None;
          }
        }
        (!ray.angle.is_empty()).then_some(Self::Ray(ray))
      }
      _ => None,
    }
  }

  pub(crate) fn text(&self, units: Option<&Lengths>) -> String {
    let one = |part: &str| match units {
      Some(units) => amount(part, units),
      None => specified(part),
    };
    // Math below zero computes to zero.
    let size = |part: &str| match one(part) {
      text if units.is_some() && text.starts_with('-') => {
        if text.ends_with('%') { "0%" } else { "0px" }.into()
      }
      text => text,
    };
    let at = |position: &Option<Position>| match position {
      Some(position) => format!("at {}", position.text(units)),
      None => String::new(),
    };
    match self {
      Self::Inset(name, sides, x, y) => {
        let name = if units.is_some() {
          "inset"
        } else {
          &name.to_ascii_lowercase()
        };
        let sides = match units {
          Some(_) => {
            let sides = self.inset().unwrap_or_default().map(|side| one(&side));
            shortest(sides.each_ref().map(String::as_str))
          }
          None => {
            let sides: Vec<String> = sides.iter().map(|side| one(side)).collect();
            match four(&sides.iter().map(String::as_str).collect::<Vec<_>>()) {
              Some(sides) if name == "inset" => shortest(sides),
              _ => sides.join(" "),
            }
          }
        };
        let radii = |list: &[&str]| {
          let corners = four(list).map(|corners| corners.map(size));
          corners.map(|corners| shortest(corners.each_ref().map(String::as_str)))
        };
        let zero = |text: &str| matches!(text, "0px" | "0%");
        let round = match (radii(x), radii(y)) {
          (Some(x), Some(y)) if x != y => format!("round {x} / {y}"),
          (Some(x), _) if !zero(&x) => format!("round {x}"),
          _ => String::new(),
        };
        call(name, &[sides, round])
      }
      Self::Circle(r, position) => call("circle", &[r.map_or(String::new(), size), at(position)]),
      Self::Ellipse(radii, position) => {
        let [x, y] = radii.map_or([String::new(), String::new()], |radii| radii.map(size));
        call("ellipse", &[x, y, at(position)])
      }
      Self::Polygon(even, round, points) => {
        let mut head = Vec::new();
        head.extend(even.then(|| "evenodd".to_owned()));
        head.extend(round.map(|round| format!("round {}", one(round))));
        let mut list = Vec::from_iter((!head.is_empty()).then(|| head.join(" ")));
        list.extend(points.iter().map(|point| point.map(one).join(" ")));
        format!("polygon({})", list.join(", "))
      }
      Self::Path(even, segments) => {
        let data = match units {
          Some(_) => data(&absolute(segments)),
          None => data(segments),
        };
        let rule = if *even { "evenodd, " } else { "" };
        format!("path({rule}\"{data}\")")
      }
      Self::Commands(even, from, commands) => {
        let rule = if *even { "evenodd " } else { "" };
        let mut out = format!("shape({rule}from {}", from.text(units));
        for command in commands {
          out.push_str(", ");
          out.push_str(&command.text(units));
        }
        out + ")"
      }
      Self::Ray(ray) => {
        let angle = match units {
          Some(units) => units.math(ray.angle, 0., &[]).map_or(String::new(), |a| {
            let mut out = String::new();
            write_printed(a.0, &mut out);
            out + "deg"
          }),
          None => specified(ray.angle),
        };
        let size = if ray.size == "closest-side" {
          ""
        } else {
          ray.size
        };
        let contain = if ray.contain { "contain" } else { "" };
        call("ray", &[angle, size.into(), contain.into(), at(&ray.at)])
      }
    }
  }

  // The sides of an `inset()` with the same box.
  fn inset(&self) -> Option<[String; 4]> {
    let Self::Inset(name, sides, ..) = self else {
      return None;
    };
    let far = |side: &str| match is(side, "auto") {
      true => "0%".into(),
      false => format!("calc(100% - {})", unit(side)),
    };
    let near = |side: &str| match is(side, "auto") {
      true => "0%".into(),
      false => unit(side).to_owned(),
    };
    Some(match (&*name.to_ascii_lowercase(), &sides[..]) {
      ("rect", [top, right, bottom, left]) => [near(top), far(right), far(bottom), near(left)],
      ("xywh", [x, y, w, h]) => {
        let end = |start: &str, size: &str| match size.contains('%') || !size.contains('(') {
          true => format!("calc(100% - ({} + {}))", unit(start), unit(size)),
          false => format!("calc(100% - ({} + max(0px, {size})))", unit(start)),
        };
        [near(y), end(x, w), end(y, h), near(x)]
      }
      _ => four(sides)?.map(|side| unit(side).to_owned()),
    })
  }

  /// The fill rule: whether it is even-odd.
  pub(crate) fn even(&self) -> bool {
    matches!(
      self,
      Self::Polygon(true, ..) | Self::Path(true, _) | Self::Commands(true, ..)
    )
  }

  /// The box and the radii of an inset, circle or ellipse in a reference box of `size`.
  pub(crate) fn rounded(
    &self,
    units: &Lengths,
    size: [f32; 2],
    center: Option<[f32; 2]>,
  ) -> Option<([f32; 4], [[f32; 2]; 4])> {
    let [width, height] = size;
    let circle = |position: &Option<Position>| {
      Some(match position {
        Some(position) => position.point(units, size)?,
        None => center.unwrap_or([width / 2., height / 2.]),
      })
    };
    let radius = |value: &str, sides: [f32; 2], base: f32| {
      if is(value, "closest-side") {
        return Some(sides[0].min(sides[1]));
      }
      if is(value, "farthest-side") {
        return Some(sides[0].max(sides[1]));
      }
      Some(units.resolve(value, base)?.max(0.))
    };
    let oval =
      |[x, y]: [f32; 2], [rx, ry]: [f32; 2]| ([x - rx, y - ry, 2. * rx, 2. * ry], [[rx, ry]; 4]);
    match self {
      Self::Inset(.., x, y) => {
        let mut sides = [0.; 4];
        for (index, (side, out)) in self.inset()?.iter().zip(&mut sides).enumerate() {
          *out = units.resolve(side, if index % 2 == 0 { height } else { width })?;
        }
        let [top, mut right, mut bottom, left] = sides;
        // Like Chrome, sides that overlap leave an empty box on one line.
        if left + right > width {
          right = width - left;
        }
        if top + bottom > height {
          bottom = height - top;
        }
        let rect = [left, top, width - left - right, height - top - bottom];
        let mut radii = [[0.; 2]; 4];
        for (axis, list) in [x, if y.is_empty() { x } else { y }]
          .into_iter()
          .enumerate()
        {
          for (corner, value) in four(list).into_iter().flatten().enumerate() {
            radii[corner][axis] = units.resolve(value, size[axis])?.max(0.);
          }
        }
        // As for `border-radius`, radii that don't fit scale down together.
        let fit = [
          rect[2] / (radii[0][0] + radii[1][0]),
          rect[3] / (radii[1][1] + radii[2][1]),
          rect[2] / (radii[2][0] + radii[3][0]),
          rect[3] / (radii[3][1] + radii[0][1]),
        ]
        .into_iter()
        .fold(1f32, f32::min);
        Some((rect, radii.map(|radius| radius.map(|value| value * fit))))
      }
      Self::Circle(r, position) => {
        let [x, y] = circle(position)?;
        let sides = [x.abs(), (width - x).abs(), y.abs(), (height - y).abs()];
        let near = sides.into_iter().fold(f32::MAX, f32::min);
        let far = sides.into_iter().fold(0., f32::max);
        let base = width.hypot(height) / std::f32::consts::SQRT_2;
        let r = radius(r.unwrap_or("closest-side"), [near, far], base)?;
        Some(oval([x, y], [r; 2]))
      }
      Self::Ellipse(radii, position) => {
        let [x, y] = circle(position)?;
        let [rx, ry] = radii.unwrap_or(["closest-side"; 2]);
        let rx = radius(rx, [x.abs(), (width - x).abs()], width)?;
        let ry = radius(ry, [y.abs(), (height - y).abs()], height)?;
        Some(oval([x, y], [rx, ry]))
      }
      _ => None,
    }
  }

  /// The outline in a reference box of `size`. `center` is where circles without a position are.
  pub(crate) fn outline(
    &self,
    units: &Lengths,
    size: [f32; 2],
    center: Option<[f32; 2]>,
  ) -> Option<Vec<Op>> {
    let mut pen = Pen::default();
    match self {
      Self::Circle(..) | Self::Ellipse(..) => {
        let ([x, y, w, h], [[rx, ry], ..]) = self.rounded(units, size, center)?;
        let (cx, cy) = (x + rx, y + ry);
        pen.go(Op::Move([x + w, cy]));
        for end in [[cx, y + h], [x, cy], [cx, y], [x + w, cy]] {
          pen.arc([rx, ry], 0., false, true, end);
        }
        pen.go(Op::Close);
      }
      Self::Inset(..) => {
        let (rect, radii) = self.rounded(units, size, center)?;
        pen.rounded(rect, radii);
      }
      Self::Polygon(_, round, points) => {
        let mut found = Vec::with_capacity(points.len());
        for [x, y] in points {
          found.push([units.resolve(x, size[0])?, units.resolve(y, size[1])?]);
        }
        let round = match round {
          Some(round) => units.resolve(round, 0.)?,
          None => 0.,
        };
        pen.polygon(&found, round);
      }
      Self::Path(_, segments) => {
        let mut quad = false;
        for Segment(kind, args) in absolute(segments) {
          let at = |index: usize| [args[index] as f32, args[index + 1] as f32];
          let reflected = match (kind, quad) {
            (b'S', false) | (b'T', true) => pen.reflected(),
            _ => pen.at,
          };
          match kind {
            b'M' => pen.go(Op::Move(at(0))),
            b'L' => pen.go(Op::Line(at(0))),
            b'H' => pen.go(Op::Line([args[0] as f32, pen.at[1]])),
            b'V' => pen.go(Op::Line([pen.at[0], args[0] as f32])),
            b'C' => pen.go(Op::Cubic(at(0), at(2), at(4))),
            b'S' => pen.go(Op::Cubic(reflected, at(0), at(2))),
            b'Q' => pen.go(Op::Quad(at(0), at(2))),
            b'T' => pen.go(Op::Quad(reflected, at(0))),
            b'A' => pen.arc(at(0), args[2] as f32, args[3] != 0., args[4] != 0., at(5)),
            _ => pen.go(Op::Close),
          }
          quad = matches!(kind, b'Q' | b'T');
        }
      }
      Self::Commands(_, from, commands) => {
        pen.go(Op::Move(from.point(units, size)?));
        let mut quad = false;
        for command in commands {
          quad = command.draw(&mut pen, units, size, quad)?;
        }
      }
      Self::Ray(_) => return None,
    }
    Some(pen.ops)
  }

  /// The parts of the shape for interpolation: its text with a gap for each amount, and the amounts. Numbers
  /// are kept as they are.
  fn parts(&self, at: bool) -> Option<(String, Vec<String>)> {
    let mut values = Vec::new();
    let mut text = String::new();
    let add: &mut dyn FnMut(&mut String, String) = &mut |text, value| {
      values.push(value);
      text.push_str(" \u{0}");
    };
    let center = |position: &Option<Position>| match position {
      Some(position) => position.offsets(),
      None => ["50%".into(), "50%".into()],
    };
    match self {
      Self::Inset(.., x, y) => {
        text.push_str("inset(");
        for side in self.inset()? {
          add(&mut text, side);
        }
        text.push_str(" round");
        for (index, list) in [x, if y.is_empty() { x } else { y }]
          .into_iter()
          .enumerate()
        {
          if index > 0 {
            text.push_str(" /");
          }
          for value in four(list).unwrap_or(["0px"; 4]) {
            add(&mut text, unit(value).into());
          }
        }
      }
      Self::Circle(..) | Self::Ellipse(..) => {
        let (radii, position) = match self {
          Self::Circle(r, position) => (vec![r.unwrap_or("closest-side")], position),
          Self::Ellipse(radii, position) => {
            (radii.unwrap_or(["closest-side"; 2]).to_vec(), position)
          }
          _ => return None,
        };
        if radii
          .iter()
          .any(|r| is(r, "closest-side") || is(r, "farthest-side"))
        {
          return None;
        }
        text.push_str(match self {
          Self::Circle(..) => "circle(",
          _ => "ellipse(",
        });
        for value in radii {
          add(&mut text, unit(value).into());
        }
        if at || position.is_some() {
          text.push_str(" at");
          for value in center(position) {
            add(&mut text, value);
          }
        }
      }
      Self::Polygon(even, round, points) => {
        text.push_str("polygon(");
        text.push_str(if *even { "evenodd" } else { "nonzero" });
        if let Some(round) = round {
          text.push_str(" round");
          add(&mut text, unit(round).into());
        }
        for point in points {
          text.push(',');
          for value in point {
            add(&mut text, unit(value).into());
          }
        }
      }
      Self::Path(even, segments) => {
        text.push_str(if *even { "path(evenodd,\"" } else { "path(\"" });
        for Segment(kind, args) in absolute(segments) {
          text.push(kind as char);
          for (index, value) in args.iter().enumerate() {
            match kind == b'A' && matches!(index, 3 | 4) {
              true => text.push_str(if *value != 0. { " 1" } else { " 0" }),
              false => add(&mut text, number(*value)),
            }
          }
        }
        text.push('"');
      }
      Self::Commands(even, from, commands) => {
        text.push_str(if *even {
          "shape(evenodd from"
        } else {
          "shape(from"
        });
        for value in from.offsets() {
          add(&mut text, value);
        }
        for command in commands {
          text.push_str(", ");
          command.parts(&mut text, add);
        }
      }
      Self::Ray(ray) => {
        text.push_str("ray(");
        add(&mut text, ray.angle.into());
        text.push(' ');
        text.push_str(ray.size);
        if ray.contain {
          text.push_str(" contain");
        }
        if let Some(at) = &ray.at {
          text.push_str(" at");
          for value in at.offsets() {
            add(&mut text, value);
          }
        }
      }
    }
    text.push(')');
    Some((text, values))
  }

  // A path as the same `shape()`.
  fn as_shape(&self) -> Option<(bool, String)> {
    let Self::Path(even, segments) = self else {
      return None;
    };
    let mut text = String::from("shape(");
    let segments = absolute(segments);
    for (index, Segment(kind, args)) in segments.iter().enumerate() {
      let px = |at: usize| format!("{}px", number(args[at]));
      let point = |at: usize| format!("{} {}", px(at), px(at + 1));
      text.push_str(&match kind {
        b'M' if index == 0 => format!("from {}", point(0)),
        b'M' => format!(", move to {}", point(0)),
        b'L' => format!(", line to {}", point(0)),
        b'H' => format!(", hline to {}", px(0)),
        b'V' => format!(", vline to {}", px(0)),
        b'C' => format!(", curve to {} with {} / {}", point(4), point(0), point(2)),
        b'Q' => format!(", curve to {} with {}", point(2), point(0)),
        b'S' => format!(", smooth to {} with {}", point(2), point(0)),
        b'T' => format!(", smooth to {}", point(0)),
        b'A' => format!(
          ", arc to {} of {}{}{} rotate {}deg",
          point(5),
          point(0),
          if args[4] != 0. { " cw" } else { "" },
          if args[3] != 0. { " large" } else { "" },
          number(args[2])
        ),
        _ => ", close".into(),
      });
    }
    text.push(')');
    Some((*even, text))
  }
}

/// The text with gaps between two shapes and the pairs of amounts that fill them, when they interpolate.
pub(crate) fn between(from: &str, to: &str) -> Option<(String, Vec<[String; 2]>)> {
  let (a, b) = (Shaped::parse(from)?, Shaped::parse(to)?);
  if a.reference.unwrap_or("border-box") != b.reference.unwrap_or("border-box") {
    return None;
  }
  let (first, second) = (a.shape?, b.shape?);
  let [left, right] = [&first, &second].map(Shape::as_shape);
  let (first, second) = match (&first, &second, &left, &right) {
    (Shape::Path(..), Shape::Commands(even, ..), Some((rule, text)), _) if even == rule => {
      (Shape::parse(text)?, second)
    }
    (Shape::Commands(even, ..), Shape::Path(..), _, Some((rule, text))) if even == rule => {
      (first, Shape::parse(text)?)
    }
    _ => (first, second),
  };
  let at = |shape: &Shape| {
    matches!(
      shape,
      Shape::Circle(_, Some(_)) | Shape::Ellipse(_, Some(_))
    )
  };
  let at = at(&first) || at(&second);
  let ((mut text, mine), (other, theirs)) = (first.parts(at)?, second.parts(at)?);
  if text != other {
    // Like Chrome, a large or clockwise arc wins.
    let plain = |text: &str| text.replace(" cw", "").replace(" large", "");
    if !text.starts_with("shape(") || plain(&text) != plain(&other) {
      return None;
    }
    let [mine, theirs] = [&text, &other].map(|text| {
      text
        .split(", ")
        .map(|part| [" cw", " large"].map(|word| part.contains(word)))
        .collect::<Vec<_>>()
    });
    let commands: Vec<String> = plain(&text)
      .split(", ")
      .zip(mine.iter().zip(&theirs))
      .map(|(part, (a, b))| {
        let (end, close) = match part.strip_suffix(')') {
          Some(part) => (part, ")"),
          None => (part, ""),
        };
        let cw = if a[0] || b[0] { " cw" } else { "" };
        let large = if a[1] || b[1] { " large" } else { "" };
        format!("{end}{cw}{large}{close}")
      })
      .collect();
    text = commands.join(", ");
  }
  if let Some(reference) = a.reference {
    text.push(' ');
    text.push_str(reference);
  }
  Some((
    text,
    mine.into_iter().zip(theirs).map(|(a, b)| [a, b]).collect(),
  ))
}

const SIZES: [&str; 5] = [
  "closest-side",
  "closest-corner",
  "farthest-side",
  "farthest-corner",
  "sides",
];

impl Position<'_> {
  pub(crate) fn text(&self, units: Option<&Lengths>) -> String {
    match units {
      Some(units) => self.offsets().map(|part| amount(&part, units)).join(" "),
      None => self.written(),
    }
  }

  pub(crate) fn point(&self, units: &Lengths, size: [f32; 2]) -> Option<[f32; 2]> {
    let [x, y] = self.offsets();
    Some([units.resolve(&x, size[0])?, units.resolve(&y, size[1])?])
  }
}

impl<'a> Command<'a> {
  fn parse(value: &'a str) -> Option<Self> {
    let parts = slashes(&components(value, ' '));
    let mut command = Command {
      kind: match &*parts.first()?.to_ascii_lowercase() {
        "move" => b'm',
        "line" => b'l',
        "hline" => b'h',
        "vline" => b'v',
        "curve" => b'c',
        "smooth" => b's',
        "arc" => b'a',
        "close" => b'z',
        _ => return None,
      },
      to: false,
      end: Vec::new(),
      coordinate: "",
      controls: Vec::new(),
      radii: Vec::new(),
      clockwise: false,
      large: false,
      rotate: None,
    };
    if command.kind == b'z' {
      return (parts.len() == 1).then_some(command);
    }
    command.to = is(parts.get(1)?, "to");
    if !command.to && !is(parts[1], "by") {
      return None;
    }
    let rest = &parts[2..];
    let point = |parts: &[&'a str]| match command.to {
      true => Position::parse(parts),
      false => Position::parse(parts)
        .filter(|_| parts.len() == 2 && parts.iter().all(|part| length(part, false))),
    };
    // The end of the parts before one of these words.
    let until = |words: &[&str], from: usize| {
      (from..rest.len())
        .find(|&at| words.iter().any(|word| is(rest[at], word)))
        .unwrap_or(rest.len())
    };
    match command.kind {
      b'm' | b'l' => command.end = vec![point(rest)?],
      b'h' | b'v' => {
        let [one] = *rest else {
          return None;
        };
        let names: [&str; 5] = match command.kind {
          b'h' => ["left", "center", "right", "x-start", "x-end"],
          _ => ["top", "center", "bottom", "y-start", "y-end"],
        };
        if !(length(one, false) || command.to && names.iter().any(|name| is(one, name))) {
          return None;
        }
        command.coordinate = one;
      }
      b'c' | b's' => {
        let with = until(&["with"], 0);
        command.end = vec![point(&rest[..with])?];
        let mut controls = Vec::new();
        if with < rest.len() {
          for control in rest[with + 1..].split(|part| *part == "/") {
            let (control, from) = match control {
              [control @ .., word, from] if is(word, "from") => {
                let from = ["start", "end", "origin"]
                  .into_iter()
                  .find(|name| is(from, name))?;
                (control, from)
              }
              _ => (control, if command.to { "origin" } else { "start" }),
            };
            let at = Position::parse(control).filter(|_| {
              command.to || control.len() == 2 && control.iter().all(|part| length(part, false))
            })?;
            controls.push((at, from));
          }
        }
        if controls.len() > 2 - usize::from(command.kind == b's')
          || command.kind == b'c' && controls.is_empty()
        {
          return None;
        }
        command.controls = controls;
      }
      _ => {
        const WORDS: [&str; 6] = ["of", "cw", "ccw", "small", "large", "rotate"];
        let mut index = until(&WORDS, 0);
        command.end = vec![point(&rest[..index])?];
        let (mut sweep, mut size) = (false, false);
        while index < rest.len() {
          let end = until(&WORDS, index + 1);
          let args = &rest[index + 1..end];
          match &*rest[index].to_ascii_lowercase() {
            "of" if command.radii.is_empty() => {
              if !(1..=2).contains(&args.len()) || !args.iter().all(|arg| length(arg, false)) {
                return None;
              }
              command.radii = args.to_vec();
            }
            word @ ("cw" | "ccw") if !sweep && args.is_empty() => {
              (sweep, command.clockwise) = (true, word == "cw")
            }
            word @ ("small" | "large") if !size && args.is_empty() => {
              (size, command.large) = (true, word == "large")
            }
            "rotate" if command.rotate.is_none() => {
              let [turn] = *args else {
                return None;
              };
              if !angle(turn) {
                return None;
              }
              command.rotate = Some(turn);
            }
            _ => return None,
          }
          index = end;
        }
        if command.radii.is_empty() {
          return None;
        }
      }
    }
    Some(command)
  }

  fn head(&self) -> &'static str {
    match (self.kind, self.to) {
      (b'm', true) => "move to",
      (b'm', _) => "move by",
      (b'l', true) => "line to",
      (b'l', _) => "line by",
      (b'h', true) => "hline to",
      (b'h', _) => "hline by",
      (b'v', true) => "vline to",
      (b'v', _) => "vline by",
      (b'c', true) => "curve to",
      (b'c', _) => "curve by",
      (b's', true) => "smooth to",
      (b's', _) => "smooth by",
      (b'a', true) => "arc to",
      (b'a', _) => "arc by",
      _ => "close",
    }
  }

  fn anchor(&self, from: &str) -> String {
    match from == if self.to { "origin" } else { "start" } {
      true => String::new(),
      false => format!(" from {from}"),
    }
  }

  fn text(&self, units: Option<&Lengths>) -> String {
    let one = |part: &str| match units {
      Some(units) => amount(part, units),
      None => specified(part),
    };
    let mut out = self.head().to_owned();
    for point in &self.end {
      out.push(' ');
      out.push_str(&point.text(units));
    }
    if !self.coordinate.is_empty() {
      out.push(' ');
      out.push_str(&match units {
        Some(units) => amount(&coordinate(self.coordinate), units),
        None => one(self.coordinate),
      });
    }
    for (index, (point, from)) in self.controls.iter().enumerate() {
      out.push_str(if index == 0 { " with " } else { " / " });
      out.push_str(&point.text(units));
      out.push_str(&self.anchor(from));
    }
    for (index, radius) in self.radii.iter().enumerate() {
      out.push_str(if index == 0 { " of " } else { " " });
      out.push_str(&one(radius));
    }
    out.push_str(if self.clockwise { " cw" } else { "" });
    out.push_str(if self.large { " large" } else { "" });
    if let Some(turn) = self.rotate {
      let turn = match units.and_then(|units| units.math(turn, 0., &[])) {
        Some((turn, _)) => {
          let mut out = String::new();
          write_printed(turn, &mut out);
          out + "deg"
        }
        None => specified(turn),
      };
      if turn != "0deg" {
        out.push_str(" rotate ");
        out.push_str(&turn);
      }
    }
    out
  }

  fn parts(&self, text: &mut String, add: &mut dyn FnMut(&mut String, String)) {
    text.push_str(self.head());
    for value in self.end.iter().flat_map(Position::offsets) {
      add(text, value);
    }
    if !self.coordinate.is_empty() {
      add(text, coordinate(self.coordinate));
    }
    for (index, (point, from)) in self.controls.iter().enumerate() {
      text.push_str(if index == 0 { " with" } else { " /" });
      for value in point.offsets() {
        add(text, value);
      }
      text.push_str(&self.anchor(from));
    }
    if let [first, ..] = self.radii[..] {
      text.push_str(" of");
      add(text, unit(first).into());
      add(text, unit(self.radii.get(1).unwrap_or(&first)).into());
      text.push_str(" rotate");
      add(text, self.rotate.unwrap_or("0deg").into());
      text.push_str(if self.clockwise { " cw" } else { "" });
      text.push_str(if self.large { " large" } else { "" });
    }
  }

  // Returns whether the curve was quadratic.
  fn draw(&self, pen: &mut Pen, units: &Lengths, size: [f32; 2], quad: bool) -> Option<bool> {
    let start = pen.at;
    let shift = |at: [f32; 2], by: [f32; 2]| [at[0] + by[0], at[1] + by[1]];
    let end = match (self.end.first(), self.to) {
      (Some(end), true) => end.point(units, size)?,
      (Some(end), false) => shift(start, end.point(units, size)?),
      (None, _) if self.coordinate.is_empty() => start,
      (None, to) => {
        let axis = usize::from(self.kind == b'v');
        let value = units.resolve(&coordinate(self.coordinate), size[axis])?;
        let mut end = start;
        end[axis] = if to { value } else { start[axis] + value };
        end
      }
    };
    let mut controls = Vec::with_capacity(2);
    for (point, from) in &self.controls {
      let point = point.point(units, size)?;
      controls.push(match *from {
        "start" => shift(start, point),
        "end" => shift(end, point),
        _ => point,
      });
    }
    match (self.kind, &controls[..]) {
      (b'm', _) => pen.go(Op::Move(end)),
      (b'c', [one]) => pen.go(Op::Quad(*one, end)),
      (b'c', [one, two]) => pen.go(Op::Cubic(*one, *two, end)),
      (b's', [two]) => {
        let one = if quad { start } else { pen.reflected() };
        pen.go(Op::Cubic(one, *two, end))
      }
      (b's', _) => {
        let one = if quad { pen.reflected() } else { start };
        pen.go(Op::Quad(one, end))
      }
      (b'a', _) => {
        // Like Chrome, one percentage takes the diagonal, and two take their axes.
        let base = size[0].hypot(size[1]) / std::f32::consts::SQRT_2;
        let [x, y] = match self.radii[..] {
          [x, y] => [units.resolve(x, size[0])?, units.resolve(y, size[1])?],
          _ => [units.resolve(self.radii[0], base)?; 2],
        };
        let turn = match self.rotate {
          Some(turn) => units.math(turn, 0., &[])?.0 as f32,
          None => 0.,
        };
        pen.arc([x, y], turn, self.large, self.clockwise, end);
      }
      (b'z', _) => pen.go(Op::Close),
      _ => pen.go(Op::Line(end)),
    }
    Some(matches!((self.kind, controls.len()), (b'c', 1) | (b's', 0)))
  }
}

/// A rectangle with rounded corners.
pub(crate) fn rounded(rect: [f32; 4], radii: [[f32; 2]; 4]) -> Vec<Op> {
  let mut pen = Pen::default();
  pen.rounded(rect, radii);
  pen.ops
}

#[derive(Default)]
struct Pen {
  ops: Vec<Op>,
  at: [f32; 2],
  start: [f32; 2],
  control: [f32; 2],
}

impl Pen {
  fn go(&mut self, op: Op) {
    // The rasterizer drops a path with huge coordinates.
    let near = |at: [f32; 2]| at.map(|value| value.clamp(-1e6, 1e6));
    let op = match op {
      Op::Move(end) => Op::Move(near(end)),
      Op::Line(end) => Op::Line(near(end)),
      Op::Quad(a, end) => Op::Quad(near(a), near(end)),
      Op::Cubic(a, b, end) => Op::Cubic(near(a), near(b), near(end)),
      Op::Close => op,
    };
    // A line after a close starts a new contour at the start of the closed one.
    if matches!(self.ops.last(), Some(Op::Close)) && !matches!(op, Op::Move(_) | Op::Close) {
      self.ops.push(Op::Move(self.at));
    }
    (self.control, self.at) = match op {
      Op::Move(end) => {
        self.start = end;
        (end, end)
      }
      Op::Line(end) => (end, end),
      Op::Quad(control, end) | Op::Cubic(_, control, end) => (control, end),
      Op::Close => (self.start, self.start),
    };
    self.ops.push(op);
  }

  fn reflected(&self) -> [f32; 2] {
    std::array::from_fn(|axis| 2. * self.at[axis] - self.control[axis])
  }

  // An elliptical arc of SVG, as cubic curves.
  fn arc(&mut self, radii: [f32; 2], turn: f32, large: bool, sweep: bool, end: [f32; 2]) {
    let start = self.at;
    let [mut rx, mut ry] = radii.map(f32::abs);
    if rx == 0. || ry == 0. || start == end {
      return self.go(Op::Line(end));
    }
    let (sin, cos) = turn.to_radians().sin_cos();
    let half = [(start[0] - end[0]) / 2., (start[1] - end[1]) / 2.];
    let x = cos * half[0] + sin * half[1];
    let y = cos * half[1] - sin * half[0];
    let over = (x / rx).powi(2) + (y / ry).powi(2);
    if over > 1. {
      (rx, ry) = (rx * over.sqrt(), ry * over.sqrt());
    }
    let d = (rx * y).powi(2) + (ry * x).powi(2);
    let k = (((rx * ry).powi(2) - d) / d).max(0.).sqrt() * if large == sweep { -1. } else { 1. };
    let (cx, cy) = (k * rx * y / ry, -k * ry * x / rx);
    let first = ((y - cy) / ry).atan2((x - cx) / rx);
    let mut sweep_angle = ((-y - cy) / ry).atan2((-x - cx) / rx) - first;
    if sweep && sweep_angle < 0. {
      sweep_angle += TAU;
    } else if !sweep && sweep_angle > 0. {
      sweep_angle -= TAU;
    }
    let center = [
      cos * cx - sin * cy + (start[0] + end[0]) / 2.,
      sin * cx + cos * cy + (start[1] + end[1]) / 2.,
    ];
    let count = (sweep_angle.abs() / FRAC_PI_2 - 1e-3).ceil().max(1.);
    let step = sweep_angle / count;
    let tangent = 4. / 3. * (step / 4.).tan();
    let place = |x: f32, y: f32| {
      [
        center[0] + cos * rx * x - sin * ry * y,
        center[1] + sin * rx * x + cos * ry * y,
      ]
    };
    for index in 0..count as usize {
      let (s0, c0) = (first + step * index as f32).sin_cos();
      let (s1, c1) = (first + step * (index + 1) as f32).sin_cos();
      let last = index + 1 == count as usize;
      self.go(Op::Cubic(
        place(c0 - tangent * s0, s0 + tangent * c0),
        place(c1 + tangent * s1, s1 - tangent * c1),
        if last { end } else { place(c1, s1) },
      ));
    }
  }

  fn rounded(&mut self, [x, y, w, h]: [f32; 4], radii: [[f32; 2]; 4]) {
    let [a, b, c, d] = radii;
    let corner = |pen: &mut Self, radius: [f32; 2], end: [f32; 2]| {
      if radius[0] > 0. && radius[1] > 0. {
        pen.arc(radius, 0., false, true, end);
      } else {
        pen.go(Op::Line(end));
      }
    };
    self.go(Op::Move([x + a[0], y]));
    self.go(Op::Line([x + w - b[0], y]));
    corner(self, b, [x + w, y + b[1]]);
    self.go(Op::Line([x + w, y + h - c[1]]));
    corner(self, c, [x + w - c[0], y + h]);
    self.go(Op::Line([x + d[0], y + h]));
    corner(self, d, [x, y + h - d[1]]);
    self.go(Op::Line([x, y + a[1]]));
    corner(self, a, [x + a[0], y]);
    self.go(Op::Close);
  }

  fn polygon(&mut self, points: &[[f32; 2]], round: f32) {
    for (index, &point) in points.iter().enumerate() {
      let to = |end: [f32; 2]| {
        let [x, y] = [end[0] - point[0], end[1] - point[1]];
        let length = x.hypot(y);
        (length, [x / length, y / length])
      };
      let (before, u) = to(points[(index + points.len() - 1) % points.len()]);
      let (after, v) = to(points[(index + 1) % points.len()]);
      let half = (u[0] * v[0] + u[1] * v[1]).clamp(-1., 1.).acos() / 2.;
      // An arc touches both edges, at most at their middles.
      let cut = (round / half.tan()).min(before.min(after) / 2.);
      let radius = cut * half.tan();
      let round = radius > 1e-3 && radius.is_finite() && half < FRAC_PI_2 - 1e-3;
      let start = match round {
        true => [point[0] + u[0] * cut, point[1] + u[1] * cut],
        false => point,
      };
      self.go(match index {
        0 => Op::Move(start),
        _ => Op::Line(start),
      });
      if round {
        let sweep = u[0] * v[1] - u[1] * v[0] < 0.;
        let end = [point[0] + v[0] * cut, point[1] + v[1] * cut];
        self.arc([radius; 2], 0., false, sweep, end);
      }
    }
    self.go(Op::Close);
  }
}

/// An `offset-rotate` value: whether it follows the path, and its angle in degrees.
pub(crate) fn turn(value: &str, units: &Lengths) -> Option<(bool, f64)> {
  let (mut auto, mut degrees) = (None, None);
  for part in components(value, ' ') {
    if auto.is_none() && is(part, "auto") {
      auto = Some(0.);
    } else if auto.is_none() && is(part, "reverse") {
      auto = Some(180.);
    } else {
      let (angle, kind) = units.math(part, f32::NAN, &[])?;
      if degrees.is_some() || kind != Kind::Angle {
        return None;
      }
      degrees = Some(angle);
    }
  }
  (auto.is_some() || degrees.is_some())
    .then(|| (auto.is_some(), auto.unwrap_or(0.) + degrees.unwrap_or(0.)))
}

/// The track of the outline that `outline` makes for a text and what else it reads, from a small cache, as
/// animations read it in each frame.
pub(crate) fn track(
  text: &str,
  reads: &str,
  outline: impl FnOnce() -> Option<Vec<Op>>,
) -> Option<std::rc::Rc<Track>> {
  type Tracks = Vec<(String, String, std::rc::Rc<Track>)>;
  thread_local! {
      static TRACKS: std::cell::RefCell<Tracks> = Default::default();
  }
  TRACKS.with_borrow_mut(|tracks| {
    let known = |entry: &(String, String, _)| entry.0 == text && entry.1 == reads;
    if let Some(entry) = tracks.iter().find(|entry| known(entry)) {
      return Some(entry.2.clone());
    }
    let track = std::rc::Rc::new(Track::new(&outline()?));
    tracks.push((text.to_owned(), reads.to_owned(), track.clone()));
    // The last track stays, however long it is.
    let mut size: usize = tracks
      .iter()
      .map(|(text, _, track)| text.len() + track.points.len() * size_of::<Point>())
      .sum();
    while tracks.len() > 1 && (tracks.len() > 16 || size > 1 << 18) {
      let (text, _, track) = tracks.remove(0);
      size -= text.len() + track.points.len() * size_of::<Point>();
    }
    Some(track)
  })
}

/// The track of a `path()`.
pub(crate) fn path_track(value: &str) -> Option<std::rc::Rc<Track>> {
  value
    .get(..5)
    .filter(|start| start.eq_ignore_ascii_case("path("))?;
  track(value, "", || {
    (Shaped::parse(value)?.shape?).outline(&Lengths::new(16., 0., 0.), [0.; 2], None)
  })
}

// The segments of SVG path data, or None when it has an error.
fn segments(data: &str) -> Option<Vec<Segment>> {
  let bytes = data.as_bytes();
  let mut at = 0;
  let space = |at: &mut usize, comma: bool| {
    let mut comma = comma;
    while let Some(&byte) = bytes.get(*at) {
      if byte == b',' && comma {
        comma = false;
      } else if !byte.is_ascii_whitespace() {
        break;
      }
      *at += 1;
    }
  };
  let number = |at: &mut usize| -> Option<f64> {
    let start = *at;
    let mut end = start + usize::from(matches!(bytes.get(start), Some(b'+' | b'-')));
    let digits = |end: &mut usize| {
      let from = *end;
      while bytes.get(*end).is_some_and(u8::is_ascii_digit) {
        *end += 1;
      }
      *end - from
    };
    let mut count = digits(&mut end);
    if bytes.get(end) == Some(&b'.') {
      end += 1;
      count += digits(&mut end);
    }
    if count == 0 {
      return None;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
      let mut exponent = end + 1;
      exponent += usize::from(matches!(bytes.get(exponent), Some(b'+' | b'-')));
      if digits(&mut exponent) > 0 {
        end = exponent;
      }
    }
    *at = end;
    data[start..end]
      .parse()
      .ok()
      .filter(|value: &f64| (*value as f32).is_finite())
  };
  let mut out: Vec<Segment> = Vec::new();
  space(&mut at, false);
  while let Some(&kind) = bytes.get(at) {
    let count = match kind.to_ascii_uppercase() {
      b'M' | b'L' | b'T' => 2,
      b'H' | b'V' => 1,
      b'C' => 6,
      b'S' | b'Q' => 4,
      b'A' => 7,
      b'Z' => 0,
      _ => return None,
    };
    if out.is_empty() && !matches!(kind, b'M' | b'm') {
      return None;
    }
    at += 1;
    let mut kind = kind;
    loop {
      let mut args = Vec::with_capacity(count);
      for index in 0..count {
        space(&mut at, index > 0);
        // A flag is one digit, also without a separator after it.
        if count == 7 && matches!(index, 3 | 4) {
          args.push(match bytes.get(at)? {
            b'0' => 0.,
            b'1' => 1.,
            _ => return None,
          });
          at += 1;
        } else {
          args.push(number(&mut at)?);
        }
      }
      out.push(Segment(kind, args));
      space(&mut at, count > 0);
      let more = count > 0
        && bytes
          .get(at)
          .is_some_and(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.'));
      if !more {
        break;
      }
      kind = match kind {
        b'M' => b'L',
        b'm' => b'l',
        kind => kind,
      };
    }
  }
  (!out.is_empty()).then_some(out)
}

fn absolute(segments: &[Segment]) -> Vec<Segment> {
  let (mut at, mut start) = ([0.; 2], [0.; 2]);
  segments
    .iter()
    .map(|Segment(kind, args)| {
      let relative = kind.is_ascii_lowercase();
      let kind = kind.to_ascii_uppercase();
      let mut args = args.clone();
      let first = if kind == b'A' { 5 } else { 0 };
      if relative {
        for (index, value) in args.iter_mut().enumerate().skip(first) {
          let axis = match kind {
            b'V' => 1,
            b'A' => (index - 5) % 2,
            _ => index % 2,
          };
          *value += at[axis];
        }
      }
      match (kind, &args[..]) {
        (b'H', [x]) => at[0] = *x,
        (b'V', [y]) => at[1] = *y,
        (b'Z', _) => at = start,
        (_, [.., x, y]) => at = [*x, *y],
        _ => {}
      }
      if kind == b'M' {
        start = at;
      }
      Segment(kind, args)
    })
    .collect()
}

fn data(segments: &[Segment]) -> String {
  let mut out = String::new();
  for Segment(kind, args) in segments {
    if !out.is_empty() {
      out.push(' ');
    }
    out.push(if *kind == b'z' { 'Z' } else { *kind as char });
    for value in args {
      out.push(' ');
      write_printed(*value, &mut out);
    }
  }
  out
}

/// The ops as SVG path data.
pub(crate) fn svg(ops: &[Op]) -> String {
  use std::fmt::Write;
  let mut out = String::new();
  for op in ops {
    let _ = match op {
      Op::Move([x, y]) => write!(out, "M{x} {y}"),
      Op::Line([x, y]) => write!(out, "L{x} {y}"),
      Op::Quad([a, b], [x, y]) => write!(out, "Q{a} {b} {x} {y}"),
      Op::Cubic([a, b], [c, d], [x, y]) => write!(out, "C{a} {b} {c} {d} {x} {y}"),
      Op::Close => write!(out, "Z"),
    };
  }
  out
}

pub(crate) fn path(ops: &[Op]) -> Option<tiny_skia::Path> {
  let mut path = tiny_skia::PathBuilder::new();
  for op in ops {
    match *op {
      Op::Move([x, y]) => path.move_to(x, y),
      Op::Line([x, y]) => path.line_to(x, y),
      Op::Quad([a, b], [x, y]) => path.quad_to(a, b, x, y),
      Op::Cubic([a, b], [c, d], [x, y]) => path.cubic_to(a, b, c, d, x, y),
      Op::Close => path.close(),
    }
  }
  path.finish()
}

/// A rectangle around the ops, as its left, top, right and bottom.
pub(crate) fn reach(ops: &[Op]) -> Option<[f32; 4]> {
  let mut points = Vec::with_capacity(ops.len());
  for op in ops {
    match *op {
      Op::Move(end) | Op::Line(end) => points.push(end),
      Op::Quad(a, end) => points.extend([a, end]),
      Op::Cubic(a, b, end) => points.extend([a, b, end]),
      Op::Close => {}
    }
  }
  let first = *points.first()?;
  let all = points.iter().fold(
    [first[0], first[1], first[0], first[1]],
    |[left, top, right, bottom], &[x, y]| [left.min(x), top.min(y), right.max(x), bottom.max(y)],
  );
  all.iter().all(|value| value.is_finite()).then_some(all)
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
  (b[0] - a[0]).hypot(b[1] - a[1])
}

fn direction(a: [f32; 2], b: [f32; 2]) -> f32 {
  (b[1] - a[1]).atan2(b[0] - a[0]).to_degrees()
}

/// An outline in short lines. Each has its end, the distance there, and the directions at its start and end.
pub(crate) struct Track {
  points: Vec<Point>,
  closed: bool,
}

type Point = ([f32; 2], f32, [f32; 2]);

impl Track {
  pub(crate) fn new(ops: &[Op]) -> Self {
    let mut track = Self {
      points: Vec::new(),
      closed: matches!(ops.last(), Some(Op::Close)),
    };
    let mut start = [0.; 2];
    for op in ops {
      let from = track.points.last().map_or([0.; 2], |point| point.0);
      match *op {
        // A move is a point at the distance of the point before it.
        Op::Move(end) => {
          start = end;
          track.points.push((end, track.length(), [0.; 2]));
        }
        Op::Line(end) => track.line(end, None),
        Op::Close => track.line(start, None),
        Op::Quad(a, end) => {
          let hull = distance(from, a) + distance(a, end);
          track.curve(hull, &|t| {
            let u = 1. - t;
            std::array::from_fn(|k| u * u * from[k] + 2. * u * t * a[k] + t * t * end[k])
          });
        }
        Op::Cubic(a, b, end) => {
          let hull = distance(from, a) + distance(a, b) + distance(b, end);
          track.curve(hull, &|t| {
            let u = 1. - t;
            std::array::from_fn(|k| {
              u * u * u * from[k]
                + 3. * u * u * t * a[k]
                + 3. * u * t * t * b[k]
                + t * t * t * end[k]
            })
          });
        }
      }
    }
    track
  }

  fn line(&mut self, end: [f32; 2], directions: Option<[f32; 2]>) {
    let from = self.points.last().map_or([0.; 2], |point| point.0);
    let length = distance(from, end);
    if length > 0. {
      let directions = directions.unwrap_or_else(|| [direction(from, end); 2]);
      self.points.push((end, self.length() + length, directions));
    }
  }

  fn curve(&mut self, hull: f32, place: &dyn Fn(f32) -> [f32; 2]) {
    let steps = (hull.sqrt() * 4.).clamp(8., 256.).ceil();
    let toward = |t: f32| direction(place((t - 1e-2).max(0.)), place((t + 1e-2).min(1.)));
    let mut before = toward(0.);
    for step in 1..=steps as usize {
      let t = step as f32 / steps;
      let after = toward(t);
      self.line(place(t), Some([before, after]));
      before = after;
    }
  }

  pub(crate) fn points(&self) -> Vec<[f32; 2]> {
    self.points.iter().map(|point| point.0).collect()
  }

  pub(crate) fn length(&self) -> f32 {
    self.points.last().map_or(0., |point| point.1)
  }

  /// The point at a distance, and the angle of the outline there in degrees.
  pub(crate) fn point(&self, distance: f32) -> ([f32; 2], f32) {
    let length = self.length();
    let distance = match self.closed && length > 0. {
      true => distance.rem_euclid(length),
      false => distance.clamp(0., length),
    };
    let end = self
      .points
      .partition_point(|point| point.1 < distance)
      .max(1)
      .min(self.points.len().saturating_sub(1));
    // Skip the moves, which have no length.
    let end = (end..self.points.len())
      .find(|&index| index > 0 && self.points[index].1 > self.points[index - 1].1)
      .or_else(|| {
        (1..self.points.len())
          .rev()
          .find(|&index| self.points[index].1 > self.points[index - 1].1)
      });
    let Some(end) = end else {
      return (self.points.first().map_or([0.; 2], |point| point.0), 0.);
    };
    let ((a, from, _), (b, to, [first, last])) = (self.points[end - 1], self.points[end]);
    let t = ((distance - from) / (to - from)).clamp(0., 1.);
    (
      [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t],
      first + ((last - first + 180.).rem_euclid(360.) - 180.) * t,
    )
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn shapes_serialize_like_chrome() {
    let units = Lengths::new(20., 800., 600.);
    for (value, specified, computed) in [
      (
        "xywh(1em 10% 60px 50%)",
        "xywh(1em 10% 60px 50%)",
        "inset(10% calc(100% - 80px) 40% 20px)",
      ),
      (
        "rect(auto 20px 30px auto round 1px 2px)",
        "rect(auto 20px 30px auto round 1px 2px)",
        "inset(0% calc(100% - 20px) calc(100% - 30px) 0% round 1px 2px)",
      ),
      (
        "inset(0 round 20px/calc(4px/2))",
        "inset(0px round 20px / calc(2px))",
        "inset(0px round 20px / 2px)",
      ),
      (
        "shape(from 0 0,curve to 1px 2px with 100% 0/0 100%)",
        "shape(from 0px 0px, curve to 1px 2px with 100% 0px / 0px 100%)",
        "shape(from 0px 0px, curve to 1px 2px with 100% 0px / 0px 100%)",
      ),
      (
        "xywh(1px 2px 3px calc(-4px))",
        "xywh(1px 2px 3px calc(-4px))",
        "inset(2px calc(100% - 4px) calc(100% - 2px) 1px)",
      ),
      ("circle(calc(-1px))", "circle(calc(-1px))", "circle(0px)"),
      ("path('M0,0,')", "path(\"M 0 0\")", "path(\"M 0 0\")"),
      ("inset(10% round 0)", "inset(10%)", "inset(10%)"),
      (
        "circle(closest-side at left 10px top 5px)",
        "circle(at left 10px top 5px)",
        "circle(at 10px 5px)",
      ),
      (
        "circle(50% at top)",
        "circle(50% at center top)",
        "circle(50% at 50% 0%)",
      ),
      (
        "content-box circle()",
        "circle() content-box",
        "circle() content-box",
      ),
      ("circle() border-box", "circle()", "circle()"),
      (
        "polygon(nonzero, 0 0, 1em 50%)",
        "polygon(0px 0px, 1em 50%)",
        "polygon(0px 0px, 20px 50%)",
      ),
      (
        "path('m 0 0 l10 10 h5 a1 2 3 014 5z')",
        "path(\"m 0 0 l 10 10 h 5 a 1 2 3 0 1 4 5 Z\")",
        "path(\"M 0 0 L 10 10 H 15 A 1 2 3 0 1 19 15 Z\")",
      ),
      (
        "shape(nonzero from top left, hline to center, curve to 10px 1em with 5px 5px from start / 6px 7px from origin, arc by 1px 2px rotate 0.5turn small cw of 5px, close)",
        "shape(from left top, hline to center, curve to 10px 1em with 5px 5px from start / 6px 7px, arc by 1px 2px of 5px cw rotate 0.5turn, close)",
        "shape(from 0% 0%, hline to 50%, curve to 10px 20px with 5px 5px from start / 6px 7px, arc by 1px 2px of 5px cw rotate 180deg, close)",
      ),
      (
        "ray(contain 45deg sides at center)",
        "ray(45deg sides contain at center center)",
        "ray(45deg sides contain at 50% 50%)",
      ),
    ] {
      let shaped = Shaped::parse(value).unwrap_or_else(|| panic!("{value}"));
      assert_eq!(shaped.text(None), specified);
      assert_eq!(shaped.text(Some(&units)), computed);
    }
    for value in [
      "xywh(0 0 -1px 5px)",
      "rect(1px 2px 3px)",
      "inset(auto)",
      "inset(1px round -1px)",
      "circle(-1px)",
      "circle() circle()",
      "ellipse(10px)",
      "polygon(0)",
      "polygon(0 0,)",
      "inset(0 round 1px/)",
      "path('M1e99 0')",
      "path('')",
      "path('M0 0 foo')",
      "path('L0 0')",
      "shape(from 0 0)",
      "shape(line to 1px 1px)",
      "shape(from 0 0, curve to 5px 5px)",
      "shape(from 0 0, arc by 1px 1px)",
      "shape(from 0 0, line by center center)",
      "ray()",
      "text",
    ] {
      assert_eq!(Shaped::parse(value), None, "{value}");
    }
  }

  #[test]
  fn tracks_give_points_and_angles() {
    let near = |value: &str, distance: f32, [x, y]: [f32; 2], angle: f32| {
      let (at, turn) = path_track(value).unwrap().point(distance);
      assert!(
        (at[0] - x).hypot(at[1] - y) < 0.1
          && ((turn - angle + 180.).rem_euclid(360.) - 180.).abs() < 0.5,
        "{value} {distance}: {at:?} {turn}"
      );
    };
    near("path('M0 0H60V40H0Z')", 80., [60., 20.], 90.);
    // A closed path goes around, and an open one ends.
    near("path('M0 0H60V40H0Z')", -10., [0., 10.], -90.);
    near("path('M0 0H60V40')", 500., [60., 40.], 90.);
    near("path('M0 0H60V40')", -5., [0., 0.], 0.);
    near("path('M0 0h60M0 40h60')", 90., [30., 40.], 0.);
    near("path('M10 0A10 10 0 0 1 -10 0')", 15.708, [0., 10.], 180.);
  }
}
