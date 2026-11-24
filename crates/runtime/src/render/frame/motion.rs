use super::super::effects::{BLURS, COMPOSITED_PROPS, TRANSFORM_PROPS, filter, pseudo_key};
use super::super::{CompositedTransform, ViewState, effects};
use super::visibility::{clips, in_view};
use crate::css;
use crate::css::properties::Prop;
use renderer::{App, Window};
use std::rc::Rc;

/// Whether an animated pair of pseudo-element styles differs from its base pair only in opacity.
pub(crate) fn fades_only(
    base: &[css::properties::Style; 2],
    pair: &[css::properties::Style; 2],
) -> bool {
    let rest = |(prop, _): &(Prop, &Rc<str>)| *prop != Prop::OPACITY;
    base.iter()
        .zip(pair.iter())
        .all(|(a, b)| a.iter().filter(rest).eq(b.iter().filter(rest)))
}

/// Whether two styles differ only in what patches change: opacity, transforms and filters that are one `blur()`.
pub(crate) fn composites(a: &css::properties::Style, b: &css::properties::Style) -> bool {
    let rest = |(prop, _): &(Prop, &Rc<str>)| {
        *prop != Prop::OPACITY
            && !TRANSFORM_PROPS.contains(prop)
            && !BLURS.as_flattened().contains(prop)
    };
    // A clip path keeps a filter in a layer of its own, which patches don't reach.
    let clipped = [Prop::CLIP_PATH, Prop::WEBKIT_CLIP_PATH]
        .into_iter()
        .any(|prop| a.get(prop).is_some_and(|value| value != "none"));
    a.iter().filter(rest).eq(b.iter().filter(rest))
        && [(0, clipped), (1, false)]
            .into_iter()
            .all(|(which, clipped)| {
                BLURS[which].iter().all(|&prop| a.get(prop) == b.get(prop))
                    || !clipped
                        && [a, b]
                            .into_iter()
                            .all(|style| effects::filter::lone_blur(filter(style, which)).is_some())
            })
}

pub(crate) fn animate(
    view: &Rc<ViewState>,
    dom: &mut crate::render::PageState,
    next: Option<std::time::Instant>,
    now: std::time::Instant,
    composite: bool,
    window: &mut Window,
    cx: &mut App,
) {
    match next {
        Some(at) if at <= now + std::time::Duration::from_millis(8) => {
            dom.styles.wake = None;
            // Paint and composite both ask for the next frame, and two callbacks would each ask again.
            let queued = view.motion_frame.replace(Some(composite));
            if let Some(queued) = queued {
                view.motion_frame.set(Some(queued && composite));
                return;
            }
            let view = view.clone();
            window.next_frame(move |window, cx| match view.motion_frame.take() {
                Some(true) => self::composite(view, window, cx),
                _ => window.refresh(),
            });
        }
        Some(at) => {
            if dom
                .styles
                .wake
                .as_ref()
                .is_none_or(|(old, _)| *old <= now || at < *old)
            {
                let delay = at.saturating_duration_since(now);
                let window_id = window.id();
                let view = view.clone();
                let task = cx.timer(delay, move |cx| {
                    if composite {
                        cx.update_window(window_id, |window, cx| self::composite(view, window, cx));
                    } else {
                        cx.refresh_window(window_id);
                    }
                });
                dom.styles.wake = Some((at, task));
            }
        }
        None => dom.styles.wake = None,
    }
}

// Animation handlers can hide or remove a popup, so run them before its final frame is painted.
pub(crate) fn animation_events(
    view: &Rc<ViewState>,
    events: Vec<(usize, &'static str, String, f64)>,
    cx: &mut App,
) {
    let renderer = view.runtime.renderer(view.id);
    for (node, kind, name, elapsed) in events {
        let (pseudo, name) = name.split_once('\0').unwrap_or(("", &name));
        let data = if kind.starts_with("animation") {
            serde_json::json!({"animationName": name, "elapsedTime": elapsed, "pseudoElement": pseudo})
        } else {
            serde_json::json!({"propertyName": name, "elapsedTime": elapsed, "pseudoElement": pseudo})
        };
        if let Some(renderer) = &renderer
            && let Err(error) = renderer.eval(
                &format!("__appsDispatch('{kind}',[{node}],{data})"),
                "apps:animation",
            )
        {
            eprintln!("apps: {error}");
        }
    }
    view.runtime.drive(cx);
}

pub(crate) fn composite(view: Rc<ViewState>, window: &mut Window, cx: &mut App) {
    if window.will_draw() {
        return;
    }
    let started = std::time::Instant::now();
    let size = window.viewport_size();
    let viewport = [f32::from(size.width), f32::from(size.height)];
    let Ok(mut host) = view.host.try_borrow_mut() else {
        return window.refresh();
    };
    let state = &mut *host;
    let Some(page) = state.pages.get_mut(&view.id) else {
        return window.refresh();
    };
    let (doc, dom) = (&page.doc, &mut page.state);
    let Some((.., styles)) = dom.layout.cull.seen.clone().filter(|(latest, focus, _)| {
        !page.dirty
            && !dom.dirty
            && (*latest, *focus) == (doc.nodes.latest(), dom.interaction.focus)
            && state.viewports.get(&view.id) == Some(&viewport)
            && view.hud.borrow().is_none()
    }) else {
        return window.refresh();
    };
    let mut events = Vec::new();
    let now = std::time::Instant::now();
    let bounds = &dom.geometry.mirror;
    let shown = |id| in_view(bounds, viewport, id, clips(doc, &styles, id));
    let side = |id| {
        bounds
            .get(&id)
            .map_or(f32::INFINITY, |rect| rect[2].min(rect[3]))
    };
    let crate::style::Frame {
        animated,
        next,
        custom,
        pseudos,
        backdrops,
        ..
    } = dom.styles.motion.update(
        doc,
        &styles,
        &dom.layout.cull.visible,
        &shown,
        &side,
        now,
        false,
        &mut events,
    );
    *view.parked.borrow_mut() = dom
        .styles
        .motion
        .parked()
        .iter()
        .map(|&id| (id, clips(doc, &styles, id).collect()))
        .collect();
    let composited_transforms = view.composited_transforms.borrow();
    let mut changes = Vec::new();
    let eligible = custom.is_empty()
        && pseudos.iter().all(|(id, pair)| {
            styles
                .pseudos
                .get(id)
                .is_some_and(|base| fades_only(base, pair))
        })
        && backdrops.is_empty()
        && animated.len() == dom.styles.animating.len()
        && animated.iter().all(|(id, style, layout)| {
            let Some(old) = dom.styles.animating.get(id) else {
                return false;
            };
            if Rc::ptr_eq(old, style) {
                return true;
            }
            let turns = TRANSFORM_PROPS
                .iter()
                .any(|&prop| old.get(prop) != style.get(prop));
            let matrix = match composited_transforms.get(id).filter(|_| turns) {
                Some((bounds, perspective)) => Some(
                    crate::css::value::transform::transform(
                        style,
                        &styles.lengths[*id],
                        *bounds,
                        *perspective,
                    )
                    .unwrap_or(crate::css::value::transform::IDENTITY),
                ),
                None if turns => return false,
                None => None,
            };
            let blurs = [0, 1].map(|which| {
                let value = filter(style, which);
                (filter(old, which) != value)
                    .then(|| effects::filter::lone_blur(value))
                    .flatten()
                    .map(|args| match args {
                        "" => 0.,
                        args => styles.lengths[*id].resolve(args, 0.).unwrap_or(0.),
                    })
            });
            changes.push((*id, effects::opacity(style), matrix, blurs));
            !layout && composites(old, style)
        });
    let renew = now + std::time::Duration::from_millis(1750);
    let mut fresh = false;
    let promoted = eligible
        && match view.promote.get() {
            Some((at, on)) if now < at => on,
            Some(_) => {
                fresh = true;
                let mut looped = true;
                let mut shared = rustc_hash::FxHashMap::default();
                // Hands the timeline of one layer to Core Animation: an element (slot 0), or its `::before` or
                // `::after`.
                let mut promote =
                    |key: u64,
                     id: &usize,
                     slot: u8,
                     base: &css::properties::Style,
                     transformed: Option<&CompositedTransform>| {
                        let origin = transformed
                            .filter(|(_, perspective)| perspective.is_none())
                            .map(|(bounds, ..)| bounds.origin);
                        let mut timing = (1. / 120., 240, 1., now, None);
                        match dom.styles.motion.period(*id, slot, &styles, now) {
                            Some((period, phase, left, shape)) => {
                                let count = (period * 120.).round().max(2.);
                                let step = period / count;
                                timing = (step, count as u32, left, now, None);
                                if let Some(shape) = shape.filter(|_| left >= 2. - phase / period)
                                    && let Some(start) =
                                        now.checked_sub(std::time::Duration::from_secs_f64(phase))
                                {
                                    let values = COMPOSITED_PROPS.map(|prop| base.get(prop));
                                    let units = &styles.lengths[*id];
                                    let size =
                                        transformed.map(|(bounds, perspective)| match origin {
                                            Some(_) => format!("{:?}", bounds.size),
                                            None => format!("{bounds:?}{perspective:?}"),
                                        });
                                    let shape_key =
                                        format!("{slot}{shape}{values:?}{units:?}{size:?}");
                                    timing = (
                                        step,
                                        count as u32,
                                        left + phase / period,
                                        start,
                                        Some(shape_key),
                                    );
                                }
                            }
                            None => looped = false,
                        }
                        let (step, count, repeat, start, shape_key) = timing;
                        let first = match shape_key {
                            Some(_) => {
                                start + std::time::Duration::from_secs_f64(step * f64::from(count))
                            }
                            None => start,
                        };
                        let local = shape_key.is_some() && origin.is_some();
                        let sample = || {
                            let times: Vec<_> = (0..=count)
                                .map(|k| {
                                    first + std::time::Duration::from_secs_f64(f64::from(k) * step)
                                })
                                .collect();
                            let timeline = dom.styles.motion.timeline(
                                *id,
                                slot,
                                &styles,
                                &times,
                                &COMPOSITED_PROPS,
                            );
                            let opacities: Vec<f32> =
                                timeline.iter().map(effects::opacity).collect();
                            let transforms: Option<Vec<_>> =
                                transformed.map(|(bounds, perspective)| {
                                    let bounds = match local {
                                        true => {
                                            renderer::Rect::new(Default::default(), bounds.size)
                                        }
                                        false => *bounds,
                                    };
                                    timeline
                                        .iter()
                                        .map(|style| {
                                            crate::css::value::transform::transform(
                                                style,
                                                &styles.lengths[*id],
                                                bounds,
                                                *perspective,
                                            )
                                            .unwrap_or(crate::css::value::transform::IDENTITY)
                                        })
                                        .collect()
                                });
                            // Core Animation can't play blurs.
                            let still = timeline.windows(2).all(|pair| {
                                BLURS
                                    .as_flattened()
                                    .iter()
                                    .all(|&prop| pair[0].get(prop) == pair[1].get(prop))
                            });
                            Rc::new((opacities, transforms, still))
                        };
                        let mut samples = match shape_key {
                            Some(shape_key) => {
                                shared.entry(shape_key).or_insert_with(sample).clone()
                            }
                            None => sample(),
                        };
                        if let Some(origin) = origin.filter(|_| local) {
                            let [x, y] = [f32::from(origin.x), f32::from(origin.y)];
                            let (there, back) = (
                                crate::css::value::transform::translate(x, y, 0.),
                                crate::css::value::transform::translate(-x, -y, 0.),
                            );
                            let moved = samples.1.as_ref().map(
                                |list: &Vec<crate::css::value::transform::Matrix>| {
                                    list.iter()
                                        .map(|matrix| {
                                            crate::css::value::transform::multiply(
                                                &there,
                                                &crate::css::value::transform::multiply(
                                                    matrix, &back,
                                                ),
                                            )
                                        })
                                        .collect()
                                },
                            );
                            samples = Rc::new((samples.0.clone(), moved, samples.2));
                        }
                        samples.2
                            && (window.promote(
                                key,
                                start,
                                step as f32,
                                repeat as f32,
                                &samples.0,
                                samples.1.as_deref(),
                            ) || samples.1.is_none() && !shown(*id))
                    };
                let on = animated.iter().all(|(id, style, _)| {
                    let base = &styles.values[*id];
                    // A node listed only for its pseudo-elements has no animation of its own.
                    Rc::ptr_eq(style, base)
                        && dom.styles.motion.period(*id, 0, &styles, now).is_none()
                        || promote(*id as u64, id, 0, base, composited_transforms.get(id))
                }) && pseudos.iter().all(|(id, _)| {
                    styles.pseudos.get(id).is_some_and(|pair| {
                        pair.iter().zip(1..).all(|(base, slot)| {
                            base.get(Prop::CONTENT)
                                .is_none_or(|content| matches!(content, "none" | "normal"))
                                || promote(pseudo_key(*id, slot), id, slot, base, None)
                        })
                    })
                });
                // Demote all layers if any promotion fails, or previously promoted layers stop with their
                // timeline.
                if !on {
                    window.demote();
                }
                let until = match looped {
                    true => now + std::time::Duration::from_secs(86400),
                    false => renew,
                };
                view.promote.set(on.then_some((until, true)));
                on
            }
            None => false,
        };
    let mut flat = view.flat.borrow_mut();
    let patched = eligible
        && (promoted
            || changes.into_iter().all(|(id, opacity, matrix, blurs)| {
                window.patch_effect(id as u64, opacity, matrix, blurs) || !shown(id) || {
                    flat.insert(id);
                    false
                }
            }) && pseudos.iter().all(|(id, pair)| {
                pair.iter().zip(1..).all(|(style, slot)| {
                    // Only a pseudo-element with content has a box and a layer.
                    style
                        .get(Prop::CONTENT)
                        .is_none_or(|content| matches!(content, "none" | "normal"))
                        || window.patch_effect(
                            pseudo_key(*id, slot),
                            effects::opacity(style),
                            None,
                            [None; 2],
                        )
                        || !shown(*id)
                })
            }));
    if patched
        && composited_transforms
            .keys()
            .any(|id| dom.styles.animating.contains_key(id))
    {
        dom.geometry.mirror.set_moved();
    }
    drop((flat, composited_transforms));
    if patched {
        dom.styles
            .animating
            .extend(animated.into_iter().map(|(id, style, _)| (id, style)));
        dom.styles.animating_pseudos.extend(pseudos);
    }
    let next = match view.promote.get() {
        Some((until, true)) if promoted => {
            let event = dom
                .styles
                .animating
                .keys()
                .filter_map(|&id| dom.styles.motion.next_event(id, &styles, now))
                .min()
                .map(|event| event.max(now + std::time::Duration::from_millis(16)));
            Some(event.map_or(until, |event| event.min(until)))
        }
        _ => next.filter(|_| patched),
    };
    animate(&view, dom, next, now, true, window, cx);
    drop(host);
    if patched && !promoted && crate::native::devtools::profiling() {
        crate::native::devtools::frame(view.id, started);
    }
    if !patched || (!promoted || fresh) && !window.present() {
        window.refresh();
    }
    // Handlers can change the page, and `Desktop::drive` can't refresh the window it runs in, so refresh here.
    if !events.is_empty() {
        animation_events(&view, events, cx);
        if view
            .host
            .try_borrow()
            .is_ok_and(|host| host.pages.get(&view.id).is_some_and(|page| page.dirty))
        {
            window.refresh();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::visibility::drop_hidden_images;
    use super::*;
    use std::sync::Arc;

    #[test]
    fn pseudo_layers_patch_only_opacity_changes() {
        let style = |props: &[(&str, &str)]| {
            let mut style = css::properties::Style::default();
            for (name, value) in props {
                style.insert(Prop::find(name).unwrap(), Rc::from(*value));
            }
            style
        };
        let base = [style(&[("content", "''"), ("opacity", "1")]), style(&[])];
        let faded = [style(&[("content", "''"), ("opacity", "0.4")]), style(&[])];
        let painted = [
            style(&[("content", "''"), ("opacity", "0.4"), ("color", "red")]),
            style(&[]),
        ];
        assert!(fades_only(&base, &faded));
        assert!(!fades_only(&base, &painted));
        assert_ne!(pseudo_key(7, 1), pseudo_key(7, 2));
        assert_ne!(pseudo_key(7, 1), 7);
    }

    #[test]
    fn blur_changes_patch_layers() {
        let style = |props: &[(&str, &str)]| {
            let mut style = css::properties::Style::default();
            for (name, value) in props {
                style.insert(Prop::find(name).unwrap(), Rc::from(*value));
            }
            style
        };
        let base = style(&[
            ("filter", "blur(8px)"),
            ("backdrop-filter", "blur(0)"),
            ("opacity", "0"),
        ]);
        let blurs = |filter, backdrop| {
            style(&[
                ("filter", filter),
                ("backdrop-filter", backdrop),
                ("opacity", "0.5"),
            ])
        };
        assert!(composites(&base, &blurs("blur(2.5px)", "blur(3px)")));
        assert!(composites(&base, &blurs("blur(0)", "blur(6px)")));
        assert!(!composites(
            &base,
            &blurs("blur(2px) grayscale(1)", "blur(3px)")
        ));
        assert!(!composites(&base, &blurs("blur(2px)", "saturate(2)")));
        assert!(!composites(&base, &blurs("none", "blur(3px)")));
        let mut clipped = base.clone();
        clipped.insert(Prop::CLIP_PATH, Rc::from("inset(2px)"));
        let mut moved = blurs("blur(2px)", "blur(0)");
        moved.insert(Prop::CLIP_PATH, Rc::from("inset(2px)"));
        assert!(!composites(&clipped, &moved));
        assert_eq!(effects::filter::lone_blur(Some("BLUR( 4px )")), Some("4px"));
        assert_eq!(effects::filter::lone_blur(Some("blur()")), Some(""));
        assert_eq!(
            effects::filter::lone_blur(Some("blur(1px) blur(2px)")),
            None
        );
    }

    #[test]
    fn exit_transitions_keep_images_until_they_end() {
        let mut doc = crate::dom::Document::parse(
            "<style>div{transition:display 1s allow-discrete}div.off{display:none}span{display:block;mask-image:url(m.svg);background-image:url(b.png)}</style><div><img src=a.svg width=200 height=200><span></span></div>",
        );
        let mut dom = crate::render::PageState::default();
        let [div, img, span] = ["div", "img", "span"]
            .map(|tag| doc.nodes.iter().position(|node| node.tag == tag).unwrap());
        let image = |size: u32| {
            Arc::new(renderer::RenderImage::new(
                size,
                size,
                vec![0; (size * size * 4) as usize],
            ))
        };
        let decoded = image(4);
        dom.rasters.svgs.insert(img, ("a.svg".into(), image(200)));
        dom.rasters
            .masks
            .insert((span, 0), ("m.svg".into(), Some(image(8))));
        dom.rasters
            .backgrounds
            .insert((span, 0), ("b.png".into(), decoded.clone()));
        dom.rasters.bitmaps.insert("b.png".into(), decoded);
        let ids = |dom: &crate::render::PageState| {
            [
                dom.rasters.svgs[&img].1.id,
                dom.rasters.masks[&(span, 0)].1.as_ref().unwrap().id,
                dom.rasters.backgrounds[&(span, 0)].1.id,
            ]
        };
        let mut kept = ids(&dom);
        let start = std::time::Instant::now();
        let mut released = Vec::new();
        let mut frame =
            |doc: &mut crate::dom::Document, dom: &mut crate::render::PageState, seconds: f64| {
                let styles = dom.restyle(doc, [800., 600.]);
                let mut visible = vec![false; doc.nodes.len()];
                for id in 0..doc.nodes.len() {
                    visible[id] = doc.nodes[id].parent.is_none_or(|parent| visible[parent])
                        && styles.values[id].get(Prop::DISPLAY) != Some("none");
                }
                let at = start + std::time::Duration::from_secs_f64(seconds);
                let frame = dom.styles.motion.update(
                    doc,
                    &styles,
                    &visible,
                    &|_| true,
                    &|_| f32::INFINITY,
                    at,
                    false,
                    &mut Vec::new(),
                );
                let visible = frame.painted.unwrap_or(visible);
                drop_hidden_images(dom, &visible, &styles, |image| released.push(image.id));
                visible[span]
            };
        assert!(frame(&mut doc, &mut dom, 0.));
        doc.nodes[div].attrs.insert("class".into(), "off".into());
        dom.styles.computed = None;
        for seconds in [0.1, 0.4, 0.7, 1.] {
            assert!(frame(&mut doc, &mut dom, seconds), "{seconds}");
            assert_eq!(ids(&dom), kept, "{seconds}");
        }
        assert!(!frame(&mut doc, &mut dom, 1.2));
        frame(&mut doc, &mut dom, 1.3);
        assert!(
            dom.rasters.svgs.is_empty()
                && dom.rasters.masks.is_empty()
                && dom.rasters.backgrounds.is_empty()
                && dom.rasters.bitmaps.is_empty()
        );
        released.sort_unstable();
        kept.sort_unstable();
        assert_eq!(released, kept);
    }
}
