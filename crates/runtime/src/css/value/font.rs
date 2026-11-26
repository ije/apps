use crate::css::value::{tokens::components, tokens::write_number, units::Kind, units::Lengths};

// The longhands of `font-variant` in the order of its value, with groups of keywords that exclude each other.
const VARIANTS: [(&str, &[&[&str]]); 6] = [
    (
        "ligatures",
        &[
            &["common-ligatures", "no-common-ligatures"],
            &["discretionary-ligatures", "no-discretionary-ligatures"],
            &["historical-ligatures", "no-historical-ligatures"],
            &["contextual", "no-contextual"],
        ],
    ),
    (
        "caps",
        &[&[
            "small-caps",
            "all-small-caps",
            "petite-caps",
            "all-petite-caps",
            "unicase",
            "titling-caps",
        ]],
    ),
    ("alternates", &[&["historical-forms"]]),
    (
        "numeric",
        &[
            &["lining-nums", "oldstyle-nums"],
            &["proportional-nums", "tabular-nums"],
            &["diagonal-fractions", "stacked-fractions"],
            &["ordinal"],
            &["slashed-zero"],
        ],
    ),
    (
        "east-asian",
        &[
            &[
                "jis78",
                "jis83",
                "jis90",
                "jis04",
                "simplified",
                "traditional",
            ],
            &["full-width", "proportional-width"],
            &["ruby"],
        ],
    ),
    ("position", &[&["sub", "super"]]),
];

pub(crate) const VARIANT: [&str; 6] = [
    "font-variant-ligatures",
    "font-variant-caps",
    "font-variant-alternates",
    "font-variant-numeric",
    "font-variant-east-asian",
    "font-variant-position",
];

/// The values of the longhands of `font-variant`.
#[cold]
pub(crate) fn variants(value: &str) -> Option<[String; 6]> {
    let mut found: [Vec<(usize, &str)>; 6] = Default::default();
    match value {
        "normal" => {}
        "none" => found[0].push((0, "none")),
        _ => {
            for token in value.split_ascii_whitespace() {
                let (slot, group) =
                    VARIANTS
                        .iter()
                        .enumerate()
                        .find_map(|(slot, (_, groups))| {
                            let group = groups.iter().position(|group| group.contains(&token))?;
                            Some((slot, group))
                        })?;
                if found[slot].iter().any(|&(other, _)| other == group) {
                    return None;
                }
                let place = found[slot].partition_point(|&(other, _)| other < group);
                found[slot].insert(place, (group, token));
            }
        }
    }
    Some(found.map(|tokens| {
        if tokens.is_empty() {
            return "normal".to_owned();
        }
        let tokens: Vec<_> = tokens.into_iter().map(|(_, token)| token).collect();
        tokens.join(" ")
    }))
}

/// The value of a property whose parts can come in any order, as Chrome writes it: `Some(None)` when it is invalid,
/// and `None` for other properties.
#[cold]
pub(crate) fn ordered(name: &str, value: &str) -> Option<Option<String>> {
    if !matches!(
        name,
        "hyphenate-character" | "font-size-adjust" | "text-underline-position" | "text-indent"
    ) && !name.starts_with("font-variant")
        && !name.starts_with("text-emphasis")
    {
        return None;
    }
    let value = &*value.to_ascii_lowercase();
    if let Some(slot) = VARIANT.iter().position(|longhand| *longhand == name) {
        return Some(variants(value).and_then(|mut parts| {
            (value == "normal" || (0..6).all(|other| (parts[other] == "normal") != (other == slot)))
                .then(|| std::mem::take(&mut parts[slot]))
        }));
    }
    let tokens = components(value, ' ');
    let pick = |names: &[&'static str]| {
        (names.iter().copied()).find(|name| tokens.iter().any(|token| token == name))
    };
    let count = |picked: &[Option<&str>]| picked.iter().flatten().count() == tokens.len();
    Some(match name {
        "font-variant" => variants(value).map(|parts| {
            serialize_variants(&parts.each_ref().map(String::as_str)).unwrap_or_default()
        }),
        "font-size-adjust" => size_adjust(value).map(|adjust| match adjust {
            None => "none".to_owned(),
            Some((metric, number)) => {
                let mut out = match metric {
                    "ex-height" => String::new(),
                    metric => format!("{metric} "),
                };
                write_number(number.into(), &mut out);
                out
            }
        }),
        // Strings keep their case.
        "hyphenate-character" => match tokens[..] {
            [token] if token == "auto" || token.starts_with(['"', '\'']) => Some(token.to_owned()),
            _ => None,
        },
        "text-underline-position" => match tokens[..] {
            ["auto"] => Some("auto".to_owned()),
            _ => {
                let parts = [pick(&["under"]), pick(&["left", "right"])];
                (!tokens.is_empty() && count(&parts))
                    .then(|| parts.into_iter().flatten().collect::<Vec<_>>().join(" "))
            }
        },
        "text-emphasis" => emphasis(value).map(|parts| parts.join(" ")),
        "text-emphasis-style" => emphasis_style(&tokens),
        "text-emphasis-position" => {
            let parts = [pick(&["over", "under"]), pick(&["right", "left"])];
            (parts[0].is_some() && count(&parts))
                .then(|| parts.into_iter().flatten().collect::<Vec<_>>().join(" "))
        }
        "text-indent" => {
            let keywords = [pick(&["hanging"]), pick(&["each-line"])];
            let mut lengths =
                (tokens.iter()).filter(|token| !matches!(**token, "hanging" | "each-line"));
            let length = lengths.next().filter(|length| {
                crate::css::value::grammar::syntax_matches("<length-percentage>", length)
            });
            length
                .filter(|_| {
                    lengths.next().is_none()
                        && keywords.iter().flatten().count() + 1 == tokens.len()
                })
                .map(|length| {
                    let parts = [Some(*length), keywords[0], keywords[1]];
                    parts.into_iter().flatten().collect::<Vec<_>>().join(" ")
                })
        }
        _ => return None,
    })
}

/// `text-emphasis` as its style and color, `initial` for the one it leaves out, or `None` when it is invalid.
#[cold]
pub(crate) fn emphasis(value: &str) -> Option<[String; 2]> {
    let tokens = components(value, ' ');
    let style = |token: &&str| emphasis_style(&[token]).is_some();
    let (color, styles) = match tokens.iter().position(|token| !style(token)) {
        None => (None, &tokens[..]),
        Some(0) => (Some(tokens[0]), &tokens[1..]),
        Some(at) if at + 1 == tokens.len() => (Some(tokens[at]), &tokens[..at]),
        _ => return None,
    };
    let lower = color.map(str::to_ascii_lowercase);
    if lower
        .as_ref()
        .is_some_and(|color| !crate::css::value::grammar::syntax_matches("<color>", color))
    {
        return None;
    }
    let initial = || "initial".to_owned();
    let style = match styles {
        [] if color.is_some() => initial(),
        styles => emphasis_style(styles)?,
    };
    Some([style, color.map_or_else(initial, str::to_owned)])
}

/// `text-emphasis-style` as Chrome writes it: the fill before the shape.
fn emphasis_style(tokens: &[&str]) -> Option<String> {
    let kind = |token: &str| match token.to_ascii_lowercase().as_str() {
        "filled" | "open" => Some(0),
        "dot" | "circle" | "double-circle" | "triangle" | "sesame" => Some(1),
        "none" => Some(2),
        _ if token.len() > 1 && token.starts_with(['"', '\'']) => Some(2),
        _ => None,
    };
    let text = |token: &str| match kind(token) {
        Some(2) => token.to_owned(),
        _ => token.to_ascii_lowercase(),
    };
    match *tokens {
        [one] => kind(one).map(|_| text(one)),
        [a, b] => match (kind(a)?, kind(b)?) {
            (0, 1) => Some(format!("{} {}", text(a), text(b))),
            (1, 0) => Some(format!("{} {}", text(b), text(a))),
            _ => None,
        },
        _ => None,
    }
}

/// `font-variant` from its longhands, when it can express them.
#[cold]
pub(crate) fn serialize_variants(parts: &[&str; 6]) -> Option<String> {
    if parts.iter().all(|part| *part == "normal") {
        return Some("normal".into());
    }
    if parts[0] == "none" {
        return parts[1..]
            .iter()
            .all(|part| *part == "normal")
            .then(|| "none".into());
    }
    let parts: Vec<_> = parts
        .iter()
        .copied()
        .filter(|part| *part != "normal")
        .collect();
    Some(parts.join(" "))
}

/// The metric and number of `font-size-adjust`.
pub(crate) fn size_adjust(value: &str) -> Option<Option<(&str, f32)>> {
    if value == "none" {
        return Some(None);
    }
    let (metric, number) = value.split_once(' ').unwrap_or(("ex-height", value));
    if !matches!(metric, "ex-height" | "cap-height" | "ch-width") {
        return None;
    }
    let (number, kind) = Lengths::new(16., 100., 100.).math(number.trim(), f32::NAN, &[])?;
    (kind == Kind::Number && number >= 0.).then_some(Some((metric, number as f32)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_variants_split_and_join() {
        let split =
            variants("tabular-nums all-small-caps no-contextual super lining-nums").unwrap();
        assert_eq!(
            split,
            [
                "no-contextual",
                "all-small-caps",
                "normal",
                "lining-nums tabular-nums",
                "normal",
                "super"
            ]
        );
        assert!(variants("small-caps unicase").is_none() && variants("normal none").is_none());
        assert_eq!(
            serialize_variants(&split.each_ref().map(String::as_str)).unwrap(),
            "no-contextual all-small-caps lining-nums tabular-nums super"
        );
        let value = |name, value| ordered(name, value).unwrap();
        assert_eq!(
            value("font-variant-east-asian", "ruby simplified").unwrap(),
            "simplified ruby"
        );
        assert_eq!(value("font-variant-numeric", "ruby"), None);
        assert_eq!(
            value("text-indent", "each-line 2em").unwrap(),
            "2em each-line"
        );
        assert_eq!(value("text-indent", "hanging"), None);
        assert_eq!(
            value("text-underline-position", "left under").unwrap(),
            "under left"
        );
        assert_eq!(value("text-underline-position", "auto under"), None);
        assert_eq!(value("font-size-adjust", "ex-height .50").unwrap(), "0.5");
        assert_eq!(value("font-size-adjust", "-1"), None);
        assert_eq!(ordered("color", "red"), None);
    }
}
