use crate::{
    BorderStyle, Bounds, ContentMask, DevicePixels, Edges, Pixels, Point, Radii, Rgba, Size, black,
    geometry::{round_stroke_to_device_pixel, to_layout_unit},
    point, px, size,
    text::{
        Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, SharedString, StrikethroughStyle,
        TextAlign, TextRun, TextWrap, UnderlineStyle, WhiteSpace, WordBreak,
    },
};

/// A length: `auto`, logical pixels, a fraction of the parent's size, or a fraction plus pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    #[default]
    Auto,
    Px(Pixels),
    Fraction(f32),
    Calc(f32, Pixels),
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
            Length::Calc(fraction, offset) => side * fraction + offset,
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
    /// The taffy calc handle of `Calc`, or `None` when it is a plain fraction. Taffy treats the handle as
    /// opaque, so it holds the fraction's bits and the offset in 1/64 device pixels instead of a pointer.
    fn calc(fraction: f32, offset: Pixels, scale: f32) -> Option<*const ()> {
        let offset = (to_layout_unit(offset.0, scale) * 64.) as i32;
        (offset != 0).then(|| {
            std::ptr::without_provenance(
                (u64::from(fraction.to_bits()) << 32 | u64::from((offset << 3) as u32)) as usize,
            )
        })
    }
    fn dimension(self, scale: f32) -> taffy::Dimension {
        match self {
            Self::Auto => taffy::Dimension::auto(),
            Self::Px(value) => taffy::Dimension::length(to_layout_unit(value.0, scale)),
            Self::Fraction(value) => taffy::Dimension::percent(value),
            Self::Calc(fraction, offset) => Self::calc(fraction, offset, scale)
                .map_or(taffy::Dimension::percent(fraction), taffy::Dimension::calc),
        }
    }
    fn auto(self, scale: f32) -> taffy::LengthPercentageAuto {
        match self {
            Self::Auto => taffy::LengthPercentageAuto::auto(),
            Self::Px(value) => taffy::LengthPercentageAuto::length(to_layout_unit(value.0, scale)),
            Self::Fraction(value) => taffy::LengthPercentageAuto::percent(value),
            Self::Calc(fraction, offset) => Self::calc(fraction, offset, scale).map_or(
                taffy::LengthPercentageAuto::percent(fraction),
                taffy::LengthPercentageAuto::calc,
            ),
        }
    }
    fn definite(self, scale: f32) -> taffy::LengthPercentage {
        match self {
            Self::Auto => taffy::LengthPercentage::length(0.),
            Self::Px(value) => taffy::LengthPercentage::length(to_layout_unit(value.0, scale)),
            Self::Fraction(value) => taffy::LengthPercentage::percent(value),
            Self::Calc(fraction, offset) => Self::calc(fraction, offset, scale).map_or(
                taffy::LengthPercentage::percent(fraction),
                taffy::LengthPercentage::calc,
            ),
        }
    }
}

/// Resolves a handle from `Length::calc` against `basis`, in device pixels.
pub(crate) fn resolve_calc(handle: *const (), basis: f32) -> f32 {
    let bits = handle.addr() as u64;
    f32::from_bits((bits >> 32) as u32) * basis + ((bits as u32 as i32) >> 3) as f32 / 64.
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

#[derive(Clone, Debug, PartialEq)]
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
    /// The box paints in the list of its nearest stacking context, by this order.
    pub z_index: Option<i32>,
    /// The box is a stacking context, which lists the boxes with a z-index below it.
    pub stacking: bool,
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
            z_index: None,
            stacking: false,
        }
    }
}

impl Style {
    /// The taffy style in device pixels. Lengths are rounded to device pixels before layout.
    pub(crate) fn to_taffy(&self, scale: f32) -> taffy::Style {
        use taffy::style as t;
        let align_items = |value: AlignItems| match value {
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

#[cfg(test)]
mod tests {
    use super::{Length, resolve_calc, to_layout_unit};
    use crate::px;

    #[test]
    fn calc_handles_keep_the_fraction_and_offset() {
        for (fraction, offset, scale) in [
            (0.5, 8., 2.),
            (1., 0.25, 1.),
            (0.333, 1000.5, 3.),
            (0., 12., 2.),
        ] {
            let handle = Length::calc(fraction, px(offset), scale).unwrap();
            assert_eq!(
                resolve_calc(handle, 300.),
                fraction * 300. + to_layout_unit(offset, scale)
            );
        }
        assert!(Length::calc(0.5, px(0.), 2.).is_none());
    }
}
