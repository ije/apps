use super::{Ids, clip, establishes, index};
use crate::css::{
    properties::Prop, properties::Style, value::shape, value::shape::Op, value::shape::Shape,
    value::shape::Shaped, value::shape::Track, value::tokens::components, value::units::Lengths,
};
use crate::dom::Document;
use crate::render::PageState;
use renderer::{Pixels, Rect, Window, point, px};

/// Whether the place of a box on its `offset-path` depends on its containing block or on an SVG element.
pub(crate) fn reads_page(style: &Style) -> bool {
    style.get(Prop::OFFSET_PATH).is_some_and(|value| {
        value != "none"
            && !value
                .get(..5)
                .is_some_and(|start| start.eq_ignore_ascii_case("path("))
    })
}

/// The style with the place of the box on its path as a `path()` that starts there.
#[allow(clippy::too_many_arguments)]
pub(crate) fn placed(
    id: usize,
    style: &Style,
    units: &Lengths,
    bounds: Rect<Pixels>,
    doc: &Document,
    state: &PageState,
    ids: &Ids,
    window: &Window,
) -> Option<Style> {
    let value = style.get(Prop::OFFSET_PATH)?;
    let distance = style.get(Prop::OFFSET_DISTANCE).unwrap_or("0px");
    let (point, tangent) = match Shaped::parse(value) {
        Some(Shaped { shape, reference }) => {
            let (size, at, block) = container(id, style, bounds, doc, state, window)?;
            let styles = state.styles.last()?;
            let edges = match block {
                Some(block) => clip::insets(
                    reference,
                    &styles.values[block],
                    &styles.lengths[block],
                    size,
                ),
                None => [0.; 4],
            };
            let inner = [
                (size[0] - edges[1] - edges[3]).max(0.),
                (size[1] - edges[0] - edges[2]).max(0.),
            ];
            let at = [at[0] - edges[3], at[1] - edges[0]];
            let start = match style.get(Prop::OFFSET_POSITION).unwrap_or("normal") {
                "normal" => [inner[0] / 2., inner[1] / 2.],
                "auto" => at,
                value => shape::Position::parse(&components(value, ' '))?.point(units, inner)?,
            };
            let (point, tangent) = match shape {
                Some(Shape::Ray(ray)) => {
                    let start = match &ray.at {
                        Some(at) => at.point(units, inner)?,
                        None => start,
                    };
                    let angle = units.math(ray.angle, 0., &[])?.0 as f32;
                    let (sin, cos) = angle.to_radians().sin_cos();
                    let reach = length(ray.size, start, inner, [sin, -cos]);
                    let reach = match ray.contain {
                        true => (reach - f32::from(bounds.size.width.max(bounds.size.height)) / 2.)
                            .max(0.),
                        false => reach,
                    };
                    let along = units.resolve(distance, reach)?;
                    (
                        [start[0] + sin * along, start[1] - cos * along],
                        angle - 90.,
                    )
                }
                Some(shape) => {
                    let mut reads = String::new();
                    super::gradient::push_bits(&mut reads, inner, units);
                    reads.push_str(&format!("{start:?}"));
                    follow(distance, units, value, &reads, || {
                        shape.outline(units, inner, Some(start))
                    })?
                }
                None => {
                    let radii = match block {
                        Some(block) => clip::edge_radii(
                            None,
                            &styles.values[block],
                            &styles.lengths[block],
                            size,
                        ),
                        None => [[0.; 2]; 4],
                    };
                    let outline = shape::rounded([0., 0., inner[0], inner[1]], radii);
                    let track = Track::new(&outline);
                    track.point(units.resolve(distance, track.length())?)
                }
            };
            ([point[0] - at[0], point[1] - at[1]], tangent)
        }
        None => {
            let url = components(value, ' ').into_iter().next()?;
            let id = url.strip_suffix(')')?.split_once('(')?.1;
            let id = id.trim().trim_matches(['"', '\'']).strip_prefix('#')?;
            let node = (index(ids, doc).get(id)).and_then(|nodes| nodes.first().copied());
            // Like Chrome, a missing element is an empty path.
            let outline = || node.and_then(|node| element(doc, node)).unwrap_or_default();
            match node.and_then(|node| doc.nodes[node].attrs.get("d")) {
                Some(data) => follow(distance, units, data, "d", || Some(outline()))?,
                None => {
                    let track = Track::new(&outline());
                    track.point(units.resolve(distance, track.length())?)
                }
            }
        }
    };
    let mut style = style.clone();
    let (sin, cos) = tangent.to_radians().sin_cos();
    let [x, y] = point;
    style.insert(
        Prop::OFFSET_PATH,
        format!("path(\"M{x} {y}l{} {}\")", cos * 100., sin * 100.),
    );
    style.remove(Prop::OFFSET_DISTANCE);
    Some(style)
}

fn follow(
    distance: &str,
    units: &Lengths,
    text: &str,
    reads: &str,
    outline: impl FnOnce() -> Option<Vec<Op>>,
) -> Option<([f32; 2], f32)> {
    let track = shape::track(text, reads, outline)?;
    Some(track.point(units.resolve(distance, track.length())?))
}

// The border box of the containing block: its size, the place of the box in it, and its node when it isn't the
// viewport.
fn container(
    id: usize,
    style: &Style,
    bounds: Rect<Pixels>,
    doc: &Document,
    state: &PageState,
    window: &Window,
) -> Option<([f32; 2], [f32; 2], Option<usize>)> {
    let styles = state.styles.last()?;
    let mut up = state.styles.walk.parent(id).map(|node| node as usize);
    let position = style.get(Prop::POSITION).unwrap_or("static");
    while let Some(node) = up {
        let style = styles.values.get(node)?;
        let found = !doc.nodes[node].tag.starts_with('#')
            && match position {
                "fixed" => establishes(style).fixed,
                "absolute" => establishes(style).absolute,
                _ => !matches!(
                    style.get(Prop::DISPLAY).unwrap_or("inline"),
                    "inline" | "contents"
                ),
            };
        if found {
            break;
        }
        up = (node != 0)
            .then(|| state.styles.walk.parent(node).map(|node| node as usize))
            .flatten();
    }
    let map = |x: f32, y: f32| {
        let at = window.transformed_bounds(Rect::new(point(px(x), px(y)), Default::default()));
        [f32::from(at.origin.x), f32::from(at.origin.y)]
    };
    let own = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
    let center = bounds.center();
    let [cx, cy] = [f32::from(center.x), f32::from(center.y)];
    let here = map(cx, cy);
    let [x, y, width, height] = match up {
        Some(node) => *state.geometry.mirror.get(&node)?,
        None => {
            let viewport = window.viewport_size();
            [
                0.,
                0.,
                f32::from(viewport.width),
                f32::from(viewport.height),
            ]
        }
    };
    // The bounds of a transformed box keep its center, so the centers give the distance in the space of the box.
    let size = match up {
        Some(node) if state.geometry.mirror.is_transformed(node) => {
            let (style, units) = (&styles.values[node], &styles.lengths[node]);
            let inner = state.layout.content_size(node)?;
            let sides = clip::insets(Some("content-box"), style, units, inner);
            [
                inner[0] + sides[1] + sides[3],
                inner[1] + sides[0] + sides[2],
            ]
        }
        _ => [width, height],
    };
    let away = [here[0] - x - width / 2., here[1] - y - height / 2.];
    let [a, b] =
        [map(cx + 1., cy), map(cx, cy + 1.)].map(|end| [end[0] - here[0], end[1] - here[1]]);
    let det = a[0] * b[1] - a[1] * b[0];
    if det.abs() < 1e-6 {
        return None;
    }
    let away = [
        (away[0] * b[1] - away[1] * b[0]) / det,
        (away[1] * a[0] - away[0] * a[1]) / det,
    ];
    let scrolled = (up.and_then(|node| state.geometry.scroll.get(&node)))
        .map_or_else(Default::default, |handle| handle.offset());
    let scrolled = [f32::from(scrolled.x), f32::from(scrolled.y)];
    let at =
        std::array::from_fn(|axis| size[axis] / 2. + away[axis] - own[axis] / 2. - scrolled[axis]);
    Some((size, at, up))
}

// The length of a ray from `start` in a box.
fn length(size: &str, [x, y]: [f32; 2], [width, height]: [f32; 2], [dx, dy]: [f32; 2]) -> f32 {
    let sides = [x.abs(), (width - x).abs(), y.abs(), (height - y).abs()];
    let corners = [[0., 0.], [width, 0.], [0., height], [width, height]]
        .map(|corner| (corner[0] - x).hypot(corner[1] - y));
    let least = |values: [f32; 4]| values.into_iter().fold(f32::MAX, f32::min);
    let most = |values: [f32; 4]| values.into_iter().fold(0., f32::max);
    match size {
        "farthest-side" => most(sides),
        "closest-corner" => least(corners),
        "farthest-corner" => most(corners),
        "sides" => {
            if x <= 0. || y <= 0. || x >= width || y >= height {
                return 0.;
            }
            let edge = |at: f32, extent: f32, d: f32| match d {
                d if d > 1e-6 => (extent - at) / d,
                d if d < -1e-6 => -at / d,
                _ => f32::MAX,
            };
            edge(x, width, dx).min(edge(y, height, dy))
        }
        _ => least(sides),
    }
}

// The outline of an SVG shape in its user space.
fn element(doc: &Document, node: usize) -> Option<Vec<Op>> {
    let node = &doc.nodes[node];
    let number = |name: &str| {
        (node.attrs.get(name))
            .and_then(|value| value.trim().trim_end_matches("px").parse::<f32>().ok())
            .unwrap_or(0.)
    };
    let units = Lengths::new(16., 0., 0.);
    let data = match node.tag.to_ascii_lowercase().as_str() {
        "path" => node.attrs.get("d")?.clone(),
        "line" => format!(
            "M{} {}L{} {}",
            number("x1"),
            number("y1"),
            number("x2"),
            number("y2")
        ),
        "polyline" => format!("M{}", node.attrs.get("points")?),
        "polygon" => format!("M{}Z", node.attrs.get("points")?),
        "rect" => {
            let (rx, ry) = (number("rx"), number("ry"));
            let radius = [
                if rx > 0. { rx } else { ry }.min(number("width") / 2.),
                if ry > 0. { ry } else { rx }.min(number("height") / 2.),
            ];
            return Some(shape::rounded(
                [number("x"), number("y"), number("width"), number("height")],
                [radius; 4],
            ));
        }
        tag @ ("circle" | "ellipse") => {
            let (rx, ry) = match tag {
                "circle" => (number("r"), number("r")),
                _ => (number("rx"), number("ry")),
            };
            let (x, y) = (number("cx"), number("cy"));
            return Shape::parse(&format!("ellipse({rx}px {ry}px at {x}px {y}px)"))?
                .outline(&units, [0.; 2], None);
        }
        _ => return None,
    };
    Shape::parse(&format!("path('{data}')"))?.outline(&units, [0.; 2], None)
}
