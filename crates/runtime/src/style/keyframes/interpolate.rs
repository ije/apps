use crate::css::{
    properties::Prop, properties::Registry, properties::Style, value::calc_size,
    value::color::color, value::easing::Easing, value::easing::time,
    value::easing::timing_function, value::grammar::matched, value::tokens::components,
    value::units::Lengths,
};
use std::borrow::Cow;
use std::rc::Rc;
pub(crate) fn add(property: &str, under: &str, value: &str, accumulate: bool) -> Option<String> {
    fn number(text: &str) -> Option<(f64, &str)> {
        let bytes = text.as_bytes();
        // An `e` is an exponent only before a digit or sign, so `2em` keeps its unit.
        let end = (0..bytes.len())
            .find(|&index| {
                let byte = bytes[index];
                !(byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'-' | b'+')
                    || matches!(byte, b'e' | b'E')
                        && bytes.get(index + 1).is_some_and(|next| {
                            next.is_ascii_digit() || matches!(next, b'-' | b'+')
                        }))
            })
            .unwrap_or(bytes.len());
        Some((text[..end].parse::<f64>().ok()?, &text[end..]))
    }
    let sum = |a: &str, b: &str, offset: f64| match (number(a), number(b)) {
        (Some((a, unit)), Some((b, other))) if unit == other || a == 0. && unit.is_empty() => {
            Some(format!(
                "{}{}",
                crate::css::value::tokens::number(a + b - offset),
                if unit.is_empty() { other } else { unit }
            ))
        }
        (Some(_), Some((0., ""))) => Some(a.to_owned()),
        (Some(_), Some(_)) if offset == 0. => Some(format!("calc({a} + {b})")),
        _ => None,
    };
    if matches!(
        property,
        "transform" | "filter" | "backdrop-filter" | "box-shadow" | "text-shadow"
    ) {
        if under == "none" || value == "none" {
            return Some(if under == "none" { value } else { under }.to_owned());
        }
        if accumulate && !property.ends_with("shadow") {
            let [a, b] = [under, value].map(|list| components(list, ' '));
            let combined: Option<Vec<String>> = (a.len() == b.len())
                .then(|| {
                    a.iter()
                        .zip(&b)
                        .map(|(a, b)| {
                            let (name, args) = a.strip_suffix(')')?.split_once('(')?;
                            let (other, more) = b.strip_suffix(')')?.split_once('(')?;
                            let [args, more] = [args, more].map(|args| components(args, ','));
                            if name != other
                                || args.len() != more.len()
                                || name.starts_with("matrix")
                            {
                                return None;
                            }
                            let offset = if name.starts_with("scale")
                                || matches!(
                                    name,
                                    "brightness" | "contrast" | "opacity" | "saturate"
                                ) {
                                1.
                            } else {
                                0.
                            };
                            let args: Option<Vec<String>> = args
                                .iter()
                                .zip(&more)
                                .map(|(a, b)| sum(a, b, offset))
                                .collect();
                            Some(format!("{name}({})", args?.join(", ")))
                        })
                        .collect()
                })
                .flatten();
            if let Some(combined) = combined {
                return Some(combined.join(" "));
            }
        }
        return Some(if property.ends_with("shadow") {
            format!("{under}, {value}")
        } else {
            format!("{under} {value}")
        });
    }
    if let (Some((a, alpha)), Some((b, other))) = (color(under), color(value)) {
        let channel = |shift: u32| ((a >> shift & 255) + (b >> shift & 255)).min(255);
        let alpha = (f64::from(alpha) + f64::from(other)).min(255.) / 255.;
        return Some(format!(
            "rgba({}, {}, {}, {})",
            channel(16),
            channel(8),
            channel(0),
            crate::css::value::tokens::number(alpha)
        ));
    }
    let [a, b] = [under, value].map(|list| components(list, ' '));
    if a.len() != b.len() {
        return None;
    }
    let parts: Option<Vec<String>> = a.iter().zip(&b).map(|(a, b)| sum(a, b, 0.)).collect();
    Some(parts?.join(" "))
}

pub(crate) fn keeps_box_in_place<'a>(
    prop: Prop,
    mut values: impl Iterator<Item = &'a str>,
) -> bool {
    match prop {
        Prop::TRANSLATE
        | Prop::OFFSET_ANCHOR
        | Prop::OFFSET_DISTANCE
        | Prop::OFFSET_PATH
        | Prop::OFFSET_POSITION
        | Prop::OFFSET_ROTATE => false,
        Prop::TRANSFORM => values.all(|value| {
            !["translate", "matrix", "perspective"]
                .iter()
                .any(|name| value.contains(name))
        }),
        _ => PAINT.contains(&prop),
    }
}

pub(crate) fn animatable(prop: Prop, registry: &Registry) -> bool {
    let key = prop.name();
    if prop.is_custom() {
        return registry.get(&prop).is_some_and(|(syntax, ..)| {
            syntax.split('|').any(|kind| {
                matches!(
                    kind.trim().trim_end_matches(['+', '#']),
                    "<length>"
                        | "<number>"
                        | "<percentage>"
                        | "<length-percentage>"
                        | "<color>"
                        | "<angle>"
                        | "<time>"
                        | "<integer>"
                        | "<resolution>"
                        | "<transform-function>"
                        | "<transform-list>"
                )
            })
        });
    }
    !key.starts_with("animation")
        && !key.starts_with("transition")
        && !key.starts_with("container")
        && key != "will-change"
}

// The property that interpolates like this one.
fn kind(prop: Prop, registry: &Registry) -> &'static str {
    match registry.get(&prop).filter(|_| prop.is_custom()) {
        Some((syntax, ..)) if syntax.contains("<transform") => "transform",
        Some((syntax, ..)) if syntax.contains("<integer>") && !syntax.contains("<number>") => {
            "z-index"
        }
        _ => prop.name(),
    }
}

pub(crate) fn lerp_of(prop: Prop, registry: &Registry, from: &str, to: &str) -> Option<Rc<Lerp>> {
    // Values of two types of a syntax switch halfway.
    if prop.is_custom()
        && let Some((syntax, ..)) = registry.get(&prop)
        && matched(syntax, from) != matched(syntax, to)
    {
        return None;
    }
    lerp(kind(prop, registry), from, to)
}

pub(crate) fn computed<'a>(
    style: &'a Style,
    prop: Prop,
    registry: &'a Registry,
) -> Option<&'a str> {
    let key = prop.name();
    if prop.is_custom() {
        return style
            .get(prop)
            .or_else(|| registry.get(&prop)?.2.as_deref());
    }
    style.get(prop).or(match key {
        "opacity" | "flex-shrink" => Some("1"),
        "flex-grow" => Some("0"),
        "transform" | "translate" | "rotate" | "scale" | "box-shadow" | "text-shadow"
        | "filter" | "backdrop-filter" | "max-width" | "max-height" => Some("none"),
        "background-color" => Some("transparent"),
        "width" | "height" | "min-width" | "min-height" | "top" | "right" | "bottom" | "left"
        | "flex-basis" | "z-index" => Some("auto"),
        "row-gap" | "column-gap" | "letter-spacing" | "word-spacing" | "outline-offset"
        | "text-indent" => Some("0px"),
        "background-position" => Some("0% 0%"),
        "offset-distance" => Some("0px"),
        "offset-rotate" => Some("auto"),
        "offset-path" | "clip-path" => Some("none"),
        "visibility" => Some("visible"),
        "display" => Some("inline"),
        "overlay" => Some("none"),
        "position" => Some("static"),
        "content" | "line-height" => Some("normal"),
        _ if key.starts_with("margin-")
            || key.starts_with("padding-")
            || key.ends_with("-radius") =>
        {
            Some("0px")
        }
        _ if key.starts_with("border-") && key.ends_with("-width") || key == "outline-width" => {
            Some("3px")
        }
        _ if key.ends_with("-color") => style.get(Prop::COLOR),
        _ => None,
    })
}

pub(crate) fn transitions(style: &Style) -> Vec<(String, f64, f64, Easing, bool)> {
    const DEFAULT: [&str; 5] = ["all", "0s", "0s", "ease", "normal"];
    let shorthand: Vec<[&str; 5]> = style
        .get(Prop::TRANSITION)
        .map(|value| components(value, ','))
        .unwrap_or_default()
        .into_iter()
        .map(|single| {
            let mut parts = DEFAULT;
            let mut times = 0;
            for part in components(single, ' ') {
                if time(part).is_some() {
                    parts[1 + times.min(1)] = part;
                    times += 1;
                } else if timing_function(part) {
                    parts[3] = part;
                } else if matches!(part, "normal" | "allow-discrete") {
                    parts[4] = part;
                } else {
                    parts[0] = part;
                }
            }
            parts
        })
        .collect();
    let longhands = [
        Prop::TRANSITION_PROPERTY,
        Prop::TRANSITION_DURATION,
        Prop::TRANSITION_DELAY,
        Prop::TRANSITION_TIMING_FUNCTION,
        Prop::TRANSITION_BEHAVIOR,
    ]
    .map(|prop| style.get(prop).map(|value| components(value, ',')));
    (0..longhands[0]
        .as_ref()
        .map_or(shorthand.len().max(1), Vec::len))
        .filter_map(|index| {
            let mut parts = shorthand
                .get(index % shorthand.len().max(1))
                .copied()
                .unwrap_or(DEFAULT);
            for (part, list) in parts.iter_mut().zip(&longhands) {
                if let Some(list) = list.as_ref().filter(|list| !list.is_empty()) {
                    *part = list[index % list.len()];
                }
            }
            Some((
                parts[0].to_owned(),
                time(parts[1])?,
                time(parts[2])?,
                Easing::parse(parts[3])?,
                parts[4] == "allow-discrete",
            ))
        })
        .collect()
}

pub(crate) fn lerp(property: &str, from: &str, to: &str) -> Option<Rc<Lerp>> {
    if matches!(property, "opacity" | "scale") && (from.contains('%') || to.contains('%')) {
        let number = |value: &str| {
            value
                .split_ascii_whitespace()
                .map(|part| {
                    part.strip_suffix('%')
                        .and_then(|value| value.parse::<f64>().ok())
                        .map_or(part.to_owned(), |value| (value / 100.).to_string())
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        let (from, to) = (number(from), number(to));
        if !from.contains('%') && !to.contains('%') {
            return lerp(property, &from, &to);
        }
    }
    thread_local! {
        static LERPS: std::cell::RefCell<Lerps> = Default::default();
    }
    let key = std::hash::BuildHasher::hash_one(&rustc_hash::FxBuildHasher, (property, from, to));
    LERPS.with_borrow_mut(|lerps| {
        let same =
            |(p, a, b, _): &(String, String, String, _)| p == property && a == from && b == to;
        if let Some(entry) = lerps[0].get(&key).filter(|entry| same(entry)) {
            return entry.3.clone();
        }
        // Two generations: a full one becomes the old one, and a hit in the old one moves to the new one.
        let entry = lerps[1].remove(&key).filter(same).unwrap_or_else(|| {
            let lerp = Lerp::new(property, from, to).map(Rc::new);
            (property.into(), from.into(), to.into(), lerp)
        });
        if lerps[0].len() >= 512 {
            let [new, old] = lerps;
            std::mem::swap(new, old);
            new.clear();
        }
        let lerp = entry.3.clone();
        lerps[0].insert(key, entry);
        lerp
    })
}

pub(crate) fn interpolate(lerp: Option<&Lerp>, from: &str, to: &str, t: f64) -> Option<Rc<str>> {
    if from == to || t == 0. {
        return Some(from.into());
    }
    if t == 1. {
        return Some(to.into());
    }
    thread_local! {
        static TEXT: std::cell::RefCell<String> = Default::default();
    }
    let lerp = lerp?;
    TEXT.with_borrow_mut(|text| {
        text.clear();
        lerp.write(t, text);
        Some(Rc::from(&**text))
    })
}

pub(crate) type Lerps = [rustc_hash::FxHashMap<u64, (String, String, String, Option<Rc<Lerp>>)>; 2];

#[derive(Debug)]
pub(crate) enum Lerp {
    Same(String),
    Between(String, String, String),
    List(Vec<Lerp>, &'static str),
    Function(String, Box<Lerp>),
    Color((u32, u8), (u32, u8)),
    Oklab([f64; 4], [f64; 4]),
    Number(f64, f64, String, bool),
    Calc(String, String, bool),
    Matrix(Box<[[f64; 13]; 2]>, bool),
    // The basis and both calculations of `calc-size()`.
    Size(String, String, String),
}

// The sizes that interpolate with keywords under `interpolate-size: allow-keywords`.
const SIZES: [Prop; 6] = [
    Prop::WIDTH,
    Prop::HEIGHT,
    Prop::MIN_WIDTH,
    Prop::MIN_HEIGHT,
    Prop::MAX_WIDTH,
    Prop::MAX_HEIGHT,
];

/// The size keyword as `calc-size()` when the element interpolates it, and other values as they are.
pub(crate) fn sized<'a>(style: &Style, prop: Prop, value: &'a str) -> Cow<'a, str> {
    if matches!(
        value,
        "auto" | "min-content" | "max-content" | "fit-content" | "stretch"
    ) && SIZES.contains(&prop)
        && style.get(Prop::INTERPOLATE_SIZE) == Some("allow-keywords")
    {
        return format!("calc-size({value}, size)").into();
    }
    value.into()
}

pub(crate) const IDENTITY: [f64; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

fn multiply(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    std::array::from_fn(|index| {
        let (column, row) = (index / 4, index % 4);
        (0..4).map(|k| a[k * 4 + row] * b[column * 4 + k]).sum()
    })
}

fn rotation([x, y, z, w]: [f64; 4]) -> [f64; 16] {
    let mut m = IDENTITY;
    m[0] = 1. - 2. * (y * y + z * z);
    m[1] = 2. * (x * y + z * w);
    m[2] = 2. * (x * z - y * w);
    m[4] = 2. * (x * y - z * w);
    m[5] = 1. - 2. * (x * x + z * z);
    m[6] = 2. * (y * z + x * w);
    m[8] = 2. * (x * z + y * w);
    m[9] = 2. * (y * z - x * w);
    m[10] = 1. - 2. * (x * x + y * y);
    m
}

fn decompose(list: &str) -> Option<([f64; 13], bool)> {
    // Relative units and percentages give NaN with these units, so `math` rejects them.
    let units = Lengths {
        font: crate::css::value::units::FontUnits::new(f32::NAN),
        root: crate::css::value::units::FontUnits::new(f32::NAN),
        viewport: [f32::NAN; 2],
        container: None,
    };
    let mut m = IDENTITY;
    for part in components(list, ' ') {
        let (name, args) = part.strip_suffix(')')?.split_once('(')?;
        let args = components(args, ',')
            .into_iter()
            .map(|arg| {
                let (value, kind) = units.math(arg, f32::NAN, &[])?;
                Some(if kind == crate::css::value::units::Kind::Angle {
                    value.to_radians()
                } else {
                    value
                })
            })
            .collect::<Option<Vec<f64>>>()?;
        let mut n = IDENTITY;
        let mut turn = |[x, y, z]: [f64; 3], angle: f64| {
            let length = (x * x + y * y + z * z).sqrt();
            if length > 0. {
                let sin = (angle / 2.).sin() / length;
                n = rotation([x * sin, y * sin, z * sin, (angle / 2.).cos()]);
            }
        };
        match (name.to_ascii_lowercase().as_str(), args.as_slice()) {
            ("translate" | "translatex", [x]) => n[12] = *x,
            ("translate", [x, y]) => [n[12], n[13]] = [*x, *y],
            ("translatey", [y]) => n[13] = *y,
            ("translatez", [z]) => n[14] = *z,
            ("translate3d", [x, y, z]) => [n[12], n[13], n[14]] = [*x, *y, *z],
            ("scale", [x]) => [n[0], n[5]] = [*x, *x],
            ("scale", [x, y]) => [n[0], n[5]] = [*x, *y],
            ("scalex", [x]) => n[0] = *x,
            ("scaley", [y]) => n[5] = *y,
            ("scalez", [z]) => n[10] = *z,
            ("scale3d", [x, y, z]) => [n[0], n[5], n[10]] = [*x, *y, *z],
            ("rotate" | "rotatez", [a]) => turn([0., 0., 1.], *a),
            ("rotatex", [a]) => turn([1., 0., 0.], *a),
            ("rotatey", [a]) => turn([0., 1., 0.], *a),
            ("rotate3d", [x, y, z, a]) => turn([*x, *y, *z], *a),
            ("skew" | "skewx", [x]) => n[4] = x.tan(),
            ("skew", [x, y]) => [n[4], n[1]] = [x.tan(), y.tan()],
            ("skewy", [y]) => n[1] = y.tan(),
            ("matrix", [a, b, c, d, e, f]) => {
                [n[0], n[1], n[4], n[5], n[12], n[13]] = [*a, *b, *c, *d, *e, *f]
            }
            ("matrix3d", values) if values.len() == 16 => n.copy_from_slice(values),
            _ => return None,
        }
        m = multiply(&m, &n);
    }
    let flat = [2, 3, 6, 7, 8, 9, 11, 14]
        .iter()
        .all(|&index| m[index] == 0.)
        && m[10] == 1.
        && m[15] == 1.;
    if m[15] == 0. || m[3] != 0. || m[7] != 0. || m[11] != 0. {
        return None;
    }
    let w = m[15];
    m.iter_mut().for_each(|value| *value /= w);
    let length = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let less =
        |a: [f64; 3], b: [f64; 3], k: f64| [a[0] - b[0] * k, a[1] - b[1] * k, a[2] - b[2] * k];
    let mut rows = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let (mut scale, mut skew) = ([0.; 3], [0.; 3]);
    scale[0] = length(rows[0]);
    rows[0] = rows[0].map(|value| value / scale[0]);
    skew[0] = dot(rows[0], rows[1]);
    rows[1] = less(rows[1], rows[0], skew[0]);
    scale[1] = length(rows[1]);
    rows[1] = rows[1].map(|value| value / scale[1]);
    skew[0] /= scale[1];
    skew[1] = dot(rows[0], rows[2]);
    rows[2] = less(rows[2], rows[0], skew[1]);
    skew[2] = dot(rows[1], rows[2]);
    rows[2] = less(rows[2], rows[1], skew[2]);
    scale[2] = length(rows[2]);
    rows[2] = rows[2].map(|value| value / scale[2]);
    skew[1] /= scale[2];
    skew[2] /= scale[2];
    let [a, b] = [rows[1], rows[2]];
    let cross = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    if dot(rows[0], cross) < 0. {
        scale = scale.map(|value| -value);
        rows = rows.map(|row| row.map(|value| -value));
    }
    let part = |sign: [f64; 3]| {
        0.5 * (1. + sign[0] * rows[0][0] + sign[1] * rows[1][1] + sign[2] * rows[2][2])
            .max(0.)
            .sqrt()
    };
    let mut quaternion = [
        part([1., -1., -1.]),
        part([-1., 1., -1.]),
        part([-1., -1., 1.]),
        part([1., 1., 1.]),
    ];
    for (index, flip) in [
        rows[2][1] > rows[1][2],
        rows[0][2] > rows[2][0],
        rows[1][0] > rows[0][1],
    ]
    .into_iter()
    .enumerate()
    {
        if flip {
            quaternion[index] = -quaternion[index];
        }
    }
    let mut out = [0.; 13];
    out[..3].copy_from_slice(&m[12..15]);
    out[3..6].copy_from_slice(&scale);
    out[6..9].copy_from_slice(&skew);
    out[9..].copy_from_slice(&quaternion);
    out.iter()
        .all(|value| value.is_finite())
        .then_some((out, flat))
}

impl Lerp {
    pub(crate) fn new(property: &str, from: &str, to: &str) -> Option<Self> {
        if from == to {
            return Some(Self::Same(from.to_owned()));
        }
        // Chrome switches images, gradients too, halfway instead of blending them.
        if property.ends_with("-image")
            || matches!(property, "border-image-source" | "overflow-clip-margin")
        {
            return None;
        }
        let keyword = |value| match (property, value) {
            ("font-weight", "normal") => "400",
            ("font-weight", "bold") => "700",
            ("letter-spacing" | "word-spacing", "normal") => "0px",
            (_, "thin" | "medium" | "thick")
                if property.ends_with("-width") && property != "border-image-width" =>
            {
                match value {
                    "thin" => "1px",
                    "medium" => "3px",
                    _ => "5px",
                }
            }
            _ => value,
        };
        if keyword(from) != from || keyword(to) != to {
            return Self::new(property, keyword(from), keyword(to));
        }
        let shown = match property {
            "visibility" if from == "visible" || to == "visible" => Some("visible"),
            "display" | "overlay" if from == "none" || to == "none" => {
                Some(if from == "none" { to } else { from })
            }
            _ => None,
        };
        if let Some(shown) = shown {
            return Some(Self::Between(
                from.to_owned(),
                to.to_owned(),
                shown.to_owned(),
            ));
        }
        if matches!(property, "clip-path" | "offset-path") {
            let (text, pairs) = crate::css::value::shape::between(from, to)?;
            let mut items = Vec::with_capacity(pairs.len() * 2 + 1);
            let mut gaps = text.split('\0');
            items.push(Self::Same(gaps.next()?.to_owned()));
            for ([a, b], text) in pairs.iter().zip(gaps) {
                items.push(Self::new("", a, b)?);
                items.push(Self::Same(text.to_owned()));
            }
            return Some(Self::List(items, ""));
        }
        if property == "offset-rotate" {
            let units = Lengths::new(16., 1., 1.);
            let turn = |value| crate::css::value::shape::turn(value, &units);
            let ((auto, a), (other, b)) = (turn(from)?, turn(to)?);
            let angle = Self::Number(a, b, "deg".into(), false);
            return (auto == other).then(|| match auto {
                true => Self::List(vec![Self::Same("auto".into()), angle], " "),
                false => angle,
            });
        }
        if matches!(property, "offset-anchor" | "offset-position") {
            let [[a, b], [c, d]] = [from, to].map(|value| {
                let parts = components(value, ' ');
                crate::css::value::shape::Position::parse(&parts)
                    .map_or_else(Default::default, |position| position.offsets())
            });
            return Some(Self::List(
                vec![Self::new("", &a, &c)?, Self::new("", &b, &d)?],
                " ",
            ));
        }
        if from.starts_with("calc-size(") || to.starts_with("calc-size(") {
            let [a, b] = [from, to].map(calc_size);
            let basis = match (a, b) {
                (Some(("any", _)), Some((basis, _))) | (Some((basis, _)), Some(("any", _))) => {
                    basis
                }
                (Some((a, _)), Some((b, _))) if a == b => a,
                (Some((basis, _)), None) | (None, Some((basis, _))) => basis,
                _ => return None,
            };
            // A length has no basis of its own, and other keywords don't interpolate.
            let [a, b] = [(a, from), (b, to)].map(|(parts, value)| match parts {
                Some((_, size)) => Some(size),
                None => match Lengths::new(16., 100., 100.).parse(value, 100.)? {
                    (_, true) => Some(value),
                    (number, _) => (number == 0.).then_some("0px"),
                },
            });
            return Some(Self::Size(basis.into(), a?.into(), b?.into()));
        }
        if property == "transform" {
            fn primitive<'a>(name: &str, args: &'a str) -> Option<(&'static str, Vec<&'a str>)> {
                let args = components(args, ',');
                Some(
                    match (name.to_ascii_lowercase().as_str(), args.as_slice()) {
                        ("translate" | "translatex", [x]) => ("translate", vec![*x, "0px"]),
                        ("translate", [x, y]) => ("translate", vec![*x, *y]),
                        ("translatey", [y]) => ("translate", vec!["0px", *y]),
                        ("translatez", [z]) => ("translate3d", vec!["0px", "0px", *z]),
                        ("translate3d", [x, y, z]) => ("translate3d", vec![*x, *y, *z]),
                        ("scale", [x]) => ("scale", vec![*x, *x]),
                        ("scale", [x, y]) => ("scale", vec![*x, *y]),
                        ("scalex", [x]) => ("scale", vec![*x, "1"]),
                        ("scaley", [y]) => ("scale", vec!["1", *y]),
                        ("scalez", [z]) => ("scale3d", vec!["1", "1", *z]),
                        ("scale3d", [x, y, z]) => ("scale3d", vec![*x, *y, *z]),
                        ("rotate" | "rotatez", [a]) => ("rotate", vec![*a]),
                        ("skew" | "skewx", [x]) => ("skew", vec![*x, "0deg"]),
                        ("skew", [x, y]) => ("skew", vec![*x, *y]),
                        ("skewy", [y]) => ("skew", vec!["0deg", *y]),
                        _ => return None,
                    },
                )
            }
            let split = |part: &str| -> Option<(String, String)> {
                let (name, args) = part.strip_suffix(')')?.split_once('(')?;
                Some((name.to_owned(), args.to_owned()))
            };
            let common = |x: &str, y: &str| -> Option<(String, String)> {
                let ((name, args), (other, more)) = (split(x)?, split(y)?);
                if name == other && components(&args, ',').len() == components(&more, ',').len() {
                    return Some((x.to_owned(), y.to_owned()));
                }
                let ((mut p, mut a), (mut q, mut b)) =
                    (primitive(&name, &args)?, primitive(&other, &more)?);
                let fill = |name: &str| if name == "scale" { "1" } else { "0px" };
                if format!("{p}3d") == q {
                    a.push(fill(p));
                    p = q;
                } else if format!("{q}3d") == p {
                    b.push(fill(q));
                    q = p;
                }
                (p == q).then(|| {
                    (
                        format!("{p}({})", a.join(", ")),
                        format!("{q}({})", b.join(", ")),
                    )
                })
            };
            let identity = |part: &str| -> Option<String> {
                let (name, args) = split(part).filter(|(name, _)| name != "perspective")?;
                let lower = name.to_ascii_lowercase();
                let zero = if lower.starts_with("scale") {
                    "1"
                } else if lower.starts_with("translate") {
                    "0px"
                } else {
                    "0deg"
                };
                let args = components(&args, ',')
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        let unit = arg
                            .trim_start_matches(|c: char| c.is_ascii_digit() || ".+-".contains(c));
                        if lower == "rotate3d" && index < 3 {
                            (*arg).to_owned()
                        } else if zero != "1"
                            && unit.len() < arg.len()
                            && unit.bytes().all(|b| b.is_ascii_alphabetic() || b == b'%')
                        {
                            format!("0{unit}")
                        } else {
                            zero.to_owned()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!("{name}({args})"))
            };
            let [a, b] = [from, to].map(|list| {
                if list == "none" {
                    Vec::new()
                } else {
                    components(list, ' ')
                }
            });
            let mut items = Vec::new();
            for index in 0..a.len().max(b.len()) {
                let pair = match (a.get(index), b.get(index)) {
                    (Some(x), Some(y)) => common(x, y),
                    (Some(x), None) => identity(x).map(|y| (x.to_string(), y)),
                    (None, Some(y)) => identity(y).map(|x| (x, y.to_string())),
                    (None, None) => None,
                };
                match pair
                    .filter(|(x, _)| !x.starts_with("matrix"))
                    .and_then(|(x, y)| Self::new("", &x, &y))
                {
                    Some(lerp) => items.push(lerp),
                    None => {
                        let [x, y] =
                            [&a, &b].map(|list| list.get(index..).unwrap_or_default().join(" "));
                        let ((x, flat), (y, also)) = (decompose(&x)?, decompose(&y)?);
                        items.push(Self::Matrix(Box::new([x, y]), flat && also));
                        break;
                    }
                }
            }
            return Some(Self::List(items, " "));
        }
        if matches!(property, "box-shadow" | "text-shadow") {
            let normalize = |value: &str| -> Option<Vec<String>> {
                if value == "none" {
                    return Some(Vec::new());
                }
                components(value, ',')
                    .into_iter()
                    .map(|shadow| {
                        let (mut lengths, mut ink, mut inset) = (Vec::new(), "black", "");
                        for part in components(shadow, ' ') {
                            if part == "inset" {
                                inset = "inset ";
                            } else if part
                                .starts_with(|c: char| c.is_ascii_digit() || ".+-".contains(c))
                                || part.starts_with("calc(")
                            {
                                lengths.push(part);
                            } else {
                                ink = part;
                            }
                        }
                        (2..=4).contains(&lengths.len()).then(|| {
                            lengths.resize(4, "0px");
                            format!("{inset}{} {ink}", lengths.join(" "))
                        })
                    })
                    .collect()
            };
            let (mut a, mut b) = (normalize(from)?, normalize(to)?);
            let (first, second) = (a.clone(), b.clone());
            for (list, other) in [(&mut a, &second), (&mut b, &first)] {
                for shadow in other.iter().skip(list.len()) {
                    list.push(if shadow.starts_with("inset") {
                        "inset 0px 0px 0px 0px transparent".into()
                    } else {
                        "0px 0px 0px 0px transparent".into()
                    });
                }
            }
            if a.is_empty() {
                return Some(Self::Same("none".into()));
            }
            return a
                .iter()
                .zip(&b)
                .map(|(a, b)| Self::new("", a, b))
                .collect::<Option<Vec<_>>>()
                .map(|list| Self::List(list, ", "));
        }
        if matches!(property, "filter" | "backdrop-filter") {
            let [a, b] = [from, to].map(|list| {
                if list == "none" {
                    Vec::new()
                } else {
                    components(list, ' ')
                }
            });
            if a.len() != b.len() {
                let (short, long) = if a.len() < b.len() {
                    (&a, &b)
                } else {
                    (&b, &a)
                };
                if short
                    .iter()
                    .zip(long)
                    .any(|(x, y)| x.split_once('(').map(|p| p.0) != y.split_once('(').map(|p| p.0))
                {
                    return None;
                }
                let mut padded = short.join(" ");
                for part in &long[short.len()..] {
                    let (name, args) = part.strip_suffix(')')?.split_once('(')?;
                    let one = matches!(name, "brightness" | "contrast" | "opacity" | "saturate");
                    if name == "url" || args.is_empty() {
                        return None;
                    }
                    let args = components(args, ' ')
                        .into_iter()
                        .map(|arg| {
                            let unit = arg.trim_start_matches(|c: char| {
                                c.is_ascii_digit() || ".+-".contains(c)
                            });
                            match unit {
                                _ if unit.len() == arg.len() => "transparent".to_owned(),
                                "%" => format!("{}%", if one { 100 } else { 0 }),
                                _ => format!("{}{unit}", u8::from(one && unit.is_empty())),
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !padded.is_empty() {
                        padded.push(' ');
                    }
                    padded.push_str(&format!("{name}({args})"));
                }
                return if a.len() < b.len() {
                    Self::new(property, &padded, to)
                } else {
                    Self::new(property, from, &padded)
                };
            }
        }
        if from == "none" || to == "none" {
            let other = if from == "none" { to } else { from };
            let identity = match property {
                "translate" | "rotate" | "scale" => components(other, ' ')
                    .into_iter()
                    .map(|part| match property {
                        "scale" => "1",
                        "rotate"
                            if part
                                .starts_with(|c: char| c.is_ascii_digit() || ".+-".contains(c)) =>
                        {
                            "0deg"
                        }
                        "rotate" => part,
                        _ => "0px",
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => return None,
            };
            return if from == "none" {
                Self::new(property, &identity, to)
            } else {
                Self::new(property, from, &identity)
            };
        }
        if matches!(property, "translate" | "scale") {
            let lists = [from, to].map(|value| components(value, ' '));
            let count = lists[0].len().max(lists[1].len());
            if lists[0].len() != lists[1].len() && lists.iter().all(|list| !list.is_empty()) {
                let [a, b] = lists.map(|mut list| {
                    for index in list.len()..count {
                        list.push(match (property, index) {
                            ("scale", 1) => list[0],
                            ("scale", _) => "1",
                            _ => "0px",
                        });
                    }
                    list.join(" ")
                });
                return Self::new(property, &a, &b);
            }
        }
        if let (Some(a), Some(b)) = (color(from), color(to)) {
            // As in browsers, colors interpolate in sRGB only when both are legacy sRGB colors.
            return Some(
                match (
                    crate::css::value::color::oklab(from),
                    crate::css::value::color::oklab(to),
                ) {
                    (Some((false, mut a)), Some((_, mut b)))
                    | (Some((_, mut a)), Some((false, mut b))) => {
                        for (a, b) in a.iter_mut().zip(&mut b) {
                            if a.is_nan() {
                                *a = *b;
                            } else if b.is_nan() {
                                *b = *a;
                            }
                        }
                        Self::Oklab(
                            a.map(|v| if v.is_nan() { 0. } else { v }),
                            b.map(|v| if v.is_nan() { 0. } else { v }),
                        )
                    }
                    _ => Self::Color(a, b),
                },
            );
        }
        for separator in [',', ' '] {
            let (a, b) = (components(from, separator), components(to, separator));
            if a.len() != b.len() || a.is_empty() {
                return None;
            }
            if a.len() > 1 {
                return a
                    .iter()
                    .zip(&b)
                    .map(|(a, b)| Self::new(property, a, b))
                    .collect::<Option<Vec<_>>>()
                    .map(|list| Self::List(list, if separator == ',' { ", " } else { " " }));
            }
        }
        fn function(value: &str) -> Option<(&str, &str)> {
            value
                .strip_suffix(')')?
                .split_once('(')
                .filter(|(name, _)| {
                    !matches!(*name, "calc" | "min" | "max" | "clamp" | "var" | "url")
                })
        }
        if let (Some((name, a)), Some((other, b))) = (function(from), function(to))
            && name == other
        {
            return Self::new(property, a, b)
                .map(|args| Self::Function(name.into(), Box::new(args)));
        }
        fn number(value: &str) -> Option<(f64, &str)> {
            let end = value
                .char_indices()
                .find(|&(index, c)| {
                    !(c.is_ascii_digit() || c == '.' || index == 0 && "+-".contains(c))
                })
                .map_or(value.len(), |(index, _)| index);
            let unit = &value[end..];
            let number = value[..end].parse::<f64>().ok()?;
            (number.is_finite() && unit.bytes().all(|b| b.is_ascii_alphabetic() || b == b'%'))
                .then_some((number, unit))
        }
        let length = |value: &str| {
            number(value).is_some_and(|(value, unit)| !unit.is_empty() || value == 0.)
                || ["calc(", "min(", "max(", "clamp("]
                    .iter()
                    .any(|name| value.starts_with(name))
        };
        let integer = matches!(property, "z-index" | "order");
        match (number(from), number(to)) {
            (Some((a, unit)), Some((b, other)))
                if unit == other || a == 0. && unit.is_empty() || b == 0. && other.is_empty() =>
            {
                let unit = if unit.is_empty() { other } else { unit };
                Some(Self::Number(a, b, unit.into(), integer))
            }
            _ if length(from) && length(to) => {
                let term = |value: &str| value.strip_prefix("calc").unwrap_or(value).to_owned();
                Some(Self::Calc(term(from), term(to), integer))
            }
            _ => None,
        }
    }

    pub(crate) fn write(&self, t: f64, out: &mut String) {
        use std::fmt::Write;
        let round = |value: f64, integer: bool, out: &mut String| {
            if integer {
                let _ = write!(out, "{}", (value + 0.5).floor());
            } else {
                crate::css::value::tokens::write_number(value, out);
            }
        };
        match self {
            Self::Size(basis, from, to) => {
                let _ = write!(
                    out,
                    "calc-size({basis}, ({from}) * {} + ({to}) * {t})",
                    1. - t
                );
            }
            Self::Same(value) => out.push_str(value),
            Self::Between(from, to, shown) => out.push_str(if t <= 0. {
                from
            } else if t >= 1. {
                to
            } else {
                shown
            }),
            Self::Matrix(pair, flat) => {
                let [a, b] = &**pair;
                let value: [f64; 13] =
                    std::array::from_fn(|index| a[index] + (b[index] - a[index]) * t);
                let (qa, qb) = (&a[9..], &b[9..]);
                let dot = (0..4)
                    .map(|index| qa[index] * qb[index])
                    .sum::<f64>()
                    .clamp(-1., 1.);
                let quaternion: [f64; 4] = if 1. - dot.abs() < 1e-9 {
                    std::array::from_fn(|index| qa[index])
                } else {
                    let theta = dot.acos();
                    let w = (t * theta).sin() / (1. - dot * dot).sqrt();
                    let s = (t * theta).cos() - dot * w;
                    std::array::from_fn(|index| qa[index] * s + qb[index] * w)
                };
                let mut m = rotation(quaternion);
                for (index, slot) in [(9, 8), (8, 7), (4, 6)] {
                    if value[slot] != 0. {
                        let mut skew = IDENTITY;
                        skew[index] = value[slot];
                        m = multiply(&m, &skew);
                    }
                }
                for (index, cell) in m.iter_mut().enumerate().take(12) {
                    *cell *= value[3 + index / 4];
                }
                m[12..15].copy_from_slice(&value[..3]);
                out.push_str(if *flat { "matrix(" } else { "matrix3d(" });
                let indices: &[usize] = if *flat {
                    &[0, 1, 4, 5, 12, 13]
                } else {
                    &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
                };
                for (position, &index) in indices.iter().enumerate() {
                    if position > 0 {
                        out.push_str(", ");
                    }
                    round(m[index], false, out);
                }
                out.push(')');
            }
            Self::List(items, separator) => {
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(separator);
                    }
                    item.write(t, out);
                }
            }
            Self::Function(name, args) => {
                out.push_str(name);
                out.push('(');
                args.write(t, out);
                out.push(')');
            }
            Self::Color(a, b) => {
                let alpha = f64::from(a.1) + (f64::from(b.1) - f64::from(a.1)) * t;
                let mut rgb = 0;
                for shift in [16, 8, 0] {
                    let channel = |(value, alpha): (u32, u8)| {
                        f64::from((value >> shift) as u8) * f64::from(alpha)
                    };
                    let premultiplied = channel(*a) + (channel(*b) - channel(*a)) * t;
                    rgb = rgb << 8
                        | if alpha > 0. {
                            (premultiplied / alpha).round().clamp(0., 255.) as u32
                        } else {
                            0
                        };
                }
                let value = rgb << 8 | alpha.round().clamp(0., 255.) as u32;
                out.push('#');
                for shift in (0..32).step_by(4).rev() {
                    let digit = (value >> shift & 15) as usize;
                    out.push(char::from(b"0123456789abcdef"[digit]));
                }
            }
            Self::Oklab(a, b) => {
                let alpha = (a[3] + (b[3] - a[3]) * t).clamp(0., 1.);
                out.push_str("oklab(");
                for index in 0..3 {
                    if index > 0 {
                        out.push(' ');
                    }
                    let premultiplied = a[index] * a[3] + (b[index] * b[3] - a[index] * a[3]) * t;
                    let value = if alpha > 0. {
                        premultiplied / alpha
                    } else {
                        a[index] + (b[index] - a[index]) * t
                    };
                    round(value, false, out);
                }
                if alpha < 1. {
                    out.push_str(" / ");
                    round(alpha, false, out);
                }
                out.push(')');
            }
            Self::Number(a, b, unit, integer) => {
                round(a + (b - a) * t, *integer, out);
                out.push_str(unit);
            }
            Self::Calc(from, to, integer) => {
                out.push_str("calc(");
                out.push_str(from);
                out.push_str(" * ");
                round(1. - t, *integer, out);
                out.push_str(" + ");
                out.push_str(to);
                out.push_str(" * ");
                round(t, *integer, out);
                out.push(')');
            }
        }
    }
}
pub(crate) const PAINT: &[Prop] = &[
    Prop::COLOR,
    Prop::OPACITY,
    Prop::TRANSFORM,
    Prop::TRANSLATE,
    Prop::ROTATE,
    Prop::SCALE,
    Prop::BOX_SHADOW,
    Prop::BACKGROUND_COLOR,
    Prop::BACKGROUND_POSITION,
    Prop::BORDER_TOP_COLOR,
    Prop::BORDER_RIGHT_COLOR,
    Prop::BORDER_BOTTOM_COLOR,
    Prop::BORDER_LEFT_COLOR,
    Prop::OUTLINE_COLOR,
    Prop::CARET_COLOR,
    Prop::BORDER_TOP_LEFT_RADIUS,
    Prop::BORDER_TOP_RIGHT_RADIUS,
    Prop::BORDER_BOTTOM_RIGHT_RADIUS,
    Prop::BORDER_BOTTOM_LEFT_RADIUS,
    Prop::VISIBILITY,
    Prop::Z_INDEX,
    Prop::FILTER,
    Prop::WEBKIT_FILTER,
    Prop::BACKDROP_FILTER,
    Prop::WEBKIT_BACKDROP_FILTER,
    Prop::OFFSET_ANCHOR,
    Prop::OFFSET_DISTANCE,
    Prop::OFFSET_PATH,
    Prop::OFFSET_POSITION,
    Prop::OFFSET_ROTATE,
];
