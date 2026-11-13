use super::paint::Effected;
use crate::style;
use crate::style::Prop;
use renderer::{Child, px};
use std::{
    collections::HashMap,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
};

type Backgrounds = HashMap<(usize, usize), (String, Arc<renderer::RenderImage>)>;
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

/// The root element, or the body when the root has no background, gives the canvas its background.
pub(super) fn canvas_source(
    doc: &crate::dom::Document,
    styles: &style::ComputedStyles,
) -> Option<usize> {
    let find = |id: usize, tag| {
        doc.nodes[id]
            .children
            .iter()
            .copied()
            .find(|&child| doc.nodes[child].tag == tag)
    };
    let html = find(0, "html")?;
    let style = &styles.values[html];
    let painted = style
        .get(Prop::BACKGROUND_COLOR)
        .and_then(style::color)
        .is_some_and(|color| color.1 > 0)
        || style
            .get(Prop::BACKGROUND_IMAGE)
            .is_some_and(|value| value != "none");
    if painted {
        Some(html)
    } else {
        find(html, "body")
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn css_background(
    style: &style::Style,
    units: style::Lengths,
    id: usize,
    slot: usize,
    radius: renderer::Radii<renderer::Length>,
    [borders, padding]: [[f32; 4]; 2],
    view: Rc<super::PageState>,
    base: Option<&url::Url>,
    assets: &HashMap<String, std::path::PathBuf>,
    // Layers of the canvas tile over the whole viewport, from the box of the root element.
    canvas: bool,
) -> Vec<Child> {
    let list = |prop| {
        style::components(style.get(prop).unwrap_or(""), ',')
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (sizes, positions, repeats, clips, origins) = (
        list(Prop::BACKGROUND_SIZE),
        list(Prop::BACKGROUND_POSITION),
        list(Prop::BACKGROUND_REPEAT),
        list(Prop::BACKGROUND_CLIP),
        list(Prop::BACKGROUND_ORIGIN),
    );
    let attachments = list(Prop::BACKGROUND_ATTACHMENT);
    let pick = |values: &[String], index: usize| {
        values
            .get(index % values.len().max(1))
            .cloned()
            .unwrap_or_default()
    };
    let pixelated = matches!(
        style.get(Prop::IMAGE_RENDERING),
        Some("pixelated" | "crisp-edges")
    );
    // Canvas layers paint outside their box.
    fn paint(
        canvas: bool,
        draw: impl Fn(renderer::Rect<renderer::Pixels>, (), &mut renderer::Window) + 'static,
    ) -> renderer::Node {
        if canvas {
            renderer::canvas(|_, _| (), draw)
        } else {
            renderer::canvas_within(|_, _| (), draw)
        }
    }
    let mut layers = Vec::new();
    let mut indexes = Vec::new();
    for (index, image) in list(Prop::BACKGROUND_IMAGE).into_iter().enumerate().rev() {
        let (size, position, repeat) = (
            pick(&sizes, index),
            pick(&positions, index),
            pick(&repeats, index),
        );
        let fixed = canvas && pick(&attachments, index).trim() == "fixed";
        // The positioning and painting areas of a layer. A fixed layer is placed in the viewport.
        let areas = move |bounds: renderer::Rect<renderer::Pixels>, window: &renderer::Window| {
            let mask = window.content_mask().bounds;
            (
                if fixed { mask } else { bounds },
                if canvas { mask } else { bounds },
            )
        };
        // Insets of the clip and origin boxes from the padding box, where the layer node sits.
        let edges = |value: String, default: [f32; 4]| match value.trim() {
            "border-box" => borders.map(|width| -width),
            "padding-box" => [0.; 4],
            "content-box" => padding,
            _ => default,
        };
        let clip = edges(pick(&clips, index), borders.map(|width| -width));
        let origin = edges(pick(&origins, index), [0.; 4]);
        // The origin box in the clip box: its offset, and how much larger it is.
        let area = [
            origin[3] - clip[3],
            origin[0] - clip[0],
            clip[1] + clip[3] - origin[1] - origin[3],
            clip[0] + clip[2] - origin[0] - origin[2],
        ];
        let inset = std::array::from_fn::<_, 4, _>(|side| px(clip[side] + borders[side]));
        let corners = move |bounds: renderer::Rect<renderer::Pixels>| {
            let outer = renderer::size(
                bounds.size.width + inset[1] + inset[3],
                bounds.size.height + inset[0] + inset[2],
            );
            radius
                .resolve(outer)
                .inset(inset[0], inset[1], inset[2], inset[3])
                .fit(bounds.size)
        };
        let layer = |node: renderer::Node| {
            node.absolute()
                .top(px(clip[0]))
                .right(px(clip[1]))
                .bottom(px(clip[2]))
                .left(px(clip[3]))
        };
        if let Some(source) = image.strip_prefix("url(").and_then(|v| v.strip_suffix(')')) {
            if let Some(path) = super::image::url_file(source, base, assets) {
                let file = path.to_string_lossy().into_owned();
                let view = view.clone();
                let natural = std::cell::Cell::new(None::<[f32; 2]>);
                layers.push(layer(paint(
                    canvas,
                    move |bounds, _, window| {
                        let (bounds, clip) = areas(bounds, window);
                        let area = if fixed { [0.; 4] } else { area };
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
                        let key = (id, slot * 64 + index);
                        let words: Vec<&str> = repeat.split_whitespace().collect();
                        let modes = match words.as_slice() {
                            ["repeat-x"] => ["repeat", "no-repeat"],
                            ["repeat-y"] => ["no-repeat", "repeat"],
                            [one] => [*one, *one],
                            [x, y, ..] => [*x, *y],
                            [] => ["repeat", "repeat"],
                        };
                        let limit = super::canvas::RASTER_MAX as f32;
                        let layout = |natural: [f32; 2]| {
                            let extent = [width + area[2], height + area[3]];
                            let [mut tile, mut at] = super::effects::tile(
                                &size,
                                &position,
                                &units,
                                extent[0],
                                extent[1],
                                Some(natural),
                            );
                            // `round` scales tiles to fit a whole number, and `space` puts whole tiles at
                            // both edges with even gaps between them.
                            let mut gap = [0.; 2];
                            let mut repeats = [false; 2];
                            for axis in 0..2 {
                                let fit = extent[axis] / tile[axis];
                                repeats[axis] = match modes[axis] {
                                    "no-repeat" => false,
                                    "round" => {
                                        tile[axis] = extent[axis] / fit.round().max(1.);
                                        true
                                    }
                                    "space" if fit.floor() >= 2. => {
                                        let count = fit.floor();
                                        gap[axis] =
                                            (extent[axis] - count * tile[axis]) / (count - 1.);
                                        at[axis] = 0.;
                                        true
                                    }
                                    "space" => {
                                        if fit < 1. {
                                            tile[axis] = 0.;
                                        }
                                        false
                                    }
                                    _ => true,
                                };
                            }
                            let [[tile_width, tile_height], [x, y]] = [tile, at];
                            // Repeated tiles group into bitmaps of at least 256 device pixels per side. This
                            // bounds draw calls, and tiles in a bitmap have no seams from filtering.
                            let group = |axis: usize, side: f32| {
                                if repeats[axis] && gap[axis] == 0. {
                                    (256. / (side * scale)).ceil().max(1.)
                                } else {
                                    1.
                                }
                            };
                            let [nx, ny] = [group(0, tile_width), group(1, tile_height)];
                            let cell = [tile_width * nx, tile_height * ny]
                                .map(|extent| (extent * scale).ceil().clamp(1., limit) as u32);
                            let source = if nx * ny > 1. {
                                format!("{file} {nx} {ny}")
                            } else {
                                file.clone()
                            };
                            (
                                [tile_width, tile_height],
                                [x + area[0], y + area[1]],
                                [nx, ny],
                                [gap, repeats.map(f32::from)],
                                cell,
                                source,
                            )
                        };
                        let fixed = |bitmaps: &Bitmaps, image: &Arc<renderer::RenderImage>| {
                            bitmaps
                                .get(&file)
                                .is_some_and(|bitmap| Arc::ptr_eq(bitmap, image))
                        };
                        let stale = |backgrounds: &Backgrounds,
                                     bitmaps: &Bitmaps,
                                     source: &str,
                                     cell: [u32; 2]| {
                            backgrounds.get(&key).is_none_or(|(old, image)| {
                                old != source
                                    || !fixed(bitmaps, image)
                                        && [image.size().width, image.size().height]
                                            .map(|side| side as u32)
                                            != cell
                            })
                        };
                        let bitmap = if natural.get().is_none_or(|natural| {
                            let (.., cell, source) = layout(natural);
                            stale(backgrounds, bitmaps, &source, cell)
                        }) {
                            // Take a decoded bitmap only where a layer holds it at once, since `retire`
                            // drops unheld bitmaps.
                            let bitmap = match bitmaps.get(&file).cloned() {
                                Some(bitmap) => bitmap,
                                None => {
                                    let mut decoding = view.decoding.borrow_mut();
                                    let done = decoding.get(&file).and_then(|(_, slot)| {
                                        slot.lock().unwrap_or_else(PoisonError::into_inner).take()
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
                            // Undecodable files are SVG or broken, and SVG draws at each size.
                            let tree = (bitmap.size().width as u32 == 0)
                                .then(|| {
                                    std::fs::read_to_string(&path)
                                        .map_err(anyhow::Error::from)
                                        .and_then(|text| {
                                            // A view box keeps its ratio in other tiles.
                                            let kept = text.contains("viewBox")
                                                && !text.contains("AspectRatio=\"none")
                                                && !text.contains("AspectRatio='none");
                                            Ok((super::canvas::svg_tree(&text, false)?, kept))
                                        })
                                        .inspect_err(|error| {
                                            eprintln!("apps: background image {file}: {error}")
                                        })
                                        .ok()
                                })
                                .flatten();
                            if natural.get().is_none() {
                                natural.set(Some(match &tree {
                                    Some((tree, _)) => [tree.size().width(), tree.size().height()],
                                    None => {
                                        [bitmap.size().width as f32, bitmap.size().height as f32]
                                    }
                                }));
                            }
                            Some((bitmap, tree))
                        } else {
                            None
                        };
                        let Some(natural) = natural.get().filter(|[w, h]| *w > 0. && *h > 0.)
                        else {
                            return;
                        };
                        let (
                            [tile_width, tile_height],
                            [x, y],
                            [nx, ny],
                            [gap, repeats],
                            cell,
                            source,
                        ) = layout(natural);
                        if tile_width <= 0. || tile_height <= 0. {
                            return;
                        }
                        if let Some((bitmap, tree)) = bitmap
                            && stale(backgrounds, bitmaps, &source, cell)
                        {
                            // Boxes with the same tiles share one grouped bitmap.
                            let group = (nx * ny > 1.).then(|| {
                                use std::hash::{Hash, Hasher};
                                let name =
                                    format!("{source} {cell:?} {tile_width} {tile_height} {scale}");
                                let mut hasher = std::hash::DefaultHasher::new();
                                name.hash(&mut hasher);
                                ((usize::MAX, hasher.finish() as usize), name)
                            });
                            let held = group.as_ref().and_then(|(at, name)| {
                                backgrounds.get(at).filter(|(old, _)| old == name)
                            });
                            let image = match held.map(|(_, image)| image.clone()) {
                                Some(image) => image,
                                None => {
                                    let raster = [tile_width, tile_height].map(|extent| {
                                        (extent * scale).ceil().clamp(1., limit) as u32
                                    });
                                    let image = match tree {
                                        Some((tree, kept)) => {
                                            let size = [tree.size().width(), tree.size().height()];
                                            let [w, h] = raster.map(|side| side as f32);
                                            let scale = (w / size[0]).min(h / size[1]);
                                            let [fw, fh] = size.map(|side| side * scale);
                                            let [x, y] = [(w - fw) / 2., (h - fh) / 2.];
                                            let fit = ([fw, fh], [1., 0., 0., 1., x, y]);
                                            match super::canvas::render_svg(
                                                &tree, raster[0], raster[1], kept.then_some(fit),
                                            ) {
                                                Ok(image) => image,
                                                Err(_) => return,
                                            }
                                        }
                                        None => bitmap,
                                    };
                                    let image = match super::canvas::pixmap(&image)
                                        .filter(|_| nx * ny > 1.)
                                        .zip(tiny_skia::Pixmap::new(cell[0], cell[1]))
                                    {
                                        Some((mut tile, mut pixels)) => {
                                            use tiny_skia as skia;
                                            // Scale a bitmap to its tile first, with clamped edges, so repeats don't
                                            // blend the opposite edges into each other.
                                            if [tile.width(), tile.height()] != raster
                                                && let Some(mut scaled) =
                                                    skia::Pixmap::new(raster[0], raster[1])
                                                && let Some(rect) = skia::Rect::from_xywh(
                                                    0.,
                                                    0.,
                                                    raster[0] as f32,
                                                    raster[1] as f32,
                                                )
                                            {
                                                scaled.fill_rect(
                                                    rect,
                                                    &skia::Paint {
                                                        shader: skia::Pattern::new(
                                                            tile.as_ref(),
                                                            skia::SpreadMode::Pad,
                                                            skia::FilterQuality::Bilinear,
                                                            1.,
                                                            skia::Transform::from_scale(
                                                                raster[0] as f32
                                                                    / tile.width() as f32,
                                                                raster[1] as f32
                                                                    / tile.height() as f32,
                                                            ),
                                                        ),
                                                        ..Default::default()
                                                    },
                                                    skia::Transform::identity(),
                                                    None,
                                                );
                                                tile = scaled;
                                            }
                                            let [w, h] = cell.map(|side| side as f32);
                                            if let Some(rect) = skia::Rect::from_xywh(0., 0., w, h)
                                            {
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
                                    if let Some((at, name)) = group {
                                        backgrounds.insert(at, (name, image.clone()));
                                    }
                                    image
                                }
                            };
                            if let Some((_, old)) = backgrounds.insert(key, (source, image))
                                && Arc::strong_count(&old) == 1
                            {
                                window.release_image(old);
                            }
                        }
                        let Some((_, image)) = backgrounds.get(&key) else {
                            return;
                        };
                        // Calculate visible tile indices directly. Iteration from distant origins or across
                        // tiny tiles can run indefinitely.
                        let visible = window.content_mask().bounds.intersect(clip);
                        let [width, height] = [tile_width * nx, tile_height * ny];
                        let [step_x, step_y] = [width + gap[0], height + gap[1]];
                        let snap = |at: f32| (at * scale).round() / scale;
                        let origin = [
                            f32::from(bounds.origin.x) + x,
                            f32::from(bounds.origin.y) + y,
                        ];
                        let span = |repeats: bool, start: f32, side: f32, low, high| {
                            if repeats {
                                let [low, high]: [f32; 2] = [f32::from(low), f32::from(high)];
                                ((low - start) / side).floor() as i64
                                    ..((high - start) / side).ceil() as i64
                            } else {
                                0..1
                            }
                        };
                        let columns = span(
                            repeats[0] > 0.,
                            origin[0],
                            step_x,
                            visible.left(),
                            visible.right(),
                        );
                        let rows = span(
                            repeats[1] > 0.,
                            origin[1],
                            step_y,
                            visible.top(),
                            visible.bottom(),
                        );
                        if columns
                            .end
                            .saturating_sub(columns.start)
                            .saturating_mul(rows.end.saturating_sub(rows.start))
                            > 1 << 16
                        {
                            return;
                        }
                        let radii = corners(bounds);
                        // Grouped tiles snap to device pixels, so they meet without seams.
                        let grouped = nx * ny > 1.;
                        for row in rows {
                            for column in columns.clone() {
                                let [left, top] = [
                                    origin[0] + column as f32 * step_x,
                                    origin[1] + row as f32 * step_y,
                                ];
                                let [left, top, right, bottom] =
                                    [left, top, left + width, top + height]
                                        .map(|at| if grouped { snap(at) } else { at });
                                window.draw_image(
                                    clip,
                                    renderer::Rect::between(
                                        renderer::point(px(left), px(top)),
                                        renderer::point(px(right), px(bottom)),
                                    ),
                                    radii,
                                    image.clone(),
                                    false,
                                    pixelated,
                                );
                            }
                        }
                    },
                )));
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
        layers.push(layer(paint(canvas, move |bounds, _, window| {
            let (bounds, clip) = areas(bounds, window);
            let area = if fixed { [0.; 4] } else { area };
            let [width, height] = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            if width <= 0. || height <= 0. {
                return;
            }
            // Keep the gradient strip's outer pixels outside the box so filtering cannot sample
            // neighboring atlas tiles.
            let scale = window.scale_factor();
            let repeated = matches!(repeat.trim(), "" | "repeat" | "repeat repeat");
            let axis = strip.filter(|_| {
                !canvas
                    && matches!(size.as_str(), "" | "auto" | "auto auto")
                    && (area == [0.; 4] || repeated)
                    && style::components(&position, ' ').iter().all(|token| {
                        token.ends_with('%')
                            || matches!(*token, "left" | "right" | "top" | "bottom" | "center")
                    })
            });
            let whole = |value: f32| (value * scale - (value * scale).round()).abs() < 0.01;
            let radii = corners(bounds);
            let [aw, ah] = [width + area[2], height + area[3]];
            let [[tw, th], [x, y]] = super::effects::tile(&size, &position, &units, aw, ah, None);
            let [x, y] = [x + area[0], y + area[1]];
            let [cw, ch] = [tw, th].map(|side| side * (256. / (side * scale)).ceil().max(1.));
            let grouped = super::effects::tile(&size, "0 0", &units, cw, ch, None)[0] == [tw, th];
            // Repeated canvas layers always tile, since they cover more than the root box.
            let [cw, ch] = if grouped { [cw, ch] } else { [tw, th] };
            let tiled = (axis.is_none()
                && repeated
                && radii.is_zero()
                && (canvas
                    || [tw, th, x, y].into_iter().all(whole)
                        && cw * ch < width * height
                        && grouped))
                .then_some([x.rem_euclid(tw), y.rem_euclid(th)]);
            let ((width, height), within) = match axis {
                Some(1) => ((3., height), [0., area[1], 3., ah]),
                Some(_) => ((width, 3.), [area[0], 0., aw, 3.]),
                None if tiled.is_some() => ((cw, ch), [0., 0., cw, ch]),
                None => ((width, height), [area[0], area[1], aw, ah]),
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
            for value in within {
                inputs.push_str(&value.to_bits().to_string());
                inputs.push('\0');
            }
            let view = &*view;
            let mut host = view.host.borrow_mut();
            let dom = host.dom.entry(view.id).or_default();
            let key = (id, slot * 64 + index);
            let fits = |image: &renderer::RenderImage| {
                [image.size().width, image.size().height].map(|side| side as u32) == [rw, rh]
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
                        &kind, &body, &size, position, &repeat, units, width, height, within,
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
                let visible = window.content_mask().bounds.intersect(clip);
                let start = |edge, at: f32, side: f32, cell: f32, low| {
                    let at = f32::from(edge) + at - if at > 0. { side } else { 0. };
                    let at = (at * scale).round() / scale;
                    px(at + cell * ((f32::from(low) - at) / cell).floor())
                };
                let mut top = start(bounds.top(), y, th, ch, visible.top());
                while top < visible.bottom() {
                    let mut left = start(bounds.left(), x, tw, cw, visible.left());
                    while left < visible.right() {
                        let target = renderer::Rect::new(
                            renderer::point(left, top),
                            renderer::size(px(cw), px(ch)),
                        );
                        window.draw_image(clip, target, radii, image.clone(), false, false);
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
                window.draw_image(bounds, target, radii, image.clone(), false, false);
            }
        })));
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
