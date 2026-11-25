use crate::{Pixels, Point, point, px};

/// Where a scroll container snaps, in CSS pixels from the corner of its border box. A scroll position is how far the
/// content moved to the start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollSnaps {
    /// Whether each axis snaps.
    pub axes: [bool; 2],
    /// The box must snap. Else it snaps to areas that are near.
    pub must: bool,
    /// The snapport: its start and its end on each axis.
    pub port: [[f32; 2]; 2],
    /// In document order.
    pub areas: Vec<SnapArea>,
    /// The area of the URL fragment.
    pub target: Option<u64>,
    /// Device pixels per CSS pixel. Offsets are whole device pixels.
    pub ratio: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapAlign {
    Start,
    Center,
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapArea {
    pub id: u64,
    /// The start and the end on each axis, without the scroll offset.
    pub rect: [[f32; 2]; 2],
    pub align: [Option<SnapAlign>; 2],
    /// No scroll in a direction goes past the area.
    pub stop: bool,
    /// The area has the focus in it.
    pub focus: bool,
}

/// How a scroll looks for its snap areas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Snap {
    /// A scroll to a place on these axes.
    To([bool; 2]),
    /// A scroll from this offset goes on in its direction.
    From(Point<Pixels>),
    /// A layout keeps the areas.
    Keep,
}

type Ids = [Option<u64>; 2];

#[derive(Clone, Copy)]
struct Search {
    snap: Snap,
    start: [f32; 2],
    // The place that the scroll looks around, and the one where it ends without snapping.
    base: [f32; 2],
    end: [f32; 2],
    // Only areas with `scroll-snap-stop: always`, the nearest first.
    stops: bool,
}

// A snap position on an axis, the positions at which the other axis shows the area, and an area that fits as well if
// the other axis is at its position.
#[derive(Clone, Copy)]
struct Found<'a> {
    at: f32,
    seen: [f32; 2],
    area: Option<&'a SnapArea>,
    other: Option<(&'a SnapArea, f32)>,
}

impl<'a> Found<'a> {
    fn sees(&self, cross: &Found) -> bool {
        let within = |at: f32, [low, high]: [f32; 2]| at >= low && at <= high;
        within(cross.at, self.seen) && within(self.at, cross.seen)
    }

    fn focused(&self) -> bool {
        self.area.is_some_and(|area| area.focus)
    }

    fn id(&self) -> Option<u64> {
        self.area.map(|area| area.id)
    }
}

fn contains(outer: &SnapArea, inner: &SnapArea) -> bool {
    (0..2).all(|axis| {
        outer.rect[axis][0] <= inner.rect[axis][0] && outer.rect[axis][1] >= inner.rect[axis][1]
    })
}

// The rules are those of Chrome 153.
impl ScrollSnaps {
    /// The offset to snap to for a scroll that ends at `to`, and the areas there. `ids` are the areas until now.
    pub(crate) fn find(
        &self,
        to: Point<Pixels>,
        snap: Snap,
        ids: Ids,
        range: [Point<Pixels>; 2],
    ) -> (Point<Pixels>, Ids) {
        let position = |at: Point<Pixels>| [-at.x.0, -at.y.0];
        let range = [position(range[1]), position(range[0])];
        let end = position(to);
        let start = match snap {
            Snap::From(from) => position(from),
            _ => end,
        };
        let search = Search {
            snap,
            start,
            base: end,
            end,
            stops: false,
        };
        let wants = match snap {
            Snap::To(axes) => axes,
            Snap::From(_) => [0, 1].map(|axis| start[axis] != end[axis]),
            Snap::Keep => [true; 2],
        };
        let wants = [0, 1].map(|axis| wants[axis] && self.axes[axis]);
        if wants == [false; 2] {
            return (to, ids);
        }
        let keeps = [0, 1].map(|axis| snap == Snap::Keep && ids[axis].is_some());
        let mut found = [0, 1].map(|axis| {
            let cross = 1 - axis;
            wants[axis].then_some(())?;
            // The axis starts at the place of the other one.
            let place = Found {
                at: end[cross].clamp(range[0][cross], range[1][cross]),
                seen: [range[0][axis], range[1][axis]],
                area: None,
                other: None,
            };
            keeps[axis]
                .then(|| self.kept(axis, &search, ids[axis], &place, range))
                .flatten()
                .or_else(|| self.nearest(axis, &search, &place, range))
        });
        if found.iter().all(Option::is_none) {
            return match wants == [true; 2] && !matches!(snap, Snap::From(_)) {
                true => self.mutual(to, start, range),
                false => (to, [None; 2]),
            };
        }
        // An axis can push the area of the other one out of view. Then the other one looks again.
        if wants == [true; 2] {
            let follow = match &found {
                [Some(x), Some(y)] if x.sees(y) => None,
                [Some(x), Some(y)] => Some(match (snap == Snap::Keep, ids) {
                    (true, [None, _]) => 1,
                    (true, [_, None]) => 0,
                    (true, _) if x.focused() => 0,
                    (true, _) if y.focused() => 1,
                    (true, _) if self.target.is_some() && x.id() == self.target => 0,
                    (true, _) => 1,
                    _ => usize::from((x.at - end[0]).abs() > (y.at - end[1]).abs()),
                }),
                [None, _] => Some(1),
                _ => Some(0),
            };
            if let Some(axis) = follow
                && let Some(cross) = found[axis]
            {
                found[1 - axis] = self.nearest(1 - axis, &search, &cross, range);
            }
        }
        for axis in [1, 0] {
            let cross = 1 - axis;
            let at = found[cross].map_or_else(
                || start[cross].clamp(range[0][cross], range[1][cross]),
                |found| found.at,
            );
            if let Some(found) = &mut found[axis]
                && let Some((area, place)) = found.other
                && (at - place).abs() <= 1.
            {
                found.area = Some(area);
            }
        }
        // An axis without a snap position stays where the scroll started.
        let round = |at: f32| px(-(at * self.ratio).round() / self.ratio);
        let at = [0, 1].map(|axis| found[axis].map_or(px(-start[axis]), |found| round(found.at)));
        let ids = [0, 1].map(|axis| match &found[axis] {
            Some(found) => found.id(),
            None => ids[axis].filter(|&id| self.areas.iter().any(|area| area.id == id)),
        });
        (point(at[0], at[1]), ids)
    }

    // Both axes snap to one area that the scroll to either of its axes does not show.
    fn mutual(
        &self,
        to: Point<Pixels>,
        start: [f32; 2],
        range: [[f32; 2]; 2],
    ) -> (Point<Pixels>, Ids) {
        let mut best: Option<([f32; 2], [f32; 2], u64)> = None;
        for area in &self.areas {
            let [Some(x), Some(y)] = [0, 1].map(|axis| self.aligned(axis, area, range)) else {
                continue;
            };
            let at = [x.at, y.at];
            let by = [0, 1].map(|axis| at[axis] - start[axis]);
            if (0..2).any(|axis| by[axis].abs() > self.reach(axis)) {
                continue;
            }
            let far = [0, 1].map(|axis| (-by[axis]).max(by[axis] - self.size(axis)).max(0.));
            if best.is_none_or(|(least, ..)| {
                far[1] < least[1] || far[1] == least[1] && far[0] < least[0]
            }) {
                best = Some((far, at, area.id));
            }
        }
        let round = |at: f32| px(-(at * self.ratio).round() / self.ratio);
        best.map_or((to, [None; 2]), |(_, at, id)| {
            (point(round(at[0]), round(at[1])), [Some(id); 2])
        })
    }

    fn size(&self, axis: usize) -> f32 {
        self.port[axis][1] - self.port[axis][0]
    }

    // Like Chrome, `proximity` reaches a third of the snapport.
    fn reach(&self, axis: usize) -> f32 {
        match self.must {
            true => f32::MAX,
            false => self.size(axis) / 3.,
        }
    }

    fn covers(&self, axis: usize, area: &SnapArea) -> bool {
        area.rect[axis][1] - area.rect[axis][0] >= self.size(axis)
    }

    fn aligned<'a>(
        &self,
        axis: usize,
        area: &'a SnapArea,
        [low, high]: [[f32; 2]; 2],
    ) -> Option<Found<'a>> {
        let first = area.rect[axis][0] - self.port[axis][0];
        let last = area.rect[axis][1] - self.port[axis][1];
        let at = match area.align[axis]? {
            SnapAlign::Start => first,
            SnapAlign::Center => (first + last) / 2.,
            SnapAlign::End => last,
        };
        let cross = 1 - axis;
        let seen = |at: f32| at.clamp(low[cross], high[cross]);
        Some(Found {
            at: at.clamp(low[axis], high[axis]),
            seen: [
                seen(area.rect[cross][0] - self.port[cross][1]),
                seen(area.rect[cross][1] - self.port[cross][0]),
            ],
            area: Some(area),
            other: None,
        })
    }

    fn kept(
        &self,
        axis: usize,
        search: &Search,
        id: Option<u64>,
        cross: &Found,
        range: [[f32; 2]; 2],
    ) -> Option<Found<'_>> {
        let area = self.areas.iter().find(|area| Some(area.id) == id)?;
        let aligned = self.aligned(axis, area, range)?;
        match self.covers(axis, area) {
            true => self.closest(axis, search, cross, range, Some(area.rect[axis])),
            false => None,
        }
        .or(Some(aligned))
    }

    fn nearest(
        &self,
        axis: usize,
        search: &Search,
        cross: &Found,
        range: [[f32; 2]; 2],
    ) -> Option<Found<'_>> {
        let found = self.closest(axis, search, cross, range, None);
        if !matches!(search.snap, Snap::From(_)) {
            return found;
        }
        if let Some(found) = found {
            let first = Search {
                base: search.start,
                stops: true,
                ..*search
            };
            let stop = self.closest(axis, &first, cross, range, None);
            let far = |found: &Found| (found.at - search.start[axis]).abs();
            return Some(match stop {
                Some(stop) if far(&stop) <= far(&found) => stop,
                _ => found,
            });
        }
        // With nothing ahead, a box that must snap stays near where it is.
        self.must.then_some(())?;
        let stay = Search {
            snap: Snap::To([true; 2]),
            base: search.start,
            end: search.start,
            ..*search
        };
        self.closest(axis, &stay, cross, range, None)
    }

    fn closest<'a>(
        &'a self,
        axis: usize,
        search: &Search,
        cross: &Found,
        range: [[f32; 2]; 2],
        inside: Option<[f32; 2]>,
    ) -> Option<Found<'a>> {
        let (base, start, end) = (search.base[axis], search.start[axis], search.end[axis]);
        let mut least = self.reach(axis);
        let mut best: Option<Found<'a>> = None;
        let mut cover: Option<Found<'a>> = None;
        let mut weigh = |found: Found<'a>, area: &'a SnapArea| {
            let by = (found.at - start).trunc();
            let ahead = !matches!(search.snap, Snap::From(_)) || by * (end - start) > 0.;
            let far = (found.at - base).abs();
            if !found.sees(cross) || !ahead || far > least {
                return;
            }
            let Some(old) = best.as_mut().filter(|_| far == least) else {
                (least, best) = (far, Some(found));
                return;
            };
            if found.focused() && !search.stops {
                *old = found;
            } else if !old.focused()
                && let Some(current) = old.area
                && Some(current.id) != self.target
            {
                let inner = contains(current, area) && current.rect != area.rect;
                if Some(area.id) == self.target || inner {
                    *old = found;
                } else if self.axes == [true; 2]
                    && area.align[1 - axis].is_some()
                    && !contains(area, current)
                    && let Some(place) = self.aligned(1 - axis, area, range)
                {
                    let near = |at: f32| (search.base[1 - axis] - at).abs();
                    if old.other.is_none_or(|(other, at)| {
                        near(place.at) < near(at)
                            || near(place.at) == near(at)
                                && contains(other, area)
                                && other.rect != area.rect
                    }) {
                        old.other = Some((area, place.at));
                    }
                }
            }
        };
        for area in &self.areas {
            let [low, high] = area.rect[axis];
            if search.stops && !area.stop
                || inside.is_some_and(|[start, end]| high.min(end) <= low.max(start))
            {
                continue;
            }
            let Some(found) = self.aligned(axis, area, range) else {
                continue;
            };
            weigh(found, area);
            if !search.stops
                && self.covers(axis, area)
                && let Some(at) = self.covering(axis, area, end)
            {
                let found = Found { at, ..found };
                if at != end {
                    weigh(found, area);
                } else if let Some(cover) = &mut cover {
                    // Areas that cover the snapport together count as one.
                    cover.seen = [
                        cover.seen[0].min(found.seen[0]),
                        cover.seen[1].max(found.seen[1]),
                    ];
                    if area.focus {
                        cover.area = Some(area);
                    }
                } else {
                    cover = Some(found);
                }
            }
        }
        cover.or(best)
    }

    // The position nearest to `end` at which the area covers the snapport, past the areas that overlap it.
    fn covering(&self, axis: usize, area: &SnapArea, end: f32) -> Option<f32> {
        let [low, high] = area.rect[axis];
        let port = [end + self.port[axis][0], end + self.port[axis][1]];
        let (mut back, mut middle, mut ahead) = ([low, high], Some([low, high]), [low, high]);
        for other in &self.areas {
            let [first, last] = other.rect[axis];
            if first > high || last < low || first <= low && last >= high {
                continue;
            }
            match last < port[0] {
                true => back[0] = back[0].max(last),
                false => back[1] = back[1].min(first),
            }
            match first > port[1] {
                true => ahead[1] = ahead[1].min(first),
                false => ahead[0] = ahead[0].max(last),
            }
            if first <= port[0] && last >= port[1] || first >= port[0] && last <= port[1] {
                middle = None;
            } else if let Some([start, end]) = &mut middle {
                match first <= port[0] {
                    true => *start = start.max(last),
                    false => *end = end.min(first),
                }
            }
        }
        let place = |[start, stop]: [f32; 2]| {
            if start >= stop {
                return None;
            }
            let [first, last] = [start - self.port[axis][0], stop - self.port[axis][1]];
            if last > first {
                return Some(end.clamp(first, last));
            }
            // The snapport does not fit between the other areas.
            let at = match area.align[axis]? {
                SnapAlign::Start => first,
                SnapAlign::Center => (first + last) / 2.,
                SnapAlign::End => last,
            };
            let [first, last] = [low - self.port[axis][0], high - self.port[axis][1]];
            (last >= first).then(|| at.clamp(first, last))
        };
        middle
            .and_then(place)
            .or_else(|| match [place(back), place(ahead)] {
                [Some(back), Some(ahead)] if (back - end).abs() < (ahead - end).abs() => Some(back),
                [back, ahead] => ahead.or(back),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{ScrollSnaps, Snap, SnapAlign, SnapArea};
    use crate::{Pixels, Point, point, px};

    fn rows(must: bool, height: f32, count: u64, stop: bool) -> ScrollSnaps {
        ScrollSnaps {
            axes: [false, true],
            must,
            port: [[0., 200.], [0., 100.]],
            areas: (0..count)
                .map(|id| SnapArea {
                    id: id + 1,
                    rect: [[0., 200.], [id as f32 * height, (id + 1) as f32 * height]],
                    align: [None, Some(SnapAlign::Start)],
                    stop,
                    focus: false,
                })
                .collect(),
            target: None,
            ratio: 1.,
        }
    }

    fn at(y: f32) -> Point<Pixels> {
        point(px(0.), px(-y))
    }

    #[test]
    fn scrolls_snap_like_chrome() {
        let end = |snaps: &ScrollSnaps, to: f32, from: Option<f32>| {
            let snap = from.map_or(Snap::To([true; 2]), |from| Snap::From(at(from)));
            -snaps
                .find(at(to), snap, [None; 2], [at(540.), at(0.)])
                .0
                .y
                .0
        };
        let must = rows(true, 80., 8, false);
        for (to, place) in [(39., 0.), (40., 0.), (41., 80.), (130., 160.), (540., 540.)] {
            assert_eq!(end(&must, to, None), place, "{to}");
        }
        assert_eq!(end(&must, 90., Some(80.)), 160.);
        assert_eq!(end(&must, 70., Some(80.)), 0.);
        assert_eq!(end(&must, 180., Some(80.)), 160.);
        assert_eq!(end(&rows(true, 80., 8, true), 300., Some(80.)), 160.);
        let near = rows(false, 150., 8, false);
        for (to, place) in [(30., 30.), (55., 50.), (90., 90.)] {
            assert_eq!(end(&near, to, None), place, "{to}");
        }
    }

    #[test]
    fn areas_at_one_place_prefer_the_focus_and_the_inner_area() {
        let mut snaps = rows(true, 80., 8, false);
        let mut inner = snaps.areas[2].clone();
        (inner.id, inner.rect[1][1]) = (20, 200.);
        snaps.areas.push(inner);
        let range = [at(540.), at(0.)];
        let found = |snaps: &ScrollSnaps| {
            snaps
                .find(at(150.), Snap::To([true; 2]), [None; 2], range)
                .1
        };
        assert_eq!(found(&snaps), [None, Some(20)]);
        snaps.areas[2].focus = true;
        assert_eq!(found(&snaps), [None, Some(3)]);
    }

    #[test]
    fn layouts_keep_the_snapped_area() {
        let old = rows(true, 80., 8, false);
        let range = [at(700.), at(0.)];
        let (offset, ids) = old.find(at(160.), Snap::To([true; 2]), [None; 2], range);
        assert_eq!(ids, [None, Some(3)]);
        assert_eq!(
            rows(true, 100., 8, false).find(offset, Snap::Keep, ids, range),
            (at(200.), ids)
        );
    }
}
