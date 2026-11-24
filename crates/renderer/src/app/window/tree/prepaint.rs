use super::*;

impl Tree {
    pub(crate) fn prepaint(&mut self, root: NodeId, window: &mut Window) {
        self.prepaints = self.prepaints.wrapping_add(1);
        window.begin_prepaint(self.prepaints);
        mem::swap(&mut self.touched, &mut self.last_touched.1);
        self.last_touched.0 = self.prepaints.wrapping_sub(1);
        self.touched.clear();
        self.stacks.clear();
        self.depths.clear();
        self.blocks.clear();
        self.hidden.clear();
        self.anchoring.clear();
        #[cfg(debug_assertions)]
        let start = window.region_places().len();
        self.copies = window.copies();
        self.prepaint_node(index_of(root), Point::default(), false, window);
        self.unshift = false;
        #[cfg(debug_assertions)]
        if !window.redraws() {
            // The boxes inside a box that a scroll moved keep their old places.
            let mut stale = vec![false; self.slots.len()];
            // The z-index lists of a moved box, and of the boxes inside it, are old too. Paint copies such a box.
            let mut lagged = vec![false; self.slots.len()];
            let mut pending = vec![(index_of(root), false)];
            while let Some((index, inside)) = pending.pop() {
                let slot = &self.slots[index as usize];
                stale[index as usize] = inside;
                let inside = inside || slot.lag != Point::default();
                lagged[index as usize] = inside;
                pending.extend(slot.children.iter().map(|&child| (index_of(child), inside)));
            }
            let boxes = |tree: &Tree| {
                let (mut pending, mut boxes) = (vec![index_of(root)], Vec::new());
                while let Some(index) = pending.pop() {
                    let slot = &tree.slots[index as usize];
                    if !stale[index as usize] {
                        boxes.push((
                            index,
                            slot.origin,
                            [slot.bounds, slot.painted, slot.exact],
                            slot.listed,
                        ));
                    }
                    pending.extend(slot.children.iter().map(|&child| index_of(child)));
                }
                boxes
            };
            let places = |tree: &Tree| {
                let place = |slot: &Slot| slot.region.map(|region| (region.bounds, region.hit));
                (tree.slots.iter().enumerate())
                    .filter(|(at, _)| !stale.get(*at).copied().unwrap_or(false))
                    .map(|(_, slot)| place(slot))
                    .collect::<Vec<_>>()
            };
            let kept = (
                boxes(self),
                places(self),
                window.region_places()[start..].to_vec(),
                self.layers.clone(),
            );
            window.truncate_regions(start);
            self.stacks.clear();
            self.blocks.clear();
            self.full = true;
            self.prepaint_node(index_of(root), Point::default(), false, window);
            self.full = false;
            let full = (
                boxes(self),
                places(self),
                window.region_places()[start..].to_vec(),
                self.layers.clone(),
            );
            let first = |kept: &[_], full: &[_]| {
                kept.iter().zip(full).position(|(kept, full)| kept != full)
            };
            let at = first(&kept.0, &full.0).map(|at| (kept.0[at], full.0[at]));
            assert!(kept.0 == full.0, "kept box differs: {at:?}");
            assert!(
                kept.1 == full.1,
                "kept hitboxes differ: {:?} {:?}",
                kept.1,
                full.1
            );
            assert!(
                kept.2 == full.2,
                "kept window hitboxes differ: {} {} {:?}",
                kept.2.len(),
                full.2.len(),
                (kept.2.iter().zip(&full.2).position(|(a, b)| a != b)).map(|at| (
                    at,
                    kept.2[at].0,
                    kept.2[at].1.bounds,
                    full.2[at].0,
                    full.2[at].1.bounds
                ))
            );
            let lists = (kept.3.iter())
                .filter(|(at, list)| {
                    !lagged.get(**at as usize).copied().unwrap_or(false)
                        && full.3.get(at) != Some(list)
                })
                .map(|(at, list)| (at, Some(list), full.3.get(at)))
                .chain(
                    (full.3.iter())
                        .filter(|(at, _)| {
                            !lagged.get(**at as usize).copied().unwrap_or(false)
                                && !kept.3.contains_key(at)
                        })
                        .map(|(at, list)| (at, None, Some(list))),
                )
                .collect::<Vec<_>>();
            assert!(lists.is_empty(), "kept layers differ: {lists:?}");
        }
    }

    // `covered`: a skip in the last pass kept the box as it was, though its stamp is older.
    fn prepaint_node(
        &mut self,
        index: u32,
        parent: Point<f32>,
        covered: bool,
        window: &mut Window,
    ) {
        let prepaints = self.prepaints;
        let slot = &mut self.slots[index as usize];
        let location = slot.layout.location;
        let mut origin = point(parent.x + location.x, parent.y + location.y);
        if let Some(split) = &slot.split {
            origin += split.shift;
        }
        let placement = window.placement();
        if !self.full
            && !slot.below
            && !slot.volatile
            && slot.placed.is_some_and(|(pass, _, placed)| {
                (covered || pass == prepaints.wrapping_sub(1))
                    && placed.transform == placement.transform
            })
        {
            self.keep(index, parent, window);
            return;
        }
        let slot = &mut self.slots[index as usize];
        // The boxes inside a box that a skip kept in the last pass are as they were, also when the box itself
        // was built again since.
        let covered = !self.full && (covered || slot.skipped == prepaints.wrapping_sub(1));
        let outer = (
            mem::replace(&mut self.skips, true),
            self.escapes.take(),
            self.escape(index, window),
            self.foreign.take(),
        );
        let (regions, touched) = (window.regions_len(), self.touched.len() as u32);
        let placement = window.placement();
        let slot = &mut self.slots[index as usize];
        slot.placed = Some((prepaints, origin, placement));
        slot.origin = origin;
        slot.lag = Point::default();
        // A listed box comes before the listed boxes inside it, which paint above it at the same z-index.
        slot.listed = slot.style.z_index.is_some() && !self.stacks.is_empty();
        if let Some(order) = slot.style.z_index
            && let Some(list) = self.stacks.last_mut()
        {
            list.push((
                order,
                index,
                window.visible_below(self.depths.last().map_or(0, |&(depth, _)| depth)),
            ));
        }
        let [bounds, exact] = self.bounds(index, origin, window);
        let slot = &mut self.slots[index as usize];
        (slot.bounds, slot.exact) = (bounds, exact);
        let (listed, [_, transforms]) = (self.stacks.last().map(Vec::len), window.prepaint_mark());
        let shifting = mem::replace(&mut self.shifting, true);
        self.visit(
            index,
            window,
            |hooks, cx, inner| hooks.prepaint(cx, inner),
            |tree, style, window| tree.prepaint_box(index, style, covered, window),
        );
        self.flatten(index, transforms, shifting, window);
        self.reach(index, window.scale_factor());
        if self.slots[index as usize].escaped.is_some() {
            self.unescape(window);
        }
        self.recorded(index, [regions, touched], listed, outer, window);
    }

    // Pushes the mask of the containing block of an out-of-flow box, which replaces intermediate clips. Returns
    // the place of that block for `escapes`.
    fn escape(&mut self, index: u32, window: &mut Window) -> Option<usize> {
        let slot = &self.slots[index as usize];
        let escape = slot.style.escape;
        // An outline takes the clip of its box, not the overflow clip inside it.
        let (escaped, depth, cut) = if escape == Escape::OUTLINE {
            let placed = self.slots[slot.parent as usize].placed;
            let cut = placed.map_or(0, |(_, _, placed)| placed.depth);
            (placed.map(|(_, _, placed)| placed.mask), None, cut)
        } else if escape != 0 {
            let at = self
                .blocks
                .iter()
                .rposition(|(contains, ..)| contains & escape != 0);
            let (mask, cut) = at.map_or_else(
                || {
                    (
                        Clip::from(Rect::new(Point::default(), window.viewport_size())),
                        0,
                    )
                },
                |at| (self.blocks[at].1, self.blocks[at].2),
            );
            (Some(mask), Some(at.map_or(0, |at| at + 1)), cut)
        } else {
            (None, None, 0)
        };
        // A listed box paints in its stacking context, outside the scrolls between that and its containing block.
        let escaped = match self.depths.last() {
            Some(&(context, _)) if slot.style.z_index.is_some() && context < cut as usize => {
                escaped.map(|mask| window.cut_between(mask, context, cut as usize))
            }
            _ => escaped,
        };
        if let Some(mask) = escaped {
            window.push_content_mask(mask);
            self.hidden.push(window.hide_viewports(cut as usize));
        }
        let slot = &mut self.slots[index as usize];
        (slot.escaped, slot.cut) = (escaped, cut);
        depth
    }

    // Ends `escape`.
    fn unescape(&mut self, window: &mut Window) {
        window.pop_content_mask();
        if let Some(hidden) = self.hidden.pop() {
            window.show_viewports(hidden);
        }
    }

    // A box is flat when no hit transform is in it and all hooks in it can shift.
    fn flatten(&mut self, index: u32, transforms: u32, outer: bool, window: &Window) {
        let slot = &mut self.slots[index as usize];
        slot.flat = mem::replace(&mut self.shifting, outer)
            && window.prepaint_mark()[1] == transforms
            && slot.hooks.as_ref().is_none_or(|hooks| hooks.shifts());
        self.shifting &= slot.flat;
    }

    fn calm(&mut self, index: u32) {
        let slot = &mut self.slots[index as usize];
        if slot.calm != self.prepaints.wrapping_sub(1) {
            slot.since = self.prepaints;
        }
        slot.calm = self.prepaints;
    }

    // Keeps what a prepaint of a box gave, for a skip of the box in a later pass.
    fn recorded(
        &mut self,
        index: u32,
        [regions, touched]: [u32; 2],
        listed: Option<usize>,
        (outer, escapes, own, foreign): (bool, Option<usize>, Option<usize>, Option<usize>),
        window: &Window,
    ) {
        let slot = &mut self.slots[index as usize];
        let min = |a: Option<usize>, b: Option<usize>| a.zip(b).map(|(a, b)| a.min(b)).or(a).or(b);
        let inner = self.escapes;
        self.escapes = min(min(escapes, own), inner);
        // A listed box paints in its stacking context, not in the chunks of the boxes around it.
        let painted = own.filter(|_| slot.style.z_index.is_none());
        let inner_foreign = self.foreign;
        self.foreign = min(min(foreign, painted), inner_foreign);
        slot.contained = inner_foreign.is_none_or(|at| at > self.blocks.len());
        // A scroll container drives its offset in each prepaint.
        let skips = mem::replace(&mut self.skips, outer)
            && slot
                .events
                .as_ref()
                .is_none_or(|events| events.scroll.is_none())
            && inner.is_none_or(|at| at > self.blocks.len())
            && self.stacks.last().map(Vec::len) == listed
            && slot.hooks.as_ref().is_none_or(|hooks| hooks.skippable());
        if slot
            .hooks
            .as_ref()
            .is_some_and(|hooks| hooks.skips_inside())
        {
            self.touched.push(index);
        }
        slot.skips = skips;
        slot.regions = Some((self.prepaints, [regions, window.regions_len()]));
        slot.touched = [touched, self.touched.len() as u32];
        self.skips &= skips;
    }

    fn prepaint_box(&mut self, index: u32, style: &Style, covered: bool, window: &mut Window) {
        let origin = self.slots[index as usize].origin;
        let [bounds, _] = self.place(index, origin, window);
        let scale = window.scale_factor();
        let slot = &self.slots[index as usize];
        let scroll = slot
            .events
            .as_ref()
            .and_then(|events| events.scroll.clone());
        let needs_hitbox = Events::needs_region(slot.events.as_deref(), style);
        let client = unsnapped(&slot.layout, scale);
        let content = match &scroll {
            Some(_) if !slot.children.is_empty() => self.content(index, origin, scale),
            _ => client,
        };
        // A box that spans columns takes pointer events in its first part.
        let first = parts(&self.slots[index as usize], bounds.origin, scale)
            .next()
            .unwrap_or(bounds);
        window.scope(Scope::Clip(style.overflow_mask(bounds)), |window| {
            let region = needs_hitbox.then(|| place_region(window, style, first, None));
            let offset = match &scroll {
                Some(handle) => {
                    handle.drive(window);
                    self.anchor(index, handle, scale);
                    handle.clamp(style, bounds, client, content, scale)
                }
                None => Point::default(),
            };
            self.slots[index as usize].region = region;
            if style.display != Display::None {
                if style.stacking {
                    let mut list = self.layers.remove(&index).unwrap_or_default();
                    list.clear();
                    self.stacks.push(list);
                    // Its z-indexed boxes paint inside its scroll.
                    let scrolls = (self.slots[index as usize].events.as_ref())
                        .is_some_and(|events| events.scroll.is_some());
                    let depth = window.depth() + usize::from(scrolls && window.opens_interest());
                    self.depths.push((depth, false));
                }
                if style.contains != 0 {
                    (self.blocks).push((
                        style.contains,
                        window.content_mask(),
                        window.depth() as u32,
                    ));
                }
                let interest = scroll.map(|_| (bounds.origin, offset));
                window.scope(Scope::Offset(offset), |window| {
                    window.with_interest(interest, |window| {
                        for child in 0..self.slots[index as usize].children.len() {
                            let child = index_of(self.slots[index as usize].children[child]);
                            if self.slots[child as usize].style.escape != Escape::OUTLINE {
                                self.prepaint_node(child, origin, covered, window);
                            }
                        }
                    })
                });
                // Outlines don't scroll with the content.
                for child in 0..self.slots[index as usize].children.len() {
                    let child = index_of(self.slots[index as usize].children[child]);
                    if self.slots[child as usize].style.escape == Escape::OUTLINE {
                        self.prepaint_node(child, origin, covered, window);
                    }
                }
                if style.contains != 0 {
                    self.blocks.pop();
                }
                if style.stacking {
                    let mut list = self.stacks.pop().unwrap_or_default();
                    if self.depths.pop().is_some_and(|(_, dirty)| dirty) {
                        self.still = false;
                    }
                    list.sort_by_key(|(order, ..)| *order);
                    self.layers.insert(index, list);
                }
            }
        });
        let below = self.slots[index as usize].children.iter().any(|&child| {
            let child = &self.slots[index_of(child) as usize];
            child.below
        });
        let lively = self.slots[index as usize].children.iter().any(|&child| {
            let child = &self.slots[index_of(child) as usize];
            child.volatile || child.lively
        });
        let slot = &mut self.slots[index as usize];
        (slot.below, slot.lively) = (below, lively);
    }

    // The near and far corners of a box and of the overflow that it doesn't clip, in the layout units of its
    // parent. Without `nested`, only the content of its lines counts, not of boxes inside it.
    fn far(&mut self, index: u32, scale: f32, nested: bool) -> [Point<f32>; 2] {
        let slot = &self.slots[index as usize];
        let Layout {
            mut location, size, ..
        } = slot.layout;
        if let Some(split) = &slot.split {
            (location.x, location.y) = (location.x + split.shift.x, location.y + split.shift.y);
        }
        let placed = |[near, far]: [Point<f32>; 2]| {
            [
                point(location.x + near.x, location.y + near.y),
                point(location.x + far.x, location.y + far.y),
            ]
        };
        if let Some(corners) = slot.far[nested as usize].filter(|_| !self.full) {
            return placed(corners);
        }
        let visible = slot
            .style
            .overflow
            .map(|overflow| overflow == Overflow::Visible);
        // The flow of a column set is one tall column, and only the boxes that it moved into columns count.
        let flow = slot.parent != NONE && self.slots[slot.parent as usize].style.columns.is_some();
        let parted = slot
            .split
            .as_ref()
            .is_some_and(|split| !split.rects.is_empty());
        let mut far = match flow || parted {
            true => point(size.width, 0.),
            false => point(size.width, size.height),
        };
        let mut near = point(0., 0.);
        for part in slot.split.iter().flat_map(|split| &split.rects) {
            far = far.max(point(part.right(), part.bottom()));
            near = near.min(point(part.left(), part.top()));
        }
        if !visible.x && !visible.y {
            let corners = self.painted(index, nested, scale, [near, far]);
            self.slots[index as usize].far[nested as usize] = Some(corners);
            return placed(corners);
        }
        // Without `nested`, only anonymous line boxes, which have no hooks, hold content of the scroll container.
        let descend = nested || slot.hooks.is_none() && slot.style.inline != Inline::None;
        if slot.measure.is_some() {
            let sides = slot.layout.padding + slot.layout.border;
            let text = slot
                .state
                .as_ref()
                .and_then(|state| state.downcast_ref::<TextState>())
                .and_then(|state| state.layout.overflow())
                .map_or(0., |width| width.0 * scale);
            let width = slot.measured.width.max(text);
            far = far.max(point(sides.left + width, sides.top + slot.measured.height));
            // Lines that overflow start at their start edge, so right-to-left ones overflow to the left.
            if slot
                .hooks
                .as_ref()
                .is_some_and(|hooks| hooks.overflows_left())
            {
                near.x = near.x.min(size.width - sides.right - width);
            }
        }
        if descend {
            for child in 0..self.slots[index as usize].children.len() {
                let child = index_of(self.slots[index as usize].children[child]);
                if self.slots[child as usize].style.escape & (Escape::FIXED | Escape::OUTLINE) == 0
                {
                    let [child_near, child_far] = self.far(child, scale, nested);
                    (near, far) = (near.min(child_near), far.max(child_far));
                }
            }
        }
        let near = point(
            if visible.x { near.x } else { 0. },
            if visible.y { near.y } else { 0. },
        );
        let far = point(
            if visible.x { far.x } else { size.width },
            if visible.y { far.y } else { size.height },
        );
        let corners = self.painted(index, nested, scale, [near, far]);
        self.slots[index as usize].far[nested as usize] = Some(corners);
        placed(corners)
    }

    // Like Chrome, lines take the boxes where layout placed them, and the overflow takes them where they paint.
    fn painted(
        &self,
        index: u32,
        nested: bool,
        scale: f32,
        corners: [Point<f32>; 2],
    ) -> [Point<f32>; 2] {
        let slot = &self.slots[index as usize];
        // An absolute box whose containing block is outside the scroll container counts where layout placed it.
        let escapes = slot.style.escape & Escape::ABSOLUTE != 0
            && std::iter::successors(Some(slot.parent), |&at| {
                Some(self.slots[at as usize].parent)
            })
            .take_while(|&at| at != NONE)
            .map(|at| &self.slots[at as usize].style)
            .find(|style| {
                style.contains & Escape::ABSOLUTE != 0
                    || style.overflow.x != Overflow::Visible
                    || style.overflow.y != Overflow::Visible
            })
            .is_none_or(|style| style.contains & Escape::ABSOLUTE == 0);
        match slot.hooks.as_ref().filter(|_| nested && !escapes) {
            Some(hooks) => hooks
                .reach(
                    Size {
                        width: Pixels(slot.layout.size.width / scale),
                        height: Pixels(slot.layout.size.height / scale),
                    },
                    corners.map(|at| point(Pixels(at.x / scale), Pixels(at.y / scale))),
                )
                .map(|at| at.map(|value| value.0 * scale)),
            None => corners,
        }
    }

    // The scrollable content of a scroll container. Like Chrome, it starts at the start of its content box, or at
    // the end on an axis where the box scrolls from its end, and overflow on the other side can't be reached. Its
    // end padding follows the margin boxes of its children and its lines, but not the overflow of boxes inside them.
    fn content(&mut self, index: u32, origin: Point<f32>, scale: f32) -> Size<Pixels> {
        let slot = &self.slots[index as usize];
        let (layout, reverse) = (slot.layout, slot.style.scrolls_from_end());
        let (padding, border) = (layout.padding, layout.border);
        // The extent before it snaps to device pixels, which `clamp` rounds like Chrome.
        let start = point(
            origin.x + border.left + padding.left,
            origin.y + border.top + padding.top,
        );
        let end = point(
            origin.x + layout.size.width - border.right - padding.right,
            origin.y + layout.size.height - border.bottom - padding.bottom,
        );
        let (mut min, mut max) = (end, start);
        for child in 0..self.slots[index as usize].children.len() {
            let id = index_of(self.slots[index as usize].children[child]);
            let child = &self.slots[id as usize];
            let (layout, absolute) = (child.layout, child.style.position == Position::Absolute);
            if child.style.escape & (Escape::FIXED | Escape::OUTLINE) != 0
                || layout.size.width == 0. && layout.size.height == 0. && child.children.is_empty()
            {
                continue;
            }
            let [mut near, mut far] = self.far(id, scale, false);
            if !absolute {
                far.x += layout.margin.right.max(0.);
                far.y += layout.margin.bottom.max(0.);
                near.x -= layout.margin.left.max(0.);
                near.y -= layout.margin.top.max(0.);
            }
            let [inner_near, inner_far] = self.far(id, scale, true);
            far = far.max(point(
                inner_far.x - padding.right,
                inner_far.y - padding.bottom,
            ));
            near = near.min(point(
                inner_near.x + padding.left,
                inner_near.y + padding.top,
            ));
            min = min.min(point(origin.x + near.x, origin.y + near.y));
            max = max.max(point(origin.x + far.x, origin.y + far.y));
        }
        let axis = |reverse: bool, start: f32, end: f32, min: f32, max: f32| match reverse {
            true => end - min.min(end),
            false => max.max(start) - start,
        };
        // `clamp` takes the scroll range from the border box.
        size(
            axis(reverse[0], start.x, end.x, min.x, max.x) + border.left + border.right,
            axis(reverse[1], start.y, end.y, min.y, max.y) + border.top + border.bottom,
        )
        .map(|value| Pixels(value / scale))
    }

    fn place(&mut self, index: u32, origin: Point<f32>, window: &mut Window) -> [Rect<Pixels>; 2] {
        let [bounds, exact] = self.bounds(index, origin, window);
        let scale = window.scale_factor();
        let slot = &mut self.slots[index as usize];
        slot.painted = bounds;
        if let Some(state) = slot
            .state
            .as_ref()
            .and_then(|state| state.downcast_ref::<TextState>())
        {
            let [top, right, bottom, left] = inset(&slot.layout, scale);
            let sides = slot.layout.padding + slot.layout.border;
            state.fit(
                px((slot.layout.size.width - sides.left - sides.right) / scale),
                window.text_system(),
                scale,
            );
            state.layout.place(Rect::new(
                exact.origin + point(left, top),
                size(
                    bounds.size.width - left - right,
                    bounds.size.height - top - bottom,
                ),
            ));
        }
        [bounds, exact]
    }

    fn keep(&mut self, index: u32, parent: Point<f32>, window: &mut Window) {
        let slot = &mut self.slots[index as usize];
        let location = slot.layout.location;
        let mut origin = point(parent.x + location.x, parent.y + location.y);
        if let Some(split) = &slot.split {
            origin += split.shift;
        }
        let (escapes, own, foreign) = (
            self.escapes.take(),
            self.escape(index, window),
            self.foreign.take(),
        );
        let placement = window.placement();
        let slot = &mut self.slots[index as usize];
        let (moved, clipped, delta, follows) =
            slot.placed
                .map_or((true, true, None, false), |(_, at, placed)| {
                    let moved = (at, placed.offset) != (origin, placement.offset);
                    let delta = (at == origin).then(|| placement.offset - placed.offset);
                    // The mask moved with the scroll offset, as the interest area of a scroll does.
                    let follows = delta.is_some_and(|by| {
                        let near = |a: Point<Pixels>, b: Point<Pixels>| {
                            (a.x - b.x).0.abs() < 1e-3 && (a.y - b.y).0.abs() < 1e-3
                        };
                        let [old, now] = [placed.mask, placement.mask];
                        let empty = old.rounded.size == Size::default();
                        near(old.bounds.origin + by, now.bounds.origin)
                            && old.bounds.size == now.bounds.size
                            && (empty || near(old.rounded.origin + by, now.rounded.origin))
                            && old.rounded.size == now.rounded.size
                            && old.radii == now.radii
                    });
                    (
                        moved,
                        moved
                            || (placed.mask, placed.visible) != (placement.mask, placement.visible),
                        delta,
                        follows,
                    )
                });
        slot.placed = Some((self.prepaints, origin, placement));
        if moved {
            slot.origin = origin;
            let [bounds, exact] = self.bounds(index, origin, window);
            let slot = &mut self.slots[index as usize];
            (slot.bounds, slot.exact) = (bounds, exact);
        }
        if let Some(order) = self.slots[index as usize].style.z_index
            && let Some(list) = self.stacks.last_mut()
        {
            list.push((
                order,
                index,
                window.visible_below(self.depths.last().map_or(0, |&(depth, _)| depth)),
            ));
        }
        // A box whose boxes all moved together by a scroll offset copies its regions moved, and its hooks move what
        // they gave the page. The boxes inside keep their old places until a prepaint goes through them, so paint
        // must copy the box: if it can't, the frame starts again without such moves.
        let slot = &self.slots[index as usize];
        let [first, last] = slot.touched.map(|at| at as usize);
        if let Some(by) = delta.filter(|by| *by != Point::default())
            && follows
            && !self.full
            && !self.unshift
            && self.copies
            && slot.skips
            && slot.flat
            && placement.transform == 0
            // Paint copies the box, or leaves it out.
            && ((slot.chunk).is_some_and(|chunk| {
                let since = match slot.calm == self.prepaints.wrapping_sub(1) {
                    true => slot.since,
                    false => self.prepaints,
                };
                chunk.translates && chunk.pass == self.paints.0 && since <= chunk.prepass.wrapping_add(1)
            })
                || slot.reach.is_some_and(|reach| !(reach + by).intersects(placement.mask.bounds)))
            && let Some(hooks) = slot.hooks.clone()
            && let Some((pass, regions)) = slot.regions
            && (first == last || pass == self.last_touched.0 && last <= self.last_touched.1.len())
            && let Some(start) = window.shift_regions(pass, regions, by)
        {
            if hooks.shift(by, slot.exact, window) {
                let touched = self.touched.len() as u32;
                for at in first..last {
                    let inside = self.last_touched.1[at];
                    self.touched.push(inside);
                    let slot = &self.slots[inside as usize];
                    if let Some(state) =
                        (slot.state.as_ref()).and_then(|state| state.downcast_ref::<TextState>())
                    {
                        state.layout.shift(by);
                    }
                    if inside != index
                        && let Some(hooks) = slot.hooks.clone()
                    {
                        hooks.shift(by, slot.exact, window);
                    }
                }
                self.calm(index);
                let slot = &mut self.slots[index as usize];
                // Paint places the box where its hooks moved it, like a translate, so the scroll moves that place.
                slot.painted.origin += by;
                slot.reach = (slot.reach).map(|reach| Rect::new(reach.origin + by, reach.size));
                if let Some(region) = &mut slot.region {
                    region.bounds.origin += by;
                }
                slot.regions = Some((self.prepaints, [start, window.regions_len()]));
                slot.touched = [touched, self.touched.len() as u32];
                slot.skipped = self.prepaints;
                slot.lag += by;
                self.shifting &= slot.flat;
                if slot.escaped.is_some() {
                    self.unescape(window);
                }
                let min = |a: Option<usize>, b: Option<usize>| {
                    a.zip(b).map(|(a, b)| a.min(b)).or(a).or(b)
                };
                self.escapes = min(escapes, own);
                let painted = own.filter(|_| self.slots[index as usize].style.z_index.is_none());
                self.foreign = min(foreign, painted);
                self.still &= delta == self.delta;
                return;
            }
            window.truncate_regions(start as usize);
        }
        // A box whose boxes all stay where they were copies its regions, and its hooks keep what it gave
        // its ancestors.
        let slot = &self.slots[index as usize];
        if !clipped
            && slot.skips
            && !(self.unshift && slot.lag != Point::default())
            && let Some((pass, regions)) = slot.regions
            && (first == last || pass == self.last_touched.0 && last <= self.last_touched.1.len())
            && let Some(start) = window.copy_regions(pass, regions)
        {
            let skip = |slot: &Slot, window: &mut Window| {
                (slot.hooks.clone()).is_some_and(|hooks| hooks.skip(slot.exact, window))
            };
            if skip(slot, window) {
                let touched = self.touched.len() as u32;
                for at in first..last {
                    let inside = self.last_touched.1[at];
                    self.touched.push(inside);
                    if inside != index {
                        skip(&self.slots[inside as usize], window);
                    }
                }
                self.calm(index);
                let slot = &mut self.slots[index as usize];
                slot.regions = Some((self.prepaints, [start, window.regions_len()]));
                slot.touched = [touched, self.touched.len() as u32];
                slot.skipped = self.prepaints;
                self.shifting &= slot.flat;
                if slot.escaped.is_some() {
                    self.unescape(window);
                }
                let min = |a: Option<usize>, b: Option<usize>| {
                    a.zip(b).map(|(a, b)| a.min(b)).or(a).or(b)
                };
                self.escapes = min(escapes, own);
                let painted = own.filter(|_| self.slots[index as usize].style.z_index.is_none());
                self.foreign = min(foreign, painted);
                self.still &= delta == self.delta;
                return;
            }
            window.truncate_regions(start as usize);
        }
        let (outer, still, parent) = (
            mem::replace(&mut self.skips, true),
            mem::replace(&mut self.still, delta.is_some()),
            // The boxes inside last moved with this box before its moves without a prepaint.
            mem::replace(
                &mut self.delta,
                delta.map(|by| by + mem::take(&mut self.slots[index as usize].lag)),
            ),
        );
        let (regions, listed) = (window.regions_len(), self.stacks.last().map(Vec::len));
        let (touched, [unsettled, transforms]) =
            (self.touched.len() as u32, window.prepaint_mark());
        let shifting = mem::replace(&mut self.shifting, true);
        self.visit(
            index,
            window,
            |hooks, cx, inner| hooks.kept(cx, inner),
            |tree, style, window| tree.keep_box(index, style, moved, window),
        );
        self.flatten(index, transforms, shifting, window);
        self.still &= window.prepaint_mark()[0] == unsettled;
        // The volatile boxes inside can reach elsewhere.
        if moved || self.slots[index as usize].lively {
            self.reach(index, window.scale_factor());
        }
        if self.slots[index as usize].escaped.is_some() {
            self.unescape(window);
        }
        self.delta = parent;
        // A calm box and all boxes in it stayed, or moved together with the scroll offset. An out-of-flow box
        // takes the clip of its containing block, which can change while its parent stays.
        let calm = mem::replace(&mut self.still, still) && !(own.is_some() && clipped && !follows);
        if calm {
            self.calm(index);
        }
        self.still &= calm && delta == parent;
        self.recorded(
            index,
            [regions, touched],
            listed,
            (outer, escapes, own, foreign),
            window,
        );
    }

    fn keep_box(&mut self, index: u32, style: &Style, moved: bool, window: &mut Window) {
        let origin = self.slots[index as usize].origin;
        let [bounds, _] = match moved {
            true => self.place(index, origin, window),
            false => [self.slots[index as usize].painted; 2],
        };
        let scale = window.scale_factor();
        let first = parts(&self.slots[index as usize], bounds.origin, scale)
            .next()
            .unwrap_or(bounds);
        window.scope(Scope::Clip(style.overflow_mask(bounds)), |window| {
            let slot = &mut self.slots[index as usize];
            if let Some(region) = &slot.region {
                slot.region = Some(place_region(window, style, first, Some(region)));
            }
            if style.display != Display::None {
                if style.stacking {
                    let mut list = self.layers.remove(&index).unwrap_or_default();
                    list.clear();
                    self.stacks.push(list);
                    // Its z-indexed boxes paint inside its scroll.
                    let scrolls = (self.slots[index as usize].events.as_ref())
                        .is_some_and(|events| events.scroll.is_some());
                    let depth = window.depth() + usize::from(scrolls && window.opens_interest());
                    self.depths.push((depth, false));
                }
                if style.contains != 0 {
                    (self.blocks).push((
                        style.contains,
                        window.content_mask(),
                        window.depth() as u32,
                    ));
                }
                // A scroll container drives its offset as in `prepaint_box`.
                let scroll = (self.slots[index as usize].events.as_ref())
                    .and_then(|events| events.scroll.clone());
                let offset = scroll.as_ref().map(|handle| {
                    handle.drive(window);
                    self.anchor(index, handle, scale);
                    let client = unsnapped(&self.slots[index as usize].layout, scale);
                    let content = match self.slots[index as usize].children.is_empty() {
                        true => client,
                        false => self.content(index, origin, scale),
                    };
                    handle.clamp(style, bounds, client, content, scale)
                });
                let interest = offset.map(|offset| (bounds.origin, offset));
                window.scope(Scope::Offset(offset.unwrap_or_default()), |window| {
                    window.with_interest(interest, |window| {
                        self.keep_children(index, origin, false, window)
                    })
                });
                self.keep_children(index, origin, true, window);
                if style.contains != 0 {
                    self.blocks.pop();
                }
                if style.stacking {
                    let mut list = self.stacks.pop().unwrap_or_default();
                    if self.depths.pop().is_some_and(|(_, dirty)| dirty) {
                        self.still = false;
                    }
                    list.sort_by_key(|(order, ..)| *order);
                    self.layers.insert(index, list);
                }
            }
        });
    }

    // Volatile boxes prepaint again, and outlines go after the content, as in `prepaint_box`.
    fn keep_children(
        &mut self,
        index: u32,
        origin: Point<f32>,
        outlines: bool,
        window: &mut Window,
    ) {
        for child in 0..self.slots[index as usize].children.len() {
            let child = index_of(self.slots[index as usize].children[child]);
            let slot = &self.slots[child as usize];
            if (slot.style.escape == Escape::OUTLINE) != outlines {
            } else if slot.volatile {
                // A listed box paints in its stacking context, not in the chunk of this box.
                let listed = slot.style.z_index.is_some() && !self.stacks.is_empty();
                self.prepaint_node(child, origin, false, window);
                match listed {
                    true => self
                        .depths
                        .last_mut()
                        .into_iter()
                        .for_each(|(_, dirty)| *dirty = true),
                    false => self.still = false,
                }
                self.skips = false;
            } else {
                self.keep(child, origin, window);
            }
        }
    }

    // Text can overflow horizontally and one glyph line vertically. Extend its reach across the window.
    fn reach(&mut self, index: u32, scale: f32) {
        let slot = &self.slots[index as usize];
        let reach = slot
            .hooks
            .as_ref()
            .is_none_or(|hooks| hooks.bounded())
            .then(|| {
                let style = &slot.style;
                let grow = style.box_shadow.iter().fold(px(0.), |grow, shadow| {
                    grow.max(
                        shadow.offset.x.abs()
                            + shadow.offset.y.abs()
                            + shadow.blur_radius * 2.
                            + shadow.spread_radius,
                    )
                });
                let mut reach = slot.painted.outset(grow);
                for part in parts(slot, slot.painted.origin, scale) {
                    reach = reach.union(part.outset(grow));
                }
                if matches!(slot.kind, Kind::Text(..) | Kind::Measured) {
                    let line = slot.text.line_height.round() * 2.;
                    reach = Rect::new(
                        point(px(-1e9), reach.origin.y - line),
                        size(px(2e9), reach.size.height + line * 2.),
                    );
                }
                reach
            });
        let reach = reach.and_then(|reach| {
            slot.children.iter().try_fold(reach, |reach, &child| {
                Some(reach.union(self.slots[index_of(child) as usize].reach?))
            })
        });
        self.slots[index as usize].reach = reach;
    }

    pub(super) fn visit(
        &mut self,
        index: u32,
        window: &mut Window,
        step: impl FnOnce(&dyn Hooks, &mut NodeCx<'_>, &mut dyn FnMut(&mut Window)),
        mut inner: impl FnMut(&mut Self, &Style, &mut Window),
    ) {
        let slot = &self.slots[index as usize];
        let style = slot.style.clone();
        let Some(hooks) = slot.hooks.clone() else {
            return inner(self, &style, window);
        };
        let (text, state, split) = (slot.text.clone(), slot.state.clone(), slot.split.clone());
        let scale = window.scale_factor();
        let mut cx = NodeCx {
            window,
            bounds: slot.bounds,
            exact: slot.exact,
            inset: inset(&slot.layout, scale),
            baselines: self.baselines(index, scale),
            text: &text,
            style: &style,
            state: state.as_ref(),
            split: split.as_deref(),
        };
        step(&*hooks, &mut cx, &mut |window| inner(self, &style, window));
    }

    pub(super) fn baselines(&self, index: u32, scale: f32) -> [Option<Pixels>; 2] {
        let Baselines { first, last } = self.slots[index as usize].baselines;
        [first, last].map(|value| value.map(|value| Pixels(value / scale)))
    }
}
