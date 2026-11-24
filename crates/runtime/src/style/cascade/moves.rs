// Marks the nodes that a fast pass must style again because they moved, and unstyles the nodes that left the walk.
use crate::css::properties::{Style, Vars};
use crate::dom::{Document, SelectorCache};
use crate::style::Shadows;
use crate::style::cascade::computed::ComputedStyles;
use crate::style::cascade::{Styled, TreeWalk};
use std::rc::Rc;
#[allow(clippy::too_many_arguments)]
pub(crate) fn mark_moves(
    doc: &Document,
    walk: &TreeWalk,
    shadows: &Shadows,
    styled: Option<&Styled>,
    matches: &mut SelectorCache,
    out: &mut ComputedStyles,
    changed: &mut Vec<u64>,
    recount: &mut bool,
    (unstyled, empty): (&Rc<Style>, &Vars),
) {
    let order = walk.walk.nodes.as_slice();
    if changed.len() < doc.nodes.len() {
        changed.resize(doc.nodes.len(), 0);
    }
    if let Some(old) = styled.filter(|old| !Rc::ptr_eq(&old.nodes, &walk.walk.nodes)) {
        // Text and comments that move need no new count.
        let changed = walk.walk.changed_since(old.serial).map_or(0, <[u32]>::len);
        *recount = walk.walk.since(old.serial).is_none_or(|touched| {
            touched[..touched.len() - changed].iter().any(|&id| {
                doc.nodes
                    .get(id as usize)
                    .is_none_or(|node| !node.tag.starts_with('#'))
            })
        });
        const GONE: u32 = u32::MAX - 1;
        const KEPT: u32 = u32::MAX - 2;
        let root = |tree: &[u32], trees: &[(usize, usize)], id: usize| {
            tree.get(id)
                .and_then(|&index| trees.get(index as usize))
                .map(|tree| tree.0)
        };
        // Without shadow trees, only the nodes that a patched walk touched can have another place.
        let touched = walk
            .walk
            .since(old.serial)
            .filter(|_| shadows.hosts.is_empty() && old.trees.len() == 1);
        let mut placed = Vec::new();
        if touched.is_none() || cfg!(debug_assertions) {
            placed.resize(doc.nodes.len(), GONE);
            for &(id, up) in old.nodes.iter() {
                if let Some(slot) = placed.get_mut(id as usize) {
                    *slot = up;
                }
            }
        }
        match touched {
            Some(touched) => {
                // A node with another parent is dirty by its snapshot. A node that joins has no style.
                for &id in touched {
                    let id = id as usize;
                    if !walk.walk.contains(id) {
                        out.leave(id, unstyled, empty);
                    } else if Rc::ptr_eq(&out.values[id], unstyled) {
                        matches.dirty[id] = true;
                    }
                }
                #[cfg(debug_assertions)]
                for (nodes, kept) in [(order, true), (&old.nodes[..], false)] {
                    for &(id, up) in nodes {
                        let slot = &mut placed[id as usize];
                        assert!(
                            *slot == if kept { up } else { KEPT } || touched.contains(&id),
                            "node {id} moved outside the patched walk"
                        );
                        assert!(
                            !kept
                                || *slot == up
                                || matches.dirty[id as usize]
                                || doc.nodes[id as usize].parent != Some(up as usize),
                            "node {id} moved without a dirty mark"
                        );
                        *slot = if kept { KEPT } else { *slot };
                    }
                }
            }
            None => {
                for &(id, up) in order {
                    let id = id as usize;
                    if placed[id] != up
                        || root(&old.tree, &old.trees, id) != root(&walk.tree, &walk.trees, id)
                    {
                        matches.dirty[id] = true;
                        if let Some(&host) = shadows.hosts.get(&id) {
                            matches.dirty[host] = true;
                        }
                    }
                    placed[id] = KEPT;
                }
                for &(id, _) in old.nodes.iter() {
                    let id = id as usize;
                    if placed.get(id).is_some_and(|&slot| slot != KEPT) {
                        out.leave(id, unstyled, empty);
                    }
                }
            }
        }
    }
}
