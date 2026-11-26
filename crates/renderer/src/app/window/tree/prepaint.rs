use super::*;

impl Tree {
    pub(super) fn checked(&self) -> bool {
        #[cfg(debug_assertions)]
        return self.checked;
        #[cfg(not(debug_assertions))]
        false
    }

    pub(crate) fn prepaint(&mut self, root: NodeId, window: &mut Window) {
        self.prepaints = self.prepaints.wrapping_add(1);
        // Like the restarts of release builds, some passes go through the boxes that scrolls moved.
        #[cfg(debug_assertions)]
        {
            self.unshift |= self.prepaints % 5 == 2;
        }
        window.begin_prepaint(self.prepaints);
        mem::swap(&mut self.touched, &mut self.last_touched.1);
        self.last_touched.0 = self.prepaints.wrapping_sub(1);
        self.touched.clear();
        self.stacks.clear();
        self.depths.clear();
        self.blocks.clear();
        self.scrolled.clear();
        self.hidden.clear();
        self.anchoring.clear();
        #[cfg(debug_assertions)]
        let start = window.region_places().len();
        self.copies = window.copies();
        window.take_marks();
        self.prepaint_node(index_of(root), Point::default(), false, window);
        self.unshift = false;
        for (mark, at, rtl) in window.take_marks() {
            if let Some(&index) = self.marks.get(&mark)
                && (self.slots[index as usize].style.mark).is_some_and(|mark| mark.inline)
            {
                let slot = &mut self.slots[index as usize];
                let shift = slot.shift;
                let at = point(at.x.0, at.y.0) * window.scale_factor();
                if (shift.x - at.x).abs() > 0.05
                    || (shift.y - at.y).abs() > 0.05
                    || slot.turned != rtl
                {
                    (slot.shift, slot.turned) = (at, rtl);
                    window.rebar();
                }
            }
        }
        // The check lays out all boxes again, so it checks every third pass, and state that a skip keeps reaches it.
        // Hooks then hold the state of the full pass, which copies don't show, so the next paint draws all boxes.
        #[cfg(debug_assertions)]
        if !window.redraws() && self.prepaints.is_multiple_of(3) {
            self.checked = true;
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
            self.scrolled.clear();
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
        self.rebase(index, window);
        let slot = &mut self.slots[index as usize];
        // The boxes inside a box that a skip kept in the last pass are as they were, also when the box itself
        // was built again since.
        let covered = !self.full && (covered || slot.skipped == prepaints.wrapping_sub(1));
        let outer = (
            [
                mem::replace(&mut self.skips, true),
                mem::replace(&mut self.nests, false),
            ],
            self.escapes.take(),
            self.escape(index, window),
            self.foreign.take(),
        );
        let (regions, touched) = (window.regions_len(), self.touched.len() as u32);
        let placement = window.placement();
        let slot = &mut self.slots[index as usize];
        slot.placed = Some((prepaints, origin, placement));
        slot.origin = origin;
        let lag = mem::take(&mut slot.lag);
        // A listed box comes before the listed boxes inside it, which paint above it at the same z-index.
        slot.listed = slot.style.z_index.is_some() && !self.stacks.is_empty();
        if let Some(order) = slot.style.z_index
            && let Some(list) = self.stacks.last_mut()
        {
            slot.list_mask = window.content_mask();
            let depth = self.depths.last().map_or(0, |&(depth, _)| depth);
            list.push((
                order,
                index,
                window.cut_between(slot.list_mask, depth, usize::MAX),
            ));
        }
        let [bounds, exact] = self.bounds(index, origin, window);
        let slot = &mut self.slots[index as usize];
        (slot.bounds, slot.exact) = (bounds, exact);
        let (listed, mark) = (self.stacks.last().map(Vec::len), window.prepaint_mark());
        let shifting = mem::replace(&mut self.shifting, true);
        // The boxes inside take the moves of this box without a prepaint, as in `keep`.
        self.lags.push(lag);
        self.visit(
            index,
            window,
            |hooks, cx, inner| hooks.prepaint(cx, inner),
            |tree, style, window| tree.prepaint_box(index, style, covered, window),
        );
        self.lags.pop();
        self.flatten(index, mark, shifting, window);
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
        let mut unscroll = Point::default();
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
            let around = self.scrolled.last().copied().unwrap_or_default();
            unscroll = around
                - match at {
                    Some(at) => self.blocks[at].3,
                    None if escape & Escape::FIXED != 0 => Point::default(),
                    // The initial containing block scrolls with the page.
                    None => self.scrolled.first().copied().unwrap_or_default(),
                };
            (Some(mask), Some(at.map_or(0, |at| at + 1)), cut)
        } else {
            (None, None, 0)
        };
        // A listed box paints in its stacking context, outside the scrolls between that and its containing block.
        let cuts = escaped.is_some()
            && slot.style.z_index.is_some()
            && (self.depths.last()).is_some_and(|&(context, _)| context < cut as usize);
        let escaped = match self.depths.last() {
            Some(&(context, _)) if cuts => {
                escaped.map(|mask| window.cut_between(mask, context, cut as usize))
            }
            _ => escaped,
        };
        if let Some(mask) = escaped {
            window.push_content_mask(mask);
            self.hidden.push(window.hide_viewports(cut as usize));
        }
        let slot = &mut self.slots[index as usize];
        (slot.escaped, slot.cut, slot.unscroll) = (escaped, cut, unscroll);
        slot.cuts = cuts;
        depth
    }

    fn scrolled(&self, offset: Point<Pixels>) -> Point<Pixels> {
        self.scrolled.last().copied().unwrap_or_default() + offset
    }

    // Ends `escape`.
    fn unescape(&mut self, window: &mut Window) {
        window.pop_content_mask();
        if let Some(hidden) = self.hidden.pop() {
            window.show_viewports(hidden);
        }
    }

    // A box is flat when no hit transform is in it and all hooks in it can shift.
    fn flatten(&mut self, index: u32, mark: PrepaintMark, outer: bool, window: &Window) {
        let slot = &mut self.slots[index as usize];
        slot.flat = mem::replace(&mut self.shifting, outer)
            && !window.transformed_since(mark)
            && slot.hooks.as_ref().is_none_or(|hooks| hooks.shifts());
        self.shifting &= slot.flat;
    }

    // The box was calm in the last pass, or in the pass `visited` that last went through it: a skip or a shift of
    // a box around it kept it from the passes since, and they keep what is in a box.
    fn ran(&self, index: u32, visited: Option<u32>) -> bool {
        let calm = self.slots[index as usize].calm;
        calm == self.prepaints.wrapping_sub(1) || Some(calm) == visited
    }

    fn calm(&mut self, index: u32, visited: Option<u32>) {
        if !self.ran(index, visited) {
            self.slots[index as usize].since = self.prepaints;
        }
        self.slots[index as usize].calm = self.prepaints;
    }

    // Keeps what a prepaint of a box gave, for a skip of the box in a later pass.
    fn recorded(
        &mut self,
        index: u32,
        [regions, touched]: [u32; 2],
        listed: Option<usize>,
        ([outer, around], escapes, own, foreign): Outer,
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
        slot.escaping = inner
            .filter(|&at| at <= self.blocks.len())
            .map(|at| at as u32);
        let skips = mem::replace(&mut self.skips, outer)
            && slot.hooks.as_ref().is_none_or(|hooks| hooks.skippable());
        let list = match (self.stacks.last(), listed) {
            (Some(list), Some(at)) => &list[at.min(list.len())..],
            _ => &[],
        };
        if slot.listing.len() != list.len()
            || slot
                .listing
                .iter()
                .zip(list)
                .any(|(&at, entry)| at != entry.1)
        {
            slot.listing = list.iter().map(|entry| entry.1).collect();
        }
        // A scroll container drives its offset in each prepaint, so a skip of a box around it asks if it rests.
        // The listed boxes in it take the cut of its viewport, which only its own prepaint gives.
        let scrolls = (slot.events.as_ref()).is_some_and(|events| events.scroll.is_some());
        let skips = skips && !(scrolls && !list.is_empty());
        slot.nests = mem::replace(&mut self.nests, around);
        self.nests |= slot.nests || scrolls;
        if scrolls || (slot.hooks.as_ref()).is_some_and(|hooks| hooks.skips_inside()) {
            self.touched.push(index);
        }
        slot.skips = skips;
        slot.regions = Some((self.prepaints, [regions, window.regions_len()]));
        slot.touched = [touched, self.touched.len() as u32];
        (slot.base_regions, slot.base_touched) = (slot.regions, slot.touched);
        (slot.own_regions, slot.own_touched) = (slot.regions, slot.touched);
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
                        self.scrolled(offset),
                        None,
                    ));
                    self.slots[index as usize].inner = window.offset() + offset;
                }
                let scrolls = scroll.is_some();
                if scrolls {
                    self.scrolled.push(self.scrolled(offset));
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
                if scrolls {
                    self.scrolled.pop();
                }
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
        let outset = slot.style.clips_past_padding().cloned();
        if !visible.x && !visible.y && outset.is_none() {
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
                if self.slots[child as usize].style.escape & Escape::OUTLINE == 0
                    && !self.unscrolled(child)
                {
                    let [child_near, child_far] = self.far(child, scale, nested);
                    (near, far) = (near.min(child_near), far.max(child_far));
                }
            }
        }
        let room = outset.map_or_else(Default::default, |crate::ClipMargin { sides, reach }| {
            let border = &self.slots[index as usize].style.border_widths;
            [
                sides.left - border.left,
                sides.top - border.top,
                sides.right - border.right,
                sides.bottom - border.bottom,
            ]
            .map(|side| ((side + reach).0 * scale).max(0.))
        });
        let near = point(
            if visible.x {
                near.x
            } else {
                near.x.max(-room[0])
            },
            if visible.y {
                near.y
            } else {
                near.y.max(-room[1])
            },
        );
        let far = point(
            if visible.x {
                far.x
            } else {
                far.x.min(size.width + room[2])
            },
            if visible.y {
                far.y
            } else {
                far.y.min(size.height + room[3])
            },
        );
        let corners = self.painted(index, nested, scale, [near, far]);
        self.slots[index as usize].far[nested as usize] = Some(corners);
        placed(corners)
    }

    // Whether a scroll container around an out-of-flow box is inside its containing block, so it doesn't scroll the
    // box. The page scrolls the absolute boxes without another containing block.
    fn unscrolled(&self, index: u32) -> bool {
        let slot = &self.slots[index as usize];
        let clips = |style: &Style| {
            style.overflow.x != Overflow::Visible || style.overflow.y != Overflow::Visible
        };
        let escape = slot.style.escape & Escape::PLACED;
        if escape == 0 {
            return false;
        }
        let up = |at: u32| Some(self.slots[at as usize].parent).filter(|&at| at != NONE);
        let mut outer = std::iter::successors(up(index), |&at| up(at))
            .map(|at| &*self.slots[at as usize].style);
        match outer.find(|style| style.contains & escape != 0 || clips(style)) {
            Some(style) if style.contains & escape != 0 => false,
            _ if escape == Escape::FIXED => true,
            found => found.is_some() && outer.any(|style| clips(style)),
        }
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
        // An out-of-flow box whose containing block is outside the scroll container counts where layout placed it.
        let escape = slot.style.escape & Escape::PLACED;
        let escapes = escape != 0
            && std::iter::successors(Some(slot.parent), |&at| {
                Some(self.slots[at as usize].parent)
            })
            .take_while(|&at| at != NONE)
            .map(|at| &self.slots[at as usize].style)
            .find(|style| {
                style.contains & escape != 0
                    || style.overflow.x != Overflow::Visible
                    || style.overflow.y != Overflow::Visible
            })
            .is_none_or(|style| style.contains & escape == 0);
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
        // The padding without the room of scroll bars, which is no part of the scrolled area.
        let bars = slot.style.bars;
        let own = [
            padding.top - crate::geometry::to_layout_unit(bars.top.0, scale),
            padding.right - crate::geometry::to_layout_unit(bars.right.0, scale),
            padding.bottom - crate::geometry::to_layout_unit(bars.bottom.0, scale),
            padding.left - crate::geometry::to_layout_unit(bars.left.0, scale),
        ];
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
            if child.style.escape & Escape::OUTLINE != 0
                || layout.size.width == 0. && layout.size.height == 0. && child.children.is_empty()
                || self.unscrolled(id)
            {
                continue;
            }
            // The end padding doesn't follow out-of-flow boxes.
            let [mut near, mut far] = match child.style.escape {
                0 => self.far(id, scale, false),
                _ => [end - origin, start - origin],
            };
            if !absolute {
                far.x += layout.margin.right.max(0.);
                far.y += layout.margin.bottom.max(0.);
                near.x -= layout.margin.left.max(0.);
                near.y -= layout.margin.top.max(0.);
            }
            let [inner_near, inner_far] = self.far(id, scale, true);
            far = far.max(point(inner_far.x - own[1], inner_far.y - own[2]));
            near = near.min(point(inner_near.x + own[3], inner_near.y + own[0]));
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

    // A box that no prepaint or paint went through since its parent last did moves with its parent: it takes the
    // moves of its parent without a prepaint, and its regions and paint chunk relocate with the copies of the regions
    // and the chunk of its parent.
    fn rebase(&mut self, index: u32, window: &Window) {
        let parent = self.slots[index as usize].parent;
        if parent == NONE {
            return;
        }
        let slot = &mut self.slots[index as usize];
        let lag = self.lags.last().copied().unwrap_or_default() - mem::take(&mut slot.ahead);
        self.drift(index, lag);
        let (above, slot) = (&self.slots[parent as usize], &self.slots[index as usize]);
        let regions = match (above.regions, above.base_regions, slot.own_regions) {
            (Some((pass, now)), Some((was, base)), Some((at, own)))
                if slot.regions.is_some_and(|(current, _)| current != pass)
                    && pass != was
                    && at == was
                    && base[0] <= own[0]
                    && own[1] <= base[1] =>
            {
                let [first, last] = slot.own_touched;
                let [start, end] = above.base_touched;
                (first == last || start <= first && last <= end).then(|| {
                    let touched = match first == last {
                        true => [0; 2],
                        false => [first, last].map(|at| at - start + above.touched[0]),
                    };
                    (
                        Some((pass, own.map(|at| at - base[0] + now[0]))),
                        touched,
                        at,
                    )
                })
            }
            _ => None,
        };
        let chunk = match (above.chunk, slot.chunk) {
            (Some(outer), Some(chunk))
                if outer.pass == self.paints.0
                    && chunk.pass != outer.pass
                    && outer.base.pass != outer.pass
                    && chunk.origin.pass == outer.base.pass =>
            {
                let (origin, base) = (chunk.origin, outer.base);
                origin.span.rebased(&base.span, &outer.span).map(|span| {
                    let by = outer.at - base.at;
                    let mask = moved_mask(origin.mask, by, base.mask, outer.mask);
                    let key_mask = moved_mask(origin.key.mask, by, base.mask, outer.mask);
                    let scale = window.scale_factor();
                    Chunk {
                        pass: outer.pass,
                        key: (origin.key).moved(outer.key.offset - base.key.offset, key_mask),
                        span,
                        at: origin.at + by,
                        mask,
                        area: origin.area.map(|area| area + by.scale(scale)),
                        ..chunk
                    }
                })
            }
            _ => None,
        };
        // No prepaint went through the box since the one of its regions, or its chunk, as the copies of its parent show.
        let slot = &mut self.slots[index as usize];
        if let Some(chunk) = chunk {
            slot.chunk = Some(chunk);
            (slot.calm, slot.since) = (
                self.prepaints.wrapping_sub(1),
                chunk.prepass.wrapping_add(1),
            );
        }
        if let Some((regions, touched, visit)) = regions {
            (slot.regions, slot.touched) = (regions, touched);
            (slot.calm, slot.since) = (self.prepaints.wrapping_sub(1), visit.wrapping_add(1));
        }
    }

    // Moves a box that a scroll moved without a prepaint. The boxes inside take the move from its `lag`.
    fn drift(&mut self, index: u32, lag: Point<Pixels>) {
        let slot = &mut self.slots[index as usize];
        if lag != Point::default() {
            // A box without boxes in it is all at its new place.
            if !slot.children.is_empty() {
                slot.lag += lag;
            }
            slot.painted.origin += lag;
            slot.bounds.origin += lag;
            slot.exact.origin += lag;
            slot.reach = slot.reach.map(|reach| reach + lag);
            if let Some(region) = &mut slot.region {
                region.bounds.origin += lag;
            }
            if let Some((_, _, placed)) = &mut slot.placed {
                placed.offset += lag;
                if placed.mask.bounds.size.width.0 < crate::gpu::scene::OPEN {
                    placed.mask.bounds.origin += lag;
                }
                if !placed.mask.radii.is_zero() {
                    placed.mask.rounded.origin += lag;
                }
            }
            let hooks = slot.hooks.clone();
            if let Some(hooks) = &hooks {
                hooks.lag(lag);
            }
            // The text of boxes with hooks that skip inside moved with the box that a scroll moved.
            if !hooks.is_some_and(|hooks| hooks.skips_inside())
                && let Some(state) =
                    (slot.state.as_ref()).and_then(|state| state.downcast_ref::<TextState>())
            {
                state.layout.shift(lag);
            }
        }
    }

    fn keep(&mut self, index: u32, parent: Point<f32>, window: &mut Window) {
        self.rebase(index, window);
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
        let visited = slot.placed.map(|(pass, ..)| pass);
        let was = slot
            .placed
            .map_or(placement.mask, |(_, _, placed)| placed.mask);
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
            let mask = window.content_mask();
            self.slots[index as usize].list_mask = mask;
            let depth = self.depths.last().map_or(0, |&(depth, _)| depth);
            list.push((order, index, window.cut_between(mask, depth, usize::MAX)));
        }
        // A box whose boxes all moved together by a scroll offset copies its regions moved, and its hooks move what
        // they gave the page. The boxes inside keep their old places until a prepaint goes through them, so paint
        // must copy the box: if it can't, the frame starts again without such moves.
        let slot = &self.slots[index as usize];
        let [first, last] = slot.touched.map(|at| at as usize);
        // Only the hooks of a box move what the boxes in it gave the page, and tell if the box reads another place.
        let stays = match &slot.hooks {
            Some(hooks) => hooks.stays(slot.exact, slot.unscroll),
            None => slot.children.is_empty(),
        };
        if let Some(by) = delta.filter(|by| *by != Point::default())
            && follows
            && !self.full
            && !self.unshift
            && self.copies
            && slot.skips
            && slot.flat
            && placement.transform == 0
            // Paint copies the box, or leaves it out, as it left it out in the last paint. Below a transform or an
            // effect, paint leaves out no box.
            && ((slot.chunk).is_some_and(|chunk| {
                let since = match self.ran(index, visited) {
                    true => slot.since,
                    false => self.prepaints,
                };
                chunk.translates && chunk.pass == self.paints.0 && since <= chunk.prepass.wrapping_add(1)
            })
                || slot.culled == self.paints.0
                    && (slot.reach)
                        .is_some_and(|reach| !(reach + by).intersects(placement.mask.bounds)))
            && self.relists(index, visited, (by, was, placement.mask), window)
            && stays
            && let Some((pass, regions)) = slot.regions
            && (first == last || pass == self.last_touched.0 && last <= self.last_touched.1.len())
            && !slot.nests
            && self.rests(index, first..last)
            && self.based(index, by)
            && let Some(start) =
                window.shift_regions(pass, regions, by, self.viewport(index, placement.mask))
        {
            // The hooks of the scroll bars read the place of their scroll container.
            self.rested(index, first..last, Some(by));
            let slot = &self.slots[index as usize];
            if (slot.hooks.clone()).is_none_or(|hooks| hooks.shift(by, slot.exact, window)) {
                let touched = self.touched.len() as u32;
                for at in first..last {
                    let inside = self.last_touched.1[at];
                    self.touched.push(inside);
                    let slot = &self.slots[inside as usize];
                    let Some(hooks) = slot.hooks.clone().filter(|hooks| hooks.skips_inside())
                    else {
                        continue;
                    };
                    if let Some(state) =
                        (slot.state.as_ref()).and_then(|state| state.downcast_ref::<TextState>())
                    {
                        state.layout.shift(by);
                    }
                    if inside != index {
                        hooks.shift_inside(by, slot.exact, window);
                    }
                }
                self.calm(index, visited);
                self.relist(index, visited, (by, was, placement.mask), window);
                self.nests |= self.slots[index as usize]
                    .events
                    .as_ref()
                    .is_some_and(|events| events.scroll.is_some());
                let slot = &mut self.slots[index as usize];
                // Paint places the box where its hooks moved it, like a translate, so the scroll moves that place.
                slot.painted.origin += by;
                slot.reach = (slot.reach).map(|reach| Rect::new(reach.origin + by, reach.size));
                if let Some(region) = &mut slot.region {
                    region.bounds.origin += by;
                }
                slot.regions = Some((self.prepaints, [start, window.regions_len()]));
                slot.touched = [touched, self.touched.len() as u32];
                (slot.own_regions, slot.own_touched) = (slot.regions, slot.touched);
                slot.skipped = self.prepaints;
                // Only the boxes inside keep their old places. A box without them has its new place, and its hooks
                // moved theirs, so it can paint. The loop above moved its text if its hooks skip inside.
                match slot.children.is_empty() {
                    true => {
                        slot.lag = Point::default();
                        if !(slot.hooks.as_ref()).is_some_and(|hooks| hooks.skips_inside())
                            && let Some(state) = (slot.state.as_ref())
                                .and_then(|state| state.downcast_ref::<TextState>())
                        {
                            state.layout.shift(by);
                        }
                    }
                    false => slot.lag += by,
                }
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
            && stays
            && !(self.unshift && slot.lag != Point::default())
            && let Some((pass, regions)) = slot.regions
            && (first == last || pass == self.last_touched.0 && last <= self.last_touched.1.len())
            && self.rests(index, first..last)
            && self.based(index, Point::default())
            && let Some(start) = window.copy_regions(pass, regions)
        {
            self.rested(index, first..last, None);
            let slot = &self.slots[index as usize];
            if (slot.hooks.clone()).is_none_or(|hooks| hooks.skip(slot.exact, window)) {
                let touched = self.touched.len() as u32;
                for at in first..last {
                    let inside = self.last_touched.1[at];
                    self.touched.push(inside);
                    let slot = &self.slots[inside as usize];
                    if inside != index
                        && let Some(hooks) = slot.hooks.clone().filter(|hooks| hooks.skips_inside())
                    {
                        hooks.skip_inside(slot.exact, window);
                    }
                }
                self.calm(index, visited);
                self.relist(index, visited, (Point::default(), was, was), window);
                let slot = &self.slots[index as usize];
                self.nests |= slot.nests
                    || (slot.events.as_ref()).is_some_and(|events| events.scroll.is_some());
                let slot = &mut self.slots[index as usize];
                slot.regions = Some((self.prepaints, [start, window.regions_len()]));
                slot.touched = [touched, self.touched.len() as u32];
                (slot.own_regions, slot.own_touched) = (slot.regions, slot.touched);
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
        // The boxes inside take the moves of this box without a prepaint, so they move with it from here.
        self.lags
            .push(mem::take(&mut self.slots[index as usize].lag));
        let (outer, still, parent) = (
            [
                mem::replace(&mut self.skips, true),
                mem::replace(&mut self.nests, false),
            ],
            mem::replace(&mut self.still, delta.is_some()),
            mem::replace(&mut self.delta, delta),
        );
        // The box stayed with its mask, or a scroll moved both.
        let based = match delta {
            Some(by) if by == Point::default() => !clipped,
            Some(_) => follows,
            None => false,
        };
        let (regions, listed) = (window.regions_len(), self.stacks.last().map(Vec::len));
        let (touched, mark) = (self.touched.len() as u32, window.prepaint_mark());
        let shifting = mem::replace(&mut self.shifting, true);
        self.visit(
            index,
            window,
            |hooks, cx, inner| hooks.kept(cx, inner),
            |tree, style, window| tree.keep_box(index, style, [moved, based], window),
        );
        self.lags.pop();
        self.flatten(index, mark, shifting, window);
        self.still &= !window.unsettled_since(mark);
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
            self.calm(index, visited);
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

    // The out-of-flow boxes in a box that moves by `by` keep their places in it: this pass kept their containing
    // blocks around the box, and moved the content of these blocks by `by` too. The initial containing block has
    // no box that tells.
    fn based(&self, index: u32, by: Point<Pixels>) -> bool {
        let near = |a: Pixels, b: Pixels| (a - b).0.abs() < 1e-3;
        self.slots[index as usize].escaping.is_none_or(|at| {
            at > 0
                && (self.blocks.get(at as usize - 1..)).is_some_and(|blocks| {
                    blocks.iter().all(|block| {
                        (block.4).is_some_and(|moved| near(moved.x, by.x) && near(moved.y, by.y))
                    })
                })
        })
    }

    // The scroll containers in a box, and their scroll bars among the hooks that skip inside, are as their last
    // prepaint left them.
    fn rests(&self, index: u32, touched: std::ops::Range<usize>) -> bool {
        let scroll = |slot: &Slot| {
            (slot.events.as_ref())
                .and_then(|events| events.scroll.as_ref())
                .is_none_or(|scroll| scroll.rests())
        };
        let slot = &self.slots[index as usize];
        if !slot.nests
            && slot
                .events
                .as_ref()
                .is_none_or(|events| events.scroll.is_none())
        {
            return true;
        }
        let inside = self.last_touched.1.get(touched).unwrap_or_default();
        scroll(slot)
            && inside.iter().all(|&inside| {
                let slot = &self.slots[inside as usize];
                scroll(slot)
                    && (slot.hooks.as_ref())
                        .is_none_or(|hooks| !hooks.skips_inside() || hooks.rests())
            })
    }

    // The viewport that a scroll container in `mask` gives its content, as `begin_interest` does, and its region.
    fn viewport(&self, index: u32, mask: Clip<Pixels>) -> Option<(Rect<Pixels>, Option<Region>)> {
        let slot = &self.slots[index as usize];
        (slot.events.as_ref()).filter(|events| events.scroll.is_some())?;
        let mask = match slot.style.overflow_mask(slot.bounds) {
            Some(clip) => clip.intersect(&mask),
            None => mask,
        };
        (mask.radii == Default::default()).then_some((mask.bounds, slot.region))
    }

    // The scroll containers in a box that skips, or shifts by `by`, stay in the list of `anchor`, and move.
    fn rested(&mut self, index: u32, touched: std::ops::Range<usize>, by: Option<Point<Pixels>>) {
        let slot = &self.slots[index as usize];
        if !slot.nests
            && slot
                .events
                .as_ref()
                .is_none_or(|events| events.scroll.is_none())
        {
            return;
        }
        for at in touched.start..touched.end.min(self.last_touched.1.len()) {
            let slot = &mut self.slots[self.last_touched.1[at] as usize];
            let Some(scroll) = (slot.events.as_ref()).and_then(|events| events.scroll.as_ref())
            else {
                continue;
            };
            if let Some(by) = by {
                scroll.shift(by);
            }
            if slot.style.scroll_anchor & ScrollAnchor::NONE == 0
                && mem::replace(&mut slot.anchoring, self.prepaints) != self.prepaints
            {
                self.anchoring.push(slot.key);
            }
        }
    }

    // The listed boxes inside a box that a scroll moves by `by`, from mask `was` to `now`, can paint copies of their
    // chunks, or paint nothing.
    fn relists(
        &self,
        index: u32,
        visited: Option<u32>,
        (by, was, now): Mv,
        window: &Window,
    ) -> bool {
        let depth = self.depths.last().map_or(0, |&(depth, _)| depth);
        let listing = &self.slots[index as usize].listing;
        (listing.is_empty() || !self.stacks.is_empty())
            && listing.iter().all(|&at| {
                let slot = &self.slots[at as usize];
                // The viewports that cut the mask of an out-of-flow box don't move with the box.
                if slot.cuts {
                    return false;
                }
                let since = match self.ran(at, visited) {
                    true => slot.since,
                    false => self.prepaints,
                };
                // Paint leaves out a box outside its mask, and an out-of-flow box has the mask of its containing block.
                let mask = match slot.escaped {
                    Some(mask) => moved_mask(mask, by, Clip::default(), Clip::default()),
                    None => window.cut_between(
                        moved_mask(slot.list_mask, by, was, now),
                        depth,
                        usize::MAX,
                    ),
                };
                slot.listed
                    && (slot.chunk.is_some_and(|chunk| {
                        chunk.translates
                            && chunk.pass == self.paints.0
                            && since <= chunk.prepass.wrapping_add(1)
                    }) || slot.culled == self.paints.0
                        && (slot.reach).is_some_and(|reach| !(reach + by).intersects(mask.bounds)))
            })
    }

    // Lists again the boxes that a skip of a box kept, and moves them as `relists` checked.
    fn relist(&mut self, index: u32, visited: Option<u32>, (by, was, now): Mv, window: &Window) {
        let listing = mem::take(&mut self.slots[index as usize].listing);
        let depth = self.depths.last().map_or(0, |&(depth, _)| depth);
        for &at in &listing {
            if by != Point::default() {
                self.drift(at, by);
            }
            let slot = &mut self.slots[at as usize];
            if by != Point::default() {
                slot.ahead += by;
                slot.list_mask = moved_mask(slot.list_mask, by, was, now);
                slot.escaped = slot
                    .escaped
                    .map(|mask| moved_mask(mask, by, Clip::default(), Clip::default()));
            }
            let mask = window.cut_between(slot.list_mask, depth, usize::MAX);
            if let (Some(order), Some(list)) = (slot.style.z_index, self.stacks.last_mut()) {
                list.push((order, at, mask));
            }
            self.calm(at, visited);
        }
        self.slots[index as usize].listing = listing;
    }

    // `kept`: the box and its mask stayed since the last pass, or moved together by a scroll offset.
    fn keep_box(
        &mut self,
        index: u32,
        style: &Style,
        [moved, kept]: [bool; 2],
        window: &mut Window,
    ) {
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
                if style.contains != 0 {
                    // The boxes in the block see this move of the offset, also by the scroll of the block.
                    let inner = window.offset() + offset.unwrap_or_default();
                    let was = mem::replace(&mut self.slots[index as usize].inner, inner);
                    (self.blocks).push((
                        style.contains,
                        window.content_mask(),
                        window.depth() as u32,
                        self.scrolled(offset.unwrap_or_default()),
                        kept.then(|| inner - was),
                    ));
                }
                if let Some(offset) = offset {
                    self.scrolled.push(self.scrolled(offset));
                }
                let interest = offset.map(|offset| (bounds.origin, offset));
                window.scope(Scope::Offset(offset.unwrap_or_default()), |window| {
                    window.with_interest(interest, |window| {
                        self.keep_children(index, origin, false, window)
                    })
                });
                if offset.is_some() {
                    self.scrolled.pop();
                }
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
            unscroll: slot.unscroll,
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
