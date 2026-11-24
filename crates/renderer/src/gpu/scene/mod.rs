mod diff;
mod span;

use crate::{
    Clip, Color, Pixels, Point, Radii, Rect, Size,
    geometry::{region, transform_bounds},
    gpu::atlas::Tile,
    gpu::{Instance, SHADOW, bytes, rect},
    point,
};
use diff::{Damage, Entry, Map, effect_bounds};
use rustc_hash::FxHashMap;
use span::moved_clip;
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

pub(crate) const NESTED: u32 = 1 << 31;

pub(crate) fn mix(hash: u64, value: u64) -> u64 {
    (hash.rotate_left(5) ^ value).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95)
}

// The place of a layer that its digest is relative to, so a layer that moves keeps its digest.
pub(crate) fn origin(layer: &SceneLayer) -> Point<f32> {
    layer.bounds.map_or_else(Point::default, |bounds| {
        point(bounds.origin.x.floor(), bounds.origin.y.floor())
    })
}

// A clip changes the pixels only where it cuts the instance, so the reach stands for it. Places are
// relative to `origin`.
pub(crate) fn within(area: Rect<f32>, clip: Rect<f32>) -> bool {
    area.is_empty() || area.intersect(clip) == area
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
mod tests;
