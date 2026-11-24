use crate::style::keyframes::interpolate::{
    Lerp, PAINT, add, animatable, computed, interpolate, keeps_box_in_place, lerp,
};
use crate::style::used::resolve_keywords;
use crate::style::vars::resolve_style;
use std::collections::HashMap;
use std::rc::Rc;
pub(crate) mod interpolate;

use crate::css::{
    properties::Prop, properties::Registry, properties::Style, sheet::Declarations,
    value::easing::Easing, value::easing::time, value::easing::timing_function,
    value::tokens::components,
};

#[derive(Clone, Debug)]
pub(crate) struct Animation {
    pub name: String,
    pub(crate) paused: bool,
    pub(crate) duration: f64,
    pub(crate) delay: f64,
    pub(crate) iterations: f64,
    pub(crate) direction: u8,
    // Index in FILLS: bit 1 fills backwards, bit 2 forwards.
    pub(crate) fill: u8,
    pub(crate) timing: Easing,
    pub(crate) easing: Easing,
    pub(crate) end_delay: f64,
    pub(crate) iteration_start: f64,
    pub(crate) composite: u8,
    pub pseudo: u8,
    pub(crate) tracks: Vec<(Prop, Vec<Keyframe>)>,
}

pub(crate) const DIRECTIONS: [&str; 4] = ["normal", "reverse", "alternate", "alternate-reverse"];
pub(crate) const FILLS: [&str; 4] = ["none", "backwards", "forwards", "both"];
pub(crate) const COMPOSITES: [&str; 3] = ["replace", "add", "accumulate"];

pub(crate) fn keyword(table: &[&str], value: &str) -> Option<u8> {
    table
        .iter()
        .position(|&name| name == value)
        .map(|index| index as u8)
}

#[derive(Clone, Debug)]
pub(crate) struct Keyframe {
    pub(crate) at: f64,
    pub(crate) value: String,
    pub(crate) timing: Option<Easing>,
    pub(crate) lerp: Option<Rc<Lerp>>,
    pub(crate) implicit: Implicit,
}

// The keys of a track without frames at 0 and 1, for the last value under it. This is a cache, so `Debug` skips it.
#[derive(Clone, Default)]
pub(crate) struct Implicit(pub(crate) std::cell::RefCell<Option<Rc<Filled>>>);

impl std::fmt::Debug for Implicit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("_")
    }
}

pub(crate) struct Filled {
    pub(crate) under: Option<String>,
    pub(crate) composite: u8,
    // Offset, value and the index of the frame that gives the timing.
    pub(crate) keys: Vec<(f64, String, Option<usize>)>,
    pub(crate) lerps: Vec<Option<Rc<Lerp>>>,
}

// A duration or delay alone transitions `all`, the initial `transition-property`.
pub(crate) const TRANSITIONS: [Prop; 4] = [
    Prop::TRANSITION,
    Prop::TRANSITION_PROPERTY,
    Prop::TRANSITION_DURATION,
    Prop::TRANSITION_DELAY,
];

pub(crate) type Animated = Vec<(usize, Rc<Style>, bool)>;

impl Animation {
    pub fn parse(
        style: &Style,
        keyframes: &HashMap<String, Vec<(f32, Declarations)>>,
        registry: &Registry,
        theme: u32,
    ) -> Vec<Self> {
        let shorthand = components(style.get(Prop::ANIMATION).unwrap_or("none"), ',');
        if shorthand.is_empty() {
            return Vec::new();
        }
        let names = style
            .get(Prop::ANIMATION_NAME)
            .map(|value| components(value, ','));
        (0..names.as_ref().map_or(shorthand.len(), Vec::len))
            .filter_map(|index| {
                // Name, duration, delay, iteration count, direction, fill mode, timing function, play state.
                let mut values = ["none", "0s", "0s", "1", "normal", "none", "ease", "running"];
                let mut times = 0;
                for part in components(shorthand[index % shorthand.len()], ' ') {
                    let slot = if time(part).is_some() {
                        times += 1;
                        if times == 1 { 1 } else { 2 }
                    } else if part == "infinite" || part.parse::<f64>().is_ok() {
                        3
                    } else if DIRECTIONS.contains(&part) {
                        4
                    } else if FILLS.contains(&part) {
                        5
                    } else if matches!(part, "paused" | "running") {
                        7
                    } else if timing_function(part) {
                        6
                    } else {
                        0
                    };
                    values[slot] = part;
                }
                for (slot, prop) in [
                    Prop::ANIMATION_NAME,
                    Prop::ANIMATION_DURATION,
                    Prop::ANIMATION_DELAY,
                    Prop::ANIMATION_ITERATION_COUNT,
                    Prop::ANIMATION_DIRECTION,
                    Prop::ANIMATION_FILL_MODE,
                    Prop::ANIMATION_TIMING_FUNCTION,
                    Prop::ANIMATION_PLAY_STATE,
                ]
                .into_iter()
                .enumerate()
                {
                    if let Some(value) = style.get(prop) {
                        let parts = components(value, ',');
                        if !parts.is_empty() {
                            values[slot] = parts[index % parts.len()];
                        }
                    }
                }
                let [name, duration, delay, count, direction, fill, timing, state] = values;
                let name = name.trim_matches(['\'', '"']);
                let rules: Vec<_> = keyframes
                    .get(name)?
                    .iter()
                    .map(|(offset, declarations)| {
                        let declarations = declarations.resolve();
                        let timing = declarations
                            .get(Prop::ANIMATION_TIMING_FUNCTION)
                            .and_then(Easing::parse);
                        (f64::from(*offset), declarations, timing)
                    })
                    .collect();
                Some(Self {
                    name: name.into(),
                    paused: state == "paused",
                    duration: time(duration)?.max(0.),
                    delay: time(delay)?,
                    iterations: if count == "infinite" {
                        f64::INFINITY
                    } else {
                        count
                            .parse::<f64>()
                            .ok()
                            .filter(|value| value.is_finite())?
                            .max(0.)
                    },
                    direction: keyword(&DIRECTIONS, direction).unwrap_or(0),
                    fill: keyword(&FILLS, fill).unwrap_or(0),
                    timing: Easing::parse(timing)?,
                    easing: Easing::Linear,
                    end_delay: 0.,
                    iteration_start: 0.,
                    composite: style
                        .get(Prop::ANIMATION_COMPOSITION)
                        .and_then(|value| {
                            let parts = components(value, ',');
                            parts
                                .get(index % parts.len().max(1))
                                .map(|part| keyword(&COMPOSITES, part).unwrap_or(1))
                        })
                        .unwrap_or(0),
                    pseudo: 0,
                    tracks: tracks(style, &rules, registry, theme, true),
                })
            })
            .collect()
    }
}

pub(crate) fn tracks(
    style: &Style,
    rules: &[(f64, Style, Option<Easing>)],
    registry: &Registry,
    theme: u32,
    merge: bool,
) -> Vec<(Prop, Vec<Keyframe>)> {
    let mut tracks: Vec<(Prop, Vec<Keyframe>)> = Vec::new();
    for (offset, declarations, timing) in rules {
        let mut frame = style.clone();
        let mut custom = Vec::new();
        frame.extend(
            declarations
                .iter()
                .filter(|(_, value)| !value.ends_with("!important"))
                .filter_map(|(key, value)| {
                    if key.is_custom() {
                        custom.push((key, value.clone()));
                        return None;
                    }
                    Some((key, value.clone()))
                }),
        );
        resolve_style(&mut frame, Some(style), None, &custom, registry, theme);
        resolve_keywords(&mut frame, Some(style), None);
        for (key, value) in declarations.iter() {
            if value.ends_with("!important")
                || key == Prop::OVERLAY
                || !animatable(key, registry) && !key.is_custom()
            {
                continue;
            }
            let Some(value) = computed(&frame, key, registry) else {
                continue;
            };
            let frame = Keyframe {
                at: *offset,
                value: value.to_owned(),
                timing: timing.clone(),
                lerp: None,
                implicit: Implicit::default(),
            };
            match tracks.iter_mut().find(|(name, _)| *name == key) {
                Some((_, frames)) => {
                    if merge {
                        frames.retain(|other| other.at != frame.at);
                    }
                    frames.push(frame);
                }
                None => tracks.push((key, vec![frame])),
            }
        }
    }
    for (key, frames) in &mut tracks {
        frames.sort_by(|a, b| a.at.total_cmp(&b.at));
        if !key.is_custom() || animatable(*key, registry) {
            for index in 1..frames.len() {
                frames[index - 1].lerp =
                    lerp(key.name(), &frames[index - 1].value, &frames[index].value);
            }
        }
    }
    tracks
}

impl Animation {
    // The active duration. 0 × ∞ is NaN, so an animation without a duration has none.
    pub(crate) fn active(&self) -> f64 {
        if self.duration == 0. {
            0.
        } else {
            self.duration * self.iterations
        }
    }

    // Whether the effect only paints, and whether it keeps the box in place.
    pub(crate) fn paints(&self) -> (bool, bool) {
        (
            self.tracks.iter().all(|(key, _)| PAINT.contains(key)),
            self.tracks.iter().all(|(key, frames)| {
                keeps_box_in_place(*key, frames.iter().map(|frame| frame.value.as_str()))
            }),
        )
    }

    // `coarse` scales the visible step of colors and opacity, which small boxes show less.
    pub fn sample(
        &self,
        elapsed: f64,
        paused: bool,
        base: &Style,
        registry: &Registry,
        out: &mut Style,
        coarse: f64,
    ) -> (Option<f64>, bool) {
        let length = self.active();
        let end = (self.delay + length + self.end_delay).max(0.);
        let start = self.delay.min(end).max(0.);
        let stop = (self.delay + length).min(end).max(0.);
        let before = elapsed < start;
        let ended = elapsed >= stop;
        let active = !paused && !ended && !before;
        let mut wake = (!paused && !ended).then_some(if before { start } else { stop } - elapsed);
        if before && self.fill & 1 == 0 || ended && self.fill & 2 == 0 {
            return (wake, ended);
        }
        let time = (elapsed - self.delay).min(length).max(0.);
        let overall = self.iteration_start
            + if self.duration == 0. {
                if before {
                    0.
                } else if self.iterations.is_finite() {
                    self.iterations
                } else {
                    1.
                }
            } else {
                time / self.duration
            };
        let mut iteration = overall.floor();
        let mut progress = overall.fract();
        if !before && time >= length && self.iterations != 0. && progress == 0. && overall > 0. {
            progress = 1.;
            iteration -= 1.;
        }
        if active {
            wake = wake.map(|wake| {
                wake.min((iteration + 1. - self.iteration_start) * self.duration - time)
            });
        }
        let reverse = match self.direction {
            1 => true,
            2 => iteration % 2. == 1.,
            3 => iteration % 2. == 0.,
            _ => false,
        };
        if reverse {
            progress = 1. - progress;
        }
        if self.easing != Easing::Linear {
            progress = self
                .easing
                .at(progress, before && !reverse || ended && reverse);
            if active {
                wake = Some(0.);
            }
        }
        let replace = self.composite == 0;
        for (property, frames) in &self.tracks {
            // Keyframes at 0 and 1 replace the value under them, so their stored segments apply.
            let fixed = replace && frames[0].at <= 0. && frames[frames.len() - 1].at >= 1.;
            let filled = (!fixed).then(|| {
                let under = out
                    .get(*property)
                    .or_else(|| computed(base, *property, registry));
                let cache = &frames[0].implicit.0;
                if let Some(filled) = cache.borrow().as_ref().filter(|filled| {
                    filled.under.as_deref() == under && filled.composite == self.composite
                }) {
                    return filled.clone();
                }
                let mut keys: Vec<(f64, String, Option<usize>)> = frames
                    .iter()
                    .enumerate()
                    .map(|(index, frame)| (frame.at, frame.value.clone(), Some(index)))
                    .collect();
                if let Some(under) = under {
                    if !replace {
                        for key in &mut keys {
                            if let Some(sum) =
                                add(property.name(), under, &key.1, self.composite == 2)
                            {
                                key.1 = sum;
                            }
                        }
                    }
                    if keys[0].0 > 0. {
                        keys.insert(0, (0., under.to_owned(), None));
                    }
                    if keys[keys.len() - 1].0 < 1. {
                        keys.push((1., under.to_owned(), None));
                    }
                }
                let last = keys.len() - 1;
                let lerps = if property.is_custom() && !animatable(*property, registry) {
                    Vec::new()
                } else {
                    (0..last.max(1))
                        .map(|index| {
                            lerp(
                                property.name(),
                                &keys[index].1,
                                &keys[(index + 1).min(last)].1,
                            )
                        })
                        .collect()
                };
                let filled = Rc::new(Filled {
                    under: under.map(str::to_owned),
                    composite: self.composite,
                    keys,
                    lerps,
                });
                *cache.borrow_mut() = Some(filled.clone());
                filled
            });
            let count = filled
                .as_ref()
                .map_or(frames.len(), |filled| filled.keys.len());
            let key = |index: usize| match &filled {
                None => {
                    let frame = &frames[index];
                    (frame.at, frame.value.as_str(), frame.timing.as_ref())
                }
                Some(filled) => {
                    let (at, value, frame) = &filled.keys[index];
                    let timing = frame.and_then(|frame| frames[frame].timing.as_ref());
                    (*at, value.as_str(), timing)
                }
            };
            let index = (0..count)
                .rposition(|index| key(index).0 <= progress)
                .unwrap_or(0)
                .min(count.saturating_sub(2));
            let (from, to) = (key(index), key((index + 1).min(count - 1)));
            let local = if to.0 > from.0 {
                let local = (progress - from.0) / (to.0 - from.0);
                if (0. ..=1.).contains(&progress) {
                    local.clamp(0., 1.)
                } else {
                    local
                }
            } else {
                1.
            };
            let timing = from.2.unwrap_or(&self.timing);
            let eased = timing.at(local, false);
            let value = if property.is_custom() && !animatable(*property, registry) {
                None
            } else {
                let lerp = match &filled {
                    None => frames[index].lerp.as_deref(),
                    Some(filled) => filled.lerps[index].as_deref(),
                };
                interpolate(lerp, from.1, to.1, eased)
            };
            if active && to.0 > from.0 {
                let steps = |count: f64| {
                    if reverse {
                        ((local * count).ceil() - 1.) / count
                    } else {
                        ((local * count).floor() + 1.) / count
                    }
                };
                let edge = if from.1 == to.1
                    || *property == Prop::DISPLAY && (from.1 == "none" || to.1 == "none")
                    || *property == Prop::VISIBILITY && (from.1 == "visible" || to.1 == "visible")
                {
                    Some(steps(1.))
                } else if let Easing::Steps(count, _) = timing {
                    Some(steps(f64::from(*count)))
                } else if value.is_none() {
                    let (mut low, mut flip) = (0., 1.);
                    for _ in 0..24 {
                        let middle = (low + flip) / 2.;
                        if timing.at(middle, false) < 0.5 {
                            low = middle;
                        } else {
                            flip = middle;
                        }
                    }
                    Some(match (reverse, local < flip) {
                        (false, true) | (true, false) => flip,
                        (false, false) => 1.,
                        (true, true) => 0.,
                    })
                } else {
                    None
                };
                // A smooth value wakes when it moves by a visible step: 1% of opacity, a color level, a quarter
                // pixel or a tenth of a degree. Values that can't be read wake each frame.
                let step = edge.is_none().then(|| {
                    fn numbers(text: &str) -> Option<Vec<(f64, &str)>> {
                        let mut list: Vec<(f64, &str)> = Vec::new();
                        let mut rest = text;
                        while let Some(at) = rest.find(|c: char| c.is_ascii_digit() || c == '.') {
                            let sign = rest[..at].ends_with('-');
                            let end = at
                                + rest[at..]
                                    .find(|c: char| !c.is_ascii_digit() && c != '.')
                                    .unwrap_or(rest.len() - at);
                            let unit = end
                                + rest[end..]
                                    .find(|c: char| !c.is_ascii_alphabetic() && c != '%')
                                    .unwrap_or(rest.len() - end);
                            let value: f64 = rest[at..end].parse().ok()?;
                            list.push((if sign { -value } else { value }, &rest[end..unit]));
                            rest = &rest[unit..];
                        }
                        (!text.contains('#')).then_some(list)
                    }
                    let (a, b) = (numbers(from.1)?, numbers(to.1)?);
                    if a.len() != b.len() {
                        return None;
                    }
                    let mut step: f64 = 1.;
                    for (&(a, unit), &(b, other)) in a.iter().zip(&b) {
                        let range = (b - a).abs();
                        if unit != other || range == 0. {
                            continue;
                        }
                        let visible = match unit {
                            "px" => 0.25,
                            "deg" => 0.1,
                            "rad" => 0.0017,
                            "turn" => 0.0003,
                            "grad" => 0.11,
                            "%" => 0.1,
                            "em" | "rem" => 0.015,
                            "" if *property == Prop::OPACITY => 0.01 * coarse,
                            "" if a.abs().max(b.abs()) > 2. => 0.5 * coarse,
                            "" => 0.001,
                            _ => return None,
                        };
                        step = step.min(visible / range);
                    }
                    let forward = !reverse;
                    let limit = if forward { 1. - local } else { local }.max(0.);
                    let moved = |by: f64| {
                        let at = if forward { local + by } else { local - by };
                        (timing.at(at, false) - eased).abs() >= step
                    };
                    let (mut low, mut high) = (0., (step / 8.).max(1e-4).min(limit));
                    while high < limit && !moved(high) {
                        (low, high) = (high, (high * 2.).min(limit));
                    }
                    if !moved(high) {
                        return Some(limit);
                    }
                    for _ in 0..6 {
                        let middle = (low + high) / 2.;
                        if moved(middle) {
                            high = middle;
                        } else {
                            low = middle;
                        }
                    }
                    Some(low)
                });
                wake = wake.map(|wake| match (edge, step) {
                    (Some(edge), _) => wake
                        .min((edge.clamp(0., 1.) - local).abs() * (to.0 - from.0) * self.duration),
                    (None, Some(Some(by))) => wake.min(by * (to.0 - from.0) * self.duration),
                    (None, _) => 0.,
                });
            }
            let value = value.unwrap_or_else(|| if eased < 0.5 { from.1 } else { to.1 }.into());
            out.insert(*property, value);
        }
        (wake, ended)
    }
}
