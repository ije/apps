use crate::{
    BorderStyle, Bounds, ContentMask, Corners, DevicePixels, Edges, Pixels, Point, Radii, Rgba,
    Size, black,
    geometry::{round_stroke_to_device_pixel, to_layout_unit},
    point, px, size,
    text::{
        Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, SharedString, StrikethroughStyle,
        TextAlign, TextRun, TextWrap, UnderlineStyle, WhiteSpace, WordBreak,
    },
};

/// A length: `auto`, logical pixels, or a fraction of the parent's size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    #[default]
    Auto,
    Px(Pixels),
    Fraction(f32),
}

impl From<Pixels> for Length {
    fn from(value: Pixels) -> Self {
        Self::Px(value)
    }
}

/// A fraction of the parent's size, like a CSS percentage divided by 100.
pub fn relative(fraction: f32) -> Length {
    Length::Fraction(fraction)
}

impl Radii<Length> {
    pub fn to_pixels(&self, size: Size<Pixels>) -> Radii<Pixels> {
        let pixels = |length: Length, side: Pixels| match length {
            Length::Px(value) => value,
            Length::Fraction(fraction) => side * fraction,
            Length::Auto => Pixels::ZERO,
        };
        Radii {
            x: self.x.map(|length| pixels(length, size.width)),
            y: self.y.map(|length| pixels(length, size.height)),
        }
    }
    pub fn resolve(&self, size: Size<Pixels>) -> Radii<Pixels> {
        self.to_pixels(size).clamp_radii_for_quad_size(size)
    }
}

impl Length {
    pub fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
    fn dimension(self, scale: f32) -> taffy::Dimension {
        match self {
            Self::Auto => taffy::Dimension::auto(),
            Self::Px(value) => taffy::Dimension::length(to_layout_unit(value.0, scale)),
            Self::Fraction(value) => taffy::Dimension::percent(value),
        }
    }
    fn auto(self, scale: f32) -> taffy::LengthPercentageAuto {
        match self {
            Self::Auto => taffy::LengthPercentageAuto::auto(),
            Self::Px(value) => taffy::LengthPercentageAuto::length(to_layout_unit(value.0, scale)),
            Self::Fraction(value) => taffy::LengthPercentageAuto::percent(value),
        }
    }
    fn definite(self, scale: f32) -> taffy::LengthPercentage {
        match self {
            Self::Auto => taffy::LengthPercentage::length(0.),
            Self::Px(value) => taffy::LengthPercentage::length(to_layout_unit(value.0, scale)),
            Self::Fraction(value) => taffy::LengthPercentage::percent(value),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Display {
    #[default]
    Block,
    Flex,
    Grid,
    None,
}

/// Where a box finds the baseline its line aligns, and the last baseline an inline-block around it uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Baseline {
    /// Block flow: the first and the last child with a baseline, or a line's items.
    #[default]
    Auto,
    /// An inline-block: its line aligns its last baseline.
    Last,
    /// Flex and grid boxes: the first baseline is also the last.
    First,
    /// Tables have no last baseline.
    FirstOnly,
    /// A piece of a line, like a strut: its bottom border edge is its baseline.
    Bottom,
    /// No baseline: the line aligns the bottom border edge.
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Position {
    #[default]
    Relative,
    Absolute,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overflow {
    #[default]
    Visible,
    /// Clipped without making a scroll container.
    Clip,
    Hidden,
    Scroll,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlexDirection {
    #[default]
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    WrapReverse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignItems {
    Start,
    End,
    FlexStart,
    FlexEnd,
    Center,
    Baseline,
    Stretch,
}

pub type AlignSelf = AlignItems;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignContent {
    Start,
    End,
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    SpaceBetween,
    SpaceEvenly,
    SpaceAround,
}

pub type JustifyContent = AlignContent;

/// How an image fits its box, like CSS `object-fit`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ObjectFit {
    #[default]
    Fill,
    Contain,
    Cover,
    ScaleDown,
    None,
}

impl ObjectFit {
    /// The bounds of the image within `bounds`.
    pub fn get_bounds(&self, bounds: Bounds<Pixels>, image: Size<DevicePixels>) -> Bounds<Pixels> {
        let image = image.map(|value| px(value.0 as f32));
        let image_ratio = image.width / image.height;
        let bounds_ratio = bounds.size.width / bounds.size.height;
        let center = |size: Size<Pixels>| {
            Bounds::new(
                point(
                    bounds.origin.x + (bounds.size.width - size.width) / 2.,
                    bounds.origin.y + (bounds.size.height - size.height) / 2.,
                ),
                size,
            )
        };
        let contain = || {
            if bounds_ratio > image_ratio {
                size(
                    image.width * (bounds.size.height / image.height),
                    bounds.size.height,
                )
            } else {
                size(
                    bounds.size.width,
                    image.height * (bounds.size.width / image.width),
                )
            }
        };
        match self {
            Self::Fill => bounds,
            Self::Contain => center(contain()),
            Self::ScaleDown => {
                if image.width > bounds.size.width || image.height > bounds.size.height {
                    center(contain())
                } else {
                    center(image)
                }
            }
            Self::Cover => center(if bounds_ratio > image_ratio {
                size(
                    bounds.size.width,
                    image.height * (bounds.size.width / image.width),
                )
            } else {
                size(
                    image.width * (bounds.size.height / image.height),
                    bounds.size.height,
                )
            }),
            Self::None => center(image),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CursorStyle {
    #[default]
    Arrow,
    IBeam,
    Crosshair,
    ClosedHand,
    OpenHand,
    PointingHand,
    ResizeLeft,
    ResizeRight,
    ResizeLeftRight,
    ResizeUp,
    ResizeDown,
    ResizeUpDown,
    ResizeUpLeftDownRight,
    ResizeUpRightDownLeft,
    ResizeColumn,
    ResizeRow,
    IBeamCursorForVerticalLayout,
    OperationNotAllowed,
    DragLink,
    DragCopy,
    ContextualMenu,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoxShadow {
    pub color: Rgba,
    pub offset: Point<Pixels>,
    pub blur_radius: Pixels,
    pub spread_radius: Pixels,
    /// Drawn inside the element's bounds.
    pub inset: bool,
}

/// The layout and paint style of a box.
#[derive(Clone, Debug)]
pub struct Style {
    pub display: Display,
    pub visibility: Visibility,
    pub overflow: Point<Overflow>,
    /// Wheel input scrolls only the axis it moves along.
    pub restrict_scroll_to_axis: bool,
    pub position: Position,
    pub inset: Edges<Length>,
    pub size: Size<Length>,
    pub min_size: Size<Length>,
    pub max_size: Size<Length>,
    /// Width divided by height.
    pub aspect_ratio: Option<f32>,
    pub margin: Edges<Length>,
    pub padding: Edges<Length>,
    pub border_widths: Edges<Pixels>,
    pub align_items: Option<AlignItems>,
    pub align_self: Option<AlignSelf>,
    pub align_content: Option<AlignContent>,
    pub justify_content: Option<JustifyContent>,
    pub gap: Size<Length>,
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub flex_basis: Length,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub baseline: Baseline,
    /// Its parent takes its first baseline from this box, even out of flow, like a grid from its
    /// first item.
    pub baseline_source: bool,
    pub background: Option<Rgba>,
    pub border_color: Option<Rgba>,
    pub border_style: BorderStyle,
    pub corner_radii: Radii<Length>,
    pub box_shadow: Vec<BoxShadow>,
    pub text: TextStyleRefinement,
    pub mouse_cursor: Option<CursorStyle>,
    pub opacity: Option<f32>,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            display: Display::Block,
            visibility: Visibility::Visible,
            overflow: Point::default(),
            restrict_scroll_to_axis: false,
            position: Position::Relative,
            inset: Edges::all(Length::Auto),
            size: Size::default(),
            min_size: Size::default(),
            max_size: Size::default(),
            aspect_ratio: None,
            margin: Edges::all(Length::Px(Pixels::ZERO)),
            padding: Edges::all(Length::Px(Pixels::ZERO)),
            border_widths: Edges::default(),
            align_items: None,
            align_self: None,
            align_content: None,
            justify_content: None,
            gap: Size::new(Length::Px(Pixels::ZERO), Length::Px(Pixels::ZERO)),
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            flex_basis: Length::Auto,
            flex_grow: 0.,
            flex_shrink: 1.,
            baseline: Baseline::Auto,
            baseline_source: false,
            background: None,
            border_color: None,
            border_style: BorderStyle::Solid,
            corner_radii: Radii::default(),
            box_shadow: Vec::new(),
            text: TextStyleRefinement::default(),
            mouse_cursor: None,
            opacity: None,
        }
    }
}

impl Style {
    /// The taffy style in device pixels. Lengths are rounded to device pixels before layout.
    pub(crate) fn to_taffy(&self, scale: f32) -> taffy::Style {
        use taffy::style as t;
        let align_items = |value: AlignItems| match value {
            AlignItems::Start => t::AlignItems::START,
            AlignItems::End => t::AlignItems::END,
            AlignItems::FlexStart => t::AlignItems::FLEX_START,
            AlignItems::FlexEnd => t::AlignItems::FLEX_END,
            AlignItems::Center => t::AlignItems::CENTER,
            AlignItems::Baseline => t::AlignItems::BASELINE,
            AlignItems::Stretch => t::AlignItems::STRETCH,
        };
        let align_content = |value: AlignContent| match value {
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
        let overflow = |value: Overflow| match value {
            Overflow::Visible => t::Overflow::Visible,
            Overflow::Clip => t::Overflow::Clip,
            Overflow::Hidden => t::Overflow::Hidden,
            Overflow::Scroll => t::Overflow::Scroll,
        };
        let auto = |edges: &Edges<Length>| taffy::Rect {
            top: edges.top.auto(scale),
            right: edges.right.auto(scale),
            bottom: edges.bottom.auto(scale),
            left: edges.left.auto(scale),
        };
        let border = |width: Pixels| {
            t::LengthPercentage::length(round_stroke_to_device_pixel(width.0, scale))
        };
        t::Style {
            display: match self.display {
                Display::Block => t::Display::Block,
                Display::Flex => t::Display::Flex,
                Display::Grid => t::Display::Grid,
                Display::None => t::Display::None,
            },
            overflow: taffy::Point {
                x: overflow(self.overflow.x),
                y: overflow(self.overflow.y),
            },
            scrollbar_width: 0.,
            position: match self.position {
                Position::Relative => t::Position::Relative,
                Position::Absolute => t::Position::Absolute,
            },
            inset: auto(&self.inset),
            size: taffy::Size {
                width: self.size.width.dimension(scale),
                height: self.size.height.dimension(scale),
            },
            min_size: taffy::Size {
                width: self.min_size.width.auto(scale),
                height: self.min_size.height.auto(scale),
            },
            max_size: taffy::Size {
                width: self.max_size.width.auto(scale),
                height: self.max_size.height.auto(scale),
            },
            aspect_ratio: self.aspect_ratio,
            margin: auto(&self.margin),
            padding: taffy::Rect {
                top: self.padding.top.definite(scale),
                right: self.padding.right.definite(scale),
                bottom: self.padding.bottom.definite(scale),
                left: self.padding.left.definite(scale),
            },
            border: taffy::Rect {
                top: border(self.border_widths.top),
                right: border(self.border_widths.right),
                bottom: border(self.border_widths.bottom),
                left: border(self.border_widths.left),
            },
            align_items: self.align_items.map(align_items).unwrap_or_default(),
            align_self: self.align_self.map(align_items),
            align_content: self.align_content.map(align_content).unwrap_or_default(),
            justify_content: self.justify_content.map(align_content).unwrap_or_default(),
            gap: taffy::Size {
                width: self.gap.width.definite(scale),
                height: self.gap.height.definite(scale),
            },
            flex_direction: match self.flex_direction {
                FlexDirection::Row => t::FlexDirection::Row,
                FlexDirection::RowReverse => t::FlexDirection::RowReverse,
                FlexDirection::Column => t::FlexDirection::Column,
                FlexDirection::ColumnReverse => t::FlexDirection::ColumnReverse,
            },
            flex_wrap: match self.flex_wrap {
                FlexWrap::NoWrap => t::FlexWrap::NoWrap,
                FlexWrap::Wrap => t::FlexWrap::Wrap,
                FlexWrap::WrapReverse => t::FlexWrap::WrapReverse,
            },
            flex_basis: self.flex_basis.dimension(scale),
            flex_grow: self.flex_grow,
            flex_shrink: self.flex_shrink,
            contain: if self.baseline == Baseline::None {
                t::Contain::LAYOUT
            } else {
                t::Contain::NONE
            },
            ..Default::default()
        }
    }

    /// The mask that clips children, or `None` when both axes are visible. Children are clipped
    /// to the padding box, whose corners are the inner border radii.
    pub fn overflow_mask(&self, bounds: Bounds<Pixels>) -> Option<ContentMask<Pixels>> {
        let (clip_x, clip_y) = (
            self.overflow.x != Overflow::Visible,
            self.overflow.y != Overflow::Visible,
        );
        if !clip_x && !clip_y {
            return None;
        }
        let mut min = bounds.origin;
        let mut max = bounds.bottom_right();
        if self
            .border_color
            .is_some_and(|color| !color.is_transparent())
        {
            min.x += self.border_widths.left;
            max.x -= self.border_widths.right;
            min.y += self.border_widths.top;
            max.y -= self.border_widths.bottom;
        }
        let clip = Bounds::from_corners(
            point(
                if clip_x { min.x } else { bounds.origin.x },
                if clip_y { min.y } else { bounds.origin.y },
            ),
            point(
                if clip_x { max.x } else { bounds.right() },
                if clip_y { max.y } else { bounds.bottom() },
            ),
        );
        let radii = self.corner_radii.resolve(bounds.size);
        if radii.is_zero() {
            return Some(ContentMask::from(clip));
        }
        let mut mask = ContentMask::rounded(
            Bounds::from_corners(min, max),
            radii.inset(
                min.y - bounds.origin.y,
                bounds.right() - max.x,
                bounds.bottom() - max.y,
                min.x - bounds.origin.x,
            ),
        );
        mask.bounds = clip;
        Some(mask)
    }
}

/// How the text of an element and its children looks.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub color: Rgba,
    pub font_family: SharedString,
    pub font_features: FontFeatures,
    pub font_fallbacks: Option<FontFallbacks>,
    pub font_size: Pixels,
    pub line_height: Pixels,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    pub background_color: Option<Rgba>,
    pub underline: Option<UnderlineStyle>,
    pub strikethrough: Option<StrikethroughStyle>,
    pub white_space: WhiteSpace,
    /// Text that does not fit is cut and ends with this string, like CSS `text-overflow`.
    pub text_overflow: Option<SharedString>,
    pub text_align: TextAlign,
    pub line_clamp: Option<usize>,
    pub letter_spacing: Pixels,
    pub word_spacing: Pixels,
    pub text_indent: Pixels,
    pub text_wrap: TextWrap,
    pub word_break: WordBreak,
    /// Light text gets thicker strokes, like the platform's font smoothing.
    pub font_smoothing: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            color: black(),
            font_family: crate::text::SYSTEM_UI_FONT.with(std::rc::Rc::clone),
            font_features: FontFeatures::default(),
            font_fallbacks: None,
            font_size: px(16.),
            line_height: px(20.),
            font_weight: FontWeight::default(),
            font_style: FontStyle::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
            white_space: WhiteSpace::Normal,
            text_overflow: None,
            text_align: TextAlign::Left,
            line_clamp: None,
            letter_spacing: Pixels::ZERO,
            word_spacing: Pixels::ZERO,
            text_indent: Pixels::ZERO,
            text_wrap: TextWrap::Wrap,
            word_break: WordBreak::Normal,
            font_smoothing: true,
        }
    }
}

impl TextStyle {
    pub fn font(&self) -> Font {
        Font {
            family: self.font_family.clone(),
            features: self.font_features.clone(),
            fallbacks: self.font_fallbacks.clone(),
            weight: self.font_weight,
            style: self.font_style,
        }
    }

    pub fn line_height_in_pixels(&self) -> Pixels {
        self.line_height.round()
    }

    pub fn to_run(&self, len: usize) -> TextRun {
        TextRun {
            len,
            font: self.font(),
            color: self.color,
            background_color: self.background_color,
            underline: self.underline,
            strikethrough: self.strikethrough,
            letter_spacing: self.letter_spacing,
            word_spacing: self.word_spacing,
            font_size: None,
        }
    }

    pub(crate) fn refine(&mut self, refinement: &TextStyleRefinement) {
        macro_rules! refine {
            ($($field:ident),*) => {$(
                if let Some(value) = &refinement.$field {
                    self.$field = value.clone();
                }
            )*};
        }
        refine!(
            color,
            font_family,
            font_features,
            font_size,
            line_height,
            font_weight,
            font_style,
            white_space,
            text_align,
            letter_spacing,
            word_spacing,
            text_indent,
            text_wrap,
            word_break,
            font_smoothing
        );
        if let Some(value) = &refinement.font_fallbacks {
            self.font_fallbacks = Some(value.clone());
        }
        if let Some(value) = refinement.background_color {
            self.background_color = Some(value);
        }
        if let Some(value) = refinement.underline {
            self.underline = Some(value);
        }
        if let Some(value) = refinement.strikethrough {
            self.strikethrough = Some(value);
        }
        if let Some(value) = &refinement.text_overflow {
            self.text_overflow = Some(value.clone());
        }
        if let Some(value) = refinement.line_clamp {
            self.line_clamp = Some(value);
        }
    }
}

/// The text style fields an element sets for itself and its children.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextStyleRefinement {
    pub color: Option<Rgba>,
    pub font_family: Option<SharedString>,
    pub font_features: Option<FontFeatures>,
    pub font_fallbacks: Option<FontFallbacks>,
    pub font_size: Option<Pixels>,
    pub line_height: Option<Pixels>,
    pub font_weight: Option<FontWeight>,
    pub font_style: Option<FontStyle>,
    pub background_color: Option<Rgba>,
    pub underline: Option<UnderlineStyle>,
    pub strikethrough: Option<StrikethroughStyle>,
    pub white_space: Option<WhiteSpace>,
    pub text_overflow: Option<SharedString>,
    pub text_align: Option<TextAlign>,
    pub line_clamp: Option<usize>,
    pub letter_spacing: Option<Pixels>,
    pub word_spacing: Option<Pixels>,
    pub text_indent: Option<Pixels>,
    pub text_wrap: Option<TextWrap>,
    pub word_break: Option<WordBreak>,
    pub font_smoothing: Option<bool>,
}

impl TextStyleRefinement {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Builder methods shared by every element with a style.
pub trait Styled: Sized {
    fn style(&mut self) -> &mut Style;

    fn text_style(&mut self) -> &mut TextStyleRefinement {
        &mut self.style().text
    }

    fn block(mut self) -> Self {
        self.style().display = Display::Block;
        self
    }
    fn flex(mut self) -> Self {
        self.style().display = Display::Flex;
        self
    }
    fn grid(mut self) -> Self {
        self.style().display = Display::Grid;
        self
    }
    fn hidden(mut self) -> Self {
        self.style().display = Display::None;
        self
    }
    fn flex_row(mut self) -> Self {
        self.style().flex_direction = FlexDirection::Row;
        self
    }
    fn flex_col(mut self) -> Self {
        self.style().flex_direction = FlexDirection::Column;
        self
    }
    fn flex_wrap(mut self) -> Self {
        self.style().flex_wrap = FlexWrap::Wrap;
        self
    }
    fn flex_nowrap(mut self) -> Self {
        self.style().flex_wrap = FlexWrap::NoWrap;
        self
    }
    fn flex_1(mut self) -> Self {
        let style = self.style();
        style.flex_grow = 1.;
        style.flex_shrink = 1.;
        style.flex_basis = relative(0.);
        self
    }
    fn flex_shrink(mut self, value: f32) -> Self {
        self.style().flex_shrink = value;
        self
    }
    fn flex_shrink_0(mut self) -> Self {
        self.style().flex_shrink = 0.;
        self
    }
    fn flex_grow(mut self) -> Self {
        self.style().flex_grow = 1.;
        self
    }
    fn items_start(mut self) -> Self {
        self.style().align_items = Some(AlignItems::FlexStart);
        self
    }
    fn items_center(mut self) -> Self {
        self.style().align_items = Some(AlignItems::Center);
        self
    }
    fn baseline(mut self, value: Baseline) -> Self {
        self.style().baseline = value;
        self
    }
    fn justify_center(mut self) -> Self {
        self.style().justify_content = Some(JustifyContent::Center);
        self
    }
    fn absolute(mut self) -> Self {
        self.style().position = Position::Absolute;
        self
    }
    fn relative(mut self) -> Self {
        self.style().position = Position::Relative;
        self
    }
    fn top(mut self, value: impl Into<Length>) -> Self {
        self.style().inset.top = value.into();
        self
    }
    fn right(mut self, value: impl Into<Length>) -> Self {
        self.style().inset.right = value.into();
        self
    }
    fn bottom(mut self, value: impl Into<Length>) -> Self {
        self.style().inset.bottom = value.into();
        self
    }
    fn left(mut self, value: impl Into<Length>) -> Self {
        self.style().inset.left = value.into();
        self
    }
    fn top_0(mut self) -> Self {
        self.style().inset.top = Length::Px(Pixels::ZERO);
        self
    }
    fn left_0(mut self) -> Self {
        self.style().inset.left = Length::Px(Pixels::ZERO);
        self
    }
    fn w(mut self, value: impl Into<Length>) -> Self {
        self.style().size.width = value.into();
        self
    }
    fn h(mut self, value: impl Into<Length>) -> Self {
        self.style().size.height = value.into();
        self
    }
    fn size(mut self, value: impl Into<Length>) -> Self {
        let value = value.into();
        self.style().size = Size::new(value, value);
        self
    }
    fn size_full(mut self) -> Self {
        self.style().size = Size::new(relative(1.), relative(1.));
        self
    }
    fn w_full(mut self) -> Self {
        self.style().size.width = relative(1.);
        self
    }
    fn h_full(mut self) -> Self {
        self.style().size.height = relative(1.);
        self
    }
    fn min_w(mut self, value: impl Into<Length>) -> Self {
        self.style().min_size.width = value.into();
        self
    }
    fn min_h(mut self, value: impl Into<Length>) -> Self {
        self.style().min_size.height = value.into();
        self
    }
    fn max_w(mut self, value: impl Into<Length>) -> Self {
        self.style().max_size.width = value.into();
        self
    }
    fn max_h(mut self, value: impl Into<Length>) -> Self {
        self.style().max_size.height = value.into();
        self
    }
    fn mt(mut self, value: impl Into<Length>) -> Self {
        self.style().margin.top = value.into();
        self
    }
    fn mr(mut self, value: impl Into<Length>) -> Self {
        self.style().margin.right = value.into();
        self
    }
    fn mb(mut self, value: impl Into<Length>) -> Self {
        self.style().margin.bottom = value.into();
        self
    }
    fn ml(mut self, value: impl Into<Length>) -> Self {
        self.style().margin.left = value.into();
        self
    }
    fn mt_auto(mut self) -> Self {
        self.style().margin.top = Length::Auto;
        self
    }
    fn mr_auto(mut self) -> Self {
        self.style().margin.right = Length::Auto;
        self
    }
    fn mb_auto(mut self) -> Self {
        self.style().margin.bottom = Length::Auto;
        self
    }
    fn ml_auto(mut self) -> Self {
        self.style().margin.left = Length::Auto;
        self
    }
    fn pt(mut self, value: impl Into<Length>) -> Self {
        self.style().padding.top = value.into();
        self
    }
    fn pr(mut self, value: impl Into<Length>) -> Self {
        self.style().padding.right = value.into();
        self
    }
    fn pb(mut self, value: impl Into<Length>) -> Self {
        self.style().padding.bottom = value.into();
        self
    }
    fn pl(mut self, value: impl Into<Length>) -> Self {
        self.style().padding.left = value.into();
        self
    }
    fn gap_x(mut self, value: impl Into<Length>) -> Self {
        self.style().gap.width = value.into();
        self
    }
    fn gap_y(mut self, value: impl Into<Length>) -> Self {
        self.style().gap.height = value.into();
        self
    }
    fn border(mut self, value: Pixels) -> Self {
        self.style().border_widths = Edges::all(value);
        self
    }
    fn border_t(mut self, value: Pixels) -> Self {
        self.style().border_widths.top = value;
        self
    }
    fn border_r(mut self, value: Pixels) -> Self {
        self.style().border_widths.right = value;
        self
    }
    fn border_b(mut self, value: Pixels) -> Self {
        self.style().border_widths.bottom = value;
        self
    }
    fn border_l(mut self, value: Pixels) -> Self {
        self.style().border_widths.left = value;
        self
    }
    fn border_color(mut self, color: Rgba) -> Self {
        self.style().border_color = Some(color);
        self
    }
    fn rounded_tl(mut self, value: Pixels) -> Self {
        let radii = &mut self.style().corner_radii;
        (radii.x.top_left, radii.y.top_left) = (Length::Px(value), Length::Px(value));
        self
    }
    fn rounded_tr(mut self, value: Pixels) -> Self {
        let radii = &mut self.style().corner_radii;
        (radii.x.top_right, radii.y.top_right) = (Length::Px(value), Length::Px(value));
        self
    }
    fn rounded_br(mut self, value: Pixels) -> Self {
        let radii = &mut self.style().corner_radii;
        (radii.x.bottom_right, radii.y.bottom_right) = (Length::Px(value), Length::Px(value));
        self
    }
    fn rounded_bl(mut self, value: Pixels) -> Self {
        let radii = &mut self.style().corner_radii;
        (radii.x.bottom_left, radii.y.bottom_left) = (Length::Px(value), Length::Px(value));
        self
    }
    fn rounded_full(mut self) -> Self {
        self.style().corner_radii = Corners::all(Length::Px(px(f32::MAX / 4.))).into();
        self
    }
    fn bg(mut self, color: Rgba) -> Self {
        self.style().background = Some(color);
        self
    }
    fn opacity(mut self, opacity: f32) -> Self {
        self.style().opacity = Some(opacity);
        self
    }
    fn cursor(mut self, cursor: CursorStyle) -> Self {
        self.style().mouse_cursor = Some(cursor);
        self
    }
    fn overflow_hidden(mut self) -> Self {
        self.style().overflow = Point::new(Overflow::Hidden, Overflow::Hidden);
        self
    }
    fn overflow_y_scroll(mut self) -> Self {
        self.style().overflow.y = Overflow::Scroll;
        self
    }
    fn overflow_y_hidden(mut self) -> Self {
        self.style().overflow.y = Overflow::Hidden;
        self
    }
    fn restrict_scroll_to_axis(mut self) -> Self {
        self.style().restrict_scroll_to_axis = true;
        self
    }
    fn text_color(mut self, color: Rgba) -> Self {
        self.text_style().color = Some(color);
        self
    }
    fn text_size(mut self, size: Pixels) -> Self {
        self.text_style().font_size = Some(size);
        self
    }
    fn line_height(mut self, height: Pixels) -> Self {
        self.text_style().line_height = Some(height);
        self
    }
    fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.text_style().font_family = Some(family.into());
        self
    }
    fn font_weight(mut self, weight: FontWeight) -> Self {
        self.text_style().font_weight = Some(weight);
        self
    }
    fn whitespace_nowrap(mut self) -> Self {
        self.text_style().white_space = Some(WhiteSpace::Nowrap);
        self
    }
    fn text_left(mut self) -> Self {
        self.text_style().text_align = Some(TextAlign::Left);
        self
    }
    fn text_center(mut self) -> Self {
        self.text_style().text_align = Some(TextAlign::Center);
        self
    }
    fn text_right(mut self) -> Self {
        self.text_style().text_align = Some(TextAlign::Right);
        self
    }
}
