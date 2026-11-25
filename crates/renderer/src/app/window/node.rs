use super::scroll::{ScrollRef, Wheel};
use crate::{
    AlignContent, AlignItems, App, AvailableSpace, Baseline, Color, Corners, Cursor, Display,
    Edges, Event, FlexDirection, FlexWrap, FontWeight, Length, Overflow, Pixels, Point, Position,
    Rect, Region, Scope, SharedString, Size, Style, TextAlign, TextOverrides, TextRun, TextStyle,
    WhiteSpace, Window, px, relative,
};
use std::{
    any::Any,
    cell::{Cell, RefCell},
    rc::Rc,
};

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
    // Return true only if paint reads nothing outside the tree, so a box that stays where it was can draw as
    // in the last frame. Paint that sometimes reads more calls `Window::paint_volatile`.
    fn cached(&self) -> bool {
        false
    }
    // Return true only if a box whose boxes all stay where they were needs no prepaint, nor `kept`.
    fn skippable(&self) -> bool {
        false
    }
    // Called instead of the prepaint of a box whose boxes all stay where they were, with its exact bounds.
    // Return true if it gave its ancestors what `kept` gives, so the boxes inside are skipped too.
    fn skip(&self, _exact: Rect, _window: &mut Window) -> bool {
        false
    }
    // Return true if `skip_inside` and `shift_inside` must run when a box around this one skips or shifts.
    fn skips_inside(&self) -> bool {
        false
    }
    // Like `skip` and `shift`, but a box around this one took the call, and what this one gives its ancestors.
    fn skip_inside(&self, _exact: Rect, _window: &mut Window) {}
    fn shift_inside(&self, _by: Point<Pixels>, _exact: Rect, _window: &mut Window) {}
    // Called instead of the prepaint of a box whose boxes all moved together by the scroll offset `by`, with its
    // exact bounds there. Return true if it moved what they gave the page, so the boxes inside are skipped.
    fn shift(&self, _by: Point<Pixels>, _exact: Rect, _window: &mut Window) -> bool {
        false
    }
    // Return true only if `shift`, or nothing for a box inside the box that a scroll moved, keeps what prepaint
    // would give.
    fn shifts(&self) -> bool {
        false
    }
    // A box around this one moved by `by` without a prepaint of this one, with `shift`. What the hooks gave the page
    // moved with it, so only their own record of their place moves.
    fn lag(&self, _by: Point<Pixels>) {}
    // The near and far corners of the rectangle between `corners` in a box of `size` where paint moves it, like a
    // transform. Scroll containers count it in their scrollable overflow.
    fn reach(&self, _size: Size<Pixels>, corners: [Point<Pixels>; 2]) -> [Point<Pixels>; 2] {
        corners
    }
    // Return true if the lines that a measured box draws overflow it to the left, like right-to-left text.
    fn overflows_left(&self) -> bool {
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

    fn cached(&self) -> bool {
        self.0.cached() && self.1.cached()
    }

    fn skippable(&self) -> bool {
        self.0.skippable() && self.1.skippable()
    }

    fn skip(&self, exact: Rect, window: &mut Window) -> bool {
        let outer = self.0.skip(exact, window);
        self.1.skip(exact, window) && outer
    }

    fn shift(&self, by: Point<Pixels>, exact: Rect, window: &mut Window) -> bool {
        self.0.shift(by, exact, window) && self.1.shift(by, exact, window)
    }

    fn shifts(&self) -> bool {
        self.0.shifts() && self.1.shifts()
    }

    fn lag(&self, by: Point<Pixels>) {
        self.0.lag(by);
        self.1.lag(by);
    }

    fn skips_inside(&self) -> bool {
        self.0.skips_inside() || self.1.skips_inside()
    }

    fn skip_inside(&self, exact: Rect, window: &mut Window) {
        self.0.skip_inside(exact, window);
        self.1.skip_inside(exact, window);
    }

    fn shift_inside(&self, by: Point<Pixels>, exact: Rect, window: &mut Window) {
        self.0.shift_inside(by, exact, window);
        self.1.shift_inside(by, exact, window);
    }

    fn reach(&self, size: Size<Pixels>, corners: [Point<Pixels>; 2]) -> [Point<Pixels>; 2] {
        self.0.reach(size, self.1.reach(size, corners))
    }

    fn overflows_left(&self) -> bool {
        self.0.overflows_left() || self.1.overflows_left()
    }
}

// Where a column set puts a box of its flow, in layout units.
#[derive(Debug, PartialEq)]
pub(crate) struct Split {
    // The move into its column, beyond the move of its parent.
    pub(crate) shift: Point<f32>,
    // A box that spans columns: its part in each column from its moved origin, and for text, the first row of
    // each column after the first with its move.
    pub(crate) rects: Box<[Rect<f32>]>,
    pub(crate) rows: Box<[(u32, Point<f32>)]>,
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
    pub(crate) split: Option<&'a Split>,
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

/// It may draw outside its box. Paint must read only its captured data and the prepaint value, or call
/// `Window::paint_volatile`, as a box that stays where it was draws as in the last frame.
pub fn canvas<T: 'static>(
    prepaint: impl Fn(Rect, &mut Window) -> T + 'static,
    paint: impl Fn(Rect, &T, &mut Window) + 'static,
) -> Node {
    node().hooks(Canvas {
        prepaint,
        paint,
        prepared: Cell::new(None),
        within: false,
        watches: false,
        placed: Cell::new(Rect::default()),
    })
}

/// Drawing must stay inside the box.
pub fn canvas_within<T: 'static>(
    prepaint: impl Fn(Rect, &mut Window) -> T + 'static,
    paint: impl Fn(Rect, &T, &mut Window) + 'static,
) -> Node {
    node().hooks(Canvas {
        prepaint,
        paint,
        prepared: Cell::new(None),
        within: true,
        watches: false,
        placed: Cell::new(Rect::default()),
    })
}

/// Like `canvas_within`, for a canvas whose prepaint must run in each frame that moves it, as one that watches
/// its place in the window.
pub fn canvas_watching<T: 'static>(
    prepaint: impl Fn(Rect, &mut Window) -> T + 'static,
    paint: impl Fn(Rect, &T, &mut Window) + 'static,
) -> Node {
    node().hooks(Canvas {
        prepaint,
        paint,
        prepared: Cell::new(None),
        within: true,
        watches: true,
        placed: Cell::new(Rect::default()),
    })
}

struct Canvas<P, F, T> {
    prepaint: P,
    paint: F,
    prepared: Cell<Option<T>>,
    within: bool,
    watches: bool,
    // Where the last prepaint placed it, which a shift moves.
    placed: Cell<Rect>,
}

impl<T: 'static, P, F> Hooks for Canvas<P, F, T>
where
    P: Fn(Rect, &mut Window) -> T + 'static,
    F: Fn(Rect, &T, &mut Window) + 'static,
{
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        self.placed.set(cx.exact);
        self.prepared
            .set(Some((self.prepaint)(cx.exact, cx.window)));
    }

    fn volatile(&self) -> bool {
        false
    }

    // A watching canvas prepares again when a box around it shifts.
    fn skips_inside(&self) -> bool {
        self.watches
    }

    fn shift_inside(&self, by: Point<Pixels>, exact: Rect, window: &mut Window) {
        self.shift(by, exact, window);
    }

    fn cached(&self) -> bool {
        true
    }

    // The prepaint value stays for paint.
    fn skippable(&self) -> bool {
        true
    }

    fn skip(&self, _: Rect, _: &mut Window) -> bool {
        true
    }

    // The exact bounds of a box inside the box that a scroll moved are those of its last prepaint, so a watching
    // canvas moves its own place.
    fn shift(&self, by: Point<Pixels>, _: Rect, window: &mut Window) -> bool {
        if self.watches {
            let placed = self.placed.get();
            let moved = Rect::new(placed.origin + by, placed.size);
            self.placed.set(moved);
            self.prepared.set(Some((self.prepaint)(moved, window)));
        }
        true
    }

    fn shifts(&self) -> bool {
        true
    }

    // A kept canvas prepares its paint again, at its kept place.
    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.prepaint(cx, inner)
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
                (self.paint)(bounds, &prepared, window)
            });
        self.prepared.set(Some(prepared));
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
