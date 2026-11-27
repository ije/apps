use super::super::Build;
use super::super::kept::{Query, hashed};
use super::super::query::{href, uses};
use super::super::svg::{SVG_SOURCE, SvgSource, svg_layer};
use super::cx::Cx;
use crate::css;
use crate::css::properties::Prop;
use crate::render::layout::size::dimension;
use crate::style::{self};
use renderer::{Child, px};
use std::{rc::Rc, sync::Arc};

/// Writes the SVG source of an inline `<svg>` for its raster, and sizes its box.
#[inline(never)]
pub(super) fn svg_box(
  cx: &Cx<'_>,
  mut el: renderer::Node,
  content_layers: &mut Vec<Child>,
) -> renderer::Node {
  let Cx {
    reader,
    id,
    node,
    style,
    units,
    position,
    containing,
    ..
  } = *cx;
  let Build {
    doc,
    styles,
    ids,
    view,
    flex_parent,
    ..
  } = *reader;
  // Padding and borders, which the drawing doesn't fill.
  let frame = {
    let padding = units.edges(style, css::properties::PADDING, containing[0]);
    let borders = css::properties::BORDERS.map(|[width, kind, _]| {
      style
        .get(kind)
        .filter(|kind| !matches!(*kind, "none" | "hidden"))
        .and_then(|_| units.resolve(style.get(width).unwrap_or("3px"), containing[0]))
        .unwrap_or(0.)
        .max(0.)
    });
    [
      padding[1] + padding[3] + borders[1] + borders[3],
      padding[0] + padding[2] + borders[0] + borders[2],
    ]
  };
  let border_box = style.get(Prop::BOX_SIZING) == Some("border-box");
  let [width, height] = [(Prop::WIDTH, "width"), (Prop::HEIGHT, "height")]
    .into_iter()
    .enumerate()
    .map(|(axis, (prop, attr))| {
      match style.get(prop) {
        Some(value) => units.resolve(value, containing[axis]),
        None => {
          (node.attrs.get(attr).and_then(|v| dimension(v, false))).map(|v| v * units.font.zoom)
        }
      }
      .map(|size| if border_box { size - frame[axis] } else { size })
    })
    .collect::<Vec<_>>()[..]
  else {
    unreachable!()
  };
  let view_box = node
    .attrs
    .get("viewBox")
    .map(|value| {
      value
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter_map(|part| part.parse::<f32>().ok())
        .collect::<Vec<_>>()
    })
    .filter(|parts| parts.len() == 4 && parts[2] > 0. && parts[3] > 0.);
  let ratio = view_box.as_ref().map(|parts| parts[2] / parts[3]);
  // Like browsers: an SVG with a ratio and no size fills the available width, and one with neither is
  // 300 by 150.
  let available = || {
    let margins = units.edges(style, css::properties::MARGIN, containing[0]);
    let insets = if matches!(position, "absolute" | "fixed") {
      [Prop::LEFT, Prop::RIGHT].map(|prop| {
        style
          .get(prop)
          .and_then(|value| units.resolve(value, containing[0]))
          .unwrap_or(0.)
      })
    } else {
      [0.; 2]
    };
    containing[0] - margins[1] - margins[3] - insets[0] - insets[1]
  };
  let [width, height] = match (width, height, ratio) {
    (None, Some(height), Some(ratio)) => [height * ratio, height],
    (Some(width), None, Some(ratio)) => [width, width / ratio],
    // A stretched flex item in a row of definite height takes that height.
    (None, None, Some(ratio))
      if flex_parent
        && reader
          .parent(id, Prop::FLEX_DIRECTION)
          .is_none_or(|value| value.starts_with("row"))
        && reader
          .parent(id, Prop::HEIGHT)
          .is_some_and(|value| value.ends_with("px"))
        && style
          .get(Prop::ALIGN_SELF)
          .filter(|value| *value != "auto")
          .or_else(|| reader.parent(id, Prop::ALIGN_ITEMS))
          .is_none_or(|value| matches!(value, "normal" | "stretch"))
        && containing[1] > 0. =>
    {
      let margins = units.edges(style, css::properties::MARGIN, containing[0]);
      let height = containing[1] - margins[0] - margins[2] - frame[1];
      [height * ratio, height]
    }
    (None, None, Some(ratio)) => {
      let width = available() - frame[0];
      [width, width / ratio]
    }
    _ => [
      width.unwrap_or(300. * units.font.zoom),
      height.unwrap_or(150. * units.font.zoom),
    ],
  }
  .map(|value| value.clamp(1., 4096.));
  // The content is in CSS pixels, and the raster scales it to the zoomed box.
  let [inner_width, inner_height] = [width, height].map(|side| side / units.font.zoom);
  let [width, height] = [width + frame[0], height + frame[1]];
  let mut scope = id;
  while let Some(parent) = doc.nodes[scope].parent {
    scope = parent;
  }
  let mut pending = vec![id];
  let mut visited = rustc_hash::FxHashSet::default();
  let mut linked = false;
  let mut targets = Vec::new();
  let mut painted = Vec::new();
  let mut hasher = rustc_hash::FxHasher::default();
  while let Some(child) = pending.pop() {
    if !visited.insert(child) {
      continue;
    }
    std::hash::Hash::hash(&(child, doc.nodes.revision(child)), &mut hasher);
    painted.push(styles.values[child].clone());
    let child = &doc.nodes[child];
    pending.extend(&child.children);
    linked |= child.tag == "use";
    if child.tag == "use"
      && let Some(reference) = href(child)
      && let Some(target) = crate::render::effects::index(ids, doc)
        .get(reference)
        .and_then(|targets| {
          targets.iter().copied().find(|&target| {
            let mut root = target;
            while let Some(parent) = doc.nodes[root].parent {
              if parent == id {
                return false;
              }
              root = parent;
            }
            root == scope
          })
        })
      && !visited.contains(&target)
    {
      targets.push(target);
      pending.push(target);
    }
  }
  // `uses` finds a link only through a `use` element of the subtree just walked.
  let used = if linked {
    uses(doc, styles, ids, id)
  } else {
    None
  };
  if used.is_some() {
    reader.log(Query::Uses(id as u32), 1, hashed(used));
  }
  let key = (
    [inner_width, inner_height],
    std::hash::Hasher::finish(&hasher),
  );
  let props = || style::SVG.iter().chain(&SVG_SOURCE);
  let kept = view.svgs.borrow().get(&id).is_some_and(|(old, at)| {
    *at == key
      && old.len() == painted.len()
      && old
        .iter()
        .zip(&painted)
        .enumerate()
        .all(|(index, (old, new))| {
          Rc::ptr_eq(old, new)
            || props()
              .all(|&prop| index == 0 && !prop.inherited() || old.get(prop) == new.get(prop))
        })
  }) && view
    .store
    .borrow()
    .page_state(view.id)
    .is_some_and(|state| state.media.svg_sources.contains_key(&id));
  if !kept {
    let mut header = format!(
      "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{inner_width}\" height=\"{inner_height}\""
    );
    for (key, value) in &node.attrs {
      if !matches!(
        key.as_str(),
        "width" | "height" | "xmlns" | "xmlns:xlink" | "style"
      ) && style::presentation(key).is_none()
      {
        header.push_str(&format!(" {key}=\"{}\"", crate::dom::escape(value)));
      }
    }
    header.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
    let svg = SvgSource {
      doc,
      styles,
      root: id,
      scale: view_box.as_ref().map_or(1., |parts| {
        (inner_width / parts[2]).min(inner_height / parts[3])
      }),
      view: view_box
        .as_ref()
        .map_or([inner_width, inner_height], |parts| [parts[2], parts[3]]),
      header,
      ids,
    };
    let attrs = |element| svg.attrs(element, true, true);
    let mut source = svg.header.clone();
    for (name, value) in attrs(id) {
      source.push_str(&format!(" {name}=\"{}\"", crate::dom::escape(&value)));
    }
    source.push('>');
    source.push_str("<defs>");
    for &target in &targets {
      source.push_str(&doc.serialize_svg(target, false, &attrs));
    }
    source.push_str("</defs>");
    for child in &node.children {
      source.push_str(&doc.serialize_svg(*child, true, &attrs));
    }
    source.push_str("</svg>");
    let mut names = Vec::new();
    let mut counted = String::with_capacity(source.len());
    let mut rest = &*source;
    const MARK: &str = " id=\"apps-";
    while let Some(start) = rest.find(MARK) {
      let (before, after) = rest.split_at(start + MARK.len());
      counted.push_str(before);
      rest = after;
      if let Some(end) = after.find('"')
        && let Ok(marked) = after[..end].parse()
      {
        counted.push_str(&names.len().to_string());
        names.push((String::new(), marked));
        rest = &after[end..];
      }
    }
    counted.push_str(rest);
    let source = counted;
    let mut pending = node.children.clone();
    while let Some(child) = pending.pop() {
      if let Some(name) = doc.nodes[child].attrs.get("id") {
        names.push((name.clone(), child));
      }
      pending.extend(&doc.nodes[child].children);
    }
    // CSS keywords ignore case, but resvg only reads `currentColor`.
    let lower = source.to_ascii_lowercase();
    let mut end = 0;
    let mut replaced = String::with_capacity(source.len());
    for (start, _) in lower.match_indices("currentcolor") {
      replaced.push_str(&source[end..start]);
      replaced.push_str("currentColor");
      end = start + "currentcolor".len();
    }
    replaced.push_str(&source[end..]);
    let source: Rc<str> = replaced.into();
    view.svgs.borrow_mut().insert(id, (painted, key));
    let mut store = view.store.borrow_mut();
    let state = store.state(view.id);
    // Discard the raster on source changes: paint checks only inline SVG dimensions.
    if state
      .rasters
      .svgs
      .get(&id)
      .is_some_and(|(old, _)| *old != source)
      && let Some((_, image)) = state.rasters.svgs.remove(&id)
      && Arc::strong_count(&image) == 1
    {
      state.media.retired.push(image);
    }
    state.rasters.svg_boxes.insert(
      id,
      (
        "".into(),
        names,
        [0.; 2],
        Vec::new(),
        [0.; 4],
        [0.; 4],
        [0.; 4],
      ),
    );
    state.media.svg_sources.insert(id, source);
  }
  el = el.w(px(width)).h(px(height));
  // As in Chrome, an `<svg>` with `overflow: visible` paints outside its box.
  let spill = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
    .into_iter()
    .all(|prop| style.get(prop).is_none_or(|value| value == "visible"));
  content_layers.push(svg_layer(
    id,
    true,
    spill,
    view.clone(),
    units.edges(style, css::properties::PADDING, containing[0]),
  ));
  el
}
