use crate::{
    Baseline, Bounds, ElementId, Pixels, Point, Size, Style, Window,
    geometry::round_half_toward_zero, point, size,
};
use rustc_hash::{FxHashMap, FxHasher};
use std::{
    any::Any,
    hash::{Hash, Hasher},
    rc::Rc,
};
use taffy::{
    AlignItemsKeyword, Baselines, BlockContext, Cache, CacheTree, Display, FlexDirection, Layout,
    LayoutBlockContainer, LayoutContainingBlock, LayoutFlexboxContainer, LayoutGridContainer,
    LayoutInput, LayoutOutput, LayoutPartialTree, NodeId, Position, RunMode, TraversePartialTree,
};

/// The space a measured node can take along one axis.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum AvailableSpace {
    Definite(Pixels),
    #[default]
    MinContent,
    MaxContent,
}

impl AvailableSpace {
    fn scaled(self, factor: f32) -> taffy::AvailableSpace {
        match self {
            Self::Definite(value) => taffy::AvailableSpace::Definite(value.0 * factor),
            Self::MinContent => taffy::AvailableSpace::MinContent,
            Self::MaxContent => taffy::AvailableSpace::MaxContent,
        }
    }
    fn unscaled(space: taffy::AvailableSpace, factor: f32) -> Self {
        match space {
            taffy::AvailableSpace::Definite(value) => Self::Definite(Pixels(value / factor)),
            taffy::AvailableSpace::MinContent => Self::MinContent,
            taffy::AvailableSpace::MaxContent => Self::MaxContent,
        }
    }
}

impl From<Pixels> for AvailableSpace {
    fn from(value: Pixels) -> Self {
        Self::Definite(value)
    }
}

/// Measures a leaf: its size and its first and last baselines from its top, if it has them.
pub type MeasureFn = dyn FnMut(
    Size<Option<Pixels>>,
    Size<AvailableSpace>,
    &mut Window,
) -> (Size<Pixels>, Option<[Pixels; 2]>);

/// A node in this frame's layout tree.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
#[repr(transparent)]
pub struct LayoutId(NodeId);

struct Node {
    style: taffy::Style,
    baseline: Baseline,
    baseline_source: bool,
    children: Vec<NodeId>,
    cache: Cache,
    /// The unrounded layout, relative to the parent.
    layout: Layout,
    /// The baselines of the last full layout.
    baselines: Baselines,
    measure: Option<Box<MeasureFn>>,
    /// The inputs and results of a measured node.
    state: Option<Rc<dyn Any>>,
}

/// The layout tree, kept across frames. A node requested again with the same identity, style,
/// children and measure inputs keeps its node, so taffy's caches skip its subtree.
pub(crate) struct LayoutEngine {
    tree: Vec<Node>,
    /// Slots of freed nodes.
    free: Vec<usize>,
    /// This frame's and the last frame's nodes by identity and occurrence.
    nodes: [FxHashMap<(u64, u32), NodeId>; 2],
    /// The scale and font generation the kept nodes were measured with.
    measured_with: (f32, usize),
    /// Nodes that no request took again, freed when the next frame begins.
    dead: Vec<NodeId>,
    /// How often each identity was requested in this frame.
    occurrences: FxHashMap<u64, u32>,
    /// Identities of the elements with an id around the request, for nodes without one.
    pub(crate) scopes: Vec<u64>,
    /// The parent of each node in this frame's tree.
    parents: FxHashMap<NodeId, NodeId>,
    bounds: FxHashMap<LayoutId, Bounds<Pixels>>,
    /// Unrounded absolute border-box origins, in device pixels.
    origins: FxHashMap<LayoutId, Point<f32>>,
    children: Vec<NodeId>,
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self {
            tree: Vec::new(),
            free: Vec::new(),
            nodes: Default::default(),
            measured_with: (0., 0),
            dead: Vec::new(),
            occurrences: FxHashMap::default(),
            scopes: Vec::new(),
            parents: FxHashMap::default(),
            bounds: FxHashMap::default(),
            origins: FxHashMap::default(),
            children: Vec::new(),
        }
    }
}

/// The identity of an element with an id, or of a node without one in a scope.
pub(crate) fn identity(id: Option<ElementId>, scope: u64) -> u64 {
    let mut hasher = FxHasher::default();
    (id.map(|id| id.0), scope).hash(&mut hasher);
    hasher.finish()
}

impl LayoutEngine {
    /// Frees the nodes the last frame did not request again, or all of them when the scale or
    /// the fonts changed.
    pub fn begin_frame(&mut self, scale: f32, fonts: usize) {
        let [current, previous] = &mut self.nodes;
        self.dead.extend(previous.drain().map(|(_, node)| node));
        std::mem::swap(current, previous);
        if std::mem::replace(&mut self.measured_with, (scale, fonts)) != (scale, fonts) {
            self.dead.extend(previous.drain().map(|(_, node)| node));
        }
        for node in self.dead.drain(..) {
            let index = usize::from(node);
            let node = &mut self.tree[index];
            node.children.clear();
            node.measure = None;
            node.state = None;
            self.free.push(index);
        }
        self.occurrences.clear();
        self.parents.clear();
        self.bounds.clear();
        self.origins.clear();
    }

    /// Takes last frame's node of the next occurrence of `id` if it is the same, or makes one.
    fn node(
        &mut self,
        id: Option<ElementId>,
        style: &Style,
        scale: f32,
        same: impl Fn(Option<&Rc<dyn Any>>) -> bool,
        measure: impl FnOnce() -> Option<Box<MeasureFn>>,
    ) -> NodeId {
        let identity = identity(id, self.scopes.last().copied().unwrap_or_default());
        let occurrence = self.occurrences.entry(identity).or_default();
        let key = (identity, *occurrence);
        *occurrence += 1;
        let taffy = style.to_taffy(scale);
        let node = match self.nodes[1].remove(&key) {
            Some(node)
                if {
                    let old = &self.tree[usize::from(node)];
                    old.style == taffy
                        && old.baseline == style.baseline
                        && old.baseline_source == style.baseline_source
                        && old.children == self.children
                        && same(old.state.as_ref())
                } =>
            {
                node
            }
            old => {
                self.dead.extend(old);
                let node = Node {
                    style: taffy,
                    baseline: style.baseline,
                    baseline_source: style.baseline_source,
                    children: self.children.clone(),
                    cache: Cache::new(),
                    layout: Layout::new(),
                    baselines: Baselines::NONE,
                    measure: measure(),
                    state: None,
                };
                match self.free.pop() {
                    Some(index) => {
                        self.tree[index] = node;
                        NodeId::from(index)
                    }
                    None => {
                        self.tree.push(node);
                        NodeId::from(self.tree.len() - 1)
                    }
                }
            }
        };
        self.nodes[0].insert(key, node);
        for &child in &self.children {
            self.parents.insert(child, node);
        }
        node
    }

    pub fn request_layout(
        &mut self,
        id: Option<ElementId>,
        style: &Style,
        scale: f32,
        children: impl IntoIterator<Item = LayoutId>,
    ) -> LayoutId {
        self.children.clear();
        self.children.extend(children.into_iter().map(|id| id.0));
        LayoutId(self.node(id, style, scale, |state| state.is_none(), || None))
    }

    /// A leaf sized by `measure`. While `state` compares equal to last frame's, the node and
    /// that state, with what `measure` stored in it, are kept.
    pub fn request_measured_layout<S: PartialEq + 'static>(
        &mut self,
        style: &Style,
        scale: f32,
        state: S,
        mut measure: impl FnMut(
            &S,
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
        ) -> (Size<Pixels>, Option<[Pixels; 2]>)
        + 'static,
    ) -> (LayoutId, Rc<S>) {
        self.children.clear();
        let state = Rc::new(state);
        let id = self.node(
            None,
            style,
            scale,
            |old| {
                old.and_then(|old| old.downcast_ref::<S>())
                    .is_some_and(|old| *old == *state)
            },
            || {
                let captured = state.clone();
                Some(Box::new(move |known, available, window: &mut Window| {
                    measure(&captured, known, available, window)
                }))
            },
        );
        let node = &mut self.tree[usize::from(id)];
        let state = match node.state.clone().map(Rc::downcast::<S>) {
            Some(Ok(kept)) => kept,
            _ => {
                node.state = Some(state.clone());
                state
            }
        };
        (LayoutId(id), state)
    }

    /// Makes `auto` sizes of the root fill the viewport, like the root element of a page.
    pub fn stretch(&mut self, id: LayoutId, viewport: Size<Pixels>, scale: f32) {
        let node = &mut self.tree[usize::from(id.0)];
        let mut style = node.style.clone();
        if style.size.width.is_auto() {
            style.size.width =
                taffy::Dimension::length(crate::geometry::to_layout_unit(viewport.width.0, scale));
        }
        if style.size.height.is_auto() {
            style.size.height =
                taffy::Dimension::length(crate::geometry::to_layout_unit(viewport.height.0, scale));
        }
        if style != node.style {
            node.style = style;
            node.cache.clear();
        }
    }

    pub fn compute_layout(
        &mut self,
        id: LayoutId,
        space: Size<AvailableSpace>,
        window: &mut Window,
    ) {
        // A node laid out again moves its subtree, so its cached bounds go.
        if !self.bounds.is_empty() {
            let mut stack = std::mem::take(&mut self.children);
            stack.clear();
            stack.push(id.0);
            while let Some(node) = stack.pop() {
                if self.bounds.remove(&LayoutId(node)).is_some() {
                    self.origins.remove(&LayoutId(node));
                }
                stack.extend_from_slice(&self.tree[usize::from(node)].children);
            }
            self.children = stack;
        }
        let scale = window.scale_factor();
        let space = taffy::Size {
            width: space.width.scaled(scale),
            height: space.height.scaled(scale),
        };
        taffy::compute_root_layout(
            &mut Tree {
                nodes: &mut self.tree,
                window,
                scale,
            },
            id.0,
            space,
        );
    }

    /// The first and last baselines of a node's border box, from its last layout.
    pub fn baselines(&self, id: LayoutId, scale: f32) -> [Option<Pixels>; 2] {
        let Baselines { first, last } = self.tree[usize::from(id.0)].baselines;
        [first, last].map(|value| value.map(|value| Pixels(value / scale)))
    }

    /// The bounds of a node in window coordinates. Edges snap to device pixels in absolute
    /// coordinates, so boxes that touch keep touching wherever they are.
    pub fn layout_bounds(&mut self, id: LayoutId, scale: f32) -> Bounds<Pixels> {
        if let Some(bounds) = self.bounds.get(&id) {
            return *bounds;
        }
        let layout = self.tree[usize::from(id.0)].layout;
        let (location, layout_size) = (layout.location, layout.size);
        let origin = match self.parents.get(&id.0).copied() {
            Some(parent) => {
                self.layout_bounds(LayoutId(parent), scale);
                let parent = self
                    .origins
                    .get(&LayoutId(parent))
                    .copied()
                    .unwrap_or_default();
                point(parent.x + location.x, parent.y + location.y)
            }
            None => point(location.x, location.y),
        };
        self.origins.insert(id, origin);
        let far = point(origin.x + layout_size.width, origin.y + layout_size.height);
        let bounds = Bounds::from_corners(
            origin.map(round_half_toward_zero),
            far.map(round_half_toward_zero),
        );
        let bounds = (bounds / scale).map(Pixels);
        self.bounds.insert(id, bounds);
        bounds
    }

    /// The bounds of a node in window coordinates before edges snap to device pixels.
    pub fn exact_bounds(&mut self, id: LayoutId, scale: f32) -> Bounds<Pixels> {
        self.layout_bounds(id, scale);
        let Some(origin) = self.origins.get(&id) else {
            return Bounds::default();
        };
        let layout = &self.tree[usize::from(id.0)].layout;
        Bounds::new(
            origin.map(|value| Pixels(value / scale)),
            size(
                Pixels(layout.size.width / scale),
                Pixels(layout.size.height / scale),
            ),
        )
    }
}

/// The nodes and the window while taffy lays them out.
struct Tree<'a> {
    nodes: &'a mut [Node],
    window: &'a mut Window,
    scale: f32,
}

impl Tree<'_> {
    fn compute(
        &mut self,
        node: NodeId,
        inputs: LayoutInput,
        block: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            return taffy::compute_hidden_layout(self, node);
        }
        let output = taffy::compute_cached_layout(self, node, inputs, |tree, node, inputs| {
            let index = usize::from(node);
            let leaf = tree.nodes[index].children.is_empty();
            let mut output = match tree.nodes[index].style.display {
                Display::None => taffy::compute_hidden_layout(tree, node),
                _ if leaf => tree.measure(index, inputs),
                Display::Flex => taffy::compute_flexbox_layout(tree, node, inputs),
                Display::Grid => taffy::compute_grid_layout(tree, node, inputs),
                _ => taffy::compute_block_layout(tree, node, inputs, block),
            };
            if inputs.run_mode == RunMode::PerformLayout {
                taffy::compute_oof_layout(tree, node, &mut output);
                output.baselines = tree.baselines(index, &output);
            }
            output
        });
        if inputs.run_mode == RunMode::PerformLayout {
            self.nodes[usize::from(node)].baselines = output.baselines;
        }
        output
    }

    fn measure(&mut self, index: usize, inputs: LayoutInput) -> LayoutOutput {
        let scale = self.scale;
        let window = &mut *self.window;
        let Node {
            style,
            measure,
            baseline,
            ..
        } = &mut self.nodes[index];
        let mut baselines = None;
        let mut output = taffy::compute_leaf_layout(
            inputs,
            &*style,
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
                let (measured, lines) = measure(known, available, window);
                baselines = lines;
                // Measured sizes round up to 1/64 device pixel, like Chrome's `LayoutUnit`.
                taffy::Size {
                    width: (measured.width.0.max(0.) * scale * 64.).ceil() / 64.,
                    height: (measured.height.0.max(0.) * scale * 64.).ceil() / 64.,
                }
            },
        );
        if let Some([first, last]) = baselines.filter(|_| *baseline != Baseline::None) {
            output.baselines = Baselines {
                first: Some(first.0 * scale),
                last: Some(last.0 * scale),
            };
        }
        output
    }

    /// A box's baselines from its laid out children, like Chrome: a block's are its first and last child's
    /// with one, a line's last is its last item that aligns baselines.
    fn baselines(&self, index: usize, output: &LayoutOutput) -> Baselines {
        let node = &self.nodes[index];
        let style = &node.style;
        let first = output.baselines.first;
        if node.baseline == Baseline::Bottom {
            let bottom = Some(output.size.height);
            return Baselines {
                first: bottom,
                last: bottom,
            };
        }
        if node.children.is_empty() {
            return output.baselines;
        }
        let children = node
            .children
            .iter()
            .map(|&child| &self.nodes[usize::from(child)])
            .filter(|child| {
                child.style.position != Position::Absolute && child.style.display != Display::None
            });
        let at = |child: &Node, value: f32| child.layout.location.y + value;
        // A grid places its items out of flow, so it takes its baseline from the marked item. An item
        // without one gives its bottom border edge.
        let source = node
            .children
            .iter()
            .map(|&child| &self.nodes[usize::from(child)])
            .find(|child| child.baseline_source)
            .map(|child| {
                at(
                    child,
                    child.baselines.first.unwrap_or(child.layout.size.height),
                )
            });
        let row = style.display == Display::Flex
            && matches!(
                style.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            );
        let (first, last) = match node.baseline {
            _ if style.contain.suppresses_baseline() => (None, None),
            _ if source.is_some() => (source, source),
            Baseline::First => (first, first),
            Baseline::FirstOnly => (first, None),
            _ if row => {
                let last = children
                    .rev()
                    .find(|child| {
                        child.style.align_self.unwrap_or(style.align_items).keyword
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
            first: if node.baseline == Baseline::Last {
                last
            } else {
                first
            },
            last,
        }
    }
}

impl TraversePartialTree for Tree<'_> {
    type ChildIter<'a>
        = std::iter::Copied<std::slice::Iter<'a, NodeId>>
    where
        Self: 'a;

    fn child_ids(&self, node: NodeId) -> Self::ChildIter<'_> {
        self.nodes[usize::from(node)].children.iter().copied()
    }

    fn child_count(&self, node: NodeId) -> usize {
        self.nodes[usize::from(node)].children.len()
    }

    fn get_child_id(&self, node: NodeId, index: usize) -> NodeId {
        self.nodes[usize::from(node)].children[index]
    }
}

impl LayoutPartialTree for Tree<'_> {
    type CoreContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type CustomIdent = String;

    fn get_core_container_style(&self, node: NodeId) -> &taffy::Style {
        &self.nodes[usize::from(node)].style
    }

    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.nodes[usize::from(node)].layout = *layout;
    }

    fn compute_child_layout(&mut self, node: NodeId, inputs: LayoutInput) -> LayoutOutput {
        self.compute(node, inputs, None)
    }
}

// Out-of-flow boxes are positioned by their parent, which is always their containing block, and
// layout is never rounded, so the hoisted lists are not kept.
impl LayoutContainingBlock for Tree<'_> {
    type OofItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_oof_item_style(&self, node: NodeId) -> &taffy::Style {
        &self.nodes[usize::from(node)].style
    }

    fn clear_hoisted_children(&mut self, _: NodeId) {}

    fn add_hoisted_children(&mut self, _: NodeId, _: &[NodeId]) {}
}

impl CacheTree for Tree<'_> {
    fn cache_get(&mut self, node: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        self.nodes[usize::from(node)].cache.get(input)
    }

    fn cache_store(&mut self, node: NodeId, input: &LayoutInput, output: LayoutOutput) {
        self.nodes[usize::from(node)].cache.store(input, output);
    }

    fn cache_clear(&mut self, node: NodeId) {
        self.nodes[usize::from(node)].cache.clear();
    }
}

impl LayoutBlockContainer for Tree<'_> {
    type BlockContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type BlockItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_block_container_style(&self, node: NodeId) -> &taffy::Style {
        &self.nodes[usize::from(node)].style
    }

    fn get_block_child_style(&self, node: NodeId) -> &taffy::Style {
        &self.nodes[usize::from(node)].style
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

impl LayoutFlexboxContainer for Tree<'_> {
    type FlexboxContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type FlexboxItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_flexbox_container_style(&self, node: NodeId) -> &taffy::Style {
        &self.nodes[usize::from(node)].style
    }

    fn get_flexbox_child_style(&self, node: NodeId) -> &taffy::Style {
        &self.nodes[usize::from(node)].style
    }
}

impl LayoutGridContainer for Tree<'_> {
    type GridContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type GridItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_grid_container_style(&self, node: NodeId) -> &taffy::Style {
        &self.nodes[usize::from(node)].style
    }

    fn get_grid_child_style(&self, node: NodeId) -> &taffy::Style {
        &self.nodes[usize::from(node)].style
    }
}
