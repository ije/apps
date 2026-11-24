use super::Document;
use std::rc::Rc;

#[derive(Default)]
pub(crate) struct Walk {
    tree: u64,
    composed: rustc_hash::FxHashMap<usize, Vec<usize>>,
    pub(crate) nodes: Rc<Vec<(u32, u32)>>,
    // The position and subtree length of each node in `nodes`, by ID. An absent node has the position ABSENT.
    index: Rc<Vec<[u32; 2]>>,
    pub(crate) serial: u64,
    // The serial of the walk before the last patch, the nodes that the patch touched, and the count of those
    // that moved, joined or left. The others only changed.
    delta: Option<(u64, Rc<Vec<u32>>, usize)>,
    // The lists before the last patch. The next patch writes into them when no one else holds them, because
    // new lists of this size cost page faults.
    spare: Rc<Vec<(u32, u32)>>,
    spare_index: Rc<Vec<[u32; 2]>>,
}

pub(crate) const ABSENT: u32 = u32::MAX;

enum Patch<'a> {
    Span(u32),
    Node {
        id: u32,
        start: usize,
        end: u32,
        children: &'a [usize],
        // The old place of each child that keeps its walk, or ABSENT.
        places: Vec<u32>,
        next: usize,
    },
}

impl Walk {
    pub(crate) fn update(
        &mut self,
        doc: &Document,
        composed: &rustc_hash::FxHashMap<usize, Vec<usize>>,
        shared: Option<&Walk>,
    ) -> bool {
        let tree = doc.nodes.tree();
        if self.tree == tree && !self.nodes.is_empty() && self.composed == *composed {
            return false;
        }
        if let Some(shared) = shared
            .filter(|shared| shared.tree == tree && !shared.nodes.is_empty())
            .filter(|shared| shared.composed == *composed)
        {
            self.tree = tree;
            self.composed.clone_from(composed);
            if Rc::ptr_eq(&self.nodes, &shared.nodes) {
                return false;
            }
            self.delta = shared.delta.clone().filter(|delta| delta.0 == self.serial);
            self.nodes = shared.nodes.clone();
            self.index = shared.index.clone();
            self.serial = shared.serial;
            return true;
        }
        let patched = self.patch(doc, composed);
        self.tree = tree;
        self.composed.clone_from(composed);
        #[cfg(debug_assertions)]
        let kept = patched.map(|changed| (changed, self.nodes.clone(), self.index.clone()));
        #[cfg(debug_assertions)]
        let patched = None::<bool>;
        if let Some(changed) = patched {
            return changed;
        }
        let mut nodes = Vec::with_capacity(self.nodes.len());
        let mut pending = vec![(0, ABSENT)];
        while let Some((id, parent)) = pending.pop() {
            nodes.push((id, parent));
            pending.extend(
                composed
                    .get(&(id as usize))
                    .unwrap_or(&doc.nodes[id as usize].children)
                    .iter()
                    .rev()
                    .map(|&child| (child as u32, id)),
            );
        }
        let mut index = vec![[ABSENT, 1]; doc.nodes.len()];
        for (at, &(id, _)) in nodes.iter().enumerate() {
            index[id as usize][0] = at as u32;
        }
        for &(id, parent) in nodes[1..].iter().rev() {
            index[parent as usize][1] += index[id as usize][1];
        }
        #[cfg(debug_assertions)]
        if let Some((changed, patched, positions)) = kept {
            assert!(*patched == nodes, "patched walk differs");
            assert!(
                nodes
                    .iter()
                    .all(|&(id, _)| positions[id as usize] == index[id as usize])
                    && positions.iter().filter(|slot| slot[0] != ABSENT).count() == nodes.len(),
                "patched walk index differs"
            );
            return changed;
        }
        self.nodes = Rc::new(nodes);
        self.index = Rc::new(index);
        self.serial = super::document::stamp();
        self.delta = None;
        true
    }

    // The nodes that moved, joined, left or changed since the walk with this serial, with duplicates. Other
    // nodes keep their parent and their order.
    pub(crate) fn since(&self, serial: u64) -> Option<&[u32]> {
        let (from, touched, _) = self.delta.as_ref()?;
        (*from == serial).then_some(touched)
    }

    // The nodes of `since` that only changed: they have a newer revision or other children, at their old place.
    pub(crate) fn changed_since(&self, serial: u64) -> Option<&[u32]> {
        let (_, _, moved) = self.delta.as_ref()?;
        Some(&self.since(serial)?[*moved..])
    }

    pub(crate) fn contains(&self, id: usize) -> bool {
        self.place(id).is_some()
    }

    pub(crate) fn place(&self, id: usize) -> Option<u32> {
        self.index
            .get(id)
            .map(|slot| slot[0])
            .filter(|&at| at != ABSENT)
    }

    // The count of the nodes in the walk from this node to the last one below it.
    pub(crate) fn span(&self, id: usize) -> usize {
        self.index.get(id).map_or(1, |slot| slot[1] as usize)
    }

    pub(crate) fn parent(&self, id: usize) -> Option<u32> {
        Some(self.nodes[self.place(id)? as usize].1)
    }

    // Copies the old walk and walks again only below nodes with a newer revision or other composed children.
    // Returns whether the walk changed, or None when a fresh walk is needed.
    fn patch(
        &mut self,
        doc: &Document,
        composed: &rustc_hash::FxHashMap<usize, Vec<usize>>,
    ) -> Option<bool> {
        let old = self.nodes.clone();
        if old.is_empty() || self.index.len() > doc.nodes.len() {
            return None;
        }
        let place = |index: &[[u32; 2]], id: usize| index.get(id).map_or(ABSENT, |slot| slot[0]);
        let mut marks: Vec<u32> = doc
            .nodes
            .changed_since(self.tree)
            .chain(
                composed
                    .iter()
                    .filter(|&(id, children)| self.composed.get(id) != Some(children))
                    .map(|(&id, _)| id),
            )
            .chain(
                self.composed
                    .keys()
                    .copied()
                    .filter(|id| !composed.contains_key(id)),
            )
            .map(|id| place(&self.index, id))
            .filter(|&at| at != ABSENT)
            .collect();
        marks.sort_unstable();
        marks.dedup();
        if marks.is_empty() {
            return Some(false);
        }
        if marks.len() > old.len() / 8 {
            return None;
        }
        let children = |id: u32| {
            composed
                .get(&(id as usize))
                .unwrap_or(&doc.nodes[id as usize].children)
                .as_slice()
        };
        let mut out = Rc::try_unwrap(std::mem::take(&mut self.spare)).unwrap_or_default();
        out.clear();
        out.reserve(old.len() + 64);
        let mut index = std::mem::take(&mut self.index);
        if Rc::get_mut(&mut index).is_none() {
            let spare = std::mem::take(&mut self.spare_index);
            let mut copy = Rc::try_unwrap(spare).unwrap_or_default();
            copy.clear();
            copy.extend_from_slice(&index);
            self.spare_index = std::mem::replace(&mut index, Rc::new(copy));
        }
        let sizes = Rc::get_mut(&mut index).expect("an index of its own");
        sizes.resize(doc.nodes.len(), [ABSENT, 1]);
        // The parts of the new walk that are not at their place in the old one.
        let mut moved: Vec<std::ops::Range<usize>> = Vec::new();
        macro_rules! copy {
            ($from:expr, $to:expr) => {
                let (from, to) = ($from as usize, $to as usize);
                if out.len() != from {
                    moved.push(out.len()..out.len() + to - from);
                }
                out.extend_from_slice(&old[from..to]);
            };
        }
        let (mut touched, mut changed) = (Vec::new(), Vec::new());
        let mut pending = Vec::new();
        let (mut cursor, mut mark) = (0u32, 0);
        let mut stack = vec![Patch::Span(old.len() as u32)];
        while let Some(top) = stack.last_mut() {
            match top {
                Patch::Span(end) => {
                    while marks.get(mark).is_some_and(|&at| at < cursor) {
                        mark += 1;
                    }
                    let Some(&at) = marks.get(mark).filter(|&&at| at < *end) else {
                        copy!(cursor, *end);
                        cursor = *end;
                        stack.pop();
                        continue;
                    };
                    mark += 1;
                    copy!(cursor, at);
                    let (id, parent) = old[at as usize];
                    changed.push(id);
                    cursor = at + 1;
                    let (list, end) = (children(id), at + sizes[id as usize][1]);
                    let kept = |child: &usize| {
                        let place = place(sizes, *child);
                        let own = place != ABSENT && old[place as usize] == (*child as u32, id);
                        if own { place } else { ABSENT }
                    };
                    let mut places: Vec<u32> = list.iter().map(kept).collect();
                    // Children keep their walk in the longest run of rising old places, so a child that
                    // moved up doesn't make all the children after it move.
                    if !places.iter().filter(|&&place| place != ABSENT).is_sorted() {
                        let mut tails: Vec<usize> = Vec::new();
                        let mut before = vec![usize::MAX; places.len()];
                        for (child, &place) in places.iter().enumerate() {
                            if place == ABSENT {
                                continue;
                            }
                            let run = tails.partition_point(|&tail| places[tail] < place);
                            if run > 0 {
                                before[child] = tails[run - 1];
                            }
                            match tails.get_mut(run) {
                                Some(tail) => *tail = child,
                                None => tails.push(child),
                            }
                        }
                        let mut last = tails.last().copied().unwrap_or(usize::MAX);
                        let all = std::mem::replace(&mut places, vec![ABSENT; before.len()]);
                        while last != usize::MAX {
                            places[last] = all[last];
                            last = before[last];
                        }
                    }
                    stack.push(Patch::Node {
                        id,
                        start: out.len(),
                        end,
                        children: list,
                        places,
                        next: 0,
                    });
                    moved.push(out.len()..out.len() + 1);
                    out.push((id, parent));
                }
                Patch::Node {
                    id,
                    start,
                    end,
                    children: list,
                    places,
                    next,
                } => {
                    let Some(&child) = list.get(*next) else {
                        touched.extend(
                            old[cursor as usize..*end as usize]
                                .iter()
                                .map(|node| node.0),
                        );
                        cursor = *end;
                        let (id, start) = (*id, *start);
                        stack.pop();
                        let size = (out.len() - start) as u32;
                        let grown =
                            size.wrapping_sub(std::mem::replace(&mut sizes[id as usize][1], size));
                        // Nodes above that are not on the stack keep their old walk. Only their length changes.
                        let above = stack.iter().rev().find_map(|patch| match patch {
                            Patch::Node { id, .. } => Some(*id),
                            Patch::Span(_) => None,
                        });
                        let mut parent = out[start].1;
                        while grown != 0 && parent != ABSENT && Some(parent) != above {
                            let slot = &mut sizes[parent as usize];
                            slot[1] = slot[1].wrapping_add(grown);
                            parent = old[slot[0] as usize].1;
                        }
                        continue;
                    };
                    let at = places[*next];
                    *next += 1;
                    if at != ABSENT {
                        touched.extend(old[cursor as usize..at as usize].iter().map(|node| node.0));
                        cursor = at;
                        stack.push(Patch::Span(at + sizes[child][1]));
                        continue;
                    }
                    let from = out.len();
                    pending.push((child as u32, *id));
                    while let Some((id, parent)) = pending.pop() {
                        out.push((id, parent));
                        touched.push(id);
                        pending.extend(children(id).iter().rev().map(|&child| (child as u32, id)));
                    }
                    moved.push(from..out.len());
                    for &(id, _) in &out[from..] {
                        sizes[id as usize][1] = 1;
                    }
                    for at in (from + 1..out.len()).rev() {
                        let (id, parent) = out[at];
                        sizes[parent as usize][1] += sizes[id as usize][1];
                    }
                }
            }
        }
        for &id in &touched {
            sizes[id as usize][0] = ABSENT;
        }
        for at in moved.into_iter().flatten() {
            sizes[out[at].0 as usize][0] = at as u32;
        }
        self.index = index;
        // After a patch of most of the walk, a full pass costs readers less than this list.
        let most = touched.len() > out.len() / 2;
        self.spare = std::mem::replace(&mut self.nodes, Rc::new(out));
        let moved = touched.len();
        touched.extend(changed);
        self.delta = (!most).then(|| (self.serial, Rc::new(touched), moved));
        self.serial = super::document::stamp();
        Some(true)
    }
}
