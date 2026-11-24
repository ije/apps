use super::{FONT_NAMES, Lengths, Style, color, components};
use crate::css::{properties::Prop, value::color::paint, value::color::rgba};
use crate::dom::Document;
use std::rc::Rc;

pub(crate) fn selection_colors(style: &Style, selection: Option<&Rc<Style>>) -> (Option<u32>, u32) {
    let normal = style.get(Prop::COLOR).and_then(color).unwrap_or((0, 255));
    let pick = |prop: Prop, current: (u32, u8)| {
        let value = selection?.get(prop)?;
        if value.eq_ignore_ascii_case("currentcolor") {
            Some(current)
        } else {
            color(value)
        }
    };
    let foreground = pick(Prop::COLOR, normal);
    let background = pick(Prop::BACKGROUND_COLOR, foreground.unwrap_or(normal)).unwrap_or(
        if foreground.is_some() {
            (0, 0)
        } else {
            (0x4488ff, 85)
        },
    );
    let fill = pick(Prop::WEBKIT_TEXT_FILL_COLOR, foreground.unwrap_or(normal)).or(foreground);
    (
        fill.map(|(color, alpha)| color << 8 | alpha as u32),
        background.0 << 8 | background.1 as u32,
    )
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum LineHeight {
    Normal,
    Scale(f32),
    Fixed(f32),
}

pub(crate) fn line_height(style: &Style) -> LineHeight {
    // Like Chrome's layout units, a line is at most 2^25 px tall.
    const LIMIT: f32 = 33_554_431.;
    match style.get(Prop::LINE_HEIGHT) {
        Some(value) if value.ends_with("px") => value
            .trim_end_matches("px")
            .parse::<f32>()
            .map_or(LineHeight::Normal, |px| LineHeight::Fixed(px.min(LIMIT))),
        Some(value) => value.parse::<f32>().map_or(LineHeight::Normal, |scale| {
            let size = style
                .get(Prop::FONT_SIZE)
                .and_then(|size| size.strip_suffix("px")?.parse::<f32>().ok())
                .unwrap_or(16.);
            LineHeight::Scale(scale.min(LIMIT / size.max(f32::MIN_POSITIVE)))
        }),
        None => LineHeight::Normal,
    }
}

pub(crate) fn restyle(
    runs: &[renderer::TextRun],
    range: std::ops::Range<usize>,
    base: &renderer::TextRun,
    pseudo: &renderer::TextRun,
) -> Vec<renderer::TextRun> {
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
            // The edges of inline boxes stay at the ends of the run.
            if from > start {
                piece.inset[0] = renderer::px(0.);
            }
            if to < end {
                piece.inset[1] = renderer::px(0.);
            }
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
                if piece.overline == base.overline {
                    piece.overline = pseudo.overline;
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
    let mut last = text[..range.start].chars().rev();
    let (mut prev, mut before) = (last.next().unwrap_or(' '), last.next().unwrap_or(' '));
    let mapped: String = source
        .chars()
        .map(|c| {
            let mapped = case(c, mode, starts_word(before, prev, c));
            (before, prev) = (prev, c);
            mapped
        })
        .collect();
    text.replace_range(range, &mapped);
}

// Like the ICU word breaks of Chrome: letters, digits and `_` join, and so does an apostrophe or a period between two letters or two digits.
pub(crate) fn starts_word(before: char, prev: char, c: char) -> bool {
    !(prev.is_alphanumeric()
        || prev == '_'
        || matches!(prev, '\'' | '\u{2019}' | '.' | ':' | '\u{b7}')
            && before.is_alphabetic()
            && c.is_alphabetic()
        || matches!(prev, '\'' | '\u{2019}' | '.' | ',' | ';')
            && before.is_numeric()
            && c.is_numeric())
}

// Apostrophes, periods, colons and combining marks, which a final sigma looks past.
pub(crate) fn case_ignorable(c: char) -> bool {
    matches!(
        c,
        '\'' | '\u{2019}' | '.' | ':' | '\u{b7}' | '\u{300}'..='\u{36f}'
    )
}

/// Pushes the mapping of `c` when it is not one char of the same UTF-8 length, and returns its last char.
pub(crate) fn push_full(
    text: &mut String,
    c: char,
    mapped: impl ExactSizeIterator<Item = char> + Clone,
) -> Option<char> {
    let first = mapped.clone().next()?;
    if mapped.len() == 1 && first.len_utf8() == c.len_utf8() {
        return None;
    }
    mapped.inspect(|&m| text.push(m)).last()
}

// Only mappings to one char of the same UTF-8 length apply, so text ranges stay valid.
pub(crate) fn case(c: char, mode: &str, word: bool) -> char {
    match mode {
        "uppercase" => {
            let mut upper = c.to_uppercase();
            upper.next().filter(|_| upper.len() == 0)
        }
        "capitalize" if word => {
            let mut upper = c.to_uppercase();
            upper.next().filter(|_| upper.len() == 0)
        }
        "lowercase" => {
            let mut lower = c.to_lowercase();
            lower.next().filter(|_| lower.len() == 0)
        }
        _ => return c,
    }
    .filter(|m| m.len_utf8() == c.len_utf8())
    .unwrap_or(c)
}

pub(crate) fn cursor(value: &str) -> Option<renderer::Cursor> {
    use renderer::Cursor::*;
    Some(match value {
        "default" => Default,
        "pointer" => Pointer,
        "text" => Text,
        "vertical-text" => VerticalText,
        "crosshair" => Crosshair,
        "grab" | "move" | "all-scroll" => Grab,
        "grabbing" => Grabbing,
        "not-allowed" | "no-drop" => NotAllowed,
        "col-resize" => ColResize,
        "row-resize" => RowResize,
        "ew-resize" => EwResize,
        "ns-resize" => NsResize,
        "e-resize" => EResize,
        "w-resize" => WResize,
        "n-resize" => NResize,
        "s-resize" => SResize,
        "nwse-resize" | "nw-resize" | "se-resize" => NwseResize,
        "nesw-resize" | "ne-resize" | "sw-resize" => NeswResize,
        "alias" => Alias,
        "copy" => Copy,
        "context-menu" => ContextMenu,
        "none" => None,
        _ => return Option::None,
    })
}

// Retain styles used as pointer keys to prevent address reuse.
pub(crate) fn styled_run(style: &Rc<Style>, units: &Lengths, len: usize) -> renderer::TextRun {
    type Runs = rustc_hash::FxHashMap<*const Style, (Rc<Style>, Vec<(Lengths, renderer::TextRun)>)>;
    thread_local!(static RUNS: std::cell::RefCell<Runs> = Default::default());
    let key = style
        .value(Prop::FONT_FAMILY)
        .map_or(std::ptr::null(), |value| value.as_ptr());
    let named = FONT_NAMES.with_borrow(|names| names.1.get(&key).map(|(_, family)| family.clone()));
    let found = named.and_then(|named| {
        RUNS.with_borrow(|runs| {
            runs.get(&Rc::as_ptr(style))?
                .1
                .iter()
                .find(|(kept, run)| kept == units && Rc::ptr_eq(&run.font.family, &named))
                .map(|(_, run)| run.clone())
        })
    });
    let mut run = found.unwrap_or_else(|| {
        let run = text_run(style, units, 0);
        RUNS.with_borrow_mut(|runs| {
            if runs.len() >= 1024 {
                runs.retain(|_, (style, _)| Rc::strong_count(style) > 1);
                if runs.len() >= 1024 {
                    runs.clear();
                }
            }
            let list = &mut runs
                .entry(Rc::as_ptr(style))
                .or_insert_with(|| (style.clone(), Vec::new()))
                .1;
            if list.len() == 4 {
                list.remove(0);
            }
            list.push((*units, run.clone()));
        });
        run
    });
    run.len = len;
    run
}

pub(crate) fn text_run(style: &Style, units: &Lengths, len: usize) -> renderer::TextRun {
    let ink = (style.get(Prop::COLOR).and_then(paint)).unwrap_or(renderer::Color::BLACK);
    let lines = style.get(Prop::TEXT_DECORATION_LINE).unwrap_or("");
    let decoration = style
        .get(Prop::TEXT_DECORATION_COLOR)
        .filter(|value| !value.eq_ignore_ascii_case("currentcolor"))
        .and_then(paint)
        .unwrap_or(ink);
    let length = |prop: Prop| {
        style
            .get(prop)
            .and_then(|value| units.resolve(value, units.font.em))
            .map(renderer::px)
    };
    // Like Chromium, `auto` and `from-font` are a tenth of the font size.
    let thickness =
        length(Prop::TEXT_DECORATION_THICKNESS).unwrap_or(renderer::px(units.font.em / 10.));
    let decoration_style = match style.get(Prop::TEXT_DECORATION_STYLE) {
        Some("double") => renderer::DecorationStyle::Double,
        Some("dotted") => renderer::DecorationStyle::Dotted,
        Some("dashed") => renderer::DecorationStyle::Dashed,
        Some("wavy") => renderer::DecorationStyle::Wavy,
        _ => renderer::DecorationStyle::Solid,
    };
    renderer::TextRun {
        len,
        font: font(style),
        color: style
            .get(Prop::WEBKIT_TEXT_FILL_COLOR)
            .and_then(color)
            .map_or(ink, rgba),
        background_color: None,
        underline: lines.contains("underline").then(|| renderer::Decoration {
            thickness,
            color: Some(decoration),
            style: decoration_style,
            offset: length(Prop::TEXT_UNDERLINE_OFFSET),
        }),
        strikethrough: lines
            .contains("line-through")
            .then_some(renderer::Decoration {
                thickness,
                color: Some(decoration),
                style: decoration_style,
                offset: None,
            }),
        overline: lines.contains("overline").then_some(renderer::Decoration {
            thickness,
            color: Some(decoration),
            style: decoration_style,
            offset: None,
        }),
        letter_spacing: spacing(style, units, Prop::LETTER_SPACING),
        word_spacing: spacing(style, units, Prop::WORD_SPACING),
        font_size: None,
        inset: Default::default(),
        nowrap: false,
        tab_size: style.get(Prop::TAB_SIZE).map_or(8., |value| {
            value
                .parse::<f32>()
                .ok()
                .filter(|count| *count >= 0.)
                .unwrap_or_else(|| -units.resolve(value, units.font.em).unwrap_or(0.).max(0.))
        }),
        stroke: style
            .get(Prop::WEBKIT_TEXT_STROKE_WIDTH)
            .and_then(|value| units.resolve(value, 0.))
            .filter(|width| *width > 0.)
            .map(|width| {
                let color = style.get(Prop::WEBKIT_TEXT_STROKE_COLOR).and_then(color);
                (renderer::px(width), color.map_or(ink, rgba))
            }),
    }
}

pub(crate) fn spacing(style: &Style, units: &Lengths, prop: Prop) -> renderer::Pixels {
    renderer::px(
        style
            .get(prop)
            .filter(|value| *value != "normal")
            .and_then(|value| units.resolve(value, units.font.em))
            .filter(|value| value.is_finite())
            .unwrap_or(0.),
    )
}

pub(crate) fn pseudo_content(doc: &Document, id: usize, pseudo: &Style, content: &str) -> String {
    let node = &doc.nodes[id];
    let mut text = String::new();
    for part in components(content, ' ') {
        if let Some(name) = part.strip_prefix("attr(").and_then(|v| v.strip_suffix(')')) {
            text.push_str(node.attrs.get(name.trim()).map_or("", String::as_str));
        } else if let Some(close) = match part {
            "open-quote" => Some(false),
            "close-quote" => Some(true),
            _ => None,
        } {
            let depth = std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
                .filter(|&parent| doc.nodes[parent].tag == "q")
                .count();
            let quotes: Vec<String> = match pseudo.get(Prop::QUOTES) {
                None | Some("auto") => ["\u{201c}", "\u{201d}", "\u{2018}", "\u{2019}"]
                    .map(str::to_owned)
                    .to_vec(),
                Some(value) => components(value, ' ')
                    .into_iter()
                    .filter_map(|part| {
                        let mut input = cssparser::Parser::new(part);
                        input.expect_string().ok().map(|string| string.to_string())
                    })
                    .collect(),
            };
            let pair = (depth * 2).min(quotes.len().saturating_sub(2));
            text.push_str(
                quotes
                    .get(pair + usize::from(close))
                    .map_or("", String::as_str),
            );
        } else if let Some(string) = part
            .strip_prefix(['"', '\''])
            .and_then(|v| v.strip_suffix(['"', '\'']))
        {
            let mut chars = string.chars().peekable();
            while let Some(c) = chars.next() {
                if c != '\\' {
                    text.push(c);
                    continue;
                }
                let mut code = String::new();
                while code.len() < 6
                    && let Some(digit) = chars.next_if(char::is_ascii_hexdigit)
                {
                    code.push(digit);
                }
                if code.is_empty() {
                    text.extend(chars.next());
                    continue;
                }
                chars.next_if_eq(&' ');
                text.push(
                    u32::from_str_radix(&code, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .unwrap_or('\u{fffd}'),
                );
            }
        }
    }
    if let Some(mode) = pseudo.get(Prop::TEXT_TRANSFORM) {
        let len = text.len();
        text_transform(&mut text, 0..len, mode);
    }
    text
}

pub(crate) fn word_break(style: &Style) -> renderer::WordBreak {
    match (style.get(Prop::WORD_BREAK), style.get(Prop::OVERFLOW_WRAP)) {
        (Some("break-all"), _) => renderer::WordBreak::BreakAll,
        (Some("keep-all"), _) => renderer::WordBreak::KeepAll,
        (Some("break-word"), _) | (_, Some("anywhere")) => renderer::WordBreak::Anywhere,
        (_, Some("break-word")) => renderer::WordBreak::BreakWord,
        _ => renderer::WordBreak::Normal,
    }
}

pub(crate) fn font(style: &Style) -> renderer::Font {
    let value = style.value(Prop::FONT_FAMILY);
    let key = value.map_or(std::ptr::null(), |value| value.as_ptr());
    let family = FONT_NAMES.with(|names| {
        let mut names = names.borrow_mut();
        if let Some((_, family)) = names.1.get(&key) {
            return family.clone();
        }
        let families = value.map_or("system-ui", |value| &**value);
        // The used family, then the other installed families of the list for missing glyphs, one per line.
        let mut list = String::new();
        for family in components(families, ',').into_iter().filter_map(|family| {
            let family = family.trim().trim_matches(['\'', '"']);
            let lower = family.to_ascii_lowercase();
            let generic: &[&str] = match lower.as_str() {
                "ui-monospace" | "ui-serif" | "ui-sans-serif" | "ui-rounded" => return None,
                "system-ui" | "-apple-system" | "blinkmacsystemfont" => &[".SystemUIFont"],
                "sans-serif" => &[
                    "Helvetica",
                    "Arial",
                    "DejaVu Sans",
                    "Liberation Sans",
                    ".SystemUIFont",
                ],
                "monospace" => &[
                    "Menlo",
                    "SF Mono",
                    "Consolas",
                    "DejaVu Sans Mono",
                    "Liberation Mono",
                    "Courier New",
                ],
                "serif" => &[
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
        }) {
            if list.split('\n').any(|kept| kept == family) {
                continue;
            }
            if !list.is_empty() {
                list.push('\n');
            }
            list += &family;
            // The system font falls back through the whole system cascade.
            if family == ".SystemUIFont" {
                break;
            }
        }
        if list.is_empty() {
            list += ".SystemUIFont";
        }
        let family: renderer::SharedString = list.into();
        if !names.0.is_empty() {
            if names.1.len() > 4096 {
                names.1.clear();
            }
            names.1.insert(key, (value.cloned(), family.clone()));
        }
        family
    });
    let mut font = renderer::Font::new(family);
    let weight = match style.get(Prop::FONT_WEIGHT) {
        Some("bold") => 700.,
        Some(value) => value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .unwrap_or(400.),
        None => 400.,
    };
    font.weight = weight.clamp(1., 1000.);
    if let Some(value) = style.get(Prop::FONT_SYNTHESIS) {
        let has = |name| value.split_ascii_whitespace().any(|word| word == name);
        font.synthesis = [has("weight"), has("style")];
    }
    font.style = match style.get(Prop::FONT_STYLE) {
        Some("italic") => renderer::FontStyle::Italic,
        Some(value) if value.starts_with("oblique") => renderer::FontStyle::Oblique,
        _ => renderer::FontStyle::Normal,
    };
    let mut features: Vec<(String, u32)> = Vec::new();
    if style.get(Prop::FONT_KERNING) == Some("none") {
        features.push(("kern".into(), 0));
    }
    let keywords = [Prop::FONT_VARIANT_LIGATURES, Prop::FONT_VARIANT_NUMERIC]
        .into_iter()
        .filter_map(|prop| style.get(prop))
        .flat_map(str::split_ascii_whitespace);
    for keyword in keywords {
        let tags: &[(&str, u32)] = match keyword {
            "none" | "no-common-ligatures" => &[("liga", 0), ("clig", 0)],
            "no-contextual" => &[("calt", 0)],
            "discretionary-ligatures" => &[("dlig", 1)],
            "lining-nums" => &[("lnum", 1)],
            "oldstyle-nums" => &[("onum", 1)],
            "proportional-nums" => &[("pnum", 1)],
            "tabular-nums" => &[("tnum", 1)],
            "diagonal-fractions" => &[("frac", 1)],
            "stacked-fractions" => &[("afrc", 1)],
            "ordinal" => &[("ordn", 1)],
            "slashed-zero" => &[("zero", 1)],
            _ => &[],
        };
        features.extend(tags.iter().map(|&(tag, value)| (tag.to_owned(), value)));
    }
    for setting in style
        .get(Prop::FONT_FEATURE_SETTINGS)
        .filter(|value| *value != "normal")
        .map(|value| components(value, ','))
        .unwrap_or_default()
    {
        let mut parts = setting.split_ascii_whitespace();
        let Some(tag) = parts
            .next()
            .map(|tag| tag.trim_matches(['"', '\'']))
            .filter(|tag| tag.len() == 4)
        else {
            continue;
        };
        let value = match parts.next() {
            None | Some("on") => 1,
            Some("off") => 0,
            Some(value) => value.parse().unwrap_or(1),
        };
        features.push((tag.to_owned(), value));
    }
    if !features.is_empty() {
        font.features = features.into();
    }
    let mut axes: Vec<([u8; 4], f32)> = Vec::new();
    for setting in style
        .get(Prop::FONT_VARIATION_SETTINGS)
        .filter(|value| *value != "normal")
        .map(|value| components(value, ','))
        .unwrap_or_default()
    {
        let mut parts = setting.split_ascii_whitespace();
        let tag = parts.next().map(|tag| tag.trim_matches(['"', '\'']));
        let (Some(Ok(tag)), Some(Ok(value))) = (
            tag.map(|tag| <[u8; 4]>::try_from(tag.as_bytes())),
            parts.next().map(str::parse::<f32>),
        ) else {
            continue;
        };
        // The last setting of an axis wins.
        axes.retain(|(name, _)| *name != tag);
        axes.push((tag, value));
    }
    if !axes.is_empty() {
        axes.sort_unstable_by_key(|(tag, _)| *tag);
        font.variations = axes.into();
    }
    if style.get(Prop::FONT_OPTICAL_SIZING) != Some("none") {
        font.optical_size = style
            .get(Prop::FONT_SIZE)
            .and_then(|size| size.strip_suffix("px")?.parse().ok());
    }
    font
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_synthesis_turns_off_synthetic_bold_and_slant() {
        let synthesis = |value: Option<&str>| {
            let style = Style::from_iter(value.map(|value| (Prop::FONT_SYNTHESIS, value.into())));
            font(&style).synthesis
        };
        assert_eq!(synthesis(None), [true, true]);
        assert_eq!(synthesis(Some("none")), [false, false]);
        assert_eq!(synthesis(Some("style")), [false, true]);
        assert_eq!(synthesis(Some("weight small-caps")), [true, false]);
    }
}
