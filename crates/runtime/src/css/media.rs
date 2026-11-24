use super::{Lengths, MediaQuery, Rule, Stylesheet};
pub(crate) fn supports_condition(condition: &str) -> bool {
    let evaluate = |source: String| match Stylesheet::parse(&source).0.as_slice() {
        [rule] => match **rule {
            Rule::Supports(_, supported, _) => Some(supported),
            _ => None,
        },
        _ => None,
    };
    evaluate(format!("@supports {condition}{{}}"))
        .or_else(|| evaluate(format!("@supports ({condition}){{}}")))
        .unwrap_or(false)
}

impl MediaQuery {
    pub(crate) fn list(
        queries: &[Self],
        [width, height]: [f32; 2],
        reduced_motion: bool,
        theme: u32,
    ) -> bool {
        queries.is_empty()
            || queries.iter().any(|query| {
                let lengths = Lengths::new(16., width, height);
                query.matches(width, height, reduced_motion, theme, lengths) == Some(true)
            })
    }

    pub(crate) fn matches(
        &self,
        width: f32,
        height: f32,
        reduced_motion: bool,
        theme: u32,
        lengths: Lengths,
    ) -> Option<bool> {
        let (name, op, value) = match self {
            Self::All => return Some(true),
            Self::Unknown(_) => return None,
            Self::Not(query) => {
                return query
                    .matches(width, height, reduced_motion, theme, lengths)
                    .map(|v| !v);
            }
            Self::Range(_, query) => {
                return query.matches(width, height, reduced_motion, theme, lengths);
            }
            Self::And(queries) | Self::Or(queries) => {
                let and = matches!(self, Self::And(_));
                let mut result = Some(and);
                for query in queries {
                    match query.matches(width, height, reduced_motion, theme, lengths) {
                        Some(value) if value != and => return Some(value),
                        None => result = None,
                        _ => {}
                    }
                }
                return result;
            }
            Self::Typed(medium, query) => {
                let not = medium.starts_with("not ");
                if !matches!(medium.rsplit(' ').next(), Some("all" | "screen")) {
                    return Some(not);
                }
                return query
                    .matches(width, height, reduced_motion, theme, lengths)
                    .map(|v| v != not);
            }
            Self::Feature(name, op, value) => (
                name.as_str(),
                match op.as_str() {
                    "min" => ">=",
                    "max" => "<=",
                    op => op,
                },
                value.as_str(),
            ),
        };
        let keyword = |values: &[&str], current: &str| match op {
            "" => Some(!matches!(current, "none" | "no-preference")),
            ":" if values.contains(&value) => Some(value == current),
            _ => None,
        };
        let actual = match name {
            "-macos-glass" => return op.is_empty().then(renderer::supports_glass),
            "orientation" => {
                return keyword(
                    &["portrait", "landscape"],
                    if height >= width {
                        "portrait"
                    } else {
                        "landscape"
                    },
                );
            }
            "hover" | "any-hover" => return keyword(&["none", "hover"], "hover"),
            "pointer" | "any-pointer" => return keyword(&["none", "coarse", "fine"], "fine"),
            "prefers-color-scheme" => {
                return keyword(
                    &["light", "dark"],
                    if theme & 1 != 0 { "dark" } else { "light" },
                );
            }
            "prefers-contrast" => {
                return keyword(
                    &["no-preference", "more", "less", "custom"],
                    if theme & 2 != 0 {
                        "more"
                    } else {
                        "no-preference"
                    },
                );
            }
            "inverted-colors" => {
                return keyword(
                    &["none", "inverted"],
                    if theme & 4 != 0 { "inverted" } else { "none" },
                );
            }
            "prefers-reduced-transparency" => {
                return keyword(
                    &["no-preference", "reduce"],
                    if theme & 8 != 0 {
                        "reduce"
                    } else {
                        "no-preference"
                    },
                );
            }
            "prefers-reduced-motion" => {
                return keyword(
                    &["no-preference", "reduce"],
                    if reduced_motion {
                        "reduce"
                    } else {
                        "no-preference"
                    },
                );
            }
            "forced-colors" => return keyword(&["none", "active"], "none"),
            "display-mode" => {
                return keyword(
                    &[
                        "browser",
                        "fullscreen",
                        "minimal-ui",
                        "picture-in-picture",
                        "standalone",
                        "window-controls-overlay",
                    ],
                    "standalone",
                );
            }
            "update" => return keyword(&["none", "slow", "fast"], "fast"),
            "scripting" => return keyword(&["none", "initial-only", "enabled"], "enabled"),
            "dynamic-range" | "video-dynamic-range" => {
                return keyword(&["standard", "high"], "standard");
            }
            "color-gamut" => {
                #[cfg(target_os = "macos")]
                let p3 = {
                    unsafe extern "C" {
                        fn apps_display_p3() -> bool;
                    }
                    theme & 64 != 0 || theme >> 16 != 0 && unsafe { apps_display_p3() }
                };
                #[cfg(not(target_os = "macos"))]
                let p3 = theme & 64 != 0;
                return match (op, value) {
                    (":", "srgb") => Some(true),
                    (":", "p3") => Some(p3),
                    (":", "rec2020") => Some(false),
                    ("", _) => Some(true),
                    _ => None,
                };
            }
            "width" => f64::from(width),
            "height" => f64::from(height),
            "aspect-ratio" => f64::from(width) / f64::from(height),
            "resolution" | "-webkit-device-pixel-ratio" | "device-pixel-ratio" => {
                f64::from(theme >> 16).max(100.) / 100.
            }
            "color" => 8.,
            "color-index" | "monochrome" => 0.,
            _ => return None,
        };
        let boundary = match name {
            _ if op.is_empty() => return Some(actual != 0.),
            "width" | "height" => f64::from(lengths.resolve(value, f32::NAN)?),
            "aspect-ratio" => {
                let (a, b) = value.split_once('/').unwrap_or((value, "1"));
                let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
                (a >= 0. && b > 0.).then_some(a / b)?
            }
            "-webkit-device-pixel-ratio" | "device-pixel-ratio" => value.parse().ok()?,
            "resolution" => {
                let number = value.trim_end_matches(|c: char| c.is_ascii_alphabetic());
                number.parse::<f64>().ok()?
                    / match &value[number.len()..] {
                        "dppx" | "x" => 1.,
                        "dpi" => 96.,
                        "dpcm" => 96. / 2.54,
                        _ => return None,
                    }
            }
            _ => value.parse().ok()?,
        };
        let ordering = actual.partial_cmp(&boundary)?;
        Some(match op {
            ":" | "=" => ordering.is_eq(),
            "<" => ordering.is_lt(),
            "<=" => ordering.is_le(),
            ">" => ordering.is_gt(),
            ">=" => ordering.is_ge(),
            _ => return None,
        })
    }

    pub(crate) fn source(&self) -> String {
        let group = |query: &Self| match query {
            Self::Not(_) | Self::And(_) | Self::Or(_) => format!("({})", query.source()),
            _ => query.source(),
        };
        match self {
            Self::All => "all".into(),
            Self::Unknown(text) | Self::Range(text, _) => text.clone(),
            Self::Feature(kind, name, value) if kind == "style" => match value.as_str() {
                "" => format!("style({name})"),
                _ => format!("style({name}: {value})"),
            },
            Self::Feature(name, op, value) => match op.as_str() {
                "" => format!("({name})"),
                ":" => format!("({name}: {value})"),
                "min" | "max" => format!("({op}-{name}: {value})"),
                _ => format!("({name} {op} {value})"),
            },
            Self::Typed(medium, query) => match **query {
                Self::All => medium.clone(),
                _ => format!("{medium} and {}", group(query)),
            },
            Self::Not(query) => format!("not {}", group(query)),
            Self::And(queries) | Self::Or(queries) => {
                queries.iter().map(group).collect::<Vec<_>>().join(
                    if matches!(self, Self::And(_)) {
                        " and "
                    } else {
                        " or "
                    },
                )
            }
        }
    }
}
