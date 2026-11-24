use super::super::border::shade_border;
use super::cx::{Cx, Frame};
use crate::css;
use crate::css::properties::Prop;
use renderer::{Color, Radii};
use renderer::{Rect, point, px};

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
        let kind = crate::render::effects::border_style(name);
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
