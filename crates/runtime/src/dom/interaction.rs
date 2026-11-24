#[derive(Default)]
pub struct Interaction {
    pub(crate) focus: Option<usize>,
    pub(crate) states: [Vec<usize>; 3],
    pub selection: rustc_hash::FxHashMap<usize, [usize; 2]>,
    pub selection_caret: Option<usize>,
    pub top_layer: Vec<usize>,
}
