use super::*;

impl Tree {
  // `unanchored` holds the boxes whose style moved or sized them since the last layout.
  pub(crate) fn layout(
    &mut self,
    root: NodeId,
    viewport: Size<Pixels>,
    unanchored: FxHashSet<u64>,
    window: &mut Window,
  ) {
    let scale = window.scale_factor();
    let space = taffy::Size {
      width: taffy::AvailableSpace::Definite(viewport.width.0 * scale),
      height: taffy::AvailableSpace::Definite(viewport.height.0 * scale),
    };
    // The initial containing block leaves out the scroll bars of the page.
    let bars = self.slots[index_of(root) as usize].style.bars;
    let viewport = Size::new(
      viewport.width - bars.left - bars.right,
      viewport.height - bars.top - bars.bottom,
    );
    self.choose_anchors(&unanchored, window);
    let mut tree = LayoutTree {
      slots: &mut self.slots,
      window,
      scale,
      exact: false,
      floats: Vec::new(),
      ceiling: f32::NEG_INFINITY,
      reach: f32::NEG_INFINITY,
      origin: [0.; 2],
      natural: false,
      naturals: Vec::new(),
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
        && let Some(inputs) = slot.inputs()
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
      let escape = slot.style.escape & Escape::PLACED;
      let (Some(old), true, true) = (
        slot.cold.as_ref().and_then(|cold| cold.room),
        slot.live,
        escape != 0 && slot.parent != NONE,
      ) else {
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
    tree.forget();
    if again {
      taffy::compute_root_layout(&mut tree, root, space);
      tree.forget();
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
      tree.forget();
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
        slot.live
          && (slot.style.escape & Escape::PLACED != 0 && slot.style.placed != 0
            || slot.style.follow.is_some()
            || slot.style.anchored.is_some())
      })
    });
    self.marks.retain(|&mark, &mut index| {
      slots
        .get(index as usize)
        .is_some_and(|slot| slot.live && slot.style.mark.is_some_and(|it| it.key == mark))
    });
    for &index in &self.escaping {
      let slot = &slots[index as usize];
      let parent = slot.parent;
      if parent == NONE {
        continue;
      }
      if let Some(follow) = slot.style.follow
        && let Some(&mark) = self.marks.get(&follow.mark)
      {
        let layout = slot.layout;
        let marked = &slots[mark as usize];
        let drop = marked.style.mark.map_or(0., |mark| mark.drop.0 * scale)
          + if follow.below {
            marked.layout.size.height
          } else {
            0.
          };
        let mut place = point(marked.shift.x, marked.shift.y + drop);
        let mut at = mark;
        while at != parent && at != NONE {
          let inner = &slots[at as usize];
          place.x += inner.layout.location.x;
          place.y += inner.layout.location.y;
          if let Some(split) = &inner.split {
            place += split.shift;
          }
          at = inner.parent;
        }
        let mut location = layout.location;
        // A block is at the start of its line, which only a text knows.
        let across = !follow.below || marked.style.mark.is_some_and(|mark| mark.inline);
        if at == parent && follow.axes & 1 != 0 && across {
          location.x = match marked.turned.unwrap_or(follow.rtl) {
            true => place.x - layout.margin.right - layout.size.width,
            false => place.x + layout.margin.left,
          };
        }
        if at == parent && follow.axes & 2 != 0 {
          location.y = place.y + layout.margin.top;
        }
        if location != layout.location {
          slots[index as usize].layout.location = location;
          changed(slots, index as usize);
          unfar(slots, parent as usize);
        }
      }
      let slot = &slots[index as usize];
      let anchored = slot.style.anchored.as_deref();
      let escape = slot.style.escape & Escape::PLACED;
      if escape == 0
        || anchored.is_none()
          && (slot.style.placed == 0 || slots[parent as usize].style.contains & escape != 0)
      {
        continue;
      }
      // The parent's place in the containing block, which is the outermost box when no box contains it.
      let (mut at, mut block) = ([0f32; 2], parent);
      loop {
        let outer = &slots[block as usize];
        if outer.style.contains & escape != 0 || outer.parent == NONE {
          break;
        }
        at = [
          at[0] + outer.layout.location.x,
          at[1] + outer.layout.location.y,
        ];
        block = outer.parent;
      }
      let outer = &slots[block as usize];
      let contained = outer.style.contains & escape != 0;
      // Paint places the fixed boxes of the viewport, which no scroll range counts.
      if !contained && escape == Escape::FIXED {
        continue;
      }
      let [origin, size] = match contained {
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
        if let Some(aligned) = anchored.and_then(|axes| axes[axis]) {
          let extent = (extent + margin_start + margin_end) / scale;
          let at = aligned.place(extent, size[axis] / scale) * scale;
          return Some(origin[axis] + at + margin_start);
        }
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
  fn choose_anchors(&mut self, unanchored: &FxHashSet<u64>, window: &mut Window) {
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
      if anchor.is_none() && offset.y != Pixels::ZERO && self.slots[index as usize].cache.is_empty()
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
      if !slot.keyed && !matches!(slot.kind, Kind::Text(_)) || size.width <= 0. || size.height <= 0.
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
