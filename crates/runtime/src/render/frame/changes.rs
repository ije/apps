use super::super::{LAYOUT_REVISION, PageState, effects};
use super::Cx;
use super::visibility::walk;
use crate::css;
use crate::css::properties::Prop;
use crate::render::layout::cull;
use crate::style;
use crate::style::ComputedStyles;
use std::hash::Hash;
use std::rc::Rc;
use std::sync::Arc;

// The boxes whose style, children or content changed since the last frame. It gives the flags of each visited
// node, the nodes with a new layout, the nodes that changed when the tree and the focus didn't, and whether a box
// built in place that others read without a record, like a row for its cells.
pub(super) fn detect(
    cx: &Cx,
    dom: &mut PageState,
    styles: &ComputedStyles,
    paint: &[(usize, Rc<css::properties::Style>)],
    info: &mut Vec<cull::Info>,
    fresh: &mut Option<Vec<usize>>,
) -> (Vec<usize>, Option<Vec<usize>>, bool) {
    let (mut changed, mut sources, mut framed) = (Vec::new(), None, false);
    walk(cx.doc, dom, styles);
    let walk = std::mem::take(&mut dom.layout.cull.walk);
    let mut marked = std::mem::take(&mut dom.layout.cull.marked);
    let (mut partial, mut list, mut placing) = (false, Vec::new(), false);
    let rewalked = !walk.holds(cx.walked);
    // Composed children can change the walk of a node without a new revision. A patched walk lists
    // those nodes, another new walk doesn't.
    let touched = walk.since(cx.serial);
    if let Some((latest, focus, computed)) = cx.last.filter(|(.., computed)| {
        (!rewalked
            || touched.is_some()
            || styles.children.is_empty() && computed.children.is_empty())
            && dom.layout.cull.frame == cx.frame
            && info.len() <= cx.doc.nodes.len()
            && dom.layout.cull.parents.len() == cx.doc.nodes.len()
    }) {
        // New nodes have a newer revision, so they get their flags below.
        info.resize(cx.doc.nodes.len(), cull::Info::default());
        match touched {
            Some(touched) => {
                for &id in touched.iter().filter(|&&id| !walk.contains(id as usize)) {
                    info[id as usize] = cull::Info::default();
                }
            }
            None if rewalked => {
                let mut present = vec![false; cx.doc.nodes.len()];
                for &(id, _) in walk.order().iter() {
                    present[id as usize] = true;
                }
                for &(id, _) in cx.walked.iter() {
                    if present.get(id as usize) == Some(&false) {
                        info[id as usize] = cull::Info::default();
                    }
                }
            }
            None => {}
        }
        partial = true;
        marked.resize(cx.doc.nodes.len(), false);
        let parents = &dom.layout.cull.parents;
        let mut mark = |mut id: usize| {
            while !marked[id] {
                marked[id] = true;
                list.push(id);
                match parents[id] {
                    u32::MAX => break,
                    parent => id = parent as usize,
                }
            }
        };
        // Relies on a node getting a new revision when it leaves or joins the tree.
        // A node that moved keeps its flags. One that joined has none yet.
        let changed = walk.changed_since(cx.serial).into_iter().flatten();
        let joined = touched.into_iter().flatten();
        let joined = joined.filter(|&&id| info[id as usize].subtree == 0);
        let patched: Vec<usize> = changed.chain(joined).map(|&id| id as usize).collect();
        let mut newer: Vec<usize> = cx.doc.nodes.changed_since(*latest).collect();
        for &id in newer
            .iter()
            .chain(&patched)
            .filter(|&&id| walk.contains(id))
        {
            mark(id);
        }
        for id in styles.values.differing(&computed.values).flatten() {
            if id < cx.doc.nodes.len()
                && !computed
                    .values
                    .get(id)
                    .is_some_and(|old| Rc::ptr_eq(old, &styles.values[id]))
            {
                mark(id);
                newer.push(id);
            }
        }
        sources = (patched.is_empty() && *focus == dom.interaction.focus).then_some(newer);
        for id in [*focus, dom.interaction.focus]
            .into_iter()
            .flatten()
            .chain(dom.layout.cull.painted.iter().copied())
            .chain(paint.iter().map(|(id, _)| *id))
            .chain(std::mem::take(&mut dom.layout.cull.swapped))
        {
            if id < cx.doc.nodes.len() {
                mark(id);
            }
        }
        for &id in &list {
            info[id] = cull::Info::default();
        }
        placing = list.len() <= 256;
        fresh.get_or_insert_default().extend(&list);
    } else {
        info.clear();
        info.resize(cx.doc.nodes.len(), cull::Info::default());
        *fresh = None;
    }
    dom.layout.cull.painted.clear();
    dom.layout
        .cull
        .painted
        .extend(paint.iter().map(|(id, _)| *id));
    // Compare units too: equal style values can resolve differently after a root font change.
    let units = |lengths: &css::value::units::Lengths| {
        let [font, root] = [lengths.font, lengths.root];
        let mut hasher = rustc_hash::FxHasher::default();
        std::hash::Hash::hash(
            &[
                font.em, font.ch, font.ex, font.cap, font.ic, font.lh, root.em, root.ch, root.ex,
                root.cap, root.ic, root.lh,
            ]
            .map(f32::to_bits),
            &mut hasher,
        );
        std::hash::Hasher::finish(&hasher)
    };
    let mut memo = rustc_hash::FxHashMap::default();
    let mut own_flags = |tag: &str, style: &Rc<css::properties::Style>, id: usize| {
        let kind = if tag.starts_with('#') {
            1
        } else {
            u8::from(cull::replaced(tag)) * 2
        };
        memo.entry((Rc::as_ptr(style), kind))
            .or_insert_with(|| {
                (
                    style.clone(),
                    cull::flags(tag, style, &styles.lengths[id], cx.frame[1]),
                )
            })
            .1
    };
    for (id, style) in paint {
        info[*id].flags = own_flags(&cx.doc.nodes[*id].tag, style, *id)
            & (cull::MOVES_CONTENT | cull::UNCULLABLE);
    }
    // Children come before their parents. A partial pass visits the marked nodes only.
    let mut order: Vec<(u32, usize)> = list
        .iter()
        .filter_map(|&id| Some((walk.place(id)?, id)))
        .collect();
    order.sort_unstable_by(|a, b| b.cmp(a));
    let mut some = order.iter().map(|place| place.1);
    let mut every = walk.order().iter().rev().map(|node| node.0 as usize);
    debug_assert!(
        !partial || some.clone().eq(every.clone().filter(|&id| marked[id])),
        "marked nodes differ"
    );
    let nodes: &mut dyn Iterator<Item = usize> = match partial {
        true => &mut some,
        false => &mut every,
    };
    for id in nodes {
        let node = &cx.doc.nodes[id];
        let style = &styles.values[id];
        let revision = cx.doc.nodes.revision(id);
        let old = dom.layout.layout_nodes[id].as_ref();
        let (shape, attrs) = match old {
            Some(old) if old.seen == revision => {
                debug_assert!(
                    (old.shape, old.attrs) == cull::content(node, style),
                    "node {id} changed without a new revision"
                );
                (old.shape, old.attrs)
            }
            _ => cull::content(node, style),
        };
        let pseudos = styles.pseudos.get(&id);
        let repseudoed = |old: &crate::render::layout::LayoutNode| match (&old.pseudos, pseudos) {
            (Some(old), Some(new)) => !Rc::ptr_eq(old, new),
            (old, new) => old.is_some() != new.is_some(),
        };
        let restyled = old.is_none_or(|old| {
            !Rc::ptr_eq(&old.style, style) || repseudoed(old) || old.shape != shape
        });
        let own = restyled || old.is_some_and(|old| old.attrs != attrs);
        // Pseudo-elements are boxes of the element, and grid items. The ones that change other than in
        // paint, and `true` when they can change the size of the box.
        let repseudo = |old: &crate::render::layout::LayoutNode| {
            if !repseudoed(old) {
                return None;
            }
            let (Some(before), Some(now)) = (&old.pseudos, pseudos) else {
                return Some(true);
            };
            let positioned = |style: &css::properties::Style| {
                style
                    .get(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"))
            };
            (before.iter().zip(now.iter()))
                .filter(|(old, new)| !cull::cosmetic(old, new))
                .map(|(old, new)| !positioned(old) || !positioned(new))
                .reduce(|a, b| a || b)
        };
        let shaped = old.map(|old| {
            let own = old.shape != shape
                || !Rc::ptr_eq(&old.style, style)
                    && (old.units != units(&styles.lengths[id])
                        || !cull::cosmetic(&old.style, style));
            (own, repseudo(old))
        });
        let reshaped = shaped.is_none_or(|(own, pseudo)| own || pseudo.is_some());
        // An absolutely positioned pseudo-element leaves the size of the box and its place as they were.
        let resized = shaped.is_none_or(|(own, pseudo)| own || pseudo == Some(true));
        let (flags, marks) = match old {
            Some(old) if !restyled => (old.flags, old.marks),
            _ => {
                let flags = own_flags(&node.tag, style, id);
                // List every tag with unrecorded sibling, row, counter or quote dependencies.
                let framed = matches!(
                    node.tag.as_str(),
                    "fieldset"
                        | "legend"
                        | "details"
                        | "summary"
                        | "dialog"
                        | "li"
                        | "table"
                        | "caption"
                        | "colgroup"
                        | "col"
                        | "q"
                        | "slot"
                ) || style::table_box(style::role(styles, id))
                    || style::role(styles, id) == style::CAPTION
                    || pseudos.is_some_and(|pair| {
                        pair.iter().any(|pseudo| {
                            pseudo.get(Prop::CONTENT).is_some_and(|value| {
                                value.contains("counter") || value.contains("quote")
                            })
                        })
                    });
                let heighted = pseudos.is_some_and(|pair| pair.iter().any(cull::reads_height));
                (
                    flags,
                    (u32::from(framed) * cull::READS_LAST_FRAME)
                        | (u32::from(heighted) * cull::READS_HEIGHT),
                )
            }
        };
        let (mut list, mut kids) = (
            rustc_hash::FxHasher::default(),
            rustc_hash::FxHasher::default(),
        );
        let (mut out, mut moved, mut below) = (0u64, 0u64, 0u32);
        let (mut positioned, mut any) = (rustc_hash::FxHasher::default(), false);
        for &child in &node.children {
            let child_info = &info[child];
            below |= child_info.flags;
            if child_info.flags & cull::OUT_OF_FLOW != 0 {
                std::hash::Hasher::write_usize(&mut positioned, child);
                any = true;
                // A positioned box that builds in place leaves its parent as it was, when only the
                // boxes in it changed.
                let inside = child_info.flags & cull::HIDDEN == 0
                    && cx.view.outs.borrow().contains_key(&child)
                    && cx.view.kept.borrow().contains_key(&child);
                out = out.wrapping_add(match inside {
                    true => child_info.shell,
                    false => child_info.subtree,
                });
            } else {
                std::hash::Hasher::write_usize(&mut list, child);
                std::hash::Hasher::write_usize(&mut kids, child);
                std::hash::Hasher::write_u64(&mut kids, child_info.subtree);
                moved = moved.max(child_info.revision);
            }
        }
        // A positioned box in a paragraph splits its lines, so it changes the layout when it comes or goes.
        if any
            && node.children.iter().any(|&child| {
                let kid = &cx.doc.nodes[child];
                if kid.tag == "#text" {
                    !kid.text.trim_ascii().is_empty()
                } else {
                    !kid.tag.starts_with('#')
                        && info[child].flags & cull::OUT_OF_FLOW == 0
                        && styles.values[child]
                            .get(Prop::DISPLAY)
                            .is_none_or(|v| v.starts_with("inline"))
                }
            })
        {
            std::hash::Hasher::write_u64(&mut list, std::hash::Hasher::finish(&positioned));
        }
        let (list, kids) = (
            std::hash::Hasher::finish(&list),
            std::hash::Hasher::finish(&kids),
        );
        // A box that only paints otherwise, or whose positioned pseudo-elements changed, builds in place.
        let repainted = placing
            && restyled
            && !resized
            && old.is_some_and(|old| {
                old.attrs == attrs
                    && old.out == out
                    && old.kids == kids
                    && (old.flags, old.marks) == (flags, marks)
            })
            && cx
                .place(
                    styles,
                    &mut dom.layout.measurements.texts,
                    &dom.geometry.mirror,
                    id,
                )
                .is_ok();
        framed |= repainted && (flags | marks) & cull::READS_LAST_FRAME != 0;
        let mut relisted = false;
        if own && !repainted || old.is_some_and(|old| old.out != out || old.kids != kids) {
            relisted = old.is_none_or(|old| old.list != list);
            let geometry = old
                .filter(|old| !resized && !relisted && moved <= old.geometry)
                .map(|old| old.geometry);
            dom.layout.layout_revision =
                LAYOUT_REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            dom.layout.layout_nodes[id] = Some(crate::render::layout::LayoutNode {
                shape,
                attrs,
                seen: revision,
                style: style.clone(),
                pseudos: pseudos.cloned(),
                units: units(&styles.lengths[id]),
                list,
                kids,
                out,
                revision: dom.layout.layout_revision,
                geometry: geometry.unwrap_or(dom.layout.layout_revision),
                shell: old
                    .filter(|_| !own)
                    .map_or(dom.layout.layout_revision, |old| old.shell),
                flags,
                marks,
            });
            changed.push(id);
        } else if let Some(old) = &mut dom.layout.layout_nodes[id] {
            old.seen = revision;
            if repainted {
                old.style = style.clone();
                old.pseudos = pseudos.cloned();
            }
        }
        let layout = dom.layout.layout_nodes[id].as_ref();
        info[id] = cull::Info {
            revision: layout.map_or(0, |node| node.geometry),
            subtree: layout.map_or(0, |node| node.revision),
            shell: layout.map_or(0, |node| node.shell),
            flags: if flags & cull::HIDDEN != 0 {
                flags
            } else {
                info[id].flags
                    | flags
                    | (u32::from(reshaped) * cull::CHANGED)
                    | (u32::from(relisted) * cull::CHILDREN_CHANGED)
                    | marks
                    | (u32::from(
                        dom.interaction.focus == Some(id) || dom.layout.cull.spills.contains(&id),
                    ) * cull::UNCULLABLE)
                    | below
                        & (cull::UNCULLABLE
                            | cull::READS_VIEWPORT
                            | cull::READS_LAST_FRAME
                            | cull::READS_HEIGHT)
            },
        };
    }
    dom.layout.cull.walk = walk;
    for id in list {
        marked[id] = false;
    }
    dom.layout.cull.marked = marked;
    (changed, sources, framed)
}

// Animated boxes take their flags and build in place where they can, and painted values go into `styles` until
// the end of the frame. It gives the number of restored values that move boxes.
pub(super) fn animate(
    cx: &Cx,
    dom: &mut PageState,
    styles: &mut Rc<ComputedStyles>,
    info: &mut [cull::Info],
    fresh: &mut Option<Vec<usize>>,
    paint: Vec<(usize, Rc<css::properties::Style>)>,
    restore: &mut Vec<(usize, Rc<css::properties::Style>)>,
) -> usize {
    for &(id, turns) in cx.composited {
        info[id].flags |= cull::COMPOSITED | (u32::from(turns) * cull::TRANSFORMED);
        fresh.iter_mut().for_each(|fresh| fresh.push(id));
    }
    for &id in cx.pseudo_layers {
        info[id].flags |= cull::COMPOSITED_PSEUDO;
        fresh.iter_mut().for_each(|fresh| fresh.push(id));
    }
    let mut animate = |info: &mut [cull::Info], id: usize| {
        let mut node = Some(id);
        while let Some(id) = node.filter(|&id| id < info.len()) {
            if info[id].flags & cull::ANIMATED != 0 {
                break;
            }
            info[id].flags |= cull::ANIMATED;
            fresh.iter_mut().for_each(|fresh| fresh.push(id));
            node = cx.doc.nodes[id].parent;
        }
    };
    for &(id, _) in cx.composited {
        animate(info, id);
    }
    // A box that can't build in place builds again with its ancestors. It then keeps a record, so the
    // next frames build it in place.
    let mut unplaced = |info: &mut [cull::Info], node: usize| {
        cx.view.repaint.borrow_mut().insert(node);
        if let Some(parent) = cx.doc.nodes[node].parent {
            animate(info, parent);
        }
    };
    let mut placed = std::mem::take(&mut *cx.view.animated.borrow_mut());
    placed.retain(|&id| {
        id < cx.doc.nodes.len()
            && paint.binary_search_by_key(&id, |(id, _)| *id).is_err()
            && cx
                .place(
                    styles,
                    &mut dom.layout.measurements.texts,
                    &dom.geometry.mirror,
                    id,
                )
                .map_err(|node| unplaced(info, node))
                .is_ok()
    });
    for (id, _) in &paint {
        if let Err(node) = cx.place(
            styles,
            &mut dom.layout.measurements.texts,
            &dom.geometry.mirror,
            *id,
        ) {
            unplaced(info, node);
        }
        cx.view.animated.borrow_mut().push(*id);
    }
    // Pseudo-elements that only fade build their element in place too. Others can move its boxes.
    for &(id, _) in cx
        .restore_pseudos
        .iter()
        .filter(|(id, _)| cx.pseudo_layers.contains(id))
    {
        if let Err(node) = cx.place(
            styles,
            &mut dom.layout.measurements.texts,
            &dom.geometry.mirror,
            id,
        ) {
            unplaced(info, node);
        }
        cx.view.animated.borrow_mut().push(id);
    }
    for &(id, _) in cx
        .restore_pseudos
        .iter()
        .filter(|(id, _)| !cx.pseudo_layers.contains(id))
    {
        animate(info, id);
    }
    placed.extend(cx.view.animated.borrow().iter());
    let mut recolored = std::mem::take(&mut dom.styles.recolored);
    // The values that move boxes. `cx.view.animated` builds the boxes of painted values in place next frame.
    let relayouts = restore.len();
    if !paint.is_empty() {
        let values = &mut Rc::make_mut(styles).values;
        for (id, style) in paint {
            if style.get(Prop::COLOR) != values[id].get(Prop::COLOR) {
                dom.styles.recolored.push(id);
            }
            restore.push((id, std::mem::replace(&mut values[id], style)));
        }
    }
    let establishes = |id: usize| cx.shown(id) && effects::establishes(&styles.values[id]).absolute;
    if !cx.same {
        dom.layout.blocks.retain(|&id, _| establishes(id));
    } else {
        // Only animated boxes, now or in the last frame, can stop being containing blocks.
        let ids = restore
            .iter()
            .map(|(id, _)| *id)
            .chain(dom.styles.animating.keys().copied());
        for id in ids.collect::<Vec<_>>() {
            if !establishes(id) {
                dom.layout.blocks.remove(&id);
            }
        }
    }
    recolored.extend(&dom.styles.recolored);
    placed.sort_unstable();
    recolored.retain(|id| placed.binary_search(id).is_err());
    for &id in &recolored {
        animate(info, id);
    }
    for id in recolored {
        let mut node = Some(id).filter(|&id| id < styles.values.len());
        while let Some(id) = node {
            dom.layout.measurements.texts.remove(&id);
            node = cx.doc.nodes[id].parent.filter(|_| {
                styles.values[id]
                    .get(Prop::DISPLAY)
                    .is_none_or(|value| matches!(value, "inline" | "contents"))
            });
        }
    }
    relayouts
}

// Measurements of boxes that a new frame size, a container query pass or a new layout changed go.
pub(super) fn remeasure(
    cx: &Cx,
    dom: &mut PageState,
    styles: &ComputedStyles,
    changed: &[usize],
    info: &mut [cull::Info],
    fresh: &mut Option<Vec<usize>>,
) {
    dom.layout.measurements.noted.clear();
    if dom.layout.cull.frame != cx.frame {
        dom.layout.measurements.retain(|_| false);
        // Text keeps the fonts that it resolved, so a font load must build it again.
        dom.layout.measurements.texts.clear();
        dom.layout.grids.clear();
        cx.view.walked.borrow_mut().clear();
        cx.view.adjoined.borrow_mut().clear();
    } else if dom.layout.container_pass && !dom.styles.cache.units.is_empty() {
        // Container units change the boxes in the resized containers, and the sizes of the boxes around them.
        let mut nodes = rustc_hash::FxHashSet::default();
        for &container in &dom.styles.cache.units {
            let mut node = dom.tree.flat_parent(cx.doc, &styles.children, container);
            while let Some(id) = node.filter(|&id| nodes.insert(id)) {
                node = dom.tree.flat_parent(cx.doc, &styles.children, id);
            }
            let mut pending = vec![container];
            while let Some(id) = pending.pop() {
                if id < cx.doc.nodes.len() && nodes.insert(id) {
                    pending.extend(&cx.doc.nodes[id].children);
                    pending.extend(dom.tree.shadows.get(&id));
                }
            }
        }
        let measured = &mut dom.layout.measurements;
        measured.retain(|id| !nodes.contains(&id));
        let (mut walked, mut adjoined) =
            (cx.view.walked.borrow_mut(), cx.view.adjoined.borrow_mut());
        for id in &nodes {
            measured.texts.remove(id);
            dom.layout.grids.remove(id);
            walked.remove(id);
            adjoined.remove(id);
        }
    }
    let mut stale = false;
    let mut around = rustc_hash::FxHashSet::default();
    let mut regridded = rustc_hash::FxHashSet::default();
    for &id in changed {
        dom.layout.measurements.texts.remove(&id);
        if dom.layout.layout_nodes[id]
            .as_ref()
            .is_some_and(|node| node.geometry == node.revision)
        {
            let mut node = Some(id);
            while let Some(id) = node.filter(|&id| around.insert(id)) {
                info[id].flags |= cull::GEOMETRY_CHANGED;
                fresh.iter_mut().for_each(|fresh| fresh.push(id));
                node = dom.tree.flat_parent(cx.doc, &styles.children, id);
            }
            // Grid tracks leave out absolutely positioned items, so the grids above one keep their layout.
            let mut node = Some(id);
            while let Some(id) = node.filter(|&id| regridded.insert(id)) {
                dom.layout.grids.remove(&id);
                node = (info[id].flags & cull::OUT_OF_FLOW == 0)
                    .then(|| dom.tree.flat_parent(cx.doc, &styles.children, id))
                    .flatten();
            }
            stale = true;
        }
    }
    if stale {
        dom.layout.measurements.retain(|id| {
            info.get(id)
                .is_none_or(|info| info.flags & cull::GEOMETRY_CHANGED == 0)
        });
    }
    dom.layout.text_layouts.clear();
}

// Images that load or decode later, and the selection, change paint without a new box.
pub(super) fn repaint(state: &mut PageState, window: &mut renderer::Window) {
    let hash = |value: &dyn Fn(&mut rustc_hash::FxHasher)| {
        let mut hasher = rustc_hash::FxHasher::default();
        value(&mut hasher);
        std::hash::Hasher::finish(&hasher)
    };
    let images = (state.media.images.iter())
        .map(|(id, image)| hash(&|h| (id, Arc::as_ptr(image)).hash(h)))
        .chain(
            state
                .rasters
                .bitmaps
                .values()
                .map(|image| Arc::as_ptr(image) as u64),
        )
        .chain((state.rasters.backgrounds.values()).map(|(_, image)| Arc::as_ptr(image) as u64))
        .chain(
            state
                .rasters
                .layer_sources
                .values()
                .map(|image| Arc::as_ptr(image) as u64),
        )
        .chain(
            (state.media.svg_images.iter())
                .map(|(id, (source, _))| hash(&|h| (id, Rc::as_ptr(source)).hash(h))),
        )
        .fold(0u64, u64::wrapping_add);
    let selection = (state.interaction.selection.iter())
        .map(|entry| hash(&|h| entry.hash(h)))
        .fold(0u64, u64::wrapping_add);
    let inputs = hash(&|h| (images, selection, state.interaction.selection_caret).hash(h));
    if std::mem::replace(&mut state.layout.paint_inputs, inputs) != inputs {
        window.repaint();
    }
}
