use crate::css;
use crate::css::value::units::Lengths;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Sizing {
    Fixed(f32),
    MinContent,
    MaxContent,
    Auto,
    Flex(f32),
    FitContent(f32),
}

pub(super) type Track = (Sizing, Sizing);

pub(super) type Lines = Vec<(String, usize)>;

pub(super) fn tracks(
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
    for part in css::value::tokens::components(value, ' ') {
        let (function, arguments) = part
            .strip_suffix(')')
            .and_then(|v| v.split_once('('))
            .map_or((part, Vec::new()), |(name, body)| {
                (name, css::value::tokens::components(body, ','))
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

pub(crate) fn placement(
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

pub(super) fn size(
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
}
