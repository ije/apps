use super::paint::Effected;
use crate::style;
use crate::style::Prop;
use renderer::{AnyElement, div, prelude::*, px, rgba};
use std::{collections::HashMap, rc::Rc, sync::Arc};

type Backgrounds = HashMap<(usize, usize), (String, Arc<renderer::RenderImage>)>;

#[allow(clippy::too_many_arguments)]
pub(super) fn css_background(
    style: &style::Style,
    units: style::Lengths,
    id: usize,
    slot: usize,
    radius: renderer::Radii<renderer::Length>,
    view: Rc<super::ViewData>,
    base: Option<&url::Url>,
    assets: &HashMap<String, std::path::PathBuf>,
) -> Vec<AnyElement> {
    let list = |prop| {
        style::components(style.get(prop).unwrap_or(""), ',')
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (sizes, positions, repeats) = (
        list(Prop::BACKGROUND_SIZE),
        list(Prop::BACKGROUND_POSITION),
        list(Prop::BACKGROUND_REPEAT),
    );
    let pick = |values: &[String], index: usize| {
        values
            .get(index % values.len().max(1))
            .cloned()
            .unwrap_or_default()
    };
    let mut layers = Vec::new();
    let mut indexes = Vec::new();
    for (index, image) in list(Prop::BACKGROUND_IMAGE).into_iter().enumerate().rev() {
        let (size, position, repeat) = (
            pick(&sizes, index),
            pick(&positions, index),
            pick(&repeats, index),
        );
        if let Some(source) = image.strip_prefix("url(").and_then(|v| v.strip_suffix(')')) {
            if let Some(path) = base
                .and_then(|base| {
                    Some((
                        base,
                        base.join(source.trim().trim_matches(['"', '\''])).ok()?,
                    ))
                })
                .and_then(|(base, url)| match url.scheme() {
                    "file" if base.scheme() == "file" => url.to_file_path().ok(),
                    "apps" => assets.get(url.path().trim_start_matches('/')).cloned(),
                    _ => None,
                })
            {
                let fit = match size.as_str() {
                    "cover" => renderer::ObjectFit::Cover,
                    "contain" => renderer::ObjectFit::Contain,
                    "" | "auto" | "auto auto" => renderer::ObjectFit::None,
                    _ => renderer::ObjectFit::Fill,
                };
                let file = path.to_string_lossy().into_owned();
                let source = format!("{size} {file}");
                let shared = (usize::MAX, {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::hash::DefaultHasher::new();
                    file.hash(&mut hasher);
                    hasher.finish() as usize
                });
                let view = view.clone();
                layers.push(
                    renderer::canvas(
                        |_, _| (),
                        move |bounds, _, window| {
                            let [width, height] =
                                [f32::from(bounds.size.width), f32::from(bounds.size.height)];
                            if width <= 0. || height <= 0. {
                                return;
                            }
                            let view = &*view;
                            let mut host = view.host.borrow_mut();
                            let backgrounds = &mut host.dom.entry(view.id).or_default().backgrounds;
                            let scale = window.scale_factor();
                            let raster = [width, height]
                                .map(|extent| (extent * scale).ceil().clamp(1., 4096.) as u32);
                            let key = (id, slot * 64 + index);
                            let decoded = |backgrounds: &Backgrounds| {
                                backgrounds
                                    .get(&shared)
                                    .filter(|(old, _)| *old == file)
                                    .map(|(_, image)| image.clone())
                            };
                            let fixed =
                                |backgrounds: &Backgrounds, image: &Arc<renderer::RenderImage>| {
                                    decoded(backgrounds)
                                        .is_some_and(|bitmap| Arc::ptr_eq(&bitmap, image))
                                };
                            // WARNING: bitmaps are decoded once per file and shared; SVGs are drawn at the box size.
                            if backgrounds.get(&key).is_none_or(|(old, image)| {
                                *old != source
                                    || !fixed(backgrounds, image)
                                        && [image.size().width, image.size().height].map(u32::from)
                                            != raster
                            }) {
                                let bitmap = decoded(backgrounds).unwrap_or_else(|| {
                                    let image = Arc::new(
                                        (|| -> anyhow::Result<_> {
                                            anyhow::ensure!(
                                                std::fs::metadata(&path)?.len() <= 64 * 1024 * 1024,
                                                "Image is too large"
                                            );
                                            super::image::decode(&std::fs::read(&path)?)
                                        })()
                                        .unwrap_or_else(|_| {
                                            renderer::RenderImage::new(0, 0, Vec::new())
                                        }),
                                    );
                                    backgrounds.insert(shared, (file.clone(), image.clone()));
                                    image
                                });
                                let image = if u32::from(bitmap.size().width) > 0 {
                                    bitmap
                                } else {
                                    let svg = std::fs::read_to_string(&path)
                                        .map_err(anyhow::Error::from)
                                        .and_then(|text| {
                                            let natural = resvg::usvg::Tree::from_str(
                                                &text,
                                                &Default::default(),
                                            )?
                                            .size();
                                            let fitted = fit.get_bounds(
                                                renderer::Bounds::new(
                                                    renderer::point(px(0.), px(0.)),
                                                    renderer::size(px(width), px(height)),
                                                ),
                                                renderer::size(
                                                    (natural.width().round().max(1.) as u32).into(),
                                                    (natural.height().round().max(1.) as u32)
                                                        .into(),
                                                ),
                                            );
                                            super::canvas::svg(
                                                &text,
                                                raster[0],
                                                raster[1],
                                                Some((
                                                    [
                                                        f32::from(fitted.size.width) * scale,
                                                        f32::from(fitted.size.height) * scale,
                                                    ],
                                                    [
                                                        1.,
                                                        0.,
                                                        0.,
                                                        1.,
                                                        f32::from(fitted.origin.x) * scale,
                                                        f32::from(fitted.origin.y) * scale,
                                                    ],
                                                )),
                                            )
                                        });
                                    svg.unwrap_or_else(|error| {
                                        eprintln!("apps: background image {file}: {error}");
                                        bitmap
                                    })
                                };
                                if let Some((_, old)) =
                                    backgrounds.insert(key, (source.clone(), image))
                                    && Arc::strong_count(&old) == 1
                                {
                                    window.drop_image(old);
                                }
                            }
                            let Some((_, image)) = backgrounds.get(&key) else {
                                return;
                            };
                            let natural = image.size();
                            if u32::from(natural.width) == 0 || u32::from(natural.height) == 0 {
                                return;
                            }
                            let target = if fixed(backgrounds, image) {
                                fit.get_bounds(bounds, natural)
                            } else {
                                bounds
                            };
                            window.paint_image(
                                bounds,
                                target,
                                renderer::Corners::default(),
                                image.clone(),
                                false,
                            );
                        },
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .into_any_element(),
                );
                indexes.push(index);
            }
            continue;
        }
        let Some((kind, body)) = image
            .split_once('(')
            .filter(|(kind, _)| kind.ends_with("-gradient"))
            .and_then(|(kind, body)| Some((kind.to_owned(), body.strip_suffix(')')?.to_owned())))
        else {
            continue;
        };
        let view = view.clone();
        layers.push(
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| {
                    let [width, height] =
                        [f32::from(bounds.size.width), f32::from(bounds.size.height)];
                    if width <= 0. || height <= 0. {
                        return;
                    }
                    let Some(source) = super::effects::gradient_svg(
                        &kind, &body, &size, &position, &repeat, units, width, height,
                    ) else {
                        return;
                    };
                    let view = &*view;
                    let mut host = view.host.borrow_mut();
                    let backgrounds = &mut host.dom.entry(view.id).or_default().backgrounds;
                    let scale = window.scale_factor();
                    let rw = (width * scale).ceil().clamp(1., 4096.) as u32;
                    let rh = (height * scale).ceil().clamp(1., 4096.) as u32;
                    let key = (id, slot * 64 + index);
                    if backgrounds.get(&key).is_none_or(|(old, image)| {
                        old != &source
                            || u32::from(image.size().width) != rw
                            || u32::from(image.size().height) != rh
                    }) && let Ok(image) = super::canvas::svg(&source, rw, rh, None)
                        && let Some((_, old)) = backgrounds.insert(key, (source, image))
                        && Arc::strong_count(&old) == 1
                    {
                        window.drop_image(old);
                    }
                    if let Some((_, image)) = backgrounds.get(&key) {
                        window.paint_image(
                            bounds,
                            bounds,
                            radius.resolve(bounds.size),
                            image.clone(),
                            false,
                        );
                    }
                },
            )
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .into_any_element(),
        );
        indexes.push(index);
    }
    let modes = list(Prop::BACKGROUND_BLEND_MODE);
    if modes
        .iter()
        .all(|mode| matches!(mode.trim(), "" | "normal"))
    {
        return layers;
    }
    // Blended layers mix with the layers and color below them, inside an isolated group.
    let mut group = div().absolute().top_0().left_0().size_full();
    if let Some((ink, alpha)) = style.get(Prop::BACKGROUND_COLOR).and_then(style::color) {
        let mut color = div()
            .absolute()
            .size_full()
            .bg(rgba(ink << 8 | alpha as u32));
        color.style().corner_radii = radius;
        group = group.child(color);
    }
    for (layer, index) in layers.into_iter().zip(indexes) {
        let blend_mode = super::effects::blend_mode(&pick(&modes, index));
        group = group.child(Effected {
            effect: renderer::PaintEffect {
                blend_mode,
                ..Default::default()
            },
            child: layer,
        });
    }
    vec![
        Effected {
            effect: renderer::PaintEffect::default(),
            child: group.into_any_element(),
        }
        .into_any_element(),
    ]
}
