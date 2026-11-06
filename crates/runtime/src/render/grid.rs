use crate::style::Prop;
use crate::{
    dom::Document,
    style::{self, ComputedStyles, Lengths},
};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct Measurements {
    pub(crate) sizes: rustc_hash::FxHashMap<(usize, Option<u32>, Option<u32>), [f32; 4]>,
    /// Fields measure their text in device pixels, like Chrome.
    pub(crate) pixel_ratio: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Grid {
    pub columns: Vec<f32>,
    pub rows: Vec<f32>,
    /// Each item, its cell, its margin box, and whether it aligns baselines.
    pub cells: Vec<(usize, [usize; 4], [f32; 4], bool)>,
    pub gap: [f32; 2],
    /// In an item's slot: the item aligns baselines, which the grid reads from its last layout.
    pub baseline: bool,
    /// The item that gives the grid its first baseline, which layout reads from that item. In an item's slot:
    /// that item.
    pub first_item: Option<usize>,
    /// The grid's first baseline from its content box top, from the first item's last layout. Only a culled
    /// first item, which did not change, uses it.
    pub first_baseline: Option<f32>,
}

/// The first and last baselines of grid items from their last layout, by node.
pub(crate) type Baselines = rustc_hash::FxHashMap<usize, [Option<f32>; 2]>;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sizing {
    Fixed(f32),
    MinContent,
    MaxContent,
    Auto,
    Flex(f32),
    FitContent(f32),
}

type Track = (Sizing, Sizing);

type Lines = Vec<(String, usize)>;

fn tracks(
    value: &str,
    units: &Lengths,
    base: Option<f32>,
    gap: f32,
) -> (Vec<Track>, Lines, Option<(usize, usize)>) {
    let sizing = |value: &str| match value {
        "auto" => Some(Sizing::Auto),
        "min-content" => Some(Sizing::MinContent),
        "max-content" => Some(Sizing::MaxContent),
        _ if value.contains('%') && base.is_none() => Some(Sizing::Auto),
        _ => value
            .strip_suffix("fr")
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite() && *v >= 0.)
            .map(Sizing::Flex)
            .or_else(|| {
                units
                    .resolve(value, base.unwrap_or(0.))
                    .map(|v| Sizing::Fixed(v.max(0.)))
            }),
    };
    let mut list = Vec::new();
    let mut names = Vec::new();
    let mut auto = None;
    for part in style::components(value, ' ') {
        let (function, arguments) = part
            .strip_suffix(')')
            .and_then(|v| v.split_once('('))
            .map_or((part, Vec::new()), |(name, body)| {
                (name, style::components(body, ','))
            });
        match (function, arguments.as_slice()) {
            _ if part.starts_with('[') => names.extend(
                part.trim_matches(['[', ']'])
                    .split_whitespace()
                    .map(|name| (name.to_owned(), list.len())),
            ),
            ("repeat", [count, body]) => {
                let (inner, lines, _) = tracks(body, units, base, gap);
                if inner.is_empty() {
                    continue;
                }
                if matches!(*count, "auto-fill" | "auto-fit") {
                    auto = auto.or(Some((
                        list.len(),
                        names.len(),
                        inner,
                        lines,
                        *count == "auto-fit",
                    )));
                    continue;
                }
                for _ in 0..count.parse::<usize>().unwrap_or(0).min(4096) {
                    if list.len() + inner.len() > 4096 {
                        break;
                    }
                    names.extend(
                        lines
                            .iter()
                            .map(|(name, line)| (name.clone(), list.len() + line)),
                    );
                    list.extend_from_slice(&inner);
                }
            }
            ("minmax", [min, max]) => {
                let min = match sizing(min) {
                    Some(Sizing::Flex(_)) | None => Sizing::Auto,
                    Some(min) => min,
                };
                list.push(match (min, sizing(max).unwrap_or(Sizing::Auto)) {
                    (Sizing::Fixed(min), Sizing::Fixed(max)) if max < min => {
                        (Sizing::Fixed(min), Sizing::Fixed(min))
                    }
                    (min, max) => (min, max),
                });
            }
            ("fit-content", [limit]) => list.push((
                Sizing::Auto,
                match sizing(limit) {
                    Some(Sizing::Fixed(limit)) => Sizing::FitContent(limit),
                    _ => Sizing::MaxContent,
                },
            )),
            ("none" | "subgrid", _) => {}
            _ => {
                let size = sizing(part).unwrap_or(Sizing::Auto);
                list.push(match size {
                    Sizing::Flex(_) => (Sizing::Auto, size),
                    _ => (size, size),
                });
            }
        }
        if list.len() >= 4096 {
            list.truncate(4096);
            break;
        }
    }
    let Some((index, mark, inner, lines, fit)) = auto else {
        return (list, names, None);
    };
    let size = |&(min, max): &Track| match (min, max) {
        (Sizing::Fixed(min), Sizing::Fixed(max)) => max.max(min),
        (_, Sizing::Fixed(size)) | (Sizing::Fixed(size), _) => size,
        _ => 0.,
    };
    let one = inner.iter().map(size).sum::<f32>() + inner.len() as f32 * gap;
    let rest = list.iter().map(size).sum::<f32>() + list.len() as f32 * gap;
    let count = base
        .filter(|_| one > 0.)
        .map_or(1., |space| ((space + gap - rest) / one).floor())
        .max(1.) as usize;
    let count = count.min(4096usize.saturating_sub(list.len()) / inner.len());
    let tail = list.split_off(index);
    for name in &mut names[mark..] {
        name.1 += inner.len() * count;
    }
    for repeat in 0..count {
        names.extend(
            lines
                .iter()
                .map(|(name, line)| (name.clone(), index + repeat * inner.len() + line)),
        );
        list.extend_from_slice(&inner);
    }
    list.extend(tail);
    (list, names, fit.then_some((index, inner.len() * count)))
}

fn placement(
    start: &str,
    end: &str,
    names: &[(String, usize)],
    explicit: usize,
) -> (Option<usize>, usize) {
    let lines = |name: &str| {
        let mut found: Vec<_> = names
            .iter()
            .filter(|(line, _)| line == name)
            .map(|(_, index)| *index)
            .collect();
        found.sort_unstable();
        found.dedup();
        found
    };
    let side = |value, suffix| {
        let (mut span, mut number, mut name) = (false, None, None);
        for word in str::split_whitespace(value) {
            match word.parse::<isize>() {
                Ok(value) => number = Some(value),
                _ if word == "span" => span = true,
                _ if word != "auto" => name = Some(word),
                _ => {}
            }
        }
        if span {
            return (None, number.unwrap_or(1).clamp(1, 4096) as usize, name);
        }
        let line = match (number, name) {
            (Some(0), _) | (None, None) => None,
            (Some(n), None) if n > 0 => Some(n - 1),
            (Some(n), None) => Some(explicit as isize + 1 + n),
            (None, Some(name)) => lines(&format!("{name}-{suffix}"))
                .first()
                .or(lines(name).first())
                .map(|&line| line as isize),
            (Some(n), Some(name)) => {
                let found = lines(name);
                usize::try_from(if n > 0 {
                    n - 1
                } else {
                    found.len() as isize + n
                })
                .ok()
                .and_then(|index| found.get(index))
                .map(|&line| line as isize)
            }
        };
        (line.map(|line| line.clamp(0, 4095) as usize), 0, None)
    };
    let (start, end) = (side(start, "start"), side(end, "end"));
    let (line, span) = match (start.0, end.0) {
        (Some(start), Some(end)) => (Some(start.min(end)), start.abs_diff(end).max(1)),
        (Some(start), None) => (
            Some(start),
            end.2.map_or(end.1.max(1), |name| {
                lines(name)
                    .into_iter()
                    .find(|&line| line > start)
                    .map_or(1, |line| line - start)
            }),
        ),
        (None, Some(end)) => {
            let span = start.2.map_or(start.1.max(1), |name| {
                lines(name)
                    .into_iter()
                    .rfind(|&line| line < end)
                    .map_or(1, |line| end - line)
            });
            (Some(end.saturating_sub(span)), span.min(end).max(1))
        }
        (None, None) => (None, if start.1 > 0 { start.1 } else { end.1.max(1) }),
    };
    (line, span.clamp(1, 4096 - line.unwrap_or(0)))
}

fn size(
    tracks: &[Track],
    items: &[(usize, usize, [f32; 3])],
    gap: f32,
    available: Option<f32>,
    stretch: bool,
    table: bool,
) -> Vec<f32> {
    use Sizing::*;
    let count = tracks.len();
    let gaps = |span: usize| span.saturating_sub(1) as f32 * gap;
    let (min, max) = (|i: usize| tracks[i].0, |i: usize| tracks[i].1);
    let factor = |i: usize| match max(i) {
        Flex(factor) => Some(factor),
        _ => None,
    };
    let mut base: Vec<f32> = (0..count)
        .map(|i| match min(i) {
            Fixed(size) => size,
            _ => 0.,
        })
        .collect();
    let mut limit: Vec<f32> = (0..count)
        .map(|i| match max(i) {
            Fixed(size) => size.max(base[i]),
            _ => f32::INFINITY,
        })
        .collect();
    let crosses = |&&(start, span, _): &&(usize, usize, [f32; 3])| {
        (start..start + span).any(|i| factor(i).is_some())
    };
    let group = |item: &&(usize, usize, [f32; 3])| {
        if crosses(item) { usize::MAX } else { item.1 }
    };
    let mut items: Vec<_> = items
        .iter()
        .filter(|(start, span, _)| *span > 0 && start + span <= count)
        .collect();
    items.sort_by_key(group);
    let mut growable = vec![false; count];
    for items in items.chunk_by(|a, b| group(a) == group(b)) {
        let flexible = group(&items[0]) == usize::MAX;
        for step in 0..6 {
            if step == 3 {
                for (limit, base) in limit.iter_mut().zip(&base) {
                    *limit = limit.max(*base);
                }
                growable.fill(false);
                continue;
            }
            if flexible && step > 3 {
                break;
            }
            let growing = step > 3;
            let current = |i: usize| {
                if growing && limit[i].is_finite() {
                    limit[i]
                } else {
                    base[i]
                }
            };
            let mut planned = vec![None::<f32>; count];
            for &&(start, span, contribution) in items {
                let targets: Vec<_> = (start..start + span)
                    .filter(|&i| {
                        (!flexible || factor(i).is_some())
                            && match step {
                                0 => matches!(min(i), MinContent | MaxContent | Auto),
                                1 => matches!(min(i), MinContent | MaxContent),
                                2 => min(i) == MaxContent || available.is_none() && min(i) == Auto,
                                4 => {
                                    matches!(max(i), MinContent | MaxContent | Auto | FitContent(_))
                                }
                                _ => matches!(max(i), MaxContent | Auto | FitContent(_)),
                            }
                    })
                    .collect();
                if targets.is_empty() {
                    continue;
                }
                let value = match step {
                    0 if available.is_none() => contribution[2]
                        .min(
                            (start..start + span)
                                .map(|i| match max(i) {
                                    Fixed(size) | FitContent(size) => size,
                                    _ => f32::INFINITY,
                                })
                                .sum::<f32>()
                                + gaps(span),
                        )
                        .max(contribution[0]),
                    0 => contribution[0],
                    1 | 4 => contribution[1],
                    _ => contribution[2],
                };
                let mut space =
                    value - (start..start + span).map(current).sum::<f32>() - gaps(span);
                let weights: Vec<f32> = targets.iter().map(|&i| factor(i).unwrap_or(1.)).collect();
                let uniform = weights.iter().sum::<f32>() <= 0.;
                let mut caps: Vec<f32> = targets
                    .iter()
                    .map(|&i| match (growing, max(i)) {
                        (true, _) if limit[i].is_finite() && !growable[i] => 0.,
                        (true, FitContent(size)) => (size - current(i)).max(0.),
                        (true, _) => f32::INFINITY,
                        (false, FitContent(size)) => (limit[i].min(size) - base[i]).max(0.),
                        (false, _) => (limit[i] - base[i]).max(0.),
                    })
                    .collect();
                let mut incurred = vec![0f32; targets.len()];
                for pass in 0..2 {
                    if pass == 1 {
                        let beyond = |i: usize| match (growing, step) {
                            (true, _) => !matches!(max(i), FitContent(_)),
                            (false, 2) => matches!(max(i), MaxContent | Auto | FitContent(_)),
                            _ => matches!(max(i), MinContent | MaxContent | Auto | FitContent(_)),
                        };
                        let any = targets.iter().any(|&i| beyond(i));
                        for ((cap, &i), incurred) in caps.iter_mut().zip(&targets).zip(&incurred) {
                            *cap = if beyond(i) || !any && !growing {
                                f32::INFINITY
                            } else {
                                *incurred
                            };
                        }
                    }
                    let weight = |k: usize| if uniform { 1. } else { weights[k] };
                    for _ in 0..=targets.len() {
                        let open: Vec<_> = (0..targets.len())
                            .filter(|&k| incurred[k] < caps[k])
                            .collect();
                        let total: f32 = open.iter().map(|&k| weight(k)).sum();
                        if space <= 1e-3 || total <= 0. {
                            break;
                        }
                        let share = space / total;
                        for k in open {
                            let add = (share * weight(k)).min(caps[k] - incurred[k]);
                            incurred[k] += add;
                            space -= add;
                        }
                    }
                }
                for (&i, incurred) in targets.iter().zip(incurred) {
                    planned[i] = Some(planned[i].unwrap_or(0.).max(incurred));
                }
            }
            for (i, planned) in planned.into_iter().enumerate() {
                let Some(planned) = planned else { continue };
                if !growing {
                    base[i] += planned;
                } else if limit[i].is_finite() {
                    limit[i] += planned;
                } else {
                    limit[i] = base[i] + planned;
                    growable[i] = step == 4;
                }
            }
        }
    }
    for (i, limit) in limit.iter_mut().enumerate() {
        if !limit.is_finite() {
            *limit = base[i];
        }
        if let FitContent(size) = max(i) {
            *limit = limit.min(size);
        }
        *limit = limit.max(base[i]);
    }
    if let Some(space) = available
        && table
    {
        // Table columns grow from their min-content toward their max-content width in proportion to the gap.
        let free = space - base.iter().sum::<f32>() - gaps(count);
        let room: f32 = (0..count).map(|i| limit[i] - base[i]).sum();
        if free > 0. && room > 0. {
            let share = (free / room).min(1.);
            for i in 0..count {
                base[i] += (limit[i] - base[i]) * share;
            }
        }
    } else if let Some(space) = available {
        let mut free = space - base.iter().sum::<f32>() - gaps(count);
        for _ in 0..=count {
            let open: Vec<_> = (0..count).filter(|&i| base[i] < limit[i]).collect();
            if open.is_empty() || free <= 1e-3 {
                break;
            }
            let share = free / open.len() as f32;
            for i in open {
                let add = share.min(limit[i] - base[i]);
                base[i] += add;
                free -= add;
            }
        }
    } else {
        base.clone_from(&limit);
    }
    let find = |range: std::ops::Range<usize>, space: f32| {
        let mut inflexible = vec![false; count];
        for _ in 0..=range.len() {
            let mut leftover = space - gaps(range.len());
            let mut factors = 0.;
            for i in range.clone() {
                match factor(i) {
                    Some(factor) if !inflexible[i] => factors += factor,
                    _ => leftover -= base[i],
                }
            }
            let size = leftover / f32::max(factors, 1.);
            let mut restart = false;
            for i in range.clone() {
                if factor(i).is_some_and(|factor| !inflexible[i] && size * factor < base[i]) {
                    inflexible[i] = true;
                    restart = true;
                }
            }
            if !restart {
                return size.max(0.);
            }
        }
        0.
    };
    let fraction = match available {
        Some(space) if space - base.iter().sum::<f32>() - gaps(count) > 0. => find(0..count, space),
        Some(_) => 0.,
        None => {
            (0..count)
                .filter_map(|i| factor(i).map(|factor| base[i] / factor.max(1.)))
                .chain(items.iter().filter(|item| crosses(item)).map(
                    |&&(start, span, contribution)| find(start..start + span, contribution[2]),
                ))
                .fold(0., f32::max)
        }
    };
    for (i, base) in base.iter_mut().enumerate() {
        if let Some(factor) = factor(i) {
            *base = base.max(fraction * factor);
        }
    }
    if stretch && let Some(space) = available {
        let free = space - base.iter().sum::<f32>() - gaps(count);
        let automatic: Vec<_> = (0..count).filter(|&i| max(i) == Auto).collect();
        // Tables give the extra width to columns in proportion to their width, like Chrome.
        let total: f32 = automatic.iter().map(|&i| base[i]).sum();
        if free > 0. {
            for &i in &automatic {
                base[i] += if table && total > 0. {
                    free * base[i] / total
                } else {
                    free / automatic.len() as f32
                };
            }
        }
    }
    base
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
    let key = (id, width.map(f32::to_bits), height.map(f32::to_bits));
    if let Some(value) = measured.sizes.get(&key) {
        return *value;
    }
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    let units = &styles.lengths[id];
    let display = style.get(Prop::DISPLAY).unwrap_or("");
    // WARNING: measuring recurses per DOM level, so deep pages must not overflow the stack.
    if display == "none"
        || std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
            .nth(256)
            .is_some()
    {
        return [0.; 4];
    }
    let base = width.unwrap_or(0.);
    let padding = units.edges(style, style::PADDING, base);
    let margin = units.edges(style, style::MARGIN, base);
    let border = style::BORDERS.map(|[width, kind, _]| {
        if style
            .get(kind)
            .is_none_or(|kind| matches!(kind, "none" | "hidden"))
        {
            return 0.;
        }
        style
            .get(width)
            .map_or(Some(3.), |v| units.resolve(v, base))
            .unwrap_or(0.)
            .max(0.)
    });
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
    let sizes = |props: [Prop; 3], base: Option<f32>| {
        let axis = usize::from(props[0] == Prop::HEIGHT);
        props.map(|prop| {
            style
                .get(prop)
                .filter(|v| base.is_some() || !v.contains('%'))
                .and_then(|v| units.resolve(v, base.unwrap_or(0.)))
                .map(|v| (v - if border_box { inner[axis] } else { 0. }).max(0.))
        })
    };
    let clamp = |value: f32, [_, min, max]: [Option<f32>; 3]| {
        value
            .min(max.unwrap_or(f32::INFINITY))
            .max(min.unwrap_or(0.))
    };
    let horizontal = [Prop::WIDTH, Prop::MIN_WIDTH, Prop::MAX_WIDTH];
    let fixed = sizes(horizontal, None);
    let used = sizes(horizontal, width);
    let available = used[0]
        .or(width.map(|width| width - outer[0]))
        .or(fixed[0])
        .map(|width| clamp(width, used).max(0.));
    let vertical = sizes([Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT], height);
    let mut content = [0f32; 3];
    if matches!(node.tag.as_str(), "svg" | "img" | "canvas" | "video") {
        let default = match node.tag.as_str() {
            "svg" => [24., 24.],
            "canvas" | "video" => [300., 150.],
            _ => [0., 0.],
        };
        let [width, height] = [0, 1].map(|axis| {
            node.attrs
                .get(["width", "height"][axis])
                .and_then(|v| super::element::dimension(v, node.tag == "canvas"))
                .unwrap_or(default[axis])
        });
        content = [width, width, height];
    } else if super::element::text_field(node) {
        // A percentage width is `auto` for the max-content size, like in Chrome.
        let [width, height] = super::element::field_size(
            doc, id, style, units, text, measured.pixel_ratio, true, available,
        );
        content = [width, width, height];
    } else if let Some(inline) = style::inline_text(doc, styles, id) {
        let wrap = !style
            .get(Prop::WHITE_SPACE)
            .is_some_and(|space| matches!(space, "pre" | "nowrap"));
        let word_break = style::word_break(style);
        let anywhere = matches!(
            word_break,
            renderer::WordBreak::Anywhere | renderer::WordBreak::BreakAll
        );
        let lines = text
            .shape_text(
                inline.text.to_string().into(),
                renderer::px(units.font.em),
                &inline.runs,
                available.filter(|_| wrap).map(renderer::px),
                style
                    .get(Prop::WEBKIT_LINE_CLAMP)
                    .or_else(|| style.get(Prop::LINE_CLAMP))
                    .and_then(|value| value.parse::<usize>().ok())
                    .map(|lines| lines.max(1)),
                word_break,
            )
            .unwrap_or_default();
        for line in &lines {
            let layout = &line.unwrapped_layout;
            let width = f32::from(layout.width);
            content[1] = content[1].max(width);
            content[2] += (1 + line.wrap_boundaries.len()) as f32 * units.font.lh;
            let mut glyphs = layout.runs.iter().flat_map(|run| &run.glyphs).peekable();
            let (mut start, mut previous) = (0., ' ');
            while let Some(glyph) = glyphs.next() {
                let character = line
                    .text
                    .get(glyph.index..)
                    .and_then(|v| v.chars().next())
                    .unwrap_or(' ');
                if !character.is_whitespace() {
                    let x = f32::from(glyph.position.x);
                    if anywhere || previous.is_whitespace() || character > '\u{2e7f}' {
                        start = x;
                    }
                    let end = glyphs
                        .peek()
                        .map_or(width, |next| f32::from(next.position.x));
                    content[0] = content[0].max(end - start);
                }
                previous = character;
            }
        }
        if !wrap {
            content[0] = content[1];
        }
    } else if matches!(display, "grid" | "inline-grid")
        && ![Prop::GRID_TEMPLATE_COLUMNS, Prop::GRID_TEMPLATE_ROWS]
            .into_iter()
            .any(|name| style.get(name).is_some_and(|v| v.starts_with("subgrid")))
    {
        let extent = |grid: &Grid| {
            [(&grid.columns, grid.gap[0]), (&grid.rows, grid.gap[1])].map(|(sizes, gap)| {
                sizes.iter().sum::<f32>() + sizes.len().saturating_sub(1) as f32 * gap
            })
        };
        let [min, _] = extent(&layout(
            doc,
            id,
            styles,
            Some(0.),
            None,
            None,
            None,
            text,
            measured,
        ));
        let [max, natural] = extent(&layout(
            doc, id, styles, None, None, None, None, text, measured,
        ));
        content = [min, max, natural];
        if available.is_some() || vertical[0].is_some() {
            let grid = layout(
                doc,
                id,
                styles,
                available,
                vertical[0],
                None,
                None,
                text,
                measured,
            );
            content[2] = extent(&grid)[1];
        }
    } else {
        let inline = !node.children.is_empty()
            && node.children.iter().all(|&child| {
                doc.nodes[child].tag.starts_with('#')
                    || doc.nodes[child].tag == "br"
                    || styles.values[child]
                        .get(Prop::DISPLAY)
                        .is_some_and(|value| value.starts_with("inline"))
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
        let (mut line, mut run) = ([0f32; 2], 0f32);
        let mut first = true;
        for &child in &node.children {
            if styles.values[child]
                .get(Prop::POSITION)
                .is_some_and(|v| matches!(v, "absolute" | "fixed"))
                || styles.values[child].get(Prop::DISPLAY) == Some("none")
                || !preserve
                    && doc.nodes[child].tag == "#text"
                    && doc.nodes[child].text.trim_ascii().is_empty()
            {
                continue;
            }
            let gap = if first { [0.; 2] } else { gaps };
            first = false;
            // Inline-level children of a block share lines between its block children.
            let inline_level = !flex
                && (doc.nodes[child].tag.starts_with('#')
                    || doc.nodes[child].tag == "br"
                    || styles.values[child]
                        .get(Prop::DISPLAY)
                        .is_some_and(|value| value.starts_with("inline")));
            if !row && !inline_level {
                content[1] = content[1].max(run);
                content[2] += line[1];
                (line, run) = ([0.; 2], 0.);
                let [min, max, _, height] =
                    intrinsic(doc, child, styles, available, None, text, measured);
                content = [
                    content[0].max(min),
                    content[1].max(max),
                    content[2] + height + gap[1],
                ];
                continue;
            }
            let [min, max, _, height] = intrinsic(doc, child, styles, None, None, text, measured);
            let width = available.map_or(max, |available| max.min(available));
            let height = if width < max {
                intrinsic(doc, child, styles, Some(width), None, text, measured)[3]
            } else {
                height
            };
            let wrap = wrap || inline_level;
            content[0] = if wrap {
                content[0].max(min)
            } else {
                content[0] + min + gap[0]
            };
            run += max + gap[0];
            if wrap
                && line[0] > 0.
                && available.is_some_and(|available| line[0] + gaps[0] + width > available)
            {
                content[2] += line[1] + gaps[1];
                line = [0.; 2];
            }
            line = [
                line[0] + if line[0] > 0. { gaps[0] } else { 0. } + width,
                line[1].max(height),
            ];
            if !flex && doc.nodes[child].tag == "br" {
                content[1] = content[1].max(run);
                content[2] += line[1].max(styles.lengths[child].font.lh);
                (line, run) = ([0.; 2], 0.);
            }
        }
        content[1] = content[1].max(run);
        content[2] += line[1];
    }
    // `::before` and `::after` boxes take room in the first and last line, or stack as blocks.
    if let Some(pseudos) = styles.pseudos.get(&id).filter(|_| {
        !matches!(
            node.tag.as_str(),
            "img" | "svg" | "canvas" | "video" | "input" | "textarea" | "select"
        )
    }) {
        for pseudo in pseudos.iter() {
            let Some(value) = pseudo
                .get(Prop::CONTENT)
                .filter(|value| !matches!(*value, "none" | "normal"))
            else {
                continue;
            };
            if pseudo.get(Prop::DISPLAY) == Some("none")
                || pseudo
                    .get(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"))
            {
                continue;
            }
            let mut units = *units;
            if let Some(size) = pseudo
                .get(Prop::FONT_SIZE)
                .and_then(|value| units.resolve(value, units.font.em))
            {
                units.font = style::FontUnits::new(size);
            }
            let [padding, margin] =
                [style::PADDING, style::MARGIN].map(|edges| units.edges(pseudo, edges, base));
            let border = style::BORDERS.map(|[width, kind, _]| {
                if pseudo
                    .get(kind)
                    .is_none_or(|kind| matches!(kind, "none" | "hidden"))
                {
                    return 0.;
                }
                pseudo
                    .get(width)
                    .map_or(Some(3.), |value| units.resolve(value, base))
                    .unwrap_or(0.)
                    .max(0.)
            });
            let sides = |axis: usize| {
                let inner =
                    padding[1 - axis] + padding[3 - axis] + border[1 - axis] + border[3 - axis];
                let outer = margin[1 - axis] + margin[3 - axis];
                match pseudo.get(Prop::BOX_SIZING) {
                    Some("border-box") => [outer, outer + inner],
                    _ => [inner + outer; 2],
                }
            };
            let string = super::element::pseudo_content(doc, id, pseudo, value);
            let [width, height] = [Prop::WIDTH, Prop::HEIGHT].map(|prop| {
                pseudo
                    .get(prop)
                    .and_then(|value| units.resolve(value, base))
            });
            let width = match width {
                Some(width) => width + sides(0)[0],
                None if string.is_empty() => sides(0)[1],
                None => {
                    let run = style::text_run(pseudo, &units, string.len());
                    let shaped = text
                        .shape_text(
                            string.into(),
                            renderer::px(units.font.em),
                            &[run],
                            None,
                            None,
                            style::word_break(pseudo),
                        )
                        .unwrap_or_default();
                    shaped
                        .iter()
                        .map(|line| f32::from(line.unwrapped_layout.width))
                        .fold(0., f32::max)
                        + sides(0)[1]
                }
            };
            if pseudo
                .get(Prop::DISPLAY)
                .is_none_or(|value| value.starts_with("inline"))
            {
                content[0] = content[0].max(width);
                content[1] += width;
            } else {
                content[0] = content[0].max(width);
                content[1] = content[1].max(width);
                content[2] +=
                    height.map_or(units.font.lh + sides(1)[1], |height| height + sides(1)[0]);
            }
        }
    }
    let [min, max] = [content[0], content[1]].map(|value| clamp(fixed[0].unwrap_or(value), fixed));
    // Like Chrome, replaced boxes sized against their container add nothing to the min-content width.
    let min = if matches!(
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
    // An `aspect-ratio` sizes the `box-sizing` box from the width, but a box that doesn't clip still grows
    // to fit its content, like in Chrome.
    let ratio = vertical[0]
        .is_none()
        .then(|| style.get(Prop::ASPECT_RATIO))
        .flatten()
        .and_then(super::element::aspect_ratio)
        .zip(available)
        .map(|(ratio, width)| {
            let height = if border_box {
                (width + inner[0]) / ratio - inner[1]
            } else {
                width / ratio
            };
            let clips = [Prop::OVERFLOW_Y, Prop::OVERFLOW]
                .into_iter()
                .find_map(|prop| style.get(prop))
                .is_some_and(|value| value != "visible");
            if clips || matches!(node.tag.as_str(), "svg" | "img" | "canvas" | "video") {
                height.max(0.)
            } else {
                height.max(content[2])
            }
        });
    let result = [
        min + outer[0],
        max + outer[0],
        available.unwrap_or(max) + outer[0],
        clamp(vertical[0].or(ratio).unwrap_or(content[2]), vertical) + outer[1],
    ];
    if measured.sizes.len() > 65536 {
        measured.sizes.clear();
    }
    measured.sizes.insert(key, result);
    result
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn layout(
    doc: &Document,
    id: usize,
    styles: &ComputedStyles,
    width: Option<f32>,
    height: Option<f32>,
    inherited: Option<&Grid>,
    baselines: Option<&Baselines>,
    text: &renderer::TextSystem,
    measured: &mut Measurements,
) -> Grid {
    use Sizing::*;
    let style = &styles.values[id];
    let units = &styles.lengths[id];
    let available = [width, height];
    let get = |prop| style.get(prop);
    let mut template = [
        get(Prop::GRID_TEMPLATE_COLUMNS),
        get(Prop::GRID_TEMPLATE_ROWS),
    ]
    .map(|v| v.map(str::to_owned));
    let mut areas = get(Prop::GRID_TEMPLATE_AREAS).map(str::to_owned);
    let mut flow = get(Prop::GRID_AUTO_FLOW).unwrap_or("row").to_owned();
    let mut implicit = [get(Prop::GRID_AUTO_COLUMNS), get(Prop::GRID_AUTO_ROWS)]
        .map(|v| v.unwrap_or("auto").to_owned());
    if template.iter().all(Option::is_none)
        && areas.is_none()
        && let Some(value) = get(Prop::GRID_TEMPLATE)
            .or_else(|| get(Prop::GRID))
            .filter(|v| *v != "none")
    {
        let (mut depth, mut quote, mut split) = (0, None, value.len());
        for (index, character) in value.char_indices() {
            match character {
                '"' | '\'' if quote.is_none_or(|quote| quote == character) => {
                    quote = quote.xor(Some(character))
                }
                '(' | '[' if quote.is_none() => depth += 1,
                ')' | ']' if quote.is_none() => depth -= 1,
                '/' if depth == 0 && quote.is_none() => {
                    split = index;
                    break;
                }
                _ => {}
            }
        }
        let (rows, columns) = (&value[..split], value.get(split + 1..).unwrap_or("").trim());
        let sizes = |value: &str| {
            let sizes: Vec<_> = value
                .split_whitespace()
                .filter(|word| !matches!(*word, "auto-flow" | "dense"))
                .collect();
            if sizes.is_empty() {
                "auto".into()
            } else {
                sizes.join(" ")
            }
        };
        if rows.contains("auto-flow") {
            flow = if rows.contains("dense") {
                "row dense"
            } else {
                "row"
            }
            .into();
            implicit[1] = sizes(rows);
            template[0] = Some(columns.into());
        } else if columns.contains("auto-flow") {
            flow = if columns.contains("dense") {
                "column dense"
            } else {
                "column"
            }
            .into();
            implicit[0] = sizes(columns);
            template[1] = Some(rows.trim().into());
        } else {
            let (mut list, mut strings) = (Vec::new(), Vec::new());
            for part in style::components(rows, ' ') {
                if part.starts_with(['"', '\'']) {
                    strings.push(part);
                    list.push("auto");
                } else if part.starts_with('[') {
                    list.push(part);
                } else if let Some(last) = list.iter_mut().rev().find(|v| !v.starts_with('[')) {
                    *last = part;
                }
            }
            template[0] = Some(columns.into());
            template[1] = Some(if strings.is_empty() {
                rows.trim().into()
            } else {
                list.join(" ")
            });
            if !strings.is_empty() {
                areas = Some(strings.join(" "));
            }
        }
    }
    let shared = [0, 1].map(|axis| {
        inherited
            .map(|grid| if axis == 0 { &grid.columns } else { &grid.rows })
            .filter(|sizes| {
                !sizes.is_empty()
                    && template[axis]
                        .as_deref()
                        .is_some_and(|v| v.starts_with("subgrid"))
            })
    });
    let pair = |prop, axis: usize| {
        let parts = style::components(get(prop)?, ' ');
        parts.get(axis ^ 1).or(parts.first()).copied()
    };
    let gap = [0, 1].map(|axis| {
        match (
            get([Prop::COLUMN_GAP, Prop::ROW_GAP][axis])
                .filter(|v| *v != "normal")
                .or_else(|| get([Prop::GRID_COLUMN_GAP, Prop::GRID_ROW_GAP][axis]))
                .or_else(|| pair(Prop::GRID_GAP, axis)),
            shared[axis],
        ) {
            (None, Some(_)) => inherited.map_or(0., |grid| grid.gap[axis]),
            (value, _) => value
                .filter(|v| available[axis].is_some() || !v.contains('%'))
                .and_then(|v| units.resolve(v, available[axis].unwrap_or(0.)))
                .unwrap_or(0.)
                .max(0.),
        }
    });
    // Each string is a row; strings need no space between them, like `'a a''b c'`.
    let area_rows: Vec<Vec<&str>> = areas
        .as_deref()
        .unwrap_or_default()
        .split(['"', '\''])
        .skip(1)
        .step_by(2)
        .map(style::area_cells)
        .collect();
    let mut regions: HashMap<&str, [usize; 4]> = HashMap::new();
    for (y, row) in area_rows.iter().enumerate() {
        for (x, &name) in row.iter().enumerate() {
            if name != "." {
                let region = regions.entry(name).or_insert([x, y, x + 1, y + 1]);
                *region = [
                    region[0].min(x),
                    region[1].min(y),
                    region[2].max(x + 1),
                    region[3].max(y + 1),
                ];
            }
        }
    }
    let mut names: [Lines; 2] = Default::default();
    for (name, [left, top, right, bottom]) in regions {
        names[0].extend([
            (format!("{name}-start"), left),
            (format!("{name}-end"), right),
        ]);
        names[1].extend([
            (format!("{name}-start"), top),
            (format!("{name}-end"), bottom),
        ]);
    }
    let mut defined: [Vec<Track>; 2] = Default::default();
    let mut explicit = [0; 2];
    let mut fit = [None; 2];
    for axis in 0..2 {
        if let Some(sizes) = shared[axis] {
            defined[axis] = sizes
                .iter()
                .map(|&size| (Fixed(size), Fixed(size)))
                .collect();
            explicit[axis] = sizes.len();
            continue;
        }
        let (list, lines, collapse) = tracks(
            template[axis].as_deref().unwrap_or("none"),
            units,
            available[axis],
            gap[axis],
        );
        let extent = if axis == 0 {
            area_rows.iter().map(Vec::len).max().unwrap_or(0)
        } else {
            area_rows.len()
        };
        explicit[axis] = list.len().max(extent).min(4096);
        names[axis].extend(lines);
        defined[axis] = list;
        fit[axis] = collapse;
    }
    let automatic = [0, 1].map(|axis| tracks(&implicit[axis], units, available[axis], 0.).0);
    let ident = |value: &&str| {
        *value != "auto" && !value.contains(char::is_whitespace) && value.parse::<isize>().is_err()
    };
    let alignment = |child: usize, axis: usize| {
        let own = &styles.values[child];
        let margins = [
            [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT],
            [Prop::MARGIN_TOP, Prop::MARGIN_BOTTOM],
        ][axis]
            .map(|name| own.get(name).is_some_and(|v| v == "auto"));
        let mut value = "normal";
        for (style, name, shorthand) in [
            (
                own,
                [Prop::JUSTIFY_SELF, Prop::ALIGN_SELF][axis],
                Prop::PLACE_SELF,
            ),
            (
                style,
                [Prop::JUSTIFY_ITEMS, Prop::ALIGN_ITEMS][axis],
                Prop::PLACE_ITEMS,
            ),
        ] {
            let parts: Vec<&str> = style
                .get(shorthand)
                .map_or(Vec::new(), |v| v.split_whitespace().collect());
            if let Some(found) = style
                .get(name)
                .or(parts.get(axis ^ 1).or(parts.first()).copied())
                .filter(|v| *v != "auto")
            {
                value = found;
                break;
            }
        }
        // Baseline alignment falls back to the start, or to the end for the last baseline.
        let last = value.starts_with("last");
        match (margins, value.split_whitespace().last().unwrap_or(value)) {
            ([true, true], _) => (Some(0.5), None),
            ([true, false], _) => (Some(1.), None),
            ([false, true], _) => (Some(0.), None),
            (_, "normal")
                if matches!(
                    doc.nodes[child].tag.as_str(),
                    "img" | "svg" | "canvas" | "video"
                ) =>
            {
                (Some(0.), None)
            }
            (_, "normal" | "stretch") => (None, None),
            (_, "center") => (Some(0.5), None),
            (_, "end" | "flex-end" | "self-end" | "right") => (Some(1.), None),
            (_, "baseline") => (Some(f32::from(u8::from(last))), (axis == 1).then_some(last)),
            _ => (Some(0.), None),
        }
    };
    /// What an item's style says, read once per style.
    #[derive(Clone, Copy)]
    struct Item<'a> {
        flowed: bool,
        order: i32,
        placed: ([Option<usize>; 2], [usize; 2]),
        alignment: [Option<f32>; 2],
        /// Aligns its first baseline with its row, or its last one when `true`.
        baseline: Option<bool>,
        /// Whether the minimum size is zero, per axis.
        shrink: [bool; 2],
        auto_width: bool,
        height: Option<&'a str>,
        subgrid: [bool; 2],
    }
    // WARNING: large grids repeat a few styles over thousands of items, so each style is read once.
    let mut memo: rustc_hash::FxHashMap<(*const style::Style, bool), Item> = Default::default();
    // A fieldset's rendered legend sits in its top border, outside its grid.
    let legend = (doc.nodes[id].tag == "fieldset")
        .then(|| super::element::legend(doc, styles, id))
        .flatten()
        .map(|(legend, ..)| legend);
    let mut read = |child: usize| -> Item {
        let style = &styles.values[child];
        let replaced = matches!(
            doc.nodes[child].tag.as_str(),
            "img" | "svg" | "canvas" | "video"
        );
        let mut item = *memo
            .entry((std::rc::Rc::as_ptr(style), replaced))
            .or_insert_with(|| {
                let flowed = !style.get(Prop::DISPLAY).is_some_and(|v| v == "none")
                    && !style
                        .get(Prop::POSITION)
                        .is_some_and(|v| matches!(v, "absolute" | "fixed"));
                let order = style
                    .get(Prop::ORDER)
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0);
                let placed = {
                    let split = |prop| {
                        style.get(prop).map_or(Vec::new(), |v| {
                            v.split('/').map(str::trim).collect::<Vec<_>>()
                        })
                    };
                    let area = split(Prop::GRID_AREA);
                    let (mut position, mut span) = ([None; 2], [1; 2]);
                    for axis in 0..2 {
                        let [name, start, end] = [
                            [
                                Prop::GRID_COLUMN,
                                Prop::GRID_COLUMN_START,
                                Prop::GRID_COLUMN_END,
                            ],
                            [Prop::GRID_ROW, Prop::GRID_ROW_START, Prop::GRID_ROW_END],
                        ][axis];
                        let pair = split(name);
                        let shorthand = if let Some(&first) = pair.first() {
                            (
                                Some(first),
                                pair.get(1).copied().or(Some(first).filter(ident)),
                            )
                        } else {
                            let start = area
                                .get(1 - axis)
                                .copied()
                                .or(area.first().copied().filter(ident));
                            (start, area.get(3 - axis).copied().or(start.filter(ident)))
                        };
                        let longhand = |prop| style.get(prop);
                        (position[axis], span[axis]) = placement(
                            longhand(start).or(shorthand.0).unwrap_or("auto"),
                            longhand(end).or(shorthand.1).unwrap_or("auto"),
                            &names[axis],
                            explicit[axis],
                        );
                        if shared[axis].is_some() {
                            let start = position[axis].map(|v| v.min(explicit[axis] - 1));
                            position[axis] = start;
                            span[axis] = span[axis].min(explicit[axis] - start.unwrap_or(0));
                        }
                    }
                    (position, span)
                };
                let shrink = [0, 1].map(|axis| {
                    style
                        .get([Prop::OVERFLOW_X, Prop::OVERFLOW_Y][axis])
                        .or_else(|| style.get(Prop::OVERFLOW))
                        .is_some_and(|v| !matches!(v, "visible" | "clip"))
                        || style
                            .get([Prop::MIN_WIDTH, Prop::MIN_HEIGHT][axis])
                            .is_some_and(|v| v == "0" || v == "0px")
                });
                let subgrid = [0, 1].map(|axis| {
                    style
                        .get([Prop::GRID_TEMPLATE_COLUMNS, Prop::GRID_TEMPLATE_ROWS][axis])
                        .is_some_and(|v| v.starts_with("subgrid"))
                });
                let [(column, _), (row, baseline)] = [0, 1].map(|axis| alignment(child, axis));
                Item {
                    flowed,
                    order,
                    placed,
                    alignment: [column, row],
                    baseline,
                    shrink,
                    auto_width: style.get(Prop::WIDTH).is_none_or(|v| v == "auto"),
                    height: style.get(Prop::HEIGHT),
                    subgrid,
                }
            });
        item.flowed &= legend != Some(child);
        item
    };
    let mut children: Vec<(usize, Item)> = doc.nodes[id]
        .children
        .iter()
        .filter(|&&child| {
            !(doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty())
        })
        .map(|&child| (child, read(child)))
        .filter(|(_, item)| item.flowed)
        .collect();
    children.sort_by_key(|(_, item)| item.order);
    let (children, derived): (Vec<usize>, Vec<Item>) = children.into_iter().unzip();
    let items: Vec<([Option<usize>; 2], [usize; 2])> =
        derived.iter().map(|item| item.placed).collect();
    let across = usize::from(flow.contains("column"));
    let down = 1 - across;
    let dense = flow.contains("dense");
    let mut count = explicit;
    for (position, span) in &items {
        count[across] = count[across].max(position[across].unwrap_or(0) + span[across]);
    }
    count[across] = count[across].max(1);
    let fits = |occupied: &Vec<Vec<bool>>, at: [usize; 2], span: [usize; 2]| {
        (at[down]..at[down] + span[down]).all(|y| {
            occupied.get(y).is_none_or(|row| {
                row.get(at[across]..(at[across] + span[across]).min(row.len()))
                    .is_none_or(|cells| !cells.contains(&true))
            })
        })
    };
    let mark = |occupied: &mut Vec<Vec<bool>>, at: [usize; 2], span: [usize; 2]| {
        let end = [at[0] + span[0], at[1] + span[1]];
        if occupied.len() < end[down] {
            occupied.resize(end[down], Vec::new());
        }
        for row in &mut occupied[at[down]..end[down]] {
            if row.len() < end[across] {
                row.resize(end[across], false);
            }
            row[at[across]..end[across]].fill(true);
        }
    };
    let mut occupied = Vec::new();
    let mut placed = vec![[0; 2]; items.len()];
    for (index, (position, span)) in items.iter().enumerate() {
        if let [Some(column), Some(row)] = *position {
            placed[index] = [column, row];
            mark(&mut occupied, placed[index], *span);
        }
    }
    // WARNING: dense packing rescans placed cells for every item, so a budget of cell checks
    // bounds pathological grids, which then fall back to sparse placement.
    let mut budget = 1usize << 24;
    let mut lanes = HashMap::new();
    for (index, (position, span)) in items.iter().enumerate() {
        if let (None, Some(lane)) = (position[across], position[down]) {
            let mut at = [0; 2];
            at[down] = lane;
            at[across] = if dense && budget > 0 {
                0
            } else {
                lanes.get(&lane).copied().unwrap_or(0)
            };
            while budget > 0 && !fits(&occupied, at, *span) && at[across] + span[across] < 4096 {
                budget = budget.saturating_sub(span[0] * span[1]);
                at[across] += 1;
            }
            lanes.insert(lane, at[across] + span[across]);
            count[across] = count[across].max(at[across] + span[across]);
            placed[index] = at;
            mark(&mut occupied, at, *span);
        }
    }
    let mut cursor = [0; 2];
    let mut free = 0;
    for (index, (position, span)) in items.iter().enumerate() {
        if position[down].is_some() {
            continue;
        }
        let dense = dense && budget > 0;
        let mut at = cursor;
        if dense {
            while occupied.get(free).is_some_and(|row: &Vec<bool>| {
                row.len() >= count[across] && !row[..count[across]].contains(&false)
            }) {
                free += 1;
            }
            budget = budget.saturating_sub(count[across]);
            at = [0; 2];
            at[down] = free;
        }
        if let Some(fixed) = position[across] {
            if !dense && fixed < at[across] {
                at[down] += 1;
            }
            at[across] = fixed;
            while budget > 0 && !fits(&occupied, at, *span) && at[down] < 4096 {
                budget = budget.saturating_sub(span[0] * span[1]);
                at[down] += 1;
            }
        } else {
            loop {
                if at[across] + span[across] > count[across] {
                    at[across] = 0;
                    at[down] += 1;
                }
                if at[down] >= 4096 || budget == 0 || fits(&occupied, at, *span) {
                    break;
                }
                budget = budget.saturating_sub(span[0] * span[1]);
                at[across] += 1;
            }
        }
        at[down] = at[down].min(4096 - span[down]);
        placed[index] = at;
        mark(&mut occupied, at, *span);
        cursor = at;
        cursor[across] += span[across];
    }
    for (at, (_, span)) in placed.iter().zip(&items) {
        for axis in 0..2 {
            count[axis] = count[axis].max(at[axis] + span[axis]);
        }
    }
    let mut cells: Vec<[usize; 4]> = placed
        .iter()
        .zip(&items)
        .map(|(at, (_, span))| {
            let mut cell = [at[0], at[1], span[0], span[1]];
            for axis in 0..2 {
                if shared[axis].is_some() {
                    cell[axis] = cell[axis].min(explicit[axis] - 1);
                    cell[axis + 2] = cell[axis + 2].min(explicit[axis] - cell[axis]);
                }
            }
            cell
        })
        .collect();
    let mut definitions: [Vec<Track>; 2] = [0, 1].map(|axis| {
        let count = if shared[axis].is_some() {
            explicit[axis]
        } else {
            count[axis].min(4096)
        };
        (0..count)
            .map(|i| {
                defined[axis].get(i).copied().unwrap_or_else(|| {
                    let list = &automatic[axis];
                    list.get((i - defined[axis].len()) % list.len().max(1))
                        .copied()
                        .unwrap_or((Auto, Auto))
                })
            })
            .collect()
    });
    for axis in 0..2 {
        let Some((start, length)) = fit[axis] else {
            continue;
        };
        let mut used: Vec<bool> = (0..definitions[axis].len())
            .map(|i| !(start..start + length).contains(&i))
            .collect();
        for cell in &cells {
            if let Some(range) = used.get_mut(cell[axis]..cell[axis] + cell[axis + 2]) {
                range.fill(true);
            }
        }
        let mut next = 0;
        let remap: Vec<usize> = used
            .iter()
            .map(|&used| {
                next += usize::from(used);
                next - usize::from(used)
            })
            .collect();
        for cell in &mut cells {
            cell[axis] = remap.get(cell[axis]).copied().unwrap_or(0);
        }
        definitions[axis] = definitions[axis]
            .iter()
            .zip(&used)
            .filter(|(_, used)| **used)
            .map(|(track, _)| *track)
            .collect();
    }
    let minimum = |child: usize, axis: usize, value: f32| {
        let style = &styles.values[child];
        if style
            .get([Prop::OVERFLOW_X, Prop::OVERFLOW_Y][axis])
            .or_else(|| style.get(Prop::OVERFLOW))
            .is_some_and(|v| !matches!(v, "visible" | "clip"))
            || style
                .get([Prop::MIN_WIDTH, Prop::MIN_HEIGHT][axis])
                .is_some_and(|v| v == "0" || v == "0px")
        {
            0.
        } else {
            value
        }
    };
    // WARNING: large grids have thousands of tracks, so track starts are summed once, in track order.
    let starts = |sizes: &[f32]| {
        let mut sum = 0f32;
        std::iter::once(0.)
            .chain(sizes.iter().map(|size| {
                sum += size;
                sum
            }))
            .collect::<Vec<_>>()
    };
    let area = |sizes: &[f32], starts: &[f32], offset: f32, gap: f32, start: usize, span: usize| {
        [
            offset + starts[start.min(sizes.len())] + start as f32 * gap,
            sizes.iter().skip(start).take(span).sum::<f32>() + span.saturating_sub(1) as f32 * gap,
        ]
    };
    let mut grid = Grid {
        gap,
        ..Grid::default()
    };
    let mut offsets = [0.; 2];
    let mut boxes = vec![[0f32; 4]; cells.len()];
    // How far baseline alignment moves each item down, or up for the last baseline.
    let mut shims = Vec::new();
    for axis in 0..2 {
        if axis == 1 {
            let column_starts = starts(&grid.columns);
            let mut aligned = Vec::new();
            for (index, (&child, cell)) in children.iter().zip(&cells).enumerate() {
                let [x, area] = area(
                    &grid.columns,
                    &column_starts,
                    offsets[0],
                    grid.gap[0],
                    cell[0],
                    cell[2],
                );
                let [min, max, used, height] =
                    intrinsic(doc, child, styles, Some(area), None, text, measured);
                let fraction = derived[index].alignment[0];
                let size = if fraction.is_some() && derived[index].auto_width {
                    max.min(area).max(min)
                } else {
                    used
                };
                let height = if size == used {
                    height
                } else {
                    intrinsic(doc, child, styles, Some(size), None, text, measured)[3]
                };
                boxes[index] = [x + (area - size) * fraction.unwrap_or(0.), 0., size, height];
                // Items in a row align their margin box's distance to the first baseline, or from the last
                // baseline to the bottom. Without a baseline, the bottom border edge is used.
                if let (Some(last), Some(&[first, end])) = (
                    derived[index].baseline,
                    baselines.and_then(|baselines| baselines.get(&child)),
                ) {
                    let [top, _, bottom, _] =
                        styles.lengths[child].edges(&styles.values[child], style::MARGIN, area);
                    let border = height - top - bottom;
                    let offset = if last {
                        bottom + border - end.unwrap_or(border)
                    } else {
                        top + first.unwrap_or(border)
                    };
                    let row = if last { cell[1] + cell[3] - 1 } else { cell[1] };
                    aligned.push((index, (row, last), offset));
                }
            }
            if !aligned.is_empty() {
                let mut shared = rustc_hash::FxHashMap::default();
                for &(_, key, offset) in &aligned {
                    let max = shared.entry(key).or_insert(offset);
                    *max = offset.max(*max);
                }
                shims = vec![0.; cells.len()];
                for (index, key, offset) in aligned {
                    shims[index] = shared[&key] - offset;
                }
            }
        }
        let mut contributions = Vec::new();
        for (index, (&child, cell)) in children.iter().zip(&cells).enumerate() {
            if shared[axis].is_some() {
                break;
            }
            if !derived[index].subgrid[axis] {
                let [min, max] = if axis == 0 {
                    let value = intrinsic(doc, child, styles, None, None, text, measured);
                    [value[0], value[1]]
                } else {
                    [boxes[index][3] + shims.get(index).copied().unwrap_or(0.); 2]
                };
                contributions.push((
                    cell[axis],
                    cell[axis + 2],
                    [if derived[index].shrink[axis] { 0. } else { min }, min, max],
                ));
                continue;
            }
            let mut offset = 0;
            for &grandchild in &doc.nodes[child].children {
                let style = &styles.values[grandchild];
                if style.get(Prop::DISPLAY).is_some_and(|v| v == "none")
                    || doc.nodes[grandchild].tag == "#text"
                        && doc.nodes[grandchild].text.trim_ascii().is_empty()
                {
                    continue;
                }
                let start = cell[axis] + offset % cell[axis + 2];
                let span = style
                    .get([Prop::GRID_COLUMN, Prop::GRID_ROW][axis])
                    .and_then(|v| v.trim().strip_prefix("span"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, cell[axis] + cell[axis + 2] - start);
                let value = intrinsic(
                    doc,
                    grandchild,
                    styles,
                    (axis == 1).then_some(boxes[index][2]),
                    None,
                    text,
                    measured,
                );
                let [min, max] = if axis == 0 {
                    [value[0], value[1]]
                } else {
                    [value[3]; 2]
                };
                contributions.push((start, span, [minimum(grandchild, axis, min), min, max]));
                offset += span;
            }
        }
        let content = get([Prop::JUSTIFY_CONTENT, Prop::ALIGN_CONTENT][axis])
            .or_else(|| pair(Prop::PLACE_CONTENT, axis))
            .unwrap_or("normal");
        let sizes = size(
            &definitions[axis],
            &contributions,
            gap[axis],
            available[axis],
            matches!(content, "normal" | "stretch"),
            axis == 0 && doc.nodes[id].tag == "table",
        );
        if let Some(space) = available[axis]
            && !sizes.is_empty()
        {
            let count = sizes.len() as f32;
            let free = space - sizes.iter().sum::<f32>() - (count - 1.) * gap[axis];
            let (offset, extra) = match content.split_whitespace().last().unwrap_or(content) {
                _ if free < 0. && content.starts_with("safe") => (0., 0.),
                "center" => (free / 2., 0.),
                "end" | "flex-end" | "right" => (free, 0.),
                "space-between" if free > 0. && count > 1. => (0., free / (count - 1.)),
                "space-around" if free > 0. => (free / count / 2., free / count),
                "space-evenly" if free > 0. => (free / (count + 1.), free / (count + 1.)),
                "space-around" | "space-evenly" => (free / 2., 0.),
                _ => (0., 0.),
            };
            offsets[axis] = offset;
            grid.gap[axis] += extra;
        }
        if axis == 0 {
            grid.columns = sizes;
        } else {
            grid.rows = sizes;
        }
    }
    let row_starts = starts(&grid.rows);
    let extent = available[0].unwrap_or_else(|| {
        grid.columns.iter().sum::<f32>()
            + grid.gap[0] * grid.columns.len().saturating_sub(1) as f32
            + offsets[0] * 2.
    });
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    for (index, (&child, cell)) in children.iter().zip(&cells).enumerate() {
        let [y, area] = area(
            &grid.rows,
            &row_starts,
            offsets[1],
            grid.gap[1],
            cell[1],
            cell[3],
        );
        let [x, _, width, mut height] = boxes[index];
        let fraction = derived[index].alignment[1];
        let value = derived[index].height;
        if value.is_some_and(|v| v.contains('%')) {
            height = intrinsic(doc, child, styles, Some(width), Some(area), text, measured)[3];
        }
        if fraction.is_none() && value.is_none_or(|v| v == "auto") {
            height = area;
        }
        let shim = shims.get(index).copied().unwrap_or(0.);
        grid.cells.push((
            child,
            *cell,
            [
                if rtl { extent - x - width } else { x },
                y + (area - height) * fraction.unwrap_or(0.)
                    + if derived[index].baseline == Some(true) {
                        -shim
                    } else {
                        shim
                    },
                width,
                height,
            ],
            derived[index].baseline.is_some(),
        ));
    }
    // Like Chrome, the grid's first baseline is the shared one of the first row's items that align their first
    // baselines, else the first item's in that row. An item without one gives its bottom border edge.
    let top = cells.iter().map(|cell| cell[1]).min();
    if let Some((index, cell)) = cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| Some(cell[1]) == top)
        .min_by_key(|(index, cell)| (derived[*index].baseline != Some(false), cell[0]))
    {
        let (child, _, [_, y, _, height], _) = grid.cells[index];
        grid.first_item = Some(child);
        if let Some(&[first, _]) = baselines.and_then(|baselines| baselines.get(&child)) {
            let area = grid.columns.iter().skip(cell[0]).take(cell[2]).sum::<f32>()
                + cell[2].saturating_sub(1) as f32 * grid.gap[0];
            let [top, _, bottom, _] =
                styles.lengths[child].edges(&styles.values[child], style::MARGIN, area);
            grid.first_baseline = Some(y + top + first.unwrap_or(height - top - bottom));
        }
    }
    grid.cells.sort_unstable_by_key(|(child, ..)| *child);
    grid
}

#[cfg(test)]
mod tests {
    use super::{Sizing::*, *};

    fn sized(
        template: &str,
        items: &[(usize, usize, [f32; 3])],
        gap: f32,
        available: Option<f32>,
    ) -> Vec<f32> {
        let (list, ..) = tracks(template, &Lengths::new(16., 800., 600.), available, gap);
        size(&list, items, gap, available, true, false)
            .into_iter()
            .map(|v| (v * 100.).round() / 100.)
            .collect()
    }

    #[test]
    fn flexible_tracks_share_space_equally_regardless_of_content() {
        let items = [
            (0, 1, [5., 5., 5.]),
            (1, 1, [5., 5., 5.]),
            (2, 1, [20., 20., 40.]),
        ];
        assert_eq!(sized("repeat(3, 1fr)", &items, 4., Some(160.)), [50.67; 3]);
        assert_eq!(
            sized("minmax(50px, 1fr) 1fr", &items[..2], 4., Some(160.)),
            [78., 78.]
        );
        assert_eq!(sized("40px 1fr 2fr", &[], 0., Some(160.)), [40., 40., 80.]);
    }

    #[test]
    fn flexible_tracks_never_shrink_below_their_minimum_contribution() {
        let items = [(0, 1, [80., 80., 120.])];
        assert_eq!(sized("1fr 1fr", &items, 0., Some(100.)), [80., 20.]);
        assert_eq!(
            sized("minmax(0, 1fr) 1fr", &items, 0., Some(100.)),
            [50., 50.]
        );
    }

    #[test]
    fn auto_tracks_take_max_content_before_flexible_tracks() {
        let items = [(0, 1, [30., 30., 70.]), (1, 1, [10., 10., 20.])];
        assert_eq!(sized("auto 1fr", &items, 4., Some(160.)), [70., 86.]);
        assert_eq!(sized("auto auto", &items, 0., Some(200.)), [125., 75.]);
        assert_eq!(
            sized("min-content 1fr", &items, 0., Some(160.)),
            [30., 130.]
        );
        assert_eq!(
            sized("max-content max-content", &items, 0., Some(10.)),
            [70., 20.]
        );
    }

    #[test]
    fn fit_content_clamps_growth_limits() {
        assert_eq!(
            sized(
                "fit-content(50px) 1fr",
                &[(0, 1, [20., 20., 80.])],
                0.,
                Some(200.)
            ),
            [50., 150.]
        );
        assert_eq!(
            sized(
                "fit-content(50px) 1fr",
                &[(0, 1, [20., 20., 30.])],
                0.,
                Some(200.)
            ),
            [30., 170.]
        );
    }

    #[test]
    fn spanning_items_grow_the_tracks_they_cross() {
        let items = [(0, 2, [10., 10., 104.]), (0, 1, [20., 20., 20.])];
        assert_eq!(sized("auto auto", &items[..1], 4., None), [50., 50.]);
        assert_eq!(sized("auto auto", &items, 4., None), [20., 80.]);
        assert_eq!(
            sized("40px auto", &[(0, 2, [100., 100., 100.])], 0., None),
            [40., 60.]
        );
    }

    #[test]
    fn indefinite_flexible_tracks_use_the_largest_fraction() {
        let items = [(0, 1, [10., 10., 30.]), (1, 1, [10., 10., 30.])];
        assert_eq!(sized("1fr 2fr", &items, 0., None), [30., 60.]);
        assert_eq!(
            sized(
                "1fr 1fr",
                &[(0, 1, [0., 0., 20.]), (1, 1, [0., 0., 50.])],
                0.,
                None
            ),
            [50., 50.]
        );
    }

    #[test]
    fn min_content_sizing_keeps_base_sizes() {
        let items = [(0, 1, [30., 30., 70.]), (1, 1, [10., 10., 20.])];
        assert_eq!(sized("auto 1fr", &items, 0., Some(0.)), [30., 10.]);
    }

    #[test]
    fn auto_repeat_fills_the_available_space() {
        let units = Lengths::new(16., 800., 600.);
        let (list, _, fit) = tracks(
            "repeat(auto-fill, minmax(45px, 1fr))",
            &units,
            Some(160.),
            4.,
        );
        assert_eq!((list.len(), fit), (3, None));
        let (list, _, fit) = tracks("40px repeat(auto-fit, 50px) 10px", &units, Some(200.), 0.);
        assert_eq!((list.len(), fit), (5, Some((1, 3))));
        assert_eq!(list[0], (Fixed(40.), Fixed(40.)));
        assert_eq!(
            tracks("repeat(auto-fill, 1fr)", &units, Some(160.), 0.)
                .0
                .len(),
            1
        );
        assert_eq!(tracks("repeat(99999, 1px)", &units, None, 0.).0.len(), 4096);
        assert_eq!(tracks("25% 1fr", &units, None, 0.).0[0], (Auto, Auto));
    }

    #[test]
    fn placement_resolves_numbers_names_and_spans() {
        let names = [
            ("a".to_owned(), 0),
            ("b".to_owned(), 1),
            ("c".to_owned(), 2),
            ("h-start".to_owned(), 0),
            ("h-end".to_owned(), 2),
        ];
        assert_eq!(placement("b", "c", &names, 2), (Some(1), 1));
        assert_eq!(placement("h", "h", &names, 2), (Some(0), 2));
        assert_eq!(placement("1", "-1", &names, 3), (Some(0), 3));
        assert_eq!(placement("span 2", "auto", &names, 3), (None, 2));
        assert_eq!(placement("2", "span 2", &names, 3), (Some(1), 2));
        assert_eq!(placement("span 2", "3", &names, 3), (Some(0), 2));
        assert_eq!(placement("a", "span c", &names, 3), (Some(0), 2));
        assert_eq!(placement("missing", "auto", &names, 3), (None, 1));
        assert_eq!(placement("auto", "span 3", &names, 3), (None, 3));
        assert_eq!(placement("99999", "auto", &names, 3), (Some(4095), 1));
        assert_eq!(placement("1", "span 99999", &names, 3), (Some(0), 4096));
    }

    #[test]
    fn fields_and_dotted_area_rows_size_the_grid() {
        let text = renderer::TextSystem::new();
        let mut doc = Document::parse(
            "<style>main{display:grid;grid-template-columns:repeat(4,auto);grid-template-areas:'t f m n''.r a.'}div{display:flex;width:200px}input{width:100%;line-height:24px;padding:0 6px;border:1px solid}</style><main><b style='grid-area:t'></b><div style='grid-area:f'><input></div><div style='grid-area:r'><input></div><b style='grid-area:a'></b></main>",
        );
        let mut state = crate::dom::State::default();
        let styles = crate::dom::styles(&mut doc, &mut state, [800., 600.], None);
        let main = doc
            .nodes
            .iter()
            .position(|node| node.tag == "main")
            .unwrap();
        let grid = layout(
            &doc,
            main,
            &styles,
            Some(800.),
            None,
            None,
            None,
            &text,
            &mut Default::default(),
        );
        // The inputs are 24px of text and 2px of border tall, and `.r a.` is four cells.
        assert_eq!(grid.rows, [26., 26.]);
        let cells: Vec<_> = grid.cells.iter().map(|(_, cell, ..)| *cell).collect();
        assert_eq!(cells[2..], [[1, 1, 1, 1], [2, 1, 1, 1]]);
    }

    #[test]
    fn aspect_ratio_children_give_rows_their_height() {
        let text = renderer::TextSystem::new();
        let mut doc = Document::parse(
            "<style>main{display:grid;grid-template-columns:300px 200px}i{display:block;aspect-ratio:5/4}.b{box-sizing:border-box;padding:10px}.c{overflow:hidden}</style><main><div><i style='width:100%'></i></div><div><i class=b></i><p style='height:20px;margin:0'></p></div></main><main><div><i class=c style='aspect-ratio:10'><p style='height:90px;margin:0'></p></i></div><div><i style='aspect-ratio:10'><p style='height:90px;margin:0'></p></i></div></main>",
        );
        let mut state = crate::dom::State::default();
        let styles = crate::dom::styles(&mut doc, &mut state, [800., 600.], None);
        let rows: Vec<_> = (0..doc.nodes.len())
            .filter(|&id| doc.nodes[id].tag == "main")
            .map(|main| {
                layout(
                    &doc,
                    main,
                    &styles,
                    Some(800.),
                    None,
                    None,
                    None,
                    &text,
                    &mut Default::default(),
                )
                .rows
            })
            .collect();
        // 300px / 1.25, and a 200px border box gives 160px; a clipping box keeps 30px, others fit their content.
        assert_eq!(rows, [vec![240.], vec![90.]]);
    }

    #[test]
    fn dense_packing_fills_holes_and_stays_bounded() {
        let text = renderer::TextSystem::new();
        let place = |html: &str| {
            let mut doc = Document::parse(html);
            let mut state = crate::dom::State::default();
            let styles = crate::dom::styles(&mut doc, &mut state, [800., 600.], None);
            let grid = doc
                .nodes
                .iter()
                .position(|node| node.tag == "main")
                .unwrap();
            layout(
                &doc,
                grid,
                &styles,
                Some(800.),
                None,
                None,
                None,
                &text,
                &mut Default::default(),
            )
            .cells
            .into_iter()
            .map(|(_, cell, ..)| cell)
            .collect::<Vec<_>>()
        };
        let cells = place(
            "<style>main{display:grid;grid-template-columns:repeat(3,10px);grid-auto-flow:row dense}.w{grid-column:span 2}</style><main><i class=w></i><i class=w></i><i></i><i></i></main>",
        );
        assert_eq!(
            cells,
            [[0, 0, 2, 1], [0, 1, 2, 1], [2, 0, 1, 1], [2, 1, 1, 1]]
        );
        let cells = place(&format!(
            "<style>main{{display:grid;grid-template-columns:repeat(4096,1px);grid-auto-flow:dense}}i{{grid-column:span 4096}}</style><main>{}</main>",
            "<i></i>".repeat(3000)
        ));
        assert!(
            cells
                .iter()
                .enumerate()
                .all(|(row, cell)| *cell == [0, row, 4096, 1])
        );
    }
}
