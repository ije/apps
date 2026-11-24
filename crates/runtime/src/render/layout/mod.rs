pub(crate) mod cull;
pub(crate) mod grid;
pub(crate) mod line;
pub(crate) mod measure;
pub(crate) mod size;

use crate::css;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Row {
    pub shift: renderer::Pixels,
    pub right: renderer::Pixels,
    pub gap: renderer::Pixels,
    pub start: usize,
    pub drop: renderer::Pixels,
    pub above: renderer::Pixels,
}

pub(crate) type Rows = Rc<[Row]>;

pub(crate) type TextLayout = (
    renderer::Paragraph,
    Rc<str>,
    std::ops::Range<usize>,
    Rows,
    [renderer::Pixels; 2],
);

pub(crate) struct LayoutNode {
    pub shape: u64,
    pub attrs: u64,
    pub seen: u64,
    pub style: Rc<css::properties::Style>,
    pub pseudos: Option<Rc<[css::properties::Style; 2]>>,
    pub units: u64,
    pub list: u64,
    pub kids: u64,
    pub(crate) out: u64,
    pub revision: u64,
    pub geometry: u64,
    // The last revision that changed the box itself, not only the boxes in it.
    pub shell: u64,
    pub flags: u32,
    pub marks: u32,
}

#[derive(Default)]
pub struct Layout {
    pub(crate) text_layouts: rustc_hash::FxHashMap<usize, TextLayout>,
    pub(in crate::render) grids: rustc_hash::FxHashMap<
        usize,
        (
            f32,
            Option<f32>,
            Option<crate::render::layout::grid::Grid>,
            crate::render::layout::grid::Grid,
        ),
    >,
    pub(in crate::render) flexed_grids: rustc_hash::FxHashMap<usize, [Option<(f32, f32)>; 2]>,
    pub(in crate::render) grid_widest: rustc_hash::FxHashMap<usize, f32>,
    // The used columns and rows of the last layout of each grid, for `getComputedStyle()`.
    pub(in crate::render) tracks: rustc_hash::FxHashMap<usize, [Box<[f32]>; 2]>,
    // The grid area of each grid item at its last build, for `getComputedStyle()`.
    pub(in crate::render) areas: rustc_hash::FxHashMap<usize, [f32; 2]>,
    pub(in crate::render) baselines: grid::Baselines,
    pub(in crate::render) layout_nodes: Vec<Option<LayoutNode>>,
    pub(in crate::render) measurements: measure::Measurements,
    pub painted: Vec<u64>,
    // A hash of the state that paint reads outside the boxes: images and the selection.
    pub(in crate::render) paint_inputs: u64,
    pub(in crate::render) layout_revision: u64,
    pub(in crate::render) containers: rustc_hash::FxHashMap<usize, [f32; 2]>,
    pub(in crate::render) atoms: rustc_hash::FxHashMap<usize, [f32; 3]>,
    // The rendered `::before` and `::after` boxes that scripts can read, by their atom key: the place of the border
    // box from the border box of the element in layout, its size, then the padding plus border of each side.
    pub pseudo_boxes: rustc_hash::FxHashMap<usize, [f32; 8]>,
    pub(in crate::render) container_pass: bool,
    pub(in crate::render) anchors: rustc_hash::FxHashMap<usize, renderer::Rect<renderer::Pixels>>,
    pub(in crate::render) blocks: rustc_hash::FxHashMap<usize, renderer::Rect<renderer::Pixels>>,
    pub(crate) cull: cull::Cull,
    // The `content-visibility: auto` boxes, each with the boxes on its path, the boxes that clip it, and the view
    // and its top and bottom at its last change.
    #[allow(clippy::type_complexity)]
    pub(in crate::render) autos:
        rustc_hash::FxHashMap<usize, (Rc<[usize]>, Rc<[usize]>, (u64, [f32; 2]))>,
    // `contentvisibilityautostatechange` events: the box and whether it skips its contents now.
    pub(in crate::render) skip_events: Vec<(usize, bool)>,
}

impl Layout {
    // What scripts read of the last layout.

    pub(crate) fn has_containers(&self) -> bool {
        !self.containers.is_empty()
    }

    pub(crate) fn container_size(&self, id: usize) -> Option<&[f32; 2]> {
        self.containers.get(&id)
    }

    // The content box size of a box, as its last layout gave it.
    pub(crate) fn content_size(&self, id: usize) -> Option<[f32; 2]> {
        self.cull.widths.get(&id).copied()
    }

    // The used columns and rows of a grid.
    pub(crate) fn tracks(&self, grid: usize) -> Option<&[Box<[f32]>; 2]> {
        self.tracks.get(&grid)
    }

    // The size of the grid area of `item` when `grid` laid it out.
    pub(crate) fn grid_area(&self, grid: usize, item: usize) -> Option<[f32; 2]> {
        self.tracks
            .contains_key(&grid)
            .then(|| self.areas.get(&item).copied())?
    }

    // The `::before` or `::after` box of `id`.
    pub(crate) fn pseudo_box(&self, id: usize, after: bool) -> Option<[f32; 8]> {
        let key = crate::render::inline::atom_key(id, if after { 2 } else { 1 });
        self.pseudo_boxes.get(&key).copied()
    }

    pub(crate) fn text_layout(&self, id: usize) -> Option<&TextLayout> {
        self.text_layouts.get(&id)
    }

    // `contentvisibilityautostatechange` events since the last call.
    pub(crate) fn take_skip_events(&mut self) -> Vec<(usize, bool)> {
        std::mem::take(&mut self.skip_events)
    }

    // Whether the last frame culled `id` as hidden, and whether `id` is a `content-visibility: auto` box.
    pub(crate) fn culled(&self, id: usize) -> bool {
        self.cull.visible.get(id) == Some(&false)
    }

    pub(crate) fn auto_box(&self, id: usize) -> bool {
        self.measurements.shown.contains_key(&id)
    }

    // A script reads the geometry of `id`: the boxes that skip it show their contents until the next frame. False
    // when it already does.
    pub(crate) fn force_layout(&mut self, id: usize) -> bool {
        let forced = self.measurements.forced.insert(id);
        if forced {
            self.measurements.waking.push(id);
            self.cull.reskipped = true;
        }
        forced
    }

    // The natural size of a decoded image.
    pub(crate) fn set_natural(&mut self, id: usize, size: [f32; 2]) {
        self.measurements.naturals.insert(id, size);
    }

    #[cfg(test)]
    pub(crate) fn set_container(&mut self, id: usize, size: [f32; 2]) {
        self.containers.insert(id, size);
    }

    // The content of node `id` changed, so it lays out again.
    pub(crate) fn relayout(&mut self, id: usize) {
        if let Some(node) = self.layout_nodes.get_mut(id) {
            *node = None;
            self.cull.seen = None;
        }
    }

    // A node left the page and waits for a GC: its layout data can go.
    pub(crate) fn forget_removed(&mut self, id: usize) {
        self.measurements.shown.remove(&id);
        self.autos.remove(&id);
        self.measurements.remembered.remove(&id);
        self.measurements.texts.remove(&id);
        if !self.pseudo_boxes.is_empty() {
            for pseudo in 1..3 {
                self.pseudo_boxes
                    .remove(&crate::render::inline::atom_key(id, pseudo));
            }
        }
        if let Some(node) = self.layout_nodes.get_mut(id) {
            *node = None;
        }
    }

    // A node is freed and its ID can come back for another node.
    pub(crate) fn forget(&mut self, id: usize) {
        self.tracks.remove(&id);
        self.areas.remove(&id);
        self.autos.remove(&id);
        self.cull.widths.remove(&id);
        self.text_layouts.remove(&id);
        self.cull.offsets.remove(&id);
        self.grids.remove(&id);
        self.baselines.remove(&id);
        if let Some(node) = self.layout_nodes.get_mut(id) {
            *node = None;
        }
        self.measurements.texts.remove(&id);
        self.cull.spills.remove(&id);
        self.measurements.naturals.remove(&id);
        self.anchors.remove(&id);
        self.anchors.remove(&crate::render::inline::atom_key(id, 2));
        self.blocks.remove(&id);
        self.containers.remove(&id);
        self.flexed_grids.remove(&id);
        self.grid_widest.remove(&id);
        for pseudo in 0..3 {
            self.atoms
                .remove(&crate::render::inline::atom_key(id, pseudo));
        }
        if let Some(order) = self.painted.get_mut(id) {
            *order = 0;
        }
    }

    pub(crate) fn forget_all(&mut self, gone: &rustc_hash::FxHashSet<usize>) {
        self.measurements
            .sizes
            .retain(|(node, ..), _| !gone.contains(node));
    }

    pub(crate) fn trim(&mut self, len: usize) {
        self.measurements.texts = Default::default();
        self.layout_nodes.truncate(len);
        self.layout_nodes.shrink_to_fit();
        self.painted.truncate(len);
        self.painted.shrink_to_fit();
        self.cull.trim(len);
    }
}
