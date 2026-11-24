pub(crate) mod color;
pub(super) mod counter;
pub(super) mod easing;
pub(super) mod grammar;
pub(super) mod resolved;
pub(super) mod shorthand;
pub(super) mod specified;
pub(super) mod tokens;
pub(super) mod transform;
pub(super) mod units;

use crate::css::{Kind, Lengths, components, paint};

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

type ShadowMemo = rustc_hash::FxHashMap<u64, (Box<str>, bool, [u32; 18], Vec<renderer::BoxShadow>)>;

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
    let mut units = [0; 18];
    units[..7].copy_from_slice(&sizes.font.bits());
    units[7..14].copy_from_slice(&sizes.root.bits());
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
                    Some((0., Kind::Angle))
                } else {
                    units.math(args, 0., &[]).filter(|(value, kind)| {
                        *kind == Kind::Angle || *kind == Kind::Number && *value == 0.
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
