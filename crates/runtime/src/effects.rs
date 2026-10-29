use crate::{
    html::Document,
    render::css,
    render::{self, Lengths, Style, components},
};
use gpui::{BlendMode, Bounds, Corners, Hsla, PaintEffect, Pixels, point, px};

/// A 4x4 column-major matrix.
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

fn translate(x: f32, y: f32, z: f32) -> Matrix {
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
    let (s, c) = angle.sin_cos();
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

/// Resolves a CSS position like `transform-origin` against a size.
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

/// The CSS transform of a box with the given window bounds, as a matrix from untransformed to
/// transformed window coordinates in logical pixels. It includes `transform-origin` and the
/// parent's `perspective` (distance and window origin).
pub(crate) fn transform(
    style: &Style,
    units: &Lengths,
    bounds: Bounds<Pixels>,
    parent_perspective: Option<(f32, [f32; 2])>,
) -> Option<Matrix> {
    let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
    let number = |value: &str| {
        if let Some(percent) = value.strip_suffix('%') {
            return percent.parse::<f32>().ok().map(|value| value / 100.);
        }
        units
            .math(value, 0., &[])
            .filter(|(_, kind)| *kind == css::Kind::Number)
            .map(|(value, _)| value as f32)
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
    if let Some(value) = style.get("translate").filter(|value| *value != "none") {
        let parts = components(value, ' ');
        then(translate(
            length(parts.first()?, size[0])?,
            parts.get(1).map_or(Some(0.), |y| length(y, size[1]))?,
            parts.get(2).map_or(Some(0.), |z| length(z, 0.))?,
        ));
    }
    if let Some(value) = style.get("rotate").filter(|value| *value != "none") {
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
    if let Some(value) = style.get("scale").filter(|value| *value != "none") {
        let parts = components(value, ' ');
        let x = number(parts.first()?)?;
        then(scale(
            x,
            parts.get(1).map_or(Some(x), |y| number(y))?,
            parts.get(2).map_or(Some(1.), |z| number(z))?,
        ));
    }
    for part in components(style.get("transform").map_or("", String::as_str), ' ') {
        let Some((name, args)) = part.strip_suffix(')').and_then(|part| part.split_once('('))
        else {
            continue;
        };
        let args = components(args, ',');
        let next = match (name.to_ascii_lowercase().as_str(), args.as_slice()) {
            ("translate", [x]) => translate(length(x, size[0])?, 0., 0.),
            ("translate", [x, y]) => translate(length(x, size[0])?, length(y, size[1])?, 0.),
            ("translatex", [x]) => translate(length(x, size[0])?, 0., 0.),
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
        style
            .get("transform-origin")
            .map_or("50% 50%", String::as_str),
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

/// The translation of a matrix that only translates in 2D.
pub(crate) fn translation(matrix: &Matrix) -> Option<[f32; 2]> {
    let plain = translate(matrix[12], matrix[13], 0.);
    (*matrix == plain).then_some([matrix[12], matrix[13]])
}

/// Maps a point through a matrix, with the perspective divide.
pub(crate) fn map(matrix: &Matrix, [x, y]: [f32; 2]) -> Option<[f32; 2]> {
    let w = matrix[3] * x + matrix[7] * y + matrix[15];
    (w > 1e-4).then(|| {
        [
            (matrix[0] * x + matrix[4] * y + matrix[12]) / w,
            (matrix[1] * x + matrix[5] * y + matrix[13]) / w,
        ]
    })
}

/// Whether the back of a transformed box faces the viewer.
pub(crate) fn backface(matrix: &Matrix, bounds: Bounds<Pixels>) -> bool {
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

/// A color matrix and opacity as a 4x5 row-major matrix.
fn color_matrix(filter: Option<render::Filter>, opacity: f32) -> Option<[f32; 20]> {
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

/// The effects of a CSS `filter`, innermost first. Each blur and drop shadow gets its own layer,
/// and consecutive color functions share one.
pub(crate) fn filters(value: &str, units: &Lengths, doc: &Document) -> Vec<PaintEffect> {
    let mut effects: Vec<PaintEffect> = Vec::new();
    let push_matrix = |effects: &mut Vec<PaintEffect>, matrix: [f32; 20]| {
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
    };
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
                for shadow in units.shadows(args, false) {
                    effects.push(PaintEffect {
                        shadow: Some((shadow.offset, shadow.blur_radius, shadow.color)),
                        ..Default::default()
                    });
                }
            }
            "url" => effects.extend(svg_filter(args, units, doc)),
            _ => {
                let (filter, opacity, _) = render::Filter::parse(part, units);
                if let Some(matrix) = color_matrix(filter, opacity) {
                    push_matrix(&mut effects, matrix);
                }
            }
        }
    }
    effects
}

/// Maps the primitives of an inline SVG `<filter>` to effects.
fn svg_filter(reference: &str, units: &Lengths, doc: &Document) -> Vec<PaintEffect> {
    let id = reference.trim().trim_matches(['"', '\'']);
    let Some(id) = id.strip_prefix('#') else {
        return Vec::new();
    };
    let Some(filter) = doc.nodes.iter().position(|node| {
        node.tag == "filter" && node.attrs.get("id").is_some_and(|value| value == id)
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
                    .and_then(|value| render::color(value))
                    .unwrap_or((0, 255));
                let opacity = numbers("flood-opacity").first().copied().unwrap_or(1.);
                let mut ink: Hsla = gpui::rgba(color.0 << 8 | color.1 as u32).into();
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
                    let (filter, opacity, _) = render::Filter::parse(&css, units);
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
        // `feOffset` after a blur of the alpha is the usual drop shadow recipe.
        let sigma = effects.remove(blur).blur;
        effects.push(PaintEffect {
            shadow: Some((offset, sigma, gpui::black().opacity(0.5))),
            ..Default::default()
        });
    }
    effects
}

/// The blur and color matrix of a CSS `backdrop-filter`.
pub(crate) fn backdrop(value: &str, units: &Lengths) -> Option<(Pixels, Option<[f32; 20]>)> {
    let mut blur = px(0.);
    let mut matrix: Option<[f32; 20]> = None;
    for effect in filters(value, units, &Document::default()) {
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

/// How a `clip-path` clips a box of the given size.
pub(crate) enum Clip {
    /// A rounded rectangle, relative to the box.
    Rounded([f32; 4], [f32; 4]),
    /// SVG shapes, in the box's coordinates.
    Shape(String),
}

pub(crate) fn clip(value: &str, units: &Lengths, size: [f32; 2], doc: &Document) -> Option<Clip> {
    let [width, height] = size;
    let (name, args) = components(value, ' ')
        .into_iter()
        .find_map(|part| part.strip_suffix(')')?.split_once('('))?;
    let svg = Clip::Shape;
    match name.to_ascii_lowercase().as_str() {
        "inset" | "rect" | "xywh" => {
            let (rect, radii) = render::clip_path(value, units, size)?;
            Some(Clip::Rounded(rect, radii))
        }
        "circle" | "ellipse" => {
            let (rect, _) = render::clip_path(value, units, size)?;
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
                crate::html::escape(data)
            )))
        }
        "url" => {
            let id = args.trim().trim_matches(['"', '\'']).strip_prefix('#')?;
            let clip = doc.nodes.iter().position(|node| {
                node.tag.eq_ignore_ascii_case("clipPath")
                    && node.attrs.get("id").is_some_and(|value| value == id)
            })?;
            let object = doc.nodes[clip]
                .attrs
                .get("clipPathUnits")
                .is_some_and(|value| value == "objectBoundingBox");
            let shapes: String = doc.nodes[clip]
                .children
                .iter()
                .map(|&child| doc.serialize(child))
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

/// The corner radii of a box as gpui corners.
pub(crate) fn corners(radii: [f32; 4]) -> Corners<Pixels> {
    Corners {
        top_left: px(radii[0]),
        top_right: px(radii[1]),
        bottom_right: px(radii[2]),
        bottom_left: px(radii[3]),
    }
}

/// The SVG of a CSS gradient layer covering a `width` by `height` box, sized, placed and
/// repeated like a background or mask layer.
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
) -> Option<String> {
    let dims = render::components(size, ' ');
    let dim = |axis: usize| {
        let extent = [width, height][axis];
        match dims.get(axis) {
            None | Some(&"auto") => extent,
            Some(value) => units.resolve(value, extent).unwrap_or(extent),
        }
    };
    let [tw, th] = if matches!(dims.as_slice(), ["cover"] | ["contain"]) {
        [width, height]
    } else {
        [dim(0).max(0.1), dim(1).max(0.1)]
    };
    let tokens = render::components(position, ' ');
    let (x, y) = match tokens.as_slice() {
        [only] if matches!(*only, "top" | "bottom") => ("center", *only),
        [only] => (*only, "center"),
        [a, b, ..] if matches!(*a, "top" | "bottom") || matches!(*b, "left" | "right") => (*b, *a),
        [a, b, ..] => (*a, *b),
        [] => ("0%", "0%"),
    };
    let [x, y] = [(x, width - tw), (y, height - th)].map(|(token, free)| match token {
        "left" | "top" => 0.,
        "center" => free / 2.,
        "right" | "bottom" => free,
        value => units.resolve(value, free).unwrap_or(0.),
    });
    let angle = |value: &str| -> Option<f32> {
        let number = |unit: &str| value.strip_suffix(unit).and_then(|v| v.parse::<f32>().ok());
        number("deg")
            .or_else(|| number("grad").map(|v| v * 0.9))
            .or_else(|| number("rad").map(f32::to_degrees))
            .or_else(|| number("turn").map(|v| v * 360.))
            .or((value == "0").then_some(0.))
            .filter(|v| v.is_finite())
    };
    let parts = render::components(body, ',');
    let first = parts.first().map_or("", |part| part.trim());
    let repeating = kind.starts_with("repeating-");
    let (mut prelude, mut direction, mut center, mut radii, mut from) =
        (false, (0f32, 0f32), (tw / 2., th / 2.), (0f32, 0f32), 0f32);
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
            } else if let Some(degrees) = angle(first) {
                prelude = true;
                degrees
            } else {
                180.
            };
            direction = (degrees.to_radians().sin(), -degrees.to_radians().cos());
            tw * direction.0.abs() + th * direction.1.abs()
        }
        "radial-gradient" => {
            prelude = ["circle", "ellipse", "at ", "side", "corner"]
                .iter()
                .any(|word| first.contains(word))
                || first.starts_with("at")
                || !first.is_empty()
                    && render::color(first).is_none()
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
            let at = render::components(at, ' ');
            let (cx, cy) = match at.as_slice() {
                [only] if matches!(*only, "top" | "bottom") => ("center", *only),
                [only] => (*only, "center"),
                [a, b, ..] if matches!(*a, "top" | "bottom") || matches!(*b, "left" | "right") => {
                    (*b, *a)
                }
                [a, b, ..] => (*a, *b),
                [] => ("center", "center"),
            };
            let [cx, cy] = [(cx, tw), (cy, th)].map(|(token, extent)| match token {
                "left" | "top" => 0.,
                "center" => extent / 2.,
                "right" | "bottom" => extent,
                value => units.resolve(value, extent).unwrap_or(extent / 2.),
            });
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
            prelude = first.starts_with("from ") || first.starts_with("at ");
            if prelude {
                let (start, at) = first
                    .split_once("at ")
                    .map_or((first, ""), |(start, at)| (start, at.trim()));
                from = start
                    .trim()
                    .strip_prefix("from")
                    .and_then(|v| angle(v.trim()))
                    .unwrap_or(0.);
                let at = render::components(at, ' ');
                let token = |index: usize, extent: f32| match at.get(index).copied() {
                    Some("left" | "top") => 0.,
                    Some("right" | "bottom") => extent,
                    Some(value) if value != "center" => {
                        units.resolve(value, extent).unwrap_or(extent / 2.)
                    }
                    _ => extent / 2.,
                };
                center = (token(0, tw), token(1, th));
            }
            360.
        }
        _ => return None,
    };
    let mut stops: Vec<(Option<f32>, (u32, u8))> = Vec::new();
    for stop in parts.iter().skip(usize::from(prelude)) {
        let tokens = render::components(stop.trim(), ' ');
        let Some(ink) = tokens.iter().find_map(|token| render::color(token)) else {
            continue;
        };
        let offsets: Vec<f32> = tokens
            .iter()
            .filter(|token| render::color(token).is_none())
            .filter_map(|token| units.resolve(token, extent).or_else(|| angle(token)))
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
    let offsets: Vec<(f32, (u32, u8))> = stops
        .into_iter()
        .map(|(offset, ink)| (offset.unwrap_or(0.), ink))
        .collect();
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
    let spread = if repeating { "repeat" } else { "pad" };
    let svg_stops = offsets
        .iter()
        .map(|(offset, (ink, alpha))| {
            format!(
                "<stop offset=\"{}\" stop-color=\"#{ink:06x}\" stop-opacity=\"{}\"/>",
                ((offset - start) / period).clamp(0., 1.),
                *alpha as f32 / 255.
            )
        })
        .collect::<String>();
    let content = match kind.trim_start_matches("repeating-") {
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
            let mut paths = String::new();
            for step in 0..180 {
                let at = (step as f32 + 0.5) * 2.;
                let angle = if repeating {
                    start + (at - start).rem_euclid(period)
                } else {
                    at
                };
                let next = offsets
                    .iter()
                    .position(|(offset, _)| *offset >= angle)
                    .unwrap_or(last);
                let (a, b) = (offsets[next.saturating_sub(1)], offsets[next]);
                let t = if b.0 > a.0 {
                    ((angle - a.0) / (b.0 - a.0)).clamp(0., 1.)
                } else {
                    1.
                };
                let mix = |shift: u32| {
                    ((((a.1).0 >> shift) & 255) as f32 * (1. - t)
                        + (((b.1).0 >> shift) & 255) as f32 * t)
                        .round() as u32
                };
                let ink = mix(16) << 16 | mix(8) << 8 | mix(0);
                let alpha = ((a.1).1 as f32 * (1. - t) + (b.1).1 as f32 * t) / 255.;
                let [start, end] = [from + step as f32 * 2. - 0.3, from + step as f32 * 2. + 2.3]
                    .map(f32::to_radians);
                paths.push_str(&format!(
                    "<path d=\"M{cx} {cy}L{} {}L{} {}Z\" fill=\"#{ink:06x}\" fill-opacity=\"{alpha}\"/>",
                    cx + reach * start.sin(),
                    cy - reach * start.cos(),
                    cx + reach * end.sin(),
                    cy - reach * end.cos()
                ));
            }
            format!(
                "<defs><clipPath id=\"c\"><rect width=\"{tw}\" height=\"{th}\"/></clipPath></defs><g clip-path=\"url(#c)\">{paths}</g>"
            )
        }
    };
    let body = match repeat.split_whitespace().next().unwrap_or("repeat") {
        "no-repeat" => format!("<g transform=\"translate({x} {y})\">{content}</g>"),
        kind => {
            let rect = match kind {
                "repeat-x" => format!("x=\"0\" y=\"{y}\" width=\"100%\" height=\"{th}\""),
                "repeat-y" => format!("x=\"{x}\" y=\"0\" width=\"{tw}\" height=\"100%\""),
                _ => "width=\"100%\" height=\"100%\"".into(),
            };
            format!(
                "<defs><pattern id=\"p\" patternUnits=\"userSpaceOnUse\" width=\"{tw}\" height=\"{th}\" x=\"{x}\" y=\"{y}\">{content}</pattern></defs><rect {rect} fill=\"url(#p)\"/>"
            )
        }
    };
    Some(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">{body}</svg>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn style(pairs: &[(&str, &str)]) -> Style {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn transforms_map_window_points_around_the_origin() {
        let units = Lengths::new(16., 800., 600.);
        let bounds = Bounds::new(point(px(100.), px(50.)), size(px(10.), px(20.)));
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
        // The center stays put and the top-left corner turns around it.
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
    fn filters_split_into_layers() {
        let units = Lengths::new(16., 800., 600.);
        let doc = Document::default();
        let effects = filters(
            "grayscale(1) contrast(2) blur(4px) drop-shadow(1px 2px 6px red) opacity(.5)",
            &units,
            &doc,
        );
        assert_eq!(effects.len(), 4);
        assert!(effects[0].color_matrix.is_some());
        assert_eq!(effects[1].blur, px(4.));
        assert_eq!(
            effects[2].shadow.map(|(offset, blur, _)| (offset, blur)),
            Some((point(px(1.), px(2.)), px(3.)))
        );
        assert_eq!(effects[3].color_matrix.map(|matrix| matrix[18]), Some(0.5));
        let (blur, matrix) = backdrop("blur(10px) saturate(1.5)", &units).unwrap();
        assert_eq!(blur, px(10.));
        assert!(matrix.is_some());
        assert_eq!(blend_mode("multiply"), BlendMode::Multiply);
        assert_eq!(blend_mode("unknown"), BlendMode::Normal);
    }

    #[test]
    fn clip_paths_become_shapes() {
        let units = Lengths::new(16., 800., 600.);
        let doc = Document::default();
        assert!(matches!(
            clip("inset(10px round 4px)", &units, [100., 60.], &doc),
            Some(Clip::Rounded([10., 10., 80., 40.], [4., 4., 4., 4.]))
        ));
        let Some(Clip::Shape(polygon)) = clip(
            "polygon(50% 0, 100% 100%, 0 100%)",
            &units,
            [100., 60.],
            &doc,
        ) else {
            panic!("polygon");
        };
        assert!(polygon.contains("points=\"50,0 100,60 0,60 \""));
        assert!(matches!(
            clip("circle(40%)", &units, [100., 60.], &doc),
            Some(Clip::Shape(_))
        ));
        assert!(matches!(
            clip("path('M0 0 L10 0 L5 5 Z')", &units, [100., 60.], &doc),
            Some(Clip::Shape(_))
        ));
    }
}
