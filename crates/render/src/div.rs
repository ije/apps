use crate::{
    AnyElement, App, Bounds, ClickEvent, Element, ElementId, Hitbox, HitboxBehavior, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Overflow, ParentElement,
    Pixels, Point, ScrollWheelEvent, Style, Styled, Visibility, Window, WindowControlArea,
    input::DispatchPhase, point, px, quad,
};
use std::{cell::RefCell, rc::Rc};

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
            let (state, hitbox) = (handle.0.clone(), hitbox.clone());
            let (overflow, restrict) = (style.overflow, style.restrict_scroll_to_axis);
            let line_height = window.line_height();
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, _| {
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
                let mut state = state.borrow_mut();
                let old = state.offset;
                state.offset.x = (state.offset.x + x).clamp(-state.max_offset.x, px(0.));
                state.offset.y = (state.offset.y + y).clamp(-state.max_offset.y, px(0.));
                if state.offset != old {
                    drop(state);
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
        let ids = window.with_text_style(Some(&style.text), |window| {
            children
                .iter_mut()
                .map(|child| child.request_layout(window))
                .collect::<Vec<_>>()
        });
        (window.request_layout(&self.style, ids.iter().copied()), ids)
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
                    Some(handle) => handle.clamp(style, bounds, content, window.scale_factor()),
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
            let radii = style.corner_radii.clamp_radii_for_quad_size(bounds.size);
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

    pub fn set_offset(&self, offset: Point<Pixels>) {
        self.0.borrow_mut().offset = offset;
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
        // Padding was snapped to device pixels for layout, so it is snapped the same way here.
        let snap = |length: crate::Length| match length {
            crate::Length::Px(value) => {
                px(crate::geometry::round_to_device_pixel(value.0, scale) / scale)
            }
            crate::Length::Fraction(fraction) => bounds.size.width * fraction,
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
