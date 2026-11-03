use crate::{Bounds, geometry::Unit};
use std::{cmp, fmt::Debug, ptr::NonNull};

/// More children make the tree shorter, but each node slower to scan.
const MAX_CHILDREN: usize = 12;

/// An R-tree that orders overlapping bounds.
#[derive(Debug)]
pub(crate) struct BoundsTree<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    nodes: Vec<Node<U>>,
    root: Option<usize>,
    /// The leaf with the highest order, checked first.
    max_leaf: Option<usize>,
    insert_path: Vec<usize>,
    search_stack: Vec<NonNull<Node<U>>>,
}

#[derive(Debug, Clone)]
struct Node<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    /// Covers all descendants.
    bounds: Bounds<U>,
    max_order: u32,
    kind: NodeKind,
}

#[derive(Debug, Clone)]
enum NodeKind {
    Leaf {
        order: u32,
    },
    Internal {
        /// 2 to `MAX_CHILDREN` children.
        children: NodeChildren,
    },
}

#[derive(Debug, Clone)]
struct NodeChildren {
    // The child with the highest max order is always last.
    indices: [usize; MAX_CHILDREN],
    len: u8,
}

impl NodeChildren {
    fn new() -> Self {
        Self {
            indices: [0; MAX_CHILDREN],
            len: 0,
        }
    }

    fn push(&mut self, index: usize) {
        debug_assert!((self.len as usize) < MAX_CHILDREN);
        self.indices[self.len as usize] = index;
        self.len += 1;
    }

    fn len(&self) -> usize {
        self.len as usize
    }

    fn as_slice(&self) -> &[usize] {
        &self.indices[..self.len as usize]
    }
}

impl<U: Unit> BoundsTree<U> {
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.root = None;
        self.max_leaf = None;
        self.insert_path.clear();
        self.search_stack.clear();
    }

    /// Returns the order of the new bounds: one above the highest order of the bounds it
    /// intersects.
    pub fn insert(&mut self, new_bounds: Bounds<U>) -> u32 {
        let max_intersecting = self.find_max_ordering(&new_bounds);
        let ordering = max_intersecting + 1;

        let new_leaf_idx = self.insert_leaf(new_bounds, ordering);

        self.max_leaf = match self.max_leaf {
            None => Some(new_leaf_idx),
            Some(old_idx) if self.nodes[old_idx].max_order < ordering => Some(new_leaf_idx),
            some => some,
        };

        ordering
    }

    fn find_max_ordering(&mut self, query: &Bounds<U>) -> u32 {
        let Some(root_idx) = self.root else {
            return 0;
        };

        if let Some(max_idx) = self.max_leaf {
            let max_node = &self.nodes[max_idx];
            if query.intersects(&max_node.bounds) {
                return max_node.max_order;
            }
        }

        self.search_stack.clear();
        self.search_stack.push(NonNull::from(&self.nodes[root_idx]));

        let mut max_found = 0u32;

        while let Some(node) = self.search_stack.pop() {
            // SAFETY: `nodes` is not changed in this function, and `search_stack` only holds
            // pointers pushed by this call.
            let node = unsafe { node.as_ref() };

            if node.max_order <= max_found {
                continue;
            }

            if !query.intersects(&node.bounds) {
                continue;
            }

            match &node.kind {
                NodeKind::Leaf { order } => {
                    max_found = cmp::max(max_found, *order);
                }
                NodeKind::Internal { children } => {
                    // The highest child is last, so it is popped first.
                    self.search_stack.extend(
                        children
                            .as_slice()
                            .iter()
                            .map(|&child_idx| &self.nodes[child_idx])
                            .filter(|node| node.max_order > max_found)
                            .map(NonNull::from),
                    );
                }
            }
        }

        max_found
    }

    fn insert_leaf(&mut self, bounds: Bounds<U>, order: u32) -> usize {
        let new_leaf_idx = self.nodes.len();
        self.nodes.push(Node {
            bounds,
            max_order: order,
            kind: NodeKind::Leaf { order },
        });

        let Some(root_idx) = self.root else {
            self.root = Some(new_leaf_idx);
            return new_leaf_idx;
        };

        if matches!(self.nodes[root_idx].kind, NodeKind::Leaf { .. }) {
            let root_bounds = self.nodes[root_idx].bounds;
            let root_order = self.nodes[root_idx].max_order;

            let mut children = NodeChildren::new();
            if order > root_order {
                children.push(root_idx);
                children.push(new_leaf_idx);
            } else {
                children.push(new_leaf_idx);
                children.push(root_idx);
            }

            let new_root_idx = self.nodes.len();
            self.nodes.push(Node {
                bounds: root_bounds.union(&bounds),
                max_order: cmp::max(root_order, order),
                kind: NodeKind::Internal { children },
            });
            self.root = Some(new_root_idx);
            return new_leaf_idx;
        }

        self.insert_path.clear();
        let mut current_idx = root_idx;

        loop {
            let current = &self.nodes[current_idx];
            let NodeKind::Internal { children } = &current.kind else {
                unreachable!("Should only traverse internal nodes");
            };

            self.insert_path.push(current_idx);

            // The child whose bounds grow least, by half perimeter.
            let mut best_child_idx = children.as_slice()[0];
            let mut best_child_pos = 0;
            let mut best_cost = bounds
                .union(&self.nodes[best_child_idx].bounds)
                .half_perimeter();

            for (pos, &child_idx) in children.as_slice().iter().enumerate().skip(1) {
                let cost = bounds.union(&self.nodes[child_idx].bounds).half_perimeter();
                if cost < best_cost {
                    best_cost = cost;
                    best_child_idx = child_idx;
                    best_child_pos = pos;
                }
            }

            if matches!(self.nodes[best_child_idx].kind, NodeKind::Leaf { .. }) {
                if children.len() < MAX_CHILDREN {
                    let node = &mut self.nodes[current_idx];

                    if let NodeKind::Internal { children } = &mut node.kind {
                        children.push(new_leaf_idx);
                        // Keeps the highest child last.
                        if order <= node.max_order {
                            let last = children.len() - 1;
                            children.indices.swap(last - 1, last);
                        }
                    }

                    node.bounds = node.bounds.union(&bounds);
                    node.max_order = cmp::max(node.max_order, order);
                    break;
                } else {
                    // The node is full, so the best leaf and the new one move into a new node.
                    let sibling_bounds = self.nodes[best_child_idx].bounds;
                    let sibling_order = self.nodes[best_child_idx].max_order;

                    let mut new_children = NodeChildren::new();
                    if order > sibling_order {
                        new_children.push(best_child_idx);
                        new_children.push(new_leaf_idx);
                    } else {
                        new_children.push(new_leaf_idx);
                        new_children.push(best_child_idx);
                    }

                    let new_internal_idx = self.nodes.len();
                    let new_internal_max = cmp::max(sibling_order, order);
                    self.nodes.push(Node {
                        bounds: sibling_bounds.union(&bounds),
                        max_order: new_internal_max,
                        kind: NodeKind::Internal {
                            children: new_children,
                        },
                    });

                    let parent = &mut self.nodes[current_idx];
                    if let NodeKind::Internal { children } = &mut parent.kind {
                        let children_len = children.len();

                        children.indices[best_child_pos] = new_internal_idx;

                        if new_internal_max > parent.max_order {
                            children.indices.swap(best_child_pos, children_len - 1);
                        }
                    }
                    break;
                }
            } else {
                current_idx = best_child_idx;
            }
        }

        let mut updated_child_idx = None;
        for &node_idx in self.insert_path.iter().rev() {
            let node = &mut self.nodes[node_idx];
            node.bounds = node.bounds.union(&bounds);

            if node.max_order < order {
                node.max_order = order;

                // Moves the updated child last. The deepest node did that when the leaf was added.
                if let Some(child_idx) = updated_child_idx
                    && let NodeKind::Internal { children } = &mut node.kind
                    && let Some(pos) = children.as_slice().iter().position(|&c| c == child_idx)
                {
                    let last = children.len() - 1;
                    if pos != last {
                        children.indices.swap(pos, last);
                    }
                }
            }

            updated_child_idx = Some(node_idx);
        }

        new_leaf_idx
    }
}

impl<U> Default for BoundsTree<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    fn default() -> Self {
        BoundsTree {
            nodes: Vec::new(),
            root: None,
            max_leaf: None,
            insert_path: Vec::new(),
            search_stack: Vec::new(),
        }
    }
}
