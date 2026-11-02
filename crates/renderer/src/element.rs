use crate::{
    AvailableSpace, Bounds, LayoutId, ObjectFit, Pixels, Point, RenderImage, Size, Style, Styled,
    Window,
};
use std::{any::Any, mem, sync::Arc};

/// Something that can be laid out and painted. Every frame builds a new tree of elements.
///
/// The phases run in order: `request_layout` adds the element to the layout tree, `prepaint`
/// sees the bounds and inserts hitboxes, and `paint` draws.
pub trait Element: 'static {
    type RequestLayoutState: 'static;
    type PrepaintState: 'static;

    fn request_layout(&mut self, window: &mut Window) -> (LayoutId, Self::RequestLayoutState);

    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut Window,
    ) -> Self::PrepaintState;

    fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
    );
}

pub trait IntoElement: Sized {
    type Element: Element;
    fn into_element(self) -> Self::Element;
    fn into_any_element(self) -> AnyElement {
        AnyElement::new(self.into_element())
    }
}

pub trait ParentElement: Sized {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>);

    fn child(mut self, child: impl IntoElement) -> Self {
        self.extend([child.into_any_element()]);
        self
    }

    fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.extend(children.into_iter().map(IntoElement::into_any_element));
        self
    }
}

/// Applies `f` when `condition` holds, for builder chains.
pub trait FluentBuilder: Sized {
    fn when(self, condition: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if condition { f(self) } else { self }
    }
    fn map<U>(self, f: impl FnOnce(Self) -> U) -> U {
        f(self)
    }
}

impl<T: IntoElement> FluentBuilder for T {}

/// An identifier that keeps element state from one frame to the next.
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

enum Phase<L, P> {
    Start,
    Layout(LayoutId, L),
    Prepaint(Bounds<Pixels>, L, P),
    Done,
}

struct Drawable<E: Element> {
    element: E,
    phase: Phase<E::RequestLayoutState, E::PrepaintState>,
}

trait ElementObject {
    fn request_layout(&mut self, window: &mut Window) -> LayoutId;
    fn prepaint(&mut self, window: &mut Window);
    fn paint(&mut self, window: &mut Window);
    fn layout_id(&self) -> Option<LayoutId>;
    fn inner(&mut self) -> &mut dyn Any;
}

impl<E: Element> ElementObject for Drawable<E> {
    fn request_layout(&mut self, window: &mut Window) -> LayoutId {
        match mem::replace(&mut self.phase, Phase::Done) {
            Phase::Start => {
                let (id, state) = self.element.request_layout(window);
                self.phase = Phase::Layout(id, state);
                id
            }
            phase => {
                // WARNING: a second request reuses the first layout node instead of panicking.
                let id = match &phase {
                    Phase::Layout(id, _) => Some(*id),
                    _ => None,
                };
                self.phase = phase;
                id.unwrap_or_else(|| window.request_layout(&Style::default(), []))
            }
        }
    }

    fn prepaint(&mut self, window: &mut Window) {
        if let Phase::Layout(id, mut layout) = mem::replace(&mut self.phase, Phase::Done) {
            let bounds = window.layout_bounds(id);
            let prepaint = self.element.prepaint(bounds, &mut layout, window);
            self.phase = Phase::Prepaint(bounds, layout, prepaint);
        }
    }

    fn paint(&mut self, window: &mut Window) {
        if let Phase::Prepaint(bounds, mut layout, mut prepaint) =
            mem::replace(&mut self.phase, Phase::Done)
        {
            self.element
                .paint(bounds, &mut layout, &mut prepaint, window);
        }
    }

    fn layout_id(&self) -> Option<LayoutId> {
        match &self.phase {
            Phase::Layout(id, _) => Some(*id),
            _ => None,
        }
    }

    fn inner(&mut self) -> &mut dyn Any {
        &mut self.element
    }
}

/// An element of any type.
pub struct AnyElement(Box<dyn ElementObject>);

impl AnyElement {
    pub fn new<E: Element>(element: E) -> Self {
        Self(Box::new(Drawable {
            element,
            phase: Phase::Start,
        }))
    }

    pub fn downcast_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.0.inner().downcast_mut()
    }

    pub fn request_layout(&mut self, window: &mut Window) -> LayoutId {
        self.0.request_layout(window)
    }

    pub fn prepaint(&mut self, window: &mut Window) {
        self.0.prepaint(window)
    }

    pub fn paint(&mut self, window: &mut Window) {
        self.0.paint(window)
    }

    /// Lays the element out on its own in the given space and returns its size.
    pub fn layout_as_root(
        &mut self,
        space: Size<AvailableSpace>,
        window: &mut Window,
    ) -> Size<Pixels> {
        let id = match self.0.layout_id() {
            Some(id) => id,
            None => self.0.request_layout(window),
        };
        window.compute_layout(id, space);
        window.layout_bounds(id).size
    }

    /// Prepaints the element with its origin at `origin`, in window coordinates.
    pub fn prepaint_at(&mut self, origin: Point<Pixels>, window: &mut Window) {
        window.with_absolute_element_offset(origin, |window| self.prepaint(window))
    }
}

impl Element for AnyElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn request_layout(&mut self, window: &mut Window) -> (LayoutId, ()) {
        (AnyElement::request_layout(self, window), ())
    }

    fn prepaint(&mut self, _: Bounds<Pixels>, _: &mut (), window: &mut Window) {
        AnyElement::prepaint(self, window)
    }

    fn paint(&mut self, _: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window) {
        AnyElement::paint(self, window)
    }
}

impl IntoElement for AnyElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
    fn into_any_element(self) -> AnyElement {
        self
    }
}

type PrepaintFn<T> = Box<dyn FnOnce(Bounds<Pixels>, &mut Window) -> T>;
type PaintFn<T> = Box<dyn FnOnce(Bounds<Pixels>, T, &mut Window)>;

/// A box that paints with closures. Prepaint gets the bounds in layout units, so positions it records
/// match the boxes around it, and paint gets them snapped to device pixels.
pub struct Canvas<T> {
    prepaint: Option<PrepaintFn<T>>,
    paint: Option<PaintFn<T>>,
    style: Style,
}

pub fn canvas<T: 'static>(
    prepaint: impl FnOnce(Bounds<Pixels>, &mut Window) -> T + 'static,
    paint: impl FnOnce(Bounds<Pixels>, T, &mut Window) + 'static,
) -> Canvas<T> {
    Canvas {
        prepaint: Some(Box::new(prepaint)),
        paint: Some(Box::new(paint)),
        style: Style::default(),
    }
}

impl<T: 'static> IntoElement for Canvas<T> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl<T: 'static> Styled for Canvas<T> {
    fn style(&mut self) -> &mut Style {
        &mut self.style
    }
}

impl<T: 'static> Element for Canvas<T> {
    type RequestLayoutState = (Style, LayoutId);
    type PrepaintState = Option<T>;

    fn request_layout(&mut self, window: &mut Window) -> (LayoutId, (Style, LayoutId)) {
        let style = mem::take(&mut self.style);
        let id = window.request_layout(&style, []);
        (id, (style, id))
    }

    fn prepaint(
        &mut self,
        _: Bounds<Pixels>,
        (_, id): &mut (Style, LayoutId),
        window: &mut Window,
    ) -> Option<T> {
        let bounds = window.exact_layout_bounds(*id);
        Some(self.prepaint.take()?(bounds, window))
    }

    fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        (style, _): &mut (Style, LayoutId),
        state: &mut Option<T>,
        window: &mut Window,
    ) {
        let (Some(state), Some(paint)) = (state.take(), self.paint.take()) else {
            return;
        };
        window.with_content_mask(style.overflow_mask(bounds), |window| {
            paint(bounds, state, window)
        });
    }
}

/// An empty element.
pub struct Empty;

impl IntoElement for Empty {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Empty {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn request_layout(&mut self, window: &mut Window) -> (LayoutId, ()) {
        (window.request_layout(&Style::default(), []), ())
    }
    fn prepaint(&mut self, _: Bounds<Pixels>, _: &mut (), _: &mut Window) {}
    fn paint(&mut self, _: Bounds<Pixels>, _: &mut (), _: &mut (), _: &mut Window) {}
}

/// An image that fills its box.
pub struct Img {
    image: Arc<RenderImage>,
    fit: ObjectFit,
    style: Style,
}

pub fn img(image: Arc<RenderImage>) -> Img {
    Img {
        image,
        fit: ObjectFit::Fill,
        style: Style::default(),
    }
}

impl Img {
    pub fn object_fit(mut self, fit: ObjectFit) -> Self {
        self.fit = fit;
        self
    }
}

impl Styled for Img {
    fn style(&mut self) -> &mut Style {
        &mut self.style
    }
}

impl IntoElement for Img {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Img {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn request_layout(&mut self, window: &mut Window) -> (LayoutId, ()) {
        let size = self.image.size();
        if self.style.aspect_ratio.is_none() && size.height.0 > 0 {
            self.style.aspect_ratio = Some(size.width.0 as f32 / size.height.0 as f32);
        }
        (window.request_layout(&self.style, []), ())
    }

    fn prepaint(&mut self, _: Bounds<Pixels>, _: &mut (), _: &mut Window) {}

    fn paint(&mut self, bounds: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window) {
        let shown = self.fit.get_bounds(bounds, self.image.size());
        let corners = self.style.corner_radii.resolve(bounds.size);
        let mask = self.style.overflow_mask(bounds);
        window.with_content_mask(mask, |window| {
            window.paint_image(shown, bounds, corners, self.image.clone(), false)
        });
    }
}
