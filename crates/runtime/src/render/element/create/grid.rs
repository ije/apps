use super::super::Build;
use super::super::kept::{Query, hashed};
use super::super::query::{baselines, shrinks};
use super::cx::{Cx, Frame};
use crate::css;
use crate::css::properties::Prop;
use crate::render::layout::grid::Grid;
use crate::render::layout::size::{aspect_ratio, layout_ceil, place, pseudo_lengths};
use crate::style::{self};
use renderer::{Child, px};

/// Lays out a grid container: its width, its tracks, the places of its pseudo-element items and its rows.
pub(super) fn grid_container(
  cx: &Cx<'_>,
  fr: &Frame,
  mut el: renderer::Node,
  items: &mut [Option<(i32, renderer::Node, [f32; 4])>; 2],
  content: &mut [f32; 2],
  auto_height: bool,
) -> (renderer::Node, Option<Grid>) {
  let Cx {
    reader,
    id,
    node,
    style,
    units,
    parsed,
    position,
    containing,
    slot,
    ..
  } = *cx;
  let Build {
    blocks,
    doc,
    styles,
    text,
    pixel_ratio,
    measured,
    view,
    ..
  } = *reader;
  let Frame {
    padding,
    borders,
    margin,
    ..
  } = *fr;
  let made = parsed.made;
  let flexed = style
    .get(Prop::DISPLAY)
    .is_some_and(|v| v.ends_with("grid"))
    && slot.is_none()
    && auto_height
    && reader
      .parent(id, Prop::DISPLAY)
      .is_some_and(|v| v.ends_with("flex"))
    && if reader
      .parent(id, Prop::FLEX_DIRECTION)
      .is_some_and(|v| v.starts_with("column"))
    {
      style
        .get(Prop::FLEX_GROW)
        .and_then(|v| v.parse::<f32>().ok())
        .is_some_and(|v| v > 0.)
        || style
          .get(Prop::FLEX_BASIS)
          .is_some_and(|v| v != "auto" && v != "content")
    } else {
      // A flex row stretches the item to its line.
      let stretched = match style.get(Prop::ALIGN_SELF).unwrap_or("auto") {
        "auto" => reader
          .parent(id, Prop::ALIGN_ITEMS)
          .is_none_or(|v| matches!(v, "normal" | "stretch")),
        value => matches!(value, "normal" | "stretch"),
      };
      stretched
        && [Prop::MARGIN_TOP, Prop::MARGIN_BOTTOM]
          .into_iter()
          .all(|prop| style.get(prop) != Some("auto"))
    };
  let shrunk = style.get(Prop::DISPLAY).is_some_and(|v| v == "grid")
    && slot.is_none()
    && ((style::table_box(style::role(styles, id))
            || matches!(position, "absolute" | "fixed"))
            && style.get(Prop::WIDTH) == Some("fit-content")
            // Like Chrome, a button is as wide as its content, also as a block.
            || style.get(Prop::WIDTH).is_none_or(|v| v == "auto") && (node.tag == "button" || {
                let (shrunk, farthest) = shrinks(doc, styles, id, true);
                reader.log(Query::Shrunk, reader.scope_of(id, farthest), hashed(shrunk));
                shrunk
            }));
  let mut widest = None;
  let fills = style.get(Prop::DISPLAY).is_some_and(|v| v == "grid")
    && !shrunk
    && slot.is_none()
    && !flexed
    && style
      .get(Prop::WIDTH)
      .is_none_or(|v| matches!(v, "auto" | "100%"))
    && !matches!(position, "absolute" | "fixed")
    && node.parent.is_some()
    && match reader.parent(id, Prop::DISPLAY).unwrap_or("block") {
      "block" | "flow-root" | "list-item" => true,
      "flex" | "inline-flex" => {
        reader
          .parent(id, Prop::FLEX_DIRECTION)
          .is_some_and(|v| v.starts_with("column"))
          && reader
            .parent(id, Prop::ALIGN_ITEMS)
            .is_none_or(|v| matches!(v, "normal" | "stretch"))
          && style
            .get(Prop::ALIGN_SELF)
            .is_none_or(|v| matches!(v, "auto" | "stretch"))
      }
      _ => false,
    };
  // The height that insets or a ratio give a grid, which holds even without rows.
  let mut definite = 0f32;
  let grid = if let Some(display) = style
    .get(Prop::DISPLAY)
    .filter(|v| matches!(*v, "grid" | "inline-grid"))
  {
    if display == "inline-grid"
      && slot.is_none()
      && style.get(Prop::WIDTH).is_none_or(|v| v == "auto")
    {
      let extent = padding[1] + padding[3] + borders[1] + borders[3] + margin[1] + margin[3];
      let [min, max, ..] = crate::render::layout::grid::intrinsic::intrinsic(
        doc,
        id,
        styles,
        None,
        None,
        text,
        &mut measured.borrow_mut(),
      );
      content[0] = (max.min(content[0] + extent).max(min) - extent).max(0.);
    }
    let horizontal = padding[1] + padding[3] + borders[1] + borders[3];
    let vertical = padding[0] + padding[2] + borders[0] + borders[2];
    let mut tracked = [None; 2];
    if shrunk {
      let [min, max, ..] = crate::render::layout::grid::intrinsic::intrinsic(
        doc,
        id,
        styles,
        None,
        None,
        text,
        &mut measured.borrow_mut(),
      );
      let [min, max] = [min, max].map(|value| (value - horizontal - margin[1] - margin[3]).max(0.));
      widest = Some([min, max]);
      let last = {
        let mut store = view.store.borrow_mut();
        let dom = store.state(view.id);
        let build = dom.layout.builds;
        let widest = dom.layout.grid_widest.entry(id).or_insert((max, build));
        if widest.0 != max {
          *widest = (max, build);
        }
        // A second build before the next layout must not read the width of the old content.
        let same = widest.1 != build;
        same
          .then(|| dom.layout.cull.widths.get(&id).map(|size| size[0].max(0.)))
          .flatten()
      };
      // Laid-out bounds are in layout units, so a fitting grid may look narrower.
      content[0] = last.map_or(content[0].min(max), |last| {
        if last + 0.5 >= max {
          last.max(max)
        } else {
          last
        }
      });
      tracked[0] = Some((last.unwrap_or(content[0]), horizontal));
    }
    if fills {
      if let Some(last) = view
        .store
        .borrow()
        .page_state(view.id)
        .and_then(|dom| dom.layout.cull.widths.get(&id))
      {
        content[0] = last[0].max(0.);
      }
      tracked[0] = Some((content[0], horizontal));
      // A table is never narrower than its content, which also sets the minimum of flex items.
      // A `<table>` box scrolls the table inside, so it can be narrower.
      if matches!(style::role(styles, id), 1 | 2 | 10..=12) && style.get(Prop::MIN_WIDTH).is_none()
      {
        let [min, ..] = crate::render::layout::grid::intrinsic::intrinsic(
          doc,
          id,
          styles,
          None,
          None,
          text,
          &mut measured.borrow_mut(),
        );
        el = el.min_w(px(layout_ceil(
          (min - margin[1] - margin[3]).max(0.),
          pixel_ratio,
        )));
      }
    }
    let inset = |prop| {
      style
        .get(prop)
        .and_then(|value| units.resolve(value, containing[1]))
    };
    let stretched = (matches!(position, "absolute" | "fixed")
      && style.get(Prop::HEIGHT).is_none_or(|v| v == "auto"))
    .then(|| Some(inset(Prop::TOP)? + inset(Prop::BOTTOM)?))
    .flatten();
    // Like Chrome, a ratio sizes the box of `box-sizing` from the used width, and the rows can make it taller.
    let ratio = style
      .get(Prop::ASPECT_RATIO)
      .filter(|_| auto_height && slot.is_none())
      .and_then(aspect_ratio)
      .map(|ratio| {
        let border_box = style.get(Prop::BOX_SIZING) == Some("border-box");
        let extra = if border_box {
          [horizontal, vertical]
        } else {
          [0.; 2]
        };
        let height = (content[0] + extra[0]) / ratio;
        let limit = |prop| {
          style
            .get(prop)
            .and_then(|v| units.resolve(v, containing[1]))
        };
        let height = height.min(limit(Prop::MAX_HEIGHT).unwrap_or(f32::INFINITY));
        (height.max(limit(Prop::MIN_HEIGHT).unwrap_or(0.)) - extra[1]).max(0.)
      });
    let height = if slot.is_some() || !auto_height {
      Some(content[1])
    } else if ratio.is_some() {
      definite = ratio.unwrap_or_default();
      ratio
    } else if let Some(insets) = stretched {
      definite = (containing[1] - insets - margin[0] - margin[2] - vertical).max(0.);
      Some(definite)
    } else if flexed {
      let height = view.store.borrow().page_state(view.id).and_then(|state| {
        state
          .layout
          .cull
          .widths
          .get(&id)
          .map(|size| size[1].max(0.))
      });
      tracked[1] = Some((height.unwrap_or(f32::NAN), vertical));
      height
    } else {
      None
    };
    let mut store = view.store.borrow_mut();
    let dom = store.state(view.id);
    for (axis, track) in tracked.iter().enumerate() {
      if let Some((used, _)) = track {
        reader.log(
          Query::Track(id as u32, axis as u8),
          0,
          used.to_bits().into(),
        );
      }
    }
    reader.log(
      Query::Baselines(id as u32),
      0,
      baselines(doc, Some(&*dom), id),
    );
    if tracked.iter().any(Option::is_some) {
      dom.layout.flexed_grids.insert(id, tracked);
    } else {
      dom.layout.flexed_grids.remove(&id);
    }
    drop(store);
    let cached = view
      .store
      .borrow()
      .page_state(view.id)
      .and_then(|state| state.layout.grids.get(&id))
      .filter(|(width, cached, inherited, _)| {
        *width == content[0]
          && *cached == height
          && inherited.as_ref() == slot.as_ref().map(|(_, grid)| grid)
      })
      .map(|(_, _, _, grid)| grid.clone());
    Some(cached.unwrap_or_else(|| {
      // A measure laid out the same grid when it has no inherited tracks and no baseline-aligned items.
      let measure = slot.is_none().then(|| {
        let key = (id, Some(content[0].to_bits()), height.map(f32::to_bits));
        measured.borrow().grid(key)
      });
      let measure = measure
        .flatten()
        .filter(|grid| !grid.cells.iter().any(|cell| cell.3));
      let store = view.store.borrow();
      let baselines = store.page_state(view.id).map(|dom| &dom.layout.baselines);
      let lay = || {
        crate::render::layout::grid::layout::layout(
          doc,
          id,
          styles,
          Some(content[0]),
          height,
          slot.as_ref().map(|(_, grid)| grid),
          baselines,
          text,
          &mut measured.borrow_mut(),
          true,
        )
      };
      let grid = match measure {
        Some(grid) => {
          let mut grid = Grid::clone(&grid);
          grid.set_first_baseline(styles, baselines);
          debug_assert_eq!(grid, lay(), "grid {id} differs from its measure");
          grid
        }
        None => lay(),
      };
      drop(store);
      view.store.borrow_mut().state(view.id).layout.grids.insert(
        id,
        (
          content[0],
          height,
          slot.as_ref().map(|(_, grid)| grid.clone()),
          grid.clone(),
        ),
      );
      grid
    }))
  } else {
    None
  };
  if let Some(grid) = &grid {
    let mut store = view.store.borrow_mut();
    let tracks = &mut store.state(view.id).layout.tracks;
    if tracks
      .get(&id)
      .is_none_or(|[columns, rows]| **columns != *grid.columns || **rows != *grid.rows)
    {
      tracks.insert(
        id,
        [grid.columns.as_slice().into(), grid.rows.as_slice().into()],
      );
    }
    drop(store);
    el = el.flex().flex_col();
    for (index, item) in items.iter_mut().enumerate() {
      let key = !(id * 2 + index);
      let Ok(at) = grid.cells.binary_search_by_key(&key, |cell| cell.0) else {
        continue;
      };
      let Some((order, mut decoration, mut margin)) = item.take() else {
        continue;
      };
      // Like a grid item, a pseudo-element takes percentages from its grid area.
      if let Some(pseudo) = styles.generated(id).and_then(|pseudos| pseudos.get(index)) {
        let base = grid.area(grid.cells[at].1)[0];
        let pseudo_units = pseudo_lengths(&units, style, pseudo, text, pixel_ratio);
        let [padding, borders] = [
          pseudo_units
            .edges(pseudo, css::properties::PADDING, base)
            .map(|v| v.max(0.)),
          pseudo_units.borders(pseudo, base),
        ];
        margin = pseudo_units.edges(pseudo, css::properties::MARGIN, base);
        let own = decoration.style();
        own.padding = renderer::Edges {
          top: px(padding[0]).into(),
          right: px(padding[1]).into(),
          bottom: px(padding[2]).into(),
          left: px(padding[3]).into(),
        };
        let side = |at: usize, old: renderer::Length| match pseudo.get(css::properties::MARGIN[at])
        {
          Some("auto") => old,
          _ => px(margin[at]).into(),
        };
        own.margin = renderer::Edges {
          top: side(0, own.margin.top),
          right: side(1, own.margin.right),
          bottom: side(2, own.margin.bottom),
          left: side(3, own.margin.left),
        };
        own.border_widths = renderer::Edges {
          top: px(borders[0]),
          right: px(borders[1]),
          bottom: px(borders[2]),
          left: px(borders[3]),
        };
      }
      let mut bounds = grid.cells[at].2;
      bounds[0] += padding[3];
      bounds[1] += padding[0];
      let Child::Node(placed) = place(decoration.into(), margin, pixel_ratio, bounds) else {
        continue;
      };
      // `::after` paints after the children.
      if index == 0 {
        el = el.child(*placed);
      } else {
        *item = Some((order, *placed, margin));
      }
    }
    el = match widest {
      Some(widest) => el.child(renderer::node().measure(
        widest,
        |&[min, max], _, known, available, _| {
          let width = known.width.unwrap_or(match available.width {
            renderer::AvailableSpace::MinContent => px(min),
            renderer::AvailableSpace::MaxContent => px(max),
            renderer::AvailableSpace::Definite(width) => width.min(px(max)).max(px(min)),
          });
          (renderer::size(width, px(0.)), None)
        },
      )),
      None if fills => el,
      None => el.w(px(
        content[0] + padding[1] + padding[3] + borders[1] + borders[3],
      )),
    };
    let rows = grid.extent(1).max(definite);
    let vertical = padding[0] + padding[2] + borders[0] + borders[2];
    if auto_height && !flexed {
      // The rows give the content height.
      let height = match el
        .style()
        .sizing
        .take()
        .and_then(|sizing| Some(sizing.height.as_ref()?(rows + vertical)))
      {
        Some(height) => height,
        None => rows + vertical,
      };
      el = el.h(px(height));
      el.style().aspect_ratio = None;
    }
    // The items are out of flow, so the rows give the min-content height for `min-height: auto` in a column.
    if !flexed
      && reader
        .parent(id, Prop::FLEX_DIRECTION)
        .is_some_and(|v| v.starts_with("column"))
      && reader
        .parent(id, Prop::DISPLAY)
        .is_some_and(|v| v.ends_with("flex"))
    {
      el = el.child(renderer::node().h(px(rows)));
    }
    // Absolute grid items need an in-flow content-height box for flex basis and min-height: auto.
    // Previously stretched rows cannot provide it.
    if flexed {
      let [.., height] = crate::render::layout::grid::intrinsic::intrinsic(
        doc,
        id,
        styles,
        Some(
          content[0] + padding[1] + padding[3] + borders[1] + borders[3] + margin[1] + margin[3],
        ),
        None,
        text,
        &mut measured.borrow_mut(),
      );
      el = el.child(renderer::node().h(px((height - vertical - margin[0] - margin[2]).max(0.))));
    }
    if !flexed && made.absolute {
      blocks.borrow_mut().insert(
        id,
        [
          content[0] + padding[1] + padding[3],
          if auto_height { rows } else { content[1] } + padding[0] + padding[2],
        ],
      );
    }
  }
  (el, grid)
}
