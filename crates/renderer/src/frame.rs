use crate::{
    BlendMode, Point, Rect, Size,
    gpu::{
        BOX, Blur, COMPOSITE_BACKDROP, COMPOSITE_BLEND, COMPOSITE_CLIP, COMPOSITE_MASK,
        COMPOSITE_MATRIX, COMPOSITE_SHADOW, Composite, Globals, Instance,
    },
    scene::{Nested, Scene},
};
use std::ops::Range;

pub(crate) const IDENTITY: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

const HALF_RESOLUTION_SIGMA: f32 = 8.;

#[derive(Clone, Copy, PartialEq)]
pub enum Target {
    Frame,
    Layer(usize),
}

pub struct Layer {
    pub texture: usize,
    pub origin: [i32; 2],
    scale: f32,
    bounds: [f32; 4],
}

struct Shelf {
    texture: usize,
    x: i32,
    y: i32,
    row: i32,
}

type Deferred = (Target, Option<Mask>, Composite);

#[derive(Clone, Copy)]
pub enum Mask {
    // A color atlas page.
    Page(u32),
    Layer(Target),
}

pub enum Command {
    Pass(Target, bool),
    Draw {
        instances: Range<u32>,
        mask_page: Option<u32>,
        color_page: Option<u32>,
    },
    Blur {
        src: Target,
        dst: Target,
        index: u32,
    },
    Copy {
        src: Target,
        dst: Target,
        region: [i32; 4],
    },
    Composite {
        src: Target,
        mask: Option<Mask>,
        backdrop: Option<Target>,
        index: u32,
    },
}

#[derive(Default)]
pub struct RenderFrame {
    pub(crate) instances: Vec<Instance>,
    pub composites: Vec<Composite>,
    pub blurs: Vec<Blur>,
    pub commands: Vec<Command>,
    pub layers: Vec<Layer>,
    pub textures: Vec<[u32; 2]>,
    free: Vec<usize>,
    shelves: Vec<Option<Shelf>>,
    deferred: Vec<Vec<Deferred>>,
    window: [u32; 2],
    max_texture: f32,
    copy: bool,
    pub skip: Vec<usize>,
    pub content: Vec<usize>,
    pub promoted: Vec<(usize, Target, [i32; 4])>,
    // The snapshot layers to read: the key, the target and the region that it holds.
    pub shots: Vec<(u64, Target, [i32; 4])>,
}

// The layer textures of a backend. A texture that no frame used for this many frames is dropped.
const SPARE_FRAMES: u8 = 3;

pub(crate) struct Pool<T> {
    textures: Vec<(T, [u32; 2], u8)>,
    slots: Vec<usize>,
}

impl<T> Default for Pool<T> {
    fn default() -> Self {
        Self {
            textures: Vec::new(),
            slots: Vec::new(),
        }
    }
}

impl<T> Pool<T> {
    pub fn fill(
        &mut self,
        frame: &RenderFrame,
        mut make: impl FnMut([u32; 2]) -> anyhow::Result<T>,
    ) -> anyhow::Result<()> {
        for (.., idle) in &mut self.textures {
            *idle = idle.saturating_add(1);
        }
        self.textures.retain(|(.., idle)| *idle <= SPARE_FRAMES);
        self.slots.clear();
        for &size in &frame.textures {
            let index = match self
                .textures
                .iter()
                .position(|(_, kept, idle)| *idle > 0 && *kept == size)
            {
                Some(index) => index,
                None => {
                    self.textures.push((make(size)?, size, 0));
                    self.textures.len() - 1
                }
            };
            self.textures[index].2 = 0;
            self.slots.push(index);
        }
        Ok(())
    }

    pub fn get(&self, frame: &RenderFrame, target: Target) -> Option<&T> {
        let Target::Layer(index) = target else {
            return None;
        };
        Some(&self.textures[self.slots[frame.layers[index].texture]].0)
    }

    pub fn clear(&mut self) {
        self.textures.clear();
        self.slots.clear();
    }
}

pub fn region(bounds: Rect<f32>) -> [i32; 4] {
    let [x, y] = [bounds.origin.x.floor(), bounds.origin.y.floor()];
    let [right, bottom] = [
        (bounds.origin.x + bounds.size.width).ceil(),
        (bounds.origin.y + bounds.size.height).ceil(),
    ];
    [x as i32, y as i32, (right - x) as i32, (bottom - y) as i32]
}

impl RenderFrame {
    pub fn record(&mut self, scene: &Scene, window: [u32; 2], max_texture: u32, copy: bool) {
        self.instances.clear();
        self.composites.clear();
        self.blurs.clear();
        self.commands.clear();
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
            let target = self.acquire(region, 1);
            self.record_layer(scene, layer, target, true, 1);
            self.promoted.push((layer, target, region));
        }
        self.shots.clear();
        for shot in scene.shots.iter().filter(|shot| shot.read) {
            let [x, y, width, height] = shot.region;
            let region = [
                x,
                y,
                width.min(max_texture as i32),
                height.min(max_texture as i32),
            ];
            if region[2] > 0 && region[3] > 0 {
                let target = self.acquire(region, 1);
                self.record_layer(scene, shot.layer, target, true, 1);
                self.shots.push((shot.key, target, region));
            }
        }
        // Shelf textures grow while the frame is recorded, so composites read their final size.
        if !self.shelves.is_empty() {
            for command in 0..self.commands.len() {
                if let Command::Composite { src, index, .. } = self.commands[command] {
                    let rect = self.rect(src);
                    self.composites[index as usize].source_rect = rect;
                }
            }
        }
    }

    pub fn texture(&self, target: Target) -> Option<usize> {
        match target {
            Target::Frame => None,
            Target::Layer(index) => Some(self.layers[index].texture),
        }
    }

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

    #[cfg(any(windows, test))]
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

    pub fn globals(&self, target: Target, text_contrast: f32) -> Globals {
        Globals {
            viewport: self.rect(target),
            text_contrast,
            pad: [0.; 3],
        }
    }

    fn acquire(&mut self, region: [i32; 4], scale: i32) -> Target {
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
            // The waiting tiles must be drawn before the texture is cleared for reuse.
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

    fn flush(&mut self, target: Target, depth: usize) {
        let Some(deferred) = self
            .deferred
            .get_mut(depth)
            .filter(|deferred| !deferred.is_empty())
        else {
            return;
        };
        self.commands.push(Command::Pass(target, false));
        for (src, mask, composite) in deferred.drain(..) {
            self.commands.push(Command::Composite {
                src,
                mask,
                backdrop: None,
                index: self.composites.len() as u32,
            });
            self.composites.push(composite);
        }
    }

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
        self.commands.push(Command::Pass(target, clear));
        let content = &scene.layers[layer];
        // Skip what a later opaque box hides, since stacked page backgrounds can fill the window many
        // times. An effect that reads the target could see the hidden pixels, so then nothing is skipped.
        let mut occluders: Vec<(usize, Rect<f32>, f32)> = Vec::new();
        if !content.nested.iter().any(|nested| {
            scene.layers[nested.layer]
                .effect
                .as_ref()
                .is_some_and(|effect| {
                    effect.backdrop.is_some() || effect.blend_mode != BlendMode::Normal
                })
        }) {
            let min = self.window[0] as f32 * self.window[1] as f32 / 64.;
            occluders.extend(
                content
                    .instances
                    .iter()
                    .enumerate()
                    .filter(|(_, instance)| {
                        instance.kind() == BOX
                            && instance.color[3] >= 1.
                            && instance.aux == [0.; 4]
                            && instance.radii == [0.; 8]
                            && instance.round_radii == [0.; 8]
                    })
                    .filter_map(|(at, instance)| {
                        let bounds = instance.reach();
                        let area = bounds.size.width * bounds.size.height;
                        (area >= min).then_some((at, bounds, area))
                    }),
            );
            if occluders.len() > 16 {
                occluders.select_nth_unstable_by(16, |a, b| b.2.total_cmp(&a.2));
                occluders.truncate(16);
            }
            occluders.sort_unstable_by_key(|occluder| std::cmp::Reverse(occluder.0));
        }
        // With `trim`, an occluder that spans the box cuts off the side it covers. Cuts repeat until none
        // applies, since one cut can make another occluder span the rest.
        let visible = |at: usize, bounds: &mut Rect<f32>, trim: bool| loop {
            let before = *bounds;
            for (_, over, _) in occluders.iter().take_while(|occluder| occluder.0 > at) {
                let (left, top, right, bottom) = (
                    bounds.origin.x,
                    bounds.origin.y,
                    bounds.right(),
                    bounds.bottom(),
                );
                let across = over.origin.x <= left && over.right() >= right;
                let down = over.origin.y <= top && over.bottom() >= bottom;
                if across && down {
                    return false;
                }
                if !trim || !(across || down) {
                    continue;
                }
                let [mut x0, mut y0, mut x1, mut y1] = [left, top, right, bottom];
                if down && over.origin.x <= left && over.right() > left {
                    x0 = over.right();
                } else if down && over.right() >= right && over.origin.x < right {
                    x1 = over.origin.x;
                } else if across && over.origin.y <= top && over.bottom() > top {
                    y0 = over.bottom();
                } else if across && over.bottom() >= bottom && over.origin.y < bottom {
                    y1 = over.origin.y;
                }
                *bounds = Rect::between(Point::new(x0, y0), Point::new(x1, y1));
            }
            if *bounds == before {
                return true;
            }
        };
        let mut nested = content.nested.iter().peekable();
        let mut start = self.instances.len() as u32;
        let mut pages = [None; 2];
        for (at, instance) in content.instances.iter().enumerate() {
            if nested.peek().is_some_and(|nested| nested.at as usize <= at) {
                self.draw(&mut start, &mut pages);
                while let Some(nested) = nested.next_if(|nested| nested.at as usize <= at) {
                    self.record_nested(scene, layer, nested, target, depth);
                }
                self.flush(target, depth);
                start = self.instances.len() as u32;
            }
            let mut instance = *instance;
            let mut bounds = instance.reach();
            let trim = instance.kind() == BOX && instance.round_radii == [0.; 8];
            if !visible(at, &mut bounds, trim) {
                continue;
            }
            if trim {
                instance.clip = bounds.to_array();
            }
            // A draw binds one page of each kind.
            if let Some((color, page)) = instance.page() {
                if pages[color as usize].is_some_and(|bound| bound != page) {
                    let other = pages[!color as usize];
                    self.draw(&mut start, &mut pages);
                    pages[!color as usize] = other;
                }
                pages[color as usize] = Some(page);
            }
            self.instances.push(instance);
        }
        self.draw(&mut start, &mut pages);
        if nested.peek().is_some() {
            for nested in nested {
                self.record_nested(scene, layer, nested, target, depth);
            }
            self.flush(target, depth);
        }
    }

    fn draw(&mut self, start: &mut u32, pages: &mut [Option<u32>; 2]) {
        let end = self.instances.len() as u32;
        if end > *start {
            self.commands.push(Command::Draw {
                instances: *start..end,
                mask_page: pages[0],
                color_page: pages[1],
            });
        }
        *start = end;
        *pages = [None; 2];
    }

    fn record_nested(
        &mut self,
        scene: &Scene,
        layer: usize,
        nested: &Nested,
        target: Target,
        depth: usize,
    ) {
        if layer != 0 || !self.skip.contains(&nested.layer) {
            self.record_effect(scene, nested, target, depth);
        }
    }

    fn region(&self, bounds: Rect<f32>, limit: [f32; 4]) -> Option<[i32; 4]> {
        let max = self.max_texture - 2.;
        let x0 = bounds.origin.x.floor().max(limit[0]);
        let y0 = bounds.origin.y.floor().max(limit[1]);
        let x1 = (bounds.origin.x + bounds.size.width)
            .ceil()
            .min(limit[0] + limit[2])
            .min(x0 + max);
        let y1 = (bounds.origin.y + bounds.size.height)
            .ceil()
            .min(limit[1] + limit[3])
            .min(y0 + max);
        (x1 > x0 && y1 > y0).then_some([x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32])
    }

    /// With `mirror`, samples outside `region` reflect into it, as backdrop-filter reads its backdrop.
    fn blur(
        &mut self,
        src: Target,
        region: [i32; 4],
        sigma: f32,
        release: bool,
        mirror: bool,
    ) -> (Target, f32) {
        let covered = match src {
            Target::Layer(_) => self.rect(src).map(|value| value as i32),
            Target::Frame => region,
        };
        let scale = if sigma >= HALF_RESOLUTION_SIGMA { 2 } else { 1 };
        let horizontal = self.acquire(covered, scale);
        self.blur_pass(src, horizontal, region, sigma, [1., 0.], mirror);
        if release {
            self.release(src);
        }
        if scale == 1 {
            return (horizontal, sigma);
        }
        let both = self.acquire(covered, scale);
        self.blur_pass(horizontal, both, region, sigma, [0., 1.], mirror);
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
        mirror: bool,
    ) {
        let index = self.blurs.len() as u32;
        let region = region.map(|value| value as f32);
        self.blurs.push(Blur {
            region,
            source_rect: self.rect(src),
            edge: if mirror { region } else { [0.; 4] },
            direction,
            sigma,
            scale: match dst {
                Target::Layer(index) => self.layers[index].scale,
                Target::Frame => 1.,
            },
        });
        self.commands.push(Command::Blur { src, dst, index });
    }

    fn composite(
        &mut self,
        target: Target,
        src: Target,
        mask: Option<Mask>,
        backdrop: Option<Target>,
        composite: Composite,
    ) {
        self.commands.push(Command::Pass(target, false));
        self.commands.push(Command::Composite {
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

    fn record_effect(&mut self, scene: &Scene, nested: &Nested, target: Target, depth: usize) {
        let layer = &scene.layers[nested.layer];
        let Some(effect) = layer.effect.as_ref() else {
            return;
        };
        let mask = nested.clip.bounds;
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
            content_mask: mask.to_array(),
            rounded_mask: nested.clip.rounded.to_array(),
            rounded_radii: nested.clip.radii.to_array(),
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
        let visible = mask
            .intersect(Rect::new(
                Point::new(target_rect[0], target_rect[1]),
                Size::new(target_rect[2], target_rect[3]),
            ))
            .to_array();

        let copies =
            self.copy && (effect.backdrop.is_some() || effect.blend_mode != BlendMode::Normal);
        let blurs = effect.blur > 0.25 || effect.shadow.is_some_and(|(_, sigma, _)| sigma > 0.25);
        if copies || blurs {
            self.flush(target, depth);
        }
        if let Some(backdrop) = &effect.backdrop
            && self.copy
            && let Some(region) = self.region(backdrop.bounds, visible)
        {
            let copy = self.acquire(region, 1);
            self.commands.push(Command::Copy {
                src: target,
                dst: copy,
                region,
            });
            let (src, sigma) = if backdrop.blur > 0.25 {
                self.blur(copy, region, backdrop.blur, true, true)
            } else {
                (copy, 0.)
            };
            let mut composite = base;
            composite.flags = COMPOSITE_BACKDROP | COMPOSITE_CLIP;
            composite.opacity = effect.opacity;
            composite.bounds = backdrop.bounds.to_array();
            composite.clip = composite.bounds;
            composite.clip_radii = backdrop.corner_radii.to_array();
            composite.blur = sigma;
            // A backdrop composite has no shadow, so `shadow` holds the edge for the blur.
            composite.shadow = region.map(|value| value as f32);
            composite.source_rect = self.rect(src);
            matrix(&backdrop.color_matrix, &mut composite);
            self.composite(target, src, None, None, composite);
            self.release(src);
        }

        let Some(content) = layer.bounds else {
            return;
        };
        // Without text, a text mask hides all.
        if effect
            .text_mask
            .is_some_and(|text| scene.layers[text].bounds.is_none())
        {
            return;
        }
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
        let content = content.outset(spread);
        let Some(region) = self.region(content, reach) else {
            return;
        };
        // Tiles must hold all their content, since drawing past a tile would reach the next one.
        let whole = Some(region)
            == self.region(content, [f32::MIN, f32::MIN, f32::INFINITY, f32::INFINITY]);
        let tile = (!copies && !blurs && whole && effect.text_mask.is_none())
            .then(|| self.tile(region, target, depth))
            .flatten();
        if tile.is_none() {
            self.flush(target, depth);
        }
        let text = effect.text_mask.map(|text| {
            let texture = self.acquire(region, 1);
            self.record_layer(scene, text, texture, true, depth + 1);
            texture
        });
        let texture = match tile {
            Some((tile, clear)) => {
                self.record_layer(scene, nested.layer, tile, clear, depth + 1);
                tile
            }
            None => {
                let texture = self.acquire(
                    [region[0] - 1, region[1] - 1, region[2] + 2, region[3] + 2],
                    1,
                );
                self.record_layer(scene, nested.layer, texture, true, depth + 1);
                texture
            }
        };

        let mut base = base;
        base.transform = effect.transform.unwrap_or(IDENTITY);
        base.opacity = effect.opacity;
        if let Some((bounds, corners)) = &effect.clip {
            base.flags |= COMPOSITE_CLIP;
            base.clip = bounds.to_array();
            base.clip_radii = corners.to_array();
        }
        let mask_texture = effect.mask.as_ref().map(|(tile, bounds)| {
            base.flags |= COMPOSITE_MASK;
            base.mask_rect = bounds.to_array();
            base.mask_tile = tile.rect.map(|value| value as f32).to_array();
            Mask::Page(tile.page)
        });
        let mask_texture = match text {
            Some(text) => {
                base.flags |= COMPOSITE_MASK;
                base.mask_rect = region.map(|value| value as f32);
                base.mask_tile = [0., 0., base.mask_rect[2], base.mask_rect[3]];
                Some(Mask::Layer(text))
            }
            None => mask_texture,
        };
        let region_rect = region.map(|value| value as f32);

        if let Some((offset, sigma, color)) = effect.shadow {
            let (src, left) = if sigma > 0.25 {
                self.blur(texture, region, sigma, false, false)
            } else {
                (texture, 0.)
            };
            let mut composite = base;
            composite.flags |= COMPOSITE_SHADOW;
            composite.blur = left;
            composite.shadow = [offset.x, offset.y, 0., 0.];
            composite.shadow_color = [
                color.r * color.a,
                color.g * color.a,
                color.b * color.a,
                color.a,
            ];
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
            self.blur(texture, region, effect.blur, true, false)
        } else {
            (texture, 0.)
        };
        let backdrop = (effect.blend_mode != BlendMode::Normal && self.copy)
            .then(|| self.region(nested.bounds, visible))
            .flatten()
            .map(|region| {
                let copy = self.acquire(region, 1);
                self.commands.push(Command::Copy {
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
        for texture in [backdrop, text].into_iter().flatten() {
            self.release(texture);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, RenderFrame};
    use crate::{
        Clip,
        gpu::{BOX, GLYPH, IMAGE, Instance, bytes, rect as bounds},
        scene::{EffectState, Scene, effect},
    };

    fn record(window: [u32; 2], quad: [f32; 4], scale: f32) -> RenderFrame {
        let mask = Clip::from(bounds([-1e6, -1e6, 2e6, 2e6]));
        let mut transform = super::IDENTITY;
        transform[0] = scale;
        let mut scene = Scene::default();
        let state = EffectState {
            transform: (scale != 1.).then_some(transform),
            opacity: 0.5,
            ..effect(mask)
        };
        scene.push_effect(state, Some(7));
        scene.push(Instance::new(BOX, bounds(quad), mask));
        scene.pop_effect();
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
    fn snapshot_layers_draw_into_their_own_target_when_they_are_read() {
        let mask = Clip::from(bounds([0., 0., 100., 100.]));
        let mut scene: Scene = Default::default();
        scene.push(Instance::new(BOX, bounds([0., 0., 10., 10.]), mask));
        scene.push_offscreen();
        scene.push(Instance::new(BOX, bounds([20., 20., 30., 10.]), mask));
        scene.pop_offscreen(Some((7, bounds([20., 20., 30., 10.]))));
        let mut frame = RenderFrame::default();
        frame.record(&scene, [100, 100], 4096, true);
        assert!(frame.shots.is_empty());
        assert_eq!(frame.instances.len(), 1, "the window draws the layer");
        scene.shots[0].read = true;
        frame.record(&scene, [100, 100], 4096, true);
        let [(7, target, [20, 20, 30, 10])] = frame.shots[..] else {
            panic!("the layer is not read");
        };
        assert_eq!(frame.instances.len(), 2);
        assert_eq!(frame.origin(target), [20, 20]);
    }

    #[test]
    fn text_masks_composite_through_their_own_layer() {
        let mask = Clip::from(bounds([0., 0., 100., 100.]));
        for has_text in [false, true] {
            let mut scene = Scene::default();
            let text = scene.mask_layer();
            scene.push_effect(
                EffectState {
                    text_mask: Some(text),
                    ..effect(mask)
                },
                None,
            );
            scene.push(Instance::new(BOX, bounds([10., 10., 50., 20.]), mask));
            scene.pop_effect();
            if has_text {
                scene.push_into(text, Instance::new(BOX, bounds([12., 12., 10., 10.]), mask));
            }
            let mut frame = RenderFrame::default();
            frame.record(&scene, [100, 100], 16384, true);
            let masks: Vec<_> = frame
                .commands
                .iter()
                .filter_map(|command| match command {
                    Command::Composite { src, mask, .. } => Some((*src, *mask)),
                    _ => None,
                })
                .collect();
            if !has_text {
                assert!(masks.is_empty());
                continue;
            }
            let [(src, Some(super::Mask::Layer(text)))] = masks[..] else {
                panic!("one composite through the text layer");
            };
            assert!(src != text && frame.texture(src) != frame.texture(text));
            assert_eq!(frame.composites[0].mask_rect, [10., 10., 50., 20.]);
        }
    }

    #[test]
    fn opaque_boxes_hide_what_they_cover() {
        let mask = Clip::from(bounds([0., 0., 100., 100.]));
        let quads = |boxes: &[([f32; 4], f32)]| {
            let mut scene = Scene::default();
            for &(rect, alpha) in boxes {
                let mut instance = Instance::new(BOX, bounds(rect), mask);
                instance.color[3] = alpha;
                scene.push(instance);
            }
            let mut frame = RenderFrame::default();
            frame.record(&scene, [100, 100], 16384, true);
            frame
                .instances
                .iter()
                .map(|instance| instance.reach().to_array())
                .collect::<Vec<_>>()
        };
        let page = ([0., 0., 100., 100.], 1.);
        let dot = ([10., 10., 20., 20.], 1.);
        assert_eq!(quads(&[page, page, dot]).len(), 2);
        assert_eq!(quads(&[page, (page.0, 0.5), dot]).len(), 3);
        let side = ([0., 0., 30., 100.], 1.);
        let main = ([30., 0., 70., 100.], 1.);
        assert_eq!(quads(&[page, side, main]).len(), 2);
        let header = ([0., 0., 100., 20.], 1.);
        assert_eq!(
            quads(&[page, header, ([0., 20., 30., 80.], 1.)]),
            [
                [30., 20., 70., 80.],
                [0., 0., 100., 20.],
                [0., 20., 30., 80.]
            ]
        );
    }

    #[test]
    fn glass_foregrounds_keep_the_border_box_and_draw_once() {
        let border = bounds([20., 30., 160., 48.]);
        let mask = Clip::from(bounds([0., 0., 800., 600.]));
        for has_content in [false, true] {
            let mut scene = Scene::default();
            scene.push(Instance::new(BOX, mask.bounds, mask));
            scene.push_effect(
                EffectState {
                    glass: Some(crate::scene::GlassState {
                        key: 5,
                        bounds: border,
                        corner_radius: 24.,
                        tint: None,
                        clear: false,
                        interactive: false,
                    }),
                    ..effect(mask)
                },
                None,
            );
            if has_content {
                scene.push(Instance::new(BOX, bounds([36., 46., 80., 16.]), mask));
            }
            scene.pop_effect();
            assert_eq!(scene.layers[1].bounds, Some(border));
            assert_eq!(scene.layers[0].nested[0].bounds, border);
            let mut frame = RenderFrame {
                skip: vec![1],
                content: vec![1],
                ..Default::default()
            };
            frame.record(&scene, [800, 600], 16384, true);
            let [(layer, target, region)] = frame.promoted[..] else {
                panic!("one glass foreground")
            };
            assert_eq!((layer, region), (1, [20, 30, 160, 48]));
            assert_eq!(frame.origin(target), [20, 30]);
            assert_eq!(frame.instances.len(), 1 + usize::from(has_content));
            assert!(frame.composites.is_empty());
            assert!(frame.commands.iter().any(|command| {
                matches!(command, Command::Pass(pass, true) if *pass == target)
            }));
        }
    }

    #[test]
    fn patched_transforms_draw_like_new_frames() {
        let mask = Clip::from(bounds([0., 0., 800., 600.]));
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
        let shift = |x: f32, y: f32| {
            let mut m = super::IDENTITY;
            [m[12], m[13]] = [x, y];
            m
        };
        let scene = |matrix: [f32; 16]| {
            let mut scene = Scene::default();
            let state = EffectState {
                transform: Some(matrix),
                ..effect(mask)
            };
            scene.push_effect(state, Some(9));
            scene.push(Instance::new(BOX, bounds([100., 100., 100., 100.]), mask));
            scene.pop_effect();
            scene.push(Instance::new(BOX, bounds([205., 140., 25., 20.]), mask));
            scene
        };
        let draw = |scene: &Scene| {
            let mut frame = RenderFrame::default();
            frame.record(scene, [800, 600], 16384, true);
            let order: Vec<_> = frame
                .commands
                .iter()
                .filter_map(|command| match command {
                    Command::Draw { instances, .. } => Some(format!("draw {instances:?}")),
                    Command::Composite { index, .. } => Some(format!("composite {index}")),
                    _ => None,
                })
                .collect();
            (
                order,
                bytes(&frame.instances).to_vec(),
                bytes(&frame.composites).to_vec(),
            )
        };
        for matrix in [turn(30.), shift(100., 40.), shift(-90., 0.)] {
            let mut patched = scene(turn(0.));
            assert!(patched.patch(9, 1., Some(matrix)));
            assert_eq!(draw(&patched), draw(&scene(matrix)));
        }
    }

    #[test]
    fn wide_blurs_draw_both_passes_at_half_resolution() {
        let blurred = |sigma: f32| {
            let mask = Clip::from(bounds([0., 0., 800., 600.]));
            let mut scene = Scene::default();
            scene.push_effect(
                EffectState {
                    blur: sigma,
                    ..effect(mask)
                },
                None,
            );
            scene.push(Instance::new(BOX, bounds([100., 100., 200., 100.]), mask));
            scene.pop_effect();
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
        let composite = wide.composites[0];
        assert_eq!(composite.blur, 0.);
        let Some(Command::Composite { src, .. }) = wide.commands.last() else {
            panic!("a composite last")
        };
        let [width, height] = wide.pixels(*src);
        assert_eq!(composite.source_rect[2..], [width * 2., height * 2.]);
    }

    #[test]
    fn small_tiles_get_a_small_texture() {
        let frame = record([3072, 2048], [100., 100., 24., 24.], 1.);
        assert_eq!(frame.textures, [[256, 256]]);
        let source = frame.commands.iter().find_map(|command| match command {
            Command::Composite { index, .. } => Some(frame.composites[*index as usize].source_rect),
            _ => None,
        });
        assert_eq!(source.map(|rect| [rect[2], rect[3]]), Some([256., 256.]));
    }

    #[test]
    fn pooled_textures_outlive_two_idle_frames() {
        let used = record([800, 600], [100., 100., 24., 24.], 1.);
        let (idle, mut pool, mut made) = (RenderFrame::default(), super::Pool::default(), 0);
        let mut fill = |pool: &mut super::Pool<usize>, frame: &RenderFrame| {
            pool.fill(frame, |_| {
                made += 1;
                Ok(made)
            })
            .unwrap();
        };
        fill(&mut pool, &used);
        let target = super::Target::Layer(0);
        assert_eq!(pool.get(&used, target), Some(&1));
        assert_eq!(pool.get(&used, super::Target::Frame), None);
        for _ in 0..2 {
            fill(&mut pool, &idle);
        }
        fill(&mut pool, &used);
        assert_eq!(pool.get(&used, target), Some(&1));
        for _ in 0..3 {
            fill(&mut pool, &idle);
        }
        fill(&mut pool, &used);
        assert_eq!(pool.get(&used, target), Some(&2));
    }

    #[test]
    fn wide_layers_fit_the_maximum_texture() {
        let frame = record([6144, 3456], [-2048., 0., 8000., 100.], 3.);
        assert!(!frame.textures.is_empty());
        assert!(frame.textures.iter().flatten().all(|side| *side <= 16384));
    }

    #[test]
    fn draws_follow_paint_order_and_split_on_atlas_pages() {
        let mask = Clip::from(bounds([0., 0., 400., 400.]));
        let sprite = |kind: u32, x: f32, page: u32| {
            let mut instance = Instance::new(kind, bounds([x, 0., 10., 10.]), mask);
            instance.info[3] = page as f32;
            instance
        };
        let mut scene = Scene::default();
        scene.push(Instance::new(BOX, bounds([0., 0., 10., 10.]), mask));
        scene.push(sprite(GLYPH, 10., 0));
        scene.push(sprite(IMAGE, 20., 3));
        scene.push(sprite(GLYPH, 30., 0));
        scene.push(sprite(GLYPH, 40., 1));
        scene.push_effect(effect(mask), None);
        scene.push(Instance::new(BOX, bounds([300., 300., 10., 10.]), mask));
        scene.pop_effect();
        scene.push(sprite(IMAGE, 50., 3));
        let mut frame = RenderFrame::default();
        frame.record(&scene, [400, 400], 16384, true);
        let draws: Vec<_> = frame
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Draw {
                    instances,
                    mask_page,
                    color_page,
                } => Some((instances.clone(), *mask_page, *color_page)),
                Command::Composite { .. } => Some((0..0, None, None)),
                _ => None,
            })
            .collect();
        assert_eq!(
            draws,
            [
                (0..4, Some(0), Some(3)),
                (4..5, Some(1), Some(3)),
                (5..6, None, None),
                (0..0, None, None),
                (6..7, None, Some(3)),
            ]
        );
        let order: Vec<_> = frame
            .instances
            .iter()
            .map(|instance| instance.rect[0])
            .collect();
        assert_eq!(order, [0., 10., 20., 30., 40., 300., 50.]);
    }
}
