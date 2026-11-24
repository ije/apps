use super::*;

// The retained paint that tree drives: copies of the spans and hit regions of the last paint.
impl Window {
    pub(crate) fn prepaint_mark(&self) -> PrepaintMark {
        PrepaintMark {
            unsettled: self.unsettled,
            transforms: self.transforms,
        }
    }

    // Whether the prepaint since `mark` made hit transforms.
    pub(crate) fn transformed_since(&self, mark: PrepaintMark) -> bool {
        self.transforms != mark.transforms
    }

    // Whether the prepaint since `mark` read state outside the tree that changed.
    pub(crate) fn unsettled_since(&self, mark: PrepaintMark) -> bool {
        self.unsettled != mark.unsettled
    }

    pub(crate) fn with_font_smoothing<R>(&mut self, on: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        let smoothing = mem::replace(&mut self.font_smoothing, on);
        let result = f(self);
        self.font_smoothing = smoothing;
        result
    }

    // Whether the next paint can copy spans of the last one.
    pub(crate) fn copies(&self) -> bool {
        !self.repaint
            && !self.atlas.evicted()
            && self.text_system.generation() == self.generation
            && self.live.pass.is_some()
            && self.scene.last_pass == self.live.pass
    }

    // Returns the pass, and the pass whose spans can be copied.
    pub(crate) fn begin_paint(&mut self) -> (u32, Option<u32>) {
        self.paints = self.paints.wrapping_add(1);
        let generation = self.text_system.generation();
        let evicted = self.atlas.take_removed(&mut self.removed);
        self.scene.begin_paint(self.paints, &mut self.removed);
        let fresh = mem::take(&mut self.repaint) || evicted || generation != self.generation;
        self.generation = generation;
        self.pending.pass = Some(self.paints);
        let last = self
            .live
            .pass
            .filter(|_| !fresh && self.scene.last_pass == self.live.pass);
        (self.paints, last)
    }

    pub(crate) fn paint_key(&self) -> PaintKey {
        PaintKey {
            mask: self.content_mask(),
            offset: self.offset(),
            opacity: self.opacity,
            smoothing: self.font_smoothing,
            scale: self.scale_factor,
        }
    }

    pub(crate) fn paint_mark(&self) -> PaintMark {
        PaintMark {
            scene: self.scene.mark(),
            input: self.pending.lens(),
            volatile: self.volatile,
        }
    }

    // The area in device pixels that a span drew in `mask`, when the span can move with its box: its paint cut
    // nothing and drew nothing outside the mask or in layers.
    pub(crate) fn plain(
        &self,
        span: &PaintSpan,
        mark: PaintMark,
        mask: Clip<Pixels>,
    ) -> Option<Rect<f32>> {
        (!self.scene.culled_since(mark.scene))
            .then(|| self.scene.plain(&span.scene, &self.snap_mask(mask)))
            .flatten()
    }

    // A move within a rounding error of whole device pixels, as layout far down the page gives, is made whole.
    pub(crate) fn snap_move(&self, by: Point<Pixels>) -> Point<Pixels> {
        by.map(|value| {
            let device = value.0 * self.scale_factor;
            match (device - device.round()).abs() < 1e-3 {
                true => Pixels(device.round() / self.scale_factor),
                false => value,
            }
        })
    }

    // Whether a plain span that drew `area` can move by `by` and draw in `mask`, as a paint there would draw.
    pub(crate) fn movable(&self, area: Rect<f32>, by: Point<Pixels>, mask: Clip<Pixels>) -> bool {
        let by = by.scale(self.scale_factor);
        let whole = |value: f32| value == value.round();
        whole(by.x) && whole(by.y) && scene::within(area + by, self.snap_mask(mask).bounds)
    }

    // Whether `now` is the mask `old` moved by whole device pixels.
    pub(crate) fn translated(
        &self,
        old: Clip<Pixels>,
        now: Clip<Pixels>,
        by: Point<Pixels>,
    ) -> bool {
        let by = by.scale(self.scale_factor);
        let [old, now] = [old, now].map(|mask| self.snap_mask(mask));
        let near =
            |a: Point<f32>, b: Point<f32>| (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3;
        let whole = |value: f32| value == value.round();
        whole(by.x)
            && whole(by.y)
            && near(old.bounds.origin + by, now.bounds.origin)
            && near(old.bounds.bottom_right() + by, now.bounds.bottom_right())
            && (old.rounded.size == Size::default() && now.rounded.size == Size::default()
                || near(old.rounded.origin + by, now.rounded.origin)
                    && near(old.rounded.bottom_right() + by, now.rounded.bottom_right()))
            && old.radii == now.radii
    }

    // Whether a copy of a span can move with its box: its layers don't read pixels below or a mask layer, or
    // turn in perspective.
    pub(crate) fn translatable(&self, span: &PaintSpan) -> bool {
        self.scene.translatable(&span.scene)
    }

    // What a span drew. `last` reads a span of the last paint.
    #[cfg(debug_assertions)]
    pub(crate) fn span_records(
        &self,
        span: &PaintSpan,
        last: bool,
        moved: Option<(Point<Pixels>, Clip<Pixels>, Clip<Pixels>)>,
    ) -> Vec<String> {
        let moved = moved.map(|(by, old, now)| {
            (
                by.scale(self.scale_factor),
                self.snap_mask(old),
                self.snap_mask(now),
            )
        });
        let mut out = self.scene.records(&span.scene, last, moved);
        let input = if last { &self.live } else { &self.pending };
        let range = |[start, end]: [u32; 2]| start as usize..end as usize;
        let [listeners, cursors, drags, marks] = span.input.map(range);
        let phases: Vec<_> = input.listeners[listeners]
            .iter()
            .map(|(phase, _)| *phase)
            .collect();
        out.push(format!("listeners {phases:?}"));
        // The prepaint check of debug builds gives regions new keys.
        let kinds: Vec<_> = input.cursors[cursors]
            .iter()
            .map(|(_, cursor)| *cursor)
            .collect();
        out.push(format!("cursors {kinds:?}"));
        out.push(format!("drag areas {}", input.drag_areas[drags].len()));
        out.push(format!("marks {:?}", &input.marks[marks]));
        out
    }

    // `None` when paint read state outside the tree, or wrote where a copy can't follow.
    pub(crate) fn paint_span(&self, mark: PaintMark) -> Option<PaintSpan> {
        if self.volatile != mark.volatile || self.text_clip.is_some() {
            return None;
        }
        let ends = self.pending.lens();
        Some(PaintSpan {
            scene: self.scene.span(mark.scene)?,
            input: std::array::from_fn(|at| [mark.input[at], ends[at]]),
        })
    }

    // Copies a span of the last paint.
    pub(crate) fn replay(
        &mut self,
        span: &PaintSpan,
        moved: Option<(Point<Pixels>, Clip<Pixels>, Clip<Pixels>)>,
    ) -> Option<PaintSpan> {
        let ([listeners, cursors, drags, marks], live) = (span.input, &self.live);
        let range = |[start, end]: [u32; 2]| start as usize..end as usize;
        if self.text_clip.is_some()
            || live
                .lens()
                .iter()
                .zip(&span.input)
                .any(|(len, [_, end])| end > len)
        {
            return None;
        }
        let start = self.pending.lens();
        let scene = match moved {
            Some((by, old, now)) => {
                let (old, now) = (self.snap_mask(old), self.snap_mask(now));
                (self.scene).replay_moved(&span.scene, by.scale(self.scale_factor), &old, &now)?
            }
            None => self.scene.replay(&span.scene)?,
        };
        let pending = &mut self.pending;
        Rc::make_mut(&mut pending.listeners).extend_from_slice(&live.listeners[range(listeners)]);
        pending
            .cursors
            .extend_from_slice(&live.cursors[range(cursors)]);
        pending
            .drag_areas
            .extend_from_slice(&live.drag_areas[range(drags)]);
        pending.marks.extend_from_slice(&live.marks[range(marks)]);
        let ends = pending.lens();
        Some(PaintSpan {
            scene,
            input: std::array::from_fn(|at| [start[at], ends[at]]),
        })
    }

    // Kept subtrees reuse their regions only while this is unchanged.
    pub(crate) fn placement(&self) -> Placement {
        let transform = (self.hit_transform.as_ref()).map_or(0, |transform| transform.id);
        Placement {
            offset: self.offset(),
            mask: self.content_mask(),
            visible: self.visible_mask().bounds,
            depth: self.depth() as u32,
            transform,
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn region_places(&self) -> Vec<(Rect, Clip<Pixels>, Hit, bool)> {
        let place = |placed: &Placed| {
            let region = placed.region;
            (
                region.bounds,
                Clip {
                    bounds: placed.clip.bounds.intersect(placed.view),
                    ..placed.clip
                },
                region.hit,
                placed.transform.is_some(),
            )
        };
        self.pending.regions.iter().map(place).collect()
    }

    #[cfg(debug_assertions)]
    pub(crate) fn redraws(&self) -> bool {
        self.again
    }

    pub(crate) fn truncate_regions(&mut self, len: usize) {
        self.pending.regions.truncate(len);
    }

    pub(crate) fn begin_prepaint(&mut self, pass: u32) {
        self.pending.prepass = Some(pass);
    }

    pub(crate) fn regions_len(&self) -> u32 {
        self.pending.regions.len() as u32
    }

    // Copies regions of the prepaint pass `pass` moved by a scroll offset `by`, and returns where they start.
    pub(crate) fn shift_regions(
        &mut self,
        pass: u32,
        range: [u32; 2],
        by: Point<Pixels>,
    ) -> Option<u32> {
        let at = self.copy_regions(pass, range)?;
        // The viewports of scrolls around the box can move with an outer scroll, so its regions see them anew.
        let viewports = self.viewport_bounds();
        let mut moved = Vec::new();
        for placed in &mut self.pending.regions[at as usize..] {
            placed.region.bounds.origin += by;
            // The open mask inside a transform stays.
            if placed.clip.bounds.size.width.0 < OPEN {
                placed.clip.bounds.origin += by;
            }
            if placed.transform.is_none() {
                placed.view = placed.clip.bounds.intersect(viewports);
            }
            if placed.clip.rounded.size != Size::default() {
                placed.clip.rounded.origin += by;
            }
            if let Some(transform) = &placed.transform {
                placed.transform = Some(transform.shifted(by, &mut moved));
            }
        }
        Some(at)
    }

    // Copies regions of the prepaint pass `pass`, and returns where they start.
    pub(crate) fn copy_regions(&mut self, pass: u32, [start, end]: [u32; 2]) -> Option<u32> {
        let source = match (self.live.prepass, self.spare.0) {
            (Some(live), _) if live == pass => &self.live.regions,
            (_, Some(spare)) if spare == pass => &self.spare.1,
            _ => return None,
        };
        let copied = source.get(start as usize..end as usize)?;
        let at = self.pending.regions.len() as u32;
        self.pending.regions.extend_from_slice(copied);
        Some(at)
    }
}
