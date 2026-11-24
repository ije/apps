use crate::css::{properties::Prop, properties::Style};
use crate::dom::Document;
use crate::style::animation::timing::{
    Clock, Events, Phase, Playback, Transition, Wake, advance, after, cancel, drop_important,
    retarget, show,
};
use crate::style::cascade::computed::ComputedStyles;
use crate::style::keyframes::interpolate::PAINT;
use crate::style::keyframes::{Animated, Animation, DIRECTIONS, FILLS, TRANSITIONS};
use rustc_hash::{FxHashMap, FxHashSet};
use std::{borrow::Cow, collections::HashMap, rc::Rc, time::Instant};
use timing::Scripted;

pub(crate) mod timing;

#[derive(Default)]
pub(crate) struct Motion {
    previous: Vec<Rc<Style>>,
    // Keep previous equal to seen except at touched indices. Updates visit only changed chunks.
    seen: super::Chunked<Rc<Style>>,
    touched: Vec<usize>,
    clocks: FxHashMap<usize, Vec<Clock>>,
    transitions: FxHashMap<usize, Vec<Transition>>,
    styled: FxHashMap<usize, (Rc<Style>, Style, Rc<Style>)>,
    parked: Vec<usize>,
    scripted: FxHashMap<usize, Vec<Scripted>>,
    script_nodes: FxHashMap<u32, usize>,
    serial: u32,
    queued: Events,
    backdrop: FxHashMap<usize, (Rc<Style>, Vec<Transition>)>,
    iterations: bool,
}

// What one `update` of the animations gives the frame.
#[derive(Default)]
pub(crate) struct Frame {
    // The nodes with animated values: their style, and whether it changes layout.
    pub(crate) animated: Animated,
    pub(crate) next: Option<Instant>,
    // Animated custom properties, which need a style pass.
    pub(crate) custom: HashMap<usize, Style>,
    pub(crate) pseudos: Vec<(usize, Rc<[Style; 2]>)>,
    pub(crate) backdrops: Vec<(usize, Rc<Style>)>,
    // The visibility of each node when animations changed it.
    pub(crate) painted: Option<Vec<bool>>,
}

impl Motion {
    // Fires `animationiteration` events from now on: a page that never listened for them is not woken for them.
    pub(crate) fn listen_iterations(&mut self) {
        self.iterations = true;
    }

    // The nodes whose animations paused because they are out of view.
    pub(crate) fn parked(&self) -> &[usize] {
        &self.parked
    }

    // Clocks line up with the effects only after `update`, so a style change before it skips them.
    fn pairs<'a>(
        &'a self,
        id: usize,
        styles: &'a ComputedStyles,
    ) -> impl Iterator<Item = (&'a Animation, &'a Clock)> {
        let effects = styles
            .extras(id)
            .animations
            .as_ref()
            .map_or(&[][..], |effects| effects);
        effects
            .iter()
            .zip(self.clocks.get(&id).into_iter().flatten())
            .filter(|(effect, clock)| effect.name == clock.name)
    }

    /// The styles of a node, or of its `::before` (`slot` 1) or `::after` (`slot` 2), at each time.
    pub fn timeline(
        &self,
        id: usize,
        slot: u8,
        styles: &ComputedStyles,
        times: &[Instant],
        props: &[Prop],
    ) -> Vec<Style> {
        let registry = &*styles.registry;
        let base = match slot {
            0 => &*styles.values[id],
            slot => match styles.generated(id) {
                Some(pair) => &pair[usize::from(slot) - 1],
                None => return Vec::new(),
            },
        };
        times
            .iter()
            .map(|&at| {
                let mut out = Style::default();
                for (effect, clock) in self.pairs(id, styles) {
                    if clock.phase != Phase::Canceled && effect.pseudo == slot {
                        let paused = clock.playback.rate == 0.;
                        effect.sample(clock.playback.at(at), paused, base, registry, &mut out, 1.);
                    }
                }
                for animation in self.scripted.get(&id).into_iter().flatten() {
                    if animation.effect.pseudo == slot {
                        let (local, paused) =
                            (animation.playback.at(at), animation.playback.rate == 0.);
                        animation
                            .effect
                            .sample(local, paused, base, registry, &mut out, 1.);
                    }
                }
                if slot == 0 {
                    drop_important(styles, id, &mut out);
                }
                for transition in self
                    .transitions
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .filter(|_| slot == 0)
                {
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

    /// The loop of the one animation of a node, or of its `::before` (`slot` 1) or `::after` (`slot` 2).
    pub fn period(
        &self,
        id: usize,
        slot: u8,
        styles: &ComputedStyles,
        now: Instant,
    ) -> Option<(f64, f64, f64, Option<String>)> {
        if slot == 0
            && self
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
            .filter(|(effect, _)| effect.pseudo == slot)
        {
            effects += 1;
            let local = playback.at(now);
            let stop = effect.delay + effect.active() + effect.end_delay.min(0.);
            if playback.rate == 0. || local >= stop {
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

    /// A CSS animation or transition of the node itself sets `prop`.
    pub fn animates(&self, id: usize, styles: &ComputedStyles, prop: Prop) -> bool {
        self.transitions
            .get(&id)
            .is_some_and(|list| list.iter().any(|transition| transition.property == prop))
            || self.pairs(id, styles).any(|(effect, _)| {
                effect.pseudo == 0 && effect.tracks.iter().any(|(track, _)| *track == prop)
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

    /// Whether the next frame starts no CSS animation or transition on these nodes: none has a new style
    /// that can start one.
    pub fn settled(&self, nodes: &[usize], styles: &ComputedStyles) -> bool {
        let moves = |style: &Style| {
            TRANSITIONS.iter().any(|&prop| style.contains(prop))
                || [Prop::ANIMATION, Prop::ANIMATION_NAME]
                    .iter()
                    .any(|&prop| style.get(prop).is_some_and(|value| value != "none"))
        };
        nodes.iter().all(|&id| {
            styles.values.get(id).is_none_or(|style| {
                self.previous
                    .get(id)
                    .is_some_and(|previous| Rc::ptr_eq(previous, style))
                    || !moves(style)
                        && styles
                            .generated(id)
                            .is_none_or(|pseudos| !pseudos.iter().any(moves))
            })
        })
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
                .extras(node)
                .animations
                .as_ref()
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
            animation.sample(node, styles, now, || &mut out, 1.);
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
        // The shorter side of a box on the page, in CSS pixels.
        side: &dyn Fn(usize) -> f32,
        now: Instant,
        repass: bool,
        events: &mut Events,
    ) -> Frame {
        let mut frame = Frame::default();
        let registry = &*styles.registry;
        let coarse = |id: usize| f64::from((48. / side(id).max(1.)).clamp(1., 4.));
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
                animation.sample(id, styles, now, || &mut out, 1.);
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
            let old = match styles.extras(id).starting.as_ref() {
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
        for (id, new) in styles
            .all_extras()
            .filter_map(|(id, extras)| Some((id, extras.backdrop.as_ref()?)))
        {
            let (old, running) = match gone.remove(&id) {
                Some((old, running)) => (Some(old), running),
                None => (styles.extras(id).backdrop_start.clone(), Vec::new()),
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
            let kept = painted.get(id) == Some(&true) && styles.extras(id).animations.is_some();
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
        for (id, effects) in styles
            .all_extras()
            .filter_map(|(id, extras)| Some((id, extras.animations.as_ref()?)))
        {
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
                        let Some(pair) = styles.generated(id) else {
                            continue;
                        };
                        (
                            &pair[usize::from(slot) - 1],
                            pseudo.entry((id, slot)).or_default(),
                        )
                    }
                };
                let (wake, ended) = effect.sample(elapsed, paused, base, registry, out, coarse(id));
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
                let Some(wake) = animation.sample(
                    id,
                    styles,
                    now,
                    || match slot {
                        0 => animated.entry(id).or_default(),
                        slot => pseudo.entry((id, slot)).or_default(),
                    },
                    coarse(id),
                ) else {
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
        frame.backdrops.clear();
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
                    frame.backdrops.push((id, Rc::new(out)));
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
                        pending.extend(styles.flat_children(doc, id));
                    }
                }
            }
        }
        frame.custom.clear();
        if animated.values().any(|values| {
            !values.vars().is_empty()
                || values.contains(Prop::FONT_SIZE)
                || values.contains(Prop::LINE_HEIGHT)
        }) {
            frame.custom = animated
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
                    for &child in styles.flat_children(doc, parent) {
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
                        pending.extend(styles.flat_children(doc, node));
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
        frame.painted = match painted {
            Cow::Owned(painted) => Some(painted),
            Cow::Borrowed(_) => None,
        };
        let mut pairs: FxHashMap<usize, [Style; 2]> = FxHashMap::default();
        let mut only = Vec::new();
        for ((id, slot), values) in pseudo {
            let Some(pair) = styles.generated(id).filter(|_| !values.is_empty()) else {
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
        frame.pseudos = pairs
            .into_iter()
            .map(|(id, pair)| (id, Rc::new(pair)))
            .collect();
        frame.animated = animated;
        frame.next = after(now, next);
        frame
    }
}

#[cfg(test)]
mod tests;
