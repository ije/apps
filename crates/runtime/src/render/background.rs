use super::paint::Effected;
use crate::style;
use crate::style::Prop;
use renderer::{Child, px};
use std::{
    collections::HashMap,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
};

type Bitmaps = HashMap<String, Arc<renderer::RenderImage>>;

pub(super) type Decoding = Arc<Mutex<Option<renderer::RenderImage>>>;

fn redraw_when_decoded(
    decoding: Decoding,
) -> impl FnOnce(&mut renderer::Window, &mut renderer::App) {
    move |window, _| {
        // Retain the slot in this closure: another box can take the image before this one paints.
        if Arc::strong_count(&decoding) == 1
            || decoding
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some()
        {
            window.refresh();
        } else {
            window.next_frame(redraw_when_decoded(decoding));
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
            if let Some(path) = super::image::url_file(source, base, assets) {
                let fit = match size.as_str() {
                    "cover" => renderer::ObjectFit::Cover,
                    "contain" => renderer::ObjectFit::Contain,
                    "" | "auto" | "auto auto" => renderer::ObjectFit::None,
                    _ => renderer::ObjectFit::Fill,
                };
                let file = path.to_string_lossy().into_owned();
                let source = format!("{size} {file}");
                let view = view.clone();
                layers.push(
                    renderer::canvas_within(
                        |_, _| (),
                        move |bounds, _, window| {
                            let [width, height] =
                                [f32::from(bounds.size.width), f32::from(bounds.size.height)];
                            if width <= 0. || height <= 0. {
                                return;
                            }
                            let view = &*view;
                            let mut host = view.host.borrow_mut();
                            let dom = host.dom.entry(view.id).or_default();
                            let (backgrounds, bitmaps) = (&mut dom.backgrounds, &mut dom.bitmaps);
                            let scale = window.scale_factor();
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
                            // Group backgrounds above 4096 tiles into bitmaps of at least 256 device pixels
                            // per repeating side to bound draw calls.
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
                            let fixed = |bitmaps: &Bitmaps, image: &Arc<renderer::RenderImage>| {
                                bitmaps
                                    .get(&file)
                                    .is_some_and(|bitmap| Arc::ptr_eq(bitmap, image))
                            };
                            if backgrounds.get(&key).is_none_or(|(old, image)| {
                                *old != source
                                    || !fixed(bitmaps, image)
                                        && [image.size().width, image.size().height]
                                            .map(|side| side as u32)
                                            != cell
                            }) {
                                // Take a decoded bitmap only where a layer holds it at once, since `retire`
                                // drops unheld bitmaps.
                                let bitmap = match bitmaps.get(&file).cloned() {
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
                                                super::image::decode_later(Box::new(move || {
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
                                                window.next_frame(redraw_when_decoded(slot));
                                            }
                                            return;
                                        };
                                        decoding.remove(&file);
                                        let image = Arc::new(image);
                                        bitmaps.insert(file.clone(), image.clone());
                                        image
                                    }
                                };
                                let image = if (bitmap.size().width as u32) > 0 {
                                    bitmap
                                } else {
                                    let svg = std::fs::read_to_string(&path)
                                        .map_err(anyhow::Error::from)
                                        .and_then(|text| {
                                            let tree = super::canvas::svg_tree(&text, false)?;
                                            let natural = tree.size();
                                            let fitted = fit.place(
                                                renderer::Rect::new(
                                                    renderer::point(px(0.), px(0.)),
                                                    renderer::size(px(width), px(height)),
                                                ),
                                                renderer::size(
                                                    natural.width().round().max(1.) as u32 as i32,
                                                    natural.height().round().max(1.) as u32 as i32,
                                                ),
                                            );
                                            super::canvas::render_svg(
                                                &tree,
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
                                        super::canvas::render_image(&pixels, Vec::new())
                                    }
                                    None => image,
                                };
                                if let Some((_, old)) =
                                    backgrounds.insert(key, (source.clone(), image))
                                    && Arc::strong_count(&old) == 1
                                {
                                    window.release_image(old);
                                }
                            }
                            let Some((_, image)) = backgrounds.get(&key) else {
                                return;
                            };
                            let natural = image.size();
                            if (natural.width as u32) == 0 || (natural.height as u32) == 0 {
                                return;
                            }
                            if tiled {
                                // Calculate visible tile indices directly. Iteration from distant origins or
                                // across tiny tiles can run indefinitely.
                                let visible = window.content_mask().bounds.intersect(bounds);
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
                                        window.draw_image(
                                            bounds,
                                            renderer::Rect::new(
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
                            let target = if fixed(bitmaps, image) {
                                fit.place(bounds, natural)
                            } else {
                                bounds
                            };
                            window.draw_image(
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
            renderer::canvas_within(
                |_, _| (),
                move |bounds, _, window| {
                    let [width, height] =
                        [f32::from(bounds.size.width), f32::from(bounds.size.height)];
                    if width <= 0. || height <= 0. {
                        return;
                    }
                    // Keep the gradient strip's outer pixels outside the box so filtering cannot sample
                    // neighboring atlas tiles.
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
                        [image.size().width, image.size().height].map(|side| side as u32)
                            == [rw, rh]
                    };
                    let entry = dom.backgrounds.get(&key);
                    if entry.is_none_or(|(old, image)| old != &inputs || !fits(image)) {
                        // Cache only on new boxes, or animated gradients retain old frames.
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
                            .and_then(|source| super::canvas::svg(&source, rw, rh).ok())?;
                            const LIMIT: u32 = 8 << 20;
                            if new && rw * rh * 4 <= LIMIT / 4 {
                                dom.gradients.push((inputs.clone(), image.clone()));
                                let bytes = |image: &renderer::RenderImage| {
                                    image.size().width as u32 * (image.size().height as u32) * 4
                                };
                                let mut total: u32 =
                                    dom.gradients.iter().map(|(_, image)| bytes(image)).sum();
                                while total > LIMIT {
                                    let (_, old) = dom.gradients.remove(0);
                                    total -= bytes(&old);
                                    if Arc::strong_count(&old) == 1 {
                                        window.release_image(old);
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
                            window.release_image(old);
                        }
                    }
                    if let Some((_, image)) = dom.backgrounds.get(&key)
                        && let Some([x, y]) = tiled
                    {
                        // Tiles snap to device pixels, so their edges never sample other atlas tiles.
                        let visible = window.content_mask().bounds.intersect(bounds);
                        let start = |edge, at: f32, side: f32, cell: f32, low| {
                            let at = f32::from(edge) + at - if at > 0. { side } else { 0. };
                            let at = (at * scale).round() / scale;
                            px(at + cell * ((f32::from(low) - at) / cell).floor().max(0.))
                        };
                        let mut top = start(bounds.top(), y, th, ch, visible.top());
                        while top < visible.bottom() {
                            let mut left = start(bounds.left(), x, tw, cw, visible.left());
                            while left < visible.right() {
                                let target = renderer::Rect::new(
                                    renderer::point(left, top),
                                    renderer::size(px(cw), px(ch)),
                                );
                                window.draw_image(bounds, target, radii, image.clone(), false);
                                left += px(cw);
                            }
                            top += px(ch);
                        }
                    } else if let Some((_, image)) = dom.backgrounds.get(&key) {
                        let (origin, size) = (bounds.origin, bounds.size);
                        let target = match axis {
                            Some(1) => renderer::Rect::new(
                                renderer::point(origin.x - size.width, origin.y),
                                renderer::size(size.width * 3., size.height),
                            ),
                            Some(_) => renderer::Rect::new(
                                renderer::point(origin.x, origin.y - size.height),
                                renderer::size(size.width, size.height * 3.),
                            ),
                            None => bounds,
                        };
                        window.draw_image(
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
            .bg(crate::style::rgba((ink, alpha)));
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
