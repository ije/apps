use crate::{
    Clip, Color, Edges, Pixels, Point, Radii, Rect, Size,
    geometry::{snap_width, to_layout_unit},
    point, px, size,
    text::TextOverrides,
};

// The shader reads the style as its position in this list.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum BorderStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
    Double,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    #[default]
    Auto,
    Px(Pixels),
    Fraction(f32),
    Calc(f32, Pixels),
    /// The bound at this index in [`Style::bounds`].
    Bound(u8),
}

/// `min()`, `max()` and `clamp()` of percentages, which resolve at layout: terms `share * basis + offset`, and the
/// least or largest of other bounds.
#[derive(Clone, Debug, PartialEq)]
pub enum Bound {
    Term(f32, f32),
    Min(Vec<Bound>),
    Max(Vec<Bound>),
}

// A calc handle with these top bits holds the address of a bound. Other handles hold a share there, which is never
// this NaN.
const BOUND_TAG: u64 = 0x7fc1 << 48;
const ADDRESS: u64 = (1 << 48) - 1;

impl Bound {
    fn resolve(&self, basis: f32) -> f32 {
        match self {
            Self::Term(share, offset) => share * basis + offset,
            Self::Min(bounds) => bounds.iter().fold(f32::INFINITY, |least, bound| {
                least.min(bound.resolve(basis))
            }),
            Self::Max(bounds) => bounds.iter().fold(f32::NEG_INFINITY, |most, bound| {
                most.max(bound.resolve(basis))
            }),
        }
    }

    fn scaled(&self, scale: f32) -> Self {
        let all = |bounds: &[Bound]| bounds.iter().map(|bound| bound.scaled(scale)).collect();
        match self {
            Self::Term(share, offset) => Self::Term(*share, to_layout_unit(*offset, scale)),
            Self::Min(bounds) => Self::Min(all(bounds)),
            Self::Max(bounds) => Self::Max(all(bounds)),
        }
    }

    pub(crate) fn hash(&self, hasher: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        match self {
            Self::Term(share, offset) => (0u8, share.to_bits(), offset.to_bits()).hash(hasher),
            Self::Min(bounds) | Self::Max(bounds) => {
                (matches!(self, Self::Min(_)), bounds.len()).hash(hasher);
                bounds.iter().for_each(|bound| bound.hash(hasher));
            }
        }
    }
}

impl From<Pixels> for Length {
    fn from(value: Pixels) -> Self {
        Self::Px(value)
    }
}

pub fn relative(fraction: f32) -> Length {
    Length::Fraction(fraction)
}

impl Length {
    /// Auto and bounds count as zero.
    pub fn of(self, basis: Pixels) -> Pixels {
        match self {
            Self::Auto => Pixels::ZERO,
            Self::Px(value) => value,
            Self::Fraction(share) => basis * share,
            Self::Calc(share, value) => basis * share + value,
            Self::Bound(_) => Pixels::ZERO,
        }
    }

    // Taffy keeps lengths in device pixels. Auto is `None`. `bounds` are the scaled bounds of the style.
    fn to_taffy(self, scale: f32, bounds: &[Bound]) -> Option<taffy::LengthPercentage> {
        use taffy::LengthPercentage as Taffy;
        Some(match self {
            Self::Auto => return None,
            Self::Px(value) => Taffy::length(to_layout_unit(value.0, scale)),
            Self::Fraction(share) => Taffy::percent(share),
            Self::Calc(share, value) => {
                calc(share, value, scale).map_or(Taffy::percent(share), Taffy::calc)
            }
            Self::Bound(index) => {
                let bound = bounds.get(usize::from(index))?;
                let address = (bound as *const Bound).expose_provenance() as u64;
                Taffy::calc(std::ptr::without_provenance(
                    (address & ADDRESS | BOUND_TAG) as usize,
                ))
            }
        })
    }
}

// Taffy treats calc handles as opaque, so a handle holds the share's bits above the offset in 1/64 device
// pixels instead of an address. Taffy tags the low 3 bits.
fn calc(share: f32, offset: Pixels, scale: f32) -> Option<*const ()> {
    // The offset stops at 2^22 device pixels: past that, the shift below would flip its sign.
    let units = ((to_layout_unit(offset.0, scale) * 64.) as i32).clamp(-(1 << 28), (1 << 28) - 1);
    let bits = u64::from(share.to_bits()) << 32 | u64::from((units << 3) as u32);
    (units != 0).then(|| std::ptr::without_provenance(bits as usize))
}

pub(crate) fn resolve_calc(handle: *const (), basis: f32) -> f32 {
    let bits = handle.addr() as u64;
    if bits & !ADDRESS == BOUND_TAG {
        let bound = std::ptr::with_exposed_provenance::<Bound>((bits & ADDRESS) as usize);
        // SAFETY: the handle comes from the scaled bounds of a `Styled`, which outlives each Taffy style made from
        // it.
        return unsafe { &*bound }.resolve(basis);
    }
    let share = f32::from_bits((bits >> 32) as u32);
    let units = (bits as u32 as i32) >> 3;
    share * basis + units as f32 / 64.
}

impl Radii<Length> {
    pub fn to_pixels(&self, size: Size<Pixels>) -> Radii<Pixels> {
        Radii {
            x: self.x.map(|length| length.of(size.width)),
            y: self.y.map(|length| length.of(size.height)),
            shape: self.shape,
        }
    }

    pub fn resolve(&self, size: Size<Pixels>) -> Radii<Pixels> {
        self.to_pixels(size).fit(size)
    }
}

// The first keyword is the initial value.
macro_rules! keywords {
    ($($name:ident { $initial:ident $(, $other:ident)* })*) => {$(
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        pub enum $name { #[default] $initial $(, $other)* }
    )*};
}

keywords! {
    Display { Block, Flex, Grid, None }
    Position { Relative, Absolute }
    Visibility { Visible, Hidden }
    Overflow { Visible, Clip, Hidden, Scroll }
    Overscroll { Auto, Contain, None }
    FlexDirection { Row, RowReverse, Column, ColumnReverse }
    FlexWrap { NoWrap, Wrap, WrapReverse }
    AlignItems { FlexStart, FlexEnd, Center, Baseline, LastBaseline, Stretch }
    AlignContent { Start, End, FlexStart, FlexEnd, Center, Stretch, SpaceBetween, SpaceEvenly, SpaceAround }
    ObjectFit { Fill, Contain, Cover, ScaleDown, None }
    // `FirstOnly` takes the first baseline and gives none for the last. `Middle` puts it at half the border box height.
    Baseline { Auto, Last, First, FirstOnly, Bottom, Middle, None }
    Inline { None, Lines, Hangs }
    Cursor {
        Default, Text, VerticalText, Pointer, Crosshair, Grab, Grabbing, NotAllowed, ColResize, RowResize,
        EwResize, NsResize, EResize, WResize, NResize, SResize, NwseResize, NeswResize, Alias, Copy,
        ContextMenu, None
    }
}

impl ObjectFit {
    pub fn place(self, bounds: Rect, image: Size<i32>) -> Rect {
        let image = image.map(|value| px(value as f32));
        let area = bounds.size;
        let wider = area.width / area.height > image.width / image.height;
        let full_height = size(image.width * (area.height / image.height), area.height);
        let full_width = size(area.width, image.height * (area.width / image.width));
        let contain = if wider { full_height } else { full_width };
        let fitted = match self {
            Self::Fill => return bounds,
            Self::Contain => contain,
            Self::Cover if wider => full_width,
            Self::Cover => full_height,
            Self::ScaleDown if image.width > area.width || image.height > area.height => contain,
            Self::ScaleDown | Self::None => image,
        };
        let corner = bounds.origin
            + point(
                (area.width - fitted.width) / 2.,
                (area.height - fitted.height) / 2.,
            );
        Rect::new(corner, fitted)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoxShadow {
    pub color: Color,
    pub offset: Point<Pixels>,
    pub blur_radius: Pixels,
    pub spread_radius: Pixels,
    pub inset: bool,
}

macro_rules! style {
    ($($(#[$doc:meta])* $field:ident: $kind:ty = $initial:expr,)*) => {
        #[derive(Clone, Debug, PartialEq)]
        pub struct Style {
            $($(#[$doc])* pub $field: $kind,)*
        }

        impl Default for Style {
            fn default() -> Self {
                Self { $($field: $initial,)* }
            }
        }
    };
}

const ZERO: Length = Length::Px(Pixels::ZERO);

style! {
    display: Display = Display::Block,
    visibility: Visibility = Visibility::Visible,
    overflow: Point<Overflow> = Point::new(Overflow::Visible, Overflow::Visible),
    restrict_scroll_to_axis: bool = false,
    overscroll: Point<Overscroll> = Point::new(Overscroll::Auto, Overscroll::Auto),
    position: Position = Position::Relative,
    inset: Edges<Length> = Edges::all(Length::Auto),
    size: Size<Length> = Size::default(),
    min_size: Size<Length> = Size::default(),
    max_size: Size<Length> = Size::default(),
    aspect_ratio: Option<f32> = None,
    margin: Edges<Length> = Edges::all(ZERO),
    padding: Edges<Length> = Edges::all(ZERO),
    /// The part of the padding that scroll bars take. All scrolled content stays clear of it.
    bars: Edges<Pixels> = Edges::default(),
    border_widths: Edges<Pixels> = Edges::default(),
    align_items: Option<AlignItems> = None,
    align_self: Option<AlignItems> = None,
    /// The horizontal self-alignment of an absolute box, as Taffy reads it.
    justify_self: Option<AlignItems> = None,
    align_content: Option<AlignContent> = None,
    justify_content: Option<AlignContent> = None,
    /// Content alignment falls back to the start when the content overflows, like `safe`.
    safe: bool = false,
    gap: Size<Length> = Size::new(ZERO, ZERO),
    flex_direction: FlexDirection = FlexDirection::Row,
    flex_wrap: FlexWrap = FlexWrap::NoWrap,
    flex_basis: Length = Length::Auto,
    flex_grow: f32 = 0.,
    flex_shrink: f32 = 1.,
    baseline: Baseline = Baseline::Auto,
    baseline_source: bool = false,
    inline: Inline = Inline::None,
    /// A block container: the auto heights of its children are not definite, unlike the flexed sizes of flex items.
    block: bool = false,
    background: Option<Color> = None,
    border_color: Option<Color> = None,
    border_style: BorderStyle = BorderStyle::Solid,
    corner_radii: Radii<Length> = Radii::default(),
    box_shadow: Vec<BoxShadow> = Vec::new(),
    text: TextOverrides = TextOverrides::default(),
    cursor: Option<Cursor> = None,
    opacity: Option<f32> = None,
    z_index: Option<i32> = None,
    stacking: bool = false,
    contains: u8 = 0,
    /// An [`Escape`] bit when the box is out of flow: only the clips of its containing block apply.
    escape: u8 = 0,
    /// The axes, 1 for x and 2 for y, whose insets place an absolute box in a containing block outside its parent.
    placed: u8 = 0,
    /// A column set: its first child is the flow of content, as wide as one column.
    columns: Option<Box<Columns>> = None,
    /// [`Break`] bits of a box in a column flow.
    breaks: u8 = 0,
    /// [`Flow`] bits of a box in a block formatting context with floats.
    flow: u8 = 0,
    /// [`ScrollAnchor`] bits.
    scroll_anchor: u8 = 0,
    /// The bounds that [`Length::Bound`] lengths of this style index.
    bounds: Vec<Bound> = Vec::new(),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Columns {
    pub count: u16,
    pub gap: Pixels,
    /// The height of the columns, which `fill` makes fixed. Without `fill`, they balance up to it.
    pub height: Option<Pixels>,
    pub fill: bool,
}

pub struct Break;

impl Break {
    pub const BEFORE: u8 = 1;
    pub const AFTER: u8 = 2;
    pub const AVOID_BEFORE: u8 = 4;
    pub const AVOID_AFTER: u8 = 8;
    pub const AVOID_INSIDE: u8 = 16;
    /// Replaced content and controls, which columns never break.
    pub const WHOLE: u8 = 32;
}

pub struct Flow;

impl Flow {
    pub const LEFT: u8 = 1;
    pub const RIGHT: u8 = 2;
    pub const CLEAR_LEFT: u8 = 4;
    pub const CLEAR_RIGHT: u8 = 8;
    /// A block column or line box in the formatting context of its parent: floats reach its lines and the boxes
    /// in it, and floats in it reach the boxes after it.
    pub const BLOCK: u8 = 16;
    /// A box in a right-to-left parent: auto margins that overflow keep its right edge.
    pub const RTL: u8 = 32;
    /// A right-to-left box: its absolute boxes resolve their insets and auto margins from the right.
    pub const RTL_BLOCK: u8 = 64;
    /// A right-to-left flex column: its items start at the right.
    pub const RTL_ITEMS: u8 = 128;
    pub const FLOAT: u8 = Self::LEFT | Self::RIGHT;
}

pub struct ScrollAnchor;

impl ScrollAnchor {
    /// `overflow-anchor: none`: the box is no scroll anchor, nor any box in it, and a scroll container keeps no
    /// anchor.
    pub const NONE: u8 = 1;
    /// A sticky box, which moves with the scroll: like in Chrome, it is no anchor, nor any box in it.
    pub const STICKY: u8 = 2;
}

pub struct Escape;

impl Escape {
    pub const ABSOLUTE: u8 = 1;
    pub const FIXED: u8 = 2;
    /// An outline: only the clips around its box apply, and it doesn't scroll with the content of the box.
    pub const OUTLINE: u8 = 4;
}

impl Style {
    /// The Taffy style, and the scaled bounds that its calc handles point to, which must outlive it.
    pub(crate) fn to_taffy(&self, scale: f32) -> (taffy::Style, Box<[Bound]>) {
        use taffy::{Dimension, LengthPercentage, LengthPercentageAuto, style as t};
        let bounds: Box<[Bound]> = self
            .bounds
            .iter()
            .map(|bound| bound.scaled(scale))
            .collect();
        let length = |value: Length| {
            value
                .to_taffy(scale, &bounds)
                .unwrap_or(LengthPercentage::length(0.))
        };
        let auto = |value: Length| {
            value
                .to_taffy(scale, &bounds)
                .map_or(LengthPercentageAuto::auto(), Into::into)
        };
        let dimension = |value: Length| {
            value
                .to_taffy(scale, &bounds)
                .map_or(Dimension::auto(), Into::into)
        };
        let edges =
            |edges: &Edges<Length>, side: &dyn Fn(Length) -> LengthPercentageAuto| taffy::Rect {
                top: side(edges.top),
                right: side(edges.right),
                bottom: side(edges.bottom),
                left: side(edges.left),
            };
        let items = |value: AlignItems| match value {
            AlignItems::FlexStart => t::AlignItems::FLEX_START,
            AlignItems::FlexEnd => t::AlignItems::FLEX_END,
            AlignItems::Center => t::AlignItems::CENTER,
            AlignItems::Baseline | AlignItems::LastBaseline => t::AlignItems::BASELINE,
            AlignItems::Stretch => t::AlignItems::STRETCH,
        };
        let content = |value: AlignContent| match value {
            AlignContent::Start => t::AlignContent::START,
            AlignContent::End => t::AlignContent::END,
            AlignContent::FlexStart => t::AlignContent::FLEX_START,
            AlignContent::FlexEnd => t::AlignContent::FLEX_END,
            AlignContent::Center => t::AlignContent::CENTER,
            AlignContent::Stretch => t::AlignContent::STRETCH,
            AlignContent::SpaceBetween => t::AlignContent::SPACE_BETWEEN,
            AlignContent::SpaceEvenly => t::AlignContent::SPACE_EVENLY,
            AlignContent::SpaceAround => t::AlignContent::SPACE_AROUND,
        };
        let safety = match self.safe {
            true => t::AlignmentSafety::Safe,
            false => t::AlignmentSafety::Default,
        };
        let content = |value: AlignContent| t::AlignContent {
            safety,
            ..content(value)
        };
        let overflow = |value: Overflow| match value {
            Overflow::Visible => t::Overflow::Visible,
            Overflow::Clip => t::Overflow::Clip,
            Overflow::Hidden => t::Overflow::Hidden,
            Overflow::Scroll => t::Overflow::Scroll,
        };
        let border = |width: Pixels| LengthPercentage::length(snap_width(width.0, scale));
        let widths = &self.border_widths;
        let padding = &self.padding;
        let mut layout = t::Style {
            display: match self.display {
                Display::Block => t::Display::Block,
                Display::Flex => t::Display::Flex,
                Display::Grid => t::Display::Grid,
                Display::None => t::Display::None,
            },
            position: match self.position {
                Position::Relative => t::Position::Relative,
                Position::Absolute => t::Position::Absolute,
            },
            overflow: taffy::Point {
                x: overflow(self.overflow.x),
                y: overflow(self.overflow.y),
            },
            scrollbar_width: 0.,
            inset: edges(&self.inset, &auto),
            margin: edges(&self.margin, &auto),
            padding: taffy::Rect {
                top: length(padding.top),
                right: length(padding.right),
                bottom: length(padding.bottom),
                left: length(padding.left),
            },
            border: taffy::Rect {
                top: border(widths.top),
                right: border(widths.right),
                bottom: border(widths.bottom),
                left: border(widths.left),
            },
            size: taffy::Size {
                width: dimension(self.size.width),
                height: dimension(self.size.height),
            },
            min_size: taffy::Size {
                width: auto(self.min_size.width),
                height: auto(self.min_size.height),
            },
            max_size: taffy::Size {
                width: auto(self.max_size.width),
                height: auto(self.max_size.height),
            },
            gap: taffy::Size {
                width: length(self.gap.width),
                height: length(self.gap.height),
            },
            align_items: self.align_items.map_or_else(Default::default, items),
            align_self: self.align_self.map(items),
            justify_self: self.justify_self.map(items),
            align_content: self.align_content.map_or_else(Default::default, content),
            justify_content: self.justify_content.map_or_else(Default::default, content),
            flex_direction: match self.flex_direction {
                FlexDirection::Row => t::FlexDirection::Row,
                FlexDirection::RowReverse => t::FlexDirection::RowReverse,
                FlexDirection::Column => t::FlexDirection::Column,
                FlexDirection::ColumnReverse => t::FlexDirection::ColumnReverse,
            },
            direction: match self.flow & Flow::RTL_ITEMS {
                0 => t::Direction::Ltr,
                _ => t::Direction::Rtl,
            },
            flex_wrap: match self.flex_wrap {
                FlexWrap::NoWrap => t::FlexWrap::NoWrap,
                FlexWrap::Wrap => t::FlexWrap::Wrap,
                FlexWrap::WrapReverse => t::FlexWrap::WrapReverse,
            },
            flex_basis: dimension(self.flex_basis),
            // A box without baselines contains its layout, so taffy reads none from inside it.
            contain: match self.baseline {
                Baseline::None => t::Contain::LAYOUT,
                _ => t::Contain::NONE,
            },
            ..Default::default()
        };
        (layout.flex_grow, layout.flex_shrink) = (self.flex_grow, self.flex_shrink);
        layout.aspect_ratio = self.aspect_ratio;
        (layout, bounds)
    }

    /// The axes where a scroll container starts at the end, like Chrome: right-to-left boxes and reversed flex
    /// boxes. Their content overflows to the left or the top, and their offsets are positive. The flex direction
    /// is physical, so a right-to-left row is a reversed row here. Rows of lines are blocks.
    pub(crate) fn scrolls_from_end(&self) -> [bool; 2] {
        let rtl = self.flow & Flow::RTL_BLOCK != 0;
        if self.display != Display::Flex || self.inline == Inline::Lines {
            return [rtl, false];
        }
        match self.flex_direction {
            FlexDirection::Row => [false, false],
            FlexDirection::RowReverse => [true, false],
            FlexDirection::Column => [rtl, false],
            FlexDirection::ColumnReverse => [rtl, true],
        }
    }

    /// A visible axis stays open, and a visible border is outside the clip.
    pub fn overflow_mask(&self, bounds: Rect) -> Option<Clip<Pixels>> {
        const FAR: Pixels = px(1e7);
        let clips = self.overflow.map(|overflow| overflow != Overflow::Visible);
        if !clips.x && !clips.y {
            return None;
        }
        // Like Chrome, the clip is the padding box, also with transparent or multicolored borders.
        let border = self.border_widths;
        let inner = Rect::between(
            bounds.origin + point(border.left, border.top),
            bounds.bottom_right() - point(border.right, border.bottom),
        );
        let (start, end) = (inner.origin, inner.bottom_right());
        let open = |clip: bool, edge: Pixels, far: Pixels| if clip { edge } else { far };
        let clip = Rect::between(
            point(open(clips.x, start.x, -FAR), open(clips.y, start.y, -FAR)),
            point(open(clips.x, end.x, FAR), open(clips.y, end.y, FAR)),
        );
        let radii = self.corner_radii.resolve(bounds.size);
        if radii.is_zero() {
            return Some(Clip::from(clip));
        }
        let inset = radii.inset(border.top, border.right, border.bottom, border.left);
        // A bevel or concave inner curve is the outer one moved in, which a curve of the padding box can't
        // match. When every curved corner is such a one and both axes clip, the curves come from the border box
        // and the clip bounds cut the padding box.
        let (x, y, shapes) = (
            radii.x.to_array(),
            radii.y.to_array(),
            radii.shape.to_array(),
        );
        if clips.x
            && clips.y
            && (0..4)
                .all(|at| (1..=2048).contains(&shapes[at]) || x[at].is_zero() || y[at].is_zero())
        {
            let (ix, iy) = (inset.x.to_array(), inset.y.to_array());
            let (mut gx, mut gy) = (x, y);
            for at in 0..4 {
                let side = if at == 0 || at == 3 {
                    border.left
                } else {
                    border.right
                };
                let edge = if at < 2 { border.top } else { border.bottom };
                (gx[at], gy[at]) = if shapes[at] < 2048 {
                    (x[at] + side, y[at] + edge)
                } else if ix[at].is_zero() || iy[at].is_zero() {
                    (Pixels::ZERO, Pixels::ZERO)
                } else {
                    // The inner bevel line, from where it meets the padding box out to the border box.
                    (
                        side + ix[at] + edge * (ix[at] / iy[at]),
                        edge + iy[at] + side * (iy[at] / ix[at]),
                    )
                };
            }
            let corners =
                |[top_left, top_right, bottom_right, bottom_left]: [Pixels; 4]| crate::Corners {
                    top_left,
                    top_right,
                    bottom_right,
                    bottom_left,
                };
            return Some(Clip {
                bounds: clip,
                rounded: bounds,
                radii: Radii {
                    x: corners(gx),
                    y: corners(gy),
                    shape: radii.shape,
                },
            });
        }
        let mut mask = Clip::rounded(inner, inset);
        mask.bounds = clip;
        Some(mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_clip_on_one_axis_leaves_the_other_visible() {
        let style = Style {
            overflow: Point::new(Overflow::Clip, Overflow::Visible),
            ..Style::default()
        };
        let mask = style
            .overflow_mask(crate::rect(px(10.), px(10.), px(20.), px(5.)))
            .unwrap();
        assert_eq!(mask.bounds.origin.x, px(10.));
        assert_eq!(mask.bounds.right(), px(30.));
        assert!(mask.bounds.origin.y < px(0.) && mask.bounds.bottom() > px(1000.));
    }

    #[test]
    fn borders_and_radii_shrink_the_clip_like_chrome() {
        let mut style = Style {
            border_widths: Edges::all(px(2.)),
            border_color: Some(Color::BLACK),
            ..Style::default()
        };
        style.overflow = Point::new(Overflow::Hidden, Overflow::Hidden);
        let bounds = crate::rect(px(0.), px(0.), px(40.), px(20.));
        let mask = style.overflow_mask(bounds).unwrap();
        assert_eq!(mask.bounds, crate::rect(px(2.), px(2.), px(36.), px(16.)));
        style.corner_radii = Radii::from(crate::Corners::all(Length::Px(px(6.))));
        let mask = style.overflow_mask(bounds).unwrap();
        assert_eq!(mask.rounded, mask.bounds);
        assert_eq!(mask.radii.x.top_left, px(4.));
        style.border_color = None;
        assert_eq!(
            style.overflow_mask(bounds).unwrap().bounds,
            crate::rect(px(2.), px(2.), px(36.), px(16.))
        );
    }

    #[test]
    fn layout_units_keep_huge_lengths() {
        assert_eq!(to_layout_unit(10_665_899., 1.), 10_665_899.);
        assert_eq!(to_layout_unit(-10_665_899., 1.), -10_665_899.);
        assert_eq!(to_layout_unit(12_000_000., 2.), 24_000_000.);
        assert_eq!(to_layout_unit(0.999_999_94, 1.), 1.);
        assert_eq!(to_layout_unit(0.99, 1.), 63. / 64.);
    }

    #[test]
    fn calc_handles_keep_the_share_and_offset() {
        for (share, offset, scale) in [
            (0.5, 8., 2.),
            (1., 0.25, 1.),
            (0.333, 1000.5, 3.),
            (0., 12., 2.),
            (0.5, -3., 1.),
        ] {
            let handle = calc(share, px(offset), scale).unwrap();
            assert_eq!(
                resolve_calc(handle, 300.),
                share * 300. + to_layout_unit(offset, scale)
            );
        }
        assert!(calc(0.5, px(0.), 2.).is_none());
        for (offset, limit) in [(5_000_000., 4_194_304.), (-5_000_000., -4_194_304.)] {
            let size = resolve_calc(calc(1., px(offset), 1.).unwrap(), 0.);
            assert!((size - limit).abs() < 1., "{offset} became {size}");
        }
    }

    #[test]
    fn styles_convert_to_taffy() {
        let style = Style {
            display: Display::Flex,
            size: Size::new(Length::Px(px(10.)), relative(0.5)),
            margin: Edges::all(Length::Auto),
            padding: Edges::all(Length::Auto),
            align_items: Some(AlignItems::Center),
            flex_basis: Length::Calc(0.5, px(4.)),
            baseline: Baseline::None,
            ..Style::default()
        };
        let (taffy, _) = style.to_taffy(2.);
        assert_eq!(taffy.display, taffy::Display::Flex);
        assert_eq!(taffy.size.width, taffy::Dimension::length(20.));
        assert_eq!(taffy.size.height, taffy::Dimension::percent(0.5));
        assert_eq!(taffy.margin.left, taffy::LengthPercentageAuto::auto());
        assert_eq!(taffy.padding.left, taffy::LengthPercentage::length(0.));
        assert_eq!(taffy.align_items, taffy::AlignItems::CENTER);
        assert_eq!(
            taffy.flex_basis,
            taffy::Dimension::calc(calc(0.5, px(4.), 2.).unwrap())
        );
        assert_eq!(taffy.contain, taffy::style::Contain::LAYOUT);
        assert_eq!(Style::default().to_taffy(1.).0.flex_shrink, 1.);
    }

    #[test]
    fn object_fit_places_images() {
        let bounds = crate::rect(px(0.), px(0.), px(100.), px(50.));
        let image = Size::new(20, 20);
        let place = |fit: ObjectFit, image| fit.place(bounds, image);
        assert_eq!(place(ObjectFit::Fill, image), bounds);
        assert_eq!(
            place(ObjectFit::Contain, image),
            crate::rect(px(25.), px(0.), px(50.), px(50.))
        );
        assert_eq!(
            place(ObjectFit::Cover, image),
            crate::rect(px(0.), px(-25.), px(100.), px(100.))
        );
        assert_eq!(
            place(ObjectFit::None, image),
            crate::rect(px(40.), px(15.), px(20.), px(20.))
        );
        assert_eq!(
            place(ObjectFit::ScaleDown, image),
            place(ObjectFit::None, image)
        );
        let big = Size::new(200, 200);
        assert_eq!(
            place(ObjectFit::ScaleDown, big),
            place(ObjectFit::Contain, big)
        );
    }
}
