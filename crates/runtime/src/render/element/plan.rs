use super::super::effects::{self, Establishes};
use crate::css;
use crate::css::{properties::Prop, properties::Style, value::units::Lengths};
use crate::style;
use renderer::{Color, Pixels, Radii};
use rustc_hash::FxHashMap;
use std::rc::Rc;

// Plans can depend only on computed style, units and pixel ratio. Box reads escape cache dependency tracking.
#[derive(PartialEq)]
pub(crate) struct Plan {
    pub(crate) made: Establishes,
    pub(crate) order: Option<i32>,
    pub(crate) opacity: f32,
    pub(crate) face: renderer::Font,
    pub(crate) line_height: f32,
    pub(crate) spacing: [Pixels; 2],
    pub(crate) smoothing: bool,
    pub(crate) ink: Option<Color>,
    pub(crate) background: Option<Color>,
    pub(crate) selection: (Option<u32>, u32),
    pub(crate) padding: Option<[f32; 4]>,
    pub(crate) borders: Option<[f32; 4]>,
    pub(crate) border_colors: [Color; 4],
    pub(crate) radii: Option<Radii<renderer::Length>>,
    pub(crate) shareable: bool,
}

impl Plan {
    fn new(style: &Style, units: &Lengths, pixel_ratio: f32) -> Self {
        let relative = |props: &[Prop]| {
            props
                .iter()
                .any(|&prop| style.get(prop).is_some_and(|value| value.contains('%')))
        };
        let ink = style.get(Prop::COLOR).and_then(css::value::color::paint);
        let text = ink.unwrap_or(Color::BLACK);
        let borders = units.borders(style, 0.);
        let corners = [
            Prop::BORDER_TOP_LEFT_RADIUS,
            Prop::BORDER_TOP_RIGHT_RADIUS,
            Prop::BORDER_BOTTOM_RIGHT_RADIUS,
            Prop::BORDER_BOTTOM_LEFT_RADIUS,
        ];
        Self {
            made: effects::establishes(style),
            order: style
                .get(Prop::Z_INDEX)
                .and_then(|value| value.parse::<i32>().ok()),
            opacity: effects::opacity(style),
            face: style::font(style),
            line_height: match style::line_height(style) {
                style::LineHeight::Fixed(_) => (units.font.lh * pixel_ratio * 64.).round(),
                _ => (units.font.lh * pixel_ratio * 64.).floor(),
            } / 64.
                / pixel_ratio,
            spacing: [Prop::LETTER_SPACING, Prop::WORD_SPACING]
                .map(|prop| style::spacing(style, units, prop)),
            smoothing: [Prop::WEBKIT_FONT_SMOOTHING, Prop::MOZ_OSX_FONT_SMOOTHING]
                .into_iter()
                .any(|name| {
                    style
                        .get(name)
                        .is_some_and(|v| v == "antialiased" || v == "grayscale")
                }),
            ink,
            background: style
                .get(Prop::BACKGROUND_COLOR)
                .and_then(css::value::color::paint)
                .filter(|color| color.a > 0.),
            selection: style::selection_colors(style, None),
            padding: (!relative(&css::properties::PADDING)).then(|| {
                units
                    .edges(style, css::properties::PADDING, 0.)
                    .map(|value| value.max(0.))
            }),
            borders: (!relative(&css::properties::BORDERS.map(|[width, ..]| width)))
                .then_some(borders),
            border_colors: css::properties::BORDERS.map(|[_, _, name]| {
                style
                    .get(name)
                    .and_then(css::value::color::paint)
                    .unwrap_or(text)
            }),
            radii: (!relative(&corners)).then(|| effects::clip::radii(style, units, || [0.; 2])),
            shareable: matches!(
                style.get(Prop::DISPLAY),
                None | Some(
                    "block" | "inline" | "inline-block" | "flex" | "inline-flex" | "flow-root"
                )
            ) && matches!(
                style.get(Prop::POSITION),
                None | Some("static" | "relative")
            ) && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
                && style
                    .get(Prop::WRITING_MODE)
                    .is_none_or(|mode| mode.starts_with("horizontal"))
                && [
                    Prop::WIDTH,
                    Prop::MIN_WIDTH,
                    Prop::MAX_WIDTH,
                    Prop::HEIGHT,
                    Prop::MIN_HEIGHT,
                    Prop::MAX_HEIGHT,
                    Prop::FLEX_BASIS,
                ]
                .iter()
                .all(|&prop| {
                    style.get(prop).is_none_or(|value| {
                        !["content", "calc-size", "stretch", "-webkit-fill"]
                            .iter()
                            .any(|keyword| value.contains(keyword))
                    })
                }),
        }
    }
}

type Stored = (Rc<Style>, Lengths, u32, Rc<Plan>);

// Retain styles used as pointer keys to prevent address reuse.
#[derive(Default)]
pub(crate) struct Plans {
    map: FxHashMap<*const Style, Vec<Stored>>,
    limit: usize,
}

impl Plans {
    pub(crate) fn get(&mut self, style: &Rc<Style>, units: &Lengths, pixel_ratio: f32) -> Rc<Plan> {
        let list = self.map.entry(Rc::as_ptr(style)).or_default();
        if let Some((.., plan)) = list
            .iter()
            .find(|(_, kept, ratio, _)| kept == units && *ratio == pixel_ratio.to_bits())
        {
            debug_assert!(
                **plan == Plan::new(style, units, pixel_ratio),
                "a style plan reads more than its style, units and pixel ratio"
            );
            return plan.clone();
        }
        let plan = Rc::new(Plan::new(style, units, pixel_ratio));
        if list.len() == 8 {
            list.remove(0);
        }
        list.push((style.clone(), *units, pixel_ratio.to_bits(), plan.clone()));
        if self.map.len() > self.limit.max(4096) {
            self.map.retain(|_, list| {
                list.retain(|(style, ..)| Rc::strong_count(style) > 1);
                !list.is_empty()
            });
            self.limit = self.map.len() * 2;
        }
        plan
    }

    pub(crate) fn clear(&mut self) {
        self.map.clear();
    }
}
