use super::*;

impl LayoutTree<'_> {
    // A column set lays out its flow as one column, then breaks it into columns of one height and moves its boxes
    // there. The set is as tall as a column.
    pub(super) fn columns(
        &mut self,
        node: NodeId,
        inputs: LayoutInput,
        output: LayoutOutput,
    ) -> LayoutOutput {
        let index = usize::from(node);
        let (Some(columns), Some(&flow)) = (
            self.slots[index].style.columns.as_deref().copied(),
            self.slots[index].children.first(),
        ) else {
            return output;
        };
        // Without a width, the set is as wide as its columns of the content's width and the gaps between them.
        if inputs.known_dimensions.width.is_none() {
            let mut output = output;
            let count = f32::from(columns.count);
            output.size.width =
                output.size.width * count + columns.gap.0 * self.scale * (count - 1.);
            return output;
        }
        if inputs.run_mode != RunMode::PerformLayout {
            let inputs = LayoutInput {
                run_mode: RunMode::PerformLayout,
                ..inputs
            };
            return LayoutOutput::from_outer_size(self.compute(node, inputs, None).size);
        }
        let flow = usize::from(flow);
        let Layout { size: one, .. } = self.slots[flow].layout;
        let pitch = one.width + columns.gap.0 * self.scale;
        let mut chances = Vec::new();
        // Like Chrome, the margin at the end of the content takes no room.
        let end = chances_in(self.slots, flow, 0., false, self.scale, &mut chances);
        chances.push((f32::INFINITY, end, END));
        // Greedy fill: the column starts, the least extra height that would fit more, whether a break that should
        // be avoided was taken or the last column overflows, and the tallest content of a column.
        let fill = |height: f32| {
            let (mut cuts, mut short, mut avoided, mut tallest) =
                (vec![0f32], f32::INFINITY, false, 0f32);
            let (mut fit, mut fit_avoided, mut at) = (None, None, 0);
            while let Some(&(cut, end, bits)) = chances.get(at) {
                let start = cuts[cuts.len() - 1];
                if bits == SLICE {
                    // The column ends inside the space: break there. Else the space fits up to its end, or a
                    // whole piece before it overflows a column of its own.
                    if end - start - height > 0.01 && cut <= start + height + 0.01 && height > 0.01
                    {
                        // Like Chrome, the part of the box that did not fit is a shortage too.
                        short = short.min(end - start - height);
                        cuts.push(start + height);
                        (fit, fit_avoided, tallest) = (None, None, tallest.max(height));
                        continue;
                    }
                    if end <= start + height + 0.01 {
                        fit = Some(at);
                    } else if cut > start + height {
                        short = short.min(cut - start - height);
                        if fit.is_none() && fit_avoided.is_none() {
                            cuts.push(cut);
                            tallest = tallest.max(cut - start);
                            continue;
                        }
                    }
                    at += 1;
                    continue;
                }
                if cut <= start {
                    at += 1;
                    continue;
                }
                let over = end - start - height;
                if over <= 0.01 {
                    match bits {
                        FORCED => {
                            cuts.push(cut);
                            (fit, fit_avoided) = (None, None);
                            tallest = tallest.max(end - start);
                        }
                        AVOID => fit_avoided = Some(at),
                        END => tallest = tallest.max(end - start),
                        _ => fit = Some(at),
                    }
                    at += 1;
                    continue;
                }
                short = short.min(over);
                let pick = fit.or(fit_avoided).unwrap_or(at);
                avoided |= fit.is_none() && chances[pick].2 == AVOID;
                if chances[pick].2 == END {
                    avoided = true;
                    break;
                }
                let (cut, end, bits) = chances[pick];
                cuts.push(if bits == SLICE { end } else { cut });
                tallest = tallest.max(end - start);
                (fit, fit_avoided, at) = (None, None, pick + 1);
            }
            (cuts, short, avoided, tallest)
        };
        let count = usize::from(columns.count);
        let limit = columns.height.map(|height| height.0 * self.scale);
        // Like Chrome, start from the runs between forced breaks with their end margins, and give the other
        // breaks to the tallest runs.
        let mut runs: Vec<(f32, f32)> = Vec::new();
        let mut from = 0.;
        for &(cut, _, bits) in &chances {
            if bits == FORCED {
                runs.push((cut - from, 1.));
                from = cut;
            }
        }
        runs.push((one.height - from, 1.));
        for _ in runs.len()..count {
            let tallest = runs
                .iter_mut()
                .max_by(|a, b| (a.0 / a.1).total_cmp(&(b.0 / b.1)));
            if let Some(run) = tallest {
                run.1 += 1.;
            }
        }
        // A column is at least as tall as the tallest piece between two breaks, which can't be split.
        let (mut piece, mut from) = (0f32, 0.);
        for &(cut, end, bits) in &chances {
            piece = piece.max(end.min(cut) - from);
            if bits == SLICE {
                from = end;
            } else {
                from = cut;
            }
        }
        let mut height = runs.iter().map(|run| run.0 / run.1).fold(piece, f32::max);
        if let Some(limit) = limit.filter(|limit| columns.fill || *limit < height) {
            height = limit;
        }
        // Balance like Chrome: grow the height by the least shortage until the content fits. A fixed number of
        // steps keeps the cost linear in the content, and long content then searches between that height and one
        // that fits.
        let fits =
            |(cuts, _, avoided, _): &(Vec<f32>, f32, bool, f32)| cuts.len() <= count && !avoided;
        let mut done = fill(height);
        if !(columns.fill && limit.is_some()) {
            let upper = limit.unwrap_or(f32::INFINITY).min(one.height.max(height));
            for _ in 0..16 {
                if fits(&done) || !done.1.is_finite() || height >= upper {
                    break;
                }
                height = (height + done.1.max(0.01)).min(upper);
                done = fill(height);
            }
            if !fits(&done) && done.1.is_finite() && height < upper {
                let mut low = height;
                height = upper;
                if fits(&fill(height)) {
                    for _ in 0..24 {
                        let middle = (low + height) / 2.;
                        if fits(&fill(middle)) {
                            height = middle;
                        } else {
                            low = middle;
                        }
                    }
                    // The tallest column of the found breaks is the exact height that holds them.
                    height = fill(height).3;
                }
                done = fill(height);
            }
        }
        let cuts = done.0;
        self.assign(flow, 0., Point::default(), &cuts, height, pitch);
        let mut output = output;
        output.size.height = height;
        if let Some(area) = &mut output.oof_positioning_area {
            area.size.height = height;
        }
        output
    }

    fn assign(
        &mut self,
        index: usize,
        top: f32,
        base: Point<f32>,
        cuts: &[f32],
        height: f32,
        pitch: f32,
    ) {
        let column = |y: f32| {
            cuts.partition_point(|&cut| cut <= y + 0.01)
                .saturating_sub(1)
        };
        let place = |column: usize| point(column as f32 * pitch, -cuts[column]);
        for child in 0..self.slots[index].children.len() {
            let child = usize::from(self.slots[index].children[child]);
            let slot = &self.slots[child];
            let (at, extent) = (top + slot.layout.location.y - lift(slot), slot.layout.size);
            let first = column(at);
            let last = column(at + (extent.height - 0.02).max(0.));
            let shift = place(first);
            let spans = first != last && splits(self.slots, child);
            let mut split = Split {
                shift: shift - base,
                rects: Box::default(),
                rows: Box::default(),
            };
            if spans {
                split.rects = (first..=last)
                    .map(|column| {
                        let from = if column == first { at } else { cuts[column] };
                        let to = if column == last {
                            at + extent.height
                        } else {
                            (cuts[column] + height).max(cuts[column + 1])
                        };
                        let y = from - cuts[column] - at + cuts[first];
                        Rect::new(
                            point((column - first) as f32 * pitch, y),
                            size(extent.width, to - from),
                        )
                    })
                    .collect();
                if let Kind::Text(_) = slot.kind {
                    let line = slot.text.line_height.0 * self.scale;
                    let start = at + slot.layout.border.top + slot.layout.padding.top;
                    split.rows = (first + 1..=last)
                        .map(|column| {
                            let row = ((cuts[column] - start) / line).round().max(0.) as u32;
                            (row, place(column) - shift)
                        })
                        .collect();
                }
            }
            let split = (split.shift != Point::default() || spans).then(|| Rc::new(split));
            if self.slots[child].split != split {
                let old = mem::replace(&mut self.slots[child].split, split);
                if old.is_some_and(|old| !old.rects.is_empty()) && !spans {
                    for inner in 0..self.slots[child].children.len() {
                        unsplit(self.slots, usize::from(self.slots[child].children[inner]));
                    }
                }
                if !self.exact {
                    changed(self.slots, child);
                }
            }
            if spans && matches!(self.slots[child].kind, Kind::Box) {
                self.assign(child, at, shift, cuts, height, pitch);
            }
        }
    }
}
