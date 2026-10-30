use crate::{
    Bounds, ContentMask, Corners, DevicePixels, Edges, Pixels, Point, Rgba, ScaledPixels, Size,
    bounds_tree::BoundsTree, point,
};
use std::{iter::Peekable, ops::Range, slice};

pub type DrawOrder = u32;

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
        match primitive {
            Primitive::Shadow(mut shadow) => {
                shadow.order = order;
                self.shadows.push(shadow);
            }
            Primitive::Quad(mut quad) => {
                quad.order = order;
                self.quads.push(quad);
            }
            Primitive::Underline(mut underline) => {
                underline.order = order;
                self.underlines.push(underline);
            }
            Primitive::Mono(mut sprite) => {
                sprite.order = order;
                self.mono.push(sprite);
            }
            Primitive::Poly(mut sprite) => {
                sprite.order = order;
                self.poly.push(sprite);
            }
            Primitive::Effect(mut effect) => {
                effect.order = order;
                self.effects.push(effect);
            }
        }
    }

    fn finish(&mut self) {
        self.shadows.sort_by_key(|shadow| shadow.order);
        self.quads.sort_by_key(|quad| quad.order);
        self.underlines.sort_by_key(|underline| underline.order);
        self.mono
            .sort_by_key(|sprite| (sprite.order, sprite.tile.texture_id.index));
        self.poly
            .sort_by_key(|sprite| (sprite.order, sprite.tile.texture_id.index));
        self.effects.sort_by_key(|effect| effect.order);
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

    /// The batches of a layer, in paint order. Ranges index into the layer's primitives.
    pub(crate) fn batches(&self, layer: usize) -> Batches<'_> {
        let layer = &self.layers[layer];
        Batches {
            starts: [0; 6],
            shadows: layer.shadows.iter().peekable(),
            quads: layer.quads.iter().peekable(),
            underlines: layer.underlines.iter().peekable(),
            mono: layer.mono.iter().peekable(),
            poly: layer.poly.iter().peekable(),
            effects: layer.effects.iter().peekable(),
        }
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
pub(crate) enum Batch {
    Shadows(Range<usize>),
    Quads(Range<usize>),
    Underlines(Range<usize>),
    Mono(AtlasTextureId, Range<usize>),
    Poly(AtlasTextureId, Range<usize>),
    Effects(Range<usize>),
}

pub(crate) struct Batches<'a> {
    starts: [usize; 6],
    shadows: Peekable<slice::Iter<'a, Shadow>>,
    quads: Peekable<slice::Iter<'a, Quad>>,
    underlines: Peekable<slice::Iter<'a, Underline>>,
    mono: Peekable<slice::Iter<'a, MonochromeSprite>>,
    poly: Peekable<slice::Iter<'a, PolychromeSprite>>,
    effects: Peekable<slice::Iter<'a, EffectPrimitive>>,
}

/// Takes the run of primitives that come before `limit` in paint order.
fn take<'a, T: 'a>(
    iter: &mut Peekable<slice::Iter<'a, T>>,
    start: &mut usize,
    kind: usize,
    limit: (u32, usize),
    order: impl Fn(&T) -> u32,
    same: impl Fn(&T) -> bool,
) -> Range<usize> {
    let first = *start;
    iter.next();
    *start += 1;
    while iter
        .next_if(|item| (order(item), kind) < limit && same(item))
        .is_some()
    {
        *start += 1;
    }
    first..*start
}

impl Iterator for Batches<'_> {
    type Item = Batch;

    fn next(&mut self) -> Option<Batch> {
        let mut heads = [
            (self.shadows.peek().map(|item| item.order), 0),
            (self.quads.peek().map(|item| item.order), 1),
            (self.underlines.peek().map(|item| item.order), 2),
            (self.mono.peek().map(|item| item.order), 3),
            (self.poly.peek().map(|item| item.order), 4),
            (self.effects.peek().map(|item| item.order), 5),
        ];
        heads.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));
        let (Some(_), kind) = heads[0] else {
            return None;
        };
        let limit = (heads[1].0.unwrap_or(u32::MAX), heads[1].1);
        let start = &mut self.starts[kind];
        Some(match kind {
            0 => Batch::Shadows(take(
                &mut self.shadows,
                start,
                kind,
                limit,
                |item| item.order,
                |_| true,
            )),
            1 => Batch::Quads(take(
                &mut self.quads,
                start,
                kind,
                limit,
                |item| item.order,
                |_| true,
            )),
            2 => Batch::Underlines(take(
                &mut self.underlines,
                start,
                kind,
                limit,
                |item| item.order,
                |_| true,
            )),
            3 => {
                let texture = self.mono.peek().unwrap().tile.texture_id;
                Batch::Mono(
                    texture,
                    take(
                        &mut self.mono,
                        start,
                        kind,
                        limit,
                        |item| item.order,
                        |item| item.tile.texture_id == texture,
                    ),
                )
            }
            4 => {
                let texture = self.poly.peek().unwrap().tile.texture_id;
                Batch::Poly(
                    texture,
                    take(
                        &mut self.poly,
                        start,
                        kind,
                        limit,
                        |item| item.order,
                        |item| item.tile.texture_id == texture,
                    ),
                )
            }
            _ => Batch::Effects(take(
                &mut self.effects,
                start,
                kind,
                limit,
                |item| item.order,
                |_| true,
            )),
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
    pub clip: Option<(Bounds<Pixels>, Corners<Pixels>)>,
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
    pub corner_radii: Corners<Pixels>,
    /// The standard deviation of a gaussian blur.
    pub blur: Pixels,
    /// A 4x5 row-major color matrix, like SVG `feColorMatrix`.
    pub color_matrix: Option<[f32; 20]>,
}

#[derive(Clone, Debug)]
pub(crate) struct BackdropState {
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
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
    pub clip: Option<(Bounds<ScaledPixels>, Corners<ScaledPixels>)>,
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
    pub corner_radii: Corners<ScaledPixels>,
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
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Rgba,
    pub element_bounds: Bounds<ScaledPixels>,
    pub element_corner_radii: Corners<ScaledPixels>,
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
    pub corner_radii: Corners<ScaledPixels>,
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
