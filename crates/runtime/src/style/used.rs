use crate::css::{Lengths, Prop, Style, components, percent_sum, percent_text};
use std::rc::Rc;
pub(crate) fn dark(style: &Style, theme: u32) -> bool {
    style
        .get(Prop::COLOR_SCHEME)
        .is_some_and(|v| v.contains("dark") && (!v.contains("light") || theme & 1 != 0))
}

// A table's border spacing is the gap of its grid and padding at its edges.
pub(crate) fn border_spacing(style: &mut Style) {
    let spacing = if style
        .get(Prop::BORDER_COLLAPSE)
        .is_some_and(|value| value == "collapse")
    {
        "0".to_owned()
    } else {
        style.get(Prop::BORDER_SPACING).unwrap_or("0").to_owned()
    };
    let parts = components(&spacing, ' ');
    if let Some(&column) = parts.first() {
        let row = parts.get(1).copied().unwrap_or(column);
        style.insert(Prop::COLUMN_GAP, column);
        style.insert(Prop::ROW_GAP, row);
        if spacing != "0" {
            for (prop, gap) in [
                (Prop::PADDING_TOP, row),
                (Prop::PADDING_RIGHT, column),
                (Prop::PADDING_BOTTOM, row),
                (Prop::PADDING_LEFT, column),
            ] {
                let value = format!("calc({} + {gap})", style.get(prop).unwrap_or("0px"));
                style.insert(prop, value);
            }
        }
    }
}

// A canvas that lays out its children as blocks, for the page to draw them with `drawElementImage()`.
pub(crate) fn drawable_canvas(node: &crate::dom::Node, style: &Style) -> bool {
    node.tag == "canvas" && style.get(Prop::CANVAS_CONTENT) == Some("drawable")
}

pub(crate) fn blockify(style: &mut Style, item: bool, table_item: bool) {
    let positioned = style
        .get(Prop::POSITION)
        .is_some_and(|value| matches!(value, "absolute" | "fixed"));
    if positioned {
        style.remove(Prop::FLOAT);
    }
    if !(item || positioned || style.get(Prop::FLOAT).is_some_and(|value| value != "none")) {
        return;
    }
    if positioned
        && style
            .get(Prop::DISPLAY)
            .is_none_or(|display| display.starts_with("inline"))
    {
        style.insert(Prop::INTERNAL_STATIC_INLINE, "1");
    }
    let display = match style.get(Prop::DISPLAY).unwrap_or("inline") {
        "inline" | "inline-block" => "block",
        "inline-flex" => "flex",
        "inline-grid" => "grid",
        "inline-table" => "table",
        "table-row" | "table-row-group" | "table-header-group" | "table-footer-group"
        | "table-cell" | "table-caption"
            if !table_item =>
        {
            "block"
        }
        _ => return,
    };
    style.insert(Prop::DISPLAY, display);
}

// Font-relative lengths in px, or None when the value has none.
pub(crate) fn pixels(value: &str, units: &Lengths) -> Option<String> {
    let relative = |text: &str| {
        let bytes = text.as_bytes();
        (1..bytes.len()).any(|at| {
            bytes[at - 1].is_ascii_digit() && {
                let unit = &text[at..];
                let unit = &unit[..unit
                    .find(|c: char| !c.is_ascii_alphabetic())
                    .unwrap_or(unit.len())];
                let unit = unit.strip_prefix(['r', 'R']).unwrap_or(unit);
                ["em", "ex", "ch", "cap", "ic", "lh"]
                    .iter()
                    .any(|name| unit.eq_ignore_ascii_case(name))
            }
        })
    };
    if !relative(value) {
        return None;
    }
    let lists: Vec<String> = components(value, ',')
        .into_iter()
        .map(|list| {
            components(list, ' ')
                .into_iter()
                .map(|token| match units.parse(token, 0.) {
                    // A percentage resolves later, so only the lengths of its sum become pixels.
                    _ if token.contains('%') => {
                        percent_sum(token, units).map_or_else(|| token.to_owned(), percent_text)
                    }
                    Some((pixels, true)) if relative(token) => format!("{pixels}px"),
                    _ => token.to_owned(),
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    Some(lists.join(", "))
}

pub(crate) fn resolve_keywords(
    style: &mut Style,
    parent: Option<&Style>,
    units: Option<&Lengths>,
) -> bool {
    let mut keyword = false;
    style.retain(|prop, value| {
        let inherit = match &**value {
            _ if prop.is_custom() => return true,
            "inherit" => true,
            "unset" | "revert" | "revert-layer" => prop.inherited(),
            "initial" => false,
            _ => return true,
        };
        keyword |= inherit && !prop.inherited();
        match parent
            .and_then(|parent| parent.value(prop))
            .filter(|_| inherit)
        {
            // A `currentcolor` of the parent stays a keyword for the child.
            Some(_) if parent.is_some_and(|parent| parent.current(prop)) => {
                *value = "currentcolor".into();
                true
            }
            Some(inherited) if !prop.inherited() => {
                let line = prop
                    .name()
                    .strip_suffix("-width")
                    .filter(|side| {
                        side.starts_with("border-") || matches!(*side, "outline" | "column-rule")
                    })
                    .and_then(|side| Prop::find(&format!("{side}-style")));
                *value = match line {
                    Some(line)
                        if parent
                            .and_then(|parent| parent.get(line))
                            .is_none_or(|style| matches!(style, "none" | "hidden")) =>
                    {
                        "0px".into()
                    }
                    _ => units
                        .and_then(|units| pixels(inherited, units))
                        .map_or_else(|| inherited.clone(), Rc::from),
                };
                true
            }
            Some(inherited) => {
                *value = inherited.clone();
                true
            }
            None => false,
        }
    });
    if style
        .get(Prop::COLOR)
        .is_some_and(|value| value.eq_ignore_ascii_case("currentcolor"))
    {
        match parent.and_then(|parent| parent.value(Prop::COLOR)) {
            Some(inherited) => style.insert(Prop::COLOR, inherited.clone()),
            None => {
                style.remove(Prop::COLOR);
            }
        }
    }
    let Some(current) = style.value(Prop::COLOR).cloned() else {
        return keyword;
    };
    style.retain_current(|prop, slot| {
        if prop.is_custom() || prop == Prop::COLOR || prop.inherited() {
            return (true, false);
        }
        if slot.eq_ignore_ascii_case("currentcolor") {
            *slot = current.clone();
            return (true, true);
        }
        let mut start = 0;
        let mut value = None::<String>;
        while let Some(index) = value.as_deref().unwrap_or(slot).as_bytes()[start..]
            .windows(12)
            .position(|window| window.eq_ignore_ascii_case(b"currentcolor"))
        {
            value
                .get_or_insert_with(|| slot.to_string())
                .replace_range(start + index..start + index + 12, &current);
            start += index + current.len();
        }
        if let Some(value) = value {
            *slot = value.into();
        }
        (true, false)
    });
    keyword
}
