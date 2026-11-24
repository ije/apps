use super::*;

impl LayoutTree<'_> {
    // Preserve Taffy's sum order for exact layout values. Fall back to Taffy for unsupported rows.
    pub(super) fn row(&mut self, node: NodeId, inputs: LayoutInput) -> Option<LayoutOutput> {
        use taffy::{AlignContentKeyword, CompactLength, MaybeMath, MaybeResolve, ResolveOrZero};
        let LayoutInput {
            known_dimensions: known,
            parent_size,
            available_space,
            run_mode,
            sizing_mode,
            ..
        } = inputs;
        let width = known.width?;
        let index = usize::from(node);
        let style = self.slots[index].taffy();
        let calc = |value, basis| crate::style::resolve_calc(value, basis);
        let aligns = |align: taffy::AlignItems| {
            align.safety == taffy::AlignmentSafety::Default
                && matches!(
                    align.keyword,
                    AlignItemsKeyword::Normal
                        | AlignItemsKeyword::Stretch
                        | AlignItemsKeyword::Center
                        | AlignItemsKeyword::FlexStart
                        | AlignItemsKeyword::Start
                )
        };
        if self.exact
            || run_mode != RunMode::PerformLayout
            || !inputs.known_dimensions_are_definite.width
            || style.flex_direction != FlexDirection::Row
            || style.flex_wrap != taffy::FlexWrap::NoWrap
            || style.direction != taffy::Direction::Ltr
            || style.box_sizing != taffy::BoxSizing::BorderBox
            || style.aspect_ratio.is_some()
            || !aligns(style.align_items)
            || !matches!(
                style.justify_content.keyword(),
                AlignContentKeyword::Normal
                    | AlignContentKeyword::Start
                    | AlignContentKeyword::FlexStart
            )
            || !matches!(
                style.align_content.keyword,
                AlignContentKeyword::Normal
                    | AlignContentKeyword::Stretch
                    | AlignContentKeyword::Start
                    | AlignContentKeyword::FlexStart
            )
            || style.gap.width.into_raw().tag() != CompactLength::LENGTH_TAG
            || self.slots[index].style.inline == Inline::Lines
        {
            return None;
        }
        let simple = self.slots[index].children.iter().all(|&child| {
            let child = self.slots[usize::from(child)].taffy();
            child.display == taffy::Display::None
                || child.position != taffy::Position::Absolute
                    && aligns(child.align_self.unwrap_or(style.align_items))
                    && !child.flex_basis.is_sizing_keyword()
                    && !child.flex_basis.is_content()
                    && plain(child)
        });
        if !simple {
            return None;
        }
        let padding = style.padding.resolve_or_zero(parent_size.width, calc);
        let border = style.border.resolve_or_zero(parent_size.width, calc);
        let inset = padding + border;
        let min = style.min_size.maybe_resolve(parent_size, calc);
        let max = style.max_size.maybe_resolve(parent_size, calc);
        let definite = match (min.height, max.height) {
            (Some(min), Some(max)) if max <= min => Some(min),
            _ => None,
        };
        let styled = match sizing_mode {
            taffy::SizingMode::InherentSize => style
                .size
                .height
                .maybe_resolve(parent_size.height, calc)
                .maybe_clamp(min.height, max.height),
            taffy::SizingMode::ContentSize => None,
        };
        let pb = padding.sum_axes() + border.sum_axes();
        let fixed = known.height.or(definite.or(styled).maybe_max(pb.height));
        let (main, cross) = (inset.left + inset.right, inset.top + inset.bottom);
        let inner = (width - main, fixed.map(|height| height - cross));
        let gap = style.gap.width.into_raw().value();
        let (align_items, contain) = (style.align_items, style.contain);
        let space = match fixed {
            Some(height) => taffy::AvailableSpace::Definite((height - cross).max(0.)),
            None => available_space.height.maybe_sub(cross).maybe_max(0.),
        };
        struct Item {
            node: NodeId,
            order: u32,
            margin: taffy::Rect<f32>,
            padding: taffy::Rect<f32>,
            border: taffy::Rect<f32>,
            stretch: bool,
            center: bool,
            cross: Option<f32>,
            sides: f32,
            scrolls: bool,
            grow: f32,
            basis: f32,
            floor: f32,
            limit: Option<f32>,
            hypothetical: f32,
            target: f32,
            frozen: bool,
            violation: f32,
        }
        let mut items = Vec::with_capacity(self.slots[index].children.len());
        for order in 0..self.slots[index].children.len() {
            let child = self.slots[index].children[order];
            let style = self.slots[usize::from(child)].taffy();
            if style.display == taffy::Display::None {
                continue;
            }
            let margin = style.margin.resolve_or_zero(Some(inner.0), calc);
            let padding = style.padding.resolve_or_zero(Some(inner.0), calc);
            let border = style.border.resolve_or_zero(Some(inner.0), calc);
            let percents = taffy::Size {
                width: Some(inner.0),
                height: inner.1,
            };
            let size = style.size.maybe_resolve(percents, calc);
            let [min, max] =
                [style.min_size, style.max_size].map(|size| size.maybe_resolve(percents, calc));
            let align = style.align_self.unwrap_or(align_items).keyword;
            let stretch = matches!(
                align,
                AlignItemsKeyword::Normal | AlignItemsKeyword::Stretch
            ) && style.size.height.is_auto();
            if stretch && inner.1.is_none() {
                return None;
            }
            let automatic = min
                .width
                .or(style.overflow.x.is_scroll_container().then_some(0.));
            let basis = style.flex_basis.maybe_resolve(Some(inner.0), calc);
            let (grow, shrink, auto) = (
                style.flex_grow,
                style.flex_shrink,
                style.flex_basis.is_auto(),
            );
            let scrolls = style.overflow.y.is_scroll_container();
            let edges = padding + border;
            let lines = taffy::Size {
                width: (padding.left + padding.right) + (border.left + border.right),
                height: edges.top + edges.bottom,
            };
            let spaced = match space {
                taffy::AvailableSpace::Definite(value) => taffy::AvailableSpace::Definite(
                    (inner.1.unwrap_or(value) - (margin.top + margin.bottom)).max(0.),
                ),
                other => other,
            };
            let known = taffy::Size {
                width: None,
                height: size
                    .height
                    .or(stretch.then(|| spaced.into_option()).flatten()),
            };
            let measure = |tree: &mut Self, width: taffy::AvailableSpace| {
                tree.compute(
                    child,
                    LayoutInput {
                        run_mode: RunMode::ComputeSize,
                        sizing_mode: taffy::SizingMode::ContentSize,
                        axis: taffy::RequestedAxis::Horizontal,
                        known_dimensions: known,
                        known_dimensions_are_definite: taffy::Size {
                            width: true,
                            height: true,
                        },
                        parent_size: taffy::Size {
                            width: None,
                            height: inner.1,
                        },
                        available_space: taffy::Size {
                            width,
                            height: spaced,
                        },
                        vertical_margins_are_collapsible: taffy::Line::FALSE,
                    },
                    None,
                )
                .size
                .width
            };
            let (basis, set) = match (basis, size.width) {
                (Some(basis), _) => (basis, true),
                (None, Some(width)) if auto => (width, false),
                _ => (measure(self, taffy::AvailableSpace::MaxContent), false),
            };
            let basis = basis.max(lines.width);
            let floor = match automatic {
                Some(floor) => floor,
                None if set => measure(self, taffy::AvailableSpace::MinContent)
                    .maybe_min(size.width)
                    .maybe_min(max.width)
                    .maybe_max(Some(edges.left + edges.right)),
                None => 0.,
            };
            let lowest = floor
                .maybe_max(min.width)
                .maybe_max(Some(edges.left + edges.right));
            let hypothetical = basis.maybe_clamp(Some(lowest), max.width);
            items.push(Item {
                node: child,
                order: order as u32,
                margin,
                padding,
                border,
                stretch,
                center: align == AlignItemsKeyword::Center,
                cross: size.height.map(|height| height.max(lines.height)),
                sides: lines.height,
                scrolls,
                grow,
                basis,
                floor,
                limit: max.width,
                hypothetical,
                target: hypothetical,
                frozen: grow == 0. && shrink == 0.,
                violation: 0.,
            });
        }
        let gaps = if items.len() <= 1 {
            0.
        } else {
            gap * (items.len() - 1) as f32
        };
        let used = gaps
            + items
                .iter()
                .map(|item| item.hypothetical + (item.margin.left + item.margin.right))
                .sum::<f32>();
        if used > inner.0 {
            return None;
        }
        if used < inner.0 {
            for item in &mut items {
                item.frozen |= item.basis > item.hypothetical;
            }
            let taken = |items: &[Item]| {
                gaps + items
                    .iter()
                    .map(|item| match item.frozen {
                        true => item.target + (item.margin.left + item.margin.right),
                        false => item.basis + (item.margin.left + item.margin.right),
                    })
                    .sum::<f32>()
            };
            let initial = inner.0 - taken(&items);
            while !items.iter().all(|item| item.frozen) {
                let used = taken(&items);
                let grows = items
                    .iter()
                    .filter(|item| !item.frozen)
                    .fold(0., |sum, item| sum + item.grow);
                let free = match grows < 1. {
                    true => (initial * grows - gaps).min(inner.0 - used),
                    false => inner.0 - used,
                };
                if free.is_normal() && grows > 0. {
                    for item in items.iter_mut().filter(|item| !item.frozen) {
                        item.target = item.basis + free * (item.grow / grows);
                    }
                }
                let mut total = 0.;
                for item in items.iter_mut().filter(|item| !item.frozen) {
                    let clamped = item
                        .target
                        .maybe_clamp(Some(item.floor), item.limit)
                        .max(0.);
                    item.violation = clamped - item.target;
                    item.target = clamped;
                    total += item.violation;
                }
                for item in items.iter_mut().filter(|item| !item.frozen) {
                    item.frozen = match total {
                        total if total > 0. => item.violation > 0.,
                        total if total < 0. => item.violation < 0.,
                        _ => true,
                    };
                }
            }
        }
        let outer = inner.0 + main;
        let parent = taffy::Size {
            width: Some(inner.0),
            height: inner.1,
        };
        let stretched =
            fixed.map(|height| (height.maybe_clamp(min.height, max.height) - cross).max(0.));
        let mut outputs = Vec::with_capacity(items.len());
        for item in &items {
            let margins = item.margin.top + item.margin.bottom;
            let available = space
                .map_definite_value(|value| (value - margins).max(0.))
                .maybe_max(item.sides);
            let height = match (item.stretch, item.cross) {
                (true, _) => stretched.map(|line| (line - margins).max(0.)),
                (false, height) => height,
            };
            let output = self.compute(
                item.node,
                LayoutInput {
                    run_mode: RunMode::PerformLayout,
                    sizing_mode: taffy::SizingMode::ContentSize,
                    axis: taffy::RequestedAxis::Both,
                    known_dimensions: taffy::Size {
                        width: Some(item.target),
                        height,
                    },
                    known_dimensions_are_definite: taffy::Size {
                        width: true,
                        height: height.is_some(),
                    },
                    parent_size: parent,
                    available_space: taffy::Size {
                        width: taffy::AvailableSpace::Definite(
                            (outer - (item.margin.left + item.margin.right)).max(0.),
                        ),
                        height: available,
                    },
                    vertical_margins_are_collapsible: taffy::Line::FALSE,
                },
                None,
            );
            if output.size.height.max(item.sides) != output.size.height {
                return None;
            }
            outputs.push(output);
        }
        let line = match stretched {
            Some(line) => line,
            None => items
                .iter()
                .zip(&outputs)
                .map(|(item, output)| output.size.height + (item.margin.top + item.margin.bottom))
                .fold(0., f32::max)
                .maybe_clamp(min.height.maybe_sub(cross), max.height.maybe_sub(cross)),
        };
        let height = fixed
            .unwrap_or(line + 0. + cross)
            .maybe_clamp(min.height, max.height)
            .max(cross);
        let (mut total, mut first) = (inset.left, None);
        for (number, (item, output)) in items.iter().zip(&outputs).enumerate() {
            let offset = if number == 0 { 0. } else { gap };
            let free = line - (output.size.height + (item.margin.top + item.margin.bottom));
            let top = inset.top + if item.center { free / 2. } else { 0. } + item.margin.top;
            let mut layout = Layout::with_order(item.order);
            layout.size = output.size;
            layout.location = taffy::Point {
                x: total + offset + item.margin.left,
                y: top,
            };
            layout.padding = item.padding;
            layout.border = item.border;
            layout.margin = item.margin;
            self.set_unrounded_layout(item.node, &layout);
            if number == 0 {
                let baseline = output.baselines.first.unwrap_or(output.size.height);
                first = Some(
                    top + if item.scrolls {
                        baseline.min(output.size.height).max(0.)
                    } else {
                        baseline
                    },
                );
            }
            total += offset + (item.margin.left + item.margin.right) + output.size.width;
        }
        let size = taffy::Size {
            width: outer,
            height,
        };
        for order in 0..self.slots[index].children.len() {
            let child = self.slots[index].children[order];
            if self.slots[usize::from(child)].taffy().display == taffy::Display::None {
                self.hide(child, order);
            }
        }
        Some(finish(size, border, contain, first))
    }

    // Preserve Taffy's sum order. Columns that can flex, shrink or move items must use Taffy.
    pub(super) fn stack(&mut self, node: NodeId, inputs: LayoutInput) -> Option<LayoutOutput> {
        use taffy::{AlignContentKeyword, CompactLength, MaybeMath, MaybeResolve, ResolveOrZero};
        let LayoutInput {
            known_dimensions: known,
            parent_size,
            available_space,
            run_mode,
            sizing_mode,
            axis,
            ..
        } = inputs;
        let joined = mem::take(&mut self.joined);
        let index = usize::from(node);
        if self.slots[index].floats
            && run_mode == RunMode::ComputeSize
            && axis == taffy::RequestedAxis::Horizontal
            && known.width.is_none()
            && self.slots[index].taffy().flex_direction == FlexDirection::Column
            && self.slots[index].style.inline != Inline::Lines
        {
            // Like Chrome, floats side by side add up in the max-content width, with the box after them.
            use taffy::ResolveOrZero;
            let calc = |value, basis| crate::style::resolve_calc(value, basis);
            let style = self.slots[index].taffy();
            let sides = (style.padding.resolve_or_zero(parent_size.width, calc)
                + style.border.resolve_or_zero(parent_size.width, calc))
            .horizontal_axis_sum();
            let most = available_space.width != taffy::AvailableSpace::MinContent;
            let (mut widest, mut run) = (0f32, [0f32; 2]);
            for order in 0..self.slots[index].children.len() {
                let child = self.slots[index].children[order];
                let slot = &self.slots[usize::from(child)];
                let (bits, style) = (slot.style.flow, slot.taffy());
                if style.display == taffy::Display::None
                    || style.position == taffy::Position::Absolute
                {
                    continue;
                }
                let margin = style
                    .margin
                    .resolve_or_zero(None, calc)
                    .horizontal_axis_sum();
                let probe = LayoutInput {
                    known_dimensions: taffy::Size::NONE,
                    parent_size: taffy::Size::NONE,
                    available_space: taffy::Size {
                        width: available_space.width,
                        height: taffy::AvailableSpace::MaxContent,
                    },
                    sizing_mode: taffy::SizingMode::InherentSize,
                    ..inputs
                };
                let size = self.compute(child, probe, None).size.width + margin;
                if bits & Flow::CLEAR_LEFT != 0 {
                    run[0] = 0.;
                }
                if bits & Flow::CLEAR_RIGHT != 0 {
                    run[1] = 0.;
                }
                if most && bits & Flow::FLOAT != 0 {
                    run[usize::from(bits & Flow::RIGHT != 0)] += size.max(0.);
                    widest = widest.max(run[0] + run[1]);
                } else {
                    widest = widest.max(size + if most { run[0] + run[1] } else { 0. });
                    run = [0.; 2];
                }
            }
            return Some(LayoutOutput::from_outer_size(taffy::Size {
                width: widest + sides,
                height: 0.,
            }));
        }
        let width = known.width?;
        let style = self.slots[index].taffy();
        // Floats lay out the same way in both passes, so the debug check compares only cached results.
        let floating = joined || self.slots[index].floats;
        let calc = |value, basis| crate::style::resolve_calc(value, basis);
        let starts = |align: Option<taffy::AlignItems>| {
            align.is_none_or(|align| {
                matches!(
                    align.keyword,
                    AlignItemsKeyword::Normal | AlignItemsKeyword::Stretch
                )
            })
        };
        // A box with floats must not fall back to flexbox, which stacks them as blocks.
        if style.flex_direction != FlexDirection::Column
            || style.flex_wrap != taffy::FlexWrap::NoWrap
            || style.gap.height.into_raw().tag() != CompactLength::LENGTH_TAG
            || self.slots[index].style.inline == Inline::Lines
            || !floating
                && (self.exact
                    || style.direction != taffy::Direction::Ltr
                    || style.box_sizing != taffy::BoxSizing::BorderBox
                    || style.aspect_ratio.is_some()
                    || !starts(Some(style.align_items)))
        {
            return None;
        }
        let flow = |child: &taffy::Style| {
            child.position != taffy::Position::Absolute && child.display != taffy::Display::None
        };
        let simple = self.slots[index].children.iter().all(|&child| {
            let floated = self.slots[usize::from(child)].style.flow & Flow::FLOAT != 0;
            let child = self.slots[usize::from(child)].taffy();
            match child.position {
                _ if floating && child.position != taffy::Position::Absolute => {
                    floated || child.box_sizing == taffy::BoxSizing::BorderBox
                }
                _ if child.display == taffy::Display::None => true,
                taffy::Position::Absolute => {
                    let align = child.align_self.unwrap_or(style.align_items);
                    align.safety == taffy::AlignmentSafety::Default
                        && matches!(
                            align.keyword,
                            AlignItemsKeyword::Normal
                                | AlignItemsKeyword::Stretch
                                | AlignItemsKeyword::FlexStart
                                | AlignItemsKeyword::Start
                        )
                        && matches!(
                            style.justify_content.keyword(),
                            AlignContentKeyword::Normal
                                | AlignContentKeyword::Start
                                | AlignContentKeyword::FlexStart
                                | AlignContentKeyword::Stretch
                                | AlignContentKeyword::SpaceBetween
                        )
                }
                _ => starts(child.align_self) && child.flex_basis.is_auto() && plain(child),
            }
        });
        if !simple {
            return None;
        }
        let padding = style.padding.resolve_or_zero(parent_size.width, calc);
        let border = style.border.resolve_or_zero(parent_size.width, calc);
        let inset = padding + border;
        let min = style.min_size.maybe_resolve(parent_size, calc);
        let max = style.max_size.maybe_resolve(parent_size, calc);
        let pb = padding.top + padding.bottom + (border.top + border.bottom);
        let definite = match (min.height, max.height) {
            (Some(min), Some(max)) if max <= min => Some(min),
            _ => None,
        };
        let styled = match sizing_mode {
            taffy::SizingMode::InherentSize => style
                .size
                .height
                .maybe_resolve(parent_size.height, calc)
                .maybe_clamp(min.height, max.height),
            taffy::SizingMode::ContentSize => None,
        };
        let ratio = style
            .aspect_ratio
            .filter(|_| floating && styled.is_none())
            .map(|ratio| width / ratio);
        let fixed = known.height.or(definite.or(styled).or(ratio).maybe_max(pb));
        let main = inset.top + inset.bottom;
        let cross = inset.left + inset.right;
        // The items of a flex column with a fixed height get definite heights, which this path doesn't give, so
        // percent heights in them need flexbox.
        if run_mode == RunMode::ComputeSize
            && (fixed.is_some() && !floating || axis == taffy::RequestedAxis::Horizontal)
            || width.maybe_clamp(min.width, max.width) != width && !floating
            || fixed.is_some()
                && !floating
                && !self.slots[index].style.block
                && self.slots[index].children.iter().any(|&child| {
                    (self.slots[usize::from(child)].children.iter())
                        .any(|&kid| relative_height(self.slots[usize::from(kid)].taffy()))
                })
        {
            return None;
        }
        let width = width.maybe_clamp(min.width, max.width);
        // A box beside floats is in the formatting context of its parent, and other boxes with floats make one.
        let context = (floating && !joined).then(|| {
            let outer = (
                mem::take(&mut self.floats),
                self.origin,
                self.ceiling,
                self.reach,
            );
            (self.ceiling, self.reach) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
            outer
        });
        let origin = if joined { self.origin } else { [0.; 2] };
        let edges = [origin[0] + inset.left, origin[0] + width - inset.right];
        let inner = width - cross;
        let line = (width - cross).max(0.);
        let gap = style.gap.height.into_raw().value();
        let (justify, contain, aligned) = (
            style.justify_content.keyword(),
            style.contain,
            style.align_items,
        );
        let perform = run_mode == RunMode::PerformLayout;
        let mut total = inset.top;
        let (mut items, mut hypothetical, mut contents, mut grows, mut shrinks) =
            (0, 0., 0., false, false);
        let mut first = None;
        // `plain` keeps percent heights out of the other columns.
        let tall = fixed
            .filter(|_| floating)
            .map(|height| (height - main).max(0.));
        // The offsets of a relatively positioned box.
        let shift = |offsets: taffy::Rect<taffy::LengthPercentageAuto>| {
            let side =
                |start: taffy::LengthPercentageAuto, end: taffy::LengthPercentageAuto, basis| {
                    let start: Option<f32> = start.maybe_resolve(basis, calc);
                    let end: Option<f32> = end.maybe_resolve(basis, calc);
                    start.or(end.map(|end| -end)).unwrap_or(0.)
                };
            [
                side(offsets.left, offsets.right, Some(inner)),
                side(offsets.top, offsets.bottom, tall),
            ]
        };
        for order in 0..self.slots[index].children.len() {
            let child = self.slots[index].children[order];
            let style = self.slots[usize::from(child)].taffy();
            if !flow(style) {
                continue;
            }
            let mut margin = style.margin.resolve_or_zero(Some(inner), calc);
            let (offsets, sized_by, self_align) = (style.inset, style.size.width, style.align_self);
            let [auto_left, auto_right] =
                [style.margin.left.is_auto(), style.margin.right.is_auto()];
            let padding = style.padding.resolve_or_zero(Some(inner), calc);
            let border = style.border.resolve_or_zero(Some(inner), calc);
            let sides = (padding + border).sum_axes();
            let [min, max] = [style.min_size, style.max_size]
                .map(|size| size.width.maybe_resolve(Some(inner), calc));
            let width = match style.size.width.maybe_resolve(Some(inner), calc) {
                None => (line - (margin.left + margin.right))
                    .max(0.)
                    .maybe_clamp(min, max),
                Some(width) => width.maybe_clamp(min, max).max(sides.width),
            };
            let (grow, shrink, styled) = (
                style.flex_grow,
                style.flex_shrink,
                style.size.height.into_raw(),
            );
            let input = |run_mode, axis, width: Option<f32>, space| LayoutInput {
                run_mode,
                sizing_mode: taffy::SizingMode::InherentSize,
                axis,
                known_dimensions: taffy::Size {
                    width,
                    height: None,
                },
                known_dimensions_are_definite: taffy::Size {
                    width: true,
                    height: true,
                },
                parent_size: taffy::Size {
                    width: Some(inner),
                    height: tall,
                },
                available_space: taffy::Size {
                    width: space,
                    height: taffy::AvailableSpace::MaxContent,
                },
                vertical_margins_are_collapsible: taffy::Line::FALSE,
            };
            let (run, requested) = if perform {
                (RunMode::PerformLayout, taffy::RequestedAxis::Both)
            } else {
                (RunMode::ComputeSize, taffy::RequestedAxis::Vertical)
            };
            let sized = |width: f32| {
                input(
                    run,
                    requested,
                    Some(width),
                    taffy::AvailableSpace::Definite(width),
                )
            };
            let bits = if floating {
                self.slots[usize::from(child)].style.flow
            } else {
                0
            };
            if bits & Flow::FLOAT != 0 {
                let width = match style.size.width.maybe_resolve(Some(inner), calc) {
                    Some(width) => width.maybe_clamp(min, max).max(sides.width),
                    None => {
                        let mut probe = |space| {
                            let probe = input(
                                RunMode::ComputeSize,
                                taffy::RequestedAxis::Horizontal,
                                None,
                                space,
                            );
                            self.compute(child, probe, None).size.width
                        };
                        let low = probe(taffy::AvailableSpace::MinContent);
                        let high = probe(taffy::AvailableSpace::MaxContent);
                        high.min(line - margin.left - margin.right)
                            .max(low)
                            .maybe_clamp(min, max)
                            .max(sides.width)
                    }
                };
                // A full layout also when the parent only measures, since some boxes measure to another height
                // than they lay out, and the floats of both passes must be the same.
                let output = self.compute(
                    child,
                    input(
                        RunMode::PerformLayout,
                        taffy::RequestedAxis::Both,
                        Some(width),
                        taffy::AvailableSpace::Definite(width),
                    ),
                    None,
                );
                let size = [
                    width + margin.left + margin.right,
                    output.size.height + margin.top + margin.bottom,
                ];
                let [x, y] = self.place(edges, size, origin[1] + total, bits);
                if perform {
                    let mut layout = Layout::with_order(order as u32);
                    layout.size = taffy::Size {
                        width,
                        height: output.size.height,
                    };
                    let [right, down] = shift(offsets);
                    layout.location = taffy::Point {
                        x: x - origin[0] + margin.left + right,
                        y: y - origin[1] + margin.top + down,
                    };
                    layout.padding = padding;
                    layout.border = border;
                    layout.margin = margin;
                    self.set_unrounded_layout(child, &layout);
                }
                continue;
            }
            let margins = margin.top + margin.bottom;
            let offset = if items == 0 { 0. } else { gap };
            let mut top = total + offset + margin.top;
            let mut left = inset.left + margin.left;
            let mut width = width;
            let mut drop = 0.;
            // `plain` keeps sizing keywords and other alignments out of the other columns.
            let align = self_align.unwrap_or(aligned).keyword;
            if floating
                && (sized_by.is_sizing_keyword()
                    || sized_by.is_auto()
                        && !matches!(
                            align,
                            AlignItemsKeyword::Normal | AlignItemsKeyword::Stretch
                        ))
            {
                let mut probe = |space| {
                    let probe = input(
                        RunMode::ComputeSize,
                        taffy::RequestedAxis::Horizontal,
                        None,
                        space,
                    );
                    self.compute(child, probe, None).size.width
                };
                let low = probe(taffy::AvailableSpace::MinContent);
                let high = probe(taffy::AvailableSpace::MaxContent);
                let room = line - margin.left - margin.right;
                width = high
                    .min(room)
                    .max(low)
                    .maybe_clamp(min, max)
                    .max(sides.width);
                left += match align {
                    AlignItemsKeyword::Center => (room - width) / 2.,
                    AlignItemsKeyword::End | AlignItemsKeyword::FlexEnd => room - width,
                    _ => 0.,
                };
            }
            let output = if floating {
                let slot = &self.slots[usize::from(child)];
                let (measured, lines) =
                    (slot.measure.is_some(), slot.style.inline == Inline::Lines);
                // A line box of one paragraph, maybe with absolute boxes like the inline-blocks of its lines.
                let mut lines_of = slot
                    .children
                    .iter()
                    .filter(|&&kid| flow(self.slots[usize::from(kid)].taffy()));
                let leaf = lines
                    .then(|| lines_of.next().filter(|_| lines_of.next().is_none()))
                    .flatten()
                    .copied()
                    .filter(|&only| {
                        let only = &self.slots[usize::from(only)];
                        only.measure.is_some() && only.style.flow & Flow::FLOAT == 0
                    });
                if bits & (Flow::CLEAR_LEFT | Flow::CLEAR_RIGHT) != 0 {
                    let clear = self.cleared(bits) - origin[1];
                    if clear > top {
                        (drop, top) = (clear - top, clear);
                    }
                }
                let y = origin[1] + top;
                self.floats.retain(|float| float[3] > y);
                let beside = !self.floats.is_empty();
                // `plain` keeps auto margins out of the other columns.
                if auto_left || auto_right {
                    let free = (line - width - margin.left - margin.right).max(0.);
                    let share = match (auto_left, auto_right) {
                        (true, true) => free / 2.,
                        (true, false) => free,
                        _ => 0.,
                    };
                    (margin.left, left) = (margin.left + share, left + share);
                }
                if bits & Flow::BLOCK != 0
                    && (slot.floats || beside)
                    && (measured || leaf.is_some() || !lines)
                {
                    if measured || leaf.is_some() {
                        let target = leaf.unwrap_or(child);
                        let own = self.slots[usize::from(target)].taffy();
                        let [own_padding, own_border] = [own.padding, own.border]
                            .map(|edges| edges.resolve_or_zero(Some(inner), calc));
                        // A measured block is the target itself: `left` and `y` hold its margins already.
                        let (outer, own_margin) = match leaf {
                            Some(_) => (
                                padding + border,
                                own.margin.resolve_or_zero(Some(inner), calc),
                            ),
                            None => (taffy::Rect::zero(), taffy::Rect::zero()),
                        };
                        let within =
                            width - outer.left - outer.right - own_margin.left - own_margin.right;
                        let content = [
                            origin[0]
                                + left
                                + outer.left
                                + own_margin.left
                                + own_padding.left
                                + own_border.left,
                            y + outer.top + own_margin.top + own_padding.top + own_border.top,
                        ];
                        let span = within
                            - own_padding.left
                            - own_padding.right
                            - own_border.left
                            - own_border.right;
                        let scale = self.scale;
                        let floats: Vec<[f32; 4]> = self
                            .floats
                            .iter()
                            .filter(|float| float[3] > content[1])
                            .filter_map(|float| {
                                let sides = if float[4] == 0. {
                                    [(float[2] - content[0]).max(0.), 0.]
                                } else {
                                    [0., (content[0] + span - float[0]).max(0.)]
                                };
                                (sides != [0.; 2]).then(|| {
                                    [
                                        float[1] - content[1],
                                        float[3] - content[1],
                                        sides[0],
                                        sides[1],
                                    ]
                                    .map(|value| value / scale)
                                })
                            })
                            .collect();
                        let floats = (!floats.is_empty()).then(|| Rc::from(floats));
                        let system = self.window.text_system().clone();
                        self.active = true;
                        // Like a float, a full layout also when the parent only measures: lines beside floats can
                        // measure to another height than they lay out.
                        let full = input(
                            RunMode::PerformLayout,
                            taffy::RequestedAxis::Both,
                            Some(within),
                            taffy::AvailableSpace::Definite(within),
                        );
                        let output = system.beside(floats, || self.compute(target, full, None));
                        if leaf.is_some() {
                            if perform {
                                let mut layout = Layout::with_order(0);
                                layout.size = output.size;
                                layout.location = taffy::Point {
                                    x: outer.left + own_margin.left,
                                    y: outer.top + own_margin.top,
                                };
                                layout.padding = own_padding;
                                layout.border = own_border;
                                layout.margin = own_margin;
                                self.set_unrounded_layout(target, &layout);
                            }
                            // The line box gets the place and the baselines of its lines.
                            let row = &mut self.slots[usize::from(child)];
                            row.cache.clear();
                            row.uncached = true;
                            let shift = outer.top + own_margin.top;
                            row.baselines = Baselines {
                                first: output.baselines.first.map(|base| base + shift),
                                last: output.baselines.last.map(|base| base + shift),
                            };
                            let (contain, first) = (row.taffy().contain, row.baselines.first);
                            let size = taffy::Size {
                                width,
                                height: output.size.height
                                    + own_margin.top
                                    + own_margin.bottom
                                    + outer.top
                                    + outer.bottom,
                            };
                            let mut output = finish(size, border, contain, first);
                            output.baselines = self.slots[usize::from(child)].baselines;
                            if perform {
                                // Like `compute`, the line box places its absolute boxes.
                                let mut candidates = taffy::OofCandidates::new();
                                for (order, &kid) in
                                    self.slots[usize::from(child)].children.iter().enumerate()
                                {
                                    if self.slots[usize::from(kid)].taffy().position
                                        == taffy::Position::Absolute
                                    {
                                        let area = |start, end| taffy::AxisStaticPosition {
                                            area: taffy::Line { start, end },
                                            align: taffy::AxisStaticAlign::from_keyword(
                                                taffy::AxisStaticEdge::Start,
                                            ),
                                        };
                                        candidates.push(taffy::OofCandidate {
                                            node: kid,
                                            order: order as u32,
                                            position: taffy::Position::Absolute,
                                            static_position: taffy::Point {
                                                x: area(outer.left, size.width - outer.right),
                                                y: area(outer.top, size.height - outer.bottom),
                                            },
                                        });
                                    }
                                }
                                output.oof_candidates = candidates;
                                taffy::compute_oof_layout(self, child, &mut output);
                            }
                            output
                        } else {
                            output
                        }
                    } else {
                        self.origin = [origin[0] + left, y];
                        self.active = true;
                        let output = self.compute(child, sized(width), None);
                        self.origin = origin;
                        output
                    }
                } else if beside {
                    // A box that makes its own formatting context goes beside the floats, or below them where it
                    // doesn't fit.
                    let auto = sized_by.is_auto();
                    let mut y = y;
                    loop {
                        // The room must hold the box over its whole height, which depends on its width.
                        let (mut tall, mut placed) = (0f32, None);
                        for _ in 0..3 {
                            let (room, low) = self.beside(edges, y, tall);
                            let [from, to] = [
                                (edges[0] + margin.left).max(room[0]),
                                (edges[1] - margin.right).min(room[1]),
                            ];
                            if auto {
                                width = (to - from).maybe_clamp(min, max).max(sides.width);
                            }
                            left = from - origin[0];
                            let output = self.compute(child, sized(width), None);
                            let height = output.size.height;
                            placed = Some((output, width <= to - from + 0.01, low));
                            if height <= tall {
                                break;
                            }
                            tall = height;
                        }
                        let Some((output, fits, low)) = placed else {
                            unreachable!()
                        };
                        if fits || low == f32::INFINITY {
                            break output;
                        }
                        (drop, y) = (drop + low - y, low);
                        top = y - origin[1];
                    }
                } else {
                    self.compute(child, sized(width), None)
                }
            } else {
                self.compute(child, sized(width), None)
            };
            let height = output.size.height;
            // Taffy's content sizes take a fixed height even below the padding, which the box itself doesn't.
            if !floating && styled.tag() == CompactLength::LENGTH_TAG && styled.value() != height {
                return None;
            }
            if perform {
                let mut layout = Layout::with_order(order as u32);
                layout.size = taffy::Size { width, height };
                layout.location = taffy::Point { x: left, y: top };
                // `plain` keeps insets out of the other columns.
                if floating {
                    let [x, y] = shift(offsets);
                    (layout.location.x, layout.location.y) = (left + x, top + y);
                }
                layout.padding = padding;
                layout.border = border;
                layout.margin = margin;
                self.set_unrounded_layout(child, &layout);
                if items == 0 {
                    first = Some(top + output.baselines.first.unwrap_or(height));
                }
            }
            total += offset + margins + height + drop;
            hypothetical += height + margins + drop;
            contents += drop;
            // Like Taffy, an item adds its margins to the content size, also negative ones.
            contents += match available_space.height {
                taffy::AvailableSpace::Definite(_) => (height + margins).max(sides.height),
                _ => height + margins,
            };
            grows |= grow > 0.;
            shrinks |= shrink > 0.;
            items += 1;
        }
        if let Some((floats, outer, ceiling, low)) = context {
            // Like a formatting context root, the box holds its floats.
            let reach = self.reach - inset.top - contents;
            if reach > 0. {
                contents += reach;
                hypothetical += reach;
            }
            (self.floats, self.origin, self.ceiling, self.reach) = (floats, outer, ceiling, low);
        }
        let gaps = if items <= 1 {
            0.
        } else {
            gap * (items - 1) as f32
        };
        let (outer, inner_main) = match fixed {
            Some(height) => {
                let inner = height - main;
                (inner + main, inner)
            }
            None => {
                let content = match available_space.height {
                    taffy::AvailableSpace::Definite(_) => contents + gaps + main,
                    _ => 0f32.max(contents + gaps) + main,
                };
                let outer = content.maybe_clamp(min.height, max.height).max(main);
                (outer, (outer - main).max(0.))
            }
        };
        let free = inner_main - (gaps + hypothetical);
        let packed = matches!(
            justify,
            AlignContentKeyword::Normal
                | AlignContentKeyword::Start
                | AlignContentKeyword::FlexStart
                | AlignContentKeyword::Stretch
        );
        if free != 0. && (!packed || free > 0. && grows || free < 0. && shrinks) && !floating {
            return None;
        }
        if !perform {
            let width = if axis == taffy::RequestedAxis::Vertical {
                0.
            } else {
                width.max(cross)
            };
            return Some(LayoutOutput::from_outer_size(taffy::Size {
                width,
                height: outer,
            }));
        }
        let size = taffy::Size {
            width: width.max(cross),
            height: outer,
        };
        // Floats take this path whatever the alignment, so it aligns the content after placing it.
        let lift = match justify {
            AlignContentKeyword::Center
            | AlignContentKeyword::SpaceAround
            | AlignContentKeyword::SpaceEvenly => 0.5,
            AlignContentKeyword::End | AlignContentKeyword::FlexEnd => 1.,
            _ => 0.,
        };
        if lift > 0. {
            let (mut above, mut below) = (f32::INFINITY, f32::NEG_INFINITY);
            for &child in &self.slots[index].children {
                let slot = &self.slots[usize::from(child)];
                if flow(slot.taffy()) {
                    let layout = &slot.layout;
                    above = above.min(layout.location.y - layout.margin.top);
                    below =
                        below.max(layout.location.y + layout.size.height + layout.margin.bottom);
                }
            }
            let mut free = size.height - inset.top - inset.bottom - (below - above);
            if self.slots[index].style.safe {
                free = free.max(0.);
            }
            let shift = inset.top - above + free * lift;
            if above.is_finite() && shift != 0. {
                for at in 0..self.slots[index].children.len() {
                    let child = usize::from(self.slots[index].children[at]);
                    if flow(self.slots[child].taffy()) {
                        self.slots[child].layout.location.y += shift;
                    }
                }
            }
        }
        let mut candidates = taffy::OofCandidates::new();
        for order in 0..self.slots[index].children.len() {
            let child = self.slots[index].children[order];
            let style = self.slots[usize::from(child)].taffy();
            if style.display == taffy::Display::None {
                self.hide(child, order);
            } else if style.position == taffy::Position::Absolute {
                let area = |start, end| taffy::AxisStaticPosition {
                    area: taffy::Line { start, end },
                    align: taffy::AxisStaticAlign::from_keyword(taffy::AxisStaticEdge::Start),
                };
                candidates.push(taffy::OofCandidate {
                    node: child,
                    order: order as u32,
                    position: style.position,
                    static_position: taffy::Point {
                        x: area(inset.left, size.width - inset.right),
                        y: area(inset.top, size.height - inset.bottom),
                    },
                });
            }
        }
        let mut output = finish(size, border, contain, first);
        output.oof_candidates = candidates;
        Some(output)
    }

    /// In a column, a baseline runs along the cross axis, so like Chrome the items that align on their first or last
    /// baselines share their line-left edge, as a group at the start or the end side of the column.
    pub(super) fn group_baselines(&mut self, index: usize, parent_width: Option<f32>, width: f32) {
        use taffy::ResolveOrZero;
        let calc = |value, basis| crate::style::resolve_calc(value, basis);
        let style = self.slots[index].taffy();
        let rtl = style.direction == taffy::Direction::Rtl;
        let inset = style.padding.resolve_or_zero(parent_width, calc)
            + style.border.resolve_or_zero(parent_width, calc);
        let [left, right] = [inset.left, width - inset.right];
        let align = self.slots[index].style.align_items;
        for last in [false, true] {
            let kind = if last {
                AlignItems::LastBaseline
            } else {
                AlignItems::Baseline
            };
            let members: Vec<usize> = (self.slots[index].children.iter())
                .map(|&child| usize::from(child))
                .filter(|&child| {
                    let slot = &self.slots[child];
                    slot.taffy().position != taffy::Position::Absolute
                        && slot.taffy().display != taffy::Display::None
                        && slot.style.align_self.or(align) == Some(kind)
                })
                .collect();
            if members.is_empty() {
                continue;
            }
            let wide = members.iter().fold(0f32, |wide, &child| {
                let layout = &self.slots[child].layout;
                wide.max(layout.size.width + layout.margin.left + layout.margin.right)
            });
            let x = if rtl != last { right - wide } else { left };
            for child in members {
                let slot = &mut self.slots[child];
                let at = x + slot.layout.margin.left;
                if slot.layout.location.x != at {
                    slot.layout.location.x = at;
                    if !self.exact {
                        changed(self.slots, child);
                    }
                }
            }
        }
    }

    /// Places a float with its margin box `size` at or below `y`, between `edges`, like CSS 2 9.5.1.
    fn place(&mut self, edges: [f32; 2], size: [f32; 2], mut y: f32, bits: u8) -> [f32; 2] {
        let right = bits & Flow::RIGHT != 0;
        // The flow only moves down, so floats that ended above it can't touch this one or the boxes after it.
        self.floats.retain(|float| float[3] > y);
        y = y.max(self.ceiling).max(self.cleared(bits));
        loop {
            let (room, low) = self.beside(edges, y, size[1]);
            if room[1] - room[0] >= size[0] - 0.01 || low == f32::INFINITY {
                let x = if right { room[1] - size[0] } else { room[0] };
                self.floats
                    .push([x, y, x + size[0], y + size[1], f32::from(u8::from(right))]);
                (self.ceiling, self.reach) = (self.ceiling.max(y), self.reach.max(y + size[1]));
                return [x, y];
            }
            y = low;
        }
    }

    /// The room between the floats beside `y..y + height`, and the first bottom of those floats.
    fn beside(&self, edges: [f32; 2], y: f32, height: f32) -> ([f32; 2], f32) {
        self.floats
            .iter()
            .fold((edges, f32::INFINITY), |(room, low), float| {
                if float[1] < y + height.max(0.01) && float[3] > y {
                    let room = match float[4] == 0. {
                        true => [room[0].max(float[2]), room[1]],
                        false => [room[0], room[1].min(float[0])],
                    };
                    (room, low.min(float[3]))
                } else {
                    (room, low)
                }
            })
    }

    /// The bottom of the floats that `bits` clears.
    fn cleared(&self, bits: u8) -> f32 {
        self.floats.iter().fold(f32::NEG_INFINITY, |bottom, float| {
            let side = if float[4] == 0. {
                Flow::CLEAR_LEFT
            } else {
                Flow::CLEAR_RIGHT
            };
            if bits & side != 0 {
                bottom.max(float[3])
            } else {
                bottom
            }
        })
    }

    fn hide(&mut self, child: NodeId, order: usize) {
        self.set_unrounded_layout(child, &Layout::with_order(order as u32));
        self.compute(
            child,
            LayoutInput {
                run_mode: RunMode::PerformLayout,
                sizing_mode: taffy::SizingMode::ContentSize,
                axis: taffy::RequestedAxis::Both,
                known_dimensions: taffy::Size::NONE,
                known_dimensions_are_definite: taffy::Size {
                    width: true,
                    height: true,
                },
                parent_size: taffy::Size::NONE,
                available_space: taffy::Size {
                    width: taffy::AvailableSpace::MaxContent,
                    height: taffy::AvailableSpace::MaxContent,
                },
                vertical_margins_are_collapsible: taffy::Line::FALSE,
            },
            None,
        );
    }

    // Taffy puts a box on the next line when it does not fit with the collapsible space that it joins, but CSS hangs
    // the space at the end of a line. Such a box keeps its line through a right margin that is smaller by its
    // overflow. This needs the hypothetical sizes of Taffy's line breaks, so it follows Taffy's flex algorithm.
    pub(super) fn lines(
        &mut self,
        node: NodeId,
        inputs: LayoutInput,
        start: usize,
    ) -> LayoutOutput {
        use taffy::{AvailableSpace, MaybeMath, MaybeResolve, ResolveOrZero};
        let measure = |known_dimensions, parent_size, available_space| LayoutInput {
            known_dimensions,
            known_dimensions_are_definite: taffy::Size {
                width: true,
                height: true,
            },
            parent_size,
            available_space,
            sizing_mode: taffy::SizingMode::ContentSize,
            axis: taffy::RequestedAxis::Horizontal,
            run_mode: RunMode::ComputeSize,
            vertical_margins_are_collapsible: taffy::Line::FALSE,
        };
        self.hung.truncate(start);
        let index = usize::from(node);
        let slot = &self.slots[index];
        let style = slot.taffy();
        if slot.style.inline != Inline::Lines
            || style.flex_wrap != taffy::FlexWrap::Wrap
            || !slot
                .children
                .iter()
                .any(|&child| hanging(self.slots, child).is_some())
        {
            return taffy::compute_flexbox_layout(self, node, inputs);
        }
        let LayoutInput {
            known_dimensions,
            parent_size,
            available_space,
            ..
        } = inputs;
        let calc = |value, basis| crate::style::resolve_calc(value, basis);
        let box_adjust = |style: &taffy::Style, sides: taffy::Size<f32>| match style.box_sizing {
            taffy::BoxSizing::ContentBox => sides,
            taffy::BoxSizing::BorderBox => taffy::Size::ZERO,
        };
        let sides = (style.padding.resolve_or_zero(parent_size.width, calc)
            + style.border.resolve_or_zero(parent_size.width, calc))
        .sum_axes();
        let ratio = style.aspect_ratio;
        let adjust = box_adjust(style, sides);
        let min = style
            .min_size
            .maybe_resolve(parent_size, calc)
            .maybe_apply_aspect_ratio(ratio)
            .maybe_add(adjust);
        let max = style
            .max_size
            .maybe_resolve(parent_size, calc)
            .maybe_apply_aspect_ratio(ratio)
            .maybe_add(adjust);
        let styled = match inputs.sizing_mode {
            taffy::SizingMode::InherentSize => style
                .size
                .maybe_resolve(parent_size, calc)
                .maybe_apply_aspect_ratio(ratio)
                .maybe_add(adjust)
                .maybe_clamp(min, max),
            taffy::SizingMode::ContentSize => taffy::Size::NONE,
        };
        let known = known_dimensions.or(min
            .zip_map(max, |min, max| match (min, max) {
                (Some(min), Some(max)) if max <= min => Some(min),
                _ => None,
            })
            .or(styled)
            .maybe_max(sides));
        let gutter = |overflow| match overflow {
            taffy::Overflow::Scroll => style.scrollbar_width,
            _ => 0.,
        };
        let inset = taffy::Size {
            width: sides.width + gutter(style.overflow.y),
            height: sides.height + gutter(style.overflow.x),
        };
        let definite =
            inputs.known_dimensions_are_definite.width || known_dimensions.width.is_none();
        let space = |known: Option<f32>, available: AvailableSpace, inset: f32| match known {
            Some(known) => AvailableSpace::Definite((known - inset).max(0.)),
            None => available.maybe_sub(inset).maybe_max(0.),
        };
        let height = space(known.height, available_space.height, inset.height);
        let width = match max.width {
            Some(max) => Some(
                space(known.width, available_space.width, inset.width)
                    .into_option()
                    .map_or(max, |width| match known.width {
                        Some(_) => width,
                        None => width.min(max),
                    })
                    .maybe_max(min.width),
            ),
            None => space(known.width, available_space.width, inset.width).into_option(),
        };
        let (Some(width), true) = (width, definite) else {
            return taffy::compute_flexbox_layout(self, node, inputs);
        };
        if inputs.run_mode == RunMode::ComputeSize
            && known.width.is_some()
            && (known.height.is_some() || inputs.axis == taffy::RequestedAxis::Horizontal)
        {
            return taffy::compute_flexbox_layout(self, node, inputs);
        }
        let inner = known.maybe_sub(inset);
        let percent = taffy::Size {
            width: inner.width.filter(|_| definite),
            ..inner
        };
        let gap = style
            .gap
            .width
            .resolve_or_zero(inner.width.or(Some(0.)), calc);
        let align_items = style.align_items;
        let mut used: Option<f32> = None;
        for at in 0..self.slots[index].children.len() {
            let child = self.slots[index].children[at];
            let item = self.slots[usize::from(child)].taffy();
            if item.position == taffy::Position::Absolute || item.display == taffy::Display::None {
                continue;
            }
            let ratio = item.aspect_ratio;
            let sides = (item.padding.resolve_or_zero(inner.width, calc)
                + item.border.resolve_or_zero(inner.width, calc))
            .sum_axes();
            let adjust = box_adjust(item, sides);
            let size = item
                .size
                .maybe_resolve(percent, calc)
                .maybe_apply_aspect_ratio(ratio)
                .maybe_add(adjust);
            let min = item.min_size.maybe_resolve(percent, calc).maybe_add(adjust);
            let max = item.max_size.maybe_resolve(percent, calc).maybe_add(adjust);
            let [low, high] = [min, max].map(|size| size.maybe_apply_aspect_ratio(ratio));
            let margin = item.margin.resolve_or_zero(inner.width, calc);
            // Sizing keywords and flex bases keep Taffy's line breaks.
            if !item.flex_basis.is_auto() || size.width.is_none() && !item.size.width.is_auto() {
                self.hung.truncate(start);
                return taffy::compute_flexbox_layout(self, node, inputs);
            }
            let basis = match size.width {
                Some(width) => width,
                None => {
                    let cross = match height {
                        AvailableSpace::Definite(height) => AvailableSpace::Definite(
                            (inner.height.unwrap_or(height) - margin.top - margin.bottom)
                                .max(0.)
                                .maybe_clamp(low.height, high.height),
                        ),
                        AvailableSpace::MinContent => low
                            .height
                            .map_or(AvailableSpace::MinContent, AvailableSpace::Definite),
                        AvailableSpace::MaxContent => high
                            .height
                            .map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
                    };
                    let stretch = matches!(
                        item.align_self.unwrap_or(align_items).keyword,
                        AlignItemsKeyword::Normal | AlignItemsKeyword::Stretch
                    ) && !item.margin.top.is_auto()
                        && !item.margin.bottom.is_auto();
                    let known = size.height.maybe_clamp(low.height, high.height);
                    let known = known.or(cross.into_option().filter(|_| stretch));
                    let input = measure(
                        taffy::Size {
                            width: None,
                            height: known,
                        },
                        taffy::Size {
                            width: None,
                            height: inner.height,
                        },
                        taffy::Size {
                            width: AvailableSpace::MaxContent,
                            height: cross,
                        },
                    );
                    self.compute(child, input, None).size.width
                }
            };
            // Taffy also clamps by the automatic minimum size, which is never larger than this basis.
            let outer = basis.max(sides.width).maybe_clamp(
                Some(
                    min.width
                        .unwrap_or(0.)
                        .maybe_max(low.width)
                        .max(sides.width),
                ),
                high.width,
            ) + (margin.left + margin.right);
            let Some(line) = used else {
                used = Some(outer);
                continue;
            };
            // In Taffy's order of sums, so both agree on each break.
            let line = line + (outer + gap);
            used = Some(line);
            if line <= width {
                continue;
            }
            used = Some(outer);
            let Some(space) = hanging(self.slots, child) else {
                continue;
            };
            let input = measure(
                taffy::Size::NONE,
                taffy::Size::NONE,
                taffy::Size {
                    width: AvailableSpace::MaxContent,
                    height: AvailableSpace::MaxContent,
                },
            );
            let hang = self.compute(space, input, None).size.width;
            if line - hang <= width {
                // A little more, so Taffy's sum with the new margin can't round above the line.
                let overflow = line - width + 1e-3;
                let mut style = self.slots[usize::from(child)].taffy().clone();
                style.margin.right = taffy::LengthPercentageAuto::length(margin.right - overflow);
                self.hung.push((child, overflow, style));
                used = Some(width);
            }
        }
        taffy::compute_flexbox_layout(self, node, inputs)
    }

    pub(super) fn measure(&mut self, index: usize, inputs: LayoutInput) -> LayoutOutput {
        let scale = self.scale;
        let window = &mut *self.window;
        let banded = window.text_system().floats().is_some();
        let Slot {
            stretched,
            measure,
            style,
            text,
            measured: kept,
            banded: was,
            ..
        } = &mut self.slots[index];
        let taffy = stretched.as_deref().unwrap_or(&style.taffy);
        use taffy::ResolveOrZero;
        let calc = |value, basis| crate::style::resolve_calc(value, basis);
        let width = inputs.parent_size.width;
        let padding = taffy.padding.resolve_or_zero(width, calc);
        let border = taffy.border.resolve_or_zero(width, calc);
        // Taffy passes the known border box size, but measure functions take the content box, like `available`.
        let inset = (padding + border).sum_axes();
        let mut baselines = None;
        let mut content = None;
        let mut output = taffy::compute_leaf_layout(
            inputs,
            taffy,
            |_, _| 0.,
            |known, available| {
                let Some(measure) = measure else {
                    return taffy::Size::ZERO;
                };
                let known = size(
                    known
                        .width
                        .map(|value| Pixels((value - inset.width).max(0.) / scale)),
                    known
                        .height
                        .map(|value| Pixels((value - inset.height).max(0.) / scale)),
                );
                let available = size(
                    AvailableSpace::unscaled(available.width, scale),
                    AvailableSpace::unscaled(available.height, scale),
                );
                let (measured, lines) = measure(text, known, available, window);
                baselines = lines;
                let size = taffy::Size {
                    width: (measured.width.0.max(0.) * scale * 64.).ceil() / 64.,
                    height: (measured.height.0.max(0.) * scale * 64.).ceil() / 64.,
                };
                content = Some(size);
                size
            },
        );
        let perform = inputs.run_mode == RunMode::PerformLayout;
        let remeasured =
            perform && content.is_some_and(|content| mem::replace(kept, content) != content);
        // Lines beside floats can change without a new size, and the place of each line comes from prepaint.
        let rebanded = perform && content.is_some() && (mem::replace(was, banded) || banded);
        if let Some([first, last]) = baselines.filter(|_| style.baseline != Baseline::None) {
            let top = padding.top + border.top;
            output.baselines = Baselines {
                first: Some(first.0 * scale + top),
                last: Some(last.0 * scale + top),
            };
        }
        // A measure that shaped the text again drops the place of its lines, also when layout takes the final size
        // from its cache, so prepaint must place the box again.
        let unplaced = content.is_some()
            && (self.slots[index].state.as_ref())
                .and_then(|state| state.downcast_ref::<TextState>())
                .is_some_and(TextState::unplaced);
        if rebanded || unplaced {
            changed(self.slots, index);
        } else if remeasured {
            unfar(self.slots, index);
        }
        output
    }

    pub(super) fn baselines(&self, index: usize, output: &LayoutOutput) -> Baselines {
        let slot = &self.slots[index];
        let style = slot.taffy();
        let baseline = slot.style.baseline;
        let first = output.baselines.first;
        if matches!(baseline, Baseline::Bottom | Baseline::Middle) {
            let bottom = Some(if baseline == Baseline::Middle {
                output.size.height / 2.
            } else {
                output.size.height
            });
            return Baselines {
                first: bottom,
                last: bottom,
            };
        }
        if slot.children.is_empty() {
            let Baselines { first, last } = output.baselines;
            return Baselines {
                first: if baseline == Baseline::Last {
                    last
                } else {
                    first
                },
                last,
            };
        }
        let children = slot
            .children
            .iter()
            .map(|&child| &self.slots[usize::from(child)])
            .filter(|child| {
                child.taffy().position != taffy::Position::Absolute
                    && child.taffy().display != taffy::Display::None
            });
        let at = |child: &Slot, value: f32| child.layout.location.y + value;
        let source = slot
            .children
            .iter()
            .map(|&child| &self.slots[usize::from(child)])
            .find(|child| child.style.baseline_source)
            .map(|child| {
                at(
                    child,
                    child.baselines.first.unwrap_or(child.layout.size.height),
                )
            });
        let row = style.display == taffy::Display::Flex
            && matches!(
                style.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            );
        let (first, last) = match baseline {
            _ if style.contain.suppresses_baseline() => (None, None),
            _ if source.is_some() => (source, source),
            Baseline::First => (first, first),
            Baseline::FirstOnly => (first, None),
            _ if row => {
                let last = children
                    .rev()
                    .find(|child| {
                        child
                            .taffy()
                            .align_self
                            .unwrap_or(style.align_items)
                            .keyword
                            == AlignItemsKeyword::Baseline
                    })
                    .map(|child| {
                        let baselines = child.baselines;
                        at(
                            child,
                            baselines
                                .last
                                .or(baselines.first)
                                .unwrap_or(child.layout.size.height),
                        )
                    })
                    .or(first);
                (first, last)
            }
            _ => (
                children
                    .clone()
                    .find_map(|child| Some(at(child, child.baselines.first?))),
                children
                    .rev()
                    .find_map(|child| Some(at(child, child.baselines.last?))),
            ),
        };
        Baselines {
            first: if baseline == Baseline::Last {
                last
            } else {
                first
            },
            last,
        }
    }
}
