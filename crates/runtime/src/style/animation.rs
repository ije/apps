use super::{
    ComputedStyles, Declarations, Prop, Registry, Style, color, components, expand,
    resolve_keywords, resolve_style,
};
use crate::dom::Document;
use rustc_hash::{FxHashMap, FxHashSet};
use std::{borrow::Cow, collections::HashMap, rc::Rc, time::Instant};

const PAINT: &[Prop] = &[
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
];

#[derive(Clone, Debug)]
pub(crate) struct Animation {
    pub name: String,
    paused: bool,
    duration: f64,
    delay: f64,
    iterations: f64,
    direction: u8,
    // Index in FILLS: bit 1 fills backwards, bit 2 forwards.
    fill: u8,
    timing: Easing,
    easing: Easing,
    end_delay: f64,
    iteration_start: f64,
    composite: u8,
    pub pseudo: u8,
    tracks: Vec<(Prop, Vec<Keyframe>)>,
}

const DIRECTIONS: [&str; 4] = ["normal", "reverse", "alternate", "alternate-reverse"];
const FILLS: [&str; 4] = ["none", "backwards", "forwards", "both"];
const COMPOSITES: [&str; 3] = ["replace", "add", "accumulate"];

fn keyword(table: &[&str], value: &str) -> Option<u8> {
    table
        .iter()
        .position(|&name| name == value)
        .map(|index| index as u8)
}

#[derive(Clone, Debug)]
struct Keyframe {
    at: f64,
    value: String,
    timing: Option<Easing>,
    lerp: Option<Rc<Lerp>>,
}

// A duration or delay alone transitions `all`, the initial `transition-property`.
const TRANSITIONS: [Prop; 4] = [
    Prop::TRANSITION,
    Prop::TRANSITION_PROPERTY,
    Prop::TRANSITION_DURATION,
    Prop::TRANSITION_DELAY,
];

type Animated = Vec<(usize, Rc<Style>, bool)>;

impl Animation {
    pub fn parse(
        style: &Style,
        keyframes: &HashMap<String, Vec<(f32, Declarations)>>,
        registry: &Registry,
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
                    let slot = if part.ends_with('s') && time(part).is_some() {
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
                    tracks: tracks(style, &rules, registry, true),
                })
            })
            .collect()
    }
}

fn tracks(
    style: &Style,
    rules: &[(f64, Style, Option<Easing>)],
    registry: &Registry,
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
        resolve_style(&mut frame, Some(style), None, &custom, registry);
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
    fn active(&self) -> f64 {
        if self.duration == 0. {
            0.
        } else {
            self.duration * self.iterations
        }
    }

    // Whether the effect only paints, and whether it keeps the box in place.
    fn paints(&self) -> (bool, bool) {
        (
            self.tracks.iter().all(|(key, _)| PAINT.contains(key)),
            self.tracks.iter().all(|(key, frames)| {
                keeps_box_in_place(*key, frames.iter().map(|frame| frame.value.as_str()))
            }),
        )
    }

    pub fn sample(
        &self,
        elapsed: f64,
        paused: bool,
        base: &Style,
        registry: &Registry,
        out: &mut Style,
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
        let mut keys: Vec<(f64, Cow<str>, Option<&Easing>)> = Vec::new();
        for (property, frames) in &self.tracks {
            // Keyframes at 0 and 1 replace the value under them, so their stored segments apply.
            let fixed = replace && frames[0].at <= 0. && frames[frames.len() - 1].at >= 1.;
            if !fixed {
                keys.clear();
                keys.extend(frames.iter().map(|frame| {
                    (
                        frame.at,
                        Cow::Borrowed(frame.value.as_str()),
                        frame.timing.as_ref(),
                    )
                }));
                let under = out
                    .get(*property)
                    .or_else(|| computed(base, *property, registry))
                    .map(str::to_owned);
                if let Some(under) = under {
                    if !replace {
                        for key in &mut keys {
                            if let Some(sum) =
                                add(property.name(), &under, &key.1, self.composite == 2)
                            {
                                key.1 = Cow::Owned(sum);
                            }
                        }
                    }
                    if keys[0].0 > 0. {
                        keys.insert(0, (0., Cow::Owned(under.clone()), None));
                    }
                    if keys[keys.len() - 1].0 < 1. {
                        keys.push((1., Cow::Owned(under), None));
                    }
                }
            }
            let count = if fixed { frames.len() } else { keys.len() };
            let key = |index: usize| {
                if fixed {
                    let frame = &frames[index];
                    (frame.at, frame.value.as_str(), frame.timing.as_ref())
                } else {
                    let (at, value, timing) = &keys[index];
                    (*at, &**value, *timing)
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
            } else if fixed {
                interpolate(frames[index].lerp.as_deref(), from.1, to.1, eased)
            } else {
                interpolate(
                    lerp(property.name(), from.1, to.1).as_deref(),
                    from.1,
                    to.1,
                    eased,
                )
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
                wake = wake.map(|wake| match edge {
                    Some(edge) => wake
                        .min((edge.clamp(0., 1.) - local).abs() * (to.0 - from.0) * self.duration),
                    None => 0.,
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
struct Playback {
    time: f64,
    since: Instant,
    rate: f64,
}

impl Playback {
    fn at(&self, now: Instant) -> f64 {
        if self.rate == 0. {
            return self.time;
        }
        self.time + now.saturating_duration_since(self.since).as_secs_f64() * self.rate
    }

    fn set(&mut self, now: Instant, rate: f64) {
        self.time = self.at(now);
        self.since = now;
        self.rate = rate;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Pending,
    Running,
    Ended,
    Canceled,
}

struct Clock {
    serial: u32,
    name: String,
    playback: Playback,
    phase: Phase,
    iteration: f64,
    delay: f64,
    held: Option<bool>,
}

#[derive(Clone)]
struct Transition {
    serial: u32,
    property: Prop,
    from: String,
    to: String,
    playback: Playback,
    delay: f64,
    duration: f64,
    timing: Easing,
    lerp: Option<Rc<Lerp>>,
    reverse: String,
    shortening: f64,
    started: bool,
}

impl Transition {
    fn progress(&self, now: Instant) -> Option<f64> {
        let time = self.playback.at(now) - self.delay;
        (time < self.duration).then(|| self.timing.at((time / self.duration).max(0.), false))
    }

    fn value(&self, progress: f64) -> Rc<str> {
        let near = if progress < 0.5 { &self.from } else { &self.to };
        interpolate(self.lerp.as_deref(), &self.from, &self.to, progress)
            .unwrap_or_else(|| near.as_str().into())
    }

    fn shows(&self, now: Instant) -> bool {
        self.property == Prop::DISPLAY
            && self
                .progress(now)
                .is_some_and(|progress| &*self.value(progress) != "none")
    }
}

fn show(
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

type Events = Vec<(usize, &'static str, String, f64)>;

fn retarget(
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

fn cancel(
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

type Wake = (usize, bool, bool, f64, f64);

fn advance(
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

fn after(now: Instant, seconds: f64) -> Option<Instant> {
    std::time::Duration::try_from_secs_f64(seconds.max(0.))
        .ok()
        .and_then(|delay| now.checked_add(delay))
}

fn drop_important(styles: &ComputedStyles, id: usize, out: &mut Style) {
    if let Some(props) = styles.important.get(&id) {
        for &prop in props.iter() {
            out.remove(prop);
        }
    }
}

pub(crate) struct Scripted {
    id: u32,
    rules: Vec<(f64, Style, Option<Easing>)>,
    effect: Animation,
    base: Option<(Rc<dyn std::any::Any>, super::Vars)>,
    playback: Playback,
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
                    (*offset, super::declarations(source), Easing::parse(easing))
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
    fn sample<'a>(
        &mut self,
        id: usize,
        styles: &ComputedStyles,
        now: Instant,
        out: impl FnOnce() -> &'a mut Style,
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
            self.effect.tracks = tracks(&base, &self.rules, &styles.registry, false);
            self.base = Some((owner, base.vars().clone()));
        }
        let (local, paused) = (self.playback.at(now), self.playback.rate == 0.);
        Some(
            self.effect
                .sample(local, paused, &base, &styles.registry, out())
                .0,
        )
    }
}

#[derive(Default)]
pub(crate) struct Motion {
    previous: Vec<Rc<Style>>,
    // Keep previous equal to seen except at touched indices. Updates visit only changed chunks.
    seen: super::Chunked<Rc<Style>>,
    touched: Vec<usize>,
    clocks: FxHashMap<usize, Vec<Clock>>,
    transitions: FxHashMap<usize, Vec<Transition>>,
    styled: FxHashMap<usize, (Rc<Style>, Style, Rc<Style>)>,
    pub custom: HashMap<usize, Style>,
    pub wake: Option<(Instant, renderer::Timer)>,
    pub parked: Vec<usize>,
    scripted: FxHashMap<usize, Vec<Scripted>>,
    script_nodes: FxHashMap<u32, usize>,
    serial: u32,
    queued: Events,
    pub pseudos: Vec<(usize, Rc<[Style; 2]>)>,
    pub painted: Option<Vec<bool>>,
    backdrop: FxHashMap<usize, (Rc<Style>, Vec<Transition>)>,
    pub backdrops: Vec<(usize, Rc<Style>)>,
    pub iterations: bool,
}

impl Motion {
    // Clocks line up with the effects only after `update`, so a style change before it skips them.
    fn pairs<'a>(
        &'a self,
        id: usize,
        styles: &'a ComputedStyles,
    ) -> impl Iterator<Item = (&'a Animation, &'a Clock)> {
        let effects = styles
            .animations
            .get(&id)
            .map_or(&[][..], |effects| effects);
        effects
            .iter()
            .zip(self.clocks.get(&id).into_iter().flatten())
            .filter(|(effect, clock)| effect.name == clock.name)
    }

    pub fn timeline(
        &self,
        id: usize,
        styles: &ComputedStyles,
        times: &[Instant],
        props: &[Prop],
    ) -> Vec<Style> {
        let registry = &*styles.registry;
        let base = &*styles.values[id];
        times
            .iter()
            .map(|&at| {
                let mut out = Style::default();
                for (effect, clock) in self.pairs(id, styles) {
                    if clock.phase != Phase::Canceled && effect.pseudo == 0 {
                        let paused = clock.playback.rate == 0.;
                        effect.sample(clock.playback.at(at), paused, base, registry, &mut out);
                    }
                }
                for animation in self.scripted.get(&id).into_iter().flatten() {
                    if animation.effect.pseudo == 0 {
                        let (local, paused) =
                            (animation.playback.at(at), animation.playback.rate == 0.);
                        animation
                            .effect
                            .sample(local, paused, base, registry, &mut out);
                    }
                }
                drop_important(styles, id, &mut out);
                for transition in self.transitions.get(&id).into_iter().flatten() {
                    if let Some(progress) = transition.progress(at) {
                        out.insert(transition.property, transition.value(progress));
                    }
                }
                let mut style = match props {
                    [] => base.clone(),
                    props => props
                        .iter()
                        .filter_map(|&prop| Some((prop, base.value(prop)?.clone())))
                        .collect(),
                };
                style.extend(out.iter().map(|(key, value)| (key, value.clone())));
                style
            })
            .collect()
    }

    pub fn period(
        &self,
        id: usize,
        styles: &ComputedStyles,
        now: Instant,
    ) -> Option<(f64, f64, f64, Option<String>)> {
        if self
            .transitions
            .get(&id)
            .is_some_and(|list| !list.is_empty())
        {
            return None;
        }
        let scripted = self.scripted.get(&id).into_iter().flatten();
        let (mut running, mut effects) = (None, 0);
        for (effect, playback) in self
            .pairs(id, styles)
            .filter(|(_, clock)| clock.phase != Phase::Canceled)
            .map(|(effect, clock)| (effect, &clock.playback))
            .chain(scripted.map(|animation| (&animation.effect, &animation.playback)))
        {
            effects += 1;
            let local = playback.at(now);
            let stop = effect.delay + effect.active() + effect.end_delay.min(0.);
            if effect.pseudo != 0 || playback.rate == 0. || local >= stop {
                continue;
            }
            if running.is_some()
                || playback.rate != 1.
                || effect.duration <= 0.
                || local < effect.delay
            {
                return None;
            }
            let period = effect.duration * if effect.direction >= 2 { 2. } else { 1. };
            let phase = (local - effect.delay + effect.iteration_start * effect.duration)
                .rem_euclid(period);
            running = Some((period, phase, (stop - local) / period, effect));
        }
        running.map(|(period, phase, left, effect)| {
            let shape = (effects == 1).then(|| {
                format!(
                    "{:?}",
                    Animation {
                        name: String::new(),
                        delay: 0.,
                        ..effect.clone()
                    }
                )
            });
            (period, phase, left, shape)
        })
    }

    pub fn next_event(&self, id: usize, styles: &ComputedStyles, now: Instant) -> Option<Instant> {
        let animations = self.pairs(id, styles).filter_map(|(effect, clock)| {
            if clock.playback.rate == 0. || matches!(clock.phase, Phase::Ended | Phase::Canceled) {
                return None;
            }
            let elapsed = clock.playback.at(now);
            let active = elapsed - effect.delay;
            let end = effect.active();
            let at = if active < 0. {
                0.
            } else if effect.duration > 0. && self.iterations {
                (((active / effect.duration).floor() + 1.) * effect.duration).min(end)
            } else {
                end
            };
            Some(effect.delay + at - elapsed)
        });
        let transitions =
            self.transitions
                .get(&id)
                .into_iter()
                .flatten()
                .filter_map(|transition| {
                    let time = transition.playback.at(now) - transition.delay;
                    (transition.playback.rate != 0.).then(|| {
                        if time < 0. {
                            -time
                        } else {
                            transition.duration - time
                        }
                    })
                });
        animations
            .chain(transitions)
            .filter_map(|seconds| after(now, seconds))
            .min()
    }

    pub fn trim(&mut self, len: usize) {
        self.previous.truncate(len);
        self.previous.shrink_to_fit();
        self.seen.clear();
        self.touched = Vec::new();
    }

    pub fn read(&mut self, id: usize, style: &Rc<Style>) {
        if self.previous.len() <= id {
            self.previous.resize_with(id + 1, Default::default);
        }
        if self.previous[id].is_empty() {
            self.previous[id] = style.clone();
            self.touched.push(id);
        }
    }

    pub fn forget(&mut self, id: usize) {
        if let Some(previous) = self.previous.get_mut(id) {
            *previous = super::empty().0;
            self.touched.push(id);
        }
        if !self.clocks.is_empty() {
            self.clocks.remove(&id);
        }
        if !self.transitions.is_empty() {
            self.transitions.remove(&id);
        }
        if !self.scripted.is_empty()
            && let Some(list) = self.scripted.remove(&id)
        {
            for animation in list {
                self.script_nodes.remove(&animation.id);
            }
        }
        if !self.backdrop.is_empty() {
            self.backdrop.remove(&id);
        }
    }

    pub fn script(
        &mut self,
        node: usize,
        mut animation: Scripted,
        time: f64,
        rate: Option<f64>,
        keep: Option<Scripted>,
    ) {
        animation.playback = Playback {
            time,
            since: Instant::now(),
            rate: rate.filter(|rate| rate.is_finite()).unwrap_or(0.),
        };
        if let Some(old) = keep {
            (animation.rules, animation.base, animation.effect.tracks) =
                (old.rules, old.base, old.effect.tracks);
        }
        self.unscript(animation.id);
        self.script_nodes.insert(animation.id, node);
        let list = self.scripted.entry(node).or_default();
        let index = list.partition_point(|other| other.id < animation.id);
        list.insert(index, animation);
    }

    pub fn unscript(&mut self, id: u32) -> Option<(usize, Scripted)> {
        let node = self.script_nodes.remove(&id)?;
        let list = self.scripted.get_mut(&node)?;
        let animation = list.remove(list.iter().position(|animation| animation.id == id)?);
        if list.is_empty() {
            self.scripted.remove(&node);
        }
        Some((node, animation))
    }

    pub fn css(
        &self,
        nodes: &[usize],
        pseudos: bool,
        styles: &ComputedStyles,
    ) -> Vec<serde_json::Value> {
        let now = Instant::now();
        let mut list = Vec::new();
        for &node in nodes {
            for transition in self.transitions.get(&node).into_iter().flatten() {
                let name = transition.property.name();
                list.push(serde_json::json!([
                    transition.serial,
                    node,
                    "transition",
                    name,
                    transition.playback.at(now) * 1000.,
                    if transition.playback.rate == 0. {
                        "paused"
                    } else {
                        "running"
                    },
                    [
                        transition.delay * 1000.,
                        0.,
                        transition.duration * 1000.,
                        1.,
                        0.
                    ],
                    "normal",
                    "backwards",
                    transition.timing.to_string(),
                    [
                        [0., name, transition.from, "linear"],
                        [1., name, transition.to, "linear"]
                    ],
                ]));
            }
        }
        for &node in nodes {
            for (effect, clock) in self.pairs(node, styles) {
                if clock.phase == Phase::Canceled
                    || effect.pseudo != 0 && !pseudos
                    || clock.phase == Phase::Ended && effect.fill & 2 == 0
                {
                    continue;
                }
                let paused = clock.held.unwrap_or(effect.paused);
                let length = effect.active();
                let time = clock.playback.at(now).min(effect.delay + length);
                let frames: Vec<_> = effect
                    .tracks
                    .iter()
                    .flat_map(|(prop, keys)| {
                        keys.iter().map(move |frame| {
                            serde_json::json!([
                                frame.at,
                                prop.name(),
                                frame.value,
                                frame.timing.as_ref().unwrap_or(&effect.timing).to_string()
                            ])
                        })
                    })
                    .collect();
                let (pseudo, name) = clock.name.split_once('\0').unwrap_or(("", &clock.name));
                list.push(serde_json::json!([
                    clock.serial,
                    node,
                    "animation",
                    name,
                    time * 1000.,
                    if clock.phase == Phase::Ended {
                        "finished"
                    } else if paused {
                        "paused"
                    } else {
                        "running"
                    },
                    [
                        effect.delay * 1000.,
                        0.,
                        effect.duration * 1000.,
                        if effect.iterations.is_finite() {
                            effect.iterations
                        } else {
                            -1.
                        },
                        0.
                    ],
                    DIRECTIONS[usize::from(effect.direction)],
                    FILLS[usize::from(effect.fill)],
                    "linear",
                    frames,
                    pseudo,
                ]));
            }
        }
        list
    }

    pub fn css_control(
        &mut self,
        serial: u32,
        action: &str,
        time: f64,
        styles: &ComputedStyles,
    ) -> bool {
        let now = Instant::now();
        for (&node, clocks) in &mut self.clocks {
            let Some(index) = clocks.iter().position(|clock| clock.serial == serial) else {
                continue;
            };
            let clock = &mut clocks[index];
            let effect = styles
                .animations
                .get(&node)
                .and_then(|effects| effects.get(index))
                .filter(|effect| effect.name == clock.name);
            let rate = |clock: &Clock| {
                f64::from(u8::from(
                    !clock
                        .held
                        .unwrap_or(effect.is_some_and(|effect| effect.paused)),
                ))
            };
            clock.playback.set(now, rate(clock));
            let end = effect.map_or(0., |effect| effect.delay + effect.active());
            match action {
                "pause" => clock.held = Some(true),
                "play" => {
                    if matches!(clock.phase, Phase::Ended | Phase::Canceled) {
                        clock.playback.time = 0.;
                        (clock.phase, clock.iteration) = (Phase::Pending, 0.);
                    }
                    clock.held = Some(false);
                }
                "seek" => clock.playback.time = time / 1000.,
                "finish" if end.is_finite() => clock.playback.time = end,
                "cancel" => {
                    if clock.phase == Phase::Running {
                        let active = (clock.playback.time - clock.delay).max(0.);
                        self.queued
                            .push((node, "animationcancel", clock.name.clone(), active));
                    }
                    clock.phase = Phase::Canceled;
                }
                _ => return false,
            }
            clock.playback.rate = rate(clock);
            return true;
        }
        for (&node, list) in &mut self.transitions {
            let Some(index) = list
                .iter()
                .position(|transition| transition.serial == serial)
            else {
                continue;
            };
            let transition = &mut list[index];
            let rate = transition.playback.rate;
            transition.playback.set(now, rate);
            let elapsed = transition.playback.time;
            match action {
                "pause" => transition.playback.rate = 0.,
                "play" => transition.playback.rate = 1.,
                "seek" => transition.playback.time = time / 1000.,
                "finish" | "cancel" => {
                    let transition = list.remove(index);
                    let (kind, time) = if action == "finish" {
                        ("transitionend", transition.duration)
                    } else {
                        (
                            "transitioncancel",
                            (elapsed - transition.delay).clamp(0., transition.duration),
                        )
                    };
                    self.queued
                        .push((node, kind, transition.property.name().to_owned(), time));
                }
                _ => return false,
            }
            return true;
        }
        false
    }

    pub fn backdrop(&self, id: usize, base: &Style) -> Option<Style> {
        let now = Instant::now();
        let mut style = None;
        for transition in self
            .backdrop
            .get(&id)
            .into_iter()
            .flat_map(|(_, list)| list)
        {
            if let Some(progress) = transition.progress(now) {
                style
                    .get_or_insert_with(|| base.clone())
                    .insert(transition.property, transition.value(progress));
            }
        }
        style
    }

    pub fn scripted(
        &mut self,
        node: usize,
        pseudo: u8,
        styles: &ComputedStyles,
        upto: u32,
    ) -> Option<Style> {
        let now = Instant::now();
        let list = self.scripted.get_mut(&node)?;
        let mut out = Style::new();
        for animation in list
            .iter_mut()
            .take_while(|animation| animation.id <= upto)
            .filter(|animation| animation.effect.pseudo == pseudo)
        {
            animation.sample(node, styles, now, || &mut out);
        }
        if pseudo == 0 {
            drop_important(styles, node, &mut out);
        }
        (!out.is_empty()).then_some(out)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        doc: &Document,
        styles: &ComputedStyles,
        visible: &[bool],
        shown: &dyn Fn(usize) -> bool,
        now: Instant,
        repass: bool,
        events: &mut Events,
    ) -> (Animated, Option<Instant>) {
        let registry = &*styles.registry;
        events.append(&mut self.queued);
        let mut revealed = Vec::new();
        for (&id, list) in &mut self.scripted {
            if visible.get(id) != Some(&false) {
                continue;
            }
            let mut out = Style::new();
            for animation in list.iter_mut().filter(|animation| {
                animation.effect.pseudo == 0
                    && animation
                        .rules
                        .iter()
                        .any(|(_, rule, _)| rule.contains(Prop::DISPLAY))
            }) {
                animation.sample(id, styles, now, || &mut out);
            }
            if out.get(Prop::DISPLAY).is_some_and(|value| value != "none") {
                revealed.push(id);
            }
        }
        let shows = |transitions: &FxHashMap<usize, Vec<Transition>>, id: usize| {
            revealed.contains(&id)
                || transitions
                    .get(&id)
                    .is_some_and(|list| list.iter().any(|transition| transition.shows(now)))
        };
        let mut painted = Cow::Borrowed(visible);
        for &id in self.transitions.keys().chain(&revealed) {
            if shows(&self.transitions, id) {
                show(
                    &mut painted,
                    doc,
                    styles,
                    &|id| shows(&self.transitions, id),
                    id,
                );
            }
        }
        let hiding = |painted: &[bool], id: usize, style: &Style| {
            style.get(Prop::DISPLAY) == Some("none")
                && doc
                    .nodes
                    .get(id)
                    .and_then(|node| node.parent)
                    .is_none_or(|parent| painted.get(parent) == Some(&true))
        };
        self.previous
            .resize_with(styles.values.len(), Default::default);
        let (mut exits, mut changed) = (Vec::new(), Vec::new());
        let mut visit: Vec<usize> = std::mem::take(&mut self.touched);
        visit.retain(|&id| id < styles.values.len());
        visit.extend(styles.values.differing(&self.seen).flatten());
        visit.sort_unstable();
        visit.dedup();
        self.seen = styles.values.clone();
        for id in visit {
            let new = &styles.values[id];
            if Rc::ptr_eq(&self.previous[id], new) {
                continue;
            }
            if TRANSITIONS.iter().any(|&prop| new.contains(prop))
                && new.get(Prop::DISPLAY) == Some("none")
            {
                exits.push(id);
            } else {
                changed.push(id);
            }
        }
        // A child can have a smaller id than its parent, so exits sort by depth to start a parent's exit
        // first.
        exits.sort_by_cached_key(|&id| {
            std::iter::successors(doc.nodes[id].parent, |&parent| doc.nodes[parent].parent).count()
        });
        for id in exits.into_iter().chain(changed) {
            let new = &styles.values[id];
            let old = std::mem::replace(&mut self.previous[id], new.clone());
            // A container query pass recomputes styles inside one frame, so it must not start transitions.
            if repass {
                continue;
            }
            let running = self.transitions.remove(&id).unwrap_or_default();
            let cancel = |kept: &[Transition], events: &mut Events| {
                cancel(&running, kept, (id, ""), now, events)
            };
            let exiting = running
                .iter()
                .any(|transition| transition.property == Prop::DISPLAY);
            let old = match styles.starting.get(&id) {
                Some(starting)
                    if old.is_empty()
                        || !exiting
                            && old.get(Prop::DISPLAY).is_some_and(|value| value == "none") =>
                {
                    starting.clone()
                }
                _ => old,
            };
            let shown = painted.get(id) == Some(&true);
            if old.is_empty()
                || !shown && !hiding(&painted, id, new)
                || !exiting && old.get(Prop::DISPLAY).is_some_and(|value| value == "none")
                || !TRANSITIONS.iter().any(|&prop| new.contains(prop))
            {
                cancel(&[], events);
                continue;
            }
            let (mut next, mut runs) = retarget(
                &old,
                new,
                &running,
                registry,
                now,
                &mut self.serial,
                (id, ""),
            );
            if !shown
                && !next
                    .iter()
                    .any(|transition| transition.property == Prop::DISPLAY)
            {
                next.clear();
                runs.clear();
            }
            cancel(&next, events);
            events.append(&mut runs);
            if !next.is_empty() {
                let exit = next.iter().any(|transition| transition.shows(now));
                self.transitions.insert(id, next);
                if exit {
                    show(
                        &mut painted,
                        doc,
                        styles,
                        &|id| shows(&self.transitions, id),
                        id,
                    );
                }
            }
        }
        let mut gone = std::mem::take(&mut self.backdrop);
        for (&id, new) in &styles.backdrops {
            let (old, running) = match gone.remove(&id) {
                Some((old, running)) => (Some(old), running),
                None => (styles.backdrop_starts.get(&id).cloned(), Vec::new()),
            };
            let next = match old {
                Some(old) if !repass && !Rc::ptr_eq(&old, new) => {
                    let (next, mut runs) = retarget(
                        &old,
                        new,
                        &running,
                        registry,
                        now,
                        &mut self.serial,
                        (id, "::backdrop\0"),
                    );
                    cancel(&running, &next, (id, "::backdrop\0"), now, events);
                    events.append(&mut runs);
                    next
                }
                _ => running,
            };
            self.backdrop.insert(id, (new.clone(), next));
        }
        for (id, (_, running)) in gone {
            cancel(&running, &[], (id, "::backdrop\0"), now, events);
        }
        let cancel_clock = |id: usize, clock: &Clock, events: &mut Events| {
            if clock.phase == Phase::Running {
                events.push((
                    id,
                    "animationcancel",
                    clock.name.clone(),
                    (clock.playback.at(now) - clock.delay).max(0.),
                ));
            }
        };
        self.clocks.retain(|&id, clocks| {
            let kept = painted.get(id) == Some(&true) && styles.animations.contains_key(&id);
            if !kept {
                for clock in clocks.iter() {
                    cancel_clock(id, clock, events);
                }
            }
            kept
        });
        self.transitions.retain(|&id, list| {
            let kept = painted.get(id) == Some(&true)
                || list
                    .iter()
                    .any(|transition| transition.property == Prop::DISPLAY)
                    && styles
                        .values
                        .get(id)
                        .is_some_and(|style| hiding(&painted, id, style));
            if !kept {
                cancel(list, &[], (id, ""), now, events);
            }
            kept
        });
        let mut animated: FxHashMap<usize, Style> = FxHashMap::default();
        animated.reserve(self.styled.len());
        let mut pseudo: FxHashMap<(usize, u8), Style> = FxHashMap::default();
        let mut wakes: Vec<Wake> = Vec::new();
        for (&id, effects) in &styles.animations {
            if painted.get(id) != Some(&true) {
                continue;
            }
            let clocks = self.clocks.entry(id).or_default();
            if clocks.len() != effects.len()
                || clocks
                    .iter()
                    .zip(effects.iter())
                    .any(|(clock, effect)| clock.name != effect.name)
            {
                let mut old = std::mem::take(clocks);
                for effect in effects.iter().rev() {
                    clocks.push(
                        match old.iter().rposition(|clock| clock.name == effect.name) {
                            Some(index) => old.remove(index),
                            None => {
                                self.serial += 1;
                                Clock {
                                    serial: self.serial,
                                    name: effect.name.clone(),
                                    playback: Playback {
                                        time: 0.,
                                        since: now,
                                        rate: f64::from(u8::from(!effect.paused)),
                                    },
                                    phase: Phase::Pending,
                                    iteration: 0.,
                                    delay: effect.delay,
                                    held: None,
                                }
                            }
                        },
                    );
                }
                clocks.reverse();
                for clock in &old {
                    cancel_clock(id, clock, events);
                }
            }
            for (effect, clock) in effects.iter().zip(clocks.iter_mut()) {
                let paused = clock.held.unwrap_or(effect.paused);
                if paused != (clock.playback.rate == 0.) {
                    clock.playback.set(now, f64::from(u8::from(!paused)));
                }
                clock.delay = effect.delay;
                if clock.phase == Phase::Canceled {
                    continue;
                }
                let elapsed = clock.playback.at(now);
                let (base, out) = match effect.pseudo {
                    0 => (&*styles.values[id], animated.entry(id).or_default()),
                    slot => {
                        let Some(pair) = styles.pseudos.get(&id) else {
                            continue;
                        };
                        (
                            &pair[usize::from(slot) - 1],
                            pseudo.entry((id, slot)).or_default(),
                        )
                    }
                };
                let (wake, ended) = effect.sample(elapsed, paused, base, registry, out);
                let active = elapsed - effect.delay;
                let iteration = if effect.duration > 0. {
                    (active / effect.duration).floor().max(0.)
                } else {
                    0.
                };
                if clock.phase == Phase::Pending && active >= 0. {
                    clock.phase = Phase::Running;
                    clock.iteration = iteration;
                    events.push((
                        id,
                        "animationstart",
                        effect.name.clone(),
                        (-effect.delay).min(effect.active()).max(0.),
                    ));
                }
                if clock.phase != Phase::Pending && effect.duration > 0. && !ended {
                    if iteration > clock.iteration && self.iterations {
                        events.push((
                            id,
                            "animationiteration",
                            effect.name.clone(),
                            iteration * effect.duration,
                        ));
                    }
                    clock.iteration = iteration;
                }
                if ended {
                    if clock.phase != Phase::Ended {
                        events.push((id, "animationend", effect.name.clone(), effect.active()));
                    }
                    clock.phase = Phase::Ended;
                } else if clock.phase == Phase::Ended {
                    clock.phase = Phase::Running;
                }
                let end = if paused || ended {
                    f64::INFINITY
                } else {
                    effect.delay + effect.active() - elapsed
                };
                let (paint, still) = effect.paints();
                wakes.push((
                    id,
                    paint,
                    still,
                    if effect.tracks.is_empty() {
                        end
                    } else {
                        wake.unwrap_or(f64::INFINITY)
                    },
                    end,
                ));
            }
        }
        for (&id, list) in &mut self.scripted {
            if painted.get(id) != Some(&true) {
                continue;
            }
            for animation in list.iter_mut() {
                let slot = animation.effect.pseudo;
                let Some(wake) = animation.sample(id, styles, now, || match slot {
                    0 => animated.entry(id).or_default(),
                    slot => pseudo.entry((id, slot)).or_default(),
                }) else {
                    continue;
                };
                let rate = animation.playback.rate;
                if rate == 0. {
                    continue;
                }
                let change = if rate > 0. {
                    wake.map_or(f64::INFINITY, |wake| wake / rate)
                } else if animation.playback.at(now) > 0. {
                    0.
                } else {
                    f64::INFINITY
                };
                let (paint, still) = animation.effect.paints();
                wakes.push((id, paint, still, change, f64::INFINITY));
            }
        }
        // Important declarations override animations, but not transitions.
        for (&id, out) in &mut animated {
            drop_important(styles, id, out);
        }
        for (&id, list) in &mut self.transitions {
            let out = animated.entry(id).or_default();
            advance(list, (id, ""), now, out, events, &mut wakes);
        }
        self.backdrops.clear();
        for (&id, (base, list)) in &mut self.backdrop {
            if !list.is_empty() {
                let mut out = (**base).clone();
                advance(
                    list,
                    (id, "::backdrop\0"),
                    now,
                    &mut out,
                    events,
                    &mut wakes,
                );
                if !list.is_empty() {
                    self.backdrops.push((id, Rc::new(out)));
                }
            }
        }
        self.transitions.retain(|_, list| !list.is_empty());
        // Drop layout values equal to the base to avoid layout on every animation frame.
        animated.retain(|&id, values| {
            values.retain(|key, value| {
                PAINT.contains(&key) || styles.values[id].value(key) != Some(&*value)
            });
            !values.is_empty()
        });
        for (&id, values) in &animated {
            if values.get(Prop::DISPLAY) == Some("none") && painted.get(id) == Some(&true) {
                let painted = painted.to_mut();
                let mut pending = vec![id];
                while let Some(id) = pending.pop() {
                    if painted.get(id) == Some(&true) {
                        painted[id] = false;
                        pending.extend(styles.children.get(&id).unwrap_or(&doc.nodes[id].children));
                    }
                }
            }
        }
        self.custom.clear();
        if animated.values().any(|values| {
            !values.vars().is_empty()
                || values.contains(Prop::FONT_SIZE)
                || values.contains(Prop::LINE_HEIGHT)
        }) {
            self.custom = animated
                .iter()
                .map(|(&id, values)| (id, values.clone()))
                .collect();
        }
        let mut inherited = Vec::new();
        for (&id, values) in &animated {
            for (key, value) in values.iter() {
                if !key.inherited() {
                    continue;
                }
                // An inherited value is the parent's own string. An equal value that the child declares is
                // another one, and must not follow the parent.
                let base = styles.values[id].value(key);
                let mut pending = vec![id];
                while let Some(parent) = pending.pop() {
                    for &child in styles
                        .children
                        .get(&parent)
                        .unwrap_or(&doc.nodes[parent].children)
                    {
                        let own = styles.values[child].value(key);
                        if own.map(Rc::as_ptr) == base.map(Rc::as_ptr)
                            && animated
                                .get(&child)
                                .is_none_or(|values| !values.contains(key))
                        {
                            inherited.push((child, key, value.clone()));
                            pending.push(child);
                        }
                    }
                }
            }
        }
        for (id, key, value) in inherited {
            animated.entry(id).or_default().insert(key, value);
        }
        let mut layout: FxHashSet<usize> = animated
            .iter()
            .filter(|(id, values)| {
                values.iter().any(|(key, _)| {
                    !PAINT.contains(&key)
                        || key != Prop::COLOR
                            && styles.values[**id]
                                .get(Prop::DISPLAY)
                                .is_none_or(|value| matches!(value, "inline" | "contents"))
                })
            })
            .map(|(id, _)| *id)
            .collect();
        let fixed =
            |id: usize, key: Prop| animated.get(&id).is_none_or(|values| !values.contains(key));
        let mut next = f64::INFINITY;
        self.parked.clear();
        for (id, paint, still, change, end) in wakes {
            let unseen = painted.get(id) != Some(&true)
                && animated
                    .get(&id)
                    .is_none_or(|values| values.get(Prop::DISPLAY) != Some("none"))
                || paint && {
                    let mut faded = false;
                    let mut chain = Some(id);
                    while let Some(node) = chain {
                        faded |= fixed(node, Prop::OPACITY)
                            && styles.values[node].get(Prop::OPACITY).is_some_and(|value| {
                                value.trim_end_matches('%').parse::<f32>() == Ok(0.)
                            });
                        chain = doc.nodes[node].parent.filter(|_| !faded);
                    }
                    let mut hidden = true;
                    let mut pending = vec![id];
                    while hidden && let Some(node) = pending.pop() {
                        let tag = &doc.nodes[node].tag;
                        if tag.starts_with('#') && tag != "#text" {
                            continue;
                        }
                        hidden = fixed(node, Prop::VISIBILITY)
                            && styles.values[node]
                                .get(Prop::VISIBILITY)
                                .is_some_and(|value| matches!(value, "hidden" | "collapse"));
                        pending.extend(
                            styles
                                .children
                                .get(&node)
                                .unwrap_or(&doc.nodes[node].children),
                        );
                    }
                    faded || hidden
                };
            // Parked animations resume only because render checks parked boxes after layout, and scrolls,
            // resizes and DOM changes draw a frame.
            let parked = !unseen && still && change < end && !shown(id);
            if parked && self.parked.last() != Some(&id) {
                self.parked.push(id);
            }
            next = next.min(if unseen || parked { end } else { change });
        }
        self.painted = match painted {
            Cow::Owned(painted) => Some(painted),
            Cow::Borrowed(_) => None,
        };
        let mut pairs: FxHashMap<usize, [Style; 2]> = FxHashMap::default();
        let mut only = Vec::new();
        for ((id, slot), values) in pseudo {
            let Some(pair) = styles.pseudos.get(&id).filter(|_| !values.is_empty()) else {
                continue;
            };
            if values.iter().any(|(key, _)| !PAINT.contains(&key)) {
                layout.insert(id);
            }
            pairs.entry(id).or_insert_with(|| {
                if !animated.contains_key(&id) {
                    only.push(id);
                }
                (**pair).clone()
            })[usize::from(slot) - 1]
                .extend(values.iter().map(|(key, value)| (key, value.clone())));
        }
        self.styled.retain(|id, _| animated.contains_key(id));
        let mut animated: Animated = animated
            .into_iter()
            .map(|(id, values)| {
                let base = &styles.values[id];
                let style = match self.styled.get_mut(&id) {
                    Some((old, sampled, style))
                        if Rc::ptr_eq(old, base)
                            && sampled
                                .iter()
                                .map(|(key, _)| key)
                                .eq(values.iter().map(|(key, _)| key)) =>
                    {
                        if *sampled != values {
                            let next = Rc::make_mut(style);
                            for (key, value) in values.iter() {
                                next.insert(key, value.clone());
                            }
                            *sampled = values;
                        }
                        style.clone()
                    }
                    _ => {
                        let mut style = (**base).clone();
                        style.extend(values.iter().map(|(key, value)| (key, value.clone())));
                        let style = Rc::new(style);
                        self.styled
                            .insert(id, (base.clone(), values, style.clone()));
                        style
                    }
                };
                (id, style, layout.contains(&id))
            })
            .collect();
        animated.extend(
            only.into_iter()
                .map(|id| (id, styles.values[id].clone(), layout.contains(&id))),
        );
        self.pseudos = pairs
            .into_iter()
            .map(|(id, pair)| (id, Rc::new(pair)))
            .collect();
        (animated, after(now, next))
    }
}

fn add(property: &str, under: &str, value: &str, accumulate: bool) -> Option<String> {
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
                super::value::number(a + b - offset),
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
            super::value::number(alpha)
        ));
    }
    let [a, b] = [under, value].map(|list| components(list, ' '));
    if a.len() != b.len() {
        return None;
    }
    let parts: Option<Vec<String>> = a.iter().zip(&b).map(|(a, b)| sum(a, b, 0.)).collect();
    Some(parts?.join(" "))
}

fn keeps_box_in_place<'a>(prop: Prop, mut values: impl Iterator<Item = &'a str>) -> bool {
    match prop {
        Prop::TRANSLATE => false,
        Prop::TRANSFORM => values.all(|value| {
            !["translate", "matrix", "perspective"]
                .iter()
                .any(|name| value.contains(name))
        }),
        _ => PAINT.contains(&prop),
    }
}

fn animatable(prop: Prop, registry: &Registry) -> bool {
    let key = prop.name();
    if prop.is_custom() {
        return registry.get(&prop).is_some_and(|(syntax, ..)| {
            syntax.split('|').all(|kind| {
                matches!(
                    kind.trim().trim_end_matches(['+', '#']),
                    "<length>"
                        | "<number>"
                        | "<percentage>"
                        | "<length-percentage>"
                        | "<color>"
                        | "<angle>"
                        | "<time>"
                )
            })
        });
    }
    !key.starts_with("animation")
        && !key.starts_with("transition")
        && !key.starts_with("container")
        && key != "will-change"
}

fn computed<'a>(style: &'a Style, prop: Prop, registry: &'a Registry) -> Option<&'a str> {
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

fn transitions(style: &Style) -> Vec<(String, f64, f64, Easing, bool)> {
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

fn lerp(property: &str, from: &str, to: &str) -> Option<Rc<Lerp>> {
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
        if !lerps
            .get(&key)
            .is_some_and(|(p, a, b, _)| p == property && a == from && b == to)
        {
            if lerps.len() >= 1024 {
                lerps.clear();
            }
            let lerp = Lerp::new(property, from, to).map(Rc::new);
            lerps.insert(key, (property.into(), from.into(), to.into(), lerp));
        }
        lerps[&key].3.clone()
    })
}

fn interpolate(lerp: Option<&Lerp>, from: &str, to: &str, t: f64) -> Option<Rc<str>> {
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

type Lerps = rustc_hash::FxHashMap<u64, (String, String, String, Option<Rc<Lerp>>)>;

#[derive(Debug)]
enum Lerp {
    Same(String),
    Between(String, String, String),
    List(Vec<Lerp>, &'static str),
    Function(String, Box<Lerp>),
    Color((u32, u8), (u32, u8)),
    Oklab([f64; 4], [f64; 4]),
    Number(f64, f64, String, bool),
    Calc(String, String, bool),
    Matrix(Box<[[f64; 13]; 2]>, bool),
}

const IDENTITY: [f64; 16] = [
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
    let units = super::Lengths {
        font: super::FontUnits::new(f32::NAN),
        root: super::FontUnits::new(f32::NAN),
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
                Some(if kind == super::parser::Kind::Angle {
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
    fn new(property: &str, from: &str, to: &str) -> Option<Self> {
        if from == to {
            return Some(Self::Same(from.to_owned()));
        }
        // Chrome switches images, gradients too, halfway instead of blending them.
        if property.ends_with("-image") || property == "border-image-source" {
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
        if let (Some(a), Some(b)) = (color(from), color(to)) {
            // As in browsers, colors interpolate in sRGB only when both are legacy sRGB colors.
            return Some(match (super::color::oklab(from), super::color::oklab(to)) {
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
            });
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
            let sides = |args: &str| {
                let (box_, round) = args.split_once(" round ").unwrap_or((args, ""));
                let sides = components(box_, ' ');
                if !(1..=4).contains(&sides.len()) {
                    return args.to_owned();
                }
                let order = [[0, 0, 0, 0], [0, 1, 0, 1], [0, 1, 2, 1], [0, 1, 2, 3]];
                let mut out = order[sides.len() - 1].map(|index| sides[index]).join(" ");
                if !round.is_empty() {
                    out.push_str(" round ");
                    out.push_str(round);
                }
                out
            };
            let [a, b] = if name == "inset" && components(a, ' ').len() != components(b, ' ').len()
            {
                [sides(a), sides(b)].map(Cow::Owned)
            } else {
                [a, b].map(Cow::Borrowed)
            };
            return Self::new(property, &a, &b)
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

    fn write(&self, t: f64, out: &mut String) {
        use std::fmt::Write;
        let round = |value: f64, integer: bool, out: &mut String| {
            if integer {
                let _ = write!(out, "{}", (value + 0.5).floor());
            } else {
                super::value::write_number(value, out);
            }
        };
        match self {
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

fn timing_function(part: &str) -> bool {
    matches!(
        part,
        "ease" | "linear" | "ease-in" | "ease-out" | "ease-in-out" | "step-start" | "step-end"
    ) || part.starts_with("cubic-bezier(")
        || part.starts_with("steps(")
        || part.starts_with("linear(")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Document;
    use std::time::Duration;

    fn easing(value: &str, progress: f64) -> f64 {
        Easing::parse(value).unwrap().at(progress, false)
    }

    fn computed_styles(doc: &Document) -> ComputedStyles {
        super::super::compute_styles(
            doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut super::super::StyleCache::default(),
            None,
        )
    }

    #[test]
    fn keyframes_follow_delay_easing_iterations_and_fill() {
        let doc = Document::parse(
            "<style>@keyframes pulse{from{opacity:1}50%{opacity:0}to{opacity:1}}div{animation:1.2s .8s infinite pulse}</style><div></div>",
        );
        let styles = computed_styles(&doc);
        let id = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
        let base = styles.values[id].clone();
        let sample = |effect: &Animation, elapsed: f64| {
            let mut out = Style::new();
            let (wake, _) =
                effect.sample(elapsed, effect.paused, &base, &Default::default(), &mut out);
            (
                out.get(Prop::OPACITY)
                    .map(|value| value.parse::<f64>().unwrap()),
                wake.is_some(),
            )
        };
        let effect = &styles.animations[&id][0];
        assert_eq!(sample(effect, 0.4), (None, true));
        assert_eq!(sample(effect, 0.8), (Some(1.), true));
        assert!(sample(effect, 1.4).0.unwrap() < 0.0001);
        assert!((sample(effect, 1.1).0.unwrap() - 0.1976).abs() < 0.001);
        assert!((sample(effect, 2.3).0.unwrap() - sample(effect, 1.1).0.unwrap()).abs() < 0.001);
        let mut finite = effect.clone();
        finite.duration = 1.;
        finite.delay = -0.5;
        finite.iterations = 2.;
        finite.direction = 2;
        finite.fill = 3;
        assert_eq!(sample(&finite, 0.).0, Some(0.));
        assert_eq!(sample(&finite, 2.), (Some(1.), false));
        finite.paused = true;
        assert!(!sample(&finite, 0.).1);
        finite.duration = 0.;
        assert!(!sample(&finite, 0.).1);
        finite.iterations = f64::INFINITY;
        assert_eq!(sample(&finite, 0.), (Some(1.), false));
    }

    #[test]
    fn keyframes_animate_many_properties_with_per_keyframe_easing() {
        let doc = Document::parse(
            "<style>@keyframes move{from{transform:translateX(0) rotate(0deg);background:red}50%{width:50%;animation-timing-function:linear}to{transform:translateX(100px) rotate(90deg);background:blue;width:100px}}@keyframes spin{to{transform:rotate(360deg)}}div{width:10px;animation:move 2s linear}p{animation:spin 1s linear infinite}</style><div></div><p></p>",
        );
        let styles = computed_styles(&doc);
        let div = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
        let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
        let mut out = Style::new();
        let sample = |id: usize, elapsed: f64, out: &mut Style| {
            styles.animations[&id][0].sample(
                elapsed,
                false,
                &styles.values[id],
                &Default::default(),
                out,
            )
        };
        sample(div, 1., &mut out);
        assert_eq!(&out["transform"], "translateX(50px) rotate(45deg)");
        assert_eq!(&out["background-color"], "#800080ff");
        assert_eq!(&out["width"], "50%");
        out = Style::new();
        sample(div, 0.5, &mut out);
        assert_eq!(&out["width"], "calc(10px * 0.5 + 50% * 0.5)");
        out = Style::new();
        sample(p, 0.25, &mut out);
        assert_eq!(&out["transform"], "rotate(90deg)");
    }

    #[test]
    fn values_interpolate_by_type() {
        let mix = |property: &str, from: &str, to: &str, t: f64| {
            interpolate(lerp(property, from, to).as_deref(), from, to, t).map(|v| v.to_string())
        };
        assert_eq!(mix("color", "red", "blue", 0.5).unwrap(), "#800080ff");
        assert_eq!(
            mix("color", "transparent", "red", 0.5).unwrap(),
            "#ff000080"
        );
        assert_eq!(
            mix("color", "oklch(0.205 0 0)", "oklch(0.922 0 0)", 0.5).unwrap(),
            "oklab(0.5635 0 0)"
        );
        assert_eq!(
            mix("color", "transparent", "oklab(1 0 0 / 0.04)", 0.5).unwrap(),
            "oklab(1 0 0 / 0.02)"
        );
        assert_eq!(
            mix("color", "#000", "oklab(1 none 0)", 0.5).unwrap(),
            "oklab(0.5 0 0)"
        );
        assert_eq!(mix("width", "0", "10px", 0.5).unwrap(), "5px");
        assert_eq!(mix("opacity", "1", "0", 0.25).unwrap(), "0.75");
        assert_eq!(mix("opacity", "0%", "1", 0.5).unwrap(), "0.5");
        assert_eq!(
            mix("scale", "50% 50%", "100% 100%", 0.5).unwrap(),
            "0.75 0.75"
        );
        assert_eq!(mix("z-index", "1", "4", 0.4).unwrap(), "2");
        assert_eq!(mix("width", "auto", "10px", 0.5), None);
        assert_eq!(
            mix("transform", "none", "scale(2) rotate(1turn)", 0.5).unwrap(),
            "scale(1.5) rotate(0.5turn)"
        );
        assert_eq!(
            mix("translate", "none", "10px 20px", 0.5).unwrap(),
            "5px 10px"
        );
        assert_eq!(
            mix("transform", "rotate(10deg)", "scale(2)", 0.5).unwrap(),
            "matrix(1.49429, 0.130734, -0.130734, 1.49429, 0, 0)"
        );
        assert_eq!(
            mix(
                "transform",
                "translateY(10px) scale(0.95)",
                "translateY(0)",
                0.5
            )
            .unwrap(),
            "translateY(5px) scale(0.975)"
        );
        assert_eq!(
            mix(
                "transform",
                "translateX(10px)",
                "translate(0px, 20px) rotate(90deg)",
                0.5
            )
            .unwrap(),
            "translate(5px, 10px) rotate(45deg)"
        );
        assert_eq!(
            mix(
                "transform",
                "rotateX(12deg) skewX(4deg)",
                "skewX(2deg)",
                0.5
            )
            .unwrap(),
            "matrix3d(1, 0, 0, 0, 0.0524238, 0.994522, 0.104528, 0, 0, -0.104528, 0.994522, 0, 0, 0, 0, 1)"
        );
        assert_eq!(
            mix("transform", "translateX(10%)", "rotate(1deg)", 0.5),
            None
        );
        assert_eq!(
            mix("transform", "  ", " rotateX(10deg)   skewX(4deg) ", 0.5).unwrap(),
            "rotateX(5deg) skewX(2deg)"
        );
        assert!(
            mix(
                "transform",
                "  rotateZ(45deg)  ",
                "rotateX(10deg) rotateZ(90deg) skewX(2deg)",
                0.5
            )
            .is_some_and(|value| value.starts_with("matrix3d("))
        );
        assert_eq!(mix("display", "flex", "none", 0.9).unwrap(), "flex");
        assert_eq!(mix("display", "flex", "block", 0.4), None);
        assert_eq!(mix("font-size", "16px", "32px", 0.25).unwrap(), "20px");
        let gradients = [
            "linear-gradient(red, blue)",
            "linear-gradient(green, yellow)",
        ];
        assert_eq!(
            mix("background-image", gradients[0], gradients[1], 0.3),
            None
        );
        assert_eq!(
            mix("box-shadow", "none", "0 0 10px red", 0.5).unwrap(),
            "0px 0px 5px 0px #ff000080"
        );
        assert_eq!(
            mix("background-position", "0% 0%", "100% 50%", 0.5).unwrap(),
            "50% 25%"
        );
        assert_eq!(
            mix("visibility", "hidden", "visible", 0.1).unwrap(),
            "visible"
        );
        let units = super::super::Lengths::new(16., 800., 600.);
        assert_eq!(
            units.resolve(&mix("left", "10px", "50%", 0.5).unwrap(), 200.),
            Some(55.)
        );
        assert_eq!(
            units.resolve(&mix("left", "calc(100% - 10px)", "0px", 0.5).unwrap(), 200.),
            Some(95.)
        );
    }

    #[test]
    fn animated_styles_drop_values_that_stop_animating() {
        let doc = Document::parse(
            "<style>@keyframes w{from{width:0px}to{width:100px}}@keyframes h{from{height:0px}to{height:10px}}div{height:3px;animation:w 1s linear,h 0.5s linear}</style><div></div>",
        );
        let id = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
        let styles = computed_styles(&doc);
        let visible = vec![true; doc.nodes.len()];
        let start = Instant::now();
        let mut motion = Motion::default();
        let mut style = |seconds: f64| {
            let (animated, _) = motion.update(
                &doc,
                &styles,
                &visible,
                &|_| true,
                start + Duration::from_secs_f64(seconds),
                false,
                &mut Vec::new(),
            );
            let style = animated.into_iter().find(|entry| entry.0 == id).unwrap().1;
            [Prop::WIDTH, Prop::HEIGHT].map(|prop| style.get(prop).map(str::to_owned))
        };
        style(0.);
        assert_eq!(style(0.25), [Some("25px".into()), Some("5px".into())]);
        assert_eq!(style(0.3), [Some("30px".into()), Some("6px".into())]);
        assert_eq!(style(0.75), [Some("75px".into()), Some("3px".into())]);
    }

    #[test]
    fn transitions_start_reverse_and_finish() {
        let mut doc = Document::parse(
            "<style>div{color:blue;transition:color 1s linear,width 1s}div.on{color:red;width:20px}</style><div>text</div>",
        );
        let id = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
        let text = doc.nodes[id].children[0];
        let visible = vec![true; doc.nodes.len()];
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut motion = Motion::default();
        let mut events = Vec::new();
        let mut update =
            |motion: &mut Motion, doc: &Document, styles: &ComputedStyles, time: f64| {
                let (animated, next) = motion.update(
                    doc,
                    styles,
                    &visible,
                    &|_| true,
                    at(time),
                    false,
                    &mut events,
                );
                let running = next.is_some();
                let values: HashMap<usize, Rc<Style>> = animated
                    .into_iter()
                    .map(|(id, style, _)| (id, style))
                    .collect();
                (values, running, events.last().map(|event| event.1))
            };
        let (values, running, _) = update(&mut motion, &doc, &computed_styles(&doc), 0.);
        assert!(values.is_empty() && !running);
        doc.nodes[id].attrs.insert("class".into(), "on".into());
        let on = computed_styles(&doc);
        let (values, running, _) = update(&mut motion, &doc, &on, 0.);
        assert!(running);
        assert_eq!(&values[&id]["color"], "blue");
        assert_eq!(&values[&text]["color"], "blue");
        assert_eq!(&values[&id]["width"], "20px");
        let (values, ..) = update(&mut motion, &doc, &on, 0.5);
        assert_eq!(&values[&id]["color"], "#800080ff");
        doc.nodes[id].attrs.remove("class");
        let off = computed_styles(&doc);
        let (values, ..) = update(&mut motion, &doc, &off, 0.5);
        assert_eq!(&values[&id]["color"], "#800080ff");
        let (values, ..) = update(&mut motion, &doc, &off, 0.75);
        assert_eq!(&values[&id]["color"], "#4000c0ff");
        let (values, running, event) = update(&mut motion, &doc, &off, 1.);
        assert!(values.is_empty() && !running);
        assert_eq!(event, Some("transitionend"));
        let (_, running, _) = update(&mut motion, &doc, &on, 1.);
        assert!(running);
        motion.forget(id);
        let (values, ..) = update(&mut motion, &doc, &off, 1.);
        assert!(!values.contains_key(&id));
    }

    #[test]
    fn registered_custom_properties_animate_and_transition() {
        let mut doc = Document::parse(
            "<style>@property --w{syntax:'<length>';inherits:true;initial-value:0px}@property --c{syntax:'<color>';inherits:false;initial-value:red}@keyframes grow{to{--w:100px}}div{animation:grow 1s linear;width:var(--w)}span{display:block;width:var(--w)}p{--c:blue;--u:1px;transition:--c 1s linear,--u 1s;color:var(--c)}p.on{--c:lime;--u:9px}</style><div><span></span></div><p>x</p>",
        );
        let find =
            |doc: &Document, tag: &str| doc.nodes.iter().position(|node| node.tag == tag).unwrap();
        let (div, span, p) = (find(&doc, "div"), find(&doc, "span"), find(&doc, "p"));
        let visible = vec![true; doc.nodes.len()];
        let start = Instant::now();
        let mut motion = Motion::default();
        let styles = computed_styles(&doc);
        motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            start,
            false,
            &mut Vec::new(),
        );
        let at = start + Duration::from_millis(500);
        motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            at,
            false,
            &mut Vec::new(),
        );
        assert_eq!(motion.custom[&div].get(Prop::new("--w")), Some("50px"));
        let restyle = |doc: &Document, animated: HashMap<usize, Style>| {
            super::super::compute_styles(
                doc,
                800.,
                600.,
                None,
                &mut HashMap::new(),
                &mut super::super::StyleCache::default(),
                Some(&super::super::StyleContext {
                    animated,
                    ..Default::default()
                }),
            )
        };
        let restyled = restyle(&doc, motion.custom.clone());
        assert_eq!(restyled.values[div].get(Prop::WIDTH), Some("50px"));
        assert_eq!(restyled.values[span].get(Prop::WIDTH), Some("50px"));
        assert_eq!(styles.values[div].get(Prop::WIDTH), Some("0px"));
        doc.nodes[p].attrs.insert("class".into(), "on".into());
        let on = computed_styles(&doc);
        motion.update(&doc, &on, &visible, &|_| true, at, false, &mut Vec::new());
        motion.update(
            &doc,
            &on,
            &visible,
            &|_| true,
            at + Duration::from_millis(500),
            false,
            &mut Vec::new(),
        );
        let values = &motion.custom[&p];
        assert_eq!(values.get(Prop::new("--c")), Some("#008080ff"));
        assert_eq!(values.get(Prop::new("--u")), None);
        let restyled = restyle(&doc, motion.custom.clone());
        assert_eq!(restyled.values[p].get(Prop::COLOR), Some("#008080ff"));
    }

    #[test]
    fn starting_styles_transition_new_and_shown_elements() {
        let mut doc = Document::parse(
            "<style>p{opacity:1;transition:opacity 1s linear;@starting-style{opacity:0}}</style><p>a</p>",
        );
        let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
        let visible = vec![true; doc.nodes.len()];
        let start = Instant::now();
        let mut motion = Motion::default();
        let mut events = Vec::new();
        let styles = computed_styles(&doc);
        assert_eq!(styles.starting[&p].get(Prop::OPACITY), Some("0"));
        motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            start,
            false,
            &mut events,
        );
        let (animated, _) = motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            start + Duration::from_millis(500),
            false,
            &mut events,
        );
        let value = animated
            .iter()
            .find(|(id, ..)| *id == p)
            .map(|(_, style, _)| style.get(Prop::OPACITY).unwrap_or("").to_owned());
        assert_eq!(value.as_deref(), Some("0.5"), "{events:?}");
        doc.nodes[p]
            .attrs
            .insert("style".into(), "display:none".into());
        motion.update(
            &doc,
            &computed_styles(&doc),
            &visible,
            &|_| true,
            start + Duration::from_secs(2),
            false,
            &mut events,
        );
        doc.nodes[p].attrs.remove("style");
        motion.update(
            &doc,
            &computed_styles(&doc),
            &visible,
            &|_| true,
            start + Duration::from_secs(3),
            false,
            &mut events,
        );
        let (animated, _) = motion.update(
            &doc,
            &computed_styles(&doc),
            &visible,
            &|_| true,
            start + Duration::from_millis(3250),
            false,
            &mut events,
        );
        let value = animated
            .iter()
            .find(|(id, ..)| *id == p)
            .map(|(_, style, _)| style.get(Prop::OPACITY).unwrap_or("").to_owned());
        assert_eq!(value.as_deref(), Some("0.25"));
    }

    #[test]
    fn lifecycle_events_follow_delays_iterations_and_cancels() {
        let mut doc = Document::parse(
            "<style>@keyframes pulse{to{opacity:0}}div{width:10px;transition:width 1s .5s}div.on{width:20px}p{animation:pulse 1s 3 .25s}</style><div>t</div><p>p</p>",
        );
        let div = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
        let visible = vec![true; doc.nodes.len()];
        let start = Instant::now();
        let mut motion = Motion {
            iterations: true,
            ..Default::default()
        };
        let mut events = Vec::new();
        let mut run = |motion: &mut Motion, doc: &Document, time: f64| {
            events.clear();
            motion.update(
                doc,
                &computed_styles(doc),
                &visible,
                &|_| true,
                start + Duration::from_secs_f64(time),
                false,
                &mut events,
            );
            events
                .iter()
                .map(|event| (event.1, event.3))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(&mut motion, &doc, 0.), []);
        doc.nodes[div].attrs.insert("class".into(), "on".into());
        assert_eq!(run(&mut motion, &doc, 0.1), [("transitionrun", 0.)]);
        assert_eq!(run(&mut motion, &doc, 0.3), [("animationstart", 0.)]);
        assert_eq!(run(&mut motion, &doc, 0.7), [("transitionstart", 0.)]);
        assert_eq!(run(&mut motion, &doc, 1.3), [("animationiteration", 1.)]);
        doc.nodes[div].attrs.remove("class");
        let events = run(&mut motion, &doc, 1.4);
        assert!(
            events
                .iter()
                .any(|event| event.0 == "transitioncancel" && (event.1 - 0.8).abs() < 1e-6),
            "{events:?}"
        );
        let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
        doc.nodes[p]
            .attrs
            .insert("style".into(), "animation:none".into());
        assert!(
            run(&mut motion, &doc, 1.5)
                .iter()
                .any(|event| event.0 == "animationcancel")
        );
    }

    fn frames(html: &str, seconds: f64) -> (usize, Vec<Option<f64>>) {
        let doc = Document::parse(&format!(
            "<style>@keyframes spin{{to{{transform:rotate(360deg)}}}}@keyframes blink{{50%{{opacity:0}}}}@keyframes grow{{to{{width:10px}}}}@keyframes still{{to{{will-change:transform}}}}@keyframes show{{to{{opacity:1}}}}</style>{html}"
        ));
        let styles = computed_styles(&doc);
        let visible = vec![true; doc.nodes.len()];
        let start = Instant::now();
        let mut motion = Motion::default();
        let (mut frame, mut count, mut wakes) = (0, 0, Vec::new());
        while frame < (seconds * 120.).round() as u64 {
            let now = start + Duration::from_nanos(frame * 1_000_000_000 / 120);
            let (_, next) = motion.update(
                &doc,
                &styles,
                &visible,
                &|_| true,
                now,
                false,
                &mut Vec::new(),
            );
            count += 1;
            wakes.push(next.map(|next| (next - now).as_secs_f64()));
            let Some(next) = next else { break };
            frame =
                (frame + 1).max(((next - start).as_nanos() * 120).div_ceil(1_000_000_000) as u64);
        }
        (count, wakes)
    }

    #[test]
    fn animations_wake_the_view_only_when_their_output_can_change() {
        let close = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 1e-6);
        let (count, wakes) = frames("<p style='animation:spin 1s linear infinite'></p>", 1.);
        assert_eq!((count, wakes[0]), (120, Some(0.)));
        let (count, wakes) = frames(
            "<p style='animation:blink 1.2s steps(1) infinite'></p>",
            2.4,
        );
        assert_eq!(count, 4);
        assert!(wakes.iter().all(|&wake| close(wake, 0.6)));
        for hidden in [
            "<p style='visibility:hidden;animation:spin 1s linear infinite'></p>",
            "<div style='opacity:0'><p style='animation:spin 1s linear infinite'></p></div>",
            "<p style='animation:still 1s infinite'></p>",
            "<p style='animation:blink 1s infinite paused'></p>",
            "<p style='opacity:0;animation:spin 1s linear infinite'></p>",
        ] {
            assert_eq!(frames(hidden, 1.), (1, vec![None]), "{hidden}");
        }
        for shown in [
            "<div style='visibility:hidden;animation:spin 1s linear infinite'><p style='visibility:visible'></p></div>",
            "<p style='visibility:hidden;animation:grow 1s infinite'></p>",
            "<p style='opacity:0;animation:show 1s infinite'></p>",
        ] {
            assert_eq!(frames(shown, 0.1).1[0], Some(0.), "{shown}");
        }
        let (_, wakes) = frames(
            "<p style='visibility:hidden;animation:spin 2s linear'></p>",
            3.,
        );
        assert!(close(wakes[0], 2.) && wakes.last() == Some(&None));
        let (_, wakes) = frames("<p style='animation:spin 1s 2s linear'></p>", 0.1);
        assert!(close(wakes[0], 2.));
        let (count, wakes) = frames("<p style='animation:spin 1s linear forwards'></p>", 5.);
        assert_eq!((count, wakes.last()), (121, Some(&None)));
    }

    #[test]
    fn animations_of_unshown_boxes_wait_unless_they_move_the_box() {
        let page = |html: &str| {
            Document::parse(&format!(
                "<style>@keyframes spin{{to{{transform:rotate(360deg)}}}}@keyframes slide{{to{{transform:translateX(9px)}}}}@keyframes blink{{50%{{opacity:0}}}}@keyframes grow{{to{{width:10px}}}}</style>{html}"
            ))
        };
        let start = Instant::now();
        for (html, parked) in [
            ("<p style='animation:spin 1s linear infinite'></p>", true),
            ("<p style='animation:blink 1s infinite'></p>", true),
            ("<p style='animation:slide 1s linear infinite'></p>", false),
            ("<p style='animation:grow 1s infinite'></p>", false),
        ] {
            let doc = page(html);
            let styles = computed_styles(&doc);
            let visible = vec![true; doc.nodes.len()];
            let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
            let mut motion = Motion::default();
            let (_, next) = motion.update(
                &doc,
                &styles,
                &visible,
                &|_| false,
                start,
                false,
                &mut Vec::new(),
            );
            assert_eq!(
                (next.is_none(), motion.parked == [p]),
                (parked, parked),
                "{html}"
            );
        }
        let doc = page("<p style='animation:spin 1s linear infinite'></p>");
        let styles = computed_styles(&doc);
        let visible = vec![true; doc.nodes.len()];
        let mut motion = Motion::default();
        motion.update(
            &doc,
            &styles,
            &visible,
            &|_| false,
            start,
            false,
            &mut Vec::new(),
        );
        let later = start + Duration::from_millis(250);
        let (animated, next) = motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            later,
            false,
            &mut Vec::new(),
        );
        assert_eq!(next, Some(later));
        assert_eq!(&animated[0].1["transform"], "rotate(90deg)");
    }

    #[test]
    fn inherited_colors_animate_without_layout() {
        let mut doc = Document::parse(
            "<html><style>body{color:blue;transition:color 1s linear}body.on{color:red}span{transition:background-color 1s linear}span.on{background-color:red}</style><body><p>a <span>b</span></p></body></html>",
        );
        let find =
            |doc: &Document, tag: &str| doc.nodes.iter().position(|node| node.tag == tag).unwrap();
        let (body, span) = (find(&doc, "body"), find(&doc, "span"));
        let visible = vec![true; doc.nodes.len()];
        let start = Instant::now();
        let mut motion = Motion::default();
        let mut update = |doc: &Document, time: u64| {
            motion
                .update(
                    doc,
                    &computed_styles(doc),
                    &visible,
                    &|_| true,
                    start + Duration::from_millis(time),
                    false,
                    &mut Vec::new(),
                )
                .0
        };
        update(&doc, 0);
        doc.nodes[body].attrs.insert("class".into(), "on".into());
        update(&doc, 0);
        let animated = update(&doc, 500);
        assert_eq!(animated.len(), 5);
        assert!(
            animated
                .iter()
                .all(|(_, style, layout)| !layout && &style["color"] == "#800080ff")
        );
        doc.nodes[span].attrs.insert("class".into(), "on".into());
        update(&doc, 500);
        let animated = update(&doc, 750);
        assert!(
            animated
                .iter()
                .any(|(id, _, layout)| *id == span && *layout)
        );
    }

    #[test]
    fn inherited_transitions_skip_children_that_declare_the_value() {
        let mut doc = Document::parse(
            "<html><style>body{color:blue;transition:color 1s linear}body.on{color:red}p{color:red}</style><body><i>a</i><p>b</p></body></html>",
        );
        let find =
            |doc: &Document, tag: &str| doc.nodes.iter().position(|node| node.tag == tag).unwrap();
        let (body, i, p) = (find(&doc, "body"), find(&doc, "i"), find(&doc, "p"));
        let visible = vec![true; doc.nodes.len()];
        let start = Instant::now();
        let mut motion = Motion::default();
        let mut update = |doc: &Document, time: u64| {
            motion
                .update(
                    doc,
                    &computed_styles(doc),
                    &visible,
                    &|_| true,
                    start + Duration::from_millis(time),
                    false,
                    &mut Vec::new(),
                )
                .0
        };
        update(&doc, 0);
        doc.nodes[body].attrs.insert("class".into(), "on".into());
        update(&doc, 0);
        let animated = update(&doc, 500);
        let moving = |id: usize| animated.iter().any(|(node, ..)| *node == id);
        assert!(moving(body) && moving(i));
        assert!(
            !moving(p),
            "a paragraph that is always red must not animate"
        );
    }

    #[test]
    fn filled_animations_keep_one_style_while_unchanged() {
        let doc = Document::parse(
            "<style>@keyframes grow{to{width:10px}}p{width:0px;animation:grow 1s forwards}</style><p></p>",
        );
        let styles = computed_styles(&doc);
        let visible = vec![true; doc.nodes.len()];
        let start = Instant::now();
        let mut motion = Motion::default();
        let mut style = |time: f64| {
            let now = start + Duration::from_secs_f64(time);
            let (animated, _) = motion.update(
                &doc,
                &styles,
                &visible,
                &|_| true,
                now,
                false,
                &mut Vec::new(),
            );
            animated.first().map(|(_, style, _)| style.clone())
        };
        assert!(style(0.).is_none());
        let (half, end, later) = (style(0.5).unwrap(), style(2.).unwrap(), style(3.).unwrap());
        assert!(!Rc::ptr_eq(&half, &end) && Rc::ptr_eq(&end, &later));
        assert_eq!(&later["width"], "10px");
    }

    #[test]
    fn linear_easing_spreads_missing_inputs_and_extrapolates() {
        assert_eq!(easing("linear(0, 1)", 0.3), 0.3);
        assert!((easing("linear(0, 0.25 50%, 1)", 0.25) - 0.125).abs() < 1e-9);
        assert!((easing("linear(0, 0.25 50%, 1)", 0.75) - 0.625).abs() < 1e-9);
        assert!((easing("linear(0, 0.5, 1 40%, 1)", 0.2) - 0.5).abs() < 1e-9);
        assert_eq!(easing("linear(0, 0.5 20% 60%, 1)", 0.4), 0.5);
        assert_eq!(easing("linear(0, 1 50%, 0.2 30%)", 0.5), 0.2);
        assert!((easing("linear(0, 1)", 1.5) - 1.5).abs() < 1e-9);
        assert!((easing("linear(0, 1.2 60%, 1)", 0.6) - 1.2).abs() < 1e-9);
        assert!(Easing::parse("linear(1)").is_none());
    }

    #[test]
    fn script_animations_follow_web_animations_timing() {
        let doc = Document::parse("<div style='opacity:0.5'></div>");
        let styles = computed_styles(&doc);
        let id = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
        let base = styles.values[id].clone();
        let opacity = |animation: &Scripted, local: f64| {
            let mut out = Style::new();
            animation
                .effect
                .sample(local, true, &base, &Default::default(), &mut out);
            out.get(Prop::OPACITY)
                .map(|value| (value.parse::<f64>().unwrap() * 1e4).round() / 1e4)
        };
        let new = |frames: &[(f64, &str, &str)], timing, direction, fill, easing| {
            let mut animation =
                Scripted::new(1, frames, timing, [direction, fill, easing, "replace"], 0);
            animation.effect.tracks = tracks(&base, &animation.rules, &Default::default(), false);
            animation
        };
        let frames = [(0., "opacity:0", "linear"), (1., "opacity:1", "linear")];
        let alternate = new(
            &frames,
            [0., 0., 1., 3., 0.5],
            "alternate",
            "none",
            "linear",
        );
        assert_eq!(opacity(&alternate, 0.6), Some(0.9));
        assert_eq!(opacity(&alternate, 3.), None);
        let cut = new(&frames, [0.2, -0.5, 1., 1., 0.], "normal", "both", "linear");
        assert_eq!(opacity(&cut, 0.), Some(0.));
        assert_eq!(opacity(&cut, 0.5), Some(0.3));
        assert_eq!(opacity(&cut, 0.7), Some(0.5));
        let spring = new(
            &frames,
            [0., 0., 1., 1., 0.],
            "normal",
            "none",
            "linear(0, 1.2 60%, 1)",
        );
        assert_eq!(opacity(&spring, 0.6), Some(1.2));
        let single = new(
            &[(1., "opacity:1", "linear")],
            [0., 0., 1., 1., 0.],
            "normal",
            "none",
            "linear",
        );
        assert_eq!(opacity(&single, 0.5), Some(0.75));
        let mut motion = Motion::default();
        motion.script(
            id,
            new(&frames, [0., 0., 10., 1., 0.], "normal", "none", "linear"),
            2.,
            Some(1.),
            None,
        );
        let visible = vec![true; doc.nodes.len()];
        let (animated, next) = motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            Instant::now(),
            false,
            &mut Vec::new(),
        );
        let value = animated[0]
            .1
            .get(Prop::OPACITY)
            .unwrap()
            .parse::<f64>()
            .unwrap();
        assert!((0.2..0.21).contains(&value) && next.is_some());
        motion.script(
            id,
            new(&frames, [0., 0., 10., 1., 0.], "normal", "none", "linear"),
            5.,
            None,
            None,
        );
        let (animated, next) = motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            Instant::now(),
            false,
            &mut Vec::new(),
        );
        assert_eq!(
            (animated[0].1.get(Prop::OPACITY), next),
            (Some("0.5"), None)
        );
        assert_eq!(motion.unscript(1).map(|(node, _)| node), Some(id));
        assert!(motion.scripted(id, 0, &styles, u32::MAX).is_none());
    }

    #[test]
    fn composite_adds_and_accumulates_values() {
        assert_eq!(add("opacity", "0.2", "0.4", false).as_deref(), Some("0.6"));
        assert_eq!(
            add("width", "10px", "2em", false).as_deref(),
            Some("calc(10px + 2em)")
        );
        assert_eq!(
            add("transform", "translateX(10px)", "rotate(5deg)", false).as_deref(),
            Some("translateX(10px) rotate(5deg)")
        );
        assert_eq!(
            add(
                "transform",
                "translateX(10px) scale(2)",
                "translateX(5px) scale(1.5)",
                true
            )
            .as_deref(),
            Some("translateX(15px) scale(2.5)")
        );
        assert_eq!(
            add(
                "background-color",
                "rgb(100, 0, 0)",
                "rgb(200, 10, 0)",
                false
            )
            .as_deref(),
            Some("rgba(255, 10, 0, 1)")
        );
        assert_eq!(add("display", "block", "none", false), None);
    }

    #[test]
    fn css_animations_of_elements_and_pseudo_elements_answer_script() {
        let doc = Document::parse(
            "<style>@keyframes fade{from{opacity:0}to{opacity:1}}@keyframes vary{from{--x:a}to{--x:b}}div{animation:fade 1s linear,vary 1s linear -0.6s paused}p::before{content:'x';animation:fade 2s linear -1s paused}</style><div></div><p></p>",
        );
        let styles = computed_styles(&doc);
        let [div, p] =
            ["div", "p"].map(|tag| doc.nodes.iter().position(|node| node.tag == tag).unwrap());
        let visible = vec![true; doc.nodes.len()];
        let mut motion = Motion::default();
        let mut events = Vec::new();
        let (animated, _) = motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            Instant::now(),
            false,
            &mut events,
        );
        let own = animated.iter().find(|(id, ..)| *id == div).unwrap();
        assert_eq!(own.1.get(Prop::new("--x")), Some("b"));
        let pair = &motion.pseudos.iter().find(|(id, _)| *id == p).unwrap().1;
        assert_eq!(pair[0].get(Prop::OPACITY), Some("0.5"));
        assert!(animated.iter().any(|(id, ..)| *id == p));
        let list = motion.css(&[div, p], false, &styles);
        assert_eq!(list.len(), 2);
        assert_eq!(
            (list[0][3].as_str(), list[0][5].as_str()),
            (Some("fade"), Some("running"))
        );
        let pseudos = motion.css(&[p], true, &styles);
        assert_eq!(
            (pseudos[0][3].as_str(), pseudos[0][11].as_str()),
            (Some("fade"), Some("::before"))
        );
        let serial = list[0][0].as_u64().unwrap() as u32;
        assert!(motion.css_control(serial, "pause", 0., &styles));
        assert_eq!(
            motion.css(&[div], false, &styles)[0][5].as_str(),
            Some("paused")
        );
        assert!(motion.css_control(serial, "seek", 250., &styles));
        assert_eq!(
            motion.css(&[div], false, &styles)[0][4].as_f64(),
            Some(250.)
        );
        assert!(motion.css_control(serial, "cancel", 0., &styles));
        assert_eq!(motion.css(&[div], false, &styles).len(), 1);
        let mut events = Vec::new();
        motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            Instant::now(),
            false,
            &mut events,
        );
        assert!(
            events
                .iter()
                .any(|(id, kind, name, _)| (*id, *kind, name.as_str())
                    == (div, "animationcancel", "fade"))
        );
        assert!(!motion.css_control(u32::MAX, "pause", 0., &styles));
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
        for invalid in [
            "steps()",
            "steps(0)",
            "steps(2.5)",
            "steps(1, jump-none)",
            "cubic-bezier(1.5, 0, 0, 1)",
            "cubic-bezier(0, 0, 1)",
            "linear(0)",
        ] {
            assert!(Easing::parse(invalid).is_none(), "{invalid}");
        }
        for name in [
            "ease-in",
            "steps(3, jump-both)",
            "cubic-bezier(0.1, 2, 0.3, -1)",
        ] {
            assert_eq!(Easing::parse(name).unwrap().to_string(), name);
        }
        assert_eq!(
            easing("linear(0, 0.5 25%, 1)", 0.5),
            easing("linear(0 0%, 0.5 25%, 1 100%)", 0.5)
        );
        let frames = HashMap::from([("pulse".into(), Vec::new())]);
        assert!(
            Animation::parse(
                &Style::from_iter([
                    (Prop::new("animation"), "".into()),
                    (Prop::new("animation-name"), "pulse".into())
                ]),
                &frames,
                &Default::default()
            )
            .is_empty()
        );
        assert!(
            Animation::parse(
                &Style::from_iter([
                    (Prop::new("animation"), "1s pulse".into()),
                    (Prop::new("animation-iteration-count"), "NaN".into())
                ]),
                &frames,
                &Default::default()
            )
            .is_empty()
        );
        assert_eq!(
            transitions(&Style::from_iter([
                (
                    Prop::TRANSITION,
                    "opacity .2s ease-in 1s, all .3s allow-discrete".into()
                ),
                (Prop::new("transition-timing-function"), "linear".into())
            ])),
            vec![
                ("opacity".into(), 0.2, 1., Easing::Linear, false),
                ("all".into(), 0.3, 0., Easing::Linear, true)
            ]
        );
    }

    type Step = (
        HashMap<usize, (Rc<Style>, bool)>,
        Option<Instant>,
        Vec<(usize, &'static str, String, f64)>,
    );

    fn step(motion: &mut Motion, doc: &Document, styles: &ComputedStyles, at: Instant) -> Step {
        let visible: Vec<bool> = (0..doc.nodes.len())
            .map(|id| styles.values[id].get(Prop::DISPLAY) != Some("none"))
            .collect();
        let mut events = Vec::new();
        let (animated, next) =
            motion.update(doc, styles, &visible, &|_| true, at, false, &mut events);
        let animated = animated
            .into_iter()
            .map(|(id, style, layout)| (id, (style, layout)))
            .collect();
        (animated, next, events)
    }

    fn node(doc: &Document, tag: &str) -> usize {
        doc.nodes.iter().position(|node| node.tag == tag).unwrap()
    }

    #[test]
    fn huge_page_times_never_panic() {
        let mut doc = Document::parse(
            "<style>@keyframes fade{to{opacity:0}}div{animation:fade 1e19s 2}p{animation:fade 1s 1e300s}span{display:block;opacity:1;transition:opacity 1e300s}span.on{opacity:0}</style><div></div><p></p><span></span>",
        );
        let start = Instant::now();
        let mut motion = Motion::default();
        step(&mut motion, &doc, &computed_styles(&doc), start);
        let span = node(&doc, "span");
        doc.nodes[span].attrs.insert("class".into(), "on".into());
        let styles = computed_styles(&doc);
        step(&mut motion, &doc, &styles, start);
        let nodes: Vec<usize> = (0..doc.nodes.len()).collect();
        let list = motion.css(&nodes, false, &styles);
        assert_eq!(list.len(), 3);
        for entry in list {
            let serial = entry[0].as_u64().unwrap() as u32;
            for (action, time) in [
                ("seek", 1e300),
                ("pause", 0.),
                ("seek", -1e300),
                ("play", 0.),
                ("seek", f64::MAX),
                ("finish", 0.),
                ("play", 0.),
            ] {
                motion.css_control(serial, action, time, &styles);
            }
        }
        for at in [start, start + Duration::from_secs(1)] {
            step(&mut motion, &doc, &styles, at);
            for id in nodes.iter().copied() {
                motion.next_event(id, &styles, at);
                motion.timeline(id, &styles, &[at], &[]);
            }
        }
        assert_eq!(after(start, 1e19), None);
        assert_eq!(after(start, f64::INFINITY), None);
    }

    #[test]
    fn play_state_changes_keep_the_time_before_them() {
        let mut doc = Document::parse(
            "<style>@keyframes spin{to{transform:rotate(360deg)}}p{animation:spin 10s linear infinite}p.paused{animation-play-state:paused}</style><p></p>",
        );
        let p = node(&doc, "p");
        let start = Instant::now();
        let mut motion = Motion::default();
        let mut turn = |doc: &Document, styles: &ComputedStyles, seconds: f64| {
            let at = start + Duration::from_secs_f64(seconds);
            let (animated, ..) = step(&mut motion, doc, styles, at);
            animated[&p].0.get(Prop::TRANSFORM).unwrap().to_owned()
        };
        let running = computed_styles(&doc);
        turn(&doc, &running, 0.);
        turn(&doc, &running, 0.5);
        doc.nodes[p].attrs.insert("class".into(), "paused".into());
        let paused = computed_styles(&doc);
        assert_eq!(turn(&doc, &paused, 1.), "rotate(36deg)");
        doc.nodes[p].attrs.remove("class");
        assert_eq!(turn(&doc, &running, 11.), "rotate(36deg)");
        assert_eq!(turn(&doc, &running, 11.5), "rotate(54deg)");
    }

    #[test]
    fn iterations_wake_only_listening_pages_and_single_effects_loop() {
        let doc = Document::parse(
            "<style>@keyframes pulse{to{opacity:0}}div{animation:pulse 1s infinite}p{animation:pulse .5s 3 alternate}i{animation:pulse 1s 2s}b{animation:pulse 1s,pulse 2s}</style><div></div><p></p><i></i><b></b>",
        );
        let [div, p, i, b] = ["div", "p", "i", "b"].map(|tag| node(&doc, tag));
        let styles = computed_styles(&doc);
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut motion = Motion::default();
        let kinds = |motion: &mut Motion, seconds: f64| {
            let (.., events) = step(motion, &doc, &styles, at(seconds));
            events
                .into_iter()
                .filter(|event| event.0 == div)
                .map(|event| event.1)
                .collect::<Vec<_>>()
        };
        assert_eq!(kinds(&mut motion, 0.), ["animationstart"]);
        assert_eq!(kinds(&mut motion, 1.1), [""; 0]);
        assert_eq!(motion.next_event(div, &styles, at(1.1)), None);
        let period = |id: usize, seconds: f64| {
            motion
                .period(id, &styles, at(seconds))
                .map(|(period, phase, left, shape)| {
                    let round = |value: f64| (value * 1e6).round() / 1e6;
                    (period, round(phase), left, shape.is_some())
                })
        };
        assert_eq!(period(div, 1.1), Some((1., 0.1, f64::INFINITY, true)));
        assert_eq!(period(p, 0.25), Some((1., 0.25, 1.25, true)));
        assert_eq!(period(i, 1.), None);
        assert_eq!(period(b, 0.5), None);
        assert_eq!(period(b, 1.5), Some((2., 1.5, 0.25, false)));
        motion.iterations = true;
        assert_eq!(kinds(&mut motion, 2.2), ["animationiteration"]);
        let next = motion.next_event(div, &styles, at(2.2)).unwrap();
        assert!((next.duration_since(start).as_secs_f64() - 3.).abs() < 1e-6);
    }

    #[test]
    fn endless_animations_without_a_duration_end_at_zero() {
        let doc = Document::parse(
            "<style>@keyframes pulse{to{opacity:0}}div{animation:pulse 0s infinite}</style><div></div>",
        );
        let div = node(&doc, "div");
        let styles = computed_styles(&doc);
        let start = Instant::now();
        let mut motion = Motion::default();
        let (.., events) = step(&mut motion, &doc, &styles, start);
        let events: Vec<_> = events
            .into_iter()
            .map(|(id, kind, _, time)| (id, kind, time))
            .collect();
        assert_eq!(
            events,
            [(div, "animationstart", 0.), (div, "animationend", 0.)]
        );
        assert_eq!(motion.next_event(div, &styles, start), None);
    }

    #[test]
    fn late_starts_fire_no_iteration_events() {
        let doc = Document::parse(
            "<style>@keyframes pulse{to{opacity:0}}div{animation:pulse 1s -2.5s 5}p{animation:pulse 1s 1s 5}</style><div></div><p></p>",
        );
        let (div, p) = (node(&doc, "div"), node(&doc, "p"));
        let styles = computed_styles(&doc);
        let start = Instant::now();
        let mut motion = Motion {
            iterations: true,
            ..Default::default()
        };
        let mut events = |seconds: f64| {
            let at = start + Duration::from_secs_f64(seconds);
            let (.., events) = step(&mut motion, &doc, &styles, at);
            let mut events: Vec<_> = events
                .into_iter()
                .map(|(id, kind, _, time)| (id, kind, time))
                .collect();
            events.sort_by_key(|event| event.0);
            events
        };
        assert_eq!(events(0.), [(div, "animationstart", 2.5)]);
        assert_eq!(events(0.6), [(div, "animationiteration", 3.)]);
        assert_eq!(
            events(3.5),
            [(div, "animationend", 5.), (p, "animationstart", 0.)]
        );
        assert_eq!(events(4.2), [(p, "animationiteration", 3.)]);
    }

    #[test]
    fn animations_keep_their_clocks_by_name() {
        let mut doc = Document::parse(
            "<style>@keyframes a{to{opacity:0}}@keyframes b{to{width:10px}}div{width:0px;animation:a 1s,b 2s linear infinite}div.one{animation:b 2s linear infinite}div.two{animation:b 2s linear infinite,b 2s linear infinite}</style><div></div>",
        );
        let div = node(&doc, "div");
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let serials = |motion: &Motion, styles: &ComputedStyles| {
            motion
                .css(&[div], false, styles)
                .iter()
                .map(|entry| entry[0].as_u64().unwrap())
                .collect::<Vec<_>>()
        };
        let kinds = |events: &[(usize, &'static str, String, f64)]| {
            events
                .iter()
                .map(|event| (event.1, event.2.clone()))
                .collect::<Vec<_>>()
        };
        let mut motion = Motion::default();
        let styles = computed_styles(&doc);
        step(&mut motion, &doc, &styles, at(0.));
        let b = serials(&motion, &styles)[1];
        doc.nodes[div].attrs.insert("class".into(), "one".into());
        let styles = computed_styles(&doc);
        let (animated, _, events) = step(&mut motion, &doc, &styles, at(1.));
        assert_eq!(kinds(&events), [("animationcancel", "a".into())]);
        assert_eq!(serials(&motion, &styles), [b]);
        assert_eq!(animated[&div].0.get(Prop::WIDTH), Some("5px"));
        doc.nodes[div].attrs.insert("class".into(), "two".into());
        let styles = computed_styles(&doc);
        let (.., events) = step(&mut motion, &doc, &styles, at(1.5));
        assert_eq!(kinds(&events), [("animationstart", "b".into())]);
        let list = serials(&motion, &styles);
        assert!(list[0] != b && list[1] == b, "{list:?}");
    }

    #[test]
    fn retargeted_transitions_cancel_before_they_run_again() {
        let mut doc = Document::parse(
            "<style>div{width:0px;transition:width 1s linear}div.a{width:10px}div.b{width:20px}</style><div></div>",
        );
        let div = node(&doc, "div");
        let start = Instant::now();
        let mut motion = Motion::default();
        let mut kinds = |doc: &Document, seconds: f64| {
            let at = start + Duration::from_secs_f64(seconds);
            let (.., events) = step(&mut motion, doc, &computed_styles(doc), at);
            events.into_iter().map(|event| event.1).collect::<Vec<_>>()
        };
        kinds(&doc, 0.);
        doc.nodes[div].attrs.insert("class".into(), "a".into());
        assert_eq!(kinds(&doc, 0.), ["transitionrun", "transitionstart"]);
        doc.nodes[div].attrs.insert("class".into(), "b".into());
        assert_eq!(
            kinds(&doc, 0.5),
            ["transitioncancel", "transitionrun", "transitionstart"]
        );
    }

    #[test]
    fn content_and_font_size_keyframes_animate() {
        let doc = Document::parse(
            "<style>@keyframes dots{33%{content:'..'}66%{content:'...'}}@keyframes grow{to{font-size:32px;line-height:40px}}i::after{content:'.';animation:dots 1s infinite}em{font-size:16px;line-height:20px;animation:grow 1s linear}</style><i></i><em></em>",
        );
        let (i, em) = (node(&doc, "i"), node(&doc, "em"));
        let styles = computed_styles(&doc);
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut motion = Motion::default();
        let mut sample = |seconds: f64| {
            step(&mut motion, &doc, &styles, at(seconds));
            let pair = &motion.pseudos.iter().find(|(id, _)| *id == i).unwrap().1;
            let sizes = motion.custom.get(&em).map(|values| {
                [Prop::FONT_SIZE, Prop::LINE_HEIGHT]
                    .map(|prop| values.get(prop).unwrap().to_owned())
            });
            (pair[1].get(Prop::CONTENT).unwrap().to_owned(), sizes)
        };
        sample(0.);
        assert_eq!(sample(0.4).0, "'..'");
        assert_eq!(sample(0.5).1, Some(["24px".into(), "30px".into()]));
        assert_eq!(sample(0.7).0, "'...'");
        assert_eq!(sample(0.95).0, "'.'");
        let doc = Document::parse(
            "<style>@keyframes dots{33%{content:'..'}66%{content:'...'}}i::after{content:'.';animation:dots 1s infinite}</style><i></i>",
        );
        let mut motion = Motion::default();
        let styles = computed_styles(&doc);
        step(&mut motion, &doc, &styles, at(0.));
        let (_, next, _) = step(&mut motion, &doc, &styles, at(0.5));
        let wake = (next.unwrap() - at(0.5)).as_secs_f64();
        assert!(wake > 0.01 && wake < 0.2, "{wake}");
    }

    #[test]
    fn display_keeps_exits_shown_until_they_end() {
        let mut doc = Document::parse(
            "<style>@keyframes out{to{opacity:0;display:none}}b{display:block;animation:out 1s linear forwards}p,s{display:block;opacity:1;transform:translateY(10px) scale(.95)}p{transition:opacity 1s linear,transform 1s linear,display 1s allow-discrete}s{transition:opacity 1s linear}p.off,s.off{display:none;opacity:0;transform:translateY(0)}</style><b></b><p></p><s></s>",
        );
        let [b, p, s] = ["b", "p", "s"].map(|tag| node(&doc, tag));
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut motion = Motion::default();
        let styles = computed_styles(&doc);
        step(&mut motion, &doc, &styles, at(0.));
        let (animated, ..) = step(&mut motion, &doc, &styles, at(0.5));
        let (style, layout) = &animated[&b];
        assert_eq!(
            (style.get(Prop::OPACITY), style.get(Prop::DISPLAY), *layout),
            (Some("0.5"), Some("block"), false)
        );
        let (animated, ..) = step(&mut motion, &doc, &styles, at(1.5));
        assert_eq!(animated[&b].0.get(Prop::DISPLAY), Some("none"));
        for id in [p, s] {
            doc.nodes[id].attrs.insert("class".into(), "off".into());
        }
        let styles = computed_styles(&doc);
        let (animated, ..) = step(&mut motion, &doc, &styles, at(2.));
        assert!(!animated.contains_key(&s));
        assert_eq!(animated[&p].0.get(Prop::DISPLAY), Some("block"));
        let (animated, ..) = step(&mut motion, &doc, &styles, at(2.5));
        let style = &animated[&p].0;
        assert_eq!(
            [Prop::DISPLAY, Prop::OPACITY, Prop::TRANSFORM].map(|prop| style.get(prop)),
            [
                Some("block"),
                Some("0.5"),
                Some("translateY(5px) scale(0.975)")
            ]
        );
        let (animated, _, events) = step(&mut motion, &doc, &styles, at(3.));
        assert!(!animated.contains_key(&p));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.1 == "transitionend")
                .count(),
            3
        );
    }

    #[test]
    fn overlay_keeps_leaving_boxes_in_the_top_layer_until_it_ends() {
        let mut doc = Document::parse(
            "<style>dialog{opacity:0;transition:opacity 1s linear,overlay 1s allow-discrete,display 1s allow-discrete}dialog[open]{opacity:1}p{display:block;transition:overlay 1s}</style><dialog open></dialog><p popover></p>",
        );
        let [d, p] = ["dialog", "p"].map(|tag| node(&doc, tag));
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut motion = Motion::default();
        doc.nodes[d].state = crate::dom::MODAL;
        doc.nodes[p].state = crate::dom::POPOVER;
        let styles = computed_styles(&doc);
        assert!(step(&mut motion, &doc, &styles, at(0.)).0.is_empty());
        doc.nodes[d].attrs.remove("open");
        doc.nodes[d].state = crate::dom::MODAL << 5;
        doc.nodes[p].state = crate::dom::POPOVER << 5;
        let styles = computed_styles(&doc);
        assert_eq!(styles.values[d].get(Prop::OVERLAY), Some("none"));
        let (animated, ..) = step(&mut motion, &doc, &styles, at(1.));
        assert_eq!(animated[&d].0.get(Prop::OVERLAY), Some("auto"));
        assert!(!animated.contains_key(&p));
        let (animated, ..) = step(&mut motion, &doc, &styles, at(1.5));
        let style = &animated[&d].0;
        assert_eq!(
            [Prop::OVERLAY, Prop::DISPLAY, Prop::OPACITY].map(|prop| style.get(prop)),
            [Some("auto"), Some("block"), Some("0.5")]
        );
        let (animated, next, events) = step(&mut motion, &doc, &styles, at(2.));
        assert!(!animated.contains_key(&d) && next.is_none());
        assert!(events.contains(&(d, "transitionend", "overlay".into(), 1.)));
        let mut doc = Document::parse(
            "<style>dialog{transition:overlay 1s allow-discrete,display 1s allow-discrete}</style><dialog></dialog>",
        );
        let d = node(&doc, "dialog");
        let mut motion = Motion::default();
        doc.nodes[d].attrs.insert("open".into(), String::new());
        doc.nodes[d].state = crate::dom::MODAL;
        step(&mut motion, &doc, &computed_styles(&doc), at(0.));
        doc.nodes[d].attrs.remove("open");
        doc.nodes[d].state = crate::dom::MODAL << 5;
        let styles = computed_styles(&doc);
        step(&mut motion, &doc, &styles, at(1.));
        let (_, next, _) = step(&mut motion, &doc, &styles, at(1.25));
        assert_eq!(next, Some(at(2.)));
    }

    #[test]
    fn backdrops_transition_from_their_starting_style_and_out() {
        let mut doc = Document::parse(
            "<style>dialog::backdrop{opacity:0;transition:opacity 1s linear}dialog[open]::backdrop{opacity:1}@starting-style{dialog[open]::backdrop{opacity:0}}</style><dialog></dialog>",
        );
        let d = node(&doc, "dialog");
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut motion = Motion::default();
        step(&mut motion, &doc, &computed_styles(&doc), at(0.));
        doc.nodes[d].attrs.insert("open".into(), String::new());
        doc.nodes[d].state = crate::dom::MODAL;
        let mut opacity = |seconds: f64, doc: &Document| {
            let (_, _, events) = step(&mut motion, doc, &computed_styles(doc), at(seconds));
            let opacity = motion
                .backdrops
                .iter()
                .find(|(id, _)| *id == d)
                .and_then(|(_, style)| style.get(Prop::OPACITY).map(str::to_owned));
            (opacity, events)
        };
        let (value, events) = opacity(1., &doc);
        assert_eq!(value.as_deref(), Some("0"));
        assert!(events.contains(&(d, "transitionrun", "::backdrop\0opacity".into(), 0.)));
        assert_eq!(opacity(1.5, &doc).0.as_deref(), Some("0.5"));
        let (value, events) = opacity(2., &doc);
        assert_eq!(value, None);
        assert!(events.contains(&(d, "transitionend", "::backdrop\0opacity".into(), 1.)));
        doc.nodes[d].attrs.remove("open");
        doc.nodes[d].state = crate::dom::MODAL << 5;
        opacity(3., &doc);
        assert_eq!(opacity(3.25, &doc).0.as_deref(), Some("0.75"));
        doc.nodes[d].state = 0;
        let (value, events) = opacity(3.5, &doc);
        assert_eq!(value, None);
        assert!(events.contains(&(d, "transitioncancel", "::backdrop\0opacity".into(), 0.5)));
    }

    #[test]
    fn script_animations_that_set_display_show_hidden_boxes() {
        let doc = Document::parse(
            "<style>@keyframes show{from,to{display:block}}div{display:none}i{display:block}b{display:none;animation:show 1s}</style><div><i></i></div><b></b>",
        );
        let [div, i, b] = ["div", "i", "b"].map(|tag| node(&doc, tag));
        let styles = computed_styles(&doc);
        let mut visible = vec![false; doc.nodes.len()];
        let mut pending = vec![0];
        while let Some(id) = pending.pop() {
            if styles.values[id].get(Prop::DISPLAY) != Some("none") {
                visible[id] = true;
                pending.extend(&doc.nodes[id].children);
            }
        }
        let mut motion = Motion::default();
        motion.script(
            div,
            Scripted::new(
                1,
                &[
                    (0., "display:block;opacity:0", "linear"),
                    (1., "display:block;opacity:1", "linear"),
                ],
                [0., 0., 1., 1., 0.],
                ["normal", "none", "linear", "replace"],
                0,
            ),
            0.,
            Some(1.),
            None,
        );
        let start = Instant::now();
        let mut update = |seconds: f64| {
            let mut events = Vec::new();
            let (animated, next) = motion.update(
                &doc,
                &styles,
                &visible,
                &|_| true,
                start + Duration::from_secs_f64(seconds),
                false,
                &mut events,
            );
            (animated, motion.painted.take(), next, events)
        };
        let (animated, painted, next, events) = update(0.5);
        let painted = painted.unwrap();
        assert!(painted[div] && painted[i] && !painted[b]);
        let (_, style, layout) = animated.iter().find(|(id, ..)| *id == div).unwrap();
        assert_eq!(style.get(Prop::DISPLAY), Some("block"));
        assert!(*layout && next.is_some() && events.is_empty());
        let (animated, painted, next, events) = update(1.5);
        assert!(animated.is_empty() && painted.is_none() && next.is_none() && events.is_empty());
    }

    #[test]
    fn tailwind_transform_lists_transition_smoothly() {
        let mut doc = Document::parse(
            "<style>@import 'tailwindcss';</style><div class='transition-transform duration-1000 ease-linear rotate-x-12'></div>",
        );
        let div = node(&doc, "div");
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut motion = Motion::default();
        step(&mut motion, &doc, &computed_styles(&doc), at(0.));
        let mut transform = |class: &str, seconds: [f64; 2]| {
            doc.nodes[div].attrs.insert(
                "class".into(),
                format!("transition-transform duration-1000 ease-linear {class}"),
            );
            let styles = computed_styles(&doc);
            step(&mut motion, &doc, &styles, at(seconds[0]));
            let (animated, ..) = step(&mut motion, &doc, &styles, at(seconds[1]));
            animated[&div].0.get(Prop::TRANSFORM).unwrap().to_owned()
        };
        assert_eq!(
            transform("rotate-x-12 -skew-x-6", [0., 0.5]),
            "rotateX(12deg) skewX(calc(0deg * 0.5 + (6deg * -1) * 0.5))"
        );
        assert_eq!(
            super::super::Lengths::new(16., 800., 600.).math(
                "calc(0deg * 0.5 + (6deg * -1) * 0.5)",
                0.,
                &[]
            ),
            Some((-3., super::super::parser::Kind::Angle))
        );
        assert!(transform("skew-x-6", [2., 2.5]).starts_with("matrix3d("));
    }

    #[test]
    fn transform_matrices_round_trip() {
        let lerp = Lerp::new(
            "transform",
            "rotate(30deg) skewX(10deg) scale(2, 3) translate(5px, 6px)",
            "skewY(1deg)",
        )
        .unwrap();
        let at = |t: f64| {
            let mut out = String::new();
            lerp.write(t, &mut out);
            out
        };
        assert_eq!(
            at(0.),
            "matrix(1.73205, 1, -1.04189, 2.86257, 2.40892, 22.1754)"
        );
        assert_eq!(at(1.), "matrix(1, 0.0174551, 0, 1, 0, 0)");
    }

    #[test]
    fn exits_keep_their_subtree_painted_until_they_end() {
        let mut doc = Document::parse(
            "<style>@keyframes spin{to{rotate:1turn}}@keyframes out{to{display:none}}div,i,p,s,u,b,em{display:block}div{transition:opacity 1s linear,display 1s allow-discrete}div.off{display:none;opacity:0}i,em{animation:spin 1s linear infinite}p{transition:opacity 1s linear}div.off p{opacity:0}s{transition:display 1s allow-discrete}div.off s{display:none}b.out{animation:out .5s forwards}a{display:block;opacity:1;transition:opacity 1s linear,display 1s allow-discrete}a.off{display:none}@starting-style{a{opacity:0}}</style><p></p><s><u></u></s><div><i></i></div><b><em></em></b><a class=off></a>",
        );
        let [div, i, p, s, u, b, em, a] =
            ["div", "i", "p", "s", "u", "b", "em", "a"].map(|tag| node(&doc, tag));
        let body = doc.nodes[p].parent.unwrap();
        for child in [s, p] {
            doc.nodes[body].children.retain(|&other| other != child);
            doc.nodes[div].children.push(child);
            doc.nodes[child].parent = Some(div);
        }
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut motion = Motion::default();
        let mut step = |doc: &Document, seconds: f64| {
            let styles = computed_styles(doc);
            let mut visible = vec![false; doc.nodes.len()];
            let mut pending = vec![0];
            while let Some(id) = pending.pop() {
                if styles.values[id].get(Prop::DISPLAY) != Some("none") {
                    visible[id] = true;
                    pending.extend(&doc.nodes[id].children);
                }
            }
            let mut events = Vec::new();
            let (animated, next) = motion.update(
                doc,
                &styles,
                &visible,
                &|_| true,
                at(seconds),
                false,
                &mut events,
            );
            let painted = motion.painted.take();
            let shown = painted.clone().unwrap_or(visible);
            let animated: HashMap<usize, Rc<Style>> = animated
                .into_iter()
                .map(|(id, style, _)| (id, style))
                .collect();
            (painted.is_some(), shown, animated, events, next)
        };
        let (changed, shown, ..) = step(&doc, 0.);
        assert!(!changed && shown[div] && !shown[a]);
        doc.nodes[div].attrs.insert("class".into(), "off".into());
        doc.nodes[a].attrs.remove("class");
        for seconds in [0.1, 0.6] {
            let (changed, shown, animated, events, _) = step(&doc, seconds);
            assert!(changed, "{seconds}");
            assert_eq!(
                [div, i, p, s, u, a].map(|id| shown[id]),
                [true; 6],
                "{events:?}"
            );
            assert!(
                !events.iter().any(|event| event.1.ends_with("cancel")),
                "{events:?}"
            );
            assert!(animated[&i].get(Prop::ROTATE).is_some());
            assert_eq!(animated[&s].get(Prop::DISPLAY), Some("block"));
            let opacity = |id: usize| {
                animated[&id]
                    .get(Prop::OPACITY)
                    .unwrap()
                    .parse::<f64>()
                    .unwrap()
            };
            assert!((opacity(p) + seconds - 1.1).abs() < 1e-3, "{}", opacity(p));
            assert!((opacity(a) - seconds + 0.1).abs() < 1e-3, "{}", opacity(a));
        }
        let (changed, shown, animated, events, _) = step(&doc, 1.2);
        assert!(!changed && [div, i, p, s, u].iter().all(|&id| !shown[id]) && shown[a]);
        assert!(!animated.contains_key(&div) && !animated.contains_key(&i));
        assert!(
            events
                .iter()
                .any(|event| event == &(div, "transitionend", "display".into(), 1.))
        );
        doc.nodes[b].attrs.insert("class".into(), "out".into());
        step(&doc, 2.);
        let (changed, shown, animated, _, next) = step(&doc, 2.6);
        assert_eq!(animated[&b].get(Prop::DISPLAY), Some("none"));
        assert!(changed && !shown[b] && !shown[em] && next.is_none());
    }

    #[test]
    fn transition_duration_alone_transitions_all_properties() {
        let mut doc = Document::parse(
            "<style>div{opacity:1;transition-duration:1s;transition-timing-function:linear}div.on{opacity:0}p{opacity:1;transition-delay:-0.5s;transition-duration:1s;transition-timing-function:linear}p.on{opacity:0}</style><div></div><p></p>",
        );
        let [div, p] = ["div", "p"].map(|tag| node(&doc, tag));
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let mut motion = Motion::default();
        step(&mut motion, &doc, &computed_styles(&doc), at(0.));
        for id in [div, p] {
            doc.nodes[id].attrs.insert("class".into(), "on".into());
        }
        let styles = computed_styles(&doc);
        step(&mut motion, &doc, &styles, at(0.));
        let (animated, ..) = step(&mut motion, &doc, &styles, at(0.25));
        assert_eq!(animated[&div].0.get(Prop::OPACITY), Some("0.75"));
        assert_eq!(animated[&p].0.get(Prop::OPACITY), Some("0.25"));
        assert!(motion.transitions[&div][0].lerp.is_some());
    }

    #[test]
    fn keyframes_interpolate_filters_keywords_shapes_and_steps() {
        let doc = Document::parse(
            "<style>@keyframes k{from{filter:none;backdrop-filter:blur(2px);font-weight:normal;border-top-width:0px;letter-spacing:normal;clip-path:inset(0px);z-index:-1}to{filter:blur(4px) brightness(50%);backdrop-filter:blur(4px) brightness(2);font-weight:bold;border-top-width:thick;letter-spacing:4px;clip-path:inset(10px 20px);z-index:-4}}div{border-top-style:solid;animation:k 1s linear -0.5s paused}</style><div></div>",
        );
        let styles = computed_styles(&doc);
        let div = node(&doc, "div");
        assert!(
            styles.animations[&div][0]
                .tracks
                .iter()
                .all(|(_, frames)| frames[0].lerp.is_some())
        );
        let (animated, ..) = step(&mut Motion::default(), &doc, &styles, Instant::now());
        let style = &animated[&div].0;
        assert_eq!(
            [
                "filter",
                "backdrop-filter",
                "font-weight",
                "border-top-width",
                "letter-spacing",
                "clip-path",
                "z-index"
            ]
            .map(|name| style.get(Prop::new(name))),
            [
                Some("blur(2px) brightness(75%)"),
                Some("blur(3px) brightness(1.5)"),
                Some("550"),
                Some("2.5px"),
                Some("2px"),
                Some("inset(5px 10px 5px 10px)"),
                Some("-2")
            ]
        );
        let base = styles.values[div].clone();
        let mut animation = Scripted::new(
            1,
            &[(0., "opacity:0", "linear"), (1., "opacity:1", "linear")],
            [1., 0., 1., 1., 0.],
            ["normal", "both", "steps(2, jump-start)", "replace"],
            0,
        );
        animation.effect.tracks = tracks(&base, &animation.rules, &Default::default(), false);
        let opacity = |local: f64| {
            let mut out = Style::new();
            animation
                .effect
                .sample(local, true, &base, &Default::default(), &mut out);
            out.get(Prop::OPACITY).map(str::to_owned)
        };
        assert_eq!(
            [0.5, 1., 1.6].map(opacity),
            ["0", "0.5", "1"].map(|v| Some(v.into()))
        );
    }
}
