use crate::css::{
    Declarations, Easing, Prop, Registry, Style, components, expand, time, timing_function,
};
use crate::dom::Document;
use crate::style::ComputedStyles;
use crate::style::animation::interpolate::{
    Lerp, add, animatable, computed, interpolate, keeps_box_in_place, lerp, transitions,
};
use crate::style::cascade::{resolve_keywords, resolve_style};
use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;
pub(super) const PAINT: &[Prop] = &[
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
];

#[derive(Clone, Debug)]
pub(crate) struct Animation {
    pub name: String,
    pub(super) paused: bool,
    pub(super) duration: f64,
    pub(super) delay: f64,
    pub(super) iterations: f64,
    pub(super) direction: u8,
    // Index in FILLS: bit 1 fills backwards, bit 2 forwards.
    pub(super) fill: u8,
    pub(super) timing: Easing,
    pub(super) easing: Easing,
    pub(super) end_delay: f64,
    pub(super) iteration_start: f64,
    pub(super) composite: u8,
    pub pseudo: u8,
    pub(super) tracks: Vec<(Prop, Vec<Keyframe>)>,
}

pub(super) const DIRECTIONS: [&str; 4] = ["normal", "reverse", "alternate", "alternate-reverse"];
pub(super) const FILLS: [&str; 4] = ["none", "backwards", "forwards", "both"];
pub(super) const COMPOSITES: [&str; 3] = ["replace", "add", "accumulate"];

pub(super) fn keyword(table: &[&str], value: &str) -> Option<u8> {
    table
        .iter()
        .position(|&name| name == value)
        .map(|index| index as u8)
}

#[derive(Clone, Debug)]
pub(super) struct Keyframe {
    pub(super) at: f64,
    pub(super) value: String,
    pub(super) timing: Option<Easing>,
    pub(super) lerp: Option<Rc<Lerp>>,
    pub(super) implicit: Implicit,
}

// The keys of a track without frames at 0 and 1, for the last value under it. This is a cache, so `Debug` skips it.
#[derive(Clone, Default)]
pub(super) struct Implicit(pub(super) std::cell::RefCell<Option<Rc<Filled>>>);

impl std::fmt::Debug for Implicit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("_")
    }
}

pub(super) struct Filled {
    pub(super) under: Option<String>,
    pub(super) composite: u8,
    // Offset, value and the index of the frame that gives the timing.
    pub(super) keys: Vec<(f64, String, Option<usize>)>,
    pub(super) lerps: Vec<Option<Rc<Lerp>>>,
}

// A duration or delay alone transitions `all`, the initial `transition-property`.
pub(super) const TRANSITIONS: [Prop; 4] = [
    Prop::TRANSITION,
    Prop::TRANSITION_PROPERTY,
    Prop::TRANSITION_DURATION,
    Prop::TRANSITION_DELAY,
];

pub(super) type Animated = Vec<(usize, Rc<Style>, bool)>;

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

pub(super) fn tracks(
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
    pub(super) fn active(&self) -> f64 {
        if self.duration == 0. {
            0.
        } else {
            self.duration * self.iterations
        }
    }

    // Whether the effect only paints, and whether it keeps the box in place.
    pub(super) fn paints(&self) -> (bool, bool) {
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

// Base rate and time changes on the current local time. Use f64: large page times can exceed Duration.
#[derive(Clone)]
pub(super) struct Playback {
    pub(super) time: f64,
    pub(super) since: Instant,
    pub(super) rate: f64,
}

impl Playback {
    pub(super) fn at(&self, now: Instant) -> f64 {
        if self.rate == 0. {
            return self.time;
        }
        self.time + now.saturating_duration_since(self.since).as_secs_f64() * self.rate
    }

    pub(super) fn set(&mut self, now: Instant, rate: f64) {
        self.time = self.at(now);
        self.since = now;
        self.rate = rate;
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Phase {
    Pending,
    Running,
    Ended,
    Canceled,
}

pub(super) struct Clock {
    pub(super) serial: u32,
    pub(super) name: String,
    pub(super) playback: Playback,
    pub(super) phase: Phase,
    pub(super) iteration: f64,
    pub(super) delay: f64,
    pub(super) held: Option<bool>,
}

#[derive(Clone)]
pub(super) struct Transition {
    pub(super) serial: u32,
    pub(super) property: Prop,
    pub(super) from: String,
    pub(super) to: String,
    pub(super) playback: Playback,
    pub(super) delay: f64,
    pub(super) duration: f64,
    pub(super) timing: Easing,
    pub(super) lerp: Option<Rc<Lerp>>,
    pub(super) reverse: String,
    pub(super) shortening: f64,
    pub(super) started: bool,
}

impl Transition {
    pub(super) fn progress(&self, now: Instant) -> Option<f64> {
        let time = self.playback.at(now) - self.delay;
        (time < self.duration).then(|| self.timing.at((time / self.duration).max(0.), false))
    }

    pub(super) fn value(&self, progress: f64) -> Rc<str> {
        let near = if progress < 0.5 { &self.from } else { &self.to };
        interpolate(self.lerp.as_deref(), &self.from, &self.to, progress)
            .unwrap_or_else(|| near.as_str().into())
    }

    pub(super) fn shows(&self, now: Instant) -> bool {
        self.property == Prop::DISPLAY
            && self
                .progress(now)
                .is_some_and(|progress| &*self.value(progress) != "none")
    }
}

pub(super) fn show(
    painted: &mut Cow<[bool]>,
    doc: &Document,
    styles: &ComputedStyles,
    shows: &dyn Fn(usize) -> bool,
    id: usize,
) {
    if painted.get(id) != Some(&false)
        || doc.nodes[id]
            .parent
            .is_some_and(|parent| painted.get(parent) != Some(&true))
    {
        return;
    }
    let painted = painted.to_mut();
    let mut pending = vec![id];
    while let Some(id) = pending.pop() {
        painted[id] = true;
        for &child in styles.children.get(&id).unwrap_or(&doc.nodes[id].children) {
            if painted.get(child) == Some(&false)
                && (styles.values[child].get(Prop::DISPLAY) != Some("none") || shows(child))
            {
                pending.push(child);
            }
        }
    }
}

pub(super) type Events = Vec<(usize, &'static str, String, f64)>;

pub(super) fn retarget(
    old: &Style,
    new: &Style,
    running: &[Transition],
    registry: &Registry,
    now: Instant,
    serial: &mut u32,
    (id, pseudo): (usize, &str),
) -> (Vec<Transition>, Events) {
    let mut runs = Vec::new();
    let mut properties: Vec<(Prop, f64, f64, Easing, bool)> = Vec::new();
    for (name, duration, delay, timing, discrete) in transitions(new) {
        let names: Vec<Prop> = match name.as_str() {
            "none" => continue,
            "all" => old
                .iter()
                .chain(new.iter())
                .map(|(key, _)| key)
                .filter(|&key| animatable(key, registry) && old.get(key) != new.get(key))
                .chain(running.iter().map(|transition| transition.property))
                .collect(),
            _ => {
                let mut longhands = Vec::new();
                if !expand(&name, "0", &mut longhands) {
                    longhands.push((name, String::new()));
                }
                longhands
                    .into_iter()
                    .map(|(name, _)| Prop::new(&name))
                    .collect()
            }
        };
        for name in names {
            properties.retain(|property| property.0 != name);
            properties.push((name, duration, delay, timing.clone(), discrete));
        }
    }
    let mut next = Vec::new();
    for (property, duration, delay, timing, discrete) in properties {
        let Some(after) = computed(new, property, registry) else {
            continue;
        };
        if property.is_custom() && !animatable(property, registry) {
            continue;
        }
        if duration.max(0.) + delay <= 0. {
            continue;
        }
        let old_transition = running.iter().find(|old| old.property == property);
        let (from, reverse, factor) = match old_transition {
            Some(old) if old.to == after => {
                next.push(old.clone());
                continue;
            }
            Some(old) => {
                let progress = old.progress(now);
                let current = progress.map_or_else(|| old.to.clone(), |p| old.value(p).to_string());
                if old.reverse == after {
                    let factor = (progress.unwrap_or(1.) * old.shortening + 1. - old.shortening)
                        .abs()
                        .clamp(0., 1.);
                    (current, old.to.clone(), factor)
                } else {
                    (current.clone(), current, 1.)
                }
            }
            None => match computed(old, property, registry) {
                Some(before) => (before.to_owned(), before.to_owned(), 1.),
                None => continue,
            },
        };
        if from == after {
            continue;
        }
        let lerp = lerp(property.name(), &from, after);
        if !discrete && (matches!(property, Prop::DISPLAY | Prop::OVERLAY) || lerp.is_none()) {
            continue;
        }
        *serial += 1;
        let delay = if delay < 0. { delay * factor } else { delay };
        next.push(Transition {
            serial: *serial,
            property,
            from,
            to: after.to_owned(),
            playback: Playback {
                time: 0.,
                since: now,
                rate: 1.,
            },
            delay,
            duration: duration.max(0.) * factor,
            timing,
            lerp,
            reverse,
            shortening: factor,
            started: false,
        });
        runs.push((
            id,
            "transitionrun",
            format!("{pseudo}{}", property.name()),
            (-delay).max(0.),
        ));
    }
    (next, runs)
}

pub(super) fn cancel(
    running: &[Transition],
    kept: &[Transition],
    (id, pseudo): (usize, &str),
    now: Instant,
    events: &mut Events,
) {
    for transition in running {
        if !kept
            .iter()
            .any(|next| next.property == transition.property && next.serial == transition.serial)
        {
            let time = transition.playback.at(now) - transition.delay;
            events.push((
                id,
                "transitioncancel",
                format!("{pseudo}{}", transition.property.name()),
                time.clamp(0., transition.duration),
            ));
        }
    }
}

pub(super) type Wake = (usize, bool, bool, f64, f64);

pub(super) fn advance(
    list: &mut Vec<Transition>,
    (id, pseudo): (usize, &str),
    now: Instant,
    out: &mut Style,
    events: &mut Events,
    wakes: &mut Vec<Wake>,
) {
    list.retain_mut(|transition| match transition.progress(now) {
        Some(progress) => {
            let time = transition.playback.at(now) - transition.delay;
            if !transition.started && time >= 0. {
                transition.started = true;
                events.push((
                    id,
                    "transitionstart",
                    format!("{pseudo}{}", transition.property.name()),
                    (-transition.delay).max(0.),
                ));
            }
            out.insert(transition.property, transition.value(progress));
            let held = (transition.playback.rate == 0.).then_some(f64::INFINITY);
            let flat = matches!(transition.property, Prop::DISPLAY | Prop::OVERLAY)
                && (transition.from == "none" || transition.to == "none");
            wakes.push((
                id,
                pseudo.is_empty() && PAINT.contains(&transition.property),
                pseudo.is_empty()
                    && keeps_box_in_place(
                        transition.property,
                        [transition.from.as_str(), transition.to.as_str()].into_iter(),
                    ),
                held.unwrap_or(if flat && time > 0. {
                    transition.duration - time
                } else {
                    (-time).max(0.)
                }),
                held.unwrap_or(transition.duration - time),
            ));
            true
        }
        None => {
            events.push((
                id,
                "transitionend",
                format!("{pseudo}{}", transition.property.name()),
                transition.duration,
            ));
            false
        }
    });
}

pub(super) fn after(now: Instant, seconds: f64) -> Option<Instant> {
    std::time::Duration::try_from_secs_f64(seconds.max(0.))
        .ok()
        .and_then(|delay| now.checked_add(delay))
}

pub(super) fn drop_important(styles: &ComputedStyles, id: usize, out: &mut Style) {
    if let Some(props) = styles.important.get(&id) {
        for &prop in props.iter() {
            out.remove(prop);
        }
    }
}

pub(crate) struct Scripted {
    pub(super) id: u32,
    pub(super) rules: Vec<(f64, Style, Option<Easing>)>,
    pub(super) effect: Animation,
    pub(super) base: Option<(Rc<dyn std::any::Any>, crate::css::Vars)>,
    pub(super) playback: Playback,
}

impl Scripted {
    pub fn new(
        id: u32,
        keyframes: &[(f64, &str, &str)],
        [delay, end_delay, duration, iterations, iteration_start]: [f64; 5],
        [direction, fill, easing, composite]: [&str; 4],
        pseudo: u8,
    ) -> Self {
        Self {
            id,
            rules: keyframes
                .iter()
                .map(|(offset, source, easing)| {
                    (
                        *offset,
                        crate::css::declarations(source),
                        Easing::parse(easing),
                    )
                })
                .collect(),
            effect: Animation {
                name: String::new(),
                paused: false,
                duration,
                delay,
                iterations,
                direction: keyword(&DIRECTIONS, direction).unwrap_or(0),
                fill: keyword(&FILLS, fill).unwrap_or(0),
                timing: Easing::Linear,
                easing: Easing::parse(easing).unwrap_or(Easing::Linear),
                end_delay,
                iteration_start,
                composite: keyword(&COMPOSITES, composite).unwrap_or(1),
                pseudo,
                tracks: Vec::new(),
            },
            base: None,
            playback: Playback {
                time: 0.,
                since: Instant::now(),
                rate: 0.,
            },
        }
    }

    // Returns the wake of the sample, or None when the pseudo-element has no style.
    pub(super) fn sample<'a>(
        &mut self,
        id: usize,
        styles: &ComputedStyles,
        now: Instant,
        out: impl FnOnce() -> &'a mut Style,
        coarse: f64,
    ) -> Option<Option<f64>> {
        let (owner, style): (Rc<dyn std::any::Any>, &Style) = match self.effect.pseudo {
            0 => (styles.values[id].clone(), &*styles.values[id]),
            slot => {
                let pair = styles.pseudos.get(&id)?;
                (pair.clone(), &pair[usize::from(slot) - 1])
            }
        };
        let vars = &styles.vars[id];
        let base = if style.vars().is_empty() && !vars.is_empty() {
            let mut style = style.clone();
            style.set_vars(vars.clone());
            Cow::Owned(style)
        } else {
            Cow::Borrowed(style)
        };
        if self
            .base
            .as_ref()
            .is_none_or(|(old, vars)| !Rc::ptr_eq(old, &owner) || !Rc::ptr_eq(vars, base.vars()))
        {
            self.effect.tracks = tracks(&base, &self.rules, &styles.registry, styles.theme, false);
            self.base = Some((owner, base.vars().clone()));
        }
        let (local, paused) = (self.playback.at(now), self.playback.rate == 0.);
        Some(
            self.effect
                .sample(local, paused, &base, &styles.registry, out(), coarse)
                .0,
        )
    }
}
