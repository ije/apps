// The intrinsic sizes of replaced elements and form controls, and the place of a child in its cell.
use crate::css;
use crate::css::properties::Prop;
use crate::dom::Document;
use crate::style;
use renderer::{Child, ControlKind, Rect, point, px, size};

// Font units from the pseudo-element's own font, as the style pass gives them to an element.
pub(crate) fn pseudo_lengths(
    units: &css::value::units::Lengths,
    style: &css::properties::Style,
    pseudo: &css::properties::Style,
    text: &renderer::TextSystem,
    pixel_ratio: f32,
) -> css::value::units::Lengths {
    let mut lengths = *units;
    let size = pseudo
        .get(Prop::FONT_SIZE)
        .and_then(|value| units.resolve(value, units.font.em))
        .unwrap_or(units.font.em);
    if size == units.font.em
        && [
            Prop::FONT_FAMILY,
            Prop::FONT_WEIGHT,
            Prop::FONT_STYLE,
            Prop::LINE_HEIGHT,
        ]
        .into_iter()
        .all(|prop| pseudo.get(prop) == style.get(prop))
    {
        return lengths;
    }
    let face = text.face(&style::font(pseudo));
    let metrics = text.metrics(face);
    let advance =
        |glyph, default: f32| text.advance(face, px(1.), glyph).map_or(default, f32::from) * size;
    lengths.font = css::value::units::FontUnits {
        em: size,
        ch: advance('0', 0.5),
        ex: metrics.x_height * size,
        cap: metrics.cap_height * size,
        ic: advance('\u{6c34}', 1.),
        lh: match style::line_height(pseudo) {
            style::LineHeight::Fixed(height) => height,
            style::LineHeight::Scale(scale) => scale * size,
            style::LineHeight::Normal => {
                text.line_metrics(face, px(size * pixel_ratio))
                    .into_iter()
                    .map(f32::from)
                    .sum::<f32>()
                    / pixel_ratio
            }
        },
        absolute: units.font.absolute,
    };
    lengths
}

pub(crate) fn aspect_ratio(value: &str) -> Option<f32> {
    let value = value.replace('/', " / ");
    let parts = css::value::tokens::components(&value, ' ');
    let numbers: Vec<f32> = parts.iter().filter_map(|part| part.parse().ok()).collect();
    let ratio = match numbers.as_slice() {
        [a, b, ..] if parts.contains(&"/") => a / b,
        [a, ..] => *a,
        _ => return None,
    };
    (ratio.is_finite() && ratio > 0.).then_some(ratio)
}

pub(crate) fn dimension(value: &str, integer: bool) -> Option<f32> {
    let value = value.trim_start_matches(['\t', '\n', '\x0c', '\r', ' ']);
    let value = if integer {
        value.strip_prefix('+').unwrap_or(value)
    } else {
        value
    };
    let digits = |from: usize| {
        value.as_bytes()[from..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    let whole = digits(0);
    let end = match value.as_bytes().get(whole) {
        Some(b'.') if !integer => whole + 1 + digits(whole + 1),
        _ => whole,
    };
    if whole == 0 || !integer && value.as_bytes().get(end) == Some(&b'%') {
        return None;
    }
    if integer {
        return value[..end]
            .parse::<u32>()
            .ok()
            .filter(|&value| value <= i32::MAX as u32)
            .map(|value| value as f32);
    }
    value[..end]
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
}

pub(crate) fn text_field(node: &crate::dom::Node, style: &css::properties::Style) -> bool {
    node.tag == "textarea"
        || node.tag == "input"
            && node.native_control(style).is_none()
            && ![
                "checkbox", "radio", "button", "submit", "reset", "image", "file", "color",
                "range", "hidden",
            ]
            .iter()
            .any(|other| node.input_type().eq_ignore_ascii_case(other))
}

pub(crate) fn natural_size(
    doc: &Document,
    id: usize,
    decoded: Option<[f32; 2]>,
    style: &css::properties::Style,
) -> Option<([f32; 2], Option<f32>)> {
    let node = &doc.nodes[id];
    let attr = |name: &str, default: f32| {
        node.attrs
            .get(name)
            .and_then(|value| dimension(value, node.tag == "canvas"))
            .unwrap_or(default)
    };
    let pictured = |size: [f32; 2]| {
        (
            size,
            (size[0] > 0. && size[1] > 0.).then(|| size[0] / size[1]),
        )
    };
    let kind = if node.tag == "input" {
        node.input_type()
    } else {
        ""
    };
    if let Some(native) = node
        .native_control(style)
        .filter(|native| *native != renderer::ControlKind::Button)
    {
        return Some((self::native_size(native), None));
    }
    match (node.tag.as_str(), decoded.filter(|_| !broken_image(node))) {
        ("canvas", _) => Some(pictured([attr("width", 300.), attr("height", 150.)])),
        ("img" | "video", Some(size)) => Some(pictured(size)),
        ("video", None) => Some(([300., 150.], None)),
        ("img", None) => {
            let (broken, alt) = (broken_image(node), node.attrs.get("alt"));
            match ["width", "height"].map(|name| {
                node.attrs
                    .get(name)
                    .and_then(|value| dimension(value, false))
            }) {
                [Some(width), Some(height)] if !broken || alt.is_none_or(|alt| alt.is_empty()) => {
                    Some(pictured([width, height]))
                }
                _ if !broken || alt.is_some_and(|alt| !alt.is_empty()) => None,
                _ if alt.is_none() && node.attrs.contains_key("src") => Some(([16., 16.], None)),
                _ => Some(([0., 0.], None)),
            }
        }
        ("object", _)
            if node.attrs.contains_key("data")
                || node.children.iter().any(|&child| {
                    let child = &doc.nodes[child];
                    !matches!(child.tag.as_str(), "param" | "#comment")
                        && (child.tag != "#text" || !child.text.trim_ascii().is_empty())
                }) =>
        {
            None
        }
        ("iframe" | "object" | "embed", _) => {
            Some(([attr("width", 300.), attr("height", 150.)], None))
        }
        ("input", _) if kind.eq_ignore_ascii_case("range") => Some(([129., 16.], None)),
        ("input", _)
            if kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio") =>
        {
            Some(([13., 13.], None))
        }
        _ => None,
    }
}

pub(crate) fn broken_image(node: &crate::dom::Node) -> bool {
    node.tag == "img"
        && (node.state & crate::dom::BROKEN != 0
            || !node.attrs.contains_key("src") && !node.attrs.contains_key("srcset"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn field_size(
    doc: &Document,
    id: usize,
    style: &css::properties::Style,
    units: &css::value::units::Lengths,
    text: &renderer::TextSystem,
    pixel_ratio: f32,
    width: bool,
    wrap_width: Option<f32>,
) -> [f32; 2] {
    let node = &doc.nodes[id];
    let textarea = node.tag == "textarea";
    if style.get(Prop::FIELD_SIZING) == Some("content") {
        let value = node.shown_text(
            node.current_value()
                .map_or_else(|| doc.text(id), str::to_owned),
        );
        let mut shown = if value.is_empty() {
            node.attrs.get("placeholder").cloned().unwrap_or_default()
        } else {
            value
        };
        if let Some(mode) = style
            .get(Prop::TEXT_TRANSFORM)
            .filter(|mode| *mode != "none")
        {
            let len = shown.len();
            style::text_transform(&mut shown, 0..len, mode);
        }
        let run = style::text_run(style, units, shown.len());
        let lines = text.shape_text(
            &shown,
            px(units.font.em),
            &[run],
            &renderer::Wrapping {
                width: wrap_width
                    .filter(|_| {
                        textarea && !matches!(style.get(Prop::WHITE_SPACE), Some("pre" | "nowrap"))
                    })
                    .map(px),
                word_break: style::word_break(style),
                ..Default::default()
            },
        );
        return [
            if width {
                lines
                    .iter()
                    .map(|line| f32::from(line.line.width))
                    .fold(0., f32::max)
                    + 1.
            } else {
                0.
            },
            if textarea {
                lines.iter().map(|line| line.rows()).sum::<usize>().max(1) as f32 * units.font.lh
            } else {
                field_line(node, style, units, text, pixel_ratio)
            },
        ];
    }
    let count = |name, default| {
        node.attrs
            .get(name)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|&value| value > 0)
            .unwrap_or(default) as f32
    };
    let width = width.then(|| {
        let face = style::font(style);
        let em = px(units.font.em);
        let family = style
            .get(Prop::FONT_FAMILY)
            .and_then(|value| value.split(',').next())
            .map(|value| value.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
            .unwrap_or_default();
        let id = text.face(&face);
        let scale = pixel_ratio;
        let (char_width, extra) = if "|american typewriter|arial hebrew|chalkboard|cochin|\
            corsiva hebrew|courier|euphemia ucas|geneva|gill sans|hei|helvetica|hoefler text|\
            inaimathi|kai|lucida grande|marker felt|monaco|mshtakan|new peninim mt|osaka|\
            raanana|stheiti|symbol|times|apple braille|apple ligothic|apple lisung|\
            apple symbols|applegothic|applemyungjo|#gungseo|#headlinea|#pcmyungjo|#pilgi|"
            .contains(&format!("|{family}|"))
        {
            let zero = text
                .advance(id, em, '0')
                .map_or(units.font.em / 2., f32::from);
            (zero * scale, 0.)
        } else {
            let average = if face.family.as_ref() == ".SystemUIFont" {
                (text.metrics(id).x_height * em.0 * scale).round()
            } else {
                text.advance(id, em, 'x')
                    .map_or(units.font.em / 2., f32::from)
                    * scale
            };
            let ascent = f32::from(text.line_metrics(id, em * scale)[0]);
            (average, if textarea { 0. } else { ascent - average })
        };
        if textarea {
            let bar = match style.get(Prop::SCROLLBAR_WIDTH) {
                _ if style.get(Prop::OVERFLOW_Y).is_some_and(|v| v == "hidden") => 0.,
                Some("none") => 0.,
                Some("thin") => 14.,
                _ => 16.,
            };
            (char_width * count("cols", 20)).ceil() / scale + bar
        } else {
            (char_width * count("size", 20) + extra).ceil() / scale
        }
    });
    [
        width.unwrap_or(0.),
        if textarea {
            count("rows", 2) * units.font.lh
        } else {
            field_line(node, style, units, text, pixel_ratio)
        },
    ]
}

/// The line height of a field. Like Chrome, a single-line field whose `line-height` is not above its font size, in
/// device pixels, takes the normal line height.
pub(crate) fn field_line(
    node: &crate::dom::Node,
    style: &css::properties::Style,
    units: &css::value::units::Lengths,
    text: &renderer::TextSystem,
    pixel_ratio: f32,
) -> f32 {
    let pixel_ratio = pixel_ratio.max(1.);
    if node.tag == "textarea"
        || (units.font.lh * pixel_ratio).floor() > (units.font.em * pixel_ratio).round()
    {
        return units.font.lh;
    }
    text.line_metrics(
        text.face(&style::font(style)),
        px(units.font.em * pixel_ratio),
    )
    .into_iter()
    .map(f32::from)
    .sum::<f32>()
        / pixel_ratio
}

/// The natural content size of a file input with the box of its button: the button and label, or 34 digits.
pub(crate) fn file_size(
    style: &css::properties::Style,
    button: &css::properties::Style,
    units: &css::value::units::Lengths,
    text: &renderer::TextSystem,
    pixel_ratio: f32,
) -> [f32; 2] {
    let em = px(units.font.em);
    let width = |label: &str| {
        f32::from(
            text.shape_line(label, em, &style::text_run(style, units, label.len()))
                .line
                .width,
        )
    };
    let zero = text
        .advance(text.face(&style::font(style)), em, '0')
        .map_or(units.font.em / 2., f32::from);
    let [padding, margin] = [css::properties::PADDING, css::properties::MARGIN]
        .map(|sides| units.edges(button, sides, 0.));
    let borders = units.borders(button, 0.);
    let edges = |side: usize| padding[side] + margin[side] + borders[side];
    // The button text takes the font size of `::file-selector-button`.
    let scale = button
        .get(Prop::FONT_SIZE)
        .and_then(|value| units.resolve(value, units.font.em))
        .map_or(1., |size| size / units.font.em);
    let content = width("Choose File") * scale + edges(1) + edges(3) + width("No file chosen");
    [
        layout_ceil((zero * 34.).max(content), pixel_ratio),
        units
            .font
            .lh
            .max(units.font.lh * scale + edges(0) + edges(2)),
    ]
}

pub(crate) fn layout_ceil(size: f32, pixel_ratio: f32) -> f32 {
    (size * pixel_ratio * 64.).ceil() / 64. / pixel_ratio
}

pub(crate) fn place(
    child: Child,
    [top, right, bottom, left]: [f32; 4],
    pixel_ratio: f32,
    [x, y, cell_width, cell_height]: [f32; 4],
) -> Child {
    let place = Rect {
        origin: point(px(x), px(y)),
        size: size(
            px(layout_ceil(
                (cell_width - right - left).max(0.),
                pixel_ratio,
            )),
            px((cell_height - top - bottom).max(0.)),
        ),
    };
    match child {
        Child::Node(mut node) => {
            let style = node.style();
            style.inset.left = place.origin.x.into();
            style.inset.top = place.origin.y.into();
            style.size = place.size.map(Into::into);
            Child::Node(node)
        }
        Child::Kept(id) | Child::Placed(id, _) => Child::Placed(id, place),
    }
}

pub(crate) fn legend(
    doc: &Document,
    styles: &style::ComputedStyles,
    fieldset: usize,
) -> Option<(usize, f32, f32)> {
    let corners = [
        Prop::BORDER_TOP_LEFT_RADIUS,
        Prop::BORDER_TOP_RIGHT_RADIUS,
        Prop::BORDER_BOTTOM_RIGHT_RADIUS,
        Prop::BORDER_BOTTOM_LEFT_RADIUS,
    ];
    if corners.into_iter().any(|corner| {
        styles.values[fieldset]
            .get(corner)
            .is_some_and(|value| !matches!(value, "0" | "0px"))
    }) {
        return None;
    }
    let legend = doc.nodes[fieldset]
        .children
        .iter()
        .copied()
        .find(|&child| {
            !doc.nodes[child].tag.starts_with('#')
                && styles.values[child].get(Prop::DISPLAY) != Some("none")
        })?;
    let style = &styles.values[legend];
    if doc.nodes[legend].tag != "legend"
        || style.get(Prop::FLOAT).is_some_and(|value| value != "none")
        || style
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "absolute" | "fixed"))
    {
        return None;
    }
    let units = styles.lengths[legend];
    let padding = units.edges(style, css::properties::PADDING, 0.);
    let border = |[width, kind, _]: [Prop; 3]| {
        if style
            .get(kind)
            .is_none_or(|kind| matches!(kind, "none" | "hidden"))
        {
            0.
        } else {
            style
                .get(width)
                .and_then(|value| units.resolve(value, 0.))
                .unwrap_or(3.)
        }
    };
    let height = units.font.lh
        + padding[0]
        + padding[2]
        + border(css::properties::BORDERS[0])
        + border(css::properties::BORDERS[2]);
    let margin = units.edges(style, css::properties::MARGIN, 0.);
    Some((
        legend,
        margin[0] + height + margin[2],
        margin[0] + height / 2.,
    ))
}

pub(crate) fn native_size(kind: ControlKind) -> [f32; 2] {
    let [width, height] = renderer::control_size(kind);
    match kind {
        ControlKind::Slider => [129., height],
        ControlKind::Text | ControlKind::Search => [160., height],
        _ => [width, height],
    }
}
