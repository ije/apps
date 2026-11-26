use crate::css::{
    properties::Prop, value::anchor, value::calc_size, value::color::color, value::corner_shape,
    value::easing::Easing, value::font::ordered, value::resolved::percent_sum,
    value::resolved::percent_text, value::shape, value::shape::BOXES, value::shape::Position,
    value::shape::Shape, value::shape::Shaped, value::shape_outside,
    value::shorthand::BORDER_IMAGE, value::shorthand::BORDER_IMAGE_INITIAL,
    value::shorthand::expand, value::tokens::components, value::tokens::number, value::units::Kind,
    value::units::Lengths, value::with_size,
};
pub(super) fn grammar(name: &str) -> Option<&'static str> {
    match name {
        "-webkit-text-decoration" => Some("*"),
        _ => Prop::find(name)?.grammar(),
    }
}

pub(crate) fn initial(name: &str) -> Option<&'static str> {
    INITIAL
        .binary_search_by(|(property, _)| (*property).cmp(name))
        .ok()
        .map(|index| INITIAL[index].1)
}

pub(crate) fn supports(name: &str, value: &str) -> Option<bool> {
    let name = Prop::unprefixed(name).unwrap_or(name);
    let value = value.trim();
    if name.starts_with("--") {
        return Some(true);
    }
    // Grid lines and counters take `<integer>`: a number token with `.` or an exponent is not one.
    if (name.starts_with("grid-row") || name.starts_with("grid-column") || name == "grid-area")
        && !name.ends_with("-gap")
        || name.starts_with("counter-")
    {
        let number = |token: &str| {
            token.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '+' | '-' | '.'))
                && token.parse::<f64>().is_ok()
                && token.contains(['.', 'e', 'E'])
        };
        if value.split(['/', ' ']).any(number) {
            return Some(false);
        }
    }
    // Like Chrome, a font weight is in [1, 1000]. Math clamps to it.
    if name == "font-weight"
        && value
            .parse::<f64>()
            .is_ok_and(|weight| !(1. ..=1000.).contains(&weight))
    {
        return Some(false);
    }
    if matches!(name, "orphans" | "widows") && value.parse::<f64>().is_ok_and(|lines| lines < 1.) {
        return Some(false);
    }
    // `<position>` has no three-value form, unlike `<bg-position>`.
    if matches!(name, "object-position" | "perspective-origin") && components(value, ' ').len() == 3
    {
        return Some(false);
    }
    let lower = value.to_ascii_lowercase();
    // What these take is checked by their parsers, unless it is a keyword or comes later.
    let shaped = matches!(name, "clip-path" | "offset-path");
    let own = shaped
        || matches!(name, "mask" | "mask-repeat")
        || name.starts_with("offset") && name != "offset-distance";
    let ordered = ordered(name, value);
    if (own || ordered.is_some())
        && !["var(", "env(", "attr("]
            .iter()
            .any(|function| lower.contains(function))
        && !matches!(
            lower.as_str(),
            "inherit" | "initial" | "unset" | "revert" | "revert-layer"
        )
    {
        if let Some(ordered) = ordered {
            return Some(ordered.is_some() && grammar(name).is_some());
        }
        let parts = components(&lower, ' ');
        return Some(match name {
            _ if shaped && lower == "none" => true,
            _ if shaped && lower.starts_with("url(") => {
                parts.len() == 1
                    || name == "offset-path" && parts.len() == 2 && BOXES.contains(&parts[1])
            }
            "clip-path" => Shaped::parse(value)
                .is_some_and(|shaped| !matches!(shaped.shape, Some(Shape::Ray(_)))),
            // Like Chrome, a path here takes no fill rule.
            "offset-path" => Shaped::parse(value).is_some_and(|shaped| {
                shaped.reference != Some("margin-box")
                    && !(matches!(shaped.shape, Some(Shape::Path(..)))
                        && ["evenodd", "nonzero"]
                            .iter()
                            .any(|rule| lower.contains(rule)))
            }),
            "offset-anchor" | "offset-position" => {
                lower == "auto"
                    || name == "offset-position" && lower == "normal"
                    || Position::parse(&parts).is_some()
            }
            "offset-rotate" => shape::turn(&lower, &Lengths::new(16., 1., 1.)).is_some(),
            "mask-repeat" => components(&lower, ',').into_iter().all(|layer| {
                let parts = components(layer, ' ');
                let both =
                    |part: &&str| matches!(*part, "repeat" | "no-repeat" | "space" | "round");
                match parts[..] {
                    [one] => both(&one) || matches!(one, "repeat-x" | "repeat-y"),
                    [_, _] => parts.iter().all(both),
                    _ => false,
                }
            }),
            _ => {
                let mut parts = Vec::new();
                expand(name, value, &mut parts) && !parts.is_empty()
            }
        });
    }
    if let Some(index) = name
        .strip_prefix("border-image")
        .map(|longhand| {
            (
                longhand,
                BORDER_IMAGE.iter().position(|name| name[12..] == *longhand),
            )
        })
        .filter(|(longhand, index)| longhand.is_empty() || index.is_some())
        .map(|(_, index)| index)
    {
        if lower.contains("var(")
            || matches!(
                lower.as_str(),
                "inherit" | "initial" | "unset" | "revert" | "revert-layer"
            )
        {
            return Some(true);
        }
        // A longhand is valid when the shorthand takes it in its place and sets nothing else.
        let text = match index {
            Some(2) => format!("100% / {value}"),
            Some(3) => format!("100% / / {value}"),
            _ => value.to_owned(),
        };
        return Some(
            crate::css::value::shorthand::border_image(&text).is_some_and(|parts| {
                index.is_none()
                    || (0..5).all(|other| {
                        Some(other) == index
                            || (other == 1 && matches!(index, Some(2 | 3)))
                            || parts[other] == BORDER_IMAGE_INITIAL[other]
                    })
            }),
        );
    }
    if let Some(axis) = name.strip_prefix("contain-intrinsic-")
        && !matches!(
            lower.as_str(),
            "inherit" | "initial" | "unset" | "revert" | "revert-layer"
        )
        && !lower.contains("var(")
    {
        // Each axis is `none`, a length, or `auto` before one of them.
        let mut axes = 0;
        let mut tokens = lower.split_ascii_whitespace();
        while let Some(token) = tokens.next() {
            let token = match token {
                "auto" => tokens.next().unwrap_or_default(),
                token => token,
            };
            if !(token == "none" || syntax_matches("<non-negative-length>", token)) {
                return Some(false);
            }
            axes += 1;
        }
        return Some(axes == 1 || axes == 2 && axis == "size");
    }
    // The marks of the cascade are not for pages.
    if name.starts_with("-internal-") {
        return Some(false);
    }
    // The maximum sizes have no `auto`.
    if name.starts_with("max-") && lower.contains("calc-size(auto") {
        return Some(false);
    }
    // An axis, and then how strict it is.
    if name == "scroll-snap-type"
        && let parts = components(&lower, ' ')
        && (matches!(parts[..], ["mandatory" | "proximity", ..])
            || parts.len() == 2 && !matches!(parts[1], "mandatory" | "proximity"))
    {
        return Some(false);
    }
    // Like Chrome, a box and a length with a unit, each once, and no math.
    if name == "overflow-clip-margin"
        && let parts = components(&lower, ' ')
        && (parts
            .iter()
            .any(|part| part.contains(['(']) && !part.starts_with("var("))
            || parts.iter().any(|part| part.parse::<f32>().is_ok())
            || parts.len() == 2 && parts[0].ends_with("-box") == parts[1].ends_with("-box"))
    {
        return Some(false);
    }
    if matches!(
        name,
        "anchor-name"
            | "anchor-scope"
            | "position-anchor"
            | "position-area"
            | "position-try"
            | "position-try-fallbacks"
            | "position-try-order"
            | "position-visibility"
    ) {
        return Some(
            lower.contains("var(")
                || matches!(
                    lower.as_str(),
                    "inherit" | "initial" | "unset" | "revert" | "revert-layer"
                )
                || anchor::specified(name, value).is_some(),
        );
    }
    if anchor::has_function(value) && !lower.contains("var(") {
        return Some(anchor::canonical(name, value).is_some_and(|text| {
            anchor::replace(&text, &mut |_| Some("1px".into()))
                .is_some_and(|text| supports(name, &text) == Some(true))
        }));
    }
    let grammar = grammar(name)?;
    // Like Chrome, a border, outline or column rule takes no negative width.
    if (matches!(name, "column-rule" | "outline" | "border")
        || name.starts_with("border-")
            && !name.contains("radius")
            && !name.contains("image")
            && !name.contains("spacing"))
        && components(&lower, ' ').iter().any(|part| {
            part.strip_prefix('-')
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit() || c == '.'))
        })
    {
        return Some(false);
    }
    if !lower.contains("var(") {
        // Like Chrome, `fr` can't be in math.
        let bytes = lower.as_bytes();
        if lower.contains("fr")
            && lower.contains('(')
            && ["calc(", "min(", "max(", "clamp("].iter().any(|function| {
                lower.match_indices(function).any(|(at, _)| {
                    if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'-') {
                        return false;
                    }
                    let mut depth = 0;
                    let end = bytes[at..]
                        .iter()
                        .position(|&byte| {
                            depth += i32::from(byte == b'(') - i32::from(byte == b')');
                            byte == b')' && depth == 0
                        })
                        .map_or(lower.len(), |end| at + end);
                    lower[at..end].match_indices("fr").any(|(fr, _)| {
                        let fr = at + fr;
                        bytes[fr - 1].is_ascii_digit()
                            && !bytes.get(fr + 2).is_some_and(u8::is_ascii_alphanumeric)
                    })
                })
            })
        {
            return Some(false);
        }
        // Like Chrome, each argument of a transform function has its own type.
        if name == "transform" {
            let units = Lengths::new(16., 100., 100.);
            for part in components(&lower, ' ') {
                let Some((function, args)) =
                    part.strip_suffix(')').and_then(|part| part.split_once('('))
                else {
                    continue;
                };
                for (index, arg) in components(args, ',').into_iter().enumerate() {
                    let arg = arg.trim();
                    let Some((amount, kind)) = units.math(arg, 100., &[]) else {
                        continue;
                    };
                    let zero = kind == Kind::Number && amount == 0.;
                    let valid = match function {
                        "rotate3d" if index < 3 => kind == Kind::Number,
                        "matrix" | "matrix3d" => kind == Kind::Number,
                        _ if function.starts_with("scale") => {
                            kind == Kind::Number || arg.ends_with('%')
                        }
                        _ if function.starts_with("rotate") || function.starts_with("skew") => {
                            kind == Kind::Angle || zero
                        }
                        _ if function.starts_with("translate") || function == "perspective" => {
                            kind == Kind::Length || zero
                        }
                        _ => true,
                    };
                    if !valid {
                        return Some(false);
                    }
                }
            }
        }
    }
    Some(
        !value.is_empty()
            // Only these draw it.
            && (!lower.contains("cross-fade(") || matches!(name, "background" | "background-image"))
            // Like Chrome, only outlines take this color in author styles.
            && (name.starts_with("outline") || !lower.contains("-webkit-focus-ring-color"))
            && (lower.contains("var(")
                || matches!(
                    lower.as_str(),
                    "inherit" | "initial" | "unset" | "revert" | "revert-layer"
                )
                || syntax_matches(grammar, &lower)),
    )
}

// Readers parse `<integer>` and `<number>` values as plain numbers, so math in them folds when it is stored.
pub(crate) fn fold(name: &str, value: &str) -> Option<String> {
    if name.starts_with("position-") || name.starts_with("anchor-") {
        return anchor::specified(name, value);
    }
    if let Some(ordered) = ordered(name, value) {
        return ordered.filter(|_| !value.contains(['"', '\'']));
    }
    if !value.ends_with(')') {
        return None;
    }
    let grammar = grammar(name)?;
    // Grid lines take `<integer>` among names and `span`, like Tailwind's `calc(2 * -1)`.
    let integer = grammar.contains("integer>")
        || name.starts_with("grid-") && (name.ends_with("-start") || name.ends_with("-end"));
    if !integer && !grammar.contains("number>") {
        return None;
    }
    // Like Chrome, a line height keeps the f32 range. Layout clamps it.
    let units = Lengths::new(16., 100., 100.);
    let (mut value, kind) = match name {
        "line-height" => units
            .math_unclamped(value, f32::NAN, &[])
            .map(|(value, kind)| (value.min(f64::from(f32::MAX)), kind))?,
        _ => units.math(value, f32::NAN, &[])?,
    };
    if grammar.contains("<non-negative-") {
        value = value.max(0.);
    }
    if name == "font-weight" {
        value = value.clamp(1., 1000.);
    }
    (kind == Kind::Number).then(|| {
        if integer {
            (value.round() as i64).to_string()
        } else {
            (value as f32 + 0.).to_string()
        }
    })
}

// Keep sorted.
const INITIAL: &[(&str, &str)] = &[
    ("-macos-glass", "none"),
    ("-macos-glass-interactive", "false"),
    ("-macos-glass-tint-color", "transparent"),
    ("-terminal-black", "auto"),
    ("-terminal-blue", "auto"),
    ("-terminal-bright-black", "auto"),
    ("-terminal-bright-blue", "auto"),
    ("-terminal-bright-cyan", "auto"),
    ("-terminal-bright-green", "auto"),
    ("-terminal-bright-magenta", "auto"),
    ("-terminal-bright-red", "auto"),
    ("-terminal-bright-white", "auto"),
    ("-terminal-bright-yellow", "auto"),
    ("-terminal-cyan", "auto"),
    ("-terminal-green", "auto"),
    ("-terminal-magenta", "auto"),
    ("-terminal-red", "auto"),
    ("-terminal-white", "auto"),
    ("-terminal-yellow", "auto"),
    ("-webkit-text-fill-color", "currentcolor"),
    ("-webkit-text-stroke-color", "currentcolor"),
    ("-webkit-text-stroke-width", "0px"),
    ("accent-color", "auto"),
    ("align-content", "normal"),
    ("align-items", "normal"),
    ("align-self", "auto"),
    ("anchor-name", "none"),
    ("anchor-scope", "none"),
    ("animation-delay", "0s"),
    ("animation-direction", "normal"),
    ("animation-duration", "0s"),
    ("animation-fill-mode", "none"),
    ("animation-iteration-count", "1"),
    ("animation-name", "none"),
    ("animation-play-state", "running"),
    ("animation-timing-function", "ease"),
    ("app-region", "none"),
    ("appearance", "none"),
    ("aspect-ratio", "auto"),
    ("backdrop-filter", "none"),
    ("backface-visibility", "visible"),
    ("background-attachment", "scroll"),
    ("background-blend-mode", "normal"),
    ("background-clip", "border-box"),
    ("background-color", "transparent"),
    ("background-image", "none"),
    ("background-origin", "padding-box"),
    ("background-position", "0% 0%"),
    ("background-repeat", "repeat"),
    ("background-size", "auto"),
    ("border-bottom-color", "currentcolor"),
    ("border-bottom-left-radius", "0px"),
    ("border-bottom-right-radius", "0px"),
    ("border-bottom-style", "none"),
    ("border-bottom-width", "3px"),
    ("border-collapse", "separate"),
    ("border-image-outset", "0"),
    ("border-image-repeat", "stretch"),
    ("border-image-slice", "100%"),
    ("border-image-source", "none"),
    ("border-image-width", "1"),
    ("border-left-color", "currentcolor"),
    ("border-left-style", "none"),
    ("border-left-width", "3px"),
    ("border-right-color", "currentcolor"),
    ("border-right-style", "none"),
    ("border-right-width", "3px"),
    ("border-spacing", "0px 0px"),
    ("border-top-color", "currentcolor"),
    ("border-top-left-radius", "0px"),
    ("border-top-right-radius", "0px"),
    ("border-top-style", "none"),
    ("border-top-width", "3px"),
    ("bottom", "auto"),
    ("box-shadow", "none"),
    ("box-sizing", "content-box"),
    ("break-after", "auto"),
    ("break-before", "auto"),
    ("break-inside", "auto"),
    ("caption-side", "top"),
    ("caret-animation", "auto"),
    ("caret-color", "auto"),
    ("caret-shape", "auto"),
    ("clear", "none"),
    ("clip-path", "none"),
    ("color", "canvastext"),
    ("color-scheme", "normal"),
    ("column-count", "auto"),
    ("column-gap", "normal"),
    ("column-rule-color", "currentcolor"),
    ("column-rule-style", "none"),
    ("column-rule-width", "3px"),
    ("column-width", "auto"),
    ("contain", "none"),
    ("contain-intrinsic-height", "none"),
    ("contain-intrinsic-width", "none"),
    ("container-name", "none"),
    ("container-type", "normal"),
    ("content", "normal"),
    ("content-visibility", "visible"),
    ("corner-bottom-left-shape", "round"),
    ("corner-bottom-right-shape", "round"),
    ("corner-top-left-shape", "round"),
    ("corner-top-right-shape", "round"),
    ("counter-increment", "none"),
    ("counter-reset", "none"),
    ("cursor", "auto"),
    ("cx", "0px"),
    ("cy", "0px"),
    ("direction", "ltr"),
    ("display", "inline"),
    ("empty-cells", "show"),
    ("field-sizing", "fixed"),
    ("fill", "black"),
    ("fill-opacity", "1"),
    ("filter", "none"),
    ("flex-basis", "auto"),
    ("flex-direction", "row"),
    ("flex-grow", "0"),
    ("flex-shrink", "1"),
    ("flex-wrap", "nowrap"),
    ("float", "none"),
    ("font-family", "system-ui"),
    ("font-size", "16px"),
    ("font-size-adjust", "none"),
    ("font-stretch", "100%"),
    ("font-style", "normal"),
    ("font-variant-alternates", "normal"),
    ("font-variant-caps", "normal"),
    ("font-variant-east-asian", "normal"),
    ("font-variant-ligatures", "normal"),
    ("font-variant-numeric", "normal"),
    ("font-variant-position", "normal"),
    ("font-weight", "400"),
    ("grid-auto-columns", "auto"),
    ("grid-auto-flow", "row"),
    ("grid-auto-rows", "auto"),
    ("grid-column-end", "auto"),
    ("grid-column-start", "auto"),
    ("grid-row-end", "auto"),
    ("grid-row-start", "auto"),
    ("grid-template-areas", "none"),
    ("grid-template-columns", "none"),
    ("grid-template-rows", "none"),
    ("height", "auto"),
    ("hyphenate-character", "auto"),
    ("hyphens", "manual"),
    ("image-rendering", "auto"),
    ("interpolate-size", "numeric-only"),
    ("isolation", "auto"),
    ("justify-content", "normal"),
    ("justify-items", "normal"),
    ("justify-self", "auto"),
    ("left", "auto"),
    ("letter-spacing", "normal"),
    ("line-height", "normal"),
    ("list-style-image", "none"),
    ("list-style-position", "outside"),
    ("list-style-type", "disc"),
    ("margin-bottom", "0px"),
    ("margin-left", "0px"),
    ("margin-right", "0px"),
    ("margin-top", "0px"),
    ("margin-trim", "none"),
    ("mask-clip", "border-box"),
    ("mask-composite", "add"),
    ("mask-image", "none"),
    ("mask-mode", "match-source"),
    ("mask-origin", "border-box"),
    ("mask-position", "0% 0%"),
    ("mask-repeat", "repeat"),
    ("mask-size", "auto"),
    ("mask-type", "luminance"),
    ("max-height", "none"),
    ("max-width", "none"),
    ("min-height", "auto"),
    ("min-width", "auto"),
    ("mix-blend-mode", "normal"),
    ("object-fit", "fill"),
    ("object-position", "50% 50%"),
    ("offset-anchor", "auto"),
    ("offset-distance", "0px"),
    ("offset-path", "none"),
    ("offset-position", "normal"),
    ("offset-rotate", "auto"),
    ("opacity", "1"),
    ("order", "0"),
    ("orphans", "2"),
    ("outline-color", "currentcolor"),
    ("outline-offset", "0px"),
    ("outline-style", "none"),
    ("outline-width", "3px"),
    ("overflow-clip-margin", "0px"),
    ("overflow-wrap", "normal"),
    ("overflow-x", "visible"),
    ("overflow-y", "visible"),
    ("overlay", "none"),
    ("overscroll-behavior-x", "auto"),
    ("overscroll-behavior-y", "auto"),
    ("padding-bottom", "0px"),
    ("padding-left", "0px"),
    ("padding-right", "0px"),
    ("padding-top", "0px"),
    ("page-break-after", "auto"),
    ("page-break-before", "auto"),
    ("page-break-inside", "auto"),
    ("perspective", "none"),
    ("perspective-origin", "50% 50%"),
    ("pointer-events", "auto"),
    ("position", "static"),
    ("position-anchor", "normal"),
    ("position-area", "none"),
    ("position-try-fallbacks", "none"),
    ("position-try-order", "normal"),
    ("position-visibility", "anchor-visible"),
    ("quotes", "auto"),
    ("r", "0px"),
    ("resize", "none"),
    ("right", "auto"),
    ("rotate", "none"),
    ("row-gap", "normal"),
    ("rx", "auto"),
    ("ry", "auto"),
    ("scale", "none"),
    ("scroll-behavior", "auto"),
    ("scroll-margin-bottom", "0px"),
    ("scroll-margin-left", "0px"),
    ("scroll-margin-right", "0px"),
    ("scroll-margin-top", "0px"),
    ("scroll-padding-bottom", "auto"),
    ("scroll-padding-left", "auto"),
    ("scroll-padding-right", "auto"),
    ("scroll-padding-top", "auto"),
    ("scroll-snap-align", "none"),
    ("scroll-snap-stop", "normal"),
    ("scroll-snap-type", "none"),
    ("scrollbar-color", "auto"),
    ("scrollbar-gutter", "auto"),
    ("scrollbar-width", "auto"),
    ("shape-margin", "0px"),
    ("shape-outside", "none"),
    ("stroke", "none"),
    ("stroke-dasharray", "none"),
    ("stroke-dashoffset", "0px"),
    ("stroke-width", "1px"),
    ("tab-size", "8"),
    ("table-layout", "auto"),
    ("text-align", "start"),
    ("text-box", "normal"),
    ("text-box-edge", "auto"),
    ("text-box-trim", "none"),
    ("text-decoration-color", "currentcolor"),
    ("text-decoration-line", "none"),
    ("text-decoration-skip-ink", "auto"),
    ("text-decoration-style", "solid"),
    ("text-decoration-thickness", "auto"),
    ("text-emphasis-color", "currentcolor"),
    ("text-emphasis-position", "over"),
    ("text-emphasis-style", "none"),
    ("text-indent", "0px"),
    ("text-overflow", "clip"),
    ("text-rendering", "auto"),
    ("text-shadow", "none"),
    ("text-size-adjust", "auto"),
    ("text-transform", "none"),
    ("text-underline-offset", "auto"),
    ("text-underline-position", "auto"),
    ("text-wrap", "wrap"),
    ("top", "auto"),
    ("touch-action", "auto"),
    ("transform", "none"),
    ("transform-origin", "50% 50%"),
    ("transform-style", "flat"),
    ("transition-behavior", "normal"),
    ("transition-delay", "0s"),
    ("transition-duration", "0s"),
    ("transition-property", "all"),
    ("transition-timing-function", "ease"),
    ("translate", "none"),
    ("unicode-bidi", "normal"),
    ("unicode-bidi", "normal"),
    ("user-select", "auto"),
    ("vertical-align", "baseline"),
    ("view-transition-name", "none"),
    ("visibility", "visible"),
    ("white-space", "normal"),
    ("widows", "2"),
    ("width", "auto"),
    ("will-change", "auto"),
    ("word-break", "normal"),
    ("word-spacing", "0px"),
    ("writing-mode", "horizontal-tb"),
    ("x", "0px"),
    ("y", "0px"),
    ("z-index", "auto"),
    ("zoom", "1"),
];

pub(crate) const SHORTHANDS: &[(&str, &[&str])] = &[
    (
        "text-emphasis",
        &["text-emphasis-style", "text-emphasis-color"],
    ),
    (
        "-webkit-text-stroke",
        &["-webkit-text-stroke-width", "-webkit-text-stroke-color"],
    ),
    (
        "scroll-margin",
        &[
            "scroll-margin-top",
            "scroll-margin-right",
            "scroll-margin-bottom",
            "scroll-margin-left",
        ],
    ),
    (
        "scroll-padding",
        &[
            "scroll-padding-top",
            "scroll-padding-right",
            "scroll-padding-bottom",
            "scroll-padding-left",
        ],
    ),
    (
        "column-rule",
        &[
            "column-rule-width",
            "column-rule-style",
            "column-rule-color",
        ],
    ),
    (
        "corner-shape",
        &[
            "corner-top-left-shape",
            "corner-top-right-shape",
            "corner-bottom-right-shape",
            "corner-bottom-left-shape",
        ],
    ),
    (
        "corner-top-shape",
        &["corner-top-left-shape", "corner-top-right-shape"],
    ),
    (
        "corner-right-shape",
        &["corner-top-right-shape", "corner-bottom-right-shape"],
    ),
    (
        "corner-bottom-shape",
        &["corner-bottom-left-shape", "corner-bottom-right-shape"],
    ),
    (
        "corner-left-shape",
        &["corner-top-left-shape", "corner-bottom-left-shape"],
    ),
    (
        "border-color",
        &[
            "border-top-color",
            "border-right-color",
            "border-bottom-color",
            "border-left-color",
        ],
    ),
    (
        "border-style",
        &[
            "border-top-style",
            "border-right-style",
            "border-bottom-style",
            "border-left-style",
        ],
    ),
    (
        "border-width",
        &[
            "border-top-width",
            "border-right-width",
            "border-bottom-width",
            "border-left-width",
        ],
    ),
    ("inset", &["top", "right", "bottom", "left"]),
    (
        "margin",
        &["margin-top", "margin-right", "margin-bottom", "margin-left"],
    ),
    (
        "padding",
        &[
            "padding-top",
            "padding-right",
            "padding-bottom",
            "padding-left",
        ],
    ),
    ("flex", &["flex-grow", "flex-shrink", "flex-basis"]),
    ("gap", &["row-gap", "column-gap"]),
    (
        "contain-intrinsic-size",
        &["contain-intrinsic-width", "contain-intrinsic-height"],
    ),
    ("overflow", &["overflow-x", "overflow-y"]),
    (
        "overscroll-behavior",
        &["overscroll-behavior-x", "overscroll-behavior-y"],
    ),
    ("place-content", &["align-content", "justify-content"]),
    ("place-items", &["align-items", "justify-items"]),
    ("place-self", &["align-self", "justify-self"]),
    (
        "position-try",
        &["position-try-order", "position-try-fallbacks"],
    ),
    (
        "border-top",
        &["border-top-width", "border-top-style", "border-top-color"],
    ),
    (
        "border-right",
        &[
            "border-right-width",
            "border-right-style",
            "border-right-color",
        ],
    ),
    (
        "border-bottom",
        &[
            "border-bottom-width",
            "border-bottom-style",
            "border-bottom-color",
        ],
    ),
    (
        "border-left",
        &[
            "border-left-width",
            "border-left-style",
            "border-left-color",
        ],
    ),
    (
        "outline",
        &["outline-width", "outline-style", "outline-color"],
    ),
    (
        "text-decoration",
        &[
            "text-decoration-line",
            "text-decoration-style",
            "text-decoration-color",
        ],
    ),
    (
        "list-style",
        &["list-style-position", "list-style-image", "list-style-type"],
    ),
];

pub(crate) fn computed_names() -> impl Iterator<Item = &'static str> {
    INITIAL.iter().map(|(name, _)| *name)
}

pub(crate) fn syntax_matches(syntax: &str, value: &str) -> bool {
    let units = Lengths::new(16., 100., 100.);
    let syntax = syntax.trim();
    let mut rest = syntax;
    let mut alternatives = Vec::new();
    while !rest.is_empty() {
        let end = if rest.starts_with('[') {
            rest.find(']').map_or(rest.len(), |end| end + 1)
        } else {
            rest.find('|').unwrap_or(rest.len())
        };
        let end = end
            + match rest.as_bytes()[end..].first() {
                Some(b'+' | b'#') => 1,
                Some(b'{') => rest[end..].find('}').map_or(0, |close| close + 1),
                _ => 0,
            };
        alternatives.push(rest[..end].trim());
        rest = rest[end..]
            .trim_start()
            .trim_start_matches('|')
            .trim_start();
    }
    alternatives.into_iter().any(|alternative| {
        if !alternative.starts_with(['<', '[', '*']) && !alternative.ends_with(['(', '+', '#', '}'])
        {
            return (value.split_ascii_whitespace())
                .map(str::to_ascii_lowercase)
                .eq(alternative.split(' ').map(str::to_owned));
        }
        let mut range = 1..=usize::MAX;
        let (kinds, separator) = match alternative.strip_suffix(['+', '#']) {
            Some(kind) => (
                kind,
                Some(if alternative.ends_with('#') { ',' } else { ' ' }),
            ),
            None => match alternative
                .strip_suffix('}')
                .and_then(|rest| rest.rsplit_once('{'))
                .and_then(|(kind, count)| {
                    let (min, max) = count.split_once(',')?;
                    Some((kind, min.trim().parse().ok()?..=max.trim().parse().ok()?))
                }) {
                Some((kind, counts)) => {
                    range = counts;
                    (kind, Some(' '))
                }
                None => (alternative, None),
            },
        };
        let kinds = kinds.trim_start_matches('[').trim_end_matches(']');
        let one = |value: &str| {
            kinds.split('|').any(|kind| {
                let (kind, value) = (kind.trim(), value.trim());
                if !kind.starts_with('<') && !kind.ends_with('(') && kind != "*" {
                    return value.eq_ignore_ascii_case(kind);
                }
                let (kind, sign) = match kind {
                    "<non-negative-length>" => ("<length>", true),
                    "<non-negative-length-percentage>" => ("<length-percentage>", true),
                    "<non-negative-number>" => ("<number>", true),
                    "<non-negative-percentage>" => ("<percentage>", true),
                    kind => (kind, false),
                };
                if sign && value.starts_with('-') {
                    return false;
                }
                let function = || {
                    ["calc(", "min(", "max(", "clamp("]
                        .iter()
                        .any(|function| value.starts_with(function))
                };
                let math = || units.math(value, f32::NAN, &[]);
                // Math in an `<integer>` rounds. A number token with `.` or an exponent is not an integer.
                let number = |integer: bool| {
                    math().is_some_and(|(n, kind)| {
                        kind == Kind::Number
                            && (!integer
                                || function()
                                || n.fract() == 0. && !value.contains(['.', 'e', 'E']))
                    })
                };
                // Math with only a number, like `calc(0)`, is no length.
                let typed = || {
                    !function()
                        || units
                            .math(value, 100., &[])
                            .is_some_and(|(_, kind)| kind == Kind::Length)
                };
                match kind {
                    "<length>" => units.resolve(value, f32::NAN).is_some() && typed(),
                    "<percentage>" => {
                        value.contains('%')
                            && units
                                .math(value, 100., &[])
                                .is_some_and(|(_, kind)| kind == Kind::Length)
                            && units.math(value, 0., &[]) == Some((0., Kind::Length))
                    }
                    "<length-percentage>" => units.resolve(value, 100.).is_some() && typed(),
                    "<number>" => number(false),
                    "<integer>" => number(true),
                    "<angle>" => math().is_some_and(|(_, kind)| kind == Kind::Angle),
                    "<time>" => math().is_some_and(|(_, kind)| kind == Kind::Time),
                    "<color>" => {
                        value.eq_ignore_ascii_case("currentcolor")
                            || color(value).is_some()
                            || (value.contains("(from ") || value.contains("contrast-color("))
                                && color(
                                    &value.to_ascii_lowercase().replace("currentcolor", "black"),
                                )
                                .is_some()
                            || ["light-dark(", "color-mix("]
                                .iter()
                                .any(|function| value.starts_with(function) && value.ends_with(')'))
                            || value.starts_with("contrast-color(") && value.contains("light-dark(")
                    }
                    "<custom-ident>" => {
                        !value.is_empty()
                            && value
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                    }
                    "<image>" | "<url>" => {
                        if let Some(valid) = super::image::valid(value) {
                            return valid;
                        }
                        value.starts_with("url(")
                            || !value.starts_with("cross-fade(")
                                && value.contains("gradient(")
                                && value.split_once('(').is_some_and(|(_, inner)| {
                                    inner.trim_end_matches(')').trim() != ""
                                })
                    }
                    "<transform-function>" | "<transform-list>" => {
                        let parts = components(value, ' ');
                        if kind == "<transform-list>" && value == "none" {
                            return true;
                        }
                        (parts.len() == 1 || kind == "<transform-list>" && !parts.is_empty())
                            && value.contains('(')
                            && supports("transform", value) == Some(true)
                    }
                    "<resolution>" => super::image::resolution(value).is_some(),
                    "<string>" => value.starts_with(['"', '\'']),
                    "<size>" => syntax_matches("min-content|max-content|fit-content|stretch|<calc-size>|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content", value),
                    "<calc-size>" => calc_size(value).is_some_and(|(basis, size)| {
                        let sized = |part: &str| part.split(|c: char| !c.is_ascii_alphanumeric() && c != '-').any(|word| word.eq_ignore_ascii_case("size"));
                        let size = with_size(size, "(1px)");
                        ((basis.eq_ignore_ascii_case("any") && !sized(calc_size(value).map_or("", |(_, size)| size)))
                            || syntax_matches("auto|<length-percentage>|min-content|max-content|fit-content|stretch|<calc-size>", basis))
                            && syntax_matches("<length-percentage>", &format!("calc({size})"))
                    }),
                    // A place, which can have an overflow position before it.
                    "<place>" | "<place-self>" | "<place-side>" | "<place-anchor>" => {
                        let lower = value.to_ascii_lowercase();
                        let mut words = lower.split_ascii_whitespace().peekable();
                        words.next_if(|word| matches!(*word, "safe" | "unsafe"));
                        let places: &[&str] = match kind {
                            "<place>" => &["center", "start", "end", "flex-start", "flex-end"],
                            "<place-self>" => &["self-start", "self-end"],
                            "<place-side>" => &["left", "right"],
                            _ => &["anchor-center"],
                        };
                        words.next().is_some_and(|word| places.contains(&word))
                            && words.next().is_none()
                    }
                    "<shape-outside>" => shape_outside(value).is_some(),
                    "<easing-function>" => Easing::parse(value).is_some(),
                    "<corner-shape>" => corner_shape(value).is_some(),
                    "*" => !value.is_empty(),
                    _ if kind.ends_with('(') => {
                        value.len() > kind.len()
                            && value.as_bytes()[..kind.len()].eq_ignore_ascii_case(kind.as_bytes())
                            && value.ends_with(')')
                    }
                    _ => value.eq_ignore_ascii_case(kind),
                }
            })
        };
        match separator {
            Some(separator) => {
                let parts = components(value, separator);
                range.contains(&parts.len()) && parts.into_iter().all(one)
            }
            None => one(value),
        }
    })
}

// The type of a registered syntax that takes the value: the first that matches.
pub(crate) fn matched<'a>(syntax: &'a str, value: &str) -> &'a str {
    match syntax.contains('|') {
        true => (syntax.split('|'))
            .find(|kind| syntax_matches(kind, value))
            .unwrap_or(syntax),
        false => syntax,
    }
}

// The computed value of a registered custom property, with the lengths of an element when they are known. Without
// them, relative lengths like `2em` stay as written, and false means that the value depends on the element.
pub(crate) fn computed(syntax: &str, value: &str, element: Option<&Lengths>) -> (String, bool) {
    let syntax = matched(syntax, value);
    let mut unknown = Lengths::new(f32::NAN, f32::NAN, f32::NAN);
    unknown.root = unknown.font;
    let units = element.unwrap_or(&unknown);
    let has = |kind: &str| syntax.contains(kind);
    let number = number;
    let mut independent = true;
    let (separator, joint) = if has("#") { (',', ", ") } else { (' ', " ") };
    let mut one = |part: &str| match units.math(part, f32::NAN, &[]) {
        Some((n, Kind::Length)) if has("<length") || has("<transform") => {
            format!("{}px", number(n))
        }
        Some((n, Kind::Number)) if has("<number>") || has("<transform") => number(n),
        Some((n, Kind::Number)) if has("<integer>") => number(n.round()),
        Some((n, Kind::Number)) if n == 0. && has("<length") => "0px".into(),
        Some((n, Kind::Angle)) if has("<angle>") || has("<transform") => {
            format!("{}deg", number(n))
        }
        Some((n, Kind::Time)) if has("<time>") => format!("{}s", number(n)),
        Some(_) => part.to_owned(),
        None => match part.strip_suffix('%').and_then(|n| n.parse::<f64>().ok()) {
            Some(percent) => format!("{}%", number(percent)),
            None => match percent_sum(part, units) {
                Some(sum) => percent_text(sum),
                None => {
                    independent &= units.math(part, 100., &[]).is_some()
                        || Lengths::new(16., 100., 100.)
                            .math(part, 100., &[])
                            .is_none();
                    part.to_owned()
                }
            },
        },
    };
    let parts: Vec<String> = components(value, separator)
        .into_iter()
        .map(|part| {
            let part = part.trim();
            // System colors depend on the color scheme of the element.
            if has("<color>")
                && super::color::system(part, false).is_none()
                && let Some(color) = super::color::serialize(part)
            {
                return color;
            }
            if has("<resolution>")
                && let Some(resolution) = super::image::resolution(part)
            {
                return format!("{}dppx", number(resolution));
            }
            if has("<transform")
                && let Some((name, args)) =
                    part.strip_suffix(')').and_then(|part| part.split_once('('))
            {
                let args: Vec<String> = components(args, ',').into_iter().map(&mut one).collect();
                return format!("{name}({})", args.join(", "));
            }
            one(part)
        })
        .collect();
    (parts.join(joint), independent)
}
