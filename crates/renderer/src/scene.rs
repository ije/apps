use crate::{
    Bounds, ContentMask, DevicePixels, Edges, Pixels, Point, Radii, Rgba, ScaledPixels, Size,
    bounds_tree::BoundsTree, point,
};

pub type DrawOrder = u32;

/// The draw order, the atlas texture of a sprite and the index of a primitive. Sorted, they
/// give the paint order.
pub(crate) type SortKey = [u32; 3];

/// The primitives of one frame. Layer 0 is the window; every effect paints into its own layer.
pub struct Scene {
    pub(crate) layers: Vec<SceneLayer>,
    active: Vec<usize>,
    spare: Vec<SceneLayer>,
}

/// The primitives painted into the window or into one effect.
#[derive(Default)]
pub(crate) struct SceneLayer {
    tree: BoundsTree<ScaledPixels>,
    stack: Vec<DrawOrder>,
    /// The effect applied to this layer, `None` for the window.
    pub(crate) effect: Option<EffectState>,
    /// The union of the clipped bounds of all primitives in this layer.
    pub(crate) bounds: Option<Bounds<ScaledPixels>>,
    pub(crate) shadows: Vec<Shadow>,
    pub(crate) quads: Vec<Quad>,
    pub(crate) underlines: Vec<Underline>,
    pub(crate) mono: Vec<MonochromeSprite>,
    pub(crate) poly: Vec<PolychromeSprite>,
    pub(crate) effects: Vec<EffectPrimitive>,
    /// The keys of the shadows, quads, underlines, mono and poly sprites and effects.
    keys: [Vec<SortKey>; 6],
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            layers: vec![SceneLayer::default()],
            active: Vec::new(),
            spare: Vec::new(),
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
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.mono.clear();
        self.poly.clear();
        self.effects.clear();
        for keys in &mut self.keys {
            keys.clear();
        }
    }

    fn insert(
        &mut self,
        bounds: Bounds<ScaledPixels>,
        mask: Bounds<ScaledPixels>,
        primitive: Primitive,
    ) {
        let clipped = bounds.intersect(&mask);
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

    /// Sorts the keys into paint order. The index keeps ties in insertion order.
    fn finish(&mut self) {
        for keys in &mut self.keys {
            keys.sort_unstable();
        }
    }
}

impl Scene {
    pub fn clear(&mut self) {
        self.active.clear();
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

    /// Paints the next primitives at one draw order, so they keep their paint order.
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
        self.current().insert(bounds, mask, primitive);
    }

    pub(crate) fn push_effect(&mut self, effect: EffectState) {
        let mut layer = self.spare.pop().unwrap_or_default();
        layer.effect = Some(effect);
        self.active.push(self.layers.len());
        self.layers.push(layer);
    }

    pub(crate) fn pop_effect(&mut self) {
        let Some(index) = self.active.pop() else {
            return;
        };
        let layer = &self.layers[index];
        let Some(effect) = layer.effect.as_ref() else {
            return;
        };
        let mut bounds = layer.bounds.map(|content| {
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
        if let Some(backdrop) = &effect.backdrop {
            bounds = Some(bounds.map_or(backdrop.bounds, |bounds| bounds.union(&backdrop.bounds)));
        }
        let Some(bounds) = bounds else {
            return;
        };
        let content_mask = effect.content_mask;
        self.insert(Primitive::Effect(EffectPrimitive {
            order: 0,
            layer: index,
            bounds,
            content_mask,
        }));
    }

    pub(crate) fn finish(&mut self) {
        for layer in &mut self.layers {
            layer.finish();
        }
    }

    /// The batches of a layer, in paint order, as keys of the layer's primitives.
    pub(crate) fn batches(&self, layer: usize) -> Batches<'_> {
        Batches(self.layers[layer].keys.each_ref().map(Vec::as_slice))
    }
}

/// Maps bounds through a 4x4 column-major matrix and returns the bounding box of the result.
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
            // WARNING: a corner behind the viewer has no 2D position, so the effect may cover anything.
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

/// The keys of each primitive kind that are not batched yet. A batch is the run of keys of one
/// kind and texture that come before the first key of the other kinds.
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

/// How an effect layer mixes with the content below it.
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

/// An effect applied to everything painted inside [`crate::Window::paint_effect`].
#[derive(Clone, Debug)]
pub struct PaintEffect {
    /// A 4x4 column-major matrix from the effect's coordinates to window coordinates, in logical pixels.
    pub transform: Option<[f32; 16]>,
    /// Multiplies the alpha of the whole group.
    pub opacity: f32,
    /// A 4x5 row-major color matrix applied to unpremultiplied colors, like SVG `feColorMatrix`.
    pub color_matrix: Option<[f32; 20]>,
    /// The standard deviation of a gaussian blur.
    pub blur: Pixels,
    /// A drop shadow of the group's alpha: offset, standard deviation and color.
    pub shadow: Option<(Point<Pixels>, Pixels, Rgba)>,
    pub blend_mode: BlendMode,
    /// A rounded rectangle that clips the group.
    pub clip: Option<(Bounds<Pixels>, Radii<Pixels>)>,
    /// An image whose alpha masks the group, stretched over the bounds.
    pub mask: Option<(std::sync::Arc<RenderImage>, Bounds<Pixels>)>,
    /// A filter for the content below the given rounded rectangle.
    pub backdrop: Option<Backdrop>,
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
        }
    }
}

/// A filter applied to the content below an element, like CSS `backdrop-filter`.
#[derive(Clone, Debug)]
pub struct Backdrop {
    pub bounds: Bounds<Pixels>,
    pub corner_radii: Radii<Pixels>,
    /// The standard deviation of a gaussian blur.
    pub blur: Pixels,
    /// A 4x5 row-major color matrix, like SVG `feColorMatrix`.
    pub color_matrix: Option<[f32; 20]>,
}

#[derive(Clone, Debug)]
pub(crate) struct BackdropState {
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Radii<ScaledPixels>,
    pub blur: f32,
    pub color_matrix: Option<[f32; 20]>,
}

/// A [`PaintEffect`] in device pixels.
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
    pub content_mask: ContentMask<ScaledPixels>,
}

#[derive(Clone, Debug)]
pub(crate) struct EffectPrimitive {
    pub order: DrawOrder,
    pub layer: usize,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
}

/// The style of a border.
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
    pub border_style: BorderStyle,
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
    /// 0 is a drop shadow outside the element, 1 an inset shadow inside it.
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
    /// The empty pixels around the content.
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
    /// One alpha byte per pixel, for glyphs.
    Monochrome = 0,
    /// BGRA pixels, for images and emoji.
    Polychrome = 1,
}

static NEXT_IMAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A decoded image: straight-alpha BGRA pixels.
pub struct RenderImage {
    pub id: u64,
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

impl RenderImage {
    /// `bytes` holds `width * height` straight-alpha BGRA pixels.
    pub fn new(width: u32, height: u32, bytes: Vec<u8>) -> Self {
        assert_eq!(bytes.len(), width as usize * height as usize * 4);
        Self {
            id: NEXT_IMAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            width,
            height,
            bytes,
        }
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
