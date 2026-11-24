use super::super::element::{self, Build, element};
use super::super::layout::{self, cull};
use crate::css;
use renderer::Window;
#[cfg(debug_assertions)]
use std::{cell::Cell, rc::Rc};

// Fields whose value changed, and positioned boxes whose parents stayed, build again on their own.
#[inline(never)]
pub(crate) fn patch(build: &Build, window: &mut Window) {
    let view = build.view;
    let changed: Vec<usize> = view
        .patches
        .borrow()
        .iter()
        .filter(|&(&id, patch)| {
            patch.value != element::kept::hashed(&build.doc.nodes[id].value)
                && build.tree.contains(id)
        })
        .map(|(&id, _)| id)
        .collect();
    for id in changed {
        let Some(patch) = view.patches.borrow_mut().remove(&id) else {
            continue;
        };
        let old = view.kept.borrow_mut().remove(&id);
        let mark = build.reads.borrow().len();
        let field = Build {
            ancestors: patch.ancestors,
            flex_parent: patch.flex_parent,
            columned: patch.columned,
            perspective: patch.perspective,
            zone: None,
            #[cfg(debug_assertions)]
            extra: Default::default(),
            #[cfg(debug_assertions)]
            plan: Default::default(),
            ..*build
        };
        #[cfg(debug_assertions)]
        let printed = field.prints.borrow().len();
        let bounds = patch.slot.as_ref().map(|(bounds, _)| *bounds);
        let child = element(&field, id, patch.containing, patch.container, patch.slot);
        let child = match bounds {
            Some(bounds) => {
                let mut units = build.styles.lengths[id];
                units.container = Some(patch.container);
                let margin = units.edges(
                    &build.styles.values[id],
                    css::properties::MARGIN,
                    patch.containing[0],
                );
                layout::size::place(child, margin, field.pixel_ratio, bounds)
            }
            None => child,
        };
        #[cfg(debug_assertions)]
        {
            if let (Some(old), Some(new)) = (&old, view.kept.borrow().get(&id)) {
                assert!(
                    old.call == new.call && old.reads == new.reads && old.print == new.print,
                    "patched field {id} builds otherwise"
                );
            }
            field.prints.borrow_mut().truncate(printed);
        }
        #[cfg(not(debug_assertions))]
        drop(old);
        build.reads.borrow_mut().truncate(mark);
        if let renderer::Child::Node(node) = child {
            window.patch(*node);
        }
    }
    // Positioned boxes whose parents stayed because only the boxes in them changed. Outside in, so a box
    // that builds inside another one's build is skipped.
    let mut stale: Vec<(usize, usize)> = {
        let (outs, kept) = (view.outs.borrow(), view.kept.borrow());
        (outs.iter())
            .filter(|&(&id, _)| {
                build.tree.contains(id)
                    && build.info[id].flags & (cull::OUT_OF_FLOW | cull::HIDDEN)
                        == cull::OUT_OF_FLOW
                    && kept
                        .get(&id)
                        .is_some_and(|record| record.subtree() != build.info[id].subtree)
            })
            .map(|(&id, patch)| (patch.ancestors.len(), id))
            .collect()
    };
    stale.sort_unstable();
    for (_, id) in stale {
        // A positioned box around it built it already.
        if view
            .kept
            .borrow()
            .get(&id)
            .is_none_or(|kept| kept.subtree() == build.info[id].subtree)
        {
            continue;
        }
        let (Some(patch), Some(old)) = (
            view.outs.borrow_mut().remove(&id),
            view.kept.borrow_mut().remove(&id),
        ) else {
            continue;
        };
        let mark = build.reads.borrow().len();
        let inner = Build {
            ancestors: patch.ancestors.clone(),
            flex_parent: patch.flex_parent,
            columned: patch.columned,
            perspective: patch.perspective,
            zone: None,
            #[cfg(debug_assertions)]
            extra: Default::default(),
            #[cfg(debug_assertions)]
            plan: Default::default(),
            ..*build
        };
        #[cfg(debug_assertions)]
        let printed = inner.prints.borrow().len();
        let bounds = patch.slot.as_ref().map(|(bounds, _)| *bounds);
        let child = element(&inner, id, patch.containing, patch.container, patch.slot);
        let child = match bounds {
            Some(bounds) => {
                let mut units = build.styles.lengths[id];
                units.container = Some(patch.container);
                let margin = units.edges(
                    &build.styles.values[id],
                    css::properties::MARGIN,
                    patch.containing[0],
                );
                layout::size::place(child, margin, inner.pixel_ratio, bounds)
            }
            None => child,
        };
        #[cfg(debug_assertions)]
        {
            inner.prints.borrow_mut().truncate(printed);
            if let Some(new) = view.kept.borrow().get(&id) {
                new.seen.set(old.seen.get());
            }
        }
        build.reads.borrow_mut().truncate(mark);
        // The records of the boxes around hold the reads of the boxes in this one, so other reads build
        // them again.
        if view
            .kept
            .borrow()
            .get(&id)
            .is_none_or(|new| !new.same_reads(&old))
        {
            let mut kept = view.kept.borrow_mut();
            for ancestor in patch.ancestors.iter() {
                kept.remove(ancestor);
            }
        }
        if let renderer::Child::Node(node) = child {
            window.patch(*node);
        }
    }
}

// Boxes that only paint otherwise build again in place.
pub(crate) fn place(
    build: &Build,
    window: &mut Window,
    #[cfg(debug_assertions)] zone: Option<Rc<cull::Zone>>,
    #[cfg(debug_assertions)] size: [f32; 2],
) {
    let view = build.view;
    // Build in place from the outside in, so a box already built inside another one's build is skipped.
    let mut places: Vec<(usize, usize)> = {
        let (kept, built) = (view.kept.borrow(), view.built.take());
        view.repaint
            .borrow()
            .iter()
            .filter(|&&id| build.tree.contains(id))
            .filter_map(|&id| {
                let ancestors = kept.get(&id)?.in_place()?;
                (!ancestors
                    .last()
                    .is_some_and(|parent| built.contains(parent)))
                .then_some((ancestors.len(), id))
            })
            .collect()
    };
    places.sort_unstable();
    let mut placed = Vec::new();
    for (_, id) in places {
        if !view.repaint.borrow().contains(&id) {
            continue;
        }
        if let Some(node) = element::repaint(build, id) {
            placed.push(node);
        }
    }
    view.repaint.borrow_mut().clear();
    view.built.borrow_mut().clear();
    #[cfg(debug_assertions)]
    if !placed.is_empty() {
        let records = std::mem::take(&mut *view.kept.borrow_mut());
        let (printed, mark) = (build.prints.borrow().len(), build.reads.borrow().len());
        let culls = Cell::new(0);
        let full = Build {
            ancestors: build.ancestors.clone(),
            perspective: build.perspective.clone(),
            zone,
            culls: &culls,
            checking: true,
            extra: Default::default(),
            plan: Default::default(),
            ..*build
        };
        let fresh = element(&full, 0, size, size, None);
        build.prints.borrow_mut().truncate(printed);
        build.reads.borrow_mut().truncate(mark);
        element::oracle::same_in_place(&fresh, &placed, build.tree);
        drop(records);
    }
    #[cfg(debug_assertions)]
    view.around.borrow_mut().clear();
    for node in placed {
        window.patch(node);
    }
}
