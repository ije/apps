use super::*;

impl LayoutTree<'_> {
    pub(super) fn flexbox(&mut self, node: NodeId, mut inputs: LayoutInput) -> LayoutOutput {
        // Taffy resolves percent heights of row items against a known height even when it is not definite. Such a
        // height comes from the content and limits of the box, so laying out without it gives the same size.
        if !inputs.known_dimensions_are_definite.height
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
}
