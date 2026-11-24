use super::{diff::DAMAGE_RECTS, *};
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
