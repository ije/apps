mod columns;
mod flex;
mod flow;
mod intern;
mod layout;
mod paint;
mod prepaint;

use super::{
    PaintKey, PaintSpan, Placement, PrepaintMark,
    node::{
        Child, Content, Events, MeasureFn, Measurement, Node, NodeCx, NodeKey, NodeStyle, Split,
    },
    scroll::Anchor,
};
use crate::{
    AlignContent, AlignItems, AvailableSpace, Baseline, Clip, Display, Escape, Flow, Hit, Hooks,
    Inline, Overflow, Pixels, Point, Position, Rect, Region, Scope, ScrollAnchor, ScrollRef, Size,
    Style, TextStyle, Visibility, Window,
    geometry::round_half_up,
    point, px, size,
    text::{TextState, same_shaping},
};
use intern::Interned;
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
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
    // The node has its own id, like the box of an element. Other boxes are anonymous.
    keyed: bool,
    // The prepaint pass that listed the box in `Tree.anchoring`.
    anchoring: u32,
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
    region: Option<Region>,
    listed: bool,
    free: [bool; 2],
    placed: Option<(u32, Point<f32>, Placement)>,
    below: bool,
    volatile: bool,
    // A volatile box is in it, so a keep of the box prepaints that box.
    lively: bool,
    // The paint pass that left the box out, as it was outside its mask.
    culled: u32,
    // A scroll container is in the box.
    nests: bool,
    // The viewport that the box gave its content in the last prepaint, when it scrolls it in an interest area.
    viewport: Option<Rect<Pixels>>,
    // Scroll viewports between its stacking context and its containing block cut the mask of the box.
    cuts: bool,
    reach: Option<Rect<Pixels>>,
    // The size that the measure function gave in the last layout, which can overflow the box.
    measured: taffy::Size<f32>,
    split: Option<Rc<Split>>,
    // Floats are in the box, or below it in blocks of its formatting context.
    floats: bool,
    // The last layout broke the lines of the box beside floats.
    banded: bool,
    // A layout beside floats emptied the cache, so the caches of the parents can still be full.
    uncached: bool,
    // Whether a layout boundary waits for a layout of its own.
    held: bool,
    // The prepaint pass that kept the box and all boxes in it where they were, and the first pass of the run
    // of such passes.
    calm: u32,
    since: u32,
    // What the last paint of the box drew, which a paint of a calm box copies: an index in `Tree.chunks`.
    chunk: u32,
    // The window regions of the last prepaint of the box, which a calm box copies when `skips`, and the boxes
    // in `touched` of that pass.
    regions: Option<(u32, [u32; 2])>,
    touched: [u32; 2],
    // The regions and touched boxes of the last prepaint that went through the boxes in it. Copies of the
    // regions relocate them as a block.
    base_regions: Option<(u32, [u32; 2])>,
    base_touched: [u32; 2],
    // The regions and touched boxes that the last prepaint of the box gave, which they relocate from with the
    // copies of the regions of its parent.
    own_regions: Option<(u32, [u32; 2])>,
    own_touched: [u32; 2],
    // All hooks in the box allow a skip, and all boxes in it that paint outside it are in `listing`.
    skips: bool,
    // The prepaint pass that skipped the box.
    skipped: u32,
    // The containing blocks of the out-of-flow boxes in it are in it too, so its paint takes no clip from outside.
    contained: bool,
    // The outermost containing block around the box that an out-of-flow box in it reads, as `escape` counts them.
    escaping: Option<u32>,
    // Where the content of a containing block was in the last pass: the offset of the window in it.
    inner: Point<Pixels>,
    // The count of scroll viewports around the containing block of an out-of-flow box, whose clips it escapes.
    cut: u32,
    // How far the scrolls between an out-of-flow box and its containing block moved it.
    unscroll: Point<Pixels>,
    // Where the text of a mark in a line puts it, from its layout.
    shift: Point<f32>,
    // The direction of the text around that mark, when it is mixed.
    turned: Option<bool>,
    // No hit transform is in it, so its regions move with a scroll as they are.
    flat: bool,
    // How far scrolls moved the box without a prepaint of the boxes in it, since one last went through them.
    lag: Point<Pixels>,
    // The part of `lag` that a skip of an ancestor already gave the box, because it is listed outside that ancestor.
    ahead: Point<Pixels>,
    // Fields that few boxes use.
    cold: Option<Box<Cold>>,
}

// The tree keeps a slot for each box, so the size of a slot is the size of the tree.
const _: () = assert!(mem::size_of::<Slot>() <= 1080);

#[derive(Default)]
struct Cold {
    escaped: Option<Clip<Pixels>>,
    // An out-of-flow box that escapes its parent: the room its containing block gave it in the last layout.
    room: Option<[f32; 2]>,
    // The near and far corners that `far` found, from the origin of the box. Cleared up the tree when a box changes.
    far: [Option<[Point<f32>; 2]>; 2],
    // The inputs of the last full layout of the box, which a layout boundary uses to lay out again alone.
    inputs: Option<LayoutInput>,
    // The floats that the box placed in the context of its parent when no other float reached it, from its origin.
    spill: Option<Spill>,
    // Whether the height of a box with `Style::fits` was the bottom of its content in its last layout.
    fits: Option<bool>,
    // The content height of a box with floats, and what it depends on, for one layout.
    natural: Option<(Vec<f32>, f32)>,
    // The boxes inside that the box listed in the stacking context around it, which a skip of the box lists again.
    listing: Box<[u32]>,
    // The content mask of the box where it was listed.
    list_mask: Clip<Pixels>,
}

#[derive(Clone, Copy)]
struct Chunk {
    pass: u32,
    // The prepaint pass that the paint followed.
    prepass: u32,
    key: PaintKey,
    unbounded: bool,
    span: PaintSpan,
    // Where the box was, the mask around it, and the area in device pixels of a span that can move with it.
    at: Point<Pixels>,
    mask: Clip<Pixels>,
    area: Option<Rect<f32>>,
    // A copy can move with the box when its mask moves with it, as in a scroll.
    translates: bool,
    // The last paint of the box that went through the boxes in it. Copies relocate that span as a block, so the
    // chunks of the boxes in it follow the copies.
    base: Output,
    // The last paint or copy of the box, which the chunk relocates from with the copies of its parent.
    origin: Output,
}

// What a paint or a copy of a box drew, and where.
#[derive(Clone, Copy)]
struct Output {
    pass: u32,
    span: PaintSpan,
    key: PaintKey,
    at: Point<Pixels>,
    mask: Clip<Pixels>,
    area: Option<Rect<f32>>,
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
            keyed: false,
            anchoring: 0,
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
            region: None,
            listed: false,
            free: [true; 2],
            placed: None,
            below: false,
            volatile: false,
            lively: false,
            culled: u32::MAX,
            nests: false,
            viewport: None,
            cuts: false,
            reach: None,
            measured: taffy::Size::ZERO,
            split: None,
            floats: false,
            banded: false,
            uncached: false,
            held: false,
            calm: 0,
            since: 0,
            chunk: NONE,
            regions: None,
            touched: [0; 2],
            base_regions: None,
            base_touched: [0; 2],
            own_regions: None,
            own_touched: [0; 2],
            skips: false,
            skipped: 0,
            contained: false,
            escaping: None,
            inner: Point::default(),
            cut: 0,
            unscroll: Point::default(),
            shift: Point::default(),
            turned: None,
            flat: false,
            lag: Point::default(),
            ahead: Point::default(),
            cold: None,
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
        self.anchoring = 0;
        self.live = true;
        self.parent = NONE;
        self.inherited = inherited.clone();
        self.text = inherited.clone();
        self.layout = Layout::new();
        self.baselines = Baselines::NONE;
        if let Some(cold) = &mut self.cold {
            let (listing, list_mask) = (mem::take(&mut cold.listing), cold.list_mask);
            **cold = Cold {
                listing,
                list_mask,
                ..Cold::default()
            };
        }
        (self.unscroll, self.shift, self.turned) = Default::default();
        self.origin = Point::default();
        (self.bounds, self.painted, self.exact) = Default::default();
        self.floats = false;
        self.banded = false;
        self.uncached = false;
        self.held = false;
        self.skipped = 0;
        self.region = None;
        self.listed = false;
        self.free = [true; 2];
        self.placed = None;
        self.below = false;
        self.volatile = false;
        self.lively = false;
        self.culled = u32::MAX;
        self.split = None;
        self.regions = None;
        self.base_regions = None;
        self.own_regions = None;
    }

    fn taffy(&self) -> &taffy::Style {
        self.stretched.as_deref().unwrap_or(&self.style.taffy)
    }

    fn cold_mut(&mut self) -> &mut Cold {
        self.cold.get_or_insert_default()
    }

    fn escaped(&self) -> Option<Clip<Pixels>> {
        self.cold.as_ref().and_then(|cold| cold.escaped)
    }

    fn far(&self) -> [Option<[Point<f32>; 2]>; 2] {
        self.cold.as_ref().map_or([None; 2], |cold| cold.far)
    }

    fn clear_far(&mut self) -> bool {
        match &mut self.cold {
            Some(cold) if cold.far != [None; 2] => {
                cold.far = [None; 2];
                true
            }
            _ => false,
        }
    }

    fn inputs(&self) -> Option<LayoutInput> {
        self.cold.as_ref().and_then(|cold| cold.inputs)
    }

    fn list_mask(&self) -> Clip<Pixels> {
        self.cold
            .as_ref()
            .map_or_else(Clip::default, |cold| cold.list_mask)
    }

    fn listing(&self) -> &[u32] {
        self.cold.as_ref().map_or(&[], |cold| &cold.listing)
    }

    // A float with a fixed size is a layout boundary: what changes in it can't move or size the boxes around it,
    // which take no baseline from it. Absolute boxes are not, since the inline-blocks of a line are absolute boxes.
    fn boundary(&self) -> bool {
        use taffy::CompactLength;
        let style = self.taffy();
        let fixed = |raw: CompactLength| raw.tag() == CompactLength::LENGTH_TAG;
        let bound = |raw: CompactLength| {
            matches!(
                raw.tag(),
                CompactLength::LENGTH_TAG | CompactLength::AUTO_TAG
            )
        };
        self.style.flow & Flow::FLOAT != 0
            && style.position != taffy::Position::Absolute
            && style.display != taffy::Display::None
            && self.split.is_none()
            && fixed(style.size.width.into_raw())
            && fixed(style.size.height.into_raw())
            && [style.min_size.width, style.min_size.height]
                .into_iter()
                .chain([style.max_size.width, style.max_size.height])
                .all(|value| bound(value.into_raw()))
    }
}

#[derive(Default)]
pub(crate) struct Styled {
    style: Style,
    taffy: taffy::Style,
    // The scaled bounds that calc handles of `taffy` point to.
    _bounds: Box<[crate::Bound]>,
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
    // Layout boundaries whose content changed, which lay out again alone when their parents keep their layout.
    bounded: Vec<u32>,
    // Absolute boxes that escape their parent, which layout places again in their containing blocks.
    escaping: FxHashSet<u32>,
    // The boxes at the static places that out-of-flow boxes follow.
    marks: FxHashMap<u64, u32>,
    // Out-of-flow boxes that escape their parent, which took the room of their containing block.
    roomed: FxHashSet<u32>,
    root: Option<u32>,
    measured_with: (f32, usize),
    stretch: Option<Size<Pixels>>,
    layers: FxHashMap<u32, Vec<(i32, u32, Clip<Pixels>)>>,
    kids: Vec<NodeId>,
    initial: Rc<TextStyle>,
    blank: Rc<Styled>,
    styles: Interned<Styled>,
    // Retain styles used as pointer keys to prevent address reuse.
    shared: FxHashMap<*const Style, (Rc<Style>, Rc<Styled>)>,
    texts: Interned<TextStyle>,
    stacks: Vec<Vec<(i32, u32, Clip<Pixels>)>>,
    // The count of scroll viewports around each stacking context in `stacks`, and whether a volatile box of its list
    // prepainted again.
    depths: Vec<(usize, bool)>,
    // The depth in `blocks` of the outermost containing block of the out-of-flow boxes that paint in their
    // parents, like `escapes`.
    foreign: Option<usize>,
    blocks: Vec<Block>,
    // The scroll offsets around the current box, each with the ones outside it.
    scrolled: Vec<Point<Pixels>>,
    painting: Vec<u32>,
    unbounded: u32,
    prepaints: u32,
    full: bool,
    // A paint could not copy a box that a scroll moved without a prepaint, so the next prepaint goes through all.
    unshift: bool,
    // All boxes in the current box are `flat` so far.
    shifting: bool,
    // The next paint can copy spans of the last one.
    copies: bool,
    graveyard: Vec<Remains>,
    // The chunks of `Slot.chunk`, and the free ones, since most boxes have none.
    chunks: Vec<Chunk>,
    spare_chunks: Vec<u32>,
    refined: FxHashMap<usize, Refined>,
    // Floats wait to paint after the blocks of their formatting context, like in CSS.
    deferred: Vec<u32>,
    // The masks of the boxes that paint now. An outline takes the mask of its box in this paint: the mask of
    // prepaint has no effect layers, whose content paints unclipped.
    masks: Vec<Clip<Pixels>>,
    // Whether the boxes that `keep` visits stay where they were, and allow a skip.
    still: bool,
    skips: bool,
    // A scroll container is among the boxes that the pass went through in the current box.
    nests: bool,
    // The outermost containing block that an out-of-flow box in the current box reads: its place in `blocks` plus
    // one, or 0 for the viewport. A box can skip only if no such block is outside it.
    escapes: Option<usize>,
    // For each out-of-flow box that prepaint visits, the scroll viewports inside its containing block, which don't
    // clip the boxes in it.
    hidden: Vec<Vec<Option<Rect<Pixels>>>>,
    // The move of the box that `keep` visits when its layout stayed and only the scroll offset moved it.
    delta: Option<Point<Pixels>>,
    // For each box that `keep` goes through, how far scrolls moved it without a prepaint of the boxes in it.
    lags: Vec<Point<Pixels>>,
    // For each box that a debug paint goes through where a release paint copies, the chunks before the paint.
    #[cfg(debug_assertions)]
    copied: Vec<Vec<(u32, Option<Chunk>)>>,
    // A prepaint since the last paint went through all boxes again for a check.
    #[cfg(debug_assertions)]
    checked: bool,
    // The boxes whose hooks skip also inside a skipped box, in this pass and the last one.
    touched: Vec<u32>,
    last_touched: (u32, Vec<u32>),
    // The paint pass, and the last one whose chunks this pass can copy.
    paints: (u32, Option<u32>),
    // The scroll containers of the last prepaint, which can choose a scroll anchor.
    anchoring: Vec<(u64, u32)>,
}

pub(crate) fn identity(id: Option<NodeKey>, scope: u64) -> u64 {
    let mut hasher = FxHasher::default();
    (id.map(|id| id.0), scope).hash(&mut hasher);
    hasher.finish()
}

impl Tree {
    fn chunk(&self, index: u32) -> Option<Chunk> {
        let at = self.slots[index as usize].chunk;
        (at != NONE).then(|| self.chunks[at as usize])
    }

    fn set_chunk(&mut self, index: u32, chunk: Option<Chunk>) {
        let at = &mut self.slots[index as usize].chunk;
        match (chunk, *at) {
            (Some(chunk), NONE) => {
                *at = match self.spare_chunks.pop() {
                    Some(spare) => {
                        self.chunks[spare as usize] = chunk;
                        spare
                    }
                    None => {
                        // Grow by a quarter, since the list lives as long as the tree.
                        if self.chunks.len() == self.chunks.capacity() {
                            self.chunks.reserve_exact(self.chunks.len() / 4 + 64);
                        }
                        self.chunks.push(chunk);
                        self.chunks.len() as u32 - 1
                    }
                }
            }
            (Some(chunk), old) => self.chunks[old as usize] = chunk,
            (None, NONE) => {}
            (None, old) => {
                self.spare_chunks.push(old);
                *at = NONE;
            }
        }
    }

    pub(crate) fn begin(&mut self, scale: f32, fonts: usize) {
        if mem::replace(&mut self.measured_with, (scale, fonts)) != (scale, fonts) {
            self.slots.clear();
            (self.chunks, self.spare_chunks) = Default::default();
            self.free.clear();
            self.keys.clear();
            self.escaping.clear();
            self.marks.clear();
            self.roomed.clear();
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
        // The boxes around keep their layout caches unless the patch clears them.
        self.dirty(index, true);
        // The layers nodes of the stacking contexts around paint the positioned boxes in the patch.
        let mut at = self.slots[index as usize].parent;
        while at != NONE {
            for kid in 0..self.slots[at as usize].children.len() {
                let child = index_of(self.slots[at as usize].children[kid]) as usize;
                if matches!(self.slots[child].kind, Kind::Layers(_)) {
                    changed(&mut self.slots, child);
                }
            }
            at = self.slots[at as usize].parent;
        }
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
            let slot = &mut self.slots[index as usize];
            slot.style = self.styles.styled(style, scale);
            // The stretched copy holds calc handles into the old style.
            slot.stretched = None;
            changed(&mut self.slots, index as usize);
            self.dirty(index, true);
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
                self.dirty(index, false);
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
        slot.keyed = id.is_some();
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
        // The box itself changed, not only what is in it.
        let mut outer = kept.is_none()
            || (restyled || stretched.is_some())
                && *slot.taffy() != *stretched.as_deref().unwrap_or(&styled.taffy)
            || restyled
                && (slot.style.baseline != styled.baseline
                    || slot.style.baseline_source != styled.baseline_source
                    || slot.style.inline != styled.inline
                    || slot.style.block != styled.block
                    || slot.style.flow != styled.flow
                    || slot.style.outside != styled.outside
                    || slot.style.sizing != styled.sizing
                    || slot.style.follow != styled.follow
                    || slot.style.anchored != styled.anchored);
        let relaid = outer || !same_measure || relisted;
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
        if styled.escape & Escape::PLACED != 0 && styled.placed != 0
            || styled.follow.is_some()
            || styled.anchored.is_some()
        {
            self.escaping.insert(index);
        }
        if let Some(mark) = styled.mark {
            self.marks.insert(mark.key, index);
        }
        // A slot can become a mark outside a line, or the mark of another box.
        if slot.style.mark != styled.mark {
            (slot.shift, slot.turned) = Default::default();
        }
        // Whether scroll containers take the absolute boxes in this box reads these styles.
        let scoped = kept.is_some()
            && (slot.style.contains != styled.contains || slot.style.overflow != styled.overflow);
        slot.style = styled;
        slot.stretched = stretched;
        slot.volatile = hooks.as_ref().is_some_and(|hooks| hooks.volatile());
        slot.placed = None;
        unfar(&mut self.slots, index as usize);
        if scoped {
            self.slots.iter_mut().for_each(|slot| {
                slot.clear_far();
            });
        }
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
            child.placed.is_none() || child.below
        });
        let lively = self.slots[index as usize].children.iter().any(|&child| {
            let child = &self.slots[index_of(child) as usize];
            child.volatile || child.lively
        });
        let floats = self.slots[index as usize].children.iter().any(|&child| {
            let child = &self.slots[index_of(child) as usize];
            child.style.flow & Flow::FLOAT != 0
                || child.floats && child.style.flow & Flow::BLOCK != 0
        });
        let slot = &mut self.slots[index as usize];
        (slot.free, slot.below, slot.lively) = (free, below, lively);
        if mem::replace(&mut slot.floats, floats) != floats {
            outer = true;
        }
        if relaid || outer {
            self.dirty(index, outer);
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

    /// Clears the layout caches of the box and the boxes around it, up to a layout boundary. `outer`: the box itself
    /// changed, so a boundary there can't hold the change.
    fn dirty(&mut self, mut index: u32, mut outer: bool) {
        while index != NONE {
            let slot = &mut self.slots[index as usize];
            let parent = slot.parent;
            if !outer && slot.inputs().is_some() && slot.boundary() {
                slot.cache.clear();
                if !mem::replace(&mut slot.held, true) {
                    self.bounded.push(index);
                }
                break;
            }
            outer = false;
            // An empty cache means its parents were cleared too, unless it was just made or laid out beside floats.
            if matches!(slot.cache.clear(), ClearState::AlreadyEmpty)
                && parent != NONE
                && !self.slots[parent as usize].uncached
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
            self.set_chunk(index, None);
            self.free.push(index);
        }
        self.orphans = pending;
        if self.graveyard.len() > 1 << 15 {
            self.bury();
        }
    }

    pub(crate) fn bury(&mut self) {
        self.graveyard.clear();
        self.graveyard.shrink_to(1024);
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
        let mut chunks = Vec::with_capacity(self.chunks.len() - self.spare_chunks.len());
        for slot in &mut slots {
            if slot.chunk != NONE {
                chunks.push(self.chunks[slot.chunk as usize]);
                slot.chunk = chunks.len() as u32 - 1;
            }
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
        for set in [&mut self.escaping, &mut self.roomed] {
            *set = (set.iter())
                .map(|&index| moved[index as usize])
                .filter(|&index| index != NONE)
                .collect();
        }
        self.marks.retain(|_, index| {
            *index = moved[*index as usize];
            *index != NONE
        });
        self.slots = slots;
        (self.chunks, self.spare_chunks) = (chunks, Vec::new());
        self.free.clear();
        self.layers.clear();
    }
}

const FORCED: u8 = 1;
const AVOID: u8 = 2;
const END: u8 = 3;
// The space below the content of a box, where a break can come anywhere: its chance holds where it starts and ends.
const SLICE: u8 = 4;

// The collapsible space at the end of `child` that hangs when it ends a line of inline boxes: the one that its last
// box joins, or the child itself.
fn hanging(slots: &[Slot], mut child: NodeId) -> Option<NodeId> {
    loop {
        let slot = &slots[usize::from(child)];
        match slot.style.inline {
            Inline::Hangs => return Some(slot.children.last().copied().unwrap_or(child)),
            Inline::Lines if slot.taffy().flex_wrap == taffy::FlexWrap::NoWrap => {
                child = *slot.children.iter().rev().find(|&&kid| {
                    let kid = slots[usize::from(kid)].taffy();
                    kid.position != taffy::Position::Absolute && kid.display != taffy::Display::None
                })?;
            }
            _ => return None,
        }
    }
}

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
                    || style.inline == Inline::Lines && taffy.flex_wrap != taffy::FlexWrap::NoWrap)
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
        let (content, bottom) = (
            start + height,
            top + size.height - border.bottom - padding.bottom,
        );
        if bottom > content + 0.01 && !avoid {
            out.push((content, bottom, SLICE));
        }
        return content.min(bottom);
    }
    let (mut last, mut used): (Option<(f32, u8)>, f32) = (None, top);
    // The breaks between the rows of a line box, which keep `orphans` and `widows` like rows of text.
    let mut rows: Vec<usize> = Vec::new();
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
            if slot.style.inline == Inline::Lines {
                rows.push(out.len());
            }
            out.push((cut, bottom, kind));
        }
        if splits(slots, child) {
            chances_in(slots, child, at, avoid, scale, out);
        }
        let bottom = at + other.layout.size.height;
        last = Some((last.map_or(bottom, |(end, _)| end.max(bottom)), breaks));
        used = used.max(bottom + other.layout.margin.bottom.max(0.));
    }
    let count = rows.len() + 1;
    let [orphans, widows] = slot.text.lines_kept.map(usize::from);
    let widows = if count < orphans + widows { 0 } else { widows };
    for (row, &at) in rows.iter().enumerate() {
        if (row + 1 < orphans || count - row - 1 < widows) && out[at].2 == 0 {
            out[at].2 = AVOID;
        }
    }
    // Like Chrome, a box taller than its content breaks anywhere below it, but not in its padding and border.
    let content = last.map_or(top, |(bottom, _)| bottom);
    let Layout {
        size,
        border,
        padding,
        ..
    } = slot.layout;
    let bottom = top + size.height - border.bottom - padding.bottom;
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

// The room from the padding box of `parent` to the far padding edges of the block that contains its out-of-flow
// boxes of `escape`, by the places of the last layout, or `None` when `parent` contains them.
fn room(slots: &[Slot], parent: u32, escape: u8, viewport: [f32; 2]) -> Option<[f32; 2]> {
    if slots[parent as usize].style.contains & escape != 0 {
        return None;
    }
    let mut at = [0f32; 2];
    let mut block = parent;
    loop {
        let outer = &slots[block as usize];
        if outer.style.contains & escape != 0 || outer.parent == NONE {
            break;
        }
        at = [
            at[0] + outer.layout.location.x,
            at[1] + outer.layout.location.y,
        ];
        block = outer.parent;
    }
    let outer = &slots[block as usize];
    let end = match outer.style.contains & escape != 0 {
        true => [
            outer.layout.size.width - outer.layout.border.right,
            outer.layout.size.height - outer.layout.border.bottom,
        ],
        false => viewport,
    };
    let border = slots[parent as usize].layout.border;
    Some([end[0] - at[0] - border.left, end[1] - at[1] - border.top])
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

// A containing block: what it contains, its mask, the count of scroll viewports around it, the scroll offsets
// around its content, and the move of its content in this pass, when the pass kept the block.
type Block = (u8, Clip<Pixels>, u32, Point<Pixels>, Option<Point<Pixels>>);

fn unfar(slots: &mut [Slot], mut index: usize) {
    while slots[index].clear_far() {
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

fn scroll_of(slot: &Slot) -> Option<ScrollRef> {
    slot.events.as_ref()?.scroll.clone()
}

fn index_of(node: NodeId) -> u32 {
    usize::from(node) as u32
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
        Box::new(move |_, known, available, window| {
            kept.measure(
                known,
                available,
                window.text_system(),
                window.scale_factor(),
            )
        }),
    )
}

struct LayoutTree<'a> {
    slots: &'a mut [Slot],
    window: &'a mut Window,
    scale: f32,
    exact: bool,
    // The margin boxes `[left, top, right, bottom, right side]` of the floats in the formatting context, from the
    // border box of its root. A float with a shape adds twice its place in `slots`, from 1, to its side.
    floats: Vec<[f32; 5]>,
    // The lowest top and bottom of all floats of the context, also the ones that ended above the current box.
    ceiling: f32,
    reach: f32,
    // Where the border box of the box that `stack` lays out next is in the formatting context.
    origin: [f32; 2],
    // The next box that `compute` lays out is beside floats: it skips the cache, and `stack` places it in the
    // formatting context of its parent.
    active: bool,
    // The next box that `compute` lays out takes its content sizes, not those of its `Sizing`.
    natural: bool,
    // The boxes that keep their natural height for this layout.
    naturals: Vec<u32>,
    joined: bool,
    // Line boxes that keep their line in this Taffy pass: the box, its overflow and its style with that much less
    // right margin.
    hung: Vec<(NodeId, f32, taffy::Style)>,
    // The size of the viewport, the initial containing block.
    viewport: [f32; 2],
    // The boxes that took the room of their containing block in this layout.
    roomed: Vec<u32>,
}

// The border box size of a layout in CSS pixels, before it snaps to device pixels.
fn unsnapped(layout: &Layout, scale: f32) -> Size<Pixels> {
    Size::new(
        px(layout.size.width / scale),
        px(layout.size.height / scale),
    )
}

// A height, min-height or max-height that depends on the height of the parent.
fn relative_height(style: &taffy::Style) -> bool {
    [
        style.size.height.into_raw(),
        style.min_size.height.into_raw(),
        style.max_size.height.into_raw(),
    ]
    .iter()
    .any(|size| {
        !matches!(
            size.tag(),
            taffy::CompactLength::AUTO_TAG | taffy::CompactLength::LENGTH_TAG
        )
    })
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

// A move by `by` from the mask `was` to `now`.
type Mv = (Point<Pixels>, Clip<Pixels>, Clip<Pixels>);
// A scroll container in a box that shifts: its regions, its viewport and its own region.
type Inner = ([u32; 2], Rect<Pixels>, Option<Region>);
// What a box keeps of the pass around it while the pass goes through it: `skips` and `nests`, `escapes`, the
// containing block of the box itself, and `foreign`.
type Outer = ([bool; 2], Option<usize>, Option<usize>, Option<usize>);

// A mask that moved by `by`, as a moved copy moves it: the mask `old` around the copy becomes `now`.
fn moved_mask(
    mask: Clip<Pixels>,
    by: Point<Pixels>,
    old: Clip<Pixels>,
    now: Clip<Pixels>,
) -> Clip<Pixels> {
    if mask == old {
        return now;
    }
    let mut moved = mask;
    if mask.bounds.size.width.0 < crate::gpu::scene::OPEN {
        moved.bounds.origin += by;
    }
    if (old.rounded, old.radii) == (mask.rounded, mask.radii) {
        (moved.rounded, moved.radii) = (now.rounded, now.radii);
    } else if !mask.radii.is_zero() {
        moved.rounded.origin += by;
    }
    moved
}

// Records of a paint and of a copy match, but for the rounding of moved places and of a transform that turns about a
// moved point.
#[cfg(debug_assertions)]
fn same_records(now: &[String], last: &[String]) -> bool {
    let split = |text: &str| {
        let (mut words, mut numbers) = (String::new(), Vec::new());
        for part in
            text.split(|char: char| !(char.is_ascii_digit() || matches!(char, '.' | '-' | 'e')))
        {
            match part.parse::<f64>() {
                Ok(number) => numbers.push(number),
                Err(_) => words.push_str(part),
            }
            words.push('|');
        }
        (words, numbers)
    };
    now.len() == last.len()
        && now.iter().zip(last).all(|(now, last)| {
            let ((a, x), (b, y)) = (split(now), split(last));
            now == last
                || a == b
                    && x.len() == y.len()
                    && x.iter()
                        .zip(&y)
                        // A copy moved over many frames adds up the rounding of f32 places, far below a pixel.
                        .all(|(x, y)| (x - y).abs() <= 1e-2 + 1e-6 * x.abs())
        })
}

// Like Chrome, a press outside the rounded corners misses the box.
fn place_region(window: &mut Window, style: &Style, first: Rect, old: Option<&Region>) -> Region {
    let region = match old {
        Some(region) => window.move_region(region, first),
        None => window.add_region(first, Hit::Pass),
    };
    let corners = style.corner_radii.resolve(first.size);
    if !corners.is_zero() {
        window.round_region(corners);
    }
    region
}

impl LayoutTree<'_> {
    fn forget(&mut self) {
        for index in mem::take(&mut self.naturals) {
            if let Some(cold) = &mut self.slots[index as usize].cold {
                cold.natural = None;
            }
        }
    }

    /// Lays out a box with sizes that are functions of its content sizes: first without them, then with the sizes.
    fn sized(
        &mut self,
        node: NodeId,
        mut inputs: LayoutInput,
        block: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        use taffy::{MaybeMath, MaybeResolve};
        let index = usize::from(node);
        let sizing = self.slots[index].style.sizing.clone().unwrap();
        let (active, scale, ratio) = (self.active, self.scale, sizing.ratio);
        // Boxes with floats measure to another height than they lay out, and floats reach the boxes after them.
        let floating = active || self.slots[index].floats;
        let outer = active.then(|| (self.floats.clone(), self.origin, self.ceiling, self.reach));
        let calc = |value, basis| crate::style::resolve_calc(value, basis);
        let style = self.slots[index].taffy();
        let [min, max] = [style.min_size, style.max_size]
            .map(|size| size.maybe_resolve(inputs.parent_size, calc));
        // The content sizes of a box are without its own limits.
        let natural = |tree: &mut Self, inputs: LayoutInput| {
            let vertical = inputs.axis != taffy::RequestedAxis::Horizontal;
            // Nested boxes with floats would lay out twice at each level.
            let mut key = Vec::new();
            if floating && vertical {
                let space = match inputs.available_space.width {
                    taffy::AvailableSpace::Definite(width) => width,
                    taffy::AvailableSpace::MinContent => -1.,
                    taffy::AvailableSpace::MaxContent => -2.,
                };
                key.extend([
                    inputs.known_dimensions.width.unwrap_or(-1.),
                    inputs.parent_size.width.unwrap_or(-1.),
                    space,
                ]);
                if let Some((floats, [x, y], ceiling, reach)) = &outer {
                    key.extend([(ceiling - y).max(0.), (reach - y).max(0.)]);
                    for f in floats.iter().filter(|float| float[3] > *y) {
                        key.extend([f[0] - x, f[1] - y, f[2] - x, f[3] - y, f[4]]);
                    }
                }
                if let Some(found) =
                    (tree.slots[index].cold.as_ref()).and_then(|cold| cold.natural.as_ref())
                    && found.0 == key
                {
                    return found.1;
                }
            }
            tree.natural = true;
            tree.active = active;
            let output = tree.compute(
                node,
                LayoutInput {
                    sizing_mode: taffy::SizingMode::ContentSize,
                    run_mode: match floating && vertical {
                        true => RunMode::PerformLayout,
                        false => RunMode::ComputeSize,
                    },
                    ..inputs
                },
                None,
            );
            if let Some((floats, origin, ceiling, reach)) = &outer {
                tree.floats.clone_from(floats);
                (tree.origin, tree.ceiling, tree.reach) = (*origin, *ceiling, *reach);
            }
            if !vertical {
                return output.size.width;
            }
            let height = output
                .size
                .height
                .max(ratio.map_or(0., |ratio| output.size.width / ratio));
            if floating {
                tree.naturals.push(index as u32);
                tree.slots[index].cold_mut().natural = Some((key, height));
            }
            height
        };
        let mut parent_size = inputs.parent_size;
        parent_size.height = None;
        if let Some((content, size)) = &sizing.width
            && inputs.known_dimensions.width.is_none()
        {
            let mut available = inputs.available_space;
            match content {
                crate::ContentWidth::Min => available.width = taffy::AvailableSpace::MinContent,
                crate::ContentWidth::Max => available.width = taffy::AvailableSpace::MaxContent,
                crate::ContentWidth::Fit => {}
            }
            let found = natural(
                self,
                LayoutInput {
                    axis: taffy::RequestedAxis::Horizontal,
                    available_space: available,
                    run_mode: RunMode::ComputeSize,
                    ..inputs
                },
            );
            let width = (size(found / scale) * scale).maybe_clamp(min.width, max.width);
            inputs.known_dimensions.width = Some(width);
            inputs.known_dimensions_are_definite.width = true;
        }
        if let Some(size) = &sizing.height
            && inputs.known_dimensions.height.is_none()
            && (inputs.run_mode == RunMode::PerformLayout
                || inputs.axis != taffy::RequestedAxis::Horizontal)
        {
            let found = natural(
                self,
                LayoutInput {
                    axis: taffy::RequestedAxis::Both,
                    parent_size,
                    available_space: taffy::Size {
                        height: taffy::AvailableSpace::MaxContent,
                        ..inputs.available_space
                    },
                    ..inputs
                },
            );
            // Like Chrome, in 64ths of a pixel, so the rounding errors of floats don't add up.
            let height = (size(found / scale) * 64.).round() / 64. * scale;
            let height = height.maybe_clamp(min.height, max.height);
            inputs.known_dimensions.height = Some(height);
            inputs.known_dimensions_are_definite.height = sizing.definite;
        }
        if let (false, RunMode::ComputeSize, Some(width), Some(height)) = (
            active,
            inputs.run_mode,
            inputs.known_dimensions.width,
            inputs.known_dimensions.height,
        ) {
            return LayoutOutput::from_outer_size(taffy::Size { width, height });
        }
        (self.natural, self.active) = (true, active);
        self.compute(node, inputs, block)
    }

    fn compute(
        &mut self,
        node: NodeId,
        mut inputs: LayoutInput,
        block: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        if self.slots[usize::from(node)].style.sizing.is_some()
            && !mem::take(&mut self.natural)
            && inputs.run_mode != RunMode::PerformHiddenLayout
        {
            return self.sized(node, inputs, block);
        }
        let joined = mem::take(&mut self.active);
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            return taffy::compute_hidden_layout(self, node);
        }
        if inputs.run_mode == RunMode::PerformLayout {
            let slot = &mut self.slots[usize::from(node)];
            slot.held = false;
            match slot.boundary() {
                true => slot.cold_mut().inputs = Some(inputs),
                false => {
                    if let Some(cold) = &mut slot.cold {
                        cold.inputs = None;
                    }
                }
            }
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
        // Taffy lays out a block as a flex column, whose items get definite heights. In a block, an auto height is
        // not definite, unless a ratio gives it, so percent heights in the child stay auto.
        if inputs.known_dimensions.height.is_some()
            && style.size.height.is_auto()
            && style.aspect_ratio.is_none()
            && style.position != taffy::Position::Absolute
            && slot.parent != NONE
            && self.slots[slot.parent as usize].style.block
            && !(slot.style.sizing.as_ref()).is_some_and(|sizing| sizing.definite)
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
                // Taffy reads `direction` in all layout, which handles right-to-left itself, so a right-to-left
                // box gives it only to the pass of its absolute boxes.
                let index = usize::from(node);
                // Like Chrome, a box that escapes its parent takes the room of its containing block, so its auto
                // size doesn't shrink to the parent.
                if tree.slots[index].style.flow & Flow::RTL_BLOCK == 0
                    && let Some(area) = output.oof_positioning_area
                    && output.oof_candidates.as_slice().iter().any(|candidate| {
                        let escape = tree.slots[usize::from(candidate.node)].style.escape
                            & (Escape::ABSOLUTE | Escape::FIXED);
                        escape != 0 && tree.slots[index].style.contains & escape == 0
                    })
                {
                    let mut claims =
                        taffy::CoreStyle::is_containing_block(tree.slots[index].taffy());
                    (claims.absolute, claims.fixed) = (true, true);
                    let mut rest = taffy::OofCandidates::new();
                    for &candidate in output.oof_candidates.take().as_slice() {
                        let child = usize::from(candidate.node);
                        let escape =
                            tree.slots[child].style.escape & (Escape::ABSOLUTE | Escape::FIXED);
                        let Some(room) = (escape != 0)
                            .then(|| room(tree.slots, index as u32, escape, tree.viewport))
                            .flatten()
                        else {
                            rest.push(candidate);
                            continue;
                        };
                        tree.slots[child].cold_mut().room = Some(room);
                        tree.roomed.push(child as u32);
                        let area = taffy::OofPositioningArea {
                            size: taffy::Size {
                                width: area.size.width.max(room[0]),
                                height: area.size.height.max(room[1]),
                            },
                            ..area
                        };
                        taffy::compute_oof_layout_for_area(
                            tree,
                            node,
                            &[candidate],
                            area,
                            taffy::Direction::Ltr,
                            claims,
                        );
                    }
                    output.oof_candidates = rest;
                }
                if tree.slots[index].style.flow & Flow::RTL_BLOCK != 0 {
                    if let Some(area) = output.oof_positioning_area
                        && !output.oof_candidates.is_empty()
                    {
                        let claims =
                            taffy::CoreStyle::is_containing_block(&tree.slots[index].taffy());
                        let candidates = output.oof_candidates.take();
                        let result = taffy::compute_oof_layout_for_area(
                            tree,
                            node,
                            candidates.as_slice(),
                            area,
                            taffy::Direction::Rtl,
                            claims,
                        );
                        output.oof_candidates = result.unclaimed;
                        // Warning: with Taffy's `content_size` feature, merge `result.scrollable_overflow_rect`
                        // into the output here, as `compute_oof_layout` does.
                    }
                } else {
                    taffy::compute_oof_layout(tree, node, &mut output);
                }
                output.baselines = tree.baselines(usize::from(node), &output);
                if tree.slots[index].style.fits {
                    let calc = |value, basis| crate::style::resolve_calc(value, basis);
                    let bottom = (tree.slots[index].children.iter())
                        .map(|&child| &tree.slots[usize::from(child)])
                        .filter(|slot| {
                            let style = slot.taffy();
                            style.display != taffy::Display::None
                                && style.position != taffy::Position::Absolute
                                && slot.style.flow & Flow::FLOAT == 0
                        })
                        .map(|slot| {
                            let (style, layout) = (slot.taffy(), &slot.layout);
                            let shift = match style.position {
                                taffy::Position::Relative => {
                                    let [top, bottom] =
                                        [style.inset.top, style.inset.bottom].map(|side| {
                                            taffy::MaybeResolve::maybe_resolve(side, None, calc)
                                        });
                                    top.or(bottom.map(|bottom| -bottom)).unwrap_or(0.)
                                }
                                _ => 0.,
                            };
                            layout.location.y - shift + layout.size.height + layout.margin.bottom
                        })
                        .fold(0f32, f32::max);
                    tree.slots[index].cold_mut().fits =
                        Some((output.size.height - bottom).abs() < 0.01);
                }
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
                Some(output)
                    if (self.slots[index].cold.as_ref())
                        .is_some_and(|cold| cold.spill.is_some()) =>
                {
                    let spill = self.slots[index]
                        .cold
                        .as_ref()
                        .and_then(|cold| cold.spill.as_ref());
                    let (floats, ceiling, reach) = spill.unwrap();
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
                    self.slots[index].cold_mut().spill =
                        Some((floats, self.ceiling - y, self.reach - y));
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
            self.slots[index].uncached = true;
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
        let mut layout = *layout;
        // Taffy gives negative free space to cross-axis auto margins. In CSS the start margin is then 0, and the
        // end margin takes the rest.
        let margin = layout.margin;
        if margin.left.min(margin.right).min(margin.top) < 0. {
            let slot = &self.slots[index];
            let style = slot.taffy();
            if style.position != taffy::Position::Absolute {
                if margin.top < 0. && style.margin.top.is_auto() {
                    layout.location.y -= margin.top;
                    layout.margin.bottom += margin.top;
                    layout.margin.top = 0.;
                }
                let rtl = slot.style.flow & Flow::RTL != 0;
                if !rtl && margin.left < 0. && style.margin.left.is_auto() {
                    layout.location.x -= margin.left;
                    layout.margin.right += margin.left;
                    layout.margin.left = 0.;
                } else if rtl && margin.right < 0. && style.margin.right.is_auto() {
                    layout.location.x += margin.right;
                    layout.margin.left += margin.right;
                    layout.margin.right = 0.;
                }
            }
        }
        let old = mem::replace(&mut self.slots[index].layout, layout);
        // Floats outside the parent can move a float or a box beside floats, so prepaint must visit it also when
        // its parent stays.
        let parent = self.slots[index].parent;
        let beside = self.slots[index].style.flow & Flow::FLOAT != 0
            || parent != NONE && self.slots[parent as usize].uncached;
        if !self.exact
            && (Layout {
                location: if beside {
                    layout.location
                } else {
                    old.location
                },
                ..layout
            }) != old
        {
            changed(self.slots, index);
        }
    }

    fn compute_child_layout(&mut self, node: NodeId, inputs: LayoutInput) -> LayoutOutput {
        let mut output = self.compute(node, inputs, None);
        let slot = &self.slots[usize::from(node)];
        if slot.style.align_self == Some(AlignItems::LastBaseline) && slot.parent != NONE {
            let parent = self.slots[slot.parent as usize].taffy();
            if parent.display == taffy::Display::Flex
                && matches!(
                    parent.flex_direction,
                    FlexDirection::Row | FlexDirection::RowReverse
                )
            {
                output.baselines.first = output.baselines.last;
            }
        }
        output
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

impl CacheTree for LayoutTree<'_> {
    fn cache_get(&mut self, node: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        let slot = &mut self.slots[usize::from(node)];
        let relative = self.exact || slot.style.relative;
        slot.cache.get(&cache_input(slot.free, relative, input))
    }

    fn cache_store(&mut self, node: NodeId, input: &LayoutInput, output: LayoutOutput) {
        let slot = &mut self.slots[usize::from(node)];
        let relative = self.exact || slot.style.relative;
        slot.uncached = false;
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
        match self.hung.iter().rfind(|(hung, ..)| *hung == node) {
            Some((_, _, style)) => style,
            None => self.slots[usize::from(node)].taffy(),
        }
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
mod tests;
