use super::paint::Effected;
use crate::style;
use crate::style::Prop;
use renderer::{AnyElement, div, prelude::*, px, rgba};
use std::{
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
};

type Backgrounds = HashMap<(usize, usize), (String, Arc<renderer::RenderImage>)>;

/// The bitmap of a file once a worker decoded it, empty when the file is no bitmap.
pub(super) type Decoding = Arc<Mutex<Option<renderer::RenderImage>>>;

/// Draws the window again once `decoding` is done, checking on each frame.
fn landed(decoding: Decoding) -> impl FnOnce(&mut renderer::Window, &mut renderer::App) {
    move |window, _| {
        if decoding
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
        {
            window.refresh();
        } else {
            window.on_next_frame(landed(decoding));
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn css_background(
    style: &style::Style,
    units: style::Lengths,
    id: usize,
    slot: usize,
    radius: renderer::Radii<renderer::Length>,
    view: Rc<super::PageState>,
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
                                // WARNING: files decode on workers like `<img>`. A decoded bitmap is taken
                                // here, where a layer holds it at once, since `retire` drops unheld bitmaps.
                                let bitmap = match decoded(backgrounds) {
                                    Some(bitmap) => bitmap,
                                    None => {
                                        let mut decoding = view.decoding.borrow_mut();
                                        let done = decoding.get(&file).and_then(|(_, slot)| {
                                            slot.lock()
                                                .unwrap_or_else(PoisonError::into_inner)
                                                .take()
                                        });
                                        let Some(image) = done else {
                                            if !decoding.contains_key(&file) {
                                                let slot = Decoding::default();
                                                decoding.insert(file.clone(), (id, slot.clone()));
                                                let (path, done) = (path.clone(), slot.clone());
                                                type Job = Box<dyn FnOnce() + Send>;
                                                static JOBS: Mutex<(VecDeque<Job>, usize)> =
                                                    Mutex::new((VecDeque::new(), 0));
                                                let mut jobs = JOBS
                                                    .lock()
                                                    .unwrap_or_else(PoisonError::into_inner);
                                                jobs.0.push_back(Box::new(move || {
                                                    let image = (|| -> anyhow::Result<_> {
                                                        anyhow::ensure!(
                                                            std::fs::metadata(&path)?.len()
                                                                <= 64 * 1024 * 1024,
                                                            "Image is too large"
                                                        );
                                                        super::image::decode(&std::fs::read(&path)?)
                                                    })(
                                                    )
                                                    .unwrap_or_else(|_| {
                                                        renderer::RenderImage::new(0, 0, Vec::new())
                                                    });
                                                    *done
                                                        .lock()
                                                        .unwrap_or_else(PoisonError::into_inner) =
                                                        Some(image);
                                                }));
                                                // WARNING: at most two files decode at once to bound their memory, and idle workers exit.
                                                if jobs.1 < 2
                                                    && std::thread::Builder::new()
                                                        .name("apps-image".into())
                                                        .spawn(|| {
                                                            while let Some(job) = {
                                                                let mut jobs =
                                                                    JOBS.lock().unwrap_or_else(
                                                                        PoisonError::into_inner,
                                                                    );
                                                                let job = jobs.0.pop_front();
                                                                jobs.1 -=
                                                                    usize::from(job.is_none());
                                                                job
                                                            } {
                                                                job();
                                                            }
                                                        })
                                                        .is_ok()
                                                {
                                                    jobs.1 += 1;
                                                }
                                                window.on_next_frame(landed(slot));
                                            }
                                            return;
                                        };
                                        decoding.remove(&file);
                                        let image = Arc::new(image);
                                        backgrounds.insert(shared, (file.clone(), image.clone()));
                                        image
                                    }
                                };
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
        let strip = super::effects::gradient_axis(&kind, &body);
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
                    // A layer that fills the box with a gradient along one axis is drawn as a strip three pixels
                    // across. The GPU stretches the middle pixel over the box.
                    // WARNING: the outer pixels stay outside the box, so filtering never reads other atlas tiles.
                    let scale = window.scale_factor();
                    let axis = strip.filter(|_| {
                        matches!(size.as_str(), "" | "auto" | "auto auto")
                            && style::components(&position, ' ').iter().all(|token| {
                                token.ends_with('%')
                                    || matches!(
                                        *token,
                                        "left" | "right" | "top" | "bottom" | "center"
                                    )
                            })
                    });
                    let (width, height) = match axis {
                        Some(1) => (3., height),
                        Some(_) => (width, 3.),
                        None => (width, height),
                    };
                    let rw = (width * scale).ceil().clamp(1., 4096.) as u32;
                    let rh = (height * scale).ceil().clamp(1., 4096.) as u32;
                    let [rw, rh] = match axis {
                        Some(1) => [3, rh],
                        Some(_) => [rw, 3],
                        None => [rw, rh],
                    };
                    // WARNING: building the gradient's SVG costs more than drawing the box, so the bitmap is
                    // kept by its exact inputs: the strings, then the box size and units as hex bits.
                    let mut inputs = String::with_capacity(
                        kind.len() + body.len() + size.len() + position.len() + repeat.len() + 168,
                    );
                    for part in [&kind, &body, &size, &position, &repeat] {
                        inputs.push_str(part);
                        inputs.push('\0');
                    }
                    super::effects::push_bits(&mut inputs, [width, height], &units);
                    let view = &*view;
                    let mut host = view.host.borrow_mut();
                    let backgrounds = &mut host.dom.entry(view.id).or_default().backgrounds;
                    let key = (id, slot * 64 + index);
                    if backgrounds.get(&key).is_none_or(|(old, image)| {
                        old != &inputs
                            || u32::from(image.size().width) != rw
                            || u32::from(image.size().height) != rh
                    }) {
                        let image = super::effects::gradient_svg(
                            &kind, &body, &size, &position, &repeat, units, width, height,
                        )
                        .and_then(|source| super::canvas::svg(&source, rw, rh, None).ok());
                        let old = match image {
                            Some(image) => backgrounds.insert(key, (inputs, image)),
                            None => backgrounds.remove(&key),
                        };
                        if let Some((_, old)) = old
                            && Arc::strong_count(&old) == 1
                        {
                            window.drop_image(old);
                        }
                    }
                    if let Some((_, image)) = backgrounds.get(&key) {
                        let (origin, size) = (bounds.origin, bounds.size);
                        let target = match axis {
                            Some(1) => renderer::Bounds::new(
                                renderer::point(origin.x - size.width, origin.y),
                                renderer::size(size.width * 3., size.height),
                            ),
                            Some(_) => renderer::Bounds::new(
                                renderer::point(origin.x, origin.y - size.height),
                                renderer::size(size.width, size.height * 3.),
                            ),
                            None => bounds,
                        };
                        window.paint_image(
                            bounds,
                            target,
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
            backdrop: None,
        });
    }
    vec![
        Effected {
            effect: renderer::PaintEffect::default(),
            child: group.into_any_element(),
            backdrop: None,
        }
        .into_any_element(),
    ]
}
