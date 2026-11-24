use super::{Command, RenderFrame};
use crate::{
    Clip,
    gpu::scene::{BackdropState, EffectState, Scene, effect},
    gpu::{BOX, GLYPH, IMAGE, Instance, bytes, rect as bounds},
};

fn record(window: [u32; 2], quad: [f32; 4], scale: f32) -> RenderFrame {
    let mask = Clip::from(bounds([-1e6, -1e6, 2e6, 2e6]));
    let mut transform = crate::geometry::IDENTITY;
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
                glass: Some(crate::gpu::scene::GlassState {
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
        assert!(
            frame
                .commands
                .iter()
                .any(|command| { matches!(command, Command::Pass(pass, true) if *pass == target) })
        );
    }
}

#[test]
fn patched_transforms_draw_like_new_frames() {
    let mask = Clip::from(bounds([0., 0., 800., 600.]));
    let turn = |degrees: f32| {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let mut m = crate::geometry::IDENTITY;
        [m[0], m[1], m[4], m[5]] = [cos, sin, -sin, cos];
        [m[12], m[13]] = [
            150. - cos * 150. + sin * 150.,
            150. - sin * 150. - cos * 150.,
        ];
        m
    };
    let shift = |x: f32, y: f32| {
        let mut m = crate::geometry::IDENTITY;
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
        let mut matrix = crate::geometry::IDENTITY;
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
                Command::Scissor(region) => format!("scissor {region:?}"),
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
        frame
            .fill_pool(pool, |_| {
                made += 1;
                Ok(made)
            })
            .unwrap();
    };
    fill(&mut pool, &used);
    let target = super::Target::Layer(0);
    assert_eq!(used.pooled(&pool, target), Some(&1));
    assert_eq!(used.pooled(&pool, super::Target::Frame), None);
    for _ in 0..2 {
        fill(&mut pool, &idle);
    }
    fill(&mut pool, &used);
    assert_eq!(used.pooled(&pool, target), Some(&1));
    for _ in 0..3 {
        fill(&mut pool, &idle);
    }
    fill(&mut pool, &used);
    assert_eq!(used.pooled(&pool, target), Some(&2));
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
        frame
            .fill_pool(&mut pool, |_| {
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
            target
                .and_then(|target| frame.pooled(&pool, target))
                .copied(),
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
fn blurred_layers_that_scroll_across_the_window_edge_keep_their_pixels() {
    let mask = Clip::from(bounds([0., 0., 800., 600.]));
    let scene = |y: f32| {
        let mut scene = Scene::default();
        scene.push(Instance::new(BOX, bounds([0., 0., 800., 600.]), mask));
        scene.push_effect(
            EffectState {
                blur: 2.,
                ..effect(mask)
            },
            None,
        );
        let open = Clip::from(bounds([-1e6, -1e6, 2e6, 2e6]));
        scene.push(Instance::new(BOX, bounds([100., y, 100., 200.]), open));
        scene.pop_effect();
        scene.diff(Some(&[]));
        scene
    };
    let mut frame = RenderFrame::default();
    let blurs = (0..12)
        .map(|step| {
            frame.record(&scene(500. - 6. * step as f32), [800, 600], 16384, true);
            let commands = frame.commands.iter();
            commands
                .filter(|command| matches!(command, Command::Blur { .. }))
                .count()
        })
        .collect::<Vec<_>>();
    assert_eq!(&blurs[..2], [1, 1], "the second frame keeps the blur");
    assert!(
        blurs[2..].iter().all(|blurs| *blurs == 0),
        "a layer that moves across the edge draws the kept blur: {blurs:?}"
    );
}

#[test]
fn damage_regions_copy_into_the_window_in_one_pass() {
    let mask = Clip::from(bounds([0., 0., 400., 300.]));
    let mut scene = Scene::default();
    scene.push(Instance::new(BOX, bounds([0., 0., 400., 300.]), mask));
    scene.push_effect(
        EffectState {
            opacity: 0.5,
            ..effect(mask)
        },
        None,
    );
    scene.push(Instance::new(BOX, bounds([20., 20., 300., 200.]), mask));
    scene.pop_effect();
    let mut frame = RenderFrame {
        damage: vec![[0, 0, 50, 50], [200, 200, 50, 50]],
        ..Default::default()
    };
    frame.record(&scene, [400, 300], 4096, true);
    let copies = (frame.commands.iter())
        .map(|command| {
            matches!(
                command,
                Command::Copy {
                    dst: super::Target::Frame,
                    ..
                }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(copies.iter().filter(|copy| **copy).count(), 2);
    assert!(
        copies.ends_with(&[true, true]),
        "the copies come last, together"
    );
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
        let regions = scene.damage_regions([400, 300], &[]);
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

#[test]
fn scissor_regions_draw_into_the_window() {
    let mask = Clip::from(bounds([0., 0., 400., 300.]));
    let mut scene = Scene::default();
    scene.push(Instance::new(BOX, bounds([0., 0., 400., 300.]), mask));
    let mut frame = RenderFrame {
        damage: vec![[10, 10, 50, 50]],
        scissors: true,
        ..Default::default()
    };
    frame.record(&scene, [400, 300], 16384, true);
    assert!(matches!(
        frame.commands[0],
        Command::Scissor(Some([10, 10, 50, 50]))
    ));
    assert!(matches!(
        frame.commands[1],
        Command::Pass(super::Target::Frame, true)
    ));
    assert!(matches!(
        frame.commands.last(),
        Some(Command::Scissor(None))
    ));
    assert!(frame.layers.is_empty());
}
