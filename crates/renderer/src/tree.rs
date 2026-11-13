use crate::{
    AlignContent, AlignItems, AvailableSpace, Baseline, Clip, Display, Escape, Hit, Hooks, Inline,
    Overflow, Pixels, Point, Position, Rect, Region, Scope, Size, Style, TextStyle, Visibility,
    Window,
    geometry::round_half_up,
    node::{Child, Content, Events, MeasureFn, Measurement, Node, NodeCx, NodeKey, NodeStyle},
    point, px, size,
    text::{TextState, same_shaping},
    window::Placement,
};
use rustc_hash::{FxHashMap, FxHasher};
use std::{
    any::Any,
    hash::{Hash, Hasher},
    mem,
    rc::Rc,
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
    Text(bool),
    Measured,
    Layers(bool),
}

struct Slot {
    key: (u64, u32),
    live: bool,
    seen: u32,
    parent: u32,
    children: Vec<NodeId>,
    style: Rc<Styled>,
    stretched: Option<Box<taffy::Style>>,
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
    bounds: Rect<Pixels>,
    painted: Rect<Pixels>,
    exact: Rect<Pixels>,
    hitbox: u32,
    listed: bool,
    free: [bool; 2],
    placed: Option<(u32, Point<f32>, Placement)>,
    below: bool,
    volatile: bool,
    reach: Option<Rect<Pixels>>,
    escaped: Option<Clip<Pixels>>,
    // The size that the measure function gave in the last layout, which can overflow the box.
    measured: taffy::Size<f32>,
    // The far corners that `far` found, from the origin of the box. Cleared up the tree when a box changes.
    far: [Option<Point<f32>>; 2],
}

type Remains = (
    Option<Box<Events>>,
    Option<Rc<dyn Hooks>>,
    Kind,
    Option<Box<MeasureFn>>,
    Option<Rc<dyn Any>>,
);

impl Slot {
    fn new(key: (u64, u32), inherited: Rc<TextStyle>, style: Rc<Styled>) -> Self {
        Self {
            key,
            live: true,
            seen: 0,
            parent: NONE,
            children: Vec::new(),
            style,
            stretched: None,
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
            bounds: Rect::default(),
            painted: Rect::default(),
            exact: Rect::default(),
            hitbox: NONE,
            listed: false,
            free: [true; 2],
            placed: None,
            below: false,
            volatile: false,
            reach: None,
            escaped: None,
            measured: taffy::Size::ZERO,
            far: [None; 2],
        }
    }
}

impl Slot {
    // Keep freed slots' child capacity. Defer heavy drops until after drawing to avoid frees during the
    // frame.
    fn release(&mut self, blank: &Rc<Styled>, graveyard: &mut Vec<Remains>) {
        self.live = false;
        self.children.clear();
        self.style = blank.clone();
        self.stretched = None;
        graveyard.push((
            self.events.take(),
            self.hooks.take(),
            mem::replace(&mut self.kind, Kind::Box),
            self.measure.take(),
            self.state.take(),
        ));
        self.cache.clear();
    }

    fn reuse(&mut self, key: (u64, u32), inherited: &Rc<TextStyle>) {
        self.key = key;
        self.live = true;
        self.parent = NONE;
        self.inherited = inherited.clone();
        self.text = inherited.clone();
        self.layout = Layout::new();
        self.baselines = Baselines::NONE;
        self.origin = Point::default();
        (self.bounds, self.painted, self.exact) = Default::default();
        self.far = [None; 2];
        self.hitbox = NONE;
        self.listed = false;
        self.free = [true; 2];
        self.placed = None;
        self.below = false;
        self.volatile = false;
        self.escaped = None;
    }

    fn taffy(&self) -> &taffy::Style {
        self.stretched.as_deref().unwrap_or(&self.style.taffy)
    }
}

#[derive(Default)]
pub(crate) struct Styled {
    style: Style,
    taffy: taffy::Style,
    relative: bool,
}

impl std::ops::Deref for Styled {
    type Target = Style;

    fn deref(&self) -> &Style {
        &self.style
    }
}

type Refined = (Rc<TextStyle>, Vec<(crate::TextOverrides, Rc<TextStyle>)>);

#[derive(Default)]
pub struct Tree {
    slots: Vec<Slot>,
    free: Vec<u32>,
    keys: FxHashMap<(u64, u32), u32>,
    occurrences: FxHashMap<u64, u32>,
    scopes: Vec<u64>,
    pass: u32,
    orphans: Vec<u32>,
    root: Option<u32>,
    measured_with: (f32, usize),
    stretch: Option<Size<Pixels>>,
    hitboxes: Vec<Region>,
    layers: FxHashMap<u32, Vec<(i32, u32, Clip<Pixels>)>>,
    kids: Vec<NodeId>,
    initial: Rc<TextStyle>,
    blank: Rc<Styled>,
    styles: Interned<Styled>,
    // Retain styles used as pointer keys to prevent address reuse.
    shared: FxHashMap<*const Style, (Rc<Style>, Rc<Styled>)>,
    texts: Interned<TextStyle>,
    stacks: Vec<Vec<(i32, u32, Clip<Pixels>)>>,
    blocks: Vec<(u8, Clip<Pixels>)>,
    painting: Vec<u32>,
    unbounded: u32,
    previous: Vec<Region>,
    prepaints: u32,
    full: bool,
    graveyard: Vec<Remains>,
    refined: FxHashMap<usize, Refined>,
}

fn identity(id: Option<NodeKey>, scope: u64) -> u64 {
    let mut hasher = FxHasher::default();
    (id.map(|id| id.0), scope).hash(&mut hasher);
    hasher.finish()
}

impl Tree {
    pub(crate) fn begin(&mut self, scale: f32, fonts: usize) {
        if mem::replace(&mut self.measured_with, (scale, fonts)) != (scale, fonts) {
            self.slots.clear();
            self.free.clear();
            self.keys.clear();
            self.orphans.clear();
            self.root = None;
            self.styles = Interned::default();
            self.shared.clear();
        }
        self.pass = self.pass.wrapping_add(1);
        self.occurrences.clear();
        self.scopes.clear();
    }

    fn index(&self, id: NodeKey) -> Option<u32> {
        self.keys.get(&(identity(Some(id), 0), 0)).copied()
    }

    pub fn contains(&self, id: impl Into<NodeKey>) -> bool {
        self.index(id.into()).is_some()
    }

    pub fn style_of(&self, id: impl Into<NodeKey>) -> Option<&Style> {
        Some(&self.slots[self.index(id.into())? as usize].style)
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
        // Read the root after freeing dropped boxes: arena compaction can move it.
        self.free_orphans();
        root.and(self.root).map(|root| NodeId::from(root as usize))
    }

    pub(crate) fn patch(&mut self, node: Node, scale: f32) {
        let identity = identity(node.id, 0);
        if node.id.is_none() || self.occurrences.contains_key(&identity) {
            return;
        }
        let Some(&index) = self.keys.get(&(identity, 0)) else {
            return;
        };
        let inherited = self.slots[index as usize].inherited.clone();
        self.commit_node(node, &inherited, scale);
        changed(&mut self.slots, index as usize);
        let mut parent = self.slots[index as usize].parent;
        while parent != NONE && self.slots[index as usize].free != [true; 2] {
            let [wide, tall] = self.slots[index as usize].free;
            let free = &mut self.slots[parent as usize].free;
            *free = [free[0] && wide, free[1] && tall];
            parent = self.slots[parent as usize].parent;
        }
        self.free_orphans();
    }

    fn commit(&mut self, child: Child, inherited: &Rc<TextStyle>, scale: f32) -> Option<NodeId> {
        let (id, place) = match child {
            Child::Node(node) => return Some(self.commit_node(*node, inherited, scale)),
            Child::Kept(id) => (id, None),
            Child::Placed(id, place) => (id, Some(place)),
        };
        let index = self.index(id);
        debug_assert!(index.is_some(), "kept box {id:?} is not in the tree");
        let index = index?;
        self.slots[index as usize].seen = self.pass;
        self.inherit(index, inherited, scale);
        let style = &self.slots[index as usize].style.style;
        let length = crate::Length::Px;
        if let Some(placed) = place.map(|place| {
            (
                length(place.origin.x),
                length(place.origin.y),
                place.size.map(length),
            )
        }) && (style.inset.left, style.inset.top, style.size) != placed
        {
            let mut style = style.clone();
            (style.inset.left, style.inset.top, style.size) = placed;
            self.slots[index as usize].style = self.styles.styled(style, scale);
            changed(&mut self.slots, index as usize);
            self.dirty(index);
        }
        Some(NodeId::from(index as usize))
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
        let text = if slot.style.text.inherits() {
            inherited.clone()
        } else {
            let own = slot.style.clone();
            self.refine(inherited, &own.text)
        };
        let slot = &mut self.slots[index as usize];
        if *text != *slot.text {
            let mut kept = same_shaping(&text, &slot.text);
            if let Kind::Text(own) = slot.kind
                && let Some(state) = slot
                    .state
                    .as_ref()
                    .and_then(|state| state.downcast_ref::<TextState>())
            {
                let new = state.restyled(own, &text, scale);
                kept = state.keep(&new);
                if !kept {
                    let (state, measure) = text_measurement(new);
                    slot.state = Some(state);
                    slot.measure = Some(measure);
                }
            }
            if slot.measure.is_some() && !kept {
                self.dirty(index);
            }
            changed(&mut self.slots, index as usize);
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
            _ if style.text.inherits() => inherited.clone(),
            Some(slot)
                if Rc::ptr_eq(&slot.inherited, inherited) && slot.style.text == style.text =>
            {
                slot.text.clone()
            }
            _ => self.refine(inherited, &style.text),
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
        if style.display == Display::None && style.z_index.is_some() {
            style.make_mut().z_index = None;
        }
        let (styled, restyled) = match style {
            NodeStyle::Shared(style) => {
                let styled = match self.shared.get(&Rc::as_ptr(&style)) {
                    Some((_, styled)) => styled.clone(),
                    None => {
                        let styled = self.styles.styled((*style).clone(), scale);
                        if self.shared.len() >= 1024 && self.shared.len().is_power_of_two() {
                            self.shared
                                .retain(|_, (style, _)| Rc::strong_count(style) > 1);
                        }
                        self.shared
                            .insert(Rc::as_ptr(&style), (style, styled.clone()));
                        styled
                    }
                };
                let restyled = kept
                    .is_none_or(|index| !Rc::ptr_eq(&self.slots[index as usize].style, &styled));
                (styled, restyled)
            }
            NodeStyle::Own(style) => {
                let restyled =
                    kept.is_none_or(|index| self.slots[index as usize].style.style != *style);
                let styled = match kept {
                    Some(index) if !restyled => self.slots[index as usize].style.clone(),
                    _ => self.styles.styled(*style, scale),
                };
                (styled, restyled)
            }
        };
        let stretched = stretch.map(|viewport| {
            let mut taffy = styled.taffy.clone();
            let length = |value: Pixels| {
                taffy::Dimension::length(crate::geometry::to_layout_unit(value.0, scale))
            };
            if taffy.size.width.is_auto() {
                taffy.size.width = length(viewport.width);
            }
            if taffy.size.height.is_auto() {
                taffy.size.height = length(viewport.height);
            }
            Box::new(taffy)
        });
        let index = match kept {
            Some(index) => index,
            None => {
                let index = match self.free.pop() {
                    Some(index) => {
                        self.slots[index as usize].reuse(key, inherited);
                        index
                    }
                    None => {
                        self.slots
                            .push(Slot::new(key, inherited.clone(), self.blank.clone()));
                        (self.slots.len() - 1) as u32
                    }
                };
                self.keys.insert(key, index);
                index
            }
        };
        let slot = &mut self.slots[index as usize];
        slot.seen = self.pass;
        let same_measure = match &measurement {
            Some(measurement) => {
                measurement.same(slot.state.as_ref())
                    && same_shaping(&slot.text, &text)
                    && slot.measure.is_some()
            }
            None => slot.measure.is_none(),
        };
        let kids = &self.kids[start..];
        let relisted = slot.children != kids;
        let relaid = kept.is_none()
            || !same_measure
            || relisted
            || (restyled || stretched.is_some())
                && *slot.taffy() != *stretched.as_deref().unwrap_or(&styled.taffy)
            || restyled
                && (slot.style.baseline != styled.baseline
                    || slot.style.baseline_source != styled.baseline_source
                    || slot.style.inline != styled.inline);
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
        slot.style = styled;
        slot.stretched = stretched;
        slot.volatile = hooks.as_ref().is_some_and(|hooks| hooks.volatile())
            || slot.style.escape & !Escape::OUTLINE != 0
            || events
                .as_ref()
                .is_some_and(|events| events.scroll.is_some());
        slot.placed = None;
        unfar(&mut self.slots, index as usize);
        let slot = &mut self.slots[index as usize];
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
        let slot = &self.slots[index as usize];
        let row = matches!(
            slot.taffy().flex_direction,
            FlexDirection::Row | FlexDirection::RowReverse
        );
        let (wide, tall) = match slot.taffy().display {
            _ if slot.taffy().aspect_ratio.is_some() => (false, false),
            taffy::Display::Grid => (false, false),
            taffy::Display::Flex => (
                row || slot.taffy().flex_wrap == taffy::FlexWrap::NoWrap,
                row,
            ),
            _ => (true, true),
        };
        let free = slot
            .children
            .iter()
            .fold([wide, tall], |[wide, tall], &child| {
                let child = self.slots[index_of(child) as usize].free;
                [wide && child[0], tall && child[1]]
            });
        let below = self.slots[index as usize].children.iter().any(|&child| {
            let child = &self.slots[index_of(child) as usize];
            child.placed.is_none() || child.below || child.volatile
        });
        let slot = &mut self.slots[index as usize];
        (slot.free, slot.below) = (free, below);
        if relaid {
            self.dirty(index);
        }
        NodeId::from(index as usize)
    }

    fn refine(&mut self, inherited: &Rc<TextStyle>, own: &crate::TextOverrides) -> Rc<TextStyle> {
        let key = Rc::as_ptr(inherited) as usize;
        if let Some((_, list)) = self.refined.get(&key)
            && let Some((_, text)) = list.iter().find(|(kept, _)| kept == own)
        {
            return text.clone();
        }
        let mut text = (**inherited).clone();
        text.refine(own);
        let text = self.texts.get(text);
        if self.refined.len() > 1024 {
            self.refined.clear();
        }
        let (_, list) = self
            .refined
            .entry(key)
            .or_insert_with(|| (inherited.clone(), Vec::new()));
        if list.len() == 8 {
            list.remove(0);
        }
        list.push((own.clone(), text.clone()));
        text
    }

    fn scope(&self) -> u64 {
        self.scopes.last().copied().unwrap_or_default()
    }

    fn dirty(&mut self, mut index: u32) {
        while index != NONE {
            let slot = &mut self.slots[index as usize];
            let parent = slot.parent;
            // An empty cache means its parents were cleared too, unless it was just made.
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
            if !slot.live || slot.seen == pass {
                continue;
            }
            self.keys.remove(&slot.key);
            pending.extend(slot.children.iter().copied().map(index_of));
            slot.release(&self.blank, &mut self.graveyard);
            self.free.push(index);
        }
        self.orphans = pending;
        if self.graveyard.len() > 1 << 15 {
            self.bury();
        }
    }

    pub(crate) fn bury(&mut self) {
        self.graveyard.clear();
        if self.free.len() > 1024 && self.free.len() * 2 > self.slots.len() {
            self.compact();
        }
    }

    fn compact(&mut self) {
        let mut moved = vec![NONE; self.slots.len()];
        let mut slots = Vec::with_capacity((self.slots.len() - self.free.len()) * 3 / 2);
        for (index, slot) in mem::take(&mut self.slots).into_iter().enumerate() {
            if slot.live {
                moved[index] = slots.len() as u32;
                slots.push(slot);
            }
        }
        let map = |node: &mut NodeId| *node = NodeId::from(moved[usize::from(*node)] as usize);
        for slot in &mut slots {
            slot.placed = None;
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
        self.free.clear();
        self.hitboxes.clear();
        self.layers.clear();
    }

    pub(crate) fn layout(&mut self, root: NodeId, viewport: Size<Pixels>, window: &mut Window) {
        let scale = window.scale_factor();
        let space = taffy::Size {
            width: taffy::AvailableSpace::Definite(viewport.width.0 * scale),
            height: taffy::AvailableSpace::Definite(viewport.height.0 * scale),
        };
        let mut tree = LayoutTree {
            slots: &mut self.slots,
            window,
            scale,
            exact: false,
        };
        taffy::compute_root_layout(&mut tree, root, space);
        #[cfg(debug_assertions)]
        {
            let kept: Vec<_> = tree
                .slots
                .iter_mut()
                .map(|slot| (mem::take(&mut slot.cache), slot.layout, slot.baselines))
                .collect();
            tree.exact = true;
            taffy::compute_root_layout(&mut tree, root, space);
            let mut pending = vec![index_of(root)];
            while let Some(index) = pending.pop() {
                let slot = &tree.slots[index as usize];
                let (_, layout, baselines) = &kept[index as usize];
                let exact = (slot.layout.location, slot.layout.size, slot.baselines);
                assert!(
                    (layout.location, layout.size, *baselines) == exact,
                    "box {index} was laid out at {:?} {:?} {baselines:?}, taffy places it at {:?} {:?} {:?}",
                    layout.location,
                    layout.size,
                    slot.layout.location,
                    slot.layout.size,
                    slot.baselines,
                );
                pending.extend(slot.children.iter().map(|&child| index_of(child)));
            }
            for (slot, (cache, layout, baselines)) in tree.slots.iter_mut().zip(kept) {
                (slot.cache, slot.layout, slot.baselines) = (cache, layout, baselines);
            }
        }
    }

    fn bounds(&self, index: u32, origin: Point<f32>, window: &Window) -> [Rect<Pixels>; 2] {
        let scale = window.scale_factor();
        let layout = &self.slots[index as usize].layout;
        let far = point(origin.x + layout.size.width, origin.y + layout.size.height);
        let snapped = Rect::between(origin.map(round_half_up), far.map(round_half_up));
        let offset = window.offset();
        let exact = Rect::new(
            origin.map(|value| Pixels(value / scale)),
            size(
                Pixels(layout.size.width / scale),
                Pixels(layout.size.height / scale),
            ),
        );
        [(snapped / scale).map(Pixels) + offset, exact + offset]
    }

    pub(crate) fn prepaint(&mut self, root: NodeId, window: &mut Window) {
        self.prepaints = self.prepaints.wrapping_add(1);
        mem::swap(&mut self.previous, &mut self.hitboxes);
        self.hitboxes.clear();
        self.stacks.clear();
        self.blocks.clear();
        #[cfg(debug_assertions)]
        let start = window.region_places().len();
        self.prepaint_node(index_of(root), Point::default(), window);
        #[cfg(debug_assertions)]
        if !window.redraws() {
            let boxes = |tree: &Tree| {
                let (mut pending, mut boxes) = (vec![index_of(root)], Vec::new());
                while let Some(index) = pending.pop() {
                    let slot = &tree.slots[index as usize];
                    boxes.push((
                        index,
                        slot.origin,
                        [slot.bounds, slot.painted, slot.exact],
                        slot.listed,
                    ));
                    pending.extend(slot.children.iter().map(|&child| index_of(child)));
                }
                boxes
            };
            let places = |tree: &Tree| {
                let place = |hitbox: &Region| (hitbox.bounds, hitbox.hit);
                tree.hitboxes.iter().map(place).collect::<Vec<_>>()
            };
            let kept = (
                boxes(self),
                places(self),
                window.region_places()[start..].to_vec(),
                self.layers.clone(),
            );
            window.truncate_regions(start);
            self.hitboxes.clear();
            self.stacks.clear();
            self.blocks.clear();
            self.full = true;
            self.prepaint_node(index_of(root), Point::default(), window);
            self.full = false;
            let full = (
                boxes(self),
                places(self),
                window.region_places()[start..].to_vec(),
                self.layers.clone(),
            );
            let first = |kept: &[_], full: &[_]| {
                kept.iter().zip(full).position(|(kept, full)| kept != full)
            };
            let at = first(&kept.0, &full.0).map(|at| (kept.0[at], full.0[at]));
            assert!(kept.0 == full.0, "kept box differs: {at:?}");
            assert!(
                kept.1 == full.1,
                "kept hitboxes differ: {:?} {:?}",
                kept.1,
                full.1
            );
            assert!(
                kept.2 == full.2,
                "kept window hitboxes differ: {:?} {:?}",
                kept.2,
                full.2
            );
            assert!(
                kept.3 == full.3,
                "kept layers differ: {:?} {:?}",
                kept.3,
                full.3
            );
        }
    }

    fn prepaint_node(&mut self, index: u32, parent: Point<f32>, window: &mut Window) {
        let prepaints = self.prepaints;
        let slot = &mut self.slots[index as usize];
        let location = slot.layout.location;
        let origin = point(parent.x + location.x, parent.y + location.y);
        let placement = window.placement();
        if !self.full
            && !slot.below
            && !slot.volatile
            && slot.placed.is_some_and(|(pass, _, placed)| {
                pass == prepaints.wrapping_sub(1)
                    && (placed.mask, placed.transform) == (placement.mask, placement.transform)
            })
        {
            self.keep(index, parent, window);
            return;
        }
        // An out-of-flow box is volatile, so its ancestors are never kept and its containing block pushed its
        // clip to `blocks` in this pass.
        let (escape, parent) = (slot.style.escape, slot.parent);
        // An outline takes the clip of its box, not the overflow clip inside it.
        let outline = (escape == Escape::OUTLINE)
            .then(|| self.slots[parent as usize].placed)
            .flatten()
            .map(|(_, _, placed)| placed.mask);
        let slot = &mut self.slots[index as usize];
        slot.escaped = outline.or_else(|| {
            (escape != 0).then(|| {
                self.blocks
                    .iter()
                    .rev()
                    .find(|(contains, _)| contains & escape != 0)
                    .map_or_else(
                        || Clip::from(Rect::new(Point::default(), window.viewport_size())),
                        |(_, mask)| *mask,
                    )
            })
        });
        // The mask from the containing block's layer replaces intermediate clips.
        if let Some(mask) = slot.escaped {
            window.push_content_mask(mask);
        }
        let placement = window.placement();
        slot.placed = Some((prepaints, origin, placement));
        slot.origin = origin;
        // A listed box comes before the listed boxes inside it, which paint above it at the same z-index.
        slot.listed = slot.style.z_index.is_some() && !self.stacks.is_empty();
        if let Some(order) = slot.style.z_index
            && let Some(list) = self.stacks.last_mut()
        {
            list.push((order, index, window.content_mask()));
        }
        let [bounds, exact] = self.bounds(index, origin, window);
        let slot = &mut self.slots[index as usize];
        (slot.bounds, slot.exact) = (bounds, exact);
        self.visit(
            index,
            window,
            |hooks, cx, inner| hooks.prepaint(cx, inner),
            |tree, style, window| tree.prepaint_box(index, style, window),
        );
        self.reach(index);
        if self.slots[index as usize].escaped.is_some() {
            window.pop_content_mask();
        }
    }

    fn prepaint_box(&mut self, index: u32, style: &Style, window: &mut Window) {
        let origin = self.slots[index as usize].origin;
        let [bounds, _] = self.place(index, origin, window);
        let scale = window.scale_factor();
        let slot = &self.slots[index as usize];
        let scroll = slot
            .events
            .as_ref()
            .and_then(|events| events.scroll.clone());
        let needs_hitbox = Events::needs_region(slot.events.as_deref(), style);
        let content = match &scroll {
            Some(_) if !slot.children.is_empty() => self.content(index, origin, scale),
            _ => bounds.size,
        };
        window.scope(Scope::Clip(style.overflow_mask(bounds)), |window| {
            let hitbox = needs_hitbox.then(|| window.add_region(bounds, Hit::Pass));
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
                if style.contains != 0 {
                    self.blocks.push((style.contains, window.content_mask()));
                }
                window.scope(Scope::Offset(offset), |window| {
                    for child in 0..self.slots[index as usize].children.len() {
                        let child = index_of(self.slots[index as usize].children[child]);
                        if self.slots[child as usize].style.escape != Escape::OUTLINE {
                            self.prepaint_node(child, origin, window);
                        }
                    }
                });
                // Outlines don't scroll with the content.
                for child in 0..self.slots[index as usize].children.len() {
                    let child = index_of(self.slots[index as usize].children[child]);
                    if self.slots[child as usize].style.escape == Escape::OUTLINE {
                        self.prepaint_node(child, origin, window);
                    }
                }
                if style.contains != 0 {
                    self.blocks.pop();
                }
                if style.stacking {
                    let mut list = self.stacks.pop().unwrap_or_default();
                    list.sort_by_key(|(order, ..)| *order);
                    self.layers.insert(index, list);
                }
            }
        });
        let below = self.slots[index as usize].children.iter().any(|&child| {
            let child = &self.slots[index_of(child) as usize];
            child.below || child.volatile
        });
        self.slots[index as usize].below = below;
    }

    // The far corner of a box and of the overflow that it doesn't clip, in the layout units of its parent. Without
    // `nested`, only the content of its lines counts, not of boxes inside it.
    fn far(&mut self, index: u32, scale: f32, nested: bool) -> Point<f32> {
        let slot = &self.slots[index as usize];
        let Layout { location, size, .. } = slot.layout;
        if let Some(far) = slot.far[nested as usize].filter(|_| !self.full) {
            return point(location.x + far.x, location.y + far.y);
        }
        let visible = slot
            .style
            .overflow
            .map(|overflow| overflow == Overflow::Visible);
        let mut far = point(size.width, size.height);
        if !visible.x && !visible.y {
            self.slots[index as usize].far[nested as usize] = Some(far);
            return point(location.x + far.x, location.y + far.y);
        }
        // Without `nested`, only anonymous line boxes, which have no hooks, hold content of the scroll container.
        let descend = nested || slot.hooks.is_none() && slot.style.inline != Inline::None;
        if slot.measure.is_some() {
            let sides = slot.layout.padding + slot.layout.border;
            let text = slot
                .state
                .as_ref()
                .and_then(|state| state.downcast_ref::<TextState>())
                .and_then(|state| state.layout.overflow())
                .map_or(0., |width| width.0 * scale);
            far = far.max(point(
                sides.left + slot.measured.width.max(text),
                sides.top + slot.measured.height,
            ));
        }
        if descend {
            for child in 0..self.slots[index as usize].children.len() {
                let child = index_of(self.slots[index as usize].children[child]);
                if self.slots[child as usize].style.escape & (Escape::FIXED | Escape::OUTLINE) == 0
                {
                    far = far.max(self.far(child, scale, nested));
                }
            }
        }
        let far = point(
            if visible.x { far.x } else { size.width },
            if visible.y { far.y } else { size.height },
        );
        self.slots[index as usize].far[nested as usize] = Some(far);
        point(location.x + far.x, location.y + far.y)
    }

    // The scrollable content of a scroll container from the start of its content box. Like Chrome, its end
    // padding follows the margin boxes of its children and its lines, but not the overflow of boxes inside them.
    fn content(&mut self, index: u32, origin: Point<f32>, scale: f32) -> Size<Pixels> {
        let layout = self.slots[index as usize].layout;
        let mut min = point(
            origin.x + layout.border.left + layout.padding.left,
            origin.y + layout.border.top + layout.padding.top,
        )
        .map(round_half_up);
        let mut max = min;
        let border = point(
            layout.border.left + layout.border.right,
            layout.border.top + layout.border.bottom,
        );
        let end = point(layout.padding.right, layout.padding.bottom);
        for child in 0..self.slots[index as usize].children.len() {
            let id = index_of(self.slots[index as usize].children[child]);
            let child = &self.slots[id as usize];
            let (layout, absolute) = (child.layout, child.style.position == Position::Absolute);
            if child.style.escape & (Escape::FIXED | Escape::OUTLINE) != 0
                || layout.size.width == 0. && layout.size.height == 0. && child.children.is_empty()
            {
                continue;
            }
            let at = point(origin.x + layout.location.x, origin.y + layout.location.y);
            let mut far = self.far(id, scale, false);
            if !absolute {
                far.x += layout.margin.right.max(0.);
                far.y += layout.margin.bottom.max(0.);
            }
            let nested = self.far(id, scale, true);
            far = far.max(point(nested.x - end.x, nested.y - end.y));
            min = min.min(at.map(round_half_up));
            max = max.max(point(origin.x + far.x, origin.y + far.y).map(round_half_up));
        }
        // `clamp` takes the scroll range from the border box.
        Size::from(max - min + border).map(|value| Pixels(value / scale))
    }

    fn place(&mut self, index: u32, origin: Point<f32>, window: &mut Window) -> [Rect<Pixels>; 2] {
        let [bounds, exact] = self.bounds(index, origin, window);
        let scale = window.scale_factor();
        let slot = &mut self.slots[index as usize];
        slot.painted = bounds;
        if let Some(state) = slot
            .state
            .as_ref()
            .and_then(|state| state.downcast_ref::<TextState>())
        {
            let [top, right, bottom, left] = inset(&slot.layout, scale);
            let sides = slot.layout.padding + slot.layout.border;
            state.fit(
                px((slot.layout.size.width - sides.left - sides.right) / scale),
                window,
            );
            state.layout.place(Rect::new(
                exact.origin + point(left, top),
                size(
                    bounds.size.width - left - right,
                    bounds.size.height - top - bottom,
                ),
            ));
        }
        [bounds, exact]
    }

    fn keep(&mut self, index: u32, parent: Point<f32>, window: &mut Window) {
        let slot = &mut self.slots[index as usize];
        let location = slot.layout.location;
        let origin = point(parent.x + location.x, parent.y + location.y);
        let placement = window.placement();
        let moved = slot
            .placed
            .is_none_or(|(_, at, placed)| (at, placed.offset) != (origin, placement.offset));
        slot.placed = Some((self.prepaints, origin, placement));
        if moved {
            slot.origin = origin;
            let [bounds, exact] = self.bounds(index, origin, window);
            let slot = &mut self.slots[index as usize];
            (slot.bounds, slot.exact) = (bounds, exact);
        }
        if let Some(order) = self.slots[index as usize].style.z_index
            && let Some(list) = self.stacks.last_mut()
        {
            list.push((order, index, window.content_mask()));
        }
        self.visit(
            index,
            window,
            |hooks, cx, inner| hooks.kept(cx, inner),
            |tree, style, window| tree.keep_box(index, style, moved, window),
        );
        if moved {
            self.reach(index);
        }
    }

    fn keep_box(&mut self, index: u32, style: &Style, moved: bool, window: &mut Window) {
        let origin = self.slots[index as usize].origin;
        let [bounds, _] = match moved {
            true => self.place(index, origin, window),
            false => [self.slots[index as usize].painted; 2],
        };
        window.scope(Scope::Clip(style.overflow_mask(bounds)), |window| {
            let slot = &mut self.slots[index as usize];
            if let Some(hitbox) = self.previous.get(slot.hitbox as usize) {
                self.hitboxes.push(window.move_region(hitbox, bounds));
                slot.hitbox = (self.hitboxes.len() - 1) as u32;
            }
            if style.display != Display::None {
                if style.stacking {
                    let mut list = self.layers.remove(&index).unwrap_or_default();
                    list.clear();
                    self.stacks.push(list);
                }
                for child in 0..self.slots[index as usize].children.len() {
                    let child = index_of(self.slots[index as usize].children[child]);
                    self.keep(child, origin, window);
                }
                if style.stacking {
                    let mut list = self.stacks.pop().unwrap_or_default();
                    list.sort_by_key(|(order, ..)| *order);
                    self.layers.insert(index, list);
                }
            }
        });
    }

    // Text can overflow horizontally and one glyph line vertically. Extend its reach across the window.
    fn reach(&mut self, index: u32) {
        let slot = &self.slots[index as usize];
        let reach = slot
            .hooks
            .as_ref()
            .is_none_or(|hooks| hooks.bounded())
            .then(|| {
                let style = &slot.style;
                let grow = style.box_shadow.iter().fold(px(0.), |grow, shadow| {
                    grow.max(
                        shadow.offset.x.abs()
                            + shadow.offset.y.abs()
                            + shadow.blur_radius * 2.
                            + shadow.spread_radius,
                    )
                });
                let mut reach = slot.painted.outset(grow);
                if matches!(slot.kind, Kind::Text(..) | Kind::Measured) {
                    let line = slot.text.line_height.round() * 2.;
                    reach = Rect::new(
                        point(px(-1e9), reach.origin.y - line),
                        size(px(2e9), reach.size.height + line * 2.),
                    );
                }
                reach
            });
        let reach = reach.and_then(|reach| {
            slot.children.iter().try_fold(reach, |reach, &child| {
                Some(reach.union(self.slots[index_of(child) as usize].reach?))
            })
        });
        self.slots[index as usize].reach = reach;
    }

    fn visit(
        &mut self,
        index: u32,
        window: &mut Window,
        step: impl FnOnce(&dyn Hooks, &mut NodeCx<'_>, &mut dyn FnMut(&mut Window)),
        mut inner: impl FnMut(&mut Self, &Style, &mut Window),
    ) {
        let slot = &self.slots[index as usize];
        let style = slot.style.clone();
        let Some(hooks) = slot.hooks.clone() else {
            return inner(self, &style, window);
        };
        let (text, state) = (slot.text.clone(), slot.state.clone());
        let scale = window.scale_factor();
        let mut cx = NodeCx {
            window,
            bounds: slot.bounds,
            exact: slot.exact,
            inset: inset(&slot.layout, scale),
            baselines: self.baselines(index, scale),
            text: &text,
            style: &style,
            state: state.as_ref(),
        };
        step(&*hooks, &mut cx, &mut |window| inner(self, &style, window));
    }

    fn baselines(&self, index: u32, scale: f32) -> [Option<Pixels>; 2] {
        let Baselines { first, last } = self.slots[index as usize].baselines;
        [first, last].map(|value| value.map(|value| Pixels(value / scale)))
    }

    pub(crate) fn paint(&mut self, root: NodeId, window: &mut Window) {
        self.painting.clear();
        self.unbounded = 0;
        self.paint_node(index_of(root), false, window);
    }

    fn paint_node(&mut self, index: u32, listed: bool, window: &mut Window) {
        let slot = &self.slots[index as usize];
        let escaped = slot.escaped;
        // A layers node paints the z-indexed boxes of its stacking context, whose reach holds them.
        if slot.listed != listed
            || self.unbounded == 0
                && !matches!(slot.kind, Kind::Layers(_))
                && slot.reach.is_some_and(|reach| {
                    !reach.intersects(escaped.unwrap_or_else(|| window.content_mask()).bounds)
                })
        {
            return;
        }
        if let Some(mask) = escaped {
            window.push_content_mask(mask);
        }
        // Below a transform or an effect, boxes paint elsewhere than their bounds, so none is skipped.
        let unbounded = slot.hooks.as_ref().is_some_and(|hooks| !hooks.bounded());
        self.unbounded += u32::from(unbounded);
        let smoothing = mem::replace(&mut window.font_smoothing, slot.text.font_smoothing);
        self.visit(
            index,
            window,
            |hooks, cx, inner| hooks.paint(cx, inner),
            |tree, style, window| tree.paint_box(index, style, window),
        );
        self.unbounded -= u32::from(unbounded);
        window.font_smoothing = smoothing;
        if escaped.is_some() {
            window.pop_content_mask();
        }
    }

    fn paint_box(&mut self, index: u32, style: &Style, window: &mut Window) {
        if style.visibility == Visibility::Hidden {
            return;
        }
        let slot = &self.slots[index as usize];
        let bounds = slot.painted;
        if let Kind::Layers(negative) = slot.kind {
            self.paint_layers(negative, window);
            return;
        }
        window.scope(Scope::Opacity(style.opacity), |window| {
            let radii = style.corner_radii.resolve(bounds.size);
            window.draw_shadows(bounds, radii, &style.box_shadow, false);
            if let Some(background) = style.background.filter(|color| !color.is_transparent()) {
                window.draw_box(bounds, radii, background, Default::default());
            }
            window.draw_shadows(bounds, radii, &style.box_shadow, true);
            if let Some(color) = style.border_color.filter(|color| !color.is_transparent())
                && style.border_widths.any(|width| !width.is_zero())
            {
                window.draw_box(
                    bounds,
                    radii,
                    color.with_alpha(0.),
                    crate::Border {
                        widths: style.border_widths,
                        color,
                        style: style.border_style,
                        sides: 0,
                    },
                );
            }
            let mask = style.overflow_mask(bounds);
            window.scope(Scope::Clip(mask), |window| {
                let slot = &self.slots[index as usize];
                if let Some(hitbox) = self.hitboxes.get(slot.hitbox as usize) {
                    if let Some(cursor) = style.cursor {
                        window.set_cursor(hitbox, cursor);
                    }
                    if let Some(events) = &slot.events {
                        if events.drag {
                            window.add_drag_area(hitbox);
                        }
                        events.listen(style, slot.text.line_height.round(), hitbox, window);
                    }
                }
                let painted = matches!(slot.kind, Kind::Text(..) | Kind::Measured)
                    && slot.hooks.as_ref().is_some_and(|hooks| {
                        let scale = window.scale_factor();
                        let mut cx = NodeCx {
                            window,
                            bounds: slot.bounds,
                            exact: slot.exact,
                            inset: inset(&slot.layout, scale),
                            baselines: self.baselines(index, scale),
                            text: &slot.text,
                            style,
                            state: slot.state.as_ref(),
                        };
                        hooks.paint_content(&mut cx)
                    });
                let slot = &self.slots[index as usize];
                match &slot.kind {
                    _ if painted => {}
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
        });
    }

    fn paint_layers(&mut self, negative: bool, window: &mut Window) {
        let Some(&context) = self.painting.last() else {
            return;
        };
        let list = self.layers.remove(&context).unwrap_or_default();
        for &(_, index, mask) in list.iter().filter(|(order, ..)| (*order < 0) == negative) {
            window.scope(Scope::Clip(Some(mask)), |window| {
                self.paint_node(index, true, window)
            });
        }
        self.layers.insert(context, list);
    }
}

fn unfar(slots: &mut [Slot], mut index: usize) {
    while slots[index].far != [None; 2] {
        slots[index].far = [None; 2];
        match slots[index].parent {
            NONE => break,
            parent => index = parent as usize,
        }
    }
}

fn changed(slots: &mut [Slot], index: usize) {
    unfar(slots, index);
    slots[index].placed = None;
    let mut parent = slots[index].parent;
    while parent != NONE && !slots[parent as usize].below {
        slots[parent as usize].below = true;
        parent = slots[parent as usize].parent;
    }
}

fn inset(layout: &Layout, scale: f32) -> [Pixels; 4] {
    let sides = layout.padding + layout.border;
    [sides.top, sides.right, sides.bottom, sides.left].map(|side| Pixels(side / scale))
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
        .chain([self.inset.top, self.inset.left])
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
        self.find(
            hasher.finish(),
            value,
            |kept, value| kept == value,
            |value| value,
        )
    }
}

impl Interned<Styled> {
    fn styled(&mut self, style: Style, scale: f32) -> Rc<Styled> {
        let mut hasher = FxHasher::default();
        style.fingerprint(&mut hasher);
        self.find(
            hasher.finish(),
            style,
            |kept, style| kept.style == *style,
            |style| {
                let taffy = style.to_taffy(scale);
                let fixed = |raw: taffy::CompactLength| {
                    matches!(
                        raw.tag(),
                        taffy::CompactLength::LENGTH_TAG | taffy::CompactLength::AUTO_TAG
                    )
                };
                let sides = |rect: taffy::Rect<taffy::CompactLength>| {
                    [rect.top, rect.right, rect.bottom, rect.left]
                };
                let relative = ![taffy.size.width.into_raw(), taffy.size.height.into_raw()]
                    .into_iter()
                    .chain([
                        taffy.min_size.width.into_raw(),
                        taffy.min_size.height.into_raw(),
                    ])
                    .chain([
                        taffy.max_size.width.into_raw(),
                        taffy.max_size.height.into_raw(),
                    ])
                    .chain(sides(taffy.padding.map(|value| value.into_raw())))
                    .chain(sides(taffy.border.map(|value| value.into_raw())))
                    .chain(sides(taffy.margin.map(|value| value.into_raw())))
                    .all(fixed);
                Styled {
                    taffy,
                    style,
                    relative,
                }
            },
        )
    }
}

impl<T> Interned<T> {
    fn find<V>(
        &mut self,
        fingerprint: u64,
        value: V,
        same: impl Fn(&T, &V) -> bool,
        make: impl FnOnce(V) -> T,
    ) -> Rc<T> {
        let list = self.values.entry(fingerprint).or_default();
        if let Some(kept) = list.iter().find(|kept| same(kept, &value)) {
            return kept.clone();
        }
        let value = Rc::new(make(value));
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
            .is_some_and(|old| old.keep(&self.0))
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
    exact: bool,
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
        let slot = &self.slots[usize::from(node)];
        let style = slot.taffy();
        if !self.exact
            && inputs.run_mode == RunMode::ComputeSize
            && inputs.sizing_mode == taffy::SizingMode::ContentSize
            && slot.parent != NONE
            && style.position != taffy::Position::Absolute
        {
            let parent = self.slots[slot.parent as usize].taffy();
            let row = matches!(
                parent.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            );
            let (axis, main, cross, size, line) = match row {
                true => (
                    taffy::RequestedAxis::Vertical,
                    inputs.known_dimensions.width,
                    inputs.known_dimensions.height,
                    style.size.height,
                    inputs.parent_size.height,
                ),
                false => (
                    taffy::RequestedAxis::Horizontal,
                    inputs.known_dimensions.height,
                    inputs.known_dimensions.width,
                    style.size.width,
                    inputs.parent_size.width,
                ),
            };
            let margins = match row {
                true => [style.margin.top, style.margin.bottom],
                false => [style.margin.left, style.margin.right],
            };
            if parent.display == taffy::Display::Flex
                && parent.flex_wrap == taffy::FlexWrap::NoWrap
                && inputs.axis == axis
                && cross.is_none()
                && size.is_auto()
                && margins.iter().all(|margin| !margin.is_auto())
                && matches!(
                    style.align_self.unwrap_or(parent.align_items).keyword,
                    AlignItemsKeyword::Normal | AlignItemsKeyword::Stretch
                )
                && let (Some(main), Some(line)) = (main, line)
            {
                let size = match row {
                    true => taffy::Size {
                        width: main,
                        height: line,
                    },
                    false => taffy::Size {
                        width: line,
                        height: main,
                    },
                };
                return LayoutOutput::from_outer_size(size);
            }
        }
        // Taffy treats measured absolute heights as definite. With auto height and no paired insets, child
        // percentage heights must stay auto.
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
            let mut output = match slot.taffy().display {
                taffy::Display::None => taffy::compute_hidden_layout(tree, node),
                _ if leaf => tree.measure(usize::from(node), inputs),
                taffy::Display::Flex => tree
                    .stack(node, inputs)
                    .or_else(|| tree.row(node, inputs))
                    .unwrap_or_else(|| tree.flexbox(node, inputs)),
                taffy::Display::Grid => taffy::compute_grid_layout(tree, node, inputs),
                _ => taffy::compute_block_layout(tree, node, inputs, block),
            };
            if inputs.run_mode == RunMode::PerformLayout {
                taffy::compute_oof_layout(tree, node, &mut output);
                output.baselines = tree.baselines(usize::from(node), &output);
            }
            output
        });
        if inputs.run_mode == RunMode::PerformLayout
            && self.slots[usize::from(node)].baselines != output.baselines
        {
            self.slots[usize::from(node)].baselines = output.baselines;
            if !self.exact {
                changed(self.slots, usize::from(node));
            }
        }
        output
    }

    // Preserve Taffy's sum order for exact layout values. Fall back to Taffy for unsupported rows.
    fn row(&mut self, node: NodeId, inputs: LayoutInput) -> Option<LayoutOutput> {
        use taffy::{AlignContentKeyword, CompactLength, MaybeMath, MaybeResolve, ResolveOrZero};
        let LayoutInput {
            known_dimensions: known,
            parent_size,
            available_space,
            run_mode,
            sizing_mode,
            ..
        } = inputs;
        let width = known.width?;
        let index = usize::from(node);
        let style = self.slots[index].taffy();
        let calc = |value, basis| crate::style::resolve_calc(value, basis);
        let aligns = |align: taffy::AlignItems| {
            align.safety == taffy::AlignmentSafety::Default
                && matches!(
                    align.keyword,
                    AlignItemsKeyword::Normal
                        | AlignItemsKeyword::Stretch
                        | AlignItemsKeyword::Center
                        | AlignItemsKeyword::FlexStart
                        | AlignItemsKeyword::Start
                )
        };
        if self.exact
            || run_mode != RunMode::PerformLayout
            || !inputs.known_dimensions_are_definite.width
            || style.flex_direction != FlexDirection::Row
            || style.flex_wrap != taffy::FlexWrap::NoWrap
            || style.direction != taffy::Direction::Ltr
            || style.box_sizing != taffy::BoxSizing::BorderBox
            || style.aspect_ratio.is_some()
            || !aligns(style.align_items)
            || !matches!(
                style.justify_content.keyword(),
                AlignContentKeyword::Normal
                    | AlignContentKeyword::Start
                    | AlignContentKeyword::FlexStart
            )
            || !matches!(
                style.align_content.keyword,
                AlignContentKeyword::Normal
                    | AlignContentKeyword::Stretch
                    | AlignContentKeyword::Start
                    | AlignContentKeyword::FlexStart
            )
            || style.gap.width.into_raw().tag() != CompactLength::LENGTH_TAG
            || self.slots[index].style.inline == Inline::Lines
        {
            return None;
        }
        let simple = self.slots[index].children.iter().all(|&child| {
            let child = self.slots[usize::from(child)].taffy();
            child.display == taffy::Display::None
                || child.position != taffy::Position::Absolute
                    && aligns(child.align_self.unwrap_or(style.align_items))
                    && !child.flex_basis.is_sizing_keyword()
                    && !child.flex_basis.is_content()
                    && plain(child)
        });
        if !simple {
            return None;
        }
        let padding = style.padding.resolve_or_zero(parent_size.width, calc);
        let border = style.border.resolve_or_zero(parent_size.width, calc);
        let inset = padding + border;
        let min = style.min_size.maybe_resolve(parent_size, calc);
        let max = style.max_size.maybe_resolve(parent_size, calc);
        let definite = match (min.height, max.height) {
            (Some(min), Some(max)) if max <= min => Some(min),
            _ => None,
        };
        let styled = match sizing_mode {
            taffy::SizingMode::InherentSize => style
                .size
                .height
                .maybe_resolve(parent_size.height, calc)
                .maybe_clamp(min.height, max.height),
            taffy::SizingMode::ContentSize => None,
        };
        let pb = padding.sum_axes() + border.sum_axes();
        let fixed = known.height.or(definite.or(styled).maybe_max(pb.height));
        let (main, cross) = (inset.left + inset.right, inset.top + inset.bottom);
        let inner = (width - main, fixed.map(|height| height - cross));
        let gap = style.gap.width.into_raw().value();
        let (align_items, contain) = (style.align_items, style.contain);
        let space = match fixed {
            Some(height) => taffy::AvailableSpace::Definite((height - cross).max(0.)),
            None => available_space.height.maybe_sub(cross).maybe_max(0.),
        };
        struct Item {
            node: NodeId,
            order: u32,
            margin: taffy::Rect<f32>,
            padding: taffy::Rect<f32>,
            border: taffy::Rect<f32>,
            stretch: bool,
            center: bool,
            cross: Option<f32>,
            sides: f32,
            scrolls: bool,
            grow: f32,
            basis: f32,
            floor: f32,
            limit: Option<f32>,
            hypothetical: f32,
            target: f32,
            frozen: bool,
            violation: f32,
        }
        let mut items = Vec::with_capacity(self.slots[index].children.len());
        for order in 0..self.slots[index].children.len() {
            let child = self.slots[index].children[order];
            let style = self.slots[usize::from(child)].taffy();
            if style.display == taffy::Display::None {
                continue;
            }
            let margin = style.margin.resolve_or_zero(Some(inner.0), calc);
            let padding = style.padding.resolve_or_zero(Some(inner.0), calc);
            let border = style.border.resolve_or_zero(Some(inner.0), calc);
            let percents = taffy::Size {
                width: Some(inner.0),
                height: inner.1,
            };
            let size = style.size.maybe_resolve(percents, calc);
            let [min, max] =
                [style.min_size, style.max_size].map(|size| size.maybe_resolve(percents, calc));
            let align = style.align_self.unwrap_or(align_items).keyword;
            let stretch = matches!(
                align,
                AlignItemsKeyword::Normal | AlignItemsKeyword::Stretch
            ) && style.size.height.is_auto();
            if stretch && inner.1.is_none() {
                return None;
            }
            let automatic = min
                .width
                .or(style.overflow.x.is_scroll_container().then_some(0.));
            let basis = style.flex_basis.maybe_resolve(Some(inner.0), calc);
            let (grow, shrink, auto) = (
                style.flex_grow,
                style.flex_shrink,
                style.flex_basis.is_auto(),
            );
            let scrolls = style.overflow.y.is_scroll_container();
            let edges = padding + border;
            let lines = taffy::Size {
                width: (padding.left + padding.right) + (border.left + border.right),
                height: edges.top + edges.bottom,
            };
            let spaced = match space {
                taffy::AvailableSpace::Definite(value) => taffy::AvailableSpace::Definite(
                    (inner.1.unwrap_or(value) - (margin.top + margin.bottom)).max(0.),
                ),
                other => other,
            };
            let known = taffy::Size {
                width: None,
                height: size
                    .height
                    .or(stretch.then(|| spaced.into_option()).flatten()),
            };
            let measure = |tree: &mut Self, width: taffy::AvailableSpace| {
                tree.compute(
                    child,
                    LayoutInput {
                        run_mode: RunMode::ComputeSize,
                        sizing_mode: taffy::SizingMode::ContentSize,
                        axis: taffy::RequestedAxis::Horizontal,
                        known_dimensions: known,
                        known_dimensions_are_definite: taffy::Size {
                            width: true,
                            height: true,
                        },
                        parent_size: taffy::Size {
                            width: None,
                            height: inner.1,
                        },
                        available_space: taffy::Size {
                            width,
                            height: spaced,
                        },
                        vertical_margins_are_collapsible: taffy::Line::FALSE,
                    },
                    None,
                )
                .size
                .width
            };
            let (basis, set) = match (basis, size.width) {
                (Some(basis), _) => (basis, true),
                (None, Some(width)) if auto => (width, false),
                _ => (measure(self, taffy::AvailableSpace::MaxContent), false),
            };
            let basis = basis.max(lines.width);
            let floor = match automatic {
                Some(floor) => floor,
                None if set => measure(self, taffy::AvailableSpace::MinContent)
                    .maybe_min(size.width)
                    .maybe_min(max.width)
                    .maybe_max(Some(edges.left + edges.right)),
                None => 0.,
            };
            let lowest = floor
                .maybe_max(min.width)
                .maybe_max(Some(edges.left + edges.right));
            let hypothetical = basis.maybe_clamp(Some(lowest), max.width);
            items.push(Item {
                node: child,
                order: order as u32,
                margin,
                padding,
                border,
                stretch,
                center: align == AlignItemsKeyword::Center,
                cross: size.height.map(|height| height.max(lines.height)),
                sides: lines.height,
                scrolls,
                grow,
                basis,
                floor,
                limit: max.width,
                hypothetical,
                target: hypothetical,
                frozen: grow == 0. && shrink == 0.,
                violation: 0.,
            });
        }
        let gaps = if items.len() <= 1 {
            0.
        } else {
            gap * (items.len() - 1) as f32
        };
        let used = gaps
            + items
                .iter()
                .map(|item| item.hypothetical + (item.margin.left + item.margin.right))
                .sum::<f32>();
        if used > inner.0 {
            return None;
        }
        if used < inner.0 {
            for item in &mut items {
                item.frozen |= item.basis > item.hypothetical;
            }
            let taken = |items: &[Item]| {
                gaps + items
                    .iter()
                    .map(|item| match item.frozen {
                        true => item.target + (item.margin.left + item.margin.right),
                        false => item.basis + (item.margin.left + item.margin.right),
                    })
                    .sum::<f32>()
            };
            let initial = inner.0 - taken(&items);
            while !items.iter().all(|item| item.frozen) {
                let used = taken(&items);
                let grows = items
                    .iter()
                    .filter(|item| !item.frozen)
                    .fold(0., |sum, item| sum + item.grow);
                let free = match grows < 1. {
                    true => (initial * grows - gaps).min(inner.0 - used),
                    false => inner.0 - used,
                };
                if free.is_normal() && grows > 0. {
                    for item in items.iter_mut().filter(|item| !item.frozen) {
                        item.target = item.basis + free * (item.grow / grows);
                    }
                }
                let mut total = 0.;
                for item in items.iter_mut().filter(|item| !item.frozen) {
                    let clamped = item
                        .target
                        .maybe_clamp(Some(item.floor), item.limit)
                        .max(0.);
                    item.violation = clamped - item.target;
                    item.target = clamped;
                    total += item.violation;
                }
                for item in items.iter_mut().filter(|item| !item.frozen) {
                    item.frozen = match total {
                        total if total > 0. => item.violation > 0.,
                        total if total < 0. => item.violation < 0.,
                        _ => true,
                    };
                }
            }
        }
        let outer = inner.0 + main;
        let parent = taffy::Size {
            width: Some(inner.0),
            height: inner.1,
        };
        let stretched =
            fixed.map(|height| (height.maybe_clamp(min.height, max.height) - cross).max(0.));
        let mut outputs = Vec::with_capacity(items.len());
        for item in &items {
            let margins = item.margin.top + item.margin.bottom;
            let available = space
                .map_definite_value(|value| (value - margins).max(0.))
                .maybe_max(item.sides);
            let height = match (item.stretch, item.cross) {
                (true, _) => stretched.map(|line| (line - margins).max(0.)),
                (false, height) => height,
            };
            let output = self.compute(
                item.node,
                LayoutInput {
                    run_mode: RunMode::PerformLayout,
                    sizing_mode: taffy::SizingMode::ContentSize,
                    axis: taffy::RequestedAxis::Both,
                    known_dimensions: taffy::Size {
                        width: Some(item.target),
                        height,
                    },
                    known_dimensions_are_definite: taffy::Size {
                        width: true,
                        height: height.is_some(),
                    },
                    parent_size: parent,
                    available_space: taffy::Size {
                        width: taffy::AvailableSpace::Definite(
                            (outer - (item.margin.left + item.margin.right)).max(0.),
                        ),
                        height: available,
                    },
                    vertical_margins_are_collapsible: taffy::Line::FALSE,
                },
                None,
            );
            if output.size.height.max(item.sides) != output.size.height {
                return None;
            }
            outputs.push(output);
        }
        let line = match stretched {
            Some(line) => line,
            None => items
                .iter()
                .zip(&outputs)
                .map(|(item, output)| output.size.height + (item.margin.top + item.margin.bottom))
                .fold(0., f32::max)
                .maybe_clamp(min.height.maybe_sub(cross), max.height.maybe_sub(cross)),
        };
        let height = fixed
            .unwrap_or(line + 0. + cross)
            .maybe_clamp(min.height, max.height)
            .max(cross);
        let (mut total, mut first) = (inset.left, None);
        for (number, (item, output)) in items.iter().zip(&outputs).enumerate() {
            let offset = if number == 0 { 0. } else { gap };
            let free = line - (output.size.height + (item.margin.top + item.margin.bottom));
            let top = inset.top + if item.center { free / 2. } else { 0. } + item.margin.top;
            let mut layout = Layout::with_order(item.order);
            layout.size = output.size;
            layout.location = taffy::Point {
                x: total + offset + item.margin.left,
                y: top,
            };
            layout.padding = item.padding;
            layout.border = item.border;
            layout.margin = item.margin;
            self.set_unrounded_layout(item.node, &layout);
            if number == 0 {
                let baseline = output.baselines.first.unwrap_or(output.size.height);
                first = Some(
                    top + if item.scrolls {
                        baseline.min(output.size.height).max(0.)
                    } else {
                        baseline
                    },
                );
            }
            total += offset + (item.margin.left + item.margin.right) + output.size.width;
        }
        let size = taffy::Size {
            width: outer,
            height,
        };
        for order in 0..self.slots[index].children.len() {
            let child = self.slots[index].children[order];
            if self.slots[usize::from(child)].taffy().display == taffy::Display::None {
                self.hide(child, order);
            }
        }
        Some(finish(size, border, contain, first))
    }

    // Preserve Taffy's sum order. Columns that can flex, shrink or move items must use Taffy.
    fn stack(&mut self, node: NodeId, inputs: LayoutInput) -> Option<LayoutOutput> {
        use taffy::{AlignContentKeyword, CompactLength, MaybeMath, MaybeResolve, ResolveOrZero};
        let LayoutInput {
            known_dimensions: known,
            parent_size,
            available_space,
            run_mode,
            sizing_mode,
            axis,
            ..
        } = inputs;
        let width = known.width?;
        let index = usize::from(node);
        let style = self.slots[index].taffy();
        let calc = |value, basis| crate::style::resolve_calc(value, basis);
        let starts = |align: Option<taffy::AlignItems>| {
            align.is_none_or(|align| {
                matches!(
                    align.keyword,
                    AlignItemsKeyword::Normal | AlignItemsKeyword::Stretch
                )
            })
        };
        if self.exact
            || style.flex_direction != FlexDirection::Column
            || style.flex_wrap != taffy::FlexWrap::NoWrap
            || style.direction != taffy::Direction::Ltr
            || style.box_sizing != taffy::BoxSizing::BorderBox
            || style.aspect_ratio.is_some()
            || !starts(Some(style.align_items))
            || style.gap.height.into_raw().tag() != CompactLength::LENGTH_TAG
            || self.slots[index].style.inline == Inline::Lines
        {
            return None;
        }
        let flow = |child: &taffy::Style| {
            child.position != taffy::Position::Absolute && child.display != taffy::Display::None
        };
        let simple = self.slots[index].children.iter().all(|&child| {
            let child = self.slots[usize::from(child)].taffy();
            match child.position {
                _ if child.display == taffy::Display::None => true,
                taffy::Position::Absolute => {
                    let align = child.align_self.unwrap_or(style.align_items);
                    align.safety == taffy::AlignmentSafety::Default
                        && matches!(
                            align.keyword,
                            AlignItemsKeyword::Normal
                                | AlignItemsKeyword::Stretch
                                | AlignItemsKeyword::FlexStart
                                | AlignItemsKeyword::Start
                        )
                        && matches!(
                            style.justify_content.keyword(),
                            AlignContentKeyword::Normal
                                | AlignContentKeyword::Start
                                | AlignContentKeyword::FlexStart
                                | AlignContentKeyword::Stretch
                                | AlignContentKeyword::SpaceBetween
                        )
                }
                _ => starts(child.align_self) && child.flex_basis.is_auto() && plain(child),
            }
        });
        if !simple {
            return None;
        }
        let padding = style.padding.resolve_or_zero(parent_size.width, calc);
        let border = style.border.resolve_or_zero(parent_size.width, calc);
        let inset = padding + border;
        let min = style.min_size.maybe_resolve(parent_size, calc);
        let max = style.max_size.maybe_resolve(parent_size, calc);
        let pb = padding.top + padding.bottom + (border.top + border.bottom);
        let definite = match (min.height, max.height) {
            (Some(min), Some(max)) if max <= min => Some(min),
            _ => None,
        };
        let styled = match sizing_mode {
            taffy::SizingMode::InherentSize => style
                .size
                .height
                .maybe_resolve(parent_size.height, calc)
                .maybe_clamp(min.height, max.height),
            taffy::SizingMode::ContentSize => None,
        };
        let fixed = known.height.or(definite.or(styled).maybe_max(pb));
        let main = inset.top + inset.bottom;
        let cross = inset.left + inset.right;
        if run_mode == RunMode::ComputeSize
            && (fixed.is_some() || axis == taffy::RequestedAxis::Horizontal)
            || width.maybe_clamp(min.width, max.width) != width
        {
            return None;
        }
        let inner = width - cross;
        let line = (width - cross).max(0.);
        let gap = style.gap.height.into_raw().value();
        let (justify, contain) = (style.justify_content.keyword(), style.contain);
        let perform = run_mode == RunMode::PerformLayout;
        let mut total = inset.top;
        let (mut items, mut hypothetical, mut contents, mut grows, mut shrinks) =
            (0, 0., 0., false, false);
        let mut first = None;
        for order in 0..self.slots[index].children.len() {
            let child = self.slots[index].children[order];
            let style = self.slots[usize::from(child)].taffy();
            if !flow(style) {
                continue;
            }
            let margin = style.margin.resolve_or_zero(Some(inner), calc);
            let padding = style.padding.resolve_or_zero(Some(inner), calc);
            let border = style.border.resolve_or_zero(Some(inner), calc);
            let sides = (padding + border).sum_axes();
            let [min, max] = [style.min_size, style.max_size]
                .map(|size| size.width.maybe_resolve(Some(inner), calc));
            let width = match style.size.width.maybe_resolve(Some(inner), calc) {
                None => (line - (margin.left + margin.right))
                    .max(0.)
                    .maybe_clamp(min, max),
                Some(width) => width.maybe_clamp(min, max).max(sides.width),
            };
            let (grow, shrink, styled) = (
                style.flex_grow,
                style.flex_shrink,
                style.size.height.into_raw(),
            );
            let output = self.compute(
                child,
                LayoutInput {
                    run_mode: if perform {
                        RunMode::PerformLayout
                    } else {
                        RunMode::ComputeSize
                    },
                    sizing_mode: taffy::SizingMode::InherentSize,
                    axis: if perform {
                        taffy::RequestedAxis::Both
                    } else {
                        taffy::RequestedAxis::Vertical
                    },
                    known_dimensions: taffy::Size {
                        width: Some(width),
                        height: None,
                    },
                    known_dimensions_are_definite: taffy::Size {
                        width: true,
                        height: true,
                    },
                    parent_size: taffy::Size {
                        width: Some(inner),
                        height: None,
                    },
                    available_space: taffy::Size {
                        width: taffy::AvailableSpace::Definite(width),
                        height: taffy::AvailableSpace::MaxContent,
                    },
                    vertical_margins_are_collapsible: taffy::Line::FALSE,
                },
                None,
            );
            let height = output.size.height;
            // Taffy's content sizes take a fixed height even below the padding, which the box itself doesn't.
            if styled.tag() == CompactLength::LENGTH_TAG && styled.value() != height {
                return None;
            }
            let margins = margin.top + margin.bottom;
            let offset = if items == 0 { 0. } else { gap };
            let top = total + offset + margin.top;
            if perform {
                let mut layout = Layout::with_order(order as u32);
                layout.size = taffy::Size { width, height };
                layout.location = taffy::Point {
                    x: inset.left + margin.left,
                    y: top,
                };
                layout.padding = padding;
                layout.border = border;
                layout.margin = margin;
                self.set_unrounded_layout(child, &layout);
                if items == 0 {
                    first = Some(top + output.baselines.first.unwrap_or(height));
                }
            }
            total += offset + margins + height;
            hypothetical += height + margins;
            contents += match available_space.height {
                taffy::AvailableSpace::Definite(_) => (height + margins).max(sides.height),
                _ => {
                    let difference = (height + margins) - height;
                    let basis =
                        height - (padding.top + padding.bottom) - (border.top + border.bottom);
                    let fraction = if difference > 0. {
                        difference / grow.max(1.)
                    } else if difference < 0. {
                        difference / (shrink.max(1.) * basis)
                    } else {
                        0.
                    };
                    height
                        + if fraction > 0. {
                            grow.max(1.) * fraction
                        } else if fraction < 0. && shrink.max(1.) * basis != 0. {
                            shrink.max(1.) * basis * fraction
                        } else {
                            0.
                        }
                }
            };
            grows |= grow > 0.;
            shrinks |= shrink > 0.;
            items += 1;
        }
        let gaps = if items <= 1 {
            0.
        } else {
            gap * (items - 1) as f32
        };
        let (outer, inner_main) = match fixed {
            Some(height) => {
                let inner = height - main;
                (inner + main, inner)
            }
            None => {
                let content = match available_space.height {
                    taffy::AvailableSpace::Definite(_) => contents + gaps + main,
                    _ => 0f32.max(contents + gaps) + main,
                };
                let outer = content.maybe_clamp(min.height, max.height).max(main);
                (outer, (outer - main).max(0.))
            }
        };
        let free = inner_main - (gaps + hypothetical);
        let packed = matches!(
            justify,
            AlignContentKeyword::Normal
                | AlignContentKeyword::Start
                | AlignContentKeyword::FlexStart
                | AlignContentKeyword::Stretch
        );
        if free != 0. && (!packed || free > 0. && grows || free < 0. && shrinks) {
            return None;
        }
        if !perform {
            let width = if axis == taffy::RequestedAxis::Vertical {
                0.
            } else {
                width.max(cross)
            };
            return Some(LayoutOutput::from_outer_size(taffy::Size {
                width,
                height: outer,
            }));
        }
        let size = taffy::Size {
            width: width.max(cross),
            height: outer,
        };
        let mut candidates = taffy::OofCandidates::new();
        for order in 0..self.slots[index].children.len() {
            let child = self.slots[index].children[order];
            let style = self.slots[usize::from(child)].taffy();
            if style.display == taffy::Display::None {
                self.hide(child, order);
            } else if style.position == taffy::Position::Absolute {
                let area = |start, end| taffy::AxisStaticPosition {
                    area: taffy::Line { start, end },
                    align: taffy::AxisStaticAlign::from_keyword(taffy::AxisStaticEdge::Start),
                };
                candidates.push(taffy::OofCandidate {
                    node: child,
                    order: order as u32,
                    position: style.position,
                    static_position: taffy::Point {
                        x: area(inset.left, size.width - inset.right),
                        y: area(inset.top, size.height - inset.bottom),
                    },
                });
            }
        }
        let mut output = finish(size, border, contain, first);
        output.oof_candidates = candidates;
        Some(output)
    }

    fn hide(&mut self, child: NodeId, order: usize) {
        self.set_unrounded_layout(child, &Layout::with_order(order as u32));
        self.compute(
            child,
            LayoutInput {
                run_mode: RunMode::PerformLayout,
                sizing_mode: taffy::SizingMode::ContentSize,
                axis: taffy::RequestedAxis::Both,
                known_dimensions: taffy::Size::NONE,
                known_dimensions_are_definite: taffy::Size {
                    width: true,
                    height: true,
                },
                parent_size: taffy::Size::NONE,
                available_space: taffy::Size {
                    width: taffy::AvailableSpace::MaxContent,
                    height: taffy::AvailableSpace::MaxContent,
                },
                vertical_margins_are_collapsible: taffy::Line::FALSE,
            },
            None,
        );
    }

    fn flexbox(&mut self, node: NodeId, inputs: LayoutInput) -> LayoutOutput {
        let mut output = taffy::compute_flexbox_layout(self, node, inputs);
        let index = usize::from(node);
        let style = self.slots[index].taffy();
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
                output = taffy::compute_flexbox_layout(
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
        if inputs.run_mode != RunMode::PerformLayout
            || self.slots[index].style.inline != Inline::Lines
        {
            return output;
        }
        let style = self.slots[index].style.clone();
        let lift = match style.justify_content {
            Some(AlignContent::Center) => 0.5,
            Some(AlignContent::End | AlignContent::FlexEnd) => 1.,
            _ => 0.,
        };
        let flows = |slot: &Slot| {
            slot.taffy().position != taffy::Position::Absolute
                && slot.taffy().display != taffy::Display::None
        };
        let align = |slot: &Slot| slot.style.align_self.or(style.align_items);
        let children = mem::take(&mut self.slots[index].children);
        let (mut first, mut right, mut top) = (0, f32::NEG_INFINITY, true);
        for at in 0..=children.len() {
            if let Some(&child) = children.get(at) {
                let slot = &self.slots[usize::from(child)];
                if !flows(slot) {
                    continue;
                }
                let layout = &slot.layout;
                let starts = layout.location.x - layout.margin.left < right - 0.5;
                right = layout.location.x + layout.size.width + layout.margin.right;
                if !starts {
                    continue;
                }
            }
            let line = &children[first..at];
            first = at;
            let (mut above, mut below) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
            for &child in line {
                let slot = &self.slots[usize::from(child)];
                if flows(slot) && align(slot) == Some(AlignItems::Baseline) {
                    let layout = &slot.layout;
                    let base = slot.baselines.first.unwrap_or(layout.size.height);
                    above = above.max(base + layout.margin.top);
                    below = below.max(layout.size.height + layout.margin.bottom - base);
                }
            }
            let mut tallest = (0., false);
            for &child in line {
                let slot = &self.slots[usize::from(child)];
                let layout = &slot.layout;
                let height = layout.size.height + layout.margin.top + layout.margin.bottom;
                let edge = align(slot)
                    .filter(|align| matches!(align, AlignItems::FlexStart | AlignItems::FlexEnd));
                if flows(slot) && edge.is_some() && height > tallest.0 {
                    tallest = (height, edge == Some(AlignItems::FlexEnd));
                }
            }
            let drop = if tallest.1 && above.is_finite() {
                (tallest.0 - above - below).max(0.)
            } else {
                0.
            };
            let last = |children: &[NodeId]| {
                children
                    .iter()
                    .rev()
                    .map(|&child| &self.slots[usize::from(child)])
                    .find(|slot| flows(slot))
            };
            let mut end = last(line);
            while let Some(slot) = end.filter(|slot| {
                slot.style.inline == Inline::Lines
                    && slot.taffy().flex_wrap == taffy::FlexWrap::NoWrap
            }) {
                end = last(&slot.children);
            }
            let hang = end
                .filter(|slot| lift > 0. && slot.style.inline == Inline::Hangs)
                .and_then(|slot| slot.children.last())
                .map_or(0., |&space| {
                    let layout = &self.slots[usize::from(space)].layout;
                    (layout.size.width + layout.margin.left + layout.margin.right) * lift
                });
            if drop > 0. || hang > 0. {
                for &child in line {
                    let slot = &mut self.slots[usize::from(child)];
                    if flows(slot) {
                        slot.layout.location.x += hang;
                        if align(slot) == Some(AlignItems::Baseline) {
                            slot.layout.location.y += drop;
                        }
                    }
                }
                if top {
                    output.baselines.first = output.baselines.first.map(|base| base + drop);
                }
            }
            top = false;
        }
        self.slots[index].children = children;
        output
    }

    fn measure(&mut self, index: usize, inputs: LayoutInput) -> LayoutOutput {
        let scale = self.scale;
        let window = &mut *self.window;
        let Slot {
            stretched,
            measure,
            style,
            text,
            measured: kept,
            ..
        } = &mut self.slots[index];
        let taffy = stretched.as_deref().unwrap_or(&style.taffy);
        use taffy::ResolveOrZero;
        let calc = |value, basis| crate::style::resolve_calc(value, basis);
        let width = inputs.parent_size.width;
        let padding = taffy.padding.resolve_or_zero(width, calc);
        let border = taffy.border.resolve_or_zero(width, calc);
        // Taffy passes the known border box size, but measure functions take the content box, like `available`.
        let inset = (padding + border).sum_axes();
        let mut baselines = None;
        let mut content = None;
        let mut output = taffy::compute_leaf_layout(
            inputs,
            taffy,
            |_, _| 0.,
            |known, available| {
                let Some(measure) = measure else {
                    return taffy::Size::ZERO;
                };
                let known = size(
                    known
                        .width
                        .map(|value| Pixels((value - inset.width).max(0.) / scale)),
                    known
                        .height
                        .map(|value| Pixels((value - inset.height).max(0.) / scale)),
                );
                let available = size(
                    AvailableSpace::unscaled(available.width, scale),
                    AvailableSpace::unscaled(available.height, scale),
                );
                let (measured, lines) = measure(text, known, available, window);
                baselines = lines;
                let size = taffy::Size {
                    width: (measured.width.0.max(0.) * scale * 64.).ceil() / 64.,
                    height: (measured.height.0.max(0.) * scale * 64.).ceil() / 64.,
                };
                content = Some(size);
                size
            },
        );
        let remeasured = inputs.run_mode == RunMode::PerformLayout
            && content.is_some_and(|content| mem::replace(kept, content) != content);
        if let Some([first, last]) = baselines.filter(|_| style.baseline != Baseline::None) {
            let top = padding.top + border.top;
            output.baselines = Baselines {
                first: Some(first.0 * scale + top),
                last: Some(last.0 * scale + top),
            };
        }
        if remeasured {
            unfar(self.slots, index);
        }
        output
    }

    fn baselines(&self, index: usize, output: &LayoutOutput) -> Baselines {
        let slot = &self.slots[index];
        let style = slot.taffy();
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
            let Baselines { first, last } = output.baselines;
            return Baselines {
                first: if baseline == Baseline::Last {
                    last
                } else {
                    first
                },
                last,
            };
        }
        let children = slot
            .children
            .iter()
            .map(|&child| &self.slots[usize::from(child)])
            .filter(|child| {
                child.taffy().position != taffy::Position::Absolute
                    && child.taffy().display != taffy::Display::None
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
                        child
                            .taffy()
                            .align_self
                            .unwrap_or(style.align_items)
                            .keyword
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
        self.slots[usize::from(node)].taffy()
    }

    fn resolve_calc_value(&self, handle: *const (), basis: f32) -> f32 {
        crate::style::resolve_calc(handle, basis)
    }

    // A box that only moved is not marked changed, since its kept prepaint follows it.
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        let index = usize::from(node);
        let old = mem::replace(&mut self.slots[index].layout, *layout);
        if !self.exact
            && (Layout {
                location: old.location,
                ..*layout
            }) != old
        {
            changed(self.slots, index);
        }
    }

    fn compute_child_layout(&mut self, node: NodeId, inputs: LayoutInput) -> LayoutOutput {
        self.compute(node, inputs, None)
    }
}

// Hoisted lists are unnecessary: parents position out-of-flow boxes, and layout stays unrounded.
impl LayoutContainingBlock for LayoutTree<'_> {
    type OofItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_oof_item_style(&self, node: NodeId) -> &taffy::Style {
        self.slots[usize::from(node)].taffy()
    }

    fn clear_hoisted_children(&mut self, _: NodeId) {}

    fn add_hoisted_children(&mut self, _: NodeId, _: &[NodeId]) {}
}

fn plain(child: &taffy::Style) -> bool {
    use taffy::CompactLength;
    child.box_sizing == taffy::BoxSizing::BorderBox
        && child.aspect_ratio.is_none()
        && !child.size.width.is_sizing_keyword()
        && matches!(
            child.size.height.into_raw().tag(),
            CompactLength::AUTO_TAG | CompactLength::LENGTH_TAG
        )
        && child.min_size.height.is_auto()
        && child.max_size.height.is_auto()
        && [
            child.margin.top,
            child.margin.right,
            child.margin.bottom,
            child.margin.left,
        ]
        .iter()
        .all(|margin| !margin.is_auto())
        && [
            child.inset.top,
            child.inset.right,
            child.inset.bottom,
            child.inset.left,
        ]
        .iter()
        .all(|inset| inset.is_auto())
}

fn finish(
    size: taffy::Size<f32>,
    border: taffy::Rect<f32>,
    contain: taffy::style::Contain,
    first: Option<f32>,
) -> LayoutOutput {
    let mut output = LayoutOutput::from_sizes_and_baselines(
        size,
        taffy::Rect::ZERO,
        if contain.suppresses_baseline() {
            Baselines::NONE
        } else {
            Baselines::from_first(first)
        },
    );
    output.oof_positioning_area = Some(taffy::OofPositioningArea {
        size: size - border.sum_axes(),
        offset: taffy::Point {
            x: border.left,
            y: border.top,
        },
    });
    output
}

fn cache_input(free: [bool; 2], relative: bool, input: &LayoutInput) -> LayoutInput {
    let mut input = *input;
    if !relative {
        input.parent_size = taffy::Size::NONE;
    }
    if input.run_mode == RunMode::ComputeSize {
        match input.axis {
            taffy::RequestedAxis::Horizontal
                if free[0] && input.known_dimensions.width.is_none() =>
            {
                input.known_dimensions.height = None;
                input.available_space.height = taffy::AvailableSpace::MaxContent;
            }
            taffy::RequestedAxis::Vertical
                if free[1] && input.known_dimensions.height.is_none() =>
            {
                input.available_space.height = taffy::AvailableSpace::MaxContent;
            }
            _ => {}
        }
    }
    input
}

impl CacheTree for LayoutTree<'_> {
    fn cache_get(&mut self, node: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        let slot = &mut self.slots[usize::from(node)];
        let relative = self.exact || slot.style.relative;
        slot.cache.get(&cache_input(slot.free, relative, input))
    }

    fn cache_store(&mut self, node: NodeId, input: &LayoutInput, output: LayoutOutput) {
        let slot = &mut self.slots[usize::from(node)];
        let relative = self.exact || slot.style.relative;
        slot.cache
            .store(&cache_input(slot.free, relative, input), output);
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
        self.slots[usize::from(node)].taffy()
    }

    fn get_block_child_style(&self, node: NodeId) -> &taffy::Style {
        self.slots[usize::from(node)].taffy()
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
        self.slots[usize::from(node)].taffy()
    }

    fn get_flexbox_child_style(&self, node: NodeId) -> &taffy::Style {
        self.slots[usize::from(node)].taffy()
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
        self.slots[usize::from(node)].taffy()
    }

    fn get_grid_child_style(&self, node: NodeId) -> &taffy::Style {
        self.slots[usize::from(node)].taffy()
    }
}

#[cfg(test)]
mod tests {
    use super::{Tree, index_of};
    use crate::{Child, node, px, size};

    fn live(tree: &Tree) -> usize {
        tree.slots.iter().filter(|slot| slot.live).count()
    }

    fn slot(tree: &Tree, id: usize) -> &super::Slot {
        &tree.slots[tree.index(id.into()).unwrap() as usize]
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
        let parent = tree.index(4usize.into()).unwrap();
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
        tree.commit_root(page(3, true), viewport, 1.).unwrap();
        tree.bury();
        let root = tree.root.unwrap();
        assert_eq!(tree.slots.len(), 1 + 3 * 2);
        assert_eq!(slot(&tree, 3).children.len(), kept.len());
        let child = index_of(slot(&tree, 3).children[0]);
        assert_eq!(
            tree.slots[child as usize].parent,
            tree.index(3usize.into()).unwrap()
        );
        assert_eq!(slot(&tree, 3).parent, root);
    }

    #[test]
    fn kept_text_follows_the_inherited_style() {
        let mut tree = Tree::default();
        let viewport = size(px(100.), px(100.));
        let page = |font: f32, child: Child| {
            let mut page = node().id(0usize).child(child);
            page.style().text.set_font_size(px(font));
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

    #[test]
    fn paint_only_text_changes_keep_the_measured_state() {
        let mut tree = Tree::default();
        let viewport = size(px(100.), px(100.));
        let page = |color: u32, child: Child| {
            let mut page = node().id(0usize).child(child);
            page.style().text.set_color(crate::Color::hex(color));
            page.into()
        };
        let text = |tree: &Tree| index_of(slot(tree, 1).children[0]) as usize;
        let state = |tree: &Tree| tree.slots[text(tree)].state.clone().unwrap();
        tree.begin(1., 0);
        tree.commit_root(
            page(0xff0000ff, node().id(1usize).child("a").into()),
            viewport,
            1.,
        );
        let first = state(&tree);
        tree.begin(1., 0);
        tree.commit_root(page(0x0000ffff, Child::Kept(1usize.into())), viewport, 1.);
        assert_eq!(
            tree.slots[text(&tree)].text.color,
            crate::Color::hex(0x0000ffff)
        );
        assert!(std::rc::Rc::ptr_eq(&first, &state(&tree)));
        tree.begin(1., 0);
        tree.commit_root(
            page(0x00ff00ff, node().id(1usize).child("a").into()),
            viewport,
            1.,
        );
        assert_eq!(
            tree.slots[text(&tree)].text.color,
            crate::Color::hex(0x00ff00ff)
        );
        assert!(std::rc::Rc::ptr_eq(&first, &state(&tree)));
    }

    #[test]
    fn patches_replace_kept_boxes_in_place() {
        let mut tree = Tree::default();
        let viewport = size(px(100.), px(100.));
        let field = |width: f32| node().id(2usize).w(px(width)).child(node().child("a"));
        let page = |child: Child| {
            node()
                .id(0usize)
                .child(node().id(1usize).child(child))
                .into()
        };
        let width = |tree: &Tree| slot(tree, 2).style.size.width;
        tree.begin(1., 0);
        tree.commit_root(page(field(10.).into()), viewport, 1.);
        let (parent, own, inner) = (
            tree.index(1usize.into()).unwrap(),
            tree.index(2usize.into()).unwrap(),
            slot(&tree, 2).children.clone(),
        );
        let slots = live(&tree);
        tree.begin(1., 0);
        tree.commit_root(Child::Kept(0usize.into()), viewport, 1.);
        tree.patch(field(20.), 1.);
        assert_eq!(width(&tree), px(20.).into());
        assert_eq!(slot(&tree, 1).children, [(own as usize).into()]);
        assert_eq!(slot(&tree, 2).parent, parent);
        assert_eq!(slot(&tree, 2).children, inner);
        assert_eq!(live(&tree), slots);
        tree.begin(1., 0);
        tree.commit_root(page(field(30.).into()), viewport, 1.);
        tree.patch(field(40.), 1.);
        assert_eq!(
            width(&tree),
            px(30.).into(),
            "a box built in this pass keeps its build"
        );
    }

    #[test]
    fn placed_boxes_keep_their_subtree_at_a_new_place() {
        let mut tree = Tree::default();
        let viewport = size(px(100.), px(100.));
        let page = |child: Child| node().id(0usize).child(child).into();
        let place = |x: f32, width: f32| {
            Child::Placed(
                1usize.into(),
                crate::Rect {
                    origin: crate::point(px(x), px(2.)),
                    size: size(px(width), px(8.)),
                },
            )
        };
        tree.begin(1., 0);
        let cell = node()
            .id(1usize)
            .absolute()
            .left(px(1.))
            .top(px(2.))
            .w(px(4.))
            .h(px(8.));
        tree.commit_root(page(cell.child("a").into()), viewport, 1.);
        let (inner, built) = (
            slot(&tree, 1).children.clone(),
            slot(&tree, 1).style.clone(),
        );
        tree.begin(1., 0);
        tree.commit_root(page(place(1., 4.)), viewport, 1.);
        assert!(
            std::rc::Rc::ptr_eq(&built, &slot(&tree, 1).style),
            "the same place keeps the style"
        );
        tree.begin(1., 0);
        tree.commit_root(page(place(10., 30.)), viewport, 1.);
        let cell = slot(&tree, 1);
        assert_eq!(
            [
                cell.style.inset.left,
                cell.style.inset.top,
                cell.style.size.width
            ],
            [px(10.).into(), px(2.).into(), px(30.).into()]
        );
        assert_eq!(cell.children, inner);
    }
}
