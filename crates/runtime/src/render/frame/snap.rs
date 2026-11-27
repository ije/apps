use crate::render::viewport_overflow;
use crate::{
  css::properties::{Prop, Style},
  dom::Document,
  render::{PageState, paint::scrollbar},
};
use renderer::{ScrollSnaps, SnapAlign, SnapArea};

// What the snap areas of a scroll container come from: document, styles, its box, its scroll range, and the room of
// its scroll bars.
pub(crate) type Key = (u64, u64, [u32; 4], [u32; 4], [u32; 4]);

// Gives the scroll containers the snap areas of this layout. True when a box moved to an area.
pub(super) fn update(doc: &Document, dom: &mut PageState) -> bool {
  let Some(styles) = dom.styles.computed.clone() else {
    return false;
  };
  let html = doc
    .nodes
    .first()
    .and_then(|root| (root.children.iter().copied()).find(|&id| doc.nodes[id].tag == "html"));
  let (focus, geometry) = (&dom.interaction.states[0], &mut dom.geometry);
  let mut moved = Vec::new();
  for (&id, handle) in &geometry.scroll {
    let Some(node) = (if id == 0 { html } else { Some(id) }) else {
      continue;
    };
    let Some(style) = styles.values.get(node) else {
      continue;
    };
    // The viewport takes the snap type of the root.
    let kind = (id == 0 || html != Some(id))
      .then(|| style.get(Prop::SCROLL_SNAP_TYPE))
      .flatten()
      .filter(|kind| *kind != "none");
    let Some(kind) = kind else {
      if handle.snaps() {
        handle.set_snaps(None);
        geometry.snaps.remove(&id);
      }
      continue;
    };
    let rect = match id {
      0 => {
        let size = handle.bounds().size;
        [0., 0., size.width.0, size.height.0]
      }
      _ => geometry.mirror.get(&id).copied().unwrap_or_default(),
    };
    if id != 0 && !(geometry.mirror.shown(id) && rect != [0.; 4]) {
      handle.forget_snaps();
      geometry.snaps.remove(&id);
      continue;
    }
    let units = &styles.lengths[node];
    let bars = match id {
      0 => [0., geometry.page_bars[1], geometry.page_bars[0], 0.],
      _ => scrollbar::room(style, styles.extras(node), units, handle, false),
    };
    let range = handle.range();
    let key: Key = (
      doc.nodes.latest(),
      geometry.layout_epoch,
      rect.map(f32::to_bits),
      [range[0].x, range[0].y, range[1].x, range[1].y].map(|value| value.0.to_bits()),
      bars.map(f32::to_bits),
    );
    if geometry.snaps.get(&id) == Some(&key) {
      continue;
    }
    geometry.snaps.insert(id, key);
    let borders = match id {
      0 => [0.; 4],
      _ => units.borders(style, rect[2]),
    };
    let [top, right, bottom, left] = [0, 1, 2, 3].map(|side| borders[side] + bars[side]);
    let client = [rect[2] - left - right, rect[3] - top - bottom];
    let padding = |prop: Prop, base: f32| {
      (style.get(prop))
        .and_then(|value| units.resolve(value, base))
        .unwrap_or(0.)
    };
    let port = [
      [
        left + padding(Prop::SCROLL_PADDING_LEFT, client[0]),
        left + client[0] - padding(Prop::SCROLL_PADDING_RIGHT, client[0]),
      ],
      [
        top + padding(Prop::SCROLL_PADDING_TOP, client[1]),
        top + client[1] - padding(Prop::SCROLL_PADDING_BOTTOM, client[1]),
      ],
    ];
    let axis = kind.split_ascii_whitespace().next().unwrap_or_default();
    let axes = [
      matches!(axis, "x" | "inline" | "both"),
      matches!(axis, "y" | "block" | "both"),
    ];
    let rtl = |style: &Style| style.get(Prop::DIRECTION) == Some("rtl");
    let own_rtl = rtl(style);
    let shown = geometry.mirror_offset(id);
    let at = [-shown.x.0, -shown.y.0];
    let scrolls = |style: &Style| {
      [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
        .into_iter()
        .any(|prop| (style.get(prop)).is_some_and(|value| !matches!(value, "visible" | "clip")))
    };
    let mut areas = Vec::new();
    let mut stack: Vec<usize> = styles
      .flat_children(doc, node)
      .iter()
      .rev()
      .copied()
      .collect();
    while let Some(child) = stack.pop() {
      let Some(style) = styles.values.get(child) else {
        continue;
      };
      let align = style
        .get(Prop::SCROLL_SNAP_ALIGN)
        .filter(|align| *align != "none");
      if let Some(align) = align
        && let Some(&area) = geometry.mirror.get(&child)
        && area != [0.; 4]
        && geometry.mirror.shown(child)
      {
        let units = &styles.lengths[child];
        let margin = |prop: Prop| {
          (style.get(prop))
            .and_then(|value| units.resolve(value, 0.))
            .unwrap_or(0.)
        };
        // Chrome measures in 1/64 pixels.
        let place = |axis: usize, sides: [Prop; 2]| {
          let first = area[axis] - rect[axis] + at[axis];
          [
            first - margin(sides[0]),
            first + area[axis + 2] + margin(sides[1]),
          ]
          .map(|value| (value * 64.).round() / 64.)
        };
        let spot = [
          place(0, [Prop::SCROLL_MARGIN_LEFT, Prop::SCROLL_MARGIN_RIGHT]),
          place(1, [Prop::SCROLL_MARGIN_TOP, Prop::SCROLL_MARGIN_BOTTOM]),
        ];
        // A wider area has its own direction.
        let flipped = match spot[0][1] - spot[0][0] > rect[2] {
          true => rtl(style),
          false => own_rtl,
        };
        let aligned = |align: &str, flipped: bool| match align {
          "start" if flipped => Some(SnapAlign::End),
          "end" if flipped => Some(SnapAlign::Start),
          "start" => Some(SnapAlign::Start),
          "end" => Some(SnapAlign::End),
          "center" => Some(SnapAlign::Center),
          _ => None,
        };
        let mut aligns = align.split_ascii_whitespace();
        let block = aligns.next().unwrap_or("none");
        areas.push(SnapArea {
          id: child as u64,
          rect: spot,
          align: [
            aligned(aligns.next().unwrap_or(block), flipped),
            aligned(block, false),
          ],
          stop: style.get(Prop::SCROLL_SNAP_STOP) == Some("always"),
          focus: focus.contains(&child),
        });
      }
      // The areas in an inner scroll container belong to it.
      if !scrolls(style) || viewport_overflow(doc, &styles, child) {
        stack.extend(styles.flat_children(doc, child).iter().rev());
      }
    }
    if handle.set_snaps(Some(ScrollSnaps {
      axes,
      must: kind.contains("mandatory"),
      port,
      areas,
      target: doc.target.map(|id| id as u64),
      ratio: dom.pixel_ratio.max(1.),
    })) {
      moved.push(id);
    }
  }
  if moved.is_empty() {
    return false;
  }
  geometry.mirror.set_moved();
  true
}
