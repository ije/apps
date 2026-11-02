use crate::{
    AnyElement, App, Bounds, ClickEvent, Element, ElementId, Hitbox, HitboxBehavior, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Overflow, ParentElement,
    Pixels, Point, ScrollWheelEvent, Style, Styled, Visibility, Window, WindowControlArea,
    input::DispatchPhase, point, px, quad,
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

type Listener<E> = Box<dyn FnMut(&E, &mut Window, &mut App)>;

/// A box: the building block of every page.
#[derive(Default)]
pub struct Div {
    style: Style,
    children: Vec<AnyElement>,
    events: Events,
}

#[derive(Default)]
struct Events {
    id: Option<ElementId>,
    mouse_down: Vec<(MouseButton, Rc<RefCell<Listener<MouseDownEvent>>>)>,
    mouse_move: Vec<Rc<RefCell<Listener<MouseMoveEvent>>>>,
    click: Vec<Rc<RefCell<Listener<ClickEvent>>>>,
    scroll: Option<ScrollHandle>,
    control: Option<WindowControlArea>,
}

pub fn div() -> Div {
    Div::default()
}

impl Div {
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.events.id = Some(id.into());
        self
    }

    pub fn on_mouse_down(
        mut self,
        button: MouseButton,
        listener: impl FnMut(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.events
            .mouse_down
            .push((button, Rc::new(RefCell::new(Box::new(listener)))));
        self
    }

    pub fn on_mouse_move(
        mut self,
        listener: impl FnMut(&MouseMoveEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.events
            .mouse_move
            .push(Rc::new(RefCell::new(Box::new(listener))));
        self
    }

    /// A left button press and release on this box. Needs an [`Self::id`].
    pub fn on_click(
        mut self,
        listener: impl FnMut(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.events
            .click
            .push(Rc::new(RefCell::new(Box::new(listener))));
        self
    }

    /// Scrolls the overflow with the wheel and keeps the offset in `handle`.
    pub fn track_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.events.scroll = Some(handle.clone());
        self
    }

    pub fn window_control_area(mut self, area: WindowControlArea) -> Self {
        self.events.control = Some(area);
        self
    }
}

impl Events {
    fn needs_hitbox(&self, style: &Style) -> bool {
        style.mouse_cursor.is_some()
            || self.scroll.is_some()
            || self.control.is_some()
            || !self.mouse_down.is_empty()
            || !self.mouse_move.is_empty()
            || !self.click.is_empty()
    }

    fn paint(&self, style: &Style, hitbox: &Hitbox, window: &mut Window) {
        for (button, listener) in &self.mouse_down {
            let (button, listener, hitbox) = (*button, listener.clone(), hitbox.clone());
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble
                    && event.button == button
                    && hitbox.is_hovered(window)
                {
                    (listener.borrow_mut())(event, window, cx);
                }
            });
        }
        for listener in &self.mouse_move {
            let (listener, hitbox) = (listener.clone(), hitbox.clone());
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                    (listener.borrow_mut())(event, window, cx);
                }
            });
        }
        if let (false, Some(id)) = (self.click.is_empty(), self.id) {
            let down = hitbox.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
                if phase == DispatchPhase::Bubble
                    && event.button == MouseButton::Left
                    && down.is_hovered(window)
                {
                    window.pending_clicks.push((id, event.clone()));
                }
            });
            let (listeners, up) = (self.click.clone(), hitbox.clone());
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !up.is_hovered(window)
                {
                    return;
                }
                let Some(down) = window
                    .pending_clicks
                    .iter()
                    .find(|(pending, _)| *pending == id)
                    .map(|(_, down)| down.clone())
                else {
                    return;
                };
                let click = ClickEvent {
                    down,
                    up: event.clone(),
                };
                for listener in &listeners {
                    (listener.borrow_mut())(&click, window, cx);
                }
            });
        }
        if let Some(handle) = &self.scroll {
            let (handle, hitbox) = (handle.clone(), hitbox.clone());
            let (overflow, restrict) = (style.overflow, style.restrict_scroll_to_axis);
            let line_height = window.line_height();
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.should_handle_scroll(window) {
                    return;
                }
                let delta = event.delta.pixel_delta(line_height);
                let axis = |own: Overflow, along: Pixels, across: Pixels, other: Overflow| match own
                {
                    Overflow::Scroll if !along.is_zero() => along,
                    Overflow::Scroll if !restrict && other != Overflow::Scroll => across,
                    _ => Pixels::ZERO,
                };
                let mut x = axis(overflow.x, delta.x, delta.y, overflow.y);
                let mut y = axis(overflow.y, delta.y, delta.x, overflow.x);
                if !x.is_zero() && !y.is_zero() {
                    if x.abs() > y.abs() {
                        y = Pixels::ZERO;
                    } else {
                        x = Pixels::ZERO;
                    }
                }
                // WARNING: a wheel tick adds to where a running scroll stops, so fast ticks lose no distance.
                let wheel = !event.delta.precise();
                let from = if wheel {
                    handle.target()
                } else {
                    handle.offset()
                };
                let max = handle.max_offset();
                let to = point(
                    (from.x + x).clamp(-max.x, px(0.)),
                    (from.y + y).clamp(-max.y, px(0.)),
                );
                if to == from {
                    return;
                }
                // WARNING: like browsers, only the innermost box that can scroll moves. Outer boxes get the event at its edge.
                cx.stop_propagation();
                if wheel {
                    handle.animate_to(to, true);
                    handle.drive(window);
                } else {
                    handle.set_offset(to);
                    window.refresh();
                }
            });
        }
    }
}

impl Styled for Div {
    fn style(&mut self) -> &mut Style {
        &mut self.style
    }
}

impl ParentElement for Div {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl IntoElement for Div {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Div {
    type RequestLayoutState = Vec<LayoutId>;
    type PrepaintState = Option<Hitbox>;

    fn request_layout(&mut self, window: &mut Window) -> (LayoutId, Vec<LayoutId>) {
        let style = &self.style;
        let children = &mut self.children;
        let id = self.events.id;
        let ids = window.with_layout_scope(id, |window| {
            window.with_text_style(Some(&style.text), |window| {
                children
                    .iter_mut()
                    .map(|child| child.request_layout(window))
                    .collect::<Vec<_>>()
            })
        });
        (
            window.request_element_layout(id, &self.style, ids.iter().copied()),
            ids,
        )
    }

    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        ids: &mut Vec<LayoutId>,
        window: &mut Window,
    ) -> Option<Hitbox> {
        let content = if ids.is_empty() {
            bounds.size
        } else {
            let mut min = point(Pixels::MAX, Pixels::MAX);
            let mut max = Point::default();
            for &id in ids.iter() {
                let child = window.layout_bounds(id);
                min = min.min(child.origin);
                max = max.max(child.bottom_right());
            }
            (max - min).into()
        };
        let needs_hitbox = self.events.needs_hitbox(&self.style);
        let style = &self.style;
        let scroll = &self.events.scroll;
        let children = &mut self.children;
        window.with_text_style(Some(&style.text), |window| {
            window.with_content_mask(style.overflow_mask(bounds), |window| {
                let hitbox =
                    needs_hitbox.then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal));
                let offset = match scroll {
                    Some(handle) => {
                        handle.drive(window);
                        handle.clamp(style, bounds, content, window.scale_factor())
                    }
                    None => Point::default(),
                };
                if style.display != crate::Display::None {
                    window.with_element_offset(offset, |window| {
                        for child in children.iter_mut() {
                            child.prepaint(window);
                        }
                    });
                }
                hitbox
            })
        })
    }

    fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        _: &mut Vec<LayoutId>,
        hitbox: &mut Option<Hitbox>,
        window: &mut Window,
    ) {
        if self.style.visibility == Visibility::Hidden {
            return;
        }
        window.with_element_opacity(self.style.opacity, |window| {
            let style = &self.style;
            let radii = style.corner_radii.resolve(bounds.size);
            window.paint_drop_shadows(bounds, radii, &style.box_shadow);
            if let Some(background) = style.background.filter(|color| !color.is_transparent()) {
                window.paint_quad(quad(
                    bounds,
                    radii,
                    background,
                    px(0.),
                    background.alpha(0.),
                    style.border_style,
                ));
            }
            window.paint_inset_shadows(bounds, radii, &style.box_shadow);
            window.with_text_style(Some(&style.text), |window| {
                window.with_content_mask(style.overflow_mask(bounds), |window| {
                    if let Some(hitbox) = hitbox {
                        if let Some(cursor) = style.mouse_cursor {
                            window.set_cursor_style(cursor, hitbox);
                        }
                        if let Some(area) = self.events.control {
                            window.insert_window_control_hitbox(area, hitbox);
                        }
                        self.events.paint(style, hitbox, window);
                    }
                    if style.display != crate::Display::None {
                        for child in &mut self.children {
                            child.paint(window);
                        }
                    }
                })
            });
            if let Some(color) = style.border_color.filter(|color| !color.is_transparent())
                && style.border_widths.any(|width| !width.is_zero())
            {
                window.paint_quad(quad(
                    bounds,
                    radii,
                    color.alpha(0.),
                    style.border_widths,
                    color,
                    style.border_style,
                ));
            }
        });
    }
}

#[derive(Default)]
struct ScrollState {
    /// Zero or negative: how far the content moved up and left.
    offset: Point<Pixels>,
    max_offset: Point<Pixels>,
    bounds: Bounds<Pixels>,
    animation: Option<Animation>,
    /// A frame callback advances `animation`.
    driven: bool,
}

/// How long a wheel tick scrolls when the box is still.
const WHEEL: Duration = Duration::from_millis(150);

/// A smooth scroll. Each axis moves by `f(u) = s·u + (3 - 2s)·u² + (s - 2)·u³` of the distance as
/// `u` goes from 0 to 1, so it starts at `s` times the average speed and stops softly. `s` stays
/// in 0..=3, where 0 eases in and out and 3 is a cubic ease-out.
struct Animation {
    from: Point<Pixels>,
    to: Point<Pixels>,
    slope: Point<f32>,
    start: Instant,
    duration: Duration,
}

impl Animation {
    /// The offset and the speed in pixels per second at `now`.
    fn at(&self, now: Instant) -> (Point<Pixels>, Point<f32>) {
        let duration = self.duration.as_secs_f32();
        let u = (now.saturating_duration_since(self.start).as_secs_f32() / duration).min(1.);
        let axis = |from: Pixels, to: Pixels, s: f32| {
            let delta = f32::from(to - from);
            (
                from + px(delta * u * (s + u * (3. - 2. * s + u * (s - 2.)))),
                delta * (1. - u) * (s + (6. - 3. * s) * u) / duration,
            )
        };
        let (x, speed_x) = axis(self.from.x, self.to.x, self.slope.x);
        let (y, speed_y) = axis(self.from.y, self.to.y, self.slope.y);
        (point(x, y), point(speed_x, speed_y))
    }
}

/// The scroll position of a box, shared between frames.
#[derive(Clone, Default)]
pub struct ScrollHandle(Rc<RefCell<ScrollState>>);

impl std::fmt::Debug for ScrollHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ScrollHandle({:?})", self.offset())
    }
}

impl ScrollHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn offset(&self) -> Point<Pixels> {
        self.0.borrow().offset
    }

    /// Moves at once and stops a smooth scroll.
    pub fn set_offset(&self, offset: Point<Pixels>) {
        let mut state = self.0.borrow_mut();
        state.offset = offset;
        state.animation = None;
    }

    /// Where the box stops: the end of a smooth scroll, or the offset.
    pub fn target(&self) -> Point<Pixels> {
        let state = self.0.borrow();
        state
            .animation
            .as_ref()
            .map_or(state.offset, |animation| animation.to)
    }

    /// Scrolls smoothly to `offset` from the current position and speed. The box's next frame
    /// starts it.
    ///
    /// A `wheel` tick starts at full speed. It lasts 150 ms, or twice the time since the last
    /// tick but at least 50 ms, like Firefox. It never starts slower than 1.5 times its average
    /// speed, so each tick is felt. Other scrolls ease in from rest and take the square root of
    /// the distance in 60 Hz frames, up to 0.7 seconds, like Chromium.
    pub fn animate_to(&self, offset: Point<Pixels>, wheel: bool) {
        let now = Instant::now();
        let mut state = self.0.borrow_mut();
        let running = state
            .animation
            .as_ref()
            .filter(|animation| now < animation.start + animation.duration);
        let (from, speed) = running.map_or((state.offset, Point::default()), |animation| {
            animation.at(now)
        });
        let distance = f32::from((offset.x - from.x).abs().max((offset.y - from.y).abs()));
        let duration = if wheel {
            running.map_or(WHEEL, |animation| {
                ((now - animation.start) * 2).clamp(Duration::from_millis(50), WHEEL)
            })
        } else {
            Duration::from_secs_f32((distance.sqrt() / 60.).clamp(1. / 60., 0.7))
        };
        let seconds = duration.as_secs_f32();
        let slope = |delta: Pixels, speed: f32| match running {
            None if wheel => 3.,
            _ if delta.is_zero() => 0.,
            _ => (speed * seconds / f32::from(delta)).clamp(if wheel { 1.5 } else { 0. }, 3.),
        };
        let animation = (distance > 0.).then(|| Animation {
            from,
            to: offset,
            slope: point(
                slope(offset.x - from.x, speed.x),
                slope(offset.y - from.y, speed.y),
            ),
            start: now,
            duration,
        });
        state.offset = from;
        state.animation = animation;
    }

    /// Advances the smooth scroll at the start of each frame, before the view renders, so the
    /// frame lays out at the new offset.
    fn drive(&self, window: &mut Window) {
        {
            let mut state = self.0.borrow_mut();
            if state.animation.is_none() || state.driven {
                return;
            }
            state.driven = true;
        }
        let handle = self.clone();
        window.on_next_frame(move |window, _| {
            let now = Instant::now();
            let mut state = handle.0.borrow_mut();
            state.driven = false;
            let Some(animation) = &state.animation else {
                return;
            };
            let offset = if now < animation.start + animation.duration {
                animation.at(now).0
            } else {
                let to = animation.to;
                state.animation = None;
                to
            };
            if offset != state.offset {
                state.offset = offset;
                window.refresh();
            }
            drop(state);
            handle.drive(window);
        });
    }

    pub fn max_offset(&self) -> Point<Pixels> {
        self.0.borrow().max_offset
    }

    /// The box's bounds in the last frame.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.0.borrow().bounds
    }

    fn clamp(
        &self,
        style: &Style,
        bounds: Bounds<Pixels>,
        content: crate::Size<Pixels>,
        scale: f32,
    ) -> Point<Pixels> {
        // Padding was truncated to layout units, so it is truncated the same way here.
        let snap = |length: crate::Length| match length {
            crate::Length::Px(value) => px(crate::geometry::to_layout_unit(value.0, scale) / scale),
            crate::Length::Fraction(fraction) => bounds.size.width * fraction,
            crate::Length::Calc(fraction, offset) => {
                bounds.size.width * fraction
                    + px(crate::geometry::to_layout_unit(offset.0, scale) / scale)
            }
            crate::Length::Auto => Pixels::ZERO,
        };
        let padding = crate::size(
            snap(style.padding.left) + snap(style.padding.right),
            snap(style.padding.top) + snap(style.padding.bottom),
        );
        // Taffy and these sums differ after a few decimals, so the range rounds to two places.
        let round = |value: Pixels| (value * 100.).round() / 100.;
        let max = Point::from(content + padding - bounds.size)
            .map(round)
            .max(Point::default());
        let mut state = self.0.borrow_mut();
        state.offset.x = state.offset.x.clamp(-max.x, px(0.));
        state.offset.y = state.offset.y.clamp(-max.y, px(0.));
        state.max_offset = max;
        state.bounds = bounds;
        state.offset
    }
}

#[cfg(test)]
mod tests {
    use super::{Animation, ScrollHandle};
    use crate::{point, px};
    use std::time::{Duration, Instant};

    #[test]
    fn smooth_scrolls_keep_position_and_speed() {
        let (start, duration) = (Instant::now(), Duration::from_millis(200));
        let curve = |slope: f32| Animation {
            from: point(px(0.), px(0.)),
            to: point(px(0.), px(-100.)),
            slope: point(0., slope),
            start,
            duration,
        };
        for slope in [0., 1.5, 3.] {
            let animation = curve(slope);
            assert_eq!(animation.at(start).0.y, px(0.));
            assert!((animation.at(start).1.y + slope * 500.).abs() < 1e-3);
            assert!((f32::from(animation.at(start + duration).0.y) + 100.).abs() < 1e-3);
            assert_eq!(animation.at(start + duration).1.y, 0.);
            let mut last = 0.;
            for step in 1..=20 {
                let y = f32::from(animation.at(start + duration * step / 20).0.y);
                assert!(y <= last, "slope {slope} moves back at step {step}");
                last = y;
            }
        }

        let handle = ScrollHandle::new();
        handle.animate_to(point(px(0.), px(-100.)), true);
        assert_eq!(handle.target().y, px(-100.));
        assert_eq!(handle.0.borrow().animation.as_ref().unwrap().slope.y, 3.);
        handle.set_offset(point(px(0.), px(-10.)));
        assert!(handle.0.borrow().animation.is_none());

        let past = Instant::now() - Duration::from_millis(50);
        handle.0.borrow_mut().animation = Some(Animation {
            start: past,
            ..curve(0.)
        });
        handle.animate_to(point(px(0.), px(-400.)), false);
        let state = handle.0.borrow();
        let new = state.animation.as_ref().unwrap();
        let old = Animation {
            start: past,
            ..curve(0.)
        }
        .at(new.start);
        assert_eq!(state.offset, new.from);
        assert!((f32::from(new.from.y - old.0.y)).abs() < 1e-3);
        assert!((new.at(new.start).1.y - old.1.y).abs() < 1e-2);
    }
}
