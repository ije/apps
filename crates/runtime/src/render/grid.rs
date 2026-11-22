use crate::style::Prop;
use crate::{
    dom::Document,
    style::{self, ComputedStyles, Lengths},
};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct Measurements {
    // The last key field marks the column and row extents of a grid layout, which intrinsic sizes repeat.
    #[allow(clippy::type_complexity)]
    pub(crate) sizes: rustc_hash::FxHashMap<(usize, Option<u32>, Option<u32>, bool), [f32; 4]>,
    // The top and bottom margins of each measured box, as [positive, negative] after they collapse with the
    // margins of children that they adjoin.
    #[allow(clippy::type_complexity)]
    pub(crate) margins: rustc_hash::FxHashMap<(usize, Option<u32>, Option<u32>), [[f32; 2]; 2]>,
    // How far the first line of a text box reaches above its strut, by the keys of `sizes`.
    #[allow(clippy::type_complexity)]
    pub(crate) leads: rustc_hash::FxHashMap<(usize, Option<u32>, Option<u32>), f32>,
    pub(crate) pixel_ratio: f32,
    pub(crate) naturals: rustc_hash::FxHashMap<usize, [f32; 2]>,
    pub(crate) texts: rustc_hash::FxHashMap<usize, Option<std::rc::Rc<style::InlineText>>>,
    // The text nodes in runs of more than one in flex and grid containers: `None` for a later node, which the
    // first builds, and for the first, whether the run has text other than white space.
    pub(crate) joined: rustc_hash::FxHashMap<usize, Option<bool>>,
    // The containers whose runs this build noted.
    pub(crate) noted: rustc_hash::FxHashSet<usize>,
    // Use style-address keys only during grid layout, while styles outlive the map.
    #[allow(clippy::type_complexity)]
    pub(crate) edges: Option<rustc_hash::FxHashMap<(*const style::Style, u32), [[f32; 4]; 3]>>,
    // The boxes with `content-visibility: auto`, true while they are near the view and show their contents.
    pub(crate) shown: rustc_hash::FxHashMap<usize, bool>,
    // For `contain-intrinsic-size: auto`: the last content size of a box that showed its contents, and whether it
    // skipped them at its last build.
    pub(crate) remembered: rustc_hash::FxHashMap<usize, (Option<[f32; 2]>, bool)>,
    // Boxes that show their contents until the next frame, because a script reads the geometry of a node in them.
    pub(crate) forced: rustc_hash::FxHashSet<usize>,
    // Boxes that start or stop to skip their contents, which the next build builds again.
    pub(crate) waking: Vec<usize>,
}

impl Measurements {
    /// Keeps the measured sizes, margins and leads of the boxes that `keep` takes.
    pub(crate) fn retain(&mut self, keep: impl Fn(usize) -> bool) {
        self.sizes.retain(|&(id, ..), _| keep(id));
        self.margins.retain(|&(id, ..), _| keep(id));
        self.leads.retain(|&(id, ..), _| keep(id));
    }

    /// Like CSS, each run of text nodes in a flex or grid container is one anonymous item. Its first node builds it
    /// with the text of all. `again` notes the runs of `children` also when this build noted them.
    pub(crate) fn note_runs(
        &mut self,
        doc: &Document,
        styles: &ComputedStyles,
        container: usize,
        children: &[usize],
        again: bool,
    ) {
        if !self.noted.insert(container) && !again {
            return;
        }
        let mut at = 0;
        while at < children.len() {
            if doc.nodes[children[at]].tag != "#text" {
                at += 1;
                continue;
            }
            let end = at
                + children[at..]
                    .iter()
                    .take_while(|&&node| {
                        matches!(doc.nodes[node].tag.as_str(), "#text" | "#comment")
                    })
                    .count();
            let run: Vec<usize> = children[at..end]
                .iter()
                .copied()
                .filter(|&node| doc.nodes[node].tag == "#text")
                .collect();
            if let [first, rest @ ..] = &run[..]
                && !rest.is_empty()
            {
                let text = run
                    .iter()
                    .any(|&node| !doc.nodes[node].text.trim_ascii().is_empty());
                self.joined.insert(*first, Some(text));
                self.joined.extend(rest.iter().map(|&node| (node, None)));
                self.texts.insert(
                    *first,
                    style::inline_text(doc, styles, *first, rest, &[], [true; 2])
                        .map(std::rc::Rc::new),
                );
            } else {
                self.joined.remove(&children[at]);
            }
            at = end;
        }
    }

    /// Whether a text child of a flex or grid container makes an item: not for a later node of a run, and for
    /// white space only with `preserve` or other text in its run.
    pub(crate) fn text_item(&self, doc: &Document, child: usize, preserve: bool) -> bool {
        let blank = doc.nodes[child].text.trim_ascii().is_empty();
        match self.joined.get(&child) {
            Some(None) => false,
            Some(&Some(text)) => text || preserve || !blank,
            None => preserve || !blank,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Grid {
    pub columns: Vec<f32>,
    pub rows: Vec<f32>,
    pub cells: Vec<(usize, [usize; 4], [f32; 4], bool)>,
    pub gap: [f32; 2],
    pub baseline: bool,
    pub first_item: Option<usize>,
    pub first_baseline: Option<f32>,
    // The height of the content when it is not the sum of the rows: flexible rows that sum to less than `1fr`
    // size again in the height that they first gave the grid.
    pub height: Option<f32>,
}

impl Grid {
    /// The size of the tracks with their gaps in an axis, or the height of the content.
    pub(crate) fn extent(&self, axis: usize) -> f32 {
        let tracks = [&self.columns, &self.rows][axis];
        self.height.filter(|_| axis == 1).unwrap_or_else(|| {
            tracks.iter().sum::<f32>() + tracks.len().saturating_sub(1) as f32 * self.gap[axis]
        })
    }

    /// The width and height of the grid area of a cell placement: column, row, columns, rows.
    pub(crate) fn area(&self, place: [usize; 4]) -> [f32; 2] {
        [0, 1].map(|axis| {
            let span = place[axis + 2];
            [&self.columns, &self.rows][axis]
                .get(place[axis]..place[axis] + span)
                .map_or(0., |tracks| tracks.iter().sum())
                + self.gap[axis] * span.saturating_sub(1) as f32
        })
    }
}

// The content size of a box that skips its contents: its `contain-intrinsic-size`, or the size it remembers for
// `auto`.
pub(crate) fn intrinsic_size(
    style: &style::Style,
    units: &Lengths,
    remembered: Option<[f32; 2]>,
) -> [f32; 2] {
    [0, 1].map(|axis| {
        let value = style
            .get(
                [
                    Prop::CONTAIN_INTRINSIC_WIDTH,
                    Prop::CONTAIN_INTRINSIC_HEIGHT,
                ][axis],
            )
            .unwrap_or("none");
        match value.strip_prefix("auto") {
            Some(length) => remembered
                .map(|size| size[axis])
                .or_else(|| units.resolve(length.trim(), 0.)),
            None => units.resolve(value, 0.),
        }
        .unwrap_or(0.)
    })
}

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

pub(super) fn placement(
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
    // Like Chrome, rows of an auto height are laid out, not sized under a content constraint, so their auto
    // minimums take the minimum contributions of the items.
    rows: bool,
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
    let mut planned = vec![None::<f32>; count];
    let (mut targets, mut weights, mut caps, mut incurred, mut open) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
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
            let cap = |i: usize| match (growing, max(i)) {
                (true, _) if limit[i].is_finite() && !growable[i] => 0.,
                (true, FitContent(size)) => (size - current(i)).max(0.),
                (true, _) => f32::INFINITY,
                (false, FitContent(size)) => (limit[i].min(size) - base[i]).max(0.),
                (false, _) => (limit[i] - base[i]).max(0.),
            };
            let beyond = |i: usize| match (growing, step) {
                (true, _) => !matches!(max(i), FitContent(_)),
                (false, 2) => matches!(max(i), MaxContent | Auto | FitContent(_)),
                _ => matches!(max(i), MinContent | MaxContent | Auto | FitContent(_)),
            };
            planned.fill(None);
            for &&(start, span, contribution) in items {
                targets.clear();
                targets.extend((start..start + span).filter(|&i| {
                    (!flexible || factor(i).is_some())
                        && match step {
                            0 => matches!(min(i), MinContent | MaxContent | Auto),
                            1 => matches!(min(i), MinContent | MaxContent),
                            2 => {
                                min(i) == MaxContent
                                    || available.is_none() && !rows && min(i) == Auto
                            }
                            4 => {
                                matches!(max(i), MinContent | MaxContent | Auto | FitContent(_))
                            }
                            _ => matches!(max(i), MaxContent | Auto | FitContent(_)),
                        }
                }));
                if targets.is_empty() {
                    continue;
                }
                let value = match step {
                    0 if available.is_none() && !rows => contribution[2]
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
                if let [i] = targets[..]
                    && !flexible
                {
                    let first = if space > 1e-3 { space.min(cap(i)) } else { 0. };
                    let rest = space - first;
                    let incurred = if (beyond(i) || !growing) && rest > 1e-3 {
                        first + rest
                    } else {
                        first
                    };
                    planned[i] = Some(planned[i].unwrap_or(0.).max(incurred));
                    continue;
                }
                weights.clear();
                weights.extend(targets.iter().map(|&i| factor(i).unwrap_or(1.)));
                let uniform = weights.iter().sum::<f32>() <= 0.;
                caps.clear();
                caps.extend(targets.iter().map(|&i| cap(i)));
                incurred.clear();
                incurred.resize(targets.len(), 0f32);
                for pass in 0..2 {
                    if pass == 1 {
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
                        open.clear();
                        open.extend((0..targets.len()).filter(|&k| incurred[k] < caps[k]));
                        let total: f32 = open.iter().map(|&k| weight(k)).sum();
                        if space <= 1e-3 || total <= 0. {
                            break;
                        }
                        let share = space / total;
                        for &k in &open {
                            let add = (share * weight(k)).min(caps[k] - incurred[k]);
                            incurred[k] += add;
                            space -= add;
                        }
                    }
                }
                for (&i, &incurred) in targets.iter().zip(&incurred) {
                    planned[i] = Some(planned[i].unwrap_or(0.).max(incurred));
                }
            }
            for (i, &planned) in planned.iter().enumerate() {
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
        let mut free = space - base.iter().sum::<f32>() - gaps(count);
        // Like Chrome, columns with a width grow to it before the others grow past their min-content width.
        for sized in [true, false] {
            let grows = |i: &usize| matches!(max(*i), Fixed(_)) == sized;
            let room: f32 = (0..count).filter(grows).map(|i| limit[i] - base[i]).sum();
            if free > 0. && room > 0. {
                let share = (free / room).min(1.);
                for i in (0..count).filter(grows) {
                    let add = (limit[i] - base[i]) * share;
                    base[i] += add;
                    free -= add;
                }
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

// The renderer ceils each measured leaf to layout units, so sums of leaves must use the same widths.
fn leaf_ceil(width: f32, pixel_ratio: f32) -> f32 {
    match pixel_ratio > 0. {
        true => super::element::layout_ceil(width, pixel_ratio),
        false => width,
    }
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
    if let Some(value) = measured.sizes.get(&(key.0, key.1, key.2, false)) {
        return *value;
    }
    // Clear before this measure, so the lead that it records stays with its size.
    if measured.sizes.len() > 65536 {
        measured.retain(|_| false);
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
    let edges = || {
        [
            units.edges(style, style::PADDING, base),
            units.edges(style, style::MARGIN, base),
            units.borders(style, base),
        ]
    };
    let [padding, margin, border] = match &mut measured.edges {
        Some(memo) => *memo
            .entry((std::rc::Rc::as_ptr(style), base.to_bits()))
            .or_insert_with(edges),
        None => edges(),
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
    let alt = super::element::broken_image(node)
        .then(|| node.attrs.get("alt").filter(|alt| !alt.is_empty()))
        .flatten();
    let sizeless = alt.is_some()
        && style.get(Prop::DISPLAY).is_none_or(|v| {
            v == "inline" || v == "inline-block" && styles.flags[id] & style::AGENT_DISPLAY != 0
        });
    let sizes = |props: [Prop; 3], base: Option<f32>| {
        let axis = usize::from(props[0] == Prop::HEIGHT);
        props.map(|prop| {
            style
                .get(prop)
                .filter(|_| !(sizeless && matches!(prop, Prop::WIDTH | Prop::HEIGHT)))
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
    // A table cell is as wide as its columns.
    let cell = style::role(styles, id) == style::CELL;
    let available = used[0]
        .filter(|_| !cell || width.is_none())
        .or(width.map(|width| width - outer[0]))
        .or(fixed[0])
        .map(|width| clamp(width, used).max(0.));
    // Content-sized widths measure the content without a width first.
    let available = match (style.get(Prop::WIDTH), width) {
        (Some(value @ ("min-content" | "max-content" | "fit-content")), Some(width)) => {
            let [min, max, ..] = intrinsic(doc, id, styles, None, None, text, measured);
            let size = match value {
                "min-content" => min,
                "max-content" => max,
                _ => max.min(width.max(min)),
            };
            Some(clamp((size - outer[0]).max(0.), used))
        }
        _ => available,
    };
    let vertical = sizes([Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT], height);
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
        .then(|| super::element::natural_size(doc, id, measured.naturals.get(&id).copied(), style))
        .flatten();
    let skipped = matches!(style.get(Prop::CONTENT_VISIBILITY), Some("hidden" | "auto"))
        && super::cull::skips(
            u32::from(super::cull::skippable(&node.tag, style)) * super::cull::SKIPS,
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
            node.attrs
                .get(["width", "height"][axis])
                .filter(|_| {
                    matches!(node.tag.as_str(), "img" | "video")
                        && style.get([Prop::WIDTH, Prop::HEIGHT][axis]).is_none()
                })
                .and_then(|v| super::element::dimension(v, false))
                .map(|v| (v - if border_box { inner[axis] } else { 0. }).max(0.))
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
                node.attrs
                    .get(["width", "height"][axis])
                    .and_then(|v| super::element::dimension(v, false))
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
    } else if super::element::text_field(node, style) {
        let [width, height] = super::element::field_size(
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
            .scrollbars
            .get(&id)
            .and_then(|parts| parts[1].as_ref())
    {
        let [width, height] =
            super::element::file_size(style, button, units, text, measured.pixel_ratio);
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
            text.shape_line(label, renderer::px(units.font.em), &run)
                .line
                .width,
        );
        let ceiled = leaf_ceil(width, measured.pixel_ratio);
        content = [ceiled, ceiled, units.font.lh];
        if node.native_control(style).is_some() {
            let [padding, height] = super::native::size(renderer::ControlKind::Button);
            let ceiled = leaf_ceil(width + padding, measured.pixel_ratio);
            content = [ceiled, ceiled, height];
        }
    } else if let Some((source, mut runs, atoms, heights, mixed, pseudos)) = match alt {
        Some(alt) => Some((
            renderer::SharedString::from(alt.clone()),
            vec![style::text_run(style, units, alt.len())],
            Vec::new(),
            Vec::new(),
            false,
            [false; 2],
        )),
        None => measured
            .texts
            .entry(id)
            .or_insert_with(|| {
                style::inline_text(doc, styles, id, &[], &[], [true; 2]).map(std::rc::Rc::new)
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
                )
            }),
    } {
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
                    .is_some_and(|v| v.contains('%'));
            runs[atom.run].inset[1] = renderer::px(if percent { used } else { max });
            // Like Chrome, a replaced box sits on the baseline, and other boxes on their last line.
            let baseline = if super::element::natural_size(
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
                    lengths.edges(own, style::MARGIN, available.unwrap_or(0.));
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
        let shape = |runs: &[renderer::TextRun]| {
            text.shape_text(
                &source,
                renderer::px(units.font.em),
                runs,
                &renderer::Wrapping {
                    width: available.filter(|_| wrap).map(renderer::px),
                    max_lines: style
                        .get(Prop::WEBKIT_LINE_CLAMP)
                        .or_else(|| style.get(Prop::LINE_CLAMP))
                        .and_then(|value| value.parse::<usize>().ok())
                        .map(|lines| lines.max(1)),
                    word_break,
                    ..Default::default()
                },
            )
        };
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
        let lines = beside.unwrap_or_else(|| shape(&runs));
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
            for (row, [above, below]) in super::text::row_extents(
                &lines,
                &runs,
                &heights,
                &boxes,
                strut,
                text,
                measured.pixel_ratio,
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
        for line in &lines {
            let width = f32::from(line.line.width);
            content[1] = content[1].max(width);
            content[2] += line.rows() as f32 * units.font.lh;
            let mut glyphs = line.line.glyphs.iter().peekable();
            let (mut start, mut previous, mut hyphen) = (0., ' ', false);
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
                        start = x - opening
                            .iter()
                            .find(|(at, _)| *at == line_start + glyph.index as usize)
                            .map_or(0., |(_, inset)| *inset);
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
        measured.leads.insert(key, lead + top);
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
        let mut extent = |width: Option<f32>, height: Option<f32>| {
            let key = (id, width.map(f32::to_bits), height.map(f32::to_bits), true);
            if let Some(&[columns, rows, ..]) = measured.sizes.get(&key) {
                return [columns, rows];
            }
            let grid = layout(doc, id, styles, width, height, None, None, text, measured);
            let [columns, rows] = [0, 1].map(|axis| grid.extent(axis));
            measured.sizes.insert(key, [columns, rows, 0., 0.]);
            [columns, rows]
        };
        let [min, _] = extent(Some(0.), None);
        let [max, natural] = extent(None, None);
        content = [min, max, natural];
        if available.is_some() || vertical[0].is_some() {
            let fit = style.get(Prop::WIDTH) == Some("fit-content");
            let width = available.map(|width| if fit { width.min(max).max(min) } else { width });
            content[2] = extent(width, vertical[0])[1];
        }
    } else {
        // The content of spliced inline boxes is in the flow of the box.
        let mut kids = std::borrow::Cow::Borrowed(&node.children[..]);
        if !matches!(display, "flex" | "inline-flex")
            && node
                .children
                .iter()
                .any(|&child| style::spliced(doc, styles, child))
        {
            let (mut flat, mut pending) = (Vec::new(), node.children.clone());
            pending.reverse();
            while let Some(child) = pending.pop() {
                match style::spliced(doc, styles, child) {
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
                .map(|&child| {
                    doc.nodes[child].tag != "#text" || measured.text_item(doc, child, preserve)
                })
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
                let [min, _, auto, _] = intrinsic(doc, child, styles, None, None, text, measured);
                let frame = frame(own, lengths, space);
                let sides = frame[1] + frame[3];
                // A length sizes the border box with `box-sizing: border-box`, which keeps only the margins outside.
                let outer = |v: f32| {
                    if own.get(Prop::BOX_SIZING) == Some("border-box") {
                        let margin = lengths.edges(own, style::MARGIN, space);
                        (v + margin[1] + margin[3]).max(sides)
                    } else {
                        v + sides
                    }
                };
                let basis = own
                    .get(Prop::FLEX_BASIS)
                    .filter(|v| !matches!(*v, "auto" | "content"))
                    .and_then(|v| lengths.resolve(v, space))
                    .map_or(auto, outer);
                let lower = if own
                    .get(Prop::OVERFLOW_X)
                    .is_some_and(|v| !matches!(v, "visible" | "clip"))
                    || own.get(Prop::MIN_WIDTH).is_some_and(|v| v != "auto")
                {
                    own.get(Prop::MIN_WIDTH)
                        .and_then(|v| lengths.resolve(v, space))
                        .map_or(0., outer)
                } else {
                    min
                };
                let factor = |prop, fallback| {
                    own.get(prop)
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
        // How far the baseline-aligned items of a flex line reach above and below their shared baseline.
        let mut aligned = [0f32; 2];
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
                let [min, max, _, height] =
                    intrinsic(doc, child, styles, available, None, text, measured);
                content = [content[0].max(min), content[1].max(max), content[2]];
                let key = (child, available.map(f32::to_bits), None);
                let [above, below] = measured.margins.get(&key).copied().unwrap_or_default();
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
                        .is_some_and(|v| v.contains('%'))
                        || fills(
                            doc,
                            styles,
                            child,
                            super::element::natural_size(
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
                        text.shape_line(" ", renderer::px(units.font.em), &run)
                            .line
                            .width,
                    )
                })
            } else {
                0.
            };
            run += max + gap[0] + spacing;
            if starts.unwrap_or_else(|| {
                wrap && line[0] > 0.
                    && available
                        .is_some_and(|available| line[0] + gaps[0] + spacing + width > available)
            }) {
                content[2] += total(line, strut) + gaps[1];
                (line, aligned, strut) = ([0.; 4], [0.; 2], false);
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
                let baseline = |style, units: &style::Lengths| {
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
                        && styles.values[child]
                            .get(Prop::ALIGN_SELF)
                            .filter(|value| *value != "auto")
                            .or(style.get(Prop::ALIGN_ITEMS))
                            .is_some_and(|value| {
                                value.ends_with("baseline") && !value.starts_with("last")
                            })
                    {
                        let [above, _, below, _] =
                            child_units.edges(child_style, style::MARGIN, available.unwrap_or(0.));
                        // An item without a baseline sits on its bottom border edge.
                        let above = above
                            + first_baseline(doc, child, styles, at, text, measured)
                                .unwrap_or(height - above - below);
                        aligned = [aligned[0].max(above), aligned[1].max(height - above)];
                        line[1] = line[1].max(aligned[0] + aligned[1]);
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
                            || super::element::natural_size(
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
        // Like Chrome, an empty item keeps the line of its outside marker.
        let height = if display == "list-item"
            && style.get(Prop::LIST_STYLE_POSITION) != Some("inside")
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
            let [min, max, _, height] =
                intrinsic(doc, child, styles, available, None, text, measured);
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
            let pseudo = &styles.pseudos[&id][index];
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
        && !node.attrs.contains_key("multiple")
        && node
            .attrs
            .get("size")
            .is_none_or(|size| size.trim().parse::<u32>().is_ok_and(|size| size <= 1))
        && ![Prop::APPEARANCE, Prop::WEBKIT_APPEARANCE]
            .into_iter()
            .any(|prop| style.get(prop) == Some("none"))
    {
        let face = text.face(&style::font(style));
        let [ascent, descent, _] = text
            .line_metrics(face, renderer::px(units.font.em * measured.pixel_ratio))
            .map(f32::from);
        content[2] = (ascent + descent) / measured.pixel_ratio + 2.;
    }
    let [min, max] = [content[0], content[1]].map(|value| clamp(fixed[0].unwrap_or(value), fixed));
    let min = if cell {
        // Like Chrome, the width of a table cell raises its max-content width, not its min-content width.
        content[0].max(fixed[1].unwrap_or(0.)).min(min.max(max))
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
        .filter(|value| {
            !(value.contains("auto") && natural.is_some_and(|(_, ratio)| ratio.is_some()))
        })
        .and_then(super::element::aspect_ratio)
        .zip(available)
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
    let own = |value: f32| [value.max(0.), value.min(0.)];
    let sets = collapsed.unwrap_or([own(margin[0]), own(margin[2])]);
    let result = [
        min + outer[0],
        max + outer[0],
        match style.get(Prop::WIDTH) {
            Some("fit-content") => available.map_or(max, |width| max.min(min.max(width))),
            Some("max-content") => max,
            Some("min-content") => min,
            _ => available.unwrap_or(max),
        } + outer[0],
        clamp(vertical[0].or(ratio).unwrap_or(content[2]), vertical)
            + inner[1]
            + sets
                .iter()
                .map(|[positive, negative]| positive + negative)
                .sum::<f32>(),
    ];
    measured.sizes.insert((key.0, key.1, key.2, false), result);
    measured.margins.insert(key, sets);
    result
}

/// The first baseline of a box from the top of its border box, as layout places it: the first line of its text, or
/// the baseline of its first item. `None` for a box without one, which then sits on its bottom border edge.
fn first_baseline(
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
    let [margin, padding] =
        [style::MARGIN, style::PADDING].map(|sides| units.edges(style, sides, base));
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
        || super::element::natural_size(doc, id, measured.naturals.get(&id).copied(), style)
            .is_some()
    {
        return None;
    }
    // A control centers its line of text.
    if matches!(node.tag.as_str(), "input" | "select" | "textarea") {
        let inner = height - margin[0] - margin[2] - top - padding[2] - border[2];
        return Some(top + (inner - units.font.lh) / 2. + strut(measured));
    }
    if matches!(node.tag.as_str(), "progress" | "meter") {
        return None;
    }
    if node.tag.starts_with('#') || measured.texts.get(&id).is_some_and(Option::is_some) {
        let lead = measured.leads.get(&(id, width.map(f32::to_bits), None));
        return Some(top + lead.copied().unwrap_or(0.) + strut(measured));
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
        let [above, _, below, _] = lengths.edges(own, style::MARGIN, width.unwrap_or(inner));
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
            return Some(y + lengths.edges(own, style::MARGIN, inner)[0] + baseline);
        }
        y += intrinsic(doc, child, styles, Some(inner), None, text, measured)[3] + gap;
    }
    None
}

// A video without a frame, with a CSS ratio and an auto size, which fills the width that it gets.
fn fills(doc: &Document, styles: &ComputedStyles, id: usize, natural_ratio: Option<f32>) -> bool {
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
fn pseudo_size(
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
    let pseudo = &styles.pseudos.get(&id)?[index];
    let value = pseudo
        .get(Prop::CONTENT)
        .filter(|value| !matches!(*value, "none" | "normal"))?;
    if pseudo.get(Prop::DISPLAY) == Some("none")
        || pseudo
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "absolute" | "fixed"))
    {
        return None;
    }
    let units =
        super::element::pseudo_lengths(&styles.lengths[id], style, pseudo, text, pixel_ratio);
    let [padding, margin] =
        [style::PADDING, style::MARGIN].map(|edges| units.edges(pseudo, edges, base));
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
        size.min(max.unwrap_or(f32::INFINITY))
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
            f32::from(renderer::widest_piece(lines, renderer::px(0.))),
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
pub(super) fn pseudo_atom(
    doc: &Document,
    id: usize,
    index: usize,
    styles: &ComputedStyles,
    base: f32,
    available: Option<f32>,
    text: &renderer::TextSystem,
    pixel_ratio: f32,
) -> Option<[f32; 3]> {
    let [_, max, used, height] =
        pseudo_size(doc, id, index, styles, base, None, text, pixel_ratio)?;
    let pseudo = &styles.pseudos[&id][index];
    let units = super::element::pseudo_lengths(
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

/// Margin, border and padding of each side: top, right, bottom, left.
fn frame(style: &style::Style, units: &Lengths, base: f32) -> [f32; 4] {
    let [margin, padding] =
        [style::MARGIN, style::PADDING].map(|sides| units.edges(style, sides, base));
    std::array::from_fn(|side| {
        let [size, kind, _] = style::BORDERS[side];
        let border = if style
            .get(kind)
            .is_none_or(|v| matches!(v, "none" | "hidden"))
        {
            0.
        } else {
            style
                .get(size)
                .map_or(Some(3.), |v| units.resolve(v, base))
                .unwrap_or(0.)
        };
        margin[side] + padding[side] + border.max(0.)
    })
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
    if style
        .get(Prop::DISPLAY)
        .is_some_and(|value| value.ends_with("grid"))
    {
        measured.note_runs(doc, styles, id, &doc.nodes[id].children, false);
    }
    // An anonymous table is as wide as its content, inside its block.
    let block = width;
    let width = match width {
        Some(space) if space > 0. && matches!(style::role(styles, id), 13 | 14) => {
            let [min, max] = [Some(0.), None].map(|width| {
                let grid = layout(doc, id, styles, width, None, None, None, text, measured);
                grid.columns.iter().sum::<f32>()
                    + grid.columns.len().saturating_sub(1) as f32 * grid.gap[0]
            });
            Some(space.min(max).max(min))
        }
        width => width,
    };
    let available = [width, height];
    let get = |prop| style.get(prop);
    let outermost = measured.edges.is_none();
    if outermost {
        measured.edges = Some(Default::default());
    }
    let template = [
        get(Prop::GRID_TEMPLATE_COLUMNS),
        get(Prop::GRID_TEMPLATE_ROWS),
    ];
    let areas = get(Prop::GRID_TEMPLATE_AREAS);
    let flow = get(Prop::GRID_AUTO_FLOW).unwrap_or("row");
    let implicit =
        [get(Prop::GRID_AUTO_COLUMNS), get(Prop::GRID_AUTO_ROWS)].map(|v| v.unwrap_or("auto"));
    let shared = [0, 1].map(|axis| {
        inherited
            .map(|grid| if axis == 0 { &grid.columns } else { &grid.rows })
            .filter(|sizes| {
                !sizes.is_empty() && template[axis].is_some_and(|v| v.starts_with("subgrid"))
            })
    });
    let gap = [0, 1].map(|axis| {
        match (
            get([Prop::COLUMN_GAP, Prop::ROW_GAP][axis]).filter(|v| *v != "normal"),
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
    let area_rows: Vec<Vec<&str>> = areas
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
            // The parent tracks hold the subgrid's margin, border and padding at both ends.
            let frame = frame(style, units, width.unwrap_or(0.));
            let [start, end] = if axis == 0 { [3, 1] } else { [0, 2] }.map(|side| frame[side]);
            let last = sizes.len() - 1;
            defined[axis] = sizes
                .iter()
                .enumerate()
                .map(|(i, &size)| {
                    let size =
                        (size - if i == 0 { start } else { 0. } - if i == last { end } else { 0. })
                            .max(0.);
                    (Fixed(size), Fixed(size))
                })
                .collect();
            explicit[axis] = sizes.len();
            continue;
        }
        let (list, lines, collapse) = tracks(
            template[axis].unwrap_or("none"),
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
    let automatic = [0, 1].map(|axis| tracks(implicit[axis], units, available[axis], 0.).0);
    // The minimum contribution of an item without a preferred size: the outer size of its min size, or of 0 for a
    // scroll container. Other items contribute their min-content size.
    let least = |style: &style::Style, units: &Lengths, axis: usize| -> Option<f32> {
        // A percentage depends on the track, so it counts as `auto` here.
        if style
            .get([Prop::WIDTH, Prop::HEIGHT][axis])
            .is_some_and(|v| v != "auto" && !v.contains('%'))
        {
            return None;
        }
        let base = available[0].unwrap_or(0.);
        let size = match style.get([Prop::MIN_WIDTH, Prop::MIN_HEIGHT][axis]) {
            Some(value) if value.contains('%') => 0.,
            Some(value) if let Some(size) = units.resolve(value, 0.) => size,
            // Like Chrome, content keywords act as `auto` in columns and as 0 in rows.
            Some(value) if axis == 1 && value.contains("content") => 0.,
            _ if style
                .get([Prop::OVERFLOW_X, Prop::OVERFLOW_Y][axis])
                .is_some_and(|v| !matches!(v, "visible" | "clip")) =>
            {
                0.
            }
            _ => return None,
        };
        let frame = frame(style, units, base);
        let margin = units.edges(style, style::MARGIN, base);
        let [start, end] = if axis == 0 { [3, 1] } else { [0, 2] };
        let (sides, margins) = (frame[start] + frame[end], margin[start] + margin[end]);
        Some(if style.get(Prop::BOX_SIZING) == Some("border-box") {
            (size + margins).max(sides)
        } else {
            size + sides
        })
    };
    let alignment = |own: &style::Style, axis: usize, replaced: bool| {
        let margins = [
            [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT],
            [Prop::MARGIN_TOP, Prop::MARGIN_BOTTOM],
        ][axis]
            .map(|name| own.get(name).is_some_and(|v| v == "auto"));
        let mut value = "normal";
        for (style, name) in [
            (own, [Prop::JUSTIFY_SELF, Prop::ALIGN_SELF][axis]),
            (style, [Prop::JUSTIFY_ITEMS, Prop::ALIGN_ITEMS][axis]),
        ] {
            if let Some(found) = style.get(name).filter(|v| *v != "auto") {
                value = found;
                break;
            }
        }
        let last = value.starts_with("last");
        match (margins, value.split_whitespace().last().unwrap_or(value)) {
            ([true, true], _) => (Some(0.5), None),
            ([true, false], _) => (Some(1.), None),
            ([false, true], _) => (Some(0.), None),
            (_, "normal") if replaced => (Some(0.), None),
            // Like Chrome, a box with a ratio and an auto height keeps the height of its ratio.
            (_, "normal")
                if axis == 1
                    && own.get(Prop::ASPECT_RATIO).is_some_and(|v| v != "auto")
                    && own.get(Prop::HEIGHT).is_none_or(|v| v == "auto") =>
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
    #[derive(Clone, Copy)]
    struct Item<'a> {
        flowed: bool,
        order: i32,
        placed: ([Option<usize>; 2], [usize; 2]),
        alignment: [Option<f32>; 2],
        baseline: Option<bool>,
        auto_width: bool,
        height: Option<&'a str>,
        subgrid: [bool; 2],
    }
    let mut memo: rustc_hash::FxHashMap<(*const style::Style, bool), Item> = Default::default();
    let table = style::table_box(style::role(styles, id));
    let legend = (doc.nodes[id].tag == "fieldset")
        .then(|| super::element::legend(doc, styles, id))
        .flatten()
        .map(|(legend, ..)| legend);
    let pseudos = styles.pseudos.get(&id).filter(|_| !table);
    let item_style = |child: usize| match pseudo_item(child) {
        Some((_, index)) => &pseudos.expect("a pseudo-element item")[index],
        None => &*styles.values[child],
    };
    let mut read = |child: usize| -> Item {
        let style = item_style(child);
        let replaced = pseudo_item(child).is_none()
            && (doc.nodes[child].tag == "svg"
                || doc.nodes[child].tag != "input"
                    && super::element::natural_size(
                        doc,
                        child,
                        measured.naturals.get(&child).copied(),
                        &styles.values[child],
                    )
                    .is_some());
        let mut item = *memo
            .entry((std::ptr::from_ref(style), replaced))
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
                    let (mut position, mut span) = ([None; 2], [1; 2]);
                    for axis in 0..2 {
                        let [start, end] = [
                            [Prop::GRID_COLUMN_START, Prop::GRID_COLUMN_END],
                            [Prop::GRID_ROW_START, Prop::GRID_ROW_END],
                        ][axis];
                        (position[axis], span[axis]) = placement(
                            style.get(start).unwrap_or("auto"),
                            style.get(end).unwrap_or("auto"),
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
                let subgrid = [0, 1].map(|axis| {
                    pseudo_item(child).is_none()
                        && style
                            .get([Prop::GRID_TEMPLATE_COLUMNS, Prop::GRID_TEMPLATE_ROWS][axis])
                            .is_some_and(|v| v.starts_with("subgrid"))
                });
                let [(column, _), (row, baseline)] =
                    [0, 1].map(|axis| alignment(style, axis, replaced));
                Item {
                    flowed,
                    order,
                    placed,
                    alignment: [column, row],
                    baseline,
                    auto_width: style.get(Prop::WIDTH).is_none_or(|v| v == "auto"),
                    height: style.get(Prop::HEIGHT),
                    subgrid,
                }
            });
        item.flowed &= legend != Some(child)
            && !(table
                && (matches!(doc.nodes[child].tag.as_str(), "col" | "colgroup")
                    || style
                        .get(Prop::DISPLAY)
                        .is_some_and(|v| v.starts_with("table-column"))));
        item
    };
    // Pseudo-elements are the first and last items, keyed by `pseudo_item`.
    let shown = |index: usize| {
        pseudos.is_some_and(|pseudos| {
            pseudos[index]
                .get(Prop::CONTENT)
                .is_some_and(|value| !matches!(value, "none" | "normal"))
        })
    };
    let mut children: Vec<(usize, Item)> = (shown(0).then_some(!(id * 2)).into_iter())
        .chain(doc.nodes[id].children.iter().copied().filter(|&child| {
            doc.nodes[child].tag != "#text" || measured.text_item(doc, child, false)
        }))
        .chain(shown(1).then_some(!(id * 2 + 1)))
        .map(|child| (child, read(child)))
        .filter(|(_, item)| item.flowed)
        .collect();
    if table {
        // Captions and header rows go first and footer rows last, whatever their place in the DOM.
        children.sort_by_key(|&(child, item)| {
            let section =
                std::iter::successors(doc.nodes[child].parent, |&parent| doc.nodes[parent].parent)
                    .take(2)
                    .find_map(|parent| match style::role(styles, parent) {
                        style::HEADER => Some(-1),
                        style::FOOTER => Some(1),
                        _ => None,
                    });
            let rank = match style::role(styles, child) {
                style::CAPTION
                    if styles.values[child].get(Prop::CAPTION_SIDE) == Some("bottom") =>
                {
                    2
                }
                style::CAPTION => -2,
                _ => section.unwrap_or(0),
            };
            (rank, item.order)
        });
        // Each row, caption and run of cells outside rows is a lane of the grid.
        let mut lane = None;
        for (child, item) in &mut children {
            let key = if style::role(styles, *child) == style::CAPTION {
                Some(*child)
            } else {
                doc.nodes[*child]
                    .parent
                    .filter(|&row| (style::ROW..=style::FOOTER).contains(&style::role(styles, row)))
            };
            let index = match lane {
                Some((index, last)) if last == key => index,
                Some((index, _)) => index + 1,
                None => 0,
            };
            lane = Some((index, key));
            item.placed.0[1] = item.placed.0[1].or(Some(index));
        }
    } else {
        children.sort_by_key(|(_, item)| item.order);
    }
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
    // Bound dense-placement cell scans, then fall back to sparse placement.
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
        .zip(&children)
        .map(|((at, (_, span)), &child)| {
            let mut cell = [at[0], at[1], span[0], span[1]];
            if table && style::role(styles, child) == style::CAPTION {
                cell[2] = count[0];
            }
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
    // Widths of `<col>`, `<colgroup>` and cells size the columns, only of the first row with `table-layout: fixed`.
    let mut columns = Vec::new();
    let mut percents: Vec<f32> = Vec::new();
    if table {
        let count = definitions[0].len();
        let base = available[0].map_or(0., |width| width - count.saturating_sub(1) as f32 * gap[0]);
        let width = |node: usize| {
            styles.values[node]
                .get(Prop::WIDTH)
                .filter(|v| available[0].is_some() || !v.contains('%'))
                .and_then(|v| styles.lengths[node].resolve(v, base))
        };
        columns.resize(count, None);
        let mut at = 0;
        for &group in doc.nodes[id].children.iter() {
            if doc.nodes[group].tag != "colgroup" {
                continue;
            }
            let cols = doc.nodes[group].children.iter().copied();
            let cols: Vec<usize> = cols.filter(|&col| doc.nodes[col].tag == "col").collect();
            for col in if cols.is_empty() { vec![group] } else { cols } {
                let span = doc.nodes[col]
                    .attrs
                    .get("span")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 1000);
                let size = width(col).or_else(|| width(group).filter(|_| col == group));
                for slot in columns.iter_mut().skip(at).take(span) {
                    *slot = size;
                }
                at += span;
            }
        }
        let fixed = style.get(Prop::TABLE_LAYOUT) == Some("fixed")
            && available[0].is_some()
            && style
                .get(Prop::WIDTH)
                .is_some_and(|v| !matches!(v, "auto" | "fit-content"));
        percents.resize(count, 0.);
        for (&child, cell) in children.iter().zip(&cells) {
            let Some(value) = styles.values[child]
                .get(Prop::WIDTH)
                .filter(|v| *v != "auto")
            else {
                continue;
            };
            if cell[2] != 1 || fixed && cell[1] != 0 || style::role(styles, child) == style::CAPTION
            {
                continue;
            }
            let size = match value.strip_suffix('%').and_then(|v| v.parse::<f32>().ok()) {
                Some(percent) => {
                    percents[cell[0]] = percents[cell[0]].max(percent / 100.);
                    match available[0] {
                        Some(_) => percent / 100. * base,
                        None => continue,
                    }
                }
                None => intrinsic(doc, child, styles, None, None, text, measured)[1],
            };
            if let Some(slot) = columns.get_mut(cell[0]) {
                *slot = match *slot {
                    Some(old) if fixed => Some(old),
                    Some(old) => Some(old.max(size)),
                    None => Some(size),
                };
            }
        }
        if fixed {
            for (track, size) in definitions[0].iter_mut().zip(&columns) {
                *track = match *size {
                    Some(size) => (Fixed(size), Fixed(size)),
                    None => (Fixed(0.), Flex(1.)),
                };
            }
        } else if columns.iter().any(Option::is_none)
            || percents.iter().any(|&percent| percent > 0.)
        {
            for (track, size) in definitions[0].iter_mut().zip(&columns) {
                if let Some(size) = *size {
                    *track = (Auto, Fixed(size));
                }
            }
        }
    }
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
                let caption = table && style::role(styles, child) == style::CAPTION;
                // A caption sits outside the border spacing of the table.
                let [x, area] = if caption {
                    [x - gap[0], area + 2. * gap[0]]
                } else {
                    [x, area]
                };
                let size = if table {
                    area
                // Like Chrome, a table item of `auto` width stretches.
                } else if fraction.is_none()
                    && style::table_box(style::role(styles, child))
                    && styles.values[child].get(Prop::WIDTH) == Some("fit-content")
                {
                    area.max(min)
                } else if fraction.is_some() && derived[index].auto_width {
                    max.min(area).max(min)
                } else {
                    used
                };
                let mut height = if size == used && !caption {
                    height
                } else {
                    intrinsic(doc, child, styles, Some(size), None, text, measured)[3]
                };
                // A subgrid of columns is as tall as its rows on the parent's columns.
                if derived[index].subgrid == [true, false]
                    && derived[index].height.is_none_or(|v| v == "auto")
                {
                    let frame = frame(&styles.values[child], &styles.lengths[child], area);
                    let inner = layout(
                        doc,
                        child,
                        styles,
                        Some(size - frame[1] - frame[3]),
                        None,
                        Some(&Grid {
                            columns: grid.columns[cell[0]..cell[0] + cell[2]].to_vec(),
                            gap: grid.gap,
                            ..Grid::default()
                        }),
                        None,
                        text,
                        measured,
                    );
                    height = inner.extent(1) + frame[0] + frame[2];
                }
                boxes[index] = [x + (area - size) * fraction.unwrap_or(0.), 0., size, height];
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
        let sized = |cell: &[usize; 4]| {
            available[axis].is_some()
                && definitions[axis]
                    .get(cell[axis]..cell[axis] + cell[axis + 2])
                    .is_some_and(|tracks| {
                        tracks.iter().all(|&(min, max)| {
                            !matches!(min, MinContent | MaxContent | Auto)
                                && !matches!(max, MinContent | MaxContent | Auto | FitContent(_))
                        })
                    })
        };
        let mut contributions = Vec::new();
        for (index, (&child, cell)) in children.iter().zip(&cells).enumerate() {
            if shared[axis].is_some() {
                break;
            }
            if sized(cell) {
                continue;
            }
            if !derived[index].subgrid[axis] {
                let [min, max] = if axis == 0 {
                    let value = intrinsic(doc, child, styles, None, None, text, measured);
                    // A caption widens the table to its min-content width only.
                    let caption = table && style::role(styles, child) == style::CAPTION;
                    [value[0], if caption { value[0] } else { value[1] }]
                } else {
                    // A caption takes the border spacing beside it from its row.
                    let spacing = if table && style::role(styles, child) == style::CAPTION {
                        gap[1]
                    } else {
                        0.
                    };
                    [boxes[index][3] + shims.get(index).copied().unwrap_or(0.) - spacing; 2]
                };
                contributions.push((
                    cell[axis],
                    cell[axis + 2],
                    [
                        least(
                            item_style(child),
                            &styles.lengths[pseudo_item(child).map_or(child, |(parent, _)| parent)],
                            axis,
                        )
                        .map_or(min, |least| least.min(min)),
                        min,
                        max,
                    ],
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
                let span = [
                    [Prop::GRID_COLUMN_START, Prop::GRID_COLUMN_END],
                    [Prop::GRID_ROW_START, Prop::GRID_ROW_END],
                ][axis]
                    .iter()
                    .find_map(|&prop| {
                        style
                            .get(prop)?
                            .trim()
                            .strip_prefix("span")?
                            .trim()
                            .parse::<usize>()
                            .ok()
                    })
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
                let least = least(style, &styles.lengths[grandchild], axis)
                    .map_or(min, |least| least.min(min));
                contributions.push((start, span, [least, min, max]));
                offset += span;
            }
        }
        if axis == 0 {
            // Percent columns grow to their share of the table, beyond the content of their cells only when there
            // is room.
            contributions.extend(
                (columns.iter().enumerate())
                    .filter(|&(i, _)| percents.get(i).is_none_or(|&percent| percent == 0.))
                    .filter_map(|(i, size)| size.map(|size| (i, 1, [0., 0., size]))),
            );
        } else if table {
            // The height of a row is the least height of its cells.
            for (&child, cell) in children.iter().zip(&cells) {
                if let Some(row) = doc.nodes[child]
                    .parent
                    .filter(|&row| style::role(styles, row) == style::ROW)
                    && let Some(height) = styles.values[row]
                        .get(Prop::HEIGHT)
                        .filter(|v| !v.contains('%'))
                        .and_then(|v| styles.lengths[row].resolve(v, 0.))
                {
                    contributions.push((cell[1], 1, [height; 3]));
                }
            }
        }
        let content = get([Prop::JUSTIFY_CONTENT, Prop::ALIGN_CONTENT][axis]).unwrap_or("normal");
        let stretch = matches!(content, "normal" | "stretch");
        let mut space = available[axis];
        let mut sizes = size(
            &definitions[axis],
            &contributions,
            gap[axis],
            space,
            stretch,
            axis == 0 && table,
            axis == 1,
        );
        // Like Chrome, the rows size again in a definite height: the min-height or max-height of the grid when they
        // leave it, or the height that flexible rows below `1fr` first gave the grid, so `0.5fr` takes half of it.
        if axis == 1 && space.is_none() && !table {
            let sides = if get(Prop::BOX_SIZING) == Some("border-box") {
                let base = available[0].unwrap_or(0.);
                let [edges, margin] = [
                    frame(style, units, base),
                    units.edges(style, style::MARGIN, base),
                ];
                edges[0] + edges[2] - margin[0] - margin[2]
            } else {
                0.
            };
            let [least, most] = [Prop::MIN_HEIGHT, Prop::MAX_HEIGHT].map(|prop| {
                get(prop)
                    .filter(|v| !v.contains('%'))
                    .and_then(|v| units.resolve(v, 0.))
                    .map(|v| v - sides)
            });
            let factors: f32 = (definitions[1].iter())
                .filter_map(|track| match track.1 {
                    Flex(factor) => Some(factor),
                    _ => None,
                })
                .sum();
            let used = sizes.iter().sum::<f32>() + sizes.len().saturating_sub(1) as f32 * gap[1];
            let height = match (least, most) {
                (Some(least), _) if used < least => Some(least),
                (_, Some(most)) if factors > 0. && used > most => {
                    Some(most.max(least.unwrap_or(0.)))
                }
                _ if factors > 0. && factors < 1. => Some(used),
                _ => None,
            };
            if let Some(height) = height {
                grid.height = Some(height);
                space = Some(height);
                sizes = size(
                    &definitions[1],
                    &contributions,
                    gap[1],
                    space,
                    stretch,
                    false,
                    true,
                );
            }
        }
        if let Some(space) = space
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
        // Like Chrome, the widest content of a table makes room for the share of its percent columns, and
        // columns of 100% with others beside them make it as wide as it can be.
        if axis == 0 && available[0].is_none() && percents.iter().any(|&percent| percent > 0.) {
            let mut sizes = sizes;
            let rest: f32 = sizes
                .iter()
                .zip(&percents)
                .filter(|(_, p)| **p == 0.)
                .map(|(size, _)| size)
                .sum();
            let total: f32 = percents.iter().sum();
            let width = sizes
                .iter()
                .zip(&percents)
                .filter(|(_, p)| **p > 0.)
                .map(|(size, p)| size / p)
                .fold(rest, f32::max);
            let width = match total < 1. {
                true => width.max(rest / (1. - total)),
                false if rest > 0. => 1e6,
                false => width,
            };
            for (size, percent) in sizes.iter_mut().zip(&percents) {
                *size = size.max(percent * width);
            }
            grid.columns = sizes;
        } else if axis == 0 {
            grid.columns = sizes;
        } else {
            grid.rows = sizes;
        }
    }
    let row_starts = starts(&grid.rows);
    let extent = block.unwrap_or_else(|| {
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
        let caption = table && style::role(styles, child) == style::CAPTION;
        if if table {
            !caption
        } else {
            fraction.is_none() && value.is_none_or(|v| v == "auto")
        } {
            // A min-height of a content keyword keeps the content height in a smaller row.
            let least = item_style(child)
                .get(Prop::MIN_HEIGHT)
                .is_some_and(|v| v.contains("content"));
            height = if least { area.max(height) } else { area };
        }
        let y = if caption && styles.values[child].get(Prop::CAPTION_SIDE) != Some("bottom") {
            y - gap[1]
        } else {
            y
        };
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
    if outermost {
        measured.edges = None;
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::{Sizing::*, *};

    #[test]
    fn items_of_one_track_size_tracks_of_every_kind() {
        let tracks = [
            (Auto, Auto),
            (MinContent, MaxContent),
            (Fixed(10.), Auto),
            (Auto, FitContent(30.)),
            (MaxContent, MaxContent),
            (Auto, Fixed(20.)),
            (MinContent, Auto),
        ];
        let items = [
            (0, 1, [5., 5., 50.]),
            (0, 1, [12., 12., 20.]),
            (1, 1, [8., 8., 40.]),
            (2, 1, [3., 3., 25.]),
            (3, 1, [6., 6., 45.]),
            (4, 1, [7., 9., 33.]),
            (5, 1, [15., 15., 60.]),
            (6, 1, [4., 4., 70.]),
            (2, 2, [30., 30., 90.]),
        ];
        let sized = |available, stretch, table| -> Vec<f32> {
            size(&tracks, &items, 2., available, stretch, table, false)
                .into_iter()
                .map(|v| (v * 1000.).round() / 1000.)
                .collect()
        };
        assert_eq!(
            sized(None, true, false),
            [50., 40., 25., 78., 33., 60., 70.]
        );
        assert_eq!(
            sized(Some(150.), false, false),
            [18.6, 14.6, 16.6, 24.6, 33., 20., 10.6]
        );
        assert_eq!(
            sized(Some(150.), true, true),
            [18.398, 13.388, 18.082, 20.02, 33., 20., 15.112]
        );
        assert_eq!(
            sized(Some(400.), false, false),
            [50., 40., 58., 30., 33., 20., 70.]
        );
        assert_eq!(
            sized(Some(400.), true, false),
            [79., 40., 87., 30., 33., 20., 99.]
        );
    }

    fn sized(
        template: &str,
        items: &[(usize, usize, [f32; 3])],
        gap: f32,
        available: Option<f32>,
    ) -> Vec<f32> {
        let (list, ..) = tracks(template, &Lengths::new(16., 800., 600.), available, gap);
        size(&list, items, gap, available, true, false, false)
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
        let styles = crate::dom::styles(&mut doc, &mut state, [800., 600.]);
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
        assert_eq!(grid.rows, [26., 26.]);
        let cells: Vec<_> = grid.cells.iter().map(|(_, cell, ..)| *cell).collect();
        assert_eq!(cells[2..], [[1, 1, 1, 1], [2, 1, 1, 1]]);
    }

    #[test]
    fn items_of_one_style_resolve_percentages_against_each_width() {
        let text = renderer::TextSystem::new();
        let mut doc = Document::parse(
            "<style>main{display:grid;grid-template-columns:100px 300px}i{display:block;padding:10% 0 0;height:0}</style><main><i></i><i></i></main>",
        );
        let mut state = crate::dom::State::default();
        let styles = crate::dom::styles(&mut doc, &mut state, [800., 600.]);
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
        let heights: Vec<_> = grid.cells.iter().map(|(.., bounds, _)| bounds[3]).collect();
        assert_eq!(heights, [10., 30.]);
        assert_eq!(grid.rows, [30.]);
    }

    #[test]
    fn aspect_ratio_children_give_rows_their_height() {
        let text = renderer::TextSystem::new();
        let mut doc = Document::parse(
            "<style>main{display:grid;grid-template-columns:300px 200px}i{display:block;aspect-ratio:5/4}.b{box-sizing:border-box;padding:10px}.c{overflow:hidden}</style><main><div><i style='width:100%'></i></div><div><i class=b></i><p style='height:20px;margin:0'></p></div></main><main><div><i class=c style='aspect-ratio:10'><p style='height:90px;margin:0'></p></i></div><div><i style='aspect-ratio:10'><p style='height:90px;margin:0'></p></i></div></main>",
        );
        let mut state = crate::dom::State::default();
        let styles = crate::dom::styles(&mut doc, &mut state, [800., 600.]);
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
        assert_eq!(rows, [vec![240.], vec![90.]]);
    }

    #[test]
    fn dense_packing_fills_holes_and_stays_bounded() {
        let text = renderer::TextSystem::new();
        let place = |html: &str| {
            let mut doc = Document::parse(html);
            let mut state = crate::dom::State::default();
            let styles = crate::dom::styles(&mut doc, &mut state, [800., 600.]);
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
