use crate::{
    AlignItems, App, Baseline, Bounds, Corners, CursorStyle, Display, Edges, FlexDirection,
    FlexWrap, FontWeight, Hitbox, JustifyContent, Length, MouseButton, MouseDownEvent,
    MouseMoveEvent, ObjectFit, Overflow, Pixels, Point, Position, RenderImage, Rgba,
    ScrollWheelEvent, SharedString, Size, Style, TextAlign, TextRun, TextStyle,
    TextStyleRefinement, WhiteSpace, Window, input::DispatchPhase, point, px, relative,
};
use std::{
    any::Any,
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

/// The space a measured node can take along one axis.
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

/// An identifier that keeps a box, and state, from one frame to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ElementId(pub u64);

impl From<usize> for ElementId {
    fn from(id: usize) -> Self {
        Self(id as u64)
    }
}

impl From<u64> for ElementId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<(&'static str, usize)> for ElementId {
    fn from((name, id): (&'static str, usize)) -> Self {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in name.bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x100000001b3);
        }
        Self(hash ^ (id as u64).rotate_left(32))
    }
}

type Listener<E> = Box<dyn FnMut(&E, &mut Window, &mut App)>;

/// What a box measures with, from its text style and its kept state.
pub type MeasureFn = dyn FnMut(
    &TextStyle,
    Size<Option<Pixels>>,
    Size<AvailableSpace>,
    &mut Window,
) -> (Size<Pixels>, Option<[Pixels; 2]>);

/// A box of the tree as the view describes it. The window keeps it in its tree: a node with the
/// same id, or at the same place in its nearest parent with an id, updates the kept box, so layout
/// caches survive.
#[derive(Default)]
pub struct Node {
    pub(crate) id: Option<ElementId>,
    pub(crate) style: Style,
    pub(crate) children: Vec<Child>,
    pub(crate) events: Option<Box<Events>>,
    pub(crate) hooks: Option<Rc<dyn Hooks>>,
    pub(crate) content: Content,
}

/// A child of a node: a new description, or the kept subtree of the node with that id.
// WARNING: boxing the description would allocate for every box built, so children move it instead.
#[allow(clippy::large_enum_variant)]
pub enum Child {
    Node(Node),
    Kept(ElementId),
}

impl From<Node> for Child {
    fn from(node: Node) -> Self {
        Self::Node(node)
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
    Image(Arc<RenderImage>, ObjectFit),
    Text(SharedString, Option<Vec<TextRun>>),
    Measured(Box<dyn Measurement>),
    Layers(bool),
}

/// The measured state of a leaf, kept while it compares equal.
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
        old.and_then(|old| old.downcast_ref::<S>())
            .is_some_and(|old| *old == self.0)
    }

    fn build(self: Box<Self>) -> (Rc<dyn Any>, Box<MeasureFn>) {
        let Measured(state, mut measure) = *self;
        let state = Rc::new(state);
        let kept = state.clone();
        (
            state,
            Box::new(move |style, known, available, window| {
                measure(&kept, style, known, available, window)
            }),
        )
    }
}

/// Code a node runs around its own box each frame. `inner` prepaints or paints the box, its
/// content and its children; hooks keep per-frame state in cells.
pub trait Hooks: 'static {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window)
    }
    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window)
    }
}

/// Hooks around other hooks.
struct Chain(Rc<dyn Hooks>, Rc<dyn Hooks>);

impl Hooks for Chain {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let (bounds, exact, baselines, text, style, state) = (
            cx.bounds,
            cx.exact,
            cx.baselines,
            cx.text,
            cx.style,
            cx.state,
        );
        self.0.prepaint(cx, &mut |window| {
            let mut cx = NodeCx {
                window,
                bounds,
                exact,
                baselines,
                text,
                style,
                state,
            };
            self.1.prepaint(&mut cx, inner)
        })
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let (bounds, exact, baselines, text, style, state) = (
            cx.bounds,
            cx.exact,
            cx.baselines,
            cx.text,
            cx.style,
            cx.state,
        );
        self.0.paint(cx, &mut |window| {
            let mut cx = NodeCx {
                window,
                bounds,
                exact,
                baselines,
                text,
                style,
                state,
            };
            self.1.paint(&mut cx, inner)
        })
    }
}

/// A node's frame for its hooks.
pub struct NodeCx<'a> {
    pub window: &'a mut Window,
    /// The border box in window coordinates, snapped to device pixels.
    pub bounds: Bounds<Pixels>,
    /// The border box in layout units.
    pub exact: Bounds<Pixels>,
    /// The first and last baselines from the top of the border box.
    pub baselines: [Option<Pixels>; 2],
    pub text: &'a TextStyle,
    pub style: &'a Style,
    pub(crate) state: Option<&'a Rc<dyn Any>>,
}

impl NodeCx<'_> {
    /// The kept state of a measured node.
    pub fn state<S: 'static>(&self) -> Option<&S> {
        self.state?.downcast_ref()
    }

    /// The shaped lines of a text node.
    pub fn text_layout(&self) -> Option<&crate::TextLayout> {
        Some(&self.state?.downcast_ref::<crate::text::TextState>()?.layout)
    }
}

pub fn node() -> Node {
    Node::default()
}

impl Node {
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn style(&mut self) -> &mut Style {
        &mut self.style
    }

    pub fn child(mut self, child: impl Into<Child>) -> Self {
        self.children.push(child.into());
        self
    }

    pub fn children<C: Into<Child>>(mut self, children: impl IntoIterator<Item = C>) -> Self {
        self.children.extend(children.into_iter().map(Into::into));
        self
    }

    /// Runs `hooks` around the box, outside the hooks it has.
    pub fn hooks(mut self, hooks: impl Hooks) -> Self {
        let outer: Rc<dyn Hooks> = Rc::new(hooks);
        self.hooks = Some(match self.hooks.take() {
            Some(inner) => Rc::new(Chain(outer, inner)),
            None => outer,
        });
        self
    }

    /// Paints this box in the list of its nearest stacking context, ordered by `order`.
    pub fn z_index(mut self, order: i32) -> Self {
        self.style.z_index = Some(order);
        self
    }

    /// Makes this box a stacking context: its descendants with a z-index paint in its own list.
    pub fn stacking(mut self) -> Self {
        self.style.stacking = true;
        self
    }

    /// A leaf sized by `measure`, from its text style and `state`. While both compare equal, the
    /// kept state and layout stay; hooks read the kept state with [`NodeCx::state`].
    pub fn measure<S: PartialEq + 'static>(
        mut self,
        state: S,
        measure: impl FnMut(
            &S,
            &TextStyle,
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
        ) -> (Size<Pixels>, Option<[Pixels; 2]>)
        + 'static,
    ) -> Self {
        self.content = Content::Measured(Box::new(Measured(state, measure)));
        self
    }

    fn events(&mut self) -> &mut Events {
        self.events.get_or_insert_default()
    }

    pub fn on_mouse_down(
        mut self,
        button: MouseButton,
        listener: impl FnMut(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.events()
            .mouse_down
            .push((button, Rc::new(RefCell::new(Box::new(listener)))));
        self
    }

    pub fn on_mouse_move(
        mut self,
        listener: impl FnMut(&MouseMoveEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.events()
            .mouse_move
            .push(Rc::new(RefCell::new(Box::new(listener))));
        self
    }

    /// Scrolls the overflow with the wheel and keeps the offset in `handle`.
    pub fn track_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.events().scroll = Some(handle.clone());
        self
    }

    /// Moves the window when dragged, like `app-region: drag`.
    pub fn window_drag_area(mut self) -> Self {
        self.events().drag = true;
        self
    }

    pub fn object_fit(mut self, fit: ObjectFit) -> Self {
        if let Content::Image(_, kept) = &mut self.content {
            *kept = fit;
        }
        self
    }

    pub fn text_style(&mut self) -> &mut TextStyleRefinement {
        &mut self.style().text
    }

    pub fn flex(mut self) -> Self {
        self.style().display = Display::Flex;
        self
    }
    pub fn grid(mut self) -> Self {
        self.style().display = Display::Grid;
        self
    }
    pub fn hidden(mut self) -> Self {
        self.style().display = Display::None;
        self
    }
    pub fn flex_row(mut self) -> Self {
        self.style().flex_direction = FlexDirection::Row;
        self
    }
    pub fn flex_col(mut self) -> Self {
        self.style().flex_direction = FlexDirection::Column;
        self
    }
    pub fn flex_wrap(mut self) -> Self {
        self.style().flex_wrap = FlexWrap::Wrap;
        self
    }
    pub fn flex_nowrap(mut self) -> Self {
        self.style().flex_wrap = FlexWrap::NoWrap;
        self
    }
    pub fn flex_1(mut self) -> Self {
        let style = self.style();
        style.flex_grow = 1.;
        style.flex_shrink = 1.;
        style.flex_basis = relative(0.);
        self
    }
    pub fn flex_shrink(mut self, value: f32) -> Self {
        self.style().flex_shrink = value;
        self
    }
    pub fn flex_shrink_0(mut self) -> Self {
        self.style().flex_shrink = 0.;
        self
    }
    pub fn items_start(mut self) -> Self {
        self.style().align_items = Some(AlignItems::FlexStart);
        self
    }
    pub fn items_center(mut self) -> Self {
        self.style().align_items = Some(AlignItems::Center);
        self
    }
    pub fn baseline(mut self, value: Baseline) -> Self {
        self.style().baseline = value;
        self
    }
    pub fn justify_center(mut self) -> Self {
        self.style().justify_content = Some(JustifyContent::Center);
        self
    }
    pub fn absolute(mut self) -> Self {
        self.style().position = Position::Absolute;
        self
    }
    pub fn relative(mut self) -> Self {
        self.style().position = Position::Relative;
        self
    }
    pub fn top(mut self, value: impl Into<Length>) -> Self {
        self.style().inset.top = value.into();
        self
    }
    pub fn right(mut self, value: impl Into<Length>) -> Self {
        self.style().inset.right = value.into();
        self
    }
    pub fn bottom(mut self, value: impl Into<Length>) -> Self {
        self.style().inset.bottom = value.into();
        self
    }
    pub fn left(mut self, value: impl Into<Length>) -> Self {
        self.style().inset.left = value.into();
        self
    }
    pub fn top_0(mut self) -> Self {
        self.style().inset.top = Length::Px(Pixels::ZERO);
        self
    }
    pub fn left_0(mut self) -> Self {
        self.style().inset.left = Length::Px(Pixels::ZERO);
        self
    }
    pub fn w(mut self, value: impl Into<Length>) -> Self {
        self.style().size.width = value.into();
        self
    }
    pub fn h(mut self, value: impl Into<Length>) -> Self {
        self.style().size.height = value.into();
        self
    }
    pub fn size(mut self, value: impl Into<Length>) -> Self {
        let value = value.into();
        self.style().size = Size::new(value, value);
        self
    }
    pub fn size_full(mut self) -> Self {
        self.style().size = Size::new(relative(1.), relative(1.));
        self
    }
    pub fn w_full(mut self) -> Self {
        self.style().size.width = relative(1.);
        self
    }
    pub fn min_w(mut self, value: impl Into<Length>) -> Self {
        self.style().min_size.width = value.into();
        self
    }
    pub fn min_h(mut self, value: impl Into<Length>) -> Self {
        self.style().min_size.height = value.into();
        self
    }
    pub fn max_w(mut self, value: impl Into<Length>) -> Self {
        self.style().max_size.width = value.into();
        self
    }
    pub fn max_h(mut self, value: impl Into<Length>) -> Self {
        self.style().max_size.height = value.into();
        self
    }
    pub fn mt(mut self, value: impl Into<Length>) -> Self {
        self.style().margin.top = value.into();
        self
    }
    pub fn mr(mut self, value: impl Into<Length>) -> Self {
        self.style().margin.right = value.into();
        self
    }
    pub fn mb(mut self, value: impl Into<Length>) -> Self {
        self.style().margin.bottom = value.into();
        self
    }
    pub fn ml(mut self, value: impl Into<Length>) -> Self {
        self.style().margin.left = value.into();
        self
    }
    pub fn mt_auto(mut self) -> Self {
        self.style().margin.top = Length::Auto;
        self
    }
    pub fn mr_auto(mut self) -> Self {
        self.style().margin.right = Length::Auto;
        self
    }
    pub fn mb_auto(mut self) -> Self {
        self.style().margin.bottom = Length::Auto;
        self
    }
    pub fn ml_auto(mut self) -> Self {
        self.style().margin.left = Length::Auto;
        self
    }
    pub fn pt(mut self, value: impl Into<Length>) -> Self {
        self.style().padding.top = value.into();
        self
    }
    pub fn pr(mut self, value: impl Into<Length>) -> Self {
        self.style().padding.right = value.into();
        self
    }
    pub fn pb(mut self, value: impl Into<Length>) -> Self {
        self.style().padding.bottom = value.into();
        self
    }
    pub fn pl(mut self, value: impl Into<Length>) -> Self {
        self.style().padding.left = value.into();
        self
    }
    pub fn gap_x(mut self, value: impl Into<Length>) -> Self {
        self.style().gap.width = value.into();
        self
    }
    pub fn gap_y(mut self, value: impl Into<Length>) -> Self {
        self.style().gap.height = value.into();
        self
    }
    pub fn border(mut self, value: Pixels) -> Self {
        self.style().border_widths = Edges::all(value);
        self
    }
    pub fn border_t(mut self, value: Pixels) -> Self {
        self.style().border_widths.top = value;
        self
    }
    pub fn border_r(mut self, value: Pixels) -> Self {
        self.style().border_widths.right = value;
        self
    }
    pub fn border_b(mut self, value: Pixels) -> Self {
        self.style().border_widths.bottom = value;
        self
    }
    pub fn border_l(mut self, value: Pixels) -> Self {
        self.style().border_widths.left = value;
        self
    }
    pub fn border_color(mut self, color: Rgba) -> Self {
        self.style().border_color = Some(color);
        self
    }
    pub fn rounded_full(mut self) -> Self {
        self.style().corner_radii = Corners::all(Length::Px(px(f32::MAX / 4.))).into();
        self
    }
    pub fn bg(mut self, color: Rgba) -> Self {
        self.style().background = Some(color);
        self
    }
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.style().opacity = Some(opacity);
        self
    }
    pub fn cursor(mut self, cursor: CursorStyle) -> Self {
        self.style().mouse_cursor = Some(cursor);
        self
    }
    pub fn overflow_hidden(mut self) -> Self {
        self.style().overflow = Point::new(Overflow::Hidden, Overflow::Hidden);
        self
    }
    pub fn overflow_y_scroll(mut self) -> Self {
        self.style().overflow.y = Overflow::Scroll;
        self
    }
    pub fn overflow_y_hidden(mut self) -> Self {
        self.style().overflow.y = Overflow::Hidden;
        self
    }
    pub fn restrict_scroll_to_axis(mut self) -> Self {
        self.style().restrict_scroll_to_axis = true;
        self
    }
    pub fn text_color(mut self, color: Rgba) -> Self {
        self.text_style().color = Some(color);
        self
    }
    pub fn text_size(mut self, size: Pixels) -> Self {
        self.text_style().font_size = Some(size);
        self
    }
    pub fn line_height(mut self, height: Pixels) -> Self {
        self.text_style().line_height = Some(height);
        self
    }
    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.text_style().font_family = Some(family.into());
        self
    }
    pub fn font_weight(mut self, weight: FontWeight) -> Self {
        self.text_style().font_weight = Some(weight);
        self
    }
    pub fn whitespace_nowrap(mut self) -> Self {
        self.text_style().white_space = Some(WhiteSpace::Nowrap);
        self
    }
    pub fn text_left(mut self) -> Self {
        self.text_style().text_align = Some(TextAlign::Left);
        self
    }
    pub fn text_center(mut self) -> Self {
        self.text_style().text_align = Some(TextAlign::Center);
        self
    }
    pub fn text_right(mut self) -> Self {
        self.text_style().text_align = Some(TextAlign::Right);
        self
    }
}

/// Paints the boxes with a z-index of the parent stacking context here: the negative ones, or the
/// others.
pub fn layers(negative: bool) -> Node {
    let mut node = node().absolute().w(px(0.)).h(px(0.));
    node.content = Content::Layers(negative);
    node
}

/// An image that fills its box.
pub fn img(image: Arc<RenderImage>) -> Node {
    let size = image.size();
    let mut node = node();
    if size.height.0 > 0 {
        node.style.aspect_ratio = Some(size.width.0 as f32 / size.height.0 as f32);
    }
    node.content = Content::Image(image, ObjectFit::Fill);
    node
}

/// Text in the inherited text style, or with its own runs that cover it.
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

/// A box that paints with closures each frame. Prepaint gets the bounds in layout units, so
/// positions it records match the boxes around it, and paint gets them snapped to device pixels
/// with what prepaint returned.
pub fn canvas<T: 'static>(
    prepaint: impl Fn(Bounds<Pixels>, &mut Window) -> T + 'static,
    paint: impl Fn(Bounds<Pixels>, T, &mut Window) + 'static,
) -> Node {
    node().hooks(Canvas {
        prepaint,
        paint,
        state: Cell::new(None),
    })
}

struct Canvas<P, F, T> {
    prepaint: P,
    paint: F,
    state: Cell<Option<T>>,
}

impl<T: 'static, P, F> Hooks for Canvas<P, F, T>
where
    P: Fn(Bounds<Pixels>, &mut Window) -> T + 'static,
    F: Fn(Bounds<Pixels>, T, &mut Window) + 'static,
{
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        self.state.set(Some((self.prepaint)(cx.exact, cx.window)));
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        inner(cx.window);
        if let Some(state) = self.state.take() {
            let (bounds, window) = (cx.bounds, &mut *cx.window);
            window.with_content_mask(cx.style.overflow_mask(bounds), |window| {
                (self.paint)(bounds, state, window)
            });
        }
    }
}

#[derive(Default)]
pub(crate) struct Events {
    mouse_down: Vec<(MouseButton, Rc<RefCell<Listener<MouseDownEvent>>>)>,
    mouse_move: Vec<Rc<RefCell<Listener<MouseMoveEvent>>>>,
    pub(crate) scroll: Option<ScrollHandle>,
    pub(crate) drag: bool,
}

impl Events {
    pub(crate) fn needs_hitbox(events: Option<&Self>, style: &Style) -> bool {
        style.mouse_cursor.is_some()
            || events.is_some_and(|events| {
                events.scroll.is_some()
                    || events.drag
                    || !events.mouse_down.is_empty()
                    || !events.mouse_move.is_empty()
            })
    }

    pub(crate) fn paint(
        &self,
        style: &Style,
        line_height: Pixels,
        hitbox: &Hitbox,
        window: &mut Window,
    ) {
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
        if let Some(handle) = &self.scroll {
            let (handle, hitbox) = (handle.clone(), hitbox.clone());
            let (overflow, restrict) = (style.overflow, style.restrict_scroll_to_axis);
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || !hitbox.should_handle_scroll(window)
                    || window.scroll_chain.as_ref().is_some_and(|chain| {
                        !chain.iter().any(|other| Rc::ptr_eq(&other.0, &handle.0))
                    })
                {
                    return;
                }
                let mut delta = event.delta.pixel_delta(line_height);
                if event.delta.precise() {
                    let [x, y] = window.rails.0;
                    if y >= x {
                        delta.x = Pixels::ZERO;
                    } else {
                        delta.y = Pixels::ZERO;
                    }
                }
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
    pub(crate) fn drive(&self, window: &mut Window) {
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

    pub(crate) fn clamp(
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

        let handle = ScrollHandle::default();
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
