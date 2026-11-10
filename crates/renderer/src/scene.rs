use crate::{
    Bounds, ContentMask, DevicePixels, Edges, Pixels, Point, Radii, Rgba, ScaledPixels, Size,
    bounds_tree::{BoundsTree, clear_list},
    point,
};

pub type DrawOrder = u32;

pub(crate) type SortKey = [u32; 3];

pub struct Scene {
    pub(crate) layers: Vec<SceneLayer>,
    active: Vec<usize>,
    spare: Vec<SceneLayer>,
    pub(crate) keyed: Vec<(u64, usize)>,
}

#[derive(Default)]
pub(crate) struct SceneLayer {
    tree: BoundsTree<ScaledPixels>,
    stack: Vec<DrawOrder>,
    pub(crate) effect: Option<EffectState>,
    pub(crate) bounds: Option<Bounds<ScaledPixels>>,
    pub(crate) shadows: Vec<Shadow>,
    pub(crate) quads: Vec<Quad>,
    pub(crate) underlines: Vec<Underline>,
    pub(crate) mono: Vec<MonochromeSprite>,
    pub(crate) poly: Vec<PolychromeSprite>,
    pub(crate) effects: Vec<EffectPrimitive>,
    keys: [Vec<SortKey>; 6],
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

pub(crate) enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Underline(Underline),
    Mono(MonochromeSprite),
    Poly(PolychromeSprite),
    Effect(EffectPrimitive),
}

impl SceneLayer {
    fn clear(&mut self) {
        self.tree.clear();
        self.stack.clear();
        self.effect = None;
        self.bounds = None;
        clear_list(&mut self.shadows);
        clear_list(&mut self.quads);
        clear_list(&mut self.underlines);
        clear_list(&mut self.mono);
        clear_list(&mut self.poly);
        clear_list(&mut self.effects);
        for keys in &mut self.keys {
            clear_list(keys);
        }
    }

    fn insert(
        &mut self,
        bounds: Bounds<ScaledPixels>,
        mask: Bounds<ScaledPixels>,
        primitive: Primitive,
        reach: Option<Bounds<ScaledPixels>>,
    ) {
        let clipped = reach.unwrap_or(bounds).intersect(&mask);
        if clipped.is_empty() {
            return;
        }
        self.bounds = Some(self.bounds.map_or(clipped, |all| all.union(&clipped)));
        let order = match self.stack.last() {
            Some(order) => *order,
            None => self.tree.insert(clipped),
        };
        let (kind, texture, index) = match primitive {
            Primitive::Shadow(mut shadow) => {
                shadow.order = order;
                self.shadows.push(shadow);
                (0, 0, self.shadows.len())
            }
            Primitive::Quad(mut quad) => {
                quad.order = order;
                self.quads.push(quad);
                (1, 0, self.quads.len())
            }
            Primitive::Underline(mut underline) => {
                underline.order = order;
                self.underlines.push(underline);
                (2, 0, self.underlines.len())
            }
            Primitive::Mono(mut sprite) => {
                sprite.order = order;
                self.mono.push(sprite);
                (3, sprite.tile.texture_id.index, self.mono.len())
            }
            Primitive::Poly(mut sprite) => {
                sprite.order = order;
                self.poly.push(sprite);
                (4, sprite.tile.texture_id.index, self.poly.len())
            }
            Primitive::Effect(mut effect) => {
                effect.order = order;
                self.effects.push(effect);
                (5, 0, self.effects.len())
            }
        };
        self.keys[kind].push([order, texture, index as u32 - 1]);
    }

    fn finish(&mut self) {
        for keys in &mut self.keys {
            keys.sort_unstable();
        }
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
        self.active.is_empty() && self.layers[0].stack.is_empty()
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn probe(&mut self, bounds: Bounds<ScaledPixels>) -> DrawOrder {
        self.layers[0].tree.insert(bounds)
    }

    // WARNING: text doesn't count. A glyph over a caret doesn't hide it, while a popup always paints a box.
    #[cfg(target_os = "macos")]
    pub(crate) fn covers(&self, order: DrawOrder, reach: Bounds<ScaledPixels>) -> bool {
        let window = &self.layers[0];
        let over = |other: DrawOrder, bounds: Bounds<ScaledPixels>, mask: Bounds<ScaledPixels>| {
            other > order && bounds.intersect(&mask).intersects(&reach)
        };
        window.shadows.iter().any(|shadow| {
            let bounds = shadow
                .bounds
                .dilate(ScaledPixels(shadow.blur_radius.0 * 3.));
            over(shadow.order, bounds, shadow.content_mask.bounds)
        }) || window
            .quads
            .iter()
            .any(|quad| over(quad.order, quad.bounds, quad.content_mask.bounds))
            || window
                .poly
                .iter()
                .any(|sprite| over(sprite.order, sprite.bounds, sprite.content_mask.bounds))
            || window
                .effects
                .iter()
                .any(|effect| over(effect.order, effect.bounds, effect.content_mask.bounds))
    }

    pub(crate) fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let layer = self.current();
        let order = layer.tree.insert(bounds);
        layer.stack.push(order);
    }

    pub(crate) fn pop_layer(&mut self) {
        self.current().stack.pop();
    }

    pub(crate) fn insert(&mut self, primitive: Primitive) {
        let (bounds, mask) = match &primitive {
            // WARNING: the shader draws the blur up to 3 sigma outside the shadow bounds.
            Primitive::Shadow(shadow) => (
                shadow
                    .bounds
                    .dilate(ScaledPixels(shadow.blur_radius.0 * 3.)),
                shadow.content_mask.bounds,
            ),
            Primitive::Quad(quad) => (quad.bounds, quad.content_mask.bounds),
            Primitive::Underline(underline) => (underline.bounds, underline.content_mask.bounds),
            Primitive::Mono(sprite) => (sprite.bounds, sprite.content_mask.bounds),
            Primitive::Poly(sprite) => (sprite.bounds, sprite.content_mask.bounds),
            Primitive::Effect(effect) => (effect.bounds, effect.content_mask.bounds),
        };
        self.current().insert(bounds, mask, primitive, None);
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
        let content_mask = effect.content_mask;
        let reach = (self.active.is_empty()
            && effect.transform.is_some()
            && self.keyed.iter().any(|(_, layer)| *layer == index))
        .then_some(content_mask.bounds);
        let primitive = Primitive::Effect(EffectPrimitive {
            order: 0,
            layer: index,
            bounds,
            content_mask,
        });
        self.current()
            .insert(bounds, content_mask.bounds, primitive, reach);
    }

    pub(crate) fn patch(&mut self, key: u64, opacity: f32, transform: Option<[f32; 16]>) -> bool {
        let Some(&(_, index)) = self.keyed.iter().find(|(keyed, _)| *keyed == key) else {
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
            .effects
            .iter_mut()
            .find(|primitive| primitive.layer == index)
            .map(|primitive| primitive.bounds = bounds)
            .is_some()
    }

    pub(crate) fn paints_over(
        &self,
        index: usize,
        reach: Bounds<ScaledPixels>,
        skip: impl Fn(usize) -> bool,
    ) -> bool {
        let window = &self.layers[0];
        let Some(own) = window.effects.iter().find(|effect| effect.layer == index) else {
            return true;
        };
        let over = |order: DrawOrder, bounds: Bounds<ScaledPixels>, mask: Bounds<ScaledPixels>| {
            order > own.order && bounds.intersect(&mask).intersects(&reach)
        };
        window.shadows.iter().any(|shadow| {
            let bounds = shadow
                .bounds
                .dilate(ScaledPixels(shadow.blur_radius.0 * 3.));
            over(shadow.order, bounds, shadow.content_mask.bounds)
        }) || window
            .quads
            .iter()
            .any(|quad| over(quad.order, quad.bounds, quad.content_mask.bounds))
            || window.underlines.iter().any(|underline| {
                over(
                    underline.order,
                    underline.bounds,
                    underline.content_mask.bounds,
                )
            })
            || window
                .mono
                .iter()
                .any(|sprite| over(sprite.order, sprite.bounds, sprite.content_mask.bounds))
            || window
                .poly
                .iter()
                .any(|sprite| over(sprite.order, sprite.bounds, sprite.content_mask.bounds))
            || window.effects.iter().any(|effect| {
                (effect.order, effect.layer) > (own.order, index)
                    && !skip(effect.layer)
                    && effect
                        .bounds
                        .intersect(&effect.content_mask.bounds)
                        .intersects(&reach)
            })
    }

    pub(crate) fn finish(&mut self) {
        for layer in &mut self.layers {
            layer.finish();
        }
    }

    pub(crate) fn batches(&self, layer: usize) -> Batches<'_> {
        Batches(self.layers[layer].keys.each_ref().map(Vec::as_slice))
    }
}

fn effect_bounds(layer: &SceneLayer) -> Option<Bounds<ScaledPixels>> {
    let effect = layer.effect.as_ref()?;
    let bounds = layer.bounds.map(|content| {
        let mut bounds = content.dilate(ScaledPixels(effect.blur * 3.));
        if let Some((offset, sigma, _)) = effect.shadow {
            let shadow = content.dilate(ScaledPixels(sigma * 3.));
            bounds = bounds.union(&(shadow + offset.map(ScaledPixels)));
        }
        match &effect.transform {
            Some(matrix) => transform_bounds(matrix, bounds),
            None => bounds,
        }
    });
    match &effect.backdrop {
        Some(backdrop) => {
            Some(bounds.map_or(backdrop.bounds, |bounds| bounds.union(&backdrop.bounds)))
        }
        None => bounds,
    }
}

pub(crate) fn transform_bounds(
    matrix: &[f32; 16],
    bounds: Bounds<ScaledPixels>,
) -> Bounds<ScaledPixels> {
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for corner in [
        bounds.origin,
        bounds.top_right(),
        bounds.bottom_left(),
        bounds.bottom_right(),
    ] {
        let [x, y] = [corner.x.0, corner.y.0];
        let w = matrix[3] * x + matrix[7] * y + matrix[15];
        if w <= 1e-4 {
            return Bounds::new(
                point(ScaledPixels(-1e6), ScaledPixels(-1e6)),
                Size::new(ScaledPixels(2e6), ScaledPixels(2e6)),
            );
        }
        let px = (matrix[0] * x + matrix[4] * y + matrix[12]) / w;
        let py = (matrix[1] * x + matrix[5] * y + matrix[13]) / w;
        min = [min[0].min(px), min[1].min(py)];
        max = [max[0].max(px), max[1].max(py)];
    }
    Bounds::from_corners(
        point(ScaledPixels(min[0]), ScaledPixels(min[1])),
        point(ScaledPixels(max[0]), ScaledPixels(max[1])),
    )
}

#[derive(Debug)]
pub(crate) enum Batch<'a> {
    Shadows(&'a [SortKey]),
    Quads(&'a [SortKey]),
    Underlines(&'a [SortKey]),
    Mono(AtlasTextureId, &'a [SortKey]),
    Poly(AtlasTextureId, &'a [SortKey]),
    Effects(&'a [SortKey]),
}

pub(crate) struct Batches<'a>([&'a [SortKey]; 6]);

impl<'a> Iterator for Batches<'a> {
    type Item = Batch<'a>;

    fn next(&mut self) -> Option<Batch<'a>> {
        let mut heads: [_; 6] =
            std::array::from_fn(|kind| (self.0[kind].first().map_or(u32::MAX, |key| key[0]), kind));
        heads.sort_unstable();
        let kind = heads[0].1;
        let &[_, texture, _] = self.0[kind].first()?;
        let len = 1 + self.0[kind][1..]
            .iter()
            .take_while(|key| (key[0], kind) < heads[1] && key[1] == texture)
            .count();
        let (batch, rest) = self.0[kind].split_at(len);
        self.0[kind] = rest;
        let texture = |kind| AtlasTextureId {
            index: texture,
            kind,
        };
        Some(match kind {
            0 => Batch::Shadows(batch),
            1 => Batch::Quads(batch),
            2 => Batch::Underlines(batch),
            3 => Batch::Mono(texture(AtlasTextureKind::Monochrome), batch),
            4 => Batch::Poly(texture(AtlasTextureKind::Polychrome), batch),
            _ => Batch::Effects(batch),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u32)]
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
    pub shadow: Option<(Point<Pixels>, Pixels, Rgba)>,
    pub blend_mode: BlendMode,
    pub clip: Option<(Bounds<Pixels>, Radii<Pixels>)>,
    pub mask: Option<(std::sync::Arc<RenderImage>, Bounds<Pixels>)>,
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
    pub bounds: Bounds<Pixels>,
    pub corner_radius: Pixels,
    pub tint: Option<Rgba>,
    pub clear: bool,
    pub interactive: bool,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct GlassState {
    pub key: u64,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radius: f32,
    pub tint: Option<Rgba>,
    pub clear: bool,
    pub interactive: bool,
}

#[derive(Clone, Debug)]
pub struct Backdrop {
    pub bounds: Bounds<Pixels>,
    pub corner_radii: Radii<Pixels>,
    pub blur: Pixels,
    pub color_matrix: Option<[f32; 20]>,
}

#[derive(Clone, Debug)]
pub(crate) struct BackdropState {
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Radii<ScaledPixels>,
    pub blur: f32,
    pub color_matrix: Option<[f32; 20]>,
}

#[derive(Clone, Debug)]
pub(crate) struct EffectState {
    pub transform: Option<[f32; 16]>,
    pub opacity: f32,
    pub color_matrix: Option<[f32; 20]>,
    pub blur: f32,
    pub shadow: Option<(Point<f32>, f32, Rgba)>,
    pub blend_mode: BlendMode,
    pub clip: Option<(Bounds<ScaledPixels>, Radii<ScaledPixels>)>,
    pub mask: Option<(AtlasTile, Bounds<ScaledPixels>)>,
    pub backdrop: Option<BackdropState>,
    pub glass: Option<GlassState>,
    pub content_mask: ContentMask<ScaledPixels>,
}

#[derive(Clone, Debug)]
pub(crate) struct EffectPrimitive {
    pub order: DrawOrder,
    pub layer: usize,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum BorderStyle {
    #[default]
    Solid = 0,
    Dashed = 1,
    Dotted = 2,
    Double = 3,
}

// WARNING: the GPU structs below are read by all three shader files, so a layout change goes into
// shaders.metal, shaders.hlsl and shaders.wgsl together.

#[derive(Default, Debug, Copy, Clone)]
#[repr(C)]
pub(crate) struct Quad {
    pub order: DrawOrder,
    pub border_style: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Rgba,
    pub border_color: Rgba,
    pub corner_radii: Radii<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub(crate) struct Underline {
    pub order: DrawOrder,
    pub wavy: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Rgba,
    pub thickness: ScaledPixels,
    pub pad: u32,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub(crate) struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Radii<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Rgba,
    pub element_bounds: Bounds<ScaledPixels>,
    pub element_corner_radii: Radii<ScaledPixels>,
    pub inset: u32,
    pub pad: u32,
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub(crate) struct MonochromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub tile: AtlasTile,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Rgba,
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub(crate) struct PolychromeSprite {
    pub order: DrawOrder,
    // WARNING: bit 8 marks a premultiplied image, which the shaders divide by alpha after sampling.
    pub grayscale: u32,
    pub tile: AtlasTile,
    pub opacity: f32,
    pub pad: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Radii<ScaledPixels>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct AtlasTile {
    pub texture_id: AtlasTextureId,
    pub tile_id: u32,
    pub padding: u32,
    pub bounds: Bounds<DevicePixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub(crate) struct AtlasTextureId {
    pub index: u32,
    pub kind: AtlasTextureKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub(crate) enum AtlasTextureKind {
    Monochrome = 0,
    Polychrome = 1,
}

static NEXT_IMAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

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
            id: NEXT_IMAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
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
    pub fn size(&self) -> Size<DevicePixels> {
        Size::new(
            DevicePixels(self.width as i32),
            DevicePixels(self.height as i32),
        )
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl std::fmt::Debug for RenderImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RenderImage({}, {}x{})",
            self.id, self.width, self.height
        )
    }
}

impl PartialEq for RenderImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_glass_can_overlap_but_cannot_cover_gpu_overlays() {
        let bounds = |[x, y, width, height]: [f32; 4]| {
            Bounds::new(
                point(ScaledPixels(x), ScaledPixels(y)),
                Size::new(ScaledPixels(width), ScaledPixels(height)),
            )
        };
        let pill = bounds([12., 12., 236., 52.]);
        let close = bounds([10., 10., 18., 18.]);
        let mask = ContentMask {
            bounds: bounds([0., 0., 260., 76.]),
            ..Default::default()
        };
        let mut scene = Scene::default();
        for (layer, bounds) in [(1, pill), (2, close)] {
            scene.layers[0].effects.push(EffectPrimitive {
                order: layer as u32,
                layer,
                bounds,
                content_mask: mask,
            });
        }
        assert!(scene.paints_over(1, pill, |_| false));
        assert!(!scene.paints_over(2, close, |_| false));
        assert!(!scene.paints_over(1, pill, |layer| layer == 2));

        scene.layers[0].quads.push(Quad {
            order: 3,
            bounds: close,
            content_mask: mask,
            ..Default::default()
        });
        assert!(scene.paints_over(1, pill, |layer| layer == 2));
        assert!(scene.paints_over(2, close, |_| false));

        scene.layers[0].quads.clear();
        scene.layers[0].effects.push(EffectPrimitive {
            order: 3,
            layer: 3,
            bounds: close,
            content_mask: mask,
        });
        assert!(scene.paints_over(1, pill, |layer| layer == 2));
        assert!(scene.paints_over(2, close, |_| false));
    }
}
