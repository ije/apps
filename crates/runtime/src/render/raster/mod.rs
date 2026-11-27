pub mod canvas;
pub mod image;
pub mod resvg;
pub mod svg;

use std::{collections::HashMap, rc::Rc, sync::Arc};
use tiny_skia as skia;

pub const RASTER_MAX: u32 = 4096;
// A new buffer costs page faults, about 0.5 ms at 1080p, so `spare` is used when it has the right size.
pub(crate) fn render_image(pixels: &skia::Pixmap, spare: Vec<u8>) -> Arc<renderer::RenderImage> {
  let mut bytes = match spare.len() == pixels.data().len() {
    true => spare,
    false => vec![0; pixels.data().len()],
  };
  for (out, &[r, g, b, a]) in bytes
    .as_chunks_mut::<4>()
    .0
    .iter_mut()
    .zip(pixels.data().as_chunks::<4>().0)
  {
    *out = [b, g, r, a];
  }
  Arc::new(renderer::RenderImage::new_premultiplied(
    pixels.width(),
    pixels.height(),
    bytes,
  ))
}

pub(crate) fn pixmap(image: &renderer::RenderImage) -> Option<skia::Pixmap> {
  let size = image.size();
  let mut pixels = skia::Pixmap::new(size.width as u32, size.height as u32)?;
  let source = image.bytes().as_chunks::<4>().0;
  if image.is_premultiplied() {
    for (out, &[b, g, r, a]) in pixels
      .data_mut()
      .as_chunks_mut::<4>()
      .0
      .iter_mut()
      .zip(source)
    {
      *out = [r, g, b, a];
    }
  } else {
    for (out, pixel) in pixels.pixels_mut().iter_mut().zip(source) {
      *out = skia::ColorU8::from_rgba(pixel[2], pixel[1], pixel[0], pixel[3]).premultiply();
    }
  }
  Some(pixels)
}

pub(crate) type SvgBoxes = (
  Rc<str>,
  Vec<(String, usize)>,
  [f32; 2],
  Vec<(usize, [f32; 4], [f32; 4])>,
  [f32; 4],
  [f32; 4],
  [f32; 4],
);

// The mask of a box, 0, or of its `::before` or `::after`, 1 and 2: its inputs and its image.
pub(crate) type Mask = (String, Option<Arc<renderer::RenderImage>>);

#[derive(Default)]
pub struct Rasters {
  pub(in crate::render) svg_boxes: rustc_hash::FxHashMap<usize, SvgBoxes>,
  pub(in crate::render) svg_trees: rustc_hash::FxHashMap<usize, usvg::Tree>,
  pub(in crate::render) masks: rustc_hash::FxHashMap<(usize, u8), Mask>,
  pub(in crate::render) svgs: rustc_hash::FxHashMap<usize, (Rc<str>, Arc<renderer::RenderImage>)>,
  pub(in crate::render) svg_cache:
    HashMap<Rc<str>, (svg::SvgGeometry, Vec<Arc<renderer::RenderImage>>)>,
  pub(in crate::render) backgrounds: HashMap<(usize, usize), (String, Arc<renderer::RenderImage>)>,
  pub(in crate::render) bitmaps: HashMap<String, Arc<renderer::RenderImage>>,
  // The decoded source of each background layer, kept while the layer keeps its raster, so `retire` keeps it.
  pub(in crate::render) layer_sources: HashMap<(usize, usize), Arc<renderer::RenderImage>>,
  pub(in crate::render) gradients: Vec<(String, Arc<renderer::RenderImage>)>,
}

impl Rasters {
  // The boxes of the elements of an inline `<svg>`.
  pub(crate) fn svg_boxes(&self, id: usize) -> Option<&SvgBoxes> {
    self.svg_boxes.get(&id)
  }

  pub(crate) fn forget(&mut self, id: usize, retired: &mut Vec<Arc<renderer::RenderImage>>) {
    retired.extend(
      self
        .svgs
        .remove(&id)
        .map(|(_, image)| image)
        .filter(|image| Arc::strong_count(image) == 1),
    );
    for slot in 0..3 {
      retired.extend(
        self
          .masks
          .remove(&(id, slot))
          .and_then(|(_, image)| image)
          .filter(|image| Arc::strong_count(image) == 1),
      );
    }
    self.svg_boxes.remove(&id);
    self.svg_trees.remove(&id);
  }

  pub(crate) fn forget_all(
    &mut self,
    gone: &rustc_hash::FxHashSet<usize>,
    retired: &mut Vec<Arc<renderer::RenderImage>>,
  ) {
    self.backgrounds.retain(|(node, _), (_, image)| {
      let keep = !gone.contains(node);
      if !keep && Arc::strong_count(image) == 1 {
        retired.push(image.clone());
      }
      keep
    });
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn removed_backgrounds_retire_only_images_that_no_box_shares() {
    let image = || Arc::new(renderer::RenderImage::new_premultiplied(1, 1, vec![0; 4]));
    let (shared, own) = (image(), image());
    let mut rasters = Rasters::default();
    rasters
      .backgrounds
      .insert((1, 0), (String::new(), shared.clone()));
    rasters
      .backgrounds
      .insert((1, 1), (String::new(), own.clone()));
    rasters
      .backgrounds
      .insert((2, 0), (String::new(), shared.clone()));
    drop(own);
    let mut retired = Vec::new();
    rasters.forget_all(&[1].into_iter().collect(), &mut retired);
    assert_eq!(rasters.backgrounds.len(), 1);
    assert_eq!(retired.len(), 1);
    assert!(!Arc::ptr_eq(&retired[0], &shared));
  }
}
