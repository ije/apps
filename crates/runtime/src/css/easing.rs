use super::{Lengths, components};
use std::rc::Rc;

pub(crate) fn timing_function(part: &str) -> bool {
    matches!(
        part,
        "ease" | "linear" | "ease-in" | "ease-out" | "ease-in-out" | "step-start" | "step-end"
    ) || part.starts_with("cubic-bezier(")
        || part.starts_with("steps(")
        || part.starts_with("linear(")
}

pub(crate) fn time(value: &str) -> Option<f64> {
    if value.ends_with(')') {
        return Lengths::new(16., 100., 100.)
            .math(value, f32::NAN, &[])
            .filter(|(value, kind)| *kind == crate::css::Kind::Time && value.is_finite())
            .map(|(value, _)| value);
    }
    let (value, scale) = value
        .strip_suffix("ms")
        .map(|value| (value, 0.001))
        .or_else(|| value.strip_suffix('s').map(|value| (value, 1.)))?;
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|value| value * scale)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Easing {
    Linear,
    Bezier(Box<[f64; 4]>),
    // The position is 0 for end, 1 for start, 2 for both and 3 for none.
    Steps(u32, u8),
    Points(Rc<[(f64, f64)]>),
}

const EASES: [(&str, [f64; 4]); 4] = [
    ("ease", [0.25, 0.1, 0.25, 1.]),
    ("ease-in", [0.42, 0., 1., 1.]),
    ("ease-out", [0., 0., 0.58, 1.]),
    ("ease-in-out", [0.42, 0., 0.58, 1.]),
];

impl Easing {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let number = |value: &str| value.parse::<f64>().ok().filter(|value| value.is_finite());
        match value {
            "linear" => return Some(Self::Linear),
            "step-start" => return Some(Self::Steps(1, 1)),
            "step-end" => return Some(Self::Steps(1, 0)),
            _ => {}
        }
        if let Some((_, curve)) = EASES.iter().find(|(name, _)| *name == value) {
            return Some(Self::Bezier(Box::new(*curve)));
        }
        let (name, args) = value.strip_suffix(')')?.split_once('(')?;
        let args = components(args, ',');
        match name {
            "cubic-bezier" => {
                let curve: [f64; 4] = args
                    .into_iter()
                    .map(number)
                    .collect::<Option<Vec<_>>>()?
                    .try_into()
                    .ok()?;
                ((0. ..=1.).contains(&curve[0]) && (0. ..=1.).contains(&curve[2]))
                    .then(|| Self::Bezier(Box::new(curve)))
            }
            "steps" => {
                let position = match args.get(1).copied() {
                    None | Some("end" | "jump-end") => 0,
                    Some("start" | "jump-start") => 1,
                    Some("jump-both") => 2,
                    Some("jump-none") => 3,
                    _ => return None,
                };
                let count = args.first()?.parse::<u32>().ok()?;
                (args.len() <= 2 && count > u32::from(position == 3))
                    .then_some(Self::Steps(count, position))
            }
            "linear" => {
                let mut stops: Vec<(Option<f64>, f64)> = Vec::new();
                for part in args {
                    let (mut output, mut inputs) = (None, Vec::new());
                    for item in components(part, ' ') {
                        match item.strip_suffix('%') {
                            Some(input) if inputs.len() < 2 => inputs.push(number(input)? / 100.),
                            None if output.is_none() => output = Some(number(item)?),
                            _ => return None,
                        }
                    }
                    let output = output?;
                    if inputs.is_empty() {
                        stops.push((None, output));
                    }
                    stops.extend(inputs.into_iter().map(|input| (Some(input), output)));
                }
                if stops.len() < 2 {
                    return None;
                }
                stops[0].0.get_or_insert(0.);
                let last = stops.len() - 1;
                let mut largest = f64::MIN;
                for (index, (input, _)) in stops.iter_mut().enumerate() {
                    if index == last && input.is_none() {
                        *input = Some(largest.max(1.));
                    }
                    if let Some(input) = input {
                        *input = input.max(largest);
                        largest = *input;
                    }
                }
                let mut index = 1;
                while index < stops.len() {
                    if stops[index].0.is_some() {
                        index += 1;
                        continue;
                    }
                    let end = (index..stops.len())
                        .find(|&end| stops[end].0.is_some())
                        .unwrap_or(last);
                    let [from, to] =
                        [stops[index - 1].0, stops[end].0].map(|input| input.unwrap_or(1.));
                    for (step, stop) in stops[index..end].iter_mut().enumerate() {
                        stop.0 =
                            Some(from + (to - from) * (step + 1) as f64 / (end - index + 1) as f64);
                    }
                    index = end;
                }
                Some(Self::Points(
                    stops
                        .into_iter()
                        .map(|(input, output)| (input.unwrap_or(1.), output))
                        .collect(),
                ))
            }
            _ => None,
        }
    }

    pub(crate) fn at(&self, progress: f64, before: bool) -> f64 {
        let curve = match self {
            Self::Linear => return progress,
            Self::Points(points) => {
                let index = points
                    .iter()
                    .rposition(|point| point.0 <= progress)
                    .unwrap_or(0)
                    .min(points.len() - 2);
                let ((x0, y0), (x1, y1)) = (points[index], points[index + 1]);
                return if x1 > x0 {
                    y0 + (y1 - y0) * (progress - x0) / (x1 - x0)
                } else {
                    y1
                };
            }
            Self::Steps(count, position) => {
                let count = f64::from(*count);
                let jumps = count + [0., 0., 1., -1.][usize::from(*position)];
                let mut step =
                    (progress * count).floor() + f64::from(u8::from(matches!(position, 1 | 2)));
                if before && (progress * count).fract() == 0. {
                    step -= 1.;
                }
                if progress >= 0. && step < 0. {
                    step = 0.;
                }
                if progress <= 1. && step > jumps {
                    step = jumps;
                }
                return step / jumps;
            }
            Self::Bezier(curve) => curve,
        };
        if progress == 0. || progress == 1. {
            return progress;
        }
        let bezier = |t: f64, a: f64, b: f64| {
            3. * (1. - t) * (1. - t) * t * a + 3. * (1. - t) * t * t * b + t * t * t
        };
        let mut t = progress;
        for _ in 0..8 {
            let x = bezier(t, curve[0], curve[2]) - progress;
            if x.abs() < 1e-7 {
                return bezier(t, curve[1], curve[3]);
            }
            let slope = 3. * (1. - t) * (1. - t) * curve[0]
                + 6. * (1. - t) * t * (curve[2] - curve[0])
                + 3. * t * t * (1. - curve[2]);
            if slope.abs() < 1e-6 {
                break;
            }
            t -= x / slope;
        }
        let (mut low, mut high) = (0., 1.);
        t = progress;
        for _ in 0..40 {
            let x = bezier(t, curve[0], curve[2]);
            if (x - progress).abs() < 1e-7 {
                break;
            }
            if x < progress {
                low = t;
            } else {
                high = t;
            }
            t = (low + high) / 2.;
        }
        bezier(t, curve[1], curve[3])
    }
}

impl std::fmt::Display for Easing {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Linear => f.write_str("linear"),
            Self::Bezier(curve) => match EASES.iter().find(|(_, other)| other == &**curve) {
                Some((name, _)) => f.write_str(name),
                None => write!(
                    f,
                    "cubic-bezier({}, {}, {}, {})",
                    curve[0], curve[1], curve[2], curve[3]
                ),
            },
            Self::Steps(count, position) => write!(
                f,
                "steps({count}{})",
                ["", ", start", ", jump-both", ", jump-none"][usize::from(*position)]
            ),
            Self::Points(points) => {
                f.write_str("linear(")?;
                for (index, (input, output)) in points.iter().enumerate() {
                    let comma = if index > 0 { ", " } else { "" };
                    write!(f, "{comma}{output} {}%", input * 100.)?;
                }
                f.write_str(")")
            }
        }
    }
}
