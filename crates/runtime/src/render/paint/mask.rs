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
    shapes: Option<&str>,
    bounds: Rect<Pixels>,
    window: &mut Window,
) -> Option<(std::sync::Arc<renderer::RenderImage>, Rect<Pixels>)> {
    let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
    let base = dom.base.clone();
    let images = [Prop::MASK_IMAGE, Prop::WEBKIT_MASK_IMAGE]
        .into_iter()
        .find_map(|prop| style.get(prop))
        .filter(|value| !value.is_empty() && *value != "none");
    (shapes.is_some() || images.is_some()).then(|| {
        let mut inputs = String::new();
        for part in [
            Prop::MASK_IMAGE,
            Prop::WEBKIT_MASK_IMAGE,
            Prop::MASK_SIZE,
            Prop::WEBKIT_MASK_SIZE,
            Prop::MASK_POSITION,
            Prop::WEBKIT_MASK_POSITION,
            Prop::MASK_REPEAT,
            Prop::WEBKIT_MASK_REPEAT,
            Prop::MASK_COMPOSITE,
        ]
        .map(|prop| style.get(prop).unwrap_or(""))
        .into_iter()
        .chain(shapes)
        {
            inputs.push_str(part);
            inputs.push('\0');
        }
        super::super::effects::gradient::push_bits(&mut inputs, size, &units);
        let [width, height] = size;
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
            let image = if shapes.is_none()
                && let Some(bytes) = super::super::effects::gradient::axis_mask(style, &units, size, raster)
            {
                bytes.map(|bytes| {
                    std::sync::Arc::new(renderer::RenderImage::new(raster[0], raster[1], bytes))
                })
            } else {
                let mut files = Vec::new();
                let layers = mask_svg(style, units, size, base.as_ref(), assets, &mut files);
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
                let source = match (shapes, layers) {
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
            if let Some((_, Some(old))) = masks.insert(key, (inputs, image)) {
                window.release_image(old);
            }
        }
        masks
            .get(&key)
            .and_then(|(_, image)| image.clone().map(|image| (image, bounds)))
    })
    .flatten()
}

pub(crate) fn mask_svg<'a>(
    style: &'a css::properties::Style,
    units: css::value::units::Lengths,
    [width, height]: [f32; 2],
    base: Option<&url::Url>,
    assets: &HashMap<String, std::path::PathBuf>,
    // The files of the `url()` layers, which the SVG names as `apps-mask:<index>`.
    files: &mut Vec<std::path::PathBuf>,
) -> Option<String> {
    let get =
        |[prop, webkit]: [Prop; 2]| style.get(prop).or_else(|| style.get(webkit)).unwrap_or("");
    let images = get([Prop::MASK_IMAGE, Prop::WEBKIT_MASK_IMAGE]);
    if images.is_empty() || images == "none" {
        return None;
    }
    let list = |props| css::value::tokens::components(get(props), ',');
    let (sizes, positions, repeats) = (
        list([Prop::MASK_SIZE, Prop::WEBKIT_MASK_SIZE]),
        list([Prop::MASK_POSITION, Prop::WEBKIT_MASK_POSITION]),
        list([Prop::MASK_REPEAT, Prop::WEBKIT_MASK_REPEAT]),
    );
    let pick = |values: &[&'a str], index: usize| {
        values
            .get(index % values.len().max(1))
            .copied()
            .unwrap_or("")
    };
    let composites =
        css::value::tokens::components(style.get(Prop::MASK_COMPOSITE).unwrap_or(""), ',');
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
        let image = image.trim();
        let [size, position, repeat] = [
            (pick(&sizes, index), "auto"),
            (pick(&positions, index), "0% 0%"),
            (pick(&repeats, index), "repeat"),
        ]
        .map(|(value, initial)| if value.is_empty() { initial } else { value });
        let layer = if let Some(source) =
            image.strip_prefix("url(").and_then(|v| v.strip_suffix(')'))
        {
            let Some(path) = super::super::raster::image::url_file(source, base, assets) else {
                continue;
            };
            let aspect = match size {
                "cover" => "xMidYMid slice",
                "contain" => "xMidYMid meet",
                _ => "none",
            };
            let ([[tw, th], at], steps) = super::super::effects::gradient::tile_repeat(
                size, position, repeat, &units, width, height, None,
            );
            files.push(path);
            super::super::effects::gradient::tiled(
                steps,
                [tw, th],
                at,
                [width, height],
                &format!(
                    "<image href=\"apps-mask:{}\" width=\"{tw}\" height=\"{th}\" preserveAspectRatio=\"{aspect}\"/>",
                    files.len() - 1
                ),
            )
        } else {
            let Some((kind, body)) = image
                .split_once('(')
                .filter(|(kind, _)| kind.ends_with("-gradient"))
                .and_then(|(kind, body)| Some((kind, body.strip_suffix(')')?)))
            else {
                continue;
            };
            let Some((svg, _)) = super::super::effects::gradient::gradient_svg(
                kind,
                body,
                size,
                position,
                repeat,
                units,
                width,
                height,
                [0., 0., width, height],
            ) else {
                continue;
            };
            let (Some(start), Some(end)) = (svg.find('>'), svg.rfind("</svg>")) else {
                continue;
            };
            svg[start + 1..end].to_owned()
        };
        // Each layer is its own SVG document, so its `id`s must not collide.
        let layer = layer
            .replace("id=\"", &format!("id=\"m{index}"))
            .replace("url(#", &format!("url(#m{index}"));
        let Some(under) = below.take() else {
            below = Some(layer);
            continue;
        };
        // `source-*` and `xor` are the `-webkit-mask-composite` names.
        let joined = match pick(&composites, index).trim() {
            "intersect" | "source-in" => {
                defs.push_str(&format!(
                    "<mask id=\"apps-in{index}\" mask-type=\"alpha\" {area}>{under}</mask>"
                ));
                format!("<g mask=\"url(#apps-in{index})\">{layer}</g>")
            }
            op @ ("subtract" | "source-out" | "exclude" | "xor") => {
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
        below = Some(joined);
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
