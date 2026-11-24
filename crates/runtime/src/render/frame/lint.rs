use super::super::PageState;
use crate::css;
use crate::css::properties::Prop;
use crate::dom::Document;
use crate::style::ComputedStyles;
use std::rc::Rc;

// Warnings for the CSS of changed nodes that the page can't draw as written.
#[inline(never)]
pub(super) fn lint(
    doc: &Document,
    dom: &mut PageState,
    styles: &ComputedStyles,
    changed: &[usize],
) -> Vec<String> {
    let mut warnings = Vec::new();
    dom.styles.linted = true;
    if dom.styles.checked.len() > 65536 {
        dom.styles.checked.clear();
        dom.styles.checked_values.clear();
    }
    for name in css::sheet::parser::DROPPED.take() {
        if name.contains(':') {
            if dom.styles.warned.insert(name.clone()) {
                warnings.push(format!(
                    "Unsupported CSS `{name}`: the value is not supported."
                ));
            }
        } else if dom.styles.warned.insert(format!("@{name}")) {
            warnings.push(format!(
                "Unsupported CSS at-rule `@{name}`: its rules are ignored."
            ));
        }
    }
    for &id in changed {
        let (node, style) = (&doc.nodes[id], &styles.values[id]);
        if node.tag.starts_with('#') {
            continue;
        }
        let pseudos = styles.text_pseudos.get(&id);
        for (index, (pseudo, values)) in [
            ("", Some(&**style)),
            (
                "::first-letter",
                pseudos.and_then(|values| values[1].as_ref()),
            ),
            (
                "::first-line",
                pseudos.and_then(|values| values[2].as_ref()),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let Some(values) = values else { continue };
            for (key, value) in values.own() {
                // A custom property takes any value, and an animated one takes values between those of
                // its keyframes. Both can change each frame.
                if key.is_custom()
                    || index == 0 && dom.styles.motion.animates(id, styles, key)
                    || !pseudo.is_empty() && style.value(key) == Some(value)
                    // Most values are the ones of the last style, so their address finds them first.
                    || dom
                        .styles.checked_values
                        .insert(
                            (Rc::as_ptr(value).cast::<u8>() as usize, index as u8, key),
                            value.clone(),
                        )
                        .is_some()
                    || !dom.styles.checked.insert((index as u8, key, value.clone()))
                {
                    continue;
                }
                let (value, name) = (&**value, key.name());
                let reason = match key {
                    Prop::LINE_HEIGHT | Prop::VERTICAL_ALIGN if !pseudo.is_empty() => {
                        "line height and baseline can't change inside a line of text"
                    }
                    Prop::TRANSFORM_STYLE if value == "preserve-3d" => {
                        "3D children are flattened into the parent's plane"
                    }
                    Prop::CLIP_PATH if value.contains("shape(") => "shape() is not supported",
                    Prop::FILTER if value.contains("url(") && !value.contains("url(#") => {
                        "only filters defined in the page are supported"
                    }
                    _ if name.ends_with("text-size-adjust")
                        || name.ends_with("print-color-adjust")
                        || name.starts_with("page-break-")
                        || name.starts_with("break-")
                        || matches!(
                            name,
                            "orphans" | "widows" | "speak" | "forced-color-adjust" | "touch-action"
                        ) =>
                    {
                        continue;
                    }
                    _ => match css::value::grammar::supports(name, value) {
                        None => "the property is not supported",
                        Some(false) => "the value is not supported",
                        Some(true) => continue,
                    },
                };
                let unknown = reason == "the property is not supported";
                if dom.styles.warned.insert(if unknown {
                    format!("{pseudo}{name}")
                } else {
                    format!("{pseudo}{name}:{value}")
                }) {
                    let mut element = node.tag.clone();
                    if let Some(value) = node.attrs.get("id") {
                        element = format!("{element}#{value}");
                    }
                    for class in node
                        .attrs
                        .get("class")
                        .into_iter()
                        .flat_map(|value| value.split_whitespace())
                        .take(3)
                    {
                        element = format!("{element}.{class}");
                    }
                    warnings.push(format!(
                        "Unsupported CSS `{name}: {value}` on <{element}>{pseudo}: {reason}."
                    ));
                }
            }
        }
    }
    warnings
}
