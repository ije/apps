use super::super::layout::cull;
use super::super::paint::{background, spacer};
use super::super::{PageState, ViewState, layout};
use super::Cx;
use crate::css;
use crate::css::properties::Prop;
use crate::dom::Document;
use crate::style;
use crate::style::ComputedStyles;
use std::{rc::Rc, sync::Arc};

pub(super) fn in_view(
    bounds: &crate::dom::LayoutMirror,
    [width, height]: [f32; 2],
    id: usize,
    mut clips: impl Iterator<Item = (usize, [bool; 2])>,
) -> bool {
    let Some(&[x, y, w, h]) = bounds.get(&id).filter(|rect| rect[2] > 0. && rect[3] > 0.) else {
        return true;
    };
    let pad = w.max(h) / 2.;
    let [left, top, right, bottom] = [x - pad, y - pad, x + w + pad, y + h + pad];
    let overlaps = |[a, b, c, d]: [f32; 4], [horizontal, vertical]: [bool; 2]| {
        (!horizontal || left.max(a) < right.min(a + c))
            && (!vertical || top.max(b) < bottom.min(b + d))
    };
    overlaps([0., 0., width, height], [true; 2])
        && clips.all(|(clip, axes)| bounds.get(&clip).is_none_or(|&rect| overlaps(rect, axes)))
}

pub(super) fn clips<'a>(
    doc: &'a crate::dom::Document,
    styles: &'a style::ComputedStyles,
    id: usize,
) -> impl Iterator<Item = (usize, [bool; 2])> + 'a {
    let mut node = Some(id);
    std::iter::from_fn(move || {
        loop {
            let child = node?;
            node = doc.nodes[child].parent.filter(|_| {
                !matches!(
                    styles.values[child].get(Prop::POSITION),
                    Some("absolute" | "fixed")
                )
            });
            let parent = node?;
            let style = &styles.values[parent];
            let axes = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                .map(|prop| style.get(prop).is_some_and(|value| value != "visible"));
            if axes != [false; 2]
                && !matches!(doc.nodes[parent].tag.as_str(), "html" | "body")
                && style
                    .get(Prop::DISPLAY)
                    .is_some_and(|value| !matches!(value, "inline" | "contents"))
            {
                return Some((parent, axes));
            }
        }
    })
}

// Returns whether the walk changed by a patch that kept the lists of cells, canvases and iframes.
pub(super) fn walk(
    doc: &crate::dom::Document,
    dom: &mut crate::render::PageState,
    styles: &style::ComputedStyles,
) -> bool {
    let serial = dom.layout.cull.walk.serial;
    let cull = &mut dom.layout.cull;
    if !cull
        .walk
        .update(doc, &styles.children, Some(&dom.styles.walk))
    {
        if cull.parents.len() < doc.nodes.len() {
            cull.parents.resize(doc.nodes.len(), u32::MAX);
            cull.kinds.resize(doc.nodes.len(), 0);
        }
        return false;
    }
    let kind = |id: usize| match doc.nodes[id].tag.as_str() {
        // Rows show when anything in them shows, as an anonymous cell can hold text or a block.
        _ if style::role(styles, id) == style::CELL
            || doc.nodes[id].parent.is_some_and(|row| {
                (style::ROW..=style::FOOTER).contains(&style::role(styles, row))
            }) =>
        {
            1
        }
        "canvas" => 2,
        "iframe" => 3,
        _ => 0,
    };
    let patched = cull.parents.len() == cull.kinds.len()
        && cull.kinds.len() <= doc.nodes.len()
        && cull.walk.since(serial).is_some_and(|touched| {
            touched.iter().all(|&id| {
                let id = id as usize;
                cull.kinds.get(id).is_none_or(|&old| old == 0)
                    && (!cull.walk.contains(id) || kind(id) == 0)
            })
        });
    if patched {
        cull.parents.resize(doc.nodes.len(), u32::MAX);
        cull.kinds.resize(doc.nodes.len(), 0);
        for &id in cull.walk.since(serial).into_iter().flatten() {
            cull.parents[id as usize] = cull.walk.parent(id as usize).unwrap_or(u32::MAX);
        }
    }
    if patched && !cfg!(debug_assertions) {
        return true;
    }
    let mut parents = vec![u32::MAX; doc.nodes.len()];
    let mut kinds = vec![0; doc.nodes.len()];
    let mut lists = [const { Vec::new() }; 3];
    for &(id, parent) in cull.walk.order().iter() {
        let id = id as usize;
        parents[id] = parent;
        kinds[id] = kind(id);
        if kinds[id] != 0 {
            lists[usize::from(kinds[id]) - 1].push(id);
        }
    }
    let [cells, canvases, iframes] = lists;
    assert!(
        !patched
            || (&parents, &kinds, &cells, &canvases, &iframes)
                == (
                    &cull.parents,
                    &cull.kinds,
                    &cull.cells,
                    &cull.canvases,
                    &cull.iframes
                ),
        "patched walk lists differ"
    );
    (cull.parents, cull.kinds) = (parents, kinds);
    (cull.cells, cull.canvases, cull.iframes) = (cells, canvases, iframes);
    patched
}

fn visibility(
    doc: &crate::dom::Document,
    dom: &crate::render::PageState,
    styles: &style::ComputedStyles,
) -> Vec<bool> {
    let mut visible = vec![false; doc.nodes.len()];
    for &(node, parent) in dom.layout.cull.walk.order().iter() {
        let node = node as usize;
        if parent != u32::MAX
            && (!visible[parent as usize]
                || cull::skipping(doc, &dom.layout, styles, parent as usize, true))
        {
            continue;
        }
        let style = &styles.values[node];
        visible[node] = !match dom.layout.layout_nodes.get(node).and_then(Option::as_ref) {
            Some(old) if Rc::ptr_eq(&old.style, style) => old.flags & cull::HIDDEN != 0,
            _ => style
                .get(Prop::DISPLAY)
                .is_some_and(|value| value == "none"),
        };
    }
    for &node in &dom.layout.cull.cells {
        if visible[node] {
            let mut parent = doc.nodes[node].parent;
            while let Some(id) = parent.filter(|&id| {
                !visible[id] && (style::ROW..=style::FOOTER).contains(&style::role(styles, id))
            }) {
                visible[id] = true;
                parent = doc.nodes[id].parent;
            }
        }
    }
    visible
}

pub(super) fn drop_hidden_images(
    dom: &mut crate::render::PageState,
    visible: &[bool],
    styles: &style::ComputedStyles,
    mut release: impl FnMut(Arc<renderer::RenderImage>),
) {
    dom.rasters.svgs.retain(|id, (_, image)| {
        visible.get(*id) == Some(&true) || {
            if Arc::strong_count(image) == 1 {
                release(image.clone());
            }
            false
        }
    });
    dom.rasters.masks.retain(|&(id, slot), (_, image)| {
        let masked = |style: &css::properties::Style, props: &[Prop]| {
            props
                .iter()
                .find_map(|&prop| style.get(prop))
                .is_some_and(|value| !value.is_empty() && value != "none")
        };
        let images = [Prop::MASK_IMAGE, Prop::WEBKIT_MASK_IMAGE];
        visible.get(id) == Some(&true)
            && match slot {
                // A clip path shape also makes a mask of the box.
                0 => styles.values.get(id).is_some_and(|style| {
                    masked(style, &images)
                        || masked(style, &[Prop::CLIP_PATH, Prop::WEBKIT_CLIP_PATH])
                }),
                _ => styles
                    .pseudos
                    .get(&id)
                    .and_then(|pseudos| pseudos.get(usize::from(slot) - 1))
                    .is_some_and(|style| masked(style, &images)),
            }
            || {
                if let Some(image) = image {
                    release(image.clone());
                }
                false
            }
    });
    for (id, canvas) in &dom.media.canvases {
        if canvas.present && visible.get(*id) != Some(&true) {
            release(canvas.image.clone());
        }
    }
    // Control rasters use layer usize::MAX minus control kind.
    dom.rasters.backgrounds.retain(|&(id, layer), (_, image)| {
        let keep = if layer > usize::MAX - 64 {
            visible.get(id) == Some(&true)
                && match layer.checked_sub(background::BORDER_IMAGE) {
                    Some(slot) if slot < 8 => match slot {
                        0 => styles.values.get(id).map(|style| &**style),
                        _ => styles
                            .pseudos
                            .get(&id)
                            .and_then(|pseudos| pseudos.get(slot - 1)),
                    }
                    .and_then(|style| style.get(Prop::BORDER_IMAGE_SOURCE))
                    .is_some_and(|value| value != "none"),
                    _ => true,
                }
        } else {
            visible.get(id) == Some(&true)
                && match layer / 64 {
                    0 => styles.values.get(id).map(|style| &**style),
                    3 => styles.backdrops.get(&id).map(|style| &**style),
                    slot => styles
                        .pseudos
                        .get(&id)
                        .and_then(|pseudos| pseudos.get(slot - 1)),
                }
                .and_then(|style| style.get(Prop::BACKGROUND_IMAGE))
                .is_some_and(|value| value != "none")
        };
        if !keep && Arc::strong_count(image) == 1 {
            release(image.clone());
        }
        keep
    });
    let backgrounds = &dom.rasters.backgrounds;
    dom.rasters
        .layer_sources
        .retain(|key, _| backgrounds.contains_key(key));
    dom.rasters.bitmaps.retain(|_, image| {
        Arc::strong_count(image) > 1 || {
            release(image.clone());
            false
        }
    });
}

// Boxes forced to show their contents for a script skip them again in the next frame.
pub(super) fn wake(
    view: &Rc<ViewState>,
    doc: &Document,
    dom: &mut PageState,
    styles: &ComputedStyles,
) {
    let mut waking = std::mem::take(&mut dom.layout.measurements.waking);
    if !view.reflowing.get() {
        waking.extend(dom.layout.measurements.forced.drain());
    }
    if !waking.is_empty() {
        let mut nodes = rustc_hash::FxHashSet::default();
        for id in waking {
            let path: Vec<_> = std::iter::successors(Some(id), |&node| {
                dom.tree.flat_parent(doc, &styles.children, node)
            })
            .collect();
            for node in path {
                if let Some(info) = dom.layout.cull.info.get_mut(node) {
                    info.flags &= !cull::SHOWN;
                }
                nodes.insert(node);
            }
            // The boxes inside kept records of the other state, like the boxes of nested `auto` boxes.
            let mut pending = doc
                .nodes
                .get(id)
                .map_or_else(Vec::new, |node| node.children.clone());
            while let Some(node) = pending.pop() {
                nodes.insert(node);
                pending.extend(&doc.nodes[node].children);
                pending.extend(dom.tree.shadows.get(&node));
            }
        }
        spacer::reskip(view, dom, &nodes);
    }
}

// The nodes that show, the list of the last frame when all nodes were visited again, and the nodes that a
// patched walk showed or hid.
pub(super) fn update(
    doc: &Document,
    dom: &mut PageState,
    styles: &ComputedStyles,
    last: Option<&(u64, Option<usize>, Rc<ComputedStyles>)>,
    same: bool,
    walked: &Rc<Vec<(u32, u32)>>,
    serial: u64,
) -> (Vec<bool>, Option<Vec<bool>>, Vec<usize>) {
    let mut visible = std::mem::take(&mut dom.layout.cull.visible);
    let mut previous = None;
    let patched = !same && walk(doc, dom, styles);
    let none = |style: &Rc<css::properties::Style>| style.get(Prop::DISPLAY) == Some("none");
    let skips = |id: usize, style: &Rc<css::properties::Style>| {
        let value = style.get(Prop::CONTENT_VISIBILITY);
        let tag = doc.nodes.get(id).map_or("", |node| node.tag.as_str());
        (
            cull::skippable(tag, style),
            value == Some("hidden"),
            value == Some("auto"),
        )
    };
    let rewalked = !dom.layout.cull.walk.holds(walked);
    // A patched walk changes the visibility of the nodes it touched only. Table cells also show their rows,
    // so their lists must be the same.
    let touched = dom.layout.cull.walk.since(serial).filter(|_| patched);
    // A `content-visibility: auto` box that shows or skips its contents shows or hides all nodes in it.
    let steady = !std::mem::take(&mut dom.layout.cull.reskipped)
        && (same
            || dom.layout.cull.cascaded
                && (!rewalked || touched.is_some())
                && last.as_ref().is_some_and(|(.., computed)| {
                    styles
                        .values
                        .differing(&computed.values)
                        .flatten()
                        .all(|id| {
                            computed.values.get(id).is_none_or(|old| {
                                styles.values.get(id).is_some_and(|new| {
                                    none(old) == none(new) && skips(id, old) == skips(id, new)
                                })
                            })
                        })
                }));
    // The nodes that a patched walk showed or hid.
    let mut flips = Vec::new();
    if steady {
        let start = visible.len();
        if start < doc.nodes.len() {
            visible.resize(doc.nodes.len(), false);
            dom.geometry
                .mirror
                .hide(&visible, &[], start..visible.len());
        }
        let walk = &dom.layout.cull.walk;
        let mut placed: Vec<(u32, u32)> = Vec::new();
        for &id in touched.into_iter().flatten() {
            match walk.place(id as usize) {
                Some(at) => placed.push((at, id)),
                None if visible[id as usize] => {
                    visible[id as usize] = false;
                    flips.push(id as usize);
                }
                None => {}
            }
        }
        placed.sort_unstable();
        placed.dedup();
        for (at, id) in placed {
            let (id, parent) = (id as usize, walk.order()[at as usize].1);
            let style = &styles.values[id];
            let shown = (parent == u32::MAX
                || visible[parent as usize]
                    && !cull::skipping(doc, &dom.layout, styles, parent as usize, true))
                && !match dom.layout.layout_nodes.get(id).and_then(Option::as_ref) {
                    Some(old) if Rc::ptr_eq(&old.style, style) => old.flags & cull::HIDDEN != 0,
                    _ => none(style),
                };
            if visible[id] != shown {
                visible[id] = shown;
                flips.push(id);
            }
        }
        for &id in &flips {
            dom.geometry.mirror.show(id, visible[id]);
        }
    } else {
        previous = Some(std::mem::replace(
            &mut visible,
            visibility(doc, dom, styles),
        ));
        dom.layout.cull.cascaded = true;
    }
    #[cfg(debug_assertions)]
    if steady && !same {
        assert!(
            visible == visibility(doc, dom, styles),
            "kept visibility differs"
        );
    }
    (visible, previous, flips)
}

// Clear node-ID caches for hidden or removed nodes here, and freed nodes in dom.release. New nodes
// can reuse their IDs.
pub(super) fn forget(
    cx: &Cx,
    dom: &mut PageState,
    styles: &ComputedStyles,
    flips: Vec<usize>,
    flipped: bool,
    window: &mut renderer::Window,
) {
    drop_hidden_images(dom, cx.visible, styles, |image| window.release_image(image));
    cx.view
        .mask_sources
        .borrow_mut()
        .retain(|key, _| dom.rasters.masks.contains_key(key));
    for id in flips.into_iter().filter(|&id| !cx.shown(id)) {
        cx.view.svgs.borrow_mut().remove(&id);
        cx.view.kept.borrow_mut().remove(&id);
        cx.view.patches.borrow_mut().remove(&id);
        cx.view.outs.borrow_mut().remove(&id);
        cx.view.walked.borrow_mut().remove(&id);
        cx.view.adjoined.borrow_mut().remove(&id);
        cx.view.fields.borrow_mut().remove(&id);
        cx.view
            .decoding
            .borrow_mut()
            .retain(|_, (node, _)| *node != id);
        dom.layout.containers.remove(&id);
        dom.layout.flexed_grids.remove(&id);
        dom.layout.grid_widest.remove(&id);
        dom.layout.tracks.remove(&id);
        dom.layout.areas.remove(&id);
        dom.layout.anchors.remove(&id);
        dom.layout
            .anchors
            .remove(&crate::render::inline::atom_key(id, 2));
    }
    if flipped {
        cx.view.svgs.borrow_mut().retain(|&id, _| cx.shown(id));
        cx.view.kept.borrow_mut().retain(|&id, _| cx.shown(id));
        cx.view.patches.borrow_mut().retain(|&id, _| cx.shown(id));
        cx.view.outs.borrow_mut().retain(|&id, _| cx.shown(id));
        cx.view.walked.borrow_mut().retain(|&id, _| cx.shown(id));
        cx.view.adjoined.borrow_mut().retain(|&id, _| cx.shown(id));
        cx.view.fields.borrow_mut().retain(|&id, _| cx.shown(id));
        cx.view
            .decoding
            .borrow_mut()
            .retain(|_, (id, _)| cx.shown(*id));
        dom.layout.containers.retain(|&id, _| cx.shown(id));
        dom.layout.flexed_grids.retain(|&id, _| cx.shown(id));
        dom.layout.grid_widest.retain(|&id, _| cx.shown(id));
        dom.layout.tracks.retain(|&id, _| cx.shown(id));
        dom.layout.areas.retain(|&id, _| cx.shown(id));
        // Pseudo-elements keep their anchors by `atom_key`.
        dom.layout
            .anchors
            .retain(|&id, _| cx.shown(id & ((1 << 30) - 1)));
    }
    {
        let mut reach = cx.view.reach.borrow_mut();
        if reach.len() > cx.doc.nodes.len() {
            reach.truncate(cx.doc.nodes.len());
            reach.shrink_to_fit();
        }
    }
    // The boxes around an `<svg>` whose spill changed build again, so spacers follow its flags.
    for id in std::mem::take(&mut dom.layout.cull.respilled) {
        let mut kept = cx.view.kept.borrow_mut();
        let mut node = Some(id).filter(|&id| id < cx.doc.nodes.len());
        while let Some(id) = node {
            kept.remove(&id);
            node = dom.tree.flat_parent(cx.doc, &styles.children, id);
        }
    }
    if cx.view.focused.replace(dom.interaction.focus) != dom.interaction.focus
        && let Some(focus) = dom.interaction.focus.filter(|&id| id < cx.doc.nodes.len())
    {
        let mut kept = cx.view.kept.borrow_mut();
        let mut node = Some(focus);
        while let Some(id) = node {
            kept.remove(&id);
            node = dom.tree.flat_parent(cx.doc, &styles.children, id);
        }
    }
    if !cx.same {
        let unscrolled: Vec<usize> = dom
            .geometry
            .scroll
            .keys()
            .copied()
            .filter(|&id| {
                cx.shown(id)
                    && id != 0
                    && !matches!(cx.doc.nodes[id].tag.as_str(), "html" | "body")
                    && !layout::size::text_field(&cx.doc.nodes[id], &styles.values[id])
                    && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                        .into_iter()
                        .all(|prop| {
                            styles.values[id]
                                .get(prop)
                                .is_none_or(|value| matches!(value, "visible" | "clip"))
                        })
            })
            .collect();
        // Pages read offsets from the layout mirror, which only handles write.
        for id in unscrolled {
            dom.geometry.scroll.remove(&id);
            dom.geometry.mirror.unscroll(id);
        }
    }
}

impl ViewState {
    // Parked animations come back when their boxes come into view.
    pub(crate) fn wake_parked(&self, window: &mut renderer::Window) {
        let parked = self.parked.take();
        let size = window.viewport_size();
        if !parked.is_empty()
            && self.store.try_borrow().is_ok_and(|store| {
                store.page_state(self.id).is_some_and(|dom| {
                    parked.iter().any(|(id, clips)| {
                        in_view(
                            &dom.geometry.mirror,
                            [f32::from(size.width), f32::from(size.height)],
                            *id,
                            clips.iter().copied(),
                        )
                    })
                })
            })
        {
            window.request_animation_frame();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::raster;
    use super::*;

    #[test]
    fn boxes_out_of_view_or_in_closed_clips_are_not_in_view() {
        let mut doc = crate::dom::Document::parse(
            "<div id=panel style='height:0;overflow:hidden'><i id=a></i><i id=b style='position:absolute'></i></div><b id=c></b><span style='overflow:hidden'><b id=d></b></span>",
        );
        let styles = crate::render::PageState::default().restyle(&mut doc, [800., 600.]);
        let id = |name: &str| {
            (0..doc.nodes.len())
                .find(|&id| {
                    doc.nodes[id]
                        .attrs
                        .get("id")
                        .is_some_and(|value| value == name)
                })
                .unwrap()
        };
        let mut bounds = crate::dom::LayoutMirror::default();
        let check = |bounds: &crate::dom::LayoutMirror, name| {
            in_view(
                bounds,
                [800., 600.],
                id(name),
                clips(&doc, &styles, id(name)),
            )
        };
        assert!(check(&bounds, "a"));
        bounds.insert(id("panel"), [0., 0., 800., 0.]);
        for name in ["a", "b", "c", "d"] {
            bounds.insert(id(name), [10., 10., 16., 16.]);
        }
        assert_eq!(
            ["a", "b", "c", "d"].map(|name| check(&bounds, name)),
            [false, true, true, true]
        );
        bounds.insert(id("panel"), [0., 0., 800., 40.]);
        bounds.insert(id("c"), [10., 620., 16., 16.]);
        assert!(check(&bounds, "a") && !check(&bounds, "c"));
        bounds.insert(id("c"), [10., 600., 16., 0.]);
        assert!(check(&bounds, "c"));
    }

    #[test]
    fn hidden_nodes_release_textures_but_canvases_keep_pixels() {
        let mut doc = crate::dom::Document::parse(
            "<style>p{background-image:linear-gradient(red,blue)}</style><canvas></canvas><p></p><p></p>",
        );
        let mut dom = crate::render::PageState::default();
        let canvas = doc
            .nodes
            .iter()
            .position(|node| node.tag == "canvas")
            .unwrap();
        let paragraphs: Vec<usize> = (0..doc.nodes.len())
            .filter(|&id| doc.nodes[id].tag == "p")
            .collect();
        let [first, second] = [paragraphs[0], paragraphs[1]];
        let styles = dom.restyle(&mut doc, [800., 600.]);
        let mut drawn = raster::canvas::Canvas::new(2, 2).unwrap();
        drawn
            .draw(
                &[4., 7., 21., 0., 0., 2., 2.],
                "#ff0000",
                &[],
                &|_| None,
                None,
            )
            .unwrap();
        dom.media.canvases.insert(canvas, drawn);
        let image = |size: u32| {
            Arc::new(renderer::RenderImage::new(
                size,
                size,
                vec![0; (size * size * 4) as usize],
            ))
        };
        let shared = image(1);
        dom.rasters
            .backgrounds
            .insert((first, 0), ("a".into(), shared.clone()));
        dom.rasters
            .backgrounds
            .insert((second, 0), ("a".into(), shared.clone()));
        dom.rasters.bitmaps.insert("a".into(), shared);
        dom.rasters
            .backgrounds
            .insert((second, 64), ("b".into(), image(2)));
        // A tiled layer draws a raster of its own, and keeps its decoded source.
        let tile = image(3);
        dom.rasters.bitmaps.insert("t".into(), tile.clone());
        dom.rasters.layer_sources.insert((second, 0), tile);
        let mut released = Vec::new();
        let mut visible = vec![false; doc.nodes.len()];
        (visible[0], visible[second]) = (true, true);
        drop_hidden_images(&mut dom, &visible, &styles, |image| released.push(image.id));
        assert_eq!(dom.media.canvases[&canvas].pixels.pixels()[0].red(), 255);
        assert_eq!(released.len(), 2);
        assert!(released.contains(&dom.media.canvases[&canvas].image.id));
        assert!(dom.rasters.backgrounds.contains_key(&(second, 0)));
        assert!(!dom.rasters.backgrounds.contains_key(&(first, 0)));
        assert!(!dom.rasters.backgrounds.contains_key(&(second, 64)));
        assert!(dom.rasters.bitmaps.contains_key("t"));
        released.clear();
        for _ in 0..2 {
            drop_hidden_images(&mut dom, &[], &styles, |image| released.push(image.id));
        }
        assert!(
            dom.rasters.backgrounds.is_empty()
                && dom.rasters.bitmaps.is_empty()
                && dom.rasters.layer_sources.is_empty()
        );
        assert!(dom.media.canvases.contains_key(&canvas));
    }

    #[test]
    fn masks_release_their_image_when_the_box_has_no_mask() {
        let doc = crate::dom::Document::parse(
            "<i style='mask-image:url(m.png)'></i><b style='clip-path:circle()'></b><u></u>",
        );
        let mut dom = crate::render::PageState::default();
        let styles = dom.restyle(&mut doc.clone(), [800., 600.]);
        let ids =
            ["i", "b", "u"].map(|tag| doc.nodes.iter().position(|node| node.tag == tag).unwrap());
        for id in ids {
            let image = Arc::new(renderer::RenderImage::new(1, 1, vec![0; 4]));
            dom.rasters
                .masks
                .insert((id, 0), (String::new(), Some(image)));
        }
        let mut released = Vec::new();
        drop_hidden_images(&mut dom, &vec![true; doc.nodes.len()], &styles, |image| {
            released.push(image.id)
        });
        assert_eq!(released.len(), 1);
        assert!(
            dom.rasters.masks.contains_key(&(ids[0], 0))
                && dom.rasters.masks.contains_key(&(ids[1], 0))
        );
    }
}
