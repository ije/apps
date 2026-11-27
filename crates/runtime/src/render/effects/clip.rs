use super::{Ids, index};
use crate::css;
use crate::css::properties::Prop;
use crate::{
  css::{
    properties::MARGIN, properties::PADDING, properties::Style, value::shape, value::shape::Shape,
    value::shape::Shaped, value::units::Lengths,
  },
  dom::Document,
};
use renderer::{Corners, Length, Pixels, Radii, px};

pub(crate) enum Clip {
  Rounded([f32; 4], [f32; 4]),
  // An SVG shape, and its left, top, right and bottom when they are known.
  Shape(String, Option<[f32; 4]>),
}

// `size` is the size of the border box of a style.
pub(crate) fn clip(
  value: &str,
  style: &Style,
  units: &Lengths,
  size: [f32; 2],
  doc: &Document,
  ids: &Ids,
) -> Option<Clip> {
  if let Some(Shaped { shape, reference }) = Shaped::parse(value) {
    let [top, right, bottom, left] = insets(reference, style, units, size);
    let inner = [
      (size[0] - left - right).max(0.),
      (size[1] - top - bottom).max(0.),
    ];
    let even = shape.as_ref().is_some_and(Shape::even);
    let outline = match &shape {
      Some(shape) => match shape.rounded(units, inner, None) {
        Some(([x, y, w, h], radii)) if radii.iter().all(|r| (r[0] - r[1]).abs() < 1e-3) => {
          return Some(Clip::Rounded(
            [x + left, y + top, w.max(0.), h.max(0.)],
            radii.map(|r| r[0]),
          ));
        }
        _ => shape.outline(units, inner, None)?,
      },
      None => {
        let radii = edge_radii(reference, style, units, size);
        if radii.iter().all(|r| r[0] == r[1]) {
          return Some(Clip::Rounded(
            [left, top, inner[0], inner[1]],
            radii.map(|r| r[0]),
          ));
        }
        shape::rounded([0., 0., inner[0], inner[1]], radii)
      }
    };
    let rule = if even { "evenodd" } else { "nonzero" };
    let reach = shape::reach(&outline)?;
    return Some(Clip::Shape(
      format!(
        "<path transform=\"translate({left} {top})\" fill-rule=\"{rule}\" d=\"{}\"/>",
        shape::svg(&outline)
      ),
      Some([
        reach[0] + left,
        reach[1] + top,
        reach[2] + left,
        reach[3] + top,
      ]),
    ));
  }
  let [width, height] = size;
  let (name, args) = value.strip_suffix(')')?.split_once('(')?;
  match name.to_ascii_lowercase().as_str() {
    "url" => {
      let id = args.trim().trim_matches(['"', '\'']).strip_prefix('#')?;
      let clip = index(ids, doc)
        .get(id)?
        .iter()
        .copied()
        .find(|&node| doc.nodes[node].tag.eq_ignore_ascii_case("clipPath"))?;
      let object = doc.nodes[clip]
        .attrs
        .get("clipPathUnits")
        .is_some_and(|value| value == "objectBoundingBox");
      let shapes: String = doc.nodes[clip]
        .children
        .iter()
        .map(|&child| {
          doc.serialize_svg(child, false, &|node| {
            doc.nodes[node]
              .attrs
              .get("style")
              .map(|style| vec![("style", style.clone())])
              .unwrap_or_default()
          })
        })
        .collect();
      Some(Clip::Shape(
        if object {
          format!("<g transform=\"scale({width} {height})\">{shapes}</g>")
        } else {
          shapes
        },
        None,
      ))
    }
    _ => None,
  }
}

// The distances from the border box to another box of a style.
pub(crate) fn insets(
  reference: Option<&str>,
  style: &Style,
  units: &Lengths,
  size: [f32; 2],
) -> [f32; 4] {
  let edges = |props| units.edges(style, props, size[0]);
  match reference {
    Some("padding-box") => units.borders(style, size[0]),
    Some("content-box" | "fill-box") => {
      let (borders, padding) = (units.borders(style, size[0]), edges(PADDING));
      std::array::from_fn(|side| borders[side] + padding[side].max(0.))
    }
    Some("margin-box") => edges(MARGIN).map(|margin| -margin),
    _ => [0.; 4],
  }
}

// How far an `overflow-clip-margin` reaches past the padding box.
pub(crate) fn margin(
  value: &str,
  style: &Style,
  units: &Lengths,
  base: f32,
) -> Option<renderer::ClipMargin> {
  let (mut reach, mut sides) = (0., [0.; 4]);
  for part in value.split(' ') {
    match part {
      "padding-box" => {}
      "border-box" => sides = units.borders(style, base),
      "content-box" => {
        sides = units.edges(style, PADDING, base).map(|side| -side.max(0.));
      }
      length => reach = units.resolve(length, 0.)?,
    }
  }
  let [top, right, bottom, left] = sides.map(px);
  Some(renderer::ClipMargin {
    sides: renderer::Edges {
      top,
      right,
      bottom,
      left,
    },
    reach: px(reach),
  })
}

// The radii of a box of a style, as its `border-radius` gives them.
pub(crate) fn edge_radii(
  reference: Option<&str>,
  style: &Style,
  units: &Lengths,
  size: [f32; 2],
) -> [[f32; 2]; 4] {
  let [top, right, bottom, left] = insets(reference, style, units, size);
  let radii = radii(style, units, || size).resolve(renderer::size(px(size[0]), px(size[1])));
  let corner = |x: Pixels, y: Pixels, h: f32, v: f32| {
    let (x, y) = (f32::from(x), f32::from(y));
    match x > 0. && y > 0. {
      true => [(x - h).max(0.), (y - v).max(0.)],
      false => [0.; 2],
    }
  };
  [
    corner(radii.x.top_left, radii.y.top_left, left, top),
    corner(radii.x.top_right, radii.y.top_right, right, top),
    corner(radii.x.bottom_right, radii.y.bottom_right, right, bottom),
    corner(radii.x.bottom_left, radii.y.bottom_left, left, bottom),
  ]
}

pub(crate) const RADII: [Prop; 4] = [
  Prop::BORDER_TOP_LEFT_RADIUS,
  Prop::BORDER_TOP_RIGHT_RADIUS,
  Prop::BORDER_BOTTOM_RIGHT_RADIUS,
  Prop::BORDER_BOTTOM_LEFT_RADIUS,
];

pub(crate) fn radii(style: &Style, units: &Lengths, basis: impl Fn() -> [f32; 2]) -> Radii<Length> {
  let mut radii = Radii::default();
  for (prop, shape, x, y, code) in [
    (
      Prop::BORDER_TOP_LEFT_RADIUS,
      Prop::CORNER_TOP_LEFT_SHAPE,
      &mut radii.x.top_left,
      &mut radii.y.top_left,
      &mut radii.shape.top_left,
    ),
    (
      Prop::BORDER_TOP_RIGHT_RADIUS,
      Prop::CORNER_TOP_RIGHT_SHAPE,
      &mut radii.x.top_right,
      &mut radii.y.top_right,
      &mut radii.shape.top_right,
    ),
    (
      Prop::BORDER_BOTTOM_RIGHT_RADIUS,
      Prop::CORNER_BOTTOM_RIGHT_SHAPE,
      &mut radii.x.bottom_right,
      &mut radii.y.bottom_right,
      &mut radii.shape.bottom_right,
    ),
    (
      Prop::BORDER_BOTTOM_LEFT_RADIUS,
      Prop::CORNER_BOTTOM_LEFT_SHAPE,
      &mut radii.x.bottom_left,
      &mut radii.y.bottom_left,
      &mut radii.shape.bottom_left,
    ),
  ] {
    let Some(value) = style.get(prop) else {
      continue;
    };
    let mut depth = 0;
    let (first, second) = match value.find(|c: char| {
      depth += i32::from(c == '(') - i32::from(c == ')');
      c == ' ' && depth == 0
    }) {
      Some(at) => (&value[..at], Some(value[at..].trim())),
      None => (value, None),
    };
    let length = |part: &str, axis: usize| match part
      .strip_suffix('%')
      .and_then(|percent| percent.parse::<f32>().ok())
      .filter(|percent| percent.is_finite())
    {
      Some(percent) => Length::Fraction(percent.max(0.) / 100.),
      None => units
        .resolve(
          part,
          if part.contains('%') {
            basis()[axis]
          } else {
            0.
          },
        )
        .map_or(Length::Auto, |length| Length::Px(px(length.max(0.)))),
    };
    *x = length(first, 0);
    *y = match second {
      Some(second) => length(second, 1),
      None if first.contains('%') => length(first, 1),
      None => *x,
    };
    match style.get(shape).and_then(css::value::corner_shape) {
      // Like Chrome, a corner of 16 or more is square, with no radius.
      Some(k) if k >= 16. => (*x, *y) = (Length::Px(px(0.)), Length::Px(px(0.))),
      Some(k) => *code = renderer::corner_shape(k as f32),
      None => {}
    }
  }
  radii
}

pub(crate) fn corners(radii: [f32; 4]) -> Corners<Pixels> {
  Corners {
    top_left: px(radii[0]),
    top_right: px(radii[1]),
    bottom_right: px(radii[2]),
    bottom_left: px(radii[3]),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn clip_paths_become_shapes() {
    let units = Lengths::new(16., 800., 600.);
    let doc = Document::parse(
      "<svg><clipPath id=c><rect width=5 height=5></rect></clipPath></svg><p id=c></p>",
    );
    let box_ = super::super::tests::style(&[
      ("padding-left", "10px"),
      ("border-top-width", "5px"),
      ("border-top-style", "solid"),
      ("margin-right", "20px"),
      ("border-top-left-radius", "30px"),
    ]);
    let clip = |value: &str| clip(value, &box_, &units, [100., 60.], &doc, &Ids::default());
    let rounded = |value: &str| match clip(value) {
      Some(Clip::Rounded(rect, radii)) => (rect, radii),
      _ => panic!("{value}"),
    };
    let shape = |value: &str| match clip(value) {
      Some(Clip::Shape(svg, reach)) => (svg, reach),
      _ => panic!("{value}"),
    };
    assert_eq!(
      rounded("inset(10px round 4px)"),
      ([10., 10., 80., 40.], [4.; 4])
    );
    // Radii that don't fit scale down together.
    assert_eq!(rounded("inset(0 round 80px)").1, [30.; 4]);
    assert_eq!(rounded("rect(auto 80px 50% 10px)").0, [10., 0., 70., 30.]);
    assert_eq!(
      rounded("xywh(5px 6px 50% 10px round 3px) border-box"),
      ([5., 6., 50., 10.], [3.; 4])
    );
    assert_eq!(rounded("content-box inset(0)").0, [10., 5., 90., 55.]);
    assert_eq!(rounded("margin-box").0, [0., 0., 120., 60.]);
    assert_eq!(rounded("border-box").1, [30., 0., 0., 0.]);
    assert_eq!(
      rounded("circle(10px at left top)"),
      ([-10., -10., 20., 20.], [10.; 4])
    );
    let (rect, radii) = rounded("circle(40%)");
    assert!((rect[2] - rect[3]).abs() < 1e-3 && (radii[0] * 2. - rect[2]).abs() < 1e-3);
    shape("inset(0 round 50%)");
    shape("ellipse(40% 20%)");
    assert_eq!(
      shape("polygon(50% 0, 100% 100%, -10px 100%)"),
      (
        "<path transform=\"translate(0 0)\" fill-rule=\"nonzero\" d=\"M50 0L100 60L-10 60Z\"/>"
          .into(),
        Some([-10., 0., 100., 60.])
      )
    );
    assert!(
      shape("path(evenodd, 'M0 0 L10 0 5 5z')")
        .0
        .contains("\"evenodd\" d=\"M0 0L10 0L5 5Z\"")
    );
    assert!(
      shape(
        "shape(from 0 0, hline to 100%, line by -50% 10px, curve by 5px 5px with 1px 1px, close)"
      )
      .0
      .contains("d=\"M0 0L100 0L50 10Q51 11 55 15Z\"")
    );
    assert!(shape("url(#c)").0.contains("<rect"));
  }
}
