use super::super::svg::svg_tree;
use tiny_skia as skia;

// The codes and argument counts must match `CanvasRenderingContext2D` and `Path2D` in `dom.js`.
pub(super) const ARGS: [usize; 45] = [
    0, 0, 6, 6, 1, 1, 1, 1, 1, 1, 1, 0, 0, 2, 2, 4, 6, 4, 1, 0, 1, 4, 4, 4, 4, 11, 5, 4, 6, 8, 5,
    12, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 3, 4, 1,
];

// `None` past the range where tiny-skia drops the whole path, so such a call is skipped like one with
// non-finite arguments.
pub(super) fn device<const N: usize>(
    [a, b, c, d, e, f]: [f64; 6],
    points: [[f64; 2]; N],
) -> Option<[(f32, f32); N]> {
    let mut out = [(0., 0.); N];
    for (out, [x, y]) in out.iter_mut().zip(points) {
        let [x, y] = [a * x + c * y + e, b * x + d * y + f];
        if !(x.abs() <= 1e37 && y.abs() <= 1e37) {
            return None;
        }
        *out = (x as f32, y as f32);
    }
    Some(out)
}

pub(super) fn matrix([a, b, c, d, e, f]: [f64; 6]) -> skia::Transform {
    skia::Transform::from_row(a as f32, b as f32, c as f32, d as f32, e as f32, f as f32)
}

pub(super) fn concat([a, b, c, d, e, f]: [f64; 6], [ma, mb, mc, md, me, mf]: [f64; 6]) -> [f64; 6] {
    [
        a * ma + c * mb,
        b * ma + d * mb,
        a * mc + c * md,
        b * mc + d * md,
        a * me + c * mf + e,
        b * me + d * mf + f,
    ]
}

// Build strokes without translation to preserve width and avoid f32 precision loss. Map paint into the same
// space.
pub(super) fn stroke_space(
    [a, b, c, d, e, f]: [f64; 6],
) -> Option<(skia::Transform, skia::Transform)> {
    let det = a * d - b * c;
    let shift = [(d * e - c * f) / det, (a * f - b * e) / det];
    let linear = matrix([a, b, c, d, 0., 0.]);
    (det != 0. && shift.iter().all(|v| v.is_finite()) && linear.is_finite()).then(|| {
        (
            linear,
            skia::Transform::from_translate(shift[0] as f32, shift[1] as f32),
        )
    })
}

pub(super) fn quad(t: [f64; 6], [x, y, w, h]: [f64; 4]) -> Option<skia::Path> {
    let [p0, p1, p2, p3] = device(t, [[x, y], [x + w, y], [x + w, y + h], [x, y + h]])?;
    let mut path = skia::PathBuilder::new();
    path.move_to(p0.0, p0.1);
    path.line_to(p1.0, p1.1);
    path.line_to(p2.0, p2.1);
    path.line_to(p3.0, p3.1);
    path.close();
    path.finish()
}

pub(super) fn fill_area(
    pixels: &mut skia::Pixmap,
    t: [f64; 6],
    area: [f64; 4],
    paint: &skia::Paint,
    mask: Option<&skia::Mask>,
) {
    let identity = skia::Transform::identity();
    if t[1] == 0. && t[2] == 0. {
        let [x, y, w, h] = area;
        if let Some([(left, top), (right, bottom)]) = device(t, [[x, y], [x + w, y + h]])
            && let Some(rect) = skia::Rect::from_ltrb(
                left.min(right),
                top.min(bottom),
                left.max(right),
                top.max(bottom),
            )
        {
            pixels.fill_rect(rect, paint, identity, mask);
        }
    } else if let Some(path) = quad(t, area) {
        pixels.fill_path(&path, paint, skia::FillRule::Winding, identity, mask);
    }
}

pub(super) fn arc(
    path: &mut skia::PathBuilder,
    t: [f64; 6],
    [cx, cy, rx, ry, rotation, start, end, counter]: [f64; 8],
) {
    use std::f64::consts::{FRAC_PI_2, TAU};
    // As Chrome's `AdjustEndAngle`: a sweep against the direction goes the other way around, and a sweep of
    // a whole turn or more draws the whole circle.
    let sweep = end - start;
    let sweep = if counter == 0. {
        if sweep >= TAU {
            TAU
        } else if sweep < 0. {
            TAU - -sweep % TAU
        } else {
            sweep
        }
    } else if sweep <= -TAU {
        -TAU
    } else if sweep > 0. {
        sweep % TAU - TAU
    } else {
        sweep
    };
    let (sin, cos) = rotation.sin_cos();
    let at = |x: f64, y: f64| {
        let (x, y) = (x * rx, y * ry);
        [cx + x * cos - y * sin, cy + x * sin + y * cos]
    };
    let parts = (sweep.abs() / FRAC_PI_2).ceil().clamp(1., 4.) as usize;
    let step = sweep / parts as f64;
    let k = 4. / 3. * (step / 4.).tan();
    let mut points = [[0.; 2]; 13];
    let (mut s0, mut c0) = start.sin_cos();
    points[0] = at(c0, s0);
    for part in 1..=parts {
        let (s1, c1) = (start + step * part as f64).sin_cos();
        points[part * 3 - 2] = at(c0 - k * s0, s0 + k * c0);
        points[part * 3 - 1] = at(c1 + k * s1, s1 - k * c1);
        points[part * 3] = at(c1, s1);
        (s0, c0) = (s1, c1);
    }
    let Some(points) = device(t, points) else {
        return;
    };
    let (x, y) = points[0];
    if path.is_empty() {
        path.move_to(x, y);
    } else {
        path.line_to(x, y);
    }
    for [c1, c2, end] in points[1..=parts * 3].as_chunks::<3>().0 {
        path.cubic_to(c1.0, c1.1, c2.0, c2.1, end.0, end.1);
    }
}

pub(super) fn trace(path: &mut skia::PathBuilder, t: [f64; 6], code: u8, a: &[f64]) {
    match code {
        12 => path.close(),
        13 | 14 => {
            let Some([(x, y)]) = device(t, [[a[0], a[1]]]) else {
                return;
            };
            if code == 13 || path.is_empty() {
                path.move_to(x, y);
            } else {
                path.line_to(x, y);
            }
        }
        15 => {
            let Some([(x1, y1), (x, y)]) = device(t, [[a[0], a[1]], [a[2], a[3]]]) else {
                return;
            };
            if path.is_empty() {
                path.move_to(x1, y1);
            }
            path.quad_to(x1, y1, x, y);
        }
        16 => {
            let Some([(x1, y1), (x2, y2), (x, y)]) =
                device(t, [[a[0], a[1]], [a[2], a[3]], [a[4], a[5]]])
            else {
                return;
            };
            if path.is_empty() {
                path.move_to(x1, y1);
            }
            path.cubic_to(x1, y1, x2, y2, x, y);
        }
        17 => {
            let [x, y, w, h] = [a[0], a[1], a[2], a[3]];
            let Some(points) = device(t, [[x, y], [x + w, y], [x + w, y + h], [x, y + h]]) else {
                return;
            };
            path.move_to(points[0].0, points[0].1);
            for (x, y) in &points[1..] {
                path.line_to(*x, *y);
            }
            path.close();
        }
        28 | 29 => {
            let args = if code == 28 {
                [a[0], a[1], a[2], a[2], 0., a[3], a[4], a[5]]
            } else {
                [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]]
            };
            if args.iter().all(|v| v.is_finite()) {
                arc(path, t, args);
            }
        }
        30 if a.iter().all(|v| v.is_finite()) => {
            let [x1, y1, x2, y2, r] = [a[0], a[1], a[2], a[3], a[4]];
            let Some([(cx, cy)]) = device(t, [[x1, y1]]) else {
                return;
            };
            let Some(last) = path.last_point().filter(|_| !path.is_empty()) else {
                path.move_to(cx, cy);
                return;
            };
            let [ta, tb, tc, td, te, tf] = t;
            let det = ta * td - tb * tc;
            let [lx, ly] = [f64::from(last.x) - te, f64::from(last.y) - tf];
            let from = [(td * lx - tc * ly) / det, (ta * ly - tb * lx) / det];
            let from = if from.iter().all(|v| v.is_finite()) {
                from
            } else {
                [f64::from(last.x), f64::from(last.y)]
            };
            let (dx0, dy0, dx1, dy1) = (x1 - from[0], y1 - from[1], x2 - x1, y2 - y1);
            let (len0, len1) = (dx0.hypot(dy0), dx1.hypot(dy1));
            let cross = dx0 * dy1 - dy0 * dx1;
            if len0 == 0. || len1 == 0. || r == 0. || cross.abs() < 1e-6 {
                path.line_to(cx, cy);
                return;
            }
            let (ux0, uy0, ux1, uy1) = (dx0 / len0, dy0 / len0, dx1 / len1, dy1 / len1);
            let turn = (ux0 * uy1 - uy0 * ux1).atan2(ux0 * ux1 + uy0 * uy1).abs();
            let (reach, k) = (r * (turn / 2.).tan(), r * 4. / 3. * (turn / 4.).tan());
            let (sx, sy) = (x1 - ux0 * reach, y1 - uy0 * reach);
            let (ex, ey) = (x1 + ux1 * reach, y1 + uy1 * reach);
            let Some([start, c1, c2, end]) = device(
                t,
                [
                    [sx, sy],
                    [sx + ux0 * k, sy + uy0 * k],
                    [ex - ux1 * k, ey - uy1 * k],
                    [ex, ey],
                ],
            ) else {
                return;
            };
            path.line_to(start.0, start.1);
            path.cubic_to(c1.0, c1.1, c2.0, c2.1, end.0, end.1);
        }
        31 if a.iter().all(|v| v.is_finite()) => {
            let [x, y, w, h] = [a[0], a[1], a[2], a[3]];
            let (right, bottom) = (x + w, y + h);
            let (sx, sy) = (w.signum(), h.signum());
            let r: [f64; 8] = std::array::from_fn(|i| a[4 + i] * if i % 2 == 0 { sx } else { sy });
            const CORNER_CURVE: f64 = 0.447_715_25;
            #[rustfmt::skip]
            let Some(points) = device(t, [
                [x + r[0], y],
                [right - r[2], y], [right - r[2] * CORNER_CURVE, y], [right, y + r[3] * CORNER_CURVE], [right, y + r[3]],
                [right, bottom - r[5]], [right, bottom - r[5] * CORNER_CURVE], [right - r[4] * CORNER_CURVE, bottom], [right - r[4], bottom],
                [x + r[6], bottom], [x + r[6] * CORNER_CURVE, bottom], [x, bottom - r[7] * CORNER_CURVE], [x, bottom - r[7]],
                [x, y + r[1]], [x, y + r[1] * CORNER_CURVE], [x + r[0] * CORNER_CURVE, y], [x + r[0], y],
                [x, y],
            ]) else {
                return;
            };
            path.move_to(points[0].0, points[0].1);
            for [line, c1, c2, end] in points[1..17].as_chunks::<4>().0 {
                path.line_to(line.0, line.1);
                path.cubic_to(c1.0, c1.1, c2.0, c2.1, end.0, end.1);
            }
            path.close();
            path.move_to(points[17].0, points[17].1);
        }
        _ => {}
    }
}

pub(super) fn replay(ops: &[f64], t: [f64; 6]) -> Option<skia::Path> {
    let (mut path, mut local, mut stack) = (skia::PathBuilder::new(), t, Vec::new());
    let mut rest = ops;
    while let [code, tail @ ..] = rest {
        let (a, tail) = tail.split_at_checked(*ARGS.get(*code as usize)?)?;
        rest = tail;
        match *code as u8 {
            0 => stack.push(local),
            1 => local = stack.pop().unwrap_or(t),
            2 => local = concat(local, [a[0], a[1], a[2], a[3], a[4], a[5]]),
            code => trace(&mut path, local, code, a),
        }
    }
    path.finish()
}

pub fn svg_path(d: &str) -> Vec<f64> {
    let d = d
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    let tree = svg_tree(
        &format!(r#"<svg xmlns="http://www.w3.org/2000/svg"><path d="{d}"/></svg>"#),
        false,
    );
    let Some(usvg::Node::Path(path)) = tree
        .as_ref()
        .ok()
        .and_then(|tree| tree.root().children().first())
    else {
        return Vec::new();
    };
    let mut ops = Vec::new();
    for segment in path.data().segments() {
        use skia::PathSegment::*;
        let (code, points): (f64, &[skia::Point]) = match &segment {
            MoveTo(p) => (13., std::slice::from_ref(p)),
            LineTo(p) => (14., std::slice::from_ref(p)),
            QuadTo(c, p) => (15., &[*c, *p]),
            CubicTo(c1, c2, p) => (16., &[*c1, *c2, *p]),
            Close => (12., &[]),
        };
        ops.push(code);
        ops.extend(points.iter().flat_map(|p| [f64::from(p.x), f64::from(p.y)]));
    }
    ops
}
