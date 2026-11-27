use super::super::STACK_LIMIT;
use super::super::columns::{column_set, columns};
use super::super::margin::{adjoining, block, clear_bits, in_flow, inline_fade, margins, run_cuts};
use super::super::{Build, element};
use super::create;
use super::cx::{Carry, Cx, Frame, spread};
use super::grid::grid_container;
use crate::css;
use crate::css::properties::Prop;
use crate::render::layout::size::place;
use crate::render::{
  layout::cull,
  paint::{Reach, text::InlineText},
};
use crate::style::{self};
use renderer::{Child, px, relative};
use std::rc::Rc;

/// Places the children of a block, flex or grid box.
// Inlined into create, so the stack check below runs in the frame of create, as one level of boxes.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub(super) fn children(
  cx: &Cx<'_>,
  fr: &Frame,
  el: renderer::Node,
  carry: &mut Carry,
  own: Option<[f32; 2]>,
  preserve: bool,
  cut_parts: &mut Vec<(usize, usize, crate::render::paint::Piece)>,
  reach: &mut Reach,
) -> renderer::Node {
  let Cx {
    reader,
    build,
    id,
    node,
    style,
    units,
    parsed,
    display,
    position: _,
    containing,
    container,
    slot,
    flow,
    multicol,
    flat,
    cuts,
    cut_pseudos,
    all_inline,
    line_justify,
    rtl,
    ref hit,
    flexbox,
    ..
  } = *cx;
  let Build {
    blocks: _,
    culls,
    doc,
    info,
    measured,
    pixel_ratio,
    styles,
    text: _,
    top_layer,
    view,
    flex_parent,
    ..
  } = *reader;
  let Carry {
    items,
    lines,
    static_anchor,
    pseudo_atoms,
    ..
  } = carry;
  let Frame {
    padding, borders, ..
  } = *fr;
  let content_box = || fr.content_box(cx);
  let (line_height, made) = (parsed.line_height, parsed.made);
  let mut content = content_box();
  let container = own.unwrap_or(container);
  // Content-sized heights size a grid like `auto`.
  let auto_height = style
    .get(Prop::HEIGHT)
    .is_none_or(|v| v == "auto" || css::value::content_size(v));
  let painted = el.kids().len();
  let (mut el, grid) = grid_container(cx, fr, el, items, &mut content, auto_height);
  // The border box of a table, which paints, has its captions outside.
  if let Some([above, below]) = (grid.as_ref())
    .map(|grid| grid.captions)
    .filter(|captions| *captions != [0.; 2])
  {
    let mut kids = el.take_children();
    let sizer = kids.split_off(painted);
    let paints = kids.split_off(usize::from(cx.stacking).min(kids.len()));
    let mut frame = renderer::node()
      .absolute()
      .top(px(above - fr.borders[0]))
      .right(px(-fr.borders[1]))
      .bottom(px(below - fr.borders[2]))
      .left(px(-fr.borders[3]))
      .children(paints);
    let style = el.style();
    let own = frame.style();
    own.border_widths = style.border_widths;
    own.background = style.background.take();
    own.border_color = style.border_color.take();
    own.border_style = style.border_style;
    own.corner_radii = std::mem::take(&mut style.corner_radii);
    own.box_shadow = std::mem::take(&mut style.box_shadow);
    if style.overflow.x != renderer::Overflow::Visible {
      own.escape = renderer::Escape::OUTLINE;
    }
    let fixed = (!auto_height && slot.is_none()).then_some(&mut style.size.height);
    let limits = [&mut style.min_size.height, &mut style.max_size.height];
    for height in fixed.into_iter().chain(limits) {
      if let renderer::Length::Px(height) = height {
        *height += px(above + below);
      }
    }
    el = el.children(kids).child(frame).children(sizer);
  }
  let mut children = if grid.is_some() && !flat.is_empty() {
    node.children.clone()
  } else {
    flat.to_vec()
  };
  // Each level of boxes takes the frames of create and its stages, so levels past the stack budget are not drawn.
  let marker = 0u8;
  if (&raw const marker as usize) < STACK_LIMIT.with(|limit| *limit) {
    children.clear();
  }
  if grid.is_some() {
    children.sort_by_key(|child| {
      styles.values[*child]
        .get(Prop::Z_INDEX)
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0)
    });
  } else if flexbox {
    let order = |child: &usize| {
      styles.values[*child]
        .get(Prop::ORDER)
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0)
    };
    if children.iter().any(|child| order(child) != 0) {
      children.sort_by_key(order);
    }
  }
  let inline_element = |child: Option<&usize>| {
    child.is_some_and(|&child| {
      let node = &doc.nodes[child];
      node.tag == "#text" && !node.text.trim_ascii().is_empty()
        || !node.tag.starts_with('#') && node.tag != "br" && cx.inline_level(child)
    })
  };
  let multicol = columns(style, &units, content[0]).filter(|_| multicol);
  let mut line_strut = false;
  let open_line = |decoration: renderer::Node, line_strut: &mut bool| {
    let (first, rest, aligned) = cx.with_strut(decoration.into(), line_justify);
    *line_strut = aligned;
    let mut next = renderer::node()
      .flex()
      .flex_row()
      .flex_wrap()
      .child(first)
      .children(rest);
    next.style().justify_content = line_justify;
    next.style().align_items = Some(renderer::AlignItems::Baseline);
    next.style().inline = renderer::Inline::Lines;
    next
  };
  let [lead, tail] = std::mem::take(lines);
  let mut line = None;
  if let Some(lead) = lead {
    if grid.is_some() {
      el = el.child(lead);
    } else {
      line = Some(open_line(lead, &mut line_strut));
    }
  }
  let mut lone = (0, None);
  let mut joined = usize::MAX;
  let mut floating = Vec::new();
  let columns = multicol.filter(|_| flow && grid.is_none() && !all_inline);
  let mut collected: Vec<(Option<usize>, Child)> = Vec::new();
  let shown = |index: usize| {
    // Columns only size and paint the cells of a table.
    if grid.is_some()
      && style::table_box(style::role(styles, id))
      && (matches!(doc.nodes[children[index]].tag.as_str(), "col" | "colgroup")
        || styles.values[children[index]]
          .get(Prop::DISPLAY)
          .is_some_and(|v| v.starts_with("table-column")))
    {
      return false;
    }
    // A plain block is an element, and its flags are closer than its node.
    if info[children[index]].flags & cull::PLAIN_BLOCK != 0 {
      return true;
    }
    let child = &doc.nodes[children[index]];
    if (flexbox || grid.is_some()) && child.tag == "#text" {
      return measured.borrow().text_item(doc, children[index], preserve);
    }
    preserve
      || !(child.tag == "#text" && child.text.trim_ascii().is_empty())
      || flow
        && grid.is_none()
        && inline_element(index.checked_sub(1).and_then(|index| children.get(index)))
        && inline_element(children.get(index + 1))
  };
  let wrap = display == "flex"
    && style.get(Prop::FLEX_DIRECTION).is_none_or(|v| v == "row")
    && style.get(Prop::FLEX_WRAP).is_some_and(|v| v == "wrap")
    && style
      .get(Prop::ALIGN_CONTENT)
      .is_none_or(|v| matches!(v, "normal" | "stretch" | "flex-start" | "start"))
    && style
      .get(Prop::ALIGN_ITEMS)
      .is_none_or(|v| !v.contains("baseline"))
    && [Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT]
      .into_iter()
      .all(|name| {
        style
          .get(name)
          .is_none_or(|v| matches!(v, "auto" | "none" | "0" | "0px"))
      })
    && !flex_parent
    && slot.is_none()
    && info[id].flags & cull::CHILDREN_CHANGED == 0;
  let stacks = grid.is_none()
    && columns.is_none()
    && !all_inline
    && display != "inline"
    && if flexbox {
      // An ordered pseudo-element goes between the children, maybe inside a run.
      items.iter().flatten().all(|(order, ..)| *order == 0)
        && (wrap
          || style
            .get(Prop::FLEX_DIRECTION)
            .is_some_and(|v| v == "column")
            && style.get(Prop::FLEX_WRAP).is_none_or(|v| v == "nowrap")
            && style
              .get(Prop::JUSTIFY_CONTENT)
              .is_none_or(|v| matches!(v, "normal" | "flex-start" | "start" | "stretch")))
    } else {
      // Floats are uncullable, so a run of blocks becomes a spacer only between them.
      flow
    };
  *reach = if info[id].flags & cull::MOVES_CONTENT != 0 {
    Reach::Moves
  } else if grid.is_some() || stacks && (!flexbox || wrap) {
    Reach::Stacks
  } else if stacks {
    Reach::Column
  } else {
    Reach::Plain
  };
  let stack = build.zone.clone().filter(|_| stacks);
  #[cfg(debug_assertions)]
  let checked = build
    .checking
    .then(|| view.kept.borrow().get(&id).map(|kept| kept.plan.clone()))
    .flatten();
  #[cfg(debug_assertions)]
  let mut culled_ids = Vec::new();
  let mut plan = None;
  if let Some(zone) = stack {
    let store = view.store.borrow();
    if let Some(dom) = store.page_state(view.id)
      && let Some(&[left, ..]) = dom.geometry.mirror.get(&id)
      && let Some(&[width, _]) = dom.layout.cull.widths.get(&id)
    {
      let gap = if flexbox {
        style
          .get(Prop::ROW_GAP)
          .and_then(|v| units.resolve(v, containing[0]))
          .unwrap_or(0.)
          .max(0.)
      } else {
        0.
      };
      let (slots, mut runs) = if wrap {
        cull::lines(
          &children,
          |index| shown(index) && info[children[index]].flags & cull::OUT_OF_FLOW == 0,
          |child| {
            let placed = *dom.layout.cull.placed.get(child)?;
            let bounds = dom.geometry.mirror.get(&child)?;
            let [top, _, bottom, left] = placed.2;
            let member = cull::member(
              info[child],
              placed,
              Some(bounds),
              &zone,
              units.viewport[1],
              false,
            );
            Some((
              member.is_some(),
              [
                bounds[0] - left,
                bounds[1] - top,
                bounds[0] - left + placed.1[0],
                bounds[1] + bounds[3] + bottom,
              ],
            ))
          },
          gap,
        )
      } else {
        cull::plan(
          &children,
          shown,
          |child| {
            cull::member(
              info[child],
              dom
                .layout
                .cull
                .placed
                .get(child)
                .copied()
                .unwrap_or_default(),
              dom.geometry.mirror.get(&child),
              &zone,
              units.viewport[1],
              flexbox,
            )
            .map(|(bounds, margins)| (bounds, margins, clear_bits(&styles.values[child])))
          },
          gap,
        )
      };
      if wrap {
        let mut watch = Vec::new();
        for (index, &child) in children.iter().enumerate() {
          match slots[index] {
            0 => {
              let placed = dom
                .layout
                .cull
                .placed
                .get(child)
                .copied()
                .unwrap_or_default();
              if placed.0 != cull::revision(info[child]) {
                watch.push((child, placed.1[0]));
              }
            }
            slot => {
              if let Some(run) = runs[slot - 1]
                .as_mut()
                .filter(|run| run.members[0] == child)
              {
                run.width = width;
                run.watch = std::mem::take(&mut watch);
              }
            }
          }
        }
      }
      if !flexbox {
        let collapsed = |child: usize, side: usize| {
          let mut units = styles.lengths[child];
          units.container = Some(container);
          margins(build, child, &units, content[0], false, false)[side]
        };
        for run in runs.iter_mut().flatten() {
          run.margins = [
            collapsed(run.members[0], 0),
            collapsed(run.members[run.members.len() - 1], 2),
          ];
        }
      }
      // The x of the content: the box moves its blocks, and so does its own scroll.
      let x = left + f32::from(dom.geometry.mirror_offset(id).x);
      plan = (!runs.is_empty()).then_some((slots, runs, [x, width], zone));
    }
  }
  let mut culled = Vec::new();
  if let (Some(grid), Some(zone)) = (&grid, &build.zone) {
    if let Some(dom) = view.store.borrow().page_state(view.id) {
      for &(child, _, [x, y, width, height], _) in &grid.cells {
        if crate::render::layout::grid::intrinsic::pseudo_item(child).is_some() {
          continue;
        }
        let placed = dom
          .layout
          .cull
          .placed
          .get(child)
          .copied()
          .unwrap_or_default();
        let [top, right, bottom, left] = placed.2;
        // Compare unsnapped cell sizes: layout snaps them later.
        if placed.1 == [width, height]
          && cull::member(
            info[child],
            placed,
            dom.geometry.mirror.get(&child),
            zone,
            units.viewport[1],
            false,
          )
          .is_some()
        {
          // Layout truncates each inset and margin to layout units, so the offsets from the anchor do too.
          let snap = |value: f32| renderer::to_layout_unit(value, pixel_ratio) / pixel_ratio;
          culled.push((
            child,
            [
              snap(x + padding[3]) + snap(left),
              snap(y + padding[0]) + snap(top),
              width - left - right,
              height - top - bottom,
            ],
          ));
        }
      }
    }
    if !culled.is_empty() {
      culls.set(culls.get() + 1);
      #[cfg(debug_assertions)]
      culled_ids.extend(culled.iter().map(|&(child, _)| child));
      el = el.child(crate::render::paint::spacer::anchor(
        culled.clone(),
        build.ancestors.clone(),
        zone.clone(),
        view.clone(),
      ));
    }
    if let Some(baseline) = grid.first_baseline.filter(|_| {
      grid
        .first_item
        .is_some_and(|item| culled.binary_search_by_key(&item, |&(id, _)| id).is_ok())
    }) {
      let baseline = baseline.max(0.);
      let mut line = renderer::node()
        .w(px(0.))
        .h(px(baseline))
        .mb(px(-baseline))
        .flex_shrink_0()
        .baseline(renderer::Baseline::Bottom);
      line.style().baseline_source = true;
      el = el.child(line);
    }
  }
  // Only a box that confines can hold boxes whose changes don't reach it.
  if build.zone.is_some() {
    let confines = |child: &usize| info[*child].flags & cull::CONFINES != 0;
    let mut set = view.culled.borrow_mut();
    for child in children.iter().filter(|child| confines(child)) {
      set.remove(child);
    }
    set.extend(
      culled
        .iter()
        .map(|(child, _)| child)
        .filter(|child| confines(child)),
    );
    if let Some((_, runs, ..)) = &plan {
      set.extend(
        runs
          .iter()
          .flatten()
          .flat_map(|run| &run.members)
          .filter(|child| confines(child)),
      );
    }
  }
  macro_rules! place {
    ($element:expr, $child:expr) => {{
      let element: renderer::Child = $element.into();
      if std::mem::take(&mut carry.strut_next) && columns.is_none() {
        el = el.child(cx.strut());
      }
      if std::mem::take(&mut carry.strut_due) {
        el = el.child(cx.strut());
      }
      match columns {
        Some(_) => collected.push(($child, element)),
        None => el.push(element),
      }
    }};
  }
  let line_break = || {
    let mut el = renderer::node().w_full();
    el.style().align_self = Some(renderer::AlignItems::FlexStart);
    el
  };
  let hangs = flow
    && !preserve
    && style
      .get(Prop::WHITE_SPACE)
      .is_none_or(|value| value != "nowrap")
    && style
      .get(Prop::TEXT_WRAP)
      .is_none_or(|value| !value.contains("nowrap"));
  el.reserve(children.len());
  let mut item_margins =
    rustc_hash::FxHashMap::<(*const css::properties::Style, u32), [f32; 4]>::default();
  let (mut scanned, mut last_flow, mut seen, mut flowed) = (0, None, 0, false);
  if children.len() == flat.len() {
    for &(cut, from, to) in cuts {
      let mut at = from;
      while at < to {
        let blocky = |child: usize| {
          !cx.in_run(child) && !cx.floated(child) && info[child].flags & cull::OUT_OF_FLOW == 0
        };
        if !blocky(children[at]) {
          at += 1;
          continue;
        }
        let mut last = at;
        while last + 1 < to && (blocky(children[last + 1]) || !cx.in_run(children[last + 1])) {
          last += 1;
        }
        while !blocky(children[last]) {
          last -= 1;
        }
        let key = |at: usize| match cut_pseudos.binary_search_by_key(&at, |part| part.0) {
          Ok(found) => {
            crate::render::inline::atom_key(children[at], cut_pseudos[found].1 as u8 + 1)
          }
          Err(_) => children[at],
        };
        cut_parts.push((
          cut,
          2 * at + 1,
          crate::render::paint::Piece::Blocks(key(at), key(last), {
            let last = children[last];
            styles.lengths[last].edges(&styles.values[last], css::properties::MARGIN, 0.)[2]
          }),
        ));
        at = last + 1;
      }
    }
  }
  // A spliced box, and an inline box in the text, has no box of its own, so its blocks and atoms take its
  // opacity.
  let fade = |child: usize| inline_fade(doc, styles, child, id);
  let faded = |child: usize, element: Child| {
    let fade = fade(child);
    match fade < 1. {
      true => renderer::node()
        .flex()
        .flex_col()
        .child(element)
        .hooks(crate::render::paint::Effected {
          effect: renderer::PaintEffect {
            opacity: fade,
            ..Default::default()
          },
          backdrop: None,
        })
        .into(),
      false => element,
    }
  };
  let (mut run_end, mut tried) = (0, 0);
  for index in 0..=children.len() {
    if index < run_end {
      continue;
    }
    let child = children.get(index).copied();
    let paragraphs = index >= tried
      && flow
      && !all_inline
      && !flexbox
      && grid.is_none()
      && line.is_none()
      && culled.is_empty();
    // The edges of spliced boxes next to a block and no other line take a line of their own.
    let edges = if paragraphs
      && !cuts.is_empty()
      && child.is_none_or(|child| !cx.in_run(child))
      && (index == 0 || !cx.in_run(children[index - 1]))
    {
      run_cuts(build, cuts, &children, [index, index], |child| {
        cx.in_run(child)
      })
    } else {
      Vec::new()
    };
    if paragraphs && (child.is_some_and(|child| cx.in_run(child)) || !edges.is_empty()) {
      let end = index
        + children[index..]
          .iter()
          .take_while(|&&kid| cx.in_run(kid))
          .count();
      tried = end;
      let run = &children[index..end];
      let (found, parts) = cx.run_text(index, end);
      if let Some(data) = found {
        #[cfg(debug_assertions)]
        reader.extra.set(super::super::kept::hashed((
          reader.extra.get(),
          &*data.text,
        )));
        cut_parts.extend(
          (parts.iter())
            .map(|&(cut, _)| (cut, 2 * index, crate::render::paint::Piece::Text(data.key))),
        );

        let content = content_box();
        let mut atom_elements: Vec<Child> = data
          .atoms
          .iter()
          .map(|atom| match atom.pseudo {
            0 => element(build, atom.node, content, container, None),
            pseudo => pseudo_atoms[usize::from(pseudo - 1)]
              .take()
              .unwrap_or_default()
              .into(),
          })
          .collect();
        let atom_slots: Rc<[crate::render::paint::text::AtomSlot]> = data
          .atoms
          .iter()
          .zip(&mut atom_elements)
          .map(|(atom, child)| cx.atom_slot(atom, child, content[0], &|| [0.; 3]))
          .collect();
        let marks: Rc<[_]> = data.marks.iter().map(|_| Default::default()).collect();
        let (justify, last) = crate::style::row_aligns(style, rtl);
        let plaintext = last.is_none()
          && style
            .get(Prop::TEXT_ALIGN)
            .is_none_or(|value| value == "start")
          && crate::style::plaintext(doc, styles, id);
        let mut paragraph = renderer::Node::from(InlineText {
          data: data.clone(),
          mixed: data.mixed(build.text),
          atoms: atom_slots.clone(),
          marks: Rc::clone(&marks),
          view: view.clone(),
          shaped: Default::default(),
          prepainted: Default::default(),
          rtl,
          plaintext,
          justify,
          last,
          reversed: style.get(Prop::WRITING_MODE) == Some("vertical-lr"),
          room: cx.margin_room(&data, id),
        });
        // Text keeps its box when runs come before it, so it can stay a scroll anchor.
        paragraph = paragraph.id(("text", data.key));
        paragraph.style().flow = renderer::Flow::BLOCK;
        if last.is_some() || justify[1] || plaintext {
          paragraph.style().flex_grow = 1.;
        }
        if data.greedy {
          paragraph
            .text_style()
            .set_text_wrap(renderer::TextWrap::Wrap);
        }
        let mut block = renderer::node()
          .flex()
          .flex_col()
          .child(paragraph.min_w(px(0.)).max_w(relative(1.)).flex_shrink(1.));
        for (at, (atom, child)) in data.atoms.iter().zip(atom_elements).enumerate() {
          let atom_box = cx.atom_box(at, atom, child, &atom_slots, [0.; 4]);
          block = block.child(match fade(atom.node) {
            fade if fade < 1. => atom_box.hooks(crate::render::paint::Effected {
              effect: renderer::PaintEffect {
                opacity: fade,
                ..Default::default()
              },
              backdrop: None,
            }),
            _ => atom_box,
          });
        }
        let struts = (
          std::mem::take(&mut carry.strut_due),
          std::mem::take(&mut carry.strut_next),
        );
        // Floats of the line come first, so layout places them before it.
        for &kid in run.iter().filter(|&&kid| cx.floated(kid)) {
          place!(
            element(build, kid, content_box(), container, None),
            Some(kid)
          );
        }
        place!(
          block.children(super::paragraph::beside(cx, fr, &data, &marks, [0.; 4])),
          None
        );
        (carry.strut_due, carry.strut_next) = struts;
        if !run.is_empty() {
          run_end = end;
          continue;
        }
      } else {
        // A run that lays out as boxes gives the spliced boxes in it the bounds of their boxes.
        for &(cut, from, to) in cuts {
          let nodes: Vec<usize> = (from.max(index)..to.min(end))
            .map(|at| children[at])
            .collect();
          if !nodes.is_empty() {
            cut_parts.push((cut, 2 * index, crate::render::paint::Piece::Nodes(nodes)));
          }
        }
      }
    }
    let Some(child) = child else {
      break;
    };
    if let Ok(found) = cut_pseudos.binary_search_by_key(&index, |part| part.0) {
      if let Some(open) = line.take() {
        place!(open, None);
      }
      let part = cut_pseudos[found].1;
      let decoration = create(build, child, content_box(), container, None, Some(part));
      // The pseudo-element is in its box, which `faded` leaves out.
      let fade = crate::render::effects::opacity(&styles.values[child]);
      let decoration = match fade < 1. {
        true => renderer::node()
          .flex()
          .flex_col()
          .child(decoration)
          .hooks(crate::render::paint::Effected {
            effect: renderer::PaintEffect {
              opacity: fade,
              ..Default::default()
            },
            backdrop: None,
          })
          .into(),
        false => decoration,
      };
      place!(faded(child, decoration), None);
      continue;
    }
    // Flex places a pseudo-element before the children of a higher order, and `::before` also before
    // those of its own.
    if flexbox && items.iter().any(Option::is_some) {
      let order = styles.values[child]
        .get(Prop::ORDER)
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0);
      let first = usize::from(
        items[1]
          .as_ref()
          .zip(items[0].as_ref())
          .is_some_and(|((after, ..), (before, ..))| after < before),
      );
      for pending in [first, 1 - first] {
        if items[pending]
          .as_ref()
          .is_some_and(|(own, ..)| *own < order || pending == 0 && *own == order)
          && let Some((_, decoration, _)) = items[pending].take()
        {
          place!(decoration, None);
        }
      }
    }
    if index != joined && shown(index) {
      if let Some((slots, runs, then, zone)) = plan.as_mut()
        && let Some(&slot) = slots.get(index).filter(|&&slot| slot > 0)
      {
        if let Some(run) = runs[slot - 1].take() {
          culls.set(culls.get() + 1);
          #[cfg(debug_assertions)]
          culled_ids.extend(&run.members);
          if let Some(open) = line.take() {
            place!(open, None);
          }
          place!(
            crate::render::paint::spacer::spacer(
              run,
              build.ancestors.clone(),
              *then,
              zone.clone(),
              view.clone()
            ),
            None
          );
        }
        continue;
      }
      if culled.binary_search_by_key(&child, |&(id, _)| id).is_ok() || top_layer.contains(&child) {
        continue;
      }
      #[cfg(debug_assertions)]
      if checked
        .as_ref()
        .is_some_and(|ids| ids.binary_search(&child).is_ok())
      {
        continue;
      }
      let placement = grid.as_ref().and_then(|grid| {
        let index = grid
          .cells
          .binary_search_by_key(&child, |(id, ..)| *id)
          .ok()?;
        let (item, [column, row, columns, rows], mut bounds, baseline) = *grid.cells.get(index)?;
        bounds[0] += padding[3];
        bounds[1] += padding[0];
        let area = grid.area([column, row, columns, rows]);
        let [columns, rows] = [
          (
            Prop::GRID_TEMPLATE_COLUMNS,
            &grid.columns,
            column..column + columns,
          ),
          (Prop::GRID_TEMPLATE_ROWS, &grid.rows, row..row + rows),
        ]
        .map(|(prop, tracks, range)| {
          let subgrid = styles.values[child]
            .get(prop)
            .is_some_and(|v| v.starts_with("subgrid"));
          tracks
            .get(range)
            .map(|tracks| if subgrid { tracks.to_vec() } else { Vec::new() })
        });
        Some((
          bounds,
          crate::render::layout::grid::Grid {
            columns: columns?,
            rows: rows?,
            gap: grid.gap,
            baseline,
            first_item: grid.first_item.filter(|&first| first == item),
            ..Default::default()
          },
          area,
        ))
      });
      let (placement, area) = match placement {
        Some((bounds, grid, area)) => (Some((bounds, grid)), Some(area)),
        None => (None, None),
      };
      // The containing block of a grid item is its grid area. Table parts take it from their table.
      let own = area.filter(|_| !style::table_part(doc, styles, child));
      if let Some(own) = own {
        let mut store = view.store.borrow_mut();
        store.state(view.id).layout.areas.insert(child, own);
      }
      let bounds = placement.as_ref().map(|(bounds, _)| *bounds);
      // A kept text field builds again from its patch, which must hold its new place.
      let field = placement
        .as_ref()
        .filter(|_| view.patches.borrow().contains_key(&child))
        .cloned();
      // The containing block of a positioned child is the padding box.
      let positioned = match styles.values[child].get(Prop::POSITION) {
        Some("absolute") => made.absolute,
        Some("fixed") => made.fixed,
        _ => false,
      };
      let containing = if positioned {
        [
          content[0] + padding[1] + padding[3],
          content[1] + padding[0] + padding[2],
        ]
      } else if let Some(own) = own {
        own
      } else if let Some((count, gap, _)) = columns {
        // Boxes in columns take percentages from the width of a column.
        let count = count as f32;
        [
          ((content[0] - gap * (count - 1.)) / count).max(0.),
          content[1],
        ]
      } else {
        content
      };
      let child_element = element(build, child, containing, container, placement);
      let child_element = match cx.inline_level(child)
        || cx.floated(child)
        || info[child].flags & cull::OUT_OF_FLOW != 0
      {
        true => child_element,
        false => faded(child, child_element),
      };
      if bounds.is_some()
        && let Child::Kept(_) = child_element
        && let Some(patch) = view.patches.borrow_mut().get_mut(&child)
      {
        patch.slot = field;
      }
      // A kept grid item can take another place, which a build in place must keep.
      if let (Some(bounds), Child::Kept(_)) = (bounds, &child_element)
        && let Some(record) = view.kept.borrow_mut().get_mut(&child)
        && let Some(slot) = &mut record.slot
      {
        slot.0 = bounds;
      }
      let child_element = match bounds {
        Some(mut bounds) => {
          let style = &styles.values[child];
          // Like Chrome, the offsets of a relative grid item take percentages of its grid area.
          if style.get(Prop::POSITION) == Some("relative")
            && let Some(area) = area
          {
            let units = &styles.lengths[child];
            for (axis, [start, end]) in [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]]
              .into_iter()
              .enumerate()
            {
              let inset = |prop| {
                style
                  .get(prop)
                  .and_then(|value| units.resolve(value, area[axis]))
              };
              // With both offsets, `left` wins, and `right` in right-to-left text.
              let end = inset(end).map(|value| -value);
              bounds[axis] += match end {
                Some(end) if axis == 0 && style.get(Prop::DIRECTION) == Some("rtl") => Some(end),
                _ => inset(start).or(end),
              }
              .unwrap_or(0.);
            }
          }
          let base = own.map_or(content[0], |area| area[0]);
          let key = (Rc::as_ptr(style), base.to_bits());
          let margin = *item_margins.entry(key).or_insert_with(|| {
            let mut units = styles.lengths[child];
            units.container = Some(container);
            units.edges(style, css::properties::MARGIN, base)
          });
          place(child_element, margin, pixel_ratio, bounds)
        }
        None => child_element,
      };
      let out_of_flow = styles.values[child]
        .get(Prop::POSITION)
        .is_some_and(|value| matches!(value, "absolute" | "fixed"));
      let child_style = &styles.values[child];
      let child_element = match children.get(index + 1) {
        Some(&next)
          if hangs
            && grid.is_none()
            && !out_of_flow
            && !cx.floated(child)
            && cx.percent_width(&child_element).is_none()
            && doc.nodes[next].tag == "#text"
            && doc.nodes[next].text.trim_ascii().is_empty()
            && shown(index + 1) =>
        {
          joined = index + 1;
          let mut child_element = child_element;
          let mut space = element(build, next, content, container, None);
          if let renderer::Child::Node(node) = &mut space {
            node.style().flex_shrink = 1e6;
          }
          let mut joint = renderer::node().flex().flex_row().flex_shrink(1.);
          joint.style().align_items = Some(renderer::AlignItems::Baseline);
          joint.style().align_self = cx.align_of(&mut child_element);
          joint.style().inline = renderer::Inline::Hangs;
          joint.child(child_element).child(space).into()
        }
        // A space of its own also hangs at the end of a line.
        _ if hangs
          && grid.is_none()
          && doc.nodes[child].tag == "#text"
          && doc.nodes[child].text.trim_ascii().is_empty() =>
        {
          let mut space = child_element;
          if let renderer::Child::Node(node) = &mut space {
            node.style().inline = renderer::Inline::Hangs;
          }
          space
        }
        _ => child_element,
      };
      // A float is a box of the column, and layout places it beside the lines and blocks after it.
      if cx.floated(child) {
        if let Some(open) = line.take() {
          place!(open, None);
        }
        place!(child_element, None);
        continue;
      }
      if flow
        && grid.is_none()
        && child_style
          .get(Prop::POSITION)
          .is_some_and(|value| matches!(value, "absolute" | "fixed"))
        && child_style
          .get(Prop::DISPLAY)
          .is_none_or(|value| value != "none")
        && [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]]
          .iter()
          .any(|axis| {
            axis
              .iter()
              .all(|prop| child_style.get(*prop).is_none_or(|value| value == "auto"))
          })
      {
        let block = child_style
          .get(Prop::DISPLAY)
          .is_some_and(|value| !value.starts_with("inline"))
          && !child_style.contains(Prop::INTERNAL_STATIC_INLINE);
        // Outside a line, a box that was inline starts where a new line would.
        let pending = if line.is_none() && !all_inline {
          for &other in &children[scanned..index] {
            if in_flow(doc, info, other) && !cx.floated(other) {
              last_flow = Some(other);
            }
          }
          scanned = index;
          last_flow
        } else {
          None
        };
        let pending = pending
          .filter(|&other| self::block(build, other))
          .map_or(0., |other| {
            let (_, [positive, negative], empty) =
              adjoining(build, other, content[0], Some(container));
            let mut units = styles.lengths[other];
            units.container = Some(container);
            let used = margins(build, other, &units, content[0], false, false);
            // An empty box passes its top margin on too.
            let used = used[2] + if empty { used[0] } else { 0. };
            (positive + negative - used).max(0.)
          });
        let mut placeholder = super::super::query::static_mark(child, pending, false)
          .flex_shrink_0()
          .h(px(
            if block
              && (line.is_some()
                || all_inline && {
                  for &other in &children[seen..index] {
                    flowed |= in_flow(doc, info, other);
                  }
                  seen = index;
                  flowed
                })
            {
              units.font.lh
            } else {
              0.
            },
          ));
        placeholder.style().align_self = Some(renderer::AlignItems::FlexStart);
        match line.take() {
          Some(open) => line = Some(open.child(placeholder)),
          None => place!(placeholder, None),
        }
      }
      if flow
        && !all_inline
        && grid.is_none()
        && line.is_none()
        && !out_of_flow
        && crate::render::inline::makes_no_line(doc, styles, child)
      {
        place!(
          renderer::node().flex().flex_row().child(child_element),
          None
        );
      } else if flow && !all_inline && grid.is_none() && cx.inline_level(child) && !out_of_flow {
        let mut next = match line.take() {
          Some(open) if std::mem::take(&mut line_strut) => {
            open.child(cx.strut()).child(child_element)
          }
          // Like a line box, each row has the strut, also a row that starts inside an inline element.
          Some(open) if !doc.nodes[child].tag.starts_with('#') => {
            let (first, rest, _) = cx.with_strut(child_element, line_justify);
            open.child(first).children(rest)
          }
          Some(open) => open.child(child_element),
          None if doc.nodes[child].tag != "br" => {
            let (first, rest, aligned) = cx.with_strut(child_element, line_justify);
            line_strut = aligned;
            renderer::node()
              .flex()
              .flex_row()
              .flex_wrap()
              .child(first)
              .children(rest)
          }
          None => renderer::node()
            .flex()
            .flex_row()
            .flex_wrap()
            .child(cx.strut())
            .child(child_element),
        };
        next.style().justify_content = line_justify;
        next.style().align_items = Some(renderer::AlignItems::Baseline);
        next.style().inline = renderer::Inline::Lines;
        if doc.nodes[child].tag == "br" {
          place!(next, None);
        } else {
          line = Some(next);
        }
      } else if out_of_flow {
        // Like Chrome, an absolute box of a grid without insets takes its place in the padding box by
        // its alignment, as the only item of a grid area. Grid lines make that area its containing
        // block.
        let child_element = if matches!(display, "grid" | "inline-grid")
          && made.absolute
          && child_style.get(Prop::POSITION) == Some("absolute")
          && ([[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]]
            .iter()
            .any(|axis| {
              axis
                .iter()
                .all(|prop| child_style.get(*prop).is_none_or(|value| value == "auto"))
            })
            || [
              Prop::GRID_COLUMN_START,
              Prop::GRID_COLUMN_END,
              Prop::GRID_ROW_START,
              Prop::GRID_ROW_END,
            ]
            .iter()
            .any(|prop| child_style.get(*prop).is_some_and(|v| v != "auto")))
        {
          let align = |own: Prop, items: Prop| {
            let value = child_style
              .get(own)
              .filter(|value| *value != "auto")
              .or_else(|| style.get(items))
              .unwrap_or("normal");
            let justify = own == Prop::JUSTIFY_SELF;
            let rtl = justify && style.get(Prop::DIRECTION) == Some("rtl");
            match value
              .trim_start_matches("unsafe ")
              .trim_start_matches("safe ")
            {
              "center" => 1,
              "left" if justify => 0,
              "right" if justify => 2,
              "end" | "flex-end" | "self-end" => 2 * usize::from(!rtl),
              _ => 2 * usize::from(rtl),
            }
          };
          let mut area = renderer::node()
            .absolute()
            .top(px(0.))
            .right(px(0.))
            .bottom(px(0.))
            .left(px(0.))
            .flex()
            .flex_col();
          // Grid lines of the box place its area on the tracks, and `auto` lines take the padding edges.
          if let Some(grid) = &grid {
            let rtl = style.get(Prop::DIRECTION) == Some("rtl");
            for (axis, [start, end], tracks, content_prop) in [
              (
                0,
                [Prop::GRID_COLUMN_START, Prop::GRID_COLUMN_END],
                &grid.columns,
                Prop::JUSTIFY_CONTENT,
              ),
              (
                1,
                [Prop::GRID_ROW_START, Prop::GRID_ROW_END],
                &grid.rows,
                Prop::ALIGN_CONTENT,
              ),
            ] {
              let [start, end] = [start, end].map(|prop| child_style.get(prop).unwrap_or("auto"));
              let gap = grid.gap[axis];
              let total = tracks.iter().sum::<f32>() + gap * tracks.len().saturating_sub(1) as f32;
              let free = if axis == 1 && auto_height {
                0.
              } else {
                (content[axis] - total).max(0.)
              };
              let shift = free
                * match style.get(content_prop).unwrap_or("normal") {
                  "center" => 0.5,
                  "end" | "flex-end" => 1.,
                  _ => 0.,
                };
              // The distance of a grid line from the start padding edge.
              let edge = |line: usize| {
                [padding[3], padding[0]][axis]
                  + shift
                  + tracks[..line.min(tracks.len())].iter().sum::<f32>()
                  + gap * line as f32
              };
              // Lines past the explicit grid are `auto`.
              let (line, span) =
                crate::render::layout::grid::track::placement(start, end, &[], tracks.len());
              let line = line.filter(|line| line + span <= tracks.len());
              let first = line.filter(|_| start != "auto").map(edge);
              let last = match (line, end) {
                (_, "auto") => None,
                (Some(line), _) => Some(edge(line + span) - gap),
                (None, _) => {
                  crate::render::layout::grid::track::placement(end, "auto", &[], tracks.len())
                    .0
                    .map(|line| edge(line) - gap)
                }
              };
              if first.is_none() && last.is_none() {
                continue;
              }
              let at = first.unwrap_or(0.);
              let size = last.map(|last| (last - at).max(0.));
              let style = area.style();
              let (near, far, length) = match axis {
                0 if rtl => (
                  &mut style.inset.right,
                  &mut style.inset.left,
                  &mut style.size.width,
                ),
                0 => (
                  &mut style.inset.left,
                  &mut style.inset.right,
                  &mut style.size.width,
                ),
                _ => (
                  &mut style.inset.top,
                  &mut style.inset.bottom,
                  &mut style.size.height,
                ),
              };
              *near = px(at).into();
              if let Some(size) = size {
                (*far, *length) = (renderer::Length::Auto, px(size).into());
              }
            }
          }
          area.style().align_items = Some(
            [
              renderer::AlignItems::FlexStart,
              renderer::AlignItems::Center,
              renderer::AlignItems::FlexEnd,
            ][align(Prop::JUSTIFY_SELF, Prop::JUSTIFY_ITEMS)],
          );
          area.style().justify_content = Some(
            [
              renderer::AlignContent::FlexStart,
              renderer::AlignContent::Center,
              renderer::AlignContent::FlexEnd,
            ][align(Prop::ALIGN_SELF, Prop::ALIGN_ITEMS)],
          );
          area.style().contains = renderer::Escape::ABSOLUTE;
          if style.get(Prop::DIRECTION) == Some("rtl") {
            area.style().flow |= renderer::Flow::RTL_BLOCK;
          }
          area.child(child_element).into()
        } else {
          child_element
        };
        // In place, out-of-flow boxes paint in tree order with the positioned boxes around them.
        if line.is_none() && columns.is_none() {
          el.push(child_element);
        } else {
          floating.push(child_element);
        }
      } else {
        if let Some(line) = line.take() {
          place!(line, None);
        }
        // An inline-block with a line of text in the font of the line is as tall as the strut above and
        // below its baseline, so it needs none.
        let covers = || {
          let own = &styles.values[child];
          let Some(built) = (match &child_element {
            renderer::Child::Node(node) => Some(node.style_ref()),
            renderer::Child::Kept(id) | renderer::Child::Placed(id, _) => build.tree.style_of(*id),
          }) else {
            return false;
          };
          let below = |length: renderer::Length| match length {
            renderer::Length::Px(value) => value.0 < 0.,
            renderer::Length::Calc(fraction, offset) => fraction < 0. || offset.0 < 0.,
            renderer::Length::Fraction(fraction) => fraction < 0.,
            renderer::Length::Auto | renderer::Length::Bound(_) => false,
          };
          own.get(Prop::DISPLAY) == Some("inline-block")
            && own
              .get(Prop::VERTICAL_ALIGN)
              .is_none_or(|value| value == "baseline")
            && own
              .get(Prop::OVERFLOW_X)
              .is_none_or(|value| value == "visible")
            && own
              .get(Prop::OVERFLOW_Y)
              .is_none_or(|value| value == "visible")
            && own.get(Prop::CONTAIN).is_none_or(|value| value == "none")
            && own
              .get(Prop::CONTENT_VISIBILITY)
              .is_none_or(|value| value == "visible")
            && built.size.height == renderer::Length::Auto
            && built.max_size.height == renderer::Length::Auto
            && built.aspect_ratio.is_none()
            && !below(built.margin.top)
            && !below(built.margin.bottom)
            && own.get(Prop::FONT_FAMILY) == style.get(Prop::FONT_FAMILY)
            && styles.lengths[child].font.em == units.font.em
            && styles.lengths[child].font.lh == line_height
            && doc.nodes[child].children.iter().any(|&kid| {
              doc.nodes[kid].tag == "#text" && !doc.nodes[kid].text.trim_ascii().is_empty()
            })
        };
        let (child_element, aligned) = if (carry.strut_due
          || carry.struts
            && all_inline
            && joined != index + 1
            && !doc.nodes[child].tag.starts_with('#')
            && (display != "inline" || cx.percent_width(&child_element).is_none())
            && !covers())
          && doc.nodes[child].tag != "br"
        {
          carry.strut_due = false;
          let (first, rest, aligned) = cx.with_strut(child_element, line_justify);
          match rest {
            Some(element) => {
              place!(first, None);
              (element, aligned)
            }
            None => (first, aligned),
          }
        } else {
          (child_element, false)
        };
        let direct = columns.is_none();
        if display == "inline" && !doc.nodes[child].tag.starts_with('#') {
          lone = (lone.0 + 1, cx.percent_width(&child_element).flatten());
        }
        place!(child_element, Some(child));
        carry.strut_next = aligned && direct;
        if flow && all_inline && doc.nodes[child].tag == "br" {
          place!(line_break(), None);
        }
      }
    }
  }
  if let Some(anchor) = static_anchor.take() {
    match line.take() {
      Some(open) => line = Some(open.child(anchor)),
      None if all_inline => place!(anchor, None),
      None => line = Some(open_line(anchor, &mut line_strut)),
    }
  }
  match (line.take(), tail) {
    (Some(open), Some(tail)) if grid.is_none() => {
      let (first, rest, _) = cx.with_strut(tail.into(), line_justify);
      place!(open.child(first).children(rest), None);
    }
    (open, tail) => {
      if let Some(open) = open {
        place!(open, None);
      }
      if let Some(tail) = tail {
        match grid {
          Some(_) => place!(tail, None),
          None => place!(open_line(tail, &mut line_strut), None),
        }
      }
    }
  }
  // An inline box around one box with a percent width must not stand between that box and the line.
  if let (1, Some(share)) = lone
    && hit.is_none()
  {
    spread(el.style(), share, line_justify);
    el.style().size.width =
      renderer::Length::Calc(1., px(padding[1] + padding[3] + borders[1] + borders[3]));
  }
  #[cfg(debug_assertions)]
  {
    culled_ids.sort_unstable();
    reader.plan.replace(culled_ids.into());
  }
  if let Some((count, gap, rule)) = columns {
    // A `column-span: all` child splits the columns into sets above and below it.
    let spanner = |child: &Option<usize>| {
      child.is_some_and(|child| styles.values[child].get(Prop::COLUMN_SPAN) == Some("all"))
    };
    let alone = !collected.iter().any(|(child, _)| spanner(child));
    let mut set = Vec::new();
    for (child, item) in collected {
      if !spanner(&child) {
        set.push(item);
        continue;
      }
      if !set.is_empty() {
        let items = std::mem::take(&mut set);
        el = el.child(column_set(style, content, count, gap, rule, alone, items));
      }
      el = el.child(item);
    }
    if !set.is_empty() {
      el = el.child(column_set(style, content, count, gap, rule, alone, set));
    }
  }
  el = el.children(floating);
  el
}
