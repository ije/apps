pub(crate) mod background;
pub(crate) mod control;
pub(crate) mod field;
pub(crate) mod mask;
pub(crate) mod native;
pub(crate) mod scrollbar;
pub(crate) mod spacer;
pub(crate) mod terminal;
pub(crate) mod text;

use crate::css;
use crate::css::properties::Prop;
use renderer::{Child, Hooks, NodeCx, Pixels, Point, Rect, Size, Window, point, px};
use std::{cell::Cell, rc::Rc};

pub(crate) struct Turned {
    clockwise: bool,
    block: f32,
    inline: f32,
    matrix: Cell<crate::css::value::transform::Matrix>,
}

impl Turned {
    pub(crate) fn node(child: Child, clockwise: bool, block: f32, inline: f32) -> renderer::Node {
        let mut style = renderer::Style {
            position: renderer::Position::Absolute,
            ..Default::default()
        };
        style.size.width = px(block).into();
        style.size.height = px(inline).into();
        let mut node = renderer::node().child(child).hooks(Turned {
            clockwise,
            block,
            inline,
            matrix: Cell::new(crate::css::value::transform::IDENTITY),
        });
        *node.style() = style;
        node
    }

    fn matrix(&self, origin: Point<Pixels>) -> crate::css::value::transform::Matrix {
        let [x, y] = [f32::from(origin.x), f32::from(origin.y)];
        let mut m = crate::css::value::transform::IDENTITY;
        if self.clockwise {
            [m[0], m[1], m[4], m[5]] = [0., 1., -1., 0.];
            [m[12], m[13]] = [x + self.block + y, y - x];
        } else {
            [m[0], m[1], m[4], m[5]] = [0., -1., 1., 0.];
            [m[12], m[13]] = [x - y, y + self.inline + x];
        }
        m
    }
}

impl Hooks for Turned {
    fn cached(&self) -> bool {
        true
    }

    // Inside a box that a scroll moved, the renderer moves its hit transform.
    fn shifts(&self) -> bool {
        true
    }

    // The matrix follows the bounds, so a kept box places its hit transform again.
    fn volatile(&self) -> bool {
        false
    }

    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.prepaint(cx, inner)
    }

    fn skippable(&self) -> bool {
        true
    }

    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let matrix = self.matrix(cx.bounds.origin);
        self.matrix.set(matrix);
        cx.window.with_keyed_hit_transform(matrix, None, inner);
    }
    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let effect = renderer::PaintEffect {
            transform: Some(self.matrix.get()),
            ..Default::default()
        };
        cx.window.with_effect(effect, inner);
    }
}

pub(crate) struct Transformed {
    pub(crate) style: Rc<css::properties::Style>,
    pub(crate) units: css::value::units::Lengths,
    pub(crate) matrix: Cell<Option<crate::css::value::transform::Matrix>>,
    pub(crate) local: super::effects::transform::Local,
}

impl Hooks for Transformed {
    fn cached(&self) -> bool {
        true
    }

    // Inside a box that a scroll moved, the renderer moves its hit transform.
    fn shifts(&self) -> bool {
        true
    }

    // The matrix follows the bounds, so a kept box places its hit transform again.
    fn volatile(&self) -> bool {
        false
    }

    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.prepaint(cx, inner)
    }

    fn skippable(&self) -> bool {
        true
    }

    fn reach(&self, size: Size<Pixels>, corners: [Point<Pixels>; 2]) -> [Point<Pixels>; 2] {
        super::effects::transform::reach(&self.local, &self.style, &self.units, size, corners)
    }

    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let matrix = super::effects::transform::transform_at(
            &self.local,
            &self.style,
            &self.units,
            cx.bounds,
        );
        self.matrix.set(matrix);
        match matrix {
            Some(matrix) => cx.window.with_keyed_hit_transform(matrix, None, inner),
            None => inner(cx.window),
        }
    }
    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        match self.matrix.get() {
            Some(matrix) => {
                let effect = renderer::PaintEffect {
                    transform: Some(matrix),
                    ..Default::default()
                };
                cx.window.with_effect(effect, inner);
            }
            None => inner(cx.window),
        }
    }
}

pub(crate) struct Effected {
    pub(crate) effect: renderer::PaintEffect,
    pub(crate) backdrop: Option<renderer::Radii<renderer::Length>>,
}

impl Hooks for Effected {
    fn volatile(&self) -> bool {
        false
    }

    fn skippable(&self) -> bool {
        true
    }

    fn skip(&self, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shift(&self, _: Point<Pixels>, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shifts(&self) -> bool {
        true
    }

    // Frames patch the layers of composited animations.
    fn cached(&self) -> bool {
        self.effect.key.is_none()
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let mut effect = self.effect.clone();
        if let Some(backdrop) = effect.backdrop.as_mut()
            && let Some(radii) = &self.backdrop
        {
            backdrop.bounds = cx.bounds;
            backdrop.corner_radii = radii.resolve(cx.bounds.size);
        }
        cx.window.with_effect(effect, inner);
    }
}

// The mask layers of a pseudo-element.
pub(crate) struct Masked {
    pub(crate) view: Rc<super::ViewState>,
    pub(crate) key: (usize, u8),
    pub(crate) style: Rc<css::properties::Style>,
    pub(crate) units: css::value::units::Lengths,
}

impl Hooks for Masked {
    fn volatile(&self) -> bool {
        false
    }

    // Nothing draws outside the mask image.
    fn bounded(&self) -> bool {
        true
    }

    fn skippable(&self) -> bool {
        true
    }

    fn skip(&self, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shift(&self, _: Point<Pixels>, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shifts(&self) -> bool {
        true
    }

    fn cached(&self) -> bool {
        true
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let mask = {
            let mut store = self.view.store.borrow_mut();
            let (_, state, app) = store.page_parts(self.view.id);
            mask::mask(
                &self.view,
                state,
                app,
                self.key,
                &self.style,
                self.units,
                None,
                cx.bounds,
                cx.window,
            )
        };
        let effect = renderer::PaintEffect {
            mask,
            ..Default::default()
        };
        cx.window.with_effect(effect, inner);
    }
}

// The children of a canvas with drawable content. The window doesn't show them, and they take no pointer
// until the page draws them.
pub(crate) struct Undrawn(pub(crate) Rc<super::ViewState>);

impl Hooks for Undrawn {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        // The drawable boxes inside find the content box of their canvas here.
        let outer = self.0.drawing.replace(cx.bounds.origin);
        cx.window.with_hit_base(cx.bounds, |window| {
            window.with_hits_at(None, |window| inner(window))
        });
        self.0.drawing.set(outer);
    }
    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        cx.window.with_snapshot(None, cx.bounds, inner);
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Source {
    Element,
    Presented,
    Drawn,
}

pub(crate) struct Picture {
    pub(crate) id: usize,
    pub(crate) view: Rc<super::ViewState>,
    pub(crate) source: Source,
    pub(crate) fit: renderer::ObjectFit,
    pub(crate) cropped: Option<renderer::Radii<Pixels>>,
    /// `object-position` when it isn't centered.
    pub(crate) position: Option<(Rc<str>, css::value::units::Lengths)>,
    pub(crate) pixelated: bool,
}

impl Hooks for Picture {
    fn volatile(&self) -> bool {
        false
    }

    fn skippable(&self) -> bool {
        true
    }

    fn skip(&self, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shift(&self, _: Point<Pixels>, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }

    fn shifts(&self) -> bool {
        true
    }

    // Canvases and video frames change without a new box. A new image repaints the window.
    fn cached(&self) -> bool {
        matches!(self.source, Source::Element)
    }

    fn bounded(&self) -> bool {
        true
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let view = &*self.view;
        let image = view.store.try_borrow().ok().and_then(|store| {
            let dom = store.page_state(view.id);
            match self.source {
                Source::Element => dom?.media.images.get(&self.id).cloned(),
                Source::Presented => view
                    .images
                    .borrow()
                    .get(&self.id)
                    .or_else(|| dom?.media.images.get(&self.id))
                    .cloned(),
                Source::Drawn => dom?
                    .media
                    .canvases
                    .get(&self.id)
                    .map(|canvas| canvas.image.clone()),
            }
        });
        if let Some(image) = image
            && image.size().width > 0
            && image.size().height > 0
        {
            let bounds = cx.bounds;
            let mut fitted = self.fit.place(bounds, image.size());
            if let Some((position, units)) = &self.position {
                let free = bounds.size - fitted.size;
                let [x, y] = super::effects::gradient::place(
                    position,
                    [f32::from(free.width), f32::from(free.height)],
                    units,
                );
                fitted.origin = bounds.origin + point(px(x), px(y));
            }
            match self.cropped {
                Some(corners) => {
                    cx.window
                        .draw_image(bounds, fitted, corners, image, false, self.pixelated)
                }
                None => {
                    let radii = cx.style.corner_radii.resolve(bounds.size);
                    cx.window
                        .draw_image(fitted, fitted, radii, image, false, self.pixelated);
                }
            }
        }
        inner(cx.window);
    }
}

pub(crate) struct Sticky {
    pub(crate) scroll: renderer::ScrollRef,
    pub(crate) border: [f32; 4],
    pub(crate) block: Option<(usize, [f32; 4])>,
}

// The layout place of an element's border box, which its `::before` and `::after` boxes measure from.
pub(crate) struct ElementOrigin(pub(crate) Rc<Cell<Point<Pixels>>>);

impl Hooks for ElementOrigin {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.0.set(cx.exact.origin);
        inner(cx.window)
    }
    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.0.set(cx.exact.origin);
        inner(cx.window)
    }
    fn volatile(&self) -> bool {
        false
    }
    fn bounded(&self) -> bool {
        true
    }
    fn cached(&self) -> bool {
        true
    }
    fn skippable(&self) -> bool {
        true
    }
    fn skip(&self, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }
    fn shifts(&self) -> bool {
        true
    }
    fn shift(&self, _: Point<Pixels>, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }
}

/// A block pseudo-element of a spliced inline box, which gives the box a part. The box of an ancestor that a
/// scroll moves moves the part with the box's other pieces.
pub(crate) struct PseudoPart {
    pub(crate) view: Rc<super::ViewState>,
    pub(crate) node: usize,
    pub(crate) key: usize,
}

impl PseudoPart {
    fn place(&self, exact: Rect<Pixels>, window: &Window) {
        let bounds = window.transformed_bounds(exact);
        let rect = [
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        ]
        .map(f32::from);
        let view = &*self.view;
        let mut store = view.store.borrow_mut();
        let dom = store.state(view.id);
        let pieces = dom.geometry.pieces.entry(self.node).or_default();
        match pieces.iter_mut().find(|(key, _)| *key == self.key) {
            Some((_, old)) => *old = rect,
            None => pieces.push((self.key, rect)),
        }
    }
}

impl Hooks for PseudoPart {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.place(cx.exact, cx.window);
        inner(cx.window)
    }
    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        self.place(cx.exact, cx.window);
        inner(cx.window)
    }
    fn volatile(&self) -> bool {
        false
    }
    fn bounded(&self) -> bool {
        true
    }
    fn cached(&self) -> bool {
        true
    }
    fn skippable(&self) -> bool {
        true
    }
    fn skip(&self, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }
    fn shifts(&self) -> bool {
        true
    }
    fn shift(&self, _: Point<Pixels>, exact: Rect<Pixels>, window: &mut Window) -> bool {
        self.place(exact, window);
        true
    }
}

// Keeps the box of a `::before` or `::after` for `getComputedStyle()`: its layout place from the border box of
// its element without the scroll of the element, its size, and its padding plus border. Moves of the element
// don't change them, so only prepaint writes them.
pub(crate) struct PseudoBox {
    pub(crate) view: Rc<super::ViewState>,
    pub(crate) element: usize,
    pub(crate) origin: Rc<Cell<Point<Pixels>>>,
    // The move of a `PseudoBlock` around it.
    pub(crate) shift: Option<Rc<Cell<Point<Pixels>>>>,
    // The element is a part of the text of its block, so its place is in the layout mirror, not in `origin`.
    pub(crate) mirrored: bool,
    pub(crate) key: usize,
}

impl Hooks for PseudoBox {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let view = &*self.view;
        let scale = cx.window.scale_factor();
        let mut store = view.store.borrow_mut();
        let dom = store.state(view.id);
        let scroll = dom
            .geometry
            .scroll
            .get(&self.element)
            .map_or(Point::default(), |handle| {
                crate::dom::geometry::painted_offset(handle, scale)
            });
        let shift = self
            .shift
            .as_ref()
            .map_or(Point::default(), |shift| shift.get());
        let origin = match self.mirrored {
            true => (dom.geometry.mirror.get(&self.element))
                .map_or(Point::default(), |&[x, y, ..]| point(px(x), px(y))),
            false => self.origin.get(),
        };
        let at = cx.exact.origin + shift - origin - scroll;
        let [x, y] = [at.x, at.y].map(f32::from);
        let [w, h] = [cx.exact.size.width, cx.exact.size.height].map(f32::from);
        let [a, b, c, d] = cx.inset.map(f32::from);
        dom.layout
            .pseudo_boxes
            .insert(self.key, [x, y, w, h, a, b, c, d]);
        drop(store);
        inner(cx.window)
    }
    fn volatile(&self) -> bool {
        false
    }
    fn bounded(&self) -> bool {
        true
    }
    fn cached(&self) -> bool {
        true
    }
    fn skippable(&self) -> bool {
        true
    }
    fn skip(&self, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }
    fn shifts(&self) -> bool {
        true
    }
    fn shift(&self, _: Point<Pixels>, _: Rect<Pixels>, _: &mut Window) -> bool {
        true
    }
}

// The insets of an out-of-flow box whose containing block is an ancestor of its layout parent.
pub(crate) struct Contained {
    // Tells mismatches of this box from those of others.
    pub(crate) key: usize,
    pub(crate) container: usize,
    // The window, with the scroll bars of the page.
    pub(crate) viewport: [f32; 2],
    // The containing block size that the build used, when it came from another box.
    pub(crate) sized: Option<[f32; 2]>,
    pub(crate) inset: [Option<f32>; 4],
    pub(crate) margin: [f32; 4],
    // The containing block is right-to-left, so `right` wins over `left`.
    pub(crate) rtl: bool,
}

impl Contained {
    // The padding box of the containing block, where the content of its scroll is.
    fn block(&self, view: &super::ViewState) -> Option<Rect<Pixels>> {
        let store = view.store.borrow();
        let dom = store.page_state(view.id)?;
        match self.container {
            container @ (0 | usize::MAX) => {
                let scroll = dom.geometry.scroll.get(&0);
                let room = scroll.map_or_else(Default::default, |scroll| scroll.room());
                let [width, height] = self.viewport.map(px);
                let size = renderer::size(width - room.right, height - room.bottom);
                let origin = match container {
                    0 => scroll.map(|scroll| scroll.bounds().origin + scroll.painted())?,
                    _ => Point::default(),
                };
                Some(Rect::new(origin, size))
            }
            // The boxes that a scroll container contains move with its scroll.
            container => dom.layout.blocks.get(&container).map(|block| {
                let scroll = dom.geometry.scroll.get(&container);
                *block + scroll.map_or(Point::default(), |s| s.painted())
            }),
        }
    }

    // The move that places `bounds` by the insets in the padding box `block`.
    fn offset(&self, block: Rect<Pixels>, bounds: Rect<Pixels>) -> Point<Pixels> {
        let [top, right, bottom, left] = self.inset;
        let [margin_top, margin_right, margin_bottom, margin_left] = self.margin.map(px);
        let mut shift = point(px(0.), px(0.));
        if let Some(right) = right.filter(|_| self.rtl || left.is_none()) {
            shift.x = block.right() - px(right) - margin_right - bounds.right();
        } else if let Some(left) = left {
            shift.x = block.left() + px(left) + margin_left - bounds.left();
        }
        if let Some(top) = top {
            shift.y = block.top() + px(top) + margin_top - bounds.top();
        } else if let Some(bottom) = bottom {
            shift.y = block.bottom() - px(bottom) - margin_bottom - bounds.bottom();
        }
        shift
    }

    // The move that places `bounds` by the insets in the padding box of the containing block.
    fn shift(
        &self,
        view: &super::ViewState,
        bounds: Rect<Pixels>,
        window: &mut Window,
    ) -> Point<Pixels> {
        let Some(block) = self.block(view) else {
            return Point::default();
        };
        // Redraw only once per mismatch to avoid endless frames when layout repeats the same size.
        if let Some([width, height]) = self.sized
            && ((f32::from(block.size.width) - width).abs() > 0.5
                || (f32::from(block.size.height) - height).abs() > 0.5)
        {
            let key = (
                self.key,
                [
                    width,
                    height,
                    block.size.width.into(),
                    block.size.height.into(),
                ]
                .map(f32::to_bits),
            );
            let [current, seen] = &mut *view.mismatched.borrow_mut();
            if current.insert(key) && !seen.contains(&key) {
                window.redraw();
            }
        }
        self.offset(block, bounds)
    }
}

// A box at its static place moves only with the scrolls of its containing block and outside it.
fn unscroll(offset: &mut Point<Pixels>, unplaced: [bool; 2], by: Point<Pixels>) {
    if unplaced[0] {
        offset.x -= by.x;
    }
    if unplaced[1] {
        offset.y -= by.y;
    }
}

// Moves an absolute or fixed `::before` or `::after` into its containing block, when that is not its element.
pub(crate) struct PseudoBlock {
    pub(crate) view: Rc<super::ViewState>,
    pub(crate) place: Option<Contained>,
    // The axes without insets.
    pub(crate) unplaced: [bool; 2],
    pub(crate) offset: Rc<Cell<Point<Pixels>>>,
}

impl Hooks for PseudoBlock {
    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let mut bounds = cx.exact;
        let mut offset = Point::default();
        unscroll(&mut offset, self.unplaced, cx.unscroll);
        bounds.origin += offset;
        if let Some(place) = &self.place {
            offset += place.shift(&self.view, bounds, cx.window);
        }
        self.offset.set(offset);
        cx.window
            .scope(renderer::Scope::Offset(offset), |window| inner(window));
    }
    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let offset = self.offset.get();
        cx.window
            .scope(renderer::Scope::Offset(offset), |window| inner(window));
    }
    fn volatile(&self) -> bool {
        true
    }
    fn bounded(&self) -> bool {
        true
    }
    fn cached(&self) -> bool {
        true
    }
}

// The distance and the origin of the perspective of a parent, its node and its borders.
pub(crate) type Perspective = ((f32, String), usize, [f32; 4]);

pub(crate) struct DomBox {
    // The axes without insets of an out-of-flow box.
    pub(crate) unplaced: [bool; 2],
    pub(crate) id: usize,
    pub(crate) plans: u8,
    pub(crate) ids: Rc<super::effects::Ids>,
    pub(crate) view: Rc<super::ViewState>,
    pub(crate) sticky: Option<Box<Sticky>>,
    pub(crate) fixed: bool,
    pub(crate) block: Option<[f32; 4]>,
    pub(crate) container: Option<usize>,
    // The containing block is right-to-left, so `right` wins over `left`.
    pub(crate) rtl: bool,
    pub(crate) sized: Option<[f32; 2]>,
    pub(crate) inset: [Option<f32>; 4],
    pub(crate) style: Rc<css::properties::Style>,
    pub(crate) effects: Option<f32>,
    pub(crate) composited: bool,
    pub(crate) transformed: bool,
    pub(crate) perspective: Option<Box<Perspective>>,
    pub(crate) space: renderer::Space,
    pub(crate) units: css::value::units::Lengths,
    pub(crate) revision: u64,
    pub(crate) sides: [f32; 2],
    pub(crate) margin: [f32; 4],
    pub(crate) cell: Option<usize>,
    pub(crate) grid: Option<(usize, bool)>,
    pub(crate) parts: Option<Rc<Parts>>,
    pub(crate) shared: Option<[f32; 4]>,
    pub(crate) leading: Option<[f32; 3]>,
    pub(crate) boxless: bool,
    pub(crate) text: Option<([u32; 2], [f32; 2])>,
    pub(crate) placed: Cell<(Rect<Pixels>, Option<crate::css::value::transform::Matrix>)>,
    // The window bounds and cull flags of the last prepaint, which a skip gives to `merge`.
    pub(crate) reached_at: Cell<(Rect<Pixels>, u32)>,
    pub(crate) offset: Cell<Point<Pixels>>,
    // The part of `offset` from the containing block.
    pub(crate) based: Cell<Point<Pixels>>,
    pub(crate) frame: bool,
    // Scrolls move or hide the box with its anchor.
    pub(crate) anchored: bool,
    pub(crate) hidden: Cell<bool>,
    pub(crate) text_clip: bool,
    // A drawable element of a canvas, which paints into a snapshot for `drawElementImage()`.
    pub(crate) drawn: bool,
    // The transform at the origin for a size, which a moved box places again.
    pub(crate) local: super::effects::transform::Local,
}

// With its counts, a box's `Rc` must stay within the small blocks of the runtime's allocator.
const _: () = assert!(size_of::<DomBox>() + 16 <= 512);

/// Boxes whose bounds join the bounds of others after a prepaint.
#[cfg_attr(debug_assertions, derive(Debug))]
pub(crate) enum Parts {
    /// The rows and row groups of a table, with their cells or rows.
    Rows(Vec<(usize, bool, Vec<usize>)>),
    /// The spliced inline boxes of a block, with their parts, and the left and right sides of its content box,
    /// which a run of blocks spans. In columns, a run of blocks spans its blocks.
    Cuts {
        sides: Option<[f32; 2]>,
        boxes: Vec<(usize, Vec<Piece>)>,
    },
}

/// A part of a spliced inline box.
#[cfg_attr(debug_assertions, derive(Debug))]
pub(crate) enum Piece {
    /// The part in a line of text, by the key of the text.
    Text(usize),
    /// A run of blocks, from the first to the last, and the bottom margin of the last. A block pseudo-element is
    /// its key.
    Blocks(usize, usize, f32),
    /// Boxes that a line of boxes holds.
    Nodes(Vec<usize>),
}

impl DomBox {
    fn block(&self, bounds: Rect<Pixels>, translation: Point<Pixels>) {
        if let Some([top, right, bottom, left]) = self.block {
            let view = &*self.view;
            let mut store = view.store.borrow_mut();
            let mut padding = Rect::new(
                bounds.origin + point(px(left), px(top)),
                renderer::size(
                    bounds.size.width - px(left + right),
                    bounds.size.height - px(top + bottom),
                ),
            );
            padding.origin += translation;
            store.state(view.id).layout.blocks.insert(self.id, padding);
        }
    }

    fn mirror(
        &self,
        dom: &mut crate::render::PageState,
        window_bounds: Rect<Pixels>,
        shape: Option<Box<[Point<Pixels>]>>,
        size: Size<Pixels>,
        parts: &[Rect<Pixels>],
    ) {
        let mirror = &mut dom.geometry.mirror;
        let [mut top, right, mut bottom, left] = self.shared.unwrap_or_default();
        if let Some([line, above, below]) = self.leading
            && (f32::from(window_bounds.size.height) - line).abs() < 0.5
        {
            (top, bottom) = (above, below);
        }
        mirror.insert(
            self.id,
            if self.boxless {
                [0.; 4]
            } else {
                [
                    f32::from(window_bounds.origin.x) + left,
                    f32::from(window_bounds.origin.y) + top,
                    f32::from(window_bounds.size.width) - left - right,
                    f32::from(window_bounds.size.height) - top - bottom,
                ]
            },
        );
        let scaled = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() > 0.01;
        mirror.transformed(
            self.id,
            scaled(window_bounds.size.width, size.width)
                || scaled(window_bounds.size.height, size.height)
                || !self.drawn
                    && (self.transformed
                        || super::effects::TRANSFORM_PROPS
                            .into_iter()
                            .any(|prop| self.style.get(prop).is_some_and(|value| value != "none"))),
        );
        let shapes = &mut dom.geometry.shapes;
        if let Some(shape) = shape {
            shapes.insert(self.id, shape);
        } else if !shapes.is_empty() {
            shapes.remove(&self.id);
        }
        // Like Chrome, a box that columns split reports the union of its parts.
        if let Some(union) = parts.iter().copied().reduce(|all, part| all.union(part)) {
            let rect = |part: Rect<Pixels>| {
                [
                    part.origin.x,
                    part.origin.y,
                    part.size.width,
                    part.size.height,
                ]
                .map(f32::from)
            };
            mirror.insert(self.id, rect(union));
            dom.geometry
                .fragments
                .insert(self.id, parts.iter().copied().map(rect).collect());
        } else if !dom.geometry.fragments.is_empty() {
            dom.geometry.fragments.remove(&self.id);
        }
    }

    fn unions(&self) {
        let view = &*self.view;
        if let Some(([first, last], [above, below])) = self.text {
            let mut store = view.store.borrow_mut();
            let bounds = &mut store.state(view.id).geometry.mirror;
            if let (Some(&[x, _, width, _]), Some(&[_, top, ..]), Some(&[_, y, _, height])) = (
                bounds.get(&self.id),
                bounds.get(&(first as usize)),
                bounds.get(&(last as usize)),
            ) && width > 0.
            {
                bounds.insert(
                    self.id,
                    [x, top - above, width, y + height - top + above + below],
                );
            }
        }
        if let Some(Parts::Cuts { sides, boxes }) = self.parts.as_deref() {
            let mut store = view.store.borrow_mut();
            let dom = store.state(view.id);
            if let Some(&[x, _, width, _]) = dom.geometry.mirror.get(&self.id) {
                let union = |rects: &mut dyn Iterator<Item = [f32; 4]>| {
                    rects.reduce(|[x, y, w, h], [a, b, c, d]| {
                        let [left, top] = [x.min(a), y.min(b)];
                        [
                            left,
                            top,
                            (x + w).max(a + c) - left,
                            (y + h).max(b + d) - top,
                        ]
                    })
                };
                // A block pseudo-element is a piece of its box, by its key.
                let rect = |key: usize| match key >> 30 {
                    0 => dom.geometry.mirror.get(&key).copied(),
                    _ => dom
                        .geometry
                        .pieces
                        .get(&(key & ((1 << 30) - 1)))?
                        .iter()
                        .find_map(|&(other, rect)| (other == key).then_some(rect)),
                };
                let start = x + sides.map_or(0., |[left, _]| left);
                let mut found = Vec::with_capacity(boxes.len());
                for (node, pieces) in boxes {
                    let mut parts: Vec<[f32; 4]> = Vec::new();
                    for piece in pieces {
                        match piece {
                            Piece::Text(key) => parts.extend(
                                (dom.geometry.pieces.get(node).into_iter().flatten())
                                    .filter(|(other, _)| other == key)
                                    .map(|(_, rect)| *rect),
                            ),
                            // Like Chrome, a run of blocks is one part as wide as the content box.
                            &Piece::Blocks(first, last, _) => {
                                let Some([left, top, across, height]) =
                                    rect(first).zip(rect(last)).and_then(|(first, last)| {
                                        union(&mut [first, last].into_iter())
                                    })
                                else {
                                    continue;
                                };
                                parts.push(match sides {
                                    Some([left, right]) => {
                                        [x + left, top, width - left - right, height]
                                    }
                                    None => [left, top, across, height],
                                });
                            }
                            Piece::Nodes(nodes) => parts.extend(union(
                                &mut nodes
                                    .iter()
                                    .filter_map(|node| dom.geometry.mirror.get(node).copied())
                                    .filter(|rect| rect[2] > 0. || rect[3] > 0.),
                            )),
                        }
                    }
                    // Like Chrome, a box that starts or ends with a block has an empty part there.
                    let blocks = |piece: Option<&Piece>| matches!(piece, Some(Piece::Blocks(..)));
                    if let (true, Some(&[_, top, ..])) = (blocks(pieces.first()), parts.first()) {
                        parts.insert(0, [start, top, 0., 0.]);
                    }
                    // It comes after the bottom margin of the last block.
                    if let (Some(&Piece::Blocks(.., margin)), Some(&[_, top, _, height])) =
                        (pieces.last(), parts.last())
                    {
                        parts.push([start, top + height + margin.max(0.), 0., 0.]);
                    }
                    // Like Chrome, an empty part is not in the union.
                    let all = union(&mut parts.iter().copied().filter(|part| part[2] > 0.));
                    found.push((*node, all, parts));
                }
                for ((node, all, parts), (_, pieces)) in found.into_iter().zip(boxes) {
                    dom.geometry.shapes.remove(&node);
                    if let Some(all) = all {
                        dom.geometry.mirror.insert(node, all);
                    }
                    dom.geometry.fragments.insert(node, parts.into());
                    // Lines of text that the box left keep no part.
                    if let Some(kept) = dom.geometry.pieces.get_mut(&node) {
                        kept.retain(|&(key, _)| {
                            key == node | 1 << 30
                                || key == node | 2 << 30
                                || pieces.iter().any(
                                    |piece| matches!(piece, Piece::Text(other) if *other == key),
                                )
                        });
                    }
                }
            }
        }
        if let Some(Parts::Rows(rows)) = self.parts.as_deref() {
            let mut store = view.store.borrow_mut();
            let bounds = &mut store.state(view.id).geometry.mirror;
            // Rows and row groups span all columns of the table. A row ends where its first cell that
            // spans no rows below ends.
            let mut across = [f32::INFINITY, f32::NEG_INFINITY];
            for (part, row, members) in rows {
                let union = members
                    .iter()
                    .filter_map(|member| bounds.get(member).copied())
                    .reduce(|[x, y, w, h], [a, b, c, d]| {
                        let [left, top] = [x.min(a), y.min(b)];
                        let bottom = if *row {
                            (y + h).min(b + d)
                        } else {
                            (y + h).max(b + d)
                        };
                        [left, top, (x + w).max(a + c) - left, bottom - top]
                    });
                if let Some(union) = union {
                    across = [across[0].min(union[0]), across[1].max(union[0] + union[2])];
                    bounds.insert(*part, union);
                }
            }
            for (part, ..) in rows {
                if let Some(rect) = bounds.get_mut(part) {
                    [rect[0], rect[2]] = [across[0], across[1] - across[0]];
                }
            }
        }
    }

    fn reached(&self, window_bounds: Rect<Pixels>, flags: u32) {
        self.reached_at.set((window_bounds, flags));
        let view = &*self.view;
        let none = [f32::INFINITY, f32::NEG_INFINITY];
        let found = view
            .reaching
            .borrow_mut()
            .pop()
            .map_or(none, |(reach, _)| reach);
        let reach = if self.plans == u8::MAX { none } else { found };
        let mut reached = view.reach.borrow_mut();
        if reached.len() <= self.id {
            reached.resize(self.id + 1, [f32::NAN; 2]);
        }
        reached[self.id] = reach;
        drop(reached);
        self.merge(reach, window_bounds, flags);
    }

    // Adds the reach of the box to the box around it.
    fn merge(&self, reach: [f32; 2], window_bounds: Rect<Pixels>, flags: u32) {
        let mut reaching = self.view.reaching.borrow_mut();
        if let Some(([bottom, top], plans)) = reaching.last_mut() {
            *bottom = bottom.min(reach[0]);
            *top = top.max(reach[1]);
            if matches!(*plans, 1 | 2)
                && flags & (super::layout::cull::PLAIN_BLOCK | super::layout::cull::UNCULLABLE)
                    == super::layout::cull::PLAIN_BLOCK
                && (*plans == 1 || flags & super::layout::cull::DEFINITE_SIZE == 0)
            {
                *bottom = bottom.min(f32::from(window_bounds.bottom()));
                *top = top.max(f32::from(window_bounds.top()));
            }
        }
    }
}

impl DomBox {
    fn contained(&self, inset: [Option<f32>; 4]) -> Option<Contained> {
        Some(Contained {
            key: self.id,
            container: self.container?,
            viewport: self.units.viewport,
            sized: self.sized,
            inset,
            margin: self.margin,
            rtl: self.rtl,
        })
    }

    // The containing block of an out-of-flow box is where the last prepaint found it, seen from the box at `exact`,
    // and has the size that the build used. A scroll moves both, and a layout that moves or sizes only one of them
    // does not.
    fn based(&self, exact: Rect<Pixels>, unscrolled: Point<Pixels>) -> bool {
        let Some(contained) = self.contained(self.inset) else {
            return true;
        };
        let block = contained.block(&self.view);
        let mut offset = Point::default();
        unscroll(&mut offset, self.unplaced, unscrolled);
        if let Some(block) = block {
            offset += contained.offset(block, Rect::new(exact.origin + offset, exact.size));
        }
        let based = self.based.get();
        let sized = (self.sized.zip(block)).is_none_or(|([width, height], block)| {
            (f32::from(block.size.width) - width).abs() <= 0.5
                && (f32::from(block.size.height) - height).abs() <= 0.5
        });
        sized && (offset.x - based.x).0.abs() < 0.01 && (offset.y - based.y).0.abs() < 0.01
    }
}

impl Hooks for DomBox {
    // Of the effect layers, only filters draw outside the boxes inside.
    fn bounded(&self) -> bool {
        self.placed.get().1.is_none()
            && !self.drawn
            && (self.effects.is_none()
                || [Prop::FILTER, Prop::WEBKIT_FILTER]
                    .into_iter()
                    .all(|prop| self.style.get(prop).is_none_or(|value| value == "none")))
    }

    fn reach(&self, size: Size<Pixels>, corners: [Point<Pixels>; 2]) -> [Point<Pixels>; 2] {
        if self.drawn
            || super::effects::TRANSFORM_PROPS
                .iter()
                .all(|&prop| self.style.get(prop).is_none_or(|value| value == "none"))
        {
            return corners;
        }
        super::effects::transform::reach(&self.local, &self.style, &self.units, size, corners)
    }

    fn prepaint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        let parts: Vec<_> = cx.fragments().collect();
        let (exact, baselines, unscrolled) = (cx.exact, cx.baselines, cx.unscroll);
        let anchored = cx.style.anchored.as_deref();
        let window = &mut *cx.window;
        let mut bounds = exact;
        let mut offset = point(px(0.), px(0.));
        unscroll(&mut offset, self.unplaced, unscrolled);
        bounds.origin += offset;
        let inset = match anchored {
            Some(axes) => self.aligned(axes, bounds.size, window),
            None => self.inset,
        };
        if let Some(sticky) = &self.sticky {
            let Sticky {
                scroll,
                border,
                block,
            } = &**sticky;
            let (mut viewport, room) = (scroll.bounds(), scroll.room());
            viewport.origin += point(px(border[3]) + room.left, px(border[0]) + room.top);
            viewport.size.width -= px(border[1] + border[3]) + room.left + room.right;
            viewport.size.height -= px(border[0] + border[2]) + room.top + room.bottom;
            if let Some(left) = self.inset[3] {
                offset.x = (viewport.left() + px(left) - bounds.left()).max(px(0.));
            }
            if let Some(right) = self.inset[1] {
                offset.x = offset.x.min(viewport.right() - px(right) - bounds.right());
            }
            if let Some(top) = self.inset[0] {
                offset.y = (viewport.top() + px(top) - bounds.top()).max(px(0.));
            }
            if let Some(bottom) = self.inset[2] {
                offset.y = offset
                    .y
                    .min(viewport.bottom() - px(bottom) - bounds.bottom());
            }
            if let Some([x, y, width, height]) =
                block.and_then(|(parent, [top, right, bottom, left])| {
                    let view = &*self.view;
                    let store = view.store.borrow();
                    let [x, y, width, height] = store
                        .page_state(view.id)?
                        .geometry
                        .mirror
                        .get(&parent)
                        .copied()?;
                    Some([
                        x + left,
                        y + top,
                        width - left - right,
                        height - top - bottom,
                    ])
                })
            {
                offset.x = offset
                    .x
                    .min(px(x + width) - bounds.right())
                    .max(px(x) - bounds.left());
                offset.y = offset
                    .y
                    .min(px(y + height) - bounds.bottom())
                    .max(px(y) - bounds.top());
            }
            bounds.origin += offset;
        }
        if let Some(contained) = self.contained(inset) {
            let shift = contained.shift(&self.view, bounds, window);
            offset += shift;
            bounds.origin += shift;
            self.based.set(offset);
        }
        if self.fixed {
            let mut viewport = window.viewport_size();
            if let Some(scroll) = (self.view.store.borrow().page_state(self.view.id))
                .and_then(|dom| dom.geometry.scroll.get(&0))
            {
                let room = scroll.room();
                viewport.width -= room.left + room.right;
                viewport.height -= room.top + room.bottom;
            }
            for axis in 0..2 {
                let [start, end] = if axis == 0 { [3, 1] } else { [0, 2] };
                let auto =
                    |side: usize| self.style.get(css::properties::MARGIN[side]) == Some("auto");
                let margin = |side: usize| {
                    if auto(side) {
                        px(0.)
                    } else {
                        px(self.margin[side])
                    }
                };
                let [extent, origin, size] = if axis == 0 {
                    [viewport.width, bounds.left(), bounds.size.width]
                } else {
                    [viewport.height, bounds.top(), bounds.size.height]
                };
                let at = match (inset[start], inset[end]) {
                    (Some(first), Some(last)) if auto(start) || auto(end) => {
                        let free = extent - px(first + last) - size - margin(start) - margin(end);
                        px(first)
                            + match (auto(start), auto(end)) {
                                (true, true) if axis == 0 => (free / 2.).max(px(0.)),
                                (true, true) => free / 2.,
                                (true, false) => free,
                                _ => margin(start),
                            }
                    }
                    (Some(first), last) if axis == 1 || !self.rtl || last.is_none() => {
                        px(first) + margin(start)
                    }
                    (_, Some(last)) => extent - px(last) - margin(end) - size,
                    (_, None) => continue,
                };
                if axis == 0 {
                    offset.x = at - origin;
                } else {
                    offset.y = at - origin;
                }
            }
            bounds.origin = exact.origin + offset;
        }
        if self.style.get(Prop::POSITION) == Some("fixed") {
            // The clip of a hidden box doesn't reach the fixed boxes in it.
            let store = self.view.store.borrow();
            self.hidden
                .set(store.pages.get(&self.view.id).is_some_and(|page| {
                    crate::render::Anchored::hidden_around(&page.state, &page.doc, self.id)
                }));
        }
        if anchored.is_some() {
            let store = self.view.store.borrow();
            if let Some(dom) = store.page_state(self.view.id)
                && let Some(found) = dom.layout.anchors.boxes.get(&self.id)
            {
                let (by, hidden) = found.prepaint(dom, !self.view.reflowing.get());
                let by = by.map(px);
                offset += point(by[0], by[1]);
                bounds.origin += point(by[0], by[1]);
                let fixed = self.style.get(Prop::POSITION) == Some("fixed");
                self.hidden.set(hidden || fixed && self.hidden.get());
            }
        }
        self.block(bounds, point(px(0.), px(0.)));
        let perspective = self.perspective.as_deref().and_then(
            |((distance, origin), parent, [top, right, bottom, left])| {
                // The place of the parent before its own transform.
                let view = &*self.view;
                let store = view.store.borrow();
                let padding = store.page_state(view.id)?.layout.blocks.get(parent)?;
                let [x, y] = [padding.origin.x, padding.origin.y].map(f32::from);
                let [width, height] = [padding.size.width, padding.size.height].map(f32::from);
                let size = [width + left + right, height + top + bottom];
                let [ox, oy, _] = crate::css::value::transform::origin(origin, &self.units, size);
                Some((*distance, [x - left + ox, y - top + oy]))
            },
        );
        let offset_style = (super::effects::offset::reads_page(&self.style) && !self.boxless)
            .then(|| {
                let view = &*self.view;
                let mut store = view.store.borrow_mut();
                let (doc, state, _) = store.page_parts(view.id);
                let (style, units) = (&self.style, &self.units);
                super::effects::offset::placed(
                    self.id, style, units, bounds, doc?, state, &self.ids, window,
                )
            })
            .flatten();
        if let Some(style) = &offset_style {
            let origin = Rect::new(renderer::Point::default(), bounds.size);
            let local = crate::css::value::transform::transform(style, &self.units, origin, None);
            let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
            self.local.set(Some(Box::new((size, local))));
        }
        // A canvas draws a drawable element as it is before its transform.
        let mut matrix = match perspective {
            Some(_) => crate::css::value::transform::transform(
                offset_style.as_ref().unwrap_or(&self.style),
                &self.units,
                bounds,
                perspective,
            ),
            None => super::effects::transform::transform_at(
                &self.local,
                &self.style,
                &self.units,
                bounds,
            ),
        }
        .filter(|_| !self.drawn);
        if self.transformed && !self.drawn {
            matrix.get_or_insert(crate::css::value::transform::IDENTITY);
            self.view
                .composited_transforms
                .borrow_mut()
                .insert(self.id, (bounds, perspective));
        }
        let space = self.space.joins || self.space.preserves;
        if space {
            matrix.get_or_insert(crate::css::value::transform::IDENTITY);
        }
        if !self.transformed
            && !space
            && let Some([x, y]) = matrix
                .as_ref()
                .and_then(super::effects::transform::translation)
        {
            let translation = point(px(x), px(y));
            offset += translation;
            bounds.origin += translation;
            matrix = None;
            // Boxes positioned in this block include the translation through the offset, so the block must too.
            let view = &*self.view;
            if self.block.is_some()
                && let Some(block) = view
                    .store
                    .borrow_mut()
                    .page_state_mut(view.id)
                    .and_then(|dom| dom.layout.blocks.get_mut(&self.id))
            {
                block.origin += translation;
            }
        }
        let mut prepaint = |window: &mut Window| {
            let window_bounds = window.transformed_bounds(bounds);
            let shape = window.hit_shape(bounds);
            // The parts move with the box's offsets and transform.
            let parts: Vec<_> = parts
                .iter()
                .map(|part| window.transformed_bounds(*part + (bounds.origin - exact.origin)))
                .collect();
            let view = &*self.view;
            let flags = {
                let mut store = view.store.borrow_mut();
                let dom = store.state(view.id);
                // Grid tracks are sized before transforms, so they compare with the untransformed box.
                if let Some(axes) = dom.layout.flexed_grids.get(&self.id)
                    && axes
                        .iter()
                        .zip([bounds.size.width, bounds.size.height])
                        .any(|(axis, size)| {
                            axis.is_some_and(|(used, extent)| {
                                let gap = (f32::from(size) - extent - used).abs();
                                gap.is_nan() || gap > 0.5
                            })
                        })
                {
                    window.redraw();
                }
                self.mirror(dom, window_bounds, shape, bounds.size, &parts);
                let width = f32::from(bounds.size.width);
                let [top, right, bottom, left] = self.margin;
                let cell = self
                    .cell
                    .and_then(|grid| {
                        let cells = &dom.layout.grids.get(&grid)?.3.cells;
                        let index = cells.binary_search_by_key(&self.id, |cell| cell.0).ok()?;
                        let [_, _, width, height] = cells[index].2;
                        Some([width, height])
                    })
                    .unwrap_or([
                        (width + right + left).max(0.),
                        f32::from(bounds.size.height) + top + bottom,
                    ]);
                let cull = &mut dom.layout.cull;
                if cull.placed.len() <= self.id {
                    cull.placed.resize(self.id + 1, Default::default());
                }
                cull.placed[self.id] = (self.revision, cell, self.margin);
                let inner = [
                    width - self.sides[0],
                    f32::from(bounds.size.height) - self.sides[1],
                ];
                cull.widths.insert(self.id, inner);
                let flags = cull.info.get(self.id).map_or(0, |info| info.flags);
                view.reaching
                    .borrow_mut()
                    .push(([f32::INFINITY, f32::NEG_INFINITY], self.plans));
                if dom
                    .layout
                    .containers
                    .get(&self.id)
                    .is_some_and(|size| (size[0] - inner[0]).abs() > 0.5)
                {
                    dom.layout.containers.insert(self.id, inner);
                    window.redraw();
                }
                if let Some((grid, aligned)) = self.grid {
                    let baselines = baselines.map(|value| value.map(f32::from));
                    if dom.layout.baselines.insert(self.id, baselines) != Some(baselines) {
                        dom.layout.grids.remove(&grid);
                        if aligned {
                            window.redraw();
                        }
                    }
                }
                flags
            };
            let moved = view
                .frames
                .borrow_mut()
                .get_mut(&self.id)
                .is_some_and(|frame| {
                    let zoom = window.zoom();
                    let bounds = [
                        window_bounds.origin.x,
                        window_bounds.origin.y,
                        window_bounds.size.width,
                        window_bounds.size.height,
                    ]
                    .map(|value| (f32::from(value) * zoom) as f64);
                    frame.bounds != bounds && {
                        frame.bounds = bounds;
                        true
                    }
                });
            // The web view can run its event loop while it moves, so only a shared borrow is held.
            if moved && let Some(frame) = view.frames.borrow().get(&self.id) {
                let _ = frame.browser.set_bounds(frame.bounds);
            }
            // The boxes in it that paint in another stacking context are hidden too.
            let clip = (self.hidden.get()).then(|| Rect::default().into());
            window.scope(renderer::Scope::Clip(clip), |window| {
                window.scope(renderer::Scope::Offset(offset), |window| inner(window))
            });
            self.reached(window_bounds, flags);
            self.unions();
        };
        match matrix {
            _ if self.drawn => {
                // The box takes the pointer where its canvas drew it.
                let view = &*self.view;
                let content = view.drawing.get();
                let drawn = view.store.borrow().page_state(view.id).and_then(|dom| {
                    let to_canvas = dom.geometry.drawn.get(&self.id)?.matrix;
                    let [x, y] = [bounds.origin.x, bounds.origin.y].map(f32::from);
                    let from_box = crate::css::value::transform::translate(-x, -y, 0.);
                    let to_window = crate::css::value::transform::translate(
                        content.x.into(),
                        content.y.into(),
                        0.,
                    );
                    let placed = crate::css::value::transform::multiply(&to_canvas, &from_box);
                    Some(crate::css::value::transform::multiply(&to_window, &placed))
                });
                window.with_hits_at(drawn, &mut prepaint)
            }
            Some(matrix) if self.transformed => {
                window.with_keyed_hit_transform(matrix, Some(self.id as u64), &mut prepaint)
            }
            Some(matrix) => {
                window.with_space_hits(matrix, self.space, self.backface_hidden(), &mut prepaint)
            }
            None => prepaint(window),
        }
        self.placed.set((bounds, matrix));
        self.offset.set(offset);
    }
    // A drawn box is a snapshot that the page reads, a composited box has layers that frames patch, and filters
    // or clips can read elements elsewhere in the page.
    fn cached(&self) -> bool {
        !self.drawn
            && !self.composited
            && !self.frame
            && !self.space.joins
            && !self.space.preserves
            && !super::effects::offset::reads_page(&self.style)
            && [
                Prop::FILTER,
                Prop::WEBKIT_FILTER,
                Prop::CLIP_PATH,
                Prop::WEBKIT_CLIP_PATH,
            ]
            .into_iter()
            .all(|prop| {
                self.style
                    .get(prop)
                    .is_none_or(|value| !value.contains("url("))
            })
    }

    fn volatile(&self) -> bool {
        self.drawn
            || self.sticky.is_some()
            || self.fixed
            || self.anchored
            || self.perspective.is_some()
            || self.transformed
            || self.frame
            || self.space.joins
            || self.space.preserves
            || super::effects::offset::reads_page(&self.style)
    }

    fn skippable(&self) -> bool {
        true
    }

    fn stays(&self, exact: Rect<Pixels>, unscroll: Point<Pixels>) -> bool {
        self.based(exact, unscroll)
    }

    fn shifts(&self) -> bool {
        true
    }

    // Moves the layout mirror, containing blocks, fragments and cull reach of the nodes below the box.
    fn shift(&self, by: Point<Pixels>, exact: Rect<Pixels>, _: &mut Window) -> bool {
        let (placed, matrix) = self.placed.get();
        let bounds = Rect::new(exact.origin + self.offset.get(), exact.size);
        let near = |a: Pixels, b: Pixels| (a - b).0.abs() < 0.01;
        if !near(placed.origin.x + by.x, bounds.origin.x)
            || !near(placed.origin.y + by.y, bounds.origin.y)
            || placed.size != bounds.size
        {
            return false;
        }
        let view = &*self.view;
        let mut store = view.store.borrow_mut();
        let dom = store.state(view.id);
        let walk = &dom.styles.walk;
        let Some(start) = walk.place(self.id) else {
            return false;
        };
        let range = start as usize..start as usize + walk.span(self.id);
        let inside = |id: usize| {
            walk.place(id)
                .is_some_and(|at| range.contains(&(at as usize)))
        };
        // Top layer boxes paint at the root, outside this box.
        if dom.interaction.top_layer.iter().any(|&node| inside(node)) {
            return false;
        }
        let [x, y] = [by.x, by.y].map(f32::from);
        let nodes = walk.snapshot();
        let mut reach = view.reach.borrow_mut();
        for &(id, _) in &nodes[range] {
            let id = id as usize;
            if let Some(parts) = dom.geometry.fragments.get_mut(&id) {
                parts
                    .iter_mut()
                    .for_each(|part| (part[0], part[1]) = (part[0] + x, part[1] + y));
            }
            if let Some(shape) = dom.geometry.shapes.get_mut(&id) {
                shape.iter_mut().for_each(|corner| *corner += by);
            }
            if let Some(pieces) = dom.geometry.pieces.get_mut(&id) {
                pieces
                    .iter_mut()
                    .for_each(|(_, part)| (part[0], part[1]) = (part[0] + x, part[1] + y));
            }
            if let Some(block) = dom.layout.blocks.get_mut(&id) {
                block.origin += by;
            }
            // A node without a box keeps an empty rectangle at the origin.
            if let Some(rect) = dom.geometry.mirror.get_mut(&id)
                && *rect != [0.; 4]
            {
                (rect[0], rect[1]) = (rect[0] + x, rect[1] + y);
            }
            if let Some(reach) = reach.get_mut(id)
                && reach[0].is_finite()
            {
                (reach[0], reach[1]) = (reach[0] + y, reach[1] + y);
            }
        }
        let reach = reach
            .get(self.id)
            .copied()
            .unwrap_or([f32::INFINITY, f32::NEG_INFINITY]);
        drop((store, nodes));
        // A transform turns about the new place.
        let matrix = matrix.and_then(|_| {
            super::effects::transform::transform_at(&self.local, &self.style, &self.units, bounds)
        });
        self.placed.set((bounds, matrix));
        let (window_bounds, flags) = self.reached_at.get();
        let window_bounds = Rect::new(window_bounds.origin + by, window_bounds.size);
        self.reached_at.set((window_bounds, flags));
        self.merge(reach, window_bounds, flags);
        true
    }

    // The shift of a box around it moved the mirror of its nodes.
    fn lag(&self, by: Point<Pixels>) {
        let (placed, matrix) = self.placed.get();
        let bounds = Rect::new(placed.origin + by, placed.size);
        let matrix = matrix.and_then(|_| {
            super::effects::transform::transform_at(&self.local, &self.style, &self.units, bounds)
        });
        self.placed.set((bounds, matrix));
        let (window_bounds, flags) = self.reached_at.get();
        let window_bounds = Rect::new(window_bounds.origin + by, window_bounds.size);
        self.reached_at.set((window_bounds, flags));
    }

    fn skip(&self, exact: Rect<Pixels>, _: &mut Window) -> bool {
        if Rect::new(exact.origin + self.offset.get(), exact.size) != self.placed.get().0 {
            return false;
        }
        let none = [f32::INFINITY, f32::NEG_INFINITY];
        let reach = (self.view.reach.borrow().get(self.id).copied()).unwrap_or(none);
        let (window_bounds, flags) = self.reached_at.get();
        self.merge(reach, window_bounds, flags);
        true
    }

    fn kept(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        // Its containing block moved or changed its size, so the box has another place.
        if !self.based(cx.exact, cx.unscroll) {
            cx.window.unsettle();
            return self.prepaint(cx, inner);
        }
        let (placed, mut matrix) = self.placed.get();
        let offset = self.offset.get();
        let bounds = Rect::new(cx.exact.origin + offset, cx.exact.size);
        let moved = bounds != placed;
        let parts: Vec<_> = match moved {
            true => cx.fragments().collect(),
            false => Vec::new(),
        };
        if moved {
            // A kept box has its size, so its transform only turns about the new place.
            if matrix.is_some() {
                matrix = super::effects::transform::transform_at(
                    &self.local,
                    &self.style,
                    &self.units,
                    bounds,
                );
            }
            self.block(cx.exact, offset);
            self.placed.set((bounds, matrix));
        }
        let mut kept = |window: &mut Window| {
            let window_bounds = window.transformed_bounds(bounds);
            let shape = window.hit_shape(bounds);
            let parts: Vec<_> = parts
                .iter()
                .map(|part| window.transformed_bounds(*part + offset))
                .collect();
            let view = &*self.view;
            let flags = {
                let mut store = view.store.borrow_mut();
                let dom = store.state(view.id);
                if moved {
                    self.mirror(dom, window_bounds, shape, bounds.size, &parts);
                }
                dom.layout
                    .cull
                    .info
                    .get(self.id)
                    .map_or(0, |info| info.flags)
            };
            view.reaching
                .borrow_mut()
                .push(([f32::INFINITY, f32::NEG_INFINITY], self.plans));
            window.scope(renderer::Scope::Offset(offset), |window| inner(window));
            self.reached(window_bounds, flags);
            if moved {
                self.unions();
            }
        };
        match matrix {
            Some(matrix) => {
                let hides_back = self.backface_hidden();
                (cx.window).with_space_hits(matrix, self.space, hides_back, &mut kept)
            }
            None => kept(cx.window),
        }
    }

    fn paint(&self, cx: &mut NodeCx<'_>, inner: &mut dyn FnMut(&mut Window)) {
        if self.hidden.get() {
            return;
        }
        let captured = &self.view.captured;
        let bounds = self.placed.get().0;
        if captured.active()
            && captured.paint(Some(self.id), bounds, cx.window, &mut |window| {
                self.draw(window, inner)
            })
        {
            return;
        }
        if !self.drawn {
            return self.draw(cx.window, inner);
        }
        // A new page or a new node with this number has no snapshot of the box yet.
        let view = &*self.view;
        let store = view.store.borrow();
        let fresh = (store.page_state(view.id))
            .is_none_or(|dom| !dom.media.snapshots.contains_key(&self.id));
        drop(store);
        let key = Some((self.id as u64, fresh));
        (cx.window).with_snapshot(key, self.placed.get().0, |window| self.draw(window, inner));
    }
}

impl DomBox {
    // A transform or z-index makes a stacking context, which isolates the blending in it.
    fn stacked(&self) -> bool {
        (crate::render::effects::TRANSFORM_PROPS.iter())
            .any(|&prop| self.style.get(prop).is_some_and(|value| value != "none"))
            || self
                .style
                .get(Prop::Z_INDEX)
                .is_some_and(|value| value != "auto")
                && self
                    .style
                    .get(Prop::POSITION)
                    .is_some_and(|value| value != "static")
    }

    fn blended(&self) -> bool {
        let store = self.view.store.borrow();
        let styles = store
            .page_state(self.view.id)
            .and_then(|dom| dom.styles.last());
        styles.is_some_and(|styles| styles.blended())
    }

    fn backface_hidden(&self) -> bool {
        self.style.get(Prop::BACKFACE_VISIBILITY) == Some("hidden")
    }

    // The insets of a box that an anchor aligns, from its place in its containing block.
    fn aligned(
        &self,
        axes: &[Option<renderer::Aligned>; 2],
        size: Size<Pixels>,
        window: &Window,
    ) -> [Option<f32>; 4] {
        let mut inset = self.inset;
        let extent = {
            let store = self.view.store.borrow();
            let dom = store.page_state(self.view.id);
            let block = dom.and_then(|dom| dom.layout.blocks.get(&self.container?));
            let viewport = window.viewport_size();
            block.map_or([viewport.width.0, viewport.height.0], |block| {
                [block.size.width.0, block.size.height.0]
            })
        };
        for (axis, aligned) in axes.iter().enumerate() {
            let Some(aligned) = aligned else { continue };
            let [start, end] = [[3, 1], [0, 2]][axis];
            let size = [size.width.0, size.height.0][axis] + self.margin[start] + self.margin[end];
            inset[start] = Some(aligned.place(size, extent[axis]));
            inset[end] = None;
        }
        inset
    }

    fn draw(&self, window: &mut Window, inner: &mut dyn FnMut(&mut Window)) {
        let (bounds, matrix) = self.placed.get();
        self::native::paint(&self.view, self.id, bounds, &self.style, window);
        // The mark of a plane goes into its layer, so it sorts with the plane.
        let mark = self.view.id << 32 | self.id as u64;
        let space = self.space.joins || self.space.preserves;
        if !space {
            window.mark(mark);
        }
        let inner = &mut |window: &mut Window| {
            if space {
                window.mark(mark);
            }
            match self.text_clip {
                true => window.with_text_clip(|window| inner(window)),
                false => inner(window),
            }
        };
        if matrix.is_none() && self.effects.is_none() && !(self.stacked() && self.blended()) {
            return inner(window);
        }
        // The matrix of a box in a 3D rendering context goes to the plane of the context.
        let (matrix, joins) = match window.space().filter(|_| self.space.joins) {
            Some(space) => (
                matrix.map(|matrix| crate::css::value::transform::multiply(space, &matrix)),
                true,
            ),
            None => (matrix, false),
        };
        let hidden = self.backface_hidden() && matrix.as_ref().is_some_and(renderer::faces_away);
        let nothing = !self.composited && matrix.as_ref().is_some_and(renderer::singular);
        if nothing || hidden && !self.space.preserves {
            return;
        }
        let (clip, mut layers) = self.effects(bounds, matrix, window);
        // Frames change the transform without a paint, so rasters inside take its largest scale, like Chrome.
        if let Some(outer) = layers
            .last_mut()
            .filter(|_| self.transformed && !self.drawn)
        {
            let store = self.view.store.borrow();
            let placed = self
                .view
                .composited_transforms
                .borrow()
                .get(&self.id)
                .copied();
            if let Some(placed) = placed
                && let Some(dom) = store.page_state(self.view.id)
                && let Some(styles) = dom.styles.last()
            {
                outer.scale = super::effects::transform::largest_scale(
                    outer.transform.as_ref(),
                    &dom.styles.motion,
                    styles,
                    self.id,
                    &self.units,
                    placed,
                    std::time::Instant::now(),
                );
            }
        }
        // Filters nest inside the layer with the transform, which is the last one.
        if let Some(outer) = layers.last_mut().filter(|_| space) {
            outer.space = renderer::Space {
                joins,
                hidden,
                ..self.space
            };
            outer.plane = bounds;
        }
        let mut paint = |window: &mut Window| match &clip {
            Some(clip) => window.with_group_clip(*clip, |window| inner(window)),
            None => inner(window),
        };
        nest_effects(&mut layers, window, &mut paint);
    }

    fn effects(
        &self,
        bounds: Rect<Pixels>,
        transform: Option<crate::css::value::transform::Matrix>,
        window: &mut Window,
    ) -> (Option<renderer::Clip<Pixels>>, Vec<renderer::PaintEffect>) {
        use super::effects::{self, clip::Clip};
        let style = &self.style;
        let units = self.units;
        let value = |props: &[Prop]| {
            props
                .iter()
                .find_map(|&prop| style.get(prop))
                .filter(|value| !matches!(*value, "none" | "normal" | "auto"))
        };
        let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
        let view = &*self.view;
        let mut store = view.store.borrow_mut();
        let empty = crate::dom::Document::default();
        let (doc, state, app) = store.page_parts(view.id);
        let doc = doc.unwrap_or(&empty);
        let mut layers = value(&[Prop::FILTER, Prop::WEBKIT_FILTER])
            .map(|value| effects::filter::filters(value, &units, doc, &self.ids))
            .unwrap_or_default();
        let mut clip = None;
        let mut shapes = None;
        match value(&[Prop::CLIP_PATH, Prop::WEBKIT_CLIP_PATH])
            .and_then(|value| effects::clip::clip(value, style, &units, size, doc, &self.ids))
        {
            // A filter draws before the clip, so its layer takes a mask.
            Some(Clip::Rounded([x, y, w, h], radii)) if !layers.is_empty() => {
                let outline = css::value::shape::rounded([x, y, w, h], radii.map(|r| [r; 2]));
                shapes = Some((
                    format!("<path d=\"{}\"/>", css::value::shape::svg(&outline)),
                    Some([x, y, x + w, y + h]),
                ));
            }
            Some(Clip::Rounded(rect, radii)) => {
                clip = Some(renderer::Clip::rounded(
                    Rect::new(
                        bounds.origin + point(px(rect[0]), px(rect[1])),
                        renderer::size(px(rect[2]), px(rect[3])),
                    ),
                    effects::clip::corners(radii),
                ))
            }
            Some(Clip::Shape(shape, reach)) => shapes = Some((shape, reach)),
            None => {}
        }
        let mask = mask::mask(
            view,
            state,
            app,
            (self.id, 0),
            style,
            units,
            shapes,
            bounds,
            window,
        );
        let visible = style
            .get(Prop::VISIBILITY)
            .is_none_or(|value| value == "visible");
        let shown = self.effects.unwrap_or(1.) > 0. && visible;
        // A composited box keeps its backdrop also at no opacity or blur, so frames can patch them.
        let backdrop = value(&[Prop::BACKDROP_FILTER, Prop::WEBKIT_BACKDROP_FILTER])
            .filter(|_| shown || self.composited && visible)
            .and_then(|value| {
                effects::filter::backdrop(value, &units).or_else(|| {
                    effects::filter::lone_blur(Some(value))
                        .filter(|_| self.composited)
                        .map(|_| (px(0.), None))
                })
            })
            .map(|(blur, color_matrix)| renderer::Backdrop {
                bounds,
                corner_radii: effects::clip::radii(style, &units, || size).resolve(bounds.size),
                blur,
                color_matrix,
            });
        let glass = style
            .get(Prop::MACOS_GLASS)
            .filter(|kind| {
                cfg!(target_os = "macos")
                    && shown
                    && layers.is_empty()
                    && clip.is_none()
                    && matches!(*kind, "regular" | "clear")
            })
            .and_then(|kind| {
                let radii = effects::clip::radii(style, &units, || size).resolve(bounds.size);
                let radius = radii.x.top_left;
                if radii != renderer::Radii::from(radius) {
                    return None;
                }
                Some(renderer::Glass {
                    key: self.id as u64,
                    bounds,
                    corner_radius: radius,
                    tint: style
                        .get(Prop::MACOS_GLASS_TINT_COLOR)
                        .and_then(css::value::color::color)
                        .filter(|(_, alpha)| *alpha > 0)
                        .map(crate::css::value::color::rgba),
                    clear: kind == "clear",
                    interactive: style.get(Prop::MACOS_GLASS_INTERACTIVE) == Some("true")
                        && style.get(Prop::POINTER_EVENTS) != Some("none")
                        && doc
                            .nodes
                            .get(self.id)
                            .is_some_and(|node| !node.attrs.contains_key("disabled")),
                })
            });
        let outer = renderer::PaintEffect {
            transform,
            opacity: self.effects.unwrap_or(1.),
            blend_mode: value(&[Prop::MIX_BLEND_MODE])
                .map_or(renderer::BlendMode::Normal, |value| {
                    effects::filter::blend_mode(value)
                }),
            mask,
            backdrop,
            glass,
            key: self.composited.then_some(self.id as u64),
            ..Default::default()
        };
        // Isolation affects mix-blend-mode only. An extra group without blending incorrectly hides content
        // from descendant backdrop filters. As in Chrome, transforms and z-index make stacking contexts that
        // isolate too.
        let isolate = (style.get(Prop::ISOLATION) == Some("isolate") || self.stacked())
            && outer.opacity >= 1.
            && outer.blend_mode == renderer::BlendMode::Normal
            && store
                .page_state(view.id)
                .and_then(|dom| dom.styles.last())
                .is_none_or(|styles| styles.blended());
        let needed = outer.transform.is_some()
            || outer.opacity < 1.
            || outer.blend_mode != renderer::BlendMode::Normal
            || outer.mask.is_some()
            || outer.backdrop.is_some()
            || outer.glass.is_some()
            || isolate
            || self.composited;
        if needed {
            if layers.len() == 1 && clip.is_none() {
                let filter = layers.pop().unwrap();
                layers.push(renderer::PaintEffect {
                    color_matrix: filter.color_matrix,
                    blur: filter.blur,
                    shadow: filter.shadow,
                    ..outer
                });
            } else {
                layers.push(outer);
            }
            // A transform alone draws in its parent, so a group of its own isolates the blending in it.
            if isolate && layers.last().is_some_and(|layer| layer.transform.is_some()) {
                layers.insert(0, renderer::PaintEffect::default());
            }
        }
        (clip, layers)
    }
}

fn nest_effects(
    layers: &mut Vec<renderer::PaintEffect>,
    window: &mut Window,
    paint: &mut dyn FnMut(&mut Window),
) {
    match layers.pop() {
        Some(effect) => window.with_effect(effect, |window| nest_effects(layers, window, paint)),
        None => paint(window),
    }
}
