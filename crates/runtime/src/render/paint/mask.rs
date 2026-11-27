use super::super::effects::clip;
use super::background;
use crate::css;
use crate::css::properties::Prop;
use renderer::{Pixels, Rect, Window};
use std::collections::HashMap;

/// The mask image of a box or a pseudo-element `key` from its mask layers and its clip path shapes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mask(
    view: &super::super::ViewState,
    dom: &mut crate::render::PageState,
    assets: &HashMap<String, std::path::PathBuf>,
    key: (usize, u8),
    style: &css::properties::Style,
    units: css::value::units::Lengths,
    shapes: Option<(String, Option<[f32; 4]>)>,
    bounds: Rect<Pixels>,
    window: &mut Window,
) -> Option<(std::sync::Arc<renderer::RenderImage>, Rect<Pixels>)> {
    let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
    let base = dom.base.clone();
    let images = style
        .get(Prop::MASK_IMAGE)
        .filter(|value| !value.is_empty() && *value != "none");
    (shapes.is_some() || images.is_some()).then(|| {
        // The mask covers the border box, or a shape alone as far as the window shows it.
        let mut area = [0., 0., size[0], size[1]];
        if images.is_none()
            && let Some(reach) = shapes.as_ref().and_then(|shape| shape.1)
        {
            let view = window.content_mask().bounds;
            let step = |room: Pixels| (f32::from(room) / 256.).ceil().clamp(0., 16.) * 256.;
            let room = [
                -step(bounds.left() - view.left()),
                -step(bounds.top() - view.top()),
                size[0].ceil() + step(view.right() - bounds.right()),
                size[1].ceil() + step(view.bottom() - bounds.bottom()),
            ];
            let reach = [reach[0].floor(), reach[1].floor(), reach[2].ceil(), reach[3].ceil()];
            let [left, top] = [0, 1].map(|side| reach[side].max(room[side]));
            let [right, bottom] = [2, 3].map(|side| reach[side].min(room[side]));
            area = [left, top, (right - left).max(1.), (bottom - top).max(1.)];
            if reach != [left, top, right, bottom] {
                window.paint_volatile();
            }
        }
        let shapes = shapes.map(|shape| shape.0);
        let mut inputs = String::new();
        for part in MASK
            .map(|prop| style.get(prop).unwrap_or(""))
            .into_iter()
            .chain(shapes.as_deref())
        {
            inputs.push_str(part);
            inputs.push('\0');
        }
        super::super::effects::gradient::push_bits(&mut inputs, size, &units);
        for side in area {
            inputs.push_str(&side.to_bits().to_string());
            inputs.push(' ');
        }
        // The boxes of layers read the style.
        if [Prop::MASK_ORIGIN, Prop::MASK_CLIP].iter().any(|&prop| style.contains(prop))
            || clip::RADII.iter().any(|&prop| style.contains(prop))
        {
            for prop in BOXES {
                inputs.push_str(style.get(prop).unwrap_or(""));
                inputs.push(' ');
            }
        }
        let [left, top, width, height] = area;
        let bounds = Rect::new(
            bounds.origin + renderer::point(renderer::px(left), renderer::px(top)),
            renderer::size(renderer::px(width), renderer::px(height)),
        );
        let scale = window.scale_factor();
        let raster = [width, height].map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
        let masks = &mut dom.rasters.masks;
        if masks.get(&key).is_none_or(|(old, image)| {
            *old != inputs
                || image.as_ref().is_some_and(|image| {
                    (image.size().width as u32) != raster[0]
                        || (image.size().height as u32) != raster[1]
                })
        }) {
            // Boxes with the same mask inputs and size share one image.
            let shared = masks.values().find_map(|(old, image)| {
                image
                    .as_ref()
                    .filter(|image| {
                        *old == inputs
                            && [image.size().width as u32, image.size().height as u32] == raster
                    })
                    .cloned()
            });
            let image = if shared.is_some() {
                shared
            } else if shapes.is_none()
                && let Some(bytes) = super::super::effects::gradient::axis_mask(style, &units, size, raster)
            {
                bytes.map(|bytes| {
                    std::sync::Arc::new(renderer::RenderImage::new(raster[0], raster[1], bytes))
                })
            } else {
                let mut files = Vec::new();
                let layers = mask_svg(style, units, size, scale, base.as_ref(), assets, &mut files);
                let shapes = shapes.as_ref().map(|shape| match left != 0. || top != 0. {
                    true => format!("<g transform=\"translate({} {})\">{shape}</g>", -left, -top),
                    false => shape.clone(),
                });
                // Hold the decoded files: `retire` drops the bitmaps that nothing holds.
                let mut sources = view.mask_sources.borrow_mut();
                let held = sources.entry(key).or_default();
                held.retain(|(file, _)| files.iter().any(|path| path.to_string_lossy() == *file));
                let mut kinds = Vec::with_capacity(files.len());
                let mut decoded = Vec::new();
                for path in &files {
                    let file = path.to_string_lossy();
                    let source = match held.iter().position(|(held, _)| *held == file) {
                        Some(index) => Some(&held[index].1),
                        None => background::load(view, &mut dom.rasters.bitmaps, key.0, &file, path, window)
                            .map(|source| &held.push_mut((file.into_owned(), source)).1),
                    };
                    // A file that still decodes is a transparent layer until the window redraws.
                    kinds.push(source.map(|(bitmap, tree)| match tree {
                        Some((tree, _)) => usvg::ImageKind::SVG(tree.clone()),
                        None => {
                            let stub = super::super::raster::resvg::stub(bitmap);
                            decoded.push((stub.clone(), bitmap.clone()));
                            usvg::ImageKind::PNG(stub)
                        }
                    }));
                }
                if kinds.iter().any(Option::is_none) {
                    inputs.push_str("\0decoding");
                }
                let source = match (&shapes, layers) {
                    (Some(shape), Some(layers)) => format!(
                        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\"><defs><clipPath id=\"apps-clip\">{shape}</clipPath></defs><g clip-path=\"url(#apps-clip)\">{layers}</g></svg>"
                    ),
                    (Some(shape), None) => format!(
                        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">{shape}</svg>"
                    ),
                    (None, Some(layers)) => format!(
                        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">{layers}</svg>"
                    ),
                    (None, None) => return None,
                };
                let mut options = super::super::raster::svg::svg_options(false, false);
                options.image_href_resolver.resolve_string = Box::new(move |href, _| {
                    kinds.get(href.strip_prefix("apps-mask:")?.parse::<usize>().ok()?)?.clone()
                });
                match usvg::Tree::from_str(&source, &options)
                    .map_err(anyhow::Error::from)
                    .and_then(|tree| {
                        super::super::raster::resvg::decoded(decoded, || {
                            super::super::raster::svg::render_svg(&tree, raster[0], raster[1], None)
                        })
                    }) {
                    Ok(image) => Some(image),
                    // Keep the failure too, or every paint builds and reports the same mask again.
                    Err(error) => {
                        eprintln!("apps: mask: {error}");
                        None
                    }
                }
            };
            if let Some((_, Some(old))) = masks.insert(key, (inputs, image))
                && std::sync::Arc::strong_count(&old) == 1
            {
                window.release_image(old);
            }
        }
        masks
            .get(&key)
            .and_then(|(_, image)| image.clone().map(|image| (image, bounds)))
    })
    .flatten()
}

const MASK: [Prop; 8] = [
    Prop::MASK_IMAGE,
    Prop::MASK_SIZE,
    Prop::MASK_POSITION,
    Prop::MASK_REPEAT,
    Prop::MASK_COMPOSITE,
    Prop::MASK_ORIGIN,
    Prop::MASK_CLIP,
    Prop::MASK_MODE,
];

// What the boxes of mask layers read.
const BOXES: [Prop; 20] = [
    Prop::PADDING_TOP,
    Prop::PADDING_RIGHT,
    Prop::PADDING_BOTTOM,
    Prop::PADDING_LEFT,
    Prop::BORDER_TOP_WIDTH,
    Prop::BORDER_RIGHT_WIDTH,
    Prop::BORDER_BOTTOM_WIDTH,
    Prop::BORDER_LEFT_WIDTH,
    Prop::BORDER_TOP_STYLE,
    Prop::BORDER_RIGHT_STYLE,
    Prop::BORDER_BOTTOM_STYLE,
    Prop::BORDER_LEFT_STYLE,
    Prop::BORDER_TOP_LEFT_RADIUS,
    Prop::BORDER_TOP_RIGHT_RADIUS,
    Prop::BORDER_BOTTOM_RIGHT_RADIUS,
    Prop::BORDER_BOTTOM_LEFT_RADIUS,
    Prop::CORNER_TOP_LEFT_SHAPE,
    Prop::CORNER_TOP_RIGHT_SHAPE,
    Prop::CORNER_BOTTOM_RIGHT_SHAPE,
    Prop::CORNER_BOTTOM_LEFT_SHAPE,
];

/// The box that a mask layer is placed in, and the outline that clips it.
pub(crate) fn layer_boxes(
    style: &css::properties::Style,
    units: &css::value::units::Lengths,
    size: [f32; 2],
    index: usize,
) -> ([f32; 4], Vec<css::value::shape::Op>) {
    let pick = |prop| {
        let values = css::value::tokens::components(style.get(prop).unwrap_or(""), ',');
        values
            .get(index % values.len().max(1))
            .map_or("border-box", |value| value.trim())
    };
    let [top, right, bottom, left] =
        clip::insets(Some(pick(Prop::MASK_ORIGIN)), style, units, size);
    let origin = [
        left,
        top,
        (size[0] - left - right).max(0.),
        (size[1] - top - bottom).max(0.),
    ];
    // Like Chrome, `no-clip` is the border box without its radii.
    let name = pick(Prop::MASK_CLIP);
    let [top, right, bottom, left] = clip::insets(Some(name), style, units, size);
    let rect = [
        left,
        top,
        (size[0] - left - right).max(0.),
        (size[1] - top - bottom).max(0.),
    ];
    let radii = match name {
        "no-clip" => [[0.; 2]; 4],
        name => clip::edge_radii(Some(name), style, units, size),
    };
    let outline = css::value::shape::rounded(rect, radii);
    (origin, outline)
}

/// Whether the outline is the whole border box.
pub(crate) fn whole(outline: &[css::value::shape::Op], size: [f32; 2]) -> bool {
    css::value::shape::reach(outline) == Some([0., 0., size[0], size[1]])
        && !outline
            .iter()
            .any(|op| matches!(op, css::value::shape::Op::Cubic(..)))
}

pub(crate) fn mask_svg<'a>(
    style: &'a css::properties::Style,
    units: css::value::units::Lengths,
    border: [f32; 2],
    scale: f32,
    base: Option<&url::Url>,
    assets: &HashMap<String, std::path::PathBuf>,
    // The files of the `url()` layers, which the SVG names as `apps-mask:<index>`.
    files: &mut Vec<std::path::PathBuf>,
) -> Option<String> {
    let [width, height] = border;
    let get = |prop: Prop| style.get(prop).unwrap_or("");
    let images = get(Prop::MASK_IMAGE);
    if images.is_empty() || images == "none" {
        return None;
    }
    let list = |prop| css::value::tokens::components(get(prop), ',');
    let (sizes, positions, repeats) = (
        list(Prop::MASK_SIZE),
        list(Prop::MASK_POSITION),
        list(Prop::MASK_REPEAT),
    );
    let pick = |values: &[&'a str], index: usize| {
        values
            .get(index % values.len().max(1))
            .copied()
            .unwrap_or("")
    };
    let (composites, modes) = (list(Prop::MASK_COMPOSITE), list(Prop::MASK_MODE));
    // Layers go from the bottom up, and each one joins the layers below it by its `mask-composite`.
    let (mut defs, mut below) = (String::new(), None::<String>);
    let area = format!(
        "maskUnits=\"userSpaceOnUse\" x=\"0\" y=\"0\" width=\"{width}\" height=\"{height}\""
    );
    for (index, image) in css::value::tokens::components(images, ',')
        .into_iter()
        .enumerate()
        .rev()
    {
        let image = css::value::image::shown(image.trim(), scale).0;
        let image = &*image;
        let [size, position, repeat] = [
            (pick(&sizes, index), "auto"),
            (pick(&positions, index), "0% 0%"),
            (pick(&repeats, index), "repeat"),
        ]
        .map(|(value, initial)| if value.is_empty() { initial } else { value });
        let (origin, outline) = layer_boxes(style, &units, border, index);
        // A layer without an image is transparent, and still joins the layers below it.
        let layer = if let Some(source) =
            image.strip_prefix("url(").and_then(|v| v.strip_suffix(')'))
            && let Some(path) = super::super::raster::image::url_file(source, base, assets)
        {
            let aspect = match size {
                "cover" => "xMidYMid slice",
                "contain" => "xMidYMid meet",
                _ => "none",
            };
            let ([[tw, th], [x, y]], steps) = super::super::effects::gradient::tile_repeat(
                size, position, repeat, &units, origin[2], origin[3], None,
            );
            files.push(path);
            super::super::effects::gradient::tiled(
                steps,
                [tw, th],
                [x + origin[0], y + origin[1]],
                [width, height],
                &format!(
                    "<image href=\"apps-mask:{}\" width=\"{tw}\" height=\"{th}\" preserveAspectRatio=\"{aspect}\"/>",
                    files.len() - 1
                ),
            )
        } else {
            let svg = image
                .split_once('(')
                .filter(|(kind, _)| kind.ends_with("-gradient"))
                .and_then(|(kind, body)| {
                    super::super::effects::gradient::gradient_svg(
                        kind,
                        body.strip_suffix(')')?,
                        size,
                        position,
                        repeat,
                        units,
                        width,
                        height,
                        origin,
                    )
                });
            match svg
                .as_ref()
                .and_then(|(svg, ..)| Some((svg, svg.find('>')?, svg.rfind("</svg>")?)))
            {
                Some((svg, start, end)) => svg[start + 1..end].to_owned(),
                None => String::new(),
            }
        };
        // Each layer is its own SVG document, so its `id`s must not collide.
        let layer = layer
            .replace("id=\"", &format!("id=\"m{index}"))
            .replace("url(#", &format!("url(#m{index}"));
        let layer = match pick(&modes, index).trim() == "luminance" {
            true => {
                defs.push_str(&format!(
                    "<mask id=\"apps-light{index}\" {area}>{layer}</mask>"
                ));
                format!(
                    "<g mask=\"url(#apps-light{index})\"><rect width=\"{width}\" height=\"{height}\"/></g>"
                )
            }
            false => layer,
        };
        let op = pick(&composites, index).trim();
        let boxed = !whole(&outline, border);
        // Like Chrome, a luminance layer joins as a whole.
        let over = below.is_none()
            || pick(&modes, index).trim() == "luminance"
            || matches!(op, "" | "add" | "source-over" | "destination-over");
        if boxed {
            defs.push_str(&format!(
                "<clipPath id=\"apps-box{index}\"><path d=\"{}\"/></clipPath>",
                css::value::shape::svg(&outline)
            ));
        }
        let inside = |layer: &str| format!("<g clip-path=\"url(#apps-box{index})\">{layer}</g>");
        let layer = match boxed && over {
            true => inside(&layer),
            false => layer,
        };
        let Some(under) = below.take() else {
            below = Some(layer);
            continue;
        };
        // `source-*`, `destination-*` and the rest are the `-webkit-mask-composite` names.
        // Like Chrome, a layer changes only what its clip covers.
        let kept = (boxed && !over).then(|| {
            defs.push_str(&format!(
                "<clipPath id=\"apps-rest{index}\"><path clip-rule=\"evenodd\" d=\"M0 0H{width}V{height}H0Z{}\"/></clipPath>",
                css::value::shape::svg(&outline)
            ));
            format!("<g clip-path=\"url(#apps-rest{index})\">{under}</g>")
        });
        let joined = match op {
            "clear" => String::new(),
            "copy" | "destination-atop" => layer,
            "source-atop" => under,
            "intersect" | "source-in" | "destination-in" => {
                defs.push_str(&format!(
                    "<mask id=\"apps-in{index}\" mask-type=\"alpha\" {area}>{under}</mask>"
                ));
                format!("<g mask=\"url(#apps-in{index})\">{layer}</g>")
            }
            op @ ("subtract" | "source-out" | "destination-out" | "exclude" | "xor") => {
                let (layer, under) = match op {
                    "destination-out" => (under, layer),
                    _ => (layer, under),
                };
                // An inverted alpha mask: white, with the shape drawn black over it.
                let out = |id: &str, shape: &str| {
                    format!(
                        "<mask id=\"{id}\" {area}><rect width=\"{width}\" height=\"{height}\" fill=\"#fff\"/><g filter=\"url(#apps-black)\">{shape}</g></mask>"
                    )
                };
                defs.push_str(&out(&format!("apps-out{index}"), &under));
                let mut joined = format!("<g mask=\"url(#apps-out{index})\">{layer}</g>");
                if matches!(op, "exclude" | "xor") {
                    defs.push_str(&out(&format!("apps-back{index}"), &layer));
                    joined.push_str(&format!("<g mask=\"url(#apps-back{index})\">{under}</g>"));
                }
                joined
            }
            _ => under + &layer,
        };
        below = Some(match kept {
            Some(kept) => inside(&joined) + &kept,
            None => joined,
        });
    }
    let content = below?;
    if defs.contains("apps-black") {
        defs.push_str(&format!("<filter id=\"apps-black\" filterUnits=\"userSpaceOnUse\" x=\"0\" y=\"0\" width=\"{width}\" height=\"{height}\"><feColorMatrix values=\"0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0\"/></filter>"));
    }
    Some(if defs.is_empty() {
        content
    } else {
        format!("<defs>{defs}</defs>{content}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_masks_draw_as_their_svg() {
        let units = css::value::units::Lengths::new(16., 800., 600.);
        for (image, composite) in [
            (
                "linear-gradient(#000 0 0), linear-gradient(#000 0 0)",
                "exclude",
            ),
            (
                "linear-gradient(to right, black 30%, transparent 70%), linear-gradient(to top, transparent, black)",
                "subtract",
            ),
            (
                "linear-gradient(to right, black 30%, transparent 70%), linear-gradient(to top, transparent, black)",
                "intersect",
            ),
            (
                "linear-gradient(to left, black, transparent), linear-gradient(to bottom, transparent 20%, black)",
                "add",
            ),
            (
                "linear-gradient(to left, black, transparent), linear-gradient(black 40%, transparent), linear-gradient(to right, transparent 10%, black)",
                "subtract, add",
            ),
        ] {
            let style: css::properties::Style =
                [("mask-image", image), ("mask-composite", composite)]
                    .into_iter()
                    .map(|(key, value)| (Prop::new(key), value.into()))
                    .collect();
            let fast =
                crate::render::effects::gradient::axis_mask(&style, &units, [100., 50.], [100, 50])
                    .unwrap();
            let layers = mask_svg(
                &style,
                units,
                [100., 50.],
                1.,
                None,
                &Default::default(),
                &mut Vec::new(),
            )
            .unwrap();
            let source = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"50\">{layers}</svg>"
            );
            let tree = crate::render::raster::svg::svg_tree(&source, false).unwrap();
            let slow = crate::render::raster::svg::svg_pixels(&tree, 100, 50, None).unwrap();
            let fast = fast.unwrap_or_else(|| vec![255; 100 * 50 * 4]);
            let worst = fast
                .as_chunks::<4>()
                .0
                .iter()
                .zip(slow.pixels())
                .map(|(fast, slow)| fast[3].abs_diff(slow.alpha()))
                .max();
            assert!(worst <= Some(8), "{image} {composite}: {worst:?}");
        }
    }
}
