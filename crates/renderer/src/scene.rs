use crate::{
    Clip, Color, Pixels, Point, Radii, Rect, Size,
    atlas::Tile,
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
    pub(crate) reads: Vec<Rect<f32>>,
    entries: Vec<Entry>,
    maps: Vec<Map>,
    prior: Vec<Entry>,
    prior_maps: Vec<Map>,
    // `prior` holds what the window shows, and `entries` this scene since `diff`.
    drawn: bool,
    // `Some` after `clear`, and true after `diff`.
    diffed: Option<bool>,
    // Paint that the content mask cut away: instances outside their clip, or content that paint skipped.
    pub(crate) culled: u32,
    // Patched regions since the last diff, which the prior entries don't know.
    patched: Damage,
    salt: u64,
}

// The window region to draw again: a few rectangles, or the whole window.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Damage {
    pub rects: Vec<Rect<f32>>,
    pub whole: bool,
}

const DAMAGE_RECTS: usize = 4;
// A rectangle joins another when their union adds at most this many pixels.
const DAMAGE_SLACK: f32 = 64. * 64.;

impl Damage {
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
}

const NESTED: u32 = 1 << 31;

// How a nested layer shows its pixels in its parent.
#[derive(Clone, Copy, Default)]
struct Map {
    parent: u32,
    clip: Rect<f32>,
    spread: f32,
    shadow: Option<(Point<f32>, f32)>,
    transform: Option<[f32; 16]>,
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

fn instance_hash(instance: &Instance, origin: Point<f32>) -> u64 {
    let mut copy = *instance;
    let shift = |[x, y, width, height]: [f32; 4]| [x - origin.x, y - origin.y, width, height];
    copy.rect = shift(instance.rect);
    copy.clip = shift(instance.reach().to_array());
    copy.round = match instance.round_radii == [0.; 8] {
        true => [0.; 4],
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
}

// What a paint span added to the scene: instances and nested layers of its layer, and the layers it made.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Span {
    layer: u32,
    instances: [u32; 2],
    nested: [u32; 2],
    layers: [u32; 2],
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
            entries: Vec::new(),
            maps: Vec::new(),
            prior: Vec::new(),
            prior_maps: Vec::new(),
            drawn: false,
            diffed: None,
            patched: Damage::default(),
            culled: 0,
            salt: 0,
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
        self.shots.clear();
        self.damage.clear();
        self.damage.whole = true;
        self.diffed = Some(false);
        // A painted scene stays as the source of the next paint's spans.
        if let Some(pass) = self.pass.take() {
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
    }

    pub(crate) fn begin_paint(&mut self, pass: u32) {
        self.pass = Some(pass);
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
    pub(crate) fn replay(&mut self, span: &Span) -> Option<Span> {
        let source = self.last.get(span.layer as usize)?;
        if source.instances.len() < span.instances[1] as usize
            || source.nested.len() < span.nested[1] as usize
            || self.last.len() < span.layers[1] as usize
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
        self.span(mark)
    }

    // The area in `clip` that a span of this paint holds, when a copy of it moved by whole pixels draws as its
    // paint there would: its clips are the mask around it, with all it draws inside the mask, or clips inside
    // the mask, and its layers have no effects that read the pixels below or turn in perspective.
    pub(crate) fn plain(&self, span: &Span, clip: &Clip<f32>) -> Option<Rect<f32>> {
        let fixed = |effect: &Option<EffectState>| {
            effect.as_ref().is_none_or(|effect| {
                effect.backdrop.is_some()
                    || effect.glass.is_some()
                    || effect.text_mask.is_some()
                    || effect
                        .transform
                        .is_some_and(|m| (m[3], m[7], m[11], m[15]) != (0., 0., 0., 1.))
            })
        };
        if (span.layers[0]..span.layers[1]).any(|at| fixed(&self.layers[at as usize].effect)) {
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
        self.span(mark)
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
                region: crate::frame::region(bounds),
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
                let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
                (
                    effect.transform.map(|matrix| bits(&matrix)),
                    effect.opacity.to_bits(),
                    effect.color_matrix.map(|matrix| bits(&matrix)),
                    effect.blur.to_bits(),
                    effect.blend_mode as u32,
                )
                    .hash(hasher);
                (
                    effect.shadow.map(|(by, sigma, color)| {
                        bits(&[by.x, by.y, sigma, color.r, color.g, color.b, color.a])
                    }),
                    effect.clip.map(|(bounds, radii)| {
                        (
                            place(bounds.to_array()),
                            bits(&radii.to_array()),
                            radii.shape,
                        )
                    }),
                    effect.mask.as_ref().map(|(tile, bounds)| {
                        (
                            tile.page,
                            bits(&tile.rect.map(|value| value as f32).to_array()),
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
        // The reach in window pixels, while no layer around it moves or spreads its pixels.
        let reach = |scene: &Self| {
            let mut child = index;
            let mut reach = None;
            loop {
                let (parent, nested) =
                    scene.layers.iter().enumerate().find_map(|(at, layer)| {
                        let nested = layer.nested.iter().find(|nested| nested.layer == child)?;
                        Some((at, nested))
                    })?;
                let reach = *reach.get_or_insert(nested.reach());
                if parent == 0 {
                    return Some(reach);
                }
                let effect = scene.layers[parent].effect.as_ref()?;
                if effect.transform.is_some()
                    || effect.blur > 0.
                    || effect.shadow.is_some()
                    || effect.backdrop.is_some()
                    || effect.blend_mode != crate::BlendMode::Normal
                {
                    return None;
                }
                child = parent;
            }
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
        match (before, reach(self)) {
            (Some(before), Some(after)) => {
                let changed = before.union(after);
                self.damage.add(changed);
                self.patched.add(changed);
            }
            _ => (self.damage.whole, self.patched.whole) = (true, true),
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
        let mut walk = Walk {
            layers: &self.layers,
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
        walk.layer(0, 0);
        (self.entries, self.maps, self.digests, self.reads) =
            (walk.entries, walk.maps, walk.digests, walk.reads);
        self.diffed = Some(true);
        let mut damage = mem::take(&mut self.patched);
        damage.whole |= !self.drawn;
        let changes = |damage: &mut Damage, entries: &[Entry], maps: &[Map], at: usize| {
            let entry = &entries[at];
            damage.add(window_reach(maps, entry.layer, entry.reach));
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
                changes(&mut damage, new, &self.maps, at);
                continue;
            }
            for skipped in next..found as usize {
                changes(&mut damage, old, &self.prior_maps, skipped);
            }
            next = found as usize + 1;
        }
        for skipped in next..old.len() {
            if damage.whole {
                break;
            }
            changes(&mut damage, old, &self.prior_maps, skipped);
        }
        // The same instance can read a tile with new pixels. Its layers can't keep their pixels either.
        self.stale.clear();
        let Some(fresh) = fresh else {
            damage.whole = true;
            self.stale.extend_from_slice(&self.digests);
            self.damage = damage;
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
                damage.add(window_reach(&self.maps, entry.layer, entry.reach));
                let mut layer = entry.layer;
                while layer != 0 {
                    self.stale.push(self.digests[layer as usize]);
                    layer = self.maps[layer as usize].parent;
                }
            }
        }
        self.damage = damage;
    }

    // The backend drew the scene as it is.
    pub(crate) fn drawn(&mut self) {
        self.damage.clear();
        // A new scene without a diff leaves the prior entries behind.
        self.drawn = self.diffed != Some(false);
        if self.diffed.take() == Some(true) {
            mem::swap(&mut self.prior, &mut self.entries);
            self.prior_maps.clone_from(&self.maps);
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
    entries: Vec<Entry>,
    maps: Vec<Map>,
    digests: Vec<u64>,
    reads: Vec<Rect<f32>>,
}

impl Walk<'_> {
    // Lists the entries of a layer and returns its digest. `context` stands for the layers around it.
    fn layer(&mut self, layer: usize, context: u64) -> u64 {
        let content = &self.layers[layer];
        let origin = match layer {
            0 => Point::default(),
            _ => origin(content),
        };
        let context = mix(context, place(origin));
        let mut digest = 1;
        let mut nested = content.nested.iter().peekable();
        for (at, instance) in content.instances.iter().enumerate() {
            while let Some(inner) = nested.next_if(|inner| inner.at as usize <= at) {
                digest = mix(digest, self.nested(layer, origin, inner, context));
            }
            let hash = instance_hash(instance, origin);
            digest = mix(digest, hash);
            self.entries.push(Entry {
                hash: mix(hash, context),
                reach: instance.reach(),
                layer: layer as u32,
                item: at as u32,
            });
        }
        for inner in nested {
            digest = mix(digest, self.nested(layer, origin, inner, context));
        }
        self.digests[layer] = digest;
        digest
    }

    // Places are relative to `base`, the origin of the parent.
    fn nested(&mut self, parent: usize, base: Point<f32>, nested: &Nested, context: u64) -> u64 {
        let layers = self.layers;
        let Some(effect) = &layers[nested.layer].effect else {
            return 0;
        };
        let mut hasher = rustc_hash::FxHasher::default();
        let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        let rect = |rect: Rect<f32>| {
            bits(&[
                rect.origin.x - base.x,
                rect.origin.y - base.y,
                rect.size.width,
                rect.size.height,
            ])
        };
        (
            effect.transform.map(|matrix| bits(&matrix)),
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
                    bits(&backdrop.corner_radii.to_array()),
                    backdrop.corner_radii.shape,
                    backdrop.blur.to_bits(),
                    backdrop.color_matrix.map(|matrix| bits(&matrix)),
                )
            }),
            effect.glass.as_ref().map(|glass| {
                (
                    glass.key,
                    rect(glass.bounds),
                    glass.corner_radius.to_bits(),
                    glass
                        .tint
                        .map(|tint| bits(&[tint.r, tint.g, tint.b, tint.a])),
                    glass.clear,
                    glass.interactive,
                )
            }),
            // A clip counts only where the layer reaches.
            rect(nested.clip.bounds.intersect(nested.bounds)),
            (nested.clip.radii != Default::default()).then(|| rect(nested.clip.rounded)),
            bits(&nested.clip.radii.to_array()),
            nested.clip.radii.shape,
        )
            .hash(&mut hasher);
        if let Some(text) = effect.text_mask {
            for instance in &layers[text].instances {
                instance_hash(instance, base).hash(&mut hasher);
            }
        }
        let own = hasher.finish();
        self.entries.push(Entry {
            hash: mix(own, context),
            reach: nested.reach(),
            layer: parent as u32,
            item: nested.layer as u32 | NESTED,
        });
        self.maps[nested.layer] = Map {
            parent: parent as u32,
            clip: nested.clip.bounds,
            spread: effect.blur * 3.,
            shadow: effect.shadow.map(|(offset, sigma, _)| (offset, sigma)),
            transform: effect.transform,
        };
        if effect.backdrop.is_some() || effect.blend_mode != BlendMode::Normal {
            let reach = window_reach(&self.maps, parent as u32, nested.reach());
            self.reads.push(reach);
        }
        let inner = origin(&layers[nested.layer]);
        let offset = place(point(inner.x - base.x, inner.y - base.y));
        mix(
            mix(own, offset),
            self.layer(nested.layer, mix(own, context)),
        )
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
        scene.begin_paint(1);
        scene.push(at(90.));
        let mark = scene.mark();
        paint(&mut scene, 20.);
        let span = scene.span(mark).unwrap();
        let painted = scene.records(&span, false, None);
        scene.clear();
        scene.begin_paint(2);
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
    fn layers_take_back_their_own_buffers() {
        let mask = Clip::from(bounds([0., 0., 200., 200.]));
        let at = Instance::new(BOX, bounds([0., 0., 10., 10.]), mask);
        let mut scene = Scene::default();
        let mut capacities = Vec::new();
        for pass in 1..5 {
            scene.begin_paint(pass);
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
        scene.begin_paint(1);
        let mark = scene.mark();
        paint(&mut scene, 60., mask);
        let span = scene.span(mark).unwrap();
        let area = scene.plain(&span, &mask).unwrap();
        assert_eq!(area, bounds([10., 60., 30., 30.]));
        // A box that the clip cuts can't move.
        assert_eq!(scene.plain(&span, &cut), None);
        scene.clear();
        scene.begin_paint(2);
        let moved = scene
            .replay_moved(&span, point(0., -8.), &mask, &cut)
            .unwrap();
        let mut fresh = Scene::default();
        fresh.begin_paint(3);
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
        scene.begin_paint(1);
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
        scene.begin_paint(2);
        let moved = scene
            .replay_moved(&span, point(0., -8.), &mask, &mask)
            .unwrap();
        let mut fresh = Scene::default();
        fresh.begin_paint(3);
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
            let mut matrix = crate::frame::IDENTITY;
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
