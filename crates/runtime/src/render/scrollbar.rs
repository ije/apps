use crate::style::Prop;
use crate::style::{self, Style};
use renderer::{
    Button, Color, Corners, Edges, Event, Hit, Pixels, Point, Rect, Region, ScrollRef, Timer,
    Window, point, px, size,
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

const HOLD: Duration = Duration::from_millis(1000);
const FADE: Duration = Duration::from_millis(300);
const OVERLAY: [[f32; 3]; 2] = [[17., 6., 11.], [15., 4., 9.]];
const INSET: f32 = 3.;
const SHORTEST: f32 = 26.;

#[derive(Clone, Copy)]
struct Part {
    fill: Color,
    radius: f32,
    border: f32,
    edge: Color,
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
                .map_or(Color::TRANSPARENT, |(ink, alpha)| {
                    Color::hex(ink << 8 | alpha as u32)
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

    fn paint(&self, bounds: Rect<Pixels>, window: &mut Window) {
        let radius = self
            .radius
            .min(f32::from(bounds.size.width.min(bounds.size.height)) / 2.);
        window.draw_box(
            bounds.inset(px(self.clip)),
            Corners::all(px((radius - self.clip).max(0.))),
            self.fill,
            Default::default(),
        );
        if self.border > 0. && !self.edge.is_transparent() {
            window.draw_box(
                bounds,
                Corners::all(px(radius)),
                Color::TRANSPARENT,
                renderer::Border {
                    widths: Edges::all(px(self.border)),
                    color: self.edge,
                    style: Default::default(),
                    sides: 0,
                },
            );
        }
    }
}

pub(crate) struct Look {
    dark: bool,
    thin: bool,
    colors: Option<[Color; 2]>,
    custom: Option<([f32; 2], [Option<Part>; 5])>,
    pub(super) forced: [bool; 2],
}

impl Look {
    pub(crate) fn new(
        style: &Style,
        parts: Option<&[Option<Style>; 5]>,
        units: &style::Lengths,
        backdrop: Option<u32>,
    ) -> Option<Self> {
        let width = style.get(Prop::SCROLLBAR_WIDTH).unwrap_or("auto");
        if width == "none" {
            return None;
        }
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
                Some([thumb?, track?].map(|(ink, alpha)| Color::hex(ink << 8 | alpha as u32)))
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

    pub(crate) fn shown(&self, axes: [bool; 2], max: Point<Pixels>) -> [bool; 2] {
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

    pub(crate) fn space(&self, shown: [bool; 2]) -> [f32; 2] {
        self.custom.as_ref().map_or([0.; 2], |(size, _)| {
            [0, 1].map(|axis| if shown[axis] { size[axis] } else { 0. })
        })
    }
}

// What the pointer does to the bars. A drag keeps its bar until the button goes up, wherever the pointer is.
#[derive(Clone, Copy, Default, PartialEq)]
enum Pointer {
    #[default]
    Away,
    // Over the bar of an axis, and whether over its thumb.
    Over(usize, bool),
    // Holds the thumb of an axis, this far from the start of the thumb.
    Drag(usize, f32),
}

impl Pointer {
    fn over(bars: &[Option<Bar>; 2], position: Point<Pixels>, window: &Window) -> Self {
        bars.iter()
            .enumerate()
            .find_map(|(axis, bar)| {
                let bar = bar.as_ref().filter(|bar| bar.hitbox.hovered(window))?;
                let position = along(position, axis);
                Some(Self::Over(
                    axis,
                    position >= bar.thumb[0] && position < bar.thumb[0] + bar.thumb[1],
                ))
            })
            .unwrap_or_default()
    }
}

#[derive(Default)]
pub(super) struct State {
    offset: Option<Point<Pixels>>,
    active: Option<Instant>,
    pointer: Pointer,
    wake: Option<(Instant, Timer)>,
}

pub(super) struct Bar {
    hitbox: Region,
    zone: Rect<Pixels>,
    track: [f32; 2],
    thumb: [f32; 2],
    view: f32,
    max: f32,
}

fn along(position: Point<Pixels>, axis: usize) -> f32 {
    f32::from(if axis == 0 { position.x } else { position.y })
}

pub(super) struct Scrollbar {
    pub(super) id: usize,
    pub(super) handle: ScrollRef,
    pub(super) look: Option<Look>,
    pub(super) axes: [bool; 2],
    pub(super) reserved: [f32; 2],
    pub(super) borders: [f32; 4],
    pub(super) origin: [f32; 2],
}

type Prepainted = Option<(Rc<RefCell<State>>, Rc<[Option<Bar>; 2]>)>;

struct Bars(Scrollbar, RefCell<Prepainted>);

impl renderer::Hooks for Bars {
    fn prepaint(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        *self.1.borrow_mut() = self.0.bars(cx.window);
    }
    fn paint(&self, cx: &mut renderer::NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        self.0.draw(self.1.take(), cx.window);
    }
}

impl From<Scrollbar> for renderer::Child {
    fn from(bar: Scrollbar) -> Self {
        let mut style = renderer::Style {
            position: renderer::Position::Absolute,
            ..Default::default()
        };
        style.inset.left = px(bar.origin[0]).into();
        style.inset.top = px(bar.origin[1]).into();
        style.size.width = px(0.).into();
        style.size.height = px(0.).into();
        let mut node = renderer::node().hooks(Bars(bar, RefCell::default()));
        *node.style() = style;
        node.into()
    }
}

impl Scrollbar {
    fn bars(&self, window: &mut Window) -> Prepainted {
        let look = self.look.as_ref()?;
        let state = window
            .keyed::<Rc<RefCell<State>>>(("scrollbar", self.id).into())
            .clone();
        let (bounds, max, offset, stretch) = (
            self.handle.bounds(),
            self.handle.max_offset(),
            self.handle.offset(),
            self.handle.stretch(),
        );
        {
            let mut state = state.borrow_mut();
            let drawn = offset + stretch;
            if state.offset.is_some_and(|old| old != drawn) {
                state.active = Some(Instant::now());
            }
            state.offset = Some(drawn);
        }
        let shown = look.shown(self.axes, max);
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
        let (max, offset, stretch) = (
            [max.x, max.y].map(f32::from),
            [offset.x, offset.y].map(f32::from),
            [stretch.x, stretch.y].map(f32::from),
        );
        let inset = if look.custom.is_some() { 0. } else { INSET };
        let bars = [0, 1].map(|axis| {
            if !shown[axis] {
                return None;
            }
            let cross = 1 - axis;
            let mut origin = [0.; 2];
            let mut extent = [0.; 2];
            origin[axis] = inner[axis];
            extent[axis] =
                inner[axis + 2] - inner[axis] - if shown[cross] { thickness[cross] } else { 0. };
            origin[cross] = inner[cross + 2] - thickness[axis];
            extent[cross] = thickness[axis];
            let zone = Rect::new(
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
                if look.custom.is_none() && stretch[axis] != 0. {
                    let end = thumb[0] + thumb[1];
                    thumb[1] = (thumb[1] - stretch[axis].abs()).max(shortest.min(thumb[1]));
                    if stretch[axis] < 0. {
                        thumb[0] = end - thumb[1];
                    }
                }
            }
            Some(Bar {
                hitbox: window.add_region(zone, Hit::Shield),
                zone,
                track,
                thumb,
                view,
                max: max[axis],
            })
        });
        Some((state, Rc::new(bars)))
    }
    fn draw(&self, prepainted: Prepainted, window: &mut Window) {
        let (Some(look), Some((state, bars))) = (&self.look, prepainted) else {
            return;
        };
        let now = Instant::now();
        let mut current = state.borrow_mut();
        let alpha = if look.custom.is_some() || current.pointer != Pointer::Away {
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
            let active = matches!(current.pointer, Pointer::Over(on, _) | Pointer::Drag(on, _) if on == axis);
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
                let thumb = if active && current.pointer != Pointer::Over(axis, false) {
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
            let width = if active { wide } else { idle };
            let cross = [bar_width - INSET - width, width];
            let (thumb_color, track_color) = look
                .colors
                .map_or((Color::hex(knob), Color::hex(track)), |[thumb, track]| {
                    (thumb, track)
                });
            let edge = |color: u32| {
                if look.colors.is_some() {
                    Color::TRANSPARENT
                } else {
                    Color::hex(color).fade(alpha)
                }
            };
            if active {
                window.draw_box(
                    span(bar.track[0], bar.track[1], cross),
                    Corners::all(px(width / 2.)),
                    track_color.fade(alpha),
                    renderer::Border {
                        widths: Edges::all(hairline),
                        color: edge(track_edge),
                        style: Default::default(),
                        sides: 0,
                    },
                );
            }
            window.draw_box(
                span(bar.thumb[0], bar.thumb[1], cross),
                Corners::all(px(width / 2.)),
                thumb_color.fade(alpha),
                renderer::Border {
                    widths: Edges::all(hairline),
                    color: edge(knob_edge),
                    style: Default::default(),
                    sides: 0,
                },
            );
        }
        if let Some((_, parts)) = &look.custom
            && let (Some(corner), [Some(x), Some(y)]) = (parts[3], &*bars)
        {
            corner.paint(
                Rect::between(
                    point(y.zone.origin.x, x.zone.origin.y),
                    point(y.zone.right(), x.zone.bottom()),
                ),
                window,
            );
        }
        drop(current);
        let handle = self.handle.clone();
        window.on_capture({
            let (state, bars) = (state.clone(), bars.clone());
            move |event, window, _| {
                let Event::PointerMove { at: position, .. } = *event else {
                    return;
                };
                let mut state = state.borrow_mut();
                if let Pointer::Drag(axis, grab) = state.pointer {
                    if let Some(bar) = &bars[axis] {
                        let travel = bar.track[1] - bar.thumb[1];
                        let ratio = if travel > 0. {
                            ((along(position, axis) - grab - bar.track[0]) / travel).clamp(0., 1.)
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
                let pointer = Pointer::over(&bars, position, window);
                if pointer != state.pointer {
                    if pointer == Pointer::Away {
                        state.active = Some(Instant::now());
                    }
                    state.pointer = pointer;
                    window.refresh();
                }
            }
        });
        window.on_capture({
            let state = state.clone();
            move |event, window, _| {
                let mut state = state.borrow_mut();
                if matches!(event, Event::PointerLeave { .. })
                    && matches!(state.pointer, Pointer::Over(..))
                {
                    state.pointer = Pointer::Away;
                    state.active = Some(Instant::now());
                    window.refresh();
                }
            }
        });
        window.on_bubble({
            let (state, handle, bars) = (state.clone(), self.handle.clone(), bars.clone());
            move |event, window, cx| {
                let Event::PointerDown {
                    at: position,
                    button: Button::Left,
                    ..
                } = *event
                else {
                    return;
                };
                let Some((axis, bar)) = bars.iter().enumerate().find_map(|(axis, bar)| {
                    Some((axis, bar.as_ref().filter(|bar| bar.hitbox.hovered(window))?))
                }) else {
                    return;
                };
                cx.stop_event();
                let position = along(position, axis);
                let mut state = state.borrow_mut();
                state.active = Some(Instant::now());
                if position >= bar.thumb[0] && position < bar.thumb[0] + bar.thumb[1] {
                    state.pointer = Pointer::Drag(axis, position - bar.thumb[0]);
                } else if bar.max > 0. {
                    let step = (bar.view * 0.875).max(bar.view - 40.).max(1.);
                    let step = if position < bar.thumb[0] { step } else { -step };
                    let mut offset = handle.target();
                    let value = if axis == 0 {
                        &mut offset.x
                    } else {
                        &mut offset.y
                    };
                    *value = (*value + px(step)).clamp(px(-bar.max), px(0.));
                    handle.animate_to(offset, false);
                }
                window.refresh();
            }
        });
        window.on_capture(move |event, window, _| {
            let Event::PointerUp {
                at: position,
                button: Button::Left,
                ..
            } = *event
            else {
                return;
            };
            let mut state = state.borrow_mut();
            if matches!(state.pointer, Pointer::Drag(..)) {
                // The pointer left no hover behind: it can be anywhere now, also outside the window.
                state.pointer = Pointer::over(&bars, position, window);
                state.active = Some(Instant::now());
                window.refresh();
            }
        });
    }
}
