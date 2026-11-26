use super::*;

impl Tree {
    pub(crate) fn paint(&mut self, root: NodeId, window: &mut Window) {
        self.paints = window.begin_paint();
        self.painting.clear();
        self.deferred.clear();
        self.unbounded = 0;
        self.paint_node(index_of(root), false, window);
        #[cfg(debug_assertions)]
        {
            self.checked = false;
        }
    }

    // An out-of-flow box escapes the clips of the scroll viewports inside its containing block.
    fn paint_node(&mut self, index: u32, listed: bool, window: &mut Window) {
        let slot = &self.slots[index as usize];
        if slot.escaped.is_some() && (slot.cut as usize) < window.depth() {
            let depth = slot.cut as usize;
            window.with_cuts_paused(depth, |window| self.paint_box_node(index, listed, window));
            return;
        }
        self.paint_box_node(index, listed, window);
    }

    fn paint_box_node(&mut self, index: u32, listed: bool, window: &mut Window) {
        let slot = &self.slots[index as usize];
        let escaped = match slot.style.escape == Escape::OUTLINE {
            true => self.masks.last().copied().or(slot.escaped),
            false => slot.escaped,
        }
        .map(|mask| window.group_clipped(mask));
        // A layers node paints the z-indexed boxes of its stacking context, whose reach holds them.
        if slot.listed != listed
            || self.unbounded == 0
                && !matches!(slot.kind, Kind::Layers(_))
                && slot.reach.is_some_and(|reach| {
                    !reach.intersects(escaped.unwrap_or_else(|| window.content_mask()).bounds)
                })
        {
            if slot.listed == listed {
                window.cull();
                self.slots[index as usize].culled = self.paints.0;
            }
            return;
        }
        // A box whose boxes all stayed where they were draws as in the last paint under the same window state. One
        // whose boxes all moved by the same whole device pixels draws it moved, if its paint cut nothing.
        // Debug builds paint it and check that the copy would draw the same.
        let (key, outside) = (window.paint_key(), self.unbounded > 0);
        let (at, mask) = (slot.bounds.origin, escaped.unwrap_or(key.mask));
        // An out-of-flow box takes the clip of its containing block, so the boxes around it can't move a copy of it.
        let foreign = slot.style.escape & !Escape::OUTLINE != 0;
        // No prepaint since the chunk's paint went through the box. A layers node paints boxes that are not in it,
        // so they copy their own chunks.
        let copy = slot
            .chunk
            .filter(|chunk| {
                !matches!(slot.kind, Kind::Layers(_))
                    && !self.checked()
                    // No pass went through a box in a box that skipped since the pass that found it calm.
                    && (slot.calm == self.prepaints
                        || slot.placed.is_some_and(|(pass, ..)| pass == slot.calm))
                    && slot.since <= chunk.prepass.wrapping_add(1)
                    && Some(chunk.pass) == self.paints.1
                    && chunk.unbounded == outside
            })
            .and_then(|chunk| {
                let by = window.snap_move(at - chunk.at);
                match chunk.area {
                    _ if (chunk.key, chunk.at) == (key, at) => Some((chunk, None)),
                    // Paint in a mask that moved with the box drew all of it moved, culled parts too.
                    _ if chunk.translates
                        && chunk.key.same_paint(&key)
                        && window.translated(chunk.mask, mask, by) =>
                    {
                        Some((chunk, Some((by, chunk.mask, mask))))
                    }
                    // A span that drew nothing can't show that the box draws nothing in its new place, unless its
                    // mask stayed: it cut nothing there.
                    Some(area)
                        if chunk.key.same_paint(&key)
                            && match area.is_empty() {
                                true => chunk.mask == mask,
                                false => window.movable(area, by, mask),
                            } =>
                    {
                        Some((chunk, Some((by, chunk.mask, mask))))
                    }
                    _ => None,
                }
            });
        // A copy that reads a tile of a replaced image is rejected in `replay`.
        #[cfg(debug_assertions)]
        let copy = copy.filter(|(chunk, _)| !window.reads_removed(&chunk.span));
        // Debug builds paint the box to check the copy, unless the boxes inside have old places.
        let lagged = self.slots[index as usize].lag != Point::default();
        if (cfg!(not(debug_assertions)) || lagged)
            && let Some((chunk, moved)) = copy
            && let Some(span) = window.replay(&chunk.span, moved)
        {
            let area = moved.map_or(chunk.area, |(by, ..)| {
                chunk
                    .area
                    .map(|area| area + by.scale(window.scale_factor()))
            });
            let output = Output {
                pass: self.paints.0,
                span,
                key,
                at,
                mask,
                area,
            };
            self.slots[index as usize].chunk = Some(Chunk {
                pass: self.paints.0,
                span,
                key,
                at,
                mask,
                area,
                origin: output,
                ..chunk
            });
            if foreign {
                window.cull();
            }
            return;
        }
        // The boxes inside a box that a scroll moved without a prepaint have old places, so it can't paint.
        if lagged {
            // WARNING: an ancestor that copies its regions keeps the lag, so the prepaint of the restart must go
            // through all of them, or each pass restarts again. That holds for each box with a lag: this paint
            // copied chunks that the next one can't, so it may reach boxes that this one didn't.
            if !self.unshift {
                for at in 0..self.slots.len() {
                    if !self.slots[at].live || self.slots[at].lag == Point::default() {
                        continue;
                    }
                    let mut at = self.slots[at].parent;
                    while at != NONE {
                        let slot = &mut self.slots[at as usize];
                        slot.regions = None;
                        at = slot.parent;
                    }
                }
            }
            self.unshift = true;
            window.restart_frame();
            return;
        }
        let (mark, deferred) = (window.paint_mark(), self.deferred.len());
        // A debug paint of a box that release builds copy keeps the chunks of the boxes in it as a copy does.
        #[cfg(debug_assertions)]
        if copy.is_some() {
            self.copied.push(Vec::new());
        }
        if slot.hooks.as_ref().is_some_and(|hooks| !hooks.cached()) {
            window.paint_volatile();
        }
        if let Some(mask) = escaped {
            window.push_content_mask(mask);
        }
        // Below a transform or an effect, boxes paint elsewhere than their bounds, so none is skipped.
        let unbounded = slot.hooks.as_ref().is_some_and(|hooks| !hooks.bounded());
        self.unbounded += u32::from(unbounded);
        self.masks.push(mask);
        window.with_font_smoothing(slot.text.font_smoothing, |window| {
            self.visit(
                index,
                window,
                |hooks, cx, inner| hooks.paint(cx, inner),
                |tree, style, window| tree.paint_box(index, style, window),
            )
        });
        self.masks.pop();
        self.unbounded -= u32::from(unbounded);
        if escaped.is_some() {
            window.pop_content_mask();
        }
        let slot_contained = self.slots[index as usize].contained;
        // Floats that the box leaves to an ancestor are not in its chunk.
        let span = (self.deferred.len() <= deferred)
            .then(|| window.paint_span(mark))
            .flatten();
        #[cfg(debug_assertions)]
        if let Some((chunk, moved)) = copy {
            let now = (span.as_ref()).map(|span| window.span_records(span, false, None));
            let last = window.span_records(&chunk.span, true, moved);
            assert!(
                now.as_ref().is_some_and(|now| same_records(now, &last)),
                "box {index} would draw a copy moved by {:?} that differs from its paint: {:?}",
                moved.map(|(by, ..)| by),
                now.map(|now| (now.iter().zip(&last))
                    .find(|(now, last)| now != last)
                    .map_or_else(
                        || format!("{} records != {}", now.len(), last.len()),
                        |pair| format!("{pair:?}")
                    ))
            );
        }
        #[cfg(debug_assertions)]
        if copy.is_some() {
            for (at, chunk) in self.copied.pop().unwrap_or_default().into_iter().rev() {
                self.slots[at as usize].chunk = chunk;
            }
        }
        #[cfg(not(debug_assertions))]
        let _ = copy;
        let fresh = span.map(|span| {
            let output = Output {
                pass: self.paints.0,
                span,
                key,
                at,
                mask,
                area: window.plain(&span, mark, mask),
            };
            Chunk {
                pass: self.paints.0,
                prepass: self.prepaints,
                key,
                unbounded: outside,
                at,
                mask,
                area: output.area,
                translates: slot_contained && window.translatable(&span),
                span,
                base: output,
                origin: output,
            }
        });
        #[cfg(debug_assertions)]
        let fresh = match (copy, span) {
            (Some((chunk, moved)), Some(span)) => {
                let area = moved.map_or(chunk.area, |(by, ..)| {
                    (chunk.area).map(|area| area + by.scale(window.scale_factor()))
                });
                let output = Output {
                    pass: self.paints.0,
                    span,
                    key,
                    at,
                    mask,
                    area,
                };
                Some(Chunk {
                    pass: self.paints.0,
                    span,
                    key,
                    at,
                    mask,
                    area,
                    origin: output,
                    ..chunk
                })
            }
            _ => fresh,
        };
        #[cfg(debug_assertions)]
        if let Some(copied) = self.copied.last_mut() {
            copied.push((index, self.slots[index as usize].chunk));
        }
        self.slots[index as usize].chunk = fresh;
        if foreign {
            window.cull();
        }
    }

    fn paint_box(&mut self, index: u32, style: &Style, window: &mut Window) {
        if style.visibility == Visibility::Hidden {
            return;
        }
        let slot = &self.slots[index as usize];
        let bounds = slot.painted;
        if let Kind::Layers(negative) = slot.kind {
            self.paint_layers(negative, window);
            return;
        }
        let pieces: Vec<_> = parts(slot, bounds.origin, window.scale_factor()).collect();
        window.scope(Scope::Opacity(style.opacity), |window| {
            let full = style.corner_radii.resolve(bounds.size);
            let count = pieces.len().max(1);
            let pieces = pieces.iter().copied();
            // Like Chrome's `box-decoration-break: slice`, a part has no border and no corners at a cut.
            for (at, part) in pieces
                .chain((count == 1).then_some(bounds))
                .take(count)
                .enumerate()
            {
                let (first, last) = (at == 0, at + 1 >= count);
                let mut radii = full;
                for corners in [&mut radii.x, &mut radii.y] {
                    if !first {
                        (corners.top_left, corners.top_right) = (px(0.), px(0.));
                    }
                    if !last {
                        (corners.bottom_left, corners.bottom_right) = (px(0.), px(0.));
                    }
                }
                let sides = if first && last {
                    0
                } else {
                    10 | u8::from(first) | u8::from(last) << 2
                };
                window.draw_shadows(part, radii, &style.box_shadow, false);
                if let Some(background) = style.background.filter(|color| !color.is_transparent()) {
                    window.draw_box(part, radii, background, Default::default());
                }
                window.draw_shadows(part, radii, &style.box_shadow, true);
                if let Some(color) = style.border_color.filter(|color| !color.is_transparent())
                    && style.border_widths.any(|width| !width.is_zero())
                {
                    window.draw_box(
                        part,
                        radii,
                        color.with_alpha(0.),
                        crate::Border {
                            widths: style.border_widths,
                            color,
                            style: style.border_style,
                            sides,
                        },
                    );
                }
            }
            let mask = style.overflow_mask(bounds);
            window.scope(Scope::Clip(mask), |window| {
                let slot = &self.slots[index as usize];
                if let Some(hitbox) = &slot.region {
                    if let Some(cursor) = style.cursor {
                        window.set_cursor(hitbox, cursor);
                    }
                    if let Some(events) = &slot.events {
                        if events.drag {
                            window.add_drag_area(hitbox);
                        }
                        events.listen(style, slot.text.line_height.round(), hitbox, window);
                    }
                }
                let painted = matches!(slot.kind, Kind::Text(..) | Kind::Measured)
                    && slot.hooks.as_ref().is_some_and(|hooks| {
                        let scale = window.scale_factor();
                        let mut cx = NodeCx {
                            window,
                            bounds: slot.bounds,
                            exact: slot.exact,
                            unscroll: slot.unscroll,
                            inset: inset(&slot.layout, scale),
                            baselines: self.baselines(index, scale),
                            text: &slot.text,
                            style,
                            state: slot.state.as_ref(),
                            split: slot.split.as_deref(),
                        };
                        hooks.paint_content(&mut cx)
                    });
                let slot = &self.slots[index as usize];
                match &slot.kind {
                    _ if painted => {}
                    Kind::Text(..) => {
                        if let Some(state) = slot
                            .state
                            .as_ref()
                            .and_then(|state| state.downcast_ref::<TextState>())
                        {
                            state.layout.paint(slot.text.text_align, window);
                        }
                    }
                    _ => {}
                }
                if style.display != Display::None {
                    let scroll = (slot.events.as_ref())
                        .and_then(|events| events.scroll.as_ref())
                        .map(|handle| handle.painted());
                    let scroll =
                        scroll.map(|offset| (u64::from(index), slot.painted.origin, offset));
                    window.with_scroll(scroll, |window| {
                        if style.stacking {
                            self.painting.push(index);
                        }
                        let mark = self.deferred.len();
                        for child in 0..self.slots[index as usize].children.len() {
                            let child = index_of(self.slots[index as usize].children[child]);
                            let slot = &self.slots[child as usize];
                            // Text runs into the box of a float with a shape, and paints over it.
                            if slot.style.flow & Flow::FLOAT != 0
                                && slot.style.z_index.is_none()
                                && slot.style.outside.is_none()
                            {
                                self.deferred.push(child);
                            } else {
                                self.paint_node(child, false, window);
                            }
                        }
                        // A plain block passes its floats up. Other boxes paint them above their blocks, with their
                        // clip, scroll offset and effects.
                        if style.flow & Flow::BLOCK == 0
                            || style.stacking
                            || style.z_index.is_some()
                            || style.opacity.is_some()
                            || style.overflow.x != Overflow::Visible
                            || style.overflow.y != Overflow::Visible
                        {
                            // A float catches the floats inside it, so painting one doesn't grow the list.
                            for at in mark..self.deferred.len() {
                                self.paint_node(self.deferred[at], false, window);
                            }
                            self.deferred.truncate(mark);
                        }
                        if style.stacking {
                            self.painting.pop();
                        }
                    });
                }
            });
        });
    }

    fn paint_layers(&mut self, negative: bool, window: &mut Window) {
        let Some(&context) = self.painting.last() else {
            return;
        };
        let list = self.layers.remove(&context).unwrap_or_default();
        for &(_, index, mask) in list.iter().filter(|(order, ..)| (*order < 0) == negative) {
            window.scope(Scope::Clip(Some(mask)), |window| {
                self.paint_node(index, true, window)
            });
        }
        self.layers.insert(context, list);
    }
}
