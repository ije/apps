use crate::css::properties::{Prop, Style};
use crate::css::value::tokens::number;
use crate::css::value::units::{FontUnits, Lengths};
use crate::dom::Node;
use crate::style::cascade::computed::{CAPTION, CELL, FOOTER, HEADER, ROW, table_box};
use crate::style::cascade::container::Container;
use crate::style::cascade::node::NodePass;
use crate::style::font;
use crate::style::used::{blockify, border_spacing, pixels};
use rustc_hash::FxHashMap;
use std::{collections::HashMap, rc::Rc};

// Font metrics per font: its face and the advances of `0` and `水` and the x and cap heights at size 1.
pub(crate) type FontMetrics = HashMap<renderer::Font, (renderer::FontId, [f32; 4])>;

// The inputs of one node that the phases of its style read.
#[derive(Clone, Copy)]
pub(crate) struct Element<'a> {
    pub(crate) id: usize,
    pub(crate) node: &'a Node,
    pub(crate) parent_style: Option<&'a Rc<Style>>,
    // The font units of the parent.
    pub(crate) parent: FontUnits,
    pub(crate) parent_role: u16,
    pub(crate) item: bool,
    pub(crate) table_item: bool,
    pub(crate) seen: &'a Option<Rc<Container>>,
}

impl NodePass<'_> {
    // The used values of a node: its table role, its font weight, family and size, its units and line height, and
    // its border widths. Returns the role, the units, and whether the font size or line height read a container and
    // the viewport.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn used_values(
        &self,
        style: &mut Style,
        element: Element,
        explicit_size: bool,
        sized: &mut std::collections::HashSet<usize>,
        containers: &FxHashMap<usize, [f32; 2]>,
        font_metrics: &mut FontMetrics,
        root: &mut FontUnits,
    ) -> (u16, Lengths, bool, bool) {
        let Element {
            id,
            node,
            parent_style,
            parent,
            parent_role,
            item,
            table_item,
            seen,
        } = element;
        let (context, [width, height], scale, native_theme, text_system) = (
            self.context,
            self.viewport,
            self.scale,
            self.native_theme,
            self.text_system,
        );
        let mut role = 0;
        if !node.tag.starts_with('#') {
            let root = node.parent == Some(0);
            if root && style.get(Prop::DISPLAY) == Some("contents") {
                style.insert(Prop::DISPLAY, "block");
            }
            blockify(style, item || root, table_item);
            let grouped = table_box(parent_role) || matches!(parent_role, 5..=7);
            let display;
            (role, display) = match style.get(Prop::DISPLAY).unwrap_or("inline") {
                "table" => (1, "grid"),
                "inline-table" => (2, "inline-grid"),
                // A `<table>` box holds the anonymous table of its rows.
                "block" if node.tag == "table" => (3, "grid"),
                "table-row" if grouped => (ROW, "contents"),
                "table-row-group" if table_box(parent_role) => (5, "contents"),
                "table-header-group" if table_box(parent_role) => (HEADER, "contents"),
                "table-footer-group" if table_box(parent_role) => (FOOTER, "contents"),
                "table-row" => (10, "grid"),
                "table-row-group" | "table-footer-group" => (11, "grid"),
                "table-header-group" => (12, "grid"),
                "table-cell" => (CELL, "block"),
                "table-caption" => (CAPTION, "block"),
                _ => (0, ""),
            };
            if role != 0 {
                style.insert(Prop::DISPLAY, display);
            }
            // Like a table, the grid is as wide as its content when its width is `auto`.
            if table_box(role) && style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
                style.insert(Prop::WIDTH, "fit-content");
            }
        }

        if table_box(role) {
            border_spacing(style);
        }
        let parent_weight = parent_style
            .and_then(|parent| parent.get(Prop::FONT_WEIGHT))
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(400.);
        if let Some(weight) = style.get(Prop::FONT_WEIGHT) {
            let value = match weight {
                "normal" => 400.,
                "bold" => 700.,
                "bolder" if parent_weight < 350. => 400.,
                "bolder" if parent_weight < 550. => 700.,
                "bolder" => 900f32.max(parent_weight),
                "lighter" if parent_weight < 550. => 100f32.min(parent_weight),
                "lighter" if parent_weight < 750. => 400.,
                "lighter" => 700.,
                value => value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| (1. ..=1000.).contains(v))
                    .unwrap_or(parent_weight),
            };
            style.insert(Prop::FONT_WEIGHT, value.to_string());
        }
        if let Some(family) = context.and_then(|context| {
            context
                .aliases
                .get(style.get(Prop::FONT_FAMILY)?.trim_matches(['\'', '"']))
        }) {
            style.insert(Prop::FONT_FAMILY, family.as_str());
        }
        let mut units = Lengths {
            font: parent,
            root: *root,
            viewport: [width, height],
            container: None,
        };
        let queried = [Prop::FONT_SIZE, Prop::LINE_HEIGHT]
            .into_iter()
            .any(|prop| style.get(prop).is_some_and(|value| value.contains("cq")));
        if queried
            && let Some(container) =
                std::iter::successors(seen.as_deref(), |link| link.up.as_deref())
                    .find(|link| link.sized)
        {
            sized.insert(container.id);
            units.container = containers.get(&container.id).copied();
        }
        let viewport = queried && units.container.is_none()
            || [Prop::FONT_SIZE, Prop::LINE_HEIGHT]
                .into_iter()
                .any(|prop| {
                    style.get(prop).is_some_and(|value| {
                        ["vw", "vh", "vi", "vb", "vmin", "vmax"]
                            .iter()
                            .any(|unit| value.contains(unit))
                    })
                });
        let size = match style.get(Prop::FONT_SIZE).unwrap_or("medium") {
            "xx-small" => Some(9.),
            "x-small" => Some(10.),
            "small" => Some(13.),
            "medium" => Some(16.),
            "large" => Some(18.),
            "x-large" => Some(24.),
            "xx-large" => Some(32.),
            "xxx-large" => Some(48.),
            "larger" => Some(parent.em * 1.2),
            "smaller" => Some(parent.em / 1.2),
            value => units.resolve(value, parent.em),
        }
        .filter(|size| size.is_finite() && *size >= 0.)
        // Glyph rasterization allocates by font size, so huge sizes abort the process.
        .map(|size| size.min(10_000.))
        .unwrap_or(parent.em);
        let monospace = |style: Option<&Style>| {
            style
                .and_then(|style| style.get(Prop::FONT_FAMILY))
                .is_some_and(|family| family.trim().eq_ignore_ascii_case("monospace"))
        };
        let absolute = match style.get(Prop::FONT_SIZE).filter(|_| explicit_size) {
            None => parent.absolute,
            Some(value)
                if value.starts_with("calc(")
                    || matches!(value, "larger" | "smaller")
                    || ["%", "em", "ex", "ch", "cap", "ic", "lh"]
                        .iter()
                        .any(|unit| value.ends_with(unit)) =>
            {
                parent.absolute
            }
            Some(value) => value.starts_with(|c: char| c.is_ascii_digit() || c == '.'),
        };
        let size = if !explicit_size
            && !parent.absolute
            && monospace(Some(style))
            && !monospace(parent_style.map(Rc::as_ref))
        {
            size * 13. / 16.
        } else {
            size
        };
        units.font = FontUnits::new(size);
        units.font.absolute = absolute;
        if let Some(text) = text_system {
            let font = font(style);
            let (id, metrics) = font_metrics.entry(font.clone()).or_insert_with(|| {
                let id = text.face(&font);
                let size = renderer::px(1.);
                (
                    id,
                    [
                        text.advance(id, size, '0').map(f32::from).unwrap_or(0.5),
                        text.metrics(id).x_height * size.0,
                        text.metrics(id).cap_height * size.0,
                        text.advance(id, size, '水').map(f32::from).unwrap_or(1.),
                    ],
                )
            });
            let [ch, ex, cap, ic] = metrics.map(|value| value * size);
            units.font = FontUnits {
                em: size,
                absolute,
                ch,
                ex,
                cap,
                ic,
                lh: text
                    .line_metrics(*id, renderer::px(size * scale))
                    .into_iter()
                    .map(f32::from)
                    .sum::<f32>()
                    / scale,
            };
        }
        let ratio = ((native_theme >> 16) as f32).max(100.) / 100.;
        if let Some(text) = style.get(Prop::LINE_HEIGHT) {
            let mut line_units = units;
            line_units.font.lh = parent.lh;
            if node.tag == "html" {
                line_units.root = units.font;
                line_units.root.lh = parent.lh;
            }
            if let Some((value, dimensional)) = line_units.parse(text, size)
                && value >= 0.
            {
                units.font.lh =
                    if dimensional { value } else { value * size }.min(33_554_431. / ratio);
                style.insert(
                    Prop::LINE_HEIGHT,
                    // Like Chrome, whose layout units count device pixels, a length is at most 2^25 device
                    // pixels, and a number gives at most the f32 range in device pixels.
                    if dimensional {
                        format!("{}px", value.min(33_554_432. / ratio))
                    } else if value >= 33_554_428. {
                        units
                            .math_unclamped(text, f32::NAN, &[])
                            .map_or(value, |(number, _)| {
                                (number as f32).min(f32::MAX / ratio / size)
                            })
                            .to_string()
                    } else {
                        value.to_string()
                    },
                );
            }
        }
        // Like Chrome, border and outline widths snap to device pixels, and a width or offset under one device
        // pixel is one device pixel.
        for prop in [
            Prop::BORDER_TOP_WIDTH,
            Prop::BORDER_RIGHT_WIDTH,
            Prop::BORDER_BOTTOM_WIDTH,
            Prop::BORDER_LEFT_WIDTH,
            Prop::OUTLINE_WIDTH,
            Prop::COLUMN_RULE_WIDTH,
            Prop::OUTLINE_OFFSET,
        ] {
            let Some(value) = style.get(prop) else {
                continue;
            };
            // Keywords and whole device pixels, the common case, stay without parsing.
            let whole = |number: &str| {
                number
                    .parse::<f32>()
                    .is_ok_and(|number| (number * ratio).fract() == 0.)
            };
            if matches!(value, "0" | "thin" | "medium" | "thick")
                || value.strip_suffix("px").is_some_and(whole)
            {
                continue;
            }
            let Some(width) = units.resolve(value, f32::NAN) else {
                continue;
            };
            let device = width * ratio;
            let snapped = match device {
                _ if device != 0. && device.abs() < 1. => device.signum(),
                _ => device.trunc(),
            } / ratio;
            if snapped != width {
                style.insert(prop, format!("{}px", number(f64::from(snapped))));
            }
        }
        if id == 0 || node.tag == "html" {
            *root = units.font;
        }
        units.root = *root;
        style.insert(Prop::FONT_SIZE, format!("{size}px"));
        // Like Chrome, these keywords are 1, 3 and 5 24ths of the font size.
        if let Some(index) = (style.get(Prop::WEBKIT_TEXT_STROKE_WIDTH)).and_then(|value| {
            ["thin", "medium", "thick"]
                .iter()
                .position(|name| *name == value)
        }) {
            let width = units.font.em * (index * 2 + 1) as f32 / 24.;
            style.insert(Prop::WEBKIT_TEXT_STROKE_WIDTH, format!("{width}px"));
        }
        // Children inherit these lengths, not their units.
        for prop in [
            Prop::LETTER_SPACING,
            Prop::WORD_SPACING,
            Prop::TEXT_INDENT,
            Prop::TEXT_UNDERLINE_OFFSET,
            Prop::WEBKIT_TEXT_STROKE_WIDTH,
            Prop::TEXT_SHADOW,
        ] {
            if let Some(value) = style.get(prop).and_then(|value| pixels(value, &units)) {
                style.insert(prop, value);
            }
        }
        (role, units, queried, viewport)
    }
}
