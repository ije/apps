// Boxes that stand in for content that a frame doesn't build: spacers for runs of culled boxes, anchors of culled
// absolute boxes, and the watcher of `content-visibility: auto` boxes.
use super::super::layout::cull::{Run, SKIPS, Zone, shift};
use crate::css::properties::Prop;
use renderer::{Child, px};
use std::{cell::Cell, rc::Rc};

pub(crate) fn spacer(
  run: Run,
  path: Rc<[usize]>,
  [left, width]: [f32; 2],
  zone: Rc<Zone>,
  view: Rc<super::super::ViewState>,
) -> Child {
  let Run {
    members,
    top,
    height,
    width: size,
    margins,
    watch,
    clear,
  } = run;
  let parent = path.last().copied().unwrap_or_default();
  let (first, last) = (Cell::new(None), Cell::new(([left, top], None::<[f32; 2]>)));
  // It wakes the blocks that it stands for, and moves their places, so each move prepares it again.
  let mut spacer = renderer::canvas_watching(
    move |bounds, window| {
      let view = &*view;
      let mut store = view.store.borrow_mut();
      let store = &mut *store;
      let Some(page) = store.pages.get_mut(&view.id) else {
        return;
      };
      let dom = &mut page.state;
      let [low, high] = zone.visible(&dom.geometry.mirror, window);
      let y = f32::from(bounds.top());
      let x = dom.geometry.mirror.get(&parent).map(|rect| {
        rect[0]
          + dom.geometry.scroll.get(&parent).map_or(0., |handle| {
            f32::from(crate::dom::geometry::painted_offset(handle, window.scale_factor()).x)
          })
      });
      let resized = x.is_none()
        || dom
          .layout
          .cull
          .widths
          .get(&parent)
          .is_none_or(|now| (now[0] - width).abs() > 0.01);
      let reflowed = watch.iter().any(|&(id, width)| {
        dom
          .layout
          .cull
          .placed
          .get(id)
          .is_none_or(|placed| (placed.1[0] - width).abs() > 0.01)
      });
      let at = [x.unwrap_or(left), y];
      let moved = first.get().is_some_and(|first| first != at);
      first.set(first.get().or(Some(at)));
      let near = f32::from(window.viewport_size().height) / 2.;
      let shown = resized || reflowed || y < high && y + height > low;
      if shown || moved && y < high + near && y + height > low - near {
        dom.layout.cull.off |= resized || reflowed;
        wake(view, &path, shown, window);
      }
      // The shift of a box around it already moved these blocks in the mirror.
      let place = members.iter().find_map(|id| {
        let rect = dom
          .geometry
          .mirror
          .get(id)
          .filter(|rect| **rect != [0.; 4])?;
        Some([rect[0], rect[1]])
      });
      let (from, was) = last.get();
      let delta = [0, 1].map(|axis| {
        let moved = place
          .zip(was)
          .map_or(0., |(now, was)| now[axis] - was[axis]);
        at[axis] - from[axis] - moved
      });
      let moves = delta.iter().any(|by| by.abs() > 0.01);
      if moves || was.is_none() {
        let by = if moves { delta } else { [0.; 2] };
        last.set((at, place.map(|[x, y]| [x + by[0], y + by[1]])));
      }
      if moves {
        shift(
          &page.doc,
          &dom.tree.shadows,
          &mut dom.layout.cull.subtrees,
          &mut dom.geometry.mirror,
          &members,
          delta,
        );
      }
    },
    |_, _, _| {},
  )
  .w(px(size))
  .h(px(height))
  .mt(px(margins[0]))
  .mb(px(margins[1]))
  .flex_shrink_0();
  // Like the blocks it stands for, floats don't move it, and only clearance does.
  spacer.style().flow = renderer::Flow::BLOCK | clear;
  spacer.into()
}

pub(crate) fn anchor(
  items: Vec<(usize, [f32; 4])>,
  path: Rc<[usize]>,
  zone: Rc<Zone>,
  view: Rc<super::super::ViewState>,
) -> Child {
  let first = Cell::new(None);
  renderer::canvas_watching(
    move |bounds, window| {
      let view = &*view;
      let mut store = view.store.borrow_mut();
      let store = &mut *store;
      let Some(page) = store.pages.get_mut(&view.id) else {
        return;
      };
      let dom = &mut page.state;
      let [low, high] = zone.visible(&dom.geometry.mirror, window);
      let [left, top] = [f32::from(bounds.left()), f32::from(bounds.top())];
      let moved = first.get().is_some_and(|first| first != [left, top]);
      first.set(first.get().or(Some([left, top])));
      let near = f32::from(window.viewport_size().height) / 2.;
      let (mut shown, mut close) = (false, false);
      for &(id, [x, y, _, height]) in &items {
        let [x, y] = [left + x, top + y];
        shown |= y < high && y + height > low;
        close |= y < high + near && y + height > low - near;
        if let Some(&[old_x, old_y, ..]) = dom.geometry.mirror.get(&id)
          && [old_x, old_y] != [x, y]
        {
          shift(
            &page.doc,
            &dom.tree.shadows,
            &mut dom.layout.cull.subtrees,
            &mut dom.geometry.mirror,
            &[id],
            [x - old_x, y - old_y],
          );
        }
      }
      if shown || moved && close {
        wake(view, &path, shown, window);
      }
    },
    |_, _, _| {},
  )
  .absolute()
  .left(px(0.))
  .top(px(0.))
  .size(px(0.))
  .into()
}

// After prepaint, shows the contents of each `content-visibility: auto` box near the view or with the focus,
// and skips the contents of the others.
pub(crate) fn autos(view: Rc<super::super::ViewState>) -> Child {
  renderer::canvas_watching(
    move |_, window| {
      let view = &*view;
      let mut guard = view.store.borrow_mut();
      let store = &mut *guard;
      let Some(page) = store.pages.get_mut(&view.id) else {
        return;
      };
      let dom = &mut page.state;
      // Like in Chrome, the view and each clip reach 150% of the view size further on each side.
      let size = window.viewport_size();
      let [x, y] = [f32::from(size.width), f32::from(size.height)].map(|side| side * 1.5);
      let view_area = [
        -x,
        -y,
        f32::from(size.width) + x,
        f32::from(size.height) + y,
      ];
      // As in Chrome, a box with the focus or the selection in it shows its contents.
      let mut relevant = rustc_hash::FxHashSet::default();
      for start in dom
        .interaction
        .focus
        .into_iter()
        .chain(dom.interaction.selection.keys().copied())
      {
        let mut node = Some(start);
        while let Some(id) = node.filter(|&id| relevant.insert(id)) {
          node = page.doc.nodes.get(id).and_then(|node| node.parent);
        }
      }
      // The boxes whose state changes, in document order, so boxes come after the boxes around them.
      let mut changes = Vec::new();
      let shown = &dom.layout.measurements.shown;
      let scroll = &dom.geometry.scroll;
      dom.layout.autos.retain(|&id, (path, clips, last)| {
        let Some(node) = dom.layout.layout_nodes.get(id).and_then(Option::as_ref) else {
          return false;
        };
        if node.flags & SKIPS == 0 || node.style.get(Prop::CONTENT_VISIBILITY) != Some("auto") {
          return false;
        }
        // A box in skipped contents keeps its state, as in Chrome.
        let relevant = relevant.contains(&id);
        if dom.layout.cull.visible.get(id) != Some(&true) && !relevant {
          return true;
        }
        let area = clips
          .iter()
          .filter_map(|clip| dom.geometry.mirror.get(clip))
          .fold(view_area, |area, rect| {
            [
              area[0].max(rect[0] - x),
              area[1].max(rect[1] - y),
              area[2].min(rect[0] + rect[2] + x),
              area[3].min(rect[1] + rect[3] + y),
            ]
          });
        let near = relevant
          || dom.geometry.mirror.get(&id).is_some_and(|rect| {
            rect[0] <= area[2]
              && rect[0] + rect[2] >= area[0]
              && rect[1] <= area[3]
              && rect[1] + rect[3] >= area[1]
          });
        if shown.get(&id) != Some(&near) {
          // Like in Chromium, a box that only resized since its last change, with one edge in place, changes
          // again only when the view moved: the change itself can take it out of the view again.
          let mut view = rustc_hash::FxHasher::default();
          for at in std::iter::once(&0).chain(clips.iter()) {
            if let Some(handle) = scroll.get(at) {
              let offset = handle.offset();
              std::hash::Hasher::write_u64(
                &mut view,
                u64::from(f32::from(offset.x).to_bits()) << 32
                  | u64::from(f32::from(offset.y).to_bits()),
              );
            }
          }
          std::hash::Hasher::write_u64(
            &mut view,
            u64::from(x.to_bits()) << 32 | u64::from(y.to_bits()),
          );
          let view = std::hash::Hasher::finish(&view) | 1;
          let edges = dom
            .geometry
            .mirror
            .get(&id)
            .map_or([f32::NAN; 2], |rect| [rect[1], rect[1] + rect[3]]);
          let resized = edges != last.1 && (edges[0] == last.1[0] || edges[1] == last.1[1]);
          if relevant || last.0 != view || !resized || shown.get(&id).is_none() {
            *last = (view, edges);
            changes.push((dom.layout.cull.walk.place(id), id, near, path.clone()));
          }
        }
        true
      });
      let removed = dom.layout.measurements.shown.len() > dom.layout.autos.len();
      if removed {
        let autos = &dom.layout.autos;
        dom
          .layout
          .measurements
          .shown
          .retain(|id, _| autos.contains_key(id));
      }
      changes.sort_unstable_by_key(|change| change.0);
      let (mut flipped, mut seen) = (false, false);
      let [width, height] = [f32::from(size.width), f32::from(size.height)];
      for (_, id, near, path) in changes {
        let shown = &mut dom.layout.measurements.shown;
        // A box in a box that skips its contents now keeps its state.
        if path[..path.len() - 1]
          .iter()
          .any(|ancestor| shown.get(ancestor) == Some(&false))
        {
          continue;
        }
        let old = shown.insert(id, near);
        // As in Chrome, new boxes report that they skip their contents before any box reports a change.
        if old.is_none() {
          let at = dom
            .layout
            .skip_events
            .iter()
            .take_while(|event| event.1)
            .count();
          dom.layout.skip_events.insert(at, (id, true));
        }
        if old.is_some() || near {
          dom.layout.skip_events.push((id, !near));
          dom.layout.measurements.waking.push(id);
          flipped = true;
          // A box that shows in the view needs its contents in this frame.
          seen |= near
            && dom.geometry.mirror.get(&id).is_some_and(|rect| {
              rect[0] < width
                && rect[0] + rect[2] > 0.
                && rect[1] < height
                && rect[1] + rect[3] > 0.
            });
        }
      }
      drop(guard);
      if seen {
        window.restart();
      } else if flipped {
        window.request_animation_frame();
      }
    },
    |_, _, _| {},
  )
  .absolute()
  .into()
}

// Builds the boxes on the path again, and the visibility of all nodes, when a box starts or stops to skip its contents.
pub(crate) fn reskip(
  view: &super::super::ViewState,
  dom: &mut crate::render::PageState,
  nodes: &rustc_hash::FxHashSet<usize>,
) {
  dom.layout.cull.reskipped = true;
  let measured = &mut dom.layout.measurements;
  let mut kept = view.kept.borrow_mut();
  for node in nodes {
    measured.boxes.remove(node);
    measured.texts.remove(node);
    dom.layout.grids.remove(node);
    kept.remove(node);
  }
}

fn wake(
  view: &super::super::ViewState,
  path: &[usize],
  shown: bool,
  window: &mut renderer::Window,
) {
  let mut kept = view.kept.borrow_mut();
  for id in path {
    kept.remove(id);
  }
  if shown {
    window.redraw();
  } else {
    window.request_animation_frame();
  }
}
