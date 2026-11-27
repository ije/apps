use crate::css;
use crate::css::properties::Prop;
use renderer::{Child, Rect, point, px, size};

pub(super) type ColumnRule = (f32, renderer::Color, renderer::BorderStyle);

pub(super) fn columns(
  style: &css::properties::Style,
  units: &css::value::units::Lengths,
  width: f32,
) -> Option<(usize, f32, Option<ColumnRule>)> {
  let count = style
    .get(Prop::COLUMN_COUNT)
    .and_then(|value| value.parse::<usize>().ok());
  let column_width = style
    .get(Prop::COLUMN_WIDTH)
    .and_then(|value| units.resolve(value, width));
  if count.is_none() && column_width.is_none() {
    return None;
  }
  let gap = style
    .get(Prop::COLUMN_GAP)
    .filter(|value| *value != "normal")
    .and_then(|value| units.resolve(value, width))
    .unwrap_or(units.font.em)
    .max(0.);
  let fit = column_width
    .filter(|value| *value > 0.)
    .map(|value| (((width + gap) / (value + gap)).floor() as usize).max(1));
  let count = match (count, fit) {
    (Some(count), Some(fit)) => count.min(fit),
    (count, fit) => count.or(fit)?,
  }
  .clamp(1, 64);
  if count < 2 {
    return None;
  }
  let (rule_width, rule_style, rule_color) = (
    style.get(Prop::COLUMN_RULE_WIDTH),
    style.get(Prop::COLUMN_RULE_STYLE),
    style.get(Prop::COLUMN_RULE_COLOR),
  );
  let rule = rule_style
    .filter(|kind| !matches!(*kind, "none" | "hidden"))
    .map(|kind| {
      let width = rule_width
        .map_or(Some(3. * units.font.zoom), |value| {
          units.resolve(value, width)
        })
        .unwrap_or(0.);
      let color = rule_color
        .or_else(|| style.get(Prop::COLOR))
        .and_then(css::value::color::paint)
        .unwrap_or(renderer::Color::BLACK);
      (width, color, super::super::effects::border_style(kind))
    })
    .filter(|(width, ..)| *width > 0.);
  Some((count, gap, rule))
}

// A set of columns: its first child holds the flow of content, as wide as one column, which the renderer breaks.
pub(super) fn column_set(
  style: &css::properties::Style,
  content: [f32; 2],
  count: usize,
  gap: f32,
  rule: Option<ColumnRule>,
  alone: bool,
  items: impl IntoIterator<Item = impl Into<Child>>,
) -> renderer::Node {
  // A height or max-height limits the columns, which then overflow to the right.
  let fixed = style.get(Prop::HEIGHT).is_some_and(|value| value != "auto");
  let limited = fixed
    || style
      .get(Prop::MAX_HEIGHT)
      .is_some_and(|value| value != "none");
  let mut set = renderer::node().flex().flex_col().w_full().flex_shrink_0();
  set.style().columns = Some(Box::new(renderer::Columns {
    count: count as u16,
    gap: px(gap),
    height: (limited && alone).then_some(px(content[1])),
    fill: fixed && style.get(Prop::COLUMN_FILL) == Some("auto"),
  }));
  let share = renderer::Length::Calc(
    1. / count as f32,
    px(-gap * (count - 1) as f32 / count as f32),
  );
  set = set.child(renderer::node().flex().flex_col().w(share).children(items));
  match rule {
    Some((width, color, kind)) => set.child(column_rules(count, gap, width, color, kind)),
    None => set,
  }
}

pub(super) fn column_rules(
  count: usize,
  gap: f32,
  width: f32,
  color: renderer::Color,
  kind: renderer::BorderStyle,
) -> Child {
  renderer::canvas(
    |_, _| (),
    move |bounds, _, window| {
      let column = (f32::from(bounds.size.width) - gap * (count - 1) as f32) / count as f32;
      for index in 1..count {
        let x = bounds.left() + px(index as f32 * (column + gap) - gap / 2. - width / 2.);
        window.draw_box(
          Rect::new(point(x, bounds.top()), size(px(width), bounds.size.height)),
          renderer::Corners::default(),
          renderer::Color::TRANSPARENT,
          renderer::Border {
            widths: renderer::Edges {
              left: px(width),
              ..Default::default()
            },
            color,
            style: kind,
            sides: 0,
          },
        );
      }
    },
  )
  .absolute()
  .top_0()
  .left_0()
  .size_full()
  .into()
}
