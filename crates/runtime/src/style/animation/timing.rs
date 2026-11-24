use crate::css::{
    properties::Prop, properties::Registry, properties::Style, value::easing::Easing,
    value::shorthand::expand,
};
use crate::dom::Document;
use crate::style::cascade::computed::ComputedStyles;
use crate::style::keyframes::interpolate::{
    Lerp, PAINT, animatable, computed, interpolate, keeps_box_in_place, lerp, transitions,
};
use crate::style::keyframes::{Animation, COMPOSITES, DIRECTIONS, FILLS, keyword, tracks};
use std::borrow::Cow;
use std::rc::Rc;
use std::time::Instant;
// Base rate and time changes on the current local time. Use f64: large page times can exceed Duration.
#[derive(Clone)]
pub(crate) struct Playback {
    pub(crate) time: f64,
    pub(crate) since: Instant,
    pub(crate) rate: f64,
}

impl Playback {
    pub(crate) fn at(&self, now: Instant) -> f64 {
        if self.rate == 0. {
            return self.time;
        }
        self.time + now.saturating_duration_since(self.since).as_secs_f64() * self.rate
    }

    pub(crate) fn set(&mut self, now: Instant, rate: f64) {
        self.time = self.at(now);
        self.since = now;
        self.rate = rate;
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Phase {
    Pending,
    Running,
    Ended,
    Canceled,
}

pub(crate) struct Clock {
    pub(crate) serial: u32,
    pub(crate) name: String,
    pub(crate) playback: Playback,
    pub(crate) phase: Phase,
    pub(crate) iteration: f64,
    pub(crate) delay: f64,
    pub(crate) held: Option<bool>,
}

#[derive(Clone)]
pub(crate) struct Transition {
    pub(crate) serial: u32,
    pub(crate) property: Prop,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) playback: Playback,
    pub(crate) delay: f64,
    pub(crate) duration: f64,
    pub(crate) timing: Easing,
    pub(crate) lerp: Option<Rc<Lerp>>,
    pub(crate) reverse: String,
    pub(crate) shortening: f64,
    pub(crate) started: bool,
}

impl Transition {
    pub(crate) fn progress(&self, now: Instant) -> Option<f64> {
        let time = self.playback.at(now) - self.delay;
        (time < self.duration).then(|| self.timing.at((time / self.duration).max(0.), false))
    }

    pub(crate) fn value(&self, progress: f64) -> Rc<str> {
        let near = if progress < 0.5 { &self.from } else { &self.to };
        interpolate(self.lerp.as_deref(), &self.from, &self.to, progress)
            .unwrap_or_else(|| near.as_str().into())
    }

    pub(crate) fn shows(&self, now: Instant) -> bool {
        self.property == Prop::DISPLAY
            && self
                .progress(now)
                .is_some_and(|progress| &*self.value(progress) != "none")
    }
}

pub(crate) fn show(
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
        for &child in styles.flat_children(doc, id) {
            if painted.get(child) == Some(&false)
                && (styles.values[child].get(Prop::DISPLAY) != Some("none") || shows(child))
            {
                pending.push(child);
            }
        }
    }
}

pub(crate) type Events = Vec<(usize, &'static str, String, f64)>;

pub(crate) fn retarget(
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

pub(crate) fn cancel(
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

pub(crate) type Wake = (usize, bool, bool, f64, f64);

pub(crate) fn advance(
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

pub(crate) fn after(now: Instant, seconds: f64) -> Option<Instant> {
    std::time::Duration::try_from_secs_f64(seconds.max(0.))
        .ok()
        .and_then(|delay| now.checked_add(delay))
}

pub(crate) fn drop_important(styles: &ComputedStyles, id: usize, out: &mut Style) {
    if let Some(props) = styles.extras(id).important.as_ref() {
        for &prop in props.iter() {
            out.remove(prop);
        }
    }
}

pub(crate) struct Scripted {
    pub(crate) id: u32,
    pub(crate) rules: Vec<(f64, Style, Option<Easing>)>,
    pub(crate) effect: Animation,
    pub(crate) base: Option<(Rc<dyn std::any::Any>, crate::css::properties::Vars)>,
    pub(crate) playback: Playback,
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
                        crate::css::sheet::declarations(source),
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
    pub(crate) fn sample<'a>(
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
                let pair = styles.generated(id)?;
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
