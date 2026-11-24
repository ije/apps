use super::super::{BUTTONS, ViewState, queue_layout};
use super::visibility::in_view;
use renderer::Event;
use std::rc::Rc;

// Presses go to the page in the bubble phase, after drag areas and scrollbars could stop them.
pub(crate) fn events(data: Rc<ViewState>) -> renderer::Node {
    renderer::canvas(|_, _| (), move |_, _, window| {
        window.paint_volatile();
        let parked = data.parked.take();
        let size = window.viewport_size();
        if !parked.is_empty()
            && data.host.try_borrow().is_ok_and(|host| {
                host.page_state(data.id).is_some_and(|dom| {
                    parked.iter().any(|(id, clips)| {
                        in_view(&dom.geometry.mirror, [f32::from(size.width), f32::from(size.height)], *id, clips.iter().copied())
                    })
                })
            })
        {
            window.request_animation_frame();
        }
        if data.inspect.get().pick {
            let view = data.clone();
            // The picker of the developer tools takes the pointer from the page.
            window.on_capture(move |event, _, cx| {
                let (at, click) = match *event {
                    Event::PointerMove { at, .. } => (at, false),
                    Event::PointerDown { at, .. } => (at, true),
                    Event::PointerUp { .. } => return cx.stop_event(),
                    _ => return,
                };
                view.host.borrow_mut().command(
                    crate::host::link::Target::Window(view.id),
                    serde_json::json!({"op":"window","args":["inspect",view.id,f32::from(at.x),f32::from(at.y),click]}),
                );
                let runtime = view.runtime.clone();
                cx.defer(move |cx| runtime.drive(cx));
                cx.stop_event();
            });
        }
        let view = data.clone();
        window.on_capture(move |event, window, cx| {
            let (Event::PointerLeave { at: position } | Event::PointerMove { at: position, .. } | Event::PointerUp { at: position, .. }) = *event else {
                return;
            };
            let (kind, button) = match *event {
                Event::PointerLeave { .. } => {
                    let [x, y] = view.screen(window, position);
                    view.runtime.dispatch(view.id, "pointerexit", &[], serde_json::json!({"clientX":f32::from(position.x),"clientY":f32::from(position.y),"screenX":x,"screenY":y}), Some(window), cx);
                    return;
                }
                Event::PointerMove { held, .. } => {
                    if held.is_none() {
                        view.buttons.set(0);
                    }
                    ("pointermove", None)
                }
                Event::PointerUp { button, .. } => {
                    let button = button as u8;
                    view.buttons.set(view.buttons.get() & !BUTTONS[usize::from(button)]);
                    ("pointerup", Some(button))
                }
                _ => return,
            };
            let mut fields = view.pointer(window, position, event.modifiers().unwrap_or_default());
            if let Some(button) = button {
                fields["button"] = button.into();
            }
            view.runtime.dispatch(view.id, kind, &[], fields, Some(window), cx);
        });
        let view = data.clone();
        window.on_bubble(move |event, window, cx| {
            if let Event::PointerDown { at, button, modifiers, clicks, .. } = *event {
                let button = button as u8;
                view.buttons.set(view.buttons.get() | BUTTONS[usize::from(button)]);
                let mut fields = view.pointer(window, at, modifiers);
                (fields["button"], fields["detail"]) = (button.into(), clicks.into());
                view.runtime.dispatch(view.id, "pointerdown", &[], fields, Some(window), cx);
            }
        });
        let view = data.clone();
        window.on_capture(move |event, window, cx| {
            let Event::Wheel { at: position, delta, lines, phase, modifiers } = *event else {
                return;
            };
            if let (renderer::WheelPhase::Began | renderer::WheelPhase::Ended, Ok(host)) =
                (phase, view.host.try_borrow())
                && let Some(link) = &host.link
            {
                let kind = if phase == renderer::WheelPhase::Began {
                    "gestureScrollBegin"
                } else {
                    "gestureScrollEnd"
                };
                link.event(
                    "input-event",
                    view.id,
                    Some(serde_json::json!({"type": kind})),
                    "apps:input",
                );
            }
            queue_layout(view.clone(), window);
            let (mut target, mut wheel, mut terminal) = (None, false, None);
            if let Ok(host) = view.host.try_borrow() {
                let viewport = *host.viewports.get(&view.id).unwrap_or(&[800., 600.]);
                if let Some(page) = host.pages.get(&view.id) {
                    let state = &page.state;
                    let seen = state.layout.cull.seen.as_ref().map(|seen| seen.2.clone()).or_else(|| state.styles.computed.clone());
                    target = seen.as_ref().and_then(|computed| {
                        let at = [position.x, position.y].map(f32::from);
                        crate::render::hit::hit_test(&page.doc, state, computed, viewport, at, true, false).as_u64()
                    })
                    .map(|id| id as usize);
                    if let Some(target) = target {
                        let mut chain = Vec::new();
                        let mut node = Some(target);
                        while let Some(id) = node {
                            if terminal.is_none() && page.doc.nodes[id].tag == "terminal" {
                                terminal = Some((id, state.media.terminals.clone()));
                            }
                            chain.extend(state.geometry.scroll.get(&id).cloned());
                            node = seen.as_ref().and_then(|computed| state.tree.flat_parent(&page.doc, &computed.children, id));
                        }
                        window.scroll_only(chain);
                    }
                    wheel = state.wheel;
                }
            }
            if wheel {
                let delta = if lines { delta.map(|value| value * 16.) } else { delta };
                let mut fields = view.pointer(window, position, modifiers);
                (fields["deltaX"], fields["deltaY"]) = ((-delta.x).into(), (-delta.y).into());
                if !view.runtime.dispatch(view.id, "wheel", target.as_slice(), fields, Some(window), cx) {
                    cx.stop_event();
                    return;
                }
            }
            // A terminal scrolls its own rows, or sends the wheel to its program.
            if let Some((node, terminals)) = terminal
                && let Some(terminal) = terminals.borrow_mut().shown(node)
            {
                let mods = u32::from(modifiers.shift()) | u32::from(modifiers.ctrl()) << 1 | u32::from(modifiers.alt()) << 2 | u32::from(modifiers.command()) << 3;
                let rows = delta.y * if lines { 16. } else { 1. };
                terminal.wheel(-rows, mods, position.x.into(), position.y.into());
                cx.stop_event();
                window.refresh();
            }
        });
    })
    .absolute()
    .size_full()
}
