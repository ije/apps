use renderer::{Child, Rect, point, px, size};
use std::rc::Rc;

pub(super) fn fieldset_border(
    legend: usize,
    height: f32,
    middle: f32,
    widths: [f32; 4],
    colors: [renderer::Color; 4],
    kinds: [&str; 4],
    view: Rc<super::super::ViewState>,
) -> Child {
    let kinds = kinds.map(str::to_owned);
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            // The legend can move without a new box here.
            window.paint_volatile();
            let gap = view
                .store
                .borrow()
                .page_state(view.id)
                .and_then(|dom| dom.geometry.mirror.get(&legend).copied())
                .map(|[x, _, width, _]| [px(x), px(x + width)]);
            let top = bounds.top() + px(middle - widths[0] / 2.);
            let [left, right, bottom] = [bounds.left(), bounds.right(), bounds.bottom()];
            let sides = [
                Rect::between(point(left, top), point(right, top + px(widths[0]))),
                Rect::between(point(right - px(widths[1]), top), point(right, bottom)),
                Rect::between(point(left, bottom - px(widths[2])), point(right, bottom)),
                Rect::between(point(left, top), point(left + px(widths[3]), bottom)),
            ];
            for (side, rect) in sides.into_iter().enumerate() {
                if widths[side] <= 0. {
                    continue;
                }
                let raised = side == 0 || side == 3;
                let (outer, inner) = bevel(&kinds[side], colors[side], raised, shade_border);
                let inner = inner.unwrap_or(outer);
                let half = px(widths[side] / 2.);
                let halves = match side {
                    0 => [
                        (rect.top(), rect.top() + half),
                        (rect.top() + half, rect.bottom()),
                    ],
                    2 => [
                        (rect.bottom() - half, rect.bottom()),
                        (rect.top(), rect.bottom() - half),
                    ],
                    1 => [
                        (rect.right() - half, rect.right()),
                        (rect.left(), rect.right() - half),
                    ],
                    _ => [
                        (rect.left(), rect.left() + half),
                        (rect.left() + half, rect.right()),
                    ],
                };
                for ((start, end), color) in halves.into_iter().zip([outer, inner]) {
                    let part = if side % 2 == 0 {
                        Rect::between(point(rect.left(), start), point(rect.right(), end))
                    } else {
                        Rect::between(point(start, rect.top()), point(end, rect.bottom()))
                    };
                    let pieces = match gap.filter(|_| side == 0) {
                        Some([from, to]) => vec![
                            Rect::between(part.origin, point(from.max(part.left()), part.bottom())),
                            Rect::between(
                                point(to.min(part.right()), part.top()),
                                part.bottom_right(),
                            ),
                        ],
                        None => vec![part],
                    };
                    for piece in pieces.into_iter().filter(|piece| piece.size.width > px(0.)) {
                        window.draw_rect(piece, color);
                    }
                }
            }
        },
    )
    .absolute()
    .top(px(-height))
    .left(px(-widths[3]))
    .right(px(-widths[1]))
    .bottom(px(-widths[2]))
    .into()
}

// The outer color of a side and, for `groove` and `ridge`, its inner one. The top and left sides are `raised`.
pub(super) fn bevel(
    kind: &str,
    color: renderer::Color,
    raised: bool,
    shade: impl Fn(renderer::Color, bool) -> renderer::Color,
) -> (renderer::Color, Option<renderer::Color>) {
    match kind {
        "inset" => (shade(color, raised), None),
        "outset" => (shade(color, !raised), None),
        "groove" => (shade(color, raised), Some(shade(color, !raised))),
        "ridge" => (shade(color, !raised), Some(shade(color, raised))),
        _ => (color, None),
    }
}

// The bands of a border canvas: a `groove` or `ridge` side draws its outer and inner halves in two bands.
pub(super) fn bands(
    borders: [f32; 4],
    colors: [renderer::Color; 4],
    kinds: [&str; 4],
    shade: impl Fn(renderer::Color, bool) -> renderer::Color,
) -> Vec<([f32; 4], [f32; 4], [renderer::Color; 4])> {
    let mut bands = vec![([0.; 4], borders, colors)];
    let split = kinds.iter().any(|kind| matches!(*kind, "groove" | "ridge"));
    if split {
        bands.push(([0.; 4], [0.; 4], [renderer::Color::TRANSPARENT; 4]));
    }
    for side in 0..4 {
        let (outer, inner) = bevel(kinds[side], colors[side], side == 0 || side == 3, &shade);
        bands[0].2[side] = outer;
        if let Some(inner) = inner {
            let half = borders[side] / 2.;
            bands[0].1[side] = half;
            bands[1].0[side] = half;
            bands[1].1[side] = half;
            bands[1].2[side] = inner;
        }
    }
    if split {
        bands[1].0 = std::array::from_fn(|side| {
            if bands[1].1[side] > 0. {
                bands[1].0[side]
            } else {
                borders[side]
            }
        });
    }
    bands
}

pub(super) fn shade_border(color: renderer::Color, dark: bool) -> renderer::Color {
    let value = color.r.max(color.g).max(color.b);
    if value <= 0. {
        let gray = if dark { 0.33 } else { 0.66 };
        return renderer::Color {
            r: gray,
            g: gray,
            b: gray,
            a: color.a,
        };
    }
    let end = if dark { 0. } else { 1. };
    let distance = (color.r - end).powi(2) + (color.g - end).powi(2) + (color.b - end).powi(2);
    if distance <= 3. * (if dark { 32. } else { 20. } / 255.0f32).powi(2) {
        return color;
    }
    let factor = if dark {
        ((value - 0.33) / value).max(0.)
    } else {
        (value + 0.33).min(1.) / value
    };
    renderer::Color {
        r: (color.r * factor).min(1.),
        g: (color.g * factor).min(1.),
        b: (color.b * factor).min(1.),
        a: color.a,
    }
}

pub(super) fn border_canvas(
    bands: Vec<([f32; 4], [f32; 4], [renderer::Color; 4])>,
    borders: [f32; 4],
    radius: renderer::Radii<renderer::Length>,
    kinds: [renderer::BorderStyle; 4],
    pseudo: bool,
) -> renderer::Node {
    renderer::canvas(
        |_, _| (),
        move |bounds, _, window| {
            let outer = Rect::new(
                point(
                    bounds.left() - px(borders[3]),
                    bounds.top() - px(borders[0]),
                ),
                size(
                    bounds.size.width + px(borders[1] + borders[3]),
                    bounds.size.height + px(borders[0] + borders[2]),
                ),
            );
            for (inset, widths, colors) in &bands {
                // Pseudo-element borders paint their outer band on `outer`, box borders on the rect between the
                // insets: the two differ by an ulp at fractional positions.
                let bounds = if pseudo && *inset == [0.; 4] {
                    outer
                } else {
                    Rect::between(
                        outer.origin + point(px(inset[3]), px(inset[0])),
                        outer.bottom_right() - point(px(inset[1]), px(inset[2])),
                    )
                };
                let corners = radius
                    .resolve(outer.size)
                    .inset(px(inset[0]), px(inset[1]), px(inset[2]), px(inset[3]))
                    .fit(bounds.size);
                // Sides that look the same draw at once, so their joins have no seams.
                let same = |a: usize, b: usize| {
                    (colors[a], kinds[a]) == (colors[b], kinds[b]) && widths[b] > 0.
                };
                for side in 0..4 {
                    if widths[side] <= 0. || (0..side).any(|before| same(side, before)) {
                        continue;
                    }
                    let sides = (side..4)
                        .filter(|&other| same(side, other))
                        .fold(0, |sides, other| sides | 1 << other);
                    window.draw_box(
                        bounds,
                        corners,
                        renderer::Color::TRANSPARENT,
                        renderer::Border {
                            widths: renderer::Edges {
                                top: px(widths[0]),
                                right: px(widths[1]),
                                bottom: px(widths[2]),
                                left: px(widths[3]),
                            },
                            color: colors[side],
                            style: kinds[side],
                            sides: if sides == 15 { 0 } else { sides },
                        },
                    );
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

pub(super) fn inset_shadows(
    shadows: Vec<renderer::BoxShadow>,
    radius: renderer::Radii<renderer::Length>,
    borders: [f32; 4],
) -> Child {
    let mut node = renderer::canvas_within(
        |_, _| (),
        move |bounds, _, window| {
            let outer = size(
                bounds.size.width + px(borders[1] + borders[3]),
                bounds.size.height + px(borders[0] + borders[2]),
            );
            let radii = radius.resolve(outer).inset(
                px(borders[0]),
                px(borders[1]),
                px(borders[2]),
                px(borders[3]),
            );
            window.draw_shadows(bounds, radii, &shadows, true)
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    // Like the background, the shadow doesn't scroll with the content or add to its scroll range.
    node.style().escape = renderer::Escape::OUTLINE;
    node.into()
}
