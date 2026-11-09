use super::paint::Effected;
use crate::style;
use crate::style::Prop;
use renderer::{Child, px, rgba};
use std::{
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
};

type Backgrounds = HashMap<(usize, usize), (String, Arc<renderer::RenderImage>)>;

pub(super) type Decoding = Arc<Mutex<Option<renderer::RenderImage>>>;

fn redraw_when_decoded(
    decoding: Decoding,
) -> impl FnOnce(&mut renderer::Window, &mut renderer::App) {
    move |window, _| {
        // WARNING: a later box using the same file can take the image in the frame that started the job. Then
        // only this closure holds the slot, and the first box still has to paint.
        if Arc::strong_count(&decoding) == 1
            || decoding
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some()
        {
            window.refresh();
        } else {
            window.on_next_frame(redraw_when_decoded(decoding));
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
) -> Vec<Child> {
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
            let source = source.trim().trim_matches(['"', '\'']);
            if let Some(path) = super::image::data_file(source).or_else(|| {
                base.and_then(|base| Some((base, base.join(source).ok()?)))
                    .and_then(|(base, url)| match url.scheme() {
                        "file" if base.scheme() == "file" => url.to_file_path().ok(),
                        "apps" => assets.get(url.path().trim_start_matches('/')).cloned(),
                        _ => None,
                    })
            }) {
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
                            // WARNING: a layer with a set size is one tile, repeated when it paints.
                            let [[tile_width, tile_height], [tile_x, tile_y]] =
                                super::effects::tile(&size, &position, &units, width, height);
                            let tiled = !matches!(
                                size.as_str(),
                                "" | "auto" | "auto auto" | "cover" | "contain"
                            );
                            let (across, down) = (
                                !matches!(repeat.trim(), "no-repeat" | "repeat-y"),
                                !matches!(repeat.trim(), "no-repeat" | "repeat-x"),
                            );
                            // WARNING: small tiles on a big box would paint millions of images, so past 4096 tiles
                            // a layer paints a bitmap of several tiles, at least 256 device pixels per repeating side.
                            let count = |repeats: bool, extent: f32, side: f32| {
                                if repeats { (extent / side).ceil() } else { 1. }
                            };
                            let many = tiled
                                && count(across, width, tile_width)
                                    * count(down, height, tile_height)
                                    > 4096.;
                            let group = |repeats: bool, side: f32| {
                                if many && repeats {
                                    (256. / (side * scale)).ceil().max(1.)
                                } else {
                                    1.
                                }
                            };
                            let [nx, ny] = [group(across, tile_width), group(down, tile_height)];
                            let (width, height) = if tiled {
                                (tile_width, tile_height)
                            } else {
                                (width, height)
                            };
                            let limit = super::canvas::RASTER_MAX as f32;
                            let raster = [width, height]
                                .map(|extent| (extent * scale).ceil().clamp(1., limit) as u32);
                            let cell = [width * nx, height * ny]
                                .map(|extent| (extent * scale).ceil().clamp(1., limit) as u32);
                            let source = if nx * ny > 1. {
                                format!("{source} {nx} {ny}")
                            } else {
                                source.clone()
                            };
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
                                            != cell
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
                                                window.on_next_frame(redraw_when_decoded(slot));
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
                                            let natural =
                                                usvg::Tree::from_str(&text, &Default::default())?
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
                                let image = match super::canvas::pixmap(&image)
                                    .filter(|_| nx * ny > 1.)
                                    .zip(tiny_skia::Pixmap::new(cell[0], cell[1]))
                                {
                                    Some((tile, mut pixels)) => {
                                        use tiny_skia as skia;
                                        let [w, h] = cell.map(|side| side as f32);
                                        if let Some(rect) = skia::Rect::from_xywh(0., 0., w, h) {
                                            pixels.fill_rect(
                                                rect,
                                                &skia::Paint {
                                                    shader: skia::Pattern::new(
                                                        tile.as_ref(),
                                                        skia::SpreadMode::Repeat,
                                                        skia::FilterQuality::Bilinear,
                                                        1.,
                                                        skia::Transform::from_scale(
                                                            w / nx / tile.width() as f32,
                                                            h / ny / tile.height() as f32,
                                                        ),
                                                    ),
                                                    ..Default::default()
                                                },
                                                skia::Transform::identity(),
                                                None,
                                            );
                                        }
                                        super::canvas::render_image(&pixels)
                                    }
                                    None => image,
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
                            if tiled {
                                // WARNING: only the tiles in the visible part of the box paint, found by index,
                                // so a far position or a tiny tile never loops forever.
                                let visible = window.content_mask().bounds.intersect(&bounds);
                                let [width, height] = [width * nx, height * ny];
                                let origin = [
                                    f32::from(bounds.origin.x) + tile_x,
                                    f32::from(bounds.origin.y) + tile_y,
                                ];
                                let span = |repeats: bool, start: f32, side: f32, low, high| {
                                    if repeats {
                                        let [low, high]: [f32; 2] =
                                            [f32::from(low), f32::from(high)];
                                        ((low - start) / side).floor() as i64
                                            ..((high - start) / side).ceil() as i64
                                    } else {
                                        0..1
                                    }
                                };
                                let columns =
                                    span(across, origin[0], width, visible.left(), visible.right());
                                let rows =
                                    span(down, origin[1], height, visible.top(), visible.bottom());
                                if columns
                                    .end
                                    .saturating_sub(columns.start)
                                    .saturating_mul(rows.end.saturating_sub(rows.start))
                                    > 1 << 16
                                {
                                    return;
                                }
                                for row in rows {
                                    for column in columns.clone() {
                                        window.paint_image(
                                            bounds,
                                            renderer::Bounds::new(
                                                renderer::point(
                                                    px(origin[0] + column as f32 * width),
                                                    px(origin[1] + row as f32 * height),
                                                ),
                                                renderer::size(px(width), px(height)),
                                            ),
                                            renderer::Corners::default(),
                                            image.clone(),
                                            false,
                                        );
                                    }
                                }
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
                    .size_full(),
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
                    let whole = |value: f32| (value * scale - (value * scale).round()).abs() < 0.01;
                    let radii = radius.resolve(bounds.size);
                    let [[tw, th], [x, y]] =
                        super::effects::tile(&size, &position, &units, width, height);
                    let [cw, ch] =
                        [tw, th].map(|side| side * (256. / (side * scale)).ceil().max(1.));
                    let tiled = (axis.is_none()
                        && matches!(repeat.trim(), "" | "repeat" | "repeat repeat")
                        && radii.is_zero()
                        && [tw, th, x, y].into_iter().all(whole)
                        && cw * ch < width * height
                        && super::effects::tile(&size, "0 0", &units, cw, ch)[0] == [tw, th])
                    .then_some([x.rem_euclid(tw), y.rem_euclid(th)]);
                    let (width, height) = match axis {
                        Some(1) => (3., height),
                        Some(_) => (width, 3.),
                        None if tiled.is_some() => (cw, ch),
                        None => (width, height),
                    };
                    let limit = super::canvas::RASTER_MAX as f32;
                    let rw = (width * scale).ceil().clamp(1., limit) as u32;
                    let rh = (height * scale).ceil().clamp(1., limit) as u32;
                    let [rw, rh] = match axis {
                        Some(1) => [3, rh],
                        Some(_) => [rw, 3],
                        None => [rw, rh],
                    };
                    let position = if tiled.is_some() {
                        "0 0"
                    } else {
                        position.as_str()
                    };
                    // WARNING: building the gradient's SVG costs more than drawing the box, so the bitmap is
                    // kept by its exact inputs: the strings, then the box size and units as hex bits.
                    let mut inputs = String::with_capacity(
                        kind.len() + body.len() + size.len() + position.len() + repeat.len() + 168,
                    );
                    for part in [
                        kind.as_str(),
                        body.as_str(),
                        size.as_str(),
                        position,
                        repeat.as_str(),
                    ] {
                        inputs.push_str(part);
                        inputs.push('\0');
                    }
                    super::effects::push_bits(&mut inputs, [width, height], &units);
                    let view = &*view;
                    let mut host = view.host.borrow_mut();
                    let dom = host.dom.entry(view.id).or_default();
                    let key = (id, slot * 64 + index);
                    let fits = |image: &renderer::RenderImage| {
                        [image.size().width, image.size().height].map(u32::from) == [rw, rh]
                    };
                    let entry = dom.backgrounds.get(&key);
                    if entry.is_none_or(|(old, image)| old != &inputs || !fits(image)) {
                        // WARNING: only new boxes take and fill the cache, so a gradient that animates
                        // doesn't keep its old frames.
                        let new = entry.is_none();
                        let cached = new
                            .then(|| {
                                let at = dom
                                    .gradients
                                    .iter()
                                    .rposition(|(old, image)| old == &inputs && fits(image))?;
                                let cached = dom.gradients.remove(at);
                                dom.gradients.push(cached.clone());
                                Some(cached.1)
                            })
                            .flatten();
                        let image = cached.or_else(|| {
                            let image = super::effects::gradient_svg(
                                &kind, &body, &size, position, &repeat, units, width, height,
                            )
                            .and_then(|source| super::canvas::svg(&source, rw, rh, None).ok())?;
                            const LIMIT: u32 = 8 << 20;
                            if new && rw * rh * 4 <= LIMIT / 4 {
                                dom.gradients.push((inputs.clone(), image.clone()));
                                let bytes = |image: &renderer::RenderImage| {
                                    u32::from(image.size().width)
                                        * u32::from(image.size().height)
                                        * 4
                                };
                                let mut total: u32 =
                                    dom.gradients.iter().map(|(_, image)| bytes(image)).sum();
                                while total > LIMIT {
                                    let (_, old) = dom.gradients.remove(0);
                                    total -= bytes(&old);
                                    if Arc::strong_count(&old) == 1 {
                                        window.drop_image(old);
                                    }
                                }
                            }
                            Some(image)
                        });
                        let old = match image {
                            Some(image) => dom.backgrounds.insert(key, (inputs, image)),
                            None => dom.backgrounds.remove(&key),
                        };
                        if let Some((_, old)) = old
                            && Arc::strong_count(&old) == 1
                        {
                            window.drop_image(old);
                        }
                    }
                    if let Some((_, image)) = dom.backgrounds.get(&key)
                        && let Some([x, y]) = tiled
                    {
                        // WARNING: tiles sit on device pixels, so their edges never sample other atlas tiles.
                        // Only the visible ones paint, starting at the first whole tile that reaches it.
                        let visible = window.content_mask().bounds.intersect(&bounds);
                        let start = |edge, at: f32, side: f32, cell: f32, low| {
                            let at = f32::from(edge) + at - if at > 0. { side } else { 0. };
                            let at = (at * scale).round() / scale;
                            px(at + cell * ((f32::from(low) - at) / cell).floor().max(0.))
                        };
                        let mut top = start(bounds.top(), y, th, ch, visible.top());
                        while top < visible.bottom() {
                            let mut left = start(bounds.left(), x, tw, cw, visible.left());
                            while left < visible.right() {
                                let target = renderer::Bounds::new(
                                    renderer::point(left, top),
                                    renderer::size(px(cw), px(ch)),
                                );
                                window.paint_image(bounds, target, radii, image.clone(), false);
                                left += px(cw);
                            }
                            top += px(ch);
                        }
                    } else if let Some((_, image)) = dom.backgrounds.get(&key) {
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
            .size_full(),
        );
        indexes.push(index);
    }
    let modes = list(Prop::BACKGROUND_BLEND_MODE);
    if modes
        .iter()
        .all(|mode| matches!(mode.trim(), "" | "normal"))
    {
        return layers.into_iter().map(Into::into).collect();
    }
    let mut group = renderer::node().absolute().top_0().left_0().size_full();
    if let Some((ink, alpha)) = style.get(Prop::BACKGROUND_COLOR).and_then(style::color) {
        let mut color = renderer::node()
            .absolute()
            .size_full()
            .bg(rgba(ink << 8 | alpha as u32));
        color.style().corner_radii = radius;
        group = group.child(color);
    }
    for (layer, index) in layers.into_iter().zip(indexes) {
        let blend_mode = super::effects::blend_mode(&pick(&modes, index));
        group = group.child(layer.hooks(Effected {
            effect: renderer::PaintEffect {
                blend_mode,
                ..Default::default()
            },
            backdrop: None,
        }));
    }
    vec![
        group
            .hooks(Effected {
                effect: renderer::PaintEffect::default(),
                backdrop: None,
            })
            .into(),
    ]
}
