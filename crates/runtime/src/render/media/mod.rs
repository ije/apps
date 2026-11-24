pub mod gpu;
pub mod player;
pub mod terminal;

use anyhow::ensure;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
};

pub(crate) type Decoded = anyhow::Result<Result<renderer::RenderImage, (String, [f32; 2])>>;

#[derive(Clone)]
pub(crate) struct Placeholder {
    pub(crate) frames: Frames,
    pub(crate) node: usize,
    pub(crate) wake: crate::jsc::Waker,
}

pub(crate) type Frames = Arc<Mutex<HashMap<usize, Arc<renderer::RenderImage>>>>;

impl Placeholder {
    pub(crate) fn show(&self, image: Arc<renderer::RenderImage>) {
        self.frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(self.node, image);
        self.wake.wake();
    }
}

// Retain SVG sources used as pointer keys to prevent address reuse.
pub(crate) type CanvasSource = (
    (u64, usize, [u32; 2]),
    Option<Rc<str>>,
    Rc<tiny_skia::Pixmap>,
);

#[derive(Default)]
pub struct Media {
    pub canvases: rustc_hash::FxHashMap<usize, crate::render::raster::canvas::Canvas>,
    pub(crate) terminals: Rc<std::cell::RefCell<terminal::Terminals>>,
    pub(crate) sources: std::cell::RefCell<rustc_hash::FxHashMap<usize, CanvasSource>>,
    pub images: rustc_hash::FxHashMap<usize, Arc<renderer::RenderImage>>,
    pub retired: Vec<Arc<renderer::RenderImage>>,
    pub(crate) retired_canvases: HashSet<usize>,
    pub(crate) recycled: Vec<(usize, Arc<renderer::RenderImage>)>,
    pub(crate) decoded: Arc<Mutex<Vec<(usize, u64, Decoded)>>>,
    pub(crate) frames: Frames,
    pub(crate) placeholders: rustc_hash::FxHashMap<usize, Placeholder>,
    pub(crate) decoding: rustc_hash::FxHashMap<usize, u64>,
    pub(crate) svg_sources: rustc_hash::FxHashMap<usize, Rc<str>>,
    pub(crate) svg_images: rustc_hash::FxHashMap<usize, (Rc<str>, [f32; 2])>,
    // The last drawing of each drawable element of a canvas, with its device pixels per CSS pixel.
    pub(crate) snapshots: rustc_hash::FxHashMap<usize, (Rc<tiny_skia::Pixmap>, f32)>,
}

impl Media {
    // Only the first replaced image can be on the GPU. Drop later images at once to bound memory in unrendered
    // windows.
    pub(crate) fn retire(&mut self, id: usize, old: Option<Arc<renderer::RenderImage>>) {
        if self.retired.is_empty() {
            self.retired_canvases.clear();
        }
        if let Some(old) = old
            && self.retired_canvases.insert(id)
        {
            self.retired.push(old);
        }
    }

    pub(crate) fn forget(&mut self, id: usize) {
        self.retired.extend(self.images.remove(&id));
        self.retired.extend(
            self.canvases
                .remove(&id)
                .filter(|canvas| canvas.present)
                .map(|canvas| canvas.image),
        );
        self.placeholders.remove(&id);
        self.snapshots.remove(&id);
        self.svg_sources.remove(&id);
        self.svg_images.remove(&id);
        self.sources.get_mut().remove(&id);
        self.decoding.remove(&id);
    }

    pub(crate) fn source_pixels(
        &self,
        id: usize,
        size: Option<[u32; 2]>,
    ) -> Option<crate::render::raster::canvas::Source<'_>> {
        use crate::render::raster::{self, canvas::Source};
        if let Some(canvas) = self.canvases.get(&id) {
            return Some(Source::Pixels(&canvas.pixels));
        }
        let image = self.images.get(&id);
        let svg = self.svg_images.get(&id).filter(|_| image.is_none());
        let size = svg.map_or([0; 2], |(_, natural)| {
            size.unwrap_or(natural.map(|side| side.ceil() as u32))
        });
        let key = (
            image.map_or(0, |image| image.id),
            svg.map_or(0, |(source, _)| Rc::as_ptr(source) as *const u8 as usize),
            size,
        );
        let mut cache = self.sources.try_borrow_mut().ok()?;
        let fresh = match cache.get(&id) {
            Some((old, _, pixels)) if *old == key => return Some(Source::Shared(pixels.clone())),
            Some(_) => {
                cache.remove(&id);
                false
            }
            None => true,
        };
        let pixels = Rc::new(match (image, svg) {
            (Some(image), _) => raster::pixmap(image)?,
            (_, Some((source, _))) => raster::svg::svg_pixels(
                &raster::svg::svg_tree(source, false).ok()?,
                size[0],
                size[1],
                None,
            )
            .ok()?,
            _ => return None,
        });
        if fresh {
            let kept: usize = cache.values().map(|(.., pixels)| pixels.data().len()).sum();
            if kept + pixels.data().len() > 64 << 20 {
                cache.clear();
            }
            cache.insert(
                id,
                (key, svg.map(|(source, _)| source.clone()), pixels.clone()),
            );
        }
        Some(Source::Shared(pixels))
    }

    pub(crate) fn load_image(
        &mut self,
        id: usize,
        read: impl FnOnce() -> anyhow::Result<Vec<u8>> + Send + 'static,
    ) -> Value {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let request = NEXT.fetch_add(1, Ordering::Relaxed);
        self.decoding.insert(id, request);
        let (decoded, wake) = (self.decoded.clone(), crate::jsc::Waker::current());
        crate::render::raster::image::decode_later(Box::new(move || {
            let result = read().and_then(|bytes| {
                ensure!(bytes.len() <= 64 * 1024 * 1024, "Image is too large");
                crate::render::raster::image::decode(&bytes)
                    .map(Ok)
                    .or_else(|error| {
                        let options = crate::render::raster::svg::svg_options(false, false);
                        let size = usvg::Tree::from_data(&bytes, &options)
                            .map_err(|_| error)?
                            .size();
                        let text = if bytes.starts_with(&[0x1f, 0x8b]) {
                            usvg::decompress_svgz(&bytes)?
                        } else {
                            bytes
                        };
                        Ok(Err((
                            String::from_utf8_lossy(&text).into_owned(),
                            [size.width(), size.height()],
                        )))
                    })
            });
            decoded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((id, request, result));
            wake.wake();
        }));
        json!(request)
    }
}
