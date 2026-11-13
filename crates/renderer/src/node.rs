use crate::{
    AlignContent, AlignItems, App, Baseline, Color, Corners, Cursor, Display, Edges, Event,
    FlexDirection, FlexWrap, FontWeight, Length, Overflow, Overscroll, Pixels, Point, Position,
    Rect, Region, Scope, SharedString, Size, Style, TextAlign, TextOverrides, TextRun, TextStyle,
    WheelPhase, WhiteSpace, Window, point, px, relative,
};
use std::{
    any::Any,
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum AvailableSpace {
    Definite(Pixels),
    #[default]
    MinContent,
    MaxContent,
}

impl AvailableSpace {
    pub(crate) fn unscaled(space: taffy::AvailableSpace, factor: f32) -> Self {
        match space {
            taffy::AvailableSpace::Definite(value) => Self::Definite(Pixels(value / factor)),
            taffy::AvailableSpace::MinContent => Self::MinContent,
            taffy::AvailableSpace::MaxContent => Self::MaxContent,
        }
    }
}

impl From<Pixels> for AvailableSpace {
    fn from(value: Pixels) -> Self {
        Self::Definite(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeKey(pub u64);

impl From<u64> for NodeKey {
    fn from(key: u64) -> Self {
        Self(key)
    }
}

impl From<usize> for NodeKey {
    fn from(key: usize) -> Self {
        Self(key as u64)
    }
}

impl From<(&'static str, usize)> for NodeKey {
    fn from((name, index): (&'static str, usize)) -> Self {
        let mut hasher = rustc_hash::FxHasher::default();
        std::hash::Hash::hash(name, &mut hasher);
        Self(std::hash::Hasher::finish(&hasher) ^ (index as u64).rotate_left(32))
    }
}

pub type MeasureFn = dyn FnMut(
    &TextStyle,
    Size<Option<Pixels>>,
    Size<AvailableSpace>,
    &mut Window,
) -> (Size<Pixels>, Option<[Pixels; 2]>);

#[derive(Default)]
pub struct Node {
    pub(crate) id: Option<NodeKey>,
    pub(crate) style: NodeStyle,
    pub(crate) children: Vec<Child>,
    pub(crate) events: Option<Box<Events>>,
    pub(crate) hooks: Option<Rc<dyn Hooks>>,
    pub(crate) content: Content,
}

// Box styles to avoid 736-byte moves in builder chains.
pub(crate) enum NodeStyle {
    Own(Box<Style>),
    Shared(Rc<Style>),
}

impl Default for NodeStyle {
    fn default() -> Self {
        Self::Own(Box::default())
    }
}

impl std::ops::Deref for NodeStyle {
    type Target = Style;

    fn deref(&self) -> &Style {
        match self {
            Self::Own(style) => style,
            Self::Shared(style) => style,
        }
    }
}

impl NodeStyle {
    pub(crate) fn make_mut(&mut self) -> &mut Style {
        if let Self::Shared(style) = self {
            *self = Self::Own(Box::new(Style::clone(style)));
        }
        let Self::Own(style) = self else {
            unreachable!("the style was copied above")
        };
        style
    }
}

// Box children to keep child lists and moves small.
pub enum Child {
    Node(Box<Node>),
    Kept(NodeKey),
    Placed(NodeKey, Rect),
}

impl From<Node> for Child {
    fn from(node: Node) -> Self {
        Self::Node(Box::new(node))
    }
}

impl From<SharedString> for Child {
    fn from(content: SharedString) -> Self {
        text(content, None).into()
    }
}

impl From<String> for Child {
    fn from(content: String) -> Self {
        text(content, None).into()
    }
}

impl From<&str> for Child {
    fn from(content: &str) -> Self {
        text(content, None).into()
    }
}

#[derive(Default)]
pub(crate) enum Content {
    #[default]
    None,
    Text(SharedString, Option<Vec<TextRun>>),
    Measured(Box<dyn Measurement>),
    Layers(bool),
}

pub(crate) trait Measurement {
    fn same(&self, old: Option<&Rc<dyn Any>>) -> bool;
    fn build(self: Box<Self>) -> (Rc<dyn Any>, Box<MeasureFn>);
}

struct Measured<S, F>(S, F);

impl<S, F> Measurement for Measured<S, F>
where
    S: PartialEq + 'static,
    F: FnMut(
            &S,
            &TextStyle,
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
        ) -> (Size<Pixels>, Option<[Pixels; 2]>)
        + 'static,
{
    fn same(&self, old: Option<&Rc<dyn Any>>) -> bool {
        old.and_then(|old| old.downcast_ref::<S>()) == Some(&self.0)
    }

    fn build(self: Box<Self>) -> (Rc<dyn Any>, Box<MeasureFn>) {
        let Measured(input, mut measure) = *self;
        let input = Rc::new(input);
        let held = input.clone();
        let measure = move |style: &TextStyle, known, available, window: &mut Window| {
            measure(&held, style, known, available, window)
        };
        (input, Box::new(measure))
    }
}

pub trait Hooks: 'static {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window)
    }
    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window)
    }
    // Kept boxes skip prepaint. Return false only if prepaint has no external reads or state for paint.
    fn volatile(&self) -> bool {
        true
    }
    // Return true only if paint stays within the box and child bounds. Otherwise culling can hide visible
    // content.
    fn bounded(&self) -> bool {
        false
    }
    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window)
    }
    fn paint_content(&self, _cx: &mut NodeCx<'_>) -> bool {
        false
    }
}

// Two hooks of one node: the outer one wraps the inner one.
struct Chain(Rc<dyn Hooks>, Rc<dyn Hooks>);

type Step = for<'a, 'b> fn(&'b dyn Hooks, &'b mut NodeCx<'a>, &'b mut dyn FnMut(&mut Window));

impl Chain {
    fn nest(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window), step: Step) {
        let NodeCx {
            bounds,
            exact,
            inset,
            baselines,
            text,
            style,
            state,
            split,
            ..
        } = *cx;
        step(&*self.0, cx, &mut |window| {
            let mut cx = NodeCx {
                window,
                bounds,
                exact,
                inset,
                baselines,
                text,
                style,
                state,
                split,
            };
            step(&*self.1, &mut cx, inner)
        })
    }
}

impl Hooks for Chain {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.nest(cx, inner, |hooks, cx, inner| hooks.prepaint(cx, inner))
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.nest(cx, inner, |hooks, cx, inner| hooks.paint(cx, inner))
    }

    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.nest(cx, inner, |hooks, cx, inner| hooks.kept(cx, inner))
    }

    fn volatile(&self) -> bool {
        self.0.volatile() || self.1.volatile()
    }

    fn bounded(&self) -> bool {
        self.0.bounded() && self.1.bounded()
    }

    fn paint_content(&self, cx: &mut NodeCx<'_>) -> bool {
        self.0.paint_content(cx) || self.1.paint_content(cx)
    }
}

pub struct NodeCx<'a> {
    pub window: &'a mut Window,
    pub bounds: Rect,
    pub exact: Rect,
    pub inset: [Pixels; 4],
    pub baselines: [Option<Pixels>; 2],
    pub text: &'a TextStyle,
    pub style: &'a Style,
    pub(crate) state: Option<&'a Rc<dyn Any>>,
    pub(crate) split: Option<&'a crate::tree::Split>,
}

impl NodeCx<'_> {
    pub fn state<S: 'static>(&self) -> Option<&S> {
        self.state?.downcast_ref()
    }

    pub fn text_layout(&self) -> Option<&crate::Paragraph> {
        Some(&self.state?.downcast_ref::<crate::text::TextState>()?.layout)
    }

    /// The parts of a box that spans columns, in window pixels. Empty for other boxes.
    pub fn fragments(&self) -> impl Iterator<Item = Rect> + '_ {
        let scale = self.window.scale_factor();
        let parts = self.split.map_or(&[][..], |split| &split.rects);
        parts
            .iter()
            .map(move |part| (*part / scale).map(Pixels) + self.exact.origin)
    }

    /// For text that spans columns, the first row of each column after the first, and how far it moves.
    pub fn row_moves(&self) -> impl Iterator<Item = (usize, Point<Pixels>)> + '_ {
        let scale = self.window.scale_factor();
        let rows = self.split.map_or(&[][..], |split| &split.rows);
        rows.iter()
            .map(move |&(row, by)| (row as usize, (by / scale).map(Pixels)))
    }
}

pub fn node() -> Node {
    thread_local!(static STYLE: Rc<Style> = Rc::default());
    shared(STYLE.with(Rc::clone))
}

pub fn shared(style: Rc<Style>) -> Node {
    Node {
        id: None,
        style: NodeStyle::Shared(style),
        children: Vec::new(),
        events: None,
        hooks: None,
        content: Content::None,
    }
}

impl Node {
    pub fn id(mut self, key: impl Into<NodeKey>) -> Self {
        self.id = Some(key.into());
        self
    }

    pub fn style(&mut self) -> &mut Style {
        self.style.make_mut()
    }

    pub fn style_ref(&self) -> &Style {
        &self.style
    }

    // Share a style only between nodes whose inputs fully determine the same style.
    pub fn shared_style(&mut self) -> Rc<Style> {
        if let NodeStyle::Own(style) = &mut self.style {
            self.style = NodeStyle::Shared(Rc::new(std::mem::take(&mut **style)));
        }
        let NodeStyle::Shared(style) = &self.style else {
            unreachable!("the style was shared above")
        };
        style.clone()
    }

    pub fn element_id(&self) -> Option<NodeKey> {
        self.id
    }

    pub fn kids(&self) -> &[Child] {
        &self.children
    }

    pub fn text_content(&self) -> Option<(&str, Option<&[TextRun]>)> {
        match &self.content {
            Content::Text(text, runs) => Some((text, runs.as_deref())),
            _ => None,
        }
    }

    pub fn shares(&self, style: &Rc<Style>) -> bool {
        matches!(&self.style, NodeStyle::Shared(own) if Rc::ptr_eq(own, style))
    }

    pub fn child(mut self, child: impl Into<Child>) -> Self {
        self.children.push(child.into());
        self
    }

    pub fn children<C: Into<Child>>(mut self, children: impl IntoIterator<Item = C>) -> Self {
        self.children.extend(children.into_iter().map(Into::into));
        self
    }

    pub fn push(&mut self, child: impl Into<Child>) {
        self.children.push(child.into());
    }

    pub fn reserve(&mut self, additional: usize) {
        self.children.reserve(additional);
    }

    pub fn adopt(mut self, text: Node) -> Self {
        debug_assert!(self.children.is_empty() && matches!(self.content, Content::None));
        self.content = text.content;
        match text.hooks {
            Some(hooks) => self.chain(hooks),
            None => self,
        }
    }

    pub fn hooks(self, hooks: impl Hooks) -> Self {
        self.chain(Rc::new(hooks))
    }

    fn chain(mut self, outer: Rc<dyn Hooks>) -> Self {
        self.hooks = Some(match self.hooks.take() {
            Some(inner) => Rc::new(Chain(outer, inner)),
            None => outer,
        });
        self
    }

    /// Sizes the node with `measure`, which layout calls again only when `input` changes.
    pub fn measure<S: PartialEq + 'static>(
        mut self,
        input: S,
        measure: impl FnMut(
            &S,
            &TextStyle,
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
        ) -> (Size<Pixels>, Option<[Pixels; 2]>)
        + 'static,
    ) -> Self {
        self.content = Content::Measured(Box::new(Measured(input, measure)));
        self
    }

    pub fn on_pointer(
        mut self,
        listener: impl FnMut(&Event, &mut Window, &mut App) + 'static,
    ) -> Self {
        let events = self.events.get_or_insert_default();
        events.pointer.push(Rc::new(RefCell::new(listener)));
        self
    }

    pub fn track_scroll(mut self, handle: &ScrollRef) -> Self {
        self.events.get_or_insert_default().scroll = Some(handle.clone());
        self
    }

    pub fn window_drag_area(mut self) -> Self {
        self.events.get_or_insert_default().drag = true;
        self
    }

    pub fn text_style(&mut self) -> &mut TextOverrides {
        &mut self.style().text
    }
}

pub fn layers(negative: bool) -> Node {
    thread_local!(static STYLE: Rc<Style> = {
        let mut node = node();
        let style = node.style();
        style.position = crate::Position::Absolute;
        style.size = Size::new(px(0.).into(), px(0.).into());
        node.shared_style()
    });
    let mut node = shared(STYLE.with(Rc::clone));
    node.content = Content::Layers(negative);
    node
}

pub fn text(text: impl Into<SharedString>, runs: Option<Vec<TextRun>>) -> Node {
    let text = text.into();
    debug_assert!(
        runs.as_ref()
            .is_none_or(|runs| runs.iter().map(|run| run.len).sum::<usize>() == text.len()),
        "invalid text runs"
    );
    let mut node = node();
    node.content = Content::Text(text, runs);
    node
}

/// It may draw outside its box.
pub fn canvas<T: 'static>(
    prepaint: impl Fn(Rect, &mut Window) -> T + 'static,
    paint: impl Fn(Rect, T, &mut Window) + 'static,
) -> Node {
    node().hooks(Canvas {
        prepaint,
        paint,
        prepared: Cell::new(None),
        within: false,
    })
}

/// Drawing must stay inside the box.
pub fn canvas_within<T: 'static>(
    prepaint: impl Fn(Rect, &mut Window) -> T + 'static,
    paint: impl Fn(Rect, T, &mut Window) + 'static,
) -> Node {
    node().hooks(Canvas {
        prepaint,
        paint,
        prepared: Cell::new(None),
        within: true,
    })
}

struct Canvas<P, F, T> {
    prepaint: P,
    paint: F,
    prepared: Cell<Option<T>>,
    within: bool,
}

impl<T: 'static, P, F> Hooks for Canvas<P, F, T>
where
    P: Fn(Rect, &mut Window) -> T + 'static,
    F: Fn(Rect, T, &mut Window) + 'static,
{
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        self.prepared
            .set(Some((self.prepaint)(cx.exact, cx.window)));
    }

    fn bounded(&self) -> bool {
        self.within
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        let Some(prepared) = self.prepared.take() else {
            return;
        };
        let bounds = cx.bounds;
        cx.window
            .scope(Scope::Clip(cx.style.overflow_mask(bounds)), |window| {
                (self.paint)(bounds, prepared, window)
            });
    }
}

type Listener = Rc<RefCell<dyn FnMut(&Event, &mut Window, &mut App)>>;

#[derive(Default)]
pub(crate) struct Events {
    pointer: Vec<Listener>,
    pub(crate) scroll: Option<ScrollRef>,
    pub(crate) drag: bool,
}

impl Events {
    pub(crate) fn needs_region(events: Option<&Self>, style: &Style) -> bool {
        style.cursor.is_some()
            || events.is_some_and(|events| {
                events.scroll.is_some() || events.drag || !events.pointer.is_empty()
            })
    }

    pub(crate) fn listen(
        &self,
        style: &Style,
        line_height: Pixels,
        region: &Region,
        window: &mut Window,
    ) {
        for listener in &self.pointer {
            let (listener, region) = (listener.clone(), *region);
            window.on_bubble(move |event, window, app| {
                if region.hovered(window) {
                    (listener.borrow_mut())(event, window, app);
                }
            });
        }
        if let Some(handle) = &self.scroll {
            let wheel = Wheel {
                handle: handle.clone(),
                region: *region,
                overflow: style.overflow,
                overscroll: style.overscroll,
                one_axis: style.restrict_scroll_to_axis,
                line_height,
            };
            window.on_bubble(move |event, window, app| {
                if let Event::Wheel {
                    delta,
                    lines,
                    phase,
                    ..
                } = *event
                {
                    wheel.scroll(delta, lines, phase, window, app);
                }
            });
        }
    }
}

struct Wheel {
    handle: ScrollRef,
    region: Region,
    overflow: Point<Overflow>,
    overscroll: Point<Overscroll>,
    one_axis: bool,
    line_height: Pixels,
}

impl Wheel {
    fn scroll(
        &self,
        delta: Point<f32>,
        lines: bool,
        phase: WheelPhase,
        window: &mut Window,
        app: &mut App,
    ) {
        let handle = &self.handle;
        let latched = window
            .gesture
            .latch
            .as_ref()
            .map(|latch| Rc::ptr_eq(&latch.0, &handle.0));
        let elsewhere = || {
            !self.region.under_pointer(window)
                || window
                    .scroll_chain
                    .as_ref()
                    .is_some_and(|chain| !chain.iter().any(|other| Rc::ptr_eq(&other.0, &handle.0)))
        };
        if latched == Some(false) || latched.is_none() && elsewhere() {
            return;
        }
        let mut delta = match lines {
            true => delta.map(|value| self.line_height * value),
            false => delta.map(px),
        };
        if !lines {
            match window.rails.0 {
                [x, y] if y >= x => delta.x = Pixels::ZERO,
                _ => delta.y = Pixels::ZERO,
            }
        }
        let (overflow, overscroll) = (self.overflow, self.overscroll);
        let along = |own: Overflow, delta: Pixels, across: Pixels, other: Overflow| {
            if own != Overflow::Scroll {
                Pixels::ZERO
            } else if !delta.is_zero() {
                delta
            } else if !self.one_axis && other != Overflow::Scroll {
                across
            } else {
                Pixels::ZERO
            }
        };
        let mut moved = point(
            along(overflow.x, delta.x, delta.y, overflow.y),
            along(overflow.y, delta.y, delta.x, overflow.x),
        );
        if !moved.x.is_zero() && !moved.y.is_zero() {
            match moved.x.abs() > moved.y.abs() {
                true => moved.y = Pixels::ZERO,
                false => moved.x = Pixels::ZERO,
            }
        }
        let max = handle.max_offset();
        let stops = [(overscroll.x, delta.x), (overscroll.y, delta.y)]
            .iter()
            .any(|(own, delta)| !delta.is_zero() && *own != Overscroll::Auto);
        if lines {
            let from = handle.target();
            let to = point(
                (from.x + moved.x).clamp(-max.x, px(0.)),
                (from.y + moved.y).clamp(-max.y, px(0.)),
            );
            if to != from {
                app.stop_event();
                handle.animate_to(to, true);
                handle.drive(window);
            } else if stops {
                app.stop_event();
            }
            return;
        }
        let momentum = phase == WheelPhase::Momentum;
        if momentum && window.gesture.spent {
            app.stop_event();
            return;
        }
        let held = phase == WheelPhase::Began && handle.hold();
        let (panned, over) = handle.pan(moved, window.event_at);
        let latched = latched == Some(true) || held || panned;
        if latched {
            window.gesture.latch = Some(handle.clone());
            window.gesture.edge = None;
        }
        let bounce = |own: Overscroll, max: Pixels, over: Pixels| match own {
            Overscroll::None => px(0.),
            _ if max > px(0.) => over,
            _ => px(0.),
        };
        let rest = point(
            bounce(overscroll.x, max.x, over.x),
            bounce(overscroll.y, max.y, over.y),
        );
        if rest != Point::default() {
            // An outer scroller may take the rest first. It comes back here if none does.
            if !latched && !stops {
                window.gesture.edge = Some((handle.clone(), rest));
                return;
            }
            window.overscroll(handle.clone(), rest, phase);
        } else if momentum && latched && over != Point::default() {
            window.gesture.spent = true;
        }
        if latched || stops {
            app.stop_event();
        }
        if panned {
            if phase == WheelPhase::None {
                handle.loosen(window);
            }
            handle.drive(window);
            window.refresh();
        }
    }
}

#[derive(Default)]
struct ScrollState {
    offset: Point<Pixels>,
    max_offset: Point<Pixels>,
    bounds: Rect,
    animation: Option<Animation>,
    driven: bool,
    stretch: Point<Pixels>,
    spring: Option<Spring>,
    loose: bool,
    glide: Option<Glide>,
}

const STIFFNESS: f32 = 20.;

// A trackpad delta that carries the time its device sent it reaches the offset spread over the time to
// the next event. Each frame then moves by its share of time, whatever the rate of the events and
// wherever they fall between frames.
struct Glide {
    to: Point<Pixels>,
    from: Instant,
    until: Instant,
    event: Instant,
    pace: Duration,
}

const PACE: Duration = Duration::from_micros(16_667);

struct Spring {
    from: Point<Pixels>,
    speed: Point<f32>,
    start: Instant,
}

impl Spring {
    fn at(&self, now: Instant) -> Option<Point<Pixels>> {
        let t = now.saturating_duration_since(self.start).as_secs_f32();
        let damping = (-12.5 * t).exp();
        let axis = |from: Pixels, speed: f32| px((f32::from(from) + 0.31 * speed * t) * damping);
        let at = point(
            axis(self.from.x, self.speed.x),
            axis(self.from.y, self.speed.y),
        );
        (t < 0.08 || at.x.abs() >= px(0.25) || at.y.abs() >= px(0.25)).then_some(at)
    }
}

const WHEEL: Duration = Duration::from_millis(150);

struct Animation {
    from: Point<Pixels>,
    to: Point<Pixels>,
    slope: Point<f32>,
    start: Instant,
    duration: Duration,
}

impl Animation {
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

impl ScrollState {
    fn pan(&mut self, delta: Point<Pixels>, sent: Option<Instant>) -> (bool, Point<Pixels>) {
        let axis = |offset: &mut Pixels, stretch: &mut Pixels, max: Pixels, mut delta: Pixels| {
            let mut moved = false;
            if !stretch.is_zero() && !delta.is_zero() {
                moved = true;
                let next = *stretch + delta / STIFFNESS;
                if next.signum() == stretch.signum() {
                    *stretch = next;
                    return (moved, px(0.));
                }
                *stretch = px(0.);
                delta = next * STIFFNESS;
            }
            let to = (*offset + delta).clamp(-max, px(0.));
            let rest = *offset + delta - to;
            moved |= to != *offset;
            *offset = to;
            (moved, rest)
        };
        let max = self.max_offset;
        let mut aim = self.glide.as_ref().map_or(self.offset, |glide| glide.to);
        let resting = aim == self.offset;
        let (moved_x, x) = axis(&mut aim.x, &mut self.stretch.x, max.x, delta.x);
        let (moved_y, y) = axis(&mut aim.y, &mut self.stretch.y, max.y, delta.y);
        let moved = moved_x || moved_y;
        if moved {
            self.animation = None;
            self.spring = None;
        }
        match sent {
            _ if !moved => {}
            Some(now) => {
                let last = self.glide.as_ref().map(|glide| (now - glide.event, glide));
                let (from, pace) = match last {
                    Some((gap, glide)) if gap < Duration::from_millis(100) => {
                        let gap = gap.clamp(PACE / 2, PACE * 6 / 5);
                        (
                            if resting { now } else { glide.from },
                            // Slower events count at once, or the scroll would run ahead and then stop.
                            gap.max((glide.pace + gap) / 2),
                        )
                    }
                    _ => (now, PACE),
                };
                self.glide = Some(Glide {
                    to: aim,
                    from,
                    until: now + pace,
                    event: now,
                    pace,
                });
            }
            None => (self.offset, self.glide) = (aim, None),
        }
        (moved, point(x, y))
    }

    fn animate_to(&mut self, offset: Point<Pixels>, wheel: bool) {
        let now = Instant::now();
        let running = self
            .animation
            .as_ref()
            .filter(|animation| now < animation.start + animation.duration);
        let (from, speed) = running.map_or((self.offset, Point::default()), |animation| {
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
        self.animation = (distance > 0.).then(|| Animation {
            from,
            to: offset,
            slope: point(
                slope(offset.x - from.x, speed.x),
                slope(offset.y - from.y, speed.y),
            ),
            start: now,
            duration,
        });
        self.offset = from;
        self.glide = None;
    }

    fn advance(&mut self, now: Instant, idle: bool) -> bool {
        self.driven = false;
        let mut moved = false;
        if let Some(animation) = &self.animation {
            let offset = if now < animation.start + animation.duration {
                animation.at(now).0
            } else {
                let to = animation.to;
                self.animation = None;
                to
            };
            moved = offset != self.offset;
            self.offset = offset;
        }
        if let Some(glide) = &mut self.glide
            && glide.to != self.offset
        {
            let span = (glide.until - glide.from).as_secs_f32();
            let done = now.saturating_duration_since(glide.from).as_secs_f32();
            let (from, to) = (self.offset, glide.to);
            self.offset = match done < span {
                true => point(
                    from.x + (to.x - from.x) * (done / span),
                    from.y + (to.y - from.y) * (done / span),
                ),
                false => to,
            };
            glide.from = glide.from.max(now);
            moved |= self.offset != from;
        }
        if self.loose && idle {
            self.loose = false;
            self.spring = (self.stretch != Point::default()).then_some(Spring {
                from: self.stretch,
                speed: Point::default(),
                start: now,
            });
        }
        if let Some(spring) = &self.spring {
            let stretch = spring.at(now);
            if stretch.is_none() {
                self.spring = None;
            }
            self.stretch = stretch.unwrap_or_default();
            moved = true;
        }
        moved
    }
}

#[derive(Clone, Default)]
pub struct ScrollRef(Rc<RefCell<ScrollState>>);

impl std::fmt::Debug for ScrollRef {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_tuple("ScrollRef").field(&self.offset()).finish()
    }
}

impl ScrollRef {
    pub fn offset(&self) -> Point<Pixels> {
        self.0.borrow().offset
    }

    pub fn stretch(&self) -> Point<Pixels> {
        self.0.borrow().stretch
    }

    pub fn max_offset(&self) -> Point<Pixels> {
        self.0.borrow().max_offset
    }

    pub fn bounds(&self) -> Rect {
        self.0.borrow().bounds
    }

    pub fn target(&self) -> Point<Pixels> {
        let scroll = self.0.borrow();
        scroll
            .animation
            .as_ref()
            .map_or(scroll.offset, |animation| animation.to)
    }

    pub fn set_offset(&self, offset: Point<Pixels>) {
        let scroll = &mut *self.0.borrow_mut();
        (scroll.offset, scroll.stretch) = (offset, Point::default());
        (scroll.animation, scroll.spring, scroll.loose) = (None, None, false);
        scroll.glide = None;
    }

    pub fn animate_to(&self, offset: Point<Pixels>, wheel: bool) {
        self.0.borrow_mut().animate_to(offset, wheel);
    }

    fn pan(&self, delta: Point<Pixels>, sent: Option<Instant>) -> (bool, Point<Pixels>) {
        self.0.borrow_mut().pan(delta, sent)
    }

    pub(crate) fn stretch_by(&self, rest: Point<Pixels>) {
        let scroll = &mut *self.0.borrow_mut();
        scroll.stretch += rest.map(|value| value / STIFFNESS);
        scroll.spring = None;
    }

    pub(crate) fn loosen(&self, window: &mut Window) {
        let mut scroll = self.0.borrow_mut();
        scroll.loose = scroll.stretch != Point::default();
        drop(scroll);
        self.drive(window);
    }

    fn hold(&self) -> bool {
        let scroll = &mut *self.0.borrow_mut();
        scroll.spring = None;
        scroll.stretch != Point::default()
    }

    pub(crate) fn release(&self, speed: Point<f32>, window: &mut Window) -> bool {
        let mut scroll = self.0.borrow_mut();
        let stretch = scroll.stretch;
        if stretch == Point::default() || scroll.spring.is_some() {
            return stretch != Point::default();
        }
        let outward = |stretch: Pixels, speed: f32| match f32::from(stretch) * speed > 0. {
            true => speed,
            false => 0.,
        };
        scroll.loose = false;
        scroll.spring = Some(Spring {
            from: stretch,
            speed: point(outward(stretch.x, speed.x), outward(stretch.y, speed.y)),
            start: Instant::now(),
        });
        drop(scroll);
        self.drive(window);
        true
    }

    pub(crate) fn drive(&self, window: &mut Window) {
        {
            let scroll = &mut *self.0.borrow_mut();
            let gliding = (scroll.glide.as_ref()).is_some_and(|glide| glide.to != scroll.offset);
            let busy =
                scroll.animation.is_some() || scroll.spring.is_some() || scroll.loose || gliding;
            if scroll.driven || !busy {
                return;
            }
            scroll.driven = true;
        }
        let handle = self.clone();
        window.next_frame(move |window, _| {
            let now = window.frame_at.unwrap_or_else(Instant::now);
            let idle = now.saturating_duration_since(window.rails.1) > Duration::from_millis(100);
            if handle.0.borrow_mut().advance(now, idle) {
                window.refresh();
            }
            handle.drive(window);
        });
    }

    pub(crate) fn clamp(
        &self,
        style: &Style,
        bounds: Rect,
        content: Size<Pixels>,
        scale: f32,
    ) -> Point<Pixels> {
        // Padding was truncated to layout units, so it is truncated the same way here.
        let unit = |value: Pixels| px(crate::geometry::to_layout_unit(value.0, scale) / scale);
        let side = |length: Length| match length {
            Length::Px(value) => unit(value),
            Length::Fraction(fraction) => bounds.size.width * fraction,
            Length::Calc(fraction, offset) => bounds.size.width * fraction + unit(offset),
            Length::Auto => Pixels::ZERO,
        };
        let padding = Size::new(
            side(style.padding.left) + side(style.padding.right),
            side(style.padding.top) + side(style.padding.bottom),
        );
        // Taffy and these sums differ after a few decimals, so the range rounds to two places.
        let round = |value: Pixels| (value * 100.).round() / 100.;
        let max = Point::from(content + padding - bounds.size)
            .map(round)
            .max(Point::default());
        let scroll = &mut *self.0.borrow_mut();
        let within =
            |at: Point<Pixels>| point(at.x.clamp(-max.x, px(0.)), at.y.clamp(-max.y, px(0.)));
        scroll.offset = within(scroll.offset);
        if let Some(glide) = &mut scroll.glide {
            glide.to = within(glide.to);
        }
        scroll.max_offset = max;
        scroll.bounds = bounds;
        scroll.offset + scroll.stretch
    }
}

#[cfg(test)]
mod tests {
    use super::{Animation, ScrollRef, Spring};
    use crate::{Pixels, Point, point, px};
    use std::time::{Duration, Instant};

    #[test]
    fn stretch_follows_the_pan_and_gives_way_to_scrolling() {
        let delta = |y: f32| point(px(0.), px(y));
        let handle = ScrollRef::default();
        handle.0.borrow_mut().max_offset = delta(100.);

        let pan = |by: Point<Pixels>| handle.pan(by, None);
        assert_eq!(pan(delta(-30.)), (true, delta(0.)));
        assert_eq!(pan(delta(50.)), (true, delta(20.)));
        assert_eq!(handle.offset().y, px(0.));
        assert_eq!(pan(delta(10.)), (false, delta(10.)));

        handle.stretch_by(delta(40.));
        assert_eq!(handle.stretch().y, px(2.));
        assert_eq!(pan(delta(20.)), (true, delta(0.)));
        assert_eq!(handle.stretch().y, px(3.));
        assert_eq!(pan(delta(-80.)), (true, delta(0.)));
        assert_eq!(handle.stretch().y, px(0.));
        assert_eq!(handle.offset().y, px(-20.));

        assert_eq!(pan(delta(-100.)), (true, delta(-20.)));
        handle.stretch_by(delta(-20.));
        assert_eq!(handle.stretch().y, px(-1.));
        handle.set_offset(delta(-50.));
        assert_eq!(handle.stretch(), Default::default());
    }

    #[test]
    fn pans_move_evenly_between_events() {
        let delta = |y: f32| point(px(0.), px(y));
        let handle = ScrollRef::default();
        handle.0.borrow_mut().max_offset = delta(1000.);
        let (start, frame) = (Instant::now(), Duration::from_micros(8_333));
        let mut last = 0.;
        let mut steps = Vec::new();
        // Events at 60 Hz, frames at 120 Hz, 3 ms after the events.
        for tick in 0..24 {
            let now = start + frame * tick;
            if tick % 2 == 0 && tick < 16 {
                handle.pan(delta(-10.), Some(now));
                // An event that moves nothing, like the end of a gesture, leaves the rest to come.
                handle.pan(delta(0.), Some(now));
            }
            handle
                .0
                .borrow_mut()
                .advance(now + Duration::from_millis(3), false);
            let y = f32::from(handle.offset().y);
            steps.push(last - y);
            last = y;
        }
        assert!(
            steps[4..16].iter().all(|step| (step - 5.).abs() < 0.01),
            "{steps:?}"
        );
        assert_eq!(handle.offset().y, px(-80.));

        handle.pan(delta(-10.), Some(start + frame * 30));
        handle.set_offset(delta(-50.));
        assert!(!handle.0.borrow_mut().advance(start + frame * 40, false));
        assert_eq!(handle.offset().y, px(-50.));
    }

    #[test]
    fn springs_go_further_with_speed_and_end_at_the_edge() {
        let start = Instant::now();
        let spring = |from: f32, speed: f32| Spring {
            from: point(px(0.), px(from)),
            speed: point(0., speed),
            start,
        };
        let at = |spring: &Spring, ms: u64| {
            spring
                .at(start + Duration::from_millis(ms))
                .map(|at| f32::from(at.y))
        };
        assert!((at(&spring(100., 0.), 50).unwrap() - 53.53).abs() < 0.01);
        assert!((at(&spring(0., 4000.), 100).unwrap() - 35.53).abs() < 0.01);
        assert_eq!(at(&spring(0., 4000.), 0), Some(0.));
        assert_eq!(at(&spring(100., 0.), 600), None);
        assert_eq!(at(&spring(0., 4000.), 800), None);
    }

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

        let handle = ScrollRef::default();
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

macro_rules! setters {
    ($($name:ident($($arg:ident: $kind:ty),*) => |$style:ident| $set:expr;)*) => {
        impl Node {$(
            pub fn $name(mut self, $($arg: $kind),*) -> Self {
                let $style = self.style();
                $set;
                self
            }
        )*}
    };
}

setters! {
    flex() => |s| s.display = Display::Flex;
    grid() => |s| s.display = Display::Grid;
    hidden() => |s| s.display = Display::None;
    absolute() => |s| s.position = Position::Absolute;
    relative() => |s| s.position = Position::Relative;
    stacking() => |s| s.stacking = true;
    z_index(order: i32) => |s| s.z_index = Some(order);
    flex_row() => |s| s.flex_direction = FlexDirection::Row;
    flex_col() => |s| s.flex_direction = FlexDirection::Column;
    flex_wrap() => |s| s.flex_wrap = FlexWrap::Wrap;
    flex_nowrap() => |s| s.flex_wrap = FlexWrap::NoWrap;
    flex_1() => |s| (s.flex_grow, s.flex_shrink, s.flex_basis) = (1., 1., relative(0.));
    flex_shrink(by: f32) => |s| s.flex_shrink = by;
    flex_shrink_0() => |s| s.flex_shrink = 0.;
    items_start() => |s| s.align_items = Some(AlignItems::FlexStart);
    items_center() => |s| s.align_items = Some(AlignItems::Center);
    justify_center() => |s| s.justify_content = Some(AlignContent::Center);
    baseline(which: Baseline) => |s| s.baseline = which;
    top(at: impl Into<Length>) => |s| s.inset.top = at.into();
    right(at: impl Into<Length>) => |s| s.inset.right = at.into();
    bottom(at: impl Into<Length>) => |s| s.inset.bottom = at.into();
    left(at: impl Into<Length>) => |s| s.inset.left = at.into();
    top_0() => |s| s.inset.top = Length::Px(Pixels::ZERO);
    left_0() => |s| s.inset.left = Length::Px(Pixels::ZERO);
    w(width: impl Into<Length>) => |s| s.size.width = width.into();
    h(height: impl Into<Length>) => |s| s.size.height = height.into();
    size(side: impl Into<Length>) => |s| s.size = { let side = side.into(); Size::new(side, side) };
    w_full() => |s| s.size.width = relative(1.);
    size_full() => |s| s.size = Size::new(relative(1.), relative(1.));
    min_w(width: impl Into<Length>) => |s| s.min_size.width = width.into();
    min_h(height: impl Into<Length>) => |s| s.min_size.height = height.into();
    max_w(width: impl Into<Length>) => |s| s.max_size.width = width.into();
    max_h(height: impl Into<Length>) => |s| s.max_size.height = height.into();
    mt(margin: impl Into<Length>) => |s| s.margin.top = margin.into();
    mr(margin: impl Into<Length>) => |s| s.margin.right = margin.into();
    mb(margin: impl Into<Length>) => |s| s.margin.bottom = margin.into();
    ml(margin: impl Into<Length>) => |s| s.margin.left = margin.into();
    mt_auto() => |s| s.margin.top = Length::Auto;
    mr_auto() => |s| s.margin.right = Length::Auto;
    mb_auto() => |s| s.margin.bottom = Length::Auto;
    ml_auto() => |s| s.margin.left = Length::Auto;
    pt(padding: impl Into<Length>) => |s| s.padding.top = padding.into();
    pr(padding: impl Into<Length>) => |s| s.padding.right = padding.into();
    pb(padding: impl Into<Length>) => |s| s.padding.bottom = padding.into();
    pl(padding: impl Into<Length>) => |s| s.padding.left = padding.into();
    gap_x(gap: impl Into<Length>) => |s| s.gap.width = gap.into();
    gap_y(gap: impl Into<Length>) => |s| s.gap.height = gap.into();
    border(width: Pixels) => |s| s.border_widths = Edges::all(width);
    border_t(width: Pixels) => |s| s.border_widths.top = width;
    border_r(width: Pixels) => |s| s.border_widths.right = width;
    border_b(width: Pixels) => |s| s.border_widths.bottom = width;
    border_l(width: Pixels) => |s| s.border_widths.left = width;
    border_color(color: Color) => |s| s.border_color = Some(color);
    // A radius this large makes the shorter sides round.
    rounded_full() => |s| s.corner_radii = Corners::all(Length::Px(px(f32::MAX / 4.))).into();
    bg(color: Color) => |s| s.background = Some(color);
    opacity(alpha: f32) => |s| s.opacity = Some(alpha);
    cursor(cursor: Cursor) => |s| s.cursor = Some(cursor);
    overflow_hidden() => |s| s.overflow = Point::new(Overflow::Hidden, Overflow::Hidden);
    overflow_y_scroll() => |s| s.overflow.y = Overflow::Scroll;
    overflow_y_hidden() => |s| s.overflow.y = Overflow::Hidden;
    restrict_scroll_to_axis() => |s| s.restrict_scroll_to_axis = true;
    text_color(color: Color) => |s| s.text.set_color(color);
    text_size(size: Pixels) => |s| s.text.set_font_size(size);
    line_height(height: Pixels) => |s| s.text.set_line_height(height);
    font_family(family: impl Into<SharedString>) => |s| s.text.set_font_family(family.into());
    font_weight(weight: FontWeight) => |s| s.text.set_font_weight(weight);
    whitespace_nowrap() => |s| s.text.set_white_space(WhiteSpace::Nowrap);
    text_left() => |s| s.text.set_text_align(TextAlign::Left);
    text_center() => |s| s.text.set_text_align(TextAlign::Center);
    text_right() => |s| s.text.set_text_align(TextAlign::Right);
}
