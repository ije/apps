use super::{
    ComputedStyles, Declarations, Prop, Style, color, components, expand, resolve_keywords,
    resolve_style,
};
use crate::dom::Document;
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Instant,
};

const PAINT: &[Prop] = &[
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
    timing: String,
    tracks: Vec<(Prop, Keyframes)>,
}

type Keyframes = Vec<(f64, String, String)>;

/// The animated style of each node, and whether it changes layout.
type Animated = Vec<(usize, Rc<Style>, bool)>;

impl Animation {
    pub fn parse(
        style: &Style,
        keyframes: &HashMap<String, Vec<(f32, Declarations)>>,
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
                let rules = keyframes.get(name)?;
                let mut tracks: Vec<(Prop, Keyframes)> = Vec::new();
                for (offset, declarations) in rules {
                    let declarations = declarations.resolve();
                    let timing = declarations
                        .get(Prop::ANIMATION_TIMING_FUNCTION)
                        .map(str::to_owned)
                        .unwrap_or_default();
                    let mut frame = style.clone();
                    frame.extend(
                        declarations
                            .iter()
                            .filter(|(_, value)| !value.ends_with("!important"))
                            .map(|(key, value)| (key, value.clone())),
                    );
                    resolve_style(&mut frame, Some(style));
                    resolve_keywords(&mut frame, Some(style));
                    for (key, value) in declarations.iter() {
                        if value.ends_with("!important") || !animatable(key) {
                            continue;
                        }
                        let Some(value) = computed(&frame, key) else {
                            continue;
                        };
                        let frame = (f64::from(*offset), value.to_owned(), timing.clone());
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
                    tracks,
                })
            })
            .collect()
    }

    /// Writes the animated values into `out`. Returns the seconds until the values can change
    /// next (0 while they change continuously, `None` when they never change), and whether the
    /// animation has ended.
    pub fn sample(&self, elapsed: f64, base: &Style, out: &mut Style) -> (Option<f64>, bool) {
        let time = elapsed - self.delay;
        let end = if self.duration == 0. {
            0.
        } else {
            self.duration * self.iterations
        };
        let ended = time >= end;
        let active = !self.paused && !ended && time >= 0.;
        let mut wake = (!self.paused && !ended).then(|| if time < 0. { -time } else { end - time });
        if time < 0. && !matches!(self.fill.as_str(), "backwards" | "both")
            || ended && !matches!(self.fill.as_str(), "forwards" | "both")
        {
            return (wake, ended);
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
        if ended && overall > 0. && progress == 0. {
            progress = 1.;
            iteration -= 1.;
        }
        if active {
            wake = wake.map(|wake| wake.min((iteration + 1.) * self.duration - time));
        }
        let reverse = self.direction == "reverse"
            || self.direction == "alternate" && iteration % 2. == 1.
            || self.direction == "alternate-reverse" && iteration % 2. == 0.;
        if reverse {
            progress = 1. - progress;
        }
        for (property, frames) in &self.tracks {
            let mut keys: Vec<(f64, &str, &str)> = frames
                .iter()
                .map(|(at, value, timing)| (*at, value.as_str(), timing.as_str()))
                .collect();
            if let Some(value) = computed(base, *property) {
                if keys[0].0 > 0. {
                    keys.insert(0, (0., value, ""));
                }
                if keys[keys.len() - 1].0 < 1. {
                    keys.push((1., value, ""));
                }
            }
            let index = keys
                .iter()
                .rposition(|key| key.0 <= progress)
                .unwrap_or(0)
                .min(keys.len().saturating_sub(2));
            let (from, to) = (keys[index], keys[(index + 1).min(keys.len() - 1)]);
            let local = if to.0 > from.0 {
                ((progress - from.0) / (to.0 - from.0)).clamp(0., 1.)
            } else {
                1.
            };
            let timing = if from.2.is_empty() {
                &self.timing
            } else {
                from.2
            };
            if active && to.0 > from.0 {
                let steps = if from.1 == to.1 || matches!(timing, "step-start" | "step-end") {
                    Some(1.)
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
            let value = interpolate(property.name(), from.1, to.1, local)
                .unwrap_or_else(|| if local < 0.5 { from.1 } else { to.1 }.to_owned());
            out.insert(*property, value);
        }
        (wake, ended)
    }
}

struct Clock {
    name: String,
    elapsed: std::time::Duration,
    last: Instant,
    ended: bool,
}

#[derive(Clone)]
struct Transition {
    property: Prop,
    from: String,
    to: String,
    start: Instant,
    delay: f64,
    duration: f64,
    timing: String,
    reverse: String,
    shortening: f64,
}

impl Transition {
    fn progress(&self, now: Instant) -> Option<f64> {
        let time = now.saturating_duration_since(self.start).as_secs_f64() - self.delay;
        (time < self.duration).then(|| easing(&self.timing, (time / self.duration).max(0.)))
    }
}

#[derive(Default)]
pub(crate) struct Motion {
    previous: Vec<Rc<Style>>,
    clocks: HashMap<usize, Vec<Clock>>,
    transitions: HashMap<usize, Vec<Transition>>,
    /// The last animated style of each node, with its base style and animated values.
    styled: HashMap<usize, (Rc<Style>, Style, Rc<Style>)>,
    /// The timer that wakes the view for the next animation change.
    pub wake: Option<(Instant, renderer::Timer)>,
}

impl Motion {
    pub fn forget(&mut self, id: usize) {
        if let Some(previous) = self.previous.get_mut(id) {
            *previous = Rc::default();
        }
        self.clocks.remove(&id);
        self.transitions.remove(&id);
    }

    pub fn update(
        &mut self,
        doc: &Document,
        styles: &ComputedStyles,
        visible: &[bool],
        now: Instant,
        repass: bool,
        events: &mut Vec<(usize, &'static str, String, f64)>,
    ) -> (Animated, Option<Instant>) {
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
            if old.is_empty()
                || !visible.get(id).copied().unwrap_or(false)
                || old.get(Prop::DISPLAY).is_some_and(|value| value == "none")
                || !new.contains(Prop::TRANSITION) && !new.contains(Prop::TRANSITION_PROPERTY)
            {
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
                        .filter(|&key| animatable(key) && old.get(key) != new.get(key))
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
                let Some(after) = computed(new, property) else {
                    continue;
                };
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
                    None => match computed(&old, property) {
                        Some(before) => (before.to_owned(), before.to_owned(), 1.),
                        None => continue,
                    },
                };
                if from == after || interpolate(property.name(), &from, after, 0.5).is_none() {
                    continue;
                }
                next.push(Transition {
                    property,
                    from,
                    to: after.to_owned(),
                    start: now,
                    delay: if delay < 0. { delay * factor } else { delay },
                    duration: duration.max(0.) * factor,
                    timing,
                    reverse,
                    shortening: factor,
                });
            }
            if !next.is_empty() {
                self.transitions.insert(id, next);
            }
        }
        self.clocks
            .retain(|&id, _| visible.get(id) == Some(&true) && styles.animations.contains_key(&id));
        self.transitions
            .retain(|&id, _| visible.get(id) == Some(&true));
        let mut animated: HashMap<usize, Style> = HashMap::new();
        let mut wakes: Vec<(usize, bool, f64, f64)> = Vec::new();
        for (&id, effects) in &styles.animations {
            if visible.get(id) != Some(&true) {
                continue;
            }
            let clocks = self.clocks.entry(id).or_default();
            clocks.truncate(effects.len());
            let out = animated.entry(id).or_default();
            for (index, effect) in effects.iter().enumerate() {
                if clocks
                    .get(index)
                    .is_none_or(|clock| clock.name != effect.name)
                {
                    let clock = Clock {
                        name: effect.name.clone(),
                        elapsed: Default::default(),
                        last: now,
                        ended: false,
                    };
                    if index == clocks.len() {
                        clocks.push(clock);
                    } else {
                        clocks[index] = clock;
                    }
                }
                let clock = &mut clocks[index];
                if !effect.paused {
                    clock.elapsed += now.saturating_duration_since(clock.last);
                }
                clock.last = now;
                let elapsed = clock.elapsed.as_secs_f64();
                let (wake, ended) = effect.sample(elapsed, &styles.values[id], out);
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
                    if effect.tracks.is_empty() {
                        end
                    } else {
                        wake.unwrap_or(f64::INFINITY)
                    },
                    end,
                ));
            }
        }
        for (&id, list) in &mut self.transitions {
            let out = animated.entry(id).or_default();
            list.retain(|transition| match transition.progress(now) {
                Some(progress) => {
                    if let Some(value) = interpolate(
                        transition.property.name(),
                        &transition.from,
                        &transition.to,
                        progress,
                    ) {
                        out.insert(transition.property, value);
                    }
                    let time = now
                        .saturating_duration_since(transition.start)
                        .as_secs_f64()
                        - transition.delay;
                    wakes.push((
                        id,
                        PAINT.contains(&transition.property),
                        (-time).max(0.),
                        transition.duration - time,
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
        let mut layout: HashSet<usize> = inherited.iter().map(|(id, ..)| *id).collect();
        for (id, key, value) in inherited {
            animated.entry(id).or_default().insert(key, value);
        }
        for (&id, values) in &animated {
            if styles.values[id]
                .get(Prop::DISPLAY)
                .is_none_or(|value| matches!(value, "inline" | "contents"))
                || values.iter().any(|(key, _)| !PAINT.contains(&key))
            {
                layout.insert(id);
            }
        }
        // WARNING: paint-only changes under a non-animated `opacity: 0` or inside a fully hidden
        // subtree can't be seen, so they only wake the view for their end events.
        let fixed =
            |id: usize, key: Prop| animated.get(&id).is_none_or(|values| !values.contains(key));
        let mut next = f64::INFINITY;
        for (id, paint, change, end) in wakes {
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
            next = next.min(if unseen { end } else { change });
        }
        self.styled.retain(|id, _| animated.contains_key(id));
        let animated = animated
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
        let next = std::time::Duration::try_from_secs_f64(next.max(0.))
            .ok()
            .and_then(|delay| now.checked_add(delay));
        (animated, next)
    }
}

fn animatable(prop: Prop) -> bool {
    let key = prop.name();
    !prop.is_custom()
        && !key.starts_with("animation")
        && !key.starts_with("transition")
        && !key.starts_with("container")
        && !matches!(
            key,
            "display" | "position" | "content" | "font-size" | "line-height" | "will-change"
        )
}

fn computed(style: &Style, prop: Prop) -> Option<&str> {
    let key = prop.name();
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
    if property == "visibility" && (from == "visible" || to == "visible") {
        return Some(
            if t <= 0. {
                from
            } else if t >= 1. {
                to
            } else {
                "visible"
            }
            .to_owned(),
        );
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
            return Some("none".into());
        }
        return a
            .iter()
            .zip(&b)
            .map(|(a, b)| interpolate("", a, b, t))
            .collect::<Option<Vec<_>>>()
            .map(|list| list.join(", "));
    }
    if from == "none" || to == "none" {
        let other = if from == "none" { to } else { from };
        let identity = match property {
            "translate" | "rotate" | "scale" => components(other, ' ')
                .into_iter()
                .map(|part| match property {
                    "scale" => "1",
                    "rotate"
                        if part.starts_with(|c: char| c.is_ascii_digit() || ".+-".contains(c)) =>
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
                        "matrix3d" => "1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1".to_owned(),
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
            interpolate(property, &identity, to, t)
        } else {
            interpolate(property, from, &identity, t)
        };
    }
    if let (Some(a), Some(b)) = (color(from), color(to)) {
        let alpha = f64::from(a.1) + (f64::from(b.1) - f64::from(a.1)) * t;
        let mut rgb = 0;
        for shift in [16, 8, 0] {
            let channel =
                |(value, alpha): (u32, u8)| f64::from((value >> shift) as u8) * f64::from(alpha);
            let premultiplied = channel(a) + (channel(b) - channel(a)) * t;
            rgb = rgb << 8
                | if alpha > 0. {
                    (premultiplied / alpha).round().clamp(0., 255.) as u32
                } else {
                    0
                };
        }
        return Some(format!(
            "#{rgb:06x}{:02x}",
            alpha.round().clamp(0., 255.) as u8
        ));
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
                .map(|(a, b)| interpolate(property, a, b, t))
                .collect::<Option<Vec<_>>>()
                .map(|list| list.join(if separator == ',' { ", " } else { " " }));
        }
    }
    fn function(value: &str) -> Option<(&str, &str)> {
        value
            .strip_suffix(')')?
            .split_once('(')
            .filter(|(name, _)| !matches!(*name, "calc" | "min" | "max" | "clamp" | "var" | "url"))
    }
    if let (Some((name, a)), Some((other, b))) = (function(from), function(to))
        && name == other
    {
        return interpolate(property, a, b, t).map(|args| format!("{name}({args})"));
    }
    fn number(value: &str) -> Option<(f64, &str)> {
        let end = value
            .char_indices()
            .find(|&(index, c)| !(c.is_ascii_digit() || c == '.' || index == 0 && "+-".contains(c)))
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
    let round = |value: f64| {
        if matches!(property, "z-index" | "order") {
            value.round()
        } else {
            (value * 1e4).round() / 1e4
        }
    };
    match (number(from), number(to)) {
        (Some((a, unit)), Some((b, other)))
            if unit == other || a == 0. && unit.is_empty() || b == 0. && other.is_empty() =>
        {
            Some(format!(
                "{}{}",
                round(a + (b - a) * t),
                if unit.is_empty() { other } else { unit }
            ))
        }
        _ if length(from) && length(to) => {
            let term = |value: &str| value.strip_prefix("calc").unwrap_or(value).to_owned();
            Some(format!(
                "calc({} * {} + {} * {})",
                term(from),
                round(1. - t),
                term(to),
                round(t)
            ))
        }
        _ => None,
    }
}

fn timing_function(part: &str) -> bool {
    matches!(
        part,
        "ease" | "linear" | "ease-in" | "ease-out" | "ease-in-out" | "step-start" | "step-end"
    ) || part.starts_with("cubic-bezier(")
        || part.starts_with("steps(")
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
            let (wake, _) = effect.sample(elapsed, &base, &mut out);
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
        styles.animations[&div][0].sample(1., &styles.values[div], &mut out);
        assert_eq!(&out["transform"], "translateX(50px) rotate(45deg)");
        assert_eq!(&out["background-color"], "#800080ff");
        assert_eq!(&out["width"], "50%");
        out = Style::new();
        styles.animations[&div][0].sample(0.5, &styles.values[div], &mut out);
        assert_eq!(&out["width"], "calc(10px * 0.5 + 50% * 0.5)");
        out = Style::new();
        styles.animations[&p][0].sample(0.25, &styles.values[p], &mut out);
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
                let (animated, next) =
                    motion.update(doc, styles, &visible, at(time), false, &mut events);
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
            let (_, next) = motion.update(&doc, &styles, &visible, now, false, &mut Vec::new());
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
            let (animated, _) = motion.update(&doc, &styles, &visible, now, false, &mut Vec::new());
            animated[0].1.clone()
        };
        let (half, end, later) = (style(0.5), style(2.), style(3.));
        assert!(!Rc::ptr_eq(&half, &end) && Rc::ptr_eq(&end, &later));
        assert_eq!(&later["width"], "10px");
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
                &frames
            )
            .is_empty()
        );
        assert!(
            Animation::parse(
                &Style::from_iter([
                    (Prop::new("animation"), "1s pulse".into()),
                    (Prop::new("animation-iteration-count"), "NaN".into())
                ]),
                &frames
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
