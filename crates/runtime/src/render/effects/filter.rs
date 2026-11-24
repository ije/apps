use super::{Ids, index};
use crate::css;
use crate::{
    css::{value::tokens::components, value::units::Lengths},
    dom::Document,
};
use renderer::{BlendMode, Color, PaintEffect, Pixels, point, px};

fn color_matrix(filter: Option<css::value::Filter>, opacity: f32) -> Option<[f32; 20]> {
    if filter.is_none() && opacity >= 1. {
        return None;
    }
    let rows = filter.map_or(
        [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
        |filter| filter.0,
    );
    let mut matrix = [0.; 20];
    for (row, values) in rows.iter().enumerate() {
        matrix[row * 5..row * 5 + 3].copy_from_slice(&values[..3]);
        matrix[row * 5 + 4] = values[3];
    }
    matrix[18] = opacity;
    Some(matrix)
}

fn then_matrix(inner: &[f32; 20], outer: &[f32; 20]) -> [f32; 20] {
    let mut result = [0.; 20];
    for row in 0..4 {
        for column in 0..5 {
            result[row * 5 + column] = (0..4)
                .map(|k| outer[row * 5 + k] * inner[k * 5 + column])
                .sum::<f32>()
                + if column == 4 { outer[row * 5 + 4] } else { 0. };
        }
    }
    result
}

pub(crate) fn filters(value: &str, units: &Lengths, doc: &Document, ids: &Ids) -> Vec<PaintEffect> {
    let mut effects: Vec<PaintEffect> = Vec::new();
    for part in components(value, ' ') {
        let Some((name, args)) = part.strip_suffix(')').and_then(|part| part.split_once('('))
        else {
            continue;
        };
        match name.to_ascii_lowercase().as_str() {
            "blur" => {
                let sigma = if args.trim().is_empty() {
                    0.
                } else {
                    units.resolve(args.trim(), 0.).unwrap_or(0.)
                };
                if sigma > 0. {
                    effects.push(PaintEffect {
                        blur: px(sigma),
                        ..Default::default()
                    });
                }
            }
            "drop-shadow" => {
                // Chrome uses the blur length as the standard deviation, not half of it as box-shadow does.
                for shadow in css::value::box_shadows(units, args, false) {
                    effects.push(PaintEffect {
                        shadow: Some((shadow.offset, shadow.blur_radius * 2., shadow.color)),
                        ..Default::default()
                    });
                }
            }
            "url" => effects.extend(svg_filter(args, units, doc, ids)),
            _ => {
                let (filter, opacity) = css::value::Filter::parse(part, units);
                let Some(matrix) = color_matrix(filter, opacity) else {
                    continue;
                };
                if let Some(last) = effects.last_mut()
                    && last.blur == px(0.)
                    && last.shadow.is_none()
                {
                    last.color_matrix = Some(match last.color_matrix {
                        Some(inner) => then_matrix(&inner, &matrix),
                        None => matrix,
                    });
                } else {
                    effects.push(PaintEffect {
                        color_matrix: Some(matrix),
                        ..Default::default()
                    });
                }
            }
        }
    }
    effects
}

fn svg_filter(reference: &str, units: &Lengths, doc: &Document, ids: &Ids) -> Vec<PaintEffect> {
    let id = reference.trim().trim_matches(['"', '\'']);
    let Some(id) = id.strip_prefix('#') else {
        return Vec::new();
    };
    let Some(filter) = index(ids, doc).get(id).and_then(|nodes| {
        nodes
            .iter()
            .copied()
            .find(|&node| doc.nodes[node].tag == "filter")
    }) else {
        return Vec::new();
    };
    let mut effects = Vec::new();
    let mut offset = point(px(0.), px(0.));
    for &child in &doc.nodes[filter].children {
        let node = &doc.nodes[child];
        let numbers = |name: &str| -> Vec<f32> {
            node.attrs
                .get(name)
                .map(|value| {
                    value
                        .split(|c: char| c.is_whitespace() || c == ',')
                        .filter_map(|part| part.parse().ok())
                        .collect()
                })
                .unwrap_or_default()
        };
        match node.tag.to_ascii_lowercase().as_str() {
            "fegaussianblur" => {
                let sigma = numbers("stdDeviation").first().copied().unwrap_or(0.);
                if sigma > 0. {
                    effects.push(PaintEffect {
                        blur: px(sigma),
                        ..Default::default()
                    });
                }
            }
            "feoffset" => {
                offset = point(
                    px(numbers("dx").first().copied().unwrap_or(0.)),
                    px(numbers("dy").first().copied().unwrap_or(0.)),
                );
            }
            "fedropshadow" => {
                let color = node
                    .attrs
                    .get("flood-color")
                    .and_then(|value| css::value::color::color(value))
                    .unwrap_or((0, 255));
                let opacity = numbers("flood-opacity").first().copied().unwrap_or(1.);
                let mut ink: Color = renderer::Color::hex(color.0 << 8 | color.1 as u32);
                ink.a *= opacity;
                effects.push(PaintEffect {
                    shadow: Some((
                        point(
                            px(numbers("dx").first().copied().unwrap_or(2.)),
                            px(numbers("dy").first().copied().unwrap_or(2.)),
                        ),
                        px(numbers("stdDeviation").first().copied().unwrap_or(2.)),
                        ink,
                    )),
                    ..Default::default()
                });
            }
            "fecolormatrix" => {
                let kind = node.attrs.get("type").map_or("matrix", String::as_str);
                let values = numbers("values");
                let css = match kind {
                    "saturate" => format!("saturate({})", values.first().copied().unwrap_or(1.)),
                    "hueRotate" => {
                        format!("hue-rotate({}deg)", values.first().copied().unwrap_or(0.))
                    }
                    _ => String::new(),
                };
                let matrix = if kind == "matrix" && values.len() == 20 {
                    values.try_into().ok()
                } else if kind == "luminanceToAlpha" {
                    let mut matrix = [0.; 20];
                    matrix[15..18].copy_from_slice(&[0.2125, 0.7154, 0.0721]);
                    Some(matrix)
                } else {
                    let (filter, opacity) = css::value::Filter::parse(&css, units);
                    color_matrix(filter, opacity)
                };
                if let Some(matrix) = matrix {
                    effects.push(PaintEffect {
                        color_matrix: Some(matrix),
                        ..Default::default()
                    });
                }
            }
            _ => {}
        }
    }
    if offset != point(px(0.), px(0.))
        && let Some(blur) = effects.iter().position(|effect| effect.blur > px(0.))
    {
        let sigma = effects.remove(blur).blur;
        effects.push(PaintEffect {
            shadow: Some((offset, sigma, renderer::Color::BLACK.fade(0.5))),
            ..Default::default()
        });
    }
    effects
}

/// The argument of a filter list that is one `blur()`.
pub(crate) fn lone_blur(value: Option<&str>) -> Option<&str> {
    let [part] = *components(value?, ' ') else {
        return None;
    };
    let (name, args) = part.strip_suffix(')')?.split_once('(')?;
    name.eq_ignore_ascii_case("blur").then_some(args.trim())
}

pub(crate) fn backdrop(value: &str, units: &Lengths) -> Option<(Pixels, Option<[f32; 20]>)> {
    let mut blur = px(0.);
    let mut matrix: Option<[f32; 20]> = None;
    for effect in filters(value, units, &Document::default(), &Ids::default()) {
        blur = blur.max(effect.blur);
        if let Some(next) = effect.color_matrix {
            matrix = Some(match matrix {
                Some(inner) => then_matrix(&inner, &next),
                None => next,
            });
        }
    }
    (blur > px(0.) || matrix.is_some()).then_some((blur, matrix))
}

pub(crate) fn blend_mode(value: &str) -> BlendMode {
    match value.trim() {
        "multiply" => BlendMode::Multiply,
        "screen" => BlendMode::Screen,
        "overlay" => BlendMode::Overlay,
        "darken" => BlendMode::Darken,
        "lighten" => BlendMode::Lighten,
        "color-dodge" => BlendMode::ColorDodge,
        "color-burn" => BlendMode::ColorBurn,
        "hard-light" => BlendMode::HardLight,
        "soft-light" => BlendMode::SoftLight,
        "difference" => BlendMode::Difference,
        "exclusion" => BlendMode::Exclusion,
        "hue" => BlendMode::Hue,
        "saturation" => BlendMode::Saturation,
        "color" => BlendMode::Color,
        "luminosity" => BlendMode::Luminosity,
        "plus-lighter" => BlendMode::PlusLighter,
        _ => BlendMode::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_split_into_layers() {
        let units = Lengths::new(16., 800., 600.);
        let doc = Document::default();
        let effects = filters(
            "grayscale(1) contrast(2) blur(4px) drop-shadow(1px 2px 6px red) opacity(.5)",
            &units,
            &doc,
            &Ids::default(),
        );
        assert_eq!(effects.len(), 4);
        assert!(effects[0].color_matrix.is_some());
        assert_eq!(effects[1].blur, px(4.));
        assert_eq!(
            effects[2].shadow.map(|(offset, blur, _)| (offset, blur)),
            Some((point(px(1.), px(2.)), px(6.)))
        );
        assert_eq!(effects[3].color_matrix.map(|matrix| matrix[18]), Some(0.5));
        let (blur, matrix) = backdrop("blur(10px) saturate(1.5)", &units).unwrap();
        assert_eq!(blur, px(10.));
        assert!(matrix.is_some());
        assert_eq!(blend_mode("multiply"), BlendMode::Multiply);
        assert_eq!(blend_mode("unknown"), BlendMode::Normal);
    }
}
