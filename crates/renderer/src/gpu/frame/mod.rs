mod effect;

use crate::{
    BlendMode, Point, Rect, Size,
    geometry::{IDENTITY, invert, plane_of, region},
    gpu::scene::{NESTED, Nested, Scene, Under},
    gpu::{
        BOX, Blur, COMPOSITE_BACKDROP, COMPOSITE_BLEND, COMPOSITE_CLIP, COMPOSITE_MASK,
        COMPOSITE_MATRIX, COMPOSITE_SHADOW, Composite, Globals, Instance,
    },
};
use std::ops::Range;

const HALF_RESOLUTION_SIGMA: f32 = 8.;
// A layer that the target cuts draws its content in cells of this size, so it keeps its pixels while it moves.
const CUT_STEP: f32 = 256.;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Target {
    #[default]
    Frame,
    Layer(usize),
}

pub struct Layer {
    pub texture: usize,
    pub origin: [i32; 2],
    scale: f32,
    bounds: [f32; 4],
    // The texels of the texture that the layer holds. A pooled texture can be larger, or hold other tiles.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    scissor: [u32; 4],
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
    textures: Vec<[u32; 2]>,
    free: Vec<usize>,
    shelves: Vec<Option<Shelf>>,
    deferred: Vec<Vec<Deferred>>,
    window: [u32; 2],
    max_texture: f32,
    copy: bool,
    skip: Vec<usize>,
    content: Vec<usize>,
    // With regions, only the window content that reaches each is drawn, then copied over the last frame.
    damage: Vec<[i32; 4]>,
    // The targets of the window content.
    pub bases: Vec<Target>,
    cull: Option<Rect<f32>>,
    // For each texture, the id of the cached layer that it keeps, or 0.
    kept: Vec<u64>,
    // Sizes of the backend's spare textures, which new layers take before other sizes.
    spare: Vec<[u32; 2]>,
    cache: Vec<Cached>,
    // Cached layers draw again, for a frame that is drawn twice in different color spaces.
    fresh: bool,
    // Layer keys drawn lately, by frame. A layer is kept when it draws the same in a later frame.
    seen: rustc_hash::FxHashMap<u64, u32>,
    count: u32,
    ids: u64,
    pub promoted: Vec<(usize, Target, [i32; 4])>,
    // The snapshot layers to read: the key, the target and the region that it holds.
    pub shots: Vec<(u64, Target, [i32; 4])>,
    // The commands of the snapshot layers start here.
    pub shot_commands: usize,
    // Set by a backend that draws damage regions over the last frame: it then keeps the window below what paints
    // after a scrolled content, so a scroll draws again only what changed there and what paints after it.
    keeps_under: bool,
    under: Option<KeptUnder>,
    // Draws only the items of the window layer before the split of `Under`, or only those after it.
    half: Option<([u32; 2], bool)>,
}

// A window-size texture. In the viewport of the scroll its rows and columns turn like a ring, so a scroll moves
// `ring` and not the pixels: the window place `p` in the viewport is at the texel
// `view.xy + (p - view.xy + ring) mod view.wh`, and the other places at `p`.
struct KeptUnder {
    scroll: u64,
    id: u64,
    count: u32,
    window: [u32; 2],
    view: [i32; 4],
    ring: [i32; 2],
}

// The pixels of a layer from an earlier frame, which draw again while its digest and place stay.
struct Cached {
    key: u64,
    digest: u64,
    id: u64,
    size: [u32; 2],
    region: [i32; 4],
    scale: f32,
    extent: [f32; 2],
    idle: u8,
    used: bool,
}

// Cached layers hold at most this many window areas.
const CACHE_WINDOWS: u64 = 2;

// The layer textures of a backend. A texture that no frame used for this many frames is dropped.
const SPARE_FRAMES: u8 = 3;
// Frames with damage regions use few layers, so they age the others this much slower.
const DAMAGED_FRAMES: u8 = 16;
const TAKEN: u8 = u8::MAX;

// Each texture has its size, its idle frames and the id of the cached layer it keeps, or 0.
pub(crate) struct Pool<T> {
    textures: Vec<(T, [u32; 2], u8, u64)>,
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
    // `sizes` and `kept` are the textures of a frame and the cached layers they keep. `alive` says whether a cached
    // layer still draws.
    fn fill(
        &mut self,
        sizes: &[[u32; 2]],
        kept: &[u64],
        alive: impl Fn(u64) -> bool,
        window: [u32; 2],
        damaged: bool,
        mut make: impl FnMut([u32; 2]) -> anyhow::Result<T>,
    ) -> anyhow::Result<()> {
        // The textures of dropped layers go back to the others.
        for (.., idle, id) in &mut self.textures {
            if *id != 0 && !alive(*id) {
                (*idle, *id) = (0, 0);
            }
        }
        // Damage regions take textures of other sizes each frame, as in a scroll, so above two windows of pixels the
        // pool ages as fast as in whole frames.
        let pixels = |[width, height]: [u32; 2]| u64::from(width) * u64::from(height);
        let pooled: u64 = self
            .textures
            .iter()
            .map(|(_, size, ..)| pixels(*size))
            .sum();
        let age = match !damaged || pooled > 2 * pixels(window) {
            true => DAMAGED_FRAMES,
            false => 1,
        };
        for (.., idle, id) in &mut self.textures {
            if *id == 0 {
                *idle = idle.saturating_add(age);
            }
        }
        self.textures
            .retain(|(.., idle, _)| *idle <= SPARE_FRAMES * DAMAGED_FRAMES);
        self.slots.clear();
        for (&size, &id) in sizes.iter().zip(kept) {
            let found = match id {
                0 => None,
                _ => self.textures.iter().position(|texture| texture.3 == id),
            };
            let index = match found.or_else(|| {
                self.textures.iter().position(|(_, kept, idle, other)| {
                    *idle != TAKEN && *other == 0 && *kept == size
                })
            }) {
                Some(index) => index,
                None => {
                    self.textures.push((make(size)?, size, 0, 0));
                    self.textures.len() - 1
                }
            };
            (self.textures[index].2, self.textures[index].3) = (TAKEN, id);
            self.slots.push(index);
        }
        for (.., idle, _) in &mut self.textures {
            if *idle == TAKEN {
                *idle = 0;
            }
        }
        Ok(())
    }

    // The sizes of textures that hold no cached layer.
    #[cfg(target_os = "macos")]
    fn spare(&self, sizes: &mut Vec<[u32; 2]>) {
        sizes.clear();
        sizes.extend(
            (self.textures.iter())
                .filter(|texture| texture.3 == 0)
                .map(|texture| texture.1),
        );
    }

    fn get(&self, texture: usize) -> &T {
        &self.textures[self.slots[texture]].0
    }

    fn clear(&mut self) {
        self.textures.clear();
        self.slots.clear();
    }
}

impl RenderFrame {
    // Gives the textures of this frame backend textures from `pool`, made with `make` when the pool has none.
    pub(crate) fn fill_pool<T>(
        &self,
        pool: &mut Pool<T>,
        make: impl FnMut([u32; 2]) -> anyhow::Result<T>,
    ) -> anyhow::Result<()> {
        let alive = |id| {
            self.cache.iter().any(|cached| cached.id == id)
                || (self.under.as_ref()).is_some_and(|under| under.id == id)
        };
        let damaged = self.damaged();
        pool.fill(
            &self.textures,
            &self.kept,
            alive,
            self.window,
            damaged,
            make,
        )
    }

    pub(crate) fn pooled<'a, T>(&self, pool: &'a Pool<T>, target: Target) -> Option<&'a T> {
        let Target::Layer(index) = target else {
            return None;
        };
        Some(pool.get(self.layers[index].texture))
    }

    // The frame forgets the cached layers too, since their pixels go.
    pub(crate) fn clear_pool<T>(&mut self, pool: &mut Pool<T>) {
        pool.clear();
        self.cache.clear();
        self.under = None;
    }

    // New layers take the sizes of the spare textures of `pool` first.
    #[cfg(target_os = "macos")]
    pub(crate) fn take_spare<T>(&mut self, pool: &Pool<T>) {
        pool.spare(&mut self.spare);
    }

    // What the backend asks of the next frames. It draws the `native` layers itself, and the frame draws the
    // content of those marked true in targets of their own. `damage` lists the window regions to draw over the last
    // frame, or none for the whole window. `fresh` draws cached layers again, and `keeps_under` keeps the window
    // below what paints after a scrolled content.
    #[cfg(target_os = "macos")]
    pub(crate) fn prepare(
        &mut self,
        native: impl Iterator<Item = (usize, bool)>,
        damage: Vec<[i32; 4]>,
        fresh: bool,
        keeps_under: bool,
    ) {
        let (mut same, mut len) = (true, 0);
        self.content.clear();
        for (layer, content) in native {
            if self.skip.get(len) != Some(&layer) {
                same = false;
                self.skip.truncate(len);
                self.skip.push(layer);
            }
            len += 1;
            if content {
                self.content.push(layer);
            }
        }
        same &= len == self.skip.len();
        self.skip.truncate(len);
        // A layer that left or joined the backend draws otherwise in the whole window.
        self.damage = if same { damage } else { Vec::new() };
        (self.fresh, self.keeps_under) = (fresh, keeps_under);
    }

    // Whether the frame skips exactly `layers`, which the backend draws.
    #[cfg(target_os = "macos")]
    pub(crate) fn skips(&self, layers: impl Iterator<Item = usize>) -> bool {
        self.skip.iter().copied().eq(layers)
    }

    // Whether the frame draws damage regions over the last frame.
    pub(crate) fn damaged(&self) -> bool {
        !self.damage.is_empty()
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn damage(&self) -> &[[i32; 4]] {
        &self.damage
    }

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
        self.kept.clear();
        self.bases.clear();
        self.count = self.count.wrapping_add(1);
        let count = self.count;
        self.seen
            .retain(|_, seen| count.wrapping_sub(*seen) <= u32::from(SPARE_FRAMES));
        self.cache
            .retain(|cached| !scene.stale.contains(&cached.digest));
        for cached in &mut self.cache {
            cached.used = false;
        }
        let under = self.record_under(scene);
        let whole = [0, 0, window[0] as i32, window[1] as i32];
        if self.damage.is_empty() {
            self.bases.push(Target::Frame);
            match under {
                Some(split) => {
                    self.copy_kept(Target::Frame, whole, false);
                    self.half = Some((split, true));
                    self.record_layer(scene, 0, Target::Frame, false, 0);
                    self.half = None;
                }
                None => self.record_layer(scene, 0, Target::Frame, true, 0),
            }
        }
        // The regions copy into the window together, since each pass into a target loads and stores all of it.
        let mut copies = Vec::with_capacity(self.damage.len());
        for at in 0..self.damage.len() {
            let region = self.damage[at];
            let base = self.region_base(region);
            match under {
                Some(split) => {
                    self.copy_kept(base, region, false);
                    self.record_region(scene, base, region, Some((split, true)), false);
                }
                None => self.record_region(scene, base, region, None, true),
            }
            copies.push((base, region));
        }
        self.copy_bases(&copies, Target::Frame);
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
        self.shot_commands = self.commands.len();
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
        // Layers that the frame could have used and didn't age. A damage region could use only some.
        let damage = &self.damage;
        for cached in &mut self.cache {
            let [x, y, width, height] = cached.region;
            if !cached.used
                && (damage.is_empty()
                    || damage.iter().any(|&[left, top, w, h]| {
                        x < left + w && left < x + width && y < top + h && top < y + height
                    }))
            {
                cached.idle += 1;
            }
        }
        self.cache.retain(|cached| cached.idle <= SPARE_FRAMES);
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

    // Brings the kept window below the items after a scrolled content up to this scene, and returns the split, or
    // forgets it when the scene has none.
    fn record_under(&mut self, scene: &Scene) -> Option<[u32; 2]> {
        let Some(info) = scene.under.as_ref().filter(|_| self.keeps_under) else {
            self.under = None;
            return None;
        };
        let (count, window) = (self.count, self.window);
        let valid = self.under.as_ref().is_some_and(|kept| {
            kept.scroll == info.scroll
                && kept.count == count.wrapping_sub(1)
                && kept.window == window
        });
        // Only a scroll starts one.
        if !valid && scene.shift.is_none() {
            self.under = None;
            return None;
        }
        if !valid {
            self.ids += 1;
            self.under = Some(KeptUnder {
                scroll: info.scroll,
                id: self.ids,
                count,
                window,
                view: [0; 4],
                ring: [0; 2],
            });
        }
        let split = info.split;
        let kept = self.under.as_mut()?;
        kept.count = count;
        let id = kept.id;
        // A ring turns only in its own viewport.
        let turns = (scene.shift).is_none_or(|(view, _)| kept.ring == [0; 2] || kept.view == view);
        let regions = (valid && turns)
            .then(|| {
                let reads = (scene.reads.iter())
                    .filter(|read| !Under::after(split, read.1))
                    .map(|read| read.0);
                info.damage.regions(reads, window, &[])
            })
            .flatten();
        let Some(regions) = regions else {
            kept.ring = [0; 2];
            let target = self.kept_layer(id, [0, 0]);
            self.bases.push(target);
            self.half = Some((split, false));
            self.record_layer(scene, 0, target, true, 0);
            self.half = None;
            return Some(split);
        };
        // The viewport shows the pixels that were `by` further, and the scene damaged the edges that it had none for.
        if let Some((view, by)) = scene.shift {
            let kept = self.under.as_mut()?;
            kept.view = view;
            kept.ring =
                [0, 1].map(|axis| (kept.ring[axis] + by[axis]).rem_euclid(view[axis + 2].max(1)));
        }
        let mut copies = Vec::with_capacity(regions.len());
        for region in regions {
            let base = self.region_base(region);
            self.record_region(scene, base, region, Some((split, false)), true);
            copies.push((base, region));
        }
        for &(base, region) in &copies {
            self.copy_kept(base, region, true);
        }
        for &(base, _) in &copies {
            self.release(base);
        }
        Some(split)
    }

    // Copies a window region out of the kept window into `other`, or into it from `other`. The places where the ring
    // turns split the region, and its parts outside the viewport stay in place.
    fn copy_kept(&mut self, other: Target, [x, y, width, height]: [i32; 4], into: bool) {
        let Some(kept) = self.under.as_ref() else {
            return;
        };
        let (id, [vx, vy, vw, vh], [rx, ry]) = (kept.id, kept.view, kept.ring);
        let parts = |start: i32, size: i32, view: i32, length: i32, ring: i32| {
            let turn = view + length - ring;
            [
                (start, view, None),
                (view, turn, Some(ring)),
                (turn, view + length, Some(ring - length)),
                (view + length, start + size, None),
            ]
            .map(|(low, high, by)| (low.max(start), high.min(start + size), by))
        };
        for (left, right, dx) in parts(x, width, vx, vw, rx) {
            for (top, bottom, dy) in parts(y, height, vy, vh, ry) {
                if left >= right || top >= bottom {
                    continue;
                }
                let origin = dx.zip(dy).map_or([0, 0], |(dx, dy)| [-dx, -dy]);
                let view = self.kept_layer(id, origin);
                let (src, dst) = if into { (other, view) } else { (view, other) };
                self.commands.push(Command::Copy {
                    src,
                    dst,
                    region: [left, top, right - left, bottom - top],
                });
            }
        }
    }

    fn copy_bases(&mut self, copies: &[(Target, [i32; 4])], dst: Target) {
        for &(src, region) in copies {
            self.commands.push(Command::Copy { src, dst, region });
        }
        for &(base, _) in copies {
            self.release(base);
        }
    }

    #[cfg(all(target_os = "macos", test))]
    pub(crate) fn keeps_under_now(&self) -> bool {
        self.under.is_some()
    }

    // A window-size texture that stays across frames, with the window at `origin`.
    fn kept_layer(&mut self, id: u64, origin: [i32; 2]) -> Target {
        let size = self.window;
        let texture = match self.kept.iter().position(|&kept| kept == id) {
            Some(texture) => texture,
            None => {
                self.textures.push(size);
                self.kept.push(id);
                self.textures.len() - 1
            }
        };
        self.layers.push(Layer {
            texture,
            origin,
            scale: 1.,
            bounds: [
                origin[0] as f32,
                origin[1] as f32,
                size[0] as f32,
                size[1] as f32,
            ],
            scissor: [0, 0, size[0], size[1]],
        });
        Target::Layer(self.layers.len() - 1)
    }

    fn region_base(&mut self, region: [i32; 4]) -> Target {
        let base = self.acquire(region, 1);
        // Layers take the same place as in a whole frame.
        if let Target::Layer(index) = base {
            self.layers[index].bounds = [0., 0., self.window[0] as f32, self.window[1] as f32];
        }
        self.bases.push(base);
        base
    }

    fn record_region(
        &mut self,
        scene: &Scene,
        base: Target,
        region: [i32; 4],
        half: Option<([u32; 2], bool)>,
        clear: bool,
    ) {
        let [x, y, width, height] = region.map(|value| value as f32);
        self.cull = Some(Rect::new(Point::new(x, y), Size::new(width, height)));
        self.half = half;
        self.record_layer(scene, 0, base, clear, 0);
        (self.cull, self.half) = (None, None);
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

    /// The texels that draws into `target` may change, or `None` for the whole window.
    #[cfg(target_os = "macos")]
    pub fn scissor(&self, target: Target) -> Option<[u32; 4]> {
        match target {
            Target::Frame => None,
            Target::Layer(index) => Some(self.layers[index].scissor),
        }
    }

    pub fn globals(&self, target: Target, text_contrast: f32) -> Globals {
        Globals {
            viewport: self.rect(target),
            text_contrast,
            wide: 0.,
            pad: [0.; 2],
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
                let area = |[width, height]: [u32; 2]| u64::from(width) * u64::from(height);
                // Damage regions change size each frame, so a spare texture up to twice as large saves a new one.
                let spare = (self.spare.iter().enumerate())
                    .filter(|(_, spare)| {
                        spare[0] >= size[0]
                            && spare[1] >= size[1]
                            && area(**spare) <= 2 * area(size).max(1 << 16)
                    })
                    .min_by_key(|(_, spare)| area(**spare))
                    .map(|(at, _)| at);
                self.textures.push(match spare {
                    Some(at) => self.spare.swap_remove(at),
                    None => size.map(|value| (value.div_ceil(256) * 256).min(max)),
                });
                self.kept.push(0);
                self.textures.len() - 1
            }
        };
        let scissor = [0, 0, size[0], size[1]];
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
            scissor,
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
                self.kept.push(0);
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
            scissor: [offset[0] - 1, offset[1] - 1, w, h].map(|value| value as u32),
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

    // A cached layer keeps its texture.
    fn release(&mut self, target: Target) {
        if let Target::Layer(index) = target
            && self.kept[self.layers[index].texture] == 0
        {
            self.free.push(self.layers[index].texture);
        }
    }

    // The pixels of a layer that drew the same in an earlier frame, now at `region`.
    fn reuse(&mut self, key: u64, region: [i32; 4]) -> Option<Target> {
        if self.fresh {
            return None;
        }
        let cached = self.cache.iter_mut().find(|cached| cached.key == key)?;
        (cached.used, cached.idle, cached.region) = (true, 0, region);
        let (scale, [width, height]) = (cached.scale, cached.extent);
        self.textures.push(cached.size);
        self.kept.push(cached.id);
        self.layers.push(Layer {
            texture: self.textures.len() - 1,
            origin: [region[0], region[1]],
            scale,
            bounds: [region[0] as f32, region[1] as f32, width, height],
            scissor: [0, 0, cached.size[0], cached.size[1]],
        });
        Some(Target::Layer(self.layers.len() - 1))
    }

    // Keeps the pixels of a layer for later frames, within the budget of the cache.
    fn keep(&mut self, target: Target, key: u64, digest: u64, region: [i32; 4]) {
        let Target::Layer(index) = target else {
            return;
        };
        let Layer {
            texture,
            scale,
            bounds,
            ..
        } = self.layers[index];
        let size = self.textures[texture];
        let pixels = |size: [u32; 2]| u64::from(size[0]) * u64::from(size[1]);
        // Small windows still keep a few small layers, since textures are at least 256 by 256 pixels.
        let budget = CACHE_WINDOWS * pixels(self.window) + (1 << 20);
        let held: u64 = self.cache.iter().map(|cached| pixels(cached.size)).sum();
        if self.kept[texture] != 0 || held + pixels(size) > budget {
            return;
        }
        self.ids += 1;
        self.kept[texture] = self.ids;
        self.cache.push(Cached {
            key,
            digest,
            id: self.ids,
            size,
            region,
            scale,
            extent: [bounds[2], bounds[3]],
            idle: 0,
            used: true,
        });
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
        // The items of the other half of the window layer don't draw, and don't hide others.
        let half = self.half.filter(|_| layer == 0);
        let skip = |top: u32| half.is_some_and(|(split, after)| Under::after(split, top) != after);
        // Skip what a later opaque box hides, since stacked page backgrounds can fill the window many
        // times. An effect that reads the target sees the pixels drawn before it, so a box after the effect
        // hides only the boxes after the effect too.
        let reads: Vec<usize> = (content.nested.iter())
            .filter(|nested| {
                (scene.layers[nested.layer].effect.as_ref()).is_some_and(|effect| {
                    effect.backdrop.is_some() || effect.blend_mode != BlendMode::Normal
                })
            })
            .map(|nested| nested.at as usize)
            .collect();
        let min = self.window[0] as f32 * self.window[1] as f32 / 64.;
        let mut occluders: Vec<(usize, Rect<f32>, f32, usize)> = (content.instances.iter())
            .enumerate()
            .filter(|(at, instance)| {
                !skip(*at as u32)
                    && instance.kind() == BOX
                    && instance.color[3] >= 1.
                    && instance.aux == [0.; 4]
                    && instance.radii == [0.; 8]
                    && instance.round_radii == [0.; 8]
            })
            .filter_map(|(at, instance)| {
                let bounds = instance.reach();
                let area = bounds.size.width * bounds.size.height;
                let read = reads.partition_point(|&read| read <= at);
                let floor = read.checked_sub(1).map_or(0, |read| reads[read]);
                (area >= min).then_some((at, bounds, area, floor))
            })
            .collect();
        if occluders.len() > 16 {
            occluders.select_nth_unstable_by(16, |a, b| b.2.total_cmp(&a.2));
            occluders.truncate(16);
        }
        occluders.sort_unstable_by_key(|occluder| std::cmp::Reverse(occluder.0));
        // With `trim`, an occluder that spans the box cuts off the side it covers. Cuts repeat until none
        // applies, since one cut can make another occluder span the rest.
        let visible = |at: usize, bounds: &mut Rect<f32>, trim: bool| loop {
            let before = *bounds;
            let occluders = occluders.iter().take_while(|occluder| occluder.0 > at);
            for (_, over, ..) in occluders.filter(|occluder| occluder.3 <= at) {
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
        let mut nested = (content.nested.iter().enumerate())
            .filter(|(index, _)| !skip(*index as u32 | NESTED))
            .map(|(_, nested)| nested)
            .peekable();
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
            if skip(at as u32) {
                continue;
            }
            let mut instance = *instance;
            let mut bounds = instance.reach();
            // A scroll viewport can cut an instance away.
            if bounds.is_empty()
                || layer == 0 && self.cull.is_some_and(|cull| !cull.intersects(bounds))
            {
                continue;
            }
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
        if !nested.reach().is_empty()
            && (layer != 0
                || !self.skip.contains(&nested.layer)
                    && self.cull.is_none_or(|cull| cull.intersects(nested.reach())))
        {
            // The cull region is in window space, and a transformed layer draws in its own.
            let cull = self.cull;
            if (scene.layers[nested.layer].effect.as_ref())
                .is_some_and(|effect| effect.transform.is_some())
            {
                self.cull = None;
            }
            self.record_effect(scene, nested, target, depth);
            self.cull = cull;
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
}

#[cfg(test)]
mod tests;
