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
    pub canvases: rustc_hash::FxHashMap<usize, crate::render::canvas::Canvas>,
    pub(crate) terminals: Rc<std::cell::RefCell<crate::web::terminal::Terminals>>,
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
}
