use crate::{
    Clip, Color, Pixels, Point, Radii, Rect, Size,
    atlas::Tile,
    geometry::{region, transform_bounds},
    gpu::{Instance, SHADOW, bytes, rect},
    point,
};
use rustc_hash::FxHashMap;
use std::{
    hash::{Hash, Hasher},
    mem, slice,
    sync::atomic::{AtomicU64, Ordering::Relaxed},
};

pub struct Scene {
    pub(crate) layers: Vec<SceneLayer>,
    // The layers of the last painted scene, which spans of that paint copy from.
    last: Vec<SceneLayer>,
    // The paint pass of `layers`, and of `last`.
    pass: Option<u32>,
    pub(crate) last_pass: Option<u32>,
    active: Vec<usize>,
    spare: Vec<SceneLayer>,
    keyed: Vec<(u64, usize)>,
    pub(crate) shots: Vec<Shot>,
    // The window region that changed since the last drawn frame.
    pub(crate) damage: Damage,
    // What each layer draws, with its effects and its nested layers. 0 for a layer that the window doesn't show.
    pub(crate) digests: Vec<u64>,
    // Digests whose cached pixels are wrong, since an atlas tile that they read has new pixels.
    pub(crate) stale: Vec<u64>,
    // Layers that read the pixels below them, in window pixels.
    pub(crate) reads: Vec<(Rect<f32>, u32)>,
    // Atlas tiles removed since the last paint: (color page, page, texels). A span of `last` that reads one
    // can't be copied, but its box can paint again while other boxes copy theirs.
    removed: Vec<(bool, u32, Rect<i32>)>,
    entries: Vec<Entry>,
    maps: Vec<Map>,
    // Layers that patches changed since the last diff, so `maps` doesn't place what they hold.
    moved: Vec<usize>,
    prior: Vec<Entry>,
    prior_maps: Vec<Map>,
    // `prior` holds what the window shows, and `entries` this scene since `diff`.
    drawn: bool,
    // `Some` after `clear`, and true after `diff`.
    diffed: Option<bool>,
    // Paint that the content mask cut away: instances outside their clip, or content that paint skipped.
    culled: u32,
    // Patched regions since the last diff, which the prior entries don't know.
    patched: Damage,
    salt: u64,
    // The content of scroll containers in the window layer, in paint order, and those of the last painted scene.
    scrolls: Vec<Scroll>,
    last_scrolls: Vec<Scroll>,
    prior_scrolls: Vec<Scroll>,
    // Open scrolls of the paint, or `usize::MAX` for one outside the window layer or paused.
    open: Vec<usize>,
    // For each `pause_cuts`, the open scrolls and cuts that it ended: (depth, scroll, cut), `usize::MAX` for none.
    held: Vec<Vec<(usize, usize, usize)>>,
    cuts: Vec<Cut>,
    last_cuts: Vec<Cut>,
    // Open cuts of the paint, or `usize::MAX` for a scroll without one.
    cutting: Vec<usize>,
    // The clips that `bake` cut, while the scene is baked.
    raw: Vec<Raw>,
    baked: bool,
    owners: [Vec<u32>; 2],
    // The window region that shows a scroll container, and how far its content moved since the drawn frame, in
    // device pixels: the backend copies the last frame moved by it there and draws only `damage`.
    pub(crate) shift: Option<Shift>,
    // The window below what paints after the content of a scroll, when something there covers its viewport.
    pub(crate) under: Option<Under>,
    // The scroll of `under`, kept while frames don't move it, and the split of the last diff.
    under_scroll: Option<u64>,
    under_split: Option<[u32; 2]>,
}

// The items of the window layer up to the end of a scroll's content, which a backend can keep drawn across frames, so
// a scroll draws again only what changed in them, and the items after them over it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Under {
    pub scroll: u64,
    // Where the items after the content start: an instance index and a nested layer index.
    pub split: [u32; 2],
    // What changed below the items after the content, within `damage`.
    pub damage: Damage,
}

impl Under {
    // Whether a top item of the window layer (an instance, or NESTED with a nested index) paints after the split.
    pub fn after(split: [u32; 2], top: u32) -> bool {
        match top & NESTED {
            0 => top >= split[0],
            _ => top & !NESTED >= split[1],
        }
    }
}

// A viewport region in device pixels, and how far its content moved.
pub(crate) type Shift = ([i32; 4], [i32; 2]);

// The instances and nested layers of the window layer that a scroll container paints. `shift` added to their
// places gives places in the content, which scrolling doesn't change.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scroll {
    id: u64,
    viewport: Clip<f32>,
    shift: Point<f32>,
    instances: [u32; 2],
    nested: [u32; 2],
    // The scroll around it, or u32::MAX.
    parent: u32,
}

// The content of a scroll container that painted in an interest area larger than its viewport, in any layer:
// `bake` clips it to the viewport.
#[derive(Clone, Copy, Debug)]
struct Cut {
    layer: u32,
    instances: [u32; 2],
    nested: [u32; 2],
    viewport: Clip<f32>,
}

// A clip that `bake` cut to a viewport, to put back before the scene is a source of copies.
#[derive(Clone, Copy)]
enum Raw {
    Instance(u32, u32, [f32; 4]),
    Nested(u32, u32, Rect<f32>),
    Effect(u32, Rect<f32>),
}

// The window region to draw again: a few rectangles, or the whole window.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Damage {
    pub rects: Vec<Rect<f32>>,
    pub whole: bool,
}

const DAMAGE_RECTS: usize = 16;
// A rectangle joins another when their union adds at most this many pixels.
const DAMAGE_SLACK: f32 = 64. * 64.;

impl Damage {
    // The window regions to draw again, or `None` for the whole window. A layer in `reads` draws whole when a region
    // reaches it, since it reads only what the frame drew again.
    pub(crate) fn regions(
        &self,
        reads: impl Iterator<Item = Rect<f32>>,
        window: [u32; 2],
        extra: &[[i32; 4]],
    ) -> Option<Vec<[i32; 4]>> {
        if self.whole {
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
        for rect in &self.rects {
            let rect = rect.outset(1.).intersect(bounds);
            if !rect.is_empty() {
                add(&mut regions, region(rect));
            }
        }
        for &region in extra {
            add(&mut regions, region);
        }
        let reads: Vec<[i32; 4]> = reads
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

    pub fn add(&mut self, rect: Rect<f32>) {
        if self.whole || rect.is_empty() {
            return;
        }
        if !(rect.origin.x.is_finite() && rect.origin.y.is_finite())
            || !(rect.size.width.is_finite() && rect.size.height.is_finite())
        {
            self.whole = true;
            return;
        }
        let area = |rect: Rect<f32>| rect.size.width * rect.size.height;
        let mut rect = rect;
        loop {
            let best = (self.rects.iter().enumerate())
                .map(|(at, other)| (at, area(other.union(rect)) - area(*other) - area(rect)))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((at, growth))
                    if growth <= DAMAGE_SLACK || self.rects.len() >= DAMAGE_RECTS =>
                {
                    rect = self.rects.swap_remove(at).union(rect);
                }
                _ => break self.rects.push(rect),
            }
        }
    }

    fn clear(&mut self) {
        self.rects.clear();
        self.whole = false;
    }
}

// An instance or a nested layer of the window, in paint order. `reach` is in the space of `layer`.
#[derive(Clone, Copy)]
struct Entry {
    hash: u64,
    reach: Rect<f32>,
    layer: u32,
    // The instance index, or NESTED with the nested layer.
    item: u32,
    // The innermost scroll that holds the entry, plus one, or 0.
    scroll: u32,
    // The item of the window layer that holds the entry, as `item`.
    top: u32,
}

pub(crate) const NESTED: u32 = 1 << 31;

// How a nested layer shows its pixels in its parent.
#[derive(Clone, Copy, Default)]
struct Map {
    parent: u32,
    clip: Rect<f32>,
    spread: f32,
    shadow: Option<(Point<f32>, f32)>,
    transform: Option<[f32; 16]>,
}

fn bits<const N: usize>(values: [f32; N]) -> [u32; N] {
    values.map(f32::to_bits)
}

pub(crate) fn mix(hash: u64, value: u64) -> u64 {
    (hash.rotate_left(5) ^ value).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95)
}

// The place of a layer that its digest is relative to, so a layer that moves keeps its digest.
pub(crate) fn origin(layer: &SceneLayer) -> Point<f32> {
    layer.bounds.map_or_else(Point::default, |bounds| {
        point(bounds.origin.x.floor(), bounds.origin.y.floor())
    })
}

fn place(origin: Point<f32>) -> u64 {
    u64::from(origin.x.to_bits()) << 32 | u64::from(origin.y.to_bits())
}

// A clip changes the pixels only where it cuts the instance, so the reach stands for it. Places are
// relative to `origin`.
pub(crate) fn within(area: Rect<f32>, clip: Rect<f32>) -> bool {
    area.is_empty() || area.intersect(clip) == area
}

// Clips at least this wide are the open mask of an effect layer.
const OPEN: f32 = 1e6;

// A clip of a moved span: the mask `old` around the span becomes the mask `now` there, the open mask of an
// effect layer stays, and other clips are inside the span and move with it.
fn moved_clip(clip: Clip<f32>, by: Point<f32>, old: &Clip<f32>, now: &Clip<f32>) -> Clip<f32> {
    if clip == *old {
        return *now;
    }
    let mut moved = clip;
    if clip.bounds.size.width < OPEN {
        moved.bounds = clip.bounds + by;
    }
    if (clip.rounded, clip.radii) == (old.rounded, old.radii) {
        (moved.rounded, moved.radii) = (now.rounded, now.radii);
    } else if !clip.radii.is_zero() {
        moved.rounded = clip.rounded + by;
    }
    moved
}

fn moved_instance(
    instance: &Instance,
    by: Point<f32>,
    old: &Clip<f32>,
    now: &Clip<f32>,
) -> Instance {
    let shift = |[x, y, width, height]: [f32; 4]| [x + by.x, y + by.y, width, height];
    let mut copy = *instance;
    copy.rect = shift(instance.rect);
    if instance.kind() == SHADOW {
        copy.aux = shift(instance.aux);
    }
    let parts = |clip: &Clip<f32>| {
        (
            clip.bounds.to_array(),
            clip.rounded.to_array(),
            clip.radii.to_array(),
            clip.radii.shape_array(),
        )
    };
    let (old, now) = (parts(old), parts(now));
    let shape = [instance.shape[2], instance.shape[3]];
    let round = (instance.round, instance.round_radii, shape);
    if instance.clip == old.0 && round == (old.1, old.2, old.3) {
        (copy.clip, copy.round, copy.round_radii) = (now.0, now.1, now.2);
        [copy.shape[2], copy.shape[3]] = now.3;
        return copy;
    }
    if instance.clip[2] < OPEN {
        copy.clip = shift(instance.clip);
    }
    if round == (old.1, old.2, old.3) {
        (copy.round, copy.round_radii) = (now.1, now.2);
        [copy.shape[2], copy.shape[3]] = now.3;
    } else if instance.round_radii != [0.; 8] {
        copy.round = shift(instance.round);
    }
    copy
}

fn moved_nested(nested: &Nested, by: Point<f32>, old: &Clip<f32>, now: &Clip<f32>) -> Nested {
    let (bounds, clip) = (nested.bounds + by, moved_clip(nested.clip, by, old, now));
    Nested {
        bounds,
        clip,
        grow: bounds.intersect(clip.bounds),
        ..*nested
    }
}

fn moved_effect(
    effect: &EffectState,
    by: Point<f32>,
    old: &Clip<f32>,
    now: &Clip<f32>,
) -> EffectState {
    let mut moved = effect.clone();
    // The transform turns about a point that moved too: T(by) * M * T(-by), for the affine ones of `plain`.
    if let Some(m) = moved.transform.as_mut() {
        let [x, y] = [by.x, by.y];
        m[12] += x - (m[0] * x + m[4] * y);
        m[13] += y - (m[1] * x + m[5] * y);
        m[14] -= m[2] * x + m[6] * y;
    }
    if let Some((clip, _)) = moved.clip.as_mut() {
        *clip = *clip + by;
    }
    if let Some((_, rect)) = moved.mask.as_mut() {
        *rect = *rect + by;
    }
    moved.content_mask = moved_clip(effect.content_mask, by, old, now);
    moved
}

// Inside `outer` on all sides, so a clip there is not cut by it.
fn inside(rect: Rect<f32>, outer: Rect<f32>) -> bool {
    rect.left() > outer.left()
        && rect.top() > outer.top()
        && rect.right() < outer.right()
        && rect.bottom() < outer.bottom()
}

// In a scroll, the mask around the content stays where the content moves, so a clip that is only that mask
// counts as the place of the instance itself.
fn instance_hash(instance: &Instance, origin: Point<f32>, viewport: Option<&Clip<f32>>) -> u64 {
    let mut copy = *instance;
    let shift = |[x, y, width, height]: [f32; 4]| [x - origin.x, y - origin.y, width, height];
    let masked = viewport.is_some_and(|viewport| instance.clip == viewport.bounds.to_array());
    copy.rect = shift(instance.rect);
    copy.clip = shift(match masked {
        true => instance.area().to_array(),
        false => instance.reach().to_array(),
    });
    copy.round = match instance.round_radii == [0.; 8] {
        true => [0.; 4],
        false if viewport.is_some_and(|viewport| instance.round == viewport.rounded.to_array()) => {
            [f32::MIN; 4]
        }
        false => shift(instance.round),
    };
    if instance.kind() == SHADOW {
        copy.aux = shift(instance.aux);
    }
    // Four lanes don't wait for each other.
    let (rows, _) = bytes(slice::from_ref(&copy)).as_chunks::<32>();
    let mut lanes = [1, 2, 3, 4];
    for row in rows {
        let (words, _) = row.as_chunks::<8>();
        for (lane, word) in lanes.iter_mut().zip(words) {
            *lane = mix(*lane, u64::from_ne_bytes(*word));
        }
    }
    lanes.into_iter().fold(0, mix)
}

// The pixels of `rect` in a layer reach this in its parent.
fn map_rect(map: &Map, rect: Rect<f32>) -> Rect<f32> {
    let mut bounds = rect.outset(map.spread);
    if let Some((offset, sigma)) = map.shadow {
        bounds = bounds.union(rect.outset(sigma * 3.) + offset);
    }
    if let Some(matrix) = &map.transform {
        bounds = transform_bounds(matrix, bounds);
    }
    bounds.intersect(map.clip)
}

fn window_reach(maps: &[Map], mut layer: u32, mut rect: Rect<f32>) -> Rect<f32> {
    while layer != 0 && !rect.is_empty() {
        let map = &maps[layer as usize];
        rect = map_rect(map, rect);
        layer = map.parent;
    }
    rect
}

// A layer of `push_offscreen` whose pixels the backend reads when `read` is set.
pub(crate) struct Shot {
    pub key: u64,
    pub layer: usize,
    pub region: [i32; 4],
    pub read: bool,
}

#[derive(Default)]
pub(crate) struct SceneLayer {
    pub(crate) instances: Vec<Instance>,
    pub(crate) nested: Vec<Nested>,
    pub(crate) effect: Option<EffectState>,
    pub(crate) bounds: Option<Rect<f32>>,
}

// Composited before the instance at `at` of its parent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Nested {
    pub at: u32,
    pub layer: usize,
    pub bounds: Rect<f32>,
    pub clip: Clip<f32>,
    // What the layer adds to the bounds of its parent.
    pub grow: Rect<f32>,
}

// Where a paint span starts in the scene.
#[derive(Clone, Copy)]
pub(crate) struct Mark {
    layer: u32,
    instances: u32,
    nested: u32,
    layers: u32,
    keyed: u32,
    shots: u32,
    scrolls: u32,
    cuts: u32,
    culled: u32,
}

// What a paint span added to the scene: instances and nested layers of its layer, and the layers it made.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Span {
    layer: u32,
    instances: [u32; 2],
    nested: [u32; 2],
    layers: [u32; 2],
    scrolls: [u32; 2],
    cuts: [u32; 2],
}

impl Nested {
    pub(crate) fn reach(&self) -> Rect<f32> {
        self.bounds.intersect(self.clip.bounds)
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            layers: vec![SceneLayer::default()],
            last: Vec::new(),
            pass: None,
            last_pass: None,
            active: Vec::new(),
            spare: Vec::new(),
            keyed: Vec::new(),
            shots: Vec::new(),
            damage: Damage {
                rects: Vec::new(),
                whole: true,
            },
            digests: Vec::new(),
            stale: Vec::new(),
            reads: Vec::new(),
            removed: Vec::new(),
            entries: Vec::new(),
            maps: Vec::new(),
            moved: Vec::new(),
            prior: Vec::new(),
            prior_maps: Vec::new(),
            drawn: false,
            diffed: None,
            patched: Damage::default(),
            culled: 0,
            salt: 0,
            scrolls: Vec::new(),
            last_scrolls: Vec::new(),
            prior_scrolls: Vec::new(),
            open: Vec::new(),
            held: Vec::new(),
            cuts: Vec::new(),
            last_cuts: Vec::new(),
            cutting: Vec::new(),
            raw: Vec::new(),
            baked: false,
            owners: Default::default(),
            shift: None,
            under: None,
            under_scroll: None,
            under_split: None,
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
        self.unbake();
        self.active.clear();
        self.keyed.clear();
        self.shots.clear();
        self.damage.clear();
        self.damage.whole = true;
        self.shift = None;
        self.under = None;
        self.diffed = Some(false);
        self.open.clear();
        self.held.clear();
        self.cutting.clear();
        // A painted scene stays as the source of the next paint's spans.
        if let Some(pass) = self.pass.take() {
            mem::swap(&mut self.last_scrolls, &mut self.scrolls);
            self.scrolls.clear();
            mem::swap(&mut self.last_cuts, &mut self.cuts);
            self.cuts.clear();
            mem::swap(&mut self.last, &mut self.layers);
            // In reverse, so the next paint takes each layer's buffers back in order and the large root buffer
            // goes to the root. Else large buffers move to small layers and all spare layers grow to the largest.
            for mut layer in self.layers.drain(..).rev() {
                layer.clear();
                self.spare.push(layer);
            }
            self.last_pass = Some(pass);
            self.layers.push(self.spare.pop().unwrap_or_default());
            return;
        }
        for mut layer in self.layers.drain(1..).rev() {
            layer.clear();
            self.spare.push(layer);
        }
        self.layers[0].clear();
        self.scrolls.clear();
        self.cuts.clear();
    }

    // The paint of a scroll container's content starts. `by` is its scroll offset in device pixels, and `viewport`
    // the mask around its content. Only the window layer keeps scrolls.
    pub(crate) fn begin_scroll(
        &mut self,
        id: u64,
        viewport: Clip<f32>,
        by: Point<f32>,
        open: bool,
    ) {
        let layer = self.active.last().copied().unwrap_or(0);
        self.cutting.push(match open {
            true => self.cuts.len(),
            false => usize::MAX,
        });
        if open {
            let layer = &self.layers[layer];
            self.cuts.push(Cut {
                layer: self.active.last().copied().unwrap_or(0) as u32,
                instances: [layer.instances.len() as u32, u32::MAX],
                nested: [layer.nested.len() as u32, u32::MAX],
                viewport,
            });
        }
        if !self.active.is_empty() {
            return self.open.push(usize::MAX);
        }
        let parent = self.outer();
        let outer = (parent != u32::MAX).then(|| self.scrolls[parent as usize].shift);
        let layer = &self.layers[0];
        self.open.push(self.scrolls.len());
        self.scrolls.push(Scroll {
            id,
            viewport,
            shift: outer.unwrap_or_default() + by,
            instances: [layer.instances.len() as u32, u32::MAX],
            nested: [layer.nested.len() as u32, u32::MAX],
            parent,
        });
    }

    // Ends the open cuts after the first `depth` scrolls, for content that escapes their viewports. Their scrolls end
    // too, so the content doesn't move with them. A cut that an outer pause ended stays ended.
    pub(crate) fn pause_cuts(&mut self, depth: usize) {
        let mut held = Vec::new();
        for at in depth..self.open.len() {
            let scroll = mem::replace(&mut self.open[at], usize::MAX);
            let cut = mem::replace(&mut self.cutting[at], usize::MAX);
            if scroll != usize::MAX {
                let window = &self.layers[0];
                self.scrolls[scroll].instances[1] = window.instances.len() as u32;
                self.scrolls[scroll].nested[1] = window.nested.len() as u32;
            }
            if cut != usize::MAX {
                let cut = &mut self.cuts[cut];
                let layer = &self.layers[cut.layer as usize];
                cut.instances[1] = layer.instances.len() as u32;
                cut.nested[1] = layer.nested.len() as u32;
            }
            if scroll != usize::MAX || cut != usize::MAX {
                held.push((at, scroll, cut));
            }
        }
        self.held.push(held);
    }

    // Starts the cuts that `pause_cuts` ended again, and the scrolls in a new part with the same id.
    pub(crate) fn resume_cuts(&mut self) {
        for (at, scroll, cut) in self.held.pop().unwrap_or_default() {
            if scroll != usize::MAX {
                let parent = (self.open[..at].iter().rev())
                    .find(|&&outer| outer != usize::MAX)
                    .map_or(u32::MAX, |&outer| outer as u32);
                let window = &self.layers[0];
                self.open[at] = self.scrolls.len();
                self.scrolls.push(Scroll {
                    instances: [window.instances.len() as u32, u32::MAX],
                    nested: [window.nested.len() as u32, u32::MAX],
                    parent,
                    ..self.scrolls[scroll]
                });
            }
            if cut != usize::MAX {
                let cut = self.cuts[cut];
                let layer = &self.layers[cut.layer as usize];
                self.cutting[at] = self.cuts.len();
                self.cuts.push(Cut {
                    instances: [layer.instances.len() as u32, u32::MAX],
                    nested: [layer.nested.len() as u32, u32::MAX],
                    ..cut
                });
            }
        }
    }

    // Clips the content of open scrolls to their viewports, for the backend.
    fn bake(&mut self) {
        if mem::replace(&mut self.baked, true) {
            return;
        }
        let raw = &mut self.raw;
        for cut in &self.cuts {
            let (viewport, index) = (cut.viewport.bounds, cut.layer);
            let [start, end] = cut.instances.map(|at| at as usize);
            let layer = &mut self.layers[index as usize];
            for (at, instance) in (start..).zip(&mut layer.instances[start..end]) {
                let clip = rect(instance.clip);
                if !within(clip, viewport) {
                    raw.push(Raw::Instance(index, at as u32, instance.clip));
                    instance.clip = clip.intersect(viewport).to_array();
                }
            }
            let [start, end] = cut.nested.map(|at| at as usize);
            for at in start..end {
                let nested = &mut self.layers[index as usize].nested[at];
                if !within(nested.clip.bounds, viewport) {
                    raw.push(Raw::Nested(index, at as u32, nested.clip.bounds));
                    nested.clip.bounds = nested.clip.bounds.intersect(viewport);
                }
                let index = nested.layer;
                if let Some(effect) = self.layers[index].effect.as_mut()
                    && !within(effect.content_mask.bounds, viewport)
                {
                    raw.push(Raw::Effect(index as u32, effect.content_mask.bounds));
                    effect.content_mask.bounds = effect.content_mask.bounds.intersect(viewport);
                }
            }
        }
    }

    fn unbake(&mut self) {
        self.baked = false;
        for raw in self.raw.drain(..).rev() {
            match raw {
                Raw::Instance(layer, at, clip) => {
                    self.layers[layer as usize].instances[at as usize].clip = clip;
                }
                Raw::Nested(layer, at, bounds) => {
                    self.layers[layer as usize].nested[at as usize].clip.bounds = bounds;
                }
                Raw::Effect(at, bounds) => {
                    if let Some(effect) = self.layers[at as usize].effect.as_mut() {
                        effect.content_mask.bounds = bounds;
                    }
                }
            }
        }
    }

    // The innermost open scroll of the window layer, or u32::MAX.
    fn outer(&self) -> u32 {
        (self.open.iter().rev())
            .find(|&&at| at != usize::MAX)
            .map_or(u32::MAX, |&at| at as u32)
    }

    pub(crate) fn end_scroll(&mut self) {
        if let Some(at) = self.cutting.pop().filter(|&at| at != usize::MAX) {
            let cut = &mut self.cuts[at];
            let layer = &self.layers[cut.layer as usize];
            cut.instances[1] = layer.instances.len() as u32;
            cut.nested[1] = layer.nested.len() as u32;
        }
        if let Some(at) = self.open.pop().filter(|&at| at != usize::MAX) {
            let layer = &self.layers[0];
            let scroll = &mut self.scrolls[at];
            scroll.instances[1] = layer.instances.len() as u32;
            scroll.nested[1] = layer.nested.len() as u32;
        }
    }

    // Copies the scrolls that a span of the last paint made, moved by `by`.
    fn replay_scrolls(
        &mut self,
        span: &Span,
        mark: Mark,
        moved: Option<(Point<f32>, &Clip<f32>, &Clip<f32>)>,
    ) {
        let [first, end] = span.scrolls;
        if first == end || self.last_scrolls.len() < end as usize {
            return;
        }
        let outer = self.outer();
        let rebase =
            |[start, stop]: [u32; 2], from: u32, to: u32| [start - from + to, stop - from + to];
        for at in first..end {
            let scroll = self.last_scrolls[at as usize];
            let (viewport, shift) = match moved {
                Some((by, old, now)) => {
                    (moved_clip(scroll.viewport, by, old, now), scroll.shift - by)
                }
                None => (scroll.viewport, scroll.shift),
            };
            self.scrolls.push(Scroll {
                viewport,
                shift,
                instances: rebase(scroll.instances, span.instances[0], mark.instances),
                nested: rebase(scroll.nested, span.nested[0], mark.nested),
                parent: match (first..end).contains(&scroll.parent) {
                    true => scroll.parent - first + mark.scrolls,
                    false => outer,
                },
                ..scroll
            });
        }
    }

    // Copies the cuts that a span of the last paint made, moved by `by`.
    fn replay_cuts(
        &mut self,
        span: &Span,
        mark: Mark,
        moved: Option<(Point<f32>, &Clip<f32>, &Clip<f32>)>,
    ) {
        let [first, end] = span.cuts;
        if first == end || self.last_cuts.len() < end as usize {
            return;
        }
        for at in first..end {
            let cut = self.last_cuts[at as usize];
            let rebase = |[start, stop]: [u32; 2], from: u32, to: u32| match cut.layer == span.layer
            {
                true => [start - from + to, stop - from + to],
                false => [start, stop],
            };
            self.cuts.push(Cut {
                layer: match cut.layer == span.layer {
                    true => mark.layer,
                    false => cut.layer + mark.layers - span.layers[0],
                },
                instances: rebase(cut.instances, span.instances[0], mark.instances),
                nested: rebase(cut.nested, span.nested[0], mark.nested),
                viewport: match moved {
                    Some((by, old, now)) => moved_clip(cut.viewport, by, old, now),
                    None => cut.viewport,
                },
            });
        }
    }

    // `removed` lists the atlas tiles removed since the last paint, and returns true when all copies must stop.
    pub(crate) fn begin_paint(
        &mut self,
        pass: u32,
        removed: impl FnOnce(&mut Vec<(bool, u32, Rect<i32>)>) -> bool,
    ) -> bool {
        self.pass = Some(pass);
        removed(&mut self.removed)
    }

    /// Paint left out content, so a copy of it can't move.
    pub(crate) fn cull(&mut self) {
        self.culled += 1;
    }

    pub(crate) fn culled_since(&self, mark: Mark) -> bool {
        self.culled != mark.culled
    }

    // This paint stops, so the last one stays the source of copies.
    #[cfg_attr(debug_assertions, allow(dead_code))]
    pub(crate) fn abort(&mut self) {
        self.pass = None;
    }

    // Spans of the last paint can't be copied anymore.
    pub(crate) fn forget(&mut self) {
        self.last_pass = None;
    }

    pub(crate) fn mark(&self) -> Mark {
        let layer = self.active.last().copied().unwrap_or(0);
        Mark {
            layer: layer as u32,
            instances: self.layers[layer].instances.len() as u32,
            nested: self.layers[layer].nested.len() as u32,
            layers: self.layers.len() as u32,
            keyed: self.keyed.len() as u32,
            shots: self.shots.len() as u32,
            scrolls: self.scrolls.len() as u32,
            cuts: self.cuts.len() as u32,
            culled: self.culled,
        }
    }

    // `None` when the span left its layer, or made keyed or snapshot layers, which a copy can't keep.
    pub(crate) fn span(&self, mark: Mark) -> Option<Span> {
        let layer = self.active.last().copied().unwrap_or(0);
        (layer as u32 == mark.layer
            && self.keyed.len() as u32 == mark.keyed
            && self.shots.len() as u32 == mark.shots)
            .then(|| Span {
                layer: mark.layer,
                instances: [mark.instances, self.layers[layer].instances.len() as u32],
                nested: [mark.nested, self.layers[layer].nested.len() as u32],
                layers: [mark.layers, self.layers.len() as u32],
                scrolls: [mark.scrolls, self.scrolls.len() as u32],
                cuts: [mark.cuts, self.cuts.len() as u32],
            })
    }

    // What a span drew, with layers and places relative to it, to compare a span of the last paint with a new one.
    #[cfg(debug_assertions)]
    pub(crate) fn records(
        &self,
        span: &Span,
        last: bool,
        moved: Option<(Point<f32>, Clip<f32>, Clip<f32>)>,
    ) -> Vec<String> {
        let layers = if last { &self.last } else { &self.layers };
        let shift = |layer: usize| layer as i64 - i64::from(span.layers[0]);
        let mut out = Vec::new();
        let push = |out: &mut Vec<String>,
                    layer: &SceneLayer,
                    [first, end]: [u32; 2],
                    [start, stop]: [u32; 2],
                    name: i64| {
            for instance in &layer.instances[first as usize..end as usize] {
                let instance = moved.map_or(*instance, |(by, old, now)| {
                    moved_instance(instance, by, &old, &now)
                });
                out.push(format!("{name} {:?}", bytes(slice::from_ref(&instance))));
            }
            for nested in &layer.nested[start as usize..stop as usize] {
                let nested = moved.map_or(*nested, |(by, old, now)| {
                    moved_nested(nested, by, &old, &now)
                });
                let moved = Nested {
                    at: nested.at - first,
                    layer: 0,
                    ..nested
                };
                out.push(format!("{name} nested {} {moved:?}", shift(nested.layer)));
            }
        };
        let own = &layers[span.layer as usize];
        push(&mut out, own, span.instances, span.nested, -1);
        for at in span.layers[0]..span.layers[1] {
            let layer = &layers[at as usize];
            let all = |len: usize| [0, len as u32];
            let (instances, nested) = (all(layer.instances.len()), all(layer.nested.len()));
            push(&mut out, layer, instances, nested, shift(at as usize));
            if let Some(effect) = &layer.effect {
                let effect = &moved.map_or_else(
                    || effect.clone(),
                    |(by, old, now)| moved_effect(effect, by, &old, &now),
                );
                let bounds = layer
                    .bounds
                    .map(|bounds| moved.map_or(bounds, |(by, ..)| bounds + by));
                out.push(format!(
                    "{} bounds {:?} effect {:?} {} {:?} {} {:?} {:?} {:?} {:?}",
                    shift(at as usize),
                    bounds,
                    effect.transform,
                    effect.opacity,
                    effect.color_matrix,
                    effect.blur,
                    effect.clip,
                    effect
                        .mask
                        .as_ref()
                        .map(|(tile, rect)| (tile.page, tile.rect, *rect)),
                    effect.content_mask,
                    effect.text_mask.map(shift),
                ));
            }
        }
        out
    }

    // Copies a span of the last paint into the current layer, and returns where it is now.
    // Debug builds paint instead, to check the copy.
    #[cfg_attr(debug_assertions, allow(dead_code))]
    // Whether the span of `last` reads an atlas tile that was removed since that paint.
    fn reads_removed(&self, span: &Span) -> bool {
        if self.removed.is_empty() {
            return false;
        }
        let removed = |(color, page, texels): (bool, u32, Rect<f32>)| {
            let texels = texels.map(|value| value as i32);
            (self.removed.iter()).any(|&(at_color, at, rect)| {
                at_color == color && at == page && rect.intersects(texels)
            })
        };
        let read = |instance: &Instance| {
            instance
                .page()
                .is_some_and(|(color, page)| removed((color, page, rect(instance.aux))))
        };
        let [start, end] = span.instances.map(|at| at as usize);
        self.last[span.layer as usize].instances[start..end]
            .iter()
            .any(read)
            || self.last[span.layers[0] as usize..span.layers[1] as usize]
                .iter()
                .any(|layer| {
                    layer.instances.iter().any(read)
                        || (layer.effect.as_ref())
                            .and_then(|effect| effect.mask.as_ref())
                            .is_some_and(|(tile, _)| {
                                removed((true, tile.page, tile.rect.map(|value| value as f32)))
                            })
                })
    }

    pub(crate) fn replay(&mut self, span: &Span) -> Option<Span> {
        let source = self.last.get(span.layer as usize)?;
        if source.instances.len() < span.instances[1] as usize
            || source.nested.len() < span.nested[1] as usize
            || self.last.len() < span.layers[1] as usize
            || self.reads_removed(span)
        {
            return None;
        }
        let mark = self.mark();
        let shift = |layer: usize| layer + mark.layers as usize - span.layers[0] as usize;
        for at in span.layers[0]..span.layers[1] {
            let source = &self.last[at as usize];
            let mut layer = self.spare.pop().unwrap_or_default();
            layer.instances.extend_from_slice(&source.instances);
            layer
                .nested
                .extend(source.nested.iter().map(|nested| Nested {
                    layer: shift(nested.layer),
                    ..*nested
                }));
            layer.effect = source.effect.clone().map(|mut effect| {
                effect.text_mask = effect.text_mask.map(shift);
                effect
            });
            layer.bounds = source.bounds;
            self.layers.push(layer);
        }
        let source = &self.last[span.layer as usize];
        let target = &mut self.layers[mark.layer as usize];
        let [start, end] = span.instances.map(|at| at as usize);
        for instance in &source.instances[start..end] {
            target.grow(instance.reach());
        }
        target
            .instances
            .extend_from_slice(&source.instances[start..end]);
        let [first, last] = span.nested.map(|at| at as usize);
        for nested in &source.nested[first..last] {
            target.grow(nested.grow);
            target.nested.push(Nested {
                at: nested.at - span.instances[0] + mark.instances,
                layer: shift(nested.layer),
                ..*nested
            });
        }
        self.replay_scrolls(span, mark, None);
        self.replay_cuts(span, mark, None);
        self.span(mark)
    }

    // The area in `clip` that a span of this paint holds, when a copy of it moved by whole pixels draws as its
    // paint there would: its clips are the mask around it, with all it draws inside the mask, or clips inside
    // the mask, and its layers have no effects that read the pixels below or turn in perspective.
    pub(crate) fn plain(&self, span: &Span, clip: &Clip<f32>) -> Option<Rect<f32>> {
        if !self.translatable(span) {
            return None;
        }
        let layer = &self.layers[span.layer as usize];
        let held = |area: Rect<f32>, own: Rect<f32>, same: bool| {
            if same {
                within(area, clip.bounds).then_some(area)
            } else {
                inside(own, clip.bounds).then_some(own)
            }
        };
        let [start, end] = span.instances.map(|at| at as usize);
        let arrays = (
            clip.bounds.to_array(),
            clip.rounded.to_array(),
            clip.radii.to_array(),
        );
        let instances = layer.instances[start..end].iter().map(|instance| {
            let same = (instance.clip, instance.round, instance.round_radii) == arrays;
            held(instance.area(), rect(instance.clip), same)
        });
        let [first, last] = span.nested.map(|at| at as usize);
        let nested = (layer.nested[first..last].iter())
            .map(|nested| held(nested.bounds, nested.clip.bounds, nested.clip == *clip));
        instances
            .chain(nested)
            .try_fold(None, |all: Option<Rect<f32>>, area| {
                let area = area?;
                Some(Some(all.map_or(area, |all| all.union(area))))
            })
            .map(|all| all.unwrap_or_default())
    }

    // Whether the layers of a span have no effects that read the pixels below or a mask layer, or turn in
    // perspective, so a copy of the span can move.
    pub(crate) fn translatable(&self, span: &Span) -> bool {
        (span.layers[0]..span.layers[1]).all(|at| {
            self.layers[at as usize]
                .effect
                .as_ref()
                .is_some_and(|effect| {
                    effect.backdrop.is_none()
                        && effect.glass.is_none()
                        && effect.text_mask.is_none()
                        && effect
                            .transform
                            .is_none_or(|m| (m[3], m[7], m[11], m[15]) == (0., 0., 0., 1.))
                })
        })
    }

    // Copies a plain span of the last paint moved by `by` device pixels, from the mask `old` into `now`.
    // Debug builds paint instead, to check the copy.
    #[cfg_attr(debug_assertions, allow(dead_code))]
    pub(crate) fn replay_moved(
        &mut self,
        span: &Span,
        by: Point<f32>,
        old: &Clip<f32>,
        now: &Clip<f32>,
    ) -> Option<Span> {
        let source = self.last.get(span.layer as usize)?;
        if source.instances.len() < span.instances[1] as usize
            || source.nested.len() < span.nested[1] as usize
            || self.last.len() < span.layers[1] as usize
            || self.reads_removed(span)
        {
            return None;
        }
        let mark = self.mark();
        let shift = |layer: usize| layer + mark.layers as usize - span.layers[0] as usize;
        for at in span.layers[0]..span.layers[1] {
            let source = &self.last[at as usize];
            let mut layer = self.spare.pop().unwrap_or_default();
            (layer.instances).extend(
                source
                    .instances
                    .iter()
                    .map(|instance| moved_instance(instance, by, old, now)),
            );
            layer
                .nested
                .extend(source.nested.iter().map(|nested| Nested {
                    layer: shift(nested.layer),
                    ..moved_nested(nested, by, old, now)
                }));
            layer.effect =
                (source.effect.as_ref()).map(|effect| moved_effect(effect, by, old, now));
            layer.bounds = source.bounds.map(|bounds| bounds + by);
            self.layers.push(layer);
        }
        let source = &self.last[span.layer as usize];
        let target = &mut self.layers[mark.layer as usize];
        let [start, end] = span.instances.map(|at| at as usize);
        for instance in &source.instances[start..end] {
            let instance = moved_instance(instance, by, old, now);
            target.grow(instance.reach());
            target.instances.push(instance);
        }
        let [first, last] = span.nested.map(|at| at as usize);
        for nested in &source.nested[first..last] {
            let nested = moved_nested(nested, by, old, now);
            target.grow(nested.grow);
            target.nested.push(Nested {
                at: nested.at - span.instances[0] + mark.instances,
                layer: shift(nested.layer),
                ..nested
            });
        }
        self.replay_scrolls(span, mark, Some((by, old, now)));
        self.replay_cuts(span, mark, Some((by, old, now)));
        self.span(mark)
    }

    // The atlas texels that this scene reads: (color page, page, texels). Copies of its spans read them again.
    pub(crate) fn texels(&self) -> impl Iterator<Item = (bool, u32, Rect<f32>)> + '_ {
        self.layers.iter().flat_map(|layer| {
            let mask = (layer.effect.as_ref())
                .and_then(|effect| effect.mask.as_ref())
                .map(|(tile, _)| (true, tile.page, tile.rect.map(|value| value as f32)));
            (layer.instances.iter())
                .filter_map(|instance| {
                    let (color, page) = instance.page()?;
                    Some((color, page, rect(instance.aux)))
                })
                .chain(mask)
        })
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
        self.push_into(self.active.last().copied().unwrap_or(0), instance);
    }

    pub(crate) fn push_into(&mut self, index: usize, instance: Instance) {
        let reach = instance.reach();
        if reach.is_empty() {
            self.culled += 1;
            return;
        }
        let layer = &mut self.layers[index];
        layer.grow(reach);
        layer.instances.push(instance);
    }

    pub(crate) fn depth(&self) -> usize {
        self.active.len()
    }

    // A layer outside the tree, which an effect reads as its mask.
    pub(crate) fn mask_layer(&mut self) -> usize {
        self.layers.push(self.spare.pop().unwrap_or_default());
        self.layers.len() - 1
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
            // The mask hides the layer, so a copy that moves can't show it.
            self.culled += u32::from(!bounds.is_empty());
            return;
        }
        let parent = self.current();
        parent.grow(reach);
        parent.nested.push(Nested {
            at: parent.instances.len() as u32,
            layer: index,
            bounds,
            clip,
            grow: reach,
        });
    }

    // A layer that no parent shows. Pair with pop_offscreen.
    pub(crate) fn push_offscreen(&mut self) {
        self.active.push(self.layers.len());
        self.layers.push(self.spare.pop().unwrap_or_default());
    }

    pub(crate) fn pop_offscreen(&mut self, shot: Option<(u64, Rect<f32>)>) {
        if let (Some(layer), Some((key, bounds))) = (self.active.pop(), shot) {
            self.shots.push(Shot {
                key,
                layer,
                region: crate::geometry::region(bounds),
                read: false,
            });
        }
    }

    // A hash of what a layer draws in `region`, with places relative to it, so a layer that only moved keeps it.
    pub(crate) fn digest(&self, layer: usize, region: Rect<f32>, hasher: &mut impl Hasher) {
        let content = &self.layers[layer];
        let place = |[x, y, w, h]: [f32; 4]| {
            [x - region.origin.x, y - region.origin.y, w, h]
                .map(|value| (value * 16.).round() as i32)
        };
        // A clip counts only inside the region, since an open clip is no place of the layer.
        let clip = |clip: [f32; 4]| place(crate::gpu::rect(clip).intersect(region).to_array());
        for instance in &content.instances {
            let mut rest = *instance;
            (rest.rect, rest.clip, rest.round) = ([0.; 4], [0.; 4], [0.; 4]);
            // A clip without round corners has no rounded box to move.
            let round = (instance.round_radii != [0.; 8]).then(|| place(instance.round));
            (place(instance.rect), clip(instance.clip), round).hash(hasher);
            // The caster of a shadow is a place too.
            if instance.kind() == SHADOW {
                place(mem::take(&mut rest.aux)).hash(hasher);
            }
            bytes(slice::from_ref(&rest)).hash(hasher);
        }
        for nested in &content.nested {
            let inner = &self.layers[nested.layer];
            (
                nested.at,
                place(nested.bounds.to_array()),
                clip(nested.clip.bounds.to_array()),
                (nested.clip.radii != Default::default())
                    .then(|| place(nested.clip.rounded.to_array())),
                nested.clip.radii.to_array().map(f32::to_bits),
                nested.clip.radii.shape,
            )
                .hash(hasher);
            if let Some(effect) = &inner.effect {
                (
                    effect.transform.map(bits),
                    effect.opacity.to_bits(),
                    effect.color_matrix.map(bits),
                    effect.blur.to_bits(),
                    effect.blend_mode as u32,
                )
                    .hash(hasher);
                (
                    effect.shadow.map(|(by, sigma, color)| {
                        bits([by.x, by.y, sigma, color.r, color.g, color.b, color.a])
                    }),
                    effect.clip.map(|(bounds, radii)| {
                        (
                            place(bounds.to_array()),
                            bits(radii.to_array()),
                            radii.shape,
                        )
                    }),
                    effect.mask.as_ref().map(|(tile, bounds)| {
                        (
                            tile.page,
                            bits(tile.rect.map(|value| value as f32).to_array()),
                            place(bounds.to_array()),
                        )
                    }),
                    effect.backdrop.as_ref().map(|backdrop| {
                        (place(backdrop.bounds.to_array()), backdrop.blur.to_bits())
                    }),
                )
                    .hash(hasher);
                if let Some(text) = effect.text_mask {
                    self.digest(text, region, hasher);
                }
            }
            self.digest(nested.layer, region, hasher);
        }
    }

    // A snapshot layer is read only when it draws otherwise than in the last frame: `last` holds the digest and
    // the size of each snapshot layer of the last frame.
    pub(crate) fn read_shots(&mut self, last: &mut FxHashMap<u64, (u64, [i32; 2])>) {
        let mut shots = FxHashMap::default();
        for at in 0..self.shots.len() {
            let (key, layer) = (self.shots[at].key, self.shots[at].layer);
            let [x, y, width, height] = self.shots[at].region;
            let mut hasher = rustc_hash::FxHasher::default();
            let region = crate::rect(x as f32, y as f32, width as f32, height as f32);
            self.digest(layer, region, &mut hasher);
            let now = (hasher.finish(), [width, height]);
            self.shots[at].read = last.get(&key) != Some(&now);
            shots.insert(key, now);
        }
        *last = shots;
    }

    // A layer that the backend could not read is read with the next frame.
    pub(crate) fn keep_unread(
        &mut self,
        read: &[crate::Snapshot],
        last: &mut FxHashMap<u64, (u64, [i32; 2])>,
    ) {
        for shot in &mut self.shots {
            if mem::take(&mut shot.read) && !read.iter().any(|read| read.key == shot.key) {
                last.remove(&shot.key);
            }
        }
    }

    // The window regions to draw again, or `None` for the whole window.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn damage_regions(
        &self,
        window: [u32; 2],
        extra: &[[i32; 4]],
    ) -> Option<Vec<[i32; 4]>> {
        (self.damage).regions(self.reads.iter().map(|read| read.0), window, extra)
    }

    pub(crate) fn keyed(&self, key: u64) -> Option<usize> {
        let &(_, index) = self.keyed.iter().find(|(keyed, _)| *keyed == key)?;
        Some(index)
    }

    // `blurs` are new radii of the filter blur and the backdrop blur.
    pub(crate) fn patch(
        &mut self,
        key: u64,
        opacity: f32,
        transform: Option<[f32; 16]>,
        blurs: [Option<f32>; 2],
    ) -> bool {
        let Some(index) = self.keyed(key) else {
            return false;
        };
        // The reach in window pixels. Layers around it map it to the window as the last diff found them,
        // unless a patch moved them since.
        let reach = |scene: &Self| {
            let (parent, nested) = scene.layers.iter().enumerate().find_map(|(at, layer)| {
                let nested = layer.nested.iter().find(|nested| nested.layer == index)?;
                Some((at, nested))
            })?;
            let reach = nested.reach();
            let mut layer = parent;
            while layer != 0 {
                let effect = scene.layers[layer].effect.as_ref()?;
                if scene.diffed == Some(false)
                    || layer >= scene.maps.len()
                    || scene.moved.contains(&layer)
                    || effect.backdrop.is_some()
                    || effect.blend_mode != crate::BlendMode::Normal
                {
                    return None;
                }
                layer = scene.maps[layer].parent as usize;
            }
            Some(window_reach(&scene.maps, parent as u32, reach))
        };
        let before = reach(self);
        let Some(effect) = self.layers[index].effect.as_mut() else {
            return false;
        };
        if transform.is_some() && effect.transform.is_none()
            || blurs[1].is_some() && effect.backdrop.is_none()
        {
            return false;
        }
        effect.opacity = opacity;
        effect.transform = transform.or(effect.transform);
        effect.blur = blurs[0].unwrap_or(effect.blur);
        if let (Some(blur), Some(backdrop)) = (blurs[1], &mut effect.backdrop) {
            backdrop.blur = blur;
        }
        // A new transform or blur changes the reach. Only the window layer can take a wider one.
        let patched = transform.is_none() && blurs[0].is_none()
            || match effect_bounds(&self.layers[index]) {
                Some(bounds) => self.layers.iter_mut().enumerate().any(|(at, layer)| {
                    layer
                        .nested
                        .iter_mut()
                        .find(|nested| nested.layer == index)
                        .is_some_and(|nested| {
                            let fits =
                                transform.is_none() && nested.bounds.intersect(bounds) == bounds;
                            (at == 0 || fits) && {
                                nested.bounds = bounds;
                                true
                            }
                        })
                }),
                None => true,
            };
        let after = reach(self);
        if !self.moved.contains(&index) {
            self.moved.push(index);
        }
        match (before, after) {
            (Some(before), Some(after)) => {
                let changed = before.union(after);
                self.damage.add(changed);
                self.patched.add(changed);
                if let Some(under) = &mut self.under {
                    under.damage.add(changed);
                }
            }
            _ => {
                (self.damage.whole, self.patched.whole) = (true, true);
                if let Some(under) = &mut self.under {
                    under.damage.whole = true;
                }
            }
        }
        // The layers around it draw otherwise, so their cached pixels can't be used.
        self.salt += 1;
        let mut layer = index;
        while let Some(parent) = (self.layers.iter())
            .position(|parent| parent.nested.iter().any(|nested| nested.layer == layer))
            .filter(|parent| *parent != 0)
        {
            if let Some(digest) = self.digests.get_mut(parent) {
                *digest = mix(*digest, self.salt);
            }
            layer = parent;
        }
        patched
    }

    // Finds what changed since the drawn window, and the digest of each layer. `fresh` holds atlas tiles
    // that got new pixels in a place that another tile used before: (color page, page, texels).
    pub(crate) fn diff(&mut self, fresh: Option<&[crate::atlas::Fresh]>) {
        self.bake();
        let mut owners = mem::take(&mut self.owners);
        let window = &self.layers[0];
        for (owner, len) in owners
            .iter_mut()
            .zip([window.instances.len(), window.nested.len()])
        {
            owner.clear();
            owner.resize(len, u32::MAX);
        }
        // Inner scrolls come later.
        for (at, scroll) in self.scrolls.iter().enumerate() {
            for (owner, [start, end]) in owners.iter_mut().zip([scroll.instances, scroll.nested]) {
                let end = (end as usize).min(owner.len());
                owner[(start as usize).min(end)..end].fill(at as u32);
            }
        }
        let mut walk = Walk {
            layers: &self.layers,
            scrolls: &self.scrolls,
            owners,
            entries: mem::take(&mut self.entries),
            maps: mem::take(&mut self.maps),
            digests: mem::take(&mut self.digests),
            reads: mem::take(&mut self.reads),
        };
        walk.entries.clear();
        walk.reads.clear();
        walk.maps.clear();
        walk.maps.resize(self.layers.len(), Map::default());
        walk.digests.clear();
        walk.digests.resize(self.layers.len(), 0);
        walk.layer(0, 0, None, 0);
        (
            self.entries,
            self.maps,
            self.digests,
            self.reads,
            self.owners,
        ) = (
            walk.entries,
            walk.maps,
            walk.digests,
            walk.reads,
            walk.owners,
        );
        self.diffed = Some(true);
        self.moved.clear();
        let mut damage = mem::take(&mut self.patched);
        damage.whole |= !self.drawn;
        // Scrolls whose content moved since the drawn frame: (scroll, its prior one, how far).
        let mut moved: Vec<(usize, usize, Point<f32>)> = Vec::new();
        if !damage.whole {
            for (at, scroll) in self.scrolls.iter().enumerate() {
                let Some(prior) = self
                    .prior_scrolls
                    .iter()
                    .position(|prior| prior.id == scroll.id)
                else {
                    continue;
                };
                let by = scroll.shift - self.prior_scrolls[prior].shift;
                if by != Point::default() {
                    moved.push((at, prior, by));
                }
            }
        }
        // The backend can copy the last frame moved when one scroll and the scrolls inside it moved together, by whole
        // device pixels, in a viewport that stayed and has square corners.
        // A scroll can have several parts, around content that escapes it.
        let within = |scrolls: &[Scroll], inner: usize, outer: usize| {
            let mut at = inner as u32;
            while at != u32::MAX {
                if scrolls[at as usize].id == scrolls[outer].id {
                    return true;
                }
                at = scrolls[at as usize].parent;
            }
            false
        };
        let copy = moved.first().copied().filter(|&(at, prior, by)| {
            let viewport = self.scrolls[at].viewport;
            let bounds = viewport.bounds;
            let whole = |value: f32| (value - value.round()).abs() < 1e-3;
            moved
                .iter()
                .all(|&(inner, _, other)| other == by && within(&self.scrolls, inner, at))
                && viewport.bounds == self.prior_scrolls[prior].viewport.bounds
                && viewport.radii == Default::default()
                && [
                    bounds.origin.x,
                    bounds.origin.y,
                    bounds.size.width,
                    bounds.size.height,
                    by.x,
                    by.y,
                ]
                .into_iter()
                .all(whole)
                && by.x.abs() < bounds.size.width
                && by.y.abs() < bounds.size.height
        });
        // The scroll whose content the backend can keep below what paints after it, and where the items after it
        // start in this scene and in the drawn one.
        let kept = copy.map(|(at, prior, _)| (at, prior)).or_else(|| {
            let id = self.under_scroll?;
            let at = self.scrolls.iter().position(|scroll| scroll.id == id)?;
            let prior = (self.prior_scrolls.iter()).position(|scroll| scroll.id == id)?;
            Some((at, prior))
        });
        // Where the items over the viewport start: the first item of the window layer after the start of the scroll
        // that isn't its content and reaches into the viewport. The items after it draw over the kept window, also
        // content of the scroll, like its scroll bar.
        let split = kept.and_then(|(at, _)| {
            let scroll = &self.scrolls[at];
            let window = &self.layers[0];
            let top = (self.entries.iter())
                .filter(|entry| {
                    !(entry.scroll != 0 && within(&self.scrolls, entry.scroll as usize - 1, at))
                        && Under::after([scroll.instances[0], scroll.nested[0]], entry.top)
                        && (window_reach(&self.maps, entry.layer, entry.reach))
                            .intersects(scroll.viewport.bounds)
                })
                .map(|entry| entry.top)
                .min_by_key(|&top| match top & NESTED {
                    0 => (top, 1),
                    _ => (window.nested[(top & !NESTED) as usize].at, 0),
                })?;
            Some(match top & NESTED {
                0 => [
                    top,
                    window.nested.partition_point(|nested| nested.at <= top) as u32,
                ],
                _ => [window.nested[(top & !NESTED) as usize].at, top & !NESTED],
            })
        });
        let splits = split.map(|split| (split, self.under_split));
        let after_new = |top| splits.is_some_and(|(new, _)| Under::after(new, top));
        // Without the split of the drawn scene, all of the kept window draws again.
        let after_old =
            |top| splits.is_some_and(|(_, old)| old.is_some_and(|old| Under::after(old, top)));
        let mut under = damage.clone();
        if copy.is_none() {
            for &(at, prior, _) in &moved {
                for damage in [&mut damage, &mut under] {
                    damage.add(self.scrolls[at].viewport.bounds);
                    damage.add(self.prior_scrolls[prior].viewport.bounds);
                }
            }
        }
        // Content of the copied scroll moved by `-by` in the window. Everything else that the viewport shows moved with
        // it too, so it draws again where it is and where the copy put it.
        let (viewport, by) = copy.map_or((Rect::default(), Point::default()), |(at, _, by)| {
            (self.scrolls[at].viewport.bounds, by)
        });
        let inside_new: Vec<bool> = (0..self.scrolls.len())
            .map(|at| copy.is_some_and(|(outer, ..)| within(&self.scrolls, at, outer)))
            .collect();
        let inside_old: Vec<bool> = (0..self.prior_scrolls.len())
            .map(|at| copy.is_some_and(|(_, outer, _)| within(&self.prior_scrolls, at, outer)))
            .collect();
        let copied = |damage: &mut Damage, reach: Rect<f32>| {
            damage.add((reach - by).intersect(viewport));
        };
        if copy.is_some() {
            let patched = damage.rects.clone();
            for rect in patched {
                copied(&mut damage, rect);
                copied(&mut under, rect);
            }
            // The edges that the copy has no pixels for.
            let [x, y, width, height] = [
                viewport.origin.x,
                viewport.origin.y,
                viewport.size.width,
                viewport.size.height,
            ];
            let edges = [
                (by.y > 0.).then(|| Rect::new(point(x, y + height - by.y), Size::new(width, by.y))),
                (by.y < 0.).then(|| Rect::new(point(x, y), Size::new(width, -by.y))),
                (by.x > 0.).then(|| Rect::new(point(x + width - by.x, y), Size::new(by.x, height))),
                (by.x < 0.).then(|| Rect::new(point(x, y), Size::new(-by.x, height))),
            ];
            for edge in edges.into_iter().flatten() {
                damage.add(edge);
                under.add(edge);
            }
        }
        let added = |damage: &mut Damage,
                     under: &mut Damage,
                     entries: &[Entry],
                     maps: &[Map],
                     at: usize| {
            let entry = &entries[at];
            let reach = window_reach(maps, entry.layer, entry.reach);
            damage.add(reach);
            if !after_new(entry.top) {
                under.add(reach);
            }
        };
        let removed = |damage: &mut Damage,
                       under: &mut Damage,
                       entries: &[Entry],
                       maps: &[Map],
                       at: usize| {
            let entry = &entries[at];
            let reach = window_reach(maps, entry.layer, entry.reach);
            let inside = entry.scroll != 0 && inside_old[entry.scroll as usize - 1];
            let after = after_old(entry.top);
            for damage in [Some(&mut *damage), (!after).then_some(&mut *under)]
                .into_iter()
                .flatten()
            {
                if !inside {
                    damage.add(reach);
                }
                if copy.is_some() {
                    copied(damage, reach);
                }
            }
        };
        let (old, new) = (&self.prior, &self.entries);
        let mut index: Option<(FxHashMap<u64, u32>, Vec<u32>)> = None;
        let mut next = 0;
        for at in 0..new.len() {
            if damage.whole {
                break;
            }
            let hash = new[at].hash;
            if old.get(next).is_some_and(|entry| entry.hash == hash) {
                next += 1;
                continue;
            }
            // A replaced entry, like a canvas with a new tile, keeps the old ones after it, so it needs no index.
            if index.is_none()
                && next < old.len()
                && (new.get(at + 1).map(|entry| entry.hash))
                    == old.get(next + 1).map(|entry| entry.hash)
            {
                added(&mut damage, &mut under, new, &self.maps, at);
                removed(&mut damage, &mut under, old, &self.prior_maps, next);
                next += 1;
                continue;
            }
            // Later old entries by hash, each with the next one of the same hash.
            let (first, chain) = index.get_or_insert_with(|| {
                let mut first = FxHashMap::default();
                let mut chain = vec![u32::MAX; old.len()];
                for (at, entry) in old.iter().enumerate().rev() {
                    if let Some(later) = first.insert(entry.hash, at as u32) {
                        chain[at] = later;
                    }
                }
                (first, chain)
            });
            let mut found = first.get(&hash).copied().unwrap_or(u32::MAX);
            for _ in 0..8 {
                if found == u32::MAX || found as usize >= next {
                    break;
                }
                found = chain[found as usize];
            }
            // An inserted entry keeps the old ones after it.
            let inserted = new
                .get(at + 1)
                .zip(old.get(next))
                .is_some_and(|(new, old)| new.hash == old.hash);
            if found == u32::MAX || (found as usize) < next || inserted {
                added(&mut damage, &mut under, new, &self.maps, at);
                continue;
            }
            for skipped in next..found as usize {
                removed(&mut damage, &mut under, old, &self.prior_maps, skipped);
            }
            next = found as usize + 1;
        }
        for skipped in next..old.len() {
            if damage.whole {
                break;
            }
            removed(&mut damage, &mut under, old, &self.prior_maps, skipped);
        }
        // What the viewport shows outside the copied scroll moved with the copy, unless it is one color all over.
        if copy.is_some() {
            for entry in &self.entries {
                if damage.whole {
                    break;
                }
                if entry.scroll != 0 && inside_new[entry.scroll as usize - 1] {
                    continue;
                }
                let reach = window_reach(&self.maps, entry.layer, entry.reach);
                if !reach.intersects(viewport) {
                    continue;
                }
                if entry.layer == 0 && entry.item & NESTED == 0 {
                    let instance = &self.layers[0].instances[entry.item as usize];
                    let plain = instance.kind() == crate::gpu::BOX
                        && instance.radii == [0.; 8]
                        && instance.round_radii == [0.; 8]
                        && (instance.aux == [0.; 4] || instance.border_color[3] == 0.);
                    if plain && reach.intersect(viewport) == viewport {
                        continue;
                    }
                }
                damage.add(reach.intersect(viewport));
                copied(&mut damage, reach);
                if !after_new(entry.top) {
                    under.add(reach.intersect(viewport));
                    copied(&mut under, reach);
                }
            }
        }
        // A scroll without anything over its viewport needs no kept window below.
        self.under_scroll = kept
            .filter(|_| split.is_some())
            .map(|(at, _)| self.scrolls[at].id);
        self.shift = (copy.is_some() && !damage.whole).then(|| {
            let region = [
                viewport.origin.x,
                viewport.origin.y,
                viewport.size.width,
                viewport.size.height,
            ];
            (
                region.map(|value| value.round() as i32),
                [by.x.round() as i32, by.y.round() as i32],
            )
        });
        // The same instance can read a tile with new pixels. Its layers can't keep their pixels either.
        self.stale.clear();
        let Some(fresh) = fresh else {
            damage.whole = true;
            self.stale.extend_from_slice(&self.digests);
            self.damage = damage;
            self.shift = None;
            self.under = None;
            return;
        };
        if !fresh.is_empty() {
            for entry in &self.entries {
                let content = &self.layers[entry.layer as usize];
                let read = match entry.item & NESTED {
                    0 => {
                        let instance = &content.instances[entry.item as usize];
                        (instance.page())
                            .map(|(color, page)| (color, page, crate::gpu::rect(instance.aux)))
                    }
                    _ => (self.layers[(entry.item & !NESTED) as usize].effect.as_ref())
                        .and_then(|effect| effect.mask.as_ref())
                        .map(|(tile, _)| (true, tile.page, tile.rect.map(|value| value as f32))),
                };
                let Some((color, page, texels)) = read else {
                    continue;
                };
                if !fresh.iter().any(|&(kind, at, tile)| {
                    kind == color && at == page && texels.intersects(tile.map(|value| value as f32))
                }) {
                    continue;
                }
                let reach = window_reach(&self.maps, entry.layer, entry.reach);
                damage.add(reach);
                if !after_new(entry.top) {
                    under.add(reach);
                }
                let mut layer = entry.layer;
                while layer != 0 {
                    self.stale.push(self.digests[layer as usize]);
                    layer = self.maps[layer as usize].parent;
                }
            }
        }
        if damage.whole {
            self.shift = None;
        }
        under.whole |= damage.whole || splits.is_some_and(|(_, old)| old.is_none());
        self.under_split = split;
        self.under = (self.under_scroll.zip(split)).map(|(scroll, split)| Under {
            scroll,
            split,
            damage: under,
        });
        self.damage = damage;
    }

    // The backend drew the scene as it is.
    pub(crate) fn drawn(&mut self) {
        self.damage.clear();
        // Frames drawn without a new diff, like patch frames, must not copy the scroll again.
        self.shift = None;
        if let Some(under) = &mut self.under {
            under.damage.clear();
        }
        // A new scene without a diff leaves the prior entries behind.
        self.drawn = self.diffed != Some(false);
        if self.diffed.take() == Some(true) {
            mem::swap(&mut self.prior, &mut self.entries);
            self.prior_maps.clone_from(&self.maps);
            self.prior_scrolls.clone_from(&self.scrolls);
        }
    }

    // The glass layers that Core Animation can draw: plain glass in the window layer that nothing paints over.
    // Later glass draws above earlier glass, so it comes first.
    #[cfg(target_os = "macos")]
    pub(crate) fn native_glass(&self, mut take: impl FnMut(usize, Rect<f32>, &GlassState)) {
        let mut taken = Vec::new();
        for nested in self.layers[0].nested.iter().rev() {
            let index = nested.layer;
            let layer = &self.layers[index];
            let Some(effect) = &layer.effect else {
                continue;
            };
            let (Some(config), Some(bounds)) = (&effect.glass, layer.bounds) else {
                continue;
            };
            let mask = &effect.content_mask;
            if effect.transform.is_some()
                || effect.opacity != 1.
                || effect.color_matrix.is_some()
                || effect.blur != 0.
                || effect.shadow.is_some()
                || effect.blend_mode != BlendMode::Normal
                || effect.clip.is_some()
                || effect.mask.is_some()
                || effect.backdrop.is_some()
                || bounds != config.bounds
                || bounds.intersect(mask.bounds) != bounds
                || mask.radii != Default::default()
                || self.paints_over(index, bounds, |layer| taken.contains(&layer))
                || bounds.size.width > 4096.
                || bounds.size.height > 4096.
            {
                continue;
            }
            let region = region(bounds);
            if region[2] <= 0 || region[3] <= 0 {
                continue;
            }
            taken.push(index);
            take(index, bounds, config);
        }
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

struct Walk<'a> {
    layers: &'a [SceneLayer],
    scrolls: &'a [Scroll],
    // The innermost scroll of each instance and nested layer of the window layer, or u32::MAX.
    owners: [Vec<u32>; 2],
    entries: Vec<Entry>,
    maps: Vec<Map>,
    digests: Vec<u64>,
    reads: Vec<(Rect<f32>, u32)>,
}

impl Walk<'_> {
    // Lists the entries of a layer and returns its digest. `context` stands for the layers around it, and `scroll`
    // holds a nested layer in a scroll.
    fn layer(&mut self, layer: usize, context: u64, scroll: Option<usize>, top: u32) -> u64 {
        let content = &self.layers[layer];
        let origin = match layer {
            0 => Point::default(),
            _ => origin(content),
        };
        let shift = scroll.map_or_else(Point::default, |at| self.scrolls[at].shift);
        let context = mix(context, place(origin + shift));
        let mut digest = 1;
        let mut nested = content.nested.iter().enumerate().peekable();
        for (at, instance) in content.instances.iter().enumerate() {
            while let Some((index, inner)) = nested.next_if(|(_, inner)| inner.at as usize <= at) {
                let scroll = self.find(layer, index, true).or(scroll);
                let top = if layer == 0 {
                    index as u32 | NESTED
                } else {
                    top
                };
                digest = mix(
                    digest,
                    self.nested(layer, origin, inner, context, scroll, top),
                );
            }
            // A scroll viewport cut it away, so it draws nothing.
            if instance.reach().is_empty() {
                continue;
            }
            let scroll = self.find(layer, at, false).or(scroll);
            // The window layer has no digest, so its entries in a scroll hash only their place in the content.
            let hash = match (layer, scroll) {
                (0, Some(_)) => 0,
                _ => instance_hash(instance, origin, None),
            };
            digest = mix(digest, hash);
            let entry = match scroll {
                Some(at) => {
                    let scroll = &self.scrolls[at];
                    let base = if layer == 0 {
                        origin - scroll.shift
                    } else {
                        origin
                    };
                    mix(
                        instance_hash(instance, base, Some(&scroll.viewport)),
                        scroll.id,
                    )
                }
                None => hash,
            };
            self.entries.push(Entry {
                hash: mix(entry, context),
                reach: instance.reach(),
                layer: layer as u32,
                item: at as u32,
                scroll: scroll.map_or(0, |at| at as u32 + 1),
                top: if layer == 0 { at as u32 } else { top },
            });
        }
        for (index, inner) in nested {
            let scroll = self.find(layer, index, true).or(scroll);
            let top = if layer == 0 {
                index as u32 | NESTED
            } else {
                top
            };
            digest = mix(
                digest,
                self.nested(layer, origin, inner, context, scroll, top),
            );
        }
        self.digests[layer] = digest;
        digest
    }

    // The innermost scroll that holds an instance or a nested layer of the window layer. Inner scrolls come later.
    fn find(&self, layer: usize, at: usize, nested: bool) -> Option<usize> {
        if layer != 0 {
            return None;
        }
        let owner = self.owners[usize::from(nested)][at];
        (owner != u32::MAX).then_some(owner as usize)
    }

    // Places are relative to `base`, the origin of the parent.
    fn nested(
        &mut self,
        parent: usize,
        base: Point<f32>,
        nested: &Nested,
        context: u64,
        scroll: Option<usize>,
        top: u32,
    ) -> u64 {
        let layers = self.layers;
        let Some(effect) = &layers[nested.layer].effect else {
            return 0;
        };
        // In a scroll, the entry hashes its place in the content, with the mask around the content as its own place.
        let entry = scroll.map(|at| {
            let scroll = &self.scrolls[at];
            let shift = scroll.shift;
            let base = if parent == 0 { base - shift } else { base };
            let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
            let rect = |rect: Rect<f32>| {
                bits(&[
                    rect.origin.x - base.x,
                    rect.origin.y - base.y,
                    rect.size.width,
                    rect.size.height,
                ])
            };
            let spreads = effect.blur != 0. || effect.shadow.is_some() || effect.backdrop.is_some();
            let masked = nested.clip.bounds == scroll.viewport.bounds && !spreads;
            let mut hasher = rustc_hash::FxHasher::default();
            (
                effect.transform.map(|matrix| {
                    let mut m = matrix;
                    let [x, y] = [shift.x, shift.y];
                    m[12] += x - (m[0] * x + m[4] * y);
                    m[13] += y - (m[1] * x + m[5] * y);
                    m[14] -= m[2] * x + m[6] * y;
                    bits(&m)
                }),
                effect.opacity.to_bits(),
                effect.color_matrix.map(|matrix| bits(&matrix)),
                effect.blur.to_bits(),
                effect.blend_mode as u32,
                effect.shadow.map(|(by, sigma, color)| {
                    bits(&[by.x, by.y, sigma, color.r, color.g, color.b, color.a])
                }),
                effect
                    .clip
                    .map(|(bounds, radii)| (rect(bounds), bits(&radii.to_array()), radii.shape)),
            )
                .hash(&mut hasher);
            (
                effect
                    .mask
                    .as_ref()
                    .map(|(tile, bounds)| (tile.page, tile.id, rect(*bounds))),
                effect
                    .backdrop
                    .as_ref()
                    .map(|backdrop| (rect(backdrop.bounds), backdrop.blur.to_bits())),
                effect
                    .glass
                    .as_ref()
                    .map(|glass| (glass.key, rect(glass.bounds))),
                match masked {
                    true => rect(nested.bounds),
                    false => rect(nested.clip.bounds.intersect(nested.bounds)),
                },
                (nested.clip.radii != Default::default()).then(|| {
                    match nested.clip.rounded == scroll.viewport.rounded {
                        true => Vec::new(),
                        false => rect(nested.clip.rounded),
                    }
                }),
                bits(&nested.clip.radii.to_array()),
                scroll.id,
            )
                .hash(&mut hasher);
            if let Some(text) = effect.text_mask {
                for instance in &layers[text].instances {
                    instance_hash(instance, base, Some(&scroll.viewport)).hash(&mut hasher);
                }
            }
            hasher.finish()
        });
        let mut hasher = rustc_hash::FxHasher::default();
        let rect = |rect: Rect<f32>| {
            bits([
                rect.origin.x - base.x,
                rect.origin.y - base.y,
                rect.size.width,
                rect.size.height,
            ])
        };
        (
            effect.transform.map(bits),
            effect.opacity.to_bits(),
            effect.color_matrix.map(bits),
            effect.blur.to_bits(),
            effect.blend_mode as u32,
            effect.shadow.map(|(by, sigma, color)| {
                bits([by.x, by.y, sigma, color.r, color.g, color.b, color.a])
            }),
            effect
                .clip
                .map(|(bounds, radii)| (rect(bounds), bits(radii.to_array()), radii.shape)),
        )
            .hash(&mut hasher);
        (
            effect.mask.as_ref().map(|(tile, bounds)| {
                (
                    tile.page,
                    tile.rect
                        .map(|value| value as f32)
                        .to_array()
                        .map(f32::to_bits),
                    tile.id,
                    rect(*bounds),
                )
            }),
            effect.backdrop.as_ref().map(|backdrop| {
                (
                    rect(backdrop.bounds),
                    bits(backdrop.corner_radii.to_array()),
                    backdrop.corner_radii.shape,
                    backdrop.blur.to_bits(),
                    backdrop.color_matrix.map(bits),
                )
            }),
            effect.glass.as_ref().map(|glass| {
                (
                    glass.key,
                    rect(glass.bounds),
                    glass.corner_radius.to_bits(),
                    glass
                        .tint
                        .map(|tint| bits([tint.r, tint.g, tint.b, tint.a])),
                    glass.clear,
                    glass.interactive,
                )
            }),
            // A clip counts only where the layer reaches.
            rect(nested.clip.bounds.intersect(nested.bounds)),
            (nested.clip.radii != Default::default()).then(|| rect(nested.clip.rounded)),
            bits(nested.clip.radii.to_array()),
            nested.clip.radii.shape,
        )
            .hash(&mut hasher);
        if let Some(text) = effect.text_mask {
            for instance in &layers[text].instances {
                instance_hash(instance, base, None).hash(&mut hasher);
            }
        }
        let own = hasher.finish();
        // The inner layer's entries hash only what doesn't change in a scroll, and its digest doesn't hold them.
        let inner_context = entry.unwrap_or(own);
        self.entries.push(Entry {
            hash: mix(inner_context, context),
            reach: nested.reach(),
            layer: parent as u32,
            item: nested.layer as u32 | NESTED,
            scroll: scroll.map_or(0, |at| at as u32 + 1),
            top,
        });
        self.maps[nested.layer] = Map {
            parent: parent as u32,
            clip: nested.clip.bounds,
            spread: effect.blur * 3.,
            shadow: effect.shadow.map(|(offset, sigma, _)| (offset, sigma)),
            transform: effect.transform,
        };
        // A blend mode or a backdrop color change reads only the pixel below, which the region draws too. A
        // backdrop blur reads around it.
        if effect
            .backdrop
            .as_ref()
            .is_some_and(|backdrop| backdrop.blur > 0.25)
        {
            let reach = window_reach(&self.maps, parent as u32, nested.reach());
            self.reads.push((reach, top));
        }
        let inner = origin(&layers[nested.layer]);
        let offset = place(point(inner.x - base.x, inner.y - base.y));
        mix(
            mix(own, offset),
            self.layer(nested.layer, mix(inner_context, context), scroll, top),
        )
    }
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
    // Shows the content only through the text that `Window::with_text_clip` captures around it.
    pub text_mask: bool,
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
            text_mask: false,
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
#[derive(Clone)]
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

#[derive(Clone)]
pub(crate) struct BackdropState {
    pub bounds: Rect<f32>,
    pub corner_radii: Radii<f32>,
    pub blur: f32,
    pub color_matrix: Option<[f32; 20]>,
}

#[derive(Clone)]
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
    // The mask layer of `PaintEffect::text_mask`.
    pub text_mask: Option<usize>,
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
    display_p3: bool,
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
            display_p3: false,
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
    /// The pixels are Display P3, not sRGB.
    pub fn display_p3(self) -> Self {
        Self {
            display_p3: true,
            ..self
        }
    }
    pub fn is_display_p3(&self) -> bool {
        self.display_p3
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
        text_mask: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{BOX, rect as bounds};

    #[test]
    fn copied_spans_draw_like_their_paint() {
        let mask = Clip::from(bounds([0., 0., 200., 200.]));
        let at = |x: f32| Instance::new(BOX, bounds([x, x, 10., 10.]), mask);
        // A span with a box, a layer that holds a nested layer, and a box after them.
        let paint = |scene: &mut Scene, x: f32| {
            scene.push(at(x));
            scene.push_effect(effect(mask), None);
            scene.push(at(x + 1.));
            scene.push_effect(effect(mask), None);
            scene.push(at(x + 2.));
            scene.pop_effect();
            scene.pop_effect();
            scene.push(at(x + 3.));
        };
        let mut scene = Scene::default();
        scene.begin_paint(1, |_| false);
        scene.push(at(90.));
        let mark = scene.mark();
        paint(&mut scene, 20.);
        let span = scene.span(mark).unwrap();
        let painted = scene.records(&span, false, None);
        scene.clear();
        scene.begin_paint(2, |_| false);
        // The copy lands after other content, in a nested layer.
        scene.push(at(50.));
        scene.push_effect(effect(mask), None);
        scene.push(at(60.));
        let copied = scene.replay(&span).unwrap();
        assert_eq!(scene.records(&copied, false, None), painted);
        assert_eq!(scene.records(&span, true, None), painted);
        scene.pop_effect();
        assert_eq!(scene.layers.len(), 4);
        assert_eq!(scene.layers[1].bounds, Some(bounds([20., 20., 50., 50.])));
        assert_eq!(scene.layers[0].nested[0].layer, 1);
    }

    #[test]
    fn layers_that_the_mask_hides_count_as_cut() {
        let mask = Clip::from(bounds([0., 0., 100., 100.]));
        let open = Clip::from(bounds([-1e6, -1e6, 2e6, 2e6]));
        let mut scene = Scene::default();
        scene.begin_paint(1, |_| false);
        for (x, culled) in [(10., 0), (300., 1)] {
            scene.push_effect(effect(mask), None);
            scene.push(Instance::new(BOX, bounds([x, x, 10., 10.]), open));
            scene.pop_effect();
            assert_eq!(scene.culled, culled);
        }
        assert_eq!(scene.layers[0].nested.len(), 1);
    }

    #[test]
    fn copied_spans_keep_their_scrolls_once() {
        let mask = Clip::from(bounds([0., 0., 200., 200.]));
        let at = |y: f32| Instance::new(BOX, bounds([0., y, 10., 10.]), mask);
        let mut spans: Vec<Span> = Vec::new();
        let mut scene = Scene::default();
        for pass in 1..6 {
            scene.begin_paint(pass, |_| false);
            let last = mem::take(&mut spans);
            // A scroll with content, an empty scroll, and a box next to them, each in its own span.
            let paint = |scene: &mut Scene, index: usize, y: f32| match index {
                0 => {
                    scene.begin_scroll(1, mask, point(0., 30.), false);
                    scene.push(at(y));
                    scene.end_scroll();
                }
                1 => {
                    scene.begin_scroll(2, mask, point(0., 0.), false);
                    scene.end_scroll();
                }
                _ => scene.push(at(y + 40.)),
            };
            for index in 0..3 {
                let mark = scene.mark();
                let span = match last.get(index) {
                    Some(span) => scene.replay(span).unwrap(),
                    None => {
                        paint(&mut scene, index, 0.);
                        scene.span(mark).unwrap()
                    }
                };
                spans.push(span);
            }
            assert_eq!(scene.scrolls.len(), 2, "pass {pass}");
            assert_eq!(scene.scrolls[0].shift, point(0., 30.));
            assert_eq!(scene.scrolls[0].instances, [0, 1]);
            scene.diff(Some(&[]));
            scene.drawn();
            scene.clear();
        }
    }

    #[test]
    fn nested_escapes_close_their_cuts() {
        // A fixed box in an absolute box: both escape the scroll, one inside the other.
        let viewport = Clip::from(bounds([0., 0., 100., 100.]));
        let open = Clip::from(bounds([0., -100., 100., 300.]));
        let at = |y: f32| Instance::new(BOX, bounds([0., y, 10., 10.]), open);
        let mut scene = Scene::default();
        scene.begin_paint(1, |_| false);
        scene.begin_scroll(1, viewport, point(0., 0.), true);
        scene.push(at(150.));
        scene.pause_cuts(0);
        scene.push(at(160.));
        scene.pause_cuts(0);
        scene.push(at(170.));
        scene.resume_cuts();
        scene.push(at(180.));
        scene.resume_cuts();
        scene.push(at(190.));
        scene.end_scroll();
        assert!(scene.cuts.iter().all(|cut| cut.instances[1] != u32::MAX));
        scene.bake();
        let heights: Vec<f32> = (scene.layers[0].instances.iter())
            .map(|instance| instance.clip[3])
            .collect();
        assert_eq!(heights, [100., 300., 300., 300., 100.]);
    }

    #[test]
    fn layers_take_back_their_own_buffers() {
        let mask = Clip::from(bounds([0., 0., 200., 200.]));
        let at = Instance::new(BOX, bounds([0., 0., 10., 10.]), mask);
        let mut scene = Scene::default();
        let mut capacities = Vec::new();
        for pass in 1..5 {
            scene.begin_paint(pass, |_| false);
            for _ in 0..1000 {
                scene.push(at);
            }
            for _ in 0..3 {
                scene.push_effect(effect(mask), None);
                scene.push(at);
                scene.pop_effect();
            }
            capacities.push(
                scene
                    .layers
                    .iter()
                    .map(|layer| layer.instances.capacity())
                    .collect::<Vec<_>>(),
            );
            scene.clear();
        }
        // After the first two passes, the root gets its large buffer back and the small layers stay small.
        assert_eq!(capacities[2], capacities[3]);
        assert!(capacities[3][1..].iter().all(|&capacity| capacity < 1000));
    }

    #[test]
    fn moved_spans_draw_like_their_paint_at_the_new_place() {
        let (mask, cut) = (
            Clip::from(bounds([0., 0., 200., 200.])),
            Clip::from(bounds([0., 50., 200., 100.])),
        );
        let paint = |scene: &mut Scene, y: f32, mask: Clip<f32>| {
            scene.push(Instance::new(BOX, bounds([10., y, 10., 10.]), mask));
            scene.push(Instance::new(BOX, bounds([30., y + 20., 10., 10.]), mask));
        };
        let mut scene = Scene::default();
        scene.begin_paint(1, |_| false);
        let mark = scene.mark();
        paint(&mut scene, 60., mask);
        let span = scene.span(mark).unwrap();
        let area = scene.plain(&span, &mask).unwrap();
        assert_eq!(area, bounds([10., 60., 30., 30.]));
        // A box that the clip cuts can't move.
        assert_eq!(scene.plain(&span, &cut), None);
        scene.clear();
        scene.begin_paint(2, |_| false);
        let moved = scene
            .replay_moved(&span, point(0., -8.), &mask, &cut)
            .unwrap();
        let mut fresh = Scene::default();
        fresh.begin_paint(3, |_| false);
        let mark = fresh.mark();
        paint(&mut fresh, 52., cut);
        let painted = fresh.records(&fresh.span(mark).unwrap(), false, None);
        assert_eq!(scene.records(&moved, false, None), painted);
        assert_eq!(
            scene.records(&span, true, Some((point(0., -8.), mask, cut))),
            painted
        );
        assert_eq!(scene.layers[0].bounds, Some(bounds([10., 52., 30., 30.])));
    }

    #[test]
    fn moved_spans_carry_their_layers_and_inner_clips() {
        let mask = Clip::from(bounds([0., 0., 200., 200.]));
        let open = Clip::from(bounds([-1e6, -1e6, 2e6, 2e6]));
        // A box clipped by its own overflow, and a layer turned a quarter about its center with a clip inside.
        let paint = |scene: &mut Scene, y: f32| {
            let inner = Clip::from(bounds([10., y, 20., 20.]));
            scene.push(Instance::new(BOX, bounds([10., y, 40., 40.]), inner));
            let [cx, cy] = [60., y + 10.];
            let mut turned = effect(mask);
            turned.transform = Some([
                0.,
                1.,
                0.,
                0.,
                -1.,
                0.,
                0.,
                0.,
                0.,
                0.,
                1.,
                0.,
                cx + cy,
                cy - cx,
                0.,
                1.,
            ]);
            scene.push_effect(turned, None);
            scene.push(Instance::new(BOX, bounds([50., y, 20., 20.]), open));
            let clip = Clip::from(bounds([52., y + 2., 10., 10.]));
            scene.push(Instance::new(BOX, bounds([50., y, 20., 20.]), clip));
            scene.pop_effect();
        };
        let mut scene = Scene::default();
        scene.begin_paint(1, |_| false);
        let mark = scene.mark();
        paint(&mut scene, 60.);
        let span = scene.span(mark).unwrap();
        assert_eq!(
            scene.plain(&span, &mask),
            Some(bounds([10., 60., 60., 20.]))
        );
        // A clip that the mask cuts doesn't say where the box clips, so the span can't move.
        let cut = Clip::from(bounds([0., 65., 200., 100.]));
        assert_eq!(scene.plain(&span, &cut), None);
        scene.clear();
        scene.begin_paint(2, |_| false);
        let moved = scene
            .replay_moved(&span, point(0., -8.), &mask, &mask)
            .unwrap();
        let mut fresh = Scene::default();
        fresh.begin_paint(3, |_| false);
        let mark = fresh.mark();
        paint(&mut fresh, 52.);
        let painted = fresh.records(&fresh.span(mark).unwrap(), false, None);
        assert_eq!(scene.records(&moved, false, None), painted);
        assert_eq!(
            scene.records(&span, true, Some((point(0., -8.), mask, mask))),
            painted
        );
    }

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
    fn digests_follow_what_a_layer_draws_and_not_where() {
        let digest = |at: f32, red: f32| {
            // Like the window, the layer has an open clip.
            let mask = Clip::from(bounds([-1e6, -1e6, 2e6, 2e6]));
            let mut scene: Scene = Default::default();
            scene.push_offscreen();
            let mut instance = Instance::new(BOX, bounds([at + 5., 5., 10., 10.]), mask);
            instance.color = [red, 0., 0., 1.];
            scene.push(instance);
            scene.push_effect(effect(mask), None);
            scene.push(Instance::new(BOX, bounds([at + 6., 6., 2., 2.]), mask));
            scene.pop_effect();
            let region = bounds([at, 0., 100., 100.]);
            scene.pop_offscreen(Some((1, region)));
            assert!(scene.layers[0].nested.is_empty(), "the window shows it");
            let mut hasher = rustc_hash::FxHasher::default();
            scene.digest(scene.shots[0].layer, region, &mut hasher);
            hasher.finish()
        };
        assert_eq!(digest(0., 1.), digest(40., 1.));
        assert_ne!(digest(0., 1.), digest(0., 0.5));
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

    #[test]
    fn patches_in_moved_layers_damage_only_their_place() {
        let mask = Clip::from(bounds([0., 0., 800., 600.]));
        let fill = |scene: &mut Scene, opacity: f32, shadow: bool| {
            scene.clear();
            scene.push(Instance::new(BOX, bounds([0., 0., 800., 600.]), mask));
            let mut matrix = crate::geometry::IDENTITY;
            [matrix[12], matrix[13]] = [300., 200.];
            scene.push_effect(
                EffectState {
                    transform: Some(matrix),
                    shadow: shadow.then(|| {
                        let color = crate::Color {
                            r: 0.,
                            g: 0.,
                            b: 0.,
                            a: 0.5,
                        };
                        (point(0., 4.), 6., color)
                    }),
                    ..effect(mask)
                },
                None,
            );
            scene.push_effect(
                EffectState {
                    opacity,
                    ..effect(mask)
                },
                Some(7),
            );
            scene.push(Instance::new(BOX, bounds([20., 20., 40., 40.]), mask));
            scene.pop_effect();
            scene.pop_effect();
        };
        for shadow in [false, true] {
            // What a new scene damages.
            let mut fresh = Scene::default();
            fill(&mut fresh, 0.9, shadow);
            fresh.diff(Some(&[]));
            fresh.drawn();
            fill(&mut fresh, 0.5, shadow);
            fresh.diff(Some(&[]));
            let wanted = fresh.damage.clone();
            assert!(!wanted.whole && !wanted.rects.is_empty());

            let mut patched = Scene::default();
            fill(&mut patched, 0.9, shadow);
            patched.diff(Some(&[]));
            patched.drawn();
            assert!(patched.patch(7, 0.5, None, [None; 2]));
            assert!(!patched.damage.whole, "shadow {shadow}");
            for rect in &wanted.rects {
                assert!(
                    patched
                        .damage
                        .rects
                        .iter()
                        .any(|got| got.intersect(*rect) == *rect),
                    "shadow {shadow}: {rect:?} in {:?}",
                    patched.damage.rects
                );
            }
            let area: f32 = (patched.damage.rects.iter())
                .map(|rect| rect.size.width * rect.size.height)
                .sum();
            assert!(area < 100. * 100., "shadow {shadow}: {area}");
            // A second patch of a layer whose place a patch changed can't use the maps of the diff.
            assert!(patched.patch(7, 0.4, None, [None; 2]));
            assert!(!patched.damage.whole);
        }
    }

    #[test]
    fn rebuilt_scenes_damage_what_changed() {
        use crate::gpu::IMAGE;
        let mask = Clip::from(bounds([0., 0., 400., 300.]));
        let quad = |at: [f32; 4], red: f32| {
            let mut instance = Instance::new(BOX, bounds(at), mask);
            instance.color = [red, 0., 0., 1.];
            instance
        };
        // Each scene is a list of boxes, with `layer` boxes inside an opacity layer turned by `turn`.
        let scene = |boxes: &[([f32; 4], f32)], layer: &[([f32; 4], f32)], turn: f32| {
            let mut scene = Scene::default();
            for &(at, red) in boxes {
                scene.push(quad(at, red));
            }
            let mut matrix = crate::geometry::IDENTITY;
            [matrix[0], matrix[5], matrix[12]] = [turn, turn, 100.];
            scene.push_effect(
                EffectState {
                    opacity: 0.5,
                    transform: (turn != 1.).then_some(matrix),
                    ..effect(mask)
                },
                None,
            );
            for &(at, red) in layer {
                scene.push(quad(at, red));
            }
            scene.pop_effect();
            let mut image = Instance::new(IMAGE, bounds([300., 200., 8., 8.]), mask);
            image.aux = [16., 16., 8., 8.];
            image.info[3] = 2.;
            scene.push(image);
            scene
        };
        let damage = |from: &mut Scene, to: Scene, fresh: &[crate::atlas::Fresh]| {
            from.diff(Some(&[]));
            from.drawn();
            let mut to = to;
            (to.prior, to.prior_maps, to.drawn) =
                (mem::take(&mut from.prior), from.prior_maps.clone(), true);
            to.diff(Some(fresh));
            to.damage
                .rects
                .iter()
                .map(|rect| rect.to_array())
                .collect::<Vec<_>>()
        };
        let a = ([10., 10., 20., 20.], 1.);
        let b = ([100., 10., 20., 20.], 1.);
        let c = ([200., 10., 20., 20.], 1.);
        let inner = ([0., 100., 10., 10.], 1.);
        let mut first = scene(&[a, b, c], &[inner], 1.);
        assert_eq!(
            damage(&mut first, scene(&[a, b, c], &[inner], 1.), &[]),
            [] as [[f32; 4]; 0]
        );
        let new = ([150., 150., 10., 10.], 1.);
        assert_eq!(
            damage(&mut first, scene(&[a, new, b, c], &[inner], 1.), &[]),
            [[150., 150., 10., 10.]],
            "an inserted box"
        );
        assert_eq!(
            damage(&mut first, scene(&[a, c], &[inner], 1.), &[]),
            [[100., 10., 20., 20.]],
            "a removed box"
        );
        let over = ([105., 15., 20., 20.], 0.5);
        let mut swapped = scene(&[a, b, over, c], &[inner], 1.);
        assert_eq!(
            damage(&mut swapped, scene(&[a, over, b, c], &[inner], 1.), &[]),
            [[105., 15., 20., 20.]],
            "boxes that change their order draw again where one covers the other"
        );
        assert_eq!(
            damage(&mut first, scene(&[a, b], &[inner, c], 1.), &[]),
            // The layer reaches higher, so its origin and all its boxes move.
            [[0., 10., 220., 100.]],
            "a box that moves into a layer"
        );
        let mut turned = scene(&[a], &[inner], 2.);
        assert_eq!(
            damage(&mut turned, scene(&[a], &[(inner.0, 0.5)], 2.), &[]),
            [[100., 200., 20., 20.]],
            "a box in a scaled layer"
        );
        // A tile at the place of a freed one has new pixels for the same instance.
        let mut first = scene(&[a], &[inner], 1.);
        let fresh = [(
            true,
            2,
            crate::Rect::new(crate::point(20, 20), crate::size(4, 4)),
        )];
        assert_eq!(
            damage(&mut first, scene(&[a], &[inner], 1.), &fresh),
            [[300., 200., 8., 8.]]
        );
    }

    #[test]
    fn damage_joins_near_rectangles_and_keeps_a_few() {
        let mut damage = Damage::default();
        damage.add(bounds([0., 0., 10., 10.]));
        damage.add(bounds([12., 0., 10., 10.]));
        assert_eq!(damage.rects, [bounds([0., 0., 22., 10.])]);
        damage.add(bounds([500., 500., 10., 10.]));
        assert_eq!(damage.rects.len(), 2);
        for at in 0..8 {
            damage.add(bounds([100. * at as f32, 300., 10., 10.]));
        }
        assert!(damage.rects.len() <= DAMAGE_RECTS);
        damage.add(bounds([0., 0., f32::INFINITY, 1.]));
        assert!(damage.whole);
    }
}
