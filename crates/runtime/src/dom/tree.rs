use super::{Document, Node};

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

impl Tree {
    // A free node ID, or a new one at the end of the arena.
    pub(crate) fn slot(&mut self, doc: &mut Document) -> usize {
        self.free.pop().map_or_else(
            || {
                doc.nodes.push(Node::default());
                doc.nodes.len() - 1
            },
            |id| id.0,
        )
    }

    pub(crate) fn place(&mut self, doc: &mut Document, id: usize, node: Node) {
        doc.nodes[id] = node;
        self.detached.push(id);
    }

    // Whether `to` is above `id`, through tree parents and the hosts of shadow roots.
    pub(crate) fn reaches(&self, doc: &Document, mut id: usize, to: usize) -> bool {
        loop {
            if id == to {
                return true;
            }
            match doc
                .nodes
                .get(id)
                .and_then(|node| node.parent)
                .or_else(|| self.hosts.get(&id).copied())
            {
                Some(parent) => id = parent,
                None => return false,
            }
        }
    }

    // The nodes of the removed subtrees that are still out of the tree: their data can go before a GC.
    pub(crate) fn take_removed(&mut self, doc: &Document) -> Vec<usize> {
        let mut gone = Vec::new();
        for root in std::mem::take(&mut self.removed) {
            if doc.nodes.get(root).is_none_or(|node| node.parent.is_some())
                || self.hosts.contains_key(&root)
            {
                continue;
            }
            let mut next = gone.len();
            gone.push(root);
            while next < gone.len() {
                let id = gone[next];
                next += 1;
                gone.extend(&doc.nodes[id].children);
                gone.extend(self.shadows.get(&id));
            }
        }
        gone
    }

    // Pages report the nodes that have wrappers, which keep their subtrees, and the wrappers that died.
    pub(crate) fn mark_live(
        &mut self,
        doc: &Document,
        wrapped: impl Iterator<Item = usize>,
        dead: impl Iterator<Item = usize>,
    ) {
        for id in wrapped {
            if doc.nodes.get(id).is_some_and(|node| !node.tag.is_empty()) {
                self.live.insert(id);
            }
        }
        for id in dead {
            self.live.remove(&id);
        }
    }

    // The nodes of detached subtrees that no wrapper holds. Each root with a held node waits for the next call.
    pub(crate) fn collect(&mut self, doc: &Document) -> Vec<usize> {
        let mut roots = std::mem::take(&mut self.detached);
        roots.sort_unstable();
        roots.dedup();
        let mut dead = Vec::new();
        for root in roots {
            if root == 0
                || doc
                    .nodes
                    .get(root)
                    .is_none_or(|node| node.parent.is_some() || node.tag.is_empty())
                || self.hosts.contains_key(&root)
            {
                continue;
            }
            let mut pending = vec![root];
            let start = dead.len();
            let mut live = false;
            while let Some(id) = pending.pop() {
                if self.live.contains(&id) {
                    live = true;
                    break;
                }
                dead.push(id);
                pending.extend(&doc.nodes[id].children);
                if let Some(root) = self.shadows.get(&id) {
                    pending.push(*root);
                }
            }
            if live {
                dead.truncate(start);
                self.detached.push(root);
            }
        }
        dead
    }

    // Counts work that makes garbage for the page's GC, until `take_created`.
    pub(crate) fn note_created(&mut self, count: usize) {
        self.created += count;
    }

    pub(crate) fn take_created(&mut self) -> usize {
        std::mem::take(&mut self.created)
    }

    pub(crate) fn free_count(&self) -> usize {
        self.free.len()
    }

    // Empties a dead node and gives its ID back.
    pub(crate) fn free(&mut self, doc: &mut Document, id: usize) {
        doc.nodes[id] = Node::default();
        if let Some(root) = self.shadows.remove(&id) {
            self.hosts.remove(&root);
        }
        self.hosts.remove(&id);
        self.free.push(std::cmp::Reverse(id));
    }

    // Cuts the free IDs at the end of the arena of `len` nodes, and returns the new length.
    pub(crate) fn trim(&mut self, mut len: usize) -> usize {
        let mut free = std::mem::take(&mut self.free).into_sorted_vec();
        let cut = free
            .iter()
            .take_while(|id| {
                let last = id.0 + 1 == len;
                if last {
                    len = id.0;
                }
                last
            })
            .count();
        free.drain(..cut);
        free.shrink_to_fit();
        self.free = free.into();
        len
    }
}
