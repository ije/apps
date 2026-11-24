use super::*;

impl RenderFrame {
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

    pub(super) fn record_effect(
        &mut self,
        scene: &Scene,
        nested: &Nested,
        target: Target,
        depth: usize,
    ) {
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
        let spread = effect
            .blur
            .max(effect.shadow.map_or(0., |(_, sigma, _)| sigma))
            * 3.;
        let full = mask.intersect(Rect::new(
            Point::new(target_rect[0], target_rect[1]),
            Size::new(target_rect[2], target_rect[3]),
        ));
        // A damage region draws only the part of a changed layer that reaches it.
        let visible = match self.cull {
            Some(cull) => {
                let offset = effect
                    .shadow
                    .map_or(0., |(offset, ..)| offset.x.abs().max(offset.y.abs()));
                full.intersect(cull.outset(spread + offset + 1.))
            }
            None => full,
        };
        let [full, visible] = [full, visible].map(|rect| rect.to_array());

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
        let content = content.outset(spread);
        let [reach, around] = match &effect.transform {
            Some(transform) => {
                let [x, y, width, height] = visible;
                let back = invert(&plane_of(transform));
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
                let reach = match seen {
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
                };
                [reach; 2]
            }
            None => {
                let at = crate::point(content.origin.x.floor(), content.origin.y.floor());
                let snap = |low: f32, size: f32, at: f32| {
                    let [low, high] = [low, low + size].map(|edge| (edge - at) / CUT_STEP);
                    [low.floor(), high.ceil()].map(|edge| edge * CUT_STEP + at)
                };
                let [[x0, x1], [y0, y1]] =
                    [snap(full[0], full[2], at.x), snap(full[1], full[3], at.y)];
                [visible, [x0, y0, x1 - x0, y1 - y0]]
            }
        };
        let Some(whole_region) = self.region(content, around) else {
            return;
        };
        // A layer that draws as in an earlier frame takes the pixels of then. One that draws the same in
        // two frames keeps them.
        let digest = (scene.digests.get(nested.layer).copied())
            .filter(|digest| *digest != 0 && effect.text_mask.is_none());
        // The pixels are the same at another place, so the key holds the region relative to the layer.
        let origin = crate::scene::origin(layer).map(|value| value as i32);
        let border =
            |region: [i32; 4]| [region[0] - 1, region[1] - 1, region[2] + 2, region[3] + 2];
        let key = digest.map(|digest| {
            [
                whole_region[0] - origin.x,
                whole_region[1] - origin.y,
                whole_region[2],
                whole_region[3],
            ]
            .iter()
            .fold(digest, |key, value| crate::scene::mix(key, *value as u64))
        });
        let cached = key.and_then(|key| self.reuse(key, border(whole_region)));
        let count = self.count;
        let seen = cached.is_none()
            && key.is_some_and(|key| {
                (self.seen.insert(key, count)).is_some_and(|seen| seen != count)
            });
        // A layer that changed draws only what the damage region reaches, and isn't kept.
        let (region, key, visible) = match cached.is_some() || seen {
            true => (whole_region, key, full),
            false => match self.region(content, reach) {
                Some(region) if region == whole_region => (region, key, full),
                Some(region) => (region, None, visible),
                None => return,
            },
        };
        let bordered = border(region);
        // Tiles must hold all their content, since drawing past a tile would reach the next one.
        let whole = Some(region)
            == self.region(content, [f32::MIN, f32::MIN, f32::INFINITY, f32::INFINITY]);
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
