use super::{
    ComputedStyles, Declarations, Prop, Registry, Style, color, components, expand,
    resolve_keywords, resolve_style,
};
use crate::dom::Document;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Instant,
};

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
    pub paused: bool,
    duration: f64,
    delay: f64,
    iterations: f64,
    direction: String,
    fill: String,
    /// The default easing of keyframes.
    timing: String,
    /// The easing of each iteration, which `Element.animate()` sets.
    easing: String,
    end_delay: f64,
    iteration_start: f64,
    /// How keyframes combine with the value below: `replace`, `add` or `accumulate`.
    composite: String,
    /// The element (0), its `::before` (1) or `::after` (2). Pseudo-element animations have names like
    /// `::before\0spin`, so their clocks and events stay apart.
    pub pseudo: u8,
    tracks: Vec<(Prop, Keyframes)>,
}

type Keyframes = Vec<(f64, String, String)>;

/// The animated style of each node, and whether it changes layout.
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
                    } else if timing_function(part) {
                        "timing-function"
                    } else {
                        "name"
                    };
                    values.insert(key, part);
                }
                for (key, prop) in [
                    ("name", Prop::ANIMATION_NAME),
                    ("duration", Prop::ANIMATION_DURATION),
                    ("delay", Prop::ANIMATION_DELAY),
                    ("iteration-count", Prop::ANIMATION_ITERATION_COUNT),
                    ("direction", Prop::ANIMATION_DIRECTION),
                    ("fill-mode", Prop::ANIMATION_FILL_MODE),
                    ("timing-function", Prop::ANIMATION_TIMING_FUNCTION),
                    ("play-state", Prop::ANIMATION_PLAY_STATE),
                ] {
                    if let Some(value) = style.get(prop) {
                        let parts = components(value, ',');
                        if !parts.is_empty() {
                            values.insert(key, parts[index % parts.len()]);
                        }
                    }
                }
                let name = values["name"].trim_matches(['\'', '"']);
                let rules: Vec<_> = keyframes
                    .get(name)?
                    .iter()
                    .map(|(offset, declarations)| {
                        let declarations = declarations.resolve();
                        let timing = declarations
                            .get(Prop::ANIMATION_TIMING_FUNCTION)
                            .map(str::to_owned)
                            .unwrap_or_default();
                        (f64::from(*offset), declarations, timing)
                    })
                    .collect();
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
                    easing: String::new(),
                    end_delay: 0.,
                    iteration_start: 0.,
                    composite: style
                        .get(Prop::ANIMATION_COMPOSITION)
                        .map(|value| components(value, ','))
                        .and_then(|parts| {
                            parts
                                .get(index % parts.len().max(1))
                                .map(|part| part.to_string())
                        })
                        .unwrap_or_else(|| "replace".into()),
                    pseudo: 0,
                    tracks: tracks(style, &rules, registry),
                })
            })
            .collect()
    }
}

/// The computed values of keyframes `(offset, declarations, easing)` for an element with `style`, by property.
fn tracks(
    style: &Style,
    rules: &[(f64, Style, String)],
    registry: &Registry,
) -> Vec<(Prop, Keyframes)> {
    let mut tracks: Vec<(Prop, Keyframes)> = Vec::new();
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
        resolve_style(&mut frame, Some(style), &custom, registry);
        resolve_keywords(&mut frame, Some(style));
        for (key, value) in declarations.iter() {
            // Custom properties that can't interpolate switch halfway, like in browsers.
            if value.ends_with("!important") || !animatable(key, registry) && !key.is_custom() {
                continue;
            }
            let Some(value) = computed(&frame, key, registry) else {
                continue;
            };
            let frame = (*offset, value.to_owned(), timing.clone());
            match tracks.iter_mut().find(|(name, _)| *name == key) {
                Some((_, frames)) => {
                    frames.retain(|(at, ..)| *at != frame.0);
                    frames.push(frame);
                }
                None => tracks.push((key, vec![frame])),
            }
        }
    }
    for (_, frames) in &mut tracks {
        frames.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    tracks
}

impl Animation {
    /// Writes the animated values into `out`. Returns the seconds until the values can change
    /// next (0 while they change continuously, `None` when they never change), and whether the
    /// animation has ended.
    pub fn sample(
        &self,
        elapsed: f64,
        base: &Style,
        registry: &Registry,
        out: &mut Style,
    ) -> (Option<f64>, bool) {
        // The phases of Web Animations: before `start`, active, and after `stop`.
        let length = if self.duration == 0. {
            0.
        } else {
            self.duration * self.iterations
        };
        let end = (self.delay + length + self.end_delay).max(0.);
        let start = self.delay.min(end).max(0.);
        let stop = (self.delay + length).min(end).max(0.);
        let before = elapsed < start;
        let ended = elapsed >= stop;
        let active = !self.paused && !ended && !before;
        let mut wake =
            (!self.paused && !ended).then_some(if before { start } else { stop } - elapsed);
        if before && !matches!(self.fill.as_str(), "backwards" | "both")
            || ended && !matches!(self.fill.as_str(), "forwards" | "both")
        {
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
        let reverse = self.direction == "reverse"
            || self.direction == "alternate" && iteration % 2. == 1.
            || self.direction == "alternate-reverse" && iteration % 2. == 0.;
        if reverse {
            progress = 1. - progress;
        }
        if !self.easing.is_empty() && self.easing != "linear" {
            progress = easing(&self.easing, progress);
            if active {
                wake = Some(0.);
            }
        }
        for (property, frames) in &self.tracks {
            let mut keys: Vec<(f64, Cow<str>, &str)> = frames
                .iter()
                .map(|(at, value, timing)| (*at, Cow::Borrowed(value.as_str()), timing.as_str()))
                .collect();
            // Missing ends and added keyframes take the value below: earlier animations, or the element's own.
            let replace = self.composite == "replace";
            if !replace || keys[0].0 > 0. || keys[keys.len() - 1].0 < 1. {
                let under = out
                    .get(*property)
                    .or_else(|| computed(base, *property, registry))
                    .map(str::to_owned);
                if let Some(under) = under {
                    if !replace {
                        for key in &mut keys {
                            if let Some(sum) = add(
                                property.name(),
                                &under,
                                &key.1,
                                self.composite == "accumulate",
                            ) {
                                key.1 = Cow::Owned(sum);
                            }
                        }
                    }
                    if keys[0].0 > 0. {
                        keys.insert(0, (0., Cow::Owned(under.clone()), ""));
                    }
                    if keys[keys.len() - 1].0 < 1. {
                        keys.push((1., Cow::Owned(under), ""));
                    }
                }
            }
            let index = keys
                .iter()
                .rposition(|key| key.0 <= progress)
                .unwrap_or(0)
                .min(keys.len().saturating_sub(2));
            let (from, to) = (&keys[index], &keys[(index + 1).min(keys.len() - 1)]);
            // An easing that overshoots goes past the first or last keyframe.
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
            let timing = if from.2.is_empty() {
                &self.timing
            } else {
                from.2
            };
            let discrete = property.is_custom() && !animatable(*property, registry);
            if active && to.0 > from.0 {
                let steps = if from.1 == to.1 || matches!(timing, "step-start" | "step-end") {
                    Some(1.)
                } else if discrete {
                    (timing == "linear").then_some(2.)
                } else {
                    timing.strip_prefix("steps(").map(|args| {
                        components(args.trim_end_matches(')'), ',')
                            .first()
                            .and_then(|value| value.parse::<f64>().ok())
                            .unwrap_or(1.)
                            .max(1.)
                    })
                };
                wake = wake.map(|wake| match steps {
                    Some(steps) => {
                        let edge = if reverse {
                            ((local * steps).ceil() - 1.) / steps
                        } else {
                            ((local * steps).floor() + 1.) / steps
                        };
                        wake.min(
                            (edge.clamp(0., 1.) - local).abs() * (to.0 - from.0) * self.duration,
                        )
                    }
                    None => 0.,
                });
            }
            let local = easing(timing, local);
            let value = (!discrete)
                .then(|| interpolate(property.name(), &from.1, &to.1, local))
                .flatten()
                .unwrap_or_else(|| if local < 0.5 { &*from.1 } else { &*to.1 }.to_owned());
            out.insert(*property, value);
        }
        (wake, ended)
    }
}

struct Clock {
    serial: u32,
    name: String,
    elapsed: std::time::Duration,
    last: Instant,
    ended: bool,
    started: bool,
    iteration: f64,
    delay: f64,
    /// Whether script paused (`true`) or played it, over `animation-play-state`.
    held: Option<bool>,
    /// Script canceled it, so it stays off until its name changes.
    canceled: bool,
}

#[derive(Clone)]
struct Transition {
    serial: u32,
    property: Prop,
    from: String,
    to: String,
    start: Instant,
    /// The seconds since `start` where script paused it.
    held: Option<f64>,
    delay: f64,
    duration: f64,
    timing: String,
    reverse: String,
    shortening: f64,
    started: bool,
}

impl Transition {
    fn elapsed(&self, now: Instant) -> f64 {
        self.held
            .unwrap_or_else(|| now.saturating_duration_since(self.start).as_secs_f64())
    }

    fn progress(&self, now: Instant) -> Option<f64> {
        let time = self.elapsed(now) - self.delay;
        (time < self.duration).then(|| easing(&self.timing, (time / self.duration).max(0.)))
    }
}

/// An animation from `Element.animate()`. Script keeps its timing model and sends the current time, so this
/// only runs the clock between updates.
pub(crate) struct Scripted {
    id: u32,
    /// Keyframes as `(offset, declarations, easing)`.
    rules: Vec<(f64, Style, String)>,
    effect: Animation,
    /// What owns the style that the keyframes were resolved against.
    base: Option<Rc<dyn std::any::Any>>,
    /// The current time in seconds at `since`, and the playback rate while time runs.
    time: f64,
    rate: Option<f64>,
    since: Instant,
}

impl Scripted {
    /// A script animation from its keyframes `(offset, CSS declarations, easing)`, its timing in seconds
    /// `[delay, end delay, duration, iterations, iteration start]`, `[direction, fill, easing, composite]` and
    /// pseudo-element slot.
    pub fn new(
        id: u32,
        keyframes: &[(f64, &str, String)],
        [delay, end_delay, duration, iterations, iteration_start]: [f64; 5],
        [direction, fill, easing, composite]: [&str; 4],
        pseudo: u8,
    ) -> Self {
        Self {
            id,
            rules: keyframes
                .iter()
                .map(|(offset, source, easing)| {
                    (*offset, super::declarations(source), easing.clone())
                })
                .collect(),
            effect: Animation {
                name: String::new(),
                paused: true,
                duration,
                delay,
                iterations,
                direction: direction.into(),
                fill: fill.into(),
                timing: "linear".into(),
                easing: easing.into(),
                end_delay,
                iteration_start,
                composite: composite.into(),
                pseudo,
                tracks: Vec::new(),
            },
            base: None,
            time: 0.,
            rate: None,
            since: Instant::now(),
        }
    }

    fn local(&self, now: Instant) -> f64 {
        self.time
            + self.rate.map_or(0., |rate| {
                now.saturating_duration_since(self.since).as_secs_f64() * rate
            })
    }

    /// The style it animates and its owner: the element's, or one of its pseudo-elements'.
    fn base<'a>(
        &self,
        id: usize,
        styles: &'a ComputedStyles,
    ) -> Option<(Rc<dyn std::any::Any>, &'a Style)> {
        match self.effect.pseudo {
            0 => Some((styles.values[id].clone(), &*styles.values[id])),
            slot => {
                let pair = styles.pseudos.get(&id)?;
                Some((pair.clone(), &pair[usize::from(slot) - 1]))
            }
        }
    }

    /// Resolves the keyframes again when the style changed.
    fn resolve(&mut self, owner: Rc<dyn std::any::Any>, base: &Style, registry: &Registry) {
        if self
            .base
            .as_ref()
            .is_none_or(|old| !Rc::ptr_eq(old, &owner))
        {
            self.effect.tracks = tracks(base, &self.rules, registry);
            self.base = Some(owner);
        }
    }
}

#[derive(Default)]
pub(crate) struct Motion {
    previous: Vec<Rc<Style>>,
    clocks: HashMap<usize, Vec<Clock>>,
    transitions: HashMap<usize, Vec<Transition>>,
    /// The last animated style of each node, with its base style and animated values.
    styled: HashMap<usize, (Rc<Style>, Style, Rc<Style>)>,
    pub custom: HashMap<usize, Style>,
    /// The timer that wakes the view for the next animation change.
    pub wake: Option<(Instant, renderer::Timer)>,
    /// Nodes whose animations wait until their box comes into view, from the last update.
    pub parked: Vec<usize>,
    /// The animations from `Element.animate()` of each node, in composite order.
    scripted: HashMap<usize, Vec<Scripted>>,
    /// The last id of a CSS animation or transition, so script can find them.
    serial: u32,
    /// Events from script controls, for the next update.
    queued: Vec<(usize, &'static str, String, f64)>,
    /// The animated `::before` and `::after` styles of the last update.
    pub pseudos: Vec<(usize, Rc<[Style; 2]>)>,
}

impl Motion {
    /// The style of node `id` at each of `times`, sampled like `update` from its last clocks without
    /// changing them or sending events, so a compositor can play it.
    pub fn timeline(&self, id: usize, styles: &ComputedStyles, times: &[Instant]) -> Vec<Style> {
        let registry = &*styles.registry;
        let base = &*styles.values[id];
        let effects = styles.animations.get(&id).map_or(&[][..], Vec::as_slice);
        let clocks = self.clocks.get(&id).map_or(&[][..], Vec::as_slice);
        times
            .iter()
            .map(|&at| {
                let mut out = Style::default();
                for (effect, clock) in effects.iter().zip(clocks) {
                    let paused = clock.held.unwrap_or(effect.paused);
                    if clock.canceled || effect.pseudo != 0 {
                        continue;
                    }
                    let mut elapsed = clock.elapsed;
                    if !paused {
                        elapsed += at.saturating_duration_since(clock.last);
                    }
                    let held;
                    let effect = if paused == effect.paused {
                        effect
                    } else {
                        held = Animation {
                            paused,
                            ..effect.clone()
                        };
                        &held
                    };
                    effect.sample(elapsed.as_secs_f64(), base, registry, &mut out);
                }
                for animation in self.scripted.get(&id).into_iter().flatten() {
                    if animation.effect.pseudo == 0 {
                        let local = animation.local(at);
                        animation.effect.sample(local, base, registry, &mut out);
                    }
                }
                for transition in self.transitions.get(&id).into_iter().flatten() {
                    if let Some(progress) = transition.progress(at)
                        && let Some(value) = interpolate(
                            transition.property.name(),
                            &transition.from,
                            &transition.to,
                            progress,
                        )
                    {
                        out.insert(transition.property, value);
                    }
                }
                let mut style = base.clone();
                style.extend(out.iter().map(|(key, value)| (key, value.clone())));
                style
            })
            .collect()
    }

    /// When the next animation or transition event of node `id` fires after `now`, if any.
    pub fn next_event(&self, id: usize, styles: &ComputedStyles, now: Instant) -> Option<Instant> {
        let effects = styles.animations.get(&id).map_or(&[][..], Vec::as_slice);
        let clocks = self.clocks.get(&id).map_or(&[][..], Vec::as_slice);
        let animations = effects.iter().zip(clocks).filter_map(|(effect, clock)| {
            if clock.held.unwrap_or(effect.paused) || clock.canceled || clock.ended {
                return None;
            }
            let elapsed = (clock.elapsed + now.saturating_duration_since(clock.last)).as_secs_f64();
            let active = elapsed - effect.delay;
            let end = effect.duration * effect.iterations;
            let at = if active < 0. {
                0.
            } else if effect.duration > 0. {
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
                    let time = transition.elapsed(now) - transition.delay;
                    transition.held.is_none().then(|| {
                        if time < 0. {
                            -time
                        } else {
                            transition.duration - time
                        }
                    })
                });
        animations
            .chain(transitions)
            .filter(|seconds| seconds.is_finite())
            .map(|seconds| now + std::time::Duration::from_secs_f64(seconds.max(0.)))
            .min()
    }

    pub fn forget(&mut self, id: usize) {
        if let Some(previous) = self.previous.get_mut(id) {
            *previous = Rc::default();
        }
        self.clocks.remove(&id);
        self.transitions.remove(&id);
        self.scripted.remove(&id);
    }

    /// Adds or replaces the script animation `id` of `node`, at `time` seconds and running at `rate`, or
    /// paused with `None`.
    pub fn script(&mut self, node: usize, mut animation: Scripted, time: f64, rate: Option<f64>) {
        animation.time = time;
        animation.rate = rate.filter(|rate| *rate != 0.);
        animation.effect.paused = animation.rate.is_none();
        animation.since = Instant::now();
        self.unscript(animation.id);
        let list = self.scripted.entry(node).or_default();
        let index = list.partition_point(|other| other.id < animation.id);
        list.insert(index, animation);
    }

    /// Removes the script animation `id`, and returns its node.
    pub fn unscript(&mut self, id: u32) -> Option<usize> {
        let node = self.scripted.iter().find_map(|(&node, list)| {
            list.iter()
                .any(|animation| animation.id == id)
                .then_some(node)
        })?;
        let list = self.scripted.get_mut(&node)?;
        list.retain(|animation| animation.id != id);
        if list.is_empty() {
            self.scripted.remove(&node);
        }
        Some(node)
    }

    /// The CSS transitions and then the CSS animations of `nodes` that script sees, as `[serial, node, kind, name,
    /// time, state, [delay, end delay, duration, iterations, iteration start], direction, fill, easing, frames]`
    /// with times in ms, infinite iterations as -1 and frames as `[offset, property, value, easing]`.
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
                    transition.elapsed(now) * 1000.,
                    if transition.held.is_some() {
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
                    transition.timing,
                    [
                        [0., name, transition.from, "linear"],
                        [1., name, transition.to, "linear"]
                    ],
                ]));
            }
        }
        for &node in nodes {
            let (Some(clocks), Some(effects)) =
                (self.clocks.get(&node), styles.animations.get(&node))
            else {
                continue;
            };
            for (clock, effect) in clocks.iter().zip(effects) {
                // Finished animations that don't fill are gone for script, like canceled ones.
                if clock.canceled
                    || effect.pseudo != 0 && !pseudos
                    || clock.ended && !matches!(effect.fill.as_str(), "forwards" | "both")
                {
                    continue;
                }
                let paused = clock.held.unwrap_or(effect.paused);
                let length = if effect.duration == 0. {
                    0.
                } else {
                    effect.duration * effect.iterations
                };
                let time = (clock.elapsed.as_secs_f64()
                    + if paused {
                        0.
                    } else {
                        now.saturating_duration_since(clock.last).as_secs_f64()
                    })
                .min(effect.delay + length);
                let frames: Vec<_> = effect
                    .tracks
                    .iter()
                    .flat_map(|(prop, keys)| {
                        keys.iter().map(move |(offset, value, timing)| {
                            serde_json::json!([
                                offset,
                                prop.name(),
                                value,
                                if timing.is_empty() {
                                    &effect.timing
                                } else {
                                    timing
                                }
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
                    if clock.ended {
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
                    effect.direction,
                    effect.fill,
                    "linear",
                    frames,
                    pseudo,
                ]));
            }
        }
        list
    }

    /// Runs a script control on the CSS animation or transition `serial`: `pause`, `play`, `seek` to `time` ms,
    /// `finish` or `cancel`. Returns whether it found it.
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
                .and_then(|effects| effects.get(index));
            let paused = clock
                .held
                .unwrap_or(effect.is_some_and(|effect| effect.paused));
            if !paused && !clock.canceled {
                clock.elapsed += now.saturating_duration_since(clock.last);
            }
            clock.last = now;
            let end = effect.map_or(0., |effect| {
                effect.delay
                    + if effect.duration == 0. {
                        0.
                    } else {
                        effect.duration * effect.iterations
                    }
            });
            match action {
                "pause" => clock.held = Some(true),
                "play" => {
                    if clock.ended || clock.canceled {
                        clock.elapsed = Default::default();
                        (clock.ended, clock.started, clock.canceled, clock.iteration) =
                            (false, false, false, 0.);
                    }
                    clock.held = Some(false);
                }
                "seek" => {
                    clock.elapsed = std::time::Duration::from_secs_f64((time / 1000.).max(0.))
                }
                "finish" if end.is_finite() => {
                    clock.elapsed = std::time::Duration::from_secs_f64(end.max(0.))
                }
                "cancel" => {
                    if clock.started && !clock.ended && !clock.canceled {
                        let active = (clock.elapsed.as_secs_f64() - clock.delay).max(0.);
                        self.queued
                            .push((node, "animationcancel", clock.name.clone(), active));
                    }
                    clock.canceled = true;
                }
                _ => return false,
            }
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
            let elapsed = transition.elapsed(now);
            let after =
                |seconds: f64| now.checked_sub(std::time::Duration::from_secs_f64(seconds.max(0.)));
            match action {
                "pause" => transition.held = Some(elapsed),
                "play" => {
                    if let Some(held) = transition.held.take() {
                        transition.start = after(held).unwrap_or(now);
                    }
                }
                "seek" if transition.held.is_some() => {
                    transition.held = Some((time / 1000.).max(0.))
                }
                "seek" => transition.start = after(time / 1000.).unwrap_or(now),
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

    /// The values that the script animations of `node` or its pseudo-element `pseudo` give it now.
    pub fn scripted(&mut self, node: usize, pseudo: u8, styles: &ComputedStyles) -> Option<Style> {
        let now = Instant::now();
        let list = self.scripted.get_mut(&node)?;
        let mut out = Style::new();
        for animation in list
            .iter_mut()
            .filter(|animation| animation.effect.pseudo == pseudo)
        {
            let Some((owner, base)) = animation.base(node, styles) else {
                continue;
            };
            animation.resolve(owner, base, &styles.registry);
            animation
                .effect
                .sample(animation.local(now), base, &styles.registry, &mut out);
        }
        (!out.is_empty()).then_some(out)
    }

    /// Samples the animations and transitions at `now`. `shown` tells whether a node's box was in view
    /// at the last layout; animations that can't move an unshown box don't wake the view.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        doc: &Document,
        styles: &ComputedStyles,
        visible: &[bool],
        shown: &dyn Fn(usize) -> bool,
        now: Instant,
        repass: bool,
        events: &mut Vec<(usize, &'static str, String, f64)>,
    ) -> (Animated, Option<Instant>) {
        let registry = &*styles.registry;
        events.append(&mut self.queued);
        self.previous
            .resize_with(styles.values.len(), Default::default);
        for (id, new) in styles.values.iter().enumerate() {
            if Rc::ptr_eq(&self.previous[id], new) {
                continue;
            }
            let old = std::mem::replace(&mut self.previous[id], new.clone());
            // WARNING: a container query pass recomputes styles inside one frame, so it must not start transitions.
            if repass {
                continue;
            }
            let running = self.transitions.remove(&id).unwrap_or_default();
            let cancel =
                |kept: &[Transition], events: &mut Vec<(usize, &'static str, String, f64)>| {
                    for transition in &running {
                        if !kept.iter().any(|next| {
                            next.property == transition.property && next.serial == transition.serial
                        }) {
                            let time = transition.elapsed(now) - transition.delay;
                            events.push((
                                id,
                                "transitioncancel",
                                transition.property.name().to_owned(),
                                time.clamp(0., transition.duration),
                            ));
                        }
                    }
                };
            let old = match styles.starting.get(&id) {
                Some(starting)
                    if old.is_empty()
                        || old.get(Prop::DISPLAY).is_some_and(|value| value == "none") =>
                {
                    starting.clone()
                }
                _ => old,
            };
            if old.is_empty()
                || !visible.get(id).copied().unwrap_or(false)
                || old.get(Prop::DISPLAY).is_some_and(|value| value == "none")
                || !new.contains(Prop::TRANSITION) && !new.contains(Prop::TRANSITION_PROPERTY)
            {
                cancel(&[], events);
                continue;
            }
            let mut properties: Vec<(Prop, f64, f64, String)> = Vec::new();
            for (name, duration, delay, timing) in transitions(new) {
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
                    properties.push((name, duration, delay, timing.clone()));
                }
            }
            let mut next = Vec::new();
            for (property, duration, delay, timing) in properties {
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
                        let current = progress
                            .and_then(|progress| {
                                interpolate(property.name(), &old.from, &old.to, progress)
                            })
                            .unwrap_or_else(|| old.to.clone());
                        if old.reverse == after {
                            let factor = (progress.unwrap_or(1.) * old.shortening + 1.
                                - old.shortening)
                                .abs()
                                .clamp(0., 1.);
                            (current, old.to.clone(), factor)
                        } else {
                            (current.clone(), current, 1.)
                        }
                    }
                    None => match computed(&old, property, registry) {
                        Some(before) => (before.to_owned(), before.to_owned(), 1.),
                        None => continue,
                    },
                };
                if from == after || interpolate(property.name(), &from, after, 0.5).is_none() {
                    continue;
                }
                self.serial += 1;
                next.push(Transition {
                    serial: self.serial,
                    property,
                    from,
                    to: after.to_owned(),
                    start: now,
                    held: None,
                    delay: if delay < 0. { delay * factor } else { delay },
                    duration: duration.max(0.) * factor,
                    timing,
                    reverse,
                    shortening: factor,
                    started: false,
                });
                let delay = next.last().map_or(0., |transition| transition.delay);
                events.push((
                    id,
                    "transitionrun",
                    property.name().to_owned(),
                    (-delay).max(0.),
                ));
            }
            cancel(&next, events);
            if !next.is_empty() {
                self.transitions.insert(id, next);
            }
        }
        let cancel =
            |id: usize, clock: &Clock, events: &mut Vec<(usize, &'static str, String, f64)>| {
                if clock.started && !clock.ended && !clock.canceled {
                    events.push((
                        id,
                        "animationcancel",
                        clock.name.clone(),
                        (clock.elapsed.as_secs_f64() - clock.delay).max(0.),
                    ));
                }
            };
        self.clocks.retain(|&id, clocks| {
            let kept = visible.get(id) == Some(&true) && styles.animations.contains_key(&id);
            if !kept {
                for clock in clocks.iter() {
                    cancel(id, clock, events);
                }
            }
            kept
        });
        self.transitions.retain(|&id, list| {
            let kept = visible.get(id) == Some(&true);
            if !kept {
                for transition in list.iter() {
                    let time = transition.elapsed(now) - transition.delay;
                    events.push((
                        id,
                        "transitioncancel",
                        transition.property.name().to_owned(),
                        time.clamp(0., transition.duration),
                    ));
                }
            }
            kept
        });
        let mut animated: HashMap<usize, Style> = HashMap::new();
        let mut pseudo: HashMap<(usize, u8), Style> = HashMap::new();
        let mut wakes: Vec<(usize, bool, bool, f64, f64)> = Vec::new();
        for (&id, effects) in &styles.animations {
            if visible.get(id) != Some(&true) {
                continue;
            }
            let clocks = self.clocks.entry(id).or_default();
            for clock in clocks.iter().skip(effects.len()) {
                cancel(id, clock, events);
            }
            clocks.truncate(effects.len());
            for (index, effect) in effects.iter().enumerate() {
                if clocks
                    .get(index)
                    .is_none_or(|clock| clock.name != effect.name)
                {
                    self.serial += 1;
                    let clock = Clock {
                        serial: self.serial,
                        name: effect.name.clone(),
                        elapsed: Default::default(),
                        last: now,
                        ended: false,
                        started: false,
                        iteration: 0.,
                        delay: effect.delay,
                        held: None,
                        canceled: false,
                    };
                    if index == clocks.len() {
                        clocks.push(clock);
                    } else {
                        cancel(id, &clocks[index], events);
                        clocks[index] = clock;
                    }
                }
                let clock = &mut clocks[index];
                let paused = clock.held.unwrap_or(effect.paused);
                if !paused && !clock.canceled {
                    clock.elapsed += now.saturating_duration_since(clock.last);
                }
                clock.last = now;
                if clock.canceled {
                    continue;
                }
                let held;
                let effect = if paused == effect.paused {
                    effect
                } else {
                    held = Animation {
                        paused,
                        ..effect.clone()
                    };
                    &held
                };
                let elapsed = clock.elapsed.as_secs_f64();
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
                let (wake, ended) = effect.sample(elapsed, base, registry, out);
                let active = elapsed - effect.delay;
                if !clock.started && active >= 0. {
                    clock.started = true;
                    events.push((
                        id,
                        "animationstart",
                        effect.name.clone(),
                        (-effect.delay).max(0.),
                    ));
                }
                if clock.started && effect.duration > 0. && !ended {
                    let iteration = (active / effect.duration).floor().max(0.);
                    if iteration > clock.iteration {
                        events.push((
                            id,
                            "animationiteration",
                            effect.name.clone(),
                            iteration * effect.duration,
                        ));
                    }
                    clock.iteration = iteration;
                }
                if ended && !clock.ended {
                    events.push((
                        id,
                        "animationend",
                        effect.name.clone(),
                        effect.duration * effect.iterations,
                    ));
                }
                clock.ended = ended;
                let end = if effect.paused || ended {
                    f64::INFINITY
                } else {
                    effect.delay
                        + if effect.duration == 0. {
                            0.
                        } else {
                            effect.duration * effect.iterations
                        }
                        - elapsed
                };
                wakes.push((
                    id,
                    effect.tracks.iter().all(|(key, _)| PAINT.contains(key)),
                    effect.tracks.iter().all(|(key, frames)| {
                        still(*key, frames.iter().map(|(_, value, _)| value.as_str()))
                    }),
                    if effect.tracks.is_empty() {
                        end
                    } else {
                        wake.unwrap_or(f64::INFINITY)
                    },
                    end,
                ));
            }
        }
        // Script animations come after CSS animations in composite order. Script runs their finish, so only
        // their changes wake the view.
        for (&id, list) in &mut self.scripted {
            if visible.get(id) != Some(&true) {
                continue;
            }
            for animation in list.iter_mut() {
                let Some((owner, base)) = animation.base(id, styles) else {
                    continue;
                };
                animation.resolve(owner, base, registry);
                let out = match animation.effect.pseudo {
                    0 => animated.entry(id).or_default(),
                    slot => pseudo.entry((id, slot)).or_default(),
                };
                let local = animation.local(now);
                let (wake, _) = animation.effect.sample(local, base, registry, out);
                let Some(rate) = animation.rate else {
                    continue;
                };
                let change = if rate > 0. {
                    wake.map_or(f64::INFINITY, |wake| wake / rate)
                } else if local > 0. {
                    0.
                } else {
                    f64::INFINITY
                };
                let tracks = &animation.effect.tracks;
                wakes.push((
                    id,
                    tracks.iter().all(|(key, _)| PAINT.contains(key)),
                    tracks.iter().all(|(key, frames)| {
                        still(*key, frames.iter().map(|(_, value, _)| value.as_str()))
                    }),
                    change,
                    f64::INFINITY,
                ));
            }
        }
        for (&id, list) in &mut self.transitions {
            let out = animated.entry(id).or_default();
            list.retain_mut(|transition| match transition.progress(now) {
                Some(progress) => {
                    if !transition.started && transition.elapsed(now) >= transition.delay {
                        transition.started = true;
                        events.push((
                            id,
                            "transitionstart",
                            transition.property.name().to_owned(),
                            (-transition.delay).max(0.),
                        ));
                    }
                    if let Some(value) = interpolate(
                        transition.property.name(),
                        &transition.from,
                        &transition.to,
                        progress,
                    ) {
                        out.insert(transition.property, value);
                    }
                    let time = transition.elapsed(now) - transition.delay;
                    // A transition that script paused never changes.
                    let held = transition.held.map(|_| f64::INFINITY);
                    wakes.push((
                        id,
                        PAINT.contains(&transition.property),
                        still(
                            transition.property,
                            [transition.from.as_str(), transition.to.as_str()].into_iter(),
                        ),
                        held.unwrap_or((-time).max(0.)),
                        held.unwrap_or(transition.duration - time),
                    ));
                    true
                }
                None => {
                    events.push((
                        id,
                        "transitionend",
                        transition.property.name().to_owned(),
                        transition.duration,
                    ));
                    false
                }
            });
        }
        self.transitions.retain(|_, list| !list.is_empty());
        animated.retain(|_, values| !values.is_empty());
        self.custom.clear();
        if animated.values().any(|values| !values.vars().is_empty()) {
            self.custom.clone_from(&animated);
        }
        let mut inherited = Vec::new();
        for (&id, values) in &animated {
            for (key, value) in values.iter() {
                if !key.inherited() {
                    continue;
                }
                let base = styles.values[id].get(key);
                let mut pending = vec![id];
                while let Some(parent) = pending.pop() {
                    for &child in styles
                        .children
                        .get(&parent)
                        .unwrap_or(&doc.nodes[parent].children)
                    {
                        if styles.values[child].get(key) == base
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
        // WARNING: the inline text of a paragraph holds the paint of its inline boxes, so render rebuilds it only for a new color.
        let layout: HashSet<usize> = animated
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
        // WARNING: paint-only changes under a non-animated `opacity: 0` or inside a fully hidden
        // subtree can't be seen, so they only wake the view for their end events.
        let fixed =
            |id: usize, key: Prop| animated.get(&id).is_none_or(|values| !values.contains(key));
        let mut next = f64::INFINITY;
        self.parked.clear();
        for (id, paint, still, change, end) in wakes {
            let unseen = paint && {
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
            // WARNING: scrolls, resizes and DOM changes draw a frame, and render checks parked boxes after
            // it lays them out, so their animations go on once they come into view.
            let parked = !unseen && still && change < end && !shown(id);
            if parked && self.parked.last() != Some(&id) {
                self.parked.push(id);
            }
            next = next.min(if unseen || parked { end } else { change });
        }
        self.styled.retain(|id, _| animated.contains_key(id));
        let mut animated: Animated = animated
            .into_iter()
            .map(|(id, values)| {
                let base = &styles.values[id];
                let style = match self.styled.get(&id) {
                    Some((old, sampled, style)) if Rc::ptr_eq(old, base) && *sampled == values => {
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
        // Animated pseudo-elements draw from their own styles, and their host rebuilds with its own one.
        let mut pairs: HashMap<usize, [Style; 2]> = HashMap::new();
        for ((id, slot), values) in pseudo {
            let Some(pair) = styles.pseudos.get(&id).filter(|_| !values.is_empty()) else {
                continue;
            };
            let layout = values.iter().any(|(key, _)| !PAINT.contains(&key));
            pairs.entry(id).or_insert_with(|| (**pair).clone())[usize::from(slot) - 1]
                .extend(values.iter().map(|(key, value)| (key, value.clone())));
            match animated.iter_mut().find(|(node, ..)| *node == id) {
                Some(entry) => entry.2 |= layout,
                None => animated.push((id, styles.values[id].clone(), layout)),
            }
        }
        self.pseudos = pairs
            .into_iter()
            .map(|(id, pair)| (id, Rc::new(pair)))
            .collect();
        let next = std::time::Duration::try_from_secs_f64(next.max(0.))
            .ok()
            .and_then(|delay| now.checked_add(delay));
        (animated, next)
    }
}

/// `composite: add` of an animation value onto the value below it, or with `accumulate`, matching transform and
/// filter functions combine their arguments. `None` where values don't add, so the animation value replaces.
fn add(property: &str, under: &str, value: &str, accumulate: bool) -> Option<String> {
    // A number with its unit, like `10px` or `0.5`.
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
                            // Scales and the filters that are 1 when unchanged combine as `a + b - 1`.
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

/// Whether animated paint values keep the box where it is, so they can't move it into view.
fn still<'a>(prop: Prop, mut values: impl Iterator<Item = &'a str>) -> bool {
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
        && !matches!(
            key,
            "display" | "position" | "content" | "font-size" | "line-height" | "will-change"
        )
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

fn transitions(style: &Style) -> Vec<(String, f64, f64, String)> {
    const DEFAULT: [&str; 4] = ["all", "0s", "0s", "ease"];
    let shorthand: Vec<[&str; 4]> = style
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
                } else if !matches!(part, "normal" | "allow-discrete") {
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
    ]
    .map(|prop| style.get(prop).map(|value| components(value, ',')));
    (0..longhands[0].as_ref().map_or(shorthand.len(), Vec::len))
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
                parts[3].to_owned(),
            ))
        })
        .collect()
}

pub(crate) fn interpolate(property: &str, from: &str, to: &str, t: f64) -> Option<String> {
    if from == to || t == 0. {
        return Some(from.to_owned());
    }
    if t == 1. {
        return Some(to.to_owned());
    }
    // WARNING: animations mix the same values every frame, so their parsed pairs are kept by a hash
    // of the property and values, checked against them.
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
            let lerp = Lerp::new(property, from, to);
            lerps.insert(key, (property.into(), from.into(), to.into(), lerp));
        }
        lerps[&key].3.as_ref().map(|lerp| lerp.at(t))
    })
}

/// Parsed pairs by a hash of the property and values, with those values.
type Lerps = rustc_hash::FxHashMap<u64, (String, String, String, Option<Lerp>)>;

/// Two values parsed once, so each frame only mixes numbers and writes them.
#[derive(Clone, Debug)]
pub(crate) enum Lerp {
    Same(String),
    /// `visibility`: the end values outside (0, 1), `visible` between them.
    Visible(String, String),
    List(Vec<Lerp>, &'static str),
    Function(String, Box<Lerp>),
    Color((u32, u8), (u32, u8)),
    /// Two numbers and their unit, rounded to integers for `z-index` and `order`.
    Number(f64, f64, String, bool),
    /// Two lengths in other units, mixed in `calc()`.
    Calc(String, String, bool),
}

impl Lerp {
    pub(crate) fn new(property: &str, from: &str, to: &str) -> Option<Self> {
        if from == to {
            return Some(Self::Same(from.to_owned()));
        }
        if property == "visibility" && (from == "visible" || to == "visible") {
            return Some(Self::Visible(from.to_owned(), to.to_owned()));
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
                "transform" => components(other, ' ')
                    .into_iter()
                    .map(|part| {
                        let (name, args) = part.strip_suffix(')')?.split_once('(')?;
                        let args = match name {
                            "matrix" => "1, 0, 0, 1, 0, 0".to_owned(),
                            "matrix3d" => {
                                "1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1".to_owned()
                            }
                            "perspective" => return None,
                            _ => components(args, ',')
                                .iter()
                                .enumerate()
                                .map(|(index, arg)| {
                                    if name.starts_with("scale") {
                                        "1".to_owned()
                                    } else if name == "rotate3d" && index < 3 {
                                        (*arg).to_owned()
                                    } else {
                                        format!(
                                            "0{}",
                                            arg.trim_start_matches(|c: char| {
                                                c.is_ascii_digit() || ".+-".contains(c)
                                            })
                                        )
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", "),
                        };
                        Some(format!("{name}({args})"))
                    })
                    .collect::<Option<Vec<_>>>()?
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
            return Some(Self::Color(a, b));
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

    pub(crate) fn at(&self, t: f64) -> String {
        let mut out = String::new();
        self.write(t, &mut out);
        out
    }

    fn write(&self, t: f64, out: &mut String) {
        use std::fmt::Write;
        // Six significant digits, like browsers serialize animated values.
        let round = |value: f64, integer: bool| {
            if integer {
                value.round().to_string()
            } else {
                super::value::number(value)
            }
        };
        match self {
            Self::Same(value) => out.push_str(value),
            Self::Visible(from, to) => out.push_str(if t <= 0. {
                from
            } else if t >= 1. {
                to
            } else {
                "visible"
            }),
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
                let _ = write!(out, "#{rgb:06x}{:02x}", alpha.round().clamp(0., 255.) as u8);
            }
            Self::Number(a, b, unit, integer) => {
                let _ = write!(out, "{}{unit}", round(a + (b - a) * t, *integer));
            }
            Self::Calc(from, to, integer) => {
                let _ = write!(
                    out,
                    "calc({from} * {} + {to} * {})",
                    round(1. - t, *integer),
                    round(t, *integer)
                );
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

/// The points `(input, output)` of a `linear()` easing, or `None` with fewer than two.
fn linear_points(value: &str) -> Option<Rc<[(f64, f64)]>> {
    type Points = HashMap<String, Option<Rc<[(f64, f64)]>>>;
    thread_local! {
        static POINTS: std::cell::RefCell<Points> = Default::default();
    }
    if let Some(points) = POINTS.with_borrow(|cache| cache.get(value).cloned()) {
        return points;
    }
    let mut stops: Vec<(Option<f64>, f64)> = Vec::new();
    for part in components(
        value.trim_start_matches("linear(").trim_end_matches(')'),
        ',',
    ) {
        let mut items = components(part, ' ').into_iter();
        let Some(output) = items.next().and_then(|value| value.parse::<f64>().ok()) else {
            continue;
        };
        let inputs: Vec<f64> = items
            .filter_map(|value| value.strip_suffix('%')?.parse::<f64>().ok())
            .map(|value| value / 100.)
            .collect();
        if inputs.is_empty() {
            stops.push((None, output));
        }
        stops.extend(
            inputs
                .into_iter()
                .take(2)
                .map(|input| (Some(input), output)),
        );
    }
    let points = (stops.len() >= 2).then(|| {
        // The first input defaults to 0, the last to 1, inputs never go back, and missing ones spread out evenly.
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
            let [from, to] = [stops[index - 1].0, stops[end].0].map(|input| input.unwrap_or(1.));
            for (step, stop) in stops[index..end].iter_mut().enumerate() {
                stop.0 = Some(from + (to - from) * (step + 1) as f64 / (end - index + 1) as f64);
            }
            index = end;
        }
        stops
            .into_iter()
            .map(|(input, output)| (input.unwrap_or(1.), output))
            .collect::<Rc<[_]>>()
    });
    POINTS.with_borrow_mut(|cache| {
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(value.to_owned(), points.clone());
    });
    points
}

pub(crate) fn easing(value: &str, progress: f64) -> f64 {
    let curve = match value {
        "linear" => return progress,
        _ if value.starts_with("linear(") => {
            let Some(points) = linear_points(value) else {
                return progress;
            };
            // Inputs before the first point or after the last extend the first or last segment.
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
    let bezier = |t: f64, a: f64, b: f64| {
        3. * (1. - t) * (1. - t) * t * a + 3. * (1. - t) * t * t * b + t * t * t
    };
    // Newton's method first and bisection when it stalls, both to 1e-7 like browsers, so a symmetric curve
    // gives exactly 0.5 at its middle.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Document;
    use std::time::Duration;

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
            let (wake, _) = effect.sample(elapsed, &base, &Default::default(), &mut out);
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
        finite.direction = "alternate".into();
        finite.fill = "both".into();
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
        styles.animations[&div][0].sample(1., &styles.values[div], &Default::default(), &mut out);
        assert_eq!(&out["transform"], "translateX(50px) rotate(45deg)");
        assert_eq!(&out["background-color"], "#800080ff");
        assert_eq!(&out["width"], "50%");
        out = Style::new();
        styles.animations[&div][0].sample(0.5, &styles.values[div], &Default::default(), &mut out);
        assert_eq!(&out["width"], "calc(10px * 0.5 + 50% * 0.5)");
        out = Style::new();
        styles.animations[&p][0].sample(0.25, &styles.values[p], &Default::default(), &mut out);
        assert_eq!(&out["transform"], "rotate(90deg)");
    }

    #[test]
    fn values_interpolate_by_type() {
        let mix = |property: &str, from: &str, to: &str, t: f64| interpolate(property, from, to, t);
        assert_eq!(mix("color", "red", "blue", 0.5).unwrap(), "#800080ff");
        assert_eq!(
            mix("color", "transparent", "red", 0.5).unwrap(),
            "#ff000080"
        );
        assert_eq!(mix("width", "0", "10px", 0.5).unwrap(), "5px");
        assert_eq!(mix("opacity", "1", "0", 0.25).unwrap(), "0.75");
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
        assert_eq!(mix("transform", "rotate(10deg)", "scale(2)", 0.5), None);
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
        let mut motion = Motion::default();
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

    /// Runs a page like the view does: a frame at each requested wake, on the next 120 Hz
    /// vsync. Returns the frames drawn in `seconds` and the wake delays.
    fn frames(html: &str, seconds: f64) -> (usize, Vec<Option<f64>>) {
        let doc = Document::parse(&format!(
            "<style>@keyframes spin{{to{{transform:rotate(360deg)}}}}@keyframes blink{{50%{{opacity:0}}}}@keyframes grow{{to{{width:10px}}}}@keyframes still{{to{{display:none}}}}@keyframes show{{to{{opacity:1}}}}</style>{html}"
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
            animated[0].1.clone()
        };
        let (half, end, later) = (style(0.5), style(2.), style(3.));
        assert!(!Rc::ptr_eq(&half, &end) && Rc::ptr_eq(&end, &later));
        assert_eq!(&later["width"], "10px");
    }

    #[test]
    fn linear_easing_spreads_missing_inputs_and_extrapolates() {
        assert_eq!(easing("linear(0, 1)", 0.3), 0.3);
        assert!((easing("linear(0, 0.25 50%, 1)", 0.25) - 0.125).abs() < 1e-9);
        assert!((easing("linear(0, 0.25 50%, 1)", 0.75) - 0.625).abs() < 1e-9);
        // `0.5` has no input, so it goes halfway between 0 and 40%.
        assert!((easing("linear(0, 0.5, 1 40%, 1)", 0.2) - 0.5).abs() < 1e-9);
        // Two inputs hold the output, and inputs never go back.
        assert_eq!(easing("linear(0, 0.5 20% 60%, 1)", 0.4), 0.5);
        assert_eq!(easing("linear(0, 1 50%, 0.2 30%)", 0.5), 0.2);
        assert!((easing("linear(0, 1)", 1.5) - 1.5).abs() < 1e-9);
        assert!((easing("linear(0, 1.2 60%, 1)", 0.6) - 1.2).abs() < 1e-9);
        assert_eq!(easing("linear(1)", 0.4), 0.4);
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
                .sample(local, &base, &Default::default(), &mut out);
            out.get(Prop::OPACITY)
                .map(|value| (value.parse::<f64>().unwrap() * 1e4).round() / 1e4)
        };
        let new = |frames: &[(f64, &str, String)], timing, direction, fill, easing| {
            let mut animation =
                Scripted::new(1, frames, timing, [direction, fill, easing, "replace"], 0);
            animation.resolve(base.clone(), &base, &Default::default());
            animation
        };
        let frames = [
            (0., "opacity:0", "linear".into()),
            (1., "opacity:1", "linear".into()),
        ];
        // Iterations start halfway, and every other one runs backwards.
        let alternate = new(
            &frames,
            [0., 0., 1., 3., 0.5],
            "alternate",
            "none",
            "linear",
        );
        assert_eq!(opacity(&alternate, 0.6), Some(0.9));
        assert_eq!(opacity(&alternate, 3.), None);
        // A negative end delay cuts the active interval, where a finished animation holds its time.
        let cut = new(&frames, [0.2, -0.5, 1., 1., 0.], "normal", "both", "linear");
        assert_eq!(opacity(&cut, 0.), Some(0.));
        assert_eq!(opacity(&cut, 0.5), Some(0.3));
        assert_eq!(opacity(&cut, 0.7), Some(0.5));
        // The effect easing applies to the whole iteration and can overshoot the keyframes.
        let spring = new(
            &frames,
            [0., 0., 1., 1., 0.],
            "normal",
            "none",
            "linear(0, 1.2 60%, 1)",
        );
        assert_eq!(opacity(&spring, 0.6), Some(1.2));
        // A single keyframe animates from the element's own value.
        let single = new(
            &[(1., "opacity:1", "linear".into())],
            [0., 0., 1., 1., 0.],
            "normal",
            "none",
            "linear",
        );
        assert_eq!(opacity(&single, 0.5), Some(0.75));
        // Motion keeps the clock between updates, and paused animations never wake the view.
        let mut motion = Motion::default();
        motion.script(
            id,
            new(&frames, [0., 0., 10., 1., 0.], "normal", "none", "linear"),
            2.,
            Some(1.),
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
        assert_eq!(motion.unscript(1), Some(id));
        assert!(motion.scripted(id, 0, &styles).is_none());
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
        // Unregistered custom properties switch halfway.
        let own = animated.iter().find(|(id, ..)| *id == div).unwrap();
        assert_eq!(own.1.get(Prop::new("--x")), Some("b"));
        // `::before` animates its own style, and its host draws again.
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
        assert!(easing("steps()", 0.5).is_finite());
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
                (Prop::TRANSITION, "opacity .2s ease-in 1s, all .3s".into()),
                (Prop::new("transition-timing-function"), "linear".into())
            ])),
            vec![
                ("opacity".into(), 0.2, 1., "linear".into()),
                ("all".into(), 0.3, 0., "linear".into())
            ]
        );
    }
}
