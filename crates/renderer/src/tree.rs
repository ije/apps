use crate::{
    AvailableSpace, Baseline, Bounds, ContentMask, Display, ElementId, Hitbox, HitboxBehavior,
    Hooks, Pixels, Point, Size, Style, TextStyle, Visibility, Window,
    geometry::round_half_up,
    node::{Child, Content, Events, MeasureFn, Measurement, Node, NodeCx},
    point, px, quad, size,
    text::TextState,
};
use rustc_hash::{FxHashMap, FxHasher};
use std::{
    any::Any,
    hash::{Hash, Hasher},
    mem,
    rc::Rc,
    sync::Arc,
};
use taffy::{
    AlignItemsKeyword, Baselines, BlockContext, Cache, CacheTree, ClearState, FlexDirection,
    Layout, LayoutBlockContainer, LayoutContainingBlock, LayoutFlexboxContainer,
    LayoutGridContainer, LayoutInput, LayoutOutput, LayoutPartialTree, NodeId, RunMode,
    TraversePartialTree,
};

const NONE: u32 = u32::MAX;

enum Kind {
    Box,
    Image(Arc<crate::RenderImage>, crate::ObjectFit),
    Text(bool),
    Measured,
    Layers(bool),
}

struct Slot {
    key: (u64, u32),
    live: bool,
    parent: u32,
    children: Vec<NodeId>,
    style: Rc<Style>,
    taffy: taffy::Style,
    events: Option<Box<Events>>,
    hooks: Option<Rc<dyn Hooks>>,
    kind: Kind,
    measure: Option<Box<MeasureFn>>,
    state: Option<Rc<dyn Any>>,
    inherited: Rc<TextStyle>,
    text: Rc<TextStyle>,
    cache: Cache,
    layout: Layout,
    baselines: Baselines,
    origin: Point<f32>,
    bounds: [Bounds<Pixels>; 3],
    hitbox: u32,
    listed: bool,
}

impl Slot {
    fn new(key: (u64, u32), inherited: Rc<TextStyle>, style: Rc<Style>) -> Self {
        Self {
            key,
            live: true,
            parent: NONE,
            children: Vec::new(),
            style,
            taffy: taffy::Style::default(),
            events: None,
            hooks: None,
            kind: Kind::Box,
            measure: None,
            state: None,
            text: inherited.clone(),
            inherited,
            cache: Cache::new(),
            layout: Layout::new(),
            baselines: Baselines::NONE,
            origin: Point::default(),
            bounds: Default::default(),
            hitbox: NONE,
            listed: false,
        }
    }
}

#[derive(Default)]
pub struct Tree {
    slots: Vec<Slot>,
    free: Vec<u32>,
    keys: FxHashMap<(u64, u32), u32>,
    occurrences: FxHashMap<u64, u32>,
    scopes: Vec<u64>,
    pass: u32,
    seen: Vec<u32>,
    orphans: Vec<u32>,
    root: Option<u32>,
    measured_with: (f32, usize),
    stretch: Option<Size<Pixels>>,
    hitboxes: Vec<Hitbox>,
    layers: FxHashMap<u32, Vec<(i32, u32, ContentMask<Pixels>)>>,
    kids: Vec<NodeId>,
    initial: Rc<TextStyle>,
    blank: Rc<Style>,
    styles: Interned<Style>,
    texts: Interned<TextStyle>,
    stacks: Vec<Vec<(i32, u32, ContentMask<Pixels>)>>,
    painting: Vec<u32>,
}

fn identity(id: Option<ElementId>, scope: u64) -> u64 {
    let mut hasher = FxHasher::default();
    (id.map(|id| id.0), scope).hash(&mut hasher);
    hasher.finish()
}

impl Tree {
    pub(crate) fn begin(&mut self, scale: f32, fonts: usize) {
        if mem::replace(&mut self.measured_with, (scale, fonts)) != (scale, fonts) {
            self.slots.clear();
            self.seen.clear();
            self.free.clear();
            self.keys.clear();
            self.orphans.clear();
            self.root = None;
        }
        self.pass = self.pass.wrapping_add(1);
        self.occurrences.clear();
        self.scopes.clear();
    }

    pub fn contains(&self, id: impl Into<ElementId>) -> bool {
        self.keys.contains_key(&(identity(Some(id.into()), 0), 0))
    }

    pub(crate) fn commit_root(
        &mut self,
        root: Child,
        viewport: Size<Pixels>,
        scale: f32,
    ) -> Option<NodeId> {
        let inherited = self.initial.clone();
        self.stretch = Some(viewport);
        let root = self.commit(root, &inherited, scale);
        self.stretch = None;
        if let Some(root) = root {
            self.slots[usize::from(root)].parent = NONE;
            self.orphans.extend(self.root.replace(index_of(root)));
        }
        // WARNING: marks hold for one pass only, so the dropped boxes are freed before the next one. Freeing can
        // compact the arena, which moves the root.
        self.free_orphans();
        root.and(self.root).map(|root| NodeId::from(root as usize))
    }

    fn commit(&mut self, child: Child, inherited: &Rc<TextStyle>, scale: f32) -> Option<NodeId> {
        match child {
            Child::Kept(id) => {
                let index = self.keys.get(&(identity(Some(id), 0), 0)).copied();
                debug_assert!(index.is_some(), "kept box {id:?} is not in the tree");
                let index = index?;
                self.seen[index as usize] = self.pass;
                self.inherit(index, inherited, scale);
                Some(NodeId::from(index as usize))
            }
            Child::Node(node) => Some(self.commit_node(node, inherited, scale)),
        }
    }

    fn inherit(&mut self, index: u32, inherited: &Rc<TextStyle>, scale: f32) {
        let slot = &mut self.slots[index as usize];
        if Rc::ptr_eq(&slot.inherited, inherited) {
            return;
        }
        if *slot.inherited == **inherited {
            slot.inherited = inherited.clone();
            return;
        }
        slot.inherited = inherited.clone();
        let text = if slot.style.text.is_empty() {
            inherited.clone()
        } else {
            let mut text = (**inherited).clone();
            text.refine(&slot.style.text);
            self.texts.get(text)
        };
        let slot = &mut self.slots[index as usize];
        if *text != *slot.text {
            if let Kind::Text(own) = slot.kind
                && let Some(state) = slot
                    .state
                    .as_ref()
                    .and_then(|state| state.downcast_ref::<TextState>())
            {
                let (state, measure) = text_measurement(state.restyled(own, &text, scale));
                slot.state = Some(state);
                slot.measure = Some(measure);
            }
            if slot.measure.is_some() {
                self.dirty(index);
            }
        }
        let slot = &mut self.slots[index as usize];
        slot.text = text.clone();
        for child in 0..slot.children.len() {
            let child = index_of(self.slots[index as usize].children[child]);
            self.inherit(child, &text, scale);
        }
    }

    fn commit_node(&mut self, node: Node, inherited: &Rc<TextStyle>, scale: f32) -> NodeId {
        let Node {
            id,
            mut style,
            children,
            events,
            hooks,
            content,
        } = node;
        let stretch = self.stretch.take();
        let identity = identity(id, if id.is_some() { 0 } else { self.scope() });
        let occurrence = self.occurrences.entry(identity).or_default();
        let key = (identity, *occurrence);
        *occurrence += 1;
        let kept = self.keys.get(&key).copied();
        let text = match kept.map(|index| &self.slots[index as usize]) {
            _ if style.text.is_empty() => inherited.clone(),
            Some(slot)
                if Rc::ptr_eq(&slot.inherited, inherited) && slot.style.text == style.text =>
            {
                slot.text.clone()
            }
            _ => {
                let mut text = (**inherited).clone();
                text.refine(&style.text);
                self.texts.get(text)
            }
        };
        if id.is_some() {
            self.scopes.push(identity);
        }
        let start = self.kids.len();
        for child in children {
            if let Some(child) = self.commit(child, &text, scale) {
                self.kids.push(child);
            }
        }
        if id.is_some() {
            self.scopes.pop();
        }
        let (kind, measurement) = match content {
            Content::None => (Kind::Box, None),
            Content::Image(image, fit) => (Kind::Image(image, fit), None),
            Content::Text(content, runs) => {
                let own = runs.is_some();
                let state = TextState::new(content, runs, &text, scale);
                (
                    Kind::Text(own),
                    Some(Box::new(TextMeasurement(state)) as Box<dyn Measurement>),
                )
            }
            Content::Measured(measurement) => (Kind::Measured, Some(measurement)),
            Content::Layers(negative) => (Kind::Layers(negative), None),
        };
        if style.display == Display::None {
            style.z_index = None;
        }
        let restyled = kept.is_none_or(|index| *self.slots[index as usize].style != style);
        let mut taffy = match kept {
            Some(_) if !restyled && stretch.is_none() => None,
            _ => Some(style.to_taffy(scale)),
        };
        if let Some(viewport) = stretch
            && let Some(taffy) = &mut taffy
        {
            let length = |value: Pixels| {
                taffy::Dimension::length(crate::geometry::to_layout_unit(value.0, scale))
            };
            if taffy.size.width.is_auto() {
                taffy.size.width = length(viewport.width);
            }
            if taffy.size.height.is_auto() {
                taffy.size.height = length(viewport.height);
            }
        }
        let index = match kept {
            Some(index) => index,
            None => {
                let slot = Slot::new(key, inherited.clone(), self.blank.clone());
                let index = match self.free.pop() {
                    Some(index) => {
                        self.slots[index as usize] = slot;
                        index
                    }
                    None => {
                        self.slots.push(slot);
                        self.seen.push(0);
                        (self.slots.len() - 1) as u32
                    }
                };
                self.keys.insert(key, index);
                index
            }
        };
        self.seen[index as usize] = self.pass;
        let slot = &mut self.slots[index as usize];
        let same_measure = match &measurement {
            Some(measurement) => {
                measurement.same(slot.state.as_ref())
                    && *slot.text == *text
                    && slot.measure.is_some()
            }
            None => slot.measure.is_none(),
        };
        let kids = &self.kids[start..];
        let relisted = slot.children != kids;
        let relaid = kept.is_none()
            || !same_measure
            || relisted
            || taffy.as_ref().is_some_and(|taffy| slot.taffy != *taffy)
            || restyled
                && (slot.style.baseline != style.baseline
                    || slot.style.baseline_source != style.baseline_source);
        if !same_measure {
            match measurement {
                Some(measurement) => {
                    let (state, measure) = measurement.build();
                    slot.state = Some(state);
                    slot.measure = Some(measure);
                }
                None => {
                    slot.state = None;
                    slot.measure = None;
                }
            }
        }
        slot.kind = kind;
        if restyled {
            slot.style = self.styles.get(style);
        }
        if let Some(taffy) = taffy {
            slot.taffy = taffy;
        }
        slot.events = events;
        slot.hooks = hooks;
        slot.inherited = inherited.clone();
        slot.text = text;
        if relisted {
            let old = mem::replace(&mut slot.children, kids.to_vec());
            self.orphans.extend(old.into_iter().map(index_of));
            for child in 0..self.slots[index as usize].children.len() {
                let child = index_of(self.slots[index as usize].children[child]);
                self.slots[child as usize].parent = index;
            }
        }
        self.kids.truncate(start);
        if relaid {
            self.dirty(index);
        }
        NodeId::from(index as usize)
    }

    fn scope(&self) -> u64 {
        self.scopes.last().copied().unwrap_or_default()
    }

    fn dirty(&mut self, mut index: u32) {
        while index != NONE {
            let slot = &mut self.slots[index as usize];
            let parent = slot.parent;
            // WARNING: an empty cache means its parents were cleared too, unless it was just made.
            if matches!(slot.cache.clear(), ClearState::AlreadyEmpty)
                && parent != NONE
                && self.slots[parent as usize].cache.is_empty()
            {
                break;
            }
            index = parent;
        }
    }

    fn free_orphans(&mut self) {
        let pass = self.pass;
        let mut pending = mem::take(&mut self.orphans);
        while let Some(index) = pending.pop() {
            let slot = &mut self.slots[index as usize];
            if !slot.live || self.seen[index as usize] == pass {
                continue;
            }
            self.keys.remove(&slot.key);
            pending.extend(slot.children.drain(..).map(index_of));
            *slot = Slot {
                live: false,
                ..Slot::new((0, 0), self.initial.clone(), self.blank.clone())
            };
            self.free.push(index);
        }
        self.orphans = pending;
        if self.free.len() > 1024 && self.free.len() * 2 > self.slots.len() {
            self.compact();
        }
    }

    fn compact(&mut self) {
        let mut moved = vec![NONE; self.slots.len()];
        let mut slots = Vec::with_capacity(self.slots.len() - self.free.len());
        let mut seen = Vec::with_capacity(slots.capacity());
        for (index, slot) in mem::take(&mut self.slots).into_iter().enumerate() {
            if slot.live {
                moved[index] = slots.len() as u32;
                seen.push(self.seen[index]);
                slots.push(slot);
            }
        }
        let map = |node: &mut NodeId| *node = NodeId::from(moved[usize::from(*node)] as usize);
        for slot in &mut slots {
            slot.children.iter_mut().for_each(map);
            if slot.parent != NONE {
                slot.parent = moved[slot.parent as usize];
            }
        }
        for index in self.keys.values_mut() {
            *index = moved[*index as usize];
        }
        self.root = self.root.map(|root| moved[root as usize]);
        self.slots = slots;
        self.seen = seen;
        self.free.clear();
        self.hitboxes.clear();
        self.layers.clear();
    }

    pub(crate) fn layout(&mut self, root: NodeId, viewport: Size<Pixels>, window: &mut Window) {
        let scale = window.scale_factor();
        taffy::compute_root_layout(
            &mut LayoutTree {
                slots: &mut self.slots,
                window,
                scale,
            },
            root,
            taffy::Size {
                width: taffy::AvailableSpace::Definite(viewport.width.0 * scale),
                height: taffy::AvailableSpace::Definite(viewport.height.0 * scale),
            },
        );
    }

    fn bounds(&self, index: u32, origin: Point<f32>, window: &Window) -> [Bounds<Pixels>; 2] {
        let scale = window.scale_factor();
        let layout = &self.slots[index as usize].layout;
        let far = point(origin.x + layout.size.width, origin.y + layout.size.height);
        let snapped = Bounds::from_corners(origin.map(round_half_up), far.map(round_half_up));
        let offset = window.element_offset();
        let exact = Bounds::new(
            origin.map(|value| Pixels(value / scale)),
            size(
                Pixels(layout.size.width / scale),
                Pixels(layout.size.height / scale),
            ),
        );
        [(snapped / scale).map(Pixels) + offset, exact + offset]
    }

    pub(crate) fn prepaint(&mut self, root: NodeId, window: &mut Window) {
        self.stacks.clear();
        self.hitboxes.clear();
        self.prepaint_node(index_of(root), Point::default(), window);
    }

    fn prepaint_node(&mut self, index: u32, parent: Point<f32>, window: &mut Window) {
        let slot = &mut self.slots[index as usize];
        let location = slot.layout.location;
        let origin = point(parent.x + location.x, parent.y + location.y);
        slot.origin = origin;
        slot.listed = false;
        let [bounds, exact] = self.bounds(index, origin, window);
        let slot = &mut self.slots[index as usize];
        [slot.bounds[0], slot.bounds[2]] = [bounds, exact];
        match slot.hooks.clone() {
            Some(hooks) => {
                let (text, state) = (slot.text.clone(), slot.state.clone());
                let baselines = self.baselines(index, window.scale_factor());
                let style = self.slots[index as usize].style.clone();
                let mut cx = NodeCx {
                    window,
                    bounds,
                    exact,
                    baselines,
                    text: &text,
                    style: &style,
                    state: state.as_ref(),
                };
                let mut inner = |window: &mut Window| self.prepaint_box(index, &style, window);
                hooks.prepaint(&mut cx, &mut inner);
            }
            None => {
                let style = slot.style.clone();
                self.prepaint_box(index, &style, window);
            }
        }
        if let Some(order) = self.slots[index as usize].style.z_index
            && let Some(list) = self.stacks.last_mut()
        {
            list.push((order, index, window.content_mask()));
            self.slots[index as usize].listed = true;
        }
    }

    fn prepaint_box(&mut self, index: u32, style: &Style, window: &mut Window) {
        let origin = self.slots[index as usize].origin;
        let [bounds, exact] = self.bounds(index, origin, window);
        let scale = window.scale_factor();
        self.slots[index as usize].bounds[1] = bounds;
        let slot = &self.slots[index as usize];
        if let Some(state) = slot
            .state
            .as_ref()
            .and_then(|state| state.downcast_ref::<TextState>())
        {
            state.fit(px(slot.layout.size.width / scale), window);
            // WARNING: like Chromium, text starts at its exact place, and only each row's baseline snaps to a device pixel.
            state.layout.place(Bounds::new(exact.origin, bounds.size));
        }
        let scroll = slot
            .events
            .as_ref()
            .and_then(|events| events.scroll.clone());
        let needs_hitbox = Events::needs_hitbox(slot.events.as_deref(), style);
        let content = match &scroll {
            Some(_) if !slot.children.is_empty() => {
                // WARNING: children are in window coordinates, which are negative above the viewport.
                let mut min = point(f32::MAX, f32::MAX);
                let mut max = point(f32::MIN, f32::MIN);
                for &child in &slot.children {
                    let layout = &self.slots[index_of(child) as usize].layout;
                    let at = point(origin.x + layout.location.x, origin.y + layout.location.y);
                    let far = point(at.x + layout.size.width, at.y + layout.size.height);
                    min = min.min(at.map(round_half_up));
                    max = max.max(far.map(round_half_up));
                }
                Size::from(max - min).map(|value| Pixels(value / scale))
            }
            _ => bounds.size,
        };
        window.with_content_mask(style.overflow_mask(bounds), |window| {
            let hitbox = needs_hitbox.then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal));
            let offset = match &scroll {
                Some(handle) => {
                    handle.drive(window);
                    handle.clamp(style, bounds, content, scale)
                }
                None => Point::default(),
            };
            self.slots[index as usize].hitbox = match hitbox {
                Some(hitbox) => {
                    self.hitboxes.push(hitbox);
                    (self.hitboxes.len() - 1) as u32
                }
                None => NONE,
            };
            if style.display != Display::None {
                if style.stacking {
                    let mut list = self.layers.remove(&index).unwrap_or_default();
                    list.clear();
                    self.stacks.push(list);
                }
                window.with_element_offset(offset, |window| {
                    for child in 0..self.slots[index as usize].children.len() {
                        let child = index_of(self.slots[index as usize].children[child]);
                        self.prepaint_node(child, origin, window);
                    }
                });
                if style.stacking {
                    let mut list = self.stacks.pop().unwrap_or_default();
                    list.sort_by_key(|(order, ..)| *order);
                    self.layers.insert(index, list);
                }
            }
        });
    }

    fn baselines(&self, index: u32, scale: f32) -> [Option<Pixels>; 2] {
        let Baselines { first, last } = self.slots[index as usize].baselines;
        [first, last].map(|value| value.map(|value| Pixels(value / scale)))
    }

    pub(crate) fn paint(&mut self, root: NodeId, window: &mut Window) {
        self.painting.clear();
        self.paint_node(index_of(root), false, window);
    }

    fn paint_node(&mut self, index: u32, listed: bool, window: &mut Window) {
        let slot = &self.slots[index as usize];
        if slot.listed != listed {
            return;
        }
        let smoothing = mem::replace(&mut window.font_smoothing, slot.text.font_smoothing);
        match slot.hooks.clone() {
            Some(hooks) => {
                let (text, state) = (slot.text.clone(), slot.state.clone());
                let [bounds, _, exact] = slot.bounds;
                let baselines = self.baselines(index, window.scale_factor());
                let style = self.slots[index as usize].style.clone();
                let mut cx = NodeCx {
                    window,
                    bounds,
                    exact,
                    baselines,
                    text: &text,
                    style: &style,
                    state: state.as_ref(),
                };
                let mut inner = |window: &mut Window| self.paint_box(index, &style, window);
                hooks.paint(&mut cx, &mut inner);
            }
            None => {
                let style = self.slots[index as usize].style.clone();
                self.paint_box(index, &style, window);
            }
        }
        window.font_smoothing = smoothing;
    }

    fn paint_box(&mut self, index: u32, style: &Style, window: &mut Window) {
        if style.visibility == Visibility::Hidden {
            return;
        }
        let slot = &self.slots[index as usize];
        let bounds = slot.bounds[1];
        if let Kind::Layers(negative) = slot.kind {
            self.paint_layers(negative, window);
            return;
        }
        window.with_element_opacity(style.opacity, |window| {
            let radii = style.corner_radii.resolve(bounds.size);
            window.paint_drop_shadows(bounds, radii, &style.box_shadow);
            if let Some(background) = style.background.filter(|color| !color.is_transparent()) {
                window.paint_quad(quad(
                    bounds,
                    radii,
                    background,
                    px(0.),
                    background.alpha(0.),
                    style.border_style,
                ));
            }
            window.paint_inset_shadows(bounds, radii, &style.box_shadow);
            let mask = style.overflow_mask(bounds);
            window.with_content_mask(mask, |window| {
                let slot = &self.slots[index as usize];
                if let Some(hitbox) = self.hitboxes.get(slot.hitbox as usize) {
                    if let Some(cursor) = style.mouse_cursor {
                        window.set_cursor_style(cursor, hitbox);
                    }
                    if let Some(events) = &slot.events {
                        if events.drag {
                            window.insert_window_drag_hitbox(hitbox);
                        }
                        events.paint(style, slot.text.line_height_in_pixels(), hitbox, window);
                    }
                }
                match &slot.kind {
                    Kind::Image(image, fit) => {
                        let shown = fit.get_bounds(bounds, image.size());
                        window.paint_image(shown, bounds, radii, image.clone(), false);
                    }
                    Kind::Text(..) => {
                        if let Some(state) = slot
                            .state
                            .as_ref()
                            .and_then(|state| state.downcast_ref::<TextState>())
                        {
                            state.layout.paint(slot.text.text_align, window);
                        }
                    }
                    _ => {}
                }
                if style.display != Display::None {
                    if style.stacking {
                        self.painting.push(index);
                    }
                    for child in 0..self.slots[index as usize].children.len() {
                        let child = index_of(self.slots[index as usize].children[child]);
                        self.paint_node(child, false, window);
                    }
                    if style.stacking {
                        self.painting.pop();
                    }
                }
            });
            if let Some(color) = style.border_color.filter(|color| !color.is_transparent())
                && style.border_widths.any(|width| !width.is_zero())
            {
                window.paint_quad(quad(
                    bounds,
                    radii,
                    color.alpha(0.),
                    style.border_widths,
                    color,
                    style.border_style,
                ));
            }
        });
    }

    fn paint_layers(&mut self, negative: bool, window: &mut Window) {
        let Some(&context) = self.painting.last() else {
            return;
        };
        let list = self.layers.remove(&context).unwrap_or_default();
        for &(_, index, mask) in list.iter().filter(|(order, ..)| (*order < 0) == negative) {
            window.with_content_mask(Some(mask), |window| self.paint_node(index, true, window));
        }
        self.layers.insert(context, list);
    }
}

fn index_of(node: NodeId) -> u32 {
    usize::from(node) as u32
}

struct Interned<T> {
    values: FxHashMap<u64, Vec<Rc<T>>>,
    limit: usize,
}

impl<T> Default for Interned<T> {
    fn default() -> Self {
        Self {
            values: FxHashMap::default(),
            limit: 1024,
        }
    }
}

trait Fingerprint {
    fn fingerprint(&self, hasher: &mut FxHasher);
}

impl Fingerprint for Style {
    fn fingerprint(&self, hasher: &mut FxHasher) {
        let length = |hasher: &mut FxHasher, length: crate::Length| match length {
            crate::Length::Auto => 0u32.hash(hasher),
            crate::Length::Px(value) => value.0.to_bits().hash(hasher),
            crate::Length::Fraction(value) => (1, value.to_bits()).hash(hasher),
            crate::Length::Calc(value, offset) => {
                (value.to_bits(), offset.0.to_bits()).hash(hasher)
            }
        };
        (
            self.display as u8,
            self.position as u8,
            self.flex_direction as u8,
        )
            .hash(hasher);
        for value in [
            self.size.width,
            self.size.height,
            self.padding.top,
            self.padding.left,
        ]
        .into_iter()
        .chain([self.margin.top, self.margin.left, self.margin.bottom])
        {
            length(hasher, value);
        }
        for color in [self.background, self.border_color, self.text.color] {
            color
                .map(|color| [color.r, color.g, color.b, color.a].map(f32::to_bits))
                .hash(hasher);
        }
        self.border_widths.top.0.to_bits().hash(hasher);
        self.text
            .font_size
            .map(|size| size.0.to_bits())
            .hash(hasher);
    }
}

impl Fingerprint for TextStyle {
    fn fingerprint(&self, hasher: &mut FxHasher) {
        [
            self.font_size.0,
            self.line_height.0,
            self.color.r,
            self.color.g,
            self.color.b,
        ]
        .map(f32::to_bits)
        .hash(hasher);
        (
            self.font_family.len(),
            self.white_space as u8,
            self.text_align as u8,
        )
            .hash(hasher);
    }
}

impl<T: Fingerprint + PartialEq> Interned<T> {
    fn get(&mut self, value: T) -> Rc<T> {
        let mut hasher = FxHasher::default();
        value.fingerprint(&mut hasher);
        let list = self.values.entry(hasher.finish()).or_default();
        if let Some(kept) = list.iter().find(|kept| ***kept == value) {
            return kept.clone();
        }
        let value = Rc::new(value);
        list.push(value.clone());
        if self.values.len() > self.limit {
            self.values.retain(|_, list| {
                list.retain(|value| Rc::strong_count(value) > 1);
                !list.is_empty()
            });
            self.limit = (self.values.len() * 2).max(1024);
        }
        value
    }
}

struct TextMeasurement(TextState);

impl Measurement for TextMeasurement {
    fn same(&self, old: Option<&Rc<dyn Any>>) -> bool {
        old.and_then(|old| old.downcast_ref::<TextState>())
            .is_some_and(|old| *old == self.0)
    }

    fn build(self: Box<Self>) -> (Rc<dyn Any>, Box<MeasureFn>) {
        text_measurement(self.0)
    }
}

fn text_measurement(state: TextState) -> (Rc<dyn Any>, Box<MeasureFn>) {
    let state = Rc::new(state);
    let kept = state.clone();
    (
        state,
        Box::new(move |_, known, available, window| kept.measure(known, available, window)),
    )
}

struct LayoutTree<'a> {
    slots: &'a mut [Slot],
    window: &'a mut Window,
    scale: f32,
}

impl LayoutTree<'_> {
    fn compute(
        &mut self,
        node: NodeId,
        mut inputs: LayoutInput,
        block: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            return taffy::compute_hidden_layout(self, node);
        }
        let style = &self.slots[usize::from(node)].taffy;
        // WARNING: taffy calls the measured height of an absolute box definite. With `height: auto` and without
        // both insets it comes from the content, so percentage heights inside must act as `auto`.
        if style.position == taffy::Position::Absolute
            && inputs.run_mode == RunMode::PerformLayout
            && style.size.height.is_auto()
            && (style.inset.top.is_auto() || style.inset.bottom.is_auto())
        {
            inputs.known_dimensions_are_definite.height = false;
        }
        let output = taffy::compute_cached_layout(self, node, inputs, |tree, node, inputs| {
            let slot = &tree.slots[usize::from(node)];
            let leaf = slot.children.is_empty();
            let mut output = match slot.taffy.display {
                taffy::Display::None => taffy::compute_hidden_layout(tree, node),
                _ if leaf => tree.measure(usize::from(node), inputs),
                taffy::Display::Flex => tree.flexbox(node, inputs),
                taffy::Display::Grid => taffy::compute_grid_layout(tree, node, inputs),
                _ => taffy::compute_block_layout(tree, node, inputs, block),
            };
            if inputs.run_mode == RunMode::PerformLayout {
                taffy::compute_oof_layout(tree, node, &mut output);
                output.baselines = tree.baselines(usize::from(node), &output);
            }
            output
        });
        if inputs.run_mode == RunMode::PerformLayout {
            self.slots[usize::from(node)].baselines = output.baselines;
        }
        output
    }

    fn flexbox(&mut self, node: NodeId, inputs: LayoutInput) -> LayoutOutput {
        let output = taffy::compute_flexbox_layout(self, node, inputs);
        let style = &self.slots[usize::from(node)].taffy;
        if inputs.known_dimensions.width.is_none()
            && matches!(
                style.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            )
            && let taffy::AvailableSpace::Definite(available) = inputs.available_space.width
            && output.size.width > available
        {
            let min = taffy::compute_flexbox_layout(
                self,
                node,
                LayoutInput {
                    run_mode: RunMode::ComputeSize,
                    available_space: taffy::Size {
                        width: taffy::AvailableSpace::MinContent,
                        ..inputs.available_space
                    },
                    ..inputs
                },
            );
            let width = available.max(min.size.width);
            if width < output.size.width {
                return taffy::compute_flexbox_layout(
                    self,
                    node,
                    LayoutInput {
                        known_dimensions: taffy::Size {
                            width: Some(width),
                            ..inputs.known_dimensions
                        },
                        ..inputs
                    },
                );
            }
        }
        output
    }

    fn measure(&mut self, index: usize, inputs: LayoutInput) -> LayoutOutput {
        let scale = self.scale;
        let window = &mut *self.window;
        let Slot {
            taffy,
            measure,
            style,
            text,
            ..
        } = &mut self.slots[index];
        let mut baselines = None;
        let mut output = taffy::compute_leaf_layout(
            inputs,
            &*taffy,
            |_, _| 0.,
            |known, available| {
                let Some(measure) = measure else {
                    return taffy::Size::ZERO;
                };
                let known = size(
                    known.width.map(|value| Pixels(value / scale)),
                    known.height.map(|value| Pixels(value / scale)),
                );
                let available = size(
                    AvailableSpace::unscaled(available.width, scale),
                    AvailableSpace::unscaled(available.height, scale),
                );
                let (measured, lines) = measure(text, known, available, window);
                baselines = lines;
                taffy::Size {
                    width: (measured.width.0.max(0.) * scale * 64.).ceil() / 64.,
                    height: (measured.height.0.max(0.) * scale * 64.).ceil() / 64.,
                }
            },
        );
        if let Some([first, last]) = baselines.filter(|_| style.baseline != Baseline::None) {
            output.baselines = Baselines {
                first: Some(first.0 * scale),
                last: Some(last.0 * scale),
            };
        }
        output
    }

    fn baselines(&self, index: usize, output: &LayoutOutput) -> Baselines {
        let slot = &self.slots[index];
        let style = &slot.taffy;
        let baseline = slot.style.baseline;
        let first = output.baselines.first;
        if baseline == Baseline::Bottom {
            let bottom = Some(output.size.height);
            return Baselines {
                first: bottom,
                last: bottom,
            };
        }
        if slot.children.is_empty() {
            return output.baselines;
        }
        let children = slot
            .children
            .iter()
            .map(|&child| &self.slots[usize::from(child)])
            .filter(|child| {
                child.taffy.position != taffy::Position::Absolute
                    && child.taffy.display != taffy::Display::None
            });
        let at = |child: &Slot, value: f32| child.layout.location.y + value;
        let source = slot
            .children
            .iter()
            .map(|&child| &self.slots[usize::from(child)])
            .find(|child| child.style.baseline_source)
            .map(|child| {
                at(
                    child,
                    child.baselines.first.unwrap_or(child.layout.size.height),
                )
            });
        let row = style.display == taffy::Display::Flex
            && matches!(
                style.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            );
        let (first, last) = match baseline {
            _ if style.contain.suppresses_baseline() => (None, None),
            _ if source.is_some() => (source, source),
            Baseline::First => (first, first),
            Baseline::FirstOnly => (first, None),
            _ if row => {
                let last = children
                    .rev()
                    .find(|child| {
                        child.taffy.align_self.unwrap_or(style.align_items).keyword
                            == AlignItemsKeyword::Baseline
                    })
                    .map(|child| {
                        let baselines = child.baselines;
                        at(
                            child,
                            baselines
                                .last
                                .or(baselines.first)
                                .unwrap_or(child.layout.size.height),
                        )
                    })
                    .or(first);
                (first, last)
            }
            _ => (
                children
                    .clone()
                    .find_map(|child| Some(at(child, child.baselines.first?))),
                children
                    .rev()
                    .find_map(|child| Some(at(child, child.baselines.last?))),
            ),
        };
        Baselines {
            first: if baseline == Baseline::Last {
                last
            } else {
                first
            },
            last,
        }
    }
}

impl TraversePartialTree for LayoutTree<'_> {
    type ChildIter<'a>
        = std::iter::Copied<std::slice::Iter<'a, NodeId>>
    where
        Self: 'a;

    fn child_ids(&self, node: NodeId) -> Self::ChildIter<'_> {
        self.slots[usize::from(node)].children.iter().copied()
    }

    fn child_count(&self, node: NodeId) -> usize {
        self.slots[usize::from(node)].children.len()
    }

    fn get_child_id(&self, node: NodeId, child: usize) -> NodeId {
        self.slots[usize::from(node)].children[child]
    }
}

impl LayoutPartialTree for LayoutTree<'_> {
    type CoreContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type CustomIdent = String;

    fn get_core_container_style(&self, node: NodeId) -> &taffy::Style {
        &self.slots[usize::from(node)].taffy
    }

    fn resolve_calc_value(&self, handle: *const (), basis: f32) -> f32 {
        crate::style::resolve_calc(handle, basis)
    }

    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.slots[usize::from(node)].layout = *layout;
    }

    fn compute_child_layout(&mut self, node: NodeId, inputs: LayoutInput) -> LayoutOutput {
        self.compute(node, inputs, None)
    }
}

// WARNING: out-of-flow boxes are positioned by their parent, which is always their containing block, and
// layout is never rounded, so the hoisted lists are not kept.
impl LayoutContainingBlock for LayoutTree<'_> {
    type OofItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_oof_item_style(&self, node: NodeId) -> &taffy::Style {
        &self.slots[usize::from(node)].taffy
    }

    fn clear_hoisted_children(&mut self, _: NodeId) {}

    fn add_hoisted_children(&mut self, _: NodeId, _: &[NodeId]) {}
}

impl CacheTree for LayoutTree<'_> {
    fn cache_get(&mut self, node: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        self.slots[usize::from(node)].cache.get(input)
    }

    fn cache_store(&mut self, node: NodeId, input: &LayoutInput, output: LayoutOutput) {
        self.slots[usize::from(node)].cache.store(input, output);
    }

    fn cache_clear(&mut self, node: NodeId) {
        self.slots[usize::from(node)].cache.clear();
    }
}

impl LayoutBlockContainer for LayoutTree<'_> {
    type BlockContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type BlockItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_block_container_style(&self, node: NodeId) -> &taffy::Style {
        &self.slots[usize::from(node)].taffy
    }

    fn get_block_child_style(&self, node: NodeId) -> &taffy::Style {
        &self.slots[usize::from(node)].taffy
    }

    fn compute_block_child_layout(
        &mut self,
        node: NodeId,
        inputs: LayoutInput,
        block: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        self.compute(node, inputs, block)
    }
}

impl LayoutFlexboxContainer for LayoutTree<'_> {
    type FlexboxContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type FlexboxItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_flexbox_container_style(&self, node: NodeId) -> &taffy::Style {
        &self.slots[usize::from(node)].taffy
    }

    fn get_flexbox_child_style(&self, node: NodeId) -> &taffy::Style {
        &self.slots[usize::from(node)].taffy
    }
}

impl LayoutGridContainer for LayoutTree<'_> {
    type GridContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type GridItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_grid_container_style(&self, node: NodeId) -> &taffy::Style {
        &self.slots[usize::from(node)].taffy
    }

    fn get_grid_child_style(&self, node: NodeId) -> &taffy::Style {
        &self.slots[usize::from(node)].taffy
    }
}

#[cfg(test)]
mod tests {
    use super::{Tree, identity, index_of};
    use crate::{Child, ElementId, node, px, size};

    fn live(tree: &Tree) -> usize {
        tree.slots.iter().filter(|slot| slot.live).count()
    }

    fn slot(tree: &Tree, id: usize) -> &super::Slot {
        let index = tree.keys[&(identity(Some(ElementId::from(id)), 0), 0)];
        &tree.slots[index as usize]
    }

    #[test]
    fn kept_subtrees_link_and_dropped_boxes_free() {
        let mut tree = Tree::default();
        let viewport = size(px(100.), px(100.));
        let page = |rows: Vec<Child>| node().id(0usize).children(rows).into();
        let row = |id: usize| node().id(id).child(node().w(px(1.))).child("text").into();
        tree.begin(1., 0);
        tree.commit_root(page(vec![row(1), row(2), row(3)]), viewport, 1.);
        assert_eq!(live(&tree), 1 + 3 * 3);
        tree.begin(1., 0);
        let kept = slot(&tree, 2).children.clone();
        tree.commit_root(page(vec![row(1), Child::Kept(2usize.into())]), viewport, 1.);
        assert_eq!(live(&tree), 1 + 2 * 3);
        assert!(tree.contains(2usize) && !tree.contains(3usize));
        assert_eq!(slot(&tree, 2).children, kept);
        tree.begin(1., 0);
        let moved = node().id(4usize).child(Child::Kept(2usize.into())).into();
        tree.commit_root(page(vec![moved]), viewport, 1.);
        assert!(tree.contains(2usize) && !tree.contains(1usize));
        assert_eq!(live(&tree), 1 + 1 + 3);
        let parent = tree.keys[&(identity(Some(ElementId::from(4usize)), 0), 0)];
        assert_eq!(slot(&tree, 2).parent, parent);
        assert!(
            slot(&tree, 2)
                .children
                .iter()
                .all(|&child| { tree.slots[index_of(child) as usize].live })
        );
    }

    #[test]
    fn mostly_free_arenas_compact() {
        let mut tree = Tree::default();
        let viewport = size(px(100.), px(100.));
        let page = |rows: usize, kept: bool| {
            let rows = (1..=rows).map(|id| match kept {
                true if id <= 3 => Child::Kept(id.into()),
                _ => node().id(id).child(node()).into(),
            });
            node()
                .id(0usize)
                .children(rows.collect::<Vec<Child>>())
                .into()
        };
        tree.begin(1., 0);
        tree.commit_root(page(2000, false), viewport, 1.);
        let kept = slot(&tree, 3).children.clone();
        tree.begin(1., 0);
        let root = tree.commit_root(page(3, true), viewport, 1.).unwrap();
        assert_eq!(tree.slots.len(), 1 + 3 * 2);
        assert_eq!(tree.root, Some(index_of(root)));
        assert_eq!(slot(&tree, 3).children.len(), kept.len());
        let child = index_of(slot(&tree, 3).children[0]);
        assert_eq!(
            tree.slots[child as usize].parent,
            tree.keys[&(identity(Some(ElementId::from(3usize)), 0), 0)]
        );
        assert_eq!(slot(&tree, 3).parent, index_of(root));
    }

    #[test]
    fn kept_text_follows_the_inherited_style() {
        let mut tree = Tree::default();
        let viewport = size(px(100.), px(100.));
        let page = |font: f32, child: Child| {
            let mut page = node().id(0usize).child(child);
            page.style().text.font_size = Some(px(font));
            page.into()
        };
        tree.begin(1., 0);
        tree.commit_root(page(10., node().id(1usize).child("a").into()), viewport, 1.);
        tree.begin(1., 0);
        tree.commit_root(page(20., Child::Kept(1usize.into())), viewport, 1.);
        let text = index_of(slot(&tree, 1).children[0]);
        assert_eq!(tree.slots[text as usize].text.font_size, px(20.));
        assert!(tree.slots[text as usize].cache.is_empty());
    }
}
