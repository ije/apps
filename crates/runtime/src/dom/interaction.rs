#[derive(Default)]
pub struct Interaction {
    pub(crate) focus: Option<usize>,
    pub(crate) states: [Vec<usize>; 3],
    pub selection: rustc_hash::FxHashMap<usize, [usize; 2]>,
    pub selection_caret: Option<usize>,
    pub top_layer: Vec<usize>,
}

impl Interaction {
    pub(crate) fn forget(&mut self, id: usize) {
        if self.focus == Some(id) {
            self.focus = None;
        }
        self.selection.remove(&id);
        if self.selection_caret == Some(id) {
            self.selection_caret = None;
        }
    }

    pub(crate) fn forget_all(&mut self, gone: &rustc_hash::FxHashSet<usize>) {
        self.top_layer.retain(|node| !gone.contains(node));
        for ids in &mut self.states {
            ids.retain(|node| !gone.contains(node));
        }
    }
}
