#[derive(Default)]
pub struct Tree {
    pub(crate) free: std::collections::BinaryHeap<std::cmp::Reverse<usize>>,
    pub(crate) removed: Vec<usize>,
    pub(crate) created: usize,
    pub(crate) detached: Vec<usize>,
    pub(crate) live: rustc_hash::FxHashSet<usize>,
    pub shadows: rustc_hash::FxHashMap<usize, usize>,
    pub(crate) hosts: rustc_hash::FxHashMap<usize, usize>,
    pub(crate) reserved: rustc_hash::FxHashSet<usize>,
}
