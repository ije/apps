use super::{Declarations, Style, components};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub(crate) struct Animation {
    pub name: String,
    pub paused: bool,
    duration: f64,
    delay: f64,
    iterations: f64,
    direction: String,
    fill: String,
    timing: String,
    frames: Vec<(f64, f32, String)>,
}

impl Animation {
    pub fn parse(
        style: &Style,
        keyframes: &HashMap<String, Vec<(f32, Declarations)>>,
    ) -> Vec<Self> {
        let shorthand = components(style.get("animation").map_or("none", String::as_str), ',');
        if shorthand.is_empty() {
            return Vec::new();
        }
        let names = style
            .get("animation-name")
            .map(|value| components(value, ','));
        (0..names.as_ref().map_or(shorthand.len(), Vec::len))
            .filter_map(|index| {
                let mut values = HashMap::from([
                    ("name", "none"),
                    ("duration", "0s"),
                    ("delay", "0s"),
                    ("iteration-count", "1"),
                    ("direction", "normal"),
                    ("fill-mode", "none"),
                    ("timing-function", "ease"),
                    ("play-state", "running"),
                ]);
                let mut times = 0;
                for part in components(shorthand[index % shorthand.len()], ' ') {
                    let key = if part.ends_with('s') && time(part).is_some() {
                        times += 1;
                        if times == 1 { "duration" } else { "delay" }
                    } else if part == "infinite" || part.parse::<f64>().is_ok() {
                        "iteration-count"
                    } else if matches!(
                        part,
                        "normal" | "reverse" | "alternate" | "alternate-reverse"
                    ) {
                        "direction"
                    } else if matches!(part, "forwards" | "backwards" | "both" | "none") {
                        "fill-mode"
                    } else if matches!(part, "paused" | "running") {
                        "play-state"
                    } else if matches!(
                        part,
                        "ease"
                            | "linear"
                            | "ease-in"
                            | "ease-out"
                            | "ease-in-out"
                            | "step-start"
                            | "step-end"
                    ) || part.starts_with("cubic-bezier(")
                        || part.starts_with("steps(")
                    {
                        "timing-function"
                    } else {
                        "name"
                    };
                    values.insert(key, part);
                }
                for key in [
                    "name",
                    "duration",
                    "delay",
                    "iteration-count",
                    "direction",
                    "fill-mode",
                    "timing-function",
                    "play-state",
                ] {
                    if let Some(value) = style.get(&format!("animation-{key}")) {
                        let parts = components(value, ',');
                        if !parts.is_empty() {
                            values.insert(key, parts[index % parts.len()]);
                        }
                    }
                }
                let name = values["name"].trim_matches(['\'', '"']);
                let rules = keyframes.get(name)?;
                let base = style
                    .get("opacity")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1.);
                let mut frames = vec![(0., base, String::new()), (1., base, String::new())];
                for (offset, declarations) in rules {
                    let mut frame = style.clone();
                    let declarations = declarations.resolve(None, 0.);
                    let Some(opacity) = declarations.get("opacity") else {
                        continue;
                    };
                    if opacity.ends_with("!important") {
                        continue;
                    }
                    frame.extend(declarations.clone());
                    super::resolve_style(&mut frame, Some(style));
                    if let Some(value) = frame
                        .get("opacity")
                        .and_then(|value| value.parse::<f32>().ok())
                        .filter(|value| value.is_finite())
                    {
                        frames.retain(|(at, _, _)| *at != *offset as f64);
                        frames.push((
                            *offset as f64,
                            value,
                            declarations
                                .get("animation-timing-function")
                                .cloned()
                                .unwrap_or_default(),
                        ));
                    }
                }
                frames.sort_by(|a, b| a.0.total_cmp(&b.0));
                Some(Self {
                    name: name.into(),
                    paused: values["play-state"] == "paused",
                    duration: time(values["duration"])?.max(0.),
                    delay: time(values["delay"])?,
                    iterations: if values["iteration-count"] == "infinite" {
                        f64::INFINITY
                    } else {
                        values["iteration-count"]
                            .parse::<f64>()
                            .ok()
                            .filter(|value| value.is_finite())?
                            .max(0.)
                    },
                    direction: values["direction"].into(),
                    fill: values["fill-mode"].into(),
                    timing: values["timing-function"].into(),
                    frames,
                })
            })
            .collect()
    }

    pub fn sample(&self, elapsed: f64) -> (Option<f32>, bool) {
        let time = elapsed - self.delay;
        let end = if self.duration == 0. {
            0.
        } else {
            self.duration * self.iterations
        };
        let running = !self.paused && time < end;
        if time < 0. && !matches!(self.fill.as_str(), "backwards" | "both")
            || time >= end && !matches!(self.fill.as_str(), "forwards" | "both")
        {
            return (None, running);
        }
        let overall = if time < 0. {
            0.
        } else if self.duration == 0. {
            if self.iterations.is_finite() {
                self.iterations
            } else {
                1.
            }
        } else {
            time.max(0.).min(end) / self.duration
        };
        let mut iteration = overall.floor();
        let mut progress = overall.fract();
        if time >= end && overall > 0. && progress == 0. {
            progress = 1.;
            iteration -= 1.;
        }
        if self.direction == "reverse"
            || self.direction == "alternate" && iteration % 2. == 1.
            || self.direction == "alternate-reverse" && iteration % 2. == 0.
        {
            progress = 1. - progress;
        }
        let frame = self
            .frames
            .windows(2)
            .find(|pair| progress <= pair[1].0)
            .unwrap();
        let delta = ((progress - frame[0].0) / (frame[1].0 - frame[0].0)).clamp(0., 1.);
        let timing = if frame[0].2.is_empty() {
            &self.timing
        } else {
            &frame[0].2
        };
        let delta = easing(timing, delta) as f32;
        (
            Some((frame[0].1 + (frame[1].1 - frame[0].1) * delta).clamp(0., 1.)),
            running,
        )
    }
}

fn time(value: &str) -> Option<f64> {
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

fn easing(value: &str, progress: f64) -> f64 {
    let curve = match value {
        "linear" => return progress,
        "step-start" => return 1.,
        "step-end" => return if progress == 1. { 1. } else { 0. },
        "ease-in" => [0.42, 0., 1., 1.],
        "ease-out" => [0., 0., 0.58, 1.],
        "ease-in-out" => [0.42, 0., 0.58, 1.],
        _ if value.starts_with("steps(") => {
            let parts = components(
                value.trim_start_matches("steps(").trim_end_matches(')'),
                ',',
            );
            let count = parts
                .first()
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(1.)
                .max(1.);
            let position = parts.get(1).copied().unwrap_or("end");
            let start = matches!(position, "start" | "jump-start" | "jump-both");
            let jumps = count
                + if position == "jump-both" {
                    1.
                } else if position == "jump-none" {
                    -1.
                } else {
                    0.
                };
            return ((progress * count).floor() + f64::from(start)).min(jumps) / jumps.max(1.);
        }
        _ if value.starts_with("cubic-bezier(") => {
            let parts: Vec<f64> = components(
                value
                    .trim_start_matches("cubic-bezier(")
                    .trim_end_matches(')'),
                ',',
            )
            .into_iter()
            .filter_map(|value| value.parse().ok())
            .collect();
            parts.try_into().unwrap_or([0.25, 0.1, 0.25, 1.])
        }
        _ => [0.25, 0.1, 0.25, 1.],
    };
    if progress == 0. || progress == 1. {
        return progress;
    }
    let (mut low, mut high) = (0., 1.);
    for _ in 0..20 {
        let t = (low + high) / 2.;
        let x =
            3. * (1. - t) * (1. - t) * t * curve[0] + 3. * (1. - t) * t * t * curve[2] + t * t * t;
        if x < progress {
            low = t;
        } else {
            high = t;
        }
    }
    let t = (low + high) / 2.;
    3. * (1. - t) * (1. - t) * t * curve[1] + 3. * (1. - t) * t * t * curve[3] + t * t * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::Document;

    #[test]
    fn keyframes_follow_delay_easing_iterations_and_fill() {
        let doc = Document::parse(
            "<style>@keyframes pulse{from{opacity:1}50%{opacity:0}to{opacity:1}}div{animation:1.2s .8s infinite pulse}</style><div></div>",
        );
        let styles = super::super::compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut super::super::StyleCache::default(),
            None,
        );
        let id = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
        let effect = &styles.animations[&id][0];
        assert_eq!(effect.sample(0.4), (None, true));
        assert_eq!(effect.sample(0.8), (Some(1.), true));
        assert!(effect.sample(1.4).0.unwrap() < 0.0001);
        assert!((effect.sample(1.1).0.unwrap() - 0.1976).abs() < 0.001);
        assert!((effect.sample(2.3).0.unwrap() - effect.sample(1.1).0.unwrap()).abs() < 0.001);
        let mut finite = effect.clone();
        finite.duration = 1.;
        finite.delay = -0.5;
        finite.iterations = 2.;
        finite.direction = "alternate".into();
        finite.fill = "both".into();
        assert_eq!(finite.sample(0.).0, Some(0.));
        assert_eq!(finite.sample(2.), (Some(1.), false));
        finite.paused = true;
        assert!(!finite.sample(0.).1);
        finite.duration = 0.;
        assert!(!finite.sample(0.).1);
        finite.iterations = f64::INFINITY;
        assert_eq!(finite.sample(0.), (Some(1.), false));
    }

    #[test]
    fn timing_functions_preserve_endpoints_and_steps() {
        for name in [
            "ease",
            "ease-in",
            "ease-out",
            "ease-in-out",
            "linear",
            "cubic-bezier(.2,0,.8,1)",
        ] {
            assert_eq!(easing(name, 0.), 0.);
            assert_eq!(easing(name, 1.), 1.);
        }
        assert_eq!(easing("steps(2, jump-end)", 0.49), 0.);
        assert_eq!(easing("steps(2, jump-end)", 0.5), 0.5);
        assert_eq!(easing("steps(2, jump-start)", 0.), 0.5);
        assert!(easing("steps()", 0.5).is_finite());
        let frames = HashMap::from([("pulse".into(), Vec::new())]);
        assert!(
            Animation::parse(
                &Style::from([
                    ("animation".into(), "".into()),
                    ("animation-name".into(), "pulse".into())
                ]),
                &frames
            )
            .is_empty()
        );
        assert!(
            Animation::parse(
                &Style::from([
                    ("animation".into(), "1s pulse".into()),
                    ("animation-iteration-count".into(), "NaN".into())
                ]),
                &frames
            )
            .is_empty()
        );
    }
}
