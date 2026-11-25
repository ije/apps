use crate::{Pixels, Point, point, px};

/// Where a scroll container snaps, in scroll positions: how far the content moved to the start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollSnaps {
    /// Whether each axis snaps, and whether it must.
    pub strict: [Option<bool>; 2],
    /// The size of the snapport.
    pub port: [f32; 2],
    pub areas: Vec<SnapArea>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapArea {
    pub id: u64,
    /// The first and the last position that the area snaps to on each axis. They differ for an area that is larger
    /// than the snapport, which can cover it at any position between.
    pub at: [Option<[f32; 2]>; 2],
    /// The positions at which the snapport shows the area on each axis.
    pub seen: [[f32; 2]; 2],
    /// No scroll goes past the area.
    pub stop: bool,
}

impl ScrollSnaps {
    /// The offset to snap to for a scroll that ends at `to`, and the areas there. A scroll from `from` goes on in its
    /// direction, as arrow keys and flings do.
    pub(crate) fn find(
        &self,
        to: Point<Pixels>,
        from: Option<Point<Pixels>>,
        range: [Point<Pixels>; 2],
    ) -> (Point<Pixels>, [Option<u64>; 2]) {
        let [max, min] = range.map(|at| [-at.x.0, -at.y.0]);
        let from = from.map(|at| [-at.x.0, -at.y.0]);
        let mut at = [-to.x.0, -to.y.0];
        let mut ids = [None; 2];
        for axis in 0..2 {
            let Some(must) = self.strict[axis] else {
                continue;
            };
            let aim = at[axis].clamp(min[axis], max[axis]);
            let start = from
                .map(|from| from[axis])
                .filter(|start| (start - aim).abs() > 0.5);
            let other = at[1 - axis];
            let mut best: Option<(f32, f32, u64)> = None;
            let mut back: Option<(f32, f32, u64)> = None;
            let mut stop: Option<(f32, u64)> = None;
            for area in &self.areas {
                let Some([first, last]) = area.at[axis] else {
                    continue;
                };
                let [low_seen, high_seen] = area.seen[1 - axis];
                if other <= low_seen || other >= high_seen {
                    continue;
                }
                let place = aim.clamp(first, last).clamp(min[axis], max[axis]);
                let far = (place - aim).abs();
                let ahead = start.is_none_or(|start| (place - start) * (aim - start) > 0.);
                let best = if ahead { &mut best } else { &mut back };
                if best.is_none_or(|(least, old, _)| far < least || far == least && place < old) {
                    *best = Some((far, place, area.id));
                }
                if let Some(start) = start.filter(|_| area.stop && ahead) {
                    let edge = if aim > start { first } else { last };
                    let edge = edge.clamp(min[axis], max[axis]);
                    if (edge - start) * (aim - start) > 0.
                        && (edge - start).abs() < (aim - start).abs()
                        && stop.is_none_or(|(old, _)| (edge - start).abs() < (old - start).abs())
                    {
                        stop = Some((edge, area.id));
                    }
                }
            }
            let found = stop.or_else(|| {
                best.or(back.filter(|_| must))
                    // Like Chrome, `proximity` reaches a third of the snapport.
                    .filter(|(far, ..)| must || *far <= self.port[axis] / 3.)
                    .map(|(_, place, id)| (place, id))
            });
            if let Some((place, id)) = found {
                (at[axis], ids[axis]) = (place, Some(id));
            }
        }
        (point(px(-at[0]), px(-at[1])), ids)
    }

    // Like Chrome, a layout keeps the areas that the box snapped to, or else snaps to the nearest ones.
    pub(crate) fn follow(
        &self,
        offset: Point<Pixels>,
        ids: [Option<u64>; 2],
        range: [Point<Pixels>; 2],
    ) -> (Point<Pixels>, [Option<u64>; 2]) {
        let mut to = offset;
        let [low, high] = range;
        for axis in 0..2 {
            let kept = self
                .areas
                .iter()
                .find(|area| Some(area.id) == ids[axis])
                .and_then(|area| area.at[axis])
                .filter(|_| self.strict[axis].is_some());
            if let Some([first, last]) = kept {
                let (value, low, high) = match axis {
                    0 => (&mut to.x, low.x, high.x),
                    _ => (&mut to.y, low.y, high.y),
                };
                *value = px(-(-value.0).clamp(first, last)).clamp(low, high);
            }
        }
        let (found, near) = self.find(to, None, range);
        let mut result = (found, near);
        for axis in 0..2 {
            let kept = self
                .areas
                .iter()
                .any(|area| Some(area.id) == ids[axis] && area.at[axis].is_some());
            if kept && self.strict[axis].is_some() {
                match axis {
                    0 => result.0.x = to.x,
                    _ => result.0.y = to.y,
                }
                result.1[axis] = ids[axis];
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{ScrollSnaps, SnapArea};
    use crate::{Pixels, Point, point, px};

    fn rows(must: bool, height: f32, count: u64, stop: bool) -> ScrollSnaps {
        ScrollSnaps {
            strict: [None, Some(must)],
            port: [200., 100.],
            areas: (0..count)
                .map(|id| {
                    let top = id as f32 * height;
                    SnapArea {
                        id: id + 1,
                        at: [None, Some([top, top.max(top + height - 100.)])],
                        seen: [[-200., 200.], [top - 100., top + height]],
                        stop,
                    }
                })
                .collect(),
        }
    }

    fn at(y: f32) -> Point<Pixels> {
        point(px(0.), px(-y))
    }

    #[test]
    fn scrolls_snap_like_chrome() {
        let end = |snaps: &ScrollSnaps, to: f32, from: Option<f32>| {
            -snaps.find(at(to), from.map(at), [at(540.), at(0.)]).0.y.0
        };
        let must = rows(true, 80., 8, false);
        for (to, place) in [(39., 0.), (40., 0.), (41., 80.), (130., 160.), (540., 540.)] {
            assert_eq!(end(&must, to, None), place, "{to}");
        }
        assert_eq!(end(&must, 90., Some(80.)), 160.);
        assert_eq!(end(&must, 70., Some(80.)), 0.);
        assert_eq!(end(&must, 180., Some(80.)), 160.);
        assert_eq!(end(&rows(true, 80., 8, true), 300., Some(80.)), 160.);
        let large = rows(true, 300., 8, false);
        for (to, place) in [(120., 120.), (220., 200.), (260., 300.)] {
            assert_eq!(end(&large, to, None), place, "{to}");
        }
        let near = rows(false, 150., 8, false);
        for (to, place) in [(30., 30.), (55., 50.), (95., 95.)] {
            assert_eq!(end(&near, to, None), place, "{to}");
        }
    }

    #[test]
    fn layouts_keep_the_snapped_area() {
        let old = rows(true, 80., 8, false);
        let range = [at(700.), at(0.)];
        let (offset, ids) = old.find(at(160.), None, range);
        assert_eq!(ids, [None, Some(3)]);
        assert_eq!(
            rows(true, 100., 8, false).follow(offset, ids, range),
            (at(200.), ids)
        );
    }
}
