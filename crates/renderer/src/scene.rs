use crate::{Clip, Color, Pixels, Point, Radii, Rect, Size, atlas::Tile, gpu::Instance, point};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

pub struct Scene {
    pub(crate) layers: Vec<SceneLayer>,
    active: Vec<usize>,
    spare: Vec<SceneLayer>,
    keyed: Vec<(u64, usize)>,
}

#[derive(Default)]
pub(crate) struct SceneLayer {
    pub(crate) instances: Vec<Instance>,
    pub(crate) nested: Vec<Nested>,
    pub(crate) effect: Option<EffectState>,
    pub(crate) bounds: Option<Rect<f32>>,
}

// Composited before the instance at `at` of its parent.
#[derive(Clone, Copy)]
pub(crate) struct Nested {
    pub at: u32,
    pub layer: usize,
    pub bounds: Rect<f32>,
    pub clip: Clip<f32>,
}

impl Nested {
    fn reach(&self) -> Rect<f32> {
        self.bounds.intersect(self.clip.bounds)
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            layers: vec![SceneLayer::default()],
            active: Vec::new(),
            spare: Vec::new(),
            keyed: Vec::new(),
        }
    }
}

impl SceneLayer {
    fn clear(&mut self) {
        self.instances.clear();
        self.nested.clear();
        self.effect = None;
        self.bounds = None;
    }

    fn grow(&mut self, reach: Rect<f32>) {
        self.bounds = Some(self.bounds.map_or(reach, |all| all.union(reach)));
    }
}

impl Scene {
    pub fn clear(&mut self) {
        self.active.clear();
        self.keyed.clear();
        for mut layer in self.layers.drain(1..) {
            layer.clear();
            self.spare.push(layer);
        }
        self.layers[0].clear();
    }

    fn current(&mut self) -> &mut SceneLayer {
        let index = self.active.last().copied().unwrap_or(0);
        &mut self.layers[index]
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn at_root(&self) -> bool {
        self.active.is_empty()
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn probe(&self) -> u32 {
        let window = &self.layers[0];
        (window.instances.len() + window.nested.len()) as u32
    }

    // Whether something painted at or after `step` of `probe` overlaps `reach`. Text doesn't count: a
    // glyph over a caret doesn't hide it, while a popup always paints a box.
    #[cfg(target_os = "macos")]
    pub(crate) fn covers(&self, step: u32, reach: Rect<f32>) -> bool {
        use crate::gpu::{GLYPH, LINE};
        let window = &self.layers[0];
        if window.nested.iter().enumerate().any(|(index, nested)| {
            nested.at + index as u32 >= step && nested.reach().intersects(reach)
        }) {
            return true;
        }
        let mut before = 0;
        window
            .instances
            .iter()
            .enumerate()
            .any(|(index, instance)| {
                while window
                    .nested
                    .get(before)
                    .is_some_and(|nested| nested.at as usize <= index)
                {
                    before += 1;
                }
                (index + before) as u32 >= step
                    && !matches!(instance.kind(), LINE | GLYPH)
                    && instance.reach().intersects(reach)
            })
    }

    pub(crate) fn push(&mut self, instance: Instance) {
        let reach = instance.reach();
        if reach.is_empty() {
            return;
        }
        let layer = self.current();
        layer.grow(reach);
        layer.instances.push(instance);
    }

    pub(crate) fn push_effect(&mut self, effect: EffectState, key: Option<u64>) {
        let mut layer = self.spare.pop().unwrap_or_default();
        if let Some(glass) = &effect.glass {
            layer.bounds = Some(glass.bounds);
        }
        layer.effect = Some(effect);
        if let Some(key) = key {
            self.keyed.push((key, self.layers.len()));
        }
        self.active.push(self.layers.len());
        self.layers.push(layer);
    }

    pub(crate) fn pop_effect(&mut self) {
        let Some(index) = self.active.pop() else {
            return;
        };
        let Some(effect) = self.layers[index].effect.as_ref() else {
            return;
        };
        let Some(bounds) = effect_bounds(&self.layers[index]) else {
            return;
        };
        let clip = effect.content_mask;
        // A keyed transform can move anywhere within its clip without a repaint.
        // `keyed` is sorted by layer, since layers are pushed in order.
        let moves = self.active.is_empty()
            && effect.transform.is_some()
            && self
                .keyed
                .binary_search_by_key(&index, |&(_, layer)| layer)
                .is_ok();
        let reach = if moves { clip.bounds } else { bounds }.intersect(clip.bounds);
        if reach.is_empty() {
            return;
        }
        let parent = self.current();
        parent.grow(reach);
        parent.nested.push(Nested {
            at: parent.instances.len() as u32,
            layer: index,
            bounds,
            clip,
        });
    }

    pub(crate) fn keyed(&self, key: u64) -> Option<usize> {
        let &(_, index) = self.keyed.iter().find(|(keyed, _)| *keyed == key)?;
        Some(index)
    }

    pub(crate) fn patch(&mut self, key: u64, opacity: f32, transform: Option<[f32; 16]>) -> bool {
        let Some(index) = self.keyed(key) else {
            return false;
        };
        let Some(effect) = self.layers[index].effect.as_mut() else {
            return false;
        };
        effect.opacity = opacity;
        let Some(matrix) = transform else {
            return true;
        };
        if effect.transform.is_none() {
            return false;
        }
        effect.transform = Some(matrix);
        let Some(bounds) = effect_bounds(&self.layers[index]) else {
            return true;
        };
        self.layers[0]
            .nested
            .iter_mut()
            .find(|nested| nested.layer == index)
            .map(|nested| nested.bounds = bounds)
            .is_some()
    }

    pub(crate) fn paints_over(
        &self,
        index: usize,
        reach: Rect<f32>,
        skip: impl Fn(usize) -> bool,
    ) -> bool {
        let window = &self.layers[0];
        let Some(own) = window
            .nested
            .iter()
            .position(|nested| nested.layer == index)
        else {
            return true;
        };
        window.instances[window.nested[own].at as usize..]
            .iter()
            .any(|instance| instance.reach().intersects(reach))
            || window.nested[own + 1..]
                .iter()
                .any(|nested| !skip(nested.layer) && nested.reach().intersects(reach))
    }
}

fn effect_bounds(layer: &SceneLayer) -> Option<Rect<f32>> {
    let effect = layer.effect.as_ref()?;
    let bounds = layer.bounds.map(|content| {
        let mut bounds = content.outset(effect.blur * 3.);
        if let Some((offset, sigma, _)) = effect.shadow {
            let shadow = content.outset(sigma * 3.);
            bounds = bounds.union(shadow + offset);
        }
        match &effect.transform {
            Some(matrix) => transform_bounds(matrix, bounds),
            None => bounds,
        }
    });
    match &effect.backdrop {
        Some(backdrop) => {
            Some(bounds.map_or(backdrop.bounds, |bounds| bounds.union(backdrop.bounds)))
        }
        None => bounds,
    }
}

pub(crate) fn transform_bounds(matrix: &[f32; 16], bounds: Rect<f32>) -> Rect<f32> {
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for corner in [
        bounds.origin,
        bounds.top_right(),
        bounds.bottom_left(),
        bounds.bottom_right(),
    ] {
        let [x, y] = [corner.x, corner.y];
        let w = matrix[3] * x + matrix[7] * y + matrix[15];
        if w <= 1e-4 {
            return Rect::new(point(-1e6, -1e6), Size::new(2e6, 2e6));
        }
        let px = (matrix[0] * x + matrix[4] * y + matrix[12]) / w;
        let py = (matrix[1] * x + matrix[5] * y + matrix[13]) / w;
        min = [min[0].min(px), min[1].min(py)];
        max = [max[0].max(px), max[1].max(py)];
    }
    Rect::between(point(min[0], min[1]), point(max[0], max[1]))
}

// The shader reads these numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum BlendMode {
    #[default]
    Normal = 0,
    Multiply = 1,
    Screen = 2,
    Overlay = 3,
    Darken = 4,
    Lighten = 5,
    ColorDodge = 6,
    ColorBurn = 7,
    HardLight = 8,
    SoftLight = 9,
    Difference = 10,
    Exclusion = 11,
    Hue = 12,
    Saturation = 13,
    Color = 14,
    Luminosity = 15,
    PlusLighter = 16,
}

#[derive(Clone, Debug)]
pub struct PaintEffect {
    pub transform: Option<[f32; 16]>,
    pub opacity: f32,
    pub color_matrix: Option<[f32; 20]>,
    pub blur: Pixels,
    pub shadow: Option<(Point<Pixels>, Pixels, Color)>,
    pub blend_mode: BlendMode,
    pub clip: Option<(Rect<Pixels>, Radii<Pixels>)>,
    pub mask: Option<(std::sync::Arc<RenderImage>, Rect<Pixels>)>,
    pub backdrop: Option<Backdrop>,
    pub glass: Option<Glass>,
    pub key: Option<u64>,
}

impl Default for PaintEffect {
    fn default() -> Self {
        Self {
            transform: None,
            opacity: 1.,
            color_matrix: None,
            blur: Pixels::ZERO,
            shadow: None,
            blend_mode: BlendMode::Normal,
            clip: None,
            mask: None,
            backdrop: None,
            glass: None,
            key: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Glass {
    pub key: u64,
    pub bounds: Rect<Pixels>,
    pub corner_radius: Pixels,
    pub tint: Option<Color>,
    pub clear: bool,
    pub interactive: bool,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct GlassState {
    pub key: u64,
    pub bounds: Rect<f32>,
    pub corner_radius: f32,
    pub tint: Option<Color>,
    pub clear: bool,
    pub interactive: bool,
}

#[derive(Clone, Debug)]
pub struct Backdrop {
    pub bounds: Rect<Pixels>,
    pub corner_radii: Radii<Pixels>,
    pub blur: Pixels,
    pub color_matrix: Option<[f32; 20]>,
}

pub(crate) struct BackdropState {
    pub bounds: Rect<f32>,
    pub corner_radii: Radii<f32>,
    pub blur: f32,
    pub color_matrix: Option<[f32; 20]>,
}

pub(crate) struct EffectState {
    pub transform: Option<[f32; 16]>,
    pub opacity: f32,
    pub color_matrix: Option<[f32; 20]>,
    pub blur: f32,
    pub shadow: Option<(Point<f32>, f32, Color)>,
    pub blend_mode: BlendMode,
    pub clip: Option<(Rect<f32>, Radii<f32>)>,
    pub mask: Option<(Tile, Rect<f32>)>,
    pub backdrop: Option<BackdropState>,
    pub glass: Option<GlassState>,
    pub content_mask: Clip<f32>,
}

// The shader reads the style as its position in this list.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum BorderStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
    Double,
}

static IMAGES: AtomicU64 = AtomicU64::new(1);

pub struct RenderImage {
    pub id: u64,
    width: u32,
    height: u32,
    bytes: Vec<u8>,
    premultiplied: bool,
}

impl RenderImage {
    pub fn new(width: u32, height: u32, bytes: Vec<u8>) -> Self {
        assert_eq!(bytes.len(), width as usize * height as usize * 4);
        Self {
            id: IMAGES.fetch_add(1, Relaxed),
            width,
            height,
            bytes,
            premultiplied: false,
        }
    }
    pub fn new_premultiplied(width: u32, height: u32, bytes: Vec<u8>) -> Self {
        Self {
            premultiplied: true,
            ..Self::new(width, height, bytes)
        }
    }
    pub fn is_premultiplied(&self) -> bool {
        self.premultiplied
    }
    pub fn size(&self) -> Size<i32> {
        Size::new(self.width as i32, self.height as i32)
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
    /// Takes the pixels for the next image. The empty image keeps its id, so its tile can still be released.
    pub fn take_bytes(&mut self) -> Vec<u8> {
        (self.width, self.height) = (0, 0);
        std::mem::take(&mut self.bytes)
    }
}

impl std::fmt::Debug for RenderImage {
    fn fmt(&self, out: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(out, "image {} of {}x{}", self.id, self.width, self.height)
    }
}

impl PartialEq for RenderImage {
    fn eq(&self, image: &RenderImage) -> bool {
        image.id == self.id
    }
}

#[cfg(test)]
pub(crate) fn effect(content_mask: Clip<f32>) -> EffectState {
    EffectState {
        transform: None,
        opacity: 1.,
        color_matrix: None,
        blur: 0.,
        shadow: None,
        blend_mode: BlendMode::Normal,
        clip: None,
        mask: None,
        backdrop: None,
        glass: None,
        content_mask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{BOX, rect as bounds};

    #[test]
    fn native_glass_can_overlap_but_cannot_cover_gpu_overlays() {
        let pill = bounds([12., 12., 236., 52.]);
        let close = bounds([10., 10., 18., 18.]);
        let mask = Clip::from(bounds([0., 0., 260., 76.]));
        let layer = |scene: &mut Scene, rect: Rect<f32>| {
            scene.push_effect(effect(mask), None);
            scene.push(Instance::new(BOX, rect, mask));
            scene.pop_effect();
        };
        let mut scene: Scene = Default::default();
        layer(&mut scene, pill);
        layer(&mut scene, close);
        assert!(scene.paints_over(1, pill, |_| false));
        assert!(!scene.paints_over(2, close, |_| false));
        assert!(!scene.paints_over(1, pill, |layer| layer == 2));
        assert!(
            scene.paints_over(3, pill, |_| false),
            "a layer that isn't there"
        );

        scene.push(Instance::new(BOX, close, mask));
        assert!(scene.paints_over(1, pill, |layer| layer == 2));
        assert!(scene.paints_over(2, close, |_| false));

        scene.layers[0].instances.clear();
        layer(&mut scene, close);
        assert!(scene.paints_over(1, pill, |layer| layer == 2));
        assert!(scene.paints_over(2, close, |_| false));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn carets_count_boxes_and_effects_painted_after_them_but_not_text() {
        use crate::gpu::{GLYPH, LINE};
        let mask = Clip::from(bounds([0., 0., 100., 100.]));
        let caret = bounds([10., 10., 2., 16.]);
        let mut scene: Scene = Default::default();
        scene.push(Instance::new(BOX, bounds([0., 0., 100., 100.]), mask));
        scene.push_effect(effect(mask), None);
        scene.push(Instance::new(BOX, caret, mask));
        scene.pop_effect();
        let step = scene.probe();
        assert!(!scene.covers(step, caret), "earlier boxes and effects");
        scene.push(Instance::new(BOX, caret, mask));
        assert!(scene.covers(step, caret), "the caret's own box");
        assert!(!scene.covers(step + 1, caret), "after the caret's own box");
        scene.push(Instance::new(GLYPH, caret, mask));
        scene.push(Instance::new(LINE, caret, mask));
        assert!(!scene.covers(step + 1, caret), "text");
        scene.push(Instance::new(BOX, bounds([50., 50., 10., 10.]), mask));
        assert!(!scene.covers(step + 1, caret), "a box elsewhere");
        scene.push_effect(effect(mask), None);
        scene.push(Instance::new(BOX, caret, mask));
        scene.pop_effect();
        assert!(scene.covers(step + 1, caret), "an effect over it");
        assert!(scene.at_root());
    }

    #[test]
    fn clipped_instances_and_effects_are_dropped() {
        let mask = Clip::from(bounds([0., 0., 100., 100.]));
        let outside = bounds([200., 0., 10., 10.]);
        let mut scene: Scene = Default::default();
        scene.push(Instance::new(BOX, outside, mask));
        scene.push_effect(effect(mask), Some(4));
        scene.push(Instance::new(
            BOX,
            outside,
            Clip::from(bounds([0., 0., 1e4, 1e4])),
        ));
        scene.pop_effect();
        assert!(scene.layers[0].instances.is_empty());
        assert!(scene.layers[0].nested.is_empty());
        assert_eq!(scene.layers[0].bounds, None);
    }
}
