use crate::{
    Bounds, Pixels, Point, Size, Style, Window, geometry::round_half_toward_zero, point, size,
};
use rustc_hash::FxHashMap;
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

pub(crate) struct LayoutEngine {
    taffy: TaffyTree<Box<MeasureFn>>,
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
            bounds: FxHashMap::default(),
            origins: FxHashMap::default(),
            children: Vec::new(),
        }
    }
}

// WARNING: taffy only fails on unknown node ids, which this engine never hands out, so its
// errors are ignored instead of unwrapped (a panic aborts the app).
impl LayoutEngine {
    pub fn clear(&mut self) {
        self.taffy.clear();
        self.bounds.clear();
        self.origins.clear();
    }

    pub fn request_layout(&mut self, style: &Style, scale: f32, children: &[LayoutId]) -> LayoutId {
        let style = style.to_taffy(scale);
        self.children.clear();
        self.children.extend(children.iter().map(|id| id.0));
        LayoutId(
            self.taffy
                .new_with_children(style, &self.children)
                .unwrap_or_else(|_| NodeId::from(u64::MAX)),
        )
    }

    pub fn request_measured_layout(
        &mut self,
        style: &Style,
        scale: f32,
        measure: Box<MeasureFn>,
    ) -> LayoutId {
        LayoutId(
            self.taffy
                .new_leaf_with_context(style.to_taffy(scale), measure)
                .unwrap_or_else(|_| NodeId::from(u64::MAX)),
        )
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
        let mut stack = vec![id.0];
        while let Some(node) = stack.pop() {
            if self.bounds.remove(&LayoutId(node)).is_some() {
                self.origins.remove(&LayoutId(node));
            }
            stack.extend(self.taffy.child_ids(node));
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
        let origin = match self.taffy.parent(id.0) {
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
