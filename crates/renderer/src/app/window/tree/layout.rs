use super::*;

impl Tree {
    pub(crate) fn layout(&mut self, root: NodeId, viewport: Size<Pixels>, window: &mut Window) {
        let scale = window.scale_factor();
        let space = taffy::Size {
            width: taffy::AvailableSpace::Definite(viewport.width.0 * scale),
            height: taffy::AvailableSpace::Definite(viewport.height.0 * scale),
        };
        self.choose_anchors(window);
        let mut tree = LayoutTree {
            slots: &mut self.slots,
            window,
            scale,
            exact: false,
            floats: Vec::new(),
            ceiling: f32::NEG_INFINITY,
            reach: f32::NEG_INFINITY,
            origin: [0.; 2],
            active: false,
            joined: false,
            hung: Vec::new(),
            viewport: [viewport.width.0 * scale, viewport.height.0 * scale],
            roomed: Vec::new(),
        };
        taffy::compute_root_layout(&mut tree, root, space);
        // A layout boundary that the layout of its parents didn't reach lays out alone, as its parents placed it.
        for index in mem::take(&mut self.bounded) {
            let slot = &tree.slots[index as usize];
            if slot.live
                && slot.held
                && let Some(inputs) = slot.inputs
            {
                tree.compute(NodeId::from(index as usize), inputs, None);
            }
        }
        // Escaped boxes took the room of their containing blocks from the last layout. Where this layout changed it,
        // the boxes on their path lay out again.
        self.roomed.extend(mem::take(&mut tree.roomed));
        let viewport_room = tree.viewport;
        let mut again = false;
        self.roomed.retain(|&index| {
            let Some(slot) = tree.slots.get(index as usize) else {
                return false;
            };
            let escape = slot.style.escape & (Escape::ABSOLUTE | Escape::FIXED);
            let (Some(old), true, true) =
                (slot.room, slot.live, escape != 0 && slot.parent != NONE)
            else {
                return false;
            };
            let Some(new) = room(tree.slots, slot.parent, escape, viewport_room) else {
                return false;
            };
            if (old[0] - new[0]).abs() > 0.5 || (old[1] - new[1]).abs() > 0.5 {
                again = true;
                let mut at = tree.slots[index as usize].parent;
                while at != NONE {
                    tree.slots[at as usize].cache.clear();
                    at = tree.slots[at as usize].parent;
                }
            }
            true
        });
        if again {
            taffy::compute_root_layout(&mut tree, root, space);
        }
        #[cfg(debug_assertions)]
        {
            let kept: Vec<_> = tree
                .slots
                .iter_mut()
                .map(|slot| (mem::take(&mut slot.cache), slot.layout, slot.baselines))
                .collect();
            tree.exact = true;
            taffy::compute_root_layout(&mut tree, root, space);
            let mut pending = vec![index_of(root)];
            while let Some(index) = pending.pop() {
                let slot = &tree.slots[index as usize];
                let (_, layout, baselines) = &kept[index as usize];
                // `contain` moves escaped boxes after layout.
                let location = match self.escaping.contains(&index) {
                    true => layout.location,
                    false => slot.layout.location,
                };
                let exact = (location, slot.layout.size, slot.baselines);
                assert!(
                    (layout.location, layout.size, *baselines) == exact,
                    "box {index} was laid out at {:?} {:?} {baselines:?}, taffy places it at {:?} {:?} {:?}",
                    layout.location,
                    layout.size,
                    slot.layout.location,
                    slot.layout.size,
                    slot.baselines,
                );
                pending.extend(slot.children.iter().map(|&child| index_of(child)));
            }
            for (slot, (cache, layout, baselines)) in tree.slots.iter_mut().zip(kept) {
                (slot.cache, slot.layout, slot.baselines) = (cache, layout, baselines);
            }
        }
        self.contain(viewport, scale);
    }

    // Taffy places an absolute box in its parent, so a box whose containing block is further out moves to its
    // insets there, before the scroll range and paint read its place. An axis without insets keeps its static
    // place. The result doesn't depend on the place it had, so cached layouts that keep it stay right.
    fn contain(&mut self, viewport: Size<Pixels>, scale: f32) {
        let slots = &mut self.slots;
        // A new page can have fewer slots.
        self.escaping.retain(|&index| {
            slots.get(index as usize).is_some_and(|slot| {
                slot.live && slot.style.escape & Escape::ABSOLUTE != 0 && slot.style.placed != 0
            })
        });
        for &index in &self.escaping {
            let slot = &slots[index as usize];
            let parent = slot.parent;
            if parent == NONE || slots[parent as usize].style.contains & Escape::ABSOLUTE != 0 {
                continue;
            }
            // The parent's place in the containing block, which is the outermost box when no box contains it.
            let (mut at, mut block) = ([0f32; 2], parent);
            loop {
                let outer = &slots[block as usize];
                if outer.style.contains & Escape::ABSOLUTE != 0 || outer.parent == NONE {
                    break;
                }
                at = [
                    at[0] + outer.layout.location.x,
                    at[1] + outer.layout.location.y,
                ];
                block = outer.parent;
            }
            let outer = &slots[block as usize];
            let [origin, size] = match outer.style.contains & Escape::ABSOLUTE != 0 {
                true => {
                    let (border, size) = (outer.layout.border, outer.layout.size);
                    [
                        [border.left, border.top],
                        [
                            size.width - border.left - border.right,
                            size.height - border.top - border.bottom,
                        ],
                    ]
                }
                false => [
                    [0., 0.],
                    [viewport.width.0 * scale, viewport.height.0 * scale],
                ],
            };
            let rtl = outer.style.flow & Flow::RTL_BLOCK != 0;
            let resolve = |length: crate::Length, basis: f32| match length {
                crate::Length::Px(value) => Some(value.0 * scale),
                crate::Length::Fraction(share) => Some(share * basis),
                crate::Length::Calc(share, offset) => Some(share * basis + offset.0 * scale),
                _ => None,
            };
            let (inset, layout) = (&slot.style.inset, slot.layout);
            let mut location = layout.location;
            let place = |start: Option<f32>, end: Option<f32>, end_wins: bool, axis: usize| {
                let [margin_start, margin_end, extent] = match axis {
                    0 => [layout.margin.left, layout.margin.right, layout.size.width],
                    _ => [layout.margin.top, layout.margin.bottom, layout.size.height],
                };
                match (start, end) {
                    (Some(start), end) if !end_wins || end.is_none() => {
                        Some(origin[axis] + start + margin_start)
                    }
                    (_, Some(end)) => Some(origin[axis] + size[axis] - end - margin_end - extent),
                    _ => None,
                }
            };
            let [left, right] = [inset.left, inset.right].map(|v| resolve(v, size[0]));
            let [top, bottom] = [inset.top, inset.bottom].map(|v| resolve(v, size[1]));
            let placed = slot.style.placed;
            if placed & 1 != 0
                && let Some(x) = place(left, right, rtl, 0)
            {
                location.x = x - at[0];
            }
            if placed & 2 != 0
                && let Some(y) = place(top, bottom, false, 1)
            {
                location.y = y - at[1];
            }
            if location != layout.location {
                slots[index as usize].layout.location = location;
                changed(slots, index as usize);
                unfar(slots, parent as usize);
            }
        }
    }

    // Before a layout, the boxes are where the last one put them. A scroll container that a scroll moved since it
    // chose its scroll anchor chooses again there when the content inside lays out again, as in Chrome.
    fn choose_anchors(&mut self, window: &mut Window) {
        let unanchored = window.take_unanchored();
        let scale = window.scale_factor();
        for at in 0..self.anchoring.len() {
            let Some(&index) = self.keys.get(&self.anchoring[at]) else {
                continue;
            };
            let Some(handle) = scroll_of(&self.slots[index as usize]) else {
                continue;
            };
            let offset = handle.offset();
            let mut anchor = handle
                .anchor()
                .filter(|anchor| anchor.offset == offset && self.keys.contains_key(&anchor.key));
            if anchor.is_none()
                && offset.y != Pixels::ZERO
                && self.slots[index as usize].cache.is_empty()
            {
                let slot = &self.slots[index as usize];
                let taffy::Layout { size, border, .. } = slot.layout;
                let view = [
                    border.left,
                    border.top,
                    size.width - border.right,
                    size.height - border.bottom,
                ];
                let shift = point(offset.x.0 * scale, offset.y.0 * scale);
                anchor = self
                    .find_anchor(index, shift, view, slot.style.contains)
                    .map(|(found, at)| Anchor {
                        key: self.slots[found as usize].key,
                        at,
                        offset,
                        suppressed: false,
                    });
            }
            // A change of the style of a box between the anchor and the scroll container suppresses the adjustment.
            if let Some(anchor) = &mut anchor
                && !unanchored.is_empty()
                && let Some(mut node) = self.keys.get(&anchor.key).copied()
            {
                loop {
                    let slot = &self.slots[node as usize];
                    if slot.keyed && slot.key.1 == 0 && unanchored.contains(&slot.key.0) {
                        anchor.suppressed = true;
                    }
                    if node == index || slot.parent == NONE {
                        break;
                    }
                    node = slot.parent;
                }
            }
            handle.set_anchor(anchor);
        }
    }

    // The first box in tree order that the viewport shows, as Chrome finds it: one that it shows whole, or else the
    // last box on the path to the first one that it shows in part. `at` places the box in the viewport, and the
    // result has the top of the anchor there.
    fn find_anchor(
        &self,
        index: u32,
        at: Point<f32>,
        view @ [left, top, right, bottom]: [f32; 4],
        contains: u8,
    ) -> Option<(u32, f32)> {
        for &child in &self.slots[index as usize].children {
            let child = index_of(child);
            let slot = &self.slots[child as usize];
            let style = &slot.style;
            if style.display == Display::None
                || style.scroll_anchor != 0
                || matches!(slot.kind, Kind::Layers(_))
                || style.escape & (Escape::FIXED | Escape::OUTLINE) != 0
                || style.escape & Escape::ABSOLUTE != 0 && contains & Escape::ABSOLUTE == 0
            {
                continue;
            }
            let location = slot.layout.location;
            let mut origin = point(at.x + location.x, at.y + location.y);
            if let Some(split) = &slot.split {
                origin += split.shift;
            }
            let size = slot.layout.size;
            let contains = contains | style.contains;
            // A scroll container is an anchor itself, since its boxes move with its own offset.
            let scrolls = (slot.events.as_ref()).is_some_and(|events| events.scroll.is_some());
            if !slot.keyed && !matches!(slot.kind, Kind::Text(_))
                || size.width <= 0.
                || size.height <= 0.
            {
                // Anonymous boxes are no anchors, but the boxes in them can be.
                if !scrolls && let Some(found) = self.find_anchor(child, origin, view, contains) {
                    return Some(found);
                }
                continue;
            }
            let far = point(origin.x + size.width, origin.y + size.height);
            if origin.x >= right || far.x <= left || origin.y >= bottom || far.y <= top {
                continue;
            }
            let whole = origin.x >= left && origin.y >= top && far.x <= right && far.y <= bottom;
            if whole || scrolls {
                return Some((child, origin.y));
            }
            return self
                .find_anchor(child, origin, view, contains)
                .or(Some((child, origin.y)));
        }
        None
    }

    // Moves the scroll offset by the move of the scroll anchor since the last layout, so the anchor stays where the
    // viewport showed it. Lists the container for the next layout to choose an anchor.
    pub(super) fn anchor(&mut self, index: u32, handle: &ScrollRef, scale: f32) {
        let slot = &mut self.slots[index as usize];
        if slot.style.scroll_anchor & ScrollAnchor::NONE != 0 {
            handle.set_anchor(None);
            return;
        }
        // A scroll from the top and a change in the same task choose an anchor at the next layout too.
        if mem::replace(&mut slot.anchoring, self.prepaints) != self.prepaints {
            self.anchoring.push(slot.key);
        }
        let offset = handle.offset();
        let Some(anchor) = handle.anchor() else {
            return;
        };
        let Some(mut node) = self
            .keys
            .get(&anchor.key)
            .copied()
            .filter(|_| anchor.offset == offset)
        else {
            return handle.set_anchor(None);
        };
        let mut top = 0.;
        while node != index {
            let slot = &self.slots[node as usize];
            top += slot.layout.location.y + slot.split.as_ref().map_or(0., |split| split.shift.y);
            node = slot.parent;
            if node == NONE {
                return handle.set_anchor(None);
            }
        }
        // Like Chrome, the anchor moves the offset by whole device pixels.
        let delta = (top + offset.y.0 * scale).round() - anchor.at.round();
        if delta != 0. && anchor.suppressed {
            return handle.set_anchor(None);
        }
        if anchor.suppressed {
            handle.set_anchor(Some(Anchor {
                suppressed: false,
                ..anchor
            }));
        }
        if delta != 0. {
            handle.anchor_by(px(-delta / scale));
        }
    }

    pub(super) fn bounds(
        &self,
        index: u32,
        origin: Point<f32>,
        window: &Window,
    ) -> [Rect<Pixels>; 2] {
        let scale = window.scale_factor();
        let layout = &self.slots[index as usize].layout;
        let far = point(origin.x + layout.size.width, origin.y + layout.size.height);
        let snapped = Rect::between(origin.map(round_half_up), far.map(round_half_up));
        let offset = window.offset();
        let exact = Rect::new(
            origin.map(|value| Pixels(value / scale)),
            size(
                Pixels(layout.size.width / scale),
                Pixels(layout.size.height / scale),
            ),
        );
        [(snapped / scale).map(Pixels) + offset, exact + offset]
    }
}

impl LayoutTree<'_> {
    pub(super) fn compute(
        &mut self,
        node: NodeId,
        mut inputs: LayoutInput,
        block: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        let joined = mem::take(&mut self.active);
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            return taffy::compute_hidden_layout(self, node);
        }
        if inputs.run_mode == RunMode::PerformLayout {
            let slot = &mut self.slots[usize::from(node)];
            slot.held = false;
            slot.inputs = slot.boundary().then_some(inputs);
        }
        let slot = &self.slots[usize::from(node)];
        let style = slot.taffy();
        if !self.exact
            && inputs.run_mode == RunMode::ComputeSize
            && inputs.sizing_mode == taffy::SizingMode::ContentSize
            && slot.parent != NONE
            && style.position != taffy::Position::Absolute
        {
            let parent = self.slots[slot.parent as usize].taffy();
            let row = matches!(
                parent.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            );
            let (axis, main, cross, size, line) = match row {
                true => (
                    taffy::RequestedAxis::Vertical,
                    inputs.known_dimensions.width,
                    inputs.known_dimensions.height,
                    style.size.height,
                    inputs.parent_size.height,
                ),
                false => (
                    taffy::RequestedAxis::Horizontal,
                    inputs.known_dimensions.height,
                    inputs.known_dimensions.width,
                    style.size.width,
                    inputs.parent_size.width,
                ),
            };
            let margins = match row {
                true => [style.margin.top, style.margin.bottom],
                false => [style.margin.left, style.margin.right],
            };
            if parent.display == taffy::Display::Flex
                && parent.flex_wrap == taffy::FlexWrap::NoWrap
                && inputs.axis == axis
                && cross.is_none()
                && size.is_auto()
                && margins.iter().all(|margin| !margin.is_auto())
                && matches!(
                    style.align_self.unwrap_or(parent.align_items).keyword,
                    AlignItemsKeyword::Normal | AlignItemsKeyword::Stretch
                )
                && let (Some(main), Some(line)) = (main, line)
            {
                let size = match row {
                    true => taffy::Size {
                        width: main,
                        height: line,
                    },
                    false => taffy::Size {
                        width: line,
                        height: main,
                    },
                };
                return LayoutOutput::from_outer_size(size);
            }
        }
        // Taffy treats measured absolute heights as definite. With auto height and no paired insets, child
        // percentage heights must stay auto.
        if style.position == taffy::Position::Absolute
            && inputs.run_mode == RunMode::PerformLayout
            && style.size.height.is_auto()
            && (style.inset.top.is_auto() || style.inset.bottom.is_auto())
        {
            inputs.known_dimensions_are_definite.height = false;
        }
        // Taffy lays out a block as a flex column, whose items get definite heights. In a block, an auto height is
        // not definite, unless a ratio gives it, so percent heights in the child stay auto.
        if inputs.known_dimensions.height.is_some()
            && style.size.height.is_auto()
            && style.aspect_ratio.is_none()
            && style.position != taffy::Position::Absolute
            && slot.parent != NONE
            && self.slots[slot.parent as usize].style.block
        {
            inputs.known_dimensions_are_definite.height = false;
        }
        let layout = |tree: &mut Self, node: NodeId, inputs: LayoutInput| {
            tree.joined = joined;
            let slot = &tree.slots[usize::from(node)];
            let leaf = slot.children.is_empty();
            let mut output = match slot.taffy().display {
                taffy::Display::None => taffy::compute_hidden_layout(tree, node),
                _ if leaf => tree.measure(usize::from(node), inputs),
                taffy::Display::Flex => tree
                    .stack(node, inputs)
                    .or_else(|| tree.row(node, inputs))
                    .unwrap_or_else(|| tree.flexbox(node, inputs)),
                taffy::Display::Grid => taffy::compute_grid_layout(tree, node, inputs),
                _ => taffy::compute_block_layout(tree, node, inputs, block),
            };
            if tree.slots[usize::from(node)].style.columns.is_some() {
                output = tree.columns(node, inputs, output);
            }
            if inputs.run_mode == RunMode::PerformLayout {
                // Taffy reads `direction` in all layout, which handles right-to-left itself, so a right-to-left
                // box gives it only to the pass of its absolute boxes.
                let index = usize::from(node);
                // Like Chrome, a box that escapes its parent takes the room of its containing block, so its auto
                // size doesn't shrink to the parent.
                if tree.slots[index].style.flow & Flow::RTL_BLOCK == 0
                    && let Some(area) = output.oof_positioning_area
                    && output.oof_candidates.as_slice().iter().any(|candidate| {
                        let escape = tree.slots[usize::from(candidate.node)].style.escape
                            & (Escape::ABSOLUTE | Escape::FIXED);
                        escape != 0 && tree.slots[index].style.contains & escape == 0
                    })
                {
                    let mut claims =
                        taffy::CoreStyle::is_containing_block(tree.slots[index].taffy());
                    (claims.absolute, claims.fixed) = (true, true);
                    let mut rest = taffy::OofCandidates::new();
                    for &candidate in output.oof_candidates.take().as_slice() {
                        let child = usize::from(candidate.node);
                        let escape =
                            tree.slots[child].style.escape & (Escape::ABSOLUTE | Escape::FIXED);
                        let Some(room) = (escape != 0)
                            .then(|| room(tree.slots, index as u32, escape, tree.viewport))
                            .flatten()
                        else {
                            rest.push(candidate);
                            continue;
                        };
                        tree.slots[child].room = Some(room);
                        tree.roomed.push(child as u32);
                        let area = taffy::OofPositioningArea {
                            size: taffy::Size {
                                width: area.size.width.max(room[0]),
                                height: area.size.height.max(room[1]),
                            },
                            ..area
                        };
                        taffy::compute_oof_layout_for_area(
                            tree,
                            node,
                            &[candidate],
                            area,
                            taffy::Direction::Ltr,
                            claims,
                        );
                    }
                    output.oof_candidates = rest;
                }
                if tree.slots[index].style.flow & Flow::RTL_BLOCK != 0 {
                    if let Some(area) = output.oof_positioning_area
                        && !output.oof_candidates.is_empty()
                    {
                        let claims =
                            taffy::CoreStyle::is_containing_block(&tree.slots[index].taffy());
                        let candidates = output.oof_candidates.take();
                        let result = taffy::compute_oof_layout_for_area(
                            tree,
                            node,
                            candidates.as_slice(),
                            area,
                            taffy::Direction::Rtl,
                            claims,
                        );
                        output.oof_candidates = result.unclaimed;
                        // Warning: with Taffy's `content_size` feature, merge `result.scrollable_overflow_rect`
                        // into the output here, as `compute_oof_layout` does.
                    }
                } else {
                    taffy::compute_oof_layout(tree, node, &mut output);
                }
                output.baselines = tree.baselines(usize::from(node), &output);
            }
            output
        };
        let index = usize::from(node);
        // A box with floats that no float of its context reaches lays out the same each time: it keeps its cache,
        // and the floats it placed.
        let quiet = joined
            && self.slots[index].floats
            && self.floats.iter().all(|float| float[3] <= self.origin[1]);
        let output = if quiet {
            let [x, y] = self.origin;
            self.floats.clear();
            match self.cache_get(node, &inputs) {
                Some(output) if self.slots[index].spill.is_some() => {
                    let (floats, ceiling, reach) = self.slots[index].spill.as_deref().unwrap();
                    self.floats.extend(
                        floats
                            .iter()
                            .map(|f| [f[0] + x, f[1] + y, f[2] + x, f[3] + y, f[4]]),
                    );
                    (self.ceiling, self.reach) =
                        (self.ceiling.max(ceiling + y), self.reach.max(reach + y));
                    output
                }
                _ => {
                    let outer = (self.ceiling, self.reach);
                    (self.ceiling, self.reach) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
                    let output = layout(self, node, inputs);
                    let floats = self
                        .floats
                        .iter()
                        .map(|f| [f[0] - x, f[1] - y, f[2] - x, f[3] - y, f[4]])
                        .collect();
                    self.slots[index].spill =
                        Some(Box::new((floats, self.ceiling - y, self.reach - y)));
                    (self.ceiling, self.reach) =
                        (outer.0.max(self.ceiling), outer.1.max(self.reach));
                    self.cache_store(node, &inputs, output.clone());
                    output
                }
            }
        } else if joined {
            // Beside floats, the result depends on them, so the cache is neither read nor stored, and a later
            // layout without floats must not find a result from before them.
            self.slots[index].cache.clear();
            self.slots[index].uncached = true;
            layout(self, node, inputs)
        } else {
            taffy::compute_cached_layout(self, node, inputs, layout)
        };
        if inputs.run_mode == RunMode::PerformLayout
            && self.slots[usize::from(node)].baselines != output.baselines
        {
            self.slots[usize::from(node)].baselines = output.baselines;
            if !self.exact {
                changed(self.slots, usize::from(node));
            }
        }
        output
    }
}

impl TraversePartialTree for LayoutTree<'_> {
    type ChildIter<'a>
        = std::iter::Copied<std::slice::Iter<'a, NodeId>>
    where
        Self: 'a;

    fn child_ids(&self, node: NodeId) -> Self::ChildIter<'_> {
        self.slots[usize::from(node)].children.iter().copied()
    }

    fn child_count(&self, node: NodeId) -> usize {
        self.slots[usize::from(node)].children.len()
    }

    fn get_child_id(&self, node: NodeId, child: usize) -> NodeId {
        self.slots[usize::from(node)].children[child]
    }
}

impl LayoutPartialTree for LayoutTree<'_> {
    type CoreContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type CustomIdent = String;

    fn get_core_container_style(&self, node: NodeId) -> &taffy::Style {
        self.slots[usize::from(node)].taffy()
    }

    fn resolve_calc_value(&self, handle: *const (), basis: f32) -> f32 {
        crate::style::resolve_calc(handle, basis)
    }

    // A box that only moved is not marked changed, since its kept prepaint follows it.
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        let index = usize::from(node);
        let mut layout = *layout;
        // Taffy gives negative free space to cross-axis auto margins. In CSS the start margin is then 0, and the
        // end margin takes the rest.
        let margin = layout.margin;
        if margin.left.min(margin.right).min(margin.top) < 0. {
            let slot = &self.slots[index];
            let style = slot.taffy();
            if style.position != taffy::Position::Absolute {
                if margin.top < 0. && style.margin.top.is_auto() {
                    layout.location.y -= margin.top;
                    layout.margin.bottom += margin.top;
                    layout.margin.top = 0.;
                }
                let rtl = slot.style.flow & Flow::RTL != 0;
                if !rtl && margin.left < 0. && style.margin.left.is_auto() {
                    layout.location.x -= margin.left;
                    layout.margin.right += margin.left;
                    layout.margin.left = 0.;
                } else if rtl && margin.right < 0. && style.margin.right.is_auto() {
                    layout.location.x += margin.right;
                    layout.margin.left += margin.right;
                    layout.margin.right = 0.;
                }
            }
        }
        let old = mem::replace(&mut self.slots[index].layout, layout);
        if !self.exact
            && (Layout {
                location: old.location,
                ..layout
            }) != old
        {
            changed(self.slots, index);
        }
    }

    fn compute_child_layout(&mut self, node: NodeId, inputs: LayoutInput) -> LayoutOutput {
        self.compute(node, inputs, None)
    }
}

// Hoisted lists are unnecessary: parents position out-of-flow boxes, and layout stays unrounded.
impl LayoutContainingBlock for LayoutTree<'_> {
    type OofItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_oof_item_style(&self, node: NodeId) -> &taffy::Style {
        self.slots[usize::from(node)].taffy()
    }

    fn clear_hoisted_children(&mut self, _: NodeId) {}

    fn add_hoisted_children(&mut self, _: NodeId, _: &[NodeId]) {}
}

impl CacheTree for LayoutTree<'_> {
    fn cache_get(&mut self, node: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        let slot = &mut self.slots[usize::from(node)];
        let relative = self.exact || slot.style.relative;
        slot.cache.get(&cache_input(slot.free, relative, input))
    }

    fn cache_store(&mut self, node: NodeId, input: &LayoutInput, output: LayoutOutput) {
        let slot = &mut self.slots[usize::from(node)];
        let relative = self.exact || slot.style.relative;
        slot.uncached = false;
        slot.cache
            .store(&cache_input(slot.free, relative, input), output);
    }

    fn cache_clear(&mut self, node: NodeId) {
        self.slots[usize::from(node)].cache.clear();
    }
}

impl LayoutBlockContainer for LayoutTree<'_> {
    type BlockContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type BlockItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_block_container_style(&self, node: NodeId) -> &taffy::Style {
        self.slots[usize::from(node)].taffy()
    }

    fn get_block_child_style(&self, node: NodeId) -> &taffy::Style {
        self.slots[usize::from(node)].taffy()
    }

    fn compute_block_child_layout(
        &mut self,
        node: NodeId,
        inputs: LayoutInput,
        block: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        self.compute(node, inputs, block)
    }
}

impl LayoutFlexboxContainer for LayoutTree<'_> {
    type FlexboxContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type FlexboxItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_flexbox_container_style(&self, node: NodeId) -> &taffy::Style {
        self.slots[usize::from(node)].taffy()
    }

    fn get_flexbox_child_style(&self, node: NodeId) -> &taffy::Style {
        match self.hung.iter().rfind(|(hung, ..)| *hung == node) {
            Some((_, _, style)) => style,
            None => self.slots[usize::from(node)].taffy(),
        }
    }
}

impl LayoutGridContainer for LayoutTree<'_> {
    type GridContainerStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;
    type GridItemStyle<'a>
        = &'a taffy::Style
    where
        Self: 'a;

    fn get_grid_container_style(&self, node: NodeId) -> &taffy::Style {
        self.slots[usize::from(node)].taffy()
    }

    fn get_grid_child_style(&self, node: NodeId) -> &taffy::Style {
        self.slots[usize::from(node)].taffy()
    }
}
