use crate::style;
use renderer::{AlignItems, Pixels, px};

// How far each line of a paragraph reaches above and below its baseline, with its inline boxes and the strut.
#[allow(clippy::too_many_arguments)]
pub(crate) fn row_extents<'a>(
  lines: impl IntoIterator<Item = &'a renderer::TextLine>,
  runs: &[renderer::TextRun],
  heights: &[(usize, style::LineHeight)],
  atoms: &[(usize, [f32; 3], Option<AlignItems>)],
  strut: (renderer::FontId, Pixels, Pixels),
  system: &renderer::TextSystem,
  scale: f32,
  room: f32,
) -> Vec<[Pixels; 2]> {
  let inline = |font, size: Pixels, line: Option<f32>| {
    let [ascent, descent, gap] = system.line_metrics(font, size * scale).map(f32::from);
    let line = line.unwrap_or(ascent + descent + gap);
    let above = ascent + ((line - ascent - descent) / 2.).floor();
    [above, line - above]
  };
  let root = inline(strut.0, strut.1, Some(f32::from(strut.2) * scale));
  let run_ends: Vec<usize> = runs
    .iter()
    .scan(0, |end, run| {
      *end += run.len;
      Some(*end)
    })
    .collect();
  let marked = runs.iter().any(|run| run.emphasis.is_some());
  let mut rows = Vec::new();
  // How far the emphasis marks and the glyphs of each row reach over and under its baseline.
  let mut marks: Vec<[f32; 4]> = Vec::new();
  let mut edges = Vec::new();
  let mut offset = 0;
  for line in lines {
    let layout = &line.line;
    let ends: Vec<usize> = (1..line.rows()).map(|row| line.row_index(row)).collect();
    let first = rows.len();
    rows.resize(first + ends.len() + 1, root);
    if marked {
      marks.resize(rows.len(), [0.; 4]);
    }
    // Glyphs of one face and size measure once per row, line height and source run.
    let mut last = None;
    for glyph in &layout.glyphs {
      let row = first + ends.partition_point(|&end| end <= glyph.index as usize);
      let at = offset + glyph.index as usize;
      // Atoms are in text order.
      if let Some((_, [_, height, baseline], align)) = atoms
        .binary_search_by_key(&at, |(place, ..)| *place)
        .ok()
        .map(|atom| &atoms[atom])
      {
        if let Some(align @ (AlignItems::FlexStart | AlignItems::FlexEnd)) = align {
          edges.push((row, height * scale, *align == AlignItems::FlexEnd));
          continue;
        }
        // Do not shift the row for fractional-pixel differences between a full-line baseline box
        // and the strut.
        let [above, below] = [baseline * scale, (height - baseline) * scale];
        rows[row] = [
          rows[row][0].max(if above - root[0] < 1. { 0. } else { above }),
          rows[row][1].max(if below - root[1] < 1. { 0. } else { below }),
        ];
        continue;
      }
      // Like Chrome, glyphs count only inside their em box, and an emphasis mark is half the font size beyond them.
      if marked && let Some(run) = runs.get(run_ends.partition_point(|&end| end <= at)) {
        let size = f32::from(run.font_size.unwrap_or(layout.font_size)) * scale;
        let em = system.metrics(glyph.font).em_ascent;
        let line = system.line_metrics(glyph.font, px(size)).map(f32::from);
        let edges = [line[0].min(size * em), line[1].min(size * (1. - em))];
        let kept = &mut marks[row];
        for side in 0..2 {
          kept[side + 2] = kept[side + 2].max(edges[side].ceil());
        }
        if let Some(mark) = &run.emphasis {
          let side = usize::from(mark.under);
          kept[side] = kept[side].max((edges[side] + size / 2.).ceil());
        }
      }
      let kind = heights
        .get(heights.partition_point(|(end, _)| *end <= at))
        .map_or(style::LineHeight::Normal, |(_, kind)| *kind);
      let source =
        (kind != style::LineHeight::Normal).then(|| run_ends.partition_point(|&end| end <= at));
      if last == Some((glyph.font, glyph.size, row, kind, source)) {
        continue;
      }
      last = Some((glyph.font, glyph.size, row, kind, source));
      let styled = source.and_then(|index| runs.get(index));
      // Synthetic small capitals and adjusted sizes don't change the line.
      let size = styled.map_or(glyph.size, |run| run.font_size.unwrap_or(layout.font_size));
      let [above, below] = inline(
        styled.map_or(glyph.font, |styled| system.face(&styled.font)),
        size,
        match kind {
          style::LineHeight::Normal => None,
          style::LineHeight::Scale(factor) => {
            Some((factor * f32::from(size) * scale * 64.).floor() / 64.)
          }
          style::LineHeight::Fixed(line) => Some((line * scale * 64.).round() / 64.),
        },
      );
      rows[row] = [rows[row][0].max(above), rows[row][1].max(below)];
    }
    offset += line.len() + 1;
  }
  edges.sort_by(|a: &(usize, f32, bool), b| b.1.total_cmp(&a.1));
  for (row, height, bottom) in edges {
    let [above, below] = &mut rows[row];
    if bottom {
      *above = above.max(height - *below);
    } else {
      *below = below.max(height - *above);
    }
  }
  // Like Chrome, marks that overflow a row move it down, less the room that the row before leaves below its glyphs,
  // and marks under the glyphs move the next row down, less the room above its glyphs.
  // The first row can reach into the margin before its block.
  let (mut overflow, mut room) = (0f32, room * scale);
  for (row, [over, under, top, bottom]) in marks.iter().enumerate() {
    let [above, below] = rows[row];
    let lift = (over - above).max(0.);
    rows[row][0] += match overflow > 0. {
      true => lift + overflow - overflow.min((above - top).max(0.)),
      false => (lift - room).max(0.),
    };
    overflow = (under - below).max(0.);
    room = (below - bottom).max(0.);
    if row + 1 == marks.len() {
      rows[row][1] += overflow;
    }
  }
  rows
    .into_iter()
    .map(|[above, below]| [above - root[0], below - root[1]].map(|extra| px(extra.max(0.) / scale)))
    .collect()
}
