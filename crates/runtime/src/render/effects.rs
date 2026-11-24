use crate::css;
use crate::css::Prop;
use crate::{
    css::{Lengths, Style, components},
    dom::Document,
};
use renderer::{BlendMode, Color, Corners, Length, PaintEffect, Pixels, Radii, Rect, point, px};

pub(crate) type Matrix = [f32; 16];
// A transform at the origin and the size it is for.
pub(crate) type Local = std::cell::Cell<Option<Box<([f32; 2], Option<Matrix>)>>>;

// Find IDs through the tree walk. The arena includes removed nodes and can greatly exceed the live page.
#[derive(Default)]
pub(crate) struct Ids {
    map: std::cell::OnceCell<std::collections::HashMap<String, Vec<usize>>>,
    walk: Option<std::rc::Rc<Vec<(u32, u32)>>>,
}

impl Ids {
    pub(crate) fn of(walk: std::rc::Rc<Vec<(u32, u32)>>) -> Self {
        Self {
            map: Default::default(),
            walk: Some(walk),
        }
    }
}

pub(crate) fn index<'a>(
    ids: &'a Ids,
    doc: &Document,
) -> &'a std::collections::HashMap<String, Vec<usize>> {
    ids.map.get_or_init(|| {
        let mut map = std::collections::HashMap::<String, Vec<usize>>::new();
        let mut add = |node: usize| {
            if let Some(id) = doc.nodes.get(node).and_then(|value| value.attrs.get("id")) {
                map.entry(id.clone()).or_default().push(node);
            }
        };
        match &ids.walk {
            Some(walk) => {
                let mut nodes: Vec<usize> = walk.iter().map(|&(node, _)| node as usize).collect();
                nodes.sort_unstable();
                nodes.into_iter().for_each(&mut add);
            }
            None => (0..doc.nodes.len()).for_each(&mut add),
        }
        map
    })
}

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

fn scale(x: f32, y: f32, z: f32) -> Matrix {
    let mut m = IDENTITY;
    m[0] = x;
    m[5] = y;
    m[10] = z;
    m
}

fn rotate(axis: [f32; 3], angle: f32) -> Option<Matrix> {
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

fn perspective(distance: f32) -> Matrix {
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
            (value, css::Kind::Number) => Some(value as f32),
            (percent, css::Kind::Length)
                if units.math(value, 0., &[]) == Some((0., css::Kind::Length)) =>
            {
                Some(percent as f32 / 100.)
            }
            _ => None,
        }
    };
    let angle = |value: &str| {
        units
            .math(value, 0., &[])
            .filter(|(value, kind)| *kind == css::Kind::Angle || *value == 0.)
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
    for part in components(style.get(Prop::TRANSFORM).unwrap_or(""), ' ') {
        let Some((name, args)) = part.strip_suffix(')').and_then(|part| part.split_once('('))
        else {
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

// The matrix of a box moved by `[x, y]`: `translate(x, y) * matrix * translate(-x, -y)`, without the rounding of
// a move there and back, so a plain translation stays exact.
pub(crate) fn placed(m: &Matrix, [x, y]: [f32; 2]) -> Matrix {
    let mut n = *m;
    for column in 0..3 {
        n[column * 4] += x * m[column * 4 + 3];
        n[column * 4 + 1] += y * m[column * 4 + 3];
    }
    let w = x * m[3] + y * m[7];
    n[12] = m[12] + x * (m[15] - m[0]) - y * m[4] - x * w;
    n[13] = m[13] + y * (m[15] - m[5]) - x * m[1] - y * w;
    n[14] = m[14] - x * m[2] - y * m[6];
    n[15] = m[15] - w;
    n
}

// The transform of a box at its place, from a cache of the transform at the origin for its size.
pub(crate) fn transform_at(
    cache: &Local,
    style: &Style,
    units: &Lengths,
    bounds: Rect<Pixels>,
) -> Option<Matrix> {
    let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
    let mut kept = cache.take().filter(|kept| kept.0 == size);
    let local = kept
        .get_or_insert_with(|| {
            let origin = Rect::new(renderer::Point::default(), bounds.size);
            Box::new((size, transform(style, units, origin, None)))
        })
        .1;
    cache.set(kept);
    local.map(|local| placed(&local, [bounds.origin.x, bounds.origin.y].map(f32::from)))
}

// The near and far corners of the rectangle between `corners` after the transform of a box of `size`.
pub(super) fn reach(
    cache: &Local,
    style: &Style,
    units: &Lengths,
    size: renderer::Size<Pixels>,
    corners: [renderer::Point<Pixels>; 2],
) -> [renderer::Point<Pixels>; 2] {
    let Some(matrix) = transform_at(cache, style, units, Rect::new(Default::default(), size))
    else {
        return corners;
    };
    let [[left, top], [right, bottom]] = corners.map(|at| [at.x, at.y].map(f32::from));
    let mut mapped = [[left, top], [right, top], [left, bottom], [right, bottom]]
        .into_iter()
        .filter_map(|corner| map(&matrix, corner))
        .map(|[x, y]| point(px(x), px(y)));
    let Some(first) = mapped.next() else {
        return corners;
    };
    mapped.fold([first; 2], |[near, far], at| [near.min(at), far.max(at)])
}

pub(crate) fn translation(matrix: &Matrix) -> Option<[f32; 2]> {
    let plain = translate(matrix[12], matrix[13], 0.);
    (*matrix == plain).then_some([matrix[12], matrix[13]])
}

fn map(matrix: &Matrix, [x, y]: [f32; 2]) -> Option<[f32; 2]> {
    let w = matrix[3] * x + matrix[7] * y + matrix[15];
    (w > 1e-4).then(|| {
        [
            (matrix[0] * x + matrix[4] * y + matrix[12]) / w,
            (matrix[1] * x + matrix[5] * y + matrix[13]) / w,
        ]
    })
}

pub(crate) fn backface(matrix: &Matrix, bounds: Rect<Pixels>) -> bool {
    let center = bounds.center();
    let [cx, cy] = [f32::from(center.x), f32::from(center.y)];
    let (Some(a), Some(b), Some(c)) = (
        map(matrix, [cx, cy]),
        map(matrix, [cx + 1., cy]),
        map(matrix, [cx, cy + 1.]),
    ) else {
        return true;
    };
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) < 0.
}

fn color_matrix(filter: Option<css::Filter>, opacity: f32) -> Option<[f32; 20]> {
    if filter.is_none() && opacity >= 1. {
        return None;
    }
    let rows = filter.map_or(
        [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
        |filter| filter.0,
    );
    let mut matrix = [0.; 20];
    for (row, values) in rows.iter().enumerate() {
        matrix[row * 5..row * 5 + 3].copy_from_slice(&values[..3]);
        matrix[row * 5 + 4] = values[3];
    }
    matrix[18] = opacity;
    Some(matrix)
}

fn then_matrix(inner: &[f32; 20], outer: &[f32; 20]) -> [f32; 20] {
    let mut result = [0.; 20];
    for row in 0..4 {
        for column in 0..5 {
            result[row * 5 + column] = (0..4)
                .map(|k| outer[row * 5 + k] * inner[k * 5 + column])
                .sum::<f32>()
                + if column == 4 { outer[row * 5 + 4] } else { 0. };
        }
    }
    result
}

pub(crate) fn filters(value: &str, units: &Lengths, doc: &Document, ids: &Ids) -> Vec<PaintEffect> {
    let mut effects: Vec<PaintEffect> = Vec::new();
    for part in components(value, ' ') {
        let Some((name, args)) = part.strip_suffix(')').and_then(|part| part.split_once('('))
        else {
            continue;
        };
        match name.to_ascii_lowercase().as_str() {
            "blur" => {
                let sigma = if args.trim().is_empty() {
                    0.
                } else {
                    units.resolve(args.trim(), 0.).unwrap_or(0.)
                };
                if sigma > 0. {
                    effects.push(PaintEffect {
                        blur: px(sigma),
                        ..Default::default()
                    });
                }
            }
            "drop-shadow" => {
                // Chrome uses the blur length as the standard deviation, not half of it as box-shadow does.
                for shadow in css::box_shadows(units, args, false) {
                    effects.push(PaintEffect {
                        shadow: Some((shadow.offset, shadow.blur_radius * 2., shadow.color)),
                        ..Default::default()
                    });
                }
            }
            "url" => effects.extend(svg_filter(args, units, doc, ids)),
            _ => {
                let (filter, opacity) = css::Filter::parse(part, units);
                let Some(matrix) = color_matrix(filter, opacity) else {
                    continue;
                };
                if let Some(last) = effects.last_mut()
                    && last.blur == px(0.)
                    && last.shadow.is_none()
                {
                    last.color_matrix = Some(match last.color_matrix {
                        Some(inner) => then_matrix(&inner, &matrix),
                        None => matrix,
                    });
                } else {
                    effects.push(PaintEffect {
                        color_matrix: Some(matrix),
                        ..Default::default()
                    });
                }
            }
        }
    }
    effects
}

fn svg_filter(reference: &str, units: &Lengths, doc: &Document, ids: &Ids) -> Vec<PaintEffect> {
    let id = reference.trim().trim_matches(['"', '\'']);
    let Some(id) = id.strip_prefix('#') else {
        return Vec::new();
    };
    let Some(filter) = index(ids, doc).get(id).and_then(|nodes| {
        nodes
            .iter()
            .copied()
            .find(|&node| doc.nodes[node].tag == "filter")
    }) else {
        return Vec::new();
    };
    let mut effects = Vec::new();
    let mut offset = point(px(0.), px(0.));
    for &child in &doc.nodes[filter].children {
        let node = &doc.nodes[child];
        let numbers = |name: &str| -> Vec<f32> {
            node.attrs
                .get(name)
                .map(|value| {
                    value
                        .split(|c: char| c.is_whitespace() || c == ',')
                        .filter_map(|part| part.parse().ok())
                        .collect()
                })
                .unwrap_or_default()
        };
        match node.tag.to_ascii_lowercase().as_str() {
            "fegaussianblur" => {
                let sigma = numbers("stdDeviation").first().copied().unwrap_or(0.);
                if sigma > 0. {
                    effects.push(PaintEffect {
                        blur: px(sigma),
                        ..Default::default()
                    });
                }
            }
            "feoffset" => {
                offset = point(
                    px(numbers("dx").first().copied().unwrap_or(0.)),
                    px(numbers("dy").first().copied().unwrap_or(0.)),
                );
            }
            "fedropshadow" => {
                let color = node
                    .attrs
                    .get("flood-color")
                    .and_then(|value| css::color(value))
                    .unwrap_or((0, 255));
                let opacity = numbers("flood-opacity").first().copied().unwrap_or(1.);
                let mut ink: Color = renderer::Color::hex(color.0 << 8 | color.1 as u32);
                ink.a *= opacity;
                effects.push(PaintEffect {
                    shadow: Some((
                        point(
                            px(numbers("dx").first().copied().unwrap_or(2.)),
                            px(numbers("dy").first().copied().unwrap_or(2.)),
                        ),
                        px(numbers("stdDeviation").first().copied().unwrap_or(2.)),
                        ink,
                    )),
                    ..Default::default()
                });
            }
            "fecolormatrix" => {
                let kind = node.attrs.get("type").map_or("matrix", String::as_str);
                let values = numbers("values");
                let css = match kind {
                    "saturate" => format!("saturate({})", values.first().copied().unwrap_or(1.)),
                    "hueRotate" => {
                        format!("hue-rotate({}deg)", values.first().copied().unwrap_or(0.))
                    }
                    _ => String::new(),
                };
                let matrix = if kind == "matrix" && values.len() == 20 {
                    values.try_into().ok()
                } else if kind == "luminanceToAlpha" {
                    let mut matrix = [0.; 20];
                    matrix[15..18].copy_from_slice(&[0.2125, 0.7154, 0.0721]);
                    Some(matrix)
                } else {
                    let (filter, opacity) = css::Filter::parse(&css, units);
                    color_matrix(filter, opacity)
                };
                if let Some(matrix) = matrix {
                    effects.push(PaintEffect {
                        color_matrix: Some(matrix),
                        ..Default::default()
                    });
                }
            }
            _ => {}
        }
    }
    if offset != point(px(0.), px(0.))
        && let Some(blur) = effects.iter().position(|effect| effect.blur > px(0.))
    {
        let sigma = effects.remove(blur).blur;
        effects.push(PaintEffect {
            shadow: Some((offset, sigma, renderer::Color::BLACK.fade(0.5))),
            ..Default::default()
        });
    }
    effects
}

/// The argument of a filter list that is one `blur()`.
pub(crate) fn lone_blur(value: Option<&str>) -> Option<&str> {
    let [part] = *components(value?, ' ') else {
        return None;
    };
    let (name, args) = part.strip_suffix(')')?.split_once('(')?;
    name.eq_ignore_ascii_case("blur").then_some(args.trim())
}

pub(crate) fn backdrop(value: &str, units: &Lengths) -> Option<(Pixels, Option<[f32; 20]>)> {
    let mut blur = px(0.);
    let mut matrix: Option<[f32; 20]> = None;
    for effect in filters(value, units, &Document::default(), &Ids::default()) {
        blur = blur.max(effect.blur);
        if let Some(next) = effect.color_matrix {
            matrix = Some(match matrix {
                Some(inner) => then_matrix(&inner, &next),
                None => next,
            });
        }
    }
    (blur > px(0.) || matrix.is_some()).then_some((blur, matrix))
}

pub(crate) fn blend_mode(value: &str) -> BlendMode {
    match value.trim() {
        "multiply" => BlendMode::Multiply,
        "screen" => BlendMode::Screen,
        "overlay" => BlendMode::Overlay,
        "darken" => BlendMode::Darken,
        "lighten" => BlendMode::Lighten,
        "color-dodge" => BlendMode::ColorDodge,
        "color-burn" => BlendMode::ColorBurn,
        "hard-light" => BlendMode::HardLight,
        "soft-light" => BlendMode::SoftLight,
        "difference" => BlendMode::Difference,
        "exclusion" => BlendMode::Exclusion,
        "hue" => BlendMode::Hue,
        "saturation" => BlendMode::Saturation,
        "color" => BlendMode::Color,
        "luminosity" => BlendMode::Luminosity,
        "plus-lighter" => BlendMode::PlusLighter,
        _ => BlendMode::Normal,
    }
}

pub(crate) enum Clip {
    Rounded([f32; 4], [f32; 4]),
    Shape(String),
}

pub(crate) fn clip(
    value: &str,
    units: &Lengths,
    size: [f32; 2],
    doc: &Document,
    ids: &Ids,
) -> Option<Clip> {
    let [width, height] = size;
    let (name, args) = components(value, ' ')
        .into_iter()
        .find_map(|part| part.strip_suffix(')')?.split_once('('))?;
    let svg = Clip::Shape;
    match name.to_ascii_lowercase().as_str() {
        "inset" | "rect" | "xywh" => {
            let (rect, radii) = css::clip_path(value, units, size)?;
            let round = args.split_once(" round ").map(|(_, round)| round.trim());
            let Some(round) = round.filter(|round| round.contains(['%', '/'])) else {
                return Some(Clip::Rounded(rect, radii));
            };
            // Percent and `/` radii can be elliptical. As for border-radius, percentages take the
            // reference box, and radii that don't fit scale down together.
            let (horizontal, vertical) = round.split_once('/').unwrap_or((round, round));
            let corners = |list: &str, base: f32| -> Option<[f32; 4]> {
                let values = components(list.trim(), ' ');
                let values = match values[..] {
                    [a] => [a; 4],
                    [a, b] => [a, b, a, b],
                    [a, b, c] => [a, b, c, b],
                    [a, b, c, d] => [a, b, c, d],
                    _ => return None,
                };
                let mut out = [0.; 4];
                for (corner, value) in values.into_iter().enumerate() {
                    out[corner] = units.resolve(value, base)?.max(0.);
                }
                Some(out)
            };
            let [w, h] = [rect[2], rect[3]];
            let (mut rx, mut ry) = (corners(horizontal, width)?, corners(vertical, height)?);
            let fit = [
                w / (rx[0] + rx[1]),
                h / (ry[1] + ry[2]),
                w / (rx[2] + rx[3]),
                h / (ry[3] + ry[0]),
            ]
            .into_iter()
            .fold(1f32, f32::min);
            rx = rx.map(|value| value * fit);
            ry = ry.map(|value| value * fit);
            if rx == ry {
                return Some(Clip::Rounded(rect, rx));
            }
            let [x, y] = [rect[0], rect[1]];
            Some(svg(format!(
                "<path d=\"M{} {y}H{}A{} {} 0 0 1 {} {}V{}A{} {} 0 0 1 {} {}H{}A{} {} 0 0 1 {x} {}V{}A{} {} 0 0 1 {} {y}Z\"/>",
                x + rx[0],
                x + w - rx[1],
                rx[1],
                ry[1],
                x + w,
                y + ry[1],
                y + h - ry[2],
                rx[2],
                ry[2],
                x + w - rx[2],
                y + h,
                x + rx[3],
                rx[3],
                ry[3],
                y + h - ry[3],
                y + ry[0],
                rx[0],
                ry[0],
                x + rx[0],
            )))
        }
        "circle" | "ellipse" => {
            let (rect, radii) = css::clip_path(value, units, size)?;
            if (rect[2] - rect[3]).abs() <= rect[2].max(rect[3]) * 1e-4 {
                return Some(Clip::Rounded(rect, radii));
            }
            Some(svg(format!(
                "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"/>",
                rect[0] + rect[2] / 2.,
                rect[1] + rect[3] / 2.,
                rect[2] / 2.,
                rect[3] / 2.
            )))
        }
        "polygon" => {
            let mut parts = components(args, ',');
            let rule = match parts.first().map(|part| part.trim()) {
                Some(rule @ ("evenodd" | "nonzero")) => {
                    parts.remove(0);
                    rule
                }
                _ => "nonzero",
            };
            let mut points = String::new();
            for part in parts {
                let pair = components(part.trim(), ' ');
                let [x, y] = <[&str; 2]>::try_from(pair).ok()?;
                points.push_str(&format!(
                    "{},{} ",
                    units.resolve(x, width)?,
                    units.resolve(y, height)?
                ));
            }
            Some(svg(format!(
                "<polygon fill-rule=\"{rule}\" points=\"{points}\"/>"
            )))
        }
        "path" => {
            let mut parts = components(args, ',');
            let rule = if parts.len() > 1 {
                parts.remove(0).trim()
            } else {
                "nonzero"
            };
            let data = parts.first()?.trim().trim_matches(['"', '\'']);
            Some(svg(format!(
                "<path fill-rule=\"{rule}\" d=\"{}\"/>",
                crate::dom::escape(data)
            )))
        }
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
            Some(svg(if object {
                format!("<g transform=\"scale({width} {height})\">{shapes}</g>")
            } else {
                shapes
            }))
        }
        _ => None,
    }
}

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
        match style.get(shape).and_then(css::corner_shape) {
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

fn angle(value: &str, units: &Lengths) -> Option<f32> {
    units
        .math(value, 0., &[])
        .filter(|(value, kind)| {
            *kind == css::Kind::Angle || *kind == css::Kind::Number && *value == 0.
        })
        .map(|(value, _)| value as f32)
}

fn first_argument(body: &str) -> (String, Option<String>) {
    let parts = css::components(body, ',');
    let words: Vec<&str> = parts
        .first()
        .map_or("", |part| part.trim())
        .split_whitespace()
        .collect();
    match words.iter().position(|word| *word == "in") {
        Some(index) => {
            let end = words[index + 1..]
                .iter()
                .position(|word| *word == "hue")
                .map_or(index + 2, |hue| index + hue + 2)
                .min(words.len());
            let rest = words[..index].iter().chain(&words[end..]);
            (
                rest.copied().collect::<Vec<_>>().join(" "),
                Some(words[index + 1..end].join(" ")),
            )
        }
        None => (words.join(" "), None),
    }
}

pub(crate) fn gradient_axis(kind: &str, body: &str, units: &Lengths) -> Option<usize> {
    if kind.trim_start_matches("repeating-") != "linear-gradient" {
        return None;
    }
    let (first, _) = first_argument(body);
    let degrees = match first.strip_prefix("to ") {
        Some(side) => match side.trim() {
            "top" => 0.,
            "right" => 90.,
            "bottom" => 180.,
            "left" => 270.,
            _ => return None,
        },
        None => angle(&first, units).unwrap_or(180.),
    };
    match degrees.rem_euclid(180.) {
        0. => Some(1),
        90. => Some(0),
        _ => None,
    }
}

// The size and offset of a background tile in its positioning area. `natural` is the size of an image.
pub(crate) fn tile(
    size: &str,
    position: &str,
    units: &Lengths,
    width: f32,
    height: f32,
    natural: Option<[f32; 2]>,
) -> [[f32; 2]; 2] {
    let dims = css::components(size, ' ');
    let [tw, th] = match (dims.as_slice(), natural) {
        ([fit @ ("cover" | "contain")], Some([nw, nh])) => {
            let [x, y] = [width / nw, height / nh];
            let scale = if *fit == "cover" { x.max(y) } else { x.min(y) };
            [nw * scale, nh * scale]
        }
        (["cover" | "contain"], None) => [width, height],
        _ => {
            let dim = |axis: usize| {
                dims.get(axis)
                    .filter(|value| **value != "auto")
                    .and_then(|value| units.resolve(value, [width, height][axis]))
            };
            match ([dim(0), dim(1)], natural) {
                ([Some(w), Some(h)], _) => [w, h],
                ([Some(w), None], Some([nw, nh])) => [w, w * nh / nw],
                ([None, Some(h)], Some([nw, nh])) => [h * nw / nh, h],
                ([None, None], Some(natural)) => natural,
                ([w, h], _) => [w.unwrap_or(width), h.unwrap_or(height)],
            }
        }
    }
    .map(|side| side.max(0.1));
    let tokens = css::components(position, ' ');
    // Three and four values give each keyword an offset from its edge.
    let mut pairs: Vec<(&str, Option<&str>)> = Vec::with_capacity(2);
    for token in tokens.iter().copied() {
        match pairs.last_mut() {
            Some(last)
                if tokens.len() > 2
                    && !matches!(token, "left" | "right" | "top" | "bottom" | "center") =>
            {
                last.1 = Some(token)
            }
            _ => pairs.push((token, None)),
        }
    }
    let center = ("center", None);
    let (x, y) = match pairs.as_slice() {
        [only] if matches!(only.0, "top" | "bottom") => (center, *only),
        [only] => (*only, center),
        [a, b, ..] if matches!(a.0, "top" | "bottom") || matches!(b.0, "left" | "right") => {
            (*b, *a)
        }
        [a, b, ..] => (*a, *b),
        [] => (("0%", None), ("0%", None)),
    };
    let at = [(x, width - tw), (y, height - th)].map(|((token, offset), free)| {
        let offset = offset
            .and_then(|offset| units.resolve(offset, free))
            .unwrap_or(0.);
        match token {
            "left" | "top" => offset,
            "center" => free / 2.,
            "right" | "bottom" => free - offset,
            value => units.resolve(value, free).unwrap_or(0.),
        }
    });
    [[tw, th], at]
}

/// The offset of a CSS `<position>` in `free`, the room left around the placed box.
pub(crate) fn place(position: &str, free: [f32; 2], units: &Lengths) -> [f32; 2] {
    let tokens = css::components(position, ' ');
    let keyword = |token: &str| matches!(token, "left" | "right" | "top" | "bottom" | "center");
    let offset = |token: &str, axis: usize| match token {
        "left" | "top" => 0.,
        "center" => free[axis] / 2.,
        "right" | "bottom" => free[axis],
        value => units.resolve(value, free[axis]).unwrap_or(0.),
    };
    if tokens.len() < 3 {
        let (x, y) = match tokens.as_slice() {
            [only] if matches!(*only, "top" | "bottom") => ("center", *only),
            [only] => (*only, "center"),
            [a, b] if matches!(*a, "top" | "bottom") || matches!(*b, "left" | "right") => (*b, *a),
            [a, b, ..] => (*a, *b),
            [] => ("0%", "0%"),
        };
        return [offset(x, 0), offset(y, 1)];
    }
    // Three or four values: keywords, each with an optional offset from its side.
    let mut at = [None; 2];
    let mut index = 0;
    while let Some(&side) = tokens.get(index) {
        let length = tokens.get(index + 1).filter(|token| !keyword(token));
        index += 1 + usize::from(length.is_some());
        let axis = match side {
            "left" | "right" => 0,
            "top" | "bottom" => 1,
            _ => continue,
        };
        let length = length.map_or(0., |value| units.resolve(value, free[axis]).unwrap_or(0.));
        at[axis] = Some(if matches!(side, "right" | "bottom") {
            free[axis] - length
        } else {
            length
        });
    }
    [0, 1].map(|axis| at[axis].unwrap_or(free[axis] / 2.))
}

fn gradient_stops<S: AsRef<str>>(
    parts: &[S],
    prelude: bool,
    units: &Lengths,
    extent: f32,
    wide: bool,
) -> Option<Vec<(f32, (u32, u8))>> {
    let color = if wide { css::wide_rgb } else { css::color };
    let mut stops: Vec<(Option<f32>, (u32, u8))> = Vec::new();
    let mut hints = Vec::new();
    for stop in parts.iter().map(AsRef::as_ref).skip(usize::from(prelude)) {
        let tokens = css::components(stop.trim(), ' ');
        let Some(ink) = tokens.iter().find_map(|token| color(token)) else {
            // A lone position between two colors moves their midpoint.
            if let [token] = tokens.as_slice()
                && !stops.is_empty()
                && let Some(offset) = units.resolve(token, extent).or_else(|| angle(token, units))
            {
                hints.push(stops.len());
                stops.push((Some(offset), (0, 0)));
            }
            continue;
        };
        let offsets: Vec<f32> = tokens
            .iter()
            .filter(|token| css::color(token).is_none())
            .filter_map(|token| units.resolve(token, extent).or_else(|| angle(token, units)))
            .take(2)
            .collect();
        if offsets.is_empty() {
            stops.push((None, ink));
        }
        stops.extend(offsets.into_iter().map(|offset| (Some(offset), ink)));
    }
    if stops.len() < 2 {
        return None;
    }
    let last = stops.len() - 1;
    stops[0].0.get_or_insert(0.);
    stops[last].0.get_or_insert(extent);
    let mut floor = f32::MIN;
    for stop in &mut stops {
        if let Some(offset) = &mut stop.0 {
            *offset = offset.max(floor);
            floor = *offset;
        }
    }
    let mut index = 1;
    while index < last {
        if stops[index].0.is_none() {
            let end = (index..=last)
                .find(|&end| stops[end].0.is_some())
                .unwrap_or(last);
            let (a, b) = (
                stops[index - 1].0.unwrap_or(0.),
                stops[end].0.unwrap_or(extent),
            );
            for (step, stop) in stops[index..end].iter_mut().enumerate() {
                stop.0 = Some(a + (b - a) * (step + 1) as f32 / (end - index + 1) as f32);
            }
            index = end;
        }
        index += 1;
    }
    let mut result = Vec::with_capacity(stops.len() + hints.len() * 8);
    for (index, &(offset, ink)) in stops.iter().enumerate() {
        let offset = offset.unwrap_or(0.);
        if !hints.contains(&index) {
            result.push((offset, ink));
            continue;
        }
        let (Some(&(a, (ink_a, alpha_a))), Some(&(Some(b), (ink_b, alpha_b)))) = (
            result.last(),
            stops
                .get(index + 1)
                .filter(|_| !hints.contains(&(index + 1))),
        ) else {
            continue;
        };
        if b <= a {
            continue;
        }
        let exponent = 0.5f32.ln() / ((offset - a) / (b - a)).clamp(0.001, 0.999).ln();
        for step in 1..16 {
            let t = step as f32 / 16.;
            let p = t.powf(exponent);
            let mix = |a: u32, b: u32| (a as f32 + (b as f32 - a as f32) * p).round() as u32;
            let (shifts, mask) = if wide {
                ([20, 10, 0], 1023)
            } else {
                ([16, 8, 0], 255)
            };
            let ink = shifts
                .map(|shift| mix(ink_a >> shift & mask, ink_b >> shift & mask) << shift)
                .into_iter()
                .fold(0, |ink, channel| ink | channel);
            result.push((
                a + (b - a) * t,
                (ink, mix(alpha_a.into(), alpha_b.into()) as u8),
            ));
        }
    }
    (result.len() >= 2).then_some(result)
}

pub(super) fn axis_mask(
    style: &css::Style,
    units: &Lengths,
    [width, height]: [f32; 2],
    raster: [u32; 2],
) -> Option<Option<Vec<u8>>> {
    let get =
        |[prop, webkit]: [Prop; 2]| style.get(prop).or_else(|| style.get(webkit)).unwrap_or("");
    let plain = |value: &str, defaults: &[&str]| {
        css::components(value, ',').iter().all(|part| {
            let part = part.trim();
            part.is_empty() || defaults.contains(&part)
        })
    };
    if !plain(
        get([Prop::MASK_SIZE, Prop::WEBKIT_MASK_SIZE]),
        &["auto", "auto auto"],
    ) || !plain(
        get([Prop::MASK_POSITION, Prop::WEBKIT_MASK_POSITION]),
        &["0% 0%", "0 0", "left top"],
    ) {
        return None;
    }
    let images = css::components(get([Prop::MASK_IMAGE, Prop::WEBKIT_MASK_IMAGE]), ',');
    let composites = css::components(style.get(Prop::MASK_COMPOSITE).unwrap_or(""), ',');
    let mut layers = Vec::with_capacity(images.len());
    for image in &images {
        let mut axes = [
            vec![1f32; raster[0] as usize],
            vec![1f32; raster[1] as usize],
        ];
        let body = image
            .trim()
            .strip_prefix("linear-gradient(")?
            .strip_suffix(')')?;
        let parts = css::components(body, ',');
        let (first, space) = first_argument(body);
        if space.is_some() {
            return None;
        }
        let (axis, reversed, prelude) = match first.as_str() {
            "to bottom" | "180deg" => (1, false, true),
            "to top" | "0deg" => (1, true, true),
            "to right" | "90deg" => (0, false, true),
            "to left" | "270deg" => (0, true, true),
            other
                if other.is_empty()
                    || css::components(other, ' ')
                        .first()
                        .is_some_and(|part| css::color(part).is_some()) =>
            {
                (1, false, false)
            }
            _ => return None,
        };
        let extent = [width, height][axis];
        let stops = gradient_stops(&parts, prelude, units, extent, false)?;
        let count = raster[axis] as usize;
        for (index, alpha) in axes[axis].iter_mut().enumerate() {
            let at = (index as f32 + 0.5) * extent / count as f32;
            let at = if reversed { extent - at } else { at };
            let end = stops.partition_point(|(offset, _)| *offset <= at);
            *alpha *= match (
                end.checked_sub(1).map(|start| stops[start]),
                stops.get(end).copied(),
            ) {
                (Some((a, (_, from))), Some((b, (_, to)))) if b > a => {
                    let t = (at - a) / (b - a);
                    (f32::from(from) * (1. - t) + f32::from(to) * t) / 255.
                }
                (Some((_, (_, alpha))), _) | (None, Some((_, (_, alpha)))) => {
                    f32::from(alpha) / 255.
                }
                (None, None) => 1.,
            };
        }
        layers.push(axes);
    }
    // As in `mask_svg`, layers go from the bottom up, and each joins the layers below it by its `mask-composite`.
    let [width, height] = raster.map(|side| side as usize);
    let (last, above) = layers.split_last()?;
    let ops: Vec<u8> = (0..above.len())
        .map(|index| {
            let composite = composites.get(index % composites.len().max(1));
            match composite.map_or("", |value| value.trim()) {
                "intersect" | "source-in" => 1,
                "subtract" | "source-out" => 2,
                "exclude" | "xor" => 3,
                _ => 0,
            }
        })
        .collect();
    let mut alpha = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let mut below = last[0][x] * last[1][y];
            for ([across, down], op) in above.iter().zip(&ops).rev() {
                let layer = across[x] * down[y];
                below = match op {
                    1 => layer * below,
                    2 => layer * (1. - below),
                    3 => layer * (1. - below) + below * (1. - layer),
                    _ => layer + below * (1. - layer),
                };
            }
            alpha.push(below);
        }
    }
    if alpha.iter().all(|alpha| *alpha >= 0.999) {
        return Some(None);
    }
    let mut bytes = vec![0; width * height * 4];
    for (pixel, alpha) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(alpha) {
        pixel[3] = (alpha * 255. + 0.5) as u8;
    }
    Some(Some(bytes))
}

// A CSS gradient, resolved to straight color stops along its extent: px for linear and radial, degrees for conic.
struct Gradient<'a> {
    kind: &'a str,
    steps: [Option<f32>; 2],
    tile: [f32; 2],
    at: [f32; 2],
    repeating: bool,
    direction: (f32, f32),
    center: (f32, f32),
    radii: (f32, f32),
    from: f32,
    extent: f32,
    wide: bool,
    offsets: Vec<(f32, (u32, u8))>,
    start: f32,
    period: f32,
}

impl Gradient<'_> {
    fn channels(&self) -> ([u32; 3], u32) {
        match self.wide {
            true => ([20, 10, 0], 1023),
            false => ([16, 8, 0], 255),
        }
    }

    // The sRGB color of a stop, or its Display P3 color when the gradient is wide.
    fn output(&self, ink: u32, alpha: u8) -> u32 {
        match self.wide {
            true => css::p3(&stop_css(true, ink, alpha)).map_or(ink, |(ink, _)| ink),
            false => ink,
        }
    }

    // The straight color at `angle` degrees of a conic gradient, before `output`. `next` is the stop after the
    // last angle, so increasing angles find their stop in a step.
    fn conic_at(&self, angle: f32, next: &mut usize) -> (u32, f32) {
        let angle = match self.repeating {
            true => self.start + (angle - self.start).rem_euclid(self.period),
            false => angle,
        };
        let last = self.offsets.len() - 1;
        if *next > last || *next > 0 && self.offsets[*next - 1].0 >= angle {
            *next = 0;
        }
        while *next < last && self.offsets[*next].0 < angle {
            *next += 1;
        }
        let next = *next;
        let (a, b) = (self.offsets[next.saturating_sub(1)], self.offsets[next]);
        let t = if b.0 > a.0 {
            ((angle - a.0) / (b.0 - a.0)).clamp(0., 1.)
        } else {
            1.
        };
        let (shifts, mask) = self.channels();
        let ink = shifts.map(|shift| {
            let mix = (((a.1).0 >> shift) & mask) as f32 * (1. - t)
                + (((b.1).0 >> shift) & mask) as f32 * t;
            (mix.round() as u32) << shift
        });
        let alpha = ((a.1).1 as f32 * (1. - t) + (b.1).1 as f32 * t) / 255.;
        (ink[0] | ink[1] | ink[2], alpha)
    }
}

fn stop_css(wide: bool, ink: u32, alpha: u8) -> String {
    match wide {
        true => {
            let [r, g, b] = [20, 10, 0].map(|shift| (ink >> shift & 1023) as f32 / 511.5 - 0.5);
            format!("color(srgb {r} {g} {b} / {})", alpha as f32 / 255.)
        }
        false => format!("#{ink:06x}{alpha:02x}"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn gradient_svg(
    kind: &str,
    body: &str,
    size: &str,
    position: &str,
    repeat: &str,
    units: Lengths,
    width: f32,
    height: f32,
    area: [f32; 4],
) -> Option<(String, bool)> {
    let gradient = gradient(kind, body, size, position, repeat, units, area)?;
    let Gradient {
        tile: [tw, th],
        at: [x, y],
        repeating,
        direction,
        center,
        radii,
        from,
        extent,
        wide,
        start,
        period,
        ..
    } = gradient;
    let offsets = &gradient.offsets;
    let spread = if repeating { "repeat" } else { "pad" };
    let svg_stops = offsets
        .iter()
        .map(|&(offset, (ink, alpha))| {
            format!(
                "<stop offset=\"{}\" stop-color=\"#{:06x}\" stop-opacity=\"{}\"/>",
                ((offset - start) / period).clamp(0., 1.),
                gradient.output(ink, alpha),
                alpha as f32 / 255.
            )
        })
        .collect::<String>();
    let content = match gradient.kind {
        "linear-gradient" => {
            let (dx, dy) = direction;
            let x1 = tw / 2. - dx * extent / 2. + dx * start;
            let y1 = th / 2. - dy * extent / 2. + dy * start;
            format!(
                "<defs><linearGradient id=\"g\" gradientUnits=\"userSpaceOnUse\" x1=\"{x1}\" y1=\"{y1}\" x2=\"{}\" y2=\"{}\" spreadMethod=\"{spread}\">{svg_stops}</linearGradient></defs><rect width=\"{tw}\" height=\"{th}\" fill=\"url(#g)\"/>",
                x1 + dx * period,
                y1 + dy * period
            )
        }
        "radial-gradient" => {
            let (cx, cy) = center;
            format!(
                "<defs><radialGradient id=\"g\" gradientUnits=\"userSpaceOnUse\" cx=\"{cx}\" cy=\"{cy}\" r=\"{period}\" spreadMethod=\"{spread}\" gradientTransform=\"translate({cx} {cy}) scale(1 {}) translate({} {})\">{svg_stops}</radialGradient></defs><rect width=\"{tw}\" height=\"{th}\" fill=\"url(#g)\"/>",
                radii.1 / radii.0.max(0.01),
                -cx,
                -cy
            )
        }
        _ => {
            let (cx, cy) = center;
            let reach = tw.hypot(th) * 2.;
            // Wedges end at each stop, so thin and hard stops keep their angles, and are at most 2deg wide.
            let mut cuts = vec![0f32, 360.];
            let repeats = if repeating {
                (360. / period).ceil() + 1.
            } else {
                1.
            };
            if repeats * offsets.len() as f32 <= 4096. {
                let first = if repeating {
                    ((-start) / period).floor()
                } else {
                    0.
                };
                for turn in 0..repeats as i32 + 1 {
                    let shift = (first + turn as f32) * if repeating { period } else { 0. };
                    cuts.extend(
                        offsets
                            .iter()
                            .map(|(offset, _)| offset + shift)
                            .filter(|angle| (0. ..360.).contains(angle)),
                    );
                }
            } else {
                cuts.extend((1..1440).map(|step| step as f32 / 4.));
            }
            cuts.sort_by(f32::total_cmp);
            cuts.dedup_by(|b, a| *b - *a < 1e-3);
            let mut pieces = Vec::with_capacity(cuts.len() * 2);
            for pair in cuts.windows(2) {
                let count = ((pair[1] - pair[0]) / 2.).ceil().max(1.);
                let width = (pair[1] - pair[0]) / count;
                pieces.extend((0..count as u32).map(|n| pair[0] + n as f32 * width));
            }
            pieces.push(360.);
            let mut paths = String::new();
            for wedge in pieces.windows(2) {
                let (ink, alpha) = gradient.conic_at((wedge[0] + wedge[1]) / 2., &mut 0);
                let ink = gradient.output(ink, (alpha * 255.) as u8);
                let [start, end] = [from + wedge[0], from + wedge[1]].map(f32::to_radians);
                paths.push_str(&format!(
                    "<path d=\"M{cx} {cy}L{} {}L{} {}Z\" fill=\"#{ink:06x}\" fill-opacity=\"{alpha}\"/>",
                    cx + reach * start.sin(),
                    cy - reach * start.cos(),
                    cx + reach * end.sin(),
                    cy - reach * end.cos()
                ));
            }
            format!(
                "<defs><clipPath id=\"c\"><rect width=\"{tw}\" height=\"{th}\"/></clipPath></defs><g clip-path=\"url(#c)\" shape-rendering=\"crispEdges\">{paths}</g>"
            )
        }
    };
    let body = tiled(gradient.steps, [tw, th], [x, y], [width, height], &content);
    Some((
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">{body}</svg>"
        ),
        wide,
    ))
}

// The pixels of a conic gradient over a `width` by `height` area, at `rw` by `rh` pixels. Each pixel takes the color
// at the angle of its center, like Skia: a scripted angle that changes each frame then costs one pass, not an SVG
// of wedges.
#[allow(clippy::too_many_arguments)]
pub(crate) fn conic_image(
    kind: &str,
    body: &str,
    size: &str,
    position: &str,
    repeat: &str,
    units: Lengths,
    [width, height]: [f32; 2],
    area: [f32; 4],
    [rw, rh]: [u32; 2],
    previous: Option<&std::sync::Arc<renderer::RenderImage>>,
) -> Option<std::sync::Arc<renderer::RenderImage>> {
    let gradient = gradient(kind, body, size, position, repeat, units, area)?;
    let [[tw, th], [x, y]] = [gradient.tile, gradient.at];
    if tw <= 0. || th <= 0. {
        return None;
    }
    let [rw, rh] = [rw, rh].map(|side| side.clamp(1, super::canvas::RASTER_MAX));
    let [sx, sy] = [width / rw as f32, height / rh as f32];
    // One entry per half device pixel at the far corner, so hard stops stay sharp.
    let reach = (tw / sx).hypot(th / sy);
    let steps = ((reach * std::f32::consts::TAU * 2.).ceil() as usize).clamp(720, 1 << 16);
    // Runs of one color are common, and a Display P3 conversion is slow, so the last one is kept.
    let (mut last, mut next) = ((u32::MAX, 0, 0), 0);
    let table: Vec<[u8; 4]> = (0..steps)
        .map(|step| {
            let (ink, alpha) =
                gradient.conic_at((step as f32 + 0.5) * 360. / steps as f32, &mut next);
            let byte = (alpha * 255.) as u8;
            if (last.0, last.1) != (ink, byte) {
                last = (ink, byte, gradient.output(ink, byte));
            }
            let [r, g, b] =
                [16, 8, 0].map(|shift| ((last.2 >> shift & 255) as f32 * alpha + 0.5) as u8);
            [b, g, r, (alpha * 255. + 0.5) as u8]
        })
        .collect();
    let mode = repeat.split_whitespace().next().unwrap_or("repeat");
    let [wrap_x, wrap_y] = match mode {
        "no-repeat" => [false; 2],
        "repeat-x" => [true, false],
        "repeat-y" => [false, true],
        _ => [true; 2],
    };
    let (cx, cy) = gradient.center;
    let turn = steps as f32 / std::f32::consts::TAU;
    let from = gradient.from.to_radians().rem_euclid(std::f32::consts::TAU);
    // A scripted angle moves less than a table step in most frames, and then the pixels are the same.
    let digest = {
        use std::hash::{Hash, Hasher};
        let mut hasher = rustc_hash::FxHasher::default();
        table.hash(&mut hasher);
        [
            rw,
            rh,
            u32::from(wrap_x),
            u32::from(wrap_y),
            u32::from(gradient.wide),
        ]
        .hash(&mut hasher);
        [sx, sy, x, y, tw, th, cx, cy, from]
            .map(f32::to_bits)
            .hash(&mut hasher);
        hasher.finish()
    };
    if let Some(previous) = previous
        && DRAWN.with_borrow(|drawn| drawn.get(&previous.id) == Some(&digest))
    {
        return Some(previous.clone());
    }
    let mut bytes = SPARE.take();
    bytes.clear();
    bytes.resize(rw as usize * rh as usize * 4, 0);
    for (row, line) in bytes.chunks_exact_mut(rw as usize * 4).enumerate() {
        // `rem_euclid` calls `fmodf`, so only rows and columns outside the first tile take it.
        let mut v = (row as f32 + 0.5) * sy - y;
        if !(0. ..th).contains(&v) {
            match wrap_y {
                true => v = v.rem_euclid(th),
                false => continue,
            }
        }
        let dy = v - cy;
        for (column, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let mut u = (column as f32 + 0.5) * sx - x;
            if !(0. ..tw).contains(&u) {
                match wrap_x {
                    true => u = u.rem_euclid(tw),
                    false => continue,
                }
            }
            // atan2(u - cx, -dy) by a polynomial: libm's is most of the time of this loop.
            let (y, x) = (u - cx, -dy);
            let (ax, ay) = (x.abs(), y.abs());
            let a = ax.min(ay) / ax.max(ay).max(f32::MIN_POSITIVE);
            let q = a * a;
            let mut angle = ((-0.046_496_47 * q + 0.159_314_22) * q - 0.327_622_76) * q * a + a;
            if ay > ax {
                angle = std::f32::consts::FRAC_PI_2 - angle;
            }
            if x < 0. {
                angle = std::f32::consts::PI - angle;
            }
            if y < 0. {
                angle = -angle;
            }
            // `angle - from` is within two turns of the range.
            let mut angle = angle - from;
            while angle < 0. {
                angle += std::f32::consts::TAU;
            }
            while angle >= std::f32::consts::TAU {
                angle -= std::f32::consts::TAU;
            }
            *pixel = table[((angle * turn) as usize).min(steps - 1)];
        }
    }
    let image = renderer::RenderImage::new_premultiplied(rw, rh, bytes);
    let image = std::sync::Arc::new(match gradient.wide {
        true => image.display_p3(),
        false => image,
    });
    DRAWN.with_borrow_mut(|drawn| {
        if drawn.len() >= 256 {
            drawn.clear();
        }
        drawn.insert(image.id, digest);
    });
    Some(image)
}

thread_local! {
    // The pixels of the last released gradient image, for the next one. New pages cost more than a memset.
    static SPARE: std::cell::Cell<Vec<u8>> = const { std::cell::Cell::new(Vec::new()) };
    // The digests of the table and place of conic images, by image.
    static DRAWN: std::cell::RefCell<rustc_hash::FxHashMap<u64, u64>> = Default::default();
}

pub(crate) fn recycle(bytes: Vec<u8>) {
    if bytes.capacity() <= 4 << 20 {
        SPARE.set(bytes);
    }
}

#[allow(clippy::too_many_arguments)]
fn gradient<'a>(
    kind: &'a str,
    body: &str,
    size: &str,
    position: &str,
    repeat: &'a str,
    units: Lengths,
    area: [f32; 4],
) -> Option<Gradient<'a>> {
    let ([[tw, th], [x, y]], steps) =
        tile_repeat(size, position, repeat, &units, area[2], area[3], None);
    let [x, y] = [x + area[0], y + area[1]];
    let parts = css::components(body, ',');
    let (first, space) = first_argument(body);
    let first = first.as_str();
    let repeating = kind.starts_with("repeating-");
    let prelude;
    let (mut direction, mut center, mut radii, mut from) =
        ((0f32, 0f32), (tw / 2., th / 2.), (0f32, 0f32), 0f32);
    let extent = match kind.trim_start_matches("repeating-") {
        "linear-gradient" => {
            let degrees = if let Some(side) = first.strip_prefix("to ") {
                prelude = true;
                let has = |name: &str| side.split_whitespace().any(|part| part == name);
                let corner = th.atan2(tw).to_degrees();
                match (has("top"), has("bottom"), has("left"), has("right")) {
                    (true, _, false, true) => corner,
                    (_, true, false, true) => 180. - corner,
                    (_, true, true, _) => 180. + corner,
                    (true, _, true, _) => 360. - corner,
                    (true, ..) => 0.,
                    (_, true, ..) => 180.,
                    (_, _, true, _) => 270.,
                    _ => 90.,
                }
            } else if let Some(degrees) = angle(first, &units) {
                prelude = true;
                degrees
            } else if space.is_some() && !first.is_empty() {
                return None;
            } else {
                prelude = space.is_some();
                180.
            };
            direction = (degrees.to_radians().sin(), -degrees.to_radians().cos());
            tw * direction.0.abs() + th * direction.1.abs()
        }
        "radial-gradient" => {
            prelude = space.is_some()
                || ["circle", "ellipse", "at ", "side", "corner"]
                    .iter()
                    .any(|word| first.contains(word))
                || first.starts_with("at")
                || !first.is_empty()
                    && css::color(first).is_none()
                    && first
                        .split_whitespace()
                        .all(|part| units.resolve(part, tw).is_some());
            let (shape, at) = if prelude {
                first
                    .split_once("at ")
                    .map_or((first, ""), |(shape, at)| (shape.trim(), at.trim()))
            } else {
                ("", "")
            };
            let [cx, cy, _] = origin(at, &units, [tw, th]);
            center = (cx, cy);
            let lengths: Vec<f32> = shape
                .split_whitespace()
                .filter_map(|part| units.resolve(part, tw))
                .collect();
            let circle =
                shape.contains("circle") || !shape.contains("ellipse") && lengths.len() == 1;
            let sides = [cx.abs(), (tw - cx).abs(), cy.abs(), (th - cy).abs()];
            let keyword = ["closest-side", "farthest-side", "closest-corner"]
                .into_iter()
                .find(|word| shape.contains(word));
            radii = match (lengths.as_slice(), keyword) {
                ([r], _) => (*r, *r),
                ([a, b, ..], _) => (*a, *b),
                (_, keyword) if circle => {
                    let corners = [
                        sides[0].hypot(sides[2]),
                        sides[0].hypot(sides[3]),
                        sides[1].hypot(sides[2]),
                        sides[1].hypot(sides[3]),
                    ];
                    let r = match keyword {
                        Some("closest-side") => sides.into_iter().fold(f32::MAX, f32::min),
                        Some("farthest-side") => sides.into_iter().fold(0., f32::max),
                        Some(_) => corners.into_iter().fold(f32::MAX, f32::min),
                        None => corners.into_iter().fold(0., f32::max),
                    };
                    (r, r)
                }
                (_, keyword) => {
                    let (x, y) = match keyword {
                        Some("closest-side") | Some("closest-corner") => {
                            (sides[0].min(sides[1]), sides[2].min(sides[3]))
                        }
                        _ => (sides[0].max(sides[1]), sides[2].max(sides[3])),
                    };
                    if keyword.is_some_and(|word| word.ends_with("side")) {
                        (x, y)
                    } else {
                        (x * std::f32::consts::SQRT_2, y * std::f32::consts::SQRT_2)
                    }
                }
            };
            radii.0.max(0.01)
        }
        "conic-gradient" => {
            prelude = space.is_some() || first.starts_with("from ") || first.starts_with("at ");
            if prelude {
                let (start, at) = first
                    .split_once("at ")
                    .map_or((first, ""), |(start, at)| (start, at.trim()));
                from = start
                    .trim()
                    .strip_prefix("from")
                    .and_then(|v| angle(v.trim(), &units))
                    .unwrap_or(0.);
                let [cx, cy, _] = origin(at, &units, [tw, th]);
                center = (cx, cy);
            }
            360.
        }
        _ => return None,
    };
    // Like Chrome on a wide-gamut screen, colors outside sRGB keep their gamut. Then stops are `css::wide_rgb`, and
    // the image is Display P3.
    let wide = cfg!(target_os = "macos")
        && (parts.iter()).any(|part| {
            css::components(part, ' ')
                .iter()
                .any(|token| css::wide(token))
        });
    let color = match wide {
        true => css::wide_rgb as fn(&str) -> _,
        false => css::color as fn(&str) -> _,
    };
    let css = |ink, alpha| stop_css(wide, ink, alpha);
    let stops = gradient_stops(&parts, prelude, &units, extent, wide)?;
    // Gradients mix premultiplied colors, and SVG mixes straight ones. So a transparent stop takes the color of
    // each neighbor, and other alpha changes get mixed stops.
    let mut offsets = Vec::with_capacity(stops.len() + 2);
    for (index, &(offset, (ink, alpha))) in stops.iter().enumerate() {
        if alpha > 0 {
            offsets.push((offset, (ink, alpha)));
            continue;
        }
        let [left, right] = [index.wrapping_sub(1), index + 1].map(|at| stops.get(at));
        for side in [left.or(right), right.or(left)] {
            offsets.push((offset, (side.map_or(ink, |stop| stop.1.0), 0)));
        }
    }
    let space = space.filter(|space| space != "srgb").or_else(|| {
        let modern = ["oklch(", "oklab(", "lab(", "lch(", "color("];
        modern
            .iter()
            .any(|name| body.contains(name))
            .then(|| "oklab".to_owned())
    });
    if space.is_some() || offsets.windows(2).any(|pair| pair[0].1.1 != pair[1].1.1) {
        let mut mixed = Vec::with_capacity(offsets.len() * 8);
        for pair in offsets.windows(2) {
            let [(a, (ink_a, alpha_a)), (b, (ink_b, alpha_b))] = [pair[0], pair[1]];
            mixed.push(pair[0]);
            if ink_a == ink_b || space.is_none() && alpha_a == alpha_b || a == b {
                continue;
            }
            let space = space.as_deref().unwrap_or("srgb");
            let [a_css, b_css] = [css(ink_a, alpha_a), css(ink_b, alpha_b)];
            for step in 1..8 {
                let t = step as f32 / 8.;
                let mix = format!(
                    "color-mix(in {space}, {a_css} {}%, {b_css})",
                    (1. - t) * 100.
                );
                if let Some(ink) = color(&mix) {
                    mixed.push((a + (b - a) * t, ink));
                }
            }
        }
        mixed.extend(offsets.last().copied());
        offsets = mixed;
    }
    let last = offsets.len() - 1;
    let start = if repeating && !kind.contains("radial") {
        offsets[0].0
    } else {
        0.
    };
    let period = if repeating {
        offsets[last].0 - start
    } else {
        extent
    };
    if period <= 0. {
        return None;
    }
    Some(Gradient {
        kind: kind.trim_start_matches("repeating-"),
        steps,
        tile: [tw, th],
        at: [x, y],
        repeating,
        direction,
        center,
        radii,
        from,
        extent,
        wide,
        offsets,
        start,
        period,
    })
}

/// The tile and its place in an area, as `tile` gives them, after `round` sized it, and the step from one tile to
/// the next on each axis, or `None` where it doesn't repeat.
pub(super) fn tile_repeat(
    size: &str,
    position: &str,
    repeat: &str,
    units: &Lengths,
    width: f32,
    height: f32,
    natural: Option<[f32; 2]>,
) -> ([[f32; 2]; 2], [Option<f32>; 2]) {
    let words: Vec<&str> = repeat.split_whitespace().collect();
    let modes = match words.as_slice() {
        ["repeat-x"] => ["repeat", "no-repeat"],
        ["repeat-y"] => ["no-repeat", "repeat"],
        [one] => [*one; 2],
        [a, b, ..] => [*a, *b],
        [] => ["repeat"; 2],
    };
    let extent = [width, height];
    let [mut sides, mut at] = tile(size, position, units, width, height, natural);
    if modes.contains(&"round") {
        let side = |axis: usize| match modes[axis] {
            "round" => extent[axis] / (extent[axis] / sides[axis]).round().max(1.),
            _ => sides[axis],
        };
        let size = format!("{}px {}px", side(0), side(1));
        [sides, at] = tile(&size, position, units, width, height, natural);
    }
    let steps = std::array::from_fn(|axis| match modes[axis] {
        "no-repeat" => None,
        // Whole tiles touch both edges, with even gaps between them.
        "space" => {
            let count = (extent[axis] / sides[axis]).floor();
            (count >= 2.).then(|| {
                at[axis] = 0.;
                sides[axis] + (extent[axis] - count * sides[axis]) / (count - 1.)
            })
        }
        _ => Some(sides[axis]),
    });
    ([sides, at], steps)
}

/// SVG that repeats `content`, a tile of `size` at `at`, over the area by `steps`.
pub(super) fn tiled(
    steps: [Option<f32>; 2],
    [tw, th]: [f32; 2],
    [x, y]: [f32; 2],
    area: [f32; 2],
    content: &str,
) -> String {
    // A pattern renders its tile, then samples it for each pixel. A few copies of the tile draw each pixel once.
    let starts = |step: Option<f32>, at: f32, extent: f32| match step {
        Some(step) if step > 0. => {
            let first = at - (at / step).ceil() * step;
            let count = ((extent - first) / step).ceil().max(0.) as usize;
            (count <= 3).then(|| (0..count).map(|n| first + n as f32 * step).collect())
        }
        Some(_) => None,
        None => Some(vec![at]),
    };
    if let (Some(xs), Some(ys)) = (starts(steps[0], x, area[0]), starts(steps[1], y, area[1])) {
        return ys
            .iter()
            .flat_map(|y| xs.iter().map(move |x| (x, y)))
            .map(|(x, y)| format!("<g transform=\"translate({x} {y})\">{content}</g>"))
            .collect();
    }
    let span = |step: Option<f32>, at: f32, side: f32| match step {
        Some(_) => ("0".to_owned(), "100%".to_owned()),
        None => (at.to_string(), side.to_string()),
    };
    let ((rx, rw), (ry, rh)) = (span(steps[0], x, tw), span(steps[1], y, th));
    format!(
        "<defs><pattern id=\"p\" patternUnits=\"userSpaceOnUse\" width=\"{}\" height=\"{}\" x=\"{x}\" y=\"{y}\">{content}</pattern></defs><rect x=\"{rx}\" y=\"{ry}\" width=\"{rw}\" height=\"{rh}\" fill=\"url(#p)\"/>",
        steps[0].unwrap_or(tw),
        steps[1].unwrap_or(th)
    )
}

pub(super) fn push_bits(key: &mut String, size: [f32; 2], units: &Lengths) {
    let [font, root] = [units.font, units.root]
        .map(|font| [font.em, font.ch, font.ex, font.cap, font.ic, font.lh]);
    let container = units.container.unwrap_or([f32::NAN; 2]);
    for &value in size
        .iter()
        .chain(&font)
        .chain(&root)
        .chain(&units.viewport)
        .chain(&container)
    {
        let bits = value.to_bits();
        key.extend(
            (0..8)
                .rev()
                .map(|digit| char::from(b"0123456789abcdef"[(bits >> (digit * 4) & 15) as usize])),
        );
    }
}

pub(super) fn opacity(style: &css::Style) -> f32 {
    style
        .get(Prop::OPACITY)
        .and_then(|value| {
            value
                .strip_suffix('%')
                .map_or(value.parse::<f32>().ok(), |v| {
                    v.parse::<f32>().ok().map(|v| v / 100.)
                })
        })
        .filter(|v| v.is_finite())
        .unwrap_or(1.)
        .clamp(0., 1.)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Establishes {
    pub(crate) stacking: bool,
    pub(crate) absolute: bool,
    pub(crate) fixed: bool,
    pub(crate) layer: bool,
    pub(crate) transformed: bool,
}

// Share this decision across build, cull and cache keys. Callers add flex/grid z-index and opacity layers.
pub(crate) fn establishes(style: &Style) -> Establishes {
    let set = |props: &[Prop]| {
        props
            .iter()
            .any(|&prop| style.get(prop).is_some_and(|value| value != "none"))
    };
    if style
        .get(Prop::DISPLAY)
        .is_some_and(|value| matches!(value, "contents" | "none"))
    {
        return Establishes::default();
    }
    let position = style.get(Prop::POSITION).unwrap_or("static");
    let transformed = set(&[Prop::TRANSFORM, Prop::TRANSLATE, Prop::ROTATE, Prop::SCALE]);
    let layer = set(&[
        Prop::FILTER,
        Prop::WEBKIT_FILTER,
        Prop::CLIP_PATH,
        Prop::WEBKIT_CLIP_PATH,
        Prop::MASK_IMAGE,
        Prop::WEBKIT_MASK_IMAGE,
        Prop::BACKDROP_FILTER,
        Prop::WEBKIT_BACKDROP_FILTER,
    ]) || style
        .get(Prop::MIX_BLEND_MODE)
        .is_some_and(|value| value != "normal")
        || style.get(Prop::ISOLATION) == Some("isolate")
        || cfg!(target_os = "macos")
            && matches!(style.get(Prop::MACOS_GLASS), Some("regular" | "clear"));
    let contained = style.get(Prop::CONTAIN).is_some_and(|value| {
        ["layout", "paint", "strict", "content"]
            .iter()
            .any(|kind| value.contains(kind))
    }) || style
        .get(Prop::CONTENT_VISIBILITY)
        .is_some_and(|value| value != "visible");
    let (mut wills, mut will_stack, mut will_position) = (false, false, false);
    for name in style
        .get(Prop::WILL_CHANGE)
        .into_iter()
        .flat_map(|value| value.split(','))
    {
        let is = |names: &[&str]| {
            names
                .iter()
                .any(|known| name.trim().eq_ignore_ascii_case(known))
        };
        wills |= is(&[
            "transform",
            "translate",
            "rotate",
            "scale",
            "perspective",
            "filter",
            "-webkit-filter",
            "backdrop-filter",
            "-webkit-backdrop-filter",
            "transform-style",
            "contain",
            "content-visibility",
        ]);
        will_position |= is(&["position"]);
        will_stack |= is(&[
            "opacity",
            "clip-path",
            "-webkit-clip-path",
            "mask",
            "mask-image",
            "-webkit-mask",
            "-webkit-mask-image",
            "mix-blend-mode",
            "isolation",
        ]);
    }
    let fixed = transformed
        || contained
        || wills
        || set(&[
            Prop::PERSPECTIVE,
            Prop::FILTER,
            Prop::WEBKIT_FILTER,
            Prop::BACKDROP_FILTER,
            Prop::WEBKIT_BACKDROP_FILTER,
        ])
        || style.get(Prop::TRANSFORM_STYLE) == Some("preserve-3d");
    Establishes {
        stacking: fixed
            || layer
            || will_stack
            || will_position
            || matches!(position, "fixed" | "sticky")
            || position != "static"
                && style
                    .get(Prop::Z_INDEX)
                    .is_some_and(|value| value.parse::<i32>().is_ok())
            || set(&[Prop::MASK, Prop::WEBKIT_MASK])
            || opacity(style) < 1.,
        absolute: fixed || will_position || position != "static",
        fixed,
        layer,
        transformed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use renderer::size;

    fn style(pairs: &[(&str, &str)]) -> Style {
        pairs
            .iter()
            .map(|(key, value)| (crate::css::Prop::new(key), (*value).into()))
            .collect()
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn transforms_move_with_their_box() {
        let units = Lengths::new(16., 800., 600.);
        let at = |x: f32, y: f32| Rect::new(renderer::point(px(x), px(y)), size(px(120.), px(40.)));
        for pairs in [
            &[("transform", "rotate(30deg) translate(10%, 2em) scale(1.5)")][..],
            &[
                ("translate", "-40px 25%"),
                ("rotate", "12deg"),
                ("transform-origin", "0 100%"),
            ],
            &[
                (
                    "transform",
                    "perspective(200px) rotateY(40deg) skewX(10deg)",
                ),
                ("transform-origin", "20px 5px"),
            ],
        ] {
            let style = style(pairs);
            let local = transform(&style, &units, at(0., 0.), None).unwrap();
            let moved = multiply(
                &translate(300., 70., 0.),
                &multiply(&local, &translate(-300., -70., 0.)),
            );
            let placed = transform(&style, &units, at(300., 70.), None).unwrap();
            for corner in [[300., 70.], [420., 110.], [350., 90.]] {
                let (a, b) = (map(&moved, corner).unwrap(), map(&placed, corner).unwrap());
                assert!(close(a, b), "{pairs:?} {a:?} {b:?}");
            }
        }
    }

    // The SVG draws the two parts of `exclude` one over the other, so it matches the XOR of the standard only
    // where alpha is 0 or 1.
    #[test]
    fn axis_masks_draw_as_their_svg() {
        let units = Lengths::new(16., 800., 600.);
        for (image, composite) in [
            (
                "linear-gradient(#000 0 0), linear-gradient(#000 0 0)",
                "exclude",
            ),
            (
                "linear-gradient(to right, black 30%, transparent 70%), linear-gradient(to top, transparent, black)",
                "subtract",
            ),
            (
                "linear-gradient(to right, black 30%, transparent 70%), linear-gradient(to top, transparent, black)",
                "intersect",
            ),
            (
                "linear-gradient(to left, black, transparent), linear-gradient(to bottom, transparent 20%, black)",
                "add",
            ),
            (
                "linear-gradient(to left, black, transparent), linear-gradient(black 40%, transparent), linear-gradient(to right, transparent 10%, black)",
                "subtract, add",
            ),
        ] {
            let style = style(&[("mask-image", image), ("mask-composite", composite)]);
            let fast = axis_mask(&style, &units, [100., 50.], [100, 50]).unwrap();
            let layers = super::super::paint::mask_svg(
                &style,
                units,
                [100., 50.],
                None,
                &Default::default(),
                &mut Vec::new(),
            )
            .unwrap();
            let source = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"50\">{layers}</svg>"
            );
            let tree = super::super::canvas::svg_tree(&source, false).unwrap();
            let slow = super::super::canvas::svg_pixels(&tree, 100, 50, None).unwrap();
            let fast = fast.unwrap_or_else(|| vec![255; 100 * 50 * 4]);
            let worst = fast
                .as_chunks::<4>()
                .0
                .iter()
                .zip(slow.pixels())
                .map(|(fast, slow)| fast[3].abs_diff(slow.alpha()))
                .max();
            assert!(worst <= Some(8), "{image} {composite}: {worst:?}");
        }
    }

    #[test]
    fn gradient_masks_composite_along_their_axes() {
        let units = Lengths::new(16., 800., 600.);
        let mask = |image: &str, composite: Option<&str>, raster: [u32; 2]| {
            let mut props = vec![("mask-image", image)];
            props.extend(composite.map(|value| ("mask-composite", value)));
            axis_mask(&style(&props), &units, [100., 50.], raster)
        };
        let alpha = |bytes: &[u8], x: usize, y: usize| bytes[(y * 100 + x) * 4 + 3];
        let opaque = "linear-gradient(to left, black calc(100% - min(12px, 0px)), transparent 100%), linear-gradient(#fff, #fff)";
        assert_eq!(mask(opaque, Some("intersect"), [100, 50]), Some(None));
        let fades = "linear-gradient(to right, transparent, black 10px), linear-gradient(to top, transparent, black 10px)";
        let bytes = mask(fades, Some("intersect"), [100, 50]).unwrap().unwrap();
        assert!(alpha(&bytes, 0, 0) < 26 && alpha(&bytes, 50, 0) == 255);
        assert!(alpha(&bytes, 50, 49) < 26);
        let bytes = mask(fades, None, [100, 50]).unwrap().unwrap();
        assert!(alpha(&bytes, 0, 0) == 255 && alpha(&bytes, 0, 49) < 26);
        let halves =
            "linear-gradient(to right, black 50%, transparent 50%), linear-gradient(#000 0 0)";
        let bytes = mask(halves, Some("exclude"), [100, 50]).unwrap().unwrap();
        assert!(alpha(&bytes, 10, 10) == 0 && alpha(&bytes, 90, 10) == 255);
        let bytes = mask(halves, Some("subtract"), [100, 50]).unwrap().unwrap();
        assert!(bytes.iter().all(|byte| *byte == 0));
        assert!(
            mask(
                "linear-gradient(45deg, black, transparent)",
                None,
                [100, 50]
            )
            .is_none()
        );
    }

    #[test]
    fn transforms_map_window_points_around_the_origin() {
        let units = Lengths::new(16., 800., 600.);
        let bounds = Rect::new(point(px(100.), px(50.)), size(px(10.), px(20.)));
        let moved = transform(
            &style(&[("transform", "translate(5px, 50%)")]),
            &units,
            bounds,
            None,
        )
        .unwrap();
        assert_eq!(translation(&moved), Some([5., 10.]));
        assert_eq!(
            transform(
                &style(&[("transform", "rotate(0deg)")]),
                &units,
                bounds,
                None
            ),
            None
        );
        let quarter = transform(&style(&[("rotate", "90deg")]), &units, bounds, None).unwrap();
        assert_eq!(translation(&quarter), None);
        assert!(close(map(&quarter, [105., 60.]).unwrap(), [105., 60.]));
        assert!(close(map(&quarter, [100., 50.]).unwrap(), [115., 55.]));
        let corner = transform(
            &style(&[("transform", "scale(2)"), ("transform-origin", "left top")]),
            &units,
            bounds,
            None,
        )
        .unwrap();
        assert!(close(map(&corner, [110., 70.]).unwrap(), [120., 90.]));
        let flipped = transform(
            &style(&[("transform", "rotateY(180deg)")]),
            &units,
            bounds,
            Some((300., [105., 60.])),
        )
        .unwrap();
        assert!(backface(&flipped, bounds));
        assert!(!backface(&quarter, bounds));
        assert!(
            transform(
                &style(&[("transform", "rotate3d(0, 0, 0, 45deg)")]),
                &units,
                bounds,
                None
            )
            .is_none()
        );
    }

    #[test]
    fn positions_place_like_chrome() {
        let units = Lengths::new(16., 800., 600.);
        for (position, expected) in [
            ("left bottom", [0., 60.]),
            ("top", [50., 0.]),
            ("20% 0", [20., 0.]),
            ("right 5px top 10px", [95., 10.]),
            ("bottom 10px right", [100., 50.]),
            ("center bottom 1em", [50., 44.]),
        ] {
            assert_eq!(place(position, [100., 60.], &units), expected, "{position}");
        }
    }

    #[test]
    fn filters_split_into_layers() {
        let units = Lengths::new(16., 800., 600.);
        let doc = Document::default();
        let effects = filters(
            "grayscale(1) contrast(2) blur(4px) drop-shadow(1px 2px 6px red) opacity(.5)",
            &units,
            &doc,
            &Ids::default(),
        );
        assert_eq!(effects.len(), 4);
        assert!(effects[0].color_matrix.is_some());
        assert_eq!(effects[1].blur, px(4.));
        assert_eq!(
            effects[2].shadow.map(|(offset, blur, _)| (offset, blur)),
            Some((point(px(1.), px(2.)), px(6.)))
        );
        assert_eq!(effects[3].color_matrix.map(|matrix| matrix[18]), Some(0.5));
        let (blur, matrix) = backdrop("blur(10px) saturate(1.5)", &units).unwrap();
        assert_eq!(blur, px(10.));
        assert!(matrix.is_some());
        assert_eq!(blend_mode("multiply"), BlendMode::Multiply);
        assert_eq!(blend_mode("unknown"), BlendMode::Normal);
    }

    #[test]
    fn linear_gradients_along_an_axis_are_found() {
        for (kind, body, axis) in [
            ("linear-gradient", "red, blue", Some(1)),
            ("linear-gradient", "to right, red, blue", Some(0)),
            ("linear-gradient", "in oklch to left, red, blue", Some(0)),
            ("linear-gradient", "-90deg, red, blue", Some(0)),
            ("linear-gradient", "0.5turn in srgb, red, blue", Some(1)),
            (
                "repeating-linear-gradient",
                "to top, red, blue 9px",
                Some(1),
            ),
            ("linear-gradient", "calc(0.25 * 360deg), red, blue", Some(0)),
            ("linear-gradient", "45deg, red, blue", None),
            ("linear-gradient", "to bottom right, red, blue", None),
            ("radial-gradient", "red, blue", None),
        ] {
            let units = Lengths::new(16., 800., 600.);
            assert_eq!(gradient_axis(kind, body, &units), axis, "{kind}({body})");
        }
    }

    #[test]
    fn gradient_stops_take_math_angles() {
        let units = Lengths::new(16., 800., 600.);
        let offsets = |parts: &[&str]| {
            gradient_stops(parts, false, &units, 360., false)
                .unwrap()
                .iter()
                .map(|(offset, _)| offset.round())
                .collect::<Vec<_>>()
        };
        assert_eq!(offsets(&["red calc(0.25 * 360deg)", "blue 0"]), [90., 90.]);
        assert_eq!(
            offsets(&[
                "red calc(0.25 * 360deg - 1deg)",
                "blue calc(0.5turn + 1deg)"
            ]),
            [89., 181.]
        );
        assert_eq!(offsets(&["red 0", "blue 25%", "green"]), [0., 90., 360.]);
    }

    #[test]
    fn gradient_hints_move_the_midpoint_like_chrome() {
        let units = Lengths::new(16., 800., 600.);
        let stops = gradient_stops(&["red", "80%", "blue"], false, &units, 100., false).unwrap();
        // Chrome mixes half and half at the hint, and less blue before it: t ^ (ln 0.5 / ln 0.8).
        let at = |offset: f32| {
            stops
                .iter()
                .find(|(at, _)| (at - offset).abs() < 0.01)
                .unwrap()
                .1
        };
        assert_eq!(at(0.), (0xff0000, 255));
        assert_eq!(at(50.), (0xe1001e, 255));
        assert_eq!(at(100.), (0x0000ff, 255));
    }

    #[test]
    fn conic_images_draw_like_their_svg() {
        let units = Lengths::new(16., 800., 600.);
        for (kind, body, size, position, repeat) in [
            (
                "conic-gradient",
                "red, yellow, blue",
                "",
                "0% 0%",
                "no-repeat",
            ),
            (
                "conic-gradient",
                "from 45deg at 30% 40%, red, #0f08, blue",
                "",
                "0% 0%",
                "no-repeat",
            ),
            (
                "conic-gradient",
                "#f00 0 90deg, #00f 90deg 200deg, #0f0 0",
                "",
                "0% 0%",
                "no-repeat",
            ),
            (
                "repeating-conic-gradient",
                "#000 0 15deg, #fff 0 30deg",
                "",
                "0% 0%",
                "no-repeat",
            ),
            (
                "conic-gradient",
                "red, blue",
                "25px 20px",
                "5px 5px",
                "repeat",
            ),
            (
                "conic-gradient",
                "red, blue",
                "25px 20px",
                "5px 5px",
                "repeat-x",
            ),
        ] {
            let area = [0., 0., 60., 40.];
            let image = conic_image(
                kind,
                body,
                size,
                position,
                repeat,
                units,
                [60., 40.],
                area,
                [120, 80],
                None,
            )
            .unwrap();
            let svg =
                gradient_svg(kind, body, size, position, repeat, units, 60., 40., area).unwrap();
            let svg = super::super::canvas::svg_gradient(svg, 120, 80).unwrap();
            let far = (image.bytes().as_chunks::<4>().0.iter())
                .zip(svg.bytes().as_chunks::<4>().0)
                .filter(|(a, b)| a.iter().zip(*b).any(|(a, b)| a.abs_diff(*b) > 24))
                .count();
            // The SVG draws wedges of 2deg at most, so pixels differ only along hard stops.
            assert!(far < 120 * 80 / 100, "{body}: {far} pixels differ");
        }
    }

    #[test]
    fn conic_images_draw_again_only_when_their_pixels_change() {
        let units = Lengths::new(16., 800., 600.);
        let draw = |turn: f32, previous| {
            let body = format!("red {turn}turn, transparent 0");
            let area = [0., 0., 100., 100.];
            conic_image(
                "conic-gradient",
                &body,
                "",
                "0% 0%",
                "",
                units,
                [100., 100.],
                area,
                [200, 200],
                previous,
            )
            .unwrap()
        };
        let first = draw(0.3, None);
        let still = draw(0.30001, Some(&first));
        assert!(
            std::sync::Arc::ptr_eq(&first, &still),
            "a move within a table step keeps the image"
        );
        let moved = draw(0.31, Some(&first));
        assert!(
            !std::sync::Arc::ptr_eq(&first, &moved),
            "a visible move draws again"
        );
    }

    #[test]
    fn gradient_interpolation_spaces_parse_in_any_order() {
        let units = Lengths::new(16., 800., 600.);
        for (body, valid) in [
            ("hue in hsl, red, blue", false),
            ("in hsl longer hue, red, blue", true),
            ("to right in oklch longer hue, red, blue", true),
        ] {
            let svg = gradient_svg(
                "linear-gradient",
                body,
                "auto",
                "",
                "",
                units,
                10.,
                10.,
                [0., 0., 10., 10.],
            );
            assert_eq!(svg.is_some(), valid, "{body}");
        }
        for kind in ["radial-gradient", "conic-gradient"] {
            for body in ["hue in hsl, red, blue", "in, red, blue"] {
                gradient_svg(
                    kind,
                    body,
                    "auto",
                    "",
                    "",
                    units,
                    10.,
                    10.,
                    [0., 0., 10., 10.],
                );
            }
        }
    }

    #[test]
    fn clip_paths_become_shapes() {
        let units = Lengths::new(16., 800., 600.);
        let doc = Document::parse(
            "<svg><clipPath id=c><rect width=5 height=5></rect></clipPath></svg><p id=c></p>",
        );
        let ids = &Ids::default();
        assert!(matches!(
            clip("inset(10px round 4px)", &units, [100., 60.], &doc, ids),
            Some(Clip::Rounded([10., 10., 80., 40.], [4., 4., 4., 4.]))
        ));
        let Some(Clip::Shape(polygon)) = clip(
            "polygon(50% 0, 100% 100%, 0 100%)",
            &units,
            [100., 60.],
            &doc,
            ids,
        ) else {
            panic!("polygon");
        };
        assert!(polygon.contains("points=\"50,0 100,60 0,60 \""));
        let Some(Clip::Shape(pill)) = clip("inset(0 round 50%)", &units, [100., 60.], &doc, ids)
        else {
            panic!("elliptical inset");
        };
        assert!(pill.contains("M50 0H50A50 30 0 0 1 100 30"), "{pill}");
        assert!(matches!(
            clip("inset(0 round 80px)", &units, [100., 60.], &doc, ids),
            Some(Clip::Rounded(_, [80., 80., 80., 80.]))
        ));
        let Some(Clip::Rounded(rect, radii)) = clip("circle(40%)", &units, [100., 60.], &doc, ids)
        else {
            panic!("circle");
        };
        assert!((rect[2] - rect[3]).abs() < 1e-3 && (radii[0] * 2. - rect[2]).abs() < 1e-3);
        assert!(matches!(
            clip("ellipse(40% 20%)", &units, [100., 60.], &doc, ids),
            Some(Clip::Shape(_))
        ));
        assert!(matches!(
            clip("path('M0 0 L10 0 L5 5 Z')", &units, [100., 60.], &doc, ids),
            Some(Clip::Shape(_))
        ));
        let Some(Clip::Shape(shapes)) = clip("url(#c)", &units, [100., 60.], &doc, ids) else {
            panic!("url");
        };
        assert!(shapes.contains("<rect"));
    }

    #[test]
    fn transforms_masks_blends_and_containment_establish_contexts_and_blocks() {
        let of = |pairs: &[(&str, &str)]| establishes(&style(pairs));
        for prop in ["translate", "rotate", "scale", "transform"] {
            let value = if prop == "rotate" {
                "5deg"
            } else if prop == "transform" {
                "scale(2)"
            } else {
                "2px"
            };
            let made = of(&[(prop, value)]);
            assert!(
                made.stacking && made.absolute && made.fixed && made.transformed,
                "{prop}"
            );
            assert!(!of(&[(prop, "none")]).stacking, "{prop}");
        }
        for (prop, value) in [
            ("mask-image", "linear-gradient(red,blue)"),
            ("-webkit-mask-image", "url(a.png)"),
            ("mask", "url(#m)"),
            ("mix-blend-mode", "multiply"),
            ("isolation", "isolate"),
            ("clip-path", "circle()"),
            ("opacity", "0.5"),
            ("will-change", "opacity"),
            ("position", "sticky"),
        ] {
            let made = of(&[(prop, value)]);
            assert!(
                made.stacking && made.absolute == (prop == "position") && !made.fixed,
                "{prop}"
            );
        }
        for (prop, value) in [
            ("filter", "blur(2px)"),
            ("backdrop-filter", "blur(2px)"),
            ("perspective", "100px"),
            ("contain", "paint"),
            ("content-visibility", "auto"),
            ("will-change", "opacity, Transform"),
            ("transform-style", "preserve-3d"),
        ] {
            let made = of(&[(prop, value)]);
            assert!(made.stacking && made.absolute && made.fixed, "{prop}");
        }
        let relative = of(&[("position", "relative")]);
        assert!(relative.absolute && !relative.fixed && !relative.stacking);
        assert!(of(&[("position", "relative"), ("z-index", "1")]).stacking);
        assert!(!of(&[("position", "relative"), ("z-index", "auto")]).stacking);
        assert!(of(&[("will-change", "position")]).absolute);
        assert_eq!(
            of(&[("contain", "size"), ("container-type", "inline-size")]),
            Establishes::default()
        );
        assert_eq!(
            of(&[("display", "contents"), ("transform", "scale(2)")]),
            Establishes::default()
        );
        assert!(of(&[("-webkit-filter", "blur(1px)")]).layer && !of(&[("mask", "url(#m)")]).layer);
    }
}
