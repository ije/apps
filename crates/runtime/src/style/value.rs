use super::{Lengths, Prop, Style, color, parser};

/// The shorthand and its top, right, bottom and left longhands.
pub(crate) const PADDING: [Prop; 5] = [
    Prop::PADDING,
    Prop::PADDING_TOP,
    Prop::PADDING_RIGHT,
    Prop::PADDING_BOTTOM,
    Prop::PADDING_LEFT,
];
pub(crate) const MARGIN: [Prop; 5] = [
    Prop::MARGIN,
    Prop::MARGIN_TOP,
    Prop::MARGIN_RIGHT,
    Prop::MARGIN_BOTTOM,
    Prop::MARGIN_LEFT,
];

/// The width, style and color of the top, right, bottom and left borders.
pub(crate) const BORDERS: [[Prop; 3]; 4] = [
    [
        Prop::BORDER_TOP_WIDTH,
        Prop::BORDER_TOP_STYLE,
        Prop::BORDER_TOP_COLOR,
    ],
    [
        Prop::BORDER_RIGHT_WIDTH,
        Prop::BORDER_RIGHT_STYLE,
        Prop::BORDER_RIGHT_COLOR,
    ],
    [
        Prop::BORDER_BOTTOM_WIDTH,
        Prop::BORDER_BOTTOM_STYLE,
        Prop::BORDER_BOTTOM_COLOR,
    ],
    [
        Prop::BORDER_LEFT_WIDTH,
        Prop::BORDER_LEFT_STYLE,
        Prop::BORDER_LEFT_COLOR,
    ],
];

impl Lengths {
    pub fn edges(&self, style: &Style, [shorthand, sides @ ..]: [Prop; 5], base: f32) -> [f32; 4] {
        let mut result = [0.; 4];
        if let Some(value) = style.get(shorthand) {
            let values: Vec<_> = components(value, ' ')
                .into_iter()
                .map(|v| self.resolve(v, base).unwrap_or(0.))
                .collect();
            let first = values.first().copied().unwrap_or(0.);
            result = [
                first,
                values.get(1).copied().unwrap_or(first),
                values.get(2).copied().unwrap_or(first),
                values.get(3).or(values.get(1)).copied().unwrap_or(first),
            ];
        }
        for (edge, side) in result.iter_mut().zip(sides) {
            if let Some(value) = style.get(side) {
                *edge = self.resolve(value, base).unwrap_or(0.);
            }
        }
        result
    }
}

pub fn length(value: &str, base: f32, font: f32, vw: f32, vh: f32) -> f32 {
    Lengths::new(font, vw, vh)
        .parse(value, base)
        .map(|(value, _)| value)
        .unwrap_or(0.)
}

pub(crate) fn components(value: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut input = cssparser::Parser::new(value);
    let mut start = input.position();
    let mut block = false;
    loop {
        let position = input.position();
        let Ok(token) = input.next_including_whitespace().cloned() else {
            break;
        };
        // A function or block ends a value, so `rotate(1deg)scale(2)` is two values.
        if separator == ' '
            && std::mem::take(&mut block)
            && !matches!(
                token,
                cssparser::Token::WhiteSpace(_)
                    | cssparser::Token::Comma
                    | cssparser::Token::Delim(_)
            )
        {
            let part = input.slice(start..position).trim();
            if !part.is_empty() {
                parts.push(part);
            }
            start = position;
        }
        if matches!(token, cssparser::Token::Comma) && separator == ','
            || matches!(token, cssparser::Token::WhiteSpace(_)) && separator == ' '
        {
            let part = input.slice(start..position).trim();
            if !part.is_empty() {
                parts.push(part);
            }
            start = input.position();
        } else if matches!(
            token,
            cssparser::Token::Function(_)
                | cssparser::Token::ParenthesisBlock
                | cssparser::Token::SquareBracketBlock
                | cssparser::Token::CurlyBracketBlock
        ) {
            if input
                .parse_nested_block(|input| {
                    input
                        .expect_no_error_token()
                        .map_err(cssparser::ParseError::<()>::from)
                })
                .is_err()
            {
                return Vec::new();
            }
            block = true;
        }
    }
    let part = input.slice_from(start).trim();
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

impl Lengths {
    pub fn shadows(&self, value: &str, inset: bool) -> Vec<render::BoxShadow> {
        let mut shadows = Vec::new();
        if value == "none" {
            return shadows;
        }
        for shadow in components(value, ',').into_iter().rev() {
            let mut lengths = Vec::new();
            let mut ink = (0, 255);
            let parts = components(shadow, ' ');
            if parts.contains(&"inset") != inset {
                continue;
            }
            for part in parts.into_iter().filter(|part| *part != "inset") {
                if let Some(color) = color(part) {
                    ink = color;
                } else if let Some(value) = self.resolve(part, self.viewport[0]) {
                    lengths.push(value);
                } else {
                    return Vec::new();
                }
            }
            if !(2..=4).contains(&lengths.len()) {
                return Vec::new();
            }
            shadows.push(render::BoxShadow {
                color: render::rgba((ink.0 << 8) | u32::from(ink.1)),
                offset: render::point(render::px(lengths[0]), render::px(lengths[1])),
                // WARNING: the renderer uses the blur radius as the Gaussian sigma, which is half the CSS blur radius.
                blur_radius: render::px(lengths.get(2).copied().unwrap_or(0.).max(0.) / 2.),
                spread_radius: render::px(lengths.get(3).copied().unwrap_or(0.)),
                inset,
            });
        }
        shadows
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Filter(pub [[f32; 4]; 3]);

impl Filter {
    pub fn parse(value: &str, units: &Lengths) -> (Option<Self>, f32, Vec<render::BoxShadow>) {
        let identity = Self([[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]]);
        let (mut matrix, mut opacity, mut shadows) =
            (identity, 1f32, Vec::<render::BoxShadow>::new());
        for part in components(value, ' ') {
            let Some((name, args)) = part.strip_suffix(')').and_then(|part| part.split_once('('))
            else {
                return (None, 1., Vec::new());
            };
            let args = args.trim();
            let amount = if args.is_empty() {
                Some(1.)
            } else if let Some(percent) = args.strip_suffix('%') {
                percent.trim().parse::<f32>().ok().map(|value| value / 100.)
            } else {
                units
                    .math(args, 0., &[])
                    .filter(|(_, kind)| *kind == parser::Kind::Number)
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
            let step = match (name.to_ascii_lowercase().as_str(), amount) {
                ("grayscale", Some(amount)) => {
                    blend([[0.2126, 0.7152, 0.0722]; 3], 1. - amount.min(1.))
                }
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
                ("opacity", Some(amount)) => {
                    opacity *= amount.min(1.);
                    continue;
                }
                ("hue-rotate", _) => {
                    let Some((angle, _)) = (if args.is_empty() {
                        Some((0., parser::Kind::Angle))
                    } else {
                        units.math(args, 0., &[]).filter(|(value, kind)| {
                            *kind == parser::Kind::Angle
                                || *kind == parser::Kind::Number && *value == 0.
                        })
                    }) else {
                        return (None, 1., Vec::new());
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
                ("drop-shadow", _) => {
                    let found = units.shadows(args, false);
                    if found.is_empty() {
                        return (None, 1., Vec::new());
                    }
                    shadows.extend(found);
                    continue;
                }
                ("blur" | "url", _) => continue,
                _ => return (None, 1., Vec::new()),
            };
            for shadow in &mut shadows {
                shadow.color = step.hsla(shadow.color);
            }
            matrix = matrix.then(step);
        }
        ((matrix != identity).then_some(matrix), opacity, shadows)
    }

    pub fn then(self, outer: Self) -> Self {
        Self(std::array::from_fn(|i| {
            let mut row = [0., 0., 0., outer.0[i][3]];
            for (j, value) in row.iter_mut().enumerate() {
                *value += (0..3).map(|k| outer.0[i][k] * self.0[k][j]).sum::<f32>();
            }
            row
        }))
    }

    fn rgb(&self, rgb: [f32; 3]) -> [f32; 3] {
        self.0
            .map(|row| (row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2] + row[3]).clamp(0., 1.))
    }

    #[cfg(test)]
    pub fn color(&self, (ink, alpha): (u32, u8)) -> (u32, u8) {
        let [r, g, b] = self
            .rgb([16, 8, 0].map(|shift| ((ink >> shift) & 255) as f32 / 255.))
            .map(|value| (value * 255.).round() as u32);
        (r << 16 | g << 8 | b, alpha)
    }

    pub fn hsla(&self, color: render::Rgba) -> render::Rgba {
        let [r, g, b] = self.rgb([color.r, color.g, color.b]);
        render::Rgba {
            r,
            g,
            b,
            a: color.a,
        }
    }
}

pub(crate) fn clip_path(
    value: &str,
    units: &Lengths,
    size: [f32; 2],
) -> Option<([f32; 4], [f32; 4])> {
    let [width, height] = size;
    let (name, args) = components(value, ' ')
        .into_iter()
        .find_map(|part| part.strip_suffix(')')?.split_once('('))?;
    let tokens = components(args, ' ');
    let split = tokens
        .iter()
        .position(|token| matches!(*token, "round" | "at"))
        .unwrap_or(tokens.len());
    let (head, tail) = (
        &tokens[..split],
        tokens.get(split + 1..).unwrap_or_default(),
    );
    let edges = |values: &[&str]| -> Option<[f32; 4]> {
        let values = match values {
            [a] => [*a; 4],
            [a, b] => [*a, *b, *a, *b],
            [a, b, c] => [*a, *b, *c, *b],
            [a, b, c, d] => [*a, *b, *c, *d],
            _ => return None,
        };
        let mut result = [0.; 4];
        for (side, value) in values.into_iter().enumerate() {
            result[side] = units.resolve(value, if side % 2 == 0 { height } else { width })?;
        }
        Some(result)
    };
    let radii = || {
        let corners = tail.split(|token| *token == "/").next().unwrap_or_default();
        edges(corners)
            .map(|[a, b, c, d]| [a, b, c, d].map(|value| value.max(0.)))
            .unwrap_or_default()
    };
    let rect = |x: f32, y: f32, w: f32, h: f32| [x, y, w.max(0.), h.max(0.)];
    match name.to_ascii_lowercase().as_str() {
        "inset" => {
            let [top, right, bottom, left] = edges(head)?;
            Some((
                rect(left, top, width - left - right, height - top - bottom),
                radii(),
            ))
        }
        "rect" => {
            let [top, right, bottom, left] = <[&str; 4]>::try_from(head).ok()?;
            let edge = |token: &str, base: f32, auto: f32| {
                if token == "auto" {
                    Some(auto)
                } else {
                    units.resolve(token, base)
                }
            };
            let [top, bottom] = [edge(top, height, 0.)?, edge(bottom, height, height)?];
            let [left, right] = [edge(left, width, 0.)?, edge(right, width, width)?];
            Some((rect(left, top, right - left, bottom - top), radii()))
        }
        "xywh" => {
            let [x, y, w, h] = <[&str; 4]>::try_from(head).ok()?;
            let [x, w] = [x, w].map(|value| units.resolve(value, width));
            let [y, h] = [y, h].map(|value| units.resolve(value, height));
            Some((rect(x?, y?, w?, h?), radii()))
        }
        "circle" | "ellipse" => {
            let (x, y) = match tail {
                [only] if matches!(*only, "top" | "bottom") => ("center", *only),
                [only] => (*only, "center"),
                [a, b, ..] if matches!(*a, "top" | "bottom") || matches!(*b, "left" | "right") => {
                    (*b, *a)
                }
                [a, b, ..] => (*a, *b),
                [] => ("center", "center"),
            };
            let [x, y] = [(x, width), (y, height)].map(|(token, extent)| match token {
                "left" | "top" => Some(0.),
                "center" => Some(extent / 2.),
                "right" | "bottom" => Some(extent),
                value => units.resolve(value, extent),
            });
            let (x, y) = (x?, y?);
            let sides = [x.abs(), (width - x).abs(), y.abs(), (height - y).abs()];
            let radius = |token: Option<&&str>, near: f32, far: f32, base: f32| match token {
                None | Some(&"closest-side") => Some(near),
                Some(&"farthest-side") => Some(far),
                Some(value) => units.resolve(value, base).map(|value| value.max(0.)),
            };
            let (rx, ry) = if name.eq_ignore_ascii_case("circle") {
                let r = radius(
                    head.first(),
                    sides.into_iter().fold(f32::MAX, f32::min),
                    sides.into_iter().fold(0., f32::max),
                    width.hypot(height) / std::f32::consts::SQRT_2,
                )?;
                (r, r)
            } else {
                (
                    radius(
                        head.first(),
                        sides[0].min(sides[1]),
                        sides[0].max(sides[1]),
                        width,
                    )?,
                    radius(
                        head.get(1),
                        sides[2].min(sides[3]),
                        sides[2].max(sides[3]),
                        height,
                    )?,
                )
            };
            Some((rect(x - rx, y - ry, 2. * rx, 2. * ry), [rx.min(ry); 4]))
        }
        _ => None,
    }
}
