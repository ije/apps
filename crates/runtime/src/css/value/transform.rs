use crate::css::{
  properties::Prop, properties::Style, value::shape::Position, value::tokens::components,
  value::units::Kind, value::units::Lengths,
};
use renderer::{Pixels, Rect};

pub(crate) type Matrix = [f32; 16];

pub(crate) const IDENTITY: Matrix = [
  1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

pub(crate) fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
  std::array::from_fn(|index| {
    let (column, row) = (index / 4, index % 4);
    (0..4).map(|k| a[k * 4 + row] * b[column * 4 + k]).sum()
  })
}

pub(crate) fn translate(x: f32, y: f32, z: f32) -> Matrix {
  let mut m = IDENTITY;
  m[12] = x;
  m[13] = y;
  m[14] = z;
  m
}

pub(crate) fn scale(x: f32, y: f32, z: f32) -> Matrix {
  let mut m = IDENTITY;
  m[0] = x;
  m[5] = y;
  m[10] = z;
  m
}

pub(crate) fn rotate(axis: [f32; 3], angle: f32) -> Option<Matrix> {
  let length = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
  if length < 1e-6 {
    return None;
  }
  let [x, y, z] = axis.map(|value| value / length);
  // Quarter turns are exact, like in browsers, so `rotate(90deg)` stays a 2D matrix.
  let quarter = angle / std::f32::consts::FRAC_PI_2;
  let (s, c) = if (quarter - quarter.round()).abs() < 1e-6 {
    [(0., 1.), (1., 0.), (0., -1.), (-1., 0.)][(quarter.round() as i64).rem_euclid(4) as usize]
  } else {
    angle.sin_cos()
  };
  if [x, y, z] == [0., 0., 1.] {
    let mut m = IDENTITY;
    [m[0], m[1], m[4], m[5]] = [c, s, -s, c];
    return Some(m);
  }
  let t = 1. - c;
  Some([
    t * x * x + c,
    t * x * y + s * z,
    t * x * z - s * y,
    0.,
    t * x * y - s * z,
    t * y * y + c,
    t * y * z + s * x,
    0.,
    t * x * z + s * y,
    t * y * z - s * x,
    t * z * z + c,
    0.,
    0.,
    0.,
    0.,
    1.,
  ])
}

pub(crate) fn perspective(distance: f32) -> Matrix {
  let mut m = IDENTITY;
  m[11] = -1. / distance.max(1.);
  m
}

pub(crate) fn origin(value: &str, units: &Lengths, size: [f32; 2]) -> [f32; 3] {
  let parts = components(value, ' ');
  let (x, y) = match parts.as_slice() {
    [only] if matches!(*only, "top" | "bottom") => ("center", *only),
    [only] => (*only, "center"),
    [a, b, ..] if matches!(*a, "top" | "bottom") || matches!(*b, "left" | "right") => (*b, *a),
    [a, b, ..] => (*a, *b),
    [] => ("center", "center"),
  };
  let [x, y] = [(x, size[0]), (y, size[1])].map(|(token, extent)| match token {
    "left" | "top" => 0.,
    "center" => extent / 2.,
    "right" | "bottom" => extent,
    value => units.resolve(value, extent).unwrap_or(extent / 2.),
  });
  let z = parts
    .get(2)
    .and_then(|value| units.resolve(value, 0.))
    .unwrap_or(0.);
  [x, y, z]
}

pub(crate) fn transform(
  style: &Style,
  units: &Lengths,
  bounds: Rect<Pixels>,
  parent_perspective: Option<(f32, [f32; 2])>,
) -> Option<Matrix> {
  let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
  let number = |value: &str| {
    if let Some(percent) = value.strip_suffix('%') {
      return percent.parse::<f32>().ok().map(|value| value / 100.);
    }
    match units.math(value, 100., &[])? {
      (value, Kind::Number) => Some(value as f32),
      (percent, Kind::Length) if units.math(value, 0., &[]) == Some((0., Kind::Length)) => {
        Some(percent as f32 / 100.)
      }
      _ => None,
    }
  };
  let angle = |value: &str| {
    units
      .math(value, 0., &[])
      .filter(|(value, kind)| *kind == Kind::Angle || *value == 0.)
      .map(|(value, _)| (value as f32).to_radians())
  };
  let length = |value: &str, base: f32| units.resolve(value, base);
  let mut matrix = IDENTITY;
  let mut changed = false;
  let mut then = |next: Matrix| {
    matrix = multiply(&matrix, &next);
    changed = true;
  };
  if let Some(value) = style.get(Prop::TRANSLATE).filter(|value| *value != "none") {
    let parts = components(value, ' ');
    then(translate(
      length(parts.first()?, size[0])?,
      parts.get(1).map_or(Some(0.), |y| length(y, size[1]))?,
      parts.get(2).map_or(Some(0.), |z| length(z, 0.))?,
    ));
  }
  if let Some(value) = style.get(Prop::ROTATE).filter(|value| *value != "none") {
    let parts = components(value, ' ');
    let (last, rest) = parts.split_last()?;
    let axis = match rest {
      [] | ["z"] => [0., 0., 1.],
      ["x"] => [1., 0., 0.],
      ["y"] => [0., 1., 0.],
      [x, y, z] => [number(x)?, number(y)?, number(z)?],
      _ => return None,
    };
    then(rotate(axis, angle(last)?)?);
  }
  if let Some(value) = style.get(Prop::SCALE).filter(|value| *value != "none") {
    let parts = components(value, ' ');
    let x = number(parts.first()?)?;
    then(scale(
      x,
      parts.get(1).map_or(Some(x), |y| number(y))?,
      parts.get(2).map_or(Some(1.), |z| number(z))?,
    ));
  }
  if let Some(value) = style
    .get(Prop::OFFSET_PATH)
    .filter(|value| *value != "none")
    && let Some(track) = super::shape::path_track(value)
  {
    let distance = length(
      style.get(Prop::OFFSET_DISTANCE).unwrap_or("0px"),
      track.length(),
    )?;
    let ([x, y], tangent) = track.point(distance);
    let (auto, turn) = super::shape::turn(style.get(Prop::OFFSET_ROTATE).unwrap_or("auto"), units)?;
    let turn = turn as f32 + if auto { tangent } else { 0. };
    let [ox, oy, _] = origin(
      style.get(Prop::TRANSFORM_ORIGIN).unwrap_or("50% 50%"),
      units,
      size,
    );
    let anchor = style
      .get(Prop::OFFSET_ANCHOR)
      .and_then(|value| Position::parse(&components(value, ' '))?.point(units, size));
    let [ax, ay] = anchor.map_or([0.; 2], |[x, y]| [x - ox, y - oy]);
    then(translate(x - ox, y - oy, 0.));
    then(rotate([0., 0., 1.], turn.to_radians())?);
    then(translate(-ax, -ay, 0.));
  }
  for part in components(style.get(Prop::TRANSFORM).unwrap_or(""), ' ') {
    let Some((name, args)) = part.strip_suffix(')').and_then(|part| part.split_once('(')) else {
      continue;
    };
    let args = components(args, ',');
    let next = match (name.to_ascii_lowercase().as_str(), args.as_slice()) {
      ("translate" | "translatex", [x]) => translate(length(x, size[0])?, 0., 0.),
      ("translate", [x, y]) => translate(length(x, size[0])?, length(y, size[1])?, 0.),
      ("translatey", [y]) => translate(0., length(y, size[1])?, 0.),
      ("translatez", [z]) => translate(0., 0., length(z, 0.)?),
      ("translate3d", [x, y, z]) => {
        translate(length(x, size[0])?, length(y, size[1])?, length(z, 0.)?)
      }
      ("scale", [x]) => scale(number(x)?, number(x)?, 1.),
      ("scale", [x, y]) => scale(number(x)?, number(y)?, 1.),
      ("scalex", [x]) => scale(number(x)?, 1., 1.),
      ("scaley", [y]) => scale(1., number(y)?, 1.),
      ("scalez", [z]) => scale(1., 1., number(z)?),
      ("scale3d", [x, y, z]) => scale(number(x)?, number(y)?, number(z)?),
      ("rotate" | "rotatez", [a]) => rotate([0., 0., 1.], angle(a)?)?,
      ("rotatex", [a]) => rotate([1., 0., 0.], angle(a)?)?,
      ("rotatey", [a]) => rotate([0., 1., 0.], angle(a)?)?,
      ("rotate3d", [x, y, z, a]) => rotate([number(x)?, number(y)?, number(z)?], angle(a)?)?,
      ("skew" | "skewx", [x]) => {
        let mut m = IDENTITY;
        m[4] = angle(x)?.tan();
        m
      }
      ("skew", [x, y]) => {
        let mut m = IDENTITY;
        m[4] = angle(x)?.tan();
        m[1] = angle(y)?.tan();
        m
      }
      ("skewy", [y]) => {
        let mut m = IDENTITY;
        m[1] = angle(y)?.tan();
        m
      }
      ("matrix", [a, b, c, d, e, f]) => {
        let mut m = IDENTITY;
        [m[0], m[1], m[4], m[5], m[12], m[13]] = [
          number(a)?,
          number(b)?,
          number(c)?,
          number(d)?,
          number(e)?,
          number(f)?,
        ];
        m
      }
      ("matrix3d", values) if values.len() == 16 => {
        let mut m = IDENTITY;
        for (slot, value) in m.iter_mut().zip(values) {
          *slot = number(value)?;
        }
        m
      }
      ("perspective", [d]) => match *d {
        "none" => IDENTITY,
        d => perspective(length(d, 0.)?),
      },
      _ => continue,
    };
    then(next);
  }
  if !changed && parent_perspective.is_none() {
    return None;
  }
  let [ox, oy, oz] = origin(
    style.get(Prop::TRANSFORM_ORIGIN).unwrap_or("50% 50%"),
    units,
    size,
  );
  let (x, y) = (
    f32::from(bounds.origin.x) + ox,
    f32::from(bounds.origin.y) + oy,
  );
  let mut result = multiply(
    &translate(x, y, oz),
    &multiply(&matrix, &translate(-x, -y, -oz)),
  );
  if let Some((distance, [px, py])) = parent_perspective {
    result = multiply(
      &translate(px, py, 0.),
      &multiply(
        &perspective(distance),
        &multiply(&translate(-px, -py, 0.), &result),
      ),
    );
  }
  result
    .iter()
    .all(|value| value.is_finite())
    .then_some(result)
    .filter(|result| *result != IDENTITY)
}

pub(crate) fn angle(value: &str, units: &Lengths) -> Option<f32> {
  units
    .math(value, 0., &[])
    .filter(|(value, kind)| *kind == Kind::Angle || *kind == Kind::Number && *value == 0.)
    .map(|(value, _)| value as f32)
}
