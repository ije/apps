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
    // With regions, only the window content that reaches each is drawn, then copied over the last frame.
    pub damage: Vec<[i32; 4]>,
    // The targets of the window content.
    pub bases: Vec<Target>,
    cull: Option<Rect<f32>>,
    // For each texture, the id of the cached layer that it keeps, or 0.
    pub kept: Vec<u64>,
    // Sizes of the backend's spare textures, which new layers take before other sizes.
    pub spare: Vec<[u32; 2]>,
    cache: Vec<Cached>,
    // Cached layers draw again, for a frame that is drawn twice in different color spaces.
    pub fresh: bool,
    // Layer keys drawn lately, by frame. A layer is kept when it draws the same in a later frame.
    seen: rustc_hash::FxHashMap<u64, u32>,
    count: u32,
    ids: u64,
    pub promoted: Vec<(usize, Target, [i32; 4])>,
    // The snapshot layers to read: the key, the target and the region that it holds.
    pub shots: Vec<(u64, Target, [i32; 4])>,
    // The commands of the snapshot layers start here.
    pub shot_commands: usize,
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
    pub fn fill(
        &mut self,
        frame: &RenderFrame,
        mut make: impl FnMut([u32; 2]) -> anyhow::Result<T>,
    ) -> anyhow::Result<()> {
        // The textures of dropped layers go back to the others.
        for (.., idle, id) in &mut self.textures {
            if *id != 0 && !frame.cache.iter().any(|cached| cached.id == *id) {
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
        let age = match frame.damage.is_empty() || pooled > 2 * pixels(frame.window) {
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
        for (&size, &id) in frame.textures.iter().zip(&frame.kept) {
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
    #[cfg(any(target_os = "macos", test))]
    pub fn spare(&self, sizes: &mut Vec<[u32; 2]>) {
        sizes.clear();
        sizes.extend(
            (self.textures.iter())
                .filter(|texture| texture.3 == 0)
                .map(|texture| texture.1),
        );
    }

    pub fn get(&self, frame: &RenderFrame, target: Target) -> Option<&T> {
        let Target::Layer(index) = target else {
            return None;
        };
        Some(&self.textures[self.slots[frame.layers[index].texture]].0)
    }

    // The frame forgets the cached layers too, since their pixels go.
    pub fn clear(&mut self, frame: &mut RenderFrame) {
        self.textures.clear();
        self.slots.clear();
        frame.cache.clear();
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

// The window regions to draw again, or `None` for the whole window. A layer that reads the pixels
// below it draws whole when a region reaches it, since it reads only what the frame drew again.
#[cfg(any(target_os = "macos", test))]
pub fn damage_regions(
    scene: &Scene,
    window: [u32; 2],
    extra: &[[i32; 4]],
) -> Option<Vec<[i32; 4]>> {
    if scene.damage.whole {
        return None;
    }
    let bounds = Rect::new(
        Point::default(),
        Size::new(window[0] as f32, window[1] as f32),
    );
    let meets = |a: [i32; 4], b: [i32; 4]| {
        a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
    };
    let join = |a: [i32; 4], b: [i32; 4]| {
        let [x, y] = [a[0].min(b[0]), a[1].min(b[1])];
        let [right, bottom] = [
            (a[0] + a[2]).max(b[0] + b[2]),
            (a[1] + a[3]).max(b[1] + b[3]),
        ];
        [x, y, right - x, bottom - y]
    };
    let mut regions: Vec<[i32; 4]> = Vec::new();
    let add = |regions: &mut Vec<[i32; 4]>, mut region: [i32; 4]| {
        while let Some(at) = regions.iter().position(|other| meets(*other, region)) {
            region = join(regions.swap_remove(at), region);
        }
        regions.push(region);
    };
    // Edges blend into the pixel next to them.
    for rect in &scene.damage.rects {
        let rect = rect.outset(1.).intersect(bounds);
        if !rect.is_empty() {
            add(&mut regions, region(rect));
        }
    }
    for &region in extra {
        add(&mut regions, region);
    }
    let reads: Vec<[i32; 4]> = (scene.reads.iter())
        .map(|read| read.intersect(bounds))
        .filter(|read| !read.is_empty())
        .map(region)
        .collect();
    while let Some(&read) = reads.iter().find(|read| {
        regions.iter().any(|region| meets(*region, **read))
            && !regions
                .iter()
                .any(|region| join(*region, **read) == *region)
    }) {
        add(&mut regions, read);
    }
    let area: i64 = (regions.iter())
        .map(|region| i64::from(region[2]) * i64::from(region[3]))
        .sum();
    (area * 2 <= i64::from(window[0]) * i64::from(window[1])).then_some(regions)
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
        if self.damage.is_empty() {
            self.bases.push(Target::Frame);
            self.record_layer(scene, 0, Target::Frame, true, 0);
        }
        for at in 0..self.damage.len() {
            let region = self.damage[at];
            let base = self.acquire(region, 1);
            // Layers take the same place as in a whole frame.
            if let Target::Layer(index) = base {
                self.layers[index].bounds = [0., 0., window[0] as f32, window[1] as f32];
            }
            self.bases.push(base);
            let [x, y, width, height] = region.map(|value| value as f32);
            self.cull = Some(Rect::new(Point::new(x, y), Size::new(width, height)));
            self.record_layer(scene, 0, base, true, 0);
            self.cull = None;
            self.commands.push(Command::Copy {
                src: base,
                dst: Target::Frame,
                region,
            });
            self.release(base);
        }
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
            // A scroll viewport can cut an instance away.
            if bounds.is_empty() || layer == 0 && self.cull.is_some_and(|cull| !cull.intersects(bounds)) {
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
        if sigma < HALF_RESOLUTION_SIGMA {
            let horizontal = self.acquire(covered, 1);
            self.blur_pass(src, horizontal, region, sigma, [1., 0.], mirror);
            if release {
                self.release(src);
            }
            return (horizontal, sigma);
        }
        // Like Skia, a wide blur halves the pixels until about 4 of them are left per sigma, blurs there, and the
        // composite scales it back up. A sample between 4 pixels averages them, so each half is a box filter.
        let halves = (sigma / 4.).log2().floor().clamp(1., 5.) as u32;
        let (mut source, mut owned) = (src, release);
        for half in 1..=halves {
            let target = self.acquire(covered, 1 << half);
            self.blur_pass(source, target, region, 0., [1., 0.], mirror);
            if owned {
                self.release(source);
            }
            (source, owned) = (target, true);
        }
        // The box filters blurred by (4^halves - 1) / 12 square pixels already.
        let boxed = ((1u32 << (2 * halves)) - 1) as f32 / 12.;
        let sigma = (sigma * sigma - boxed).max(0.).sqrt();
        let scale = 1 << halves;
        let horizontal = self.acquire(covered, scale);
        self.blur_pass(source, horizontal, region, sigma, [1., 0.], mirror);
        self.release(source);
        let both = self.acquire(covered, scale);
        self.blur_pass(horizontal, both, region, sigma, [0., 1.], mirror);
        self.release(horizontal);
        (both, 0.)
    }

    // A layer that draws as before keeps its blur too. `key` is the key of the layer and the kind of blur.
    fn blurred(
        &mut self,
        src: Target,
        region: [i32; 4],
        sigma: f32,
        release: bool,
        key: Option<(u64, u64, bool)>,
    ) -> (Target, f32) {
        let key = key.map(|(key, digest, keep)| {
            (
                crate::scene::mix(key, u64::from(sigma.to_bits())),
                digest,
                keep,
            )
        });
        let [x, y, width, height] = self.rect(src).map(|value| value as i32);
        if let Some(hit) = key.and_then(|(key, ..)| self.reuse(key, [x, y, width, height])) {
            if release {
                self.release(src);
            }
            return (
                hit,
                if sigma >= HALF_RESOLUTION_SIGMA {
                    0.
                } else {
                    sigma
                },
            );
        }
        let (blurred, left) = self.blur(src, region, sigma, release, false);
        if let Some((key, digest, true)) = key {
            self.keep(blurred, key, digest, [x, y, width, height]);
        }
        (blurred, left)
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
            shapes: {
                let [top, bottom] = nested.clip.radii.shape_array();
                [0., 0., top, bottom]
            },
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
        // A backdrop with no blur and no color change draws its backdrop again as it is.
        if let Some(backdrop) = &effect.backdrop
            && self.copy
            && (backdrop.blur > 0.25 || backdrop.color_matrix.is_some())
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
            [composite.shapes[0], composite.shapes[1]] = backdrop.corner_radii.shape_array();
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
        // The region of a transformed layer is in its own space: the visible part mapped back through the
        // transform. A composited transform moves the layer without a repaint, so the region keeps a window
        // of room on each side and moves in steps of a window, and small moves keep its pixels.
        let reach = match &effect.transform {
            Some(transform) => {
                let [x, y, width, height] = visible;
                let back = crate::window::invert(&crate::window::plane_of(transform));
                let seen = back.and_then(|m| {
                    [
                        [x, y],
                        [x + width, y],
                        [x, y + height],
                        [x + width, y + height],
                    ]
                    .into_iter()
                    .try_fold(
                        [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
                        |at, [x, y]| {
                            let [u, v, q] =
                                [0, 1, 2].map(|row| m[row] * x + m[3 + row] * y + m[6 + row]);
                            (q > 1e-6).then(|| {
                                let [u, v] = [u / q, v / q];
                                [at[0].min(u), at[1].min(v), at[2].max(u), at[3].max(v)]
                            })
                        },
                    )
                });
                match seen {
                    Some([left, top, right, bottom]) => {
                        // At most 4 windows on a side, around the middle of the visible part.
                        let span = |low: f32, high: f32, step: f32| {
                            let from = (low / step).floor() * step - step;
                            let to = (high / step).ceil() * step + step;
                            let mid = ((low + high) / 2. / step).round() * step;
                            [from.max(mid - 2. * step), to.min(mid + 2. * step)]
                        };
                        let ([x0, x1], [y0, y1]) = (span(left, right, w), span(top, bottom, h));
                        [x0, y0, x1 - x0, y1 - y0]
                    }
                    None => [-w, -h, 3. * w, 3. * h],
                }
            }
            None => visible,
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
        // A layer that draws as in an earlier frame takes the pixels of then. One that draws the same in
        // two frames keeps them.
        let digest = (scene.digests.get(nested.layer).copied())
            .filter(|digest| *digest != 0 && effect.text_mask.is_none());
        // The pixels are the same at another place, so the key holds the region relative to the layer.
        let origin = crate::scene::origin(layer).map(|value| value as i32);
        let bordered = [region[0] - 1, region[1] - 1, region[2] + 2, region[3] + 2];
        let key = digest.map(|digest| {
            [
                region[0] - origin.x,
                region[1] - origin.y,
                region[2],
                region[3],
            ]
            .iter()
            .fold(digest, |key, value| crate::scene::mix(key, *value as u64))
        });
        let cached = key.and_then(|key| self.reuse(key, bordered));
        let count = self.count;
        let seen = cached.is_none()
            && key.is_some_and(|key| {
                (self.seen.insert(key, count)).is_some_and(|seen| seen != count)
            });
        let tile = (cached.is_none() && !seen && !copies && !blurs && whole)
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
        // Blurs keep their pixels when the layer does.
        let keeps = cached.is_some() || seen;
        let texture = match (cached, tile) {
            (Some(cached), _) => cached,
            (None, Some((tile, clear))) => {
                self.record_layer(scene, nested.layer, tile, clear, depth + 1);
                tile
            }
            (None, None) => {
                let texture = self.acquire(bordered, 1);
                self.record_layer(scene, nested.layer, texture, true, depth + 1);
                if let (true, Some(key), Some(digest)) = (seen, key, digest) {
                    self.keep(texture, key, digest, bordered);
                }
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
            [base.shapes[0], base.shapes[1]] = corners.shape_array();
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

        let kept = |kind: u64| {
            key.zip(digest)
                .map(|(key, digest)| (crate::scene::mix(key, kind), digest, keeps))
        };
        if let Some((offset, sigma, color)) = effect.shadow {
            let (src, left) = if sigma > 0.25 {
                self.blurred(texture, region, sigma, false, kept(1))
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
            self.blurred(texture, region, effect.blur, true, kept(2))
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
        scene::{BackdropState, EffectState, Scene, effect},
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
        assert!(scene.patch(7, 0.25, None, [None; 2]) && !scene.patch(8, 0.25, None, [None; 2]));
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
    fn opaque_boxes_hide_only_what_backdrops_read_before_them() {
        let mask = Clip::from(bounds([0., 0., 100., 100.]));
        let mut scene = Scene::default();
        let page = |scene: &mut Scene, red: f32| {
            let mut instance = Instance::new(BOX, bounds([0., 0., 100., 100.]), mask);
            instance.color = [red, 0., 0., 1.];
            scene.push(instance);
        };
        page(&mut scene, 0.1);
        page(&mut scene, 0.2);
        let state = EffectState {
            backdrop: Some(BackdropState {
                bounds: bounds([0., 0., 50., 50.]),
                corner_radii: Default::default(),
                blur: 4.,
                color_matrix: None,
            }),
            ..effect(mask)
        };
        scene.push_effect(state, None);
        scene.push(Instance::new(BOX, bounds([0., 0., 50., 50.]), mask));
        scene.pop_effect();
        page(&mut scene, 0.3);
        page(&mut scene, 0.4);
        let mut frame = RenderFrame::default();
        frame.record(&scene, [100, 100], 16384, true);
        let reds: Vec<_> = (frame.instances.iter())
            .map(|instance| instance.color[0])
            .collect();
        assert_eq!(
            reds,
            [0.2, 0., 0.4],
            "the backdrop reads the second page, and the last page hides the third"
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
            assert!(patched.patch(9, 1., Some(matrix), [None; 2]));
            assert_eq!(draw(&patched), draw(&scene(matrix)));
        }
    }

    #[test]
    fn transformed_layers_draw_content_that_moves_into_view() {
        let mask = Clip::from(bounds([0., 0., 800., 600.]));
        for shift in [-1700., -5000., 0.] {
            let mut scene = Scene::default();
            let mut matrix = super::IDENTITY;
            matrix[12] = shift;
            let state = EffectState {
                transform: Some(matrix),
                ..effect(mask)
            };
            scene.push_effect(state, Some(9));
            let inside = Clip::from(bounds([-1e6, -1e6, 2e6, 2e6]));
            let quad = bounds([100. - shift, 100., 100., 100.]);
            scene.push(Instance::new(BOX, quad, inside));
            scene.pop_effect();
            let mut frame = RenderFrame::default();
            frame.record(&scene, [800, 600], 16384, true);
            let source = frame.commands.iter().find_map(|command| match command {
                Command::Composite { index, .. } => Some(frame.composites[*index as usize].bounds),
                _ => None,
            });
            let [x, _, width, _] = source.expect("the layer draws");
            assert!(
                x <= 100. - shift && 200. - shift <= x + width,
                "{shift}: {source:?}"
            );
        }
    }

    #[test]
    fn patched_blurs_draw_like_new_frames() {
        let mask = Clip::from(bounds([0., 0., 800., 600.]));
        let scene = |blur: f32, backdrop: f32, around: bool| {
            let mut scene = Scene::default();
            scene.push(Instance::new(BOX, bounds([0., 0., 400., 300.]), mask));
            if around {
                let state = EffectState {
                    opacity: 0.5,
                    ..effect(mask)
                };
                scene.push_effect(state, None);
            }
            let state = EffectState {
                blur,
                backdrop: Some(BackdropState {
                    bounds: bounds([100., 100., 100., 100.]),
                    corner_radii: Default::default(),
                    blur: backdrop,
                    color_matrix: None,
                }),
                ..effect(mask)
            };
            scene.push_effect(state, Some(9));
            scene.push(Instance::new(BOX, bounds([100., 100., 100., 100.]), mask));
            scene.pop_effect();
            if around {
                scene.pop_effect();
            }
            scene
        };
        let draw = |scene: &Scene| {
            let mut frame = RenderFrame::default();
            frame.record(scene, [800, 600], 16384, true);
            let order: Vec<_> = frame
                .commands
                .iter()
                .map(|command| match command {
                    Command::Pass(..) => "pass".into(),
                    Command::Draw { instances, .. } => format!("draw {instances:?}"),
                    Command::Blur { index, .. } => format!("blur {index}"),
                    Command::Copy { region, .. } => format!("copy {region:?}"),
                    Command::Composite { index, .. } => format!("composite {index}"),
                })
                .collect();
            (
                order,
                bytes(&frame.instances).to_vec(),
                bytes(&frame.composites).to_vec(),
                bytes(&frame.blurs).to_vec(),
                scene.layers[0].nested[0].bounds,
            )
        };
        for (blur, backdrop) in [(8., 0.), (0., 6.), (12., 4.), (0., 0.)] {
            let mut patched = scene(2., 0., false);
            patched.drawn();
            assert!(patched.patch(9, 1., None, [Some(blur), Some(backdrop)]));
            assert_eq!(draw(&patched), draw(&scene(blur, backdrop, false)));
            // The redrawn region holds the old and the new reach of the blur.
            let reach = bounds([100., 100., 100., 100.]).outset(blur.max(2.) * 3.);
            let [damage] = patched.damage.rects[..] else {
                panic!("one region")
            };
            assert_eq!(damage.intersect(reach), reach);
        }
        // A layer inside another can't grow past the reach it was drawn with. Its parent keeps the old reach.
        let mut nested = scene(8., 0., true);
        assert!(nested.patch(9, 1., None, [Some(2.), None]));
        let (drawn, fresh) = (draw(&nested), draw(&scene(2., 0., true)));
        assert_eq!((drawn.0, drawn.3), (fresh.0, fresh.3));
        assert!(!nested.patch(9, 1., None, [Some(12.), None]));
    }

    #[test]
    fn wide_blurs_draw_at_a_fraction_of_the_pixels() {
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
        // Two halves leave 5 pixels per sigma, where both passes blur.
        let wide = blurred(20.);
        let [half, quarter, across, down] = wide.blurs[..] else {
            panic!("two halves and two blur passes")
        };
        assert_eq!(
            (half.sigma, half.scale, quarter.sigma, quarter.scale),
            (0., 2., 0., 4.)
        );
        assert_eq!((across.direction, across.scale), ([1., 0.], 4.));
        assert_eq!((down.direction, down.scale), ([0., 1.], 4.));
        assert!((across.sigma - (400f32 - 15. / 12.).sqrt()).abs() < 1e-4);
        let composite = wide.composites[0];
        assert_eq!(composite.blur, 0.);
        let Some(Command::Composite { src, .. }) = wide.commands.last() else {
            panic!("a composite last")
        };
        let [width, height] = wide.pixels(*src);
        assert_eq!(composite.source_rect[2..], [width * 4., height * 4.]);
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

    #[test]
    fn layers_that_draw_as_before_keep_their_pixels() {
        let mask = Clip::from(bounds([0., 0., 400., 300.]));
        let scene = |red: f32, opacity: f32| {
            let mut scene = Scene::default();
            scene.push(Instance::new(BOX, bounds([0., 0., 400., 300.]), mask));
            scene.push_effect(
                EffectState {
                    opacity,
                    ..effect(mask)
                },
                None,
            );
            let mut instance = Instance::new(BOX, bounds([10., 10., 50., 50.]), mask);
            instance.color = [red, 0., 0., 1.];
            scene.push(instance);
            scene.pop_effect();
            scene.diff(Some(&[]));
            scene
        };
        let (mut frame, mut pool, mut made) = (RenderFrame::default(), super::Pool::default(), 0);
        let mut draw = |frame: &mut RenderFrame, scene: &Scene| {
            frame.record(scene, [400, 300], 16384, true);
            pool.fill(frame, |_| {
                made += 1;
                Ok(made)
            })
            .unwrap();
            let target = frame.commands.iter().find_map(|command| match command {
                Command::Composite { src, .. } => Some(*src),
                _ => None,
            });
            (
                frame.instances.len(),
                target.and_then(|target| pool.get(frame, target)).copied(),
            )
        };
        let (drawn, _) = draw(&mut frame, &scene(1., 0.5));
        assert_eq!(drawn, 2);
        let (drawn, kept) = draw(&mut frame, &scene(1., 0.5));
        assert_eq!(drawn, 2, "the second frame keeps the layer");
        let (drawn, reused) = draw(&mut frame, &scene(1., 0.25));
        assert_eq!(
            (drawn, reused),
            (1, kept),
            "a new opacity draws the kept pixels"
        );
        let mut stale = scene(1., 0.5);
        stale.stale = stale.digests.clone();
        assert_eq!(draw(&mut frame, &stale).0, 2, "a stale layer draws again");
        for red in 0..12 {
            for _ in 0..2 {
                draw(&mut frame, &scene(red as f32 / 12., 0.5));
            }
        }
        assert!(frame.cache.len() <= 5, "unused layers go");
        assert!(made <= 8, "the textures of dropped layers are used again");
    }

    #[test]
    fn layers_that_draw_as_before_keep_their_blurs() {
        let mask = Clip::from(bounds([0., 0., 800., 600.]));
        let scene = |x: f32, sigma: f32| {
            let mut scene = Scene::default();
            scene.push(Instance::new(BOX, bounds([0., 0., 800., 600.]), mask));
            let state = EffectState {
                blur: sigma,
                shadow: Some((crate::point(4., 4.), 12., crate::Color::BLACK)),
                ..effect(mask)
            };
            scene.push_effect(state, None);
            scene.push(Instance::new(BOX, bounds([x, 100., 100., 100.]), mask));
            scene.pop_effect();
            scene.diff(Some(&[]));
            scene
        };
        let mut frame = RenderFrame::default();
        let mut blurs = |scene: &Scene| {
            frame.record(scene, [800, 600], 16384, true);
            let blurs = frame.commands.iter();
            blurs
                .filter(|command| matches!(command, Command::Blur { .. }))
                .count()
        };
        assert_eq!(
            blurs(&scene(100., 2.)),
            4,
            "a shadow at half resolution, its half, and a blur"
        );
        assert_eq!(
            blurs(&scene(100., 2.)),
            4,
            "the second frame keeps the blurs"
        );
        assert_eq!(
            blurs(&scene(100., 2.)),
            0,
            "the third frame draws the kept blurs"
        );
        assert_eq!(blurs(&scene(160., 2.)), 0, "a moved layer keeps its blurs");
        assert_eq!(blurs(&scene(160., 3.)), 1, "another blur draws again");
    }

    #[test]
    fn layers_that_read_below_draw_whole_with_damage_that_reaches_them() {
        let mask = Clip::from(bounds([0., 0., 400., 300.]));
        let mut scene = Scene::default();
        scene.push(Instance::new(BOX, bounds([0., 0., 400., 300.]), mask));
        scene.push_effect(
            EffectState {
                backdrop: Some(BackdropState {
                    bounds: bounds([100., 100., 100., 50.]),
                    corner_radii: Default::default(),
                    blur: 4.,
                    color_matrix: None,
                }),
                ..effect(mask)
            },
            None,
        );
        scene.pop_effect();
        scene.diff(Some(&[]));
        scene.drawn();
        let regions = |scene: &mut Scene, rect: [f32; 4]| {
            scene.damage.add(bounds(rect));
            let regions = super::damage_regions(scene, [400, 300], &[]);
            scene.damage = Default::default();
            regions
        };
        assert_eq!(
            regions(&mut scene, [10., 10., 8., 8.]),
            Some(vec![[9, 9, 10, 10]])
        );
        assert_eq!(
            regions(&mut scene, [190., 140., 20., 20.]),
            Some(vec![[100, 100, 111, 61]])
        );
        assert_eq!(
            regions(&mut scene, [0., 0., 300., 300.]),
            None,
            "most of the window"
        );
    }
}
