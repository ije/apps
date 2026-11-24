use crate::{Color, Rect, point, size};
use objc2::rc::Retained;
use objc2_quartz_core::{CALayer, CAMediaTiming};

#[test]
fn page_sublayers_stay_in_the_overlay_below_the_caret() {
    let mut renderer = super::Renderer::new(false).unwrap();
    let sublayers = |layer: &CALayer| -> Vec<*const CALayer> {
        // SAFETY: a plain layer property.
        unsafe { layer.sublayers() }.map_or_else(Vec::new, |list| {
            list.iter().map(|layer| Retained::as_ptr(&layer)).collect()
        })
    };
    let turn = [
        0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 1., 0., 65., -15., 0., 1.,
    ];
    let since = objc2_quartz_core::CACurrentMediaTime();
    renderer.promote(
        1,
        3,
        [20, 10, 40, 30],
        [0.5, f64::INFINITY, 0.25],
        &[1., 0.5],
        &[turn, turn],
    );
    let caret = Rect {
        origin: point(4., 6.),
        size: size(2., 16.),
    };
    let black = Color {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 1.,
    };
    renderer.set_caret(Some((caret, black, std::time::Instant::now())));
    renderer.promote(
        2,
        4,
        [100, 10, 40, 30],
        [1. / 120., 1., 0.],
        &[1., 0.5],
        &[],
    );
    let caret = Retained::as_ptr(&renderer.caret.as_ref().unwrap().0);
    let promoted: Vec<_> = renderer
        .promoted
        .iter()
        .map(|promoted| Retained::as_ptr(&promoted.sublayer).cast::<CALayer>())
        .collect();
    assert!(
        sublayers(&renderer.layer).is_empty(),
        "AppKit owns the frame layer's sublayers"
    );
    assert_eq!(
        sublayers(&renderer.overlay),
        [promoted[1], promoted[0], caret]
    );
    let first = &renderer.promoted[0].sublayer;
    assert_eq!((first.position().x, first.position().y), (20., 10.));
    let turned = first.transform();
    assert_eq!(
        [
            turned.m11, turned.m12, turned.m21, turned.m22, turned.m41, turned.m42
        ],
        [0., 1., -1., 0., 35., -5.],
        "a turn about the center of the sublayer, from its top left corner"
    );
    // SAFETY: a plain layer property.
    let looped =
        unsafe { first.animationForKey(objc2_foundation::ns_string!("transform")) }.unwrap();
    assert_eq!(
        (looped.repeatCount(), looped.duration()),
        (f32::INFINITY, 0.5)
    );
    assert!(
        (since - 0.25 - looped.beginTime()).abs() < 0.05,
        "it starts at the first sample"
    );
    assert!(renderer.overlay.masksToBounds());
    let position = renderer.caret.as_ref().unwrap().0.position();
    assert_eq!((position.x, position.y), (4., 6.));
    renderer.demote();
    assert_eq!(sublayers(&renderer.overlay), [caret]);
    renderer.set_caret(None);
    assert!(sublayers(&renderer.overlay).is_empty());
}

#[test]
fn opacity_patches_redraw_only_their_region_like_a_full_frame() {
    use crate::{
        Clip,
        gpu::scene::{EffectState, Scene, effect},
        gpu::{BOX, Instance, rect},
    };
    use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLCommandQueue};
    let (send, receive) = std::sync::mpsc::channel();
    let job = move || {
        let mut renderer = super::Renderer::new(false).unwrap();
        renderer.set_drawable_size(objc2_core_foundation::CGSize::new(200., 120.));
        let mask = Clip::from(rect([0., 0., 200., 120.]));
        let quad = |bounds: [f32; 4], color: [f32; 4]| {
            let mut instance = Instance::new(BOX, rect(bounds), mask);
            instance.color = color;
            instance
        };
        let mut scene = Scene::default();
        scene.push(quad([0., 0., 200., 120.], [0.9, 0.9, 0.85, 1.]));
        scene.push(quad([150., 10., 40., 40.], [0.2, 0.4, 0.8, 1.]));
        let state = EffectState {
            opacity: 0.8,
            ..effect(mask)
        };
        scene.push_effect(state, Some(7));
        scene.push(quad([20., 20., 30., 30.], [0.9, 0.1, 0.1, 1.]));
        scene.pop_effect();
        scene.push(quad([40., 30., 50., 10.], [0.1, 0.1, 0.1, 0.7]));
        renderer.draw(&scene, false);
        scene.drawn();
        for opacity in [0.3, 0.55] {
            scene.patch(7, opacity, None, [None; 2]);
            renderer.draw(&scene, false);
            scene.drawn();
            let (damage, drawn) = (
                renderer.frame.damage.clone(),
                renderer.frame.instances.len(),
            );
            let last = renderer.last.clone().unwrap();
            let commands = renderer.queue.commandBuffer().unwrap();
            let buffer = renderer.read_back(&commands, &last, [200, 120]).unwrap();
            commands.commit();
            commands.waitUntilCompleted();
            // SAFETY: the GPU finished writing the shared buffer.
            let mut pixels = unsafe {
                std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), buffer.length())
            }
            .to_vec();
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            let (_, full) = renderer.draw(&scene, true).unwrap().unwrap();
            send.send((damage, drawn, pixels == full)).unwrap();
        }
        super::super::threads::end_thread();
    };
    super::super::threads::spawn_render(job, || {});
    for _ in 0..2 {
        let (damage, drawn, same) = receive.recv().unwrap();
        assert_eq!(damage, [[19, 19, 32, 32]]);
        assert!(drawn < 4, "the far box is not drawn again");
        assert!(same, "the region matches a full frame");
    }
}

#[test]
fn rebuilt_scenes_redraw_only_what_changed_like_a_full_frame() {
    use crate::{
        Clip,
        gpu::atlas::TileKey,
        gpu::scene::{BackdropState, EffectState, Scene, effect},
        gpu::{BOX, IMAGE, Instance, rect},
    };
    use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLCommandQueue};
    #[derive(Clone, Copy)]
    struct Step {
        name: &'static str,
        box_color: f32,
        box_at: [f32; 2],
        inner: f32,
        opacity: f32,
        image: usize,
        turned: f32,
        shift: f32,
        corner: f32,
    }
    let first = Step {
        name: "first",
        box_color: 0.8,
        box_at: [150., 10.],
        inner: 0.1,
        opacity: 0.8,
        image: 0,
        turned: 0.2,
        shift: 0.,
        corner: 0.2,
    };
    let steps = [
        first,
        Step {
            name: "same",
            ..first
        },
        Step {
            name: "box color",
            box_color: 0.3,
            ..first
        },
        Step {
            name: "moved box",
            box_color: 0.3,
            box_at: [120., 14.],
            ..first
        },
        Step {
            name: "layer content",
            box_color: 0.3,
            box_at: [120., 14.],
            inner: 0.6,
            ..first
        },
        Step {
            name: "layer opacity",
            box_color: 0.3,
            box_at: [120., 14.],
            inner: 0.6,
            opacity: 0.5,
            ..first
        },
        Step {
            name: "image",
            box_color: 0.3,
            box_at: [120., 14.],
            inner: 0.6,
            opacity: 0.5,
            image: 1,
            ..first
        },
        Step {
            name: "turned layer content",
            box_color: 0.3,
            box_at: [120., 14.],
            inner: 0.6,
            opacity: 0.5,
            image: 1,
            turned: 0.9,
            ..first
        },
        Step {
            name: "below a backdrop",
            box_color: 0.3,
            box_at: [100., 50.],
            inner: 0.6,
            opacity: 0.5,
            image: 1,
            turned: 0.9,
            ..first
        },
        Step {
            name: "moved layer",
            box_color: 0.3,
            box_at: [100., 50.],
            inner: 0.6,
            opacity: 0.5,
            image: 1,
            turned: 0.9,
            shift: 3.,
            ..first
        },
        Step {
            name: "part of a shadowed layer",
            box_color: 0.3,
            box_at: [100., 50.],
            inner: 0.6,
            opacity: 0.5,
            image: 1,
            turned: 0.9,
            shift: 3.,
            corner: 0.7,
        },
    ];
    let (send, receive) = std::sync::mpsc::channel();
    let job = move || {
        let mut renderer = super::Renderer::new(false).unwrap();
        renderer.set_drawable_size(objc2_core_foundation::CGSize::new(200., 120.));
        let mask = Clip::from(rect([0., 0., 200., 120.]));
        let quad = |bounds: [f32; 4], color: [f32; 4]| {
            let mut instance = Instance::new(BOX, rect(bounds), mask);
            instance.color = color;
            instance
        };
        let tiles = [[230, 40, 40, 255], [40, 200, 90, 255]].map(|pixel: [u8; 4]| {
            let side = crate::size(4, 4);
            (renderer.atlas())
                .get_or_insert_with(&TileKey::Image(u64::from(pixel[0])), &mut || {
                    let bytes = pixel.repeat(16).into();
                    Ok(Some((Rect::new(Default::default(), side), side, bytes)))
                })
                .unwrap()
                .0
        });
        let build = |scene: &mut Scene, step: Step| {
            scene.push(quad([0., 0., 200., 120.], [0.9, 0.9, 0.85, 1.]));
            let [x, y] = step.box_at;
            scene.push(quad([x, y, 40., 40.], [0.2, 0.4, step.box_color, 1.]));
            let state = EffectState {
                opacity: step.opacity,
                ..effect(mask)
            };
            scene.push_effect(state, Some(7));
            let shift = step.shift;
            let inner = [0.9, step.inner, 0.1, 1.];
            scene.push(quad([20. + shift, 20. + shift, 30., 30.], inner));
            scene.push(quad(
                [24. + shift, 60. + shift, 20., 10.],
                [0.1, 0.5, 0.5, 1.],
            ));
            scene.pop_effect();
            let state = EffectState {
                shadow: Some((
                    crate::point(2., 2.),
                    2.,
                    crate::Color {
                        r: 0.,
                        g: 0.,
                        b: 0.,
                        a: 0.5,
                    },
                )),
                ..effect(mask)
            };
            scene.push_effect(state, Some(8));
            scene.push(quad([150., 80., 45., 36.], [0.8, 0.8, 0.8, 1.]));
            scene.push(quad([186., 106., 6., 6.], [step.corner, 0.3, 0.3, 1.]));
            scene.pop_effect();
            // A blend mode reads only the pixel below, so a change below it doesn't draw all of it.
            let state = EffectState {
                blend_mode: crate::BlendMode::Multiply,
                ..effect(mask)
            };
            scene.push_effect(state, None);
            scene.push(quad([140., 70., 60., 50.], [0.9, 0.6, 0.9, 0.5]));
            scene.pop_effect();
            scene.push(quad([40., 30., 50., 10.], [0.1, 0.1, 0.1, 0.7]));
            let tile = tiles[step.image];
            let mut image = Instance::new(IMAGE, rect([60., 84., 16., 16.]), mask);
            image.aux = tile.rect.map(|value| value as f32).to_array();
            (image.info[2], image.info[3]) = (1., tile.page as f32);
            scene.push(image);
            let (sin, cos) = 0.3f32.sin_cos();
            let mut turn = crate::geometry::IDENTITY;
            [turn[0], turn[1], turn[4], turn[5], turn[12], turn[13]] =
                [cos, sin, -sin, cos, 10., -20.];
            let state = EffectState {
                transform: Some(turn),
                ..effect(mask)
            };
            scene.push_effect(state, None);
            scene.push(quad([150., 60., 20., 20.], [step.turned, 0.2, 0.6, 1.]));
            scene.pop_effect();
            let state = EffectState {
                backdrop: Some(BackdropState {
                    bounds: rect([96., 64., 50., 40.]),
                    corner_radii: Default::default(),
                    blur: 3.,
                    color_matrix: None,
                }),
                ..effect(mask)
            };
            scene.push_effect(state, None);
            scene.push(quad([96., 64., 50., 40.], [1., 1., 1., 0.3]));
            scene.pop_effect();
        };
        let read = |renderer: &super::Renderer| {
            let last = renderer.last.clone().unwrap();
            let commands = renderer.queue.commandBuffer().unwrap();
            let buffer = renderer.read_back(&commands, &last, [200, 120]).unwrap();
            commands.commit();
            commands.waitUntilCompleted();
            // SAFETY: the GPU finished writing the shared buffer.
            let mut pixels = unsafe {
                std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), buffer.length())
            }
            .to_vec();
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            pixels
        };
        let mut scene = Scene::default();
        for step in steps {
            scene.clear();
            build(&mut scene, step);
            scene.diff(Some(&[]));
            let committed = renderer.committed;
            renderer.draw(&scene, false);
            scene.drawn();
            if step.name == "part of a shadowed layer" {
                let frame = &renderer.frame;
                // The shadow blurs the region of the layer.
                let blurred = (frame.blurs.iter())
                    .map(|blur| blur.region[2] * blur.region[3])
                    .fold(0., f32::max);
                assert!(
                    blurred > 0. && blurred < 40. * 30.,
                    "the layer draws whole: {blurred}"
                );
            }
            let drawn = (renderer.committed != committed).then(|| {
                let frame = &renderer.frame;
                (frame.damage.clone(), frame.instances.len())
            });
            let pixels = read(&renderer);
            let (_, full) = renderer.draw(&scene, true).unwrap().unwrap();
            send.send((step.name, drawn, pixels == full)).unwrap();
            // The next rebuild draws again where a patch drew, though its scene draws as before.
            if step.name == "layer opacity" {
                scene.patch(7, 0.25, None, [None; 2]);
                renderer.draw(&scene, false);
                scene.drawn();
                let drawn = Some((renderer.frame.damage.clone(), 0));
                let pixels = read(&renderer);
                let (_, full) = renderer.draw(&scene, true).unwrap().unwrap();
                send.send(("patched", drawn, pixels == full)).unwrap();
            }
        }
        super::super::threads::end_thread();
    };
    super::super::threads::spawn_render(job, || {});
    for _ in 0..=steps.len() {
        let (name, drawn, same) = receive.recv().unwrap();
        assert!(same, "{name}: the frame matches a full frame");
        let expect: Option<(&[[i32; 4]], usize)> = match name {
            "first" => Some((&[], 11)),
            "same" => None,
            "box color" => Some((&[[149, 9, 42, 42]], 2)),
            "moved box" => Some((&[[119, 9, 72, 46]], 2)),
            // The layer draws its boxes again, the window its background and the bar over it.
            "layer content" => Some((&[[19, 19, 32, 32]], 4)),
            // The layer keeps its pixels.
            "layer opacity" => Some((&[[19, 19, 32, 52]], 2)),
            "patched" => Some((&[[19, 19, 32, 52]], 0)),
            // The patched layer draws again, and joins the image in one region.
            "image" => Some((&[[19, 19, 58, 82]], 3)),
            // The layer turns into the backdrop, which reads all of its region again.
            "turned layer content" => Some((&[[96, 64, 60, 44]], 2)),
            "below a backdrop" => Some((&[[96, 13, 65, 91]], 2)),
            // The layer moves with its pixels.
            "moved layer" => Some((&[[19, 19, 35, 55]], 2)),
            // The layer draws only the part that reaches the region.
            "part of a shadowed layer" => Some((&[[181, 101, 19, 19]], 3)),
            _ => unreachable!(),
        };
        assert_eq!(
            drawn.as_ref().map(|(damage, drawn)| (&damage[..], *drawn)),
            expect,
            "{name}"
        );
    }
}

#[test]
fn returned_drawables_draw_what_changed_since_their_frame() {
    use crate::{
        Clip,
        gpu::scene::Scene,
        gpu::{BOX, Instance, rect},
    };
    use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLCommandQueue};
    let (send, receive) = std::sync::mpsc::channel();
    let job = move || {
        let size = objc2_core_foundation::CGSize::new(200., 120.);
        let [mut renderer, mut reference] = [(), ()].map(|_| {
            let mut renderer = super::Renderer::new(false).unwrap();
            renderer.set_drawable_size(size);
            renderer
        });
        let mask = Clip::from(rect([0., 0., 200., 120.]));
        let quad = |bounds: [f32; 4], color: [f32; 4]| {
            let mut instance = Instance::new(BOX, rect(bounds), mask);
            instance.color = color;
            instance
        };
        let mut scene = Scene::default();
        let (mut uncopied, mut widest) = (0, 0);
        for frame in 0..12 {
            scene.clear();
            scene.push(quad([0., 0., 200., 120.], [0.9, 0.9, 0.85, 1.]));
            let x = 10. + 12. * frame as f32;
            scene.push(quad([x, 10., 10., 10.], [0.2, 0.4, 0.8, 1.]));
            let shade = (frame / 3) as f32 / 4.;
            scene.push(quad([150., 80., 20., 20.], [shade, 0.5, 0.2, 1.]));
            scene.diff(Some(&[]));
            renderer.draw(&scene, false);
            scene.drawn();
            let damaged = !renderer.frame.damage.is_empty();
            // A drawable from frames ago draws the steps of the box since then.
            // One frame moves the box 12px, so its own region is 24px wide. A drawable comes back after 3
            // frames at most, so a region never holds more than 3 moves.
            uncopied += u32::from(renderer.frame.damage.iter().any(|region| region[2] > 24));
            widest =
                (renderer.frame.damage.iter()).fold(widest, |widest, region| widest.max(region[2]));
            let last = renderer.last.clone().unwrap();
            let commands = renderer.queue.commandBuffer().unwrap();
            let buffer = renderer.read_back(&commands, &last, [200, 120]).unwrap();
            commands.commit();
            commands.waitUntilCompleted();
            // SAFETY: the GPU finished writing the shared buffer.
            let mut pixels = unsafe {
                std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), buffer.length())
            }
            .to_vec();
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            let (_, full) = reference.draw(&scene, true).unwrap().unwrap();
            send.send((frame, damaged, pixels == full)).unwrap();
        }
        send.send((u32::MAX, uncopied > 4 && widest <= 36, true))
            .unwrap();
        super::super::threads::end_thread();
    };
    super::super::threads::spawn_render(job, || {});
    for _ in 0..=12 {
        let (frame, damaged, same) = receive.recv().unwrap();
        assert!(same, "frame {frame} matches a full frame");
        assert!(damaged || frame == 0, "frame {frame} draws its damage");
    }
}

#[test]
fn scrolled_content_copies_the_last_frame_moved() {
    use crate::{
        Clip,
        gpu::scene::{EffectState, Scene, effect},
        gpu::{BOX, Instance, rect},
        point,
    };
    use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLCommandQueue};
    let (send, receive) = std::sync::mpsc::channel();
    // Scroll offsets in device pixels, and a change: 1 and 2 recolor a row (2 one under the bar), 3 the bar. A
    // part of a pixel can't be copied, and a change inside the content draws it.
    let steps: [(f32, u8); 13] = [
        (0., 0),
        (6., 0),
        (12., 0),
        (30., 0),
        (30.5, 0),
        (40., 0),
        (52., 1),
        (46., 0),
        (20., 0),
        (20., 2),
        (14., 0),
        (14., 3),
        (8., 0),
    ];
    let job = move || {
        let size = objc2_core_foundation::CGSize::new(200., 120.);
        let [mut renderer, mut reference] = [(), ()].map(|_| {
            let mut renderer = super::Renderer::new(false).unwrap();
            renderer.set_drawable_size(size);
            renderer
        });
        let window = Clip::from(rect([0., 0., 200., 120.]));
        let viewport = Clip::from(rect([10., 10., 180., 100.]));
        let quad = |bounds: [f32; 4], clip: Clip<f32>, color: [f32; 4]| {
            let mut instance = Instance::new(BOX, rect(bounds), clip);
            instance.color = color;
            instance
        };
        let read = |renderer: &super::Renderer| {
            let last = renderer.last.clone().unwrap();
            let commands = renderer.queue.commandBuffer().unwrap();
            let buffer = renderer.read_back(&commands, &last, [200, 120]).unwrap();
            commands.commit();
            commands.waitUntilCompleted();
            // SAFETY: the GPU finished writing the shared buffer.
            let mut pixels = unsafe {
                std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), buffer.length())
            }
            .to_vec();
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            pixels
        };
        let mut scene = Scene::default();
        for (offset, changed) in steps {
            scene.clear();
            scene.push(quad([0., 0., 200., 120.], window, [0.9, 0.9, 0.85, 1.]));
            scene.begin_scroll(7, viewport, point(0., offset), false);
            for row in 0..12 {
                let y = 14. + 22. * row as f32 - offset;
                let shade = row as f32 / 12.;
                let color = match (changed, row) {
                    (1, 4) | (2, 1) => [0.9, 0.1, 0.1, 1.],
                    _ => [shade, 0.4, 1. - shade, 1.],
                };
                // A composited opacity animation, which patch frames draw without a new diff.
                if row == 3 {
                    let state = EffectState {
                        opacity: 0.8,
                        ..effect(viewport)
                    };
                    scene.push_effect(state, Some(9));
                }
                scene.push(quad([20., y, 120., 16.], viewport, color));
                if row == 3 {
                    scene.pop_effect();
                }
                scene.push(quad(
                    [150., y + 4., 30., 8.],
                    viewport,
                    [0.2, 0.2, 0.2, 0.5],
                ));
            }
            // A bar that stays over the top of the viewport, painted inside it as a fixed box is.
            let bar = match changed {
                3 => [0.6, 0.2, 0.5, 0.7],
                _ => [0.1, 0.5, 0.3, 0.8],
            };
            scene.pause_cuts(0);
            scene.push(quad([0., 0., 200., 24.], window, bar));
            scene.resume_cuts();
            // Content after the bar, as a scroll bar is.
            scene.push(quad(
                [184., 60. - offset, 6., 40.],
                viewport,
                [0.3, 0.3, 0.3, 0.6],
            ));
            scene.end_scroll();
            scene.diff(Some(&[]));
            let shifted = scene.shift;
            renderer.draw(&scene, false);
            scene.drawn();
            let kept = renderer.frame.keeps_under_now();
            let area: i32 = renderer
                .frame
                .damage
                .iter()
                .map(|region| region[2] * region[3])
                .sum();
            let pixels = read(&renderer);
            let (_, full) = reference.draw(&scene, true).unwrap().unwrap();
            // A patch frame after the scroll must not copy the scroll again.
            assert!(scene.patch(9, 0.4, None, [None; 2]));
            renderer.draw(&scene, false);
            scene.drawn();
            let patched = read(&renderer);
            let (_, patched_full) = reference.draw(&scene, true).unwrap().unwrap();
            send.send((
                offset,
                shifted,
                area,
                pixels == full,
                kept,
                patched == patched_full,
            ))
            .unwrap();
        }
        super::super::threads::end_thread();
    };
    super::super::threads::spawn_render(job, || {});
    let mut last = 0.;
    for (at, (offset, changed)) in steps.into_iter().enumerate() {
        let (got, shifted, area, same, kept, patched) = receive.recv().unwrap();
        assert_eq!(got, offset);
        assert!(
            same,
            "offset {offset}, change {changed}: the frame matches a full frame"
        );
        assert!(
            patched,
            "offset {offset}, change {changed}: the patch frame matches a full frame"
        );
        // The first scroll keeps the window below the bar, and later frames keep it up to date.
        assert_eq!(kept, at > 0, "offset {offset}, change {changed}");
        let by = offset - last;
        let copies = by != 0. && by.fract() == 0. && last.fract() == 0.;
        assert_eq!(
            shifted.map(|(_, by)| by),
            copies.then_some([0, by as i32]),
            "offset {offset}"
        );
        if copies && changed == 0 {
            // The edge, and the bar where it is and where the copy put it.
            assert!(
                area <= 200 * (24 + 2) * 2 + 182 * (by.abs() as i32 + 2),
                "offset {offset}: {area}"
            );
        }
        last = offset;
    }
}
