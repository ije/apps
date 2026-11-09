use super::effects::{self, Establishes};
use crate::style::{self, Lengths, Prop, Style};
use renderer::{Pixels, Radii, Rgba, rgba};
use rustc_hash::FxHashMap;
use std::rc::Rc;

// WARNING: what `create` parses of a computed style, with its units and the pixel ratio, and nothing else, so a plan
// reads no box and its memo hides no read. The cascade gives nodes that match alike one style, so the rows of a list
// share a plan. A value that holds a percentage is `None`, and `create` resolves it against its containing block.
#[derive(PartialEq)]
pub(super) struct Plan {
    pub(super) made: Establishes,
    pub(super) order: Option<i32>,
    pub(super) opacity: f32,
    pub(super) face: renderer::Font,
    pub(super) line_height: f32,
    pub(super) spacing: [Pixels; 2],
    pub(super) smoothing: bool,
    pub(super) ink: Option<(u32, u8)>,
    pub(super) background: Option<(u32, u8)>,
    pub(super) selection: (Option<u32>, u32),
    pub(super) padding: Option<[f32; 4]>,
    pub(super) borders: Option<[f32; 4]>,
    pub(super) border_colors: [Rgba; 4],
    pub(super) radii: Option<Radii<renderer::Length>>,
    // WARNING: whether the box style that `create` builds from this plan can be a function of the plan, the call, the
    // reads and a few facts of the children, see `create`. Styles that size boxes by their content, place them out of
    // flow or turn them read more of the subtree, so they are never shared.
    pub(super) shareable: bool,
}

impl Plan {
    fn new(style: &Style, units: &Lengths, pixel_ratio: f32) -> Self {
        let relative = |props: &[Prop]| {
            props
                .iter()
                .any(|&prop| style.get(prop).is_some_and(|value| value.contains('%')))
        };
        let ink = style.get(Prop::COLOR).and_then(style::color);
        let text = ink.map_or(rgba(0x000000ff), |(ink, alpha)| rgba(ink << 8 | alpha as u32));
        let borders = style::BORDERS.map(|[width, kind, _]| {
            if style
                .get(kind)
                .is_none_or(|kind| matches!(kind, "none" | "hidden"))
            {
                return 0.;
            }
            style
                .get(width)
                .map_or(Some(3.), |width| units.resolve(width, 0.))
                .unwrap_or(0.)
                .max(0.)
        });
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
                .and_then(style::color)
                .filter(|(_, alpha)| *alpha > 0),
            selection: style::selection_colors(style, None),
            padding: (!relative(&style::PADDING))
                .then(|| units.edges(style, style::PADDING, 0.).map(|value| value.max(0.))),
            borders: (!relative(&style::BORDERS.map(|[width, ..]| width))).then_some(borders),
            border_colors: style::BORDERS.map(|[_, _, name]| {
                style
                    .get(name)
                    .and_then(style::color)
                    .map_or(text, |(ink, alpha)| rgba(ink << 8 | alpha as u32))
            }),
            radii: (!relative(&corners)).then(|| effects::radii(style, units, || [0.; 2])),
            shareable: matches!(
                style.get(Prop::DISPLAY),
                None | Some("block" | "inline" | "inline-block" | "flex" | "inline-flex" | "flow-root")
            ) && matches!(style.get(Prop::POSITION), None | Some("static" | "relative"))
                && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
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
                .all(|&prop| style.get(prop).is_none_or(|value| !value.contains("content"))),
        }
    }
}

type Stored = (Rc<Style>, Lengths, u32, Rc<Plan>);

// WARNING: plans by style pointer, the last 8 units of each, which change as windows resize. An entry holds its style,
// so the pointer is not reused while it lives, and entries whose style only the memo holds go when the memo grows. A
// new scale or new fonts clear it, like kept boxes.
#[derive(Default)]
pub(super) struct Plans {
    map: FxHashMap<*const Style, Vec<Stored>>,
    limit: usize,
}

impl Plans {
    pub(super) fn get(&mut self, style: &Rc<Style>, units: &Lengths, pixel_ratio: f32) -> Rc<Plan> {
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

    pub(super) fn clear(&mut self) {
        self.map.clear();
    }
}
