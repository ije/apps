pub(crate) mod anchor;
pub mod color;
pub(crate) mod counter;
pub(crate) mod easing;
pub(crate) mod font;
pub(crate) mod grammar;
pub(crate) mod image;
pub(crate) mod resolved;
pub(crate) mod shape;
pub(crate) mod shorthand;
pub(crate) mod specified;
pub(crate) mod timeline;
pub(crate) mod tokens;
pub(crate) mod transform;
pub(crate) mod units;

use crate::css::{
  value::color::paint, value::tokens::components, value::units::Kind, value::units::Lengths,
};

// The superellipse parameter of a `corner-shape` value, like 2 for `squircle`.
pub(crate) fn corner_shape(value: &str) -> Option<f64> {
  let lower = value.trim().to_ascii_lowercase();
  Some(match lower.as_str() {
    "round" => 1.,
    "squircle" => 2.,
    "square" => f64::INFINITY,
    "bevel" => 0.,
    "scoop" => -1.,
    "notch" => f64::NEG_INFINITY,
    _ => match lower
      .strip_prefix("superellipse(")?
      .strip_suffix(')')?
      .trim()
    {
      "infinity" => f64::INFINITY,
      "-infinity" => f64::NEG_INFINITY,
      inner => match Lengths::new(16., 100., 100.).math(inner, f32::NAN, &[])? {
        (k, Kind::Number) if !k.is_nan() => k,
        _ => return None,
      },
    },
  })
}

// The basis and the calculation of `calc-size()`.
pub(crate) fn calc_size(value: &str) -> Option<(&str, &str)> {
  let (name, args) = value.trim().strip_suffix(')')?.split_once('(')?;
  let [basis, size] = components(args, ',')[..] else {
    return None;
  };
  name
    .eq_ignore_ascii_case("calc-size")
    .then(|| (basis.trim(), size.trim()))
}

// Whether the size reads the content of its box: not a length or the space around it.
pub(crate) fn content_size(value: &str) -> bool {
  match calc_size(value) {
    Some((basis, _)) => content_size(basis) || matches!(basis, "auto" | "content"),
    None => matches!(value, "min-content" | "max-content" | "fit-content"),
  }
}

// Whether a height is the one that `auto` gives in a block.
pub(crate) fn auto_size(value: &str) -> bool {
  match calc_size(value) {
    Some((basis, size)) => size.trim() == "size" && auto_size(basis),
    None => value == "auto" || content_size(value),
  }
}

// A calculation of `calc-size()` with a value for its `size`.
pub(crate) fn with_size(calculation: &str, size: &str) -> String {
  let word = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '%' | '.');
  let mut out = String::new();
  let mut rest = calculation;
  while !rest.is_empty() {
    let end = rest.find(|c| !word(c)).unwrap_or(rest.len()).max(1);
    match &rest[..end] {
      part if part.eq_ignore_ascii_case("size") => {
        out.push_str(size);
      }
      part => out.push_str(part),
    }
    rest = &rest[end..];
  }
  out
}

pub fn length(value: &str, base: f32, font: f32, vw: f32, vh: f32) -> f32 {
  Lengths::new(font, vw, vh)
    .parse(value, base)
    .map(|(value, _)| value)
    .unwrap_or(0.)
}

pub(crate) fn area_cells(row: &str) -> Vec<&str> {
  let mut cells = Vec::new();
  let mut rest = row.trim_start();
  while let Some(first) = rest.chars().next() {
    let end = rest
      .find(|c: char| c.is_whitespace() || (c == '.') != (first == '.'))
      .unwrap_or(rest.len());
    cells.push(if first == '.' { "." } else { &rest[..end] });
    rest = rest[end..].trim_start();
  }
  cells
}

type ShadowMemo = rustc_hash::FxHashMap<u64, (Box<str>, bool, [u32; 20], Vec<renderer::BoxShadow>)>;

thread_local! {
    static SHADOWS: std::cell::RefCell<ShadowMemo> = Default::default();
}

pub(crate) fn box_shadows(sizes: &Lengths, value: &str, inset: bool) -> Vec<renderer::BoxShadow> {
  if value == "none" {
    return Vec::new();
  }
  let [vw, vh] = sizes.viewport.map(f32::to_bits);
  let [cw, ch] = sizes
    .container
    .map_or([u32::MAX; 2], |size| size.map(f32::to_bits));
  let mut units = [0; 20];
  units[..8].copy_from_slice(&sizes.font.bits());
  units[8..16].copy_from_slice(&sizes.root.bits());
  units[16..].copy_from_slice(&[vw, vh, cw, ch]);
  let hash = {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    (value, inset, units).hash(&mut hasher);
    hasher.finish()
  };
  if let Some(shadows) = SHADOWS.with_borrow(|memo| {
    memo
      .get(&hash)
      .filter(|(old, flag, bits, _)| **old == *value && *flag == inset && *bits == units)
      .map(|(.., shadows)| shadows.clone())
  }) {
    return shadows;
  }
  let mut shadows = Vec::new();
  'parse: for shadow in components(value, ',').into_iter().rev() {
    let mut lengths = Vec::new();
    let mut ink = renderer::Color::BLACK;
    let parts = components(shadow, ' ');
    if parts.contains(&"inset") != inset {
      continue;
    }
    for part in parts.into_iter().filter(|part| *part != "inset") {
      if let Some(color) = paint(part) {
        ink = color;
      } else if let Some(value) = sizes.resolve(part, sizes.viewport[0]) {
        lengths.push(value);
      } else {
        shadows.clear();
        break 'parse;
      }
    }
    if !(2..=4).contains(&lengths.len()) {
      shadows.clear();
      break 'parse;
    }
    shadows.push(renderer::BoxShadow {
      color: ink,
      offset: renderer::point(renderer::px(lengths[0]), renderer::px(lengths[1])),
      // The renderer uses the blur radius as the Gaussian sigma, which is half the CSS blur radius.
      blur_radius: renderer::px(lengths.get(2).copied().unwrap_or(0.).max(0.) / 2.),
      spread_radius: renderer::px(lengths.get(3).copied().unwrap_or(0.)),
      inset,
    });
  }
  SHADOWS.with_borrow_mut(|memo| {
    if memo.len() >= 1024 {
      memo.clear();
    }
    memo.insert(hash, (value.into(), inset, units, shadows.clone()));
  });
  shadows
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Filter(pub [[f32; 4]; 3]);

impl Filter {
  // One filter function: `effects::filter::filters` splits the list and composes the matrices.
  pub fn parse(value: &str, units: &Lengths) -> (Option<Self>, f32) {
    let identity = Self([[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]]);
    let Some((name, args)) = value
      .strip_suffix(')')
      .and_then(|part| part.split_once('('))
    else {
      return (None, 1.);
    };
    let args = args.trim();
    let amount = if args.is_empty() {
      Some(1.)
    } else if let Some(percent) = args.strip_suffix('%') {
      percent.trim().parse::<f32>().ok().map(|value| value / 100.)
    } else {
      units
        .math(args, 0., &[])
        .filter(|(_, kind)| *kind == Kind::Number)
        .map(|(value, _)| value as f32)
    }
    .filter(|value| value.is_finite() && *value >= 0.);
    let blend = |rows: [[f32; 3]; 3], amount: f32| {
      Self(std::array::from_fn(|i| {
        let mut row = [0.; 4];
        for (j, value) in row.iter_mut().take(3).enumerate() {
          *value = rows[i][j] + amount * (f32::from(i == j) - rows[i][j]);
        }
        row
      }))
    };
    let diagonal = |scale: f32, offset: f32| {
      Self(std::array::from_fn(|i| {
        let mut row = [0., 0., 0., offset];
        row[i] = scale;
        row
      }))
    };
    let matrix = match (name.to_ascii_lowercase().as_str(), amount) {
      ("grayscale", Some(amount)) => blend([[0.2126, 0.7152, 0.0722]; 3], 1. - amount.min(1.)),
      ("sepia", Some(amount)) => blend(
        [
          [0.393, 0.769, 0.189],
          [0.349, 0.686, 0.168],
          [0.272, 0.534, 0.131],
        ],
        1. - amount.min(1.),
      ),
      ("saturate", Some(amount)) => blend([[0.213, 0.715, 0.072]; 3], amount),
      ("invert", Some(amount)) => diagonal(1. - 2. * amount.min(1.), amount.min(1.)),
      ("brightness", Some(amount)) => diagonal(amount, 0.),
      ("contrast", Some(amount)) => diagonal(amount, 0.5 - 0.5 * amount),
      ("opacity", Some(amount)) => return (None, amount.min(1.)),
      ("hue-rotate", _) => {
        let Some((angle, _)) = (if args.is_empty() {
          Some((0., Kind::Angle))
        } else {
          units
            .math(args, 0., &[])
            .filter(|(value, kind)| *kind == Kind::Angle || *kind == Kind::Number && *value == 0.)
        }) else {
          return (None, 1.);
        };
        let (sin, cos) = (angle as f32).to_radians().sin_cos();
        Self([
          [
            0.213 + cos * 0.787 - sin * 0.213,
            0.715 - cos * 0.715 - sin * 0.715,
            0.072 - cos * 0.072 + sin * 0.928,
            0.,
          ],
          [
            0.213 - cos * 0.213 + sin * 0.143,
            0.715 + cos * 0.285 + sin * 0.140,
            0.072 - cos * 0.072 - sin * 0.283,
            0.,
          ],
          [
            0.213 - cos * 0.213 - sin * 0.787,
            0.715 - cos * 0.715 + sin * 0.715,
            0.072 + cos * 0.928 + sin * 0.072,
            0.,
          ],
        ])
      }
      _ => return (None, 1.),
    };
    ((matrix != identity).then_some(matrix), 1.)
  }

  #[cfg(test)]
  pub fn color(&self, (ink, alpha): (u32, u8)) -> (u32, u8) {
    let rgb = [16, 8, 0].map(|shift| ((ink >> shift) & 255) as f32 / 255.);
    let [r, g, b] = self.0.map(|row| {
      ((row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2] + row[3]).clamp(0., 1.) * 255.).round()
        as u32
    });
    (r << 16 | g << 8 | b, alpha)
  }
}

// The shape and the box of a `shape-outside`.
pub(crate) fn shape_outside(value: &str) -> Option<shape::Shaped<'_>> {
  use shape::Shape::{Circle, Ellipse, Inset, Polygon};
  shape::Shaped::parse(value).filter(|shaped| {
    let known = |frame| shape::BOXES[..4].contains(&frame);
    let basic =
      |shape: &shape::Shape| matches!(shape, Inset(..) | Circle(..) | Ellipse(..) | Polygon(..));
    shaped.reference.is_none_or(known) && shaped.shape.as_ref().is_none_or(basic)
  })
}

// The text of a `shape-outside` with a shape, where the margin box is the default.
pub(crate) fn outside_text(value: &str, units: Option<&Lengths>) -> Option<String> {
  let shaped = shape_outside(value)?;
  let text = shaped.shape?.text(units);
  Some(match shaped.reference {
    None | Some("margin-box") => text,
    Some(frame) => format!("{text} {frame}"),
  })
}

#[cfg(test)]
#[test]
fn grid_area_rows_split_dots_from_names() {
  assert_eq!(
    area_cells(".replace actions."),
    [".", "replace", "actions", "."]
  );
  assert_eq!(area_cells("  a  ...b  c-1 "), ["a", ".", "b", "c-1"]);
  assert!(area_cells("   ").is_empty());
}
