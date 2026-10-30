//! Scrollbars. By default they are macOS overlay bars that show while the box scrolls or the pointer is
//! over them, then fade out. `::-webkit-scrollbar` styles give bars that always show and take layout space.

use crate::style::Prop;
use crate::style::{self, Style};
use render::{
    Bounds, Corners, DispatchPhase, Edges, Hitbox, HitboxBehavior, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, ScrollHandle, Timer, Window,
    point, prelude::*, px, quad, rgba, size, transparent_black,
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

/// How long an overlay bar stays after the last scroll, and how long it then fades.
const HOLD: Duration = Duration::from_millis(1000);
const FADE: Duration = Duration::from_millis(300);
/// AppKit's overlay `NSScroller`: bar, idle knob and hovered knob widths, regular then thin.
const OVERLAY: [[f32; 3]; 2] = [[17., 6., 11.], [15., 4., 9.]];
/// The knob's distance from the bar's edges, and its shortest length.
const INSET: f32 = 3.;
const SHORTEST: f32 = 26.;

/// A `::-webkit-scrollbar-*` box.
#[derive(Clone, Copy)]
struct Part {
    fill: Rgba,
    radius: f32,
    border: f32,
    edge: Rgba,
    /// How far `background-clip` keeps the fill inside the box.
    clip: f32,
}

impl Part {
    fn new(style: &Style, units: &style::Lengths, thickness: f32) -> Self {
        let length = |prop| {
            style
                .get(prop)
                .and_then(|value| units.resolve(style::components(value, ' ').first()?, thickness))
                .unwrap_or(0.)
                .max(0.)
        };
        let color = |prop| {
            style
                .get(prop)
                .and_then(style::color)
                .map_or(transparent_black(), |(ink, alpha)| {
                    rgba(ink << 8 | alpha as u32)
                })
        };
        let border = if style
            .get(Prop::BORDER_TOP_STYLE)
            .is_some_and(|kind| !matches!(kind, "none" | "hidden"))
        {
            length(Prop::BORDER_TOP_WIDTH)
        } else {
            0.
        };
        Part {
            fill: color(Prop::BACKGROUND_COLOR),
            radius: length(Prop::BORDER_TOP_LEFT_RADIUS),
            border,
            edge: color(Prop::BORDER_TOP_COLOR),
            clip: match style.get(Prop::BACKGROUND_CLIP) {
                Some("padding-box") => border,
                Some("content-box") => border + length(Prop::PADDING_TOP),
                _ => 0.,
            },
        }
    }

    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let radius = self
            .radius
            .min(f32::from(bounds.size.width.min(bounds.size.height)) / 2.);
        window.paint_quad(quad(
            bounds.inset(px(self.clip)),
            Corners::all(px((radius - self.clip).max(0.))),
            self.fill,
            Edges::all(px(0.)),
            transparent_black(),
            Default::default(),
        ));
        if self.border > 0. && !self.edge.is_transparent() {
            window.paint_quad(quad(
                bounds,
                Corners::all(px(radius)),
                transparent_black(),
                Edges::all(px(self.border)),
                self.edge,
                Default::default(),
            ));
        }
    }
}

/// How a box draws its bars.
pub(super) struct Look {
    /// Light knobs, for dark color schemes.
    dark: bool,
    thin: bool,
    /// `scrollbar-color`: thumb, then track.
    colors: Option<[Rgba; 2]>,
    /// `::-webkit-scrollbar` bar sizes (x height, y width) and parts: bar, track, thumb, corner, hovered thumb.
    custom: Option<([f32; 2], [Option<Part>; 5])>,
    /// Axes, x then y, whose custom bar shows without overflow: `overflow: scroll` or `scrollbar-gutter: stable`.
    pub(super) forced: [bool; 2],
}

impl Look {
    /// `None` when `scrollbar-width: none` or `::-webkit-scrollbar { display: none }` hides the bars.
    /// `backdrop` is the nearest opaque background color behind the bars.
    pub(super) fn new(
        style: &Style,
        parts: Option<&[Option<Style>; 5]>,
        units: &style::Lengths,
        backdrop: Option<u32>,
    ) -> Option<Self> {
        let width = style.get(Prop::SCROLLBAR_WIDTH).unwrap_or("auto");
        if width == "none" {
            return None;
        }
        // A `color-scheme` picks the knobs; without one, a dark backdrop does, by HSL lightness like Chromium.
        // Transparent windows follow the system appearance.
        let scheme = style.get(Prop::COLOR_SCHEME).unwrap_or("");
        let system = || crate::desktop::native::theme_state() & 1 != 0;
        let mut look = Look {
            dark: match (scheme.contains("dark"), scheme.contains("light")) {
                (true, false) => true,
                (false, true) => false,
                (true, true) => system(),
                _ => backdrop.map_or_else(system, |ink| {
                    let [r, g, b] = [16, 8, 0].map(|shift| (ink >> shift) & 255);
                    r.max(g).max(b) + r.min(g).min(b) <= 255
                }),
            },
            thin: width == "thin",
            colors: style.get(Prop::SCROLLBAR_COLOR).and_then(|value| {
                let parts = style::components(value, ' ');
                let [thumb, track] =
                    [parts.first()?, parts.get(1)?].map(|value| style::color(value));
                Some([thumb?, track?].map(|(ink, alpha)| rgba(ink << 8 | alpha as u32)))
            }),
            custom: None,
            forced: [
                style
                    .get(Prop::OVERFLOW_X)
                    .is_some_and(|value| value == "scroll"),
                style
                    .get(Prop::OVERFLOW_Y)
                    .is_some_and(|value| value == "scroll")
                    || style
                        .get(Prop::SCROLLBAR_GUTTER)
                        .is_some_and(|value| value.starts_with("stable")),
            ],
        };
        // Like Chromium, `scrollbar-width` and `scrollbar-color` turn the `::-webkit-scrollbar` styles off.
        if let Some(parts) = parts
            && let Some(bar) = &parts[0]
            && width == "auto"
            && look.colors.is_none()
        {
            if bar.get(Prop::DISPLAY).is_some_and(|value| value == "none") {
                return None;
            }
            let size = [Prop::HEIGHT, Prop::WIDTH].map(|name| {
                bar.get(name)
                    .and_then(|value| units.resolve(value, 0.))
                    .map_or(15., |value| value.max(0.))
            });
            let thickness = size[0].max(size[1]);
            look.custom = Some((
                size,
                parts
                    .each_ref()
                    .map(|part| part.as_ref().map(|part| Part::new(part, units, thickness))),
            ));
        }
        Some(look)
    }

    /// The axes, x then y, whose bar shows for a box that scrolls on `axes` by up to `max`.
    pub(super) fn shown(&self, axes: [bool; 2], max: Point<Pixels>) -> [bool; 2] {
        let max = [max.x, max.y];
        [0, 1].map(|axis| {
            axes[axis]
                && match &self.custom {
                    Some((size, _)) => {
                        size[axis] > 0. && (self.forced[axis] || max[axis] > px(0.5))
                    }
                    None => max[axis] > px(0.5),
                }
        })
    }

    /// The layout space of shown custom bars: x is the height at the bottom, y the width at the right.
    pub(super) fn space(&self, shown: [bool; 2]) -> [f32; 2] {
        self.custom.as_ref().map_or([0.; 2], |(size, _)| {
            [0, 1].map(|axis| if shown[axis] { size[axis] } else { 0. })
        })
    }
}

#[derive(Default)]
pub(super) struct State {
    /// The offset of the last frame.
    offset: Option<Point<Pixels>>,
    /// When the box last scrolled, or the pointer left its bar.
    active: Option<Instant>,
    /// The axis of the bar under the pointer, and whether the pointer is on its thumb.
    hover: Option<(usize, bool)>,
    /// The dragged bar, and where the pointer holds its thumb.
    drag: Option<(usize, f32)>,
    wake: Option<(Instant, Timer)>,
}

pub(super) struct Bar {
    hitbox: Hitbox,
    zone: Bounds<Pixels>,
    /// Along the axis: the start and length of the track and of the thumb.
    track: [f32; 2],
    thumb: [f32; 2],
    /// The visible length and the scroll range.
    view: f32,
    max: f32,
}

fn along(position: Point<Pixels>, axis: usize) -> f32 {
    f32::from(if axis == 0 { position.x } else { position.y })
}

/// The bars of a scroll box. It also anchors the scroll range at the padding edge, as a zero-size box.
pub(super) struct Scrollbar {
    pub(super) id: usize,
    pub(super) handle: ScrollHandle,
    pub(super) look: Option<Look>,
    /// The axes, x then y, that the user scrolls.
    pub(super) axes: [bool; 2],
    /// The layout space the bars have, from `Look::space`.
    pub(super) reserved: [f32; 2],
    /// Top, right, bottom and left border widths.
    pub(super) borders: [f32; 4],
    pub(super) origin: [f32; 2],
}

impl IntoElement for Scrollbar {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl render::Element for Scrollbar {
    type RequestLayoutState = ();
    type PrepaintState = Option<(Rc<RefCell<State>>, Rc<[Option<Bar>; 2]>)>;
    fn request_layout(&mut self, window: &mut Window) -> (render::LayoutId, ()) {
        let mut style = render::Style {
            position: render::Position::Absolute,
            ..Default::default()
        };
        style.inset.left = px(self.origin[0]).into();
        style.inset.top = px(self.origin[1]).into();
        style.size.width = px(0.).into();
        style.size.height = px(0.).into();
        (window.request_layout(&style, []), ())
    }
    fn prepaint(
        &mut self,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
    ) -> Self::PrepaintState {
        let look = self.look.as_ref()?;
        let state = window.with_element_state(
            ("scrollbar", self.id).into(),
            |state: Option<Rc<RefCell<State>>>, _| {
                let state = state.unwrap_or_default();
                (state.clone(), state)
            },
        );
        let (bounds, max, offset) = (
            self.handle.bounds(),
            self.handle.max_offset(),
            self.handle.offset(),
        );
        {
            let mut state = state.borrow_mut();
            if state.offset.is_some_and(|old| old != offset) {
                state.active = Some(Instant::now());
            }
            state.offset = Some(offset);
        }
        let shown = look.shown(self.axes, max);
        // WARNING: custom bars take layout space from last frame's overflow, so a change lays the frame out again.
        if look.space(shown) != self.reserved {
            window.redraw();
        }
        if !shown.contains(&true) {
            return None;
        }
        let [top, right, bottom, left] = self.borders;
        let inner = [
            f32::from(bounds.left()) + left,
            f32::from(bounds.top()) + top,
            f32::from(bounds.right()) - right,
            f32::from(bounds.bottom()) - bottom,
        ];
        let thickness = [0, 1].map(|axis| match &look.custom {
            Some((size, _)) => size[axis],
            None => OVERLAY[usize::from(look.thin)][0],
        });
        let space = look.space(shown);
        let (max, offset) = (
            [max.x, max.y].map(f32::from),
            [offset.x, offset.y].map(f32::from),
        );
        let inset = if look.custom.is_some() { 0. } else { INSET };
        let bars = [0, 1].map(|axis| {
            if !shown[axis] {
                return None;
            }
            // The bar runs along `axis` at the far side of the other axis, and stops short of the other bar.
            let cross = 1 - axis;
            let mut origin = [0.; 2];
            let mut extent = [0.; 2];
            origin[axis] = inner[axis];
            extent[axis] =
                inner[axis + 2] - inner[axis] - if shown[cross] { thickness[cross] } else { 0. };
            origin[cross] = inner[cross + 2] - thickness[axis];
            extent[cross] = thickness[axis];
            let zone = Bounds::new(
                point(px(origin[0]), px(origin[1])),
                size(px(extent[0]), px(extent[1])),
            );
            let track = [origin[axis] + inset, (extent[axis] - 2. * inset).max(0.)];
            let view = inner[axis + 2] - inner[axis] - space[cross];
            let mut thumb = [track[0], 0.];
            if max[axis] > 0. {
                let shortest = if look.custom.is_some() {
                    thickness[axis]
                } else {
                    SHORTEST
                };
                thumb[1] = (track[1] * view / (view + max[axis]))
                    .max(shortest)
                    .min(track[1]);
                thumb[0] += (track[1] - thumb[1]) * (-offset[axis] / max[axis]).clamp(0., 1.);
            }
            Some(Bar {
                hitbox: window.insert_hitbox(zone, HitboxBehavior::BlockMouseExceptScroll),
                zone,
                track,
                thumb,
                view,
                max: max[axis],
            })
        });
        Some((state, Rc::new(bars)))
    }
    fn paint(
        &mut self,
        _: Bounds<Pixels>,
        _: &mut (),
        prepainted: &mut Self::PrepaintState,
        window: &mut Window,
    ) {
        let (Some(look), Some((state, bars))) = (&self.look, prepainted.take()) else {
            return;
        };
        let now = Instant::now();
        let mut current = state.borrow_mut();
        let alpha = if look.custom.is_some() || current.hover.is_some() || current.drag.is_some() {
            1.
        } else if let Some(at) = current.active {
            let elapsed = now - at;
            if elapsed < HOLD {
                let due = at + HOLD;
                if current.wake.as_ref().is_none_or(|(at, _)| *at <= now) {
                    let task = window.timer(due - now, |window, _| window.refresh());
                    current.wake = Some((due, task));
                }
                1.
            } else if elapsed < HOLD + FADE {
                window.request_animation_frame();
                1. - (elapsed - HOLD).as_secs_f32() / FADE.as_secs_f32()
            } else {
                0.
            }
        } else {
            0.
        };
        let [bar_width, idle, wide] = OVERLAY[usize::from(look.thin)];
        let hairline = px(1. / window.scale_factor());
        let (knob, knob_edge, track, track_edge) = if look.dark {
            (0xffffff80, 0x00000033, 0xc9c9c926, 0xe6e6e633)
        } else {
            (0x00000080, 0xffffff26, 0xfafafabf, 0xdcdcdcc7)
        };
        for (axis, bar) in bars.iter().enumerate() {
            let Some(bar) = bar else { continue };
            let active = current.hover.is_some_and(|(hovered, _)| hovered == axis)
                || current.drag.is_some_and(|(dragged, _)| dragged == axis);
            let span = |start: f32, length: f32, cross: [f32; 2]| {
                let mut bounds = bar.zone;
                if axis == 0 {
                    bounds.origin = point(px(start), bounds.origin.y + px(cross[0]));
                    bounds.size = size(px(length), px(cross[1]));
                } else {
                    bounds.origin = point(bounds.origin.x + px(cross[0]), px(start));
                    bounds.size = size(px(cross[1]), px(length));
                }
                bounds
            };
            if let Some((size, parts)) = &look.custom {
                for part in parts[..2].iter().flatten() {
                    part.paint(bar.zone, window);
                }
                let thumb =
                    if current.hover == Some((axis, true)) || active && current.drag.is_some() {
                        parts[4].or(parts[2])
                    } else {
                        parts[2]
                    };
                if let Some(thumb) = thumb
                    && bar.thumb[1] > 0.
                {
                    thumb.paint(span(bar.thumb[0], bar.thumb[1], [0., size[axis]]), window);
                }
                continue;
            }
            if alpha <= 0. {
                continue;
            }
            // The knob sits `INSET` from the far edge; a hovered bar widens it and shows the track.
            let width = if active { wide } else { idle };
            let cross = [bar_width - INSET - width, width];
            let (thumb_color, track_color) = look
                .colors
                .map_or((rgba(knob), rgba(track)), |[thumb, track]| (thumb, track));
            let edge = |color: u32| {
                if look.colors.is_some() {
                    transparent_black()
                } else {
                    rgba(color).opacity(alpha)
                }
            };
            if active {
                window.paint_quad(quad(
                    span(bar.track[0], bar.track[1], cross),
                    Corners::all(px(width / 2.)),
                    track_color.opacity(alpha),
                    Edges::all(hairline),
                    edge(track_edge),
                    Default::default(),
                ));
            }
            window.paint_quad(quad(
                span(bar.thumb[0], bar.thumb[1], cross),
                Corners::all(px(width / 2.)),
                thumb_color.opacity(alpha),
                Edges::all(hairline),
                edge(knob_edge),
                Default::default(),
            ));
        }
        if let Some((_, parts)) = &look.custom
            && let (Some(corner), [Some(x), Some(y)]) = (parts[3], &*bars)
        {
            corner.paint(
                Bounds::from_corners(
                    point(y.zone.origin.x, x.zone.origin.y),
                    point(y.zone.right(), x.zone.bottom()),
                ),
                window,
            );
        }
        drop(current);
        let handle = self.handle.clone();
        window.on_mouse_event({
            let (state, bars) = (state.clone(), bars.clone());
            move |event: &MouseMoveEvent, phase, window, _| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let mut state = state.borrow_mut();
                if let Some((axis, grab)) = state.drag {
                    if let Some(bar) = &bars[axis] {
                        let travel = bar.track[1] - bar.thumb[1];
                        let ratio = if travel > 0. {
                            ((along(event.position, axis) - grab - bar.track[0]) / travel)
                                .clamp(0., 1.)
                        } else {
                            0.
                        };
                        let mut offset = handle.offset();
                        if axis == 0 {
                            offset.x = px(-ratio * bar.max);
                        } else {
                            offset.y = px(-ratio * bar.max);
                        }
                        handle.set_offset(offset);
                        window.refresh();
                    }
                    return;
                }
                let hover = bars.iter().enumerate().find_map(|(axis, bar)| {
                    let bar = bar.as_ref().filter(|bar| bar.hitbox.is_hovered(window))?;
                    let position = along(event.position, axis);
                    Some((
                        axis,
                        position >= bar.thumb[0] && position < bar.thumb[0] + bar.thumb[1],
                    ))
                });
                if hover != state.hover {
                    if hover.is_none() {
                        state.active = Some(Instant::now());
                    }
                    state.hover = hover;
                    window.refresh();
                }
            }
        });
        window.on_mouse_event({
            let state = state.clone();
            move |_: &MouseExitEvent, phase, window, _| {
                let mut state = state.borrow_mut();
                if phase == DispatchPhase::Capture
                    && state.drag.is_none()
                    && state.hover.take().is_some()
                {
                    state.active = Some(Instant::now());
                    window.refresh();
                }
            }
        });
        window.on_mouse_event({
            let (state, handle) = (state.clone(), self.handle.clone());
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                let Some((axis, bar)) = bars.iter().enumerate().find_map(|(axis, bar)| {
                    Some((
                        axis,
                        bar.as_ref().filter(|bar| bar.hitbox.is_hovered(window))?,
                    ))
                }) else {
                    return;
                };
                cx.stop_propagation();
                let position = along(event.position, axis);
                let mut state = state.borrow_mut();
                state.active = Some(Instant::now());
                if position >= bar.thumb[0] && position < bar.thumb[0] + bar.thumb[1] {
                    state.drag = Some((axis, position - bar.thumb[0]));
                } else if bar.max > 0. {
                    // A click on the track scrolls a page toward it, by WebKit's page step.
                    let step = (bar.view * 0.875).max(bar.view - 40.).max(1.);
                    let step = if position < bar.thumb[0] { step } else { -step };
                    let mut offset = handle.offset();
                    let value = if axis == 0 {
                        &mut offset.x
                    } else {
                        &mut offset.y
                    };
                    *value = (*value + px(step)).clamp(px(-bar.max), px(0.));
                    handle.set_offset(offset);
                }
                window.refresh();
            }
        });
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, _| {
            let mut state = state.borrow_mut();
            if phase == DispatchPhase::Capture
                && event.button == MouseButton::Left
                && state.drag.take().is_some()
            {
                state.active = Some(Instant::now());
                window.refresh();
            }
        });
    }
}
