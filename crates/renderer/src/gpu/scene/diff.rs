use super::*;

// The window region to draw again: a few rectangles, or the whole window.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Damage {
    pub rects: Vec<Rect<f32>>,
    pub whole: bool,
}

pub(super) const DAMAGE_RECTS: usize = 16;
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

    pub(super) fn clear(&mut self) {
        self.rects.clear();
        self.whole = false;
    }
}

// An instance or a nested layer of the window, in paint order. `reach` is in the space of `layer`.
#[derive(Clone, Copy)]
pub(super) struct Entry {
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

// How a nested layer shows its pixels in its parent.
#[derive(Clone, Copy, Default)]
pub(super) struct Map {
    parent: u32,
    clip: Rect<f32>,
    spread: f32,
    shadow: Option<(Point<f32>, f32)>,
    transform: Option<[f32; 16]>,
}

fn bits<const N: usize>(values: [f32; N]) -> [u32; N] {
    values.map(f32::to_bits)
}

fn place(origin: Point<f32>) -> u64 {
    u64::from(origin.x.to_bits()) << 32 | u64::from(origin.y.to_bits())
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

impl Scene {
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
    pub(crate) fn diff(&mut self, fresh: Option<&[crate::gpu::atlas::Fresh]>) {
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
}

pub(super) fn effect_bounds(layer: &SceneLayer) -> Option<Rect<f32>> {
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
