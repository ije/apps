use crate::style::Prop;
use crate::{
    dom::Document,
    style::{self, Lengths, Style, components, parser},
};
use renderer::{BlendMode, Color, Corners, Length, PaintEffect, Pixels, Radii, Rect, point, px};

pub(crate) type Matrix = [f32; 16];

// Find IDs through the tree walk. The arena includes removed nodes and can greatly exceed the live page.
#[derive(Default)]
pub(crate) struct Ids {
    map: std::cell::OnceCell<std::collections::HashMap<String, Vec<usize>>>,
    walk: Option<std::rc::Rc<Vec<(u32, u32)>>>,
}

impl Ids {
    pub(crate) fn new() -> Self {
        Self::default()
    }

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
            (value, parser::Kind::Number) => Some(value as f32),
            (percent, parser::Kind::Length)
                if units.math(value, 0., &[]) == Some((0., parser::Kind::Length)) =>
            {
                Some(percent as f32 / 100.)
            }
            _ => None,
        }
    };
    let angle = |value: &str| {
        units
            .math(value, 0., &[])
            .filter(|(value, kind)| *kind == parser::Kind::Angle || *value == 0.)
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

pub(crate) fn translation(matrix: &Matrix) -> Option<[f32; 2]> {
    let plain = translate(matrix[12], matrix[13], 0.);
    (*matrix == plain).then_some([matrix[12], matrix[13]])
}

pub(crate) fn map(matrix: &Matrix, [x, y]: [f32; 2]) -> Option<[f32; 2]> {
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

fn color_matrix(filter: Option<style::Filter>, opacity: f32) -> Option<[f32; 20]> {
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
            "url" => effects.extend(svg_filter(args, units, doc, ids)),
            _ => {
                let (filter, opacity, _) = style::Filter::parse(part, units);
                if let Some(matrix) = color_matrix(filter, opacity) {
                    push_matrix(&mut effects, matrix);
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
                    .and_then(|value| style::color(value))
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
                    let (filter, opacity, _) = style::Filter::parse(&css, units);
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

pub(crate) fn backdrop(value: &str, units: &Lengths) -> Option<(Pixels, Option<[f32; 20]>)> {
    let mut blur = px(0.);
    let mut matrix: Option<[f32; 20]> = None;
    for effect in filters(value, units, &Document::default(), &Ids::new()) {
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
            let (rect, radii) = style::clip_path(value, units, size)?;
            Some(Clip::Rounded(rect, radii))
        }
        "circle" | "ellipse" => {
            let (rect, radii) = style::clip_path(value, units, size)?;
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
    for (prop, x, y) in [
        (
            Prop::BORDER_TOP_LEFT_RADIUS,
            &mut radii.x.top_left,
            &mut radii.y.top_left,
        ),
        (
            Prop::BORDER_TOP_RIGHT_RADIUS,
            &mut radii.x.top_right,
            &mut radii.y.top_right,
        ),
        (
            Prop::BORDER_BOTTOM_RIGHT_RADIUS,
            &mut radii.x.bottom_right,
            &mut radii.y.bottom_right,
        ),
        (
            Prop::BORDER_BOTTOM_LEFT_RADIUS,
            &mut radii.x.bottom_left,
            &mut radii.y.bottom_left,
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

fn angle(value: &str) -> Option<f32> {
    let number = |unit: &str| value.strip_suffix(unit).and_then(|v| v.parse::<f32>().ok());
    number("deg")
        .or_else(|| number("grad").map(|v| v * 0.9))
        .or_else(|| number("rad").map(f32::to_degrees))
        .or_else(|| number("turn").map(|v| v * 360.))
        .or((value == "0").then_some(0.))
        .filter(|v| v.is_finite())
}

fn first_argument(body: &str) -> (String, Option<String>) {
    let parts = style::components(body, ',');
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

pub(crate) fn gradient_axis(kind: &str, body: &str) -> Option<usize> {
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
        None => angle(&first).unwrap_or(180.),
    };
    match degrees.rem_euclid(180.) {
        0. => Some(1),
        90. => Some(0),
        _ => None,
    }
}

pub(crate) fn tile(
    size: &str,
    position: &str,
    units: &Lengths,
    width: f32,
    height: f32,
) -> [[f32; 2]; 2] {
    let dims = style::components(size, ' ');
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
    let tokens = style::components(position, ' ');
    let (x, y) = match tokens.as_slice() {
        [only] if matches!(*only, "top" | "bottom") => ("center", *only),
        [only] => (*only, "center"),
        [a, b, ..] if matches!(*a, "top" | "bottom") || matches!(*b, "left" | "right") => (*b, *a),
        [a, b, ..] => (*a, *b),
        [] => ("0%", "0%"),
    };
    let at = [(x, width - tw), (y, height - th)].map(|(token, free)| match token {
        "left" | "top" => 0.,
        "center" => free / 2.,
        "right" | "bottom" => free,
        value => units.resolve(value, free).unwrap_or(0.),
    });
    [[tw, th], at]
}

#[allow(clippy::too_many_arguments)]
fn gradient_stops<S: AsRef<str>>(
    parts: &[S],
    prelude: bool,
    units: &Lengths,
    extent: f32,
) -> Option<Vec<(f32, (u32, u8))>> {
    let mut stops: Vec<(Option<f32>, (u32, u8))> = Vec::new();
    for stop in parts.iter().map(AsRef::as_ref).skip(usize::from(prelude)) {
        let tokens = style::components(stop.trim(), ' ');
        let Some(ink) = tokens.iter().find_map(|token| style::color(token)) else {
            continue;
        };
        let offsets: Vec<f32> = tokens
            .iter()
            .filter(|token| style::color(token).is_none())
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
    Some(
        stops
            .into_iter()
            .map(|(offset, ink)| (offset.unwrap_or(0.), ink))
            .collect(),
    )
}

pub(super) fn axis_mask(
    style: &style::Style,
    units: &Lengths,
    [width, height]: [f32; 2],
    raster: [u32; 2],
) -> Option<[Vec<f32>; 2]> {
    let get =
        |[prop, webkit]: [Prop; 2]| style.get(prop).or_else(|| style.get(webkit)).unwrap_or("");
    let plain = |value: &str, defaults: &[&str]| {
        style::components(value, ',').iter().all(|part| {
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
    let images = style::components(get([Prop::MASK_IMAGE, Prop::WEBKIT_MASK_IMAGE]), ',');
    if images.len() > 1
        && !style::components(style.get(Prop::MASK_COMPOSITE)?, ',')
            .iter()
            .all(|part| part.trim() == "intersect")
    {
        return None;
    }
    let mut axes = [
        vec![1f32; raster[0] as usize],
        vec![1f32; raster[1] as usize],
    ];
    for image in &images {
        let body = image
            .trim()
            .strip_prefix("linear-gradient(")?
            .strip_suffix(')')?;
        let parts = style::components(body, ',');
        let (first, space) = first_argument(body);
        if space.is_some() {
            return None;
        }
        let (axis, reversed, prelude) = match first.as_str() {
            "to bottom" | "180deg" => (1, false, true),
            "to top" | "0deg" => (1, true, true),
            "to right" | "90deg" => (0, false, true),
            "to left" | "270deg" => (0, true, true),
            other if style::color(other).is_some() || other.is_empty() => (1, false, false),
            _ => return None,
        };
        let extent = [width, height][axis];
        let stops = gradient_stops(&parts, prelude, units, extent)?;
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
    }
    Some(axes)
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
) -> Option<String> {
    let [[tw, th], [x, y]] = tile(size, position, &units, width, height);
    let parts = style::components(body, ',');
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
            } else if let Some(degrees) = angle(first) {
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
                    && style::color(first).is_none()
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
            let at = style::components(at, ' ');
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
            prelude = space.is_some() || first.starts_with("from ") || first.starts_with("at ");
            if prelude {
                let (start, at) = first
                    .split_once("at ")
                    .map_or((first, ""), |(start, at)| (start, at.trim()));
                from = start
                    .trim()
                    .strip_prefix("from")
                    .and_then(|v| angle(v.trim()))
                    .unwrap_or(0.);
                let at = style::components(at, ' ');
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
    let mut offsets = gradient_stops(&parts, prelude, &units, extent)?;
    let space = space
        .filter(|space| !space.starts_with("srgb"))
        .or_else(|| {
            let modern = ["oklch(", "oklab(", "lab(", "lch(", "color("];
            modern
                .iter()
                .any(|name| body.contains(name))
                .then(|| "oklab".to_owned())
        });
    if let Some(space) = space {
        let mut mixed = Vec::with_capacity(offsets.len() * 8);
        for pair in offsets.windows(2) {
            let [(a, (ink_a, alpha_a)), (b, (ink_b, alpha_b))] = [pair[0], pair[1]];
            mixed.push(pair[0]);
            if (ink_a, alpha_a) == (ink_b, alpha_b) || a == b {
                continue;
            }
            for step in 1..8 {
                let t = step as f32 / 8.;
                let mix = format!(
                    "color-mix(in {space}, #{ink_a:06x}{alpha_a:02x} {}%, #{ink_b:06x}{alpha_b:02x})",
                    (1. - t) * 100.
                );
                if let Some(ink) = style::color(&mix) {
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

pub(super) fn opacity(style: &style::Style) -> f32 {
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
            .map(|(key, value)| (crate::style::Prop::new(key), (*value).into()))
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

    #[test]
    fn gradient_masks_that_intersect_multiply_along_their_axes() {
        let units = Lengths::new(16., 800., 600.);
        let mask = |image: &str, composite: Option<&str>, raster: [u32; 2]| {
            let mut props = vec![("mask-image", image)];
            props.extend(composite.map(|value| ("mask-composite", value)));
            axis_mask(&style(&props), &units, [100., 50.], raster)
        };
        let opaque = "linear-gradient(to left, black calc(100% - min(12px, 0px)), transparent 100%), linear-gradient(#fff, #fff)";
        let [x, y] = mask(opaque, Some("intersect"), [100, 50]).unwrap();
        assert!(x.iter().chain(&y).all(|alpha| *alpha >= 0.999));
        let fades = "linear-gradient(to right, transparent, black 10px), linear-gradient(to top, transparent, black 10px)";
        let [x, y] = mask(fades, Some("intersect"), [100, 50]).unwrap();
        assert!(x[0] < 0.1 && x[50] >= 0.999);
        assert!(y[0] >= 0.999 && y[49] < 0.1);
        assert!(mask(fades, None, [100, 50]).is_none());
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
    fn filters_split_into_layers() {
        let units = Lengths::new(16., 800., 600.);
        let doc = Document::default();
        let effects = filters(
            "grayscale(1) contrast(2) blur(4px) drop-shadow(1px 2px 6px red) opacity(.5)",
            &units,
            &doc,
            &Ids::new(),
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
            ("linear-gradient", "45deg, red, blue", None),
            ("linear-gradient", "to bottom right, red, blue", None),
            ("radial-gradient", "red, blue", None),
        ] {
            assert_eq!(gradient_axis(kind, body), axis, "{kind}({body})");
        }
    }

    #[test]
    fn gradient_interpolation_spaces_parse_in_any_order() {
        let units = Lengths::new(16., 800., 600.);
        for (body, valid) in [
            ("hue in hsl, red, blue", false),
            ("in hsl longer hue, red, blue", true),
            ("to right in oklch longer hue, red, blue", true),
        ] {
            let svg = gradient_svg("linear-gradient", body, "auto", "", "", units, 10., 10.);
            assert_eq!(svg.is_some(), valid, "{body}");
        }
        for kind in ["radial-gradient", "conic-gradient"] {
            for body in ["hue in hsl, red, blue", "in, red, blue"] {
                gradient_svg(kind, body, "auto", "", "", units, 10., 10.);
            }
        }
    }

    #[test]
    fn clip_paths_become_shapes() {
        let units = Lengths::new(16., 800., 600.);
        let doc = Document::parse(
            "<svg><clipPath id=c><rect width=5 height=5></rect></clipPath></svg><p id=c></p>",
        );
        let ids = &Ids::new();
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
