use super::PageState;
use crate::css;
use crate::css::properties::Prop;
use crate::dom::Document;
use crate::dom::{LEAVING, TOP_LAYER};
use crate::style;

// Whether a convex shape holds a point.
fn in_shape(
  shape: &[renderer::Point<renderer::Pixels>],
  at: renderer::Point<renderer::Pixels>,
) -> bool {
  // Twice the area of the triangle of a side and a point, with the sign of the whole shape.
  let cross = |a: usize, b: usize, c: renderer::Point<renderer::Pixels>| {
    let (ab, ac) = (shape[b] - shape[a], c - shape[a]);
    f64::from(ab.x.0) * f64::from(ac.y.0) - f64::from(ab.y.0) * f64::from(ac.x.0)
  };
  let turn: f64 = (2..shape.len())
    .map(|last| cross(0, last - 1, shape[last]))
    .sum();
  turn.abs() > 1e-3
    && (0..shape.len()).all(|side| cross(side, (side + 1) % shape.len(), at) * turn.signum() >= 0.)
}

// An ancestor, the clip in it, the blocks it makes once read, whether the point is in its border, and its matrix
// with 1 when that is 2D.
type Around = (
  usize,
  [f32; 4],
  Option<crate::render::effects::Establishes>,
  bool,
  Option<[f32; 5]>,
);

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Hit {
  // All the elements at the point, the topmost first.
  All,
  // The topmost element, like `elementFromPoint()`.
  First,
  // The text node nearest to the point, else the topmost element, like `caretPositionFromPoint()`.
  Caret,
}

// Where a node is with respect to the drawable content of a canvas.
#[derive(Clone, Copy, PartialEq)]
enum Drawn {
  Page,
  // In the drawable content of a canvas, which takes no pointer.
  Hidden,
  // In an element that the canvas drew.
  Shown,
}

// A node to visit, or a node that the point hit.
struct Pending {
  id: usize,
  clip: [f32; 4],
  hit: bool,
  depth: usize,
  drawn: Drawn,
  ring: bool,
}

// `First` and `Caret` give one node at most.
pub(crate) fn hit_test(
  doc: &Document,
  state: &PageState,
  computed: &style::ComputedStyles,
  viewport: [f32; 2],
  [x, y]: [f32; 2],
  mode: Hit,
) -> Vec<usize> {
  let mut nearest = Vec::new();
  // Like Chrome, a box at the height of the point comes first, then the boxes below it, then the boxes above it.
  // Then the vertical and the horizontal distance of the point to the box.
  let gap = |[left, top, width, height]: [f32; 4]| {
    let rank = if y < top {
      1.
    } else if y > top + height {
      2.
    } else {
      0.
    };
    [
      rank,
      (top - y).max(y - top - height).max(0.),
      (left - x).max(x - left - width).max(0.),
    ]
  };
  let order = |a: &[f32; 3], b: &[f32; 3]| {
    (a[0].total_cmp(&b[0]))
      .then(a[1].total_cmp(&b[1]))
      .then(a[2].total_cmp(&b[2]))
  };
  let screen = [0., 0., viewport[0], viewport[1]];
  let up = |id: usize| {
    doc.nodes[id]
      .parent
      .or_else(|| state.tree.hosts.get(&id).copied())
  };
  let top: Vec<usize> = state
    .interaction
    .top_layer
    .iter()
    .copied()
    .filter(|&id| {
      doc.nodes[id].state & (TOP_LAYER | LEAVING) != 0
        && doc.nodes[id].tag != "html"
        && std::iter::successors(Some(id), |&id| up(id)).all(|id| {
          computed
            .values
            .get(id)
            .is_none_or(|style| style.get(Prop::DISPLAY) != Some("none"))
        })
        && std::iter::successors(Some(id), |&id| up(id)).last() == Some(0)
    })
    .collect();
  let mut pending: Vec<Pending> = std::iter::once(0)
    .chain(top.iter().copied())
    .map(|id| Pending {
      id,
      clip: screen,
      hit: false,
      depth: 0,
      drawn: Drawn::Page,
      ring: false,
    })
    .collect();
  let mut hits = Vec::new();
  // The ancestors of the current node with the clips inside them and, once read, the containing blocks they
  // make. The last field is the matrix from the box to the window, once read.
  let mut path: Vec<Around> = Vec::new();
  let inside = |clip: [f32; 4]| x >= clip[0] && x < clip[2] && y >= clip[1] && y < clip[3];
  while let Some(Pending {
    id,
    mut clip,
    hit,
    depth,
    mut drawn,
    mut ring,
  }) = pending.pop()
  {
    if hit {
      hits.push(id);
      continue;
    }
    // The last frame marked each subtree with an out-of-flow box uncullable.
    if !inside(clip)
      && state
        .layout
        .cull
        .info
        .get(id)
        .is_some_and(|info| info.flags & crate::render::layout::cull::UNCULLABLE == 0)
    {
      continue;
    }
    if !state.layout.anchors.boxes.is_empty()
      && (state.layout.anchors.boxes.get(&id)).is_some_and(|found| found.hidden.get())
    {
      continue;
    }
    let node = &doc.nodes[id];
    let Some(style) = computed.values.get(id) else {
      continue;
    };
    if state.styles.inert(id)
      || style
        .get(Prop::DISPLAY)
        .is_some_and(|value| value == "none")
    {
      continue;
    }
    path.truncate(depth);
    if drawn != Drawn::Page {
      drawn = match state.geometry.drawn.get(&id) {
        Some(place) => {
          // The clip is in the content box of the canvas.
          let canvas = (path.iter().rev()).find(|(id, ..)| doc.nodes[*id].tag == "canvas");
          if let (Some([x, y, width, height]), Some(&(canvas, ..))) = (place.clip, canvas)
            && let Some(&[left, top, wide, _]) = state.geometry.mirror.get(&canvas)
          {
            let (style, units) = (&computed.values[canvas], &computed.lengths[canvas]);
            let (border, padding) = (
              units.borders(style, wide),
              units.edges(style, css::properties::PADDING, wide),
            );
            let [x, y] = [
              x + left + border[3] + padding[3],
              y + top + border[0] + padding[0],
            ];
            clip = [
              clip[0].max(x),
              clip[1].max(y),
              clip[2].min(x + width),
              clip[3].min(y + height),
            ];
          }
          Drawn::Shown
        }
        None if node.attrs.contains_key("drawable") => Drawn::Hidden,
        None => drawn,
      };
    }
    // An out-of-flow box gets only the clip of its containing block, as the renderer draws it. That clip
    // holds the clip of each box between, so a box without a clip keeps it.
    if (clip != screen || ring)
      && let Some(escape) = style
        .get(Prop::POSITION)
        .filter(|&value| matches!(value, "absolute" | "fixed"))
    {
      (clip, ring) = path
        .iter_mut()
        .rev()
        .find_map(|(id, clip, made, ring, _)| {
          let made =
            made.get_or_insert_with(|| crate::render::effects::establishes(&computed.values[*id]));
          (if escape == "fixed" {
            made.fixed
          } else {
            made.absolute
          })
          .then_some((*clip, *ring))
        })
        .unwrap_or((screen, false));
    }
    // In the border of such a box, its boxes with layers of their own miss the pointer.
    if ring && layered(doc, computed, id, style, up(id)) {
      clip = [f32::INFINITY; 4];
    }
    let canvas = style::drawable_canvas(node, style);
    if inside(clip)
      && let Some(&[left, top, width, height]) = state.geometry.mirror.get(&id)
    {
      let size = renderer::size(renderer::px(width), renderer::px(height));
      let rect = renderer::Rect::new(renderer::point(renderer::px(left), renderer::px(top)), size);
      let at = renderer::point(renderer::px(x), renderer::px(y));
      let contained = style.get(Prop::CONTAIN).is_some_and(|value| {
        ["paint", "strict", "content"]
          .iter()
          .any(|kind| value.contains(kind))
      }) && !matches!(
        style.get(Prop::DISPLAY).unwrap_or("inline"),
        "inline" | "contents"
      ) || style
        .get(Prop::CONTENT_VISIBILITY)
        .is_some_and(|value| value != "visible")
        && (state.layout.cull.info.get(id))
          .is_some_and(|info| info.flags & crate::render::layout::cull::SKIPS != 0);
      let axes = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y].map(|prop| {
        canvas
          || contained
          || drawn != Drawn::Hidden
            && style
              .get(prop)
              .is_some_and(|value| matches!(value, "hidden" | "clip" | "auto" | "scroll"))
      });
      let clips = canvas
        || drawn != Drawn::Hidden
          && axes.contains(&true)
          && !super::viewport_overflow(doc, computed, id);
      let out = x < left || x >= left + width || y < top || y >= top + height;
      let margin = clips && style.contains(Prop::OVERFLOW_CLIP_MARGIN);
      let rounded = !node.tag.starts_with('#')
        && (clips || !out)
        && crate::render::effects::clip::RADII
          .iter()
          .any(|&prop| style.contains(prop));
      // A point outside the bounds is outside a clip on both axes.
      let far = clips && out && axes == [true; 2] && !margin;
      path.push((id, clip, None, ring, None));
      // The size of a transformed box and the point in it.
      let local = (!far
        && (clips || rounded)
        && state.geometry.mirror.is_transformed(id)
        && !node.tag.starts_with('#'))
      .then(|| local_point(&mut path, computed, state, [x, y]))
      .flatten();
      path.pop();
      let point = at;
      let (rect, at) = match local {
        Some((size, [x, y])) => (
          renderer::Rect::new(
            Default::default(),
            renderer::size(renderer::px(size[0]), renderer::px(size[1])),
          ),
          renderer::point(renderer::px(x), renderer::px(y)),
        ),
        None => (rect, at),
      };
      let corners = match rounded && !far {
        false => renderer::Radii::default(),
        true => crate::render::effects::clip::radii(style, &computed.lengths[id], || {
          [rect.size.width, rect.size.height].map(f32::from)
        })
        .resolve(rect.size),
      };
      let selectable = !style
        .get(Prop::POINTER_EVENTS)
        .is_some_and(|value| value == "none")
        && !style
          .get(Prop::VISIBILITY)
          .is_some_and(|value| matches!(value, "hidden" | "collapse"));
      if mode == Hit::Caret && node.tag == "#text" && selectable && height > 0. {
        nearest.push((id, gap([left, top, width, height])));
      }
      let within = |[left, top, width, height]: [f32; 4]| {
        width > 0. && height > 0. && x >= left && x < left + width && y >= top && y < top + height
      };
      // A node without a shape of its own is hidden with the box around it.
      let shapes = &state.geometry.shapes;
      let shape_of = |id: usize| {
        (shapes.get(&id)).filter(|_| computed.values[id].get(Prop::DISPLAY) != Some("contents"))
      };
      let shape = shape_of(id).filter(|_| !shapes.is_empty());
      let shown = shapes.is_empty()
        || shape.is_some()
        || (path.iter().rev())
          .find_map(|(id, ..)| shape_of(*id))
          .is_none_or(|shape| !shape.is_empty());
      if shown
                && (mode == Hit::Caret || !node.tag.starts_with('#'))
                && match state.geometry.fragments.is_empty() {
                    _ if shape.is_some() => shape.is_some_and(|shape| in_shape(shape, point)),
                    true => within([left, top, width, height]),
                    false => state
                        .geometry.fragments
                        .get(&id)
                        .map_or(within([left, top, width, height]), |parts| {
                            parts.iter().copied().any(within)
                        }),
                }
                // Like Chrome, a point outside the rounded corners misses the box and what it clips.
                && (node.tag.starts_with('#')
                    || shape.is_some_and(|shape| shape.len() != 4)
                    || corners.contains(rect, at))
                && selectable
                && drawn != Drawn::Hidden
      {
        pending.push(Pending {
          id,
          clip,
          hit: true,
          depth,
          drawn,
          ring: false,
        });
      }
      // A canvas shows its drawable content only inside its box.
      if canvas {
        drawn = Drawn::Hidden;
      }
      let nowhere = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
      ];
      if far {
        clip = nowhere;
      }
      let (size, [x, y]) = local.unwrap_or(([width, height], [x - left, y - top]));
      // Most points are farther from the edges than all of the borders and paddings.
      let deep = || {
        state.layout.content_size(id).is_some_and(|inner| {
          let room = [size[0] - inner[0], size[1] - inner[1]];
          (room[0]..=size[0] - room[0]).contains(&x) && (room[1]..=size[1] - room[1]).contains(&y)
        })
      };
      if clips && !far && (rounded || margin || !deep()) {
        let units = &computed.lengths[id];
        let borders = units.borders(style, size[0]);
        let mut edges = borders;
        let clipped = margin
          && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y].iter().all(|&prop| {
            matches!(style.get(prop), Some("clip"))
              || contained && matches!(style.get(prop), None | Some("visible"))
          });
        if let Some(value) = style.get(Prop::OVERFLOW_CLIP_MARGIN).filter(|_| clipped)
          && let Some(renderer::ClipMargin { sides, reach }) =
            crate::render::effects::clip::margin(value, style, units, size[0])
        {
          let sides = [sides.top, sides.right, sides.bottom, sides.left];
          edges = std::array::from_fn(|side| borders[side] - f32::from(sides[side] + reach));
        }
        // Like Chrome, a box with a layer of its own lets the pointer in to its border box, and corners turn
        // the clip margin off.
        let past = |edges: [f32; 4], axes: [bool; 2]| {
          axes[0] && !(edges[3]..size[0] - edges[1]).contains(&x)
            || axes[1] && !(edges[0]..size[1] - edges[2]).contains(&y)
        };
        let shut = if !corners.is_zero() {
          let edges: [f32; 4] = std::array::from_fn(|side| edges[side].max(borders[side]));
          let at = renderer::point(renderer::px(x), renderer::px(y));
          let inner = renderer::Rect::between(
            renderer::point(renderer::px(edges[3]), renderer::px(edges[0])),
            renderer::point(
              renderer::px(size[0] - edges[1]),
              renderer::px(size[1] - edges[2]),
            ),
          );
          let [top, right, bottom, left] = edges.map(renderer::px);
          past(edges, [true; 2]) || !corners.inset(top, right, bottom, left).contains(inner, at)
        } else {
          past(edges, axes) && {
            ring = layered(doc, computed, id, style, up(id));
            !ring || past(edges.map(|side| side.min(0.)), axes)
          }
        };
        if shut {
          clip = nowhere;
        }
      }
    }
    // The fallback content of a canvas has no boxes, and skipped contents take no pointer.
    if node.tag == "canvas" && !canvas
      || crate::render::layout::cull::skipping(doc, &state.layout, computed, id, false)
    {
      continue;
    }
    path.push((id, clip, None, ring, None));
    pending.extend(
      computed
        .flat_children(doc, id)
        .iter()
        .filter(|child| !top.contains(child))
        .map(|&id| Pending {
          id,
          clip,
          hit: false,
          depth: depth + 1,
          drawn,
          ring,
        }),
    );
  }
  hits.sort_by_cached_key(|&id| {
    let painted = |id: usize| {
      std::iter::successors(Some(id), |&id| up(id))
        .find_map(|id| {
          state
            .layout
            .painted
            .get(id)
            .copied()
            .filter(|&order| order > 0)
        })
        .unwrap_or(0)
    };
    // Elements that a canvas drew are above the canvas, the last one drawn first.
    let drawn = std::iter::successors(Some(id), |&id| doc.nodes[id].parent)
      .filter(|_| !state.geometry.drawn.is_empty())
      .find_map(|at| Some((at, state.geometry.drawn.get(&at)?.order)));
    std::cmp::Reverse(match drawn {
      Some((element, order)) => {
        let canvas = std::iter::successors(doc.nodes[element].parent, |&id| doc.nodes[id].parent)
          .find(|&id| doc.nodes[id].tag == "canvas");
        (canvas.map_or(0, painted), order, painted(id))
      }
      None => (painted(id), 0, 0),
    })
  });
  for &id in top.iter().rev() {
    if hits
      .first()
      .is_some_and(|&hit| std::iter::successors(Some(hit), |&hit| up(hit)).any(|hit| hit == id))
    {
      break;
    }
    if !state.styles.inert(id)
      && computed
        .extras(id)
        .backdrop
        .as_ref()
        .is_some_and(|backdrop| {
          backdrop.get(Prop::POINTER_EVENTS) != Some("none")
            && backdrop
              .get(Prop::VISIBILITY)
              .is_none_or(|value| value == "visible")
            && backdrop.get(Prop::DISPLAY) != Some("none")
        })
      && x >= 0.
      && y >= 0.
      && x < viewport[0]
      && y < viewport[1]
    {
      hits.insert(0, id);
      break;
    }
  }
  if mode == Hit::Caret {
    if let Some(&id) = hits.first()
      && doc.nodes[id].tag == "#text"
    {
      return vec![id];
    }
    let target = hits.first().copied();
    let editable = |id: usize| {
      std::iter::successors(Some(id), |&id| doc.nodes[id].parent)
        .find_map(|id| doc.nodes[id].content_editable())
        .unwrap_or(false)
    };
    let editing = target.is_some_and(editable);
    // Like Chrome: the line nearest to the point, then the nearest text on it. An editable target takes
    // only its editable text.
    let text = nearest
      .into_iter()
      .filter(|&(id, _)| {
        std::iter::successors(doc.nodes[id].parent, |&id| doc.nodes[id].parent)
          .any(|id| Some(id) == target)
          && (!editing || editable(id))
      })
      .map(|(id, key)| {
        // A text box that spans the point's height can hold more lines: measure each.
        let lines = (state.layout.text_layouts.get(&id))
          .filter(|_| key[0] == 0.)
          .map(|(layout, text, range, rows, font)| {
            crate::render::paint::text::text_rects(
              layout,
              text,
              rows,
              [range.start, range.end],
              *font,
            )
          });
        (
          id,
          lines
            .into_iter()
            .flatten()
            .map(gap)
            .min_by(order)
            .unwrap_or(key),
        )
      })
      .min_by(|a, b| order(&a.1, &b.1))
      .map(|(id, _)| id);
    return text.or(target).into_iter().collect();
  }
  if mode == Hit::First {
    hits.truncate(1);
    if hits.is_empty() && x >= 0. && y >= 0. && x < viewport[0] && y < viewport[1] {
      hits.extend((doc.nodes[0].children.iter()).find(|&&id| !doc.nodes[id].tag.starts_with('#')));
    }
  }
  hits
}

// Whether Chrome gives the box a paint layer of its own.
fn layered(
  doc: &Document,
  computed: &crate::style::ComputedStyles,
  id: usize,
  style: &css::properties::Style,
  parent: Option<usize>,
) -> bool {
  let made = crate::render::effects::establishes(style);
  made.stacking
    || made.absolute
    || matches!(
      doc.nodes[id].tag.as_str(),
      "iframe" | "svg" | "input" | "textarea" | "video" | "canvas" | "embed" | "object"
    )
    || style
      .get(Prop::Z_INDEX)
      .is_some_and(|value| value.parse::<i32>().is_ok())
      && parent
        .and_then(|parent| computed.values.get(parent)?.get(Prop::DISPLAY))
        .is_some_and(|display| display.ends_with("flex") || display.ends_with("grid"))
}

// The size of the last box of a path without its transforms, and the point in its space. The center of a box stays
// the center of its bounds.
fn local_point(
  path: &mut [Around],
  computed: &crate::style::ComputedStyles,
  state: &super::PageState,
  point: [f32; 2],
) -> Option<([f32; 2], [f32; 2])> {
  let size = |node: usize| {
    let (style, units) = (computed.values.get(node)?, computed.lengths.get(node)?);
    let inner = state.layout.content_size(node)?;
    let sides = crate::render::effects::clip::insets(Some("content-box"), style, units, inner);
    Some([
      inner[0] + sides[1] + sides[3],
      inner[1] + sides[0] + sides[2],
    ])
  };
  // Each box resolves its transform once in a hit test: the boxes in it take its matrix.
  let known = path.iter().rposition(|entry| entry.4.is_some());
  let mut m = known
    .and_then(|at| path[at].4)
    .unwrap_or([1., 0., 0., 1., 1.]);
  for entry in &mut path[known.map_or(0, |at| at + 1)..] {
    let node = entry.0;
    let style = (state.styles.animating.get(&node)).or(computed.values.get(node))?;
    if super::effects::TRANSFORM_PROPS
      .iter()
      .any(|&prop| style.get(prop).is_some_and(|value| value != "none"))
      && let Some(size) = size(node)
    {
      let [w, h] = size.map(renderer::px);
      let bounds = renderer::Rect::new(Default::default(), renderer::size(w, h));
      if let Some(t) =
        css::value::transform::transform(style, &computed.lengths[node], bounds, None)
      {
        let flat = [t[2], t[3], t[6], t[7], t[8], t[9], t[11]] == [0.; 7];
        m = [
          m[0] * t[0] + m[2] * t[1],
          m[1] * t[0] + m[3] * t[1],
          m[0] * t[4] + m[2] * t[5],
          m[1] * t[4] + m[3] * t[5],
          if flat { m[4] } else { 0. },
        ];
      }
    }
    entry.4 = Some(m);
  }
  let id = path.last()?.0;
  let &[left, top, width, height] = state.geometry.mirror.get(&id)?;
  let det = m[0] * m[3] - m[1] * m[2];
  // The bounds of a box give its size, unless its turn is near 45 degrees or in 3D.
  let [a, b, c, d, flat] = m.map(f32::abs);
  let share = a * d - b * c;
  let size = match flat != 0. && share.abs() > 0.2 {
    true => [
      (width * d - height * c) / share,
      (height * a - width * b) / share,
    ],
    false => size(id)?,
  };
  let [x, y] = [point[0] - left - width / 2., point[1] - top - height / 2.];
  (det.abs() > 1e-6).then(|| {
    (
      size,
      [
        size[0] / 2. + (x * m[3] - y * m[2]) / det,
        size[1] / 2. + (y * m[0] - x * m[1]) / det,
      ],
    )
  })
}
