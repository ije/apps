#[derive(Default)]
pub struct Interaction {
    pub(crate) focus: Option<usize>,
    pub(crate) states: [Vec<usize>; 3],
    pub selection: rustc_hash::FxHashMap<usize, [usize; 2]>,
    pub selection_caret: Option<usize>,
    pub top_layer: Vec<usize>,
    // The custom highlights by name, from the lowest to the highest, and their parts.
    pub(crate) highlights: Vec<(std::rc::Rc<str>, u32)>,
    pub(crate) highlighted: rustc_hash::FxHashMap<u32, Highlight>,
    pub(crate) highlight_changes: u64,
}

// The text nodes of each range, and the parts in each text node with their ranges.
#[derive(Default)]
pub(crate) struct Highlight {
    pub(crate) ranges: rustc_hash::FxHashMap<u32, Vec<usize>>,
    pub(crate) nodes: rustc_hash::FxHashMap<usize, Vec<(u32, [usize; 2])>>,
}

impl Interaction {
    pub(crate) fn forget(&mut self, id: usize) {
        if self.focus == Some(id) {
            self.focus = None;
        }
        self.selection.remove(&id);
        for highlight in self.highlighted.values_mut() {
            highlight.nodes.remove(&id);
        }
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
