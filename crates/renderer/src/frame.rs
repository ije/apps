//! Turns a scene into draw commands for the platform renderers, including the offscreen
//! layers that effects are painted into.

use crate::{
    BlendMode, Bounds, Point, ScaledPixels, Size,
    scene::{
        AtlasTextureId, Batch, EffectPrimitive, MonochromeSprite, PolychromeSprite, Quad, Scene,
        Shadow, SortKey, Underline,
    },
};
use std::{mem, ops::Range};

const IDENTITY: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];
const COMPOSITE_MATRIX: u32 = 1;
const COMPOSITE_CLIP: u32 = 2;
const COMPOSITE_MASK: u32 = 4;
const COMPOSITE_SHADOW: u32 = 8;
const COMPOSITE_BACKDROP: u32 = 16;
const COMPOSITE_BLEND: u32 = 32;

/// The shader globals of one render target, which covers `viewport_size` device pixels from
/// `viewport_origin` of the window.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Globals {
    pub gamma_ratios: [f32; 4],
    pub viewport_size: [f32; 2],
    pub viewport_origin: [f32; 2],
    pub window_size: [f32; 2],
    pub grayscale_enhanced_contrast: f32,
    pub subpixel_enhanced_contrast: f32,
    pub premultiplied_alpha: u32,
    pub is_bgr: u32,
    pub pad: [u32; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TextParams {
    pub gamma_ratios: [f32; 4],
    pub grayscale_enhanced_contrast: f32,
    pub subpixel_enhanced_contrast: f32,
    pub is_bgr: bool,
}

/// Draws an offscreen layer onto a target. `mask_tile` is in mask texture pixels.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Composite {
    pub transform: [f32; 16],
    pub color_matrix: [f32; 16],
    pub color_offset: [f32; 4],
    pub shadow_color: [f32; 4],
    pub bounds: [f32; 4],
    pub clip: [f32; 4],
    pub clip_radii: [f32; 8],
    pub mask_rect: [f32; 4],
    pub mask_tile: [f32; 4],
    pub content_mask: [f32; 4],
    pub rounded_mask: [f32; 4],
    pub rounded_radii: [f32; 8],
    pub shadow: [f32; 4],
    pub source_rect: [f32; 4],
    pub backdrop_rect: [f32; 4],
    pub opacity: f32,
    pub blur: f32,
    pub flags: u32,
    pub blend: u32,
}

/// One direction of a separable blur, drawn at `scale` window pixels per target pixel.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Blur {
    pub region: [f32; 4],
    pub source_rect: [f32; 4],
    pub direction: [f32; 2],
    pub sigma: f32,
    pub scale: f32,
}

/// Blurs at least this wide, in device pixels, run at half resolution: the gaussian hides the
/// lost detail, and both passes draw a quarter of the pixels.
const HALF_RESOLUTION_SIGMA: f32 = 8.;

/// The window, or an offscreen layer covering part of the window.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Frame,
    Layer(usize),
}

/// An offscreen layer: a pooled texture whose first pixel is at `origin` in window device
/// pixels, with `scale` window pixels per texture pixel. Small layers share one texture as tiles,
/// and draw only inside `bounds`.
pub struct Layer {
    pub texture: usize,
    pub origin: [i32; 2],
    scale: f32,
    bounds: [f32; 4],
}

/// The texture that the small layers of one nesting depth share, packed in rows.
struct Shelf {
    texture: usize,
    x: i32,
    y: i32,
    row: i32,
}

type Deferred = (Target, Option<AtlasTextureId>, Composite);

pub enum Cmd {
    /// Draw the next commands into the target, cleared first if `true`.
    Pass(Target, bool),
    Quads(Range<u32>),
    Shadows(Range<u32>),
    Underlines(Range<u32>),
    Mono(AtlasTextureId, Range<u32>),
    Poly(AtlasTextureId, Range<u32>),
    /// Draw a blur into the cleared `dst`, then continue with the previous target.
    Blur {
        src: Target,
        dst: Target,
        index: u32,
    },
    /// Copy the `[x, y, width, height]` window region.
    Copy {
        src: Target,
        dst: Target,
        region: [i32; 4],
    },
    Composite {
        src: Target,
        mask: Option<AtlasTextureId>,
        backdrop: Option<Target>,
        index: u32,
    },
}

/// The instance data and commands of one frame, kept to reuse the allocations.
#[derive(Default)]
pub struct RenderFrame {
    pub quads: Vec<Quad>,
    pub shadows: Vec<Shadow>,
    pub underlines: Vec<Underline>,
    pub mono: Vec<MonochromeSprite>,
    pub poly: Vec<PolychromeSprite>,
    pub composites: Vec<Composite>,
    pub blurs: Vec<Blur>,
    pub cmds: Vec<Cmd>,
    pub layers: Vec<Layer>,
    /// The size of each pooled texture the layers use.
    pub textures: Vec<[u32; 2]>,
    free: Vec<usize>,
    shelves: Vec<Option<Shelf>>,
    /// The composites of tiles, per depth, that wait until the effects of a batch are drawn.
    deferred: Vec<Vec<Deferred>>,
    window: [u32; 2],
    max_texture: f32,
    copy: bool,
    /// Effect layers of the window that the compositor plays, so the frame leaves them out.
    pub skip: Vec<usize>,
    /// Of those, the layers whose content `record` draws into a target of its own, listed after it
    /// in `promoted` with that target and their `[x, y, width, height]` window region.
    pub content: Vec<usize>,
    pub promoted: Vec<(usize, Target, [i32; 4])>,
}

/// Copies the primitives of `keys` in their order.
fn extend<T: Copy>(items: &mut Vec<T>, primitives: &[T], keys: &[SortKey]) -> Range<u32> {
    let start = items.len() as u32;
    items.extend(keys.iter().map(|key| primitives[key[2] as usize]));
    start..items.len() as u32
}

/// The whole device pixels that cover `bounds`, as `[x, y, width, height]`.
pub fn region(bounds: Bounds<ScaledPixels>) -> [i32; 4] {
    let [x, y] = [bounds.origin.x.0.floor(), bounds.origin.y.0.floor()];
    let [right, bottom] = [
        (bounds.origin.x.0 + bounds.size.width.0).ceil(),
        (bounds.origin.y.0 + bounds.size.height.0).ceil(),
    ];
    [x as i32, y as i32, (right - x) as i32, (bottom - y) as i32]
}

fn rect(bounds: Bounds<ScaledPixels>) -> [f32; 4] {
    [
        bounds.origin.x.0,
        bounds.origin.y.0,
        bounds.size.width.0,
        bounds.size.height.0,
    ]
}

fn radii(radii: &crate::Radii<ScaledPixels>) -> [f32; 8] {
    let (x, y) = (&radii.x, &radii.y);
    [
        x.top_left.0,
        x.top_right.0,
        x.bottom_right.0,
        x.bottom_left.0,
        y.top_left.0,
        y.top_right.0,
        y.bottom_right.0,
        y.bottom_left.0,
    ]
}

pub fn bytes<T>(items: &[T]) -> &[u8] {
    // SAFETY: the instance types are `repr(C)` plain data that the shaders read as raw bytes.
    unsafe { std::slice::from_raw_parts(items.as_ptr() as *const u8, mem::size_of_val(items)) }
}

impl RenderFrame {
    /// Records the scene for a `window` sized frame. Backdrops and blend modes need `copy`,
    /// the support for copying from the window texture.
    pub fn record(&mut self, scene: &Scene, window: [u32; 2], max_texture: u32, copy: bool) {
        self.quads.clear();
        self.shadows.clear();
        self.underlines.clear();
        self.mono.clear();
        self.poly.clear();
        self.composites.clear();
        self.blurs.clear();
        self.cmds.clear();
        self.layers.clear();
        self.textures.clear();
        self.free.clear();
        self.shelves.clear();
        self.window = window;
        self.max_texture = max_texture as f32;
        self.copy = copy;
        self.record_layer(scene, 0, Target::Frame, true, 0);
        self.promoted.clear();
        for index in 0..self.content.len() {
            let layer = self.content[index];
            let Some(bounds) = scene.layers[layer].bounds else {
                continue;
            };
            let region = region(bounds);
            let target = self.acquire(region);
            self.record_layer(scene, layer, target, true, 1);
            self.promoted.push((layer, target, region));
        }
        // WARNING: shelf textures grow while the frame is recorded, so composites read their final size.
        if !self.shelves.is_empty() {
            for cmd in 0..self.cmds.len() {
                if let Cmd::Composite { src, index, .. } = self.cmds[cmd] {
                    let rect = self.rect(src);
                    self.composites[index as usize].source_rect = rect;
                }
            }
        }
    }

    /// The pooled texture of a target, `None` for the window.
    pub fn texture(&self, target: Target) -> Option<usize> {
        match target {
            Target::Frame => None,
            Target::Layer(index) => Some(self.layers[index].texture),
        }
    }

    /// The `[x, y, width, height]` window region the target covers.
    pub fn rect(&self, target: Target) -> [f32; 4] {
        match target {
            Target::Frame => [0., 0., self.window[0] as f32, self.window[1] as f32],
            Target::Layer(index) => {
                let layer = &self.layers[index];
                let size = self.textures[layer.texture];
                [
                    layer.origin[0] as f32,
                    layer.origin[1] as f32,
                    size[0] as f32 * layer.scale,
                    size[1] as f32 * layer.scale,
                ]
            }
        }
    }

    /// The size of the target in its own pixels.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn pixels(&self, target: Target) -> [f32; 2] {
        match target {
            Target::Frame => self.window.map(|value| value as f32),
            Target::Layer(index) => self.textures[self.layers[index].texture].map(|v| v as f32),
        }
    }

    pub fn origin(&self, target: Target) -> [i32; 2] {
        match target {
            Target::Frame => [0, 0],
            Target::Layer(index) => self.layers[index].origin,
        }
    }

    pub fn globals(&self, target: Target, text: &TextParams) -> Globals {
        let rect = self.rect(target);
        Globals {
            gamma_ratios: text.gamma_ratios,
            viewport_size: [rect[2], rect[3]],
            viewport_origin: [rect[0], rect[1]],
            window_size: [self.window[0] as f32, self.window[1] as f32],
            grayscale_enhanced_contrast: text.grayscale_enhanced_contrast,
            subpixel_enhanced_contrast: text.subpixel_enhanced_contrast,
            premultiplied_alpha: 0,
            is_bgr: text.is_bgr as u32,
            pad: [0; 2],
        }
    }

    /// Returns a target covering the `[x, y, width, height]` region of the window.
    fn acquire(&mut self, region: [i32; 4]) -> Target {
        self.acquire_scaled(region, 1)
    }

    /// Like `acquire`, with `scale` window pixels per texture pixel.
    fn acquire_scaled(&mut self, region: [i32; 4], scale: i32) -> Target {
        let size = [region[2], region[3]].map(|side| (side.max(1) as u32).div_ceil(scale as u32));
        let fit = self
            .free
            .iter()
            .enumerate()
            .filter(|(_, slot)| {
                let texture = self.textures[**slot];
                texture[0] >= size[0] && texture[1] >= size[1]
            })
            .min_by_key(|(_, slot)| {
                let texture = self.textures[**slot];
                texture[0] * texture[1]
            })
            .map(|(index, _)| index);
        let texture = match fit {
            Some(index) => self.free.swap_remove(index),
            None => {
                // Round up so textures can be reused by later frames.
                let max = self.max_texture as u32;
                self.textures
                    .push(size.map(|value| (value.div_ceil(256) * 256).min(max)));
                self.textures.len() - 1
            }
        };
        let size = self.textures[texture];
        self.layers.push(Layer {
            texture,
            origin: [region[0], region[1]],
            scale: scale as f32,
            bounds: [
                region[0] as f32,
                region[1] as f32,
                (size[0] * scale as u32) as f32,
                (size[1] * scale as u32) as f32,
            ],
        });
        Target::Layer(self.layers.len() - 1)
    }

    /// Returns a tile of the shared texture of `depth` for `region`, and whether the texture must
    /// be cleared first. `None` when the region is too big for a tile.
    fn tile(&mut self, region: [i32; 4], target: Target, depth: usize) -> Option<(Target, bool)> {
        let size = self
            .window
            .map(|value| (value.div_ceil(256) * 256).min(self.max_texture as u32) as i32);
        // A transparent pixel around each tile keeps filtering from reading the next tile.
        let (w, h) = (region[2] + 2, region[3] + 2);
        if w > size[0] / 2 || h > size[1] / 2 {
            return None;
        }
        if self.shelves.len() <= depth {
            self.shelves.resize_with(depth + 1, || None);
        }
        let mut clear = false;
        let shelf = match &mut self.shelves[depth] {
            Some(shelf) => shelf,
            slot => {
                self.textures.push([0, 0]);
                clear = true;
                slot.insert(Shelf {
                    texture: self.textures.len() - 1,
                    x: 0,
                    y: 0,
                    row: 0,
                })
            }
        };
        if shelf.x + w > size[0] {
            (shelf.x, shelf.y, shelf.row) = (0, shelf.y + shelf.row, 0);
        }
        if shelf.y + h > size[1] {
            // WARNING: the waiting tiles must be drawn before the texture is cleared for reuse.
            (shelf.x, shelf.y, shelf.row) = (0, 0, 0);
            clear = true;
            self.flush(target, depth);
        }
        let shelf = self.shelves[depth].as_mut()?;
        let offset = [shelf.x + 1, shelf.y + 1];
        shelf.x += w;
        shelf.row = shelf.row.max(h);
        let (texture, used) = (shelf.texture, [shelf.x, shelf.y + shelf.row]);
        let grown = &mut self.textures[texture];
        *grown = [0, 1].map(|axis| grown[axis].max((used[axis] as u32).div_ceil(256) * 256));
        self.layers.push(Layer {
            texture,
            origin: [region[0] - offset[0], region[1] - offset[1]],
            scale: 1.,
            bounds: region.map(|value| value as f32),
        });
        Some((Target::Layer(self.layers.len() - 1), clear))
    }

    /// Composites the waiting tiles of `depth` into `target`.
    fn flush(&mut self, target: Target, depth: usize) {
        let Some(deferred) = self
            .deferred
            .get_mut(depth)
            .filter(|deferred| !deferred.is_empty())
        else {
            return;
        };
        self.cmds.push(Cmd::Pass(target, false));
        for (src, mask, composite) in deferred.drain(..) {
            self.cmds.push(Cmd::Composite {
                src,
                mask,
                backdrop: None,
                index: self.composites.len() as u32,
            });
            self.composites.push(composite);
        }
    }

    // WARNING: commands run in order, so a texture can be reused once the commands reading it are recorded.
    fn release(&mut self, target: Target) {
        if let Target::Layer(index) = target {
            self.free.push(self.layers[index].texture);
        }
    }

    fn record_layer(
        &mut self,
        scene: &Scene,
        layer: usize,
        target: Target,
        clear: bool,
        depth: usize,
    ) {
        self.cmds.push(Cmd::Pass(target, clear));
        let primitives = &scene.layers[layer];
        for batch in scene.batches(layer) {
            let cmd = match batch {
                Batch::Quads(keys) => Cmd::Quads(extend(&mut self.quads, &primitives.quads, keys)),
                Batch::Shadows(keys) => {
                    Cmd::Shadows(extend(&mut self.shadows, &primitives.shadows, keys))
                }
                Batch::Underlines(keys) => {
                    Cmd::Underlines(extend(&mut self.underlines, &primitives.underlines, keys))
                }
                Batch::Mono(texture, keys) => {
                    Cmd::Mono(texture, extend(&mut self.mono, &primitives.mono, keys))
                }
                Batch::Poly(texture, keys) => {
                    Cmd::Poly(texture, extend(&mut self.poly, &primitives.poly, keys))
                }
                Batch::Effects(keys) => {
                    for key in keys {
                        let primitive = &primitives.effects[key[2] as usize];
                        if layer == 0 && self.skip.contains(&primitive.layer) {
                            continue;
                        }
                        self.record_effect(scene, primitive, target, depth);
                    }
                    self.flush(target, depth);
                    continue;
                }
            };
            self.cmds.push(cmd);
        }
    }

    /// The integer `[x, y, width, height]` region covering `bounds`, limited to what can be seen
    /// and to the maximum texture size with a border pixel on each side.
    fn region(&self, bounds: Bounds<ScaledPixels>, limit: [f32; 4]) -> Option<[i32; 4]> {
        let max = self.max_texture - 2.;
        let x0 = bounds.origin.x.0.floor().max(limit[0]);
        let y0 = bounds.origin.y.0.floor().max(limit[1]);
        let x1 = (bounds.origin.x.0 + bounds.size.width.0)
            .ceil()
            .min(limit[0] + limit[2])
            .min(x0 + max);
        let y1 = (bounds.origin.y.0 + bounds.size.height.0)
            .ceil()
            .min(limit[1] + limit[3])
            .min(y0 + max);
        (x1 > x0 && y1 > y0).then_some([x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32])
    }

    /// Blurs `region` of `src` into a new target. Returns it with the sigma that the composite
    /// still blurs vertically: a narrow blur leaves that half to the composite, while a wide one
    /// blurs both ways at half resolution, which the composite scales up.
    fn blur(&mut self, src: Target, region: [i32; 4], sigma: f32, release: bool) -> (Target, f32) {
        let covered = match src {
            Target::Layer(_) => self.rect(src).map(|value| value as i32),
            Target::Frame => region,
        };
        let scale = if sigma >= HALF_RESOLUTION_SIGMA { 2 } else { 1 };
        let horizontal = self.acquire_scaled(covered, scale);
        self.blur_pass(src, horizontal, region, sigma, [1., 0.]);
        if release {
            self.release(src);
        }
        if scale == 1 {
            return (horizontal, sigma);
        }
        let both = self.acquire_scaled(covered, scale);
        self.blur_pass(horizontal, both, region, sigma, [0., 1.]);
        self.release(horizontal);
        (both, 0.)
    }

    fn blur_pass(
        &mut self,
        src: Target,
        dst: Target,
        region: [i32; 4],
        sigma: f32,
        direction: [f32; 2],
    ) {
        let index = self.blurs.len() as u32;
        self.blurs.push(Blur {
            region: region.map(|value| value as f32),
            source_rect: self.rect(src),
            direction,
            sigma,
            scale: match dst {
                Target::Layer(index) => self.layers[index].scale,
                Target::Frame => 1.,
            },
        });
        self.cmds.push(Cmd::Blur { src, dst, index });
    }

    fn composite(
        &mut self,
        target: Target,
        src: Target,
        mask: Option<AtlasTextureId>,
        backdrop: Option<Target>,
        composite: Composite,
    ) {
        self.cmds.push(Cmd::Pass(target, false));
        self.cmds.push(Cmd::Composite {
            src,
            mask,
            backdrop,
            index: self.composites.len() as u32,
        });
        self.composites.push(composite);
    }

    fn defer(&mut self, depth: usize, composite: Deferred) {
        if self.deferred.len() <= depth {
            self.deferred.resize_with(depth + 1, Vec::new);
        }
        self.deferred[depth].push(composite);
    }

    fn record_effect(
        &mut self,
        scene: &Scene,
        primitive: &EffectPrimitive,
        target: Target,
        depth: usize,
    ) {
        let layer = &scene.layers[primitive.layer];
        let Some(effect) = layer.effect.as_ref() else {
            return;
        };
        let mask = primitive.content_mask.bounds;
        let matrix = |matrix: &Option<[f32; 20]>, composite: &mut Composite| {
            if let Some(m) = matrix {
                composite.flags |= COMPOSITE_MATRIX;
                composite.color_matrix = [
                    m[0], m[5], m[10], m[15], m[1], m[6], m[11], m[16], m[2], m[7], m[12], m[17],
                    m[3], m[8], m[13], m[18],
                ];
                composite.color_offset = [m[4], m[9], m[14], m[19]];
            }
        };
        let base = Composite {
            transform: IDENTITY,
            color_matrix: IDENTITY,
            color_offset: [0.; 4],
            shadow_color: [0.; 4],
            bounds: [0.; 4],
            clip: [0.; 4],
            clip_radii: [0.; 8],
            mask_rect: [0.; 4],
            mask_tile: [0.; 4],
            content_mask: rect(mask),
            rounded_mask: rect(primitive.content_mask.rounded),
            rounded_radii: radii(&primitive.content_mask.corner_radii),
            shadow: [0.; 4],
            source_rect: [0.; 4],
            backdrop_rect: [0.; 4],
            opacity: 1.,
            blur: 0.,
            flags: 0,
            blend: 0,
        };
        let target_rect = match target {
            Target::Frame => self.rect(target),
            Target::Layer(index) => self.layers[index].bounds,
        };
        let visible = rect(mask.intersect(&Bounds::new(
            Point::new(ScaledPixels(target_rect[0]), ScaledPixels(target_rect[1])),
            Size::new(ScaledPixels(target_rect[2]), ScaledPixels(target_rect[3])),
        )));

        let copies =
            self.copy && (effect.backdrop.is_some() || effect.blend_mode != BlendMode::Normal);
        let blurs = effect.blur > 0.25 || effect.shadow.is_some_and(|(_, sigma, _)| sigma > 0.25);
        if copies || blurs {
            self.flush(target, depth);
        }
        if let Some(backdrop) = &effect.backdrop
            && self.copy
            && let Some(region) = self.region(
                backdrop.bounds.dilate(ScaledPixels(backdrop.blur * 3.)),
                visible,
            )
        {
            let copy = self.acquire(region);
            self.cmds.push(Cmd::Copy {
                src: target,
                dst: copy,
                region,
            });
            let (src, sigma) = if backdrop.blur > 0.25 {
                self.blur(copy, region, backdrop.blur, true)
            } else {
                (copy, 0.)
            };
            let mut composite = base;
            composite.flags = COMPOSITE_BACKDROP | COMPOSITE_CLIP;
            composite.opacity = effect.opacity;
            composite.bounds = rect(backdrop.bounds);
            composite.clip = composite.bounds;
            composite.clip_radii = radii(&backdrop.corner_radii);
            composite.blur = sigma;
            composite.source_rect = self.rect(src);
            matrix(&backdrop.color_matrix, &mut composite);
            self.composite(target, src, None, None, composite);
            self.release(src);
        }

        let Some(content) = layer.bounds else {
            return;
        };
        // WARNING: transformed content can be visible beyond the window, so allow a window of margin.
        let (w, h) = (self.window[0] as f32, self.window[1] as f32);
        let reach = if effect.transform.is_some() {
            [-w, -h, 3. * w, 3. * h]
        } else {
            visible
        };
        let spread = effect
            .blur
            .max(effect.shadow.map_or(0., |(_, sigma, _)| sigma))
            * 3.;
        let content = content.dilate(ScaledPixels(spread));
        let Some(region) = self.region(content, reach) else {
            return;
        };
        // Tiles must hold all their content, since drawing past a tile would reach the next one.
        let whole = Some(region)
            == self.region(content, [f32::MIN, f32::MIN, f32::INFINITY, f32::INFINITY]);
        let tile = (!copies && !blurs && whole)
            .then(|| self.tile(region, target, depth))
            .flatten();
        if tile.is_none() {
            self.flush(target, depth);
        }
        let texture = match tile {
            Some((tile, clear)) => {
                self.record_layer(scene, primitive.layer, tile, clear, depth + 1);
                tile
            }
            None => {
                // A transparent pixel around the content keeps the clamped edges smooth.
                let texture =
                    self.acquire([region[0] - 1, region[1] - 1, region[2] + 2, region[3] + 2]);
                self.record_layer(scene, primitive.layer, texture, true, depth + 1);
                texture
            }
        };

        let mut base = base;
        base.transform = effect.transform.unwrap_or(IDENTITY);
        base.opacity = effect.opacity;
        if let Some((bounds, corners)) = &effect.clip {
            base.flags |= COMPOSITE_CLIP;
            base.clip = rect(*bounds);
            base.clip_radii = radii(corners);
        }
        let mask_texture = effect.mask.as_ref().map(|(tile, bounds)| {
            base.flags |= COMPOSITE_MASK;
            base.mask_rect = rect(*bounds);
            base.mask_tile = [
                tile.bounds.origin.x.0 as f32,
                tile.bounds.origin.y.0 as f32,
                tile.bounds.size.width.0 as f32,
                tile.bounds.size.height.0 as f32,
            ];
            tile.texture_id
        });
        let region_rect = region.map(|value| value as f32);

        if let Some((offset, sigma, color)) = effect.shadow {
            let (src, left) = if sigma > 0.25 {
                self.blur(texture, region, sigma, false)
            } else {
                (texture, 0.)
            };
            let rgba = color;
            let mut composite = base;
            composite.flags |= COMPOSITE_SHADOW;
            composite.blur = left;
            composite.shadow = [offset.x, offset.y, sigma, 0.];
            composite.shadow_color = [rgba.r * rgba.a, rgba.g * rgba.a, rgba.b * rgba.a, rgba.a];
            composite.bounds = [
                region_rect[0] + offset.x,
                region_rect[1] + offset.y,
                region_rect[2],
                region_rect[3],
            ];
            composite.source_rect = self.rect(src);
            if tile.is_some() {
                self.defer(depth, (src, mask_texture, composite));
            } else {
                self.composite(target, src, mask_texture, None, composite);
            }
            if src != texture {
                self.release(src);
            }
        }

        let (src, sigma) = if effect.blur > 0.25 {
            self.blur(texture, region, effect.blur, true)
        } else {
            (texture, 0.)
        };
        let backdrop = (effect.blend_mode != BlendMode::Normal && self.copy)
            .then(|| self.region(primitive.bounds, visible))
            .flatten()
            .map(|region| {
                let copy = self.acquire(region);
                self.cmds.push(Cmd::Copy {
                    src: target,
                    dst: copy,
                    region,
                });
                copy
            });
        let mut composite = base;
        matrix(&effect.color_matrix, &mut composite);
        composite.blur = sigma;
        composite.bounds = region_rect;
        composite.blend = effect.blend_mode as u32;
        composite.source_rect = self.rect(src);
        if let Some(backdrop) = backdrop {
            composite.flags |= COMPOSITE_BLEND;
            composite.backdrop_rect = self.rect(backdrop);
        }
        if tile.is_some() {
            self.defer(depth, (src, mask_texture, composite));
            return;
        }
        self.composite(target, src, mask_texture, backdrop, composite);
        self.release(src);
        if let Some(backdrop) = backdrop {
            self.release(backdrop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Blur, Cmd, Composite, Globals, RenderFrame};
    use crate::{
        Bounds, ContentMask, ScaledPixels, Size,
        scene::{
            EffectState, MonochromeSprite, PolychromeSprite, Primitive, Quad, Scene, Shadow,
            Underline,
        },
    };
    use std::mem::size_of;

    /// A frame of one `opacity` group around a quad at `[x, y, width, height]`, scaled by `scale`.
    fn record(window: [u32; 2], quad: [f32; 4], scale: f32) -> RenderFrame {
        let bounds = |[x, y, w, h]: [f32; 4]| {
            Bounds::new(
                crate::point(ScaledPixels(x), ScaledPixels(y)),
                Size::new(ScaledPixels(w), ScaledPixels(h)),
            )
        };
        let mask = ContentMask {
            bounds: bounds([-1e6, -1e6, 2e6, 2e6]),
            ..Default::default()
        };
        let mut transform = super::IDENTITY;
        transform[0] = scale;
        let mut scene = Scene::default();
        let state = EffectState {
            transform: (scale != 1.).then_some(transform),
            opacity: 0.5,
            color_matrix: None,
            blur: 0.,
            shadow: None,
            blend_mode: Default::default(),
            clip: None,
            mask: None,
            backdrop: None,
            content_mask: mask,
        };
        scene.push_effect(state, Some(7));
        scene.insert(Primitive::Quad(Quad {
            bounds: bounds(quad),
            content_mask: mask,
            ..Default::default()
        }));
        scene.pop_effect();
        scene.finish();
        let mut frame = RenderFrame::default();
        frame.record(&scene, window, 16384, true);
        assert!(scene.patch(7, 0.25, None) && !scene.patch(8, 0.25, None));
        let mut patched = RenderFrame::default();
        patched.record(&scene, window, 16384, true);
        assert!(
            patched
                .composites
                .iter()
                .any(|composite| composite.opacity == 0.25)
        );
        frame
    }

    #[test]
    fn patched_transforms_draw_like_new_frames() {
        let bounds = |[x, y, w, h]: [f32; 4]| {
            Bounds::new(
                crate::point(ScaledPixels(x), ScaledPixels(y)),
                Size::new(ScaledPixels(w), ScaledPixels(h)),
            )
        };
        let mask = ContentMask {
            bounds: bounds([0., 0., 800., 600.]),
            ..Default::default()
        };
        let turn = |degrees: f32| {
            let (sin, cos) = degrees.to_radians().sin_cos();
            let mut m = super::IDENTITY;
            [m[0], m[1], m[4], m[5]] = [cos, sin, -sin, cos];
            [m[12], m[13]] = [
                150. - cos * 150. + sin * 150.,
                150. - sin * 150. - cos * 150.,
            ];
            m
        };
        let scene = |degrees: f32| {
            let mut scene = Scene::default();
            let state = EffectState {
                transform: Some(turn(degrees)),
                opacity: 1.,
                color_matrix: None,
                blur: 0.,
                shadow: None,
                blend_mode: Default::default(),
                clip: None,
                mask: None,
                backdrop: None,
                content_mask: mask,
            };
            scene.push_effect(state, Some(9));
            scene.insert(Primitive::Quad(Quad {
                bounds: bounds([100., 100., 100., 100.]),
                content_mask: mask,
                ..Default::default()
            }));
            scene.pop_effect();
            // Covers the layer only once it turns, and must still draw over it.
            scene.insert(Primitive::Quad(Quad {
                bounds: bounds([205., 140., 25., 20.]),
                content_mask: mask,
                ..Default::default()
            }));
            scene.finish();
            scene
        };
        let draw = |scene: &Scene| {
            let mut frame = RenderFrame::default();
            frame.record(scene, [800, 600], 16384, true);
            let order: Vec<_> = frame
                .cmds
                .iter()
                .filter_map(|cmd| match cmd {
                    Cmd::Quads(range) => Some(format!("quads {range:?}")),
                    Cmd::Composite { index, .. } => Some(format!("composite {index}")),
                    _ => None,
                })
                .collect();
            (
                order,
                super::bytes(&frame.quads).to_vec(),
                super::bytes(&frame.composites).to_vec(),
            )
        };
        let mut patched = scene(0.);
        assert!(patched.patch(9, 1., Some(turn(30.))));
        assert_eq!(draw(&patched), draw(&scene(30.)));
    }

    #[test]
    fn wide_blurs_draw_both_passes_at_half_resolution() {
        let blurred = |sigma: f32| {
            let bounds = |[x, y, w, h]: [f32; 4]| {
                Bounds::new(
                    crate::point(ScaledPixels(x), ScaledPixels(y)),
                    Size::new(ScaledPixels(w), ScaledPixels(h)),
                )
            };
            let mask = ContentMask {
                bounds: bounds([0., 0., 800., 600.]),
                ..Default::default()
            };
            let mut scene = Scene::default();
            scene.push_effect(
                EffectState {
                    transform: None,
                    opacity: 1.,
                    color_matrix: None,
                    blur: sigma,
                    shadow: None,
                    blend_mode: Default::default(),
                    clip: None,
                    mask: None,
                    backdrop: None,
                    content_mask: mask,
                },
                None,
            );
            scene.insert(Primitive::Quad(Quad {
                bounds: bounds([100., 100., 200., 100.]),
                content_mask: mask,
                ..Default::default()
            }));
            scene.pop_effect();
            scene.finish();
            let mut frame = RenderFrame::default();
            frame.record(&scene, [800, 600], 16384, true);
            frame
        };
        let narrow = blurred(4.);
        let [blur] = narrow.blurs[..] else {
            panic!("one blur pass")
        };
        assert_eq!((blur.direction, blur.scale), ([1., 0.], 1.));
        assert_eq!(narrow.composites[0].blur, 4.);
        let wide = blurred(20.);
        let [across, down] = wide.blurs[..] else {
            panic!("two blur passes")
        };
        assert_eq!((across.direction, across.scale), ([1., 0.], 2.));
        assert_eq!((down.direction, down.scale), ([0., 1.], 2.));
        // The composite only scales up the half-size result, which covers the same window region.
        let composite = wide.composites[0];
        assert_eq!(composite.blur, 0.);
        let Some(Cmd::Composite { src, .. }) = wide.cmds.last() else {
            panic!("a composite last")
        };
        let [width, height] = wide.pixels(*src);
        assert_eq!(composite.source_rect[2..], [width * 2., height * 2.]);
    }

    #[test]
    fn small_tiles_get_a_small_texture() {
        let frame = record([3072, 2048], [100., 100., 24., 24.], 1.);
        assert_eq!(frame.textures, [[256, 256]]);
        let source = frame.cmds.iter().find_map(|cmd| match cmd {
            Cmd::Composite { index, .. } => Some(frame.composites[*index as usize].source_rect),
            _ => None,
        });
        assert_eq!(source.map(|rect| [rect[2], rect[3]]), Some([256., 256.]));
    }

    #[test]
    fn wide_layers_fit_the_maximum_texture() {
        let frame = record([6144, 3456], [-2048., 0., 8000., 100.], 3.);
        assert!(!frame.textures.is_empty());
        assert!(frame.textures.iter().flatten().all(|side| *side <= 16384));
    }

    #[test]
    fn shaders_match_instance_layouts() {
        let module =
            naga::front::wgsl::parse_str(include_str!("platform/linux/shaders.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).unwrap();
        for (name, size) in [
            ("Globals", size_of::<Globals>()),
            ("Quad", size_of::<Quad>()),
            ("Shadow", size_of::<Shadow>()),
            ("Underline", size_of::<Underline>()),
            ("MonochromeSprite", size_of::<MonochromeSprite>()),
            ("PolychromeSprite", size_of::<PolychromeSprite>()),
            ("Composite", size_of::<Composite>()),
            ("Blur", size_of::<Blur>()),
        ] {
            let (handle, _) = module
                .types
                .iter()
                .find(|(_, ty)| ty.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("missing struct {name}"));
            assert_eq!(layouter[handle].size as usize, size, "{name}");
        }
    }
}
