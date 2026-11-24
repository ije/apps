use super::{Ids, index};
use crate::css;
use crate::css::properties::Prop;
use crate::{
    css::{properties::Style, value::tokens::components, value::units::Lengths},
    dom::Document,
};
use renderer::{Corners, Length, Pixels, Radii, px};

pub(crate) enum Clip {
    Rounded([f32; 4], [f32; 4]),
    Shape(String),
}

pub(crate) fn clip(
    value: &str,
    units: &Lengths,
    size: [f32; 2],
    doc: &Document,
    ids: &Ids,
) -> Option<Clip> {
    let [width, height] = size;
    let (name, args) = components(value, ' ')
        .into_iter()
        .find_map(|part| part.strip_suffix(')')?.split_once('('))?;
    let svg = Clip::Shape;
    match name.to_ascii_lowercase().as_str() {
        "inset" | "rect" | "xywh" => {
            let (rect, radii) = css::value::clip_path(value, units, size)?;
            let round = args.split_once(" round ").map(|(_, round)| round.trim());
            let Some(round) = round.filter(|round| round.contains(['%', '/'])) else {
                return Some(Clip::Rounded(rect, radii));
            };
            // Percent and `/` radii can be elliptical. As for border-radius, percentages take the
            // reference box, and radii that don't fit scale down together.
            let (horizontal, vertical) = round.split_once('/').unwrap_or((round, round));
            let corners = |list: &str, base: f32| -> Option<[f32; 4]> {
                let values = components(list.trim(), ' ');
                let values = match values[..] {
                    [a] => [a; 4],
                    [a, b] => [a, b, a, b],
                    [a, b, c] => [a, b, c, b],
                    [a, b, c, d] => [a, b, c, d],
                    _ => return None,
                };
                let mut out = [0.; 4];
                for (corner, value) in values.into_iter().enumerate() {
                    out[corner] = units.resolve(value, base)?.max(0.);
                }
                Some(out)
            };
            let [w, h] = [rect[2], rect[3]];
            let (mut rx, mut ry) = (corners(horizontal, width)?, corners(vertical, height)?);
            let fit = [
                w / (rx[0] + rx[1]),
                h / (ry[1] + ry[2]),
                w / (rx[2] + rx[3]),
                h / (ry[3] + ry[0]),
            ]
            .into_iter()
            .fold(1f32, f32::min);
            rx = rx.map(|value| value * fit);
            ry = ry.map(|value| value * fit);
            if rx == ry {
                return Some(Clip::Rounded(rect, rx));
            }
            let [x, y] = [rect[0], rect[1]];
            Some(svg(format!(
                "<path d=\"M{} {y}H{}A{} {} 0 0 1 {} {}V{}A{} {} 0 0 1 {} {}H{}A{} {} 0 0 1 {x} {}V{}A{} {} 0 0 1 {} {y}Z\"/>",
                x + rx[0],
                x + w - rx[1],
                rx[1],
                ry[1],
                x + w,
                y + ry[1],
                y + h - ry[2],
                rx[2],
                ry[2],
                x + w - rx[2],
                y + h,
                x + rx[3],
                rx[3],
                ry[3],
                y + h - ry[3],
                y + ry[0],
                rx[0],
                ry[0],
                x + rx[0],
            )))
        }
        "circle" | "ellipse" => {
            let (rect, radii) = css::value::clip_path(value, units, size)?;
            if (rect[2] - rect[3]).abs() <= rect[2].max(rect[3]) * 1e-4 {
                return Some(Clip::Rounded(rect, radii));
            }
            Some(svg(format!(
                "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"/>",
                rect[0] + rect[2] / 2.,
                rect[1] + rect[3] / 2.,
                rect[2] / 2.,
                rect[3] / 2.
            )))
        }
        "polygon" => {
            let mut parts = components(args, ',');
            let rule = match parts.first().map(|part| part.trim()) {
                Some(rule @ ("evenodd" | "nonzero")) => {
                    parts.remove(0);
                    rule
                }
                _ => "nonzero",
            };
            let mut points = String::new();
            for part in parts {
                let pair = components(part.trim(), ' ');
                let [x, y] = <[&str; 2]>::try_from(pair).ok()?;
                points.push_str(&format!(
                    "{},{} ",
                    units.resolve(x, width)?,
                    units.resolve(y, height)?
                ));
            }
            Some(svg(format!(
                "<polygon fill-rule=\"{rule}\" points=\"{points}\"/>"
            )))
        }
        "path" => {
            let mut parts = components(args, ',');
            let rule = if parts.len() > 1 {
                parts.remove(0).trim()
            } else {
                "nonzero"
            };
            let data = parts.first()?.trim().trim_matches(['"', '\'']);
            Some(svg(format!(
                "<path fill-rule=\"{rule}\" d=\"{}\"/>",
                crate::dom::escape(data)
            )))
        }
        "url" => {
            let id = args.trim().trim_matches(['"', '\'']).strip_prefix('#')?;
            let clip = index(ids, doc)
                .get(id)?
                .iter()
                .copied()
                .find(|&node| doc.nodes[node].tag.eq_ignore_ascii_case("clipPath"))?;
            let object = doc.nodes[clip]
                .attrs
                .get("clipPathUnits")
                .is_some_and(|value| value == "objectBoundingBox");
            let shapes: String = doc.nodes[clip]
                .children
                .iter()
                .map(|&child| {
                    doc.serialize_svg(child, false, &|node| {
                        doc.nodes[node]
                            .attrs
                            .get("style")
                            .map(|style| vec![("style", style.clone())])
                            .unwrap_or_default()
                    })
                })
                .collect();
            Some(svg(if object {
                format!("<g transform=\"scale({width} {height})\">{shapes}</g>")
            } else {
                shapes
            }))
        }
        _ => None,
    }
}

pub(crate) fn radii(style: &Style, units: &Lengths, basis: impl Fn() -> [f32; 2]) -> Radii<Length> {
    let mut radii = Radii::default();
    for (prop, shape, x, y, code) in [
        (
            Prop::BORDER_TOP_LEFT_RADIUS,
            Prop::CORNER_TOP_LEFT_SHAPE,
            &mut radii.x.top_left,
            &mut radii.y.top_left,
            &mut radii.shape.top_left,
        ),
        (
            Prop::BORDER_TOP_RIGHT_RADIUS,
            Prop::CORNER_TOP_RIGHT_SHAPE,
            &mut radii.x.top_right,
            &mut radii.y.top_right,
            &mut radii.shape.top_right,
        ),
        (
            Prop::BORDER_BOTTOM_RIGHT_RADIUS,
            Prop::CORNER_BOTTOM_RIGHT_SHAPE,
            &mut radii.x.bottom_right,
            &mut radii.y.bottom_right,
            &mut radii.shape.bottom_right,
        ),
        (
            Prop::BORDER_BOTTOM_LEFT_RADIUS,
            Prop::CORNER_BOTTOM_LEFT_SHAPE,
            &mut radii.x.bottom_left,
            &mut radii.y.bottom_left,
            &mut radii.shape.bottom_left,
        ),
    ] {
        let Some(value) = style.get(prop) else {
            continue;
        };
        let mut depth = 0;
        let (first, second) = match value.find(|c: char| {
            depth += i32::from(c == '(') - i32::from(c == ')');
            c == ' ' && depth == 0
        }) {
            Some(at) => (&value[..at], Some(value[at..].trim())),
            None => (value, None),
        };
        let length = |part: &str, axis: usize| match part
            .strip_suffix('%')
            .and_then(|percent| percent.parse::<f32>().ok())
            .filter(|percent| percent.is_finite())
        {
            Some(percent) => Length::Fraction(percent.max(0.) / 100.),
            None => units
                .resolve(
                    part,
                    if part.contains('%') {
                        basis()[axis]
                    } else {
                        0.
                    },
                )
                .map_or(Length::Auto, |length| Length::Px(px(length.max(0.)))),
        };
        *x = length(first, 0);
        *y = match second {
            Some(second) => length(second, 1),
            None if first.contains('%') => length(first, 1),
            None => *x,
        };
        match style.get(shape).and_then(css::value::corner_shape) {
            // Like Chrome, a corner of 16 or more is square, with no radius.
            Some(k) if k >= 16. => (*x, *y) = (Length::Px(px(0.)), Length::Px(px(0.))),
            Some(k) => *code = renderer::corner_shape(k as f32),
            None => {}
        }
    }
    radii
}

pub(crate) fn corners(radii: [f32; 4]) -> Corners<Pixels> {
    Corners {
        top_left: px(radii[0]),
        top_right: px(radii[1]),
        bottom_right: px(radii[2]),
        bottom_left: px(radii[3]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_paths_become_shapes() {
        let units = Lengths::new(16., 800., 600.);
        let doc = Document::parse(
            "<svg><clipPath id=c><rect width=5 height=5></rect></clipPath></svg><p id=c></p>",
        );
        let ids = &Ids::default();
        assert!(matches!(
            clip("inset(10px round 4px)", &units, [100., 60.], &doc, ids),
            Some(Clip::Rounded([10., 10., 80., 40.], [4., 4., 4., 4.]))
        ));
        let Some(Clip::Shape(polygon)) = clip(
            "polygon(50% 0, 100% 100%, 0 100%)",
            &units,
            [100., 60.],
            &doc,
            ids,
        ) else {
            panic!("polygon");
        };
        assert!(polygon.contains("points=\"50,0 100,60 0,60 \""));
        let Some(Clip::Shape(pill)) = clip("inset(0 round 50%)", &units, [100., 60.], &doc, ids)
        else {
            panic!("elliptical inset");
        };
        assert!(pill.contains("M50 0H50A50 30 0 0 1 100 30"), "{pill}");
        assert!(matches!(
            clip("inset(0 round 80px)", &units, [100., 60.], &doc, ids),
            Some(Clip::Rounded(_, [80., 80., 80., 80.]))
        ));
        let Some(Clip::Rounded(rect, radii)) = clip("circle(40%)", &units, [100., 60.], &doc, ids)
        else {
            panic!("circle");
        };
        assert!((rect[2] - rect[3]).abs() < 1e-3 && (radii[0] * 2. - rect[2]).abs() < 1e-3);
        assert!(matches!(
            clip("ellipse(40% 20%)", &units, [100., 60.], &doc, ids),
            Some(Clip::Shape(_))
        ));
        assert!(matches!(
            clip("path('M0 0 L10 0 L5 5 Z')", &units, [100., 60.], &doc, ids),
            Some(Clip::Shape(_))
        ));
        let Some(Clip::Shape(shapes)) = clip("url(#c)", &units, [100., 60.], &doc, ids) else {
            panic!("url");
        };
        assert!(shapes.contains("<rect"));
    }
}
