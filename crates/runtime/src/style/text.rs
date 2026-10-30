use super::{ComputedStyles, FONT_NAMES, Lengths, Style, color, components};
use crate::dom::Document;
use std::rc::Rc;

pub(crate) fn selection_colors(style: &Style, selection: Option<&Style>) -> (Option<u32>, u32) {
    let normal = style
        .get("color")
        .and_then(|value| color(value))
        .unwrap_or((0, 255));
    let foreground = selection
        .and_then(|style| style.get("color"))
        .and_then(|value| {
            if value.eq_ignore_ascii_case("currentcolor") {
                Some(normal)
            } else {
                color(value)
            }
        });
    let background = selection
        .and_then(|style| style.get("background-color"))
        .and_then(|value| {
            if value.eq_ignore_ascii_case("currentcolor") {
                Some(foreground.unwrap_or(normal))
            } else {
                color(value)
            }
        })
        .unwrap_or(if foreground.is_some() {
            (0, 0)
        } else {
            (0x4488ff, 85)
        });
    let fill = selection
        .and_then(|style| style.get("-webkit-text-fill-color"))
        .and_then(|value| {
            if value.eq_ignore_ascii_case("currentcolor") {
                Some(foreground.unwrap_or(normal))
            } else {
                color(value)
            }
        })
        .or(foreground);
    (
        fill.map(|(color, alpha)| color << 8 | alpha as u32),
        background.0 << 8 | background.1 as u32,
    )
}

#[derive(Clone)]
pub(crate) struct InlineText {
    pub text: Rc<str>,
    pub runs: Vec<render::TextRun>,
    pub ranges: Vec<(usize, std::ops::Range<usize>)>,
    pub backgrounds: Vec<(usize, u32)>,
    pub cursors: Vec<(usize, render::CursorStyle)>,
    pub shadows: Vec<(f32, f32, f32, u32)>,
    pub first_line: Option<(render::TextRun, render::TextRun, Option<String>)>,
}

pub(crate) fn restyle(
    runs: &[render::TextRun],
    range: std::ops::Range<usize>,
    base: &render::TextRun,
    pseudo: &render::TextRun,
) -> Vec<render::TextRun> {
    let mut restyled = Vec::with_capacity(runs.len() + 2);
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let (a, b) = (range.start.clamp(start, end), range.end.clamp(start, end));
        for (from, to, inside) in [(start, a, false), (a, b, true), (b, end, false)] {
            if from >= to {
                continue;
            }
            let mut piece = run.clone();
            piece.len = to - from;
            if inside {
                if piece.color == base.color {
                    piece.color = pseudo.color;
                }
                if piece.font.family == base.font.family {
                    piece.font.family = pseudo.font.family.clone();
                }
                if piece.font.weight == base.font.weight {
                    piece.font.weight = pseudo.font.weight;
                }
                if piece.font.style == base.font.style {
                    piece.font.style = pseudo.font.style;
                }
                if piece.underline == base.underline {
                    piece.underline = pseudo.underline;
                }
                if piece.strikethrough == base.strikethrough {
                    piece.strikethrough = pseudo.strikethrough;
                }
                if piece.font_size == base.font_size {
                    piece.font_size = pseudo.font_size;
                }
                if piece.letter_spacing == base.letter_spacing {
                    piece.letter_spacing = pseudo.letter_spacing;
                }
                if piece.word_spacing == base.word_spacing {
                    piece.word_spacing = pseudo.word_spacing;
                }
                piece.background_color = piece.background_color.or(pseudo.background_color);
            }
            restyled.push(piece);
        }
        start = end;
    }
    restyled
}

pub(crate) fn text_transform(text: &mut String, range: std::ops::Range<usize>, mode: &str) {
    let Some(source) = text.get(range.clone()) else {
        return;
    };
    let mut word = true;
    let mapped: String = source
        .chars()
        .map(|c| {
            let (mut upper, mut lower) = (c.to_uppercase(), c.to_lowercase());
            let mapped = match mode {
                "uppercase" => upper.next().filter(|_| upper.len() == 0),
                "capitalize" if word => upper.next().filter(|_| upper.len() == 0),
                "lowercase" => lower.next().filter(|_| lower.len() == 0),
                _ => None,
            };
            word = !c.is_alphanumeric();
            mapped.filter(|m| m.len_utf8() == c.len_utf8()).unwrap_or(c)
        })
        .collect();
    text.replace_range(range, &mapped);
}

pub(crate) fn cursor(value: &str) -> Option<render::CursorStyle> {
    use render::CursorStyle::*;
    Some(match value {
        "default" => Arrow,
        "pointer" => PointingHand,
        "text" => IBeam,
        "vertical-text" => IBeamCursorForVerticalLayout,
        "crosshair" => Crosshair,
        "grab" | "move" | "all-scroll" => OpenHand,
        "grabbing" => ClosedHand,
        "not-allowed" | "no-drop" => OperationNotAllowed,
        "col-resize" => ResizeColumn,
        "row-resize" => ResizeRow,
        "ew-resize" => ResizeLeftRight,
        "ns-resize" => ResizeUpDown,
        "e-resize" => ResizeRight,
        "w-resize" => ResizeLeft,
        "n-resize" => ResizeUp,
        "s-resize" => ResizeDown,
        "nwse-resize" | "nw-resize" | "se-resize" => ResizeUpLeftDownRight,
        "nesw-resize" | "ne-resize" | "sw-resize" => ResizeUpRightDownLeft,
        "alias" => DragLink,
        "copy" => DragCopy,
        "context-menu" => ContextualMenu,
        _ => return Option::None,
    })
}

pub(crate) fn text_run(style: &Style, units: &Lengths, len: usize) -> render::TextRun {
    let (ink, alpha) = style
        .get("color")
        .and_then(|value| color(value))
        .unwrap_or((0, 255));
    let ink: render::Rgba = render::rgba(ink << 8 | alpha as u32);
    let lines = style.get("text-decoration-line").map_or("", String::as_str);
    let decoration = style
        .get("text-decoration-color")
        .filter(|value| !value.eq_ignore_ascii_case("currentcolor"))
        .and_then(|value| color(value))
        .map_or(ink, |(ink, alpha)| render::rgba(ink << 8 | alpha as u32));
    let length = |name: &str| {
        style
            .get(name)
            .and_then(|value| units.resolve(value, units.font.em))
            .map(render::px)
    };
    let thickness = length("text-decoration-thickness").unwrap_or(render::px(1.));
    let decoration_style = match style.get("text-decoration-style").map(String::as_str) {
        Some("double") => render::DecorationStyle::Double,
        Some("dotted") => render::DecorationStyle::Dotted,
        Some("dashed") => render::DecorationStyle::Dashed,
        Some("wavy") => render::DecorationStyle::Wavy,
        _ => render::DecorationStyle::Solid,
    };
    render::TextRun {
        len,
        font: font(style),
        color: ink,
        background_color: None,
        underline: lines.contains("underline").then(|| render::UnderlineStyle {
            thickness,
            color: Some(decoration),
            style: decoration_style,
            offset: length("text-underline-offset"),
        }),
        strikethrough: lines
            .contains("line-through")
            .then_some(render::StrikethroughStyle {
                thickness,
                color: Some(decoration),
                style: decoration_style,
            }),
        letter_spacing: spacing(style, units, "letter-spacing"),
        word_spacing: spacing(style, units, "word-spacing"),
        font_size: None,
    }
}

/// A CSS `letter-spacing` or `word-spacing` value.
pub(crate) fn spacing(style: &Style, units: &Lengths, name: &str) -> render::Pixels {
    render::px(
        style
            .get(name)
            .filter(|value| *value != "normal")
            .and_then(|value| units.resolve(value, units.font.em))
            .filter(|value| value.is_finite())
            .unwrap_or(0.),
    )
}

/// Builds one text layout for an element whose content is only text and plain inline elements.
pub(crate) fn inline_text(
    doc: &Document,
    styles: &ComputedStyles,
    id: usize,
) -> Option<InlineText> {
    let parent = &styles.values[id];
    let own = doc.nodes[id].tag == "#text";
    if !own && doc.nodes[id].children.is_empty()
        || parent.get("display").is_some_and(|value| {
            matches!(
                value.as_str(),
                "grid" | "flex" | "inline-flex" | "inline-grid"
            )
        })
    {
        return None;
    }
    let siblings = doc.nodes[id]
        .parent
        .map_or(&[][..], |parent| &doc.nodes[parent].children[..]);
    let position = siblings
        .iter()
        .position(|&sibling| sibling == id)
        .unwrap_or(0);
    // Text in a flex or grid container is an item of its own, so its edges are trimmed.
    let item = doc.nodes[id].parent.is_some_and(|parent| {
        styles.values[parent]
            .get("display")
            .is_some_and(|value| value.ends_with("flex") || value.ends_with("grid"))
    });
    let inline_sibling = |sibling: Option<&usize>| {
        own && !item
            && sibling.is_some_and(|&sibling| {
                !doc.nodes[sibling].tag.starts_with('#')
                    && doc.nodes[sibling].tag != "br"
                    && styles.values[sibling]
                        .get("display")
                        .is_none_or(|value| value.starts_with("inline"))
            })
    };
    let trailing = inline_sibling(siblings.get(position + 1));
    let mut text = String::new();
    let mut runs: Vec<render::TextRun> = Vec::new();
    let mut ranges = Vec::new();
    let mut backgrounds = Vec::new();
    let mut cursors = Vec::new();
    let mut space = !inline_sibling(
        position
            .checked_sub(1)
            .and_then(|index| siblings.get(index)),
    );
    let mut pending: Vec<_> = if own {
        vec![(id, None, None)]
    } else {
        doc.nodes[id]
            .children
            .iter()
            .rev()
            .map(|&child| (child, None, None))
            .collect()
    };
    while let Some((child, end, background)) = pending.pop() {
        if let Some(index) = end {
            let (_, range): &mut (usize, std::ops::Range<usize>) = &mut ranges[index];
            range.end = text.len();
            continue;
        }
        let node = &doc.nodes[child];
        let style = &styles.values[child];
        if node.tag == "#comment" {
            continue;
        }
        if node.tag == "br" {
            text.push('\n');
            runs.push(text_run(style, &styles.lengths[child], 1));
            space = true;
            continue;
        }
        if node.tag != "#text"
            && (style.get("display").is_some_and(|value| value != "inline")
                || matches!(
                    node.tag.as_str(),
                    "img"
                        | "svg"
                        | "canvas"
                        | "input"
                        | "button"
                        | "select"
                        | "textarea"
                        | "iframe"
                        | "video"
                        | "audio"
                        | "object"
                        | "embed"
                        | "progress"
                        | "meter"
                ))
            || styles.pseudos.contains_key(&child)
            || styles.animations.contains_key(&child)
            // Runs carry their own font size; the paragraph's line height must hold the child's.
            || styles.lengths[child].font.lh > styles.lengths[id].font.lh + 0.01
            || ["white-space", "visibility", "pointer-events", "text-shadow"]
                .iter()
                .any(|key| style.get(*key) != parent.get(*key))
            || style.iter().any(|(key, value)| {
                !key.starts_with("--")
                    && parent.get(key) != Some(value)
                    && !matches!(
                        value.as_str(),
                        "" | "0"
                            | "0px"
                            | "none"
                            | "normal"
                            | "auto"
                            | "inline"
                            | "baseline"
                            | "static"
                            | "visible"
                            | "border-box"
                            | "content-box"
                            | "transparent"
                            | "currentcolor"
                            | "solid"
                    )
                    && !matches!(
                        key.as_str(),
                        "color"
                            | "font-family"
                            | "font-weight"
                            | "font-style"
                            | "text-decoration-line"
                            | "text-decoration-color"
                            | "text-decoration-style"
                            | "text-decoration-thickness"
                            | "text-underline-offset"
                            | "text-transform"
                            | "color-scheme"
                            | "caret-color"
                            | "cursor"
                            | "fill"
                            | "text-align"
                            | "background-color"
                            | "border-top-left-radius"
                            | "border-top-right-radius"
                            | "border-bottom-right-radius"
                            | "border-bottom-left-radius"
                            | "box-decoration-break"
                            | "user-select"
                            | "-webkit-user-select"
                            | "font-size"
                            | "line-height"
                            | "letter-spacing"
                            | "word-spacing"
                            | "transition-property"
                            | "transition-duration"
                            | "transition-timing-function"
                            | "transition-delay"
                            | "transition-behavior"
                            | "will-change"
                            | "font-feature-settings"
                            | "font-variation-settings"
                            | "-webkit-font-smoothing"
                            | "-moz-osx-font-smoothing"
                            | "outline-color"
                            | "outline-style"
                            | "outline-width"
                            | "outline-offset"
                    )
            })
        {
            return None;
        }
        let index = ranges.len();
        ranges.push((child, text.len()..text.len()));
        if node.tag != "#text"
            && let Some(style) = style
                .get("cursor")
                .filter(|value| Some(*value) != parent.get("cursor"))
                .and_then(|value| cursor(value))
        {
            cursors.push((index, style));
        }
        let inherited_background = background;
        let background = style
            .get("background-color")
            .and_then(|value| color::color(value))
            .filter(|(_, alpha)| *alpha != 0)
            .or(background);
        if background != inherited_background
            && let Some((color, alpha)) = background
        {
            backgrounds.push((index, color << 8 | alpha as u32));
        }
        pending.push((child, Some(index), background));
        pending.extend(
            node.children
                .iter()
                .rev()
                .map(|&child| (child, None, background)),
        );
        if !node.text.is_empty() {
            let start = text.len();
            let mode = style.get("white-space").map_or("normal", String::as_str);
            let preserve = matches!(mode, "pre" | "pre-wrap" | "break-spaces");
            let transform = style.get("text-transform").map(String::as_str);
            let mut word = true;
            for c in node.text.chars() {
                if c.is_whitespace() && !preserve && !(c == '\n' && mode == "pre-line") {
                    if !space {
                        text.push(' ');
                    }
                    space = true;
                    word = true;
                    continue;
                }
                let mapped = match transform {
                    Some("uppercase") => {
                        let mut mapped = c.to_uppercase();
                        mapped.next().filter(|_| mapped.next().is_none())
                    }
                    Some("capitalize") if word => {
                        let mut mapped = c.to_uppercase();
                        mapped.next().filter(|_| mapped.next().is_none())
                    }
                    Some("lowercase") => {
                        let mut mapped = c.to_lowercase();
                        mapped.next().filter(|_| mapped.next().is_none())
                    }
                    _ => None,
                };
                let c = mapped.filter(|m| m.len_utf8() == c.len_utf8()).unwrap_or(c);
                text.push(c);
                space = c == '\n';
                word = !c.is_alphanumeric();
            }
            if text.len() > start {
                let mut run = text_run(style, &styles.lengths[child], text.len() - start);
                let size = styles.lengths[child].font.em;
                if (size - styles.lengths[id].font.em).abs() > 0.01 {
                    run.font_size = Some(render::px(size));
                }
                runs.push(run);
            }
        }
    }
    if text.contains(['\t', '\r']) {
        return None;
    }
    if text.ends_with(' ')
        && !trailing
        && !parent
            .get("white-space")
            .is_some_and(|value| matches!(value.as_str(), "pre" | "pre-wrap" | "break-spaces"))
        || text.ends_with('\n')
    {
        text.pop();
        if let Some(run) = runs.last_mut() {
            run.len -= 1;
            if run.len == 0 {
                runs.pop();
            }
        }
        for (_, range) in &mut ranges {
            range.start = range.start.min(text.len());
            range.end = range.end.min(text.len());
        }
    }
    if text.is_empty() {
        return None;
    }
    let pseudo_run = |style: &Style| {
        let units = &styles.lengths[id];
        let mut run = text_run(style, units, 0);
        // `::first-letter` and `::first-line` may change the size inside the line.
        run.font_size = style
            .get("font-size")
            .and_then(|value| units.resolve(value, units.font.em))
            .filter(|size| (size - units.font.em).abs() > 0.01)
            .map(render::px);
        run.background_color = style
            .get("background-color")
            .and_then(|value| color(value))
            .filter(|(_, alpha)| *alpha != 0)
            .map(|(ink, alpha)| render::rgba(ink << 8 | alpha as u32));
        run
    };
    let pseudos = styles
        .text_pseudos
        .get(&id)
        .filter(|_| !own && parent.get("display").is_some_and(|value| value != "inline"));
    if let Some(letter) = pseudos.and_then(|pseudos| pseudos[1].as_ref()) {
        let start = text.len() - text.trim_start().len();
        let (mut end, mut found) = (start, false);
        for (index, c) in text[start..].char_indices() {
            if c.is_whitespace() || found && c.is_alphanumeric() {
                break;
            }
            found |= c.is_alphanumeric();
            end = start + index + c.len_utf8();
        }
        if found {
            if let Some(mode) = letter
                .get("text-transform")
                .filter(|mode| Some(*mode) != parent.get("text-transform"))
            {
                text_transform(&mut text, start..end, mode);
            }
            runs = restyle(
                &runs,
                start..end,
                &text_run(parent, &styles.lengths[id], 0),
                &pseudo_run(letter),
            );
        }
    }
    let ink = parent
        .get("color")
        .and_then(|value| color(value))
        .map_or(0xff, |(ink, alpha)| ink << 8 | alpha as u32);
    let shadows = parent
        .get("text-shadow")
        .filter(|value| *value != "none")
        .map_or_else(Vec::new, |value| {
            components(value, ',')
                .into_iter()
                .rev()
                .filter_map(|shadow| {
                    let (mut ink, mut lengths) = (ink, Vec::new());
                    for part in components(shadow, ' ') {
                        match color(part) {
                            Some((color, alpha)) => ink = color << 8 | alpha as u32,
                            None if part.eq_ignore_ascii_case("currentcolor") => {}
                            None => lengths.push(styles.lengths[id].resolve(part, 0.)?),
                        }
                    }
                    match lengths[..] {
                        [x, y] => Some((x, y, 0., ink)),
                        [x, y, blur] => Some((x, y, blur.max(0.), ink)),
                        _ => None,
                    }
                })
                .collect()
        });
    Some(InlineText {
        text: text.into(),
        runs,
        ranges,
        backgrounds,
        cursors,
        shadows,
        first_line: pseudos.and_then(|pseudos| pseudos[2].as_ref()).map(|line| {
            (
                text_run(parent, &styles.lengths[id], 0),
                pseudo_run(line),
                line.get("text-transform")
                    .filter(|mode| Some(*mode) != parent.get("text-transform"))
                    .cloned(),
            )
        }),
    })
}

pub(crate) fn font(style: &Style) -> render::Font {
    let families = style.get("font-family").map_or("system-ui", String::as_str);
    let family = FONT_NAMES.with(|names| {
        let mut names = names.borrow_mut();
        if let Some(family) = names.1.get(families) {
            return family.clone();
        }
        let family: render::SharedString = components(families, ',')
            .into_iter()
            .find_map(|family| {
                let family = family.trim().trim_matches(['\'', '"']);
                let lower = family.to_ascii_lowercase();
                let generic: &[&str] = match lower.as_str() {
                    // The macOS system fonts, like WebKit: SF Mono, New York and SF Rounded.
                    "ui-monospace" if cfg!(target_os = "macos") => {
                        &[".AppleSystemUIFontMonospaced"]
                    }
                    "ui-serif" if cfg!(target_os = "macos") => &[".AppleSystemUIFontSerif"],
                    "ui-rounded" if cfg!(target_os = "macos") => &[".AppleSystemUIFontRounded"],
                    "system-ui" | "-apple-system" | "blinkmacsystemfont" | "ui-sans-serif"
                    | "sans-serif" | "ui-rounded" => &[".SystemUIFont"],
                    "monospace" | "ui-monospace" => &[
                        "Menlo",
                        "SF Mono",
                        "Consolas",
                        "DejaVu Sans Mono",
                        "Liberation Mono",
                        "Courier New",
                    ],
                    "serif" | "ui-serif" => &[
                        "Times",
                        "Times New Roman",
                        "DejaVu Serif",
                        "Liberation Serif",
                    ],
                    "cursive" => &["Apple Chancery", "Comic Sans MS"],
                    "fantasy" => &["Papyrus", "Impact"],
                    _ => {
                        return if names.0.is_empty() {
                            Some(family.to_owned())
                        } else {
                            names.0.get(&lower).cloned()
                        };
                    }
                };
                generic
                    .iter()
                    .find(|name| {
                        name.starts_with('.') || names.0.contains_key(&name.to_ascii_lowercase())
                    })
                    .or(generic.first().filter(|_| names.0.is_empty()))
                    .map(|name| name.to_string())
            })
            .unwrap_or_else(|| ".SystemUIFont".into())
            .into();
        if !names.0.is_empty() {
            names.1.insert(families.to_owned(), family.clone());
        }
        family
    });
    let mut font = render::font(family);
    let weight = match style.get("font-weight").map(String::as_str) {
        Some("bold") => 700.,
        Some(value) => value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .unwrap_or(400.),
        None => 400.,
    };
    font.weight = render::FontWeight(weight.clamp(1., 1000.));
    font.style = match style.get("font-style").map(String::as_str) {
        Some("italic") => render::FontStyle::Italic,
        Some(value) if value.starts_with("oblique") => render::FontStyle::Oblique,
        _ => render::FontStyle::Normal,
    };
    font
}
