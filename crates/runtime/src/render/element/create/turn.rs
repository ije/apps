use super::super::kept::{Query, hashed};
use super::super::margin::definite_height;
use super::cx::{Cx, Turn};
use crate::css::properties::Prop;
use crate::render::layout::grid::intrinsic::frame_size;
use crate::render::layout::size::{layout_ceil, turned};
use renderer::{Corners, Length, Radii, point};

/// The turned layout of a box whose writing mode is orthogonal to the one of its parent.
pub(super) fn turn(
  cx: &Cx<'_>,
  margin: [f32; 4],
  padding: [f32; 4],
  borders: [f32; 4],
) -> Option<Turn> {
  cx.style.get(Prop::WRITING_MODE)?;
  let build = cx.reader;
  cx.reader.parent(cx.id, Prop::WRITING_MODE);
  let clockwise = turned(build.doc, build.styles, cx.id).filter(|_| cx.id != 0)?;
  let (style, units, containing) = (cx.style, cx.units, cx.containing);
  let sides = match style.get(Prop::BOX_SIZING) {
    Some("border-box") => [0.; 2],
    _ => [[3, 1], [0, 2]]
      .map(|[start, end]| padding[start] + padding[end] + borders[start] + borders[end]),
  };
  let definite = definite_height(build.doc, build.styles, cx.parent);
  build.log(Query::Definite(cx.id as u32), 0, hashed(definite));
  let length = |prop, axis: usize| {
    style
      .get(prop)
      .filter(|value| axis == 0 || definite || !value.contains('%'))
      .and_then(|value| units.resolve(value, containing[axis]))
      .map(|value| value.max(0.) + sides[axis])
  };
  let clamp = |value: f32, [min, max]: [Prop; 2], axis| {
    value
      .min(length(max, axis).unwrap_or(f32::INFINITY))
      .max(length(min, axis).unwrap_or(0.))
  };
  let area = cx
    .slot
    .as_ref()
    .map(|([_, _, width, height], _)| [*width, *height]);
  let outer = [margin[1] + margin[3], margin[0] + margin[2]];
  let stretched = |axis: usize| match area {
    Some(area) => Some(area[axis] - outer[axis]),
    None => (cx.stretches(axis) && (axis == 0 || definite)).then(|| containing[axis] - outer[axis]),
  };
  let frame = |inline| {
    frame_size(
      build.doc,
      cx.id,
      build.styles,
      inline,
      build.text,
      &mut build.measured.borrow_mut(),
    )
  };
  let room = match definite {
    true => containing[1] - outer[1],
    false => units.viewport[1],
  };
  let inline = length(Prop::HEIGHT, 1)
    .or_else(|| stretched(1))
    .unwrap_or_else(|| frame(None)[0].min(room));
  let inline = clamp(inline, [Prop::MIN_HEIGHT, Prop::MAX_HEIGHT], 1);
  let block = length(Prop::WIDTH, 0)
    .or_else(|| stretched(0))
    .unwrap_or_else(|| frame(Some(inline))[1]);
  let block = clamp(block, [Prop::MIN_WIDTH, Prop::MAX_WIDTH], 0);
  let ceil = |size| layout_ceil(size, build.pixel_ratio);
  Some(Turn {
    clockwise,
    inline: ceil(inline),
    block: ceil(block),
  })
}

/// Gives the box of a turned layout the sizes, corners and shadows of its frame.
pub(super) fn turn_style(style: &mut renderer::Style, turn: &Turn) {
  style.size.width = renderer::px(turn.inline).into();
  style.size.height = renderer::px(turn.block).into();
  style.min_size = Default::default();
  style.max_size = Default::default();
  style.aspect_ratio = None;
  style.corner_radii = turn_corners(std::mem::take(&mut style.corner_radii), turn.clockwise);
  for shadow in &mut style.box_shadow {
    let [x, y] = [shadow.offset.x, shadow.offset.y];
    shadow.offset = match turn.clockwise {
      true => point(y, -x),
      false => point(-y, x),
    };
  }
}

fn corners<T>(
  Corners {
    top_left,
    top_right,
    bottom_right,
    bottom_left,
  }: Corners<T>,
  clockwise: bool,
) -> Corners<T> {
  match clockwise {
    true => Corners {
      top_left: top_right,
      top_right: bottom_right,
      bottom_right: bottom_left,
      bottom_left: top_left,
    },
    false => Corners {
      top_left: bottom_left,
      top_right: top_left,
      bottom_right: top_right,
      bottom_left: bottom_right,
    },
  }
}

pub(super) fn turn_corners(radii: Radii<Length>, clockwise: bool) -> Radii<Length> {
  Radii {
    x: corners(radii.y, clockwise),
    y: corners(radii.x, clockwise),
    shape: corners(radii.shape, clockwise),
  }
}
