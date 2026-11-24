use super::LayoutMirror;
use std::rc::Rc;

// A move of the mirror for a scroll: by how much, and the nodes of the walk it moved.
type ScrollShift = ([f32; 2], Rc<Vec<(u32, u32)>>, std::ops::Range<usize>);

// Where a canvas drew a drawable element, which takes the pointer there.
pub(crate) struct Drawn {
    // From the border box of the element to the content box of the canvas, in CSS pixels.
    pub(crate) matrix: [f32; 16],
    // The part of the content box of the canvas that shows the element.
    pub(crate) clip: Option<[f32; 4]>,
    // Elements drawn later take the pointer first.
    pub(crate) order: u32,
}

#[derive(Default)]
pub struct Geometry {
    pub mirror: LayoutMirror,
    // The parts of boxes that columns split, which take pointer events.
    pub(crate) fragments: rustc_hash::FxHashMap<usize, Box<[[f32; 4]]>>,
    // The parts of spliced inline boxes in each line of text that holds them, by the key of that text.
    pub(crate) pieces: rustc_hash::FxHashMap<usize, Vec<(usize, [f32; 4])>>,
    pub scroll: rustc_hash::FxHashMap<usize, renderer::ScrollRef>,
    pub(crate) scroll_offsets: rustc_hash::FxHashMap<usize, renderer::Point<renderer::Pixels>>,
    // The scroll offsets that the mirror shows, as of the last prepaint, the moves of the mirror for user scrolls
    // since then, and whether the boxes in each scroll move as a whole, by style epoch and walk.
    pub(crate) mirror_offsets: rustc_hash::FxHashMap<usize, renderer::Point<renderer::Pixels>>,
    pub(crate) scroll_shifts: Vec<ScrollShift>,
    // Scrolls that moved since the mirror did, and whether they alone left it stale.
    pub(crate) unshifted: Vec<usize>,
    pub(crate) only_scrolled: bool,
    pub(crate) shiftable: rustc_hash::FxHashMap<usize, (u64, u64, bool)>,
    pub(crate) layout_epoch: u64,
    pub(crate) drawn: rustc_hash::FxHashMap<usize, Drawn>,
    pub(crate) drawn_order: u32,
}
