use super::components;

fn expand_border(key: &str, value: &str, out: &mut Vec<(String, String)>) -> bool {
    let mut parts = Vec::new();
    expand("border-top", value, &mut parts);
    out.extend(
        parts
            .into_iter()
            .map(|(name, value)| (name.replacen("border-top", key, 1), value)),
    );
    true
}

pub(super) fn expand(key: &str, value: &str, out: &mut Vec<(String, String)>) -> bool {
    let alias = match key {
        "inline-size" => "width",
        "block-size" => "height",
        "min-inline-size" => "min-width",
        "max-inline-size" => "max-width",
        "min-block-size" => "min-height",
        "max-block-size" => "max-height",
        "overflow-inline" => "overflow-x",
        "overflow-block" => "overflow-y",
        "overscroll-behavior-inline" => "overscroll-behavior-x",
        "overscroll-behavior-block" => "overscroll-behavior-y",
        "word-wrap" => "overflow-wrap",
        "grid-row-gap" => "row-gap",
        "-webkit-background-clip" => "background-clip",
        "grid-column-gap" => "column-gap",
        _ => "",
    };
    if !alias.is_empty() {
        out.push((alias.into(), value.into()));
        return true;
    }
    if let Some((property, rest)) = key
        .split_once("-inline-")
        .or_else(|| key.split_once("-block-"))
        && matches!(
            property,
            "margin" | "padding" | "inset" | "border" | "scroll-margin" | "scroll-padding"
        )
        && let Some((side, rest)) = rest
            .split_once('-')
            .map_or(Some((rest, "")), Some)
            .filter(|(side, _)| matches!(*side, "start" | "end"))
    {
        if key.contains("-inline-") && !property.starts_with("scroll") {
            if property == "border" && rest.is_empty() {
                return expand_border(key, value, out);
            }
            out.push((key.into(), value.into()));
            return true;
        }
        let side = match (key.contains("-inline-"), side) {
            (true, "start") => "left",
            (true, _) => "right",
            (false, "start") => "top",
            _ => "bottom",
        };
        let physical = match (property, rest) {
            ("inset", _) => side.to_owned(),
            (_, "") => format!("{property}-{side}"),
            _ => format!("{property}-{side}-{rest}"),
        };
        if !expand(&physical, value, out) {
            out.push((physical, value.into()));
        }
        return true;
    }
    const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];
    let (names, kind): (Vec<String>, u8) = match key {
        "margin" | "padding" | "scroll-margin" | "scroll-padding" => {
            (SIDES.map(|side| format!("{key}-{side}")).into(), 0)
        }
        "inset" => (SIDES.map(String::from).into(), 0),
        "border-width" | "border-style" | "border-color" => {
            let suffix = &key[7..];
            (
                SIDES.map(|side| format!("border-{side}-{suffix}")).into(),
                0,
            )
        }
        "border-radius" => (
            ["top-left", "top-right", "bottom-right", "bottom-left"]
                .map(|corner| format!("border-{corner}-radius"))
                .into(),
            1,
        ),
        "margin-inline"
        | "padding-inline"
        | "margin-block"
        | "padding-block"
        | "inset-inline"
        | "inset-block"
        | "border-inline-width"
        | "border-inline-style"
        | "border-inline-color"
        | "border-block-width"
        | "border-block-style"
        | "border-block-color" => {
            let (property, rest) = key
                .split_once(if key.contains("-inline") {
                    "-inline"
                } else {
                    "-block"
                })
                .unwrap_or((key, ""));
            let sides = if key.contains("-inline") {
                ["inline-start", "inline-end"]
            } else {
                ["top", "bottom"]
            };
            (
                sides
                    .map(|side| match property {
                        "inset" if side.starts_with("inline") => format!("inset-{side}"),
                        "inset" => side.to_owned(),
                        "border" => format!("border-{side}{rest}"),
                        _ => format!("{property}-{side}"),
                    })
                    .into(),
                2,
            )
        }
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left"
        | "border-inline" | "border-block" | "outline" => {
            let sides: &[&str] = match key {
                "border-top" => &["top"],
                "border-right" => &["right"],
                "border-bottom" => &["bottom"],
                "border-left" => &["left"],
                "border-inline" => &["inline-start", "inline-end"],
                "border-block" => &["top", "bottom"],
                "outline" => &[""],
                _ => &SIDES,
            };
            let mut names = Vec::new();
            for side in sides {
                for part in ["width", "style", "color"] {
                    names.push(if side.is_empty() {
                        format!("outline-{part}")
                    } else {
                        format!("border-{side}-{part}")
                    });
                }
            }
            (names, 3)
        }
        "background" => (
            ["color", "image", "position", "size", "repeat", "clip"]
                .map(|part| format!("background-{part}"))
                .into(),
            4,
        ),
        "font" => (
            [
                "font-style",
                "font-variant",
                "font-weight",
                "font-stretch",
                "font-size",
                "line-height",
                "font-family",
            ]
            .map(String::from)
            .into(),
            5,
        ),
        "flex" => (
            ["flex-grow", "flex-shrink", "flex-basis"]
                .map(String::from)
                .into(),
            6,
        ),
        "flex-flow" => (["flex-direction", "flex-wrap"].map(String::from).into(), 7),
        "place-items" | "place-content" | "place-self" => {
            let suffix = &key[6..];
            (
                [format!("align-{suffix}"), format!("justify-{suffix}")].into(),
                2,
            )
        }
        "gap" | "grid-gap" => (["row-gap", "column-gap"].map(String::from).into(), 2),
        "overflow" => (["overflow-x", "overflow-y"].map(String::from).into(), 2),
        "overscroll-behavior" => (
            ["overscroll-behavior-x", "overscroll-behavior-y"]
                .map(String::from)
                .into(),
            2,
        ),
        "text-decoration" | "-webkit-text-decoration" => (
            ["line", "style", "color", "thickness"]
                .map(|part| format!("text-decoration-{part}"))
                .into(),
            8,
        ),
        "list-style" => (
            ["list-style-type", "list-style-position", "list-style-image"]
                .map(String::from)
                .into(),
            9,
        ),
        "container" => (
            ["container-name", "container-type"]
                .map(String::from)
                .into(),
            10,
        ),
        _ => return false,
    };
    if matches!(
        value,
        "inherit" | "initial" | "unset" | "revert" | "revert-layer"
    ) {
        out.extend(names.into_iter().map(|name| (name, value.into())));
        return true;
    }
    let parts = components(value, ' ');
    let Some(&first) = parts.first() else {
        return true;
    };
    let length = |part: &str| {
        part.trim_start_matches(['-', '+'])
            .starts_with(|c: char| c.is_ascii_digit() || c == '.')
            || ["calc(", "min(", "max(", "clamp(", "var("]
                .iter()
                .any(|f| part.starts_with(f))
    };
    let mut values: Vec<String> = match kind {
        0 => {
            let side = |index: usize, fallback: usize| {
                (*parts.get(index).or(parts.get(fallback)).unwrap_or(&first)).to_owned()
            };
            vec![side(0, 0), side(1, 0), side(2, 0), side(3, 1)]
        }
        1 => {
            let corners = |half: &str| -> Option<[String; 4]> {
                let parts = components(half.trim(), ' ');
                let side = |index: usize, fallback: usize| {
                    (*parts
                        .get(index)
                        .or(parts.get(fallback))
                        .unwrap_or(&parts[0]))
                    .to_owned()
                };
                (!parts.is_empty()).then(|| [side(0, 0), side(1, 0), side(2, 0), side(3, 1)])
            };
            let halves = components(value, '/');
            let Some(x) = corners(halves.first().copied().unwrap_or_default()) else {
                return false;
            };
            let Some(y) = halves.get(1).map_or(Some(x.clone()), |half| corners(half)) else {
                return false;
            };
            x.into_iter()
                .zip(y)
                .map(|(x, y)| if x == y { x } else { format!("{x} {y}") })
                .collect()
        }
        2 => vec![first.into(), parts.get(1).unwrap_or(&first).to_string()],
        3 => {
            let (mut width, mut style, mut color) = (
                "medium".to_owned(),
                "none".to_owned(),
                "currentcolor".to_owned(),
            );
            for &part in &parts {
                if matches!(
                    part,
                    "none"
                        | "hidden"
                        | "dotted"
                        | "dashed"
                        | "solid"
                        | "double"
                        | "groove"
                        | "ridge"
                        | "inset"
                        | "outset"
                        | "auto"
                ) {
                    style = part.into();
                } else if matches!(part, "thin" | "medium" | "thick")
                    || length(part) && !part.starts_with("var(")
                {
                    width = part.into();
                } else {
                    color = part.into();
                }
            }
            width = match width.as_str() {
                "thin" => "1px".into(),
                "medium" => "3px".into(),
                "thick" => "5px".into(),
                _ => width,
            };
            names
                .chunks(3)
                .flat_map(|_| [width.clone(), style.clone(), color.clone()])
                .collect()
        }
        4 => {
            let layers = components(value, ',');
            let mut result = vec![Vec::new(); 6];
            let mut color = "transparent".to_owned();
            for (index, layer) in layers.iter().enumerate() {
                let (mut image, mut position, mut size, mut repeat, mut boxes) = (
                    "none".to_owned(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                );
                let mut after_slash = false;
                for part in components(layer, ' ') {
                    if part == "/" {
                        after_slash = true;
                    } else if let Some((before, after)) =
                        part.split_once('/').filter(|_| !part.contains('('))
                    {
                        position.push(before.to_owned());
                        size.push(after.to_owned());
                        after_slash = true;
                    } else if part.starts_with("url(")
                        || part.contains("gradient(")
                        || part.starts_with("image-set(")
                        || part == "none"
                    {
                        image = part.into();
                    } else if matches!(
                        part,
                        "repeat" | "repeat-x" | "repeat-y" | "no-repeat" | "space" | "round"
                    ) {
                        repeat.push(part);
                    } else if matches!(part, "border-box" | "padding-box" | "content-box" | "text")
                    {
                        boxes.push(part);
                    } else if matches!(part, "scroll" | "fixed" | "local") {
                        continue;
                    } else if after_slash
                        && (length(part) || matches!(part, "auto" | "cover" | "contain"))
                    {
                        size.push(part.into());
                    } else if length(part)
                        || matches!(part, "left" | "right" | "top" | "bottom" | "center")
                    {
                        position.push(part.into());
                    } else if index + 1 == layers.len() {
                        color = part.into();
                    }
                }
                let join = |values: Vec<String>, fallback: &str| {
                    if values.is_empty() {
                        fallback.to_owned()
                    } else {
                        values.join(" ")
                    }
                };
                result[1].push(image);
                result[2].push(join(position, "0% 0%"));
                result[3].push(join(size, "auto"));
                result[4].push(join(
                    repeat.into_iter().map(String::from).collect(),
                    "repeat",
                ));
                result[5].push(boxes.last().unwrap_or(&"border-box").to_string());
            }
            result[0].push(color);
            result.into_iter().map(|values| values.join(", ")).collect()
        }
        5 => {
            if matches!(
                first,
                "caption" | "icon" | "menu" | "message-box" | "small-caption" | "status-bar"
            ) {
                vec![
                    "normal".into(),
                    "normal".into(),
                    "normal".into(),
                    "normal".into(),
                    "13px".into(),
                    "normal".into(),
                    "system-ui".into(),
                ]
            } else {
                let mut values = vec!["normal".to_owned(); 6];
                let mut index = 0;
                while let Some(&part) = parts.get(index) {
                    match part {
                        "italic" => values[0] = part.into(),
                        _ if part.starts_with("oblique") => values[0] = part.into(),
                        "small-caps" => values[1] = part.into(),
                        "bold" | "bolder" | "lighter" => values[2] = part.into(),
                        _ if part.ends_with("condensed") || part.ends_with("expanded") => {
                            values[3] = part.into()
                        }
                        "normal" => {}
                        _ if part.parse::<f32>().is_ok() => values[2] = part.into(),
                        _ => break,
                    }
                    index += 1;
                }
                let Some(size) = parts.get(index) else {
                    return true;
                };
                let (size, mut line) = size
                    .split_once('/')
                    .map_or((*size, None), |(size, line)| (size, Some(line.to_owned())));
                index += 1;
                if line.as_deref() == Some("") || parts.get(index) == Some(&"/") {
                    index += usize::from(parts.get(index) == Some(&"/"));
                    line = parts.get(index).map(|part| part.to_string());
                    index += 1;
                } else if let Some(rest) = parts.get(index).and_then(|part| part.strip_prefix('/'))
                {
                    line = Some(rest.to_owned());
                    index += 1;
                }
                let family = parts.get(index..).unwrap_or_default().join(" ");
                if family.is_empty() {
                    return true;
                }
                values[4] = size.into();
                values[5] = line.unwrap_or_else(|| "normal".into());
                values.push(family);
                values
            }
        }
        6 => match parts.as_slice() {
            ["none"] => vec!["0".into(), "0".into(), "auto".into()],
            ["auto"] => vec!["1".into(), "1".into(), "auto".into()],
            [grow] if grow.parse::<f32>().is_ok() => {
                vec![grow.to_string(), "1".into(), "0%".into()]
            }
            [basis] => vec!["1".into(), "1".into(), basis.to_string()],
            [grow, shrink] if shrink.parse::<f32>().is_ok() => {
                vec![grow.to_string(), shrink.to_string(), "0%".into()]
            }
            [grow, basis] => vec![grow.to_string(), "1".into(), basis.to_string()],
            [grow, shrink, basis, ..] => {
                vec![grow.to_string(), shrink.to_string(), basis.to_string()]
            }
            [] => return true,
        },
        7 => {
            let mut values = vec!["row".to_owned(), "nowrap".to_owned()];
            for &part in &parts {
                values[usize::from(part.contains("wrap"))] = part.into();
            }
            values
        }
        8 => {
            let (mut lines, mut values) = (
                Vec::new(),
                vec![
                    String::new(),
                    "solid".into(),
                    "currentcolor".into(),
                    "auto".into(),
                ],
            );
            for &part in &parts {
                if matches!(
                    part,
                    "none" | "underline" | "overline" | "line-through" | "blink"
                ) {
                    lines.push(part);
                } else if matches!(part, "solid" | "double" | "dotted" | "dashed" | "wavy") {
                    values[1] = part.into();
                } else if length(part) || matches!(part, "auto" | "from-font") {
                    values[3] = part.into();
                } else {
                    values[2] = part.into();
                }
            }
            values[0] = if lines.is_empty() {
                "none".into()
            } else {
                lines.join(" ")
            };
            values
        }
        9 => {
            let mut values = vec!["disc".to_owned(), "outside".to_owned(), "none".to_owned()];
            for &part in &parts {
                match part {
                    "inside" | "outside" => values[1] = part.into(),
                    _ if part.starts_with("url(") => values[2] = part.into(),
                    _ => values[0] = part.into(),
                }
            }
            values
        }
        _ => {
            let (name, kind) = value
                .split_once('/')
                .map_or((value, "normal"), |(name, kind)| (name.trim(), kind.trim()));
            vec![name.into(), kind.into()]
        }
    };
    values.truncate(names.len());
    out.extend(names.into_iter().zip(values));
    true
}
