use crate::{
    AlignContent, AlignItems, AvailableSpace, Baseline, Clip, Display, Escape, Flow, Hit, Hooks,
    Inline, Overflow, Pixels, Point, Position, Rect, Region, Scope, Size, Style, TextStyle,
    Visibility, Window,
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
    split: Option<Rc<Split>>,
    // Floats are in the box, or below it in blocks of its formatting context.
    floats: bool,
    // The last layout broke the lines of the box beside floats.
    banded: bool,
    // The floats that the box placed in the context of its parent when no other float reached it, from its origin.
    spill: Option<Box<Spill>>,
}

// Where a column set puts a box of its flow, in layout units.
#[derive(Debug, PartialEq)]
pub(crate) struct Split {
    // The move into its column, beyond the move of its parent.
    pub(crate) shift: Point<f32>,
    // A box that spans columns: its part in each column from its moved origin, and for text, the first row of
    // each column after the first with its move.
    pub(crate) rects: Box<[Rect<f32>]>,
    pub(crate) rows: Box<[(u32, Point<f32>)]>,
}

type Spill = (Vec<[f32; 5]>, f32, f32);

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
            split: None,
            floats: false,
            banded: false,
            spill: None,
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
        self.floats = false;
        self.banded = false;
        self.spill = None;
        self.hitbox = NONE;
        self.listed = false;
        self.free = [true; 2];
        self.placed = None;
        self.below = false;
        self.volatile = false;
        self.escaped = None;
        self.split = None;
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
    // A float was in the tree: layout skips the caches of boxes beside floats, so `dirty` goes up to the root.
    floated: bool,
    // Floats wait to paint after the blocks of their formatting context, like in CSS.
    deferred: Vec<u32>,
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
                    || slot.style.inline != styled.inline
                    || slot.style.flow != styled.flow);
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
                if self.slots[child as usize].parent != index {
                    unsplit(&mut self.slots, child as usize);
                }
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
        let floats = self.slots[index as usize].children.iter().any(|&child| {
            let child = &self.slots[index_of(child) as usize];
            child.style.flow & Flow::FLOAT != 0
                || child.floats && child.style.flow & Flow::BLOCK != 0
        });
        self.floated |= floats;
        let slot = &mut self.slots[index as usize];
        (slot.free, slot.below) = (free, below);
        let relaid = mem::replace(&mut slot.floats, floats) != floats || relaid;
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
            // An empty cache means its parents were cleared too, unless it was just made or laid out beside floats.
            if matches!(slot.cache.clear(), ClearState::AlreadyEmpty)
                && !self.floated
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
            floats: Vec::new(),
            ceiling: f32::NEG_INFINITY,
            reach: f32::NEG_INFINITY,
            origin: [0.; 2],
            active: false,
            joined: false,
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
        let mut origin = point(parent.x + location.x, parent.y + location.y);
        if let Some(split) = &slot.split {
            origin += split.shift;
        }
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
        self.reach(index, window.scale_factor());
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
        // A box that spans columns takes pointer events in its first part.
        let first = parts(&self.slots[index as usize], bounds.origin, scale)
            .next()
            .unwrap_or(bounds);
        window.scope(Scope::Clip(style.overflow_mask(bounds)), |window| {
            let hitbox = needs_hitbox.then(|| window.add_region(first, Hit::Pass));
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
        let Layout {
            mut location, size, ..
        } = slot.layout;
        if let Some(split) = &slot.split {
            (location.x, location.y) = (location.x + split.shift.x, location.y + split.shift.y);
        }
        if let Some(far) = slot.far[nested as usize].filter(|_| !self.full) {
            return point(location.x + far.x, location.y + far.y);
        }
        let visible = slot
            .style
            .overflow
            .map(|overflow| overflow == Overflow::Visible);
        // The flow of a column set is one tall column, and only the boxes that it moved into columns count.
        let flow = slot.parent != NONE && self.slots[slot.parent as usize].style.columns.is_some();
        let parted = slot
            .split
            .as_ref()
            .is_some_and(|split| !split.rects.is_empty());
        let mut far = match flow || parted {
            true => point(size.width, 0.),
            false => point(size.width, size.height),
        };
        for part in slot.split.iter().flat_map(|split| &split.rects) {
            far = far.max(point(part.right(), part.bottom()));
        }
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
            let shift = child
                .split
                .as_ref()
                .map_or(Point::default(), |split| split.shift);
            let at = point(
                origin.x + layout.location.x + shift.x,
                origin.y + layout.location.y + shift.y,
            );
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
        let mut origin = point(parent.x + location.x, parent.y + location.y);
        if let Some(split) = &slot.split {
            origin += split.shift;
        }
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
            self.reach(index, window.scale_factor());
        }
    }

    fn keep_box(&mut self, index: u32, style: &Style, moved: bool, window: &mut Window) {
        let origin = self.slots[index as usize].origin;
        let [bounds, _] = match moved {
            true => self.place(index, origin, window),
            false => [self.slots[index as usize].painted; 2],
        };
        let scale = window.scale_factor();
        let first = parts(&self.slots[index as usize], bounds.origin, scale)
            .next()
            .unwrap_or(bounds);
        window.scope(Scope::Clip(style.overflow_mask(bounds)), |window| {
            let slot = &mut self.slots[index as usize];
            if let Some(hitbox) = self.previous.get(slot.hitbox as usize) {
                self.hitboxes.push(window.move_region(hitbox, first));
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
    fn reach(&mut self, index: u32, scale: f32) {
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
                for part in parts(slot, slot.painted.origin, scale) {
                    reach = reach.union(part.outset(grow));
                }
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
        let (text, state, split) = (slot.text.clone(), slot.state.clone(), slot.split.clone());
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
            split: split.as_deref(),
        };
        step(&*hooks, &mut cx, &mut |window| inner(self, &style, window));
    }

    fn baselines(&self, index: u32, scale: f32) -> [Option<Pixels>; 2] {
        let Baselines { first, last } = self.slots[index as usize].baselines;
        [first, last].map(|value| value.map(|value| Pixels(value / scale)))
    }

    pub(crate) fn paint(&mut self, root: NodeId, window: &mut Window) {
        self.painting.clear();
        self.deferred.clear();
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
        let pieces: Vec<_> = parts(slot, bounds.origin, window.scale_factor()).collect();
        window.scope(Scope::Opacity(style.opacity), |window| {
            let full = style.corner_radii.resolve(bounds.size);
            let count = pieces.len().max(1);
            let pieces = pieces.iter().copied();
            // Like Chrome's `box-decoration-break: slice`, a part has no border and no corners at a cut.
            for (at, part) in pieces
                .chain((count == 1).then_some(bounds))
                .take(count)
                .enumerate()
            {
                let (first, last) = (at == 0, at + 1 >= count);
                let mut radii = full;
                for corners in [&mut radii.x, &mut radii.y] {
                    if !first {
                        (corners.top_left, corners.top_right) = (px(0.), px(0.));
                    }
                    if !last {
                        (corners.bottom_left, corners.bottom_right) = (px(0.), px(0.));
                    }
                }
                let sides = if first && last {
                    0
                } else {
                    10 | u8::from(first) | u8::from(last) << 2
                };
                window.draw_shadows(part, radii, &style.box_shadow, false);
                if let Some(background) = style.background.filter(|color| !color.is_transparent()) {
                    window.draw_box(part, radii, background, Default::default());
                }
                window.draw_shadows(part, radii, &style.box_shadow, true);
                if let Some(color) = style.border_color.filter(|color| !color.is_transparent())
                    && style.border_widths.any(|width| !width.is_zero())
                {
                    window.draw_box(
                        part,
                        radii,
                        color.with_alpha(0.),
                        crate::Border {
                            widths: style.border_widths,
                            color,
                            style: style.border_style,
                            sides,
                        },
                    );
                }
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
                            split: slot.split.as_deref(),
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
                    let mark = self.deferred.len();
                    for child in 0..self.slots[index as usize].children.len() {
                        let child = index_of(self.slots[index as usize].children[child]);
                        let slot = &self.slots[child as usize];
                        if slot.style.flow & Flow::FLOAT != 0 && slot.style.z_index.is_none() {
                            self.deferred.push(child);
                        } else {
                            self.paint_node(child, false, window);
                        }
                    }
                    // A plain block passes its floats up. Other boxes paint them above their blocks, with their
                    // clip, scroll offset and effects.
                    if style.flow & Flow::BLOCK == 0
                        || style.stacking
                        || style.z_index.is_some()
                        || style.opacity.is_some()
                        || style.overflow.x != Overflow::Visible
                        || style.overflow.y != Overflow::Visible
                    {
                        // A float catches the floats inside it, so painting one doesn't grow the list.
                        for at in mark..self.deferred.len() {
                            self.paint_node(self.deferred[at], false, window);
                        }
                        self.deferred.truncate(mark);
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

const FORCED: u8 = 1;
const AVOID: u8 = 2;
const END: u8 = 3;
// The space below the content of a box, where a break can come anywhere: its chance holds where it starts and ends.
const SLICE: u8 = 4;

// Whether columns can break inside a box of their flow: in text between rows, in a flow of blocks or rows of
// inline boxes between them. Other boxes stay whole, and so do boxes with out-of-flow children, like the layers of
// backgrounds and outlines, which paint over the whole box.
fn splits(slots: &[Slot], index: usize) -> bool {
    let slot = &slots[index];
    let (style, taffy) = (&slot.style, slot.taffy());
    match slot.kind {
        Kind::Text(_) => slot.children.is_empty(),
        Kind::Box => {
            taffy.display == taffy::Display::Flex
                && (taffy.flex_direction == FlexDirection::Column
                    && taffy.flex_wrap == taffy::FlexWrap::NoWrap
                    || style.inline == Inline::Lines)
                && style.columns.is_none()
                && style.breaks & crate::Break::WHOLE == 0
                && style.overflow.x == Overflow::Visible
                && style.overflow.y == Overflow::Visible
                && style.escape == 0
                && slot.hooks.as_ref().is_none_or(|hooks| hooks.bounded())
                && slot
                    .children
                    .iter()
                    .all(|&child| slots[usize::from(child)].style.position != Position::Absolute)
        }
        _ => false,
    }
}

// The break chances inside a box at `top` of a column flow, in order: where the next column starts, where the
// content before it ends, and FORCED or AVOID.
fn chances_in(
    slots: &[Slot],
    index: usize,
    top: f32,
    avoid: bool,
    scale: f32,
    out: &mut Vec<(f32, f32, u8)>,
) -> f32 {
    let slot = &slots[index];
    let avoid = avoid || slot.style.breaks & crate::Break::AVOID_INSIDE != 0;
    if let Kind::Text(_) = slot.kind {
        let Layout {
            size,
            border,
            padding,
            ..
        } = slot.layout;
        let line = slot.text.line_height.0 * scale;
        let start = top + border.top + padding.top;
        let height = slot.measured.height;
        let rows = (height / line).round();
        // Rows of other heights make no even grid of lines.
        if line > 0. && (rows * line - height).abs() <= 0.5 {
            let rows = rows as u32;
            let [orphans, widows] = slot.text.lines_kept.map(u32::from);
            // Like Chrome, text with fewer rows than both keep needs only `orphans`.
            let widows = if rows < orphans + widows { 0 } else { widows };
            for row in 1..rows {
                let y = start + row as f32 * line;
                let kept = avoid || row < orphans || rows - row < widows;
                out.push((y, y, if kept { AVOID } else { 0 }));
            }
        }
        let (content, bottom) = (start + height, top + size.height);
        if bottom > content + 0.01 && !avoid {
            out.push((content, bottom, SLICE));
        }
        return content.min(bottom);
    }
    let (mut last, mut used): (Option<(f32, u8)>, f32) = (None, top);
    for &child in &slot.children {
        let child = usize::from(child);
        let other = &slots[child];
        if other.style.position == Position::Absolute || other.style.display == Display::None {
            continue;
        }
        let at = top + other.layout.location.y - lift(other);
        let breaks = other.style.breaks;
        if let Some((bottom, before)) = last
            && at >= bottom - 0.01
        {
            use crate::Break;
            let kind = if (before & Break::AFTER) | (breaks & Break::BEFORE) != 0 {
                FORCED
            } else if avoid || (before & Break::AVOID_AFTER) | (breaks & Break::AVOID_BEFORE) != 0 {
                AVOID
            } else {
                0
            };
            // Like Chrome, the margin at the top of a column is kept only after a forced break.
            let cut = match kind {
                FORCED => at - other.layout.margin.top.max(0.),
                _ => at,
            };
            out.push((cut, bottom, kind));
        }
        if splits(slots, child) {
            chances_in(slots, child, at, avoid, scale, out);
        }
        let bottom = at + other.layout.size.height;
        last = Some((last.map_or(bottom, |(end, _)| end.max(bottom)), breaks));
        used = used.max(bottom + other.layout.margin.bottom.max(0.));
    }
    // Like Chrome, a box taller than its content breaks anywhere below it.
    let content = last.map_or(top, |(bottom, _)| bottom);
    let bottom = top + slot.layout.size.height;
    if bottom > used + 0.01 && !avoid {
        out.push((used, bottom, SLICE));
    }
    content
}

// The offset of a relatively positioned box, which moves it but not its place in a column flow.
fn lift(slot: &Slot) -> f32 {
    let style = slot.taffy();
    let length = |side: taffy::LengthPercentageAuto| {
        let raw = side.into_raw();
        (raw.tag() == taffy::CompactLength::LENGTH_TAG).then(|| raw.value())
    };
    match style.position {
        taffy::Position::Relative => length(style.inset.top)
            .or_else(|| length(style.inset.bottom).map(|bottom| -bottom))
            .unwrap_or(0.),
        _ => 0.,
    }
}

// The parts of a box that spans columns, in window pixels from `origin`, its place.
fn parts(
    slot: &Slot,
    origin: Point<Pixels>,
    scale: f32,
) -> impl Iterator<Item = Rect<Pixels>> + '_ {
    let parts = slot.split.as_deref().map_or(&[][..], |split| &split.rects);
    parts
        .iter()
        .map(move |part| (*part / scale).map(Pixels) + origin)
}

// Clears the column places of a box, and of the boxes inside it when it spanned columns.
fn unsplit(slots: &mut [Slot], index: usize) {
    if let Some(split) = slots[index].split.take() {
        changed(slots, index);
        if !split.rects.is_empty() {
            for child in 0..slots[index].children.len() {
                unsplit(slots, usize::from(slots[index].children[child]));
            }
        }
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
        (
            self.breaks,
            self.text.lines_kept,
            self.columns.as_deref().map(|columns| {
                let height = columns.height.map(|height| height.0.to_bits());
                (columns.count, columns.gap.0.to_bits(), height, columns.fill)
            }),
        )
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
            self.lines_kept,
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
    // The margin boxes `[left, top, right, bottom, right side]` of the floats in the formatting context, from the
    // border box of its root.
    floats: Vec<[f32; 5]>,
    // The lowest top and bottom of all floats of the context, also the ones that ended above the current box.
    ceiling: f32,
    reach: f32,
    // Where the border box of the box that `stack` lays out next is in the formatting context.
    origin: [f32; 2],
    // The next box that `compute` lays out is beside floats: it skips the cache, and `stack` places it in the
    // formatting context of its parent.
    active: bool,
    joined: bool,
}

impl LayoutTree<'_> {
    fn compute(
        &mut self,
        node: NodeId,
        mut inputs: LayoutInput,
        block: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        let joined = mem::take(&mut self.active);
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
        let layout = |tree: &mut Self, node: NodeId, inputs: LayoutInput| {
            tree.joined = joined;
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
            if tree.slots[usize::from(node)].style.columns.is_some() {
                output = tree.columns(node, inputs, output);
            }
            if inputs.run_mode == RunMode::PerformLayout {
                taffy::compute_oof_layout(tree, node, &mut output);
                output.baselines = tree.baselines(usize::from(node), &output);
            }
            output
        };
        let index = usize::from(node);
        // A box with floats that no float of its context reaches lays out the same each time: it keeps its cache,
        // and the floats it placed.
        let quiet = joined
            && self.slots[index].floats
            && self.floats.iter().all(|float| float[3] <= self.origin[1]);
        let output = if quiet {
            let [x, y] = self.origin;
            self.floats.clear();
            match self.cache_get(node, &inputs) {
                Some(output) if self.slots[index].spill.is_some() => {
                    let (floats, ceiling, reach) = self.slots[index].spill.as_deref().unwrap();
                    self.floats.extend(
                        floats
                            .iter()
                            .map(|f| [f[0] + x, f[1] + y, f[2] + x, f[3] + y, f[4]]),
                    );
                    (self.ceiling, self.reach) =
                        (self.ceiling.max(ceiling + y), self.reach.max(reach + y));
                    output
                }
                _ => {
                    let outer = (self.ceiling, self.reach);
                    (self.ceiling, self.reach) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
                    let output = layout(self, node, inputs);
                    let floats = self
                        .floats
                        .iter()
                        .map(|f| [f[0] - x, f[1] - y, f[2] - x, f[3] - y, f[4]])
                        .collect();
                    self.slots[index].spill =
                        Some(Box::new((floats, self.ceiling - y, self.reach - y)));
                    (self.ceiling, self.reach) =
                        (outer.0.max(self.ceiling), outer.1.max(self.reach));
                    self.cache_store(node, &inputs, output.clone());
                    output
                }
            }
        } else if joined {
            // Beside floats, the result depends on them, so the cache is neither read nor stored, and a later
            // layout without floats must not find a result from before them.
            self.slots[index].cache.clear();
            layout(self, node, inputs)
        } else {
            taffy::compute_cached_layout(self, node, inputs, layout)
        };
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

    // A column set lays out its flow as one column, then breaks it into columns of one height and moves its boxes
    // there. The set is as tall as a column.
    fn columns(&mut self, node: NodeId, inputs: LayoutInput, output: LayoutOutput) -> LayoutOutput {
        let index = usize::from(node);
        let (Some(columns), Some(&flow)) = (
            self.slots[index].style.columns.as_deref().copied(),
            self.slots[index].children.first(),
        ) else {
            return output;
        };
        // Without a width, the set is as wide as its columns of the content's width and the gaps between them.
        if inputs.known_dimensions.width.is_none() {
            let mut output = output;
            let count = f32::from(columns.count);
            output.size.width =
                output.size.width * count + columns.gap.0 * self.scale * (count - 1.);
            return output;
        }
        if inputs.run_mode != RunMode::PerformLayout {
            let inputs = LayoutInput {
                run_mode: RunMode::PerformLayout,
                ..inputs
            };
            return LayoutOutput::from_outer_size(self.compute(node, inputs, None).size);
        }
        let flow = usize::from(flow);
        let Layout { size: one, .. } = self.slots[flow].layout;
        let pitch = one.width + columns.gap.0 * self.scale;
        let mut chances = Vec::new();
        // Like Chrome, the margin at the end of the content takes no room.
        let end = chances_in(self.slots, flow, 0., false, self.scale, &mut chances);
        chances.push((f32::INFINITY, end, END));
        // Greedy fill: the column starts, the least extra height that would fit more, whether a break that should
        // be avoided was taken or the last column overflows, and the tallest content of a column.
        let fill = |height: f32| {
            let (mut cuts, mut short, mut avoided, mut tallest) =
                (vec![0f32], f32::INFINITY, false, 0f32);
            let (mut fit, mut fit_avoided, mut at) = (None, None, 0);
            while let Some(&(cut, end, bits)) = chances.get(at) {
                let start = cuts[cuts.len() - 1];
                if bits == SLICE {
                    // The column ends inside the space: break there. Else the space fits up to its end, or a
                    // whole piece before it overflows a column of its own.
                    if end - start - height > 0.01 && cut <= start + height + 0.01 && height > 0.01
                    {
                        // Like Chrome, the part of the box that did not fit is a shortage too.
                        short = short.min(end - start - height);
                        cuts.push(start + height);
                        (fit, fit_avoided, tallest) = (None, None, tallest.max(height));
                        continue;
                    }
                    if end <= start + height + 0.01 {
                        fit = Some(at);
                    } else if cut > start + height {
                        short = short.min(cut - start - height);
                        if fit.is_none() && fit_avoided.is_none() {
                            cuts.push(cut);
                            tallest = tallest.max(cut - start);
                            continue;
                        }
                    }
                    at += 1;
                    continue;
                }
                if cut <= start {
                    at += 1;
                    continue;
                }
                let over = end - start - height;
                if over <= 0.01 {
                    match bits {
                        FORCED => {
                            cuts.push(cut);
                            (fit, fit_avoided) = (None, None);
                            tallest = tallest.max(end - start);
                        }
                        AVOID => fit_avoided = Some(at),
                        END => tallest = tallest.max(end - start),
                        _ => fit = Some(at),
                    }
                    at += 1;
                    continue;
                }
                short = short.min(over);
                let pick = fit.or(fit_avoided).unwrap_or(at);
                avoided |= fit.is_none() && chances[pick].2 == AVOID;
                if chances[pick].2 == END {
                    avoided = true;
                    break;
                }
                let (cut, end, bits) = chances[pick];
                cuts.push(if bits == SLICE { end } else { cut });
                tallest = tallest.max(end - start);
                (fit, fit_avoided, at) = (None, None, pick + 1);
            }
            (cuts, short, avoided, tallest)
        };
        let count = usize::from(columns.count);
        let limit = columns.height.map(|height| height.0 * self.scale);
        // Like Chrome, start from the runs between forced breaks with their end margins, and give the other
        // breaks to the tallest runs.
        let mut runs: Vec<(f32, f32)> = Vec::new();
        let mut from = 0.;
        for &(cut, _, bits) in &chances {
            if bits == FORCED {
                runs.push((cut - from, 1.));
                from = cut;
            }
        }
        runs.push((one.height - from, 1.));
        for _ in runs.len()..count {
            let tallest = runs
                .iter_mut()
                .max_by(|a, b| (a.0 / a.1).total_cmp(&(b.0 / b.1)));
            if let Some(run) = tallest {
                run.1 += 1.;
            }
        }
        let mut height = runs.iter().map(|run| run.0 / run.1).fold(0., f32::max);
        if let Some(limit) = limit.filter(|limit| columns.fill || *limit < height) {
            height = limit;
        }
        // Balance like Chrome: grow the height by the least shortage until the content fits. A fixed number of
        // steps keeps the cost linear in the content, and long content then searches between that height and one
        // that fits.
        let fits =
            |(cuts, _, avoided, _): &(Vec<f32>, f32, bool, f32)| cuts.len() <= count && !avoided;
        let mut done = fill(height);
        if !(columns.fill && limit.is_some()) {
            let upper = limit.unwrap_or(f32::INFINITY).min(one.height.max(height));
            for _ in 0..16 {
                if fits(&done) || !done.1.is_finite() || height >= upper {
                    break;
                }
                height = (height + done.1.max(0.01)).min(upper);
                done = fill(height);
            }
            if !fits(&done) && done.1.is_finite() && height < upper {
                let mut low = height;
                height = upper;
                if fits(&fill(height)) {
                    for _ in 0..24 {
                        let middle = (low + height) / 2.;
                        if fits(&fill(middle)) {
                            height = middle;
                        } else {
                            low = middle;
                        }
                    }
                    // The tallest column of the found breaks is the exact height that holds them.
                    height = fill(height).3;
                }
                done = fill(height);
            }
        }
        let cuts = done.0;
        self.assign(flow, 0., Point::default(), &cuts, height, pitch);
        let mut output = output;
        output.size.height = height;
        if let Some(area) = &mut output.oof_positioning_area {
            area.size.height = height;
        }
        output
    }

    fn assign(
        &mut self,
        index: usize,
        top: f32,
        base: Point<f32>,
        cuts: &[f32],
        height: f32,
        pitch: f32,
    ) {
        let column = |y: f32| {
            cuts.partition_point(|&cut| cut <= y + 0.01)
                .saturating_sub(1)
        };
        let place = |column: usize| point(column as f32 * pitch, -cuts[column]);
        for child in 0..self.slots[index].children.len() {
            let child = usize::from(self.slots[index].children[child]);
            let slot = &self.slots[child];
            let (at, extent) = (top + slot.layout.location.y - lift(slot), slot.layout.size);
            let first = column(at);
            let last = column(at + (extent.height - 0.02).max(0.));
            let shift = place(first);
            let spans = first != last && splits(self.slots, child);
            let mut split = Split {
                shift: shift - base,
                rects: Box::default(),
                rows: Box::default(),
            };
            if spans {
                split.rects = (first..=last)
                    .map(|column| {
                        let from = if column == first { at } else { cuts[column] };
                        let to = if column == last {
                            at + extent.height
                        } else {
                            (cuts[column] + height).max(cuts[column + 1])
                        };
                        let y = from - cuts[column] - at + cuts[first];
                        Rect::new(
                            point((column - first) as f32 * pitch, y),
                            size(extent.width, to - from),
                        )
                    })
                    .collect();
                if let Kind::Text(_) = slot.kind {
                    let line = slot.text.line_height.0 * self.scale;
                    let start = at + slot.layout.border.top + slot.layout.padding.top;
                    split.rows = (first + 1..=last)
                        .map(|column| {
                            let row = ((cuts[column] - start) / line).round().max(0.) as u32;
                            (row, place(column) - shift)
                        })
                        .collect();
                }
            }
            let split = (split.shift != Point::default() || spans).then(|| Rc::new(split));
            if self.slots[child].split != split {
                let old = mem::replace(&mut self.slots[child].split, split);
                if old.is_some_and(|old| !old.rects.is_empty()) && !spans {
                    for inner in 0..self.slots[child].children.len() {
                        unsplit(self.slots, usize::from(self.slots[child].children[inner]));
                    }
                }
                if !self.exact {
                    changed(self.slots, child);
                }
            }
            if spans && matches!(self.slots[child].kind, Kind::Box) {
                self.assign(child, at, shift, cuts, height, pitch);
            }
        }
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
        let joined = mem::take(&mut self.joined);
        let index = usize::from(node);
        if self.slots[index].floats
            && run_mode == RunMode::ComputeSize
            && axis == taffy::RequestedAxis::Horizontal
            && known.width.is_none()
            && self.slots[index].taffy().flex_direction == FlexDirection::Column
            && self.slots[index].style.inline != Inline::Lines
        {
            // Like Chrome, floats side by side add up in the max-content width, with the box after them.
            use taffy::ResolveOrZero;
            let calc = |value, basis| crate::style::resolve_calc(value, basis);
            let style = self.slots[index].taffy();
            let sides = (style.padding.resolve_or_zero(parent_size.width, calc)
                + style.border.resolve_or_zero(parent_size.width, calc))
            .horizontal_axis_sum();
            let most = available_space.width != taffy::AvailableSpace::MinContent;
            let (mut widest, mut run) = (0f32, [0f32; 2]);
            for order in 0..self.slots[index].children.len() {
                let child = self.slots[index].children[order];
                let slot = &self.slots[usize::from(child)];
                let (bits, style) = (slot.style.flow, slot.taffy());
                if style.display == taffy::Display::None
                    || style.position == taffy::Position::Absolute
                {
                    continue;
                }
                let margin = style
                    .margin
                    .resolve_or_zero(None, calc)
                    .horizontal_axis_sum();
                let probe = LayoutInput {
                    known_dimensions: taffy::Size::NONE,
                    parent_size: taffy::Size::NONE,
                    available_space: taffy::Size {
                        width: available_space.width,
                        height: taffy::AvailableSpace::MaxContent,
                    },
                    sizing_mode: taffy::SizingMode::InherentSize,
                    ..inputs
                };
                let size = self.compute(child, probe, None).size.width + margin;
                if bits & Flow::CLEAR_LEFT != 0 {
                    run[0] = 0.;
                }
                if bits & Flow::CLEAR_RIGHT != 0 {
                    run[1] = 0.;
                }
                if most && bits & Flow::FLOAT != 0 {
                    run[usize::from(bits & Flow::RIGHT != 0)] += size.max(0.);
                    widest = widest.max(run[0] + run[1]);
                } else {
                    widest = widest.max(size + if most { run[0] + run[1] } else { 0. });
                    run = [0.; 2];
                }
            }
            return Some(LayoutOutput::from_outer_size(taffy::Size {
                width: widest + sides,
                height: 0.,
            }));
        }
        let width = known.width?;
        let style = self.slots[index].taffy();
        // Floats lay out the same way in both passes, so the debug check compares only cached results.
        let floating = joined || self.slots[index].floats;
        let calc = |value, basis| crate::style::resolve_calc(value, basis);
        let starts = |align: Option<taffy::AlignItems>| {
            align.is_none_or(|align| {
                matches!(
                    align.keyword,
                    AlignItemsKeyword::Normal | AlignItemsKeyword::Stretch
                )
            })
        };
        // A box with floats must not fall back to flexbox, which stacks them as blocks.
        if style.flex_direction != FlexDirection::Column
            || style.flex_wrap != taffy::FlexWrap::NoWrap
            || style.gap.height.into_raw().tag() != CompactLength::LENGTH_TAG
            || self.slots[index].style.inline == Inline::Lines
            || !floating
                && (self.exact
                    || style.direction != taffy::Direction::Ltr
                    || style.box_sizing != taffy::BoxSizing::BorderBox
                    || style.aspect_ratio.is_some()
                    || !starts(Some(style.align_items)))
        {
            return None;
        }
        let flow = |child: &taffy::Style| {
            child.position != taffy::Position::Absolute && child.display != taffy::Display::None
        };
        let simple = self.slots[index].children.iter().all(|&child| {
            let floated = self.slots[usize::from(child)].style.flow & Flow::FLOAT != 0;
            let child = self.slots[usize::from(child)].taffy();
            match child.position {
                _ if floating && child.position != taffy::Position::Absolute => {
                    floated || child.box_sizing == taffy::BoxSizing::BorderBox
                }
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
        let ratio = style
            .aspect_ratio
            .filter(|_| floating && styled.is_none())
            .map(|ratio| width / ratio);
        let fixed = known.height.or(definite.or(styled).or(ratio).maybe_max(pb));
        let main = inset.top + inset.bottom;
        let cross = inset.left + inset.right;
        if run_mode == RunMode::ComputeSize
            && (fixed.is_some() && !floating || axis == taffy::RequestedAxis::Horizontal)
            || width.maybe_clamp(min.width, max.width) != width && !floating
        {
            return None;
        }
        let width = width.maybe_clamp(min.width, max.width);
        // A box beside floats is in the formatting context of its parent, and other boxes with floats make one.
        let context = (floating && !joined).then(|| {
            let outer = (
                mem::take(&mut self.floats),
                self.origin,
                self.ceiling,
                self.reach,
            );
            (self.ceiling, self.reach) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
            outer
        });
        let origin = if joined { self.origin } else { [0.; 2] };
        let edges = [origin[0] + inset.left, origin[0] + width - inset.right];
        let inner = width - cross;
        let line = (width - cross).max(0.);
        let gap = style.gap.height.into_raw().value();
        let (justify, contain, aligned) = (
            style.justify_content.keyword(),
            style.contain,
            style.align_items,
        );
        let perform = run_mode == RunMode::PerformLayout;
        let mut total = inset.top;
        let (mut items, mut hypothetical, mut contents, mut grows, mut shrinks) =
            (0, 0., 0., false, false);
        let mut first = None;
        // `plain` keeps percent heights out of the other columns.
        let tall = fixed
            .filter(|_| floating)
            .map(|height| (height - main).max(0.));
        // The offsets of a relatively positioned box.
        let shift = |offsets: taffy::Rect<taffy::LengthPercentageAuto>| {
            let side =
                |start: taffy::LengthPercentageAuto, end: taffy::LengthPercentageAuto, basis| {
                    let start: Option<f32> = start.maybe_resolve(basis, calc);
                    let end: Option<f32> = end.maybe_resolve(basis, calc);
                    start.or(end.map(|end| -end)).unwrap_or(0.)
                };
            [
                side(offsets.left, offsets.right, Some(inner)),
                side(offsets.top, offsets.bottom, tall),
            ]
        };
        for order in 0..self.slots[index].children.len() {
            let child = self.slots[index].children[order];
            let style = self.slots[usize::from(child)].taffy();
            if !flow(style) {
                continue;
            }
            let mut margin = style.margin.resolve_or_zero(Some(inner), calc);
            let (offsets, sized_by, self_align) = (style.inset, style.size.width, style.align_self);
            let [auto_left, auto_right] =
                [style.margin.left.is_auto(), style.margin.right.is_auto()];
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
            let input = |run_mode, axis, width: Option<f32>, space| LayoutInput {
                run_mode,
                sizing_mode: taffy::SizingMode::InherentSize,
                axis,
                known_dimensions: taffy::Size {
                    width,
                    height: None,
                },
                known_dimensions_are_definite: taffy::Size {
                    width: true,
                    height: true,
                },
                parent_size: taffy::Size {
                    width: Some(inner),
                    height: tall,
                },
                available_space: taffy::Size {
                    width: space,
                    height: taffy::AvailableSpace::MaxContent,
                },
                vertical_margins_are_collapsible: taffy::Line::FALSE,
            };
            let (run, requested) = if perform {
                (RunMode::PerformLayout, taffy::RequestedAxis::Both)
            } else {
                (RunMode::ComputeSize, taffy::RequestedAxis::Vertical)
            };
            let sized = |width: f32| {
                input(
                    run,
                    requested,
                    Some(width),
                    taffy::AvailableSpace::Definite(width),
                )
            };
            let bits = if floating {
                self.slots[usize::from(child)].style.flow
            } else {
                0
            };
            if bits & Flow::FLOAT != 0 {
                let width = match style.size.width.maybe_resolve(Some(inner), calc) {
                    Some(width) => width.maybe_clamp(min, max).max(sides.width),
                    None => {
                        let mut probe = |space| {
                            let probe = input(
                                RunMode::ComputeSize,
                                taffy::RequestedAxis::Horizontal,
                                None,
                                space,
                            );
                            self.compute(child, probe, None).size.width
                        };
                        let low = probe(taffy::AvailableSpace::MinContent);
                        let high = probe(taffy::AvailableSpace::MaxContent);
                        high.min(line - margin.left - margin.right)
                            .max(low)
                            .maybe_clamp(min, max)
                            .max(sides.width)
                    }
                };
                // A full layout also when the parent only measures, since some boxes measure to another height
                // than they lay out, and the floats of both passes must be the same.
                let output = self.compute(
                    child,
                    input(
                        RunMode::PerformLayout,
                        taffy::RequestedAxis::Both,
                        Some(width),
                        taffy::AvailableSpace::Definite(width),
                    ),
                    None,
                );
                let size = [
                    width + margin.left + margin.right,
                    output.size.height + margin.top + margin.bottom,
                ];
                let [x, y] = self.place(edges, size, origin[1] + total, bits);
                if perform {
                    let mut layout = Layout::with_order(order as u32);
                    layout.size = taffy::Size {
                        width,
                        height: output.size.height,
                    };
                    let [right, down] = shift(offsets);
                    layout.location = taffy::Point {
                        x: x - origin[0] + margin.left + right,
                        y: y - origin[1] + margin.top + down,
                    };
                    layout.padding = padding;
                    layout.border = border;
                    layout.margin = margin;
                    self.set_unrounded_layout(child, &layout);
                }
                continue;
            }
            let margins = margin.top + margin.bottom;
            let offset = if items == 0 { 0. } else { gap };
            let mut top = total + offset + margin.top;
            let mut left = inset.left + margin.left;
            let mut width = width;
            let mut drop = 0.;
            // `plain` keeps sizing keywords and other alignments out of the other columns.
            let align = self_align.unwrap_or(aligned).keyword;
            if floating
                && (sized_by.is_sizing_keyword()
                    || sized_by.is_auto()
                        && !matches!(
                            align,
                            AlignItemsKeyword::Normal | AlignItemsKeyword::Stretch
                        ))
            {
                let mut probe = |space| {
                    let probe = input(
                        RunMode::ComputeSize,
                        taffy::RequestedAxis::Horizontal,
                        None,
                        space,
                    );
                    self.compute(child, probe, None).size.width
                };
                let low = probe(taffy::AvailableSpace::MinContent);
                let high = probe(taffy::AvailableSpace::MaxContent);
                let room = line - margin.left - margin.right;
                width = high
                    .min(room)
                    .max(low)
                    .maybe_clamp(min, max)
                    .max(sides.width);
                left += match align {
                    AlignItemsKeyword::Center => (room - width) / 2.,
                    AlignItemsKeyword::End | AlignItemsKeyword::FlexEnd => room - width,
                    _ => 0.,
                };
            }
            let output = if floating {
                let slot = &self.slots[usize::from(child)];
                let (measured, lines) =
                    (slot.measure.is_some(), slot.style.inline == Inline::Lines);
                // A line box of one paragraph, maybe with absolute boxes like the inline-blocks of its lines.
                let mut lines_of = slot
                    .children
                    .iter()
                    .filter(|&&kid| flow(self.slots[usize::from(kid)].taffy()));
                let leaf = lines
                    .then(|| lines_of.next().filter(|_| lines_of.next().is_none()))
                    .flatten()
                    .copied()
                    .filter(|&only| {
                        let only = &self.slots[usize::from(only)];
                        only.measure.is_some() && only.style.flow & Flow::FLOAT == 0
                    });
                if bits & (Flow::CLEAR_LEFT | Flow::CLEAR_RIGHT) != 0 {
                    let clear = self.cleared(bits) - origin[1];
                    if clear > top {
                        (drop, top) = (clear - top, clear);
                    }
                }
                let y = origin[1] + top;
                self.floats.retain(|float| float[3] > y);
                let beside = !self.floats.is_empty();
                // `plain` keeps auto margins out of the other columns.
                if auto_left || auto_right {
                    let free = (line - width - margin.left - margin.right).max(0.);
                    let share = match (auto_left, auto_right) {
                        (true, true) => free / 2.,
                        (true, false) => free,
                        _ => 0.,
                    };
                    (margin.left, left) = (margin.left + share, left + share);
                }
                if bits & Flow::BLOCK != 0
                    && (slot.floats || beside)
                    && (measured || leaf.is_some() || !lines)
                {
                    if measured || leaf.is_some() {
                        let target = leaf.unwrap_or(child);
                        let own = self.slots[usize::from(target)].taffy();
                        let [own_padding, own_border] = [own.padding, own.border]
                            .map(|edges| edges.resolve_or_zero(Some(inner), calc));
                        // A measured block is the target itself: `left` and `y` hold its margins already.
                        let (outer, own_margin) = match leaf {
                            Some(_) => (
                                padding + border,
                                own.margin.resolve_or_zero(Some(inner), calc),
                            ),
                            None => (taffy::Rect::zero(), taffy::Rect::zero()),
                        };
                        let within =
                            width - outer.left - outer.right - own_margin.left - own_margin.right;
                        let content = [
                            origin[0]
                                + left
                                + outer.left
                                + own_margin.left
                                + own_padding.left
                                + own_border.left,
                            y + outer.top + own_margin.top + own_padding.top + own_border.top,
                        ];
                        let span = within
                            - own_padding.left
                            - own_padding.right
                            - own_border.left
                            - own_border.right;
                        let scale = self.scale;
                        let floats: Vec<[f32; 4]> = self
                            .floats
                            .iter()
                            .filter(|float| float[3] > content[1])
                            .filter_map(|float| {
                                let sides = if float[4] == 0. {
                                    [(float[2] - content[0]).max(0.), 0.]
                                } else {
                                    [0., (content[0] + span - float[0]).max(0.)]
                                };
                                (sides != [0.; 2]).then(|| {
                                    [
                                        float[1] - content[1],
                                        float[3] - content[1],
                                        sides[0],
                                        sides[1],
                                    ]
                                    .map(|value| value / scale)
                                })
                            })
                            .collect();
                        let floats = (!floats.is_empty()).then(|| Rc::from(floats));
                        let system = self.window.text_system().clone();
                        *system.floats.borrow_mut() = floats;
                        self.active = true;
                        // Like a float, a full layout also when the parent only measures: lines beside floats can
                        // measure to another height than they lay out.
                        let full = input(
                            RunMode::PerformLayout,
                            taffy::RequestedAxis::Both,
                            Some(within),
                            taffy::AvailableSpace::Definite(within),
                        );
                        let output = self.compute(target, full, None);
                        *system.floats.borrow_mut() = None;
                        if leaf.is_some() {
                            if perform {
                                let mut layout = Layout::with_order(0);
                                layout.size = output.size;
                                layout.location = taffy::Point {
                                    x: outer.left + own_margin.left,
                                    y: outer.top + own_margin.top,
                                };
                                layout.padding = own_padding;
                                layout.border = own_border;
                                layout.margin = own_margin;
                                self.set_unrounded_layout(target, &layout);
                            }
                            // The line box gets the place and the baselines of its lines.
                            let row = &mut self.slots[usize::from(child)];
                            row.cache.clear();
                            let shift = outer.top + own_margin.top;
                            row.baselines = Baselines {
                                first: output.baselines.first.map(|base| base + shift),
                                last: output.baselines.last.map(|base| base + shift),
                            };
                            let (contain, first) = (row.taffy().contain, row.baselines.first);
                            let size = taffy::Size {
                                width,
                                height: output.size.height
                                    + own_margin.top
                                    + own_margin.bottom
                                    + outer.top
                                    + outer.bottom,
                            };
                            let mut output = finish(size, border, contain, first);
                            output.baselines = self.slots[usize::from(child)].baselines;
                            if perform {
                                // Like `compute`, the line box places its absolute boxes.
                                let mut candidates = taffy::OofCandidates::new();
                                for (order, &kid) in
                                    self.slots[usize::from(child)].children.iter().enumerate()
                                {
                                    if self.slots[usize::from(kid)].taffy().position
                                        == taffy::Position::Absolute
                                    {
                                        let area = |start, end| taffy::AxisStaticPosition {
                                            area: taffy::Line { start, end },
                                            align: taffy::AxisStaticAlign::from_keyword(
                                                taffy::AxisStaticEdge::Start,
                                            ),
                                        };
                                        candidates.push(taffy::OofCandidate {
                                            node: kid,
                                            order: order as u32,
                                            position: taffy::Position::Absolute,
                                            static_position: taffy::Point {
                                                x: area(outer.left, size.width - outer.right),
                                                y: area(outer.top, size.height - outer.bottom),
                                            },
                                        });
                                    }
                                }
                                output.oof_candidates = candidates;
                                taffy::compute_oof_layout(self, child, &mut output);
                            }
                            output
                        } else {
                            output
                        }
                    } else {
                        self.origin = [origin[0] + left, y];
                        self.active = true;
                        let output = self.compute(child, sized(width), None);
                        self.origin = origin;
                        output
                    }
                } else if beside {
                    // A box that makes its own formatting context goes beside the floats, or below them where it
                    // doesn't fit.
                    let auto = sized_by.is_auto();
                    let mut y = y;
                    loop {
                        // The room must hold the box over its whole height, which depends on its width.
                        let (mut tall, mut placed) = (0f32, None);
                        for _ in 0..3 {
                            let (room, low) = self.beside(edges, y, tall);
                            let [from, to] = [
                                (edges[0] + margin.left).max(room[0]),
                                (edges[1] - margin.right).min(room[1]),
                            ];
                            if auto {
                                width = (to - from).maybe_clamp(min, max).max(sides.width);
                            }
                            left = from - origin[0];
                            let output = self.compute(child, sized(width), None);
                            let height = output.size.height;
                            placed = Some((output, width <= to - from + 0.01, low));
                            if height <= tall {
                                break;
                            }
                            tall = height;
                        }
                        let Some((output, fits, low)) = placed else {
                            unreachable!()
                        };
                        if fits || low == f32::INFINITY {
                            break output;
                        }
                        (drop, y) = (drop + low - y, low);
                        top = y - origin[1];
                    }
                } else {
                    self.compute(child, sized(width), None)
                }
            } else {
                self.compute(child, sized(width), None)
            };
            let height = output.size.height;
            // Taffy's content sizes take a fixed height even below the padding, which the box itself doesn't.
            if !floating && styled.tag() == CompactLength::LENGTH_TAG && styled.value() != height {
                return None;
            }
            if perform {
                let mut layout = Layout::with_order(order as u32);
                layout.size = taffy::Size { width, height };
                layout.location = taffy::Point { x: left, y: top };
                // `plain` keeps insets out of the other columns.
                if floating {
                    let [x, y] = shift(offsets);
                    (layout.location.x, layout.location.y) = (left + x, top + y);
                }
                layout.padding = padding;
                layout.border = border;
                layout.margin = margin;
                self.set_unrounded_layout(child, &layout);
                if items == 0 {
                    first = Some(top + output.baselines.first.unwrap_or(height));
                }
            }
            total += offset + margins + height + drop;
            hypothetical += height + margins + drop;
            contents += drop;
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
        if let Some((floats, outer, ceiling, low)) = context {
            // Like a formatting context root, the box holds its floats.
            let reach = self.reach - inset.top - contents;
            if reach > 0. {
                contents += reach;
                hypothetical += reach;
            }
            (self.floats, self.origin, self.ceiling, self.reach) = (floats, outer, ceiling, low);
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
        if free != 0. && (!packed || free > 0. && grows || free < 0. && shrinks) && !floating {
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

    /// Places a float with its margin box `size` at or below `y`, between `edges`, like CSS 2 9.5.1.
    fn place(&mut self, edges: [f32; 2], size: [f32; 2], mut y: f32, bits: u8) -> [f32; 2] {
        let right = bits & Flow::RIGHT != 0;
        // The flow only moves down, so floats that ended above it can't touch this one or the boxes after it.
        self.floats.retain(|float| float[3] > y);
        y = y.max(self.ceiling).max(self.cleared(bits));
        loop {
            let (room, low) = self.beside(edges, y, size[1]);
            if room[1] - room[0] >= size[0] - 0.01 || low == f32::INFINITY {
                let x = if right { room[1] - size[0] } else { room[0] };
                self.floats
                    .push([x, y, x + size[0], y + size[1], f32::from(u8::from(right))]);
                (self.ceiling, self.reach) = (self.ceiling.max(y), self.reach.max(y + size[1]));
                return [x, y];
            }
            y = low;
        }
    }

    /// The room between the floats beside `y..y + height`, and the first bottom of those floats.
    fn beside(&self, edges: [f32; 2], y: f32, height: f32) -> ([f32; 2], f32) {
        self.floats
            .iter()
            .fold((edges, f32::INFINITY), |(room, low), float| {
                if float[1] < y + height.max(0.01) && float[3] > y {
                    let room = match float[4] == 0. {
                        true => [room[0].max(float[2]), room[1]],
                        false => [room[0], room[1].min(float[0])],
                    };
                    (room, low.min(float[3]))
                } else {
                    (room, low)
                }
            })
    }

    /// The bottom of the floats that `bits` clears.
    fn cleared(&self, bits: u8) -> f32 {
        self.floats.iter().fold(f32::NEG_INFINITY, |bottom, float| {
            let side = if float[4] == 0. {
                Flow::CLEAR_LEFT
            } else {
                Flow::CLEAR_RIGHT
            };
            if bits & side != 0 {
                bottom.max(float[3])
            } else {
                bottom
            }
        })
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
        let banded = window.text_system().floats.borrow().is_some();
        let Slot {
            stretched,
            measure,
            style,
            text,
            measured: kept,
            banded: was,
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
        let perform = inputs.run_mode == RunMode::PerformLayout;
        let remeasured =
            perform && content.is_some_and(|content| mem::replace(kept, content) != content);
        // Lines beside floats can change without a new size, and the place of each line comes from prepaint.
        let rebanded = perform && content.is_some() && (mem::replace(was, banded) || banded);
        if let Some([first, last]) = baselines.filter(|_| style.baseline != Baseline::None) {
            let top = padding.top + border.top;
            output.baselines = Baselines {
                first: Some(first.0 * scale + top),
                last: Some(last.0 * scale + top),
            };
        }
        if rebanded {
            changed(self.slots, index);
        } else if remeasured {
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
    fn column_and_break_styles_intern_apart() {
        let mut styles = super::Interned::<super::Styled>::default();
        let mut style = crate::Style::default();
        let plain = styles.styled(style.clone(), 2.);
        style.breaks = crate::Break::AVOID_INSIDE;
        let avoid = styles.styled(style.clone(), 2.);
        style.columns = Some(Box::new(crate::Columns {
            count: 2,
            gap: px(10.),
            height: None,
            fill: false,
        }));
        let set = styles.styled(style.clone(), 2.);
        style.text.set_lines_kept([3, 2]);
        let kept = styles.styled(style, 2.);
        assert!(!std::rc::Rc::ptr_eq(&plain, &avoid));
        assert!(!std::rc::Rc::ptr_eq(&avoid, &set));
        assert!(!std::rc::Rc::ptr_eq(&set, &kept));
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
