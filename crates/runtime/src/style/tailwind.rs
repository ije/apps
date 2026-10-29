use crate::style::{self, Style};

pub(crate) fn utility(class: &str, width: f32, theme: &Style) -> Option<(u32, Style)> {
    let mut class = class;
    let mut breakpoint = 0;
    while let Some((variant, rest)) = class.split_once(':') {
        let min = if let Some(value) = theme.get(&format!("--breakpoint-{variant}")) {
            style::length(value, width, 16., width, 0.)
        } else {
            match variant {
                "sm" => 640.,
                "md" => 768.,
                "lg" => 1024.,
                "xl" => 1280.,
                "2xl" => 1536.,
                _ => return None,
            }
        };
        if !min.is_finite() || min <= 0. || width < min {
            return None;
        }
        breakpoint = breakpoint.max(min.to_bits());
        class = rest;
    }
    let mut style = Style::new();
    for (prefix, namespace, property) in [
        ("text-", "--text-", "font-size"),
        ("font-", "--font-weight-", "font-weight"),
        ("font-", "--font-", "font-family"),
        ("shadow-", "--shadow-", "box-shadow"),
    ] {
        if let Some(name) = class.strip_prefix(prefix) {
            let variable = format!("{namespace}{name}");
            if theme.contains_key(&variable) {
                style.insert(property.into(), format!("var({variable})"));
                return Some((breakpoint, style));
            }
        }
    }
    if let Some(radius) = class.strip_prefix("rounded") {
        let radius = radius.strip_prefix('-').unwrap_or(radius);
        let (corners, radius) = match radius.split_once('-') {
            Some(("t", radius)) => (&["top-left", "top-right"][..], radius),
            Some(("r", radius)) => (&["top-right", "bottom-right"][..], radius),
            Some(("b", radius)) => (&["bottom-right", "bottom-left"][..], radius),
            Some(("l", radius)) => (&["top-left", "bottom-left"][..], radius),
            Some(("tl", radius)) => (&["top-left"][..], radius),
            Some(("tr", radius)) => (&["top-right"][..], radius),
            Some(("br", radius)) => (&["bottom-right"][..], radius),
            Some(("bl", radius)) => (&["bottom-left"][..], radius),
            _ => (
                &["top-left", "top-right", "bottom-right", "bottom-left"][..],
                radius,
            ),
        };
        let variable = format!("--radius-{radius}");
        let value = if theme.contains_key(&variable) {
            format!("var({variable})")
        } else {
            match radius {
                "none" => "0".into(),
                "xs" => "2px".into(),
                "" | "sm" => "4px".into(),
                "md" => "6px".into(),
                "lg" => "8px".into(),
                "xl" => "12px".into(),
                "2xl" => "16px".into(),
                "3xl" => "24px".into(),
                "4xl" => "32px".into(),
                "full" => "9999px".into(),
                _ => length(radius)?,
            }
        };
        for corner in corners {
            style.insert(format!("border-{corner}-radius"), value.clone());
        }
        return Some((breakpoint, style));
    }
    let fixed = match class {
        "flex" => Some(("display", "flex")),
        "grid" => Some(("display", "grid")),
        "block" => Some(("display", "block")),
        "hidden" => Some(("display", "none")),
        "flex-row" => Some(("flex-direction", "row")),
        "flex-col" => Some(("flex-direction", "column")),
        "flex-wrap" => Some(("flex-wrap", "wrap")),
        "flex-nowrap" => Some(("flex-wrap", "nowrap")),
        "flex-1" => Some(("flex", "1")),
        "grow" => Some(("flex-grow", "1")),
        "grow-0" => Some(("flex-grow", "0")),
        "shrink" => Some(("flex-shrink", "1")),
        "shrink-0" => Some(("flex-shrink", "0")),
        "items-start" => Some(("align-items", "flex-start")),
        "items-end" => Some(("align-items", "flex-end")),
        "items-center" => Some(("align-items", "center")),
        "items-stretch" => Some(("align-items", "stretch")),
        "justify-start" => Some(("justify-content", "flex-start")),
        "justify-end" => Some(("justify-content", "flex-end")),
        "justify-center" => Some(("justify-content", "center")),
        "justify-between" => Some(("justify-content", "space-between")),
        "justify-around" => Some(("justify-content", "space-around")),
        "text-left" => Some(("text-align", "left")),
        "text-center" => Some(("text-align", "center")),
        "text-right" => Some(("text-align", "right")),
        "text-xs" => Some(("font-size", "0.75rem")),
        "text-sm" => Some(("font-size", "0.875rem")),
        "text-base" => Some(("font-size", "1rem")),
        "text-lg" => Some(("font-size", "1.125rem")),
        "text-xl" => Some(("font-size", "1.25rem")),
        "text-2xl" => Some(("font-size", "1.5rem")),
        "text-3xl" => Some(("font-size", "1.875rem")),
        "text-4xl" => Some(("font-size", "2.25rem")),
        "text-5xl" => Some(("font-size", "3rem")),
        "text-6xl" => Some(("font-size", "3.75rem")),
        "font-thin" => Some(("font-weight", "100")),
        "font-extralight" => Some(("font-weight", "200")),
        "font-light" => Some(("font-weight", "300")),
        "font-normal" => Some(("font-weight", "400")),
        "font-medium" => Some(("font-weight", "500")),
        "font-semibold" => Some(("font-weight", "600")),
        "font-bold" => Some(("font-weight", "700")),
        "font-extrabold" => Some(("font-weight", "800")),
        "font-black" => Some(("font-weight", "900")),
        "shadow-2xs" => Some(("box-shadow", "0 1px 0 0 #0000000d")),
        "shadow-xs" => Some(("box-shadow", "0 1px 2px 0 #0000000d")),
        "shadow-sm" => Some((
            "box-shadow",
            "0 1px 3px 0 #0000001a, 0 1px 2px -1px #0000001a",
        )),
        "shadow-md" => Some((
            "box-shadow",
            "0 4px 6px -1px #0000001a, 0 2px 4px -2px #0000001a",
        )),
        "shadow-lg" => Some((
            "box-shadow",
            "0 10px 15px -3px #0000001a, 0 4px 6px -4px #0000001a",
        )),
        "shadow-xl" => Some((
            "box-shadow",
            "0 20px 25px -5px #0000001a, 0 8px 10px -6px #0000001a",
        )),
        "shadow-2xl" => Some(("box-shadow", "0 25px 50px -12px #00000040")),
        "shadow-none" => Some(("box-shadow", "none")),
        "border" => Some(("border-width", "1px")),
        "cursor-pointer" => Some(("cursor", "pointer")),
        "cursor-grab" => Some(("cursor", "grab")),
        "cursor-grabbing" => Some(("cursor", "grabbing")),
        "cursor-default" => Some(("cursor", "default")),
        _ => None,
    };
    if let Some((key, value)) = fixed {
        style.insert(key.into(), value.into());
        return Some((breakpoint, style));
    }
    if let Some(columns) = class.strip_prefix("grid-cols-") {
        let columns = columns
            .parse::<u16>()
            .ok()
            .filter(|n| (1..=128).contains(n))?;
        style.insert(
            "grid-template-columns".into(),
            format!("repeat({columns},1fr)"),
        );
        return Some((breakpoint, style));
    }
    let negative = class.starts_with('-');
    let class = class.strip_prefix('-').unwrap_or(class);
    for (prefix, properties) in [
        ("min-w-", &["min-width"][..]),
        ("max-w-", &["max-width"][..]),
        ("min-h-", &["min-height"][..]),
        ("max-h-", &["max-height"][..]),
        ("w-", &["width"][..]),
        ("h-", &["height"][..]),
        ("size-", &["width", "height"][..]),
        (
            "p-",
            &[
                "padding-top",
                "padding-right",
                "padding-bottom",
                "padding-left",
            ][..],
        ),
        ("px-", &["padding-right", "padding-left"][..]),
        ("py-", &["padding-top", "padding-bottom"][..]),
        ("pt-", &["padding-top"][..]),
        ("pr-", &["padding-right"][..]),
        ("pb-", &["padding-bottom"][..]),
        ("pl-", &["padding-left"][..]),
        (
            "m-",
            &["margin-top", "margin-right", "margin-bottom", "margin-left"][..],
        ),
        ("mx-", &["margin-right", "margin-left"][..]),
        ("my-", &["margin-top", "margin-bottom"][..]),
        ("mt-", &["margin-top"][..]),
        ("mr-", &["margin-right"][..]),
        ("mb-", &["margin-bottom"][..]),
        ("ml-", &["margin-left"][..]),
        ("gap-x-", &["column-gap"][..]),
        ("gap-y-", &["row-gap"][..]),
        ("gap-", &["row-gap", "column-gap"][..]),
    ] {
        let Some(value) = class.strip_prefix(prefix) else {
            continue;
        };
        let margin = properties[0].starts_with("margin-");
        let sizing = properties[0].ends_with("width") || properties[0].ends_with("height");
        if negative && !margin {
            return None;
        }
        let value = match value {
            "auto" if !negative && (margin || matches!(prefix, "w-" | "h-" | "size-")) => {
                "auto".into()
            }
            "full" if sizing => "100%".into(),
            "screen"
                if matches!(
                    prefix,
                    "w-" | "h-" | "min-w-" | "min-h-" | "max-w-" | "max-h-"
                ) =>
            {
                if properties[0].ends_with("width") {
                    "100vw"
                } else {
                    "100vh"
                }
                .into()
            }
            "px" => "1px".into(),
            _ if value.starts_with('[') => length(value)?,
            _ if sizing && value.contains('/') => {
                let (a, b) = value.split_once('/')?;
                let a = a.parse::<f32>().ok()?;
                let b = b.parse::<f32>().ok()?;
                let percent = a / b * 100.;
                if a < 0. || b <= 0. || !percent.is_finite() {
                    return None;
                }
                format!("{percent}%")
            }
            _ if theme.contains_key(&format!("--spacing-{value}")) => {
                format!("var(--spacing-{value})")
            }
            _ => {
                let number = value.parse::<f32>().ok()?;
                if number < 0. || !(number * 0.25).is_finite() {
                    return None;
                }
                if theme.contains_key("--spacing") {
                    format!("calc(var(--spacing) * {number})")
                } else {
                    format!("{}rem", number * 0.25)
                }
            }
        };
        let value = if negative {
            if value.contains('(') {
                format!("calc({value} * -1)")
            } else {
                format!("-{value}")
            }
        } else {
            value
        };
        for property in properties {
            style.insert((*property).into(), value.clone());
        }
        return Some((breakpoint, style));
    }
    if negative {
        return None;
    }
    for (prefix, property) in [
        ("bg-", "background-color"),
        ("text-", "color"),
        ("border-", "border-color"),
    ] {
        let Some(value) = class.strip_prefix(prefix) else {
            continue;
        };
        let color = value
            .strip_prefix('[')
            .and_then(|v| v.strip_suffix(']'))
            .unwrap_or(value);
        let variable = format!("--color-{value}");
        if theme.contains_key(&variable) {
            style.insert(property.into(), format!("var({variable})"));
        } else if matches!(color, "black" | "white" | "transparent")
            || color.starts_with('#') && style::color(color).is_some()
        {
            style.insert(property.into(), color.into());
        } else if prefix == "text-" {
            style.insert("font-size".into(), length(value)?);
        } else if prefix == "border-" {
            let width = if value.starts_with('[') {
                length(value)?
            } else {
                format!("{}px", value.parse::<u16>().ok()?)
            };
            style.insert("border-width".into(), width);
        } else {
            return None;
        }
        return Some((breakpoint, style));
    }
    None
}

fn length(value: &str) -> Option<String> {
    let value = value
        .strip_prefix('[')?
        .strip_suffix(']')?
        .replace('_', " ");
    let (number, dimensional) = style::Lengths::new(16., 800., 600.).parse(&value, 800.)?;
    ((dimensional || number == 0.) && (number >= 0. || value.contains('('))).then_some(value)
}
