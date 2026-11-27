use super::{RASTER_MAX, render_image};
use anyhow::{Context, Result};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use tiny_skia as skia;

pub fn fonts(system: bool) -> MutexGuard<'static, (Arc<usvg::fontdb::Database>, bool)> {
  static FONTS: LazyLock<Mutex<(Arc<usvg::fontdb::Database>, bool)>> =
    LazyLock::new(Mutex::default);
  let mut fonts = FONTS.lock().unwrap();
  if system && !fonts.1 {
    Arc::make_mut(&mut fonts.0).load_system_fonts();
    fonts.1 = true;
  }
  fonts
}

pub fn svg(source: &str, width: u32, height: u32) -> Result<Arc<renderer::RenderImage>> {
  render_svg(&svg_tree(source)?, width, height, None)
}

// The image of `effects::gradient::gradient_svg`.
pub fn svg_gradient(
  (source, p3): (String, bool),
  width: u32,
  height: u32,
) -> Option<Arc<renderer::RenderImage>> {
  let image = svg(&source, width, height).ok()?;
  match p3 {
    true => Some(Arc::new(Arc::into_inner(image)?.display_p3())),
    false => Some(image),
  }
}

// usvg reads any file that an SVG names in an image. Page content must not reach files this way: a canvas
// hands their pixels to the page, and a UNC path sends the user's credentials to its host.
pub fn svg_options(text: bool) -> usvg::Options<'static> {
  let mut options = usvg::Options {
    fontdb: fonts(text).0.clone(),
    ..Default::default()
  };
  options.image_href_resolver.resolve_string = Box::new(|_, _| None);
  options
}

pub fn svg_tree(source: &str) -> Result<usvg::Tree> {
  Ok(usvg::Tree::from_str(
    source,
    &svg_options(source.contains("<text")),
  )?)
}

pub fn render_svg(
  tree: &usvg::Tree,
  width: u32,
  height: u32,
  transform: Option<([f32; 2], [f32; 6])>,
) -> Result<Arc<renderer::RenderImage>> {
  Ok(render_image(
    &svg_pixels(tree, width, height, transform)?,
    Vec::new(),
  ))
}

// Route all SVG rasterization here to enforce RASTER_MAX. Keep transforms in requested-size pixels.
pub fn svg_pixels(
  tree: &usvg::Tree,
  width: u32,
  height: u32,
  transform: Option<([f32; 2], [f32; 6])>,
) -> Result<skia::Pixmap> {
  let [width, height] = [width.max(1), height.max(1)];
  let [w, h] = [width.min(RASTER_MAX), height.min(RASTER_MAX)];
  let mut pixels = skia::Pixmap::new(w, h).context("Cannot allocate SVG image")?;
  let size = tree.size();
  let ([fit_width, fit_height], [a, b, c, d, e, f]) =
    transform.unwrap_or(([width as f32, height as f32], [1., 0., 0., 1., 0., 0.]));
  super::resvg::render(
    tree,
    skia::Transform::from_scale(w as f32 / width as f32, h as f32 / height as f32)
      .pre_concat(skia::Transform::from_row(a, b, c, d, e, f))
      .pre_scale(fit_width / size.width(), fit_height / size.height()),
    &mut pixels.as_mut(),
  );
  Ok(pixels)
}

/// The size, the content box, the element boxes and the painted area of an SVG tree.
pub type SvgGeometry = (
  [f32; 2],
  [f32; 4],
  Vec<(String, [f32; 4], [f32; 4])>,
  [f32; 4],
);

pub fn svg_boxes(tree: &usvg::Tree) -> SvgGeometry {
  let rect = |rect: usvg::Rect| [rect.x(), rect.y(), rect.width(), rect.height()];
  let mut boxes = Vec::new();
  // usvg applies the transform of a `<use>` twice in `abs_transform`, so canvas boxes come from the
  // relative transforms.
  let root = tree.root().abs_transform();
  let mut pending: Vec<_> = tree
    .root()
    .children()
    .iter()
    .map(|node| (node, root))
    .collect();
  while let Some((node, transform)) = pending.pop() {
    let transform = match node {
      usvg::Node::Group(group) => transform.pre_concat(group.transform()),
      _ => transform,
    };
    if !node.id().is_empty() {
      boxes.push((
        node.id().to_owned(),
        rect(
          node
            .bounding_box()
            .transform(transform)
            .unwrap_or(node.abs_bounding_box()),
        ),
        rect(node.bounding_box()),
      ));
    }
    if let usvg::Node::Group(group) = node {
      pending.extend(group.children().iter().map(|child| (child, transform)));
    }
  }
  let content = match tree.root().children() {
    [usvg::Node::Group(group)] if group.id().is_empty() => group.bounding_box(),
    _ => tree.root().bounding_box(),
  };
  let painted = tree.root().abs_layer_bounding_box();
  (
    [tree.size().width(), tree.size().height()],
    rect(content),
    boxes,
    [painted.x(), painted.y(), painted.width(), painted.height()],
  )
}

pub fn svg_shape(source: &str) -> Option<([f32; 4], f32)> {
  type Shapes = std::collections::HashMap<String, Option<([f32; 4], f32)>>;
  thread_local! {
      static SHAPES: std::cell::RefCell<Shapes> = Default::default();
  }
  if let Some(shape) = SHAPES.with_borrow(|shapes| shapes.get(source).copied()) {
    return shape;
  }
  let shape = svg_tree(source).ok().and_then(|tree| {
    let node = tree.node_by_id("apps-shape")?;
    let rect = node.bounding_box();
    let length = match node {
      usvg::Node::Path(path) => {
        let (mut length, mut start, mut last) = (0., skia::Point::zero(), skia::Point::zero());
        let mut add = |from: skia::Point, to: skia::Point| {
          length += ((to.x - from.x).powi(2) + (to.y - from.y).powi(2)).sqrt()
        };
        for segment in path.data().segments() {
          match segment {
            skia::PathSegment::MoveTo(point) => (start, last) = (point, point),
            skia::PathSegment::LineTo(point) => {
              add(last, point);
              last = point;
            }
            skia::PathSegment::QuadTo(control, point) => {
              let mut from = last;
              for step in 1..=16 {
                let t = step as f32 / 16.;
                let u = 1. - t;
                let to = skia::Point::from_xy(
                  u * u * last.x + 2. * u * t * control.x + t * t * point.x,
                  u * u * last.y + 2. * u * t * control.y + t * t * point.y,
                );
                add(from, to);
                from = to;
              }
              last = point;
            }
            skia::PathSegment::CubicTo(first, second, point) => {
              let mut from = last;
              for step in 1..=16 {
                let t = step as f32 / 16.;
                let u = 1. - t;
                let [a, b, c, d] = [u * u * u, 3. * u * u * t, 3. * u * t * t, t * t * t];
                let to = skia::Point::from_xy(
                  a * last.x + b * first.x + c * second.x + d * point.x,
                  a * last.y + b * first.y + c * second.y + d * point.y,
                );
                add(from, to);
                from = to;
              }
              last = point;
            }
            skia::PathSegment::Close => {
              add(last, start);
              last = start;
            }
          }
        }
        length
      }
      _ => 0.,
    };
    Some(([rect.x(), rect.y(), rect.width(), rect.height()], length))
  });
  SHAPES.with_borrow_mut(|shapes| {
    if shapes.len() >= 256 {
      shapes.clear();
    }
    shapes.insert(source.to_owned(), shape);
  });
  shape
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn svg_boxes_map_elements_to_canvas_and_own_coordinates() {
    let (size, content, boxes, _) = svg_boxes(&svg_tree(r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 100 50"><rect id="r" x="10" y="5" width="30" height="20"/><g transform="translate(5 30)"><rect id="apps-7" width="10" height="10"/></g></svg>"##).unwrap());
    assert_eq!(size, [200., 100.]);
    assert_eq!(content, [5., 5., 35., 35.]);
    let find = |name: &str| {
      boxes
        .iter()
        .find(|(id, ..)| id == name)
        .map(|(_, canvas, object)| (*canvas, *object))
    };
    assert_eq!(find("r"), Some(([20., 10., 60., 40.], [10., 5., 30., 20.])));
    assert_eq!(
      find("apps-7"),
      Some(([10., 60., 20., 20.], [0., 0., 10., 10.]))
    );
    let (_, _, boxes, _) = svg_boxes(&svg_tree(r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><rect id="s" width="10" height="10"/></defs><use id="u" href="#s" x="20" y="10" transform="scale(2)"/></svg>"##).unwrap());
    assert_eq!(
      boxes
        .iter()
        .find(|(id, ..)| id == "u")
        .map(|(_, canvas, _)| *canvas),
      Some([40., 20., 20., 20.])
    );
  }

  #[test]
  fn svg_filters_past_their_layer_draw() {
    let tree = svg_tree(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200"><filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="200" height="2000"><feComposite in="SourceGraphic" in2="SourceGraphic" operator="arithmetic" k2="1"/></filter><rect x="20" y="20" width="160" height="160" fill="#2e8b57" filter="url(#f)"/></svg>"##).unwrap();
    let pixels = svg_pixels(&tree, 200, 200, None).unwrap();
    assert_eq!(pixels.pixel(100, 100).unwrap().alpha(), 255);
  }
}
