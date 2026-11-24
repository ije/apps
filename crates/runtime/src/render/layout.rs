use crate::css;
use std::{collections::HashMap, rc::Rc, sync::Arc};

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

pub(crate) type SvgBoxes = (
    Rc<str>,
    Vec<(String, usize)>,
    [f32; 2],
    Vec<(usize, [f32; 4], [f32; 4])>,
    [f32; 4],
    [f32; 4],
    [f32; 4],
);

// The mask of a box, 0, or of its `::before` or `::after`, 1 and 2: its inputs and its image.
pub(crate) type Mask = (String, Option<Arc<renderer::RenderImage>>);

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
    pub(crate) grids: rustc_hash::FxHashMap<
        usize,
        (
            f32,
            Option<f32>,
            Option<crate::render::grid::Grid>,
            crate::render::grid::Grid,
        ),
    >,
    pub(crate) flexed_grids: rustc_hash::FxHashMap<usize, [Option<(f32, f32)>; 2]>,
    pub(crate) grid_widest: rustc_hash::FxHashMap<usize, f32>,
    // The used columns and rows of the last layout of each grid, for `getComputedStyle()`.
    pub(crate) tracks: rustc_hash::FxHashMap<usize, [Box<[f32]>; 2]>,
    // The grid area of each grid item at its last build, for `getComputedStyle()`.
    pub(crate) areas: rustc_hash::FxHashMap<usize, [f32; 2]>,
    pub(crate) baselines: crate::render::grid::Baselines,
    pub(crate) layout_nodes: Vec<Option<LayoutNode>>,
    pub(crate) measurements: crate::render::grid::Measurements,
    pub painted: Vec<u64>,
    // A hash of the state that paint reads outside the boxes: images and the selection.
    pub(crate) paint_inputs: u64,
    pub(crate) layout_revision: u64,
    pub(crate) containers: rustc_hash::FxHashMap<usize, [f32; 2]>,
    pub(crate) atoms: rustc_hash::FxHashMap<usize, [f32; 3]>,
    // The rendered `::before` and `::after` boxes that scripts can read, by their atom key: the place of the border
    // box from the border box of the element in layout, its size, then the padding plus border of each side.
    pub pseudo_boxes: rustc_hash::FxHashMap<usize, [f32; 8]>,
    pub(crate) container_pass: bool,
    pub(crate) anchors: rustc_hash::FxHashMap<usize, renderer::Rect<renderer::Pixels>>,
    pub(crate) blocks: rustc_hash::FxHashMap<usize, renderer::Rect<renderer::Pixels>>,
    pub(crate) cull: crate::render::cull::Cull,
    // The `content-visibility: auto` boxes, each with the boxes on its path, the boxes that clip it, and the view
    // and its top and bottom at its last change.
    #[allow(clippy::type_complexity)]
    pub(crate) autos: rustc_hash::FxHashMap<usize, (Rc<[usize]>, Rc<[usize]>, (u64, [f32; 2]))>,
    // `contentvisibilityautostatechange` events: the box and whether it skips its contents now.
    pub(crate) skip_events: Vec<(usize, bool)>,
}

#[derive(Default)]
pub struct Rasters {
    pub(crate) svg_boxes: rustc_hash::FxHashMap<usize, SvgBoxes>,
    pub(crate) svg_trees: rustc_hash::FxHashMap<usize, usvg::Tree>,
    pub(crate) masks: rustc_hash::FxHashMap<(usize, u8), Mask>,
    pub(crate) svgs: rustc_hash::FxHashMap<usize, (Rc<str>, Arc<renderer::RenderImage>)>,
    pub(crate) svg_cache: HashMap<
        Rc<str>,
        (
            crate::render::canvas::SvgGeometry,
            Vec<Arc<renderer::RenderImage>>,
        ),
    >,
    pub(crate) backgrounds: HashMap<(usize, usize), (String, Arc<renderer::RenderImage>)>,
    pub(crate) bitmaps: HashMap<String, Arc<renderer::RenderImage>>,
    // The decoded source of each background layer, kept while the layer keeps its raster, so `retire` keeps it.
    pub(crate) layer_sources: HashMap<(usize, usize), Arc<renderer::RenderImage>>,
    pub(crate) gradients: Vec<(String, Arc<renderer::RenderImage>)>,
}
