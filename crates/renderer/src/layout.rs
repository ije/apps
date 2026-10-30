use crate::{
    Bounds, ElementId, Pixels, Point, Size, Style, Window, geometry::round_half_toward_zero, point,
    size,
};
use rustc_hash::{FxHashMap, FxHasher};
use std::{
    any::Any,
    hash::{Hash, Hasher},
    rc::Rc,
};
use taffy::{TaffyTree, TraversePartialTree as _, tree::NodeId};

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

pub type MeasureFn =
    dyn FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window) -> Size<Pixels>;

/// A node in this frame's layout tree.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
#[repr(transparent)]
pub struct LayoutId(NodeId);

/// The layout tree, kept across frames. A node requested again with the same identity, style,
/// children and measure inputs keeps its taffy node, so taffy's caches skip its subtree.
pub(crate) struct LayoutEngine {
    taffy: TaffyTree<Box<MeasureFn>>,
    /// This frame's and the last frame's nodes by identity and occurrence, with the order they
    /// were made in.
    nodes: [FxHashMap<(u64, u32), (NodeId, u64)>; 2],
    made: u64,
    /// The scale and font generation the kept nodes were measured with.
    measured_with: (f32, usize),
    /// Nodes that no request took again, freed when the next frame begins.
    dead: Vec<(u64, NodeId)>,
    /// How often each identity was requested in this frame.
    occurrences: FxHashMap<u64, u32>,
    /// Identities of the elements with an id around the request, for nodes without one.
    pub(crate) scopes: Vec<u64>,
    /// The inputs and results of measured nodes.
    measured: FxHashMap<NodeId, Rc<dyn Any>>,
    /// The parent of each node in this frame's tree.
    parents: FxHashMap<NodeId, NodeId>,
    bounds: FxHashMap<LayoutId, Bounds<Pixels>>,
    /// Unrounded absolute border-box origins, in device pixels.
    origins: FxHashMap<LayoutId, Point<f32>>,
    children: Vec<NodeId>,
}

impl Default for LayoutEngine {
    fn default() -> Self {
        let mut taffy = TaffyTree::new();
        taffy.disable_rounding();
        Self {
            taffy,
            nodes: Default::default(),
            made: 0,
            measured_with: (0., 0),
            dead: Vec::new(),
            occurrences: FxHashMap::default(),
            scopes: Vec::new(),
            measured: FxHashMap::default(),
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

// WARNING: taffy only fails on unknown node ids, which this engine never hands out, so its
// errors are ignored instead of unwrapped (a panic aborts the app).
impl LayoutEngine {
    /// Frees the nodes the last frame did not request again, or all of them when the scale or
    /// the fonts changed.
    pub fn begin_frame(&mut self, scale: f32, fonts: usize) {
        let [current, previous] = &mut self.nodes;
        self.dead
            .extend(previous.drain().map(|(_, (node, made))| (made, node)));
        std::mem::swap(current, previous);
        if std::mem::replace(&mut self.measured_with, (scale, fonts)) != (scale, fonts) {
            self.dead
                .extend(previous.drain().map(|(_, (node, made))| (made, node)));
        }
        // WARNING: taffy's `remove` clears the parent link of each child, so parents, which are
        // made after their children, go first and no removal follows a stale link.
        self.dead
            .sort_unstable_by_key(|entry| std::cmp::Reverse(entry.0));
        for (_, node) in self.dead.drain(..) {
            let _ = self.taffy.remove(node);
            self.measured.remove(&node);
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
        style: taffy::Style,
        same: impl Fn(Option<&Rc<dyn Any>>) -> bool,
        make: impl FnOnce(&mut TaffyTree<Box<MeasureFn>>, taffy::Style, &[NodeId]) -> NodeId,
    ) -> NodeId {
        let identity = identity(id, self.scopes.last().copied().unwrap_or_default());
        let occurrence = self.occurrences.entry(identity).or_default();
        let key = (identity, *occurrence);
        *occurrence += 1;
        let node = match self.nodes[1].remove(&key) {
            Some((node, made))
                if self.taffy.style(node).is_ok_and(|old| *old == style)
                    && self.taffy.child_ids(node).eq(self.children.iter().copied())
                    && same(self.measured.get(&node)) =>
            {
                self.nodes[0].insert(key, (node, made));
                node
            }
            old => {
                self.dead.extend(old.map(|(node, made)| (made, node)));
                let node = make(&mut self.taffy, style, &self.children);
                self.made += 1;
                self.nodes[0].insert(key, (node, self.made));
                node
            }
        };
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
        LayoutId(self.node(
            id,
            style.to_taffy(scale),
            |measured| measured.is_none(),
            |taffy, style, children| {
                taffy
                    .new_with_children(style, children)
                    .unwrap_or_else(|_| NodeId::from(u64::MAX))
            },
        ))
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
        ) -> Size<Pixels>
        + 'static,
    ) -> (LayoutId, Rc<S>) {
        self.children.clear();
        let state = Rc::new(state);
        let node = self.node(
            None,
            style.to_taffy(scale),
            |old| {
                old.and_then(|old| old.downcast_ref::<S>())
                    .is_some_and(|old| *old == *state)
            },
            |taffy, style, _| {
                let captured = state.clone();
                let measure: Box<MeasureFn> = Box::new(move |known, available, window| {
                    measure(&captured, known, available, window)
                });
                taffy
                    .new_leaf_with_context(style, measure)
                    .unwrap_or_else(|_| NodeId::from(u64::MAX))
            },
        );
        let state = match self.measured.get(&node).cloned().map(Rc::downcast::<S>) {
            Some(Ok(kept)) => kept,
            _ => {
                self.measured.insert(node, state.clone());
                state
            }
        };
        (LayoutId(node), state)
    }

    /// Makes `auto` sizes of the root fill the viewport, like the root element of a page.
    pub fn stretch(&mut self, id: LayoutId, viewport: Size<Pixels>, scale: f32) {
        let Ok(style) = self.taffy.style(id.0) else {
            return;
        };
        let mut style = style.clone();
        if style.size.width.is_auto() {
            style.size.width = taffy::Dimension::length(crate::geometry::round_to_device_pixel(
                viewport.width.0,
                scale,
            ));
        }
        if style.size.height.is_auto() {
            style.size.height = taffy::Dimension::length(crate::geometry::round_to_device_pixel(
                viewport.height.0,
                scale,
            ));
        }
        let _ = self.taffy.set_style(id.0, style);
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
                stack.extend(self.taffy.child_ids(node));
            }
            self.children = stack;
        }
        let scale = window.scale_factor();
        let space = taffy::Size {
            width: space.width.scaled(scale),
            height: space.height.scaled(scale),
        };
        let _ = self
            .taffy
            .compute_layout_with_measure(id.0, space, |input, _, measure, style| {
                taffy::compute_leaf_layout(
                    input,
                    style,
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
                        let measured = measure(known, available, window);
                        // Measured sizes round up to whole device pixels.
                        taffy::Size {
                            width: (measured.width.0.max(0.) * scale).ceil(),
                            height: (measured.height.0.max(0.) * scale).ceil(),
                        }
                    },
                )
            });
    }

    /// The bounds of a node in window coordinates. Edges snap to device pixels in absolute
    /// coordinates, so boxes that touch keep touching wherever they are.
    pub fn layout_bounds(&mut self, id: LayoutId, scale: f32) -> Bounds<Pixels> {
        if let Some(bounds) = self.bounds.get(&id) {
            return *bounds;
        }
        let Ok(layout) = self.taffy.layout(id.0) else {
            return Bounds::default();
        };
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
}
