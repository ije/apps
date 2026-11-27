use super::super::measure::Measurements;
use super::intrinsic_size;
use super::layout::{frame, layout};
use crate::css;
use crate::css::properties::Prop;
use crate::{
  dom::Document,
  style::{self, ComputedStyles},
};

// The renderer ceils each measured leaf to layout units, so sums of leaves must use the same widths.
pub(super) fn leaf_ceil(width: f32, pixel_ratio: f32) -> f32 {
  match pixel_ratio > 0. {
    true => super::super::size::layout_ceil(width, pixel_ratio),
    false => width,
  }
}

/// The content size that a keyword or `calc-size()` gives, from the min-content and max-content sizes, the space of
/// `stretch` and the size of `auto`. `sides` are the padding and borders that `box-sizing: border-box` counts.
pub(crate) fn keyword_content(
  value: &str,
  units: &css::value::units::Lengths,
  [min, max]: [f32; 2],
  space: Option<f32>,
  auto: f32,
  base: f32,
  sides: f32,
) -> Option<f32> {
  Some(
    match value {
      "min-content" => min,
      "max-content" | "content" => max,
      "fit-content" => space.map_or(auto, |space| max.min(min.max(space))),
      "stretch" | "-webkit-fill-available" => space?,
      "auto" => auto,
      "any" => 0.,
      _ => match css::value::calc_size(value) {
        Some((basis, size)) => {
          let basis = keyword_content(basis, units, [min, max], space, auto, base, sides)?;
          let basis = format!("({}px)", basis + sides);
          let size = css::value::with_size(size, &basis);
          units.resolve(&format!("calc({size})"), base)? - sides
        }
        None => units.resolve(value, base)? - sides,
      },
    }
    .max(0.),
  )
}

// Sets the sizes, minimums and maximums of an axis that are keywords. The automatic minimum is 0.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn keyword_sizes(
  style: &css::properties::Style,
  units: &css::value::units::Lengths,
  props: [Prop; 3],
  sizes: &mut [Option<f32>; 3],
  ends: [f32; 2],
  space: Option<f32>,
  auto: f32,
  base: f32,
  sides: f32,
) {
  for (slot, prop) in sizes.iter_mut().zip(props) {
    if let Some(value) = style.get(prop).filter(|value| keyword(value)) {
      let least = prop == props[1] && value != "fit-content";
      let auto = if least { 0. } else { auto };
      *slot = keyword_content(value, units, ends, space, auto, base, sides);
    }
  }
}

fn keyword(value: &str) -> bool {
  matches!(
    value,
    "min-content" | "max-content" | "fit-content" | "stretch" | "-webkit-fill-available"
  ) || value.starts_with("calc-size(")
}

// The place of the content sizes of a box that has keyword sizes in `Measurements::sizes`.
const RAW: Option<u32> = Some(u32::MAX);
// The place of the column extent of a grid that laid out no rows.
const COLUMNS: Option<u32> = Some(u32::MAX - 2);

/// The border box size that a keyword or `calc-size()` value of a size property gives. `space` is the size of the
/// containing block, and `fills` says that an `auto` width fills it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn keyword_size(
  doc: &Document,
  id: usize,
  styles: &ComputedStyles,
  prop: Prop,
  space: [f32; 2],
  fills: bool,
  text: &renderer::TextSystem,
  measured: &mut Measurements,
) -> Option<f32> {
  let (style, units) = (&styles.values[id], &styles.lengths[id]);
  let value = style.get(prop).filter(|value| keyword(value))?;
  let axis = usize::from(matches!(
    prop,
    Prop::HEIGHT | Prop::MIN_HEIGHT | Prop::MAX_HEIGHT
  ));
  let [margin, padding] = [css::properties::MARGIN, css::properties::PADDING]
    .map(|sides| units.edges(style, sides, space[0]));
  let border = units.borders(style, space[0]);
  let [start, end] = [[3, 1], [0, 2]][axis];
  let inner = padding[start] + padding[end] + border[start] + border[end];
  let outer = inner + margin[start] + margin[end];
  let width = (axis == 1).then_some(space[0]);
  // Keyword sizes change the result, so the content sizes are apart.
  intrinsic(doc, id, styles, width, None, text, measured);
  let key = match axis {
    0 => (id, RAW, None, true),
    _ => (id, width.map(f32::to_bits), RAW, true),
  };
  let [min, max, ..] = measured.size(key)?;
  let room = (space[axis] - outer).max(0.);
  let auto = match (axis, fills) {
    // The automatic minimum is 0, also for flex items, whose own one layout adds.
    _ if matches!(prop, Prop::MIN_WIDTH | Prop::MIN_HEIGHT) => 0.,
    (0, true) => room,
    (0, false) => max.min(min.max(room)),
    _ => max,
  };
  let sides = match style.get(Prop::BOX_SIZING) {
    Some("border-box") => inner,
    _ => 0.,
  };
  keyword_content(
    value,
    units,
    [min, max],
    Some(room),
    auto,
    space[axis],
    sides,
  )
  .map(|size| size + inner)
}

pub(crate) fn intrinsic(
  doc: &Document,
  id: usize,
  styles: &ComputedStyles,
  width: Option<f32>,
  height: Option<f32>,
  text: &renderer::TextSystem,
  measured: &mut Measurements,
) -> [f32; 4] {
  if let Some((parent, index)) = pseudo_item(id) {
    return pseudo_size(
      doc,
      parent,
      index,
      styles,
      width.unwrap_or(0.),
      width,
      text,
      measured.pixel_ratio,
    )
    .unwrap_or_default();
  }
  let key = (id, width.map(f32::to_bits), height.map(f32::to_bits));
  if let Some(value) = measured.size((key.0, key.1, key.2, false)) {
    return value;
  }
  if super::super::size::turned(doc, styles, id).is_some() {
    let units = &styles.lengths[id];
    let base = width.unwrap_or(0.);
    let margin = units.edges(&styles.values[id], css::properties::MARGIN, base);
    let [across, along] = [margin[1] + margin[3], margin[0] + margin[2]];
    let [widest, _] = frame_size(doc, id, styles, None, text, measured);
    let inline = widest.min(units.viewport[1]);
    let [_, block] = frame_size(doc, id, styles, Some(inline), text, measured);
    let result = [
      block + across,
      block + across,
      block + across,
      inline + along,
    ];
    measured.set_size((key.0, key.1, key.2, false), result);
    let own = |value: f32| [value.max(0.), value.min(0.)];
    measured.set_margins(key, [own(margin[0]), own(margin[2])]);
    return result;
  }
  measure(doc, id, styles, width, height, text, measured, key)
}

// Marks the measures of a turned box in the frame of its layout, apart from those on the page.
const FRAME: u32 = u32::MAX - 1;

/// The width of the lines of a turned box and its height across them, as border box sizes: the widest without an
/// `inline` size, and for that size.
pub(crate) fn frame_size(
  doc: &Document,
  id: usize,
  styles: &ComputedStyles,
  inline: Option<f32>,
  text: &renderer::TextSystem,
  measured: &mut Measurements,
) -> [f32; 2] {
  let key = (id, inline.map(f32::to_bits), Some(FRAME));
  let [_, widest, _, height] = match measured.size((key.0, key.1, key.2, false)) {
    Some(value) => value,
    None => measure(doc, id, styles, inline, None, text, measured, key),
  };
  [widest, height]
}

#[allow(clippy::too_many_arguments)]
fn measure(
  doc: &Document,
  id: usize,
  styles: &ComputedStyles,
  width: Option<f32>,
  height: Option<f32>,
  text: &renderer::TextSystem,
  measured: &mut Measurements,
  key: (usize, Option<u32>, Option<u32>),
) -> [f32; 4] {
  // Clear before this measure, so the lead that it records stays with its size.
  if measured.boxes.len() > 32768 {
    measured.boxes.clear();
  }
  let node = &doc.nodes[id];
  let style = &styles.values[id];
  let units = &styles.lengths[id];
  let display = style.get(Prop::DISPLAY).unwrap_or("");
  // Measuring recurses per DOM level, so deep pages must not overflow the stack.
  if display == "none"
    || std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
      .nth(256)
      .is_some()
  {
    return [0.; 4];
  }
  let base = width.unwrap_or(0.);
  let turn = match key.2 == Some(FRAME) {
    true => super::super::size::turned(doc, styles, id),
    false => None,
  };
  let edges = || {
    let edges = [
      units.edges(style, css::properties::PADDING, base),
      units.edges(style, css::properties::MARGIN, base),
      units.borders(style, base),
    ];
    match turn {
      // The margins of a turned box stay on the page.
      Some(clockwise) => {
        let [padding, _, border] = edges;
        let sides = |sides| super::super::size::turn_sides(sides, clockwise);
        [sides(padding), [0.; 4], sides(border)]
      }
      None => edges,
    }
  };
  let [padding, margin, border] = match &mut measured.edges {
    Some(memo) if turn.is_none() => *memo
      .entry((std::rc::Rc::as_ptr(style), base.to_bits()))
      .or_insert_with(edges),
    _ => edges(),
  };
  let inner = [
    padding[1] + padding[3] + border[1] + border[3],
    padding[0] + padding[2] + border[0] + border[2],
  ];
  let outer = [
    inner[0] + margin[1] + margin[3],
    inner[1] + margin[0] + margin[2],
  ];
  let border_box = style
    .get(Prop::BOX_SIZING)
    .is_some_and(|v| v == "border-box");
  let alt = super::super::size::broken_image(node)
    .then(|| node.attrs.get("alt").filter(|alt| !alt.is_empty()))
    .flatten();
  let sizeless = alt.is_some()
    && style.get(Prop::DISPLAY).is_none_or(|v| {
      v == "inline" || v == "inline-block" && styles.flags[id] & style::AGENT_DISPLAY != 0
    });
  let keywords = std::cell::Cell::new([false; 2]);
  let sizes = |props: [Prop; 3], base: Option<f32>| {
    let axis = usize::from((props[0] == Prop::HEIGHT) != turn.is_some());
    props.map(|prop| {
      let value = style
        .get(prop)
        .filter(|_| !(sizeless && matches!(prop, Prop::WIDTH | Prop::HEIGHT)))
        .filter(|v| !matches!(*v, "auto" | "none"))?;
      if keyword(value) {
        if turn.is_some() {
          return None;
        }
        let mut axes = keywords.get();
        axes[axis] = true;
        keywords.set(axes);
        return None;
      }
      Some(value)
        .filter(|v| base.is_some() || !v.contains('%'))
        .and_then(|v| units.resolve(v, base.unwrap_or(0.)))
        .map(|v| (v - if border_box { inner[axis] } else { 0. }).max(0.))
    })
  };
  let sides = |axis: usize| if border_box { inner[axis] } else { 0. };
  let clamp = |value: f32, [_, min, max]: [Option<f32>; 3]| {
    value
      .min(max.unwrap_or(f32::INFINITY))
      .max(min.unwrap_or(0.))
  };
  let mut horizontal = [Prop::WIDTH, Prop::MIN_WIDTH, Prop::MAX_WIDTH];
  let mut upright = [Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT];
  if turn.is_some() {
    std::mem::swap(&mut horizontal, &mut upright);
  }
  let mut fixed = sizes(horizontal, None);
  let mut used = sizes(horizontal, width);
  // Content-sized widths measure the content without a width first.
  if let Some(width) = width.filter(|_| keywords.get()[0]) {
    intrinsic(doc, id, styles, None, None, text, measured);
    let raw = measured.size((id, RAW, None, true));
    let [min, max, ..] = raw.unwrap_or_default();
    let room = (width - outer[0]).max(0.);
    let shrinks = display.starts_with("inline")
      || style.get(Prop::FLOAT).is_some_and(|value| value != "none")
      || (node.parent)
        .and_then(|parent| styles.values[parent].get(Prop::DISPLAY))
        .is_some_and(|display| display.ends_with("flex"));
    let auto = match shrinks {
      true => max.min(min.max(room)),
      false => room,
    };
    keyword_sizes(
      style,
      units,
      horizontal,
      &mut used,
      [min, max],
      Some(room),
      auto,
      width,
      sides(0),
    );
  }
  // A table cell is as wide as its columns.
  let cell = style::role(styles, id) == style::CELL;
  let available = used[0]
    .filter(|_| !cell || width.is_none())
    .or(width.map(|width| width - outer[0]))
    .or(fixed[0])
    .map(|width| clamp(width, used).max(0.));
  // As in layout, the bars and gutters that show whatever the content leave less room to the content.
  let full = available;
  let scrolls = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
    .into_iter()
    .any(|prop| {
      style
        .get(prop)
        .is_some_and(|v| !matches!(v, "visible" | "clip"))
    });
  let bars = match turn.is_none() && scrolls {
    true => {
      let extras = styles.extras(id);
      let handle = Default::default();
      let room = crate::render::paint::scrollbar::room(style, extras, units, &handle, true);
      room[1] + room[3]
    }
    false => 0.,
  };
  let available = available.map(|width| (width - bars).max(0.));
  let mut vertical = sizes(upright, height);
  let floated = |child: usize| {
    !matches!(display, "flex" | "inline-flex" | "grid" | "inline-grid")
      && styles.values[child]
        .get(Prop::FLOAT)
        .is_some_and(|v| v != "none")
      && styles.values[child]
        .get(Prop::POSITION)
        .is_none_or(|v| !matches!(v, "absolute" | "fixed"))
  };
  let mut content = [0f32; 3];
  // The pseudo-elements that are in the text of the element.
  let mut taken = [false; 2];
  let mut collapsed = None;
  let natural = (node.tag != "svg")
    .then(|| super::super::size::natural_size(doc, id, measured.naturals.get(&id).copied(), style))
    .flatten()
    .map(|(size, ratio)| (size.map(|side| side * units.font.zoom), ratio));
  let skipped = matches!(style.get(Prop::CONTENT_VISIBILITY), Some("hidden" | "auto"))
    && super::super::cull::skips(
      u32::from(super::super::cull::skippable(&node.tag, style)) * super::super::cull::SKIPS,
      style,
      id,
      measured,
      false,
    );
  if skipped {
    let remembered = measured.remembered.get(&id).and_then(|entry| entry.0);
    let [width, height] = intrinsic_size(style, units, remembered);
    content = [width, width, height];
  } else if let Some((natural, ratio)) = natural {
    let hint = [0, 1].map(|axis| {
      node
        .attrs
        .get(["width", "height"][axis])
        .filter(|_| {
          matches!(node.tag.as_str(), "img" | "video")
            && style.get([Prop::WIDTH, Prop::HEIGHT][axis]).is_none()
        })
        .and_then(|v| super::super::size::dimension(v, false))
        .map(|v| (v * units.font.zoom - if border_box { inner[axis] } else { 0. }).max(0.))
    });
    // Like layout, a video without a frame and with a CSS ratio has no natural size: it adds no width, and
    // the ratio gives its height from the width it gets.
    let fills = fills(doc, styles, id, ratio);
    let height = vertical[0].or(hint[1]);
    let width = hint[0]
      .or_else(|| ratio.zip(height).map(|(ratio, height)| height * ratio))
      .unwrap_or(natural[0]);
    let height = height.unwrap_or_else(|| {
      ratio.map_or(natural[1], |ratio| {
        clamp(used[0].unwrap_or(width), used) / ratio
      })
    });
    content = if fills {
      [0.; 3]
    } else {
      [width, width, height]
    };
  } else if node.tag == "svg" {
    let [width, height] = [0, 1].map(|axis| {
      [used[0], vertical[0]][axis].or_else(|| {
        node
          .attrs
          .get(["width", "height"][axis])
          .and_then(|v| super::super::size::dimension(v, false))
          .map(|v| v * units.font.zoom)
      })
    });
    let ratio = node.attrs.get("viewBox").and_then(|view| {
      let mut parts = view
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<f32>().ok())
        .skip(2);
      Some(parts.next()? / parts.next().filter(|height| *height > 0.)?)
    });
    let [width, height] = match (width, height, ratio) {
      (None, Some(height), Some(ratio)) => [height * ratio, height],
      (Some(width), None, Some(ratio)) => [width, width / ratio],
      // Like browsers, an SVG with a ratio and no size fills the available width, and adds nothing to the
      // content sizes.
      (None, None, Some(ratio)) => {
        let width = available.unwrap_or(0.);
        [width, width / ratio]
      }
      _ => [width.unwrap_or(24.), height.unwrap_or(24.)],
    };
    content = [width, width, height];
  } else if super::super::size::text_field(node, style) {
    let [width, height] = super::super::size::field_size(
      doc,
      id,
      style,
      units,
      text,
      measured.pixel_ratio,
      true,
      available,
    );
    content = [width, width, height];
  } else if node.tag == "input"
    && node.input_type().eq_ignore_ascii_case("file")
    && let Some(button) = styles
      .extras(id)
      .scrollbars
      .as_ref()
      .and_then(|parts| parts[1].as_ref())
  {
    let [width, height] =
      super::super::size::file_size(style, button, units, text, measured.pixel_ratio);
    content = [width, width, height];
  } else if node.tag == "input"
    && let Some(kind) = Some(node.input_type()).filter(|value| {
      ["button", "submit", "reset"]
        .iter()
        .any(|kind| value.eq_ignore_ascii_case(kind))
    })
  {
    let label = node.attrs.get("value").map_or(
      if kind.eq_ignore_ascii_case("submit") {
        "Submit"
      } else if kind.eq_ignore_ascii_case("reset") {
        "Reset"
      } else {
        ""
      },
      String::as_str,
    );
    let run = style::text_run(style, units, label.len());
    let width = f32::from(
      text
        .shape_line(label, renderer::px(units.font.em), &run)
        .line
        .width,
    );
    let ceiled = leaf_ceil(width, measured.pixel_ratio);
    content = [ceiled, ceiled, units.font.lh];
    if node.native_control(style).is_some() {
      let [padding, height] = super::super::size::native_size(renderer::ControlKind::Button);
      let ceiled = leaf_ceil(width + padding, measured.pixel_ratio);
      content = [ceiled, ceiled, height];
    }
  } else if let Some((source, mut runs, atoms, heights, mixed, pseudos, margin_room, inline)) =
    match alt {
      Some(alt) => Some((
        renderer::SharedString::from(alt.clone()),
        vec![style::text_run(style, units, alt.len())],
        Vec::new(),
        Vec::new(),
        false,
        [false; 2],
        0.,
        None,
      )),
      None => measured
        .texts
        .entry(id)
        .or_insert_with(|| {
          crate::render::inline::inline_text(doc, styles, id, &[], &[], [true; 2])
            .map(std::rc::Rc::new)
        })
        .clone()
        .map(|inline| {
          (
            inline.text.clone(),
            inline.runs.to_vec(),
            inline.atoms.clone(),
            inline.line_heights.clone(),
            inline.mixed(text),
            inline.pseudos,
            match inline.leads {
              true => crate::render::inline::margin_room(doc, styles, id),
              false => 0.,
            },
            Some(inline),
          )
        }),
    }
  {
    taken = pseudos;
    let mut boxes = Vec::with_capacity(atoms.len());
    for atom in &atoms {
      if atom.pseudo != 0 {
        let [width, height, baseline] = pseudo_atom(
          doc,
          id,
          usize::from(atom.pseudo - 1),
          styles,
          base,
          available,
          text,
          measured.pixel_ratio,
        )
        .unwrap_or_default();
        runs[atom.run].inset[1] = renderer::px(width);
        boxes.push((atom.at, [width, height, baseline], None));
        continue;
      }
      let [_, max, used, height] =
        intrinsic(doc, atom.node, styles, available, None, text, measured);
      // A percent width takes its share of the line, so the box can wrap to a line of its own.
      let percent = available.is_some()
        && styles.values[atom.node]
          .get(Prop::WIDTH)
          .is_some_and(|v| v.contains('%') || v == "stretch");
      runs[atom.run].inset[1] = renderer::px(if percent { used } else { max });
      // Like Chrome, a replaced box sits on the baseline, and other boxes on their last line.
      let baseline = if super::super::size::natural_size(
        doc,
        atom.node,
        measured.naturals.get(&atom.node).copied(),
        &styles.values[atom.node],
      )
      .is_some()
      {
        height
      } else if styles.values[atom.node]
        .get(Prop::DISPLAY)
        .is_some_and(|value| matches!(value, "inline-flex" | "inline-grid"))
      {
        // A flex or grid box sits on the baseline of its first item, or else on its bottom border edge.
        let (own, lengths) = (&styles.values[atom.node], &styles.lengths[atom.node]);
        let [above, _, below, _] =
          lengths.edges(own, css::properties::MARGIN, available.unwrap_or(0.));
        above
          + first_baseline(doc, atom.node, styles, available, text, measured)
            .unwrap_or(height - above - below)
      } else {
        let (own, lengths) = (&styles.values[atom.node], &styles.lengths[atom.node]);
        let [ascent, descent, _] = text
          .line_metrics(
            text.face(&style::font(own)),
            renderer::px(lengths.font.em * measured.pixel_ratio),
          )
          .map(|v| f32::from(v) / measured.pixel_ratio);
        let below = lengths.font.lh - ascent - ((lengths.font.lh - ascent - descent) / 2.);
        height - frame(own, lengths, available.unwrap_or(0.))[2] - below
      };
      boxes.push((atom.at, [max, height, baseline], None));
    }
    let wrap = !style
      .get(Prop::WHITE_SPACE)
      .is_some_and(|space| matches!(space, "pre" | "nowrap"));
    let word_break = style::word_break(style);
    let anywhere = matches!(
      word_break,
      renderer::WordBreak::Anywhere | renderer::WordBreak::BreakAll
    );
    let indent = crate::render::element::query::row_indents(style, units, available.unwrap_or(0.));
    let wrapping = renderer::Wrapping {
      width: available.filter(|_| wrap).map(renderer::px),
      max_lines: style
        .get(Prop::WEBKIT_LINE_CLAMP)
        .or_else(|| style.get(Prop::LINE_CLAMP))
        .and_then(|value| value.parse::<usize>().ok())
        .map(|lines| lines.max(1)),
      word_break,
      indent,
      ..Default::default()
    };
    let em = renderer::px(units.font.em);
    let shape = |runs: &[renderer::TextRun]| text.shape_text(&source, em, runs, &wrapping);
    let float = if alt.is_some() { 16. } else { 0. };
    let beside = alt
      .is_some()
      .then(|| {
        let mut runs = runs.clone();
        runs[0].inset[0] = renderer::px(float);
        shape(&runs)
      })
      .filter(|lines| {
        available
          .filter(|_| wrap)
          .zip(lines.first())
          .is_none_or(|(width, line)| f32::from(line.first_row_end()) <= width + 0.01)
      });
    let top = if beside.is_none() { float } else { 0. };
    // Text that does not wrap shapes the same for each width that it is measured at.
    let lines = match (beside, inline.filter(|_| atoms.is_empty())) {
      (Some(lines), _) => std::rc::Rc::new(lines),
      (None, Some(inline)) => match measured.lines.get(&id).filter(|entry| {
        std::rc::Rc::ptr_eq(&entry.0, &inline) && entry.1 == em && entry.2 == wrapping
      }) {
        Some(entry) => entry.3.clone(),
        None => {
          let lines = std::rc::Rc::new(shape(&runs));
          measured
            .lines
            .insert(id, (inline, em, wrapping, lines.clone()));
          lines
        }
      },
      (None, None) => std::rc::Rc::new(shape(&runs)),
    };
    let mut opening = Vec::new();
    let mut at = 0;
    for run in &*runs {
      if run.inset[0] > renderer::px(0.) {
        opening.push((at, f32::from(run.inset[0])));
      }
      at += run.len;
    }
    // Runs of other sizes or line heights make some rows taller than the strut.
    let mut lead = 0.;
    if mixed || !boxes.is_empty() {
      let strut = (
        text.face(&style::font(style)),
        renderer::px(units.font.em),
        renderer::px(units.font.lh),
      );
      for (row, [above, below]) in super::super::line::row_extents(
        lines.iter(),
        &runs,
        &heights,
        &boxes,
        strut,
        text,
        measured.pixel_ratio,
        margin_room,
      )
      .into_iter()
      .enumerate()
      {
        if row == 0 {
          lead = f32::from(above);
        }
        content[2] += f32::from(above + below);
      }
    }
    let mut line_start = 0;
    for (index, line) in lines.iter().enumerate() {
      let first = f32::from(indent.of(index)[0]);
      let width = f32::from(line.line.width);
      content[1] = content[1].max(width + first);
      content[2] += line.rows() as f32 * units.font.lh;
      let mut glyphs = line.line.glyphs.iter().peekable();
      let (mut start, mut previous, mut hyphen) = (-first, ' ', false);
      while let Some(glyph) = glyphs.next() {
        let character = line
          .text
          .get(glyph.index as usize..)
          .and_then(|v| v.chars().next())
          .unwrap_or(' ');
        if !character.is_whitespace() {
          let x = f32::from(glyph.x);
          // The same break chances as wrapping, also after a hyphen that follows a letter or digit.
          if anywhere
            || previous.is_whitespace()
            || character > '\u{2e7f}'
              && !(word_break == renderer::WordBreak::KeepAll
                && character.is_alphanumeric()
                && previous.is_alphanumeric())
            || hyphen && character.is_alphabetic()
          {
            start = x
              - if glyph.index == 0 { first } else { 0. }
              - opening
                .iter()
                .find(|(at, _)| *at == line_start + glyph.index as usize)
                .map_or(0., |(_, inset)| *inset);
          }
          if let Some(hyphen) = line.hyphen_before(glyph.index as usize) {
            content[0] = content[0].max(x + f32::from(hyphen.line.width) - start);
            start = x;
          }
          let end = glyphs.peek().map_or(width, |next| f32::from(next.x));
          content[0] = content[0].max(end - start);
        }
        hyphen = character == '-' && previous.is_alphanumeric();
        previous = character;
      }
      line_start += line.text.len() + 1;
    }
    if !wrap {
      content[0] = content[1] - float;
    }
    measured.set_lead(key, lead + top);
    let ceil = |width: f32| leaf_ceil(width, measured.pixel_ratio);
    content = [
      ceil(content[0].max(float)),
      ceil(content[1] + top),
      (content[2] + top).max(float),
    ];
  } else if matches!(display, "grid" | "inline-grid")
    && ![Prop::GRID_TEMPLATE_COLUMNS, Prop::GRID_TEMPLATE_ROWS]
      .into_iter()
      .any(|name| style.get(name).is_some_and(|v| v.starts_with("subgrid")))
  {
    // Without rows, only the columns are laid out, and the rows and captions are 0.
    let mut extent = |width: Option<f32>, height: Option<f32>, rows: bool| {
      let bits = if rows {
        height.map(f32::to_bits)
      } else {
        COLUMNS
      };
      let key = (id, width.map(f32::to_bits), bits, true);
      let whole = (id, key.1, None, true);
      if let Some([columns, tall, captions, _]) =
        (measured.size(key)).or_else(|| measured.size(whole).filter(|_| !rows))
      {
        return [columns, tall, captions];
      }
      let grid = layout(
        doc, id, styles, width, height, None, None, text, measured, rows,
      );
      let [columns, rows] = [0, 1].map(|axis| grid.extent(axis));
      let captions = grid.captions[0] + grid.captions[1];
      measured.set_size(key, [columns, rows, captions, 0.]);
      if bits != COLUMNS {
        measured.set_grid((id, key.1, bits), grid);
      }
      [columns, rows, captions]
    };
    let sized = available.is_some() || vertical[0].is_some();
    let [min, ..] = extent(Some(0.), None, false);
    let [max, natural, mut captions] = extent(None, None, !sized);
    content = [min, max, natural];
    if sized {
      let fit = style.get(Prop::WIDTH) == Some("fit-content");
      let width = available.map(|width| if fit { width.min(max).max(min) } else { width });
      [_, content[2], captions] = extent(width, vertical[0], true);
    }
    // The height of a table leaves out its captions.
    if captions > 0. {
      let captions = extent(available, vertical[0], true)[2];
      vertical = vertical.map(|height| height.map(|height| height + captions));
    }
  } else {
    // The content of spliced inline boxes is in the flow of the box.
    let mut kids = std::borrow::Cow::Borrowed(&node.children[..]);
    if !matches!(display, "flex" | "inline-flex")
      && node
        .children
        .iter()
        .any(|&child| crate::render::inline::spliced(doc, styles, child))
    {
      let (mut flat, mut pending) = (Vec::new(), node.children.clone());
      pending.reverse();
      while let Some(child) = pending.pop() {
        match crate::render::inline::spliced(doc, styles, child) {
          true => pending.extend(doc.nodes[child].children.iter().rev()),
          false => flat.push(child),
        }
      }
      kids = std::borrow::Cow::Owned(flat);
    }
    let inline = !kids.is_empty()
      && kids.iter().all(|&child| {
        doc.nodes[child].tag.starts_with('#')
          || doc.nodes[child].tag == "br"
          || styles.values[child]
            .get(Prop::DISPLAY)
            .is_none_or(|value| value.starts_with("inline"))
      });
    let flex = matches!(display, "flex" | "inline-flex");
    let row = inline
      || flex
        && style
          .get(Prop::FLEX_DIRECTION)
          .is_none_or(|v| !v.starts_with("column"));
    let wrap = inline || flex && style.get(Prop::FLEX_WRAP).is_some_and(|v| v != "nowrap");
    let gaps = [Prop::COLUMN_GAP, Prop::ROW_GAP].map(|name| {
      style
        .get(name)
        .filter(|_| flex)
        .and_then(|v| units.resolve(v, available.unwrap_or(0.)))
        .unwrap_or(0.)
        .max(0.)
    });
    let preserve = style
      .get(Prop::WHITE_SPACE)
      .is_some_and(|v| matches!(v, "pre" | "pre-wrap" | "break-spaces"));
    if flex {
      measured.note_runs(doc, styles, id, &node.children, false);
    }
    let made: Vec<bool> = match flex {
      true => (node.children.iter())
        .map(|&child| doc.nodes[child].tag != "#text" || measured.text_item(doc, child, preserve))
        .collect(),
      false => Vec::new(),
    };
    let skipped = |at: usize, child: usize| {
      styles.values[child]
        .get(Prop::POSITION)
        .is_some_and(|v| matches!(v, "absolute" | "fixed"))
        || styles.values[child].get(Prop::DISPLAY) == Some("none")
        || doc.nodes[child].tag == "#text"
          && !match flex {
            true => made[at],
            false => preserve || !doc.nodes[child].text.trim_ascii().is_empty(),
          }
        || doc.nodes[child].tag == "#text"
          && node.tag == "details"
          && !node.attrs.contains_key("open")
        || floated(child)
    };
    // Flex base sizes decide where lines break, and the widths of a single line where text wraps.
    // The flexed widths, and whether each item starts a line after the first.
    let (mut resolved, mut starts) = (Vec::new(), Vec::new());
    if let Some(space) = available.filter(|_| flex && row) {
      let mut items: Vec<[f32; 4]> = Vec::new();
      for (at, &child) in node.children.iter().enumerate() {
        if skipped(at, child) {
          continue;
        }
        let own = &styles.values[child];
        let lengths = &styles.lengths[child];
        let [min, max, auto, _] = intrinsic(doc, child, styles, None, None, text, measured);
        let frame = frame(own, lengths, space);
        let sides = frame[1] + frame[3];
        // A length sizes the border box with `box-sizing: border-box`, which keeps only the margins outside.
        let outer = |v: f32| {
          if own.get(Prop::BOX_SIZING) == Some("border-box") {
            let margin = lengths.edges(own, css::properties::MARGIN, space);
            (v + margin[1] + margin[3]).max(sides)
          } else {
            v + sides
          }
        };
        let basis = match own.get(Prop::FLEX_BASIS) {
          Some("min-content") => min,
          Some("max-content" | "fit-content") => max,
          Some("content") => max,
          value => value
            .filter(|v| *v != "auto")
            .or(own.get(Prop::WIDTH).filter(|v| v.contains('%')))
            .and_then(|v| lengths.resolve(v, space))
            .map_or(auto, outer),
        };
        let lower = if own
          .get(Prop::OVERFLOW_X)
          .is_some_and(|v| !matches!(v, "visible" | "clip"))
          || own.get(Prop::MIN_WIDTH).is_some_and(|v| v != "auto")
        {
          own
            .get(Prop::MIN_WIDTH)
            .and_then(|v| lengths.resolve(v, space))
            .map_or(0., outer)
        } else {
          min
        };
        let factor = |prop, fallback| {
          own
            .get(prop)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(fallback)
            .max(0.)
        };
        items.push([
          basis.max(lower),
          lower,
          factor(Prop::FLEX_GROW, 0.),
          factor(Prop::FLEX_SHRINK, 1.) * basis,
        ]);
      }
      let hypothetical = items
        .iter()
        .map(|item| if wrap { item[0].min(space) } else { item[0] })
        .collect::<Vec<_>>();
      resolved.clone_from(&hypothetical);
      starts = vec![false; items.len()];
      // Each line resolves the flexible lengths of its items.
      let mut start = 0;
      while start < items.len() {
        let mut end = start + 1;
        let mut used = hypothetical[start];
        while wrap && end < items.len() && used + gaps[0] + hypothetical[end] <= space {
          used += gaps[0] + hypothetical[end];
          end += 1;
        }
        if !wrap {
          end = items.len();
        }
        let line = start..end;
        if let Some(next) = starts.get_mut(end) {
          *next = true;
        }
        start = end;
        let space = space - gaps[0] * line.len().saturating_sub(1) as f32;
        let mut frozen = vec![false; line.len()];
        for _ in 0..line.len() {
          let free = space - resolved[line.clone()].iter().sum::<f32>();
          let index = 2 + usize::from(free < 0.);
          let total: f32 = line
            .clone()
            .filter(|&i| !frozen[i - line.start])
            .map(|i| items[i][index])
            .sum();
          if total <= 0. || free.abs() < 0.01 {
            break;
          }
          let free = if index == 2 {
            free * total.min(1.)
          } else {
            free
          };
          let mut clamped = false;
          for i in line.clone() {
            if !frozen[i - line.start] {
              resolved[i] += free * items[i][index] / total;
              if resolved[i] < items[i][1] {
                resolved[i] = items[i][1];
                frozen[i - line.start] = true;
                clamped = true;
              }
            }
          }
          if !clamped {
            break;
          }
          // Restart the distribution from the bases of the items that are not frozen.
          for i in line.clone() {
            if !frozen[i - line.start] {
              resolved[i] = items[i][0];
            }
          }
        }
      }
    }
    let (mut resolved, mut starts) = (resolved.into_iter(), starts.into_iter());
    // `line` is the width, the tallest piece, and how far raised pieces reach above and below the strut.
    let (mut line, mut run) = ([0f32; 4], 0f32);
    // How far the items of a flex line that align on their first or last baselines reach above and below it.
    let mut aligned = [[0f32; 2]; 2];
    // Margins that adjoin the top edge, those after the last box placed, and whether a box was placed.
    let (mut top, mut pending, mut placed) = ([0f32; 2], [0f32; 2], false);
    let join = |a: [f32; 2], b: [f32; 2]| [a[0].max(b[0]), a[1].min(b[1])];
    // Whether the line has an inline box, so it has the strut.
    let mut strut = false;
    let total = |line: [f32; 4], strut: bool| {
      line[1].max(if strut || line[2] + line[3] > 0. {
        units.font.lh + line[2] + line[3]
      } else {
        0.
      })
    };
    let mut first = true;
    // Like Chrome, an inside marker before a block is in a line of its own.
    if super::super::size::inside_marker(styles, id)
      && (kids.iter().enumerate())
        .find(|&(at, &child)| !skipped(at, child))
        .is_some_and(|(_, &child)| {
          !doc.nodes[child].tag.starts_with('#')
            && doc.nodes[child].tag != "br"
            && (styles.values[child].get(Prop::DISPLAY))
              .is_some_and(|value| !value.starts_with("inline"))
        })
    {
      (content[2], placed) = (units.font.lh, true);
    }
    // A collapsed space between inline boxes, which hangs at the end of a line.
    let (mut spaced, mut space) = (false, None);
    for (at, &child) in kids.iter().enumerate() {
      if skipped(at, child) {
        spaced |= inline && doc.nodes[child].tag == "#text" && line[0] > 0.;
        continue;
      }
      let gap = if first { [0.; 2] } else { gaps };
      first = false;
      let inline_level = !flex
        && (doc.nodes[child].tag.starts_with('#')
          || doc.nodes[child].tag == "br"
          || styles.values[child]
            .get(Prop::DISPLAY)
            .is_none_or(|value| value.starts_with("inline")));
      if !row && !inline_level {
        content[1] = content[1].max(run);
        let height = total(line, strut);
        if height > 0. {
          content[2] += pending[0] + pending[1] + height;
          (pending, placed) = ([0.; 2], true);
        }
        (line, run, spaced, strut) = ([0.; 4], 0., false, false);
        let [min, max, _, height] = intrinsic(doc, child, styles, available, None, text, measured);
        content = [content[0].max(min), content[1].max(max), content[2]];
        let key = (child, available.map(f32::to_bits), None);
        let [above, below] = measured.margins(key).unwrap_or_default();
        // Flex children keep their margins apart. Like Chrome, block children collapse adjoining ones.
        if flex {
          content[2] += height + gap[1];
          continue;
        }
        let inner = height - above[0] - above[1] - below[0] - below[1];
        match (inner > 0., placed) {
          (false, false) => top = join(top, join(above, below)),
          (false, true) => pending = join(pending, join(above, below)),
          (true, false) => {
            top = join(top, above);
            content[2] += inner;
            (pending, placed) = (below, true);
          }
          (true, true) => {
            let between = join(pending, above);
            content[2] += between[0] + between[1] + inner;
            (pending, placed) = (below, true);
          }
        }
        continue;
      }
      let [min, max, _, height] = intrinsic(doc, child, styles, None, None, text, measured);
      let width = resolved.next().unwrap_or_else(|| match available {
        Some(available)
          if styles.values[child]
            .get(Prop::WIDTH)
            .is_some_and(|v| v.contains('%') || v == "stretch")
            || fills(
              doc,
              styles,
              child,
              super::super::size::natural_size(
                doc,
                child,
                measured.naturals.get(&child).copied(),
                &styles.values[child],
              )
              .and_then(|(_, ratio)| ratio),
            )
            || doc.nodes[child].tag == "svg"
              && styles.values[child].get(Prop::WIDTH).is_none()
              && !doc.nodes[child].attrs.contains_key("width") =>
        {
          intrinsic(doc, child, styles, Some(available), None, text, measured)[2]
        }
        Some(available) => max.min(available),
        None => max,
      });
      // Flex lines break where their items were collected, before they grow or shrink.
      let starts = starts.next();
      // A box with its own width takes percentages from the container.
      let sized = flex
        && styles.values[child]
          .get(Prop::WIDTH)
          .is_some_and(|v| v != "auto");
      let at = if sized && available.is_some() {
        available
      } else {
        (width != max).then_some(width)
      };
      let height = match at {
        Some(_) => intrinsic(doc, child, styles, at, None, text, measured)[3],
        None => height,
      };
      let wrap = wrap || inline_level;
      content[0] = if wrap {
        content[0].max(min)
      } else {
        content[0] + min + gap[0]
      };
      let spacing = if std::mem::take(&mut spaced) && inline_level {
        *space.get_or_insert_with(|| {
          let run = style::text_run(style, units, 1);
          f32::from(
            text
              .shape_line(" ", renderer::px(units.font.em), &run)
              .line
              .width,
          )
        })
      } else {
        0.
      };
      run += max + gap[0] + spacing;
      if starts.unwrap_or_else(|| {
        wrap
          && line[0] > 0.
          && available.is_some_and(|available| line[0] + gaps[0] + spacing + width > available)
      }) {
        content[2] += total(line, strut) + gaps[1];
        (line, aligned, strut) = ([0.; 4], [[0.; 2]; 2], false);
      }
      line[0] += if line[0] > 0. { gaps[0] + spacing } else { 0. } + width;
      // Like a line box, a raised or lowered inline box grows the line by what it reaches past the strut.
      let (child_style, child_units) = (&styles.values[child], &styles.lengths[child]);
      let boxed = !inline_level
        || doc.nodes[child].tag.starts_with('#')
        || child_style
          .get(Prop::DISPLAY)
          .is_some_and(|value| value != "inline");
      let raise = match child_style.get(Prop::VERTICAL_ALIGN) {
        _ if boxed => None,
        Some("super") => Some(units.font.em / 3. + 1. / measured.pixel_ratio),
        Some("sub") => Some(-units.font.em / 5. - 1. / measured.pixel_ratio),
        Some(value) => child_units.resolve(value, child_units.font.lh),
        None => None,
      };
      // How far the inline box of the child reaches past the strut, above and below.
      let reach = |raise: f32| {
        let baseline = |style, units: &css::value::units::Lengths| {
          f32::from(text.baseline(
            text.face(&style::font(style)),
            renderer::px(units.font.em),
            renderer::px(units.font.lh),
            measured.pixel_ratio,
          ))
        };
        let (above, root) = (baseline(child_style, child_units), baseline(style, units));
        [
          above + raise - root,
          child_units.font.lh - above - raise - (units.font.lh - root),
        ]
      };
      if !boxed {
        strut = true;
      }
      match raise {
        Some(raise) => {
          let [above, below] = reach(raise);
          line[2] = line[2].max(above);
          line[3] = line[3].max(below);
        }
        None => {
          // Like a line box, a line with an inline box on the baseline keeps the strut, and the
          // padding, borders and margins of the inline box hang outside the line.
          let height = if boxed {
            height
          } else {
            let [above, below] = reach(0.);
            line[2] = line[2].max(above);
            line[3] = line[3].max(below);
            let frame = frame(child_style, child_units, available.unwrap_or(0.));
            height - frame[0] - frame[2]
          };
          line[1] = line[1].max(height);
          // Like layout, items aligned on their baselines can make the line taller than each item.
          if flex
            && row
            && let Some(align) = styles.values[child]
              .get(Prop::ALIGN_SELF)
              .filter(|value| *value != "auto")
              .or(style.get(Prop::ALIGN_ITEMS))
              .filter(|value| value.ends_with("baseline"))
          {
            let [above, _, below, _] = child_units.edges(
              child_style,
              css::properties::MARGIN,
              available.unwrap_or(0.),
            );
            // An item without a baseline sits on its bottom border edge.
            let last = usize::from(align.starts_with("last"));
            let above = above
              + baseline(doc, child, styles, at, last == 1, text, measured)
                .unwrap_or(height - above - below);
            let group = &mut aligned[last];
            *group = [group[0].max(above), group[1].max(height - above)];
            line[1] = line[1].max(group[0] + group[1]);
          }
          // A replaced box on the baseline of the strut keeps the strut below it in the line.
          if inline_level
            && !flex
            && child_style
              .get(Prop::VERTICAL_ALIGN)
              .is_none_or(|value| value == "baseline")
            && (matches!(doc.nodes[child].tag.as_str(), "textarea" | "svg")
              || doc.nodes[child].tag == "progress"
                && [Prop::APPEARANCE, Prop::WEBKIT_APPEARANCE]
                  .into_iter()
                  .any(|prop| child_style.get(prop) == Some("none"))
              || super::super::size::natural_size(
                doc,
                child,
                measured.naturals.get(&child).copied(),
                child_style,
              )
              .is_some())
          {
            let root = f32::from(text.baseline(
              text.face(&style::font(style)),
              renderer::px(units.font.em),
              renderer::px(units.font.lh),
              measured.pixel_ratio,
            ));
            // A range sits on the bottom of its border box.
            let hang = if doc.nodes[child].tag == "input" {
              frame(child_style, child_units, available.unwrap_or(0.))[2]
            } else {
              0.
            };
            line[2] = line[2].max(height - hang - root);
            line[3] = line[3].max(hang - (units.font.lh - root));
          }
        }
      }
      if !flex && doc.nodes[child].tag == "br" {
        content[1] = content[1].max(run);
        content[2] += total(line, strut).max(styles.lengths[child].font.lh);
        (line, run, strut) = ([0.; 4], 0., false);
      }
    }
    content[1] = content[1].max(run);
    // Like Chrome, an empty item keeps the line of its marker.
    let height = if display == "list-item"
      && style.get(Prop::LIST_STYLE_TYPE) != Some("none")
      && node.children.iter().all(|&child| {
        doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty()
      }) {
      units.font.lh
    } else {
      total(line, strut)
    };
    if height > 0. {
      content[2] += pending[0] + pending[1] + height;
      (pending, placed) = ([0.; 2], true);
    }
    // Like Chrome, margins of the first and last children pass through edges without padding or border,
    // unless the box starts a block formatting context.
    let parent = node
      .parent
      .and_then(|parent| styles.values[parent].get(Prop::DISPLAY))
      .unwrap_or("block");
    let contained = flex
      || node.tag == "fieldset"
      || !matches!(display, "block" | "list-item" | "")
      || matches!(parent, "flex" | "inline-flex" | "grid" | "inline-grid")
      || [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
        .into_iter()
        .any(|prop| {
          style
            .get(prop)
            .is_some_and(|v| !matches!(v, "visible" | "clip"))
        })
      || style.get(Prop::FLOAT).is_some_and(|v| v != "none")
      || style
        .get(Prop::POSITION)
        .is_some_and(|v| matches!(v, "absolute" | "fixed"))
      || style.get(Prop::CONTAIN).is_some_and(|v| v != "none")
      || style
        .get(Prop::CONTENT_VISIBILITY)
        .is_some_and(|v| v != "visible");
    let own = |value: f32| [value.max(0.), value.min(0.)];
    let mut sets = [own(margin[0]), own(margin[2])];
    if !flex && crate::render::trims(style, 0) {
      top = [0.; 2];
    }
    if !flex && crate::render::trims(style, 2) {
      pending = [0.; 2];
    }
    if !placed {
      pending = top;
      top = [0.; 2];
    }
    if !contained && inner[1] == 0. && !placed && vertical.iter().all(Option::is_none) {
      sets = [join(join(sets[0], sets[1]), pending), [0.; 2]];
    } else {
      if !contained && padding[0] + border[0] == 0. {
        sets[0] = join(sets[0], top);
      } else {
        content[2] += top[0] + top[1];
      }
      if !contained
        && padding[2] + border[2] == 0.
        && vertical[0].is_none()
        && vertical[1].is_none_or(|min| min == 0.)
      {
        sets[1] = join(sets[1], pending);
      } else {
        content[2] += pending[0] + pending[1];
      }
    }
    collapsed = Some(sets);
  }
  // Like Chrome, floats side by side add up in the max-content width, and the box holds the lowest. A float
  // that clears the side of a float before it starts a new row below them.
  if natural.is_none() {
    let (mut low, mut sum, mut row, mut top, mut tall) = (0f32, 0f32, 0f32, 0f32, 0f32);
    let mut sides = [false; 2];
    for &child in node.children.iter().filter(|&&child| floated(child)) {
      let [min, max, _, height] = intrinsic(doc, child, styles, available, None, text, measured);
      let style = &styles.values[child];
      let right = |value: &str| matches!(value, "right" | "inline-end");
      if match style.get(Prop::CLEAR).unwrap_or("none") {
        "none" => false,
        "both" => sides[0] || sides[1],
        value => sides[usize::from(right(value))],
      } {
        (row, top, sides) = (0., tall, [false; 2]);
      }
      sides[usize::from(style.get(Prop::FLOAT).is_some_and(right))] = true;
      row += max.max(0.);
      (low, sum, tall) = (low.max(min), sum.max(row), tall.max(top + height));
    }
    content = [content[0].max(low), content[1] + sum, content[2].max(tall)];
  }
  // A grid lays out its pseudo-elements as items.
  if !matches!(display, "grid" | "inline-grid")
    && !matches!(
      node.tag.as_str(),
      "img" | "svg" | "canvas" | "video" | "input" | "textarea" | "select"
    )
  {
    for index in (0..2).filter(|&index| !taken[index]) {
      let Some([_, width, _, height]) = pseudo_size(
        doc,
        id,
        index,
        styles,
        base,
        None,
        text,
        measured.pixel_ratio,
      ) else {
        continue;
      };
      let pseudo = &styles.generated(id).unwrap()[index];
      if matches!(display, "flex" | "inline-flex")
        && style
          .get(Prop::FLEX_DIRECTION)
          .is_none_or(|v| !v.starts_with("column"))
      {
        content[0] += width;
        content[1] += width;
        content[2] = content[2].max(height);
      } else if pseudo
        .get(Prop::DISPLAY)
        .is_none_or(|value| value.starts_with("inline"))
        || pseudo.get(Prop::FLOAT).is_some_and(|value| value != "none")
      {
        content[0] = content[0].max(width);
        content[1] += width;
      } else {
        content[0] = content[0].max(width);
        content[1] = content[1].max(width);
        content[2] += height;
      }
    }
  }
  if node.tag == "select"
    && ![Prop::APPEARANCE, Prop::WEBKIT_APPEARANCE]
      .into_iter()
      .any(|prop| style.get(prop) == Some("none"))
  {
    let (face, em, ratio) = (style::font(style), units.font.em, measured.pixel_ratio);
    if let Some(rows) = super::super::size::list_rows(node) {
      content[2] = rows as f32 * super::super::size::option_row(&face, em, text, ratio);
    } else {
      let [ascent, descent, _] = text
        .line_metrics(text.face(&face), renderer::px(em * ratio))
        .map(f32::from);
      content[2] = (ascent + descent) / ratio + 2.;
      let widest = super::super::size::widest_option(doc, id, &face, em, text);
      // The look takes 4px before the label and 12px after it.
      let width = (widest * ratio).ceil() / ratio + 16.;
      content[0] = width;
      content[1] = width;
    }
  }
  let mut bounds = [fixed; 2];
  if keywords.get()[0] {
    if width.is_none() {
      measured.set_size((id, RAW, None, true), [content[0], content[1], 0., 0.]);
    }
    for (end, bounds) in bounds.iter_mut().enumerate() {
      let ends = [content[0], content[1]];
      // Without a space, `fit-content` is the content size.
      keyword_sizes(
        style,
        units,
        horizontal,
        bounds,
        ends,
        None,
        content[end],
        0.,
        sides(0),
      );
    }
    fixed = bounds[1];
  }
  let [min, max] = [0, 1].map(|end| clamp(bounds[end][0].unwrap_or(content[end]), bounds[end]));
  let min = if cell {
    // Like Chrome, the width of a table cell raises its max-content width, not its min-content width.
    let most = fixed[2].map_or(f32::INFINITY, |most| most.max(min));
    content[0].max(fixed[1].unwrap_or(0.)).min(most)
  } else if matches!(
    node.tag.as_str(),
    "img" | "video" | "canvas" | "iframe" | "svg" | "embed" | "object" | "input" | "textarea"
  ) && [Prop::WIDTH, Prop::MAX_WIDTH]
    .into_iter()
    .any(|prop| style.get(prop).is_some_and(|value| value.contains('%')))
  {
    0.
  } else {
    min
  };
  let ratio = vertical[0]
    .is_none()
    .then(|| style.get(Prop::ASPECT_RATIO))
    .flatten()
    .filter(|value| !(value.contains("auto") && natural.is_some_and(|(_, ratio)| ratio.is_some())))
    .and_then(super::super::size::aspect_ratio)
    .zip(full)
    .map(|(ratio, width)| {
      let height = if border_box {
        (width + inner[0]) / ratio - inner[1]
      } else {
        width / ratio
      };
      let clips = style
        .get(Prop::OVERFLOW_Y)
        .is_some_and(|value| value != "visible");
      if clips || matches!(node.tag.as_str(), "svg" | "img" | "canvas" | "video") {
        height.max(0.)
      } else {
        height.max(content[2])
      }
    });
  if keywords.get()[1] {
    let size = ratio.unwrap_or(content[2]);
    measured.set_size((id, key.1, RAW, true), [size, size, 0., 0.]);
    let props = [Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT];
    let room = height.map(|height| (height - outer[1]).max(0.));
    let base = height.unwrap_or(0.);
    keyword_sizes(
      style,
      units,
      props,
      &mut vertical,
      [size; 2],
      room,
      size,
      base,
      sides(1),
    );
  }
  let own = |value: f32| [value.max(0.), value.min(0.)];
  let mut sets = collapsed.unwrap_or([own(margin[0]), own(margin[2])]);
  let mut result = [
    min + outer[0],
    max + outer[0],
    full.unwrap_or(max) + outer[0],
    clamp(vertical[0].or(ratio).unwrap_or(content[2]), vertical)
      + inner[1]
      + sets
        .iter()
        .map(|[positive, negative]| positive + negative)
        .sum::<f32>(),
  ];
  // A max-width below the max-content width wraps the content.
  if width.is_none()
    && turn.is_none()
    && bounds[1][0].is_none()
    && max < content[1]
    && natural.is_none()
    && !matches!(node.tag.as_str(), "svg" | "input" | "select" | "textarea")
  {
    // The measure of the wrapped content reads the content sizes.
    measured.set_size((key.0, key.1, key.2, false), result);
    result[3] = intrinsic(
      doc,
      id,
      styles,
      Some(max + outer[0]),
      height,
      text,
      measured,
    )[3];
    let at = (id, Some((max + outer[0]).to_bits()), key.2);
    sets = measured.margins(at).unwrap_or(sets);
    if let Some(lead) = measured.lead(at) {
      measured.set_lead(key, lead);
    }
  }
  measured.set_size((key.0, key.1, key.2, false), result);
  measured.set_margins(key, sets);
  result
}

/// The first or last baseline of a box from the top of its border box.
pub(super) fn baseline(
  doc: &Document,
  id: usize,
  styles: &ComputedStyles,
  width: Option<f32>,
  last: bool,
  text: &renderer::TextSystem,
  measured: &mut Measurements,
) -> Option<f32> {
  let (node, style, units) = (&doc.nodes[id], &styles.values[id], &styles.lengths[id]);
  let display = style.get(Prop::DISPLAY).unwrap_or("");
  let flex = display.ends_with("flex")
    && style
      .get(Prop::FLEX_DIRECTION)
      .is_none_or(|value| !value.starts_with("column"));
  if !last
    || flex
    || display.ends_with("grid")
    || matches!(node.tag.as_str(), "input" | "select" | "textarea")
  {
    return first_baseline(doc, id, styles, width, text, measured);
  }
  let first = first_baseline(doc, id, styles, width, text, measured)?;
  let [.., used, height] = intrinsic(doc, id, styles, width, None, text, measured);
  let base = width.unwrap_or(0.);
  let [margin, padding] = [css::properties::MARGIN, css::properties::PADDING]
    .map(|sides| units.edges(style, sides, base));
  let border = units.borders(style, base);
  let bottom = height - margin[0] - margin[2] - padding[2] - border[2];
  if node.tag.starts_with('#') || measured.texts.get(&id).is_some_and(Option::is_some) {
    let strut = f32::from(text.baseline(
      text.face(&style::font(style)),
      renderer::px(units.font.em),
      renderer::px(units.font.lh),
      measured.pixel_ratio,
    ));
    return Some(bottom - units.font.lh + strut);
  }
  let sides = [margin, padding, border];
  let inner = used - sides.iter().map(|side| side[1] + side[3]).sum::<f32>();
  let mut at = bottom;
  for &child in node.children.iter().rev() {
    let own = &styles.values[child];
    if doc.nodes[child].tag == "#comment"
      || doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty()
      || own.get(Prop::DISPLAY) == Some("none")
      || (own.get(Prop::POSITION)).is_some_and(|v| matches!(v, "absolute" | "fixed"))
    {
      continue;
    }
    let outer = intrinsic(doc, child, styles, Some(inner), None, text, measured)[3];
    if let Some(found) = baseline(doc, child, styles, Some(inner), true, text, measured) {
      let above = (styles.lengths[child]).edges(own, css::properties::MARGIN, inner)[0];
      return Some(at - outer + above + found);
    }
    at -= outer;
  }
  Some(first)
}

/// The first baseline of a box from the top of its border box, as layout places it: the first line of its text, or
/// the baseline of its first item. `None` for a box without one, which then sits on its bottom border edge.
pub(super) fn first_baseline(
  doc: &Document,
  id: usize,
  styles: &ComputedStyles,
  width: Option<f32>,
  text: &renderer::TextSystem,
  measured: &mut Measurements,
) -> Option<f32> {
  let [.., used, height] = intrinsic(doc, id, styles, width, None, text, measured);
  let (node, style, units) = (&doc.nodes[id], &styles.values[id], &styles.lengths[id]);
  let base = width.unwrap_or(0.);
  let [margin, padding] = [css::properties::MARGIN, css::properties::PADDING]
    .map(|sides| units.edges(style, sides, base));
  let border = units.borders(style, base);
  let top = padding[0] + border[0];
  let strut = |measured: &Measurements| {
    f32::from(text.baseline(
      text.face(&style::font(style)),
      renderer::px(units.font.em),
      renderer::px(units.font.lh),
      measured.pixel_ratio,
    ))
  };
  // Like Chrome, a replaced box sits on its bottom edge.
  if node.tag == "svg"
    || super::super::size::natural_size(doc, id, measured.naturals.get(&id).copied(), style)
      .is_some()
  {
    return None;
  }
  // A control centers its line of text.
  if matches!(node.tag.as_str(), "input" | "select" | "textarea") {
    let inner = height - margin[0] - margin[2] - top - padding[2] - border[2];
    if node.tag == "textarea" {
      return Some(top + strut(measured));
    }
    return Some(top + (inner - units.font.lh) / 2. + strut(measured));
  }
  if matches!(node.tag.as_str(), "progress" | "meter") {
    return None;
  }
  if node.tag.starts_with('#') || measured.texts.get(&id).is_some_and(Option::is_some) {
    let lead = measured.lead((id, width.map(f32::to_bits), None));
    return Some(top + lead.unwrap_or(0.) + strut(measured));
  }
  let display = style.get(Prop::DISPLAY).unwrap_or("");
  let flex = matches!(display, "flex" | "inline-flex");
  let grid = display.ends_with("grid");
  let preserve = style
    .get(Prop::WHITE_SPACE)
    .is_some_and(|v| matches!(v, "pre" | "pre-wrap" | "break-spaces"));
  let inner = used
    - [margin, padding, border]
      .iter()
      .map(|edges| edges[1] + edges[3])
      .sum::<f32>();
  let in_flow = |child: usize, measured: &Measurements| {
    let own = &styles.values[child];
    match doc.nodes[child].tag.as_str() {
      "#comment" => false,
      "#text" if flex || grid => measured.text_item(doc, child, preserve),
      "#text" => preserve || !doc.nodes[child].text.trim_ascii().is_empty(),
      _ => {
        own.get(Prop::DISPLAY) != Some("none")
          && own
            .get(Prop::POSITION)
            .is_none_or(|v| !matches!(v, "absolute" | "fixed"))
          && (flex || grid || own.get(Prop::FLOAT).is_none_or(|v| v == "none"))
      }
    }
  };
  // The baseline of an item from the top of its margin box, or its bottom border edge without one.
  let item = |child: usize, width: Option<f32>, measured: &mut Measurements| {
    let (own, lengths) = (&styles.values[child], &styles.lengths[child]);
    let [above, _, below, _] = lengths.edges(own, css::properties::MARGIN, width.unwrap_or(inner));
    let outer = intrinsic(doc, child, styles, width, None, text, measured)[3];
    let baseline = first_baseline(doc, child, styles, width, text, measured);
    (above + baseline.unwrap_or(outer - above - below), outer)
  };
  let align = |child: usize| {
    (styles.values[child].get(Prop::ALIGN_SELF))
      .filter(|v| *v != "auto")
      .or(style.get(Prop::ALIGN_ITEMS))
      .unwrap_or("normal")
  };
  let shared = |align: &str| align.ends_with("baseline") && !align.starts_with("last");
  let shift = |align: &str, free: f32| match align {
    "center" | "safe center" => free / 2.,
    "end" | "flex-end" | "self-end" => free,
    _ => 0.,
  };
  // Like CSS, the items aligned on their baselines give the baseline of the first row, or else its first item.
  if grid {
    let layout = layout(
      doc,
      id,
      styles,
      Some(inner),
      None,
      None,
      None,
      text,
      measured,
      true,
    );
    let first = layout.cells.iter().map(|cell| cell.1[1]).min()?;
    // (aligned, column, baseline)
    let mut found: Option<(bool, usize, f32)> = None;
    for &(child, cell, [_, y, width, _], _) in &layout.cells {
      let aligned = shared(align(child));
      match found {
        _ if cell[1] != first => continue,
        Some((true, ..)) if !aligned => continue,
        Some((false, column, _)) if !aligned && column <= cell[0] => continue,
        _ => {}
      }
      let at = y + item(child, Some(width), measured).0;
      found = Some(match found {
        Some((true, column, max)) => (true, column, max.max(at)),
        _ => (aligned, cell[0], at),
      });
    }
    return found.map(|(.., baseline)| top + baseline);
  }
  if flex
    && style
      .get(Prop::FLEX_DIRECTION)
      .is_none_or(|v| !v.starts_with("column"))
  {
    let cross = height - margin[0] - margin[2] - top - padding[2] - border[2];
    let (mut aligned, mut first) = (None::<f32>, None);
    for &child in &node.children {
      if !in_flow(child, measured) {
        continue;
      }
      let align = align(child);
      if shared(align) {
        let (baseline, _) = item(child, None, measured);
        aligned = Some(aligned.unwrap_or(0f32).max(baseline));
      } else if first.is_none() {
        let (baseline, outer) = item(child, None, measured);
        first = Some(shift(align, cross - outer) + baseline);
      }
    }
    return aligned.or(first).map(|baseline| top + baseline);
  }
  // Columns and blocks take the baseline of their first item that has one.
  let gap = (style.get(Prop::ROW_GAP))
    .filter(|_| flex)
    .and_then(|v| units.resolve(v, 0.))
    .unwrap_or(0.)
    .max(0.);
  let mut y = top;
  for &child in &node.children {
    if !in_flow(child, measured) {
      continue;
    }
    if let Some(baseline) = first_baseline(doc, child, styles, Some(inner), text, measured) {
      let (own, lengths) = (&styles.values[child], &styles.lengths[child]);
      return Some(y + lengths.edges(own, css::properties::MARGIN, inner)[0] + baseline);
    }
    y += intrinsic(doc, child, styles, Some(inner), None, text, measured)[3] + gap;
  }
  None
}

// A video without a frame, with a CSS ratio and an auto size, which fills the width that it gets.
pub(super) fn fills(
  doc: &Document,
  styles: &ComputedStyles,
  id: usize,
  natural_ratio: Option<f32>,
) -> bool {
  let (node, style) = (&doc.nodes[id], &styles.values[id]);
  node.tag == "video"
    && natural_ratio.is_none()
    && style.get(Prop::ASPECT_RATIO).is_some()
    && [(Prop::WIDTH, "width"), (Prop::HEIGHT, "height")]
      .iter()
      .all(|&(prop, name)| {
        style
          .get(prop)
          .map_or(!node.attrs.contains_key(name), |value| value == "auto")
      })
}

/// The pseudo-element of an item key from `layout`: its element and its index, 0 for `::before`.
pub(crate) fn pseudo_item(child: usize) -> Option<(usize, usize)> {
  (child > usize::MAX / 2).then_some((!child / 2, !child % 2))
}

/// The sizes of an in-flow pseudo-element like `intrinsic`, with percentages of `base`. A `width` is the room of
/// its grid area, which an `auto` width fills.
#[allow(clippy::too_many_arguments)]
pub(super) fn pseudo_size(
  doc: &Document,
  id: usize,
  index: usize,
  styles: &ComputedStyles,
  base: f32,
  width: Option<f32>,
  text: &renderer::TextSystem,
  pixel_ratio: f32,
) -> Option<[f32; 4]> {
  let style = &styles.values[id];
  let pseudo = &styles.generated(id)?[index];
  let value = crate::render::inline::drawn(pseudo, style)?;
  if pseudo
    .get(Prop::POSITION)
    .is_some_and(|value| matches!(value, "absolute" | "fixed"))
  {
    return None;
  }
  let units =
    super::super::size::pseudo_lengths(&styles.lengths[id], style, pseudo, text, pixel_ratio);
  let [padding, margin] = [css::properties::PADDING, css::properties::MARGIN]
    .map(|edges| units.edges(pseudo, edges, base));
  let border = units.borders(pseudo, base);
  // The frame of each axis as [outside the size, outside the content].
  let sides = |axis: usize| {
    let inner = padding[1 - axis] + padding[3 - axis] + border[1 - axis] + border[3 - axis];
    let outer = margin[1 - axis] + margin[3 - axis];
    match pseudo.get(Prop::BOX_SIZING) {
      Some("border-box") => [outer, outer + inner],
      _ => [inner + outer; 2],
    }
  };
  let limits = |props: [Prop; 2], axis: usize| {
    props.map(|prop| {
      pseudo
        .get(prop)
        .and_then(|value| units.resolve(value, base))
        .map(|limit| limit + sides(axis)[0])
    })
  };
  let clamp = |size: f32, [min, max]: [Option<f32>; 2]| {
    size
      .min(max.unwrap_or(f32::INFINITY))
      .max(min.unwrap_or(0.))
  };
  let string = style::pseudo_content(doc, id, pseudo, value);
  let [fixed, tall] = [Prop::WIDTH, Prop::HEIGHT].map(|prop| {
    pseudo
      .get(prop)
      .and_then(|value| units.resolve(value, base))
  });
  let run = style::text_run(pseudo, &units, string.len());
  let wraps = pseudo
    .get(Prop::WHITE_SPACE)
    .is_none_or(|v| !matches!(v, "nowrap" | "pre"));
  let shape = |width: Option<f32>| {
    text.shape_text(
      &string,
      renderer::px(units.font.em),
      std::slice::from_ref(&run),
      &renderer::Wrapping {
        width: width.filter(|_| wraps).map(renderer::px),
        word_break: style::word_break(pseudo),
        ..Default::default()
      },
    )
  };
  let lines = (fixed.is_none() && !string.is_empty()).then(|| shape(None));
  let [min, max] = match (fixed, &lines) {
    (Some(fixed), _) => [fixed + sides(0)[0]; 2],
    (None, None) => [sides(0)[1]; 2],
    (None, Some(lines)) => [
      f32::from(renderer::widest_piece(
        lines,
        renderer::px(0.),
        Default::default(),
      )),
      lines
        .iter()
        .map(|line| f32::from(line.line.width))
        .fold(0., f32::max),
    ]
    .map(|size| size + sides(0)[1]),
  }
  .map(|size| clamp(size, limits([Prop::MIN_WIDTH, Prop::MAX_WIDTH], 0)));
  let used = match (fixed, width) {
    (None, Some(width)) => clamp(width, limits([Prop::MIN_WIDTH, Prop::MAX_WIDTH], 0)),
    _ => max,
  };
  // Like the render, blank content of a block-level pseudo-element makes no line.
  let rows = if string.trim_ascii().is_empty()
    && pseudo
      .get(Prop::DISPLAY)
      .is_some_and(|v| !v.starts_with("inline"))
    && pseudo
      .get(Prop::WHITE_SPACE)
      .is_none_or(|v| !matches!(v, "pre" | "pre-wrap" | "pre-line" | "break-spaces"))
  {
    0
  } else if used < max && wraps {
    shape(Some(used - sides(0)[1]))
      .iter()
      .map(|line| line.rows())
      .sum()
  } else {
    1
  };
  let height = tall.map_or(rows as f32 * units.font.lh + sides(1)[1], |height| {
    height + sides(1)[0]
  });
  let height = clamp(height, limits([Prop::MIN_HEIGHT, Prop::MAX_HEIGHT], 1));
  Some([min, max, used, height])
}

/// The width, height and baseline of an inline-level pseudo-element that is an atom of its element's text.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pseudo_atom(
  doc: &Document,
  id: usize,
  index: usize,
  styles: &ComputedStyles,
  base: f32,
  available: Option<f32>,
  text: &renderer::TextSystem,
  pixel_ratio: f32,
) -> Option<[f32; 3]> {
  let [_, max, used, height] = pseudo_size(doc, id, index, styles, base, None, text, pixel_ratio)?;
  let pseudo = &styles.generated(id).unwrap()[index];
  let units = super::super::size::pseudo_lengths(
    &styles.lengths[id],
    &styles.values[id],
    pseudo,
    text,
    pixel_ratio,
  );
  let blank = pseudo.get(Prop::CONTENT).is_some_and(|value| {
    style::pseudo_content(doc, id, pseudo, value)
      .trim_ascii()
      .is_empty()
  });
  // An inline-block without lines sits on its bottom margin edge, others on their last line.
  let baseline = if blank {
    height
  } else {
    let [ascent, descent, _] = text
      .line_metrics(
        text.face(&style::font(pseudo)),
        renderer::px(units.font.em * pixel_ratio),
      )
      .map(|v| f32::from(v) / pixel_ratio);
    let below = units.font.lh - ascent - ((units.font.lh - ascent - descent) / 2.);
    height - frame(pseudo, &units, base)[2] - below
  };
  let percent = available.is_some() && pseudo.get(Prop::WIDTH).is_some_and(|v| v.contains('%'));
  Some([if percent { used } else { max }, height, baseline])
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn fixed_scroll_bars_leave_the_content_less_room() {
    let text = renderer::TextSystem::new();
    let mut doc = Document::parse(
      "<style>.s{overflow-y:scroll}.s::-webkit-scrollbar,.g::-webkit-scrollbar{width:40px}.g{overflow-y:hidden;scrollbar-gutter:stable both-edges}</style><div class=s>aaa aaa aaa</div><div class=g>aaa aaa aaa</div><div class=s style='scrollbar-width:none'>aaa aaa aaa</div>",
    );
    let mut state = crate::render::PageState::default();
    let styles = state.restyle(&mut doc, [800., 600.]);
    let divs: Vec<usize> = (0..doc.nodes.len())
      .filter(|&id| doc.nodes[id].tag == "div")
      .collect();
    let mut measured = Measurements::default();
    let mut size =
      |id, width| intrinsic(&doc, id, &styles, Some(width), None, &text, &mut measured);
    let [line, lines] = [500., 70.].map(|width| size(divs[2], width)[3]);
    assert!(lines > line);
    assert_eq!(size(divs[0], 110.)[2..], [110., lines]);
    assert_eq!(size(divs[1], 150.)[3], lines);
    assert_eq!(size(divs[2], 110.)[3], line);
  }
}
