use crate::css::value::transform::Matrix;
use crate::css::value::transform::transform;
use crate::css::value::transform::translate;
use crate::css::{properties::Style, value::units::Lengths};
use renderer::{Pixels, Rect, point, px};

// A transform at the origin and the size it is for.
pub(crate) type Local = std::cell::Cell<Option<Box<([f32; 2], Option<Matrix>)>>>;

// a move there and back, so a plain translation stays exact.
pub(crate) fn placed(m: &Matrix, [x, y]: [f32; 2]) -> Matrix {
    let mut n = *m;
    for column in 0..3 {
        n[column * 4] += x * m[column * 4 + 3];
        n[column * 4 + 1] += y * m[column * 4 + 3];
    }
    let w = x * m[3] + y * m[7];
    n[12] = m[12] + x * (m[15] - m[0]) - y * m[4] - x * w;
    n[13] = m[13] + y * (m[15] - m[5]) - x * m[1] - y * w;
    n[14] = m[14] - x * m[2] - y * m[6];
    n[15] = m[15] - w;
    n
}

// The transform of a box at its place, from a cache of the transform at the origin for its size.
pub(crate) fn transform_at(
    cache: &Local,
    style: &Style,
    units: &Lengths,
    bounds: Rect<Pixels>,
) -> Option<Matrix> {
    let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
    let mut kept = cache.take().filter(|kept| kept.0 == size);
    let local = kept
        .get_or_insert_with(|| {
            let origin = Rect::new(renderer::Point::default(), bounds.size);
            Box::new((size, transform(style, units, origin, None)))
        })
        .1;
    cache.set(kept);
    local.map(|local| placed(&local, [bounds.origin.x, bounds.origin.y].map(f32::from)))
}

// The near and far corners of the rectangle between `corners` after the transform of a box of `size`.
pub(crate) fn reach(
    cache: &Local,
    style: &Style,
    units: &Lengths,
    size: renderer::Size<Pixels>,
    corners: [renderer::Point<Pixels>; 2],
) -> [renderer::Point<Pixels>; 2] {
    let Some(matrix) = transform_at(cache, style, units, Rect::new(Default::default(), size))
    else {
        return corners;
    };
    let [[left, top], [right, bottom]] = corners.map(|at| [at.x, at.y].map(f32::from));
    let mut mapped = [[left, top], [right, top], [left, bottom], [right, bottom]]
        .into_iter()
        .filter_map(|corner| map(&matrix, corner))
        .map(|[x, y]| point(px(x), px(y)));
    let Some(first) = mapped.next() else {
        return corners;
    };
    mapped.fold([first; 2], |[near, far], at| [near.min(at), far.max(at)])
}

pub(crate) fn translation(matrix: &Matrix) -> Option<[f32; 2]> {
    let plain = translate(matrix[12], matrix[13], 0.);
    (*matrix == plain).then_some([matrix[12], matrix[13]])
}

/// The scale for the rasters in the layer of a composited transform animation: the largest that the animation
/// gives from `now` on, over one loop or 4 seconds, since frames change the transform without a paint.
pub(crate) fn largest_scale(
    current: Option<&Matrix>,
    motion: &crate::style::Motion,
    styles: &crate::style::ComputedStyles,
    id: usize,
    units: &Lengths,
    (bounds, perspective): crate::render::CompositedTransform,
    now: std::time::Instant,
) -> Option<f32> {
    let span = (motion.period(id, 0, styles, now)).map_or(4., |(period, ..)| period);
    let times: Vec<_> = (0..=64)
        .map(|k| now + std::time::Duration::from_secs_f64(span * f64::from(k) / 64.))
        .collect();
    let timeline = motion.timeline(id, 0, styles, &times, &super::TRANSFORM_PROPS);
    let scales = timeline.iter().map(|style| {
        transform(style, units, bounds, perspective).map_or(1., |m| renderer::scale_of(&m))
    });
    (scales.chain(current.map(renderer::scale_of)))
        .filter(|scale| scale.is_finite())
        .reduce(f32::max)
}

fn map(matrix: &Matrix, [x, y]: [f32; 2]) -> Option<[f32; 2]> {
    let w = matrix[3] * x + matrix[7] * y + matrix[15];
    (w > 1e-4).then(|| {
        [
            (matrix[0] * x + matrix[4] * y + matrix[12]) / w,
            (matrix[1] * x + matrix[5] * y + matrix[13]) / w,
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::{close, style};
    use super::*;
    use crate::css::value::transform::multiply;
    use renderer::size;

    #[test]
    fn transforms_move_with_their_box() {
        let units = Lengths::new(16., 800., 600.);
        let at = |x: f32, y: f32| Rect::new(renderer::point(px(x), px(y)), size(px(120.), px(40.)));
        for pairs in [
            &[("transform", "rotate(30deg) translate(10%, 2em) scale(1.5)")][..],
            &[
                ("translate", "-40px 25%"),
                ("rotate", "12deg"),
                ("transform-origin", "0 100%"),
            ],
            &[
                (
                    "transform",
                    "perspective(200px) rotateY(40deg) skewX(10deg)",
                ),
                ("transform-origin", "20px 5px"),
            ],
        ] {
            let style = style(pairs);
            let local = transform(&style, &units, at(0., 0.), None).unwrap();
            let moved = multiply(
                &translate(300., 70., 0.),
                &multiply(&local, &translate(-300., -70., 0.)),
            );
            let placed = transform(&style, &units, at(300., 70.), None).unwrap();
            for corner in [[300., 70.], [420., 110.], [350., 90.]] {
                let (a, b) = (map(&moved, corner).unwrap(), map(&placed, corner).unwrap());
                assert!(close(a, b), "{pairs:?} {a:?} {b:?}");
            }
        }
    }

    // The SVG draws the two parts of `exclude` one over the other, so it matches the XOR of the standard only
    // where alpha is 0 or 1.

    #[test]
    fn transforms_map_window_points_around_the_origin() {
        let units = Lengths::new(16., 800., 600.);
        let bounds = Rect::new(point(px(100.), px(50.)), size(px(10.), px(20.)));
        let moved = transform(
            &style(&[("transform", "translate(5px, 50%)")]),
            &units,
            bounds,
            None,
        )
        .unwrap();
        assert_eq!(translation(&moved), Some([5., 10.]));
        assert_eq!(
            transform(
                &style(&[("transform", "rotate(0deg)")]),
                &units,
                bounds,
                None
            ),
            None
        );
        let quarter = transform(&style(&[("rotate", "90deg")]), &units, bounds, None).unwrap();
        assert_eq!(translation(&quarter), None);
        assert!(close(map(&quarter, [105., 60.]).unwrap(), [105., 60.]));
        assert!(close(map(&quarter, [100., 50.]).unwrap(), [115., 55.]));
        let corner = transform(
            &style(&[("transform", "scale(2)"), ("transform-origin", "left top")]),
            &units,
            bounds,
            None,
        )
        .unwrap();
        assert!(close(map(&corner, [110., 70.]).unwrap(), [120., 90.]));
        let flipped = transform(
            &style(&[("transform", "rotateY(180deg)")]),
            &units,
            bounds,
            Some((300., [105., 60.])),
        )
        .unwrap();
        assert!(renderer::faces_away(&flipped));
        assert!(!renderer::faces_away(&quarter));
        assert!(
            transform(
                &style(&[("transform", "rotate3d(0, 0, 0, 45deg)")]),
                &units,
                bounds,
                None
            )
            .is_none()
        );
    }

    #[test]
    fn composited_scale_animations_raster_at_their_largest_scale() {
        // Grows for one second, then holds at its own size.
        let mut doc = crate::dom::Document::parse(
            "<style>@keyframes up{from{transform:scale(.25)}5%,to{transform:scale(1)}}\
             div{width:100px;height:100px;animation:up 20s linear}</style><div></div>",
        );
        let mut dom = crate::render::PageState::default();
        let div = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
        let styles = dom.restyle(&mut doc, [800., 600.]);
        let now = std::time::Instant::now();
        let frame = dom.styles.motion.update(
            &doc,
            &styles,
            &vec![true; doc.nodes.len()],
            &|_| true,
            &|_| f32::INFINITY,
            now,
            false,
            &mut Vec::new(),
        );
        let (_, style, _) = frame.animated.iter().find(|(id, ..)| *id == div).unwrap();
        let units = styles.lengths[div];
        let bounds = Rect::new(point(px(0.), px(0.)), renderer::size(px(100.), px(100.)));
        let first = transform(style, &units, bounds, None).unwrap();
        assert!(
            (renderer::scale_of(&first) - 0.25).abs() < 0.01,
            "{first:?}"
        );
        // The first paint sees a quarter of the size, but frames grow the layer to its full size.
        let largest = largest_scale(
            Some(&first),
            &dom.styles.motion,
            &styles,
            div,
            &units,
            (bounds, None),
            now,
        );
        assert_eq!(largest, Some(1.));
    }
}
