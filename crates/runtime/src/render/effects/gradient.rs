use crate::css;
use crate::css::properties::Prop;
use crate::css::value::transform::angle;
use crate::css::value::transform::origin;
use crate::css::value::units::Lengths;

fn first_argument(body: &str) -> (String, Option<String>) {
    let parts = css::value::tokens::components(body, ',');
    let words: Vec<&str> = parts
        .first()
        .map_or("", |part| part.trim())
        .split_whitespace()
        .collect();
    match words.iter().position(|word| *word == "in") {
        Some(index) => {
            let end = words[index + 1..]
                .iter()
                .position(|word| *word == "hue")
                .map_or(index + 2, |hue| index + hue + 2)
                .min(words.len());
            let rest = words[..index].iter().chain(&words[end..]);
            (
                rest.copied().collect::<Vec<_>>().join(" "),
                Some(words[index + 1..end].join(" ")),
            )
        }
        None => (words.join(" "), None),
    }
}

pub(crate) fn gradient_axis(kind: &str, body: &str, units: &Lengths) -> Option<usize> {
    if kind.trim_start_matches("repeating-") != "linear-gradient" {
        return None;
    }
    let (first, _) = first_argument(body);
    let degrees = match first.strip_prefix("to ") {
        Some(side) => match side.trim() {
            "top" => 0.,
            "right" => 90.,
            "bottom" => 180.,
            "left" => 270.,
            _ => return None,
        },
        None => angle(&first, units).unwrap_or(180.),
    };
    match degrees.rem_euclid(180.) {
        0. => Some(1),
        90. => Some(0),
        _ => None,
    }
}

// The size and offset of a background tile in its positioning area. `natural` is the size of an image.
pub(crate) fn tile(
    size: &str,
    position: &str,
    units: &Lengths,
    width: f32,
    height: f32,
    natural: Option<[f32; 2]>,
) -> [[f32; 2]; 2] {
    let dims = css::value::tokens::components(size, ' ');
    let [tw, th] = match (dims.as_slice(), natural) {
        ([fit @ ("cover" | "contain")], Some([nw, nh])) => {
            let [x, y] = [width / nw, height / nh];
            let scale = if *fit == "cover" { x.max(y) } else { x.min(y) };
            [nw * scale, nh * scale]
        }
        (["cover" | "contain"], None) => [width, height],
        _ => {
            let dim = |axis: usize| {
                dims.get(axis)
                    .filter(|value| **value != "auto")
                    .and_then(|value| units.resolve(value, [width, height][axis]))
            };
            match ([dim(0), dim(1)], natural) {
                ([Some(w), Some(h)], _) => [w, h],
                ([Some(w), None], Some([nw, nh])) => [w, w * nh / nw],
                ([None, Some(h)], Some([nw, nh])) => [h * nw / nh, h],
                ([None, None], Some(natural)) => natural,
                ([w, h], _) => [w.unwrap_or(width), h.unwrap_or(height)],
            }
        }
    }
    .map(|side| side.max(0.1));
    let tokens = css::value::tokens::components(position, ' ');
    // Three and four values give each keyword an offset from its edge.
    let mut pairs: Vec<(&str, Option<&str>)> = Vec::with_capacity(2);
    for token in tokens.iter().copied() {
        match pairs.last_mut() {
            Some(last)
                if tokens.len() > 2
                    && !matches!(token, "left" | "right" | "top" | "bottom" | "center") =>
            {
                last.1 = Some(token)
            }
            _ => pairs.push((token, None)),
        }
    }
    let center = ("center", None);
    let (x, y) = match pairs.as_slice() {
        [only] if matches!(only.0, "top" | "bottom") => (center, *only),
        [only] => (*only, center),
        [a, b, ..] if matches!(a.0, "top" | "bottom") || matches!(b.0, "left" | "right") => {
            (*b, *a)
        }
        [a, b, ..] => (*a, *b),
        [] => (("0%", None), ("0%", None)),
    };
    let at = [(x, width - tw), (y, height - th)].map(|((token, offset), free)| {
        let offset = offset
            .and_then(|offset| units.resolve(offset, free))
            .unwrap_or(0.);
        match token {
            "left" | "top" => offset,
            "center" => free / 2.,
            "right" | "bottom" => free - offset,
            value => units.resolve(value, free).unwrap_or(0.),
        }
    });
    [[tw, th], at]
}

/// The offset of a CSS `<position>` in `free`, the room left around the placed box.
pub(crate) fn place(position: &str, free: [f32; 2], units: &Lengths) -> [f32; 2] {
    let tokens = css::value::tokens::components(position, ' ');
    let keyword = |token: &str| matches!(token, "left" | "right" | "top" | "bottom" | "center");
    let offset = |token: &str, axis: usize| match token {
        "left" | "top" => 0.,
        "center" => free[axis] / 2.,
        "right" | "bottom" => free[axis],
        value => units.resolve(value, free[axis]).unwrap_or(0.),
    };
    if tokens.len() < 3 {
        let (x, y) = match tokens.as_slice() {
            [only] if matches!(*only, "top" | "bottom") => ("center", *only),
            [only] => (*only, "center"),
            [a, b] if matches!(*a, "top" | "bottom") || matches!(*b, "left" | "right") => (*b, *a),
            [a, b, ..] => (*a, *b),
            [] => ("0%", "0%"),
        };
        return [offset(x, 0), offset(y, 1)];
    }
    // Three or four values: keywords, each with an optional offset from its side.
    let mut at = [None; 2];
    let mut index = 0;
    while let Some(&side) = tokens.get(index) {
        let length = tokens.get(index + 1).filter(|token| !keyword(token));
        index += 1 + usize::from(length.is_some());
        let axis = match side {
            "left" | "right" => 0,
            "top" | "bottom" => 1,
            _ => continue,
        };
        let length = length.map_or(0., |value| units.resolve(value, free[axis]).unwrap_or(0.));
        at[axis] = Some(if matches!(side, "right" | "bottom") {
            free[axis] - length
        } else {
            length
        });
    }
    [0, 1].map(|axis| at[axis].unwrap_or(free[axis] / 2.))
}

fn gradient_stops<S: AsRef<str>>(
    parts: &[S],
    prelude: bool,
    units: &Lengths,
    extent: f32,
    wide: bool,
) -> Option<Vec<(f32, (u32, u8))>> {
    let color = if wide {
        css::value::color::wide_rgb
    } else {
        css::value::color::color
    };
    let mut stops: Vec<(Option<f32>, (u32, u8))> = Vec::new();
    let mut hints = Vec::new();
    for stop in parts.iter().map(AsRef::as_ref).skip(usize::from(prelude)) {
        let tokens = css::value::tokens::components(stop.trim(), ' ');
        let Some(ink) = tokens.iter().find_map(|token| color(token)) else {
            // A lone position between two colors moves their midpoint.
            if let [token] = tokens.as_slice()
                && !stops.is_empty()
                && let Some(offset) = units.resolve(token, extent).or_else(|| angle(token, units))
            {
                hints.push(stops.len());
                stops.push((Some(offset), (0, 0)));
            }
            continue;
        };
        let offsets: Vec<f32> = tokens
            .iter()
            .filter(|token| css::value::color::color(token).is_none())
            .filter_map(|token| units.resolve(token, extent).or_else(|| angle(token, units)))
            .take(2)
            .collect();
        if offsets.is_empty() {
            stops.push((None, ink));
        }
        stops.extend(offsets.into_iter().map(|offset| (Some(offset), ink)));
    }
    if stops.len() < 2 {
        return None;
    }
    let last = stops.len() - 1;
    stops[0].0.get_or_insert(0.);
    stops[last].0.get_or_insert(extent);
    let mut floor = f32::MIN;
    for stop in &mut stops {
        if let Some(offset) = &mut stop.0 {
            *offset = offset.max(floor);
            floor = *offset;
        }
    }
    let mut index = 1;
    while index < last {
        if stops[index].0.is_none() {
            let end = (index..=last)
                .find(|&end| stops[end].0.is_some())
                .unwrap_or(last);
            let (a, b) = (
                stops[index - 1].0.unwrap_or(0.),
                stops[end].0.unwrap_or(extent),
            );
            for (step, stop) in stops[index..end].iter_mut().enumerate() {
                stop.0 = Some(a + (b - a) * (step + 1) as f32 / (end - index + 1) as f32);
            }
            index = end;
        }
        index += 1;
    }
    let mut result = Vec::with_capacity(stops.len() + hints.len() * 8);
    for (index, &(offset, ink)) in stops.iter().enumerate() {
        let offset = offset.unwrap_or(0.);
        if !hints.contains(&index) {
            result.push((offset, ink));
            continue;
        }
        let (Some(&(a, (ink_a, alpha_a))), Some(&(Some(b), (ink_b, alpha_b)))) = (
            result.last(),
            stops
                .get(index + 1)
                .filter(|_| !hints.contains(&(index + 1))),
        ) else {
            continue;
        };
        if b <= a {
            continue;
        }
        let exponent = 0.5f32.ln() / ((offset - a) / (b - a)).clamp(0.001, 0.999).ln();
        for step in 1..16 {
            let t = step as f32 / 16.;
            let p = t.powf(exponent);
            let mix = |a: u32, b: u32| (a as f32 + (b as f32 - a as f32) * p).round() as u32;
            let (shifts, mask) = if wide {
                ([20, 10, 0], 1023)
            } else {
                ([16, 8, 0], 255)
            };
            let ink = shifts
                .map(|shift| mix(ink_a >> shift & mask, ink_b >> shift & mask) << shift)
                .into_iter()
                .fold(0, |ink, channel| ink | channel);
            result.push((
                a + (b - a) * t,
                (ink, mix(alpha_a.into(), alpha_b.into()) as u8),
            ));
        }
    }
    (result.len() >= 2).then_some(result)
}

pub(crate) fn axis_mask(
    style: &css::properties::Style,
    units: &Lengths,
    size: [f32; 2],
    raster: [u32; 2],
) -> Option<Option<Vec<u8>>> {
    let plain = |prop: Prop, defaults: &[&str]| {
        css::value::tokens::components(style.get(prop).unwrap_or(""), ',')
            .iter()
            .all(|part| {
                let part = part.trim();
                part.is_empty() || defaults.contains(&part)
            })
    };
    if !plain(Prop::MASK_SIZE, &["auto", "auto auto"])
        || !plain(
            Prop::MASK_POSITION,
            &["0% 0%", "0 0", "0px 0px", "left top"],
        )
        || !plain(Prop::MASK_MODE, &["alpha", "match-source"])
    {
        return None;
    }
    let images = css::value::tokens::components(style.get(Prop::MASK_IMAGE).unwrap_or(""), ',');
    let composites =
        css::value::tokens::components(style.get(Prop::MASK_COMPOSITE).unwrap_or(""), ',');
    let mut layers = Vec::with_capacity(images.len());
    for (index, image) in images.iter().enumerate() {
        let (origin, outline) = super::super::paint::mask::layer_boxes(style, units, size, index);
        if origin != [0., 0., size[0], size[1]] && !plain(Prop::MASK_REPEAT, &["repeat"]) {
            return None;
        }
        // The coverage of the outline that clips the layer.
        let cover = match super::super::paint::mask::whole(&outline, size) {
            false => {
                let mut mask = tiny_skia::Mask::new(raster[0], raster[1])?;
                if let Some(path) = css::value::shape::path(&outline) {
                    mask.fill_path(
                        &path,
                        tiny_skia::FillRule::Winding,
                        true,
                        tiny_skia::Transform::from_scale(
                            raster[0] as f32 / size[0],
                            raster[1] as f32 / size[1],
                        ),
                    );
                }
                Some(mask.take())
            }
            true => None,
        };
        let mut axes = [
            vec![1f32; raster[0] as usize],
            vec![1f32; raster[1] as usize],
        ];
        let body = image
            .trim()
            .strip_prefix("linear-gradient(")?
            .strip_suffix(')')?;
        let parts = css::value::tokens::components(body, ',');
        let (first, space) = first_argument(body);
        if space.is_some() {
            return None;
        }
        let (axis, reversed, prelude) = match first.as_str() {
            "to bottom" | "180deg" => (1, false, true),
            "to top" | "0deg" => (1, true, true),
            "to right" | "90deg" => (0, false, true),
            "to left" | "270deg" => (0, true, true),
            other
                if other.is_empty()
                    || css::value::tokens::components(other, ' ')
                        .first()
                        .is_some_and(|part| css::value::color::color(part).is_some()) =>
            {
                (1, false, false)
            }
            _ => return None,
        };
        let extent = origin[2 + axis];
        let stops = gradient_stops(&parts, prelude, units, extent, false)?;
        let count = raster[axis] as usize;
        for (index, alpha) in axes[axis].iter_mut().enumerate() {
            let at = (index as f32 + 0.5) * size[axis] / count as f32;
            let at = match extent == size[axis] {
                true => at,
                false => (at - origin[axis]).rem_euclid(extent),
            };
            let at = if reversed { extent - at } else { at };
            let end = stops.partition_point(|(offset, _)| *offset <= at);
            *alpha *= match (
                end.checked_sub(1).map(|start| stops[start]),
                stops.get(end).copied(),
            ) {
                (Some((a, (_, from))), Some((b, (_, to)))) if b > a => {
                    let t = (at - a) / (b - a);
                    (f32::from(from) * (1. - t) + f32::from(to) * t) / 255.
                }
                (Some((_, (_, alpha))), _) | (None, Some((_, (_, alpha)))) => {
                    f32::from(alpha) / 255.
                }
                (None, None) => 1.,
            };
        }
        layers.push((axes, cover));
    }
    // As in `mask_svg`, layers go from the bottom up, and each joins the layers below it by its `mask-composite`.
    let [width, height] = raster.map(|side| side as usize);
    let (last, above) = layers.split_last()?;
    let ops = (0..above.len())
        .map(|index| {
            let composite = composites.get(index % composites.len().max(1));
            Some(match composite.map_or("", |value| value.trim()) {
                "intersect" | "source-in" | "destination-in" => 1,
                "subtract" | "source-out" => 2,
                "exclude" | "xor" => 3,
                "" | "add" | "source-over" | "destination-over" => 0,
                _ => return None,
            })
        })
        .collect::<Option<Vec<u8>>>()?;
    let mut alpha = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let value = |(axes, _): &([Vec<f32>; 2], Option<Vec<u8>>)| axes[0][x] * axes[1][y];
            let covered = |(_, cover): &([Vec<f32>; 2], Option<Vec<u8>>)| {
                (cover.as_ref()).map_or(1., |cover| f32::from(cover[y * width + x]) / 255.)
            };
            let mut below = value(last) * covered(last);
            for (layer, op) in above.iter().zip(&ops).rev() {
                let (cover, layer) = (covered(layer), value(layer));
                let joined = match op {
                    1 => layer * below,
                    2 => layer * (1. - below),
                    3 => layer * (1. - below) + below * (1. - layer),
                    _ => layer + below * (1. - layer),
                };
                // Like Chrome, a layer changes only what its clip covers.
                below += (joined - below) * cover;
            }
            alpha.push(below);
        }
    }
    if alpha.iter().all(|alpha| *alpha >= 0.999) {
        return Some(None);
    }
    let mut bytes = vec![0; width * height * 4];
    for (pixel, alpha) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(alpha) {
        pixel[3] = (alpha * 255. + 0.5) as u8;
    }
    Some(Some(bytes))
}

// A CSS gradient, resolved to straight color stops along its extent: px for linear and radial, degrees for conic.
struct Gradient<'a> {
    kind: &'a str,
    steps: [Option<f32>; 2],
    tile: [f32; 2],
    at: [f32; 2],
    repeating: bool,
    direction: (f32, f32),
    center: (f32, f32),
    radii: (f32, f32),
    from: f32,
    extent: f32,
    wide: bool,
    offsets: Vec<(f32, (u32, u8))>,
    start: f32,
    period: f32,
}

impl Gradient<'_> {
    fn channels(&self) -> ([u32; 3], u32) {
        match self.wide {
            true => ([20, 10, 0], 1023),
            false => ([16, 8, 0], 255),
        }
    }

    // The sRGB color of a stop, or its Display P3 color when the gradient is wide.
    fn output(&self, ink: u32, alpha: u8) -> u32 {
        match self.wide {
            true => css::value::color::p3(&stop_css(true, ink, alpha)).map_or(ink, |(ink, _)| ink),
            false => ink,
        }
    }

    // The straight color at `angle` degrees of a conic gradient, before `output`. `next` is the stop after the
    // last angle, so increasing angles find their stop in a step.
    fn conic_at(&self, angle: f32, next: &mut usize) -> (u32, f32) {
        let angle = match self.repeating {
            true => self.start + (angle - self.start).rem_euclid(self.period),
            false => angle,
        };
        let last = self.offsets.len() - 1;
        if *next > last || *next > 0 && self.offsets[*next - 1].0 >= angle {
            *next = 0;
        }
        while *next < last && self.offsets[*next].0 < angle {
            *next += 1;
        }
        let next = *next;
        let (a, b) = (self.offsets[next.saturating_sub(1)], self.offsets[next]);
        let t = if b.0 > a.0 {
            ((angle - a.0) / (b.0 - a.0)).clamp(0., 1.)
        } else {
            1.
        };
        let (shifts, mask) = self.channels();
        let ink = shifts.map(|shift| {
            let mix = (((a.1).0 >> shift) & mask) as f32 * (1. - t)
                + (((b.1).0 >> shift) & mask) as f32 * t;
            (mix.round() as u32) << shift
        });
        let alpha = ((a.1).1 as f32 * (1. - t) + (b.1).1 as f32 * t) / 255.;
        (ink[0] | ink[1] | ink[2], alpha)
    }
}

fn stop_css(wide: bool, ink: u32, alpha: u8) -> String {
    match wide {
        true => {
            let [r, g, b] = [20, 10, 0].map(|shift| (ink >> shift & 1023) as f32 / 511.5 - 0.5);
            format!("color(srgb {r} {g} {b} / {})", alpha as f32 / 255.)
        }
        false => format!("#{ink:06x}{alpha:02x}"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn gradient_svg(
    kind: &str,
    body: &str,
    size: &str,
    position: &str,
    repeat: &str,
    units: Lengths,
    width: f32,
    height: f32,
    area: [f32; 4],
) -> Option<(String, bool)> {
    let gradient = gradient(kind, body, size, position, repeat, units, area)?;
    let Gradient {
        tile: [tw, th],
        at: [x, y],
        repeating,
        direction,
        center,
        radii,
        from,
        extent,
        wide,
        start,
        period,
        ..
    } = gradient;
    let offsets = &gradient.offsets;
    let spread = if repeating { "repeat" } else { "pad" };
    let svg_stops = offsets
        .iter()
        .map(|&(offset, (ink, alpha))| {
            format!(
                "<stop offset=\"{}\" stop-color=\"#{:06x}\" stop-opacity=\"{}\"/>",
                ((offset - start) / period).clamp(0., 1.),
                gradient.output(ink, alpha),
                alpha as f32 / 255.
            )
        })
        .collect::<String>();
    let content = match gradient.kind {
        "linear-gradient" => {
            let (dx, dy) = direction;
            let x1 = tw / 2. - dx * extent / 2. + dx * start;
            let y1 = th / 2. - dy * extent / 2. + dy * start;
            format!(
                "<defs><linearGradient id=\"g\" gradientUnits=\"userSpaceOnUse\" x1=\"{x1}\" y1=\"{y1}\" x2=\"{}\" y2=\"{}\" spreadMethod=\"{spread}\">{svg_stops}</linearGradient></defs><rect width=\"{tw}\" height=\"{th}\" fill=\"url(#g)\"/>",
                x1 + dx * period,
                y1 + dy * period
            )
        }
        "radial-gradient" => {
            let (cx, cy) = center;
            format!(
                "<defs><radialGradient id=\"g\" gradientUnits=\"userSpaceOnUse\" cx=\"{cx}\" cy=\"{cy}\" r=\"{period}\" spreadMethod=\"{spread}\" gradientTransform=\"translate({cx} {cy}) scale(1 {}) translate({} {})\">{svg_stops}</radialGradient></defs><rect width=\"{tw}\" height=\"{th}\" fill=\"url(#g)\"/>",
                radii.1 / radii.0.max(0.01),
                -cx,
                -cy
            )
        }
        _ => {
            let (cx, cy) = center;
            let reach = tw.hypot(th) * 2.;
            // Wedges end at each stop, so thin and hard stops keep their angles, and are at most 2deg wide.
            let mut cuts = vec![0f32, 360.];
            let repeats = if repeating {
                (360. / period).ceil() + 1.
            } else {
                1.
            };
            if repeats * offsets.len() as f32 <= 4096. {
                let first = if repeating {
                    ((-start) / period).floor()
                } else {
                    0.
                };
                for turn in 0..repeats as i32 + 1 {
                    let shift = (first + turn as f32) * if repeating { period } else { 0. };
                    cuts.extend(
                        offsets
                            .iter()
                            .map(|(offset, _)| offset + shift)
                            .filter(|angle| (0. ..360.).contains(angle)),
                    );
                }
            } else {
                cuts.extend((1..1440).map(|step| step as f32 / 4.));
            }
            cuts.sort_by(f32::total_cmp);
            cuts.dedup_by(|b, a| *b - *a < 1e-3);
            let mut pieces = Vec::with_capacity(cuts.len() * 2);
            for pair in cuts.windows(2) {
                let count = ((pair[1] - pair[0]) / 2.).ceil().max(1.);
                let width = (pair[1] - pair[0]) / count;
                pieces.extend((0..count as u32).map(|n| pair[0] + n as f32 * width));
            }
            pieces.push(360.);
            let mut paths = String::new();
            for wedge in pieces.windows(2) {
                let (ink, alpha) = gradient.conic_at((wedge[0] + wedge[1]) / 2., &mut 0);
                let ink = gradient.output(ink, (alpha * 255.) as u8);
                let [start, end] = [from + wedge[0], from + wedge[1]].map(f32::to_radians);
                paths.push_str(&format!(
                    "<path d=\"M{cx} {cy}L{} {}L{} {}Z\" fill=\"#{ink:06x}\" fill-opacity=\"{alpha}\"/>",
                    cx + reach * start.sin(),
                    cy - reach * start.cos(),
                    cx + reach * end.sin(),
                    cy - reach * end.cos()
                ));
            }
            format!(
                "<defs><clipPath id=\"c\"><rect width=\"{tw}\" height=\"{th}\"/></clipPath></defs><g clip-path=\"url(#c)\" shape-rendering=\"crispEdges\">{paths}</g>"
            )
        }
    };
    let body = tiled(gradient.steps, [tw, th], [x, y], [width, height], &content);
    Some((
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">{body}</svg>"
        ),
        wide,
    ))
}

// The pixels of a conic gradient over a `width` by `height` area, at `rw` by `rh` pixels. Each pixel takes the color
// at the angle of its center, like Skia: a scripted angle that changes each frame then costs one pass, not an SVG
// of wedges.
#[allow(clippy::too_many_arguments)]
pub(crate) fn conic_image(
    kind: &str,
    body: &str,
    size: &str,
    position: &str,
    repeat: &str,
    units: Lengths,
    [width, height]: [f32; 2],
    area: [f32; 4],
    [rw, rh]: [u32; 2],
    previous: Option<&std::sync::Arc<renderer::RenderImage>>,
) -> Option<std::sync::Arc<renderer::RenderImage>> {
    let gradient = gradient(kind, body, size, position, repeat, units, area)?;
    let [[tw, th], [x, y]] = [gradient.tile, gradient.at];
    if tw <= 0. || th <= 0. {
        return None;
    }
    let [rw, rh] = [rw, rh].map(|side| side.clamp(1, super::super::raster::RASTER_MAX));
    let [sx, sy] = [width / rw as f32, height / rh as f32];
    // One entry per half device pixel at the far corner, so hard stops stay sharp.
    let reach = (tw / sx).hypot(th / sy);
    let steps = ((reach * std::f32::consts::TAU * 2.).ceil() as usize).clamp(720, 1 << 16);
    // Runs of one color are common, and a Display P3 conversion is slow, so the last one is kept.
    let (mut last, mut next) = ((u32::MAX, 0, 0), 0);
    let table: Vec<[u8; 4]> = (0..steps)
        .map(|step| {
            let (ink, alpha) =
                gradient.conic_at((step as f32 + 0.5) * 360. / steps as f32, &mut next);
            let byte = (alpha * 255.) as u8;
            if (last.0, last.1) != (ink, byte) {
                last = (ink, byte, gradient.output(ink, byte));
            }
            let [r, g, b] =
                [16, 8, 0].map(|shift| ((last.2 >> shift & 255) as f32 * alpha + 0.5) as u8);
            [b, g, r, (alpha * 255. + 0.5) as u8]
        })
        .collect();
    let mode = repeat.split_whitespace().next().unwrap_or("repeat");
    let [wrap_x, wrap_y] = match mode {
        "no-repeat" => [false; 2],
        "repeat-x" => [true, false],
        "repeat-y" => [false, true],
        _ => [true; 2],
    };
    let (cx, cy) = gradient.center;
    let turn = steps as f32 / std::f32::consts::TAU;
    let from = gradient.from.to_radians().rem_euclid(std::f32::consts::TAU);
    // A scripted angle moves less than a table step in most frames, and then the pixels are the same.
    let digest = {
        use std::hash::{Hash, Hasher};
        let mut hasher = rustc_hash::FxHasher::default();
        table.hash(&mut hasher);
        [
            rw,
            rh,
            u32::from(wrap_x),
            u32::from(wrap_y),
            u32::from(gradient.wide),
        ]
        .hash(&mut hasher);
        [sx, sy, x, y, tw, th, cx, cy, from]
            .map(f32::to_bits)
            .hash(&mut hasher);
        hasher.finish()
    };
    if let Some(previous) = previous
        && DRAWN.with_borrow(|drawn| drawn.get(&previous.id) == Some(&digest))
    {
        return Some(previous.clone());
    }
    let mut bytes = SPARE.take();
    bytes.clear();
    bytes.resize(rw as usize * rh as usize * 4, 0);
    for (row, line) in bytes.chunks_exact_mut(rw as usize * 4).enumerate() {
        // `rem_euclid` calls `fmodf`, so only rows and columns outside the first tile take it.
        let mut v = (row as f32 + 0.5) * sy - y;
        if !(0. ..th).contains(&v) {
            match wrap_y {
                true => v = v.rem_euclid(th),
                false => continue,
            }
        }
        let dy = v - cy;
        for (column, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let mut u = (column as f32 + 0.5) * sx - x;
            if !(0. ..tw).contains(&u) {
                match wrap_x {
                    true => u = u.rem_euclid(tw),
                    false => continue,
                }
            }
            // atan2(u - cx, -dy) by a polynomial: libm's is most of the time of this loop.
            let (y, x) = (u - cx, -dy);
            let (ax, ay) = (x.abs(), y.abs());
            let a = ax.min(ay) / ax.max(ay).max(f32::MIN_POSITIVE);
            let q = a * a;
            let mut angle = ((-0.046_496_47 * q + 0.159_314_22) * q - 0.327_622_76) * q * a + a;
            if ay > ax {
                angle = std::f32::consts::FRAC_PI_2 - angle;
            }
            if x < 0. {
                angle = std::f32::consts::PI - angle;
            }
            if y < 0. {
                angle = -angle;
            }
            // `angle - from` is within two turns of the range.
            let mut angle = angle - from;
            while angle < 0. {
                angle += std::f32::consts::TAU;
            }
            while angle >= std::f32::consts::TAU {
                angle -= std::f32::consts::TAU;
            }
            *pixel = table[((angle * turn) as usize).min(steps - 1)];
        }
    }
    let image = renderer::RenderImage::new_premultiplied(rw, rh, bytes);
    let image = std::sync::Arc::new(match gradient.wide {
        true => image.display_p3(),
        false => image,
    });
    DRAWN.with_borrow_mut(|drawn| {
        if drawn.len() >= 256 {
            drawn.clear();
        }
        drawn.insert(image.id, digest);
    });
    Some(image)
}

thread_local! {
    // The pixels of the last released gradient image, for the next one. New pages cost more than a memset.
    static SPARE: std::cell::Cell<Vec<u8>> = const { std::cell::Cell::new(Vec::new()) };
    // The digests of the table and place of conic images, by image.
    static DRAWN: std::cell::RefCell<rustc_hash::FxHashMap<u64, u64>> = Default::default();
}

pub(crate) fn recycle(bytes: Vec<u8>) {
    if bytes.capacity() <= 4 << 20 {
        SPARE.set(bytes);
    }
}

#[allow(clippy::too_many_arguments)]
fn gradient<'a>(
    kind: &'a str,
    body: &str,
    size: &str,
    position: &str,
    repeat: &'a str,
    units: Lengths,
    area: [f32; 4],
) -> Option<Gradient<'a>> {
    let ([[tw, th], [x, y]], steps) =
        tile_repeat(size, position, repeat, &units, area[2], area[3], None);
    let [x, y] = [x + area[0], y + area[1]];
    let parts = css::value::tokens::components(body, ',');
    let (first, space) = first_argument(body);
    let first = first.as_str();
    let repeating = kind.starts_with("repeating-");
    let prelude;
    let (mut direction, mut center, mut radii, mut from) =
        ((0f32, 0f32), (tw / 2., th / 2.), (0f32, 0f32), 0f32);
    let extent = match kind.trim_start_matches("repeating-") {
        "linear-gradient" => {
            let degrees = if let Some(side) = first.strip_prefix("to ") {
                prelude = true;
                let has = |name: &str| side.split_whitespace().any(|part| part == name);
                let corner = th.atan2(tw).to_degrees();
                match (has("top"), has("bottom"), has("left"), has("right")) {
                    (true, _, false, true) => corner,
                    (_, true, false, true) => 180. - corner,
                    (_, true, true, _) => 180. + corner,
                    (true, _, true, _) => 360. - corner,
                    (true, ..) => 0.,
                    (_, true, ..) => 180.,
                    (_, _, true, _) => 270.,
                    _ => 90.,
                }
            } else if let Some(degrees) = angle(first, &units) {
                prelude = true;
                degrees
            } else if space.is_some() && !first.is_empty() {
                return None;
            } else {
                prelude = space.is_some();
                180.
            };
            direction = (degrees.to_radians().sin(), -degrees.to_radians().cos());
            tw * direction.0.abs() + th * direction.1.abs()
        }
        "radial-gradient" => {
            prelude = space.is_some()
                || ["circle", "ellipse", "at ", "side", "corner"]
                    .iter()
                    .any(|word| first.contains(word))
                || first.starts_with("at")
                || !first.is_empty()
                    && css::value::color::color(first).is_none()
                    && first
                        .split_whitespace()
                        .all(|part| units.resolve(part, tw).is_some());
            let (shape, at) = if prelude {
                first
                    .split_once("at ")
                    .map_or((first, ""), |(shape, at)| (shape.trim(), at.trim()))
            } else {
                ("", "")
            };
            let [cx, cy, _] = origin(at, &units, [tw, th]);
            center = (cx, cy);
            let lengths: Vec<f32> = shape
                .split_whitespace()
                .filter_map(|part| units.resolve(part, tw))
                .collect();
            let circle =
                shape.contains("circle") || !shape.contains("ellipse") && lengths.len() == 1;
            let sides = [cx.abs(), (tw - cx).abs(), cy.abs(), (th - cy).abs()];
            let keyword = ["closest-side", "farthest-side", "closest-corner"]
                .into_iter()
                .find(|word| shape.contains(word));
            radii = match (lengths.as_slice(), keyword) {
                ([r], _) => (*r, *r),
                ([a, b, ..], _) => (*a, *b),
                (_, keyword) if circle => {
                    let corners = [
                        sides[0].hypot(sides[2]),
                        sides[0].hypot(sides[3]),
                        sides[1].hypot(sides[2]),
                        sides[1].hypot(sides[3]),
                    ];
                    let r = match keyword {
                        Some("closest-side") => sides.into_iter().fold(f32::MAX, f32::min),
                        Some("farthest-side") => sides.into_iter().fold(0., f32::max),
                        Some(_) => corners.into_iter().fold(f32::MAX, f32::min),
                        None => corners.into_iter().fold(0., f32::max),
                    };
                    (r, r)
                }
                (_, keyword) => {
                    let (x, y) = match keyword {
                        Some("closest-side") | Some("closest-corner") => {
                            (sides[0].min(sides[1]), sides[2].min(sides[3]))
                        }
                        _ => (sides[0].max(sides[1]), sides[2].max(sides[3])),
                    };
                    if keyword.is_some_and(|word| word.ends_with("side")) {
                        (x, y)
                    } else {
                        (x * std::f32::consts::SQRT_2, y * std::f32::consts::SQRT_2)
                    }
                }
            };
            radii.0.max(0.01)
        }
        "conic-gradient" => {
            prelude = space.is_some() || first.starts_with("from ") || first.starts_with("at ");
            if prelude {
                let (start, at) = first
                    .split_once("at ")
                    .map_or((first, ""), |(start, at)| (start, at.trim()));
                from = start
                    .trim()
                    .strip_prefix("from")
                    .and_then(|v| angle(v.trim(), &units))
                    .unwrap_or(0.);
                let [cx, cy, _] = origin(at, &units, [tw, th]);
                center = (cx, cy);
            }
            360.
        }
        _ => return None,
    };
    // Like Chrome on a wide-gamut screen, colors outside sRGB keep their gamut. Then stops are `css::value::color::wide_rgb`, and
    // the image is Display P3.
    let wide = cfg!(target_os = "macos")
        && (parts.iter()).any(|part| {
            css::value::tokens::components(part, ' ')
                .iter()
                .any(|token| css::value::color::wide(token))
        });
    let color = match wide {
        true => css::value::color::wide_rgb as fn(&str) -> _,
        false => css::value::color::color as fn(&str) -> _,
    };
    let css = |ink, alpha| stop_css(wide, ink, alpha);
    let stops = gradient_stops(&parts, prelude, &units, extent, wide)?;
    // Gradients mix premultiplied colors, and SVG mixes straight ones. So a transparent stop takes the color of
    // each neighbor, and other alpha changes get mixed stops.
    let mut offsets = Vec::with_capacity(stops.len() + 2);
    for (index, &(offset, (ink, alpha))) in stops.iter().enumerate() {
        if alpha > 0 {
            offsets.push((offset, (ink, alpha)));
            continue;
        }
        let [left, right] = [index.wrapping_sub(1), index + 1].map(|at| stops.get(at));
        for side in [left.or(right), right.or(left)] {
            offsets.push((offset, (side.map_or(ink, |stop| stop.1.0), 0)));
        }
    }
    let space = space.filter(|space| space != "srgb").or_else(|| {
        let modern = ["oklch(", "oklab(", "lab(", "lch(", "color("];
        modern
            .iter()
            .any(|name| body.contains(name))
            .then(|| "oklab".to_owned())
    });
    if space.is_some() || offsets.windows(2).any(|pair| pair[0].1.1 != pair[1].1.1) {
        let mut mixed = Vec::with_capacity(offsets.len() * 8);
        for pair in offsets.windows(2) {
            let [(a, (ink_a, alpha_a)), (b, (ink_b, alpha_b))] = [pair[0], pair[1]];
            mixed.push(pair[0]);
            let space = space.as_deref();
            // The longer way between two equal hues is all of them.
            let longer = space.is_some_and(|space| space.contains("longer"));
            if ink_a == ink_b && !longer || space.is_none() && alpha_a == alpha_b || a == b {
                continue;
            }
            let space = space.unwrap_or("srgb");
            let count = if longer { 24 } else { 8 };
            let [a_css, b_css] = [css(ink_a, alpha_a), css(ink_b, alpha_b)];
            for step in 1..count {
                let t = step as f32 / count as f32;
                let mix = format!(
                    "color-mix(in {space}, {a_css} {}%, {b_css})",
                    (1. - t) * 100.
                );
                if let Some(ink) = color(&mix) {
                    mixed.push((a + (b - a) * t, ink));
                }
            }
        }
        mixed.extend(offsets.last().copied());
        offsets = mixed;
    }
    let last = offsets.len() - 1;
    let start = if repeating && !kind.contains("radial") {
        offsets[0].0
    } else {
        0.
    };
    let period = if repeating {
        offsets[last].0 - start
    } else {
        extent
    };
    if period <= 0. {
        return None;
    }
    Some(Gradient {
        kind: kind.trim_start_matches("repeating-"),
        steps,
        tile: [tw, th],
        at: [x, y],
        repeating,
        direction,
        center,
        radii,
        from,
        extent,
        wide,
        offsets,
        start,
        period,
    })
}

/// The tile and its place in an area, as `tile` gives them, after `round` sized it, and the step from one tile to
/// the next on each axis, or `None` where it doesn't repeat.
pub(crate) fn tile_repeat(
    size: &str,
    position: &str,
    repeat: &str,
    units: &Lengths,
    width: f32,
    height: f32,
    natural: Option<[f32; 2]>,
) -> ([[f32; 2]; 2], [Option<f32>; 2]) {
    let words: Vec<&str> = repeat.split_whitespace().collect();
    let modes = match words.as_slice() {
        ["repeat-x"] => ["repeat", "no-repeat"],
        ["repeat-y"] => ["no-repeat", "repeat"],
        [one] => [*one; 2],
        [a, b, ..] => [*a, *b],
        [] => ["repeat"; 2],
    };
    let extent = [width, height];
    let [mut sides, mut at] = tile(size, position, units, width, height, natural);
    if modes.contains(&"round") {
        let side = |axis: usize| match modes[axis] {
            "round" => extent[axis] / (extent[axis] / sides[axis]).round().max(1.),
            _ => sides[axis],
        };
        let size = format!("{}px {}px", side(0), side(1));
        [sides, at] = tile(&size, position, units, width, height, natural);
    }
    let steps = std::array::from_fn(|axis| match modes[axis] {
        "no-repeat" => None,
        // Whole tiles touch both edges, with even gaps between them.
        "space" => {
            let count = (extent[axis] / sides[axis]).floor();
            (count >= 2.).then(|| {
                at[axis] = 0.;
                sides[axis] + (extent[axis] - count * sides[axis]) / (count - 1.)
            })
        }
        _ => Some(sides[axis]),
    });
    ([sides, at], steps)
}

/// SVG that repeats `content`, a tile of `size` at `at`, over the area by `steps`.
pub(crate) fn tiled(
    steps: [Option<f32>; 2],
    [tw, th]: [f32; 2],
    [x, y]: [f32; 2],
    area: [f32; 2],
    content: &str,
) -> String {
    // A pattern renders its tile, then samples it for each pixel. A few copies of the tile draw each pixel once.
    let starts = |step: Option<f32>, at: f32, extent: f32| match step {
        Some(step) if step > 0. => {
            let first = at - (at / step).ceil() * step;
            let count = ((extent - first) / step).ceil().max(0.) as usize;
            (count <= 3).then(|| (0..count).map(|n| first + n as f32 * step).collect())
        }
        Some(_) => None,
        None => Some(vec![at]),
    };
    if let (Some(xs), Some(ys)) = (starts(steps[0], x, area[0]), starts(steps[1], y, area[1])) {
        return ys
            .iter()
            .flat_map(|y| xs.iter().map(move |x| (x, y)))
            .map(|(x, y)| format!("<g transform=\"translate({x} {y})\">{content}</g>"))
            .collect();
    }
    let span = |step: Option<f32>, at: f32, side: f32| match step {
        Some(_) => ("0".to_owned(), "100%".to_owned()),
        None => (at.to_string(), side.to_string()),
    };
    let ((rx, rw), (ry, rh)) = (span(steps[0], x, tw), span(steps[1], y, th));
    format!(
        "<defs><pattern id=\"p\" patternUnits=\"userSpaceOnUse\" width=\"{}\" height=\"{}\" x=\"{x}\" y=\"{y}\">{content}</pattern></defs><rect x=\"{rx}\" y=\"{ry}\" width=\"{rw}\" height=\"{rh}\" fill=\"url(#p)\"/>",
        steps[0].unwrap_or(tw),
        steps[1].unwrap_or(th)
    )
}

pub(crate) fn push_bits(key: &mut String, size: [f32; 2], units: &Lengths) {
    let [font, root] = [units.font, units.root]
        .map(|font| [font.em, font.ch, font.ex, font.cap, font.ic, font.lh]);
    let container = units.container.unwrap_or([f32::NAN; 2]);
    for &value in size
        .iter()
        .chain(&font)
        .chain(&root)
        .chain(&units.viewport)
        .chain(&container)
    {
        let bits = value.to_bits();
        key.extend(
            (0..8)
                .rev()
                .map(|digit| char::from(b"0123456789abcdef"[(bits >> (digit * 4) & 15) as usize])),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::style;
    use super::*;

    #[test]
    fn gradient_masks_composite_along_their_axes() {
        let units = Lengths::new(16., 800., 600.);
        let mask = |image: &str, composite: Option<&str>, raster: [u32; 2]| {
            let mut props = vec![("mask-image", image)];
            props.extend(composite.map(|value| ("mask-composite", value)));
            axis_mask(&style(&props), &units, [100., 50.], raster)
        };
        let alpha = |bytes: &[u8], x: usize, y: usize| bytes[(y * 100 + x) * 4 + 3];
        let opaque = "linear-gradient(to left, black calc(100% - min(12px, 0px)), transparent 100%), linear-gradient(#fff, #fff)";
        assert_eq!(mask(opaque, Some("intersect"), [100, 50]), Some(None));
        let fades = "linear-gradient(to right, transparent, black 10px), linear-gradient(to top, transparent, black 10px)";
        let bytes = mask(fades, Some("intersect"), [100, 50]).unwrap().unwrap();
        assert!(alpha(&bytes, 0, 0) < 26 && alpha(&bytes, 50, 0) == 255);
        assert!(alpha(&bytes, 50, 49) < 26);
        let bytes = mask(fades, None, [100, 50]).unwrap().unwrap();
        assert!(alpha(&bytes, 0, 0) == 255 && alpha(&bytes, 0, 49) < 26);
        let halves =
            "linear-gradient(to right, black 50%, transparent 50%), linear-gradient(#000 0 0)";
        let bytes = mask(halves, Some("exclude"), [100, 50]).unwrap().unwrap();
        assert!(alpha(&bytes, 10, 10) == 0 && alpha(&bytes, 90, 10) == 255);
        let bytes = mask(halves, Some("subtract"), [100, 50]).unwrap().unwrap();
        assert!(bytes.iter().all(|byte| *byte == 0));
        assert!(
            mask(
                "linear-gradient(45deg, black, transparent)",
                None,
                [100, 50]
            )
            .is_none()
        );
    }

    #[test]
    fn positions_place_like_chrome() {
        let units = Lengths::new(16., 800., 600.);
        for (position, expected) in [
            ("left bottom", [0., 60.]),
            ("top", [50., 0.]),
            ("20% 0", [20., 0.]),
            ("right 5px top 10px", [95., 10.]),
            ("bottom 10px right", [100., 50.]),
            ("center bottom 1em", [50., 44.]),
        ] {
            assert_eq!(place(position, [100., 60.], &units), expected, "{position}");
        }
    }

    #[test]
    fn linear_gradients_along_an_axis_are_found() {
        for (kind, body, axis) in [
            ("linear-gradient", "red, blue", Some(1)),
            ("linear-gradient", "to right, red, blue", Some(0)),
            ("linear-gradient", "in oklch to left, red, blue", Some(0)),
            ("linear-gradient", "-90deg, red, blue", Some(0)),
            ("linear-gradient", "0.5turn in srgb, red, blue", Some(1)),
            (
                "repeating-linear-gradient",
                "to top, red, blue 9px",
                Some(1),
            ),
            ("linear-gradient", "calc(0.25 * 360deg), red, blue", Some(0)),
            ("linear-gradient", "45deg, red, blue", None),
            ("linear-gradient", "to bottom right, red, blue", None),
            ("radial-gradient", "red, blue", None),
        ] {
            let units = Lengths::new(16., 800., 600.);
            assert_eq!(gradient_axis(kind, body, &units), axis, "{kind}({body})");
        }
    }

    #[test]
    fn gradient_stops_take_math_angles() {
        let units = Lengths::new(16., 800., 600.);
        let offsets = |parts: &[&str]| {
            gradient_stops(parts, false, &units, 360., false)
                .unwrap()
                .iter()
                .map(|(offset, _)| offset.round())
                .collect::<Vec<_>>()
        };
        assert_eq!(offsets(&["red calc(0.25 * 360deg)", "blue 0"]), [90., 90.]);
        assert_eq!(
            offsets(&[
                "red calc(0.25 * 360deg - 1deg)",
                "blue calc(0.5turn + 1deg)"
            ]),
            [89., 181.]
        );
        assert_eq!(offsets(&["red 0", "blue 25%", "green"]), [0., 90., 360.]);
    }

    #[test]
    fn gradient_hints_move_the_midpoint_like_chrome() {
        let units = Lengths::new(16., 800., 600.);
        let stops = gradient_stops(&["red", "80%", "blue"], false, &units, 100., false).unwrap();
        // Chrome mixes half and half at the hint, and less blue before it: t ^ (ln 0.5 / ln 0.8).
        let at = |offset: f32| {
            stops
                .iter()
                .find(|(at, _)| (at - offset).abs() < 0.01)
                .unwrap()
                .1
        };
        assert_eq!(at(0.), (0xff0000, 255));
        assert_eq!(at(50.), (0xe1001e, 255));
        assert_eq!(at(100.), (0x0000ff, 255));
    }

    #[test]
    fn conic_images_draw_like_their_svg() {
        let units = Lengths::new(16., 800., 600.);
        for (kind, body, size, position, repeat) in [
            (
                "conic-gradient",
                "red, yellow, blue",
                "",
                "0% 0%",
                "no-repeat",
            ),
            (
                "conic-gradient",
                "from 45deg at 30% 40%, red, #0f08, blue",
                "",
                "0% 0%",
                "no-repeat",
            ),
            (
                "conic-gradient",
                "#f00 0 90deg, #00f 90deg 200deg, #0f0 0",
                "",
                "0% 0%",
                "no-repeat",
            ),
            (
                "repeating-conic-gradient",
                "#000 0 15deg, #fff 0 30deg",
                "",
                "0% 0%",
                "no-repeat",
            ),
            (
                "conic-gradient",
                "red, blue",
                "25px 20px",
                "5px 5px",
                "repeat",
            ),
            (
                "conic-gradient",
                "red, blue",
                "25px 20px",
                "5px 5px",
                "repeat-x",
            ),
        ] {
            let area = [0., 0., 60., 40.];
            let image = conic_image(
                kind,
                body,
                size,
                position,
                repeat,
                units,
                [60., 40.],
                area,
                [120, 80],
                None,
            )
            .unwrap();
            let svg =
                gradient_svg(kind, body, size, position, repeat, units, 60., 40., area).unwrap();
            let svg = super::super::super::raster::svg::svg_gradient(svg, 120, 80).unwrap();
            let far = (image.bytes().as_chunks::<4>().0.iter())
                .zip(svg.bytes().as_chunks::<4>().0)
                .filter(|(a, b)| a.iter().zip(*b).any(|(a, b)| a.abs_diff(*b) > 24))
                .count();
            // The SVG draws wedges of 2deg at most, so pixels differ only along hard stops.
            assert!(far < 120 * 80 / 100, "{body}: {far} pixels differ");
        }
    }

    #[test]
    fn conic_images_draw_again_only_when_their_pixels_change() {
        let units = Lengths::new(16., 800., 600.);
        let draw = |turn: f32, previous| {
            let body = format!("red {turn}turn, transparent 0");
            let area = [0., 0., 100., 100.];
            conic_image(
                "conic-gradient",
                &body,
                "",
                "0% 0%",
                "",
                units,
                [100., 100.],
                area,
                [200, 200],
                previous,
            )
            .unwrap()
        };
        let first = draw(0.3, None);
        let still = draw(0.30001, Some(&first));
        assert!(
            std::sync::Arc::ptr_eq(&first, &still),
            "a move within a table step keeps the image"
        );
        let moved = draw(0.31, Some(&first));
        assert!(
            !std::sync::Arc::ptr_eq(&first, &moved),
            "a visible move draws again"
        );
    }

    #[test]
    fn gradient_interpolation_spaces_parse_in_any_order() {
        let units = Lengths::new(16., 800., 600.);
        for (body, valid) in [
            ("hue in hsl, red, blue", false),
            ("in hsl longer hue, red, blue", true),
            ("to right in oklch longer hue, red, blue", true),
        ] {
            let svg = gradient_svg(
                "linear-gradient",
                body,
                "auto",
                "",
                "",
                units,
                10.,
                10.,
                [0., 0., 10., 10.],
            );
            assert_eq!(svg.is_some(), valid, "{body}");
        }
        for kind in ["radial-gradient", "conic-gradient"] {
            for body in ["hue in hsl, red, blue", "in, red, blue"] {
                gradient_svg(
                    kind,
                    body,
                    "auto",
                    "",
                    "",
                    units,
                    10.,
                    10.,
                    [0., 0., 10., 10.],
                );
            }
        }
    }
}
