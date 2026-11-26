use super::*;

// Clips at least this wide are the open mask of an effect layer.
pub(crate) const OPEN: f32 = 1e6;

// A clip of a moved span: the mask `old` around the span becomes the mask `now` there, the open mask of an
// effect layer stays, and other clips are inside the span and move with it.
pub(super) fn moved_clip(
    clip: Clip<f32>,
    by: Point<f32>,
    old: &Clip<f32>,
    now: &Clip<f32>,
) -> Clip<f32> {
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

impl Scene {
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
                let mut instance = moved.map_or(*instance, |(by, old, now)| {
                    moved_instance(instance, by, &old, &now)
                });
                // Only the part of a clip that cuts the instance counts. A clip that equaled the mask of the last
                // paint takes the mask of the copy, so an inner clip that the old mask hid can't come back.
                instance.clip = instance.reach().to_array();
                // Fields as floats, so the check can allow the rounding of moved copies.
                let fields: Vec<f32> = bytes(slice::from_ref(&instance))
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|word| f32::from_ne_bytes(*word))
                    .collect();
                out.push(format!("{name} {fields:?}"));
            }
            for nested in &layer.nested[start as usize..stop as usize] {
                let nested = moved.map_or(*nested, |(by, old, now)| {
                    moved_nested(nested, by, &old, &now)
                });
                let moved = Nested {
                    at: nested.at - first,
                    layer: 0,
                    clip: Clip {
                        bounds: nested.reach(),
                        ..nested.clip
                    },
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

    // Whether the span of `last` reads an atlas tile that was removed since that paint.
    pub(crate) fn reads_removed(&self, span: &Span) -> bool {
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
}
