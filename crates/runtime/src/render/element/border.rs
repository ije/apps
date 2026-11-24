use super::cx::{Cx, Frame};
use crate::css;
use crate::css::properties::Prop;
use renderer::{Child, Rect, point, px, size};
use renderer::{Color, Radii};
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
                .host
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
                let (outer, inner) = match kinds[side].as_str() {
                    "groove" => (
                        shade_border(colors[side], raised),
                        shade_border(colors[side], !raised),
                    ),
                    "ridge" => (
                        shade_border(colors[side], !raised),
                        shade_border(colors[side], raised),
                    ),
                    "inset" => (
                        shade_border(colors[side], raised),
                        shade_border(colors[side], raised),
                    ),
                    "outset" => (
                        shade_border(colors[side], !raised),
                        shade_border(colors[side], !raised),
                    ),
                    _ => (colors[side], colors[side]),
                };
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
                // Pseudo-element borders paint on `outer`, box borders on the rect between the insets: the two
                // differ by an ulp at fractional positions.
                let bounds = if pseudo {
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

/// Draws the outline of a box, or the focus ring of `outline-style: auto`.
#[inline(never)]
pub(super) fn outline(
    cx: &Cx<'_>,
    fr: &Frame,
    mut el: renderer::Node,
    radius: Radii<renderer::Length>,
) -> renderer::Node {
    let Cx {
        style,
        units,
        parsed,
        containing,
        ..
    } = *cx;
    let borders = fr.borders;
    let ink = parsed.ink.unwrap_or(Color::BLACK);
    if style.get(Prop::OUTLINE_STYLE) == Some("auto") {
        // Like Chrome, a focus ring ignores the outline width: a 2px ring of the outline color, centered on the border
        // edge moved out by `outline-offset`, or 1px inside it with a border, above a 1px ring outside it of white, or
        // of #101010 in a dark scheme. Its corners follow the border radius, or else have a radius of 3px.
        let offset = style
            .get(Prop::OUTLINE_OFFSET)
            .and_then(|v| units.resolve(v, containing[0]))
            .unwrap_or(0.)
            .trunc()
            - if borders.iter().all(|width| *width >= 1.) {
                1.
            } else {
                0.
            };
        let color = style
            .get(Prop::OUTLINE_COLOR)
            .and_then(css::value::color::paint)
            .unwrap_or(ink);
        let under = renderer::Color::hex(if cx.dark() { 0x101010ff } else { 0xffffffff });
        let mut ring = renderer::canvas(
            |_, _| (),
            move |bounds, _, window| {
                let outer = Rect::between(
                    bounds.origin - point(px(borders[3]), px(borders[0])),
                    bounds.bottom_right() + point(px(borders[1]), px(borders[2])),
                );
                let corners = radius.resolve(outer.size).fit(outer.size);
                let corners = if corners.is_zero() {
                    renderer::Radii::from(px(3.))
                } else {
                    let grown = |radius: renderer::Pixels| (radius + px(offset)).max(px(0.));
                    renderer::Radii {
                        x: corners.x.map(grown),
                        y: corners.y.map(grown),
                        shape: corners.shape,
                    }
                };
                for (at, color) in [(offset + 1., under), (offset, color)] {
                    let path = Rect::between(
                        outer.origin - point(px(at), px(at)),
                        outer.bottom_right() + point(px(at), px(at)),
                    );
                    if path.size.width <= px(0.) || path.size.height <= px(0.) {
                        continue;
                    }
                    let rect = Rect::between(
                        path.origin - point(px(1.), px(1.)),
                        path.bottom_right() + point(px(1.), px(1.)),
                    );
                    let corners = corners.inset(px(-1.), px(-1.), px(-1.), px(-1.));
                    window.draw_box(
                        rect,
                        corners.fit(rect.size),
                        renderer::Color::TRANSPARENT,
                        renderer::Border {
                            widths: renderer::Edges::all(px(2.)),
                            color,
                            style: renderer::BorderStyle::Solid,
                            sides: 0,
                        },
                    );
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        ring.style().escape = renderer::Escape::OUTLINE;
        el = el.child(ring);
    } else if let Some(width) = style
        .get(Prop::OUTLINE_STYLE)
        .filter(|kind| !matches!(*kind, "none" | "hidden"))
        .and_then(|_| {
            units.resolve(
                style.get(Prop::OUTLINE_WIDTH).unwrap_or("3px"),
                containing[0],
            )
        })
        .filter(|width| *width > 0.)
    {
        let grow = width
            + style
                .get(Prop::OUTLINE_OFFSET)
                .and_then(|v| units.resolve(v, containing[0]))
                .unwrap_or(0.);
        let color = style
            .get(Prop::OUTLINE_COLOR)
            .and_then(css::value::color::paint)
            .unwrap_or(ink);
        let name = style.get(Prop::OUTLINE_STYLE).unwrap_or("solid");
        let kind = super::super::effects::border_style(name);
        // The top and left sides, then the right and bottom ones, of each band from the outer edge in.
        let bands: Vec<[renderer::Color; 2]> = match name {
            "inset" => vec![[true, false].map(|dark| shade_border(color, dark))],
            "outset" => vec![[false, true].map(|dark| shade_border(color, dark))],
            "groove" => vec![
                [true, false].map(|dark| shade_border(color, dark)),
                [false, true].map(|dark| shade_border(color, dark)),
            ],
            "ridge" => vec![
                [false, true].map(|dark| shade_border(color, dark)),
                [true, false].map(|dark| shade_border(color, dark)),
            ],
            _ => vec![[color; 2]],
        };
        // Like the border box, the outline is square at corners without a radius.
        let mut outline = renderer::canvas(
            |_, _| (),
            move |bounds, _, window| {
                let outer = Rect::between(
                    bounds.origin - point(px(borders[3]), px(borders[0])),
                    bounds.bottom_right() + point(px(borders[1]), px(borders[2])),
                );
                let corners = radius.resolve(outer.size);
                let grown = |radius: renderer::Pixels| {
                    if radius > px(0.) {
                        (radius + px(grow)).max(px(0.))
                    } else {
                        radius
                    }
                };
                let corners = corners.moved_out(
                    px(grow),
                    renderer::Radii {
                        x: corners.x.map(grown),
                        y: corners.y.map(grown),
                        shape: corners.shape,
                    },
                );
                let band = width / bands.len() as f32;
                for (at, colors) in bands.iter().enumerate() {
                    let inset = px(band * at as f32 - grow);
                    let rect = Rect::between(
                        outer.origin + point(inset, inset),
                        outer.bottom_right() - point(inset, inset),
                    );
                    let inner = px(band * at as f32);
                    let corners = corners.inset(inner, inner, inner, inner).fit(rect.size);
                    let split = colors[0] != colors[1];
                    for (sides, color) in [(if split { 9 } else { 0 }, colors[0]), (6, colors[1])]
                        .into_iter()
                        .take(1 + usize::from(split))
                    {
                        window.draw_box(
                            rect,
                            corners,
                            renderer::Color::TRANSPARENT,
                            renderer::Border {
                                widths: renderer::Edges::all(px(band)),
                                color,
                                style: kind,
                                sides,
                            },
                        );
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        // The outline takes the clips of its box, not the overflow clip of the box itself.
        outline.style().escape = renderer::Escape::OUTLINE;
        el = el.child(outline);
    }
    el
}
