use super::{LayoutMirror, Walk};
use std::rc::Rc;

// A move of the mirror for a scroll: by how much, and the nodes of the walk it moved.
type ScrollShift = ([f32; 2], Rc<Vec<(u32, u32)>>, std::ops::Range<usize>);

// Where a canvas drew a drawable element, which takes the pointer there.
pub(crate) struct Drawn {
    // From the border box of the element to the content box of the canvas, in CSS pixels.
    pub(crate) matrix: [f32; 16],
    // The part of the content box of the canvas that shows the element.
    pub(crate) clip: Option<[f32; 4]>,
    // Elements drawn later take the pointer first.
    pub(crate) order: u32,
}

#[derive(Default)]
pub struct Geometry {
    pub mirror: LayoutMirror,
    // The parts of boxes that columns split, which take pointer events.
    pub(crate) fragments: rustc_hash::FxHashMap<usize, Box<[[f32; 4]]>>,
    // The parts of spliced inline boxes in each line of text that holds them, by the key of that text.
    pub(crate) pieces: rustc_hash::FxHashMap<usize, Vec<(usize, [f32; 4])>>,
    pub scroll: rustc_hash::FxHashMap<usize, renderer::ScrollRef>,
    pub(crate) scroll_offsets: rustc_hash::FxHashMap<usize, renderer::Point<renderer::Pixels>>,
    // The scroll offsets that the mirror shows, as of the last prepaint, the moves of the mirror for user scrolls
    // since then, and whether the boxes in each scroll move as a whole, by style epoch and walk.
    pub(crate) mirror_offsets: rustc_hash::FxHashMap<usize, renderer::Point<renderer::Pixels>>,
    pub(crate) scroll_shifts: Vec<ScrollShift>,
    // Scrolls that moved since the mirror did, and whether they alone left it stale.
    pub(crate) unshifted: Vec<usize>,
    pub(crate) only_scrolled: bool,
    pub(crate) shiftable: rustc_hash::FxHashMap<usize, (u64, u64, bool)>,
    pub(crate) layout_epoch: u64,
    pub(crate) drawn: rustc_hash::FxHashMap<usize, Drawn>,
    pub(crate) drawn_order: u32,
}

impl Geometry {
    // The scroll offset of a box that the mirror shows.
    pub(crate) fn mirror_offset(&self, id: usize) -> renderer::Point<renderer::Pixels> {
        self.mirror_offsets.get(&id).copied().unwrap_or_default()
    }

    // After prepaint, the mirror shows the scroll offsets now.
    pub(crate) fn prepainted(&mut self, scale: f32) {
        self.mirror_offsets.clear();
        for (&id, handle) in &self.scroll {
            self.mirror_offsets
                .insert(id, painted_offset(handle, scale));
        }
    }

    // Prepaint moves the mirror from where it put it, so the moves for user scrolls go back first.
    pub(crate) fn unshift(&mut self) {
        self.unshifted.clear();
        self.only_scrolled = false;
        for (by, nodes, range) in std::mem::take(&mut self.scroll_shifts).into_iter().rev() {
            self.shift(&nodes[range], [-by[0], -by[1]]);
        }
    }

    // Moves the mirror, the column parts and the inline pieces of these nodes.
    pub(crate) fn shift(&mut self, nodes: &[(u32, u32)], [x, y]: [f32; 2]) {
        for &(id, _) in nodes {
            let id = id as usize;
            if let Some(rect) = self.mirror.get_mut(&id)
                && *rect != [0.; 4]
            {
                (rect[0], rect[1]) = (rect[0] + x, rect[1] + y);
            }
            if !self.fragments.is_empty()
                && let Some(parts) = self.fragments.get_mut(&id)
            {
                parts
                    .iter_mut()
                    .for_each(|part| (part[0], part[1]) = (part[0] + x, part[1] + y));
            }
            if !self.pieces.is_empty()
                && let Some(pieces) = self.pieces.get_mut(&id)
            {
                pieces
                    .iter_mut()
                    .for_each(|(_, part)| (part[0], part[1]) = (part[0] + x, part[1] + y));
            }
        }
    }

    // How far the user scrolls since the last prepaint moved a node of the mirror. Text layouts keep their places.
    pub(crate) fn scroll_shift(&self, walk: &Walk, id: usize) -> [f32; 2] {
        let Some(place) = walk.place(id).map(|place| place as usize) else {
            return [0.; 2];
        };
        self.scroll_shifts
            .iter()
            .filter(|(_, nodes, range)| {
                range.contains(&place) && nodes.get(place).is_some_and(|node| node.0 as usize == id)
            })
            .fold([0.; 2], |[x, y], (by, ..)| [x + by[0], y + by[1]])
    }

    // The scrolls that moved since the last call. It clamps offsets to their range and marks the mirror as moved.
    pub(crate) fn scroll_events(&mut self) -> Vec<usize> {
        let mut changed = Vec::new();
        for (&id, handle) in &self.scroll {
            let offset = scroll_offset(handle, handle.offset());
            // `set_offset` stops a smooth scroll, so it only runs when the range clamps the offset.
            if offset != handle.offset() {
                handle.set_offset(offset);
            }
            let previous = self.scroll_offsets.entry(id).or_default();
            if *previous != offset {
                *previous = offset;
                changed.push(id);
            }
        }
        changed.sort_unstable();
        // Prepaint moves the mirror for a scroll, so a geometry read before it moves the mirror, or lays out again.
        if !changed.is_empty() {
            self.only_scrolled = (self.only_scrolled || !self.mirror.moved())
                && changed
                    .iter()
                    .all(|id| self.mirror_offsets.contains_key(id));
            for &id in &changed {
                if !self.unshifted.contains(&id) {
                    self.unshifted.push(id);
                }
            }
            self.mirror.set_moved();
        }
        self.sync_scroll();
        changed
    }

    // Moves the mirror for the user scrolls since the last prepaint, when a script reads geometry. True when the
    // mirror is then current without a layout, which needs a `key` of the styles. `whole` tells whether the boxes in a scroll, the nodes of its
    // walk range, move as a whole with it; its answer is kept by `key`.
    pub(crate) fn shift_scrolls(
        &mut self,
        walk: &Walk,
        key: Option<u64>,
        scale: f32,
        whole: impl Fn(usize, &[(u32, u32)]) -> Option<bool>,
    ) -> bool {
        let unshifted = std::mem::take(&mut self.unshifted);
        if !std::mem::take(&mut self.only_scrolled) {
            return false;
        }
        let mut shifted = true;
        for id in unshifted {
            let Some(to) = self
                .scroll
                .get(&id)
                .map(|handle| painted_offset(handle, scale))
            else {
                shifted = false;
                continue;
            };
            // Moves the mirror of the boxes in a scroll that the user moved, so geometry reads before the next
            // prepaint see the new offset. Not when its boxes don't move as a whole, like fixed, sticky or
            // transformed boxes.
            let Some(&from) = self.mirror_offsets.get(&id).filter(|_| id != 0) else {
                shifted = false;
                continue;
            };
            let (Some(start), Some(key)) = (walk.place(id), key) else {
                shifted = false;
                continue;
            };
            let by = [f32::from(to.x - from.x), f32::from(to.y - from.y)];
            let range = start as usize + 1..start as usize + walk.span(id);
            let key = (key, walk.serial);
            let moves = match self.shiftable.get(&id) {
                Some(&(epoch, serial, moves)) if (epoch, serial) == key => moves,
                _ => {
                    let Some(moves) = whole(id, &walk.order()[range.clone()]) else {
                        shifted = false;
                        continue;
                    };
                    self.shiftable.insert(id, (key.0, key.1, moves));
                    moves
                }
            };
            if !moves {
                shifted = false;
                continue;
            }
            let nodes = walk.snapshot();
            self.shift(&nodes[range.clone()], by);
            self.scroll_shifts.push((by, nodes, range));
            self.mirror_offsets.insert(id, to);
        }
        if shifted {
            self.mirror.settle();
        }
        shifted
    }

    pub(crate) fn sync_scroll(&mut self) {
        for (&id, handle) in &self.scroll {
            let (offset, max) = (scroll_offset(handle, handle.offset()), handle.overflow());
            self.mirror.scroll(
                id,
                [
                    -f32::from(offset.x),
                    -f32::from(offset.y),
                    f32::from(max.x),
                    f32::from(max.y),
                ],
            );
        }
    }
}

pub(crate) fn scroll_offset(
    handle: &renderer::ScrollRef,
    offset: renderer::Point<renderer::Pixels>,
) -> renderer::Point<renderer::Pixels> {
    let bounds = handle.bounds();
    match bounds.size.width > renderer::px(0.) || bounds.size.height > renderer::px(0.) {
        true => handle.clamped(offset),
        false => offset,
    }
}

pub(crate) fn painted_offset(
    handle: &renderer::ScrollRef,
    scale: f32,
) -> renderer::Point<renderer::Pixels> {
    handle.painted_at(scroll_offset(handle, handle.offset()), scale)
}

impl Geometry {
    // Where a canvas drew element `id`. A matrix or clip that is `None` stays as it was, and the element takes
    // the pointer before the others unless it `keeps` its order. True when the matrix changed.
    pub(crate) fn draw(
        &mut self,
        id: usize,
        matrix: Option<[f32; 16]>,
        clip: Option<[f32; 4]>,
        keeps: bool,
    ) -> bool {
        let old = self.drawn.remove(&id);
        if !(keeps && old.is_some()) {
            self.drawn_order += 1;
        }
        let new = Drawn {
            matrix: matrix
                .or(old.as_ref().map(|old| old.matrix))
                .unwrap_or(crate::css::value::transform::IDENTITY),
            clip: clip.or(old.as_ref().and_then(|old| old.clip)),
            order: match &old {
                Some(old) if keeps => old.order,
                _ => self.drawn_order,
            },
        };
        let moved = old.is_none_or(|old| old.matrix != new.matrix);
        self.drawn.insert(id, new);
        moved
    }

    // Sets the marks of the mirror that come from styles, once for each style pass `epoch`.
    pub(crate) fn style_flags<'a>(
        &mut self,
        styles: impl Iterator<Item = &'a Rc<crate::css::properties::Style>>,
        epoch: u64,
        flags: impl Fn(&crate::css::properties::Style) -> [bool; 3],
    ) {
        if self.layout_epoch == epoch {
            return;
        }
        self.layout_epoch = epoch;
        let mut known = rustc_hash::FxHashMap::default();
        for (id, style) in styles.enumerate() {
            let bits = *known
                .entry(Rc::as_ptr(style))
                .or_insert_with(|| flags(style));
            self.mirror.style_flags(id, bits);
        }
    }

    pub(crate) fn forget(&mut self, id: usize) {
        self.mirror.remove(&id);
        self.fragments.remove(&id);
        self.pieces.remove(&id);
        self.scroll.remove(&id);
        self.scroll_offsets.remove(&id);
        self.drawn.remove(&id);
    }
}
