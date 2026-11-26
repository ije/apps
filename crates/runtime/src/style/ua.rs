use crate::css::{properties::Prop, sheet::Stylesheet};
use std::rc::Rc;

// The user agent style sheet, which build.rs packs.
pub(crate) fn ua() -> &'static str {
    packed!("ua.css")
}

pub(crate) const FULLSCREEN_UA: &str = "
:not(:root):fullscreen{position:fixed!important;inset:0!important;margin:0!important;box-sizing:border-box!important;min-width:0!important;max-width:none!important;min-height:0!important;max-height:none!important;width:100%!important;height:100%!important;transform:none!important;object-fit:contain}
iframe:fullscreen{border:none!important;padding:0!important}
:not(:root):fullscreen::backdrop{background:black}
";

thread_local! {
    pub(crate) static UA_SHEET: Rc<Stylesheet> = Rc::new(Stylesheet::parse(ua()));
    pub(crate) static FULLSCREEN_SHEET: Rc<Stylesheet> = Rc::new(Stylesheet::parse(&format!("{}{FULLSCREEN_UA}", ua())));
    #[cfg(target_os = "macos")]
    pub(crate) static GLASS_SHEET: Rc<Stylesheet> = Rc::new(Stylesheet::parse(include_str!("tailwind/liquid-glass.css")));
}

pub(crate) const SVG: [Prop; 43] = [
    Prop::CLIP_PATH,
    Prop::CLIP_RULE,
    Prop::COLOR,
    Prop::DISPLAY,
    Prop::FILL,
    Prop::FILL_OPACITY,
    Prop::FILL_RULE,
    Prop::FILTER,
    Prop::FLOOD_COLOR,
    Prop::FLOOD_OPACITY,
    Prop::FONT_FAMILY,
    Prop::FONT_SIZE,
    Prop::FONT_STYLE,
    Prop::FONT_WEIGHT,
    Prop::IMAGE_RENDERING,
    Prop::ISOLATION,
    Prop::LETTER_SPACING,
    Prop::LIGHTING_COLOR,
    Prop::MASK,
    Prop::MASK_TYPE,
    Prop::MIX_BLEND_MODE,
    Prop::OPACITY,
    Prop::PAINT_ORDER,
    Prop::SHAPE_RENDERING,
    Prop::STOP_COLOR,
    Prop::STOP_OPACITY,
    Prop::STROKE,
    Prop::STROKE_DASHARRAY,
    Prop::STROKE_DASHOFFSET,
    Prop::STROKE_LINECAP,
    Prop::STROKE_LINEJOIN,
    Prop::STROKE_MITERLIMIT,
    Prop::STROKE_OPACITY,
    Prop::STROKE_WIDTH,
    Prop::TEXT_ANCHOR,
    Prop::TEXT_DECORATION,
    Prop::TEXT_RENDERING,
    Prop::TRANSFORM,
    Prop::TRANSFORM_ORIGIN,
    Prop::VECTOR_EFFECT,
    Prop::VISIBILITY,
    Prop::WORD_SPACING,
    Prop::FONT_STRETCH,
];

pub(crate) fn presentation(name: &str) -> Option<Prop> {
    Prop::find(name).filter(|prop| {
        SVG.contains(prop) && !matches!(*prop, Prop::TRANSFORM | Prop::TRANSFORM_ORIGIN)
    })
}

pub(crate) fn svg_transform(value: &str) -> Option<String> {
    let mut result = Vec::new();
    for part in value.split_inclusive(')') {
        let part = part.trim_matches(|c: char| c == ',' || c.is_ascii_whitespace());
        if part.is_empty() {
            continue;
        }
        let (name, args) = part.strip_suffix(')')?.split_once('(')?;
        let args = args
            .split(|c: char| c == ',' || c.is_ascii_whitespace())
            .filter(|arg| !arg.is_empty())
            .map(|arg| arg.parse::<f32>().ok())
            .collect::<Option<Vec<_>>>()?;
        result.push(match (name.trim(), args.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => format!("matrix({a}, {b}, {c}, {d}, {e}, {f})"),
            ("translate", [x]) => format!("translate({x}px)"),
            ("translate", [x, y]) => format!("translate({x}px, {y}px)"),
            ("scale", [x]) => format!("scale({x})"),
            ("scale", [x, y]) => format!("scale({x}, {y})"),
            ("rotate", [a]) => format!("rotate({a}deg)"),
            ("rotate", [a, x, y]) => {
                format!(
                    "translate({x}px, {y}px) rotate({a}deg) translate({}px, {}px)",
                    -x, -y
                )
            }
            ("skewX", [a]) => format!("skewX({a}deg)"),
            ("skewY", [a]) => format!("skewY({a}deg)"),
            _ => return None,
        });
    }
    Some(if result.is_empty() {
        "none".into()
    } else {
        result.join(" ")
    })
}
