use super::{Lengths, Prop, Style, color, parser};

// A style holds longhands only: the cascade expands every shorthand.
pub(crate) const PADDING: [Prop; 4] = [
    Prop::PADDING_TOP,
    Prop::PADDING_RIGHT,
    Prop::PADDING_BOTTOM,
    Prop::PADDING_LEFT,
];
pub(crate) const MARGIN: [Prop; 4] = [
    Prop::MARGIN_TOP,
    Prop::MARGIN_RIGHT,
    Prop::MARGIN_BOTTOM,
    Prop::MARGIN_LEFT,
];

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

pub(crate) fn components(value: &str, separator: char) -> Vec<&str> {
    let bytes = value.as_bytes();
    let mut parts = Vec::new();
    let (mut open, mut depth, mut start, mut closed) = (0u64, 0u32, 0, false);
    let simple = 'scan: {
        for (index, &byte) in bytes.iter().enumerate() {
            // The tokenizer ends a value after a block, as in `rotate(1deg)scale(2)`, so this fast scan must
            // give up there.
            if std::mem::take(&mut closed)
                && separator == ' '
                && !byte.is_ascii_whitespace()
                && byte != b','
            {
                break 'scan false;
            }
            match byte {
                b'"' | b'\'' | b'\\' => break 'scan false,
                b'/' if bytes.get(index + 1) == Some(&b'*') => break 'scan false,
                b'(' | b'[' | b'{' => {
                    if depth == 32
                        || byte == b'('
                            && index >= 3
                            && bytes[index - 3..index].eq_ignore_ascii_case(b"url")
                    {
                        break 'scan false;
                    }
                    open = open << 2 | u64::from(byte / 32 - 1);
                    depth += 1;
                }
                b')' | b']' | b'}' => {
                    if depth == 0 || open & 3 != u64::from(byte / 32 - 1) {
                        break 'scan false;
                    }
                    open >>= 2;
                    depth -= 1;
                    closed = depth == 0;
                }
                byte if depth == 0 && separator != ' ' && byte == separator as u8 => {
                    parts.extend(Some(value[start..index].trim()).filter(|part| !part.is_empty()));
                    start = index + 1;
                }
                byte if depth == 0 && separator == ' ' && byte.is_ascii_whitespace() => {
                    parts.extend(Some(value[start..index].trim()).filter(|part| !part.is_empty()));
                    start = index + 1;
                }
                _ => {}
            }
        }
        depth == 0
    };
    if simple {
        parts.extend(Some(value[start..].trim()).filter(|part| !part.is_empty()));
        return parts;
    }
    tokenized(value, separator)
}

fn tokenized(value: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut input = cssparser::Parser::new(value);
    let mut start = input.position();
    let mut block = false;
    loop {
        let position = input.position();
        let Ok(token) = input.next_including_whitespace().cloned() else {
            break;
        };
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
            || matches!(token, cssparser::Token::Delim(delim) if delim == separator)
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

type ShadowMemo = rustc_hash::FxHashMap<u64, (Box<str>, bool, [u32; 18], Vec<renderer::BoxShadow>)>;

thread_local! {
    static SHADOWS: std::cell::RefCell<ShadowMemo> = Default::default();
}

impl Lengths {
    pub fn edges(&self, style: &Style, sides: [Prop; 4], base: f32) -> [f32; 4] {
        sides.map(|side| {
            style
                .get(side)
                .map_or(0., |value| self.resolve(value, base).unwrap_or(0.))
        })
    }

    pub fn borders(&self, style: &Style, base: f32) -> [f32; 4] {
        BORDERS.map(|[width, kind, _]| {
            if style
                .get(kind)
                .is_none_or(|kind| matches!(kind, "none" | "hidden"))
            {
                return 0.;
            }
            style
                .get(width)
                .map_or(Some(3.), |width| self.resolve(width, base))
                .unwrap_or(0.)
                .max(0.)
        })
    }

    pub fn shadows(&self, value: &str, inset: bool) -> Vec<renderer::BoxShadow> {
        if value == "none" {
            return Vec::new();
        }
        let [vw, vh] = self.viewport.map(f32::to_bits);
        let [cw, ch] = self
            .container
            .map_or([u32::MAX; 2], |size| size.map(f32::to_bits));
        let mut units = [0; 18];
        units[..7].copy_from_slice(&self.font.bits());
        units[7..14].copy_from_slice(&self.root.bits());
        units[14..].copy_from_slice(&[vw, vh, cw, ch]);
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = rustc_hash::FxHasher::default();
            (value, inset, units).hash(&mut hasher);
            hasher.finish()
        };
        if let Some(shadows) = SHADOWS.with_borrow(|memo| {
            memo.get(&hash)
                .filter(|(old, flag, bits, _)| **old == *value && *flag == inset && *bits == units)
                .map(|(.., shadows)| shadows.clone())
        }) {
            return shadows;
        }
        let mut shadows = Vec::new();
        'parse: for shadow in components(value, ',').into_iter().rev() {
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
                    shadows.clear();
                    break 'parse;
                }
            }
            if !(2..=4).contains(&lengths.len()) {
                shadows.clear();
                break 'parse;
            }
            shadows.push(renderer::BoxShadow {
                color: renderer::Color::hex((ink.0 << 8) | u32::from(ink.1)),
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
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Filter(pub [[f32; 4]; 3]);

impl Filter {
    // One filter function: `effects::filters` splits the list and composes the matrices.
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
        let matrix = match (name.to_ascii_lowercase().as_str(), amount) {
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
            ("opacity", Some(amount)) => return (None, amount.min(1.)),
            ("hue-rotate", _) => {
                let Some((angle, _)) = (if args.is_empty() {
                    Some((0., parser::Kind::Angle))
                } else {
                    units.math(args, 0., &[]).filter(|(value, kind)| {
                        *kind == parser::Kind::Angle
                            || *kind == parser::Kind::Number && *value == 0.
                    })
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
            ((row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2] + row[3]).clamp(0., 1.) * 255.)
                .round() as u32
        });
        (r << 16 | g << 8 | b, alpha)
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

#[test]
fn numbers_print_like_the_exact_formatter() {
    let exact = |value: f64| {
        let value = if value.abs() < 1e-9 { 0. } else { value };
        let digits = if value == 0. || !value.is_finite() {
            0
        } else {
            (5 - value.abs().log10().floor() as i32).max(0) as usize
        };
        let text = format!("{value:.digits$}");
        let text = if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.')
        } else {
            &text
        };
        if text == "-0" { "0" } else { text }.to_owned()
    };
    let mut values = vec![0., -0., 1., 2.5, 0.3, 1e-9, -1e-9, 2e-9, 9.999995];
    values.extend([9.9999949, 0.0000125, 123456.5, 99999.95, 1e300]);
    values.extend([12.000004, 0.00999999, f64::NAN]);
    values.extend([0.1 + 0.2, 1. / 3., -2. / 3., 1. / 0.]);
    values.extend((1..60).flat_map(|power| [2f64.powi(-power), -3. * 2f64.powi(-power)]));
    let mut seed = 7u64;
    for _ in 0..200_000 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mantissa = (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
        values.push(mantissa * 10f64.powi((seed % 19) as i32 - 10));
        values.push(((seed >> 20) % 2_000_000) as f64 / 10f64.powi((seed % 9) as i32));
    }
    for value in values {
        assert_eq!(number(value), exact(value), "{value:e}");
    }
}

#[test]
fn split_values_like_the_tokenizer() {
    let values = [
        "0 1px 3px 0 rgb(0 0 0 / 0.1), 0 1px 2px -1px rgb(0 0 0 / 0.1)",
        "rotate(1deg)scale(2)",
        "f(x)(y)",
        "f(x)[y]",
        "f(a]b) c",
        "calc(1px)/2 3",
        "calc(1px), 2",
        "  a \t b\n c  ",
        "a,b, ,c,",
        "f(x) , g(y)",
        "translate(1px, 2px) rotate(3deg)",
        "repeat(2, minmax(0, 1fr)) [a b] 1fr",
        "f((1)) g",
        "f(",
        "a)",
        "url(a b) c",
        "URL(x) y",
        "\"a b\" c",
        "a /* b */ c",
        "var(--a,1px) 2px",
        "1px-2px",
        "f(1)%",
        "f(1)-1px",
        "{a b} c",
        "ﬁ g(ä) h",
    ];
    let mut seed = 1u64;
    let random: Vec<String> = (0..20000)
        .map(|_| {
            (0..seed % 24)
                .map(|_| {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    b"ab1-+.%/*(),[]{}  \t#"[(seed >> 33) as usize % 21] as char
                })
                .collect()
        })
        .collect();
    for value in values
        .iter()
        .copied()
        .chain(random.iter().map(String::as_str))
    {
        for separator in [' ', ',', '/'] {
            assert_eq!(
                components(value, separator),
                tokenized(value, separator),
                "{value:?} {separator:?}"
            );
        }
    }
}

pub(crate) fn number(value: f64) -> String {
    let mut out = String::new();
    write_number(value, &mut out);
    out
}

pub(crate) fn write_number(value: f64, out: &mut String) {
    use std::fmt::Write;
    let value = if value.abs() < 1e-9 { 0. } else { value };
    let digits = if value == 0. || !value.is_finite() {
        0
    } else {
        (5 - value.abs().log10().floor() as i32).max(0) as usize
    };
    let scaled = value.abs() * 10f64.powi(digits as i32);
    if (1..=15).contains(&digits) && (scaled.fract() - 0.5).abs() > 1e-6 {
        let (whole, power) = (scaled.round() as u64, 10u64.pow(digits as u32));
        if value < 0. {
            out.push('-');
        }
        let _ = write!(out, "{}", whole / power);
        let (mut fraction, mut width) = (whole % power, digits);
        if fraction != 0 {
            while fraction % 10 == 0 {
                fraction /= 10;
                width -= 1;
            }
            let _ = write!(out, ".{fraction:0width$}");
        }
        return;
    }
    let start = out.len();
    let _ = write!(out, "{value:.digits$}");
    if out[start..].contains('.') {
        let end = out.trim_end_matches('0').trim_end_matches('.').len();
        out.truncate(end);
    }
    if &out[start..] == "-0" {
        out.truncate(start);
        out.push('0');
    }
}
