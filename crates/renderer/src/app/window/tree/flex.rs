use super::*;

impl LayoutTree<'_> {
    pub(super) fn flexbox(&mut self, node: NodeId, mut inputs: LayoutInput) -> LayoutOutput {
        // Taffy resolves percent heights of row items against a known height even when it is not definite. Such a
        // height comes from the content and limits of the box, so laying out without it gives the same size.
        if !inputs.known_dimensions_are_definite.height
            && self.slots[usize::from(node)].style.sizing.is_none()
            && self.slots[usize::from(node)].style.inline == Inline::None
            && matches!(
                self.slots[usize::from(node)].taffy().flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            )
            && self.slots[usize::from(node)]
                .children
                .iter()
                .any(|&child| relative_height(self.slots[usize::from(child)].taffy()))
        {
            inputs.known_dimensions.height = None;
        }
        let start = self.hung.len();
        let mut output = self.lines(node, inputs, start);
        let index = usize::from(node);
        let style = self.slots[index].taffy();
        if inputs.known_dimensions.width.is_none()
            && matches!(
                style.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            )
            && let taffy::AvailableSpace::Definite(available) = inputs.available_space.width
            && output.size.width > available
        {
            let min = self.lines(
                node,
                LayoutInput {
                    run_mode: RunMode::ComputeSize,
                    available_space: taffy::Size {
                        width: taffy::AvailableSpace::MinContent,
                        ..inputs.available_space
                    },
                    ..inputs
                },
                start,
            );
            let width = available.max(min.size.width);
            if width < output.size.width {
                output = self.lines(
                    node,
                    LayoutInput {
                        known_dimensions: taffy::Size {
                            width: Some(width),
                            ..inputs.known_dimensions
                        },
                        ..inputs
                    },
                    start,
                );
            }
        }
        if inputs.run_mode == RunMode::PerformLayout
            && matches!(
                self.slots[index].taffy().flex_direction,
                FlexDirection::Column | FlexDirection::ColumnReverse
            )
        {
            self.group_baselines(index, inputs.parent_size.width, output.size.width);
        }
        if inputs.run_mode == RunMode::PerformLayout
            && self.slots[index].style.inline == Inline::None
            && matches!(
                self.slots[index].taffy().flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            )
        {
            self.row_baselines(index, inputs.parent_size.width, &mut output);
        }
        if inputs.run_mode != RunMode::PerformLayout
            || self.slots[index].style.inline != Inline::Lines
        {
            self.hung.truncate(start);
            return output;
        }
        let style = self.slots[index].style.clone();
        let lift = match style.justify_content {
            Some(AlignContent::Center) => 0.5,
            Some(AlignContent::End | AlignContent::FlexEnd) => 1.,
            _ => 0.,
        };
        let flows = |slot: &Slot| {
            slot.taffy().position != taffy::Position::Absolute
                && slot.taffy().display != taffy::Display::None
        };
        let align = |slot: &Slot| slot.style.align_self.or(style.align_items);
        let children = mem::take(&mut self.slots[index].children);
        let (mut first, mut right, mut top) = (0, f32::NEG_INFINITY, true);
        for at in 0..=children.len() {
            if let Some(&child) = children.get(at) {
                let slot = &self.slots[usize::from(child)];
                if !flows(slot) {
                    continue;
                }
                let layout = &slot.layout;
                let starts = layout.location.x - layout.margin.left < right - 0.5;
                right = layout.location.x + layout.size.width + layout.margin.right;
                if !starts {
                    continue;
                }
            }
            let line = &children[first..at];
            first = at;
            let (mut above, mut below) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
            for &child in line {
                let slot = &self.slots[usize::from(child)];
                if flows(slot) && align(slot) == Some(AlignItems::Baseline) {
                    let layout = &slot.layout;
                    let base = slot.baselines.first.unwrap_or(layout.size.height);
                    above = above.max(base + layout.margin.top);
                    below = below.max(layout.size.height + layout.margin.bottom - base);
                }
            }
            let mut tallest = (0., false);
            for &child in line {
                let slot = &self.slots[usize::from(child)];
                let layout = &slot.layout;
                let height = layout.size.height + layout.margin.top + layout.margin.bottom;
                let edge = align(slot)
                    .filter(|align| matches!(align, AlignItems::FlexStart | AlignItems::FlexEnd));
                if flows(slot) && edge.is_some() && height > tallest.0 {
                    tallest = (height, edge == Some(AlignItems::FlexEnd));
                }
            }
            let drop = if tallest.1 && above.is_finite() {
                (tallest.0 - above - below).max(0.)
            } else {
                0.
            };
            // Struts after the last box take no room. A line that keeps a box through a smaller margin already
            // leaves out that much of its space, in the margin of a space of its own.
            let hang = line
                .iter()
                .rfind(|&&child| {
                    let slot = &self.slots[usize::from(child)];
                    flows(slot) && slot.layout.size.width > 0.
                })
                .filter(|_| lift > 0.)
                .and_then(|&end| {
                    let space = hanging(self.slots, end)?;
                    let layout = &self.slots[usize::from(space)].layout;
                    let kept = self.hung[start..]
                        .iter()
                        .find(|(hung, ..)| *hung == end && space != end)
                        .map_or(0., |&(_, overflow, _)| overflow);
                    Some(
                        (layout.size.width + layout.margin.left + layout.margin.right - kept)
                            * lift,
                    )
                })
                .unwrap_or(0.);
            if drop > 0. || hang > 0. {
                for &child in line {
                    let slot = &mut self.slots[usize::from(child)];
                    if flows(slot) {
                        slot.layout.location.x += hang;
                        if align(slot) == Some(AlignItems::Baseline) {
                            slot.layout.location.y += drop;
                        }
                    }
                }
                if top {
                    output.baselines.first = output.baselines.first.map(|base| base + drop);
                }
            }
            top = false;
        }
        // Taffy ignores align-content on a single line, which a button or a block still aligns in its height.
        let lift = match style.align_content {
            Some(AlignContent::Center) => 0.5,
            Some(AlignContent::End | AlignContent::FlexEnd) => 1.,
            _ => 0.,
        };
        let own = self.slots[index].taffy();
        if lift > 0. && own.flex_wrap == taffy::FlexWrap::NoWrap {
            use taffy::ResolveOrZero;
            let calc = |value, basis| crate::style::resolve_calc(value, basis);
            let width = inputs.parent_size.width;
            let edges =
                own.padding.resolve_or_zero(width, calc) + own.border.resolve_or_zero(width, calc);
            let (mut above, mut below) = (f32::INFINITY, f32::NEG_INFINITY);
            for &child in &children {
                let slot = &self.slots[usize::from(child)];
                if flows(slot) {
                    let layout = &slot.layout;
                    above = above.min(layout.location.y - layout.margin.top);
                    below =
                        below.max(layout.location.y + layout.size.height + layout.margin.bottom);
                }
            }
            let mut free = output.size.height - edges.top - edges.bottom - (below - above);
            if style.safe {
                free = free.max(0.);
            }
            let shift = edges.top - above + free * lift;
            if above.is_finite() && shift != 0. {
                for &child in &children {
                    let slot = &mut self.slots[usize::from(child)];
                    if flows(slot) {
                        slot.layout.location.y += shift;
                    }
                }
                let baselines = &mut output.baselines;
                for base in [&mut baselines.first, &mut baselines.last] {
                    *base = base.map(|base| base + shift);
                }
            }
        }
        self.slots[index].children = children;
        self.hung.truncate(start);
        output
    }

    /// Taffy puts the items that align on their baselines at the top of their line. Like Chrome, those that align on
    /// their last baselines go to its end, and `wrap-reverse` swaps the start and the end.
    fn row_baselines(
        &mut self,
        index: usize,
        parent_width: Option<f32>,
        output: &mut LayoutOutput,
    ) {
        let style = self.slots[index].taffy();
        let reverse = style.flex_wrap == taffy::FlexWrap::WrapReverse;
        let items = self.slots[index].style.align_items;
        let kind = |slot: &Slot| {
            let margin = slot.taffy().margin;
            slot.style.align_self.or(items).filter(|align| {
                matches!(align, AlignItems::Baseline | AlignItems::LastBaseline)
                    && !margin.top.is_auto()
                    && !margin.bottom.is_auto()
            })
        };
        let flows = |slot: &Slot| {
            slot.taffy().position != taffy::Position::Absolute
                && slot.taffy().display != taffy::Display::None
        };
        let members: Vec<usize> = (self.slots[index].children.iter())
            .map(|&child| usize::from(child))
            .filter(|&child| flows(&self.slots[child]))
            .collect();
        if !(members.iter()).any(|&child| match kind(&self.slots[child]) {
            Some(AlignItems::LastBaseline) => true,
            align => reverse && align.is_some(),
        }) {
            return;
        }
        use taffy::ResolveOrZero;
        let calc = |value, basis| crate::style::resolve_calc(value, basis);
        let edges = style.padding.resolve_or_zero(parent_width, calc)
            + style.border.resolve_or_zero(parent_width, calc);
        let wraps = style.flex_wrap != taffy::FlexWrap::NoWrap;
        let row = style.flex_direction == FlexDirection::Row;
        let gap = style
            .gap
            .height
            .resolve_or_zero(Some(output.size.height), calc);
        let stretch = matches!(
            style.align_content.keyword,
            taffy::AlignContentKeyword::Normal | taffy::AlignContentKeyword::Stretch
        );
        let span = |slots: &[Slot], child: usize| {
            let layout = &slots[child].layout;
            [
                layout.location.y - layout.margin.top,
                layout.location.y + layout.size.height + layout.margin.bottom,
            ]
        };
        // The lines from the top: their top, bottom and items.
        let mut lines: Vec<(f32, f32, Vec<usize>)> = Vec::new();
        let mut reached = f32::NEG_INFINITY;
        for &child in &members {
            let layout = &self.slots[child].layout;
            let [start, end] = match row {
                true => [layout.location.x, layout.location.x + layout.size.width],
                false => [-layout.location.x - layout.size.width, -layout.location.x],
            };
            let [top, bottom] = span(self.slots, child);
            match lines.last_mut() {
                Some(line) if !wraps || start > reached - 0.5 => {
                    (line.0, line.1) = (line.0.min(top), line.1.max(bottom));
                    line.2.push(child);
                }
                _ => lines.push((top, bottom, vec![child])),
            }
            reached = end;
        }
        // `wrap-reverse` lines go up.
        if lines.len() > 1 && lines[0].0 > lines[1].0 {
            lines.reverse();
        }
        let count = lines.len();
        for at in 0..count {
            let next = lines.get(at + 1).map(|line| line.0 - gap);
            if !wraps || stretch && at == 0 {
                lines[at].0 = edges.top;
            }
            if !wraps || stretch {
                lines[at].1 = next.unwrap_or(output.size.height - edges.bottom);
            }
        }
        // Lines that stretch share the free space. Without it, a stretched item tells no size of its line.
        let natural = |line: &[usize]| {
            let mut tallest = 0f32;
            let mut groups = [[0f32; 2]; 2];
            for &child in line {
                let slot = &self.slots[child];
                let [top, bottom] = span(self.slots, child);
                let last = match kind(slot) {
                    Some(align) => align == AlignItems::LastBaseline,
                    None => {
                        if matches!(
                            slot.style.align_self.or(items),
                            None | Some(AlignItems::Stretch)
                        ) && slot.taffy().size.height.is_auto()
                        {
                            return None;
                        }
                        tallest = tallest.max(bottom - top);
                        continue;
                    }
                };
                let base = match last {
                    true => slot.baselines.last,
                    false => slot.baselines.first,
                };
                let above = base.unwrap_or(slot.layout.size.height) + slot.layout.margin.top;
                let group = &mut groups[usize::from(last)];
                *group = [group[0].max(above), group[1].max(bottom - top - above)];
            }
            Some(
                tallest
                    .max(groups[0][0] + groups[0][1])
                    .max(groups[1][0] + groups[1][1]),
            )
        };
        let sizes: Option<Vec<f32>> = lines.iter().map(|(.., line)| natural(line)).collect();
        if wraps && let Some(sizes) = sizes {
            let inner = output.size.height - edges.top - edges.bottom;
            let used = sizes.iter().sum::<f32>() + gap * (count - 1) as f32;
            let extra = match stretch {
                true => (inner - used).max(0.) / count as f32,
                false => 0.,
            };
            let mut at = edges.top;
            for (line, size) in lines.iter_mut().zip(sizes) {
                if stretch {
                    line.0 = at;
                }
                line.1 = line.0 + size + extra;
                at = line.1 + gap;
            }
        }
        for (top, bottom, line) in &lines {
            for last in [false, true] {
                let group = match last {
                    true => AlignItems::LastBaseline,
                    false => AlignItems::Baseline,
                };
                let base = |slot: &Slot| {
                    let baselines = slot.baselines;
                    let base = if last {
                        baselines.last
                    } else {
                        baselines.first
                    };
                    base.unwrap_or(slot.layout.size.height)
                };
                let (mut above, mut below) = (0f32, 0f32);
                for &child in line {
                    let slot = &self.slots[child];
                    if kind(slot) == Some(group) {
                        let layout = &slot.layout;
                        above = above.max(base(slot) + layout.margin.top);
                        below = below.max(layout.size.height + layout.margin.bottom - base(slot));
                    }
                }
                let shared = match last != reverse {
                    true => bottom - below,
                    false => top + above,
                };
                for &child in line {
                    if kind(&self.slots[child]) == Some(group) {
                        let base = base(&self.slots[child]);
                        self.slots[child].layout.location.y = shared - base;
                    }
                }
            }
        }
        let line = if reverse { lines.last() } else { lines.first() };
        let first = line.and_then(|(.., line)| {
            let aligned = [AlignItems::Baseline, AlignItems::LastBaseline]
                .into_iter()
                .find_map(|group| {
                    let mut line = line.iter().copied();
                    line.find(|&child| kind(&self.slots[child]) == Some(group))
                });
            let slot = &self.slots[aligned.or(line.first().copied())?];
            let base = match kind(slot) {
                Some(AlignItems::LastBaseline) => slot.baselines.last,
                _ => slot.baselines.first,
            };
            Some(slot.layout.location.y + base?)
        });
        output.baselines.first = first.or(output.baselines.first);
    }
}
